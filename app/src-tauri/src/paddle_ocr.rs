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
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

// ── 引擎部署 / 按需下载 ───────────────────────────────────────────
//
// PaddleOCR-json 体积大（.7z 约 88MB，解压后约 300MB），不随仓库分发
// （见 .gitignore）。运行时若缺失，由前端按需触发下载到 `<exe 根>\paddle-ocr`。

/// PaddleOCR-json v1.4.1 Windows x64 发行包。
/// 注意：该 Release 的 Windows 资产只有 `.7z`（没有 `.zip`），
/// 用 `Expand-Archive` 解不了，故使用纯 Rust 的 sevenz-rust 解压。
pub const PADDLE_OCR_URL: &str = "https://github.com/hiroi-sora/PaddleOCR-json/releases/download/v1.4.1/PaddleOCR-json_v1.4.1_windows_x64.7z";

/// 引擎安装根目录：`<exe 根>\paddle-ocr`（与 storage.rs 数据根一致）。
pub fn engine_root() -> PathBuf {
    crate::storage::lunac_root_dir().join("paddle-ocr")
}

/// 引擎是否已就绪（能定位到 PaddleOCR-json.exe 且默认中文模型配置存在）。
pub fn engine_installed() -> bool {
    match paddle_ocr_dir() {
        Ok(dir) => dir.join(paddle_ocr_config_for_lang("chs")).exists(),
        Err(_) => false,
    }
}

/// 下载并安装 PaddleOCR-json 引擎。
///
/// 流程：下载 .7z → 解压到 staging → 校验 → 原子替换到 `<exe 根>\paddle-ocr`。
/// 任何一步失败都会清理半成品，不会留下损坏目录（否则 `paddle_ocr_dir()`
/// 会定位到残缺目录、OCR 永久失败却看不出原因）。
///
/// `on_progress(已下载字节, 总字节)`：总字节未知时为 0。
pub fn install_engine<F: FnMut(u64, u64)>(mut on_progress: F) -> Result<(), String> {
    let root_dir = crate::storage::lunac_root_dir();
    let temp_dir = root_dir.join("temp");
    fs::create_dir_all(&temp_dir).map_err(|e| format!("创建临时目录失败: {e}"))?;
    let archive = temp_dir.join("paddle-ocr.7z");
    let staging = temp_dir.join("paddle-ocr-staging");

    // ── 1. 下载 ──────────────────────────────────────────────────
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(900))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    let mut resp = client
        .get(PADDLE_OCR_URL)
        .send()
        .map_err(|e| format!("下载失败（网络不可达？）: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败: HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    {
        let mut out = fs::File::create(&archive).map_err(|e| format!("创建文件失败: {e}"))?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut done: u64 = 0;
        loop {
            let n = resp
                .read(&mut buf)
                .map_err(|e| format!("读取响应失败: {e}"))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .map_err(|e| format!("写入文件失败: {e}"))?;
            done += n as u64;
            on_progress(done, total);
        }
    }

    // ── 2. 解压到 staging（先不碰正式目录）───────────────────────
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| format!("创建解压目录失败: {e}"))?;
    if let Err(e) = sevenz_rust::decompress_file(&archive, &staging) {
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_file(&archive);
        return Err(format!("7z 解压失败: {e}"));
    }
    let _ = fs::remove_file(&archive);

    // ── 3. 校验 staging（必须含 exe + 默认中文模型配置）──────────
    let verify = |dir: &PathBuf| -> bool {
        dir.join("PaddleOCR-json.exe").exists()
            && dir.join(paddle_ocr_config_for_lang("chs")).exists()
    };
    let staged_dir = match find_engine_dir(&staging) {
        Some(d) if verify(&d) => d,
        _ => {
            let _ = fs::remove_dir_all(&staging);
            return Err(
                "解压后未找到可用的 PaddleOCR-json.exe / models 配置，安装包可能不完整".into(),
            );
        }
    };

    // ── 4. 原子替换到 <exe 根>\paddle-ocr ───────────────────────
    let target = engine_root();
    if target.exists() {
        fs::remove_dir_all(&target).map_err(|e| format!("清理旧引擎目录失败: {e}"))?;
    }
    // staged_dir 可能已是 staging 本身；统一 rename（同盘，原子）
    fs::rename(&staged_dir, &target).map_err(|e| format!("移动到目标目录失败: {e}"))?;
    let _ = fs::remove_dir_all(&staging);

    // ── 5. 最终验证（以运行时实际查找结果为准）──────────────────
    if !engine_installed() {
        return Err("安装完成但引擎仍无法定位，请检查目录权限".into());
    }
    Ok(())
}

/// PaddleOCR-json 输出结构 ────────────────────────────────────────
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

/// 在给定目录下查找 PaddleOCR-json.exe **所在目录**，支持两种发行结构：
///   A. 扁平：root/PaddleOCR-json.exe（models/ 与 exe 同级）
///   B. 嵌套：root/<子目录>/PaddleOCR-json.exe（最多两层，兼容
///      `paddle-ocr/PaddleOCR-json/PaddleOCR-json_v1.4.1/` 这种解包结构）
pub fn find_engine_dir(root: &Path) -> Option<PathBuf> {
    if root.join("PaddleOCR-json.exe").exists() {
        return Some(root.to_path_buf());
    }
    for entry in fs::read_dir(root).ok()?.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        if p.join("PaddleOCR-json.exe").exists() {
            return Some(p);
        }
        if let Ok(sub_entries) = fs::read_dir(&p) {
            for sub in sub_entries.flatten() {
                let sp = sub.path();
                if sp.is_dir() && sp.join("PaddleOCR-json.exe").exists() {
                    return Some(sp);
                }
            }
        }
    }
    None
}

/// 返回 PaddleOCR-json 可执行文件所在目录。
///
/// 搜索优先级：
///   1. `<exe 根>\paddle-ocr\`  — 安装根/数据根（见 storage.rs），也是自动下载的落点
///   2. 项目根目录下的 `paddle-ocr/`（dev 模式：从 target/ 向上导航）
///   3. 当前工作目录（兜底）
fn paddle_ocr_dir() -> Result<PathBuf, String> {
    let exe_dir = std::env::current_exe()
        .map_err(|e| format!("无法获取 exe 路径: {e}"))?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    // Priority 1: <exe 根>\paddle-ocr\
    if let Some(found) = find_engine_dir(&exe_dir.join("paddle-ocr")) {
        return Ok(found);
    }

    // Priority 2: 从 target/ 向上导航到项目根（dev mode）
    if let Some(found) = find_engine_dir(
        &exe_dir
            .join("..")
            .join("..")
            .join("..")
            .join("..")
            .join("paddle-ocr"),
    ) {
        return Ok(found);
    }

    // Priority 3: 相对当前工作目录（兜底）
    if let Some(found) =
        find_engine_dir(&std::env::current_dir().unwrap_or_default().join("paddle-ocr"))
    {
        return Ok(found);
    }

    Err(format!(
        "OCR 引擎未安装。请点击「下载并安装」自动获取，或手动放置到 {}",
        engine_root().display()
    ))
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
