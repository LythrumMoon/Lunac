//! 白箱边界账本（W0，v2 移植；口径见 `docs/sandbox-research.md` §6）。
//!
//! 目标不是「再加一个开关」，而是让每一次边界判定都留下**一条可核对的记录**：
//!   · 落点 `<exe 根>\ModuleData\security\decisions-YYYYMMDD.ndjson`（按天分片，不进 `chat.db`）；
//!   · 每行固定 12 个字段（新增字段只许追加），`ts` 由宿主生成、前端传入的一律忽略；
//!   · `target` / `evidence` 进盘前过 `mask_secrets()`；
//!   · 单文件 ≤ 32 MiB、目录最多 14 天、总计 ≤ 200 MiB，超限先删最旧的分片；
//!   · 导出一个**自包含** JSON（账本 + policy + 版本 / OS build），不含 Key / Cookie。
//!
//! 边界：本模块只记录 / 读取 / 导出，**不执行任何拦截**。真正的拦截仍在审批卡、
//! agent 静态分析与工作区锁（v1 同口径）。写盘失败由调用方静默忽略 —— 账本是旁路，
//! 不能因为写盘慢或目录只读而打断对话。
//!
//! `mask_secrets` 是 core-agent `log::mask_secrets` 的**逐条同规则**移植：两个 crate
//! 之间没有依赖（cdylib 不能反向引 core-agent 的二进制），这里的测试用同一组样本
//! 钉住行为，避免「两套脱敏规则各说各话」。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 行格式版本（宿主写 1）。
pub const LEDGER_VERSION: u32 = 1;

/// 单文件上限（32 MiB）。
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// 目录总量上限（200 MiB）。
const MAX_TOTAL_BYTES: u64 = 200 * 1024 * 1024;
/// 保留天数（含今天）。
const KEEP_DAYS: i64 = 14;

/// 判定枚举（与前端 `SecurityDecision` 一一对应）。
pub const DECISIONS: [&str; 4] = ["allow", "ask", "deny", "auto"];

/// 原因码唯一枚举（`docs/sandbox-research.md` §6.2 的稳定契约）。
///
/// UI、账本、导出、测试都只认它；**不许在调用点拼中文字符串**。
pub const REASON_CODES: [&str; 25] = [
    "AUTO_READONLY",
    "RUN_MODE_AUTO",
    "WHITELIST_TOOL",
    "WHITELIST_CMD",
    "ASK_DEFAULT",
    "DANGER_RULE",
    "OPAQUE_COMMAND",
    "SECRET_IN_PAYLOAD",
    "PLAN_MODE_BLOCK",
    "PLAN_APPROVAL",
    "ASK_QUESTION",
    "WORKSPACE_LOCK",
    "PROTECTED_DIR",
    "USER_ALLOW",
    "USER_DENY",
    "USER_ALWAYS",
    "ENGINE_TIMEOUT",
    "ENGINE_OOM",
    "MXC_FILE_DENY",
    "MXC_NET_DENY",
    "MXC_CAP_DENY",
    "MXC_UI_DENY",
    "BACKEND_UNAVAILABLE",
    // 技能市场（§待办 3-28.9 ②③）：哈希不符 / 升级扩大工具权限。
    "HASH_MISMATCH",
    "PERMISSION_EXPAND",
];

/// 账本里的一行（字段固定、顺序固定；新增字段只许追加在末尾）。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    /// 行格式版本（宿主写 1）
    #[serde(default)]
    pub v: u32,
    /// 宿主生成的 UTC 时间戳
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub actor: String,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub decision: String,
    #[serde(default)]
    pub reason_code: String,
    #[serde(default)]
    pub reason_text: String,
    #[serde(default)]
    pub policy_source: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub evidence: Value,
    #[serde(default)]
    pub reversible: bool,
}

/// 前端提交的一条判定（**没有** `v` / `ts`：这两个字段由宿主生成）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Incoming {
    #[serde(default)]
    session: String,
    #[serde(default)]
    actor: String,
    #[serde(default)]
    tool: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    decision: String,
    #[serde(default)]
    reason_code: String,
    #[serde(default)]
    reason_text: String,
    #[serde(default)]
    policy_source: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    evidence: Option<Value>,
    #[serde(default)]
    reversible: bool,
}

