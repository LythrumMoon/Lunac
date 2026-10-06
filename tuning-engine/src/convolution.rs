// ── FFT 快速卷积（EAPO 的 `Convolution` 内核，2026-10-03）─────────────
//
// 用途：把一段音频与一条**脉冲响应**（IR：房间 / 耳机校正 / 混响）卷积。与双二阶链不同，
// 卷积**无法逐样本做** —— 它要看到整段 IR，所以走分块 FFT（overlap-add）。
//
// **为什么是 overlap-add**：实现最直白（每块独立卷积、结果按偏移相加），而「块内不产生
// 环绕」的条件是 `FFT 长度 ≥ 块长 + IR 长度 − 1`；取 `块长 = IR 长度`、`N = next_pow2(2L)`
// 即满足（2L ≥ 2L−1）。这条不变量错了会表现为「上一块的尾巴漏进下一块」，很难听出来。
//
// 精度口径与 `measure` 同一档：单测拿「朴素直接卷积」当**独立参照物**对拍（同 `fft` 拿朴素
// DFT 对拍的精神），而不是自己跟自己对。
//
// **这一层到「能用在渲染里」为止**：`load_ir` 把 IR 从 WAV 读进来并校验（单声道 + 采样率
// 完全一致），`apply_interleaved` 把卷积作用到一段交错样本上。**流式（实时）那份还没做** ——
// 它要一块跨调用保留的 overlap-add 尾巴，等宿主播放路径真正接线时再补（见 docs/ai-spec §4.10）。

use std::path::Path;

use crate::fft::{fft_in_place, ifft_in_place, next_pow2};
use crate::wav;

/// 时域直接卷积（O(n·m)）。**只给测试当参照物**，不进生产路径。
#[cfg(test)]
fn convolve_direct(signal: &[f64], ir: &[f64]) -> Vec<f64> {
    if signal.is_empty() || ir.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0.0; signal.len() + ir.len() - 1];
    for (i, &s) in signal.iter().enumerate() {
        for (j, &h) in ir.iter().enumerate() {
            out[i + j] += s * h;
        }
    }
    out
}

/// FFT 快速卷积（overlap-add）。返回长度 `signal.len() + ir.len() − 1`。
///
/// 空 `ir` 是**错误**（没有脉冲响应就没有卷积）；空 `signal` 返回空。
pub fn convolve(signal: &[f64], ir: &[f64]) -> Result<Vec<f64>, String> {
    if ir.is_empty() {
        return Err("脉冲响应为空，无法卷积".to_string());
    }
    if signal.is_empty() {
        return Ok(Vec::new());
    }
    let l = ir.len();
    // 块长 = IR 长度；FFT 长度 next_pow2(2L) ≥ 2L−1 ⇒ 块内不环绕（见模块头注释）。
    let n = next_pow2(2 * l).max(2);
    let block = l;
    let out_len = signal.len() + l - 1;
    let mut out = vec![0.0; out_len];

    // IR 的频域：一次算好，逐块复用。
    let mut hr = vec![0.0; n];
    let mut hi = vec![0.0; n];
    hr[..l].copy_from_slice(ir);
    fft_in_place(&mut hr, &mut hi)?;

    let mut xr = vec![0.0; n];
    let mut xi = vec![0.0; n];
    let mut pos = 0usize;
    while pos < signal.len() {
        let take = block.min(signal.len() - pos);
        for v in xr.iter_mut() {
            *v = 0.0;
        }
        for v in xi.iter_mut() {
            *v = 0.0;
        }
        xr[..take].copy_from_slice(&signal[pos..pos + take]);
        fft_in_place(&mut xr, &mut xi)?;
        // 频域相乘（逐点复数乘）
        for k in 0..n {
            let (a, b) = (xr[k], xi[k]);
            let (c, d) = (hr[k], hi[k]);
            xr[k] = a * c - b * d;
            xi[k] = a * d + b * c;
        }
        ifft_in_place(&mut xr, &mut xi)?;
        // overlap-add：整块（含超出块长的尾巴）按偏移累加，尾巴正好接进下一块。
        let end = (pos + n).min(out_len);
        for k in 0..(end - pos) {
            out[pos + k] += xr[k];
        }
        pos += block;
    }
    Ok(out)
}

