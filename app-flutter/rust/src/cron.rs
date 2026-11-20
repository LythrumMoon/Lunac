//! 定时任务：宿主侧调度 + 只读列表 + 立即运行（重建 v1 丢失的 `cron.rs`）。
//!
//! 数据契约与 core-agent 的 `CronCreate` / `CronDelete` / `CronList` **完全一致**
//! （见 `core-agent/src/tools.rs` 的 `cron_*`）：
//!   · `ModuleData/tasks/cron.json` —— job 定义，**只由 agent 侧写**（宿主只在
//!     [`set_write_access`] 里改 `writeAccess` 一个字）；
//!   · `ModuleData/tasks/cron-state.json` —— 运行态，**只由本模块写**；
//!   · `ModuleData/tasks/cron.lock` —— 两个写者共用的跨进程锁（陈旧 30s 接管）。
//!
//! 目录名、`heartbeatAt` 的 90s 窗口、`lastStatus` 的取值都与 agent 侧读取口径对齐：
//! agent 的 `CronList` 会把本模块写的运行态合并进它返回的 job 视图。
//!
//! 调度口径（v1 同款）：
//!   · 每 [`TICK`] 醒一次，重读 `cron.json`（文件小，直接读）；
//!   · 同一时刻**只跑一个 job**；前端在流式对话时报忙（[`set_busy`]）⇒ 记 `skipped_busy`，
//!     **不排队**（避免「用户刚打开 App 就被补跑一堆任务」）；
//!   · 错过只补「最近一次且延迟 ≤10 分钟」，更久记 `skipped_missed`（见 [`is_missed`]）；
//!   · 连续失败 [`AUTO_DISABLE_AFTER`] 次自动停用（`autoDisabled=true`），用户改回
//!     `enabled` 后恢复计数；
//!   · 执行 = 单发 `agent.exe --print`（query 走 **stdin**）+ `--output-format stream-json`，
//!     **不写**任何会话文件；用量以 `source="cron"` 记进 `ModuleData/usage/`。
//!
//! **如实登记（未做）**：① 单发默认 `--permission-mode plan`（只读），只有用户显式打开
//! `writeAccess` 才换 `acceptEdits` + `--unattended-allow Write,Edit,NotebookEdit` ——
//! **不**使用 `--dangerously-skip-permissions`（v1 明确禁止：它会关工作区锁并绕过危险
//! 命令的人工确认，见 `docs/forbidden.md` §禁用 1.2）；② 不做 `--checkpoint` / `--resume`
//! 续跑；③ 完成通知只落在 `cron-state.json` 的 `lastStatus`（v1 的 `cron-finished` 浮层
//! 事件尚未接到 Dart 侧）。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// 调度心跳间隔（agent 侧以 90s 判定「调度器活着」）。
const TICK: Duration = Duration::from_secs(30);
/// 心跳窗口（与 core-agent `CRON_ALIVE_WINDOW_MS` 同值）。
const ALIVE_WINDOW_MS: u64 = 90_000;
/// 单次执行上限：到点强杀并记失败。
const RUN_TIMEOUT: Duration = Duration::from_secs(600);
/// 连续失败多少次自动停用（v1 口径：3 次）。
const AUTO_DISABLE_AFTER: u64 = 3;
/// 错过运行的宽限：只补「延迟 ≤10 分钟」的那一拍，更久记 `skipped_missed`（v1 口径）。
const MISSED_GRACE_MS: u64 = 10 * 60_000;
/// 每个 job 每天最多跑多少次（与 core-agent 的硬上限同值）。
const HARD_MAX_RUNS_PER_DAY: u64 = 48;
/// 跨进程锁的超时 / 陈旧阈值（与 core-agent 同值）。
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_STALE: Duration = Duration::from_secs(30);

/// 前端「正在流式对话」的忙标记（调度器看到就跳过这一轮）。
static BUSY: AtomicBool = AtomicBool::new(false);
/// 本进程是否已有 job 在跑（全局同一时刻只跑一个）。
static RUNNING: AtomicBool = AtomicBool::new(false);
/// 调度线程只起一次。
static STARTED: AtomicBool = AtomicBool::new(false);
/// 进程内写 `cron-state.json` 的互斥（跨进程另有 `cron.lock`）。
static STATE_GUARD: OnceLock<std::sync::Mutex<()>> = OnceLock::new();

fn state_guard() -> &'static std::sync::Mutex<()> {
    STATE_GUARD.get_or_init(|| std::sync::Mutex::new(()))
}

pub fn dir(root: &Path) -> PathBuf {
    root.join("ModuleData").join("tasks")
}

pub fn definitions_path(root: &Path) -> PathBuf {
    dir(root).join("cron.json")
}

