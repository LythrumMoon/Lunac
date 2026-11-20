// ── Screenshot（§待办 3-28.5 第一步，2026-11-16）─────────────────────
//
// 为什么要有它：模型说「看一眼界面」时只能自己拼 headless 浏览器 / PowerShell 截图脚本；
// 2026-11-01 的手柄图会话因此出了十几版。这一件把「抓一帧」变成一次确定性调用：
// 输入 = 整块虚拟桌面或一个像素矩形，输出 = PNG 文件路径 + 像素尺寸（**不过 IPC 传图**）。
//
// 口径：
//   · 抓的是**虚拟桌面合成后的画面**（多显示器合并，物理像素），走 GDI `BitBlt` ——
//     与 v1 `app/src-tauri/src/screenshot.rs` 同一条路（无 COM / WinRT，几十行就能跑）；
//   · 受 DRM 保护的内容会是黑的；要按窗口抓帧得换 WinRT 那条路，本工具不做（描述里如实写）；
//   · 产物写 `<系统临时目录>\lunac\shots\`，**不进工作区**（不碰用户文件）；
//   · **隐私**：只在模型显式调用时发生；`tools::needs_approval` 里要问，`gated_in_read_only`
//     保证计划相位也会问。不做后台 / 周期性抓屏。
//
// 不做框选 UI：交互式框选是宿主的全屏覆盖窗（v1 的 `screenshot_overlay_*`），属于前端那一半。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::tools::Ctx;

/// 一次抓帧的区域（屏幕物理像素；`x` / `y` 可为负 —— 虚拟桌面左上角不一定是 (0,0)）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub fn run(_ctx: &Ctx, input: &Value) -> Result<String, String> {
    let screen = virtual_rect()?;
    let rect = resolve_rect(input, screen)?;
    let img = capture(rect)?;
    let path = save_png(&img)?;
    let mode = input
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("screen");
    let report = json!({
        "path": path.to_string_lossy(),
        "width": img.width(),
        "height": img.height(),
        "mode": mode,
        "screen": rect_json(screen),
        "captured": rect_json(rect),
    });
    serde_json::to_string_pretty(&report).map_err(|e| format!("结果序列化失败：{e}"))
}

fn rect_json(rect: Rect) -> Value {
    json!({
        "x": rect.x,
        "y": rect.y,
        "width": rect.width,
        "height": rect.height,
    })
}

/// 按入参算出要抓的区域（纯函数，单测不碰屏幕）。
///
/// `mode=screen`（缺省）= 整块虚拟桌面；`mode=rect` = `rect{x,y,width,height}`，
/// 先夹回屏幕内（多屏时坐标可为负），夹完小于 2×2 就报错并给出屏幕尺寸。
pub fn resolve_rect(input: &Value, screen: Rect) -> Result<Rect, String> {
    let mode = input
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("screen");
    match mode {
        "screen" => Ok(screen),
        "rect" => {
            let rect = input
                .get("rect")
                .ok_or("mode=rect requires rect {x,y,width,height}")?;
            let field = |key: &str| -> Result<i64, String> {
                rect.get(key)
                    .and_then(Value::as_i64)
                    .ok_or_else(|| format!("rect.{key} is required (integer)"))
            };
            let (x, y, width, height) = (
                field("x")?,
                field("y")?,
                field("width")?,
                field("height")?,
            );
            if width <= 0 || height <= 0 {
                return Err("rect.width / rect.height must be positive".into());
            }
            clip_rect(x, y, width, height, screen)
        }
        other => Err(format!(
            "mode must be \"screen\" or \"rect\" (got {other:?})"
        )),
    }
}

/// 把请求矩形夹回屏幕；夹完太小 ⇒ 报错（不静默给一张 1px 的图）。
fn clip_rect(x: i64, y: i64, width: i64, height: i64, screen: Rect) -> Result<Rect, String> {
    let screen_right = screen.x as i64 + screen.width as i64;
    let screen_bottom = screen.y as i64 + screen.height as i64;
    let left = x.max(screen.x as i64);
    let top = y.max(screen.y as i64);
    let right = (x + width).min(screen_right);
    let bottom = (y + height).min(screen_bottom);
    let (w, h) = (right - left, bottom - top);
    if w < 2 || h < 2 {
        return Err(format!(
            "rect falls outside the screen (screen is {}x{} at {},{})",
            screen.width, screen.height, screen.x, screen.y
        ));
    }
    Ok(Rect {
        x: left as i32,
        y: top as i32,
        width: w as u32,
        height: h as u32,
    })
}

