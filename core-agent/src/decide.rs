// ── 「要不要把这条输入发给云模型」的决策边界（S1 第一步，2026-10-09）──────────
//
// 定位与落点见 `docs/agent-feature-backlog.md` §4.1.1 / §4.1.3：本地小模型**只参与决策**。
// 本文件刻意**不引任何模型**，先把三件事定死，后端再可插拔地插进来：
//
//   ① **fail-open**：决策层的任何故障（后端没起 / 超时 / 输出解析不了）一律 **Send**
//      （发送）并把 `fallback=true` 标出来。**绝不因为决策层坏了就把用户的话吞掉** ——
//      这是本条最不可回退的纪律（错误的两个方向里，「该发却不发」是 UX 事故）。
//   ② **后端可插拔**：`Decider` 是 trait。现在只有 `Heuristic`（纯规则桩，零依赖）；
//      形态 B 的本地模型（子进程 `llama-server` + GGUF，OpenAI 兼容端点）后续实现**同一个
//      trait**，调用方一行都不用改。
//   ③ **可观测**：每次决策返回 `Decision { action, reason, fallback }` —— 调用方据此落
//      日志与用量。`reason` 是**机器可读的原因码**（ASCII 键值），不是散文。
//
// ⚠️ **本模块只提供决策，不负责「谁在什么时候问」**：**当前未接线**（刻意的）——
// 调用点 = L10 桌宠常驻输入，核心 agent 接线另议。接线前本模块整体没有调用方，
// 故挂模块级 dead_code 豁免；**接线后删**（连同本条注释与下面那行豁免）。
#![allow(dead_code)]

use crate::log;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// 决策动作。**只有两档**：`Send` / `Skip`。
///
/// 「不确定」**不是**第三档 —— 它就是 `Send`，靠 `fallback` 标出来供事后归因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 发给云模型。
    Send,
    /// 不发（省一次云调用）。
    Skip,
}

/// 一次决策的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub action: Action,
    /// 机器可读的原因码（进日志 / 用量；**不要塞中文散文**）。
    pub reason: &'static str,
    /// 是否走了「后备路径」（后端故障）。**为真时 `action` 必为 `Send`。**
    pub fallback: bool,
}

impl Decision {
    /// 后端给出的「发」。
    pub fn send(reason: &'static str) -> Self {
        Decision { action: Action::Send, reason, fallback: false }
    }
    /// 后端给出的「不发」。
    pub fn skip(reason: &'static str) -> Self {
        Decision { action: Action::Skip, reason, fallback: false }
    }
    /// **fail-open 的唯一出口**：任何故障都归到这里，保证「拿不准就发」。
    pub fn allow_fallback(reason: &'static str) -> Self {
        Decision { action: Action::Send, reason, fallback: true }
    }
}

/// 决策后端。
///
/// **实现者只回 `Result`**：`Err` 表示「我这层坏了」（进程没起 / 超时 / 输出不是合法枚举），
/// 由 [`decide`] 统一折成 fail-open —— 后端**不许自己决定「故障时发不发」**，
/// 那样每个后端都会写一份不同的兜底策略，迟早漂移。
pub trait Decider {
    fn decide(&self, input: &str) -> Result<Decision, String>;
}

/// 统一入口：**fail-open 的落点**，也是唯一会写日志的地方。
///
/// 顺序：空输入 → `Skip`（不发也没内容可发）；否则问后端；后端 `Err` ⇒ 放行。
pub fn decide(d: &dyn Decider, input: &str) -> Decision {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        // 空输入不是「决策」——但走同一条返回口径，调用方不必特判。
        return Decision::skip("empty-input");
    }
    match d.decide(trimmed) {
        Ok(dec) => {
            log::info(format!(
                "decide: action={} reason={} fallback=false",
                action_str(dec.action),
                dec.reason
            ));
            dec
        }
        Err(e) => {
            // ⚠️ 这一行就是「绝不吞消息」的现场：后端坏了照样发。
            log::warn(format!("decide: 后端故障，按 fail-open 放行（{e}）"));
            Decision::allow_fallback("fail-open-backend-error")
        }
    }
}

fn action_str(a: Action) -> &'static str {
    match a {
        Action::Send => "send",
        Action::Skip => "skip",
    }
}

