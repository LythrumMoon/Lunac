//! 文件转换的宿主侧（移植 v1 `app/src-tauri/src/convert.rs`，2026-11-12）。
//!
//! 逐条对齐的部分：
//!   · 格式白名单与各族的候选目标（v1 `convert.rs:35-52`）；
//!   · 判族只看扩展名（v1 `ext_of` / `kind_of`，`convert.rs:54-73`）；
//!   · ffmpeg 参数（v1 `convert.rs:547-562`）与 LibreOffice 无头参数
//!     （v1 `convert.rs:465-476`）；
//!   · 引擎查找顺序：exe 根自带 → PATH（`where.exe`）→ LibreOffice 常见安装位置
//!     （v1 `resolve_engine` / `resolve_soffice`，`convert.rs:109-175`）；
//!   · 输出路径：源目录 / 同主名 / 换扩展名，冲突时 ` (1)` 递增，绝不覆盖源文件
//!     （v1 `unique_output_in`，`convert.rs:394-405`）；
//!   · 失败时把引擎 stderr 原样带回（截 500 字符），并删掉半成品
//!     （v1 `convert.rs:619-630`）；
//!   · **进度与取消**（2026-11-15 补）：解析 `-progress pipe:1` 的 `out_time_us`，
//!     用 ffprobe 探到的总时长算百分比；`cancel()` 按 pid 杀进程树并删半成品
//!     （v1 `convert_progress` / `convert_cancel` 同口径）。
//!
//! 与 v1 的差异（v2 是 C ABI，没有 Tauri 事件循环）：
//!   · 进度不推事件，改成 **Dart 轮询** `lunac_convert_progress`；取消走
//!     `lunac_convert_cancel`；
//!   · 不做 ffprobe 预探测来摘音频目标（视频目标一律含音频族）；这里只用它读时长；
//!   · LibreOffice 的「插件自带副本」在 v2 等价为 `<exe 根>/convert/libreoffice/`。
//!
//! 纪律：找不到引擎 / 子进程失败都返回**一句人话**，绝不 panic；单测不启动 ffmpeg。

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

// ── 格式表（v1 `convert.rs:35-52`）─────────────────────────────────

const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "jfif", "webp", "bmp", "tif", "tiff", "gif", "ico", "heic", "avif",
];
const AUDIO_EXTS: &[&str] = &[
    "mp3", "wav", "flac", "m4a", "aac", "ogg", "opus", "wma", "aiff", "ape", "amr",
];
const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "avi", "mov", "wmv", "flv", "m4v", "mpg", "mpeg", "ts", "3gp", "rmvb",
];

/// 可选的输出格式（v1 刻意只列 ffmpeg 稳的：不加 avif / heic / ico）。
const IMAGE_TARGETS: &[&str] = &["png", "jpg", "webp", "bmp", "tiff", "gif"];
const AUDIO_TARGETS: &[&str] = &["mp3", "wav", "flac", "m4a", "aac", "ogg", "opus"];
const VIDEO_TARGETS: &[&str] = &["mp4", "mkv", "webm", "avi", "mov", "gif"];
/// PDF 走 LibreOffice 无头模式（ffmpeg 读不了 PDF）。
const PDF_TARGETS: &[&str] = &["docx", "pptx", "xlsx", "png"];

/// 不支持的后缀（v1 `convert.unsupported` 文案）。
pub const UNSUPPORTED: &str = "这个后缀不支持转换";
/// v1 `convert.err_no_file`。
pub const ERR_NO_FILE: &str = "文件不存在或不可读";
/// v1 `convert.err_no_engine`。
pub const ERR_NO_ENGINE: &str = "未找到 ffmpeg，请先安装";
/// v1 `convert.err_no_pdf_engine`。
pub const ERR_NO_PDF_ENGINE: &str = "未找到 LibreOffice，PDF 转换不可用";
/// v1 `convert.err_out_dir`。
pub const ERR_OUT_DIR: &str = "保存目录不存在，请重新设置";

fn ext_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn kind_of(ext: &str) -> &'static str {
    if ext == "pdf" {
        "pdf"
    } else if IMAGE_EXTS.contains(&ext) {
        "image"
    } else if AUDIO_EXTS.contains(&ext) {
        "audio"
    } else if VIDEO_EXTS.contains(&ext) {
        "video"
    } else {
        "unknown"
    }
}

