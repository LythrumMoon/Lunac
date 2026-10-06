// ── player.rs — 本地文件播放（音乐插件「本地音乐」页的出声口，2026-10-01）────
//
// **为什么这一层在宿主**：WebView 里的插件放不出声音 —— 解码、设备枚举、出声全是系统
// 能力，与「联网必须走宿主」是同一条纪律（ai-spec §4.6 开头那段）。所以分工与
// `music.rs` 管 librespot 完全一致：**宿主管一条真实的音频流，插件只画 UI + 轮询**。
//
// **为什么用 rodio**（内含 cpal 输出 + symphonia 解码）：播放要的是一台状态机 ——
// 位置口径 / 暂停语义 / seek 饱和 / 「这一首放完了」的判据，这几件事正是最容易写错的
// 地方，rodio 的 `Player` 已经把它们做对了；自己拼 cpal + symphonia 等于重写一遍。
// 代价是两条不轻的依赖，见 `Cargo.toml` 里那一段注释。
//
// ⚠️ **rodio 的 `stop()` 是一次性的**：`Controls::stopped` 在源码里只置位、**永不
// 复位**（`player.rs`），而每条 source 都被 `.stoppable()` 包着 ⇒ 调过一次 `stop()`
// 之后，**这台 Player 再 append 什么都会被立刻掐掉**。所以这里的形态是
// 「**设备跨曲目复用、Player 每首一条**」（见 `play()`）—— 写成「一台 Player 反复
// append」的话，第一次停下之后所有后续播放都没声音、而且**一句报错都没有**。
//
// **DSP 挂点**：链套在 `append()` 的那条 source 上 —— `tuning-engine` 是 lib + bin，
// 它的 `chain::ChainRuntime` 在这里被复用，见 ai-spec §4.10 与下面的 `TuningSource`。
// **刻意不另写一份滤波器**。
// **2026-10-03**：从 `Chain::process_sample`（逐样本）换成 `ChainRuntime::process_frame`
//（按帧）—— 延迟 / 声道复制 / 卷积 / 条件段（If-Else）都**要自己的缓冲**，逐样本那条路
// 装不下它们（换之前它们在应用里**根本不生效**）。`TuningSource` 因此要**攒帧**。
// 2026-10-01 用户第 5 条把这一刀落实了（「先做最小可用」）：链由**预设**驱动，
// 面板只有「总开关 + 一档预设」，改档**不打断正在放的这一首**（走版本号热换链）。
//
// **生命周期 = 音乐插件窗**（与 librespot 同一条纪律）：窗口一关由
// `music::on_plugin_window_destroyed()` 调 `stop_if_running()` —— 界面都没了、声音
// 还在放，用户只能去任务栏里找一个不认识的进程。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rodio::{ChannelCount, Decoder, Sample, SampleRate, Source};
use serde::{Deserialize, Serialize};
use tuning_engine::chain::{Chain, ChainRuntime};
use tuning_engine::config::ChainConfig;
use tuning_engine::convolution::Convolver;

/// 默认音量。**刻意不是 1.0**：满量程起播在「戴着耳机、上一首很轻」的场景下很吓人。
const DEFAULT_VOLUME: f32 = 0.8;
const DEFAULT_VOLUME_BITS: u32 = DEFAULT_VOLUME.to_bits();

/// 一条正在播的流。
///
/// `sink`（设备）与 `player`（这一首的队列）**不是一回事**：设备要跨曲目留下来，
/// 否则每换一首都要重开一次 WASAPI 流（几十毫秒，且常常带一声爆音）；而 Player
/// 必须每首一条（理由见文件头那条 ⚠️）。
struct Session {
    /// **必须活着**：它一 drop，输出流就没了（rodio 文档原话：playback ends if the
    /// DeviceSink is dropped），表现是「点播放没有任何反应」。
    sink: rodio::MixerDeviceSink,
    player: rodio::Player,
    path: PathBuf,
    /// 解码器报的总时长（毫秒）。**0 = 报不出来**（没有 Xing 头的 mp3 就是这样）——
    /// 界面据此把这栏留空，不去画一条假的进度。
    duration_ms: u64,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
/// 音量（0.0–1.0）存**位模式**（原子量里没有 f32）。跨曲目、跨次播放都保留 ——
/// 每首都回到默认音量是最烦人的一类行为。
static VOLUME: AtomicU32 = AtomicU32::new(DEFAULT_VOLUME_BITS);

#[derive(Debug, Serialize, Default, Clone)]
pub struct PlayerDto {
    /// 有内容在播。**`false` 的三种含义要靠 `path` 区分**：没起过 / 已停（path 空）
    /// 与**这一首放完了**（path 有值）—— 后者是界面「自动下一首」的触发条件。
    pub active: bool,
    pub playing: bool,
    pub path: String,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: f32,
    /// **这是一个要出画面的视频**（2026-10-01 用户第 8 条）。
    ///
    /// 名字表只有一份：直接用 `media_lib::is_video_ext`（`VIDEO_EXTS`），
    /// **不在这里再抄一遍** —— 「加了一个格式、只有一侧认」是最典型的静默失效。
    /// 前端据此决定要不要开那块画面层（画面由插件窗的 `<video>` 出，见 ai-spec §4.6）。
    pub video: bool,
}

fn volume() -> f32 {
    f32::from_bits(VOLUME.load(Ordering::Relaxed))
}

/// 这个路径要不要出画面（= 扩展名在 `media_lib::VIDEO_EXTS` 里）。
///
/// **判据只有扩展名**：内容是判据那条纪律在这里不适用 —— 这条路径每 `LOCAL_POLL_MS`
/// （500ms）被问一次，「每次去解一遍容器头」的代价远大于它换来的那点确定性；
/// 而且**这条路的两头都不依赖它**：真解不出来的视频，画面层自己会报错、
/// 宿主的 rodio 也会如实报「解不开」（见 `open_decoder`）。
fn is_video_path(p: &Path) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    crate::media_lib::is_video_ext(&ext)
}

/// 打开解码器：**解码器 + 总时长（毫秒，0 = 报不出来）**。
///
/// 必须走 `Decoder::try_from(File)` 而不是 `Decoder::new(BufReader::new(file))`：
/// 前者的 `TryFrom<File>` 会顺手把 `byte_len` 设上（rodio 文档：「Automatically sets
/// byte_len from metadata」），而 symphonia **靠这个才算得出总时长、才支持 seek**。
fn open_decoder(path: &Path) -> Result<(Decoder<std::io::BufReader<std::fs::File>>, u64), String> {
    if !path.is_file() {
        return Err(format!("找不到这个文件：{}", path.display()));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("打不开 {}：{e}", path.display()))?;
    let dec = Decoder::try_from(file).map_err(|e| format!("解不开 {}：{e}", path.display()))?;
    let ms = dec.total_duration().map(|d| d.as_millis() as u64).unwrap_or(0);
    Ok((dec, ms))
}

/// 当前状态。**`active` 的判据是「队列里还有没有没放完的 source」**：
/// rodio 的 `Done` 包装在一条 source 放完时把它从计数里减掉（见其 `player.rs`），
/// 所以 `empty()` 就是「放完了」——比拿位置和时长对表稳得多（没有 Xing 头的 mp3
/// 时长是 0，对不了表）。
fn dto_of(session: Option<&Session>) -> PlayerDto {
    let volume = volume();
    let Some(s) = session else {
        return PlayerDto { volume, ..Default::default() };
    };
    let active = !s.player.empty();
    PlayerDto {
        active,
        playing: active && !s.player.is_paused(),
        path: s.path.to_string_lossy().into_owned(),
        position_ms: s.player.get_pos().as_millis() as u64,
        duration_ms: s.duration_ms,
        volume,
        video: is_video_path(&s.path),
    }
}

// ── 调音（`tuning-engine` 的挂点，2026-10-01 用户第 5 条）──────────────
//
// **口径 = 「先做最小可用」**（用户 2026-10-01 选定）：把引擎当**库依赖**接进来，
// 在播放链上套一层 `Chain`；面板只给「总开关 + 一档预设」，**不再有独立的调音插件**。
// 面板 → 宿主只有两个命令（`player_tuning_get` / `player_tuning_set`），下发的是
// **预设键**而不是滤波器数组：一条链的合法性由引擎那份 `ChainConfig` 校验兜底，
// 前端不可能拼出一条「界面上亮着、实际直通」的配置。
//
// **预设表是唯一名单**（前端照 `presets` 字段画按钮，不自己维护第二份）。
// 每条预设的 `preamp_db` = **该链最大正向增益的相反数** —— 这就是引擎文档里那条
// 「防削波靠减 preamp、不靠链里夹 limiter」的静态形态（见 ai-spec §4.10「链与削波」）。
// 代价写明白：低音增强整体会比直通**略轻**，换来的是任何素材都不削波。
const TUNING_PRESETS: &[(&str, &str)] = &[
    // 直通（空链）：开关开着但选了它 = 什么都不加，用来做 A/B 的参照
    ("flat", r#"{ "preamp_db": 0 }"#),
    // 低音增强：低频 +6dB，9k 以上让出 2dB；preamp −6 ⇒ 峰值 0dB，不削波
    (
        "bass",
        r#"{ "preamp_db": -6, "filters": [
            { "kind": "low_shelf", "freq_hz": 120, "gain_db": 6 },
            { "kind": "high_shelf", "freq_hz": 9000, "gain_db": -2 } ] }"#,
    ),
    // 人声：2.2k 提 4dB 让咬字靠前，350 让出 3dB 减哄头；preamp −4
    (
        "vocal",
        r#"{ "preamp_db": -4, "filters": [
            { "kind": "peaking", "freq_hz": 2200, "gain_db": 4, "q": 1.2 },
            { "kind": "peaking", "freq_hz": 350, "gain_db": -3, "q": 1.4 } ] }"#,
    ),
    // 高音：9k 以上 +5dB；preamp −5
    (
        "treble",
        r#"{ "preamp_db": -5, "filters": [
            { "kind": "high_shelf", "freq_hz": 9000, "gain_db": 5 } ] }"#,
    ),
    // 响度补偿：低 +6 / 高 +4 的微笑曲线（小音量下补两端）；preamp −6
    (
        "loudness",
        r#"{ "preamp_db": -6, "filters": [
            { "kind": "low_shelf", "freq_hz": 100, "gain_db": 6 },
            { "kind": "high_shelf", "freq_hz": 10000, "gain_db": 4 } ] }"#,
    ),
];

/// 面板可编辑的一段滤波器（P0，2026-10-02）。
///
/// **`on = false` 的段保留在配置里、但不参与建链** —— 用户关掉一段是「暂时听听看」，
/// 不是「删掉」；关/开必须无损往返。`on` 是**面板概念**：引擎的 `Filter` 有
/// `deny_unknown_fields`、压根不认识它，所以下发前由 `effective_json()` 摘掉关闭的段
/// （见那个函数的注释）。
///
/// `gain_db` / `q` 用 `Option`：引擎对它们的判据是「该有的必须有、不该有的一个字都不许写」
/// （`config.rs` 那条 `deny_unknown_fields` + `uses_gain`）。这里原样透传，
/// **不在宿主这一层替它补齐或缺省** —— 否则两层校验会漂移。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TuningFilter {
    pub kind: String,
    pub freq_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_db: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<f64>,
    #[serde(default = "yes")]
    pub on: bool,
}

fn yes() -> bool {
    true
}

/// **一条通道段**（引擎 `if_else` 的一条，P5-2 扬声器声道槽）：`channel` 命中跑 `then`，
/// 不命中跑 `else_filters`。
///
/// 面板的「通道槽」就映射到它：**全局槽 = `filters`**、通道 k = 第 k 条的 `then`。
/// `then` / `else_filters` 里的段与 `filters` 是**同一种** `TuningFilter`（同样带面板用的
/// `on`）—— 引擎不认识 `on`，下发前由 `effective_json` 摘掉（同 `TuningFilter` 那条纪律）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TuningCondBlock {
    /// 命中的声道下标（0 起）。
    channel: usize,
    #[serde(default)]
    then: Vec<TuningFilter>,
    /// `else` 是 Rust 关键字，字段名靠 `rename` 接回来。
    #[serde(default, rename = "else")]
    else_filters: Vec<TuningFilter>,
}

/// **一条声道复制**（引擎 `channel_copy` 的一条，P5-4 起有界面）：处理后把 `from` 的快照
/// 抄进 `to`（`0→1, 1→2` 这类链式复制与遍历顺序无关，见引擎的 `ChainRuntime`）。
///
/// 与 `graphic_eq` 那类「原样透传的 `serde_json::Value`」不同，这一项**定成类型**：宿主必须
/// 读得出 `from` / `to` —— `for_channels` 要按当前流把越界的复制项裁掉（见那个方法的注释）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TuningCopy {
    /// 源声道下标（0 起）。
    from: usize,
    /// 目标声道下标（0 起）。
    to: usize,
}

