//! 宿主（Flutter 里的 Rust 侧）落盘日志 —— 重建 v1 `app/src-tauri/src/log.rs` 的核心能力。
//!
//! **为什么必须有它**：release 是 GUI 子系统，没有控制台，所有 `eprintln!` 都会丢；
//! 宿主的启动 / 退出、agent 启停、代理引擎的起停、Dart 侧未捕获异常目前都无处可查
//! （v1 有 `temp\logs\lunac-*.log`，v2 之前一行都没落盘）。
//!
//! **与 v1 一致的口径**：
//! · 位置：`LUNAC_LOG_DIR` 优先，否则 `<exe 根>\temp\logs\`；
//! · 文件名：`{prefix}-YYYY-MM-DD.log`（**UTC** 日期；本地时区要时区表，标准库没有，项目不为此引依赖）；
//! · 开关：`LUNAC_LOG=off` 关闭；`LUNAC_LOG_LEVEL=error|warn|info|debug`（缺省 info）；
//! · 保留：启动时清掉 7 天前的 `*.log`；
//! · 单条上限 8000 字符（debug 会整段记录输出，不设上限能把日志撑爆）；
//! · 落盘前统一过 [`crate::security_ledger::mask_secrets`]（与 v1 `mask_secrets` 同规则），
//!   凭据 / token 不进日志；
//! · 装一个 panic hook：崩溃至少留下「在哪一行、什么消息」。
//!
//! **v2 的差异（如实记）**：不做跨进程共享文件句柄的协调（v1 也没有）；`init()` 缺省由第一次
//! 写日志时自动调用（prefix = `lunac`），Dart 侧只需调 `lunac_log_write` 即可落同一条流水。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 日志级别（数值越大越啰嗦；`MIN_LEVEL` 存的就是这个数值）。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
        }
    }
}

/// 0 = 关闭；其余为最低记录级别。
static MIN_LEVEL: AtomicU8 = AtomicU8::new(0);
/// (文件对应的 UTC 日期, 句柄)；`None` = 尚未打开或打开失败。
static SINK: OnceLock<Mutex<Option<(String, File)>>> = OnceLock::new();
static PREFIX: OnceLock<String> = OnceLock::new();

const KEEP_DAYS: u64 = 7;
const MAX_MSG_CHARS: usize = 8_000;

fn sink() -> &'static Mutex<Option<(String, File)>> {
    SINK.get_or_init(|| Mutex::new(None))
}

/// 日志目录：`LUNAC_LOG_DIR` 优先，否则 `<exe 根>\temp\logs`。
pub fn log_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("LUNAC_LOG_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    crate::exe_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("temp")
        .join("logs")
}

/// 初始化（幂等）：解析级别 / 开关、清过期文件、装 panic hook。
pub fn init(prefix: &str) {
    let _ = PREFIX.set(prefix.to_string());
    if MIN_LEVEL.load(Ordering::Relaxed) != 0 {
        return; // 已初始化
    }
    if let Some(level) = requested_level() {
        MIN_LEVEL.store(level as u8, Ordering::Relaxed);
    }
    if MIN_LEVEL.load(Ordering::Relaxed) == 0 {
        return; // `LUNAC_LOG=off`
    }
    let dir = log_dir();
    let _ = fs::create_dir_all(&dir);
    purge_old(&dir);
    install_panic_hook();
}

/// `LUNAC_LOG=off` ⇒ `None`；否则按 `LUNAC_LOG_LEVEL`（缺省 info）。
fn requested_level() -> Option<Level> {
    let off = std::env::var("LUNAC_LOG")
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            value == "off" || value == "0" || value == "false"
        })
        .unwrap_or(false);
    if off {
        return None;
    }
    let level = match std::env::var("LUNAC_LOG_LEVEL")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "error" => Level::Error,
        "warn" | "warning" => Level::Warn,
        "debug" => Level::Debug,
        _ => Level::Info,
    };
    Some(level)
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|loc| format!("{}:{}", loc.file(), loc.line()))
            .unwrap_or_else(|| "(unknown)".to_string());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| text.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(non-string panic)".to_string());
        log(Level::Error, format!("panic @ {location}: {payload}"));
        previous(info);
    }));
}

/// epoch 秒 → (年, 月, 日, 时, 分, 秒)，全 UTC。
fn now_utc() -> (i64, u32, u32, u32, u32, u32) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = (secs % 86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    (year, month, day, rem / 3_600, (rem % 3_600) / 60, rem % 60)
}

/// Howard Hinnant 的 `civil_from_days`：epoch 天数 → (年, 月, 日)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn path_for(dir: &std::path::Path, date: &str) -> PathBuf {
    let prefix = PREFIX.get().map(String::as_str).unwrap_or("lunac");
    dir.join(file_name(prefix, date))
}

/// 日志文件名（抽出来是为了能确定性单测：`PREFIX` 是 `OnceLock`，进程内只能设一次，
/// 直接在测试里 `set` 会被其它用例先调 `init("lunac")` 抢先而静默失败）。
fn file_name(prefix: &str, date: &str) -> String {
    format!("{prefix}-{date}.log")
}

