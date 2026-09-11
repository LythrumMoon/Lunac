// core-agent/src/mcp.rs
// Lunac 自研 agent 后端 —— P3：MCP 工具桥（stdio client）
//
// src-tauri 在 spawn agent.exe 时会带 `--mcp-server stdio:<lunac.exe 路径>`。
// 本模块据此把 lunac.exe 以 `--mcp-server` 拉起 —— 那个进程会拦截该参数、
// 进入 stdio MCP server 模式，读取 `<exe 根>\tools\*.json` 里的用户自定义
// 工具（handler 有 shell / http / builtin 三种，实现在 src-tauri/mcp_server.rs）。
//
// 握手顺序（MCP 2024-11-05）：
//   initialize → notifications/initialized → tools/list → （模型调用时）tools/call
//
// 工具名统一加 `mcp__` 前缀，避免与六件内置工具重名；前缀必须稳定，因为
// 前端审批卡的「始终允许」是按完整工具名记进 localStorage 白名单的。
//
// 桥是尽力而为：连不上 / 握手失败只往 stderr 记一行，六件内置工具照常工作。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

const PREFIX: &str = "mcp__";
const PROTOCOL_VERSION: &str = "2024-11-05";
/// 握手（initialize / tools/list）等待上限
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// 单次 tools/call 上限（shell handler 自身 60s 超时，http 可能更慢）
const CALL_TIMEOUT: Duration = Duration::from_secs(180);
/// 工具名限字符集与 ≤64 长度（Anthropic 兼容端点的校验），故原名上限要减去前缀
const MAX_RAW_NAME: usize = 64 - PREFIX.len();

/// 是否为本桥接进来的 MCP 工具（按名字前缀判定）
pub fn is_mcp(name: &str) -> bool {
    name.starts_with(PREFIX)
}

pub struct Bridge {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    next_id: u64,
    /// 请求体里的工具名 → MCP 侧原名
    names: HashMap<String, String>,
    /// 已转成 Anthropic 工具 schema 的定义，直接并入请求体
    defs: Vec<Value>,
}

impl Bridge {
    /// `spec` 形如 `stdio:<lunac.exe 路径>`（仅支持 stdio 传输）。
    /// `disallowed` 来自 `--disallowedTools`：命中的用户工具不接进来。
    pub fn connect(spec: &str, disallowed: &[String]) -> Result<Self, String> {
        let exe = spec
            .strip_prefix("stdio:")
            .ok_or_else(|| format!("不支持的 MCP 传输: {spec}"))?
            .trim();
        if exe.is_empty() {
            return Err("MCP server 路径为空".into());
        }

        let mut cmd = Command::new(exe);
        cmd.arg("--mcp-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // MCP server 的日志直接并进本进程 stderr（上游会转发到前端/终端）；
            // stdout 必须独占给 JSON-RPC，不能混入任何别的输出。
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().map_err(|e| format!("spawn {exe}: {e}"))?;
        let stdin = child.stdin.take().ok_or("MCP stdin 不可用")?;
        let stdout = child.stdout.take().ok_or("MCP stdout 不可用")?;

        // 读线程：stdout 每行一条 JSON-RPC 消息 → mpsc，主线程按 id 取用
        let (tx, rx) = mpsc::channel::<Value>();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
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
                    Err(e) => eprintln!("[agent] MCP 非 JSON 输出（已忽略）: {e}"),
                }
            }
        });

        let mut bridge = Bridge {
            child,
            stdin,
            rx,
            next_id: 0,
            names: HashMap::new(),
            defs: Vec::new(),
        };
        bridge.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "lunac-agent", "version": "0.2.0" },
            }),
            HANDSHAKE_TIMEOUT,
        )?;
        bridge.notify("notifications/initialized", json!({}))?;
        let listed = bridge.request("tools/list", json!({}), HANDSHAKE_TIMEOUT)?;
        bridge.adopt_tools(listed.get("tools"), disallowed);
        Ok(bridge)
    }

    pub fn defs(&self) -> &[Value] {
        &self.defs
    }

    /// 调用一个 MCP 工具，返回给模型看的文本。
    /// 上游 `isError=true` 时转成 Err —— 调用方会包成 is_error 的 tool_result。
    pub fn call(&mut self, name: &str, input: &Value) -> Result<String, String> {
        let raw = self
            .names
            .get(name)
            .cloned()
            .ok_or_else(|| format!("未知 MCP 工具: {name}"))?;
        let res = self.request(
            "tools/call",
            json!({ "name": raw, "arguments": input }),
            CALL_TIMEOUT,
        )?;
        let text = content_text(res.get("content"));
        if res.get("isError").and_then(Value::as_bool).unwrap_or(false) {
            Err(text)
        } else {
            Ok(text)
        }
    }

    /// 把 `tools/list` 的结果转成请求体的工具 schema。
    fn adopt_tools(&mut self, tools: Option<&Value>, disallowed: &[String]) {
        let Some(list) = tools.and_then(Value::as_array) else {
            return;
        };
        for tool in list {
            let Some(raw) = tool.get("name").and_then(Value::as_str) else {
                continue;
            };
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let name = self.unique_name(raw);
            // 黑名单按原名或带前缀名任一命中即不接入
            if disallowed.iter().any(|d| d == raw || d == &name) {
                eprintln!("[agent] MCP 工具 {name} 已被 --disallowedTools 禁用");
                continue;
            }
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let schema = tool
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
            self.names.insert(name.clone(), raw.to_string());
            self.defs.push(json!({
                "name": name,
                "description": description,
                "input_schema": schema,
            }));
        }
    }

    fn unique_name(&self, raw: &str) -> String {
        let cleaned: String = raw
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .take(MAX_RAW_NAME)
            .collect();
        let mut name = format!("{PREFIX}{cleaned}");
        let mut n = 2;
        while self.names.contains_key(&name) {
            name = format!("{PREFIX}{cleaned}_{n}");
            n += 1;
        }
        if cleaned != raw {
            eprintln!("[agent] MCP 工具名 {raw} → {name}（含非法字符或超长）");
        }
        name
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }))?;
        loop {
            match self.rx.recv_timeout(timeout) {
                Ok(msg) => {
                    // 只认自己的 id：服务端通知（无 id）等一律跳过
                    if msg.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    if let Some(err) = msg.get("error") {
                        let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
                        let text = err.get("message").and_then(Value::as_str).unwrap_or("");
                        return Err(format!("MCP {method} 报错 {code}: {text}"));
                    }
                    return msg
                        .get("result")
                        .cloned()
                        .ok_or_else(|| format!("MCP {method} 回包既无 result 也无 error"));
                }
                Err(RecvTimeoutError::Timeout) => {
                    let _ = self.child.kill();
                    return Err(format!("MCP {method} 超时（{}s）", timeout.as_secs()));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!("MCP 连接已关闭（{method}）"))
                }
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    fn send(&mut self, msg: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .map_err(|e| format!("写 MCP stdin 失败: {e}"))?;
        self.stdin
            .flush()
            .map_err(|e| format!("flush MCP stdin 失败: {e}"))
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // agent.exe 退出时一并收掉 MCP server（它只是个 stdio 子进程）
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// MCP 的 `content` 是 block 数组（`[{type:"text",text:"…"}]`），
/// 非文本块（image 等）退化成 JSON 文本，至少让模型看得见有东西。
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|b| {
                b.get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| b.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}