/// 产物目录：`<系统临时目录>\lunac\shots`（不进工作区）。
fn shots_dir() -> PathBuf {
    std::env::temp_dir().join("lunac").join("shots")
}

/// PNG 落盘，返回绝对路径。
fn save_png(img: &::image::RgbaImage) -> Result<PathBuf, String> {
    use ::image::ImageEncoder as _;
    let dir = shots_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("shot-{stamp}-{}.png", std::process::id()));
    let mut png: Vec<u8> = Vec::new();
    ::image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ::image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("PNG 编码失败：{e}"))?;
    std::fs::write(&path, &png).map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
    Ok(path)
}

/// 虚拟桌面的位置与尺寸（物理像素）。
pub fn virtual_rect() -> Result<Rect, String> {
    #[cfg(target_os = "windows")]
    {
        imp::virtual_rect()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("Screenshot is implemented on Windows only".into())
    }
}

/// 抓一块屏幕区域 → RGBA。
pub fn capture(rect: Rect) -> Result<::image::RgbaImage, String> {
    #[cfg(target_os = "windows")]
    {
        imp::capture(rect)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = rect;
        Err("Screenshot is implemented on Windows only".into())
    }
}

/// 把 RGBA 图降采样成 `cols×rows` 灰度（每格平均亮度，Rec.601 权重）。
///
/// 给「画面变没变」的粗判用（v1 `screenshot_signature` 的口径）；不需要精度。
/// 目前只有单测在调它（生产路径走 `ImageDiff` 的逐像素比对），保留是因为它是 v1
/// 固定镜头指纹的同口径实现，属「视觉回归」那一环的现成零件。
#[allow(dead_code)]
pub fn downsample_gray(img: &::image::RgbaImage, cols: u32, rows: u32) -> Vec<u8> {
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

/// 把一张图写进 shots 目录（测试 / 未来「保存给定图」复用）。
#[allow(dead_code)]
pub fn save_png_named(img: &::image::RgbaImage, name: &str) -> Result<PathBuf, String> {
    let dir = shots_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    let path = dir.join(name);
    write_png(img, &path)?;
    Ok(path)
}

fn write_png(img: &::image::RgbaImage, path: &Path) -> Result<(), String> {
    use ::image::ImageEncoder as _;
    let mut png: Vec<u8> = Vec::new();
    ::image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ::image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("PNG 编码失败：{e}"))?;
    std::fs::write(path, &png).map_err(|e| format!("写 {} 失败：{e}", path.display()))
}

#[cfg(target_os = "windows")]
mod imp {
    use super::Rect;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDCW, DeleteDC, DeleteObject,
        GetDIBits, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
        HGDIOBJ, ROP_CODE, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    /// 虚拟桌面（含多显示器）的左上角与尺寸，物理像素。
    pub fn virtual_rect() -> Result<Rect, String> {
        let x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let w = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
        if w <= 0 || h <= 0 {
            return Err("拿不到屏幕尺寸".into());
        }
        Ok(Rect {
            x,
            y,
            width: w as u32,
            height: h as u32,
        })
    }

    /// 抓一块矩形 → RGBA。整屏抓帧只是「矩形 = 虚拟桌面」的特例，共用同一套 BitBlt 内核。
    pub fn capture(rect: Rect) -> Result<::image::RgbaImage, String> {
        let (wi, hi) = (rect.width as i32, rect.height as i32);
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

            // CAPTUREBLT：连同分层窗口一起抓（否则某些浮层 / 半透明内容会是黑的）。
            let rop = ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0);
            let blit_ok = BitBlt(mem_dc, 0, 0, wi, hi, screen_dc, rect.x, rect.y, rop).is_ok();

            let mut out: Result<::image::RgbaImage, String> = Err("BitBlt 失败".into());
            if blit_ok {
                out = read_pixels(mem_dc, bitmap, wi, hi);
            }

