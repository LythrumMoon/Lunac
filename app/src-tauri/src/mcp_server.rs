// MCP stdio server — bridges user-defined tools to agent.exe Agent
//
// Reads tool definitions from <exe 根>\tools\*.json（便携模式数据根，见 storage.rs）
// Implements MCP stdio transport (JSON-RPC 2.0 over stdin/stdout)
// Spawned by agent.exe as a child process: --mcp-server stdio:<lunac.exe 路径>

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::process::Stdio;

// ── Tool definition types ─────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value, // JSON Schema for tool parameters
    pub handler: ToolHandler,
    /// elicitation 声明（MCP 2025-06-18，A13）：这个工具被调用时，服务端先向**用户**
    /// 弹卡收集补充输入，拿到答案并入 `arguments` 后再执行 handler。缺省 = 不弹卡。
    /// 语义与边界见下方 `maybe_elicit()`。**不是** `inputSchema` 的替代品：`inputSchema`
    /// 是给**模型**看的入参契约，`requestedSchema` 是给**用户**填的表单 —— 两者可以不同。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elicit: Option<ElicitSpec>,
}

/// 工具 JSON 里的 `elicit` 字段（A13）。`message` 显示在卡片顶部，`requestedSchema`
/// 是给用户看的表单结构（MCP 规定只支持 object，且字段为 string/number/boolean/enum）。
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ElicitSpec {
    pub message: String,
    #[serde(default, rename = "requestedSchema")]
    pub requested_schema: Value,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "type")]
pub enum ToolHandler {
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
    /// 内置工具：由 lunac 进程内直接执行（如图片图案特征分析）。
    /// 完全离线，不依赖外部命令或网络。
    #[serde(rename = "builtin")]
    Builtin { name: String },
}

// ── MCP protocol types ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct McpRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct McpResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<McpError>,
}

#[derive(Debug, Serialize)]
struct McpError {
    code: i32,
    message: String,
}

// ── Session：stdio 上的**双向**请求（A13）──────────────────────────
//
// 原来 run_stdio 是单向的（读一条请求、写一条回包）。elicitation / roots 要求服务端
// 也能**主动向客户端发请求并等回包**，所以把「一进一出」封成一个会话对象。
//
// 为什么不用线程：stdio 是单进程内的同步循环，主循环此刻正阻塞在 `handle_request`
// 里，不会同时读 stdin / 写 stdout —— 直接在同一线程里读回包即可，没有并发竞争。
struct Session<'a> {
    input: &'a mut dyn BufRead,
    output: &'a mut dyn Write,
    /// 服务端自己发出的请求 id（与客户端的 id 空间独立：双方靠「有没有 method」区分
    /// 请求与响应，不靠 id 前缀）。
    next_id: u64,
    /// 客户端在 initialize 里声明了 roots 能力？（没声明就不该发 roots/list）
    client_roots: bool,
    /// 是否已经问过 roots（问过就不重复问，即便结果为空）。
    roots_asked: bool,
    /// 首个 root（工作区）。`None` = 没拿到 / 客户端不支持。
    root: Option<PathBuf>,
}

impl Session<'_> {
    /// 向客户端发一个请求并等回包。跳过期间到达的通知 / 别的消息，只认 id 对上的响应。
    fn send_request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        });
        let line = serde_json::to_string(&msg).map_err(|e| e.to_string())?;
        writeln!(self.output, "{line}").map_err(|e| format!("写 stdout 失败: {e}"))?;
        self.output
            .flush()
            .map_err(|e| format!("flush stdout 失败: {e}"))?;

        let mut buf = String::new();
        loop {
            buf.clear();
            let n = self
                .input
                .read_line(&mut buf)
                .map_err(|e| format!("读 stdin 失败: {e}"))?;
            if n == 0 {
                return Err("客户端关闭了连接".into());
            }
            let trimmed = buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            // 客户端此刻不该反向发请求；真收到就跳过（本实现不支持）。
            if v.get("method").is_some() {
                continue;
            }
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(err) = v.get("error") {
                return Err(format!("客户端返回错误: {err}"));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// 取工作区根（懒加载一次）。客户端没声明 roots ⇒ `None`，shell 沿用继承的 cwd。
    fn workspace_root(&mut self) -> Option<PathBuf> {
        if !self.client_roots {
            return None;
        }
        if !self.roots_asked {
            self.roots_asked = true;
            self.root = match self.send_request("roots/list", json!({})) {
                Ok(res) => res
                    .get("roots")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
                    .and_then(|r| r.get("uri"))
                    .and_then(Value::as_str)
                    .and_then(file_uri_to_path),
                Err(e) => {
                    eprintln!("[mcp] roots/list 失败（沿用继承的 cwd）：{e}");
                    None
                }
            };
        }
        self.root.clone()
    }
}

/// `file:///C:/x/y` → `C:\x\y`（只处理本机绝对路径；解析不出来就 `None`）。
fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri
        .strip_prefix("file:///")
        .or_else(|| uri.strip_prefix("file://"))?;
    if rest.is_empty() {
        return None;
    }
    Some(PathBuf::from(rest))
}

/// 执行一次用户工具调用（A13）：先按 `elicit` 声明向**用户**弹卡收集输入，再执行 handler。
/// `arguments` 是模型给的原始入参；用户填的答案**并入**它（同名键以用户填的为准）。
fn run_tool_def(
    session: &mut Session,
    tool: &ToolDef,
    arguments: &Value,
) -> Result<String, String> {
    let args = maybe_elicit(session, tool, arguments)?;
    // roots（A13）：客户端声明了 roots 就把工作区根作为 shell handler 的 cwd。
    let cwd = session.workspace_root();
    execute_tool_handler(&tool.handler, &args, cwd.as_deref())
}

/// `elicit` 声明的触发点（A13）：向客户端发 `elicitation/create`，等用户填完回包，
/// 把 `content` 并进 `arguments`。没声明 elicit 的工具原样返回入参。
///
/// **不弹卡的边界**：只有用户**显式**在 `tools\*.json` 里写了 `elicit` 才会走到这里；
/// 出厂 `tools\` 为空 ⇒ 这一步在真实默认安装下永远不会触发。
fn maybe_elicit(session: &mut Session, tool: &ToolDef, arguments: &Value) -> Result<Value, String> {
    let Some(spec) = &tool.elicit else {
        return Ok(arguments.clone());
    };
    let res = session.send_request(
        "elicitation/create",
        json!({ "message": spec.message, "requestedSchema": spec.requested_schema }),
    )?;
    let action = res.get("action").and_then(Value::as_str).unwrap_or("cancel");
    if action != "accept" {
        // 用户拒绝 / 取消：作为工具错误回给模型（不是桥坏了）。
        return Err("用户拒绝了本次输入请求（工具未执行）".into());
    }
    let mut merged = if arguments.is_object() {
        arguments.clone()
    } else {
        json!({})
    };
    if let Some(content) = res.get("content").and_then(Value::as_object) {
        if let Some(obj) = merged.as_object_mut() {
            for (k, v) in content {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    Ok(merged)
}

// ── Tool loader ───────────────────────────────────────────────────

fn tools_dir() -> PathBuf {
    crate::storage::lunac_root_dir().join("tools")
}

fn load_tools() -> Vec<ToolDef> {
    let dir = tools_dir();
    let mut tools = Vec::new();

    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "json") {
                if let Ok(content) = fs::read_to_string(&path) {
                    match serde_json::from_str::<ToolDef>(&content) {
                        Ok(tool) => tools.push(tool),
                        Err(e) => {
                            eprintln!("[mcp] Failed to parse {}: {}", path.display(), e);
                        }
                    }
                }
            }
        }
    }

    tools
}

