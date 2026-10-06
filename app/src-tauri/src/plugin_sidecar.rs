// src/plugin_sidecar.rs
//! **插件 sidecar**（L12 档 1，2026-10-06 定稿并实施）：让**磁盘插件**带自己的本机进程。
//!
//! 形态 = 「插件目录里带一个二进制，宿主负责起进程 + 通信 + 收尾，UI 仍在 WebView 里」。
//! 本仓内置插件（音乐 `librespot` / 转换 `ffmpeg` / OCR `PaddleOCR` / 代理 `mihomo`）
//! 早就在这么做，只是那份能力**编在宿主里**、磁盘插件够不着；这个模块把那套能力开放出来，
//! **不必再为某个插件改主程序**（「插件不依赖宿主代码」这条纪律的自然延伸）。
//!
//! ## 五条硬约束（对应 backlog L12 「sidecar 契约草案」的 ⑧/⑨）
//!
//! 1. **必须绑定面板**：起进程时记下调用方的窗口 label，该窗口 `Destroyed` ⇒ 收进程
//!    （与预检 #57 / `child_job.rs` 的既有纪律一致）。前端面板关闭时也会显式调 `stop`。
//! 2. **`cwd` / `command` 限制在插件目录内**（`plugin_market::safe_join`，与 `entry` / `dest` 同判据）。
//! 3. **宿主代请求**：`http` 通道由**宿主**去连 `127.0.0.1:<port>`，**插件 JS 自己从不碰端口**
//!    —— 前端 CSP 的 `default-src` 不含 `127.0.0.1`（`tauri.conf.json`），插件直接 `fetch()` 会被拦掉。
//! 4. **目标范围 = 数据 + 用户授权**：缺省只放行该插件**自己上报的那个端口**；要串本机其他应用
//!    必须在清单里声明 `allowLocalPorts` / `allowLocalAny`，并在**信任卡**上由用户批准。
//!    模型只能「提议」（写声明 / 拉起卡），**不能**自己把门推开（`plugin-trusted.json` 是唯一开关）。
//! 5. **不是沙箱、不许自称沙箱**：sidecar 是完整本机进程，信任卡是「如实告知 + 用户裁决」，不是隔离。
//!
//! ## 通道
//!
//! `stdio`（缺省）：宿主 ↔ 进程走 stdin/stdout 的 **NDJSON**（每行一个 JSON 对象）；
//! `http` / `both`：进程监听 `127.0.0.1:<port>`，**由宿主代请求**（`http()`）。
//! 起进程后必须先收到一条 `{"method":"ready", ...}`（`http` 时带 `port`）才算起好了，超时 10s。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use crate::plugin_market::{self, SidecarSpec};

/// 握手超时：起进程后等 `ready` 行。
const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// 单条 `request` 的默认超时。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Windows `CREATE_NO_WINDOW`：不弹控制台（stdio 管道照常可用）。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// 信任记录文件名（落在 `<exe 根>\config\`，与 `mcp-trusted.json` 同一处）。
const TRUST_FILE: &str = "plugin-trusted.json";
/// sidecar 主动推给插件面板的事件名。
pub const EVENT_NAME: &str = "plugin-sidecar-event";
/// sidecar 进程退出的事件名。
pub const EXIT_EVENT: &str = "plugin-sidecar-exit";

/// 起 sidecar 的结果。`status = "needs_trust"` 时前端要先弹信任卡，
/// `status = "started"` 时 `port` 是 http 通道的端口（stdio-only 时为 `None`）。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SidecarStart {
    pub status: String,
    pub plugin_id: String,
    pub port: Option<u16>,
    pub command: String,
    pub args: Vec<String>,
    pub sha256: String,
    pub transport: String,
    pub allow_local_ports: Vec<u16>,
    pub allow_local_any: bool,
}

/// 宿主代请求的 HTTP 回执（只回状态码 + 文本体，前端拿不到 socket）。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SidecarHttpReply {
    pub status: u16,
    pub body: String,
}

