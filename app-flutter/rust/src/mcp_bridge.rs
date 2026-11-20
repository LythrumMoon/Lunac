//! 宿主 MCP 桥：agent 以 `<lunac.exe> --mcp-server` 拉起本进程，走 stdio 上的
//! **行分隔 JSON-RPC 2.0**（与 v1 `app/src-tauri/src/mcp_server.rs` 同协议）。
//!
//! 为什么必须有它：`core-agent` 的 11 个「桥工具」（`Remember` / `MemorySearch` /
//! `SessionSearch` / `PetGrowth` / `PetPersonaSuggest` / 资源库 / MCP 读侧）以及**记忆注入**
//! （`fetch_memory`）都只经这条桥读写宿主数据。没有桥 ⇒ 长期记忆永远是空的、
//! `Remember` 直接失败、桌宠成长笔记写不进去。
//!
//! 本轮重建的范围（其余按「如实登记」写进 `docs/todo.md`）：
//!   · `initialize`（含 roots / elicitation 能力声明）/ `tools/list` / `tools/call` /
//!     `resources/list` / `resources/read` / `prompts/list` / `prompts/get`；
//!   · **用户工具**（`<exe 根>/tools/*.json`）：`shell` / `http` / `builtin` 三种 handler，
//!     支持 `{{key}}` 占位替换、60s 硬超时、`elicit` 声明先向用户弹卡（与服务端→客户端
//!     的 `roots/list` / `elicitation/create` 反向请求同一套机制）；
//!   · **资源**：把 `tools/*.json` 报成 resource 并原样读回（只允许该目录内的 `.json`，
//!     用 `canonicalize()` 比前缀做边界检查）；
//!   · **提示词模板**（`<exe 根>/prompts/*.md`，YAML frontmatter 的 `description:`）；
//!   · 自定义方法（**不列进 `tools/list`**，与 v1 同口径）：
//!     `lunac/memory_read`、`lunac/memory_write`、`lunac/pet_growth_read`、
//!     `lunac/pet_growth_write`、`lunac/persona_suggestion_write`、`lunac/history_search`、
//!     `lunac/resource_hub_sources`、`lunac/resource_hub_install`（后端在 `resource_hub.rs`）。
//!
//! **未实现（与 v1 同口径，不是缺口）**：`sampling/createMessage`。v1 `mcp_server.rs` 里
//! 也没有它（全文件 0 命中）⇒ MCP 的「服务端反向请求客户端采样」这条能力 v1 同样没有；
//! 被调用时**如实回一句原因**，不假装成功。
//! `builtin` 目前只有 `image_pattern_analysis`（v1 `analyze_image_pattern` 原样移植，
//! 见 `image_pattern.rs`）。
//!
//! stdout 必须独占给 JSON-RPC：本模块只往 stdout 写协议行，诊断一律走 stderr。

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// 与 v1 一致：声明 elicitation / resources / prompts 同一版本线。
const PROTOCOL_VERSION: &str = "2025-06-18";
/// 单个 resource 的读取上限（用户工具定义是几 KB 的 JSON）。
const MAX_RESOURCE_BYTES: u64 = 512 * 1024;
/// prompt 正文读取上限。
const MAX_PROMPT_BYTES: u64 = 256 * 1024;
/// shell handler 的硬超时（v1 同值）。
const SHELL_TIMEOUT_SECS: u64 = 60;

// ── 用户工具定义（`<exe 根>/tools/*.json`，与 v1 同结构）──────────────

#[derive(Debug, Clone, serde::Deserialize)]
struct ToolDef {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "inputSchema")]
    input_schema: Value,
    handler: ToolHandler,
    /// 声明了就按 `elicitation/create` 先向**用户**弹卡，答案并入 `arguments` 再执行。
    #[serde(default)]
    elicit: Option<ElicitSpec>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ElicitSpec {
    message: String,
    #[serde(default, rename = "requestedSchema")]
    requested_schema: Value,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type")]
enum ToolHandler {
    #[serde(rename = "shell")]
    Shell { command: String },
    #[serde(rename = "http")]
    Http {
        method: String,
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default)]
        body: Option<String>,
    },
    /// 进程内执行的内置工具（v2 目前只保留分派点，`image_pattern_analysis` 未移植）。
    #[serde(rename = "builtin")]
    Builtin { name: String },
}

// ── 会话：stdio 上的**双向**请求（roots / elicitation）──────────────────
//
// v1 同款设计：主循环此刻正阻塞在 `handle_request` 里，不会同时读写 stdio，
// 所以「服务端主动发请求 + 在同一线程里读回包」没有并发竞争。
struct Session<'a> {
    root: PathBuf,
    input: &'a mut dyn BufRead,
    output: &'a mut dyn Write,
    /// 服务端自己发出的请求 id（与客户端 id 空间独立：靠「有没有 method」区分请求/响应）。
    next_id: u64,
    /// 客户端在 `initialize` 里声明了 roots 能力？（没声明就不该发 `roots/list`）
    client_roots: bool,
    roots_asked: bool,
    /// 工作区根（`roots/list` 的第一条）；`None` = 没拿到 / 客户端不支持。
    workspace: Option<PathBuf>,
}

impl<'a> Session<'a> {
    fn new(root: &Path, input: &'a mut dyn BufRead, output: &'a mut dyn Write) -> Self {
        Self {
            root: root.to_path_buf(),
            input,
            output,
            next_id: 0,
            client_roots: false,
            roots_asked: false,
            workspace: None,
        }
    }

    /// 写一条协议行（并 flush）；失败 = 管道断了。
    fn write_line(&mut self, value: &Value) -> bool {
        let Ok(text) = serde_json::to_string(value) else {
            return true;
        };
        if writeln!(self.output, "{text}").is_err() {
            return false;
        }
        self.output.flush().is_ok()
    }

