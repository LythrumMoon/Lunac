//! **插件 sidecar 运行时**（v1 `app/src-tauri/src/plugin_sidecar.rs` 的 v2 移植，
//! 2026-11-15）。
//!
//! ## 它解决什么
//!
//! v1 的插件是 TS + WebView；v2 不加载第三方 WebView（唯一例外是桌宠的 Live2D）。
//! 但「AI 开发的插件可以用任何框架 / 语言」这条要求不能丢 —— 落点就是 **sidecar**：
//! 插件目录里带自己的可执行文件（Python / Node / Go / Rust 都行），宿主负责
//! 「起进程 + 通信 + 收尾」，UI 由宿主画（插件运行时面板）。
//!
//! ## 契约（与 v1 完全一致，老清单可直接用）
//!
//! · 清单 `sidecar` 段见 [`crate::plugin_market::SidecarSpec`]，**必须同时声明能力
//!   `process.spawn`**；
//! · 沟通走 `stdio` 的 NDJSON（每行一个 JSON）与/或 `http`（进程监听 `127.0.0.1:<port>`，
//!   **由宿主代请求**，插件代码自己从不碰端口）；
//! · 握手行 `{"method":"ready","params":{"port":N}}`；`http` 传输还有第二个就绪信号
//!   = 清单写死的端口可连（第三方 http 服务不会说我们的握手行）；
//! · `request` 走 stdio：`{"id":N,"method":"…","params":…}` → `{"id":N,"result":…}` /
//!   `{"id":N,"error":{"message":"…"}}`；无 `id` 的行是主动推事件；
//! · **信任门**：`config/plugin-trusted.json`，指纹 = `sha256(id + command + sha256)`
//!   前 16 hex —— 命令或校验和一变，旧信任立即失效；
//! · 进程绑 Panel 生命周期（`owner`），`stop_owner` 一次收掉；spawn 之后立刻进
//!   Job Object（宿主崩溃时内核收掉，见 [`crate::child_job`]）；
//! · **不是沙箱**：sidecar 是完整本机进程，信任卡是「如实告知 + 用户裁决」，不是隔离。
//!
//! ## 与 v1 的差异（如实登记）
//!
//! · v1 用 Tauri 事件把 sidecar 主动推转给面板；v2 没有事件总线 —— 改成**轮询**
//!   [`events_json`]（面板每秒拉一次，带游标）。
//! · 内存 / 进程数限额：清单字段照 v1 解析并校验，Windows 上真的建独立 Job 生效；
//!   非 Windows 静默降级（与 v1 同）。
//! · **多了一条「sidecar → 宿主」的请求**（2026-11-15）：`kv.*` 让任意语言的插件进程
//!   用同一套 NDJSON 读写自己的插件存储（v1 是 WebView 里的 TS 直接 `invoke`）。
//!   作用域锁在**当前插件 id**；插件不要直接摸 `kv.json`（宿主写入有全局互斥）。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::child_job;
use crate::plugin_kv;
use crate::plugin_market::{self, SidecarSpec};

/// 单条 `request` 的默认超时。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// 宿主代请求（http 通道）的超时。
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
/// Windows `CREATE_NO_WINDOW`：不弹控制台（stdio 管道照常可用）。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// 信任记录文件名（落在 `<exe 根>\config\`）。
const TRUST_FILE: &str = "plugin-trusted.json";
/// `restart = on-failure` 的最大重启次数（超出即如实报错，不无限重试）。
const MAX_RESTARTS: u32 = 3;
/// 每个插件保留的事件条数（轮询用，超出丢最旧的）。
const MAX_EVENTS: usize = 200;
/// `request.method` 长度上限。
const MAX_METHOD_CHARS: usize = 128;

/// 一条活着的 sidecar 会话（按插件 id 唯一）。
struct Session {
    /// 起它的 `owner`（面板 / 窗口）—— `stop_owner` 据此收进程。
    owner: String,
    /// 数据根（`<exe 根>`）：重起与「sidecar → 宿主」的 kv 请求都要用它。
    root: PathBuf,
    /// 清单里那一段（重起时原样复用）。
    spec: SidecarSpec,
    /// 插件目录（重起时原样复用）。
    dir: PathBuf,
    child: Child,
    stdin: ChildStdin,
    /// 独立 job（只有清单配置了资源上限时才存在；`Drop` 关闭句柄即收掉残留子进程）。
    job: Option<child_job::Job>,
    /// http 端口（stdio-only 时为 `None`）。
    port: Option<u16>,
    next_id: u64,
    /// 已重启次数。
    attempts: u32,
}

/// 一次「起进程 + 等 ready」的产物（还没进 sessions 表）。
struct Proc {
    child: Child,
    stdin: ChildStdin,
    job: Option<child_job::Job>,
    port: Option<u16>,
}

/// 已发出、待回包的请求：插件 id → (请求 id → 等待方)。
type Pending = HashMap<String, HashMap<u64, Sender<Result<Value, String>>>>;

/// 轮询用的事件日志。
#[derive(Default)]
struct EventLog {
    cursor: u64,
    items: Vec<Value>,
}

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    static S: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pending() -> &'static Mutex<Pending> {
    static S: OnceLock<Mutex<Pending>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn logs() -> &'static Mutex<HashMap<String, EventLog>> {
    static S: OnceLock<Mutex<HashMap<String, EventLog>>> = OnceLock::new();
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

