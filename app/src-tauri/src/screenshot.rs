// src/screenshot.rs
//! **屏幕截图**（2026-10-06，通用宿主能力）：抓整块虚拟桌面（多显示器合并），
//! 由**宿主自己开一扇全屏置顶覆盖窗**做框选，再把裁剪结果交给调用方。
//!
//! **为什么在宿主**：截屏只能走 Windows 原生 API（WebView 里的 `getDisplayMedia`
//! 在 WebView2 里不可用/要用户选源），而本仓的纪律是「原生能力归宿主」（同 OCR / 播放 / 联网）。
//!
//! **实现选型 = GDI `BitBlt`，不是 `Windows.Graphics.Capture`**：
//!   · BitBlt 无 COM / WinRT、无异步抓帧，几十行就能跑（`windows` crate 已在依赖树里，只加 feature）；
//!   · 代价如实记：**受 DRM 保护的内容会是黑的**（Netflix 这类），且抓的是**虚拟桌面合成后的画面**。
//!   对「框选一段文字做 OCR」这个用法足够；要按窗口/按显示器抓帧再换 WinRT 那条路。
//!
//! **框选为什么必须是一扇独立的全屏置顶窗**（2026-10-06 二改，用户口径「改成 ShareX 那种截屏形式」）：
//! 一改时是在**插件自己的 WebView 里**铺一层 `position: fixed; inset: 0` 的 overlay ——
//! 它只能盖住**那个窗口**，而 OCR 是 `layout.takeover` 型（面板内嵌在主窗里），于是
//! 「全屏框选」实际只等于「Lunac 主窗内框选」，且整屏截图被缩放进窗口才显示（4K 上精度也差）。
//! ShareX 的做法正是**自己开一扇覆盖整个虚拟桌面的无边框置顶窗体**（`RegionCaptureForm`）
//! 再 GDI 抓帧铺上去 —— 本模块复刻这条路，但把「抓帧」留在宿主、把「拖框」交给一页专用前端。
//!
//! **会话态**：抓到的 RGBA 留在宿主（`Session`），覆盖页只来取一张 PNG 用来显示；
//! 框选完前端只回**像素矩形**，由宿主从内存里裁 —— 几 MB 的图不必来回过 IPC。
//!
//! **前端怎么用**：`screenshot_overlay_begin` → 听 `screenshot-overlay-done`（载荷 = 裁剪后的
//! `ScreenShot`）或 `screenshot-overlay-cancelled`。data URL 是**同源**的，画到 canvas 不会污染
//! （用 `asset.localhost` 的本地路径反而会 taint）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

/// 覆盖窗的窗口 label。**刻意不带 `plugin-` 前缀** —— 它不是插件窗口，
/// `main.rs` 的窗口事件分流里为它单开一条（见那里的 `is_overlay`）。
pub const OVERLAY_LABEL: &str = "shot-overlay";

/// 框选完成（载荷 = 裁剪后的 [`ScreenShot`]）。
pub const EVENT_DONE: &str = "screenshot-overlay-done";
/// 框选取消（Esc / 点空处 / 关窗；无载荷）。
pub const EVENT_CANCELLED: &str = "screenshot-overlay-cancelled";

/// 主窗口 label（结果往这里发）。
const MAIN_LABEL: &str = "main";

/// 截屏结果：PNG data URL + 像素尺寸（前端按它算框选映射）。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScreenShot {
    pub data_url: String,
    pub width: u32,
    pub height: u32,
}

/// 一次框选会话。**存的是 RGBA 原始像素**（不是 PNG）：裁剪时直接切片，
/// 不必为了裁一块再把整张 4K 图解码一遍。
struct Session {
    rgba: image::RgbaImage,
}

/// 正在抓帧。**连点两下按钮**时必须挡住第二次抓帧 —— 覆盖窗是置顶的，
/// 第二次抓到的画面里会有**它自己**（递归截图，图像一层套一层）。
static CAPTURING: AtomicBool = AtomicBool::new(false);