    /// 向客户端发一个请求并等回包：跳过期间到达的通知 / 别的消息，只认 id 对上的响应。
    fn send_request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let message = json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        });
        if !self.write_line(&message) {
            return Err("写 stdout 失败".to_string());
        }
        let mut line = String::new();
        loop {
            line.clear();
            let read = self
                .input
                .read_line(&mut line)
                .map_err(|e| format!("读 stdin 失败: {e}"))?;
            if read == 0 {
                return Err("客户端关闭了连接".to_string());
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            // 客户端此刻不该反向发请求；真收到就跳过（本实现不支持）。
            if value.get("method").is_some() {
                continue;
            }
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(format!("客户端返回错误: {error}"));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// 取工作区根（懒加载一次）；客户端不支持 roots ⇒ `None`（shell 沿用继承的 cwd）。
    fn workspace_root(&mut self) -> Option<PathBuf> {
        if !self.client_roots {
            return None;
        }
        if !self.roots_asked {
            self.roots_asked = true;
            self.workspace = match self.send_request("roots/list", json!({})) {
                Ok(result) => result
                    .get("roots")
                    .and_then(Value::as_array)
                    .and_then(|roots| roots.first())
                    .and_then(|root| root.get("uri"))
                    .and_then(Value::as_str)
                    .and_then(file_uri_to_path),
                Err(reason) => {
                    eprintln!("[lunac-mcp] roots/list 失败（沿用继承的 cwd）：{reason}");
                    None
                }
            };
        }
        self.workspace.clone()
    }
}

/// `file:///C:/x/y` → `C:\x\y`（只处理本机绝对路径）。
fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri
        .strip_prefix("file:///")
        .or_else(|| uri.strip_prefix("file://"))?;
    if rest.is_empty() {
        return None;
    }
    Some(PathBuf::from(rest))
}

// ── 工具 / 资源 / 提示词的目录 ────────────────────────────────────────

fn tools_dir(root: &Path) -> PathBuf {
    root.join("tools")
}

fn prompts_dir(root: &Path) -> PathBuf {
    root.join("prompts")
}

/// 读 `tools/*.json`：坏文件只往 stderr 记一行，绝不因此让整条桥失败。
fn load_tools(root: &Path) -> Vec<ToolDef> {
    let dir = tools_dir(root);
    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
        return Vec::new();
    }
    let mut tools = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            match serde_json::from_str::<ToolDef>(&content) {
                Ok(tool) => tools.push(tool),
                Err(error) => eprintln!("[lunac-mcp] 跳过坏工具定义 {}：{error}", path.display()),
            }
        }
    }
    tools
}

/// `tools/list` 的 `result`：只给 name / description / inputSchema（**不暴露 handler**）。
fn build_tools_list(tools: &[ToolDef]) -> Value {
    let items: Vec<Value> = tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema,
            })
        })
        .collect();
    json!({ "tools": items, "nextCursor": null })
}

/// 把 `uri` 解析成「`root` 之内一个 `.json` 文件」。
///
/// **安全边界（硬）**：`uri` 来自模型，而模型会被读到的文件内容提示注入 ⇒ 只允许
/// `tools\` 目录内的 `.json`；判据是 `canonicalize()` 之后比前缀（`..` / 符号链接都会展开）。
fn resolve_resource(root: &Path, uri: &str) -> Result<PathBuf, String> {
    let trimmed = uri.trim();
    if trimmed.is_empty() {
        return Err("缺少 uri 参数".to_string());
    }
    let candidate = if let Some(rest) = trimmed.strip_prefix("file:///") {
        PathBuf::from(rest)
    } else if trimmed.contains("://") {
        return Err(format!("只支持 file:// 资源，收到：{trimmed}"));
    } else {
        if trimmed.contains(['/', '\\']) {
            return Err("资源名不得包含路径分隔符，请传 resources/list 给出的 uri".to_string());
        }
        tools_dir(root).join(format!("{}.json", trimmed.trim_end_matches(".json")))
    };
    let dir = tools_dir(root);
    let real_root = dir
        .canonicalize()
        .map_err(|e| format!("tools 目录不可用：{e}"))?;
    let real = candidate
        .canonicalize()
        .map_err(|_| format!("资源不存在：{}", candidate.display()))?;
    if !real.starts_with(&real_root) {
        return Err(format!(
            "拒绝访问：{} 不在 tools 目录内（只允许读该目录下的 .json）",
            real.display()
        ));
    }
    if !real.is_file() {
        return Err(format!("不是文件：{}", real.display()));
    }
    if real.extension().is_none_or(|ext| ext != "json") {
        return Err("只支持 .json 工具定义文件".to_string());
    }
    let size = std::fs::metadata(&real).map(|meta| meta.len()).unwrap_or(0);
    if size > MAX_RESOURCE_BYTES {
        return Err(format!(
            "资源过大（{size} 字节，上限 {MAX_RESOURCE_BYTES}）"
        ));
    }
    Ok(real)
}

/// `resources/read` 的 `result`（规范形状 `{contents:[{uri,mimeType,text}]}`）。
fn read_resource_result(root: &Path, uri: &str) -> Result<Value, String> {
    let path = resolve_resource(root, uri)?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("读取失败：{e}"))?;
    Ok(json!({
        "contents": [{
            "uri": format!("file:///{}", path.display()),
            "mimeType": "application/json",
            "text": text,
        }]
    }))
}

/// 一个可用的 prompt 模板（`prompts/<name>.md`）。
struct PromptDef {
    name: String,
    description: String,
    body: String,
}

