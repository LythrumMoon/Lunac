// ── 效果链与渲染（WAV → DSP → WAV 的中间那一段）────────────────────
//
// 一条链 = **一段 preamp + N 段双二阶**，顺序即处理顺序。就这些 —— 不再长出别的概念。
//
// **preamp 放在最前**，它是整条链的**余量控制**（`x * 10^(preamp_db/20)`）：
// 把它并进第一段的系数也行，但那样「链的总增益」就不再是「preamp + 各段之和」，
// 上面板也好、给 AI 看也好都会绕。放在最前面，`response_db()` 就是一行加法。
//
// **削波在这里不被处理，只被暴露**：DSP 内环全是 f64，`+12dB` 的链喂满量程信号
// 出来就是 2.0 —— 这是**正确**的（浮点域没有削波这回事）。越界只在写整数 WAV 时
// 被钳位，而 `wav::Audio::clipped_samples()` 会把它数出来。所以「防削波」的正确
// 形态是：**渲染完先看峰值与越界样本数，再决定 preamp 减多少** —— 而不是在链里
// 偷偷夹一个 limiter（那会把用户要的增益也一起夹掉，还查不出来）。

use crate::biquad::{Biquad, State};
use crate::config::{ChainConfig, Filter};
use crate::measure::{self, Response};
use crate::wav::Audio;

/// 一条可运行的链（**只有系数，没有状态** —— 状态全在 `ChainRuntime` 里）。
#[derive(Debug, Clone)]
pub struct Chain {
    preamp: f64,
    preamp_db: f64,
    stages: Vec<Biquad>,
    /// 全局延迟的样本数（0 = 不延迟）。按**音频自己的采样率**换算。
    delay_samples: usize,
    /// 声道复制对（`from → to`）。
    copies: Vec<(usize, usize)>,
    /// 卷积用的脉冲响应（`None` = 这条链不做卷积）。在 `build` 时就从磁盘读好并校验
    ///（单声道 + 采样率一致），运行期不再碰文件。
    ir: Option<Vec<f64>>,
    /// 条件段（If / Else，按声道）。系数在这里备好，**每条声道跑哪一串**要到
    /// `ChainRuntime::new` 拿到声道数才能定（见 `CondStage`）。
    cond: Vec<CondStage>,
}

/// 一段条件滤波器组（`ChainConfig::CondBlock` 编译成系数之后的形态）。
#[derive(Debug, Clone)]
struct CondStage {
    /// 命中的声道下标（越界校验在 `ChainRuntime::new`）。
    channel: usize,
    then_stages: Vec<Biquad>,
    else_stages: Vec<Biquad>,
}

/// 把一串**已校验**的滤波器设计成双二阶，并逐段挡住不稳定系数。
///
/// `prefix` 为空 ⇒ 错误是「第 N 段…」（普通 `filters` 的老口径，测试钉着它）；
/// 非空 ⇒ 「`<prefix>` 第 N 段…」（条件段的 then / else，得能定位到是哪一块的哪一边）。
fn build_stages(filters: &[Filter], sample_rate: f64, prefix: &str) -> Result<Vec<Biquad>, String> {
    let mut out = Vec::with_capacity(filters.len());
    for (i, f) in filters.iter().enumerate() {
        let at = if prefix.is_empty() {
            format!("第 {} 段", i + 1)
        } else {
            format!("{prefix} 第 {} 段", i + 1)
        };
        // 配置那一层管「参数是否合理」，这里管「设计结果能不能用」。
        // 不稳定的系数进了渲染只会得到一段爆音，且很难回查到是哪一段。
        let b = Biquad::design(f.kind, f.freq_hz, f.gain_db, f.q, sample_rate)
            .map_err(|e| format!("{at}：{e}"))?;
        if !b.is_stable() {
            return Err(format!(
                "{at}（{}）：设计出的系数不稳定（频率太贴近 Nyquist？）",
                f.kind.name()
            ));
        }
        out.push(b);
    }
    Ok(out)
}

impl Chain {
    /// 把配置编译成滤波器。`sample_rate` 决定频率映射（配置本身不含采样率，
    /// 同一条配置可以在 44.1k / 48k 两个流上各自实例化）。
    pub fn build(cfg: &ChainConfig, sample_rate: f64) -> Result<Chain, String> {
        let stages = build_stages(&cfg.filters, sample_rate, "")?;
        // 条件段：then / else 各编一串系数。**这里只编系数** —— 哪条声道该跑哪一串，
        // 要等 `ChainRuntime::new` 拿到声道数。
        let mut cond = Vec::with_capacity(cfg.cond_blocks.len());
        for (bi, b) in cfg.cond_blocks.iter().enumerate() {
            let at = format!("if_else 第 {} 个块（声道 {}）", bi + 1, b.channel);
            cond.push(CondStage {
                channel: b.channel,
                then_stages: build_stages(&b.then, sample_rate, &format!("{at} 的 then"))?,
                else_stages: build_stages(&b.else_filters, sample_rate, &format!("{at} 的 else"))?,
            });
        }
        // 延迟样本数：`delay_ms` 已由 config 校验过范围，这里再挡一道「程序化调用绕过
        // config」传进来的非法值；按**音频自己的采样率**换算（配置不含采样率）。
        if !cfg.delay_ms.is_finite() || cfg.delay_ms < 0.0 {
            return Err(format!("delay_ms 非法：{}", cfg.delay_ms));
        }
        let delay_samples = (cfg.delay_ms / 1000.0 * sample_rate).round() as usize;
        // 卷积的 IR：**在这里读盘**（配置层不碰文件系统，而校验单声道 / 采样率又必须知道
        // 渲染流的采样率）。读不出来 / 不匹配就整条链编不出来 —— 比「渲染到一半才发现」早。
        let ir = match &cfg.convolution {
            None => None,
            Some(path) => Some(crate::convolution::load_ir(path, sample_rate)?),
        };
        Ok(Chain {
            preamp: db_to_linear(cfg.preamp_db),
            preamp_db: cfg.preamp_db,
            stages,
            delay_samples,
            copies: cfg.channel_copy.clone(),
            ir,
            cond,
        })
    }