fn session() -> &'static Mutex<Option<Session>> {
    static S: OnceLock<Mutex<Option<Session>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// 取出并**清空**会话。`None` = 没有会话，或已经被 `finish` / `cancel` 结算过。
///
/// 「会话在不在」同时就是「这次框选还没结算」的判据 —— `on_overlay_destroyed`
/// 靠它区分「用户 Alt+F4 关掉了」与「正常完成/取消」。
fn take_session() -> Option<Session> {
    session().lock().ok().and_then(|mut g| g.take())
}

// ── 命令 ──────────────────────────────────────────────────────────

/// 发起一次框选：抓帧 → 存会话 → 开覆盖窗。
///
/// **建窗刻意不用 `run_blocking`**（同 `plugin_window::open_plugin_window`）：
/// 建窗不是阻塞 IO，而是「交给事件循环去建」，必须在**非主线程**上发起
/// （主线程发起会与消息泵互等）—— 所以这里 `async fn` 本身就是要的那条非主线程。
#[tauri::command]
pub async fn screenshot_overlay_begin(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    // 覆盖窗已经开着（上一次还没结算）⇒ 只把焦点还给它，**别重抓**
    //（重抓会把已经铺在屏幕上的这扇窗自己拍进去）
    if app.get_webview_window(OVERLAY_LABEL).is_some() {
        return open_overlay(&app);
    }
    // 另一次抓帧正在进行（连点两下）⇒ 忽略这次
    if CAPTURING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    // 抓帧是纯 CPU/IO 重活（4K 全屏 BitBlt），必须离开主线程
    let captured = crate::commands::run_blocking(capture_rgba).await;
    CAPTURING.store(false, Ordering::SeqCst);
    let rgba = captured?;
    {
        let mut guard = session().lock().map_err(|e| format!("lock: {e}"))?;
        *guard = Some(Session { rgba });
    }
    open_overlay(&app)
}

/// 覆盖页启动后来取「要铺在背景上的那张图」。
///
/// 走「**先备好、页面自己来取**」而不是「建窗时把 data URL 塞进 URL / init script」：
/// 与 `plugin_window` 的 PENDING 载荷同一套理由 —— 页面与建窗谁先就绪是不确定的。
#[tauri::command]
pub async fn screenshot_overlay_image() -> Result<ScreenShot, String> {
    crate::commands::run_blocking(|| {
        let guard = session().lock().map_err(|e| format!("lock: {e}"))?;
        let s = guard.as_ref().ok_or_else(|| "ERR_NO_SESSION".to_string())?;
        Ok(ScreenShot {
            data_url: to_data_url(&s.rgba)?,
            width: s.rgba.width(),
            height: s.rgba.height(),
        })
    })
    .await
}

/// 框选完成：按**像素**矩形裁剪 → 关窗 → 把结果发给主窗。
///
/// 坐标是相对**截图左上角**的像素值（前端按 `img` 渲染矩形 ÷ `naturalWidth` 换算）。
#[tauri::command]
pub async fn screenshot_overlay_finish(
    app: tauri::AppHandle,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
) -> Result<(), String> {
    let shot = crate::commands::run_blocking(move || crop(x, y, width, height)).await?;
    close_overlay(&app);
    emit_to_main(&app, EVENT_DONE, shot);
    Ok(())
}

/// 取消框选（Esc / 点空处 / 右键）。
#[tauri::command]
pub fn screenshot_overlay_cancel(app: tauri::AppHandle) -> Result<(), String> {
    let _ = take_session();
    close_overlay(&app);
    emit_to_main(&app, EVENT_CANCELLED, ());
    Ok(())
}

/// 覆盖窗被关掉了（`main.rs` 的 `Destroyed` 分支调）。
///
/// 「会话还在」= 没走 `finish` / `cancel` ⇒ 当作取消，让调用方（OCR 面板）
/// 从「正在截屏…」里退出来 —— 否则用户 Alt+F4 之后面板会一直卡在那句话上。
pub fn on_overlay_destroyed(app: &tauri::AppHandle) {
    if take_session().is_some() {
        emit_to_main(app, EVENT_CANCELLED, ());
    }
}

