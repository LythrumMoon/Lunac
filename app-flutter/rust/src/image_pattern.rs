//! 内置工具 `image_pattern_analysis`：本地图片图案特征分析。
//!
//! **按 v1 `app/src-tauri/src/mcp_server.rs` 的 `analyze_image_pattern` 原样移植**
//! （纯像素统计，不做语义理解）：解码图片 → 缩略到 160px → 统计亮度网格 / 色相分布 /
//! 主色 top5 / Sobel 边缘方向 / 3×3 局部纹理 / 左右对称度 → 输出结构化中文报告。
//!
//! 为什么放在宿主（`lunac.exe`）而不是 agent：它是 `tools/*.json` 里 `builtin` handler 的
//! 落点 —— 用户工具定义只写 `"handler": {"type":"builtin","name":"image_pattern_analysis"}`，
//! 真正执行在宿主进程内完成（完全离线，不依赖外部命令或网络）。

use std::collections::HashMap;

/// 解码并分析一张图片；`path` 是绝对路径（jpg / png 等由 `image` 的解码器决定）。
pub fn analyze(path: &str) -> Result<String, String> {
    let image = image::open(path).map_err(|e| format!("无法解码图片 {path}（{e}）"))?;
    let (orig_w, orig_h) = (image.width(), image.height());
    // 缩略到 160 宽以内，控制计算量。
    let thumb = image.thumbnail(160, 160);
    let rgb = thumb.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    if w == 0 || h == 0 {
        return Err("图片尺寸无效".to_string());
    }

    let px = |x: usize, y: usize| -> (f32, f32, f32) {
        let pixel = rgb.get_pixel(x as u32, y as u32);
        (
            pixel[0] as f32 / 255.0,
            pixel[1] as f32 / 255.0,
            pixel[2] as f32 / 255.0,
        )
    };
    let lum = |x: usize, y: usize| -> f32 {
        let (r, g, b) = px(x, y);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    };

    let n = (w * h) as f32;

    // ① 亮度：整体均值 + 4×4 网格 + 高光点（>0.85）。
    const GRID: usize = 4;
    let (gw, gh) = (w / GRID, h / GRID);
    let cells_px = (gw.max(1) * gh.max(1)) as f32;
    let mut grid_lum = [[0f32; GRID]; GRID];
    let mut total_lum = 0f32;
    let mut high_light = 0usize;
    for y in 0..h {
        for x in 0..w {
            let value = lum(x, y);
            total_lum += value;
            if value > 0.85 {
                high_light += 1;
            }
            if gw > 0 && gh > 0 {
                grid_lum[(y / gh).min(GRID - 1)][(x / gw).min(GRID - 1)] += value;
            }
        }
    }
    let avg_lum = total_lum / n;
    let grid_str: Vec<String> = grid_lum
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| format!("{:>3.0}", (value / cells_px) * 100.0))
                .collect::<Vec<_>>()
                .join("  ")
        })
        .collect();

    // ② 色相分布（12 桶，S>0.15 计入）+ 饱和度均值 + 主色 top5。
    let mut hue_hist = [0usize; 12];
    let mut sat_sum = 0f32;
    let mut color_buckets: HashMap<u16, usize> = HashMap::new();
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = px(x, y);
            let (hue, saturation, _value) = rgb_to_hsv(r, g, b);
            sat_sum += saturation;
            if saturation > 0.15 {
                hue_hist[((hue / 30.0) as usize).min(11)] += 1;
            }
            let key = (((r * 4.0) as u16) & 0xF)
                | (((g * 4.0) as u16) & 0xF) << 4
                | (((b * 4.0) as u16) & 0xF) << 8;
            *color_buckets.entry(key).or_insert(0) += 1;
        }
    }
    let avg_sat = sat_sum / n;
    let mut ranked: Vec<(u16, usize)> = color_buckets.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    let main_colors: Vec<String> = ranked
        .iter()
        .take(5)
        .map(|(key, count)| {
            let r = ((key & 0xF) * 17) as f32 / 255.0;
            let g = (((key >> 4) & 0xF) * 17) as f32 / 255.0;
            let b = (((key >> 8) & 0xF) * 17) as f32 / 255.0;
            format!(
                "{} {:.0}%",
                color_name(r, g, b),
                (*count as f32 / n) * 100.0
            )
        })
        .collect();

    // ③ 边缘方向直方图（Sobel，|角度| 分 4 桶）+ 纹理密度（3×3 局部标准差）。
    let mut edge_hist = [0usize; 4];
    let mut edge_count = 0usize;
    let mut texture_sum = 0f32;
    let mut tex_cnt = 0usize;
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let mut vals = [0f32; 9];
            let mut mean = 0f32;
            for dy in 0..3 {
                for dx in 0..3 {
                    let value = lum(x + dx - 1, y + dy - 1);
                    vals[dy * 3 + dx] = value;
                    mean += value;
                }
            }
            mean /= 9.0;
            let mut var = 0f32;
            for value in &vals {
                var += (value - mean) * (value - mean);
            }
            texture_sum += (var / 9.0).sqrt();
            tex_cnt += 1;

            let gx = (lum(x + 1, y - 1) + 2.0 * lum(x + 1, y) + lum(x + 1, y + 1))
                - (lum(x - 1, y - 1) + 2.0 * lum(x - 1, y) + lum(x - 1, y + 1));
            let gy = (lum(x - 1, y + 1) + 2.0 * lum(x, y + 1) + lum(x + 1, y + 1))
                - (lum(x - 1, y - 1) + 2.0 * lum(x, y - 1) + lum(x + 1, y - 1));
            if gx * gx + gy * gy > 0.003 {
                edge_count += 1;
                let angle = gy.atan2(gx).to_degrees().abs();
                let dir = if angle <= 22.5 {
                    0
                } else if angle <= 67.5 {
                    1
                } else if angle <= 112.5 {
                    2
                } else if angle <= 157.5 {
                    3
                } else {
                    0
                };
                edge_hist[dir] += 1;
            }
        }
    }
    let edge_ratio = edge_count as f32 / tex_cnt.max(1) as f32;
    let texture = texture_sum / tex_cnt.max(1) as f32;

    // ④ 水平对称度（左右镜像亮度差）。
    let mut sym_diff = 0f32;
    let mut sym_cnt = 0usize;
    for y in 0..h {
        for x in 0..w / 2 {
            sym_diff += (lum(x, y) - lum(w - 1 - x, y)).abs();
            sym_cnt += 1;
        }
    }
    let symmetry = 1.0 - (sym_diff / sym_cnt.max(1) as f32);

    let bright_desc = match avg_lum {
        value if value > 0.75 => "高亮",
        value if value > 0.5 => "明亮",
        value if value > 0.3 => "中等",
        _ => "暗调",
    };
    let texture_desc = if texture > 0.22 {
        "高纹理（颗粒/笔触感强）"
    } else if texture > 0.12 {
        "中纹理（细腻）"
    } else {
        "低纹理（平滑）"
    };
    let symmetry_desc = if symmetry > 0.9 {
        "高度对称"
    } else if symmetry > 0.78 {
        "近似对称"
    } else {
        "非对称"
    };

    // Rust 的 format! 不支持 `%` 格式类型 ⇒ 百分比统一手动乘 100 后格式化。
    let edge_pct = format!("{:.0}", edge_ratio * 100.0);
    let high_pct = format!("{:.1}", high_light as f32 / n * 100.0);
    let sym_pct = format!("{:.0}", symmetry * 100.0);

    Ok(format!(
        "图片尺寸: {orig_w}×{orig_h}px\n\
         整体基调: {bright_desc}，{hue_desc}\n\
         主色调: {main_colors}\n\
         平均饱和度: {avg_sat:.2}（0 灰阶 ~ 1 纯彩）\n\
         亮度分布（4×4 网格，左上→右下，0-100）:\n\
         \x20 {g0}\n\
         \x20 {g1}\n\
         \x20 {g2}\n\
         \x20 {g3}\n\
         笔触/边缘方向: {edge_desc}（边缘占比 {edge_pct}%）\n\
         纹理: {texture_desc}\n\
         高光点占比: {high_pct}%（细密高光点≈点彩/星芒闪光）\n\
         对称性: {symmetry_desc}（{sym_pct}%）\n\
         构图建议: {comp_notes}",
        orig_w = orig_w,
        orig_h = orig_h,
        bright_desc = bright_desc,
        hue_desc = dominant_hue_desc(&hue_hist, n),
        main_colors = main_colors.join("、"),
        avg_sat = avg_sat,
        g0 = grid_str[0],
        g1 = grid_str[1],
        g2 = grid_str[2],
        g3 = grid_str[3],
        edge_desc = edge_summary(&edge_hist),
        edge_pct = edge_pct,
        texture_desc = texture_desc,
        high_pct = high_pct,
        symmetry_desc = symmetry_desc,
        comp_notes = composition_notes(avg_lum, symmetry, edge_ratio),
    ))
}

