//! 调音控制面（重建 v1 `app/src-tauri/src/player.rs` 的 tuning 部分 + 预设 / A-B 快照）。
//!
//! **与 v1 的关系**：v1 的调音状态机全部写在 `player.rs` 里（与 rodio 的播放源同文件），
//! 本模块把**控制面**原样搬过来：配置（`<exe 根>\config\tuning.json`）、内置预设、
//! 用户预设（`config\tuning\presets\preset-*.json`）、A/B 盲测快照、频响曲线 DTO。
//! **DSP 内核一行都不重写** —— 直接依赖仓库里那棵 `tuning-engine/`（v1 也是库依赖）。
//!
//! 本模块**不含播放**：出声链路（rodio）是下一步的 `player`；这里只保证「链能编出来、
//! 配置能存、曲线能算」。`build_chain_from` 就是那条唯一的合法性入口
//! （`ChainConfig::from_json` 的 `deny_unknown_fields` + 逐段校验兜底）。
//!
//! 口径与 v1 逐条一致：
//!   · 默认预设 = `flat`（2026-10-03 用户口径：不主动调就什么都不加）；
//!   · 默认**关闭**（DSP 改声音，必须用户主动打开）；
//!   · `on=false` 的段保留在配置里、下发前摘掉（关/开无损往返）；
//!   · 低音 / 高音是**快捷增益**，由 `effective_json` 追加成 low_shelf / high_shelf；
//!   · 校验一律用 **44.1kHz**（Nyquist 最窄的常用档，过了就等于 48k / 96k 都过）；
//!   · 写顺序 = 先校验 → 再落盘 → 最后改内存并自增版本号（失败不会留下「界面已生效、
//!     重启就没了」的分裂状态）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tuning_engine::chain::Chain;
use tuning_engine::config::ChainConfig;

/// 校验用的采样率（v1 同口径）。
const VALIDATE_RATE: f64 = 44_100.0;
/// 频响曲线采样点数（20Hz–20kHz 对数等分）。
const RESPONSE_POINTS: usize = 240;
/// 默认预设键。
const DEFAULT_PRESET: &str = "flat";
/// 用户手改过链之后 `preset` 的取值。
const CUSTOM_PRESET: &str = "custom";
const BASS_SHELF_HZ: f64 = 120.0;
const TREBLE_SHELF_HZ: f64 = 9_000.0;
/// 用户预设文件名前缀（躲开 Windows 保留名 `CON` / `NUL` 之类）。
const PRESET_FILE_PREFIX: &str = "preset-";

/// 内置预设表（`preamp_db` = 该链最大正向增益的相反数 ⇒ 不削波）。
/// 与 v1 `TUNING_PRESETS` **逐字节一致**。
const TUNING_PRESETS: &[(&str, &str)] = &[
    ("flat", r#"{ "preamp_db": 0 }"#),
    (
        "bass",
        r#"{ "preamp_db": -6, "filters": [
            { "kind": "low_shelf", "freq_hz": 120, "gain_db": 6 },
            { "kind": "high_shelf", "freq_hz": 9000, "gain_db": -2 } ] }"#,
    ),
    (
        "vocal",
        r#"{ "preamp_db": -4, "filters": [
            { "kind": "peaking", "freq_hz": 2200, "gain_db": 4, "q": 1.2 },
            { "kind": "peaking", "freq_hz": 350, "gain_db": -3, "q": 1.4 } ] }"#,
    ),
    (
        "treble",
        r#"{ "preamp_db": -5, "filters": [
            { "kind": "high_shelf", "freq_hz": 9000, "gain_db": 5 } ] }"#,
    ),
    (
        "loudness",
        r#"{ "preamp_db": -6, "filters": [
            { "kind": "low_shelf", "freq_hz": 100, "gain_db": 6 },
            { "kind": "high_shelf", "freq_hz": 10000, "gain_db": 4 } ] }"#,
    ),
];

fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

fn yes() -> bool {
    true
}

fn default_preset() -> String {
    DEFAULT_PRESET.to_string()
}

/// 面板可编辑的一段滤波器（`on=false` 的段留着、不参与建链）。
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

/// 一条通道段（引擎 `if_else`）：`channel` 命中跑 `then`，否则跑 `else`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TuningCondBlock {
    pub channel: usize,
    #[serde(default)]
    pub then: Vec<TuningFilter>,
    #[serde(default, rename = "else")]
    pub else_filters: Vec<TuningFilter>,
}

/// 一条声道复制（引擎 `channel_copy`）：处理后把 `from` 的快照抄进 `to`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TuningCopy {
    pub from: usize,
    pub to: usize,
}

/// 落盘形态（`<exe 根>\config\tuning.json`，与 v1 **同字段名**，换回 v1 不丢设置）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuningConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_preset")]
    pub preset: String,
    #[serde(default)]
    pub preamp_db: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bass_db: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub treble_db: f64,
    #[serde(default)]
    pub filters: Vec<TuningFilter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphic_eq: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub delay_ms: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channel_copy: Vec<TuningCopy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convolution: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub if_else: Vec<TuningCondBlock>,
}

