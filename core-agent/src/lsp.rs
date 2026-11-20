//! LSP 诊断工具（2026-10-23，A13 剩余能力 `LSP`）：让模型在**改完文件后自己要看诊断**。
//!
//! 形态（与 `RepoMap` 同一条纪律）：**只读、本地、免审批、按需调用** —— 工具本身只
//! 「起一个本机语言服务器 → didOpen 一个文件 → 收回 publishDiagnostics」，不写用户文件、
//! 不发网络请求、不进系统提示词（工具描述在**请求体**里，固定前缀只多这一件工具）。
//!
//! **为什么是「起进程」而不是自己解析语法**：诊断必须与用户真实的编译器 / 语言服务器一致，
//! 自己写一套解析器只会给出第二套「大概对」的结论（同「同一个值只写一处」）。
//!
//! **失败一律 fail-open**：没装服务器 ⇒ `reason="no-server"`、扩展名不认识 ⇒
//! `"unsupported-extension"`、起不来 ⇒ `"spawn-failed"`、初始化失败 ⇒ `"initialize-failed"`、
//! 服务器中途退出 ⇒ `"server-exited"`、等不到推送 ⇒ `"timeout"`。
//! **绝不抛硬错误**（坏文件 / 坏环境不该中断模型的一轮），只有参数缺失才是 `Err`。
//!
//! ⚠️ **诚实边界**：
//!   · **只认 PATH 上的可执行文件**（Windows 额外试 `.exe` / `.cmd` / `.bat`）；用 nvm / 虚拟环境
//!     装在别处的服务器探测不到 ⇒ 回 `no-server`，不猜。rustup 的 `rust-analyzer.exe` 若只是
//!     个「工具链里没装该组件」的 shim，会表现为 `server-exited`（stderr 会带原因）。
//!   · **每次调用都新起一个服务器、结束时杀掉**：进程内不缓存（缓存要处理崩溃重启 / 配置变更 /
//!     索引失效，MVP 不做）。代价 = 冷启动慢（rust-analyzer 尤甚），故给了 `timeout` 入参。
//!   · **不是「全项目体检」**：只对**一个文件** didOpen，其余问题要模型逐个文件问。
//!   · `no-server` 时返回 `servers` 候选名，便于模型 / 用户知道该装什么。

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// 默认等待预算（毫秒）。冷启动的 rust-analyzer 在中等项目上常要 10–20 s。
const DEFAULT_TIMEOUT_MS: u64 = 20_000;
const MIN_TIMEOUT_MS: u64 = 1_000;
const MAX_TIMEOUT_MS: u64 = 60_000;
/// 单帧上限（防服务器回一个畸形 Content-Length 把内存打爆）。
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
/// 单条诊断消息的字符上限（模型侧够定位，不把整段模板字符串灌进上下文）。
const MAX_MESSAGE_CHARS: usize = 300;
/// 服务器 stderr 只留尾部这么多字符（用来说明「为什么起不来」）。
const STDERR_TAIL_CHARS: usize = 600;
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

/// 一个候选语言服务器：可执行名 + 启动参数。
struct ServerCmd {
    program: &'static str,
    args: &'static [&'static str],
}

/// 某种语言的配置：LSP `languageId` + 按优先级排列的服务器候选。
struct LangSpec {
    language_id: &'static str,
    servers: &'static [ServerCmd],
}

const TSSERVER: &[ServerCmd] = &[ServerCmd {
    program: "typescript-language-server",
    args: &["--stdio"],
}];
const PYRIGHT: &[ServerCmd] = &[
    ServerCmd {
        program: "pyright-langserver",
        args: &["--stdio"],
    },
    ServerCmd {
        program: "pylsp",
        args: &[],
    },
];
const RUST_ANALYZER: &[ServerCmd] = &[ServerCmd {
    program: "rust-analyzer",
    args: &[],
}];
const CLANGD: &[ServerCmd] = &[ServerCmd {
    program: "clangd",
    args: &[],
}];
const GOPLS: &[ServerCmd] = &[ServerCmd {
    program: "gopls",
    args: &[],
}];
const LUA_LS: &[ServerCmd] = &[ServerCmd {
    program: "lua-language-server",
    args: &[],
}];

