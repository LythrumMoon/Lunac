// src-tauri/src/log.rs
// 宿主（lunac.exe）侧的极简落盘日志（零第三方依赖）。
//
// 为什么需要它：release 是 GUI 子系统、没有控制台，`eprintln!` 全部丢失。
// agent.exe 的 stderr 原本只被转发给前端 + eprintln，进程一退什么也不剩；
// 宿主的启动/退出、agent 启停、前端 JS 报错同样无处可查。
//
// 位置：`<exe 根>\temp\logs\lunac-YYYY-MM-DD.log`（经 storage::lunac_root_dir()，
//       见 ai-spec §11 规则 7：数据落盘统一走 storage，禁止各自硬编码路径）。
//       可用 `LUNAC_LOG_DIR` 覆盖（agent 侧也读它，保证两个进程写同一目录）。
// 开关：`LUNAC_LOG=off` 彻底关闭；`LUNAC_LOG_LEVEL=error|warn|info|debug`（默认 info）。
// 保留：启动时清理 7 天前的 `*.log`。
//
// 不引第三方日志框架（项目约定不新增依赖，见 docs/ai-spec.md §11）；
// 时间戳是 UTC —— 本地时区需要时区表，标准库没有，项目也不为此引 chrono。
//
// 与 `core-agent/src/log.rs` 是**刻意的两份实现**（两个独立进程、各自解析自己的
// 数据根目录），改这里请同步改那边，反之亦然。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy)]
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

/// 0 = 关闭；其余为最低记录级别（数值越大越啰嗦）
static MIN_LEVEL: AtomicU8 = AtomicU8::new(0);
/// (文件对应的 UTC 日期, 句柄)；None = 尚未打开或打开失败
static SINK: OnceLock<Mutex<Option<(String, File)>>> = OnceLock::new();
static PREFIX: OnceLock<String> = OnceLock::new();

const KEEP_DAYS: u64 = 7;
/// 单条消息截断上限：debug 级会整段记录输出，不设上限能把日志撑爆
const MAX_MSG_CHARS: usize = 8_000;

fn sink() -> &'static Mutex<Option<(String, File)>> {
    SINK.get_or_init(|| Mutex::new(None))
}

// ── 目录与时间 ───────────────────────────────────────────────────

/// 日志目录：`LUNAC_LOG_DIR` 优先，否则 `<exe 根>\temp\logs`（走 storage 统一解析）。
pub fn log_dir() -> PathBuf {
    if let Ok(d) = std::env::var("LUNAC_LOG_DIR") {
        let d = d.trim();
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    crate::storage::lunac_root_dir().join("temp").join("logs")
}

/// epoch 秒 → (天数, 时, 分, 秒)
fn split_secs(secs: u64) -> (i64, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = (secs % 86_400) as u32;
    (days, rem / 3_600, (rem % 3_600) / 60, rem % 60)
}

/// Howard Hinnant 的 civil_from_days：epoch 天数 → (年, 月, 日)。
/// 无时区表、无闰秒，对日志时间戳足够（UTC）。
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
    (y + i64::from(m <= 2), m, d)
}

/// 当前 UTC：(年, 月, 日, 时, 分, 秒)
fn now_utc() -> (i64, u32, u32, u32, u32, u32) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, h, mi, s) = split_secs(secs);
    let (y, mo, d) = civil_from_days(days);
    (y, mo, d, h, mi, s)
}

fn utc_date() -> String {
    let (y, mo, d, ..) = now_utc();
    format!("{y:04}-{mo:02}-{d:02}")
}

fn utc_stamp() -> String {
    let (y, mo, d, h, mi, s) = now_utc();
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

// ── 初始化 ───────────────────────────────────────────────────────

/// 进程启动时调用一次。`prefix` 同时作为文件名前缀与日志内的进程名。
pub fn init(prefix: &str) {
    let _ = PREFIX.set(prefix.to_string());
    let level = requested_level();
    MIN_LEVEL.store(level, Ordering::Relaxed);
    if level == 0 {
        return;
    }

    let dir = log_dir();
    if let Err(e) = fs::create_dir_all(&dir) {
        // 打不开就彻底关掉，避免每行都尝试一次并刷屏
        MIN_LEVEL.store(0, Ordering::Relaxed);
        eprintln!("[log] cannot create {}: {e}", dir.display());
        return;
    }
    purge_old(&dir);
    ensure_open(&dir);
    install_panic_hook();

    info(format!(
        "=== {prefix} start pid={} exe={} cwd={} ===",
        std::process::id(),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "?".into()),
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "?".into()),
    ));
}