fn security_dir(root: &Path) -> PathBuf {
    root.join("ModuleData").join("security")
}

fn export_dir(root: &Path) -> PathBuf {
    security_dir(root).join("exports")
}

fn shard_path(root: &Path, date: &str) -> Result<PathBuf, String> {
    validate_iso_date(date)?;
    Ok(security_dir(root).join(format!("decisions-{date}.ndjson")))
}

/// 严格 `YYYY-MM-DD`（日期来自前端，必须挡住路径拼串）。
fn validate_iso_date(date: &str) -> Result<(), String> {
    let bytes = date.as_bytes();
    let ok = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() });
    if !ok || parse_date(date).is_none() {
        return Err(format!("日期不合法：{date}（要 YYYY-MM-DD）"));
    }
    Ok(())
}

fn parse_date(date: &str) -> Option<(i32, u32, u32)> {
    if date.len() != 10 {
        return None;
    }
    let year: i32 = date.get(0..4)?.parse().ok()?;
    let month: u32 = date.get(5..7)?.parse().ok()?;
    let day: u32 = date.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

// ── 日期换算（Howard Hinnant 的 civil 算法，不引时区库）────────────────

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year as i64 - 1 } else { year as i64 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { y + 1 } else { y } as i32;
    (year, month, day)
}

/// 宿主生成的 UTC 时间戳与日期分片键：`(2026-11-01T05:12:33Z, 2026-11-01)`。
///
/// `pub(crate)`：计划留档（`plan_archive`）复用同一份时间格式，不另写一套。
pub(crate) fn utc_now() -> (String, String) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let date = format!("{year:04}-{month:02}-{day:02}");
    let stamp = format!(
        "{date}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    );
    (stamp, date)
}

// ── 脱敏（core-agent `log::mask_secrets` 的同规则移植）──────────────────

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

/// 进盘前抹掉明显的凭据。账本会被用户导出求助，密钥不能跟着出门。
pub fn mask_secrets(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
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
            // `Authorization: Bearer <token>`：键名分支会把 `Bearer` 当成值，真正的 token
            // 就漏在下一段。跳过认证方案词，让 `***` 落在 token 上（与 core-agent 同步）。
            if let Some(scheme) = auth_scheme_len(&chars, i) {
                for _ in 0..scheme {
                    out.push(chars[i]);
                    i += 1;
                }
                while i < chars.len() && chars[i] == ' ' {
                    out.push(chars[i]);
                    i += 1;
                }
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

/// 从 `i` 起是否是一个 HTTP 认证方案词（后接空白）；返回词长。
///
/// 只服务于键名分支：`Authorization: Bearer <token>` 里的 `Bearer` 不是秘密，
/// 真正要抹掉的是它后面的 token。
fn auth_scheme_len(chars: &[char], i: usize) -> Option<usize> {
    const SCHEMES: [&str; 3] = ["bearer", "basic", "digest"];
    for scheme in SCHEMES {
        let len = scheme.chars().count();
        if i + len >= chars.len() {
            continue;
        }
        let candidate: String = chars[i..i + len].iter().collect();
        if !candidate.eq_ignore_ascii_case(scheme) {
            continue;
        }
        if chars[i + len] == ' ' {
            return Some(len);
        }
    }
    None
}

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

/// 递归脱敏一个 JSON 值（只处理字符串；数字 / 布尔 / null 原样）。
fn mask_json(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(mask_secrets(text)),
        Value::Array(items) => Value::Array(items.iter().map(mask_json).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), mask_json(item)))
                .collect(),
        ),
        other => other.clone(),
    }
}

// ── 写入 / 读取 / 导出 ──────────────────────────────────────────────

