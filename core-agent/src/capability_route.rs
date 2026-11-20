// core-agent/src/capability_route.rs
// 能力路由（S1 第五切片，2026-10-24）—— 判定「这一轮该走哪条路」。
//
// 定位（`docs/archive/agent-feature-backlog.md` §4.1.1 五类本地决策之一）：**分类成枚举**，
// 只回答「这轮更像纯对话 / 直接用本机工具 / 适合派子代理」。**不生成工具参数、不选工具**。
//
// ⚠️ **本切片只记账，没有任何下游消费者**（如实记，不假装有）：现有架构里「改工具数组」=
// 改请求固定前缀（违反 `docs/code-standards.md` §规范 6 的缓存不变量），「往消息里塞路由提示」=
// 改主请求前缀（违反 S1「不动主请求前缀」的硬约束）。⇒ 本轮只把判定落进决策账本，作为将来
// 「决策保真度」实验（§4.1.6 阶段 1）的**评测数据**；**开关开与关，可见行为逐字节一致**。
// 真要有消费者，接线点必须从「回合收尾」移到「回合开始」，且需先重新裁定上面两条约束。
//
// **为什么在回合收尾判定**（与判定 / 抽取同一个位置）：用户此时已经在看回复，多一次 ~200ms
// 的本地调用不影响任何可见延迟；也避免在回合开始阻塞模型请求。**只记账时时机不影响结果**。
//
// 开关复用 `LUNAC_LOCAL_DECIDE`（默认关）、端点复用 `LUNAC_LOCAL_MODEL_URL`；
// 默认关 / 未配端点 / 故障 ⇒ **零请求、零账本行**（故障时记账标 `fallback`，与其余切片同口径）。

use serde_json::Value;

use crate::decide;
use crate::log;

/// 路由档位。`Unknown` **只由 fail-open 产生**（模型侧只允许回三档，GBNF 锁死）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// 纯对话 / 解释 / 建议 —— 不需要工具。
    Chat,
    /// 直接使用本机工具（读改文件、跑命令、仓库内检索）。
    Work,
    /// 更适合派子代理（大范围探索、跨多文件改造、独立调研）。
    Delegate,
    /// 没判出来（后端故障 / 通道忙 / 空输入）。语义 = **按现状**（模型自己决定）。
    Unknown,
}

impl Route {
    /// 落账本的字符串形态（`chat` / `work` / `delegate` / `unknown`）。
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Chat => "chat",
            Route::Work => "work",
            Route::Delegate => "delegate",
            Route::Unknown => "unknown",
        }
    }
}

/// 一次路由判定的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routing {
    pub route: Route,
    /// 是否走了后备路径（后端故障 / 通道忙）。为真时 `route` 必为 [`Route::Unknown`]。
    pub fallback: bool,
    /// 机器可读的原因码（进账本 / 日志；**不要塞中文散文**）。
    pub reason: &'static str,
}

impl Routing {
    fn judged(route: Route) -> Self {
        let reason = match route {
            Route::Chat => "model-chat",
            Route::Work => "model-work",
            Route::Delegate => "model-delegate",
            Route::Unknown => "model-unknown",
        };
        Routing { route, fallback: false, reason }
    }
    /// **fail-open 的唯一出口**：判不出来 ⇒ `unknown` + `fallback`（= 按现状，不做任何改动）。
    fn fail_open(reason: &'static str) -> Self {
        Routing { route: Route::Unknown, fallback: true, reason }
    }
    /// **busy-skip 的唯一出口**：本地通道忙 ⇒ 立即跳过（不排队）。
    pub fn busy_skip() -> Self {
        Routing { route: Route::Unknown, fallback: true, reason: "busy-skip" }
    }
}

/// 分类提示词。**短 + few-shot**（抽象说明会把 4B 带偏，同 `decide::CLASSIFY_SYSTEM` 的经验）。
const ROUTE_SYSTEM: &str = "Decide how one user turn should be handled by a coding agent.\n\
Answer with one word: CHAT, WORK or DELEGATE.\n\
CHAT: no tools needed (explanation, small talk, advice).\n\
WORK: needs local tools directly (read/edit files, run commands, search the repo).\n\
DELEGATE: better handled by a sub-agent (broad exploration, many files, independent research).\n\
user: 这个报错一般是什么意思\n\
assistant: CHAT\n\
user: 把 src/parse.rs 里的分隔符改成逗号\n\
assistant: WORK\n\
user: 通读整个仓库，找出所有直接调用 reqwest 的地方并整理成清单\n\
assistant: DELEGATE";

