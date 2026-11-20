// ── ImageAnnotate（§待办 3-19 第二步，2026-10-30）────────────────────
//
// 为什么要有它：`ImageMeasure` 把「量」从模型手里拿走，这一件把「画」也拿走。
// 2026-10-27 / 29 那次「四杯照片标尺寸」的会话里，模型每轮现写一份 PowerShell +
// `System.Drawing` 脚本：坐标常量埋在脚本里（改一处错一处）、自检只 `Write-Host` 打印
// 像素值（没有断言）、中文还被 PowerShell 5.1 按 GB2312 读成乱码（`.ps1` 存成 UTF-8 无 BOM）。
// 现在模型只产出**规格 JSON**，绘制一次做完：
//   · 绘制 = 系统 GDI+（`gdiplus.dll`）：字体直接用系统已装字体（微软雅黑优先），
//     **不随包分发字体**，也不再有「脚本文件编码」这一环；
//   · 输出 = PNG（由 `image` crate 编码，与 `ImageMeasure` 共用同一套解码口径）。
//
// 边界（与 `ImageMeasure` 刻意不同）：
//   · 它**写盘** ⇒ 进 [`crate::tools::needs_approval`]，只读档 / 计划相位由
//     [`crate::tools::write_blocked`] 直接拒；**不进** [`crate::tools::parallel_safe`]。
//   · 源图走 `tools::resolve_read_path`（与 `Read` 同一套工作区锁 / 禁读目录闸），
//     输出走 `tools::resolve_write_path`（同一套越界闸）—— 不新造第二套路径归一。
//   · 只在 Windows 上实现；其它平台**如实报错**（本工具链只出 Windows 包，不做假的成功）。
//   · 坐标越界**直接拒**并报出图的真实尺寸：这正是「模型把坐标整体算低 60–70 px」那一类
//     错误的确定性拦截点，比让它自己看图纠正便宜得多。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::tools::Ctx;

/// 一次调用最多画多少个图元（防住把整张图铺满的规格）。
const MAX_ITEMS: usize = 200;
/// 默认颜色 = 不透明红（`#FF3B30`）—— 机械标注里最不容易和照片撞色的一种。
const DEFAULT_COLOR: u32 = 0xFFFF_3B30;
const DEFAULT_WIDTH: f32 = 2.0;
const DEFAULT_ARROW_SIZE: f32 = 14.0;
const DEFAULT_FONT_SIZE: f32 = 20.0;
/// 尺寸线标签的默认法向偏移：**负数 = 法线反方向**（水平线时落在线上方 14 px）。
const DEFAULT_LABEL_OFFSET: f32 = -14.0;
/// 文字底色的默认内边距。
const DEFAULT_BG_PAD: f32 = 4.0;
/// 中文字体候选：**只用系统字体**，不随包分发（见文件头）。
const FONT_CANDIDATES: [&str; 4] = ["Microsoft YaHei", "SimHei", "SimSun", "Arial"];
/// 顶层允许的键（拼错键名 = 悄悄画出错东西，所以一条都不许留）。
const TOP_KEYS: &[&str] = &["file_path", "output_path", "items", "font_family"];
/// 每个图元允许的键。
const ITEM_KEYS: &[&str] = &[
    "type",
    "from",
    "to",
    "at",
    "text",
    "color",
    "width",
    "arrow",
    "arrow_dir",
    "arrow_size",
    "size",
    "align",
    "label_offset",
    "bg",
    "pad",
];

// ── 入口 ─────────────────────────────────────────────────────────

pub fn run(ctx: &Ctx, input: &Value) -> Result<String, String> {
    // 先过写档位闸：只读档 / 计划相位下**连读源图都不做**，直接如实拒。
    // 文案与 `Write` 同一句（同一个 `write_blocked`），不另写一份判据。
    if let Some(why) = crate::tools::write_blocked(ctx, "ImageAnnotate") {
        return Err(why);
    }
    check_keys(input, TOP_KEYS, "input")?;

    let src_raw = input
        .get("file_path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if src_raw.is_empty() {
        return Err("file_path is required".into());
    }
    let src = crate::tools::resolve_read_path(ctx, src_raw)?;

    let items = parse_items(input)?;

    let out = match input
        .get("output_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => crate::tools::resolve_write_path(ctx, "ImageAnnotate", raw)?,
        None => {
            // 缺省落在源图旁边：`<stem>.annotated.png`。同样过越界闸（工作区锁）。
            let derived = default_output(&src);
            crate::tools::resolve_write_path(ctx, "ImageAnnotate", &derived.to_string_lossy())?
        }
    };
    if out == src {
        return Err("output_path must differ from file_path (ImageAnnotate never overwrites the source image)".into());
    }

    let family = input
        .get("font_family")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    render(&src, &out, &items, family)
}

fn default_output(src: &Path) -> PathBuf {
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
    src.with_file_name(format!("{stem}.annotated.png"))
}

// ── 规格解析（平台无关）──────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
struct Pt(f32, f32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Line,
    Dimension,
    Arrow,
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arrows {
    None,
    Start,
    End,
    Both,
}

/// 箭头「身体」长在线段的哪一侧：
///   · `Out`（默认，制图标准）：尖端落在端点、三角底边**在线段内侧**（两个箭头尖端相背）；
///   · `In`（空间不足时的画法）：三角画在端点**外侧**、尖端仍落在端点并朝中点指。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArrowDir {
    Out,
    In,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug)]