/// 落盘形态（`<exe 根>\config\tuning.json`，与 `music.json` / `proxy.json` 同级同口径）。
///
/// **默认关闭**：DSP 会改声音，必须是用户主动打开的（opt-in），不能「装完就替你调过音」。
///
/// **2026-10-02（P0）**：从「只存一个预设键」扩成「存整条可编辑的链」。`preset` 保留，
/// 语义变成**这条链来自哪个内置预设**（用户手改过就是 `"custom"`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TuningConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default = "default_preset")]
    preset: String,
    #[serde(default)]
    preamp_db: f64,
    /// **低音 / 高音快捷增益**（dB，2026-10-03 用户要求 Peace 那三条水平滑块）。
    /// 它们**不是引擎字段**：`effective_json` 在 `filters` 之后**追加一条 low_shelf /
    /// high_shelf** 来表达（见 `BASS_SHELF_HZ` / `TREBLE_SHELF_HZ`）—— 引擎一个字都不用改，
    /// 而频响曲线（宿主按链算）自然把它们算进去。
    #[serde(default, skip_serializing_if = "is_zero")]
    bass_db: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    treble_db: f64,
    #[serde(default)]
    filters: Vec<TuningFilter>,
    /// 以下五项是**引擎形态的新效果器**（GraphicEQ / 延迟 / 声道复制 / 卷积 / If-Else）。
    /// 面板到 P5 才有专门的界面，眼下由配置文件（或 AI）写入，宿主**原样透传**给引擎 ——
    /// 用 `serde_json::Value` 而不再声明一遍形状：合法性一律由引擎的
    /// `ChainConfig::from_json` 兜底（`deny_unknown_fields` + 逐条校验），宿主这一层
    /// 替它摆字段只会让两层漂移（同 `TuningFilter` 的纪律，见 ai-spec 预检 #51 ③）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    graphic_eq: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "is_zero")]
    delay_ms: f64,
    /// 声道复制（P5-4 起有界面）：定成类型而不是 `serde_json::Value`，见 `TuningCopy` 的注释。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    channel_copy: Vec<TuningCopy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    convolution: Option<String>,
    /// 通道段（P5-2）：面板的「通道槽」写的就是这里。空 = 只有全局链。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    if_else: Vec<TuningCondBlock>,
}

fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

impl TuningConfig {
    /// 造一条**只有参数段**的配置（preamp + filters），五项新效果器一律留空。
    ///
    /// 三个用处：预设展开（`default_tuning_config`）、整条链重置（`player_tuning_set`）、
    /// 单测。写成构造器而不是到处手写那五个字段 —— 以后再加新效果器时，
    /// 「参数段配置」这条路径只有一处要改。
    fn parametric(
        enabled: bool,
        preset: String,
        preamp_db: f64,
        filters: Vec<TuningFilter>,
    ) -> Self {
        Self {
            enabled,
            preset,
            preamp_db,
            // 低音 / 高音快捷增益由 `from_preset` → `TuningPreset::apply_to` 或
            // `player_tuning_set_chain` 填；构造一条「只有参数段」的配置时一律 0dB。
            bass_db: 0.0,
            treble_db: 0.0,
            filters,
            graphic_eq: Vec::new(),
            delay_ms: 0.0,
            channel_copy: Vec::new(),
            convolution: None,
            if_else: Vec::new(),
        }
    }

    /// 用一条预设**整条**造配置（`enabled` 由调用方给）。`preset` 会记成预设的名字。
    fn from_preset(preset: &TuningPreset, enabled: bool) -> Self {
        let mut cfg = Self::parametric(enabled, String::new(), 0.0, Vec::new());
        preset.apply_to(&mut cfg);
        cfg
    }

    /// 裁掉**超出这条流的声道数**的通道段（P5-2）与声道复制（P5-4）。
    ///
    /// **为什么要在运行期裁**：一条配置是跨曲目复用的（同一条链在 2 声道与 6 声道两条流上
    /// 各自实例化），而 `ChainRuntime::new` 对越界下标是**报错**（那是对的 —— 手写配置里
    /// 的越界就是错误）。但「给 6 声道配了通道 5、现在放的是立体声」这种情况**不该让整条链
    /// 编不出来**（那会把全局 EQ 也一起丢掉，听起来是「调音突然全没了」）。⇒ 宿主在这里
    /// 按**当前流**裁一刀，并如实记一行日志。
    ///
    /// ⚠️ **两样都要裁**：`if_else`（通道段）与 `channel_copy`（声道复制）在引擎里都会让
    /// `ChainRuntime::new` 对越界下标报错 —— 只裁一样的话，另一样照样能把整条链顶掉。
    fn for_channels(&self, channels: usize) -> TuningConfig {
        let bad_cond: Vec<usize> = self
            .if_else
            .iter()
            .filter(|b| b.channel >= channels)
            .map(|b| b.channel)
            .collect();
        let bad_copy: Vec<(usize, usize)> = self
            .channel_copy
            .iter()
            .filter(|c| c.from >= channels || c.to >= channels)
            .map(|c| (c.from, c.to))
            .collect();
        if bad_cond.is_empty() && bad_copy.is_empty() {
            return self.clone();
        }
        crate::log::warn(format!(
            "player: 通道段 {bad_cond:?} / 声道复制 {bad_copy:?} 超出这条流的 {channels} 声道，\
             本次渲染跳过它们（配置本身不动）"
        ));
        let mut cfg = self.clone();
        cfg.if_else.retain(|b| b.channel < channels);
        cfg.channel_copy.retain(|c| c.from < channels && c.to < channels);
        cfg
    }
}

/// 默认预设键（`TuningConfig` 的 serde 缺省值也是它）。
///
/// ⚠️ **2026-10-03 用户口径：改成 `flat`（空链）**。原来是 `bass` —— 那时面板还画着内置
/// 预设列表，用户能看见「低音增强亮着」；现在面板**不再画内置预设**（用户要求整批删掉），
/// 那份默认链就成了「看着没开、其实在起作用」的隐藏状态 —— 正是本仓最不能接受的那种。
/// 「原声 / 直通」才是这条链该有的初始值：用户不主动调，就什么都不加。
const DEFAULT_PRESET: &str = "flat";

/// 用户手改过链之后 `preset` 的取值。**不是**预设表里的一项，前端不拿它画按钮。
const CUSTOM_PRESET: &str = "custom";

/// 低音 / 高音快捷增益用的搁架频点（Hz）。
/// 与内置预设里那两条 shelf 取同一组值（`bass` 预设用 120、`treble` 用 9000）——
/// 换一个频点会让人对不上「以前那档听起来的样子」。
const BASS_SHELF_HZ: f64 = 120.0;
const TREBLE_SHELF_HZ: f64 = 9000.0;

fn default_preset() -> String {
    DEFAULT_PRESET.to_string()
}

/// 全新安装（没有 `tuning.json`）时的配置：`enabled=false` + 默认预设**展开好的链**。
///
/// 展开而不是留空：否则界面会「亮着某档预设、表里却一段都没有」，用户改一下才发现
/// 自己那档被清掉了 —— 那正是本仓最反感的假状态。
/// ⚠️ 默认档是 `flat`（2026-10-03 起）⇒ 展开结果就是**空链 + preamp 0dB**（原声 / 直通）。
fn default_tuning_config() -> TuningConfig {
    let (preamp_db, filters) = filters_from_preset(DEFAULT_PRESET).unwrap_or((0.0, Vec::new()));
    TuningConfig::parametric(false, DEFAULT_PRESET.to_string(), preamp_db, filters)
}

/// 把一条内置预设**展开成可编辑的段**。
///
/// 预设键从「唯一的真相」降级成**快捷入口**：选一档 = 把它的段填进表里，用户接着改
/// （改完 `preset` 变 `custom`）。所以预设表与可编辑链之间只需要这一条单向展开。
fn filters_from_preset(key: &str) -> Option<(f64, Vec<TuningFilter>)> {
    let json = preset_json(key)?;
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let preamp_db = v.get("preamp_db").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let mut out = Vec::new();
    for f in v.get("filters").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
        out.push(TuningFilter {
            kind: f.get("kind")?.as_str()?.to_string(),
            freq_hz: f.get("freq_hz")?.as_f64()?,
            gain_db: f.get("gain_db").and_then(|x| x.as_f64()),
            q: f.get("q").and_then(|x| x.as_f64()),
            on: true,
        });
    }
    Some((preamp_db, out))
}

/// **升级迁移**：老配置只有 `{enabled, preset}`（没有 `filters`）。
///
/// 不迁移的话，升级后用户**一直在用的那条链会静默消失**（界面上还亮着 preset、
/// 实际是直通）—— 这类「看着开着、其实没生效」正是本仓最不能接受的状态。
/// 判据只看「`filters` 是不是空的」：非空就说明是新配置（哪怕 `preset` 是 `custom`）。
fn migrate_tuning(mut cfg: TuningConfig) -> TuningConfig {
    if !cfg.filters.is_empty() {
        return cfg;
    }
    match filters_from_preset(&cfg.preset) {
        Some((preamp_db, filters)) if !filters.is_empty() => {
            cfg.preamp_db = preamp_db;
            cfg.filters = filters;
            cfg
        }
        // 认不出的预设键（旧版本写的、手改错的）⇒ 回落到默认档，与改造前的行为一致
        _ => default_tuning_config(),
    }
}

/// 把当前链编译成**引擎认识的那份 JSON**。
///
/// 两件事在这里收口：
///   ① **只发 `on` 的段** —— 引擎不认识 `on`（它 `deny_unknown_fields`，多一个字段就报错）；
///   ② **不替引擎补字段** —— `gain_db` / `q` 是 `Some` 才写。引擎对它们的判据是
///      「该有的必须有、不该有的一个字都不许写」，宿主这一层补齐或缺省都会让两层漂移。
fn effective_json(cfg: &TuningConfig) -> String {
    let mut filters = engine_filters(&cfg.filters);
    // 低音 / 高音是**快捷控制**（Peace 那三条水平滑块的后两条），不是用户手加的两段 ——
    // 所以它们不进面板那张频段柱，而是在这里追加成两条 shelf。0dB 时一条都不追加，
    // 落盘与下发 JSON 都干净。
    if cfg.bass_db != 0.0 {
        filters.push(serde_json::json!({
            "kind": "low_shelf", "freq_hz": BASS_SHELF_HZ, "gain_db": cfg.bass_db,
        }));
    }
    if cfg.treble_db != 0.0 {
        filters.push(serde_json::json!({
            "kind": "high_shelf", "freq_hz": TREBLE_SHELF_HZ, "gain_db": cfg.treble_db,
        }));
    }
    let mut o = serde_json::Map::new();
    o.insert("preamp_db".into(), serde_json::json!(cfg.preamp_db));
    o.insert("filters".into(), serde_json::Value::Array(filters));
    // 新效果器：**只在用户真的填了才带上** —— 引擎对每个字段都有缺省语义
    //（不写 = 不启用），少发几个字段能让落盘配置与下发 JSON 都干净。
    if !cfg.graphic_eq.is_empty() {
        o.insert("graphic_eq".into(), serde_json::Value::Array(cfg.graphic_eq.clone()));
    }
    if cfg.delay_ms != 0.0 {
        o.insert("delay_ms".into(), serde_json::json!(cfg.delay_ms));
    }
    if !cfg.channel_copy.is_empty() {
        // `TuningCopy` 是 `Serialize`（字段名 `from` / `to`）⇒ 直接编成引擎认识的那份形状。
        o.insert("channel_copy".into(), serde_json::json!(cfg.channel_copy));
    }
    if let Some(ir) = &cfg.convolution {
        // 相对路径按 **Lunac 根目录**展开：进程 cwd 不一定是安装目录（dev 下还是
        // `target\debug`），直接把它丢给引擎会「同一个配置在两台机器上时灵时不灵」。
        let p = PathBuf::from(ir);
        let full = if p.is_absolute() { p } else { crate::storage::lunac_root_dir().join(p) };
        o.insert("convolution".into(), serde_json::json!(full.to_string_lossy()));
    }
    // 通道段（P5-2）：每条段同样**只发 `on` 的**、摘掉面板用的 `on`；两边都空的块
    // **整个丢掉** —— 引擎对「then 与 else 都空」的块直接报错（那是个摆设），而用户在面板上
    // 把某条通道的段全关掉是很正常的操作，不该因此让**整条链**编不出来。
    let cond: Vec<serde_json::Value> = cfg
        .if_else
        .iter()
        .filter_map(|b| {
            let then = engine_filters(&b.then);
            let els = engine_filters(&b.else_filters);
            if then.is_empty() && els.is_empty() {
                return None;
            }
            let mut o = serde_json::Map::new();
            o.insert("channel".into(), serde_json::json!(b.channel));
            o.insert("then".into(), serde_json::Value::Array(then));
            if !els.is_empty() {
                o.insert("else".into(), serde_json::Value::Array(els));
            }
            Some(serde_json::Value::Object(o))
        })
        .collect();
    if !cond.is_empty() {
        o.insert("if_else".into(), serde_json::Value::Array(cond));
    }
    serde_json::Value::Object(o).to_string()
}

/// 把面板那层的一段段滤波器编成**引擎认识的那份 JSON**。
///
/// 两条规矩（`filters` 与通道段的 `then` / `else` **共用**这一份实现）：
///   ① **只发 `on` 的段**（引擎不认识 `on`，它 `deny_unknown_fields`）；
///   ② **不替引擎补字段** —— `gain_db` / `q` 是 `Some` 才写。引擎对它们的判据是
///      「该有的必须有、不该有的一个字都不许写」，宿主这一层补齐或缺省都会让两层漂移。
fn engine_filters(filters: &[TuningFilter]) -> Vec<serde_json::Value> {
    filters
        .iter()
        .filter(|f| f.on)
        .map(|f| {
            let mut o = serde_json::Map::new();
            o.insert("kind".into(), serde_json::Value::String(f.kind.clone()));
            o.insert("freq_hz".into(), serde_json::json!(f.freq_hz));
            if let Some(g) = f.gain_db {
                o.insert("gain_db".into(), serde_json::json!(g));
            }
            if let Some(q) = f.q {
                o.insert("q".into(), serde_json::json!(q));
            }
            serde_json::Value::Object(o)
        })
        .collect()
}

impl Default for TuningConfig {
    /// **就是 `default_tuning_config()`**（含「把默认预设展开成段」这一步）。
    /// 写成「空链 + 一个预设键」那种简版会让「读盘失败」这条兜底路径变成
    /// 「界面亮着一档、表里一段都没有」—— 那正是本仓最反感的假状态。
    fn default() -> Self {
        default_tuning_config()
    }
}