/// 这个扩展名是不是视频（`player.rs` 判「要不要出画面」用的唯一名单，
/// 与转换面板共用同一份 —— 「加了一个格式、只有一侧认」是最典型的静默失效）。
pub(crate) fn is_video_ext(ext: &str) -> bool {
    let ext = ext.trim().to_ascii_lowercase();
    VIDEO_EXTS.contains(&ext.as_str())
}

/// 该族的目标清单（v1 `targets_for`，`convert.rs:77-90`）：视频额外给音频目标，
/// 「从视频里抽音轨」是最常用的一类转换。
fn targets_for(kind: &str) -> Vec<String> {
    let list: &[&str] = match kind {
        "image" => IMAGE_TARGETS,
        "audio" => AUDIO_TARGETS,
        "video" => VIDEO_TARGETS,
        "pdf" => PDF_TARGETS,
        _ => &[],
    };
    let mut out: Vec<String> = list.iter().map(|s| s.to_string()).collect();
    if kind == "video" {
        out.extend(AUDIO_TARGETS.iter().map(|s| s.to_string()));
    }
    out
}

/// 该输入实际能转成什么（v1 `targets_for_input`）：去掉源扩展名本身。
///
/// v1 还会在 ffprobe 探测到无音轨时摘掉音频目标；v2 不做预探测（见文件头）。
fn targets_for_input(kind: &str, ext: &str) -> Vec<String> {
    let mut out = targets_for(kind);
    out.retain(|t| t != ext);
    out
}

/// 某族缺引擎时的提示（面板据此禁用「开始转换」并给出装法）。
fn missing_engine_message(kind: &str) -> &'static str {
    if kind == "pdf" {
        ERR_NO_PDF_ENGINE
    } else {
        ERR_NO_ENGINE
    }
}

// ── 引擎查找（v1 `convert.rs:107-175`）────────────────────────────

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
}

/// 找 ffmpeg / soffice：**exe 根自带的副本 → PATH**（只找、不下载）。
///
/// `pub(crate)`：`player.rs` 播放 `.webm` 时要借它找 ffmpeg 抽音轨
/// （v2 的 rodio 走 symphonia，不认 webm 容器 ⇒ 那一小段必须转一次）。
pub(crate) fn resolve_engine(name: &str) -> Option<PathBuf> {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let exe = format!("{name}.exe");
    for cand in [root.join(&exe), root.join("ffmpeg").join("bin").join(&exe)] {
        if cand.is_file() {
            return Some(cand);
        }
    }
    let out = Command::new("where.exe").arg(name).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}

