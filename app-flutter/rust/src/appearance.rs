// 外观 / 主题后端 —— 移植自 v1 `app/src-tauri/src/appearance.rs`（口径逐项一致）。
//
// 三件事，彼此独立：
//   ① `system_theme()` —— 读 Windows 的强调色与深 / 浅色偏好；
//   ② `themes_json()` —— 扫 `<exe 根>\themes\<目录>\theme.json`，解析成前端可消费的结构
//                        （tokens + 已解析为绝对路径的资产；含路径穿越防护）；
//   ③ `themes_dir()`  —— 返回（并按需创建）主题根目录，供「打开主题目录」入口使用。
//
// v2 现状（2026-11-14 起）：设置「风格」可以**点主题包立即切换** —— 本模块负责
// 扫描 / 解析主题（含资产路径校验）、`config/appearance.json` 的全部外观字段（主题 id /
// 自定义配色 / 背景图与四个滑块 / `tintBase`），以及背景图的系统文件对话框；
// Dart 侧把 tokens 套到运行时色板（`LunacColors.apply`）并在 `MaterialApp.builder`
// 画背景层，三处 Flutter 入口启动时各应用一次。取色器仍是十六进制输入（v1 的色板 /
// 色相条 / 屏幕取色未移植，登记在 docs/todo.md M5v-补5）。
//
// ── AccentColor / ColorizationColor 的字节序（v1 于 2026-09-19 本机实测，不得凭记忆改）──
//
//   注册表原值：AccentColor = 0xFF201E19、ColorizationColor = 0xC4191E20；
//   官方接口：UISettings.GetColorValue(Accent) = RGB(55,67,71) = #374347。
//   结论：**AccentColor 是 0xAABBGGRR（BGR），ColorizationColor 是 0xAARRGGBB（RGB）** ——
//   两者 24 位色值互为字节倒序，按各自口径解析得到同一个颜色（自洽）；同族 StartColorMenu
//   按 AABBGGRR 解析与 AccentPalette 第 5 项字节逐字节相等（第二条独立证据）。
//   单测 `accent_dword_byte_order_matches_measured_evidence` 把本机实测值钉死。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// 内置默认主题的**编译期内嵌副本**（唯一真相源仍是 v1 的
/// `app/src-tauri/themes/default/theme.json`；v2 与 v1 同仓库，直接引用同一份文件）。
const DEFAULT_THEME_JSON: &str =
    include_str!("../../../app/src-tauri/themes/default/theme.json");

// ── 系统主题 ─────────────────────────────────────────────────────

/// 系统主题现状。`source` 只用于判断这份数据是不是真读到了：
/// `"registry"` = 成功读注册表；`"fallback"` = 读不到，用默认值（accent 空串、深色）。
#[derive(Debug, Serialize, Clone, Default)]
pub struct SystemTheme {
    /// "#RRGGBB"（大写十六进制）；读不到时为空串。
    pub accent: String,
    /// true = 系统为深色。
    pub dark: bool,
    /// "registry" | "fallback"
    pub source: String,
}

// ── 主题包数据结构 ───────────────────────────────────────────────

/// 主题色板。字段**全部 Option**：缺省即「前端用自己的默认值」，
/// 这样只写了一半字段的主题也能用，且没写到的部分与现状逐像素一致。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ThemeTokens {
    #[serde(default)]
    pub accent: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub text_dim: Option<String>,
    #[serde(default)]
    pub text_muted: Option<String>,
    #[serde(default)]
    pub border_glass: Option<String>,
    /// 允许 "#RRGGBB" 与 "r,g,b" 两种写法（由前端解析）。
    #[serde(default)]
    pub surface: Option<String>,
    /// CSS 长度原样字符串（如 "14px" / "4px 14px 4px 14px" / "0"）。
    #[serde(default)]
    pub radius_search: Option<String>,
    #[serde(default)]
    pub radius_results: Option<String>,
    #[serde(default)]
    pub pattern_opacity: Option<f64>,
}

/// 主题资产：值是 theme.json 里写的**相对路径**（相对主题目录），解析前不做任何拼接。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ThemeAssets {
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub search_pattern: Option<String>,
    /// key = 插件 id（"ai-agent" / "web-search" / …），value = 图标相对路径。
    #[serde(default)]
    pub icons: HashMap<String, String>,
}

/// 对应 `themes\<目录名>\theme.json`。字段级 `#[serde(default)]`：
/// 旧版本或用户手工编辑的 theme.json 缺字段也能读。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ThemeManifest {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub tokens: ThemeTokens,
    #[serde(default)]
    pub assets: ThemeAssets,
}

