// src-tauri/src/convert.rs
// 文件转换插件（图片 / 音频 / 视频互转）的宿主命令 —— 2026-09-27
//
// **为什么必须走宿主**：前端插件跑在 WebView 里，**没有执行外部进程的能力**，
// 而格式转换必须调 ffmpeg。这与 music.rs / 插件市场索引「联网由宿主做」是同一条纪律
// （ai-spec §11 规则 67 第 ⑥ 条、code-rules 预检 #36）。
//
// **一个引擎覆盖三类**：ffmpeg 同时能读写 图片（png/jpg/webp/bmp/tiff/gif）、
// 音频（mp3/wav/flac/m4a/aac/ogg/opus）、视频（mp4/mkv/webm/avi/mov/gif），
// 所以不引第二套实现（曾考虑用 `image` crate 单做图片，但那样就有两条路径、
// 两套错误语义，且 image 的 webp **只有解码**）。实测本机 ffmpeg 8.1.1 full build
// 的编码器含 libwebp / png / tiff / gif / mjpeg / libx264 / libx265 / libvpx-vp9 /
// aac / flac / libmp3lame，够用。
//
// **ffmpeg 不随包分发**（体积 + 许可）：找不到就在面板上如实说「未找到 ffmpeg」
// 并给出装法 —— 与 OCR 引擎缺失、插件市场拉不到索引是同一条「坏状态要可见」的纪律。
//
// **阻塞纪律（code-rules 预检 #35）**：转换是分钟级的子进程等待 ⇒ 一律 `async` 薄壳 +
// `run_blocking`，绝不在主线程上等。

use crate::commands::run_blocking;
use serde::Serialize;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tauri::{AppHandle, Emitter};

// ── 格式表 ────────────────────────────────────────────────────────
//
// 判断只看扩展名（不看魔数）：这里的目标是「用户点了一个 .mp4，给他一个格式候选列表」，
// 而 ffmpeg 自己会在解不开时明确报错（错误信息原样回给面板）。按魔数判类型是**接收方**
// 才需要的纪律（见 ai-spec §11 规则 60 对图片附件的口径），不适用于本场景。

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "jfif", "webp", "bmp", "tif", "tiff", "gif", "ico", "heic", "avif"];
const AUDIO_EXTS: &[&str] = &["mp3", "wav", "flac", "m4a", "aac", "ogg", "opus", "wma", "aiff", "ape", "amr"];
const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "webm", "avi", "mov", "wmv", "flv", "m4v", "mpg", "mpeg", "ts", "3gp", "rmvb"];

/// 可选的输出格式（刻意只列 ffmpeg 稳的：不加 avif / heic 这类要看构建的编码器，
/// 也不加 ico —— ffmpeg 写 ico 会走 mjpeg 且尺寸受限，容易给用户一个「转出来是坏的」）。
const IMAGE_TARGETS: &[&str] = &["png", "jpg", "webp", "bmp", "tiff", "gif"];
const AUDIO_TARGETS: &[&str] = &["mp3", "wav", "flac", "m4a", "aac", "ogg", "opus"];
const VIDEO_TARGETS: &[&str] = &["mp4", "mkv", "webm", "avi", "mov", "gif"];

fn ext_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn kind_of(ext: &str) -> &'static str {
    if IMAGE_EXTS.contains(&ext) {
        "image"
    } else if AUDIO_EXTS.contains(&ext) {
        "audio"
    } else if VIDEO_EXTS.contains(&ext) {
        "video"
    } else {
        "unknown"
    }
}

/// 该输入能转成什么。**视频额外给音频目标** —— 「从视频里抽音轨」是最常用的一类转换，
/// 让用户先转成 mp4 再来一次是多余的两步。
fn targets_for(kind: &str) -> Vec<String> {
    let list: &[&str] = match kind {
        "image" => IMAGE_TARGETS,
        "audio" => AUDIO_TARGETS,
        "video" => VIDEO_TARGETS,
        _ => &[],
    };
    let mut out: Vec<String> = list.iter().map(|s| s.to_string()).collect();
    if kind == "video" {
        out.extend(AUDIO_TARGETS.iter().map(|s| s.to_string()));
    }
    out
}

