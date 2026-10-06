// ── 链配置的 JSON 形态与校验 ── 见 docs/ai-spec.md §4.10
//
// 配置是引擎**唯一的对外输入面**（面板按钮、AI 下参数、命令行 `check` 走的都是它），
// 所以「什么算合法」必须在这里一次说清。核心口径只有一条：
// **写了用不上的字段就报错，绝不静默忽略** —— 「给 low_pass 配了 gain_db」如果被忽略，
// 用户会以为那个增益生效了，等测出来没变化再去查 DSP，方向就全错了。
//
// 校验分两层，分工要守住：
//   · 这里管**策略**（增益别超 ±30dB、用不上的字段要删、频率落在音频带内）；
//   · `biquad::design` 只管**数学上会炸**的入参。
//   ⇒ 换个调用方（比如将来 AI 直接下参数）也绕不过这两道。

use serde::Deserialize;

use crate::biquad::Kind;

/// 单段增益上限。超过这个数基本是**单位写错**（把 dB 当线性倍数填了），
/// 而且 ±30dB 已远超任何合理的校正量。
pub const MAX_GAIN_DB: f64 = 30.0;

/// 预增益的绝对值上限。preamp 只用来**为削波腾出余量**，不是增益段。
pub const MAX_PREAMP_DB: f64 = 60.0;

/// 全局延迟的上限（毫秒）。EAPO 对 Delay 没有硬上限，但 5s 已远超「对齐扬声器 / 声场」
/// 的合理量级，而更大的值意味着一条 5s×fs 的延迟线（48k 下 ~1MB/声道）—— 上限在这里挡住。
pub const MAX_DELAY_MS: f64 = 5000.0;

/// 没写 `q` 时的缺省值：1/√2 = 0.7071…（巴特沃斯，最平坦）。
pub const DEFAULT_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// 一段滤波器 —— **校验并归一化之后**的形态：每个字段都是定值，不再有 `Option`，
/// 下游（`chain`）拿到就能直接 `design`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Filter {
    pub kind: Kind,
    pub freq_hz: f64,
    /// 只有 `peaking` / `low_shelf` / `high_shelf` 有增益，其余类型恒为 0。
    pub gain_db: f64,
    /// 一阶类型用不上 Q，这里给的是 `DEFAULT_Q`（不参与设计）。
    pub q: f64,
}

/// 一条效果链。**顺序即处理顺序**。
///
/// 处理顺序（`ChainRuntime` 里）：① preamp → ② 各段双二阶 → ③ 延迟 → ④ 声道复制。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChainConfig {
    pub preamp_db: f64,
    pub filters: Vec<Filter>,
    /// 全局延迟（毫秒）。0 = 不延迟。**所有声道同量**（EAPO 的 `Delay` 也是这个语义）。
    /// 延迟只改相位、不改幅度 ⇒ 对频响判据（±0.1dB）无影响。
    pub delay_ms: f64,
    /// 声道复制：处理后把 `from` 声道的样本写进 `to` 声道（EAPO 的 channel copy 语义）。
    /// **声道下标在 config 层不校验**（这里不知道声道数），由 `ChainRuntime::new` 兜底。
    pub channel_copy: Vec<(usize, usize)>,
    /// 卷积用的脉冲响应（IR）WAV 路径（可选，EAPO 的 `Convolution` 语义）。
    ///
    /// config 层只做「非空」这一道**形态**校验；能不能读出来、采样率 / 声道数是否匹配，
    /// 要等 `Chain::build` 拿到渲染流的采样率才判得了（配置层不碰文件系统）。
    pub convolution: Option<String>,
    /// 条件段（EAPO 的 `If / Else` 语义）：**按声道**决定这一段滤波器组作用在哪条声道。
    /// 条件目前只有「声道」一种；设备等条件要等 S3 多设备时再加（见 backlog L5）。
    pub cond_blocks: Vec<CondBlock>,
}

/// 一段**条件滤波器组**：`channel` 命中走 `then`，不命中走 `else_filters`。
///
/// 「条件」刻意只做成**静态（按声道）**的：这样它在 `ChainRuntime::new` 里就能定下每条声道
/// 实际要跑哪一串滤波器，不必逐样本求值。EAPO 的 `If` 还支持设备 / `not` / `and` / `or`，
/// 那些等有真实用例（S3 按设备切配置）再加 —— 先只落「某条声道单独校正」这个最常用的形态。
#[derive(Debug, Clone, PartialEq)]
pub struct CondBlock {
    /// 命中的声道下标。**这里不校验它是否越界**（不知道声道数），由 `ChainRuntime::new` 兜底。
    pub channel: usize,
    /// 命中时应用的滤波器（顺序即处理顺序）。
    pub then: Vec<Filter>,
    /// 不命中时应用的滤波器。空 = 不做任何事（这就是「If 没有 Else 分支」）。
    pub else_filters: Vec<Filter>,
}

