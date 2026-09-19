// src-tauri/src/appearance.rs
// 外观 / 主题后端。
//
// 三件事，彼此独立：
//   ① `get_system_theme()` —— 读 Windows 的强调色与深/浅色偏好，供前端把系统色融进自己的 UI；
//   ② `list_themes()`     —— 扫 `<exe 根>\themes\<目录名>\theme.json`，把主题包解析成
//                            前端可直接消费的结构（tokens + 已解析为绝对路径的资产）；
//   ③ `themes_dir()`      —— 返回（并按需创建）主题根目录，供「打开主题目录」入口使用。
//
// 数据根目录唯一来源是 `storage::lunac_root_dir()`（= exe 所在目录，见 ai-spec §11 规则 7），
// 本模块不硬编码任何路径。命令与返回结构一律 snake_case —— 与 `get_ai_config` / `api_key`
// 等既有命令一致，前端才能沿用同一套调用习惯。
//
// ── AccentColor / ColorizationColor 的字节序（2026-09-19 本机实测，不得凭记忆改）──────────
//
// 两个 DWORD 的通道顺序说法历来混乱，故在本机做了完整取证：
//
//   注册表原值（PowerShell 读原始 DWORD）：
//     HKCU\Software\Microsoft\Windows\DWM\AccentColor            = 0xFF201E19
//     HKCU\Software\Microsoft\Windows\DWM\ColorizationColor      = 0xC4191E20
//     HKCU\...\Themes\Personalize\AppsUseLightTheme              = 0（深色）
//     HKCU\...\Explorer\Accent\StartColorMenu                    = 0xFF39352C
//     HKCU\...\Explorer\Accent\AccentPalette(32B)                = 5D7078FF 4E5D64FF 435055FF
//                                                                  374347FF 2C3539FF 21282AFF
//                                                                  111516FF 88179800
//   官方接口交叉验证：
//     dwmapi!DwmGetColorizationColor → 0xE3171B1D（ARGB，与 ColorizationColor 近似色）
//     WinRT Windows.UI.ViewManagement.UISettings.GetColorValue(Accent) → RGB(55,67,71) = #374347
//
//   逐项解析：
//     AccentColor       0xFF201E19  AABBGGRR=#191E20   AARRGGBB=#201E19
//     ColorizationColor 0xC4191E20  AABBGGRR=#201E19   AARRGGBB=#191E20
//     StartColorMenu    0xFF39352C  AABBGGRR=#2C3539   AARRGGBB=#39352C
//
//   结论（两条互相印证，都不是猜的）：
//     1. **AccentColor 是 0xAABBGGRR（BGR 字节序），ColorizationColor 是 0xAARRGGBB（RGB）**。
//        依据一：两者的 24 位色值恰好互为字节倒序（0xFF201E19 vs 0xC4191E20），只有「一个 BGR、
//        另一个 RGB」才会出现这种关系；按上述口径解析，两者得到**同一个** #191E20 —— 自洽。
//        依据二：同族的 `StartColorMenu`（same key 家族，已知同为 aabbggrr）按 AABBGGRR 解析得
//        #2C3539，与 `AccentPalette` 的第 5 项字节 `2C 35 39 FF`（色板字节序固定为 R,G,B,A，
//        由 WinRT 强调色 #374347 == 色板第 4 项 `37 43 47 FF` 反证）**逐字节相等**。
//     2. 本机强调色（官方 API 口径）是 **#374347**（= AccentPalette 第 4 项），而 DWM 的
//        AccentColor 是系统对这个色做的**加深派生**（#191E20，约 55% 明度）—— 两者本就不相等，
//        所以「拿 DWM 值直接跟设置面板里看到的强调色比」比不出字节序，必须用上面两条同族证据。
//
//   因此本模块的解析口径固定为：AccentColor → BGR、ColorizationColor → RGB。单测
//   `accent_dword_byte_order_matches_measured_evidence` 把本机实测值钉死，改错立刻红。

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use serde::{Deserialize, Serialize};

