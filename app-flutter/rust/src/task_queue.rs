//! 通用后台任务队列（v2 重建，2026-11-16）。
//!
//! **为什么重建**：v1 的 `app/src-tauri/src/task_queue.rs` 已随那次误删**永久丢失**
//! （`main.rs` 里还留着 `mod task_queue;`，文件本身没了）；agent 侧的 `TaskEnqueue` /
//! `TaskList` / `TaskCancel`（`core-agent/src/tools.rs`）仍在，且数据契约是写死的 ——
//! 缺的就是宿主这个 worker。**契约一个字节都不改**：
//!   · `ModuleData/tasks/queue.json` = `{version:1, items:[…]}`；
//!     agent 只写「入队（pending）/ 取消」，宿主 worker 写 running / done / failed /
//!     round / sessionId / summary / error / updatedAt；
//!   · `ModuleData/tasks/queue.lock` —— 与 agent 共用的跨进程锁（同 `cron.lock` 那套）。
//!
//! **执行口径（与 v1 一致）**：
//!   · 每 [`TICK`] 醒一次；严格 FIFO；同一时刻只跑一条（`cron::try_acquire_run()` 单飞闸，
//!     与定时任务互相让路）；前端在流式对话时（`cron::busy()`）不领新任务；
//!   · 固定**只读档**：`--permission-mode plan` + `--disallowedTools` 裁掉写类 / 命令类 /
//!     外网 / 子代理 / Cron* / Task* —— 后台任务没有审批回包通道；
//!   · 单任务最多 `maxRounds`（1..=20，缺省 5）轮：第 1 轮发正文，之后
//!     `--resume <session> --checkpoint` 续跑**同一个会话**，只在上一轮以退出码 3
//!     （清单还有未完成项）结束时才再给一轮；每轮 10 分钟墙钟；
//!   · `maxRounds` 用尽仍未完成 ⇒ 记 `failed`（不悄悄无限续命）；
//!   · 宿主退出时仍在跑的条目在**下次启动**记 `failed`，**不自动重跑**（避免重复副作用）。
//!
//! **如实登记（不做）**：① 写类任务不支持（契约里没有 `writeAccess` 这一项）；
//! ② running 的任务不能取消（只能取消 pending —— 与 `TaskCancel` 的工具描述一致）；
//! ③ 宿主没开时任务不跑（agent 的 `TaskEnqueue` 提示语里已写明）。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// 轮询间隔（v1 同口径：3 秒）。
const TICK: Duration = Duration::from_secs(3);
/// 单轮墙钟上限（v1 同口径：10 分钟）。
const ROUND_TIMEOUT: Duration = Duration::from_secs(600);
/// 续跑轮数硬上限（与 `core-agent` 的 `TASK_HARD_ROUNDS` 同值）。
const MAX_ROUNDS_CAP: u64 = 20;
/// 列表预览的字符数（与 agent 的 `TASK_PREVIEW_CHARS` 同值）。
const PREVIEW_CHARS: usize = 200;

/// 续跑时发的那句提示（只在 `--resume` 那一轮用）。
///
/// 为什么要显式发一句：`--resume <id>` 只恢复上文，不会自己产生新提问；而清单里
/// 「未完成」这件事模型得再看见一次才接着做（`--continue` 的内置提示是同一用意，
/// 但它写死 `latest`，多任务下会接错会话）。
const CONTINUE_PROMPT: &str =
    "继续完成 TodoWrite 清单里尚未完成的事项（上一轮以「未完成」结束）。不要重复已经完成的步骤；全部完成后正常收尾。";

/// 调度线程只起一次。
static STARTED: AtomicBool = AtomicBool::new(false);

pub fn dir(root: &Path) -> PathBuf {
    root.join("ModuleData").join("tasks")
}

pub fn queue_path(root: &Path) -> PathBuf {
    dir(root).join("queue.json")
}

/// 除 `cron::CRON_DISALLOWED` 之外，队列还要多裁的三件（任务不能自己再生任务）。
const TASK_EXTRA_DISALLOWED: &[&str] = &["TaskEnqueue", "TaskCancel", "TaskList"];

// ── 读 / 写队列文件 ────────────────────────────────────────────────

enum QueueFile {
    Missing,
    Invalid(String),
    Value(Value),
}