/// 播放源与命令之间**唯一的共享点**。
///
/// `pub(crate)`：**在线音乐那条链也要它**（`live_audio.rs` 把 librespot 的 PCM 喂进
/// 同一条调音链）—— 两条链共享同一个版本号与同一份配置，所以调音页改一下两边都跟着变。
pub(crate) struct TuningShared {
    /// 配置版本号。播放源每个样本**原子读一次**（几乎免费），只有变了才去拿锁重建链
    /// —— 在音频线程上逐个样本 lock 是优先级反转，绝对不许。
    version: AtomicU64,
    cfg: Mutex<TuningConfig>,
    /// **最近一次建链用的采样率**（Hz，0 = 还没放过东西）。
    ///
    /// 频响曲线要在某个采样率上算（同一条链在 44.1k / 48k 上的高频端不一样），
    /// 而面板是**异步**问宿主的 —— 这条流现在用的采样率只有播放源知道。
    /// 所以在 `retune()` 里顺手记一份：曲线就画「你正在听的那个采样率」上的响应。
    last_rate: AtomicU32,
    /// **最近一次建链那条流的声道数**（0 = 还没放过东西）。
    ///
    /// 面板的**通道槽**（P5-2）要按它决定能选几条声道 —— 给一条 2 声道流配「通道 5」
    /// 那种段在运行期是要被裁掉的（见 `TuningConfig::for_channels`），面板不该让用户选到它。
    /// 同样在 `retune()` 里顺手记。
    last_channels: AtomicU32,
}

static TUNING: OnceLock<Arc<TuningShared>> = OnceLock::new();

/// 进程内唯一的那份调音状态（首次访问时读盘）。
///
/// `pub(crate)`：`live_audio.rs` 给在线那条链取的就是它。
pub(crate) fn tuning_shared() -> Arc<TuningShared> {
    TUNING
        .get_or_init(|| {
            // 起始版本号是 1（不是 0）：源的 `seen` 也从 0 起，于是**第一次 `next()`
            // 就会建一次链**，不必在构造函数里多写一遍。
            Arc::new(TuningShared {
                version: AtomicU64::new(1),
                cfg: Mutex::new(load_tuning_config()),
                last_rate: AtomicU32::new(0),
                last_channels: AtomicU32::new(0),
            })
        })
        .clone()
}

fn tuning_config_path() -> PathBuf {
    crate::storage::lunac_root_dir().join("config").join("tuning.json")
}

fn load_tuning_config() -> TuningConfig {
    let Ok(text) = std::fs::read_to_string(tuning_config_path()) else {
        return default_tuning_config();
    };
    // BOM 要容忍：PowerShell 的 `Set-Content -Encoding UTF8` 会写（同 music.rs）
    let cfg: TuningConfig =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_else(|_| default_tuning_config());
    migrate_tuning(cfg)
}

fn save_tuning_config(cfg: &TuningConfig) -> Result<(), String> {
    let p = tuning_config_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建 config 目录失败：{e}"))?;
    }
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| format!("写 {} 失败：{e}", p.display()))
}

fn preset_json(key: &str) -> Option<&'static str> {
    TUNING_PRESETS.iter().find(|(k, _)| *k == key).map(|(_, j)| *j)
}

// ── 用户预设（P5-1，2026-10-03）────────────────────────────────────
//
// **预设 = 一整条链的快照**，落成 `<exe 根>\config\tuning\presets\preset-<文件名>.json`。
// 内置那 5 档（`TUNING_PRESETS`）只有 `preamp_db` + `filters`；用户预设可以有**全部**字段
// （GraphicEQ / 延迟 / 复制 / 卷积 / 条件段都能一起存进来）。
//
// **文件名与显示名分开**：文件名是**净化过**的（Windows 不允许的字符换成 `_`，并加
// `preset-` 前缀以躲开 `CON` / `NUL` 这类**保留名**），**显示名**存在 JSON 的 `name` 里。
// 删 / 改按**显示名**扫描目录去找文件 —— 从文件名反推显示名会丢字符。

/// 预设文件的固定前缀（见上面那条：躲开 Windows 保留名）。
const PRESET_FILE_PREFIX: &str = "preset-";

/// 一个预设（= 一条链的快照）。字段与 `TuningConfig` 的链部分**一一对应**。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TuningPreset {
    /// 显示名（面板上那一行）。**导入时缺省用文件名**（去前缀与扩展名）。
    #[serde(default)]
    name: String,
    #[serde(default)]
    preamp_db: f64,
    /// 低音 / 高音快捷增益（见 `TuningConfig` 那两条）。
    #[serde(default, skip_serializing_if = "is_zero")]
    bass_db: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    treble_db: f64,
    #[serde(default)]
    filters: Vec<TuningFilter>,
    /// 以下五项与 `TuningConfig` 同义（引擎形态，原样透传）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    graphic_eq: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "is_zero")]
    delay_ms: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    channel_copy: Vec<TuningCopy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    convolution: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    if_else: Vec<TuningCondBlock>,
}

impl TuningPreset {
    /// 从当前配置截一份快照（`name` = 存下来的名字）。
    fn snapshot(cfg: &TuningConfig, name: &str) -> Self {
        Self {
            name: name.to_string(),
            preamp_db: cfg.preamp_db,
            bass_db: cfg.bass_db,
            treble_db: cfg.treble_db,
            filters: cfg.filters.clone(),
            graphic_eq: cfg.graphic_eq.clone(),
            delay_ms: cfg.delay_ms,
            channel_copy: cfg.channel_copy.clone(),
            convolution: cfg.convolution.clone(),
            if_else: cfg.if_else.clone(),
        }
    }

    /// 把预设**整条**盖到配置上：`enabled` 不动，`preset` 记成这个名字，其余字段全换。
    ///
    /// **整条换**（而不是只换 preamp / filters）是刻意的：预设就是「一条链的快照」，
    /// 只换一半会造出「点了预设、IR 还在响」这种说不清的状态（同 `player_tuning_set`）。
    fn apply_to(&self, cfg: &mut TuningConfig) {
        cfg.preset = self.name.clone();
        cfg.preamp_db = self.preamp_db;
        cfg.bass_db = self.bass_db;
        cfg.treble_db = self.treble_db;
        cfg.filters = self.filters.clone();
        cfg.graphic_eq = self.graphic_eq.clone();
        cfg.delay_ms = self.delay_ms;
        cfg.channel_copy = self.channel_copy.clone();
        cfg.convolution = self.convolution.clone();
        cfg.if_else = self.if_else.clone();
    }
}

fn presets_dir() -> PathBuf {
    crate::storage::lunac_root_dir()
        .join("config")
        .join("tuning")
        .join("presets")
}

/// 显示名 → 预设文件名。`None` = 这个名字落不成文件（净化后是空的）。
fn preset_file_name(name: &str) -> Option<String> {
    let mut cleaned = String::new();
    for ch in name.trim().chars() {
        // 中日韩 / 字母数字直接留；路径分隔符与 Windows 禁用字符换成 `_`
        if ch.is_alphanumeric() || matches!(ch, '-' | '_' | ' ' | '.' | '(' | ')') {
            cleaned.push(ch);
        } else {
            cleaned.push('_');
        }
    }
    let cleaned = cleaned.trim().trim_matches('.').trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(format!("{PRESET_FILE_PREFIX}{cleaned}.json"))
    }
}

/// 读一个预设文件；`name` 缺省用文件名（去 `preset-` 前缀与扩展名）。
fn read_preset_file(path: &Path) -> Result<TuningPreset, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("读不了：{e}"))?;
    let mut p: TuningPreset = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("不是合法的预设 JSON：{e}"))?;
    if p.name.trim().is_empty() {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        p.name = stem.strip_prefix(PRESET_FILE_PREFIX).unwrap_or(stem).to_string();
    }
    if p.name.trim().is_empty() {
        return Err("预设没有名字".to_string());
    }
    Ok(p)
}

/// 读出预设目录里的全部预设（按显示名排序）。目录不存在 = 空；**单个文件坏了只跳过 + 记一行**
/// （不让一个手改坏的文件把整个列表打没）。
fn list_user_presets(dir: &Path) -> Vec<TuningPreset> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        match read_preset_file(&path) {
            Ok(p) => out.push(p),
            Err(e) => crate::log::warn(format!("预设 {} 读不了，已跳过（{e}）", path.display())),
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 按**显示名**找预设文件（文件名是净化过的，反推不出显示名 ⇒ 只能扫目录）。
fn find_user_preset_path(dir: &Path, name: &str) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Ok(p) = read_preset_file(&path) {
            if p.name == name {
                return Some(path);
            }
        }
    }
    None
}

/// 按显示名读一条用户预设。
fn find_user_preset(dir: &Path, name: &str) -> Option<TuningPreset> {
    find_user_preset_path(dir, name).and_then(|p| read_preset_file(&p).ok())
}

/// 存一条预设。`overwrite=false` 且同名已存在 ⇒ `ERR_PRESET_EXISTS`（让前端去确认）。
fn save_user_preset(dir: &Path, preset: &TuningPreset, overwrite: bool) -> Result<(), String> {
    if !overwrite && find_user_preset_path(dir, &preset.name).is_some() {
        return Err(format!(
            "ERR_PRESET_EXISTS：已经有一个叫「{}」的预设",
            preset.name
        ));
    }
    let file = preset_file_name(&preset.name)
        .ok_or_else(|| format!("预设名「{}」里没有可用字符", preset.name))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("建预设目录失败：{e}"))?;
    let text = serde_json::to_string_pretty(preset).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(file), text).map_err(|e| format!("写预设失败：{e}"))
}

fn delete_user_preset(dir: &Path, name: &str) -> Result<(), String> {
    let path = find_user_preset_path(dir, name).ok_or_else(|| format!("没有叫「{name}」的预设"))?;
    std::fs::remove_file(&path).map_err(|e| format!("删预设失败：{e}"))
}

/// 重命名（改的是 JSON 里的 `name`，文件名跟着净化后的新名字走）。
fn rename_user_preset(dir: &Path, from: &str, to: &str) -> Result<(), String> {
    let old_path = find_user_preset_path(dir, from).ok_or_else(|| format!("没有叫「{from}」的预设"))?;
    if find_user_preset_path(dir, to).is_some() {
        return Err(format!("ERR_PRESET_EXISTS：已经有一个叫「{to}」的预设"));
    }
    let mut preset = read_preset_file(&old_path)?;
    preset.name = to.to_string();
    save_user_preset(dir, &preset, true)?;
    // 名字变了 ⇒ 文件名一般也变了（净化结果不同）⇒ 旧文件要删；**同名净化结果相同则不删**
    // （否则会把刚写好的那份一起删掉）。
    match find_user_preset_path(dir, to) {
        Some(new_path) if new_path != old_path => {
            let _ = std::fs::remove_file(&old_path);
        }
        _ => {}
    }
    Ok(())
}

/// 取一条预设：**内置优先**，其次用户预设文件。返回的 `name` 就是显示名 / 内置键。
fn resolve_preset(name: &str) -> Option<TuningPreset> {
    if let Some((preamp_db, filters)) = filters_from_preset(name) {
        return Some(TuningPreset {
            name: name.to_string(),
            preamp_db,
            filters,
            ..Default::default()
        });
    }
    find_user_preset(&presets_dir(), name)
}

/// 把**当前配置**编译成链。**采样率必须来自那条流**（配置里刻意没有采样率，
/// 同一条链要在 44.1k / 48k 上各自实例化 —— 见 ai-spec §4.10）。
///
/// **校验的唯一入口**：无论链是从预设展开的还是用户一段段手改的，最终都要过
/// `ChainConfig::from_json`（`deny_unknown_fields` + 逐段越界检查 + 「该有增益的
/// 必须有」）—— 这就是「前端可以下发滤波器数组」那条纪律的兜底（ai-spec 预检 #51 ③）。
fn build_chain_from(cfg: &TuningConfig, sample_rate: f64) -> Result<Chain, String> {
    let json = effective_json(cfg);
    let cc = ChainConfig::from_json(&json, sample_rate)?;
    Chain::build(&cc, sample_rate)
}

/// 频响曲线的采样点数（20Hz–20kHz 对数等分）。240 点在 600px 宽的画布上已经比
/// 像素还密，再多只是白算。
const RESPONSE_POINTS: usize = 240;

/// 频响曲线（**宿主算、前端只画**）。
///
/// 为什么不由前端算：那等于把 RBJ 的系数公式**再实现一遍**，两份必然漂移 ——
/// 而「曲线说 +6dB、听起来不是」是最难查的一类问题。引擎里 `Chain::response_db()`
/// 已经是解析式的（preamp + 各段之和），拿过来直接用即可。
#[derive(Debug, Serialize, Clone)]
pub struct TuningResponse {
    /// 算这条曲线用的采样率（= 最近一次播放那条流的；还没放过东西就是 48k）。
    pub sample_rate: f64,
    pub freqs: Vec<f64>,
    pub db: Vec<f64>,
}

/// 曲线用的采样率：最近一次建链记下的那个，没记过就用 48k（最常见的档）。
fn tuning_response_rate() -> f64 {
    let r = tuning_shared().last_rate.load(Ordering::Relaxed);
    if r == 0 { 48000.0 } else { r as f64 }
}

/// 最近一次建链那条流的**声道数**（0 = 还没放过东西）。
///
/// 面板的通道槽按它决定能选几条（P5-2）。**兜底交给面板**（`channels || 2`）——
/// 宿主在这里假装「就是 2」会把「还不知道」这件事藏起来。
fn tuning_response_channels() -> usize {
    tuning_shared().last_channels.load(Ordering::Relaxed) as usize
}

/// 20Hz–20kHz 对数等分（含两端）。
fn log_spaced_freqs(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    let (l0, l1) = (lo.ln(), hi.ln());
    (0..n)
        .map(|i| (l0 + (l1 - l0) * i as f64 / (n - 1) as f64).exp())
        .collect()
}

