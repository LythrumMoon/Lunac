// core-agent/src/tools.rs
// P1 内置工具：Read / Write / Edit / Bash / Glob / Grep
//
// 工具名保持 PascalCase —— 前端 main.ts 对 "Bash" 有专门的命令展示分支
// （agentToolArgsDelta / classifyRequest），改名会破坏既有 UI 契约。
//
// 权限策略（P1 无审批通道，按启动参数静态裁决；P2 接入 can_use_tool 后
// 改为「先问前端，再执行」）：
//   · --permission-mode plan      → 只读：Write / Edit / Bash 直接拒绝
//   · 其余档位（默认 acceptEdits）→ 六件工具全开
//   · LUNAC_WORKSPACE_LOCKED=1    → 文件类工具限制在工作区内，越界拒绝
//   · --dangerously-skip-permissions → 忽略工作区锁
//
// 本文件为 Lunac 自研实现，不派生自任何第三方源码。

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// 单次工具结果回传给模型的字符上限（超出截断，避免撑爆上下文）
const MAX_RESULT_CHARS: usize = 30_000;
/// Read 一次最多返回的行数
const MAX_READ_LINES: usize = 2000;
/// Glob / Grep 的结果条数上限
const MAX_ENTRIES: usize = 200;
/// 文本文件大小上限（超过视为二进制/不适合阅读）
const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;
/// 子进程 stdout/stderr 的捕获上限
const MAX_PIPE_BYTES: usize = 512 * 1024;
/// Bash 默认/最大超时（毫秒）
const BASH_TIMEOUT_MS: u64 = 120_000;
const BASH_MAX_TIMEOUT_MS: u64 = 600_000;
/// Glob 递归深度上限
const MAX_DEPTH: usize = 12;

/// 目录遍历时跳过的常见重目录（避免 Glob/Grep 卡在依赖上）
const SKIP_DIRS: [&str; 13] = [
    ".git", "node_modules", "target", "dist", "out", ".next", ".nuxt", ".venv", "venv",
    "__pycache__", ".idea", ".vscode", "build",
];

/// 工具执行上下文（由 main.rs 依据启动参数构建）
pub struct Ctx {
    /// 进程工作目录 —— src-tauri 用它传「AI 工作区」
    pub cwd: PathBuf,
    /// `--add-dir` 追加的可访问目录
    pub add_dirs: Vec<PathBuf>,
    /// `--permission-mode plan` → 只读
    pub read_only: bool,
    /// 越界拦截开关（工作区已配置时为真）
    pub locked: bool,
}

// ── 工具定义（Anthropic Messages API 的 tools schema）─────────────

/// 六个内置工具的 schema；`disallowed`（来自 `--disallowedTools`）里的
/// 名字不进入请求体 —— 数组更短，也少一轮缓存失效。
pub fn defs(disallowed: &[String]) -> Vec<Value> {
    let all = vec![
        json!({
            "name": "Read",
            "description": "Read a UTF-8 text file from the local filesystem. Returns the \
                content with 1-based line numbers. Use offset/limit for large files.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute or workspace-relative path" },
                    "offset": { "type": "integer", "description": "First line to return (1-based)" },
                    "limit": { "type": "integer", "description": "Maximum number of lines" }
                },
                "required": ["file_path"]
            }
        }),
        json!({
            "name": "Write",
            "description": "Write a file (creates parent directories). Overwrites existing content.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["file_path", "content"]
            }
        }),
        json!({
            "name": "Edit",
            "description": "Replace an exact string in a file. Fails when old_string is missing \
                or appears more than once (set replace_all to replace every occurrence).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": { "type": "string" },
                    "old_string": { "type": "string" },
                    "new_string": { "type": "string" },
                    "replace_all": { "type": "boolean", "description": "Replace every occurrence" }
                },
                "required": ["file_path", "old_string", "new_string"]
            }
        }),
        json!({
            "name": "Bash",
            "description": "Run a shell command (cmd /C on Windows) in the working directory \
                and return its combined output. Long-running commands are killed on timeout.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "timeout": { "type": "integer", "description": "Timeout in milliseconds (default 120000, max 600000)" }
                },
                "required": ["command"]
            }
        }),
        json!({
            "name": "Glob",
            "description": "Find files by glob pattern (** recurses, * does not cross directories). \
                Returns matching file paths.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "e.g. \"**/*.rs\" or \"src/*.ts\"" },
                    "path": { "type": "string", "description": "Base directory (defaults to the working directory)" }
                },
                "required": ["pattern"]
            }
        }),
        json!({
            "name": "Grep",
            "description": "Search file contents with a Rust regular expression. Returns \
                path:line:content for every match.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regular expression" },
                    "path": { "type": "string", "description": "File or directory (defaults to the working directory)" },
                    "glob": { "type": "string", "description": "Only search files matching this pattern, e.g. \"*.ts\"" },
                    "ignore_case": { "type": "boolean" }
                },
                "required": ["pattern"]
            }
        }),
    ];

    all.into_iter()
        .filter(|d| {
            let name = d.get("name").and_then(Value::as_str).unwrap_or("");
            !disallowed.iter().any(|x| x == name)
        })
        .collect()
}

