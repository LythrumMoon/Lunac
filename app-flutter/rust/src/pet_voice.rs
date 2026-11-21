//! 桌宠语音（v1 `app/src-tauri/src/tts_zipvoice.rs` 的**配置层**）。
//!
//! **本版只做配置与资产体检，不做合成**：
//!   · v1 的合成通路是 **ZipVoice 零样本克隆**（sherpa-onnx C API 运行期加载）；
//!     运行库与模型（`sherpa-onnx-c-api.dll` / `onnxruntime.dll` / `encoder.int8.onnx` /
//!     `decoder.int8.onnx` / `vocos_24khz.onnx`）是**外部资产**，要按 `Modules/pet/zipvoice/`
//!     的布局手工放入或由插件清单下载 —— 属于「需要人工准备」的部分，按用户口径先跳过。
//!   · 但**配置必须能读写**：v1 的 `config/pet-voice.json` 字段（`enabled` / `referenceAudio` /
//!     `referenceText` / `numSteps` / `numThreads` / `speed` / `volume`）逐字保留，
//!     这样资产一旦补齐、合成通路移植后，用户已有配置直接生效。
//!
//! 纪律：坏 JSON ⇒ 默认值；越界数值夹到区间（v1 `clamped_speed` / `clamped_volume` 同口径）。
//! 资产体检要**认文件头**（PE 的 `MZ` / ONNX 的 protobuf 首字节），因为 v1 出过一次真实事故：
//! `vocos_24khz.onnx` 被镜像站的 HTML 落地页顶掉，尺寸还对得上，喂进 ONNX Runtime 直接带走进程。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 语音引擎：2026-11-21 起默认 GPT-SoVITS（V5 / gsv.cpp HTTP 服务）。
pub const ENGINE_GSV: &str = "gpt_sovits";
/// 旧引擎（ZipVoice）；v2 仍只保存配置，合成未移植。
pub const ENGINE_ZIPVOICE: &str = "zipvoice";
/// gsv.cpp 默认本地端口（`gsv_server --port 9880`）。
pub const GSV_PORT_DEFAULT: u16 = 9880;
const GSV_SAMPLE_STEPS_DEFAULT: i32 = 4;
const GSV_TEXT_LANG_DEFAULT: &str = "all_zh";

/// 语速区间（与 v1 `SPEED_MIN..=SPEED_MAX` 一致）。
pub const SPEED_MIN: f32 = 0.5;
pub const SPEED_MAX: f32 = 2.0;
/// 音量区间：0 = 静音、1 = 原始（>1 会削顶失真，v1 同口径）。
pub const VOLUME_MIN: f32 = 0.0;
pub const VOLUME_MAX: f32 = 1.0;
/// 流匹配步数 / 后端线程数（v1 缺省 8 / 4）。
const STEPS_DEFAULT: i32 = 8;
const THREADS_DEFAULT: i32 = 4;
const STEPS_RANGE: (i32, i32) = (1, 32);
const THREADS_RANGE: (i32, i32) = (1, 16);

/// v1 `PetVoiceConfig`（字段名与语义逐字一致）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PetVoiceConfig {
    pub enabled: bool,
    /// `gpt_sovits`（默认）/ `zipvoice`。
    #[serde(default = "default_engine")]
    pub engine: String,
    /// GPT-SoVITS / gsv.cpp 的本地 HTTP 地址（`POST /tts`）。
    #[serde(default = "default_server_url")]
    pub server_url: String,
    /// v5turbo 建议 4 步；v5dev 32 步。
    #[serde(default = "default_gsv_steps")]
    pub sample_steps: i32,
    /// 参考音频 / 目标文本的语言：`all_zh` / `all_en` / `auto`。
    #[serde(default = "default_text_lang")]
    pub text_lang: String,
    /// 参考音频绝对路径（空 = 未配置 ⇒ 不朗读）。
    pub reference_audio: String,
    /// 参考音频对应的那句话（GSV / ZipVoice 都需要）。
    pub reference_text: String,
    pub num_steps: i32,
    pub num_threads: i32,
    pub speed: f32,
    pub volume: f32,
}

