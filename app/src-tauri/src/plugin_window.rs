// src-tauri/src/plugin_window.rs
// 插件悬浮窗 —— 2026-09-27 多窗口基础设施
//
// **为什么要有这个模块**：在此之前整个项目**只有 `main` 一个窗口**
// （`tauri.conf.json` 的 `app.windows` 只有一项，全仓没有任何
// `WebviewWindowBuilder`）。而插件面板是**内嵌**在主窗口的 `#results-list` 里的，
// 于是 `runSearchNow()` 开头那句 `if (pluginActive) return;` 会把插件开着时
// 用户敲进搜索栏的每一个字**整段丢弃** —— 想看插件就用不了搜索，想搜索就得关掉插件。
// 用户要求「插件窗口与搜索窗同时存在」，所以这里把插件面板搬进**独立窗口**。
//
// **一条铁纪律：插件窗口必须是「惰性」的。**
// 宿主有一批**全局单值**状态：`hotkey::UI_MODE` / `DETACHED` / `QUERY_EMPTY`、
// `MAIN_HWND`、以及 `main.rs` `on_window_event` 里的 `Destroyed` 全局清理
// （`cli_bridge::kill_and_cleanup()` + `kill_port(5173)` + `agent_server::stop()`）。
// 这些东西都只描述**主窗口**。插件窗口若参与其中任何一条，就会出现
// 「关掉一个音乐小窗把整个应用的后端清掉」这类灾难。因此：
//   ① `on_window_event` 必须按 `window.label()` 分流（见 main.rs）；
//   ② 插件窗口的前端**绝不**调 `set_ui_mode` / `set_detached` / `set_query_state`；
//   ③ 热键与 Esc 逻辑一律只认 `MAIN_HWND`（原本就如此，不要改）。
//
// **失焦守卫要放行**：`hotkey.rs` 的轮询里有「可见但不在前台 ⇒ 自动隐藏」。
// 用户点插件窗口时主窗口就不在前台了 —— 若不放行，两者根本没法同时存在。
// 所以这里维护一个「有没有插件窗口开着」的原子量给那条守卫用（见 `MAIN_KEEP_ALIVE`）。
//
// **参数传递不走 URL query**：`WebviewUrl::App` 的路径会经 url join 处理，
// 把 `?id=…&input=…` 塞进 PathBuf 是在赌它的拼接实现。改成「宿主存一份待初始化
// 载荷、前端启动后自己来取」（`plugin_window_init`），既没有时序问题也不用转义。

use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// 插件窗口的 label 前缀。`capabilities/default.json` 里的 `windows` 按
/// `plugin-*` 通配授权，所以这个前缀与那里的通配必须保持一致。
pub const LABEL_PREFIX: &str = "plugin-";

/// 当前开着的插件窗口数。`hotkey.rs` 的失焦守卫读它。
///
/// 用**计数**而不是 bool：同时开音乐 + 备忘录两个小窗是允许的，
/// 关掉其中一个不该让守卫重新生效（那会把主窗口在用户点另一个小窗时藏掉）。
pub static OPEN_WINDOWS: AtomicUsize = AtomicUsize::new(0);

/// 插件窗口默认尺寸。刻意做成**竖向小窗**（不是主窗口 800×200 的横条）：
/// 插件面板的天然形态就是「一列内容」，撑成 800 宽只会让文字行长失控。
const WIN_W: f64 = 420.0;
const WIN_H: f64 = 560.0;
const MIN_W: f64 = 300.0;
const MIN_H: f64 = 200.0;

/// 待初始化载荷：label → (plugin_id, input)。
///
/// 前端 `plugin.html` 启动后主动来取（`plugin_window_init`），所以这里必须在
/// 窗口创建**之前**就写好 —— 页面加载可能快过宿主命令的返回。
static PENDING: OnceLock<Mutex<HashMap<String, (String, String)>>> = OnceLock::new();

fn pending() -> &'static Mutex<HashMap<String, (String, String)>> {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 插件 id 直接进窗口 label，所以必须**白名单**收窄：只允许
/// `[a-z0-9._-]`、非空、≤ 40 字符。这一条与 `plugin_market::is_safe_id()` 同源
/// （那边是「它将来是目录名」，这边是「它是窗口 label」），但**不复用**那个函数：
/// 两处的用途不同，耦合起来以后改一边会悄悄改动另一边。
fn is_safe_plugin_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-')
        && !id.starts_with('.')
}

pub fn label_for(plugin_id: &str) -> String {
    format!("{LABEL_PREFIX}{plugin_id}")
}

#[derive(Debug, Serialize)]
pub struct PluginWindowInit {
    pub plugin_id: String,
    pub input: String,
}

/// 前端启动后自报家门来取载荷。`window` 由 Tauri 注入 = 调用方那个窗口。
#[tauri::command]
pub fn plugin_window_init(window: WebviewWindow) -> Result<PluginWindowInit, String> {
    let label = window.label().to_string();
    let guard = pending().lock().map_err(|e| format!("lock: {e}"))?;
    match guard.get(&label) {
        Some((plugin_id, input)) => Ok(PluginWindowInit {
            plugin_id: plugin_id.clone(),
            input: input.clone(),
        }),
        None => Err("ERR_NO_INIT".into()),
    }
}

/// 某个插件窗现在是否开着。**自动弹出类功能**（音乐插件，见 `music.rs` 的
/// `spawn_spotify_watcher`）用它判断该不该建窗 —— 已经开着就不要再去 `open()`，
/// 那条复用路径会 `set_focus()` 抢焦点。
pub fn is_open(app: &AppHandle, plugin_id: &str) -> bool {
    app.get_webview_window(&label_for(plugin_id)).is_some()
}

