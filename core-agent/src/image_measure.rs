// ── ImageMeasure（§待办 3-19 第一步，2026-10-30）─────────────────────
//
// 为什么要有它：模型（视觉模型也一样）**报绝对像素坐标不可靠**。2026-10-27 / 29 那次
// 「四杯照片标尺寸」的会话里，模型只能现写 PowerShell + System.Drawing 采样脚本去猜，
// 12 轮都在修同一张图。这一层把「量」变成确定性调用：输入探针，输出整数坐标与可复算的口径。
//
// 边界：**只测量、不绘制**（绘制 = `ImageAnnotate`，§待办 3-19 第二步）。
// 只读：路径走 `tools::resolve_read_path` —— 与 `Read` 同一套工作区锁 / 禁读目录闸，
// 不新造第二套路径归一，否则「读禁目录」这类闸会在新工具上漏掉。

use serde_json::{json, Value};
use std::path::Path;

use crate::tools::Ctx;

/// 一次调用最多接受多少条行 / 列探针（防止把整张图的剖面全要回来）。
const MAX_SCANS: usize = 32;
/// ASCII 缩略图的列数上限（`ascii_columns` 只在显式给了才出图，所以没有默认值）。
const MAX_ASCII_COLUMNS: u32 = 200;
/// 前景判据的默认亮度阈值（0–255，`luma < threshold` 算前景）。
const DEFAULT_THRESHOLD: u8 = 128;
/// ASCII 灰阶：从「亮」到「暗」。
const ASCII_RAMP: &[u8] = b" .:-=+*#%@";

pub fn run(ctx: &Ctx, input: &Value) -> Result<String, String> {
    let raw = input
        .get("file_path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if raw.is_empty() {
        return Err("file_path is required".into());
    }
    let path = crate::tools::resolve_read_path(ctx, raw)?;
    let report = measure_at(&path, input)?;
    serde_json::to_string_pretty(&report).map_err(|e| format!("结果序列化失败：{e}"))
}