impl TuningConfig {
    /// 只有参数段（preamp + filters）的配置，五项新效果器留空。
    fn parametric(enabled: bool, preset: String, preamp_db: f64, filters: Vec<TuningFilter>) -> Self {
        Self {
            enabled,
            preset,
            preamp_db,
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

    /// 用一条预设**整条**造配置。
    fn from_preset(preset: &TuningPreset, enabled: bool) -> Self {
        let mut cfg = Self::parametric(enabled, String::new(), 0.0, Vec::new());
        preset.apply_to(&mut cfg);
        cfg
    }
}

impl Default for TuningConfig {
    fn default() -> Self {
        default_tuning_config()
    }
}

/// 一条预设（= 一整条链的快照）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TuningPreset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    preamp_db: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    bass_db: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    treble_db: f64,
    #[serde(default)]
    filters: Vec<TuningFilter>,
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

    /// 整条盖到配置上（`enabled` 不动）。
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

// ── 路径 ──────────────────────────────────────────────────────────────

fn config_path(root: &Path) -> PathBuf {
    root.join("config").join("tuning.json")
}

fn presets_dir(root: &Path) -> PathBuf {
    root.join("config").join("tuning").join("presets")
}

fn ab_slot_path(root: &Path, slot: &str) -> Option<PathBuf> {
    let s = slot.trim().to_ascii_lowercase();
    if s != "a" && s != "b" {
        return None;
    }
    Some(
        root.join("config")
            .join("tuning")
            .join(format!("ab-{s}.json")),
    )
}

// ── 共享状态（版本号 / 最近采样率 / 声道数）─────────────────────────

/// 播放源与命令之间唯一的共享点（v1 `TuningShared` 同形）。
pub struct TuningShared {
    /// 配置版本号：播放源每个样本原子读一次，变了才重建链（音频线程不许拿锁）。
    pub version: AtomicU64,
    cfg: Mutex<TuningConfig>,
    /// 最近一次建链的采样率（Hz，0 = 还没放过东西）。
    pub last_rate: AtomicU32,
    /// 最近一次建链那条流的声道数（0 = 还不知道）。
    pub last_channels: AtomicU32,
}

static SHARED: OnceLock<(PathBuf, Arc<TuningShared>)> = OnceLock::new();

impl TuningShared {
    /// 当前配置的快照（拿锁 → clone → 立刻放开；不要在临界区里做别的事）。
    pub fn snapshot(&self) -> TuningConfig {
        match self.cfg.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => TuningConfig::default(),
        }
    }

    /// 记下「这条流的采样率 / 声道数」（播放源在换链那一刻调）。
    pub fn note_stream(&self, rate_hz: u32, channels: u32) {
        self.last_rate.store(rate_hz, Ordering::Relaxed);
        self.last_channels.store(channels, Ordering::Relaxed);
    }
}

/// 裁掉**超出这条流的声道数**的通道段与声道复制（v1 `TuningConfig::for_channels`）。
///
/// 越界的段在运行期会被 `ChainRuntime::new` 报错 ⇒ 整条链（含全局 EQ）一起作废，
/// 听起来是「调音突然全没了」。这里按当前流裁一刀，配置本身不动。
pub fn for_channels(cfg: &TuningConfig, channels: usize) -> TuningConfig {
    let bad_cond: Vec<usize> = cfg
        .if_else
        .iter()
        .filter(|block| block.channel >= channels)
        .map(|block| block.channel)
        .collect();
    let bad_copy: Vec<(usize, usize)> = cfg
        .channel_copy
        .iter()
        .filter(|copy| copy.from >= channels || copy.to >= channels)
        .map(|copy| (copy.from, copy.to))
        .collect();
    if bad_cond.is_empty() && bad_copy.is_empty() {
        return cfg.clone();
    }
    eprintln!(
        "[lunac-tuning] 通道段 {bad_cond:?} / 声道复制 {bad_copy:?} 超出这条流的 {channels} 声道，本次渲染跳过它们"
    );
    let mut trimmed = cfg.clone();
    trimmed.if_else.retain(|block| block.channel < channels);
    trimmed
        .channel_copy
        .retain(|copy| copy.from < channels && copy.to < channels);
    trimmed
}

/// 播放源取进程内那份调音状态（含版本号 / 最近采样率）。
pub(crate) fn shared_handle(root: &Path) -> Arc<TuningShared> {
    shared(root)
}

/// 取进程内唯一的那份调音状态（首次访问读盘）。`root` 只在第一次生效。
fn shared(root: &Path) -> Arc<TuningShared> {
    SHARED
        .get_or_init(|| {
            let cfg = load_config(root);
            (
                root.to_path_buf(),
                Arc::new(TuningShared {
                    version: AtomicU64::new(1),
                    cfg: Mutex::new(cfg),
                    last_rate: AtomicU32::new(0),
                    last_channels: AtomicU32::new(0),
                }),
            )
        })
        .1
        .clone()
}

/// 共享状态初始化时用的根目录（预设 / A-B 槽按它算路径）。
fn shared_root(root: &Path) -> PathBuf {
    let _ = shared(root);
    SHARED.get().map(|(path, _)| path.clone()).unwrap_or_else(|| root.to_path_buf())
}

fn load_config(root: &Path) -> TuningConfig {
    let Ok(text) = std::fs::read_to_string(config_path(root)) else {
        return default_tuning_config();
    };
    let cfg: TuningConfig = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .unwrap_or_else(|_| default_tuning_config());
    migrate_tuning(cfg)
}

fn save_config(root: &Path, cfg: &TuningConfig) -> Result<(), String> {
    let path = config_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建 config 目录失败：{e}"))?;
    }
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("写 {} 失败：{e}", path.display()))
}

fn current(root: &Path) -> Result<TuningConfig, String> {
    let shared = shared(root);
    let cfg = shared.cfg.lock().map_err(|e| e.to_string())?.clone();
    Ok(cfg)
}