/// 一条活着的 sidecar 会话（按插件 id 唯一）。
struct Session {
    /// 宿主句柄（`restart = on-failure` 时用来自动静默重起）。
    app: AppHandle,
    /// 起它的那个窗口（面板）—— 该窗口销毁即收进程。
    owner_label: String,
    /// 清单里那一段（重起时原样复用）。
    spec: SidecarSpec,
    /// 插件目录（重起时原样复用）。
    dir: PathBuf,
    child: Child,
    stdin: ChildStdin,
    /// http 端口（stdio-only 时为 `None`）。
    port: Option<u16>,
    next_id: u64,
    /// 已发出、待回包的请求（id → 等待方）。
    pending: HashMap<u64, Sender<Result<Value, String>>>,
    /// 已重启次数（`restart = on-failure` 时用，上限 `MAX_RESTARTS`）。
    attempts: u32,
}

/// `on-failure` 的最大重启次数（超出即如实报错，不无限重试）。
const MAX_RESTARTS: u32 = 3;

/// 一次「起进程 + 等 ready」的产物（还没进 `sessions()` 表）。
struct Proc {
    child: Child,
    stdin: ChildStdin,
    port: Option<u16>,
}

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    static S: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 本会话内「仅本次信任」的指纹（不落盘）。
fn session_trusted() -> &'static Mutex<HashSet<String>> {
    static S: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 本会话内「已拒绝」的指纹（不落盘；拒绝后本会话内不再询问）。
fn denied() -> &'static Mutex<HashSet<String>> {
    static S: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashSet::new()))
}

// ── 信任门 ─────────────────────────────────────────────────────────────────

fn trust_path() -> PathBuf {
    crate::storage::lunac_root_dir()
        .join("config")
        .join(TRUST_FILE)
}

/// 读信任记录。**任何失败都当「没有记录」**（与 `.mcp.json` 那条路同一条纪律：
/// 读不出 / 坏掉一律按不信任，绝不反过来当已信任）。
fn load_trust() -> BTreeMap<String, Vec<String>> {
    let Ok(text) = std::fs::read_to_string(trust_path()) else {
        return BTreeMap::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn is_persistently_trusted(id: &str, fp: &str) -> bool {
    load_trust()
        .get(id)
        .map(|v| v.iter().any(|x| x == fp))
        .unwrap_or(false)
}

/// 指纹 = `sha256(id \x1f command \x1f sha256)` 的前 8 字节（16 hex）。
/// 覆盖所有会改变行为的字段：**命令一变 / 校验和一变 ⇒ 旧信任立即失效**（重新弹卡）。
pub fn fingerprint(plugin_id: &str, spec: &SidecarSpec) -> String {
    let mut h = Sha256::new();
    h.update(plugin_id.trim().as_bytes());
    h.update([0x1f]);
    h.update(spec.command.trim().as_bytes());
    h.update([0x1f]);
    h.update(spec.sha256.trim().as_bytes());
    hex(&h.finalize())[..16].to_string()
}

fn spec_for(plugin_id: &str) -> Result<(PathBuf, SidecarSpec), String> {
    let id = plugin_id.trim();
    if !plugin_market::is_safe_id(id) {
        return Err(format!("插件 id 非法：{plugin_id}"));
    }
    let dir = plugin_market::plugins_dir().join(id);
    if !dir.is_dir() {
        return Err(format!("插件不存在：{id}"));
    }
    let text = std::fs::read_to_string(dir.join(plugin_market::MANIFEST_FILE))
        .map_err(|e| format!("读不到插件清单：{e}"))?;
    let manifest = plugin_market::parse_manifest(&text)?;
    let spec = manifest
        .sidecar
        .ok_or_else(|| format!("插件 {id} 没有声明 sidecar"))?;
    Ok((dir, spec))
}

fn is_trusted(id: &str, spec: &SidecarSpec) -> bool {
    let fp = fingerprint(id, spec);
    is_persistently_trusted(id, &fp)
        || session_trusted()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&fp)
}

/// 记忆一次授权（`remember = true` 落盘；否则只在本会话内有效）。
pub fn grant(plugin_id: &str, remember: bool) -> Result<(), String> {
    let (_, spec) = spec_for(plugin_id)?;
    let fp = fingerprint(plugin_id.trim(), &spec);
    if remember {
        let mut map = load_trust();
        let entry = map.entry(plugin_id.trim().to_string()).or_default();
        if !entry.iter().any(|x| *x == fp) {
            entry.push(fp);
        }
        let path = trust_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("建 config 目录失败：{e}"))?;
        }
        let text = serde_json::to_string_pretty(&map).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| format!("写信任记录失败：{e}"))?;
        crate::log::info(format!("plugin_sidecar: 已信任 {plugin_id}（已落盘）"));
    } else {
        session_trusted()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fp);
        crate::log::info(format!("plugin_sidecar: 已信任 {plugin_id}（仅本次）"));
    }
    Ok(())
}

