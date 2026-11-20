//! OCR 插件的宿主侧（移植 v1 `app/src-tauri/src/paddle_ocr.rs`）。
//!
//! 引擎是 `ocr` 插件的 **`archive` 依赖**（v1 2026-09-30 起的部署口径）：
//! 解压到 `<exe 根>\Modules\ocr\paddle-ocr\`；老版本装在 `<exe 根>\paddle-ocr\`
//! 的那份仍然认（升级后 OCR 不突然失效）。引擎解包结构两种都支持：
//!   A. 扁平：`root/PaddleOCR-json.exe`（`models/` 与 exe 同级）
//!   B. 嵌套：`root/<子目录>/[<子目录>/]PaddleOCR-json.exe`
//!      （兼容 `paddle-ocr/PaddleOCR-json/PaddleOCR-json_v1.4.1/` 这种解包）
//!
//! 协议（单图模式，v1 `paddle_ocr.rs:7-9`）：
//!   `PaddleOCR-json.exe -image_path=<path> -config_path=<config> -ensure_ascii=false`
//!   stdout → `{"code":100,"data":[{"text":"...","box":[...],"score":0.99}]}`
//!   `code != 100` 时 `data` 是错误字符串。
//!
//! 纪律：任何失败都降级成一句人话（不 panic）；失败文案只说「哪一步没成 +
//! 用户能做什么」，原始 stderr / 解析细节只进日志。测试只测纯解析与临时目录里的
//! 假发行结构，**不真的跑 PaddleOCR**（引擎约 300MB，识别要几秒）。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// 单图识别超时：首次加载模型在慢机器上要几秒，60s 足够且不至于挂死面板。
const OCR_TIMEOUT: Duration = Duration::from_secs(60);

// ── 引擎落点（v1 `paddle_ocr.rs:29-95`）────────────────────────────

/// 引擎安装根目录：`<exe 根>\Modules\ocr\paddle-ocr`（与插件目录同源）。
pub fn engine_root(exe_root: &Path) -> PathBuf {
    exe_root.join("Modules").join("ocr").join("paddle-ocr")
}

/// 搜索顺序：正规落点优先，其次老版本落点（v1 `paddle_ocr_dir` 的 1、2）。
pub fn search_paths(exe_root: &Path) -> Vec<PathBuf> {
    vec![engine_root(exe_root), exe_root.join("paddle-ocr")]
}

/// 在给定目录下查找 `PaddleOCR-json.exe` **所在目录**，支持扁平与嵌套两种发行结构
/// （嵌套最多两层，与 v1 `find_engine_dir` 完全一致）。
pub fn find_engine_dir(root: &Path) -> Option<PathBuf> {
    if root.join("PaddleOCR-json.exe").exists() {
        return Some(root.to_path_buf());
    }
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("PaddleOCR-json.exe").exists() {
            return Some(path);
        }
        if let Ok(inner) = std::fs::read_dir(&path) {
            for sub in inner.flatten() {
                let sub_path = sub.path();
                if sub_path.is_dir() && sub_path.join("PaddleOCR-json.exe").exists() {
                    return Some(sub_path);
                }
            }
        }
    }
    None
}

/// 按搜索顺序定位引擎目录；找不到返回 None。
pub fn locate_engine(exe_root: &Path) -> Option<PathBuf> {
    search_paths(exe_root)
        .iter()
        .find_map(|root| find_engine_dir(root))
}

/// PaddleOCR-json v1.4.1 支持的识别语言配置（v1 同表）。
pub fn config_for_lang(lang: &str) -> &'static str {
    match lang {
        "cht" | "chinese_cht" | "traditional" => "models/config_chinese_cht.txt",
        "en" | "english" => "models/config_en.txt",
        "japan" | "japanese" | "ja" => "models/config_japan.txt",
        "korean" | "ko" => "models/config_korean.txt",
        "cyrillic" | "ru" | "russian" => "models/config_cyrillic.txt",
        _ => "models/config_chinese.txt", // 默认中文（含中英混合）
    }
}