/// 「先校验、再落盘、最后改内存与版本号」的唯一落点（v1 `apply_tuning_config`）。
fn apply(root: &Path, cfg: TuningConfig) -> Result<serde_json::Value, String> {
    save_config(root, &cfg)?;
    let shared = shared(root);
    {
        let mut guard = shared.cfg.lock().map_err(|e| e.to_string())?;
        *guard = cfg;
    }
    shared.version.fetch_add(1, Ordering::SeqCst);
    Ok(dto(root))
}

// ── 预设表 / 迁移 / 引擎 JSON ────────────────────────────────────────

fn preset_json(key: &str) -> Option<&'static str> {
    TUNING_PRESETS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, json)| *json)
}

/// 把内置预设展开成可编辑的段。
fn filters_from_preset(key: &str) -> Option<(f64, Vec<TuningFilter>)> {
    let json = preset_json(key)?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let preamp_db = value.get("preamp_db").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let mut out = Vec::new();
    for filter in value
        .get("filters")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default()
    {
        out.push(TuningFilter {
            kind: filter.get("kind")?.as_str()?.to_string(),
            freq_hz: filter.get("freq_hz")?.as_f64()?,
            gain_db: filter.get("gain_db").and_then(|x| x.as_f64()),
            q: filter.get("q").and_then(|x| x.as_f64()),
            on: true,
        });
    }
    Some((preamp_db, out))
}

/// 全新安装的配置：`enabled=false` + 默认预设**展开好**的链。
fn default_tuning_config() -> TuningConfig {
    let (preamp_db, filters) = filters_from_preset(DEFAULT_PRESET).unwrap_or((0.0, Vec::new()));
    TuningConfig::parametric(false, DEFAULT_PRESET.to_string(), preamp_db, filters)
}

/// 老配置（只有 `{enabled, preset}`）升级迁移：把预设展开成段。
///
/// 不迁移的话，升级后用户一直在用的那条链会**静默消失**（界面亮着 preset、实际直通）。
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
        _ => default_tuning_config(),
    }
}

/// 把面板那层滤波器编成引擎认识的那份 JSON（只发 `on` 的段、不替引擎补字段）。
fn engine_filters(filters: &[TuningFilter]) -> Vec<serde_json::Value> {
    filters
        .iter()
        .filter(|f| f.on)
        .map(|f| {
            let mut out = serde_json::Map::new();
            out.insert("kind".into(), serde_json::json!(f.kind));
            out.insert("freq_hz".into(), serde_json::json!(f.freq_hz));
            if let Some(gain) = f.gain_db {
                out.insert("gain_db".into(), serde_json::json!(gain));
            }
            if let Some(q) = f.q {
                out.insert("q".into(), serde_json::json!(q));
            }
            serde_json::Value::Object(out)
        })
        .collect()
}

/// 当前链 → 引擎 JSON（低音 / 高音追加成两条 shelf；相对 IR 路径按根目录展开）。
fn effective_json(root: &Path, cfg: &TuningConfig) -> String {
    let mut filters = engine_filters(&cfg.filters);
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
    let mut out = serde_json::Map::new();
    out.insert("preamp_db".into(), serde_json::json!(cfg.preamp_db));
    out.insert("filters".into(), serde_json::Value::Array(filters));
    if !cfg.graphic_eq.is_empty() {
        out.insert(
            "graphic_eq".into(),
            serde_json::Value::Array(cfg.graphic_eq.clone()),
        );
    }
    if cfg.delay_ms != 0.0 {
        out.insert("delay_ms".into(), serde_json::json!(cfg.delay_ms));
    }
    if !cfg.channel_copy.is_empty() {
        out.insert(
            "channel_copy".into(),
            serde_json::json!(cfg.channel_copy),
        );
    }
    if let Some(ir) = &cfg.convolution {
        let path = PathBuf::from(ir);
        let full = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        out.insert(
            "convolution".into(),
            serde_json::json!(full.to_string_lossy()),
        );
    }
    let cond: Vec<serde_json::Value> = cfg
        .if_else
        .iter()
        .filter_map(|block| {
            let then = engine_filters(&block.then);
            let els = engine_filters(&block.else_filters);
            if then.is_empty() && els.is_empty() {
                return None;
            }
            let mut item = serde_json::Map::new();
            item.insert("channel".into(), serde_json::json!(block.channel));
            item.insert("then".into(), serde_json::Value::Array(then));
            if !els.is_empty() {
                item.insert("else".into(), serde_json::Value::Array(els));
            }
            Some(serde_json::Value::Object(item))
        })
        .collect();
    if !cond.is_empty() {
        out.insert("if_else".into(), serde_json::Value::Array(cond));
    }
    serde_json::Value::Object(out).to_string()
}

/// 编译当前配置（**校验的唯一入口**）。
pub fn build_chain(root: &Path, cfg: &TuningConfig, sample_rate: f64) -> Result<Chain, String> {
    let json = effective_json(root, cfg);
    let parsed = ChainConfig::from_json(&json, sample_rate)?;
    Chain::build(&parsed, sample_rate)
}

// ── DTO ───────────────────────────────────────────────────────────────

