//! VSCode 扩展 ↔ Agent 的本机 HTTP / SSE 桥（v1 `agent_server.rs` + `cli_bridge.rs` 的移植）。
//!
//! **为什么要有它**：VSCode 扩展的 Webview 拿不到宿主 IPC，只能 `fetch` + `EventSource`。
//! v1 因此在 `127.0.0.1:8789` 起了三个端点，并与桌面端**共用同一个 `agent.exe` 进程**
//! （不是第二份 agent，也不是第二套会话）：
//!
//! | 方法 / 路径 | 作用 |
//! |---|---|
//! | `GET /health` | `{"status":"agent_ready"\|"idle"}` |
//! | `POST /agent/start` | 起常驻 agent（已在跑 ⇒ `already_running`） |
//! | `POST /agent/message` | 把请求体（一条 stream-json）写进 agent 的 stdin |
//! | `GET /agent/stream` | SSE：`event: output` + 每条 stdout 事件；agent 退出时 `event: done` |
//!
//! **与 v1 的差异（如实登记）**：
//!   · 进程状态直接复用 `agent.rs`（v1 是 `cli_bridge` 自己持有一份 `Child` + `ChildStdin`）——
//!     v2 已经有唯一一份 agent 进程状态，这里不再造第二份（`forbidden.md` §禁用 62）。
//!   · 广播只在 `agent.rs` 的 stdout 读线程里追加一次调用（`broadcast`），Dart 侧的
//!     `lunac_agent_poll` 队列不受影响 —— 两条消费路径互不抢占。
//!   · 没有 v1 的 `wake::note`（v2 的唤醒是另一套通道）；除此之外端点、CORS、状态码逐条一致。
//!
//! **安全边界**：只绑 loopback（`127.0.0.1`），但 `Access-Control-Allow-Origin: *` 与 v1 一致 ——
//! 本机任何进程都能驱动 agent。这是「VSCode 扩展」这个能力本身的代价，与 v1 同口径；
//! 要收紧就得先改扩展侧的契约（不能单方面在宿主加 token）。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;

/// 监听地址（与 v1 完全一致）。
pub const BRIDGE_ADDR: &str = "127.0.0.1:8789";

/// 每个 SSE 订阅者的缓冲上限（与 v1 同值）。
///
/// 有界是关键：客户端半开（不再读）时 `send` 会失败 ⇒ 广播时把它摘掉，
/// 而不是把 agent 的输出无限堆在内存里。
const OUTPUT_CHANNEL_CAPACITY: usize = 512;

static RUNNING: AtomicBool = AtomicBool::new(false);
static SUBSCRIBERS: OnceLock<Mutex<Vec<mpsc::SyncSender<String>>>> = OnceLock::new();
/// `/agent/start` 需要的 exe 根（定位 `agent.exe` 与 `config/ai.json`）。
static ROOT: OnceLock<PathBuf> = OnceLock::new();

fn subscribers() -> &'static Mutex<Vec<mpsc::SyncSender<String>>> {
    SUBSCRIBERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 一条请求该走哪个处理分支（纯函数，单测直接打）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `OPTIONS *` —— CORS 预检。
    Preflight,
    Health,
    Start,
    Message,
    Stream,
    NotFound,
}

/// 从请求行（`"POST /agent/message HTTP/1.1"`）解析分支。查询串不计。
pub fn route_of(request_line: &str) -> Route {
    let mut parts = request_line.trim().split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let path = path.split('?').next().unwrap_or("");
    match method {
        "OPTIONS" => Route::Preflight,
        "GET" if path == "/health" => Route::Health,
        "POST" if path == "/agent/start" => Route::Start,
        "POST" if path == "/agent/message" => Route::Message,
        "GET" if path == "/agent/stream" => Route::Stream,
        _ => Route::NotFound,
    }
}

// ── 输出广播（agent stdout → SSE）──────────────────────────────────

/// 注册一个 SSE 订阅者；返回的 Receiver 在桥停掉 / 客户端断开后自然结束。
pub fn subscribe_output() -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::sync_channel(OUTPUT_CHANNEL_CAPACITY);
    if let Ok(mut guard) = subscribers().lock() {
        guard.push(tx);
    }
    rx
}

/// 把一条 agent stdout 事件广播给所有 SSE 订阅者。
///
/// 由 `agent.rs` 的读线程在**每条 JSON 行**入队后调用；没有订阅者时几乎零成本
/// （一次空 Vec 遍历）。发送失败的订阅者（客户端已断开）在这里被摘掉。
pub fn broadcast(line: &str) {
    let Ok(mut guard) = subscribers().lock() else {
        return;
    };
    if guard.is_empty() {
        return;
    }
    guard.retain(|tx| tx.send(line.to_string()).is_ok());
}

/// 当前订阅者数量（诊断 / 单测用）。
pub fn subscriber_count() -> usize {
    subscribers().lock().map(|g| g.len()).unwrap_or(0)
}

// ── HTTP 细节 ─────────────────────────────────────────────────────

