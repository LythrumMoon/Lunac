// ── 本地「只读预筛」决策层（S1 首个切片，2026-10-15）────────────────────────────
//
// 定位见 `docs/todo.md` §待办 4 S1 与 `docs/archive/agent-feature-backlog.md` §4.1.1：本地小模型
// **只参与决策**，不产生可见文本、不动主请求前缀（不废缓存）、不承担 `tool_use` 往返。
//
// ⚠️ **本模块的语义 = agent 侧的「只读预筛」，不是「输入门」**（§4.1.7：本地模型**不是输入门**，
// 「这条消息过不过 API」是桌宠侧的形态，agent 侧要的是重要节点上的判定）。旧输入门语义已**整体
// 替换**，不留第二套（docs/forbidden.md §禁用 62）。本文件刻意**不引任何模型**，只把三件事定死：
//
//   ① **问题**：**这一轮值不值得派一次后台复盘 fork**？二元判定 `RUN` / `SKIP`。
//      `SKIP` ⇒ 该轮**不派**复盘（省一次云调用）；`RUN` / 任何故障 ⇒ **照旧派**。
//   ② **fail-open**：决策层的任何故障（后端没起 / 超时 / 输出解析不了）一律 **RUN**（照旧走
//      现状路径）并标 `fallback=true`。**首切片铁律：默认关 / 未配端点 / 故障三态都必须与
//      「不接线」逐字节一致** —— 本地判定只当「旁路加速」，绝不改变默认行为。
//   ③ **可插拔 + 可观测**：`Decider` 是 trait，实现只回 `Result`（`Err` = 我这层坏了，由
//      [`decide`] 统一折成 fail-open）。每次决策返回 `Decision { action, reason, fallback }`，
//      `reason` 是**机器可读的原因码**（ASCII 键值），调用方据此落**独立决策账本**（`decisions.rs`）。
//
// **消费点（首切片只有一处）**：`main.rs` 主循环里「**是否派后台复盘 fork**」（约 :5145 一带）。
// **摊销口径**：**每 K 轮**（K = `LUNAC_LOCAL_DECIDE_K`，默认 5）+ **压缩事件**；**不做每轮**（
// 每轮调本地模型会把延迟与开销线性放大，见 §4.1.6 的摊销口径）。
//
// **开关**：`LUNAC_LOCAL_DECIDE`（默认**关**；真值 `1/true/on/yes`，口径同 `LUNAC_DUMP_API`）。
// 未设 `LUNAC_LOCAL_MODEL_URL` 时 [`LocalHttpConfig::from_env`] 返回 `None` ⇒ **整层透明关闭**。
//
// **冷启动预热（2026-10-15）**：真机暴露 —— `llama-server` 冷启后**第一次**判定耗时
// **>1500ms**（撞 [`LOCAL_DECIDE_TIMEOUT`]）⇒ 首次预筛静默 fail-open（行为安全，但等于没生效）；
// 热机后正常。故 [`warmup_with`] 发**一次**极短请求把模型加载 / 首次 KV / 连接吃进缓存，
// **丢弃结果、不进账本、不改预筛语义**。**超时值 1500ms 不动**；只在开关开 + 端点已配时预热
// （[`warmup_config`]），默认关 ⇒ **零预热、零请求**。

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::log;
use serde_json::{json, Value};

/// 决策动作。**只有两档**：`Run`（值不值得不重要，照旧派）/ `Skip`（本轮不派）。
///
/// 「不确定」**不是**第三档 —— 它就是 `Run`，靠 `fallback` 标出来供事后归因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 照旧派这一次后台复盘。
    Run,
    /// 本轮不派（省一次云调用）。
    Skip,
}

impl Action {
    /// 落账本用的字符串形态（`run` / `skip`）。
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Run => "run",
            Action::Skip => "skip",
        }
    }
}

/// 一次决策的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub action: Action,
    /// 机器可读的原因码（进日志 / 账本；**不要塞中文散文**）。
    pub reason: &'static str,
    /// 是否走了「后备路径」（后端故障）。**为真时 `action` 必为 `Run`。**
    pub fallback: bool,
}