/// RGB(0..1) → HSV(h:0..360, s:0..1, v:0..1)。
fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let h = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { delta / max };
    (h, s, max)
}

fn color_name(r: f32, g: f32, b: f32) -> &'static str {
    let (h, s, v) = rgb_to_hsv(r, g, b);
    if v < 0.15 {
        return "黑";
    }
    if s < 0.15 {
        return if v > 0.9 {
            "白"
        } else if v > 0.6 {
            "浅灰"
        } else {
            "深灰"
        };
    }
    const NAMES: [&str; 12] = [
        "红", "橙", "黄", "黄绿", "绿", "青绿", "青", "蓝", "蓝紫", "紫", "品红", "粉",
    ];
    NAMES[((h / 30.0) as usize).min(11)]
}

fn dominant_hue_desc(hue_hist: &[usize; 12], n: f32) -> String {
    const NAMES: [&str; 12] = [
        "红调", "橙调", "黄调", "黄绿调", "绿调", "青绿调", "青调", "蓝调", "蓝紫调", "紫调",
        "品红调", "粉调",
    ];
    let total: usize = hue_hist.iter().sum();
    if total == 0 {
        return "无明显彩色倾向（近灰阶）".to_string();
    }
    let mut best = 0usize;
    for index in 1..12 {
        if hue_hist[index] > hue_hist[best] {
            best = index;
        }
    }
    format!(
        "{}（彩色占比 {:.0}%）",
        NAMES[best],
        total as f32 / n * 100.0
    )
}