/// 引擎是否就绪：能找到 exe 所在目录，且默认中文模型配置存在。
#[cfg(test)]
pub fn engine_installed(exe_root: &Path) -> bool {
    locate_engine(exe_root)
        .map(|dir| dir.join(config_for_lang("chs")).exists())
        .unwrap_or(false)
}

/// `lunac_ocr_status` 的 JSON（面板用：已装 / 未装 + 搜索过的路径）。
pub fn status_json(exe_root: &Path) -> String {
    let located = locate_engine(exe_root);
    let installed = located
        .as_ref()
        .map(|dir| dir.join(config_for_lang("chs")).exists())
        .unwrap_or(false);
    serde_json::json!({
        "installed": installed,
        "enginePath": located
            .as_ref()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default(),
        "searchPaths": search_paths(exe_root)
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
    })
    .to_string()
}

// ── 面板动作：选择图片 / 截屏识别（v1 `ocr.ts` 的两个按钮）──────────

/// 「选择文件」：系统单选框（图片过滤），返回选中路径；取消 = 空串。
pub fn pick_image() -> String {
    crate::convert::pick_inputs("image")
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// 「截屏识别」：拉起系统截图浮层，等剪贴板出现**新**图片。
///
/// v1 是宿主画一整块虚拟桌面的覆盖窗（`src-tauri/src/screenshot.rs`）；v2 还没有那扇窗，
/// 这里改用 Win10/11 自带的截图浮层（`ms-screenclip:`，框选后默认复制到剪贴板），
/// 拿到图后与 v1 `screenshot-overlay-done` 走同一条后续：落盘 bmp → OCR。
///
/// 为什么不用「直接读剪贴板」：剪贴板里常常本来就有一张旧图，必须先记指纹基线，
/// 只认**变化后**的那张，否则用户取消截屏也会识别一张不相干的图。
/// 返回 JSON：`{"path":"...", "error":""}`（取消 / 超时时 `path` 为空串）。
pub fn capture_screen(timeout: Duration) -> String {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::core::HSTRING;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let baseline = crate::clipboard_payload::image_fingerprint();
        let verb = HSTRING::from("open");
        let target = HSTRING::from("ms-screenclip:");
        let result = ShellExecuteW(None, &verb, &target, None, None, SW_SHOWNORMAL);
        // ShellExecuteW 约定：返回值 > 32 才算成功。
        if result.0 as isize <= 32 {
            return serde_json::json!({
                "path": "",
                "error": "没能拉起系统截屏（ms-screenclip 不可用）",
            })
            .to_string();
        }
        let started = Instant::now();
        while started.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(250));
            let Some(current) = crate::clipboard_payload::image_fingerprint() else {
                continue;
            };
            if Some(current) == baseline {
                continue;
            }
            if let Some(path) = crate::clipboard_payload::save_image_now() {
                return serde_json::json!({ "path": path, "error": "" }).to_string();
            }
        }
        serde_json::json!({
            "path": "",
            "error": "没有截到图片（已取消或超时）",
        })
        .to_string()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = timeout;
        serde_json::json!({ "path": "", "error": "当前平台不支持截屏识别" }).to_string()
    }
}

// ── stdout 解析（纯函数，单测覆盖）─────────────────────────────────

/// 从 PaddleOCR-json 的 stdout 里取文本行。
///
/// 单图模式会先打印 init banner，再输出一行 JSON；取**最后一条**以 `{` 开头的行。
/// 空 stdout = 未识别到文字（`Ok(vec![])`）；坏 JSON / 错误 code 都是 `Err`。
pub fn parse_stdout(stdout: &str) -> Result<Vec<String>, String> {
    let json_line = stdout
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .unwrap_or("")
        .trim();
    if json_line.is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(json_line)
        .map_err(|e| format!("OCR 引擎输出看不懂（JSON 解析失败：{e}）"))?;
    let code = value.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
    let data = value.get("data").cloned().unwrap_or(serde_json::Value::Null);
    if code == 100 {
        let blocks = data.as_array().cloned().unwrap_or_default();
        return Ok(blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(|v| v.as_str()))
            .map(|text| text.to_string())
            .collect());
    }
    // 错误：data 通常是字符串错误消息；不是字符串时给带 code 的兜底。
    Err(match data.as_str() {
        Some(message) if !message.trim().is_empty() => message.trim().to_string(),
        _ => format!("OCR 引擎返回错误码 {code}"),
    })
}

