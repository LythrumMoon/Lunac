// app_indexer.rs — System application scanner for Lunac
//
// Scans Start Menu, Desktop, Program Files, and PATH for executables
// and shortcuts. Supports custom app registration from JSON registry.

use pinyin::ToPinyin;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Start Menu / 自定义应用的扫描结果缓存。
/// search_apps 每次击键都会调用 scan_all()；目录遍历 + pinyin 建索引很慢，
/// 不加缓存是“快速打字顿卡”的主要来源。TTL 内直接返回缓存，新增/删除自定义
/// 应用时调用 invalidate_scan_cache() 主动失效。
struct ScanCache {
    at: Instant,
    apps: Vec<AppEntry>,
}
static SCAN_CACHE: Mutex<Option<ScanCache>> = Mutex::new(None);
const SCAN_CACHE_TTL: Duration = Duration::from_secs(30);
/// 落盘缓存版本号；结构不兼容时直接丢弃旧文件。
const SCAN_CACHE_VERSION: u32 = 1;
/// 落盘缓存最大可用年龄，超过视为不可信（等重新扫描）。
const DISK_CACHE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// 后台重建进行中标记，避免并发重复扫描。
static REFRESHING: AtomicBool = AtomicBool::new(false);

/// 落盘缓存文件：<exe 根>\temp\app-index-cache.json（缓存类数据，随卸载一并清除）。
fn scan_cache_path() -> PathBuf {
    crate::storage::lunac_root_dir()
        .join("temp")
        .join("app-index-cache.json")
}

/// 落盘结构（内存缓存 → 文件）。
#[derive(Serialize, Deserialize)]
struct PersistedScanCache {
    version: u32,
    saved_ms: u64,
    apps: Vec<AppEntry>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 使扫描缓存失效（自定义应用增删后调用）：同时清内存与磁盘，
/// 防止重建前退出导致下次启动复活旧列表。
pub fn invalidate_scan_cache() {
    if let Ok(mut cache) = SCAN_CACHE.lock() {
        *cache = None;
    }
    let _ = fs::remove_file(scan_cache_path());
}

/// Check if a string contains any Chinese character
fn has_chinese(s: &str) -> bool {
    s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

/// Generate pinyin tokens for a Chinese name: full pinyin + first letters.
/// Returns empty vec if the name has no Chinese characters.
fn generate_pinyin_tokens(name: &str) -> Vec<String> {
    if !has_chinese(name) {
        return vec![];
    }
    let syllables: Vec<&str> = name
        .to_pinyin()
        .flatten()
        .map(|p| p.plain())
        .collect();
    if syllables.is_empty() {
        return vec![];
    }
    let mut tokens = Vec::new();
    // Full pinyin: "微信" → "weixin"
    let full: String = syllables.concat();
    tokens.push(full.clone());
    // First letters: "微信" → "wx"
    let firsts: String = syllables.iter().filter_map(|s| s.chars().next()).collect();
    if firsts != full {
        tokens.push(firsts);
    }
    tokens
}

/// A launchable application entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppEntry {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// "start_menu" | "custom"
    #[serde(default = "default_source")]
    pub source: String,
}

fn default_source() -> String {
    "custom".into()
}

/// Custom app registry saved as JSON.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppRegistry {
    pub apps: Vec<AppEntry>,
}

// ── Registry path ─────────────────────────────────────────────────
//
// 2026-09 存储目录重构：业务数据统一放 %LOCALAPPDATA%\Lunac\ModuleData\
// （与 storage.rs 的 history/memo 同一根）。自定义启动项存
// ModuleData\custom\app_registry.json，旧版本曾放在 exe 同目录，首读自动迁移。

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn registry_path() -> PathBuf {
    crate::storage::module_data_dir().join("custom").join("app_registry.json")
}

/// 旧路径（exe 同目录，release 安装目录内/开发 target 目录内）。
/// 只用于一次性迁移读取，不再写入。
fn legacy_registry_path() -> PathBuf {
    exe_dir().join("app_registry.json")
}

// ── Registry I/O ──────────────────────────────────────────────────