fn read_queue_file(path: &Path) -> QueueFile {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return QueueFile::Missing,
        Err(e) => return QueueFile::Invalid(e.to_string()),
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(value) => QueueFile::Value(value),
        Err(e) => QueueFile::Invalid(e.to_string()),
    }
}

/// 把读到的值归一成 items 数组（`{items:[…]}` 或裸数组两种历史形态都认）。
fn items_of(value: Value) -> Option<Vec<Value>> {
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        return Some(items.clone());
    }
    value.as_array().cloned()
}

/// 面板用的快照：`{ok, reason, items:[…]}`；`reason` 取 `ok` / `missing` / `invalid`。
pub fn snapshot_json(root: &Path) -> String {
    match read_queue_file(&queue_path(root)) {
        QueueFile::Missing => json!({
            "ok": false,
            "reason": "missing",
            "items": [],
        })
        .to_string(),
        QueueFile::Invalid(e) => json!({
            "ok": false,
            "reason": "invalid",
            "error": e,
            "items": [],
        })
        .to_string(),
        QueueFile::Value(value) => match items_of(value) {
            Some(items) => {
                let views: Vec<Value> = items.iter().map(item_view).collect();
                json!({ "ok": true, "reason": "ok", "items": views }).to_string()
            }
            None => json!({
                "ok": false,
                "reason": "invalid",
                "error": "queue.json 既没有 items 数组也不是数组",
                "items": [],
            })
            .to_string(),
        },
    }
}

/// 单条的界面视图：预览 prompt，其余字段原样带出（字段名与 agent 契约一致）。
fn item_view(item: &Value) -> Value {
    let prompt = item.get("prompt").and_then(Value::as_str).unwrap_or("");
    let chars = prompt.chars().count();
    let mut view = item.clone();
    if let Some(obj) = view.as_object_mut() {
        obj.insert(
            "prompt".into(),
            json!(prompt.chars().take(PREVIEW_CHARS).collect::<String>()),
        );
        obj.insert("promptChars".into(), json!(chars));
        obj.insert("promptTruncated".into(), json!(chars > PREVIEW_CHARS));
    }
    view
}

fn status_of(item: &Value) -> &str {
    item.get("status").and_then(Value::as_str).unwrap_or("pending")
}

fn id_of(item: &Value) -> &str {
    item.get("id").and_then(Value::as_str).unwrap_or("")
}

/// 在持锁状态下读 + 改 + 原子写回。`f` 返回 `Err` ⇒ 不落盘。
fn mutate_queue<F>(root: &Path, f: F) -> Result<Value, String>
where
    F: FnOnce(&mut Vec<Value>) -> Result<Value, String>,
{
    let dir = dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    let _lock = crate::cron::acquire_store_lock(&dir, "queue.lock")?;
    let mut items = match read_queue_file(&queue_path(root)) {
        QueueFile::Missing => Vec::new(),
        QueueFile::Invalid(e) => {
            return Err(format!("queue.json 已损坏（{e}）；拒绝改写，请先修好或删除它"))
        }
        QueueFile::Value(value) => items_of(value)
            .ok_or_else(|| "queue.json 既没有 items 数组也不是数组".to_string())?,
    };
    let out = f(&mut items)?;
    let root_value = json!({ "version": 1, "items": items });
    crate::cron::write_json_atomic(&queue_path(root), &root_value)?;
    Ok(out)
}

fn now_ms() -> u64 {
    crate::cron::now_ms()
}

// ── 前端入口：取消 pending ────────────────────────────────────────

/// 取消一条**排队中**的任务（只有 pending 能取消；running 按拒绝处理）。
pub fn cancel_json(root: &Path, payload: &str) -> String {
    let value: Value = match serde_json::from_str(payload.trim()) {
        Ok(value) => value,
        Err(e) => return json!({ "ok": false, "error": format!("入参不是 JSON：{e}") }).to_string(),
    };
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if id.is_empty() {
        return json!({ "ok": false, "error": "缺少任务 id" }).to_string();
    }
    let result = mutate_queue(root, |items| {
        let Some(item) = items.iter_mut().find(|item| id_of(item) == id) else {
            return Err("找不到这条任务（可能已被处理）".to_string());
        };
        let status = status_of(item);
        if status != "pending" {
            return Err(format!("只能取消排队中的任务（当前状态：{status}）"));
        }
        if let Some(obj) = item.as_object_mut() {
            obj.insert("status".into(), json!("cancelled"));
            obj.insert("updatedAt".into(), json!(now_ms()));
        }
        Ok(json!({ "ok": true, "error": "", "id": id, "status": "cancelled" }))
    });
    match result {
        Ok(out) => out.to_string(),
        Err(reason) => json!({ "ok": false, "error": reason }).to_string(),
    }
}