fn default_engine() -> String {
    ENGINE_GSV.to_string()
}

fn default_server_url() -> String {
    format!("http://127.0.0.1:{GSV_PORT_DEFAULT}")
}

fn default_gsv_steps() -> i32 {
    GSV_SAMPLE_STEPS_DEFAULT
}

fn default_text_lang() -> String {
    GSV_TEXT_LANG_DEFAULT.to_string()
}

impl Default for PetVoiceConfig {
    fn default() -> Self {
        Self {
            // v1 缺省 true：资产与参考音频齐备就自动生效。
            enabled: true,
            engine: default_engine(),
            server_url: default_server_url(),
            sample_steps: default_gsv_steps(),
            text_lang: default_text_lang(),
            reference_audio: String::new(),
            reference_text: String::new(),
            num_steps: STEPS_DEFAULT,
            num_threads: THREADS_DEFAULT,
            speed: 1.0,
            volume: 1.0,
        }
    }
}

impl PetVoiceConfig {
    fn normalize(mut self) -> Self {
        self.engine = match self.engine.trim().to_ascii_lowercase().as_str() {
            ENGINE_ZIPVOICE => ENGINE_ZIPVOICE.to_string(),
            _ => ENGINE_GSV.to_string(),
        };
        self.server_url = self.server_url.trim().trim_end_matches('/').to_string();
        if self.server_url.is_empty() {
            self.server_url = default_server_url();
        }
        self.text_lang = match self.text_lang.trim() {
            "all_en" => "all_en".to_string(),
            "auto" => "auto".to_string(),
            _ => GSV_TEXT_LANG_DEFAULT.to_string(),
        };
        self.sample_steps = clamp_int(self.sample_steps, (1, 32), GSV_SAMPLE_STEPS_DEFAULT);
        self.reference_audio = self.reference_audio.trim().to_string();
        self.reference_text = self.reference_text.trim().to_string();
        self.num_steps = clamp_int(self.num_steps, STEPS_RANGE, STEPS_DEFAULT);
        self.num_threads = clamp_int(self.num_threads, THREADS_RANGE, THREADS_DEFAULT);
        self.speed = if self.speed.is_finite() {
            self.speed.clamp(SPEED_MIN, SPEED_MAX)
        } else {
            1.0
        };
        self.volume = if self.volume.is_finite() {
            self.volume.clamp(VOLUME_MIN, VOLUME_MAX)
        } else {
            1.0
        };
        self
    }
}

fn clamp_int(value: i32, range: (i32, i32), fallback: i32) -> i32 {
    if value <= 0 {
        return fallback;
    }
    value.clamp(range.0, range.1)
}

/// f32 → 两位小数的 JSON 数值：`1.6f32` 直接序列化会变成 `1.600000023841858`，
/// 面板拉条 / 数字框会显示一长串尾数。
fn round2(value: f32) -> f64 {
    ((value as f64) * 100.0).round() / 100.0
}

/// 配置落点：`<exe 根>\config\pet-voice.json`（与 v1 同一个文件）。
pub fn config_path(root: &Path) -> PathBuf {
    root.join("config").join("pet-voice.json")
}

pub fn load(root: &Path) -> PetVoiceConfig {
    match std::fs::read_to_string(config_path(root)) {
        Ok(text) => parse(&text),
        Err(_) => PetVoiceConfig::default(),
    }
}

pub fn parse(text: &str) -> PetVoiceConfig {
    serde_json::from_str::<PetVoiceConfig>(text)
        .map(PetVoiceConfig::normalize)
        .unwrap_or_default()
}

pub fn save(root: &Path, config: &PetVoiceConfig) -> Result<(), String> {
    let path = config_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建 config 目录失败：{e}"))?;
    }
    let text = serde_json::to_string_pretty(config)
        .map_err(|e| format!("序列化语音配置失败：{e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写 config/pet-voice.json 失败：{e}"))
}

// ══════════════════════════════════════════════════════════════════
//  资产体检（v1 `dir_assets_ready` 同口径，只认文件头）
// ══════════════════════════════════════════════════════════════════