impl ChainConfig {
    /// 解析 + 逐条校验。`sample_rate` 用来判频率是否越过 Nyquist。
    pub fn from_json(json: &str, sample_rate: f64) -> Result<ChainConfig, String> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(format!("采样率非法：{sample_rate}"));
        }
        let raw: RawConfig = serde_json::from_str(strip_bom(json))
            .map_err(|e| format!("配置 JSON 解析失败：{e}"))?;

        if !raw.preamp_db.is_finite() || raw.preamp_db.abs() > MAX_PREAMP_DB {
            return Err(format!(
                "preamp_db 必须是 ±{MAX_PREAMP_DB} dB 内的有限值，收到 {}",
                raw.preamp_db
            ));
        }

        let nyquist = sample_rate / 2.0;
        let mut filters = Vec::with_capacity(raw.filters.len());
        for (i, rf) in raw.filters.into_iter().enumerate() {
            filters.push(convert(i, rf, nyquist)?);
        }
        // GraphicEQ（参照 EqualizerAPO 的 `GraphicEQ: f1 g1; f2 g2; ...`）：解析期**展开**
        // 成 peaking 段追加到表尾。为什么不新增一种 `Kind` —— 它不是一个滤波器，而是一组
        // 等间距/自定义间距的 peaking；展开成既有段后，链、渲染、测量、±0.1dB 判据全部
        // 天然复用，`ChainConfig` 的结构也不必动（配置层加字段不影响下游与老配置）。
        filters.extend(expand_graphic_eq(&raw.graphic_eq, nyquist)?);

        if !raw.delay_ms.is_finite() || raw.delay_ms < 0.0 || raw.delay_ms > MAX_DELAY_MS {
            return Err(format!(
                "delay_ms 必须是 0..={MAX_DELAY_MS} 内的有限值，收到 {}",
                raw.delay_ms
            ));
        }
        // 声道下标在 config 层不校验（这里不知道声道数）—— 交给 `ChainRuntime::new`。
        let channel_copy = raw.channel_copy.into_iter().map(|c| (c.from, c.to)).collect();

        // 卷积的 IR 路径：只挡「空路径」这一种**明显写错**。写了一个空串通常意味着用户以为
        // 「留空 = 关掉卷积」，但那样更像拼错字段 —— 报错比静默当成「没有卷积」更安全
        //（同「写了用不上的字段一律报错」这条口径）。两端空白顺手去掉（Windows 路径常被引号包住）。
        let convolution = match raw.convolution {
            None => None,
            Some(p) => {
                let t = p.trim();
                if t.is_empty() {
                    return Err("convolution 必须是一个非空的 IR WAV 路径".to_string());
                }
                Some(t.to_string())
            }
        };

        // 条件段（If / Else）：每段的两串滤波器都走与普通段**同一套**转换 / 校验
        //（`convert`），只是报错前缀换成「if_else 第 N 个块（声道 X）的 then / else」——
        // 长配置里得能一眼定位到是哪一段的哪一边。
        let mut cond_blocks = Vec::with_capacity(raw.if_else.len());
        for (bi, rb) in raw.if_else.into_iter().enumerate() {
            let RawCondBlock {
                channel,
                then: raw_then,
                else_: raw_else,
            } = rb;
            let block_no = bi + 1;
            let mut then = Vec::with_capacity(raw_then.len());
            for (i, rf) in raw_then.into_iter().enumerate() {
                then.push(convert(i, rf, nyquist).map_err(|e| {
                    format!("if_else 第 {block_no} 个块（声道 {channel}）的 then：{e}")
                })?);
            }
            let mut else_filters = Vec::with_capacity(raw_else.len());
            for (i, rf) in raw_else.into_iter().enumerate() {
                else_filters.push(convert(i, rf, nyquist).map_err(|e| {
                    format!("if_else 第 {block_no} 个块（声道 {channel}）的 else：{e}")
                })?);
            }
            // 两边都空 = 这一段什么也不做。**不许静默存在**：它看起来像一段配置，实际是个摆设
            //（同「写了用不上的字段一律报错」）。
            if then.is_empty() && else_filters.is_empty() {
                return Err(format!(
                    "if_else 第 {block_no} 个块（声道 {channel}）：then 与 else 都是空的 —— \
                     这一段什么也不做，请删掉它"
                ));
            }
            cond_blocks.push(CondBlock {
                channel,
                then,
                else_filters,
            });
        }

        Ok(ChainConfig {
            preamp_db: raw.preamp_db,
            filters,
            delay_ms: raw.delay_ms,
            channel_copy,
            convolution,
            cond_blocks,
        })
    }

    pub fn len(&self) -> usize {
        self.filters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }
}

