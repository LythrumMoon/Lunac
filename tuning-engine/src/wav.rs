// ── WAV 读写（PCM 16/24/32 位与 IEEE float32）──────────────────────
//
// 「WAV → DSP → WAV」这条链路的两端。**只做 WAV**：它是无损、无专利、到处都能打开的
// 交换格式，而这套引擎的输入输出就是「把一段音频按配置改一遍」（S1 接上播放链路之后
// 才需要实时通路，那是另一码事）。
//
// 三条设计口径：
//   ① **解码 / 编码是纯函数**（吃 `&[u8]`、吐 `Vec<u8>`），文件读写只是薄薄一层
//      —— 这样 WAV 的每一种形态都能在单测里造出来，不需要往磁盘上放测试素材。
//   ② **样本一律是 `f64`（范围 [-1, 1]）**：DSP 内环全用 f64，进来一次转换、
//      出去一次转换，中间不再有第二种表示。整数格式的满量程映射**按对称约定**
//      （见 `to_int` / `from_int`），不是 `(x+1)/2` 那种会把 0 点挪走的写法。
//   ③ **遇到不认识的格式要拒绝并说清是什么**，不许猜 —— 把 ADPCM 当 PCM 解出来的
//      是噪声，而用户会以为「我的音乐被这个软件弄坏了」。
//
// **不做的**：写 extensible 头（写出去一律用 canonical 的 44 字节头，兼容性最好）、
// RF64 / WAVE64（>4GB）、压缩格式、多声道掩码。需要这些时再说，别提前长。

use std::path::Path;

/// 样本格式。**解码时保留原格式**，这样「读进来再写出去」不会偷偷降位深。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    Pcm16,
    Pcm24,
    Pcm32,
    Float32,
}

impl SampleFormat {
    pub fn bits(self) -> u16 {
        match self {
            SampleFormat::Pcm16 => 16,
            SampleFormat::Pcm24 => 24,
            SampleFormat::Pcm32 => 32,
            SampleFormat::Float32 => 32,
        }
    }

    /// WAV 里的 `wFormatTag`（IEEE float 是 3，其余是 1 = PCM）。
    pub fn wav_format_tag(self) -> u16 {
        match self {
            SampleFormat::Float32 => 3,
            _ => 1,
        }
    }

    pub fn bytes_per_sample(self) -> usize {
        self.bits() as usize / 8
    }
}

/// 一段音频。`samples` 是**交错**的（L R L R …），范围 [-1, 1]。
#[derive(Debug, Clone)]
pub struct Audio {
    pub sample_rate: u32,
    pub channels: u16,
    pub format: SampleFormat,
    pub samples: Vec<f64>,
}

impl Audio {
    /// 帧数（一帧 = 所有通道在同一个时刻的样本）。
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }

    /// 峰值绝对值。**给「有没有削波」用**（见 `clipped_samples`）。
    pub fn peak(&self) -> f64 {
        self.samples.iter().fold(0.0f64, |m, v| m.max(v.abs()))
    }

    /// 超出 [-1, 1] 的样本个数。
    ///
    /// **为什么要单独数出来**：整数量化时越界样本会被**钳位**（那是必须的，不能写出
    /// 非法数据），但钳位是「悄悄改掉内容」—— 所以渲染完必须把这件事报出来，
    /// 让用户知道该减 preamp，而不是自己听出「怎么爆了」。
    pub fn clipped_samples(&self) -> usize {
        self.samples.iter().filter(|v| v.abs() > 1.0).count()
    }
}

/// 整数样本 ↔ `f64` 的满量程。**除数是 2^(bits-1)**（对称约定）：
/// 16 位是 32768，于是 `-1.0` ↔ `-32768`、`0.0` ↔ `0`。
fn full_scale(f: SampleFormat) -> f64 {
    match f {
        SampleFormat::Pcm16 => 32768.0,
        SampleFormat::Pcm24 => 8388608.0,
        SampleFormat::Pcm32 | SampleFormat::Float32 => 2147483648.0,
    }
}

