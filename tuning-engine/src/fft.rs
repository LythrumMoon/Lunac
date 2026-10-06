// ── FFT（测量用）────────────────────────────────────────────────
//
// **只服务测量**：渲染路径（`chain`）是纯时域的双二阶滤波，一次 FFT 都不需要。
// 这里的两件事：把 `measure` 要的「脉冲响应 → 幅度谱」做出来，以及给测试提供
// 一条**与解析增益对拍**的独立通路。
//
// **为什么手写而不是引 crate**（见 Cargo.toml 的注释）：只有 radix-2、只需要正变换，
// 而正确性可以用「朴素 DFT」当参照物钉死 —— 两条独立实现给出同一个数，比信任一个
// 第三方 crate 更硬，也少一条供应链。
//
// **不支持非 2 的幂**：调用方（测量 / 测试）自己零填充到 2 的幂。这条是刻意的 ——
// 要支持任意长度就得上 Bluestein，而本引擎没有任何一处需要它。

use std::f64::consts::PI;

/// 就地 radix-2 FFT（DIT，位反转 + 蝶形）。`re` / `im` 长度必须**相等且是 2 的幂**。
pub fn fft_in_place(re: &mut [f64], im: &mut [f64]) -> Result<(), String> {
    if re.len() != im.len() {
        return Err(format!("实部与虚部长度不一致（{} vs {}）", re.len(), im.len()));
    }
    let n = re.len();
    // `n == 0` 与「不是 2 的幂」都拒：`n & (n-1) != 0` 对 0 也是真，但分开写更好读。
    if n == 0 || n & (n - 1) != 0 {
        return Err(format!("FFT 长度必须是 2 的幂（收到 {n}）"));
    }

    // ① 位反转置换：把「时域自然序」换成「频域位逆序」
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    // ② 蝶形：按 2 / 4 / 8 … 逐级合并
    let mut len = 2usize;
    while len <= n {
        let ang = -2.0 * PI / len as f64; // 正变换 = 负号
        let (wr, wi) = (ang.cos(), ang.sin());
        let half = len / 2;
        let mut base = 0usize;
        while base < n {
            // 旋转因子逐级递推（比每个 k 现算一次 sin/cos 快，且误差不会累积到 0.1dB 量级）
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..half {
                let (ur, ui) = (re[base + k], im[base + k]);
                let (xr, xi) = (re[base + k + half], im[base + k + half]);
                let tr = cr * xr - ci * xi;
                let ti = cr * xi + ci * xr;
                re[base + k] = ur + tr;
                im[base + k] = ui + ti;
                re[base + k + half] = ur - tr;
                im[base + k + half] = ui - ti;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
            base += len;
        }
        len <<= 1;
    }
    Ok(())
}

/// 就地 radix-2 **逆变换**（`ifft(X) = conj(fft(conj(X))) / N`）。
///
/// 蝶形 / 位反转只有一份代码：正变换已被「朴素 DFT 对拍」钉死，逆变换靠上面那条恒等式
/// 复用同一条路径 —— 比再写一遍共轭旋转因子更不容易出错。
///
/// ⚠️ 长度校验必须在**改动 `im` 之前**做：否则入参非法时会把调用方的数组改坏。
pub fn ifft_in_place(re: &mut [f64], im: &mut [f64]) -> Result<(), String> {
    if re.len() != im.len() {
        return Err(format!("实部与虚部长度不一致（{} vs {}）", re.len(), im.len()));
    }
    let n = re.len();
    if n == 0 || n & (n - 1) != 0 {
        return Err(format!("FFT 长度必须是 2 的幂（收到 {n}）"));
    }
    for v in im.iter_mut() {
        *v = -*v;
    }
    fft_in_place(re, im)?;
    let inv = 1.0 / n as f64;
    for i in 0..n {
        re[i] *= inv;
        im[i] = -im[i] * inv;
    }
    Ok(())
}

/// 大于等于 `n` 的最小 2 的幂（`n <= 1` 时返回 1）。
///
/// 存在的理由：`fft_in_place` 只吃 2 的幂，而测量的长度来自「用户要多少个测量点」——
/// 这个函数就是那道转换，且它**只放大不缩小**（宁可多算几个空样本，也不能悄悄截掉尾巴）。
pub fn next_pow2(n: usize) -> usize {
    if n <= 1 {
        return 1;
    }
    n.next_power_of_two()
}

/// 周期型 Hann 窗（`w[k] = 0.5·(1 − cos(2πk/N))`）。
///
/// **用周期型而不是对称型**（`…/(N-1)`）：这里做的是「单段记录的 DFT 分析」，
/// 周期型的 DFT 泄漏与相干增益性质才是教科书给的那一套；对称型是给滤波器设计用的。
pub fn hann_window(n: usize) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    (0..n)
        .map(|k| 0.5 * (1.0 - (2.0 * PI * k as f64 / n as f64).cos()))
        .collect()
}