/// 工具名列表（system/init 上报给前端）
pub fn names(tools: &[Value]) -> Vec<String> {
    tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// 是否需要先过用户审批（P2 的 `can_use_tool`）。
///
/// 只读三件不需要（工作区锁已是硬边界）；写类三件一律先问 —— 前端
/// `classifyRequest()` 会自行处理「白名单 / 内置安全前缀自动放行」与
/// 「危险命令只给手动确认」，所以 agent 侧不做二次判断，问就完了。
/// 返回真实改动前用户应看到提示的调用也在此列（含 Bash 的只读命令）。
///
/// `plan` 档不在此判断：那三件工具会被 tools::run 直接拒绝，压根到不了审批。
pub fn needs_approval(name: &str) -> bool {
    matches!(name, "Write" | "Edit" | "Bash")
}

// ── 分发 ─────────────────────────────────────────────────────────

/// 执行一个工具调用。Err 会成为 is_error=true 的 tool_result
/// （模型看得见，可自行纠正），而不是中断整轮对话。
pub fn run(ctx: &Ctx, name: &str, input: &Value) -> Result<String, String> {
    match name {
        "Read" => read(ctx, input),
        "Write" => write(ctx, input),
        "Edit" => edit(ctx, input),
        "Bash" => bash(ctx, input),
        "Glob" => glob(ctx, input),
        "Grep" => grep(ctx, input),
        other => Err(format!("Unknown tool: {other}")),
    }
}

// ── 路径解析与越界拦截 ───────────────────────────────────────────

fn str_arg(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing required parameter \"{key}\""))
}

/// 词法规范化（不要求路径存在，故不能用 canonicalize）：
/// 去掉 `.` 与 `..`，避免 `..\..\` 绕过工作区前缀判断。
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn resolve(ctx: &Ctx, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        normalize(p)
    } else {
        normalize(&ctx.cwd.join(p))
    }
}

/// 工作区锁生效时，路径必须落在 cwd 或 `--add-dir` 之内。
fn guard(ctx: &Ctx, path: &Path) -> Result<(), String> {
    if !ctx.locked {
        return Ok(());
    }
    let inside = std::iter::once(&ctx.cwd)
        .chain(ctx.add_dirs.iter())
        .any(|root| path.starts_with(root));
    if inside {
        Ok(())
    } else {
        Err(format!(
            "Access denied: {} is outside the workspace ({})",
            path.display(),
            ctx.cwd.display()
        ))
    }
}

// ── Read ─────────────────────────────────────────────────────────

fn read(ctx: &Ctx, input: &Value) -> Result<String, String> {
    let path = resolve(ctx, &str_arg(input, "file_path")?);
    guard(ctx, &path)?;

    let meta = fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.is_dir() {
        return Err(format!(
            "{} is a directory — use Glob or Grep instead",
            path.display()
        ));
    }
    if meta.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "{} is too large ({} bytes, limit {MAX_TEXT_BYTES}) — read a slice with Bash instead",
            path.display(),
            meta.len()
        ));
    }

    let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();

    let offset = input
        .get("offset")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(MAX_READ_LINES as u64)
        .min(MAX_READ_LINES as u64) as usize;

    let start = (offset - 1).min(lines.len());
    let end = start.saturating_add(limit).min(lines.len());

    let mut out = String::new();
    for (i, line) in lines[start..end].iter().enumerate() {
        out.push_str(&format!("{}\t{}\n", start + i + 1, line));
    }
    if out.is_empty() {
        out = format!("(empty file: {} lines total)", lines.len());
    }
    Ok(truncate(out))
}

