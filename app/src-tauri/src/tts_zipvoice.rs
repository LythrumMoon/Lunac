// ── tts_zipvoice.rs — 桌宠语音的 **ZipVoice 零样本克隆** 通路（2026-10-10）────────
//
// **它解决什么**：本文件是桌宠语音**唯一的合成通路**（2026-10-12 起去掉了 WinRT 系统音色兜底，
// 见 `tts.rs` 文件头）。给一段 **3~30 秒**参考音频 + 那句参考文本，ZipVoice 就能用该音色朗读任意文本。
//
// **为什么是「运行期加载 DLL」而不是 `cargo add sherpa-onnx`**：
// 本项目用 `x86_64-pc-windows-gnu`（MinGW）工具链，而 sherpa-onnx 的 Rust crate 下的是
// **MSVC/MT 静态库** ⇒ 链接期必挂。这里改用宿主自带的 `sherpa-onnx-c-api.dll`，**运行期**
// 用 `libloading` 取函数指针 —— 只依赖 PE 导出表，与工具链 ABI 无关。
//
// **为什么不能像别的插件那样「起 exe 传参数」**：`sherpa-onnx-offline-tts.exe` 的 argv 经
// MSVC CRT 按 **ANSI(GBK)** 转换，中文必乱码（实测 PowerShell 与 Rust 两条路都复现）。
// 走 C API 时文本是 **UTF-8 的 `CString`**，不经 argv ⇒ 中文正确。
//
// **资产归属（用户口径：内置到桌宠插件里）**：运行库 + 模型都在**桌宠插件目录**下 ——
//   `<Modules>\pet\zipvoice\sherpa-onnx-c-api.dll`（+ 同目录 `onnxruntime.dll` 等）
//   `<Modules>\pet\zipvoice\model\`（encoder.int8.onnx / decoder.int8.onnx / tokens.txt /
//                                    lexicon.txt / espeak-ng-data/）
//   `<Modules>\pet\zipvoice\vocos_24khz.onnx`
// 由桌宠清单的 `dependencies`（`type:"archive"`，只支持 .7z/.zip）装入；宿主只**读**这个目录。
//
// **常驻**：模型只加载一次（`TTS` 里的 `OnceLock`），之后每次朗读只花合成时间。
// 失败一律**静默不读**（见 `tts.rs`）—— 朗读是锦上添花，不该把气泡带崩。
//
// **音色库缓存（2026-10-12）**：用户经设置窗导入的参考音频，会被**规整成 PCM16 WAV** 复制进
// `config\pet-voices\`（见 `import_pet_voice_audio`）—— 原文件被移动 / 删除后朗读照常。
// 目录卫生：只保留最近一次导入的那一份副本（见 `prune_voice_cache`）。

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::ffi::{c_char, c_float, c_void, CString};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use libloading::{Library, Symbol};
use serde::{Deserialize, Serialize};

// ══════════════════════════════════════════════════════════════════
//  sherpa-onnx C API 的 FFI 定义
//  （逐字段抄自 `sherpa-onnx-sys` 1.13.8 的 bindgen 产物 `src/tts.rs` —— 布局必须一致）
// ══════════════════════════════════════════════════════════════════