/// 面板可见的调音状态（P0 起**带上整条链与频响曲线**）。
///
/// 2026-10-02 之前这里刻意「绝不回传滤波器数组」；P0 把它反过来 —— 用户要能一段段
/// 自己调，面板就必须拿到链。安全边界不变：**链的合法性一律由引擎的
/// `ChainConfig::from_json` 兜底**（见 `build_chain_from`），前端拼不出「界面上亮着、
/// 实际直通」的配置。
#[derive(Debug, Serialize, Clone)]
pub struct TuningDto {
    pub enabled: bool,
    /// 这条链来自哪个内置预设；用户手改过就是 `"custom"`。
    pub preset: String,
    /// 可选预设键 —— **与 `TUNING_PRESETS` 同源**，前端照它画按钮。
    pub presets: Vec<String>,
    /// **用户预设的显示名**（`config\tuning\presets\` 里那份，按名字排序）。
    /// 与内置 `presets` 分开两份：内置用 i18n 键出文案，用户预信用自己的名字。
    pub user_presets: Vec<String>,
    /// 可选滤波器类型 —— **与引擎的 `Kind::ALL` 同源**。前端照它画下拉，
    /// 并按 `gain` / `order` 决定那两个输入框要不要出现。
    /// 名单只在宿主这一份，前端不维护第二份（同 `presets` 的纪律）。
    pub kinds: Vec<TuningKind>,
    pub preamp_db: f64,
    /// **低音 / 高音快捷增益**（dB，2026-10-03）：面板中栏那两条水平滑块的值。
    /// 引擎不认识它们，由 `effective_json` 追加成 low_shelf / high_shelf 两条。
    pub bass_db: f64,
    pub treble_db: f64,
    pub filters: Vec<TuningFilter>,
    /// 新效果器：**引擎形态原样回传**（面板 P5 才有界面；眼下它只是把盘上的值带回来，
    /// 好让一次「改 EQ」的提交不会把它们冲掉 —— 见 `player_tuning_set_chain`）。
    pub graphic_eq: Vec<serde_json::Value>,
    pub delay_ms: f64,
    /// 声道复制（P5-4 起有界面）：定成类型，见 `TuningCopy` 的注释。
    pub channel_copy: Vec<TuningCopy>,
    pub convolution: Option<String>,
    /// 通道段（P5-2 声道槽）：全局槽在 `filters`，通道 k 在这里第 k 条段的 `then`。
    /// 带面板用的 `on`，前端照 `TuningFilter` 那套编辑（下发前由宿主摘掉）。
    pub if_else: Vec<TuningCondBlock>,
    /// 最近一次建链那条流的声道数（`0` = 还不知道 ⇒ 面板按 2 兜底）。
    pub channels: usize,
    /// **按声道的曲线**（P5-2 通道槽）：与 `if_else` **同序**一一对应，第 i 条是
    /// `if_else[i].channel` 那条声道真正会发出的响应（全局链 + 它命中的分支）。
    ///
    /// 为什么由宿主算而不是前端拿 `response` 加一加：曲线的权威在引擎（§4.6 那条纪律），
    /// 而且「哪个块命中哪条声道、else 又落到谁头上」只有引擎那份条件段语义说得清。
    pub cond_db: Vec<Vec<f64>>,
    pub response: TuningResponse,
}

/// 一个滤波器类型的对外形态（见 `TuningDto::kinds`）。
#[derive(Debug, Serialize, Clone)]
pub struct TuningKind {
    pub name: String,
    /// 用不用 `gain_db`。false 的类型给了增益，引擎会**拒掉整条链**。
    pub gain: bool,
    /// 阶数：`1` 表示没有 Q（一阶没有 Q 这个参数）。
    pub order: u8,
}

fn tuning_kinds() -> Vec<TuningKind> {
    tuning_engine::biquad::Kind::ALL
        .iter()
        .map(|k| TuningKind {
            name: k.name().to_string(),
            gain: k.uses_gain(),
            order: k.order(),
        })
        .collect()
}

fn tuning_dto(cfg: &TuningConfig) -> TuningDto {
    let rate = tuning_response_rate();
    let freqs = log_spaced_freqs(20.0, 20_000.0, RESPONSE_POINTS);
    // 建不起链（某段越界、或全被关掉）⇒ 画一条平线。**不报错** —— `set` 那条路
    // 已经如实报了错，面板要能一直画出来（与 `player_tuning_get` 同一口径）。
    let (db, cond_db) = match build_chain_from(cfg, rate) {
        Ok(chain) => {
            let db = freqs.iter().map(|f| chain.response_db(*f, rate)).collect();
            // 通道槽的曲线（P5-2）：与 `if_else` **同序**逐条算 —— 前端照 `channel` 对位取用。
            let cond_db = cfg
                .if_else
                .iter()
                .map(|b| {
                    freqs
                        .iter()
                        .map(|f| chain.response_db_for_channel(*f, rate, b.channel))
                        .collect()
                })
                .collect();
            (db, cond_db)
        }
        Err(_) => (vec![0.0; freqs.len()], Vec::new()),
    };
    TuningDto {
        enabled: cfg.enabled,
        preset: cfg.preset.clone(),
        presets: TUNING_PRESETS.iter().map(|(k, _)| (*k).to_string()).collect(),
        user_presets: list_user_presets(&presets_dir())
            .into_iter()
            .map(|p| p.name)
            .collect(),
        kinds: tuning_kinds(),
        preamp_db: cfg.preamp_db,
        bass_db: cfg.bass_db,
        treble_db: cfg.treble_db,
        filters: cfg.filters.clone(),
        graphic_eq: cfg.graphic_eq.clone(),
        delay_ms: cfg.delay_ms,
        channel_copy: cfg.channel_copy.clone(),
        convolution: cfg.convolution.clone(),
        if_else: cfg.if_else.clone(),
        channels: tuning_response_channels(),
        cond_db,
        response: TuningResponse { sample_rate: rate, freqs, db },
    }
}

/// 按配置处理一条**交错**样本流（rodio 的 `Source` 就是逐样本拉的）。
///
/// 三条纪律：
/// ① **每个声道各持一份滤波器状态** —— 共享状态会把左右声道串起来（左边先响、
///    右边听到左边的尾巴），而这种错在单声道素材上完全看不出来（引擎侧也有单测钉这条）。
/// ② **配置热换**：`next()` 里只做一次原子读，版本号变了才拿锁重建链
///    ⇒ 改档**不打断正在放的这一首**（换链那一刻状态清零，听感上是一个瞬态）。
/// ③ **建不起链就直通**（并记一行日志）—— 宁可什么都不加，也不能让「上一档」继续生效。
/// `pub(crate)`：本地文件（`play()`）与在线音乐（`live_audio.rs`）**共用这一层** ——
/// 调音链挂在「输出」上而不是挂在「解码器」上，所以只要音频流经过我们，逐段开关 /
/// 声道槽 / 延迟 / 复制 / 卷积 / 条件段全都自动生效。
pub(crate) struct TuningSource<S: Source> {
    inner: S,
    shared: Arc<TuningShared>,
    seen: u64,
    /// `None` = 直通（未启用、或那一档没建起来）。
    ///
    /// **2026-10-03 起是 `ChainRuntime` 而不是 `Chain`**：延迟 / 声道复制 / 卷积 / 条件段
    /// （If-Else）都**要自己的缓冲**，塞不进 `Chain::process_sample` 那个
    /// 「声道数 × 段数」的定长状态切片 ⇒ 它们只能按**帧**处理（见 `chain::ChainRuntime`）。
    rt: Option<ChainRuntime>,
    /// 卷积（IR）—— **每条声道一个**流式卷积器（单声道 IR，同一条喂所有声道）。
    /// `None` = 没配卷积。它排在 runtime **之后**（与 `chain::render` 的处理顺序一致）。
    conv: Option<Vec<Convolver>>,
    /// **攒帧**用的输入缓冲（长度最多 = 声道数）。rodio 的 `Source` 是**逐样本**拉的，
    /// 而 `ChainRuntime` 按**帧**处理 ⇒ 这里把一帧攒齐再交出去；缓冲复用，
    /// **不在音频线程上分配**。
    buf: Vec<f64>,
    /// 已处理、待逐个吐出的样本（就是刚处理完的那一帧）。同样复用。
    out: Vec<f64>,
    /// `out` 里已吐到第几个。
    out_pos: usize,
}

impl<S: Source> TuningSource<S> {
    pub(crate) fn new(inner: S, shared: Arc<TuningShared>) -> Self {
        Self {
            inner,
            shared,
            seen: 0,
            rt: None,
            conv: None,
            buf: Vec::new(),
            out: Vec::new(),
            out_pos: 0,
        }
    }

    /// 版本号没变就立刻返回（常态路径 = 一次宽松原子读）。
    fn retune(&mut self) {
        let v = self.shared.version.load(Ordering::Relaxed);
        if v == self.seen {
            return;
        }
        self.seen = v;
        self.rt = None;
        self.conv = None;
        let cfg = match self.shared.cfg.lock() {
            Ok(g) => g.clone(),
            Err(_) => return,
        };
        let rate_raw = self.inner.sample_rate().get();
        let rate = rate_raw as f64;
        // 记下这条流的采样率：面板那条频响曲线要在**同一个采样率**上算（同一条链在
        // 44.1k / 48k 上的高频端不一样）。**排在 `enabled` 判断之前** —— 关着的时候
        // 用户照样在看曲线，那时也该画「你正在听的那个采样率」上的响应。
        self.shared.last_rate.store(rate_raw, Ordering::Relaxed);
        // 声道数同理（面板的**通道槽**按它决定能选几条）——**也排在 `enabled` 判断之前**。
        let channels = self.inner.channels().get() as usize;
        self.shared
            .last_channels
            .store(channels as u32, Ordering::Relaxed);
        if !cfg.enabled {
            return;
        }
        if channels == 0 {
            return;
        }
        // 通道段按**这条流**裁一刀（见 `for_channels` 的注释）：越界的段跳过 ——
        // 不让「6 声道配置 + 立体声素材」把整条链（含全局 EQ）一起作废。
        let cfg = cfg.for_channels(channels);
        let chain = match build_chain_from(&cfg, rate) {
            Ok(c) => c,
            Err(e) => {
                crate::log::warn(format!("player: 调音链没建起来，这一首按直通放（{e}）"));
                return;
            }
        };
        // 运行期那一层（每声道状态 + 延迟线 + 复制表 + 条件段）就是在这里实例化的。
        // **不再用「段数 == 0 就跳过」那条捷径**：延迟 / 复制 / 卷积 / 条件段都可能
        // 「一段双二阶都没有」却仍然真实生效 —— 那种情况下跳过 = 静默不生效。
        match ChainRuntime::new(&chain, channels) {
            Ok(rt) => self.rt = Some(rt),
            Err(e) => {
                crate::log::warn(format!("player: 调音链的运行期建不起来，这一首按直通放（{e}）"));
                return;
            }
        }
        // 卷积（IR）：每条声道一个**流式**卷积器（引擎的 `Convolver`）。
        // ⚠️ 它引入 = IR 长度 的延迟（引擎里写着）—— 音乐播放没有音画同步问题，可以接受。
        if let Some(ir) = chain.ir() {
            match Convolver::new(ir) {
                // 同一条 IR 喂所有声道 ⇒ 克隆即可（频谱与状态各持一份）
                Ok(c) => self.conv = Some((0..channels).map(|_| c.clone()).collect()),
                Err(e) => {
                    crate::log::warn(format!("player: 卷积内核没建起来，这一段按不卷积放（{e}）"));
                }
            }
        }
    }
}

impl<S: Source> Iterator for TuningSource<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        // ① 先把上一帧剩下的样本吐完（连吐 `声道数` 次才需要再处理一帧）
        if self.out_pos < self.out.len() {
            let y = self.out[self.out_pos];
            self.out_pos += 1;
            return Some(y as Sample);
        }
        // ② 攒下一帧。rodio 逐样本拉，这里攒齐 `声道数` 个再交给 runtime。
        let channels = self.inner.channels().get() as usize;
        if channels == 0 {
            return self.inner.next();
        }
        self.buf.clear();
        while self.buf.len() < channels {
            match self.inner.next() {
                Some(s) => self.buf.push(s as f64),
                None => break,
            }
        }
        if self.buf.is_empty() {
            return None; // 流放完了
        }
        self.retune();
        // 满一帧才处理；**末尾的半帧**（流长度不是声道数的整数倍）原样放出去 ——
        // `process_frame` 要求长度严格等于声道数，硬塞会报错，而为了半帧去补零更糟。
        if self.buf.len() == channels {
            if let Some(rt) = self.rt.as_mut() {
                // 处理失败（理论上只剩声道数不符）也**不许吞掉声音**：原值放出去。
                let _ = rt.process_frame(&mut self.buf);
            }
            // ⑤ 卷积排在 runtime 之后（与 `chain::render` 的处理顺序一致）。逐样本喂
            //（`Convolver` 自己攒块、自己攒尾巴 —— 见那边的注释）。
            if let Some(cs) = self.conv.as_mut() {
                for (ch, v) in self.buf.iter_mut().enumerate() {
                    if let Some(c) = cs.get_mut(ch) {
                        *v = c.process_sample(*v);
                    }
                }
            }
        }
        // 交出这一帧：`out` 成为「待吐的帧」，`buf` 留作下一个帧的攒帧缓冲（复用，不分配）。
        std::mem::swap(&mut self.buf, &mut self.out);
        self.out_pos = 1;
        Some(self.out[0] as Sample)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S: Source> Source for TuningSource<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    /// 必须转发：本类型夹在 `amplify`（rodio 的 seek 就是从它往下打的）与解码器中间，
    /// 不转发的话 `player_seek` 会一律回 `NotSupported` —— 界面表现是「进度条拖不动」。
    /// 跳转成功后**清掉滤波器状态**：那些状态属于「上一段音频」，留着会带一声爆音出来。
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let r = self.inner.try_seek(pos);
        if r.is_ok() {
            // 状态属于「上一段音频」，留着会带一声爆音出来（延迟线尤其明显）。
            if let Some(rt) = self.rt.as_mut() {
                rt.reset();
            }
            // 卷积的块 / 尾巴同理（不复位的话，上一段的尾巴会响在新位置上）。
            if let Some(cs) = self.conv.as_mut() {
                for c in cs.iter_mut() {
                    c.reset();
                }
            }
            // 攒帧缓冲也一样丢：播放位置已经跳了，留着会拼出「半帧旧 + 半帧新」。
            self.buf.clear();
            self.out.clear();
            self.out_pos = 0;
        }
        r
    }
}

