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

use crate::plugin_market::PluginWindowShape;

/// 插件窗口的 label 前缀。`capabilities/default.json` 里的 `windows` 按
/// `plugin-*` 通配授权，所以这个前缀与那里的通配必须保持一致。
pub const LABEL_PREFIX: &str = "plugin-";

/// 当前开着的插件窗口数。`hotkey.rs` 的失焦守卫读它。
///
/// 用**计数**而不是 bool：同时开音乐 + 备忘录两个小窗是允许的，
/// 关掉其中一个不该让守卫重新生效（那会把主窗口在用户点另一个小窗时藏掉）。
pub static OPEN_WINDOWS: AtomicUsize = AtomicUsize::new(0);

/// 「这个插件窗现在可见吗」这条下发事件的名字（2026-09-29 加）。
///
/// **为什么必须有它**：WebView2 自己**报不出**这件事。Win32 的 `IsWindowVisible()`
/// 对**最小化**窗口返回真，WebView2 据此把 `document.visibilityState` 一直留在
/// `visible` —— 实测：插件窗最小化后 rAF 仍按刷新率（~170fps）满速跑、renderer
/// 稳定吃 **~5% 单核**，而且**连 `visibilitychange` 都不会触发**（数据与取证方式
/// 见 [architecture-rendering.md](../../../docs/architecture-rendering.md) §6.2）。
/// 所以「该不该停动画 / 物理」只能由宿主用一条**显式事件**告诉前端 —— 桌宠这类
/// 常驻动画插件没有第二条路。
pub const VISIBLE_EVENT: &str = "plugin-window-visibility";

/// `VISIBLE_EVENT` 的载荷。做成结构体而不是裸 bool：事件载荷将来要加字段时，
/// 裸 bool 的接收方会静默拿到 `undefined`。
#[derive(Clone, Debug, Serialize)]
pub struct PluginWindowVisibility {
    /// **哪个窗口**的可见态变了（2026-09-29 补）。
    ///
    /// 必须带上：tauri 的 `Emitter::emit` 是**广播**（发给所有窗口），而同时开两个
    /// 插件窗是常态（音乐 + 桌宠）。没有这个字段，另一个窗口会把「别人被最小化了」
    /// 当成自己的事 —— 桌宠会因此停掉动画，而它明明还在屏幕上。
    pub label: String,
    pub visible: bool,
}

/// label → **上一次已经下发过**的那个 `visible` 值。
///
/// 只在**翻转**时发：`WindowEvent::Resized` 在拖拽缩放时每移动一像素就来一条
/// （发的是 `WM_SIZE`），原样转发会把前端事件队列刷爆 —— 而接收方只关心这一位。
static LAST_VISIBLE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();

fn last_visible() -> &'static Mutex<HashMap<String, bool>> {
    LAST_VISIBLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 记账：这个 label 的可见态**翻转了吗**（顺手写入新值）。
///
/// 抽成一条纯记账函数是为了能单测 —— 事件本身要真窗口才发得出来，
/// 但「什么时候该发」这件事与窗口无关。
fn take_visibility_change(label: &str, now: bool) -> bool {
    let mut guard = match last_visible().lock() {
        Ok(g) => g,
        Err(_) => return false, // 锁中毒：宁可这一轮不下发，也不要 panic 掉事件循环
    };
    if guard.get(label) == Some(&now) {
        return false;
    }
    guard.insert(label.to_string(), now);
    true
}

/// 下发「这个插件窗现在可见吗」——**只在翻转时**。`Resized` 那条路走它。
///
/// 参数收成 `app + label` 而不是 `&WebviewWindow`：两个调用点手里一个是
/// `&AppHandle`（`open()` 的复用路径）、一个是 `&Window`（`main.rs` 的事件回调），
/// 而这两种类型没有共同 trait 可写 —— 只有 label 是它们共有的。
pub fn announce_visibility(app: &AppHandle, label: &str) {
    announce(app, label, false);
}