/// 追加一条判定，返回落点路径。`date` 由前端给（Rust 没有本地时区），
/// `ts` 由宿主生成 —— 前端传的 `ts` 会被忽略。
pub fn append(root: &Path, date: &str, entry: &Value) -> Result<PathBuf, String> {
    let path = shard_path(root, date)?;
    let incoming: Incoming =
        serde_json::from_value(entry.clone()).map_err(|e| format!("判定不合法：{e}"))?;
    if incoming.actor.trim().is_empty() {
        return Err("判定缺少 actor".into());
    }
    if incoming.tool.trim().is_empty() {
        return Err("判定缺少 tool".into());
    }
    if !DECISIONS.contains(&incoming.decision.as_str()) {
        return Err(format!(
            "判定 decision 不合法：{}（要 allow / ask / deny / auto）",
            incoming.decision
        ));
    }
    if !REASON_CODES.contains(&incoming.reason_code.as_str()) {
        return Err(format!(
            "判定 reasonCode 不合法：{}（只认 docs/sandbox-research.md §6.2 的枚举）",
            incoming.reason_code
        ));
    }
    let (ts, _) = utc_now();
    let record = Decision {
        v: LEDGER_VERSION,
        ts,
        session: incoming.session,
        actor: incoming.actor,
        tool: incoming.tool,
        target: mask_secrets(&incoming.target),
        decision: incoming.decision,
        reason_code: incoming.reason_code,
        reason_text: mask_secrets(&incoming.reason_text),
        policy_source: incoming.policy_source,
        scope: mask_secrets(&incoming.scope),
        evidence: incoming.evidence.map(|v| mask_json(&v)).unwrap_or(Value::Null),
        reversible: incoming.reversible,
    };
    std::fs::create_dir_all(security_dir(root)).map_err(|e| e.to_string())?;
    let line = serde_json::to_string(&record).map_err(|e| e.to_string())?;
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        writeln!(file, "{line}").map_err(|e| e.to_string())?;
    }
    // 上限维护：先按天删、再按总量删、最后压当前分片。失败不影响已写入的那一行。
    prune(root, date, KEEP_DAYS, MAX_TOTAL_BYTES);
    trim_to_size(&path, MAX_FILE_BYTES);
    Ok(path)
}

/// 最近的判定，**新 → 旧**。单行损坏只跳过该行（与用量日志同口径）。
pub fn recent(root: &Path, limit: usize) -> Vec<Decision> {
    let mut out = Vec::new();
    if limit == 0 {
        return out;
    }
    for (_, path) in shards(root) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines().rev() {
            if out.len() >= limit {
                return out;
            }
            if let Ok(record) = serde_json::from_str::<Decision>(line) {
                out.push(record);
            }
        }
    }
    out
}

/// 账本目录下的分片，**新 → 旧**（按文件名里的日期排序）。
fn shards(root: &Path) -> Vec<(String, PathBuf)> {
    let dir = security_dir(root);
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found: Vec<(String, PathBuf)> = read
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_string();
            let date = name.strip_prefix("decisions-")?.strip_suffix(".ndjson")?;
            parse_date(date)?;
            Some((date.to_string(), path))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found
}

/// 导出：一个自包含 JSON（账本 + policy + 版本 / OS build），不含 Key / Cookie。
pub fn export(root: &Path, policy: &Value, app_version: &str) -> Result<PathBuf, String> {
    let (stamp, date) = utc_now();
    let dir = export_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file_stamp = stamp
        .trim_end_matches('Z')
        .replace([':', 'T'], "-")
        .replace(' ', "-");
    let path = dir.join(format!("lunac-security-{file_stamp}.json"));
    let entries = recent(root, usize::MAX);
    let version = if app_version.trim().is_empty() {
        env!("CARGO_PKG_VERSION")
    } else {
        app_version
    };
    let bundle = serde_json::json!({
        // 导出格式版本：与账本行版本独立（导出结构以后可以扩字段）。
        "v": LEDGER_VERSION,
        "exportedAt": stamp,
        "appVersion": version,
        "osBuild": os_build(),
        "policy": policy,
        "days": KEEP_DAYS,
        "totalDays": shards(root).len(),
        "date": date,
        "entries": entries,
    });
    let text = serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())?;
    std::fs::write(&path, format!("{text}\n")).map_err(|e| e.to_string())?;
    Ok(path)
}