    pub fn stages(&self) -> &[Biquad] {
        &self.stages
    }

    /// 卷积用的脉冲响应（`None` = 这条链不做卷积）。
    ///
    /// ⚠️ **`response()` / `response_db()` / `process_sample()` 都不含卷积，也不含条件段**
    ///（If / Else）：前者没有解析参照物，后者是**按声道**的（那两个测量通路是单声道路径）。
    /// 那条 ±0.1dB 的判据只覆盖参数段（preamp + 双二阶 + GraphicEQ 展开段）。卷积自己的
    /// 正确性由 `convolution` 模块拿「朴素直接卷积」对拍钉死；条件段由 `ChainRuntime` 的
    /// 逐声道单测钉死（喂两个不同信号、逐位比对）。真正渲染（`render`）才会把它们用上。
    pub fn ir(&self) -> Option<&[f64]> {
        self.ir.as_deref()
    }

    /// preamp（dB）。单独留着而不是从线性值反算 —— 反算会引入 `log10` 的舍入，
    /// 而「理论总增益 = preamp + 各段之和」这条等式要能被精确断言。
    pub fn preamp_db(&self) -> f64 {
        self.preamp_db
    }

    /// 整条链在某个频点的**理论**增益（dB）：preamp + 各段解析式之和。
    ///
    /// 这是 `response()`（实测）的参照物，也是 t9 那条验收断言的另一半。
    pub fn response_db(&self, freq_hz: f64, sample_rate: f64) -> f64 {
        self.preamp_db
            + self
                .stages
                .iter()
                .map(|c| c.response_db(freq_hz, sample_rate))
                .sum::<f64>()
    }

    /// **某一条声道**在某个频点的理论增益（dB）—— 就是 `response_db` 再加上这条声道
    /// 命中的条件段分支（`channel` 相等走 `then`，否则走 `else`）。
    ///
    /// 面板的「通道槽」画曲线要用它：用户在通道 L 那一槽里调 EQ 时，该看到的是
    /// **这条声道真正会发出来的那条响应**（全局链 + 这条声道的分支），而不是全局那一条。
    /// 不含卷积 —— 同 `response_db`（卷积没有解析参照物，理由见 `ir()` 的注释）。
    ///
    /// ⚠️ 条件段语义是「**逐块**判断」：某个块没命中本声道时，它的 `else` 也会作用到本声道
    /// —— 所以这里是**遍历全部块**、而不是只找 channel 相等的那一个。
    pub fn response_db_for_channel(&self, freq_hz: f64, sample_rate: f64, channel: usize) -> f64 {
        let mut db = self.response_db(freq_hz, sample_rate);
        for b in &self.cond {
            let stages = if b.channel == channel {
                &b.then_stages
            } else {
                &b.else_stages
            };
            db += stages
                .iter()
                .map(|c| c.response_db(freq_hz, sample_rate))
                .sum::<f64>();
        }
        db
    }

    /// 整条链的**实测**频响（喂一遍单位脉冲 → FFT）。判据口径见 `measure` 模块。
    pub fn response(&self, sample_rate: f64, fft_len: usize) -> Result<Response, String> {
        let mut states = vec![State::default(); self.stages.len()];
        measure::response_by_impulse(
            |x| self.process_sample(&mut states, x),
            sample_rate,
            fft_len,
        )
    }

    /// 单声道逐样本。`states` 每段滤波器一份（长度 = 链长）。
    pub fn process_sample(&self, states: &mut [State], x: f64) -> f64 {
        debug_assert_eq!(states.len(), self.stages.len(), "每段滤波器各要一份状态");
        let mut y = x * self.preamp;
        for (c, st) in self.stages.iter().zip(states.iter_mut()) {
            y = c.process(st, y);
        }
        y
    }

    /// 处理一整块**交错**样本（原地）。`states` 长度必须是 `channels × 链长`。
    ///
    /// **每个通道各持一份状态** —— 共享状态会把左右声道串起来（左边先响，
    /// 右边听到的是左边的尾巴），这种错在单声道素材上完全看不出来。
    ///
    /// ⚠️ 本方法**不含延迟与声道复制**（那两样要自己的缓冲，见 `ChainRuntime`）。
    /// 它保留下来是给「只要幅度」的通路用的（`response()` 那条冲激测量），
    /// 而延迟 / 复制只改相位与路由、不改幅度 —— 于是那条判据不受影响。
    /// **真正渲染请用 `ChainRuntime`**（`render()` 已经切过去了）。
    pub fn process_block(
        &self,
        states: &mut [State],
        block: &mut [f64],
        channels: usize,
    ) -> Result<(), String> {
        if channels == 0 {
            return Err("声道数必须为正".to_string());
        }
        if !block.len().is_multiple_of(channels) {
            return Err(format!(
                "样本个数 {} 不是声道数 {channels} 的整数倍",
                block.len()
            ));
        }
        let n = self.stages.len();
        if states.len() != channels * n {
            return Err(format!(
                "状态槽位数 {} 应为 声道数 × 段数 = {}",
                states.len(),
                channels * n
            ));
        }

        for frame in block.chunks_mut(channels) {
            for (ch, x) in frame.iter_mut().enumerate() {
                let base = ch * n;
                let mut y = *x * self.preamp;
                for (i, c) in self.stages.iter().enumerate() {
                    y = c.process(&mut states[base + i], y);
                }
                *x = y;
            }
        }
        Ok(())
    }
}

