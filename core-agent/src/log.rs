// core-agent/src/log.rs
// 极简落盘日志（零第三方依赖）。
//
// 为什么需要它：release 版是 GUI 子系统、没有控制台，`eprintln!` 全部丢失；
// 宿主 src-tauri 只把 stderr 转发给前端，也不落盘 —— 一旦线上出问题（例如
// 某个工具报错），事后没有任何东西可查。本模块把关键事件写到文件里。
//
// 位置：`LUNAC_LOG_DIR`（宿主 src-tauri 注入 = `<exe 根>\temp\logs`）；
//       未注入时回退 `<agent.exe 所在目录>\temp\logs`（独立烟测场景）。
// 文件：`<prefix>-YYYY-MM-DD.log`（日期按 **UTC**，跨天自动换文件）。
// 开关：`LUNAC_LOG=off` 彻底关闭；`LUNAC_LOG_LEVEL=error|warn|info|debug`（默认 info）。
// 保留：启动时清理 7 天前的 `*.log`。
//
// 不引第三方日志框架（项目约定不新增依赖，见 docs/ai-spec.md §11）：
// 这里只需要「一行一条 + 级别 + UTC 时间戳 + 追加写」，手写远小于引依赖的代价。
// 时间戳是 UTC 而非本地时间：本地时区需要时区表，标准库没有，项目也不为此引 chrono。
//
// 与 `app/src-tauri/src/log.rs` 是**刻意的两份实现**（两个独立进程、各自解析自己的
// 数据根目录），改这里请同步改那边，反之亦然。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

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
/// 单条消息截断上限：debug 级会整段记录工具输出，不设上限能把日志撑爆
const MAX_MSG_CHARS: usize = 8_000;

fn sink() -> &'static Mutex<Option<(String, File)>> {
    SINK.get_or_init(|| Mutex::new(None))
}

// ── 目录与时间 ───────────────────────────────────────────────────

/// 日志目录：`LUNAC_LOG_DIR` 优先（宿主注入），否则 `<exe 目录>\temp\logs`。
pub fn log_dir() -> PathBuf {
    if let Ok(d) = std::env::var("LUNAC_LOG_DIR") {
        let d = d.trim();
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("temp")
        .join("logs")
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

pub fn debug(msg: impl AsRef<str>) {
    log(Level::Debug, msg);
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

/// FNV-1a 64 位哈希（调用方自己格式化成 16 位十六进制）。
///
/// 用途：给每次 API 请求的**固定前缀**留指纹（ai-spec §11 规则 23 的归因埋点）。
/// 为什么不用真哈希库：本仓库不引依赖（与规则 20 同因），而这里只需要「两段文本
/// 是否逐字节相同」这一个性质 —— FNV-1a 足够，且**只输出哈希、不输出原文**，
/// 天然满足「进日志的字符串必须脱敏」这条硬要求（前缀里可能含用户文件内容）。
pub fn hash64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
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

// ── API 报文调试转储（LUNAC_DUMP_API，默认关）────────────────────────
//
// 目的：现有可观测性只有 usage 级（result.usage.requests[] + usage_delta + agent-*.log），
// 看不到「端点到底收了什么、回了什么」。本转储把每次 API 调用的报文各写一行，供 AI 侧
// 端到端 / 混测取证。
//
// 开关：LUNAC_DUMP_API 为真值（1 / true / on / yes，忽略大小写与首尾空白）才开；缺省关。
// 关闭时调用方先经 dump_api_enabled() 短路，热路径不做任何序列化 / 字符串拼接。
//
// 落点：log_dir() 下的 api-dump.jsonl（目录解析与 agent-*.log 完全一致，见 log_dir()），
// 一行一条 JSON，便于 grep / jq。
//
// 硬要求（见 docs/forbidden.md §禁用 6）：
//   · 绝不落凭据 —— body 一律先过 mask_secrets() 再写；
//   · 请求头一个都不记（连头名也不记）—— Authorization / x-api-key 这类承载 key 的头是
//     本通道唯一的敏感面，记头名换不来排障价值，却要逐项论证「该头不含凭据」；一个头
//     都不记是最省心的安全选择；
//   · 单条 body 截断到 MAX_DUMP_BODY_CHARS 字符，超出补 truncated 与真实字符数。
//
// 失败纪律：写失败只 warn、绝不影响主流程（同 review-last 的纪律）。

/// 单条 body 落盘前的字符上限。取值理由：一次主请求的 history 可达数十万字符，不设上限
/// 会把磁盘写爆；64K 字符足以看清绝大多数单请求报文的结构（含工具 schema 与多轮历史），
/// 越过部分由 truncated 与 body_chars 交代。
const MAX_DUMP_BODY_CHARS: usize = 65_536;

/// 调试转储是否开启。进程内读一次并缓存（env 在进程生命周期内不变）。
pub fn dump_api_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| is_truthy(&std::env::var("LUNAC_DUMP_API").unwrap_or_default()))
}

/// env 布尔风格：1 / true / on / yes（忽略大小写与首尾空白）为真。
/// `pub(crate)`：`decide.rs` 的 `LUNAC_LOCAL_DECIDE` 开关复用同一口径（单一真相源）。
pub(crate) fn is_truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on" | "yes"
    )
}