/// 最近一次退出原因（插件 id → 一句人话）。启动时清掉。
fn exits() -> &'static Mutex<HashMap<String, String>> {
    static S: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock<T>(m: &'static Mutex<T>) -> MutexGuard<'static, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ── 信任门 ────────────────────────────────────────────────────────────────

fn trust_path(root: &Path) -> PathBuf {
    root.join("config").join(TRUST_FILE)
}

/// 读信任记录。**任何失败都当「没有记录」**（读不出 / 坏掉一律按不信任）。
fn load_trust(root: &Path) -> BTreeMap<String, Vec<String>> {
    let Ok(text) = std::fs::read_to_string(trust_path(root)) else {
        return BTreeMap::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn is_persistently_trusted(root: &Path, id: &str, fp: &str) -> bool {
    load_trust(root)
        .get(id)
        .map(|v| v.iter().any(|x| x == fp))
        .unwrap_or(false)
}

/// 指纹 = `sha256(id \x1f command \x1f sha256)` 的前 8 字节（16 hex）。
///
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

fn spec_for(root: &Path, plugin_id: &str) -> Result<(PathBuf, SidecarSpec), String> {
    let id = plugin_id.trim();
    if !plugin_market::is_safe_id(id) {
        return Err(format!("插件 id 非法：{plugin_id}"));
    }
    let dir = plugin_market::resolve_plugin_dir(&plugin_market::plugins_dir(root), id)
        .ok_or_else(|| format!("插件不存在：{id}"))?;
    let text = std::fs::read_to_string(dir.join(plugin_market::MANIFEST_FILE))
        .map_err(|e| format!("读不到插件清单：{e}"))?;
    let manifest = plugin_market::parse_manifest(&text)?;
    let spec = manifest
        .sidecar
        .ok_or_else(|| format!("插件 {id} 没有声明 sidecar"))?;
    Ok((dir, spec))
}

fn is_trusted(root: &Path, id: &str, spec: &SidecarSpec) -> bool {
    let fp = fingerprint(id, spec);
    is_persistently_trusted(root, id, &fp) || lock(session_trusted()).contains(&fp)
}

/// 记忆一次授权（`remember = true` 落盘；否则只在本会话内有效）。
pub fn grant(root: &Path, plugin_id: &str, remember: bool) -> Result<(), String> {
    let (_, spec) = spec_for(root, plugin_id)?;
    let fp = fingerprint(plugin_id.trim(), &spec);
    if remember {
        let mut map = load_trust(root);
        let entry = map.entry(plugin_id.trim().to_string()).or_default();
        if !entry.iter().any(|x| *x == fp) {
            entry.push(fp);
        }
        let path = trust_path(root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("建 config 目录失败：{e}"))?;
        }
        let text = serde_json::to_string_pretty(&map).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| format!("写信任记录失败：{e}"))?;
    } else {
        lock(session_trusted()).insert(fp);
    }
    Ok(())
}

/// 记一次拒绝（本会话内不再询问）。
pub fn deny(root: &Path, plugin_id: &str) -> Result<(), String> {
    let (_, spec) = spec_for(root, plugin_id)?;
    let fp = fingerprint(plugin_id.trim(), &spec);
    lock(denied()).insert(fp);
    Ok(())
}

// ── 事件日志 ──────────────────────────────────────────────────────────────

fn push_event(plugin_id: &str, payload: Value) {
    let mut g = lock(logs());
    let log = g.entry(plugin_id.to_string()).or_default();
    log.cursor += 1;
    let seq = log.cursor;
    log.items.push(json!({ "seq": seq, "payload": payload }));
    if log.items.len() > MAX_EVENTS {
        log.items.remove(0);
    }
}

fn reset_log(plugin_id: &str) {
    lock(logs()).remove(plugin_id);
}

// ── 启动 / 停止 ───────────────────────────────────────────────────────────

fn started(id: &str, port: Option<u16>) -> Value {
    json!({
        "ok": true,
        "status": "started",
        "pluginId": id,
        "port": port,
        "command": "",
        "args": Vec::<String>::new(),
        "sha256": "",
        "transport": "",
        "allowLocalPorts": Vec::<u16>::new(),
        "allowLocalAny": false,
        "error": "",
    })
}

/// 起 sidecar（幂等：已有活着的会话就直接复用）。
///
/// `owner` = 调用方面板 / 窗口的标识（**必须绑定面板**）。
pub fn start(root: &Path, owner: &str, plugin_id: &str) -> Value {
    let id = plugin_id.trim();
    let (dir, spec) = match spec_for(root, id) {
        Ok(v) => v,
        Err(reason) => {
            return json!({ "ok": false, "status": "", "pluginId": id, "port": null, "error": reason })
        }
    };
    let fp = fingerprint(id, &spec);
    if lock(denied()).contains(&fp) {
        return json!({
            "ok": false,
            "status": "",
            "pluginId": id,
            "port": null,
            "error": "已拒绝该 sidecar 启动（本会话内不再询问）",
        });
    }
    if !is_trusted(root, id, &spec) {
        // 交给面板弹信任卡（如实列出 command / args / sha256 / 授权端口）。
        return json!({
            "ok": true,
            "status": "needs_trust",
            "pluginId": id,
            "port": null,
            "command": spec.command,
            "args": spec.args,
            "sha256": spec.sha256,
            "transport": spec.transport,
            "allowLocalPorts": spec.allow_local_ports,
            "allowLocalAny": spec.allow_local_any,
            "error": "",
        });
    }

    // 已有活着的会话 ⇒ 复用；已退出的 ⇒ 清掉
    {
        let mut g = lock(sessions());
        if let Some(s) = g.get_mut(id) {
            if matches!(s.child.try_wait(), Ok(None)) {
                return started(id, s.port);
            }
            g.remove(id);
        }
    }

    // 新一轮启动：先清掉上一轮的退出原因 / 事件 / 待回包，避免把旧日志带给面板。
    lock(exits()).remove(id);
    lock(pending()).remove(id);
    reset_log(id);

    match spawn_proc(root, id, &spec, &dir) {
        Ok(p) => {
            let port = p.port;
            let transport = if spec.transport.trim().is_empty() {
                "stdio".to_string()
            } else {
                spec.transport.trim().to_string()
            };
            eprintln!("[lunac-plugin] {id} 已启动（transport={transport}, port={port:?}, owner={owner}）");
            lock(sessions()).insert(
                id.to_string(),
                Session {
                    owner: owner.to_string(),
                    root: root.to_path_buf(),
                    spec,
                    dir,
                    child: p.child,
                    stdin: p.stdin,
                    job: p.job,
                    port,
                    next_id: 1,
                    attempts: 0,
                },
            );
            started(id, port)
        }
        Err(reason) => json!({
            "ok": false,
            "status": "",
            "pluginId": id,
            "port": null,
            "error": reason,
        }),
    }
}

fn spawn_proc(root: &Path, id: &str, spec: &SidecarSpec, dir: &Path) -> Result<Proc, String> {
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
            plugin_market::safe_join(dir, c).ok_or_else(|| format!("sidecar.cwd 越界：{c}"))?
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
    // **spawn 之后立刻绑定生命周期**：配置了资源上限 ⇒ 独立 job；否则回落全局 job。
    let limits = child_job::JobLimits {
        process_memory_limit: spec.memory_limit_mib as usize * 1024 * 1024,
        job_memory_limit: 0,
        active_process_limit: spec.process_limit,
    };
    let mut job = if limits.is_set() {
        child_job::Job::create(limits)
    } else {
        None
    };
    if let Some(j) = &job {
        if j.assign(&child) {
            eprintln!(
                "[lunac-plugin] {id} 使用独立 job（memory={} MiB, processes={}）",
                spec.memory_limit_mib, spec.process_limit
            );
        } else {
            eprintln!("[lunac-plugin] {id} 独立 job 分配失败，回落全局 job（资源限额未生效）");
            job = None;
        }
    }
    if job.is_none() {
        child_job::assign(&child);
    }

    let stdin = child.stdin.take().ok_or("拿不到 sidecar stdin")?;
    let stdout = child.stdout.take().ok_or("拿不到 sidecar stdout")?;
    if let Some(se) = child.stderr.take() {
        let pid = id.to_string();
        std::thread::spawn(move || {
            for line in BufReader::new(se).lines().map_while(Result::ok) {
                let line = line.trim();
                if !line.is_empty() {
                    eprintln!("[lunac-plugin][{pid}] stderr: {}", truncate(line, 400));
                }
            }
        });
    }

    let (ready_tx, ready_rx) = mpsc::channel();
    spawn_reader(id.to_string(), root.to_path_buf(), BufReader::new(stdout), ready_tx);

    let ready_port = match wait_ready(spec, &ready_rx) {
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

    Ok(Proc {
        child,
        stdin,
        job,
        port,
    })
}

/// 等「就绪」。**两个信号任一成立即可**（见模块头）：
///   ① 进程吐出 NDJSON `{"method":"ready"}` 行；
///   ② `transport` 含 http **且清单写了固定 `port`** 时，该端口**可连** ——
///      第三方 http 服务（`llama-server` 等）不会说我们的握手行。
fn wait_ready(
    spec: &SidecarSpec,
    ready_rx: &mpsc::Receiver<Result<Option<u16>, String>>,
) -> Result<Option<u16>, String> {
    let wants_http = matches!(spec.transport.trim(), "http" | "both");
    let probe_port = wants_http && spec.port != 0;
    let wait_secs = spec.ready_timeout_sec.max(1);
    let deadline = Instant::now() + Duration::from_secs(wait_secs);
    loop {
        match ready_rx.try_recv() {
            Ok(Ok(p)) => return Ok(p),
            Ok(Err(e)) => return Err(e),
            Err(TryRecvError::Disconnected) => return Err("sidecar 进程未握手就退出了".into()),
            Err(TryRecvError::Empty) => {}
        }
        if probe_port && tcp_port_open(spec.port) {
            return Ok(Some(spec.port));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "sidecar 未就绪（{wait_secs}s 内既没收到 ready 行，端口 {} 也没起来）",
                spec.port
            ));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// 回环端口是否已在监听（就绪探测用）。**只连 `127.0.0.1`**，不发请求。
fn tcp_port_open(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(300),
    )
    .is_ok()
}

/// 会话活着检查 + `restart = on-failure` 的惰性重起（下一次调用时探测并重起）。
fn ensure_alive(s: &mut Session, plugin_id: &str) -> Result<(), String> {
    if matches!(s.child.try_wait(), Ok(None)) {
        return Ok(());
    }
    if s.spec.restart.trim() != "on-failure" {
        return Err("sidecar 进程已退出（restart 非 on-failure，不自动重起）".into());
    }
    if s.attempts >= MAX_RESTARTS {
        return Err(format!(
            "sidecar 进程已退出，且已重起 {MAX_RESTARTS} 次（放弃）"
        ));
    }
    let backoff = Duration::from_millis(200 * (1u64 << s.attempts.min(4)));
    eprintln!(
        "[lunac-plugin] {plugin_id} 进程已退出，{}ms 后重起（第 {} 次）",
        backoff.as_millis(),
        s.attempts + 1
    );
    std::thread::sleep(backoff);
    s.attempts += 1;
    let p = spawn_proc(&s.root, plugin_id, &s.spec, &s.dir)?;
    s.child = p.child;
    s.stdin = p.stdin;
    s.job = p.job; // 旧 job 句柄在这里被 drop（关闭即收掉旧进程树的残留）
    s.port = p.port;
    lock(exits()).remove(plugin_id);
    Ok(())
}

/// stdout 读线程：解 NDJSON、分发回包 / 记录事件 / 处理握手 / 应答宿主请求。
///
/// 抽成 `impl BufRead` 是为了单测能直接喂行（不真的起进程）。
///
/// **sidecar → 宿主**的请求（`{"id":N,"method":"kv.get",…}`）在这一层答：宿主必须自己
/// 序列化插件存储的读写（`plugin_kv` 的全局互斥），让插件进程直接摸文件会与宿主写冲突。
fn spawn_reader<R: BufRead + Send + 'static>(
    plugin_id: String,
    root: PathBuf,
    reader: R,
    ready_tx: Sender<Result<Option<u16>, String>>,
) {
    std::thread::spawn(move || {
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "[lunac-plugin][{plugin_id}] 非 JSON 行（{e}）：{}",
                        truncate(line, 200)
                    );
                    continue;
                }
            };
            let id = v.get("id").and_then(Value::as_u64);
            let method = v.get("method").and_then(Value::as_str);
            // ① 带 id + method = sidecar 在**问宿主**（目前只开放 kv.*）；宿主答完继续。
            if let (Some(request_id), Some(method)) = (id, method) {
                let reply = host_request(&root, &plugin_id, request_id, method, v.get("params"));
                write_to_session(&plugin_id, &reply);
                continue;
            }
            if id.is_none() {
                if method == Some("ready") {
                    let port = v
                        .pointer("/params/port")
                        .and_then(Value::as_u64)
                        .map(|p| p as u16);
                    let _ = ready_tx.send(Ok(port));
                    continue;
                }
                // 主动推：进事件日志，面板轮询时取走。
                push_event(&plugin_id, v);
                continue;
            }
            let id = id.unwrap_or(0);
            let waiter = lock(pending())
                .get_mut(&plugin_id)
                .and_then(|m| m.remove(&id));
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
                None => eprintln!("[lunac-plugin][{plugin_id}] 收到无对应请求的回包 id={id}"),
            }
        }
        // EOF = 进程退出：唤醒所有等待方，记退出原因。
        let _ = ready_tx.send(Err("sidecar 进程未握手就退出了".into()));
        let waiters: Vec<Sender<Result<Value, String>>> = lock(pending())
            .get_mut(&plugin_id)
            .map(|m| m.drain().map(|(_, tx)| tx).collect())
            .unwrap_or_default();
        for tx in waiters {
            let _ = tx.send(Err("sidecar 进程已退出".into()));
        }
        lock(exits()).insert(plugin_id.clone(), "sidecar 进程已退出".to_string());
        push_event(&plugin_id, json!({ "method": "exit" }));
    });
}

