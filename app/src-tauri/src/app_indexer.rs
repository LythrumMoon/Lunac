// app_indexer.rs — System application scanner for Lunac
//
// Scans Start Menu (and the custom app registry) for launchable entries.
//
// 应用列表的存储（2026-09 修订）：**落盘文件是唯一真相**
//   <exe 根>\temp\app-index-cache.json
// 搜索路径只读这个文件 —— 不做目录扫描、不写盘、也不持有任何进程内状态，
// 所以任何动作（窗口唤出/隐藏、切进插件、增删自定义启动项）都不会让搜索卡住。
// 全量扫描只发生在「后台刷新」路径上，扫完原子写回文件。

use pinyin::ToPinyin;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 落盘文件版本号；结构不兼容时直接丢弃（等后台重扫重建）。
const SCAN_CACHE_VERSION: u32 = 1;
/// 后台重扫的最小间隔：热键唤出时若文件比这个更旧，就异步重扫一次。
/// 它只决定「何时刷新」，与「搜索读什么」无关 —— 搜索永远只读文件。
const REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(30);
/// 后台重建进行中标记，避免并发重复扫描 / 同时写同一个文件。
static REFRESHING: AtomicBool = AtomicBool::new(false);

/// 列表文件：<exe 根>\temp\app-index-cache.json（纯缓存，可随时重建，随卸载一并清除）。
fn scan_cache_path() -> PathBuf {
    crate::storage::lunac_root_dir()
        .join("temp")
        .join("app-index-cache.json")
}

/// 落盘结构。`saved_ms` 只用来判断「结果有多旧、该不该后台重扫」。
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

/// 读列表文件；缺失 / 损坏 / 版本不符一律 None（等同「还没有列表」）。
fn read_cache() -> Option<PersistedScanCache> {
    let text = fs::read_to_string(scan_cache_path()).ok()?;
    let payload: PersistedScanCache = serde_json::from_str(&text).ok()?;
    if payload.version != SCAN_CACHE_VERSION {
        return None;
    }
    Some(payload)
}

/// 当前应用列表 —— 搜索与列表命令的**唯一入口**。
/// **永不阻塞**：只读文件，不扫描、不写盘、无进程内状态可失效。
pub fn apps() -> Vec<AppEntry> {
    read_cache().map(|p| p.apps).unwrap_or_default()
}

/// 原子写：先写同目录临时文件再 rename 覆盖。
/// 搜索路径随时可能在读这个文件，直接 `fs::write`（截断+写入）会让读方
/// 看到半截 JSON —— 表现为「结果突然空了」。失败一律静默（列表丢了只是下次重扫）。
fn write_cache(apps: &[AppEntry]) {
    let path = scan_cache_path();
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let payload = PersistedScanCache {
        version: SCAN_CACHE_VERSION,
        saved_ms: now_ms(),
        apps: apps.to_vec(),
    };
    let Ok(json) = serde_json::to_string(&payload) else { return };
    let tmp = dir.join("app-index-cache.json.tmp");
    if fs::write(&tmp, json).is_err() {
        return;
    }
    let _ = fs::rename(&tmp, &path);
}

/// 扫描结果比 `REFRESH_MIN_INTERVAL` 更旧（或压根没有文件）才后台重扫。
/// 供启动与 Alt+Space 唤出路径调用，自身不阻塞。
pub fn refresh_if_stale() {
    let fresh = read_cache()
        .map(|p| now_ms().saturating_sub(p.saved_ms) < REFRESH_MIN_INTERVAL.as_millis() as u64)
        .unwrap_or(false);
    if !fresh {
        refresh_in_background();
    }
}

/// 后台全量重扫并写回文件（去重：已有任务在跑则忽略）。非阻塞。
pub fn refresh_in_background() {
    if REFRESHING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    std::thread::spawn(|| {
        write_cache(&scan_all_uncached());
        REFRESHING.store(false, Ordering::SeqCst);
    });
}

