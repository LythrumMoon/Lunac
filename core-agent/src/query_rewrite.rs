// core-agent/src/query_rewrite.rs
// 检索查询改写（S1 第三切片，2026-10-24）—— 把一条检索查询扩成「额外的关键词」。
//
// 定位（`docs/archive/agent-feature-backlog.md` §4.1.1 五类本地决策之一）：**模板化改写**，
// 只产出检索用的关键词 / 同义表达，**不做网页摘要、不回答、不生成任何可见文本**。
//
// **唯一的消费点 = `MemorySearch` 工具**（长期记忆的只读检索内核 `memory_search.rs`）。改写结果
// 只用于**补充召回**：原查询先检索、命中不足 `limit` 时再用扩展词补，**原命中一条都不减少、
// 顺序也不动**（见 `main.rs` 的 `merge_expansion_hits`）。这样即便本地模型改写得离谱，最坏
// 也只是「没补充到东西」，绝不会把原本能查到的条目挤出去。
//
// **硬约束（与 S1 其余切片同源）**：
//   · **不改任何注入**：不碰 `system` / history / 上下文块 / `MEMORY.md` ⇒ 开关开与关，
//     主请求逐字节不变（docs/forbidden.md §禁用 45、docs/code-standards.md §规范 6）；
//   · **默认关 / 未配端点 / 故障 ⇒ 与不接线逐字节一致**：开关复用 `decide::enabled()`，
//     端点复用 `decide::LocalHttpConfig::from_env()`，任何故障都折成「空扩展」（fail-open）；
//   · **本地模型通道**：与判定 / 抽取 / 预热**共用** `decide::try_acquire_local_channel()`
//     （`llama-server` 是 `-np 1` 单并发槽）；拿不到锁**立即跳过本次**，绝不排队。
//
// **不做「每 K 轮」摊销**：本切片的调用频次由**模型是否调用 `MemorySearch`** 决定，天然低频；
// 再叠一层 K 摊销只会让「模型明确要查记忆」这一轮拿不到扩展词。摊销口径的单一真相源仍在
// `decide::amortize_k()`（判定 / 抽取用），本模块不新造计时器。
//
// **隐私**：查询可能含用户敏感信息 ⇒ 决策账本只落 `hash` + 长度，绝不落原文（同
// `memory_candidates.rs` / `main.rs` 的 `MemorySearch` 日志口径）。

use std::time::Duration;

use serde_json::Value;

use crate::decide;
use crate::log;

/// 本切片**专属**的改写提示词。
///
/// 只要求「额外关键词」：同义词 / 近义表达 / 专名缩写展开。**不要求复述原查询** ——
/// 原查询本来就会被单独检索一次（见文件头「补充召回」）。
const REWRITE_SYSTEM: &str = r#"You expand a search query into extra keywords for retrieving long-term memory entries.
Output STRICT JSON only: {"terms":["...","..."]}. If no useful extra keyword exists, output {"terms":[]}.
The retrieval is plain substring matching, so terms must be SHORT keywords that could appear verbatim in a stored entry: Chinese 1-4 characters, English one word. Never emit phrases with modifiers (e.g. emit "规范", not "代码规范").
Rules: at most 5 terms; synonyms, alternate names and abbreviations are welcome; never answer the query and never invent facts."#;

/// 改写输出的 **GBNF 语法约束**（llama.cpp 请求体的 `grammar` 字段）。
///
/// 与 `memory_candidates::FACTS_GRAMMAR` 形状相同、**键名不同**（`terms` vs `facts`）——
/// 两者服务的是不同 schema、不同语义（检索词 vs 记忆事实），故各自一份；共用一份会让
/// 「改一处 schema 顺手改坏另一处」。约束走请求体、不走启动参数（§禁用 62）。
const TERMS_GRAMMAR: &str = r#"root ::= "{" ws "\"terms\"" ws ":" ws arr ws "}"
arr ::= "[" ws ( str ( ws "," ws str )* )? ws "]"
str ::= "\"" ( [^"\\\x7F\x00-\x1F] | "\\" ( ["\\/bfnrt] | "u" [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] ) )* "\""
ws ::= [ \t\r\n]*"#;