/// 同上，但**无条件发**。宿主**主动**改变窗口可见性时走它（`open()` 的复用路径）。
///
/// 为什么不能复用上面那条：`show()` 不产生 `WM_SIZE`，所以
/// 「插件自己 `hide()` 了 → 用户又从主窗口点了一次「打开」把它 `show` 回来」
/// 在 `Resized` 那条路上**根本不会出现**；而这一次正是插件恢复动画的唯一信号，
/// 不能因为「记账里还是 true」被吞掉。
pub fn announce_visibility_now(app: &AppHandle, label: &str) {
    announce(app, label, true);
}

fn announce(app: &AppHandle, label: &str, force: bool) {
    let window = match app.get_webview_window(label) {
        Some(w) => w,
        None => return,
    };
    let now = !window.is_minimized().unwrap_or(false) && window.is_visible().unwrap_or(true);
    // 无论发不发都要记账，否则下一次比较会拿旧值、多发一条
    let changed = take_visibility_change(label, now);
    if !changed && !force {
        return;
    }
    let _ = window.emit(
        VISIBLE_EVENT,
        PluginWindowVisibility {
            label: label.to_string(),
            visible: now,
        },
    );
    crate::log::info(&format!("plugin_window: {label} visible={now}"));
}

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

/// **AI 聊天的独立窗口**（2026-09-29 用户定：「ai 插件也需要独立界面状态……并去除小窗口
/// 和大窗口，独立界面尺寸设置成与音乐插件相同的」）。
///
/// 它走的仍是**插件窗那一整套基础设施**，只是换了 id 与页面：
///   · label = `plugin-chat` ⇒ 落进 `plugin-*` 通配，`capabilities` 授权、
///     `on_window_event` 按 label 分流、`OPEN_WINDOWS` 失焦放行**全部自动复用**；
///   · 页面 = **`index.html`**（不是 `plugin.html`）⇒ 聊天的界面与逻辑就是主界面那一份，
///     不必把这套界面搬进 `plugin-window.ts`（见 ai-spec §4.8「聊天独立窗」）。
///
/// 尺寸与音乐默认态同档（用户原话「设置成与音乐插件相同的」）；聊天是「一列对话 + 输入栏」，
/// 1280 宽放得下代码块与表格，720 高够看几轮。
const CHAT_WINDOW_ID: &str = "chat";
const CHAT_W: f64 = 1280.0;
const CHAT_H: f64 = 720.0;
/// 聊天下限。比通用那档（300×200）高得多：这条界面有输入栏 + 历史抽屉 + 工具卡，
/// 缩到 300 宽会直接散架（同 settings 不进可悬浮清单的理由）。
const CHAT_MIN_W: f64 = 720.0;
const CHAT_MIN_H: f64 = 420.0;

/// 音乐插件窗的**最小高度**（2026-09-28 加）。
///
/// 通用那档 `MIN_H = 200` 是按「竖向小窗」定的，套到音乐插件上就出事了：
/// 播放态实测只要 **159**（长条 130 + 外层 29；鼠标进窗时关闭操作栏浮出再 +40 = 199），
/// 而 200 这个下限把它顶成 200 ⇒ 用户看到的是「长条下面凭空多出 40px 空白」的
/// **562×200 窗口**（2026-09-28 用户报的「那个 562×200 的渲染窗口」就是它，
/// 不是前端算错 —— 前端下发的确实是 159，是在这里被 clamp 掉的）。
/// 下限只需要挡住「误缩到 0」，不必等于某个具体态的最小值。
const MUSIC_MIN_H: f64 = 120.0;

/// 按插件 id 选初始窗口尺寸。**音乐插件是唯一有定尺要求的**，其余走通用尺寸；
/// 插件清单里的 `window.width/height` 优先于这两档（`0` = 没写 ⇒ 用这里的缺省）。
fn default_size(plugin_id: &str, shape: Option<&PluginWindowShape>) -> (f64, f64) {
    let fallback = if plugin_id == CHAT_WINDOW_ID {
        (CHAT_W, CHAT_H)
    } else if plugin_id == "music" {
        (MUSIC_W, MUSIC_H)
    } else {
        (WIN_W, WIN_H)
    };
    let Some(s) = shape else { return fallback };
    // 只写了一半时另一半仍走缺省 —— 少写一个数不该让另一个跟着回缺省（那会让
    // 「只想改高度」的插件突然吃到通用宽度）。
    (
        if s.width > 0.0 { s.width } else { fallback.0 },
        if s.height > 0.0 { s.height } else { fallback.1 },
    )
}