/// 记一次拒绝（本会话内不再询问）。
pub fn deny(plugin_id: &str) -> Result<(), String> {
    let (_, spec) = spec_for(plugin_id)?;
    let fp = fingerprint(plugin_id.trim(), &spec);
    denied()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(fp);
    crate::log::info(format!("plugin_sidecar: 用户拒绝 {plugin_id}"));
    Ok(())
}

// ── 启动 / 停止 ─────────────────────────────────────────────────────────────

fn started(id: &str, port: Option<u16>) -> SidecarStart {
    SidecarStart {
        status: "started".into(),
        plugin_id: id.to_string(),
        port,
        command: String::new(),
        args: Vec::new(),
        sha256: String::new(),
        transport: String::new(),
        allow_local_ports: Vec::new(),
        allow_local_any: false,
    }
}

/// 起 sidecar（幂等：已有活着的会话就直接复用）。
///
/// `owner_label` = 调用方窗口 label（**必须绑定面板**，见模块头 1）。
pub fn start(app: &AppHandle, owner_label: &str, plugin_id: &str) -> Result<SidecarStart, String> {
    let id = plugin_id.trim();
    let (dir, spec) = spec_for(id)?;
    let fp = fingerprint(id, &spec);

    if denied()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&fp)
    {
        return Err("已拒绝该 sidecar 启动（本会话内不再询问）".into());
    }
    if !is_trusted(id, &spec) {
        // 交给前端弹信任卡（如实列出 command / args / sha256 / 授权端口）
        return Ok(SidecarStart {
            status: "needs_trust".into(),
            plugin_id: id.to_string(),
            port: None,
            command: spec.command.clone(),
            args: spec.args.clone(),
            sha256: spec.sha256.clone(),
            transport: spec.transport.clone(),
            allow_local_ports: spec.allow_local_ports.clone(),
            allow_local_any: spec.allow_local_any,
        });
    }

    // 已有活着的会话 ⇒ 复用；已退出的 ⇒ 清掉
    {
        let mut g = sessions().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = g.get_mut(id) {
            if matches!(s.child.try_wait(), Ok(None)) {
                return Ok(started(id, s.port));
            }
            g.remove(id);
        }
    }

    let p = spawn_proc(app, owner_label, id, &spec, &dir)?;
    let port = p.port;
    sessions().lock().unwrap_or_else(|e| e.into_inner()).insert(
        id.to_string(),
        Session {
            app: app.clone(),
            owner_label: owner_label.to_string(),
            spec: spec.clone(),
            dir: dir.clone(),
            child: p.child,
            stdin: p.stdin,
            port,
            next_id: 1,
            pending: HashMap::new(),
            attempts: 0,
        },
    );
    let t = if spec.transport.trim().is_empty() {
        "stdio"
    } else {
        spec.transport.trim()
    };
    crate::log::info(format!(
        "plugin_sidecar: {id} 已启动（transport={t}, port={port:?}, owner={owner_label}）"
    ));
    Ok(started(id, port))
}