            let _ = SelectObject(mem_dc, old);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(mem_dc);
            let _ = DeleteDC(screen_dc);
            out
        }
    }

    /// 从 DIB 取像素（32bpp 顶朝下），BGRA → RGBA。
    unsafe fn read_pixels(
        mem_dc: windows::Win32::Graphics::Gdi::HDC,
        bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
        w: i32,
        h: i32,
    ) -> Result<::image::RgbaImage, String> {
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        // 负高度 = 顶朝下（第一行就是屏幕最上面一行），省一次翻转。
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
        // BGRA → RGBA，并把 alpha 拉满（GDI 的 alpha 通道常常是 0，留着会全透明）。
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        ::image::RgbaImage::from_raw(w as u32, h as u32, buf)
            .ok_or_else(|| "像素缓冲尺寸不匹配".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }
    }

    #[test]
    fn screen_mode_defaults_to_the_whole_virtual_desktop() {
        let rect = resolve_rect(&json!({}), screen()).unwrap();
        assert_eq!(rect, screen());
        let rect = resolve_rect(&json!({"mode": "screen"}), screen()).unwrap();
        assert_eq!(rect, screen());
    }

    #[test]
    fn rect_mode_clips_to_the_screen_and_keeps_negative_origins() {
        // 负原点的虚拟桌面（副屏在主屏左边）：x=-1920 起。
        let multi = Rect {
            x: -1920,
            y: 0,
            width: 3840,
            height: 1080,
        };
        let rect = resolve_rect(
            &json!({"mode":"rect","rect":{"x":-2000,"y":-50,"width":400,"height":200}}),
            multi,
        )
        .unwrap();
        assert_eq!(
            rect,
            Rect {
                x: -1920,
                y: 0,
                width: 320,
                height: 150
            }
        );
    }

    #[test]
    fn bad_or_empty_rects_are_rejected_with_the_screen_size() {
        let err = resolve_rect(&json!({"mode": "rect"}), screen())
            .err()
            .expect("缺 rect 必须报错");
        assert!(err.contains("rect"), "{err}");

        let err = resolve_rect(
            &json!({"mode":"rect","rect":{"x":5000,"y":5000,"width":100,"height":100}}),
            screen(),
        )
        .err()
        .expect("完全在屏幕外必须报错");
        assert!(err.contains("1920x1080"), "{err}");

        let err = resolve_rect(
            &json!({"mode":"rect","rect":{"x":0,"y":0,"width":0,"height":10}}),
            screen(),
        )
        .err()
        .expect("零宽必须报错");
        assert!(err.contains("positive"), "{err}");

        let err = resolve_rect(&json!({"mode": "window"}), screen())
            .err()
            .expect("未知 mode 必须报错");
        assert!(err.contains("screen"), "{err}");
    }

    /// 灰度指纹：纯色块的平均亮度就是那个颜色（Rec.601）。
    #[test]
    fn downsample_gray_averages_blocks() {
        let img = ::image::RgbaImage::from_pixel(32, 18, ::image::Rgba([255, 255, 255, 255]));
        let sig = downsample_gray(&img, 16, 9);
        assert_eq!(sig.len(), 16 * 9);
        assert!(sig.iter().all(|v| *v == 255), "{sig:?}");

        let dark = ::image::RgbaImage::from_pixel(4, 4, ::image::Rgba([0, 0, 0, 255]));
        assert!(downsample_gray(&dark, 4, 4).iter().all(|v| *v == 0));
    }

    /// **真机（`#[ignore]`）**：真的抓一帧整个虚拟桌面并落 PNG。
    #[test]
    #[ignore]
    fn live_capture_writes_a_png() {
        let screen = virtual_rect().expect("拿不到屏幕尺寸");
        assert!(screen.width > 0 && screen.height > 0);
        let img = capture(screen).expect("抓帧失败");
        assert_eq!((img.width(), img.height()), (screen.width, screen.height));
        let path = save_png(&img).expect("落盘失败");
        let meta = std::fs::metadata(&path).expect("文件不存在");
        assert!(meta.len() > 1024, "PNG 只有 {} 字节", meta.len());
        println!("PASS 抓帧 {}x{} → {}", img.width(), img.height(), path.display());
        let _ = std::fs::remove_file(&path);
    }
}
