//! 桌宠（移植 v1 `app/src/plugins/builtin/pet.ts` 的「窗口 + 形象」那一半）。
//!
//! **v1 的事实**（以源码为准）：
//!   · 配置在**同源 localStorage** 的键 `lunac.pet.v1`（字段见 [`PetConfig`]）；
//!   · 桌宠窗是一个独立、无边框、透明、置顶、不进任务栏的 **WebView 窗口**，
//!     形象由用户导入：图片（png/jpg/webp/gif）或 Live2D `*.model3.json`；
//!   · Live2D 渲染器就是 `<Modules>\pet\engine\live2d-engine.js` + `live2dcubismcore.min.js`。
//!
//! **v2 的落地方式**（debug-only）：本模块在 `lunac_core.dll` 里起一个**专用线程**，
//! 自己建 Win32 透明置顶窗并挂 **WebView2**。原因：`webview_windows` 之类的 Flutter 插件
//! 没有暴露 `ICoreWebView2Controller::put_DefaultBackgroundColor(transparent)`，也不支持
//! 分层窗，硬塞进 Flutter 窗会得到一块白底；直接持有 WebView2 控制器才能拿到 v1 的
//! 「真透明 + 同一份 JS 渲染器」。窗口线程与 Flutter 主线程只通过
//! 「配置文件 + `PostMessage` + 事件队列」通信，不共享任何 COM 指针。
//!
//! 窗口里的 host 页面由 [`HOST_HTML`] 生成，经自定义源 `https://lunac.pet/` 提供：
//!   · `/host.html`                        —— host 页面；
//!   · `/engine/live2d-engine.js`          —— 与 v1 同一份渲染器（`<exe>\Modules\pet\engine\`）；
//!   · `/engine/live2dcubismcore.min.js`   —— Cubism Core（同上）；
//!   · `/fs/<utf8 路径的十六进制>`          —— 用户形象文件（模型目录 / 图片本体）。
//! 三者同源，`fetch` / 纹理加载都不受 `file://` 的 CORS 限制。
//!
//! 安装包零第三方模型资产：本模块不复制、不下载任何模型；引擎文件是 v1 已有的构建产物。

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// 与 v1 `live2d.ts` 的 `PET_SCALE_MIN/MAX` 同值（越界静默回落 1）。
pub const PET_SCALE_MIN: f64 = 0.4;
pub const PET_SCALE_MAX: f64 = 8.0;

/// v1 清单 `window` 段的宽 / 高（逻辑像素）。
pub const PET_WIN_W_DEFAULT: f64 = 320.0;
pub const PET_WIN_H_DEFAULT: f64 = 510.0;
const PET_WIN_W_MIN: f64 = 160.0;
const PET_WIN_W_MAX: f64 = 1000.0;
const PET_WIN_H_MIN: f64 = 250.0;
const PET_WIN_H_MAX: f64 = 1200.0;

/// 配置文件名沿用 v1 的 localStorage 键名 —— 字段可一对一对上，迁移时不丢语义。
const CONFIG_FILE: &str = "lunac.pet.v1.json";

/// 桌宠配置。**字段名与 v1 `PetConfig` 完全一致**（`serde(rename_all = "camelCase")`），
/// 只挑 v2 真正用到的那些；v1 其它字段（expressions / params / proactive / tts / …）经
/// `extra` **原样保留**，读写配置时不会被抹掉。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetConfig {
    /// 图片形象（绝对路径）。与 `model` 二选一；`model` 非空时优先（与 v1 `loadConfig` 同口径）。
    #[serde(default)]
    pub image: String,
    /// Live2D 模型定义（`*.model3.json` 绝对路径）。
    #[serde(default)]
    pub model: String,
    /// 形象缩放（0.4..8）。图片走 CSS transform，Live2D 走引擎 `setScale`。
    #[serde(default = "one")]
    pub scale: f64,
    /// 形象相对基准位置的像素偏移（v1 里由 Alt+拖动写入；v2 只读回放）。
    #[serde(default)]
    pub offset_x: f64,
    #[serde(default)]
    pub offset_y: f64,
    /// 整体旋转（角度制，−180..180），Live2D 专属。
    #[serde(default)]
    pub rotation: f64,
    /// 鼠标穿透（默认关）。开着时窗口不吃任何鼠标事件 —— 只能在控制台里关掉。
    #[serde(default)]
    pub click_through: bool,
    /// 置顶（v1 清单里恒为置顶；v2 给它一个开关，默认 true）。
    #[serde(default = "yes")]
    pub topmost: bool,
    /// 桌宠窗当前是否显示。
    #[serde(default = "yes")]
    pub visible: bool,
    /// 窗口左上角（**物理像素**）。`None` = 由宿主挑一个默认位置（右下角）。
    #[serde(default)]
    pub pos_x: Option<i32>,
    #[serde(default)]
    pub pos_y: Option<i32>,
    /// 窗口宽 / 高（逻辑像素）。
    #[serde(default = "default_win_w")]
    pub win_w: f64,
    #[serde(default = "default_win_h")]
    pub win_h: f64,
    /// v1 其它配置字段（原样保留）。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn one() -> f64 {
    1.0
}
fn yes() -> bool {
    true
}
fn default_win_w() -> f64 {
    PET_WIN_W_DEFAULT
}
fn default_win_h() -> f64 {
    PET_WIN_H_DEFAULT
}

impl Default for PetConfig {
    fn default() -> Self {
        Self {
            image: String::new(),
            model: String::new(),
            scale: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            rotation: 0.0,
            click_through: false,
            topmost: true,
            visible: true,
            pos_x: None,
            pos_y: None,
            win_w: PET_WIN_W_DEFAULT,
            win_h: PET_WIN_H_DEFAULT,
            extra: Map::new(),
        }
    }
}

impl PetConfig {
    /// 把任意来源（旧配置 / 控制台传入的 JSON）收敛成合法值。
    /// **纯函数**，可单测；口径与 v1 `loadConfig` 一致（越界 / 非法静默回落）。
    pub fn normalize(mut self) -> Self {
        if !self.scale.is_finite() || !(PET_SCALE_MIN..=PET_SCALE_MAX).contains(&self.scale) {
            self.scale = 1.0;
        }
        if !self.rotation.is_finite() {
            self.rotation = 0.0;
        } else {
            self.rotation = self.rotation.clamp(-180.0, 180.0);
        }
        if !self.offset_x.is_finite() {
            self.offset_x = 0.0;
        }
        if !self.offset_y.is_finite() {
            self.offset_y = 0.0;
        }
        if !self.win_w.is_finite() {
            self.win_w = PET_WIN_W_DEFAULT;
        }
        self.win_w = self.win_w.clamp(PET_WIN_W_MIN, PET_WIN_W_MAX);
        if !self.win_h.is_finite() {
            self.win_h = PET_WIN_H_DEFAULT;
        }
        self.win_h = self.win_h.clamp(PET_WIN_H_MIN, PET_WIN_H_MAX);
        // v1 的口径：`model` 优先；两者都空 = 没有形象（窗口显示空状态提示）。
        self.image = self.image.trim().to_string();
        self.model = self.model.trim().to_string();
        self
    }

    pub fn is_live2d(&self) -> bool {
        !self.model.is_empty()
    }
}

/// 解析一段配置 JSON（坏 JSON 回落默认值，不让桌宠开不出来）。
pub fn parse_config(text: &str) -> PetConfig {
    serde_json::from_str::<PetConfig>(text)
        .map(PetConfig::normalize)
        .unwrap_or_default()
}

/// 配置落点：`<exe 根>\ModuleData\pet\lunac.pet.v1.json`（v1 的 localStorage 键名 + 文件名）。
pub fn config_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join("ModuleData").join("pet").join(CONFIG_FILE)
}

pub fn load_config(root: &std::path::Path) -> PetConfig {
    let path = config_path(root);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_config(&text),
        Err(_) => PetConfig::default(),
    }
}