pub fn state_path(root: &Path) -> PathBuf {
    dir(root).join("cron-state.json")
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 调度形式（与 `CronCreate` 的入参一致，不用 cron 表达式）。
#[derive(Clone, Debug, PartialEq)]
pub enum Schedule {
    /// `{"kind":"every","everyMinutes":N}`（15..=1440）。
    Every { every_minutes: u64 },
    /// `{"kind":"daily","at":"HH:MM"}`（本地时间）。
    Daily { hour: u32, minute: u32 },
}

impl Schedule {
    pub fn text(&self) -> String {
        match self {
            Self::Every { every_minutes } => format!("每 {every_minutes} 分钟"),
            Self::Daily { hour, minute } => format!("每天 {hour:02}:{minute:02}"),
        }
    }
}

fn parse_hhmm(text: &str) -> Option<(u32, u32)> {
    let (hour, minute) = text.trim().split_once(':')?;
    if hour.len() != 2 || minute.len() != 2 {
        return None;
    }
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

pub fn parse_schedule(value: &Value) -> Option<Schedule> {
    match value.get("kind").and_then(Value::as_str)? {
        "every" => {
            let every_minutes = value.get("everyMinutes").and_then(Value::as_u64)?;
            if !(15..=1440).contains(&every_minutes) {
                return None;
            }
            Some(Schedule::Every { every_minutes })
        }
        "daily" => {
            let (hour, minute) = parse_hhmm(value.get("at").and_then(Value::as_str)?)?;
            Some(Schedule::Daily { hour, minute })
        }
        _ => None,
    }
}

/// 下一次执行时刻（epoch ms）。`anchor` = 上次运行时间（没跑过就用创建时间 / 当下）。
///
/// `offset_minutes` = 本地时区相对 UTC 的偏移（东八区 480）—— 传参而不是内部取，
/// 单测才能不依赖机器时区。
pub fn next_run_at(schedule: &Schedule, anchor_ms: u64, now_ms: u64, offset_minutes: i64) -> u64 {
    match schedule {
        Schedule::Every { every_minutes } => {
            let step = every_minutes.saturating_mul(60_000);
            let anchor = anchor_ms.min(now_ms);
            if anchor.saturating_add(step) > now_ms {
                anchor.saturating_add(step)
            } else {
                // 睡过了（App 没开）也只补一次：下一拍从现在开始算，不连发。
                now_ms.saturating_add(step)
            }
        }
        Schedule::Daily { hour, minute } => {
            let day = 86_400_000i64;
            let offset = offset_minutes * 60_000;
            let local_now = now_ms as i64 + offset;
            let local_midnight = local_now - local_now.rem_euclid(day);
            let want = *hour as i64 * 3_600_000 + *minute as i64 * 60_000;
            let mut candidate = local_midnight + want;
            if candidate <= local_now {
                candidate += day;
            }
            (candidate - offset).max(0) as u64
        }
    }
}

/// 本地时区偏移（分钟）。用 Win32 `GetLocalTime` 与 UTC 现时对比；非 Windows 返回 0。
#[cfg(windows)]
fn local_offset_minutes() -> i64 {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTimeParts {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(out: *mut SystemTimeParts);
        fn GetSystemTime(out: *mut SystemTimeParts);
    }
    fn as_minutes(parts: &SystemTimeParts) -> i64 {
        // 不跨月比较：只算「当日秒数」，差值取最近 24h 内的那个方向。
        parts.hour as i64 * 60 + parts.minute as i64
    }
    let mut local = SystemTimeParts::default();
    let mut utc = SystemTimeParts::default();
    unsafe {
        GetLocalTime(&mut local);
        GetSystemTime(&mut utc);
    }
    let mut diff = as_minutes(&local) - as_minutes(&utc);
    if diff > 12 * 60 {
        diff -= 24 * 60;
    } else if diff < -12 * 60 {
        diff += 24 * 60;
    }
    diff
}

#[cfg(not(windows))]
fn local_offset_minutes() -> i64 {
    0
}

/// 本地日期 + 本地时间（`YYYY-MM-DD`, `HH:MM`）。
///
/// 记忆的来源前缀、日志文件名与「今日跑过几次」的归零判断共用这一份换算，不各写一套。
pub(crate) fn local_stamp(ms: u64) -> (String, String) {
    let offset = local_offset_minutes() * 60_000;
    let local = ms as i64 + offset;
    let day = 86_400_000i64;
    let days = local.div_euclid(day);
    let rest = local.rem_euclid(day);
    let hour = rest / 3_600_000;
    let minute = (rest / 60_000) % 60;
    // 1970-01-01 起的天数 → 公历（Howard Hinnant 的 civil_from_days）。
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day_of_month = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (
        format!("{year:04}-{month:02}-{day_of_month:02}"),
        format!("{hour:02}:{minute:02}"),
    )
}

/// 一条 job（定义里的字段；未知字段原样忽略）。
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: String,
    pub prompt: String,
    pub schedule: Schedule,
    pub enabled: bool,
    pub write_access: bool,
    pub max_runs_per_day: u64,
    pub notify: String,
    pub workspace: String,
    pub created_at: u64,
}

fn job_from_value(value: &Value) -> Option<Job> {
    let id = value.get("id").and_then(Value::as_str)?.trim().to_string();
    if id.is_empty() {
        return None;
    }
    let prompt = value.get("prompt").and_then(Value::as_str)?.trim().to_string();
    if prompt.is_empty() {
        return None;
    }
    let schedule = parse_schedule(value.get("schedule")?)?;
    Some(Job {
        id,
        prompt,
        schedule,
        enabled: value.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        write_access: value
            .get("writeAccess")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        max_runs_per_day: value
            .get("maxRunsPerDay")
            .and_then(Value::as_u64)
            .unwrap_or(12)
            .clamp(1, HARD_MAX_RUNS_PER_DAY),
        notify: value
            .get("notify")
            .and_then(Value::as_str)
            .unwrap_or("toast")
            .to_string(),
        workspace: value
            .get("workspace")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        created_at: value
            .get("createdAt")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

/// 读定义的归一化结果：`(jobs, tag)`，`tag` ∈ `ok` / `missing` / `invalid`（与 agent 同词）。
pub fn read_jobs(root: &Path) -> (Vec<Job>, &'static str) {
    let path = definitions_path(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), "missing"),
        Err(_) => return (Vec::new(), "invalid"),
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return (Vec::new(), "invalid"),
    };
    let list = value
        .get("jobs")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| value.as_array().cloned());
    let Some(list) = list else {
        return (Vec::new(), "invalid");
    };
    (list.iter().filter_map(job_from_value).collect(), "ok")
}

/// 跨进程锁：`create_new` 获取，陈旧 30s 接管，Drop 时删除（与 core-agent 同口径）。
///
/// `pub(crate)`：任务队列（`task_queue.rs`）与 cron 共用同一套锁实现
/// （`queue.lock` / `cron.lock` 只是文件名不同）—— 不许各写一份（`forbidden.md` §禁用 62）。
pub(crate) struct StoreLock {
    path: PathBuf,
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(crate) fn acquire_store_lock(dir: &Path, name: &str) -> Result<StoreLock, String> {
    let path = dir.join(name);
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write as _;
                let _ = write!(file, "pid={} at={}", std::process::id(), now_ms());
                return Ok(StoreLock { path });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .map(|age| age > LOCK_STALE)
                    .unwrap_or(false);
                if stale {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                if Instant::now() >= deadline {
                    return Err(format!("任务存储忙（拿不到 {name}）"));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("创建 {} 失败：{e}", path.display())),
        }
    }
}

pub(crate) fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写 {} 失败：{e}", tmp.display()))?;
    if let Err(first) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(path);
        std::fs::rename(&tmp, path)
            .map_err(|e| format!("提交 {} 失败：{e}（首次 rename：{first}）", path.display()))?;
    }
    Ok(())
}

