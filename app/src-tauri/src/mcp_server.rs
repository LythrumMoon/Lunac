// MCP stdio server — bridges user-defined tools to agent.exe Agent
//
// Reads tool definitions from <exe 根>\tools\*.json（便携模式数据根，见 storage.rs）
// Implements MCP stdio transport (JSON-RPC 2.0 over stdin/stdout)
// Spawned by agent.exe as a child process: --mcp-server stdio:<lunac.exe 路径>

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::Command as StdCommand;
use std::process::Stdio;

// ── Tool definition types ─────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value, // JSON Schema for tool parameters
    pub handler: ToolHandler,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "type")]
pub enum ToolHandler {
    #[serde(rename = "shell")]
    Shell { command: String },
    #[serde(rename = "http")]
    Http {
        method: String,
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default)]
        body: Option<String>,
    },
    /// 内置工具：由 lunac 进程内直接执行（如图片图案特征分析）。
    /// 完全离线，不依赖外部命令或网络。
    #[serde(rename = "builtin")]
    Builtin { name: String },
}

// ── MCP protocol types ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct McpRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct McpResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<McpError>,
}

#[derive(Debug, Serialize)]
struct McpError {
    code: i32,
    message: String,
}

// ── Tool loader ───────────────────────────────────────────────────

fn tools_dir() -> PathBuf {
    crate::storage::lunac_root_dir().join("tools")
}

fn load_tools() -> Vec<ToolDef> {
    let dir = tools_dir();
    let mut tools = Vec::new();

    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "json") {
                if let Ok(content) = fs::read_to_string(&path) {
                    match serde_json::from_str::<ToolDef>(&content) {
                        Ok(tool) => tools.push(tool),
                        Err(e) => {
                            eprintln!("[mcp] Failed to parse {}: {}", path.display(), e);
                        }
                    }
                }
            }
        }
    }

    tools
}

// ── Tool executor ─────────────────────────────────────────────────

fn resolve_template(template: &str, params: &Value) -> String {
    let mut result = template.to_string();
    if let Some(obj) = params.as_object() {
        for (key, val) in obj {
            let placeholder = format!("{{{{{} }}}}", key);
            // Try brace-less variant too (some tools use {{key}})
            let placeholder2 = format!("{{{{{}}}}}", key);
            let val_str = match val {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            result = result.replace(&placeholder, &val_str);
            result = result.replace(&placeholder2, &val_str);
        }
    }
    result
}

/// Run a shell command with a hard timeout so a hung process can never block
/// the agent turn forever. stdout/stderr are drained on background threads
/// (piped buffers would otherwise deadlock), and on timeout the child is
/// killed and an error is returned.
fn run_shell_with_timeout(command: &str, timeout_secs: u64) -> Result<String, String> {
    let mut c = if cfg!(target_os = "windows") {
        let mut c = StdCommand::new("cmd");
        c.args(["/c", command]);
        #[cfg(target_os = "windows")]
        {
            // CREATE_NO_WINDOW — GUI 进程（release）下不设置会弹出 cmd 窗口
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        c
    } else {
        let mut c = StdCommand::new("sh");
        c.args(["-c", command]);
        c
    };
    c.stdout(Stdio::piped());
    c.stderr(Stdio::piped());

    let mut child = c.spawn().map_err(|e| format!("Failed to execute: {}", e))?;

    let out = child.stdout.take().map(|o| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = String::new();
            let mut r = o;
            let _ = r.read_to_string(&mut buf);
            buf
        })
    });
    let err = child.stderr.take().map(|e| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = String::new();
            let mut r = e;
            let _ = r.read_to_string(&mut buf);
            buf
        })
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "命令执行超时（{}s），已终止。耗时任务请改为后台运行。",
                    timeout_secs
                ));
            }
        }
    };

    let stdout = out.map(|h| h.join().unwrap_or_default()).unwrap_or_default();
    let stderr = err.map(|h| h.join().unwrap_or_default()).unwrap_or_default();

    if exit_status.success() {
        // Return stdout if non-empty, otherwise stderr for informational output
        if !stdout.trim().is_empty() {
            Ok(stdout)
        } else if !stderr.trim().is_empty() {
            Ok(stderr)
        } else {
            Ok("(no output)".into())
        }
    } else {
        Err(format!(
            "Command failed (exit {}): {}",
            exit_status,
            stderr.trim()
        ))
    }
}