impl Decision {
    /// 后端给出的「照旧派」。
    pub fn run(reason: &'static str) -> Self {
        Decision { action: Action::Run, reason, fallback: false }
    }
    /// 后端给出的「本轮不派」。
    pub fn skip(reason: &'static str) -> Self {
        Decision { action: Action::Skip, reason, fallback: false }
    }
    /// **fail-open 的唯一出口**：任何故障都归到这里，保证「拿不准就照旧派」。
    pub fn allow_fallback(reason: &'static str) -> Self {
        Decision { action: Action::Run, reason, fallback: true }
    }
}

/// 决策后端。
///
/// **实现者只回 `Result`**：`Err` 表示「我这层坏了」（进程没起 / 超时 / 输出不是合法枚举），
/// 由 [`decide`] 统一折成 fail-open —— 后端**不许自己决定「故障时派不派」**，
/// 那样每个后端都会写一份不同的兜底策略，迟早漂移。
pub trait Decider {
    fn decide(&self, input: &str) -> Result<Decision, String>;
}

/// 统一入口：**fail-open 的落点**，也是唯一会写日志的地方。
///
/// 顺序：空输入 → `Skip`（没有内容可判，也就没有值得复盘的东西）；否则过第二道超时闸问后端；
/// 后端 `Err` / 超时 ⇒ 放行（`Run`）。
pub fn decide(d: &dyn Decider, input: &str) -> Decision {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        // 空输入不是「决策」——但走同一条返回口径，调用方不必特判。
        return Decision::skip("empty-input");
    }
    // 第二道超时闸（`within_deadline`）：对阻塞式 HTTP 调用，第一道是 reqwest 自带的
    // `timeout`；本闸兜住「调用本身没设超时」或「本地进程卡死但连接还在」这类情况。
    match within_deadline(LOCAL_DECIDE_TIMEOUT, || d.decide(trimmed)) {
        Ok(dec) => {
            log::info(format!(
                "decide: action={} reason={} fallback=false",
                dec.action.as_str(),
                dec.reason
            ));
            dec
        }
        Err(e) => {
            // ⚠️ 这一行就是「与不接线逐字节一致」的现场：后端坏了照样照旧派。
            log::warn(format!("decide: 后端故障，按 fail-open 放行（{e}）"));
            Decision::allow_fallback("fail-open-backend-error")
        }
    }
}

// ── 开关与摊销周期（都归本模块 = 本地决策的唯一真相源）────────────────────

/// 「每 K 轮」的默认周期。
const DEFAULT_K: u64 = 5;

/// `LUNAC_LOCAL_DECIDE` 是否开启（默认**关**）。进程内读一次并缓存（env 生命周期内不变）。
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        log::is_truthy(&std::env::var("LUNAC_LOCAL_DECIDE").unwrap_or_default())
    })
}

/// 摊销周期 K（`LUNAC_LOCAL_DECIDE_K`，默认 5；非法 / 0 ⇒ 默认）。进程内读一次并缓存。
pub fn amortize_k() -> u64 {
    static K: OnceLock<u64> = OnceLock::new();
    *K.get_or_init(|| parse_k(std::env::var("LUNAC_LOCAL_DECIDE_K").ok().as_deref()))
}

/// 解析 K：非空且 `> 0` 的十进制才算数，其余（缺省 / 非法 / 0）回默认。
fn parse_k(v: Option<&str>) -> u64 {
    v.and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|k| *k > 0)
        .unwrap_or(DEFAULT_K)
}

/// 桩后端（形态 A：纯规则 / 启发式）。**零依赖、零模型、可立即验证。**
///
/// 判据**刻意保守** —— 宁可多复盘也别漏。只把**明显不值得复盘**的回合判 `Skip`：
///   · 以 `/` 开头 ⇒ 当成会话内斜杠命令（`/compact` 那类），没有可长期留存的事实；
///   · 不含任何字母 / 数字（纯符号、纯空白）⇒ 没有实质内容。
/// 其余一律 `Run`。
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
        Ok(Decision::run("heuristic-default"))
    }
}