fn read_state(root: &Path) -> Value {
    std::fs::read_to_string(state_path(root))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({ "version": 1, "heartbeatAt": 0, "jobs": {} }))
}

fn state_entry(state: &Value, id: &str) -> Value {
    state
        .get("jobs")
        .and_then(|jobs| jobs.get(id))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

/// 写运行态（进程内互斥 + 跨进程锁）。
fn update_state(root: &Path, id: &str, patch: Value) -> Result<Value, String> {
    let _guard = state_guard()
        .lock()
        .map_err(|_| "定时任务运行态锁损坏".to_string())?;
    let dir = dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    let _lock = acquire_store_lock(&dir, "cron.lock")?;
    let mut state = read_state(root);
    if state.get("jobs").and_then(Value::as_object).is_none() {
        state["jobs"] = json!({});
    }
    let mut entry = state
        .get("jobs")
        .and_then(|jobs| jobs.get(id))
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let (Some(target), Some(source)) = (entry.as_object_mut(), patch.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    state["jobs"][id] = entry.clone();
    state["heartbeatAt"] = json!(now_ms());
    write_json_atomic(&state_path(root), &state)?;
    Ok(entry)
}

fn write_heartbeat(root: &Path) {
    let Ok(_guard) = state_guard().lock() else {
        return;
    };
    let dir = dir(root);
    let _ = std::fs::create_dir_all(&dir);
    let Ok(_lock) = acquire_store_lock(&dir, "cron.lock") else {
        return;
    };
    let mut state = read_state(root);
    state["heartbeatAt"] = json!(now_ms());
    let _ = write_json_atomic(&state_path(root), &state);
}

fn increment(entry: &Value, key: &str) -> u64 {
    entry.get(key).and_then(Value::as_u64).unwrap_or(0) + 1
}

/// 合并后的 job 视图（给面板 / FFI；字段名与 agent 的 `CronList` 对齐）。
fn job_view(job: &Job, entry: &Value, auto_disabled: bool) -> Value {
    let last_run_at = entry.get("lastRunAt").cloned().unwrap_or(Value::Null);
    let next_run_at = entry.get("nextRunAt").cloned().unwrap_or(Value::Null);
    json!({
        "id": job.id,
        "prompt": job.prompt.chars().take(200).collect::<String>(),
        "promptChars": job.prompt.chars().count(),
        "promptTruncated": job.prompt.chars().count() > 200,
        "schedule": match &job.schedule {
            Schedule::Every { every_minutes } => json!({ "kind": "every", "everyMinutes": every_minutes }),
            Schedule::Daily { hour, minute } => {
                json!({ "kind": "daily", "at": format!("{hour:02}:{minute:02}") })
            }
        },
        "scheduleText": job.schedule.text(),
        "enabled": job.enabled,
        "writeAccess": job.write_access,
        "maxRunsPerDay": job.max_runs_per_day,
        "notify": job.notify,
        "workspace": job.workspace,
        "createdAt": job.created_at,
        "lastRunAt": last_run_at,
        "nextRunAt": next_run_at,
        "nextRunAtLocal": next_run_at.as_u64().map(|ms| {
            let (date, time) = local_stamp(ms);
            format!("{date} {time}")
        }).unwrap_or_default(),
        "lastStatus": entry.get("lastStatus").cloned().unwrap_or_else(|| json!("never")),
        "lastSummary": entry.get("lastSummary").cloned().unwrap_or_else(|| json!("")),
        "lastError": entry.get("lastError").cloned().unwrap_or_else(|| json!("")),
        "runCount": entry.get("runCount").cloned().unwrap_or_else(|| json!(0)),
        "failureCount": entry.get("failureCount").cloned().unwrap_or_else(|| json!(0)),
        "consecutiveFailures": entry.get("consecutiveFailures").cloned().unwrap_or_else(|| json!(0)),
        "todayRuns": entry.get("todayRuns").cloned().unwrap_or_else(|| json!(0)),
        "autoDisabled": auto_disabled,
    })
}

/// 设置面板读的列表（含 `schedulerAlive`）。
pub fn list_json(root: &Path) -> String {
    let (jobs, tag) = read_jobs(root);
    let state = read_state(root);
    let heartbeat = state.get("heartbeatAt").and_then(Value::as_u64).unwrap_or(0);
    let scheduler_alive = heartbeat > 0 && now_ms().saturating_sub(heartbeat) <= ALIVE_WINDOW_MS;
    let views: Vec<Value> = jobs
        .iter()
        .map(|job| {
            let entry = state_entry(&state, &job.id);
            let auto_disabled = entry
                .get("autoDisabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            job_view(job, &entry, auto_disabled)
        })
        .collect();
    json!({
        "ok": tag == "ok",
        "reason": if tag == "ok" { Value::Null } else { json!(tag) },
        "schedulerAlive": scheduler_alive,
        "count": views.len(),
        "jobs": views,
    })
    .to_string()
}

/// 立即运行一条 job（阻塞到跑完；Dart 侧用 `Isolate.run` 调用）。
pub fn run_now(root: &Path, id: &str) -> String {
    let (jobs, tag) = read_jobs(root);
    if tag != "ok" {
        return json!({ "ok": false, "error": format!("cron.json 不可用（{tag}）") }).to_string();
    }
    let Some(job) = jobs.iter().find(|job| job.id == id.trim()) else {
        return json!({ "ok": false, "error": "找不到这条定时任务（可能刚被删除）" }).to_string();
    };
    let Some(_guard) = try_acquire_run() else {
        return json!({ "ok": false, "error": "已有后台任务在运行，稍后再试" }).to_string();
    };
    let outcome = execute(root, job);
    match outcome {
        Ok(outcome) => {
            let entry = record_outcome(root, job, &outcome, true);
            json!({
                "ok": true,
                "error": "",
                "job": job_view(job, &entry, false),
            })
            .to_string()
        }
        Err(reason) => json!({ "ok": false, "error": reason }).to_string(),
    }
}

/// 前端忙态：流式对话开始 / 结束时调用（调度器看到忙就记 `skipped_busy`）。
pub fn set_busy(busy: bool) {
    BUSY.store(busy, Ordering::SeqCst);
}

/// 前端是否在流式对话（任务队列 worker 也只读这一个信号）。
pub(crate) fn busy() -> bool {
    BUSY.load(Ordering::SeqCst)
}

/// 「同一时刻只跑一个后台 agent」的单飞闸（cron 与任务队列共用）。
///
/// 为什么放在 cron：它是第一个使用者，且两边的口径必须一致（宿主同一时刻只养一个
/// 无头 agent；并行跑会把 token 与磁盘快照互相踩）。任务队列只能**借用**它，不许
/// 自己再立一个标志（`forbidden.md` §禁用 62）。
pub(crate) struct RunGuard;

impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

/// 拿到闸 ⇒ `Some`（Drop 时自动释放）；有人已在跑 ⇒ `None`。
pub(crate) fn try_acquire_run() -> Option<RunGuard> {
    if RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        Some(RunGuard)
    } else {
        None
    }
}

/// 逐 job 的「无人值守可写」授权（只有用户点两次确认后才能打开）。
pub fn set_write_access(root: &Path, id: &str, allow: bool) -> String {
    let dir = dir(root);
    if let Err(reason) = std::fs::create_dir_all(&dir) {
        return json!({ "ok": false, "error": format!("创建 {} 失败：{reason}", dir.display()) })
            .to_string();
    }
    let _lock = match acquire_store_lock(&dir, "cron.lock") {
        Ok(lock) => lock,
        Err(reason) => return json!({ "ok": false, "error": reason }).to_string(),
    };
    let mut value: Value = match std::fs::read_to_string(definitions_path(root)) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(e) => {
                return json!({ "ok": false, "error": format!("cron.json 解析失败：{e}（拒绝改写）") })
                    .to_string()
            }
        },
        Err(_) => return json!({ "ok": false, "error": "cron.json 不存在".to_string() }).to_string(),
    };
    let jobs = if let Some(jobs) = value.get_mut("jobs").and_then(Value::as_array_mut) {
        jobs
    } else if value.as_array().is_some() {
        value = json!({ "version": 1, "jobs": value });
        value["jobs"].as_array_mut().expect("刚包成 jobs 数组")
    } else {
        return json!({ "ok": false, "error": "cron.json 里没有 jobs 数组" }).to_string();
    };
    let mut found = false;
    for job in jobs.iter_mut() {
        if job.get("id").and_then(Value::as_str) == Some(id.trim()) {
            job["writeAccess"] = json!(allow);
            found = true;
        }
    }
    if !found {
        return json!({ "ok": false, "error": "找不到这条定时任务" }).to_string();
    }
    match write_json_atomic(&definitions_path(root), &value) {
        Ok(()) => json!({ "ok": true, "error": "", "writeAccess": allow }).to_string(),
        Err(reason) => json!({ "ok": false, "error": reason }).to_string(),
    }
}