fn spawn_proc(
    app: &AppHandle,
    owner_label: &str,
    id: &str,
    spec: &SidecarSpec,
    dir: &Path,
) -> Result<Proc, String> {
    // command：插件目录内的相对路径（清单已校验，这里再走一遍 = 防清单与目录不一致）
    let exe = plugin_market::safe_join(dir, spec.command.trim())
        .ok_or_else(|| format!("sidecar.command 越界：{}", spec.command))?;
    if !exe.is_file() {
        return Err(format!("sidecar 可执行文件不存在：{}", exe.display()));
    }
    if !spec.sha256.trim().is_empty() {
        let got = file_sha256(&exe)?;
        if !got.eq_ignore_ascii_case(spec.sha256.trim()) {
            return Err(format!(
                "sidecar 校验和不匹配（期望 {}，实际 {got}）",
                spec.sha256.trim()
            ));
        }
    }
    let cwd = {
        let c = spec.cwd.trim();
        if c.is_empty() {
            dir.to_path_buf()
        } else {
            plugin_market::safe_join(dir, c)
                .ok_or_else(|| format!("sidecar.cwd 越界：{c}"))?
        }
    };

    let mut cmd = Command::new(&exe);
    cmd.args(&spec.args)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // env 只增不删（继承宿主环境）
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn().map_err(|e| format!("起 sidecar 失败：{e}"))?;
    // **spawn 之后立刻 assign**（预检 #57：宿主崩溃时由内核收掉，不留孤儿）
    crate::child_job::assign(&child);

    let stdin = child.stdin.take().ok_or("拿不到 sidecar stdin")?;
    let stdout = child.stdout.take().ok_or("拿不到 sidecar stdout")?;
    if let Some(se) = child.stderr.take() {
        let pid = id.to_string();
        std::thread::spawn(move || {
            for line in BufReader::new(se).lines().map_while(Result::ok) {
                let line = line.trim();
                if !line.is_empty() {
                    crate::log::warn(format!("plugin_sidecar[{pid}] stderr: {}", truncate(line, 400)));
                }
            }
        });
    }

    let (ready_tx, ready_rx) = mpsc::channel();
    spawn_reader(
        app.clone(),
        id.to_string(),
        owner_label.to_string(),
        stdout,
        ready_tx,
    );

    let ready = match ready_rx.recv_timeout(READY_TIMEOUT) {
        Ok(Ok(port)) => Ok(port),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("sidecar 握手超时（未收到 ready）".into()),
    };
    let ready_port = match ready {
        Ok(p) => p,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
    };

    let wants_http = matches!(spec.transport.trim(), "http" | "both");
    let port = if wants_http {
        match spec.port {
            0 => Some(ready_port.ok_or_else(|| {
                "transport 含 http，但 ready 行没给 port（且清单 port = 0）".to_string()
            })?),
            p => Some(p),
        }
    } else {
        None
    };

    Ok(Proc { child, stdin, port })
}

/// 会话活着检查 + `restart = on-failure` 的**惰性重起**（下一次调用时探测并重起；
/// 不做后台守护线程）。返回 `Err` 表示进程已死且不允许 / 已用尽重起次数。
fn ensure_alive(s: &mut Session, plugin_id: &str) -> Result<(), String> {
    if matches!(s.child.try_wait(), Ok(None)) {
        return Ok(());
    }
    if s.spec.restart.trim() != "on-failure" {
        return Err("sidecar 进程已退出（restart 非 on-failure，不自动重起）".into());
    }
    if s.attempts >= MAX_RESTARTS {
        return Err(format!("sidecar 进程已退出，且已重起 {MAX_RESTARTS} 次（放弃）"));
    }
    let backoff = Duration::from_millis(200 * (1u64 << s.attempts.min(4)));
    crate::log::warn(format!(
        "plugin_sidecar: {plugin_id} 进程已退出，{}ms 后重起（第 {} 次）",
        backoff.as_millis(),
        s.attempts + 1
    ));
    std::thread::sleep(backoff);
    s.attempts += 1;
    let p = spawn_proc(&s.app, &s.owner_label, plugin_id, &s.spec, &s.dir)?;
    s.child = p.child;
    s.stdin = p.stdin;
    s.port = p.port;
    Ok(())
}