/// 确定性快判（形态 A 的判据）：**明显不值得复盘**的回合（斜杠命令 / 无实质内容）。
///
/// 供调用方在问本地模型前短路（省一次模型调用）；与 [`Heuristic`] 后端共用同一对判据
/// （单一真相源，不得各写一份）。见 `main.rs` 的 `prescreen`。
pub fn obviously_not_reviewable(input: &str) -> bool {
    Heuristic::looks_like_command(input) || !Heuristic::has_letters(input)
}

/// 给「真后端」（本地模型）用的时限封装。
///
/// 为什么放在这里而不是各后端各写一份：**超时口径必须一致**，否则「多慢算坏」会各说各话。
/// 形态 B 的 `llama-server` 后端用它包住 HTTP 调用（见 [`decide`]），超时即 `Err`（⇒ fail-open）。
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
// 判定提示词刻意**极短**：二元分类不需要上下文，短 prompt ⇒ 本地 prefill 快、CPU 上也够用。

/// 分类提示词。**短 + 给几个样例** —— 实测（2026-10-09）：一段三行的抽象说明会把 4B 带偏，
/// 改成「一眼能对上的 few-shot」后判定才稳。解析端对 JSON / 纯词两种形态都认。
const CLASSIFY_SYSTEM: &str = "Decide whether this turn is worth spending ONE background memory-review pass on \
(a review reads the recent conversation and may write a long-term memory entry).\n\
Answer with one word: RUN or SKIP.\n\
Skip turns with nothing durable to remember (small talk, thanks, session commands).\n\
user: 帮我看下 src/main.rs 这个解析报错\n\
assistant: RUN\n\
user: /compact\n\
assistant: SKIP\n\
user: 好的谢谢\n\
assistant: SKIP\n\
user: 这个函数为什么返回 None\n\
assistant: RUN";

/// 判定输出的 **GBNF 语法约束**（llama.cpp 请求体的 `grammar` 字段）。
///
/// 作用：把输出空间**锁死**到判定枚举上 —— 模型再想「开始作答」也吐不出自由文本，
/// 只能回 `RUN` / `SKIP`（允许前后空白 / 换行）。**枚举名取大写、与 [`CLASSIFY_SYSTEM`]
/// 的 few-shot 及 [`parse_verdict`] 的认读口径一致**（`parse_verdict` 对大小写不敏感）。
///
/// 实测（2026-10-15，`llama-server` build **b11524** `win-cpu-x64`、Qwen3-4B-Instruct-2507-Q4_K_M、
/// 纯 CPU、`-c 8192 -np 1`）：给「你是诗人，只能写诗、绝不给单个词」的系统提示，
/// **不带本约束**时回整段诗（`finish_reason:"length"`、出满 max_tokens）；
/// **带上本约束**后被强制回枚举（`SKIP`）。⇒ 本字段确被服务端应用，非静默忽略。
///
/// **走请求体、不走启动参数**：无需改插件 `local-model` 的 `sidecar.args`，
/// 插件版本与 `build-plugins.ps1` 均不动（§禁用 62 单一真相源 —— 约束只此一处）。
const VERDICT_GRAMMAR: &str = r#"root ::= ws? verdict ws?
verdict ::= "RUN" | "SKIP"
ws ::= [ \t\r\n]*"#;

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
    /// **没配 URL ⇒ `None`** —— 调用方据此整层透明关闭（别去连一个空地址）。
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
            // **GBNF 输出约束**（见 `VERDICT_GRAMMAR`）：把输出锁死到 `RUN` / `SKIP` 二选一。
            // 光靠 few-shot 时模型可能开始自由作答，`parse_verdict` 认不出 ⇒ `Err` ⇒ fail-open；
            // 约束后不可能再输出散文。llama.cpp 兼容端点认这个字段名（实测 build b11524）。
            "grammar": VERDICT_GRAMMAR,
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