// ── 单图识别 ─────────────────────────────────────────────────────

/// 识别一张图片里的文字。
///
/// `Ok(lines)` = 按版面顺序的文本行（空 = 图里没字）；`Err` = 一句人话。
pub fn recognize_image(exe_root: &Path, image_path: &str, lang: &str) -> Result<Vec<String>, String> {
    let image_path = image_path.trim();
    if image_path.is_empty() {
        return Err("没有要识别的图片（请先加图片附件或从剪贴板取图）".to_string());
    }
    let image = Path::new(image_path);
    if !image.exists() {
        return Err(format!(
            "图片不存在：{image_path}（可能已移动或删除，请重新取图）"
        ));
    }
    let dir = locate_engine(exe_root).ok_or_else(|| {
        format!(
            "OCR 引擎未安装：请把 PaddleOCR-json 解压到 {}（含 PaddleOCR-json.exe 与 models/）",
            engine_root(exe_root).display()
        )
    })?;
    let config = dir.join(config_for_lang(lang));
    if !config.exists() {
        return Err(format!(
            "OCR 引擎缺少模型配置：{}（引擎目录可能不完整，请重新安装引擎）",
            config.display()
        ));
    }
    run_engine(&dir, image_path, &config)
}

fn run_engine(dir: &Path, image_path: &str, config: &Path) -> Result<Vec<String>, String> {
    let exe = dir.join("PaddleOCR-json.exe");
    let mut cmd = Command::new(&exe);
    // current_dir 必须是 exe 所在目录：配置里的模型路径是相对路径（v1 同口径）。
    cmd.current_dir(dir)
        .args([
            &format!("-image_path={image_path}"),
            &format!("-config_path={}", config.display()),
            "-ensure_ascii=false", // 中文直接 UTF-8 输出，不做 \uXXXX 转义
            "-cpu_threads=4",      // 限制线程数，减少内存占用
            "-det_db_box_thresh=0.3",
            "-det_db_thresh=0.3",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 OCR 引擎失败：{e}（请检查引擎文件是否完整）"))?;

    // 两个管道都自己读：不读的话输出超过管道缓冲会把子进程卡死，
    // 进而被我们的超时误判成「识别超时」。
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let stdout_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_end(&mut buffer);
        }
        buffer
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.read_to_end(&mut buffer);
        }
        buffer
    });

    let deadline = Instant::now() + OCR_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "OCR 识别超时（超过 {} 秒）：请换一张更小的图片，或重装引擎",
                        OCR_TIMEOUT.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("等待 OCR 引擎失败：{e}"));
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("读取 OCR 引擎退出状态失败：{e}"))?;
    let stdout_bytes = stdout_reader.join().unwrap_or_default();
    let stderr_bytes = stderr_reader.join().unwrap_or_default();

    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_bytes);
        let detail = stderr.trim();
        eprintln!("[lunac-ocr] 引擎异常退出（{status}）：{detail}");
        return Err(if detail.is_empty() {
            "OCR 引擎异常退出：请确认图片能正常打开，或重装引擎".to_string()
        } else {
            format!("OCR 引擎异常退出：{detail}（请确认图片能正常打开，或重装引擎）")
        });
    }

    // 非 UTF-8 按 lossy 处理（引擎正常输出 UTF-8，个别模型文件损坏时不 panic）。
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    parse_stdout(&stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lunac-ocr-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    /// 造假发行结构：只放一个空 exe 文件，引擎定位只认文件名与目录层级。
    fn write_fake_exe(dir: &Path) {
        std::fs::create_dir_all(dir).expect("engine dir");
        std::fs::write(dir.join("PaddleOCR-json.exe"), b"").expect("fake exe");
    }

    #[test]
    fn parse_stdout_reads_the_last_json_line_after_banner() {
        let stdout = "PaddleOCR-json init...\r\n\
             {\"code\":100,\"data\":[\
             {\"text\":\"第一行\",\"box\":[[1,2],[3,4]],\"score\":0.99},\
             {\"text\":\"second\",\"box\":[[1,2],[3,4]],\"score\":0.88}]}";
        assert_eq!(
            parse_stdout(stdout).expect("valid"),
            vec!["第一行".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn parse_stdout_maps_error_code_to_readable_message() {
        let stdout = r#"{"code":200,"data":"Image path does not exist."}"#;
        assert_eq!(
            parse_stdout(stdout).unwrap_err(),
            "Image path does not exist."
        );
        // data 不是字符串时给带 code 的兜底，而不是 panic。
        let odd = parse_stdout(r#"{"code":201}"#).unwrap_err();
        assert!(odd.contains("201"), "unexpected: {odd}");
    }

    #[test]
    fn parse_stdout_handles_empty_data_and_empty_output() {
        assert!(parse_stdout(r#"{"code":100,"data":[]}"#)
            .expect("empty data")
            .is_empty());
        // 空 data 但 code=100：按「没识别到文字」处理。
        assert!(parse_stdout(r#"{"code":100}"#)
            .expect("missing data")
            .is_empty());
        assert!(parse_stdout("").expect("empty stdout").is_empty());
        assert!(parse_stdout("init banner only\n").expect("banner").is_empty());
    }

    #[test]
    fn parse_stdout_rejects_broken_json() {
        assert!(parse_stdout("{not json").is_err());
        assert!(parse_stdout(r#"{"code":100,"data":[{"text":123}]}"#)
            .expect("non-string text is skipped")
            .is_empty());
    }

    #[test]
    fn find_engine_dir_supports_flat_layout() {
        let root = temp_root("flat");
        write_fake_exe(&root);
        assert_eq!(find_engine_dir(&root), Some(root.clone()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_engine_dir_supports_nested_layout() {
        let root = temp_root("nested");
        let nested = root
            .join("PaddleOCR-json")
            .join("PaddleOCR-json_v1.4.1");
        write_fake_exe(&nested);
        assert_eq!(find_engine_dir(&root), Some(nested));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn locate_engine_prefers_modules_and_falls_back_to_legacy() {
        // 缺失：两条搜索路径都返回出来，定位结果为 None。
        let root = temp_root("missing");
        assert!(locate_engine(&root).is_none());
        assert_eq!(search_paths(&root).len(), 2);
        assert_eq!(
            search_paths(&root)[0],
            root.join("Modules").join("ocr").join("paddle-ocr")
        );

        // 老版本落点单独存在时也能找到。
        let legacy = root.join("paddle-ocr").join("PaddleOCR-json");
        write_fake_exe(&legacy);
        assert_eq!(locate_engine(&root), Some(legacy));
        // 引擎目录存在但缺默认中文配置 → 未就绪。
        assert!(!engine_installed(&root));

        // 正规落点优先于老版本。
        let modules = engine_root(&root);
        let versioned = modules.join("PaddleOCR-json_v1.4.1");
        write_fake_exe(&versioned);
        std::fs::create_dir_all(versioned.join("models")).unwrap();
        std::fs::write(versioned.join("models").join("config_chinese.txt"), b"").unwrap();
        assert_eq!(locate_engine(&root), Some(versioned));
        assert!(engine_installed(&root));

        let status: serde_json::Value = serde_json::from_str(&status_json(&root)).unwrap();
        assert_eq!(status["installed"], true);
        assert_eq!(status["searchPaths"].as_array().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }
}