/// 与 `ThemeAssets` 同形，但值换成**绝对路径**。
/// 单个资产不存在 / 越界时只把那一项置 None —— 不让一个坏路径让整个主题读失败。
#[derive(Debug, Serialize, Clone, Default)]
pub struct ResolvedAssets {
    pub background: Option<String>,
    pub search_pattern: Option<String>,
    pub icons: HashMap<String, String>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct ThemeInfo {
    pub manifest: ThemeManifest,
    /// 主题目录绝对路径。
    pub dir: String,
    /// 目录内存在名为 `builtin` 的标记文件即视为内置主题。
    pub builtin: bool,
    pub resolved: ResolvedAssets,
}

// ── 对外入口 ─────────────────────────────────────────────────────

/// 系统强调色与深 / 浅色偏好（JSON 对象字符串）。
pub fn system_theme_json() -> String {
    serde_json::to_string(&system_theme()).unwrap_or_else(|_| "{}".to_string())
}

/// 列出 `<root>/themes` 下的全部主题（JSON 数组字符串；目录不存在 = 空表，**不创建**）。
pub fn themes_json(root: &Path) -> String {
    let list = scan_themes_in(&root.join("themes"));
    serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
}

/// 主题目录绝对路径；**不存在则创建**（用户要从这里放自己的主题）。
pub fn themes_dir(root: &Path) -> String {
    let dir = root.join("themes");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        // 建不出来不阻断：仍把路径回给前端，用户至少能看到「该往哪放」。
        eprintln!(
            "[lunac-appearance] 创建主题目录失败 {}: {err}",
            dir.display()
        );
    }
    dir.to_string_lossy().to_string()
}

// ── 系统主题读取 ─────────────────────────────────────────────────

/// AccentColor 是 `0xAABBGGRR` → "#RRGGBB"。
fn accent_color_to_hex(value: u32) -> String {
    let r = (value & 0xFF) as u8;
    let g = ((value >> 8) & 0xFF) as u8;
    let b = ((value >> 16) & 0xFF) as u8;
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// ColorizationColor 是 `0xAARRGGBB` → "#RRGGBB"。
fn colorization_color_to_hex(value: u32) -> String {
    let r = ((value >> 16) & 0xFF) as u8;
    let g = ((value >> 8) & 0xFF) as u8;
    let b = (value & 0xFF) as u8;
    format!("#{r:02X}{g:02X}{b:02X}")
}

fn system_theme() -> SystemTheme {
    #[cfg(target_os = "windows")]
    {
        windows_system_theme()
    }
    #[cfg(not(target_os = "windows"))]
    {
        SystemTheme {
            accent: String::new(),
            dark: true,
            source: "fallback".into(),
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_system_theme() -> SystemTheme {
    const DWM_KEY: &str = r"Software\Microsoft\Windows\DWM";
    const PERSONALIZE_KEY: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

    // 读不到 `AppsUseLightTheme` 时按深色 —— 未知环境下选「不会闪白」的默认值更安全。
    let dark = reg_read_dword(PERSONALIZE_KEY, "AppsUseLightTheme")
        .map(|value| value == 0)
        .unwrap_or(true);

    if let Some(value) = reg_read_dword(DWM_KEY, "AccentColor") {
        return SystemTheme {
            accent: accent_color_to_hex(value),
            dark,
            source: "registry".into(),
        };
    }
    if let Some(value) = reg_read_dword(DWM_KEY, "ColorizationColor") {
        return SystemTheme {
            accent: colorization_color_to_hex(value),
            dark,
            source: "registry".into(),
        };
    }
    SystemTheme {
        accent: String::new(),
        dark,
        source: "fallback".into(),
    }
}

/// 读 `HKCU\<sub_key>` 下的一个 REG_DWORD；不存在 / 类型不符 / 其它错误一律 None。
///
/// 用手写 Win32 FFI 而非 `reg.exe`：spawn 进程既慢又会闪黑框，且解析输出依赖系统语言。
#[cfg(target_os = "windows")]
fn reg_read_dword(sub_key: &str, value: &str) -> Option<u32> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueW(
            hkey: isize,
            sub_key: *const u16,
            value: *const u16,
            flags: u32,
            data_type: *mut u32,
            data: *mut std::ffi::c_void,
            data_len: *mut u32,
        ) -> i32;
    }

    /// HKEY_CURRENT_USER = 0x80000001（预定义句柄，不是真实指针）。
    const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
    /// 只接受 DWORD：类型不符时让 RegGetValueW 直接返回错误。
    const RRF_RT_REG_DWORD: u32 = 0x0000_0010;
    const ERROR_SUCCESS: i32 = 0;

    let wide = |text: &str| -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let sub_key_w = wide(sub_key);
    let value_w = wide(value);

    let mut data: u32 = 0;
    let mut data_len: u32 = std::mem::size_of::<u32>() as u32;
    let mut data_type: u32 = 0;

    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            sub_key_w.as_ptr(),
            value_w.as_ptr(),
            RRF_RT_REG_DWORD,
            &mut data_type,
            &mut data as *mut u32 as *mut std::ffi::c_void,
            &mut data_len,
        )
    };

    if status == ERROR_SUCCESS && data_len == std::mem::size_of::<u32>() as u32 {
        Some(data)
    } else {
        None
    }
}