/// 该输入**实际**能转成什么（已去掉源扩展名本身）。`has_audio` 只对视频有意义：
/// 无音轨的视频（录屏、静音素材）抽不出音轨，提前摘掉音频目标 —— 给用户一个
/// 点了必然失败的候选，比不给更差（实测无音轨 mp4 → mp3 报的是
/// `Output file does not contain any stream`，用户看不懂）。
fn targets_for_input(kind: &str, ext: &str, has_audio: bool) -> Vec<String> {
    let mut out = targets_for(kind);
    out.retain(|t| t != ext);
    if kind == "video" && !has_audio {
        out.retain(|t| !AUDIO_TARGETS.contains(&t.as_str()));
    }
    out
}

// ── 引擎查找 ──────────────────────────────────────────────────────

/// 找 ffmpeg / ffprobe。顺序：**exe 根自带的副本 → PATH**（WinGet Links 就在 PATH 上）。
/// 只找、不下载。
fn resolve_engine(name: &str) -> Option<PathBuf> {
    let root = crate::storage::lunac_root_dir();
    let exe = format!("{name}.exe");
    let local = [
        root.join(&exe),
        root.join("ffmpeg").join("bin").join(&exe),
    ];
    for cand in local {
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
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}

#[derive(Debug, Serialize)]
pub struct EngineDto {
    pub found: bool,
    pub path: String,
    /// 形如 `8.1.1`；空 = 没探到（不影响能不能用）。
    pub version: String,
}

#[tauri::command]
pub async fn convert_engine_status() -> Result<EngineDto, String> {
    run_blocking(|| {
        let Some(ffmpeg) = resolve_engine("ffmpeg") else {
            return Ok(EngineDto {
                found: false,
                path: String::new(),
                version: String::new(),
            });
        };
        // `-version` 的第一行是 `ffmpeg version 8.1.1-full_build-... `，取第二个词
        let version = Command::new(&ffmpeg)
            .arg("-version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(2))
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();
        Ok(EngineDto {
            found: true,
            path: ffmpeg.to_string_lossy().to_string(),
            version,
        })
    })
    .await
}

// ── 输入探测 ──────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct MediaDto {
    pub path: String,
    pub name: String,
    /// `image` / `audio` / `video` / `unknown`
    pub kind: String,
    pub ext: String,
    pub size: i64,
    /// 秒；0 = 未知（图片没有这个概念，探测失败也是 0）。
    pub duration: f64,
    /// 可用的目标扩展名（不含源扩展名本身）。空 = 这个后缀不支持转换。
    pub targets: Vec<String>,
}

/// 一次 ffprobe 同时拿「秒数」与「有没有音轨」——两件事本来就出自同一条探测，
/// 分两次起进程没必要。返回 `(秒, 有音轨)`；**探测失败一律 `(0.0, true)`**：
/// 拦不住就交给 ffmpeg 如实报错，也好过因为探测失败而凭空砍掉候选。
fn probe_media(ffprobe: &Path, input: &Path) -> (f64, bool) {
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type",
            "-of",
            "default=nw=1",
        ])
        .arg(input)
        .output();
    let Ok(out) = out else { return (0.0, true) };
    if !out.status.success() {
        return (0.0, true);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (mut duration, mut has_audio, mut saw_any) = (0.0f64, false, false);
    for line in text.lines().map(str::trim) {
        if let Some(v) = line.strip_prefix("duration=") {
            saw_any = true;
            duration = v.parse().unwrap_or(0.0);
        } else if let Some(v) = line.strip_prefix("codec_type=") {
            saw_any = true;
            if v == "audio" {
                has_audio = true;
            }
        }
    }
    if saw_any {
        (duration, has_audio)
    } else {
        (0.0, true)
    }
}

/// 找 ffprobe 再探（`(秒, 有音轨)`）。图片不用探 —— 它既没时长也没音轨。
fn probe_input(input: &Path) -> (f64, bool) {
    resolve_engine("ffprobe")
        .map(|ff| probe_media(&ff, input))
        .unwrap_or((0.0, true))
}

#[tauri::command]
pub async fn convert_probe(path: String) -> Result<MediaDto, String> {
    run_blocking(move || {
        let p = PathBuf::from(path.trim());
        if !p.is_file() {
            return Err("ERR_NO_FILE".into());
        }
        let ext = ext_of(&p.to_string_lossy());
        let kind = kind_of(&ext).to_string();
        let size = std::fs::metadata(&p).map(|m| m.len() as i64).unwrap_or(0);
        let (duration, has_audio) = if kind == "image" || kind == "unknown" {
            (0.0, true)
        } else {
            probe_input(&p)
        };
        let targets = targets_for_input(&kind, &ext, has_audio);
        Ok(MediaDto {
            path: p.to_string_lossy().to_string(),
            name: p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            kind,
            ext,
            size,
            duration,
            targets,
        })
    })
    .await
}