/// 应答 sidecar 的宿主请求（目前只开放**插件存储** `kv.*`）。
///
/// 作用域 = **当前插件 id**：插件进程只能读写自己的 KV（`plugin_kv` 按 id 分文件），
/// 拿不到别人的；写操作由宿主串行（`plugin_kv` 内部一把全局互斥），插件不要直接摸文件。
fn host_request(
    root: &Path,
    plugin_id: &str,
    id: u64,
    method: &str,
    params: Option<&Value>,
) -> Value {
    let ok = |result: Value| json!({ "id": id, "result": result });
    let fail = |message: String| json!({ "id": id, "error": { "message": message } });
    let key = || {
        params
            .and_then(|p| p.get("key"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|k| !k.is_empty())
    };
    match method {
        "kv.get" => {
            let Some(key) = key() else {
                return fail("kv.get 缺少 params.key".to_string());
            };
            match host_reply(plugin_kv::get_json(root, plugin_id, key)) {
                Ok(value) => ok(json!({
                    "found": value.get("found").cloned().unwrap_or(Value::Bool(false)),
                    "value": value.get("value").cloned().unwrap_or(Value::Null),
                })),
                Err(reason) => fail(reason),
            }
        }
        "kv.set" => {
            let Some(key) = key() else {
                return fail("kv.set 缺少 params.key".to_string());
            };
            let Some(value) = params.and_then(|p| p.get("value")) else {
                return fail("kv.set 缺少 params.value".to_string());
            };
            match host_reply(plugin_kv::set_json(
                root,
                &json!({ "id": plugin_id, "key": key, "value": value }),
            )) {
                Ok(value) => ok(json!({
                    "bytes": value.get("bytes").cloned().unwrap_or(Value::Null),
                })),
                Err(reason) => fail(reason),
            }
        }
        "kv.delete" => {
            let Some(key) = key() else {
                return fail("kv.delete 缺少 params.key".to_string());
            };
            match host_reply(plugin_kv::delete_json(root, plugin_id, key)) {
                Ok(value) => ok(json!({
                    "removed": value.get("removed").cloned().unwrap_or(Value::Bool(false)),
                })),
                Err(reason) => fail(reason),
            }
        }
        "kv.list" => match host_reply(plugin_kv::list_json(root, plugin_id)) {
            Ok(value) => ok(json!({
                "entries": value.get("entries").cloned().unwrap_or(Value::Null),
                "bytes": value.get("bytes").cloned().unwrap_or(Value::Null),
            })),
            Err(reason) => fail(reason),
        },
        "kv.clear" => match host_reply(plugin_kv::clear_json(root, plugin_id)) {
            Ok(value) => ok(json!({
                "bytes": value.get("bytes").cloned().unwrap_or(Value::Null),
            })),
            Err(reason) => fail(reason),
        },
        other => fail(format!(
            "未知的宿主方法「{other}」：只支持 kv.get / kv.set / kv.delete / kv.list / kv.clear"
        )),
    }
}

/// 宿主回执（`{ok, error}`）→ `Ok(整份回执)` / `Err(一句人话)`。
fn host_reply(raw: String) -> Result<Value, String> {
    let value: Value = serde_json::from_str(&raw).map_err(|_| "宿主回执解析失败".to_string())?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(value);
    }
    Err(value
        .get("error")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .unwrap_or("宿主拒绝了这个操作")
        .to_string())
}