// ── 主题目录扫描 ─────────────────────────────────────────────────

/// 扫描指定主题根目录（命令壳与单测共用的实现）。
///
/// 单个主题失败**只跳过它自己**：解析失败 / 读不到 JSON 都记一行后继续 ——
/// 一个手工编辑坏了的主题不能让用户装有主题的列表整体消失。
fn scan_themes_in(root: &Path) -> Vec<ThemeInfo> {
    let mut out = Vec::new();

    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        // 目录不存在是正常的（首次启动 / dev 模式）→ 空表。
        Err(_) => return out,
    };

    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    // 按目录名排序，保证前端拿到的顺序稳定。
    dirs.sort_by_key(|path| path.file_name().map(|name| name.to_os_string()));

    for dir in dirs {
        let json_path = dir.join("theme.json");
        if !json_path.exists() {
            continue;
        }
        // **内置默认主题以「编译进二进制的这一份」为准**（v1 2026-10-24 结论）：
        // 安装目录里的 `themes\default\theme.json` 升级时不会被覆盖，老安装会停在旧版本，
        // 于是 dev 与 release 会呈现两套默认配色。`builtin` 标记文件 =「这份由我们维护」。
        let is_builtin_default = dir
            .file_name()
            .map(|name| name == "default")
            .unwrap_or(false)
            && dir.join("builtin").exists();
        let text = if is_builtin_default {
            DEFAULT_THEME_JSON.to_string()
        } else {
            match std::fs::read_to_string(&json_path) {
                Ok(text) => text,
                Err(err) => {
                    eprintln!(
                        "[lunac-appearance] 读取主题文件失败，已跳过 {}: {err}",
                        json_path.display()
                    );
                    continue;
                }
            }
        };
        let manifest: ThemeManifest = match serde_json::from_str(&text) {
            Ok(manifest) => manifest,
            Err(err) => {
                eprintln!(
                    "[lunac-appearance] 解析主题失败，已跳过 {}: {err}",
                    json_path.display()
                );
                continue;
            }
        };

        out.push(ThemeInfo {
            builtin: dir.join("builtin").exists(),
            resolved: resolve_assets(&dir, &manifest.assets),
            dir: dir.to_string_lossy().to_string(),
            manifest,
        });
    }

    out
}

// ── 外观配置（`config/appearance.json`）────────────────────────────

/// 外观配置（v1 localStorage `lunac-appearance` 的字段名与默认值，逐项一致）。
///
/// 背景图相关的四个字段（`bgImage` / `bgBlur` / `bgSaturate` / `bgOpacity` / `sheen`）
/// 先保留在结构里（读写都不丢），但 v2 设置面板还没有背景编辑器 —— 见 docs/todo.md。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppearanceConfig {
    /// `"default"` = 不套主题包。
    pub theme_id: String,
    /// 自定义底色（空串 = 跟随主题）。
    pub base_color: String,
    /// 底色透明度 0.3–1（v2 没有桌面透视，应用时与默认深底混合）。
    pub surface_alpha: f64,
    /// 按钮线条色（空串 = 跟随主题）与其透明度。
    pub btn_line_color: String,
    pub btn_line_alpha: f64,
    /// 按钮背景色（空串 = 跟随主题）与其透明度。
    pub btn_bg_color: String,
    pub btn_bg_alpha: f64,
    /// 文字明暗度 −100…100（HSL 明度偏移，0 = 不改）。
    pub text_light: f64,
    /// 「其他颜色」统一浮层（空串 = 跟随主题）与其透明度。
    pub overlay_color: String,
    pub overlay_alpha: f64,
    /// 背景图绝对路径（v1 用 `convertFileSrc` 的 asset URL，v2 直接存盘上路径）。
    pub bg_image: Option<String>,
    /// 背景图：毛玻璃化 px / 饱和度 % / 不透明度 0–1 / 反光 0–1。
    pub bg_blur: f64,
    pub bg_saturate: f64,
    pub bg_opacity: f64,
    pub sheen: f64,
    /// 「恢复默认主题」开关（v1 `tintBase`）：配色回默认、四种透明度写回默认值。
    pub tint_base: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme_id: "default".to_string(),
            base_color: String::new(),
            surface_alpha: 1.0,
            btn_line_color: String::new(),
            btn_line_alpha: 1.0,
            btn_bg_color: String::new(),
            btn_bg_alpha: 0.09,
            text_light: 0.0,
            overlay_color: String::new(),
            overlay_alpha: 0.1,
            bg_image: None,
            bg_blur: 4.0,
            bg_saturate: 92.0,
            bg_opacity: 0.5,
            sheen: 0.0,
            tint_base: false,
        }
    }
}