/// 复频谱的幅度 → dB（只看 `0..=n/2` 这半段，实信号的负频是镜像、没有信息量）。
///
/// **不做归一化**（不给 `20·log10(|X|)` 减任何常数）：调用方要的是「相对增益」，
/// 而相对增益在两处相减时那个常数自动抵消。测量频率响应时更是这样 —— 脉冲响应的
/// DFT 就是频率响应本身，一个系数都不该动。
pub fn spectrum_db(re: &[f64], im: &[f64]) -> Vec<f64> {
    /// 数值下限：`20·log10(0)` 是 −∞，而 −∞ 一旦流进比较 / JSON 就会变成一堆
    /// 「为什么这一格是 null」的谜题。−240 dB 已经远在任何真实判据之下。
    const FLOOR_DB: f64 = -240.0;
    let n = re.len().min(im.len());
    (0..=n / 2)
        .map(|k| {
            let mag = (re[k] * re[k] + im[k] * im[k]).sqrt();
            if mag <= 1e-12 {
                FLOOR_DB
            } else {
                20.0 * mag.log10()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 参照物：按定义直算的 DFT（O(n²)）。**故意写得笨** —— 它唯一的职责是独立于
    /// 被测实现，两条路子给出同一个数才算数（同 ai-spec 里「拿线上报文当证据」的精神）。
    fn naive_dft(re0: &[f64], im0: &[f64]) -> Vec<(f64, f64)> {
        let n = re0.len();
        let mut out = Vec::with_capacity(n);
        for k in 0..n {
            let (mut sr, mut si) = (0.0f64, 0.0f64);
            for t in 0..n {
                let ang = -2.0 * PI * (k as f64) * (t as f64) / (n as f64);
                let (c, s) = (ang.cos(), ang.sin());
                // (re0 + j·im0) · (c + j·s)
                sr += re0[t] * c - im0[t] * s;
                si += re0[t] * s + im0[t] * c;
            }
            out.push((sr, si));
        }
        out
    }

    /// 确定性伪随机（不引 rand crate）：一个固定的整数序列，够乱就行。
    fn sample_seq(n: usize, mul: usize, modu: usize, centre: f64) -> Vec<f64> {
        (0..n).map(|k| ((k * mul) % modu) as f64 - centre).collect()
    }

    #[test]
    fn fft_matches_naive_dft() {
        for n in [2usize, 4, 8, 16, 64] {
            let re0 = sample_seq(n, 7, 13, 6.0);
            let im0 = sample_seq(n, 5, 11, 5.0);
            let mut re = re0.clone();
            let mut im = im0.clone();
            fft_in_place(&mut re, &mut im).expect("2 的幂应当被接受");

            let want = naive_dft(&re0, &im0);
            for k in 0..n {
                assert!(
                    (re[k] - want[k].0).abs() < 1e-9 && (im[k] - want[k].1).abs() < 1e-9,
                    "n={n} k={k}：fft=({}, {})，朴素 DFT=({}, {})",
                    re[k],
                    im[k],
                    want[k].0,
                    want[k].1
                );
            }
        }
    }

    #[test]
    fn fft_rejects_bad_lengths() {
        // 长度不是 2 的幂：必须**当场拒绝**，不能悄悄截断或补零 —— 补零是调用方的事，
        // 藏在底层会让「我量到的谱为什么糊了」变成一道查不出来的谜。
        let mut re = vec![0.0; 6];
        let mut im = vec![0.0; 6];
        let err = fft_in_place(&mut re, &mut im).unwrap_err();
        assert!(err.contains('6'), "错误消息要带上实际长度，收到：{err}");

        // 实部 / 虚部长度不一致
        let mut re = vec![0.0; 8];
        let mut im = vec![0.0; 4];
        assert!(fft_in_place(&mut re, &mut im).is_err());

        // 空输入
        assert!(fft_in_place(&mut [], &mut []).is_err());
    }

    #[test]
    fn ifft_undoes_fft() {
        let n = 64usize;
        let re0 = sample_seq(n, 7, 13, 6.0);
        let im0 = sample_seq(n, 5, 11, 5.0);
        let (mut re, mut im) = (re0.clone(), im0.clone());
        fft_in_place(&mut re, &mut im).unwrap();
        ifft_in_place(&mut re, &mut im).unwrap();
        for k in 0..n {
            assert!(
                (re[k] - re0[k]).abs() < 1e-9 && (im[k] - im0[k]).abs() < 1e-9,
                "k={k}：往返后 ({}, {})，原值 ({}, {})",
                re[k], im[k], re0[k], im0[k]
            );
        }
        // 非法长度必须在**改动入参之前**拒掉（否则会把调用方的数组改坏）
        let mut bad_re = vec![0.0; 6];
        let mut bad_im = vec![3.0; 6];
        assert!(ifft_in_place(&mut bad_re, &mut bad_im).is_err());
        assert_eq!(bad_im, vec![3.0; 6], "被拒时不得改动入参");
    }

    #[test]
    fn next_pow2_rounds_up() {
        assert_eq!(next_pow2(0), 1);
        assert_eq!(next_pow2(1), 1);
        assert_eq!(next_pow2(2), 2);
        assert_eq!(next_pow2(3), 4);
        assert_eq!(next_pow2(5), 8);
        assert_eq!(next_pow2(1024), 1024);
        assert_eq!(next_pow2(1025), 2048);
    }

    #[test]
    fn hann_window_shape() {
        let n = 64;
        let w = hann_window(n);
        assert_eq!(w.len(), n);
        assert!(w[0].abs() < 1e-12, "周期型 Hann 的首个样本是 0，收到 {}", w[0]);
        // 峰值在中点、且是 1
        let peak_idx = w
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        assert_eq!(peak_idx, n / 2);
        assert!((w[peak_idx] - 1.0).abs() < 1e-12, "峰值应为 1，收到 {}", w[peak_idx]);
        // 全程落在 [0, 1]，且**首尾不相连的那一段不为 0**（周期型与对称型的分界）
        for (k, v) in w.iter().enumerate() {
            assert!((0.0..=1.0).contains(v), "k={k} 越界：{v}");
        }
        assert!(w[n - 1] > 0.0, "周期型的末样本不该是 0（对称型才是）");
    }

    #[test]
    fn spectrum_db_peaks_at_the_tone_bin() {
        // 1 kHz / 48 kHz / 1024 点 ⇒ 第 1024·1000/48000 ≈ 21.33 个 bin，整数拍不了，
        // 所以取能被整除的组合：48 kHz 下 1024 点，bin 间隔 46.875 Hz，取 bin 32 = 1500 Hz。
        let n = 1024usize;
        let rate = 48000.0f64;
        let bin = 32usize;
        let freq = bin as f64 * rate / n as f64; // 1500 Hz
        let w = hann_window(n);
        let mut re: Vec<f64> = (0..n)
            .map(|k| (2.0 * PI * freq * k as f64 / rate).sin() * w[k])
            .collect();
        let mut im = vec![0.0f64; n];
        fft_in_place(&mut re, &mut im).unwrap();
        let db = spectrum_db(&re, &im);

        assert_eq!(db.len(), n / 2 + 1, "只该给前半段");
        let (peak_idx, _) = db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        assert_eq!(peak_idx, bin, "峰值应当正好落在音调所在的那一格");
        // 离得远的格子应当低得多（窗型 + DFT 的泄漏抑制）—— 用「至少低 40dB」当判据，
        // 不去钉绝对电平（那取决于相干增益，换个窗就变）。
        assert!(
            db[peak_idx] - db[bin + 8] > 40.0,
            "离开主瓣 8 格应当低 40dB 以上：峰值 {} dB，+8 格 {} dB",
            db[peak_idx],
            db[bin + 8]
        );
    }
}