struct Item {
    kind: Kind,
    from: Pt,
    to: Pt,
    /// `text` 图元的锚点；`dimension` 的标签锚点（缺省 = 自动放在中点法线方向）。
    at: Option<Pt>,
    color: u32,
    width: f32,
    arrows: Arrows,
    arrow_dir: ArrowDir,
    arrow_size: f32,
    /// `text` 的正文 / `dimension` 的标签；`line` / `arrow` 不用。
    text: Option<String>,
    label_offset: f32,
    size: f32,
    align: Align,
    bg: Option<u32>,
    pad: f32,
}

fn check_keys(input: &Value, allowed: &[&str], where_: &str) -> Result<(), String> {
    let Some(obj) = input.as_object() else {
        return Err(format!("{where_} must be an object"));
    };
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "{where_}: unknown key `{key}` (allowed: {})",
                allowed.join(", ")
            ));
        }
    }
    Ok(())
}

fn parse_items(input: &Value) -> Result<Vec<Item>, String> {
    let arr = input
        .get("items")
        .and_then(Value::as_array)
        .ok_or("items is required and must be an array of drawing items")?;
    if arr.is_empty() {
        return Err("items must not be empty".into());
    }
    if arr.len() > MAX_ITEMS {
        return Err(format!(
            "items has {} entries (limit {MAX_ITEMS})",
            arr.len()
        ));
    }
    arr.iter()
        .enumerate()
        .map(|(i, v)| parse_item(v, i))
        .collect()
}

fn parse_item(v: &Value, index: usize) -> Result<Item, String> {
    let where_ = format!("items[{index}]");
    check_keys(v, ITEM_KEYS, &where_)?;
    let obj = v.as_object().expect("check_keys 已确认是对象");

    let kind = match obj.get("type").and_then(Value::as_str) {
        Some("line") => Kind::Line,
        Some("dimension") => Kind::Dimension,
        Some("arrow") => Kind::Arrow,
        Some("text") => Kind::Text,
        Some(other) => {
            return Err(format!(
                "{where_}: unknown type `{other}` (line / dimension / arrow / text)"
            ))
        }
        None => return Err(format!("{where_}: `type` is required")),
    };

    let from = point(obj, "from", &where_)?;
    let to = point(obj, "to", &where_)?;
    if kind != Kind::Text && (from.is_none() || to.is_none()) {
        return Err(format!("{where_}: `from` and `to` are required"));
    }
    let at = point(obj, "at", &where_)?;

    let text = match obj.get("text") {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::String(_)) => return Err(format!("{where_}: `text` must not be empty")),
        Some(_) => return Err(format!("{where_}: `text` must be a string")),
        None => None,
    };
    if kind == Kind::Text {
        if text.is_none() {
            return Err(format!("{where_}: `text` is required for type `text`"));
        }
        if at.is_none() {
            return Err(format!(
                "{where_}: `at` is required for type `text` (the anchor point)"
            ));
        }
    }

    let arrows = match obj.get("arrow").and_then(Value::as_str) {
        Some("none") => Arrows::None,
        Some("start") => Arrows::Start,
        Some("end") => Arrows::End,
        Some("both") => Arrows::Both,
        Some(other) => {
            return Err(format!(
                "{where_}: unknown arrow `{other}` (none / start / end / both)"
            ))
        }
        None => match kind {
            Kind::Dimension => Arrows::Both,
            Kind::Arrow => Arrows::End,
            _ => Arrows::None,
        },
    };
    let arrow_dir = match obj.get("arrow_dir").and_then(Value::as_str) {
        Some("out") => ArrowDir::Out,
        Some("in") => ArrowDir::In,
        Some(other) => {
            return Err(format!(
            "{where_}: unknown arrow_dir `{other}` (out = 尖在端点、底边朝内；in = 三角画在外侧)"
        ))
        }
        None => ArrowDir::Out,
    };
    let align = match obj.get("align").and_then(Value::as_str) {
        Some("left") => Align::Left,
        Some("center") => Align::Center,
        Some("right") => Align::Right,
        Some(other) => {
            return Err(format!(
                "{where_}: unknown align `{other}` (left / center / right)"
            ))
        }
        None => {
            if kind == Kind::Text {
                Align::Left
            } else {
                Align::Center
            }
        }
    };

    Ok(Item {
        kind,
        from: from.unwrap_or(Pt(0.0, 0.0)),
        to: to.unwrap_or(Pt(0.0, 0.0)),
        at,
        color: color(obj, "color", DEFAULT_COLOR, &where_)?,
        width: number(obj, "width", DEFAULT_WIDTH, 0.5, 64.0, &where_)?,
        arrows,
        arrow_dir,
        arrow_size: number(obj, "arrow_size", DEFAULT_ARROW_SIZE, 4.0, 200.0, &where_)?,
        text,
        label_offset: number(
            obj,
            "label_offset",
            DEFAULT_LABEL_OFFSET,
            -2_000.0,
            2_000.0,
            &where_,
        )?,
        size: number(obj, "size", DEFAULT_FONT_SIZE, 6.0, 400.0, &where_)?,
        align,
        bg: match obj.get("bg") {
            None => None,
            Some(_) => Some(color(obj, "bg", 0x8000_0000, &where_)?),
        },
        pad: number(obj, "pad", DEFAULT_BG_PAD, 0.0, 64.0, &where_)?,
    })
}