fn rd_u16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn rd_u32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// 一个样本的字节 → `f64`。
fn from_raw(raw: &[u8], f: SampleFormat) -> f64 {
    match f {
        SampleFormat::Pcm16 => i16::from_le_bytes([raw[0], raw[1]]) as f64 / full_scale(f),
        SampleFormat::Pcm24 => {
            // 24 位是**有符号**的：先拼成低 24 位，再左移 8 位做符号扩展，最后算术右移回来。
            let v = (raw[0] as i32) | ((raw[1] as i32) << 8) | ((raw[2] as i32) << 16);
            ((v << 8) >> 8) as f64 / full_scale(f)
        }
        SampleFormat::Pcm32 => {
            i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as f64 / full_scale(f)
        }
        // float 格式**不钳位**：越界值原样带过去（那是上游的事，不是格式的事）
        SampleFormat::Float32 => f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as f64,
    }
}

/// 一个 `f64` → 样本字节。整数格式**必须钳位**（写不出越界数据），float 不钳。
fn push_raw(out: &mut Vec<u8>, x: f64, f: SampleFormat) {
    match f {
        SampleFormat::Float32 => out.extend_from_slice(&(x as f32).to_le_bytes()),
        _ => {
            let s = (x * full_scale(f)).round();
            // 上限是 2^(bits-1) − 1、下限是 −2^(bits-1)：`+1.0` 会撞到 32768 这个
            // 「表示不了的正数」，所以正向夹到 32767（IEEE float 才没这个问题）。
            let max = full_scale(f) - 1.0;
            let v = s.clamp(-full_scale(f), max) as i64;
            let b = v.to_le_bytes();
            out.extend_from_slice(&b[..f.bytes_per_sample()]);
        }
    }
}

/// 解码一段 WAV 字节。
pub fn decode(bytes: &[u8]) -> Result<Audio, String> {
    if bytes.len() < 12 {
        return Err(format!("文件太短（{} 字节），不是 WAV", bytes.len()));
    }
    if &bytes[0..4] != b"RIFF" {
        return Err("不是 RIFF 容器（WAV 必须是 RIFF）".to_string());
    }
    if &bytes[8..12] != b"WAVE" {
        return Err(format!(
            "RIFF 但不是 WAVE（是 {:?}）",
            String::from_utf8_lossy(&bytes[8..12])
        ));
    }

    let mut fmt: Option<(u16, u16, u32, u16)> = None; // (格式码, 声道, 采样率, 位深)
    let mut data: Option<&[u8]> = None;

    // 逐块走。**必须按块跳而不是「假设 fmt 在前、data 在后」** —— 真实文件里
    // LIST / fact / PEAK 会插在中间，写死的偏移量在那些文件上会直接读出垃圾。
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = rd_u32(bytes, pos + 4).unwrap() as usize;
        let body = pos + 8;
        let end = body
            .checked_add(size)
            .ok_or_else(|| format!("块长度溢出（{:?} 声明 {size} 字节）", String::from_utf8_lossy(id)))?;
        if end > bytes.len() {
            // **这条必须拦**：按声明的长度去读会读到越界（或被截断的数据），
            // 而症状是「后半段音频变成噪声」，查起来像 DSP 的锅。
            return Err(format!(
                "块 {:?} 声明 {size} 字节，但文件只剩 {} 字节（文件被截断）",
                String::from_utf8_lossy(id),
                bytes.len() - body
            ));
        }

        if id == b"fmt " {
            if size < 16 {
                return Err(format!("fmt 块只有 {size} 字节，至少要有 16"));
            }
            let mut tag = rd_u16(bytes, body).unwrap();
            let channels = rd_u16(bytes, body + 2).unwrap();
            let rate = rd_u32(bytes, body + 4).unwrap();
            let bits = rd_u16(bytes, body + 14).unwrap();
            if tag == 0xFFFE {
                // WAVE_FORMAT_EXTENSIBLE：真正的编码在 SubFormat GUID 的头两个字节
                if size < 40 {
                    return Err(format!("extensible 的 fmt 块只有 {size} 字节，至少要有 40"));
                }
                tag = rd_u16(bytes, body + 24).unwrap();
            }
            fmt = Some((tag, channels, rate, bits));
        } else if id == b"data" {
            data = Some(&bytes[body..end]);
        }

        // RIFF 的块按**偶数字节**对齐（奇数长度后面补一个填充字节）
        pos = end + (size & 1);
    }

    let (tag, channels, sample_rate, bits) = fmt.ok_or("WAV 里没有 fmt 块")?;
    if channels == 0 {
        return Err("声道数为 0".to_string());
    }
    if sample_rate == 0 {
        return Err("采样率为 0".to_string());
    }
    let format = match (tag, bits) {
        (1, 16) => SampleFormat::Pcm16,
        (1, 24) => SampleFormat::Pcm24,
        (1, 32) => SampleFormat::Pcm32,
        (3, 32) => SampleFormat::Float32,
        (1 | 3, b) => {
            return Err(format!(
                "不支持的位深 {b}（本引擎只认 16 / 24 / 32 位 PCM 与 32 位 float）"
            ))
        }
        (t, _) => {
            return Err(format!(
                "不支持的 WAV 编码格式 0x{t:04X}（本引擎只读 PCM 与 IEEE float，压缩格式不读）"
            ))
        }
    };

    let raw = data.ok_or("WAV 里没有 data 块")?;
    let bps = format.bytes_per_sample();
    let count = raw.len() / bps; // 尾部不足一个样本的零头丢掉（宁可少一个，也不越界）
    let samples = (0..count)
        .map(|i| from_raw(&raw[i * bps..i * bps + bps], format))
        .collect();

    Ok(Audio {
        sample_rate,
        channels,
        format,
        samples,
    })
}