/// 判定预演：调 `agent.exe --analyze <command>`（core-agent `bash_safety::analyze`）。
///
/// 只分析、不执行；返回与 `can_use_tool` 的 `analysis` 同形的对象
/// （`{dangerous, opaque, readonly}`），预演与真实审批因此共用同一套判定。
pub fn analyze_command(root: &Path, command: &str) -> Result<Value, String> {
    if command.trim().is_empty() {
        return Err("预演需要一条命令".into());
    }
    let exe = crate::agent::find_agent_exe(root)
        .ok_or_else(|| "找不到 agent.exe（先运行 scripts\\build-core.ps1 构建）".to_string())?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--analyze")
        .arg(command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：预演不该闪一个黑框。
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 agent.exe 失败：{e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("判定预演超时（agent.exe 3 秒未返回）".into());
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(15)),
            Err(e) => return Err(format!("等待 agent.exe 失败：{e}")),
        }
    }
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        use std::io::Read;
        let _ = pipe.read_to_string(&mut stdout);
    }
    parse_analysis(&stdout)
}

/// 校验 `--analyze` 的输出：形状不对就报错，**别把坏 JSON 当成「无风险」**。
fn parse_analysis(text: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(text.trim())
        .map_err(|e| format!("agent.exe 的分析输出不是 JSON：{e}"))?;
    let object = value
        .as_object()
        .ok_or("agent.exe 的分析输出必须是对象")?;
    let shape_ok = object.get("dangerous").map(Value::is_array) == Some(true)
        && object.get("opaque").map(Value::is_array) == Some(true)
        && object.get("readonly").map(Value::is_boolean) == Some(true);
    if !shape_ok {
        return Err("agent.exe 的分析输出缺字段（要 {dangerous, opaque, readonly}）".into());
    }
    Ok(value)
}

/// 当前系统的版本描述（导出包里用来复盘「在哪个 Windows build 上判的」）。
#[cfg(windows)]
fn os_build() -> String {
    #[repr(C)]
    struct OsVersionInfoW {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }
    // 手写 extern：给 `windows` crate 加 `Win32_System_SystemInformation` feature 会触发
    // dlltool 报错（本机没有 gcc/dlltool，见 docs/todo.md §3-30 的坑）。
    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
    }
    let mut info = OsVersionInfoW {
        size: std::mem::size_of::<OsVersionInfoW>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform: 0,
        csd: [0; 128],
    };
    // SAFETY: `info` 是本函数栈上的 `OSVERSIONINFOW`，`size` 已按结构体实际大小填好；
    // `RtlGetVersion` 只写这块内存。
    let status = unsafe { RtlGetVersion(&mut info) };
    if status < 0 {
        return format!("Windows (版本未知，NTSTATUS {status})");
    }
    format!("Windows {}.{}.{}", info.major, info.minor, info.build)
}

#[cfg(not(windows))]
fn os_build() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

/// 按天删分片：保留 `keep_days` 天（含 `today`），其余整个文件删掉。
fn prune(root: &Path, today: &str, keep_days: i64, max_total: u64) {
    let Some((year, month, day)) = parse_date(today) else {
        return;
    };
    let cutoff = days_from_civil(year, month, day) - (keep_days - 1).max(0);
    for (date, path) in shards(root) {
        let Some((y, m, d)) = parse_date(&date) else {
            continue;
        };
        if days_from_civil(y, m, d) < cutoff {
            let _ = std::fs::remove_file(path);
        }
    }
    // 总量超限：从最旧的分片开始删（今天的分片最后删）。
    let mut items = shards(root);
    items.reverse();
    let mut total: u64 = items
        .iter()
        .filter_map(|(_, path)| std::fs::metadata(path).ok())
        .map(|meta| meta.len())
        .sum();
    for (date, path) in items {
        if total <= max_total || date == today {
            continue;
        }
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let _ = std::fs::remove_file(&path);
        total = total.saturating_sub(len);
    }
}