/// 扩展名 → 语言配置（**小写、不带点**）。只收常见且「服务器安装方式稳定」的几种；
/// 不认识就老老实实回 `unsupported-extension`，不猜语言、不乱起进程。
fn lang_for(ext: &str) -> Option<LangSpec> {
    let spec = match ext {
        "rs" => LangSpec {
            language_id: "rust",
            servers: RUST_ANALYZER,
        },
        "ts" => LangSpec {
            language_id: "typescript",
            servers: TSSERVER,
        },
        "tsx" => LangSpec {
            language_id: "typescriptreact",
            servers: TSSERVER,
        },
        "js" => LangSpec {
            language_id: "javascript",
            servers: TSSERVER,
        },
        "jsx" => LangSpec {
            language_id: "javascriptreact",
            servers: TSSERVER,
        },
        "mjs" | "cjs" => LangSpec {
            language_id: "javascript",
            servers: TSSERVER,
        },
        "py" | "pyi" => LangSpec {
            language_id: "python",
            servers: PYRIGHT,
        },
        "go" => LangSpec {
            language_id: "go",
            servers: GOPLS,
        },
        "c" | "h" => LangSpec {
            language_id: "c",
            servers: CLANGD,
        },
        "cc" | "cpp" | "cxx" | "hpp" | "hxx" | "hh" => LangSpec {
            language_id: "cpp",
            servers: CLANGD,
        },
        "lua" => LangSpec {
            language_id: "lua",
            servers: LUA_LS,
        },
        _ => return None,
    };
    Some(spec)
}

/// 工具实现入口（由 `tools::run` 分派；参数缺失才返回 `Err`）。
pub fn run(ctx: &crate::tools::Ctx, input: &Value) -> Result<String, String> {
    let raw = input
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing required parameter \"file_path\"".to_string())?;
    // 路径解析与读类闸**复用 Read 那一套**（不新造第二套归一 / 越界判断）。
    let path = crate::tools::resolve_read_path(ctx, raw)?;
    let root = crate::tools::lsp_root(ctx);
    let timeout = Duration::from_millis(clamp_u64(
        input.get("timeout").and_then(Value::as_u64),
        DEFAULT_TIMEOUT_MS,
        MIN_TIMEOUT_MS,
        MAX_TIMEOUT_MS,
    ));
    let limit = clamp_u64(
        input.get("limit").and_then(Value::as_u64),
        DEFAULT_LIMIT as u64,
        1,
        MAX_LIMIT as u64,
    ) as usize;

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let spec = match lang_for(&ext) {
        Some(s) => s,
        None => {
            return Ok(report(
                &path,
                None,
                None,
                Vec::new(),
                0,
                "unsupported-extension",
                None,
                &[],
            ))
        }
    };

    let candidates: Vec<&str> = spec.servers.iter().map(|c| c.program).collect();
    let (program, args) = match find_server(&spec) {
        Some(x) => x,
        None => {
            return Ok(report(
                &path,
                Some(spec.language_id),
                None,
                Vec::new(),
                0,
                "no-server",
                Some("no language server found on PATH"),
                &candidates,
            ))
        }
    };

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            return Ok(report(
                &path,
                Some(spec.language_id),
                Some(&program),
                Vec::new(),
                0,
                "read-failed",
                Some(&format!("{e}")),
                &candidates,
            ))
        }
    };

    // ── 起服务器 + 跑一次最短会话 ────────────────────────────────
    let mut client = match ProcClient::spawn(&program, &args, &root) {
        Ok(c) => c,
        Err(e) => {
            return Ok(report(
                &path,
                Some(spec.language_id),
                Some(&program),
                Vec::new(),
                0,
                "spawn-failed",
                Some(&format!("{e}")),
                &candidates,
            ))
        }
    };
    let root_uri = to_file_uri(&root);
    let uri = to_file_uri(&path);
    let outcome = run_session(
        &mut client,
        &root_uri,
        &uri,
        spec.language_id,
        &text,
        timeout,
    );
    // 失败时把服务器 stderr 的尾巴带上 —— 否则「起不来」只有一句「exited」，用户无从下手。
    let stderr_tail = client.stderr_tail();
    match outcome {
        Ok(session) => {
            let (converted, total) = convert(&session.diagnostics, limit);
            let (reason, error) = if session.got_any {
                ("ok", None)
            } else {
                ("timeout", Some("no diagnostics were published before the timeout"))
            };
            Ok(report(
                &path,
                Some(spec.language_id),
                Some(&program),
                converted,
                total,
                reason,
                error,
                &candidates,
            ))
        }
        Err(SessionError::Initialize(detail)) => Ok(report(
            &path,
            Some(spec.language_id),
            Some(&program),
            Vec::new(),
            0,
            "initialize-failed",
            Some(&with_stderr(&detail, &stderr_tail)),
            &candidates,
        )),
        Err(SessionError::ServerExited) => Ok(report(
            &path,
            Some(spec.language_id),
            Some(&program),
            Vec::new(),
            0,
            "server-exited",
            Some(&with_stderr("the server closed the connection", &stderr_tail)),
            &candidates,
        )),
        Err(SessionError::Timeout) => Ok(report(
            &path,
            Some(spec.language_id),
            Some(&program),
            Vec::new(),
            0,
            "timeout",
            Some("the server did not publish diagnostics before the timeout"),
            &candidates,
        )),
    }
}