/// 建窗或复用（同一个插件不建第二个）。宿主内部与前端命令**共用这一条实现** ——
/// 「复用要推一条 `plugin-window-input`」「建窗前必须写好 PENDING」这些约束只有一份。
pub fn open(app: &AppHandle, plugin_id: &str, input: &str) -> Result<(), String> {
    if !is_safe_plugin_id(plugin_id) {
        return Err("ERR_BAD_PLUGIN_ID".into());
    }
    let label = label_for(plugin_id);

    // 必须在建窗**之前**写好：页面可能比这条命令返回得更快，前端一启动就会来取
    {
        let mut guard = pending().lock().map_err(|e| format!("lock: {e}"))?;
        guard.insert(label.clone(), (plugin_id.to_string(), input.to_string()));
    }

    // 已经开着 ⇒ 只把它拎到前面，并把新入参推给它（同一个插件不该开出两个窗）
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.unminimize();
        let _ = existing.show();
        let _ = existing.set_focus();
        let _ = existing.emit("plugin-window-input", input);
        return Ok(());
    }

    let built = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("plugin.html".into()))
        .title(plugin_id)
        .inner_size(WIN_W, WIN_H)
        .min_inner_size(MIN_W, MIN_H)
        // 与主窗口同一套观感：无边框 + 透明 + 毛玻璃（圆角与阴影由前端 CSS 画）
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(true)
        .always_on_top(true)
        .skip_taskbar(false)
        .visible(true)
        .build()
        .map_err(|e| format!("建插件窗口失败：{e}"))?;

    let _ = built.set_focus();
    OPEN_WINDOWS.fetch_add(1, Ordering::SeqCst);
    crate::log::info(&format!("plugin_window: opened {label}"));
    Ok(())
}

/// 打开（或复用）插件悬浮窗 —— 前端命令，实现见 `open()`。
///
/// **刻意不用 `run_blocking`**：建窗不是阻塞 IO，而是「交给事件循环去建」——
/// 它必须在**非主线程**上发起（主线程发起会与消息泵互等）。所以这里用
/// `async fn` 薄壳本身，理由与 `music::spotify_connect` 相同。
#[tauri::command]
pub async fn open_plugin_window(
    app: AppHandle,
    plugin_id: String,
    input: Option<String>,
) -> Result<(), String> {
    open(&app, &plugin_id, input.as_deref().unwrap_or_default())
}

/// 关掉自己（标题栏的 ×）。`window` 由 Tauri 注入。
///
/// 这里用 `destroy()` 而不是 `close()`：`main.rs` 的 `on_window_event` 对
/// `CloseRequested` 有 `prevent_close()`（那是主窗口「关掉=收进托盘」的语义）。
/// 虽然那里已按 label 分流，但用 `destroy()` 可以**不依赖**那条分流也仍然正确 ——
/// 少一处隐式耦合。计数在 `on_window_event` 的 `Destroyed` 分支里减。
#[tauri::command]
pub fn plugin_window_close(window: WebviewWindow) -> Result<(), String> {
    window.destroy().map_err(|e| format!("关闭插件窗口失败：{e}"))
}

#[tauri::command]
pub fn plugin_window_minimize(window: WebviewWindow) -> Result<(), String> {
    window.minimize().map_err(|e| format!("最小化插件窗口失败：{e}"))
}

/// 当前置顶状态。前端拿它初始化按钮的按下态 —— 建窗时就是 `always_on_top(true)`，
/// 所以按钮初始必须是「已置顶」，不能靠前端猜。
#[tauri::command]
pub fn plugin_window_pin_state(window: WebviewWindow) -> Result<bool, String> {
    window
        .is_always_on_top()
        .map_err(|e| format!("读置顶状态失败：{e}"))
}

#[tauri::command]
pub fn plugin_window_set_pin(window: WebviewWindow, pinned: bool) -> Result<(), String> {
    window
        .set_always_on_top(pinned)
        .map_err(|e| format!("设置置顶失败：{e}"))
}

/// 供 `main.rs` 的 `Destroyed` 分支调用（只在 label 带插件前缀时）。
/// **不能**在别处随手调 —— 计数错一次，失焦守卫就会永久失效或永久误隐藏。
pub fn note_destroyed(label: &str) {
    if !label.starts_with(LABEL_PREFIX) {
        return;
    }
    pending().lock().map(|mut g| {
        g.remove(label);
    }).ok();
    // saturating：宁可停在 0，也不要回绕成 usize::MAX 把守卫永久关掉
    OPEN_WINDOWS
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n.saturating_sub(1)))
        .ok();
    crate::log::info(&format!("plugin_window: closed {label}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_prefix_matches_capability_glob() {
        // capabilities/default.json 里按 "plugin-*" 授权，改前缀必须同时改那里
        assert_eq!(label_for("music"), "plugin-music");
        assert!(label_for("music").starts_with(LABEL_PREFIX));
    }

    #[test]
    fn plugin_id_whitelist_is_narrow() {
        assert!(is_safe_plugin_id("music"));
        assert!(is_safe_plugin_id("clipboard-history"));
        assert!(is_safe_plugin_id("my.plugin_2"));
        // 路径与 label 的注入面
        assert!(!is_safe_plugin_id(""));
        assert!(!is_safe_plugin_id("../evil"));
        assert!(!is_safe_plugin_id("a/b"));
        assert!(!is_safe_plugin_id("a\\b"));
        assert!(!is_safe_plugin_id("A-B")); // 大写不收（label 一律小写，避免大小写不匹配）
        assert!(!is_safe_plugin_id(".hidden"));
        assert!(!is_safe_plugin_id(&"x".repeat(41)));
    }
}