/// 把一段**内存里的**交错样本套上调音链，得到一条可直接 `append` 的 source。
///
/// 唯一的用处是**调音实时测量**（2026-10-02，`tuning_probe.rs`）：那里要播一段
/// 扫频，并且必须让它**走和音乐同一条链**（否则量到的是直通，不是「层层叠加后的」
/// 输出）。所以这里刻意复用 `TuningSource`，而不是另给测量写一条旁路 ——
/// 旁路一旦与播放链不同源，「测出来是平的」就说明不了任何事。
pub(crate) fn tuned_buffer(samples: Vec<f32>, channels: u16, rate: u32) -> impl Source {
    let ch = ChannelCount::new(channels).unwrap_or(ChannelCount::MIN);
    let sr = SampleRate::new(rate).unwrap_or(SampleRate::MIN);
    TuningSource::new(rodio::buffer::SamplesBuffer::new(ch, sr, samples), tuning_shared())
}

/// 同一段内存样本 **不经调音链**播出去（直通）。
///
/// 用途：调音实时测量的**旁路那一趟**（`tuning_probe.rs`，2026-10-03 起测两趟）——
/// 只有「同一段扫频、一趟直通、一趟走链」两条曲线相除，才能把**系统**（EAPO / 设备 /
/// 别的音效）从「链」里剥出来。与 `tuned_buffer` 是同一段样本的两种走法，其余完全相同。
pub(crate) fn plain_buffer(samples: Vec<f32>, channels: u16, rate: u32) -> impl Source {
    let ch = ChannelCount::new(channels).unwrap_or(ChannelCount::MIN);
    let sr = SampleRate::new(rate).unwrap_or(SampleRate::MIN);
    rodio::buffer::SamplesBuffer::new(ch, sr, samples)
}

/// 测量期间把我们**自己**正在放的那一首暂停（返回「本来是不是在放」）。
///
/// 回环采集录的是**整台输出设备**的混音：不暂停的话，正在播的那首歌会和扫频
/// 叠在一起，量出来的曲线是两段互不相干的信号相除的结果 —— 看起来像「曲线很脏」，
/// 实则是测量方法被污染了。外部程序（浏览器 / 别的播放器）那份我们管不着，
/// 只能在界面上提示用户先静音它们（见 `music.tuning_probe_hint`）。
pub(crate) fn pause_for_probe() -> bool {
    let guard = match SESSION.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    match guard.as_ref() {
        Some(s) if !s.player.is_paused() => {
            s.player.pause();
            true
        }
        _ => false,
    }
}

/// 测量结束后把上面那次暂停还回去（`was == false` 时什么也不做）。
pub(crate) fn resume_after_probe(was: bool) {
    if !was {
        return;
    }
    if let Ok(guard) = SESSION.lock() {
        if let Some(s) = guard.as_ref() {
            s.player.play();
        }
    }
}

/// 测量结果要对齐的那条「链的合成曲线」，在**同一个采样率**上算。
///
/// 与 `tuning_dto` 同一口径：**永不失败** —— 建不起链（某段越界 / 全被关掉）
/// 就给一条平线，测量报告本身不该因此变成一条错误。
pub(crate) fn tuning_reference_db(freqs: &[f64], rate: f64) -> Vec<f64> {
    let cfg = match tuning_shared().cfg.lock() {
        Ok(g) => g.clone(),
        Err(_) => TuningConfig::default(),
    };
    match build_chain_from(&cfg, rate) {
        Ok(chain) => freqs.iter().map(|f| chain.response_db(*f, rate)).collect(),
        Err(_) => vec![0.0; freqs.len()],
    }
}

/// 起播一个文件（换曲也走它）。**同步实现**，命令那一层在外面套 `run_blocking`。
fn play(path: &str) -> Result<PlayerDto, String> {
    // 先把解码器开出来：**开不了就别把正在放的那首停掉**（用户点了坏文件，
    // 不该把他的播放也带走）。
    let p = PathBuf::from(path);
    let (decoder, duration_ms) = open_decoder(&p)?;
    // 调音状态要在**拿 SESSION 锁之前**取：首次访问它会读一次盘，
    // 而 SESSION 锁是播放/暂停/状态轮询共用的那把 —— 别把读盘塞进临界区。
    let tuning = tuning_shared();

    let mut guard = SESSION.lock().map_err(|e| e.to_string())?;
    // 设备沿用上一首的（没有才开新的）；Player 换一条新的 —— 这一步同时也是
    // 「停掉上一首」：旧 Session 的其余字段在这里被 drop，旧 Player 的 Drop 会
    // 把它的 source 收掉。
    let sink = match guard.take() {
        Some(old) => old.sink,
        None => {
            let mut s = rodio::DeviceSinkBuilder::open_default_sink()
                .map_err(|_| "ERR_NO_AUDIO_DEVICE".to_string())?;
            // 默认会在 drop 时往 stderr 打一句「Dropping DeviceSink, audio playing …」
            // —— 我们**每换一首曲目**都会走到那条路（见下面的形态），每首歌一行噪音。
            s.log_on_drop(false);
            s
        }
    };
    let player = rodio::Player::connect_new(sink.mixer());
    player.set_volume(volume());
    // 解码器外面套一层调音链（关着的时候它逐样本原样透传，代价是一次原子读）
    player.append(TuningSource::new(decoder, tuning));
    *guard = Some(Session { sink, player, path: p, duration_ms });
    Ok(dto_of(guard.as_ref()))
}

// ── 命令 ──────────────────────────────────────────────────────────
// 命名前缀 `player_` —— 与 `media_*`（媒体库）刻意分开：这一层管的是**出声**，
// 与索引、扫描无关；界面上它们也确实是两件事（列表能看 ≠ 能放）。

/// 播一个本地文件（`path` 必须存在且能被 symphonia 认出来）。
#[tauri::command]
pub async fn player_play(path: String) -> Result<PlayerDto, String> {
    crate::commands::run_blocking(move || play(&path)).await
}

#[tauri::command]
pub fn player_pause() -> Result<PlayerDto, String> {
    let guard = SESSION.lock().map_err(|e| e.to_string())?;
    if let Some(s) = guard.as_ref() {
        s.player.pause();
    }
    Ok(dto_of(guard.as_ref()))
}

#[tauri::command]
pub fn player_resume() -> Result<PlayerDto, String> {
    let guard = SESSION.lock().map_err(|e| e.to_string())?;
    if let Some(s) = guard.as_ref() {
        s.player.play();
    }
    Ok(dto_of(guard.as_ref()))
}

/// 停止并把设备一起放掉（**不是**暂停）。关窗时走的就是这条路。
#[tauri::command]
pub fn player_stop() -> Result<PlayerDto, String> {
    let mut guard = SESSION.lock().map_err(|e| e.to_string())?;
    *guard = None;
    Ok(dto_of(None))
}

/// 跳转（毫秒）。**失败不是错误**：有些容器/channel 不支持 seek（rodio 会回
/// `SeekError::NotSupported`），如实回报当前真实位置就好 —— 报错只会让用户以为
/// 整个播放坏了。
#[tauri::command]
pub fn player_seek(position_ms: u64) -> Result<PlayerDto, String> {
    let guard = SESSION.lock().map_err(|e| e.to_string())?;
    if let Some(s) = guard.as_ref() {
        if let Err(e) = s.player.try_seek(Duration::from_millis(position_ms)) {
            crate::log::warn(&format!("player: 跳转失败（{e}）"));
        }
    }
    Ok(dto_of(guard.as_ref()))
}

/// 音量 0.0–1.0（超出即夹取，不报错 —— 滑条抖动送进来 1.0000001 不该是错误）。
#[tauri::command]
pub fn player_volume(volume: f32) -> Result<PlayerDto, String> {
    let v = if volume.is_finite() { volume.clamp(0.0, 1.0) } else { DEFAULT_VOLUME };
    VOLUME.store(v.to_bits(), Ordering::Relaxed);
    let guard = SESSION.lock().map_err(|e| e.to_string())?;
    if let Some(s) = guard.as_ref() {
        s.player.set_volume(v);
    }
    Ok(dto_of(guard.as_ref()))
}

/// 轮询用（界面每 500ms 一次）。**永不失败** —— 轮询里的错误只会变成一行红字噪音。
#[tauri::command]
pub fn player_status() -> PlayerDto {
    match SESSION.lock() {
        Ok(g) => dto_of(g.as_ref()),
        Err(_) => PlayerDto { volume: volume(), ..Default::default() },
    }
}

/// 当前调音设置。**永不失败**（与 `player_status` 同一口径：面板要能一直画出来）。
#[tauri::command]
pub fn player_tuning_get() -> TuningDto {
    let shared = tuning_shared();
    // 注意：这里**必须先绑一个局部量再返回**。写成 `match ... { }` 当尾表达式时，
    // 那个 `MutexGuard` 的临时值要到语句结束才析构，而它借用的 `shared` 先被 drop
    // ⇒ E0597（编译器给的就是这条建议）。
    let dto = match shared.cfg.lock() {
        Ok(g) => tuning_dto(&g),
        Err(_) => tuning_dto(&TuningConfig::default()),
    };
    dto
}

/// 「**先校验、再落盘、最后才改内存与版本号**」这一条的唯一落点。
///
/// 顺序反过来的话，写盘失败会留下「界面上已经生效、重启就没了」的分裂状态。
/// 校验由调用方在进来**之前**做完（它知道该用哪个采样率）。
fn apply_tuning_config(cfg: TuningConfig) -> Result<TuningDto, String> {
    save_tuning_config(&cfg)?;
    let shared = tuning_shared();
    {
        let mut g = shared.cfg.lock().map_err(|e| e.to_string())?;
        *g = cfg;
    }
    // 自增版本号 ⇒ **正在放的那一首**下一个样本就换上新链（不必重播、不必重开窗）
    shared.version.fetch_add(1, Ordering::SeqCst);
    Ok(player_tuning_get())
}

// ── A-B 盲测（2026-10-06）───────────────────────────────────────────
//
// 两个**快照槽**（A / B）各存一份**整条链**的快照，落在
// `<exe 根>\config\tuning\ab-a.json` / `ab-b.json`。「随机映射 + 揭晓」是**纯前端**的事
// （宿主只认槽名），所以这里只提供「存 / 取 / 有没有」三件事。
//
// 应用槽必须走 `apply_tuning_config`（预检 #51 ③：先校验 → 再落盘 → 最后改内存与版本号），
// 与预设是同一条路 —— 于是「切 A/B」**不重播**，当前这首下一个样本就换上新链。

/// A / B 两槽各自有没有快照（前端据此置灰按钮）。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TuningAbState {
    pub a: bool,
    pub b: bool,
}

/// 槽名 → 快照文件。只认 `a` / `b`（其余返回 `None`，调用方据此报错）。
fn ab_slot_path(slot: &str) -> Option<PathBuf> {
    let s = slot.trim().to_ascii_lowercase();
    if s != "a" && s != "b" {
        return None;
    }
    Some(
        crate::storage::lunac_root_dir()
            .join("config")
            .join("tuning")
            .join(format!("ab-{s}.json")),
    )
}

fn tuning_ab_state() -> TuningAbState {
    TuningAbState {
        a: ab_slot_path("a").map(|p| p.is_file()).unwrap_or(false),
        b: ab_slot_path("b").map(|p| p.is_file()).unwrap_or(false),
    }
}

/// A / B 两槽各自有没有快照。
#[tauri::command]
pub fn player_tuning_ab_state() -> TuningAbState {
    tuning_ab_state()
}

/// 把**当前这条链**快照进 A / B 槽。
#[tauri::command]
pub fn player_tuning_ab_save(slot: String) -> Result<TuningAbState, String> {
    let path = ab_slot_path(&slot)
        .ok_or_else(|| format!("ERR_BAD_SLOT：槽只能是 a / b（收到 {slot}）"))?;
    let cfg = current_tuning()?;
    // 存之前先让引擎验一遍（同 `preset_save`：存一条编不成链的快照 = 埋一个「一点就直通」的雷）
    build_chain_from(&cfg, 44100.0)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建 tuning 目录失败：{e}"))?;
    }
    let text = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
    let name = slot.trim().to_ascii_uppercase();
    crate::log::info(format!("player：调音 A/B 快照已存入 {name} 槽"));
    Ok(tuning_ab_state())
}

/// 应用 A / B 槽的快照（经唯一写点 `apply_tuning_config`）。
#[tauri::command]
pub fn player_tuning_ab_apply(slot: String) -> Result<TuningDto, String> {
    let path = ab_slot_path(&slot)
        .ok_or_else(|| format!("ERR_BAD_SLOT：槽只能是 a / b（收到 {slot}）"))?;
    let name = slot.trim().to_ascii_uppercase();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Err(format!("ERR_EMPTY_SLOT：{name} 槽还没有快照"));
    };
    let parsed: TuningConfig = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("ERR_BAD_SLOT_FILE：{name} 槽的快照读不出来（{e}）"))?;
    let cfg = migrate_tuning(parsed);
    // 校验用 44.1kHz（同 `set_chain`）；真实链由 `retune` 按当前流的采样率重建
    build_chain_from(&cfg, 44100.0)?;
    apply_tuning_config(cfg)
}

/// 当前配置的一份克隆（拿锁、克隆、立刻放开 —— 不要在临界区里做别的事）。
fn current_tuning() -> Result<TuningConfig, String> {
    let shared = tuning_shared();
    let cfg = shared.cfg.lock().map_err(|e| e.to_string())?.clone();
    Ok(cfg)
}