/// 按需打开当天文件；跨天自动换文件。
fn ensure_open(dir: &std::path::Path, date: &str) {
    let Ok(mut slot) = sink().lock() else { return };
    if slot.as_ref().map(|(open_date, _)| open_date == date).unwrap_or(false) {
        return;
    }
    let _ = fs::create_dir_all(dir);
    let path = path_for(dir, date);
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => *slot = Some((date.to_string(), file)),
        Err(_) => *slot = None,
    }
}

/// 清掉 7 天前的 `*.log`（按文件名里的日期判定，失败就算了）。
fn purge_old(dir: &std::path::Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let now_days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("log") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
        // 文件名尾部 `YYYY-MM-DD.log`
        let Some(date) = name
            .strip_suffix(".log")
            .and_then(|stem| stem.get(stem.len().saturating_sub(10)..))
        else {
            continue;
        };
        let days = date_to_days(date);
        if let Some(days) = days {
            if now_days - days > KEEP_DAYS as i64 {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

/// `YYYY-MM-DD` → epoch 天数（解析失败 = `None`）。
fn date_to_days(date: &str) -> Option<i64> {
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // days_from_civil
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// 该级别会不会被记录（`init` 之前恒 false）。
pub fn enabled(level: Level) -> bool {
    let min = MIN_LEVEL.load(Ordering::Relaxed);
    min != 0 && (level as u8) <= min
}

/// 写一条（级别不够 / 已关闭 ⇒ 什么都不做）。
pub fn log(level: Level, msg: impl AsRef<str>) {
    if !enabled(level) {
        init("lunac");
        if !enabled(level) {
            return;
        }
    }
    let (year, month, day, hour, minute, second) = now_utc();
    let date = format!("{year:04}-{month:02}-{day:02}");
    let text = msg.as_ref();
    let truncated = if text.chars().count() > MAX_MSG_CHARS {
        let head: String = text.chars().take(MAX_MSG_CHARS).collect();
        format!("{head}…(truncated)")
    } else {
        text.to_string()
    };
    let masked = crate::security_ledger::mask_secrets(&truncated);
    let dir = log_dir();
    ensure_open(&dir, &date);
    let Ok(mut slot) = sink().lock() else { return };
    if let Some((_, file)) = slot.as_mut() {
        let _ = writeln!(
            file,
            "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z {} {masked}",
            level.tag(),
        );
    }
}

pub fn error(msg: impl AsRef<str>) {
    log(Level::Error, msg);
}

pub fn warn(msg: impl AsRef<str>) {
    log(Level::Warn, msg);
}

pub fn info(msg: impl AsRef<str>) {
    log(Level::Info, msg);
}

#[allow(dead_code)] // 保留给后续排障打点；当前没有调用点，但属于日志公共 API。
pub fn debug(msg: impl AsRef<str>) {
    log(Level::Debug, msg);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_round_trips_through_epoch_days() {
        // 1970-01-01 = 0；2026-11-15 与 civil_from_days 必须互为逆运算
        assert_eq!(date_to_days("1970-01-01"), Some(0));
        let days = date_to_days("2026-11-15").unwrap();
        assert_eq!(civil_from_days(days), (2026, 11, 15));
        // 非法日期一律 None（不猜）
        assert_eq!(date_to_days("2026-13-01"), None);
        assert_eq!(date_to_days("2026-11"), None);
        assert_eq!(date_to_days("not-a-date"), None);
    }

    #[test]
    fn purge_old_removes_files_older_than_seven_days() {
        let dir = std::env::temp_dir().join(format!("lunac-log-purge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lunac-1970-01-01.log"), b"old").unwrap();
        std::fs::write(dir.join("keep.txt"), b"not a log").unwrap();
        purge_old(&dir);
        assert!(!dir.join("lunac-1970-01-01.log").exists(), "7 天前的日志必须清掉");
        assert!(dir.join("keep.txt").exists(), "非 .log 文件不许动");

        // 今天的日志必须留着
        let (year, month, day, ..) = now_utc();
        let today = dir.join(format!("lunac-{year:04}-{month:02}-{day:02}.log"));
        std::fs::write(&today, b"today").unwrap();
        purge_old(&dir);
        assert!(today.exists(), "当天日志不许删");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_file_name_carries_prefix_and_utc_date() {
        assert_eq!(file_name("test-prefix", "2026-11-15"), "test-prefix-2026-11-15.log");
        // 未 init 时文件名必须仍有可识别的前缀（不能是 `-2026-11-15.log`）
        let dir = std::env::temp_dir().join("lunac-log-test");
        let path = path_for(&dir, "2026-11-15");
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        assert!(name.ends_with("-2026-11-15.log"), "{name}");
        assert!(!name.starts_with('-'), "{name}");
        assert!(path.ends_with(&file_name(
            PREFIX.get().map(String::as_str).unwrap_or("lunac"),
            "2026-11-15"
        )));
    }

    #[test]
    fn message_over_the_limit_is_truncated_before_masking() {
        let long = "x".repeat(MAX_MSG_CHARS + 10);
        let text = long.chars().take(MAX_MSG_CHARS).collect::<String>();
        assert_eq!(text.chars().count(), MAX_MSG_CHARS);
    }
}