fn requested_level() -> u8 {
    if let Ok(v) = std::env::var("LUNAC_LOG") {
        let v = v.trim();
        if v.eq_ignore_ascii_case("off") || v == "0" {
            return 0;
        }
    }
    match std::env::var("LUNAC_LOG_LEVEL")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "off" | "none" => 0,
        "error" => Level::Error as u8,
        "warn" => Level::Warn as u8,
        "debug" => Level::Debug as u8,
        _ => Level::Info as u8,
    }
}

fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        error(format!("PANIC: {info}"));
        default(info);
    }));
}

fn path_for(dir: &Path, date: &str) -> PathBuf {
    let prefix = PREFIX.get().map(String::as_str).unwrap_or("lunac");
    dir.join(format!("{prefix}-{date}.log"))
}

/// 确保当前句柄指向「今天的」文件；跨天时换文件。
fn ensure_open(dir: &Path) {
    let date = utc_date();
    let mut guard = match sink().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.as_ref().map(|(d, _)| d.as_str()) == Some(date.as_str()) {
        return;
    }
    let path = path_for(dir, &date);
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => *guard = Some((date, f)),
        Err(e) => {
            MIN_LEVEL.store(0, Ordering::Relaxed);
            eprintln!("[log] open {} failed: {e}", path.display());
        }
    }
}

/// 清理超过 KEEP_DAYS 的日志（按文件修改时间）。失败一律忽略 —— 日志清理
/// 不该影响主流程。
fn purge_old(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let Some(cutoff) = SystemTime::now().checked_sub(Duration::from_secs(KEEP_DAYS * 86_400))
    else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if modified < cutoff {
            let _ = fs::remove_file(&path);
        }
    }
}

// ── 写入 ─────────────────────────────────────────────────────────

pub fn enabled(level: Level) -> bool {
    let min = MIN_LEVEL.load(Ordering::Relaxed);
    min != 0 && (level as u8) <= min
}

pub fn log(level: Level, msg: impl AsRef<str>) {
    if !enabled(level) {
        return;
    }
    let msg = msg.as_ref();
    let dir = log_dir();
    let date = utc_date();
    let stamp = utc_stamp();
    let tag = level.tag();

    let mut guard = match sink().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.as_ref().map(|(d, _)| d.as_str()) != Some(date.as_str()) {
        drop(guard);
        ensure_open(&dir);
        guard = match sink().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
    }
    let Some((_, file)) = guard.as_mut() else {
        return;
    };

    // 多行消息逐行加前缀，方便直接 grep 单个事件
    let body = truncate_chars(msg, MAX_MSG_CHARS);
    let mut buf = String::with_capacity(body.len() + 64);
    for line in body.lines() {
        buf.push_str(&format!("{stamp} {tag} {line}\n"));
    }
    if buf.is_empty() {
        buf.push_str(&format!("{stamp} {tag} \n"));
    }
    let _ = file.write_all(buf.as_bytes());
    // 每行都 flush：崩溃/被杀进程时也要留住最后一条线索
    let _ = file.flush();
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

// ── 工具 ─────────────────────────────────────────────────────────

/// 按 UTF-8 边界截断到 `max` 个字符。
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("…(truncated)");
    out
}

/// 凭据的**末 4 位**（空串 → `(empty)`；不足 5 位 → `****`，太短就没有「末 4 位」可言）。
///
/// 用途：日志里分辨「现在生效的是哪一把 key」。真实的坑是**改了凭据不生效**
/// （`.env` 与另一份配置各执一词，2026-09-15 的 401 就是这么来的）——
/// 有末 4 位就足以定位是哪一份在生效，而又不足以复原整把 key。
///
/// 调用方请把它写成 `key_tail=d423` 这种标签再拼进日志：`SECRET_KEYS` 只认
/// `api_key` / `token` / `secret` 等**精确**键名，`key_tail` 不会被 `mask_secrets`
/// 二次打码（也正因如此才留得住末 4 位）。
pub fn key_tail(secret: &str) -> String {
    let s = secret.trim();
    if s.is_empty() {
        return "(empty)".to_string();
    }
    let n = s.chars().count();
    if n <= 4 {
        return "****".to_string();
    }
    s.chars().skip(n - 4).collect()
}

/// 敏感键名：命中后其「值」整体打码。判据是键名后（可含空格）紧跟取值符
/// （`:` / `=` / 引号），这样 `"max_tokens":8192` 里的 `token` 不会被误伤。
const SECRET_KEYS: [&str; 9] = [
    "api_key",
    "apikey",
    "api-key",
    "search_key",
    "token",
    "authorization",
    "password",
    "passwd",
    "secret",
];