/// stdout 读线程：解 NDJSON、分发回包 / 转推事件 / 处理握手。
fn spawn_reader(
    app: AppHandle,
    plugin_id: String,
    owner_label: String,
    stdout: std::process::ChildStdout,
    ready_tx: Sender<Result<Option<u16>, String>>,
) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(e) => {
                    crate::log::warn(format!(
                        "plugin_sidecar[{plugin_id}]: 非 JSON 行（{e}）：{}",
                        truncate(line, 200)
                    ));
                    continue;
                }
            };
            let id = v.get("id").and_then(Value::as_u64);
            let method = v.get("method").and_then(Value::as_str);
            if id.is_none() {
                if method == Some("ready") {
                    let port = v
                        .pointer("/params/port")
                        .and_then(Value::as_u64)
                        .map(|p| p as u16);
                    let _ = ready_tx.send(Ok(port));
                    continue;
                }
                // 主动推：原样转给插件面板
                emit_to(&app, &owner_label, EVENT_NAME, json!({ "pluginId": plugin_id, "payload": v }));
                continue;
            }
            let id = id.unwrap_or(0);
            let waiter = {
                let mut g = sessions().lock().unwrap_or_else(|e| e.into_inner());
                g.get_mut(&plugin_id).and_then(|s| s.pending.remove(&id))
            };
            match waiter {
                Some(tx) => {
                    let res = match v.get("error") {
                        Some(err) => Err(err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("sidecar error")
                            .to_string()),
                        None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    let _ = tx.send(res);
                }
                None => crate::log::warn(format!(
                    "plugin_sidecar[{plugin_id}]: 收到无对应请求的回包 id={id}"
                )),
            }
        }
        // EOF = 进程退出：唤醒所有等待方，通知面板
        let _ = ready_tx.send(Err("sidecar 进程未握手就退出了".into()));
        let waiters: Vec<Sender<Result<Value, String>>> = {
            let mut g = sessions().lock().unwrap_or_else(|e| e.into_inner());
            match g.get_mut(&plugin_id) {
                Some(s) => s.pending.drain().map(|(_, tx)| tx).collect(),
                None => Vec::new(),
            }
        };
        for tx in waiters {
            let _ = tx.send(Err("sidecar 进程已退出".into()));
        }
        emit_to(&app, &owner_label, EXIT_EVENT, json!({ "pluginId": plugin_id }));
    });
}

fn emit_to(app: &AppHandle, label: &str, event: &str, payload: Value) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.emit(event, payload);
    }
}

// ── 请求（stdio） / 代请求（http） ───────────────────────────────────────────

/// 走 stdio 发一条 NDJSON 请求并等回包。
pub fn request(plugin_id: &str, method: &str, params: Value) -> Result<Value, String> {
    let id = plugin_id.trim();
    let (tx, rx) = mpsc::channel();
    let req_id = {
        let mut g = sessions().lock().unwrap_or_else(|e| e.into_inner());
        let s = g
            .get_mut(id)
            .ok_or_else(|| format!("sidecar 未启动：{id}"))?;
        ensure_alive(s, id)?;
        let n = s.next_id;
        s.next_id += 1;
        s.pending.insert(n, tx);
        let line = json!({ "id": n, "method": method, "params": params }).to_string();
        if let Err(e) = writeln!(s.stdin, "{line}").and_then(|_| s.stdin.flush()) {
            s.pending.remove(&n);
            return Err(format!("写 sidecar stdin 失败：{e}"));
        }
        n
    };
    match rx.recv_timeout(REQUEST_TIMEOUT) {
        Ok(res) => res,
        Err(_) => {
            let mut g = sessions().lock().unwrap_or_else(|e| e.into_inner());
            if let Some(s) = g.get_mut(id) {
                s.pending.remove(&req_id);
            }
            Err(format!("sidecar 请求超时（{method}）"))
        }
    }
}