fn edge_summary(hist: &[usize; 4]) -> String {
    const NAMES: [&str; 4] = ["水平", "右斜", "垂直", "左斜"];
    let total: usize = hist.iter().sum();
    if total == 0 {
        return "无明显边缘".to_string();
    }
    let mut order: Vec<usize> = (0..4).collect();
    order.sort_by(|a, b| hist[*b].cmp(&hist[*a]));
    format!("{}为主，其次{}", NAMES[order[0]], NAMES[order[1]])
}

fn composition_notes(avg_lum: f32, symmetry: f32, edge_ratio: f32) -> String {
    let mut notes = Vec::new();
    if symmetry > 0.9 {
        notes.push("正面/对称构图，适合圣像式、庄重主题");
    }
    if edge_ratio > 0.3 {
        notes.push("边缘密集，笔触/纹理丰富，适合做肌理感背景");
    }
    if avg_lum > 0.7 {
        notes.push("整体高亮，主体宜靠近左上光源或居中加光环");
    }
    if avg_lum < 0.35 {
        notes.push("暗调为主，建议局部高光点缀（暗底浮金效果）");
    }
    if notes.is_empty() {
        notes.push("常规构图，可局部提亮或强化对比以增强表现力");
    }
    notes.join("；")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_png(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "lunac-image-pattern-{tag}-{}.png",
            std::process::id()
        ));
        // 左半白右半黑（非对称）⇒ 对称度 < 1、主色只有黑白两档。
        let image = image::RgbImage::from_fn(40, 40, |x, _y| {
            if x < 20 {
                image::Rgb([255, 255, 255])
            } else {
                image::Rgb([0, 0, 0])
            }
        });
        image.save(&path).expect("写测试 PNG 失败");
        path
    }

    #[test]
    fn analyze_reports_size_brightness_and_composition() {
        let path = temp_png("basic");
        let report = analyze(path.to_str().unwrap()).expect("应能分析");
        assert!(report.contains("图片尺寸: 40×40px"), "{report}");
        assert!(report.contains("亮度分布"), "{report}");
        assert!(report.contains("对称性"), "{report}");
        assert!(report.contains("构图建议"), "{report}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn analyze_rejects_non_images_with_a_readable_reason() {
        let path = std::env::temp_dir().join(format!(
            "lunac-image-pattern-bad-{}.bin",
            std::process::id()
        ));
        std::fs::write(&path, b"not an image").unwrap();
        let error = analyze(path.to_str().unwrap()).unwrap_err();
        assert!(error.contains("无法解码图片"), "{error}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn hsv_and_color_names_cover_edges() {
        assert_eq!(rgb_to_hsv(0.0, 0.0, 0.0), (0.0, 0.0, 0.0));
        assert_eq!(color_name(0.0, 0.0, 0.0), "黑");
        assert_eq!(color_name(1.0, 1.0, 1.0), "白");
        assert_eq!(color_name(1.0, 0.0, 0.0), "红");
        assert_eq!(color_name(0.0, 1.0, 0.0), "绿");
        // h=240° 落在第 9 桶（0-30 为 0 号桶，240/30 = 8）⇒ 名字是「蓝紫」。
        assert_eq!(color_name(0.0, 0.0, 1.0), "蓝紫");
    }
}