/// 从 WAV 读一条脉冲响应（IR），并校验它能用在 `sample_rate` 的流上。
///
/// 两条硬约束 —— 本引擎**不做重采样、不做多声道 IR**：IR 必须是**单声道**，采样率必须与
/// 渲染流**完全一致**。任一条不满足就报错：静默重采样 / 只取第一声道都会让结果悄悄偏掉，
/// 而「听起来还凑合」正是最难查的那种错（同「写了用不上的字段一律报错」的口径）。
pub fn load_ir(path: &str, sample_rate: f64) -> Result<Vec<f64>, String> {
    let audio = wav::read(Path::new(path)).map_err(|e| format!("读不了脉冲响应 {path}：{e}"))?;
    if audio.channels != 1 {
        return Err(format!(
            "脉冲响应 {path} 是 {} 声道 —— 本引擎只接受**单声道** IR（多声道请先合成一条）",
            audio.channels
        ));
    }
    if audio.sample_rate as f64 != sample_rate {
        return Err(format!(
            "脉冲响应 {path} 是 {} Hz，而渲染流是 {sample_rate} Hz —— 本引擎不做重采样，\
             请先把 IR 转成同样的采样率",
            audio.sample_rate
        ));
    }
    if audio.samples.is_empty() {
        return Err(format!("脉冲响应 {path} 里一个样本都没有"));
    }
    Ok(audio.samples)
}

/// **流式**卷积器（每声道一个实例）：逐样本喂、逐样本取。
///
/// 与 [`convolve`]（整段离线）是同一条 overlap-add 内核的两副形态 —— 这里把**输入块**与
/// **overlap 尾巴**都留在自己的状态里，于是它能挂在**播放链**上逐样本跑
/// （宿主 `player.rs` 的 `TuningSource` 就是那个调用方）。
///
/// ⚠️ **延迟 = `ir_len() - 1` 个样本**：块长 = IR 长度，「第 n 个输出」要等第 n+L 个输入
/// 才凑得齐一块。前 `L-1` 个输出因此恒为 0（这是**位移**，不是丢失）。音乐播放没有音画
/// 同步问题，所以可以接受；但对很短的校正 IR，位移就是几百个样本。
#[derive(Debug, Clone)]
pub struct Convolver {
    /// FFT 长度 = `next_pow2(2L)`（块内不环绕，同 `convolve`）。
    n: usize,
    /// 块长 = IR 长度。
    block: usize,
    /// IR 的频谱（构造时算一次，此后每块复用）。
    hr: Vec<f64>,
    hi: Vec<f64>,
    /// 输入块（实部）+ 频域暂存（虚部）。
    xr: Vec<f64>,
    xi: Vec<f64>,
    /// 已攒的输入样本数（`< block`）。
    in_len: usize,
    /// 上一块漏下来的尾巴（长度 `block - 1`）。
    tail: Vec<f64>,
    /// 当前块算出的输出（长度 `block`）。
    out: Vec<f64>,
    /// `out` 已吐到第几个；`== block` 表示没得吐（还没攒满一块）。
    out_pos: usize,
}

impl Convolver {
    pub fn new(ir: &[f64]) -> Result<Convolver, String> {
        if ir.is_empty() {
            return Err("脉冲响应为空，无法卷积".to_string());
        }
        let block = ir.len();
        let n = next_pow2(2 * block).max(2);
        let mut hr = vec![0.0; n];
        let mut hi = vec![0.0; n];
        hr[..block].copy_from_slice(ir);
        fft_in_place(&mut hr, &mut hi)?;
        Ok(Convolver {
            n,
            block,
            hr,
            hi,
            xr: vec![0.0; n],
            xi: vec![0.0; n],
            in_len: 0,
            tail: vec![0.0; block - 1],
            out: vec![0.0; block],
            // 初始「没得吐」：第一次 `process_sample` 会先把样本攒进块里。
            out_pos: block,
        })
    }

    /// IR 长度（= 引入的延迟样本数 + 1，见类型注释）。
    pub fn ir_len(&self) -> usize {
        self.block
    }

    /// 把状态清回初始（换流 / seek 之后必须调，否则尾巴会串到新位置上去）。
    pub fn reset(&mut self) {
        for v in self.xr.iter_mut() {
            *v = 0.0;
        }
        for v in self.xi.iter_mut() {
            *v = 0.0;
        }
        for v in self.tail.iter_mut() {
            *v = 0.0;
        }
        for v in self.out.iter_mut() {
            *v = 0.0;
        }
        self.in_len = 0;
        self.out_pos = self.block;
    }