/// 失败详情 + stderr 尾巴（stderr 为空就原样返回详情）。
fn with_stderr(detail: &str, stderr: &str) -> String {
    if stderr.is_empty() {
        detail.to_string()
    } else {
        format!("{detail}; stderr: {stderr}")
    }
}

/// 组装工具返回的 JSON 文本（**恒为 JSON**，与 `CronList` / `TaskList` 同风格）。
fn report(
    path: &Path,
    language: Option<&str>,
    server: Option<&str>,
    diagnostics: Vec<Value>,
    total: usize,
    reason: &str,
    error: Option<&str>,
    candidates: &[&str],
) -> String {
    json!({
        "file": path.display().to_string(),
        "language": language,
        "server": server,
        "diagnostics": diagnostics,
        "total": total,
        "reason": reason,
        "error": error,
        "servers": candidates,
    })
    .to_string()
}

/// LSP 的 `Diagnostic` → 给模型看的扁平列表：**1 基行列**、按位置排序、截断消息与条数。
/// 返回（截断后的列表，截断前的条数）。
fn convert(diags: &[Value], limit: usize) -> (Vec<Value>, usize) {
    let total = diags.len();
    let mut out: Vec<Value> = diags
        .iter()
        .map(|d| {
            // LSP 的 line / character 都是 **0 基**；给人 / 给模型看一律换 1 基。
            let line = d
                .pointer("/range/start/line")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + 1;
            let column = d
                .pointer("/range/start/character")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + 1;
            let severity = match d.get("severity").and_then(Value::as_u64) {
                Some(1) => "error",
                Some(2) => "warning",
                Some(3) => "info",
                Some(4) => "hint",
                _ => "error",
            };
            let message = d.get("message").and_then(Value::as_str).unwrap_or("");
            let mut item = json!({
                "line": line,
                "column": column,
                "severity": severity,
                "message": truncate_chars(message, MAX_MESSAGE_CHARS),
            });
            if let Some(source) = d.get("source").and_then(Value::as_str) {
                item["source"] = json!(source);
            }
            if let Some(code) = d.get("code") {
                if !code.is_null() {
                    item["code"] = code.clone();
                }
            }
            item
        })
        .collect();
    out.sort_by_key(|d| {
        (
            d.get("line").and_then(Value::as_u64).unwrap_or(0),
            d.get("column").and_then(Value::as_u64).unwrap_or(0),
        )
    });
    out.truncate(limit);
    (out, total)
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

fn clamp_u64(v: Option<u64>, default: u64, min: u64, max: u64) -> u64 {
    v.unwrap_or(default).clamp(min, max)
}

// ── 服务器探测 ─────────────────────────────────────────────────

/// 按候选顺序在 PATH 上找第一个存在的可执行文件；找不到 ⇒ `None`（上层回 `no-server`）。
fn find_server(spec: &LangSpec) -> Option<(String, Vec<String>)> {
    for cand in spec.servers {
        if let Some(found) = find_on_path(cand.program) {
            return Some((
                found.display().to_string(),
                cand.args.iter().map(|s| s.to_string()).collect(),
            ));
        }
    }
    None
}

/// 在 PATH（+ Windows 的常见可执行后缀）里找可执行文件。
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let direct = dir.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        if cfg!(windows) {
            for ext in [".exe", ".cmd", ".bat", ".com"] {
                let with_ext = dir.join(format!("{name}{ext}"));
                if with_ext.is_file() {
                    return Some(with_ext);
                }
            }
        }
    }
    None
}

// ── 会话（与传输解耦：真进程 / 测试里的脚本化对端共用同一段逻辑）──────

/// 传输抽象：只做「发一条 JSON」与「在预算内收一条 JSON」。
///
/// 抽这一层**只为单测能跑**（本机不一定装了语言服务器）：真实现是子进程 stdio，
/// 测试实现是按脚本吐消息的假对端 —— 会话逻辑（握手指令、自动应答、等诊断）只有一份。
trait Transport {
    fn send(&mut self, msg: &Value) -> std::io::Result<()>;
    fn recv(&mut self, timeout: Duration) -> Incoming;
}

/// 收一条的结果：**超时与对端退出必须分开** —— 前者是 `timeout`、后者是 `server-exited`，
/// 混成一个 `None` 会让「没装组件」被误报成「太慢」。
enum Incoming {
    Msg(Value),
    Timeout,
    Closed,
}