/// LibreOffice 的「插件自带副本」在 v2 的等价位置：`<exe 根>/convert/libreoffice/`。
fn bundled_soffice(root: &Path) -> Option<PathBuf> {
    let base = root.join("convert").join("libreoffice");
    [
        base.join("program").join("soffice.exe"),
        base.join("LibreOffice").join("program").join("soffice.exe"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// 找 LibreOffice：自带副本 → PATH / exe 根 → 两个默认安装位置（v1 顺序）。
fn resolve_soffice() -> Option<PathBuf> {
    if let Some(root) = exe_dir() {
        if let Some(p) = bundled_soffice(&root) {
            return Some(p);
        }
    }
    if let Some(p) = resolve_engine("soffice") {
        return Some(p);
    }
    [
        r"C:\Program Files\LibreOffice\program\soffice.exe",
        r"C:\Program Files (x86)\LibreOffice\program\soffice.exe",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.is_file())
}

// ── 计划（`lunac_convert_plan`）────────────────────────────────────

#[derive(Debug, Serialize, PartialEq, Clone)]
pub struct ConvertPlan {
    /// `image` / `audio` / `video` / `pdf` / `unknown`
    pub kind: String,
    /// 该族可选目标格式（已去掉源扩展名本身）。
    pub targets: Vec<String>,
    /// 不支持的后缀等不可转换原因；能转时为空串。
    pub error: String,
    /// 该族要用的引擎：`ffmpeg` / `libreoffice`（unknown 时为空）。
    pub engine: String,
    /// 引擎是否已找到（面板据此禁用按钮并显示装法）。
    #[serde(rename = "engineFound")]
    pub engine_found: bool,
}

/// 按扩展名给一份转换计划（纯格式判断；找不到引擎不算「不能转」，只标 `engineFound=false`）。
pub fn plan(path: &str) -> ConvertPlan {
    let ext = ext_of(path);
    let kind = kind_of(&ext);
    if kind == "unknown" {
        return ConvertPlan {
            kind: kind.to_string(),
            targets: Vec::new(),
            error: UNSUPPORTED.to_string(),
            engine: String::new(),
            engine_found: false,
        };
    }
    let (engine, engine_found) = if kind == "pdf" {
        ("libreoffice", resolve_soffice().is_some())
    } else {
        ("ffmpeg", resolve_engine("ffmpeg").is_some())
    };
    ConvertPlan {
        kind: kind.to_string(),
        targets: targets_for_input(kind, &ext),
        error: String::new(),
        engine: engine.to_string(),
        engine_found,
    }
}

// ── 参数拼装（纯函数，单测直接断言）────────────────────────────────

/// ffmpeg 参数（v1 `convert.rs:547-562`）：
/// `-nostdin` / `-y` / `-hide_banner` / `-loglevel error` / `-progress pipe:1` / `-i src` / dst。
///
/// v2 不解析进度，但仍保留 `-progress pipe:1` 与 v1 同形；子进程 stdout 置空即可。
fn ffmpeg_args(src: &Path, dst: &Path) -> Vec<String> {
    [
        "-nostdin",
        "-y",
        "-hide_banner",
        "-loglevel",
        "error",
        "-progress",
        "pipe:1",
        "-i",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([
        src.to_string_lossy().to_string(),
        dst.to_string_lossy().to_string(),
    ])
    .collect()
}

/// LibreOffice 参数（v1 `convert.rs:465-476`）：独立用户配置 + 无头 + 无恢复 +
/// `--convert-to <target> --outdir <tmp> <src>`。
fn soffice_args(src: &Path, target: &str, out_dir: &Path, profile: &Path) -> Vec<String> {
    let url = format!("file:///{}", profile.to_string_lossy().replace('\\', "/"));
    vec![
        format!("-env:UserInstallation={url}"),
        "--headless".to_string(),
        "--norestore".to_string(),
        "--convert-to".to_string(),
        target.to_string(),
        "--outdir".to_string(),
        out_dir.to_string_lossy().to_string(),
        src.to_string_lossy().to_string(),
    ]
}

// ── 输出路径（v1 `unique_output_in`，`convert.rs:394-405`）─────────

/// 指定目录下的不冲突输出路径：已存在就加 ` (1)` / ` (2)` 递增，最多 999。
fn unique_output_in(dir: &Path, stem: &str, target: &str) -> Result<PathBuf, String> {
    let mut candidate = dir.join(format!("{stem}.{target}"));
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({n}).{target}"));
        n += 1;
        if n > 999 {
            return Err("同名文件太多，换个目录再试".into());
        }
    }
    Ok(candidate)
}

// ── 转换（`lunac_convert_run`）────────────────────────────────────

// ── 进度 / 取消（v1 `convert_progress` / `convert_cancel`）──────────

/// 当前这次转换的可观测状态。
#[derive(Clone)]
struct Running {
    pid: u32,
    dst: PathBuf,
    /// 百分比；`-1` = 不知道总时长（面板显示不确定态）。
    percent: f64,
    total_ms: i64,
}

fn running() -> &'static Mutex<Option<Running>> {
    static S: OnceLock<Mutex<Option<Running>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// 取消标记：`run()` 在子进程失败后据此把错误翻成「已取消」而不是 ffmpeg 的原文。
fn cancel_flag() -> &'static AtomicBool {
    static S: AtomicBool = AtomicBool::new(false);
    &S
}

/// 百分比 = 已处理微秒 / 总毫秒；总时长未知时有意义的是 `-1`（不确定态）。
pub fn percent_from_us(out_time_us: i64, total_ms: i64) -> f64 {
    if total_ms <= 0 || out_time_us < 0 {
        return -1.0;
    }
    ((out_time_us as f64 / 1000.0) / total_ms as f64 * 100.0).clamp(0.0, 99.9)
}

/// 解析 ffprobe `format=duration` 的一行输出（`123.456\r\n`）。读不出返回 0。
pub fn parse_ffprobe_duration(text: &str) -> f64 {
    text.lines()
        .map(str::trim)
        .find_map(|line| line.parse::<f64>().ok())
        .filter(|value| *value > 0.0)
        .unwrap_or(0.0)
}

/// 用 ffprobe 读媒体总时长（秒）。找不到 ffprobe / 读不到都返回 0 —— 此时进度是不确定态。
fn probe_duration_secs(ffmpeg: &Path, src: &Path) -> f64 {
    let probe = resolve_engine("ffprobe").or_else(|| {
        let sibling = ffmpeg.with_file_name("ffprobe.exe");
        sibling.is_file().then_some(sibling)
    });
    let Some(probe) = probe else {
        return 0.0;
    };
    let out = Command::new(probe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(src)
        .stdin(Stdio::null())
        .output();
    match out {
        Ok(out) if out.status.success() => {
            parse_ffprobe_duration(&String::from_utf8_lossy(&out.stdout))
        }
        _ => 0.0,
    }
}

/// 状态快照（Dart 每 300ms 轮询一次）。
pub fn progress_json() -> String {
    let guard = running().lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(item) => serde_json::json!({
            "running": true,
            "percent": item.percent,
            "totalMs": item.total_ms,
            "cancelled": cancel_flag().load(Ordering::SeqCst),
        })
        .to_string(),
        None => serde_json::json!({
            "running": false,
            "percent": 0.0,
            "totalMs": 0,
            "cancelled": cancel_flag().load(Ordering::SeqCst),
        })
        .to_string(),
    }
}

fn kill_pid(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output();
    }
}

/// 取消当前转换：按 pid 结束进程树，并删掉半成品。
///
/// **只对 ffmpeg 生效**（PDF / LibreOffice 走一次性进程，不在这里杀 —— 避免误杀用户
/// 自己开着的 LibreOffice）。没有在跑的转换时是 no-op。
pub fn cancel() -> Result<(), String> {
    let run = {
        let mut guard = running().lock().map_err(|_| "转换状态锁损坏".to_string())?;
        guard.take()
    };
    let Some(run) = run else {
        return Ok(());
    };
    cancel_flag().store(true, Ordering::SeqCst);
    kill_pid(run.pid);
    let _ = std::fs::remove_file(&run.dst);
    Ok(())
}

/// 跑一次转换并返回输出路径。
///
/// `out_dir` 为空 = 源文件同目录（v1 `convert_run` 的默认口径）。
pub fn run(src: &str, target: &str, out_dir: &str) -> Result<String, String> {
    let src_path = PathBuf::from(src.trim());
    if !src_path.is_file() {
        return Err(ERR_NO_FILE.into());
    }
    let target = target.trim().trim_start_matches('.').to_lowercase();
    if target.is_empty() {
        return Err("没有选择目标格式".into());
    }
    let src_ext = ext_of(&src_path.to_string_lossy());
    let kind = kind_of(&src_ext);
    let stem = src_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or_else(|| "源文件没有名字".to_string())?;

    let out_dir = match out_dir.trim() {
        "" => src_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        dir => PathBuf::from(dir),
    };
    if !out_dir.is_dir() {
        return Err(ERR_OUT_DIR.into());
    }

    if kind == "pdf" {
        return run_pdf(&src_path, &target, &out_dir, &stem);
    }
    if !targets_for_input(kind, &src_ext)
        .iter()
        .any(|t| *t == target)
    {
        return Err(format!("不支持 {src_ext} → {target}"));
    }
    let Some(ffmpeg) = resolve_engine("ffmpeg") else {
        return Err(missing_engine_message(kind).into());
    };

    let dst = unique_output_in(&out_dir, &stem, &target)?;
    // 总时长只用于算百分比；ffprobe 不在时 percent 恒 -1（面板显示不确定态）。
    let total_ms = (probe_duration_secs(&ffmpeg, &src_path) * 1000.0).round() as i64;
    cancel_flag().store(false, Ordering::SeqCst);
    let mut child = Command::new(&ffmpeg)
        .args(ffmpeg_args(&src_path, &dst))
        .stdin(Stdio::null())
        // `-progress pipe:1` 读出来算百分比；读完即丢（见下面的读线程）。
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 ffmpeg 失败：{e}"))?;
    // 与 v1 同口径：转换进程绑到宿主生命周期上（宿主崩溃时由内核收掉）。
    crate::child_job::assign(&child);
    let pid = child.id();
    {
        let mut guard = running().lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(Running {
            pid,
            dst: dst.clone(),
            percent: if total_ms > 0 { 0.0 } else { -1.0 },
            total_ms,
        });
    }

    // 进度：只认 `out_time_us`（ffmpeg 8 的 out_time_ms 其实也是微秒）。
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Some(text) = line.strip_prefix("out_time_us=") else {
                    continue;
                };
                let Ok(us) = text.trim().parse::<i64>() else {
                    continue;
                };
                let mut guard = running().lock().unwrap_or_else(|e| e.into_inner());
                if let Some(item) = guard.as_mut() {
                    if item.pid == pid {
                        item.percent = percent_from_us(us, item.total_ms);
                    }
                }
            }
        });
    }

    // stderr 必须在独立线程里排空：ffmpeg 报错刷屏时管道写满会与 `wait()` 死锁。
    let stderr = child.stderr.take();
    let err_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(mut e) = stderr {
            let _ = e.read_to_string(&mut buf);
        }
        buf
    });

    let status = child.wait().map_err(|e| format!("等待 ffmpeg 失败：{e}"))?;
    // 清掉运行态（取消时已经 take 过，这里可能是 None）。
    {
        let mut guard = running().lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().map(|item| item.pid) == Some(pid) {
            *guard = None;
        }
    }
    let err_text = err_handle.join().unwrap_or_default();
    if !status.success() {
        // 失败要把半成品删掉，别在用户目录里留一个「转坏了的文件」（v1 同口径）。
        let _ = std::fs::remove_file(&dst);
        if cancel_flag().load(Ordering::SeqCst) {
            return Err("已取消转换".into());
        }
        let detail = err_text.trim();
        let detail = if detail.is_empty() {
            format!("ffmpeg 退出码 {:?}", status.code())
        } else {
            detail.chars().take(500).collect::<String>()
        };
        return Err(format!("转换失败：{detail}"));
    }
    Ok(dst.to_string_lossy().to_string())
}