// ── 冷启动预热（2026-10-15）─────────────────────────────────
//
// 背景见文件头。**只做一件事**：在 agent 启动时**后台**发一次极短请求，把 `llama-server` 的
// 冷启开销（模型加载 / 首次 KV / 连接建立）吃进缓存，使「启动后第一次真实判定」落在超时内。
// 与判定**共用一条通道**（走 `Decider::decide` ⇒ `LocalHttp` 的同一构造：同 client 配置 /
// 同样带 `grammar` 约束 / 同样的极小 `max_tokens`），故不另造第二套请求构造（§禁用 62）。

/// 预热用的极短输入：只走一次 prefill，**结果丢弃** —— 进不到账本、也改不了任何判定语义。
const WARMUP_INPUT: &str = "warmup";

/// 预热的**专用超时**：**必须长于** [`LOCAL_DECIDE_TIMEOUT`]。
///
/// 为什么不能沿用判定的 1500ms（真机实测 2026-10-15）：冷启后的**第一次推理本身**就 >1500ms ——
/// 若预热沿用 1500ms 上限，预热请求会在冷推理完成前就超时收摊，服务端并不保证留下「已热身」的
/// 状态。预热在**后台线程**、只打本地端点、只跑一次，用一个更宽的上限把它**跑完**才是它存在的
/// 意义。⚠️ 这只放宽**预热自己**的上限；判定的 1500ms（`LOCAL_DECIDE_TIMEOUT`）**一个字不改**。
pub const WARMUP_TIMEOUT: Duration = Duration::from_secs(30);

/// 预热是否该做：**开关开 + 端点已配**两个条件同时成立才预热，否则 `None`（⇒ 零请求）。
///
/// 抽成纯函数（`enabled` / `cfg` 由调用方传入）便于**确定性单测**，不依赖进程 env。
fn warmup_gate(enabled: bool, cfg: Option<LocalHttpConfig>) -> Option<LocalHttpConfig> {
    if enabled {
        cfg
    } else {
        None
    }
}

/// 生产入口：从**同一真相源**读两个开关（[`enabled`] / [`LocalHttpConfig::from_env`]），
/// 并把超时换成预热的专用上限（[`WARMUP_TIMEOUT`]；见其注释）。
///
/// `Some` = 该预热；`None` = 默认关 / 未配端点 ⇒ **零预热、零请求**。`main.rs` 的启动预热
/// （`spawn_local_decide_warmup`）据此决定要不要丢后台线程。
pub fn warmup_config() -> Option<LocalHttpConfig> {
    warmup_gate(enabled(), LocalHttpConfig::from_env()).map(|mut cfg| {
        cfg.timeout = WARMUP_TIMEOUT;
        cfg
    })
}

/// 对给定后端**发一次**预热请求、丢弃结果、失败**静默**。返回是否拿到可解析的枚举（仅供测试断言）。
///
/// ⚠️ 本函数**本身是阻塞的**（等这一次请求回来）——「不阻塞主循环 / 启动」由调用方把它丢进
/// **后台线程**保证（见 `main.rs`）。请求本身还受 `LocalHttpConfig::timeout` 兜底，故即便
/// 对端接了连接却不回包，也只会被超时截断（见 `warmup_is_bounded_by_timeout_on_a_hanging_endpoint`）。
/// 失败**只记 info**，绝不上抛、绝不 panic —— 预热只是旁路加速，坏了也不得影响判定路径。
pub fn warmup_with(d: &dyn Decider) -> bool {
    match d.decide(WARMUP_INPUT) {
        Ok(dec) => {
            log::info(format!("本地判定预热完成（结果丢弃：{}）", dec.action.as_str()));
            true
        }
        Err(e) => {
            log::info(format!("本地判定预热未成功（忽略，不影响判定路径）：{e}"));
            false
        }
    }
}