fn read_http_request(stream: &mut TcpStream) -> Result<(String, String), String> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(|e| e.to_string())?;
    if request_line.trim().is_empty() {
        return Err("空请求".into());
    }
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    // 上限：一条 user 消息不可能超过 16 MB，超过就是有人在拿本机端口灌数据。
    const MAX_BODY: usize = 16 * 1024 * 1024;
    let body = if content_length > 0 {
        let take = content_length.min(MAX_BODY);
        let mut buf = vec![0u8; take];
        reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
        String::from_utf8_lossy(&buf).to_string()
    } else {
        String::new()
    };
    Ok((request_line.trim().to_string(), body))
}

fn write_json(stream: &mut TcpStream, code: u16, body: &str) -> Result<(), String> {
    let status = match code {
        200 => "200 OK",
        204 => "204 No Content",
        400 => "400 Bad Request",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        _ => "500 Internal Server Error",
    };
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json; charset=utf-8\r\n\
         Access-Control-Allow-Origin: *\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body,
    );
    stream
        .write_all(resp.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())
}

fn cors_preflight(stream: &mut TcpStream) -> Result<(), String> {
    let headers = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\n\
                   Access-Control-Allow-Methods: POST, GET, OPTIONS\r\n\
                   Access-Control-Allow-Headers: *\r\nContent-Length: 0\r\n\r\n";
    stream
        .write_all(headers.as_bytes())
        .map_err(|e| e.to_string())
}