/// 在**给定频点**上算当前链的合成响应（dB）。
///
/// 实时测量（`tuning_probe.rs`）用它把「实测」与「解析式」画在同一条频点上 ——
/// 两边各算一套频点会看不出该对齐哪儿。建不起链时给平线（与 `response_of` 同口径）。
pub fn reference_db(root: &Path, freqs: &[f64], rate: f64) -> Vec<f64> {
    let cfg = match shared(root).cfg.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => TuningConfig::default(),
    };
    match build_chain(root, &cfg, rate) {
        Ok(chain) => freqs.iter().map(|f| chain.response_db(*f, rate)).collect(),
        Err(_) => vec![0.0; freqs.len()],
    }
}

fn log_spaced_freqs(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    let (l0, l1) = (lo.ln(), hi.ln());
    (0..n)
        .map(|i| (l0 + (l1 - l0) * i as f64 / (n - 1) as f64).exp())
        .collect()
}

/// 一条链的频响（20Hz–20kHz，240 点）。
fn response_of(root: &Path, cfg: &TuningConfig, rate: f64) -> (Vec<f64>, Vec<f64>, Vec<Vec<f64>>) {
    let freqs = log_spaced_freqs(20.0, 20_000.0, RESPONSE_POINTS);
    match build_chain(root, cfg, rate) {
        Ok(chain) => {
            let db = freqs.iter().map(|f| chain.response_db(*f, rate)).collect();
            let cond_db = cfg
                .if_else
                .iter()
                .map(|block| {
                    freqs
                        .iter()
                        .map(|f| chain.response_db_for_channel(*f, rate, block.channel))
                        .collect()
                })
                .collect();
            (freqs, db, cond_db)
        }
        // 建不起链 ⇒ 画一条平线（`set` 那条路已经如实报过错）
        Err(_) => (freqs.clone(), vec![0.0; freqs.len()], Vec::new()),
    }
}

/// 面板可见的调音状态（字段名用 camelCase，与 v2 其它 DTO 一致）。
fn dto(root: &Path) -> serde_json::Value {
    let shared = shared(root);
    let cfg = match shared.cfg.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => TuningConfig::default(),
    };
    let rate = {
        let r = shared.last_rate.load(Ordering::Relaxed);
        if r == 0 {
            48_000.0
        } else {
            r as f64
        }
    };
    let (freqs, db, cond_db) = response_of(root, &cfg, rate);
    let kinds: Vec<serde_json::Value> = tuning_engine::biquad::Kind::ALL
        .iter()
        .map(|kind| {
            serde_json::json!({
                "name": kind.name(),
                "gain": kind.uses_gain(),
                "order": kind.order(),
            })
        })
        .collect();
    serde_json::json!({
        "enabled": cfg.enabled,
        "preset": cfg.preset,
        "presets": TUNING_PRESETS.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
        "userPresets": list_user_presets(&presets_dir(root)).into_iter().map(|p| p.name).collect::<Vec<_>>(),
        "kinds": kinds,
        "preampDb": cfg.preamp_db,
        "bassDb": cfg.bass_db,
        "trebleDb": cfg.treble_db,
        "filters": cfg.filters,
        "graphicEq": cfg.graphic_eq,
        "delayMs": cfg.delay_ms,
        "channelCopy": cfg.channel_copy,
        "convolution": cfg.convolution,
        "ifElse": cfg.if_else,
        "channels": shared.last_channels.load(Ordering::Relaxed) as usize,
        "condDb": cond_db,
        "response": { "sampleRate": rate, "freqs": freqs, "db": db },
    })
}

// ── 用户预设的读写 ────────────────────────────────────────────────────

fn preset_file_name(name: &str) -> Option<String> {
    let mut cleaned = String::new();
    for ch in name.trim().chars() {
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

fn read_preset_file(path: &Path) -> Result<TuningPreset, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("读不了：{e}"))?;
    let mut preset: TuningPreset = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("不是合法的预设 JSON：{e}"))?;
    if preset.name.trim().is_empty() {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        preset.name = stem
            .strip_prefix(PRESET_FILE_PREFIX)
            .unwrap_or(stem)
            .to_string();
    }
    if preset.name.trim().is_empty() {
        return Err("预设没有名字".to_string());
    }
    Ok(preset)
}

fn list_user_presets(dir: &Path) -> Vec<TuningPreset> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        match read_preset_file(&path) {
            Ok(preset) => out.push(preset),
            Err(e) => eprintln!("[lunac-tuning] 预设 {} 读不了，已跳过（{e}）", path.display()),
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn find_user_preset_path(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Ok(preset) = read_preset_file(&path) {
            if preset.name == name {
                return Some(path);
            }
        }
    }
    None
}

fn find_user_preset(dir: &Path, name: &str) -> Option<TuningPreset> {
    find_user_preset_path(dir, name).and_then(|path| read_preset_file(&path).ok())
}

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

fn rename_user_preset(dir: &Path, from: &str, to: &str) -> Result<(), String> {
    let old = find_user_preset_path(dir, from).ok_or_else(|| format!("没有叫「{from}」的预设"))?;
    if find_user_preset_path(dir, to).is_some() {
        return Err(format!("ERR_PRESET_EXISTS：已经有一个叫「{to}」的预设"));
    }
    let mut preset = read_preset_file(&old)?;
    preset.name = to.to_string();
    save_user_preset(dir, &preset, true)?;
    if let Some(new_path) = find_user_preset_path(dir, to) {
        if new_path != old {
            let _ = std::fs::remove_file(&old);
        }
    }
    Ok(())
}

/// 内置优先，其次用户预设。
fn resolve_preset(root: &Path, name: &str) -> Option<TuningPreset> {
    if let Some((preamp_db, filters)) = filters_from_preset(name) {
        return Some(TuningPreset {
            name: name.to_string(),
            preamp_db,
            filters,
            ..Default::default()
        });
    }
    find_user_preset(&presets_dir(root), name)
}