// ── Resources（A3，MCP 读侧）────────────────────────────────────────
//
// `resources/list` 把 `<exe 根>\tools\*.json` 报成 resource；`resources/read` 把其中
// 一个**原样读回来**。为什么要有读侧：模型虽然能从 `mcp__*` 的工具 schema 知道每个用户
// 工具的名字与入参，但**看不到 handler**（它到底跑哪条命令 / 打哪个 HTTP 端点），
// 而 `tools\` 通常在**工作区之外** ⇒ 内置 `Read` 会被工作区锁直接拒掉。
//
// **安全边界（硬，不得放宽）**：`uri` 是**模型**给的，而模型会被它读到的文件内容
// 提示注入 —— 所以这里只允许「`tools\` 目录内的 `.json` 文件」，其余一律拒。
// 判据是 `canonicalize()` 之后比前缀（`..` 与符号链接都会被展开），不是字符串检查。

/// 单个 resource 的读取上限。用户工具定义是小的 JSON（几 KB），给足余量即可；
/// 真正超长的工具结果由 agent 侧的 `tools::apply_budget` 统一收口（落盘 + 头尾内联）。
const MAX_RESOURCE_BYTES: u64 = 512 * 1024;

/// 把 `uri` 解析成「`root` 之内的一个 `.json` 文件」。
///
/// 接受三种写法（模型不一定照抄我们给的 URI）：
///   · `file:///C:/…/tools/foo.json`（`resources/list` 给出的原始形状）
///   · `foo.json`
///   · `foo`（后两种都在 `root` 下解析）
///
/// 拆成「只依赖 `root`、不碰全局状态」的纯函数是为了**可测** —— 边界条件（越界、
/// 非 .json、目录、空串）都要有单测钉住，而 `tools_dir()` 依赖 exe 根、测试里不能碰。
fn resolve_resource(root: &std::path::Path, uri: &str) -> Result<PathBuf, String> {
    let trimmed = uri.trim();
    if trimmed.is_empty() {
        return Err("缺少 uri 参数".into());
    }
    let candidate = if let Some(rest) = trimmed.strip_prefix("file:///") {
        PathBuf::from(rest)
    } else if trimmed.contains("://") {
        return Err(format!("只支持 file:// 资源，收到：{trimmed}"));
    } else {
        if trimmed.contains(['/', '\\']) {
            return Err("资源名不得包含路径分隔符，请传 resources/list 给出的 uri".into());
        }
        root.join(format!("{}.json", trimmed.trim_end_matches(".json")))
    };

    // 必须落在 root 之内：canonicalize 会展开 `..` 与符号链接，比前缀才靠得住。
    let real_root = root
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
    if real.extension().map_or(true, |e| e != "json") {
        return Err("只支持 .json 工具定义文件".into());
    }
    let size = fs::metadata(&real).map(|m| m.len()).unwrap_or(0);
    if size > MAX_RESOURCE_BYTES {
        return Err(format!(
            "资源过大（{size} 字节，上限 {MAX_RESOURCE_BYTES}）"
        ));
    }
    Ok(real)
}

/// `resources/read` 的 `result` 部分（规范形状 `{contents:[{uri,mimeType,text}]}`）。
fn read_resource_result(uri: &str) -> Result<Value, String> {
    let path = resolve_resource(&tools_dir(), uri)?;
    let text = fs::read_to_string(&path).map_err(|e| format!("读取失败：{e}"))?;
    Ok(json!({
        "contents": [{
            "uri": format!("file:///{}", path.display()),
            "mimeType": "application/json",
            "text": text,
        }]
    }))
}

// ── Prompts（A13，MCP 服务端侧）────────────────────────────────────
//
// `prompts/list` / `prompts/get` 把 `<exe 根>\prompts\*.md` 报给客户端，由 agent 侧转发成
// 两件条件注册的只读工具（`ListMcpPromptsTool` / `GetMcpPromptTool`）。用途：用户写一段
// 可复用的提示词模板（`{{arg}}` 占位），模型在需要时取来按参数展开 —— 与「技能」不同，
// prompt **不加载任何代码**，只是一段参数化的文本。
//
// 与 resources 的差别：resources 暴露的是「用户工具的定义文件」（给模型**看** handler），
// prompts 暴露的是「用户写好的提示词模板」（给模型**用**）。两者都只读、都限定在各自目录内。

/// prompt 正文的读取上限（模板通常很小，给足余量）。
const MAX_PROMPT_BYTES: u64 = 256 * 1024;

fn prompts_dir() -> PathBuf {
    crate::storage::lunac_root_dir().join("prompts")
}

/// 一个可用的 prompt 模板（`prompts\<name>.md`）。
struct PromptDef {
    name: String,
    description: String,
    body: String,
}

/// 拆出可选的 YAML frontmatter（`---\n…\n---`），返回 `(frontmatter, 正文)`。
/// 没有 frontmatter 时返回 `(None, 原文)`（去掉 BOM）。
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let t = text.trim_start_matches('\u{feff}');
    if let Some(rest) = t.strip_prefix("---") {
        if let Some(idx) = rest.find("\n---") {
            let fm = &rest[..idx];
            let body = rest[idx + 4..].trim_start_matches(['\r', '\n']);
            return (Some(fm), body);
        }
    }
    (None, t)
}

/// 从 frontmatter 里取一个 `key: value`（值去掉两侧引号）。
fn fm_field<'a>(fm: &'a str, key: &str) -> Option<&'a str> {
    fm.lines().find_map(|l| {
        let l = l.trim();
        l.strip_prefix(key)
            .and_then(|r| r.strip_prefix(':'))
            .map(|v| v.trim().trim_matches('"'))
            .filter(|v| !v.is_empty())
    })
}

/// prompt 的描述：优先 frontmatter 的 `description:`，否则正文里第一行非空、
/// 去掉前导 `#` 的文字。
fn prompt_description(fm: Option<&str>, body: &str) -> String {
    if let Some(d) = fm.and_then(|f| fm_field(f, "description")) {
        return d.to_string();
    }
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.trim_start_matches('#').trim().to_string())
        .unwrap_or_default()
}

/// 扫 `prompts\` 目录，按文件名排序返回（顺序稳定 ⇒ prompts/list 的排列稳定）。
fn load_prompts() -> Vec<PromptDef> {
    load_prompts_from(&prompts_dir())
}

/// 扫**指定目录**（拆出来是为了单测能指向临时目录，不碰真实 exe 根）。
fn load_prompts_from(dir: &Path) -> Vec<PromptDef> {
    let mut prompts = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return prompts;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.extension().map_or(false, |e| e == "md") {
            continue;
        }
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size > MAX_PROMPT_BYTES {
            eprintln!("[mcp] 跳过过大的 prompt（{size} 字节）：{}", path.display());
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
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
        let (fm, body) = split_frontmatter(&raw);
        prompts.push(PromptDef {
            description: prompt_description(fm, body),
            name,
            body: body.to_string(),
        });
    }
    prompts.sort_by(|a, b| a.name.cmp(&b.name));
    prompts
}

/// `prompts/list` 的 `result` 部分。
fn build_prompts_list(prompts: &[PromptDef]) -> Value {
    let items: Vec<Value> = prompts
        .iter()
        .map(|p| json!({ "name": p.name, "description": p.description }))
        .collect();
    json!({ "prompts": items, "nextCursor": null })
}

/// `prompts/get` 的 `result` 部分：`{description, messages:[{role,content}]}`。
/// `arguments` 里的键按 `{{key}}` / `{{key }}` 两种写法替换（复用 `resolve_template`）。
fn prompt_get_result(prompts: &[PromptDef], name: &str, arguments: &Value) -> Result<Value, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("缺少 name 参数：要传 prompts/list 给出的 prompt 名".into());
    }
    let Some(p) = prompts.iter().find(|p| p.name == name) else {
        return Err(format!("prompt 不存在：{name}"));
    };
    let text = resolve_template(&p.body, arguments);
    Ok(json!({
        "description": p.description,
        "messages": [{
            "role": "user",
            "content": { "type": "text", "text": text }
        }]
    }))
}


