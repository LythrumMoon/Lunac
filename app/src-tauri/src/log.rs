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
///
/// 补词（2026-10-13，`mask_secrets` 扩展）：`passphrase` / `secret_key` / `private_key` /
/// `client_secret` / `access_key` / `refresh_token` / `session_token` —— 都是「名字本身就
/// 指向机密」的常见键名。**取舍**：只用**整词**判定（键名后紧跟取值符才算，见 `match_secret_key`），
/// 且不加入 `key` / `id` / `client` 这类过泛的词（会把 `key=...`、`id=...` 正常数据打成码）。
/// 注：`secret_key=...` 这类**不能**靠更短的 `secret` 兜住（`secret` 后紧跟的是 `_` 不是取值符，
/// 判据要求取值符），所以必须单列。
const SECRET_KEYS: [&str; 16] = [
    "api_key",
    "apikey",
    "api-key",
    "search_key",
    "token",
    "authorization",
    "password",
    "passwd",
    "passphrase",
    "secret",
    "secret_key",
    "client_secret",
    "private_key",
    "access_key",
    "refresh_token",
    "session_token",
];

/// 打日志 / 写报文 / 落记忆前抹掉明显的凭据。日志常被用户贴出来求助，密钥不能跟着出门。
///
/// **纯字符串变换**：返回 `String`、无 `Result`、不新增失败路径；只把「明显的凭据值」打码成
/// `***`，其余字符（**含换行与缩进**）逐个保留 ⇒ 天然不破坏多行 bullet 的续行缩进。既有多处
/// 共用（日志 / API 报文 / 记忆落盘），故新模式的**共同取舍 = 尽量锚定形态**（前缀、`://…@`
/// 结构、`eyJ` 头等），避免把普通文本里的 `token` 一词打成码。
///
/// 覆盖六类：
///   ① 键值对 `"api_key":"<value>"` —— 保留键名与分隔符，值整体替换成 `***`；
///   ② `Bearer <token>` —— 同上，保留 `Bearer `；
///   ③ 裸 `sk-` key —— 保留 `sk-` 前缀；
///   ④ **PEM 私钥多行块** —— 保留 `BEGIN` / `END` 两行，中段每行内容换成 `***`（换行数不变）；
///   ⑤ **裸 JWT** —— `eyJ…` 三段 base64url 点分，保留 `eyJ` 头；
///   ⑥ **前缀型 token**（`ghp_` / `xoxb-` / `AIza` / `AKIA` 等）与**裸连接串**
///      `scheme://user:password@host`（保留 `scheme://user:` 与 `@host` 形状，只抹口令）。
pub fn mask_secrets(s: &str) -> String {
    // 先抹 PEM 私钥块（唯一一处**多行**结构，单独按行处理；非 PEM 文本逐字节原样返回）。
    let s = mask_pem_blocks(s);
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
        // ③ 前缀型 token（GitHub / Slack / Google / AWS）：保留可辨认前缀，其余打码
        if let Some(plen) = match_prefixed_token(&chars, i) {
            for _ in 0..plen {
                out.push(chars[i]);
                i += 1;
            }
            i = value_end(&chars, i);
            out.push_str("***");
            continue;
        }
        // ④ 裸 JWT：保留 `eyJ` 头，三段 base64url 的其余部分打码
        if let Some(hlen) = match_jwt(&chars, i) {
            for _ in 0..hlen {
                out.push(chars[i]);
                i += 1;
            }
            i = value_end(&chars, i);
            out.push_str("***");
            continue;
        }
        // ⑤ 裸连接串 `scheme://user:password@host`：保留 scheme / user / host 形状，只抹口令
        if let Some((at, colon)) = match_conn_string(&chars, i) {
            while i <= colon {
                out.push(chars[i]);
                i += 1;
            }
            out.push_str("***");
            i = at; // 继续从 '@' 起逐个复制（host 部分不动）
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

// ── PEM 私钥块（多行，单独按行处理）────────────────────────────────

/// 抹掉 PEM 私钥多行块：**保留 `BEGIN` / `END` 两行**，把中间每一行（除缩进外）的内容换成
/// `***`，**换行数与缩进形状原样保留**（多行 bullet 的续行缩进不许被破坏 —— 见 `mcp_server.rs`
/// 的 `memory_masks_secrets_before_persisting`）。
///
/// **锚定形态**：只认行首（可带空白 / bullet 缩进）的 `-----BEGIN … PRIVATE KEY-----`
/// （含 `RSA` / `EC` / `OPENSSH` / `ENCRYPTED` 等各种词形）⇒ `-----BEGIN CERTIFICATE-----`
/// 这类证书 / 公钥不动（避免把非机密内容打成码）。**没有匹配的 END 行时原样返回该块**
/// （宁可漏一处也不误伤正文）。
fn mask_pem_blocks(s: &str) -> String {
    let lines: Vec<&str> = s.split_inclusive('\n').collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < lines.len() {
        if pem_marker_line(lines[i], "BEGIN") {
            if let Some(end) = (i + 1..lines.len()).find(|&j| pem_marker_line(lines[j], "END")) {
                out.push_str(lines[i]); // BEGIN 行原样
                for body in &lines[i + 1..end] {
                    out.push_str(&mask_pem_body_line(body));
                }
                out.push_str(lines[end]); // END 行原样
                i = end + 1;
                continue;
            }
        }
        out.push_str(lines[i]);
        i += 1;
    }
    out
}

/// 该行是否是 PEM 的 `-----BEGIN … PRIVATE KEY-----` / `-----END … PRIVATE KEY-----` 行
/// （大小写不敏感、容忍前后空白）。
fn pem_marker_line(line: &str, kind: &str) -> bool {
    let up = line.trim().to_ascii_uppercase();
    up.starts_with(&format!("-----{kind} ")) && up.ends_with("-----") && up.contains("PRIVATE KEY")
}

/// PEM 中段一行：保留行首空白与行尾换行符，把内容换成 `***`；空行 / 纯空白行原样保留。
fn mask_pem_body_line(line: &str) -> String {
    let (content, nl) = if let Some(c) = line.strip_suffix("\r\n") {
        (c, "\r\n")
    } else if let Some(c) = line.strip_suffix('\n') {
        (c, "\n")
    } else {
        (line, "")
    };
    if content.trim().is_empty() {
        return line.to_string();
    }
    let lead = &content[..content.len() - content.trim_start().len()];
    format!("{lead}***{nl}")
}

// ── 前缀型 token / JWT / 连接串（单行结构）──────────────────────────

/// 前缀型 token 的**字面前缀**命中（返回前缀长度）；**大小写敏感** —— 这些前缀本身带大小写
/// 含义（`AIza` / `AKIA` 大写、`ghp_` / `xoxb-` 小写），普通文本里的同名字母组合不会因此被打码。
/// 还要求后面**紧跟一个 token 字符**（字母 / 数字 / `_` / `-`），免得把光秃秃的 `AIza` 一词打码。
fn match_prefixed_token(chars: &[char], i: usize) -> Option<usize> {
    const PREFIXES: [&str; 10] = [
        "ghp_", "gho_", "ghs_", "ghu_", // GitHub PAT / OAuth / server / user
        "xoxb-", "xoxp-", "xoxa-", // Slack bot / user / app
        "AIza",   // Google API key
        "AKIA", "ASIA", // AWS access key id
    ];
    for p in PREFIXES {
        let plen = p.chars().count();
        if i + plen >= chars.len() {
            continue;
        }
        let cand: String = chars[i..i + plen].iter().collect();
        if cand != p {
            continue;
        }
        let next = chars[i + plen];
        if next.is_ascii_alphanumeric() || next == '_' || next == '-' {
            return Some(plen);
        }
    }
    None
}

/// JWT 起点命中（返回 `eyJ` 头长度 3）：要求**恰好三段 base64url、点分**。
/// JWT 头恒以 `eyJ` 开头（base64url 的 `{"`），据此锚定，普通文本不会误伤。
fn match_jwt(chars: &[char], i: usize) -> Option<usize> {
    const HEAD: usize = 3;
    if chars.get(i) != Some(&'e') || chars.get(i + 1) != Some(&'y') || chars.get(i + 2) != Some(&'J')
    {
        return None;
    }
    let mut j = i + HEAD;
    for seg in 0..3 {
        let start = j;
        while j < chars.len() && is_base64url(chars[j]) {
            j += 1;
        }
        if j == start {
            return None; // 该段为空 ⇒ 不是 JWT
        }
        if seg < 2 {
            if chars.get(j) != Some(&'.') {
                return None;
            }
            j += 1;
        }
    }
    Some(HEAD)
}

fn is_base64url(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// 裸连接串 `scheme://user:password@host` 命中：返回 `(at, colon)` 两个**绝对下标**
/// （`at` = `@` 的位置、`colon` = userinfo 里第一个 `:` 的位置），由调用方保留
/// `scheme://user:` 与 `@host` 的形状、只把 `:` 与 `@` 之间的口令换成 `***`。
///
/// **锚定形态**：`<scheme>://` 紧邻 + userinfo（`/`、`?`、`#`、空白之前的）内确有 `@` 且
/// 口令非空 ⇒ 普通 URL（无 `user:pass@`）与普通文本都不会误伤。
fn match_conn_string(chars: &[char], i: usize) -> Option<(usize, usize)> {
    if !chars.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut j = i;
    while j < chars.len()
        && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '+' | '.' | '-'))
    {
        j += 1;
    }
    if !(chars.get(j) == Some(&':') && chars.get(j + 1) == Some(&'/') && chars.get(j + 2) == Some(&'/'))
    {
        return None;
    }
    let userinfo_start = j + 3;
    let mut k = userinfo_start;
    let mut colon = None;
    while k < chars.len() {
        let c = chars[k];
        if c == '@' {
            break;
        }
        if c == ':' && colon.is_none() {
            colon = Some(k);
        }
        if matches!(c, '/' | '?' | '#') || c.is_whitespace() {
            return None; // 撞到路径 / 查询 / 空白还没见 '@' ⇒ 不是 user:pass@host
        }
        k += 1;
    }
    let colon = colon?;
    if k >= chars.len() {
        return None; // 没找到 '@'
    }
    if k == colon + 1 {
        return None; // 口令为空
    }
    Some((k, colon))
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

    /// PEM 私钥多行块（2026-10-13 扩展）：BEGIN / END 两行保留、中段内容打码，
    /// 且**换行数与续行缩进形状不变**（与 `mcp_server.rs` 那条 bullet 形状口径一致）。
    /// 覆盖 RSA / EC / OPENSSH / ENCRYPTED 各种词形，并确认证书（CERTIFICATE）不误伤。
    #[test]
    fn mask_hides_pem_private_key_body_keeps_begin_end() {
        for kind in [
            "RSA PRIVATE KEY",
            "EC PRIVATE KEY",
            "OPENSSH PRIVATE KEY",
            "ENCRYPTED PRIVATE KEY",
            "PRIVATE KEY",
        ] {
            let s = format!("私钥：\n  -----BEGIN {kind}-----\n  SECRETBBBBODY\n  MORE\n  -----END {kind}-----\n完");
            let m = mask_secrets(&s);
            assert!(m.contains(&format!("-----BEGIN {kind}-----")), "{kind} BEGIN 行应保留：{m}");
            assert!(m.contains(&format!("-----END {kind}-----")), "{kind} END 行应保留：{m}");
            assert!(!m.contains("SECRETBBBBODY"), "{kind} 私钥体泄露：{m}");
            assert_eq!(m.matches('\n').count(), s.matches('\n').count(), "{kind} 换行数必须不变：{m}");
            assert!(m.contains("\n  ***\n"), "{kind} 续行缩进被破坏：{m}");
        }
        // 证书 / 公钥（非机密）不动
        let cert = "-----BEGIN CERTIFICATE-----\nAAAB\n-----END CERTIFICATE-----";
        assert_eq!(mask_secrets(cert), cert);
    }

    /// 裸 JWT（不依赖前面有键名）：三段 base64url 点分整体打码；两段的不算 JWT。
    #[test]
    fn mask_hides_bare_jwt() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let m = mask_secrets(&format!("here {jwt} end"));
        assert!(!m.contains("SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"), "JWT 泄露：{m}");
        assert!(m.starts_with("here eyJ"), "锚定头 eyJ 应保留：{m}");
        assert!(m.ends_with(" end"), "尾部普通文本不该动：{m}");
        // 只有两段 ⇒ 不是 JWT，原样保留
        let two = "eyJhbGciOiJIUzI1NiJ9.onlytwo";
        assert_eq!(mask_secrets(two), two);
    }

    /// 前缀型 token：GitHub / Slack / Google / AWS 各一例都要打码，且保留可辨认前缀。
    #[test]
    fn mask_hides_prefixed_tokens() {
        for tok in [
            "ghp_16C7e42F292c6912E7710c838347Ae178B4a",
            "xoxb-123456789012-abcdefABCDEF",
            "AIzaSyD-1234567890abcdefghijklmnopqrstu",
            "AKIAIOSFODNN7EXAMPLE",
        ] {
            let m = mask_secrets(&format!("key={tok} done"));
            assert!(!m.contains(tok), "前缀型 token 泄露：{m}");
            assert!(m.ends_with(" done"), "尾部普通文本不该动：{m}");
        }
        // 前缀保留（可辨认是哪一类），其余打码
        assert_eq!(mask_secrets("ghp_abcdef123456"), "ghp_***");
        // 光秃秃的 AIza（后面不是 token 字符）不该被打码
        assert_eq!(mask_secrets("AIza"), "AIza");
    }

    /// 裸连接串 `scheme://user:password@host`：只抹口令，保留 scheme / user / host 形状；
    /// 普通 URL（无 `user:pass@`）不误伤。
    #[test]
    fn mask_hides_bare_connection_string() {
        let m = mask_secrets("dsn=postgres://alice:s3cretPw@db.internal:5432/app");
        assert!(!m.contains("s3cretPw"), "连接串口令泄露：{m}");
        assert_eq!(m, "dsn=postgres://alice:***@db.internal:5432/app");
        let plain = "see https://example.com/path?q=1";
        assert_eq!(mask_secrets(plain), plain);
    }

    /// 多行 bullet 的形状守卫：`- ` 起头、续行缩进、换行数都不受脱敏影响。
    #[test]
    fn mask_keeps_multiline_bullet_shape() {
        let s = "- 用户提到 key sk-live-99887766\n  第二行：token=abcdef123456\n  普通续行\n- 另一条";
        let m = mask_secrets(s);
        assert!(m.starts_with("- "), "首行仍是 bullet：{m}");
        assert!(!m.contains("99887766"), "裸 key 泄露：{m}");
        assert!(!m.contains("abcdef123456"), "键值对泄露：{m}");
        assert!(m.contains("\n  "), "续行缩进被破坏：{m}");
        assert!(m.contains("普通续行"), "普通文本不该动：{m}");
        assert_eq!(m.matches('\n').count(), s.matches('\n').count(), "换行数必须不变：{m}");
    }

    /// 反向守卫（按实现如实写）：只有「敏感键名 / 锚定前缀」紧跟取值符或形态时才打码，
    /// `token` 只是普通词（后面不是 `:` / `=` / 引号）一律不动。
    #[test]
    fn mask_keeps_plain_words_untouched() {
        for s in [
            "the token count is 5",
            "tokenize the string",
            "refresh tokens are rotated",
            "https://example.com/path?q=token",
        ] {
            assert_eq!(mask_secrets(s), s, "普通文本被误伤：{s}");
        }
    }
}
