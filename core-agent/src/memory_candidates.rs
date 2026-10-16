// core-agent/src/memory_candidates.rs
// 记忆候选抽取（S1 第二切片，2026-10-15）—— 从回合里抽「值得记住的事实」，**只落账本**。
//
// 定位（`docs/archive/agent-feature-backlog.md` §4.1.1 五类本地决策之一）：**结构化抽取**，
// 「不负责总结成文」。本模块**只抽候选**、**绝不**写 `MEMORY.md`、**绝不**改任何 prompt /
// `system` / history / 上下文块、**不做任何注入** ⇒ **开关开与关，可见行为逐字节不变**
// （只有决策账本多出一种 `kind`）。摊销口径与 S1 首切片**同一套**（每 K 轮 + 压缩事件，
// 复用 `decide::amortize_k()` 与既有接线点）——**不新造计时器 / 不新造第二套账本**
// （`docs/forbidden.md` §禁用 62）。
//
// fail-open：超时 / 端点不可用 / 解析失败 / **输出不合 schema** ⇒ **丢弃本次候选**
// （[`Extraction::fail_open`]，`facts` 为空 + `fallback=true`），**绝不影响主流程**；调用方按
// S1 的同口径记一条 `fallback=true` 的账本行（留痕比静默更有用，且保住「恰好 N 次触发」不变量）。
//
// **隐私边界（硬）**：候选是**对用户内容的抽取** ⇒ 账本只落**可追溯引用**（`turn` / `session_id`
// / 输入 `hash`）与**每个候选的 hash + 长度**，**绝不落原文**（原文留在 `chat.db` / history，
// 靠引用回查）。本模块的 `facts` **只在内存里流转**，交给调用方换算成 hash + 长度后即丢弃。

use std::time::Duration;

use serde_json::Value;

use crate::decide;
use crate::log;

/// 本切片**专属**的抽取 system prompt。
///
/// **不复用** `decide::VERDICT_GRAMMAR` 那套二元判定（它是 `RUN`/`SKIP`，本切片要的是
/// **结构化抽取**）；口径同 `docs/archive/agent-feature-backlog.md` §4.1.1 —— 只抽「值得记住的
/// 事实」、不负责总结成文。四类纳入 + 显式排除清单（含**禁写凭据**）与复盘 fork 的
/// `REVIEW_SYSTEM` 同源，但**本切片只输出 JSON 候选、不做任何写入**。
const EXTRACT_SYSTEM: &str = r#"You extract durable long-term facts from an assistant conversation turn, for future long-term memory.
Output STRICT JSON only: {"facts":["...","..."]}. If nothing is worth remembering, output {"facts":[]}.
Only include durable facts: user preferences and work style, project conventions and rules, environment/toolchain facts, background and decisions of long-term tasks.
Do NOT include: one-off task details, play-by-play progress, anything already readable from the repo/files, small talk, and NEVER credentials/keys/passwords.
Each fact: one short self-contained sentence, no leading dash."#;

/// 抽取输出的 **GBNF 语法约束**（llama.cpp 请求体的 `grammar` 字段，**本切片专属**）。
///
/// 作用：把输出**锁死**成 `{"facts":["...",...]}` 的严格 JSON —— 模型再想自由作答也吐不出
/// 散文（实测见下方「选型」）。**走请求体、不走启动参数**：无需改插件 `local-model` 的
/// `sidecar.args`，插件版本与 `build-plugins.ps1` 均不动（§禁用 62 —— 约束只此一处）。
///
/// **选型（GBNF vs `response_format: json_schema`，2026-10-15 实测）**：两者**都生效**
/// （同一条样例两种约束下输出逐字节相同、均为合法 `{"facts":[...]}`）；选 **GBNF** 的理由 ——
/// ① 与 S1（`decide.rs` 的 `grammar` 字段）**同一条通道、同一个概念**（`docs/code-standards.md`
/// §规范 2 单一真相源：约束本地输出的机制只此一种），不留第二套；② GBNF 是**客户端直接给语法**，
/// 不依赖服务端把 `json_schema` 转译成语法的实现是否逐版本一致；③ 请求体更小、无 schema 漂移。
/// 约束**确被服务端应用**（非静默忽略）的对照证据（2026-10-15 真机实测）：给一条「只能用散文、
/// 绝不输出 JSON」的系统提示时，**不带本约束**回整段散文、**带上本约束**被强制回合法 JSON。
const FACTS_GRAMMAR: &str = r#"root ::= "{" ws "\"facts\"" ws ":" ws arr ws "}"
arr ::= "[" ws ( str ( ws "," ws str )* )? ws "]"
str ::= "\"" ( [^"\\\x7F\x00-\x1F] | "\\" ( ["\\/bfnrt] | "u" [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] ) )* "\""
ws ::= [ \t\r\n]*"#;

