// ── 第二模型预审（§待办 3-28.14，2026-11-16）────────────────────────
//
// 定位：审批链上的**第二意见**，挂在「本来就要弹人工卡」的那一刻之前。
//
// **口径（与 dsh 的差别写死在这里）**：输出只有两态 ——
//   · `Deny(reason)`  ⇒ 直接拒，理由回喂给主模型（工具结果里带一句话）；
//   · `Escalate`      ⇒ 照旧弹人工卡（人工仍是最终决策点）。
//   **永不 `Allow`** —— 与 `docs/forbidden.md` §禁用 1.2「危险命令永远人工确认、永不始终允许」
//   同一条纪律。所以本模块的枚举里**没有** allow 这一档，写不出来就是做不到。
//
// **fail-closed 的落点**：超时 / 崩溃 / 输出认不出 / 端点没配 ⇒ 一律 `Escalate`（人工卡）。
//   与 `safety_prescreen` 的 fail-open 语义相反，这是**有意**的：那边只记账，这边会拦人，
//   判不出来就必须交给人，静默放行等于把闸拆了。
//
// **开关**：`LUNAC_SECOND_REVIEW`（默认关，真值口径同 `LUNAC_LOCAL_DECIDE`）。
//   端点：`LUNAC_SECOND_REVIEW_URL`（缺省回落到 `LUNAC_LOCAL_MODEL_URL`）；
//   模型名：`LUNAC_SECOND_REVIEW_MODEL`（缺省回落 `LUNAC_LOCAL_MODEL_NAME` / `local`）；
//   可选 `LUNAC_SECOND_REVIEW_KEY`（Bearer；本地 llama-server 不需要）。
//   开关关 / 没配端点 ⇒ **零请求、零延迟**，行为与不接线逐字节一致。
//
// **采样**：不做 —— 每一次「本来就要弹卡」的调用都过一遍（这正是要加闸的那批）；
//   只在开关开 + 端点已配时才发生，默认零成本。

use std::time::Duration;

use serde_json::{json, Value};

use crate::decide;
use crate::log;

/// 单次预审的时限：与判定 / 安全预筛同一条口径（同为「一个短枚举」的调用）。
/// 超时 ⇒ `Escalate`（人工卡），不会把工具调用永久挂住。
const REVIEW_TIMEOUT: Duration = decide::LOCAL_DECIDE_TIMEOUT;

/// 输入摘要最多带多少字符 —— 再长就把 prompt 撑爆，也超出小模型的判别能力。
const MAX_INPUT_CHARS: usize = 1_200;
/// 拒绝理由最多保留多少字符（那是要进对话流的一句话，不是论文）。
const MAX_REASON_CHARS: usize = 200;

/// 预审结论。**只有两档**：拒绝 / 升级到人工。没有 allow。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// 直接拒（附一句人话理由）。
    Deny(String),
    /// 交给人 —— 机器可读的原因码，进日志用。
    Escalate(&'static str),
}

impl Verdict {
    #[cfg(test)]
    pub fn is_deny(&self) -> bool {
        matches!(self, Verdict::Deny(_))
    }
}

/// 后端返回的原始结论（解析后的两态）。
///
/// `pub` 是因为它出现在 `pub trait Reviewer` 的方法签名里（模块本身是私有的，对外仍不可见）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Raw {
    Deny(String),
    Escalate,
}

/// 预审后端。实现者只回 `Result`（`Err` = 我这层坏了，由 [`review`] 折成 `Escalate`）。
pub trait Reviewer {
    fn review(&self, prompt: &str) -> Result<Raw, String>;
}

/// 远程 / 本地 OpenAI 兼容端点。
#[derive(Debug, Clone)]
pub struct ReviewConfig {
    /// 端点根（**不带** `/v1/...`）。
    pub base_url: String,
    pub model: String,
    /// Bearer；本地 llama-server 不需要。
    pub token: Option<String>,
    pub timeout: Duration,
}

pub struct HttpReview {
    cfg: ReviewConfig,
}

impl HttpReview {
    pub fn new(cfg: ReviewConfig) -> Self {
        HttpReview { cfg }
    }
}

