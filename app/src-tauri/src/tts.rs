// ── tts.rs — 桌宠回复的**本机离线语音朗读**（2026-10-10）──────────────────────
//
// **为什么合成 + 播放都在宿主**：WebView 放不出声音（与「联网 / 音频必须走宿主」是同一条
// 纪律，见 archive 的 ai-spec §4.6）。前端只发一句话，剩下的全在这里做。
//
// **引擎：只有 ZipVoice**（桌宠自带那套零样本克隆，见 `tts_zipvoice.rs`）—— 用**用户导入的
// 参考音色**朗读任意文本。这里只做「取 WAV 字节 → 落临时文件 → 喂口型包络 → rodio 播放 →
// 删临时文件」的收尾；合成本身在 `tts_zipvoice`。
//
// **语音音量（2026-10-12）**：播放前在 `play_wav_bytes` **集中读一次** `PetVoiceConfig.volume`
// （夹取到 `VOLUME_MIN..=VOLUME_MAX`），交给 `play_wav` 用 rodio `Player::set_volume` 生效——
// 两条朗读通路都收尾到 `play_wav_bytes`，故音量对它们一致生效。
//
// **静默不读（2026-10-12 用户裁决，去掉了 WinRT 系统音色兜底）**：ZipVoice **不可用**
// （资产缺失 / 未配置参考音色 / 开关关着）或**合成失败**时 —— **不合成、不出声、不弹错**，
// 只记一行 `log::warn`。朗读是锦上添花，配不齐就静静跳过，绝不打扰桌宠的气泡显示。
// （原实现会在失败时回落 WinRT 系统音色，2026-10-12 用户明确要求整条去掉。）
//
// **线程**：合成 + 播放会阻塞，整段跑在独立后台线程上，命令不阻塞调用方。

use std::path::Path;

use rodio::Decoder;
use tauri::Emitter;

/// **口型包络**的下发事件（2026-10-10）：桌宠窗收到就按这条时间线驱动模型的嘴。
/// 由宿主在**开始播放之前**定向发给桌宠窗（WebView 放不出声音，见文件头），
/// 于是「音频在宿主放」+「口型在 WebView 演」共用同一条时间线。
pub const MOUTH_EVENT: &str = "pet-tts-mouth";
/// **「语音播完」信号**的下发事件（2026-10-13）：桌宠窗收到就把气泡自动收起的起算推迟到这一刻
/// （`pet.ts` 的 `TTS_DONE_EVENT`，**必须与前端同名**）。宿主在**一次朗读结束之后**（播完 /
/// 未就绪静默跳过 / 合成失败**都算**）定向发一条 —— 前端的逻辑因此只有一条：等它就对了。
pub const TTS_DONE_EVENT: &str = "pet-tts-done";
/// 桌宠窗的 label（宿主按 `plugin-<id>` 命名）。定向发，不给别的插件窗广播。
const PET_LABEL: &str = "plugin-pet";

/// 包络的采样率（每秒几个点）。40 与 `motionSync` 一类做法同量级，肉眼足够顺滑。
const MOUTH_FPS: u32 = 40;

/// `MOUTH_EVENT` 的载荷。`env` 是 0..=255 的「张嘴幅度」，逐格一个点。
#[derive(Clone, serde::Serialize)]
struct MouthPayload {
    fps: u32,
    env: Vec<u8>,
}

/// 极简 WAV 解析：只认 **PCM16** 与 **IEEE float32**（ZipVoice 那条通路产的是 PCM16，
/// 见 `tts_zipvoice::encode_wav_pcm16`）。返回 `(采样率, 声道数, 单声道样本)`。
///
/// 认不出 / 不是这两种编码 ⇒ `None`（那就没有口型，只剩朗读 —— 不该为锦上添花去 panic）。
fn parse_wav(bytes: &[u8]) -> Option<(u32, u16, Vec<f32>)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // (format, channels, rate, bits)
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        // 声明长度越界（坏文件 / 流式的 0xFFFFFFFF）⇒ 不再往后走；已经拿到的 fmt/data 照用。
        let Some(body) = bytes.get(pos + 8..pos + 8 + size) else {
            break;
        };
        if id == b"fmt " && size >= 16 {
            fmt = Some((
                u16::from_le_bytes([body[0], body[1]]),
                u16::from_le_bytes([body[2], body[3]]),
                u32::from_le_bytes([body[4], body[5], body[6], body[7]]),
                u16::from_le_bytes([body[14], body[15]]),
            ));
        } else if id == b"data" {
            data = Some(body);
        }
        // 块按偶数字节对齐（RIFF 规范）；`saturating_add` 防坏长度把 usize 撑溢出。
        pos = pos.saturating_add(8).saturating_add(size + (size & 1));
    }
    let (format, ch, rate, bits) = fmt?;
    let data = data?;
    if ch == 0 || rate == 0 {
        return None;
    }
    let ch = ch as usize;
    let mono: Vec<f32> = match (format, bits) {
        // 1 = PCM；只认 16 bit
        (1, 16) => data
            .chunks_exact(2 * ch)
            .map(|fr| {
                let sum: i32 = fr
                    .chunks_exact(2)
                    .map(|s| i16::from_le_bytes([s[0], s[1]]) as i32)
                    .sum();
                (sum as f32 / ch as f32) / 32768.0
            })
            .collect(),
        // 3 = IEEE float
        (3, 32) => data
            .chunks_exact(4 * ch)
            .map(|fr| {
                let sum: f32 = fr
                    .chunks_exact(4)
                    .map(|s| f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
                    .sum();
                sum / ch as f32
            })
            .collect(),
        _ => return None,
    };
    Some((rate, ch as u16, mono))
}

