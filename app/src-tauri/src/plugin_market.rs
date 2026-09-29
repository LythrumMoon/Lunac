// src-tauri/src/plugin_market.rs
// 插件市场（L1，2026-09-21）：从 https 的 zip 安装第三方插件到固定目录。
//
// 目录约定：**<exe 根>\Modules\<id>\**（2026-09-28 从 `plugins\` 改名而来；与 `skills\` / `tools\` 同级，
// 都在安装根下，卸载随目录清掉。旧 `plugins\` 由 `migrate_legacy_plugins_dir()` 一次性搬过来）。
// 包里必须带一份清单 `lunac-plugin.json`（见 `PluginManifest`），入口是**已编译好的 ESM**（`import()`），
// 因为前端 CSP 是 `script-src 'self' 'unsafe-inline' https://asset.localhost` —— 插件代码只能经
// **asset 协议**从磁盘加载，**不能走 CDN**（同 ai-spec §3.7 的 KaTeX 缺陷是同一条约束）。
//
// **依赖随插件一起装**（2026-09-28 加）：清单里的 `dependencies[]` 由宿主在插件落盘后逐条拉取
// （见 `install_dependencies`）—— 这是「release 里没有 librespot」这类问题的根治办法：
// 依赖不再由构建脚本塞进安装包，而是**跟着用到它的那个插件**走。依赖只收 https、
// 可选 sha256 校验、落点必须留在插件目录内。
//
// **这份代码解压的是「可执行代码」，所以校验比 tools / skills 那两个先例更严**：
//   ① 只收 https（明文 http 的 zip 会被解压执行）；
//   ② 压缩包与**解压后总量**都有上限（zip bomb）；
//   ③ 逐条拒绝绝对路径、`..`、超长路径（路径穿越）；
//   ④ id 只允许安全字符（它直接当目录名）；
//   ⑤ **同 id 视为「重装 / 升级」**（2026-09-28 改，原为「已存在即拒绝」）：旧目录先改名成
//      `.old-*` 备份 —— 新包或依赖任一步失败就把旧的搬回来，成功才删备份。于是「装重一次」
//      既不会无声换掉代码（失败可回滚），也不必让用户先卸载再装（用户要的就是一键升级）；
//   ⑥ 先解到 `.staging-*` 再改名进正式目录，失败即清理，不留半成品。
//
// 安全边界要诚实：这里做的是**防事故**（写坏路径、把包塞爆），**不是**防恶意 ——
// 插件是用户自己选择安装的可执行代码，装上就等同于本机权限（与 tools\ 的 shell handler 同族）。
// 界面上必须把「来源 + 这是可执行代码」显著写出来，别让用户以为它只是个配置。

use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 插件清单文件名（放在包根，也可放在 zip 里的单层子目录下 —— 见 `find_manifest`）。
pub const MANIFEST_FILE: &str = "lunac-plugin.json";
/// 压缩包大小上限。插件里常有引擎与图片，给得比 tools / skills 宽，但仍要拦住「几百 MB 的包」。
pub const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024;
/// 解压后总量上限（zip bomb 防护）。
pub const MAX_TOTAL_BYTES: u64 = 192 * 1024 * 1024;
/// 条目数上限（防「十万个空文件」）。
pub const MAX_ENTRIES: usize = 4000;
/// 单个依赖文件的体积上限（2026-09-28）。依赖里的引擎比插件包本身大得多
/// （PaddleOCR 模型 / librespot 二进制都在几十 MB 量级），所以给到 512 MB 仍要拦住「几个 GB」。
pub const MAX_DEP_BYTES: u64 = 512 * 1024 * 1024;
/// 依赖条目数上限（防清单里写一万条把安装拖死）。
pub const MAX_DEP_ENTRIES: usize = 32;
/// 声明的宿主能力条数上限（权限名很短，16 条已经远超任何真实插件）。
pub const MAX_PERMISSIONS: usize = 16;
/// 宿主当前**认识**的能力（2026-09-28）。**只加不减**（旧插件会带着老名字装在盘上），
/// 清单里出现不认识的名字**只 warn、不拒绝安装** —— 那可能是给更新版宿主声明的能力，
/// 在旧宿主上装不上反而是坏事（它只是拿不到那个能力，与「装不上」是两回事）。
pub const KNOWN_PERMISSIONS: &[&str] = &["layout.takeover", "window.resize", "window.float"];
/// id 长度上限（同时是目录名长度上限）。
const MAX_ID_CHARS: usize = 48;

/// 插件市场**索引文件**的地址（2026-09-21，L1 续）：市场面板要能列出「没装但可下载」的插件，
/// 而本地扫描只能看见「已经装了的」。
///
/// **2026-09-28 换到独立公开仓库 `LythrumMoon/lunac-plugins`**（原先指向主仓库
/// `LythrumMoon/Lunac` 的 `plugins/index.json`）。原因很硬：**主仓库是私有的**，而
/// `raw.githubusercontent.com` 对私有仓库要鉴权 ⇒ 终端用户的宿主**永远拉不到**那份索引
/// （等于这个功能在真机上一直是坏的），包也一样下不到。现在索引与包都在公开仓库里，
/// 由 `scripts/build-plugins.ps1` + `scripts/publish-plugins.ps1` 生成并推送。
///
/// 为什么由**宿主**去拉而不是前端 `fetch()`：CSP 的 `default-src` 不含 github 域（`connect-src`
/// 未单独声明 ⇒ 回落 default-src），前端联网会被直接拦掉。放进宿主还多一层好处 ——
/// **索引里的每个 URL 都要按插件包的标准重新校验**（见 `parse_index`），前端拿到手的已经是
/// 筛过的结果，别让「谁写索引谁就决定了前端能下什么」成为一条绕过校验的路。
pub const INDEX_URL: &str =
    "https://raw.githubusercontent.com/LythrumMoon/lunac-plugins/main/index.json";
/// 索引文件体积上限（纯文本清单，1 MB 足够列几千条；超了就是拿它当文件传输用）。
pub const MAX_INDEX_BYTES: u64 = 1024 * 1024;
/// 索引条目数上限（防「十万条空条目」把面板拖死）。
pub const MAX_INDEX_ENTRIES: usize = 500;