/// 资产目录：`<exe 根>\Modules\pet\zipvoice`（v1 由桌宠插件清单装入，宿主只读）。
pub fn assets_dir(root: &Path) -> PathBuf {
    root.join("Modules").join("pet").join("zipvoice")
}

/// 必须存在的五个文件（相对资产目录）。
const REQUIRED_ASSETS: [&str; 5] = [
    "sherpa-onnx-c-api.dll",
    "onnxruntime.dll",
    "model/encoder.int8.onnx",
    "model/decoder.int8.onnx",
    "vocos_24khz.onnx",
];

fn starts_with(path: &Path, magic: &[u8]) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = vec![0u8; magic.len()];
    match file.read_exact(&mut head) {
        Ok(()) => head == magic,
        Err(_) => false,
    }
}

/// PE 可执行文件（DLL）认 `MZ`；ONNX 是 protobuf，首字节是 field 1（`0x08`）。
fn asset_ok(path: &Path) -> bool {
    if path.extension().and_then(|ext| ext.to_str()) == Some("onnx") {
        starts_with(path, &[0x08])
    } else {
        starts_with(path, b"MZ")
    }
}

/// 缺哪些资产（空 = 齐备）。
pub fn missing_assets(root: &Path) -> Vec<String> {
    let dir = assets_dir(root);
    REQUIRED_ASSETS
        .iter()
        .filter(|name| !asset_ok(&dir.join(name)))
        .map(|name| (*name).to_string())
        .collect()
}

/// 给面板的状态：配置 + 资产/服务体检 + **一句人话的原因**。
pub fn status_json(root: &Path) -> String {
    let config = load(root);
    let gsv = config.engine == ENGINE_GSV;
    let server_ready = gsv && server_reachable(&config.server_url);
    let reference_file_ready =
        !config.reference_audio.is_empty() && Path::new(&config.reference_audio).is_file();
    let missing = if gsv { Vec::new() } else { missing_assets(root) };
    let assets_ready = if gsv { server_ready } else { missing.is_empty() };
    let (reason, reason_text) = if gsv {
        if !server_ready {
            (
                "gsv_server_unreachable",
                format!(
                    "GPT-SoVITS 服务未连通（{}）。先启动 gsv.cpp 的 Start GAG.bat / gsv_server，再点试听。",
                    config.server_url
                ),
            )
        } else if config.reference_audio.is_empty() {
            (
                "no_reference",
                "服务已连通，但还没导入参考音色（3~10 秒录音 + 对应的那句话）。".to_string(),
            )
        } else if !reference_file_ready {
            (
                "reference_missing",
                format!("参考音频文件不存在：{}", config.reference_audio),
            )
        } else if config.reference_text.is_empty() {
            (
                "no_reference_text",
                "请填写参考录音里说的那句话（prompt_text，GPT-SoVITS 必需）。".to_string(),
            )
        } else {
            ("ready", "GPT-SoVITS 已就绪，可以试听。".to_string())
        }
    } else if !assets_ready {
        (
            "assets_missing",
            format!(
                "语音资产缺失或损坏：{}。把 ZipVoice 运行库与模型放到 {} 后可用（本版暂未移植合成通路）。",
                missing.join(" / "),
                assets_dir(root).display()
            ),
        )
    } else if config.reference_audio.is_empty() {
        (
            "no_reference",
            "资产齐备，但还没导入参考音色（一段 3~10 秒的录音 + 对应的那句话）。".to_string(),
        )
    } else {
        (
            "synthesis_not_ported",
            "ZipVoice 资产齐备，但本版还没移植 ZipVoice 合成通路：配置会保存，合成暂不可用。".to_string(),
        )
    };
    let can_speak = config.enabled
        && gsv
        && server_ready
        && reference_file_ready
        && !config.reference_text.is_empty();
    serde_json::json!({
        "enabled": config.enabled,
        "engine": config.engine,
        "serverUrl": config.server_url,
        "sampleSteps": config.sample_steps,
        "textLang": config.text_lang,
        "referenceAudio": config.reference_audio,
        "referenceText": config.reference_text,
        "numSteps": config.num_steps,
        "numThreads": config.num_threads,
        "speed": round2(config.speed),
        "volume": round2(config.volume),
        "assetsReady": assets_ready,
        "serverReady": server_ready,
        "assetDir": if gsv {
            root.join("Modules").join("pet").join("gsv").display().to_string()
        } else {
            assets_dir(root).display().to_string()
        },
        "missingAssets": if gsv && !server_ready {
            vec!["gsv_server (HTTP 未连通)".to_string()]
        } else {
            missing
        },
        "speedMin": SPEED_MIN,
        "speedMax": SPEED_MAX,
        "volumeMin": VOLUME_MIN,
        "volumeMax": VOLUME_MAX,
        "reason": reason,
        "reasonText": reason_text,
        "canSpeak": can_speak,
        "path": config_path(root).display().to_string(),
    })
    .to_string()
}