/// 按插件 id 选**最小**尺寸。
///
/// ⚠️ **两处必须同时用这一条**：`min_inner_size()` 是系统级硬约束
/// （Windows 走 `WM_GETMINMAXINFO`），只放开 `plugin_window_resize` 里的 clamp
/// 而不改它，`set_size` 照样会被系统夹回去 —— 表现同样是「命令成功、窗口没动」。
fn min_size(plugin_id: &str, shape: Option<&PluginWindowShape>) -> (f64, f64) {
    let (mut w, mut h) = if plugin_id == CHAT_WINDOW_ID {
        (CHAT_MIN_W, CHAT_MIN_H)
    } else if plugin_id == "music" {
        (MIN_W, MUSIC_MIN_H)
    } else {
        (MIN_W, MIN_H)
    };
    if let Some(s) = shape {
        if s.min_width > 0.0 {
            w = s.min_width;
        }
        if s.min_height > 0.0 {
            h = s.min_height;
        }
    }
    // 「最小 > 初始」在建窗那一刻会被系统夹一次。两条都在清单里给过时
    // `validate_window_shape` 已经拒了整包，但**只给最小、不给初始**的写法仍能凑出
    // 这个矛盾（初始这时走的是宿主缺省那一档，可能比它小）—— 所以这里再兜一次。
    let (def_w, def_h) = default_size(plugin_id, shape);
    (w.min(def_w), h.min(def_h))
}

/// 这个插件窗加载哪个页面。**聊天窗是唯一的例外**：它加载 `index.html`
/// （主界面那份入口），因为聊天的界面与逻辑就是它 —— 换成 `plugin.html` 就得把
/// 整套聊天界面搬进 `plugin-window.ts`（见 `CHAT_WINDOW_ID`）。
fn page_for(plugin_id: &str) -> &'static str {
    if plugin_id == CHAT_WINDOW_ID {
        "index.html"
    } else {
        "plugin.html"
    }
}

/// 读这个插件清单里声明的窗口形态。
///
/// **读不到就算没声明**（走缺省），不报错：编译进主程序的插件盘上本来就没有目录，
/// 而「清单坏了」这件事 `list_installed` 那边已经如实列给用户看了，不该在这里
/// 变成「窗开不出来」。`parse_manifest` 里的 warn 会照常留痕。
fn declared_shape(plugin_id: &str) -> Option<PluginWindowShape> {
    let path = crate::plugin_market::plugins_dir()
        .join(plugin_id)
        .join("lunac-plugin.json");
    let text = std::fs::read_to_string(path).ok()?;
    crate::plugin_market::parse_manifest(&text).ok()?.window
}

/// 从窗口 label 反推插件 id（`plugin-<id>`）；认不出来就按通用尺寸处理。
fn plugin_id_of(label: &str) -> &str {
    label.strip_prefix(LABEL_PREFIX).unwrap_or("")
}

/// 待初始化载荷：label → 建窗那一刻的启动参数。
///
/// 前端 `plugin.html` 启动后主动来取（`plugin_window_init`），所以这里必须在
/// 窗口创建**之前**就写好 —— 页面加载可能快过宿主命令的返回。
struct PendingInit {
    plugin_id: String,
    input: String,
    /// 清单 `window.chrome`：要不要宿主那根标题栏（缺省 `true`）。
    /// 桌宠那种「窗口就是那片画面」的插件写 `false`，前端据此收起标题栏。
    chrome: bool,
}

static PENDING: OnceLock<Mutex<HashMap<String, PendingInit>>> = OnceLock::new();

fn pending() -> &'static Mutex<HashMap<String, PendingInit>> {
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
    /// 要不要顶部那根标题栏（清单 `window.chrome`，缺省 `true`）。
    /// **必须由宿主告知**：前端看不出自己的窗是哪种形态，而 `plugin.html` 是同一份。
    pub chrome: bool,
}