/// PDF → Word / PPT / Excel / 图片：LibreOffice 无头模式（v1 `convert.rs:445-525`）。
///
/// 先转到临时目录再改名：LibreOffice 会按源主名直接覆盖目标目录里的同名文件。
fn run_pdf(src: &Path, target: &str, out_dir: &Path, stem: &str) -> Result<String, String> {
    if !PDF_TARGETS.contains(&target) {
        return Err(format!("不支持 pdf → {target}"));
    }
    let Some(soffice) = resolve_soffice() else {
        return Err(ERR_NO_PDF_ENGINE.into());
    };
    let tmp = std::env::temp_dir().join(format!(
        "lunac-convert-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("建临时目录失败：{e}"))?;
    let profile = tmp.join("profile");
    let out = Command::new(&soffice)
        .args(soffice_args(src, target, &tmp, &profile))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("启动 LibreOffice 失败：{e}"))?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr)
            .trim()
            .chars()
            .take(500)
            .collect::<String>();
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(if detail.is_empty() {
            format!("转换失败：LibreOffice 退出码 {:?}", out.status.code())
        } else {
            format!("转换失败：{detail}")
        });
    }
    let produced = std::fs::read_dir(&tmp)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file() && ext_of(&p.to_string_lossy()) == target);
    let Some(produced) = produced else {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("转换失败：LibreOffice 没有产出文件（PDF 转图片只导出第一页）".into());
    };
    let dst = unique_output_in(out_dir, stem, target)?;
    let moved = std::fs::rename(&produced, &dst).or_else(|_| std::fs::copy(&produced, &dst).map(|_| ()));
    let _ = std::fs::remove_dir_all(&tmp);
    moved.map_err(|e| format!("写入输出文件失败：{e}"))?;
    Ok(dst.to_string_lossy().to_string())
}