/// 启用 / 停用一条定时任务（前端入口，§待办 3-28.16）。
///
/// 只写定义的 `enabled`；`enabled=true` 时**顺带清掉**运行态里的「自动停用」与连续失败
/// 计数，并按当前时刻重排下一次 —— 否则被自动停用的 job 再怎么点「启用」也不会跑
/// （`should_run` 同时要求 `enabled && !autoDisabled`）。
pub fn set_enabled(root: &Path, id: &str, enabled: bool) -> String {
    let id = id.trim();
    if id.is_empty() {
        return json!({ "ok": false, "error": "缺少任务 id" }).to_string();
    }
    let dir = dir(root);
    if let Err(reason) = std::fs::create_dir_all(&dir) {
        return json!({ "ok": false, "error": format!("创建 {} 失败：{reason}", dir.display()) })
            .to_string();
    }
    // 定义文件的锁必须在 `update_state` 之前释放：两者用的是同一个 `cron.lock`，
    // 嵌套获取会等自己的锁直到超时。
    let written = (|| -> Result<(), String> {
        let _lock = acquire_store_lock(&dir, "cron.lock")?;
        let mut value: Value = match std::fs::read_to_string(definitions_path(root)) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(value) => value,
                Err(e) => {
                    return Err(format!("cron.json 解析失败：{e}（拒绝改写）"));
                }
            },
            Err(_) => return Err("cron.json 不存在".to_string()),
        };
        let jobs = if let Some(jobs) = value.get_mut("jobs").and_then(Value::as_array_mut) {
            jobs
        } else if value.as_array().is_some() {
            value = json!({ "version": 1, "jobs": value });
            value["jobs"].as_array_mut().expect("刚包成 jobs 数组")
        } else {
            return Err("cron.json 里没有 jobs 数组".to_string());
        };
        let mut found = false;
        for job in jobs.iter_mut() {
            if job.get("id").and_then(Value::as_str) == Some(id) {
                job["enabled"] = json!(enabled);
                found = true;
            }
        }
        if !found {
            return Err("找不到这条定时任务".to_string());
        }
        write_json_atomic(&definitions_path(root), &value)
    })();
    if let Err(reason) = written {
        return json!({ "ok": false, "error": reason }).to_string();
    }
    if enabled {
        let job = read_jobs(root)
            .0
            .into_iter()
            .find(|job| job.id == id);
        if let Some(job) = job {
            let now = now_ms();
            let next = next_run_at(&job.schedule, now, now, local_offset_minutes());
            let _ = update_state(
                root,
                &job.id,
                json!({
                    "autoDisabled": false,
                    "consecutiveFailures": 0,
                    "nextRunAt": next,
                }),
            );
        }
    }
    json!({ "ok": true, "error": "", "enabled": enabled }).to_string()
}