/// 拆出可选的 YAML frontmatter（`---\n…\n---`），返回 `(frontmatter, 正文)`。
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let text = text.trim_start_matches('\u{feff}');
    if let Some(rest) = text.strip_prefix("---") {
        if let Some(index) = rest.find("\n---") {
            let frontmatter = &rest[..index];
            let body = rest[index + 4..].trim_start_matches(['\r', '\n']);
            return (Some(frontmatter), body);
        }
    }
    (None, text)
}

/// 从 frontmatter 里取一个 `key: value`（值去掉两侧引号）。
fn fm_field<'a>(frontmatter: &'a str, key: &str) -> Option<&'a str> {
    frontmatter.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(':'))
            .map(|value| value.trim().trim_matches('"'))
            .filter(|value| !value.is_empty())
    })
}

/// prompt 描述：优先 frontmatter 的 `description:`，否则正文第一行非空（去掉前导 `#`）。
fn prompt_description(frontmatter: Option<&str>, body: &str) -> String {
    if let Some(description) = frontmatter.and_then(|fm| fm_field(fm, "description")) {
        return description.to_string();
    }
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches('#').trim().to_string())
        .unwrap_or_default()
}

/// 扫 `prompts/` 目录，按文件名排序（顺序稳定 ⇒ `prompts/list` 排列稳定）。
fn load_prompts(root: &Path) -> Vec<PromptDef> {
    let dir = prompts_dir(root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut prompts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        if size > MAX_PROMPT_BYTES {
            eprintln!("[lunac-mcp] 跳过过大的 prompt（{size} 字节）：{}", path.display());
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if name.trim().is_empty() {
            continue;
        }
        let (frontmatter, body) = split_frontmatter(&raw);
        prompts.push(PromptDef {
            description: prompt_description(frontmatter, body),
            name,
            body: body.to_string(),
        });
    }
    prompts.sort_by(|a, b| a.name.cmp(&b.name));
    prompts
}

fn build_prompts_list(prompts: &[PromptDef]) -> Value {
    let items: Vec<Value> = prompts
        .iter()
        .map(|prompt| json!({ "name": prompt.name, "description": prompt.description }))
        .collect();
    json!({ "prompts": items, "nextCursor": null })
}

/// `prompts/get` 的 `result`：`{description, messages:[{role,content}]}`；
/// `arguments` 按 `{{key}}` / `{{key }}` 两种写法替换（复用 `resolve_template`）。
fn prompt_get_result(prompts: &[PromptDef], name: &str, arguments: &Value) -> Result<Value, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("缺少 name 参数：要传 prompts/list 给出的 prompt 名".to_string());
    }
    let Some(prompt) = prompts.iter().find(|prompt| prompt.name == name) else {
        return Err(format!("prompt 不存在：{name}"));
    };
    let text = resolve_template(&prompt.body, arguments);
    Ok(json!({
        "description": prompt.description,
        "messages": [{
            "role": "user",
            "content": { "type": "text", "text": text },
        }]
    }))
}

// ── 工具执行 ──────────────────────────────────────────────────────────

/// 把 `{{key}}` / `{{key }}` 占位替换成入参值（字符串原样、其它走 JSON 文本）。
fn resolve_template(template: &str, params: &Value) -> String {
    let mut result = template.to_string();
    if let Some(object) = params.as_object() {
        for (key, value) in object {
            let text = match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            result = result.replace(&format!("{{{{{key} }}}}"), &text);
            result = result.replace(&format!("{{{{{key}}}}}"), &text);
        }
    }
    result
}

/// 跑一条 shell 命令（60s 硬超时；stdout/stderr 后台线程抽干，避免管道写满死锁）。
fn run_shell_with_timeout(
    command: &str,
    timeout_secs: u64,
    cwd: Option<&Path>,
) -> Result<String, String> {
    // 必须 `raw_arg` 原样拼命令行：`command` 来自用户的工具定义，带引号 + 空格是常态，
    // 而 `Command::arg` 会按 MSVC 规则把 `"` 转义成 `\"`（cmd 不认这个转义）。
    #[cfg(target_os = "windows")]
    let mut child_command = {
        use std::os::windows::process::CommandExt;
        let mut command_line = Command::new("cmd");
        command_line.raw_arg("/c").raw_arg(command);
        // CREATE_NO_WINDOW：GUI 进程（release）下不设会弹出 cmd 窗口。
        command_line.creation_flags(0x0800_0000);
        command_line
    };
    #[cfg(not(target_os = "windows"))]
    let mut child_command = {
        let mut command_line = Command::new("sh");
        command_line.args(["-c", command]);
        command_line
    };
    if let Some(dir) = cwd {
        child_command.current_dir(dir);
    }
    child_command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = child_command
        .spawn()
        .map_err(|e| format!("Failed to execute: {e}"))?;
    let stdout = child.stdout.take().map(|pipe| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buffer = String::new();
            let mut pipe = pipe;
            let _ = pipe.read_to_string(&mut buffer);
            buffer
        })
    });
    let stderr = child.stderr.take().map(|pipe| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buffer = String::new();
            let mut pipe = pipe;
            let _ = pipe.read_to_string(&mut buffer);
            buffer
        })
    });

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "命令执行超时（{timeout_secs}s），已终止。耗时任务请改为后台运行。"
                ));
            }
        }
    };

    let stdout = stdout.map(|handle| handle.join().unwrap_or_default()).unwrap_or_default();
    let stderr = stderr.map(|handle| handle.join().unwrap_or_default()).unwrap_or_default();
    if status.success() {
        if !stdout.trim().is_empty() {
            Ok(stdout)
        } else if !stderr.trim().is_empty() {
            Ok(stderr)
        } else {
            Ok("(no output)".to_string())
        }
    } else {
        Err(format!(
            "Command failed (exit {status}): {}",
            stderr.trim()
        ))
    }
}