// ── 长期记忆（A4，2026-09-20）──────────────────────────────────────
//
// 落点 `<exe 根>\ModuleData\memory\MEMORY.md`（与 `history\chat.db` 同在 ModuleData 下），
// 两个自定义方法（与 history 那两个同构：**不进 `tools/list`、不吃审批卡**）：
//   · `lunac/memory_read`  —— agent 启动时注入系统提示词；复盘 fork 开跑前读一次
//   · `lunac/memory_write` —— 内置工具 `Remember` 的后端，**按条目追加**
//
// 为什么是独立的小文本、而不是往 chat.db 里再开一张表（backlog §4 要求「照 SessionSearch
// 那套设计」，指通道与冻结快照纪律，不是指存储介质）：两者**不是一类东西** ——
//   · `chat.db` 是**原始流水**（全量、按会话、FTS5 检索，靠 `SessionSearch` 现查）；
//   · `MEMORY.md` 是从流水里**提炼出来的少量结论**（用户偏好 / 项目约定 / 踩过的坑）。
//     流水里没有「哪句值得留」这个判断，提炼那一步正是 A4 复盘 fork 的职责。
// 选文本而非 DB 的现实理由：① 它要进**系统提示词的固定前缀**（启动时读一次、进程内冻结），
// 必须是一段稳定纯文本；② 用户要能直接读、直接改（Hermes 的分层同款）；③ 写入是
// 「追加 + 上限」的小文件，为它开表属过度设计。
//
// **上限是硬闸，不是提示**：这段内容每轮都随系统提示词重发，无限增长会同时吃掉上下文与
// 命中率。超限**报错**而不是静默截断 —— 静默截断会让模型以为已经记住了。

const MEMORY_FILE: &str = "MEMORY.md";
/// 单条记忆上限（一条通常只有一两句；超过这个量级说明该拆成多条或先提炼）
const MAX_MEMORY_ENTRY_CHARS: usize = 2_000;
/// 记忆文件上限 ≈ 1500 token（占 128k 预算的 1.2%）。注入侧同口径 —— 文件整段进固定前缀。
const MAX_MEMORY_CHARS: usize = 6_000;

fn memory_path() -> PathBuf {
    crate::storage::module_data_dir().join("memory").join(MEMORY_FILE)
}

/// 读记忆全文（不存在 ⇒ 空串，属全新安装的正常情况）。
///
/// 与 `resolve_resource` 同理拆出「只依赖 path」的形态：单测必须能指向临时目录，
/// 绝不能碰真实的 `<exe 根>\ModuleData`（那是用户数据）。
fn read_memory_at(path: &std::path::Path) -> Result<String, String> {
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(path).map_err(|e| format!("读取记忆失败：{e}"))
}

fn read_memory() -> Result<String, String> {
    read_memory_at(&memory_path())
}

/// 追加（或整体替换）一条记忆。
///
/// 去重按**整条条目**比对：复盘 fork 每 10 轮跑一次，很容易把同一条事实反复写进来，
/// 而重复条目既占上限又稀释提示词。
fn write_memory_at(
    path: &std::path::Path,
    content: &str,
    replace: bool,
) -> Result<String, String> {
    let text = content.trim();
    if text.is_empty() {
        return Err("缺少 content 参数：要记的内容不能为空".into());
    }
    let len = text.chars().count();
    let existing = read_memory_at(path).unwrap_or_default();

    let next = if replace {
        if len > MAX_MEMORY_CHARS {
            return Err(format!(
                "替换内容过长（{len} 字符 > 上限 {MAX_MEMORY_CHARS}）：先精简再写"
            ));
        }
        format!("{text}\n")
    } else {
        if len > MAX_MEMORY_ENTRY_CHARS {
            return Err(format!(
                "单条记忆过长（{len} 字符 > 上限 {MAX_MEMORY_ENTRY_CHARS}）：请拆成多条，或先提炼成结论"
            ));
        }
        // 多行条目缩进成一条 bullet，保证「一行 = 一条」这个形状不被破坏
        let bullet = format!("- {}", text.replace('\n', "\n  "));
        if existing.contains(&bullet) {
            return Ok("(already remembered — nothing changed)".into());
        }
        let mut head = existing.clone();
        if !head.is_empty() && !head.ends_with('\n') {
            head.push('\n');
        }
        let merged = format!("{head}{bullet}\n");
        if merged.chars().count() > MAX_MEMORY_CHARS {
            return Err(format!(
                "长期记忆已满（{} 字符上限）：先用 `Remember` 的 `replace` 模式整理合并，再追加新条目",
                MAX_MEMORY_CHARS
            ));
        }
        merged
    };

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("建记忆目录失败 {e}"))?;
    }
    fs::write(path, next.as_bytes()).map_err(|e| format!("写入记忆失败：{e}"))?;
    Ok(format!(
        "{} 记忆已保存（{} 字符）。",
        if replace { "整体替换：" } else { "追加一条：" },
        next.chars().count()
    ))
}

fn write_memory_entry(content: &str, replace: bool) -> Result<String, String> {
    write_memory_at(&memory_path(), content, replace)
}

/// 分派长期记忆的自定义方法（形状与 `handle_history_method` 一致：错误走
/// `result + isError`，因为这是「这次没写成」，不是「方法不存在」）。
fn handle_memory_method(method: &str, params: Option<&Value>) -> Result<String, String> {
    match method {
        "lunac/memory_read" => {
            let text = read_memory()?;
            if text.trim().is_empty() {
                Ok("(long-term memory is empty)".into())
            } else {
                Ok(text)
            }
        }
        "lunac/memory_write" => {
            let content = params
                .and_then(|p| p.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let replace = params
                .and_then(|p| p.get("replace"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            write_memory_entry(content, replace)
        }
        other => Err(format!("未知记忆方法: {other}")),
    }
}

// ── Tool executor ─────────────────────────────────────────────────

fn resolve_template(template: &str, params: &Value) -> String {
    let mut result = template.to_string();
    if let Some(obj) = params.as_object() {
        for (key, val) in obj {
            let placeholder = format!("{{{{{} }}}}", key);
            // Try brace-less variant too (some tools use {{key}})
            let placeholder2 = format!("{{{{{}}}}}", key);
            let val_str = match val {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            result = result.replace(&placeholder, &val_str);
            result = result.replace(&placeholder2, &val_str);
        }
    }
    result
}

/// Run a shell command with a hard timeout so a hung process can never block
/// the agent turn forever. stdout/stderr are drained on background threads
/// (piped buffers would otherwise deadlock), and on timeout the child is
/// killed and an error is returned.
fn run_shell_with_timeout(
    command: &str,
    timeout_secs: u64,
    cwd: Option<&std::path::Path>,
) -> Result<String, String> {
    // 必须 `raw_arg` 原样拼命令行：`command` 来自用户的工具定义（`tools\*.json`），
    // 里面带引号 + 空格是常态（如 `python "C:\my tools\x.py" --arg "a b"`），
    // 而 `Command::arg` 会按 MSVC 规则把 `"` 转义成 `\"` —— cmd 不认这个转义，
    // 命令会整条失败（实测见 main.rs `kill_port`：cmd 报「此时不应有 \"tokens=5\"」）。
    #[cfg(target_os = "windows")]
    let mut c = {
        use std::os::windows::process::CommandExt;
        let mut c = StdCommand::new("cmd");
        c.raw_arg("/c").raw_arg(command);
        // CREATE_NO_WINDOW — GUI 进程（release）下不设置会弹出 cmd 窗口
        c.creation_flags(0x0800_0000);
        c
    };
    #[cfg(not(target_os = "windows"))]
    let mut c = {
        let mut c = StdCommand::new("sh");
        c.args(["-c", command]);
        c
    };
    // roots（A13）：客户端声明了 roots 时，用工作区根作为命令的工作目录；
    // 客户端不支持 roots ⇒ 保持进程继承的 cwd（与原行为一致）。
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    c.stdout(Stdio::piped());
    c.stderr(Stdio::piped());

    let mut child = c.spawn().map_err(|e| format!("Failed to execute: {}", e))?;

    let out = child.stdout.take().map(|o| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = String::new();
            let mut r = o;
            let _ = r.read_to_string(&mut buf);
            buf
        })
    });
    let err = child.stderr.take().map(|e| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = String::new();
            let mut r = e;
            let _ = r.read_to_string(&mut buf);
            buf
        })
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "命令执行超时（{}s），已终止。耗时任务请改为后台运行。",
                    timeout_secs
                ));
            }
        }
    };

    let stdout = out.map(|h| h.join().unwrap_or_default()).unwrap_or_default();
    let stderr = err.map(|h| h.join().unwrap_or_default()).unwrap_or_default();

    if exit_status.success() {
        // Return stdout if non-empty, otherwise stderr for informational output
        if !stdout.trim().is_empty() {
            Ok(stdout)
        } else if !stderr.trim().is_empty() {
            Ok(stderr)
        } else {
            Ok("(no output)".into())
        }
    } else {
        Err(format!(
            "Command failed (exit {}): {}",
            exit_status,
            stderr.trim()
        ))
    }
}

