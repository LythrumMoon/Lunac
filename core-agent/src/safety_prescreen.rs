// core-agent/src/safety_prescreen.rs
// 安全预筛（S1 第四切片，2026-10-24）—— 给「需人工确认的工具调用」打一个危险度档位。
//
// 定位（`docs/archive/agent-feature-backlog.md` §4.1.1 五类本地决策之一）：**二 / 三分类**，
// 只回答「这次工具调用有多危险」。**明确不做最终裁决** —— 审批的唯一决策点仍在前端
// `classifyRequest()`，`CMD_BLACKLIST` / 静态安全分析命中时**任何档位都强制人工确认**
// （`docs/forbidden.md` §禁用 1 第 2 / 4 条）。本切片的产物**只进决策账本**：
//   · **不改审批**（不拦、不放行、不改任何档位）；
//   · **不改工具结果 / 不改 history / 不改 system / 不改任何注入**；
//   ⇒ 开关开与关，用户可见行为**逐字节一致**（只有账本多一种 `kind`）。
//
// **为什么放后台线程**（与判定 / 抽取不同）：判定在回合开始、抽取在回合收尾，都在用户等待的
// 间隙；本切片挂**工具执行前**，同步跑 1.5s 会把每次写类操作都拖慢。⇒ 生产入口
// `main.rs` 的 `maybe_safety_prescreen` 一律 `thread::spawn`（fire-and-forget），**绝不在
// 工具路径上等待**；账本写盘由 `decisions::append` 的进程级锁保证行不交错。
//
// **采样**：每 K 次「需审批的工具调用」判一次（K 复用 `decide::amortize_k()`，默认 5）——
// 本切片是**采样留痕**，不是全覆盖审计；不设 K 会让连续编辑产生大量后台请求与账本噪声。
//
// **fail-open**：超时 / 端点不可用 / 解析失败 ⇒ 记 `unknown` + `fallback=true`。
// ⚠️ **不记成 `low`** —— 那会让统计系统性低估风险；`unknown` 的语义就是「这一档没判出来」。
//
// 开关复用 `LUNAC_LOCAL_DECIDE`（默认关）、端点复用 `LUNAC_LOCAL_MODEL_URL`；
// 默认关 / 未配端点 ⇒ **零后台线程、零请求、零账本行**。

use serde_json::Value;

use crate::decide;
use crate::log;

/// 危险度档位。`Unknown` **只由 fail-open 产生**（模型侧只允许回三档，GBNF 锁死）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    Low,
    Medium,
    High,
    /// 没判出来（后端故障 / 通道忙 / 空输入）。**不得**当成 `Low` 统计。
    Unknown,
}

impl Risk {
    /// 落账本的字符串形态（`low` / `medium` / `high` / `unknown`）。
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Low => "low",
            Risk::Medium => "medium",
            Risk::High => "high",
            Risk::Unknown => "unknown",
        }
    }
}

/// 一次预筛的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub risk: Risk,
    /// 是否走了后备路径（后端故障 / 通道忙）。为真时 `risk` 必为 [`Risk::Unknown`]。
    pub fallback: bool,
    /// 机器可读的原因码（进账本 / 日志；**不要塞中文散文**）。
    pub reason: &'static str,
}

impl Verdict {
    fn judged(risk: Risk) -> Self {
        let reason = match risk {
            Risk::Low => "model-low",
            Risk::Medium => "model-medium",
            Risk::High => "model-high",
            Risk::Unknown => "model-unknown",
        };
        Verdict { risk, fallback: false, reason }
    }
    /// **fail-open 的唯一出口**：判不出来 ⇒ `unknown` + `fallback`，绝不冒充低危。
    fn fail_open(reason: &'static str) -> Self {
        Verdict { risk: Risk::Unknown, fallback: true, reason }
    }
    /// **busy-skip 的唯一出口**：本地通道忙 ⇒ 立即跳过（不排队）。
    pub fn busy_skip() -> Self {
        Verdict { risk: Risk::Unknown, fallback: true, reason: "busy-skip" }
    }
}

/// 分类提示词。**短 + few-shot**（同 `decide::CLASSIFY_SYSTEM` 的经验：抽象说明会把 4B 带偏）。
const SAFETY_SYSTEM: &str = "Rate how risky ONE agent tool call is for the local machine.\n\
Answer with one word: LOW, MEDIUM or HIGH.\n\
LOW: read-only, or a local effect that is trivially undone.\n\
MEDIUM: writes files / changes local state, but recoverable.\n\
HIGH: destructive or irreversible, or sends local data out (delete / format / disable security / exfiltrate secrets / pipe remote content into a shell).\n\
tool=Write {\"path\":\"notes.txt\",\"content\":\"hello\"}\n\
risk: MEDIUM\n\
tool=PowerShell {\"command\":\"Get-ChildItem\"}\n\
risk: LOW\n\
tool=PowerShell {\"command\":\"Remove-Item -Recurse -Force C:\\\\Users\"}\n\
risk: HIGH";