struct Session {
    diagnostics: Vec<Value>,
    /// 有没有收到过 `publishDiagnostics`（**空列表也算收到** —— 文件本来就是干净的）。
    got_any: bool,
}

#[derive(Debug)]
enum SessionError {
    Initialize(String),
    ServerExited,
    Timeout,
}

/// 跑一次最短会话：`initialize → initialized → didOpen → 收诊断`。
fn run_session<T: Transport>(
    t: &mut T,
    root_uri: &str,
    uri: &str,
    language_id: &str,
    text: &str,
    timeout: Duration,
) -> Result<Session, SessionError> {
    let deadline = Instant::now() + timeout;
    t.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": Value::Null,
            "clientInfo": { "name": "lunac-agent", "version": env!("CARGO_PKG_VERSION") },
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": { "publishDiagnostics": { "relatedInformation": false } }
            },
            "workspaceFolders": [{ "uri": root_uri, "name": "workspace" }]
        }
    }))
    .map_err(|e| SessionError::Initialize(e.to_string()))?;

    // 初始化响应：等到 id=1 的 result / error（期间自动应答服务端发起的请求）。
    match wait_for(t, deadline, |v| {
        if v.get("id").and_then(Value::as_u64) == Some(1) && v.get("method").is_none() {
            Some(v.clone())
        } else {
            None
        }
    }) {
        Ok(v) if v.get("error").is_none() => {}
        Ok(v) => {
            let detail = v.get("error").map(|e| e.to_string()).unwrap_or_default();
            return Err(SessionError::Initialize(detail));
        }
        Err(WaitError::Timeout) => {
            return Err(SessionError::Initialize("initialize timed out".to_string()))
        }
        Err(WaitError::Closed) => return Err(SessionError::ServerExited),
    }

    let _ = t.send(&json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
    let _ = t.send(&json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": language_id,
                "version": 1,
                "text": text,
            }
        }
    }));

    // 等诊断：**空列表也接受**（文件本身就是干净的），但只在预算耗尽时才拿空列表收尾 ——
    // rust-analyzer 会先推一条空诊断、分析完再推真实结果，先到的空列表不代表「没有错误」。
    let mut latest: Option<Vec<Value>> = None;
    let mut got_any = false;
    let mut closed = false;
    loop {
        match wait_for(t, deadline, |v| {
            if v.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics") {
                Some(v.clone())
            } else {
                None
            }
        }) {
            Ok(msg) => {
                got_any = true;
                let diags = msg
                    .pointer("/params/diagnostics")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let non_empty = !diags.is_empty();
                latest = Some(diags);
                // 有实际诊断 ⇒ 立刻收（继续等只会等到同一个列表的重复推送）。
                if non_empty {
                    break;
                }
            }
            Err(WaitError::Closed) => {
                closed = true;
                break;
            }
            Err(WaitError::Timeout) => break,
        }
    }
    if !got_any {
        return Err(if closed {
            SessionError::ServerExited
        } else {
            SessionError::Timeout
        });
    }
    Ok(Session {
        diagnostics: latest.unwrap_or_default(),
        got_any,
    })
}

enum WaitError {
    Timeout,
    Closed,
}

/// 等一条**满足判据**的消息；期间自动应答服务端发起的请求（`client/registerCapability` /
/// `window/workDoneProgress/create` / `workspace/configuration` 等），否则多数服务器会一直
/// 等回执、不再推诊断。
fn wait_for<T: Transport, F>(
    t: &mut T,
    deadline: Instant,
    mut accept: F,
) -> Result<Value, WaitError>
where
    F: FnMut(&Value) -> Option<Value>,
{
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(WaitError::Timeout);
        }
        match t.recv(left) {
            Incoming::Msg(msg) => {
                // 服务端 → 客户端的**请求**：带 id 且带 method。先应答，再让判据看一眼。
                if msg.get("id").is_some() && msg.get("method").is_some() {
                    let _ = respond(t, &msg);
                    continue;
                }
                if let Some(hit) = accept(&msg) {
                    return Ok(hit);
                }
            }
            Incoming::Timeout => return Err(WaitError::Timeout),
            Incoming::Closed => return Err(WaitError::Closed),
        }
    }
}

/// 应答服务端请求。`workspace/configuration` 要按 items 数量回一个数组（回 `null` 有的
/// 服务器会当成「没配」再问一次）；其余一律 `result: null`。
fn respond<T: Transport>(t: &mut T, msg: &Value) -> std::io::Result<()> {
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let result = if method == "workspace/configuration" {
        let n = msg
            .pointer("/params/items")
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0);
        json!(vec![Value::Null; n])
    } else {
        Value::Null
    };
    t.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