/// 输出约束（GBNF，llama.cpp 请求体的 `grammar` 字段）：锁死三档，模型吐不出散文。
///
/// 与判定 / 抽取 / 安全预筛同一条通道、同一个概念（§规范 2 单一真相源：约束本地输出的
/// 机制只有 GBNF 一种）。走请求体、不走启动参数，插件与构建脚本均不动。
const ROUTE_GRAMMAR: &str = r#"root ::= ws? route ws?
route ::= "CHAT" | "WORK" | "DELEGATE"
ws ::= [ \t\r\n]*"#;

/// 路由后端（实现者只回 `Result`）：`Ok` = 已解析出的档位；`Err` = 我这层坏了，
/// 由 [`route`] 统一折成 fail-open。
pub trait Router {
    fn route(&self, input: &str) -> Result<Route, String>;
}

/// 本地模型后端（形态同 `decide::LocalHttp`：OpenAI 兼容端点、回环、显式禁代理）。
pub struct LocalRouter {
    cfg: decide::LocalHttpConfig,
}

impl LocalRouter {
    /// 超时**复用判定的上限**（`decide::LOCAL_DECIDE_TIMEOUT`）：同为「1 个 token 的枚举」，
    /// 输入是一段被截断的用户提问 —— 不新造第四个超时值；超时即 fail-open，无损失。
    pub fn new(cfg: decide::LocalHttpConfig) -> Self {
        LocalRouter { cfg: decide::LocalHttpConfig { timeout: decide::LOCAL_DECIDE_TIMEOUT, ..cfg } }
    }
}

impl Router for LocalRouter {
    fn route(&self, input: &str) -> Result<Route, String> {
        // ⚠️ 先取锁再打端点：本地通道（判定 / 抽取 / 预热 / 改写 / 安全预筛 / 本切片共用）
        // 忙时立即 Err（⇒ busy-skip，不排队）。
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
                { "role": "system", "content": ROUTE_SYSTEM },
                { "role": "user", "content": input },
            ],
            // 只回一个词 ⇒ 8 个 token 足够；温度 0 求确定性。
            "max_tokens": 8,
            "temperature": 0.0,
            // **GBNF 输出约束**（见 `ROUTE_GRAMMAR`）。
            "grammar": ROUTE_GRAMMAR,
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
        parse_route(content)
    }
}

/// 统一入口：**fail-open 的落点**，也是唯一会写日志的地方。
pub fn route(r: &dyn Router, input: &str) -> Routing {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        // 空输入没有可判的内容 —— 与「后端坏了」区分开（不带 fallback），仍记 `unknown`。
        return Routing { route: Route::Unknown, fallback: false, reason: "empty-input" };
    }
    match decide::within_deadline(decide::LOCAL_DECIDE_TIMEOUT, || r.route(trimmed)) {
        Ok(rt) => {
            log::info(format!("能力路由：{}（只记账，无消费者）", rt.as_str()));
            Routing::judged(rt)
        }
        Err(err) if err == decide::BUSY_SKIP => {
            log::warn("能力路由：本地通道忙，立即跳过本次（不排队）");
            Routing::busy_skip()
        }
        Err(err) => {
            log::warn(format!("能力路由后端故障，按 fail-open 记 unknown（{err}）"));
            Routing::fail_open("fail-open-backend-error")
        }
    }
}

/// 解析模型那一个词的输出。认 JSON `{"route":"work"}` 与裸词两种形态；**认不出 ⇒ `Err`**。
fn parse_route(content: &str) -> Result<Route, String> {
    let c = content.trim();
    if let Ok(v) = serde_json::from_str::<Value>(c) {
        if let Some(s) = v.get("route").and_then(Value::as_str) {
            return route_from_word(s);
        }
    }
    route_from_word(c)
}