/// **运行期状态**：每声道的滤波器状态 + 每声道一条延迟线 + 声道复制。
///
/// 为什么必须有它：`process_sample(states, x)` 的 `states` 是 `声道数 × 段数` 的定长
/// `State` 切片 —— 装不下「每声道一条任意长度的延迟线」，也没有「声道路由」的位置。
/// 延迟与声道复制（EAPO 的 `Delay` / channel copy）是**有自己缓冲**的效果，状态必须挂在
/// runtime 上。`Chain` 保持「只有系数」，于是同一条链可被多条 runtime 复用（不同音频 /
/// 不同声道数各自实例化）。
///
/// 处理顺序：① preamp → ② 各段双二阶 → ②′ 条件段（If / Else，按声道）→ ③ 延迟 →
/// ④ 声道复制。
#[derive(Debug, Clone)]
pub struct ChainRuntime {
    stages: Vec<Biquad>,
    preamp: f64,
    channels: usize,
    /// `channels × stages` 个双二阶状态。
    states: Vec<State>,
    /// 延迟线长度（帧）；0 = 不延迟，此时 `delay_buf` 为空。
    delay_samples: usize,
    /// `channels × delay_samples` 的环形缓冲（`delay_samples == 0` 时为空）。
    delay_buf: Vec<Vec<f64>>,
    delay_pos: usize,
    copies: Vec<(usize, usize)>,
    /// 条件段（If / Else）。条件是**静态（按声道）**的 ⇒ 每条声道固定跑其中一支，
    /// 在 `new` 里就能定下来，逐样本不做任何判断。
    cond: Vec<CondStage>,
    /// `channels × per_channel_cond_slots` 个条件段状态。
    cond_states: Vec<State>,
    /// 每条声道给条件段预留的状态槽数（= 各块 `then` + `else` 长度之和；两边都留，
    /// 因为不同声道可能各命中一边）。
    per_channel_cond_slots: usize,
}

impl ChainRuntime {
    pub fn new(chain: &Chain, channels: usize) -> Result<ChainRuntime, String> {
        if channels == 0 {
            return Err("声道数必须为正".to_string());
        }
        for &(from, to) in &chain.copies {
            if from >= channels || to >= channels {
                return Err(format!(
                    "声道复制的下标越界：{from}→{to}（当前 {channels} 个声道）"
                ));
            }
        }
        // 条件段的声道下标同理：越界 = 这个块永远不命中（一个**静默失效**的配置）。
        // 在这里挡住，而不是让它悄悄什么也不做。
        for (bi, b) in chain.cond.iter().enumerate() {
            if b.channel >= channels {
                return Err(format!(
                    "if_else 第 {} 个块的条件声道越界：{}（当前 {channels} 个声道）",
                    bi + 1,
                    b.channel
                ));
            }
        }
        let per_channel_cond_slots = chain
            .cond
            .iter()
            .map(|b| b.then_stages.len() + b.else_stages.len())
            .sum::<usize>();
        let states = vec![State::default(); channels * chain.stages.len()];
        let cond_states = vec![State::default(); channels * per_channel_cond_slots];
        let (delay_buf, delay_samples) = if chain.delay_samples > 0 {
            (
                vec![vec![0.0; chain.delay_samples]; channels],
                chain.delay_samples,
            )
        } else {
            (Vec::new(), 0)
        };
        Ok(ChainRuntime {
            stages: chain.stages.clone(),
            preamp: chain.preamp,
            channels,
            states,
            delay_samples,
            delay_buf,
            delay_pos: 0,
            copies: chain.copies.clone(),
            cond: chain.cond.clone(),
            cond_states,
            per_channel_cond_slots,
        })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// 处理**一帧**（`frame.len()` 必须等于声道数），原地改。
    pub fn process_frame(&mut self, frame: &mut [f64]) -> Result<(), String> {
        if frame.len() != self.channels {
            return Err(format!(
                "一帧应有 {} 个样本，收到 {}",
                self.channels,
                frame.len()
            ));
        }
        // ① + ② + ②′：preamp、各段双二阶、条件段（每声道各持一份状态）
        let n = self.stages.len();
        for (ch, x) in frame.iter_mut().enumerate() {
            let base = ch * n;
            let mut y = *x * self.preamp;
            for (i, c) in self.stages.iter().enumerate() {
                y = c.process(&mut self.states[base + i], y);
            }
            // ②′ 条件段（If / Else）：这条声道该走 then 还是 else 是**静态**的
            //（`new` 已校验过声道下标），逐样本只做「跑哪一串」这个查表。
            let mut slot = ch * self.per_channel_cond_slots;
            for b in &self.cond {
                let stages = if b.channel == ch { &b.then_stages } else { &b.else_stages };
                for (i, c) in stages.iter().enumerate() {
                    y = c.process(&mut self.cond_states[slot + i], y);
                }
                slot += b.then_stages.len() + b.else_stages.len();
            }
            *x = y;
        }
        // ③ 延迟（所有声道同量；环形缓冲：先读最老的、再写入当前）
        if self.delay_samples > 0 {
            let pos = self.delay_pos;
            for ch in 0..self.channels {
                let buf = &mut self.delay_buf[ch];
                let out = buf[pos];
                buf[pos] = frame[ch];
                frame[ch] = out;
            }
            self.delay_pos = (pos + 1) % self.delay_samples;
        }
        // ④ 声道复制：**先取快照再写**，于是 `0→1, 1→2` 这类链式复制与遍历顺序无关。
        if !self.copies.is_empty() {
            let src = frame.to_vec();
            for &(from, to) in &self.copies {
                frame[to] = src[from];
            }
        }
        Ok(())
    }

    /// 把所有**运行期状态**清回初始：每声道滤波器状态、条件段状态、延迟线、延迟位置。
    ///
    /// **换流 / seek 之后必须调** —— 那些状态属于「上一段音频」，留着会带一声爆音出来
    /// （宿主 `player.rs` 的 `TuningSource::try_seek` 就是唯一调用点）。
    pub fn reset(&mut self) {
        for s in self.states.iter_mut() {
            *s = State::default();
        }
        for s in self.cond_states.iter_mut() {
            *s = State::default();
        }
        for buf in self.delay_buf.iter_mut() {
            for v in buf.iter_mut() {
                *v = 0.0;
            }
        }
        self.delay_pos = 0;
    }

    /// 处理一整块**交错**样本（原地）。长度必须是声道数的整数倍。
    pub fn process_interleaved(&mut self, block: &mut [f64]) -> Result<(), String> {
        if self.channels == 0 {
            return Err("声道数必须为正".to_string());
        }
        if !block.len().is_multiple_of(self.channels) {
            return Err(format!(
                "样本个数 {} 不是声道数 {} 的整数倍",
                block.len(),
                self.channels
            ));
        }
        for frame in block.chunks_mut(self.channels) {
            self.process_frame(frame)?;
        }
        Ok(())
    }
}

/// 按配置处理一段音频（保持采样率 / 声道数 / 样本格式 / **长度**不变）。
///
/// **采样率取音频自己的那个** —— 配置里不写采样率，就是为了同一条配置能用在
/// 44.1k 与 48k 两条流上。如果某个滤波器的频率在新采样率下越过了 Nyquist，
/// `Chain::build` 会带着段号报错，不会默默算出一段扭曲的曲线。
///
/// 处理顺序：① preamp ② 各段双二阶 ②′ 条件段（If / Else，按声道）③ 延迟 ④ 声道复制
///（这五步在 `ChainRuntime` 里）⑤ **卷积**。卷积放最后：单声道 IR 对每条声道相同，与前面的
/// 延迟 / 声道复制可交换，放最后省一次缓冲；`apply_interleaved` 会把多出的尾巴截掉，
/// 所以长度仍与输入一致。
pub fn render(input: &Audio, cfg: &ChainConfig) -> Result<Audio, String> {
    let channels = input.channels as usize;
    if channels == 0 {
        return Err("音频没有声道".to_string());
    }
    let chain = Chain::build(cfg, input.sample_rate as f64)?;

    let mut out = input.clone();
    // 走 runtime（而不是旧的 `process_block`）：这样**延迟与声道复制也会生效**，
    // 与 `ChainConfig` 的处理顺序一致。
    let mut rt = ChainRuntime::new(&chain, channels)?;
    rt.process_interleaved(&mut out.samples)?;
    // ⑤ 卷积（只有配了 IR 才做）。
    if let Some(ir) = chain.ir() {
        crate::convolution::apply_interleaved(&mut out.samples, channels, ir)?;
    }
    Ok(out)
}

fn db_to_linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biquad::Kind;
    use crate::wav::SampleFormat;