// ── 文件 / 目录选择（v1 用 Tauri 的 dialog 插件；v2 手写 Win32）────────
//
// 与 `pet.rs::pick_file` 同一条路（GetOpenFileNameW），区别是这里要**多选**，
// 并且按页签过滤（PDF / 视频 / 音频）。目录选择用 SHBrowseForFolderW。

/// 按页签弹多选文件框，返回选中路径（取消 = 空数组）。
///
/// `kind` 与 v1 的四个页签对应：`pdf` / `video` / `extract`（视频源） / `audio`。
/// 不认识的 kind 退回「所有支持的类型」。
pub fn pick_inputs(kind: &str) -> Vec<String> {
    #[cfg(windows)]
    unsafe {
        use windows::core::{PCWSTR, PWSTR};
        use windows::Win32::UI::Controls::Dialogs::{
            GetOpenFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST,
            OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
        };

        let filter_text = match kind {
            "pdf" => "PDF (*.pdf)\0*.pdf\0所有文件 (*.*)\0*.*\0\0".to_string(),
            "video" | "extract" => "视频 (*.mp4;*.mkv;*.webm;*.avi;*.mov;*.wmv;*.flv;*.m4v)\0*.mp4;*.mkv;*.webm;*.avi;*.mov;*.wmv;*.flv;*.m4v\0所有文件 (*.*)\0*.*\0\0".to_string(),
            "audio" => "音频 (*.mp3;*.wav;*.flac;*.m4a;*.aac;*.ogg;*.opus;*.wma)\0*.mp3;*.wav;*.flac;*.m4a;*.aac;*.ogg;*.opus;*.wma\0所有文件 (*.*)\0*.*\0\0".to_string(),
            "image" => "图片 (*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.gif;*.tif;*.tiff)\0*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.gif;*.tif;*.tiff\0所有文件 (*.*)\0*.*\0\0".to_string(),
            // 「所有文件」优先（快捷启动的「添加启动项」：可启动任何文件）。
            "all" => "所有文件 (*.*)\0*.*\0\0".to_string(),
            _ => "支持的文件\0*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.gif;*.mp4;*.mkv;*.mov;*.mp3;*.wav;*.flac;*.pdf\0所有文件 (*.*)\0*.*\0\0".to_string(),
        };
        let filter: Vec<u16> = filter_text.encode_utf16().collect();
        // 多选时系统把「目录 + 各文件名」写进同一个缓冲区，给足 32k。
        let mut buffer = vec![0u16; 32 * 1024];
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: PWSTR(buffer.as_mut_ptr()),
            nMaxFile: buffer.len() as u32,
            Flags: OFN_FILEMUSTEXIST
                | OFN_PATHMUSTEXIST
                | OFN_NOCHANGEDIR
                | OFN_ALLOWMULTISELECT
                | OFN_EXPLORER,
            ..Default::default()
        };
        if !GetOpenFileNameW(&mut ofn).as_bool() {
            return Vec::new();
        }
        // 单选取整串；多选是 `目录\0文件1\0文件2\0\0`。
        let mut parts: Vec<String> = Vec::new();
        let mut start = 0usize;
        for index in 0..buffer.len() {
            if buffer[index] != 0 {
                continue;
            }
            if index == start {
                break; // 连续两个 0 = 结束
            }
            parts.push(String::from_utf16_lossy(&buffer[start..index]));
            start = index + 1;
        }
        if parts.len() == 1 {
            return parts;
        }
        let Some(dir) = parts.first().cloned() else {
            return Vec::new();
        };
        parts
            .into_iter()
            .skip(1)
            .map(|name| format!("{dir}\\{name}"))
            .collect()
    }
    #[cfg(not(windows))]
    {
        let _ = kind;
        Vec::new()
    }
}

