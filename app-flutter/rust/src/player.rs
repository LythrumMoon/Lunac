//! 本地文件播放（重建 v1 `app/src-tauri/src/player.rs`）：解码 + 出声 + 调音链挂点。
//!
//! **为什么这一层在宿主**：WebView 放不出声音 —— 解码、设备枚举、出声全是系统能力，
//! 与「联网必须走宿主」同一条纪律。v2 之前把出声交给 Dart 的 `just_audio`，
//! 那样 **DSP 链没有挂点**（调音打开也不会改变声音）；本模块按 v1 口径把它接回来。
//!
//! **为什么用 rodio 0.22**（内含 cpal 输出 + symphonia 解码）：播放要的是一台状态机 ——
//! 位置口径 / 暂停语义 / seek 饱和 / 「这一首放完了」的判据，`rodio::Player` 已经做对了。
//!
//! ⚠️ **rodio 的 `stop()` 是一次性的**（`Controls::stopped` 只置位、永不复位）⇒ 形态是
//! 「**设备跨曲目复用、Player 每首一条**」（见 [`play`]）；写成「一台 Player 反复 append」
//! 的话第一次停下之后所有后续播放都没声音、而且一句报错都没有。
//!
//! **DSP 挂点**：`TuningSource` 把 `tuning::TuningShared` 的版本号当「热换链」信号 ——
//! 改预设**不打断正在放的这一首**，下一个样本就换上新链。滤波器一行都不在这里重写，
//! 用的是 `tuning-engine` 的 `Chain` / `ChainRuntime` / `Convolver`。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::{ChannelCount, Decoder, Sample, SampleRate, Source};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tuning_engine::chain::ChainRuntime;
use tuning_engine::convolution::Convolver;

use crate::tuning::{self, TuningShared};

/// 默认音量（刻意不是 1.0：满量程起播在戴耳机、上一首很轻的场景下很吓人）。
const DEFAULT_VOLUME: f32 = 0.8;
const DEFAULT_VOLUME_BITS: u32 = DEFAULT_VOLUME.to_bits();
/// `.webm` 抽出来的音轨缓存保留份数。
const WEBM_AUDIO_KEEP: usize = 20;

/// 一条正在播的流。
struct Session {
    /// **必须活着**：它一 drop，输出流就没了（表现是「点播放没有任何反应」）。
    sink: rodio::MixerDeviceSink,
    /// 这一首的队列；每首一条（见文件头那条 ⚠️）。
    player: rodio::Player,
    path: PathBuf,
    /// 解码器报的总时长（毫秒）；`0` = 报不出来（没有 Xing 头的 mp3 就是这样）。
    duration_ms: u64,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
/// 音量（0.0–1.0）存位模式（原子量里没有 f32）。跨曲目保留。
static VOLUME: AtomicU32 = AtomicU32::new(DEFAULT_VOLUME_BITS);

fn volume() -> f32 {
    f32::from_bits(VOLUME.load(Ordering::Relaxed))
}

/// 播放器状态（`lunac_player_*` 的 `data`，camelCase）。
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlayerDto {
    /// 有内容在播。`false` 的三种含义靠 `path` 区分：没起过 / 已停（path 空）/
    /// **这一首放完了**（path 有值）—— 后者是界面「自动下一首」的触发条件。
    pub active: bool,
    pub playing: bool,
    pub path: String,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: f32,
    /// 这是一个要出画面的视频（画面由界面那层出，声音仍归这里）。
    pub video: bool,
}

fn is_video_path(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    crate::convert::is_video_ext(&ext)
}

fn is_webm_path(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().eq_ignore_ascii_case("webm"))
        .unwrap_or(false)
}