/// 数值字段的合法区间（v1 `APPEARANCE_RANGE`；越界一律夹取，不让坏配置把界面画死）。
const APPEARANCE_RANGES: [(&str, f64, f64); 6] = [
    ("surfaceAlpha", 0.3, 1.0),
    ("btnLineAlpha", 0.0, 1.0),
    ("btnBgAlpha", 0.0, 1.0),
    ("textLight", -100.0, 100.0),
    ("overlayAlpha", 0.0, 1.0),
    ("sheen", 0.0, 1.0),
];

/// `#RRGGBB` 或空串（空串 = 跟随主题）；其它写法一律非法。
fn is_hex_or_empty(text: &str) -> bool {
    let bytes = text.as_bytes();
    text.is_empty()
        || (bytes.len() == 7
            && bytes[0] == b'#'
            && bytes[1..].iter().all(|byte| byte.is_ascii_hexdigit()))
}

/// 外观配置落点：`<exe 根>/config/appearance.json`。
///
/// v1 把外观偏好放在 localStorage `lunac-appearance`（本机偏好）；v2 没有 WebView，
/// 与其它偏好一样落到配置文件。
fn config_path(root: &Path) -> PathBuf {
    root.join("config").join("appearance.json")
}

/// 读外观配置：缺文件 / 坏 JSON → 默认值；字段越界夹取、颜色非法回落空串。
pub fn load_config(root: &Path) -> AppearanceConfig {
    let mut config = std::fs::read_to_string(config_path(root))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| serde_json::from_value::<AppearanceConfig>(value).ok())
        .unwrap_or_default();
    normalize(&mut config);
    config
}

fn normalize(config: &mut AppearanceConfig) {
    let defaults = AppearanceConfig::default();
    for (field, low, high) in APPEARANCE_RANGES {
        let current = match field {
            "surfaceAlpha" => config.surface_alpha,
            "btnLineAlpha" => config.btn_line_alpha,
            "btnBgAlpha" => config.btn_bg_alpha,
            "textLight" => config.text_light,
            "overlayAlpha" => config.overlay_alpha,
            _ => config.sheen,
        };
        let value = if current.is_finite() {
            current.clamp(low, high)
        } else {
            match field {
                "surfaceAlpha" => defaults.surface_alpha,
                "btnLineAlpha" => defaults.btn_line_alpha,
                "btnBgAlpha" => defaults.btn_bg_alpha,
                "textLight" => defaults.text_light,
                "overlayAlpha" => defaults.overlay_alpha,
                _ => defaults.sheen,
            }
        };
        match field {
            "surfaceAlpha" => config.surface_alpha = value,
            "btnLineAlpha" => config.btn_line_alpha = value,
            "btnBgAlpha" => config.btn_bg_alpha = value,
            "textLight" => config.text_light = value,
            "overlayAlpha" => config.overlay_alpha = value,
            _ => config.sheen = value,
        }
    }
    // 背景图四个滑块（v1 `clampAppearance` 同区间：毛玻璃 0–40 / 饱和度 0–200 /
    // 不透明度 0–1 / 反光 0–1）。坏值回落默认值，越界夹取。
    let bg_ranges: [(&mut f64, f64, f64, f64); 4] = [
        (&mut config.bg_blur, 0.0, 40.0, defaults.bg_blur),
        (&mut config.bg_saturate, 0.0, 200.0, defaults.bg_saturate),
        (&mut config.bg_opacity, 0.0, 1.0, defaults.bg_opacity),
        (&mut config.sheen, 0.0, 1.0, defaults.sheen),
    ];
    for (slot, low, high, fallback) in bg_ranges {
        *slot = if slot.is_finite() {
            slot.clamp(low, high)
        } else {
            fallback
        };
    }
    if !is_hex_or_empty(&config.base_color) {
        config.base_color = defaults.base_color;
    }
    if !is_hex_or_empty(&config.btn_line_color) {
        config.btn_line_color = defaults.btn_line_color;
    }
    if !is_hex_or_empty(&config.btn_bg_color) {
        config.btn_bg_color = defaults.btn_bg_color;
    }
    if !is_hex_or_empty(&config.overlay_color) {
        config.overlay_color = defaults.overlay_color;
    }
    if config.theme_id.trim().is_empty() {
        config.theme_id = defaults.theme_id;
    }
}

/// 面板要的一份配置快照（JSON 字符串，字段名与 Dart `AppearanceConfig` 一一对应）。
pub fn config_json(root: &Path) -> String {
    serde_json::to_string(&load_config(root)).unwrap_or_else(|_| "{}".to_string())
}