fn server_reachable(url: &str) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let port = parsed.port_or_known_default().unwrap_or(80);
    let Ok(addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    addrs.into_iter().any(|addr| {
        TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
    })
}

/// 局部更新（只改 patch 里出现的键，其余保留）。返回 `{ok, error, status}`。
pub fn set_json(root: &Path, patch: &str) -> String {
    let mut current = load(root);
    let value: serde_json::Value = match serde_json::from_str(patch) {
        Ok(value) => value,
        Err(e) => {
            return serde_json::json!({ "ok": false, "error": format!("语音配置不是合法 JSON：{e}") })
                .to_string()
        }
    };
    if let Some(object) = value.as_object() {
        if let Some(enabled) = object.get("enabled").and_then(|v| v.as_bool()) {
            current.enabled = enabled;
        }
        if let Some(text) = object.get("engine").and_then(|v| v.as_str()) {
            current.engine = text.to_string();
        }
        if let Some(text) = object.get("serverUrl").and_then(|v| v.as_str()) {
            current.server_url = text.to_string();
        }
        if let Some(steps) = object.get("sampleSteps").and_then(|v| v.as_i64()) {
            current.sample_steps = steps as i32;
        }
        if let Some(text) = object.get("textLang").and_then(|v| v.as_str()) {
            current.text_lang = text.to_string();
        }
        if let Some(text) = object.get("referenceAudio").and_then(|v| v.as_str()) {
            current.reference_audio = text.to_string();
        }
        if let Some(text) = object.get("referenceText").and_then(|v| v.as_str()) {
            current.reference_text = text.to_string();
        }
        if let Some(steps) = object.get("numSteps").and_then(|v| v.as_i64()) {
            current.num_steps = steps as i32;
        }
        if let Some(threads) = object.get("numThreads").and_then(|v| v.as_i64()) {
            current.num_threads = threads as i32;
        }
        if let Some(speed) = object.get("speed").and_then(|v| v.as_f64()) {
            current.speed = speed as f32;
        }
        if let Some(volume) = object.get("volume").and_then(|v| v.as_f64()) {
            current.volume = volume as f32;
        }
    }
    // 归一化（夹区间 / 去空白）在落盘前做一次，读回来的状态就是生效值。
    let normalized = parse(&serde_json::to_string(&current).unwrap_or_default());
    if let Err(reason) = save(root, &normalized) {
        return serde_json::json!({ "ok": false, "error": reason }).to_string();
    }
    let status: serde_json::Value =
        serde_json::from_str(&status_json(root)).unwrap_or_else(|_| serde_json::json!({}));
    serde_json::json!({ "ok": true, "error": "", "status": status }).to_string()
}

/// 「选择参考音频」：复用宿主已有的音频文件选择器（单选，取第一条）。
pub fn pick_audio_json() -> String {
    let path = crate::convert::pick_inputs("audio")
        .into_iter()
        .next()
        .unwrap_or_default();
    serde_json::json!({
        "ok": !path.is_empty(),
        "canceled": path.is_empty(),
        "path": path,
    })
    .to_string()
}