// ── 真进程传输（LSP over stdio）────────────────────────────────

/// 一次调用的服务器句柄：stdout 由后台线程按帧读、经 channel 交给主线程（好做超时）；
/// stderr 也由后台线程收着，失败时把尾巴带给调用方（说明「为什么起不来」）。
struct ProcClient {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    stderr: std::sync::Arc<std::sync::Mutex<String>>,
}

impl ProcClient {
    fn spawn(program: &str, args: &[String], root: &Path) -> std::io::Result<Self> {
        let mut cmd = build_command(program);
        cmd.args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW —— GUI 宿主下不闪黑框
        }
        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "no stdout"))?;
        let stderr_pipe = child.stderr.take();

        let (tx, rx) = mpsc::channel::<Value>();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(Some(v)) = read_frame(&mut reader) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        });
        let stderr = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        if let Some(pipe) = stderr_pipe {
            let sink = stderr.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(pipe);
                let mut line = String::new();
                // 只保留尾部：服务器可能刷几千行日志，留全量既没用又占内存。
                while let Ok(n) = reader.read_line(&mut line) {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut s) = sink.lock() {
                        s.push_str(&line);
                        if s.chars().count() > STDERR_TAIL_CHARS * 2 {
                            let tail: String = s
                                .chars()
                                .rev()
                                .take(STDERR_TAIL_CHARS)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev()
                                .collect();
                            *s = tail;
                        }
                    }
                    line.clear();
                }
            });
        }
        Ok(Self {
            child,
            stdin,
            rx,
            stderr,
        })
    }

    fn stderr_tail(&self) -> String {
        self.stderr
            .lock()
            .map(|s| truncate_chars(s.trim(), STDERR_TAIL_CHARS))
            .unwrap_or_default()
    }
}

impl Transport for ProcClient {
    fn send(&mut self, msg: &Value) -> std::io::Result<()> {
        write_frame(&mut self.stdin, msg)
    }

    fn recv(&mut self, timeout: Duration) -> Incoming {
        match self.rx.recv_timeout(timeout) {
            Ok(v) => Incoming::Msg(v),
            Err(RecvTimeoutError::Timeout) => Incoming::Timeout,
            // 读线程结束 = 服务器退出 / 管道断裂（与「太慢」是两回事）。
            Err(RecvTimeoutError::Disconnected) => Incoming::Closed,
        }
    }
}

impl Drop for ProcClient {
    fn drop(&mut self) {
        // 尽量走协议收尾（shutdown / exit），失败就直接杀 —— 绝不留孤儿进程。
        let _ = self.send(&json!({ "jsonrpc": "2.0", "id": 9_999, "method": "shutdown", "params": Value::Null }));
        let _ = self.send(&json!({ "jsonrpc": "2.0", "method": "exit", "params": Value::Null }));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(windows)]
fn build_command(program: &str) -> Command {
    // Windows 上 npm 装的服务器（typescript-language-server / pyright-langserver）是 `.cmd`
    // 包装器，`Command::new` 不能直接执行 ⇒ 经 `cmd /C` 起；`.exe` 仍直连。
    let lower = program.to_ascii_lowercase();
    if lower.ends_with(".cmd") || lower.ends_with(".bat") {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(program);
        c
    } else {
        Command::new(program)
    }
}

#[cfg(not(windows))]
fn build_command(program: &str) -> Command {
    Command::new(program)
}

// ── LSP 帧编解码（`Content-Length: N\r\n\r\n{json}`）──────────────

fn read_frame<R: BufRead>(reader: &mut R) -> std::io::Result<Option<Value>> {
    let mut content_len: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None); // EOF
        }
        let trimmed = line.trim_end_matches(&['\r', '\n'][..]);
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_len = rest.trim().parse::<usize>().ok();
        }
    }
    let len = match content_len {
        Some(l) => l,
        None => return Ok(None),
    };
    if len > MAX_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(serde_json::from_slice(&buf).ok())
}

