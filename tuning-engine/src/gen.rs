// ── 测试信号生成（正弦 / 静音）── 见 docs/ai-spec.md §4.10
//
// 只做**答案已知**的信号：`render` 把电平改了多少，`measure` 量出来就该是多少。
//
// **不做脉冲 WAV**：脉冲形式的那次测量由 `measure` 自己解析地喂进去（`response_by_impulse`），
// 不需要先落一个 WAV 再读回来。真需要「外部设备播脉冲」是 S2 的事，到时候再说。
//
// **不做噪声 / 扫频**：噪声要随机源（不可复现），扫频属于 S2 的 loopback 闭环。

use std::f64::consts::PI;

use crate::wav::{Audio, SampleFormat};

/// 生成用的样本格式：**16 位 PCM**（刻意不用 float32）。
/// 这样「生成 → 写盘 → 读回 → 渲染 → 再写盘」整条真实通路都进了验收回路；
/// 而 16 位的量化误差（≈ −96dBFS）比 0.1dB 那条判据低两个数量级，不会碍事。
const FORMAT: SampleFormat = SampleFormat::Pcm16;

/// 时长 → 帧数。**至少 1 帧**（「0.0001 秒」这种请求给一帧比给零帧有用）。
fn frames_for(sample_rate: u32, seconds: f64) -> Result<usize, String> {
    if sample_rate == 0 {
        return Err("采样率必须为正".to_string());
    }
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(format!("时长必须为正的有限值，收到 {seconds}"));
    }
    let f = (seconds * sample_rate as f64).round();
    if f < 1.0 {
        return Err(format!(
            "{seconds} 秒在 {sample_rate}Hz 下不足一帧 —— 太短了，生成不了信号"
        ));
    }
    Ok(f as usize)
}

fn check_channels(channels: u16) -> Result<usize, String> {
    if channels == 0 {
        return Err("声道数必须为正".to_string());
    }
    Ok(channels as usize)
}

/// 一个正弦：`x[n] = amp · sin(2π·f·n/fs)`，所有声道相同。
///
/// 各声道相同是**故意的**：这样「声道状态有没有串」这件事在 `chain` 那边由更严格的
/// 用例管（喂不同信号），这里只管生成一个已知答案的信号。
pub fn tone(
    sample_rate: u32,
    channels: u16,
    freq_hz: f64,
    seconds: f64,
    amp: f64,
) -> Result<Audio, String> {
    let ch = check_channels(channels)?;
    let frames = frames_for(sample_rate, seconds)?;
    let nyquist = sample_rate as f64 / 2.0;
    if !freq_hz.is_finite() || freq_hz <= 0.0 || freq_hz >= nyquist {
        return Err(format!(
            "频率必须落在 (0, {nyquist}) 之间，收到 {freq_hz}"
        ));
    }
    if !amp.is_finite() || amp <= 0.0 || amp > 1.0 {
        return Err(format!("振幅必须落在 (0, 1] 之间，收到 {amp}"));
    }

    let step = 2.0 * PI * freq_hz / sample_rate as f64;
    let mut samples = Vec::with_capacity(frames * ch);
    for n in 0..frames {
        let v = amp * (step * n as f64).sin();
        for _ in 0..ch {
            samples.push(v);
        }
    }
    Ok(Audio {
        sample_rate,
        channels,
        format: FORMAT,
        samples,
    })
}