/// 把回执写回 sidecar 的 stdin（会话已不在就静默丢弃 —— 读线程此时本就在收尾）。
fn write_to_session(plugin_id: &str, message: &Value) {
    let mut guard = match sessions().lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let Some(session) = guard.get_mut(plugin_id) else {
        return;
    };
    if let Err(error) = writeln!(session.stdin, "{message}").and_then(|_| session.stdin.flush()) {
        eprintln!("[lunac-plugin][{plugin_id}] 回 sidecar 失败：{error}");
    }
}

// ── 请求（stdio）/ 代请求（http） ─────────────────────────────────────────
/// 走 stdio 发一条 NDJSON 请求并等回包。
pub fn request(plugin_id: &str, method: &str, params: Value) -> Result<Value, String> {
    let id = plugin_id.trim();
    let method = method.trim();
    if method.is_empty() || method.chars().count() > MAX_METHOD_CHARS {
        return Err("sidecar 请求的 method 为空或过长".into());
    }
    if method.chars().any(char::is_control) {
        return Err("sidecar 请求的 method 含控制字符".into());
    }
    let (tx, rx) = mpsc::channel();
    let req_id = {
        let mut g = lock(sessions());
        let s = g
            .get_mut(id)
            .ok_or_else(|| format!("sidecar 未启动：{id}"))?;
        ensure_alive(s, id)?;
        let n = s.next_id;
        s.next_id += 1;
        lock(pending())
            .entry(id.to_string())
            .or_default()
            .insert(n, tx);
        let line = json!({ "id": n, "method": method, "params": params }).to_string();
        if let Err(e) = writeln!(s.stdin, "{line}").and_then(|_| s.stdin.flush()) {
            if let Some(m) = lock(pending()).get_mut(id) {
                m.remove(&n);
            }
            return Err(format!("写 sidecar stdin 失败：{e}"));
        }
        n
    };
    match rx.recv_timeout(REQUEST_TIMEOUT) {
        Ok(res) => res,
        Err(_) => {
            if let Some(m) = lock(pending()).get_mut(id) {
                m.remove(&req_id);
            }
            Err(format!("sidecar 请求超时（{method}）"))
        }
    }
}