fn point(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    where_: &str,
) -> Result<Option<Pt>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None);
    };
    let Some(p) = v.as_object() else {
        return Err(format!(
            "{where_}: `{key}` must be {{\"x\": <px>, \"y\": <px>}}"
        ));
    };
    for k in p.keys() {
        if k != "x" && k != "y" {
            return Err(format!("{where_}: `{key}` has unknown key `{k}`"));
        }
    }
    let x = p
        .get("x")
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("{where_}: `{key}.x` must be a number"))? as f32;
    let y = p
        .get("y")
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("{where_}: `{key}.y` must be a number"))? as f32;
    if !x.is_finite() || !y.is_finite() {
        return Err(format!("{where_}: `{key}` must be finite numbers"));
    }
    Ok(Some(Pt(x, y)))
}

fn number(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    default: f32,
    lo: f32,
    hi: f32,
    where_: &str,
) -> Result<f32, String> {
    let Some(v) = obj.get(key) else {
        return Ok(default);
    };
    let n = v
        .as_f64()
        .ok_or_else(|| format!("{where_}: `{key}` must be a number"))? as f32;
    if !n.is_finite() || n < lo || n > hi {
        return Err(format!("{where_}: `{key}` out of range [{lo}, {hi}]"));
    }
    Ok(n)
}

/// 颜色：`#RGB` / `#RRGGBB` / `#RRGGBBAA`（**CSS 顺序，alpha 在最后**），转 GDI+ 的 `0xAARRGGBB`。
fn color(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    default: u32,
    where_: &str,
) -> Result<u32, String> {
    let Some(v) = obj.get(key) else {
        return Ok(default);
    };
    let s = v
        .as_str()
        .ok_or_else(|| format!("{where_}: `{key}` must be a string like \"#FF3B30\""))?;
    parse_hex(s).map_err(|e| format!("{where_}: `{key}` {e}"))
}

fn parse_hex(raw: &str) -> Result<u32, String> {
    let t = raw.trim().trim_start_matches('#');
    let (rgb, alpha) = match t.len() {
        3 => {
            let mut expanded = String::with_capacity(6);
            for ch in t.chars() {
                expanded.push(ch);
                expanded.push(ch);
            }
            (expanded, "FF".to_string())
        }
        6 => (t.to_string(), "FF".to_string()),
        8 => (t[..6].to_string(), t[6..].to_string()),
        _ => return Err(format!("`{raw}` is not #RGB / #RRGGBB / #RRGGBBAA")),
    };
    let byte = |s: &str| u8::from_str_radix(s, 16).map_err(|_| format!("`{raw}` is not hex"));
    let (r, g, b) = (byte(&rgb[0..2])?, byte(&rgb[2..4])?, byte(&rgb[4..6])?);
    let a = byte(&alpha)?;
    Ok((u32::from(a) << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b))
}

// ── 平台入口 ─────────────────────────────────────────────────────

#[cfg(windows)]
fn render(src: &Path, out: &Path, items: &[Item], family: Option<&str>) -> Result<String, String> {
    let report = gdi::render(src, out, items, family)?;
    serde_json::to_string_pretty(&report).map_err(|e| format!("结果序列化失败：{e}"))
}

#[cfg(not(windows))]
fn render(
    _src: &Path,
    _out: &Path,
    _items: &[Item],
    _family: Option<&str>,
) -> Result<String, String> {
    Err(
        "ImageAnnotate is implemented on Windows only (this toolchain ships Windows builds); \
         use PowerShell + System.Drawing there instead"
            .into(),
    )
}

// ── GDI+ 绘制（Windows）──────────────────────────────────────────

#[cfg(windows)]
mod gdi {
    use super::*;
    use std::sync::OnceLock;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::Graphics::GdiPlus::*;
    // ⚠️ 必须显式再导一次 `Ok`：GDI+ 那一坨里也有一个 `pub const Ok: Status`，
    // glob 导入会把它带进来，于是本模块里所有 `Ok(...)` 都变成「调用一个 Status 常量」。
    // 显式 use 的优先级高于 glob，这一行就是修这个影子。
    use std::result::Result::Ok;

    /// 32bpp ARGB 的像素格式值（Gdipluspixelformats.h）。`windows` 0.58 没导出这个常量，
    /// 只导出以 `i32` 接收它的 `GdipCreateBitmapFromScan0`。
    const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x0026_200A;

    fn ok(status: Status) -> bool {
        status.0 == 0
    }

    fn check(status: Status, what: &str) -> Result<(), String> {
        if ok(status) {
            Ok(())
        } else {
            Err(format!("GDI+ {what} 失败（status {}）", status.0))
        }
    }