/// 清单里的 `window` 段：插件对自己**悬浮窗形态**的参数声明（2026-09-29 加，为桌宠 L1）。
///
/// **为什么是清单字段、不是 `permissions` 里的一条**：`permissions` 回答的是「这个插件
/// 被允许动什么」（声明 + 告知语义），而这一段是**建窗参数**（多大、要不要标题栏、
/// 进不进任务栏）。把尺寸塞进能力名里只会逼出 `window.size.280x380` 这种怪东西，
/// 而且它并不比一个 JSON 对象表达得更多。注意：**「能不能开独立窗」仍然只认
/// `window.float`**，这一段只在该插件已经在开窗时生效。
///
/// **缺省值逐项等于加这个字段之前的行为**（`open()` 里那三个 `.xxx()` 的历史取值），
/// 所以没写这一段的老插件建出来的窗与 2026-09-29 之前完全一致。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginWindowShape {
    /// 初始宽度（逻辑像素）。`0` = 用宿主按 id 给的那一档缺省值。
    #[serde(default)]
    pub width: f64,
    /// 初始高度。`0` = 同上。
    #[serde(default)]
    pub height: f64,
    /// 最小宽度。`0` = 同上。
    #[serde(default)]
    pub min_width: f64,
    /// 最小高度。`0` = 同上。
    #[serde(default)]
    pub min_height: f64,
    /// 能不能手动缩放。缺省 `true`（与历史行为一致）。
    #[serde(default = "default_true")]
    pub resizable: bool,
    /// 进不进任务栏。缺省 `false`（= 进），与历史行为一致。
    #[serde(default)]
    pub skip_taskbar: bool,
    /// 建窗即置顶。缺省 `true`，与历史行为一致。
    #[serde(default = "default_true")]
    pub always_on_top: bool,
    /// 带不带宿主那根标题栏（置顶 / 最小化 / 关闭三个按钮）。缺省 `true`。
    /// 桌宠这一类「窗口就是那片画面本身」的插件写 `false`，前端据此收起标题栏。
    #[serde(default = "default_true")]
    pub chrome: bool,
}

fn default_true() -> bool {
    true
}

/// 窗口尺寸的合法区间（逻辑像素）。下限挡住「误写到 0.5」，上限挡住「把窗口顶出屏幕
/// 且用户抓不回来」—— 与 `plugin_window::plugin_window_resize` 的 clamp 口径同源。
const MIN_WINDOW_SIDE: f64 = 100.0;
const MAX_WINDOW_SIDE: f64 = 4000.0;

/// 校验 `window` 段。口径：**能静态判定的一律拒绝整包**（同清单其它字段）。
///
/// 只校验「数值是不是人写的」这一类事实，不替插件决定该多大 —— 尺寸本身是它的自由，
/// 0（= 用宿主缺省）也是合法写法。
fn validate_window_shape(shape: &PluginWindowShape) -> Result<(), String> {
    let sides = [
        ("width", shape.width),
        ("height", shape.height),
        ("minWidth", shape.min_width),
        ("minHeight", shape.min_height),
    ];
    for (name, v) in sides {
        if !v.is_finite() {
            return Err(format!("window.{name} 不是有限数：{v}"));
        }
        if v != 0.0 && !(MIN_WINDOW_SIDE..=MAX_WINDOW_SIDE).contains(&v) {
            return Err(format!(
                "window.{name} = {v} 超出允许区间（0 或 {MIN_WINDOW_SIDE}~{MAX_WINDOW_SIDE}）"
            ));
        }
    }
    // 「最小 > 默认」在建窗那一刻会被系统夹一次，表现为「命令成功、窗口没动」。
    // 两处都给过的时候才发现问题就太晚了，这里直接拒。
    if shape.min_width != 0.0 && shape.width != 0.0 && shape.min_width > shape.width {
        return Err("window.minWidth 大于 window.width".into());
    }
    if shape.min_height != 0.0 && shape.height != 0.0 && shape.min_height > shape.height {
        return Err("window.minHeight 大于 window.height".into());
    }
    Ok(())
}

/// 插件清单。除 `id` 外全部有默认值：缺字段的包也能装，但至少要能读出 id 与入口。
///
/// 字段刻意与前端 `Plugin` 契约（`app/src/plugins/registry.ts`）对齐：`keywords` 参与搜索匹配、
/// `icon` 是 emoji 兜底、`description` 是英文兜底（显示名与描述仍优先走 i18n 的 `plugin.<id>`）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PluginManifest {
    /// 插件 id，**同时是目录名** ⇒ 只允许 `[a-z0-9._-]`，不许以 `.` 开头
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub version: String,
    /// ESM 入口，相对清单所在目录，如 `index.js`。缺省 `index.js`。
    #[serde(default = "default_entry")]
    pub entry: String,
    /// 作者 / 仓库地址（界面上显示来源用）
    #[serde(default)]
    pub homepage: String,
    /// 随插件一起装的依赖（2026-09-28，见 `PluginDependency`）。缺省 = 无依赖。
    #[serde(default)]
    pub dependencies: Vec<PluginDependency>,
    /// **声明的宿主能力**（2026-09-28）：插件要用的宿主特权，逗号分隔的一组短名，
    /// 如 `["layout.takeover"]`。宿主桥只放行**声明过**的能力，界面上也如实列出来。
    ///
    /// 诚实说明：这一层目前是**声明 + 告知**，不是沙箱 —— 插件是本机可执行代码，
    /// 它绕过桥直接 `invoke()` 宿主命令照样能调（这与「装谁 = 信任谁」是同一条边界）。
    /// 它真正挡住的是「不小心用了没声明的东西」与「用户不知道装的东西要什么权限」。
    #[serde(default)]
    pub permissions: Vec<String>,
    /// 悬浮窗形态（2026-09-29，见 `PluginWindowShape`）。不写这一段 = 宿主缺省形态。
    ///
    /// 由**宿主**在建窗时读（`plugin_window::declared_shape()`），不进前端契约 ——
    /// 前端不需要知道窗口多大，它只管往 `#results-list` 里画东西。
    #[serde(default)]
    pub window: Option<PluginWindowShape>,
}

fn default_entry() -> String {
    "index.js".to_string()
}

/// 一条依赖（清单里的 `dependencies[]` 元素，2026-09-28）。
///
/// 两种形态（`kind` 决定，缺省 `file`）：
///   · **`file`**（默认）：从 `url`（**必须 https**）下载单个文件到插件目录内的 `dest`。
///     适合「引擎二进制 / 模型」这类固定文件（librespot.exe / PaddleOCR 模型）。
///     可选 `sha256`：写了就**必须**对上，否则整条拒绝（供应链完整性）。
///   · **`npm`**：`npm install --prefix <插件目录> <package>@<version>`，装进插件目录的
///     `node_modules\`。要求用户机器上有 node/npm —— 找不到就**如实报错**，不静默跳过。
///
/// **为什么不在构建期把依赖塞进安装包**：依赖只对「用到它的那个插件」有意义，
/// 塞进主安装包会让所有用户替少数人的功能买单，而且 release 一旦漏拷（librespot 就是这个）
/// 就只能等下一个版本。改成跟着插件走，装/卸/升级都在一起。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PluginDependency {
    /// `file`（缺省）/ `npm`
    #[serde(default, rename = "type")]
    pub kind: String,
    /// `file`：https 下载地址
    #[serde(default)]
    pub url: String,
    /// `npm`：包名（可带 scope，如 `@scope/pkg`）。缺省退回 `url` 字段
    #[serde(default)]
    pub package: String,
    /// `npm`：版本范围（可选，空 = latest）
    #[serde(default)]
    pub version: String,
    /// `file`：相对插件目录的落点（如 `bin\librespot.exe`）。不许越界（走 `safe_join`）
    #[serde(default)]
    pub dest: String,
    /// `file`：期望的 sha256（十六进制，大小写不敏感）。空 = 不校验（安装时会 warn 留痕）
    #[serde(default)]
    pub sha256: String,
}

impl PluginDependency {
    fn is_npm(&self) -> bool {
        self.kind.eq_ignore_ascii_case("npm")
    }
}