fn execute_tool_handler(
    handler: &ToolHandler,
    params: &Value,
    cwd: Option<&Path>,
) -> Result<String, String> {
    match handler {
        ToolHandler::Builtin { name } => run_builtin_tool(name, params),
        ToolHandler::Shell { command } => {
            let resolved = resolve_template(command, params);
            run_shell_with_timeout(&resolved, SHELL_TIMEOUT_SECS, cwd)
        }
        ToolHandler::Http {
            method,
            url,
            headers,
            body,
        } => {
            let resolved_url = resolve_template(url, params);
            let resolved_body = body.as_ref().map(|body| resolve_template(body, params));
            let client = reqwest::blocking::Client::new();
            let mut request = match method.to_uppercase().as_str() {
                "GET" => client.get(&resolved_url),
                "POST" => {
                    let mut request = client.post(&resolved_url);
                    if let Some(body) = resolved_body.as_ref() {
                        request = request.body(body.clone());
                    }
                    request
                }
                "PUT" => {
                    let mut request = client.put(&resolved_url);
                    if let Some(body) = resolved_body.as_ref() {
                        request = request.body(body.clone());
                    }
                    request
                }
                "DELETE" => client.delete(&resolved_url),
                other => return Err(format!("Unsupported HTTP method: {other}")),
            };
            for (key, value) in headers {
                request = request.header(key.as_str(), value.as_str());
            }
            let response = request
                .send()
                .map_err(|e| format!("HTTP request failed: {e}"))?;
            let status = response.status();
            let text = response
                .text()
                .map_err(|e| format!("Failed to read response: {e}"))?;
            if status.is_success() {
                Ok(text)
            } else {
                Err(format!("HTTP {}: {text}", status.as_u16()))
            }
        }
    }
}

/// 进程内内置工具分派（与 v1 同名同语义；目前只有图片图案分析）。
fn run_builtin_tool(name: &str, params: &Value) -> Result<String, String> {
    match name {
        "image_pattern_analysis" => {
            let path = params
                .get("path")
                .and_then(Value::as_str)
                .filter(|path| !path.trim().is_empty())
                .ok_or_else(|| "缺少 path 参数：需传入图片的绝对路径（jpg/png）".to_string())?;
            crate::image_pattern::analyze(path)
        }
        other => Err(format!("未知内置工具：{other}")),
    }
}

/// 执行一次用户工具调用：先按 `elicit` 声明弹卡收集输入，再执行 handler。
fn run_tool_def(
    session: &mut Session,
    tool: &ToolDef,
    arguments: &Value,
) -> Result<String, String> {
    let arguments = maybe_elicit(session, tool, arguments)?;
    let cwd = session.workspace_root();
    execute_tool_handler(&tool.handler, &arguments, cwd.as_deref())
}