/// 从 WAV 字节算一条**口型包络**：每 1/`MOUTH_FPS` 秒取一个 0..=255 的「张嘴幅度」。
///
/// 做法 = 逐格取**峰值**，再按整段峰值归一化；静音段（< 6% 峰值）直接压成 0（不然嘴会半张
/// 不收），其余取一次平方根让轻声也有可见的开口。纯 CPU、几毫秒，跑在朗读那条后台线程上。
fn mouth_envelope(bytes: &[u8]) -> (u32, Vec<u8>) {
    let Some((rate, _ch, mono)) = parse_wav(bytes) else {
        return (MOUTH_FPS, Vec::new());
    };
    let per = ((rate as usize) / (MOUTH_FPS as usize)).max(1);
    let mut peaks: Vec<f32> = Vec::with_capacity(mono.len() / per + 1);
    let mut i = 0usize;
    while i < mono.len() {
        let end = (i + per).min(mono.len());
        let mut peak = 0f32;
        for &s in &mono[i..end] {
            let a = s.abs();
            if a > peak {
                peak = a;
            }
        }
        peaks.push(peak);
        i = end;
    }
    if peaks.is_empty() {
        return (MOUTH_FPS, Vec::new());
    }
    let max = peaks.iter().copied().fold(0f32, f32::max).max(1e-6);
    let env = peaks
        .iter()
        .map(|v| {
            let n = (v / max).clamp(0.0, 1.0);
            let g = if n < 0.06 { 0.0 } else { n.sqrt() };
            (g * 255.0).round().clamp(0.0, 255.0) as u8
        })
        .collect();
    (MOUTH_FPS, env)
}

/// 用 ZipVoice 合成 `text` 并播放。**阻塞**（在后台线程里跑）。
///
/// **静默不读（2026-10-12 用户裁决）**：ZipVoice 不可用或合成失败 ⇒ **只记一行 `log::warn`
/// 后直接返回** —— 不回落任何系统音色、不出声、不打扰气泡（见文件头）。
fn speak_blocking(app: &tauri::AppHandle, text: &str) {
    if !crate::tts_zipvoice::available() {
        crate::log::warn("语音朗读：ZipVoice 未就绪（资产或参考音色未配齐），本轮静默不读");
        return;
    }
    if let Err(e) = crate::tts_zipvoice::speak(app, text) {
        crate::log::warn(format!("语音朗读：ZipVoice 合成失败，本轮静默不读（{e}）"));
    }
}

/// 合成好的 WAV 字节 → 落临时文件 → 口型包络下发给桌宠窗 → rodio 播放 → 删文件。
/// 由 `tts_zipvoice::speak` 调（合成本身在那边）。
pub(crate) fn play_wav_bytes(app: &tauri::AppHandle, bytes: &[u8]) -> Result<(), String> {
    // 复用 `commands::temp_image_path` 的临时文件命名规则（同 `screenshot_monitor` 那条口径）。
    let path = crate::commands::temp_image_path("tts", "wav");
    std::fs::write(&path, bytes).map_err(|e| format!("写临时音频失败：{e}"))?;

    // **语音音量在此集中读取一次**（两条朗读通路都收尾到这里）：取宿主 `PetVoiceConfig.volume` 并夹取到
    // `VOLUME_MIN..=VOLUME_MAX`（见 `tts_zipvoice`），再交给 `play_wav` 真正生效。
    let volume = crate::tts_zipvoice::load_config().clamped_volume();

    // **无论播放成败都要删临时文件** —— 先播、再删、最后把结果抛出去。
    let (fps, env) = mouth_envelope(bytes);
    let play_res = play_wav(&path, app, fps, &env, volume);
    let _ = std::fs::remove_file(&path);
    play_res
}

