// ── ImageDiff（§待办 3-28.5 第二步，2026-11-16）─────────────────────
//
// 视觉回归 = **固定镜头 + golden 图 + diff**，不靠肉眼（docs/todo.md §待办 3-28.5）。
// 这一件负责最后一步：读两张本地图片，逐像素比较，给出可复算的数字 ——
//   · 不同像素数 / 占比（`different / total * 100`）
//   · 差异包围盒（第一个与最后一个差异像素围出来的矩形）
//   · 平均绝对差（0–255，所有通道）
//
// 边界：
//   · **绝不缩放对齐**：尺寸不同就直接报错并给出两边尺寸 —— 缩放会把「整体偏移」
//     洗成「几乎一样」，那是最容易骗过自己的失败模式；
//   · 只读：两张图都走 `tools::resolve_read_path`（与 `Read` 同一套工作区锁 / 禁读目录闸）；
//   · 不落盘（要看差异图用 `ImageAnnotate` 自己画，本工具只给数字）。

use serde_json::{json, Value};
use std::path::Path;

use crate::tools::Ctx;

/// 最多回报多少个差异采样点（够定位「从哪开始不一样」，又不至于把结果撑爆）。
const MAX_SAMPLE_POINTS: usize = 32;

pub fn run(ctx: &Ctx, input: &Value) -> Result<String, String> {
    let golden = require_path(input, "golden")?;
    let actual = require_path(input, "actual")?;
    let golden = crate::tools::resolve_read_path(ctx, golden)?;
    let actual = crate::tools::resolve_read_path(ctx, actual)?;
    let threshold = input
        .get("threshold")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(255) as u8;
    let report = diff_at(&golden, &actual, threshold)?;
    serde_json::to_string_pretty(&report).map_err(|e| format!("结果序列化失败：{e}"))
}

fn require_path<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("{key} is required (path to the image)"))
}

/// 纯比较入口（不碰 `Ctx`）：`threshold` = 单通道差值超过多少才算「不同」。
pub(crate) fn diff_at(golden: &Path, actual: &Path, threshold: u8) -> Result<Value, String> {
    let a = decode(golden)?;
    let b = decode(actual)?;
    if a.dimensions() != b.dimensions() {
        return Err(format!(
            "image sizes differ: golden {}x{}, actual {}x{} (ImageDiff never scales)",
            a.width(),
            a.height(),
            b.width(),
            b.height()
        ));
    }
    let (width, height) = a.dimensions();
    let total = width as u64 * height as u64;
    let mut different: u64 = 0;
    let mut sum_abs: u64 = 0;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut samples: Vec<Value> = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let pa = a.get_pixel(x, y).0;
            let pb = b.get_pixel(x, y).0;
            let mut delta: u8 = 0;
            for channel in 0..4 {
                let d = pa[channel].abs_diff(pb[channel]);
                delta = delta.max(d);
            }
            sum_abs += delta as u64;
            if delta > threshold {
                different += 1;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
                if samples.len() < MAX_SAMPLE_POINTS {
                    samples.push(json!({
                        "x": x,
                        "y": y,
                        "delta": delta,
                        "rgb": [pb[0], pb[1], pb[2]],
                    }));
                }
            }
        }
    }
    let percent = if total == 0 {
        0.0
    } else {
        (different as f64) * 100.0 / (total as f64)
    };
    let mean_abs = if total == 0 {
        0.0
    } else {
        sum_abs as f64 / (total as f64)
    };
    let bbox = if different == 0 {
        Value::Null
    } else {
        json!({
            "x": min_x,
            "y": min_y,
            "width": max_x - min_x + 1,
            "height": max_y - min_y + 1,
        })
    };
    Ok(json!({
        "width": width,
        "height": height,
        "threshold": threshold,
        "totalPixels": total,
        "differentPixels": different,
        "percent": (percent * 10000.0).round() / 10000.0,
        "meanAbsDiff": (mean_abs * 10000.0).round() / 10000.0,
        "bbox": bbox,
        "samples": samples,
    }))
}