    /// 喂一个样本、取一个样本。
    ///
    /// **每一拍都必须攒输入**（哪怕这一拍有输出可吐）—— 攒与吐是 1:1 的两条流水线，
    /// 一旦在「吐」的时候跳过「攒」，输出就会在块与块之间**插零**（每块尾巴多出 `L-1`
    /// 个 0）。这条错了不会报错，只会把音频变成断续的 —— 所以单测拿直接卷积逐点对拍。
    pub fn process_sample(&mut self, x: f64) -> f64 {
        // ① 攒输入
        self.xr[self.in_len] = x;
        self.in_len += 1;
        if self.in_len == self.block {
            // ② 块满 ⇒ 补零 + FFT 卷积 + overlap-add，一次算出 `block` 个输出
            for v in self.xr[self.block..self.n].iter_mut() {
                *v = 0.0;
            }
            for v in self.xi.iter_mut() {
                *v = 0.0;
            }
            // 长度在构造时已保证是 2 的幂 ⇒ 这两步不会失败（失败也没法在音频线程上做什么）。
            let _ = fft_in_place(&mut self.xr, &mut self.xi);
            for k in 0..self.n {
                let (a, b) = (self.xr[k], self.xi[k]);
                let (c, d) = (self.hr[k], self.hi[k]);
                self.xr[k] = a * c - b * d;
                self.xi[k] = a * d + b * c;
            }
            let _ = ifft_in_place(&mut self.xr, &mut self.xi);
            // 本块前段 += 上一块的尾巴；新尾巴 = 本块后 `block-1` 个
            for j in 0..self.block {
                let t = if j < self.tail.len() { self.tail[j] } else { 0.0 };
                self.out[j] = self.xr[j] + t;
            }
            for j in 0..self.tail.len() {
                self.tail[j] = self.xr[self.block + j];
            }
            self.in_len = 0;
            // 「攒 L 个 ↔ 吐 L 个」严丝合缝：算新块时上一队的 L 个正好吐完（`out_pos == block`），
            // 所以这里可以直接覆盖 `out`，不需要第二条队列。
            self.out_pos = 0;
        }
        // ③ 取一个（队列空 = 还没攒满第一块 ⇒ 延迟段输出 0，见类型注释）
        if self.out_pos < self.block {
            let y = self.out[self.out_pos];
            self.out_pos += 1;
            y
        } else {
            0.0
        }
    }
}

