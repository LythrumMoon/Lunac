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
// 部署（2026-09-30 改）：引擎是 **`ocr` 插件的依赖**，由插件市场安装插件时按清单的
// `dependencies[]`（`type = "archive"`）下载解压到 `Modules\ocr\paddle-ocr\`
// （见 plugin_market.rs 的 install_archive_dependency）。
//
// **不再随安装包分发、宿主也不再自下载**：引擎压缩后约 88MB / 解压约 300MB，塞进安装包等于
// 让所有用户替少数人的功能买单（见 ai-spec §3.5）。宿主侧因此**没有**任何下载代码 ——
// 引擎的获取只有一条路：装/修插件时走插件依赖（`install_plugin_dependencies`）。
// 老版本装在 `<exe 根>\paddle-ocr\` 的那份仍然认（见 `paddle_ocr_dir` 的优先级），
// 免得老用户升级后 OCR 突然失效；卸载时由 nsis-hooks.nsh 清掉。

use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

// ── 引擎落点 ──────────────────────────────────────────────────────

/// 引擎安装根目录：`<exe 根>\Modules\ocr\paddle-ocr`（与插件目录同源，见 plugin_market）。
pub fn engine_root() -> PathBuf {
    crate::plugin_market::plugins_dir()
        .join("ocr")
        .join("paddle-ocr")
}

/// 引擎是否已就绪（能定位到 PaddleOCR-json.exe 且默认中文模型配置存在）。
pub fn engine_installed() -> bool {
    match paddle_ocr_dir() {
        Ok(dir) => dir.join(paddle_ocr_config_for_lang("chs")).exists(),
        Err(_) => false,
    }
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
///   1. `<exe 根>\Modules\ocr\paddle-ocr\`  — **正规落点**（2026-09-30）：`ocr` 插件的
///      `archive` 依赖解压到这里（见 plugin_market.rs），删插件 = 引擎一起删
///   2. `<exe 根>\paddle-ocr\`  — **老版本的落点**（引擎曾随安装包分发）。留着它是因为
///      老用户升级后引擎还躺在原处，去掉这条他们的 OCR 会突然失效；卸载由 nsis-hooks.nsh 兜底
///   3. 项目根目录下的 `paddle-ocr/`（dev 模式：从 target/ 向上导航）
///   4. 当前工作目录（兜底）
fn paddle_ocr_dir() -> Result<PathBuf, String> {
    let exe_dir = std::env::current_exe()
        .map_err(|e| format!("无法获取 exe 路径: {e}"))?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    // Priority 1: 插件依赖的落点（<exe 根>\Modules\ocr\paddle-ocr\）
    if let Some(found) = find_engine_dir(&engine_root()) {
        return Ok(found);
    }

    // Priority 2: 老版本的落点（<exe 根>\paddle-ocr\）
    if let Some(found) = find_engine_dir(&exe_dir.join("paddle-ocr")) {
        return Ok(found);
    }

    // Priority 3: 从 target/ 向上导航到项目根（dev mode）
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

    // Priority 4: 相对当前工作目录（兜底）
    if let Some(found) =
        find_engine_dir(&std::env::current_dir().unwrap_or_default().join("paddle-ocr"))
    {
        return Ok(found);
    }

    Err(format!(
        "OCR 引擎未安装。请在 设置 → 插件 里装上/修复「OCR 文字识别」（引擎会作为它的依赖一并下载），或手动放置到 {}",
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
    // **拆成 spawn + wait_with_output，只为拿到句柄绑生命周期**（预检 #57，2026-10-02）：
    // `.output()` 内部 spawn 完就把句柄吞了，我们没法把它加进 job —— 宿主崩溃时
    // 这个 OCR 子进程就成了孤儿（它会驻留几秒做推理，不是瞬间进程）。
    // ⚠️ 两个管道必须**显式**声明 piped：`.output()` 会替我们设，`.spawn()` **不会**
    // （默认继承）—— 漏了这一句，`wait_with_output` 拿回的 stdout 就是空的，
    // 表现为「识别成功但一个字都没有」，而那是极难查的一类静默失效。
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn PaddleOCR-json: {}", e))?;
    crate::child_job::assign(&child);
    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to read PaddleOCR-json output: {}", e))?;

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