fn ab_state(root: &Path) -> serde_json::Value {
    serde_json::json!({
        "a": ab_slot_path(root, "a").map(|p| p.is_file()).unwrap_or(false),
        "b": ab_slot_path(root, "b").map(|p| p.is_file()).unwrap_or(false),
    })
}

// ── 对 Dart 的入口（JSON in/out，`{ok, error, data}`）─────────────────

fn ok(data: serde_json::Value) -> String {
    serde_json::json!({ "ok": true, "error": "", "data": data }).to_string()
}

fn fail(error: impl Into<String>) -> String {
    serde_json::json!({ "ok": false, "error": error.into(), "data": null }).to_string()
}

/// 读调音状态（含频响曲线）。
pub fn get_json(root: &Path) -> String {
    ok(dto(root))
}

/// 只改总开关（不重置链）。
pub fn set_enabled_json(root: &Path, enabled: bool) -> String {
    match current(root) {
        Ok(mut cfg) => {
            cfg.enabled = enabled;
            match apply(root, cfg) {
                Ok(data) => ok(data),
                Err(e) => fail(e),
            }
        }
        Err(e) => fail(e),
    }
}

/// 整条链重置成某条预设（内置 / 用户预设都认），并设置 `enabled`（v1 `player_tuning_set`）。
pub fn set_preset_json(root: &Path, enabled: bool, preset: &str) -> String {
    let Some(found) = resolve_preset(root, preset) else {
        return fail(format!("ERR_BAD_PRESET：没有这个预设（{preset}）"));
    };
    let cfg = TuningConfig::from_preset(&found, enabled);
    if let Err(e) = build_chain(root, &cfg, VALIDATE_RATE) {
        return fail(e);
    }
    match apply(root, cfg) {
        Ok(data) => ok(data),
        Err(e) => fail(e),
    }
}

/// 把一条预设整条展开进来（`enabled` 不动）。
pub fn apply_preset_json(root: &Path, preset: &str) -> String {
    let Some(found) = resolve_preset(root, preset) else {
        return fail(format!("ERR_BAD_PRESET：没有这个预设（{preset}）"));
    };
    let mut cfg = match current(root) {
        Ok(cfg) => cfg,
        Err(e) => return fail(e),
    };
    found.apply_to(&mut cfg);
    if let Err(e) = build_chain(root, &cfg, VALIDATE_RATE) {
        return fail(e);
    }
    match apply(root, cfg) {
        Ok(data) => ok(data),
        Err(e) => fail(e),
    }
}

/// 覆盖整条链（`enabled` 不动、`preset` 变 `custom`）。
///
/// 入参是 Dart 侧的 camelCase JSON：`{preampDb, bassDb?, trebleDb?, filters, graphicEq?,
/// delayMs?, channelCopy?, convolution?, ifElse?}`。
pub fn set_chain_json(root: &Path, payload: &str) -> String {
    let value: serde_json::Value = match serde_json::from_str(payload) {
        Ok(value) => value,
        Err(e) => return fail(format!("入参不是合法 JSON：{e}")),
    };
    let from_value = |key: &str| value.get(key).cloned().unwrap_or(serde_json::Value::Null);
    let filters: Vec<TuningFilter> = match serde_json::from_value::<Vec<TuningFilter>>(from_value("filters")) {
        Ok(filters) => filters,
        Err(e) => return fail(format!("filters 形状不对：{e}")),
    };
    let graphic_eq = match from_value("graphicEq") {
        serde_json::Value::Null => Vec::new(),
        other => match serde_json::from_value::<Vec<serde_json::Value>>(other) {
            Ok(items) => items,
            Err(e) => return fail(format!("graphicEq 形状不对：{e}")),
        },
    };
    let channel_copy = match from_value("channelCopy") {
        serde_json::Value::Null => Vec::new(),
        other => match serde_json::from_value::<Vec<TuningCopy>>(other) {
            Ok(items) => items,
            Err(e) => return fail(format!("channelCopy 形状不对：{e}")),
        },
    };
    let if_else = match from_value("ifElse") {
        serde_json::Value::Null => Vec::new(),
        other => match serde_json::from_value::<Vec<TuningCondBlock>>(other) {
            Ok(items) => items,
            Err(e) => return fail(format!("ifElse 形状不对：{e}")),
        },
    };
    let mut cfg = match current(root) {
        Ok(cfg) => cfg,
        Err(e) => return fail(e),
    };
    cfg.preset = CUSTOM_PRESET.to_string();
    cfg.preamp_db = value.get("preampDb").and_then(|v| v.as_f64()).unwrap_or(0.0);
    cfg.bass_db = value.get("bassDb").and_then(|v| v.as_f64()).unwrap_or(0.0);
    cfg.treble_db = value.get("trebleDb").and_then(|v| v.as_f64()).unwrap_or(0.0);
    cfg.filters = filters;
    cfg.graphic_eq = graphic_eq;
    cfg.delay_ms = value.get("delayMs").and_then(|v| v.as_f64()).unwrap_or(0.0);
    cfg.channel_copy = channel_copy;
    cfg.convolution = value
        .get("convolution")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    cfg.if_else = if_else;
    if let Err(e) = build_chain(root, &cfg, VALIDATE_RATE) {
        return fail(e);
    }
    match apply(root, cfg) {
        Ok(data) => ok(data),
        Err(e) => fail(e),
    }
}