/// 裸词 ⇒ 档位。**精确匹配**（不用 `contains`）：脏输出一律 `Err` ⇒ fail-open，不做猜测。
fn route_from_word(s: &str) -> Result<Route, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "chat" => Ok(Route::Chat),
        "work" => Ok(Route::Work),
        "delegate" => Ok(Route::Delegate),
        other => Err(format!("模型输出认不出档位：{}", log::truncate_chars(other, 40))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Boom;
    impl Router for Boom {
        fn route(&self, _i: &str) -> Result<Route, String> {
            Err("llama-server 没起".into())
        }
    }
    struct Fixed(Route);
    impl Router for Fixed {
        fn route(&self, _i: &str) -> Result<Route, String> {
            Ok(self.0)
        }
    }

    /// 后端故障 ⇒ **unknown + fallback**（= 按现状，绝不假装判出了某条路）。
    #[test]
    fn backend_failure_fails_open_to_unknown() {
        let r = route(&Boom, "帮我看下报错");
        assert_eq!(r.route, Route::Unknown);
        assert!(r.fallback);
        assert_eq!(r.reason, "fail-open-backend-error");
    }

    /// 空输入 ⇒ `unknown`、**不带 fallback**（没有可判的内容不算故障）。
    #[test]
    fn empty_input_is_unknown_without_fallback() {
        let r = route(&Boom, "  \n\t ");
        assert_eq!(r.route, Route::Unknown);
        assert!(!r.fallback);
        assert_eq!(r.reason, "empty-input");
    }

    /// 三档都能原样传回，且 `fallback=false`。
    #[test]
    fn all_three_routes_pass_through() {
        for (rt, s) in [
            (Route::Chat, "model-chat"),
            (Route::Work, "model-work"),
            (Route::Delegate, "model-delegate"),
        ] {
            let r = route(&Fixed(rt), "随便一句");
            assert_eq!(r.route, rt);
            assert!(!r.fallback);
            assert_eq!(r.reason, s);
        }
    }

    /// 解析：JSON 与裸词都认；脏输出 / 空串 **Err**（⇒ fail-open）。
    #[test]
    fn parse_route_accepts_json_and_bare_words() {
        assert_eq!(parse_route(r#"{"route":"work"}"#).unwrap(), Route::Work);
        assert_eq!(parse_route(" CHAT \n").unwrap(), Route::Chat);
        assert_eq!(parse_route("Delegate").unwrap(), Route::Delegate);
        assert!(parse_route("").is_err());
        assert!(parse_route("use a tool").is_err(), "脏输出必须 Err");
        assert!(parse_route("我不确定").is_err());
    }

    /// grammar 必须同时放行三档（少一档就会把该档判成 fail-open）。
    #[test]
    fn route_grammar_admits_all_three_routes() {
        for w in ["CHAT", "WORK", "DELEGATE"] {
            assert!(ROUTE_GRAMMAR.contains(w), "grammar 必须放行 {w}");
        }
    }

    /// 本地通道忙 ⇒ **立即** busy-skip（不排队）；档位 unknown、fallback 为真。
    #[test]
    fn local_router_busy_skips_without_waiting() {
        use std::time::Instant;
        // 通道锁是进程级静态量 ⇒ 与其它碰锁的单测串行（见 decide::TEST_CHANNEL_LOCK）。
        let _serial = decide::TEST_CHANNEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = decide::LocalHttpConfig {
            base_url: "http://127.0.0.1:1".into(),
            model: "local".into(),
            timeout: decide::LOCAL_DECIDE_TIMEOUT,
        };
        let guard = decide::try_acquire_local_channel().expect("首个请求应拿到通道");
        let t0 = Instant::now();
        let r = route(&LocalRouter::new(cfg), "把分隔符改成逗号");
        let used = t0.elapsed();
        assert!(used < std::time::Duration::from_secs(1), "持锁时必须立即返回：{used:?}");
        assert_eq!(r.route, Route::Unknown);
        assert!(r.fallback);
        assert_eq!(r.reason, "busy-skip");
        drop(guard);
    }

    /// 端点连不上（真发一次 HTTP）⇒ fail-open、不 panic。
    #[test]
    fn local_router_unreachable_fails_open() {
        // 它也会取本地通道锁（`LocalRouter::route` 先取锁再打端点）⇒ 同样要串行。
        let _serial = decide::TEST_CHANNEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let r = LocalRouter::new(decide::LocalHttpConfig {
            base_url: "http://127.0.0.1:1".into(),
            model: "local".into(),
            timeout: std::time::Duration::from_millis(500),
        });
        let out = route(&r, "把分隔符改成逗号");
        assert_eq!(out.route, Route::Unknown);
        assert!(out.fallback);
    }

    /// **端到端**（要外部环境，故默认 `#[ignore]`）：设 `LUNAC_LOCAL_MODEL_URL` 后手动跑。
    #[test]
    #[ignore = "需要本地 llama-server：设 LUNAC_LOCAL_MODEL_URL 后跑 cargo test -- --ignored"]
    fn local_model_end_to_end_when_configured() {
        let Some(cfg) = decide::LocalHttpConfig::from_env() else {
            eprintln!("跳过：未设 LUNAC_LOCAL_MODEL_URL");
            return;
        };
        let r = LocalRouter::new(cfg);
        let out = route(&r, "把 src/parse.rs 里的分隔符改成逗号");
        assert!(!out.fallback, "不该走 fail-open（说明没连上 / 没解析出来）：{out:?}");
        eprintln!("样例 改分隔符 ⇒ {out:?}");
    }
}