/// 端口是否被授权（纯函数，有单测）。
///
/// 缺省只放行**该插件自己上报的端口**；要串其他本机应用必须过 `allowLocalAny`
/// 或在 `allowLocalPorts` 里 —— 两者都是清单声明 + 用户信任卡批准的。
fn port_allowed(own: Option<u16>, allow_any: bool, allow_ports: &[u16], target: u16) -> bool {
    if Some(target) == own {
        return true;
    }
    if allow_any {
        return true;
    }
    allow_ports.contains(&target)
}

/// 由**宿主**代插件请求 `127.0.0.1:<port>`（插件代码自己从不碰端口）。
///
/// `port = None` 时用该插件自己 sidecar 的端口；给了就必须在授权范围内。
pub fn http(
    root: &Path,
    plugin_id: &str,
    method: &str,
    path: &str,
    body: Option<String>,
    port: Option<u16>,
) -> Result<(u16, String), String> {
    let id = plugin_id.trim();
    // path 必须是「以 / 开头的相对路径」：挡住 `http://evil` 之类的整串注入。
    if !path.starts_with('/') || path.contains("://") || path.chars().any(char::is_control) {
        return Err(format!(
            "sidecar.http 的 path 必须是以 / 开头的相对路径：{path}"
        ));
    }
    let (_, spec) = spec_for(root, id)?;
    if !is_trusted(root, id, &spec) {
        return Err("该插件的 sidecar 尚未被信任（先调 start 弹信任卡）".into());
    }
    let own = {
        let mut g = lock(sessions());
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

    // 审计：每一次本机请求都留痕（「可审计」是这套授权模型优于放宽 CSP 的一条）。
    eprintln!("[lunac-plugin] {id} → http {method} 127.0.0.1:{target}{path}");

    let m = reqwest::Method::from_bytes(method.trim().to_uppercase().as_bytes())
        .map_err(|_| format!("非法 HTTP 方法：{method}"))?;
    let url = format!("http://127.0.0.1:{target}{path}");
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("Client error: {e}"))?;
    let mut rb = client.request(m, &url);
    if let Some(b) = body {
        rb = rb.body(b);
    }
    let resp = rb.send().map_err(|e| format!("请求本机端口失败：{e}"))?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    Ok((status, text))
}