/// 合并写入外观配置（只覆盖传入的键）。未知主题 id 收回到 `default`。
pub fn save_config_patch(
    root: &Path,
    patch: &serde_json::Value,
) -> Result<AppearanceConfig, String> {
    let Some(object) = patch.as_object() else {
        return Err("配置格式非法（必须是对象）".to_string());
    };
    let path = config_path(root);
    let mut value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    let target = value.as_object_mut().expect("已过滤为对象");
    for (key, item) in object {
        match key.as_str() {
            "themeId" => {
                let id = item.as_str().ok_or("themeId 必须是字符串")?;
                let known = id == "default"
                    || scan_themes_in(&root.join("themes"))
                        .iter()
                        .any(|theme| theme.manifest.id == id);
                target.insert(
                    "themeId".into(),
                    serde_json::json!(if known { id } else { "default" }),
                );
            }
            "baseColor" | "btnLineColor" | "btnBgColor" | "overlayColor" => {
                let text = item.as_str().ok_or("颜色必须是字符串")?;
                if !is_hex_or_empty(text) {
                    return Err(format!("{key} 必须是 #RRGGBB 或空串（跟随主题）"));
                }
                target.insert(key.clone(), serde_json::json!(text));
            }
            "surfaceAlpha" | "btnLineAlpha" | "btnBgAlpha" | "textLight"
            | "overlayAlpha" | "sheen" => {
                let number = item.as_f64().ok_or("滑块值必须是数字")?;
                if !number.is_finite() {
                    return Err("滑块值必须是有限数字".to_string());
                }
                let (_, low, high) = APPEARANCE_RANGES
                    .iter()
                    .find(|(field, _, _)| *field == key.as_str())
                    .copied()
                    .ok_or("未知字段")?;
                target.insert(key.clone(), serde_json::json!(number.clamp(low, high)));
            }
            "bgImage" => {
                if item.is_null() {
                    target.insert("bgImage".into(), serde_json::Value::Null);
                } else if let Some(text) = item.as_str() {
                    target.insert("bgImage".into(), serde_json::json!(text));
                } else {
                    return Err("bgImage 必须是字符串或 null".to_string());
                }
            }
            "bgBlur" | "bgSaturate" | "bgOpacity" => {
                let number = item.as_f64().ok_or("数值字段必须是数字")?;
                if !number.is_finite() {
                    return Err("滑块值必须是有限数字".to_string());
                }
                let (low, high) = match key.as_str() {
                    "bgBlur" => (0.0, 40.0),
                    "bgSaturate" => (0.0, 200.0),
                    _ => (0.0, 1.0),
                };
                target.insert(key.clone(), serde_json::json!(number.clamp(low, high)));
            }
            "tintBase" => {
                let flag = item.as_bool().ok_or("tintBase 必须是布尔值")?;
                target.insert("tintBase".into(), serde_json::json!(flag));
            }
            _ => return Err(format!("未知字段：{key}")),
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("配置目录不可写 —— {e}"))?;
    }
    let text = serde_json::to_string_pretty(&value).map_err(|e| format!("序列化失败 —— {e}"))?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).map_err(|e| format!("写配置失败 —— {e}"))?;
    std::fs::rename(&temp, &path).map_err(|e| format!("写配置失败 —— {e}"))?;
    Ok(load_config(root))
}

/// 当前主题 id（缺文件 / 坏 JSON → `default`）。
pub fn theme_id(root: &Path) -> String {
    load_config(root).theme_id
}

/// 写主题 id（`default` 或已安装主题；未知 id 收回到 `default`）。返回实际写入的 id。
pub fn set_theme_id(root: &Path, id: &str) -> Result<String, String> {
    let config = save_config_patch(root, &serde_json::json!({ "themeId": id }))?;
    Ok(config.theme_id)
}

