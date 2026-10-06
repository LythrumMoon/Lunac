// ── 双二阶滤波器（RBJ cookbook）──────────────────────────────────
//
// 这是引擎的「声学核心」：链上的每一个效果器都是一段双二阶（或一阶）差分方程。
//
// **通用性来自 RBJ（Audio EQ Cookbook）的那一套双线性变换闭式解** —— 它给的是
// **数字域的精确解**（不是模拟原型再近似），所以「某频点的增益 = 设计值」可以由
// 解析式直接算出来，不必靠扫描。`response_db()` 就是那个解析式，它有两个用途：
//   ① 给测量结果当**参照物**（见 `measure` 模块与那批 ±0.1dB 的测试）；
//   ② 让「滤波器设计对不对」这件事在单测里可判（不用先渲染一遍 WAV）。
//
// **一阶那两种（`low_pass_1` / `high_pass_1`）不是 RBJ 表里的式子**，而是从
// 「一阶模拟原型的双线性变换 + 在 f0 预畸变」直接推出来的闭式解：
//     K = tan(π·f0/fs)
//     低通：b = [K/(1+K), K/(1+K), 0]，a = [1, (K-1)/(1+K), 0]
//     高通：b = [1/(1+K), -1/(1+K), 0]，a = [1, (K-1)/(1+K), 0]
//   （两条都满足：低通 DC 增益 1、Nyquist 为 0；高通反之。见 `known_gain_identities` 测试。
//   写出来是因为 RBJ 那张一阶表我记不准，而推导只有三行 —— 记不准的东西不写。）
//
// **各阶的边界**：二阶 8 种 + 一阶 2 种。一阶的 shelf 本切片不做（记在 backlog L5）。

use std::f64::consts::PI;

/// 滤波器类型。名字（`name()`）就是配置 JSON 里写的那个串 —— **两边只此一份**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    // ── 二阶（双二阶）──
    Peaking,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    BandPass,
    Notch,
    AllPass,
    // ── 一阶 ──
    LowPass1,
    HighPass1,
}

impl Kind {
    /// 配置里能写的全部取值（错误消息里要列出来，所以从这一份推导）。
    pub const ALL: [Kind; 10] = [
        Kind::Peaking,
        Kind::LowShelf,
        Kind::HighShelf,
        Kind::LowPass,
        Kind::HighPass,
        Kind::BandPass,
        Kind::Notch,
        Kind::AllPass,
        Kind::LowPass1,
        Kind::HighPass1,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Peaking => "peaking",
            Kind::LowShelf => "low_shelf",
            Kind::HighShelf => "high_shelf",
            Kind::LowPass => "low_pass",
            Kind::HighPass => "high_pass",
            Kind::BandPass => "band_pass",
            Kind::Notch => "notch",
            Kind::AllPass => "all_pass",
            Kind::LowPass1 => "low_pass_1",
            Kind::HighPass1 => "high_pass_1",
        }
    }

    pub fn parse(s: &str) -> Result<Kind, String> {
        Kind::ALL
            .into_iter()
            .find(|k| k.name() == s)
            .ok_or_else(|| {
                let names: Vec<&str> = Kind::ALL.iter().map(|k| k.name()).collect();
                format!("未知的滤波器类型 {s:?}（可用：{}）", names.join(" / "))
            })
    }

    pub fn order(self) -> u8 {
        match self {
            Kind::LowPass1 | Kind::HighPass1 => 1,
            _ => 2,
        }
    }

    /// 这个类型用不用 `gain_db`。**校验要按它判**：给 `high_pass` 配 `gain_db` 是写错了，
    /// 静默忽略会让用户以为那个增益生效了（EAPO 的 `Config` 里也有这一条）。
    pub fn uses_gain(self) -> bool {
        matches!(self, Kind::Peaking | Kind::LowShelf | Kind::HighShelf)
    }
}

/// 一段双二阶的系数（**已归一化到 `a0 = 1`**）。
///
/// 一阶滤波器的 `b2 = a2 = 0` —— 用同一个结构承载，省掉一条分支；
/// 差分方程本身对阶数不敏感。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

/// 逐样本处理的状态（**每个通道、每一段各自一份**，不能共享）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct State {
    z1: f64,
    z2: f64,
}

