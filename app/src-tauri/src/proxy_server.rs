// src/proxy_server.rs
// Mini Anthropic-compatible HTTP proxy — built into lunac.exe.
// Listens on 127.0.0.1:8788, accepts Anthropic Messages API requests
// from cli.exe, translates to OpenAI/DeepSeek, streams back Anthropic SSE.
//
// This eliminates the need for proxy.exe. The protocol translation
// happens in-process, no subprocess overhead.

use serde::Deserialize;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};

use std::thread;

static SERVER_RUNNING: AtomicBool = AtomicBool::new(false);

// ── Anthropic request (subset) ────────────────────────────────────

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AnthRequest {
    #[serde(default)]
    model: String,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
    #[serde(default)]
    messages: Vec<AnthMessage>,
    #[serde(default)]
    system: Option<Value>,
    #[serde(default)]
    stream: bool,
}

fn default_max_tokens() -> u32 {
    8192
}

#[derive(Debug, Deserialize)]
struct AnthMessage {
    role: String,
    content: Value, // can be string or [{type: "text", text: "..."}]
}

// ── OpenAI response types ─────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OaiStreamChunk {
    #[serde(default)]
    id: String,
    #[serde(default)]
    choices: Vec<OaiStreamChoice>,
    #[serde(default)]
    usage: Option<OaiUsage>,
}

#[derive(Debug, Deserialize)]
struct OaiStreamChoice {
    #[serde(default)]
    delta: Option<OaiDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OaiDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
struct OaiUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
}

// ── Anthropic SSE emitter ────────────────────────────────────────

struct SseEmitter {
    message_id: String,
    model: String,
    text_started: bool,
}

impl SseEmitter {
    fn new(model: &str) -> Self {
        Self {
            message_id: format!("msg_{}", rand_id()),
            model: model.to_string(),
            text_started: false,
        }
    }

    fn begin_text(&mut self) -> String {
        if self.text_started {
            return String::new();
        }
        self.text_started = true;

        let mut out = String::new();
        out.push_str(&sse("message_start", &serde_json::json!({
            "type": "message_start",
            "message": {
                "id": self.message_id,
                "type": "message",
                "role": "assistant",
                "model": self.model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {
                    "input_tokens": 0,
                    "output_tokens": 0,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0,
                },
            },
        })));
        out.push_str(&sse("content_block_start", &serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "text", "text": "" },
        })));
        out
    }

    fn text_delta(&self, text: &str) -> String {
        sse("content_block_delta", &serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": text },
        }))
    }

    fn finish(
        &self,
        stop_reason: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> String {
        let mut out = String::new();
        if self.text_started {
            out.push_str(&sse("content_block_stop", &serde_json::json!({
                "type": "content_block_stop",
                "index": 0,
            })));
        }
        out.push_str(&sse("message_delta", &serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": stop_reason },
            "usage": {
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
            },
        })));
        out.push_str(&format!(
            "event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
        ));
        out
    }
}

fn sse(event: &str, data: &Value) -> String {
    format!(
        "event: {}\ndata: {}\n\n",
        event,
        serde_json::to_string(data).unwrap_or_default()
    )
}

fn rand_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{:x}", ts)
}

// ── Message extraction ────────────────────────────────────────────

/// Extract plain text from Anthropic content (handles string or content-block array)
fn extract_content(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(blocks) = content.as_array() {
        let mut texts = Vec::new();
        for block in blocks {
            if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                texts.push(t.to_string());
            }
        }
        return texts.join("\n");
    }
    String::new()
}

// ── Translation: Anthropic messages → OpenAI messages ────────────

struct OaiMsg {
    role: String,
    content: String,
}