// ── Write ────────────────────────────────────────────────────────

fn write(ctx: &Ctx, input: &Value) -> Result<String, String> {
    if ctx.read_only {
        return Err("Write is disabled in plan mode (read-only)".into());
    }
    let path = resolve(ctx, &str_arg(input, "file_path")?);
    guard(ctx, &path)?;
    let content = str_arg(input, "content")?;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
    }
    fs::write(&path, content.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!(
        "Wrote {} bytes to {}",
        content.len(),
        path.display()
    ))
}

// ── Edit ─────────────────────────────────────────────────────────

fn edit(ctx: &Ctx, input: &Value) -> Result<String, String> {
    if ctx.read_only {
        return Err("Edit is disabled in plan mode (read-only)".into());
    }
    let path = resolve(ctx, &str_arg(input, "file_path")?);
    guard(ctx, &path)?;

    let old = str_arg(input, "old_string")?;
    if old.is_empty() {
        return Err("old_string must not be empty".into());
    }
    let new = str_arg(input, "new_string")?;
    let replace_all = input
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let count = text.matches(&old).count();
    if count == 0 {
        return Err(format!("old_string not found in {}", path.display()));
    }
    if count > 1 && !replace_all {
        return Err(format!(
            "old_string appears {count} times in {} — add more context or set replace_all=true",
            path.display()
        ));
    }

    let updated = if replace_all {
        text.replace(&old, &new)
    } else {
        text.replacen(&old, &new, 1)
    };
    fs::write(&path, updated.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!(
        "Edited {}: replaced {count} occurrence(s)",
        path.display()
    ))
}

// ── Bash ─────────────────────────────────────────────────────────

fn bash(ctx: &Ctx, input: &Value) -> Result<String, String> {
    if ctx.read_only {
        return Err("Bash is disabled in plan mode (read-only)".into());
    }
    let command = str_arg(input, "command")?;
    let timeout = input
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(BASH_TIMEOUT_MS)
        .min(BASH_MAX_TIMEOUT_MS);

    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(&command);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(&command);
        c
    };
    cmd.current_dir(&ctx.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // CREATE_NO_WINDOW —— GUI 宿主下不加会闪黑框
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }

    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    let out_pipe: Option<ChildStdout> = child.stdout.take();
    let err_pipe = child.stderr.take();
    // 必须并发读干管道，否则子进程写满缓冲区后会卡死
    let h_out = thread::spawn(move || drain(out_pipe));
    let h_err = thread::spawn(move || drain(err_pipe));

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if started.elapsed() > Duration::from_millis(timeout) {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("wait failed: {e}")),
        }
    };

    let stdout = h_out.join().unwrap_or_default();
    let stderr = h_err.join().unwrap_or_default();

    let mut out = String::new();
    if timed_out {
        out.push_str(&format!("(timed out after {timeout} ms — process killed)\n"));
    }
    match status.and_then(|s| s.code()) {
        Some(code) => out.push_str(&format!("exit code: {code}\n")),
        None if !timed_out => out.push_str("exit code: (none)\n"),
        None => {}
    }
    if !stdout.trim().is_empty() {
        out.push_str(&stdout);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    if !stderr.trim().is_empty() {
        out.push_str("[stderr]\n");
        out.push_str(&stderr);
    }
    if out.trim().is_empty() {
        out = "(no output)".into();
    }
    Ok(truncate(out))
}