/// 校验依赖条目（`parse_manifest` 调用；**校验不过整包拒绝**，同清单其它字段的口径）。
///
/// 只在**能静态判定**的范围内校验（https / 落点不越界 / 字段齐不齐），
/// 真正的落盘与校验和比对在 `install_dependencies` 里。
fn validate_dependency(d: &PluginDependency) -> Result<(), String> {
    if d.is_npm() {
        let pkg = if d.package.trim().is_empty() {
            d.url.trim()
        } else {
            d.package.trim()
        };
        if pkg.is_empty() {
            return Err("npm 依赖缺 package（包名）".into());
        }
        // 包名只允许 npm 命名规则用到的字符，避免把 `--foo` 之类当参数注进 npm 命令行
        let ok = pkg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '/'));
        if !ok || pkg.starts_with('-') {
            return Err(format!("npm 包名非法：{pkg}"));
        }
        return Ok(());
    }
    if !matches!(d.kind.as_str(), "" | "file") {
        return Err(format!("依赖 type 只支持 file / npm，收到：{}", d.kind));
    }
    if !d.url.starts_with("https://") {
        return Err(format!("依赖只允许 https 地址：{}", d.url));
    }
    if d.dest.trim().is_empty() {
        return Err(format!("依赖缺少 dest（落点）：{}", d.url));
    }
    if safe_join(Path::new("."), &d.dest).is_none() {
        return Err(format!("依赖 dest 越界：{}", d.dest));
    }
    Ok(())
}

/// 校验一条能力名：只允许 `[a-z0-9._-]`、非空、≤ 32 字符。**不做白名单拒绝** ——
/// 见 `KNOWN_PERMISSIONS` 的注释（不认识的名字只 warn）。
fn validate_permission(name: &str) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 32 {
        return Err(format!("能力名长度不合法：{name}"));
    }
    let ok = n
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'));
    if !ok {
        return Err(format!("能力名只允许小写字母/数字/./-/_：{name}"));
    }
    Ok(())
}

/// 列给前端的已安装插件（= 清单 + 落点，外加「能不能用」的判定）。
///
/// `rename_all = "camelCase"`：前端接口（`app/src/plugins/market.ts` 的 `MarketPluginInfo`）
/// 按 JS 习惯写 `entryPath`，别让调用方去记哪一个字段是 snake_case。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPlugin {
    pub id: String,
    pub name: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub icon: String,
    pub version: String,
    pub entry: String,
    pub homepage: String,
    /// 插件目录绝对路径（前端用 `convertFileSrc` 拼出 ESM 入口 URL）
    pub dir: String,
    /// 入口文件的绝对路径（前端直接用它，免得自己拼相对路径）
    pub entry_path: String,
    /// 清单与入口都通过校验
    pub valid: bool,
    /// `valid = false` 时的原因（原样显示在面板上，不猜）
    pub error: String,
    /// 这个插件声明的依赖（2026-09-28）。界面上只是「它要拉什么」的提示，
    /// 真正的拉取在 `install_dependencies`（装插件时顺带完成）。
    #[serde(default)]
    pub dependencies: Vec<PluginDependency>,
    /// 这个插件声明的宿主能力（2026-09-28）。界面上如实列出（见 `PluginManifest::permissions`）。
    #[serde(default)]
    pub permissions: Vec<String>,
}

/// 插件根目录：`<exe 根>\Modules`（2026-09-28 由 `plugins` 改名而来；与 skills / tools 同级）。
pub fn plugins_dir() -> PathBuf {
    crate::storage::lunac_root_dir().join("Modules")
}

/// 一次性迁移：旧版把插件装在 `<exe 根>\plugins\`，改成 `Modules\` 之后要把已装的搬过来。
///
/// **只在目标不存在时搬**（不覆盖新目录里的东西）；搬不动就**留着旧目录**并 warn ——
/// 宁可让旧的少一层识别，也不要删掉用户已经装好的插件。
/// 返回搬过来的条目数（仅用于日志与测试）。
pub fn migrate_legacy_plugins_dir(root: &Path) -> usize {
    let old = root.join("plugins");
    let new = root.join("Modules");
    if !old.is_dir() {
        return 0;
    }
    if let Err(e) = fs::create_dir_all(&new) {
        crate::log::warn(format!("旧插件目录迁移失败（建新目录）：{e}"));
        return 0;
    }
    let Ok(entries) = fs::read_dir(&old) else {
        return 0;
    };
    let mut moved = 0usize;
    for e in entries.flatten() {
        let src = e.path();
        if !src.is_dir() {
            continue;
        }
        let target = new.join(e.file_name());
        if target.exists() {
            crate::log::warn(format!(
                "旧插件 {} 与 Modules 下的同名目录冲突，保留 Modules 里那份",
                e.file_name().to_string_lossy()
            ));
            continue;
        }
        match fs::rename(&src, &target) {
            Ok(()) => moved += 1,
            Err(err) => crate::log::warn(format!("旧插件迁移失败：{err}")),
        }
    }
    // 只剩空目录（或本来就没有子目录）才删掉旧的；还有东西就留着，别动用户的文件
    if fs::read_dir(&old).map(|mut it| it.next().is_none()).unwrap_or(false) {
        let _ = fs::remove_dir(&old);
    }
    if moved > 0 {
        crate::log::info(format!("旧 plugins\\ 目录已迁移 {moved} 个插件到 Modules\\"));
    }
    moved
}

/// id 合法性 —— 它**直接当目录名**用，所以只允许安全字符。
///
/// 拒绝项：空、超长、非 `[a-z0-9._-]`、以 `.` 开头（避免与 `.staging-*` 之类的内部目录撞上）、
/// 含 `..`（防穿越）、含 Windows 保留字符（已被字符集挡住）。
pub fn is_safe_id(id: &str) -> bool {
    let id = id.trim();
    if id.is_empty() || id.chars().count() > MAX_ID_CHARS {
        return false;
    }
    if id.starts_with('.') || id.contains("..") {
        return false;
    }
    id.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.')
}

/// 入口必须是**相对**的、以 `.js` / `.mjs` 结尾、不含 `..`。
pub fn validate_entry(entry: &str) -> Result<(), String> {
    let e = entry.trim().replace('\\', "/");
    if e.is_empty() {
        return Err("清单里的 entry 是空的".into());
    }
    if e.starts_with('/') || e.contains(':') || e.split('/').any(|seg| seg == "..") {
        return Err(format!("entry 必须是包内相对路径：{entry}"));
    }
    if !(e.ends_with(".js") || e.ends_with(".mjs")) {
        return Err(format!("entry 必须是 .js / .mjs（ESM 模块）：{entry}"));
    }
    Ok(())
}