/// 改调音设置（`enabled` + 预设键）。**这是预设那条「快捷入口」的老命令**，
/// 语义没变：选一档 = 把它的段**展开**进表（P0 起链是可编辑的，展开之后用户接着改）。
///
/// 校验用 **44.1kHz**（常见采样率里 Nyquist 最窄的那个）—— 它过了就等于 48k /
/// 96k 都过，不需要为每条流各试一遍。**校验不过就不落盘**：宁可这次设置失败，
/// 也不能让界面上亮着一档而实际是直通。
#[tauri::command]
pub fn player_tuning_set(enabled: bool, preset: String) -> Result<TuningDto, String> {
    let p = resolve_preset(&preset)
        .ok_or_else(|| format!("ERR_BAD_PRESET：没有这个预设（{preset}）"))?;
    // 这条命令是**整条链重置**：预设是什么就用什么（内置只有 `preamp` + `filters`，
    // 用户预设可能还带着 GraphicEQ / IR / 延迟…，一并照搬）。
    let cfg = TuningConfig::from_preset(&p, enabled);
    build_chain_from(&cfg, 44100.0)?;
    apply_tuning_config(cfg)
}

/// 只改总开关（P0 加的）。
///
/// **为什么与 `player_tuning_set` 分开**：那条路会**重置整条链**，拿它来开/关会把
/// 用户刚调好的每一段冲掉 —— 一个「只是关一下」的动作不该有这种副作用。
#[tauri::command]
pub fn player_tuning_set_enabled(enabled: bool) -> Result<TuningDto, String> {
    let mut cfg = current_tuning()?;
    cfg.enabled = enabled;
    apply_tuning_config(cfg)
}

/// 把一条预设**整条**展开进来（内置 / 用户预设都认，P5-1）。`enabled` 不动。
///
/// ⚠️ 2026-10-03 起是**整条换**（以前只换 `preamp` + `filters`、保留其余字段）——
/// 与「预设 = 一条链的快照」这个模型一致；否则会出现「点了预设、IR 还在响」。
#[tauri::command]
pub fn player_tuning_apply_preset(preset: String) -> Result<TuningDto, String> {
    let p = resolve_preset(&preset)
        .ok_or_else(|| format!("ERR_BAD_PRESET：没有这个预设（{preset}）"))?;
    let mut cfg = current_tuning()?;
    p.apply_to(&mut cfg);
    build_chain_from(&cfg, 44100.0)?;
    apply_tuning_config(cfg)
}

/// **覆盖整条链**（P0 的主入口）。`enabled` 不动，`preset` 变 `custom`。
///
/// 前端下发的就是滤波器数组 —— 合法性一律由 `build_chain_from` 里那层
/// `ChainConfig::from_json` 兜底（未知字段 / 越界 / 「该有增益的必须有」）。
/// 校验用 44.1kHz（同 `player_tuning_set`）。
#[tauri::command]
pub fn player_tuning_set_chain(
    preamp_db: f64,
    // 低音 / 高音快捷增益（见 `TuningConfig`）。`Option` 是为了老前端不传时也能过。
    bass_db: Option<f64>,
    treble_db: Option<f64>,
    filters: Vec<TuningFilter>,
    // 五项新效果器（引擎形态，由引擎校验）。**用 `Option` 是为了老前端不传时也能过** ——
    // 但语义是「**覆盖整条链**」：`None` = 清空。面板那道调用会把 `player_tuning_get`
    // 拿到的原值一并回传，所以正常路径不会把用户手写的 IR / GraphicEQ 误清。
    graphic_eq: Option<Vec<serde_json::Value>>,
    delay_ms: Option<f64>,
    channel_copy: Option<Vec<TuningCopy>>,
    convolution: Option<String>,
    // 通道段（P5-2）：面板把它维护的**全部**通道槽一并回传（全局槽在 `filters`）。
    if_else: Option<Vec<TuningCondBlock>>,
) -> Result<TuningDto, String> {
    let mut cfg = current_tuning()?;
    cfg.preset = CUSTOM_PRESET.to_string();
    cfg.preamp_db = preamp_db;
    cfg.bass_db = bass_db.unwrap_or(0.0);
    cfg.treble_db = treble_db.unwrap_or(0.0);
    cfg.filters = filters;
    cfg.graphic_eq = graphic_eq.unwrap_or_default();
    cfg.delay_ms = delay_ms.unwrap_or(0.0);
    cfg.channel_copy = channel_copy.unwrap_or_default();
    // 空串 = 没配（引擎也会拒空路径，这里顺手归一成 `None`，落盘更干净）
    cfg.convolution = convolution.filter(|s| !s.trim().is_empty());
    cfg.if_else = if_else.unwrap_or_default();
    build_chain_from(&cfg, 44100.0)?;
    apply_tuning_config(cfg)
}

// ── 用户预设的命令（P5-1，2026-10-03）──────────────────────────────
//
// 文件对话框在**前端**（`@tauri-apps/plugin-dialog`，WebView2 不支持 `window.prompt`），
// 这里只收路径、只做文件 IO —— 与「宿主只管文件、前端只管界面」这条分工一致。

/// 把**当前这条链**存成一个预设。`overwrite=false` 且同名已存在 ⇒ `ERR_PRESET_EXISTS`
/// （前端据此让用户确认覆盖，不许静默盖掉人家存的链）。
#[tauri::command]
pub fn player_tuning_preset_save(name: String, overwrite: bool) -> Result<TuningDto, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("预设名不能为空".to_string());
    }
    let mut cfg = current_tuning()?;
    // **存之前先让引擎验一遍**：存进去一条编不成链的预设，等于埋一个「点了就直通」的雷。
    build_chain_from(&cfg, 44100.0)?;
    save_user_preset(&presets_dir(), &TuningPreset::snapshot(&cfg, name), overwrite)?;
    cfg.preset = name.to_string();
    apply_tuning_config(cfg)
}

/// 删一条用户预设。正用着它的那条链**不动**（链是链、预设是预设），只把来源标成 `custom`。
#[tauri::command]
pub fn player_tuning_preset_delete(name: String) -> Result<TuningDto, String> {
    let mut cfg = current_tuning()?;
    delete_user_preset(&presets_dir(), &name)?;
    if cfg.preset == name {
        cfg.preset = CUSTOM_PRESET.to_string();
    }
    apply_tuning_config(cfg)
}

/// 重命名一条用户预设（同名已存在 ⇒ `ERR_PRESET_EXISTS`）。正用着它的话，来源跟着改名。
#[tauri::command]
pub fn player_tuning_preset_rename(from: String, to: String) -> Result<TuningDto, String> {
    let to = to.trim();
    if to.is_empty() {
        return Err("预设名不能为空".to_string());
    }
    let mut cfg = current_tuning()?;
    rename_user_preset(&presets_dir(), &from, to)?;
    if cfg.preset == from {
        cfg.preset = to.to_string();
    }
    apply_tuning_config(cfg)
}

/// 导入一个预设文件（路径由前端对话框给）。**导入即应用** —— 导进来却不生效还要再点一下
/// 才奇怪；同名已存在则覆盖（是用户自己选的文件，覆盖是明确的意图）。
#[tauri::command]
pub fn player_tuning_preset_import(path: String) -> Result<TuningDto, String> {
    let preset = read_preset_file(Path::new(&path)).map_err(|e| format!("导入失败：{e}"))?;
    let cfg = current_tuning()?;
    // 先验再落盘 / 再换链 —— 一条坏预设不许把当前正在听的链顶掉。
    let candidate = TuningConfig::from_preset(&preset, cfg.enabled);
    build_chain_from(&candidate, 44100.0)?;
    save_user_preset(&presets_dir(), &preset, true)?;
    apply_tuning_config(candidate)
}

/// 把**当前这条链**导出成预设文件（路径由前端「另存为」对话框给）。
#[tauri::command]
pub fn player_tuning_preset_export(path: String) -> Result<(), String> {
    let cfg = current_tuning()?;
    build_chain_from(&cfg, 44100.0)?;
    // 链来自某个预设就用它的名字，否则叫 `custom`（别的播放器/用户看到名字就知道是什么）
    let name = if cfg.preset.is_empty() {
        CUSTOM_PRESET.to_string()
    } else {
        cfg.preset.clone()
    };
    let text = serde_json::to_string_pretty(&TuningPreset::snapshot(&cfg, &name))
        .map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("导出失败：{e}"))
}