/// 把模型那一个词的输出解析成 `Decision`。**认不出 ⇒ `Err`**（⇒ 上层 fail-open）。
///
/// 认三种形态（按优先级）：① JSON `{"verdict":"run"|"skip"}`；② JSON `{"run": true/false}`；
/// ③ 裸词（含 `skip` ⇒ Skip；含 `run` ⇒ Run）。**`skip` 先于 `run` 判**：先判否定的更稳。
fn parse_verdict(content: &str) -> Result<Decision, String> {
    let c = content.trim();
    if let Ok(v) = serde_json::from_str::<Value>(c) {
        if let Some(s) = v.get("verdict").and_then(Value::as_str) {
            return verdict_from_word(s);
        }
        if let Some(b) = v.get("run").and_then(Value::as_bool) {
            return Ok(if b { Decision::run("model-run") } else { Decision::skip("model-skip") });
        }
    }
    let low = c.to_ascii_lowercase();
    if low.contains("skip") || low.contains("false") {
        return Ok(Decision::skip("model-skip"));
    }
    if low.contains("run") || low.contains("true") {
        return Ok(Decision::run("model-run"));
    }
    Err(format!("模型输出认不出枚举：{}", log::truncate_chars(c, 80)))
}

/// 把一个裸词解析成判定（对大小写不敏感、容忍首尾空白）。
fn verdict_from_word(s: &str) -> Result<Decision, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "run" => Ok(Decision::run("model-run")),
        "skip" => Ok(Decision::skip("model-skip")),
        other => Err(format!("模型输出认不出枚举：{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 后端故障 ⇒ **必须照旧派**（这是本模块存在的第一理由，最不可回退）。
    struct Boom;
    impl Decider for Boom {
        fn decide(&self, _input: &str) -> Result<Decision, String> {
            Err("llama-server 没起".into())
        }
    }

    /// 后端自报「拿不准」⇒ 也照旧派（fail-open 的另一半）。
    struct Unsure;
    impl Decider for Unsure {
        fn decide(&self, _input: &str) -> Result<Decision, String> {
            Ok(Decision::allow_fallback("model-unsure"))
        }
    }

    /// 恒 `Skip` / 恒 `Run` 的注入桩（供接线行为证据用）。
    struct AlwaysSkip;
    impl Decider for AlwaysSkip {
        fn decide(&self, _input: &str) -> Result<Decision, String> {
            Ok(Decision::skip("stub-skip"))
        }
    }
    struct AlwaysRun;
    impl Decider for AlwaysRun {
        fn decide(&self, _input: &str) -> Result<Decision, String> {
            Ok(Decision::run("stub-run"))
        }
    }

    #[test]
    fn backend_failure_fails_open_to_run() {
        let d = decide(&Boom, "帮我改一下这个文件");
        assert_eq!(d.action, Action::Run, "后端故障必须照旧派，绝不改默认行为");
        assert!(d.fallback, "必须标出这是后备路径");
        assert_eq!(d.reason, "fail-open-backend-error");
    }

    #[test]
    fn unsure_backend_also_runs() {
        let d = decide(&Unsure, "随便说点什么");
        assert_eq!(d.action, Action::Run);
        assert!(d.fallback);
    }

    #[test]
    fn injected_stubs_return_fixed_verdicts() {
        assert_eq!(decide(&AlwaysSkip, "hello").action, Action::Skip);
        assert_eq!(decide(&AlwaysRun, "hello").action, Action::Run);
    }

    #[test]
    fn empty_input_is_skipped_without_asking_backend() {
        // 用 Boom 当后端：若它被调用，就会走 fail-open 变成 Run ⇒ 本断言能区分「先判空」。
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
    fn heuristic_runs_plain_text_and_cjk() {
        for s in ["hello", "帮我看下这个报错", "1+1 等于几", "what's up"] {
            let d = decide(&Heuristic, s);
            assert_eq!(d.action, Action::Run, "普通话必须照旧派：{s}");
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
        let r = within_deadline(Duration::from_millis(0), || Ok(Decision::run("x")));
        assert!(r.is_err(), "超过上限必须 Err（⇒ 上层 fail-open）");
    }

    #[test]
    fn deadline_passes_fast_calls() {
        let r = within_deadline(LOCAL_DECIDE_TIMEOUT, || Ok(Decision::skip("y")));
        assert_eq!(r.unwrap().action, Action::Skip);
    }

    #[test]
    fn parse_verdict_accepts_json_and_bare_words() {
        assert_eq!(parse_verdict(r#"{"verdict":"run"}"#).unwrap().action, Action::Run);
        assert_eq!(parse_verdict(r#"{"verdict":"skip"}"#).unwrap().action, Action::Skip);
        assert_eq!(parse_verdict(r#"{"run": true}"#).unwrap().action, Action::Run);
        assert_eq!(parse_verdict(r#"{"run": false}"#).unwrap().action, Action::Skip);
        assert_eq!(parse_verdict("RUN").unwrap().action, Action::Run);
        assert_eq!(parse_verdict(" skip\n").unwrap().action, Action::Skip);
        assert!(parse_verdict("我不知道").is_err(), "认不出必须 Err（⇒ fail-open）");
    }

    /// `VERDICT_GRAMMAR` 必须**同时**放行两个枚举 —— 否则约束会误伤其中一档（把 `SKIP` 也堵死）。
    #[test]
    fn verdict_grammar_admits_both_verdicts() {
        assert!(VERDICT_GRAMMAR.contains("\"RUN\""), "grammar 必须放行 RUN");
        assert!(VERDICT_GRAMMAR.contains("\"SKIP\""), "grammar 必须放行 SKIP");
    }

    /// `VERDICT_GRAMMAR` 允许前后空白 ⇒ `parse_verdict` 必须能认带空白的枚举（实测约束下曾回 `SKIP\n`）。
    #[test]
    fn parse_verdict_tolerates_grammar_whitespace() {
        assert_eq!(parse_verdict("  RUN  ").unwrap().action, Action::Run);
        assert_eq!(parse_verdict("SKIP\n\t ").unwrap().action, Action::Skip);
    }

    /// K 解析：非空且 > 0 才算数，缺省 / 非法 / 0 都回默认 5。
    #[test]
    fn amortize_k_parsing() {
        assert_eq!(parse_k(None), 5);
        assert_eq!(parse_k(Some("")), 5);
        assert_eq!(parse_k(Some("1")), 1);
        assert_eq!(parse_k(Some(" 7 ")), 7);
        assert_eq!(parse_k(Some("0")), 5);
        assert_eq!(parse_k(Some("-3")), 5);
        assert_eq!(parse_k(Some("abc")), 5);
    }

    /// 开关默认关：未设 `LUNAC_LOCAL_DECIDE` ⇒ 判定层整体不介入（读取口径与 `LUNAC_DUMP_API` 同源）。
    #[test]
    fn decide_flag_defaults_off() {
        assert!(!log::is_truthy(&std::env::var("LUNAC_LOCAL_DECIDE").unwrap_or_default()));
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
        // **不把具体判 RUN/SKIP 钉进 CI** —— 4B 是概率模型，钉它的准确率只会制造假红。
        let pos = decide(&d, "帮我看下这个报错");
        assert!(!pos.fallback, "不该走 fail-open（说明没连上 / 没解析出来）：{pos:?}");
        assert!(pos.reason.starts_with("model-"), "必须解析出模型枚举：{pos:?}");
        eprintln!("样例 帮我看下这个报错 ⇒ {pos:?}");
        let neg = decide(&d, "/compact");
        assert!(!neg.fallback, "不该走 fail-open：{neg:?}");
        eprintln!("样例 /compact ⇒ {neg:?}");
    }

    /// **最要紧的一条**：本地模型没起（端点连不上）时，`decide()` 必须**照旧派**，
    /// 绝不能因为决策层坏了就改变默认行为。用 `127.0.0.1:1` 保证连接立即被拒。
    #[test]
    fn local_http_unreachable_fails_open_to_run() {
        let d = LocalHttp::new(LocalHttpConfig {
            base_url: "http://127.0.0.1:1".into(),
            model: "local".into(),
            timeout: Duration::from_millis(500),
        });
        let out = decide(&d, "帮我看看这个报错");
        assert_eq!(out.action, Action::Run, "本地模型连不上绝不能改变默认行为");
        assert!(out.fallback);
        assert_eq!(out.reason, "fail-open-backend-error");
    }

    // ── 冷启动预热（2026-10-15）─────────────────────────────────

    /// 预热门控：**开关开 + 端点已配**两个条件同时成立才预热（其余 ⇒ `None` = 零请求）。
    #[test]
    fn warmup_gate_requires_flag_and_endpoint() {
        let cfg = LocalHttpConfig {
            base_url: "http://127.0.0.1:8090".into(),
            model: "local".into(),
            timeout: LOCAL_DECIDE_TIMEOUT,
        };
        assert!(warmup_gate(false, None).is_none());
        assert!(warmup_gate(false, Some(cfg.clone())).is_none(), "默认关 ⇒ 零预热");
        assert!(warmup_gate(true, None).is_none(), "未配端点 ⇒ 零预热");
        assert!(warmup_gate(true, Some(cfg)).is_some(), "开关开 + 端点配 ⇒ 预热");
    }

    /// 预热上限**必须严格长于**判定上限 —— 否则冷启第一次推理（本就更慢）会被预热的自身超时
    /// 提前掐掉、留下未热身的服务端（真机实测见 `WARMUP_TIMEOUT` 的注释）。
    #[test]
    fn warmup_timeout_exceeds_decide_timeout() {
        assert!(WARMUP_TIMEOUT > LOCAL_DECIDE_TIMEOUT, "预热上限必须宽于判定上限");
    }

    /// 预热**恰好一次**请求（不是每次判定都预热）：计数桩上 `warmup_with` 只让它被问一次。
    #[test]
    fn warmup_asks_the_backend_exactly_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        struct Counting(Arc<AtomicUsize>);
        impl Decider for Counting {
            fn decide(&self, _i: &str) -> Result<Decision, String> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(Decision::run("counted"))
            }
        }
        let n = Arc::new(AtomicUsize::new(0));
        let d = Counting(n.clone());
        assert!(warmup_with(&d), "计数桩恒 Ok ⇒ 预热返回 true");
        assert_eq!(n.load(Ordering::SeqCst), 1, "预热恰好发一次请求");
    }

    /// 预热失败**静默**：后端 `Err` ⇒ 返回 false、不 panic、不上抛；判定路径照常 fail-open。
    #[test]
    fn warmup_failure_is_silent_and_non_fatal() {
        assert!(!warmup_with(&Boom), "端点不可达 ⇒ 预热失败（返回 false）");
        let d = decide(&Boom, "帮我改一下这个文件");
        assert_eq!(d.action, Action::Run, "预热坏了也不得改判定语义");
        assert!(d.fallback);
    }

    /// 「不阻塞」的另一半证据：**请求本身有超时兜底** —— 对端接了连接却永不回包时，预热会被
    /// `timeout` 截断（而非挂住）；再配 `main.rs` 的后台线程 ⇒ 绝不在主循环 / 启动路径上。
    #[test]
    fn warmup_is_bounded_by_timeout_on_a_hanging_endpoint() {
        use std::io::Read;
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // 「接了连接但永不回包」的服务：请求只能靠超时闸兜住（模拟本地进程卡死的形态）。
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                std::thread::sleep(Duration::from_secs(5));
            }
        });
        let d = LocalHttp::new(LocalHttpConfig {
            base_url: format!("http://{addr}"),
            model: "local".into(),
            timeout: Duration::from_millis(300),
        });
        let t0 = Instant::now();
        let ok = warmup_with(&d);
        let used = t0.elapsed();
        assert!(!ok, "对端不回包 ⇒ 预热失败（静默）");
        assert!(used < Duration::from_secs(2), "预热必须被超时兜住、不得挂住：用了 {used:?}");
    }
}