pub fn load_registry() -> AppRegistry {
    let path = registry_path();
    if path.exists() {
        return fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
    }
    // 迁移：旧版 exe 同目录的 app_registry.json → ModuleData\custom\
    let legacy = legacy_registry_path();
    if legacy.exists() {
        if let Ok(json) = fs::read_to_string(&legacy) {
            if let Ok(reg) = serde_json::from_str::<AppRegistry>(&json) {
                let _ = fs::create_dir_all(path.parent().expect("registry dir"));
                let _ = fs::write(&path, json);
                return reg;
            }
        }
    }
    AppRegistry::default()
}

fn save_registry(reg: &AppRegistry) {
    let path = registry_path();
    if let (Some(dir), Ok(json)) = (path.parent(), serde_json::to_string_pretty(reg)) {
        let _ = fs::create_dir_all(dir);
        let _ = fs::write(path, json);
    }
}

pub fn add_custom_app(name: &str, path: &str) -> Result<(), String> {
    let target = Path::new(path);
    if !target.exists() {
        return Err(format!("File not found: {}", path));
    }
    let mut reg = load_registry();
    // Don't duplicate
    if reg.apps.iter().any(|a| a.path.eq_ignore_ascii_case(path)) {
        return Err("App already registered".into());
    }
    let display_name = if name.trim().is_empty() {
        target
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string())
    } else {
        name.to_string()
    };
    reg.apps.push(AppEntry {
        name: display_name,
        path: path.to_string(),
        icon: None,
        source: "custom".into(),
    });
    save_registry(&reg);
    invalidate_scan_cache();
    Ok(())
}

pub fn remove_custom_app(path: &str) -> Result<(), String> {
    let mut reg = load_registry();
    let before = reg.apps.len();
    reg.apps.retain(|a| !a.path.eq_ignore_ascii_case(path));
    if reg.apps.len() == before {
        return Err("App not found in registry".into());
    }
    save_registry(&reg);
    invalidate_scan_cache();
    Ok(())
}

pub fn list_custom_apps() -> Vec<AppEntry> {
    load_registry().apps
}

// ── Scanning ──────────────────────────────────────────────────────

/// Walk a directory tree, collecting .lnk files (depth-limited).
fn walk_lnk(dir: &Path, apps: &mut Vec<AppEntry>, source: &str, depth: u32) {
    if depth > 3 {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            // Skip system/hidden dirs
            if name.starts_with('.') {
                continue;
            }
            walk_lnk(&path, apps, source, depth + 1);
        } else if name.to_lowercase().ends_with(".lnk") {
            let app_name = name[..name.len() - 4].to_string();
            apps.push(AppEntry {
                name: app_name,
                path: path.to_string_lossy().to_string(),
                icon: None,
                source: source.into(),
            });
        }
    }
}

/// Full scan: only Start Menu paths + custom registry.
///
/// 三级策略（2026-09）：
///   ① 内存缓存新鲜（TTL 内）→ 直接返回，零 IO；
///   ② 内存缓存过期 → 立即返回旧数据 + **后台重建**（stale-while-revalidate），
///      搜索路径永不因目录扫描阻塞；
///   ③ 无任何缓存（首启且无落盘）→ 同步扫一次。
/// 每次重建都把结果落盘到 <exe 根>\temp\app-index-cache.json，供下次启动预热。
pub fn scan_all() -> Vec<AppEntry> {
    let stale: Option<Vec<AppEntry>> = match SCAN_CACHE.lock() {
        Ok(cache) => match cache.as_ref() {
            Some(c) if c.at.elapsed() < SCAN_CACHE_TTL => return c.apps.clone(),
            Some(c) => Some(c.apps.clone()),
            None => None,
        },
        Err(_) => None,
    };

    if let Some(apps) = stale {
        // 过期但有旧数据：先把旧结果交给用户，后台换成新的
        refresh_scan_cache_in_background();
        return apps;
    }

    // 无缓存：同步扫描 + 落盘（仅首启且无落盘缓存时发生一次）
    let apps = scan_all_uncached();
    store_and_persist(&apps);
    apps
}

