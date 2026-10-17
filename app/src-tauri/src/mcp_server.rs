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


// ── 长期记忆（A4，2026-09-20；作用域化 2026-10-13）──────────────────
//
// 落点（与 `history\chat.db` 同在 ModuleData 下）：
//   · 全局 = `<exe 根>\ModuleData\memory\MEMORY.md`（主 AI 的长期记忆）
//   · 桌宠 = `<exe 根>\ModuleData\memory\pet\MEMORY.md`（桌宠私有成长笔记）
// 两个自定义方法（与 history 那两个同构：**不进 `tools/list`、不吃审批卡**），
// 入参带 `scope`（`global` / `pet`，缺省 global）决定读 / 写哪一份：
//   · `lunac/memory_read`  —— agent 启动 / 按轮注入记忆；复盘 fork 开跑前读一次
//   · `lunac/memory_write` —— 内置工具 `Remember` 的后端，**按条目追加**
// 两份各一个文件 ⇒ 桌宠轮里写的东西**不会**反向注入主 AI 对话（作用域隔离）。
//
// 为什么是独立的小文本、而不是往 chat.db 里再开一张表（backlog §4 要求「照 SessionSearch
// 那套设计」，指通道与冻结快照纪律，不是指存储介质）：两者**不是一类东西** ——
//   · `chat.db` 是**原始流水**（全量、按会话、FTS5 检索，靠 `SessionSearch` 现查）；
//   · `MEMORY.md` 是从流水里**提炼出来的少量结论**（用户偏好 / 项目约定 / 踩过的坑）。
//     流水里没有「哪句值得留」这个判断，提炼那一步正是 A4 复盘 fork 的职责。
// 选文本而非 DB 的现实理由：① 它要作为一段**稳定纯文本**注入（agent 侧注在 history 开头，
// 见 `core-agent` 的 `context_block_message`）；② 用户要能直接读、直接改（Hermes 的分层同款）；
// ③ 写入是「追加 + 上限」的小文件，为它开表属过度设计。
//
// **上限是硬闸，不是提示**：这段内容每轮都随上下文重发，无限增长会同时吃掉上下文与
// 命中率。超限**报错**而不是静默截断 —— 静默截断会让模型以为已经记住了。

const MEMORY_FILE: &str = "MEMORY.md";
/// 单条记忆上限（一条通常只有一两句；超过这个量级说明该拆成多条或先提炼）
const MAX_MEMORY_ENTRY_CHARS: usize = 2_000;
/// **正文额度**：只统计每条去掉行首 `- ` 与 `[YYYY-MM-DD · scope] ` 前缀后的正文
/// （含续行）。⇒ 前缀再长也不挤占正文档位，文件总长随条目数 / 前缀长度浮动。
/// 约 1500 token（占 128k 预算的 1.2%），与注入侧口径一致（整段注入，见 `core-agent`
/// 的 `context_block_message`）。
const MEMORY_BODY_BUDGET_CHARS: usize = 6_000;
/// **总长度硬顶**（含前缀 / 换行 / 续行缩进）：该文件每轮都进上下文，需防「条目多 +
/// 前缀长」把注入体积顶爆。按 50 条 × 24 字符前缀 + 6000 正文字额 ≈ 7200，取 9000
/// 留约 25% 余量。
const MEMORY_TOTAL_HARD_CAP_CHARS: usize = 9_000;

/// 记忆作用域（2026-10-13）：主 AI 的**全局**长期记忆 vs **桌宠私有**的成长笔记。
///
/// 两份**各一个文件**（见 [`memory_path_for`]）、各走一套读写 ⇒ 「桌宠轮里写的东西不会
/// 反向影响主 AI 对话」（作用域隔离）；也各有各的注入路径（见 `core-agent` 的按轮注入）。
/// `scope` 是 agent 桥方法 `lunac/memory_*` 的入参；缺省 = `Global`（向后兼容无 scope 的旧调用）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemoryScope {
    Global,
    Pet,
}

impl MemoryScope {
    /// 解析桥方法传来的 `scope` 字符串：缺省 / 空 = 全局；`pet` = 桌宠私有；
    /// 其余值**报错**（不静默当全局 —— 那会把桌宠的东西写进主 AI 记忆）。
    pub fn parse(raw: Option<&str>) -> Result<Self, String> {
        match raw.map(str::trim) {
            None | Some("") | Some("global") => Ok(MemoryScope::Global),
            Some("pet") => Ok(MemoryScope::Pet),
            Some(other) => Err(format!("未知记忆作用域: {other}")),
        }
    }

    /// 落盘标注用的稳定英文 id（**不随界面语言漂移**）。
    fn id(self) -> &'static str {
        match self {
            MemoryScope::Global => "global",
            MemoryScope::Pet => "pet",
        }
    }
}

/// 桌宠私有目录 = `ModuleData\memory\pet\`（桌宠记忆与成长笔记 / 人格建议的**共同父目录**）。
fn pet_scope_dir() -> PathBuf {
    crate::storage::module_data_dir().join("memory").join("pet")
}

/// 某一作用域的记忆文件路径。
/// 全局 = `ModuleData\memory\MEMORY.md`（沿用旧落点）；桌宠 = `ModuleData\memory\pet\MEMORY.md`。
fn memory_path_for(scope: MemoryScope) -> PathBuf {
    match scope {
        MemoryScope::Global => crate::storage::module_data_dir().join("memory").join(MEMORY_FILE),
        MemoryScope::Pet => pet_scope_dir().join(MEMORY_FILE),
    }
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

fn read_memory(scope: MemoryScope) -> Result<String, String> {
    read_memory_at(&memory_path_for(scope))
}

/// 行首来源前缀（B1，2026-10-17）的严格形状：`[YYYY-MM-DD · global|pet] `。
///
/// 只认宿主自己生成的那一种（日期 = 10 位 ISO、scope = 两个稳定英文 id）；不匹配
/// （老的无前缀条目 / 正文恰好以 `[` 开头）原样返回。返回 `(前缀含方括号, 前缀后的正文)`。
fn split_provenance_prefix(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix('[')?;
    let (inner, after) = rest.split_once("] ")?;
    let (date, scope) = inner.split_once(" · ")?;
    let date_ok = date.len() == 10
        && date.as_bytes().iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            _ => b.is_ascii_digit(),
        });
    if date_ok && matches!(scope, "global" | "pet") {
        Some((&text[..inner.len() + 2], after))
    } else {
        None
    }
}

/// 去掉行首来源前缀；没有前缀（旧数据 / 非宿主生成）⇒ 原样返回。
fn strip_provenance_prefix(text: &str) -> &str {
    split_provenance_prefix(text).map(|(_, body)| body).unwrap_or(text)
}

/// 本次写入要盖的来源前缀（不含行首 `- `，也不含其后的空格）。
fn provenance_prefix(scope: MemoryScope, date: &str) -> String {
    format!("[{date} · {}]", scope.id())
}

/// 条目归一（**去重与逐条删共用同一个判据**）：先去掉行首的 `[日期 · scope]` 前缀，
/// 再折叠所有空白、去首尾、统一小写。
///
/// 去前缀是 B1（2026-10-17）的**去重语义变化**：同一条事实不同日期写入时必须视为同一条
/// （否则日期一换就重复堆积）；日期只作「最后确认时间」，不参与正文比较。
fn normalize_entry(text: &str) -> String {
    strip_provenance_prefix(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// 把一条原始条目（从 `- ` 起头、可能含续行）的首行前缀换成本次写入的前缀。
///
/// 旧的无前缀条目也走这里就地补前缀（**只迁移被再次写入的那条**，不做全量回填 ——
/// 历史条目没有可信的旧日期）。续行（含缩进与 CRLF）原样保留。
fn retag_entry_first_line(raw: &str, prefix: &str) -> String {
    let (first, rest) = match raw.split_once('\n') {
        Some((f, r)) => (f, Some(r)),
        None => (raw, None),
    };
    let (first, cr) = match first.strip_suffix('\r') {
        Some(f) => (f, "\r"),
        None => (first, ""),
    };
    let body = first.strip_prefix("- ").unwrap_or(first);
    let body = strip_provenance_prefix(body);
    let mut out = format!("- {prefix} {body}");
    out.push_str(cr);
    if let Some(rest) = rest {
        out.push('\n');
        out.push_str(rest);
    }
    out
}

/// 给整段记忆文本里的每条 bullet 重盖行首前缀（replace 模式用）。
///
/// **日期一律以宿主为准**：模型若在内容里自带 `[日期 · scope]`（哪怕看起来合法）也先剥掉，
/// 再由宿主盖上本次日期 —— 不允许模型生成 / 伪造日期。已有前缀的条目被整体重写，等价于
/// 「本次重写即一次确认」；非 bullet 行（含续行缩进）原样保留。
fn tag_bullet_lines(text: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let (line, cr) = match line.strip_suffix('\r') {
            Some(l) => (l, "\r"),
            None => (line, ""),
        };
        if line == "-" {
            out.push_str("- ");
            out.push_str(prefix);
        } else if let Some(body) = line.strip_prefix("- ") {
            out.push_str("- ");
            out.push_str(prefix);
            let body = strip_provenance_prefix(body);
            if !body.is_empty() {
                out.push(' ');
                out.push_str(body);
            }
        } else {
            out.push_str(line);
        }
        out.push_str(cr);
    }
    out
}

/// 宿主本地日期（`YYYY-MM-DD`）。
///
/// 日期必须由宿主取：模型生成的日期不可信；UTC 在东八区会跨日错一天（项目不引 chrono，
/// 见 `log.rs` 顶注），这里直接调 Win32 `GetLocalTime`（`windows` 已是既有依赖）。
fn local_today() -> String {
    let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay)
}