/// 把当前链存成用户预设。
pub fn preset_save_json(root: &Path, name: &str, overwrite: bool) -> String {
    let name = name.trim();
    if name.is_empty() {
        return fail("预设名不能为空");
    }
    let mut cfg = match current(root) {
        Ok(cfg) => cfg,
        Err(e) => return fail(e),
    };
    if let Err(e) = build_chain(root, &cfg, VALIDATE_RATE) {
        return fail(e);
    }
    if let Err(e) = save_user_preset(
        &presets_dir(&shared_root(root)),
        &TuningPreset::snapshot(&cfg, name),
        overwrite,
    ) {
        return fail(e);
    }
    cfg.preset = name.to_string();
    match apply(root, cfg) {
        Ok(data) => ok(data),
        Err(e) => fail(e),
    }
}

pub fn preset_delete_json(root: &Path, name: &str) -> String {
    let mut cfg = match current(root) {
        Ok(cfg) => cfg,
        Err(e) => return fail(e),
    };
    if let Err(e) = delete_user_preset(&presets_dir(&shared_root(root)), name) {
        return fail(e);
    }
    if cfg.preset == name {
        cfg.preset = CUSTOM_PRESET.to_string();
    }
    match apply(root, cfg) {
        Ok(data) => ok(data),
        Err(e) => fail(e),
    }
}

pub fn preset_rename_json(root: &Path, from: &str, to: &str) -> String {
    let to = to.trim();
    if to.is_empty() {
        return fail("预设名不能为空");
    }
    let mut cfg = match current(root) {
        Ok(cfg) => cfg,
        Err(e) => return fail(e),
    };
    if let Err(e) = rename_user_preset(&presets_dir(&shared_root(root)), from, to) {
        return fail(e);
    }
    if cfg.preset == from {
        cfg.preset = to.to_string();
    }
    match apply(root, cfg) {
        Ok(data) => ok(data),
        Err(e) => fail(e),
    }
}

/// 导入预设文件（**导入即应用**；同名覆盖）。
pub fn preset_import_json(root: &Path, path: &str) -> String {
    let preset = match read_preset_file(Path::new(path)) {
        Ok(preset) => preset,
        Err(e) => return fail(format!("导入失败：{e}")),
    };
    let cfg = match current(root) {
        Ok(cfg) => cfg,
        Err(e) => return fail(e),
    };
    let candidate = TuningConfig::from_preset(&preset, cfg.enabled);
    if let Err(e) = build_chain(root, &candidate, VALIDATE_RATE) {
        return fail(e);
    }
    if let Err(e) = save_user_preset(&presets_dir(&shared_root(root)), &preset, true) {
        return fail(e);
    }
    match apply(root, candidate) {
        Ok(data) => ok(data),
        Err(e) => fail(e),
    }
}

/// 导出当前链到预设文件。
pub fn preset_export_json(root: &Path, path: &str) -> String {
    let cfg = match current(root) {
        Ok(cfg) => cfg,
        Err(e) => return fail(e),
    };
    if let Err(e) = build_chain(root, &cfg, VALIDATE_RATE) {
        return fail(e);
    }
    let name = if cfg.preset.is_empty() {
        CUSTOM_PRESET.to_string()
    } else {
        cfg.preset.clone()
    };
    let text = match serde_json::to_string_pretty(&TuningPreset::snapshot(&cfg, &name)) {
        Ok(text) => text,
        Err(e) => return fail(e.to_string()),
    };
    match std::fs::write(path, text) {
        Ok(()) => ok(serde_json::json!({})),
        Err(e) => fail(format!("导出失败：{e}")),
    }
}

/// A / B 两槽各自有没有快照。
pub fn ab_state_json(root: &Path) -> String {
    ok(ab_state(&shared_root(root)))
}

/// 把当前链快照进 A / B 槽。
pub fn ab_save_json(root: &Path, slot: &str) -> String {
    let root = shared_root(root);
    let Some(path) = ab_slot_path(&root, slot) else {
        return fail(format!("ERR_BAD_SLOT：槽只能是 a / b（收到 {slot}）"));
    };
    let cfg = match current(&root) {
        Ok(cfg) => cfg,
        Err(e) => return fail(e),
    };
    if let Err(e) = build_chain(&root, &cfg, VALIDATE_RATE) {
        return fail(e);
    }
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return fail(format!("建 tuning 目录失败：{e}"));
        }
    }
    let text = match serde_json::to_string_pretty(&cfg) {
        Ok(text) => text,
        Err(e) => return fail(e.to_string()),
    };
    if let Err(e) = std::fs::write(&path, text) {
        return fail(format!("写 {} 失败：{e}", path.display()));
    }
    ok(ab_state(&root))
}

/// 应用 A / B 槽的快照（经唯一写点 `apply`）。
pub fn ab_apply_json(root: &Path, slot: &str) -> String {
    let root = shared_root(root);
    let Some(path) = ab_slot_path(&root, slot) else {
        return fail(format!("ERR_BAD_SLOT：槽只能是 a / b（收到 {slot}）"));
    };
    let name = slot.trim().to_ascii_uppercase();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return fail(format!("ERR_EMPTY_SLOT：{name} 槽还没有快照"));
    };
    let parsed: TuningConfig = match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
        Ok(cfg) => cfg,
        Err(e) => return fail(format!("ERR_BAD_SLOT_FILE：{name} 槽的快照读不出来（{e}）")),
    };
    let cfg = migrate_tuning(parsed);
    if let Err(e) = build_chain(&root, &cfg, VALIDATE_RATE) {
        return fail(e);
    }
    match apply(&root, cfg) {
        Ok(data) => ok(data),
        Err(e) => fail(e),
    }
}