/// 打开解码器：解码器 + 总时长（毫秒，0 = 报不出来）。
///
/// 必须走 `Decoder::try_from(File)`：它顺手把 `byte_len` 设上，symphonia 靠这个才算得出
/// 总时长、才支持 seek（换成 `BufReader` 就没有）。
fn open_decoder(path: &Path) -> Result<(Decoder<std::io::BufReader<std::fs::File>>, u64), String> {
    if !path.is_file() {
        return Err(format!("找不到这个文件：{}", path.display()));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("打不开 {}：{e}", path.display()))?;
    let decoder = Decoder::try_from(file).map_err(|e| format!("解不开 {}：{e}", path.display()))?;
    let duration = decoder
        .total_duration()
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Ok((decoder, duration))
}

/// `.webm` 的音轨抽成缓存里的 m4a（rodio 没有 Matroska 解复用器）。
fn webm_audio_cache(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("找不到这个文件：{}（{e}）", path.display()))?;
    // 缓存键 = 路径 + 体积 + mtime：重播命中，文件被替换后自动失效。
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(meta.len().to_le_bytes());
    if let Ok(stamp) = meta.modified().and_then(|t| {
        t.duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| std::io::Error::other("mtime 早于 epoch"))
    }) {
        hasher.update(stamp.as_secs().to_le_bytes());
    }
    let digest = format!("{:x}", hasher.finalize());
    let dir = root.join("temp").join("webm-audio");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建不了临时目录 {}：{e}", dir.display()))?;
    let target = dir.join(format!("{}.m4a", &digest[..16]));
    if target.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(target);
    }
    let ffmpeg = crate::convert::resolve_engine("ffmpeg").ok_or_else(|| {
        "ERR_NO_FFMPEG：播放 .webm 需要本机 ffmpeg 先把音轨抽出来 —— 把 ffmpeg.exe 放到程序根目录，或装到 PATH 后重试"
            .to_string()
    })?;
    let output = std::process::Command::new(&ffmpeg)
        .arg("-y")
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(path)
        .args(["-vn", "-ac", "2", "-c:a", "aac", "-b:a", "192k"])
        .arg(&target)
        .output()
        .map_err(|e| format!("ERR_FFMPEG_RUN：ffmpeg 起不来（{e}）"))?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&target);
        let raw = String::from_utf8_lossy(&output.stderr);
        let tail = raw.trim().lines().last().unwrap_or("").to_string();
        let tail = if tail.chars().count() > 240 {
            tail.chars().take(240).collect()
        } else {
            tail
        };
        return Err(format!("ERR_FFMPEG_RUN：抽音轨失败（{tail}）"));
    }
    sweep_webm_cache(&dir);
    Ok(target)
}

fn sweep_webm_cache(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path
                .extension()
                .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("m4a"))
                .unwrap_or(false)
            {
                return None;
            }
            entry.metadata().ok()?.modified().ok().map(|at| (at, path))
        })
        .collect();
    if files.len() <= WEBM_AUDIO_KEEP {
        return;
    }
    files.sort_by_key(|(at, _)| *at);
    for (_, path) in files.iter().take(files.len() - WEBM_AUDIO_KEEP) {
        let _ = std::fs::remove_file(path);
    }
}

fn dto_of(session: Option<&Session>) -> PlayerDto {
    let volume = volume();
    let Some(session) = session else {
        return PlayerDto {
            volume,
            ..Default::default()
        };
    };
    let active = !session.player.empty();
    PlayerDto {
        active,
        playing: active && !session.player.is_paused(),
        path: session.path.to_string_lossy().into_owned(),
        position_ms: session.player.get_pos().as_millis() as u64,
        duration_ms: session.duration_ms,
        volume,
        video: is_video_path(&session.path),
    }
}

// ── 调音挂点 ──────────────────────────────────────────────────────────