/// 单次执行的结果（摘要 / 错误写进运行态）。
struct Outcome {
    status: &'static str,
    summary: String,
    error: String,
}

/// 定时任务单发**永远裁掉**的工具（v1 口径：写类 + 命令类 + 外部出口 + 子代理 + Cron*）。
///
/// 理由：无人值守没有审批回包通道 —— 能花钱 / 能出网 / 能派生任务的工具一律不给。
/// `writeAccess` 只放行写类三件（见 [`CRON_WRITE_TOOLS`]），命令类与外网工具照旧裁。
///
/// `pub(crate)`：任务队列（`task_queue.rs`）的只读单发用的是**同一张**底表，
/// 只额外多裁 `Task*` 三件 —— 两张手抄的名单必然漂移（`forbidden.md` §禁用 62）。
pub(crate) const CRON_DISALLOWED: &[&str] = &[
    "Write",
    "Edit",
    "NotebookEdit",
    "Cmd",
    "PowerShell",
    "ImageGen",
    "EnterWorktree",
    "ExitWorktree",
    "Remember",
    "PetGrowth",
    "PetPersonaSuggest",
    "WebSearch",
    "WebFetch",
    "AskUserQuestion",
    "Agent",
    "CronCreate",
    "CronDelete",
    "CronList",
];

/// 用户逐 job 打开 `writeAccess` 后才放行的写类三件（v1 阶段 2B 同口径）。
const CRON_WRITE_TOOLS: &[&str] = &["Write", "Edit", "NotebookEdit"];