/// 保存配置（自动建目录）。失败只返回原因，不影响调用方。
pub fn save_config(root: &std::path::Path, cfg: &PetConfig) -> Result<(), String> {
    let path = config_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let text = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化配置失败：{e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写配置失败：{e}"))
}

// ══════════════════════════════════════════════════════════════════
//  「主动反应」配置（v1 `ProactiveCfg`，2026-10-09 / 2026-10-17 口径）
// ══════════════════════════════════════════════════════════════════
//
// 字段名与 v1 **逐字一致**；计数字段（`date` / `used`）与配置共用同一个对象
// （v1 同款：不引入第二个存储）。读「今日已用」时 `date != 今天` ⇒ 视为 0
// （跨日自动归零，不必写盘）。
//
// 这一层只做「配置的读写 + 归一化」；调度 / 抓屏 / 发问在桌宠侧，后续接。
// 归一化必须是纯函数：坏值回落默认、越界夹到区间（v1 `clampInt` 同口径）。

/// (min, max, fallback)：与 v1 `PROACTIVE_*` 常量逐条对应。
const PROACTIVE_INTERVAL: (u32, u32, u32) = (1, 1440, 30);
const PROACTIVE_CAP: (u32, u32, u32) = (0, 200, 20);
const PROACTIVE_POLL: (u32, u32, u32) = (5, 600, 20);
const PROACTIVE_GAP: (u32, u32, u32) = (30, 3600, 600);
const PROACTIVE_DIFF: (u32, u32, u32) = (1, 64, 12);

fn clamp_int(value: Option<&Value>, (min, max, fallback): (u32, u32, u32)) -> u32 {
    match value.and_then(Value::as_f64) {
        Some(raw) if raw.is_finite() => {
            let rounded = raw.round() as i64;
            rounded.clamp(min as i64, max as i64) as u32
        }
        _ => fallback,
    }
}

/// 把「当前盘上的 proactive」与「本次 patch」合成一份合法配置。
///
/// * 缺字段 ⇒ 沿用当前；当前也没有 ⇒ 用默认；
/// * 旧键（`autoShot` / `autoShotMin` / `screenWatch` / `screenWatchSec`）做一次性迁移：
///   三个开关按 OR 合并，轮询秒数用 `screenWatchSec` 回填（v1 同口径，迁移后旧键消失）；
/// * `patch` 显式给了 `used` 才改计数（`0` 表示清零），否则保留当前值。
pub fn normalize_proactive(current: Option<&Value>, patch: Option<&Value>) -> Value {
    let cur = current.and_then(Value::as_object);
    let patch_map = patch.and_then(Value::as_object);
    let pick = |key: &str| -> Option<&Value> {
        patch_map
            .and_then(|map| map.get(key))
            .or_else(|| cur.and_then(|map| map.get(key)))
    };
    let patch_flag = |key: &str| {
        patch_map
            .and_then(|map| map.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let enabled = pick("enabled").and_then(Value::as_bool).unwrap_or(false)
        || patch_flag("autoShot")
        || patch_flag("screenWatch");
    let interval_min = clamp_int(pick("intervalMin"), PROACTIVE_INTERVAL);
    let daily_cap = clamp_int(pick("dailyCap"), PROACTIVE_CAP);
    let screen_poll_sec = clamp_int(
        pick("screenPollSec").or_else(|| pick("screenWatchSec")),
        PROACTIVE_POLL,
    );
    let screen_min_gap_sec = clamp_int(pick("screenMinGapSec"), PROACTIVE_GAP);
    let screen_diff = clamp_int(pick("screenDiff"), PROACTIVE_DIFF);
    let date = pick("date")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // `used`：patch 显式给了就按它（0 = 清零）；否则保留当前（坏值按 0）。
    let used_source = patch_map
        .and_then(|map| map.get("used"))
        .or_else(|| cur.and_then(|map| map.get("used")));
    let used = used_source
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value.floor() as u64)
        .unwrap_or(0);
    json!({
        "enabled": enabled,
        "intervalMin": interval_min,
        "dailyCap": daily_cap,
        "screenPollSec": screen_poll_sec,
        "screenMinGapSec": screen_min_gap_sec,
        "screenDiff": screen_diff,
        "date": date,
        "used": used,
    })
}

/// 合并保存「主动反应」配置，返回保存后的完整配置 JSON。
pub fn set_proactive_json(patch: &str) -> String {
    let root = exe_root();
    let mut cfg = load_config(&root);
    let incoming: Value = match serde_json::from_str(patch) {
        Ok(value) => value,
        Err(e) => return json!({ "error": format!("配置不是合法 JSON：{e}") }).to_string(),
    };
    if !incoming.is_object() {
        return json!({ "error": "配置必须是 JSON 对象" }).to_string();
    }
    let current = cfg.extra.get("proactive").cloned();
    let normalized = normalize_proactive(current.as_ref(), Some(&incoming));
    cfg.extra.insert("proactive".to_string(), normalized);
    if let Err(e) = save_config(&root, &cfg) {
        return json!({ "error": e }).to_string();
    }
    notify_window(WM_PET_APPLY, 0);
    serde_json::to_string(&cfg).unwrap_or_else(|_| "{}".to_string())
}

// ══════════════════════════════════════════════════════════════════
//  桌宠旁挂文件 / 动作文件（v1 `commands.rs` 的对应物）
// ══════════════════════════════════════════════════════════════════
//
// 用户口径（2026-10-10）：**热键、表情、动作都跟模型文件夹走** —— 换模型 / 换机器
// 拷走整个文件夹就完整。布局与 v1 一致：
//   · `<模型目录>\lunac-live2d\lunac.hotkeys.json` / `lunac.chains.json`；
//   · lunac 新建的动作在 `<模型目录>\lunac-live2d\motions\`；
//   · 用户 / 作者自带的 `*.motion3.json` 原地不动（改名 / 删除按名先查新目录再查模型根）。
// 旧位置（模型目录根）由 `resolve_pet_side_file` 做一次性迁移（移动，失败则退回旧位置）。
//
// 口子收最小：side 文件只认固定文件名；动作文件只认**纯文件名 + `.motion3.json`**。

const PET_SIDE_DIR: &str = "lunac-live2d";
const PET_MOTIONS_SUBDIR: &str = "motions";

/// 模型文件（`*.model3.json`）所在目录。
fn pet_model_dir(model_path: &str) -> Result<std::path::PathBuf, String> {
    let path = std::path::Path::new(model_path);
    if !path.is_file() {
        return Err("模型文件不存在".to_string());
    }
    path.parent()
        .map(|dir| dir.to_path_buf())
        .ok_or_else(|| "模型路径没有父目录".to_string())
}

fn pet_side_file_path(model_path: &str, name: &str) -> Result<std::path::PathBuf, String> {
    Ok(pet_model_dir(model_path)?.join(PET_SIDE_DIR).join(name))
}

fn pet_side_legacy_path(model_path: &str, name: &str) -> Result<std::path::PathBuf, String> {
    Ok(pet_model_dir(model_path)?.join(name))
}

/// 读路径：新位置优先；只有旧位置有 ⇒ 顺手搬过去（搬不动就退回旧位置）。
fn resolve_pet_side_file(model_path: &str, name: &str) -> Result<std::path::PathBuf, String> {
    let new_path = pet_side_file_path(model_path, name)?;
    if new_path.is_file() {
        return Ok(new_path);
    }
    let old_path = pet_side_legacy_path(model_path, name)?;
    if old_path.is_file() {
        if let Some(parent) = new_path.parent() {
            if std::fs::create_dir_all(parent).is_ok() && std::fs::rename(&old_path, &new_path).is_ok()
            {
                return Ok(new_path);
            }
        }
        return Ok(old_path);
    }
    Ok(new_path)
}

/// 写路径：恒为新位置，并保证父目录存在（旧位置若还留着同一份，尽力先搬过去）。
fn prepare_pet_side_write(model_path: &str, name: &str) -> Result<std::path::PathBuf, String> {
    let new_path = pet_side_file_path(model_path, name)?;
    if let Some(parent) = new_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 lunac-live2d 目录失败：{e}"))?;
    }
    if !new_path.is_file() {
        if let Ok(old_path) = pet_side_legacy_path(model_path, name) {
            if old_path.is_file() {
                let _ = std::fs::rename(&old_path, &new_path);
            }
        }
    }
    Ok(new_path)
}

fn read_side_file(model_path: &str, name: &str, what: &str) -> Result<Option<String>, String> {
    let path = resolve_pet_side_file(model_path, name)?;
    if !path.is_file() {
        return Ok(None);
    }
    std::fs::read_to_string(&path)
        .map(Some)
        .map_err(|e| format!("读{what}失败：{e}"))
}

fn write_side_file(
    model_path: &str,
    name: &str,
    json: &str,
    what: &str,
) -> Result<String, String> {
    let path = prepare_pet_side_write(model_path, name)?;
    std::fs::write(&path, json).map_err(|e| format!("写{what}失败：{e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// 动作文件目录：`<模型目录>/lunac-live2d/motions`。
fn motion_file_dir(model_path: &str) -> Result<std::path::PathBuf, String> {
    Ok(pet_model_dir(model_path)?
        .join(PET_SIDE_DIR)
        .join(PET_MOTIONS_SUBDIR))
}

/// 校验动作文件名：只允许纯文件名 + `.motion3.json` 后缀。
fn motion_file_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("文件名不能为空".to_string());
    }
    if trimmed.contains('\\') || trimmed.contains('/') || trimmed.contains("..") {
        return Err("文件名只能是不带路径的名字".to_string());
    }
    if !trimmed.to_ascii_lowercase().ends_with(".motion3.json") {
        return Err("文件必须叫 *.motion3.json".to_string());
    }
    Ok(trimmed.to_string())
}

/// 按名查现有动作文件：先新目录，再模型根（用户 / 作者自带）。
fn find_motion_file(model_path: &str, name: &str) -> Result<Option<std::path::PathBuf>, String> {
    let pure = motion_file_name(name)?;
    let dir = pet_model_dir(model_path)?;
    let new_path = dir.join(PET_SIDE_DIR).join(PET_MOTIONS_SUBDIR).join(&pure);
    if new_path.is_file() {
        return Ok(Some(new_path));
    }
    let old_path = dir.join(&pure);
    if old_path.is_file() {
        return Ok(Some(old_path));
    }
    Ok(None)
}

/// 一个动作文件（`{ 路径, 展示名 }`，与 v1 `MotionFile` 同形）。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MotionFileRef {
    pub path: String,
    pub name: String,
}

/// **递归**列出模型目录下所有 `*.motion3.json`（深度 ≤ 5、条目 ≤ 3000），按路径排序。
/// 顺序 = 注入引擎的组内序号（前端编号必须与它一致）。
pub fn list_motion_files(dir: &str) -> Result<Vec<MotionFileRef>, String> {
    const MAX_DEPTH: usize = 5;
    const MAX_FILES: usize = 3000;
    let root = std::path::PathBuf::from(dir);
    if !root.is_dir() {
        return Err("不是一个文件夹".to_string());
    }
    let mut out: Vec<MotionFileRef> = Vec::new();
    let mut queue: std::collections::VecDeque<(std::path::PathBuf, usize)> =
        std::collections::VecDeque::new();
    queue.push_back((root, 0));
    while let Some((current, depth)) = queue.pop_front() {
        if out.len() >= MAX_FILES {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth + 1 <= MAX_DEPTH {
                    queue.push_back((path, depth + 1));
                }
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.to_ascii_lowercase().ends_with(".motion3.json") {
                continue;
            }
            out.push(MotionFileRef {
                path: path.to_string_lossy().to_string(),
                name: name.trim_end_matches(".motion3.json").to_string(),
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// 递归列出 `*.exp3.json`（热键「重新导入」的回落数据源；展示名优先取 `Name`）。
pub fn list_exp3_files(dir: &str) -> Result<Vec<MotionFileRef>, String> {
    const MAX_DEPTH: usize = 5;
    const MAX_FILES: usize = 3000;
    let root = std::path::PathBuf::from(dir);
    if !root.is_dir() {
        return Err("不是一个文件夹".to_string());
    }
    let mut out: Vec<MotionFileRef> = Vec::new();
    let mut queue: std::collections::VecDeque<(std::path::PathBuf, usize)> =
        std::collections::VecDeque::new();
    queue.push_back((root, 0));
    while let Some((current, depth)) = queue.pop_front() {
        if out.len() >= MAX_FILES {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth + 1 <= MAX_DEPTH {
                    queue.push_back((path, depth + 1));
                }
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !file_name.to_ascii_lowercase().ends_with(".exp3.json") {
                continue;
            }
            let display = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|value| {
                    value
                        .get("Name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| file_name.trim_end_matches(".exp3.json").to_string());
            out.push(MotionFileRef {
                path: path.to_string_lossy().to_string(),
                name: display,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// 扫描一个模型文件夹根（不递归）：`model3` / `vtube` / `cdi3` 三类绝对路径。
pub fn scan_model_folder(dir: &str) -> Result<Value, String> {
    let root = std::path::PathBuf::from(dir);
    if !root.is_dir() {
        return Err("不是一个文件夹".to_string());
    }
    let entries = std::fs::read_dir(&root).map_err(|e| format!("读文件夹失败：{e}"))?;
    let mut model3: Vec<String> = Vec::new();
    let mut vtube: Vec<String> = Vec::new();
    let mut cdi3: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
        else {
            continue;
        };
        let text = path.to_string_lossy().to_string();
        if name.ends_with(".model3.json") {
            model3.push(text);
        } else if name.ends_with(".vtube.json") {
            vtube.push(text);
        } else if name.ends_with(".cdi3.json") {
            cdi3.push(text);
        }
    }
    model3.sort();
    vtube.sort();
    cdi3.sort();
    Ok(json!({ "model3": model3, "vtube": vtube, "cdi3": cdi3 }))
}

// ── JSON 口子（给 FFI 用：失败也是一段 JSON，不让 Dart 侧抛异常）──────────

fn ok_error<T: Serialize>(result: Result<T, String>, key: &str) -> String {
    match result {
        Ok(value) => json!({ "ok": true, key: value }).to_string(),
        Err(e) => json!({ "ok": false, "error": e }).to_string(),
    }
}

/// side 文件白名单：`hotkeys` / `chains`（面板只允许这两个名字）。
fn side_name(kind: &str) -> Option<&'static str> {
    match kind {
        "hotkeys" => Some("lunac.hotkeys.json"),
        "chains" => Some("lunac.chains.json"),
        _ => None,
    }
}

pub fn read_side_json(model_path: &str, kind: &str) -> String {
    let Some(name) = side_name(kind) else {
        return json!({ "ok": false, "error": "未知的旁挂文件类型" }).to_string();
    };
    match read_side_file(model_path, name, "旁挂文件") {
        Ok(text) => json!({ "ok": true, "text": text }).to_string(),
        Err(e) => json!({ "ok": false, "error": e }).to_string(),
    }
}

pub fn write_side_json(model_path: &str, kind: &str, json_text: &str) -> String {
    let Some(name) = side_name(kind) else {
        return json!({ "ok": false, "error": "未知的旁挂文件类型" }).to_string();
    };
    ok_error(
        write_side_file(model_path, name, json_text, "旁挂文件"),
        "path",
    )
}

pub fn motion_files_json(model_path: &str) -> String {
    match pet_model_dir(model_path) {
        Ok(dir) => ok_error(list_motion_files(&dir.to_string_lossy()), "files"),
        Err(e) => json!({ "ok": false, "error": e }).to_string(),
    }
}

pub fn motion_write_json(model_path: &str, name: &str, contents: &str) -> String {
    let result = (|| -> Result<String, String> {
        let pure = motion_file_name(name)?;
        let dir = motion_file_dir(model_path)?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建动作目录失败：{e}"))?;
        let path = dir.join(&pure);
        if path.exists() {
            return Err(format!("{pure} 已存在"));
        }
        std::fs::write(&path, contents).map_err(|e| format!("写动作文件失败：{e}"))?;
        Ok(path.to_string_lossy().to_string())
    })();
    ok_error(result, "path")
}

pub fn motion_delete_json(model_path: &str, name: &str) -> String {
    let result = (|| -> Result<(), String> {
        let path = find_motion_file(model_path, name)?.ok_or_else(|| "文件不存在".to_string())?;
        std::fs::remove_file(&path).map_err(|e| format!("删除动作文件失败：{e}"))
    })();
    ok_error(result, "deleted")
}

pub fn motion_rename_json(model_path: &str, from: &str, to: &str) -> String {
    let result = (|| -> Result<String, String> {
        let source = find_motion_file(model_path, from)?.ok_or_else(|| "原文件不存在".to_string())?;
        let pure = motion_file_name(to)?;
        let target = source
            .parent()
            .ok_or_else(|| "原文件没有父目录".to_string())?
            .join(&pure);
        if target.exists() {
            return Err(format!("{pure} 已存在"));
        }
        std::fs::rename(&source, &target).map_err(|e| format!("重命名动作文件失败：{e}"))?;
        Ok(target.to_string_lossy().to_string())
    })();
    ok_error(result, "path")
}

pub fn exp3_files_json(model_path: &str) -> String {
    match pet_model_dir(model_path) {
        Ok(dir) => ok_error(list_exp3_files(&dir.to_string_lossy()), "files"),
        Err(e) => json!({ "ok": false, "error": e }).to_string(),
    }
}

pub fn scan_folder_json(model_path: &str) -> String {
    let result = pet_model_dir(model_path)
        .and_then(|dir| scan_model_folder(&dir.to_string_lossy()));
    ok_error(result, "scan")
}

/// 给配置 JSON 附加 `extraMotions`（模型目录里的动作清单；模型为空 ⇒ 空数组）。
///
/// 这不是配置字段（不会落盘）：只是「每次套用配置时按当前模型目录重扫一遍」，
/// 让桌宠窗的引擎把 `*.motion3.json` 注入 `Lunac` 合成组，序号与「预设动作」页一致。
pub fn config_with_extra_motions(cfg: &PetConfig) -> Value {
    let mut value = serde_json::to_value(cfg).unwrap_or(Value::Null);
    let extras = if cfg.model.trim().is_empty() {
        json!([])
    } else {
        match pet_model_dir(&cfg.model) {
            Ok(dir) => match list_motion_files(&dir.to_string_lossy()) {
                Ok(files) => json!(files),
                Err(_) => json!([]),
            },
            Err(_) => json!([]),
        }
    };
    if let Value::Object(map) = &mut value {
        map.insert("extraMotions".to_string(), extras);
    }
    value
}

/// 发给桌宠 WebView 的 `apply` 消息（配置 + 动作清单）。
pub fn config_payload(cfg: &PetConfig) -> String {
    json!({ "type": "apply", "config": config_with_extra_motions(cfg) }).to_string()
}

/// `model3.json` → `FileReferences.Physics` → `physics3.json` 的 `Output[].Destination.Id`。
///
/// 这就是**物理输出参数**：它们每帧被物理写值，动作模板里给它们铺曲线无效
/// （v1 `listPhysicsOutputs` 同口径；「新建动作」默认排除，opt-in 才包含）。
pub fn physics_output_ids(model_path: &str) -> Vec<String> {
    let Ok(dir) = pet_model_dir(model_path) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(model_path) else {
        return Vec::new();
    };
    let Ok(definition) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let relative = definition
        .get("FileReferences")
        .and_then(|value| value.get("Physics"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if relative.is_empty() {
        return Vec::new();
    }
    let Ok(raw) = std::fs::read_to_string(dir.join(relative)) else {
        return Vec::new();
    };
    let Ok(physics) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    if let Some(settings) = physics.get("PhysicsSettings").and_then(Value::as_array) {
        for setting in settings {
            if let Some(outputs) = setting.get("Output").and_then(Value::as_array) {
                for output in outputs {
                    let Some(id) = output
                        .get("Destination")
                        .and_then(|value| value.get("Id"))
                        .and_then(Value::as_str)
                    else {
                        continue;
                    };
                    let id = id.trim();
                    if !id.is_empty() && !out.iter().any(|existing| existing == id) {
                        out.push(id.to_string());
                    }
                }
            }
        }
    }
    out
}

pub fn physics_outputs_json(model_path: &str) -> String {
    json!({ "ok": true, "ids": physics_output_ids(model_path) }).to_string()
}


// ══════════════════════════════════════════════════════════════════
//  模型定义（`*.model3.json`）校验
// ══════════════════════════════════════════════════════════════════

/// 导入前 / 控制台展示用的模型摘要（对齐 v1 `validateModelFile` 的返回信息）。
#[derive(Clone, Debug, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub ok: bool,
    pub version: f64,
    pub textures: usize,
    pub motions: usize,
    pub expressions: usize,
    /// 模型声明的表情名（`FileReferences.Expressions[].Name`）—— 「模型调试」页直接列成按钮。
    pub expression_names: Vec<String>,
    /// 动作组名（`FileReferences.Motions` 的键）。
    pub motion_groups: Vec<String>,
    /// 失败原因 / 成功时的动作组摘要。
    pub reason: String,
}

/// 解析并校验 `*.model3.json`（Cubism 4：`Version >= 3` 且 `FileReferences.Moc` 非空）。
/// **纯逻辑**（除读文件外无副作用），可单测。
pub fn model_info_from_text(text: &str) -> ModelInfo {
    let parsed: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => {
            return ModelInfo {
                reason: "模型定义不是合法 JSON".to_string(),
                ..Default::default()
            }
        }
    };
    let version = parsed
        .get("Version")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let fr = parsed.get("FileReferences");
    let moc = fr
        .and_then(|f| f.get("Moc"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if moc.trim().is_empty() || version < 3.0 {
        return ModelInfo {
            version,
            reason: "这不是 Cubism 4 的模型定义（缺 FileReferences.Moc 或 Version < 3）—— 请选 *.model3.json"
                .to_string(),
            ..Default::default()
        };
    }
    let textures = fr
        .and_then(|f| f.get("Textures"))
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let expressions = fr
        .and_then(|f| f.get("Expressions"))
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let motions_map = fr.and_then(|f| f.get("Motions")).and_then(|v| v.as_object());
    let motions = motions_map.map(|m| m.len()).unwrap_or(0);
    let groups = motions_map
        .map(|m| m.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    // 表情名：引擎的 `model.expression(name)` 按 `Expressions[].Name` 查表（v1 同口径）。
    let expression_names = fr
        .and_then(|f| f.get("Expressions"))
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("Name").and_then(|n| n.as_str()))
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ModelInfo {
        ok: true,
        version,
        textures,
        motions,
        expressions,
        expression_names,
        motion_groups: groups.clone(),
        reason: if groups.is_empty() {
            "无动作组".to_string()
        } else {
            groups.join(" / ")
        },
    }
}

pub fn read_model_info(path: &std::path::Path) -> ModelInfo {
    match std::fs::read_to_string(path) {
        Ok(text) => model_info_from_text(&text),
        Err(e) => ModelInfo {
            reason: format!("读不到模型文件：{e}"),
            ..Default::default()
        },
    }
}

// ══════════════════════════════════════════════════════════════════
//  资源路径（hex 编解码 + 放行范围）—— host 页与 Rust 侧同一套规则
// ══════════════════════════════════════════════════════════════════

/// UTF-8 字节 → 小写十六进制（host 页 JS 用同一算法）。
/// 生产侧只解码（[`decode_fs_path`]）；编码只用于单测验证两边一致。
#[cfg(test)]
pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

pub fn hex_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    Some(out)
}

/// `/fs/<hex>` → 磁盘路径。
pub fn decode_fs_path(hex: &str) -> Option<std::path::PathBuf> {
    let bytes = hex_decode(hex)?;
    let text = String::from_utf8(bytes).ok()?;
    if text.is_empty() {
        return None;
    }
    Some(std::path::PathBuf::from(text))
}

fn lower(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('/', "\\").to_lowercase()
}

/// 只放行「当前形象」相关的文件：
///   · 图片模式：图片本体；
///   · Live2D 模式：模型目录下的任意文件（模型 JSON / 纹理 / 动作 / 表情）。
/// 其它路径一律拒绝（host 页在 WebView2 里，必须当成不可信输入）。
pub fn path_allowed(cfg: &PetConfig, path: &std::path::Path) -> bool {
    let target = lower(path);
    if cfg.is_live2d() {
        let model = std::path::Path::new(&cfg.model);
        let dir = match model.parent() {
            Some(d) => lower(d),
            None => return false,
        };
        // 必须落在模型目录内（父目录 + 分隔符），或正好是模型文件本身。
        return target == lower(model) || target.starts_with(&(dir + "\\"));
    }
    if cfg.image.is_empty() {
        return false;
    }
    target == lower(std::path::Path::new(&cfg.image))
}

/// 自定义源的 host 页地址（与 [`HOST_HTML`] 里的资源地址同源）。
pub const HOST_ORIGIN: &str = "https://lunac.pet";
pub const HOST_PAGE_URL: &str = "https://lunac.pet/host.html";

pub fn mime_of(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

/// host 页面：透明背景 + 图片 / Live2D 两条渲染路径 + 右键菜单 + 拖动 / 滚轮缩放。
///
/// 与 v1 `pet.ts` 的对齐点：
///   · 形象二选一（`model` 优先）、空状态提示、图片「呼吸感」、滚轮以指针为锚缩放；
///   · 左键按住拖动窗口（v1 `getCurrentWindow().startDragging()`）；未拖动的一次点击
///     播放模型自带的第一个动作组（v1 用热键播动作，这里给一个无需控制台的入口）；
///   · 右键菜单：打开控制台 / 隐藏桌宠 / 退出桌宠（v1 菜单的「设置 / 关闭」两项的 v2 版）。
pub const HOST_HTML: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>
html,body{margin:0;padding:0;width:100%;height:100%;background:transparent;overflow:hidden;
  -webkit-user-select:none;user-select:none;}
#stage{position:fixed;inset:0;display:flex;align-items:flex-end;justify-content:center;cursor:grab;}
#stage.dragging{cursor:grabbing;}
#img{display:none;max-width:100%;max-height:100%;object-fit:contain;object-position:bottom center;
  transform-origin:50% 100%;pointer-events:none;-webkit-user-drag:none;
  filter:drop-shadow(0 6px 14px rgba(0,0,0,.35));}
#l2d{position:absolute;inset:0;display:none;}
#l2d canvas{display:block;}
#empty{position:absolute;left:50%;bottom:38%;transform:translateX(-50%);text-align:center;
  font:12px/1.7 system-ui,'Microsoft YaHei',sans-serif;color:rgba(240,240,240,.85);
  background:rgba(32,32,36,.72);border:1px solid rgba(255,255,255,.16);border-radius:10px;
  padding:9px 13px;display:none;backdrop-filter:blur(14px);}
#empty span{font-size:11px;opacity:.75;}
#err{position:fixed;left:8px;right:8px;bottom:8px;display:none;font:11px/1.55 system-ui,sans-serif;
  color:#ff9a9a;background:rgba(30,30,34,.94);border:1px solid rgba(255,120,120,.42);
  border-radius:8px;padding:6px 9px;word-break:break-all;}
#menu{position:absolute;display:none;flex-direction:column;min-width:132px;padding:4px;
  background:rgba(38,38,44,.97);border:1px solid rgba(255,255,255,.18);border-radius:9px;
  box-shadow:0 8px 24px rgba(0,0,0,.4);font:12px system-ui,'Microsoft YaHei',sans-serif;color:#eee;}
#menu.open{display:flex;}
#menu button{appearance:none;background:none;border:0;text-align:left;font:inherit;color:#eee;
  padding:6px 10px;border-radius:6px;cursor:pointer;white-space:nowrap;}
#menu button:hover{background:rgba(255,255,255,.12);}
#menu button.danger:hover{color:#ff8a8a;}
</style></head><body>
<div id="stage">
  <div id="l2d"></div>
  <img id="img" alt="" draggable="false">
  <div id="empty">还没有形象<br><span>在控制台里选一张图片或一个 Live2D 模型</span></div>
</div>
<div id="err"></div>
<div id="menu">
  <button data-act="console">打开控制台</button>
  <button data-act="through">鼠标穿透</button>
  <button data-act="hide">隐藏桌宠</button>
  <button data-act="quit" class="danger">退出桌宠</button>
</div>
<script>
'use strict';
// ── 与宿主同源的资源地址（规则与 rust/src/pet.rs 的 hex_encode 一致）──
var ENC = new TextEncoder();
function fsUrl(p) {
  var bytes = ENC.encode(p), out = '';
  for (var i = 0; i < bytes.length; i++) out += bytes[i].toString(16).padStart(2, '0');
  return 'https://lunac.pet/fs/' + out;
}
// v1 的引擎 bundle 在模块求值时会读这两个 Tauri 注入的成员：
//   · convertFileSrc(绝对路径) → 走我们的自定义源（同源，fetch / 纹理都能过）；
//   · invoke('list_expression_files') 我们没有宿主命令 ⇒ 明确拒绝，引擎会降级为
//     「只用 model3.json 声明的表情」（见 live2d.ts 的 try/catch）。
window.__TAURI_INTERNALS__ = {
  convertFileSrc: fsUrl,
  invoke: function () { return Promise.reject(new Error('lunac pet host: invoke 不可用')); }
};

var cfg = null;
var enginePromise = null;
var handle = null;
var mountedKey = '';
var motionGroup = '';
var rafId = 0;
var t0 = performance.now();
var dragging = false;

function post(o) { try { window.chrome.webview.postMessage(o); } catch (e) {} }
function $(id) { return document.getElementById(id); }
function showErr(msg) {
  var el = $('err');
  el.textContent = String(msg || '未知错误');
  el.style.display = 'block';
  post({ type: 'error', message: String(msg || '') });
}
function clearErr() { $('err').style.display = 'none'; }

// ── 引擎（与 v1 完全同一份：Core 先注入，再动态 import 渲染器）──
function loadEngine() {
  if (enginePromise) return enginePromise;
  enginePromise = new Promise(function (resolve, reject) {
    var s = document.createElement('script');
    s.src = 'https://lunac.pet/engine/live2dcubismcore.min.js';
    s.async = false;
    s.onload = function () { resolve(); };
    s.onerror = function () { reject(new Error('Cubism Core 加载失败（<exe>\\Modules\\pet\\engine\\ 下缺文件）')); };
    document.head.appendChild(s);
  }).then(function () {
    return import('https://lunac.pet/engine/live2d-engine.js');
  });
  return enginePromise;
}

function stopBreath() { if (rafId) cancelAnimationFrame(rafId); rafId = 0; }
function startBreath() {
  stopBreath();
  var img = $('img');
  function tick(now) {
    rafId = requestAnimationFrame(tick);
    if (!cfg || cfg.model || !cfg.image) return;
    var wobble = Math.sin((now - t0) / 1400) * 3;
    // translate 放在 scale 之前 ⇒ 位移是屏幕像素（与 v1 imgTransform 同口径）。
    img.style.transform = 'translate(' + cfg.offsetX + 'px,' + (cfg.offsetY + wobble) + 'px) scale(' + cfg.scale + ')';
  }
  rafId = requestAnimationFrame(tick);
}

function destroyModel() {
  if (handle) { try { handle.destroy(); } catch (e) {} }
  handle = null;
  mountedKey = '';
  motionGroup = '';
  var l2d = $('l2d');
  l2d.innerHTML = '';
  l2d.style.display = 'none';
}

function applyImage(c) {
  destroyModel();
  $('empty').style.display = 'none';
  var img = $('img');
  img.src = fsUrl(c.image);
  img.style.display = 'block';
  img.style.transform = 'translate(' + c.offsetX + 'px,' + c.offsetY + 'px) scale(' + c.scale + ')';
  startBreath();
}

async function applyModel(c) {
  var l2d = $('l2d');
  if (handle && mountedKey === c.model) {
    handle.setScale(c.scale);
    handle.setOffset(c.offsetX, c.offsetY);
    handle.setRotation(c.rotation);
    return;
  }
  stopBreath();
  $('img').style.display = 'none';
  $('empty').style.display = 'none';
  destroyModel();
  l2d.style.display = 'block';
  var mod = await loadEngine();
  handle = await mod.mountLive2D(l2d, {
    modelPath: c.model,
    scale: c.scale,
    offsetX: c.offsetX,
    offsetY: c.offsetY,
    rotation: c.rotation,
    // 「预设动作」页扫到的 *.motion3.json：按清单顺序注入 `Lunac` 合成组，下标即行号。
    extraMotions: c.extraMotions || [],
    // 与 v1 缺省窗口下的参考模型区同值（pet-window-size 的 STAGE_W / STAGE_H）。
    refStageW: 300,
    refStageH: 400
  });
  mountedKey = c.model;
  // 点击互动：优先用模型自己声明的第一个动作组（v1 靠热键播动作；这里给一个无控制台入口）。
  motionGroup = '';
  try {
    var def = await (await fetch(fsUrl(c.model))).json();
    var groups = Object.keys((def.FileReferences && def.FileReferences.Motions) || {});
    if (groups.length) motionGroup = groups[0];
  } catch (e) {}
  // 恢复该模型持久化的表情 / 参数（v1 同口径：按模型路径分键，切回旧模型时恢复它自己的）。
  try {
    var ex = (c.expressions && c.expressions[c.model]) || '';
    if (ex) handle.setExpression(ex);
    var ps = (c.params && c.params[c.model]) || {};
    for (var id in ps) {
      if (Object.prototype.hasOwnProperty.call(ps, id)) handle.setParameter(id, Number(ps[id]) || 0);
    }
  } catch (e) {}
}

function applyConfig(c) {
  cfg = c;
  clearErr();
  $('menu').classList.remove('open');
  var hasModel = !!(c.model && c.model.trim());
  var hasImage = !!(c.image && c.image.trim());
  if (hasModel) {
    applyModel(c).catch(function (e) { showErr('Live2D 载入失败：' + (e && e.message ? e.message : e)); });
    return;
  }
  if (hasImage) { applyImage(c); return; }
  destroyModel();
  $('img').style.display = 'none';
  $('empty').style.display = 'block';
  stopBreath();
}

// ── 交互：拖动窗口 / 点击播放动作 / 滚轮缩放 / 右键菜单 ──
var downAt = null, moved = false;
var stage = $('stage');
stage.addEventListener('mousedown', function (ev) {
  if (ev.button !== 0) return;
  downAt = { x: ev.clientX, y: ev.clientY };
  moved = false;
  $('menu').classList.remove('open');
});
window.addEventListener('mousemove', function (ev) {
  if (!downAt || moved) return;
  if (Math.abs(ev.clientX - downAt.x) + Math.abs(ev.clientY - downAt.y) > 4) {
    moved = true;
    stage.classList.add('dragging');
    post({ type: 'drag' });
  }
});
window.addEventListener('mouseup', function () {
  if (downAt && !moved && handle && motionGroup) {
    try { handle.playMotion(motionGroup, 0); } catch (e) {}
  }
  downAt = null;
  moved = false;
  stage.classList.remove('dragging');
});

stage.addEventListener('wheel', function (ev) {
  if (!cfg) return;
  if (!cfg.model && !cfg.image) return;
  ev.preventDefault();
  var factor = ev.deltaY < 0 ? 1.1 : 1 / 1.1;
  var next = Math.min(8, Math.max(0.4, cfg.scale * factor));
  if (next === cfg.scale) return;
  cfg.scale = next;
  if (handle) handle.setScale(next);
  else if (cfg.image) {
    var img = $('img');
    img.style.transform = 'translate(' + cfg.offsetX + 'px,' + cfg.offsetY + 'px) scale(' + next + ')';
  }
  post({ type: 'scale', value: next });
}, { passive: false });

// ── 右键菜单（含穿透态 Alt+右键唤出） ──
// 菜单因「穿透态 Alt+右键」而临时关掉穿透 ⇒ 关菜单时必须恢复成原状，否则用户点一次
// 右键就永久失去穿透（v1 `menuThroughPaused` 同语义）。
var menuThroughPaused = false;
function updateMenuThrough() {
  var b = document.querySelector('#menu button[data-act="through"]');
  if (b) b.textContent = '鼠标穿透：' + (cfg && cfg.clickThrough ? '开' : '关');
}
function openMenu(x, y) {
  var menu = $('menu');
  updateMenuThrough();
  menu.style.left = Math.max(2, Math.min(x, window.innerWidth - 140)) + 'px';
  menu.style.top = Math.max(2, Math.min(y, window.innerHeight - 170)) + 'px';
  menu.classList.add('open');
}
function closeMenu() {
  $('menu').classList.remove('open');
  if (menuThroughPaused) {
    menuThroughPaused = false;
    if (cfg) cfg.clickThrough = true;
    // 只恢复窗口样式与钩子，不落盘（这次暂停本来就不是用户改配置）。
    post({ type: 'through', value: true, persist: false });
  }
}
stage.addEventListener('contextmenu', function (ev) {
  ev.preventDefault();
  openMenu(ev.clientX, ev.clientY);
});
document.addEventListener('mousedown', function (ev) {
  var menu = $('menu');
  if (menu.classList.contains('open') && !menu.contains(ev.target)) closeMenu();
});
$('menu').addEventListener('click', function (ev) {
  var act = ev.target && ev.target.getAttribute ? ev.target.getAttribute('data-act') : '';
  if (!act) return;
  if (act === 'through') {
    // 用户显式改了穿透 ⇒ 之前的「临时暂停」语义作废（关菜单时不再自动恢复）。
    menuThroughPaused = false;
    var on = !(cfg && cfg.clickThrough);
    if (cfg) cfg.clickThrough = on;
    updateMenuThrough();
    post({ type: 'through', value: on, persist: true });
    // 开了穿透就再也点不到菜单了：直接收起，免得一块菜单「卡」在屏幕上。
    if (on) closeMenu();
    return;
  }
  closeMenu();
  post({ type: act });
});

// ── 宿主 → 页面 ──
window.chrome.webview.addEventListener('message', function (ev) {
  var m = ev.data;
  if (typeof m === 'string') { try { m = JSON.parse(m); } catch (e) { return; } }
  if (!m) return;
  if (m.type === 'apply') { applyConfig(m.config); return; }
  if (m.type === 'alt-rclick') {
    // 穿透态下宿主全局钩子的 Alt+右键：先临时关穿透（同时停钩子），再唤出菜单。
    var ax = Number(m.x), ay = Number(m.y);
    if (!isFinite(ax) || !isFinite(ay)) return;
    if (cfg && cfg.clickThrough && !menuThroughPaused) {
      menuThroughPaused = true;
      post({ type: 'through', value: false, persist: false });
    }
    openMenu(ax, ay);
    return;
  }
  if (m.type === 'debug') { applyDebug(m.action); return; }
  if (m.type === 'coreIds') {
    try {
      post({ type: 'coreIds', ids: handle ? handle.listCoreIds() : null });
    } catch (e) {
      post({ type: 'coreIds', ids: null, error: String((e && e.message) || e) });
    }
    return;
  }
  if (m.type === 'motion') {
    if (!handle) { post({ type: 'motion', ok: false, error: '模型还没载入' }); return; }
    try {
      // 合成组名与 v1 `MOTION_EXTRA_GROUP` 一致；index 即 `list_motion_files` 内的位置。
      handle.playMotion('Lunac', Number(m.index) || 0);
      post({ type: 'motion', ok: true });
    } catch (e) {
      post({ type: 'motion', ok: false, error: String((e && e.message) || e) });
    }
    return;
  }
});

// 模型调试（v1「模型调试」页的事件同义）：换表情 / 钉参数。
// 引擎句柄与 v1 同一份（`live2d-engine.js`），方法名逐字一致。
function applyDebug(a) {
  if (!a || typeof a !== 'object') return;
  if (!handle) { post({ type: 'debug', ok: false, error: '模型还没载入' }); return; }
  try {
    if (typeof a.expression === 'string') {
      if (a.expression) handle.setExpression(a.expression);
      else handle.resetExpressions();
    }
    if (a.param && typeof a.param.id === 'string' && a.param.id) {
      handle.setParameter(a.param.id, Number(a.param.value) || 0);
    }
    post({ type: 'debug', ok: true });
  } catch (e) {
    post({ type: 'debug', ok: false, error: String((e && e.message) || e) });
  }
}

// 页面准备好了：向宿主要当前配置（宿主也在 NavigationCompleted 时补发一次，双保险）。
post({ type: 'ready' });
</script>
</body></html>
"#;

// ══════════════════════════════════════════════════════════════════
//  跨线程状态（窗口线程 ↔ Flutter 主线程）
// ══════════════════════════════════════════════════════════════════

static PET_HWND: AtomicIsize = AtomicIsize::new(0);
static PET_OPEN: AtomicBool = AtomicBool::new(false);
static PET_ERROR: Mutex<Option<String>> = Mutex::new(None);
static PET_EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// 待转发给 WebView 的「模型调试」指令（Flutter 线程写、窗口线程取）。
static PET_DEBUG_OUT: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// 待转发给 WebView 的「穿透态 Alt+右键」**屏幕坐标**（鼠标钩子线程写、窗口线程取）。
///
/// 换算成页面 CSS 坐标需要窗口矩形与 DPI，那两样只有窗口线程知道（见
/// `WM_PET_ALT_RCLICK` 分支），所以这里只存原始屏幕坐标。
static ALT_RCLICKS: Mutex<Vec<(i32, i32)>> = Mutex::new(Vec::new());

fn exe_root() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

fn push_event(value: Value) {
    if let Ok(mut list) = PET_EVENTS.lock() {
        list.push(value.to_string());
    }
}

fn set_error(message: impl Into<String>) {
    let message = message.into();
    // 桌宠窗是独立线程：失败原因必须能落到 stderr（`flutter run` / 排障时看得到）。
    eprintln!("[lunac-pet] {message}");
    if let Ok(mut slot) = PET_ERROR.lock() {
        *slot = Some(message.clone());
    }
    push_event(json!({ "type": "error", "message": message }));
}

/// 运行时状态（控制台 / 测试用）。
pub fn state_json() -> String {
    let error = PET_ERROR.lock().ok().and_then(|e| e.clone());
    json!({
        "open": PET_OPEN.load(Ordering::SeqCst),
        "error": error,
    })
    .to_string()
}

/// 配置 JSON（控制台读取）。
pub fn config_json() -> String {
    serde_json::to_string(&load_config(&exe_root())).unwrap_or_else(|_| "{}".to_string())
}

/// 合并保存：调用方只传关心的字段，其余沿用盘上那份。
///
/// 返回保存后的完整配置 JSON；坏 JSON 返回 `{"error": "..."}`。
pub fn apply_json(patch: &str) -> String {
    let root = exe_root();
    let current = load_config(&root);
    let mut merged = match serde_json::to_value(&current) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    let incoming: Value = match serde_json::from_str(patch) {
        Ok(v) => v,
        Err(e) => return json!({ "error": format!("配置不是合法 JSON：{e}") }).to_string(),
    };
    if let Value::Object(map) = incoming {
        for (k, v) in map {
            merged.insert(k, v);
        }
    } else {
        return json!({ "error": "配置必须是 JSON 对象" }).to_string();
    }
    let text = Value::Object(merged).to_string();
    let cfg = parse_config(&text);
    if let Err(e) = save_config(&root, &cfg) {
        return json!({ "error": e }).to_string();
    }
    notify_window(WM_PET_APPLY, 0);
    serde_json::to_string(&cfg).unwrap_or_else(|_| "{}".to_string())
}

fn notify_window(msg: u32, wparam: usize) {
    let raw = PET_HWND.load(Ordering::SeqCst);
    if raw == 0 {
        return;
    }
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
        let hwnd = HWND(raw as *mut std::ffi::c_void);
        let _ = PostMessageW(Some(hwnd), msg, WPARAM(wparam), LPARAM(0));
    }
    #[cfg(not(windows))]
    let _ = (msg, wparam);
}

/// 鼠标钩子命中「Alt+右键」时由 `mouse_hook` 的消息泵线程调用（窗口线程会换来
/// 页面 CSS 坐标再投给 WebView）。
///
/// 窗口不在时直接丢弃 —— 没有窗就没有菜单可唤出，攒着只会恢复时突然弹一串。
pub fn deliver_alt_rclick(x: i32, y: i32) {
    if PET_HWND.load(Ordering::SeqCst) == 0 {
        return;
    }
    if let Ok(mut list) = ALT_RCLICKS.lock() {
        list.push((x, y));
    } else {
        return;
    }
    notify_window(WM_PET_ALT_RCLICK, 0);
}

/// 显示桌宠（没有窗口就先起线程）。返回当前状态 JSON。
pub fn show() -> String {
    let root = exe_root();
    let mut cfg = load_config(&root);
    cfg.visible = true;
    let _ = save_config(&root, &cfg);
    if PET_HWND.load(Ordering::SeqCst) == 0 {
        spawn_window();
    } else {
        notify_window(WM_PET_SHOW, 0);
    }
    state_json()
}

/// 隐藏桌宠（保留窗口与引擎）。
pub fn hide() -> String {
    let root = exe_root();
    let mut cfg = load_config(&root);
    cfg.visible = false;
    let _ = save_config(&root, &cfg);
    notify_window(WM_PET_HIDE, 0);
    state_json()
}

/// 退出桌宠：销毁窗口与 WebView2（进程内的线程随之结束）。
pub fn quit() -> String {
    let root = exe_root();
    let mut cfg = load_config(&root);
    cfg.visible = false;
    let _ = save_config(&root, &cfg);
    notify_window(WM_PET_QUIT, 0);
    state_json()
}

/// 设置穿透 / 置顶（先落盘，再让窗口线程套用）。
pub fn set_click_through(on: bool) -> String {
    let root = exe_root();
    let mut cfg = load_config(&root);
    cfg.click_through = on;
    let _ = save_config(&root, &cfg);
    notify_window(WM_PET_APPLY, 0);
    state_json()
}

pub fn set_topmost(on: bool) -> String {
    let root = exe_root();
    let mut cfg = load_config(&root);
    cfg.topmost = on;
    let _ = save_config(&root, &cfg);
    notify_window(WM_PET_APPLY, 0);
    state_json()
}

/// 重置位置：清掉记住的坐标，窗口线程会挪到默认位置（主屏右下）。
pub fn reset_position() -> String {
    let root = exe_root();
    let mut cfg = load_config(&root);
    cfg.pos_x = None;
    cfg.pos_y = None;
    let _ = save_config(&root, &cfg);
    notify_window(WM_PET_RESET_POS, 0);
    state_json()
}

/// 「模型调试」一条指令：表情 / 参数，转发给桌宠窗的 WebView，并按需落盘。
///
/// 入参是 JSON 对象：
/// - `{"expression":"smile"}` 换表情（空串 = 还原）；
/// - `{"param":{"id":"ParamAngleX","value":12}}` 钉一个参数值；
/// - `persist:false` = 只预览不落盘（拖滑块时用），缺省写盘。
///
/// 落盘键与 v1 一致：`expressions[<model>] / params[<model>][<id>]`（存在 `extra` 里原样保留）。
/// 桌宠窗没开时**仍然落盘**并如实回 `open:false`（v1：值会记住，显示后生效）。
pub fn debug_json(payload: &str) -> String {
    let value: Value = match serde_json::from_str(payload) {
        Ok(v @ Value::Object(_)) => v,
        _ => return json!({ "ok": false, "error": "调试指令必须是 JSON 对象" }).to_string(),
    };
    let root = exe_root();
    let mut cfg = load_config(&root);
    let model = cfg.model.clone();
    if model.is_empty() {
        return json!({ "ok": false, "error": "还没有选 Live2D 模型" }).to_string();
    }
    let persist = value.get("persist").and_then(|v| v.as_bool()).unwrap_or(true);
    let mut changed = false;
    if let Some(expression) = value.get("expression").and_then(|v| v.as_str()) {
        let mut map = cfg
            .extra
            .get("expressions")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let name = expression.trim();
        if name.is_empty() {
            changed = map.remove(&model).is_some();
        } else {
            map.insert(model.clone(), Value::String(name.to_string()));
            changed = true;
        }
        cfg.extra.insert("expressions".into(), Value::Object(map));
    }
    if let Some(param) = value.get("param") {
        let id = param
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let number = param.get("value").and_then(|v| v.as_f64());
        let Some(number) = number else {
            return json!({ "ok": false, "error": "参数指令缺 value" }).to_string();
        };
        if id.is_empty() {
            return json!({ "ok": false, "error": "参数指令缺 id" }).to_string();
        }
        let mut all = cfg
            .extra
            .get("params")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let mut one = all
            .get(&model)
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        one.insert(id, json!(number));
        all.insert(model.clone(), Value::Object(one));
        cfg.extra.insert("params".into(), Value::Object(all));
        changed = true;
    }
    if changed && persist {
        if let Err(e) = save_config(&root, &cfg) {
            return json!({ "ok": false, "error": e }).to_string();
        }
    }
    let open = PET_HWND.load(Ordering::SeqCst) != 0;
    if open {
        let message = json!({ "type": "debug", "action": value }).to_string();
        if let Ok(mut queue) = PET_DEBUG_OUT.lock() {
            queue.push(message);
        }
        notify_window(WM_PET_DEBUG, 0);
    }
    json!({ "ok": true, "error": "", "open": open }).to_string()
}

/// 事件队列（控制台 / 主页面轮询）：返回 JSON 数组字符串并清空。
pub fn poll_events() -> String {
    let list = PET_EVENTS
        .lock()
        .map(|mut l| std::mem::take(&mut *l))
        .unwrap_or_default();
    format!("[{}]", list.join(","))
}

/// 往桌宠 WebView 发一条消息（复用「模型调试」那条队列 + 窗口消息）。窗口没开返回 false。
pub fn send_page_message(value: Value) -> bool {
    let open = PET_HWND.load(Ordering::SeqCst) != 0;
    if open {
        if let Ok(mut queue) = PET_DEBUG_OUT.lock() {
            queue.push(value.to_string());
        }
        notify_window(WM_PET_DEBUG, 0);
    }
    open
}

/// 请求桌宠窗的引擎报一份「运行时参数表」（新建动作模板用）。
pub fn request_core_ids() -> String {
    let open = send_page_message(json!({ "type": "coreIds" }));
    json!({
        "ok": open,
        "open": open,
        "error": if open { "" } else { "桌宠窗没开：先显示一次桌宠，让引擎读出参数表" },
    })
    .to_string()
}

/// 播放 `Lunac` 合成组里的第 `index` 个动作（行号 = `list_motion_files` 顺序）。
pub fn play_motion(index: u32) -> String {
    let open = send_page_message(json!({ "type": "motion", "index": index }));
    json!({
        "ok": open,
        "open": open,
        "error": if open { "" } else { "桌宠窗没开" },
    })
    .to_string()
}

/// 弹出系统的「选择形象文件」对话框（v1 用 `@tauri-apps/plugin-dialog`，这里手写 Win32）。
/// 返回选中的绝对路径；取消 / 失败返回空串。
pub fn pick_file() -> String {
    #[cfg(windows)]
    unsafe {
        use windows::core::{PCWSTR, PWSTR};
        use windows::Win32::UI::Controls::Dialogs::{
            GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST,
            OPENFILENAMEW,
        };
        let filter: Vec<u16> = "形象图片 (*.png;*.jpg;*.jpeg;*.webp;*.gif)\0*.png;*.jpg;*.jpeg;*.webp;*.gif\0Live2D 模型 (*.model3.json)\0*.model3.json\0所有文件 (*.*)\0*.*\0\0"
            .encode_utf16()
            .collect();
        let mut buffer = vec![0u16; 4096];
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: PWSTR(buffer.as_mut_ptr()),
            nMaxFile: buffer.len() as u32,
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        if !GetOpenFileNameW(&mut ofn).as_bool() {
            return String::new();
        }
        let len = buffer.iter().position(|c| *c == 0).unwrap_or(0);
        String::from_utf16_lossy(&buffer[..len])
    }
    #[cfg(not(windows))]
    {
        String::new()
    }
}

pub fn model_info_json(path: &str) -> String {
    let info = read_model_info(std::path::Path::new(path));
    serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_string())
}

// ── 窗口线程的消息号 ─────────────────────────────────────────────
const WM_PET_APPLY: u32 = 0x8000 + 10; // WM_APP + 10
const WM_PET_SHOW: u32 = 0x8000 + 11;
const WM_PET_HIDE: u32 = 0x8000 + 12;
const WM_PET_QUIT: u32 = 0x8000 + 13;
const WM_PET_RESET_POS: u32 = 0x8000 + 16;
const WM_PET_DRAG: u32 = 0x8000 + 17;
const WM_PET_DEBUG: u32 = 0x8000 + 18;
const WM_PET_ALT_RCLICK: u32 = 0x8000 + 19;

fn spawn_window() {
    #[cfg(windows)]
    {
        let _ = std::thread::Builder::new()
            .name("lunac-pet".to_string())
            .spawn(|| {
                eprintln!("[lunac-pet] window thread started");
                if let Err(e) = windows_host::run() {
                    set_error(format!("桌宠窗启动失败：{e}"));
                    PET_OPEN.store(false, Ordering::SeqCst);
                    PET_HWND.store(0, Ordering::SeqCst);
                }
            });
    }
    #[cfg(not(windows))]
    {
        set_error("当前平台还没有桌宠窗口实现（仅 Windows）");
    }
}

#[cfg(windows)]
mod windows_host {
    use super::*;

    use std::cell::RefCell;

    use windows::core::{w, HSTRING, PCWSTR, Interface};
    use windows::Win32::Foundation::{
        COLORREF, E_POINTER, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
    };
    use windows::Win32::Graphics::Dwm::{DwmEnableBlurBehindWindow, DWM_BB_BLURREGION, DWM_BB_ENABLE, DWM_BLURBEHIND};
    use windows::Win32::Graphics::Gdi::{
        CreateRectRgn, GetMonitorInfoW, MonitorFromPoint, UpdateWindow, MONITORINFO,
        MONITOR_DEFAULTTOPRIMARY,
    };
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Shell::SHCreateMemStream;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
        GetMessageW, GetWindowLongPtrW, GetWindowRect, PostMessageW, PostQuitMessage,
        RegisterClassW, SendMessageW, SetLayeredWindowAttributes, SetWindowLongPtrW,
        SetWindowPos, ShowWindow, TranslateMessage, GWL_EXSTYLE, HTTRANSPARENT, HWND_NOTOPMOST,
        HWND_TOPMOST, LWA_ALPHA, MA_NOACTIVATE, MSG, SET_WINDOW_POS_FLAGS, SW_HIDE,
        SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_MOUSEACTIVATE,
        WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_DESTROY, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };

    use webview2_com::Microsoft::Web::WebView2::Win32::{
        CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2, ICoreWebView2Controller,
        ICoreWebView2Environment, ICoreWebView2EnvironmentOptions, COREWEBVIEW2_COLOR,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
    };
    use webview2_com::{
        CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
        CreateCoreWebView2EnvironmentCompletedHandler, WebMessageReceivedEventHandler,
        WebResourceRequestedEventHandler,
    };

    /// 窗口线程内部错误：windows / webview2-com 两种错误源都收进来。
    #[derive(Debug)]
    enum HostError {
        Windows(windows::core::Error),
        WebView2(webview2_com::Error),
        Recv,
    }

    impl std::fmt::Display for HostError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                HostError::Windows(e) => write!(f, "{e}"),
                HostError::WebView2(e) => write!(f, "{e:?}"),
                HostError::Recv => write!(f, "WebView2 回调通道已关闭"),
            }
        }
    }

    impl From<windows::core::Error> for HostError {
        fn from(e: windows::core::Error) -> Self {
            HostError::Windows(e)
        }
    }

    impl From<webview2_com::Error> for HostError {
        fn from(e: webview2_com::Error) -> Self {
            HostError::WebView2(e)
        }
    }

    thread_local! {
        /// 窗口线程自己的 WebView2 状态（COM 接口都不是 Send，只能留在本线程）。
        static UI: RefCell<UiState> = RefCell::new(UiState::default());
    }

    #[derive(Default)]
    struct UiState {
        webview: Option<ICoreWebView2>,
        environment: Option<ICoreWebView2Environment>,
        config: PetConfig,
        page_ready: bool,
    }

    fn dpi_scale(hwnd: HWND) -> f64 {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if dpi == 0 {
            1.0
        } else {
            dpi as f64 / 96.0
        }
    }

    /// 主屏工作区里挑一个默认位置：右下角（留 48px 边距），与常见桌宠位置一致。
    fn default_rect(cfg: &PetConfig, scale: f64) -> (i32, i32, i32, i32) {
        let w = (cfg.win_w * scale).round() as i32;
        let h = (cfg.win_h * scale).round() as i32;
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let monitor = unsafe { MonitorFromPoint(windows::Win32::Foundation::POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
        let mut right = 1280;
        let mut bottom = 800;
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            right = info.rcWork.right;
            bottom = info.rcWork.bottom;
        }
        let x = (right - w - 48).max(0);
        let y = (bottom - h - 48).max(0);
        (x, y, w, h)
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            // 透明窗：不要擦背景（否则会闪一块白）。
            WM_ERASEBKGND => LRESULT(1),
            // 点桌宠不抢主窗焦点（与附件抽屉同口径）。
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_NCHITTEST => {
                let through = UI.with(|ui| ui.borrow().config.click_through);
                if through {
                    return LRESULT(HTTRANSPARENT as isize);
                }
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            WM_PET_APPLY => {
                apply_from_disk(hwnd);
                return LRESULT(0);
            }
            WM_PET_SHOW => {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                UI.with(|ui| ui.borrow_mut().config.visible = true);
                return LRESULT(0);
            }
            WM_PET_HIDE => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                UI.with(|ui| ui.borrow_mut().config.visible = false);
                return LRESULT(0);
            }
            WM_PET_RESET_POS => {
                let cfg = UI.with(|ui| ui.borrow().config.clone());
                let scale = dpi_scale(hwnd);
                let (x, y, w, h) = default_rect(&cfg, scale);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    x,
                    y,
                    w,
                    h,
                    SET_WINDOW_POS_FLAGS(0x0004 | 0x0010), // SWP_NOZORDER | SWP_NOACTIVATE
                );
                save_position(hwnd);
                return LRESULT(0);
            }
            WM_PET_DRAG => {
                drag_window(hwnd);
                return LRESULT(0);
            }
            WM_PET_DEBUG => {
                let messages = PET_DEBUG_OUT
                    .lock()
                    .map(|mut queue| std::mem::take(&mut *queue))
                    .unwrap_or_default();
                UI.with(|ui| {
                    if let Some(webview) = &ui.borrow().webview {
                        for payload in &messages {
                            let wide = HSTRING::from(payload.as_str());
                            let _ = unsafe { webview.PostWebMessageAsJson(&wide) };
                        }
                    }
                });
                return LRESULT(0);
            }
            WM_PET_ALT_RCLICK => {
                // 钩子给的是屏幕物理像素：换成窗口内 CSS 像素（物理 ÷ DPI 缩放）。
                let points = ALT_RCLICKS
                    .lock()
                    .map(|mut queue| std::mem::take(&mut *queue))
                    .unwrap_or_default();
                let mut rect = RECT::default();
                let _ = unsafe { GetWindowRect(hwnd, &mut rect) };
                let scale = dpi_scale(hwnd);
                UI.with(|ui| {
                    if let Some(webview) = &ui.borrow().webview {
                        for (sx, sy) in points {
                            let x = ((sx - rect.left) as f64 / scale).round() as i32;
                            let y = ((sy - rect.top) as f64 / scale).round() as i32;
                            let payload = json!({
                                "type": "alt-rclick",
                                "x": x,
                                "y": y,
                            })
                            .to_string();
                            let _ = unsafe {
                                webview.PostWebMessageAsJson(&HSTRING::from(payload))
                            };
                        }
                    }
                });
                return LRESULT(0);
            }
            WM_PET_QUIT => {
                PET_OPEN.store(false, Ordering::SeqCst);
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            }
            WM_EXITSIZEMOVE => {
                save_position(hwnd);
                return LRESULT(0);
            }
            WM_DESTROY => {
                PET_OPEN.store(false, Ordering::SeqCst);
                PET_HWND.store(0, Ordering::SeqCst);
                // 进程内不得留悬挂钩子：回调是本进程的静态函数，卸载后系统打过来就崩。
                crate::mouse_hook::stop();
                push_event(json!({ "type": "closed" }));
                PostQuitMessage(0);
                return LRESULT(0);
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    fn save_position(hwnd: HWND) {
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
            return;
        }
        let root = exe_root();
        let mut cfg = load_config(&root);
        cfg.pos_x = Some(rect.left);
        cfg.pos_y = Some(rect.top);
        let _ = save_config(&root, &cfg);
        UI.with(|ui| ui.borrow_mut().config = cfg.clone());
        push_event(json!({ "type": "config", "config": cfg }));
    }

    fn drag_window(hwnd: HWND) {
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
            let _ = SendMessageW(
                hwnd,
                WM_NCLBUTTONDOWN,
                Some(WPARAM(2)), // HTCAPTION
                Some(LPARAM(0)),
            );
        }
    }

    fn apply_from_disk(hwnd: HWND) {
        let root = exe_root();
        let cfg = load_config(&root);
        apply_config(hwnd, cfg);
    }

    fn apply_config(hwnd: HWND, cfg: PetConfig) {
        let scale = dpi_scale(hwnd);
        let w = (cfg.win_w * scale).round() as i32;
        let h = (cfg.win_h * scale).round() as i32;
        let (x, y) = match (cfg.pos_x, cfg.pos_y) {
            (Some(px), Some(py)) => (px, py),
            _ => {
                let d = default_rect(&cfg, scale);
                (d.0, d.1)
            }
        };
        let top_insert = if cfg.topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(top_insert),
                x,
                y,
                w,
                h,
                SET_WINDOW_POS_FLAGS(0x0010), // SWP_NOACTIVATE（z 序由 hWndInsertAfter 决定）
            );
        }
        set_click_through_style(hwnd, cfg.click_through);
        // 穿透态下页面收不到鼠标消息 ⇒ Alt+右键只能靠全局钩子旁听（v1 同口径：
        // 两个开关同进同退；非穿透态保持关闭，幂等）。
        crate::mouse_hook::watch(cfg.click_through);
        if cfg.visible {
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        } else {
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        }
        resize_webview(hwnd);
        UI.with(|ui| {
            let mut state = ui.borrow_mut();
            state.config = cfg.clone();
            if let Some(webview) = &state.webview {
                let payload = config_payload(&cfg);
                let wide = HSTRING::from(payload);
                let _ = unsafe { webview.PostWebMessageAsJson(&wide) };
            }
        });
    }

    fn set_click_through_style(hwnd: HWND, on: bool) {
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let mut next = WINDOW_EX_STYLE(ex as u32);
            if on {
                next |= WS_EX_TRANSPARENT | WS_EX_LAYERED;
            } else {
                next &= !(WS_EX_TRANSPARENT | WS_EX_LAYERED);
            }
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next.0 as isize);
            if on {
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            }
        }
    }

    fn resize_webview(hwnd: HWND) {
        let mut client = RECT::default();
        if unsafe { GetClientRect(hwnd, &mut client) }.is_err() {
            return;
        }
        UI.with(|ui| {
            let state = ui.borrow();
            if let Some(env) = &state.environment {
                let _ = env; // 环境只用于创建响应；这里只调控制器。
            }
            if let Some(webview) = &state.webview {
                // 控制器随 webview 一起取（见 UiState::controller）。
                let _ = webview;
            }
        });
        if let Some(controller) = CONTROLLER.with(|c| c.borrow().clone()) {
            let _ = unsafe {
                controller.SetBounds(RECT {
                    left: 0,
                    top: 0,
                    right: client.right,
                    bottom: client.bottom,
                })
            };
        }
    }

    thread_local! {
        static CONTROLLER: RefCell<Option<ICoreWebView2Controller>> = RefCell::new(None);
    }

    /// 自定义源：`https://lunac.pet/...`。
    fn serve(uri: &str, cfg: &PetConfig) -> (u16, &'static str, Vec<u8>) {
        let path = uri
            .strip_prefix(HOST_ORIGIN)
            .unwrap_or(uri)
            .split(['?', '#'])
            .next()
            .unwrap_or("");
        if path == "/host.html" || path == "/" || path.is_empty() {
            return (200, "text/html; charset=utf-8", HOST_HTML.as_bytes().to_vec());
        }
        if let Some(rest) = path.strip_prefix("/engine/") {
            let engine_dir = exe_root().join("Modules").join("pet").join("engine");
            let file = engine_dir.join(rest);
            return match std::fs::read(&file) {
                Ok(bytes) => (200, mime_of(&file), bytes),
                Err(_) => (404, "text/plain; charset=utf-8", b"engine asset not found".to_vec()),
            };
        }
        if let Some(hex) = path.strip_prefix("/fs/") {
            if let Some(file) = decode_fs_path(hex) {
                if !path_allowed(cfg, &file) {
                    return (403, "text/plain; charset=utf-8", b"forbidden".to_vec());
                }
                return match std::fs::read(&file) {
                    Ok(bytes) => (200, mime_of(&file), bytes),
                    Err(_) => (404, "text/plain; charset=utf-8", b"not found".to_vec()),
                };
            }
        }
        (404, "text/plain; charset=utf-8", b"not found".to_vec())
    }

    fn install_webview(hwnd: HWND) -> Result<(), HostError> {
        let udf = exe_root().join("ModuleData").join("pet").join("webview2");
        let _ = std::fs::create_dir_all(&udf);

        let environment = {
            let (tx, rx) = std::sync::mpsc::channel();
            let options: ICoreWebView2EnvironmentOptions =
                CoreWebView2EnvironmentOptions::default().into();
            CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| unsafe {
                    CreateCoreWebView2EnvironmentWithOptions(
                        PCWSTR::null(),
                        &HSTRING::from(udf.to_string_lossy().to_string()),
                        &options,
                        &handler,
                    )
                    .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(move |error, env| {
                    error?;
                    tx.send(env.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                        .expect("send environment over mpsc channel");
                    Ok(())
                }),
            )?;
            rx.recv().map_err(|_| HostError::Recv)??
        };

        // 资源处理器 / 状态缓存都要用 environment；控制器闭包会 move 掉它，先克隆两份。
        let env_for_state = environment.clone();
        let env_for_handler = environment.clone();
        let controller = {
            let (tx, rx) = std::sync::mpsc::channel();
            CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| unsafe {
                    environment
                        .CreateCoreWebView2Controller(hwnd, &handler)
                        .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(move |error, controller| {
                    error?;
                    tx.send(controller.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                        .expect("send controller over mpsc channel");
                    Ok(())
                }),
            )?;
            rx.recv().map_err(|_| HostError::Recv)??
        };

        let webview = unsafe { controller.CoreWebView2()? };
        let mut client = RECT::default();
        let _ = unsafe { GetClientRect(hwnd, &mut client) };
        unsafe {
            // 真透明的关键：默认背景 alpha = 0（v1 的 WebView2 透明窗同款）。
            // 这个方法在 ICoreWebView2Controller2 上（控制器本体没有）。
            let controller2: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller2 =
                controller.cast()?;
            controller2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                R: 0,
                G: 0,
                B: 0,
                A: 0,
            })?;
            controller.SetBounds(RECT {
                left: 0,
                top: 0,
                right: client.right,
                bottom: client.bottom,
            })?;
            controller.SetIsVisible(true)?;
            let settings = webview.Settings()?;
            settings.SetAreDefaultContextMenusEnabled(false)?;
            settings.SetAreDevToolsEnabled(false)?;
            settings.SetIsStatusBarEnabled(false)?;
        }

        // 自定义源（host 页 + 引擎 + 用户形象）。
        {
            let handler = WebResourceRequestedEventHandler::create(Box::new(move |_sender, args| {
                if let Some(args) = args {
                    if let Ok(request) = unsafe { args.Request() } {
                        let mut raw_uri = windows::core::PWSTR::null();
                        if unsafe { request.Uri(&mut raw_uri) }.is_ok() {
                            let uri = webview2_com::CoTaskMemPWSTR::from(raw_uri).to_string();
                            let cfg = UI.with(|ui| ui.borrow().config.clone());
                            let (status, mime, bytes) = serve(&uri, &cfg);
                            let headers = HSTRING::from(format!(
                                "Content-Type: {mime}\r\nCache-Control: no-store\r\n"
                            ));
                            let stream = unsafe { SHCreateMemStream(Some(&bytes)) };
                            if let Ok(response) = unsafe {
                                env_for_handler.CreateWebResourceResponse(
                                    stream.as_ref(),
                                    status as i32,
                                    &HSTRING::from(if status == 200 { "OK" } else { "Error" }),
                                    &headers,
                                )
                            } {
                                let _ = unsafe { args.SetResponse(&response) };
                            }
                        }
                    }
                }
                Ok(())
            }));
            let mut token = 0;
            unsafe {
                webview.AddWebResourceRequestedFilter(
                    &HSTRING::from(format!("{HOST_ORIGIN}/*")),
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                )?;
                webview.add_WebResourceRequested(&handler, &mut token)?;
            }
        }

        // 页面消息：拖动 / 缩放 / 右键菜单 / 就绪。
        {
            let handler = WebMessageReceivedEventHandler::create(Box::new(move |_sender, args| {
                if let Some(args) = args {
                    let mut raw = windows::core::PWSTR::null();
                    if unsafe { args.WebMessageAsJson(&mut raw) }.is_ok() {
                        let message = webview2_com::CoTaskMemPWSTR::from(raw).to_string();
                        if let Ok(value) = serde_json::from_str::<Value>(&message) {
                            handle_page_message(hwnd, &value);
                        }
                    }
                }
                Ok(())
            }));
            let mut token = 0;
            unsafe { webview.add_WebMessageReceived(&handler, &mut token)? };
        }

        CONTROLLER.with(|c| *c.borrow_mut() = Some(controller.clone()));
        UI.with(|ui| {
            let mut state = ui.borrow_mut();
            state.webview = Some(webview.clone());
            state.environment = Some(env_for_state.clone());
        });

        unsafe {
            webview.Navigate(&HSTRING::from(HOST_PAGE_URL))?;
        }
        Ok(())
    }

    fn handle_page_message(hwnd: HWND, msg: &Value) {
        let kind = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "ready" => {
                UI.with(|ui| {
                    let mut state = ui.borrow_mut();
                    state.page_ready = true;
                    let cfg = state.config.clone();
                    if let Some(webview) = &state.webview {
                        let payload = config_payload(&cfg);
                        let _ = unsafe { webview.PostWebMessageAsJson(&HSTRING::from(payload)) };
                    }
                });
            }
            "drag" => {
                unsafe { let _ = PostMessageW(Some(hwnd), WM_PET_DRAG, WPARAM(0), LPARAM(0)); }
            }
            "scale" => {
                if let Some(value) = msg.get("value").and_then(|v| v.as_f64()) {
                    let root = exe_root();
                    let mut cfg = load_config(&root);
                    cfg.scale = value;
                    cfg = cfg.normalize();
                    let _ = save_config(&root, &cfg);
                    UI.with(|ui| ui.borrow_mut().config = cfg.clone());
                    push_event(json!({ "type": "config", "config": cfg }));
                }
            }
            "hide" => {
                let _ = hide();
            }
            "console" => push_event(json!({ "type": "console" })),
            "through" => {
                // 页面请求改穿透（菜单里那条 / 唤出菜单时的临时暂停）。
                // `persist=false` 只改窗口样式与钩子，不落盘 —— 临时暂停结束后按
                // 配置恢复（v1 `openMenuFromScreen` 的「临时关穿透」同语义）。
                let on = msg.get("value") == Some(&Value::Bool(true));
                let persist = msg.get("persist") == Some(&Value::Bool(true));
                set_click_through_style(hwnd, on);
                crate::mouse_hook::watch(on);
                UI.with(|ui| {
                    let mut state = ui.borrow_mut();
                    let mut cfg = state.config.clone();
                    cfg.click_through = on;
                    state.config = cfg;
                });
                if persist {
                    let root = exe_root();
                    let mut cfg = load_config(&root);
                    cfg.click_through = on;
                    let _ = save_config(&root, &cfg);
                    push_event(json!({ "type": "config", "config": cfg }));
                }
            }
            "coreIds" => {
                push_event(json!({
                    "type": "coreIds",
                    "ids": msg.get("ids").cloned().unwrap_or(Value::Null),
                    "error": msg.get("error").and_then(|v| v.as_str()).unwrap_or(""),
                }));
            }
            "motion" => {
                push_event(json!({
                    "type": "motion",
                    "ok": msg.get("ok") == Some(&Value::Bool(true)),
                    "error": msg.get("error").and_then(|v| v.as_str()).unwrap_or(""),
                }));
            }
            "quit" => {
                let _ = quit();
            }
            "error" => {
                let message = msg
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未知错误")
                    .to_string();
                set_error(message);
            }
            _ => {}
        }
    }

    pub(super) fn run() -> Result<(), String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }

        let root = exe_root();
        let cfg = load_config(&root);
        let scale = {
            // 建窗之前先按系统 DPI 估算尺寸（建完再用 GetDpiForWindow 精修）。
            #[cfg(windows)]
            {
                use windows::Win32::UI::HiDpi::{GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
                unsafe { let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2); }
                let dpi = unsafe { GetDpiForSystem() };
                if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 }
            }
        };
        let (x, y, w, h) = default_rect(&cfg, scale);

        let instance: HINSTANCE = unsafe {
            GetModuleHandleW(None)
                .map(|m| HINSTANCE(m.0))
                .map_err(|e| format!("GetModuleHandleW 失败：{e}"))?
        };
        let class = w!("LunacPetWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        };
        unsafe {
            RegisterClassW(&wc);
        }

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                class,
                w!("Lunac 桌宠"),
                WS_POPUP, // 先不显示，等页面加载完再 Show（避免白色闪一下）
                x,
                y,
                w,
                h,
                None,
                None,
                Some(instance),
                None,
            )
        }
        .map_err(|e| format!("CreateWindowExW 失败：{e}"))?;

        PET_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        PET_OPEN.store(true, Ordering::SeqCst);
        UI.with(|ui| ui.borrow_mut().config = cfg.clone());

        // 透明：DWM 空 region（tao/wry 同款配方）。窗口本身不擦背景。
        unsafe {
            let region = CreateRectRgn(0, 0, -1, -1);
            let bb = DWM_BLURBEHIND {
                dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
                fEnable: true.into(),
                hRgnBlur: region,
                fTransitionOnMaximized: false.into(),
            };
            let _ = DwmEnableBlurBehindWindow(hwnd, &bb);
        }

        install_webview(hwnd).map_err(|e| format!("WebView2 初始化失败：{e}"))?;

        // 初次套用（位置 / 尺寸 / 置顶 / 穿透 / 显隐）。
        apply_config(hwnd, cfg.clone());

        if cfg.visible {
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
            let _ = unsafe { UpdateWindow(hwnd) };
        }
        push_event(json!({ "type": "ready" }));

        let mut msg = MSG::default();
        loop {
            let ret = unsafe { GetMessageW(&mut msg, None, 0, 0).0 };
            match ret {
                -1 | 0 => break,
                _ => {
                    let _ = unsafe { TranslateMessage(&msg) };
                    unsafe { DispatchMessageW(&msg) };
                }
            }
        }
        CONTROLLER.with(|c| *c.borrow_mut() = None);
        UI.with(|ui| *ui.borrow_mut() = UiState::default());
        PET_OPEN.store(false, Ordering::SeqCst);
        PET_HWND.store(0, Ordering::SeqCst);
        Ok(())
    }
}