// ── 内部 ──────────────────────────────────────────────────────────

fn emit_to_main<P: Serialize + Clone>(app: &tauri::AppHandle, event: &str, payload: P) {
    use tauri::{Emitter, Manager};
    if let Some(main) = app.get_webview_window(MAIN_LABEL) {
        let _ = main.emit(event, payload);
    }
}

fn open_overlay(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
    // 已经开着（连点两下按钮）⇒ 复用，别开第二扇
    if let Some(existing) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = existing.set_focus();
        return Ok(());
    }
    let (x, y, w, h) = virtual_rect()?;
    let win = WebviewWindowBuilder::new(app, OVERLAY_LABEL, WebviewUrl::App("screenshot.html".into()))
        .title("Lunac")
        // 无边框 + 置顶 + 不进任务栏 = ShareX 那个 `RegionCaptureForm` 的形态
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        // 先不显示：几何要用**物理**单位摆准了再亮相（否则会先闪一下默认尺寸的白窗）
        .visible(false)
        .build()
        .map_err(|e| format!("建截屏覆盖窗失败：{e}"))?;

    // ⚠️ builder 的 `position` / `inner_size` 收的是**逻辑**单位（DPI 缩放下 ≠ 物理像素），
    // 而我们要铺满的是**整块虚拟桌面**（`GetSystemMetrics` 给的是物理像素）——
    // 所以建完再用物理 API 摆一次。用显式几何而不是 `fullscreen(true)`：
    // 后者在 Windows 上只覆盖窗口所在的那一台显示器。
    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    let _ = win.set_size(tauri::PhysicalSize::new(w, h));
    let _ = win.show();
    let _ = win.set_focus();
    crate::log::info("screenshot: overlay opened");
    Ok(())
}

fn close_overlay(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window(OVERLAY_LABEL) {
        // 用 `destroy()` 而不是 `close()`：`close()` 会先问 `CloseRequested`，
        // 而那条路在 `main.rs` 里被「关掉=收进托盘」占用着（虽然已按 label 分流）。
        // 直接销毁可以**不依赖**那条分流也仍然正确 —— 少一处隐式耦合。
        let _ = w.destroy();
    }
}

/// 从会话里裁一块。矩形先夹回图内（前端算出来的本该已在界内，兜一次底）。
fn crop(x: i64, y: i64, w: i64, h: i64) -> Result<ScreenShot, String> {
    let s = take_session().ok_or_else(|| "ERR_NO_SESSION".to_string())?;
    let iw = s.rgba.width() as i64;
    let ih = s.rgba.height() as i64;
    let cx = x.clamp(0, iw);
    let cy = y.clamp(0, ih);
    let cw = w.min(iw - cx).max(0);
    let ch = h.min(ih - cy).max(0);
    if cw < 2 || ch < 2 {
        return Err("ERR_TOO_SMALL".into());
    }
    let sub = image::imageops::crop_imm(&s.rgba, cx as u32, cy as u32, cw as u32, ch as u32).to_image();
    Ok(ScreenShot {
        data_url: to_data_url(&sub)?,
        width: cw as u32,
        height: ch as u32,
    })
}