// ── 转换 ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct ConvertProgress {
    /// 0–100；**-1 = 时长未知**（面板改成不确定态的进度条，不要显示假的百分比）。
    pub percent: f64,
    pub total_ms: i64,
}

#[derive(Debug, Serialize)]
pub struct ConvertResult {
    pub output: String,
    pub size: i64,
    pub elapsed_ms: i64,
}

/// 输出路径：源文件同目录、同主名、换扩展名。
///
/// **三条硬约束**：① 绝不等于源文件（否则就是覆盖用户的原始素材）；
/// ② 已存在就加 ` (1)` / ` (2)` 递增 —— 静默覆盖是最不该发生的事；
/// ③ 路径由**宿主**算，前端只传目标扩展名（别让前端决定写哪儿）。
fn unique_output(input: &Path, target: &str) -> Result<PathBuf, String> {
    let dir = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or_else(|| "源文件没有名字".to_string())?;
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

/// 跑一次转换并返回输出路径。进度经 `convert-progress` 事件回传。
///
/// ffmpeg 参数口径：`-nostdin`（别去抢 stdin）、`-y`（输出路径已由我们算成不存在的，
/// `-y` 只是为了兜住「算完到开跑之间被别人建了」这一瞬间）、`-loglevel error`
/// （stderr 只留错误 ⇒ 既能原样回报失败原因，又不会把日志刷爆）、
/// `-progress pipe:1`（结构化进度走 stdout，实测字段 `out_time_us` 是**微秒**，
/// 同行的 `out_time_ms` 在 ffmpeg 8 里同样是微秒 —— 所以只认 `out_time_us`）。
#[tauri::command]
pub async fn convert_run(app: AppHandle, input: String, target: String) -> Result<ConvertResult, String> {
    run_blocking(move || {
        let started = std::time::Instant::now();
        let src = PathBuf::from(input.trim());
        if !src.is_file() {
            return Err("ERR_NO_FILE".into());
        }
        let target = target.trim().trim_start_matches('.').to_lowercase();
        let src_ext = ext_of(&src.to_string_lossy());
        let kind = kind_of(&src_ext);
        // 先探再校验：无音轨的视频抽音轨这一条只有 probe 之后才知道，放在这里
        // 才能给出「不支持 mp4 → mp3」这种能看懂的拒绝，而不是 ffmpeg 的报错原文。
        let (total_secs, has_audio) = if kind == "image" || kind == "unknown" {
            (0.0, true)
        } else {
            probe_input(&src)
        };
        if !targets_for_input(kind, &src_ext, has_audio).iter().any(|t| *t == target) {
            return Err(format!("不支持 {src_ext} → {target}"));
        }
        let Some(ffmpeg) = resolve_engine("ffmpeg") else {
            return Err("ERR_NO_ENGINE".into());
        };

        let dst = unique_output(&src, &target)?;
        let total_ms = (total_secs * 1000.0).round() as i64;

        let mut child = Command::new(&ffmpeg)
            .arg("-nostdin")
            .arg("-y")
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-progress")
            .arg("pipe:1")
            .arg("-i")
            .arg(&src)
            .arg(&dst)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("启动 ffmpeg 失败：{e}"))?;

        // stderr 必须在**独立线程**里排空：两个管道都塞满时（ffmpeg 报错刷屏）
        // 主线程读 stdout、stderr 无人读 ⇒ 子进程写阻塞 ⇒ 双向死锁。
        let stderr = child.stderr.take();
        let err_handle = std::thread::spawn(move || {
            let mut buf = String::new();
            if let Some(mut e) = stderr {
                let _ = e.read_to_string(&mut buf);
            }
            buf
        });

        if let Some(stdout) = child.stdout.take() {
            let mut last_emit = 0.0f64;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Some(us) = line.strip_prefix("out_time_us=") else { continue };
                let Ok(us) = us.trim().parse::<i64>() else { continue };
                let percent = if total_ms > 0 {
                    ((us as f64 / 1000.0) / total_ms as f64 * 100.0).clamp(0.0, 99.9)
                } else {
                    -1.0
                };
                // 节流：ffmpeg 每 ~0.5s 一行，够用；这里再挡一次 1% 内的重复
                if (percent - last_emit).abs() < 1.0 && percent >= 0.0 {
                    continue;
                }
                last_emit = percent;
                let _ = app.emit("convert-progress", ConvertProgress { percent, total_ms });
            }
        }

        let status = child.wait().map_err(|e| format!("等待 ffmpeg 失败：{e}"))?;
        let err_text = err_handle.join().unwrap_or_default();
        if !status.success() {
            // 失败要把半成品删掉，别在用户目录里留一个「转坏了的文件」
            let _ = std::fs::remove_file(&dst);
            let detail = err_text.trim();
            let detail = if detail.is_empty() {
                format!("ffmpeg 退出码 {:?}", status.code())
            } else {
                detail.chars().take(500).collect::<String>()
            };
            crate::log::warn(&format!("convert: {src_ext} → {target} 失败：{detail}"));
            return Err(format!("转换失败：{detail}"));
        }

        let size = std::fs::metadata(&dst).map(|m| m.len() as i64).unwrap_or(0);
        let elapsed_ms = started.elapsed().as_millis() as i64;
        let _ = app.emit(
            "convert-progress",
            ConvertProgress {
                percent: 100.0,
                total_ms,
            },
        );
        crate::log::info(&format!(
            "convert: {} → {}（{} 字节，{}ms）",
            src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            dst.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            size,
            elapsed_ms
        ));
        Ok(ConvertResult {
            output: dst.to_string_lossy().to_string(),
            size,
            elapsed_ms,
        })
    })
    .await
}