fn execute_tool_handler(
    handler: &ToolHandler,
    params: &Value,
    cwd: Option<&std::path::Path>,
) -> Result<String, String> {
    match handler {
        ToolHandler::Builtin { name } => run_builtin_tool(name, params),
        ToolHandler::Shell { command } => {
            let resolved = resolve_template(command, params);
            run_shell_with_timeout(&resolved, 60, cwd)
        }
        ToolHandler::Http {
            method,
            url,
            headers,
            body,
        } => {
            let resolved_url = resolve_template(url, params);
            let resolved_body = body.as_ref().map(|b| resolve_template(b, params));

            let client = reqwest::blocking::Client::new();
            let mut req = match method.to_uppercase().as_str() {
                "GET" => client.get(&resolved_url),
                "POST" => {
                    let mut r = client.post(&resolved_url);
                    if let Some(ref b) = resolved_body {
                        r = r.body(b.clone());
                    }
                    r
                }
                "PUT" => {
                    let mut r = client.put(&resolved_url);
                    if let Some(ref b) = resolved_body {
                        r = r.body(b.clone());
                    }
                    r
                }
                "DELETE" => client.delete(&resolved_url),
                other => return Err(format!("Unsupported HTTP method: {}", other)),
            };

            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }

            let resp = req.send().map_err(|e| format!("HTTP request failed: {}", e))?;
            let status = resp.status();
            let body_text = resp
                .text()
                .map_err(|e| format!("Failed to read response: {}", e))?;

            if status.is_success() {
                Ok(body_text)
            } else {
                Err(format!("HTTP {}: {}", status.as_u16(), body_text))
            }
        }
    }
}

// ── Builtin tools（进程内执行，完全离线）───────────────────────────

/// 分派内置工具调用。当前提供「本地图片图案特征分析」。
fn run_builtin_tool(name: &str, params: &Value) -> Result<String, String> {
    match name {
        "image_pattern_analysis" => {
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| "缺少 path 参数：需传入图片的绝对路径（jpg/png）".to_string())?;
            analyze_image_pattern(path)
        }
        other => Err(format!("未知内置工具: {}", other)),
    }
}

/// RGB(0..1) → HSV(h:0..360, s:0..1, v:0..1)
fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

fn color_name(r: f32, g: f32, b: f32) -> &'static str {
    let (h, s, v) = rgb_to_hsv(r, g, b);
    if v < 0.15 {
        return "黑";
    }
    if s < 0.15 {
        return if v > 0.9 { "白" } else if v > 0.6 { "浅灰" } else { "深灰" };
    }
    const NAMES: [&str; 12] = [
        "红", "橙", "黄", "黄绿", "绿", "青绿", "青", "蓝", "蓝紫", "紫", "品红", "粉",
    ];
    NAMES[((h / 30.0) as usize).min(11)]
}

fn dominant_hue_desc(hue_hist: &[usize; 12], n: f32) -> String {
    const NAMES: [&str; 12] = [
        "红调", "橙调", "黄调", "黄绿调", "绿调", "青绿调", "青调", "蓝调", "蓝紫调", "紫调",
        "品红调", "粉调",
    ];
    let total: usize = hue_hist.iter().sum();
    if total == 0 {
        return "无明显彩色倾向（近灰阶）".into();
    }
    let mut best = 0usize;
    for i in 1..12 {
        if hue_hist[i] > hue_hist[best] {
            best = i;
        }
    }
    format!(
        "{}（彩色占比 {:.0}%）",
        NAMES[best],
        total as f32 / n * 100.0
    )
}

fn edge_summary(hist: &[usize; 4]) -> String {
    const NAMES: [&str; 4] = ["水平", "右斜", "垂直", "左斜"];
    let total: usize = hist.iter().sum();
    if total == 0 {
        return "无明显边缘".into();
    }
    let mut order: Vec<usize> = (0..4).collect();
    order.sort_by(|a, b| hist[*b].cmp(&hist[*a]));
    format!("{}为主，其次{}", NAMES[order[0]], NAMES[order[1]])
}

fn composition_notes(avg_lum: f32, symmetry: f32, edge_ratio: f32) -> String {
    let mut notes = Vec::new();
    if symmetry > 0.9 {
        notes.push("正面/对称构图，适合圣像式、庄重主题");
    }
    if edge_ratio > 0.3 {
        notes.push("边缘密集，笔触/纹理丰富，适合做肌理感背景");
    }
    if avg_lum > 0.7 {
        notes.push("整体高亮，主体宜靠近左上光源或居中加光环");
    }
    if avg_lum < 0.35 {
        notes.push("暗调为主，建议局部高光点缀（暗底浮金效果）");
    }
    if notes.is_empty() {
        notes.push("常规构图，可局部提亮或强化对比以增强表现力");
    }
    notes.join("；")
}