fn write_frame<W: Write>(writer: &mut W, msg: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

// ── 路径 → `file://` URI ───────────────────────────────────────

/// Windows 绝对路径 → `file:///D:/a%20b/x.rs`。
///
/// **不用 `canonicalize`**：它会返回 `\\?\D:\...` 的 verbatim 前缀，拼成 URI 后服务器认不出
/// （与宿主那个 `normalize_path` 的坑同源）。
fn to_file_uri(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('\\', "/");
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    let mut out = String::from("file://");
    for b in s.bytes() {
        let keep = b.is_ascii_alphanumeric() || matches!(b, b'/' | b':' | b'-' | b'_' | b'.' | b'~');
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::Cursor;

    /// 常见扩展名都要认得语言；不认识的必须回 `None`（上层据此回 `unsupported-extension`）。
    #[test]
    fn language_table_covers_common_files_only() {
        for (ext, id) in [
            ("rs", "rust"),
            ("ts", "typescript"),
            ("tsx", "typescriptreact"),
            ("js", "javascript"),
            ("jsx", "javascriptreact"),
            ("py", "python"),
            ("go", "go"),
            ("cpp", "cpp"),
            ("c", "c"),
            ("lua", "lua"),
        ] {
            let spec = lang_for(ext).unwrap_or_else(|| panic!("{ext} 应有语言配置"));
            assert_eq!(spec.language_id, id, "{ext} 的 languageId 不对");
            assert!(!spec.servers.is_empty(), "{ext} 应有至少一个服务器候选");
        }
        assert!(lang_for("txt").is_none(), "未知扩展名不得瞎猜语言");
        assert!(lang_for("").is_none());
    }

    /// `file://` URI：盘符保留、反斜杠转正斜杠、空格与中文按 UTF-8 百分号编码。
    #[test]
    fn file_uri_encodes_spaces_and_non_ascii() {
        let uri = to_file_uri(Path::new(r"D:\a b\测试.rs"));
        assert!(uri.starts_with("file:///D:/a%20b/"), "{uri}");
        assert!(uri.ends_with("/%E6%B5%8B%E8%AF%95.rs"), "{uri}");
        // 普通路径不被过度编码（`:` 与 `/` 保留，否则服务器认不出盘符）。
        assert_eq!(to_file_uri(Path::new(r"D:\code\x.rs")), "file:///D:/code/x.rs");
    }

    /// 帧编解码：写出去的必须能被自己读回来（header 大小写 / 长度计算都不许漂）。
    #[test]
    fn frame_roundtrip_keeps_the_payload() {
        let mut buf: Vec<u8> = Vec::new();
        write_frame(
            &mut buf,
            &json!({ "jsonrpc": "2.0", "id": 7, "result": { "ok": true } }),
        )
        .expect("write");
        assert!(buf.starts_with(b"Content-Length: "));
        let mut cur = Cursor::new(buf);
        let v = read_frame(&mut cur).expect("read").expect("有帧");
        assert_eq!(&v["id"], &json!(7));
        assert_eq!(v.pointer("/result/ok"), Some(&json!(true)));
        // 流结束 ⇒ Ok(None)（不是错误 —— 服务器正常退出）。
        assert!(read_frame(&mut cur).expect("eof").is_none());
    }

    /// 诊断转换：0 基 → 1 基、严重级映射、按位置排序、条数截断但 total 保真。
    #[test]
    fn diagnostics_are_one_based_sorted_and_limited() {
        let diags = vec![
            json!({ "range": { "start": { "line": 9, "character": 4 } }, "severity": 2, "message": "late" }),
            json!({ "range": { "start": { "line": 1, "character": 0 } }, "severity": 1, "message": "early", "source": "rustc", "code": "E0308" }),
            json!({ "range": { "start": { "line": 4, "character": 2 } }, "severity": 4, "message": "hint" }),
        ];
        let (out, total) = convert(&diags, 2);
        assert_eq!(total, 3, "total 必须是截断前的条数");
        assert_eq!(out.len(), 2, "limit 生效");
        assert_eq!(out[0]["line"], 2);
        assert_eq!(out[0]["column"], 1, "character 0 → column 1");
        assert_eq!(out[0]["severity"], "error");
        assert_eq!(out[0]["source"], "rustc");
        assert_eq!(out[0]["code"], "E0308");
        assert_eq!(out[1]["line"], 5);
        assert_eq!(out[1]["severity"], "hint");
        // 缺 severity 时按 error 报（宁严勿松，别让错误被当成噪音）。
        let (only, _) = convert(&[json!({ "range": { "start": {} }, "message": "m" })], 10);
        assert_eq!(only[0]["severity"], "error");
    }

    /// 超长消息要截断并带省略号（避免整段模板字符串灌进上下文）。
    #[test]
    fn long_messages_are_truncated() {
        let long = "x".repeat(MAX_MESSAGE_CHARS + 20);
        let (out, _) = convert(
            &[json!({ "range": { "start": { "line": 0, "character": 0 } }, "message": long })],
            10,
        );
        let msg = out[0]["message"].as_str().unwrap();
        assert!(
            msg.chars().count() <= MAX_MESSAGE_CHARS + 1,
            "{}",
            msg.chars().count()
        );
        assert!(msg.ends_with('…'));
    }

    /// 没装服务器时必须**返回 JSON 说明**（不抛错），且带上候选可执行名。
    #[test]
    fn missing_server_is_reported_as_json_not_an_error() {
        let path = Path::new(r"D:\tmp\solo.zzz");
        let text = report(
            path,
            None,
            None,
            Vec::new(),
            0,
            "no-server",
            Some("no language server found on PATH"),
            &["rust-analyzer"],
        );
        let v: Value = serde_json::from_str(&text).expect("必须是合法 JSON");
        assert_eq!(v["reason"], "no-server");
        assert_eq!(v["servers"][0], "rust-analyzer");
        assert_eq!(v["diagnostics"], json!([]));
    }

    // ── 会话逻辑（用脚本化对端跑，不依赖本机装没装语言服务器）────────

    /// 假对端：按脚本往下吐消息，同时记下我们发出去的所有帧。
    struct Mock {
        inbox: VecDeque<Value>,
        sent: Vec<Value>,
        /// 脚本吐完后是「对端超时」还是「对端退出」（两种失败语义必须各自可测）。
        timeout_when_empty: bool,
    }

    impl Mock {
        fn new(script: Vec<Value>) -> Self {
            Self {
                inbox: script.into(),
                sent: Vec::new(),
                timeout_when_empty: false,
            }
        }
        /// 吐完之后表现为「太慢」而不是「退出」。
        fn timing_out(mut self) -> Self {
            self.timeout_when_empty = true;
            self
        }
        fn methods(&self) -> Vec<String> {
            self.sent
                .iter()
                .filter_map(|m| m.get("method").and_then(Value::as_str).map(str::to_string))
                .collect()
        }
    }

    impl Transport for Mock {
        fn send(&mut self, msg: &Value) -> std::io::Result<()> {
            self.sent.push(msg.clone());
            Ok(())
        }
        fn recv(&mut self, _timeout: Duration) -> Incoming {
            match self.inbox.pop_front() {
                Some(v) => Incoming::Msg(v),
                None if self.timeout_when_empty => Incoming::Timeout,
                None => Incoming::Closed,
            }
        }
    }

    /// 正常链路：`initialize` 应答 → 服务端的 `workspace/configuration` 请求**必须被自动应答**
    /// （回等长数组）→ `publishDiagnostics` 被收下 → 会话返回诊断。
    #[test]
    fn session_handshakes_answers_server_requests_and_collects_diagnostics() {
        let mut mock = Mock::new(vec![
            // 服务端在 initialize 应答之前先发一个「要配置」的请求 —— 不自动应答就会死等。
            json!({ "jsonrpc": "2.0", "id": 100, "method": "workspace/configuration",
                     "params": { "items": [{}, {}] } }),
            json!({ "jsonrpc": "2.0", "id": 1, "result": { "capabilities": {} } }),
            json!({ "jsonrpc": "2.0", "method": "window/logMessage",
                     "params": { "type": 3, "message": "indexing" } }),
            json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                     "params": { "uri": "file:///D:/x.rs", "diagnostics": [
                        { "range": { "start": { "line": 2, "character": 4 } },
                          "severity": 1, "message": "boom" }
                     ] } }),
        ]);
        let out = run_session(
            &mut mock,
            "file:///D:/",
            "file:///D:/x.rs",
            "rust",
            "fn main() {}",
            Duration::from_secs(5),
        )
        .expect("会话应成功");

        assert!(out.got_any);
        assert_eq!(out.diagnostics.len(), 1);
        assert_eq!(out.diagnostics[0]["message"], "boom");

        let methods = mock.methods();
        assert_eq!(methods[0], "initialize", "第一条必须是 initialize");
        assert!(methods.contains(&"initialized".to_string()));
        assert!(methods.contains(&"textDocument/didOpen".to_string()));

        // 自动应答：必须是 id=100、且 result 是**两个 null** 的数组。
        let reply = mock
            .sent
            .iter()
            .find(|m| m.get("id") == Some(&json!(100)))
            .expect("必须应答服务端请求");
        assert_eq!(reply["result"], json!([null, null]));
    }

    /// 服务端在 initialize 阶段直接关连接 ⇒ 必须报 `ServerExited`（而不是默默超时），
    /// 这样调用方才能把 stderr 尾巴带给用户（「工具链里没这个组件」就是这么发现的）。
    #[test]
    fn session_reports_server_exit_during_initialize() {
        let mut mock = Mock::new(Vec::new()); // 一条也不吐 = 对端退出
        let err = run_session(
            &mut mock,
            "file:///D:/",
            "file:///D:/x.rs",
            "rust",
            "",
            Duration::from_secs(5),
        )
        .err()
        .expect("应失败");
        assert!(matches!(err, SessionError::ServerExited));
    }

    /// initialize 回了 error ⇒ 带出服务器给的原文（不是一句「失败」）。
    #[test]
    fn session_surfaces_initialize_error_payload() {
        let mut mock = Mock::new(vec![json!({
            "jsonrpc": "2.0", "id": 1,
            "error": { "code": -32002, "message": "no workspace" }
        })]);
        match run_session(
            &mut mock,
            "file:///D:/",
            "file:///D:/x.rs",
            "rust",
            "",
            Duration::from_secs(5),
        ) {
            Err(SessionError::Initialize(detail)) => {
                assert!(detail.contains("no workspace"), "{detail}")
            }
            _ => panic!("应报 initialize 失败"),
        }
    }

    /// 一条诊断都不推（服务器正常但一直沉默）⇒ `Timeout`。
    #[test]
    fn session_times_out_without_diagnostics() {
        let mut mock = Mock::new(vec![json!({ "jsonrpc": "2.0", "id": 1, "result": {} })])
            .timing_out();
        // 只给 1ms：初始化应答拿到之后，等诊断的那一步立刻到点。
        let err = run_session(
            &mut mock,
            "file:///D:/",
            "file:///D:/x.rs",
            "rust",
            "",
            Duration::from_millis(1),
        )
        .err()
        .expect("应超时");
        assert!(matches!(err, SessionError::Timeout));
    }

    /// **真机冒烟**（默认忽略）：用本机 `rust-analyzer` 跑一遍完整会话。
    ///
    /// 为什么手动跑：起真服务器要几十秒、还依赖本机装没装**可用**的 RA（rustup 的 shim
    /// 可能只是个空壳）⇒ 默认忽略，避免把常规测试拖慢 / 变脆。跑法：
    /// `cargo test --bins lsp_real_server_smoke -- --ignored --nocapture`
    #[test]
    #[ignore = "需要本机可用 rust-analyzer；手动跑（见注释里的命令）"]
    fn lsp_real_server_smoke() {
        let Some(ra) = find_on_path("rust-analyzer") else {
            eprintln!("skip: rust-analyzer 不在 PATH 上");
            return;
        };
        // rustup 的 shim 在「工具链里没装 rust-analyzer 组件」时会直接报错退出 —— 先探一下。
        let probe = Command::new(&ra).arg("--version").output();
        match probe {
            Ok(o) if o.status.success() => {}
            _ => {
                eprintln!("skip: {} 不可用（可能是没装组件的 rustup shim）", ra.display());
                return;
            }
        }
        // 造一个最小 cargo 工程（RA 需要可加载的工程；光放一个 .rs 它会当 detached file，
        // 分析深度因版本而异 —— 用真工程才能稳定拿到类型错误）。
        let dir = std::env::temp_dir().join(format!("lunac-lsp-smoke-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("mkdir");
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"lsp-smoke\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
        )
        .expect("cargo.toml");
        let file = dir.join("src").join("lib.rs");
        // 这句必然报 E0308（expected i32, found &str）。
        std::fs::write(&file, "pub fn f() -> i32 { \"not a number\" }\n").expect("lib.rs");

        let ctx = test_ctx(&dir);
        let out = run(
            &ctx,
            &json!({ "file_path": file.to_str().unwrap(), "timeout": 90_000 }),
        )
        .expect("LSP 不得抛硬错误");
        eprintln!("LSP smoke => {out}");
        let v: Value = serde_json::from_str(&out).expect("必须是 JSON");
        assert_eq!(v["reason"], "ok", "应拿到诊断（{out}）");
        assert!(
            v["total"].as_u64().unwrap_or(0) >= 1,
            "这段代码必然有错误：{out}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 最小 `Ctx`（与 `tools::tests::test_ctx` 同形；只给 LSP 用到的字段赋值）。
    fn test_ctx(cwd: &Path) -> crate::tools::Ctx {
        use std::sync::atomic::AtomicBool;
        use std::sync::{Arc, Mutex};
        crate::tools::Ctx {
            cwd: cwd.to_path_buf(),
            add_dirs: Vec::new(),
            read_only: false,
            locked: false,
            deny_read_dirs: Vec::new(),
            worktree: Arc::new(Mutex::new(None)),
            plan_phase: Arc::new(AtomicBool::new(false)),
            tool_output: None,
            shell_control: None,
            memory_scope: crate::tools::MemoryScope::Global,
            skill_scope: None,
        }
    }
}