/// 编码成 WAV 字节（canonical 44 字节头 + `Audio::format`）。
///
/// 返回 `Result` 只为一件事：**WAV 的尺寸字段是 32 位**，超过 4GB 就表示不了。
/// 那种时候必须报错，不能让它回绕成一个「看起来正常」的短文件。
pub fn encode(audio: &Audio) -> Result<Vec<u8>, String> {
    if audio.channels == 0 {
        return Err("声道数为 0，写不出合法 WAV".to_string());
    }
    if audio.sample_rate == 0 {
        return Err("采样率为 0，写不出合法 WAV".to_string());
    }
    let bps = audio.format.bytes_per_sample();
    let block_align = audio.channels as u32 * bps as u32;
    let data_len = audio
        .samples
        .len()
        .checked_mul(bps)
        .ok_or("样本数溢出，无法表达".to_string())?;
    if data_len + 44 > u32::MAX as usize {
        return Err(format!(
            "结果超过 4GB（{data_len} 字节）—— WAV 的尺寸字段装不下，请改用更短的片段或分次渲染"
        ));
    }

    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&audio.format.wav_format_tag().to_le_bytes());
    out.extend_from_slice(&audio.channels.to_le_bytes());
    out.extend_from_slice(&audio.sample_rate.to_le_bytes());
    out.extend_from_slice(&(audio.sample_rate * block_align).to_le_bytes());
    out.extend_from_slice(&(block_align as u16).to_le_bytes());
    out.extend_from_slice(&audio.format.bits().to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for &x in &audio.samples {
        push_raw(&mut out, x, audio.format);
    }
    Ok(out)
}

/// 读一个 WAV 文件。
pub fn read(path: &Path) -> Result<Audio, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读不了 {}：{e}", path.display()))?;
    decode(&bytes)
}