/// 端口是否被授权（纯函数，有单测）。
///
/// 缺省只放行**该插件自己上报的端口**；要串其他本机应用必须过 `allowLocalAny`
/// 或在 `allowLocalPorts` 里 —— 两者都是清单声明 + 用户信任卡批准的
/// （见模块头 4）。**模型不能自己把门推开。**
fn port_allowed(own: Option<u16>, allow_any: bool, allow_ports: &[u16], target: u16) -> bool {
    if Some(target) == own {
        return true;
    }
    if allow_any {
        return true;
    }
    allow_ports.contains(&target)
}

/// 由**宿主**代插件请求 `127.0.0.1:<port>`（插件 JS 自己从不碰端口 —— CSP）。
///
/// `port` 省略时用该插件自己 sidecar 的端口；给了就必须在授权范围内。
pub fn http(
    plugin_id: &str,
    method: &str,
    path: &str,
    body: Option<String>,
    port: Option<u16>,
) -> Result<SidecarHttpReply, String> {
    let id = plugin_id.trim();
    // path 必须是「以 / 开头的相对路径」：挡住 `http://evil` 之类的整串注入
    if !path.starts_with('/') || path.contains("://") || path.chars().any(char::is_control) {
        return Err(format!("sidecar.http 的 path 必须是以 / 开头的相对路径：{path}"));
    }
    let (_, spec) = spec_for(id)?;
    if !is_trusted(id, &spec) {
        return Err("该插件的 sidecar 尚未被信任（先调 sidecar.start 弹信任卡）".into());
    }
    let own = {
        let mut g = sessions().lock().unwrap_or_else(|e| e.into_inner());
        match g.get_mut(id) {
            Some(s) => {
                ensure_alive(s, id)?;
                s.port
            }
            None => None,
        }
    };
    let target = port
        .or(own)
        .ok_or_else(|| "sidecar 未提供 http 端口（transport 不含 http？）".to_string())?;
    if !port_allowed(own, spec.allow_local_any, &spec.allow_local_ports, target) {
        return Err(format!(
            "未授权访问本机端口 {target}（需在清单 sidecar.allowLocalPorts / allowLocalAny 声明并由用户信任）"
        ));
    }

    // 审计：每一次本机请求都留痕（「可审计」是这套授权模型优于放宽 CSP 的一条）
    crate::log::info(format!(
        "plugin_sidecar: {id} → http {method} 127.0.0.1:{target}{path}"
    ));

    let m = reqwest::Method::from_bytes(method.trim().to_uppercase().as_bytes())
        .map_err(|_| format!("非法 HTTP 方法：{method}"))?;
    let url = format!("http://127.0.0.1:{target}{path}");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("Client error: {e}"))?;
    let mut rb = client.request(m, &url);
    if let Some(b) = body {
        rb = rb.body(b);
    }
    let resp = rb.send().map_err(|e| format!("请求本机端口失败：{e}"))?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    Ok(SidecarHttpReply { status, body: text })
}

// ── 生命周期 ───────────────────────────────────────────────────────────────

fn kill(s: &mut Session) {
    let _ = s.child.kill();
    let _ = s.child.wait();
}

pub fn is_running(plugin_id: &str) -> bool {
    let mut g = sessions().lock().unwrap_or_else(|e| e.into_inner());
    match g.get_mut(plugin_id.trim()) {
        Some(s) => matches!(s.child.try_wait(), Ok(None)),
        None => false,
    }
}

/// 收掉某个插件的 sidecar（面板关闭 / 卸载 / 插件自调 stop）。
pub fn stop(plugin_id: &str) -> bool {
    let mut g = sessions().lock().unwrap_or_else(|e| e.into_inner());
    match g.remove(plugin_id.trim()) {
        Some(mut s) => {
            kill(&mut s);
            crate::log::info(format!("plugin_sidecar: {} 已停止", plugin_id.trim()));
            true
        }
        None => false,
    }
}

/// **面板关闭 ⇒ 收进程**（预检 #57 / 契约 ⑥）：按窗口 label 收掉它起的那些 sidecar。
pub fn on_plugin_window_destroyed(label: &str) {
    let mut g = sessions().lock().unwrap_or_else(|e| e.into_inner());
    let victims: Vec<String> = g
        .iter()
        .filter(|(_, s)| s.owner_label == label)
        .map(|(id, _)| id.clone())
        .collect();
    for id in victims {
        if let Some(mut s) = g.remove(&id) {
            kill(&mut s);
            crate::log::info(format!("plugin_sidecar: {id} 随窗口 {label} 关闭而停止"));
        }
    }
}

