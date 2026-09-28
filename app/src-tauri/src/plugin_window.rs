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

/// 音乐插件专用尺寸。**两个态各一套，都由前端在态变化时通过 `plugin_window_resize` 下发**，
/// 这里只是**建窗那一刻的初值**（免得开窗先闪一下小尺寸）。
///
/// - **默认态 = `1280×720`**（用户 2026-09-28 定，16:9）：面板按 Spotify 复刻成
///   「左栏 Library + 中间曲目表」，1280 宽才放得下两栏。**这个态不追求内容严丝合缝**
///   —— 两侧各自内部滚动，窗口是多大就是多大。
/// - **播放态 = `550×130`**（`music.ts` 的 `MUSIC_W_BAR`）：定尺长条，
///   外层垫料被 CSS 清零（见 `styles.css` 的 `body:has(.music-root.music-player-on)`），
///   鼠标进窗时关闭操作栏浮出 ⇒ 550×170。
///   ⚠️ 早先那个 `562` 是**默认态**的宽度（内容 550 + 外层 12），那个口径现在只对
///   「播放态也想吃外层垫料」的老布局成立，别再把它当成全局窗口宽。
const MUSIC_W: f64 = 1280.0;
const MUSIC_H: f64 = 720.0;

/// 音乐插件窗的**最小高度**（2026-09-28 加）。
///
/// 通用那档 `MIN_H = 200` 是按「竖向小窗」定的，套到音乐插件上就出事了：
/// 播放态实测只要 **159**（长条 130 + 外层 29；鼠标进窗时关闭操作栏浮出再 +40 = 199），
/// 而 200 这个下限把它顶成 200 ⇒ 用户看到的是「长条下面凭空多出 40px 空白」的
/// **562×200 窗口**（2026-09-28 用户报的「那个 562×200 的渲染窗口」就是它，
/// 不是前端算错 —— 前端下发的确实是 159，是在这里被 clamp 掉的）。
/// 下限只需要挡住「误缩到 0」，不必等于某个具体态的最小值。
const MUSIC_MIN_H: f64 = 120.0;

/// 按插件 id 选初始窗口尺寸。**音乐插件是唯一有定尺要求的**，其余走通用尺寸。
fn default_size(plugin_id: &str) -> (f64, f64) {
    if plugin_id == "music" {
        (MUSIC_W, MUSIC_H)
    } else {
        (WIN_W, WIN_H)
    }
}

/// 按插件 id 选**最小**尺寸。
///
/// ⚠️ **两处必须同时用这一条**：`min_inner_size()` 是系统级硬约束
/// （Windows 走 `WM_GETMINMAXINFO`），只放开 `plugin_window_resize` 里的 clamp
/// 而不改它，`set_size` 照样会被系统夹回去 —— 表现同样是「命令成功、窗口没动」。
fn min_size(plugin_id: &str) -> (f64, f64) {
    if plugin_id == "music" {
        (MIN_W, MUSIC_MIN_H)
    } else {
        (MIN_W, MIN_H)
    }
}

/// 从窗口 label 反推插件 id（`plugin-<id>`）；认不出来就按通用尺寸处理。
fn plugin_id_of(label: &str) -> &str {
    label.strip_prefix(LABEL_PREFIX).unwrap_or("")
}

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

    let (w, h) = default_size(plugin_id);
    let (min_w, min_h) = min_size(plugin_id);
    let built = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("plugin.html".into()))
        .title(plugin_id)
        .inner_size(w, h)
        .min_inner_size(min_w, min_h)
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

/// 切「能不能手动缩放窗口」。
///
/// **播放态要关掉**（用户 2026-09-28 定）：播放态是定尺长条 550×130，手动拉一下
/// 只会被下一轮 tick 贴回去（用户看到的就是「拉了没反应」，像坏了）；
/// 关掉缩放之后交互是一致的 —— 想调尺寸就去默认面板。
/// 默认态保持可缩放（它的内容本来就会随窗口变）。
///
/// **这是宿主侧唯一一处「窗口能力」开关**：前端只在**态变化**时调它一次，
/// 别放进每秒那轮 tick（`set_resizable` 会打一次窗口消息，没必要每秒来一次）。
#[tauri::command]
pub fn plugin_window_set_resizable(window: WebviewWindow, resizable: bool) -> Result<(), String> {
    window
        .set_resizable(resizable)
        .map_err(|e| format!("切换窗口缩放失败：{e}"))
}