impl Biquad {
    /// 按类型 / 频率 / 增益 / Q 设计系数。
    ///
    /// **这里只挡「数学上会炸」的入参**（`f0` 落在 `(0, fs/2)` 之外、`Q <= 0`）；
    /// 「增益别超过 ±30dB」这类**策略性**上限在 `config` 里管 —— 两者的分工要守住，
    /// 否则换个调用方（比如将来 AI 直接下参数）就会绕过其中一道。
    pub fn design(
        kind: Kind,
        freq_hz: f64,
        gain_db: f64,
        q: f64,
        sample_rate: f64,
    ) -> Result<Biquad, String> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(format!("采样率非法：{sample_rate}"));
        }
        let nyquist = sample_rate / 2.0;
        // 三个判断都**显式带上 NaN**（`!is_finite()` 在前）：`freq_hz <= 0.0` 对 NaN 是 false，
        // 不写这一半的话 NaN 会一路走到系数里去，最后变成一堆静默的 NaN 样本。
        if !freq_hz.is_finite() || freq_hz <= 0.0 || freq_hz >= nyquist {
            return Err(format!("频率必须落在 (0, {nyquist}) 之间，收到 {freq_hz}"));
        }
        if kind != Kind::LowPass1 && kind != Kind::HighPass1 && (!q.is_finite() || q <= 0.0) {
            return Err(format!("Q 必须为正，收到 {q}"));
        }
        if kind.uses_gain() && !gain_db.is_finite() {
            return Err(format!("增益非法：{gain_db}"));
        }

        if matches!(kind, Kind::LowPass1 | Kind::HighPass1) {
            return Ok(first_order(kind, freq_hz, sample_rate));
        }

        let w0 = 2.0 * PI * freq_hz / sample_rate;
        let (cw, sw) = (w0.cos(), w0.sin());
        // 二阶的带宽参数。shelf 也用这一条（RBJ 明确允许把「斜率 S」换成「Q」来给）。
        let alpha = sw / (2.0 * q);
        // `A = 10^(dBgain/40)`：RBJ 的约定，**除数是 40 不是 20**（幅度取过平方根）。
        let a = if kind.uses_gain() {
            10f64.powf(gain_db / 40.0)
        } else {
            1.0
        };

        // 六个系数一律先按「未归一化」写，最后整体除 a0（见本文件头）。
        let (b0, b1, b2, a0, a1, a2) = match kind {
            Kind::Peaking => (
                1.0 + alpha * a,
                -2.0 * cw,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cw,
                1.0 - alpha / a,
            ),
            Kind::LowShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cw + s),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
                    a * ((a + 1.0) - (a - 1.0) * cw - s),
                    (a + 1.0) + (a - 1.0) * cw + s,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cw),
                    (a + 1.0) + (a - 1.0) * cw - s,
                )
            }
            Kind::HighShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cw + s),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
                    a * ((a + 1.0) + (a - 1.0) * cw - s),
                    (a + 1.0) - (a - 1.0) * cw + s,
                    2.0 * ((a - 1.0) - (a + 1.0) * cw),
                    (a + 1.0) - (a - 1.0) * cw - s,
                )
            }
            Kind::LowPass => (
                (1.0 - cw) / 2.0,
                1.0 - cw,
                (1.0 - cw) / 2.0,
                1.0 + alpha,
                -2.0 * cw,
                1.0 - alpha,
            ),
            Kind::HighPass => (
                (1.0 + cw) / 2.0,
                -(1.0 + cw),
                (1.0 + cw) / 2.0,
                1.0 + alpha,
                -2.0 * cw,
                1.0 - alpha,
            ),
            // 「constant 0 dB peak gain」那一支：中心频点增益正好 1。
            Kind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            Kind::Notch => (1.0, -2.0 * cw, 1.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            Kind::AllPass => (
                1.0 - alpha,
                -2.0 * cw,
                1.0 + alpha,
                1.0 + alpha,
                -2.0 * cw,
                1.0 - alpha,
            ),
            Kind::LowPass1 | Kind::HighPass1 => unreachable!("一阶在上面已经返回"),
        };

        Ok(Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        })
    }

    /// 解析增益：`|H(e^{jω})|` 换算成 dB。**这是「理论值」那一侧**，见文件头。
    pub fn response_db(&self, freq_hz: f64, sample_rate: f64) -> f64 {
        /// 同 `fft::spectrum_db`：别让 −∞ 流出去（陷波正好落在中心频点时会算出 0）。
        const FLOOR_DB: f64 = -240.0;
        let w = 2.0 * PI * freq_hz / sample_rate;
        let num = poly_mag(self.b0, self.b1, self.b2, w);
        let den = poly_mag(1.0, self.a1, self.a2, w);
        if den <= 0.0 || num <= 0.0 {
            return FLOOR_DB;
        }
        20.0 * (num / den).log10()
    }

    /// 逐样本：转置直接 II 型（浮点下数值最稳，且状态只有两个）。
    pub fn process(&self, st: &mut State, x: f64) -> f64 {
        let y = self.b0 * x + st.z1;
        st.z1 = self.b1 * x - self.a1 * y + st.z2;
        st.z2 = self.b2 * x - self.a2 * y;
        y
    }

    /// 极点在单位圆内 ⇒ 稳定。判据用 Jury 判据（不需要解二次方程）：
    /// `|a2| < 1` 且 `|a1| < 1 + a2`。
    pub fn is_stable(&self) -> bool {
        self.a2.abs() < 1.0 && self.a1.abs() < 1.0 + self.a2
    }
}