fn translate_messages(req: &AnthRequest) -> Vec<OaiMsg> {
    let mut out = Vec::new();

    // System prompt
    if let Some(ref system) = req.system {
        let sys_text = extract_content(system);
        if !sys_text.is_empty() {
            out.push(OaiMsg { role: "system".into(), content: sys_text });
        }
    }

    // User/assistant messages
    for msg in &req.messages {
        let text = extract_content(&msg.content);
        if !text.is_empty() {
            out.push(OaiMsg { role: msg.role.clone(), content: text });
        }
    }

    out
}

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

    // Read headers to find Content-Length
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let line = line.trim().to_string();
        if line.is_empty() {
            break;
        }
        if let Some(val) = line.to_lowercase().strip_prefix("content-length:") {
            content_length = val.trim().parse().unwrap_or(0);
        }
    }

    // Read body
    let body = if content_length > 0 {
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
        String::from_utf8_lossy(&buf).to_string()
    } else {
        String::new()
    };

    Ok((format!("{} {}", method, path), body))
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<(), String> {
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        status_text(status),
        content_type,
        body.len(),
        body,
    );
    stream.write_all(response.as_bytes()).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    Ok(())
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        500 => "500 Internal Server Error",
        _ => "500 Internal Server Error",
    }
}

// ── Core: handle Anthropic POST /v1/messages ──────────────────────

fn handle_anthropic_request(
    stream: &mut TcpStream,
    body: &str,
    api_url: &str,
    api_key: &str,
    model: &str,
) -> Result<(), String> {
    let req: AnthRequest = serde_json::from_str(body).map_err(|e| format!("Parse error: {}", e))?;

    // Translate to OpenAI format
    let oai_messages = translate_messages(&req);

    let oai_body = serde_json::json!({
        "model": model,
        "messages": oai_messages.iter().map(|m| serde_json::json!({
            "role": m.role,
            "content": m.content,
        })).collect::<Vec<_>>(),
        "stream": true,
        "stream_options": { "include_usage": true },
    });

    // Call DeepSeek/OpenAI API
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("HTTP client: {}", e))?;

    let resp = client
        .post(format!("{}/v1/chat/completions", api_url))
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&oai_body)
        .send()
        .map_err(|e| format!("API error: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        return Err(format!("{} returned {}: {}", api_url, status, text));
    }

    // Stream SSE back to cli.exe in Anthropic format
    let mut raw = stream.try_clone().map_err(|e| e.to_string())?;
    // Send HTTP headers first (chunked for streaming)
    let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n";
    raw.write_all(headers.as_bytes()).map_err(|e| e.to_string())?;

    let mut sse = SseEmitter::new(model);
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut stop_reason = "end_turn".to_string();
    let mut text_started = false;

    let mut reader = BufReader::new(resp);
    let mut line = String::new();
    let mut data = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }

        let trimmed = line.trim().to_string();

        if trimmed.is_empty() {
            // Empty line = end of SSE event
            let payload = data.trim().strip_prefix("data: ").unwrap_or(&data);
            if payload == "[DONE]" || payload.is_empty() {
                data.clear();
                continue;
            }

            if let Ok(chunk) = serde_json::from_str::<OaiStreamChunk>(payload) {
                if let Some(ref choice) = chunk.choices.first() {
                    if let Some(ref delta) = choice.delta {
                        // Text content
                        if let Some(ref text) = delta.content {
                            if !text.is_empty() {
                                if !text_started {
                                    let preamble = sse.begin_text();
                                    if !preamble.is_empty() {
                                        write_chunk(&mut raw, &preamble)?;
                                    }
                                    text_started = true;
                                }
                                let event = sse.text_delta(text);
                                write_chunk(&mut raw, &event)?;
                            }
                        }
                    }
                    if let Some(ref reason) = choice.finish_reason {
                        stop_reason = reason.to_string();
                    }
                }
                if let Some(ref usage) = chunk.usage {
                    input_tokens = usage.prompt_tokens.unwrap_or(0);
                    output_tokens = usage.completion_tokens.unwrap_or(0);
                }
            }
            data.clear();
        } else if let Some(payload) = trimmed.strip_prefix("data: ") {
            data = format!("data: {}", payload);
        } else if trimmed == "data: [DONE]" {
            data = "[DONE]".into();
        }
    }

    // Send finish events
    let finish_sse = sse.finish(&stop_reason, input_tokens, output_tokens);
    write_chunk(&mut raw, &finish_sse)?;
    write_chunk(&mut raw, "")?; // EOF chunk
    raw.flush().map_err(|e| e.to_string())?;

    Ok(())
}