fn execute_tool_handler(handler: &ToolHandler, params: &Value) -> Result<String, String> {
    match handler {
        ToolHandler::Builtin { name } => run_builtin_tool(name, params),
        ToolHandler::Shell { command } => {
            let resolved = resolve_template(command, params);
            run_shell_with_timeout(&resolved, 60)
        }
        ToolHandler::Http {
            method,
            url,
            headers,
            body,
        } => {
            let resolved_url = resolve_template(url, params);
            let resolved_body = body.as_ref().map(|b| resolve_template(b, params));

            let client = reqwest::blocking::Client::new();
            let mut req = match method.to_uppercase().as_str() {
                "GET" => client.get(&resolved_url),
                "POST" => {
                    let mut r = client.post(&resolved_url);
                    if let Some(ref b) = resolved_body {
                        r = r.body(b.clone());
                    }
                    r
                }
                "PUT" => {
                    let mut r = client.put(&resolved_url);
                    if let Some(ref b) = resolved_body {
                        r = r.body(b.clone());
                    }
                    r
                }
                "DELETE" => client.delete(&resolved_url),
                other => return Err(format!("Unsupported HTTP method: {}", other)),
            };

            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }

            let resp = req.send().map_err(|e| format!("HTTP request failed: {}", e))?;
            let status = resp.status();
            let body_text = resp
                .text()
                .map_err(|e| format!("Failed to read response: {}", e))?;

            if status.is_success() {
                Ok(body_text)
            } else {
                Err(format!("HTTP {}: {}", status.as_u16(), body_text))
            }
        }
    }
}

// ── Builtin tools（进程内执行，完全离线）───────────────────────────

/// 分派内置工具调用。当前提供「本地图片图案特征分析」。
fn run_builtin_tool(name: &str, params: &Value) -> Result<String, String> {
    match name {
        "image_pattern_analysis" => {
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| "缺少 path 参数：需传入图片的绝对路径（jpg/png）".to_string())?;
            analyze_image_pattern(path)
        }
        other => Err(format!("未知内置工具: {}", other)),
    }
}

/// RGB(0..1) → HSV(h:0..360, s:0..1, v:0..1)
fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