/// 单个分片超过上限时，丢掉最旧的行，只保留最近的（不删整个文件 ——
/// 今天的判定比「文件整洁」重要）。
fn trim_to_size(path: &Path, max_bytes: u64) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    if text.len() as u64 <= max_bytes {
        return;
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut size = 0u64;
    for line in text.lines().rev() {
        let line_size = line.len() as u64 + 1;
        if size + line_size > max_bytes && !kept.is_empty() {
            break;
        }
        size += line_size;
        kept.push(line);
    }
    kept.reverse();
    let body = format!("{}\n", kept.join("\n"));
    let _ = std::fs::write(path, body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lunac-ledger-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn entry(decision: &str, code: &str) -> Value {
        json!({
            "session": "sess-1",
            "actor": "agent",
            "tool": "Cmd",
            "target": "powershell -c Get-Date",
            "decision": decision,
            "reasonCode": code,
            "reasonText": "默认需要确认",
            "policySource": "auto_approval.dart",
            "scope": "D:\\proj",
            "evidence": { "detail": "Get-Date" },
            "reversible": false,
        })
    }

    #[test]
    fn append_and_recent_round_trip() {
        let root = temp_root("roundtrip");
        let path = append(&root, "2026-11-15", &entry("ask", "ASK_DEFAULT")).expect("append");
        assert!(path.ends_with("ModuleData\\security\\decisions-2026-11-15.ndjson")
            || path.ends_with("ModuleData/security/decisions-2026-11-15.ndjson"));
        append(&root, "2026-11-15", &entry("allow", "USER_ALLOW")).expect("append");

        let rows = recent(&root, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].reason_code, "USER_ALLOW", "新的在前");
        assert_eq!(rows[1].reason_code, "ASK_DEFAULT");
        assert_eq!(rows[0].v, 1, "版本由宿主写");
        assert_eq!(rows[0].ts.len(), 20, "UTC 时间戳固定 20 字符");
        assert!(rows[0].ts.ends_with('Z'));
        assert_eq!(rows[0].session, "sess-1");
        assert_eq!(rows[0].scope, "D:\\proj");
        assert_eq!(rows[0].evidence["detail"], json!("Get-Date"));
        // 只剩最近 1 条
        assert_eq!(recent(&root, 1).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ts_is_host_generated_and_input_is_ignored() {
        let root = temp_root("ts");
        let mut incoming = entry("auto", "RUN_MODE_AUTO");
        incoming["ts"] = json!("1999-01-01T00:00:00Z");
        incoming["v"] = json!(99);
        append(&root, "2026-11-15", &incoming).expect("append");
        let row = &recent(&root, 1)[0];
        assert_ne!(row.ts, "1999-01-01T00:00:00Z");
        assert_eq!(row.v, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_unknown_decision_and_reason_code() {
        let root = temp_root("reject");
        assert!(append(&root, "2026-11-15", &entry("maybe", "ASK_DEFAULT")).is_err());
        assert!(append(&root, "2026-11-15", &entry("ask", "MADE_UP_CODE")).is_err());
        assert!(append(&root, "../../evil", &entry("ask", "ASK_DEFAULT")).is_err());
        let mut no_actor = entry("ask", "ASK_DEFAULT");
        no_actor["actor"] = json!(" ");
        assert!(append(&root, "2026-11-15", &no_actor).is_err());
        assert!(recent(&root, 10).is_empty(), "非法行一条都不落盘");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn target_and_evidence_are_masked_before_disk() {
        let root = temp_root("mask");
        let mut incoming = entry("ask", "SECRET_IN_PAYLOAD");
        incoming["target"] = json!("curl -H \"Authorization: Bearer abc123456\" https://x");
        incoming["evidence"] = json!({ "detail": "api_key: sk-live-9999", "n": 3 });
        append(&root, "2026-11-15", &incoming).expect("append");
        let row = &recent(&root, 1)[0];
        assert!(!row.target.contains("abc123456"), "{}", row.target);
        assert!(row.target.contains("Bearer"));
        let detail = row.evidence["detail"].as_str().unwrap();
        assert!(!detail.contains("sk-live-9999"), "{detail}");
        assert_eq!(row.evidence["n"], json!(3), "非字符串原样保留");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn mask_secrets_matches_the_agent_rules() {
        // 与 core-agent `log::mask_secrets` 同一组样本（两边各有一份，防规则漂移）。
        assert_eq!(
            mask_secrets("api_key=abc123 tail"),
            "api_key=*** tail".to_string()
        );
        assert_eq!(
            mask_secrets("Authorization: Bearer eyJhbGci"),
            "Authorization: Bearer ***".to_string()
        );
        assert_eq!(mask_secrets("run --key sk-abcdef"), "run --key sk-***");
        // `max_tokens` 里的 token 不误伤（前面不是完整键名边界）
        assert_eq!(mask_secrets("\"max_tokens\":8192"), "\"max_tokens\":8192");
    }

    #[test]
    fn retention_drops_old_shards_and_total_cap_drops_oldest() {
        let root = temp_root("retention");
        let dir = security_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("decisions-2020-01-01.ndjson"), "{}\n").unwrap();
        std::fs::write(dir.join("decisions-2026-11-14.ndjson"), "{}\n").unwrap();
        append(&root, "2026-11-15", &entry("ask", "ASK_DEFAULT")).expect("append");
        assert!(!dir.join("decisions-2020-01-01.ndjson").exists(), "14 天前删掉");
        assert!(dir.join("decisions-2026-11-14.ndjson").exists(), "昨天保留");

        // 总量上限压到 1 字节 ⇒ 只留今天的分片
        prune(&root, "2026-11-15", KEEP_DAYS, 1);
        assert!(dir.join("decisions-2026-11-15.ndjson").exists(), "今天的永不删");
        assert!(!dir.join("decisions-2026-11-14.ndjson").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn oversized_shard_keeps_the_newest_lines() {
        let root = temp_root("trim");
        let path = security_dir(&root).join("decisions-2026-11-15.ndjson");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "old-line\nmid-line\nnew-line\n").unwrap();
        trim_to_size(&path, 20);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("new-line"), "{text}");
        assert!(!text.contains("old-line"), "{text}");
        assert!(text.len() <= 20, "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn export_bundle_is_self_contained_and_has_no_credentials() {
        let root = temp_root("export");
        let mut incoming = entry("deny", "USER_DENY");
        incoming["target"] = json!("echo token=abc123");
        append(&root, "2026-11-15", &incoming).expect("append");
        let path = export(
            &root,
            &json!({ "runMode": "allowlist", "workspace": "D:\\proj" }),
            "0.9.45",
        )
        .expect("export");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("abc123"), "导出包不含凭据：{text}");
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["appVersion"], json!("0.9.45"));
        assert!(value["osBuild"].as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(value["policy"]["runMode"], json!("allowlist"));
        assert_eq!(value["entries"].as_array().unwrap().len(), 1);
        assert!(!path.exists() || path.to_string_lossy().contains("exports"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 实跑：需要 `core-agent/target/{release,debug}/agent.exe`（未构建就跳过）。
    #[test]
    fn analyze_command_runs_the_real_agent_when_available() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        if crate::agent::find_agent_exe(&root).is_none() {
            eprintln!("skip: agent.exe not built");
            return;
        }
        let readonly = analyze_command(&root, "git status").expect("analyze readonly");
        assert_eq!(readonly["readonly"], json!(true));

        let danger = analyze_command(&root, "rm -rf /").expect("analyze danger");
        assert!(
            !danger["dangerous"].as_array().unwrap().is_empty(),
            "危险命令必须被同一套 bash_safety 认出来：{danger}"
        );
        assert!(analyze_command(&root, "   ").is_err(), "空命令直接报错");
    }

    #[test]
    fn analyze_output_must_have_the_expected_shape() {
        let ok = parse_analysis(r#"{"dangerous":[],"opaque":[],"readonly":true}"#).expect("合法");
        assert_eq!(ok["readonly"], json!(true));
        assert!(parse_analysis("not json").is_err());
        assert!(
            parse_analysis(r#"{"dangerous":[]}"#).is_err(),
            "缺字段不许当成「无风险」"
        );
        assert!(parse_analysis("[]").is_err());
    }

    #[test]
    fn date_math_round_trips() {
        for (y, m, d) in [(2024, 2, 29), (2026, 11, 15), (1999, 12, 31), (2000, 1, 1)] {
            let days = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(days), (y, m, d));
        }
        let (_, date) = utc_now();
        assert_eq!(date.len(), 10);
        assert!(validate_iso_date(&date).is_ok());
    }
}