// ── 系统主题 ─────────────────────────────────────────────────────

/// 系统主题现状。`source` 只用于前端/日志判断这份数据是不是真读到了：
/// `"registry"` = 成功读注册表；`"fallback"` = 读不到，用默认值（accent 空串、深色）。
#[derive(Debug, Serialize, Clone, Default)]
pub struct SystemTheme {
    /// "#RRGGBB"（大写十六进制）；读不到时为空串 ""
    pub accent: String,
    /// true = 系统为深色
    pub dark: bool,
    /// "registry" | "fallback"
    pub source: String,
}

// ── 主题包数据结构 ───────────────────────────────────────────────

/// 主题色板。字段**全部 Option**：缺省即「前端用自己的默认值」，
/// 这样只写了一半字段的主题也能用，且没写到的部分与现状逐像素一致。
///
/// `surface` 允许 "#RRGGBB" 与 "r,g,b" 两种写法（前端自己解析）；
/// `radius_*` 是 CSS 长度原样字符串（如 "14px" / "4px 14px 4px 14px" / "0"）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ThemeTokens {
    #[serde(default)]
    pub accent: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub text_dim: Option<String>,
    /// 弱化文字色（`--text-muted`）与玻璃边框色（`--border-glass`）。
    /// 2026-09-19 补：默认主题要能把「原配色」**整套**存进来，而这两项此前 schema
    /// 表达不了 ⇒ 只选中主题就会把它们重置为空、回落 `:root` = 「主题存不全自己」。
    /// 值是任意合法 CSS 颜色（`#837a78` / `rgba(232, 216, 202, 0.1)` 都可以）。
    #[serde(default)]
    pub text_muted: Option<String>,
    #[serde(default)]
    pub border_glass: Option<String>,
    #[serde(default)]
    pub surface: Option<String>,
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
    /// key = 插件 id（"ai-agent" / "web-search" / …），value = 图标相对路径
    #[serde(default)]
    pub icons: HashMap<String, String>,
}