    const FS: u32 = 48000;
    /// 与 `measure` 同一口径的缓冲区长度（理由见那边：判据是 `tail_db`，不是这个数）。
    const N: usize = 32768;

    fn cfg(json: &str) -> ChainConfig {
        ChainConfig::from_json(json, FS as f64).unwrap()
    }

    fn tone(rate: u32, ch: u16, freq: f64, frames: usize, amp: f64) -> Audio {
        let mut samples = Vec::with_capacity(frames * ch as usize);
        for n in 0..frames {
            let v = amp * (2.0 * std::f64::consts::PI * freq * n as f64 / rate as f64).sin();
            for _ in 0..ch {
                samples.push(v);
            }
        }
        Audio {
            sample_rate: rate,
            channels: ch,
            format: SampleFormat::Pcm16,
            samples,
        }
    }

    #[test]
    fn an_empty_chain_is_a_bit_exact_passthrough() {
        // 没有滤波器、preamp 0dB ⇒ 输出与输入逐位相同（不是「近似相同」）。
        // 这条守的是「渲染不能凭空改变素材」——任何多余的乘除都会在这里露出来。
        let a = tone(FS, 2, 997.0, 512, 0.7);
        let out = render(&a, &cfg("{}")).unwrap();
        assert_eq!(out.channels, a.channels);
        assert_eq!(out.sample_rate, a.sample_rate);
        assert_eq!(out.format, a.format);
        assert_eq!(out.samples, a.samples);
    }