/// 单条候选的字符上限（超长则截断）—— 防单条候选异常长撑爆账本引用与后续消费。
const MAX_FACT_CHARS: usize = 300;

/// 单次抽取的**硬上限**：**故意长于** [`decide::LOCAL_DECIDE_TIMEOUT`]（1500ms）。
///
/// 为什么不沿用 1500ms（2026-10-15 真机实测）：抽取要生成**几十个 token** 的 JSON 数组，而判定
/// 只要 **1 个 token** 的枚举 —— 两者耗时不同量级。实测（Qwen3-4B、纯 CPU、`-c 8192 -np 1`、
/// 稳定 34 输出 token、prompt 已缓存）：**min 1851ms / p50 1885ms / max 2414ms** ⇒ 1500ms 会把
/// **绝大多数**抽取判超时 ⇒ fail-open 丢候选（真机 `cargo test -- --ignored` 实测
/// `latency_ms=1503 fallback=true` 坐实）。
/// 取 **5s**：约 2.6× p50，覆盖冷启 / 抖动；仍是**硬上限**（超时即 fail-open，绝不拖住主流程）。
pub const LOCAL_EXTRACT_TIMEOUT: Duration = Duration::from_millis(5_000);

/// 一次抽取的结果。**`facts` 只在内存流转**，落账本时只取 hash + 长度（见文件头隐私边界）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extraction {
    /// 抽取到的候选事实文本（未截断前的原文；**绝不落账本**）。
    pub facts: Vec<String>,
    /// 是否走了 fail-open 后备路径（超时 / 端点不可用 / 解析失败 / 不合 schema）。
    pub fallback: bool,
    /// 机器可读的原因码（进账本 / 日志；**不要塞中文散文**）。
    pub reason: &'static str,
}

impl Extraction {
    /// 后端给出的结果（成功解析出 schema）。
    fn ok(facts: Vec<String>) -> Self {
        let reason = if facts.is_empty() { "empty-facts" } else { "ok" };
        Extraction { facts, fallback: false, reason }
    }
    /// **fail-open 的唯一出口**：任何故障都归到这里 ⇒ 丢弃本次候选（空 + `fallback`）。
    fn fail_open() -> Self {
        Extraction { facts: Vec::new(), fallback: true, reason: "fail-open" }
    }
    /// **busy-skip 的唯一出口**：本地模型通道忙（另一个本地请求在飞）⇒ **立即跳过本次**（不排队）。
    /// 与 `fail_open` 同形（空 + `fallback`），只把原因码换成 `busy-skip` 供事后归因。
    pub fn busy_skip() -> Self {
        Extraction { facts: Vec::new(), fallback: true, reason: "busy-skip" }
    }
}

/// 抽取后端（实现者只回 `Result`）：`Ok` = 已解析出合法 schema 的候选列表（含空列表）；
/// `Err` = 我这层坏了（没起 / 超时 / 输出解析不了 / 不合 schema），由 [`extract`] 折成 fail-open。
pub trait Extractor {
    fn extract(&self, input: &str) -> Result<Vec<String>, String>;
}

/// 本地模型后端（形态同 `decide::LocalHttp`：OpenAI 兼容端点、回环、显式禁代理）。
pub struct LocalExtract {
    cfg: decide::LocalHttpConfig,
}