/// 本地图片图案特征分析：解码图片 → 缩略采样 → 统计亮度/色相/边缘/
/// 高光/纹理/对称度，输出结构化中文报告（纯像素统计，不做语义理解）。
fn analyze_image_pattern(path: &str) -> Result<String, String> {
    let img = image::open(path).map_err(|e| format!("无法解码图片 {}（{}）", path, e))?;
    let (orig_w, orig_h) = (img.width(), img.height());
    // 缩略到 160 宽以内，控制计算量
    let thumb = img.thumbnail(160, 160);
    let rgb = thumb.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    if w == 0 || h == 0 {
        return Err("图片尺寸无效".into());
    }

    let px = |x: usize, y: usize| -> (f32, f32, f32) {
        let p = rgb.get_pixel(x as u32, y as u32);
        (
            p[0] as f32 / 255.0,
            p[1] as f32 / 255.0,
            p[2] as f32 / 255.0,
        )
    };
    let lum = |x: usize, y: usize| -> f32 {
        let (r, g, b) = px(x, y);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    };

    let n = (w * h) as f32;

    // 1. 亮度：整体均值 + 4×4 网格 + 高光点（>0.85）
    const GRID: usize = 4;
    let (gw, gh) = (w / GRID, h / GRID);
    let cells_px = (gw.max(1) * gh.max(1)) as f32;
    let mut grid_lum = [[0f32; GRID]; GRID];
    let mut total_lum = 0f32;
    let mut high_light = 0usize;
    for y in 0..h {
        for x in 0..w {
            let l = lum(x, y);
            total_lum += l;
            if l > 0.85 {
                high_light += 1;
            }
            if gw > 0 && gh > 0 {
                grid_lum[(y / gh).min(GRID - 1)][(x / gw).min(GRID - 1)] += l;
            }
        }
    }
    let avg_lum = total_lum / n;
    let grid_str: Vec<String> = grid_lum
        .iter()
        .map(|row| {
            row.iter()
                .map(|v| format!("{:>3.0}", (v / cells_px) * 100.0))
                .collect::<Vec<_>>()
                .join("  ")
        })
        .collect();

    // 2. 色相分布（12 桶，S>0.15 计入）+ 饱和度均值 + 主色 top5
    let mut hue_hist = [0usize; 12];
    let mut sat_sum = 0f32;
    let mut color_buckets: HashMap<u16, usize> = HashMap::new();
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = px(x, y);
            let (hue, s, _v) = rgb_to_hsv(r, g, b);
            sat_sum += s;
            if s > 0.15 {
                hue_hist[((hue / 30.0) as usize).min(11)] += 1;
            }
            let key = (((r * 4.0) as u16) & 0xF)
                | (((g * 4.0) as u16) & 0xF) << 4
                | (((b * 4.0) as u16) & 0xF) << 8;
            *color_buckets.entry(key).or_insert(0) += 1;
        }
    }
    let avg_sat = sat_sum / n;
    let mut ranked: Vec<(u16, usize)> = color_buckets.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    let main_colors: Vec<String> = ranked
        .iter()
        .take(5)
        .map(|(k, c)| {
            let r = ((k & 0xF) * 17) as f32 / 255.0;
            let g = (((k >> 4) & 0xF) * 17) as f32 / 255.0;
            let b = (((k >> 8) & 0xF) * 17) as f32 / 255.0;
            format!("{} {:.0}%", color_name(r, g, b), (*c as f32 / n) * 100.0)
        })
        .collect();

    // 3. 边缘方向直方图（Sobel，|角度| 分 4 桶）+ 纹理密度（3×3 局部标准差）
    let mut edge_hist = [0usize; 4]; // 水平 / 右斜 / 垂直 / 左斜
    let mut edge_count = 0usize;
    let mut texture_sum = 0f32;
    let mut tex_cnt = 0usize;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let mut vals = [0f32; 9];
            let mut mean = 0f32;
            for dy in 0..3 {
                for dx in 0..3 {
                    let v = lum(x + dx - 1, y + dy - 1);
                    vals[dy * 3 + dx] = v;
                    mean += v;
                }
            }
            mean /= 9.0;
            let mut var = 0f32;
            for v in &vals {
                var += (v - mean) * (v - mean);
            }
            texture_sum += (var / 9.0).sqrt();
            tex_cnt += 1;

            let gx = (lum(x + 1, y - 1) + 2.0 * lum(x + 1, y) + lum(x + 1, y + 1))
                - (lum(x - 1, y - 1) + 2.0 * lum(x - 1, y) + lum(x - 1, y + 1));
            let gy = (lum(x - 1, y + 1) + 2.0 * lum(x, y + 1) + lum(x + 1, y + 1))
                - (lum(x - 1, y - 1) + 2.0 * lum(x, y - 1) + lum(x + 1, y - 1));
            if gx * gx + gy * gy > 0.003 {
                edge_count += 1;
                let ang = gy.atan2(gx).to_degrees().abs();
                let dir = if ang <= 22.5 {
                    0
                } else if ang <= 67.5 {
                    1
                } else if ang <= 112.5 {
                    2
                } else if ang <= 157.5 {
                    3
                } else {
                    0
                };
                edge_hist[dir] += 1;
            }
        }
    }
    let edge_ratio = edge_count as f32 / tex_cnt.max(1) as f32;
    let texture = texture_sum / tex_cnt.max(1) as f32;

    // 4. 水平对称度（左右镜像亮度差）
    let mut sym_diff = 0f32;
    let mut sym_cnt = 0usize;
    for y in 0..h {
        for x in 0..w / 2 {
            sym_diff += (lum(x, y) - lum(w - 1 - x, y)).abs();
            sym_cnt += 1;
        }
    }
    let symmetry = 1.0 - (sym_diff / sym_cnt.max(1) as f32);

    let bright_desc = match avg_lum {
        x if x > 0.75 => "高亮",
        x if x > 0.5 => "明亮",
        x if x > 0.3 => "中等",
        _ => "暗调",
    };
    let texture_desc = if texture > 0.22 {
        "高纹理（颗粒/笔触感强）"
    } else if texture > 0.12 {
        "中纹理（细腻）"
    } else {
        "低纹理（平滑）"
    };
    let symmetry_desc = if symmetry > 0.9 {
        "高度对称"
    } else if symmetry > 0.78 {
        "近似对称"
    } else {
        "非对称"
    };

    // Rust 的 format! 不支持 `%` 格式类型，百分比统一手动乘 100 后格式化
    let edge_pct = format!("{:.0}", edge_ratio * 100.0);
    let high_pct = format!("{:.1}", high_light as f32 / n * 100.0);
    let sym_pct = format!("{:.0}", symmetry * 100.0);

    Ok(format!(
        "图片尺寸: {orig_w}×{orig_h}px\n\
         整体基调: {bright_desc}，{hue_desc}\n\
         主色调: {main_colors}\n\
         平均饱和度: {avg_sat:.2}（0 灰阶 ~ 1 纯彩）\n\
         亮度分布（4×4 网格，左上→右下，0-100）:\n\
         \x20 {g0}\n\
         \x20 {g1}\n\
         \x20 {g2}\n\
         \x20 {g3}\n\
         笔触/边缘方向: {edge_desc}（边缘占比 {edge_pct}%）\n\
         纹理: {texture_desc}\n\
         高光点占比: {high_pct}%（细密高光点≈点彩/星芒闪光）\n\
         对称性: {symmetry_desc}（{sym_pct}%）\n\
         构图建议: {comp_notes}",
        orig_w = orig_w,
        orig_h = orig_h,
        bright_desc = bright_desc,
        hue_desc = dominant_hue_desc(&hue_hist, n),
        main_colors = main_colors.join("、"),
        avg_sat = avg_sat,
        g0 = grid_str[0],
        g1 = grid_str[1],
        g2 = grid_str[2],
        g3 = grid_str[3],
        edge_desc = edge_summary(&edge_hist),
        edge_pct = edge_pct,
        texture_desc = texture_desc,
        high_pct = high_pct,
        symmetry_desc = symmetry_desc,
        comp_notes = composition_notes(avg_lum, symmetry, edge_ratio),
    ))
}

// ── MCP protocol handler ──────────────────────────────────────────

fn build_tools_list(tools: &[ToolDef]) -> Value {
    let items: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.input_schema,
            })
        })
        .collect();
    json!({ "tools": items, "nextCursor": null })
}

// ── 往期会话检索（自定义方法，2026-09-19，A2）──────────────────────
//
// 这两个方法**不进 `tools/list`**，因此不会出现在用户的工具列表里、也不经过审批卡。
//
// 为什么不做成 `<exe 根>\tools\*.json` 里的用户工具：
//   ① 那是**用户**的工具目录 —— 内置能力混进去，用户能在 Tool Editor 里改坏或删掉；
//   ② MCP 工具的调用在 core-agent 里**一律要弹审批卡**（`needs_approval` 对 `mcp__*`
//      恒真），而「读自己本地的会话历史」与 `Read` / `Grep` 同级，不该每问一句就打扰一次。
// 所以走自定义方法，由 agent 侧注册成一等公民工具 `SessionSearch`（免审批、串行）。

/// `history_search` 单次返回的消息条数默认值与上限（上限防模型一次要太多把上下文吃掉）。
const HISTORY_SEARCH_DEFAULT_LIMIT: usize = 10;
const HISTORY_SEARCH_MAX_LIMIT: usize = 30;
/// 注入系统提示词的「往期会话索引」：最多列多少条会话、总字符预算。
///
/// 预算刻意压得小（≈500 token）：这段属于**每轮都发**的固定前缀。虽然它会被前缀缓存
/// 覆盖，但冷启动那一轮要实打实付这笔钱，而且它会挤占其它内容的可见度。
const HISTORY_INDEX_SESSIONS: usize = 20;
const HISTORY_INDEX_BUDGET_CHARS: usize = 2000;