    #[test]
    fn preamp_shifts_the_whole_response_by_exactly_its_value() {
        let with = Chain::build(&cfg(r#"{ "preamp_db": -6.0 }"#), FS as f64).unwrap();
        let without = Chain::build(&cfg("{}"), FS as f64).unwrap();

        assert_eq!(with.preamp_db(), -6.0);
        for f in [20.0, 100.0, 1000.0, 15000.0, 20000.0] {
            let d = with.response_db(f, FS as f64) - without.response_db(f, FS as f64);
            assert!((d + 6.0).abs() < 1e-12, "@{f}Hz 的偏移是 {d}dB，应为 −6dB");
        }
    }

    /// **t7 的核心**：把 `measure` 那条 ±0.1dB 的口径从「单个滤波器」升到「整条链」。
    #[test]
    fn the_whole_chain_matches_its_own_theory_within_a_tenth_of_a_db() {
        let cfg = cfg(
            r#"{
                "preamp_db": -3.0,
                "filters": [
                    { "kind": "low_shelf",  "freq_hz": 120,  "gain_db": 4,  "q": 0.7 },
                    { "kind": "peaking",    "freq_hz": 250,  "gain_db": -6, "q": 2.5 },
                    { "kind": "notch",      "freq_hz": 1000, "q": 4 },
                    { "kind": "peaking",    "freq_hz": 2500, "gain_db": 5,  "q": 1.2 },
                    { "kind": "high_shelf", "freq_hz": 9000, "gain_db": -4 },
                    { "kind": "high_pass_1","freq_hz": 30 }
                ]
            }"#,
        );
        let chain = Chain::build(&cfg, FS as f64).unwrap();
        assert_eq!(chain.stages().len(), 6);

        let r = chain.response(FS as f64, N).unwrap();
        assert!(
            r.tail_db < -100.0,
            "冲激响应没衰减干净（tail={}dB）⇒ 这次测量不可信，先加长 fft_len",
            r.tail_db
        );

        let mut comparable = 0usize;
        let mut total = 0usize;
        for (i, &f) in r.freqs.iter().enumerate() {
            total += 1;
            let theory = chain.response_db(f, FS as f64);
            if theory > -60.0 {
                comparable += 1;
                assert!(
                    (r.db[i] - theory).abs() <= 0.1,
                    "@{f:.2}Hz：实测 {}dB vs 理论 {}dB（差 {}dB）",
                    r.db[i],
                    theory,
                    r.db[i] - theory
                );
            } else {
                // 陷波零点邻域：不作深度承诺，但必须确实沉下去了（口径同 `measure`）
                assert!(
                    r.db[i] < -40.0,
                    "@{f:.2}Hz：理论 {theory}dB 但实测只有 {}dB —— 该沉的地方没沉",
                    r.db[i]
                );
            }
        }
        assert!(
            comparable as f64 / total as f64 > 0.9,
            "可比点只占 {:.1}%",
            comparable as f64 / total as f64 * 100.0
        );
    }

    #[test]
    fn each_channel_keeps_its_own_state() {
        // 左右两声道喂**不同的信号**，各自的结果必须与「单独渲染它」逐位一致。
        // 状态一旦被共享，这条立刻挂（而共享状态在单声道素材上根本看不出来）。
        let l = tone(FS, 1, 1000.0, 4000, 0.5);
        let r = tone(FS, 1, 3000.0, 4000, 0.5);
        let mut stereo = l.clone();
        stereo.channels = 2;
        stereo.samples.clear();
        for i in 0..l.samples.len() {
            stereo.samples.push(l.samples[i]);
            stereo.samples.push(r.samples[i]);
        }

        let c = cfg(r#"{ "filters": [{ "kind": "peaking", "freq_hz": 1000, "gain_db": 9, "q": 3 }] }"#);
        let out = render(&stereo, &c).unwrap();
        let out_l = render(&l, &c).unwrap();
        let out_r = render(&r, &c).unwrap();

        assert_eq!(out.samples.len(), stereo.samples.len());
        for i in 0..out_l.samples.len() {
            assert_eq!(out.samples[2 * i], out_l.samples[i], "左声道第 {i} 个样本");
            assert_eq!(out.samples[2 * i + 1], out_r.samples[i], "右声道第 {i} 个样本");
        }
    }

    #[test]
    fn preamp_is_what_keeps_a_boosted_chain_from_clipping() {
        // +12dB 的峰 + 满量程一半的信号 ⇒ 浮点域出来接近 2.0（会削波）。
        // preamp −12dB 把同样的链拉回 0dB 总增益 ⇒ 一个越界样本都不该有。
        // 这就是「preamp 防削波」在 S0 里的**可判形态**。
        let boost = cfg(r#"{ "filters": [{ "kind": "peaking", "freq_hz": 1000, "gain_db": 12, "q": 1.0 }] }"#);
        let trimmed = cfg(
            r#"{ "preamp_db": -12.0,
                 "filters": [{ "kind": "peaking", "freq_hz": 1000, "gain_db": 12, "q": 1.0 }] }"#,
        );
        let input = tone(FS, 1, 1000.0, 4800, 0.5);

        let loud = render(&input, &boost).unwrap();
        assert!(
            loud.clipped_samples() > 0,
            "峰值只有 {} —— 这个用例没造出削波，判据就形同虚设",
            loud.peak()
        );

        let safe = render(&input, &trimmed).unwrap();
        assert_eq!(safe.clipped_samples(), 0, "峰值 {} 仍然越界", safe.peak());
        // 取**后半段**的峰值 —— 开头那一下是滤波器的启动瞬态，不是稳态电平
        let tail = safe.samples[2400..]
            .iter()
            .fold(0.0f64, |m, v| m.max(v.abs()));
        assert!((tail - 0.5).abs() < 0.01, "稳态峰值是 {tail}，应为 0.5");

        // 总增益也要对得上：+12 − 12 = 0dB
        let chain = Chain::build(&trimmed, FS as f64).unwrap();
        assert!(chain.response_db(1000.0, FS as f64).abs() < 1e-9);
    }

    #[test]
    fn build_reports_which_stage_is_the_problem() {
        // 程序化调用（将来的 AI / 面板）绕过了 `config` 的校验，`build` 必须自己站住。
        // 第一段合法、第二段越界 ⇒ 报错必须点名**第二段**。
        let bad = ChainConfig {
            preamp_db: 0.0,
            filters: vec![
                crate::config::Filter {
                    kind: Kind::Peaking,
                    freq_hz: 1000.0,
                    gain_db: 3.0,
                    q: 1.0,
                },
                crate::config::Filter {
                    kind: Kind::Peaking,
                    freq_hz: 24000.0, // == Nyquist
                    gain_db: 3.0,
                    q: 1.0,
                },
            ],
            delay_ms: 0.0,
            channel_copy: Vec::new(),
            convolution: None,
            cond_blocks: Vec::new(),
        };
        let e = Chain::build(&bad, FS as f64).unwrap_err();
        assert!(e.contains("第 2 段"), "{e}");
    }

    #[test]
    fn process_block_rejects_shapes_it_cannot_process() {
        let chain = Chain::build(&cfg(r#"{ "filters": [{ "kind": "notch", "freq_hz": 1000 }] }"#), FS as f64).unwrap();
        let mut block = vec![0.0; 4];

        let mut st = vec![State::default(); 2];
        assert!(chain.process_block(&mut st, &mut block, 0).is_err(), "声道数 0");

        // 5 个样本 / 2 声道 ⇒ 半帧
        let mut block5 = vec![0.0; 5];
        assert!(chain.process_block(&mut st, &mut block5, 2).is_err());

        // 立体声要 2 份状态，这里只有 1 份
        let mut st1 = vec![State::default(); 1];
        assert!(chain.process_block(&mut st1, &mut block, 2).is_err());

        // 正确的形状能过
        assert!(chain.process_block(&mut st, &mut block, 2).is_ok());
    }

    #[test]
    fn render_respects_the_audio_own_sample_rate() {
        // 同一条配置用在 44.1k 上必须能过（配置里没有采样率，就是为了这个）
        let c = cfg(r#"{ "filters": [{ "kind": "high_shelf", "freq_hz": 8000, "gain_db": -3 }] }"#);
        let a = tone(44100, 2, 1000.0, 1000, 0.4);
        let out = render(&a, &c).unwrap();
        assert_eq!(out.sample_rate, 44100);
        assert_eq!(out.samples.len(), a.samples.len());

        // 但 23kHz 在 44.1k 上越过了 Nyquist（22.05k）⇒ 必须报错而不是硬算
        let too_high = cfg(r#"{ "filters": [{ "kind": "notch", "freq_hz": 23000 }] }"#);
        assert!(render(&a, &too_high).is_err());
    }

    // ── 延迟与声道复制（ChainRuntime，2026-10-03）──────────────────

    #[test]
    fn delay_shifts_the_signal_by_exactly_n_samples() {
        // 1ms @48k = 48 样本。单位冲激 ⇒ 第 48 个样本出，前后都是 0。
        let c = cfg(r#"{ "delay_ms": 1.0 }"#);
        let mut a = tone(FS, 1, 1000.0, 256, 0.0); // 全零
        a.samples[0] = 1.0; // 单位冲激
        let out = render(&a, &c).unwrap();
        assert_eq!(out.samples.len(), a.samples.len());
        for i in 0..48 {
            assert!(out.samples[i].abs() < 1e-15, "第 {i} 个样本应为 0");
        }
        assert!((out.samples[48] - 1.0).abs() < 1e-12, "第 48 个样本应为 1，收到 {}", out.samples[48]);
        for i in 49..out.samples.len() {
            assert!(out.samples[i].abs() < 1e-15, "第 {i} 个样本应为 0");
        }
    }

    #[test]
    fn a_zero_delay_is_a_bit_exact_passthrough() {
        let a = tone(FS, 2, 997.0, 512, 0.7);
        let out = render(&a, &cfg(r#"{ "delay_ms": 0 }"#)).unwrap();
        assert_eq!(out.samples, a.samples);
    }

    #[test]
    fn channel_copy_mirrors_one_channel_into_another() {
        // 立体声：L = 1kHz 正弦、R = 全零；把 0→1 复制 ⇒ 两条声道逐位相同。
        let l = tone(FS, 1, 1000.0, 480, 0.5);
        let mut stereo = l.clone();
        stereo.channels = 2;
        stereo.samples.clear();
        for i in 0..l.samples.len() {
            stereo.samples.push(l.samples[i]);
            stereo.samples.push(0.0);
        }
        let out = render(&stereo, &cfg(r#"{ "channel_copy": [ { "from": 0, "to": 1 } ] }"#)).unwrap();
        for i in 0..l.samples.len() {
            assert_eq!(out.samples[2 * i], out.samples[2 * i + 1], "第 {i} 帧左右应相同");
        }
    }

    #[test]
    fn channel_copy_rejects_out_of_range_indices() {
        let chain = Chain::build(
            &cfg(r#"{ "channel_copy": [ { "from": 0, "to": 5 } ] }"#),
            FS as f64,
        )
        .unwrap();
        let e = ChainRuntime::new(&chain, 2).unwrap_err();
        assert!(e.contains("越界"), "{e}");
    }

    #[test]
    fn runtime_rejects_bad_frame_and_block_shapes() {
        let chain = Chain::build(&cfg(r#"{ "filters": [{ "kind": "notch", "freq_hz": 1000 }] }"#), FS as f64).unwrap();
        let mut rt = ChainRuntime::new(&chain, 2).unwrap();
        assert_eq!(rt.channels(), 2);
        // 一帧给 3 个样本
        let mut three = [0.0; 3];
        assert!(rt.process_frame(&mut three).is_err());
        // 5 个样本 / 2 声道 ⇒ 半帧
        let mut block5 = vec![0.0; 5];
        assert!(rt.process_interleaved(&mut block5).is_err());
        // 正确形状能过
        let mut block4 = vec![0.0; 4];
        assert!(rt.process_interleaved(&mut block4).is_ok());
    }

    #[test]
    fn delay_and_copies_leave_the_magnitude_response_alone() {
        // 延迟 / 复制只改相位与路由，不改幅度 —— 频响判据（±0.1dB）不受影响。
        let c = cfg(r#"{ "delay_ms": 2.0, "filters": [ { "kind": "peaking", "freq_hz": 1000, "gain_db": 6, "q": 2 } ] }"#);
        let chain = Chain::build(&c, FS as f64).unwrap();
        assert!((chain.response_db(1000.0, FS as f64) - 6.0).abs() < 1e-9);
    }

    // ── 卷积接入渲染（2026-10-03）────────────────────────────────────

    /// 造一个临时 IR WAV（float32，避免量化误差干扰断言）。
    fn write_ir(name: &str, samples: &[f64], rate: u32, channels: u16) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("lunac-tuning-chain-ir-{}-{name}.wav", std::process::id()));
        let a = Audio {
            sample_rate: rate,
            channels,
            format: SampleFormat::Float32,
            samples: samples.to_vec(),
        };
        crate::wav::write(&p, &a).unwrap();
        p
    }

    /// 把路径拼进配置 JSON（`Debug` 会把反斜杠转义成 `\\`，正好是合法 JSON 字符串）。
    fn cfg_with_ir(p: &std::path::Path) -> ChainConfig {
        cfg(&format!(r#"{{ "convolution": {:?} }}"#, p.to_str().unwrap()))
    }

    #[test]
    fn convolution_with_a_delta_ir_is_a_bit_exact_passthrough() {
        let p = write_ir("delta", &[1.0], FS, 1);
        let a = tone(FS, 2, 997.0, 512, 0.7);
        let out = render(&a, &cfg_with_ir(&p)).unwrap();
        assert_eq!(out.samples.len(), a.samples.len());
        for (x, y) in out.samples.iter().zip(&a.samples) {
            assert!((x - y).abs() < 1e-6, "delta IR 应当直通：{x} vs {y}");
        }
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn convolution_shifts_by_the_ir_delay_without_changing_the_length() {
        // IR = [0,0,1]（右移 2 帧）⇒ 长度不变、冲激落在第 2 帧、末尾尾巴被切掉。
        let p = write_ir("delay2", &[0.0, 0.0, 1.0], FS, 1);
        let mut a = tone(FS, 1, 1000.0, 64, 0.0);
        a.samples[0] = 1.0;
        let out = render(&a, &cfg_with_ir(&p)).unwrap();
        assert_eq!(out.samples.len(), 64, "卷积后长度必须与输入一致");
        assert!(out.samples[0].abs() < 1e-9 && out.samples[1].abs() < 1e-9);
        assert!((out.samples[2] - 1.0).abs() < 1e-6, "第 2 帧应为 1");
        for i in 3..64 {
            assert!(out.samples[i].abs() < 1e-9, "第 {i} 帧应为 0");
        }
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn convolution_scales_the_signal_by_the_ir_gain() {
        // 单点 IR = [0.5] ⇒ 逐样本 ×0.5（乘性增益，位置不动）。
        let p = write_ir("half", &[0.5], FS, 1);
        let a = tone(FS, 1, 997.0, 256, 0.7);
        let out = render(&a, &cfg_with_ir(&p)).unwrap();
        assert_eq!(out.samples.len(), a.samples.len());
        for (x, y) in out.samples.iter().zip(&a.samples) {
            assert!((x - y * 0.5).abs() < 1e-6, "{x} 应为 {}", y * 0.5);
        }
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn convolution_rejects_a_missing_ir_or_a_wrong_rate() {
        // 文件不存在 ⇒ 编链就失败（而不是渲染到一半才发现）
        let c = cfg(r#"{ "convolution": "Z:\\definitely\\missing\\ir.wav" }"#);
        assert!(Chain::build(&c, FS as f64).is_err(), "缺失的 IR 必须报错");

        // 采样率不符 ⇒ 拒绝（本引擎不做重采样）
        let p = write_ir("rate44", &[1.0], 44100, 1);
        let e = Chain::build(&cfg_with_ir(&p), FS as f64).unwrap_err();
        assert!(e.contains("重采样") || e.contains("Hz"), "{e}");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn convolution_is_ignored_by_the_parametric_response_measurement() {
        // 响应测量只覆盖参数段（preamp + 双二阶）—— 配了 IR 也不该改变 `response_db`。
        // 这是刻意的分工：任意 IR 没有解析参照物，±0.1dB 判据不把它算进来。
        let p = write_ir("narrow", &[1.0, 0.0, 0.0, 0.0], FS, 1);
        let c = cfg_with_ir(&p);
        let chain = Chain::build(&c, FS as f64).unwrap();
        assert!(chain.ir().is_some(), "IR 应已读进链里");
        assert!(chain.response_db(1000.0, FS as f64).abs() < 1e-12);
        let _ = std::fs::remove_file(&p);
    }

    // ── 条件段 If / Else（2026-10-03）────────────────────────────────

    /// 左右喂**同一个** 1kHz 正弦的立体声素材。
    fn identical_stereo(frames: usize) -> Audio {
        let one = tone(FS, 1, 1000.0, frames, 0.5);
        let mut stereo = one.clone();
        stereo.channels = 2;
        stereo.samples.clear();
        for v in &one.samples {
            stereo.samples.push(*v);
            stereo.samples.push(*v);
        }
        stereo
    }

    const PEAK_9DB: &str =
        r#"{ "kind": "peaking", "freq_hz": 1000, "gain_db": 9, "q": 3 }"#;

    #[test]
    fn if_else_applies_filters_only_to_the_named_channel() {
        // 只给声道 0 挂 +9dB 的 peaking ⇒ 左声道与「单独渲染同一条链」**逐位一致**，
        // 右声道**原样不动**（`then` 命中 0、其余声道两串都空）。
        let stereo = identical_stereo(4000);
        let one = tone(FS, 1, 1000.0, 4000, 0.5);
        let c = cfg(&format!(
            r#"{{ "if_else": [ {{ "channel": 0, "then": [ {PEAK_9DB} ] }} ] }}"#
        ));
        let out = render(&stereo, &c).unwrap();
        let left = render(&one, &cfg(&format!(r#"{{ "filters": [ {PEAK_9DB} ] }}"#))).unwrap();

        assert_eq!(out.samples.len(), stereo.samples.len());
        for i in 0..one.samples.len() {
            assert_eq!(out.samples[2 * i], left.samples[i], "左声道第 {i} 个样本");
            assert_eq!(out.samples[2 * i + 1], one.samples[i], "右声道第 {i} 个样本应原样");
        }
    }

    #[test]
    fn if_else_else_branch_covers_the_other_channels() {
        // 声道 0 的 `then` 为空 ⇒ 只有「不命中」的声道走 `else` 的 +9dB。
        // 左声道原样、右声道被抬高 —— 这正是 If/Else 的「否则」分支。
        let stereo = identical_stereo(4000);
        let one = tone(FS, 1, 1000.0, 4000, 0.5);
        let c = cfg(&format!(
            r#"{{ "if_else": [ {{ "channel": 0, "then": [], "else": [ {PEAK_9DB} ] }} ] }}"#
        ));
        let out = render(&stereo, &c).unwrap();
        let right = render(&one, &cfg(&format!(r#"{{ "filters": [ {PEAK_9DB} ] }}"#))).unwrap();

        for i in 0..one.samples.len() {
            assert_eq!(out.samples[2 * i], one.samples[i], "左声道第 {i} 个样本应原样");
            assert_eq!(out.samples[2 * i + 1], right.samples[i], "右声道第 {i} 个样本");
        }
    }

    #[test]
    fn per_channel_response_adds_the_branch_that_channel_takes() {
        // 面板的「通道槽」画曲线用的就是这条：全局链 **加上** 本声道命中的那一支。
        // 判据：与「把那一支直接拼进全局 filters」的另一条链逐点比 —— 频响相乘 ⇔ dB 相加，
        // 所以两者必须逐位相等（拿 ±0.1dB 那种实测口径反而松了）。
        let c = cfg(&format!(
            r#"{{ "preamp_db": -2.0,
                 "filters": [ {{ "kind": "peaking", "freq_hz": 500, "gain_db": 3, "q": 1.0 }} ],
                 "if_else": [ {{ "channel": 0, "then": [ {PEAK_9DB} ],
                                 "else": [ {{ "kind": "low_shelf", "freq_hz": 120, "gain_db": 6 }} ] }} ] }}"#
        ));
        let chain = Chain::build(&c, FS as f64).unwrap();
        let hit = Chain::build(
            &cfg(&format!(
                r#"{{ "preamp_db": -2.0,
                     "filters": [ {{ "kind": "peaking", "freq_hz": 500, "gain_db": 3, "q": 1.0 }}, {PEAK_9DB} ] }}"#
            )),
            FS as f64,
        )
        .unwrap();
        let miss = Chain::build(
            &cfg(
                r#"{ "preamp_db": -2.0,
                     "filters": [ { "kind": "peaking", "freq_hz": 500, "gain_db": 3, "q": 1.0 },
                                  { "kind": "low_shelf", "freq_hz": 120, "gain_db": 6 } ] }"#,
            ),
            FS as f64,
        )
        .unwrap();

        for f in [20.0, 120.0, 500.0, 1000.0, 5000.0, 20000.0] {
            let a = chain.response_db_for_channel(f, FS as f64, 0);
            let want = hit.response_db(f, FS as f64);
            assert!((a - want).abs() < 1e-12, "命中声道 @{f}Hz：{a} vs {want}");
            let b = chain.response_db_for_channel(f, FS as f64, 1);
            let want = miss.response_db(f, FS as f64);
            assert!((b - want).abs() < 1e-12, "未命中声道 @{f}Hz：{b} vs {want}");
        }
        // 没有条件段时逐点等于全局响应 —— 面板切回「全局」槽画的就是这条。
        let plain = Chain::build(&cfg(r#"{ "preamp_db": -2.0 }"#), FS as f64).unwrap();
        for f in [20.0, 500.0, 20000.0] {
            assert!(
                (plain.response_db_for_channel(f, FS as f64, 0) - plain.response_db(f, FS as f64))
                    .abs()
                    < 1e-12
            );
        }
    }

    #[test]
    fn reset_clears_the_delay_line_and_filter_states() {
        // 先喂一段别的音频把延迟线与滤波器状态「用热」，再 `reset()` —— 之后必须与
        // **全新实例**逐位一致（宿主 seek 后靠的就是这条：状态属于上一段音频）。
        let c = cfg(
            r#"{ "delay_ms": 1.0,
                 "filters": [ { "kind": "peaking", "freq_hz": 1000, "gain_db": 6, "q": 2 } ] }"#,
        );
        let chain = Chain::build(&c, FS as f64).unwrap();
        let probe = tone(FS, 1, 997.0, 480, 0.5);

        let mut used = ChainRuntime::new(&chain, 1).unwrap();
        let mut warm = tone(FS, 1, 1000.0, 96, 0.6);
        used.process_interleaved(&mut warm.samples).unwrap();
        used.reset();

        let mut fresh = ChainRuntime::new(&chain, 1).unwrap();
        let mut a = probe.samples.clone();
        let mut b = probe.samples.clone();
        used.process_interleaved(&mut a).unwrap();
        fresh.process_interleaved(&mut b).unwrap();
        assert_eq!(a, b, "reset 之后必须与全新实例逐位一致");
    }

    #[test]
    fn if_else_rejects_a_condition_channel_that_does_not_exist() {
        // 声道 5 在 2 声道流上**永远不命中** = 一个静默失效的配置 ⇒ 必须在 `new` 里挡住。
        let chain = Chain::build(
            &cfg(&format!(
                r#"{{ "if_else": [ {{ "channel": 5, "then": [ {PEAK_9DB} ] }} ] }}"#
            )),
            FS as f64,
        )
        .unwrap();
        let e = ChainRuntime::new(&chain, 2).unwrap_err();
        assert!(e.contains("越界"), "{e}");
    }
}