/// 音乐插件窗关了 ⇒ 收掉正在放的声音。**返回「这次是不是真的收掉了」** ——
/// 调用方据此决定要不要打日志（这条回调每个插件窗都走一遍，没在放就别记一行
/// 「已停止播放」）。判据不在这里重复，调用方用的是同一条 label 检查。
pub fn stop_if_running() -> bool {
    let mut guard = match SESSION.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    if guard.is_none() {
        return false;
    }
    *guard = None;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个最小的合法 WAV（单声道 16bit 8kHz、8 个样本）。
    ///
    /// **手写 44 字节头而不是引一个 wav 生成器**：这条测试要证的只是「解码器认得出
    /// 这个文件、并给得出时长」—— 用调音引擎的 `gen` 会把宿主测试变成跨 crate 依赖。
    fn tiny_wav(path: &Path) {
        let samples: [i16; 8] = [0, 3000, 6000, 3000, 0, -3000, -6000, -3000];
        let data_len = (samples.len() * 2) as u32;
        let mut b = Vec::with_capacity(44 + data_len as usize);
        b.extend(b"RIFF");
        b.extend((36 + data_len).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes()); // PCM
        b.extend(1u16.to_le_bytes()); // 单声道
        b.extend(8000u32.to_le_bytes());
        b.extend((8000u32 * 2).to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(b"data");
        b.extend(data_len.to_le_bytes());
        for s in samples {
            b.extend(s.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    /// 一段能真听的 WAV：8kHz 单声道 16bit，`secs` 秒的 440Hz 正弦。
    /// 给那条 `#[ignore]` 的真机播放测试用（它要一段**真存在的声音**，不能是几个样本）。
    fn tone_wav(path: &Path, secs: f64) {
        let rate = 8000f64;
        let n = (rate * secs) as usize;
        let mut b = Vec::with_capacity(44 + n * 2);
        b.extend(b"RIFF");
        b.extend(((36 + n * 2) as u32).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend((rate as u32).to_le_bytes());
        b.extend(((rate as u32) * 2).to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(b"data");
        b.extend((n as u32 * 2).to_le_bytes());
        for i in 0..n {
            let v = (i as f64 / rate * 440.0 * std::f64::consts::TAU).sin() * 3000.0;
            b.extend((v as i16).to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("lunac-player-test-{name}"));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn a_decodable_file_yields_its_duration() {
        let p = tmp("ok.wav");
        tiny_wav(&p);
        let (dec, ms) = open_decoder(&p).unwrap();
        // 8 个样本 @ 8kHz = 1ms（取整后的真实值由解码器算，这里只要求它算出来了）
        assert!(ms <= 10, "这段只有 1ms，报出来却是 {ms}ms");
        // rodio 0.22 的 `sample_rate()` / `channels()` 回的是 NonZero（0 非法），取 .get() 比。
        assert_eq!(dec.sample_rate().get(), 8000);
        assert_eq!(dec.channels().get(), 1);
        let _ = std::fs::remove_file(&p);
    }

    /// 取错误文案。**不用 `unwrap_err()`** —— 它要求 `Ok` 那侧实现 `Debug`，
    /// 而 `Decoder` 没有（编译期报错，见本轮）。
    fn err_of(path: &Path) -> String {
        match open_decoder(path) {
            Ok(_) => panic!("这个文件本该开不了：{}", path.display()),
            Err(e) => e,
        }
    }

    #[test]
    fn a_missing_file_says_which_path() {
        let p = tmp("definitely-not-here.wav");
        let e = err_of(&p);
        assert!(e.contains("definitely-not-here.wav"), "{e}");
    }

    #[test]
    fn a_broken_file_fails_instead_of_playing_noise() {
        // 后缀像音频、内容是垃圾 —— 必须报错，而不是当成一段静音播出去
        let p = tmp("broken.mp3");
        std::fs::write(&p, b"this is not audio at all").unwrap();
        let e = err_of(&p);
        assert!(e.contains("broken.mp3"), "{e}");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn volume_is_clamped_and_survives_without_a_session() {
        // 没有会话时调音量：不报错、值要记住（下次起播直接用它）
        let d = player_volume(2.5).unwrap();
        assert_eq!(d.volume, 1.0, "超过 1.0 要夹到 1.0");
        assert!(!d.active);
        let d = player_volume(-1.0).unwrap();
        assert_eq!(d.volume, 0.0);
        let d = player_volume(0.5).unwrap();
        assert_eq!(d.volume, 0.5);
        // 复原，别把默认值留在静态量里影响别的测试
        player_volume(DEFAULT_VOLUME).unwrap();
    }

    #[test]
    fn an_idle_status_is_inactive_but_keeps_the_volume() {
        let d = player_status();
        assert!(!d.active);
        assert!(!d.playing);
        assert_eq!(d.path, "");
        assert_eq!(d.duration_ms, 0);
        assert!(d.volume > 0.0);
        // 没有会话时不该凭空冒出「这是视频」（那会让前端开一块没有源的黑屏）
        assert!(!d.video);
    }

    /// 「要不要出画面」的判据 = 扩展名，且 **表只有一份**（`media_lib::VIDEO_EXTS`）。
    #[test]
    fn video_is_decided_by_the_extension_alone() {
        assert!(is_video_path(Path::new(r"D:\Movies\假期.MP4")));
        assert!(is_video_path(Path::new(r"D:\Movies\clip.mov")));
        assert!(!is_video_path(Path::new(r"D:\Music\song.mp3")));
        assert!(!is_video_path(Path::new(r"D:\Music\no-extension")));
        // 刻意不收的容器（Chromium 不出画面 / 音轨是 Opus）——别再被加回来
        assert!(!is_video_path(Path::new(r"D:\Movies\a.mkv")));
        assert!(!is_video_path(Path::new(r"D:\Movies\a.webm")));
    }

    #[test]
    fn stopping_an_idle_player_is_a_no_op() {
        // 关窗那条路上每个插件窗都会调一次 —— 没在放就必须回 false（调用方不打日志）
        assert!(!stop_if_running());
    }

    /// **真机跑一条真实的音频流**。默认 `#[ignore]` —— 它要一台音频输出设备，
    /// 而在没有声卡的机器 / CI 上会直接失败（那是环境问题，不是代码问题）。
    ///
    /// 跑法：`cargo test --bins real_playback -- --ignored --nocapture`
    ///
    /// 它验的是**只有真机才碰得到的那部分**：设备开得出来、`append` 之后
    /// `empty()` 变假、位置真的在走、暂停之后真的停住、`stop_if_running()`
    /// 能把设备收掉。上面那几条单测只能验纯函数，碰不到这条流
    /// （而本条对应的三条纪律 —— 设备要活着 / `stop()` 一次性 / `empty()` 判活跃 ——
    /// 恰恰都是「错了也不报错」的那种）。
    #[test]
    #[ignore = "需要音频输出设备，按需用 --ignored 跑"]
    fn real_playback_advances_position_and_stops() {
        let p = tmp("real-play.wav");
        tone_wav(&p, 0.6); // 0.6 秒的 440Hz —— 够验「位置在走」，又不至于吵
        let _ = player_volume(0.15); // 小声一点：这条测试会真的出声

        let started = play(&p.display().to_string())
            .expect("开不出音频输出设备（这台机器上有可用的输出设备吗？）");
        assert!(started.active, "刚 append 完就该是 active：{started:?}");
        assert!(started.duration_ms >= 500, "时长该报出来，实得 {}", started.duration_ms);

        std::thread::sleep(Duration::from_millis(250));
        let mid = player_status();
        assert!(mid.position_ms > 0, "位置没在走：{mid:?}");

        // 暂停之后位置**必须停住**（这是「暂停语义」那条纪律的真机判据）
        let paused = player_pause().unwrap();
        assert!(!paused.playing);
        let a = player_status().position_ms;
        std::thread::sleep(Duration::from_millis(200));
        let b = player_status().position_ms;
        assert!(b <= a + 20, "暂停后位置还在走：{a} → {b}");
        assert!(player_resume().unwrap().playing);

        // 收掉：设备一起放掉，`active` 变假
        assert!(stop_if_running(), "有会话在时该回 true");
        assert!(!player_status().active);
        assert!(!stop_if_running(), "收过之后再收就是空操作");

        let _ = std::fs::remove_file(&p);
        player_volume(DEFAULT_VOLUME).unwrap(); // 别把 0.15 留给别的用例
    }

    // ── 调音（2026-10-01 用户第 5 条）──────────────────────────────

    /// 测试用的一条最小 source：一串**交错**样本。
    struct TestSource {
        samples: Vec<Sample>,
        rate: u32,
        ch: u16,
        i: usize,
    }

    impl TestSource {
        /// 一段锯齿（周期 32 样本）—— 频带宽，任何搁架 / 峰值滤波器都改得动它。
        /// 用直流或单一正弦的话，某些档的表现会非常接近直通，判据就钝了。
        ///
        /// ⚠️ **采样率一律用 48000，不要退回 8000**：预设最高摸到 10 kHz
        /// （`loudness` 的 high_shelf），而 8 kHz 流的 Nyquist 只有 4 kHz ⇒
        /// `ChainConfig::from_json` 会**如实拒绝**这条配置（引擎那条纪律：越过 Nyquist
        /// 就报错，不默默算出一条扭曲的曲线），`TuningSource` 于是按直通走 ——
        /// 表现就是「挂了链却一个样本都没变」这个**假失败**（2026-10-01 踩过，
        /// 报错信息里一个字都没提到采样率）。
        fn ramp(n: usize, ch: u16, rate: u32) -> Self {
            let samples = (0..n).map(|k| ((k % 32) as f32 / 32.0) - 0.5).collect();
            Self { samples, rate, ch, i: 0 }
        }
    }

    impl Iterator for TestSource {
        type Item = Sample;
        fn next(&mut self) -> Option<Sample> {
            let v = self.samples.get(self.i).copied();
            self.i += 1;
            v
        }
        fn size_hint(&self) -> (usize, Option<usize>) {
            let left = self.samples.len().saturating_sub(self.i);
            (left, Some(left))
        }
    }

    impl Source for TestSource {
        fn current_span_len(&self) -> Option<usize> {
            Some(self.samples.len())
        }
        fn channels(&self) -> ChannelCount {
            ChannelCount::new(self.ch).unwrap()
        }
        fn sample_rate(&self) -> SampleRate {
            SampleRate::new(self.rate).unwrap()
        }
        fn total_duration(&self) -> Option<Duration> {
            None
        }
    }

    /// 一份**展开好的**配置（P0 起链是「可编辑的段」，不再是光秃秃一个键）。
    fn cfg_of(enabled: bool, preset: &str) -> TuningConfig {
        let (preamp_db, filters) = filters_from_preset(preset).unwrap_or((0.0, Vec::new()));
        TuningConfig::parametric(enabled, preset.to_string(), preamp_db, filters)
    }

    /// 一份**不碰磁盘**的共享状态（`tuning_shared()` 会读 `target\debug\config\`，
    /// 单测不该依赖那个文件，也不该在测试里写它）。
    fn shared_with(enabled: bool, preset: &str) -> Arc<TuningShared> {
        Arc::new(TuningShared {
            version: AtomicU64::new(1),
            cfg: Mutex::new(cfg_of(enabled, preset)),
            last_rate: AtomicU32::new(0),
            last_channels: AtomicU32::new(0),
        })
    }

    #[test]
    fn every_preset_compiles_at_every_common_rate() {
        // 预设表就是下发面 —— 有一条建不起来，用户就会选中一个「亮着但没声」的档
        for (key, _) in TUNING_PRESETS {
            let cfg = cfg_of(true, key);
            for rate in [44100.0, 48000.0, 96000.0] {
                build_chain_from(&cfg, rate)
                    .unwrap_or_else(|e| panic!("预设 {key} 在 {rate}Hz 上建不起来：{e}"));
            }
        }
    }

    #[test]
    fn preset_keys_are_unique_and_shared_with_the_panel() {
        let mut seen = std::collections::HashSet::new();
        for (key, json) in TUNING_PRESETS {
            assert!(!key.is_empty(), "预设键不能是空串");
            assert!(seen.insert(*key), "预设键重复：{key}");
            assert!(!json.is_empty(), "预设 {key} 的配置不能是空串");
        }
        // 面板照 `presets` 画按钮 ⇒ 它必须与这张表同源（少一条 = 界面上永远选不到那一档）
        let dto = tuning_dto(&TuningConfig::default());
        assert_eq!(dto.presets.len(), TUNING_PRESETS.len());
        assert!(dto.presets.iter().any(|k| k == &dto.preset), "默认档必须在名单里");
    }

    #[test]
    fn legacy_config_without_filters_is_migrated_from_its_preset() {
        // P0 之前的配置只有 `{enabled, preset}`。不迁移的话，升级后用户**一直在用的
        // 那条链会静默消失**（界面上还亮着 preset、实际是直通）。
        let legacy: TuningConfig =
            serde_json::from_str(r#"{ "enabled": true, "preset": "vocal" }"#).unwrap();
        assert!(legacy.filters.is_empty(), "前提：老配置反序列化出来确实没有段");
        let m = migrate_tuning(legacy);
        assert!(!m.filters.is_empty(), "必须把预设展开成段");
        assert_eq!(m.preset, "vocal");
        assert!(m.preamp_db < 0.0, "vocal 的 preamp 是负的（防削波），收到 {}", m.preamp_db);

        // 认不出的预设键 ⇒ 回落默认档（与改造前同一行为）
        let junk: TuningConfig = serde_json::from_str(r#"{ "preset": "nope" }"#).unwrap();
        let m2 = migrate_tuning(junk);
        assert_eq!(m2.preset, DEFAULT_PRESET);
        // 认不出的键 ⇒ 回落默认档。**默认档是 `flat`（空链）**（2026-10-03 用户口径
        // 「改成空链条、原声作为初始值」）⇒ 这里断言的是「一段都不加、preamp 为 0」。
        assert_eq!(DEFAULT_PRESET, "flat");
        assert!(m2.filters.is_empty(), "默认档是空链，不该凭空多出段：{:?}", m2.filters);
        assert_eq!(m2.preamp_db, 0.0);

        // **已经有段的配置一个字都不许被改**（哪怕 preset 是 custom）
        let edited: TuningConfig = serde_json::from_str(
            r#"{ "preset": "custom", "preamp_db": -2,
                 "filters": [ { "kind": "peaking", "freq_hz": 100, "gain_db": 3 } ] }"#,
        )
        .unwrap();
        let m3 = migrate_tuning(edited);
        assert_eq!(m3.filters.len(), 1);
        assert_eq!(m3.preamp_db, -2.0);
        assert_eq!(m3.preset, "custom");
    }

    #[test]
    fn disabled_filters_stay_in_the_config_but_never_reach_the_engine() {
        // `on:false` 是「暂时听听看」不是删掉 ⇒ 配置里原样留着；而引擎那边**不许**看到它
        // （引擎的 `Filter` 是 `deny_unknown_fields`，多一个字段就报错）。
        let cfg = cfg_of(true, "vocal");
        assert!(cfg.filters.len() >= 2, "vocal 至少两段，这个用例才有意义");
        let mut with_off = cfg.clone();
        with_off.filters[0].on = false;

        let json = effective_json(&with_off);
        assert!(!json.contains("\"on\""), "on 是面板概念，不能进引擎的 JSON：{json}");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["filters"].as_array().unwrap().len(), cfg.filters.len() - 1);
        // 关一段**真的**改变了链（不是「关了个寂寞」）
        assert_ne!(effective_json(&cfg), json);

        // 全关掉 = 空链 + preamp，照样能建（直通的意义上的合法配置）
        for f in with_off.filters.iter_mut() {
            f.on = false;
        }
        build_chain_from(&with_off, 48000.0).unwrap();
    }

    #[test]
    fn effective_json_neither_invents_nor_drops_fields_the_engine_cares_about() {
        // 一阶滤波器没有 Q、也没有增益：宿主**不许**替它补上（引擎对「不该有的字段」直接报错）
        let one_pole = TuningConfig::parametric(
            true,
            CUSTOM_PRESET.into(),
            0.0,
            vec![TuningFilter {
                kind: "high_pass_1".into(),
                freq_hz: 30.0,
                gain_db: None,
                q: None,
                on: true,
            }],
        );
        let json = effective_json(&one_pole);
        assert!(!json.contains("\"q\""), "{json}");
        assert!(!json.contains("gain_db"), "{json}");
        build_chain_from(&one_pole, 48000.0).unwrap();

        // 反过来：peaking **必须**有 gain_db —— 缺了要由引擎拒掉，这就是
        // 「前端可以下发滤波器数组、合法性由引擎兜底」那条纪律的判据。
        let missing_gain = TuningConfig::parametric(
            true,
            CUSTOM_PRESET.into(),
            0.0,
            vec![TuningFilter {
                kind: "peaking".into(),
                freq_hz: 100.0,
                gain_db: None,
                q: None,
                on: true,
            }],
        );
        let e = build_chain_from(&missing_gain, 48000.0).unwrap_err();
        assert!(e.contains("gain_db"), "错误里要点出缺的是哪个字段：{e}");

        // 越界的段同样要被引擎拦下（前端改坏了参数不该悄悄生效）
        let too_high = TuningConfig::parametric(
            true,
            CUSTOM_PRESET.into(),
            0.0,
            vec![TuningFilter {
                kind: "peaking".into(),
                freq_hz: 24000.0,
                gain_db: Some(3.0),
                q: Some(1.0),
                on: true,
            }],
        );
        assert!(build_chain_from(&too_high, 48000.0).is_err());
    }

    /// 低音 / 高音快捷增益（2026-10-03）：它们**不是引擎字段**，由 `effective_json`
    /// 追加成 low_shelf / high_shelf 两条；0dB 时一条都不该冒出来。
    #[test]
    fn bass_and_treble_shortcuts_become_shelf_filters_only_when_nonzero() {
        let mut cfg = cfg_of(true, "flat");
        assert!(!effective_json(&cfg).contains("shelf"), "0dB 时不该凭空多出 shelf");

        cfg.bass_db = 6.0;
        cfg.treble_db = -3.0;
        let json = effective_json(&cfg);
        assert!(json.contains("\"low_shelf\""), "{json}");
        assert!(json.contains("\"high_shelf\""), "{json}");

        // 追加出来的两条必须真的过得了引擎那层校验，而且曲线要算得进去
        let chain = build_chain_from(&cfg, 48000.0).unwrap();
        let low = chain.response_db(30.0, 48000.0);
        let high = chain.response_db(15000.0, 48000.0);
        assert!(low > 5.0, "低音搁架没生效：{low}");
        assert!(high < -2.0, "高音搁架没生效：{high}");
    }

    /// 五项新效果器（GraphicEQ / 延迟 / 复制 / 卷积 / If-Else）要**原样透传**给引擎，
    /// 且**合法性一律由引擎兜底** —— 宿主这一层既不摆形状、也不吞错。
    #[test]
    fn effective_json_passes_the_new_effect_blocks_through_to_the_engine() {
        let mut cfg = cfg_of(true, "flat");
        cfg.graphic_eq = vec![serde_json::json!({ "freq_hz": 1000, "gain_db": 3 })];
        cfg.delay_ms = 1.5;
        cfg.channel_copy = vec![TuningCopy { from: 0, to: 1 }];
        cfg.if_else = vec![TuningCondBlock {
            channel: 0,
            then: vec![TuningFilter {
                kind: "peaking".into(),
                freq_hz: 2000.0,
                gain_db: Some(4.0),
                q: None,
                on: true,
            }],
            else_filters: Vec::new(),
        }];
        let json = effective_json(&cfg);
        for k in ["graphic_eq", "delay_ms", "channel_copy", "if_else"] {
            assert!(json.contains(k), "缺了 {k}：{json}");
        }
        // 没配卷积就不该出现这个字段（引擎对缺省字段有语义）
        assert!(!json.contains("convolution"), "{json}");
        // 通道段下发的是**引擎形态**（没有面板用的 `on`）
        assert!(!json.contains("\"on\""), "引擎不认识 on：{json}");
        // 能被引擎接受 ⇒ 形状是对的
        build_chain_from(&cfg, 48000.0).unwrap();

        // 坏段（增益越界）必须被**引擎**拒掉 —— 类型这一层只管形状，量纲一律归引擎
        let mut bad = cfg.clone();
        bad.if_else[0].then[0].gain_db = Some(999.0);
        assert!(build_chain_from(&bad, 48000.0).is_err());
    }

    /// 通道槽的边界（P5-2）：把某条通道的段**全部关掉**（`on = false`）不该让整条链编不出来
    /// —— 那个块会被 `effective_json` 整个丢掉（引擎对「then 与 else 都空」是直接报错的）。
    #[test]
    fn a_fully_disabled_channel_slot_is_dropped_instead_of_breaking_the_chain() {
        let mut cfg = cfg_of(true, "flat");
        cfg.if_else = vec![TuningCondBlock {
            channel: 0,
            then: vec![TuningFilter {
                kind: "peaking".into(),
                freq_hz: 1000.0,
                gain_db: Some(3.0),
                q: None,
                on: false, // 面板上被关掉
            }],
            else_filters: Vec::new(),
        }];
        let json = effective_json(&cfg);
        assert!(!json.contains("if_else"), "空的通道段不该下发：{json}");
        build_chain_from(&cfg, 48000.0).unwrap();
    }

    /// `for_channels`（P5-2 / P5-4）：越界的通道段**与声道复制**按**当前流**裁掉，
    /// 而不是让整条链报错 —— 两样在引擎里都会让 `ChainRuntime::new` 报错，只裁一样不够。
    #[test]
    fn for_channels_drops_slots_and_copies_the_stream_does_not_have() {
        let mut cfg = cfg_of(true, "flat");
        cfg.if_else = vec![
            TuningCondBlock { channel: 0, then: Vec::new(), else_filters: Vec::new() },
            TuningCondBlock { channel: 5, then: Vec::new(), else_filters: Vec::new() },
        ];
        cfg.channel_copy = vec![
            TuningCopy { from: 0, to: 1 }, // 立体声里合法
            TuningCopy { from: 0, to: 5 }, // `to` 越界
            TuningCopy { from: 5, to: 0 }, // `from` 越界
        ];
        // 立体声：通道 5 那段 + 两条越界复制被裁掉，剩下的原样保留
        let stereo = cfg.for_channels(2);
        assert_eq!(stereo.if_else.len(), 1);
        assert_eq!(stereo.if_else[0].channel, 0);
        assert_eq!(stereo.channel_copy, vec![TuningCopy { from: 0, to: 1 }]);
        // 全都在范围内 ⇒ 一个都不动（不无谓地克隆 / 打日志）
        assert_eq!(cfg.for_channels(6).if_else.len(), 2);
        assert_eq!(cfg.for_channels(6).channel_copy.len(), 3);
    }

    // ── 用户预设（P5-1，2026-10-03）────────────────────────────────

    #[test]
    fn preset_file_name_sanitizes_and_avoids_windows_reserved_names() {
        assert_eq!(preset_file_name("我的低音").as_deref(), Some("preset-我的低音.json"));
        // 路径分隔符与 Windows 禁用字符换成 `_`
        assert_eq!(preset_file_name("a/b:c*d").as_deref(), Some("preset-a_b_c_d.json"));
        // 保留名靠 `preset-` 前缀躲开（`CON.json` 在 Windows 上仍是保留名）
        assert_eq!(preset_file_name("CON").as_deref(), Some("preset-CON.json"));
        // 净化后空的（只有空白 / 点）⇒ 这个名字落不成文件
        assert_eq!(preset_file_name("   "), None);
        assert_eq!(preset_file_name("..."), None);
    }

    #[test]
    fn user_presets_round_trip_in_a_temp_dir() {
        let dir = std::env::temp_dir().join(format!("lunac-tune-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut cfg = cfg_of(true, "flat");
        cfg.preamp_db = -3.5;
        cfg.delay_ms = 1.5;
        let p = TuningPreset::snapshot(&cfg, "我的低音");
        save_user_preset(&dir, &p, false).unwrap();

        // 列表里给的是**显示名**（不是文件名）
        let names: Vec<String> = list_user_presets(&dir).into_iter().map(|x| x.name).collect();
        assert_eq!(names, vec!["我的低音"]);
        // 同名再存且不覆盖 ⇒ `ERR_PRESET_EXISTS`（前端据此让用户确认）
        assert!(save_user_preset(&dir, &p, false)
            .unwrap_err()
            .contains("ERR_PRESET_EXISTS"));
        save_user_preset(&dir, &p, true).unwrap(); // 覆盖能过
        let back = find_user_preset(&dir, "我的低音").unwrap();
        assert_eq!(back.preamp_db, -3.5);
        assert_eq!(back.delay_ms, 1.5);

        // 重命名：旧名字找不到、新名字找得到，且**不能留下两个文件**
        rename_user_preset(&dir, "我的低音", "新名字").unwrap();
        assert!(find_user_preset(&dir, "我的低音").is_none());
        assert!(find_user_preset(&dir, "新名字").is_some());
        assert_eq!(list_user_presets(&dir).len(), 1, "重命名留下了多余文件");

        delete_user_preset(&dir, "新名字").unwrap();
        assert!(list_user_presets(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 「存 → 取」路上**一个字段都不许丢**（五项新效果器尤其容易在这条路上被漏掉）。
    #[test]
    fn a_preset_snapshot_keeps_every_chain_field() {
        let mut cfg = cfg_of(true, "flat");
        cfg.preamp_db = -2.0;
        cfg.graphic_eq = vec![serde_json::json!({ "freq_hz": 1000, "gain_db": 3 })];
        cfg.delay_ms = 2.5;
        cfg.channel_copy = vec![TuningCopy { from: 0, to: 1 }];
        cfg.convolution = Some("ir/x.wav".into());
        cfg.if_else = vec![TuningCondBlock {
            channel: 1,
            then: vec![TuningFilter {
                kind: "high_shelf".into(),
                freq_hz: 6000.0,
                gain_db: Some(-2.0),
                q: None,
                on: true,
            }],
            else_filters: Vec::new(),
        }];
        let p = TuningPreset::snapshot(&cfg, "全");

        let mut back = cfg_of(false, "flat");
        p.apply_to(&mut back);
        assert_eq!(back.preamp_db, -2.0);
        assert_eq!(back.filters, cfg.filters);
        assert_eq!(back.graphic_eq, cfg.graphic_eq);
        assert_eq!(back.delay_ms, 2.5);
        assert_eq!(back.channel_copy, cfg.channel_copy);
        assert_eq!(back.convolution, Some("ir/x.wav".into()));
        assert_eq!(back.if_else, cfg.if_else);
        assert_eq!(back.preset, "全");
        // `enabled` 是**调用方**的事 —— `apply_to` 不许动它
        assert!(!back.enabled);
    }

    /// 卷积的 IR 路径：相对路径要锚到 **Lunac 根目录** —— 进程 cwd 不一定是安装目录
    /// （dev 下是 `target\debug`），直接丢给引擎会「同一份配置时灵时不灵」。
    #[test]
    fn a_relative_ir_path_is_anchored_at_the_lunac_root() {
        let mut cfg = cfg_of(true, "flat");
        cfg.convolution = Some("ir/room.wav".into());
        let json = effective_json(&cfg);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let p = v.get("convolution").and_then(|x| x.as_str()).unwrap();
        assert!(Path::new(p).is_absolute(), "相对路径必须被锚到 Lunac 根：{p}");
        assert!(p.ends_with("room.wav"), "{p}");
    }

    #[test]
    fn the_response_curve_is_log_spaced_and_really_reflects_the_chain() {
        let dto = tuning_dto(&cfg_of(true, "bass"));
        let r = &dto.response;
        assert_eq!(r.freqs.len(), RESPONSE_POINTS);
        assert_eq!(r.db.len(), RESPONSE_POINTS);
        assert!(r.sample_rate >= 8000.0);
        // 两端
        assert!((19.9..=20.1).contains(&r.freqs[0]), "起点是 {}Hz", r.freqs[0]);
        assert!(r.freqs[RESPONSE_POINTS - 1] >= 19_999.0);
        // 对数等分 ⇒ 相邻比值恒定（这条钉的是「别换成线性等分」：
        // 线性画出来低频那一半会被压成一根线，用户没法在低频调音）
        let ratio = r.freqs[1] / r.freqs[0];
        for w in r.freqs.windows(2) {
            assert!((w[1] / w[0] - ratio).abs() < 1e-9, "不是对数等分：{} → {}", w[0], w[1]);
        }
        // 曲线必须**真的是这条链**。bass = low_shelf(+6dB @120) + high_shelf(−2dB @9k)
        // + preamp(−6) ⇒ 低频端被 preamp 抵到 **0dB 附近**（+6 − 6），高端 ≈ −8dB。
        // 判据取「两端差 ≥4dB」而不是「20Hz 必须 >2dB」—— 后者把 preamp 忘掉了，
        // 而 preamp 正是这条链的余量控制（见 chain.rs 文件头那段）。
        // 少了这条断言，「曲线恒为平线」也能过。
        let low = r.db[0];
        let high = r.db[RESPONSE_POINTS - 1];
        assert!(low - high > 4.0, "低端 {low}dB / 高端 {high}dB —— 曲线没接上链");
        assert!((-1.0..=1.0).contains(&low), "低端应被 preamp 抵到 0dB 附近，收到 {low}dB");
        assert!(r.db.iter().all(|d| d.is_finite()));
    }

    #[test]
    fn an_unknown_preset_is_rejected_before_touching_the_disk() {
        // 这条必须在碰磁盘**之前**失败：一次笔误不该把用户的配置文件写坏
        let e = player_tuning_set(true, "nope".to_string()).unwrap_err();
        assert!(e.contains("ERR_BAD_PRESET"), "{e}");
        assert!(e.contains("nope"), "错误里要点出是哪个键：{e}");
    }

    #[test]
    fn disabled_tuning_is_bit_exact_passthrough() {
        let out: Vec<Sample> =
            TuningSource::new(TestSource::ramp(64, 1, 48000), shared_with(false, "bass")).collect();
        let raw: Vec<Sample> = TestSource::ramp(64, 1, 48000).collect();
        assert_eq!(out, raw, "关着的时候必须逐位原样透传");
    }

    #[test]
    fn the_flat_preset_stays_a_passthrough_even_when_enabled() {
        // 「开着 + flat」是面板上做 A/B 的参照物：它要是悄悄改了一点点，
        // 用户拿它对比就永远分不清「听到的差别是不是开关本身造成的」
        let out: Vec<Sample> =
            TuningSource::new(TestSource::ramp(64, 1, 48000), shared_with(true, "flat")).collect();
        let raw: Vec<Sample> = TestSource::ramp(64, 1, 48000).collect();
        assert_eq!(out, raw);
    }

    #[test]
    fn enabling_a_preset_changes_the_samples() {
        let out: Vec<Sample> =
            TuningSource::new(TestSource::ramp(256, 1, 48000), shared_with(true, "bass")).collect();
        let raw: Vec<Sample> = TestSource::ramp(256, 1, 48000).collect();
        assert_eq!(out.len(), raw.len(), "链不能改变样本个数");
        assert!(
            out.iter().zip(&raw).any(|(a, b)| (a - b).abs() > 1e-4),
            "挂了链却一个样本都没变 —— 链根本没接上"
        );
    }

    #[test]
    fn a_version_bump_is_picked_up_without_restarting_the_source() {
        // 「改档不打断正在放的这一首」的判据：同一条 source，前一半是直通，
        // 中途改配置并自增版本号，后一半必须换上新链
        let shared = shared_with(false, "bass");
        let mut src = TuningSource::new(TestSource::ramp(256, 1, 48000), shared.clone());
        let head: Vec<Sample> = (0..128).map(|_| src.next().unwrap()).collect();
        {
            *shared.cfg.lock().unwrap() = cfg_of(true, "bass");
            shared.version.fetch_add(1, Ordering::SeqCst);
        }
        let tail: Vec<Sample> = (0..128).map(|_| src.next().unwrap()).collect();

        let raw: Vec<Sample> = TestSource::ramp(256, 1, 48000).collect();
        assert_eq!(head, raw[..128].to_vec(), "改之前必须是直通");
        assert!(
            tail.iter().zip(&raw[128..]).any(|(a, b)| (a - b).abs() > 1e-4),
            "版本号涨了却没换链 —— 面板改了档、听感上什么都没发生"
        );
    }

    #[test]
    fn each_channel_keeps_its_own_state() {
        // 左右两声道喂**不同的信号**：共享状态时左边先响会把右边的输出带偏，
        // 而这种错在单声道素材上完全看不出来（引擎侧那条单测是同一条纪律）。
        let left: Vec<Sample> = TestSource::ramp(256, 1, 48000).collect();
        let right: Vec<Sample> = left.iter().map(|v| -v * 0.5).collect();
        let mut interleaved = Vec::with_capacity(512);
        for i in 0..256 {
            interleaved.push(left[i]);
            interleaved.push(right[i]);
        }

        let shared = shared_with(true, "bass");
        let stereo: Vec<Sample> = TuningSource::new(
            TestSource { samples: interleaved, rate: 48000, ch: 2, i: 0 },
            shared.clone(),
        )
        .collect();

        for (ch, mono_in) in [(0usize, left), (1usize, right)] {
            let mono: Vec<Sample> = TuningSource::new(
                TestSource { samples: mono_in, rate: 48000, ch: 1, i: 0 },
                shared.clone(),
            )
            .collect();
            for i in 0..256 {
                let got = stereo[i * 2 + ch];
                assert!(
                    (got - mono[i]).abs() < 1e-6,
                    "第 {i} 个样本的第 {ch} 声道是 {got}，单独渲染它是 {}",
                    mono[i]
                );
            }
        }
    }
}