// ══════════════════════════════════════════════════════════════════
//  单测（纯逻辑，不建窗口）
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_match_v1_window() {
        let cfg = parse_config("{}");
        assert_eq!(cfg.scale, 1.0);
        assert_eq!(cfg.win_w, 320.0);
        assert_eq!(cfg.win_h, 510.0);
        assert!(!cfg.click_through);
        assert!(cfg.topmost);
        assert!(cfg.visible);
    }

    #[test]
    fn config_normalize_clamps_and_keeps_v1_fields() {
        let cfg = parse_config(
            r#"{"scale":99,"rotation":-999,"winW":1,"winH":99999,
                "expressions":{"C:/m.model3.json":"smile"},"tts":true,
                "clickThrough":true,"image":" C:/a.png ","model":"  "}"#,
        );
        assert_eq!(cfg.scale, 1.0); // 越界回落 1（与 v1 loadConfig 同口径）
        assert_eq!(cfg.rotation, -180.0);
        assert_eq!(cfg.win_w, 160.0);
        assert_eq!(cfg.win_h, 1200.0);
        assert_eq!(cfg.image, "C:/a.png");
        assert!(cfg.model.is_empty());
        assert!(cfg.click_through);
        // v1 的其它字段原样保留（不能因为 v2 不认识就抹掉）。
        assert!(cfg.extra.contains_key("expressions"));
        assert_eq!(cfg.extra.get("tts"), Some(&Value::Bool(true)));
    }

    #[test]
    fn config_roundtrip_keeps_field_names() {
        let cfg = parse_config(r#"{"image":"C:/pet.gif","clickThrough":false}"#);
        let text = serde_json::to_string(&cfg).unwrap();
        assert!(text.contains("\"clickThrough\""));
        assert!(text.contains("\"winW\""));
        let again = parse_config(&text);
        assert_eq!(again.image, "C:/pet.gif");
    }

    #[test]
    fn proactive_defaults_match_v1() {
        let value = normalize_proactive(None, None);
        assert_eq!(value["enabled"], Value::Bool(false));
        assert_eq!(value["intervalMin"], 30);
        assert_eq!(value["dailyCap"], 20);
        assert_eq!(value["screenPollSec"], 20);
        assert_eq!(value["screenMinGapSec"], 600);
        assert_eq!(value["screenDiff"], 12);
        assert_eq!(value["date"], "");
        assert_eq!(value["used"], 0);
    }

    #[test]
    fn proactive_clamps_numbers_and_merges_patch() {
        let current = json!({"enabled": true, "intervalMin": 45, "dailyCap": 20,
            "date": "2026-11-15", "used": 3});
        let value = normalize_proactive(
            Some(&current),
            Some(&json!({"intervalMin": 0, "screenDiff": 999, "dailyCap": -5})),
        );
        // 数字越界 ⇒ 夹到区间；未出现在 patch 里的字段沿用盘上值。
        assert_eq!(value["enabled"], Value::Bool(true));
        assert_eq!(value["intervalMin"], 1);
        assert_eq!(value["dailyCap"], 0);
        assert_eq!(value["screenDiff"], 64);
        assert_eq!(value["date"], "2026-11-15");
        assert_eq!(value["used"], 3);
    }

    #[test]
    fn proactive_migrates_legacy_keys_and_resets_used() {
        let current = json!({"date": "2026-11-15", "used": 7});
        let value = normalize_proactive(
            Some(&current),
            Some(&json!({"autoShot": true, "screenWatchSec": 60, "used": 0})),
        );
        assert_eq!(value["enabled"], Value::Bool(true));
        assert_eq!(value["screenPollSec"], 60);
        // 旧键不会留在输出里（迁移即收敛）。
        assert!(value.get("autoShot").is_none());
        assert!(value.get("screenWatchSec").is_none());
        assert_eq!(value["used"], 0);
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-pet-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn motion_file_name_rejects_paths_and_wrong_suffix() {
        assert!(motion_file_name("a.motion3.json").is_ok());
        assert!(motion_file_name("../a.motion3.json").is_err());
        assert!(motion_file_name("sub/a.motion3.json").is_err());
        assert!(motion_file_name("a.json").is_err());
    }

    #[test]
    fn motion_listing_is_recursive_and_sorted_by_path() {
        let dir = temp_dir("motion-list");
        let model = dir.join("m.model3.json");
        std::fs::write(&model, "{}").unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("b.motion3.json"), "{}").unwrap();
        std::fs::write(dir.join("sub").join("a.motion3.json"), "{}").unwrap();
        std::fs::write(dir.join("ignore.json"), "{}").unwrap();
        let files = list_motion_files(&dir.to_string_lossy()).unwrap();
        let names: Vec<String> = files.iter().map(|file| file.name.clone()).collect();
        assert_eq!(names, vec!["b".to_string(), "a".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn motion_write_rename_delete_round_trip() {
        let dir = temp_dir("motion-ops");
        let model = dir.join("m.model3.json");
        std::fs::write(&model, "{}").unwrap();
        let model_text = model.to_string_lossy().to_string();

        let written = motion_write_json(&model_text, "gen.motion3.json", "{}");
        let value: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(value["ok"], Value::Bool(true));
        // 同名再建 ⇒ 拒绝（不覆盖用户 / 作者的动作）。
        let again: Value =
            serde_json::from_str(&motion_write_json(&model_text, "gen.motion3.json", "{}"))
                .unwrap();
        assert_eq!(again["ok"], Value::Bool(false));

        let renamed: Value = serde_json::from_str(&motion_rename_json(
            &model_text,
            "gen.motion3.json",
            "renamed.motion3.json",
        ))
        .unwrap();
        assert_eq!(renamed["ok"], Value::Bool(true));
        assert!(dir
            .join("lunac-live2d")
            .join("motions")
            .join("renamed.motion3.json")
            .is_file());

        let deleted: Value =
            serde_json::from_str(&motion_delete_json(&model_text, "renamed.motion3.json"))
                .unwrap();
        assert_eq!(deleted["ok"], Value::Bool(true));
        let files = list_motion_files(&dir.to_string_lossy()).unwrap();
        assert!(files.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn side_files_migrate_from_legacy_root() {
        let dir = temp_dir("side-migrate");
        let model = dir.join("m.model3.json");
        std::fs::write(&model, "{}").unwrap();
        let model_text = model.to_string_lossy().to_string();
        let legacy = dir.join("lunac.hotkeys.json");
        std::fs::write(&legacy, "{\"version\":1}").unwrap();

        let text = read_side_file(&model_text, "lunac.hotkeys.json", "热键文件")
            .unwrap()
            .unwrap();
        assert_eq!(text, "{\"version\":1}");
        // 旧位置搬走，新位置落地；再次读取仍命中同一份。
        assert!(!legacy.is_file());
        assert!(dir
            .join("lunac-live2d")
            .join("lunac.hotkeys.json")
            .is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_info_accepts_cubism4() {
        let info = model_info_from_text(
            r#"{"Version":4,"FileReferences":{"Moc":"a.moc3","Textures":["t0.png"],
                "Expressions":[{"File":"e.exp3.json","Name":"smile"}],
                "Motions":{"Idle":[{"File":"i.motion3.json"}]}}}"#,
        );
        assert!(info.ok);
        assert_eq!(info.version, 4.0);
        assert_eq!(info.textures, 1);
        assert_eq!(info.motions, 1);
        assert_eq!(info.expressions, 1);
        assert_eq!(info.expression_names, vec!["smile".to_string()]);
        assert_eq!(info.motion_groups, vec!["Idle".to_string()]);
        assert_eq!(info.reason, "Idle");
    }

    #[test]
    fn model_info_rejects_old_or_broken_definitions() {
        let old = model_info_from_text(r#"{"Version":2,"FileReferences":{"Moc":"a.moc3"}}"#);
        assert!(!old.ok);
        assert!(old.reason.contains("Cubism 4"));
        let bad = model_info_from_text("not json");
        assert!(!bad.ok);
        assert_eq!(bad.reason, "模型定义不是合法 JSON");
    }

    #[test]
    fn hex_roundtrip_handles_chinese_paths() {
        let path = "C:\\用户\\模型\\x.model3.json";
        let hex = hex_encode(path.as_bytes());
        assert_eq!(decode_fs_path(&hex).unwrap(), std::path::PathBuf::from(path));
        assert!(hex_decode("abc").is_none());
        assert!(hex_decode("zz").is_none());
    }

    #[test]
    fn allowed_paths_are_limited_to_the_current_shape() {
        let cfg = parse_config(r#"{"model":"C:/models/cat/x.model3.json"}"#);
        assert!(path_allowed(&cfg, std::path::Path::new("C:/models/cat/x.model3.json")));
        assert!(path_allowed(&cfg, std::path::Path::new("C:/models/cat/textures/t0.png")));
        assert!(!path_allowed(&cfg, std::path::Path::new("C:/models/other/t0.png")));
        assert!(!path_allowed(&cfg, std::path::Path::new("C:/Windows/win.ini")));

        let image = parse_config(r#"{"image":"C:/pics/pet.gif"}"#);
        assert!(path_allowed(&image, std::path::Path::new("C:/pics/pet.gif")));
        assert!(!path_allowed(&image, std::path::Path::new("C:/pics/other.gif")));
    }

    #[test]
    fn config_path_uses_v1_key_name() {
        let p = config_path(std::path::Path::new("C:/lunac"));
        assert!(p.ends_with("ModuleData\\pet\\lunac.pet.v1.json"));
    }

    #[test]
    fn mime_matches_extensions() {
        assert_eq!(mime_of(std::path::Path::new("a.png")), "image/png");
        assert_eq!(mime_of(std::path::Path::new("a.MODEL3.JSON")), "application/json; charset=utf-8");
        assert_eq!(mime_of(std::path::Path::new("a.moc3")), "application/octet-stream");
    }
}