// ── 生命周期 ──────────────────────────────────────────────────────────────

fn kill(s: &mut Session) {
    let _ = s.child.kill();
    let _ = s.child.wait();
    // 关闭独立 job 的句柄 —— KILL_ON_JOB_CLOSE 会收掉 sidecar 自己 fork 出的残留进程。
    s.job.take();
}

pub fn is_running(plugin_id: &str) -> bool {
    let mut g = lock(sessions());
    match g.get_mut(plugin_id.trim()) {
        Some(s) => matches!(s.child.try_wait(), Ok(None)),
        None => false,
    }
}

/// 收掉某个插件的 sidecar（面板关闭 / 卸载 / 插件自调 stop）。
pub fn stop(plugin_id: &str) -> bool {
    let mut g = lock(sessions());
    match g.remove(plugin_id.trim()) {
        Some(mut s) => {
            kill(&mut s);
            eprintln!("[lunac-plugin] {} 已停止", plugin_id.trim());
            true
        }
        None => false,
    }
}

/// **面板关闭 ⇒ 收进程**：按 `owner` 收掉它起的那些 sidecar。
pub fn stop_owner(owner: &str) -> usize {
    let mut g = lock(sessions());
    let victims: Vec<String> = g
        .iter()
        .filter(|(_, s)| s.owner == owner)
        .map(|(id, _)| id.clone())
        .collect();
    let mut count = 0;
    for id in victims {
        if let Some(mut s) = g.remove(&id) {
            kill(&mut s);
            count += 1;
            eprintln!("[lunac-plugin] {id} 随 owner={owner} 关闭而停止");
        }
    }
    count
}

/// 宿主退出：全量收掉。
pub fn kill_all() {
    let mut g = lock(sessions());
    for (_, mut s) in g.drain() {
        kill(&mut s);
    }
}

// ── 小工具 ────────────────────────────────────────────────────────────────

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

// ── 对外 JSON 口子（Dart 侧只认 JSON） ────────────────────────────────────

/// 面板状态：清单 / 信任 / 运行 / 退出原因 / 事件游标。
pub fn status_json(root: &Path, plugin_id: &str) -> String {
    let id = plugin_id.trim();
    let (dir, spec) = match spec_for(root, id) {
        Ok(v) => v,
        Err(reason) => {
            return json!({
                "ok": false,
                "pluginId": id,
                "hasSidecar": false,
                "error": reason,
            })
            .to_string()
        }
    };
    let fp = fingerprint(id, &spec);
    let (running, port) = {
        let mut g = lock(sessions());
        match g.get_mut(id) {
            Some(s) => (matches!(s.child.try_wait(), Ok(None)), s.port),
            None => (false, None),
        }
    };
    let cursor = lock(logs()).get(id).map(|l| l.cursor).unwrap_or(0);
    let exit_reason = lock(exits()).get(id).cloned().unwrap_or_default();
    json!({
        "ok": true,
        "pluginId": id,
        "dir": dir.to_string_lossy(),
        "hasSidecar": true,
        "spec": {
            "command": spec.command,
            "args": spec.args,
            "transport": spec.transport,
            "port": spec.port,
            "cwd": spec.cwd,
            "autostart": spec.autostart,
            "restart": spec.restart,
            "sha256": spec.sha256,
            "allowLocalPorts": spec.allow_local_ports,
            "allowLocalAny": spec.allow_local_any,
            "readyTimeoutSec": spec.ready_timeout_sec,
            "memoryLimitMiB": spec.memory_limit_mib,
            "processLimit": spec.process_limit,
        },
        "trusted": is_trusted(root, id, &spec),
        "denied": lock(denied()).contains(&fp),
        "running": running,
        "port": port,
        "exitReason": exit_reason,
        "cursor": cursor,
        "error": "",
    })
    .to_string()
}

/// 起 sidecar；未信任时返回 `status:"needs_trust"` + 信任卡内容。
pub fn start_json(root: &Path, owner: &str, plugin_id: &str) -> String {
    start(root, owner, plugin_id).to_string()
}

/// stdio 请求：`params_json` 为空串 → `null`。
pub fn request_json(plugin_id: &str, method: &str, params_json: &str) -> String {
    let params = if params_json.trim().is_empty() {
        Value::Null
    } else {
        match serde_json::from_str::<Value>(params_json) {
            Ok(v) => v,
            Err(e) => {
                return json!({ "ok": false, "result": Value::Null, "error": format!("params 不是合法 JSON：{e}") })
                    .to_string()
            }
        }
    };
    match request(plugin_id, method, params) {
        Ok(result) => json!({ "ok": true, "result": result, "error": "" }).to_string(),
        Err(reason) => json!({ "ok": false, "result": Value::Null, "error": reason }).to_string(),
    }
}