/// RGBA → PNG data URL。**流式编码，不 clone 像素缓冲**（4K 虚拟桌面那一份几十 MB）。
pub fn to_data_url(img: &image::RgbaImage) -> Result<String, String> {
    use base64::Engine as _;
    use image::ImageEncoder as _;
    let mut png: Vec<u8> = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("PNG 编码失败：{e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    Ok(format!("data:image/png;base64,{b64}"))
}

/// 抓整块虚拟桌面 → RGBA。
pub fn capture_rgba() -> Result<image::RgbaImage, String> {
    #[cfg(target_os = "windows")]
    {
        imp::capture_rgba()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("截屏目前只在 Windows 上实现".into())
    }
}

/// 虚拟桌面的位置与尺寸（物理像素）。
pub fn virtual_rect() -> Result<(i32, i32, u32, u32), String> {
    #[cfg(target_os = "windows")]
    {
        imp::virtual_rect()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("截屏目前只在 Windows 上实现".into())
    }
}

/// 抓屏幕上一块矩形（物理像素）→ RGBA。
pub fn capture_rgba_rect(x: i32, y: i32, w: u32, h: u32) -> Result<image::RgbaImage, String> {
    #[cfg(target_os = "windows")]
    {
        imp::capture_rgba_rect(x, y, w, h)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (x, y, w, h);
        Err("截屏目前只在 Windows 上实现".into())
    }
}

/// 目标显示器的矩形（物理像素）。`label = Some(窗口 label)` 且该窗口存在时取
/// **那扇窗所在的显示器**（桌宠 `plugin-pet` 用的就是这个），否则回退到主显示器。
///
/// 为什么用 `MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)` 而不是自己枚举
/// 显示器：系统会按窗口/坐标**就近**判定归属，多显示器、DPI 缩放、窗口跨屏这些
/// 情况都不必自己处理（本仓也没有任何显示器枚举代码）。句柄为 `None` 时传
/// `HWND::default()`（NULL），`MONITOR_DEFAULTTONEAREST` 对 NULL 给的就是主显示器。
fn monitor_rect(
    app: &tauri::AppHandle,
    label: Option<&str>,
) -> Result<(i32, i32, u32, u32), String> {
    #[cfg(target_os = "windows")]
    {
        use tauri::Manager;
        // 窗口存在 → 取其 HWND 并换成**本 crate `windows` 版本**的 HWND：
        // Tauri 依赖的是另一版 `windows`（0.61），两者的 HWND 只是同一个
        // `*mut c_void` 的 newtype，`h.0` 直接搬过来即可（同 `hotkey.rs` 的写法）。
        let hwnd = label
            .and_then(|l| app.get_webview_window(l))
            .and_then(|w| w.hwnd().ok())
            .map(|h| windows::Win32::Foundation::HWND(h.0));
        imp::monitor_rect(hwnd)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, label);
        Err("截屏目前只在 Windows 上实现".into())
    }
}

