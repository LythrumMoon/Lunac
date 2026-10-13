// ── tts_zipvoice.rs — 桌宠语音的**合成通路**（2026-10-10）────────────────────
//
// **引擎：ZipVoice 零样本克隆** —— 给一段 **3~10 秒**参考音频 + 那句参考文本，就能用该音色朗读任意
// 文本（上限 10 秒见 `REF_MAX_SECS`；参考音频导入时**重采样 24 kHz 单声道**，见 `REF_TARGET_SR`）。
// **合成期吃参考音频** ⇒ 参考音频规格越界会触发 sherpa 内部 AV（见规模守卫）。（2026-10-12 起去掉了
// WinRT 系统音色兜底，见 `tts.rs` 文件头。）
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
// **音色库缓存（2026-10-12）**：用户经设置窗导入的参考音频，会被**规整成 24 kHz 单声道 PCM16 WAV**
// （重采样到模型原生率，见 `REF_TARGET_SR`）复制进 `config\pet-voices\`（见 `import_pet_voice_audio`）
// —— 原文件被移动 / 删除后朗读照常。目录卫生：只保留最近一次导入的那一份副本（见 `prune_voice_cache`）。
//
// **规模守卫（2026-10-13）**：参考音频**规格越界**（时长 / 样本数超限，或磁盘上残留的旧缓存如
// 192 kHz × 29.2 s）一律在**进引擎前**挡掉 —— 超规格输入会让 sherpa 内部在崩溃性内存拷贝上
// `0xc0000005` AV。见 `REF_MAX_SECS` / `MAX_REF_SAMPLES` 与 `ref_exceeds_scale`。

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::ffi::{c_char, c_float, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use libloading::os::windows::{
    Library as WinLibrary, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
};
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

// ⚠️ **ABI 保留（勿删）**：本结构是 sherpa-onnx 1.13.8 `SherpaOnnxOfflineTtsModelConfig` 的一个字段
// （逐字段抄自官方 bindgen 产物，**内存布局必须一致**）。本宿主已不使用 Supertonic 引擎，但**仍须保留
// 此字段**并在装配配置时置零（`mem::zeroed()`）—— 否则 `OfflineTtsModelConfig` 整体尺寸 / 偏移与 DLL
// 期望不符，`SherpaOnnxCreateOfflineTts` 会读到错位内存而崩溃。
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

// Win32 裸 FFI（kernel32）：确定性加载 DLL + 「实际加载了哪份依赖」的探针。
// ⚠️ 同一个 Win32 符号在本 crate 里**只能有一种声明**（见 `win_proc.rs` 顶部）——
// 以下符号全仓仅此处声明。
#[link(name = "kernel32")]
extern "system" {
    /// 取**已加载**模块的句柄（未加载返回 NULL，**不触发加载** ⇒ 探针里是安全只读操作）。
    fn GetModuleHandleW(lpModuleName: *const u16) -> *mut c_void;
    /// 取模块**全路径**；`nSize` 是 `lpFilename` 的 u16 个数，返回写入的字符数（不含结尾 NUL）。
    fn GetModuleFileNameW(hModule: *mut c_void, lpFilename: *mut u16, nSize: u32) -> u32;
    /// 取已加载模块的导出符号地址。
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const u8) -> *mut c_void;
}

// ONNX Runtime 的 C API 基址表 —— 仅探针用来读版本。
// C 定义：`const OrtApiBase* OrtGetApiBase(void);`，
// `struct OrtApiBase { const OrtApi*(*GetApi)(uint32_t); const char*(*GetVersionString)(void); }`。
// `get_api` 字段虽不用，但保留它才能让 `get_version_string` 落在正确偏移上。
#[repr(C)]
struct OrtApiBase {
    get_api: unsafe extern "system" fn(u32) -> *const c_void,
    get_version_string: unsafe extern "system" fn() -> *const c_char,
}
type OrtGetApiBaseFn = unsafe extern "system" fn() -> *const OrtApiBase;

/// 取**已加载**模块的绝对路径（`GetModuleHandleW` + `GetModuleFileNameW`）。取不到返回 `None`。
unsafe fn loaded_module_path(name: &str) -> Option<String> {
    let mut wide: Vec<u16> = name.encode_utf16().collect();
    wide.push(0);
    let hmod = GetModuleHandleW(wide.as_ptr());
    if hmod.is_null() {
        return None;
    }
    // 32768 = 长路径上限（含 `\\?\` 前缀），足够容纳任何模块路径。
    let mut buf = vec![0u16; 32768];
    let n = GetModuleFileNameW(hmod, buf.as_mut_ptr(), buf.len() as u32);
    if n == 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(String::from_utf16_lossy(&buf))
}