/// 单次改写最多采纳几条扩展词（再多会稀释检索排序）。
const MAX_TERMS: usize = 5;

/// 补完中文 2 字片段后的总上限（片段只用于召回，见 [`add_cjk_bigrams`]）。
const MAX_TERMS_AFTER_CJK: usize = 8;

/// 单条扩展词的字符上限（超长则截断）—— 关键词不该是长句。
const MAX_TERM_CHARS: usize = 40;

/// 单次改写的**硬上限**。
///
/// **不另造数值**：直接取 [`decide::LOCAL_DECIDE_TIMEOUT`]（单一真相源）。改写的冷 prefill
/// 与判定同量级（实测 187 token / ~103 tok/s ≈ 1.8s），只是多生成几条短词 —— 3000ms 覆盖
/// 「冷 prefill + 几条短词」，热机后同一 system 命中前缀缓存、实际远低于此。超时即 fail-open。
pub const LOCAL_REWRITE_TIMEOUT: Duration = decide::LOCAL_DECIDE_TIMEOUT;

/// 一次改写的产物。`terms` 只在内存里流转（账本只落 hash + 长度）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    /// 额外关键词（已去重 / 已截断 / 已限条数）。
    pub terms: Vec<String>,
    /// 是否走了 fail-open 后备路径（超时 / 端点不可用 / 解析失败 / 不合 schema）。
    pub fallback: bool,
    /// 机器可读的原因码（进账本 / 日志；**不要塞中文散文**）。
    pub reason: &'static str,
}

impl Expansion {
    /// 正常结果（含「模型合法地回了空列表」）。
    fn ok(terms: Vec<String>) -> Self {
        let reason = if terms.is_empty() { "empty-terms" } else { "ok" };
        Expansion { terms, fallback: false, reason }
    }
    /// **fail-open 的唯一出口**：任何故障 ⇒ 空扩展（检索照原查询走）。
    fn fail_open() -> Self {
        Expansion { terms: Vec::new(), fallback: true, reason: "fail-open" }
    }
    /// **busy-skip 的唯一出口**：本地通道忙 ⇒ 立即跳过（不排队），与 fail-open 同形。
    pub fn busy_skip() -> Self {
        Expansion { terms: Vec::new(), fallback: true, reason: "busy-skip" }
    }
}

/// 改写后端（实现者只回 `Result`）：`Ok` = 已解析出合法 schema 的关键词列表（含空列表）；
/// `Err` = 我这层坏了（没起 / 超时 / 输出解析不了 / 不合 schema），由 [`expand`] 折成 fail-open。
pub trait Rewriter {
    fn rewrite(&self, query: &str) -> Result<Vec<String>, String>;
}

/// 本地模型后端（形态同 `decide::LocalHttp` / `memory_candidates::LocalExtract`：OpenAI 兼容
/// 端点、回环、显式禁代理）。
pub struct LocalRewrite {
    cfg: decide::LocalHttpConfig,
}

impl LocalRewrite {
    pub fn new(cfg: decide::LocalHttpConfig) -> Self {
        // 本切片**沿用判定那条上限**（理由见 [`LOCAL_REWRITE_TIMEOUT`]），不另造数值。
        LocalRewrite { cfg: decide::LocalHttpConfig { timeout: LOCAL_REWRITE_TIMEOUT, ..cfg } }
    }
}