/// 前端实测内容尺寸后下发窗口尺寸。
///
/// **为什么由前端决定**：音乐插件的播放态与默认态高度差近一倍，而内容里的是
/// 定尺区块（封面 100、歌词 65、控制条 35）—— 这些在 CSS 里算得准，在宿主里
/// 只能靠猜。前端用 `scrollHeight` 实测后调这里，宿主只负责 `set_size`。
///
/// **必须收窄**：宽度不许小于 `MIN_W`（再小标题栏三个按钮会挤成一团），
/// 也不许被一个坏值撑到几千像素（那会把窗口顶出屏幕且用户抓不回来）。
/// **下限按插件取**（`min_size`）：音乐插件的播放态只有 159 高，套通用的 200 会
/// 把它顶出一截空白（2026-09-28 用户报的 562×200）。
///
/// **「没生效」必须由回读判定，不能只看 `is_minimized()`**（2026-09-28 改）：
/// 窗口最小化时 `set_size` 会返回 `Ok` 但可见几何一点不变 —— 于是
/// 「命令成功、窗口没动」这种最难查的状态就出现了。所以下发之后**回读实际尺寸**，
/// 对不上就如实报错：前端只有真的贴合了才记账（见 music.ts 的 applyResize），
/// 用户把它恢复出来时下一轮自然会重新贴合。
///
/// ⚠️ 早先这里是「`is_minimized()` 为真就直接拒」。那条实测会**误判**：
/// 2026-09-28 的验收里，窗口在系统层面已经不是最小化（`IsIconic` 为假、
/// 肉眼可见、CDP 也能操作），而 tao 那条 `is_minimized()` 仍然回真 ⇒ 播放态
/// 永远被拒、窗口卡在默认态的 1280×720，而前端每轮都在重试（表现为「静默失效」）。
/// 回读判定**同时覆盖**了那个场景：真被最小化时回读必然对不上，谎报则不会拦。
#[tauri::command]
pub fn plugin_window_resize(window: WebviewWindow, width: f64, height: f64) -> Result<(), String> {
    if !width.is_finite() || !height.is_finite() {
        return Err("ERR_BAD_SIZE".into());
    }
    let (min_w, min_h) = min_size(plugin_id_of(window.label()));
    let w = width.clamp(min_w, 2000.0);
    let h = height.clamp(min_h, 2000.0);
    window
        .set_size(tauri::LogicalSize::new(w, h))
        .map_err(|e| format!("调整插件窗口尺寸失败：{e}"))?;
    // 容差 2px：LogicalSize → PhysicalSize 那一趟会按 DPI 比例取整
    // （1.25 缩放下 550 → 688 → 550.4），严格相等会把自己误判成失败。
    let scale = window.scale_factor().unwrap_or(1.0);
    let got = window
        .inner_size()
        .map(|s| (f64::from(s.width) / scale, f64::from(s.height) / scale))
        .map_err(|e| format!("读窗口尺寸失败：{e}"))?;
    if (got.0 - w).abs() > 2.0 || (got.1 - h).abs() > 2.0 {
        return Err(format!("ERR_SIZE_STUCK({:.0}x{:.0}≠{w}x{h})", got.0, got.1));
    }
    Ok(())
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
    fn music_window_min_height_does_not_clamp_the_player_state() {
        // 播放态实测 159（长条 130 + 外层 29）⇒ 音乐窗的下限必须低于它，否则
        // `set_size` 会被夹成 200，长条下面多出一截空白（2026-09-28 的线上现象）。
        let (_, music_min_h) = min_size("music");
        assert!(music_min_h < 159.0, "music 最小高度 {music_min_h} 会把 159 的播放态夹住");
        // 任何插件都不得出现「最小 > 默认」（那样一建窗就被系统夹一次）
        for id in ["music", "memo", "clipboard-history"] {
            let (min_w, min_h) = min_size(id);
            let (def_w, def_h) = default_size(id);
            assert!(min_w <= def_w && min_h <= def_h, "{id} 的最小尺寸大于默认尺寸");
        }
    }

    #[test]
    fn plugin_id_of_reads_back_the_label() {
        assert_eq!(plugin_id_of(&label_for("music")), "music");
        // 认不出来（主窗口的 label 是 "main"）时回空串 ⇒ 落到通用那一档
        assert_eq!(plugin_id_of("main"), "");
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