/// 写一行 API 转储。kind = request / response；status 仅 response 有（None = 网络层没
/// 拿到响应，此时 error 给失败原因）；body 是本次报文（request = 请求体原文，
/// response = 累积后的最终报文，流式响应的形态见调用点注释）。
///
/// 无论哪个分支都先过 mask_secrets()；写失败只 warn。本函数自带开关判断，关闭时立即返回。
pub fn dump_api(
    kind: &str,
    req_id: &str,
    model: &str,
    status: Option<u16>,
    error: Option<&str>,
    body: &Value,
) {
    if !dump_api_enabled() {
        return;
    }
    let raw = body.to_string();
    let total = raw.chars().count();
    let truncated = total > MAX_DUMP_BODY_CHARS;
    let masked = mask_secrets(&raw);
    let body_text = if truncated {
        truncate_chars(&masked, MAX_DUMP_BODY_CHARS)
    } else {
        masked
    };

    let mut obj = serde_json::Map::new();
    obj.insert("ts".into(), json!(utc_stamp()));
    obj.insert("kind".into(), json!(kind));
    obj.insert("req_id".into(), json!(req_id));
    obj.insert("model".into(), json!(model));
    obj.insert("status".into(), json!(status));
    if let Some(e) = error {
        obj.insert("error".into(), json!(mask_secrets(e)));
    }
    if truncated {
        obj.insert("truncated".into(), json!(true));
        obj.insert("body_chars".into(), json!(total));
    }
    obj.insert("body".into(), json!(body_text));
    let line = Value::Object(obj).to_string();

    let dir = log_dir();
    if let Err(e) = fs::create_dir_all(&dir) {
        warn(format!("API 转储写入失败（建目录 {}）：{e}", dir.display()));
        return;
    }
    let path = dir.join("api-dump.jsonl");
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut f) => {
            let _ = f.write_all(line.as_bytes());
            let _ = f.write_all(b"\n");
            let _ = f.flush();
        }
        Err(e) => warn(format!("API 转储写入失败（{}）：{e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 指纹的**唯一**用途是「同一段文本是否逐字节相同」，所以两个性质必须成立：
    /// 相同输入恒等、改一个字节即变（后者正是我们要抓的「前缀被改写」）。
    #[test]
    fn hash64_is_stable_and_sensitive() {
        assert_eq!(hash64(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash64("abc"), hash64("abc"));
        assert_ne!(hash64("abc"), hash64("abd"));
        assert_ne!(hash64("abc"), hash64("abc "));
    }

    #[test]
    fn mask_hides_json_key_values() {
        let s = r#"{"api_key":"sk-abcdef123456","model":"deepseek-flash"}"#;
        let masked = mask_secrets(s);
        assert!(!masked.contains("abcdef123456"), "secret leaked: {masked}");
        assert!(masked.contains("deepseek-flash"), "non-secret mangled: {masked}");
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

    #[test]
    fn civil_date_matches_known_epochs() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        assert_eq!(civil_from_days(20_709), (2026, 9, 13));
    }

    #[test]
    fn dump_flag_truthy_forms() {
        for v in ["1", "true", "TRUE", " on ", "Yes"] {
            assert!(is_truthy(v), "{v} 应为真");
        }
        for v in ["", "0", "false", "off", "no", "2"] {
            assert!(!is_truthy(v), "{v} 应为假");
        }
    }
}