/// `|c0 + c1·z⁻¹ + c2·z⁻²|` 在 `z = e^{jω}` 上取模。
///
/// 一个二阶多项式的幅度，拆成实部 / 虚部手算即可 —— 为它引一个复数库里外都不值。
/// **注意 `z⁻¹ = e^{−jω}` 的虚部是负号**，写成正号会让整条曲线镜像。
fn poly_mag(c0: f64, c1: f64, c2: f64, w: f64) -> f64 {
    let (s1, c1v) = (w.sin(), w.cos());
    let (s2, c2v) = ((2.0 * w).sin(), (2.0 * w).cos());
    let re = c0 + c1 * c1v + c2 * c2v;
    let im = -(c1 * s1 + c2 * s2);
    (re * re + im * im).sqrt()
}

/// 一阶（RBJ 表外，见文件头的推导）。
fn first_order(kind: Kind, freq_hz: f64, sample_rate: f64) -> Biquad {
    let k = (PI * freq_hz / sample_rate).tan();
    let d = 1.0 + k;
    let a1 = (k - 1.0) / d;
    match kind {
        Kind::LowPass1 => Biquad {
            b0: k / d,
            b1: k / d,
            b2: 0.0,
            a1,
            a2: 0.0,
        },
        Kind::HighPass1 => Biquad {
            b0: 1.0 / d,
            b1: -1.0 / d,
            b2: 0.0,
            a1,
            a2: 0.0,
        },
        _ => unreachable!("first_order 只处理一阶类型"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 48000.0;

    /// 解析增益（不经过 `process`）—— 测试里的「理论值」。
    fn gain(c: &Biquad, f: f64) -> f64 {
        c.response_db(f, FS)
    }

    #[test]
    fn known_gain_identities() {
        // 这一条是**滤波器设计对不对**的总闸：每个类型都有闭式解能钉住的锚点，
        // 系数写错一位就一定挂在这里（比「看着曲线差不多」硬得多）。
        let f0 = 1000.0;
        for gain_db in [-12.0, -3.0, 0.0, 6.0, 12.0] {
            let pk = Biquad::design(Kind::Peaking, f0, gain_db, 1.0, FS).unwrap();
            assert!(
                (gain(&pk, f0) - gain_db).abs() < 1e-6,
                "peaking 在中心频点的增益必须等于设计值：期望 {gain_db}，得到 {}",
                gain(&pk, f0)
            );

            // **锚点必须取在精确的频点上**（0 = DC、fs/2 = Nyquist）—— 取 1Hz 当「近似 DC」
            // 会留下 4e-6dB 的残差，那种容差既拦不住错、又会在换采样率时误报。
            let ls = Biquad::design(Kind::LowShelf, f0, gain_db, 0.707, FS).unwrap();
            assert!(
                (gain(&ls, 0.0) - gain_db).abs() < 1e-6,
                "low_shelf 的 DC 增益必须等于设计值：期望 {gain_db}，得到 {}",
                gain(&ls, 0.0)
            );
            assert!(
                gain(&ls, FS / 2.0).abs() < 1e-6,
                "low_shelf 在 Nyquist 应当正好回到 0dB，得到 {}",
                gain(&ls, FS / 2.0)
            );

            let hs = Biquad::design(Kind::HighShelf, f0, gain_db, 0.707, FS).unwrap();
            assert!(
                (gain(&hs, FS / 2.0) - gain_db).abs() < 1e-6,
                "high_shelf 在 Nyquist 的增益必须等于设计值：期望 {gain_db}，得到 {}",
                gain(&hs, FS / 2.0)
            );
            assert!(
                gain(&hs, 0.0).abs() < 1e-6,
                "high_shelf 在 DC 应当正好回到 0dB，得到 {}",
                gain(&hs, 0.0)
            );
        }

        // 低通：DC 通、Nyquist 堵（两处都是**零点**，所以 Nyquist 直接掉到数值下限）
        let lp = Biquad::design(Kind::LowPass, f0, 0.0, 0.707, FS).unwrap();
        assert!(gain(&lp, 0.0).abs() < 1e-6, "low_pass 的 DC 增益应为 0dB");
        assert!(gain(&lp, FS / 2.0) < -60.0, "low_pass 在 Nyquist 应当很低，得到 {}", gain(&lp, FS / 2.0));

        // 高通：反过来
        let hp = Biquad::design(Kind::HighPass, f0, 0.0, 0.707, FS).unwrap();
        assert!(gain(&hp, FS / 2.0).abs() < 1e-6, "high_pass 在 Nyquist 应为 0dB");
        assert!(gain(&hp, 0.0) < -60.0, "high_pass 在 DC 应当很低，得到 {}", gain(&hp, 0.0));

        // 带通（constant 0dB peak gain 形态）：中心 0dB、两端低
        let bp = Biquad::design(Kind::BandPass, f0, 0.0, 2.0, FS).unwrap();
        assert!(gain(&bp, f0).abs() < 1e-6, "band_pass 在中心应为 0dB，得到 {}", gain(&bp, f0));
        assert!(gain(&bp, 20.0) < -30.0 && gain(&bp, FS / 2.0) < -30.0);

        // 陷波：中心极低、两端 0dB
        let no = Biquad::design(Kind::Notch, f0, 0.0, 8.0, FS).unwrap();
        assert!(gain(&no, f0) < -60.0, "notch 在中心应当很深，得到 {}", gain(&no, f0));
        assert!(gain(&no, 0.0).abs() < 1e-6 && gain(&no, FS / 2.0).abs() < 1e-6);

        // 一阶：低通 DC 通 / 高通 Nyquist 通，且两者在 f0 **正好**是 −3.0103dB
        //（预畸变就是为了这个：K = tan(ω0/2) 时 |H(e^{jω0})| = 1/√2 是恒等式，不是近似）
        let lp1 = Biquad::design(Kind::LowPass1, f0, 0.0, 0.707, FS).unwrap();
        assert!(gain(&lp1, 0.0).abs() < 1e-6, "一阶低通的 DC 增益应为 0dB");
        assert!(
            (gain(&lp1, f0) + 3.010299956639812).abs() < 1e-6,
            "一阶低通在 f0 应当正好是 −3.0103dB，得到 {}",
            gain(&lp1, f0)
        );
        let hp1 = Biquad::design(Kind::HighPass1, f0, 0.0, 0.707, FS).unwrap();
        assert!(gain(&hp1, FS / 2.0).abs() < 1e-6, "一阶高通的 Nyquist 增益应为 0dB");
        assert!(
            (gain(&hp1, f0) + 3.010299956639812).abs() < 1e-6,
            "一阶高通在 f0 应当正好是 −3.0103dB，得到 {}",
            gain(&hp1, f0)
        );
    }

    #[test]
    fn all_pass_is_flat() {
        // 全通的定义就是「只改相位、不改幅度」—— 扫一遍全频段，一个点都不许偏。
        for q in [0.5, 1.0, 4.0] {
            let ap = Biquad::design(Kind::AllPass, 1000.0, 0.0, q, FS).unwrap();
            for i in 0..200 {
                let f = 20.0 * (1000.0f64).powf(i as f64 / 199.0); // 20Hz → 20kHz 对数扫
                assert!(
                    gain(&ap, f).abs() < 1e-9,
                    "all_pass 在 {f:.1}Hz 偏了 {} dB",
                    gain(&ap, f)
                );
            }
        }
    }

    #[test]
    fn higher_q_narrows_the_peaking_band() {
        // Q 的语义：越大越窄。判据取「偏离中心一个倍频程处的提升量」—— 窄的那种更小。
        let wide = Biquad::design(Kind::Peaking, 1000.0, 12.0, 0.5, FS).unwrap();
        let narrow = Biquad::design(Kind::Peaking, 1000.0, 12.0, 6.0, FS).unwrap();
        assert!(
            gain(&narrow, 2000.0) < gain(&wide, 2000.0) - 1.0,
            "Q=6 在 2kHz 处应当比 Q=0.5 窄得多：{} vs {}",
            gain(&narrow, 2000.0),
            gain(&wide, 2000.0)
        );
    }

    #[test]
    fn design_rejects_out_of_range() {
        for f in [0.0, -100.0, FS / 2.0, FS / 2.0 + 1.0, FS, f64::NAN] {
            assert!(
                Biquad::design(Kind::Peaking, f, 0.0, 1.0, FS).is_err(),
                "{f} 不该被接受"
            );
        }
        for q in [0.0, -1.0] {
            assert!(Biquad::design(Kind::Peaking, 1000.0, 0.0, q, FS).is_err());
        }
        assert!(Biquad::design(Kind::Peaking, 1000.0, 0.0, 1.0, 0.0).is_err());
        assert!(Biquad::design(Kind::Peaking, 1000.0, f64::NAN, 1.0, FS).is_err());
    }

    #[test]
    fn coefficients_are_stable_across_the_parameter_grid() {
        // 稳定性是「不炸」的前提：任何一组合法参数设计出来的滤波器，极点都必须在单位圆内。
        for kind in Kind::ALL {
            for f in [20.0, 100.0, 1000.0, 10000.0, 20000.0] {
                for g in [-30.0, -6.0, 0.0, 6.0, 30.0] {
                    for q in [0.1, 0.707, 1.0, 8.0, 20.0] {
                        let c = Biquad::design(kind, f, g, q, FS).unwrap();
                        assert!(
                            c.is_stable(),
                            "不稳定：{} f={f} g={g} q={q}（a1={} a2={}）",
                            kind.name(),
                            c.a1,
                            c.a2
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn process_matches_direct_evaluation() {
        // 转置直接 II 型必须与「按差分方程直算」逐样本一致（同一组系数、同一串输入）。
        // 参照物：y[n] = b0·x[n] + b1·x[n-1] + b2·x[n-2] − a1·y[n-1] − a2·y[n-2]
        let c = Biquad::design(Kind::Peaking, 1000.0, 6.0, 1.5, FS).unwrap();
        let x: Vec<f64> = (0..64)
            .map(|k| (0.31 * k as f64).sin() + 0.2 * (1.7 * k as f64).cos())
            .collect();

        let mut st = State::default();
        let got: Vec<f64> = x.iter().map(|&v| c.process(&mut st, v)).collect();

        let mut want = vec![0.0f64; x.len()];
        for n in 0..x.len() {
            let xn1 = if n >= 1 { x[n - 1] } else { 0.0 };
            let xn2 = if n >= 2 { x[n - 2] } else { 0.0 };
            let yn1 = if n >= 1 { want[n - 1] } else { 0.0 };
            let yn2 = if n >= 2 { want[n - 2] } else { 0.0 };
            want[n] = c.b0 * x[n] + c.b1 * xn1 + c.b2 * xn2 - c.a1 * yn1 - c.a2 * yn2;
        }
        for n in 0..x.len() {
            assert!(
                (got[n] - want[n]).abs() < 1e-12,
                "n={n}：process={} 直算={}",
                got[n],
                want[n]
            );
        }
    }
}