/// 宿主退出：全量收掉（`child_job` 的 Job Object 是崩溃路径的兜底，这里是正常路径）。
pub fn kill_all() {
    let mut g = sessions().lock().unwrap_or_else(|e| e.into_inner());
    for (_, mut s) in g.drain() {
        kill(&mut s);
    }
}

// ── 小工具 ─────────────────────────────────────────────────────────────────

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn file_sha256(p: &Path) -> Result<String, String> {
    let f = std::fs::File::open(p).map_err(|e| format!("打开可执行文件失败：{e}"))?;
    let mut r = BufReader::new(f);
    let mut h = Sha256::new();
    std::io::copy(&mut r, &mut h).map_err(|e| format!("读可执行文件失败：{e}"))?;
    Ok(hex(&h.finalize()))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

// ── Tauri 命令 ─────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn plugin_sidecar_start(
    app: AppHandle,
    window: WebviewWindow,
    plugin_id: String,
) -> Result<SidecarStart, String> {
    let owner = window.label().to_string();
    crate::commands::run_blocking(move || start(&app, &owner, &plugin_id)).await
}

#[tauri::command]
pub async fn plugin_sidecar_request(
    plugin_id: String,
    method: String,
    params: Option<Value>,
) -> Result<Value, String> {
    crate::commands::run_blocking(move || request(&plugin_id, &method, params.unwrap_or(Value::Null)))
        .await
}

#[tauri::command]
pub async fn plugin_sidecar_http(
    plugin_id: String,
    method: String,
    path: String,
    body: Option<String>,
    port: Option<u16>,
) -> Result<SidecarHttpReply, String> {
    crate::commands::run_blocking(move || http(&plugin_id, &method, &path, body, port)).await
}

#[tauri::command]
pub fn plugin_sidecar_stop(plugin_id: String) -> bool {
    stop(&plugin_id)
}

#[tauri::command]
pub fn plugin_sidecar_running(plugin_id: String) -> bool {
    is_running(&plugin_id)
}

#[tauri::command]
pub fn plugin_sidecar_trust(plugin_id: String, remember: bool) -> Result<(), String> {
    grant(&plugin_id, remember)
}

#[tauri::command]
pub fn plugin_sidecar_deny(plugin_id: String) -> Result<(), String> {
    deny(&plugin_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(command: &str, sha: &str) -> SidecarSpec {
        SidecarSpec {
            command: command.into(),
            sha256: sha.into(),
            ..Default::default()
        }
    }

    #[test]
    fn fingerprint_covers_command_and_sha() {
        let a = spec("bin/tool.exe", "");
        let b = spec("bin/other.exe", "");
        let c = spec("bin/tool.exe", "00");
        assert_eq!(fingerprint("p", &a), fingerprint("p", &a));
        assert_ne!(fingerprint("p", &a), fingerprint("p", &b), "command 变了指纹必须变");
        assert_ne!(fingerprint("p", &a), fingerprint("p", &c), "sha256 变了指纹必须变");
        assert_ne!(fingerprint("p", &a), fingerprint("q", &a), "id 变了指纹必须变");
        assert_eq!(fingerprint("p", &a).len(), 16);
    }

    #[test]
    fn port_authorization_is_fail_closed() {
        // 自己的端口永远放行
        assert!(port_allowed(Some(8080), false, &[], 8080));
        // 别人的端口：没声明就拒（即便有 own）
        assert!(!port_allowed(Some(8080), false, &[], 11434));
        // 白名单里有才放行
        assert!(port_allowed(Some(8080), false, &[11434], 11434));
        // allowAny = 全放行
        assert!(port_allowed(None, true, &[], 11434));
        // 没有 own、没有声明 ⇒ 拒
        assert!(!port_allowed(None, false, &[], 11434));
    }
}