/// 桩后端（形态 A：纯规则 / 启发式）。**零依赖、零模型、可立即验证。**
///
/// 判据**刻意保守** —— 宁可多发也别吞。只有**明显不是给模型的**才 `Skip`：
///   · 以 `/` 开头 ⇒ 当成斜杠命令（`/compact` 那类会话内操作）；
///   · 不含任何字母 / 数字（纯符号、纯空白）⇒ 不是话。
/// 其余一律 `Send`。
pub struct Heuristic;

impl Heuristic {
    fn looks_like_command(s: &str) -> bool {
        s.starts_with('/')
    }
    /// 是否含字母或数字（`is_alphanumeric` 对 CJK 返回真 ⇒ 中文会被判成「有内容」）。
    fn has_letters(s: &str) -> bool {
        s.chars().any(|c| c.is_alphanumeric())
    }
}

impl Decider for Heuristic {
    fn decide(&self, input: &str) -> Result<Decision, String> {
        if Self::looks_like_command(input) {
            return Ok(Decision::skip("slash-command"));
        }
        if !Self::has_letters(input) {
            return Ok(Decision::skip("no-letters"));
        }
        Ok(Decision::send("heuristic-default"))
    }
}

/// 给「真后端」（本地模型）用的时限封装。
///
/// 为什么放在这里而不是各后端各写一份：**超时口径必须一致**，否则「多慢算坏」会各说各话。
/// 形态 B 的 `llama-server` 后端应当用它包住自己的 HTTP 调用，超时即 `Err`（⇒ fail-open）。
pub const LOCAL_DECIDE_TIMEOUT: Duration = Duration::from_millis(1500);

/// 运行 `f` 并给它一个截止时间。超时 / 耗时超限 ⇒ `Err`（由 [`decide`] 折成放行）。
///
/// ⚠️ **这不是真抢占**：它只能在 `f` 返回之后判「有没有超时」。对阻塞式 HTTP 调用，
/// 仍应由那次调用**自带**超时（reqwest 的 `timeout`）；本函数是**第二道闸**，
/// 用来兜住「调用本身没设超时」或「本地进程卡死但连接还在」这类情况。
pub fn within_deadline<T>(limit: Duration, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let t0 = Instant::now();
    let out = f()?;
    let used = t0.elapsed();
    if used > limit {
        return Err(format!("决策超时：用了 {}ms（上限 {}ms）", used.as_millis(), limit.as_millis()));
    }
    Ok(out)
}

// ── 本地模型后端（形态 B：子进程 `llama-server`，OpenAI 兼容端点）──────────────
//
// **它不负责把 `llama-server` 拉起来** —— 那是宿主 / 插件（`local-model`，走清单 `sidecar`）
// 的事。这里只负责「打端点、要一个枚举、解析」。**任何一步失败都 Err ⇒ `decide()` fail-open。**
//
// 决策提示词刻意**极短**：分类不需要上下文，短 prompt ⇒ 本地 prefill 快、CPU 上也够用。

/// 分类提示词。**短 + 给两个样例** —— 实测（2026-10-09）：一段三行的抽象说明会把 0.5B 带偏
/// （它把「帮我看下这个报错」判成 `SKIP`）；改成「一眼能对上的 few-shot」后同一输入判 `SEND`。
/// 解析端对 JSON / 纯词两种形态都认。
const CLASSIFY_SYSTEM: &str = "Decide if the user's message is meant for an AI assistant.\n\
Answer with one word: SEND or SKIP.\n\
user: 帮我看下这个报错\n\
assistant: SEND\n\
user: /compact\n\
assistant: SKIP\n\
user: 这个函数为什么返回 None\n\
assistant: SEND";

/// 本地模型后端配置。
#[derive(Debug, Clone)]
pub struct LocalHttpConfig {
    /// 端点根，如 `http://127.0.0.1:8080`（**不带** `/v1/...`）。
    pub base_url: String,
    /// 模型名（llama-server 只认它自己加载的那一个，随便填一个非空串即可）。
    pub model: String,
    /// 单次决策的 HTTP 超时。
    pub timeout: Duration,
}

impl LocalHttpConfig {
    /// 从环境变量读：`LUNAC_LOCAL_MODEL_URL`（必填）与 `LUNAC_LOCAL_MODEL_NAME`（缺省 `local`）。
    /// **没配 URL ⇒ `None`** —— 调用方据此退回桩后端或直接 fail-open（别去连一个空地址）。
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("LUNAC_LOCAL_MODEL_URL").ok()?;
        let url = url.trim().to_string();
        if url.is_empty() {
            return None;
        }
        let model = std::env::var("LUNAC_LOCAL_MODEL_NAME").unwrap_or_else(|_| "local".into());
        Some(LocalHttpConfig { base_url: url, model, timeout: LOCAL_DECIDE_TIMEOUT })
    }
}