/// `elicit` 声明的触发点：向客户端发 `elicitation/create`，等用户填完，把 `content`
/// **并入** `arguments`（同名键以用户填的为准）。没声明 elicit 的工具原样返回入参。
fn maybe_elicit(session: &mut Session, tool: &ToolDef, arguments: &Value) -> Result<Value, String> {
    let Some(spec) = tool.elicit.as_ref() else {
        return Ok(arguments.clone());
    };
    let result = session.send_request(
        "elicitation/create",
        json!({ "message": spec.message, "requestedSchema": spec.requested_schema }),
    )?;
    let action = result
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("cancel");
    if action != "accept" {
        return Err("用户拒绝了本次输入请求（工具未执行）".to_string());
    }
    let mut merged = if arguments.is_object() {
        arguments.clone()
    } else {
        json!({})
    };
    if let Some(content) = result.get("content").and_then(Value::as_object) {
        if let Some(object) = merged.as_object_mut() {
            for (key, value) in content {
                object.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(merged)
}

/// FFI 入口：阻塞跑完整个 stdio 会话（agent 关管道 = EOF ⇒ 返回 0）。
pub fn serve_stdio() -> i32 {
    let root = crate::exe_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    // 主程序（Flutter）在 debug 构建里会把「The Dart VM service is listening on …」写到
    // stdout **且不保证带换行**；先补一个换行，保证下面第一行 JSON 不会粘在横幅尾巴上
    // （粘上就会被 agent 当成非 JSON 丢弃 ⇒ initialize 超时）。独立控制台进程里这只是个空行。
    let _ = writeln!(stdout);
    let _ = stdout.flush();
    serve(&root, stdin.lock(), stdout.lock())
}

/// 主循环（拆出 `input` / `output` 只为可单测）。
///
/// 每行一条 JSON-RPC：空行跳过、非 JSON 跳过（v1 同口径 —— 绝不往 stdout 写非协议内容）；
/// 没有 `id` 的消息是通知，不回应。
pub fn serve(root: &Path, mut input: impl BufRead, mut output: impl Write) -> i32 {
    let mut session = Session::new(root, &mut input, &mut output);
    let mut line = String::new();
    loop {
        line.clear();
        let read = match session.input.read_line(&mut line) {
            Ok(read) => read,
            Err(_) => break,
        };
        if read == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(trimmed) else {
            eprintln!("[lunac-mcp] 忽略一条非 JSON 输入");
            continue;
        };
        let Some(response) = handle_request(&mut session, &request) else {
            continue;
        };
        if !session.write_line(&response) {
            break;
        }
    }
    0
}

/// 单条请求 → 一条回应；通知（无 id）返回 `None`。
fn handle_request(session: &mut Session, request: &Value) -> Option<Value> {
    let id = request.get("id").cloned()?;
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = request.get("params");
    let root = session.root.clone();
    Some(match method {
        "initialize" => {
            // 客户端声明了 roots 才允许反向发 `roots/list`（shell handler 的 cwd）。
            session.client_roots = params
                .and_then(|value| value.get("capabilities"))
                .and_then(|capabilities| capabilities.get("roots"))
                .is_some();
            ok(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {
                        "tools": {},
                        "resources": {},
                        "prompts": {},
                        // elicitation 由服务端在 `tools/call` 时按工具 JSON 的 `elicit` 字段发起。
                        "elicitation": {},
                    },
                    "serverInfo": { "name": "lunac-mcp", "version": "0.3.0" },
                }),
            )
        }
        "tools/list" => ok(id, build_tools_list(&load_tools(&root))),
        "tools/call" => {
            let name = params
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let arguments = params
                .and_then(|value| value.get("arguments"))
                .cloned()
                .unwrap_or(Value::Null);
            if name.is_empty() {
                return Some(err(id, -32602, "Missing tool name"));
            }
            let tools = load_tools(&root);
            match tools.iter().find(|tool| tool.name == name) {
                Some(tool) => match run_tool_def(session, tool, &arguments) {
                    Ok(text) => text_ok(id, text),
                    // 工具执行失败 = `result + isError`（不是 JSON-RPC error）。
                    Err(message) => text_err(id, message),
                },
                None => err(id, -32602, format!("Tool not found: {name}")),
            }
        }
        "resources/list" => {
            let dir = tools_dir(&root);
            let mut resources = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_none_or(|ext| ext != "json") {
                        continue;
                    }
                    resources.push(json!({
                        "uri": format!("file:///{}", path.display()),
                        "name": path.file_stem().unwrap_or_default().to_string_lossy(),
                        "mimeType": "application/json",
                    }));
                }
            }
            ok(id, json!({ "resources": resources }))
        }
        // 读侧：`uri` 来自模型 ⇒ 边界检查在 `read_resource_result` 里；
        // 标准方法的失败走 JSON-RPC error（只有 `tools/call` 才用 `result + isError`）。
        "resources/read" => {
            let uri = params
                .and_then(|value| value.get("uri"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            match read_resource_result(&root, uri) {
                Ok(result) => ok(id, result),
                Err(message) => err(id, -32002, message),
            }
        }
        "prompts/list" => ok(id, build_prompts_list(&load_prompts(&root))),
        "prompts/get" => {
            let name = params
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = params
                .and_then(|value| value.get("arguments"))
                .cloned()
                .unwrap_or(Value::Null);
            match prompt_get_result(&load_prompts(&root), name, &arguments) {
                Ok(result) => ok(id, result),
                Err(message) => err(id, -32002, message),
            }
        }
        "lunac/history_search" => {
            let query = param_str(params, "query");
            let limit = params
                .and_then(|value| value.get("limit"))
                .and_then(Value::as_u64)
                .unwrap_or(10) as usize;
            // 2026-11-20：图书馆式索引的可选收窄条件（旧调用方不传 = 全库检索）。
            let tags: Vec<String> = params
                .and_then(|value| value.get("tags"))
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|tag| !tag.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            text_ok(
                id,
                crate::chat_history::search_text_filtered(
                    &root,
                    &query,
                    &tags,
                    param_opt(params, "date"),
                    param_opt(params, "session"),
                    param_opt(params, "task"),
                    limit,
                ),
            )
        }
        "lunac/memory_read" => {
            let scope = match crate::memory::MemoryScope::parse(
                param_opt(params, "scope"),
                param_opt(params, "skill"),
            ) {
                Ok(scope) => scope,
                Err(reason) => return Some(text_err(id, reason)),
            };
            match crate::memory::read_scope(&root, &scope) {
                Ok(text) if text.trim().is_empty() => {
                    text_ok(id, "(long-term memory is empty)".to_string())
                }
                Ok(text) => text_ok(id, text),
                Err(reason) => text_err(id, reason),
            }
        }
        "lunac/memory_write" => {
            let scope = match crate::memory::MemoryScope::parse(
                param_opt(params, "scope"),
                param_opt(params, "skill"),
            ) {
                Ok(scope) => scope,
                Err(reason) => return Some(text_err(id, reason)),
            };
            let content = param_str(params, "content");
            let replace = params
                .and_then(|value| value.get("replace"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let page = match crate::memory::parse_page(param_opt(params, "page")) {
                Ok(page) => page,
                Err(reason) => return Some(text_err(id, reason)),
            };
            match crate::memory::write_entry(&root, &scope, &content, replace, page.as_deref()) {
                Ok(text) => text_ok(id, text),
                Err(reason) => text_err(id, reason),
            }
        }
        "lunac/pet_growth_read" => match crate::memory::read_pet_list_text(&root, "growth") {
            Ok(text) if text.trim().is_empty() => {
                text_ok(id, "(pet growth notes are empty)".to_string())
            }
            Ok(text) => text_ok(id, text),
            Err(reason) => text_err(id, reason),
        },
        "lunac/pet_growth_write" => {
            let content = param_str(params, "content");
            match crate::memory::append_pet_list(&root, "growth", &content) {
                Ok(text) => text_ok(id, text),
                Err(reason) => text_err(id, reason),
            }
        }
        "lunac/persona_suggestion_write" => {
            let content = param_str(params, "content");
            match crate::memory::append_pet_list(&root, "suggestions", &content) {
                Ok(text) => text_ok(id, text),
                Err(reason) => text_err(id, reason),
            }
        }
        "lunac/resource_hub_sources" => match crate::resource_hub::list_sources(&root) {
            Ok(items) => match serde_json::to_string(&serde_json::json!({
                "total": items.len(),
                "items": items,
            })) {
                Ok(text) => text_ok(id, text),
                Err(e) => text_err(id, format!("序列化来源列表失败：{e}")),
            },
            Err(reason) => text_err(id, reason),
        },
        "lunac/resource_hub_install" => {
            let Some(source) = params.and_then(|value| value.get("source")) else {
                return Some(text_err(
                    id,
                    "缺少 source 参数（形状 = { manifest, rule } 或 { manifest, script }）"
                        .to_string(),
                ));
            };
            let force = params
                .and_then(|value| value.get("force"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            match crate::resource_hub::install_source(&root, source, force) {
                Ok(path) => text_ok(id, format!("已安装来源：{path}")),
                Err(reason) => text_err(id, reason),
            }
        }
        other => err(id, -32601, format!("Method not found: {other}")),
    })
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

/// 自定义方法的成功回包（形状与 `tools/call` 一致：`result.content` + `isError`）。
fn text_ok(id: Value, text: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "content": [{ "type": "text", "text": text }], "isError": false },
    })
}

/// 自定义方法的失败回包：用 `result + isError`（「这次没做成」），不是 JSON-RPC error
/// （后者在 agent 侧会被当成「桥坏了 / 方法不存在」）。
fn text_err(id: Value, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "content": [{ "type": "text", "text": message }], "isError": true },
    })
}

