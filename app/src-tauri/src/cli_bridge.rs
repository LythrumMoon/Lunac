// src/cli_bridge.rs
// Global CLI state — shared between Tauri commands and HTTP agent server.
// Allows VSCode extension to drive Agent mode via HTTP while the desktop
// app uses Tauri IPC on the same cli.exe process.
//
// Architecture:
//   CLI_STDIN   → write user messages to cli.exe
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
/// cli.exe output forever — prevents unbounded background memory growth.
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

/// Write a JSON line to cli.exe stdin. Used by both Tauri command and HTTP endpoint.
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

/// Kill cli.exe and clean up all state. Called on stop or app shutdown.
/// Never blocks indefinitely: TerminateProcess is asynchronous, so we poll
/// try_wait for up to 2s instead of an unbounded wait() (a hung cli.exe
/// must not freeze stop_cli).
pub fn kill_and_cleanup() {
    // Kill process
    if let Some(mut child) = take_process() {
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
/// instead of accumulating cli.exe output in memory.
pub fn subscribe_output() -> OutputRx {
    let (tx, rx) = mpsc::sync_channel(OUTPUT_CHANNEL_CAPACITY);
    if let Ok(mut guard) = ensure_txs().lock() {
        guard.push(tx);
    }
    rx
}

/// Broadcast a line from cli.exe stdout to all registered SSE subscribers.
/// Cleans up dead subscribers (receivers that have been dropped).
pub fn broadcast_output(line: String) {
    let txs = ensure_txs();
    if let Ok(mut guard) = txs.lock() {
        guard.retain(|tx| tx.send(line.clone()).is_ok());
    }
}