/// 给「打本地端点的抽取后端」套上**本地模型通道**的进程级非阻塞互斥（判定 / 抽取 / 预热共用
/// **同一把锁**，见 `decide::try_acquire_local_channel`）：拿不到锁 ⇒ `Err(`[`decide::BUSY_SKIP`]`)`
/// （由 [`extract`] 折成 `busy-skip`，**立即跳过、绝不排队**）。生产里包住 [`LocalExtract`]。
pub struct ChannelGated<E> {
    inner: E,
}

impl<E> ChannelGated<E> {
    pub fn new(inner: E) -> Self {
        ChannelGated { inner }
    }
}

impl<E: Extractor> Extractor for ChannelGated<E> {
    fn extract(&self, input: &str) -> Result<Vec<String>, String> {
        // ⚠️ 先取锁再打端点；拿不到**立即** return Err（`ok_or_else` 不做任何等待）。
        let _guard = decide::try_acquire_local_channel().ok_or_else(|| decide::BUSY_SKIP.to_string())?;
        self.inner.extract(input)
    }
}

impl LocalExtract {
    pub fn new(cfg: decide::LocalHttpConfig) -> Self {
        // 抽取自有**更宽但仍硬**的超时上限（理由见 [`LOCAL_EXTRACT_TIMEOUT`]）：HTTP client 的
        // `timeout` 与第二道闸都按它走，**不沿用判定的 1500ms**。
        LocalExtract { cfg: decide::LocalHttpConfig { timeout: LOCAL_EXTRACT_TIMEOUT, ..cfg } }
    }
}

impl Extractor for LocalExtract {
    fn extract(&self, input: &str) -> Result<Vec<String>, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(self.cfg.timeout)
            // 与 `decide::LocalHttp` 同因：这是**回环**端点，reqwest 在 Windows 上会读系统代理
            // 且不认绕过表 ⇒ 必须显式禁代理（否则本地端点被塞给代理、连不上）。
            .no_proxy()
            .build()
            .map_err(|e| format!("建 HTTP client 失败：{e}"))?;
        let url = format!("{}/v1/chat/completions", self.cfg.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.cfg.model,
            "messages": [
                { "role": "system", "content": EXTRACT_SYSTEM },
                { "role": "user", "content": input },
            ],
            // 抽取的候选条数很少、每条很短 ⇒ 256 token 足够；温度 0 求确定性。
            "max_tokens": 256,
            "temperature": 0.0,
            // **GBNF 输出约束**（见 `FACTS_GRAMMAR`）：把输出锁死成严格 JSON。
            "grammar": FACTS_GRAMMAR,
        });
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| format!("连不上本地模型（{url}）：{e}"))?;
        if !resp.status().is_success() {
            return Err(format!("本地模型 HTTP {}", resp.status()));
        }
        let v: Value = resp.json().map_err(|e| format!("本地模型响应不是 JSON：{e}"))?;
        let content = v
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .ok_or_else(|| "响应里没有 choices[0].message.content".to_string())?;
        parse_facts(content)
    }
}

/// 统一入口：**fail-open 的落点**，也是唯一会写日志的地方。
///
/// 空输入 ⇒ 空候选（没有可抽的内容，**不是故障**、不带 fallback）；否则过第二道超时闸
/// [`LOCAL_EXTRACT_TIMEOUT`] 问后端，后端 `Err` / 超时 ⇒ 丢弃候选。
pub fn extract(e: &dyn Extractor, input: &str) -> Extraction {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Extraction::ok(Vec::new());
    }
    match decide::within_deadline(LOCAL_EXTRACT_TIMEOUT, || e.extract(trimmed)) {
        Ok(facts) => {
            log::info(format!("记忆候选抽取：{} 条（只落账本，不改任何注入）", facts.len()));
            Extraction::ok(facts)
        }
        Err(err) if err == decide::BUSY_SKIP => {
            // 本地通道忙（判定 / 预热在飞）⇒ **立即跳过本次抽取**，绝不排队等待。
            log::warn("记忆候选抽取：本地通道忙，立即跳过本次（不排队）");
            Extraction::busy_skip()
        }
        Err(err) => {
            log::warn(format!("记忆候选抽取后端故障，按 fail-open 丢弃本次候选（{err}）"));
            Extraction::fail_open()
        }
    }
}