fn run_agent(root: &Path, job: &Job) -> Result<Outcome, String> {
    let spec = crate::agent::launch_spec(root)?;
    let mut cmd = Command::new(&spec.exe);
    // query 走 **stdin**（v1 同口径）：命令行长度与引号都不再是风险。
    cmd.arg("--print")
        .args(["--output-format", "stream-json"]);
    if job.write_access {
        // 无人值守可写：只放行写类三件，命令类 / 外部工具仍被下面裁掉。
        cmd.args(["--permission-mode", "acceptEdits"])
            .arg("--unattended-allow")
            .arg("Write,Edit,NotebookEdit");
    } else {
        cmd.args(["--permission-mode", "plan"]);
    }
    cmd.arg("--add-dir").arg(&spec.workdir);
    // `--disallowedTools` 是 variadic ⇒ **必须放最后**，否则会把后面的 `--add-dir`
    // 及其值当成工具名吞进去（v1 踩过同一个坑）。
    cmd.arg("--disallowedTools");
    for name in CRON_DISALLOWED {
        if job.write_access && CRON_WRITE_TOOLS.contains(name) {
            continue;
        }
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
    // 写入 query 后关掉 stdin（drop）⇒ agent 读到 EOF，开始执行。
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write as _;
        let _ = stdin.write_all(job.prompt.as_bytes());
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
                if started.elapsed() > RUN_TIMEOUT {
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
    // `--output-format stream-json`：最后一条 `{"type":"result",...}` 就是本轮收口。
    let parsed = stdout_text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("result"))
        .next_back()
        .unwrap_or(Value::Null);
    // 日志落 `temp/logs/cron-<id>-<epoch>.log`（失败也留证据）。落盘前脱敏 ——
    // agent 的 stdout 里可能带 key / token（与 v1 的 `mask_secrets` 同口径）。
    let log_dir = root.join("temp").join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join(format!("cron-{}-{}.log", job.id, now_ms()));
    let _ = std::fs::write(
        &log_path,
        format!(
            "job: {}\nschedule: {}\nwriteAccess: {}\n--- stdout ---\n{}\n--- stderr ---\n{}\n",
            job.id,
            job.schedule.text(),
            job.write_access,
            crate::security_ledger::mask_secrets(&stdout_text),
            crate::security_ledger::mask_secrets(&stderr_text),
        ),
    );

    let Some(status) = status else {
        return Ok(Outcome {
            status: "failed",
            summary: String::new(),
            error: format!("超过 {} 秒被强杀（日志：{}）", RUN_TIMEOUT.as_secs(), log_path.display()),
        });
    };
    // 用量以 `source="cron"` 记一档（v1：cron 的花费不混进普通聊天）。
    record_usage(root, &spec.model, &parsed, now_ms());
    let text = parsed
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let is_error = parsed
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || !status.success();
    if is_error {
        let error = if text.is_empty() {
            format!(
                "退出码 {}（日志：{}）",
                status.code().unwrap_or(-1),
                log_path.display()
            )
        } else {
            text.chars().take(400).collect()
        };
        Ok(Outcome {
            status: "failed",
            summary: String::new(),
            error,
        })
    } else {
        Ok(Outcome {
            status: "ok",
            summary: text.chars().take(400).collect(),
            error: String::new(),
        })
    }
}

fn execute(root: &Path, job: &Job) -> Result<Outcome, String> {
    run_agent(root, job)
}

/// 把单发的 `result.usage` 记进用量日志（`source="cron"`，与 v1 同口径）。
///
/// 记账失败只留一行 stderr —— 记账不该影响任务本身。
///
/// `pub(crate)`：任务队列（`task_queue.rs`）的单发也记这一档（v1 口径：队列用量归 cron 分区）。
pub(crate) fn record_usage(root: &Path, model: &str, parsed: &Value, ts: u64) {
    let Some(usage) = parsed.get("usage") else {
        return;
    };
    if !usage.is_object() {
        return;
    }
    let pick = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let mut record = json!({
        "ts": ts,
        "model": model,
        "source": "cron",
        "input": pick("input_tokens"),
        "output": pick("output_tokens"),
        "cacheRead": pick("cache_read_input_tokens"),
        "cacheCreate": pick("cache_creation_input_tokens"),
    });
    for (key, field) in [("requests", "requests"), ("subagent", "subagent"), ("review", "review")] {
        if let Some(value) = usage.get(key) {
            if value.is_array() || value.is_object() {
                record[field] = value.clone();
            }
        }
    }
    let (date, _) = local_stamp(ts);
    if let Err(reason) = crate::usage::append(root, &date, &record) {
        eprintln!("[lunac-cron] 记用量失败：{reason}");
    }
}

/// 到期但延迟超过 [`MISSED_GRACE_MS`] ⇒ 这一拍已错过，记 `skipped_missed` 而不是补跑。
///
/// v1 口径：睡过只补「最近一次且延迟 ≤10 分钟」，更久就放弃这一拍（避免一开机连烧）。
pub fn is_missed(planned: u64, now: u64) -> bool {
    now > planned.saturating_add(MISSED_GRACE_MS)
}

/// 把一次执行结果写进运行态（`allow_auto_disable=false` 时不动自动停用标记）。
fn record_outcome(root: &Path, job: &Job, outcome: &Outcome, allow_auto_disable: bool) -> Value {
    let state = read_state(root);
    let entry = state_entry(&state, &job.id);
    let today = local_stamp(now_ms()).0;
    let same_day = entry.get("todayDate").and_then(Value::as_str) == Some(today.as_str());
    let today_runs = if same_day {
        increment(&entry, "todayRuns")
    } else {
        1
    };
    let mut consecutive = if outcome.status == "ok" {
        0
    } else {
        increment(&entry, "consecutiveFailures")
    };
    let auto_disabled = allow_auto_disable
        && outcome.status != "ok"
        && consecutive >= AUTO_DISABLE_AFTER
        && job.enabled;
    if auto_disabled {
        consecutive = 0;
    }
    let completed = now_ms();
    let next = next_run_at(
        &job.schedule,
        completed,
        completed,
        local_offset_minutes(),
    );
    let patch = json!({
        "lastRunAt": completed,
        "lastStatus": outcome.status,
        "lastSummary": outcome.summary,
        "lastError": outcome.error,
        "runCount": increment(&entry, "runCount"),
        "failureCount": if outcome.status == "ok" {
            entry.get("failureCount").and_then(Value::as_u64).unwrap_or(0)
        } else {
            increment(&entry, "failureCount")
        },
        "consecutiveFailures": consecutive,
        "autoDisabled": auto_disabled || entry.get("autoDisabled").and_then(Value::as_bool).unwrap_or(false),
        "todayDate": today,
        "todayRuns": today_runs,
        "nextRunAt": next,
    });
    update_state(root, &job.id, patch).unwrap_or_else(|e| {
        eprintln!("[lunac-cron] 写运行态失败：{e}");
        json!({})
    })
}

/// 记一次「跳过」：不打日志、不写摘要，只推进 `nextRunAt`。
fn record_skip(root: &Path, job: &Job, entry: &Value, status: &str) {
    let now = now_ms();
    let next = next_run_at(&job.schedule, now, now, local_offset_minutes());
    let _ = update_state(
        root,
        &job.id,
        json!({
            "lastStatus": status,
            "nextRunAt": next,
            "runCount": entry.get("runCount").cloned().unwrap_or_else(|| json!(0)),
        }),
    );
}

/// 起调度线程（只起一次）。App 启动时调一次即可，重复调用是无操作。
pub fn start_scheduler(root: PathBuf) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || loop {
        tick(&root);
        std::thread::sleep(TICK);
    });
}