/// 打日志前抹掉明显的凭据。日志常被用户贴出来求助，密钥不能跟着出门。
/// 覆盖三类：`sk-` 前缀的裸 key、`Bearer <token>`、`"api_key":"<value>"` 这类键值对。
pub fn mask_secrets(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        // ① 键值对 / Bearer：保留键名与分隔符，值整体替换成 ***
        if let Some(klen) = match_secret_key(&chars, i).or_else(|| match_bearer(&chars, i)) {
            for _ in 0..klen {
                out.push(chars[i]);
                i += 1;
            }
            while i < chars.len() && matches!(chars[i], ' ' | ':' | '=' | '"' | '\'') {
                out.push(chars[i]);
                i += 1;
            }
            i = value_end(&chars, i);
            out.push_str("***");
            continue;
        }
        // ② 裸 key（如命令行参数里的 sk-xxx）：保留 sk- 前缀便于辨认
        if let Some(value_start) = sk_value_start(&chars, i) {
            while i < value_start {
                out.push(chars[i]);
                i += 1;
            }
            i = value_end(&chars, i);
            out.push_str("sk-***");
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 从 `i` 起是否命中某个敏感键名（大小写不敏感），且其后紧跟取值符。
fn match_secret_key(chars: &[char], i: usize) -> Option<usize> {
    for key in SECRET_KEYS {
        let klen = key.chars().count();
        if i + klen > chars.len() {
            continue;
        }
        let candidate: String = chars[i..i + klen].iter().collect();
        if !candidate.eq_ignore_ascii_case(key) {
            continue;
        }
        let mut j = i + klen;
        while j < chars.len() && chars[j] == ' ' {
            j += 1;
        }
        if j < chars.len() && matches!(chars[j], ':' | '=' | '"' | '\'') {
            return Some(klen);
        }
    }
    None
}

/// 从 `i` 起是否是 `Bearer`（大小写不敏感）且后接空白 —— `Authorization: Bearer x`。
fn match_bearer(chars: &[char], i: usize) -> Option<usize> {
    const WORD: &str = "bearer";
    let klen = WORD.chars().count();
    if i + klen > chars.len() {
        return None;
    }
    let candidate: String = chars[i..i + klen].iter().collect();
    if !candidate.eq_ignore_ascii_case(WORD) {
        return None;
    }
    match chars.get(i + klen) {
        Some(c) if c.is_whitespace() => Some(klen),
        _ => None,
    }
}

/// `sk-` 裸 key 的起点（自动跳过前置引号），命中返回 `sk-` 中 `s` 的下标。
fn sk_value_start(chars: &[char], i: usize) -> Option<usize> {
    let mut j = i;
    if j < chars.len() && (chars[j] == '"' || chars[j] == '\'') {
        j += 1;
    }
    let rest: String = chars[j..chars.len().min(j + 3)].iter().collect();
    if rest.eq_ignore_ascii_case("sk-") {
        Some(j)
    } else {
        None
    }
}

/// 值的结束位置：空白、引号、逗号、右括号。
fn value_end(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() || matches!(c, '"' | '\'' | ',' | '}' | ']' | ';') {
            break;
        }
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_hides_json_key_values() {
        let s = r#"{"api_key":"sk-abcdef123456","model":"deepseek-flash"}"#;
        let masked = mask_secrets(s);
        assert!(!masked.contains("abcdef123456"), "secret leaked: {masked}");
        assert!(masked.contains("deepseek-flash"), "non-secret mangled: {masked}");
    }

    /// 末 4 位是「改了 key 却没生效」这类问题唯一的现场线索，必须留得住 ——
    /// 既不能被 `mask_secrets` 二次打码，太短的 key 也不能整段漏出去。
    #[test]
    fn key_tail_survives_masking_but_real_key_names_do_not() {
        assert_eq!(key_tail("sk-46a5ed7ca9e54cfd9603d6113e85d423"), "d423");
        assert_eq!(key_tail("   "), "(empty)");
        assert_eq!(key_tail("ab"), "****");

        assert_eq!(mask_secrets("key_tail=d423"), "key_tail=d423");
        assert_eq!(mask_secrets("api_key=d423"), "api_key=***");
    }

    #[test]
    fn mask_hides_bearer_and_bare_sk() {
        let masked = mask_secrets("Authorization: Bearer sk-live-99887766");
        assert!(!masked.contains("99887766"), "secret leaked: {masked}");

        let bare = mask_secrets(r#"{"search_key":"sk-live-11223344"}"#);
        assert!(!bare.contains("11223344"), "secret leaked: {bare}");
    }

    #[test]
    fn mask_keeps_ordinary_text() {
        let s = r#"tool PowerShell ok (12ms) args={"command":"Get-Date"}"#;
        assert_eq!(mask_secrets(s), s);
        // max_tokens 里的 token 不算键名（后面不是取值符）
        let t = r#"{"max_tokens":8192,"input_tokens":176}"#;
        assert_eq!(mask_secrets(t), t);
    }
}