/// 现在（毫秒）。与前端 `Date.now()` 同口径 —— 会话的 `created_at` 就是这么存的。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 相对天数标签（与 `chat_db::render_digest` 内部同口径，检索结果里也要用）。
fn rel_age(created_at: u64, now: u64) -> String {
    match now.saturating_sub(created_at) / 86_400_000 {
        0 => "today".into(),
        1 => "1d ago".into(),
        n => format!("{n}d ago"),
    }
}

/// 把检索命中渲染成给模型看的文本：**按会话分组**，方便它一眼看出「这些是同一段对话」。
fn render_hits(query: &str, hits: &[crate::chat_db::HistoryHit], now: u64) -> String {
    if hits.is_empty() {
        return format!(
            "No past message matched \"{query}\".\n\
             (Searched this app's own chat history only — not files on disk.)"
        );
    }
    let mut out = format!(
        "{} matching message(s) for \"{query}\" in past sessions (newest first).\n\
         Note: messages are truncated; re-run with a narrower query if you need more.",
        hits.len()
    );
    let mut cur = String::new();
    for h in hits {
        if h.session_id != cur {
            cur = h.session_id.clone();
            let title = if h.title.trim().is_empty() {
                "(untitled)"
            } else {
                h.title.trim()
            };
            out.push_str(&format!(
                "\n\n## {}  [{}]\n",
                title,
                rel_age(h.created_at, now)
            ));
        }
        out.push_str(&format!("- {} #{}: {}\n", h.role, h.idx, h.content.trim()));
    }
    out
}

/// 分派往期会话检索的自定义方法。
fn handle_history_method(method: &str, params: Option<&Value>) -> Result<String, String> {
    let db = crate::storage::chat_db_path();
    match method {
        "lunac/history_search" => {
            let query = params
                .and_then(|p| p.get("query"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if query.is_empty() {
                return Err("缺少 query 参数：需要传入要在往期会话里检索的关键词".into());
            }
            let limit = params
                .and_then(|p| p.get("limit"))
                .and_then(Value::as_u64)
                .unwrap_or(HISTORY_SEARCH_DEFAULT_LIMIT as u64)
                .clamp(1, HISTORY_SEARCH_MAX_LIMIT as u64) as usize;
            let hits = crate::chat_db::search(&db, &query, limit)?;
            Ok(render_hits(&query, &hits, now_ms()))
        }
        "lunac/history_index" => {
            // 库还不存在（全新安装）⇒ 空库、什么都不注入，属正常情况。
            let briefs = crate::chat_db::recent_sessions(&db, HISTORY_INDEX_SESSIONS)?;
            Ok(crate::chat_db::render_digest(
                &briefs,
                HISTORY_INDEX_BUDGET_CHARS,
                now_ms(),
            ))
        }
        other => Err(format!("未知历史方法: {other}")),
    }
}

/// 自定义方法的成功回包（形状与 `tools/call` 一致）。
fn text_ok(id: Option<Value>, text: String) -> McpResponse {
    McpResponse {
        jsonrpc: "2.0".into(),
        id,
        result: Some(json!({
            "content": [{ "type": "text", "text": text }],
            "isError": false
        })),
        error: None,
    }
}

/// 自定义方法的失败回包：**用 `result + isError`，不用 JSON-RPC error**。
/// 语义区别 —— 这是「这次没做成」（库打不开 / 参数缺失 / 记忆已满），不是「方法不存在」；
/// 走 error 会让 agent 侧把它当成桥坏了。
fn text_err(id: Option<Value>, msg: String) -> McpResponse {
    McpResponse {
        jsonrpc: "2.0".into(),
        id,
        result: Some(json!({
            "content": [{ "type": "text", "text": msg }],
            "isError": true
        })),
        error: None,
    }
}

fn handle_request(req: &McpRequest, tools: &[ToolDef], session: &mut Session) -> McpResponse {
    match req.method.as_deref() {
        Some("initialize") => {
            // 记下客户端是否支持 roots —— 没声明就不该反向发 roots/list（MCP 规范）。
            session.client_roots = req
                .params
                .as_ref()
                .and_then(|p| p.get("capabilities"))
                .and_then(|c| c.get("roots"))
                .is_some();
            McpResponse {
                jsonrpc: "2.0".into(),
                id: req.id.clone(),
                result: Some(json!({
                    // A13：升到 2025-06-18 —— elicitation 是该版本引入的能力。roots/prompts
                    // 两版都支持，但统一到一个版本号最省事（客户端按此值协商）。
                    "protocolVersion": "2025-06-18",
                    "capabilities": {
                        "tools": {},
                        "prompts": {},
                        "resources": {},
                        // elicitation 由服务端在 `tools/call` 时按工具 JSON 的 `elicit` 字段发起。
                        "elicitation": {}
                    },
                    "serverInfo": {
                        "name": "lunac-mcp",
                        "version": "0.1.0"
                    }
                })),
                error: None,
            }
        }
        Some("tools/list") => McpResponse {
            jsonrpc: "2.0".into(),
            id: req.id.clone(),
            result: Some(build_tools_list(tools)),
            error: None,
        },
        Some("tools/call") => {
            let params = req.params.as_ref().and_then(|v| v.get("name")).and_then(|n| n.as_str()).map(|n| n.to_string());
            let args = req.params.as_ref().and_then(|v| v.get("arguments")).cloned().unwrap_or(Value::Null);

            match params {
                Some(name) => {
                    match tools.iter().find(|t| t.name == name) {
                        Some(tool) => match run_tool_def(session, tool, &args) {
                            Ok(text) => McpResponse {
                                jsonrpc: "2.0".into(),
                                id: req.id.clone(),
                                result: Some(json!({
                                    "content": [{ "type": "text", "text": text }],
                                    "isError": false
                                })),
                                error: None,
                            },
                            Err(msg) => McpResponse {
                                jsonrpc: "2.0".into(),
                                id: req.id.clone(),
                                result: Some(json!({
                                    "content": [{ "type": "text", "text": msg }],
                                    "isError": true
                                })),
                                error: None,
                            },
                        },
                        None => McpResponse {
                            jsonrpc: "2.0".into(),
                            id: req.id.clone(),
                            error: Some(McpError {
                                code: -32602,
                                message: format!("Tool not found: {}", name),
                            }),
                            result: None,
                        },
                    }
                }
                None => McpResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id.clone(),
                    error: Some(McpError {
                        code: -32602,
                        message: "Missing tool name".into(),
                    }),
                    result: None,
                },
            }
        }
        Some("resources/list") => {
            let dir = tools_dir();
            let mut resources = Vec::new();
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(false, |e| e == "json") {
                        resources.push(json!({
                            "uri": format!("file:///{}", path.display()),
                            "name": path.file_stem().unwrap_or_default().to_string_lossy(),
                            "mimeType": "application/json"
                        }));
                    }
                }
            }
            McpResponse {
                jsonrpc: "2.0".into(),
                id: req.id.clone(),
                result: Some(json!({ "resources": resources })),
                error: None,
            }
        }
        // 读侧（A3）：`uri` 来自**模型**，因此边界检查在 `read_resource_result` 里
        // （只允许 tools 目录内的 .json）。错误走 JSON-RPC error（这是标准方法，
        // 不是工具调用 —— 工具调用才用 `result + isError`）。
        Some("resources/read") => {
            let uri = req
                .params
                .as_ref()
                .and_then(|p| p.get("uri"))
                .and_then(Value::as_str)
                .unwrap_or("");
            match read_resource_result(uri) {
                Ok(result) => McpResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id.clone(),
                    result: Some(result),
                    error: None,
                },
                Err(message) => McpResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(McpError {
                        code: -32002,
                        message,
                    }),
                },
            }
        }
        // Prompts（A13）：标准方法，与 resources 一样 —— 失败走 JSON-RPC error（工具调用
        // 才用 `result + isError`）。`prompts/list` 恒成功（目录不存在 = 空列表）。
        Some("prompts/list") => McpResponse {
            jsonrpc: "2.0".into(),
            id: req.id.clone(),
            result: Some(build_prompts_list(&load_prompts())),
            error: None,
        },
        Some("prompts/get") => {
            let name = req
                .params
                .as_ref()
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let args = req
                .params
                .as_ref()
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or(Value::Null);
            match prompt_get_result(&load_prompts(), name, &args) {
                Ok(result) => McpResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id.clone(),
                    result: Some(result),
                    error: None,
                },
                Err(message) => McpResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(McpError {
                        code: -32002,
                        message,
                    }),
                },
            }
        }
        // 往期会话检索：**自定义方法，不列进 tools/list**（见 handle_history_method 的注释）。
        Some(m) if m.starts_with("lunac/history_") => {
            let method = m.to_string();
            match handle_history_method(&method, req.params.as_ref()) {
                Ok(text) => text_ok(req.id.clone(), text),
                Err(msg) => text_err(req.id.clone(), msg),
            }
        }
        // 长期记忆（A4）：同上，**自定义方法，不列进 tools/list**（见 handle_memory_method）。
        Some(m) if m.starts_with("lunac/memory_") => {
            let method = m.to_string();
            match handle_memory_method(&method, req.params.as_ref()) {
                Ok(text) => text_ok(req.id.clone(), text),
                Err(msg) => text_err(req.id.clone(), msg),
            }
        }
        _ => {
            // Notifications (no id) should be silently ignored per MCP spec
            if req.id.is_some() {
                McpResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id.clone(),
                    error: Some(McpError {
                        code: -32601,
                        message: format!("Method not found: {:?}", req.method),
                    }),
                    result: None,
                }
            } else {
                // Notification — don't respond
                McpResponse {
                    jsonrpc: "2.0".into(),
                    id: None,
                    result: None,
                    error: None,
                }
            }
        }
    }
}