/// 经**已加载**的模块取 ORT 版本串（`OrtGetApiBase()->GetVersionString()`）。
/// 任何一步拿不到（模块未加载 / 无该导出 / 返回空指针）一律 `None` —— **不 panic**。
unsafe fn loaded_ort_version(name: &str) -> Option<String> {
    let mut wide: Vec<u16> = name.encode_utf16().collect();
    wide.push(0);
    let hmod = GetModuleHandleW(wide.as_ptr());
    if hmod.is_null() {
        return None;
    }
    let proc = GetProcAddress(hmod, b"OrtGetApiBase\0".as_ptr());
    if proc.is_null() {
        return None;
    }
    // 与 `OrtGetApiBase` 的 C 签名一致：返回 `const OrtApiBase*`。
    let get_api_base: OrtGetApiBaseFn = std::mem::transmute(proc);
    let base = get_api_base();
    if base.is_null() {
        return None;
    }
    let ver = ((*base).get_version_string)();
    if ver.is_null() {
        return None;
    }
    Some(CStr::from_ptr(ver).to_string_lossy().into_owned())
}

/// **探针 1（常驻，诊断价值高，仅一行日志）**：打印**实际**被映射进来的 `onnxruntime.dll`
/// 全路径与版本。
///
/// 动机（2026-10-13）：机器上有两份 `onnxruntime.dll`（随包 ORT 1.28.2 / System32 ORT 1.17），
/// sherpa 1.13.8 要求 **ORT ≥ 1.26**。加载已走确定性方式（见 `create_session`），但「随包
/// 那一份到底有没有被选中」只能靠运行时证据 —— 这里把它落成日志。任何字段取不到打 `n/a`。
fn log_loaded_onnxruntime() {
    let (path, version) = unsafe {
        (
            loaded_module_path("onnxruntime.dll"),
            loaded_ort_version("onnxruntime.dll"),
        )
    };
    crate::log::info(format!(
        "zipvoice: onnxruntime path={} version={}",
        path.as_deref().unwrap_or("n/a"),
        version.as_deref().unwrap_or("n/a"),
    ));
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

/// 读文件**前 `n` 字节**（不整读大文件 —— decoder 有 124 MB）。读不满 / 打不开返回 `None`。
fn read_head(p: &Path, n: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut buf = vec![0u8; n];
    let mut got = 0usize;
    while got < n {
        match f.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(k) => got += k,
            Err(_) => return None,
        }
    }
    buf.truncate(got);
    Some(buf)
}

/// 下限体积：**只**用来拦「空 / 近空的残file」（如只落了 2 字节 `MZ`），**不设业务下限**
/// （真实件：`sherpa-onnx-c-api.dll` 4.6 MB / `onnxruntime.dll` 17.8 MB / 三个 onnx 均 > 5 MB）。
const ASSET_MIN_BYTES: u64 = 1024;

/// 文件体积过下限（拦空 / 近空残file）。`is_pe` / `is_onnx` 共用这一个判据。
fn size_at_least(p: &Path, min: u64) -> bool {
    std::fs::metadata(p).map(|m| m.len() >= min).unwrap_or(false)
}

/// 是不是可加载的 **PE**（DLL）：以 `MZ`（`4D 5A`）开头且体积过下限。
fn is_pe(p: &Path) -> bool {
    size_at_least(p, ASSET_MIN_BYTES) && matches!(read_head(p, 2), Some(b) if b.as_slice() == b"MZ")
}

/// 是不是 **ONNX** 模型：`ModelProto` 首字段是 `ir_version`（field 1, varint）⇒ 首字节恒为 **0x08**；
/// 再要体积过下限，排除空 / 截断残file。
fn is_onnx(p: &Path) -> bool {
    size_at_least(p, ASSET_MIN_BYTES) && matches!(read_head(p, 1), Some(b) if b.first() == Some(&0x08u8))
}