// ── worker ───────────────────────────────────────────────────────

/// 启动 worker（幂等；App 启动时调一次）。
pub fn start_worker(root: PathBuf) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    // 上一进程若在任务执行中退出，条目会永远停在 running —— 启动时如实记 failed。
    mark_stale_running_as_failed(&root);
    std::thread::spawn(move || loop {
        tick(&root);
        std::thread::sleep(TICK);
    });
}

/// 把上次遗留的 `running` 记成 `failed`（**不重跑**：副作用已经发生过）。
fn mark_stale_running_as_failed(root: &Path) {
    // 队列文件不存在 ⇒ 什么都不做（不要顺手创建一个空文件）。
    if matches!(read_queue_file(&queue_path(root)), QueueFile::Missing) {
        return;
    }
    let _ = mutate_queue(root, |items| {
        let now = now_ms();
        for item in items.iter_mut() {
            if status_of(item) != "running" {
                continue;
            }
            if let Some(obj) = item.as_object_mut() {
                obj.insert("status".into(), json!("failed"));
                obj.insert(
                    "error".into(),
                    json!("宿主在任务运行期间退出（不会自动重跑；如需继续请重新入队）"),
                );
                obj.insert("updatedAt".into(), json!(now));
            }
        }
        Ok(Value::Null)
    });
}

fn tick(root: &Path) {
    // 用户在流式对话 ⇒ 不领新任务（与 cron 的 skipped_busy 同口径）。
    if crate::cron::busy() {
        return;
    }
    // 严格 FIFO：第一条 pending。
    let Some(items) = read_items(root) else {
        return;
    };
    let Some(pending_id) = items
        .iter()
        .find(|item| status_of(item) == "pending")
        .map(|item| id_of(item).to_string())
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    // 单飞闸：cron 正在跑 / 另一个任务的尾轮还没收 ⇒ 下一拍再说。
    let Some(_guard) = crate::cron::try_acquire_run() else {
        return;
    };
    // 重新在锁内确认「还是 pending」再认领 —— 用户可能刚好在这一刻取消了它。
    let Some(item) = claim(root, &pending_id) else {
        return;
    };
    let outcome = run_item(root, &item);
    finish(root, &pending_id, outcome);
}

fn read_items(root: &Path) -> Option<Vec<Value>> {
    match read_queue_file(&queue_path(root)) {
        QueueFile::Value(value) => items_of(value),
        _ => None,
    }
}

/// 认领：从 pending 改成 running（带 `startedAt` / `pid`），返回认领到的条目。
fn claim(root: &Path, id: &str) -> Option<Value> {
    let now = now_ms();
    let mut claimed: Option<Value> = None;
    let result = mutate_queue(root, |items| {
        let Some(item) = items.iter_mut().find(|item| id_of(item) == id) else {
            return Ok(Value::Null);
        };
        if status_of(item) != "pending" {
            return Ok(Value::Null);
        }
        if let Some(obj) = item.as_object_mut() {
            obj.insert("status".into(), json!("running"));
            obj.insert("startedAt".into(), json!(now));
            obj.insert("updatedAt".into(), json!(now));
            obj.insert("pid".into(), json!(std::process::id()));
        }
        claimed = Some(item.clone());
        Ok(Value::Null)
    });
    result.ok().and_then(|_| claimed)
}

/// 单条任务的最终结果。
struct ItemOutcome {
    status: &'static str,
    round: u64,
    session: String,
    summary: String,
    error: String,
}

/// 单轮的结果。
struct RoundOutcome {
    code: i32,
    session: String,
    summary: String,
    error: String,
    log_path: PathBuf,
}