/// SSE：先回头，再把广播的每一行作为 `event: output` 推出去；通道关闭（agent 退出）后
/// 补一条 `event: done`（v1 同形 —— 扩展据此把状态从 running 推回 idle）。
fn stream_sse(stream: &mut TcpStream) -> Result<(), String> {
    let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                   Cache-Control: no-cache\r\nAccess-Control-Allow-Origin: *\r\n\
                   Connection: close\r\n\r\n";
    stream
        .write_all(headers.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    let rx = subscribe_output();
    // 先发一条 `ready`：扩展拿到响应后不必等第一条 agent 输出就知道连上了。
    let _ = stream.write_all(b"event: ready\ndata: {}\n\n");
    let _ = stream.flush();
    for line in rx {
        let sse = format!("event: output\ndata: {line}\n\n");
        if stream.write_all(sse.as_bytes()).is_err() || stream.flush().is_err() {
            break; // 客户端断开
        }
    }
    let _ = stream.write_all(b"event: done\ndata: {}\n\n");
    let _ = stream.flush();
    Ok(())
}

fn handle_connection(mut stream: TcpStream) {
    let result = (|| -> Result<(), String> {
        let (request_line, body) = read_http_request(&mut stream)?;
        match route_of(&request_line) {
            Route::Preflight => cors_preflight(&mut stream),
            Route::Health => {
                let status = if crate::agent::running() {
                    "agent_ready"
                } else {
                    "idle"
                };
                write_json(&mut stream, 200, &format!(r#"{{"status":"{status}"}}"#))
            }
            Route::Start => {
                if crate::agent::running() {
                    return write_json(&mut stream, 200, r#"{"status":"already_running"}"#);
                }
                let Some(root) = ROOT.get() else {
                    return write_json(&mut stream, 500, r#"{"error":"bridge root missing"}"#);
                };
                match crate::agent::start(root) {
                    Ok(()) => write_json(&mut stream, 200, r#"{"status":"started"}"#),
                    Err(e) => write_json(
                        &mut stream,
                        500,
                        &format!(r#"{{"error":"{}"}}"#, json_escape(&e)),
                    ),
                }
            }
            Route::Message => {
                if !crate::agent::running() {
                    return write_json(
                        &mut stream,
                        400,
                        r#"{"error":"CLI not running. POST /agent/start first."}"#,
                    );
                }
                match crate::agent::send(&body) {
                    Ok(()) => write_json(&mut stream, 200, r#"{"status":"sent"}"#),
                    Err(e) => write_json(
                        &mut stream,
                        500,
                        &format!(r#"{{"error":"{}"}}"#, json_escape(&e)),
                    ),
                }
            }
            Route::Stream => {
                if !crate::agent::running() {
                    return write_json(
                        &mut stream,
                        400,
                        r#"{"error":"CLI not running. POST /agent/start first."}"#,
                    );
                }
                stream_sse(&mut stream)
            }
            Route::NotFound => write_json(&mut stream, 404, r#"{"error":"not found"}"#),
        }
    })();
    if let Err(e) = result {
        let _ = write_json(
            &mut stream,
            500,
            &format!(r#"{{"error":"{}"}}"#, json_escape(&e)),
        );
    }
}

/// JSON 字符串的最小转义（错误信息里可能有引号 / 反斜杠 / 换行）。
///
/// 不引入 serde 的 `json!` 只为了这一个用途：这里拼的是**手写**响应体（与 v1 同形），
/// 跨行错误信息原样塞进去会把响应体弄坏。
fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

// ── 生命周期 ──────────────────────────────────────────────────────

/// 起桥（幂等）。`root` = exe 根（`/agent/start` 用它定位 agent.exe）。
pub fn start(root: PathBuf) -> Result<(), String> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let _ = ROOT.set(root);
    let listener = match TcpListener::bind(BRIDGE_ADDR) {
        Ok(l) => l,
        Err(e) => {
            RUNNING.store(false, Ordering::SeqCst);
            crate::log::warn(format!("VSCode 桥绑定 {BRIDGE_ADDR} 失败：{e}"));
            return Err(format!("绑定 {BRIDGE_ADDR} 失败：{e}"));
        }
    };
    thread::spawn(move || {
        for stream in listener.incoming() {
            if !RUNNING.load(Ordering::SeqCst) {
                break;
            }
            match stream {
                Ok(stream) => {
                    thread::spawn(move || handle_connection(stream));
                }
                Err(e) => crate::log::warn(format!("VSCode 桥 accept 失败：{e}")),
            }
        }
    });
    crate::log::info(format!("VSCode 桥监听 http://{BRIDGE_ADDR}"));
    Ok(())
}

/// 停桥（幂等）。监听线程在下一次 accept 时看到标志退出。
pub fn stop() {
    RUNNING.store(false, Ordering::SeqCst);
    if let Ok(mut guard) = subscribers().lock() {
        guard.clear(); // 关掉全部 SSE，客户端会收到 done / 断开
    }
}

/// 桥是否在监听（FFI / 界面诊断用）。
pub fn running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_match_the_v1_endpoint_table() {
        assert_eq!(route_of("OPTIONS /agent/start HTTP/1.1"), Route::Preflight);
        assert_eq!(route_of("GET /health HTTP/1.1"), Route::Health);
        assert_eq!(route_of("POST /agent/start HTTP/1.1"), Route::Start);
        assert_eq!(route_of("POST /agent/message HTTP/1.1"), Route::Message);
        assert_eq!(route_of("GET /agent/stream HTTP/1.1"), Route::Stream);
        // 查询串不影响匹配（EventSource 可能带 `?t=`）。
        assert_eq!(route_of("GET /agent/stream?t=1 HTTP/1.1"), Route::Stream);
        // 方法与路径都要对上：GET /agent/start 不是 start。
        assert_eq!(route_of("GET /agent/start HTTP/1.1"), Route::NotFound);
        assert_eq!(route_of("POST /health HTTP/1.1"), Route::NotFound);
        assert_eq!(route_of("GET /nope HTTP/1.1"), Route::NotFound);
        assert_eq!(route_of(""), Route::NotFound);
    }

    #[test]
    fn health_endpoint_answers_over_tcp() {
        // 端口被占（真有 Lunac 在跑 / 别的程序占着）就跳过 —— 环境问题不该把测试判红。
        if std::net::TcpListener::bind(BRIDGE_ADDR).is_err() {
            eprintln!("skip: {BRIDGE_ADDR} 已被占用");
            return;
        }
        if start(PathBuf::from(".")).is_err() {
            eprintln!("skip: 桥起不来");
            return;
        }
        let resp = http_request("GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        stop();
        assert!(resp.contains("200 OK"), "{resp}");
        // CORS 头必须与 v1 一致（VSCode Webview 靠它跨源 fetch）。
        assert!(resp.contains("Access-Control-Allow-Origin: *"), "{resp}");
        assert!(
            resp.contains("\"idle\"") || resp.contains("\"agent_ready\""),
            "{resp}"
        );
        // 未实现的路径如实 404（不是空响应 / 挂住）。
        if start(PathBuf::from(".")).is_ok() {
            let missing = http_request("GET /nope HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
            stop();
            assert!(missing.contains("404 Not Found"), "{missing}");
        }
    }

    /// 极简 HTTP 客户端（只给单测用）：发一段请求、把响应读到对端关闭。
    fn http_request(raw: &str) -> String {
        use std::io::Read as _;
        use std::time::Duration;
        let mut stream = TcpStream::connect(BRIDGE_ADDR).expect("连不上桥");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = stream.read_to_string(&mut out);
        out
    }

    #[test]
    fn json_escape_keeps_the_response_body_parseable() {
        assert_eq!(json_escape("ok"), "ok");
        assert_eq!(json_escape("a\"b"), "a\\\"b");
        assert_eq!(json_escape("a\\b"), "a\\\\b");
        assert_eq!(json_escape("a\nb"), "a\\nb");
        // 控制字符走 \u 形式（否则响应体里会出现裸换行 / 非法字节）。
        assert_eq!(json_escape("\u{1}"), "\\u0001");
        // 中文原样保留（UTF-8 响应体，不必转义）。
        assert_eq!(json_escape("启动失败"), "启动失败");
    }

    #[test]
    fn broadcast_only_reaches_live_subscribers() {
        // 没有订阅者时广播是空操作（agent 每行都会调它，不能有副作用）。
        broadcast(r#"{"type":"system"}"#);
        let rx = subscribe_output();
        assert_eq!(subscriber_count(), 1);
        broadcast(r#"{"type":"result"}"#);
        assert_eq!(rx.recv().unwrap(), r#"{"type":"result"}"#);
        drop(rx);
        // 订阅者被丢弃后，下一次广播把它摘掉（半开客户端不会无限堆积）。
        broadcast(r#"{"type":"result"}"#);
        assert_eq!(subscriber_count(), 0);
    }
}