/// 用 rodio 播一个 WAV 文件，**阻塞到放完**。`volume` = 语音音量（rodio 会把每条样本乘以它；1.0 = 原始）。
///
/// 形态照 `player.rs`：`DeviceSinkBuilder::open_default_sink()` → `Player::connect_new` →
/// `append`。这里的 sink / Player 都是**本次一次性的**（每条朗读各开一次、放完即弃），
/// 与音乐播放那套常驻设备的状态机刻意分开 —— 两者互不干扰，朗读结束回收掉这条流即可。
fn play_wav(path: &Path, app: &tauri::AppHandle, fps: u32, env: &[u8], volume: f32) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开音频失败：{e}"))?;
    // `Decoder::try_from(File)` 会顺手设上 byte_len（同 player.rs 的 `open_decoder`）。
    let decoder = Decoder::try_from(file).map_err(|e| format!("解码音频失败：{e}"))?;

    let mut device = rodio::DeviceSinkBuilder::open_default_sink()
        .map_err(|_| "ERR_NO_AUDIO_DEVICE".to_string())?;
    // 默认 drop 时会往 stderr 打一行噪音；我们每条朗读都会建 / 弃一次，关掉它。
    device.log_on_drop(false);
    let player = rodio::Player::connect_new(device.mixer());
    // **音量在这里真正生效**：rodio `Player::set_volume` 把每条样本乘以该值（1.0 = 原始）。0 = 静音。
    player.set_volume(volume);
    // 口型包络**在 `append` 之前**下发（定向给桌宠窗）：窗外收到即开始计时，与音频几乎同线。
    // 桌宠窗没开 / 不是纯文本回复 ⇒ 发不出去也无所谓（`let _`），朗读照常。
    if !env.is_empty() {
        let _ = app.emit_to(
            PET_LABEL,
            MOUTH_EVENT,
            MouthPayload {
                fps,
                env: env.to_vec(),
            },
        );
    }
    player.append(decoder);
    // `append` 之后 `sleep_until_end` 会阻塞到这条 source 播完（rodio 0.22 语义）。
    player.sleep_until_end();
    Ok(())
}

/// 读出桌宠的回复。`enabled == Some(false)` ⇒ 直接跳过（前端还可能传 `None`，视为开）。
///
/// **命令本身立即返回**：合成 + 播放（ZipVoice 合成会阻塞）整段丢到后台线程；
/// 失败只记日志、不 panic、也不回给调用方 —— 朗读失败不该影响气泡显示。
///
/// **「播完」信号**（2026-10-13）：线程跑完 `speak_blocking`（无论播完 / 静默跳过 / 合成失败）
/// 都定向发一条 `TTS_DONE_EVENT` 给桌宠窗 —— 桌宠据此把气泡自动收起的起算推迟到这一刻。
/// 定向（`emit_to`）而非广播：同时开着音乐 / 聊天等插件窗是常态，广播会让别的窗也收到。
#[tauri::command]
pub fn tts_speak(app: tauri::AppHandle, text: String, enabled: Option<bool>) -> Result<(), String> {
    if enabled == Some(false) {
        return Ok(());
    }
    let t = text.trim().to_string();
    if t.is_empty() {
        return Ok(());
    }
    std::thread::spawn(move || {
        speak_blocking(&app, &t);
        // 发不出去（桌宠窗没开）也无所谓：`let _` 吞掉，朗读 / 气泡一律不受影响。
        let _ = app.emit_to(PET_LABEL, TTS_DONE_EVENT, ());
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一段 **PCM16 单声道** WAV（只用于单测，故不做参数校验）。
    fn wav_pcm16(sr: u32, samples: &[i16]) -> Vec<u8> {
        let mut out = Vec::new();
        let data_len = (samples.len() * 2) as u32;
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // 单声道
        out.extend_from_slice(&sr.to_le_bytes());
        out.extend_from_slice(&(sr * 2).to_le_bytes()); // byte rate
        out.extend_from_slice(&2u16.to_le_bytes()); // block align
        out.extend_from_slice(&16u16.to_le_bytes()); // 16 bit
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    /// 前 1 秒静音、后 1 秒满幅 ⇒ 包络前半全 0（闭嘴）、后半接近满值（张嘴）。
    #[test]
    fn mouth_envelope_tracks_amplitude() {
        let sr = 8000u32;
        let mut s = vec![0i16; sr as usize];
        s.extend(std::iter::repeat(20000i16).take(sr as usize));
        let (fps, env) = mouth_envelope(&wav_pcm16(sr, &s));
        assert_eq!(fps, MOUTH_FPS);
        let half = env.len() / 2;
        assert!((env.len() as i32 - (MOUTH_FPS * 2) as i32).abs() <= 1, "len={}", env.len());
        assert!(env[..half].iter().all(|&v| v == 0), "静音段必须闭嘴");
        assert!(env[half..].iter().all(|&v| v > 200), "有声段必须张嘴");
    }

    /// 认不出的字节流 ⇒ **空包络**（调用方据此不驱动口型），绝不 panic。
    #[test]
    fn mouth_envelope_bails_out_on_garbage() {
        let (fps, env) = mouth_envelope(b"not a wav at all");
        assert_eq!(fps, MOUTH_FPS);
        assert!(env.is_empty());
    }
}