fn finish(root: &Path, id: &str, outcome: ItemOutcome) {
    let now = now_ms();
    let result = mutate_queue(root, |items| {
        let Some(item) = items.iter_mut().find(|item| id_of(item) == id) else {
            return Ok(Value::Null);
        };
        if let Some(obj) = item.as_object_mut() {
            obj.insert("status".into(), json!(outcome.status));
            obj.insert("round".into(), json!(outcome.round));
            obj.insert("sessionId".into(), json!(outcome.session));
            obj.insert("summary".into(), json!(outcome.summary));
            obj.insert("error".into(), json!(outcome.error));
            obj.insert("finishedAt".into(), json!(now));
            obj.insert("updatedAt".into(), json!(now));
        }
        Ok(Value::Null)
    });
    if let Err(reason) = result {
        eprintln!("[lunac-tasks] 写回任务结果失败：{reason}");
    }
}

/// 轮数归一：缺省 5，夹在 `1..=20`。
fn clamp_rounds(raw: Option<u64>) -> u64 {
    raw.unwrap_or(5).clamp(1, MAX_ROUNDS_CAP)
}

/// 是否还要再来一轮（纯函数，便于单测）。
///
/// 只有「退出码 3（清单未完成）+ 还没用尽 maxRounds + 拿得到会话 id」三条同时成立才续跑。
/// 拿不到 session id 就没法 `--resume`，再跑一轮只会重复劳动 ⇒ 直接收口记失败。
fn should_continue(code: i32, round: u64, max_rounds: u64, session: &str) -> bool {
    code == 3 && round < max_rounds && !session.trim().is_empty()
}

fn run_item(root: &Path, item: &Value) -> ItemOutcome {
    let prompt = item.get("prompt").and_then(Value::as_str).unwrap_or("");
    let max_rounds = clamp_rounds(item.get("maxRounds").and_then(Value::as_u64));
    let mut session = item
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut round = 1u64;
    loop {
        update_progress(root, id_of(item), round, &session);
        let outcome = run_round(root, item, round, prompt, &session);
        let RoundOutcome {
            code,
            session: got_session,
            summary,
            error,
            log_path,
        } = match outcome {
            Ok(outcome) => outcome,
            Err(reason) => {
                return ItemOutcome {
                    status: "failed",
                    round,
                    session,
                    summary: String::new(),
                    error: reason,
                }
            }
        };
        if !got_session.trim().is_empty() {
            session = got_session;
        }
        if code == 0 {
            return ItemOutcome {
                status: "done",
                round,
                session,
                summary,
                error: String::new(),
            };
        }
        if !should_continue(code, round, max_rounds, &session) {
            // 退出码 3 + 轮数用尽 = 「没做完」；其余失败照原样带出。
            let error = if code == 3 {
                format!(
                    "已用尽 {max_rounds} 轮仍未完成（日志：{}）",
                    log_path.display()
                )
            } else if error.is_empty() {
                format!("退出码 {code}（日志：{}）", log_path.display())
            } else {
                error
            };
            return ItemOutcome {
                status: "failed",
                round,
                session,
                summary: String::new(),
                error,
            };
        }
        round += 1;
    }
}

fn update_progress(root: &Path, id: &str, round: u64, session: &str) {
    let now = now_ms();
    let _ = mutate_queue(root, |items| {
        let Some(item) = items.iter_mut().find(|item| id_of(item) == id) else {
            return Ok(Value::Null);
        };
        if let Some(obj) = item.as_object_mut() {
            obj.insert("round".into(), json!(round));
            obj.insert("sessionId".into(), json!(session));
            obj.insert("updatedAt".into(), json!(now));
        }
        Ok(Value::Null)
    });
}