/// 解析并校验清单文本。**校验不过一个字段都不落盘**（调用方据此拒绝整包）。
pub fn parse_manifest(text: &str) -> Result<PluginManifest, String> {
    // BOM 容错：编辑器常写 BOM，而 serde_json 见 BOM 直接判非法（同 hooks.json 的处置）
    let text = text.trim_start_matches('\u{feff}');
    let m: PluginManifest =
        serde_json::from_str(text).map_err(|e| format!("插件清单不是合法 JSON：{e}"))?;
    if !is_safe_id(&m.id) {
        return Err(format!(
            "插件 id 非法（只允许小写字母/数字/-/_/.，且不能以 . 开头）：{}",
            m.id
        ));
    }
    validate_entry(&m.entry)?;
    if m.dependencies.len() > MAX_DEP_ENTRIES {
        return Err(format!(
            "依赖条目过多（{} 条，上限 {MAX_DEP_ENTRIES}）",
            m.dependencies.len()
        ));
    }
    for d in &m.dependencies {
        validate_dependency(d)?;
    }
    if m.permissions.len() > MAX_PERMISSIONS {
        return Err(format!(
            "声明的能力过多（{} 条，上限 {MAX_PERMISSIONS}）",
            m.permissions.len()
        ));
    }
    for p in &m.permissions {
        validate_permission(p)?;
        let p = p.trim();
        if !KNOWN_PERMISSIONS.contains(&p) {
            // 只 warn：这可能是给更新版宿主声明的能力（见 KNOWN_PERMISSIONS 的注释）
            crate::log::warn(format!(
                "插件 {} 声明了宿主不认识的能力「{p}」（已忽略，不影响安装）",
                m.id
            ));
        }
    }
    if let Some(shape) = &m.window {
        validate_window_shape(shape)?;
    }
    Ok(m)
}

/// 市场索引里的一条：**只描述「去哪下」**，不描述包内容（包的真相在包自己的清单里）。
///
/// 字段名对齐前端 `MarketIndexEntry`（`app/src/plugins/market.ts`）；`url` 必须是 https 的
/// 插件 zip 地址，其余字段只用于**下载前的预览**（装了之后一律以包内清单为准）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct PluginIndexEntry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    /// https 的插件 zip 地址（唯一的必填项之一）
    pub url: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub homepage: String,
}

/// 解析并**逐条校验**市场索引（顶层是一个数组）。
///
/// 校验口径（与「这个包能不能被下载执行」同源，不是另立一套）：
///   - `id` 必须过 `is_safe_id()`（它将来要当目录名用）；
///   - `url` 必须 `https://` 开头（明文 http 的包会被解压执行）；
///   - `name` 不能全空（否则面板上是一行没有名字的东西）。
///
/// **坏条目只丢自己，不丢整份索引**：一条写错的 URL 不该让整个市场变空 —— 这与
/// 「坏包必须可见」（`list_installed`）是两条不同的处置：那边是用户**已经装在盘上**的东西，
/// 消失了他会找不到；这边是**一条指向别处的推荐**，丢掉即可，但要 `warn` 留痕。
/// 同 id 重复取**第一条**（索引是人写的，重复意味着作者改漏了，先出现的那条是他先写的）。
pub fn parse_index(text: &str) -> Result<Vec<PluginIndexEntry>, String> {
    let text = text.trim_start_matches('\u{feff}');
    let raw: Vec<PluginIndexEntry> =
        serde_json::from_str(text).map_err(|e| format!("插件索引不是合法 JSON：{e}"))?;
    let mut out: Vec<PluginIndexEntry> = Vec::new();
    for e in raw {
        if !is_safe_id(&e.id) {
            crate::log::warn(format!("插件索引里有一条 id 非法，已跳过：{}", e.id));
            continue;
        }
        if !e.url.starts_with("https://") {
            crate::log::warn(format!("插件索引里的 {} 不是 https 地址，已跳过", e.id));
            continue;
        }
        if e.name.trim().is_empty() {
            crate::log::warn(format!("插件索引里的 {} 没有名字，已跳过", e.id));
            continue;
        }
        if out.iter().any(|o| o.id == e.id) {
            crate::log::warn(format!("插件索引里的 {} 重复，保留先出现的那条", e.id));
            continue;
        }
        out.push(e);
        if out.len() >= MAX_INDEX_ENTRIES {
            crate::log::warn(format!("插件索引超过 {MAX_INDEX_ENTRIES} 条，其余已忽略"));
            break;
        }
    }
    Ok(out)
}

/// 把 zip 里的条目名规范化成 root 下的路径；**任何越界都返回 None**。
///
/// 判据（逐条，缺一不可）：非空、不是绝对路径（`/` 或 `\` 开头、含盘符 `:`）、
/// 分段里没有 `..`、没有空段（`a//b`）、分段不以空格或 `.` 结尾（Windows 会把这种名字悄悄改掉，
/// 让「写下去的文件」与「校验过的路径」不是同一个东西）。反斜杠统一按分隔符看（zip 规范用 `/`，
/// 但手工打的包常混用，而它们在 Windows 上都会被当成目录分隔符）。
pub fn safe_join(root: &Path, name: &str) -> Option<PathBuf> {
    let unified = name.replace('\\', "/");
    if unified.is_empty() || unified.starts_with('/') || unified.contains(':') {
        return None;
    }
    let mut out = root.to_path_buf();
    let mut depth = 0usize;
    for seg in unified.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return None;
        }
        if seg.ends_with(' ') || seg.ends_with('.') {
            return None;
        }
        out.push(seg);
        depth += 1;
        if depth > 16 {
            return None;
        }
    }
    Some(out)
}

/// 解压 zip 到 `dest`（`dest` 必须已经存在或可被创建）。
///
/// `max_total` 是解压总量上限 —— 抽成参数只为让单测能用极小的上限造 zip bomb 场景，
/// 生产调用一律用 `MAX_TOTAL_BYTES`。
pub fn extract_zip<R: Read + Seek>(reader: R, dest: &Path, max_total: u64) -> Result<(), String> {
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| format!("不是合法的 zip：{e}"))?;
    if archive.len() > MAX_ENTRIES {
        return Err(format!("包内条目过多（{} 个）", archive.len()));
    }
    fs::create_dir_all(dest).map_err(|e| format!("建目录失败：{e}"))?;
    let mut written: u64 = 0;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("读第 {i} 个条目失败：{e}"))?;
        // 只收普通文件与目录 —— 符号链接 / 设备节点一律跳过（它们的落点是「别的地方」）
        if !file.is_file() && !file.is_dir() {
            continue;
        }
        let raw_name = file.name().to_string();
        let Some(path) = safe_join(dest, &raw_name) else {
            return Err(format!("包里含越界路径，已拒绝安装：{raw_name}"));
        };
        if file.is_dir() {
            fs::create_dir_all(&path).map_err(|e| format!("建目录失败：{e}"))?;
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("建目录失败：{e}"))?;
        }
        let mut out = fs::File::create(&path).map_err(|e| format!("写文件失败：{e}"))?;
        // 分块拷贝 + 累计计数：不信 zip 头里声明的大小（那是包自己写的），按真实写出的字节算
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buf).map_err(|e| format!("解压失败：{e}"))?;
            if n == 0 {
                break;
            }
            written += n as u64;
            if written > max_total {
                return Err(format!(
                    "解压后体积超过上限（{} MB），已拒绝安装",
                    max_total / 1024 / 1024
                ));
            }
            out.write_all(&buf[..n])
                .map_err(|e| format!("写文件失败：{e}"))?;
        }
    }
    Ok(())
}