fn color_name(r: f32, g: f32, b: f32) -> &'static str {
    let (h, s, v) = rgb_to_hsv(r, g, b);
    if v < 0.15 {
        return "黑";
    }
    if s < 0.15 {
        return if v > 0.9 { "白" } else if v > 0.6 { "浅灰" } else { "深灰" };
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
        return "无明显彩色倾向（近灰阶）".into();
    }
    let mut best = 0usize;
    for i in 1..12 {
        if hue_hist[i] > hue_hist[best] {
            best = i;
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
        return "无明显边缘".into();
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

/// 本地图片图案特征分析：解码图片 → 缩略采样 → 统计亮度/色相/边缘/
/// 高光/纹理/对称度，输出结构化中文报告（纯像素统计，不做语义理解）。
fn analyze_image_pattern(path: &str) -> Result<String, String> {
    let img = image::open(path).map_err(|e| format!("无法解码图片 {}（{}）", path, e))?;
    let (orig_w, orig_h) = (img.width(), img.height());
    // 缩略到 160 宽以内，控制计算量
    let thumb = img.thumbnail(160, 160);
    let rgb = thumb.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    if w == 0 || h == 0 {
        return Err("图片尺寸无效".into());
    }

    let px = |x: usize, y: usize| -> (f32, f32, f32) {
        let p = rgb.get_pixel(x as u32, y as u32);
        (
            p[0] as f32 / 255.0,
            p[1] as f32 / 255.0,
            p[2] as f32 / 255.0,
        )
    };
    let lum = |x: usize, y: usize| -> f32 {
        let (r, g, b) = px(x, y);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    };

    let n = (w * h) as f32;

    // 1. 亮度：整体均值 + 4×4 网格 + 高光点（>0.85）
    const GRID: usize = 4;
    let (gw, gh) = (w / GRID, h / GRID);
    let cells_px = (gw.max(1) * gh.max(1)) as f32;
    let mut grid_lum = [[0f32; GRID]; GRID];
    let mut total_lum = 0f32;
    let mut high_light = 0usize;
    for y in 0..h {
        for x in 0..w {
            let l = lum(x, y);
            total_lum += l;
            if l > 0.85 {
                high_light += 1;
            }
            if gw > 0 && gh > 0 {
                grid_lum[(y / gh).min(GRID - 1)][(x / gw).min(GRID - 1)] += l;
            }
        }
    }
    let avg_lum = total_lum / n;
    let grid_str: Vec<String> = grid_lum
        .iter()
        .map(|row| {
            row.iter()
                .map(|v| format!("{:>3.0}", (v / cells_px) * 100.0))
                .collect::<Vec<_>>()
                .join("  ")
        })
        .collect();

    // 2. 色相分布（12 桶，S>0.15 计入）+ 饱和度均值 + 主色 top5
    let mut hue_hist = [0usize; 12];
    let mut sat_sum = 0f32;
    let mut color_buckets: HashMap<u16, usize> = HashMap::new();
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = px(x, y);
            let (hue, s, _v) = rgb_to_hsv(r, g, b);
            sat_sum += s;
            if s > 0.15 {
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
        .map(|(k, c)| {
            let r = ((k & 0xF) * 17) as f32 / 255.0;
            let g = (((k >> 4) & 0xF) * 17) as f32 / 255.0;
            let b = (((k >> 8) & 0xF) * 17) as f32 / 255.0;
            format!("{} {:.0}%", color_name(r, g, b), (*c as f32 / n) * 100.0)
        })
        .collect();

    // 3. 边缘方向直方图（Sobel，|角度| 分 4 桶）+ 纹理密度（3×3 局部标准差）
    let mut edge_hist = [0usize; 4]; // 水平 / 右斜 / 垂直 / 左斜
    let mut edge_count = 0usize;
    let mut texture_sum = 0f32;
    let mut tex_cnt = 0usize;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let mut vals = [0f32; 9];
            let mut mean = 0f32;
            for dy in 0..3 {
                for dx in 0..3 {
                    let v = lum(x + dx - 1, y + dy - 1);
                    vals[dy * 3 + dx] = v;
                    mean += v;
                }
            }
            mean /= 9.0;
            let mut var = 0f32;
            for v in &vals {
                var += (v - mean) * (v - mean);
            }
            texture_sum += (var / 9.0).sqrt();
            tex_cnt += 1;

            let gx = (lum(x + 1, y - 1) + 2.0 * lum(x + 1, y) + lum(x + 1, y + 1))
                - (lum(x - 1, y - 1) + 2.0 * lum(x - 1, y) + lum(x - 1, y + 1));
            let gy = (lum(x - 1, y + 1) + 2.0 * lum(x, y + 1) + lum(x + 1, y + 1))
                - (lum(x - 1, y - 1) + 2.0 * lum(x, y - 1) + lum(x + 1, y - 1));
            if gx * gx + gy * gy > 0.003 {
                edge_count += 1;
                let ang = gy.atan2(gx).to_degrees().abs();
                let dir = if ang <= 22.5 {
                    0
                } else if ang <= 67.5 {
                    1
                } else if ang <= 112.5 {
                    2
                } else if ang <= 157.5 {
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

    // 4. 水平对称度（左右镜像亮度差）
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
        x if x > 0.75 => "高亮",
        x if x > 0.5 => "明亮",
        x if x > 0.3 => "中等",
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

    // Rust 的 format! 不支持 `%` 格式类型，百分比统一手动乘 100 后格式化
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

// ── MCP protocol handler ──────────────────────────────────────────

fn build_tools_list(tools: &[ToolDef]) -> Value {
    let items: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.input_schema,
            })
        })
        .collect();
    json!({ "tools": items, "nextCursor": null })
}

fn handle_request(req: &McpRequest, tools: &[ToolDef]) -> McpResponse {
    match req.method.as_deref() {
        Some("initialize") => McpResponse {
            jsonrpc: "2.0".into(),
            id: req.id.clone(),
            result: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "lunac-mcp",
                    "version": "0.1.0"
                }
            })),
            error: None,
        },
        Some("tools/list") => McpResponse {
            jsonrpc: "2.0".into(),
            id: req.id.clone(),
            result: Some(build_tools_list(tools)),
            error: None,
        },
        Some("tools/call") => {
            let params = req.params.as_ref().and_then(|v| v.get("name")).and_then(|n| n.as_str()).map(|n| n.to_string());
            let args = req.params.as_ref().and_then(|v| v.get("arguments")).cloned().unwrap_or(Value::Null);

            match params {
                Some(name) => {
                    match tools.iter().find(|t| t.name == name) {
                        Some(tool) => match execute_tool_handler(&tool.handler, &args) {
                            Ok(text) => McpResponse {
                                jsonrpc: "2.0".into(),
                                id: req.id.clone(),
                                result: Some(json!({
                                    "content": [{ "type": "text", "text": text }],
                                    "isError": false
                                })),
                                error: None,
                            },
                            Err(msg) => McpResponse {
                                jsonrpc: "2.0".into(),
                                id: req.id.clone(),
                                result: Some(json!({
                                    "content": [{ "type": "text", "text": msg }],
                                    "isError": true
                                })),
                                error: None,
                            },
                        },
                        None => McpResponse {
                            jsonrpc: "2.0".into(),
                            id: req.id.clone(),
                            error: Some(McpError {
                                code: -32602,
                                message: format!("Tool not found: {}", name),
                            }),
                            result: None,
                        },
                    }
                }
                None => McpResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id.clone(),
                    error: Some(McpError {
                        code: -32602,
                        message: "Missing tool name".into(),
                    }),
                    result: None,
                },
            }
        }
        Some("resources/list") => {
            let dir = tools_dir();
            let mut resources = Vec::new();
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(false, |e| e == "json") {
                        resources.push(json!({
                            "uri": format!("file:///{}", path.display()),
                            "name": path.file_stem().unwrap_or_default().to_string_lossy(),
                            "mimeType": "application/json"
                        }));
                    }
                }
            }
            McpResponse {
                jsonrpc: "2.0".into(),
                id: req.id.clone(),
                result: Some(json!({ "resources": resources })),
                error: None,
            }
        }
        _ => {
            // Notifications (no id) should be silently ignored per MCP spec
            if req.id.is_some() {
                McpResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id.clone(),
                    error: Some(McpError {
                        code: -32601,
                        message: format!("Method not found: {:?}", req.method),
                    }),
                    result: None,
                }
            } else {
                // Notification — don't respond
                McpResponse {
                    jsonrpc: "2.0".into(),
                    id: None,
                    result: None,
                    error: None,
                }
            }
        }
    }
}

// ── stdio transport entry point ────────────────────────────────────

pub fn run_stdio() {
    let tools = load_tools();
    eprintln!("[mcp] Loaded {} user-defined tools from {:?}", tools.len(), tools_dir());

    let stdin = io::stdin();
    let stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[mcp] Stdin error: {}", e);
                continue;
            }
        };

        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }

        let req: McpRequest = match serde_json::from_str(&trimmed) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[mcp] Parse error: {} for input: {:.200}", e, trimmed);
                continue;
            }
        };

        let resp = handle_request(&req, &tools);

        // Only respond to requests that have an id (notifications are one-way)
        if resp.id.is_some() || resp.error.is_some() {
            let resp_json = serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into());
            let mut stdout_lock = stdout.lock();
            if let Err(e) = writeln!(stdout_lock, "{}", resp_json) {
                eprintln!("[mcp] Write error: {}", e);
                return; // client closed stdin → exit
            }
            if let Err(e) = stdout_lock.flush() {
                eprintln!("[mcp] Flush error: {}", e);
                return;
            }
        }
    }
}