fn tick(root: &Path) {
    write_heartbeat(root);
    let (jobs, tag) = read_jobs(root);
    if tag != "ok" {
        return;
    }
    let state = read_state(root);
    let now = now_ms();
    for job in jobs {
        let entry = state_entry(&state, &job.id);
        let auto_disabled = entry
            .get("autoDisabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let today = local_stamp(now).0;
        let same_day = entry.get("todayDate").and_then(Value::as_str) == Some(today.as_str());
        let today_runs = if same_day {
            entry.get("todayRuns").and_then(Value::as_u64).unwrap_or(0)
        } else {
            0
        };
        let planned = entry.get("nextRunAt").and_then(Value::as_u64);
        let Some(planned) = planned else {
            // 首次见到：从「现在」排下一拍，不立刻跑（App 刚打开不该突然烧 API）。
            if job.enabled {
                let anchor = if job.created_at > 0 { job.created_at } else { now };
                let next = next_run_at(&job.schedule, anchor, now, local_offset_minutes());
                let _ = update_state(root, &job.id, json!({ "nextRunAt": next }));
            }
            continue;
        };
        if !should_run(
            job.enabled,
            auto_disabled,
            today_runs,
            job.max_runs_per_day,
            Some(planned),
            now,
        ) {
            continue;
        }
        // 到点了但已经错过太久（App 关了一夜）：只推进到下一拍，不补跑。
        if is_missed(planned, now) {
            record_skip(root, &job, &entry, "skipped_missed");
            continue;
        }
        if BUSY.load(Ordering::SeqCst) {
            record_skip(root, &job, &entry, "skipped_busy");
            continue;
        }
        match try_acquire_run() {
            Some(_guard) => {
                let outcome = execute(root, &job);
                match outcome {
                    Ok(outcome) => {
                        record_outcome(root, &job, &outcome, true);
                    }
                    Err(reason) => {
                        let outcome = Outcome {
                            status: "failed",
                            summary: String::new(),
                            error: reason,
                        };
                        record_outcome(root, &job, &outcome, true);
                    }
                }
            }
            None => record_skip(root, &job, &entry, "skipped_busy"),
        }
    }
}

/// 供单测：判断此刻该不该跑（纯函数，不碰磁盘）。
pub fn should_run(
    enabled: bool,
    auto_disabled: bool,
    today_runs: u64,
    max_runs_per_day: u64,
    planned: Option<u64>,
    now: u64,
) -> bool {
    enabled
        && !auto_disabled
        && today_runs < max_runs_per_day
        && planned.map(|ms| ms <= now).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lunac-cron-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ModuleData").join("tasks")).unwrap();
        dir
    }

    #[test]
    fn every_schedule_advances_past_now() {
        let schedule = Schedule::Every {
            every_minutes: 30,
        };
        // 上一拍在 10 分钟前 ⇒ 下一拍还在未来（不补跑）。
        let now = 1_700_000_000_000u64;
        assert_eq!(
            next_run_at(&schedule, now - 10 * 60_000, now, 480),
            now + 20 * 60_000
        );
        // 睡过了 3 小时 ⇒ 下一拍从现在算起（不连发）。
        assert_eq!(
            next_run_at(&schedule, now - 3 * 3_600_000, now, 480),
            now + 30 * 60_000
        );
    }

    #[test]
    fn daily_schedule_uses_local_wall_clock() {
        // 2025-11-15 00:00 UTC = 本地（+08:00）08:00。
        let midnight_utc = 1_763_164_800_000u64;
        let schedule = Schedule::Daily {
            hour: 9,
            minute: 30,
        };
        let next = next_run_at(&schedule, midnight_utc, midnight_utc, 480);
        let (date, time) = local_stamp(next);
        assert_eq!(date, "2025-11-15");
        assert_eq!(time, "09:30");
        // 已经过了 09:30 就排到明天。
        let after = midnight_utc + 3 * 3_600_000; // 本地 11:00
        let next = next_run_at(&schedule, after, after, 480);
        let (date, time) = local_stamp(next);
        assert_eq!(date, "2025-11-16");
        assert_eq!(time, "09:30");
    }

    #[test]
    fn cron_json_is_parsed_tolerantly() {
        let root = temp_root("parse");
        std::fs::write(
            definitions_path(&root),
            r#"{"version":1,"jobs":[
                {"id":"cron_a","prompt":"整理待办","schedule":{"kind":"every","everyMinutes":60},"enabled":true},
                {"id":"cron_b","prompt":"日报","schedule":{"kind":"daily","at":"09:05"},"maxRunsPerDay":99,"writeAccess":true},
                {"id":"","prompt":"坏","schedule":{"kind":"every","everyMinutes":60}},
                {"id":"cron_c","prompt":"坏档期","schedule":{"kind":"every","everyMinutes":5}},
                {"id":"cron_d","prompt":"坏时间","schedule":{"kind":"daily","at":"9:5"}}
            ]}"#,
        )
        .unwrap();
        let (jobs, tag) = read_jobs(&root);
        assert_eq!(tag, "ok");
        assert_eq!(jobs.len(), 2, "坏条目逐条跳过：{jobs:?}");
        assert_eq!(jobs[0].id, "cron_a");
        assert_eq!(jobs[1].max_runs_per_day, HARD_MAX_RUNS_PER_DAY, "上限夹取");
        assert!(jobs[1].write_access);

        // 裸数组也接受（与 agent 侧 `cron_read_defs` 同口径）。
        std::fs::write(
            definitions_path(&root),
            r#"[{"id":"cron_x","prompt":"p","schedule":{"kind":"every","everyMinutes":15}}]"#,
        )
        .unwrap();
        let (jobs, tag) = read_jobs(&root);
        assert_eq!(tag, "ok");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "cron_x");

        // 损坏 = invalid（不猜、不清空）。
        std::fs::write(definitions_path(&root), "{not json").unwrap();
        assert_eq!(read_jobs(&root).1, "invalid");
        std::fs::remove_file(definitions_path(&root)).unwrap();
        assert_eq!(read_jobs(&root).1, "missing");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn state_round_trip_and_view_merge() {
        let root = temp_root("state");
        std::fs::write(
            definitions_path(&root),
            r#"{"version":1,"jobs":[{"id":"cron_a","prompt":"p","schedule":{"kind":"every","everyMinutes":60}}]}"#,
        )
        .unwrap();
        let (jobs, _) = read_jobs(&root);
        let job = &jobs[0];
        let outcome = Outcome {
            status: "ok",
            summary: "完成".to_string(),
            error: String::new(),
        };
        let entry = record_outcome(&root, job, &outcome, true);
        assert_eq!(entry["lastStatus"], "ok");
        assert_eq!(entry["runCount"], 1);
        assert!(entry["nextRunAt"].as_u64().unwrap() > now_ms());
        let view: Value = serde_json::from_str(&list_json(&root)).unwrap();
        assert_eq!(view["ok"], true);
        assert_eq!(view["count"], 1);
        assert_eq!(view["jobs"][0]["lastStatus"], "ok");
        assert_eq!(view["jobs"][0]["scheduleText"], "每 60 分钟");
        assert!(!view["jobs"][0]["nextRunAtLocal"].as_str().unwrap().is_empty());
        // 失败累计到阈值 ⇒ 自动停用。
        for _ in 0..AUTO_DISABLE_AFTER {
            let outcome = Outcome {
                status: "failed",
                summary: String::new(),
                error: "boom".to_string(),
            };
            record_outcome(&root, job, &outcome, true);
        }
        let view: Value = serde_json::from_str(&list_json(&root)).unwrap();
        assert_eq!(view["jobs"][0]["autoDisabled"], true);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_access_toggle_only_touches_that_job() {
        let root = temp_root("write");
        std::fs::write(
            definitions_path(&root),
            r#"{"version":1,"jobs":[
                {"id":"cron_a","prompt":"a","schedule":{"kind":"every","everyMinutes":60}},
                {"id":"cron_b","prompt":"b","schedule":{"kind":"every","everyMinutes":90}}
            ]}"#,
        )
        .unwrap();
        let result: Value = serde_json::from_str(&set_write_access(&root, "cron_b", true)).unwrap();
        assert_eq!(result["ok"], true);
        let (jobs, _) = read_jobs(&root);
        assert!(!jobs[0].write_access);
        assert!(jobs[1].write_access);
        let missing: Value = serde_json::from_str(&set_write_access(&root, "nope", true)).unwrap();
        assert_eq!(missing["ok"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 前端「启用 / 停用」：写定义；重新启用时清掉自动停用与连续失败，并重排下一次。
    #[test]
    fn enable_toggle_writes_the_definition_and_resets_auto_disable() {
        let root = temp_root("enable");
        std::fs::write(
            definitions_path(&root),
            r#"{"version":1,"jobs":[
                {"id":"cron_a","prompt":"a","schedule":{"kind":"every","everyMinutes":60}},
                {"id":"cron_b","prompt":"b","schedule":{"kind":"every","everyMinutes":90},"enabled":false}
            ]}"#,
        )
        .unwrap();
        // 造一份「自动停用」运行态：只有 enabled 位被清掉时它才会真的不跑。
        update_state(
            &root,
            "cron_b",
            json!({"autoDisabled": true, "consecutiveFailures": 3}),
        )
        .unwrap();

        let on: Value = serde_json::from_str(&set_enabled(&root, "cron_b", true)).unwrap();
        assert_eq!(on["ok"], true);
        let (jobs, _) = read_jobs(&root);
        assert!(jobs[1].enabled);
        let state = read_state(&root);
        assert_eq!(state["jobs"]["cron_b"]["autoDisabled"], false);
        assert_eq!(state["jobs"]["cron_b"]["consecutiveFailures"], 0);
        assert!(state["jobs"]["cron_b"]["nextRunAt"].as_u64().unwrap() > 0);

        let off: Value = serde_json::from_str(&set_enabled(&root, "cron_a", false)).unwrap();
        assert_eq!(off["ok"], true);
        let (jobs, _) = read_jobs(&root);
        assert!(!jobs[0].enabled);

        let missing: Value = serde_json::from_str(&set_enabled(&root, "nope", true)).unwrap();
        assert_eq!(missing["ok"], false);
        let blank: Value = serde_json::from_str(&set_enabled(&root, "  ", true)).unwrap();
        assert_eq!(blank["ok"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn should_run_respects_busy_gate_inputs() {
        let now = 1_000_000u64;
        assert!(should_run(true, false, 0, 12, Some(now - 1), now));
        assert!(!should_run(false, false, 0, 12, Some(now - 1), now));
        assert!(!should_run(true, true, 0, 12, Some(now - 1), now), "自动停用");
        assert!(!should_run(true, false, 12, 12, Some(now - 1), now), "当天额度用完");
        assert!(!should_run(true, false, 0, 12, Some(now + 1), now), "还没到点");
        assert!(!should_run(true, false, 0, 12, None, now), "首次见到不立刻跑");
    }

    #[test]
    fn missed_runs_beyond_the_grace_window_are_skipped() {
        let now = 1_700_000_000_000u64;
        assert!(!is_missed(now, now), "准点不算错过");
        assert!(!is_missed(now - MISSED_GRACE_MS, now), "宽限内照常补跑");
        assert!(is_missed(now - MISSED_GRACE_MS - 1, now), "超出 10 分钟只推进不补跑");
    }

    #[test]
    fn usage_is_logged_with_the_cron_source() {
        let root = temp_root("usage");
        let ts = 1_763_164_800_000u64;
        record_usage(
            &root,
            "deepseek-flash",
            &json!({
                "type": "result",
                "usage": {
                    "input_tokens": 12,
                    "output_tokens": 34,
                    "cache_read_input_tokens": 5,
                    "cache_creation_input_tokens": 0,
                    "requests": [{"in": 12, "out": 34, "read": 5, "create": 0}],
                },
            }),
            ts,
        );
        let (date, _) = local_stamp(ts);
        let records = crate::usage::read_log(&root, &date);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source, "cron");
        assert_eq!(records[0].model, "deepseek-flash");
        assert_eq!(records[0].input, 12);
        assert_eq!(records[0].output, 34);
        assert_eq!(records[0].cache_read, 5);
        assert_eq!(records[0].requests.len(), 1);
        // 没有 usage 的 result（被强杀 / 解析失败）不写空账。
        record_usage(&root, "deepseek-flash", &json!({"type": "result"}), ts);
        assert_eq!(crate::usage::read_log(&root, &date).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