/// 在解压出来的目录里找清单：**包根**或**唯一的一层子目录**（GitHub 的 zip 打包会给一个顶层目录）。
fn find_manifest(root: &Path) -> Result<PathBuf, String> {
    let direct = root.join(MANIFEST_FILE);
    if direct.is_file() {
        return Ok(direct);
    }
    let mut found: Option<PathBuf> = None;
    let entries = fs::read_dir(root).map_err(|e| format!("读目录失败：{e}"))?;
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let cand = p.join(MANIFEST_FILE);
        if cand.is_file() {
            // 有第二个就说明形状不唯一 —— 宁可拒绝，也不要猜用户装的是哪一个
            if found.is_some() {
                return Err(format!("包里有不止一份 {MANIFEST_FILE}，无法判断用哪份"));
            }
            found = Some(cand);
        }
    }
    found.ok_or_else(|| format!("包里没有 {MANIFEST_FILE}"))
}

/// 安装一份 zip 字节流：解到 staging → 校验清单 → 改名进 `<plugins_root>\<id>` → 装依赖。
///
/// 返回值是插件 id。**同 id 视为「重装 / 升级」**（2026-09-28 改）：旧目录先改名成 `.old-*`
/// 备份，新包或依赖任一步失败就把旧的搬回来、成功才删备份。于是「装重一次」既不会无声换掉
/// 代码（失败可回滚），也不必让用户先卸载再装。
pub fn install_from_bytes(bytes: &[u8], plugins_root: &Path) -> Result<String, String> {
    if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(format!(
            "压缩包超过上限（{} MB）",
            MAX_ARCHIVE_BYTES / 1024 / 1024
        ));
    }
    fs::create_dir_all(plugins_root).map_err(|e| format!("建插件目录失败：{e}"))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let staging = plugins_root.join(format!(".staging-{stamp}"));
    let _ = fs::remove_dir_all(&staging);

    // ── 第一步：解压 + 校验。**这一步失败还没碰过任何已有插件**，只需清 staging ──
    let prepared = (|| -> Result<(PluginManifest, PathBuf), String> {
        extract_zip(std::io::Cursor::new(bytes), &staging, MAX_TOTAL_BYTES)?;
        let manifest_path = find_manifest(&staging)?;
        let text = fs::read_to_string(&manifest_path)
            .map_err(|e| format!("读清单失败：{e}"))?;
        let manifest = parse_manifest(&text)?;
        let plugin_root = manifest_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| staging.clone());
        let entry_path = safe_join(&plugin_root, &manifest.entry)
            .ok_or_else(|| format!("entry 越界：{}", manifest.entry))?;
        if !entry_path.is_file() {
            return Err(format!(
                "清单里写的入口不存在：{}",
                manifest.entry
            ));
        }
        Ok((manifest, plugin_root))
    })();
    let (manifest, plugin_root) = match prepared {
        Ok(v) => v,
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
    };

    let target = plugins_root.join(&manifest.id);
    let backup = plugins_root.join(format!(".old-{stamp}"));
    let had_old = target.exists();
    // ── 第二步：把旧版本挪去备份（挪不动就别动目标，直接失败）──
    if had_old {
        let _ = fs::remove_dir_all(&backup);
        if let Err(e) = fs::rename(&target, &backup) {
            let _ = fs::remove_dir_all(&staging);
            return Err(format!("备份旧版本失败（未改动已装插件）：{e}"));
        }
    }

    // ── 第三步：把解压好的目录改名进正式位置 ──
    let inner = plugins_root.join(format!(".staging-{stamp}-inner"));
    let placed = (|| -> Result<(), String> {
        // 包的形状可能有两种：清单就在 staging 根（整包就是插件），或在 staging\<子目录>（GitHub zip）
        if plugin_root == staging {
            fs::rename(&staging, &target).map_err(|e| format!("落盘失败：{e}"))?;
        } else {
            fs::rename(&plugin_root, &inner).map_err(|e| format!("落盘失败：{e}"))?;
            let _ = fs::remove_dir_all(&staging);
            fs::rename(&inner, &target).map_err(|e| format!("落盘失败：{e}"))?;
        }
        Ok(())
    })();
    if let Err(e) = placed {
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_dir_all(&inner);
        if had_old {
            let _ = fs::rename(&backup, &target);
        }
        return Err(e);
    }

    // ── 第四步：装依赖。失败 = 整次安装失败（把新版本删掉、旧版本搬回来）──
    if let Err(e) = install_dependencies(&target, &manifest.dependencies) {
        let _ = fs::remove_dir_all(&target);
        if had_old {
            let _ = fs::rename(&backup, &target);
        }
        return Err(format!("依赖未装好，已回滚到上一版：{e}"));
    }

    if had_old {
        let _ = fs::remove_dir_all(&backup);
    }
    Ok(manifest.id.clone())
}

/// 逐条安装清单里的依赖（`dependencies[]`）。**任意一条失败即中止**，由调用方决定是否回滚。
///
/// 顺序执行、不并发：依赖条数是个位数，并发带来的收益抵不过「并发失败时哪一半落了盘」的复杂度。
pub fn install_dependencies(plugin_dir: &Path, deps: &[PluginDependency]) -> Result<(), String> {
    if deps.is_empty() {
        return Ok(());
    }
    if deps.len() > MAX_DEP_ENTRIES {
        return Err(format!("依赖条目过多（{} 条）", deps.len()));
    }
    for (i, d) in deps.iter().enumerate() {
        let what = if d.is_npm() {
            let pkg = if d.package.trim().is_empty() { d.url.trim() } else { d.package.trim() };
            format!("npm:{pkg}")
        } else {
            d.url.clone()
        };
        // 清单在 `parse_manifest` 已校验过一次；这里再校验一次是**双保险** ——
        // `install_dependencies` 是 pub，将来可能被「只补依赖」的入口直接调用。
        validate_dependency(d).map_err(|e| format!("第 {} 条依赖（{what}）非法：{e}", i + 1))?;
        let r = if d.is_npm() {
            install_npm_dependency(plugin_dir, d)
        } else {
            install_file_dependency(plugin_dir, d)
        };
        r.map_err(|e| format!("第 {} 条依赖（{what}）安装失败：{e}", i + 1))?;
    }
    Ok(())
}

/// 下载一个依赖文件到插件目录内（`file` 形态）。
///
/// 三道闸与插件包同源：**只 https**、**体积上限**、**落点必须留在插件目录内**（`safe_join`）；
/// 另外多一道 **sha256**（清单写了就必须对上）。先写 `.part` 再改名 —— 半截文件不能被当成装好的。
fn install_file_dependency(plugin_dir: &Path, d: &PluginDependency) -> Result<(), String> {
    let dest = safe_join(plugin_dir, &d.dest)
        .ok_or_else(|| format!("dest 越界，已拒绝：{}", d.dest))?;
    let bytes = download_bytes(&d.url, MAX_DEP_BYTES)?;
    let want = d.sha256.trim();
    if want.is_empty() {
        crate::log::warn(format!(
            "依赖 {} 没有 sha256，已按「只校验 https」安装 —— 作者最好补上校验和",
            d.url
        ));
    } else {
        let got = hex_sha256(&bytes);
        if !got.eq_ignore_ascii_case(want) {
            return Err(format!("sha256 不匹配（期望 {want}，实得 {got}）"));
        }
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("建依赖目录失败：{e}"))?;
    }
    let mut tmp = dest.clone().into_os_string();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, &bytes).map_err(|e| format!("写依赖文件失败：{e}"))?;
    fs::rename(&tmp, &dest).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("落盘依赖失败：{e}")
    })?;
    Ok(())
}