/// 弹系统的「选择文件夹」对话框（转换保存目录）；取消 / 失败返回空串。
pub fn pick_out_dir() -> String {
    pick_dir_with_title("选择转换结果的保存目录")
}

/// 弹系统的「选择文件夹」对话框，标题由调用方给；取消 / 失败返回空串。
///
/// 这里**手写 shell32 的 ABI**而不是开 `windows` crate 的 `Win32_UI_Shell_Common`
/// feature：那个 feature 会让 `windows` crate 生成更多 import lib（本机没有 dlltool，
/// 构建会直接失败）。结构体布局与 `BROWSEINFOW` 一致（x64 上 `ulFlags` 后自带 4 字节对齐）。
pub fn pick_dir_with_title(title_text: &str) -> String {
    #[cfg(windows)]
    unsafe {
        use std::ffi::c_void;

        #[repr(C)]
        struct BrowseInfoW {
            hwnd_owner: *mut c_void,
            pidl_root: *const c_void,
            display_name: *mut u16,
            title: *const u16,
            flags: u32,
            callback: Option<
                unsafe extern "system" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> i32,
            >,
            lparam: isize,
            image: i32,
        }

        #[link(name = "shell32")]
        extern "system" {
            fn SHBrowseForFolderW(info: *const BrowseInfoW) -> *mut c_void;
            fn SHGetPathFromIDListW(pidl: *const c_void, path: *mut u16) -> i32;
        }
        #[link(name = "ole32")]
        extern "system" {
            fn CoTaskMemFree(pv: *const c_void);
        }

        const BIF_RETURNONLYFSDIRS: u32 = 0x0000_0001;
        const BIF_NEWDIALOGSTYLE: u32 = 0x0000_0040;
        let title: Vec<u16> = format!("{title_text}\0").encode_utf16().collect();
        let mut display = vec![0u16; 260];
        let info = BrowseInfoW {
            hwnd_owner: std::ptr::null_mut(),
            pidl_root: std::ptr::null(),
            display_name: display.as_mut_ptr(),
            title: title.as_ptr(),
            flags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
            callback: None,
            lparam: 0,
            image: 0,
        };
        let pidl = SHBrowseForFolderW(&info);
        if pidl.is_null() {
            return String::new();
        }
        let mut path = [0u16; 260];
        let ok = SHGetPathFromIDListW(pidl, path.as_mut_ptr()) != 0;
        CoTaskMemFree(pidl);
        if !ok {
            return String::new();
        }
        let len = path.iter().position(|c| *c == 0).unwrap_or(0);
        String::from_utf16_lossy(&path[..len])
    }
    #[cfg(not(windows))]
    {
        let _ = title_text;
        String::new()
    }
}