/// 弹出系统的「选择背景图片」对话框（v1 走 `@tauri-apps/plugin-dialog`，这里手写 Win32，
/// 与 `pet::pick_file` 同一条路）。返回选中的绝对路径；取消 / 失败返回空串。
pub fn pick_bg_image() -> String {
    #[cfg(windows)]
    unsafe {
        use windows::core::{PCWSTR, PWSTR};
        use windows::Win32::UI::Controls::Dialogs::{
            GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST,
            OPENFILENAMEW,
        };
        let filter: Vec<u16> = "图片 (*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.gif;*.avif)\0*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.gif;*.avif\0所有文件 (*.*)\0*.*\0\0"
            .encode_utf16()
            .collect();
        let title: Vec<u16> = "选择背景图片\0".encode_utf16().collect();
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

// ── 资产路径解析（含路径穿越防护）─────────────────────────────────

fn resolve_assets(theme_dir: &Path, assets: &ThemeAssets) -> ResolvedAssets {
    ResolvedAssets {
        background: resolve_asset(theme_dir, assets.background.as_deref(), "background"),
        search_pattern: resolve_asset(
            theme_dir,
            assets.search_pattern.as_deref(),
            "search_pattern",
        ),
        icons: assets
            .icons
            .iter()
            .filter_map(|(plugin, relative)| {
                resolve_asset(theme_dir, Some(relative), "icons")
                    .map(|absolute| (plugin.clone(), absolute))
            })
            .collect(),
    }
}

/// 把 theme.json 里的相对路径解析为绝对路径。
///
/// **theme.json 是用户可编辑的文件**，所以「相对路径」必须当成不可信输入：
///   ① 拒绝绝对路径与盘符前缀；
///   ② 拒绝任何非 `Normal` 路径成分（尤其 `..`）；
///   ③ 拼好后再规范化核对，确认仍落在主题目录内（挡符号链接 / 目录联结）。
/// 违反任一条 → None + 一行日志；文件不存在 → None（不告警，只写部分资产是正常的）。
fn resolve_asset(theme_dir: &Path, value: Option<&str>, label: &str) -> Option<String> {
    let relative = value?.trim();
    if relative.is_empty() {
        return None;
    }

    let relative_path = Path::new(relative);
    if relative_path.is_absolute() {
        eprintln!("[lunac-appearance] {label} 资产是绝对路径，已拒绝：{relative}");
        return None;
    }
    if relative_path
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        eprintln!("[lunac-appearance] {label} 资产含 `..` / 根 / 盘符等非法成分，已拒绝：{relative}");
        return None;
    }

    let full = theme_dir.join(relative_path);
    if !full.exists() {
        return None;
    }
    if !is_within(theme_dir, &full) {
        eprintln!("[lunac-appearance] {label} 资产解析后越出主题目录，已拒绝：{relative}");
        return None;
    }
    Some(full.to_string_lossy().to_string())
}

/// `target` 规范化后是否仍在 `base` 之内；规范化失败时退回词法比较（宁可误拒）。
fn is_within(base: &Path, target: &Path) -> bool {
    let norm = |path: &Path| {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    };
    norm(target).starts_with(norm(base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 每个用例独占一个临时目录（进程 id + 标签），不依赖任何真实用户目录。
    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-v2-appearance-test-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 字节序结论必须能被 v1 的本机实测值复现：两个 DWORD 是对方的字节倒序，
    /// 按各自口径解析必须得到同一个颜色。
    #[test]
    fn accent_dword_byte_order_matches_measured_evidence() {
        assert_eq!(accent_color_to_hex(0xFF20_1E19), "#191E20");
        assert_eq!(colorization_color_to_hex(0xC419_1E20), "#191E20");
    }

    /// ① 完整 theme.json：tokens 全字段 + 三类资产都解析成存在文件的绝对路径。
    #[test]
    fn parses_full_theme_json_and_resolves_assets() {
        let root = temp_root("full");
        let theme_dir = root.join("mytheme");
        fs::create_dir_all(theme_dir.join("icons")).unwrap();
        fs::write(theme_dir.join("builtin"), b"").unwrap();
        fs::write(theme_dir.join("bg.png"), b"x").unwrap();
        fs::write(theme_dir.join("pattern.png"), b"x").unwrap();
        fs::write(theme_dir.join("icons").join("ai.png"), b"x").unwrap();
        fs::write(
            theme_dir.join("theme.json"),
            r##"{
                "id": "mytheme",
                "name": "My Theme",
                "version": "1.0.0",
                "author": "tester",
                "tokens": {
                    "accent": "#C0A0A0",
                    "text": "#EAE2DA",
                    "text_dim": "#B2A9A3",
                    "surface": "28,26,32",
                    "radius_search": "14px",
                    "radius_results": "0",
                    "pattern_opacity": 0.18
                },
                "assets": {
                    "background": "bg.png",
                    "search_pattern": "pattern.png",
                    "icons": { "ai-agent": "icons/ai.png" }
                }
            }"##,
        )
        .unwrap();

        let got = scan_themes_in(&root);
        assert_eq!(got.len(), 1);
        let theme = &got[0];
        assert_eq!(theme.manifest.id, "mytheme");
        assert_eq!(theme.manifest.name, "My Theme");
        assert_eq!(theme.manifest.tokens.accent.as_deref(), Some("#C0A0A0"));
        assert_eq!(theme.manifest.tokens.surface.as_deref(), Some("28,26,32"));
        assert_eq!(theme.manifest.tokens.radius_search.as_deref(), Some("14px"));
        assert_eq!(theme.manifest.tokens.pattern_opacity, Some(0.18));
        assert!(theme.builtin, "目录内有 builtin 标记文件 → builtin = true");
        assert_eq!(theme.dir, theme_dir.to_string_lossy().to_string());
        assert_eq!(
            theme.resolved.background,
            Some(theme_dir.join("bg.png").to_string_lossy().to_string())
        );
        assert_eq!(
            theme.resolved.icons.get("ai-agent"),
            Some(&theme_dir.join("icons/ai.png").to_string_lossy().to_string())
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// ② 最小 manifest `{"id":"x","name":"y"}` 也能解析；目录不存在 = 空表且**不创建**。
    #[test]
    fn parses_minimal_manifest_and_missing_root_is_empty() {
        let root = temp_root("minimal");
        let theme_dir = root.join("bare");
        fs::create_dir_all(&theme_dir).unwrap();
        fs::write(theme_dir.join("theme.json"), r#"{"id":"x","name":"y"}"#).unwrap();

        let got = scan_themes_in(&root);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].manifest.id, "x");
        assert_eq!(got[0].manifest.name, "y");
        assert!(got[0].manifest.tokens.accent.is_none());
        assert!(!got[0].builtin);

        let missing = root.join("nope");
        assert!(scan_themes_in(&missing).is_empty());
        assert!(!missing.exists());

        let _ = fs::remove_dir_all(&root);
    }

    /// ③ 路径穿越（`..`）必须被拒：目标真实存在，被拒只能来自防护。
    #[test]
    fn rejects_parent_directory_escape() {
        let root = temp_root("traversal");
        let theme_dir = root.join("evil");
        fs::create_dir_all(&theme_dir).unwrap();
        fs::write(root.join("outside.png"), b"x").unwrap();

        let assets = ThemeAssets {
            background: Some("../../windows/system32/x.png".into()),
            search_pattern: Some("../outside.png".into()),
            icons: HashMap::new(),
        };
        let resolved = resolve_assets(&theme_dir, &assets);
        assert!(resolved.background.is_none());
        assert!(resolved.search_pattern.is_none());

        let _ = fs::remove_dir_all(&root);
    }

    /// ④ 绝对路径资产必须被拒（同样让目标真实存在，排除「不存在」这个解释）。
    #[test]
    fn rejects_absolute_asset_path() {
        let root = temp_root("absolute");
        let theme_dir = root.join("theme");
        fs::create_dir_all(&theme_dir).unwrap();
        let outside = std::env::temp_dir().join(format!(
            "lunac-v2-appearance-abs-target-{}.png",
            std::process::id()
        ));
        fs::write(&outside, b"x").unwrap();

        let mut icons = HashMap::new();
        icons.insert("memo".to_string(), outside.to_string_lossy().to_string());
        let assets = ThemeAssets {
            background: Some(outside.to_string_lossy().to_string()),
            search_pattern: None,
            icons,
        };
        let resolved = resolve_assets(&theme_dir, &assets);
        assert!(resolved.background.is_none());
        assert!(resolved.icons.is_empty());

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&outside);
    }

    /// ⑤ 字段类型错误只跳过坏主题，不连累同目录的好主题。
    #[test]
    fn type_error_skips_only_the_broken_theme() {
        let root = temp_root("badtype");
        let bad = root.join("broken");
        fs::create_dir_all(&bad).unwrap();
        fs::write(
            bad.join("theme.json"),
            r#"{"id":"broken","name":"b","tokens":{"pattern_opacity":"0.18"}}"#,
        )
        .unwrap();
        let good = root.join("good");
        fs::create_dir_all(&good).unwrap();
        fs::write(good.join("theme.json"), r#"{"id":"good","name":"g"}"#).unwrap();

        let got = scan_themes_in(&root);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].manifest.id, "good");

        let _ = fs::remove_dir_all(&root);
    }

    /// ⑥ 主题列表的 JSON 字段名与 Dart 侧一一对应。
    #[test]
    fn theme_json_has_the_contract_fields() {
        let root = temp_root("json");
        let theme_dir = root.join("themes").join("t");
        fs::create_dir_all(&theme_dir).unwrap();
        fs::write(
            theme_dir.join("theme.json"),
            r##"{"id":"t","name":"T","tokens":{"accent":"#AABBCC"}}"##,
        )
        .unwrap();

        let text = themes_json(&root);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let item = &value.as_array().unwrap()[0];
        for key in ["manifest", "dir", "builtin", "resolved"] {
            assert!(item.get(key).is_some(), "缺少 {key}：{text}");
        }
        assert_eq!(item["manifest"]["id"], "t");
        assert_eq!(item["manifest"]["tokens"]["accent"], "#AABBCC");
        assert!(item["resolved"]["icons"].is_object());

        let _ = fs::remove_dir_all(&root);
    }

    /// ⑦ 系统主题 JSON 字段名固定（Dart 侧 `SystemTheme.fromJson` 依赖它）。
    #[test]
    fn system_theme_json_has_the_contract_fields() {
        let text = system_theme_json();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        for key in ["accent", "dark", "source"] {
            assert!(value.get(key).is_some(), "缺少 {key}：{text}");
        }
    }

    /// ⑧ 仓库里随包发货的主题包（v1 与 v2 共用 `app/src-tauri/themes`）必须真的解析得出、
    /// 声明的资产也真的在盘上 —— 手写 JSON 的路径写错了编译器管不着，`resolve_asset`
    /// 只会把那一项悄悄置 None，用户看到的是「背景没了」却没有任何线索。
    ///
    /// **M6 打包时要把这个目录拷到 `<exe 根>/themes`**（本测试同时是那条打包要求的依据）。
    #[test]
    fn shipped_theme_packs_parse_and_their_assets_are_present() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../app/src-tauri/themes");
        let list = scan_themes_in(&root);
        let ids: Vec<&str> = list.iter().map(|theme| theme.manifest.id.as_str()).collect();
        assert!(ids.contains(&"default"), "缺内置默认主题：{ids:?}");

        for theme in &list {
            let dir = PathBuf::from(&theme.dir);
            let id = &theme.manifest.id;
            assert!(!theme.manifest.name.trim().is_empty(), "{id} 没有名字");
            assert!(
                theme.manifest.tokens.accent.is_some(),
                "{id} 没写 tokens.accent —— 主题色只有一个真相源就是这个字段"
            );
            if let Some(relative) = &theme.manifest.assets.background {
                assert!(dir.join(relative).is_file(), "{id} 的背景不存在：{relative}");
            }
            if let Some(relative) = &theme.manifest.assets.search_pattern {
                assert!(dir.join(relative).is_file(), "{id} 的花纹不存在：{relative}");
            }
            for (plugin, relative) in &theme.manifest.assets.icons {
                assert!(
                    dir.join(relative).is_file(),
                    "{id} 的插件图标 {plugin} 不存在：{relative}"
                );
            }
        }
    }

    /// ⑨ 主题选择：缺文件默认 `default`；写已安装主题保留其它键；未知 id 收回到 `default`。
    #[test]
    fn theme_id_round_trips_and_unknown_values_are_clamped() {
        let root = temp_root("theme-id");
        assert_eq!(theme_id(&root), "default", "缺文件 = 默认主题");

        let theme_dir = root.join("themes").join("t");
        fs::create_dir_all(&theme_dir).unwrap();
        fs::write(
            theme_dir.join("theme.json"),
            r##"{"id":"t","name":"T","tokens":{"accent":"#AABBCC"}}"##,
        )
        .unwrap();

        // 保留文件里的其它键（后续背景图 / 自定义配色会写进同一个文件）
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            config_path(&root),
            r#"{"themeId":"default","bgImage":"x.png"}"#,
        )
        .unwrap();
        assert_eq!(set_theme_id(&root, "t").unwrap(), "t");
        assert_eq!(theme_id(&root), "t");
        let text = fs::read_to_string(config_path(&root)).unwrap();
        assert!(text.contains("\"bgImage\": \"x.png\""), "其它键被覆盖：{text}");

        // 未安装的 id 收回到 default（与 v1 的夹取同口径）
        assert_eq!(set_theme_id(&root, "not-installed").unwrap(), "default");
        assert_eq!(theme_id(&root), "default");

        // 坏 JSON 读成 default，不 panic
        fs::write(config_path(&root), "{broken").unwrap();
        assert_eq!(theme_id(&root), "default");
        let _ = fs::remove_dir_all(&root);
    }

    /// ⑩ 外观配置：默认值对齐 v1；补丁只覆盖传入键、越界夹取、颜色非法当场报错。
    #[test]
    fn appearance_config_defaults_clamps_and_validates_patches() {
        let root = temp_root("appearance-config");
        let config = load_config(&root);
        assert_eq!(config.theme_id, "default");
        assert_eq!(config.surface_alpha, 1.0);
        assert_eq!(config.btn_bg_alpha, 0.09);
        assert_eq!(config.text_light, 0.0);
        assert_eq!(config.bg_blur, 4.0);
        assert_eq!(config.bg_saturate, 92.0);
        assert_eq!(config.bg_opacity, 0.5);
        assert!(!config.tint_base);
        assert!(config.base_color.is_empty(), "空串 = 跟随主题");

        let saved = save_config_patch(
            &root,
            &serde_json::json!({
                "baseColor": "#112233",
                "surfaceAlpha": 5.0,
                "textLight": -400,
                "btnLineColor": "",
                "bgImage": "D:/pics/bg.png",
                "bgBlur": 99,
                "bgSaturate": -5,
                "bgOpacity": 0.25,
                "sheen": 0.5,
                "tintBase": true,
            }),
        )
        .unwrap();
        assert_eq!(saved.base_color, "#112233");
        assert_eq!(saved.surface_alpha, 1.0, "越界夹取到上限");
        assert_eq!(saved.text_light, -100.0);
        assert!(saved.btn_line_color.is_empty());
        assert_eq!(saved.btn_bg_alpha, 0.09, "未传入的字段保持默认");
        assert_eq!(saved.bg_image.as_deref(), Some("D:/pics/bg.png"));
        assert_eq!(saved.bg_blur, 40.0, "毛玻璃上限 40px");
        assert_eq!(saved.bg_saturate, 0.0, "饱和度下限 0%");
        assert_eq!(saved.bg_opacity, 0.25);
        assert_eq!(saved.sheen, 0.5);
        assert!(saved.tint_base);

        // 非法颜色 / 未知字段：整条报错，盘上文件不改
        let before = fs::read_to_string(config_path(&root)).unwrap();
        assert!(save_config_patch(&root, &serde_json::json!({"baseColor": "red"})).is_err());
        assert!(save_config_patch(&root, &serde_json::json!({"nope": 1})).is_err());
        assert_eq!(fs::read_to_string(config_path(&root)).unwrap(), before);

        let _ = fs::remove_dir_all(&root);
    }
}