// ── 文件对话框（导入 / 导出预设、挑卷积 IR）──────────────────────────
//
// v1 借 Tauri 的 dialog 插件；v2 是纯 C ABI，没有那层 ⇒ 直接调 Win32 的
// `GetOpenFileNameW` / `GetSaveFileNameW`（与 `music.rs::pick_playlist_file` /
// `convert.rs::pick_inputs` 同一条路，不引第三方对话框 crate）。
//
// 三条纪律：
//   ① 取消**不是错误**：返回 `canceled: true`，界面据此什么都不做；
//   ② 只选、不读、不校验 —— 校验一律交给 `preset_import_json` / `set_chain_json`
//      （面板不解析文件，避免把引擎那套判断再写一遍）；
//   ③ 非 Windows 平台返回 `canceled: false` + 一句原因（`flutter` 目前只有 Windows 端）。

#[cfg(windows)]
#[allow(unsafe_code)]
fn pick_open_path(filter: &str, title: &str) -> Option<String> {
    unsafe {
        use windows::core::{PCWSTR, PWSTR};
        use windows::Win32::UI::Controls::Dialogs::{
            GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST,
            OPENFILENAMEW,
        };
        let filter: Vec<u16> = filter.encode_utf16().collect();
        let title: Vec<u16> = title.encode_utf16().collect();
        let mut buffer = vec![0u16; 4096];
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: PWSTR(buffer.as_mut_ptr()),
            nMaxFile: buffer.len() as u32,
            lpstrTitle: PCWSTR(title.as_ptr()),
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        if !GetOpenFileNameW(&mut ofn).as_bool() {
            return None;
        }
        let len = buffer.iter().position(|c| *c == 0).unwrap_or(0);
        Some(String::from_utf16_lossy(&buffer[..len]))
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn pick_save_path(file_name: &str, ext: &str, title: &str) -> Option<String> {
    unsafe {
        use windows::core::{PCWSTR, PWSTR};
        use windows::Win32::UI::Controls::Dialogs::{
            GetSaveFileNameW, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
            OPENFILENAMEW,
        };
        let mut buffer = vec![0u16; 4096];
        for (slot, unit) in buffer.iter_mut().zip(file_name.encode_utf16()) {
            *slot = unit;
        }
        let title: Vec<u16> = title.encode_utf16().collect();
        let ext: Vec<u16> = ext.encode_utf16().chain(std::iter::once(0)).collect();
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            lpstrFile: PWSTR(buffer.as_mut_ptr()),
            nMaxFile: buffer.len() as u32,
            lpstrTitle: PCWSTR(title.as_ptr()),
            lpstrDefExt: PCWSTR(ext.as_ptr()),
            Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        if !GetSaveFileNameW(&mut ofn).as_bool() {
            return None;
        }
        let len = buffer.iter().position(|c| *c == 0).unwrap_or(0);
        Some(String::from_utf16_lossy(&buffer[..len]))
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn picked(picked: Option<String>) -> String {
    match picked {
        Some(path) => serde_json::json!({
            "ok": true, "canceled": false, "path": path, "error": "",
        })
        .to_string(),
        None => serde_json::json!({
            "ok": false, "canceled": true, "path": "", "error": "",
        })
        .to_string(),
    }
}

/// 选一个预设文件（导入用）。返回 `{ok,canceled,path,error}`。
pub fn pick_preset_json() -> String {
    #[cfg(windows)]
    {
        return picked(pick_open_path(
            "调音预设 (*.json)\0*.json\0所有文件 (*.*)\0*.*\0\0",
            "导入调音预设",
        ));
    }
    #[cfg(not(windows))]
    {
        fail("ERR_NO_DIALOG：这个平台没有系统文件对话框")
    }
}

/// 选导出的落盘位置（另存为）。返回 `{ok,canceled,path,error}`。
pub fn pick_export_json() -> String {
    #[cfg(windows)]
    {
        return picked(pick_save_path("tuning-preset.json", "json", "导出调音预设"));
    }
    #[cfg(not(windows))]
    {
        fail("ERR_NO_DIALOG：这个平台没有系统文件对话框")
    }
}

/// 选一个卷积用的脉冲响应（WAV）。返回 `{ok,canceled,path,error}`。
///
/// 面板**不解析 WAV 头**：单声道 / 采样率一致这两条要拿到流的采样率才判得了，
/// 一并在提交那一次由引擎回错（见 docs/archive/ai-spec.md §4.6 的 P5-4 第 4 条）。
pub fn pick_ir_json() -> String {
    #[cfg(windows)]
    {
        return picked(pick_open_path(
            "脉冲响应 (*.wav)\0*.wav\0所有文件 (*.*)\0*.*\0\0",
            "选择脉冲响应",
        ));
    }
    #[cfg(not(windows))]
    {
        fail("ERR_NO_DIALOG：这个平台没有系统文件对话框")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-tuning-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_config_is_flat_and_disabled() {
        let cfg = default_tuning_config();
        assert!(!cfg.enabled, "DSP 默认必须关（opt-in）");
        assert_eq!(cfg.preset, "flat");
        assert_eq!(cfg.preamp_db, 0.0);
        assert!(cfg.filters.is_empty(), "flat 展开就是空链");
    }

    #[test]
    fn legacy_config_is_migrated_to_expanded_filters() {
        // 老配置只有 {enabled, preset}：升级后必须把 bass 那档展开出来
        let legacy: TuningConfig =
            serde_json::from_str(r#"{ "enabled": true, "preset": "bass" }"#).unwrap();
        let cfg = migrate_tuning(legacy);
        assert_eq!(cfg.filters.len(), 2);
        assert_eq!(cfg.preamp_db, -6.0);
        assert_eq!(cfg.filters[0].kind, "low_shelf");
        // 认不出的 preset ⇒ 回落默认档
        let broken: TuningConfig =
            serde_json::from_str(r#"{ "enabled": true, "preset": "nope" }"#).unwrap();
        assert_eq!(migrate_tuning(broken).preset, "flat");
    }

    #[test]
    fn effective_json_drops_off_filters_and_appends_shelf_gains() {
        let root = temp_root("effective");
        let cfg = TuningConfig {
            enabled: true,
            preset: "custom".into(),
            preamp_db: -3.0,
            bass_db: 4.0,
            treble_db: -2.0,
            filters: vec![
                TuningFilter {
                    kind: "peaking".into(),
                    freq_hz: 1000.0,
                    gain_db: Some(2.0),
                    q: Some(1.0),
                    on: true,
                },
                TuningFilter {
                    kind: "peaking".into(),
                    freq_hz: 3000.0,
                    gain_db: Some(9.0),
                    q: None,
                    on: false,
                },
            ],
            ..TuningConfig::default()
        };
        let json = effective_json(&root, &cfg);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let filters = value["filters"].as_array().unwrap();
        assert_eq!(filters.len(), 3, "关闭的段必须被摘掉，低/高音各追加一条");
        assert_eq!(filters[0]["freq_hz"], 1000.0);
        assert_eq!(filters[1]["kind"], "low_shelf");
        assert_eq!(filters[1]["freq_hz"], BASS_SHELF_HZ);
        assert_eq!(filters[2]["kind"], "high_shelf");
        assert_eq!(filters[2]["freq_hz"], TREBLE_SHELF_HZ);
        // 引擎必须能认这份 JSON
        build_chain(&root, &cfg, VALIDATE_RATE).expect("引擎应接受这条链");
    }

    #[test]
    fn invalid_chain_is_rejected_with_a_reason() {
        let root = temp_root("invalid");
        // 未知字段：引擎 `deny_unknown_fields` 必定拒绝
        let mut cfg = default_tuning_config();
        cfg.filters = vec![TuningFilter {
            kind: "peaking".into(),
            freq_hz: 1000.0,
            gain_db: Some(1.0),
            q: Some(1.0),
            on: true,
        }];
        cfg.graphic_eq = vec![serde_json::json!({ "bogus": 1 })];
        assert!(build_chain(&root, &cfg, VALIDATE_RATE).is_err(), "未知效果器字段必须报错");
    }

    #[test]
    fn response_curve_shows_the_bass_shelf() {
        let root = temp_root("response");
        let preset = resolve_preset(&root, "bass").unwrap();
        let cfg = TuningConfig::from_preset(&preset, true);
        let (freqs, db, _) = response_of(&root, &cfg, 48_000.0);
        assert_eq!(freqs.len(), RESPONSE_POINTS);
        assert_eq!(db.len(), RESPONSE_POINTS);
        // 120Hz 附近应明显高于 10kHz（低音 +6 / 高音 −2，preamp −6）
        let near = |hz: f64| {
            let index = freqs
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    (a.1 - hz).abs().partial_cmp(&(b.1 - hz).abs()).unwrap()
                })
                .map(|(i, _)| i)
                .unwrap();
            db[index]
        };
        assert!(near(120.0) > near(10_000.0) + 3.0, "低音必须明显高于高音");
    }

    #[test]
    fn user_presets_round_trip_and_require_overwrite() {
        let dir = temp_root("presets").join("presets");
        let cfg = TuningConfig {
            preset: "mine".into(),
            preamp_db: -1.5,
            filters: vec![TuningFilter {
                kind: "low_shelf".into(),
                freq_hz: 120.0,
                gain_db: Some(3.0),
                q: None,
                on: true,
            }],
            ..TuningConfig::default()
        };
        save_user_preset(&dir, &TuningPreset::snapshot(&cfg, "我的链"), false).unwrap();
        assert!(save_user_preset(&dir, &TuningPreset::snapshot(&cfg, "我的链"), false).is_err());
        save_user_preset(&dir, &TuningPreset::snapshot(&cfg, "我的链"), true).unwrap();
        let listed = list_user_presets(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "我的链");
        assert_eq!(listed[0].filters.len(), 1);
        // 重命名后旧文件必须消失、按新名字能找到
        rename_user_preset(&dir, "我的链", "改名后").unwrap();
        assert!(find_user_preset(&dir, "我的链").is_none());
        assert!(find_user_preset(&dir, "改名后").is_some());
        delete_user_preset(&dir, "改名后").unwrap();
        assert!(list_user_presets(&dir).is_empty());
    }

    #[test]
    fn preset_file_names_avoid_windows_reserved_words_and_illegal_chars() {
        assert_eq!(preset_file_name("我的链"), Some("preset-我的链.json".to_string()));
        assert_eq!(preset_file_name("a/b:c"), Some("preset-a_b_c.json".to_string()));
        assert_eq!(preset_file_name("   "), None);
        // 前缀保证 `CON` 这类保留名不会被当成设备名
        assert_eq!(preset_file_name("CON"), Some("preset-CON.json".to_string()));
    }
}
