// core-agent/src/main.rs
// Lunac 自研 agent 核心 —— P0：纯文本流式对话闭环
//
// 本程序是 lunac 桌面端的唯一 agent 后端：遵守既有 stream-json 契约
// （见 docs/ai-spec.md §3、app/src/main.ts 对 `cli-output` 的逐行解析），
// 由 src-tauri 的 start_cli 拉起（二进制落点见 core_dir()）。
//
// 契约速查
//   env    LUNAC_AGENT_BASE_URL（完整端点，请求再拼 /v1/messages）
//          LUNAC_AGENT_TOKEN     （鉴权必须走 Bearer；用 x-api-key 会被
//                                 兼容端点判 401）
//          LUNAC_AGENT_MODEL
//   stdin  每行一条 JSON
//            {"type":"user","session_id":"","message":{"role":"user",
//             "content":[{"type":"text","text":"…"}]},"parent_tool_use_id":null}
//   stdout 每行一条 JSON（非 JSON 行会被前端忽略）
//            {"type":"system","subtype":"init",…}
//            {"type":"stream_event","event":{…content_block_delta…}}
//            {"type":"assistant","message":{"content":[{"type":"text",…}]}}
//            {"type":"result","subtype":"success","usage":{…}}
//
// P0 范围与边界
//   ✅ 多轮上下文（进程内 history）、SSE 增量打字、用量上报、错误回传
//   ✅ 思考档位跨模型自适应（MAX_THINKING_TOKENS → thinking 形态 + 400 降级）
//   ❌ 工具调用 / 权限审批（can_use_tool）/ MCP 工具桥 / skills —— P1–P4
//
// 本文件为 Lunac 自研实现，不派生自任何第三方源码。

use std::cell::Cell;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// 单次回复的 token 上限（无思考时的基线）
const BASE_MAX_TOKENS: u32 = 8192;
/// 请求总超时（含流式读取整段响应）
const REQUEST_TIMEOUT_SECS: u64 = 1800;
const CONNECT_TIMEOUT_SECS: u64 = 30;

/// P0 无工具，明确告知模型别假装有工具，避免它凭空描述读文件/跑命令
const SYSTEM_PROMPT: &str = "You are Lunac's built-in assistant, running inside a Windows desktop launcher. \
Answer in the user's language and keep it concise. \
You currently have NO tools available — do not claim to read files, run commands, or browse the web.";

// ── 思考档位：跨模型自适应 ───────────────────────────────────────
//
// lunac 的思考档位由 src-tauri 经 `MAX_THINKING_TOKENS` 传入
// （0=fast 不思考 / 8192=think / 32768=deep）。
//
// 但各供应商的兼容端点对 `thinking` 字段的接受度不一样：
//   · DeepSeek   —— 只认 enabled / disabled，传 adaptive 会 400
//   · 原生 Messages 端点的新模型 —— 要求 adaptive，传 enabled+budget 可能 400
//   · Kimi 等兼容层 —— 可能整个字段都不支持，带了就 400
//
// 所以这里**不硬编码模型名单**：按 env 决定首选形态，遇到「与 thinking
// 相关的 400」就沿降级链自动重试一次，并把最终可用的形态缓存在进程内，
// 后续轮次不再试错。

#[derive(Debug, Clone, Copy, PartialEq)]
enum Thinking {
    /// `{"type":"disabled"}` —— fast 档
    Disabled,
    /// `{"type":"enabled","budget_tokens":n}` —— think/deep 档
    Budget(u32),
    /// `{"type":"adaptive"}` —— 原生 Messages 端点的新模型
    Adaptive,
    /// 完全不发该字段，交由端点默认（未设置 env 时即是此态）
    Omit,
}

impl Thinking {
    fn from_env() -> Self {
        match std::env::var("MAX_THINKING_TOKENS")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            Some(0) => Thinking::Disabled,
            Some(n) => Thinking::Budget(n),
            None => Thinking::Omit,
        }
    }

    fn to_json(self) -> Option<Value> {
        match self {
            Thinking::Disabled => Some(json!({ "type": "disabled" })),
            Thinking::Budget(n) => Some(json!({ "type": "enabled", "budget_tokens": n })),
            Thinking::Adaptive => Some(json!({ "type": "adaptive" })),
            Thinking::Omit => None,
        }
    }

    /// 降级链。注意 Disabled **不会**退到 Adaptive —— 那等于反过来把思考打开。
    fn next(self) -> Option<Self> {
        match self {
            Thinking::Disabled => Some(Thinking::Omit),
            Thinking::Budget(_) => Some(Thinking::Adaptive),
            Thinking::Adaptive => Some(Thinking::Omit),
            Thinking::Omit => None,
        }
    }
}

/// 端点要求 `budget_tokens < max_tokens`，故思考档必须抬高 max_tokens，
/// 否则 deep 档（32768）配 8192 会被判参数非法。
fn max_tokens_for(plan: Thinking) -> u32 {
    match plan {
        Thinking::Budget(n) => (n + 4096).max(BASE_MAX_TOKENS),
        _ => BASE_MAX_TOKENS,
    }
}