impl Reviewer for HttpReview {
    fn review(&self, prompt: &str) -> Result<Raw, String> {
        // 本地端点复用「本地通道」那把只有一个槽的锁（llama-server 默认 `-np 1`）；
        // 远程端点也共用它 —— 预审本来就不该和主回合抢时间，忙就升级给人。
        let _guard = decide::try_acquire_local_channel()
            .ok_or_else(|| decide::BUSY_SKIP.to_string())?;
        let client = reqwest::blocking::Client::builder()
            .timeout(self.cfg.timeout)
            // 与判定 / 安全预筛同因：回环端点会被系统代理劫持。
            .no_proxy()
            .build()
            .map_err(|e| format!("建 HTTP client 失败：{e}"))?;
        let url = format!(
            "{}/v1/chat/completions",
            self.cfg.base_url.trim_end_matches('/')
        );
        let body = json!({
            "model": self.cfg.model,
            "messages": [
                { "role": "system", "content": REVIEW_SYSTEM },
                { "role": "user", "content": prompt },
            ],
            // 一句话（DENY: …）⇒ 64 个 token 足够；温度 0 求确定性。
            "max_tokens": 64,
            "temperature": 0.0,
            // **GBNF 输出约束**：锁死两态，模型不可能回「allow」。
            "grammar": REVIEW_GRAMMAR,
        });
        let mut request = client.post(&url).json(&body);
        if let Some(token) = self.cfg.token.as_deref().filter(|t| !t.is_empty()) {
            request = request.bearer_auth(token);
        }
        let resp = request
            .send()
            .map_err(|e| format!("连不上预审模型（{url}）：{e}"))?;
        if !resp.status().is_success() {
            return Err(format!("预审模型 HTTP {}", resp.status()));
        }
        let v: Value = resp
            .json()
            .map_err(|e| format!("预审模型响应不是 JSON：{e}"))?;
        let content = v
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .ok_or_else(|| "响应里没有 choices[0].message.content".to_string())?;
        parse_reply(content)
    }
}

/// 预审提示词。**短 + 两态**（抽象说明会把小模型带偏，与 `safety_prescreen` 同经验）。
const REVIEW_SYSTEM: &str = "You are a strict security reviewer for a coding agent on Windows.\n\
You see ONE tool call the agent wants to run. You may only:\n\
DENY: <one short reason>  — clearly destructive, exfiltrates secrets, or unrelated to the user's task\n\
ESCALATE                  — everything else (a human decides)\n\
You can NEVER approve. Reply with exactly one line.";

/// 输出约束（GBNF）：两态 + 拒绝理由（一行）。
const REVIEW_GRAMMAR: &str = r#"root ::= ws? verdict ws?
verdict ::= "ESCALATE" | deny
deny ::= "DENY: " reason
reason ::= [^\r\n]+
ws ::= [ \t\r\n]*"#;

/// 拼给预审模型的输入（纯函数，单测直接喂）。
pub fn prompt_for(tool: &str, input: &Value) -> String {
    let raw = serde_json::to_string(input).unwrap_or_else(|_| "<unserializable>".to_string());
    let summary = log::truncate_chars(&raw, MAX_INPUT_CHARS);
    format!("tool={tool}\ninput={summary}")
}

/// 统一入口：**fail-closed 的唯一出口**，也是唯一会写日志的地方。
pub fn review(r: &dyn Reviewer, tool: &str, input: &Value) -> Verdict {
    let prompt = prompt_for(tool, input);
    match decide::within_deadline(REVIEW_TIMEOUT, || r.review(&prompt)) {
        Ok(Raw::Deny(reason)) => {
            let reason = clean_reason(&reason);
            log::warn(format!("第二模型预审：拒绝 {tool}（{reason}）"));
            Verdict::Deny(reason)
        }
        Ok(Raw::Escalate) => {
            log::info(format!("第二模型预审：{tool} 升级到人工卡"));
            Verdict::Escalate("model-escalate")
        }
        Err(err) if err == decide::BUSY_SKIP => {
            log::warn("第二模型预审：通道忙，升级到人工卡（不排队）");
            Verdict::Escalate("busy-skip")
        }
        Err(err) => {
            log::warn(format!(
                "第二模型预审失败，按 fail-closed 升级到人工卡（{err}）"
            ));
            Verdict::Escalate("fail-escalate")
        }
    }
}