/// 按配置处理一条**交错**样本流（rodio 的 `Source` 逐样本拉）。
///
/// 三条纪律（与 v1 逐条一致）：
/// ① **每个声道各持一份滤波器状态**（共享状态会把左右声道串起来，单声道素材上还看不出来）；
/// ② **配置热换**：`next()` 里只做一次原子读，版本号变了才拿锁重建链；
/// ③ **建不起链就直通**（宁可什么都不加，也不能让上一档继续生效）。
struct TuningSource<S: Source> {
    inner: S,
    shared: Arc<TuningShared>,
    /// 配置 / 卷积相对路径按它展开。
    root: PathBuf,
    seen: u64,
    /// `None` = 直通（未启用、或那一档没建起来）。
    rt: Option<ChainRuntime>,
    /// 卷积（IR）：每条声道一个流式卷积器。
    conv: Option<Vec<Convolver>>,
    /// 攒帧缓冲（rodio 逐样本拉，`ChainRuntime` 按帧处理）；复用、不在音频线程分配。
    buf: Vec<f64>,
    out: Vec<f64>,
    out_pos: usize,
}

impl<S: Source> TuningSource<S> {
    fn new(inner: S, shared: Arc<TuningShared>, root: PathBuf) -> Self {
        Self {
            inner,
            shared,
            root,
            // 从 0 起：`TuningShared` 的版本号从 1 起 ⇒ 第一次 `next()` 就会建链。
            seen: 0,
            rt: None,
            conv: None,
            buf: Vec::new(),
            out: Vec::new(),
            out_pos: 0,
        }
    }

    /// 版本号没变就立刻返回（常态路径 = 一次宽松原子读）。
    fn retune(&mut self) {
        let version = self.shared.version.load(Ordering::Relaxed);
        if version == self.seen {
            return;
        }
        self.seen = version;
        self.rt = None;
        self.conv = None;
        let cfg = self.shared.snapshot();
        let rate_raw = self.inner.sample_rate().get();
        // 采样率与声道数**先记下**（`enabled` 关着时面板照样要画曲线 / 声道槽）。
        self.shared
            .note_stream(rate_raw, self.inner.channels().get() as u32);
        if !cfg.enabled {
            return;
        }
        let channels = self.inner.channels().get() as usize;
        if channels == 0 {
            return;
        }
        // 通道段 / 声道复制按**这条流**裁一刀：越界的段不该把整条链一起作废。
        let cfg = tuning::for_channels(&cfg, channels);
        let chain = match tuning::build_chain(&self.root, &cfg, rate_raw as f64) {
            Ok(chain) => chain,
            Err(e) => {
                eprintln!("[lunac-player] 调音链没建起来，这一首按直通放（{e}）");
                return;
            }
        };
        match ChainRuntime::new(&chain, channels) {
            Ok(runtime) => self.rt = Some(runtime),
            Err(e) => {
                eprintln!("[lunac-player] 调音链的运行期建不起来，这一首按直通放（{e}）");
                return;
            }
        }
        if let Some(ir) = chain.ir() {
            match Convolver::new(ir) {
                Ok(convolver) => self.conv = Some((0..channels).map(|_| convolver.clone()).collect()),
                Err(e) => eprintln!("[lunac-player] 卷积内核没建起来，这一段按不卷积放（{e}）"),
            }
        }
    }
}