/// 对**交错**样本逐声道卷积（原地），**输出长度不变**（卷积多出的尾巴被截断）。
///
/// 每条声道与**同一条** IR 卷积 —— 单声道 IR 的语义。截断意味着「带明显群延迟的线性相位
/// IR」会表现为整体位移 + 尾部被切；校正类 IR（最小相位 / 已做延迟补偿）才是这条通路的目标。
pub fn apply_interleaved(samples: &mut [f64], channels: usize, ir: &[f64]) -> Result<(), String> {
    if channels == 0 {
        return Err("声道数必须为正".to_string());
    }
    if !samples.len().is_multiple_of(channels) {
        return Err(format!(
            "样本个数 {} 不是声道数 {channels} 的整数倍",
            samples.len()
        ));
    }
    if ir.is_empty() {
        return Err("脉冲响应为空，无法卷积".to_string());
    }
    let frames = samples.len() / channels;
    if frames == 0 {
        return Ok(());
    }
    let mut work = vec![0.0f64; frames];
    for c in 0..channels {
        // 抽取这条声道 → 卷积 → 只写回原帧数的前 `frames` 个（剩下的就是被截掉的尾巴）。
        for (i, v) in work.iter_mut().enumerate() {
            *v = samples[i * channels + c];
        }
        let conv = convolve(&work, ir)?; // 长度 = frames + L − 1 ≥ frames（L ≥ 1）
        for (i, out) in samples[c..].iter_mut().step_by(channels).enumerate() {
            *out = conv[i];
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_impulse_is_an_identity() {
        let s: Vec<f64> = (0..37).map(|i| (i as f64).sin()).collect();
        let out = convolve(&s, &[1.0]).unwrap();
        assert_eq!(out.len(), s.len());
        for (a, b) in out.iter().zip(&s) {
            assert!((a - b).abs() < 1e-12, "恒等卷积不该改值");
        }
    }

    #[test]
    fn a_delayed_impulse_shifts_by_its_index() {
        let s = vec![1.0, 2.0, 3.0];
        let out = convolve(&s, &[0.0, 0.0, 1.0]).unwrap();
        assert_eq!(out.len(), 5);
        assert!((out[2] - 1.0).abs() < 1e-12);
        assert!((out[3] - 2.0).abs() < 1e-12);
        assert!((out[4] - 3.0).abs() < 1e-12);
        assert!(out[0].abs() < 1e-12 && out[1].abs() < 1e-12);
    }

    #[test]
    fn matches_direct_convolution() {
        // 两条独立实现给出同一串数才算数（同 fft 拿朴素 DFT 对拍的口径）。
        let signal: Vec<f64> = (0..300)
            .map(|i| ((i * 37 % 101) as f64 - 50.0) / 50.0)
            .collect();
        let ir: Vec<f64> = (0..17)
            .map(|i| ((i * 13 % 29) as f64 - 14.0) / 14.0)
            .collect();
        let fast = convolve(&signal, &ir).unwrap();
        let slow = convolve_direct(&signal, &ir);
        assert_eq!(fast.len(), slow.len());
        for (i, (a, b)) in fast.iter().zip(&slow).enumerate() {
            assert!((a - b).abs() < 1e-9, "第 {i} 个样本：fft={a} 直接={b}");
        }
    }

    #[test]
    fn a_long_signal_is_chunked_without_smearing_between_blocks() {
        // signal 远长于块长（>1 块）时，overlap-add 的接缝必须严丝合缝。
        let mut signal = vec![0.0; 1000];
        signal[0] = 1.0; // 单位冲激
        let ir: Vec<f64> = vec![1.0; 40]; // 40 点矩形 ⇒ 输出是 40 长的矩形
        let fast = convolve(&signal, &ir).unwrap();
        let slow = convolve_direct(&signal, &ir);
        assert_eq!(fast.len(), slow.len());
        for (i, (a, b)) in fast.iter().zip(&slow).enumerate() {
            assert!((a - b).abs() < 1e-9, "第 {i} 个样本：fft={a} 直接={b}");
        }
    }

    #[test]
    fn rejects_an_empty_ir_and_handles_an_empty_signal() {
        assert!(convolve(&[1.0, 2.0], &[]).is_err());
        assert!(convolve(&[], &[1.0]).unwrap().is_empty());
    }

    // ── 作用到缓冲 + 读 IR（2026-10-03）──────────────────────────────

    #[test]
    fn apply_interleaved_with_a_delta_ir_is_an_identity() {
        let mut s: Vec<f64> = (0..24).map(|i| (i as f64 * 0.37).sin()).collect();
        let before = s.clone();
        apply_interleaved(&mut s, 3, &[1.0]).unwrap();
        for (a, b) in s.iter().zip(&before) {
            assert!((a - b).abs() < 1e-12, "delta IR 应当逐位直通");
        }
    }

    #[test]
    fn apply_interleaved_keeps_length_and_cuts_the_tail() {
        // 8 帧、IR = [0,0,1]（右移 2）⇒ 输出仍是 8 帧：前 2 帧为 0，原最后 2 帧被切掉。
        let mut s: Vec<f64> = (1..=8).map(|i| i as f64).collect();
        apply_interleaved(&mut s, 1, &[0.0, 0.0, 1.0]).unwrap();
        assert_eq!(s.len(), 8, "卷积后长度必须与输入一致");
        assert!(s[0].abs() < 1e-12 && s[1].abs() < 1e-12);
        for i in 0..6 {
            assert!((s[i + 2] - (i as f64 + 1.0)).abs() < 1e-9, "第 {} 个", i + 2);
        }
    }

    #[test]
    fn apply_interleaved_processes_channels_independently() {
        // 立体声：L 第 0 帧、R 第 1 帧各放一个冲激；IR = [0,2]（右移 1、×2）。
        // 两条声道的结果必须各归各（串了就会互相出现在对方的格子里）。
        let mut s = vec![0.0f64; 8];
        s[0] = 1.0; // L 帧 0
        s[3] = 1.0; // R 帧 1
        apply_interleaved(&mut s, 2, &[0.0, 2.0]).unwrap();
        // L：[1,0,0,0]⊗[0,2] = [0,2,0,0]（截到 4）
        // R：[0,1,0,0]⊗[0,2] = [0,0,2,0]（截到 4）
        let want = [0.0, 0.0, 2.0, 0.0, 0.0, 2.0, 0.0, 0.0];
        for (i, (a, b)) in s.iter().zip(&want).enumerate() {
            assert!((a - b).abs() < 1e-9, "第 {i} 个：{a} vs {b}");
        }
    }

    #[test]
    fn apply_interleaved_rejects_bad_shapes() {
        let mut half_frame = vec![0.0; 5];
        assert!(apply_interleaved(&mut half_frame, 2, &[1.0]).is_err(), "半帧");
        let mut even = vec![0.0; 4];
        assert!(apply_interleaved(&mut even, 0, &[1.0]).is_err(), "声道 0");
        assert!(apply_interleaved(&mut even, 2, &[]).is_err(), "空 IR");
    }

    /// 造一个临时 IR WAV（float32，避免量化误差干扰断言）。
    fn write_temp_ir(name: &str, samples: &[f64], rate: u32, channels: u16) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("lunac-tuning-ir-{}-{name}.wav", std::process::id()));
        let a = crate::wav::Audio {
            sample_rate: rate,
            channels,
            format: crate::wav::SampleFormat::Float32,
            samples: samples.to_vec(),
        };
        crate::wav::write(&p, &a).unwrap();
        p
    }

    #[test]
    fn load_ir_reads_a_matching_mono_wav() {
        let p = write_temp_ir("mono48k", &[1.0, 0.5, 0.25], 48000, 1);
        let ir = load_ir(p.to_str().unwrap(), 48000.0).unwrap();
        assert_eq!(ir.len(), 3);
        assert!((ir[0] - 1.0).abs() < 1e-6 && (ir[1] - 0.5).abs() < 1e-6);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn load_ir_rejects_stereo_a_wrong_rate_and_a_missing_file() {
        let stereo = write_temp_ir("stereo48k", &[1.0, 1.0, 0.0, 0.0], 48000, 2);
        let e = load_ir(stereo.to_str().unwrap(), 48000.0).unwrap_err();
        assert!(e.contains("声道"), "多声道 IR 应当被拒：{e}");
        let _ = std::fs::remove_file(&stereo);

        let mono44 = write_temp_ir("mono44k", &[1.0], 44100, 1);
        let e = load_ir(mono44.to_str().unwrap(), 48000.0).unwrap_err();
        assert!(e.contains("重采样") || e.contains("Hz"), "采样率不符应当被拒：{e}");
        let _ = std::fs::remove_file(&mono44);

        let missing = std::env::temp_dir().join("lunac-tuning-ir-does-not-exist.wav");
        assert!(load_ir(missing.to_str().unwrap(), 48000.0).is_err(), "文件不存在");
    }

    // ── 流式卷积器（2026-10-03，给播放链用）──────────────────────────

    #[test]
    fn a_streaming_convolver_with_a_unit_ir_is_a_passthrough() {
        // IR = [1] ⇒ 块长 1、延迟 0，逐样本直通
        let mut c = Convolver::new(&[1.0]).unwrap();
        assert_eq!(c.ir_len(), 1);
        for i in 0..20 {
            let x = (i as f64 * 0.37).sin();
            assert!((c.process_sample(x) - x).abs() < 1e-12, "第 {i} 个");
        }
    }

    /// **流式 vs 离线**：逐样本喂进去，结果除开一个**已知延迟**外必须与
    /// `convolve_direct`（独立参照物）逐点一致 —— 这条钉住块边界与 overlap 尾巴。
    #[test]
    fn the_streaming_convolver_matches_direct_convolution_with_a_known_latency() {
        let signal: Vec<f64> = (0..97)
            .map(|i| ((i * 37 % 101) as f64 - 50.0) / 50.0)
            .collect();
        let ir: Vec<f64> = (0..13).map(|i| ((i * 11 % 17) as f64 - 8.0) / 8.0).collect();
        let direct = convolve_direct(&signal, &ir);

        let mut c = Convolver::new(&ir).unwrap();
        let got: Vec<f64> = signal.iter().map(|&x| c.process_sample(x)).collect();

        let lat = ir.len() - 1;
        for n in 0..lat {
            assert!(got[n].abs() < 1e-12, "前 {lat} 个输出应当是延迟（0），第 {n} 个不是");
        }
        for n in 0..(signal.len() - lat) {
            assert!(
                (got[lat + n] - direct[n]).abs() < 1e-9,
                "第 {n} 个：流式 {} vs 直接 {}",
                got[lat + n],
                direct[n]
            );
        }
    }

    #[test]
    fn resetting_a_streaming_convolver_clears_its_block_and_tail() {
        let ir = [0.3, -0.2, 0.5, 0.1];
        let mut c = Convolver::new(&ir).unwrap();
        let probe: Vec<f64> = (0..40).map(|i| ((i * 7 % 23) as f64 - 11.0) / 11.0).collect();
        // 先喂一半（把块与尾巴都用起来），再 reset，再喂完整的 40 个
        for &x in &probe[..17] {
            c.process_sample(x);
        }
        c.reset();
        let got: Vec<f64> = probe.iter().map(|&x| c.process_sample(x)).collect();

        let mut fresh = Convolver::new(&ir).unwrap();
        let want: Vec<f64> = probe.iter().map(|&x| fresh.process_sample(x)).collect();
        assert_eq!(got, want, "reset 之后必须与全新实例逐位一致");
    }
}