/// 朗读一段文本（GPT-SoVITS / gsv.cpp HTTP）。
///
/// **立即返回**：合成 + 播放整段丢到后台线程（单条可能要几百毫秒到几秒）。
/// 参数不完整 / 服务未连通时返回可读错误，供「试听」按钮当场提示。
pub fn speak_json(root: &Path, text: &str) -> String {
    let config = load(root);
    let text = text.trim();
    if !config.enabled {
        return serde_json::json!({ "ok": false, "error": "语音朗读已关闭" }).to_string();
    }
    if text.is_empty() {
        return serde_json::json!({ "ok": false, "error": "没有要朗读的文本" }).to_string();
    }
    if config.engine != ENGINE_GSV {
        return serde_json::json!({
            "ok": false,
            "error": "当前引擎不是 GPT-SoVITS（ZipVoice 合成通路尚未移植）",
        })
        .to_string();
    }
    if config.reference_audio.is_empty() || !Path::new(&config.reference_audio).is_file() {
        return serde_json::json!({ "ok": false, "error": "参考音频未导入或文件不存在" })
            .to_string();
    }
    if config.reference_text.is_empty() {
        return serde_json::json!({ "ok": false, "error": "请填写参考音频对应的文本" })
            .to_string();
    }
    if !server_reachable(&config.server_url) {
        return serde_json::json!({
            "ok": false,
            "error": format!("GPT-SoVITS 服务未连通：{}（先启动 Start GAG.bat / gsv_server）", config.server_url),
        })
        .to_string();
    }
    let root = root.to_path_buf();
    let text = text.to_string();
    std::thread::spawn(move || match synthesize_and_play(&root, &config, &text) {
        Ok(path) => crate::log::info(format!("[pet-tts] 已播放：{}", path.display())),
        Err(reason) => crate::log::warn(format!("[pet-tts] 合成/播放失败：{reason}")),
    });
    serde_json::json!({ "ok": true, "queued": true }).to_string()
}

fn synthesize_and_play(
    root: &Path,
    config: &PetVoiceConfig,
    text: &str,
) -> Result<PathBuf, String> {
    let bytes = synthesize_wav(config, text)?;
    let dir = root.join("temp").join("pet-tts");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建朗读临时目录失败：{e}"))?;
    let path = dir.join(format!("pet-{}.wav", crate::cron::now_ms()));
    std::fs::write(&path, bytes).map_err(|e| format!("写朗读音频失败：{e}"))?;
    play_wav(&path, config.volume)?;
    Ok(path)
}

fn synthesize_wav(config: &PetVoiceConfig, text: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .no_proxy()
        .build()
        .map_err(|e| format!("创建 TTS 客户端失败：{e}"))?;
    let url = format!("{}/tts", config.server_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "text": text,
        "text_lang": config.text_lang,
        "ref_audio_path": config.reference_audio,
        "prompt_text": config.reference_text,
        "prompt_lang": config.text_lang,
        "sample_steps": config.sample_steps,
    });
    let response = client
        .post(url)
        .json(&body)
        .send()
        .map_err(|e| format!("请求 GPT-SoVITS 失败：{e}"))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .map_err(|e| format!("读取 GPT-SoVITS 响应失败：{e}"))?;
    if !status.is_success() {
        let snippet = String::from_utf8_lossy(&bytes[..bytes.len().min(300)]);
        return Err(format!("GPT-SoVITS 返回 {}：{}", status.as_u16(), snippet));
    }
    if !bytes.starts_with(b"RIFF") || bytes.len() < 44 {
        return Err("GPT-SoVITS 返回的不是 WAV（检查 gsv_server 是否正常）".to_string());
    }
    Ok(bytes.to_vec())
}