/// 生产入口：开关关 / 没配端点 ⇒ `None`（= 照旧弹卡）。
///
/// 返回 `Some(一句话)` = 第二模型拒了，调用方把它当 `denied` 回填（工具不执行）。
pub fn gate(tool: &str, input: &Value) -> Option<String> {
    if !enabled() {
        return None;
    }
    let Some(cfg) = config_from_env() else {
        // 开了开关却没配端点：**不能假装审过**。日志说清楚，行为退回人工卡。
        log::warn("第二模型预审已开启，但没有配置端点（LUNAC_SECOND_REVIEW_URL / LUNAC_LOCAL_MODEL_URL），本次直接弹人工卡");
        return None;
    };
    match review(&HttpReview::new(cfg), tool, input) {
        Verdict::Deny(reason) => Some(format!("第二模型预审拒绝：{reason}")),
        Verdict::Escalate(_) => None,
    }
}

/// 开关（`LUNAC_SECOND_REVIEW`，默认关）。进程内读一次并缓存。
pub fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        log::is_truthy(&std::env::var("LUNAC_SECOND_REVIEW").unwrap_or_default())
    })
}

pub fn config_from_env() -> Option<ReviewConfig> {
    let review_url = std::env::var("LUNAC_SECOND_REVIEW_URL").ok();
    let local_url = std::env::var("LUNAC_LOCAL_MODEL_URL").ok();
    let review_model = std::env::var("LUNAC_SECOND_REVIEW_MODEL").ok();
    let local_model = std::env::var("LUNAC_LOCAL_MODEL_NAME").ok();
    let key = std::env::var("LUNAC_SECOND_REVIEW_KEY").ok();
    resolve_config(
        review_url.as_deref(),
        local_url.as_deref(),
        review_model.as_deref(),
        local_model.as_deref(),
        key.as_deref(),
    )
}

/// 端点 / 模型名的优先级（纯函数，单测不碰 env）：
/// 专用变量 > 本地模型变量 > 关闭。
fn resolve_config(
    review_url: Option<&str>,
    local_url: Option<&str>,
    review_model: Option<&str>,
    local_model: Option<&str>,
    key: Option<&str>,
) -> Option<ReviewConfig> {
    let url = [review_url, local_url]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())?;
    let model = [review_model, local_model]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("local");
    Some(ReviewConfig {
        base_url: url.to_string(),
        model: model.to_string(),
        token: key.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
        timeout: REVIEW_TIMEOUT,
    })
}

/// 解析模型的一行输出。**只认两态**；认不出 ⇒ `Err`（⇒ 人工卡）。
fn parse_reply(content: &str) -> Result<Raw, String> {
    let line = content.trim();
    let upper = line.to_ascii_uppercase();
    if upper.starts_with("ESCALATE") {
        return Ok(Raw::Escalate);
    }
    if upper.starts_with("DENY") {
        // 支持 `DENY: xxx` / `DENY:xxx`；只吃第一行。
        let after = line["DENY".len()..].trim_start_matches([':', '：', ' ']);
        let first_line = after.lines().next().unwrap_or("").trim();
        let reason = if first_line.is_empty() {
            "模型拒绝但没给理由".to_string()
        } else {
            first_line.to_string()
        };
        return Ok(Raw::Deny(reason));
    }
    Err(format!(
        "预审输出认不出（只允许 DENY: … / ESCALATE）：{}",
        log::truncate_chars(line, 60)
    ))
}