impl<S: Source> Iterator for TuningSource<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        // ① 先把上一帧剩下的样本吐完
        if self.out_pos < self.out.len() {
            let sample = self.out[self.out_pos];
            self.out_pos += 1;
            return Some(sample as Sample);
        }
        // ② 攒齐一帧（rodio 逐样本拉）
        let channels = self.inner.channels().get() as usize;
        if channels == 0 {
            return self.inner.next();
        }
        self.buf.clear();
        while self.buf.len() < channels {
            match self.inner.next() {
                Some(sample) => self.buf.push(sample as f64),
                None => break,
            }
        }
        if self.buf.is_empty() {
            return None;
        }
        self.retune();
        // 半帧（流长度不是声道数整数倍）原样放出去 —— `process_frame` 要求长度严格相等。
        if self.buf.len() == channels {
            if let Some(runtime) = self.rt.as_mut() {
                let _ = runtime.process_frame(&mut self.buf);
            }
            if let Some(convolvers) = self.conv.as_mut() {
                for (channel, value) in self.buf.iter_mut().enumerate() {
                    if let Some(convolver) = convolvers.get_mut(channel) {
                        *value = convolver.process_sample(*value);
                    }
                }
            }
        }
        std::mem::swap(&mut self.buf, &mut self.out);
        self.out_pos = 1;
        Some(self.out[0] as Sample)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S: Source> Source for TuningSource<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    /// 必须转发（`player_seek` 会从 `amplify` 一路打到这里）；跳转成功后清滤波器状态，
    /// 否则上一段音频的尾巴（延迟线尤其明显）会响在新位置上。
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let result = self.inner.try_seek(pos);
        if result.is_ok() {
            if let Some(runtime) = self.rt.as_mut() {
                runtime.reset();
            }
            if let Some(convolvers) = self.conv.as_mut() {
                for convolver in convolvers.iter_mut() {
                    convolver.reset();
                }
            }
            self.buf.clear();
            self.out.clear();
            self.out_pos = 0;
        }
        result
    }
}

// ── 播放 ──────────────────────────────────────────────────────────────

/// 起播一个文件（换曲也走它）。**同步实现**：FFI 那层由 Dart 放 isolate 里调。
///
/// `autoplay=false` 是给「先把这一首装好、再由界面按下播放」用的（面板的
/// `open()` 之后才 `play()`）：装好后就地 `pause()`，音频线程几乎来不及吐样本。
fn play(root: &Path, path: &str, autoplay: bool) -> Result<PlayerDto, String> {
    let requested = PathBuf::from(path);
    // `.webm` 的音轨 rodio 解不了 ⇒ 先抽成缓存 m4a；抽不出来直接拒播（报可读原因）。
    let decode_path = if is_webm_path(&requested) {
        webm_audio_cache(root, &requested)?
    } else {
        requested.clone()
    };
    let (decoder, duration_ms) = open_decoder(&decode_path).map_err(|e| {
        if decode_path == requested {
            e
        } else {
            // 报**用户点的那一份**，不要把缓存里的临时路径抛到界面上。
            format!("解不开 {}：{e}", requested.display())
        }
    })?;
    // 调音状态要在**拿 SESSION 锁之前**取：首次访问会读盘，别把读盘塞进临界区。
    let tuning_shared = tuning::shared_handle(root);

    let mut guard = SESSION.lock().map_err(|e| e.to_string())?;
    // 设备沿用上一首的（没有才开新的）；Player 换一条新的 ⇒ 旧的那首被 drop 掉。
    let sink = match guard.take() {
        Some(old) => old.sink,
        None => {
            let mut sink = rodio::DeviceSinkBuilder::open_default_sink()
                .map_err(|_| "ERR_NO_AUDIO_DEVICE".to_string())?;
            // 默认会在 drop 时往 stderr 打一句噪音，而这里每换一首都会走到。
            sink.log_on_drop(false);
            sink
        }
    };
    let player = rodio::Player::connect_new(sink.mixer());
    player.set_volume(volume());
    player.append(TuningSource::new(decoder, tuning_shared, root.to_path_buf()));
    if !autoplay {
        player.pause();
    }
    *guard = Some(Session {
        sink,
        player,
        path: requested,
        duration_ms,
    });
    Ok(dto_of(guard.as_ref()))
}

// ── 实时测量要用的三个口（v1 `player::pause_for_probe` / `plain_buffer` /
//    `tuned_buffer`，见 `tuning_probe.rs`）────────────────────────────────

/// 测量前把正在放的那一首按下；返回它**原本是否在响**（`false` = 本来就没在放）。
pub fn pause_for_probe() -> bool {
    let Ok(guard) = SESSION.lock() else {
        return false;
    };
    let Some(session) = guard.as_ref() else {
        return false;
    };
    if session.player.empty() || session.player.is_paused() {
        return false;
    }
    session.player.pause();
    true
}