    /// 进程内只启动一次 GDI+，**刻意不 Shutdown**：`agent.exe` 是常驻进程，
    /// 每次调用启停既慢又要处理「启动时的绘制还在跑」的竞态，而 GDI+ 的占用可以忽略。
    fn ensure_started() -> Result<(), String> {
        static STATE: OnceLock<Option<String>> = OnceLock::new();
        let failure = STATE.get_or_init(|| unsafe {
            let input = GdiplusStartupInput {
                GdiplusVersion: 1,
                DebugEventCallback: 0,
                SuppressBackgroundThread: BOOL(0),
                SuppressExternalCodecs: BOOL(0),
            };
            let mut token = 0usize;
            let mut output = GdiplusStartupOutput::default();
            let status = GdiplusStartup(&mut token, &input, &mut output);
            if ok(status) {
                None
            } else {
                Some(format!("GDI+ 初始化失败（status {}）", status.0))
            }
        });
        match failure {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }

    // ── RAII 句柄：任何一条错误路径都不会漏释放 ──────────────────

    struct Pen(*mut GpPen);
    impl Drop for Pen {
        fn drop(&mut self) {
            unsafe {
                let _ = GdipDeletePen(self.0);
            }
        }
    }

    struct Brush(*mut GpSolidFill);
    impl Drop for Brush {
        fn drop(&mut self) {
            unsafe {
                let _ = GdipDeleteBrush(self.0 as *mut GpBrush);
            }
        }
    }

    struct Font(*mut GpFont);
    impl Drop for Font {
        fn drop(&mut self) {
            unsafe {
                let _ = GdipDeleteFont(self.0);
            }
        }
    }

    struct Family(*mut GpFontFamily, String);
    impl Drop for Family {
        fn drop(&mut self) {
            unsafe {
                let _ = GdipDeleteFontFamily(self.0);
            }
        }
    }

    struct Fmt(*mut GpStringFormat);
    impl Drop for Fmt {
        fn drop(&mut self) {
            unsafe {
                let _ = GdipDeleteStringFormat(self.0);
            }
        }
    }

    struct Canvas {
        gfx: *mut GpGraphics,
        bmp: *mut GpBitmap,
    }
    impl Drop for Canvas {
        fn drop(&mut self) {
            unsafe {
                let _ = GdipDeleteGraphics(self.gfx);
                let _ = GdipDisposeImage(self.bmp as *mut GpImage);
            }
        }
    }

    fn pen(color: u32, width: f32) -> Result<Pen, String> {
        let mut p: *mut GpPen = std::ptr::null_mut();
        unsafe { check(GdipCreatePen1(color, width, UnitPixel, &mut p), "建画笔")? };
        Ok(Pen(p))
    }

    fn brush(color: u32) -> Result<Brush, String> {
        let mut b: *mut GpSolidFill = std::ptr::null_mut();
        unsafe { check(GdipCreateSolidFill(color, &mut b), "建画刷")? };
        Ok(Brush(b))
    }

    impl Family {
        /// 找一族**系统已装**的字体。`override_name` 给了就只试它（失败如实报错，
        /// 不悄悄换成别的字体 —— 那会让「中文显示成方框」变得难以察觉）。
        fn load(override_name: Option<&str>) -> Result<Self, String> {
            let names: Vec<&str> = match override_name {
                Some(n) => vec![n],
                None => FONT_CANDIDATES.to_vec(),
            };
            for name in &names {
                let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
                let mut family: *mut GpFontFamily = std::ptr::null_mut();
                let status = unsafe {
                    GdipCreateFontFamilyFromName(
                        PCWSTR(wide.as_ptr()),
                        std::ptr::null_mut(),
                        &mut family,
                    )
                };
                // 已知怪癖：字体不存在时 GDI+ 可能回 Ok 但给空指针 —— 两个都要看。
                if ok(status) && !family.is_null() {
                    return Ok(Self(family, (*name).to_string()));
                }
            }
            Err(format!(
                "找不到可用字体（试过 {}）—— 用 font_family 指一个系统已装字体",
                names.join(" / ")
            ))
        }

        fn ptr(&self) -> *const GpFontFamily {
            self.0
        }
    }

    impl Fmt {
        fn new() -> Result<Self, String> {
            let mut f: *mut GpStringFormat = std::ptr::null_mut();
            unsafe {
                check(GdipCreateStringFormat(0, 0, &mut f), "建文本格式")?;
                // 不换行 + 左上对齐：我们自己用测量出的 bbox 定位，交给 GDI+ 排版会引入偏差。
                let _ = GdipSetStringFormatFlags(f, StringFormatFlagsNoWrap.0);
                let _ = GdipSetStringFormatAlign(f, StringAlignment(0));
                let _ = GdipSetStringFormatLineAlign(f, StringAlignment(0));
            }
            Ok(Self(f))
        }
    }

    impl Canvas {
        fn new(scan0: *mut u8, w: u32, h: u32) -> Result<Self, String> {
            let mut bmp: *mut GpBitmap = std::ptr::null_mut();
            unsafe {
                // `GdipCreateBitmapFromScan0` **引用**这块缓冲（不拷贝）⇒ 绘制直接落在我们的
                // `Vec<u8>` 上，省掉一次 LockBits 往返。缓冲生命周期由调用方保证。
                check(
                    GdipCreateBitmapFromScan0(
                        w as i32,
                        h as i32,
                        (w * 4) as i32,
                        PIXEL_FORMAT_32BPP_ARGB,
                        Some(scan0 as *const u8),
                        &mut bmp,
                    ),
                    "创建位图",
                )?;
            }
            let mut gfx: *mut GpGraphics = std::ptr::null_mut();
            unsafe {
                let status = GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut gfx);
                if !ok(status) {
                    let _ = GdipDisposeImage(bmp as *mut GpImage);
                    return Err(format!("GDI+ 取绘制上下文失败（status {}）", status.0));
                }
            }
            Ok(Self { gfx, bmp })
        }
    }