/// 宿主代请求。`port = 0` 表示「用该插件自己的端口」。
pub fn http_json(
    root: &Path,
    plugin_id: &str,
    method: &str,
    path: &str,
    body: &str,
    port: u16,
) -> String {
    let target = if port == 0 { None } else { Some(port) };
    let body = if body.is_empty() { None } else { Some(body.to_string()) };
    match http(root, plugin_id, method, path, body, target) {
        Ok((status, text)) => {
            json!({ "ok": true, "status": status, "body": text, "error": "" }).to_string()
        }
        Err(reason) => json!({ "ok": false, "status": 0, "body": "", "error": reason }).to_string(),
    }
}

pub fn stop_json(plugin_id: &str) -> String {
    json!({ "ok": true, "stopped": stop(plugin_id), "error": "" }).to_string()
}

pub fn stop_owner_json(owner: &str) -> String {
    json!({ "ok": true, "stopped": stop_owner(owner), "error": "" }).to_string()
}

pub fn trust_json(root: &Path, plugin_id: &str, remember: bool) -> String {
    match grant(root, plugin_id, remember) {
        Ok(()) => json!({ "ok": true, "error": "" }).to_string(),
        Err(reason) => json!({ "ok": false, "error": reason }).to_string(),
    }
}

pub fn deny_json(root: &Path, plugin_id: &str) -> String {
    match deny(root, plugin_id) {
        Ok(()) => json!({ "ok": true, "error": "" }).to_string(),
        Err(reason) => json!({ "ok": false, "error": reason }).to_string(),
    }
}