// ── 单元测试 ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_table_covers_the_three_families() {
        assert_eq!(kind_of("png"), "image");
        assert_eq!(kind_of("jpeg"), "image");
        assert_eq!(kind_of("webp"), "image");
        assert_eq!(kind_of("mp3"), "audio");
        assert_eq!(kind_of("flac"), "audio");
        assert_eq!(kind_of("mp4"), "video");
        assert_eq!(kind_of("mkv"), "video");
        assert_eq!(kind_of("exe"), "unknown");
        assert_eq!(kind_of(""), "unknown");
    }

    #[test]
    fn video_targets_include_audio_extraction() {
        let v = targets_for("video");
        assert!(v.iter().any(|t| t == "mp4"));
        assert!(v.iter().any(|t| t == "mp3"), "视频必须能抽音轨");
        // 音频不该反过来给视频目标
        let a = targets_for("audio");
        assert!(!a.iter().any(|t| t == "mp4"));
        assert!(targets_for("unknown").is_empty());
    }

    #[test]
    fn silent_video_loses_audio_targets() {
        let with_audio = targets_for_input("video", "mp4", true);
        assert!(with_audio.iter().any(|t| t == "mp3"));
        let silent = targets_for_input("video", "mp4", false);
        assert!(!silent.iter().any(|t| t == "mp3"), "无音轨的视频不该列音频目标");
        assert!(silent.iter().any(|t| t == "webm"), "摘音频目标不该动视频目标");
        // has_audio 只对视频有意义：纯音频文件的候选不受它影响
        assert!(targets_for_input("audio", "wav", false).iter().any(|t| t == "mp3"));
        // 源扩展名本身要被摘掉
        assert!(!targets_for_input("audio", "wav", true).iter().any(|t| t == "wav"));
    }

    #[test]
    fn ext_of_is_case_insensitive_and_dotless() {
        assert_eq!(ext_of("D:\\a\\B.MP4"), "mp4");
        assert_eq!(ext_of("no-extension"), "");
        // 目录里的点不能让扩展名判错
        assert_eq!(ext_of("D:\\a.b\\c"), "");
    }

    #[test]
    fn unique_output_never_collides_or_overwrites() {
        let dir = std::env::temp_dir().join(format!("lunac-conv-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("clip.mp4");
        std::fs::write(&src, b"x").unwrap();

        // ① 目标扩展名与源不同 ⇒ 直接用 `clip.webm`
        let first = unique_output(&src, "webm").unwrap();
        assert_eq!(first.file_name().unwrap().to_string_lossy(), "clip.webm");

        // ② 已存在 ⇒ 加 ` (1)`
        std::fs::write(&first, b"y").unwrap();
        let second = unique_output(&src, "webm").unwrap();
        assert_eq!(second.file_name().unwrap().to_string_lossy(), "clip (1).webm");

        // ③ 目标扩展名与源相同（不该发生，但必须保证不覆盖源文件）
        let same = unique_output(&src, "mp4").unwrap();
        assert_ne!(same, src);
        assert_eq!(same.file_name().unwrap().to_string_lossy(), "clip (1).mp4");

        std::fs::remove_dir_all(&dir).ok();
    }
}
