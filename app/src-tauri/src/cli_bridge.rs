// src/cli_bridge.rs
// Global CLI state — shared between Tauri commands and HTTP agent server.
// Allows VSCode extension to drive Agent mode via HTTP while the desktop
// app uses Tauri IPC on the same agent.exe process.
//
// Architecture:
//   CLI_STDIN   → write user messages to agent.exe
//   CLI_PROCESS → lifecycle (kill on stop)
//   CLI_OUTPUT  → mpsc broadcast for SSE streaming (HTTP) + Tauri events

use std::io::Write;
use std::process::{Child, ChildStdin};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

type OutputTx = mpsc::SyncSender<String>;
type OutputRx = mpsc::Receiver<String>;

/// Bounded per-subscriber buffer. If an SSE client stops reading (half-open
/// socket), sends fail and the subscriber is dropped instead of buffering
/// agent.exe output forever — prevents unbounded background memory growth.
const OUTPUT_CHANNEL_CAPACITY: usize = 512;

static CLI_PROCESS: OnceLock<Mutex<Option<Child>>> = OnceLock::new();
static CLI_STDIN: OnceLock<Mutex<Option<ChildStdin>>> = OnceLock::new();
/// Broadcast senders for cli stdout lines.
/// Each connected HTTP SSE client registers one sender here.
static CLI_OUTPUT_TXS: OnceLock<Arc<Mutex<Vec<OutputTx>>>> = OnceLock::new();

fn ensure_txs() -> &'static Arc<Mutex<Vec<OutputTx>>> {
    CLI_OUTPUT_TXS.get_or_init(|| Arc::new(Mutex::new(Vec::new())))
}

// ── Process lifecycle ─────────────────────────────────────────────

pub fn set_process(child: Child) {
    let _ = CLI_PROCESS
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map(|mut g| *g = Some(child));
}

pub fn set_stdin(stdin: ChildStdin) {
    let _ = CLI_STDIN
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map(|mut g| *g = Some(stdin));
}

pub fn take_process() -> Option<Child> {
    CLI_PROCESS
        .get()
        .and_then(|m| m.lock().ok())
        .and_then(|mut g| g.take())
}

pub fn is_running() -> bool {
    CLI_STDIN
        .get()
        .and_then(|m| m.lock().ok())
        .map(|g| g.is_some())
        .unwrap_or(false)
}

/// Write a JSON line to agent.exe stdin. Used by both Tauri command and HTTP endpoint.
pub fn write_to_cli(json_line: &str) -> Result<(), String> {
    let mut guard = CLI_STDIN
        .get()
        .ok_or("CLI not initialized")?
        .lock()
        .map_err(|e| e.to_string())?;
    let stdin = guard.as_mut().ok_or("CLI stdin not available")?;
    stdin
        .write_all(json_line.as_bytes())
        .map_err(|e| e.to_string())?;
    stdin.write_all(b"\n").map_err(|e| e.to_string())?;
    stdin.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// 强制结束 `pid` 及其**整棵子进程树**（Windows `taskkill /F /T`）。
///
/// 为什么需要它：`Child::kill()` 走的是 `TerminateProcess`，它**不会**连带子进程 ——
/// agent 正在跑的 Cmd / PowerShell 会变孤儿继续跑（用户报的「重试 / 停止后任务没停、
/// 命令还在直接运行」）。`taskkill /T` 顺着父进程树把 agent.exe 与它当前的命令子进程
/// 一起收掉。
///
/// ⚠️ **必须在该进程还活着时调用**：父进程一死，`/T` 就查不到子进程了 —— 所以调用点
/// 排在 `child.kill()` **之前**。失败不致命：调用方随后仍会 `child.kill()`，最坏退化成
/// 旧行为（只杀 agent.exe）。
///
/// 非 Windows 上是空操作（本应用只发 Windows）。
fn kill_process_tree(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000); // CREATE_NO_WINDOW — 不闪控制台
        if let Ok(mut c) = cmd.spawn() {
            // 最多等 1s：taskkill 正常是毫秒级，卡住也不能拖死 stop_cli。
            let deadline = Instant::now() + Duration::from_millis(1000);
            while Instant::now() < deadline {
                match c.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => break,
                }
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = pid;
}

/// Kill agent.exe **and the command(s) it is currently running**, then clean up state.
/// Called on stop or app shutdown.
///
/// 只 `TerminateProcess(agent.exe)` 是不够的：它 spawn 的命令子进程不会跟着死。所以先
/// `taskkill /F /T` 收整棵进程树（见 `kill_process_tree`），再 `child.kill()` 兜底。
/// Never blocks indefinitely: taskkill 与 TerminateProcess 都带超时轮询，绝不无限等
/// （一个卡死的 agent.exe 不许冻住 stop_cli）。
pub fn kill_and_cleanup() {
    // Kill the whole tree first (parent must still be alive for /T to find children),
    // then the process itself as a fallback.
    if let Some(mut child) = take_process() {
        kill_process_tree(child.id());
        let _ = child.kill();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,           // exited
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => break,                      // timeout / error — drop handle
            }
        }
    }
    // Clear stdin
    if let Some(m) = CLI_STDIN.get() {
        if let Ok(mut g) = m.lock() {
            *g = None;
        }
    }
    // Clear process
    if let Some(m) = CLI_PROCESS.get() {
        if let Ok(mut g) = m.lock() {
            *g = None;
        }
    }
}

// ── Output broadcast (stdout lines → SSE + Tauri events) ─────────

/// Register a new subscriber. Returns a Receiver for SSE streaming.
/// The stdout reader thread in commands.rs will send each line to all registrants.
/// Bounded channel: a subscriber that stops reading gets dropped (send fails)
/// instead of accumulating agent.exe output in memory.
pub fn subscribe_output() -> OutputRx {
    let (tx, rx) = mpsc::sync_channel(OUTPUT_CHANNEL_CAPACITY);
    if let Ok(mut guard) = ensure_txs().lock() {
        guard.push(tx);
    }
    rx
}

/// Broadcast a line from agent.exe stdout to all registered SSE subscribers.
/// Cleans up dead subscribers (receivers that have been dropped).
pub fn broadcast_output(line: String) {
    let txs = ensure_txs();
    if let Ok(mut guard) = txs.lock() {
        guard.retain(|tx| tx.send(line.clone()).is_ok());
    }
}