/// 拉取 `since` 之后的事件（面板每秒轮询一次）。
pub fn events_json(plugin_id: &str, since: u64) -> String {
    let id = plugin_id.trim();
    let (cursor, events) = {
        let g = lock(logs());
        match g.get(id) {
            Some(log) => (
                log.cursor,
                log.items
                    .iter()
                    .filter(|e| e.get("seq").and_then(Value::as_u64).unwrap_or(0) > since)
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            None => (0, Vec::new()),
        }
    };
    json!({
        "ok": true,
        "cursor": cursor,
        "events": events,
        "running": is_running(id),
        "exitReason": lock(exits()).get(id).cloned().unwrap_or_default(),
        "error": "",
    })
    .to_string()
}

/// 宿主退出前调用（Dart 侧在窗口销毁 / 应用退出时调）。
pub fn kill_all_json() -> String {
    kill_all();
    json!({ "ok": true, "error": "" }).to_string()
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

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "lunac-plugin-runtime-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    fn write_plugin(root: &Path, id: &str, manifest: &str) {
        let dir = plugin_market::plugins_dir(root).join(id);
        std::fs::create_dir_all(&dir).expect("plugin dir");
        std::fs::write(dir.join(plugin_market::MANIFEST_FILE), manifest).expect("manifest");
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
        assert!(port_allowed(Some(8080), false, &[], 8080));
        assert!(!port_allowed(Some(8080), false, &[], 11434));
        assert!(port_allowed(Some(8080), false, &[11434], 11434));
        assert!(port_allowed(None, true, &[], 11434));
        assert!(!port_allowed(None, false, &[], 11434));
    }

    #[test]
    fn trust_gate_round_trips_and_fails_closed_on_bad_file() {
        let root = temp_root("trust");
        write_plugin(
            &root,
            "demo",
            r#"{"id":"demo","entry":"index.js","permissions":["process.spawn"],
                "sidecar":{"command":"bin/demo.exe","transport":"stdio"}}"#,
        );
        assert!(!is_trusted(&root, "demo", &spec("bin/demo.exe", "")));
        grant(&root, "demo", true).expect("grant");
        assert!(is_trusted(&root, "demo", &spec("bin/demo.exe", "")));
        // 落盘文件坏了 ⇒ 一律按不信任（绝不反过来当已信任）。
        std::fs::write(trust_path(&root), "{oops").expect("bad trust");
        assert!(!is_trusted(&root, "demo", &spec("bin/demo.exe", "")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn start_reports_needs_trust_with_the_card_payload() {
        let root = temp_root("needs-trust");
        write_plugin(
            &root,
            "demo",
            r#"{"id":"demo","entry":"index.js","permissions":["process.spawn"],
                "sidecar":{"command":"bin/demo.exe","args":["--x"],"transport":"stdio",
                           "allowLocalPorts":[11434]}}"#,
        );
        let v: Value = serde_json::from_str(&start_json(&root, "panel", "demo")).expect("json");
        assert_eq!(v["status"], "needs_trust");
        assert_eq!(v["command"], "bin/demo.exe");
        assert_eq!(v["args"][0], "--x");
        assert_eq!(v["allowLocalPorts"][0], 11434);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn denied_plugins_do_not_start_again_in_the_same_session() {
        let root = temp_root("deny");
        write_plugin(
            &root,
            "demo-deny",
            r#"{"id":"demo-deny","entry":"index.js","permissions":["process.spawn"],
                "sidecar":{"command":"bin/demo.exe"}}"#,
        );
        deny(&root, "demo-deny").expect("deny");
        let v: Value = serde_json::from_str(&start_json(&root, "panel", "demo-deny")).expect("json");
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap_or("").contains("已拒绝"));
        lock(denied()).clear();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reader_resolves_pending_requests_and_logs_events() {
        let id = format!("reader-{}", std::process::id());
        let (ready_tx, ready_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        lock(pending())
            .entry(id.clone())
            .or_default()
            .insert(7, tx);
        let lines = "{\"method\":\"ready\",\"params\":{\"port\":8099}}\n\
                     {\"method\":\"event\",\"params\":{\"n\":1}}\n\
                     {\"id\":7,\"result\":{\"ok\":true}}\n";
        spawn_reader(id.clone(), std::env::temp_dir(), std::io::Cursor::new(lines), ready_tx);
        let ready = ready_rx.recv_timeout(Duration::from_secs(2)).expect("ready");
        assert_eq!(ready, Ok(Some(8099)));
        let reply = rx.recv_timeout(Duration::from_secs(2)).expect("reply");
        assert_eq!(reply.unwrap()["ok"], true);
        // 事件日志里应有 ready 之后的那条主动推（ready 本身不进日志）。
        let mut cursor = 0u64;
        let mut seen = Vec::new();
        for _ in 0..20 {
            let v: Value = serde_json::from_str(&events_json(&id, cursor)).expect("json");
            cursor = v["cursor"].as_u64().unwrap_or(cursor);
            for e in v["events"].as_array().cloned().unwrap_or_default() {
                seen.push(e["payload"]["method"].as_str().unwrap_or("").to_string());
            }
            if seen.contains(&"event".to_string()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(seen.contains(&"event".to_string()), "主动推应进事件日志：{seen:?}");
        lock(pending()).remove(&id);
        reset_log(&id);
    }

    /// sidecar → 宿主的 `kv.*`：作用域 = 自己的插件 id（读不到别人的），失败给一句人话。
    #[test]
    fn host_kv_requests_are_scoped_to_the_plugin() {
        let root = temp_root("host-kv");
        let set = host_request(
            &root,
            "kv-a",
            1,
            "kv.set",
            Some(&json!({ "key": "theme", "value": { "dark": true } })),
        );
        assert_eq!(set["id"], 1);
        assert!(set["result"]["bytes"].as_u64().is_some(), "{set}");

        let got = host_request(&root, "kv-a", 2, "kv.get", Some(&json!({ "key": "theme" })));
        assert_eq!(got["result"]["found"], true);
        assert_eq!(got["result"]["value"]["dark"], true);

        // 另一个插件看不到同一个 key（存储按 id 分文件）。
        let other = host_request(&root, "kv-b", 3, "kv.get", Some(&json!({ "key": "theme" })));
        assert_eq!(other["result"]["found"], false);

        let listed = host_request(&root, "kv-a", 4, "kv.list", None);
        assert_eq!(listed["result"]["entries"][0]["key"], "theme");

        let removed = host_request(&root, "kv-a", 5, "kv.delete", Some(&json!({ "key": "theme" })));
        assert_eq!(removed["result"]["removed"], true);

        let unknown = host_request(&root, "kv-a", 6, "fs.read", None);
        assert!(
            unknown["error"]["message"].as_str().unwrap().contains("只支持 kv."),
            "{unknown}"
        );
        let missing = host_request(&root, "kv-a", 7, "kv.get", None);
        assert!(
            missing["error"]["message"].as_str().unwrap().contains("params.key"),
            "{missing}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn request_on_an_unstarted_plugin_is_an_error() {
        let err = request("never-started", "ping", Value::Null).expect_err("must fail");
        assert!(err.contains("未启动"), "{err}");
    }

    #[test]
    fn http_path_must_be_a_relative_slash_path() {
        let root = temp_root("http-path");
        write_plugin(
            &root,
            "demo-http",
            r#"{"id":"demo-http","entry":"index.js","permissions":["process.spawn"],
                "sidecar":{"command":"bin/demo.exe","transport":"http","port":8090}}"#,
        );
        let err = http(&root, "demo-http", "GET", "http://evil/x", None, None)
            .expect_err("must fail");
        assert!(err.contains("相对路径"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 真起一个进程的端到端：`.bat` 先说 ready，再对第一条请求回包。
    /// 这条用例同时覆盖 spawn / 握手 / NDJSON 往返 / 收进程。
    #[cfg(windows)]
    #[test]
    fn stdio_round_trip_against_a_real_child_process() {
        let root = temp_root("e2e");
        let dir = plugin_market::plugins_dir(&root).join("e2e");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join("reply.bat"),
            "@echo off\r\n\
             echo {\"method\":\"ready\",\"params\":{\"port\":0}}\r\n\
             set /p line=\r\n\
             echo {\"id\":1,\"result\":{\"ok\":true,\"echo\":\"yes\"}}\r\n",
        )
        .expect("bat");
        std::fs::write(
            dir.join(plugin_market::MANIFEST_FILE),
            r#"{"id":"e2e","entry":"index.js","permissions":["process.spawn"],
                "sidecar":{"command":"reply.bat","transport":"stdio","readyTimeoutSec":15}}"#,
        )
        .expect("manifest");

        let v: Value = serde_json::from_str(&start_json(&root, "unit", "e2e")).expect("json");
        assert_eq!(v["status"], "needs_trust", "{v}");
        grant(&root, "e2e", false).expect("grant");
        let v: Value = serde_json::from_str(&start_json(&root, "unit", "e2e")).expect("json");
        assert_eq!(v["status"], "started", "{v}");

        let reply: Value =
            serde_json::from_str(&request_json("e2e", "ping", "")).expect("reply json");
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["result"]["echo"], "yes");

        assert!(stop("e2e"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