fn clean_reason(reason: &str) -> String {
    let one_line = reason.split(['\r', '\n']).next().unwrap_or("").trim();
    if one_line.is_empty() {
        return "模型拒绝但没给理由".to_string();
    }
    log::truncate_chars(one_line, MAX_REASON_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Boom;
    impl Reviewer for Boom {
        fn review(&self, _p: &str) -> Result<Raw, String> {
            Err("模型没起".into())
        }
    }

    struct Fixed(Raw);
    impl Reviewer for Fixed {
        fn review(&self, _p: &str) -> Result<Raw, String> {
            Ok(self.0.clone())
        }
    }

    /// 拒绝是唯二的一态；理由原样保留（单行、截断）。
    #[test]
    fn deny_is_one_of_only_two_verdicts() {
        let v = review(
            &Fixed(Raw::Deny("删了用户家目录".into())),
            "Cmd",
            &json!({"command": "rm -rf /"}),
        );
        assert_eq!(v, Verdict::Deny("删了用户家目录".into()));
        assert!(v.is_deny());
    }

    /// 模型说「升级」⇒ 交给人；**枚举里没有 allow**（编译期就写不出来）。
    #[test]
    fn escalate_goes_to_the_human_card() {
        let v = review(&Fixed(Raw::Escalate), "Write", &json!({"path": "a.txt"}));
        assert_eq!(v, Verdict::Escalate("model-escalate"));
        assert!(!v.is_deny());
    }

    /// 后端故障 / 通道忙 ⇒ **fail-closed**：一律人工卡，绝不静默放行。
    #[test]
    fn backend_failure_escalates_instead_of_allowing() {
        let v = review(&Boom, "Cmd", &json!({"command": "dir"}));
        assert_eq!(v, Verdict::Escalate("fail-escalate"));
    }

    /// 解析：只认 `DENY: …` / `ESCALATE`；`ALLOW` 之类一律 `Err`（⇒ 人工卡）。
    #[test]
    fn parse_reply_refuses_to_invent_an_allow() {
        assert_eq!(
            parse_reply("DENY: 会把密钥发到公网").unwrap(),
            Raw::Deny("会把密钥发到公网".into())
        );
        assert_eq!(
            parse_reply("DENY:elevates privileges").unwrap(),
            Raw::Deny("elevates privileges".into())
        );
        assert_eq!(parse_reply("ESCALATE\n").unwrap(), Raw::Escalate);
        // 模型开始自由作答 ⇒ 认不出 ⇒ 不猜。
        assert!(parse_reply("ALLOW").is_err());
        assert!(parse_reply("allow: looks fine").is_err());
        assert!(parse_reply("").is_err());
        assert!(parse_reply("我不确定").is_err());
    }

    /// prompt 带工具名与参数，且**不向模型索要 allow**。
    #[test]
    fn prompt_carries_the_call_but_never_asks_for_approval() {
        let p = prompt_for("Cmd", &json!({"command": "shutdown /s"}));
        assert!(p.starts_with("tool=Cmd\ninput="));
        assert!(p.contains("shutdown /s"));
        assert!(REVIEW_SYSTEM.contains("NEVER approve"));
        assert!(!REVIEW_GRAMMAR.to_ascii_uppercase().contains("ALLOW"));
    }

    /// 超长参数必须被截断（不能把 prompt 撑爆）。
    #[test]
    fn long_inputs_are_truncated() {
        let long = "x".repeat(MAX_INPUT_CHARS * 3);
        let p = prompt_for("Write", &json!({ "content": long }));
        assert!(p.chars().count() <= MAX_INPUT_CHARS + 32, "prompt 长度 {}", p.chars().count());
    }

    /// 端点 / 模型名的优先级：专用变量 > 本地模型变量；都没配 ⇒ 关闭。
    #[test]
    fn dedicated_endpoint_wins_over_the_local_model_one() {
        let cfg = resolve_config(
            Some("http://127.0.0.1:9000"),
            Some("http://127.0.0.1:8080"),
            Some("reviewer"),
            Some("qwen"),
            Some(" k "),
        )
        .expect("dedicated endpoint");
        assert_eq!(cfg.base_url, "http://127.0.0.1:9000");
        assert_eq!(cfg.model, "reviewer");
        assert_eq!(cfg.token.as_deref(), Some("k"));

        let cfg = resolve_config(None, Some(" http://127.0.0.1:8080 "), None, None, None)
            .expect("falls back to the local model");
        assert_eq!(cfg.base_url, "http://127.0.0.1:8080");
        assert_eq!(cfg.model, "local");
        assert!(cfg.token.is_none());

        assert!(resolve_config(None, None, None, None, None).is_none());
        assert!(
            resolve_config(Some("   "), Some("\t"), None, None, None).is_none(),
            "空白端点等于没配"
        );
    }
}