fn param_str(params: Option<&Value>, key: &str) -> String {
    param_opt(params, key).unwrap_or_default().to_string()
}

fn param_opt<'a>(params: Option<&'a Value>, key: &str) -> Option<&'a str> {
    params
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-mcp-bridge-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run(root: &Path, requests: &[Value]) -> Vec<Value> {
        let mut input = String::new();
        for request in requests {
            input.push_str(&serde_json::to_string(request).unwrap());
            input.push('\n');
        }
        let mut output = Vec::new();
        let code = serve(root, Cursor::new(input.into_bytes()), &mut output);
        assert_eq!(code, 0);
        String::from_utf8(output)
            .unwrap()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("回包是 JSON"))
            .collect()
    }

    fn request(id: u64, method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    }

    #[test]
    fn initialize_handshake_and_notifications() {
        let root = temp_root("init");
        let replies = run(
            &root,
            &[
                request(1, "initialize", json!({ "protocolVersion": "2025-06-18" })),
                json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
                request(2, "tools/list", json!({})),
                request(3, "resources/list", json!({})),
                request(4, "prompts/list", json!({})),
            ],
        );
        // 通知不产生回包 ⇒ 4 条回包对应 4 个请求。
        assert_eq!(replies.len(), 4);
        assert_eq!(replies[0]["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(replies[1]["result"]["tools"], json!([]));
        assert_eq!(replies[2]["result"]["resources"], json!([]));
        assert_eq!(replies[3]["result"]["prompts"], json!([]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_write_then_read_adds_provenance_and_dedupes() {
        let root = temp_root("memory");
        let replies = run(
            &root,
            &[
                request(
                    1,
                    "lunac/memory_write",
                    json!({ "content": "项目用 cargo test --lib 验收" }),
                ),
                // 同一条正文再写一次：不追加，只刷新日期。
                request(
                    2,
                    "lunac/memory_write",
                    json!({ "content": "项目用 cargo test --lib 验收" }),
                ),
                request(3, "lunac/memory_read", json!({})),
                request(
                    4,
                    "lunac/memory_write",
                    json!({ "content": "模型自带假日期", "scope": "bad" }),
                ),
            ],
        );
        assert_eq!(replies[0]["result"]["isError"], false);
        assert!(replies[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("already remembered"));
        let text = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("- ["), "带来源前缀：{text}");
        assert!(text.contains(" · global] "), "来源作用域：{text}");
        assert_eq!(text.matches("项目用 cargo test").count(), 1, "去重：{text}");
        // 未知作用域：走 isError（不是 JSON-RPC error）。
        assert_eq!(replies[3]["result"]["isError"], true);
        assert!(replies[3].get("error").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_write_respects_scope_and_replace() {
        let root = temp_root("scope");
        let replies = run(
            &root,
            &[
                request(
                    1,
                    "lunac/memory_write",
                    json!({ "content": "桌宠私有", "scope": "pet" }),
                ),
                request(
                    2,
                    "lunac/memory_write",
                    json!({ "content": "- 合并后的一条", "replace": true }),
                ),
                request(3, "lunac/memory_read", json!({ "scope": "pet" })),
                request(4, "lunac/memory_read", json!({})),
            ],
        );
        let pet = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(pet.contains("桌宠私有"));
        assert!(pet.contains("· pet]"), "{pet}");
        let global = replies[3]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(global.contains("合并后的一条"));
        assert!(!global.contains("桌宠私有"), "作用域隔离：{global}");
        // replace 之后正文里只剩一条。
        assert_eq!(global.matches("- ").count(), 1, "{global}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_memory_reports_the_v1_receipt() {
        let root = temp_root("empty");
        let replies = run(&root, &[request(1, "lunac/memory_read", json!({}))]);
        assert_eq!(
            replies[0]["result"]["content"][0]["text"],
            "(long-term memory is empty)"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pet_growth_and_suggestions_are_append_only() {
        let root = temp_root("pet");
        let replies = run(
            &root,
            &[
                request(1, "lunac/pet_growth_read", json!({})),
                request(2, "lunac/pet_growth_write", json!({ "content": "今天学会歪头" })),
                request(3, "lunac/pet_growth_read", json!({})),
                request(
                    4,
                    "lunac/persona_suggestion_write",
                    json!({ "content": "可以更爱笑一点" }),
                ),
            ],
        );
        assert_eq!(
            replies[0]["result"]["content"][0]["text"],
            "(pet growth notes are empty)"
        );
        assert_eq!(replies[1]["result"]["isError"], false);
        let text = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("- 今天学会歪头"), "{text}");
        assert_eq!(replies[3]["result"]["isError"], false);
        let suggestions = crate::memory::list_pet_entries(&root, "suggestions").unwrap();
        assert_eq!(suggestions, vec!["可以更爱笑一点".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn history_search_finds_past_messages() {
        let root = temp_root("history");
        crate::chat_history::upsert(
            &root,
            &json!({
                "id": "s1",
                "title": "代理排查",
                "createdAt": 1_700_000_000_000u64,
                "updatedAt": 1_700_000_100_000u64,
                "messages": [
                    { "role": "user", "content": "帮我看看 vless 为什么连不上" },
                    { "role": "assistant", "content": "先检查 UUID 与 flow 是否匹配" },
                ],
            }),
        )
        .unwrap();
        let replies = run(
            &root,
            &[
                request(1, "lunac/history_search", json!({ "query": "VLESS" })),
                request(2, "lunac/history_search", json!({ "query": "不存在的词" })),
            ],
        );
        let hit = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(hit.contains("代理排查"), "{hit}");
        assert!(hit.contains("vless"), "大小写不敏感：{hit}");
        let miss = replies[1]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(miss.starts_with("No past message matched"), "{miss}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_method_unknown_tool_and_unknown_resource_are_honest() {
        let root = temp_root("unknown");
        let replies = run(
            &root,
            &[
                request(1, "nope/nothing", json!({})),
                request(2, "resources/read", json!({ "uri": "x.json" })),
                request(
                    3,
                    "tools/call",
                    json!({ "name": "weather", "arguments": {} }),
                ),
                request(4, "lunac/resource_hub_sources", json!({})),
            ],
        );
        assert_eq!(replies[0]["error"]["code"], -32601);
        assert_eq!(replies[1]["error"]["code"], -32002);
        // 未知工具 = JSON-RPC error（v1 口径），不是「桥坏了」的 isError。
        assert_eq!(replies[2]["error"]["code"], -32602);
        assert!(replies[2]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Tool not found"));
        // 资源库（`lunac/resource_hub_*`）2026-11-15 已接上宿主实现：空目录 = total 0。
        let listed = replies[3]["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(listed).unwrap();
        assert_eq!(parsed["total"], json!(0));
        assert_eq!(parsed["items"], json!([]));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 资源库来源：AI 侧的 `SourceInstall` / `ResourceHubSearch` 走的就是这两条桥方法。
    #[test]
    fn resource_hub_bridge_installs_and_lists_sources() {
        let root = temp_root("resource-hub");
        let source = json!({
            "manifest": {
                "rsp": 1,
                "id": "demo.bridge",
                "name": "桥上的来源",
                "version": "1.0.0",
                "kind": ["comic"],
                "engine": "declarative",
                "capabilities": ["search", "chapters"],
            },
            "rule": { "search": { "url": "https://example.com/s?q={query}" } },
        });
        let replies = run(
            &root,
            &[
                request(
                    1,
                    "lunac/resource_hub_install",
                    json!({ "source": source, "force": false }),
                ),
                request(2, "lunac/resource_hub_sources", json!({})),
                // 缺 source 参数 ⇒ result + isError（与其它桥方法同口径）
                request(3, "lunac/resource_hub_install", json!({})),
                // 不合法来源 ⇒ isError，且不落盘
                request(
                    4,
                    "lunac/resource_hub_install",
                    json!({ "source": { "manifest": { "id": "../evil" } } }),
                ),
            ],
        );
        let installed = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(installed.contains("已安装来源"), "{installed}");
        assert!(installed.ends_with("demo.bridge.json"), "{installed}");

        let listed = replies[1]["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(listed).unwrap();
        assert_eq!(parsed["total"], json!(1));
        assert_eq!(parsed["items"][0]["manifest"]["id"], json!("demo.bridge"));
        assert!(parsed["items"][0]["rule"].is_object());

        assert_eq!(replies[2]["result"]["isError"], json!(true));
        assert!(replies[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("source"));
        assert_eq!(replies[3]["result"]["isError"], json!(true));
        assert_eq!(
            crate::resource_hub::list_sources(&root).unwrap().len(),
            1,
            "非法来源不能落盘"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn write_tool(root: &Path, name: &str, body: &str) {
        let dir = tools_dir(root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.json")), body).unwrap();
    }

    #[test]
    fn user_tools_are_listed_and_shell_handler_runs_with_arguments() {
        let root = temp_root("tools");
        write_tool(
            &root,
            "echo",
            r#"{"name":"echo","description":"回显文本","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}},"handler":{"type":"shell","command":"echo {{text}}"}}"#,
        );
        // 坏文件不能把整条桥带崩。
        write_tool(&root, "broken", "{ not json");
        // 非 0 退出码 ⇒ result + isError（不是 JSON-RPC error）。
        write_tool(
            &root,
            "fail",
            r#"{"name":"fail","description":"注定失败","handler":{"type":"shell","command":"exit 3"}}"#,
        );
        let replies = run(
            &root,
            &[
                request(1, "tools/list", json!({})),
                request(2, "tools/call", json!({ "name": "echo", "arguments": { "text": "hello-mcp" } })),
                request(3, "tools/call", json!({ "name": "fail", "arguments": {} })),
            ],
        );
        let tools = replies[0]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2, "坏定义要被跳过");
        let echo = tools.iter().find(|tool| tool["name"] == "echo").unwrap();
        assert_eq!(echo["description"], "回显文本");
        assert!(echo.get("handler").is_none(), "handler 不能暴露给模型");
        assert_eq!(replies[1]["result"]["isError"], false);
        assert!(replies[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("hello-mcp"));
        assert_eq!(replies[2]["result"]["isError"], true);
        assert!(replies[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Command failed"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn elicit_answer_is_merged_into_tool_arguments() {
        let root = temp_root("elicit");
        write_tool(
            &root,
            "ask",
            r#"{"name":"ask","description":"先问再答","handler":{"type":"shell","command":"echo {{text}}"},"elicit":{"message":"你的名字？","requestedSchema":{"type":"object","properties":{"text":{"type":"string"}}}}}"#,
        );
        // 服务端会先发 `elicitation/create`（它的 id 从 1 开始）⇒ 这里把回包直接摆在后一行。
        let replies = run(
            &root,
            &[
                request(1, "tools/call", json!({ "name": "ask", "arguments": {} })),
                json!({ "jsonrpc": "2.0", "id": 1, "result": { "action": "accept", "content": { "text": "bob" } } }),
            ],
        );
        // 输出里多出一条「服务端 → 客户端」的 elicitation/create（id 1），后面才是工具结果。
        let ask = replies
            .iter()
            .find(|reply| reply.get("method").is_some())
            .expect("应当先发 elicitation/create");
        assert_eq!(ask["method"], "elicitation/create");
        assert_eq!(ask["params"]["message"], "你的名字？");
        let response = replies
            .iter()
            .find(|reply| reply.get("method").is_none())
            .expect("应当有工具结果");
        assert_eq!(response["result"]["isError"], false);
        assert!(response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("bob"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn elicit_cancel_aborts_the_tool() {
        let root = temp_root("elicit-cancel");
        write_tool(
            &root,
            "ask",
            r#"{"name":"ask","description":"先问再答","handler":{"type":"shell","command":"echo {{text}}"},"elicit":{"message":"你的名字？"}}"#,
        );
        let replies = run(
            &root,
            &[
                request(1, "tools/call", json!({ "name": "ask", "arguments": {} })),
                json!({ "jsonrpc": "2.0", "id": 1, "result": { "action": "cancel" } }),
            ],
        );
        let response = replies
            .iter()
            .find(|reply| reply.get("method").is_none())
            .expect("应当有工具结果");
        assert_eq!(response["result"]["isError"], true);
        assert!(response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("用户拒绝"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resources_are_listed_and_read_but_confined_to_tools_dir() {
        let root = temp_root("resources");
        write_tool(&root, "echo", r#"{"name":"echo","description":"d","handler":{"type":"shell","command":"echo hi"}}"#);
        let listed = run(&root, &[request(1, "resources/list", json!({}))]);
        let resources = listed[0]["result"]["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 1);
        let uri = resources[0]["uri"].as_str().unwrap().to_string();
        assert_eq!(resources[0]["mimeType"], "application/json");

        let read = run(&root, &[request(1, "resources/read", json!({ "uri": uri }))]);
        assert!(read[0]["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("echo hi"));

        // 越界（`..`）、非 .json、外部绝对路径一律拒（-32002）。
        for bad in [
            "file:///C:/Windows/win.ini",
            "../secret.json",
            "echo.txt",
        ] {
            let replies = run(&root, &[request(1, "resources/read", json!({ "uri": bad }))]);
            assert_eq!(replies[0]["error"]["code"], -32002, "应拒绝：{bad}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prompts_are_listed_and_expanded_with_arguments() {
        let root = temp_root("prompts");
        let dir = prompts_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("greet.md"),
            "---\ndescription: 打招呼模板\n---\n# Greet\n\nHello {{name }}!",
        )
        .unwrap();
        let replies = run(
            &root,
            &[
                request(1, "prompts/list", json!({})),
                request(2, "prompts/get", json!({ "name": "greet", "arguments": { "name": "Lunac" } })),
                request(3, "prompts/get", json!({ "name": "nope", "arguments": {} })),
            ],
        );
        let prompts = replies[0]["result"]["prompts"].as_array().unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0]["name"], "greet");
        assert_eq!(prompts[0]["description"], "打招呼模板");
        let text = replies[1]["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("Hello Lunac!"), "占位符要被替换：{text}");
        assert_eq!(replies[2]["error"]["code"], -32002);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn builtin_image_pattern_analysis_runs_inside_the_host() {
        let root = temp_root("builtin");
        // 生成一张 40×40 的黑白图（左白右黑）。
        let image_path = root.join("probe.png");
        let image = image::RgbImage::from_fn(40, 40, |x, _y| {
            if x < 20 {
                image::Rgb([255, 255, 255])
            } else {
                image::Rgb([0, 0, 0])
            }
        });
        image.save(&image_path).unwrap();
        write_tool(
            &root,
            "pattern",
            r#"{"name":"pattern","description":"看图","inputSchema":{"type":"object","properties":{"path":{"type":"string"}}},"handler":{"type":"builtin","name":"image_pattern_analysis"}}"#,
        );
        let replies = run(
            &root,
            &[request(
                1,
                "tools/call",
                json!({ "name": "pattern", "arguments": { "path": image_path.to_str().unwrap() } }),
            )],
        );
        assert_eq!(replies[0]["result"]["isError"], false);
        let text = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("图片尺寸: 40×40px"), "{text}");
        assert!(text.contains("构图建议"), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_template_handles_both_placeholder_styles() {
        let params = json!({ "name": "Lunac", "count": 3 });
        // v1 支持的两种写法：`{{key}}` 与 `{{key }}`（键后一个空格）。
        assert_eq!(
            resolve_template("{{name}} x{{count }}", &params),
            "Lunac x3"
        );
        // 未知键原样保留（不静默清空）。
        assert_eq!(resolve_template("{{missing}}", &params), "{{missing}}");
    }
}