/// 把管道读干（超上限后继续读但丢弃，防止子进程阻塞）
fn drain<R: Read>(pipe: Option<R>) -> String {
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if buf.len() < MAX_PIPE_BYTES {
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

// ── Glob ─────────────────────────────────────────────────────────

fn glob(ctx: &Ctx, input: &Value) -> Result<String, String> {
    let pattern = str_arg(input, "pattern")?;
    let base = match input.get("path").and_then(Value::as_str) {
        Some(p) if !p.trim().is_empty() => resolve(ctx, p),
        _ => ctx.cwd.clone(),
    };
    guard(ctx, &base)?;

    // glob crate 以 `/` 为分隔符、`\` 是转义符 —— 必须转成正斜杠
    let joined = if Path::new(&pattern).is_absolute() {
        pattern.clone()
    } else {
        format!("{}/{}", base.display(), pattern)
    }
    .replace('\\', "/");

    let opts = glob::MatchOptions {
        case_sensitive: false,
        require_literal_separator: true,
        require_literal_leading_dot: true,
    };
    let entries = glob::glob_with(&joined, opts).map_err(|e| format!("bad pattern: {e}"))?;

    let mut hits: Vec<String> = Vec::new();
    for entry in entries {
        if let Ok(p) = entry {
            if p.is_file() {
                hits.push(p.display().to_string());
                if hits.len() >= MAX_ENTRIES {
                    break;
                }
            }
        }
    }
    if hits.is_empty() {
        return Ok(format!("No files matched {pattern}"));
    }
    hits.sort();
    Ok(truncate(hits.join("\n")))
}

// ── Grep ─────────────────────────────────────────────────────────

fn grep(ctx: &Ctx, input: &Value) -> Result<String, String> {
    let pattern = str_arg(input, "pattern")?;
    let base = match input.get("path").and_then(Value::as_str) {
        Some(p) if !p.trim().is_empty() => resolve(ctx, p),
        _ => ctx.cwd.clone(),
    };
    guard(ctx, &base)?;

    let ignore_case = input
        .get("ignore_case")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let filter = input
        .get("glob")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);

    let re = regex::RegexBuilder::new(&pattern)
        .case_insensitive(ignore_case)
        .build()
        .map_err(|e| format!("bad regex: {e}"))?;

    let mut files: Vec<PathBuf> = Vec::new();
    if base.is_file() {
        files.push(base.clone());
    } else {
        walk(&base, 0, &mut files);
    }

    let mut hits: Vec<String> = Vec::new();
    'files: for file in files {
        if let Some(f) = &filter {
            if !filter_match(f, &file, &base) {
                continue;
            }
        }
        let Ok(meta) = fs::metadata(&file) else { continue };
        if meta.len() > MAX_TEXT_BYTES {
            continue;
        }
        let Ok(bytes) = fs::read(&file) else { continue };
        if is_binary(&bytes) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (i, line) in text.lines().enumerate() {
            if re.is_match(line) {
                hits.push(format!("{}:{}:{}", file.display(), i + 1, line.trim_end()));
                if hits.len() >= MAX_ENTRIES {
                    break 'files;
                }
            }
        }
    }

    if hits.is_empty() {
        return Ok(format!("No matches for {pattern}"));
    }
    Ok(truncate(hits.join("\n")))
}

/// 递归收集文本文件（跳过 SKIP_DIRS，深度与数量封顶）
fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_ENTRIES * 5 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(&path, depth + 1, out);
        } else if ft.is_file() {
            out.push(path);
        }
    }
}

/// 文件过滤器：含 `/` 的按相对路径匹配，否则只比对文件名
fn filter_match(filter: &str, path: &Path, base: &Path) -> bool {
    let filter = filter.replace('\\', "/");
    if filter.contains('/') {
        let rel = path
            .strip_prefix(base)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        glob::Pattern::new(&filter)
            .map(|p| p.matches(&rel))
            .unwrap_or(false)
    } else {
        let name = path.file_name().map(|s| s.to_string_lossy().to_string());
        match (glob::Pattern::new(&filter), name) {
            (Ok(p), Some(n)) => p.matches(&n),
            _ => false,
        }
    }
}

/// 前 8KB 有 NUL 字节即视为二进制
fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|&b| b == 0)
}

/// 单条工具结果上限（MCP 桥的结果也走这里，见 main.rs `run_tool`）
pub fn truncate(mut s: String) -> String {
    if s.chars().count() > MAX_RESULT_CHARS {
        let cut: String = s.chars().take(MAX_RESULT_CHARS).collect();
        s = format!("{cut}\n… (truncated at {MAX_RESULT_CHARS} chars)");
    }
    s
}