    // ── 主流程 ─────────────────────────────────────────────────

    pub fn render(
        src: &Path,
        out: &Path,
        items: &[Item],
        family: Option<&str>,
    ) -> Result<Value, String> {
        ensure_started()?;
        let (w, h, mut pixels) = load_bgra(src)?;
        for (i, item) in items.iter().enumerate() {
            check_bounds(item, i, w, h)?;
        }

        let mut resolved: Vec<Value> = Vec::with_capacity(items.len());
        let mut font_name = String::new();
        {
            let canvas = Canvas::new(pixels.as_mut_ptr(), w, h)?;
            unsafe {
                // 线要抗锯齿（尺寸线多是斜的），文字要网格拟合（小字号中文更干净）。
                let _ = GdipSetSmoothingMode(canvas.gfx, SmoothingModeAntiAlias);
                let _ = GdipSetTextRenderingHint(canvas.gfx, TextRenderingHintAntiAliasGridFit);
            }

            let mut family_handle: Option<Family> = None;
            let mut fmt: Option<Fmt> = None;
            for item in items {
                let needs_text = matches!(item.kind, Kind::Text) || item.text.is_some();
                if needs_text && family_handle.is_none() {
                    let f = Family::load(family)?;
                    font_name = f.1.clone();
                    family_handle = Some(f);
                }
                if needs_text && fmt.is_none() {
                    fmt = Some(Fmt::new()?);
                }

                let mut entry = match item.kind {
                    Kind::Text => draw_text(
                        canvas.gfx,
                        family_handle.as_ref(),
                        fmt.as_ref(),
                        item,
                        "text",
                    )?,
                    Kind::Dimension => {
                        let mut line = draw_stroke(canvas.gfx, item)?;
                        // 没给 `text` 的尺寸线就只画线 —— 别为此去加载字体（也不该报字体错误）。
                        if item.text.is_some() {
                            if let Value::Object(map) = &mut line {
                                map.insert(
                                    "label".into(),
                                    draw_text(
                                        canvas.gfx,
                                        family_handle.as_ref(),
                                        fmt.as_ref(),
                                        item,
                                        "label",
                                    )?,
                                );
                            }
                        }
                        line
                    }
                    _ => draw_stroke(canvas.gfx, item)?,
                };
                if let Value::Object(map) = &mut entry {
                    map.insert("index".into(), json!(resolved.len()));
                }
                resolved.push(entry);
            }
        }
        // `canvas` 已 drop（GDI+ 的绘制全部落盘）⇒ 现在才能安全地改这块缓冲的通道顺序。
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.swap(0, 2); // BGRA -> RGBA；alpha 恒为 0xFF，不用动
        }
        let image =
            ::image::RgbaImage::from_raw(w, h, pixels).ok_or("像素缓冲尺寸与图片尺寸不一致")?;
        if let Some(parent) = out.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("{}: {e}", parent.display()))?;
            }
        }
        image
            .save_with_format(out, ::image::ImageFormat::Png)
            .map_err(|e| format!("{}: {e}", out.display()))?;

        Ok(json!({
            "input": src.display().to_string(),
            "output": out.display().to_string(),
            "format": "PNG",
            "width": w,
            "height": h,
            "font": font_name,
            "items": resolved,
        }))
    }

    fn load_bgra(src: &Path) -> Result<(u32, u32, Vec<u8>), String> {
        // `::image::` 前缀是必须的：本 crate 根上已经有一个 `mod image`（A8 图片附件）。
        let reader = ::image::ImageReader::open(src)
            .map_err(|e| format!("{}: {e}", src.display()))?
            .with_guessed_format()
            .map_err(|e| format!("{}: {e}", src.display()))?;
        let decoded = reader
            .decode()
            .map_err(|e| format!("不是能解码的图片（{}）：{e}", src.display()))?;
        let rgba = decoded.to_rgba8();
        let (w, h) = rgba.dimensions();
        if w == 0 || h == 0 {
            return Err("图片尺寸为 0".into());
        }
        let mut bgra = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for p in rgba.pixels() {
            // 一律置成不透明：GDI+ 的 alpha 混合会把「源图自带的透明像素」当成可穿透，
            // 存档 PNG 时反而变成黑块。
            bgra.extend_from_slice(&[p[2], p[1], p[0], 0xFF]);
        }
        Ok((w, h, bgra))
    }

    /// 越界坐标**直接拒**：报出真实尺寸，模型下一轮就能自己改对（见文件头）。
    fn check_bounds(item: &Item, index: usize, w: u32, h: u32) -> Result<(), String> {
        let (fw, fh) = (w as f32, h as f32);
        let mut points = vec![("at", item.at)];
        if item.kind != Kind::Text {
            points.push(("from", Some(item.from)));
            points.push(("to", Some(item.to)));
        }
        for (name, p) in points {
            let Some(p) = p else { continue };
            if p.0 < 0.0 || p.1 < 0.0 || p.0 > fw || p.1 > fh {
                return Err(format!(
                    "items[{index}].{name} ({:.1}, {:.1}) is outside the {w}x{h} image — \
                     measure the real edges with ImageMeasure first",
                    p.0, p.1
                ));
            }
        }
        Ok(())
    }

    // ── 图元 ───────────────────────────────────────────────────

    fn draw_stroke(gfx: *mut GpGraphics, item: &Item) -> Result<Value, String> {
        let p = pen(item.color, item.width)?;
        unsafe {
            check(
                GdipDrawLine(gfx, p.0, item.from.0, item.from.1, item.to.0, item.to.1),
                "画线",
            )?;
        }
        let mut heads = Vec::new();
        for (tip, other) in arrow_ends(item) {
            if let Some(tri) = arrow_head(tip, other, item.arrow_size, item.arrow_dir) {
                fill_triangle(gfx, item.color, tri)?;
                heads.push(json!({
                    "tip": point_json(tri[0]),
                    "base1": point_json(tri[1]),
                    "base2": point_json(tri[2]),
                }));
            }
        }
        Ok(json!({
            "type": kind_name(item.kind),
            "from": point_json(item.from),
            "to": point_json(item.to),
            "length": round1(distance(item.from, item.to)),
            "width": round1(item.width),
            "arrow_heads": heads,
        }))
    }

    fn draw_text(
        gfx: *mut GpGraphics,
        family: Option<&Family>,
        fmt: Option<&Fmt>,
        item: &Item,
        kind: &str,
    ) -> Result<Value, String> {
        let Some(family) = family else {
            return Err("内部错误：文字图元没有字体".into());
        };
        let Some(fmt) = fmt else {
            return Err("内部错误：文字图元没有文本格式".into());
        };
        let text = item.text.as_deref().unwrap_or("");
        let anchor = match item.kind {
            Kind::Text => item.at.unwrap_or(Pt(0.0, 0.0)),
            // 尺寸线标签：给了 `at` 就用它（精确落点），否则按法线偏移自动放。
            _ => item.at.unwrap_or_else(|| label_anchor(item)),
        };

        let mut raw: *mut GpFont = std::ptr::null_mut();
        unsafe {
            check(
                GdipCreateFont(family.ptr(), item.size, 0, UnitPixel, &mut raw),
                "建字体",
            )?;
        }
        let font = Font(raw);

        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let probe = RectF {
            X: 0.0,
            Y: 0.0,
            Width: 100_000.0,
            Height: 100_000.0,
        };
        let mut bbox = RectF::default();
        let (mut fitted, mut lines) = (0i32, 0i32);
        unsafe {
            check(
                GdipMeasureString(
                    gfx,
                    PCWSTR(wide.as_ptr()),
                    -1,
                    font.0,
                    &probe,
                    fmt.0,
                    &mut bbox,
                    &mut fitted,
                    &mut lines,
                ),
                "量文字宽高",
            )?;
        }
        let ink_w = bbox.Width.max(0.0);
        let ink_h = bbox.Height.max(0.0);
        // 锚点：水平按 `align` 对齐，**垂直恒为中点**（放标签时不用再猜基线）。
        let x = match item.align {
            Align::Left => anchor.0,
            Align::Center => anchor.0 - ink_w / 2.0,
            Align::Right => anchor.0 - ink_w,
        };
        let y = anchor.1 - ink_h / 2.0;

        if let Some(bg) = item.bg {
            let b = brush(bg)?;
            unsafe {
                check(
                    GdipFillRectangle(
                        gfx,
                        b.0 as *mut GpBrush,
                        x - item.pad,
                        y - item.pad,
                        ink_w + item.pad * 2.0,
                        ink_h + item.pad * 2.0,
                    ),
                    "画文字底色",
                )?;
            }
        }

        let b = brush(item.color)?;
        let layout = RectF {
            X: x - bbox.X,
            Y: y - bbox.Y,
            Width: ink_w + 8.0,
            Height: ink_h + 8.0,
        };
        unsafe {
            check(
                GdipDrawString(
                    gfx,
                    PCWSTR(wide.as_ptr()),
                    -1,
                    font.0,
                    &layout,
                    fmt.0,
                    b.0 as *const GpBrush,
                ),
                "画文字",
            )?;
        }

        Ok(json!({
            "type": kind,
            "text": text,
            "size": round1(item.size),
            "bbox": {
                "x": round1(x),
                "y": round1(y),
                "width": round1(ink_w),
                "height": round1(ink_h),
            },
        }))
    }

    fn fill_triangle(gfx: *mut GpGraphics, color: u32, tri: [Pt; 3]) -> Result<(), String> {
        let b = brush(color)?;
        let pts = [
            PointF {
                X: tri[0].0,
                Y: tri[0].1,
            },
            PointF {
                X: tri[1].0,
                Y: tri[1].1,
            },
            PointF {
                X: tri[2].0,
                Y: tri[2].1,
            },
        ];
        unsafe {
            check(
                GdipFillPolygon(gfx, b.0 as *mut GpBrush, pts.as_ptr(), 3, FillModeWinding),
                "画箭头",
            )
        }
    }

    // ── 几何 ───────────────────────────────────────────────────

    fn arrow_ends(item: &Item) -> Vec<(Pt, Pt)> {
        match item.arrows {
            Arrows::None => Vec::new(),
            Arrows::Start => vec![(item.from, item.to)],
            Arrows::End => vec![(item.to, item.from)],
            Arrows::Both => vec![(item.from, item.to), (item.to, item.from)],
        }
    }

    /// 箭头三角：尖端**恒在端点**，底边中心按 `dir` 落在线段内侧（`Out`）或外侧（`In`）。
    fn arrow_head(tip: Pt, other: Pt, size: f32, dir: ArrowDir) -> Option<[Pt; 3]> {
        let (dx, dy) = (tip.0 - other.0, tip.1 - other.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.01 {
            return None;
        }
        let (ux, uy) = (dx / len, dy / len); // 从 other 指向 tip（朝外）
        let base = match dir {
            ArrowDir::Out => Pt(tip.0 - ux * size, tip.1 - uy * size),
            ArrowDir::In => Pt(tip.0 + ux * size, tip.1 + uy * size),
        };
        let half = size * 0.36;
        let (px, py) = (-uy * half, ux * half);
        Some([
            tip,
            Pt(base.0 + px, base.1 + py),
            Pt(base.0 - px, base.1 - py),
        ])
    }

    /// 尺寸线标签的自动落点：线段中点 + 法线 `(-dy, dx)` 方向的 `label_offset`。
    /// 水平线（从左往右）的法线是 `(0, 1)`（向下）⇒ 默认的 -14 落在**线上方**。
    fn label_anchor(item: &Item) -> Pt {
        let mid = Pt(
            (item.from.0 + item.to.0) / 2.0,
            (item.from.1 + item.to.1) / 2.0,
        );
        let (dx, dy) = (item.to.0 - item.from.0, item.to.1 - item.from.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.001 {
            return mid;
        }
        let (nx, ny) = (-dy / len, dx / len);
        Pt(
            mid.0 + nx * item.label_offset,
            mid.1 + ny * item.label_offset,
        )
    }

    fn distance(a: Pt, b: Pt) -> f32 {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        (dx * dx + dy * dy).sqrt()
    }

    fn kind_name(kind: Kind) -> &'static str {
        match kind {
            Kind::Line => "line",
            Kind::Dimension => "dimension",
            Kind::Arrow => "arrow",
            Kind::Text => "text",
        }
    }

    fn point_json(p: Pt) -> Value {
        json!({ "x": round1(p.0), "y": round1(p.1) })
    }

    fn round1(v: f32) -> f64 {
        (f64::from(v) * 10.0).round() / 10.0
    }
}