/// 写入内存缓存并落盘。
fn store_and_persist(apps: &[AppEntry]) {
    if let Ok(mut cache) = SCAN_CACHE.lock() {
        *cache = Some(ScanCache {
            at: Instant::now(),
            apps: apps.to_vec(),
        });
    }
    persist_scan_cache(apps);
}

/// 落盘（best-effort，失败静默 —— 缓存丢失只影响下次启动速度）。
fn persist_scan_cache(apps: &[AppEntry]) {
    let path = scan_cache_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let payload = PersistedScanCache {
        version: SCAN_CACHE_VERSION,
        saved_ms: now_ms(),
        apps: apps.to_vec(),
    };
    if let Ok(json) = serde_json::to_string(&payload) {
        let _ = fs::write(&path, json);
    }
}

/// 无条件重扫并落盘（后台线程调用）。
fn rebuild_scan_cache() {
    let apps = scan_all_uncached();
    store_and_persist(&apps);
}

/// 后台重建（去重：已有重建在跑则忽略）。非阻塞，可在热键唤出路径安全调用。
pub fn refresh_scan_cache_in_background() {
    if REFRESHING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    std::thread::spawn(|| {
        rebuild_scan_cache();
        REFRESHING.store(false, Ordering::SeqCst);
    });
}

/// 缓存过期（或缺失）才后台重建 —— 供 Alt+Space 唤出 / 启动时调用。
pub fn refresh_scan_cache_if_stale() {
    let stale = match SCAN_CACHE.lock() {
        Ok(c) => c
            .as_ref()
            .map(|c| c.at.elapsed() >= SCAN_CACHE_TTL)
            .unwrap_or(true),
        Err(_) => true,
    };
    if stale {
        refresh_scan_cache_in_background();
    }
}

/// 启动时从落盘缓存预热内存；返回是否成功载入。
/// 文件缺失 / 版本不符 / 超过 DISK_CACHE_MAX_AGE 一律忽略（改走重扫）。
pub fn warm_cache_from_disk() -> bool {
    let text = match fs::read_to_string(scan_cache_path()) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let payload: PersistedScanCache = match serde_json::from_str(&text) {
        Ok(p) => p,
        Err(_) => return false,
    };
    if payload.version != SCAN_CACHE_VERSION {
        return false;
    }
    let age = Duration::from_millis(now_ms().saturating_sub(payload.saved_ms));
    if age > DISK_CACHE_MAX_AGE {
        return false;
    }
    // at 回推为「文件保存时刻」，让 TTL 延续落盘时间而非启动时刻：
    // 落盘越久 → 载入后越容易被判为过期 → 触发后台重建。
    let at = Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
    if let Ok(mut cache) = SCAN_CACHE.lock() {
        *cache = Some(ScanCache {
            at,
            apps: payload.apps,
        });
    }
    true
}

fn scan_all_uncached() -> Vec<AppEntry> {
    let mut apps: Vec<AppEntry> = Vec::new();
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    let programdata = std::env::var("ProgramData").unwrap_or_default();

    // 1. Start Menu .lnk files (both user and all-users)
    // Locked to system Start Menu paths for consistency and user expectation
    let start_menu_paths = [
        PathBuf::from(&appdata).join(r"Microsoft\Windows\Start Menu\Programs"),
        PathBuf::from(&programdata).join(r"Microsoft\Windows\Start Menu\Programs"),
    ];
    for p in &start_menu_paths {
        walk_lnk(p, &mut apps, "start_menu", 0);
    }

    // 2. Custom apps from registry (user-added paths, any format)
    let reg = load_registry();
    for a in &reg.apps {
        if Path::new(&a.path).exists() {
            apps.push(a.clone());
        }
    }

    // Deduplicate by name (case-insensitive)
    apps.sort_by(|a, b| {
        source_priority(&a.source)
            .cmp(&source_priority(&b.source))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    apps.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));

    apps
}

/// Lower number = higher priority
fn source_priority(source: &str) -> u8 {
    match source {
        "start_menu" => 0,
        "custom" => 2,
        _ => 5,
    }
}