fn write_chunk(stream: &mut TcpStream, data: &str) -> Result<(), String> {
    if data.is_empty() {
        stream
            .write_all(b"0\r\n\r\n")
            .map_err(|e| e.to_string())?;
    } else {
        let hex_len = format!("{:X}\r\n", data.len());
        stream.write_all(hex_len.as_bytes()).map_err(|e| e.to_string())?;
        stream.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
        stream.write_all(b"\r\n").map_err(|e| e.to_string())?;
    }
    stream.flush().map_err(|e| e.to_string())?;
    Ok(())
}

// ── Connection handler ────────────────────────────────────────────

fn handle_connection(
    mut stream: TcpStream,
    api_url: String,
    api_key: String,
    model: String,
) {
    let result = (|| -> Result<(), String> {
        let (req_line, body) = read_http_request(&mut stream)?;

        // Health check
        if req_line.starts_with("GET /health") {
            return write_http_response(
                &mut stream,
                200,
                "application/json",
                r#"{"status":"ok"}"#,
            );
        }

        // CORS preflight
        if req_line.starts_with("OPTIONS") {
            let headers = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, GET, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nContent-Length: 0\r\n\r\n";
            stream.write_all(headers.as_bytes()).map_err(|e| e.to_string())?;
            return Ok(());
        }

        // Anthropic API
        if req_line.starts_with("POST /v1/messages") {
            if let Err(e) =
                handle_anthropic_request(&mut stream, &body, &api_url, &api_key, &model)
            {
                let error_body = serde_json::json!({
                    "type": "error",
                    "error": { "type": "api_error", "message": e }
                });
                return write_http_response(
                    &mut stream,
                    500,
                    "application/json",
                    &error_body.to_string(),
                );
            }
            return Ok(());
        }

        // OpenAPI-compatible passthrough: proxy /v1/chat/completions
        if req_line.starts_with("POST /v1/chat/completions") {
            // Forward directly to DeepSeek/OpenAI
            let client = reqwest::blocking::Client::new();
            let resp = client
                .post(format!("{}/v1/chat/completions", api_url))
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .body(body)
                .send()
                .map_err(|e| format!("Forward error: {}", e))?;

            let status = resp.status();
            let resp_body = resp.text().unwrap_or_default();

            let status_code = status.as_u16();
            return write_http_response(
                &mut stream,
                status_code,
                "application/json",
                &resp_body,
            );
        }

        // 404
        write_http_response(&mut stream, 404, "application/json", r#"{"error":"not found"}"#)
    })();

    if let Err(e) = result {
        let _ = write_http_response(&mut stream, 500, "application/json", &format!(r#"{{"error":"{}"}}"#, e));
    }
}

// ── Public API ────────────────────────────────────────────────────

/// Start the built-in proxy server on 127.0.0.1:8788.
/// Returns immediately; the server runs in a background thread.
/// Safe to call multiple times (idempotent).
pub fn start(api_url: String, api_key: String, model: String) -> Result<(), String> {
    if SERVER_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(()); // already running
    }

    let listener =
        TcpListener::bind("127.0.0.1:8788").map_err(|e| format!("Proxy bind error: {}", e))?;
    listener
        .set_nonblocking(false)
        .map_err(|e| e.to_string())?;

    thread::spawn(move || {
        for stream in listener.incoming() {
            if !SERVER_RUNNING.load(Ordering::SeqCst) {
                break;
            }
            match stream {
                Ok(stream) => {
                    let url = api_url.clone();
                    let key = api_key.clone();
                    let m = model.clone();
                    thread::spawn(move || handle_connection(stream, url, key, m));
                }
                Err(e) => {
                    eprintln!("[proxy_server] accept error: {}", e);
                }
            }
        }
    });

    eprintln!("[proxy_server] listening on http://127.0.0.1:8788");
    Ok(())
}

/// Stop the proxy server. Subsequent connections will be rejected.
pub fn stop() {
    SERVER_RUNNING.store(false, Ordering::SeqCst);
}