// ── 单元测试（不启动 ffmpeg / LibreOffice）─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_table_covers_the_four_families() {
        assert_eq!(kind_of("png"), "image");
        assert_eq!(kind_of("jpeg"), "image");
        assert_eq!(kind_of("avif"), "image");
        assert_eq!(kind_of("mp3"), "audio");
        assert_eq!(kind_of("opus"), "audio");
        assert_eq!(kind_of("mp4"), "video");
        assert_eq!(kind_of("rmvb"), "video");
        assert_eq!(kind_of("pdf"), "pdf");
        assert_eq!(kind_of("exe"), "unknown");
        assert_eq!(kind_of(""), "unknown");

        // 来源：v1 `ext_of` 大小写不敏感、目录里的点不能判错。
        assert_eq!(ext_of("D:\\a\\B.MP4"), "mp4");
        assert_eq!(ext_of("D:\\a.b\\c"), "");
        assert_eq!(ext_of("no-extension"), "");
    }

    #[test]
    fn targets_follow_the_family_and_drop_the_source_ext() {
        // 图片：png 源不能再列 png，其余目标保持 v1 顺序。
        assert_eq!(
            targets_for_input("image", "png"),
            vec!["jpg", "webp", "bmp", "tiff", "gif"]
        );
        // 音频：wav 源去掉 wav。
        let audio = targets_for_input("audio", "wav");
        assert!(audio.contains(&"mp3".to_string()));
        assert!(!audio.contains(&"wav".to_string()));
        // 视频：既给视频目标，也给音频目标（抽音轨）。
        let video = targets_for_input("video", "mp4");
        assert!(video.contains(&"webm".to_string()));
        assert!(video.contains(&"mp3".to_string()), "视频必须能抽音轨");
        assert!(!video.contains(&"mp4".to_string()));
        // PDF：目标清单与 v1 一致。
        assert_eq!(
            targets_for_input("pdf", "pdf"),
            vec!["docx", "pptx", "xlsx", "png"]
        );
        assert!(targets_for_input("unknown", "exe").is_empty());
    }

    #[test]
    fn unsupported_extension_has_no_targets_and_a_reason() {
        let unsupported = plan("D:\\notes\\readme.txt");
        assert_eq!(unsupported.kind, "unknown");
        assert!(unsupported.targets.is_empty());
        assert_eq!(unsupported.error, UNSUPPORTED);
        assert!(unsupported.engine.is_empty());
        assert!(!unsupported.engine_found);

        let ok = plan("D:\\media\\clip.MP4");
        assert_eq!(ok.kind, "video");
        assert!(ok.error.is_empty());
        assert_eq!(ok.engine, "ffmpeg");
        assert!(!ok.targets.contains(&"mp4".to_string()), "源扩展名要摘掉");
        assert_eq!(plan("D:\\docs\\a.pdf").engine, "libreoffice");
    }

    #[test]
    fn image_args_are_the_v1_ffmpeg_shape() {
        let args = ffmpeg_args(Path::new(r"D:\pics\a.png"), Path::new(r"D:\pics\a.jpg"));
        assert_eq!(
            args[..8].iter().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "-nostdin",
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-progress",
                "pipe:1",
                "-i",
            ]
        );
        assert_eq!(args[8], r"D:\pics\a.png");
        assert_eq!(args[9], r"D:\pics\a.jpg");
        assert_eq!(args.len(), 10);
    }

    #[test]
    fn audio_args_use_the_same_shape() {
        let args = ffmpeg_args(Path::new(r"D:\music\a.wav"), Path::new(r"D:\music\a.mp3"));
        assert_eq!(args[7], "-i");
        assert_eq!(args[8], r"D:\music\a.wav");
        assert_eq!(args[9], r"D:\music\a.mp3");
        assert_eq!(args.len(), 10);
    }

    #[test]
    fn video_args_use_the_same_shape() {
        let args = ffmpeg_args(Path::new(r"D:\movies\a.mp4"), Path::new(r"D:\movies\a.webm"));
        assert_eq!(args[7], "-i");
        assert_eq!(args[8], r"D:\movies\a.mp4");
        assert_eq!(args[9], r"D:\movies\a.webm");
        assert_eq!(args.len(), 10);
    }

    #[test]
    fn pdf_args_call_libreoffice_headless() {
        let args = soffice_args(
            Path::new(r"D:\docs\a.pdf"),
            "docx",
            Path::new(r"C:\Temp\lunac-convert-1"),
            Path::new(r"C:\Temp\lunac-convert-1\profile"),
        );
        assert_eq!(
            args[0],
            r"-env:UserInstallation=file:///C:/Temp/lunac-convert-1/profile"
        );
        assert_eq!(
            args[1..4].iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["--headless", "--norestore", "--convert-to"]
        );
        assert_eq!(args[4], "docx");
        assert_eq!(args[5], "--outdir");
        assert_eq!(args[6], r"C:\Temp\lunac-convert-1");
        assert_eq!(args[7], r"D:\docs\a.pdf");
        assert_eq!(args.len(), 8);
    }

    #[test]
    fn engine_missing_messages_match_v1() {
        assert_eq!(missing_engine_message("pdf"), ERR_NO_PDF_ENGINE);
        assert_eq!(missing_engine_message("image"), ERR_NO_ENGINE);
        assert_eq!(missing_engine_message("audio"), ERR_NO_ENGINE);
        assert_eq!(missing_engine_message("video"), ERR_NO_ENGINE);
        assert!(ERR_NO_ENGINE.contains("ffmpeg"));
        assert!(ERR_NO_PDF_ENGINE.contains("LibreOffice"));
    }

    #[test]
    fn percent_is_clamped_and_unknown_duration_reports_minus_one() {
        // 10s 总长、已处理 5s ⇒ 50%。
        assert!((percent_from_us(5_000_000, 10_000) - 50.0).abs() < 0.001);
        // 不会报到 100（收尾由面板的完成态表达）。
        assert_eq!(percent_from_us(99_000_000, 10_000), 99.9);
        // 负值 / 总时长未知 ⇒ 不确定态。
        assert_eq!(percent_from_us(-1, 10_000), -1.0);
        assert_eq!(percent_from_us(1_000, 0), -1.0);
    }

    #[test]
    fn ffprobe_duration_parses_numbers_and_tolerates_junk() {
        assert!((parse_ffprobe_duration("123.456\r\n") - 123.456).abs() < 0.001);
        assert_eq!(parse_ffprobe_duration("N/A\n"), 0.0);
        assert_eq!(parse_ffprobe_duration(""), 0.0);
        assert_eq!(parse_ffprobe_duration("0\n"), 0.0);
    }

    #[test]
    fn progress_is_idle_by_default_and_cancel_is_a_noop() {
        let value: serde_json::Value = serde_json::from_str(&progress_json()).unwrap();
        assert_eq!(value["running"], false);
        // 没有在跑的转换时取消不报错（面板可能在收尾瞬间点一下）。
        assert!(cancel().is_ok());
        // 清掉可能残留的取消标记，避免影响同进程里的其它用例。
        cancel_flag().store(false, Ordering::SeqCst);
    }
}