/// 用 rodio 播一段 WAV（一次性 sink，播完即弃；与音乐播放互不干扰）。
fn play_wav(path: &Path, volume: f32) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开音频失败：{e}"))?;
    let decoder = rodio::Decoder::try_from(file).map_err(|e| format!("解码音频失败：{e}"))?;
    let mut device = rodio::DeviceSinkBuilder::open_default_sink()
        .map_err(|_| "没有可用的音频输出设备".to_string())?;
    device.log_on_drop(false);
    let player = rodio::Player::connect_new(device.mixer());
    player.set_volume(volume.clamp(VOLUME_MIN, VOLUME_MAX));
    player.append(decoder);
    player.sleep_until_end();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-pet-voice-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn defaults_match_v1_and_clamp_out_of_range_values() {
        let config = PetVoiceConfig::default();
        assert!(config.enabled);
        assert_eq!(config.engine, ENGINE_GSV);
        assert_eq!(config.server_url, "http://127.0.0.1:9880");
        assert_eq!(config.sample_steps, 4);
        assert_eq!(config.text_lang, "all_zh");
        assert_eq!(config.speed, 1.0);
        assert_eq!(config.volume, 1.0);
        assert_eq!(config.num_steps, 8);
        assert_eq!(config.num_threads, 4);

        let parsed = parse(
            r#"{"enabled":false,"speed":9.5,"volume":-3,"numSteps":999,"numThreads":0,
                "referenceAudio":"  D:/a.wav  ","referenceText":"  你好  "}"#,
        );
        assert!(!parsed.enabled);
        assert_eq!(parsed.speed, SPEED_MAX);
        assert_eq!(parsed.volume, VOLUME_MIN);
        assert_eq!(parsed.num_steps, 32);
        // 0 / 负数 ⇒ 回落默认（v1 clampInt 口径）。
        assert_eq!(parsed.num_threads, THREADS_DEFAULT);
        assert_eq!(parsed.reference_audio, "D:/a.wav");
        assert_eq!(parsed.reference_text, "你好");
    }

    #[test]
    fn broken_json_falls_back_to_defaults() {
        let parsed = parse("{not json");
        assert!(parsed.enabled);
        assert_eq!(parsed.speed, 1.0);
    }

    #[test]
    fn save_and_load_round_trip() {
        let root = temp_root("round-trip");
        let mut config = PetVoiceConfig::default();
        config.enabled = false;
        config.speed = 1.4;
        config.volume = 0.35;
        config.reference_text = "这是参考音色".to_string();
        save(&root, &config).unwrap();
        let loaded = load(&root);
        assert!(!loaded.enabled);
        assert_eq!(loaded.speed, 1.4);
        assert_eq!(loaded.volume, 0.35);
        assert_eq!(loaded.reference_text, "这是参考音色");
        assert!(config_path(&root).ends_with("config\\pet-voice.json"));
    }

    #[test]
    fn gpt_sovits_status_checks_server_and_reference() {
        use std::net::TcpListener;
        let root = temp_root("gsv-status");
        // 默认引擎 = GPT-SoVITS；服务没起 ⇒ gsv_server_unreachable。
        let status: serde_json::Value = serde_json::from_str(&status_json(&root)).unwrap();
        assert_eq!(status["engine"], ENGINE_GSV);
        assert_eq!(status["reason"], "gsv_server_unreachable");
        assert_eq!(status["canSpeak"], false);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let audio = root.join("ref.wav");
        std::fs::write(&audio, b"RIFF").unwrap();
        let audio_json = audio.display().to_string().replace('\\', "\\\\");
        let patch = format!(
            r#"{{"serverUrl":"http://127.0.0.1:{port}","referenceAudio":"{audio_json}","referenceText":"参考文本"}}"#
        );
        let reply: serde_json::Value = serde_json::from_str(&set_json(&root, &patch)).unwrap();
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["status"]["serverReady"], true);
        assert_eq!(reply["status"]["reason"], "ready");
        assert_eq!(reply["status"]["canSpeak"], true);
        drop(listener);
    }

    #[test]
    fn gsv_request_uses_expected_payload_and_accepts_wav() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::mpsc;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel::<String>();
        let handle = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let n = socket.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let _ = tx.send(request);
            let mut wav = b"RIFF".to_vec();
            wav.extend_from_slice(&[0u8; 40]);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                wav.len()
            );
            let _ = socket.write_all(head.as_bytes());
            let _ = socket.write_all(&wav);
        });

        let mut config = PetVoiceConfig::default();
        config.server_url = format!("http://127.0.0.1:{port}");
        config.reference_audio = "D:/ref.wav".to_string();
        config.reference_text = "参考文本".to_string();
        let bytes = synthesize_wav(&config, "你好").unwrap();
        assert!(bytes.starts_with(b"RIFF"));
        let request = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(request.contains("/tts"), "{request}");
        assert!(request.contains("\"text\":\"你好\""), "{request}");
        assert!(request.contains("\"sample_steps\":4"), "{request}");
        assert!(request.contains("\"ref_audio_path\":\"D:/ref.wav\""), "{request}");
        assert!(request.contains("\"prompt_text\":\"参考文本\""), "{request}");
        handle.join().unwrap();
    }

    #[test]
    fn zipvoice_assets_are_still_reported_for_legacy_configs() {
        let root = temp_root("assets");
        let dir = assets_dir(&root);
        std::fs::create_dir_all(dir.join("model")).unwrap();
        // 空目录：五个都缺。
        assert_eq!(missing_assets(&root).len(), 5);

        let mut pe = b"MZ".to_vec();
        pe.extend_from_slice(&[0u8; 64]);
        let mut onnx = vec![0x08u8];
        onnx.extend_from_slice(&[0u8; 64]);
        let write = |rel: &str, bytes: &[u8]| {
            std::fs::write(dir.join(rel), bytes).unwrap();
        };
        write("sherpa-onnx-c-api.dll", &pe);
        write("onnxruntime.dll", &pe);
        write("model/encoder.int8.onnx", &onnx);
        write("model/decoder.int8.onnx", &onnx);
        // vocos 被 HTML 落地页顶掉（v1 真实事故形状）⇒ 仍算缺。
        write("vocos_24khz.onnx", b"<!DOCTYPE html><html>");
        assert_eq!(missing_assets(&root), vec!["vocos_24khz.onnx".to_string()]);

        write("vocos_24khz.onnx", &onnx);
        assert!(missing_assets(&root).is_empty());
    }

    #[test]
    fn status_reports_reason_keys_in_v1_order() {
        let root = temp_root("status");
        // 旧引擎路径仍可读：显式选 zipvoice 后再检查资产。
        let reply: serde_json::Value =
            serde_json::from_str(&set_json(&root, r#"{"engine":"zipvoice"}"#)).unwrap();
        assert_eq!(reply["status"]["engine"], "zipvoice");
        // ① 资产缺失 ⇒ assets_missing
        let status: serde_json::Value =
            serde_json::from_str(&status_json(&root)).expect("status 应是合法 JSON");
        assert_eq!(status["reason"], "assets_missing");
        assert_eq!(status["canSpeak"], false);
        assert!(status["reasonText"]
            .as_str()
            .unwrap_or_default()
            .contains("资产"));

        let dir = assets_dir(&root);
        std::fs::create_dir_all(dir.join("model")).unwrap();
        let mut pe = b"MZ".to_vec();
        pe.extend_from_slice(&[0u8; 8]);
        let mut onnx = vec![0x08u8];
        onnx.extend_from_slice(&[0u8; 8]);
        for rel in [
            "sherpa-onnx-c-api.dll",
            "onnxruntime.dll",
            "model/encoder.int8.onnx",
            "model/decoder.int8.onnx",
            "vocos_24khz.onnx",
        ] {
            let bytes = if rel.ends_with(".onnx") {
                onnx.clone()
            } else {
                pe.clone()
            };
            std::fs::write(dir.join(rel), bytes).unwrap();
        }
        // ② 资产齐备但没参考音色 ⇒ no_reference
        let status: serde_json::Value = serde_json::from_str(&status_json(&root)).unwrap();
        assert_eq!(status["reason"], "no_reference");
        assert_eq!(status["assetsReady"], true);

        // ③ 参考音色也有 ⇒ synthesis_not_ported（本版如实登记）
        let reply: serde_json::Value = serde_json::from_str(&set_json(
            &root,
            r#"{"referenceAudio":"D:/ref.wav","referenceText":"参考","speed":1.6}"#,
        ))
        .unwrap();
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["status"]["reason"], "synthesis_not_ported");
        assert_eq!(reply["status"]["speed"], 1.6);
        let saved = load(&root);
        assert_eq!(saved.reference_audio, "D:/ref.wav");
    }
}