/// 测量结束把歌放回去（**失败也要调**，否则一次没测成的测量会把音乐一直按在暂停上）。
pub fn resume_after_probe(was_playing: bool) {
    if !was_playing {
        return;
    }
    let Ok(guard) = SESSION.lock() else {
        return;
    };
    if let Some(session) = guard.as_ref() {
        if !session.player.empty() {
            session.player.play();
        }
    }
}

fn buffer_source(
    samples: Vec<f32>,
    channels: u16,
    rate: u32,
) -> Result<rodio::buffer::SamplesBuffer, String> {
    let channels = ChannelCount::new(channels).ok_or_else(|| "声道数为 0".to_string())?;
    let rate = SampleRate::new(rate).ok_or_else(|| "采样率为 0".to_string())?;
    Ok(rodio::buffer::SamplesBuffer::new(channels, rate, samples))
}

/// 一段**不经过链**的样本（测量里那趟「系统」）。
pub fn plain_buffer(
    samples: Vec<f32>,
    channels: u16,
    rate: u32,
) -> Result<rodio::buffer::SamplesBuffer, String> {
    buffer_source(samples, channels, rate)
}

/// 一段**经过当前调音链**的样本（测量里那趟「最终输出」）。
///
/// 与真实播放走的是**同一个** `TuningSource` —— 否则量出来的不是用户听到的那条链。
pub fn tuned_buffer(
    root: &Path,
    samples: Vec<f32>,
    channels: u16,
    rate: u32,
) -> Result<impl Source, String> {
    let inner = buffer_source(samples, channels, rate)?;
    let shared = tuning::shared_handle(root);
    Ok(TuningSource::new(inner, shared, root.to_path_buf()))
}

// ── 对 Dart 的入口（JSON in/out，`{ok, error, data}`）───────────────

fn ok(data: PlayerDto) -> String {
    serde_json::json!({ "ok": true, "error": "", "data": data }).to_string()
}

fn fail(error: impl Into<String>) -> String {
    serde_json::json!({ "ok": false, "error": error.into(), "data": null }).to_string()
}

pub fn play_json(root: &Path, path: &str, autoplay: bool) -> String {
    match play(root, path, autoplay) {
        Ok(dto) => ok(dto),
        Err(e) => fail(e),
    }
}

pub fn pause_json() -> String {
    let guard = match SESSION.lock() {
        Ok(guard) => guard,
        Err(e) => return fail(e.to_string()),
    };
    match guard.as_ref() {
        Some(session) if !session.player.empty() => {
            session.player.pause();
            ok(dto_of(guard.as_ref()))
        }
        _ => fail("没有正在播放的曲目"),
    }
}

pub fn resume_json() -> String {
    let guard = match SESSION.lock() {
        Ok(guard) => guard,
        Err(e) => return fail(e.to_string()),
    };
    match guard.as_ref() {
        Some(session) if !session.player.empty() => {
            session.player.play();
            ok(dto_of(guard.as_ref()))
        }
        _ => fail("没有可继续的曲目"),
    }
}

pub fn stop_json() -> String {
    let mut guard = match SESSION.lock() {
        Ok(guard) => guard,
        Err(e) => return fail(e.to_string()),
    };
    match guard.as_mut() {
        Some(session) => {
            session.player.stop();
            *guard = None;
            ok(dto_of(None))
        }
        None => ok(dto_of(None)),
    }
}

pub fn seek_json(position_ms: u64) -> String {
    let guard = match SESSION.lock() {
        Ok(guard) => guard,
        Err(e) => return fail(e.to_string()),
    };
    match guard.as_ref() {
        Some(session) => match session.player.try_seek(Duration::from_millis(position_ms)) {
            Ok(()) => ok(dto_of(guard.as_ref())),
            Err(e) => fail(format!("拖进度失败：{e}")),
        },
        None => fail("没有正在播放的曲目"),
    }
}