/// 指定目录下的 ZipVoice 资产是否**可用**（两个 DLL 是 PE、三个 onnx 是 ONNX，且都非近空）。
///
/// ⚠️ **为什么查头字节、而不是只查「文件在不在」**（2026-10-13）：只判存在会把**被污染 / 截断**
/// 的资产当成可用 —— 例：gh-proxy 之类的 GitHub 加速镜像用 HTML 落地页把 `vocos_24khz.onnx` 顶掉，
/// **总尺寸还与上游完全一致**（2026-10-12 实测事故）。这种坏文件喂进 ONNX Runtime ⇒ 它抛的 MSVC
/// C++ 异常（`0xe06d7363`）**跨 FFI 边界没有 Rust 侧的捕获点**（`catch_unwind` 只接 Rust panic）
/// ⇒ **宿主进程被直接带走**（用户报的「点试听就闪退」）。这里当场判「未就绪」，把崩溃换成一条
/// 可读的「资产缺失或损坏」（`reason_for` → `assets_missing`）。
fn dir_assets_ready(dir: &Path) -> bool {
    is_pe(&dir.join("sherpa-onnx-c-api.dll"))
        && is_pe(&dir.join("onnxruntime.dll"))
        && is_onnx(&dir.join("model").join("encoder.int8.onnx"))
        && is_onnx(&dir.join("model").join("decoder.int8.onnx"))
        && is_onnx(&dir.join("vocos_24khz.onnx"))
}

/// 当前资产是否齐备（供命令 / 前端看的无参入口）。
pub fn assets_ready() -> bool {
    dir_assets_ready(&zipvoice_dir())
}

/// 音色库缓存目录：`config\pet-voices\`（与 `pet-voice.json` 同层，走既有 `lunac_root_dir()`）。
/// 实体 = 用户导入的参考音色经**规整后的 PCM16 WAV 副本**（见 `import_pet_voice_audio`）。
fn voices_dir() -> PathBuf {
    crate::storage::lunac_root_dir()
        .join("config")
        .join("pet-voices")
}

/// ZipVoice 参考音频的**可用时长区间（秒）**。
///
/// 上限 **10** 的出处：官方 ZipVoice 仓库（k2-fsa/ZipVoice）README「Prompt length」写明参考 prompt
/// 应**很短**（单人 < 3 秒 / 对话 < 10 秒），「a very long prompt will slow down the inference and
/// degenerate the speech quality」；sherpa-onnx v1.13.8 官方示例音频 `leijun-1.wav` 实测为
/// **24 kHz / 单声道 / 16-bit / 6.06 秒**。取 10 秒作上限：既容纳官方 6 秒示例，也贴住官方「过长为界」。
/// 下限沿用 3 秒（原业务下限，本次不动）。
pub const REF_MIN_SECS: f64 = 3.0;
pub const REF_MAX_SECS: f64 = 10.0;

/// ZipVoice 参考音频的**目标采样率（Hz）**：模型元数据 `sample_rate = 24000`，声码器为
/// `vocos_24khz`（见文件头与官方 v1.13.8 `sherpa-onnx/csrc/offline-tts-zipvoice-model-meta-data.h`）。
/// 导入时把参考音频**重采样到此率**再落缓存（见 `import_voice_file`）。
pub const REF_TARGET_SR: i32 = 24000;

/// 送进引擎的参考音频**样本数硬上限**（兜底守卫，与 `REF_MAX_SECS` 同口径）：
/// **目标率 24 kHz × 上限 10 秒 = 240 000**。历史缓存 / 手改配置指向的超规格 wav（如用户磁盘上那条
/// 192 kHz × 29.2 s = 5 613 276 样本的旧缓存）据此在**进引擎前**挡掉（见 `ref_exceeds_scale`）。
pub const MAX_REF_SAMPLES: usize = 240_000;

/// 不可用的原因（**机器可读 key**，供前端本地化成文案）。空串 = 可用。
///
/// key 集合：`disabled` / `no_ref_audio` / `no_ref_text` / `ref_missing` / `ref_out_of_spec` /
/// `assets_missing`。前端 i18n 的 `pet.tts_reason_*` 列表需与这里对齐（见 `pet.ts` 的
/// `PET_VOICE_REASONS`）。`speak_blocking` 的**静默不读**判据就是它非空（见 `tts.rs`）。
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
    let ref_path = Path::new(cfg.reference_audio.trim());
    if !ref_path.is_file() {
        return "ref_missing".into();
    }
    // 参考音频**规格越界**（磁盘上残留的旧缓存 / 手改配置指向的超规格 wav）：按文件体积**廉价**
    // 判定（读元数据，不解码）。要求用户重新导入 —— **绝不**把它送进引擎（`speak` 侧还有一道按
    // 实际样本数的硬守卫，见 `ref_exceeds_scale`）。
    if ref_file_exceeds_scale(ref_path) {
        return "ref_out_of_spec".into();
    }
    if !assets_ready {
        return "assets_missing".into();
    }
    String::new()
}

/// 参考音频**解码后样本数**是否超出可送入引擎的上限（`MAX_REF_SAMPLES`）。这是 `speak` 的
/// **运行时硬守卫**，兜住任何人绕过导入（历史缓存 / 手改配置）直接指向超规格 wav 的情况。
fn ref_exceeds_scale(n_samples: usize) -> bool {
    n_samples > MAX_REF_SAMPLES
}