/// 本地模型后端。见文件头「形态 B」。
pub struct LocalHttp {
    cfg: LocalHttpConfig,
}

impl LocalHttp {
    pub fn new(cfg: LocalHttpConfig) -> Self {
        LocalHttp { cfg }
    }
}

impl Decider for LocalHttp {
    fn decide(&self, input: &str) -> Result<Decision, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(self.cfg.timeout)
            // ⚠️ **必须显式禁代理**：这是**回环**端点（`127.0.0.1`），而 reqwest 在 Windows 上
            // 会读**系统代理**（`ProxyEnable` / `ProxyServer`），且**不认 `ProxyOverride` 绕过表**。
            // 实测（2026-10-09，用户开了系统代理 `127.0.0.1:7892`）：本机 stub 的测试都因此失败。
            // 若不禁代理，本地模型（以及任何 loopback 端点）会被塞给代理 ⇒ 连不上。
            .no_proxy()
            .build()
            .map_err(|e| format!("建 HTTP client 失败：{e}"))?;
        let url = format!("{}/v1/chat/completions", self.cfg.base_url.trim_end_matches('/'));
        let body = json!({
            "model": self.cfg.model,
            "messages": [
                { "role": "system", "content": CLASSIFY_SYSTEM },
                { "role": "user", "content": input },
            ],
            // 只回一个词 ⇒ 8 个 token 足够；温度 0 求确定性。
            "max_tokens": 8,
            "temperature": 0.0,
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
        parse_verdict(content)
    }
}

/// 把模型那一个词的输出解析成 `Decision`。**认不出 ⇒ `Err`**（⇒ 上层 fail-open）。
///
/// 认三种形态（按优先级）：① `{"send": true/false}`；② 含 `skip`；③ 含 `send`。
/// **`skip` 先于 `send` 判**：`skip` 里不含 `send`，反过来也不含，但先判否定的更稳。
fn parse_verdict(content: &str) -> Result<Decision, String> {
    let c = content.trim();
    if let Ok(v) = serde_json::from_str::<Value>(c) {
        if let Some(b) = v.get("send").and_then(Value::as_bool) {
            return Ok(if b { Decision::send("model-send") } else { Decision::skip("model-skip") });
        }
    }
    let low = c.to_ascii_lowercase();
    if low.contains("skip") || low.contains("false") {
        return Ok(Decision::skip("model-skip"));
    }
    if low.contains("send") || low.contains("true") {
        return Ok(Decision::send("model-send"));
    }
    Err(format!("模型输出认不出枚举：{}", log::truncate_chars(c, 80)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 后端故障 ⇒ **必须放行**（这是本模块存在的第一理由，最不可回退）。
    struct Boom;
    impl Decider for Boom {
        fn decide(&self, _input: &str) -> Result<Decision, String> {
            Err("llama-server 没起".into())
        }
    }

    /// 后端自报「拿不准」⇒ 也放行（fail-open 的另一半）。
    struct Unsure;
    impl Decider for Unsure {
        fn decide(&self, _input: &str) -> Result<Decision, String> {
            Ok(Decision::allow_fallback("model-unsure"))
        }
    }

    #[test]
    fn backend_failure_fails_open_to_send() {
        let d = decide(&Boom, "帮我改一下这个文件");
        assert_eq!(d.action, Action::Send, "后端故障绝不能吞消息");
        assert!(d.fallback, "必须标出这是后备路径");
        assert_eq!(d.reason, "fail-open-backend-error");
    }

    #[test]
    fn unsure_backend_also_sends() {
        let d = decide(&Unsure, "随便说点什么");
        assert_eq!(d.action, Action::Send);
        assert!(d.fallback);
    }

    #[test]
    fn empty_input_is_skipped_without_asking_backend() {
        // 用 Boom 当后端：若它被调用，就会走 fail-open 变成 Send ⇒ 本断言能区分「先判空」。
        let d = decide(&Boom, "   \n\t ");
        assert_eq!(d.action, Action::Skip);
        assert!(!d.fallback);
        assert_eq!(d.reason, "empty-input");
    }

    #[test]
    fn heuristic_skips_slash_commands() {
        let d = decide(&Heuristic, "/compact");
        assert_eq!(d.action, Action::Skip);
        assert_eq!(d.reason, "slash-command");
    }

    #[test]
    fn heuristic_skips_punctuation_only() {
        let d = decide(&Heuristic, "……！？");
        assert_eq!(d.action, Action::Skip);
        assert_eq!(d.reason, "no-letters");
    }

    #[test]
    fn heuristic_sends_plain_text_and_cjk() {
        for s in ["hello", "帮我看下这个报错", "1+1 等于几", "what's up"] {
            let d = decide(&Heuristic, s);
            assert_eq!(d.action, Action::Send, "普通话必须发：{s}");
            assert!(!d.fallback, "桩后端不产生 fallback：{s}");
        }
    }

    #[test]
    fn heuristic_never_falls_back_on_normal_input() {
        // 桩后端是纯函数，永远 `Ok` ⇒ 任何非空输入都不该带 fallback 标记。
        let d = decide(&Heuristic, "hello");
        assert!(!d.fallback);
        assert_eq!(d.reason, "heuristic-default");
    }

    #[test]
    fn deadline_flags_slow_calls() {
        // 立刻返回、但声明一个 0 上限 ⇒ 应当被第二道闸判超时。
        let r = within_deadline(Duration::from_millis(0), || Ok(Decision::send("x")));
        assert!(r.is_err(), "超过上限必须 Err（⇒ 上层 fail-open）");
    }

    #[test]
    fn deadline_passes_fast_calls() {
        let r = within_deadline(LOCAL_DECIDE_TIMEOUT, || Ok(Decision::skip("y")));
        assert_eq!(r.unwrap().action, Action::Skip);
    }

    #[test]
    fn parse_verdict_accepts_json_and_bare_words() {
        assert_eq!(parse_verdict("{\"send\": true}").unwrap().action, Action::Send);
        assert_eq!(parse_verdict("{\"send\": false}").unwrap().action, Action::Skip);
        assert_eq!(parse_verdict("SEND").unwrap().action, Action::Send);
        assert_eq!(parse_verdict(" skip\n").unwrap().action, Action::Skip);
        assert!(parse_verdict("我不知道").is_err(), "认不出必须 Err（⇒ fail-open）");
    }

    /// **端到端**（要外部环境，故默认 `#[ignore]`）：起一个真的 `llama-server`、设好
    /// `LUNAC_LOCAL_MODEL_URL` 之后跑 `cargo test -- --ignored`。
    /// **未设环境变量时直接跳过**（只打印一行），不会把常规测试搞红。
    #[test]
    #[ignore = "需要本地 llama-server：设 LUNAC_LOCAL_MODEL_URL 后跑 cargo test -- --ignored"]
    fn local_model_end_to_end_when_configured() {
        let Some(cfg) = LocalHttpConfig::from_env() else {
            eprintln!("跳过：未设 LUNAC_LOCAL_MODEL_URL");
            return;
        };
        let d = LocalHttp::new(cfg);
        // **只硬断言「通路」**：真的走通了本地模型（不是 fail-open），且解析出了枚举。
        // **不把具体判 SEND/SKIP 钉进 CI** —— 0.5B 是概率模型，钉它的准确率只会制造假红；
        // 准确率要用样例集单独量（思路见 backlog §4.1.6「决策保真度」）。
        let pos = decide(&d, "帮我看下这个报错");
        assert!(!pos.fallback, "不该走 fail-open（说明没连上 / 没解析出来）：{pos:?}");
        assert!(pos.reason.starts_with("model-"), "必须解析出模型枚举：{pos:?}");
        eprintln!("样例[应 SEND] 帮我看下这个报错 ⇒ {pos:?}");
        let neg = decide(&d, "/compact");
        assert!(!neg.fallback, "不该走 fail-open：{neg:?}");
        eprintln!("样例[应 SKIP] /compact ⇒ {neg:?}");
    }

    /// **最要紧的一条**：本地模型没起（端点连不上）时，`decide()` 必须**放行**，
    /// 绝不能因为决策层坏了就把用户的话吞掉。用 `127.0.0.1:1` 保证连接立即被拒。
    #[test]
    fn local_http_unreachable_fails_open_to_send() {
        let d = LocalHttp::new(LocalHttpConfig {
            base_url: "http://127.0.0.1:1".into(),
            model: "local".into(),
            timeout: Duration::from_millis(500),
        });
        let out = decide(&d, "帮我看看这个报错");
        assert_eq!(out.action, Action::Send, "本地模型连不上绝不能吞消息");
        assert!(out.fallback);
        assert_eq!(out.reason, "fail-open-backend-error");
    }
}