/// 把模型输出解析成候选列表。**严格要求 schema**（`{"facts": [<string>...]}`）：
/// 顶层须是含 `facts` 数组的对象、数组元素须**全是字符串**；任何不符（含 JSON 解析失败）
/// ⇒ `Err`（⇒ 上层 fail-open）。空白候选被剔除；超长候选按 [`MAX_FACT_CHARS`] 截断。
fn parse_facts(content: &str) -> Result<Vec<String>, String> {
    let v: Value =
        serde_json::from_str(content.trim()).map_err(|e| format!("候选输出不是合法 JSON：{e}"))?;
    let arr = v
        .get("facts")
        .and_then(Value::as_array)
        .ok_or_else(|| "候选输出不合 schema：缺 facts 数组".to_string())?;
    let mut out: Vec<String> = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item
            .as_str()
            .ok_or_else(|| "候选输出不合 schema：facts 元素不是字符串".to_string())?;
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        out.push(log::truncate_chars(s, MAX_FACT_CHARS));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 恒失败的桩（端点不可达）。
    struct Boom;
    impl Extractor for Boom {
        fn extract(&self, _i: &str) -> Result<Vec<String>, String> {
            Err("llama-server 没起".into())
        }
    }
    /// 恒成功的桩（固定候选）。
    struct Fixed(Vec<&'static str>);
    impl Extractor for Fixed {
        fn extract(&self, _i: &str) -> Result<Vec<String>, String> {
            Ok(self.0.iter().map(|s| s.to_string()).collect())
        }
    }

    /// 后端故障 ⇒ **丢弃候选 + 标 fallback**，绝不 panic / 上抛（这是本模块的第一纪律）。
    #[test]
    fn backend_failure_fails_open_to_empty() {
        let out = extract(&Boom, "随便一段回合");
        assert!(out.facts.is_empty(), "故障必须丢弃候选");
        assert!(out.fallback, "必须标出这是后备路径");
        assert_eq!(out.reason, "fail-open");
    }

    /// 空输入 ⇒ 空候选、**不带 fallback**（没有可抽的内容不算故障）。
    #[test]
    fn empty_input_is_empty_without_fallback() {
        let out = extract(&Boom, "   \n\t ");
        assert!(out.facts.is_empty());
        assert!(!out.fallback, "空输入不是故障，不该标 fallback");
        assert_eq!(out.reason, "empty-facts");
    }

    /// 正常抽取：候选原样返回、`fallback=false`；空列表也走 `ok`（`reason=empty-facts`）。
    #[test]
    fn successful_extraction_passes_through() {
        let out = extract(&Fixed(vec!["用户偏好 4 空格缩进"]), "x");
        assert_eq!(out.facts, vec!["用户偏好 4 空格缩进"]);
        assert!(!out.fallback);
        assert_eq!(out.reason, "ok");

        let empty = extract(&Fixed(vec![]), "x");
        assert!(empty.facts.is_empty());
        assert!(!empty.fallback, "模型合法回空列表 ≠ 故障");
        assert_eq!(empty.reason, "empty-facts");
    }

    /// schema 解析：合法 JSON（含空白/缩进）可解析；空串 / 缺 facts / 元素非字符串 / 非 JSON **全 Err**。
    #[test]
    fn parse_facts_enforces_schema() {
        assert_eq!(parse_facts(r#"{"facts":["a","b"]}"#).unwrap(), vec!["a", "b"]);
        // GBNF 实测会带缩进换行 ⇒ 必须容忍空白。
        assert_eq!(
            parse_facts("{\n  \"facts\": [\n    \"x\"\n  ]\n}").unwrap(),
            vec!["x"]
        );
        assert!(parse_facts(r#"{"facts":[]}"#).unwrap().is_empty());
        // 空白候选被剔除。
        assert_eq!(parse_facts(r#"{"facts":["  ","a"]}"#).unwrap(), vec!["a"]);
        // 不合 schema 的四种：全 Err（⇒ 上层 fail-open）。
        assert!(parse_facts("").is_err(), "空串");
        assert!(parse_facts(r#"{"nope":[]}"#).is_err(), "缺 facts");
        assert!(parse_facts(r#"{"facts":"a"}"#).is_err(), "facts 不是数组");
        assert!(parse_facts(r#"{"facts":[1,2]}"#).is_err(), "元素非字符串");
        assert!(parse_facts("not json at all").is_err(), "非 JSON");
    }

    /// 超长候选被截断（防异常长候选）。
    #[test]
    fn parse_facts_truncates_overlong_candidates() {
        let long = "字".repeat(MAX_FACT_CHARS + 50);
        let got = parse_facts(&format!(r#"{{"facts":["{long}"]}}"#)).unwrap();
        assert_eq!(got.len(), 1);
        assert!(got[0].chars().count() <= MAX_FACT_CHARS + 16, "须被截断：{}", got[0].chars().count());
    }

    /// `FACTS_GRAMMAR` 必须锁定 `facts` 字段名（否则约束会与解析口径不符）。
    #[test]
    fn facts_grammar_pins_the_facts_key() {
        assert!(FACTS_GRAMMAR.contains("\\\"facts\\\""), "grammar 必须锁定 facts 键名");
        assert!(FACTS_GRAMMAR.contains("root"), "grammar 必须有 root 规则");
    }

    /// 抽取的硬上限**必须严格长于**判定上限 —— 抽取要生成几十个 token，判定只要 1 个；
    /// 沿用 1500ms 会把绝大多数抽取判超时（实测见 [`LOCAL_EXTRACT_TIMEOUT`] 注释）。
    #[test]
    fn extract_timeout_exceeds_the_decide_timeout() {
        assert!(LOCAL_EXTRACT_TIMEOUT > decide::LOCAL_DECIDE_TIMEOUT, "抽取上限必须宽于判定上限");
    }

    /// 端点连不上（真发一次 HTTP）⇒ fail-open、不 panic（用 `127.0.0.1:1` 保证立即被拒）。
    #[test]
    fn local_extract_unreachable_fails_open() {
        let e = LocalExtract::new(decide::LocalHttpConfig {
            base_url: "http://127.0.0.1:1".into(),
            model: "local".into(),
            timeout: Duration::from_millis(500),
        });
        let out = extract(&e, "用户偏好 4 空格缩进");
        assert!(out.facts.is_empty());
        assert!(out.fallback);
    }

    /// 本地模型通道忙（另一个本地请求在飞）⇒ 抽取**立即** busy-skip（不排队、不 panic）。
    #[test]
    fn gated_extract_busy_skips_without_waiting() {
        use std::time::Instant;
        // 通道锁是进程级静态量 ⇒ 与其它碰锁的单测串行（见 decide::TEST_CHANNEL_LOCK）。
        let _serial = decide::TEST_CHANNEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        /// 「若被真正调用就会睡 5s」的桩 —— 证明是**被跳过**而非**排队等待**。
        struct SlowExtract;
        impl Extractor for SlowExtract {
            fn extract(&self, _i: &str) -> Result<Vec<String>, String> {
                std::thread::sleep(Duration::from_secs(5));
                Ok(vec!["should-not-be-called".into()])
            }
        }
        let guard = decide::try_acquire_local_channel().expect("首个请求应拿到通道");
        let e = ChannelGated::new(SlowExtract);
        let t0 = Instant::now();
        let out = extract(&e, "用户偏好 4 空格缩进");
        let used = t0.elapsed();
        assert!(used < Duration::from_secs(1), "持锁时必须立即返回、不排队（用了 {used:?}）");
        assert!(out.facts.is_empty(), "busy-skip 丢弃候选");
        assert!(out.fallback, "busy-skip 必须标 fallback");
        assert_eq!(out.reason, "busy-skip");
        drop(guard);
        // 通道释放后：同一个 gate 放行到后端，正常抽到候选。
        let ok = extract(&ChannelGated::new(Fixed(vec!["x"])), "y");
        assert_eq!(ok.facts, vec!["x"]);
        assert!(!ok.fallback, "通道空闲时不产生 busy-skip");
    }
}