/// 对应 `themes\<目录名>\theme.json`。
/// 结构与字段级 `#[serde(default)]`：旧版本或用户手工编辑的 theme.json 缺字段也能读
/// （缺的走默认值），这是「用户可以直接拿记事本改主题」的前提。
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
/// 单个资产不存在 / 越界时只把那一项置 None —— **不让一个坏路径让整个主题读失败**。
#[derive(Debug, Serialize, Clone, Default)]
pub struct ResolvedAssets {
    pub background: Option<String>,
    pub search_pattern: Option<String>,
    pub icons: HashMap<String, String>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct ThemeInfo {
    pub manifest: ThemeManifest,
    /// 主题目录绝对路径
    pub dir: String,
    /// 目录内存在名为 `builtin` 的标记文件即视为内置主题（约定见仓库 themes\default\）
    pub builtin: bool,
    pub resolved: ResolvedAssets,
}

// ── 系统主题读取 ─────────────────────────────────────────────────

/// AccentColor 是 `0xAABBGGRR`（依据见文件头实测记录）→ "#RRGGBB"。
fn accent_color_to_hex(v: u32) -> String {
    let r = (v & 0xFF) as u8;
    let g = ((v >> 8) & 0xFF) as u8;
    let b = ((v >> 16) & 0xFF) as u8;
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// ColorizationColor 是 `0xAARRGGBB`（依据见文件头实测记录）→ "#RRGGBB"。
fn colorization_color_to_hex(v: u32) -> String {
    let r = ((v >> 16) & 0xFF) as u8;
    let g = ((v >> 8) & 0xFF) as u8;
    let b = (v & 0xFF) as u8;
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// 系统强调色与深/浅色偏好。
///
/// 读取顺序 **AccentColor → ColorizationColor**（前者是设置面板里选的强调色，
/// 后者是 DWM 合成用的色）—— 两个键都在 `HKCU\Software\Microsoft\Windows\DWM`。
/// 深色判定独立走 `AppsUseLightTheme`：任何一项读不到都不影响其它项。
#[tauri::command]
pub fn get_system_theme() -> SystemTheme {
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

    // 读不到 `AppsUseLightTheme` 时按深色 —— 本应用当前只有深色 UI，
    // 未知环境下选一个「不会闪白」的默认值比猜浅色安全。
    let dark = reg_read_dword(PERSONALIZE_KEY, "AppsUseLightTheme")
        .map(|v| v == 0)
        .unwrap_or(true);

    if let Some(v) = reg_read_dword(DWM_KEY, "AccentColor") {
        return SystemTheme {
            accent: accent_color_to_hex(v),
            dark,
            source: "registry".into(),
        };
    }
    if let Some(v) = reg_read_dword(DWM_KEY, "ColorizationColor") {
        return SystemTheme {
            accent: colorization_color_to_hex(v),
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
/// 用手写 Win32 FFI 而非 `reg.exe`：spawn 进程既慢又会闪黑框，且解析命令行输出
/// 依赖系统语言（本项目已有「读计划任务必须走 /xml」的同类教训）。写法沿用
/// `app_indexer.rs` / `hotkey.rs` 的 `#[link] extern "system"` 风格 —— 不新增依赖。
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

    /// HKEY_CURRENT_USER = 0x80000001（预定义句柄，不是真实指针）
    const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
    /// 只接受 DWORD：类型不符时让 RegGetValueW 直接返回错误，省掉一次人工判断
    const RRF_RT_REG_DWORD: u32 = 0x0000_0010;
    const ERROR_SUCCESS: i32 = 0;

    let wide = |s: &str| -> Vec<u16> {
        std::ffi::OsStr::new(s)
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

/// `<exe 根>\themes`
fn themes_root() -> PathBuf {
    crate::storage::lunac_root_dir().join("themes")
}

/// 列出全部已安装主题。目录不存在时返回空表（**不创建** —— 创建由 `themes_dir()` 负责，
/// 因为「看一眼主题列表」不该有写盘的副作用）。
#[tauri::command]
pub fn list_themes() -> Vec<ThemeInfo> {
    scan_themes_in(&themes_root())
}

/// 主题目录绝对路径；**不存在则创建**（用户要从这里放自己的主题）。
#[tauri::command]
pub fn themes_dir() -> String {
    let dir = themes_root();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        // 建不出来不阻断：仍把路径回给前端，用户至少能看到「该往哪放」以及失败原因
        crate::log::warn(format!(
            "[appearance] 创建主题目录失败 {}：{e}",
            dir.display()
        ));
    }
    dir.to_string_lossy().to_string()
}

/// 扫描指定主题根目录（命令壳与单测共用的实现）。
///
/// 单个主题失败**只跳过它自己**：解析失败 / 读不到 JSON 都记 `warn` 后继续 ——
/// 一个手工编辑坏了的主题不能让用户装有主题的列表整体消失。
fn scan_themes_in(root: &Path) -> Vec<ThemeInfo> {
    let mut out = Vec::new();

    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        // 目录不存在是正常的（首次启动 / dev 模式读不到仓库里的 themes）→ 空表
        Err(_) => return out,
    };

    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    // 按目录名排序，保证前端拿到的顺序稳定
    dirs.sort_by_key(|p| p.file_name().map(|n| n.to_os_string()));

    for dir in dirs {
        let json_path = dir.join("theme.json");
        if !json_path.exists() {
            continue;
        }
        let text = match std::fs::read_to_string(&json_path) {
            Ok(t) => t,
            Err(e) => {
                crate::log::warn(format!(
                    "[appearance] 读取主题文件失败，已跳过 {}：{e}",
                    json_path.display()
                ));
                continue;
            }
        };
        let manifest: ThemeManifest = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                crate::log::warn(format!(
                    "[appearance] 解析主题失败，已跳过 {}：{e}",
                    json_path.display()
                ));
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
            .filter_map(|(plugin, rel)| {
                resolve_asset(theme_dir, Some(rel), "icons")
                    .map(|abs| (plugin.clone(), abs))
            })
            .collect(),
    }
}

/// 把 theme.json 里的相对路径解析为绝对路径。
///
/// **theme.json 是用户可编辑的文件**，所以「相对路径」三个字必须当成不可信输入处理：
///   ① 拒绝绝对路径与盘符前缀（`C:\x`、`\\server\share\x`）；
///   ② 拒绝任何非 `Normal` 路径成分 —— 尤其 `..`（`../../windows/system32/x.png`
///      就是最典型的越界读文件尝试）；
///   ③ 拼好之后再规范化核对一次，确认仍落在主题目录内（挡住符号链接 / 目录联结
///      这类「语法上没问题、语义上跑出去」的情况）。
/// 违反任一条 → None + `warn`；文件不存在 → None（**不告警**，主题只写部分资产是正常的）。
fn resolve_asset(theme_dir: &Path, value: Option<&str>, label: &str) -> Option<String> {
    let rel = value?.trim();
    if rel.is_empty() {
        return None;
    }

    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        crate::log::warn(format!(
            "[appearance] {label} 资产是绝对路径，已拒绝：{rel}"
        ));
        return None;
    }
    if rel_path
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        crate::log::warn(format!(
            "[appearance] {label} 资产含 `..` / 根 / 盘符等非法路径成分，已拒绝：{rel}"
        ));
        return None;
    }

    let full = theme_dir.join(rel_path);
    if !full.exists() {
        return None;
    }
    if !is_within(theme_dir, &full) {
        crate::log::warn(format!(
            "[appearance] {label} 资产解析后越出主题目录，已拒绝：{rel}"
        ));
        return None;
    }
    Some(full.to_string_lossy().to_string())
}

/// `target` 规范化后是否仍在 `base` 之内。规范化失败（理论上不会，调用前已确认存在）
/// 时退回词法比较 —— 宁可误拒也不要放行一个没验证过的路径。
fn is_within(base: &Path, target: &Path) -> bool {
    let norm = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    norm(target).starts_with(norm(base))
}

// ── 单测 ─────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 每个用例独占一个临时目录（进程 id + 标签），不依赖任何真实用户目录。
    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-appearance-test-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 字节序结论必须能被本机实测值复现：AccentColor 与 ColorizationColor 是对方的字节倒序，
    /// 按各自口径解析必须得到同一个颜色（见文件头取证）。
    #[test]
    fn accent_dword_byte_order_matches_measured_evidence() {
        assert_eq!(accent_color_to_hex(0xFF20_1E19), "#191E20");
        assert_eq!(colorization_color_to_hex(0xC419_1E20), "#191E20");
    }

    /// ① 完整 theme.json：tokens 全字段 + 三类资产都解析成存在文件的绝对路径。
    #[test]
    fn parses_full_theme_json_and_resolves_assets() {
        let root = temp_root("full");
        let tdir = root.join("mytheme");
        fs::create_dir_all(tdir.join("icons")).unwrap();
        fs::write(tdir.join("builtin"), b"").unwrap();
        fs::write(tdir.join("bg.png"), b"x").unwrap();
        fs::write(tdir.join("pattern.png"), b"x").unwrap();
        fs::write(tdir.join("icons").join("ai.png"), b"x").unwrap();
        fs::write(
            tdir.join("theme.json"),
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
        let t = &got[0];
        assert_eq!(t.manifest.id, "mytheme");
        assert_eq!(t.manifest.name, "My Theme");
        assert_eq!(t.manifest.version, "1.0.0");
        assert_eq!(t.manifest.author, "tester");
        assert_eq!(t.manifest.tokens.accent.as_deref(), Some("#C0A0A0"));
        assert_eq!(t.manifest.tokens.text_dim.as_deref(), Some("#B2A9A3"));
        assert_eq!(t.manifest.tokens.surface.as_deref(), Some("28,26,32"));
        assert_eq!(t.manifest.tokens.radius_search.as_deref(), Some("14px"));
        assert_eq!(t.manifest.tokens.radius_results.as_deref(), Some("0"));
        assert_eq!(t.manifest.tokens.pattern_opacity, Some(0.18));
        assert!(t.builtin, "目录内有 builtin 标记文件 → builtin = true");
        assert_eq!(t.dir, tdir.to_string_lossy().to_string());

        assert_eq!(
            t.resolved.background,
            Some(tdir.join("bg.png").to_string_lossy().to_string())
        );
        assert_eq!(
            t.resolved.search_pattern,
            Some(tdir.join("pattern.png").to_string_lossy().to_string())
        );
        assert_eq!(
            t.resolved.icons.get("ai-agent"),
            // theme.json 里写的是 "icons/ai.png"，拼接后原样保留正斜杠
            Some(&tdir.join("icons/ai.png").to_string_lossy().to_string())
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// ② 最小 manifest `{"id":"x","name":"y"}` 也能解析，其余字段走默认值。
    #[test]
    fn parses_minimal_manifest_with_defaults() {
        let root = temp_root("minimal");
        let tdir = root.join("bare");
        fs::create_dir_all(&tdir).unwrap();
        fs::write(tdir.join("theme.json"), r#"{"id":"x","name":"y"}"#).unwrap();

        let got = scan_themes_in(&root);
        assert_eq!(got.len(), 1);
        let t = &got[0];
        assert_eq!(t.manifest.id, "x");
        assert_eq!(t.manifest.name, "y");
        assert_eq!(t.manifest.version, "");
        assert_eq!(t.manifest.author, "");
        assert!(t.manifest.tokens.accent.is_none());
        assert!(t.manifest.tokens.pattern_opacity.is_none());
        assert!(t.manifest.assets.background.is_none());
        assert!(t.manifest.assets.icons.is_empty());
        assert!(!t.builtin, "没有 builtin 标记文件 → builtin = false");
        assert!(t.resolved.background.is_none());

        // 目录不存在 → 空表，且**不创建**目录
        let missing = root.join("nope");
        assert!(scan_themes_in(&missing).is_empty());
        assert!(!missing.exists());

        let _ = fs::remove_dir_all(&root);
    }

    /// ③ 路径穿越（`..`）必须被拒；且「主题目录外真实存在的文件」也不能靠 `../` 拿到。
    #[test]
    fn rejects_parent_directory_escape() {
        let root = temp_root("traversal");
        let tdir = root.join("evil");
        fs::create_dir_all(&tdir).unwrap();
        // 故意让越界目标真实存在：这样「被拒」只能来自防护，而不是「文件不存在」
        fs::write(root.join("outside.png"), b"x").unwrap();

        let assets = ThemeAssets {
            background: Some("../../windows/system32/x.png".into()),
            search_pattern: Some("../outside.png".into()),
            icons: HashMap::new(),
        };
        let r = resolve_assets(&tdir, &assets);
        assert!(r.background.is_none());
        assert!(r.search_pattern.is_none());

        let _ = fs::remove_dir_all(&root);
    }

    /// ⑤ 绝对路径资产必须被拒（同样让目标真实存在，排除「不存在」这个解释）。
    #[test]
    fn rejects_absolute_asset_path() {
        let root = temp_root("absolute");
        let tdir = root.join("theme");
        fs::create_dir_all(&tdir).unwrap();
        let outside = std::env::temp_dir().join(format!(
            "lunac-appearance-abs-target-{}.png",
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
        let r = resolve_assets(&tdir, &assets);
        assert!(r.background.is_none());
        assert!(r.icons.is_empty());

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&outside);
    }

    /// ④ 字段类型错误（`pattern_opacity` 给字符串）时整体解析失败，`list_themes` 跳过它，
    ///    但**不能连累**同目录下的正常主题。
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

        // serde 层：类型不符确实解析失败（不会静默变成 None）
        assert!(serde_json::from_str::<ThemeManifest>(
            r#"{"id":"b","tokens":{"pattern_opacity":"0.18"}}"#
        )
        .is_err());

        let got = scan_themes_in(&root);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].manifest.id, "good");

        let _ = fs::remove_dir_all(&root);
    }
}