// ── 测试 ─────────────────────────────────────────────────────────

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 并行安全的临时文件名（同 `image_measure` 的夹具口径）。
    fn temp_path(tag: &str) -> PathBuf {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "lunac-imageannotate-{}-{seq}-{tag}.png",
            std::process::id()
        ))
    }

    /// 200×100 的纯白底图。
    fn fixture(tag: &str) -> PathBuf {
        let path = temp_path(tag);
        let img = ::image::RgbaImage::from_pixel(200, 100, ::image::Rgba([255, 255, 255, 255]));
        img.save_with_format(&path, ::image::ImageFormat::Png)
            .expect("写夹具");
        path
    }

    fn spec(items: Value) -> Vec<Item> {
        parse_items(&json!({ "items": items })).expect("规格应能解析")
    }

    fn pixel(path: &Path, x: u32, y: u32) -> [u8; 4] {
        let img = ::image::open(path).expect("读输出图").to_rgba8();
        img.get_pixel(x, y).0
    }

    #[test]
    fn dimension_line_paints_between_its_endpoints() {
        let src = fixture("dim");
        let out = temp_path("dim-out");
        let items = spec(json!([
            { "type": "dimension", "from": { "x": 20, "y": 50 }, "to": { "x": 140, "y": 50 },
              "arrow": "both", "color": "#000000", "width": 2 }
        ]));
        let report = gdi::render(&src, &out, &items, None).expect("应画得出来");
        assert_eq!(report["font"], json!(""), "没画文字就不该去找字体");

        // 中点被画上（红色的默认色不生效，这里显式给了黑色）。
        let mid = pixel(&out, 80, 50);
        assert!(mid[0] < 128, "尺寸线中点应是暗的，实际 {mid:?}");

        // 关键断言：**同一张输出图拿 `ImageMeasure` 复算**，bbox 必须落回端点附近。
        let measured =
            crate::image_measure::measure_at(&out, &json!({ "threshold": 128 })).expect("复算");
        let bbox = &measured["foreground"]["bbox"];
        let (x, right) = (bbox["x"].as_u64().unwrap(), bbox["right"].as_u64().unwrap());
        let y = bbox["y"].as_u64().unwrap();
        // 端点 20..140 + 箭头（默认 Both）：左右各会超出约 14 px。
        assert!((5..=20).contains(&x), "左端 {x} 应在箭头外沿附近");
        assert!(
            (140..=155).contains(&right),
            "右端 {right} 应在箭头外沿附近"
        );
        assert!((44..=56).contains(&y), "纵向应落在 y=50 附近，实际 {y}");

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn later_items_paint_over_earlier_ones() {
        let src = fixture("order");
        let out = temp_path("order-out");
        let items = spec(json!([
            { "type": "line", "from": { "x": 40, "y": 30 }, "to": { "x": 160, "y": 30 },
              "color": "#FF0000", "width": 6 },
            { "type": "line", "from": { "x": 40, "y": 30 }, "to": { "x": 160, "y": 30 },
              "color": "#0000FF", "width": 6 }
        ]));
        gdi::render(&src, &out, &items, None).expect("应画得出来");
        let p = pixel(&out, 100, 30);
        assert!(p[2] > 200 && p[0] < 60, "后画的那条应为蓝色，实际 {p:?}");
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn chinese_text_renders_ink_and_reports_its_bbox() {
        let src = fixture("text");
        let out = temp_path("text-out");
        let items = spec(json!([
            { "type": "text", "at": { "x": 100, "y": 50 }, "text": "尺寸 12.5 cm",
              "size": 24, "color": "#000000", "align": "center" }
        ]));
        let report = gdi::render(&src, &out, &items, None).expect("应画得出来");
        assert_ne!(report["font"], json!(""), "画了中文就必须报出用的字体");

        let bbox = &report["items"][0]["bbox"];
        let width = bbox["width"].as_f64().unwrap();
        assert!(width > 40.0, "中文标签应当有实际宽度，实际 {width}");

        // 文字区域内必须有暗像素（字体缺失时 GDI+ 会画方框或什么都不画）。
        let x = bbox["x"].as_f64().unwrap() as u32;
        let y = bbox["y"].as_f64().unwrap() as u32;
        let w = bbox["width"].as_f64().unwrap() as u32;
        let h = bbox["height"].as_f64().unwrap() as u32;
        let img = ::image::open(&out).expect("读输出图").to_rgba8();
        let ink = (x..x + w)
            .flat_map(|px| (y..y + h).map(move |py| (px, py)))
            .filter(|(px, py)| img.get_pixel(*px, *py).0[0] < 128)
            .count();
        assert!(ink > 30, "文字区域应有笔画像素，实际只有 {ink}");

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn output_is_a_png_of_the_source_size() {
        let src = fixture("png");
        let out = temp_path("png-out");
        let items = spec(json!([
            { "type": "arrow", "from": { "x": 10, "y": 10 }, "to": { "x": 60, "y": 40 } }
        ]));
        let report = gdi::render(&src, &out, &items, None).expect("应画得出来");
        assert_eq!(report["width"], json!(200));
        assert_eq!(report["height"], json!(100));

        let bytes = std::fs::read(&out).expect("读输出");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "必须是真的 PNG");
        assert_eq!(
            report["items"][0]["arrow_heads"].as_array().unwrap().len(),
            1
        );

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn coordinates_outside_the_image_are_rejected_with_its_real_size() {
        let src = fixture("bounds");
        let out = temp_path("bounds-out");
        let items = spec(json!([
            { "type": "line", "from": { "x": 10, "y": 10 }, "to": { "x": 900, "y": 10 } }
        ]));
        let err = gdi::render(&src, &out, &items, None).expect_err("越界必须拒");
        assert!(err.contains("200x100"), "错误里要带真实尺寸，实际：{err}");
        let _ = std::fs::remove_file(&src);
    }

    #[test]
    fn spec_parsing_is_strict_about_typos() {
        let err = parse_items(&json!({ "items": [
            { "type": "dimenison", "from": { "x": 1, "y": 1 }, "to": { "x": 2, "y": 2 } }
        ]}))
        .expect_err("未知 type 必须拒");
        assert!(err.contains("dimenison"));

        let err = parse_items(&json!({ "items": [
            { "type": "line", "from": { "x": 1, "y": 1 }, "to": { "x": 2, "y": 2 }, "colour": "#fff" }
        ]}))
        .expect_err("未知键必须拒");
        assert!(err.contains("colour"));

        let err = parse_items(&json!({ "items": [] })).expect_err("空 items 必须拒");
        assert!(err.contains("empty"));

        let err = parse_items(&json!({ "items": [
            { "type": "text", "text": "hi" }
        ]}))
        .expect_err("text 缺 at 必须拒");
        assert!(err.contains("at"));
    }

    #[test]
    fn colors_follow_css_ordering() {
        assert_eq!(parse_hex("#FF3B30").unwrap(), 0xFFFF_3B30);
        assert_eq!(parse_hex("#F00").unwrap(), 0xFFFF_0000);
        // CSS 的 alpha 在**最后**：`#00000080` = 半透明黑。
        assert_eq!(parse_hex("#00000080").unwrap(), 0x8000_0000);
        assert!(parse_hex("#12345").is_err());
        assert!(parse_hex("red").is_err());
    }
}