/// 前端启动后自报家门来取载荷。`window` 由 Tauri 注入 = 调用方那个窗口。
#[tauri::command]
pub fn plugin_window_init(window: WebviewWindow) -> Result<PluginWindowInit, String> {
    let label = window.label().to_string();
    let guard = pending().lock().map_err(|e| format!("lock: {e}"))?;
    match guard.get(&label) {
        Some(p) => Ok(PluginWindowInit {
            plugin_id: p.plugin_id.clone(),
            input: p.input.clone(),
            chrome: p.chrome,
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

    // 清单里声明的窗口形态（桌宠那种「无标题栏 + 不进任务栏 + 定尺」）。读不到 = 用缺省。
    // **必须在写 PENDING 之前取**：`chrome` 要跟着载荷一起交给前端。
    let shape = declared_shape(plugin_id);

    // 必须在建窗**之前**写好：页面可能比这条命令返回得更快，前端一启动就会来取。
    //
    // **聊天窗也写**（2026-09-29）：它加载的是 `index.html`，但同样靠这条载荷取
    // 「带过来的那句话」（用户在搜索栏点了 AI 条目 / 右键「问 AI」时那句）——
    // 聊天窗启动时调 `plugin_window_init` 取走（take 语义，取完即删）。
    {
        let mut guard = pending().lock().map_err(|e| format!("lock: {e}"))?;
        guard.insert(
            label.clone(),
            PendingInit {
                plugin_id: plugin_id.to_string(),
                input: input.to_string(),
                chrome: shape.as_ref().map_or(true, |s| s.chrome),
            },
        );
    }

    // 已经开着 ⇒ 只把它拎到前面，并把新入参推给它（同一个插件不该开出两个窗）
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.unminimize();
        let _ = existing.show();
        let _ = existing.set_focus();
        let _ = existing.emit("plugin-window-input", input);
        // 「我又可见了」这一声必须由**主动方**喊：`show()` 不产生 `WM_SIZE`，
        // 所以插件自己 `hide()` 之后，`Resized` 那条路永远不会替我们补上这一次
        // （没有它，桌宠被重新打开后动画不会恢复 —— 见 announce_visibility_now）。
        announce_visibility_now(app, &label);
        return Ok(());
    }

    let (w, h) = default_size(plugin_id, shape.as_ref());
    let (min_w, min_h) = min_size(plugin_id, shape.as_ref());
    let built = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(page_for(plugin_id).into()))
        .title(plugin_id)
        .inner_size(w, h)
        .min_inner_size(min_w, min_h)
        // 与主窗口同一套观感：无边框 + 透明 + 毛玻璃（圆角与阴影由前端 CSS 画）
        .decorations(false)
        .transparent(true)
        .shadow(false)
        // 下面三项**可被清单的 `window` 段覆盖**（桌宠要：禁缩放 + 不进任务栏）。
        // 括号里的值是「没声明时」的历史行为 —— 别随手改。
        .resizable(shape.as_ref().map_or(true, |s| s.resizable))
        .always_on_top(shape.as_ref().map_or(true, |s| s.always_on_top))
        .skip_taskbar(shape.as_ref().is_some_and(|s| s.skip_taskbar))
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

/// 按插件 id **从外面**关掉它的悬浮窗（2026-09-29 加，给「卸载插件」用）。
///
/// 为什么需要它：卸载发生在**主窗口**的设置面板里，而 `plugin_window_close` 只拿得到
/// 「调用方自己那个窗」—— 主窗口调它只会关掉主窗口。卸载若不顺手把插件的悬浮窗关掉，
/// 窗口还留在屏幕上、还接着跑插件代码，用户看到的就是「卸了等于没卸」。
///
/// **没开着不是错**（照旧回 `Ok`）：卸载一个当前没开窗的插件是最常见的情况，
/// 让调用方为此判错只会逼它写一堆无意义的 try/catch。
#[tauri::command]
pub fn close_plugin_window(app: AppHandle, plugin_id: String) -> Result<(), String> {
    if !is_safe_plugin_id(&plugin_id) {
        return Err("ERR_BAD_PLUGIN_ID".into());
    }
    match app.get_webview_window(&label_for(&plugin_id)) {
        Some(w) => {
            w.destroy().map_err(|e| format!("关闭插件窗口失败：{e}"))?;
            crate::log::info(&format!("plugin_window: closed by uninstall {plugin_id}"));
            Ok(())
        }
        None => Ok(()),
    }
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

/// 切「鼠标穿透」：开着时点击 / 滚轮**直接穿到桌面**，窗口收不到任何鼠标事件。
/// **桌宠的必需能力** —— 不透明的像素吃掉鼠标，透明的那一片必须把鼠标还给桌面，
/// 否则用户的桌面就被一块看不见的玻璃盖住了（底下的图标、别的窗口全点不动）。
///
/// **为什么做成宿主命令、而不是直接放开 `core:window:allow-set-ignore-cursor-events`**：
/// 那条权限是**按窗口**授的，而 `capabilities/default.json` 的 `windows` 里含 `main`
/// —— 一旦放开，任何一处前端 bug 都能把**主窗口**也变成穿透的，而穿透后的窗口**收不到
/// 鼠标**，用户没有任何办法点回来（只能去杀进程）。这里收成一条**只认 `plugin-` 前缀**
/// 的命令：桌宠要，主窗口不许。前缀常量与 `capabilities` 的通配是同一个（有单测钉住）。
///
/// **回值 = 本次下发的值，不是「窗口现在的真实状态」**：tauri / tao 没有
/// `is_ignore_cursor_events()` 这种读法（`tao` 只把 `WS_EX_TRANSPARENT | WS_EX_LAYERED`
/// 按标记位算出来，不给回读）。调用方按回值记账即可，**别把它当成回读**。
#[tauri::command]
pub fn plugin_window_set_click_through(window: WebviewWindow, ignore: bool) -> Result<bool, String> {
    if !window.label().starts_with(LABEL_PREFIX) {
        return Err("ERR_NOT_PLUGIN_WINDOW".into());
    }
    window
        .set_ignore_cursor_events(ignore)
        .map_err(|e| format!("设置鼠标穿透失败：{e}"))?;
    Ok(ignore)
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
    let id = plugin_id_of(window.label());
    // 下限也要按**清单声明的**那一份取：桌宠禁缩放、且定尺比通用下限小的话，
    // 用通用下限会让它每轮 resize 都报 `ERR_SIZE_STUCK`（同一个坑，见上面注释）。
    let (min_w, min_h) = min_size(id, declared_shape(id).as_ref());
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
    // 可见性记账同样要收 —— 窗口标签可以被复用（关掉再开是常态），
    // 留着旧值会让下一次建窗的**第一条** `Resized` 被误判成「没翻转」而不下发。
    last_visible().lock().map(|mut g| {
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
        let (_, music_min_h) = min_size("music", None);
        assert!(music_min_h < 159.0, "music 最小高度 {music_min_h} 会把 159 的播放态夹住");
        // 任何插件都不得出现「最小 > 默认」（那样一建窗就被系统夹一次）
        for id in ["music", "memo", "clipboard-history"] {
            let (min_w, min_h) = min_size(id, None);
            let (def_w, def_h) = default_size(id, None);
            assert!(min_w <= def_w && min_h <= def_h, "{id} 的最小尺寸大于默认尺寸");
        }
    }

    /// 一个「桌宠那样」的形态：定尺、禁缩放、不进任务栏、不要标题栏。
    fn pet_shape() -> PluginWindowShape {
        PluginWindowShape {
            width: 260.0,
            height: 380.0,
            min_width: 120.0,
            min_height: 160.0,
            resizable: false,
            skip_taskbar: true,
            always_on_top: true,
            chrome: false,
        }
    }

    #[test]
    fn declared_window_shape_overrides_defaults() {
        // 没声明 ⇒ 与加这个字段之前逐项一致（420×560 / 最小 300×200 / 可缩放 / 进任务栏）
        assert_eq!(default_size("pet", None), (WIN_W, WIN_H));
        assert_eq!(min_size("pet", None), (MIN_W, MIN_H));

        let shape = pet_shape();
        assert_eq!(default_size("pet", Some(&shape)), (260.0, 380.0));
        assert_eq!(min_size("pet", Some(&shape)), (120.0, 160.0));
    }

    #[test]
    fn chat_window_is_its_own_size_class() {
        // 聊天独立窗（2026-09-29）：尺寸与音乐**默认态**同档（用户原话「独立界面尺寸
        // 设置成与音乐插件相同的」），但**不是**走 420×560 的通用缺省。
        assert_eq!(default_size(CHAT_WINDOW_ID, None), (CHAT_W, CHAT_H));
        assert_eq!(min_size(CHAT_WINDOW_ID, None), (CHAT_MIN_W, CHAT_MIN_H));
        assert_eq!((CHAT_W, CHAT_H), (MUSIC_W, MUSIC_H), "必须与音乐默认态同档");
        // 下限要比通用那档高：这条界面有输入栏 + 历史抽屉 + 工具卡，300×200 会散架
        assert!(CHAT_MIN_W > MIN_W && CHAT_MIN_H > MIN_H);
        // 最小 ≤ 默认（否则建窗那一刻被系统夹一次，同 music 那条判据）
        let (min_w, min_h) = min_size(CHAT_WINDOW_ID, None);
        let (def_w, def_h) = default_size(CHAT_WINDOW_ID, None);
        assert!(min_w <= def_w && min_h <= def_h);
    }

    #[test]
    fn chat_window_is_the_only_one_loading_the_main_page() {
        // 聊天窗加载 index.html（复用它 = 聊天界面一行都不用搬）；**别的插件一律
        // plugin.html** —— 这条一旦被改错，插件窗会去跑整个主界面（抢全局状态）。
        assert_eq!(page_for(CHAT_WINDOW_ID), "index.html");
        for id in ["music", "pet", "memo", "ocr", "settings"] {
            assert_eq!(page_for(id), "plugin.html", "{id} 不该加载主界面");
        }
    }

    #[test]
    fn partial_window_shape_keeps_the_other_side_at_default() {
        // 「只想改高度」不该让宽度跟着变成 0（那会建出一个 0 宽的窗）
        let only_height = PluginWindowShape {
            width: 0.0,
            height: 300.0,
            ..pet_shape()
        };
        assert_eq!(default_size("pet", Some(&only_height)), (WIN_W, 300.0));
    }

    #[test]
    fn min_size_never_exceeds_default_size() {
        // 只给 `minHeight`、不给 `height` 时，初始高度走的是宿主缺省那一档，
        // 完全可能比声明的最小高度还小 —— 那会在建窗那一刻被系统夹一次。
        let only_min_h = PluginWindowShape {
            min_height: 900.0,
            ..pet_shape()
        };
        let (def_w, def_h) = default_size("pet", Some(&only_min_h));
        let (min_w, min_h) = min_size("pet", Some(&only_min_h));
        assert!(min_h <= def_h, "最小高度 {min_h} 大于初始高度 {def_h}");
        assert!(min_w <= def_w);
    }

    #[test]
    fn plugin_id_of_reads_back_the_label() {
        assert_eq!(plugin_id_of(&label_for("music")), "music");
        // 认不出来（主窗口的 label 是 "main"）时回空串 ⇒ 落到通用那一档
        assert_eq!(plugin_id_of("main"), "");
    }

    #[test]
    fn visibility_event_fires_only_on_flip() {
        // 拖拽缩放期间 `Resized` 会连着来一串；只有「翻转」那一次该下发，
        // 否则前端事件队列会被一串同值刷爆。
        // 用独有 label：这组用例共享同一个全局记账表，多个用例并行跑。
        let label = "plugin-test-vis-flip";
        assert!(take_visibility_change(label, true), "首次（表里还没有这一条）应当下发");
        assert!(!take_visibility_change(label, true), "同一个值重复来不出声");
        assert!(take_visibility_change(label, false), "最小化那一次必须下发");
        assert!(!take_visibility_change(label, false), "连续最小化不重复下发");
        assert!(take_visibility_change(label, true), "还原那一次必须下发");
        // 收尾：别把条目留给别的用例
        last_visible().lock().map(|mut g| {
            g.remove(label);
        }).ok();
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