/// 纯测量入口（**不碰 `Ctx`**）：路径校验在 [`run`] 里做完，这里只处理像素。
///
/// `pub(crate)`：`ImageAnnotate` 的测试拿它**复算**自己画出来的结果（§待办 3-19 验收判据 2）。
pub(crate) fn measure_at(path: &Path, input: &Value) -> Result<Value, String> {
    // `::image::` 前面那个冒号是**必须的**：本 crate 根上已经有一个 `mod image`（A8 图片附件），
    // 不加会被解析成那个模块而不是外部 crate。
    let reader = ::image::ImageReader::open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .with_guessed_format()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let format = reader
        .format()
        .map(|f| format!("{f:?}"))
        .unwrap_or_else(|| "unknown".to_string());
    let decoded = reader
        .decode()
        .map_err(|e| format!("不是能解码的图片（{}）：{e}", path.display()))?;
    let luma = decoded.to_luma8();
    let (width, height) = luma.dimensions();
    if width == 0 || height == 0 {
        return Err("图片尺寸为 0".into());
    }

    let threshold = input
        .get("threshold")
        .and_then(Value::as_u64)
        .map(|value| value.clamp(1, 254) as u8)
        .unwrap_or(DEFAULT_THRESHOLD);
    let (x0, y0, x1, y1) = region_of(input, width, height)?;

    // ── 前景统计：一遍扫完，同时得到像素数与 bbox ──────────────────
    let mut pixels: u64 = 0;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for y in y0..y1 {
        for x in x0..x1 {
            if luma.get_pixel(x, y)[0] < threshold {
                pixels += 1;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    let area = u64::from(x1 - x0) * u64::from(y1 - y0);
    let ratio = if area == 0 {
        0.0
    } else {
        (pixels as f64 / area as f64 * 10_000.0).round() / 10_000.0
    };
    // `right` / `bottom` 是**闭区间端点**，与 `width` / `height` 一起给：
    // 模型要做「右端离绳 4px」这类判断时不用再自己加一，少一次算错的机会。
    let bbox = (pixels > 0).then(|| {
        json!({
            "x": min_x,
            "y": min_y,
            "width": max_x - min_x + 1,
            "height": max_y - min_y + 1,
            "right": max_x,
            "bottom": max_y,
        })
    });

    let mut report = serde_json::Map::new();
    report.insert("path".into(), json!(path.display().to_string()));
    report.insert("format".into(), json!(format));
    report.insert("width".into(), json!(width));
    report.insert("height".into(), json!(height));
    report.insert(
        "region".into(),
        json!({ "x": x0, "y": y0, "width": x1 - x0, "height": y1 - y0 }),
    );
    report.insert("threshold".into(), json!(threshold));
    report.insert(
        "foreground".into(),
        json!({
            "rule": format!("luma < {threshold}"),
            "pixels": pixels,
            "ratio": ratio,
            "bbox": bbox,
        }),
    );

    // ── 行 / 列探针：回答「这一行（列）上暗区从哪到哪」────────────────
    if let Some(rows) = input.get("scan_rows").and_then(Value::as_array) {
        let out: Vec<Value> = rows
            .iter()
            .filter_map(Value::as_u64)
            .take(MAX_SCANS)
            .filter_map(|row| u32::try_from(row).ok())
            .filter(|y| (y0..y1).contains(y))
            .map(|y| {
                let span = span_on_row(&luma, y, x0, x1, threshold);
                json!({ "y": y, "first": span.map(|s| s.0), "last": span.map(|s| s.1) })
            })
            .collect();
        if !out.is_empty() {
            report.insert("scan_rows".into(), Value::Array(out));
        }
    }
    if let Some(cols) = input.get("scan_cols").and_then(Value::as_array) {
        let out: Vec<Value> = cols
            .iter()
            .filter_map(Value::as_u64)
            .take(MAX_SCANS)
            .filter_map(|col| u32::try_from(col).ok())
            .filter(|x| (x0..x1).contains(x))
            .map(|x| {
                let span = span_on_col(&luma, x, y0, y1, threshold);
                json!({ "x": x, "first": span.map(|s| s.0), "last": span.map(|s| s.1) })
            })
            .collect();
        if !out.is_empty() {
            report.insert("scan_cols".into(), Value::Array(out));
        }
    }

    // ── ASCII 缩略图：给「大致在哪」一个便宜的全局视图 ────────────────
    if let Some(columns) = input.get("ascii_columns").and_then(Value::as_u64) {
        let columns = (columns as u32).clamp(8, MAX_ASCII_COLUMNS).min(x1 - x0);
        let rows = ((u64::from(y1 - y0) * u64::from(columns)) / (u64::from(x1 - x0) * 2))
            .clamp(1, 200) as u32;
        report.insert(
            "ascii".into(),
            json!({
                "columns": columns,
                "rows": rows,
                "lines": build_ascii(&luma, (x0, y0, x1, y1), columns, rows),
            }),
        );
    }

    Ok(Value::Object(report))
}

/// 解析 `region` 并把越界部分裁掉；返回**半开区间** `(x0, y0, x1, y1)`。
fn region_of(input: &Value, width: u32, height: u32) -> Result<(u32, u32, u32, u32), String> {
    let Some(region) = input.get("region").filter(|value| value.is_object()) else {
        return Ok((0, 0, width, height));
    };
    let get = |key: &str| region.get(key).and_then(Value::as_u64);
    let x = get("x").unwrap_or(0);
    let y = get("y").unwrap_or(0);
    let w = get("width").unwrap_or(u64::from(width));
    let h = get("height").unwrap_or(u64::from(height));
    let x0 = x.min(u64::from(width)) as u32;
    let y0 = y.min(u64::from(height)) as u32;
    let x1 = x.saturating_add(w).min(u64::from(width)) as u32;
    let y1 = y.saturating_add(h).min(u64::from(height)) as u32;
    if x1 <= x0 || y1 <= y0 {
        return Err(format!(
            "region 与图片没有交集（图片 {width}×{height}，region x={x} y={y} w={w} h={h}）"
        ));
    }
    Ok((x0, y0, x1, y1))
}

fn span_on_row(
    img: &::image::GrayImage,
    y: u32,
    x0: u32,
    x1: u32,
    threshold: u8,
) -> Option<(u32, u32)> {
    let mut first = None;
    let mut last = None;
    for x in x0..x1 {
        if img.get_pixel(x, y)[0] < threshold {
            first.get_or_insert(x);
            last = Some(x);
        }
    }
    first.zip(last)
}

fn span_on_col(
    img: &::image::GrayImage,
    x: u32,
    y0: u32,
    y1: u32,
    threshold: u8,
) -> Option<(u32, u32)> {
    let mut first = None;
    let mut last = None;
    for y in y0..y1 {
        if img.get_pixel(x, y)[0] < threshold {
            first.get_or_insert(y);
            last = Some(y);
        }
    }
    first.zip(last)
}

/// 每个格子取**最暗**的那个像素：缩略图要能保住细线（取均值会把 1px 的线抹平）。
fn build_ascii(
    img: &::image::GrayImage,
    region: (u32, u32, u32, u32),
    columns: u32,
    rows: u32,
) -> Vec<String> {
    let (x0, y0, x1, y1) = region;
    let cell_w = ((x1 - x0) as f64 / columns as f64).max(1.0);
    let cell_h = ((y1 - y0) as f64 / rows as f64).max(1.0);
    let mut lines = Vec::with_capacity(rows as usize);
    for row in 0..rows {
        let top = y0 + (row as f64 * cell_h) as u32;
        let bottom = (y0 + ((row + 1) as f64 * cell_h) as u32).min(y1).max(top + 1);
        let mut line = String::with_capacity(columns as usize);
        for col in 0..columns {
            let left = x0 + (col as f64 * cell_w) as u32;
            let right = (x0 + ((col + 1) as f64 * cell_w) as u32).min(x1).max(left + 1);
            let mut darkest = 255u8;
            for y in top..bottom {
                for x in left..right {
                    darkest = darkest.min(img.get_pixel(x, y)[0]);
                }
            }
            let index = ((u16::from(255 - darkest) * (ASCII_RAMP.len() as u16 - 1)) / 255) as usize;
            line.push(ASCII_RAMP[index] as char);
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::image::{ImageBuffer, Luma};

    /// 100×60 白底 + (10,20) 起 30×10 的黑块：bbox 与行/列剖面都能对表。
    ///
    /// 文件名带**自增序号**：测试是并行跑的，共用同一个临时文件会互相删掉对方那张
    /// （第一版就是这么挂的：一条测试把图删了，另一条正在读）。
    fn fixture() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let mut img: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::from_pixel(100, 60, Luma([255]));
        for y in 20..30 {
            for x in 10..40 {
                img.put_pixel(x, y, Luma([0]));
            }
        }
        let path = std::env::temp_dir().join(format!(
            "lunac-imagemeasure-{}-{seq}.png",
            std::process::id()
        ));
        img.save(&path).expect("写测试图");
        path
    }

    #[test]
    fn bbox_and_ratio_are_deterministic() {
        let path = fixture();
        let report = measure_at(&path, &json!({})).expect("测量失败");
        assert_eq!(report["width"], json!(100));
        assert_eq!(report["height"], json!(60));
        assert_eq!(report["foreground"]["pixels"], json!(300));
        assert_eq!(report["foreground"]["bbox"]["x"], json!(10));
        assert_eq!(report["foreground"]["bbox"]["y"], json!(20));
        assert_eq!(report["foreground"]["bbox"]["width"], json!(30));
        assert_eq!(report["foreground"]["bbox"]["height"], json!(10));
        assert_eq!(report["foreground"]["bbox"]["right"], json!(39));
        assert_eq!(report["foreground"]["bbox"]["bottom"], json!(29));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn scans_report_first_and_last_foreground_pixel() {
        let path = fixture();
        let report = measure_at(&path, &json!({ "scan_rows": [20, 5], "scan_cols": [10] }))
            .expect("测量失败");
        let rows = report["scan_rows"].as_array().expect("scan_rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["first"], json!(10));
        assert_eq!(rows[0]["last"], json!(39));
        // 空白行两端都是 null（**不能写 0**：0 会被读成「贴左边缘」）。
        assert!(rows[1]["first"].is_null());
        let cols = report["scan_cols"].as_array().expect("scan_cols");
        assert_eq!(cols[0]["first"], json!(20));
        assert_eq!(cols[0]["last"], json!(29));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn region_clips_the_measurement() {
        let path = fixture();
        let report = measure_at(&path, &json!({ "region": { "x": 0, "y": 0, "width": 20, "height": 60 } }))
            .expect("测量失败");
        // 只能看到 x∈[0,20) 的那一段黑块：宽 10（10..19）
        assert_eq!(report["foreground"]["bbox"]["x"], json!(10));
        assert_eq!(report["foreground"]["bbox"]["width"], json!(10));
        assert_eq!(report["foreground"]["pixels"], json!(100));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ascii_is_bounded_and_keeps_row_width() {
        let path = fixture();
        let report = measure_at(&path, &json!({ "ascii_columns": 20 })).expect("测量失败");
        let lines = report["ascii"]["lines"].as_array().expect("ascii");
        assert!(!lines.is_empty());
        for line in lines {
            assert_eq!(line.as_str().unwrap().chars().count(), 20);
        }
        let _ = std::fs::remove_file(&path);
    }
}
