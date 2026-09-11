// src/agent_server.rs
// HTTP server for VSCode extension → Agent mode bridge.
// Listens on 127.0.0.1:8789, provides 3 endpoints:
//
//   POST /agent/start   → spawn agent.exe (one-time)
//   POST /agent/message  → write JSON to agent.exe stdin
//   GET  /agent/stream   → SSE stream of agent.exe stdout (stream-json)
//
// The VSCode webview uses fetch() + EventSource to drive Agent mode
// without Tauri IPC, while sharing the same agent.exe process as the
// desktop app.

use crate::cli_bridge;
use crate::commands;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

static SERVER_RUNNING: AtomicBool = AtomicBool::new(false);

// ── HTTP helpers ──────────────────────────────────────────────────

fn read_http_request(stream: &mut TcpStream) -> Result<(String, String), String> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).map_err(|e| e.to_string())?;

    let parts: Vec<&str> = request_line.trim().split_whitespace().collect();
    if parts.len() < 2 {
        return Err("Invalid request".into());
    }
    let method = parts[0].to_string();
    let path = parts[1].to_string();

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let line = line.trim().to_string();
        if line.is_empty() { break; }
        if let Some(val) = line.to_lowercase().strip_prefix("content-length:") {
            content_length = val.trim().parse().unwrap_or(0);
        }
    }

    let body = if content_length > 0 {
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
        String::from_utf8_lossy(&buf).to_string()
    } else {
        String::new()
    };

    Ok((format!("{} {}", method, path), body))
}

fn write_json(stream: &mut TcpStream, code: u16, body: &str) -> Result<(), String> {
    let status = match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        500 => "500 Internal Server Error",
        _ => "500 Internal Server Error",
    };
    let resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        status, body.len(), body,
    );
    stream.write_all(resp.as_bytes()).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    Ok(())
}

fn cors_preflight(stream: &mut TcpStream) -> Result<(), String> {
    let headers = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, GET, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nContent-Length: 0\r\n\r\n";
    stream.write_all(headers.as_bytes()).map_err(|e| e.to_string())?;
    Ok(())
}

// ── SSE streaming ─────────────────────────────────────────────────

fn stream_sse(stream: &mut TcpStream) -> Result<(), String> {
    // Send SSE headers
    let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n";
    stream.write_all(headers.as_bytes()).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    // Subscribe to CLI output
    let rx = cli_bridge::subscribe_output();

    // Stream each line as an SSE event
    // Event type is "output", data is the raw stream-json line
    for line in rx {
        let sse = format!("event: output\ndata: {}\n\n", line);
        if stream.write_all(sse.as_bytes()).is_err() {
            break; // client disconnected
        }
        if stream.flush().is_err() {
            break;
        }
    }

    // Send done event when agent.exe exits (channel closed)
    let done = "event: done\ndata: {}\n\n";
    let _ = stream.write_all(done.as_bytes());
    let _ = stream.flush();
    Ok(())
}

// ── Connection handler ────────────────────────────────────────────

fn handle_connection(mut stream: TcpStream) {
    let result = (|| -> Result<(), String> {
        let (req_line, body) = read_http_request(&mut stream)?;

        // CORS preflight
        if req_line.starts_with("OPTIONS") {
            return cors_preflight(&mut stream);
        }

        // Health check
        if req_line.starts_with("GET /health") {
            let status = if cli_bridge::is_running() { "agent_ready" } else { "idle" };
            return write_json(&mut stream, 200, &format!(r#"{{"status":"{}"}}"#, status));
        }

        // POST /agent/start
        if req_line.starts_with("POST /agent/start") {
            if cli_bridge::is_running() {
                return write_json(&mut stream, 200, r#"{"status":"already_running"}"#);
            }
            match commands::start_agent_http() {
                Ok(msg) => write_json(&mut stream, 200, &format!(r#"{{"status":"started","message":"{}"}}"#, msg))?,
                Err(e) => write_json(&mut stream, 500, &format!(r#"{{"error":"{}"}}"#, e))?,
            }
            return Ok(());
        }

        // POST /agent/message
        if req_line.starts_with("POST /agent/message") {
            if !cli_bridge::is_running() {
                return write_json(&mut stream, 400, r#"{"error":"CLI not running. POST /agent/start first."}"#);
            }
            match cli_bridge::write_to_cli(&body) {
                Ok(()) => write_json(&mut stream, 200, r#"{"status":"sent"}"#)?,
                Err(e) => write_json(&mut stream, 500, &format!(r#"{{"error":"{}"}}"#, e))?,
            }
            return Ok(());
        }

        // GET /agent/stream
        if req_line.starts_with("GET /agent/stream") {
            if !cli_bridge::is_running() {
                return write_json(&mut stream, 400, r#"{"error":"CLI not running. POST /agent/start first."}"#);
            }
            return stream_sse(&mut stream);
        }

        // 404
        write_json(&mut stream, 404, r#"{"error":"not found"}"#)
    })();

    if let Err(e) = result {
        let _ = write_json(&mut stream, 500, &format!(r#"{{"error":"{}"}}"#, e));
    }
}

// ── Public API ────────────────────────────────────────────────────

/// Start the Agent HTTP bridge on 127.0.0.1:8789.
/// The desktop app can also drive Agent via Tauri IPC on the same agent.exe process.
pub fn start() -> Result<(), String> {
    if SERVER_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    let listener =
        TcpListener::bind("127.0.0.1:8789").map_err(|e| format!("Agent server bind error: {}", e))?;
    listener.set_nonblocking(false).map_err(|e| e.to_string())?;

    thread::spawn(move || {
        for stream in listener.incoming() {
            if !SERVER_RUNNING.load(Ordering::SeqCst) {
                break;
            }
            match stream {
                Ok(stream) => {
                    thread::spawn(move || handle_connection(stream));
                }
                Err(e) => {
                    eprintln!("[agent_server] accept error: {}", e);
                }
            }
        }
    });

    eprintln!("[agent_server] listening on http://127.0.0.1:8789");
    Ok(())
}

pub fn stop() {
    SERVER_RUNNING.store(false, Ordering::SeqCst);
}