/// 判断 400 是否与思考参数有关 —— 只有这类才值得降级重试，
/// 避免把「模型名不存在」这种无关 400 也白重试一遍。
fn thinking_related_error(detail: &str) -> bool {
    let d = detail.to_ascii_lowercase();
    d.contains("thinking") || d.contains("adaptive") || d.contains("budget_tokens")
}


// ── 运行配置 ─────────────────────────────────────────────────────

struct Cfg {
    client: reqwest::blocking::Client,
    endpoint: String,
    token: String,
    model: String,
    /// 当前生效的思考形态；400 降级后会被就地改写并缓存（Cell 便于 &Cfg 共享）
    thinking: Cell<Thinking>,
}

impl Cfg {
    fn from_env() -> Result<Self, String> {
        let base = std::env::var("LUNAC_AGENT_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .ok_or("LUNAC_AGENT_BASE_URL is not set")?;
        let token = std::env::var("LUNAC_AGENT_TOKEN")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .ok_or("LUNAC_AGENT_TOKEN is not set")?;
        let model = std::env::var("LUNAC_AGENT_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .ok_or("LUNAC_AGENT_MODEL is not set")?;

        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .map_err(|e| format!("build http client: {e}"))?;

        Ok(Self {
            client,
            endpoint: format!("{}/v1/messages", base.trim_end_matches('/')),
            token,
            model,
            thinking: Cell::new(Thinking::from_env()),
        })
    }
}

// ── stdout 输出（前端逐行 JSON.parse）────────────────────────────

fn emit(line: Value) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{line}");
    let _ = lock.flush();
}

fn emit_stream_event(event: Value) {
    emit(json!({ "type": "stream_event", "event": event }));
}

/// system/init —— 前端据此把 agentState 从 starting 推进到 idle。
/// 真实 CLI 每轮查询都会发一次，这里保持一致。
fn emit_init(model: &str) {
    emit(json!({
        "type": "system",
        "subtype": "init",
        "session_id": "",
        "model": model,
        "tools": [],
    }));
}

// ── 入口 ─────────────────────────────────────────────────────────

fn main() {
    // src-tauri 传入的 CLI 风格参数一律接受并忽略（P0 无工具/无 MCP）。
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("agent 0.1.0 (lunac self-developed agent core, P0)");
        return;
    }
    if args.iter().any(|a| a == "--mcp-server") {
        eprintln!("[agent] P0: --mcp-server 已接受但未实现（MCP 工具桥为 P3）");
    }

    let cfg = match Cfg::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[agent] {e}");
            emit(json!({
                "type": "result",
                "subtype": "error_during_execution",
                "is_error": true,
                "num_turns": 0,
                "result": e,
                "session_id": "",
            }));
            std::process::exit(1);
        }
    };

    // stdin 读取线程 → 查询线程（mpsc 解耦）。
    // 解耦的意义：查询期间仍能持续 drain stdin，否则 P2 的
    // control_response（审批回包）在模型思考期间根本读不到。
    let (tx, rx) = mpsc::channel::<Value>();
    thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(trimmed) {
                Ok(v) => {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
                Err(e) => eprintln!("[agent] 忽略非法 JSON 输入行: {e}"),
            }
        }
        // stdin 关闭 ⇒ 上游（lunac.exe）已退出
    });

    // system/init 由 run_query 每轮发出（前端据此推进 agentState），启动时不必发
    let mut history: Vec<Value> = Vec::new();
    for msg in rx {
        match msg.get("type").and_then(Value::as_str) {
            Some("user") => {
                let prompt = extract_user_text(&msg);
                if prompt.trim().is_empty() {
                    continue;
                }
                run_query(&cfg, &mut history, &prompt);
            }
            // P0 不产生 can_use_tool，收到审批回包直接忽略
            Some("control_response") => {}
            other => eprintln!("[agent] 忽略输入类型: {other:?}"),
        }
    }
    eprintln!("[agent] stdin closed, exiting");
}