// ── stdio transport entry point ────────────────────────────────────

pub fn run_stdio() {
    let tools = load_tools();
    eprintln!("[mcp] Loaded {} user-defined tools from {:?}", tools.len(), tools_dir());

    let stdin = io::stdin();
    let stdout = io::stdout();
    // 会话化 IO（A13）：`handle_request` 里的 elicitation / roots 要能**反向**读写，
    // 所以把两个锁交给 `Session` 持有（借用期覆盖整个循环）。
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    let mut session = Session {
        input: &mut input,
        output: &mut output,
        next_id: 0,
        client_roots: false,
        roots_asked: false,
        root: None,
    };

    let mut line = String::new();
    loop {
        line.clear();
        match session.input.read_line(&mut line) {
            Ok(0) => break, // EOF：客户端关闭 stdin → 退出
            Ok(_) => {}
            Err(e) => {
                eprintln!("[mcp] Stdin error: {}", e);
                continue;
            }
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let req: McpRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[mcp] Parse error: {} for input: {:.200}", e, trimmed);
                continue;
            }
        };

        let resp = handle_request(&req, &tools, &mut session);

        // Only respond to requests that have an id (notifications are one-way)
        if resp.id.is_some() || resp.error.is_some() {
            let resp_json = serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into());
            if let Err(e) = writeln!(session.output, "{}", resp_json) {
                eprintln!("[mcp] Write error: {}", e);
                return; // client closed stdin → exit
            }
            if let Err(e) = session.output.flush() {
                eprintln!("[mcp] Flush error: {}", e);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `resources/read` 的**安全边界**（A3，2026-09-20）。`uri` 是模型给的，而模型会被
    /// 读到的文件内容提示注入 ⇒ 「只能读 tools 目录内的 .json」这条必须由单测钉死，
    /// 否则一次改动就可能把任意文件读取能力悄悄放出去。
    ///
    /// 用临时目录而不是 `tools_dir()`：后者指向真实 exe 根，测试不许碰用户数据。
    #[test]
    fn resources_read_is_confined_to_the_tools_dir() {
        let root = std::env::temp_dir().join(format!(
            "lunac-mcp-res-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let outside = root.with_extension("outside");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("deploy.json"), "{\"name\":\"deploy\"}").unwrap();
        fs::write(root.join("readme.md"), "not a tool").unwrap();
        // 目录外的诱饵：越界读取如果成功，就是这条测试要抓的回归
        fs::write(&outside, "secret").unwrap();

        // ① 三种合法写法都要能解析到同一个文件
        for uri in [
            format!("file:///{}", root.join("deploy.json").display()),
            "deploy.json".to_string(),
            "deploy".to_string(),
        ] {
            let got = resolve_resource(&root, &uri).unwrap_or_else(|e| panic!("{uri} 应可解析: {e}"));
            assert_eq!(got, root.join("deploy.json").canonicalize().unwrap());
        }

        // ② 越界 / 非 .json / 目录 / 空串 / 别的 scheme 一律拒
        for bad in [
            format!("file:///{}", outside.display()),
            format!("file:///{}/../{}", root.display(), outside.file_name().unwrap().to_string_lossy()),
            "readme.md".to_string(),
            "readme".to_string(),
            String::new(),
            "https://example.com/x.json".to_string(),
            "..\\deploy".to_string(),
            "sub/deploy.json".to_string(),
        ] {
            assert!(
                resolve_resource(&root, &bad).is_err(),
                "必须拒绝：{bad:?}"
            );
        }

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&outside);
    }

    /// 长期记忆的**追加 / 去重 / 上限**三条硬行为（A4，2026-09-20）。
    /// 用临时目录而不是 `memory_path()`：后者指向真实 ModuleData，测试不许碰用户数据。
    #[test]
    fn memory_appends_dedupes_and_enforces_the_cap() {
        let dir = std::env::temp_dir().join(format!(
            "lunac-mcp-mem-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let file = dir.join("MEMORY.md");

        // ① 文件不存在 ⇒ 读到空串（全新安装的正常情况，不是错误）
        assert_eq!(read_memory_at(&file).unwrap(), "");

        // ② 追加两条 ⇒ 逐条成行，先写的在前
        assert!(write_memory_at(&file, "用户偏好中文", false).is_ok());
        assert!(write_memory_at(&file, "项目用 Rust", false).is_ok());
        let text = read_memory_at(&file).unwrap();
        assert_eq!(text, "- 用户偏好中文\n- 项目用 Rust\n");

        // ③ 同一条重写 ⇒ 不改文件（去重）
        let before = text.clone();
        assert!(write_memory_at(&file, "用户偏好中文", false).unwrap().contains("already"));
        assert_eq!(read_memory_at(&file).unwrap(), before, "重复条目不得写进去");

        // ④ 空条目 / 超长单条 / 超长替换 ⇒ 拒绝（**不许静默截断**）
        assert!(write_memory_at(&file, "   ", false).is_err());
        assert!(write_memory_at(&file, &"x".repeat(MAX_MEMORY_ENTRY_CHARS + 1), false).is_err());
        assert!(write_memory_at(&file, &"x".repeat(MAX_MEMORY_CHARS + 1), true).is_err());

        // ⑤ 写满再追加 ⇒ 报错（而不是悄悄丢掉旧条目）
        // 每条都不同 —— 相同的条目会被去重挡掉（③），那样这条测试就变成测去重了。
        let filler = "y".repeat(400);
        let mut wrote = 0;
        loop {
            let entry = format!("{filler}{wrote}");
            if write_memory_at(&file, &entry, false).is_err() {
                break;
            }
            wrote += 1;
            if wrote > 50 {
                panic!("上限没有生效：一直写得进去");
            }
        }
        assert!(read_memory_at(&file).unwrap().chars().count() <= MAX_MEMORY_CHARS);
        assert!(wrote > 0, "上限之前应当还能写进去几条");

        // ⑥ replace 模式：整体替换，旧内容不再保留
        assert!(write_memory_at(&file, "- 只剩这一条", true).is_ok());
        assert_eq!(read_memory_at(&file).unwrap(), "- 只剩这一条\n");

        let _ = fs::remove_dir_all(&dir);
    }

    /// prompts 读侧（A13）的硬行为：按文件名排序、YAML frontmatter 描述、`{{arg}}` 模板替换、
    /// 非 `.md` 不入列、空 name / 找不到的名字都报错。
    ///
    /// 用 `load_prompts_from(临时目录)` 而不是 `load_prompts()`：后者指向真实 exe 根，
    /// 测试不许碰用户数据。
    #[test]
    fn prompts_are_sorted_described_and_templated() {
        let dir = std::env::temp_dir().join(format!(
            "lunac-mcp-prompts-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        // b 有 frontmatter（描述取 `description:`）；a 没有（描述取正文首个非空行）
        fs::write(
            dir.join("b.md"),
            "---\ndescription: 部署检查清单\n---\n请检查 {{target}} 的部署。\n",
        )
        .unwrap();
        fs::write(dir.join("a.md"), "# 代码评审\n评审 {{lang}} 代码。\n").unwrap();
        fs::write(dir.join("note.txt"), "忽略我").unwrap(); // 非 .md 不入列

        let list = load_prompts_from(&dir);
        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"], "必须按文件名排序（prompts/list 的排列要稳定）");
        assert_eq!(list[0].description, "代码评审", "无 frontmatter ⇒ 取正文首个非空行（去 #）");
        assert_eq!(list[1].description, "部署检查清单", "有 frontmatter ⇒ 取 description:");

        let listed = build_prompts_list(&list);
        assert_eq!(
            listed.pointer("/prompts/0/name").and_then(Value::as_str),
            Some("a")
        );
        assert_eq!(listed.pointer("/prompts/1/description").and_then(Value::as_str), Some("部署检查清单"));

        let got = prompt_get_result(&list, "b", &json!({ "target": "prod" })).unwrap();
        assert_eq!(
            got.pointer("/messages/0/content/text").and_then(Value::as_str),
            Some("请检查 prod 的部署。\n"),
            "{{target}} 要按 arguments 替换"
        );
        assert_eq!(got.get("description").and_then(Value::as_str), Some("部署检查清单"));

        assert!(prompt_get_result(&list, "  ", &json!({})).is_err(), "空 name 要报错");
        assert!(prompt_get_result(&list, "missing", &json!({})).is_err(), "找不到的名字要报错");

        let _ = fs::remove_dir_all(&dir);
    }

    /// elicitation（A13）：服务端主动发 `elicitation/create`，把用户回包的 `content`
    /// **并入**原入参（同名键以用户填的为准）；用户 decline 则整个工具调用报错。
    #[test]
    fn elicitation_merges_content_and_aborts_on_decline() {
        let tool = ToolDef {
            name: "deploy".into(),
            description: String::new(),
            input_schema: json!({}),
            handler: ToolHandler::Builtin { name: "noop".into() },
            elicit: Some(ElicitSpec {
                message: "需要部署目标".into(),
                requested_schema: json!({
                    "type": "object",
                    "properties": { "target": { "type": "string" } }
                }),
            }),
        };

        // ① accept + content ⇒ 并入原入参（同名键覆盖、其余保留）
        let reply = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"action\":\"accept\",\
                     \"content\":{\"target\":\"prod\",\"extra\":1}}}\n";
        let mut input = std::io::Cursor::new(reply.as_bytes().to_vec());
        let mut output: Vec<u8> = Vec::new();
        let mut session = Session {
            input: &mut input,
            output: &mut output,
            next_id: 0,
            client_roots: false,
            roots_asked: false,
            root: None,
        };
        let merged = maybe_elicit(&mut session, &tool, &json!({ "target": "old", "keep": true }))
            .expect("accept 应当成功");
        assert_eq!(merged.get("target").and_then(Value::as_str), Some("prod"), "用户填的覆盖原参数");
        assert_eq!(merged.get("keep").and_then(Value::as_bool), Some(true), "没冲突的原参数保留");
        assert_eq!(merged.get("extra").and_then(Value::as_u64), Some(1), "用户新填的键并进来");
        let sent = String::from_utf8(output).unwrap();
        assert!(sent.contains("\"method\":\"elicitation/create\""), "实际发出：{sent}");
        assert!(sent.contains("需要部署目标"), "message 要原样带给客户端");

        // ② decline ⇒ Err（工具不执行）
        let reply = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"action\":\"decline\"}}\n";
        let mut input = std::io::Cursor::new(reply.as_bytes().to_vec());
        let mut output: Vec<u8> = Vec::new();
        let mut session = Session {
            input: &mut input,
            output: &mut output,
            next_id: 0,
            client_roots: false,
            roots_asked: false,
            root: None,
        };
        assert!(maybe_elicit(&mut session, &tool, &json!({})).is_err(), "用户拒绝 ⇒ 工具不执行");

        // ③ 没声明 elicit 的工具：原样返回，且**不发任何请求**
        let plain = ToolDef { elicit: None, ..tool };
        let mut input = std::io::Cursor::new(Vec::new());
        let mut output: Vec<u8> = Vec::new();
        let mut session = Session {
            input: &mut input,
            output: &mut output,
            next_id: 0,
            client_roots: false,
            roots_asked: false,
            root: None,
        };
        let args = json!({ "a": 1 });
        assert_eq!(maybe_elicit(&mut session, &plain, &args).unwrap(), args);
        assert!(output.is_empty(), "没声明 elicit 就一个字节都不该发");
    }

    /// roots（A13）：客户端**声明了** roots 才发 `roots/list`，取首个 root 的 `uri`
    /// 还原成本机路径；只问一次（拿到后缓存）；没声明就一个字节都不发。
    #[test]
    fn roots_are_asked_only_when_declared_and_first_uri_wins() {
        // ① 没声明 roots：不问、不发请求
        let mut input = std::io::Cursor::new(Vec::new());
        let mut output: Vec<u8> = Vec::new();
        let mut session = Session {
            input: &mut input,
            output: &mut output,
            next_id: 0,
            client_roots: false,
            roots_asked: false,
            root: None,
        };
        assert_eq!(session.workspace_root(), None);
        assert!(output.is_empty(), "客户端没声明 roots 就不该发 roots/list");

        // ② 声明了 + 回包给两个 root ⇒ 取第一个；再问一次不再发请求（缓存）
        let reply = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"roots\":[\
                     {\"uri\":\"file:///D:/work/first\"},{\"uri\":\"file:///D:/second\"}]}}\n";
        let mut input = std::io::Cursor::new(reply.as_bytes().to_vec());
        let mut output: Vec<u8> = Vec::new();
        let mut session = Session {
            input: &mut input,
            output: &mut output,
            next_id: 0,
            client_roots: true,
            roots_asked: false,
            root: None,
        };
        assert_eq!(
            session.workspace_root(),
            Some(PathBuf::from("D:/work/first")),
            "取首个 root"
        );
        // 再问一次：不再发请求（缓存），返回值不变
        assert_eq!(session.workspace_root(), Some(PathBuf::from("D:/work/first")));
        // 到这里 `session` 不再使用 ⇒ 对 `output` 的借用结束，可以读回它
        let sent = String::from_utf8(output).unwrap();
        assert!(sent.contains("\"method\":\"roots/list\""), "实际发出：{sent}");
        assert_eq!(
            sent.matches("\"method\":\"roots/list\"").count(),
            1,
            "roots 只问一次，拿到后缓存：{sent}"
        );

        // ③ `file:///` 解析边界
        assert_eq!(file_uri_to_path("file:///C:/x"), Some(PathBuf::from("C:/x")));
        assert_eq!(file_uri_to_path("file:///"), None, "空路径不是有效 root");
        assert_eq!(file_uri_to_path("https://example.com"), None, "非 file: 一律拒");
    }
}