#[repr(C)]
#[derive(Copy, Clone)]
struct OfflineTtsVitsModelConfig {
    model: *const c_char,
    lexicon: *const c_char,
    tokens: *const c_char,
    data_dir: *const c_char,
    noise_scale: c_float,
    noise_scale_w: c_float,
    length_scale: c_float,
    dict_dir: *const c_char,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct OfflineTtsMatchaModelConfig {
    acoustic_model: *const c_char,
    vocoder: *const c_char,
    lexicon: *const c_char,
    tokens: *const c_char,
    data_dir: *const c_char,
    noise_scale: c_float,
    length_scale: c_float,
    dict_dir: *const c_char,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct OfflineTtsKokoroModelConfig {
    model: *const c_char,
    voices: *const c_char,
    tokens: *const c_char,
    data_dir: *const c_char,
    length_scale: c_float,
    dict_dir: *const c_char,
    lexicon: *const c_char,
    lang: *const c_char,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct OfflineTtsKittenModelConfig {
    model: *const c_char,
    voices: *const c_char,
    tokens: *const c_char,
    data_dir: *const c_char,
    length_scale: c_float,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct OfflineTtsZipvoiceModelConfig {
    tokens: *const c_char,
    encoder: *const c_char,
    decoder: *const c_char,
    vocoder: *const c_char,
    data_dir: *const c_char,
    lexicon: *const c_char,
    feat_scale: c_float,
    t_shift: c_float,
    target_rms: c_float,
    guidance_scale: c_float,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct OfflineTtsPocketModelConfig {
    lm_flow: *const c_char,
    lm_main: *const c_char,
    encoder: *const c_char,
    decoder: *const c_char,
    text_conditioner: *const c_char,
    vocab_json: *const c_char,
    token_scores_json: *const c_char,
    voice_embedding_cache_capacity: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct OfflineTtsSupertonicModelConfig {
    duration_predictor: *const c_char,
    text_encoder: *const c_char,
    vector_estimator: *const c_char,
    vocoder: *const c_char,
    tts_json: *const c_char,
    unicode_indexer: *const c_char,
    voice_style: *const c_char,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct OfflineTtsModelConfig {
    vits: OfflineTtsVitsModelConfig,
    num_threads: i32,
    debug: i32,
    provider: *const c_char,
    matcha: OfflineTtsMatchaModelConfig,
    kokoro: OfflineTtsKokoroModelConfig,
    kitten: OfflineTtsKittenModelConfig,
    zipvoice: OfflineTtsZipvoiceModelConfig,
    pocket: OfflineTtsPocketModelConfig,
    supertonic: OfflineTtsSupertonicModelConfig,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct OfflineTtsConfig {
    model: OfflineTtsModelConfig,
    rule_fsts: *const c_char,
    max_num_sentences: i32,
    rule_fars: *const c_char,
    silence_scale: c_float,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct GeneratedAudio {
    samples: *const f32,
    n: i32,
    sample_rate: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct GenerationConfig {
    silence_scale: c_float,
    speed: c_float,
    sid: i32,
    reference_audio: *const f32,
    reference_audio_len: i32,
    reference_sample_rate: i32,
    reference_text: *const c_char,
    num_steps: i32,
    extra: *const c_char,
}

type CreateFn = unsafe extern "C" fn(*const OfflineTtsConfig) -> *const c_void;
type DestroyFn = unsafe extern "C" fn(*const c_void);
type GenFn = unsafe extern "C" fn(
    *const c_void,
    *const c_char,
    *const GenerationConfig,
    Option<unsafe extern "C" fn(*const f32, i32, c_float, *mut c_void) -> i32>,
    *mut c_void,
) -> *const GeneratedAudio;
type DestroyAudioFn = unsafe extern "C" fn(*const GeneratedAudio);

// `onnxruntime.dll` 等依赖必须先被找到 —— 加载前把 DLL 目录塞进搜索路径。
#[link(name = "kernel32")]
extern "system" {
    fn SetDllDirectoryW(lpPathName: *const u16) -> i32;
}

// ══════════════════════════════════════════════════════════════════
//  配置 / 路径
// ══════════════════════════════════════════════════════════════════

/// 桌宠语音配置（落 `config\pet-voice.json`）。字段都带缺省，老文件不会解析失败。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PetVoiceConfig {
    /// 开关。缺省 **true** —— 只要资产与参考音频齐备就自动生效。
    #[serde(default = "d_true")]
    pub enabled: bool,
    /// 参考音频（你自己录的那段）绝对路径。空 = 未配置 ⇒ **静默不读**（不再回落系统音色）。
    #[serde(default)]
    pub reference_audio: String,
    /// 参考音频对应的那句话（ZipVoice **必需**）。
    #[serde(default)]
    pub reference_text: String,
    /// 流匹配步数（越小越快、越小越糙）。缺省 8。
    #[serde(default = "d_steps")]
    pub num_steps: i32,
    /// 后端线程数。缺省 4。
    #[serde(default = "d_threads")]
    pub num_threads: i32,
    /// 语速。
    #[serde(default = "d_one")]
    pub speed: f32,
    /// **语音音量**（0.0 = 静音，1.0 = 原始音量）。播放时乘到每条样本上（见 `tts::play_wav`）。
    /// 老 `pet-voice.json` 缺这个字段时回落 **1.0**（见 `d_one`）—— `#[serde(default)]` 保证旧文件不解析失败。
    #[serde(default = "d_one")]
    pub volume: f32,
}

/// 语速的生效区间（`PetVoiceConfig.speed`）—— 与设置窗「语音语速」拉条范围一致。
pub const SPEED_MIN: f32 = 0.5;
pub const SPEED_MAX: f32 = 2.0;
/// **音量**的生效区间（`PetVoiceConfig.volume`）—— 0 = 静音、1 = 原始。
/// 上限取 1 而非 2：音量是对每条样本的乘数，>1 会削顶失真。
pub const VOLUME_MIN: f32 = 0.0;
pub const VOLUME_MAX: f32 = 1.0;

impl PetVoiceConfig {
    /// 语速夹取到 `SPEED_MIN..=SPEED_MAX`（合成侧用，替代原先散落的 `clamp(0.5, 2.0)`）。
    pub fn clamped_speed(&self) -> f32 {
        self.speed.clamp(SPEED_MIN, SPEED_MAX)
    }
    /// 音量夹取到 `VOLUME_MIN..=VOLUME_MAX`（播放侧用）。
    pub fn clamped_volume(&self) -> f32 {
        self.volume.clamp(VOLUME_MIN, VOLUME_MAX)
    }
}

fn d_true() -> bool {
    true
}
fn d_steps() -> i32 {
    8
}
fn d_threads() -> i32 {
    4
}
fn d_one() -> f32 {
    1.0
}

impl Default for PetVoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reference_audio: String::new(),
            reference_text: String::new(),
            num_steps: 8,
            num_threads: 4,
            speed: 1.0,
            volume: 1.0,
        }
    }
}

fn config_path() -> PathBuf {
    crate::storage::lunac_root_dir().join("config").join("pet-voice.json")
}

pub fn load_config() -> PetVoiceConfig {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str::<PetVoiceConfig>(&s).ok())
        .unwrap_or_default()
}

pub fn save_config(cfg: &PetVoiceConfig) -> Result<(), String> {
    let p = config_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建配置目录失败：{e}"))?;
    }
    let s = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化失败：{e}"))?;
    std::fs::write(&p, s).map_err(|e| format!("写配置失败：{e}"))
}

/// ZipVoice 运行库 + 模型所在的目录（**桌宠插件目录下**）。
fn zipvoice_dir() -> PathBuf {
    crate::plugin_market::plugins_dir().join("pet").join("zipvoice")
}

/// 资产是否齐备（DLL + 模型 + vocoder）。任一缺 = 这条通路不可用。
pub fn assets_ready() -> bool {
    let d = zipvoice_dir();
    d.join("sherpa-onnx-c-api.dll").is_file()
        && d.join("model").join("encoder.int8.onnx").is_file()
        && d.join("model").join("decoder.int8.onnx").is_file()
        && d.join("vocos_24khz.onnx").is_file()
}

/// 音色库缓存目录：`config\pet-voices\`（与 `pet-voice.json` 同层，走既有 `lunac_root_dir()`）。
/// 实体 = 用户导入的参考音色经**规整后的 PCM16 WAV 副本**（见 `import_pet_voice_audio`）。
fn voices_dir() -> PathBuf {
    crate::storage::lunac_root_dir()
        .join("config")
        .join("pet-voices")
}

/// ZipVoice 参考音频的**可用时长区间（秒）** —— 出处见本文件文件头（3~30 秒）。
pub const REF_MIN_SECS: f64 = 3.0;
pub const REF_MAX_SECS: f64 = 30.0;

/// 不可用的原因（**机器可读 key**，供前端本地化成文案）。空串 = 可用。
///
/// 取值：`disabled` / `no_ref_audio` / `no_ref_text` / `ref_missing` / `assets_missing`。
/// `speak_blocking` 的**静默不读**判据就是它非空（见 `tts.rs`）。
fn reason_for(cfg: &PetVoiceConfig, assets_ready: bool) -> String {
    if !cfg.enabled {
        return "disabled".into();
    }
    if cfg.reference_audio.trim().is_empty() {
        return "no_ref_audio".into();
    }
    if cfg.reference_text.trim().is_empty() {
        return "no_ref_text".into();
    }
    if !Path::new(cfg.reference_audio.trim()).is_file() {
        return "ref_missing".into();
    }
    if !assets_ready {
        return "assets_missing".into();
    }
    String::new()
}

/// 这条通路此刻**能不能用**（给命令 / 前端看的完整判据）。
pub fn available() -> bool {
    reason_for(&load_config(), assets_ready()).is_empty()
}

// ══════════════════════════════════════════════════════════════════
//  常驻 TTS（模型只加载一次）
// ══════════════════════════════════════════════════════════════════

/// 一份已经建好的 TTS 会话。`_lib` 必须**握着**（drop 掉 DLL 就卸载，函数指针即失效）。
struct TtsSession {
    _lib: Library,
    handle: *const c_void,
    gen: GenFn,
    destroy: DestroyFn,
    destroy_audio: DestroyAudioFn,
}

// 只是一组函数指针 + 一个不透明句柄；跨线程共享由外层 Mutex 串行化。
unsafe impl Send for TtsSession {}

static TTS: OnceLock<Mutex<Option<TtsSession>>> = OnceLock::new();

fn tts_slot() -> &'static Mutex<Option<TtsSession>> {
    TTS.get_or_init(|| Mutex::new(None))
}

/// 句柄变动（换了线程数 / 模型目录）⇒ 作废重来。这里只按线程数记一个简单指纹。
static TTS_FINGERPRINT: OnceLock<Mutex<i64>> = OnceLock::new();

fn fingerprint_of(cfg: &PetVoiceConfig) -> i64 {
    cfg.num_threads as i64
}

/// 取（必要时**建**）常驻会话。失败不缓存 —— 用户补上文件后应当能再试。
fn ensure_session(cfg: &PetVoiceConfig) -> Result<(), String> {
    let fp = fingerprint_of(cfg);
    let fp_slot = TTS_FINGERPRINT.get_or_init(|| Mutex::new(i64::MIN));
    {
        let guard = tts_slot().lock().map_err(|_| "TTS 锁中毒".to_string())?;
        let cur = *fp_slot.lock().map_err(|_| "TTS 锁中毒".to_string())?;
        if guard.is_some() && cur == fp {
            return Ok(());
        }
    }
    let session = create_session(cfg)?;
    let mut guard = tts_slot().lock().map_err(|_| "TTS 锁中毒".to_string())?;
    if let Some(old) = guard.take() {
        unsafe { (old.destroy)(old.handle) };
    }
    *guard = Some(session);
    *fp_slot.lock().map_err(|_| "TTS 锁中毒".to_string())? = fp;
    Ok(())
}

fn create_session(cfg: &PetVoiceConfig) -> Result<TtsSession, String> {
    let dir = zipvoice_dir();
    let model = dir.join("model");
    let dll_path = dir.join("sherpa-onnx-c-api.dll");

    // 先把 DLL 目录塞进搜索路径，`onnxruntime.dll` 才找得到。
    let mut wide: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(dir.as_os_str()).collect();
    wide.push(0);
    unsafe { SetDllDirectoryW(wide.as_ptr()) };

    let lib = unsafe { Library::new(&dll_path) }
        .map_err(|e| format!("加载 sherpa-onnx-c-api.dll 失败：{e}"))?;

    let create: Symbol<CreateFn> = unsafe { lib.get(b"SherpaOnnxCreateOfflineTts\0") }
        .map_err(|e| format!("找不到 SherpaOnnxCreateOfflineTts：{e}"))?;
    let destroy: Symbol<DestroyFn> = unsafe { lib.get(b"SherpaOnnxDestroyOfflineTts\0") }
        .map_err(|e| format!("找不到 SherpaOnnxDestroyOfflineTts：{e}"))?;
    let generate: Symbol<GenFn> =
        unsafe { lib.get(b"SherpaOnnxOfflineTtsGenerateWithConfig\0") }
            .map_err(|e| format!("找不到 SherpaOnnxOfflineTtsGenerateWithConfig：{e}"))?;
    let destroy_audio: Symbol<DestroyAudioFn> =
        unsafe { lib.get(b"SherpaOnnxDestroyOfflineTtsGeneratedAudio\0") }
            .map_err(|e| format!("找不到 SherpaOnnxDestroyOfflineTtsGeneratedAudio：{e}"))?;

    // 路径全部转 UTF-8 CString；**在 create 调用期间必须存活**。
    let p = |x: PathBuf| -> Result<CString, String> {
        CString::new(x.to_string_lossy().as_bytes()).map_err(|e| format!("路径含 NUL：{e}"))
    };
    let c_tokens = p(model.join("tokens.txt"))?;
    let c_encoder = p(model.join("encoder.int8.onnx"))?;
    let c_decoder = p(model.join("decoder.int8.onnx"))?;
    let c_vocoder = p(dir.join("vocos_24khz.onnx"))?;
    let c_data = p(model.join("espeak-ng-data"))?;
    let c_lexicon = p(model.join("lexicon.txt"))?;
    let c_provider = CString::new("cpu").unwrap();

    let conf = OfflineTtsConfig {
        model: OfflineTtsModelConfig {
            vits: unsafe { std::mem::zeroed() },
            num_threads: cfg.num_threads.clamp(1, 32),
            debug: 0,
            provider: c_provider.as_ptr(),
            matcha: unsafe { std::mem::zeroed() },
            kokoro: unsafe { std::mem::zeroed() },
            kitten: unsafe { std::mem::zeroed() },
            zipvoice: OfflineTtsZipvoiceModelConfig {
                tokens: c_tokens.as_ptr(),
                encoder: c_encoder.as_ptr(),
                decoder: c_decoder.as_ptr(),
                vocoder: c_vocoder.as_ptr(),
                data_dir: c_data.as_ptr(),
                lexicon: c_lexicon.as_ptr(),
                feat_scale: 0.1,
                t_shift: 0.5,
                target_rms: 0.1,
                guidance_scale: 1.0,
            },
            pocket: unsafe { std::mem::zeroed() },
            supertonic: unsafe { std::mem::zeroed() },
        },
        rule_fsts: std::ptr::null(),
        max_num_sentences: 1,
        rule_fars: std::ptr::null(),
        silence_scale: 0.0,
    };

    let handle = unsafe { create(&conf) };
    if handle.is_null() {
        return Err("创建 ZipVoice 会话失败（模型文件或参数有问题）".into());
    }
    Ok(TtsSession {
        handle,
        gen: *generate,
        destroy: *destroy,
        destroy_audio: *destroy_audio,
        _lib: lib,
    })
}

// ══════════════════════════════════════════════════════════════════
//  朗读
// ══════════════════════════════════════════════════════════════════

/// 用 ZipVoice 合成 `text` 并播放。**阻塞**（在后台线程里跑）。
/// `app` 只用来把**口型包络**下发给桌宠窗（见 `tts::play_wav_bytes`）。
pub fn speak(app: &tauri::AppHandle, text: &str) -> Result<(), String> {
    let cfg = load_config();
    if !cfg.enabled {
        return Err("ZipVoice 未启用".into());
    }
    let ref_path = cfg.reference_audio.trim();
    if ref_path.is_empty() {
        return Err("未配置参考音频".into());
    }
    let (ref_samples, ref_sr) = read_wav_mono(Path::new(ref_path))?;
    ensure_session(&cfg)?;

    let text_c = CString::new(text.as_bytes()).map_err(|e| format!("文本含 NUL：{e}"))?;
    let ref_text_c =
        CString::new(cfg.reference_text.as_bytes()).map_err(|e| format!("参考文本含 NUL：{e}"))?;
    let gen_cfg = GenerationConfig {
        silence_scale: 0.0,
        speed: cfg.clamped_speed(),
        sid: 0,
        reference_audio: ref_samples.as_ptr(),
        reference_audio_len: ref_samples.len() as i32,
        reference_sample_rate: ref_sr,
        reference_text: ref_text_c.as_ptr(),
        num_steps: cfg.num_steps.clamp(1, 64),
        extra: std::ptr::null(),
    };

    let guard = tts_slot().lock().map_err(|_| "TTS 锁中毒".to_string())?;
    let session = guard.as_ref().ok_or("TTS 会话未就绪")?;
    let audio = unsafe {
        (session.gen)(
            session.handle,
            text_c.as_ptr(),
            &gen_cfg,
            None,
            std::ptr::null_mut(),
        )
    };
    if audio.is_null() {
        return Err("合成失败（返回空音频）".into());
    }
    let (samples, sr) = unsafe {
        let a = &*audio;
        (
            std::slice::from_raw_parts(a.samples, a.n.max(0) as usize).to_vec(),
            a.sample_rate,
        )
    };
    unsafe { (session.destroy_audio)(audio) };

    // 与 WinRT 那条复用同一个「WAV 落盘 → rodio 播放」的收尾（见 tts.rs）。
    let wav = encode_wav_pcm16(&samples, sr);
    crate::tts::play_wav_bytes(app, &wav)
}

// ══════════════════════════════════════════════════════════════════
//  极简 WAV 读写（只认 PCM16 / float32，取第一声道）
// ══════════════════════════════════════════════════════════════════

fn read_wav_mono(path: &Path) -> Result<(Vec<f32>, i32), String> {
    let b = std::fs::read(path).map_err(|e| format!("读参考音频失败：{e}"))?;
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("参考音频不是 RIFF/WAVE（请用 WAV）".into());
    }
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let sz = u32::from_le_bytes([b[pos + 4], b[pos + 5], b[pos + 6], b[pos + 7]]) as usize;
        let end = (pos + 8 + sz).min(b.len());
        let body = &b[pos + 8..end];
        if id == b"fmt " && body.len() >= 16 {
            fmt = Some((
                u16::from_le_bytes([body[0], body[1]]),
                u16::from_le_bytes([body[2], body[3]]),
                u32::from_le_bytes([body[4], body[5], body[6], body[7]]),
                u16::from_le_bytes([body[14], body[15]]),
            ));
        } else if id == b"data" {
            data = Some(body);
        }
        pos += 8 + sz + (sz & 1);
    }
    let (format, channels, rate, bits) = fmt.ok_or("参考音频缺少 fmt 块")?;
    let data = data.ok_or("参考音频缺少 data 块")?;
    let ch = channels.max(1) as usize;
    let mut out = Vec::new();
    if format == 1 && bits == 16 {
        for i in 0..(data.len() / (2 * ch)) {
            let o = i * 2 * ch;
            out.push(i16::from_le_bytes([data[o], data[o + 1]]) as f32 / 32768.0);
        }
    } else if format == 3 && bits == 32 {
        for i in 0..(data.len() / (4 * ch)) {
            let o = i * 4 * ch;
            out.push(f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]));
        }
    } else {
        return Err(format!("不支持的 WAV 格式（fmt={format} bits={bits}）—— 请用 16bit PCM WAV"));
    }
    if out.is_empty() {
        return Err("参考音频是空的".into());
    }
    Ok((out, rate as i32))
}