/// 写一个 WAV 文件。
pub fn write(path: &Path, audio: &Audio) -> Result<(), String> {
    let bytes = encode(audio)?;
    std::fs::write(path, bytes).map_err(|e| format!("写不了 {}：{e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 造 WAV 的小工具（自己拼字节，不依赖被测实现 —— 否则就是自己验自己）──

    fn fmt_chunk(channels: u16, rate: u32, bits: u16, tag: u16) -> Vec<u8> {
        let block = channels as u32 * (bits as u32 / 8);
        let mut v = Vec::new();
        v.extend_from_slice(&tag.to_le_bytes());
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * block).to_le_bytes());
        v.extend_from_slice(&(block as u16).to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        v
    }

    /// 按块拼一个 WAV：`chunks` 是 (id, payload) 序列（`data` 也在里面）。
    fn build_wav(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (id, payload) in chunks {
            body.extend_from_slice(*id);
            body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            body.extend_from_slice(payload);
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&body);
        out
    }

    fn sine(rate: u32, channels: u16, freq: f64, frames: usize) -> Audio {
        let mut samples = Vec::with_capacity(frames * channels as usize);
        for n in 0..frames {
            let v = (2.0 * std::f64::consts::PI * freq * n as f64 / rate as f64).sin() * 0.5;
            for _ in 0..channels {
                samples.push(v);
            }
        }
        Audio {
            sample_rate: rate,
            channels,
            format: SampleFormat::Pcm16,
            samples,
        }
    }

    fn assert_close(got: &[f64], want: &[f64], tol: f64) {
        assert_eq!(got.len(), want.len(), "样本个数不一致");
        for (i, (a, b)) in got.iter().zip(want).enumerate() {
            assert!((a - b).abs() <= tol, "第 {i} 个样本：{a} vs {b}（容差 {tol}）");
        }
    }

    #[test]
    fn pcm16_round_trip() {
        let a = sine(48000, 2, 440.0, 64);
        let bytes = encode(&a).unwrap();
        let b = decode(&bytes).unwrap();
        assert_eq!(b.sample_rate, 48000);
        assert_eq!(b.channels, 2);
        assert_eq!(b.format, SampleFormat::Pcm16);
        assert_eq!(b.frames(), a.frames());
        // 16bit 的量化步长 = 1/32768
        assert_close(&b.samples, &a.samples, 1.0 / 32768.0);
        // 头部必须是 canonical 44 字节 + 样本
        assert_eq!(bytes.len(), 44 + a.samples.len() * 2);
    }

    #[test]
    fn pcm24_round_trip() {
        let mut a = sine(44100, 1, 1000.0, 64);
        a.format = SampleFormat::Pcm24;
        let b = decode(&encode(&a).unwrap()).unwrap();
        assert_eq!(b.format, SampleFormat::Pcm24);
        assert_eq!(b.sample_rate, 44100);
        assert_close(&b.samples, &a.samples, 1.0 / 8388608.0);
    }

    #[test]
    fn pcm32_round_trip() {
        let mut a = sine(96000, 1, 1000.0, 64);
        a.format = SampleFormat::Pcm32;
        let b = decode(&encode(&a).unwrap()).unwrap();
        assert_eq!(b.format, SampleFormat::Pcm32);
        assert_close(&b.samples, &a.samples, 1.0 / 2147483648.0);
    }

    #[test]
    fn float32_round_trip_keeps_full_precision() {
        let mut a = sine(48000, 1, 997.0, 64);
        a.format = SampleFormat::Float32;
        // 故意塞几个超出 [-1,1] 的值：float 格式能原样带过去（钳位只发生在**整数**格式）
        a.samples[3] = 1.5;
        a.samples[4] = -2.25;
        let b = decode(&encode(&a).unwrap()).unwrap();
        assert_eq!(b.format, SampleFormat::Float32);
        assert_close(&b.samples, &a.samples, 1e-7);
    }

    #[test]
    fn decode_accepts_extensible_and_skips_unknown_chunks() {
        // 真实文件里常有 LIST / fact / PEAK 之类；fmt 也可能写成 extensible(0xFFFE)。
        // 两者都要认，否则「用户的 WAV 打不开」会变成一道无解的支持题。
        let mut fmt = fmt_chunk(2, 48000, 16, 0xFFFE);
        fmt.extend_from_slice(&22u16.to_le_bytes()); // cbSize
        fmt.extend_from_slice(&16u16.to_le_bytes()); // 有效位
        fmt.extend_from_slice(&3u32.to_le_bytes()); // 通道掩码（立体声）
        fmt.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]); // KSDATAFORMAT_SUBTYPE_PCM
        let data = vec![0u8; 8];
        let wav = build_wav(&[
            (b"fmt ", fmt),
            (b"LIST", vec![0u8; 10]),
            (b"data", data),
        ]);
        let a = decode(&wav).unwrap();
        assert_eq!(a.format, SampleFormat::Pcm16);
        assert_eq!(a.sample_rate, 48000);
        assert_eq!(a.channels, 2);
        assert_eq!(a.samples.len(), 4);
    }

    #[test]
    fn decode_rejects_what_it_cannot_handle() {
        // 不是 RIFF
        assert!(decode(b"not a wav at all, sorry").is_err());
        // RIFF 但不是 WAVE
        let mut not_wave = b"RIFF".to_vec();
        not_wave.extend_from_slice(&4u32.to_le_bytes());
        not_wave.extend_from_slice(b"AVI ");
        assert!(decode(&not_wave).is_err());

        // 压缩格式（0x0011 = IMA ADPCM）：必须**说清是什么**，不能当 PCM 硬解
        let wav = build_wav(&[
            (b"fmt ", fmt_chunk(1, 48000, 4, 0x0011)),
            (b"data", vec![0u8; 16]),
        ]);
        let err = decode(&wav).unwrap_err();
        assert!(err.contains("0011"), "错误消息要带上格式码，收到：{err}");

        // 位深不支持（8bit）
        let wav = build_wav(&[
            (b"fmt ", fmt_chunk(1, 48000, 8, 1)),
            (b"data", vec![0u8; 16]),
        ]);
        assert!(decode(&wav).is_err());

        // data 声明得比文件实际还长（截断文件）—— 这条最危险：不拦就会读到越界
        let fmt = fmt_chunk(1, 48000, 16, 1);
        let mut body = Vec::new();
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        body.extend_from_slice(&fmt);
        body.extend_from_slice(b"data");
        body.extend_from_slice(&9999u32.to_le_bytes()); // 谎报长度
        body.extend_from_slice(&[0u8; 16]);
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(&body);
        assert!(decode(&wav).is_err());
    }

    #[test]
    fn encode_clamps_but_the_caller_can_see_it() {
        // 钳位是**必须的**（不能写出非法数据），但瞒着用户就不行 ——
        // 所以 `clipped_samples()` 必须在写之前就数得出来。
        let mut a = sine(48000, 1, 1000.0, 8);
        a.samples[2] = 2.0;
        a.samples[5] = -1.5;
        assert_eq!(a.clipped_samples(), 2);
        assert!((a.peak() - 2.0).abs() < 1e-12);

        let b = decode(&encode(&a).unwrap()).unwrap();
        assert!(b.peak() <= 1.0 + 1e-9, "写出去的样本不许越界");
        assert!((b.samples[2] - 1.0).abs() < 1e-4, "正向越界钳到满量程");
        assert!((b.samples[5] + 1.0).abs() < 1e-4, "负向越界钳到负满量程");
    }

    #[test]
    fn read_write_round_trip_on_disk() {
        let dir = std::env::temp_dir().join("lunac-tuning-wav-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("roundtrip.wav");
        let a = sine(48000, 2, 440.0, 100);
        write(&p, &a).unwrap();
        let b = read(&p).unwrap();
        assert_eq!(b.sample_rate, a.sample_rate);
        assert_eq!(b.channels, a.channels);
        assert_close(&b.samples, &a.samples, 1.0 / 32768.0);
        let _ = std::fs::remove_file(&p);
    }
}