impl Rewriter for LocalRewrite {
    fn rewrite(&self, query: &str) -> Result<Vec<String>, String> {
        // ⚠️ 先取锁再打端点：本地通道（判定 / 抽取 / 预热 / 改写共用）忙时**立即** Err。
        let _guard = decide::try_acquire_local_channel()
            .ok_or_else(|| decide::BUSY_SKIP.to_string())?;
        let client = reqwest::blocking::Client::builder()
            .timeout(self.cfg.timeout)
            // 与判定 / 抽取同因：回环端点，reqwest 在 Windows 上会读系统代理且不认绕过表。
            .no_proxy()
            .build()
            .map_err(|e| format!("建 HTTP client 失败：{e}"))?;
        let url = format!("{}/v1/chat/completions", self.cfg.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.cfg.model,
            "messages": [
                { "role": "system", "content": REWRITE_SYSTEM },
                { "role": "user", "content": query },
            ],
            // 5 条短词 ⇒ 64 token 足够；温度 0 求确定性。
            "max_tokens": 64,
            "temperature": 0.0,
            // **GBNF 输出约束**（见 `TERMS_GRAMMAR`）：把输出锁死成严格 JSON。
            "grammar": TERMS_GRAMMAR,
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
        parse_terms(content, query)
    }
}

/// 统一入口：**fail-open 的落点**，也是唯一会写日志的地方。
///
/// 空查询 ⇒ 空扩展（没有可改的内容，**不是故障**、不带 fallback）；否则过第二道超时闸
/// [`LOCAL_REWRITE_TIMEOUT`] 问后端，后端 `Err` / 超时 ⇒ 空扩展。
pub fn expand(r: &dyn Rewriter, query: &str) -> Expansion {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Expansion::ok(Vec::new());
    }
    match decide::within_deadline(LOCAL_REWRITE_TIMEOUT, || r.rewrite(trimmed)) {
        Ok(terms) => {
            log::info(format!("检索查询改写：{} 条扩展词（只用于补充召回）", terms.len()));
            Expansion::ok(terms)
        }
        Err(err) if err == decide::BUSY_SKIP => {
            log::warn("检索查询改写：本地通道忙，立即跳过本次（不排队）");
            Expansion::busy_skip()
        }
        Err(err) => {
            log::warn(format!("检索查询改写后端故障，按 fail-open 用原查询（{err}）"));
            Expansion::fail_open()
        }
    }
}

/// 把模型输出解析成扩展词列表。**严格要求 schema**（`{"terms": [<string>...]}`）。
///
/// 归一：trim → 剔空 → 截断到 [`MAX_TERM_CHARS`] → 去掉与**原查询**重复的词（小写子串判断：
/// 原查询里已有的词再补一遍不会带来新召回）→ 小写去重 → 最多 [`MAX_TERMS`] 条。
/// 任何 schema 不符（含 JSON 解析失败）⇒ `Err`（⇒ 上层 fail-open）。
fn parse_terms(content: &str, query: &str) -> Result<Vec<String>, String> {
    let v: Value =
        serde_json::from_str(content.trim()).map_err(|e| format!("改写输出不是合法 JSON：{e}"))?;
    let arr = v
        .get("terms")
        .and_then(Value::as_array)
        .ok_or_else(|| "改写输出不合 schema：缺 terms 数组".to_string())?;
    let lower_query = query.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for item in arr {
        let s = item
            .as_str()
            .ok_or_else(|| "改写输出不合 schema：terms 元素不是字符串".to_string())?;
        let s = log::truncate_chars(s.trim(), MAX_TERM_CHARS);
        if s.is_empty() {
            continue;
        }
        let low = s.to_lowercase();
        // 原查询里已经有的词 ⇒ 不重复补（它已经在第一遍检索里生效了）。
        if lower_query.contains(&low) {
            continue;
        }
        if seen.contains(&low) {
            continue;
        }
        seen.push(low);
        out.push(s);
        if out.len() >= MAX_TERMS {
            break;
        }
    }
    add_cjk_bigrams(&mut out, &lower_query);
    Ok(out)
}