/// `npm install --prefix <插件目录> <包名>@<版本>`（`npm` 形态）。
///
/// **依赖用户机器上有 Node.js** —— 这是用户明确选择的档位（「再加 npm 生态」）。
/// 找不到 npm 时**如实报错**（不静默跳过：那样插件会在运行到一半时才炸）。
fn install_npm_dependency(plugin_dir: &Path, d: &PluginDependency) -> Result<(), String> {
    let pkg = if d.package.trim().is_empty() {
        d.url.trim()
    } else {
        d.package.trim()
    };
    let spec = if d.version.trim().is_empty() {
        pkg.to_string()
    } else {
        format!("{pkg}@{}", d.version.trim())
    };
    // npm 在「没有 package.json 的目录」里装包会先试图往上找项目根，可能装错地方。
    // 先补一份最小 package.json，把落点钉死在这个插件目录里。
    let pkg_json = plugin_dir.join("package.json");
    if !pkg_json.is_file() {
        let name = plugin_dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "plugin".into());
        let text = format!(
            "{{\n  \"name\": \"lunac-plugin-{name}\",\n  \"private\": true,\n  \"version\": \"0.0.0\"\n}}\n"
        );
        fs::write(&pkg_json, text).map_err(|e| format!("写 package.json 失败：{e}"))?;
    }
    // Windows 下 `npm` 实际是 `npm.cmd`，而 `Command` 走 CreateProcess、**不会**按 PATHEXT 补后缀
    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let mut cmd = Command::new(npm);
    cmd.arg("install")
        .arg("--no-audit")
        .arg("--no-fund")
        .arg("--prefix")
        .arg(plugin_dir)
        .arg(&spec)
        .current_dir(plugin_dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "未找到 npm —— npm 依赖需要先安装 Node.js（nodejs.org）".to_string()
        } else {
            format!("启动 npm 失败：{e}")
        }
    })?;
    if !out.status.success() {
        let tail: String = String::from_utf8_lossy(&out.stderr)
            .lines()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!("npm 退出码 {}：{tail}", out.status.code().unwrap_or(-1)));
    }
    Ok(())
}

/// 从 https 拉一段字节（有体积上限）。仅给依赖下载用 —— 插件包本体那条路在 commands.rs 里，
/// 因为它还要兼顾 Content-Length 与错误上报的口径。
fn download_bytes(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err("只允许 https 的依赖地址".into());
    }
    let over = || format!("依赖文件超过上限（{} MB）", limit / 1024 / 1024);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("Client error: {e}"))?;
    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| format!("下载失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}：下载失败", resp.status().as_u16()));
    }
    if let Some(len) = resp.content_length() {
        if len > limit {
            return Err(over());
        }
    }
    let mut bytes: Vec<u8> = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = resp.read(&mut buf).map_err(|e| format!("读取出错：{e}"))?;
        if n == 0 {
            break;
        }
        if bytes.len() as u64 + n as u64 > limit {
            return Err(over());
        }
        bytes.extend_from_slice(&buf[..n]);
    }
    Ok(bytes)
}