/// 把记忆正文按行切成 `(行首字节偏移, 行内容)`（不丢字节：偏移用于按内容删除时精确定位）。
/// 行尾 `\r` 一并剥掉，兼容 CRLF。
fn line_offsets(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    for i in 0..=bytes.len() {
        if i == bytes.len() || bytes[i] == b'\n' {
            let raw = &text[start..i];
            out.push((start, raw.strip_suffix('\r').unwrap_or(raw)));
            start = i + 1;
        }
    }
    out
}

/// 把记忆正文拆成条目：`(起始字节, 结束字节(不含), 条目正文)`。
/// 条目 = 一个 `- ` 起头的 bullet 及其后续**续行**（写入侧用两空格缩进续行）。
/// 列出（给前端）与按内容逐条删**必须共用这一套切分**，否则会出现「列出来却删不掉」。
fn split_entries(text: &str) -> Vec<(usize, usize, String)> {
    let mut out: Vec<(usize, usize, String)> = Vec::new();
    let mut cur: Option<(usize, String)> = None;
    for (offset, line) in line_offsets(text) {
        if line.starts_with("- ") || line == "-" {
            if let Some((s, body)) = cur.take() {
                out.push((s, offset, body));
            }
            cur = Some((offset, line.strip_prefix("- ").unwrap_or("").to_string()));
        } else if let Some((_, body)) = cur.as_mut() {
            let cont = line.trim();
            if !cont.is_empty() {
                body.push('\n');
                body.push_str(cont);
            }
        }
        // 既不是 bullet、又没有当前条目 ⇒ 前导空行 / 噪声，跳过（写入侧不会产生别的形状）。
    }
    if let Some((s, body)) = cur {
        out.push((s, text.len(), body));
    }
    out
}

/// 统计记忆文本的**正文字数**（动态上限口径）。
///
/// 每个 bullet 去掉行首 `- ` 与 `[YYYY-MM-DD · scope] ` 前缀后计正文；续行 / 非 bullet
/// 行按 trim 后的内容计。换行与续行缩进不计 ⇒ 前缀不占正文额度（前缀再多也不挤占 6000）。
fn memory_body_chars(text: &str) -> usize {
    let mut total = 0;
    for (_, line) in line_offsets(text) {
        let body = if line == "-" {
            ""
        } else {
            line.strip_prefix("- ").unwrap_or(line)
        };
        let body = strip_provenance_prefix(body).trim();
        if !body.is_empty() {
            total += body.chars().count();
        }
    }
    total
}

/// 落盘前的两口径校验（调用方先做单条上限）：**正文额度 → 总长度硬顶**，顺序固定。
///
/// 两个拒绝语义必须能区分（便于提示与排查）：正文超出报「正文额度已满」，
/// 总量超出报「总长度硬顶」。输入按**最终落盘内容**传入（已脱敏、含前缀与结尾换行）。
fn check_memory_budgets(text: &str) -> Result<(), String> {
    let body = memory_body_chars(text);
    if body > MEMORY_BODY_BUDGET_CHARS {
        return Err(format!(
            "长期记忆正文额度已满（{body} 字符 > 正文额度 {MEMORY_BODY_BUDGET_CHARS}）：先用 Remember 的 replace 模式整理合并，再追加新条目"
        ));
    }
    let total = text.chars().count();
    if total > MEMORY_TOTAL_HARD_CAP_CHARS {
        return Err(format!(
            "长期记忆总长度硬顶（{total} 字符 > 硬顶 {MEMORY_TOTAL_HARD_CAP_CHARS}）：条目或前缀过多，请用 replace 模式精简"
        ));
    }
    Ok(())
}

/// 列出某作用域的全部记忆条目（正文，不含 `- ` 与 `[日期 · scope]` 前缀）；文件不存在 ⇒ 空列表。
///
/// 列表是给用户看的（桌宠设置窗），来源前缀属于注入 / 落盘元数据，不在这里回显；
/// 逐条删按内容匹配时两边都会经 `normalize_entry` 去前缀，故「列出来的那条一定删得掉」。
fn list_memory_entries_at(path: &std::path::Path) -> Result<Vec<String>, String> {
    Ok(split_entries(&read_memory_at(path)?)
        .into_iter()
        .map(|(_, _, body)| strip_provenance_prefix(&body).to_string())
        .collect())
}

/// 按内容删掉**一条**记忆（归一后比对，与去重同判据）：删 A 不动 B。
/// 命中后连同它的续行整块摘掉，其余条目原样保留（不改动格式）。找不到 ⇒ `Err`（不谎报成功）。
fn delete_memory_entry_at(path: &std::path::Path, needle: &str) -> Result<String, String> {
    let target = normalize_entry(needle);
    if target.is_empty() {
        return Err("缺少要删除的条目内容".into());
    }
    let text = read_memory_at(path)?;
    let Some((start, end)) = split_entries(&text)
        .into_iter()
        .find(|(_, _, body)| normalize_entry(body) == target)
        .map(|(s, e, _)| (s, e))
    else {
        return Err("没找到这条记忆（可能已被删除或已改写）".into());
    };
    let mut next = String::with_capacity(text.len());
    next.push_str(&text[..start]);
    next.push_str(&text[end..]);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("建记忆目录失败 {e}"))?;
    }
    fs::write(path, next.as_bytes()).map_err(|e| format!("写入记忆失败：{e}"))?;
    Ok(format!("已删除一条记忆（剩 {} 条）", split_entries(&next).len()))
}

/// 写入层的来源标注（B1，2026-10-17）：`None` = 不加前缀（成长笔记 / 人格建议复用同一落盘助手）。
///
/// 日期由宿主取（`local_today()` 的本地日期），scope 用稳定英文 id —— 两者都不交给模型。
#[derive(Clone, Copy)]
struct MemoryProvenance<'a> {
    scope: MemoryScope,
    date: &'a str,
}

/// 追加（或整体替换）一条记忆。
///
/// **逐条来源标注（B1）**：带 `provenance` 时给首行加 `- [YYYY-MM-DD · scope] ` 前缀；
/// 日期 = 本次写入的**最后确认时间**（不是初次得知时间），多行续行缩进原样保留。
///
/// 去重按**去掉来源前缀后的归一正文**比对（折叠空白 + 大小写不敏感）：复盘 fork 很容易把
/// 同一条事实隔天再写一次，若把日期算进比较就会重复堆积。命中时**不追加**，只把该条的
/// 日期刷新为本次写入日期；旧的无前缀条目也在这里**原地补前缀**（只迁移被再次写入的那条，
/// 不批量回填 —— 历史条目没有可信的旧日期）。归一判据与 [`delete_memory_entry_at`] 共用，
/// 保证「列出来的那条一定删得掉」。
fn write_memory_at(
    path: &std::path::Path,
    content: &str,
    replace: bool,
    provenance: Option<MemoryProvenance<'_>>,
) -> Result<String, String> {
    // 凭据兜底（2026-10-11，与 A17 / M2-8 同类）：复盘 fork 可能把对话里出现的 API key /
    // 私钥原样写进来，而 MEMORY.md 会被注入以后每一次对话 ⇒ 落盘前统一过 mask_secrets。
    // 复用既有的 mask_secrets（log.rs，返回 String、纯字符串变换）：它只把明显的凭据值
    // 打码成 *** / sk-***，其余字符（含换行）逐个保留 ⇒ 既不会弄坏多行 bullet 的续行缩进，
    // 也不新增任何写失败路径（脱敏本身没有 Result，最坏只是内容里多两处码，记忆照样写下去）。
    // 脱敏后再走原有的 trim / 长度闸 / 去重 / 落盘，判据与口径不变。
    let masked = crate::log::mask_secrets(content);
    let text = masked.trim();
    if text.is_empty() {
        return Err("缺少 content 参数：要记的内容不能为空".into());
    }
    let prefix = provenance.map(|p| provenance_prefix(p.scope, p.date));
    let existing = read_memory_at(path).unwrap_or_default();

    let next = if replace {
        // 整体重写：所有 bullet 的日期 / scope 由宿主重盖（模型自带的假日期一律覆盖）。
        let body = match &prefix {
            Some(p) => tag_bullet_lines(text, p),
            None => text.to_string(),
        };
        // 按**最终落盘内容**（含结尾换行）校验两口径：正文额度 → 总长度硬顶。
        let next = format!("{body}\n");
        check_memory_budgets(&next)?;
        next
    } else {
        // 防止模型自带前缀 ⇒ 先剥掉再统一由宿主盖（日期永远以宿主为准）。
        // 无来源标注的调用方（成长笔记 / 人格建议）保持原样，不碰正文。
        let body = match &prefix {
            Some(_) => strip_provenance_prefix(text),
            None => text,
        };
        let tagged = match &prefix {
            Some(p) => format!("{p} {body}"),
            None => body.to_string(),
        };
        // 多行条目缩进成一条 bullet，保证「一行 = 一条」这个形状不被破坏；
        // 前缀只加在首行 `- ` 之后，续行缩进保持原样。
        let bullet = format!("- {}", tagged.replace('\n', "\n  "));
        // 单条上限按**最终落盘内容**（含 `- ` 与来源前缀）校验。
        let len = bullet.chars().count();
        if len > MAX_MEMORY_ENTRY_CHARS {
            return Err(format!(
                "单条记忆过长（{len} 字符 > 上限 {MAX_MEMORY_ENTRY_CHARS}）：请拆成多条，或先提炼成结论"
            ));
        }
        // 归一后比对（去来源前缀 + 折叠空白 + 大小写不敏感）：与 delete_memory_entry_at 同判据。
        let n = normalize_entry(body);
        if let Some((start, end, _)) = split_entries(&existing)
            .into_iter()
            .find(|(_, _, body)| normalize_entry(body) == n)
        {
            // 命中同一条 ⇒ 不追加；把日期刷新为本次写入日期（旧无前缀条目则就地补前缀）。
            let refreshed = match &prefix {
                Some(p) => retag_entry_first_line(&existing[start..end], p),
                None => existing[start..end].to_string(),
            };
            let updated = if refreshed == existing[start..end] {
                existing.clone()
            } else {
                let mut u = String::with_capacity(existing.len());
                u.push_str(&existing[..start]);
                u.push_str(&refreshed);
                u.push_str(&existing[end..]);
                u
            };
            check_memory_budgets(&updated)?;
            if updated != existing {
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir).map_err(|e| format!("建记忆目录失败 {e}"))?;
                }
                fs::write(path, updated.as_bytes()).map_err(|e| format!("写入记忆失败：{e}"))?;
            }
            return Ok(if prefix.is_some() {
                "(already remembered — date refreshed)"
            } else {
                "(already remembered — nothing changed)"
            }
            .into());
        }
        let mut head = existing.clone();
        if !head.is_empty() && !head.ends_with('\n') {
            head.push('\n');
        }
        let merged = format!("{head}{bullet}\n");
        check_memory_budgets(&merged)?;
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