/// 中文长词补 2 字片段（**确定性，不依赖模型**）。
///
/// 检索是**纯子串匹配**，而 4B 倾向输出「代码规范 / 风格要求」这类带修饰的短语，与存储条目
/// 里的「排版规范」并不互为子串 ⇒ 扩展词一条也召不回（2026-10-24 真机：`t7` 夹具上
/// 4 条扩展词仍 **0 命中**）。这里把每个 CJK 长词的滑窗 2 字片段补进去（已在原查询里的跳过），
/// 让「规范」这种能真正命中的短词进入召回。
///
/// 上限 [`MAX_TERMS_AFTER_CJK`]：片段只是召回用的，不该无限膨胀（排序里命中词数会被它抬高）。
fn add_cjk_bigrams(out: &mut Vec<String>, lower_query: &str) {
    let mut extra: Vec<String> = Vec::new();
    for t in out.iter() {
        let chars: Vec<char> = t.chars().filter(|c| !c.is_whitespace()).collect();
        if chars.len() < 3 || !chars.iter().any(|c| is_cjk(*c)) {
            continue;
        }
        for w in chars.windows(2) {
            if !w.iter().all(|c| is_cjk(*c)) {
                continue;
            }
            let g: String = w.iter().collect();
            let low = g.to_lowercase();
            if lower_query.contains(&low) || out.contains(&g) || extra.contains(&g) {
                continue;
            }
            extra.push(g);
        }
    }
    out.extend(extra);
    out.truncate(MAX_TERMS_AFTER_CJK);
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x4E00..=0x9FFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 恒失败的桩（端点不可达）。
    struct Boom;
    impl Rewriter for Boom {
        fn rewrite(&self, _q: &str) -> Result<Vec<String>, String> {
            Err("llama-server 没起".into())
        }
    }
    /// 恒成功的桩（固定扩展词）。
    struct Fixed(Vec<&'static str>);
    impl Rewriter for Fixed {
        fn rewrite(&self, _q: &str) -> Result<Vec<String>, String> {
            Ok(self.0.iter().map(|s| s.to_string()).collect())
        }
    }

    /// 后端故障 ⇒ **空扩展 + fallback**，绝不 panic / 上抛（检索照原查询走）。
    #[test]
    fn backend_failure_fails_open_to_no_expansion() {
        let out = expand(&Boom, "四空格缩进");
        assert!(out.terms.is_empty(), "故障必须不扩展");
        assert!(out.fallback, "必须标出这是后备路径");
        assert_eq!(out.reason, "fail-open");
    }

    /// 空查询 ⇒ 空扩展、**不带 fallback**（没有可改的内容不算故障）。
    #[test]
    fn empty_query_is_empty_without_fallback() {
        let out = expand(&Boom, "  \n\t ");
        assert!(out.terms.is_empty());
        assert!(!out.fallback, "空查询不是故障，不该标 fallback");
        assert_eq!(out.reason, "empty-terms");
    }

    /// 正常改写：扩展词原样返回、`fallback=false`；空列表也走 `ok`。
    #[test]
    fn successful_rewrite_passes_through() {
        let out = expand(&Fixed(vec!["indentation", "spaces"]), "缩进");
        assert_eq!(out.terms, vec!["indentation", "spaces"]);
        assert!(!out.fallback);

        let empty = expand(&Fixed(vec![]), "缩进");
        assert!(empty.terms.is_empty());
        assert!(!empty.fallback, "模型合法回空列表 ≠ 故障");
        assert_eq!(empty.reason, "empty-terms");
    }

    /// schema：合法 JSON（含 GBNF 常见的换行缩进）可解析；缺键 / 非数组 / 元素非字符串 / 非 JSON 全 Err。
    #[test]
    fn parse_terms_enforces_schema() {
        assert_eq!(parse_terms(r#"{"terms":["a","b"]}"#, "q").unwrap(), vec!["a", "b"]);
        assert_eq!(
            parse_terms("{\n  \"terms\": [\n    \"x\"\n  ]\n}", "q").unwrap(),
            vec!["x"]
        );
        assert!(parse_terms(r#"{"terms":[]}"#, "q").unwrap().is_empty());
        assert!(parse_terms("", "q").is_err(), "空串");
        assert!(parse_terms(r#"{"facts":[]}"#, "q").is_err(), "键名必须是 terms");
        assert!(parse_terms(r#"{"terms":"a"}"#, "q").is_err(), "terms 不是数组");
        assert!(parse_terms(r#"{"terms":[1,2]}"#, "q").is_err(), "元素非字符串");
        assert!(parse_terms("not json at all", "q").is_err(), "非 JSON");
    }

    /// 归一：剔空、去重（大小写不敏感）、去掉原查询里已有的词、截断、限 5 条。
    #[test]
    fn parse_terms_normalizes_and_caps() {
        // 空格子被剔除；`Rust` 与 `rust` 只留一个；`缩进` 已在原查询里 ⇒ 不重复补。
        let got = parse_terms(r#"{"terms":["  ","Rust","rust","缩进","spaces"]}"#, "四空格缩进").unwrap();
        assert_eq!(got, vec!["Rust", "spaces"]);

        // 超长被截断。
        let long = "字".repeat(MAX_TERM_CHARS + 30);
        let got = parse_terms(&format!(r#"{{"terms":["{long}"]}}"#), "q").unwrap();
        assert!(got[0].chars().count() <= MAX_TERM_CHARS + 16, "须被截断：{}", got[0].chars().count());

        // 条数封顶。
        let many: Vec<String> = (0..9).map(|i| format!("\"t{i}\"")).collect();
        let got = parse_terms(&format!(r#"{{"terms":[{}]}}"#, many.join(",")), "q").unwrap();
        assert_eq!(got.len(), MAX_TERMS, "最多采纳 MAX_TERMS 条");
    }

    /// `TERMS_GRAMMAR` 必须锁定 `terms` 键名（否则约束会与解析口径不符）。
    #[test]
    fn terms_grammar_pins_the_terms_key() {
        assert!(TERMS_GRAMMAR.contains("\\\"terms\\\""), "grammar 必须锁定 terms 键名");
        assert!(TERMS_GRAMMAR.contains("root"), "grammar 必须有 root 规则");
    }

    /// 改写上限**必须跟判定同一个值**（单一真相源；2026-10-24 起两者都取 `LOCAL_DECIDE_TIMEOUT`）。
    #[test]
    fn rewrite_timeout_follows_the_decide_timeout() {
        assert_eq!(
            LOCAL_REWRITE_TIMEOUT,
            decide::LOCAL_DECIDE_TIMEOUT,
            "改写上限必须引用判定的那个值，不得自成一套"
        );
    }

    /// 本地通道忙 ⇒ **立即** busy-skip（不排队、不 panic）；释放后同一路径恢复可用。
    #[test]
    fn local_rewrite_busy_skips_without_waiting() {
        use std::time::Instant;
        // 通道锁是进程级静态量 ⇒ 与其它碰锁的单测串行（见 decide::TEST_CHANNEL_LOCK）。
        let _serial = decide::TEST_CHANNEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = decide::LocalHttpConfig {
            base_url: "http://127.0.0.1:1".into(),
            model: "local".into(),
            timeout: Duration::from_millis(300),
        };
        let guard = decide::try_acquire_local_channel().expect("首个请求应拿到通道");
        let t0 = Instant::now();
        let out = expand(&LocalRewrite::new(cfg.clone()), "四空格缩进");
        let used = t0.elapsed();
        assert!(used < Duration::from_secs(1), "持锁时必须立即返回、不排队（用了 {used:?}）");
        assert!(out.terms.is_empty());
        assert!(out.fallback);
        assert_eq!(out.reason, "busy-skip");
        drop(guard);
        // 通道释放后：走到真实 HTTP（端点不可达 ⇒ fail-open，但原因码不再是 busy-skip）。
        let out2 = expand(&LocalRewrite::new(cfg), "四空格缩进");
        assert_eq!(out2.reason, "fail-open", "通道空闲时应真的尝试后端");
    }

    /// 端点连不上（真发一次 HTTP）⇒ fail-open、不 panic（用 `127.0.0.1:1` 保证立即被拒）。
    #[test]
    fn local_rewrite_unreachable_fails_open() {
        // 它也会取本地通道锁（`LocalRewrite::rewrite` 先取锁再打端点）⇒ 同样要串行。
        let _serial = decide::TEST_CHANNEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let r = LocalRewrite::new(decide::LocalHttpConfig {
            base_url: "http://127.0.0.1:1".into(),
            model: "local".into(),
            timeout: Duration::from_millis(500),
        });
        let out = expand(&r, "四空格缩进");
        assert!(out.terms.is_empty());
        assert!(out.fallback);
    }

    /// **端到端**（要外部环境，故默认 `#[ignore]`）：设 `LUNAC_LOCAL_MODEL_URL` 后跑
    /// `cargo test --bin agent -- --ignored`。**未设环境变量时直接跳过**。
    #[test]
    #[ignore = "需要本地 llama-server：设 LUNAC_LOCAL_MODEL_URL 后跑 cargo test -- --ignored"]
    fn local_model_end_to_end_when_configured() {
        let Some(cfg) = decide::LocalHttpConfig::from_env() else {
            eprintln!("跳过：未设 LUNAC_LOCAL_MODEL_URL");
            return;
        };
        let out = expand(&LocalRewrite::new(cfg), "缩进风格");
        // 只硬断言「通路」：真的走通了本地模型（不是 fail-open）。
        // 不把具体扩展词钉进 CI —— 4B 是概率模型。
        assert!(!out.fallback, "不该走 fail-open（说明没连上 / 没解析出来）：{out:?}");
        eprintln!("样例 缩进风格 ⇒ {out:?}");
    }

    /// 中文 2 字片段补齐：带修饰的短语要能拆出**可命中**的短词。
    ///
    /// 2026-10-24 真机（`t7`）：4B 只回「代码规范 / 风格要求」，而条目写的是「排版规范」——
    /// 子串检索一条也召不回。补齐后「规范」进入扩展词，命中恢复。
    #[test]
    fn cjk_bigrams_bridge_phrase_level_terms() {
        let out = parse_terms(r#"{"terms":["代码规范","风格要求"]}"#, "代码风格硬性要求").unwrap();
        assert!(
            out.contains(&"规范".to_string()),
            "必须补出能在「排版规范」里命中的短词：{out:?}"
        );
        assert!(out.iter().all(|t| t.chars().count() <= 4), "扩展词应保持短：{out:?}");
    }

    /// 纯英文扩展词不补片段（没有 CJK 滑窗可补）。
    #[test]
    fn cjk_bigrams_do_not_touch_ascii_terms() {
        let out = parse_terms(r#"{"terms":["indentation","tabs"]}"#, "code style").unwrap();
        assert_eq!(out, vec!["indentation", "tabs"]);
    }

    /// **收益探针（默认 `#[ignore]`）**：查询与记忆条目**没有任何字面重合**时，
    /// 原查询 0 命中、扩展词必须补到至少 1 条 —— 这是本切片唯一的价值主张。
    ///
    /// 2026-10-24 真机对照（`t7` 夹具）：关改写 **0/3** 命中；开改写两次「4 条扩展词」
    /// **仍 0 命中** ⇒ 本探针用来钉住并驱动这条短板。
    #[test]
    #[ignore = "需要本地 llama-server：设 LUNAC_LOCAL_MODEL_URL 后跑 cargo test -- --ignored"]
    fn local_rewrite_bridges_a_query_with_no_literal_overlap() {
        let Some(cfg) = decide::LocalHttpConfig::from_env() else {
            eprintln!("跳过：未设 LUNAC_LOCAL_MODEL_URL");
            return;
        };
        let query = "代码风格硬性要求";
        let mut bullets: Vec<String> = (1..=130)
            .map(|i| format!("归档条目 {i} ：占位旧记忆"))
            .collect();
        bullets.push("排版规范 TABS_POLICY：禁止 tab，统一用四个空格缩进。".to_string());
        assert!(
            crate::memory_search::search(&bullets, query, 5).is_empty(),
            "夹具前提：原查询必须 0 命中"
        );
        let out = expand(&LocalRewrite::new(cfg), query);
        eprintln!("扩展词 ⇒ {out:?}");
        assert!(!out.fallback, "{out:?}");
        assert!(!out.terms.is_empty(), "扩展词为空 ⇒ 补召回无从谈起");
        let hits = crate::memory_search::search(&bullets, &out.terms.join(" "), 5);
        assert!(!hits.is_empty(), "扩展词没补到任何条目：{out:?}");
    }
}