/// sha256 的十六进制小写表示（用于依赖校验和比对）。
fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 扫插件目录，列出全部已安装插件（含「坏包」——它们也要在面板上可见，否则用户只会看到插件莫名其妙消失）。
pub fn list_installed(plugins_root: &Path) -> Vec<InstalledPlugin> {
    let mut out: Vec<InstalledPlugin> = Vec::new();
    let Ok(entries) = fs::read_dir(plugins_root) else {
        return out;
    };
    for e in entries.flatten() {
        let dir = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if !dir.is_dir() || name.starts_with('.') {
            continue;
        }
        let mut item = InstalledPlugin {
            id: name.clone(),
            name: name.clone(),
            description: String::new(),
            keywords: Vec::new(),
            icon: String::new(),
            version: String::new(),
            entry: String::new(),
            homepage: String::new(),
            dir: dir.to_string_lossy().to_string(),
            entry_path: String::new(),
            valid: false,
            error: String::new(),
            dependencies: Vec::new(),
            permissions: Vec::new(),
        };
        match fs::read_to_string(dir.join(MANIFEST_FILE)) {
            Ok(text) => match parse_manifest(&text) {
                Ok(m) => {
                    item.id = m.id;
                    item.name = if m.name.trim().is_empty() {
                        name.clone()
                    } else {
                        m.name.clone()
                    };
                    item.description = m.description.clone();
                    item.keywords = m.keywords.clone();
                    item.icon = m.icon.clone();
                    item.version = m.version.clone();
                    item.entry = m.entry.clone();
                    item.homepage = m.homepage.clone();
                    item.dependencies = m.dependencies.clone();
                    item.permissions = m.permissions.clone();
                    match safe_join(&dir, &m.entry) {
                        Some(p) if p.is_file() => {
                            item.entry_path = p.to_string_lossy().to_string();
                            item.valid = true;
                        }
                        _ => item.error = format!("入口文件不存在：{}", m.entry),
                    }
                }
                Err(err) => item.error = err,
            },
            Err(_) => item.error = format!("缺少 {MANIFEST_FILE}"),
        }
        out.push(item);
    }
    // 顺序稳定（目录名升序）：面板每次重绘的顺序不该随 read_dir 抖动
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// 卸载：**只删 `<plugins_root>\<id>`**，且 id 必须安全、目录必须真的在插件根下（防穿越）。
pub fn uninstall(id: &str, plugins_root: &Path) -> Result<(), String> {
    if !is_safe_id(id) {
        return Err(format!("插件 id 非法：{id}"));
    }
    let dir = plugins_root.join(id);
    if !dir.is_dir() {
        return Err("插件不存在".into());
    }
    // 双保险：canonicalize 之后确认它确实落在插件根内（符号链接 / 手工造的怪目录都拦下）
    let real_root = plugins_root
        .canonicalize()
        .map_err(|e| format!("插件目录不可用：{e}"))?;
    let real_dir = dir.canonicalize().map_err(|e| format!("插件目录不可用：{e}"))?;
    if !real_dir.starts_with(&real_root) {
        return Err("插件目录不在插件根内，已拒绝删除".into());
    }
    fs::remove_dir_all(&real_dir).map_err(|e| format!("删除失败：{e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn tmp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "lunac-test-plugins-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// 造一个 zip：`files` 是 (包内路径, 内容)
    fn zip_bytes(files: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn good_manifest(id: &str) -> String {
        format!(
            r#"{{"id":"{id}","name":"Demo","description":"d","keywords":["demo"],"icon":"x","version":"1.0.0","entry":"index.js"}}"#
        )
    }

    #[test]
    fn safe_join_rejects_traversal_and_absolute_paths() {
        let root = Path::new("C:\\plugins");
        assert!(safe_join(root, "index.js").is_some());
        assert!(safe_join(root, "sub/dir/a.js").is_some());
        // 越界的四种写法
        assert!(safe_join(root, "../evil.js").is_none());
        assert!(safe_join(root, "a/../../evil.js").is_none());
        assert!(safe_join(root, "/abs.js").is_none());
        assert!(safe_join(root, "C:/abs.js").is_none());
        // Windows 会悄悄改掉以点/空格结尾的分段 —— 校验过的路径与写下去的路径就不是同一个了
        assert!(safe_join(root, "a./b.js").is_none());
        assert!(safe_join(root, "a /b.js").is_none());
        // 反斜杠按分隔符看（手工打的包常混用）
        assert!(safe_join(root, "..\\evil.js").is_none());
    }

    #[test]
    fn manifest_requires_safe_id_and_js_entry() {
        assert!(parse_manifest(&good_manifest("demo-pet")).is_ok());
        // BOM 容错
        assert!(parse_manifest(&format!("\u{feff}{}", good_manifest("demo"))).is_ok());
        // id：大写 / 穿越 / 点开头 / 空
        assert!(parse_manifest(&good_manifest("Demo")).is_err());
        assert!(parse_manifest(&good_manifest("../evil")).is_err());
        assert!(parse_manifest(&good_manifest(".hidden")).is_err());
        assert!(parse_manifest(&good_manifest("")).is_err());
        // entry：绝对路径 / 穿越 / 不是 js
        for bad in ["/index.js", "../index.js", "index.ts", "index"] {
            let text = format!(
                r#"{{"id":"demo","entry":"{bad}"}}"#
            );
            assert!(parse_manifest(&text).is_err(), "entry={bad} 应被拒");
        }
        // 缺 entry ⇒ 用默认 index.js
        let m = parse_manifest(r#"{"id":"demo"}"#).unwrap();
        assert_eq!(m.entry, "index.js");
    }

    #[test]
    fn extract_zip_refuses_entries_outside_the_root() {
        let root = tmp_root("traversal");
        let bytes = zip_bytes(&[
            ("lunac-plugin.json", &good_manifest("demo")),
            ("../evil.js", "alert(1)"),
        ]);
        let err = extract_zip(std::io::Cursor::new(&bytes), &root, MAX_TOTAL_BYTES).unwrap_err();
        assert!(err.contains("越界路径"), "{err}");
        // 一个字节都不该落到 root 之外（root 的父目录里不能出现 evil.js）
        assert!(!root.parent().unwrap().join("evil.js").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_zip_stops_when_unpacked_size_exceeds_the_cap() {
        let root = tmp_root("bomb");
        let big = "x".repeat(200 * 1024);
        let bytes = zip_bytes(&[("lunac-plugin.json", &good_manifest("demo")), ("index.js", &big)]);
        // 上限设成 1KB ⇒ 第二个文件写到一半就该被拦下
        let err = extract_zip(std::io::Cursor::new(&bytes), &root, 1024).unwrap_err();
        assert!(err.contains("超过上限"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn install_round_trips_upgrades_and_rejects_broken_entry() {
        let root = tmp_root("install");
        let bytes = zip_bytes(&[
            ("lunac-plugin.json", &good_manifest("demo-pet")),
            ("index.js", "export default { id: 'demo-pet' };"),
        ]);
        let id = install_from_bytes(&bytes, &root).unwrap();
        assert_eq!(id, "demo-pet");
        let list = list_installed(&root);
        assert_eq!(list.len(), 1);
        assert!(list[0].valid, "{:?}", list[0].error);
        assert!(list[0].entry_path.ends_with("index.js"));
        assert!(!list[0].entry_path.contains(".staging"));
        // 同一 id 再装一次 ⇒ **视为升级**：成功覆盖，且不留 .old / .staging 残渣
        let bytes2 = zip_bytes(&[
            ("lunac-plugin.json", &good_manifest("demo-pet")),
            ("index.js", "export default { id: 'demo-pet', v: 2 };"),
        ]);
        assert_eq!(install_from_bytes(&bytes2, &root).unwrap(), "demo-pet");
        assert!(fs::read_to_string(root.join("demo-pet").join("index.js"))
            .unwrap()
            .contains("v: 2"));
        // 清单写的入口不存在 ⇒ 拒绝，且**旧版本原样保留**、不留任何残渣
        let bad = zip_bytes(&[("lunac-plugin.json", &good_manifest("demo-pet"))]);
        let err = install_from_bytes(&bad, &root).unwrap_err();
        assert!(err.contains("入口不存在"), "{err}");
        assert!(root.join("demo-pet").join("index.js").is_file());
        let leftovers: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "留下残渣：{leftovers:?}");
        // 卸载
        uninstall("demo-pet", &root).unwrap();
        assert!(list_installed(&root).is_empty());
        // 非法 id 不许删
        assert!(uninstall("../x", &root).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn install_accepts_a_single_top_level_folder() {
        let root = tmp_root("github-shape");
        let bytes = zip_bytes(&[
            ("LunacPet/lunac-plugin.json", &good_manifest("lunac-pet")),
            ("LunacPet/index.js", "export default {};"),
        ]);
        assert_eq!(install_from_bytes(&bytes, &root).unwrap(), "lunac-pet");
        let list = list_installed(&root);
        assert!(list[0].valid, "{:?}", list[0].error);
        assert!(root.join("lunac-pet").join("index.js").is_file());
        assert!(!root.join("lunac-pet").join("LunacPet").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn list_reports_a_broken_package_instead_of_hiding_it() {
        let root = tmp_root("broken");
        fs::create_dir_all(root.join("half-installed")).unwrap();
        fs::write(root.join("half-installed").join(MANIFEST_FILE), "{ not json").unwrap();
        // 内部目录（以 . 开头）不进清单
        fs::create_dir_all(root.join(".staging-1")).unwrap();
        let list = list_installed(&root);
        assert_eq!(list.len(), 1);
        assert!(!list[0].valid);
        assert!(list[0].error.contains("JSON"), "{:?}", list[0].error);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn index_keeps_good_entries_and_drops_bad_ones() {
        let text = r#"[
            {"id":"lunac-pet","name":"Lunac Pet","description":"desk pet","version":"1.0.0",
             "url":"https://github.com/LythrumMoon/Lunac/releases/download/v1/lunac-pet.zip"},
            {"id":"Bad_Id","name":"Bad","url":"https://example.com/b.zip"},
            {"id":"plain-http","name":"Plain","url":"http://example.com/p.zip"},
            {"id":"nameless","name":"  ","url":"https://example.com/n.zip"},
            {"id":"lunac-pet","name":"Duplicated","url":"https://example.com/dup.zip"}
        ]"#;
        let out = parse_index(text).unwrap();
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].id, "lunac-pet");
        assert_eq!(out[0].version, "1.0.0");
        assert_eq!(out[0].name, "Lunac Pet");
    }

    #[test]
    fn index_rejects_non_json_and_accepts_bom_and_empty() {
        assert!(parse_index("{ not json").is_err());
        assert!(parse_index("\u{feff}[]").unwrap().is_empty());
        // 缺省字段（只有 id + url）也要能用 —— 名字退回 id（前端行为），这里只要求不报错
        let out = parse_index(r#"[{"id":"solo","name":"Solo","url":"https://e.com/a.zip"}]"#).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].description.is_empty() && out[0].version.is_empty());
    }

    /// 依赖字段（2026-09-28）：好条目过、坏条目整包拒。
    #[test]
    fn manifest_validates_dependencies() {
        let ok = r#"{"id":"music","entry":"index.js","dependencies":[
            {"type":"file","url":"https://e.com/librespot.exe","dest":"bin/librespot.exe","sha256":"AB12"},
            {"type":"npm","package":"@scope/pkg","version":"^2.0.0"},
            {"url":"https://e.com/model.bin","dest":"model.bin"}
        ]}"#;
        let m = parse_manifest(ok).unwrap();
        assert_eq!(m.dependencies.len(), 3);
        assert!(!m.dependencies[0].is_npm());
        assert!(m.dependencies[1].is_npm());

        // 逐条坏：明文 http / dest 越界 / 缺 dest / npm 缺包名 / 未知 type
        let bad = [
            r#"[{"url":"http://e.com/a.exe","dest":"a.exe"}]"#,
            r#"[{"url":"https://e.com/a.exe","dest":"../a.exe"}]"#,
            r#"[{"url":"https://e.com/a.exe"}]"#,
            r#"[{"type":"npm"}]"#,
            r#"[{"type":"npm","package":"--registry=http://evil"}]"#,
            r#"[{"type":"zip","url":"https://e.com/a.zip"}]"#,
        ];
        for b in bad {
            let text = format!(r#"{{"id":"demo","entry":"index.js","dependencies":{b}}}"#);
            assert!(parse_manifest(&text).is_err(), "应被拒：{b}");
        }
    }

    /// 声明的宿主能力（2026-09-28）：合法的过、格式非法的整包拒、**不认识的只 warn 不拒**。
    #[test]
    fn manifest_validates_permissions() {
        let ok = r#"{"id":"ocr","entry":"index.js","permissions":["layout.takeover","window.resize"]}"#;
        let m = parse_manifest(ok).unwrap();
        assert_eq!(m.permissions.len(), 2);
        // 不认识的名字（更新版宿主的能力）**不影响安装**
        let fwd = r#"{"id":"demo","entry":"index.js","permissions":["some.future.cap"]}"#;
        assert!(parse_manifest(fwd).is_ok());
        // 格式非法 / 写不下：整包拒
        for bad in [r#"[""]"#, r#"["UPPER"]"#, r#"["has space"]"#, r#"["bad/slash"]"#] {
            let text = format!(r#"{{"id":"demo","entry":"index.js","permissions":{bad}}}"#);
            assert!(parse_manifest(&text).is_err(), "应被拒：{bad}");
        }
        let many = (0..(MAX_PERMISSIONS + 1)).map(|i| format!("\"cap{i}\"")).collect::<Vec<_>>().join(",");
        let text = format!(r#"{{"id":"demo","entry":"index.js","permissions":[{many}]}}"#);
        assert!(parse_manifest(&text).is_err());
    }

    /// 窗口形态（2026-09-29，为桌宠 L1）：合法写法照收，**静态能判定的矛盾整包拒**。
    #[test]
    fn manifest_validates_window_shape() {
        // 不写这一段完全合法（= 宿主缺省形态）
        let none = r#"{"id":"demo","entry":"index.js"}"#;
        assert!(parse_manifest(none).unwrap().window.is_none());

        // 桌宠那种写法：定尺 + 禁缩放 + 不进任务栏 + 不要标题栏
        let pet = r#"{"id":"pet","entry":"index.js","window":{"width":260,"height":380,
            "minWidth":120,"minHeight":160,"resizable":false,"skipTaskbar":true,"chrome":false}}"#;
        let s = parse_manifest(pet).unwrap().window.unwrap();
        assert_eq!(s.width, 260.0);
        assert!(!s.resizable);
        assert!(s.skip_taskbar);
        assert!(!s.chrome);
        // 缺省：`alwaysOnTop` 没写 ⇒ true（与建窗历史行为一致）
        assert!(s.always_on_top);

        // `0` = 用宿主缺省那一档，合法（不是错误写法）
        let zero = r#"{"id":"demo","entry":"index.js","window":{"width":0,"height":0}}"#;
        assert!(parse_manifest(zero).is_ok());

        // 静态能判定的坏值：整包拒（否则建出一个用户抓不回来的窗口）
        for bad in [
            r#"{"width":-5}"#,
            r#"{"width":0.5}"#,
            r#"{"height":99999}"#,
            r#"{"width":300,"minWidth":400}"#,
            r#"{"height":300,"minHeight":400}"#,
        ] {
            let text = format!(r#"{{"id":"demo","entry":"index.js","window":{bad}}}"#);
            assert!(parse_manifest(&text).is_err(), "应被拒：{bad}");
        }
    }

    /// 旧 `plugins\` 目录一次性搬到 `Modules\`（2026-09-28 改名）。
    #[test]
    fn legacy_plugins_dir_is_migrated_into_modules() {
        let root = tmp_root("migrate");
        let old = root.join("plugins");
        fs::create_dir_all(old.join("memo")).unwrap();
        fs::write(old.join("memo").join(MANIFEST_FILE), good_manifest("memo")).unwrap();
        fs::write(old.join("memo").join("index.js"), "export default {};").unwrap();
        // 新旧同名冲突时保留新目录里那份
        fs::create_dir_all(root.join("Modules").join("other")).unwrap();
        fs::create_dir_all(old.join("other")).unwrap();

        assert_eq!(migrate_legacy_plugins_dir(&root), 1);
        assert!(root.join("Modules").join("memo").join("index.js").is_file());
        // 有冲突条目 ⇒ 旧目录保留（不删用户文件）
        assert!(root.join("plugins").is_dir());
        // 再跑一次是幂等的（不会把已搬走的又搬一遍）
        assert_eq!(migrate_legacy_plugins_dir(&root), 0);
        let _ = fs::remove_dir_all(&root);
    }

    /// 依赖落点越界必须在**写盘之前**就被拦下（`safe_join` 是唯一判据）。
    #[test]
    fn dependency_dest_cannot_escape_the_plugin_dir() {
        let root = tmp_root("dep-dest");
        let plugin = root.join("demo");
        fs::create_dir_all(&plugin).unwrap();
        // 故意用一个不存在的 dest 去触发校验分支：越界在 safe_join 那一步就返回 Err，
        // 因此这里不会真的联网 —— 网络错误与越界错误都能得到 Err，断言只看「返回 Err」
        let dep = PluginDependency {
            kind: "file".into(),
            url: "https://127.0.0.1:1/should-not-be-fetched".into(),
            dest: "../escape.exe".into(),
            ..Default::default()
        };
        let err = install_dependencies(&plugin, &[dep]).unwrap_err();
        assert!(err.contains("越界"), "{err}");
        assert!(!root.join("escape.exe").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
