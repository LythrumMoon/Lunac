// ── 频率响应测量（脉冲响应 → FFT → dB）────────────────────────────
//
// S0 的**验收口径**就在这一块：把一条链的增益量出来，跟解析式（`biquad::response_db`）
// 对到 **±0.1dB**。这条口径同时钉住三件事 —— 系数设计、逐样本处理、FFT —— 三者
// 任何一个错位都会让「实测」离开「理论」。
//
// **方法：冲激响应 + FFT**。喂一个单位脉冲进处理器，得到 `h[n]`，它的 DFT **就是**
// 频率响应 `H(e^{jω})`（定义式，没有任何近似）。两条纪律：
//   ① **不加窗**。加窗是「信号不是冲激」时才需要的补丁；这里 h 本身就是冲激响应，
//      加窗等于把 `H` 乘上一个平滑核，反而把要测的东西改掉了。
//   ② **长度要够**。「够」的判据不是拍脑袋定的时长，而是 `tail_db`（见下）——
//      截断没截干净时，测出来的曲线会在低频 / 高 Q 处出现假波纹，而那种波纹看起来
//      很像「滤波器设计有问题」，会把人往错的方向带。

use crate::fft::{fft_in_place, hann_window, next_pow2, spectrum_db};

/// 一次测量的结果。
#[derive(Debug, Clone)]
pub struct Response {
    pub sample_rate: f64,
    /// 每个点的频率（Hz），与 `db` 一一对应。
    pub freqs: Vec<f64>,
    /// 每个点的增益（dB，**未做任何归一化** —— 见 `fft::spectrum_db` 的注释）。
    pub db: Vec<f64>,
    /// 缓冲区尾部残余电平（dB，相对脉冲响应的峰值）。
    ///
    /// **这是「本次测量可不可信」的判据**：IIR 的冲激响应是无限长的，截到 `fft_len`
    /// 就必然丢尾巴；`tail_db = -120` 说明丢的东西比峰值低 120dB（可以忽略），
    /// 而 `tail_db = -20` 说明截断噪声会直接盖住曲线的细节（这时该加长缓冲区，
    /// **不是**换个滤波器）。所以它跟着结果一起返回、并进 CLI 的输出。
    pub tail_db: f64,
}

/// 用「喂单位脉冲 → 取前 `fft_len` 个输出 → FFT」量频率响应。
///
/// `process` 是**逐样本的处理器**（一个闭包，内部自己持有状态）—— 单个 `Biquad`、
/// 整条链、将来某个外部效果器都能喂进来，测量这一层不需要知道它们的形态。
/// `fft_len` 必须是 2 的幂（`fft::fft_in_place` 的约束，也是 `next_pow2` 的用途）。
pub fn response_by_impulse<F>(
    mut process: F,
    sample_rate: f64,
    fft_len: usize,
) -> Result<Response, String>
where
    F: FnMut(f64) -> f64,
{
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err(format!("采样率非法：{sample_rate}"));
    }
    if fft_len == 0 || fft_len & (fft_len - 1) != 0 {
        return Err(format!("测量缓冲区长度必须是 2 的幂（收到 {fft_len}）"));
    }

    // 喂单位脉冲：第 0 个样本给 1，其余给 0。
    let mut re: Vec<f64> = (0..fft_len)
        .map(|n| process(if n == 0 { 1.0 } else { 0.0 }))
        .collect();
    let tail_db = tail_level_db(&re);

    let mut im = vec![0.0f64; fft_len];
    fft_in_place(&mut re, &mut im)?;
    let db = spectrum_db(&re, &im);
    let freqs = (0..db.len())
        .map(|k| k as f64 * sample_rate / fft_len as f64)
        .collect();

    Ok(Response {
        sample_rate,
        freqs,
        db,
        tail_db,
    })
}

/// 冲激响应尾部的残余电平（dB，相对峰值）。见 `Response::tail_db`。
///
/// 取**最后 10%** 的样本看最大值 —— 取最后一个样本会被恰好过零的采样点骗到，
/// 取整段的能量又分不清「衰减慢」与「起点高」。
pub fn tail_level_db(h: &[f64]) -> f64 {
    const FLOOR_DB: f64 = -240.0;
    if h.is_empty() {
        return FLOOR_DB;
    }
    let peak = h.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
    if peak <= 0.0 {
        return FLOOR_DB;
    }
    let start = h.len() - (h.len() / 10).max(1);
    let tail = h[start..].iter().fold(0.0f64, |m, &v| m.max(v.abs()));
    if tail <= 0.0 {
        return FLOOR_DB;
    }
    20.0 * (tail / peak).log10()
}