fn encode_wav_pcm16(samples: &[f32], sr: i32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut b: Vec<u8> = Vec::with_capacity(44 + samples.len() * 2);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&(sr as u32).to_le_bytes());
    b.extend_from_slice(&((sr as u32) * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    b
}

// ══════════════════════════════════════════════════════════════════
//  音色库导入（任意常见音频 → 规整成 PCM16 WAV 存进缓存目录）
// ══════════════════════════════════════════════════════════════════

/// 按声道数把交错样本**混成单声道**（多声道取平均；单声道原样返回）。
fn to_mono(interleaved: Vec<f32>, channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved;
    }
    interleaved
        .chunks_exact(channels)
        .map(|fr| fr.iter().copied().sum::<f32>() / channels as f32)
        .collect()
}

/// 用 **rodio**（内含 symphonia）解码任意常见音频（wav / mp3 / flac / ogg / m4a）成
/// `(单声道样本, 采样率)`。走它而不是 `read_wav_mono`：那一份只认 RIFF/WAVE，而用户导入的
/// 往往是 mp3 / m4a。**解不出来必须报错，绝不静默成功**（错误文案对用户可读，原始错误不进界面）。
fn decode_audio_mono(path: &Path) -> Result<(Vec<f32>, i32), String> {
    use rodio::Source;
    let file =
        std::fs::File::open(path).map_err(|_| "打不开这个音频文件（可能已被移动或删除）".to_string())?;
    let decoder = rodio::Decoder::try_from(file)
        .map_err(|_| "这个文件不是可识别的音频（支持 wav / mp3 / flac / ogg / m4a）".to_string())?;
    let channels = decoder.channels().get() as usize;
    let sr = decoder.sample_rate().get() as i32;
    let interleaved: Vec<f32> = decoder.collect();
    Ok((to_mono(interleaved, channels), sr))
}