/// 跑一轮单发 agent。
fn run_round(
    root: &Path,
    item: &Value,
    round: u64,
    prompt: &str,
    session: &str,
) -> Result<RoundOutcome, String> {
    let spec = crate::agent::launch_spec(root)?;
    let mut cmd = Command::new(&spec.exe);
    cmd.arg("--print").args(["--output-format", "stream-json"]);
    // 只读档 + 检查点：没有检查点就无法「下一轮接着同一个会话」。
    cmd.args(["--permission-mode", "plan"]);
    cmd.arg("--checkpoint");
    if round > 1 {
        cmd.arg("--resume").arg(session);
    }
    cmd.arg("--add-dir").arg(&spec.workdir);
    // `--disallowedTools` 是 variadic ⇒ **必须放最后**。
    cmd.arg("--disallowedTools");
    for name in crate::cron::CRON_DISALLOWED
        .iter()
        .chain(TASK_EXTRA_DISALLOWED.iter())
    {
        cmd.arg(name);
    }
    cmd.current_dir(&spec.workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &spec.env {
        cmd.env(key, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 agent.exe 失败：{e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let query = if round > 1 { CONTINUE_PROMPT } else { prompt };
        let _ = stdin.write_all(query.as_bytes());
    }
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_handle = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stream) = stdout {
            let _ = stream.read_to_string(&mut text);
        }
        text
    });
    let err_handle = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stream) = stderr {
            let _ = stream.read_to_string(&mut text);
        }
        text
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if started.elapsed() > ROUND_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("等待 agent.exe 失败：{e}")),
        }
    };
    let stdout_text = out_handle.join().unwrap_or_default();
    let stderr_text = err_handle.join().unwrap_or_default();

    // 最后一个 `{"type":"result"}` 就是本轮收口；`session_id` 从任意一行取第一个非空。
    let mut result = Value::Null;
    let mut session_id = String::new();
    for line in stdout_text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if session_id.is_empty() {
            if let Some(id) = value.get("session_id").and_then(Value::as_str) {
                if !id.trim().is_empty() {
                    session_id = id.to_string();
                }
            }
        }
        if value.get("type").and_then(Value::as_str) == Some("result") {
            result = value;
        }
    }
    if let Some(id) = result.get("session_id").and_then(Value::as_str) {
        if !id.trim().is_empty() {
            session_id = id.to_string();
        }
    }

    // 证据落 `temp/logs/task-<id>-<epoch>.log`（失败也留），落盘前脱敏。
    let log_dir = root.join("temp").join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join(format!("task-{}-{}.log", id_of(item), now_ms()));
    let _ = std::fs::write(
        &log_path,
        format!(
            "task: {}\nround: {round}\nresume: {session}\n--- stdout ---\n{}\n--- stderr ---\n{}\n",
            id_of(item),
            crate::security_ledger::mask_secrets(&stdout_text),
            crate::security_ledger::mask_secrets(&stderr_text),
        ),
    );

    let Some(status) = status else {
        return Ok(RoundOutcome {
            code: -1,
            session: session_id,
            summary: String::new(),
            error: format!(
                "超过 {} 秒被强杀（日志：{}）",
                ROUND_TIMEOUT.as_secs(),
                log_path.display()
            ),
            log_path,
        });
    };
    crate::cron::record_usage(root, &spec.model, &result, now_ms());
    let text = result
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let is_error = result
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let code = status.code().unwrap_or(-1);
    Ok(RoundOutcome {
        code: if is_error && code == 0 { 1 } else { code },
        session: session_id,
        summary: text.chars().take(400).collect(),
        error: if is_error {
            if text.is_empty() {
                format!("退出码 {code}")
            } else {
                text.chars().take(400).collect()
            }
        } else {
            String::new()
        },
        log_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-task-queue-{tag}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(super::dir(&dir)).expect("create ModuleData/tasks");
        dir
    }

    fn write_queue(root: &Path, items: Value) {
        std::fs::write(
            queue_path(root),
            serde_json::to_string(&json!({ "version": 1, "items": items })).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn snapshot_normalizes_missing_bare_and_corrupt_files() {
        let root = temp_root("snapshot");
        let missing: Value = serde_json::from_str(&snapshot_json(&root)).unwrap();
        assert_eq!(missing["ok"], false);
        assert_eq!(missing["reason"], "missing");

        // 裸数组是历史形态，必须照样认。
        std::fs::write(
            queue_path(&root),
            r#"[{"id":"task_1","status":"pending","prompt":"hi"}]"#,
        )
        .unwrap();
        let bare: Value = serde_json::from_str(&snapshot_json(&root)).unwrap();
        assert_eq!(bare["ok"], true);
        assert_eq!(bare["items"][0]["id"], "task_1");

        std::fs::write(queue_path(&root), "{not json").unwrap();
        let broken: Value = serde_json::from_str(&snapshot_json(&root)).unwrap();
        assert_eq!(broken["ok"], false);
        assert_eq!(broken["reason"], "invalid");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn preview_is_truncated_but_the_contract_fields_stay() {
        let long = "x".repeat(PREVIEW_CHARS + 50);
        let view = item_view(&json!({"id":"t","prompt":long,"status":"pending"}));
        assert_eq!(view["prompt"].as_str().unwrap().chars().count(), PREVIEW_CHARS);
        assert_eq!(view["promptTruncated"], true);
        assert_eq!(view["promptChars"], (PREVIEW_CHARS + 50) as u64);
        assert_eq!(view["status"], "pending");
    }

    #[test]
    fn cancel_only_touches_pending_items() {
        let root = temp_root("cancel");
        write_queue(
            &root,
            json!([
                {"id":"task_1","status":"pending","prompt":"a"},
                {"id":"task_2","status":"running","prompt":"b"},
            ]),
        );
        let ok: Value = serde_json::from_str(&cancel_json(
            &root,
            r#"{"id":"task_1"}"#,
        ))
        .unwrap();
        assert_eq!(ok["ok"], true);
        let items = read_items(&root).unwrap();
        assert_eq!(status_of(&items[0]), "cancelled");

        let running: Value = serde_json::from_str(&cancel_json(
            &root,
            r#"{"id":"task_2"}"#,
        ))
        .unwrap();
        assert_eq!(running["ok"], false, "running 不能取消（按拒绝处理）");
        assert!(running["error"].as_str().unwrap().contains("running"));

        let missing: Value = serde_json::from_str(&cancel_json(
            &root,
            r#"{"id":"nope"}"#,
        ))
        .unwrap();
        assert_eq!(missing["ok"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn claim_flips_pending_to_running_and_leaves_others_alone() {
        let root = temp_root("claim");
        write_queue(
            &root,
            json!([
                {"id":"task_1","status":"pending","prompt":"a"},
                {"id":"task_2","status":"pending","prompt":"b"},
            ]),
        );
        let claimed = claim(&root, "task_1").expect("应认领第一条");
        assert_eq!(claimed["status"], "running");
        assert_eq!(claimed["pid"], std::process::id() as u64);
        let items = read_items(&root).unwrap();
        assert_eq!(status_of(&items[0]), "running");
        assert_eq!(status_of(&items[1]), "pending", "FIFO 不许跳队");
        assert!(claim(&root, "task_1").is_none(), "已 running 的不能再认领");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_running_items_are_marked_failed_without_rerunning() {
        let root = temp_root("stale");
        write_queue(
            &root,
            json!([
                {"id":"task_1","status":"running","prompt":"a"},
                {"id":"task_2","status":"pending","prompt":"b"},
            ]),
        );
        mark_stale_running_as_failed(&root);
        let items = read_items(&root).unwrap();
        assert_eq!(status_of(&items[0]), "failed");
        assert!(items[0]["error"].as_str().unwrap().contains("不会自动重跑"));
        assert_eq!(status_of(&items[1]), "pending", "pending 不动");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rounds_are_clamped_and_continuation_needs_a_resumable_session() {
        assert_eq!(clamp_rounds(None), 5);
        assert_eq!(clamp_rounds(Some(0)), 1);
        assert_eq!(clamp_rounds(Some(99)), MAX_ROUNDS_CAP);

        assert!(should_continue(3, 1, 5, "sess_1"));
        assert!(!should_continue(3, 5, 5, "sess_1"), "轮数用尽不再续");
        assert!(!should_continue(3, 1, 5, "  "), "拿不到会话 id 就没法 resume");
        assert!(!should_continue(0, 1, 5, "sess_1"), "正常结束不续");
        assert!(!should_continue(1, 1, 5, "sess_1"), "失败不续");
    }

    #[test]
    fn disabled_tool_list_keeps_the_autonomy_tools_out() {
        // 底表（cron 的）必须仍挡住写类 / 命令类 / 外网类。
        for name in ["Write", "Edit", "Cmd", "PowerShell", "WebFetch", "Agent"] {
            assert!(
                crate::cron::CRON_DISALLOWED.contains(&name),
                "无人值守底表缺少 {name}"
            );
        }
        // 队列额外多裁任务三件，且不与底表重复（重复就等于名单已经开始漂移）。
        for name in TASK_EXTRA_DISALLOWED {
            assert!(
                !crate::cron::CRON_DISALLOWED.contains(name),
                "{name} 不该同时出现在两张名单里"
            );
        }
    }
}