/// 一段信号里**最强那个分量**的频率与电平。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneLevel {
    /// 主频（Hz）。**是抛物线插值后的连续值**，不是「落在哪一格」。
    pub freq_hz: f64,
    /// 主频处的电平（dBFS：0dBFS = 满量程正弦，即 `A = 1`）。
    pub db: f64,
    /// 频谱中位数（dBFS）—— 「这段信号是不是一个干净的单音」的粗判据：
    /// 主频离它越远，单音越干净；两者挨得近就说明这是段噪声。
    pub floor_db: f64,
}

/// 零填充倍率。**不是随便定的**（理由见 `tone_level` 里那段）。
const PAD: usize = 8;

/// 最多分析这么多个样本（2^17 = 131072，≈2.7 秒 @48k）。
///
/// 电平测量不需要更长，而**零填充是按倍率放大的**：不做上限，一个十分钟的 WAV
/// 就会要求几百 MB 的 FFT 缓冲区。只取前一段，对「一段稳态的单音」这件事无损。
const MAX_ANALYSIS: usize = 1 << 17;

/// 量一段信号的**主频与电平**（加 Hann 窗 + 8 倍零填充 + 抛物线插值）。
///
/// 两个用处：① S0 的端到端验收 —— `gen` 出来、`render` 过一遍的信号，电平该等于理论值；
/// ② S2 的 loopback —— 那正是「播出一段已知信号、录回来，看它变了多少」，同一个函数。
///
/// **零填充 + 插值不是修饰，是判据的前提**：真实频率很少正好落在格子上，只读峰值格时
/// 扇贝损失最坏会让电平低 1.4dB（Hann 窗），±0.1dB 的判据就整个被盖住了。扇贝损失随
/// 偏移量**平方**衰减，所以 8 倍零填充把最坏情况压到 1/16 格（≈0.02dB），再由对数幅度上
/// 的抛物线顶点补掉最后那一点 —— 这两条都有测试钉着（见本模块末尾那两条）。
///
/// **窗只覆盖真实样本**（长 `l`），补零发生在加窗之后。反过来做（按补零后的长度建窗）
/// 等于把信号拦腰截断，实测会低 3.5dB —— 这个坑踩过。
///
/// 与冲激响应那条（`response_by_impulse`）必须分开：那条量的是**系统**，这条量的是**信号**。
pub fn tone_level(samples: &[f64], sample_rate: f64) -> Result<ToneLevel, String> {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err(format!("采样率非法：{sample_rate}"));
    }
    if samples.len() < 64 {
        return Err(format!("至少要 64 个样本才量得出频谱（收到 {}）", samples.len()));
    }
    let x = &samples[..samples.len().min(MAX_ANALYSIS)];
    let l = x.len();

    let w = hann_window(l);
    // 相干增益：加窗会把信号整体压小，电平要按它补回来。**按定义算**（而不是写死 0.5）——
    // 哪天换成别的窗，这里不用改。
    let cg: f64 = w.iter().sum::<f64>() / l as f64;

    let m = next_pow2(l.saturating_mul(PAD));
    let mut re = vec![0.0f64; m];
    for i in 0..l {
        re[i] = x[i] * w[i];
    }
    let mut im = vec![0.0f64; m];
    fft_in_place(&mut re, &mut im)?;
    let db = spectrum_db(&re, &im);

    // 峰值格：跳过 DC（那里是直流分量，不是我们要的音调）。
    // 上限取 `len - 2` 是为了让抛物线插值永远有左右两个邻居。
    let mut k = 1usize;
    for i in 2..db.len() - 1 {
        if db[i] > db[k] {
            k = i;
        }
    }

    // 对数幅度上的抛物线顶点：p 是相对峰值格的偏移（格）
    let (y0, y1, y2) = (db[k - 1], db[k], db[k + 1]);
    let denom = y0 - 2.0 * y1 + y2;
    let p = if denom.abs() > 1e-12 {
        (0.5 * (y0 - y2) / denom).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let peak_db = y1 - 0.25 * (y0 - y2) * p;
    let freq_hz = (k as f64 + p) * sample_rate / m as f64;

    // 单边幅度：`A = 2·|X| / (l·cg)`（实信号的正频那一半）。这里只把它折成 dB 常数。
    let scale_db = 20.0 * (2.0 / (l as f64 * cg)).log10();

    // 本底：中位数（去掉 DC 与 Nyquist 两格）。用中位数而不是均值 —— 均值会被
    // 主瓣那几格拖高，量出来的「本底」就成了「主瓣的尾巴」。
    let mut half: Vec<f64> = db[1..db.len() - 1].to_vec();
    half.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_db = half[half.len() / 2];

    Ok(ToneLevel {
        freq_hz,
        db: peak_db + scale_db,
        floor_db: median_db + scale_db,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biquad::{Biquad, Kind, State};

    const FS: f64 = 48000.0;
    /// 32768 点 ≈ 0.68 秒 @48k。**这个长度不是拍脑袋的**：测试里衰减最慢的一档是
    /// Q=4 @100Hz（极点半径 ≈ 0.9984 ⇒ 每样本 −0.014dB），32768 点就是 −467dB ——
    /// 远在 `tail_db < −100` 那条断言之下。真正的判据是那条断言，不是这个常量。
    const N: usize = 32768;

    /// 把一段 `Biquad` 包成测量要的逐样本闭包。
    fn biquad_closure(c: Biquad) -> impl FnMut(f64) -> f64 {
        let mut st = State::default();
        move |x| c.process(&mut st, x)
    }

    #[test]
    fn tail_level_db_reads_the_decay() {
        // 全程等幅 ⇒ 0dB
        assert!(tail_level_db(&vec![1.0; 100]).abs() < 1e-9);

        // 精确的台阶：前 90% 是 1.0，后 10% 是 1e-3 ⇒ 正好 −60dB。
        // 用台阶而不是指数，是为了**把「只看最后 10%」这条窗的口径本身**钉死：
        // 若实现改成「看整段」或「只看最后一个样本」，这条立刻挂。
        let mut step = vec![1.0f64; 1000];
        for v in step[900..].iter_mut() {
            *v = 1e-3;
        }
        assert!((tail_level_db(&step) + 60.0).abs() < 1e-9, "台阶应当给出 −60dB");

        // 全零要有个确定的答案，不能是 −∞（那会污染 JSON 与大小比较）
        assert_eq!(tail_level_db(&[0.0; 64]), -240.0);
        assert_eq!(tail_level_db(&[]), -240.0);
    }

    #[test]
    fn response_by_impulse_rejects_bad_length() {
        let c = Biquad::design(Kind::Peaking, 1000.0, 3.0, 1.0, FS).unwrap();
        assert!(response_by_impulse(biquad_closure(c), FS, 100).is_err());
    }

    /// **S0 的验收口径**：实测（脉冲响应 → FFT）与理论（解析式）对到 ±0.1dB。
    ///
    /// 覆盖全部 10 种类型 × 3 个中心频率 × 3 档增益 × 3 档 Q。**这条测试就是
    /// backlog L5 里那句「某滤波器在某频点的增益 = 理论值 ±0.1dB」。**
    #[test]
    fn impulse_measurement_matches_the_analytic_gain_within_a_tenth_of_a_db() {
        let mut comparable_total = 0usize;
        let mut all_total = 0usize;

        for kind in Kind::ALL {
            for f0 in [100.0, 1000.0, 10000.0] {
                for gain_db in [-12.0, 0.0, 6.0] {
                    for q in [0.5, 1.0, 4.0] {
                        if !kind.uses_gain() && gain_db != 0.0 {
                            continue; // 增益对这类滤波器无意义，别拿它当噪声
                        }
                        let c = Biquad::design(kind, f0, gain_db, q, FS).unwrap();
                        let r = response_by_impulse(biquad_closure(c), FS, N).unwrap();

                        // 前提：这次测量没有被截断污染。判据取 −100dB —— 远低于下面
                        // 那 0.1dB 的判据，所以它一旦不成立，先怀疑的是缓冲区长度。
                        assert!(
                            r.tail_db < -100.0,
                            "{} f0={f0} g={gain_db} q={q}：冲激响应没衰减干净（tail={}dB），\
                             这条测量结果不可信 —— 加长 fft_len 再来",
                            kind.name(),
                            r.tail_db
                        );

                        for (i, &f) in r.freqs.iter().enumerate() {
                            all_total += 1;
                            let theory = c.response_db(f, FS);
                            if theory > -60.0 {
                                comparable_total += 1;
                                assert!(
                                    (r.db[i] - theory).abs() <= 0.1,
                                    "{} f0={f0} g={gain_db} q={q} @ {f:.2}Hz：\
                                     实测 {}dB vs 理论 {}dB（差 {}dB）",
                                    kind.name(),
                                    r.db[i],
                                    theory,
                                    r.db[i] - theory
                                );
                            } else {
                                // 理论值已经沉到 −60dB 以下（陷波 / 阻带的零点附近）：
                                // 那里**不作深度承诺** —— 零点是一个点，实测只能给「离它
                                // 最近那一格」的值，两个数没有可比性。但仍然要求它确实沉
                                // 下去了（−40dB），否则「跳过」就变成了放过 bug。
                                assert!(
                                    r.db[i] < -40.0,
                                    "{} f0={f0} g={gain_db} q={q} @ {f:.2}Hz：\
                                     理论 {theory}dB 但实测只有 {}dB —— 该沉的地方没沉",
                                    kind.name(),
                                    r.db[i]
                                );
                            }
                        }
                    }
                }
            }
        }

        // 防止「跳过太多导致判据形同虚设」：可比点必须占绝大多数。
        //（−60dB 以下只出现在零点邻域，真实占比是个位数百分比。）
        let ratio = comparable_total as f64 / all_total as f64;
        assert!(
            ratio > 0.9,
            "可比点只占 {:.1}% —— 阈值是不是被调松了？",
            ratio * 100.0
        );
    }

    // ── `tone_level`：量的是**信号**，不是系统（与上面那批是两条独立通路）──

    const RATE: f64 = 48000.0;
    /// 8192 点 @48k ⇒ 格宽 48000/8192 = 5.859375Hz。
    const FFTN: usize = 8192;
    /// 满量程一半的正弦的理论电平。
    const HALF_SCALE_DB: f64 = -6.020599913279624;

    fn sine(rate: f64, freq: f64, n: usize, amp: f64) -> Vec<f64> {
        (0..n)
            .map(|k| amp * (2.0 * std::f64::consts::PI * freq * k as f64 / rate).sin())
            .collect()
    }

    #[test]
    fn tone_level_reads_a_bin_aligned_sine_exactly() {
        // 第 256 格正好是 1500Hz
        let t = tone_level(&sine(RATE, 1500.0, FFTN, 0.5), RATE).unwrap();
        assert!((t.freq_hz - 1500.0).abs() < 0.05, "频率 {}", t.freq_hz);
        assert!((t.db - HALF_SCALE_DB).abs() < 0.01, "电平 {} dBFS", t.db);
        assert!(
            t.db - t.floor_db > 60.0,
            "本底没拉开：主频 {}dBFS / 本底 {}dBFS",
            t.db,
            t.floor_db
        );
    }

    #[test]
    fn tone_level_is_not_fooled_by_a_tone_between_bins() {
        // 两个最坏位置：① 正好落在两个**原始**格中间（只读峰值格会低 1.4dB）；
        // ② 偏离峰值格 1/16 格 —— 8 倍零填充之后，这才是**填充网格**上的最坏位置。
        for bins in [256.5, 256.0 + 1.0 / 16.0] {
            let f = bins * RATE / FFTN as f64;
            let t = tone_level(&sine(RATE, f, FFTN, 0.5), RATE).unwrap();
            assert!(
                (t.db - HALF_SCALE_DB).abs() < 0.03,
                "{bins} 格：电平 {}dBFS（应为 {HALF_SCALE_DB}）—— 扇贝损失没补回来",
                t.db
            );
            assert!(
                (t.freq_hz - f).abs() < 0.15,
                "{bins} 格：频率 {} 应为 {f}",
                t.freq_hz
            );
        }
    }

    #[test]
    fn tone_level_round_trips_through_a_real_wav() {
        // gen → 写盘 → 读回 → 量。t9 那趟验收的单元版（少了 render 那一步）。
        let a = crate::gen::tone(48000, 1, 1500.0, 0.2, 0.5).unwrap();
        let back = crate::wav::decode(&crate::wav::encode(&a).unwrap()).unwrap();
        let t = tone_level(&back.samples, 48000.0).unwrap();
        assert!((t.freq_hz - 1500.0).abs() < 0.05, "频率 {}", t.freq_hz);
        // 16 位量化误差远在 0.01dB 之下
        assert!((t.db - HALF_SCALE_DB).abs() < 0.01, "电平 {}dBFS", t.db);
    }

    #[test]
    fn tone_level_rejects_bad_input() {
        assert!(tone_level(&[0.0; 63], RATE).is_err(), "太短");
        assert!(tone_level(&[0.0; 1024], 0.0).is_err(), "采样率 0");
    }

    #[test]
    fn tone_level_of_silence_is_a_floor_not_a_panic() {
        let t = tone_level(&[0.0; 2048], RATE).unwrap();
        assert!(t.db < -200.0, "静音的电平 {}dBFS", t.db);
        assert!(t.floor_db.is_finite());
    }
}