/// 一段静音。
///
/// 存在理由不是「造素材」，而是**自检**：把静音喂进渲染器，输出必须仍然是静音。
/// 任何直流泄漏、不稳定系数、状态没清零都会让它露出来 —— 而这类问题在白噪声或
/// 音乐素材上只是「听起来有点脏」，很难定位。
pub fn silence(sample_rate: u32, channels: u16, seconds: f64) -> Result<Audio, String> {
    let ch = check_channels(channels)?;
    let frames = frames_for(sample_rate, seconds)?;
    Ok(Audio {
        sample_rate,
        channels,
        format: FORMAT,
        samples: vec![0.0; frames * ch],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ChainConfig;
    use crate::wav::SampleFormat as F;

    #[test]
    fn tone_has_exactly_the_asked_shape() {
        let a = tone(48000, 2, 1000.0, 0.5, 0.5).unwrap();
        assert_eq!(a.sample_rate, 48000);
        assert_eq!(a.channels, 2);
        assert_eq!(a.format, F::Pcm16);
        assert_eq!(a.frames(), 24000);
        assert_eq!(a.samples.len(), 48000);

        // 逐点对照公式（不是「看起来像正弦」）
        for n in [0usize, 1, 12, 240, 23999] {
            let want = 0.5 * (2.0 * PI * 1000.0 * n as f64 / 48000.0).sin();
            assert!((a.samples[2 * n] - want).abs() < 1e-12, "第 {n} 帧");
            assert_eq!(a.samples[2 * n], a.samples[2 * n + 1], "左右声道应相同");
        }
        // 峰值就是给的振幅
        assert!((a.peak() - 0.5).abs() < 1e-3, "峰值 {}", a.peak());
        // 整数格式下不该有任何越界
        assert_eq!(a.clipped_samples(), 0);
    }

    #[test]
    fn tone_rounds_the_length_and_keeps_at_least_one_frame() {
        // 0.0001 秒 @48k = 4.8 帧 ⇒ 5 帧（不是 0）
        let a = tone(48000, 1, 1000.0, 0.0001, 1.0).unwrap();
        assert_eq!(a.frames(), 5);
    }

    #[test]
    fn tone_rejects_impossible_parameters() {
        assert!(tone(0, 1, 1000.0, 1.0, 0.5).is_err(), "采样率 0");
        assert!(tone(48000, 0, 1000.0, 1.0, 0.5).is_err(), "0 声道");
        assert!(tone(48000, 1, 0.0, 1.0, 0.5).is_err(), "0Hz");
        assert!(tone(48000, 1, 24000.0, 1.0, 0.5).is_err(), "Nyquist");
        assert!(tone(48000, 1, 1000.0, 0.0, 0.5).is_err(), "0 秒");
        assert!(tone(48000, 1, 1000.0, -1.0, 0.5).is_err(), "负时长");
        assert!(tone(48000, 1, 1000.0, 1.0, 0.0).is_err(), "振幅 0");
        assert!(tone(48000, 1, 1000.0, 1.0, 1.5).is_err(), "振幅越界");
    }

    #[test]
    fn silence_is_all_zero() {
        let a = silence(44100, 2, 0.1).unwrap();
        assert_eq!(a.frames(), 4410);
        assert!(a.samples.iter().all(|v| *v == 0.0));
        assert!(silence(44100, 0, 0.1).is_err());
        assert!(silence(0, 1, 0.1).is_err());
    }

    #[test]
    fn rendering_silence_stays_silent_no_matter_what_the_chain_does() {
        // 自检：直流泄漏 / 系数不稳 / 状态没清零 —— 这三类毛病都会在这里冒出来。
        // 挑一条增益很大的链，让它更容易暴露。
        let chain = ChainConfig::from_json(
            r#"{
                "preamp_db": 6.0,
                "filters": [
                    { "kind": "peaking",    "freq_hz": 60,  "gain_db": 12, "q": 6 },
                    { "kind": "low_shelf",  "freq_hz": 200, "gain_db": 9 },
                    { "kind": "high_pass",  "freq_hz": 30,  "q": 0.6 }
                ]
            }"#,
            48000.0,
        )
        .unwrap();

        let a = silence(48000, 2, 0.25).unwrap();
        let out = crate::chain::render(&a, &chain).unwrap();
        assert!(
            out.samples.iter().all(|v| *v == 0.0),
            "静音进去非静音出来：峰值 {}",
            out.peak()
        );
    }

    #[test]
    fn format_is_pcm16_on_purpose() {
        // 这条是「口径」不是「实现细节」：换成 float32 会让验收绕过真实量化通路。
        assert_eq!(FORMAT, F::Pcm16);
    }
}