fn decode(path: &Path) -> Result<::image::RgbaImage, String> {
    // `::image::` 前面的冒号是必须的：本 crate 根上已有一个 `mod image`（A8 图片附件）。
    let reader = ::image::ImageReader::open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .with_guessed_format()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    reader
        .decode()
        .map(|img| img.to_rgba8())
        .map_err(|e| format!("不是能解码的图片（{}）：{e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::image::Rgba;

    fn temp_png(name: &str, img: &::image::RgbaImage) -> std::path::PathBuf {
        use ::image::ImageEncoder as _;
        let path = std::env::temp_dir().join(format!(
            "lunac-diff-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mut png = Vec::new();
        ::image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                ::image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        std::fs::write(&path, png).unwrap();
        path
    }

    #[test]
    fn identical_images_are_zero_percent_without_a_bbox() {
        let img = ::image::RgbaImage::from_pixel(8, 6, Rgba([10, 20, 30, 255]));
        let a = temp_png("same-a.png", &img);
        let b = temp_png("same-b.png", &img);
        let report = diff_at(&a, &b, 0).unwrap();
        assert_eq!(report["differentPixels"], 0);
        assert_eq!(report["percent"], 0.0);
        assert_eq!(report["totalPixels"], 48);
        assert_eq!(report["bbox"], Value::Null);
        assert!(report["samples"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn one_changed_pixel_gives_a_one_by_one_bbox_and_a_sample() {
        let a_img = ::image::RgbaImage::from_pixel(5, 4, Rgba([0, 0, 0, 255]));
        let mut b_img = a_img.clone();
        b_img.put_pixel(3, 2, Rgba([255, 255, 255, 255]));
        let a = temp_png("one-a.png", &a_img);
        let b = temp_png("one-b.png", &b_img);
        let report = diff_at(&a, &b, 0).unwrap();
        assert_eq!(report["differentPixels"], 1);
        assert_eq!(report["percent"], 5.0);
        assert_eq!(report["bbox"]["x"], 3);
        assert_eq!(report["bbox"]["y"], 2);
        assert_eq!(report["bbox"]["width"], 1);
        assert_eq!(report["bbox"]["height"], 1);
        assert_eq!(report["samples"][0]["x"], 3);
        assert_eq!(report["samples"][0]["delta"], 255);
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn threshold_ignores_small_differences_but_keeps_the_mean() {
        let a_img = ::image::RgbaImage::from_pixel(4, 4, Rgba([100, 100, 100, 255]));
        let mut b_img = a_img.clone();
        for x in 0..4 {
            b_img.put_pixel(x, 0, Rgba([105, 105, 105, 255]));
        }
        let a = temp_png("thr-a.png", &a_img);
        let b = temp_png("thr-b.png", &b_img);
        let low = diff_at(&a, &b, 0).unwrap();
        assert_eq!(low["differentPixels"], 4);
        assert!((low["meanAbsDiff"].as_f64().unwrap() - 1.25).abs() < 1e-6);
        let high = diff_at(&a, &b, 8).unwrap();
        assert_eq!(high["differentPixels"], 0);
        assert!((high["meanAbsDiff"].as_f64().unwrap() - 1.25).abs() < 1e-6);
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn different_sizes_are_rejected_instead_of_scaled() {
        let a = temp_png("size-a.png", &::image::RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 4])));
        let b = temp_png("size-b.png", &::image::RgbaImage::from_pixel(4, 5, Rgba([1, 2, 3, 4])));
        let err = diff_at(&a, &b, 0).err().expect("尺寸不同必须报错");
        assert!(err.contains("4x4"), "{err}");
        assert!(err.contains("4x5"), "{err}");
        assert!(err.contains("never scales"), "{err}");
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn missing_paths_are_reported_in_plain_words() {
        let input = json!({"golden": "a.png"});
        assert!(require_path(&input, "actual").unwrap_err().contains("actual"));
        let missing = decode(Path::new("definitely-missing-lunac-image.png"))
            .err()
            .expect("不存在的文件必须报错");
        assert!(missing.contains("definitely-missing"), "{missing}");
    }
}