/// 去掉开头的 UTF-8 BOM。
///
/// Windows 上记事本、PowerShell 的 `Set-Content -Encoding UTF8` 都会写 BOM，而
/// `serde_json` 不认它 ⇒ 报「expected value at line 1 column 1」。用户对着一个**看起来
/// 完全正常**的配置文件查半天，方向全错。这条不是洁癖，是实际会踩的坑（本仓的构建脚本
/// 就被 BOM 咬过一次）。
fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

/// 把一条「原始」滤波器变成「归一化」滤波器，顺路把该报的错报全。
///
/// 报错一律带 **`第 N 段`**（从 1 数起）—— 配置长了以后，「哪个字段非法」不够用，
/// 得能一眼定位到是第几段。
fn convert(i: usize, rf: RawFilter, nyquist: f64) -> Result<Filter, String> {
    let n = i + 1;
    let kind = Kind::parse(&rf.kind).map_err(|e| format!("第 {n} 段：{e}"))?;

    if !rf.freq_hz.is_finite() || rf.freq_hz <= 0.0 || rf.freq_hz >= nyquist {
        return Err(format!(
            "第 {n} 段（{}）：freq_hz 必须落在 (0, {nyquist}) 之间，收到 {}",
            kind.name(),
            rf.freq_hz
        ));
    }

    let gain_db = match (kind.uses_gain(), rf.gain_db) {
        (true, None) => {
            return Err(format!(
                "第 {n} 段（{}）：这种类型必须给 gain_db",
                kind.name()
            ))
        }
        (false, Some(g)) => {
            return Err(format!(
                "第 {n} 段（{}）：这种类型没有增益，请删掉 gain_db（收到 {g}）",
                kind.name()
            ))
        }
        (true, Some(g)) => {
            if !g.is_finite() || g.abs() > MAX_GAIN_DB {
                return Err(format!(
                    "第 {n} 段（{}）：gain_db 必须是 ±{MAX_GAIN_DB} dB 内的有限值，收到 {g}",
                    kind.name()
                ));
            }
            g
        }
        (false, None) => 0.0,
    };

    let q = match rf.q {
        // 缺省即巴特沃斯；一阶用不上 Q，给同一个定值免得下游到处判 Option
        None => DEFAULT_Q,
        Some(q) => {
            if kind.order() == 1 {
                return Err(format!(
                    "第 {n} 段（{}）：一阶滤波器没有 Q，请删掉 q（收到 {q}）",
                    kind.name()
                ));
            }
            if !q.is_finite() || q <= 0.0 {
                return Err(format!(
                    "第 {n} 段（{}）：Q 必须是正的有限值，收到 {q}",
                    kind.name()
                ));
            }
            q
        }
    };

    Ok(Filter {
        kind,
        freq_hz: rf.freq_hz,
        gain_db,
        q,
    })
}