/// 编码成 PNG 写进**临时目录**，返回文件绝对路径。
///
/// **为什么返回路径而不是 data URL**：4K 整屏的 PNG data URL 有十几 MB，
/// 过 IPC 既慢又占内存（ai-spec §4.5「几 MB 的图不过 IPC」）。调用方（桌宠插件）
/// 直接读这个本地文件即可。
fn write_png_temp(img: &image::RgbaImage) -> Result<String, String> {
    use image::ImageEncoder as _;
    let mut png: Vec<u8> = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("PNG 编码失败：{e}"))?;
    let path = crate::commands::temp_image_path("shot", "png");
    std::fs::write(&path, &png).map_err(|e| format!("写临时文件失败：{e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// 抓「当前显示器」整屏存成临时 PNG 的返回：**文件路径** + 像素尺寸（桌宠插件用）。
#[derive(serde::Serialize)]
pub struct ShotFile {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

/// 抓「当前显示器」整屏 → 存临时 PNG → 返回文件路径。
///
/// 目标显示器：`label` 给了且窗口存在 → 该窗口所在的显示器；否则主显示器。
///
/// **⚠️ 隐私（硬约束，预检 #72 ①，2026-10-10 修订）**：抓屏**只在用户显式触发**时发生 ——
/// 用户手势（桌宠右键「看一眼屏幕」）或用户**主动开启**的「主动反应」子选项（自动抓屏 /
/// 屏幕变化触发）。后者是显式 opt-in + 默认关 + 有间隔与每日上限 + 桌宠不可见即停，
/// 且界面上有可见提示。**除这两条外**，任何后台 / 周期性抓屏都不许接进来。
#[tauri::command]
pub async fn screenshot_monitor(
    app: tauri::AppHandle,
    label: Option<String>,
) -> Result<ShotFile, String> {
    // 抓帧 + PNG 编码是纯 CPU/IO 重活（4K 整屏），必须离开主线程（预检 #72 ③）
    crate::commands::run_blocking(move || {
        let (x, y, w, h) = monitor_rect(&app, label.as_deref())?;
        let img = capture_rgba_rect(x, y, w, h)?;
        let path = write_png_temp(&img)?;
        Ok(ShotFile {
            path,
            width: img.width(),
            height: img.height(),
        })
    })
    .await
}

/// 降采样后的指纹尺寸（`SIG_COLS × SIG_ROWS` 灰度字节）。
const SIG_COLS: u32 = 16;
const SIG_ROWS: u32 = 9;

/// 抓「当前显示器」整屏 → **降采样成 16×9 灰度**（144 字节）返回，**不落盘**。
///
/// 「屏幕变化触发」（2026-10-10，主动反应的一个**显式 opt-in** 子开关）要周期性地比对
/// 画面，不能每次都往临时目录落一个几 MB 的 PNG。这里只回一个极小的指纹：宿主做完
/// BitBlt + 降采样，前端拿前后两组指纹算「平均绝对差」，超过阈值就当画面变了。
///
/// **隐私**：与 `screenshot_monitor` 同一条口径（见那里的注释）—— 只由用户显式开启的
/// 主动反应子选项触发，默认关、有频次上限、桌宠不可见时一律不抓。
#[tauri::command]
pub async fn screenshot_signature(
    app: tauri::AppHandle,
    label: Option<String>,
) -> Result<Vec<u8>, String> {
    crate::commands::run_blocking(move || {
        let (x, y, w, h) = monitor_rect(&app, label.as_deref())?;
        let img = capture_rgba_rect(x, y, w, h)?;
        Ok(downsample_gray(&img, SIG_COLS, SIG_ROWS))
    })
    .await
}

/// 把 RGBA 图降采样成 `cols×rows` 灰度（每格取平均亮度，Rec.601 权重）。
/// 只用于「画面变没变」的粗判，不需要精度。
fn downsample_gray(img: &image::RgbaImage, cols: u32, rows: u32) -> Vec<u8> {
    let (w, h) = (img.width().max(1), img.height().max(1));
    let mut out = vec![0u8; (cols * rows) as usize];
    for gy in 0..rows {
        for gx in 0..cols {
            let x0 = (gx * w) / cols;
            let x1 = (((gx + 1) * w) / cols).max(x0 + 1).min(w);
            let y0 = (gy * h) / rows;
            let y1 = (((gy + 1) * h) / rows).max(y0 + 1).min(h);
            let (mut sum, mut n) = (0u64, 0u64);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = img.get_pixel(x, y).0;
                    sum += (299 * p[0] as u64 + 587 * p[1] as u64 + 114 * p[2] as u64) / 1000;
                    n += 1;
                }
            }
            out[(gy * cols + gx) as usize] = if n > 0 { (sum / n) as u8 } else { 0 };
        }
    }
    out
}

#[cfg(target_os = "windows")]
mod imp {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDCW, DeleteDC, DeleteObject,
        GetDIBits, GetMonitorInfoW, MonitorFromWindow, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        ROP_CODE, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    /// 虚拟桌面（含多显示器）的左上角与尺寸，物理像素。
    pub fn virtual_rect() -> Result<(i32, i32, u32, u32), String> {
        let x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let w = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
        if w <= 0 || h <= 0 {
            return Err("拿不到屏幕尺寸".into());
        }
        Ok((x, y, w as u32, h as u32))
    }

    pub fn capture_rgba() -> Result<image::RgbaImage, String> {
        let (x, y, w, h) = virtual_rect()?;
        capture_rgba_rect(x, y, w, h)
    }

    /// 抓屏幕上一块矩形（物理像素）→ RGBA。`x` / `y` 是**屏幕坐标**左上角，
    /// `w` / `h` 是截取范围。整屏抓帧只是「矩形 = 整块虚拟桌面」的特例，
    /// 共用同一套 BitBlt 内核 —— 参数化后按显示器抓帧不必再写第二份。
    pub fn capture_rgba_rect(x: i32, y: i32, w: u32, h: u32) -> Result<image::RgbaImage, String> {
        let (wi, hi) = (w as i32, h as i32);
        if wi <= 0 || hi <= 0 {
            return Err("抓帧区域尺寸非法".into());
        }

        unsafe {
            let screen_dc = CreateDCW(w!("DISPLAY"), PCWSTR::null(), PCWSTR::null(), None);
            if screen_dc.is_invalid() {
                return Err("CreateDCW(DISPLAY) 失败".into());
            }
            let mem_dc = CreateCompatibleDC(screen_dc);
            if mem_dc.is_invalid() {
                let _ = DeleteDC(screen_dc);
                return Err("CreateCompatibleDC 失败".into());
            }
            let bitmap = CreateCompatibleBitmap(screen_dc, wi, hi);
            if bitmap.is_invalid() {
                let _ = DeleteDC(mem_dc);
                let _ = DeleteDC(screen_dc);
                return Err("CreateCompatibleBitmap 失败".into());
            }
            let old = SelectObject(mem_dc, HGDIOBJ(bitmap.0));

            // CAPTUREBLT：连同分层窗口一起抓（否则某些浮层/半透明内容会是黑的）
            let rop = ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0);
            let blit_ok = BitBlt(mem_dc, 0, 0, wi, hi, screen_dc, x, y, rop).is_ok();

            let mut out: Result<image::RgbaImage, String> = Err("BitBlt 失败".into());
            if blit_ok {
                out = read_pixels(mem_dc, bitmap, wi, hi);
            }

            // 释放（顺序与创建相反；SelectObject 先把原对象放回去）
            let _ = SelectObject(mem_dc, old);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(mem_dc);
            let _ = DeleteDC(screen_dc);
            out
        }
    }

    /// 目标显示器的矩形（物理像素）。`hwnd = Some(窗口)` 取该窗口所在的那台，
    /// `None`（或句柄无效）取主显示器 —— 两者都靠 `MONITOR_DEFAULTTONEAREST`
    /// 由系统就近判定，不自己枚举显示器。
    pub fn monitor_rect(hwnd: Option<HWND>) -> Result<(i32, i32, u32, u32), String> {
        unsafe {
            let monitor = MonitorFromWindow(hwnd.unwrap_or_default(), MONITOR_DEFAULTTONEAREST);
            if monitor.is_invalid() {
                return Err("MonitorFromWindow 失败".into());
            }
            let mut mi = MONITORINFO::default();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if !GetMonitorInfoW(monitor, &mut mi).as_bool() {
                return Err("GetMonitorInfoW 失败".into());
            }
            // rcMonitor 是物理像素的屏幕坐标；转成 (left, top, width, height)
            let r = mi.rcMonitor;
            Ok((
                r.left,
                r.top,
                (r.right - r.left) as u32,
                (r.bottom - r.top) as u32,
            ))
        }
    }

    /// 从 DIB 取出像素（32bpp 顶朝下），BGRA → RGBA。
    unsafe fn read_pixels(
        mem_dc: windows::Win32::Graphics::Gdi::HDC,
        bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
        w: i32,
        h: i32,
    ) -> Result<image::RgbaImage, String> {
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        // 负高度 = 顶朝下（第一行就是屏幕最上面一行），省一次翻转
        bmi.bmiHeader.biHeight = -h;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0 as u32;

        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let lines = GetDIBits(
            mem_dc,
            bitmap,
            0,
            h as u32,
            Some(buf.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        if lines == 0 {
            return Err("GetDIBits 失败".into());
        }
        // BGRA → RGBA，并把 alpha 拉满（GDI 的 alpha 通道常常是 0，留着会全透明）
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        image::RgbaImage::from_raw(w as u32, h as u32, buf)
            .ok_or_else(|| "像素缓冲尺寸不匹配".to_string())
    }
}