/// 参考音频**文件体积**是否超出「规整后上限」。仅用于 `reason_for` 的**廉价**越界判据（读文件
/// 元数据，不解码整个 wav）—— PCM16 WAV 体积 ≈ 44 字节头 + 每样本 2 字节，故上限取
/// `MAX_REF_SAMPLES × 2 + 4096`（余量留给可能存在的额外 RIFF chunk）。体积越界 ⇒ 该文件**不可能**
/// 是规整后的缓存（那是按导入规则生成的）⇒ 判超规格、要求重新导入。
fn ref_file_exceeds_scale(path: &Path) -> bool {
    const MAX_BYTES: u64 = (MAX_REF_SAMPLES as u64) * 2 + 4096;
    std::fs::metadata(path).map(|m| m.len() > MAX_BYTES).unwrap_or(false)
}

/// 这条通路此刻**能不能用**（给命令 / 前端看的完整判据）。
pub fn available() -> bool {
    reason_for(&load_config(), assets_ready()).is_empty()
}

/// 语音**此刻不可用**的原因 key（与 `get_pet_voice` 的 `PetVoiceStatus.reason` **同一套**，
/// 见 `reason_for`）；可用时返回 `None`。
///
/// 供**用户主动触发**的调用方（设置窗「试听」按钮，见 `tts.rs::tts_speak`）当场据此提示；
/// 桌宠**自动朗读**那条路径不看它 —— 仍是**静默不读**（用户 2026-10-12 裁决）。
pub fn unavailable_reason() -> Option<String> {
    let r = reason_for(&load_config(), assets_ready());
    if r.is_empty() {
        None
    } else {
        Some(r)
    }
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

/// 会话指纹（换了**线程数**）⇒ 作废重来。参考音频是**每次生成**的参数、不进指纹。
static TTS_FINGERPRINT: OnceLock<Mutex<String>> = OnceLock::new();

fn fingerprint_of(cfg: &PetVoiceConfig) -> String {
    format!("zipvoice:{}", cfg.num_threads)
}

/// 取（必要时**建**）常驻会话。失败不缓存 —— 用户补上文件后应当能再试。
fn ensure_session(cfg: &PetVoiceConfig) -> Result<(), String> {
    let fp = fingerprint_of(cfg);
    let fp_slot = TTS_FINGERPRINT.get_or_init(|| Mutex::new(String::new()));
    {
        let guard = tts_slot().lock().map_err(|_| "TTS 锁中毒".to_string())?;
        let cur = fp_slot.lock().map_err(|_| "TTS 锁中毒".to_string())?.clone();
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

/// 路径 → UTF-8 `CString`（含 NUL 的路径给可读错误）。**在 create 调用期间必须存活**。
fn path_cstring(x: PathBuf) -> Result<CString, String> {
    CString::new(x.to_string_lossy().as_bytes()).map_err(|e| format!("路径含 NUL：{e}"))
}

/// 加载一次 sherpa-onnx 运行库并取齐 4 个函数指针（`<pet>\zipvoice\sherpa-onnx-c-api.dll`）。
fn load_sherpa() -> Result<(Library, CreateFn, DestroyFn, GenFn, DestroyAudioFn), String> {
    let dll_path = zipvoice_dir().join("sherpa-onnx-c-api.dll");

    // **确定性加载**（2026-10-13）：走 `LoadLibraryExW(file, NULL, flags)`（经 libloading 的
    // `os::windows::Library::load_with_flags`），flags =
    // `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS`。
    // 前者把 **sherpa 所在目录**（= zipvoice 目录）置于**依赖搜索最前** ⇒ 保证 sherpa 链接的
    // `onnxruntime.dll` 从 zipvoice 目录取，而不是被 System32 里 Windows 自带的旧版（1.17，低于
    // sherpa 1.13.8 要求的 1.26）抢走；后者保留「应用目录 / System32 / 用户目录」作兜底。
    //
    // **不再用 `SetDllDirectoryW`**：它改的是**进程级**默认搜索路径（对全进程后续所有加载生效），
    // 而上面的 flags 是**单次加载**级、语义更窄更准；两套并存属语义重复，且前者带全局副作用，故移除。
    // **不调 `SetDefaultDllDirectories`**：`LOAD_LIBRARY_SEARCH_*` 在 Windows 8+（含本目标的
    // Windows 10/11）原生支持、无需前置调用；而 `SetDefaultDllDirectories` 会把**当前目录 / PATH**
    // 从全进程默认搜索路径里剔掉 —— 那会改变宿主其它组件的加载行为，超出本次范围，故不采用。
    let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS;
    let win_lib = unsafe { WinLibrary::load_with_flags(&dll_path, flags) }
        .map_err(|e| format!("加载 sherpa-onnx-c-api.dll 失败：{e}"))?;
    let lib: Library = win_lib.into();

    // 探针 1（常驻）：sherpa 已加载 ⇒ 其依赖 `onnxruntime.dll` 已解析，打印**实际**命中的那一份。
    log_loaded_onnxruntime();

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
    // 先把 4 个函数指针**拷出来**、再显式释放 `Symbol`（它们借用 `lib`）—— 否则下面把 `lib`
    // 移进返回值时会撞 `E0505`（`Symbol` 的借用尚未结束）。函数指针的有效期由 `lib` 决定。
    let fns = (*create, *destroy, *generate, *destroy_audio);
    drop((create, destroy, generate, destroy_audio));
    Ok((lib, fns.0, fns.1, fns.2, fns.3))
}

/// 建一份常驻会话 —— 装配 ZipVoice 模型配置。失败给可读错误、**不 panic**（本模块纪律）。
fn create_session(cfg: &PetVoiceConfig) -> Result<TtsSession, String> {
    let (lib, create, destroy, gen, destroy_audio) = load_sherpa()?;
    // 不用 `unwrap()`：这是**试听路径**上的 panic 点（`"cpu"` 恒无 NUL、实际不会触发，但不留
    // panic）—— 本模块纪律是「失败给可读错误，不 panic、不 abort」。
    let c_provider = CString::new("cpu").map_err(|_| "provider 名非法".to_string())?;
    let num_threads = cfg.num_threads.clamp(1, 32);

    // 各路径 CString **在 create 调用期间必须存活** ⇒ 绑在此作用域内直到 `create(&conf)` 返回。
    let dir = zipvoice_dir();
    let model = dir.join("model");
    let c_tokens = path_cstring(model.join("tokens.txt"))?;
    let c_encoder = path_cstring(model.join("encoder.int8.onnx"))?;
    let c_decoder = path_cstring(model.join("decoder.int8.onnx"))?;
    let c_vocoder = path_cstring(dir.join("vocos_24khz.onnx"))?;
    let c_data = path_cstring(model.join("espeak-ng-data"))?;
    let c_lexicon = path_cstring(model.join("lexicon.txt"))?;
    let conf = OfflineTtsConfig {
        model: OfflineTtsModelConfig {
            vits: unsafe { std::mem::zeroed() },
            num_threads,
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
            // `pocket` / `supertonic` 是本仓库不用的引擎字段，但**必须保留**（ABI：布局不能少字段，
            // 见 `OfflineTtsSupertonicModelConfig` 上的注释）—— 置零即可。
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
        return Err("创建语音会话失败（模型文件或参数有问题）".into());
    }
    Ok(TtsSession {
        handle,
        gen,
        destroy,
        destroy_audio,
        _lib: lib,
    })
}

// ══════════════════════════════════════════════════════════════════
//  朗读
// ══════════════════════════════════════════════════════════════════

/// 合成 `text` 并播放。**阻塞**（在后台线程里跑）。吃参考音频（规格越界当场挡下）。
/// `app` 只用来把**口型包络**下发给桌宠窗（见 `tts::play_wav_bytes`）。
pub fn speak(app: &tauri::AppHandle, text: &str) -> Result<(), String> {
    let cfg = load_config();
    if !cfg.enabled {
        return Err("语音朗读未启用".into());
    }
    // **资产前置守卫**（2026-10-13）：缺 / 坏资产 ⇒ 当场返回可读错误，**绝不**把它们喂进
    // ONNX Runtime（那会以未捕获的 C++ 异常把宿主进程带走，见 `dir_assets_ready`）。
    // `available()`（调用方的前置校验）已经拦过一道；这里是**直接调用 `speak`** 时的第二道保险。
    if !dir_assets_ready(&zipvoice_dir()) {
        return Err("语音组件未就绪（资产缺失或损坏）".into());
    }

    let ref_path = cfg.reference_audio.trim();
    if ref_path.is_empty() {
        return Err("未配置参考音频".into());
    }
    let (ref_samples, ref_sr) = read_wav_mono(Path::new(ref_path))?;
    // **规模守卫（兜底，治本）**：参考音频规模超出上限 ⇒ 当场返回可读原因 key，**绝不**送进引擎
    // —— 超规格输入会让 sherpa 内部在崩溃性内存拷贝上 AV（见文件头 2026-10-13 记录）。这道闸兜住
    // 「绕过导入的历史缓存 / 手改配置」；`reason_for` 侧还有一道按文件体积的廉价前置闸。
    if ref_sr <= 0 || ref_exceeds_scale(ref_samples.len()) {
        return Err("ref_out_of_spec".into());
    }
    let ref_text_c = Some(
        CString::new(cfg.reference_text.as_bytes())
            .map_err(|e| format!("参考文本含 NUL：{e}"))?,
    );
    ensure_session(&cfg)?;

    let text_c = CString::new(text.as_bytes()).map_err(|e| format!("文本含 NUL：{e}"))?;
    let gen_cfg = GenerationConfig {
        silence_scale: 0.0,
        speed: cfg.clamped_speed(),
        sid: 0,
        reference_audio: ref_samples.as_ptr(),
        reference_audio_len: ref_samples.len() as i32,
        reference_sample_rate: ref_sr,
        reference_text: ref_text_c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
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
    // 采样率由引擎返回 —— 写进 WAV 头，播放端照读。
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
//  音色库导入（任意常见音频 → 规整成 24 kHz 单声道 PCM16 WAV 存进缓存目录）
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

/// 把**单声道** f32 样本从 `src_sr` 重采样到 `dst_sr`（**线性插值**）。
///
/// 取舍：这是**导入期、一次性**的规整，线性插值实现最小、零依赖、结果确定且无重采样器状态；
/// 目标是 24 kHz 的语音（带宽 < 12 kHz），常见源（44.1k / 48k）多为整数或近似 2 倍下采样，线性
/// 插值的混叠在听感上可接受 —— 且参考音频**只用来提取说话人音色**、不决定合成音质本身。此仓库
/// 没有现成的音频重采样实现可复用（另有 `screenshot.rs` 的 `downsample_gray`，那是图像、不适用）。
/// （sherpa 内部对非目标率也走 `LinearResample`，但那是**运行期**开销；在导入期先做既省内存，
/// 也让落缓存的数据贴合模型原生率。）
///
/// `src_sr == dst_sr` / 任一采样率非法 / 空输入 ⇒ 原样返回（避免无谓拷贝与累计误差）。
fn resample_linear_mono(samples: &[f32], src_sr: i32, dst_sr: i32) -> Vec<f32> {
    if src_sr <= 0 || dst_sr <= 0 || samples.is_empty() || src_sr == dst_sr {
        return samples.to_vec();
    }
    let ratio = dst_sr as f64 / src_sr as f64;
    let out_len = (samples.len() as f64 * ratio).round() as usize;
    let last = samples.len() - 1;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        // 目标第 i 点映射回源域的位置；落在末点及其后一律取末样本（防越界）。
        let src_pos = i as f64 / ratio;
        let i0 = src_pos.floor() as usize;
        if i0 >= last {
            out.push(samples[last]);
            continue;
        }
        let frac = (src_pos - i0 as f64) as f32;
        out.push(samples[i0] + (samples[i0 + 1] - samples[i0]) * frac);
    }
    out
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

/// 字节流的 **SHA-256 十六进制小写**串（`content_name` 与自建音色落点共用）。
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// 缓存副本的文件名 = **内容哈希**（规整后 WAV 的 SHA-256）⇒ 同一段音频重复导入必落同一文件、
/// 不同音频必不互相覆盖，且天然免路径注入（文件名只含 `voice-` + 十六进制 + `.wav`）。
fn content_name(wav: &[u8]) -> String {
    format!("voice-{}.wav", sha256_hex(wav))
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
/// 校验 → 规整成 24 kHz 单声道 PCM16 WAV → 写进 `dir` → 清理旧副本 → 返回缓存副本路径。
///
/// 校验（失败各给一句可读错误）：文件不存在 / 解不出音频 / 解出来是空的 / 时长越界（3~10 秒）。
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
    // **规整**：重采样到模型原生率（24 kHz）—— `decode_audio_mono` 已把多声道混成单声道。
    // 既贴合模型元数据 `sample_rate` / 声码器 `vocos_24khz`，也避免把 44.1k / 48k / 192k 的大样本
    // 数组送进引擎（sherpa 内部虽会 `LinearResample`，但那是**运行期**开销，样本量越大越贴近它的
    // 规模红线，见 `MAX_REF_SAMPLES`）。
    let normalized = resample_linear_mono(&samples, sr, REF_TARGET_SR);
    let wav = encode_wav_pcm16(&normalized, REF_TARGET_SR);
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
/// 校验（可读 / 可解码 / 时长 3~10 秒）→ **规整成 24 kHz 单声道 PCM16 WAV** 复制进音色库缓存目录 →
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

    /// 写一段 **PCM16 单声道** WAV，样本全 0（样本量很大时比 `write_wav` 逐样本快得多）。
    fn write_wav_zeros(path: &Path, sr: u32, n: usize) {
        let data_len = (n * 2) as u32;
        let mut b = Vec::with_capacity(44 + n * 2);
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
        b.resize(44 + n * 2, 0);
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

    /// 资产头字节校验（`dir_assets_ready`）：空目录 / 缺件 / 被污染（HTML 顶替）/ 近空残file
    /// ⇒ 一律判**未就绪**（⇒ 命令返回可读错误，而**不是**把坏文件喂给 ONNX Runtime 崩掉进程）；
    /// 形状（PE / ONNX 魔数 + 体积）正确 ⇒ 就绪。
    ///
    /// 复刻 2026-10-12 的真实事故：`vocos_24khz.onnx` 被 GitHub 加速镜像的 HTML 落地页顶掉，
    /// 头几字节是 `<!DOCTYPE html>`（**总尺寸还与上游一致**）。
    #[test]
    fn dir_assets_ready_rejects_missing_and_corrupt_assets() {
        let dir = temp_root("assets");
        // ① 空目录：缺件 ⇒ 未就绪
        assert!(!dir_assets_ready(&dir), "空目录必须判未就绪");

        let write = |rel: &str, bytes: &[u8]| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes).unwrap();
        };
        let mut pe = b"MZ".to_vec();
        pe.extend_from_slice(&[0u8; 4096]);
        let mut onnx = vec![0x08u8];
        onnx.extend_from_slice(&[0u8; 4096]);
        write("sherpa-onnx-c-api.dll", &pe);
        write("onnxruntime.dll", &pe);
        write("model/encoder.int8.onnx", &onnx);
        write("model/decoder.int8.onnx", &onnx);
        // ② vocos 被 HTML 落地页污染（真实事故形状）⇒ 必须判未就绪
        write("vocos_24khz.onnx", b"<!DOCTYPE html><html lang=\"en\"><head>...");
        assert!(!dir_assets_ready(&dir), "被 HTML 污染的 onnx 必须判未就绪");
        // ③ 换成头字节合法的 vocos ⇒ 就绪（证明②拦的是内容、不是文件名）
        write("vocos_24khz.onnx", &onnx);
        assert!(dir_assets_ready(&dir), "形状正确应判就绪");
        // ④ 只落了个空壳（不足下限）⇒ 未就绪
        write("vocos_24khz.onnx", &[0x08u8]);
        assert!(!dir_assets_ready(&dir), "近空残file必须判未就绪");
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

    /// 失败分支③：时长越界（< 下限 3s、> 上限 10s）—— 都不落文件。40s、12s 都应越界。
    #[test]
    fn import_rejects_out_of_range_duration() {
        let root = temp_root("dur");
        let cache = root.join("cache");
        let short = root.join("short.wav");
        let long = root.join("long.wav");
        let too_long = root.join("too_long.wav");
        write_wav(&short, 8000, 1.0); // 1s（< 下限 3s）
        write_wav(&long, 8000, 40.0); // 40s（>> 上限）
        write_wav(&too_long, 8000, 12.0); // 12s（> 新上限 10s）
        let e1 = import_voice_file(&short, &cache).unwrap_err();
        assert!(e1.contains("时长需在"), "err={e1}");
        let e2 = import_voice_file(&long, &cache).unwrap_err();
        assert!(e2.contains("时长需在"), "err={e2}");
        let e3 = import_voice_file(&too_long, &cache).unwrap_err();
        assert!(e3.contains("时长需在"), "err={e3}");
        assert!(!cache.exists(), "越界不应落文件");
        std::fs::remove_dir_all(&root).ok();
    }

    /// **新上限边界**：刚好在上限（10.0s）⇒ 接受并落文件；刚超上限（10.1s）⇒ 拒绝且**不落文件**。
    #[test]
    fn import_accepts_at_max_rejects_just_over() {
        let root = temp_root("bound");
        let cache = root.join("cache");
        let at_max = root.join("at_max.wav");
        let over = root.join("over.wav");
        write_wav(&at_max, 8000, REF_MAX_SECS); // 10.0s，正好在限上
        write_wav(&over, 8000, REF_MAX_SECS + 0.1); // 10.1s，刚超限
        let ok = import_voice_file(&at_max, &cache).expect("刚好在上限应被接受");
        assert!(ok.is_file());
        // 规整后应为 24 kHz、约 10s，且样本数不超硬上限（不会触发规模守卫）
        let (s, sr) = read_wav_mono(&ok).unwrap();
        assert_eq!(sr, REF_TARGET_SR);
        assert!(!ref_exceeds_scale(s.len()), "上限内的样本不应判超规格");
        let before = std::fs::read_dir(&cache).unwrap().flatten().count();
        let e = import_voice_file(&over, &cache).unwrap_err();
        assert!(e.contains("时长需在"), "err={e}");
        let after = std::fs::read_dir(&cache).unwrap().flatten().count();
        assert_eq!(before, after, "越界不应落文件、不改动缓存目录");
        std::fs::remove_dir_all(&root).ok();
    }

    /// **重采样正确性**（`resample_linear_mono` + 导入规整）：48 kHz 源 → 24 kHz 输出，
    /// 时长保持 ±1%、样本数 ≈ 目标率 × 秒数、不出现整段静音 / 长度错位。
    #[test]
    fn import_resamples_to_target_rate_preserving_duration() {
        let root = temp_root("resample");
        let cache = root.join("cache");
        let src = root.join("src48.wav");
        write_wav(&src, 48000, 4.0);
        let cached = import_voice_file(&src, &cache).unwrap();
        let (s, sr) = read_wav_mono(&cached).unwrap();
        assert_eq!(sr, REF_TARGET_SR, "落缓存应为模型原生率 24 kHz");
        let secs = s.len() as f64 / sr as f64;
        assert!((secs - 4.0).abs() / 4.0 < 0.01, "时长应保持 ±1%：secs={secs}");
        let expect = REF_TARGET_SR as i64 * 4;
        assert!((s.len() as i64 - expect).abs() <= 2, "样本数应≈目标率×秒数：{}", s.len());
        assert!(s.iter().any(|&v| v.abs() > 0.1), "不应出现整段静音");
        // 纯函数直测：48k→24k 为 2:1 整数下采样，样本数应精确半减、无长度错位
        let ramp: Vec<f32> = (0..48000).map(|i| (i % 2) as f32).collect();
        let out = resample_linear_mono(&ramp, 48000, 24000);
        assert_eq!(out.len(), 24000);
        std::fs::remove_dir_all(&root).ok();
    }

    /// **规模守卫的核心回归断言**：`ref_exceeds_scale` 是 `speak` 在**进引擎前**的拦阻判据 ——
    /// 旧缓存形状（192 kHz × 29.2 s = 5 613 276 样本）必须判真；规整后上限（24 kHz × 10s =
    /// 240 000）必须判假。并校验 `MAX_REF_SAMPLES` 与「目标率 × 上限秒数」口径一致。
    #[test]
    fn scale_guard_flags_oversized_reference() {
        assert_eq!(
            MAX_REF_SAMPLES,
            REF_TARGET_SR as usize * REF_MAX_SECS as usize,
            "MAX_REF_SAMPLES 应与 目标率 × 上限秒数 一致"
        );
        assert!(ref_exceeds_scale(5_613_276), "旧缓存样本数（192kHz×29.2s）应判超规格");
        assert!(!ref_exceeds_scale(MAX_REF_SAMPLES), "上限样本本身不超");
        assert!(ref_exceeds_scale(MAX_REF_SAMPLES + 1), "刚超上限即判超规格");
    }

    /// **老数据兜底**：复刻用户磁盘上那条 192 kHz × 29.2 s 的旧缓存形状，证明 `reason_for` 判
    /// 超规格（`ref_out_of_spec`）⇒ `available()` 为假 ⇒ `tts.rs::speak_blocking` **不会**把它送进
    /// 引擎（静默跳过），设置窗则显示「请重新导入」文案。仅按文件体积（≈11.2 MB）即判，不必解码。
    #[test]
    fn old_oversized_cache_is_blocked_before_engine() {
        let dir = temp_root("oldcache");
        let old = dir.join("voice-192k-29s.wav");
        write_wav_zeros(&old, 192000, 5_613_276); // 192 kHz × 29.2 s
        let cfg = PetVoiceConfig {
            enabled: true,
            reference_audio: old.to_string_lossy().to_string(),
            reference_text: "旧缓存".into(),
            ..Default::default()
        };
        assert_eq!(reason_for(&cfg, true), "ref_out_of_spec");
        // 兜底闸同样判真（即便有人绕过体积闸直读样本，也进不了引擎）
        assert!(ref_exceeds_scale(5_613_276));
        std::fs::remove_dir_all(&dir).ok();
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
        // 缓存的是**可读回的 PCM16 WAV**，已被规整到 24 kHz，时长≈4s
        let (s, sr) = read_wav_mono(&da).unwrap();
        assert_eq!(sr, REF_TARGET_SR);
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