// ── 只用于接 JSON 的原始形态 ──
//
// `Option` 是**必须的**：要能区分「没写 gain_db」和「写了 gain_db: 0」——
// 前者在 peaking 上是漏填，后者在 low_pass 上是写错，两种都得报错。

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    preamp_db: f64,
    #[serde(default)]
    filters: Vec<RawFilter>,
    /// GraphicEQ：一组「中心频率 + 增益」，解析期展开成 peaking 段（见 `expand_graphic_eq`）。
    #[serde(default)]
    graphic_eq: Vec<RawGraphicBand>,
    /// 全局延迟（毫秒）。
    #[serde(default)]
    delay_ms: f64,
    /// 声道复制（`[{ from, to }]`）。
    #[serde(default)]
    channel_copy: Vec<RawCopy>,
    /// 卷积用的脉冲响应 WAV 路径。
    #[serde(default)]
    convolution: Option<String>,
    /// 条件段（`If / Else`）：`[{ channel, then: [...], else: [...] }]`。
    #[serde(default)]
    if_else: Vec<RawCondBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCondBlock {
    /// 命中的声道下标。
    channel: usize,
    /// 命中时应用的滤波器（必填；可以为空数组，但 then 与 else 不能同时为空）。
    then: Vec<RawFilter>,
    /// 不命中时应用的滤波器（`else` 是 Rust 关键字，字段名靠 `rename` 接回来）。
    #[serde(default, rename = "else")]
    else_: Vec<RawFilter>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCopy {
    from: usize,
    to: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGraphicBand {
    freq_hz: f64,
    gain_db: f64,
}

/// 把 GraphicEQ 展开成 peaking 段（RBJ 的「带宽倍频程 ↔ Q」关系）。
///
/// 每段的 Q 由**相邻频点的对数间距**折算：带宽（倍频程）取左右邻居间距的均值（端点用
/// 单侧，只有一个频点时按 1 倍频程兜底），再按 `Q = 1 / (2·sinh(ln2/2 · BW))` 换算 ——
/// 这正是 Audio EQ Cookbook 里 peaking 的 BW↔Q 定义，所以「等间隔 1/3 倍频程」和
/// 「自定义疏密」都能得到合理的带宽容差。
///
/// 校验与 `filters` 同一套口径（越界 / 非有限都报、且带段号），另加一条：**频点必须
/// 严格升序** —— Q 靠邻居推导，乱序会把带宽算成一个负数式的东西。
fn expand_graphic_eq(bands: &[RawGraphicBand], nyquist: f64) -> Result<Vec<Filter>, String> {
    if bands.is_empty() {
        return Ok(Vec::new());
    }
    for (i, b) in bands.iter().enumerate() {
        let n = i + 1;
        if !b.freq_hz.is_finite() || b.freq_hz <= 0.0 || b.freq_hz >= nyquist {
            return Err(format!(
                "graphic_eq 第 {n} 段：freq_hz 必须落在 (0, {nyquist}) 之间，收到 {}",
                b.freq_hz
            ));
        }
        if !b.gain_db.is_finite() || b.gain_db.abs() > MAX_GAIN_DB {
            return Err(format!(
                "graphic_eq 第 {n} 段：gain_db 必须是 ±{MAX_GAIN_DB} dB 内的有限值，收到 {}",
                b.gain_db
            ));
        }
        if i > 0 && b.freq_hz <= bands[i - 1].freq_hz {
            return Err(format!(
                "graphic_eq 第 {n} 段：频点必须严格升序（前一段 {}，本段 {}）",
                bands[i - 1].freq_hz, b.freq_hz
            ));
        }
    }
    let ln2 = std::f64::consts::LN_2;
    let mut out = Vec::with_capacity(bands.len());
    for (i, b) in bands.iter().enumerate() {
        let lo = if i > 0 { (b.freq_hz / bands[i - 1].freq_hz).log2() } else { 0.0 };
        let hi = if i + 1 < bands.len() { (bands[i + 1].freq_hz / b.freq_hz).log2() } else { 0.0 };
        let bw = match (i > 0, i + 1 < bands.len()) {
            (true, true) => (lo + hi) / 2.0,
            (false, true) => hi,
            (true, false) => lo,
            (false, false) => 1.0,
        }
        .max(0.05); // 极密频点 / 单点的下限，免得 sinh(0)=0 把 Q 变成无穷
        let q = 1.0 / (2.0 * (ln2 / 2.0 * bw).sinh());
        out.push(Filter {
            kind: Kind::Peaking,
            freq_hz: b.freq_hz,
            gain_db: b.gain_db,
            q,
        });
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFilter {
    kind: String,
    freq_hz: f64,
    gain_db: Option<f64>,
    q: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 48000.0;

    fn parse(json: &str) -> Result<ChainConfig, String> {
        ChainConfig::from_json(json, FS)
    }

    #[test]
    fn parses_a_typical_chain_and_fills_defaults() {
        let cfg = parse(
            r#"{
                "preamp_db": -6.0,
                "filters": [
                    { "kind": "peaking", "freq_hz": 105, "gain_db": 4.5, "q": 1.4 },
                    { "kind": "high_shelf", "freq_hz": 8000, "gain_db": -3 },
                    { "kind": "low_pass_1", "freq_hz": 18000 }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(cfg.preamp_db, -6.0);
        assert_eq!(cfg.len(), 3);

        assert_eq!(cfg.filters[0].kind, Kind::Peaking);
        assert_eq!(cfg.filters[0].freq_hz, 105.0);
        assert_eq!(cfg.filters[0].gain_db, 4.5);
        assert_eq!(cfg.filters[0].q, 1.4);

        // 没写 q ⇒ 巴特沃斯
        assert_eq!(cfg.filters[1].q, DEFAULT_Q);
        assert_eq!(cfg.filters[1].gain_db, -3.0);

        // 无增益的类型：gain 归一成 0，且**不需要**在 JSON 里写出来
        assert_eq!(cfg.filters[2].kind, Kind::LowPass1);
        assert_eq!(cfg.filters[2].gain_db, 0.0);
        assert_eq!(cfg.filters[2].q, DEFAULT_Q);
    }

    #[test]
    fn missing_optional_sections_mean_a_plain_passthrough() {
        // 空对象是合法的：没有任何滤波器 = 直通（preamp 0dB）
        let cfg = parse("{}").unwrap();
        assert!(cfg.is_empty());
        assert_eq!(cfg.preamp_db, 0.0);
        assert_eq!(cfg, ChainConfig::default());
    }

    #[test]
    fn rejects_unknown_fields() {
        // 顶层拼错
        let e = parse(r#"{ "pre_amp_db": -6 }"#).unwrap_err();
        assert!(e.contains("pre_amp_db"), "错误里要点出拼错的字段名：{e}");

        // 段内拼错（`frequency` 而不是 `freq_hz`）—— 这类错最容易被静默吃掉
        let e = parse(r#"{ "filters": [{ "kind": "peaking", "frequency": 100, "gain_db": 3 }] }"#)
            .unwrap_err();
        assert!(e.contains("frequency") || e.contains("freq_hz"), "收到：{e}");
    }

    #[test]
    fn rejects_a_kind_that_needs_gain_without_it() {
        let e = parse(r#"{ "filters": [{ "kind": "peaking", "freq_hz": 100, "q": 1 }] }"#)
            .unwrap_err();
        assert!(e.contains("第 1 段"), "要指出是第几段：{e}");
        assert!(e.contains("gain_db"), "要指出缺的是哪个字段：{e}");
    }

    #[test]
    fn rejects_gain_on_a_kind_that_has_none() {
        // 这是「静默忽略」最容易害人的一处：用户以为那个增益生效了
        let e = parse(r#"{ "filters": [{ "kind": "high_pass", "freq_hz": 80, "gain_db": 6 }] }"#)
            .unwrap_err();
        assert!(e.contains("第 1 段"), "{e}");
        assert!(e.contains("删掉 gain_db"), "要明确说该删掉：{e}");
    }

    #[test]
    fn gain_limit_is_inclusive_and_only_for_gain_kinds() {
        // 边界 ±30 必须放行（限制是「不得超过」）
        for g in ["30", "-30", "0"] {
            parse(&format!(
                r#"{{ "filters": [{{ "kind": "low_shelf", "freq_hz": 200, "gain_db": {g} }}] }}"#
            ))
            .unwrap_or_else(|e| panic!("gain_db = {g} 应当合法：{e}"));
        }
        // 越界一点点就拒
        let e = parse(r#"{ "filters": [{ "kind": "low_shelf", "freq_hz": 200, "gain_db": 30.001 }] }"#)
            .unwrap_err();
        assert!(e.contains("±30"), "{e}");
        // 不带增益的类型不受这条影响（它连 gain_db 都不许写）
        parse(r#"{ "filters": [{ "kind": "notch", "freq_hz": 50 }] }"#).unwrap();
    }

    #[test]
    fn rejects_frequencies_outside_the_audio_band() {
        for bad in ["0", "-100", "24000", "99999"] {
            let e = parse(&format!(
                r#"{{ "filters": [{{ "kind": "notch", "freq_hz": {bad} }}] }}"#
            ))
            .unwrap_err();
            assert!(e.contains("freq_hz"), "freq_hz = {bad} 应当被拒：{e}");
        }
        // 紧贴 Nyquist 的下方仍然合法（22050 在 44.1kHz 下正好是 Nyquist，必须拒）
        parse(r#"{ "filters": [{ "kind": "notch", "freq_hz": 23999 }] }"#).unwrap();
        assert!(ChainConfig::from_json(
            r#"{ "filters": [{ "kind": "notch", "freq_hz": 22050 }] }"#,
            44100.0
        )
        .is_err());
    }

    #[test]
    fn rejects_q_where_it_is_useless_or_illegal() {
        // 一阶没有 Q
        let e = parse(r#"{ "filters": [{ "kind": "high_pass_1", "freq_hz": 120, "q": 0.7 }] }"#)
            .unwrap_err();
        assert!(e.contains("没有 Q"), "{e}");
        // 二阶的 Q 必须为正
        for bad in ["0", "-1"] {
            let e = parse(&format!(
                r#"{{ "filters": [{{ "kind": "band_pass", "freq_hz": 1000, "q": {bad} }}] }}"#
            ))
            .unwrap_err();
            assert!(e.contains("Q 必须"), "q = {bad} 应当被拒：{e}");
        }
    }

    #[test]
    fn rejects_an_unknown_kind_and_lists_the_valid_ones() {
        let e = parse(r#"{ "filters": [{ "kind": "peak", "freq_hz": 100, "gain_db": 3 }] }"#)
            .unwrap_err();
        assert!(e.contains("peak"), "{e}");
        // 可用取值要列出来，否则用户只能去翻文档
        for k in ["peaking", "low_shelf", "high_shelf", "all_pass"] {
            assert!(e.contains(k), "错误里应列出 {k}：{e}");
        }
        // 全部类型都真的能解析（两边名单只此一份）
        for k in Kind::ALL {
            let json = format!(
                r#"{{ "filters": [{{ "kind": "{}", "freq_hz": 1000, "gain_db": 0 }}] }}"#,
                k.name()
            );
            let r = parse(&json);
            if k.uses_gain() {
                r.unwrap_or_else(|e| panic!("{} 应当合法：{e}", k.name()));
            } else {
                // 不带增益的类型带了 gain_db ⇒ 必须报错（这条已单独测过，这里只确认「不是因为 unknown kind」）
                assert!(r.unwrap_err().contains("删掉 gain_db"), "{}", k.name());
            }
        }
    }

    #[test]
    fn the_error_names_which_filter_is_broken() {
        // 第二段才坏：报错必须是「第 2 段」，否则长配置里没法定位
        let e = parse(
            r#"{
                "filters": [
                    { "kind": "peaking", "freq_hz": 100, "gain_db": 3 },
                    { "kind": "peaking", "freq_hz": 200, "gain_db": 999 }
                ]
            }"#,
        )
        .unwrap_err();
        assert!(e.contains("第 2 段"), "{e}");
    }

    #[test]
    fn rejects_broken_json_and_impossible_sample_rate() {
        assert!(parse("{ not json").unwrap_err().contains("解析失败"));
        assert!(ChainConfig::from_json("{}", 0.0).is_err());
        assert!(ChainConfig::from_json("{}", f64::NAN).is_err());
    }

    #[test]
    fn a_utf8_bom_is_tolerated() {
        // 记事本 / PowerShell `Set-Content -Encoding UTF8` 都会加 BOM。
        // 不加这一步的话，一个**看起来完全正常**的配置文件会报
        // 「expected value at line 1 column 1」，把用户引向完全错误的方向。
        let with_bom = "\u{feff}{ \"preamp_db\": -3 }";
        assert_eq!(parse(with_bom).unwrap().preamp_db, -3.0);
        // 带 BOM 且内容也不合法的，仍然要报错（BOM 只是被容忍，不是被忽略一切）
        assert!(parse("\u{feff}{ \"pre_amp_db\": -3 }").is_err());
    }

    #[test]
    fn rejects_a_preamp_that_is_not_finite_or_is_absurd() {
        assert!(parse(r#"{ "preamp_db": 60.001 }"#).is_err());
        assert!(parse(r#"{ "preamp_db": -60.0 }"#).is_ok());
        // 合法范围内的负 preamp 是主要用法（给滤波器腾余量）
        let cfg = parse(r#"{ "preamp_db": -4.5 }"#).unwrap();
        assert_eq!(cfg.preamp_db, -4.5);
    }

    // ── GraphicEQ（2026-10-03，参照 EqualizerAPO）────────────────────

    #[test]
    fn graphic_eq_expands_into_peaking_bands_with_derived_q() {
        // **非等距**频点：Q 必须跟着间距变（不是拍一个常数）。
        let cfg = parse(
            r#"{
                "graphic_eq": [
                    { "freq_hz": 100,  "gain_db": 6 },
                    { "freq_hz": 200,  "gain_db": -3 },
                    { "freq_hz": 1600, "gain_db": 0 }
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(cfg.len(), 3, "三段 graphic_eq 应展开成三段 peaking");
        for f in &cfg.filters {
            assert_eq!(f.kind, Kind::Peaking);
            assert!(f.q > 0.0 && f.q.is_finite(), "Q 必须为正有限值，收到 {}", f.q);
        }
        assert_eq!(cfg.filters[0].gain_db, 6.0);
        assert_eq!(cfg.filters[1].gain_db, -3.0);
        assert!((cfg.filters[0].q - cfg.filters[1].q).abs() > 1e-6, "Q 应随间距变化");

        // **等距 1 倍频程**时 Q 必须落在 RBJ 的闭式解上：Q = 1 / (2·sinh(ln2/2 · 1))。
        let uni = parse(
            r#"{ "graphic_eq": [ { "freq_hz": 1000, "gain_db": 0 }, { "freq_hz": 2000, "gain_db": 0 }, { "freq_hz": 4000, "gain_db": 0 } ] }"#,
        )
        .unwrap();
        let expect = 1.0 / (2.0 * (std::f64::consts::LN_2 / 2.0).sinh());
        assert!(
            (uni.filters[1].q - expect).abs() < 1e-12,
            "1 倍频程的 Q 应为 {expect}，收到 {}",
            uni.filters[1].q
        );
    }

    #[test]
    fn graphic_eq_all_zero_is_a_passthrough() {
        // 全 0 增益的 graphic_eq = 一条 0dB 的链（peaking 的 0dB 就是直通）
        let cfg = parse(
            r#"{ "graphic_eq": [ { "freq_hz": 100, "gain_db": 0 }, { "freq_hz": 1000, "gain_db": 0 } ] }"#,
        )
        .unwrap();
        let chain = crate::chain::Chain::build(&cfg, FS).unwrap();
        for f in [20.0, 100.0, 1000.0, 5000.0, 15000.0] {
            assert!(chain.response_db(f, FS).abs() < 1e-9, "@{f}Hz 应为 0dB");
        }
    }

    #[test]
    fn graphic_eq_can_mix_with_ordinary_filters() {
        let cfg = parse(
            r#"{
                "preamp_db": -2,
                "filters": [ { "kind": "high_pass_1", "freq_hz": 30 } ],
                "graphic_eq": [ { "freq_hz": 1000, "gain_db": 4 } ]
            }"#,
        )
        .unwrap();
        assert_eq!(cfg.len(), 2, "普通段在前、graphic_eq 展开段在后");
        assert_eq!(cfg.filters[0].kind, Kind::HighPass1);
        assert_eq!(cfg.filters[1].kind, Kind::Peaking);
    }

    #[test]
    fn graphic_eq_rejects_unsorted_out_of_band_or_absurd_bands() {
        // 必须严格升序
        let e = parse(
            r#"{ "graphic_eq": [ { "freq_hz": 1000, "gain_db": 3 }, { "freq_hz": 100, "gain_db": 3 } ] }"#,
        )
        .unwrap_err();
        assert!(e.contains("升序"), "{e}");
        // 频点越界（含 == Nyquist）
        let e = parse(r#"{ "graphic_eq": [ { "freq_hz": 24000, "gain_db": 3 } ] }"#).unwrap_err();
        assert!(e.contains("freq_hz"), "{e}");
        // 增益越界
        let e = parse(r#"{ "graphic_eq": [ { "freq_hz": 1000, "gain_db": 99 } ] }"#).unwrap_err();
        assert!(e.contains("gain_db"), "{e}");
        // 段内拼错字段（被 deny_unknown_fields 挡下，不静默忽略）
        assert!(parse(r#"{ "graphic_eq": [ { "freq": 1000, "gain_db": 3 } ] }"#).is_err());
    }

    // ── 延迟与声道复制（2026-10-03）──────────────────────────────────

    #[test]
    fn parses_delay_and_channel_copy() {
        let cfg = parse(
            r#"{ "delay_ms": 1.5, "channel_copy": [ { "from": 0, "to": 1 }, { "from": 1, "to": 0 } ] }"#,
        )
        .unwrap();
        assert_eq!(cfg.delay_ms, 1.5);
        assert_eq!(cfg.channel_copy, vec![(0usize, 1usize), (1, 0)]);
        // 不写就是 0 / 空（老配置完全不受影响）
        let plain = parse("{}").unwrap();
        assert_eq!(plain.delay_ms, 0.0);
        assert!(plain.channel_copy.is_empty());
    }

    #[test]
    fn rejects_an_out_of_range_delay() {
        for bad in ["-1", "5000.001"] {
            let e = parse(&format!(r#"{{ "delay_ms": {bad} }}"#)).unwrap_err();
            assert!(e.contains("delay_ms"), "delay_ms = {bad} 应被拒：{e}");
        }
        parse(r#"{ "delay_ms": 5000 }"#).unwrap();
    }

    #[test]
    fn rejects_a_misspelled_channel_copy_field() {
        // deny_unknown_fields：`target` 拼错必须报错，不能静默当成「没有复制」
        assert!(parse(r#"{ "channel_copy": [ { "from": 0, "target": 1 } ] }"#).is_err());
    }

    // ── 卷积（IR 路径，2026-10-03）──────────────────────────────────

    #[test]
    fn parses_and_trims_the_convolution_path() {
        // 两端空白必须去掉（Windows 上被引号包住的路径很常见）
        let cfg = parse(r#"{ "convolution": "  C:\\ir\\room.wav  " }"#).unwrap();
        assert_eq!(cfg.convolution.as_deref(), Some("C:\\ir\\room.wav"));
        // 不写就是 None —— 老配置完全不受影响
        assert_eq!(parse("{}").unwrap().convolution, None);
    }

    #[test]
    fn rejects_an_empty_convolution_path() {
        // 空串 / 全空白都必须报错：静默当成「没有卷积」会让用户以为那条 IR 生效了
        for bad in ["", "   "] {
            let e = parse(&format!(r#"{{ "convolution": "{bad}" }}"#)).unwrap_err();
            assert!(e.contains("convolution"), "空路径应当被拒：{e}");
        }
        // deny_unknown_fields：拼错字段名不能被静默忽略
        assert!(parse(r#"{ "convolve": "a.wav" }"#).is_err());
    }

    // ── 条件段 If / Else（2026-10-03）────────────────────────────────

    #[test]
    fn parses_if_else_blocks() {
        let cfg = parse(
            r#"{
                "if_else": [
                    { "channel": 0, "then": [ { "kind": "peaking", "freq_hz": 1000, "gain_db": 3 } ] },
                    { "channel": 1, "then": [],
                      "else": [ { "kind": "high_shelf", "freq_hz": 8000, "gain_db": -3 } ] }
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(cfg.cond_blocks.len(), 2);
        assert_eq!(cfg.cond_blocks[0].channel, 0);
        assert_eq!(cfg.cond_blocks[0].then.len(), 1);
        assert!(cfg.cond_blocks[0].else_filters.is_empty());
        assert!(cfg.cond_blocks[1].then.is_empty());
        assert_eq!(cfg.cond_blocks[1].else_filters.len(), 1);
        assert_eq!(cfg.cond_blocks[1].else_filters[0].gain_db, -3.0);
        // 不写就是空 —— 老配置完全不受影响
        assert!(parse("{}").unwrap().cond_blocks.is_empty());
    }

    #[test]
    fn rejects_if_else_blocks_that_do_nothing_or_are_misspelled() {
        // then 与 else 都空 ⇒ 一段摆设，报错（不许静默存在）
        let e = parse(r#"{ "if_else": [ { "channel": 0, "then": [] } ] }"#).unwrap_err();
        assert!(e.contains("if_else"), "{e}");
        // 块里的滤波器走**同一套**校验，且报错要点出是哪一块的哪一边
        let e = parse(
            r#"{ "if_else": [ { "channel": 0, "then": [ { "kind": "peaking", "freq_hz": 1000, "gain_db": 999 } ] } ] }"#,
        )
        .unwrap_err();
        assert!(e.contains("if_else") && e.contains("then"), "{e}");
        // 段内拼错字段（deny_unknown_fields）与缺 then 都必须报错
        assert!(parse(r#"{ "if_else": [ { "channel": 0, "than": [] } ] }"#).is_err());
        assert!(parse(r#"{ "if_else": [ { "channel": 0, "else": [] } ] }"#).is_err());
    }
}