/// 输出约束（GBNF，llama.cpp 请求体的 `grammar` 字段）：锁死三档，模型吐不出散文。
///
/// 与判定 / 抽取同一条通道、同一个概念（`docs/code-standards.md` §规范 2 单一真相源：
/// 约束本地输出的机制只有 GBNF 一种）。走请求体、不走启动参数，插件与构建脚本均不动。
const RISK_GRAMMAR: &str = r#"root ::= ws? risk ws?
risk ::= "LOW" | "MEDIUM" | "HIGH"
ws ::= [ \t\r\n]*"#;

/// 预筛后端（实现者只回 `Result`）：`Ok` = 已解析出的档位；`Err` = 我这层坏了，
/// 由 [`screen`] 统一折成 fail-open。
pub trait Screener {
    fn screen(&self, input: &str) -> Result<Risk, String>;
}

/// 本地模型后端（形态同 `decide::LocalHttp`：OpenAI 兼容端点、回环、显式禁代理）。
pub struct LocalScreen {
    cfg: decide::LocalHttpConfig,
}

impl LocalScreen {
    /// 超时**复用判定的上限**（`decide::LOCAL_DECIDE_TIMEOUT`）：同为「1 个 token 的枚举」，
    /// 输入只是一条 ≤400 字的工具摘要 —— 不新造第三个超时值；超时即 fail-open，无损失。
    pub fn new(cfg: decide::LocalHttpConfig) -> Self {
        LocalScreen { cfg: decide::LocalHttpConfig { timeout: decide::LOCAL_DECIDE_TIMEOUT, ..cfg } }
    }
}

impl Screener for LocalScreen {
    fn screen(&self, input: &str) -> Result<Risk, String> {
        // ⚠️ 先取锁再打端点：本地通道（判定 / 抽取 / 预热 / 改写 / 本切片共用）忙时立即 Err。
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
                { "role": "system", "content": SAFETY_SYSTEM },
                { "role": "user", "content": input },
            ],
            // 只回一个词 ⇒ 8 个 token 足够；温度 0 求确定性。
            "max_tokens": 8,
            "temperature": 0.0,
            // **GBNF 输出约束**（见 `RISK_GRAMMAR`）。
            "grammar": RISK_GRAMMAR,
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
        parse_risk(content)
    }
}

/// 统一入口：**fail-open 的落点**，也是唯一会写日志的地方。
pub fn screen(s: &dyn Screener, input: &str) -> Verdict {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        // 空输入没有可判的内容 —— 与「后端坏了」区分开（不带 fallback），但仍记 `unknown`。
        return Verdict { risk: Risk::Unknown, fallback: false, reason: "empty-input" };
    }
    match decide::within_deadline(decide::LOCAL_DECIDE_TIMEOUT, || s.screen(trimmed)) {
        Ok(risk) => {
            log::info(format!("安全预筛：{}（只记账，不改审批）", risk.as_str()));
            Verdict::judged(risk)
        }
        Err(err) if err == decide::BUSY_SKIP => {
            log::warn("安全预筛：本地通道忙，立即跳过本次（不排队）");
            Verdict::busy_skip()
        }
        Err(err) => {
            log::warn(format!("安全预筛后端故障，按 fail-open 记 unknown（{err}）"));
            Verdict::fail_open("fail-open-backend-error")
        }
    }
}

/// 解析模型那一个词的输出。认 JSON `{"risk":"high"}` 与裸词两种形态；**认不出 ⇒ `Err`**。
fn parse_risk(content: &str) -> Result<Risk, String> {
    let c = content.trim();
    if let Ok(v) = serde_json::from_str::<Value>(c) {
        if let Some(s) = v.get("risk").and_then(Value::as_str) {
            return risk_from_word(s);
        }
    }
    risk_from_word(c)
}