fn write_memory_entry(scope: MemoryScope, content: &str, replace: bool) -> Result<String, String> {
    let date = local_today();
    write_memory_at(
        &memory_path_for(scope),
        content,
        replace,
        Some(MemoryProvenance { scope, date: &date }),
    )
}

/// 把记忆文件截断为空（**文件不存在 ⇒ 幂等空操作**）。
/// 拆出「只依赖 path」的形态，与 `read_memory_at` / `write_memory_at` 同理 ——
/// 单测必须能指向临时目录，绝不碰真实的 `<exe 根>\ModuleData`（那是用户数据）。
fn clear_memory_at(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    fs::write(path, b"").map_err(|e| format!("清空记忆失败：{e}"))
}

/// 前端「查看长期记忆」（斜杠命令 `/memory`）的只读入口：`read_memory` 的 pub 包装
/// （文件不存在 ⇒ 空串）。给宿主 `#[tauri::command]` 调用。
pub fn memory_read_text(scope: MemoryScope) -> Result<String, String> {
    read_memory(scope)
}

/// 前端「清空长期记忆 / 桌宠成长笔记」的入口：把该作用域的记忆文件截断为空。
/// 破坏性操作，调用方（前端）负责二次确认；这里只做确定性落盘，不新增提示路径。
pub fn clear_memory_text(scope: MemoryScope) -> Result<(), String> {
    clear_memory_at(&memory_path_for(scope))
}

/// 列出某作用域的全部记忆条目（**桌宠设置窗「成长笔记」列表的数据源**）。
pub fn memory_entries(scope: MemoryScope) -> Result<Vec<String>, String> {
    list_memory_entries_at(&memory_path_for(scope))
}

/// 按内容删掉**一条**记忆（**桌宠设置窗「逐条删」的数据源**）：删 A 不动 B。
pub fn memory_delete_entry(scope: MemoryScope, needle: &str) -> Result<String, String> {
    delete_memory_entry_at(&memory_path_for(scope), needle)
}

/// 某作用域记忆的设置窗载荷：条目列表 + 两条上限。上限**以宿主常量为唯一真相源**
/// （前端只回显，不另立一份，见 `docs/ui-style.md` §界面 3 的 `maxChars` 纪律）。
/// `maxTotalChars` 回显的是**正文档位**（前缀不计入），与写侧同一口径。
pub fn memory_scope_payload(scope: MemoryScope) -> Result<Value, String> {
    Ok(serde_json::json!({
        "entries": memory_entries(scope)?,
        "maxEntryChars": MAX_MEMORY_ENTRY_CHARS,
        "maxTotalChars": MEMORY_BODY_BUDGET_CHARS,
    }))
}

/// 从桥方法参数解析作用域（缺省 = 全局；无法识别 ⇒ 报错，绝不静默落到全局）。
fn memory_scope_from(params: Option<&Value>) -> Result<MemoryScope, String> {
    MemoryScope::parse(params.and_then(|p| p.get("scope")).and_then(Value::as_str))
}