/// 缓存副本的文件名 = **内容哈希**（规整后 WAV 的 SHA-256）⇒ 同一段音频重复导入必落同一文件、
/// 不同音频必不互相覆盖，且天然免路径注入（文件名只含 `voice-` + 十六进制 + `.wav`）。
fn content_name(wav: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(wav);
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    format!("voice-{hex}.wav")
}

/// 音色库**目录卫生**：缓存目录只保留 `keep` 这一份 `*.wav`，其余删掉。
///
/// 理由：`config\pet-voice.json` 只持有一个 `reference_audio`，所以音量库的实体也只有当前这
/// 一份；不清理的话每次导入都会留一份，目录会无限增长（这条义务见 `voices_dir` 注释）。
fn prune_voice_cache(dir: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p == *keep {
            continue;
        }
        if p.extension().map(|x| x.eq_ignore_ascii_case("wav")).unwrap_or(false) {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// 导入核心（**可单测**：`dir` 由调用方给，生产传 `voices_dir()`、测试传系统 temp 子目录）：
/// 校验 → 规整成 PCM16 WAV → 写进 `dir` → 清理旧副本 → 返回缓存副本路径。
///
/// 校验（失败各给一句可读错误）：文件不存在 / 解不出音频 / 解出来是空的 / 时长越界（3~30 秒）。
fn import_voice_file(src: &Path, dir: &Path) -> Result<PathBuf, String> {
    if !src.is_file() {
        return Err("找不到这个音频文件（可能已被移动或删除）".into());
    }
    let (samples, sr) = decode_audio_mono(src)?;
    if samples.is_empty() || sr <= 0 {
        return Err("这段音频里没有解码出声音".into());
    }
    let secs = samples.len() as f64 / sr as f64;
    if secs < REF_MIN_SECS || secs > REF_MAX_SECS {
        return Err(format!(
            "参考音频时长需在 {:.0}~{:.0} 秒之间，这段约 {:.1} 秒",
            REF_MIN_SECS, REF_MAX_SECS, secs
        ));
    }
    let wav = encode_wav_pcm16(&samples, sr);
    std::fs::create_dir_all(dir).map_err(|_| "创建音色库目录失败".to_string())?;
    let dst = dir.join(content_name(&wav));
    if !dst.is_file() {
        std::fs::write(&dst, &wav).map_err(|_| "写入音色库失败".to_string())?;
    }
    prune_voice_cache(dir, &dst);
    Ok(dst)
}

// ══════════════════════════════════════════════════════════════════
//  命令
// ══════════════════════════════════════════════════════════════════

/// 前端拿到的语音状态（配置 + 可用性 + 缺什么）。
#[derive(Debug, Serialize)]
pub struct PetVoiceStatus {
    pub config: PetVoiceConfig,
    /// 运行时/模型是否已就位。
    pub assets_ready: bool,
    /// 现在这条通路能否真的用（开关 + 参考音频 + 资产全齐）。
    pub available: bool,
    /// 不可用时的原因（机器可读 key，见 `reason_for`）；**空串 = 可用**。
    pub reason: String,
    /// 资产目录（给前端显示 / 排障）。
    pub dir: String,
}

#[tauri::command]
pub fn get_pet_voice() -> PetVoiceStatus {
    let config = load_config();
    let assets_ready = assets_ready();
    let reason = reason_for(&config, assets_ready);
    let available = reason.is_empty();
    PetVoiceStatus {
        config,
        assets_ready,
        available,
        reason,
        dir: zipvoice_dir().to_string_lossy().to_string(),
    }
}

#[tauri::command]
pub fn set_pet_voice(config: PetVoiceConfig) -> Result<(), String> {
    save_config(&config)
}

/// 导入一个「参考音色」音频（设置窗「添加音频文件」按钮走它）：
/// 校验（可读 / 可解码 / 时长 3~30 秒）→ **规整成 PCM16 WAV** 复制进音色库缓存目录 →
/// **返回缓存副本的绝对路径**。前端据此调 `set_pet_voice({ reference_audio: 缓存路径, reference_text })`
/// （**共用同一份 `PetVoiceConfig`，不新增第二套配置**）。
///
/// 为什么缓存：用户选的那份原文件被移动 / 删除后朗读不该跟着挂，缓存副本才是「音色库」的实体。
///
/// **async 薄壳 + `run_blocking`**：解码 / 规整 / 落盘都是阻塞 IO 与 CPU（可能几 MB 的音频），
/// 不能跑在主线程（见 code-standards §规范 5）。
#[tauri::command]
pub async fn import_pet_voice_audio(path: String) -> Result<String, String> {
    crate::commands::run_blocking(move || {
        let src = PathBuf::from(path.trim());
        if src.as_os_str().is_empty() {
            return Err("没有选择音频文件".into());
        }
        let dst = import_voice_file(&src, &voices_dir())?;
        Ok(dst.to_string_lossy().to_string())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试夹具目录：**建在系统 temp（仓库之外）**，用完删掉（绝不落本仓库）。
    fn temp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lunac-voice-import-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 写一段 **PCM16 单声道** WAV（只用于导入校验，故不做参数校验）。
    fn write_wav(path: &Path, sr: u32, secs: f64) {
        let n = (sr as f64 * secs) as usize;
        let data_len = (n * 2) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&1u16.to_le_bytes()); // 单声道
        b.extend_from_slice(&sr.to_le_bytes());
        b.extend_from_slice(&(sr * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..n {
            let s: i16 = if i % 2 == 0 { 8000 } else { -8000 };
            b.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    /// **静默不读的判据** `reason_for`：逐条覆盖 —— 可用 / 关 / 无音频 / 无文本 / 音频不在 / 资产缺。
    #[test]
    fn reason_covers_every_silent_branch() {
        let dir = temp_root("reason");
        let audio = dir.join("ref.wav");
        write_wav(&audio, 8000, 4.0);
        let p = audio.to_string_lossy().to_string();

        let mut cfg = PetVoiceConfig {
            enabled: true,
            reference_audio: p.clone(),
            reference_text: "你好".into(),
            ..Default::default()
        };
        assert_eq!(reason_for(&cfg, true), "", "全齐应可用（空原因）");
        cfg.enabled = false;
        assert_eq!(reason_for(&cfg, true), "disabled");
        cfg.enabled = true;
        cfg.reference_audio = String::new();
        assert_eq!(reason_for(&cfg, true), "no_ref_audio");
        cfg.reference_audio = p.clone();
        cfg.reference_text = String::new();
        assert_eq!(reason_for(&cfg, true), "no_ref_text");
        cfg.reference_text = "你好".into();
        cfg.reference_audio = dir.join("nope.wav").to_string_lossy().to_string();
        assert_eq!(reason_for(&cfg, true), "ref_missing");
        cfg.reference_audio = p;
        assert_eq!(reason_for(&cfg, false), "assets_missing");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 失败分支①：文件不存在 —— 可读错误、且不落任何文件。
    #[test]
    fn import_rejects_missing_file() {
        let root = temp_root("missing");
        let cache = root.join("cache");
        let err = import_voice_file(&root.join("nope.wav"), &cache).unwrap_err();
        assert!(err.contains("找不到"), "err={err}");
        assert!(!cache.exists(), "失败不应建目录 / 落文件");
        std::fs::remove_dir_all(&root).ok();
    }

    /// 失败分支②：不是音频 —— 必须报错，**绝不静默成功**。
    #[test]
    fn import_rejects_non_audio() {
        let root = temp_root("notaudio");
        let not_audio = root.join("readme.mp3");
        std::fs::write(&not_audio, b"this is definitely not audio data").unwrap();
        let err = import_voice_file(&not_audio, &root.join("cache")).unwrap_err();
        assert!(err.contains("不是可识别的音频"), "err={err}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// 失败分支③：时长越界（< 3s 与 > 30s）—— 都不落文件。
    #[test]
    fn import_rejects_out_of_range_duration() {
        let root = temp_root("dur");
        let cache = root.join("cache");
        let short = root.join("short.wav");
        let long = root.join("long.wav");
        write_wav(&short, 8000, 1.0); // 1s
        write_wav(&long, 8000, 40.0); // 40s
        let e1 = import_voice_file(&short, &cache).unwrap_err();
        assert!(e1.contains("时长需在"), "err={e1}");
        let e2 = import_voice_file(&long, &cache).unwrap_err();
        assert!(e2.contains("时长需在"), "err={e2}");
        assert!(!cache.exists(), "越界不应落文件");
        std::fs::remove_dir_all(&root).ok();
    }

    /// 成功路径：缓存副本落点 / 命名（内容哈希）/ PCM16 可读回 / 旧副本被清理。
    #[test]
    fn import_caches_pcm16_wav_and_prunes_old_copy() {
        let root = temp_root("cache");
        let src = root.join("src");
        let cache = root.join("cache");
        std::fs::create_dir_all(&src).unwrap();
        let a = src.join("a.wav");
        let b = src.join("b.wav");
        write_wav(&a, 8000, 4.0);
        write_wav(&b, 8000, 5.0);

        let da = import_voice_file(&a, &cache).unwrap();
        assert!(da.is_file());
        assert!(da.starts_with(&cache), "副本应落缓存目录：{}", da.display());
        let name = da.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("voice-") && name.ends_with(".wav"), "name={name}");
        // 缓存的是**可读回的 PCM16 WAV**，时长≈4s
        let (s, sr) = read_wav_mono(&da).unwrap();
        assert_eq!(sr, 8000);
        assert!((s.len() as f64 / sr as f64 - 4.0).abs() < 0.05, "len={}", s.len());
        // 同一段再导 ⇒ 内容哈希同 ⇒ 落同一文件（幂等、不新增长）
        let da2 = import_voice_file(&a, &cache).unwrap();
        assert_eq!(da, da2);
        // 换一段不同音频 ⇒ 旧副本被清理，只剩新的一份
        let db = import_voice_file(&b, &cache).unwrap();
        assert_ne!(da, db);
        assert!(db.is_file());
        assert!(!da.is_file(), "旧副本应被清理");
        let n = std::fs::read_dir(&cache).unwrap().flatten().count();
        assert_eq!(n, 1, "缓存目录应只剩一份副本");
        std::fs::remove_dir_all(&root).ok();
    }

    /// **音量**：缺省回落 1.0（含「老 json 缺字段」）；`clamped_volume` 把越界值夹到 `0.0..=1.0`；
    /// `clamped_speed` 把语速夹到 `0.5..=2.0`（边界各测一轮）。
    #[test]
    fn voice_volume_default_and_clamps() {
        // 缺省（Default）与「老 json 缺 volume 字段」都回落 1.0
        assert_eq!(PetVoiceConfig::default().volume, 1.0);
        let old_json = r#"{"enabled":true,"reference_audio":"","reference_text":"","num_steps":8,"num_threads":4,"speed":1.0}"#;
        let cfg: PetVoiceConfig = serde_json::from_str(old_json).unwrap();
        assert_eq!(cfg.volume, 1.0, "老配置缺 volume 应回落 1.0");
        // 音量夹取边界
        let v = |x: f32| PetVoiceConfig { volume: x, ..Default::default() }.clamped_volume();
        assert_eq!(v(-1.0), VOLUME_MIN);
        assert_eq!(v(0.0), 0.0);
        assert_eq!(v(0.5), 0.5);
        assert_eq!(v(1.0), 1.0);
        assert_eq!(v(5.0), VOLUME_MAX);
        // 语速夹取边界
        let s = |x: f32| PetVoiceConfig { speed: x, ..Default::default() }.clamped_speed();
        assert_eq!(s(0.1), SPEED_MIN);
        assert_eq!(s(0.5), 0.5);
        assert_eq!(s(1.0), 1.0);
        assert_eq!(s(2.0), 2.0);
        assert_eq!(s(9.0), SPEED_MAX);
    }
}