/// 裸词 ⇒ 档位。**精确匹配**（不用 `contains`）：`high` / `medium` / `low` 互不为子串，
/// 但 `contains` 会把「not high」这类脏输出误判成高危；精确匹配遇到脏输出就 `Err` ⇒ fail-open。
fn risk_from_word(s: &str) -> Result<Risk, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "low" => Ok(Risk::Low),
        "medium" => Ok(Risk::Medium),
        "high" => Ok(Risk::High),
        other => Err(format!("模型输出认不出档位：{}", log::truncate_chars(other, 40))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Boom;
    impl Screener for Boom {
        fn screen(&self, _i: &str) -> Result<Risk, String> {
            Err("llama-server 没起".into())
        }
    }
    struct Fixed(Risk);
    impl Screener for Fixed {
        fn screen(&self, _i: &str) -> Result<Risk, String> {
            Ok(self.0)
        }
    }

    /// 后端故障 ⇒ **unknown + fallback**，绝不冒充 low（否则统计会低估风险）。
    #[test]
    fn backend_failure_fails_open_to_unknown() {
        let v = screen(&Boom, "tool=Write {}");
        assert_eq!(v.risk, Risk::Unknown);
        assert!(v.fallback);
        assert_eq!(v.reason, "fail-open-backend-error");
    }

    /// 空输入 ⇒ `unknown`、**不带 fallback**（没有可判的内容不算故障）。
    #[test]
    fn empty_input_is_unknown_without_fallback() {
        let v = screen(&Boom, "  \n\t ");
        assert_eq!(v.risk, Risk::Unknown);
        assert!(!v.fallback);
        assert_eq!(v.reason, "empty-input");
    }

    /// 三档都能原样传回，且 `fallback=false`。
    #[test]
    fn all_three_risks_pass_through() {
        for (r, s) in [(Risk::Low, "model-low"), (Risk::Medium, "model-medium"), (Risk::High, "model-high")] {
            let v = screen(&Fixed(r), "tool=Cmd {}");
            assert_eq!(v.risk, r);
            assert!(!v.fallback);
            assert_eq!(v.reason, s);
        }
    }

    /// 解析：JSON 与裸词都认；脏输出 / 空串 **Err**（⇒ fail-open）。
    #[test]
    fn parse_risk_accepts_json_and_bare_words() {
        assert_eq!(parse_risk(r#"{"risk":"high"}"#).unwrap(), Risk::High);
        assert_eq!(parse_risk(" LOW \n").unwrap(), Risk::Low);
        assert_eq!(parse_risk("Medium").unwrap(), Risk::Medium);
        assert!(parse_risk("").is_err());
        assert!(parse_risk("not high").is_err(), "脏输出必须 Err，不能被 contains 误判");
        assert!(parse_risk("我不确定").is_err());
    }

    /// grammar 必须同时放行三档（少一档就会把该档判成 fail-open）。
    #[test]
    fn risk_grammar_admits_all_three_levels() {
        for w in ["LOW", "MEDIUM", "HIGH"] {
            assert!(RISK_GRAMMAR.contains(w), "grammar 必须放行 {w}");
        }
    }

    /// 本地通道忙 ⇒ **立即** busy-skip（不排队）；档位 unknown、fallback 为真。
    #[test]
    fn local_screen_busy_skips_without_waiting() {
        use std::time::Instant;
        let _serial = decide::TEST_CHANNEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = decide::LocalHttpConfig {
            base_url: "http://127.0.0.1:1".into(),
            model: "local".into(),
            timeout: decide::LOCAL_DECIDE_TIMEOUT,
        };
        let guard = decide::try_acquire_local_channel().expect("首个请求应拿到通道");
        let t0 = Instant::now();
        let v = screen(&LocalScreen::new(cfg), "tool=Cmd {\"command\":\"dir\"}");
        let used = t0.elapsed();
        assert!(used < std::time::Duration::from_secs(1), "持锁时必须立即返回：{used:?}");
        assert_eq!(v.risk, Risk::Unknown);
        assert!(v.fallback);
        assert_eq!(v.reason, "busy-skip");
        drop(guard);
    }

    /// 端点连不上（真发一次 HTTP）⇒ fail-open、不 panic。
    #[test]
    fn local_screen_unreachable_fails_open() {
        // 它也会取本地通道锁（`LocalScreen::screen` 先取锁再打端点）⇒ 同样要串行。
        let _serial = decide::TEST_CHANNEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let s = LocalScreen::new(decide::LocalHttpConfig {
            base_url: "http://127.0.0.1:1".into(),
            model: "local".into(),
            timeout: std::time::Duration::from_millis(500),
        });
        let v = screen(&s, "tool=Cmd {\"command\":\"dir\"}");
        assert_eq!(v.risk, Risk::Unknown);
        assert!(v.fallback);
    }

    /// **端到端**（要外部环境，故默认 `#[ignore]`）：设 `LUNAC_LOCAL_MODEL_URL` 后手动跑。
    #[test]
    #[ignore = "需要本地 llama-server：设 LUNAC_LOCAL_MODEL_URL 后跑 cargo test -- --ignored"]
    fn local_model_end_to_end_when_configured() {
        let Some(cfg) = decide::LocalHttpConfig::from_env() else {
            eprintln!("跳过：未设 LUNAC_LOCAL_MODEL_URL");
            return;
        };
        let s = LocalScreen::new(cfg);
        let v = screen(&s, "tool=PowerShell {\"command\":\"Remove-Item -Recurse -Force D:\\\\tmp\"}");
        assert!(!v.fallback, "不该走 fail-open（说明没连上 / 没解析出来）：{v:?}");
        eprintln!("样例 递归删除 ⇒ {v:?}");
    }
}