/// Search apps by fuzzy-matching the query against app names.
/// Only returns executables (.exe, .lnk) — no folders.
/// Custom apps are exceptions (user explicitly added them).
pub fn search_apps(query: &str, limit: usize) -> Vec<AppEntry> {
    if query.trim().is_empty() {
        return Vec::new();
    }

    let all = scan_all();
    let q = query.to_lowercase();
    let mut scored: Vec<(AppEntry, i32)> = all
        .into_iter()
        .filter(|app| {
            // All system apps are .lnk from Start Menu — no folder filtering needed
            // Custom apps can be anything (user's choice)
            if app.source == "custom" {
                return true;
            }
            // Start Menu .lnk files must exist and be a file
            let p = Path::new(&app.path);
            !p.is_dir() && app.path.to_lowercase().ends_with(".lnk")
        })
        .map(|app| {
            let name = app.name.to_lowercase();
            let mut score = if name == q {
                1000
            } else if name.starts_with(&q) {
                500 - source_priority(&app.source) as i32
            } else if name.contains(&q) {
                300 - source_priority(&app.source) as i32
            } else {
                // Fuzzy: count matching characters in order
                let mut chars = q.chars().peekable();
                let mut pos = 0i32;
                for c in name.chars() {
                    if chars.peek() == Some(&c) {
                        chars.next();
                        pos += 1;
                    }
                }
                if chars.peek().is_none() && pos > 0 {
                    (pos * 20) - source_priority(&app.source) as i32
                } else {
                    0
                }
            };

            // Pinyin matching — convert Chinese app name to pinyin tokens
            // and score against the query (same priority logic as JS registry.ts)
            if score == 0 || score < 300 {
                let py_tokens = generate_pinyin_tokens(&app.name);
                for py in &py_tokens {
                    let py_score = if py == &q {
                        600 // exact pinyin match
                    } else if py.starts_with(&q) {
                        450
                    } else if py.contains(&q) {
                        280
                    } else {
                        let mut chars = q.chars().peekable();
                        let mut pos = 0i32;
                        for c in py.chars() {
                            if chars.peek() == Some(&c) {
                                chars.next();
                                pos += 1;
                            }
                        }
                        if chars.peek().is_none() && pos > 0 {
                            pos * 15
                        } else {
                            0
                        }
                    };
                    if py_score > score {
                        score = py_score;
                    }
                }
            }

            (app, score)
        })
        .filter(|(_, s)| *s > 0)
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.truncate(limit);
    scored.into_iter().map(|(app, _)| app).collect()
}

/// Launch an application by its path.
/// Uses ShellExecuteW directly to avoid CMD window flash.
#[cfg(target_os = "windows")]
pub fn launch_app(path: &str) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: isize,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show_cmd: i32,
        ) -> isize;
    }

    const SW_SHOWNORMAL: i32 = 1;

    // "open" verb lets Windows pick the default handler for .lnk/.exe/URLs
    let op: Vec<u16> = OsStr::new("open").encode_wide().chain(std::iter::once(0)).collect();
    let file: Vec<u16> = OsStr::new(path).encode_wide().chain(std::iter::once(0)).collect();

    let ret = unsafe {
        ShellExecuteW(0, op.as_ptr(), file.as_ptr(),
            std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
    };

    // ShellExecuteW returns > 32 on success
    if ret > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed with code {}", ret))
    }
}

#[cfg(not(target_os = "windows"))]
pub fn launch_app(path: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map_err(|e| format!("Failed to launch: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_empty() {
        assert!(search_apps("", 10).is_empty());
    }

    #[test]
    fn test_registry_roundtrip() {
        let _ = add_custom_app("TestApp", "C:\\Windows\\notepad.exe");
        let apps = list_custom_apps();
        assert!(apps.iter().any(|a| a.name == "TestApp"));
        let _ = remove_custom_app("C:\\Windows\\notepad.exe");
        let apps2 = list_custom_apps();
        assert!(!apps2.iter().any(|a| a.name == "TestApp"));
    }
}