/// 分派长期记忆的自定义方法（形状与 `handle_history_method` 一致：错误走
/// `result + isError`，因为这是「这次没写成」，不是「方法不存在」）。
/// `scope` 决定读 / 写哪一份（全局 / 桌宠私有，见 [`MemoryScope`]）。
fn handle_memory_method(method: &str, params: Option<&Value>) -> Result<String, String> {
    match method {
        "lunac/memory_read" => {
            let text = read_memory(memory_scope_from(params)?)?;
            if text.trim().is_empty() {
                Ok("(long-term memory is empty)".into())
            } else {
                Ok(text)
            }
        }
        "lunac/memory_write" => {
            let scope = memory_scope_from(params)?;
            let content = params
                .and_then(|p| p.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let replace = params
                .and_then(|p| p.get("replace"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            write_memory_entry(scope, content, replace)
        }
        other => Err(format!("未知记忆方法: {other}")),
    }
}

// ── 桌宠成长笔记 / 人格修订建议（2026-10-13，人格地基）────────────────
//
// 两款**宿主固定路径**的追加式小文件（AI 不参与拼路径，**不进 `config\`** —— 避 §7 工作锁、
// 也让 `config\pet-persona.md` 保持「用户资产」的纯粹）：
//   · 成长笔记 = `ModuleData\memory\pet\GROWTH.md`（与桌宠私有记忆 `MEMORY.md` **同目录、另立文件**）
//   · 人格修订建议 = `ModuleData\memory\pet\PERSONA_SUGGESTIONS.md`
//
// 与长期记忆同款纪律（**复用同一批助手，不另造第二套**）：
//   · **只追加**（AI 侧唯一写入形态）；逐条删 / 清空只经宿主命令（用户 UI 下轮接）；
//   · 上限**复用既有记忆口径**（`MAX_MEMORY_ENTRY_CHARS` / `MEMORY_BODY_BUDGET_CHARS` /
//     `MEMORY_TOTAL_HARD_CAP_CHARS` —— §62 不得新增第三份常量），超限**报错不静默截断**；
//   · 落盘前过 `mask_secrets`（体现在复用的 `write_memory_at` 里）；
//   · 去重 / 逐条删共用 `normalize_entry` 归一判据（列出来的按内容删得掉）；
//   · 格式 = 与记忆一致的扁平 bullet（`- ` 起头），故 `split_entries` / `delete_memory_entry_at`
//     直接可用。
//
// ⚠️ **AI 无任何改核心人格的路径**：桥只给两条**只追加**方法（`lunac/pet_growth_write` /
//    `lunac/persona_suggestion_write`）；把建议**合并进 `pet-persona.md`** 只能走宿主命令
//    `merge_persona_suggestion`（复用 `storage` 那套人格校验：长度 / NUL / 先校验后写），AI 侧到不了。

const PET_GROWTH_FILE: &str = "GROWTH.md";
const PERSONA_SUGGESTIONS_FILE: &str = "PERSONA_SUGGESTIONS.md";

/// 成长笔记文件路径（与桌宠私有记忆同目录、**另立文件**，不混进 `MEMORY.md`）。
fn pet_growth_path() -> PathBuf {
    pet_scope_dir().join(PET_GROWTH_FILE)
}

/// 人格修订建议文件路径（宿主固定路径，**不进 `config\`**）。
fn persona_suggestions_path() -> PathBuf {
    pet_scope_dir().join(PERSONA_SUGGESTIONS_FILE)
}

// — 成长笔记（桌宠私有，只追加）—

/// 读成长笔记全文（不存在 ⇒ 空串）；`lunac/pet_growth_read` 与注入侧共用。
pub fn pet_growth_read_text() -> Result<String, String> {
    read_memory_at(&pet_growth_path())
}

/// 列出成长笔记全部条目（桌宠设置窗「成长笔记」列表的数据源）。
pub fn pet_growth_entries() -> Result<Vec<String>, String> {
    list_memory_entries_at(&pet_growth_path())
}

/// 按内容删掉**一条**成长笔记（删 A 不动 B）。
pub fn pet_growth_delete_entry(needle: &str) -> Result<String, String> {
    delete_memory_entry_at(&pet_growth_path(), needle)
}

/// 清空全部成长笔记（截断为空；文件不存在 ⇒ 幂等空操作）。
pub fn pet_growth_clear() -> Result<(), String> {
    clear_memory_at(&pet_growth_path())
}

/// **追加**一条成长笔记（无 replace：AI 侧只能追加，改删历史只由用户逐条做）。
pub fn pet_growth_append(content: &str) -> Result<String, String> {
    // 成长笔记不是用户事实记忆：不盖 `[日期 · scope]` 前缀（格式保持原样）。
    write_memory_at(&pet_growth_path(), content, false, None)
}

/// 成长笔记的设置窗载荷：条目列表 + 两条上限（上限以宿主常量为唯一真相源）。
pub fn pet_growth_payload() -> Result<Value, String> {
    Ok(serde_json::json!({
        "entries": pet_growth_entries()?,
        "maxEntryChars": MAX_MEMORY_ENTRY_CHARS,
        "maxTotalChars": MEMORY_BODY_BUDGET_CHARS,
    }))
}

// — 人格修订建议（只追加；合并走宿主命令、复用 `set_pet_persona` 的校验）—

/// 列出全部人格修订建议。
pub fn persona_suggestions_list() -> Result<Vec<String>, String> {
    list_memory_entries_at(&persona_suggestions_path())
}

/// 删除一条人格修订建议。
pub fn persona_suggestion_delete(needle: &str) -> Result<String, String> {
    delete_memory_entry_at(&persona_suggestions_path(), needle)
}

/// **追加**一条人格修订建议（AI 侧唯一写入形态；不接受任何「直接改人格」的语义）。
pub fn persona_suggestion_append(content: &str) -> Result<String, String> {
    // 人格建议同样是待办清单，不是用户事实记忆：不盖来源前缀。
    write_memory_at(&persona_suggestions_path(), content, false, None)
}

/// 清空全部人格修订建议（把 `PERSONA_SUGGESTIONS.md` 截断为空；文件不存在 ⇒ 幂等空操作）。
/// **只动建议区** —— 核心人格（`config\pet-persona.md`）与另两张列表（`MEMORY.md` / `GROWTH.md`）
/// 都不碰。破坏性操作，调用方（前端）负责二次确认；复用 `clear_memory_at` 与固定路径真相源
/// （`persona_suggestions_path()`），**不新增第二份常量 / 第二处路径拼接**。
pub fn persona_suggestions_clear() -> Result<(), String> {
    clear_memory_at(&persona_suggestions_path())
}

/// 建议区的设置窗载荷：条目列表 + 上限。
pub fn persona_suggestions_payload() -> Result<Value, String> {
    Ok(serde_json::json!({
        "entries": persona_suggestions_list()?,
        "maxEntryChars": MAX_MEMORY_ENTRY_CHARS,
        "maxTotalChars": MEMORY_BODY_BUDGET_CHARS,
    }))
}

/// 把一条**已存在于建议区**的人格修订建议合并进人格文件（用户「接受」后的动作，UI 下轮做）。
///
/// 合并 = 在现有人格后追加该建议（一段）。**校验复用 `storage::save_persona_text_at`**
/// —— 那正是 `set_persona` / `set_pet_persona` 的同一实现（长度 `MAX_PERSONA_CHARS` / NUL /
/// 先校验后写）。顺序：先确认建议在册 → 先校验后写人格 → **写成了才**把该条建议从建议区摘掉
/// （校验不过时人格与建议区**都不动**）。
///
/// 参数显式给两个路径：单测必须能指向临时目录，绝不碰真实 `ModuleData` / `config\`。
fn merge_persona_suggestion_at(
    suggestion: &str,
    persona_path: &std::path::Path,
    suggestions_path: &std::path::Path,
) -> Result<(), String> {
    let trimmed = suggestion.trim();
    if trimmed.is_empty() {
        return Err("要合并的建议内容为空".into());
    }
    // 只能合并**建议区里真有**的那一条：不允许凭空塞一段进人格。
    let target = normalize_entry(trimmed);
    if !list_memory_entries_at(suggestions_path)?
        .iter()
        .any(|e| normalize_entry(e) == target)
    {
        return Err("没找到这条人格建议（可能已被删除或改写）".into());
    }
    let current = read_memory_at(persona_path)?;
    let merged = if current.trim().is_empty() {
        trimmed.to_string()
    } else {
        format!("{}\n\n{}", current.trim_end(), trimmed)
    };
    // 先校验后写（同一套判据）：合并后超上限 / 含 NUL 一律拒绝。
    crate::storage::save_persona_text_at(persona_path, &merged)?;
    // 写成了才摘掉建议（建议区是待办清单，不是账本；摘不掉不回滚人格）。
    let _ = delete_memory_entry_at(suggestions_path, trimmed);
    Ok(())
}

/// 真实路径版本（宿主命令 `merge_persona_suggestion` 用）。
pub fn merge_persona_suggestion(suggestion: &str) -> Result<(), String> {
    merge_persona_suggestion_at(
        suggestion,
        &crate::storage::pet_persona_config_path(),
        &persona_suggestions_path(),
    )
}

/// 分派桌宠自我区（成长笔记 / 人格建议）的**自定义方法**（形状与 `handle_memory_method` 一致：
/// 错误走 `result + isError`）。**三条只有「读成长笔记」与两条「只追加写」** ——
/// 没有任何改核心人格、或改删历史条目的入口（合并只能走宿主命令）。
fn handle_pet_self_method(method: &str, params: Option<&Value>) -> Result<String, String> {
    let content = params
        .and_then(|p| p.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("");
    match method {
        "lunac/pet_growth_read" => {
            let text = pet_growth_read_text()?;
            if text.trim().is_empty() {
                Ok("(pet growth notes are empty)".into())
            } else {
                Ok(text)
            }
        }
        "lunac/pet_growth_write" => pet_growth_append(content),
        "lunac/persona_suggestion_write" => persona_suggestion_append(content),
        other => Err(format!("未知桌宠自我区方法: {other}")),
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
// 这个方法**不进 `tools/list`**，因此不会出现在用户的工具列表里、也不经过审批卡。
//
// 为什么不做成 `<exe 根>\tools\*.json` 里的用户工具：
//   ① 那是**用户**的工具目录 —— 内置能力混进去，用户能在 Tool Editor 里改坏或删掉；
//   ② MCP 工具的调用在 core-agent 里**一律要弹审批卡**（`needs_approval` 对 `mcp__*`
//      恒真），而「读自己本地的会话历史」与 `Read` / `Grep` 同级，不该每问一句就打扰一次。
// 所以走自定义方法，由 agent 侧注册成一等公民工具 `SessionSearch`（免审批、串行）。

/// `history_search` 单次返回的消息条数默认值与上限（上限防模型一次要太多把上下文吃掉）。
const HISTORY_SEARCH_DEFAULT_LIMIT: usize = 10;
const HISTORY_SEARCH_MAX_LIMIT: usize = 30;

/// 现在（毫秒）。与前端 `Date.now()` 同口径 —— 会话的 `created_at` 就是这么存的。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 相对天数标签（检索结果里用；Rust 标准库没有时区表，绝对日期只能是 UTC、会与本地错位）。
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
        // 桌宠自我区（成长笔记 / 人格建议，2026-10-13）：同上，自定义方法、不列进 tools/list。
        // 只有「读成长笔记」与两条「只追加写」；**没有**任何改核心人格的方法（合并走宿主命令）。
        Some(m) if m.starts_with("lunac/pet_") || m.starts_with("lunac/persona_") => {
            let method = m.to_string();
            match handle_pet_self_method(&method, req.params.as_ref()) {
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

    /// 测试用固定日期（B1）：日期由宿主盖，写侧单测不依赖真实时钟。
    const TEST_DATE: &str = "2026-10-17";

    fn tagged(scope: MemoryScope, date: &str) -> Option<MemoryProvenance<'_>> {
        Some(MemoryProvenance { scope, date })
    }

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
    /// 用临时目录而不是 `memory_path_for(..)`：后者指向真实 ModuleData，测试不许碰用户数据。
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

        // ② 追加两条 ⇒ 逐条成行，先写的在前，每条都带宿主来源前缀（B1）
        assert!(write_memory_at(&file, "用户偏好中文", false, tagged(MemoryScope::Global, TEST_DATE)).is_ok());
        assert!(write_memory_at(&file, "项目用 Rust", false, tagged(MemoryScope::Global, TEST_DATE)).is_ok());
        let text = read_memory_at(&file).unwrap();
        assert_eq!(
            text,
            "- [2026-10-17 · global] 用户偏好中文\n- [2026-10-17 · global] 项目用 Rust\n"
        );

        // ③ 同一条重写 ⇒ 不改文件（去重；同日期刷新是 no-op）
        let before = text.clone();
        assert!(write_memory_at(&file, "用户偏好中文", false, tagged(MemoryScope::Global, TEST_DATE))
            .unwrap()
            .contains("already"));
        assert_eq!(read_memory_at(&file).unwrap(), before, "重复条目不得写进去");

        // ④ 空条目 / 超长单条 / 超正文额度替换 ⇒ 拒绝（**不许静默截断**）
        assert!(write_memory_at(&file, "   ", false, tagged(MemoryScope::Global, TEST_DATE)).is_err());
        assert!(write_memory_at(
            &file,
            &"x".repeat(MAX_MEMORY_ENTRY_CHARS + 1),
            false,
            tagged(MemoryScope::Global, TEST_DATE)
        )
        .is_err());
        assert!(write_memory_at(
            &file,
            &"x".repeat(MEMORY_BODY_BUDGET_CHARS + 1),
            true,
            tagged(MemoryScope::Global, TEST_DATE)
        )
        .is_err());

        // ⑤ 写满再追加 ⇒ 报错（而不是悄悄丢掉旧条目）
        // 每条都不同 —— 相同的条目会被去重挡掉（③），那样这条测试就变成测去重了。
        let filler = "y".repeat(400);
        let mut wrote = 0;
        loop {
            let entry = format!("{filler}{wrote}");
            if write_memory_at(&file, &entry, false, tagged(MemoryScope::Global, TEST_DATE)).is_err() {
                break;
            }
            wrote += 1;
            if wrote > 50 {
                panic!("上限没有生效：一直写得进去");
            }
        }
        let full = read_memory_at(&file).unwrap();
        assert!(
            memory_body_chars(&full) <= MEMORY_BODY_BUDGET_CHARS,
            "正文必须 ≤ {MEMORY_BODY_BUDGET_CHARS}，实际 {}",
            memory_body_chars(&full)
        );
        assert!(full.chars().count() <= MEMORY_TOTAL_HARD_CAP_CHARS);
        assert!(wrote > 0, "上限之前应当还能写进去几条");

        // ⑥ replace 模式：整体替换，旧内容不再保留；无前缀 bullet 由宿主补前缀
        assert!(write_memory_at(
            &file,
            "- 只剩这一条",
            true,
            tagged(MemoryScope::Global, TEST_DATE)
        )
        .is_ok());
        assert_eq!(read_memory_at(&file).unwrap(), "- [2026-10-17 · global] 只剩这一条\n");

        // 模型自带的「假日期 / 假 scope」一律被宿主覆盖（日期只信宿主）。
        assert!(write_memory_at(
            &file,
            "- [1999-01-01 · pet] 伪造日期",
            true,
            tagged(MemoryScope::Global, TEST_DATE)
        )
        .is_ok());
        assert_eq!(
            read_memory_at(&file).unwrap(),
            "- [2026-10-17 · global] 伪造日期\n",
            "replace 也由宿主重盖前缀"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// 凭据兜底（2026-10-11，与 A17 / M2-8 同类）：写进记忆的内容落盘前必须过 mask_secrets
    /// —— 对话里出现的 API key 不能跟着 MEMORY.md 进入以后的每一次对话上下文。同时钉住
    /// 「脱敏不弄坏 bullet 形状」与「脱敏不引入写失败路径」两点（复用 log.rs 的 mask_secrets）。
    /// 用临时目录而不是 memory_path_for(..)：测试不许碰真实 ModuleData。
    #[test]
    fn memory_masks_secrets_before_persisting() {
        let dir = std::env::temp_dir().join(format!(
            "lunac-mcp-memsk-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let file = dir.join("MEMORY.md");

        // 裸 key 与键值对两条路径都要打码；多行那条还要保住「- 起头 + 续行缩进」的 bullet 形状。
        // 前缀是落盘前才盖的 ⇒ 这里同时钉住「前缀不会绕过脱敏」。
        assert!(write_memory_at(
            &file,
            "用户提到 key sk-live-99887766",
            false,
            tagged(MemoryScope::Global, TEST_DATE)
        )
        .is_ok());
        assert!(write_memory_at(
            &file,
            "偏好中文\n第二行：token=abcdef123456",
            false,
            tagged(MemoryScope::Global, TEST_DATE)
        )
        .is_ok());
        let text = read_memory_at(&file).unwrap();
        assert!(!text.contains("99887766"), "裸 key 泄露：{text}");
        assert!(!text.contains("abcdef123456"), "键值对凭据泄露：{text}");
        assert!(text.starts_with("- [2026-10-17 · global] "), "首条仍须是带前缀的 bullet：{text}");
        assert!(text.contains("\n  "), "多行续行缩进被破坏了：{text}");

        // 普通内容逐字不变（不能被脱敏误伤）
        assert!(write_memory_at(
            &file,
            "项目用 Rust 构建",
            false,
            tagged(MemoryScope::Global, TEST_DATE)
        )
        .is_ok());
        assert!(read_memory_at(&file)
            .unwrap()
            .contains("- [2026-10-17 · global] 项目用 Rust 构建"));

        let _ = fs::remove_dir_all(&dir);
    }

    /// 清空记忆（斜杠命令 `/memory clear` 的后端 `clear_memory_at`）：把文件截断为空，
    /// 且对「本来就不存在」幂等。用临时目录而不是 `memory_path_for(..)`：测试不许碰真实 ModuleData。
    #[test]
    fn memory_clear_truncates_and_is_idempotent() {
        let dir = std::env::temp_dir().join(format!(
            "lunac-mcp-memclear-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let file = dir.join("MEMORY.md");

        // 先写一条 ⇒ 再清空 ⇒ 读到空串（截断而非删除文件，路径保持不变）
        assert!(write_memory_at(&file, "用户偏好中文", false, tagged(MemoryScope::Global, TEST_DATE)).is_ok());
        assert!(!read_memory_at(&file).unwrap().is_empty());
        assert!(clear_memory_at(&file).is_ok());
        assert_eq!(read_memory_at(&file).unwrap(), "");
        assert!(file.exists(), "清空是截断，不是删文件");

        // 文件本就不存在 ⇒ 空操作也返回 Ok（幂等）
        let missing = dir.join("nope.md");
        assert!(clear_memory_at(&missing).is_ok());

        let _ = fs::remove_dir_all(&dir);
    }

    /// 每个用例一个独立临时目录（不碰真实 `<exe 根>\ModuleData`）。
    fn tmp_memory_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "lunac-mcp-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    /// 作用域隔离（2026-10-13，桌宠私有记忆的地基）：全局与桌宠**各一个文件**，
    /// 写一份不动另一份 —— `scope` 参数化落到不同的路径就是这条隔离的全部实现。
    #[test]
    fn memory_scopes_are_isolated() {
        // 路径映射本身：两份落点不同、都在 memory 目录下。
        let g = memory_path_for(MemoryScope::Global);
        let p = memory_path_for(MemoryScope::Pet);
        assert_ne!(g, p, "全局与桌宠不能是同一个文件");
        assert!(g.ends_with("memory/MEMORY.md"), "{g:?}");
        assert!(p.ends_with("memory/pet/MEMORY.md"), "{p:?}");

        // 两个临时文件代表两个作用域：写桌宠**不得**改动全局。
        let dir = tmp_memory_dir("memscope");
        let gfile = dir.join("MEMORY.md");
        let pfile = dir.join("pet").join("MEMORY.md");
        assert!(write_memory_at(&gfile, "全局约定", false, tagged(MemoryScope::Global, TEST_DATE)).is_ok());
        assert!(write_memory_at(&pfile, "桌宠笔记", false, tagged(MemoryScope::Pet, TEST_DATE)).is_ok());
        assert_eq!(read_memory_at(&gfile).unwrap(), "- [2026-10-17 · global] 全局约定\n");
        assert_eq!(read_memory_at(&pfile).unwrap(), "- [2026-10-17 · pet] 桌宠笔记\n");

        let g_before = read_memory_at(&gfile).unwrap();
        assert!(write_memory_at(&pfile, "桌宠第二条", false, tagged(MemoryScope::Pet, TEST_DATE)).is_ok());
        assert_eq!(read_memory_at(&gfile).unwrap(), g_before, "写桌宠私有不得影响全局注入");
        assert_eq!(list_memory_entries_at(&pfile).unwrap().len(), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    /// 作用域解析：缺省 / `global` = 全局，`pet` = 桌宠；无法识别**报错**（绝不静默落全局）。
    #[test]
    fn memory_scope_parse_is_strict() {
        assert_eq!(MemoryScope::parse(None).unwrap(), MemoryScope::Global);
        assert_eq!(MemoryScope::parse(Some("")).unwrap(), MemoryScope::Global);
        assert_eq!(MemoryScope::parse(Some("global")).unwrap(), MemoryScope::Global);
        assert_eq!(MemoryScope::parse(Some("pet")).unwrap(), MemoryScope::Pet);
        assert_eq!(MemoryScope::parse(Some(" pet ")).unwrap(), MemoryScope::Pet);
        assert!(MemoryScope::parse(Some("other")).is_err());
    }

    /// B1（2026-10-17）：写入层给每条 bullet 盖 `- [YYYY-MM-DD · scope]` 前缀，日期取宿主本地
    /// 日期；两个作用域各写各的文件、前缀里的 scope id 正确（互不串）。
    #[test]
    fn memory_entries_are_tagged_with_host_date_and_scope() {
        let dir = tmp_memory_dir("memtag");
        let gfile = dir.join("MEMORY.md");
        let pfile = dir.join("pet").join("MEMORY.md");
        assert!(write_memory_at(&gfile, "用户偏好中文", false, tagged(MemoryScope::Global, TEST_DATE)).is_ok());
        assert!(write_memory_at(&pfile, "桌宠喜欢被摸头", false, tagged(MemoryScope::Pet, TEST_DATE)).is_ok());
        assert_eq!(read_memory_at(&gfile).unwrap(), "- [2026-10-17 · global] 用户偏好中文\n");
        assert_eq!(read_memory_at(&pfile).unwrap(), "- [2026-10-17 · pet] 桌宠喜欢被摸头\n");
        assert!(!read_memory_at(&gfile).unwrap().contains("摸头"), "全局文件不得混入桌宠条目");
        assert!(!read_memory_at(&pfile).unwrap().contains("用户偏好"), "桌宠文件不得混入全局条目");

        // 日期来自宿主本地时钟（Win32 GetLocalTime），必须是 10 位 ISO 形状。
        let today = local_today();
        let b = today.as_bytes();
        assert_eq!(today.len(), 10, "日期必须 YYYY-MM-DD：{today}");
        assert!(b[4] == b'-' && b[7] == b'-', "分隔符位置错：{today}");
        assert!(
            b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit()),
            "非法日期：{today}"
        );
        assert_eq!(
            split_provenance_prefix(&format!("[{} · global] x", today)).map(|(_, body)| body),
            Some("x")
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// B1 去重新语义：去掉 `[日期 · scope]` 后同正文 ⇒ **不追加**，只把该条日期刷新为本次
    /// 写入日期（语义 = 最后确认时间），正文与条目数不变。
    #[test]
    fn memory_same_body_on_a_new_date_is_refreshed_not_duplicated() {
        let dir = tmp_memory_dir("memrefresh");
        let file = dir.join("MEMORY.md");
        assert!(write_memory_at(
            &file,
            "User prefers TABS",
            false,
            tagged(MemoryScope::Global, "2026-10-16")
        )
        .is_ok());
        let msg = write_memory_at(
            &file,
            "user prefers   tabs",
            false,
            tagged(MemoryScope::Global, "2026-10-17"),
        )
        .unwrap();
        assert!(msg.contains("already"), "{msg}");
        assert_eq!(
            read_memory_at(&file).unwrap(),
            "- [2026-10-17 · global] User prefers TABS\n",
            "只刷新日期：正文保持首次落盘的原文，条目数不变"
        );
        assert_eq!(list_memory_entries_at(&file).unwrap(), vec!["User prefers TABS".to_string()]);

        // 再以更早的日期写入同一条 ⇒ 同样只刷新（最后确认时间可以变早，由写入顺序决定）。
        assert!(write_memory_at(
            &file,
            "USER PREFERS TABS",
            false,
            tagged(MemoryScope::Global, "2026-10-15")
        )
        .unwrap()
        .contains("already"));
        assert_eq!(read_memory_at(&file).unwrap(), "- [2026-10-15 · global] User prefers TABS\n");

        let _ = fs::remove_dir_all(&dir);
    }

    /// B1 老数据兼容：文件里已有的无前缀旧 bullet 照常可读；同正文再次写入时**原地补前缀**
    /// （不是新增第二条），且**不批量回填**其他历史条目（它们没有可信的旧日期）。
    #[test]
    fn memory_legacy_untagged_entry_is_migrated_in_place_on_rewrite() {
        let dir = tmp_memory_dir("memlegacy");
        let file = dir.join("MEMORY.md");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&file, "- 用户偏好中文\n- 项目用 Rust\n").unwrap();

        let msg = write_memory_at(
            &file,
            "  用户偏好中文 ",
            false,
            tagged(MemoryScope::Global, TEST_DATE),
        )
        .unwrap();
        assert!(msg.contains("already"), "{msg}");
        assert_eq!(
            read_memory_at(&file).unwrap(),
            "- [2026-10-17 · global] 用户偏好中文\n- 项目用 Rust\n",
            "只给被再次写入的那条补前缀：不新增、不批量回填"
        );
        assert_eq!(
            list_memory_entries_at(&file).unwrap(),
            vec!["用户偏好中文".to_string(), "项目用 Rust".to_string()],
            "旧条目照常可读（列表不展示来源前缀）"
        );

        // 另一条老条目再次写入 ⇒ 同样原地补，文件里仍只有两条 bullet。
        write_memory_at(
            &file,
            "项目用 Rust",
            false,
            tagged(MemoryScope::Global, "2026-10-18"),
        )
        .unwrap();
        assert_eq!(
            read_memory_at(&file).unwrap(),
            "- [2026-10-17 · global] 用户偏好中文\n- [2026-10-18 · global] 项目用 Rust\n"
        );
        assert_eq!(split_entries(&read_memory_at(&file).unwrap()).len(), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    /// B1：多行 bullet 的前缀只加在首行 `- ` 之后，续行缩进原样保留（新写入与旧条目迁移都是）。
    #[test]
    fn memory_multiline_bullets_keep_continuation_indent() {
        let dir = tmp_memory_dir("memmulti");
        let file = dir.join("MEMORY.md");
        assert!(write_memory_at(
            &file,
            "第一行\n第二行\n第三行",
            false,
            tagged(MemoryScope::Global, TEST_DATE)
        )
        .is_ok());
        assert_eq!(
            read_memory_at(&file).unwrap(),
            "- [2026-10-17 · global] 第一行\n  第二行\n  第三行\n"
        );

        // 旧的无前缀多行条目 + 同正文写入 ⇒ 补前缀，续行缩进不动。
        let legacy = dir.join("LEGACY.md");
        fs::write(&legacy, "- 多行第一条\n  续行内容\n").unwrap();
        assert!(write_memory_at(
            &legacy,
            "多行第一条\n续行内容",
            false,
            tagged(MemoryScope::Global, "2026-10-18")
        )
        .unwrap()
        .contains("already"));
        assert_eq!(
            read_memory_at(&legacy).unwrap(),
            "- [2026-10-18 · global] 多行第一条\n  续行内容\n"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// B1：加前缀后各条硬闸仍按**最终落盘内容**生效（单条 2000 含前缀 / 正文额度 6000 /
    /// 总长硬顶 9000），超限报错、不静默截断、不落盘。
    ///
    /// 2026-10-17 动态上限口径：整文件闸从「文件总长 6000」改为「正文合计 6000」
    /// （前缀不计入，另受总长 9000 硬顶约束）⇒ 本测试的整文件断言同步改口径。
    #[test]
    fn memory_caps_still_apply_after_tagging() {
        let dir = tmp_memory_dir("memcap");
        let file = dir.join("MEMORY.md");
        let prefix = provenance_prefix(MemoryScope::Global, TEST_DATE);
        let overhead = "- ".chars().count() + prefix.chars().count() + 1;
        let fit = "a".repeat(MAX_MEMORY_ENTRY_CHARS - overhead);
        assert!(
            write_memory_at(&file, &fit, false, tagged(MemoryScope::Global, TEST_DATE)).is_ok(),
            "恰好 2000 字符（含前缀）应写进去"
        );
        assert_eq!(
            read_memory_at(&file).unwrap().trim_end().chars().count(),
            MAX_MEMORY_ENTRY_CHARS,
            "单条上限必须按最终落盘内容（含 - 与来源前缀）计算"
        );

        let over = "b".repeat(MAX_MEMORY_ENTRY_CHARS - overhead + 1);
        assert!(
            write_memory_at(&file, &over, false, tagged(MemoryScope::Global, TEST_DATE)).is_err(),
            "加前缀后超 2000 必须拒绝"
        );

        // 正文额度：清空后反复追加，正文合计绝不越过 6000；越线那次报错且不落盘。
        assert!(clear_memory_at(&file).is_ok());
        let big = "c".repeat(400);
        let mut i = 0;
        loop {
            let entry = format!("{big}{i}");
            if let Err(e) = write_memory_at(&file, &entry, false, tagged(MemoryScope::Global, TEST_DATE)) {
                assert!(e.contains("正文额度"), "400 字条目应先撞正文额度：{e}");
                break;
            }
            i += 1;
            assert!(i < 100, "总量上限没生效：一直写得进去");
        }
        let full = read_memory_at(&file).unwrap();
        assert!(
            memory_body_chars(&full) <= MEMORY_BODY_BUDGET_CHARS,
            "正文合计必须 ≤ {MEMORY_BODY_BUDGET_CHARS}，实际 {}",
            memory_body_chars(&full)
        );
        assert!(
            full.chars().count() <= MEMORY_TOTAL_HARD_CAP_CHARS,
            "文件总长必须 ≤ {MEMORY_TOTAL_HARD_CAP_CHARS}，实际 {}",
            full.chars().count()
        );
        assert!(
            write_memory_at(&file, &format!("{big}{i}"), false, tagged(MemoryScope::Global, TEST_DATE)).is_err()
        );
        assert_eq!(read_memory_at(&file).unwrap(), full, "超限不得落盘");

        let _ = fs::remove_dir_all(&dir);
    }

    /// ① 动态上限的正面语义：正文合计 6000（前缀不计入）⇒ 允许写入，哪怕文件总长已超 6000。
    #[test]
    fn memory_body_budget_excludes_provenance_prefix() {
        let dir = tmp_memory_dir("membudget");
        let file = dir.join("MEMORY.md");
        // 50 条 × 120 字 = 6000 正文；每条前缀 + "- " + 换行约 25 字 ⇒ 总长 7000+。
        for i in 0..50 {
            let entry = format!("{i:03}{}", "a".repeat(117));
            assert_eq!(entry.chars().count(), 120);
            assert!(
                write_memory_at(&file, &entry, false, tagged(MemoryScope::Global, TEST_DATE)).is_ok(),
                "正文未超额（第 {i} 条）不应因前缀被拒"
            );
        }
        let full = read_memory_at(&file).unwrap();
        assert_eq!(memory_body_chars(&full), MEMORY_BODY_BUDGET_CHARS);
        assert!(
            full.chars().count() > MEMORY_BODY_BUDGET_CHARS && full.chars().count() > 7_000,
            "前缀不计入正文，文件总长应超过 7000：实际 {}",
            full.chars().count()
        );
        assert!(full.chars().count() <= MEMORY_TOTAL_HARD_CAP_CHARS);
        assert_eq!(list_memory_entries_at(&file).unwrap().len(), 50);

        let _ = fs::remove_dir_all(&dir);
    }

    /// ② 正文额度超 1 字符 ⇒ 拒，且错误语义 = 正文额度（不是总长硬顶）。
    #[test]
    fn memory_body_budget_overflow_is_rejected_with_body_semantics() {
        let dir = tmp_memory_dir("membodyover");
        let file = dir.join("MEMORY.md");
        // 3 × 1900 = 5700；再来一条 301 ⇒ 正文 6001（单条均未超 2000）。
        for i in 0..3 {
            let entry = format!("{i}{}", "d".repeat(1899));
            assert!(write_memory_at(&file, &entry, false, tagged(MemoryScope::Global, TEST_DATE)).is_ok());
        }
        let before = read_memory_at(&file).unwrap();
        let err = write_memory_at(
            &file,
            &format!("{}e", "d".repeat(300)),
            false,
            tagged(MemoryScope::Global, TEST_DATE),
        )
        .unwrap_err();
        assert!(err.contains("正文额度"), "应报正文额度：{err}");
        assert!(!err.contains("总长度硬顶"), "正文先超时不应报总长：{err}");
        assert_eq!(read_memory_at(&file).unwrap(), before, "超正文额度不得落盘");

        let _ = fs::remove_dir_all(&dir);
    }

    /// ③ 正文未满 6000，但条目极多导致前缀把文件顶过 9000 ⇒ 拒，语义 = 总长度硬顶。
    #[test]
    fn memory_total_hard_cap_rejects_prefix_heavy_file() {
        let dir = tmp_memory_dir("memtotalcap");
        let file = dir.join("MEMORY.md");
        // 每条正文 20 字、落盘一行 45 字（前缀 21 + "- " 2 + 空格 1 + 换行 1）。
        // 200 条 ⇒ 总长恰 9000；第 201 条把总量顶到 9045（正文仍只有 4020）。
        let mut wrote = 0usize;
        loop {
            let entry = format!("t{wrote:019}");
            assert_eq!(entry.chars().count(), 20);
            match write_memory_at(&file, &entry, false, tagged(MemoryScope::Global, TEST_DATE)) {
                Ok(_) => {
                    wrote += 1;
                    assert!(wrote <= 200, "9000 硬顶没生效：第 {wrote} 条仍写进去了");
                }
                Err(e) => {
                    assert!(e.contains("总长度硬顶"), "应报总长度硬顶：{e}");
                    assert!(!e.contains("正文额度"), "正文未满时不应报正文额度：{e}");
                    break;
                }
            }
        }
        assert_eq!(wrote, 200);
        let full = read_memory_at(&file).unwrap();
        assert_eq!(full.chars().count(), MEMORY_TOTAL_HARD_CAP_CHARS);
        assert!(memory_body_chars(&full) < MEMORY_BODY_BUDGET_CHARS, "正文应仍有余量");

        let _ = fs::remove_dir_all(&dir);
    }

    /// ④ `replace: true` 同样受两口径约束：正文超额先报正文额度；条目前缀把总长顶过 9000
    /// 则报总长度硬顶（正文未满也不放行）。
    #[test]
    fn memory_replace_obeys_both_budgets() {
        let dir = tmp_memory_dir("memreplace");
        let file = dir.join("MEMORY.md");

        // 正文超额：4 × 2000 = 8000 正文，总长约 8100（未过 9000）⇒ 必须报正文额度。
        let body_over: Vec<String> = (0..4).map(|i| format!("- {i}{}", "r".repeat(1999))).collect();
        let err = write_memory_at(
            &file,
            &body_over.join("\n"),
            true,
            tagged(MemoryScope::Global, TEST_DATE),
        )
        .unwrap_err();
        assert!(err.contains("正文额度"), "replace 正文超额应报正文额度：{err}");
        assert!(!file.exists(), "拒绝时不得落盘");

        // 总长超额：300 条 × 11 字正文 = 3300 正文；落盘每行 36 字 ⇒ 总长 10800 ⇒ 报总长硬顶。
        let too_many: Vec<String> = (0..300).map(|i| format!("- e{i:03}{}", "x".repeat(7))).collect();
        let err = write_memory_at(
            &file,
            &too_many.join("\n"),
            true,
            tagged(MemoryScope::Global, TEST_DATE),
        )
        .unwrap_err();
        assert!(err.contains("总长度硬顶"), "replace 前缀过多应报总长硬顶：{err}");
        assert!(!err.contains("正文额度"), "正文未满时不应报正文额度：{err}");
        assert!(!file.exists(), "拒绝时不得落盘");

        let _ = fs::remove_dir_all(&dir);
    }

    /// ⑤ 两口径按 **mask_secrets 之后**的内容判定（不是入参原文）。
    /// · 脱敏变短：原文正文 6100+ 会超额，脱敏后只剩十几个字 ⇒ 应允许写入；
    /// · 脱敏变长：原文正文恰 6000，脱敏后 6001 ⇒ 应报正文额度（按原文判会误放行）。
    #[test]
    fn memory_budgets_are_judged_after_masking() {
        let dir = tmp_memory_dir("memmaskbudget");

        // 变短：裸 key 的整段值被打成 sk-***。
        let shrink_file = dir.join("SHRINK.md");
        let long_secret = format!("偏好中文 sk-{}", "z".repeat(6_100));
        assert!(
            write_memory_at(&shrink_file, &long_secret, false, tagged(MemoryScope::Global, TEST_DATE)).is_ok(),
            "脱敏后已低于正文额度，不得按原文拒绝"
        );
        let shrunk = read_memory_at(&shrink_file).unwrap();
        assert!(shrunk.contains("sk-***"), "脱敏未生效：{shrunk}");
        assert!(memory_body_chars(&shrunk) < MEMORY_BODY_BUDGET_CHARS);

        // 变长：3 × 1900 + (296 + sk-a) = 6000；sk-a → sk-*** 后为 6001。
        let grow_file = dir.join("GROW.md");
        for i in 0..3 {
            let entry = format!("{i}{}", "g".repeat(1899));
            assert!(write_memory_at(&grow_file, &entry, false, tagged(MemoryScope::Global, TEST_DATE)).is_ok());
        }
        assert_eq!(memory_body_chars(&read_memory_at(&grow_file).unwrap()), 5700);
        let before = read_memory_at(&grow_file).unwrap();
        let tail = format!("{}sk-a", "g".repeat(296));
        assert_eq!(tail.chars().count(), 300);
        let err = write_memory_at(&grow_file, &tail, false, tagged(MemoryScope::Global, TEST_DATE)).unwrap_err();
        assert!(err.contains("正文额度"), "脱敏后正文 6001 应报正文额度：{err}");
        assert_eq!(read_memory_at(&grow_file).unwrap(), before, "超限不得落盘");

        let _ = fs::remove_dir_all(&dir);
    }

    /// 去重按**归一**判据（折叠空白 + 大小写不敏感）：只差空格 / 大小写的同一条不得重复落盘。
    /// 归一判据与逐条删共用 ⇒ 「列出来的那条一定删得掉」。
    #[test]
    fn memory_dedupes_on_normalized_entry() {
        let dir = tmp_memory_dir("memnorm");
        let file = dir.join("MEMORY.md");
        assert!(write_memory_at(&file, "User prefers   TABS", false, tagged(MemoryScope::Global, TEST_DATE)).is_ok());
        // 只差内部多余空格 ⇒ 视为同一条
        assert!(write_memory_at(&file, "User prefers TABS", false, tagged(MemoryScope::Global, TEST_DATE))
            .unwrap()
            .contains("already"));
        // 只差大小写 ⇒ 视为同一条
        assert!(write_memory_at(&file, "user prefers tabs", false, tagged(MemoryScope::Global, TEST_DATE))
            .unwrap()
            .contains("already"));
        // 内容确实不同 ⇒ 不是重复，要落盘
        assert!(!write_memory_at(&file, "User prefers spaces", false, tagged(MemoryScope::Global, TEST_DATE))
            .unwrap()
            .contains("already"));
        assert_eq!(list_memory_entries_at(&file).unwrap().len(), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    /// 逐条删（2026-10-13，桌宠设置窗「逐条删」的地基）：删 A 不动 B；多行条目整块摘；
    /// 找不到 / 空内容 ⇒ 报错（不谎报成功）；删空后读为空串（注入侧即无正文）。
    #[test]
    fn memory_delete_removes_one_entry_only() {
        let dir = tmp_memory_dir("memdel");
        let file = dir.join("MEMORY.md");
        assert!(write_memory_at(&file, "条目A", false, tagged(MemoryScope::Global, TEST_DATE)).is_ok());
        assert!(write_memory_at(&file, "条目B", false, tagged(MemoryScope::Global, TEST_DATE)).is_ok());
        assert!(write_memory_at(&file, "多行第一条\n续行内容", false, tagged(MemoryScope::Global, TEST_DATE)).is_ok());
        assert_eq!(list_memory_entries_at(&file).unwrap().len(), 3);

        // 删 A（归一匹配：大小写差异也认）—— B 与多行条目必须原样在
        assert!(delete_memory_entry_at(&file, "条目a").is_ok());
        assert_eq!(
            list_memory_entries_at(&file).unwrap(),
            vec!["条目B".to_string(), "多行第一条\n续行内容".to_string()],
            "只该删掉 A"
        );
        let text = read_memory_at(&file).unwrap();
        assert!(text.contains("] 条目B"), "文件里条目 B 应带前缀：{text}");
        assert!(!text.contains("条目A"));

        // 找不到 / 空 needle ⇒ 报错，不谎报成功
        assert!(delete_memory_entry_at(&file, "没有这条").is_err());
        assert!(delete_memory_entry_at(&file, "   ").is_err());

        // 多行条目（含续行）整块删
        assert!(delete_memory_entry_at(&file, "多行第一条\n续行内容").is_ok());
        assert_eq!(list_memory_entries_at(&file).unwrap(), vec!["条目B".to_string()]);

        // 删空 ⇒ 读到空串、无条目（注入侧即空）
        assert!(delete_memory_entry_at(&file, "条目B").is_ok());
        assert_eq!(read_memory_at(&file).unwrap(), "");
        assert!(list_memory_entries_at(&file).unwrap().is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    // ── 桌宠成长笔记 / 人格建议（2026-10-13，人格地基）─────────────

    /// 落点：与桌宠私有记忆**同目录、另立文件**，且两款都**不在 `config\` 下**。
    #[test]
    fn pet_self_paths_are_fixed_siblings_outside_config() {
        let growth = pet_growth_path();
        let suggest = persona_suggestions_path();
        let pet_mem = memory_path_for(MemoryScope::Pet);
        assert_eq!(growth.parent(), pet_mem.parent(), "与桌宠记忆同目录（同族）");
        assert_ne!(growth, pet_mem, "成长笔记另立文件，不混进 MEMORY.md");
        assert_ne!(suggest, pet_mem, "建议区另立文件，不混进 MEMORY.md");
        assert!(growth.ends_with("memory/pet/GROWTH.md"), "{growth:?}");
        assert!(suggest.ends_with("memory/pet/PERSONA_SUGGESTIONS.md"), "{suggest:?}");
        for p in [&growth, &suggest] {
            assert!(
                !p.to_string_lossy().contains("config"),
                "人格两区不得落进 config\\（§7 工作锁 / 用户资产纯粹）：{p:?}"
            );
        }
    }

    /// 成长笔记的**只追加 / 归一去重 / 上限 / 逐条删**四条硬行为。
    ///
    /// 成长笔记与桌宠记忆走**同一批助手**（`write_memory_at` / `list_memory_entries_at` /
    /// `delete_memory_entry_at`，只是落点换成 `GROWTH.md`）—— 故这里用临时 `GROWTH.md` 直接钉住
    /// 这四条本事；固定落点的形状由 `pet_self_paths_are_fixed_siblings_outside_config` 单独钉。
    #[test]
    fn pet_growth_file_is_append_only_deduped_capped_and_deletable() {
        let dir = tmp_memory_dir("petgrowth");
        let file = dir.join("GROWTH.md");

        // ① 只追加：先写一条，再写另一条 —— 前一条必须原样在（不覆盖）
        assert!(write_memory_at(&file, "说话可以更简短一点", false, None).is_ok());
        assert!(write_memory_at(&file, "被夸时轻轻晃一下就好", false, None).is_ok());
        assert_eq!(
            list_memory_entries_at(&file).unwrap(),
            vec!["说话可以更简短一点".to_string(), "被夸时轻轻晃一下就好".to_string()]
        );

        // ② 归一去重：只差空白 / 大小写视为同一条，不重复落盘
        assert!(write_memory_at(&file, "  说话可以更简短一点  ", false, None)
            .unwrap()
            .contains("already"));
        assert_eq!(list_memory_entries_at(&file).unwrap().len(), 2);

        // ③ 单条超限 ⇒ 报错、不落盘（不静默截断）
        let before = read_memory_at(&file).unwrap();
        assert!(write_memory_at(&file, &"x".repeat(MAX_MEMORY_ENTRY_CHARS + 1), false, None).is_err());
        assert_eq!(read_memory_at(&file).unwrap(), before, "超限不得落盘");

        // ④ 逐条删：删 A 不动 B
        assert!(delete_memory_entry_at(&file, "说话可以更简短一点").is_ok());
        assert_eq!(
            list_memory_entries_at(&file).unwrap(),
            vec!["被夸时轻轻晃一下就好".to_string()]
        );

        // ⑤ 总量超限 ⇒ 报错、不落盘（把文件顶到接近上限再追加）
        let big = "y".repeat(1_900);
        let mut i = 0;
        while write_memory_at(&file, &format!("{big}{i}"), false, None).is_ok() {
            i += 1;
            assert!(i < 20, "总量上限没生效：一直写得进去");
        }
        let full = read_memory_at(&file).unwrap();
        assert!(memory_body_chars(&full) <= MEMORY_BODY_BUDGET_CHARS);
        assert!(full.chars().count() <= MEMORY_TOTAL_HARD_CAP_CHARS);
        assert!(write_memory_at(&file, &format!("{big}{i}"), false, None).is_err(), "总量超限必须报错");
        assert_eq!(read_memory_at(&file).unwrap(), full, "总量超限不得落盘");

        let _ = fs::remove_dir_all(&dir);
    }

    /// 桌宠自我区的**桥方法**只认「读成长笔记」+ 两条「只追加写」；
    /// **任何「写核心人格」的方法名一律落到「未知方法」** —— 这是「AI 无改核心人格路径」的
    /// 宿主侧证据（AI 只能经 `lunac/*` 到达宿主）。
    #[test]
    fn pet_self_bridge_rejects_any_persona_write() {
        // 两条合法写 = 只追加。空 content ⇒ 报「content 为空」，**不是**「未知方法」⇒ 证明被识别。
        for ok in ["lunac/pet_growth_write", "lunac/persona_suggestion_write"] {
            let e = handle_pet_self_method(ok, Some(&json!({ "content": "   " }))).unwrap_err();
            assert!(!e.contains("未知"), "{ok} 应被识别为合法方法：{e}");
        }
        // 任何「写人格」形态一律不被识别（桥不提供这条路径）
        for bad in [
            "lunac/persona_write",
            "lunac/persona_merge",
            "lunac/pet_persona_write",
            "lunac/set_pet_persona",
            "lunac/merge_persona_suggestion",
        ] {
            let e = handle_pet_self_method(bad, Some(&json!({ "content": "x" }))).unwrap_err();
            assert!(e.contains("未知"), "{bad} 不该被桥识别（AI 不得改核心人格）：{e}");
        }
    }

    /// ⑦ 建议 → 合并：**复用 `set_persona` / `set_pet_persona` 同一套校验**
    /// （长度上限 / NUL / 先校验后写）；且只能合并建议区里真有的那一条。
    #[test]
    fn persona_suggestion_merge_reuses_persona_validation() {
        let dir = tmp_memory_dir("petmerge");
        let persona = dir.join("pet-persona.md");
        let sug = dir.join("PERSONA_SUGGESTIONS.md");

        // 不在册的建议 ⇒ 拒绝（不允许凭空塞一段进人格）
        assert!(merge_persona_suggestion_at("凭空一段", &persona, &sug).is_err());

        // 在册建议 ⇒ 合并成功：人格追加该段、建议从建议区摘掉
        assert!(write_memory_at(&persona, "你是桌面宠物。", true, None).is_ok());
        assert!(write_memory_at(&sug, "说话时可以偶尔用一点点俏皮话", false, None).is_ok());
        assert!(merge_persona_suggestion_at("说话时可以偶尔用一点点俏皮话", &persona, &sug).is_ok());
        let p = read_memory_at(&persona).unwrap();
        assert!(p.contains("你是桌面宠物。") && p.contains("说话时可以偶尔用一点点俏皮话"));
        assert!(list_memory_entries_at(&sug).unwrap().is_empty(), "合并成功后建议应被摘掉");

        // 长度上限：合并后超 `MAX_PERSONA_CHARS` ⇒ 报错，**人格与建议区都不动**（先校验后写）
        fs::create_dir_all(&dir).unwrap();
        fs::write(&persona, "x".repeat(crate::storage::MAX_PERSONA_CHARS)).unwrap();
        assert!(write_memory_at(&sug, "再补一句俏皮话", false, None).is_ok());
        let before = read_memory_at(&persona).unwrap();
        let err = merge_persona_suggestion_at("再补一句俏皮话", &persona, &sug).unwrap_err();
        assert!(err.contains("上限") || err.contains("过长"), "应报长度上限：{err}");
        assert_eq!(read_memory_at(&persona).unwrap(), before, "校验不过不得写人格");
        assert_eq!(list_memory_entries_at(&sug).unwrap().len(), 1, "校验不过不得摘建议");

        let _ = fs::remove_dir_all(&dir);
    }

    /// ⑧ 清空建议区：**只截断 `PERSONA_SUGGESTIONS.md`** —— 核心人格 `pet-persona.md` 与另两张
    /// 列表（`MEMORY.md` / `GROWTH.md`）一个字节都不动；文件不存在 ⇒ 幂等空操作（不报错、不新建）。
    ///
    /// 清空命令 `clear_persona_suggestions` 走的正是 `clear_memory_at(&persona_suggestions_path())`
    /// （与 `pet_growth_clear` 同款复用）⇒ 这里用临时文件直接钉住「只动这一份」这条硬行为；
    /// 固定落点由 `pet_self_paths_are_fixed_siblings_outside_config` 单独钉。
    #[test]
    fn persona_suggestions_clear_only_truncates_its_own_file() {
        let dir = tmp_memory_dir("petsuggestclear");
        let persona = dir.join("pet-persona.md");
        let mem = dir.join("MEMORY.md");
        let growth = dir.join("GROWTH.md");
        let sug = dir.join("PERSONA_SUGGESTIONS.md");

        // 四份都先有内容
        assert!(write_memory_at(&persona, "你是桌面宠物。", true, None).is_ok());
        assert!(write_memory_at(&mem, "用户喜欢简洁", false, tagged(MemoryScope::Pet, TEST_DATE)).is_ok());
        assert!(write_memory_at(&growth, "说话可以更简短一点", false, None).is_ok());
        assert!(write_memory_at(&sug, "可以偶尔用点俏皮话", false, None).is_ok());
        assert!(write_memory_at(&sug, "被夸时轻轻晃一下", false, None).is_ok());
        assert_eq!(list_memory_entries_at(&sug).unwrap().len(), 2);

        let persona_before = read_memory_at(&persona).unwrap();
        let mem_before = read_memory_at(&mem).unwrap();
        let growth_before = read_memory_at(&growth).unwrap();

        // 清空建议区（与宿主命令同一实现）：条目清空、读为空串
        assert!(clear_memory_at(&sug).is_ok());
        assert_eq!(read_memory_at(&sug).unwrap(), "");
        assert!(list_memory_entries_at(&sug).unwrap().is_empty(), "清空后建议列表必须为空");

        // 另三份原样不动
        assert_eq!(read_memory_at(&persona).unwrap(), persona_before, "核心人格不得被清空波及");
        assert_eq!(read_memory_at(&mem).unwrap(), mem_before, "事实记忆不得被清空波及");
        assert_eq!(read_memory_at(&growth).unwrap(), growth_before, "成长笔记不得被清空波及");

        // 幂等：文件不存在时清空不报错，且不凭空新建文件
        let missing = dir.join("NOT_THERE.md");
        assert!(clear_memory_at(&missing).is_ok(), "文件不存在 ⇒ 幂等空操作，不报错");
        assert!(!missing.exists(), "清空不得凭空新建文件");

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