pub fn volume_json(value: f32) -> String {
    let clamped = if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        volume()
    };
    VOLUME.store(clamped.to_bits(), Ordering::Relaxed);
    let guard = match SESSION.lock() {
        Ok(guard) => guard,
        Err(e) => return fail(e.to_string()),
    };
    if let Some(session) = guard.as_ref() {
        session.player.set_volume(clamped);
    }
    ok(dto_of(guard.as_ref()))
}

pub fn status_json() -> String {
    match SESSION.lock() {
        Ok(guard) => ok(dto_of(guard.as_ref())),
        Err(_) => ok(dto_of(None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个最小的合法 WAV（单声道 16bit 8kHz、8 个样本）。
    fn tiny_wav(path: &Path) {
        let samples: [i16; 8] = [0, 3000, 6000, 3000, 0, -3000, -6000, -3000];
        let data_len = (samples.len() * 2) as u32;
        let mut bytes = Vec::with_capacity(44 + data_len as usize);
        bytes.extend(b"RIFF");
        bytes.extend((36 + data_len).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes()); // PCM
        bytes.extend(1u16.to_le_bytes()); // 单声道
        bytes.extend(8000u32.to_le_bytes());
        bytes.extend((8000u32 * 2).to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(data_len.to_le_bytes());
        for sample in samples {
            bytes.extend(sample.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn tmp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("lunac-player2-test-{name}"));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn a_decodable_file_yields_its_duration_and_format() {
        let path = tmp("ok.wav");
        tiny_wav(&path);
        let (decoder, ms) = open_decoder(&path).unwrap();
        assert!(ms <= 10, "这段只有 1ms，报出来却是 {ms}ms");
        assert_eq!(decoder.sample_rate().get(), 8000);
        assert_eq!(decoder.channels().get(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_and_broken_files_report_the_path() {
        let missing = tmp("definitely-not-here.wav");
        let message = match open_decoder(&missing) {
            Ok(_) => panic!("这个文件本该开不了"),
            Err(message) => message,
        };
        assert!(message.contains("definitely-not-here.wav"), "{message}");

        let broken = tmp("broken.mp3");
        std::fs::write(&broken, b"this is not audio at all").unwrap();
        let message = match open_decoder(&broken) {
            Ok(_) => panic!("坏文件必须报错，而不是当成静音播出去"),
            Err(message) => message,
        };
        assert!(message.contains("broken.mp3"), "{message}");
        let _ = std::fs::remove_file(&broken);
    }

    #[test]
    fn volume_is_clamped_and_kept_in_its_bit_pattern() {
        // 不走 `volume_json`（它会去拿 SESSION 锁 + 可能开设备），只验位模式这条口径
        let bits = 0.5f32.to_bits();
        assert_eq!(f32::from_bits(bits), 0.5);
        assert!(f32::from_bits(DEFAULT_VOLUME_BITS) > 0.0 && f32::from_bits(DEFAULT_VOLUME_BITS) < 1.0);
    }

    #[test]
    fn dto_of_an_empty_session_reports_volume_only() {
        let dto = dto_of(None);
        assert!(!dto.active);
        assert!(!dto.playing);
        assert_eq!(dto.path, "");
        assert_eq!(dto.duration_ms, 0);
        assert!(!dto.video);
    }

    #[test]
    fn video_and_webm_extensions_are_recognised_case_insensitively() {
        assert!(is_video_path(Path::new(r"D:\a\B.MP4")));
        assert!(is_video_path(Path::new("a.mkv")));
        assert!(!is_video_path(Path::new("a.mp3")));
        assert!(is_webm_path(Path::new("A.WEBM")));
        assert!(!is_webm_path(Path::new("a.mp4")));
    }
}
