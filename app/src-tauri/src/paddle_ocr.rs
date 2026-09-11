// src-tauri/src/paddle_ocr.rs
// PaddleOCR-json 子进程 OCR — 基于百度 PP-OCRv4 模型，中文精度远超 Windows OCR。
//
// PaddleOCR-json 是 Umi-OCR (42k+ GitHub stars) 内使用的 C++ OCR 引擎，
// 通过命令行调用并以 JSON 输出识别结果。
//
// 协议（单图模式）：
//   PaddleOCR-json.exe -image_path=<path> -config_path=<config> -ensure_ascii=false
//   stdout → JSON: {"code":100,"data":[{"text":"...","box":[...],"score":0.99}]}
//
// 部署：paddle-ocr/ 目录与 core/ 平级，发行版通过 NSIS 打包到 lunac.exe 同目录。

use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

// ── PaddleOCR-json 输出结构 ──────────────────────────────────────
//
// 成功：{"code":100,"data":[{"text":"...","box":[...],"score":0.99}]}
// 错误：{"code":200,"data":"Image path does not exist. ..."}
//  data 字段在 code=100 时为数组，code!=100 时为字符串错误消息。
//  用 serde_json::Value 兼容两种类型，根据 code 分别处理。

#[derive(Debug, Deserialize)]
struct PaddleOcrResponse {
    code: i32,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct PaddleOcrBlock {
    text: String,
    #[allow(dead_code)]
    score: f64,
}

// ── 路径解析 ─────────────────────────────────────────────────────

/// 返回 PaddleOCR-json 可执行文件目录。
///
/// 目录结构可能有三种形态：
///   A. 扁平：paddle-ocr/PaddleOCR-json.exe（models/ 与 exe 同级）
///   B. 嵌套：paddle-ocr/PaddleOCR-json/PaddleOCR-json_v1.4.1/PaddleOCR-json.exe（dev repo 结构）
///   C. NSIS 安装后：%LOCALAPPDATA%\Lunac\paddle-ocr\PaddleOCR-json\PaddleOCR-json_v1.4.1\...
///
/// 搜索优先级：
///   1. %LOCALAPPDATA%\Lunac\paddle-ocr\ — NSIS 安装（扁平和嵌套两种结构）
///   2. exe 同目录下的 paddle-ocr\      — 手动放置
///   3. 项目根目录下的 paddle-ocr/（dev 嵌套结构）
///   4. 当前工作目录（兜底）
fn paddle_ocr_dir() -> Result<PathBuf, String> {
    /// 在给定目录下搜索 PaddleOCR-json.exe，支持扁平和嵌套两种结构
    fn find_exe_in_dir(root: &PathBuf) -> Option<PathBuf> {
        // A. 扁平：exe 直接在 root 下
        if root.join("PaddleOCR-json.exe").exists() {
            return Some(root.clone());
        }
        // B. 嵌套：root/PaddleOCR-json/PaddleOCR-json_v*/PaddleOCR-json.exe
        // 遍历 root 下的一级子目录，再找 PaddleOCR-json_v* 子目录
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    // 尝试 p/PaddleOCR-json.exe（扁平嵌套）
                    if p.join("PaddleOCR-json.exe").exists() {
                        return Some(p);
                    }
                    // 尝试 p/*/PaddleOCR-json.exe（深层嵌套）
                    if let Ok(sub_entries) = std::fs::read_dir(&p) {
                        for sub in sub_entries.flatten() {
                            let sp = sub.path();
                            if sp.is_dir() && sp.join("PaddleOCR-json.exe").exists() {
                                return Some(sp);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    // Priority 1: <exe_dir>\paddle-ocr\（安装根/数据根，见 storage.rs）
    let exe_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let release_dir = exe_dir.join("paddle-ocr");
    if let Some(found) = find_exe_in_dir(&release_dir) {
        return Ok(found);
    }

    // Priority 2: 从 target/ 向上导航到项目根（dev mode）
    if let Some(found) = find_exe_in_dir(
        &exe_dir.join("..").join("..").join("..").join("..").join("paddle-ocr")
    ) {
        return Ok(found);
    }

    // Priority 3: 相对当前工作目录（兜底）
    if let Some(found) = find_exe_in_dir(
        &std::env::current_dir().unwrap_or_default().join("paddle-ocr")
    ) {
        return Ok(found);
    }

    Err("PaddleOCR-json.exe not found. Place it in the app data root (<exe_dir>\\paddle-ocr) or run scripts/download-paddle-ocr.ps1 first.".into())
}

/// PaddleOCR-json v1.4.1 支持的识别语言配置
pub fn paddle_ocr_config_for_lang(lang: &str) -> &str {
    match lang {
        "cht" | "chinese_cht" | "traditional" => "models/config_chinese_cht.txt",
        "en" | "english" => "models/config_en.txt",
        "japan" | "japanese" | "ja" => "models/config_japan.txt",
        "korean" | "ko" => "models/config_korean.txt",
        "cyrillic" | "ru" | "russian" => "models/config_cyrillic.txt",
        _ => "models/config_chinese.txt", // 默认中文（含中英混合）
    }
}

// ── 核心识别函数 ─────────────────────────────────────────────────

/// 调用 PaddleOCR-json.exe 识别图片中的文字。
///
/// # Arguments
/// * `image_path` - 图片文件绝对路径（支持 PNG/JPG/BMP/TIFF）
/// * `lang` - 识别语言（"chs"/"cht"/"en"/"japan"/"korean"/"cyrillic"），默认 "chs"
///
/// # Returns
/// * `Ok(text)` - 识别到的所有文字，按行合并，用换行分隔
/// * `Err(msg)` - 错误信息
pub fn recognize_image(image_path: &str, lang: &str) -> Result<String, String> {
    let image = std::path::Path::new(image_path);
    if !image.exists() {
        return Err(format!(
            "Image file not found: {}\n(Please verify the file path or re-paste the image)",
            image.display()
        ));
    }

    let dir = paddle_ocr_dir()?;
    let exe = dir.join("PaddleOCR-json.exe");
    let config = dir.join(paddle_ocr_config_for_lang(lang));

    if !config.exists() {
        return Err(format!("Config file not found: {}", config.display()));
    }

    // 构建命令：单图模式，输出到 stdout
    // CRITICAL: current_dir 必须设为 PaddleOCR-json.exe 所在目录，
    // 因为 config 中的模型路径（如 models/ch_PP-OCRv3_det_infer）是相对路径。
    let mut cmd = Command::new(&exe);
    cmd.current_dir(&dir)
        .args([
            &format!("-image_path={}", image_path),
            &format!("-config_path={}", config.display()),
            "-ensure_ascii=false", // 中文直接 UTF-8 输出，不做 \uXXXX 转义
            "-cpu_threads=4",      // 限制线程数，减少内存占用
            "-det_db_box_thresh=0.3",
            "-det_db_thresh=0.3",
        ]);
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = cmd
        .output()
        .map_err(|e| format!("Failed to spawn PaddleOCR-json: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "PaddleOCR-json exited with error: {}",
            stderr.trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // PaddleOCR-json 单图模式输出：先打印 init banner，然后一行 JSON。
    // 提取最后一行 JSON（跳过前面的 init 文本行）。
    let json_line = stdout
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .unwrap_or("")
        .trim()
        .to_string();

    if json_line.is_empty() {
        return Ok(String::new()); // 未识别到文字
    }

    // 解析 JSON 输出
    let response: PaddleOcrResponse = serde_json::from_str(&json_line)
        .map_err(|e| format!("Failed to parse PaddleOCR-json output: {}\nLine: {}", e, json_line))?;

    match response.code {
        100 => {
            // 成功：data 是 [{text, score, box}, ...] 数组
            let blocks: Vec<PaddleOcrBlock> = serde_json::from_value(response.data)
                .map_err(|e| format!("Failed to parse OCR blocks: {}", e))?;
            if blocks.is_empty() {
                return Ok(String::new());
            }
            let text = blocks
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            Ok(text)
        }
        _ => {
            // 错误：data 可能是字符串错误消息
            let msg = if response.data.is_string() {
                response.data.as_str().unwrap_or("Unknown error").to_string()
            } else {
                format!("PaddleOCR-json returned code {}: {:?}", response.code, response.data)
            };
            Err(msg)
        }
    }
}

// ── 测试 ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_for_lang() {
        assert_eq!(paddle_ocr_config_for_lang("chs"), "models/config_chinese.txt");
        assert_eq!(paddle_ocr_config_for_lang("en"), "models/config_en.txt");
        assert_eq!(paddle_ocr_config_for_lang("ja"), "models/config_japan.txt");
        assert_eq!(paddle_ocr_config_for_lang("unknown"), "models/config_chinese.txt");
    }
}