/// 从上游消息里取出纯文本。content 既可能是 block 数组，也可能是裸字符串。
fn extract_user_text(msg: &Value) -> String {
    match msg.pointer("/message/content") {
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(Value::as_str) == Some("text") {
                    b.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

// ── 单轮查询 ─────────────────────────────────────────────────────

fn run_query(cfg: &Cfg, history: &mut Vec<Value>, prompt: &str) {
    emit_init(&cfg.model);
    let started = Instant::now();

    history.push(json!({
        "role": "user",
        "content": [{ "type": "text", "text": prompt }],
    }));

    // 发送（思考形态可降级重试）：只有「与 thinking 相关的 400」才沿降级链
    // 前进一次，并把可用的形态写回 cfg 缓存，后续轮次不再试错。
    let resp = loop {
        let plan = cfg.thinking.get();
        let mut body = json!({
            "model": cfg.model,
            "max_tokens": max_tokens_for(plan),
            "stream": true,
            "system": SYSTEM_PROMPT,
            "messages": history,
        });
        if let Some(t) = plan.to_json() {
            body["thinking"] = t;
        }

        let sent = cfg
            .client
            .post(&cfg.endpoint)
            .header("authorization", format!("Bearer {}", cfg.token))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&body)
            .send();

        let r = match sent {
            Ok(r) => r,
            Err(e) => return finish_error(history, &format!("请求失败: {e}"), started),
        };
        if r.status().is_success() {
            break r;
        }

        let status = r.status();
        let detail = r.text().unwrap_or_default();
        if status == 400 && thinking_related_error(&detail) {
            if let Some(next) = plan.next() {
                eprintln!("[agent] 端点不接受 thinking={plan:?} → 降级为 {next:?}");
                cfg.thinking.set(next);
                continue;
            }
        }
        let detail: String = detail.trim().chars().take(800).collect();
        return finish_error(history, &format!("HTTP {status}: {detail}"), started);
    };

    let mut acc = String::new();
    let mut in_tokens: u64 = 0;
    let mut out_tokens: u64 = 0;
    let mut cache_read: u64 = 0;
    let mut cache_create: u64 = 0;
    let mut api_error: Option<String> = None;

    // SSE：只关心 `data:` 载荷；`event:`/空行/注释行一律跳过
    let reader = BufReader::new(resp);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        let index = || ev.get("index").and_then(Value::as_u64).unwrap_or(0);

        match ev.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                let u = &ev["message"]["usage"];
                in_tokens = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                cache_read = u
                    .get("cache_read_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                cache_create = u
                    .get("cache_creation_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
            }
            "content_block_start" => {
                let ty = ev
                    .pointer("/content_block/type")
                    .and_then(Value::as_str)
                    .unwrap_or("text");
                if ty == "thinking" {
                    emit_stream_event(json!({
                        "type": "content_block_start",
                        "index": index(),
                        "content_block": { "type": "thinking" },
                    }));
                } else if ty == "text" {
                    emit_stream_event(json!({
                        "type": "content_block_start",
                        "index": index(),
                        "content_block": { "type": "text", "text": "" },
                    }));
                }
            }
            "content_block_delta" => {
                let delta = &ev["delta"];
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
                        acc.push_str(text);
                        emit_stream_event(json!({
                            "type": "content_block_delta",
                            "index": index(),
                            "delta": { "type": "text_delta", "text": text },
                        }));
                    }
                    "thinking_delta" => {
                        let thinking = delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        emit_stream_event(json!({
                            "type": "content_block_delta",
                            "index": index(),
                            "delta": { "type": "thinking_delta", "thinking": thinking },
                        }));
                    }
                    // P0 不产生工具调用，input_json_delta 忽略
                    _ => {}
                }
            }
            "content_block_stop" => {
                emit_stream_event(json!({ "type": "content_block_stop", "index": index() }));
            }
            "message_delta" => {
                if let Some(n) = ev["usage"].get("output_tokens").and_then(Value::as_u64) {
                    out_tokens = n;
                }
            }
            "message_stop" => {
                emit_stream_event(json!({ "type": "message_stop" }));
            }
            "error" => {
                api_error = Some(
                    ev.pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("未知流错误")
                        .to_string(),
                );
            }
            _ => {}
        }
    }

    if let Some(err) = api_error {
        return finish_error(history, &err, started);
    }

    // assistant 整包：前端只在「本轮没有任何增量」时用它兜底，不会重复渲染
    emit(json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{ "type": "text", "text": acc }],
        },
    }));
    history.push(json!({
        "role": "assistant",
        "content": [{ "type": "text", "text": acc }],
    }));

    emit(json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "duration_ms": started.elapsed().as_millis() as u64,
        "num_turns": 1,
        "result": acc,
        "session_id": "",
        "total_cost_usd": 0.0,
        "usage": {
            "input_tokens": in_tokens,
            "output_tokens": out_tokens,
            "cache_read_input_tokens": cache_read,
            "cache_creation_input_tokens": cache_create,
        },
    }));
}

/// 出错时收尾：把刚压入的 user 消息弹出，避免失败轮污染后续上下文。
fn finish_error(history: &mut Vec<Value>, msg: &str, started: Instant) {
    eprintln!("[agent] {msg}");
    let last_is_user = history
        .last()
        .and_then(|m| m.get("role"))
        .and_then(Value::as_str)
        == Some("user");
    if last_is_user {
        history.pop();
    }
    emit(json!({
        "type": "result",
        "subtype": "error_during_execution",
        "is_error": true,
        "duration_ms": started.elapsed().as_millis() as u64,
        "num_turns": 0,
        "result": msg,
        "session_id": "",
        "total_cost_usd": 0.0,
    }));
}