/// Check if a string contains any Chinese character
///
/// `pub(crate)`：`file_indexer` 的拼音兜底要复用同一个判据（见 `score_pinyin`）——
/// 「有汉字才转拼音」这一条两边必须一致，各写一份必然漂移。
pub(crate) fn has_chinese(s: &str) -> bool {
    s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

/// Generate pinyin tokens for a Chinese name: full pinyin + first letters.
/// Returns empty vec if the name has no Chinese characters.
///
/// `pub(crate)`：`file_indexer::score_pinyin` 复用（同上，避免两套拼音口径）。
pub(crate) fn generate_pinyin_tokens(name: &str) -> Vec<String> {
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
// 自定义启动项属**业务数据**（不是缓存，丢了就是用户资产丢失），存
// <exe 根>\ModuleData\custom\app_registry.json —— 与 storage.rs 的
// history/memo 同一根，随卸载一并清除；旧版本曾放在 exe 同目录，首读自动迁移。

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
    sync_custom_apps();
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
    sync_custom_apps();
    Ok(())
}

/// 自定义启动项增删后，**就地**把列表文件改成新内容：系统项沿用文件里的现状，
/// custom 项按注册表现算 —— 不重扫 Start Menu、更不删文件。
///
/// 旧实现在这里把整个列表文件删掉，下一次搜索因为无文件可读而退化成全量同步
/// 扫描，正是「加完自定义启动项后第一次搜索卡一下」的来源。改成就地重写后，
/// 新增的启动项**立刻**可被搜到，且搜索路径依然只是读一次文件。
fn sync_custom_apps() {
    let system: Vec<AppEntry> = apps().into_iter().filter(|a| a.source != "custom").collect();
    write_cache(&merge_apps(system, custom_apps()));
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

/// 全量扫描：Start Menu 的 .lnk + 注册表里仍然存在的自定义启动项。
/// **只在后台刷新路径上调用**（`refresh_in_background` / `sync_custom_apps`），
/// 搜索路径走 `apps()` 读文件，永远不会进到这里。
fn scan_all_uncached() -> Vec<AppEntry> {
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    let programdata = std::env::var("ProgramData").unwrap_or_default();

    // Start Menu .lnk（用户级 + 全局级）。锁定系统 Start Menu 路径，
    // 与用户预期一致（不扫桌面/Program Files 这类噪声很大的位置）。
    let mut system: Vec<AppEntry> = Vec::new();
    for p in [
        PathBuf::from(&appdata).join(r"Microsoft\Windows\Start Menu\Programs"),
        PathBuf::from(&programdata).join(r"Microsoft\Windows\Start Menu\Programs"),
    ] {
        walk_lnk(&p, &mut system, "start_menu", 0);
    }

    merge_apps(system, custom_apps())
}

/// 注册表里**路径仍然存在**的自定义启动项（已删除的条目直接忽略）
fn custom_apps() -> Vec<AppEntry> {
    load_registry()
        .apps
        .into_iter()
        .filter(|a| Path::new(&a.path).exists())
        .collect()
}

/// 合并系统项与自定义项：按来源优先级 + 名称排序，同名（忽略大小写）只保留
/// 优先级更高的那条 —— 即系统项优先，用户给同名应用加的自定义项被吞掉。
///
/// 注意**不能用 `dedup_by`**：排序键里含来源优先级，同名条目不一定相邻
/// （中间会插进别的低优先级条目），`dedup_by` 只处理相邻重复，会漏掉跨来源的
/// 重名 → 搜索结果里出现两个同名条目。这里用「全表 seen 集合」去重。
fn merge_apps(mut system: Vec<AppEntry>, custom: Vec<AppEntry>) -> Vec<AppEntry> {
    let mut apps = Vec::with_capacity(system.len() + custom.len());
    apps.append(&mut system);
    apps.extend(custom);
    apps.sort_by(|a, b| {
        source_priority(&a.source)
            .cmp(&source_priority(&b.source))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let mut seen = HashSet::new();
    apps.retain(|a| seen.insert(a.name.to_lowercase()));
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
///
/// 只读列表文件（`apps()`）—— 每次击键调用也不会扫描目录、不会写盘。
pub fn search_apps(query: &str, limit: usize) -> Vec<AppEntry> {
    if query.trim().is_empty() {
        return Vec::new();
    }

    let all = apps();
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

/// **以管理员身份运行**（Windows 专用）：`ShellExecuteW` 的 `runas` verb 会走
/// UAC 提权通道。两点必须清楚：
///
///   ① **这是全项目唯一一处「主动请求提权」的用户界面入口**（另一处是
///      `auto_start.rs` 里创建登录计划任务，只在自启修复时用）。`runas` 的
///      弹框就是用户的同意闸门 —— 用户点「否」时 `ShellExecuteW` 返回
///      `SE_ERR_ACCESSDENIED (5)`，我们把错误原样回给前端提示，**不静默失败**。
///   ② **不接受命令行字符串**：`file` 与 `params` 分开传入，`ShellExecuteW`
///      自己拼参数，全程不经 `cmd.exe`。纪律与 `launch_app` 一致（前端可传任意
///      路径 —— 信任边界是「这是我们自己的 WebView 前端」，不是「任意 URI 都放行」）。
///
/// 注意：本进程自身**不是**提升的（`asInvoker`，见 ai-spec §11 规则 29），
/// 所以 UAC 一定会弹；这也意味着提权后的目标与我们**不在同一个权限上下文**里。
#[cfg(target_os = "windows")]
pub fn launch_elevated(file: &str, params: Option<&str>) -> Result<(), String> {
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
    const SE_ERR_ACCESSDENIED: isize = 5;

    let wide = |s: &str| -> Vec<u16> { OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect() };
    let op: Vec<u16> = OsStr::new("runas").encode_wide().chain(std::iter::once(0)).collect();
    let file_w = wide(file);
    let params_w = params.map(wide);

    let ret = unsafe {
        ShellExecuteW(
            0,
            op.as_ptr(),
            file_w.as_ptr(),
            params_w.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };

    if ret > 32 {
        crate::log::info(format!("launch_elevated: {file} {}", params.unwrap_or("")));
        Ok(())
    } else if ret == SE_ERR_ACCESSDENIED {
        Err(format!("提权被拒绝（UAC 取消或策略限制）: {file}"))
    } else {
        Err(format!("ShellExecuteW(runas) failed with code {}", ret))
    }
}

#[cfg(not(target_os = "windows"))]
pub fn launch_elevated(file: &str, _params: Option<&str>) -> Result<(), String> {
    Err(format!("以管理员身份运行仅支持 Windows: {file}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// 下面几个测试都读写同一份列表 / 注册表文件（`cargo test` 默认并行跑测试），
    /// 用一把锁串起来，免得互相把对方刚写进去的内容覆盖掉。
    static FILE_LOCK: Mutex<()> = Mutex::new(());

    fn lock_files() -> MutexGuard<'static, ()> {
        FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_search_empty() {
        assert!(search_apps("", 10).is_empty());
    }

    #[test]
    fn test_registry_roundtrip() {
        let _g = lock_files();
        let _ = add_custom_app("TestApp", "C:\\Windows\\notepad.exe");
        let apps = list_custom_apps();
        assert!(apps.iter().any(|a| a.name == "TestApp"));
        let _ = remove_custom_app("C:\\Windows\\notepad.exe");
        let apps2 = list_custom_apps();
        assert!(!apps2.iter().any(|a| a.name == "TestApp"));
    }

    fn mk(name: &str, source: &str) -> AppEntry {
        AppEntry {
            name: name.into(),
            path: format!("{name}.lnk"),
            icon: None,
            source: source.into(),
        }
    }

    /// 「文件是唯一真相」：写进去就能读出来，且读的是文件而不是进程内状态。
    #[test]
    fn test_cache_file_is_the_only_source() {
        let _g = lock_files();
        let mut list = apps();
        list.push(mk("LunacSmokeApp", "custom"));
        write_cache(&list);
        assert!(apps().iter().any(|a| a.name == "LunacSmokeApp"));

        // 清掉本次写入的条目，别把测试数据留在列表文件里
        let cleaned: Vec<AppEntry> = apps()
            .into_iter()
            .filter(|a| a.name != "LunacSmokeApp")
            .collect();
        write_cache(&cleaned);
        assert!(!apps().iter().any(|a| a.name == "LunacSmokeApp"));
    }

    /// 合并规则：系统项优先、同名去重时保留系统项、自定义项不被丢掉。
    #[test]
    fn test_merge_apps_priority_and_dedup() {
        let merged = merge_apps(
            vec![mk("WeChat", "start_menu")],
            vec![mk("wechat", "custom"), mk("MyTool", "custom")],
        );
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|a| a.name == "WeChat" && a.source == "start_menu"));
        assert!(merged.iter().any(|a| a.name == "MyTool"));
    }
}
