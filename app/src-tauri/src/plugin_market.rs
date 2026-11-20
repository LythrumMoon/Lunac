// src-tauri/src/plugin_market.rs
// 插件市场（L1，2026-09-21）：从 https 的 zip 安装第三方插件到固定目录。
//
// 目录约定：**<exe 根>\Modules\<id>\**（2026-09-28 从 `plugins\` 改名而来；与 `skills\` / `tools\` 同级，
// 都在安装根下，卸载随目录清掉。旧 `plugins\` 由 `migrate_legacy_plugins_dir()` 一次性搬过来）。
// 包里必须带一份清单 `lunac-plugin.json`（见 `PluginManifest`），入口是**已编译好的 ESM**（`import()`），
// 因为前端 CSP 的 `script-src` 只放行 `'self'` 与 asset 协议（**http / https 两种写法都要写上**：
// Windows 上 asset 协议的 origin 是 `http://asset.localhost`，见 ai-spec §3.5「加载通道」）—— 插件代码只能经
// **asset 协议**从磁盘加载，**不能走 CDN**（同 ai-spec §3.7 的 KaTeX 缺陷是同一条约束）。
//
// **依赖随插件一起装**（2026-09-28 加）：清单里的 `dependencies[]` 由宿主在插件落盘后逐条拉取
// （见 `install_dependencies`）—— 这是「release 里没有 librespot」这类问题的根治办法：
// 依赖不再由构建脚本塞进安装包，而是**跟着用到它的那个插件**走。依赖只收 https、
// 可选 sha256 校验、落点必须留在插件目录内。
//
// **`archive` 依赖**（2026-09-30 加，为 PaddleOCR-json）：有些依赖不是一个文件而是一整棵目录
// （引擎 exe + 上百 MB 模型），上游只发 `.7z`。这条类型把压缩包下下来**解到插件目录内**的
// `dest` 目录里（`Modules\ocr\paddle-ocr\`），于是「装 OCR 插件 = 引擎一起就位」，
// 而不是让**所有**用户替少数人的功能在安装包里多背 70MB。解压走的是与插件包同一套判据
// （路径穿越 / 解压后总量 / 条目数），7z 那条**必须**用 `decompress_with_extract_fn` 自己
// 校验路径 —— sevenz-rust 的默认解压是 `dest.join(entry.name())`，对 `..\..\x` 不做任何拦截。
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

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

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
/// 单个依赖文件的体积上限（2026-09-28；2026-10-09 由 512 MB 提到 4 GiB）。
/// 依赖里的引擎 / 模型比插件包本身大得多（PaddleOCR 模型、librespot 二进制都在几十 MB 量级），
/// **2026-10-09 本地决策模型换成 3B 级后单体到 2.5 GiB** ⇒ 512 MB 的旧闸会直接把它拒掉。
/// 4 GiB 覆盖 3B–4B 的 Q4_K_M（约 2–3 GiB）并留余量；仍能拦住「几十 GB 的怪物」。
/// ⚠️ 这个闸只对 `file` 形态生效；大文件走的是 **流式落盘**（见 `fetch_resumable`），
/// 不再把整包读进内存 —— 否则 2.5 GiB 的分配会直接把安装过程顶爆。
pub const MAX_DEP_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// 依赖条目数上限（防清单里写一万条把安装拖死）。
pub const MAX_DEP_ENTRIES: usize = 32;
/// `archive` 依赖**解压后**的总量上限（2026-09-30）。比插件包那档（192MB）宽：
/// 这条走的是「引擎 + 模型」——PaddleOCR-json 的 `.7z` 约 88MB、解开约 300MB，
/// 按体积卡在 192MB 会让它永远装不上。1 GiB 仍能拦住「解出一个几十 GB 的树」。
pub const MAX_DEP_EXTRACT_BYTES: u64 = 1024 * 1024 * 1024;
/// `archive` 依赖的条目数上限。模型目录动辄上千个小文件，比插件包那档（4000）放宽。
pub const MAX_DEP_EXTRACT_ENTRIES: usize = 20000;
/// 声明的宿主能力条数上限（权限名很短，16 条已经远超任何真实插件）。
pub const MAX_PERMISSIONS: usize = 16;
/// `reuse`（复用哪个**基础插件**的挂载监听）条数上限（2026-10-05）。几个就够，4 是宽松上限。
pub const MAX_REUSE: usize = 4;
/// 宿主当前**认识**的能力（2026-09-28）。**只加不减**（旧插件会带着老名字装在盘上），
/// 清单里出现不认识的名字**只 warn、不拒绝安装** —— 那可能是给更新版宿主声明的能力，
/// 在旧宿主上装不上反而是坏事（它只是拿不到那个能力，与「装不上」是两回事）。
pub const KNOWN_PERMISSIONS: &[&str] = &[
    "layout.takeover",
    "process.spawn",
    "window.resize",
    "window.float",
    // 2026-10-18：宿主代请求（见 plugin_net.rs）。插件不得直连网络。
    "net.fetch",
    // 2026-10-18：图片 / 视频经宿主本地代理（见 plugin_media.rs）。
    "media.proxy",
    // 2026-10-18：插件脚本沙箱（见 plugin_engine.rs）。
    "engine.eval",
    // 2026-10-18：插件 KV 存储（见 plugin_kv.rs）。
    "storage.plugin",
    // 2026-10-21：隐藏 WebView 嗅探番剧 m3u8 / mp4（见 plugin_sniff.rs）。
    "webview.sniff",
];
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
    /// 建窗即置顶。`None` = **未声明** ⇒ 由宿主 `plugin_window::default_always_on_top()`
    /// 按窗口 id 决定（只有桌宠**模型窗**默认置顶）；写了 `true` / `false` 则**显式覆盖**默认。
    #[serde(default)]
    pub always_on_top: Option<bool>,
    /// 带不带宿主那根标题栏（置顶 / 最小化 / 关闭三个按钮）。缺省 `true`。
    /// 桌宠这一类「窗口就是那片画面本身」的插件写 `false`，前端据此收起标题栏。
    #[serde(default = "default_true")]
    pub chrome: bool,
}

fn default_true() -> bool {
    true
}

/// 清单里的 `render` 段（2026-10-18，插件框架重构 E.3）。
///
/// `engine` 决定挂载方式：`dom` 是现状的 light-DOM；`iframe` 是 opt-in 的作用域边界。
/// `appearance` 决定是否继承宿主 token：`inherit` 遵守 `ui-style.md`，`custom` 才允许自绘。
/// `slots` 声明**常驻挂载锚点**（2026-10-18）：目前只有 `chat-sidebar` / `settings-section`；
/// 只对 iframe 引擎开放（DOM 插件没有独立文档，宿主无法把它的样式限制在槽位内）。
/// 三个字段都有缺省值 ⇒ 老插件零变化。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "lowercase")]
pub struct PluginRenderSpec {
    #[serde(default)]
    pub engine: RenderEngine,
    #[serde(default)]
    pub appearance: RenderAppearance,
    #[serde(default)]
    pub slots: Vec<RenderSlot>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RenderEngine {
    #[default]
    Dom,
    Iframe,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RenderAppearance {
    #[default]
    Inherit,
    Custom,
}

/// 常驻挂载锚点（2026-10-17 用户拍板新增两个；2026-10-18 施工）。
///
/// **是枚举不是自由字符串**：每加一个锚点都会扩大宿主 DOM 契约，必须重新拍板
/// （见 `docs/todo.md` C.4.1）。
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RenderSlot {
    ChatSidebar,
    SettingsSection,
}

/// `provides` 条数上限（能力身份，几个就够）。
pub const MAX_PROVIDES: usize = 8;

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
    /// **复用哪个内置（基础）插件的挂载监听**（2026-10-05 用户定：自定义插件按钮失效，
    /// 允许「复用内置插件监听」）。
    ///
    /// 磁盘插件自己的 `attach(root)` 之外，再按这里的 id 追加挂上基础插件那套监听 —— 这样
    /// AI 生成的插件若照搬了某个基础插件的面板 HTML，按钮就能真的工作，而不是一整个哑掉。
    /// **只认宿主里编译进去的基础插件**（settings / quick-launch / memo / translate /
    /// tool-editor），认不出的 id 一律忽略并 warn（与 `permissions` 同一条「只 warn」纪律）。
    /// 注意：这**不违反**「拓展插件不在 bundle 里」—— 复用是**声明在插件自己的清单里**的，
    /// 宿主代码里没有一行按某个磁盘插件的 id 分支；卸载它照样什么也不留。
    #[serde(default)]
    pub reuse: Vec<String>,
    /// 能力身份（2026-10-18，插件框架重构 E.1）：供宿主按能力定位插件。
    /// **不改变「id = 目录名」硬契约**；多个插件声明同一 `provides` 只 warn 并取 id 最小者。
    #[serde(default)]
    pub provides: Vec<String>,
    /// 渲染方式与外观级别（2026-10-18，E.3）。缺省 = `dom` + `inherit`。
    #[serde(default)]
    pub render: PluginRenderSpec,
    /// 悬浮窗形态（2026-09-29，见 `PluginWindowShape`）。不写这一段 = 宿主缺省形态。
    ///
    /// 由**宿主**在建窗时读（`plugin_window::declared_shape()`），不进前端契约 ——
    /// 前端不需要知道窗口多大，它只管往 `#results-list` 里画东西。
    #[serde(default)]
    pub window: Option<PluginWindowShape>,
    /// 插件自带的本机进程（2026-10-06，L12 档 1，见 `SidecarSpec`）。不写 = 无 sidecar。
    ///
    /// **必须与 `permissions: ["process.spawn"]` 同时出现**；起进程前还要过**独立信任门**
    /// （`config\plugin-trusted.json`）。这一段由宿主在 `plugin_sidecar` 模块里读并执行。
    #[serde(default)]
    pub sidecar: Option<SidecarSpec>,
    /// 开发工具链（轨道 H，2026-10-18，见 `PluginDevSpec`）。不写 = 无开发环境。
    ///
    /// **永不随安装自动执行**：装工具链 / 跑构建都由用户在设置页显式授权
    /// （`config\plugin-toolchain.json` 是按指纹的授权账本）。
    #[serde(default)]
    pub dev: Option<PluginDevSpec>,
}

fn default_entry() -> String {
    "index.js".to_string()
}

/// 一条依赖（清单里的 `dependencies[]` 元素，2026-09-28）。
///
/// 三种形态（`type` 决定，缺省 `file`）：
///   · **`file`**（默认）：从 `url`（**必须 https**）下载单个文件到插件目录内的 `dest`。
///     适合「单文件引擎/二进制」这类（librespot.exe）。
///     可选 `sha256`：写了就**必须**对上，否则整条拒绝（供应链完整性）。
///   · **`archive`**（2026-09-30）：从 `url`（**必须 https**）下载压缩包（`.7z` / `.zip`，
///     按魔数自动识别），解压到插件目录内的 `dest` **目录**。适合「引擎 + 模型」这种
///     一整棵树的依赖（PaddleOCR-json）。`sha256` 校验的是**压缩包本身**。
///   · **`npm`**：`npm install --prefix <插件目录> <package>@<version>`，装进插件目录的
///     `node_modules\`。要求用户机器上有 node/npm —— 找不到就**如实报错**，不静默跳过。
///
/// **为什么不在构建期把依赖塞进安装包**：依赖只对「用到它的那个插件」有意义，
/// 塞进主安装包会让所有用户替少数人的功能买单，而且 release 一旦漏拷（librespot 就是这个）
/// 就只能等下一个版本。改成跟着插件走，装/卸/升级都在一起。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PluginDependency {
    /// `file`（缺省）/ `archive` / `npm`
    #[serde(default, rename = "type")]
    pub kind: String,
    /// `file` / `archive`：https 下载地址
    #[serde(default)]
    pub url: String,
    /// `npm`：包名（可带 scope，如 `@scope/pkg`）。缺省退回 `url` 字段
    #[serde(default)]
    pub package: String,
    /// `npm`：版本范围（可选，空 = latest）
    #[serde(default)]
    pub version: String,
    /// `file`：相对插件目录的落点（如 `bin\librespot.exe`）。
    /// `archive`：相对插件目录的**解压目标目录**（如 `paddle-ocr`）。不许越界（走 `safe_join`）
    #[serde(default)]
    pub dest: String,
    /// `file`：期望的 sha256（十六进制，大小写不敏感）。空 = 不校验（安装时会 warn 留痕）
    /// `archive`：期望的**压缩包** sha256（同上）
    #[serde(default)]
    pub sha256: String,
}

impl PluginDependency {
    fn is_npm(&self) -> bool {
        self.kind.eq_ignore_ascii_case("npm")
    }

    fn is_archive(&self) -> bool {
        self.kind.eq_ignore_ascii_case("archive")
    }
}

/// **sidecar**：插件自带的本机进程声明（2026-10-06 定稿，L12 档 1）。
///
/// 形态 = 「插件目录里带一个二进制，宿主负责起进程 + 通信 + 收尾」。UI 仍在 WebView 里
/// （不替代 WebView），插件背后多一台「引擎进程」—— 本仓内置插件（librespot / ffmpeg /
/// PaddleOCR / mihomo）早就在这么做，这一段把那套能力开放给磁盘插件，不必为此改主程序。
///
/// **必须与 `permissions: ["process.spawn"]` 同时出现**（见 `validate_sidecar`）：
/// 能 spawn 任意二进制的插件能力无上界，不能靠「声明即生效」，要过**独立信任门**
/// （`config\plugin-trusted.json`，指纹 = 插件 id + command + sha256）。
///
/// 通信：`stdio`（宿主 ↔ 进程走 stdin/stdout 的 NDJSON）/ `http`（进程监听 `127.0.0.1:port`，
/// **由宿主代请求** —— 前端 CSP 的 `default-src` 不含 `127.0.0.1`，插件 JS 不能直接 fetch）/
/// `both`。宿主与进程的握手行是 `{"method":"ready","params":{"port":<N>}}`（http 时带端口）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SidecarSpec {
    /// 可执行文件路径（**相对插件目录**，不许绝对路径 / 不许含 `..`）。可与
    /// `dependencies[{type:"file"}]` 配合（先下载、校验后再起）。
    pub command: String,
    /// 直接传给 `CreateProcess` 的参数数组（**不经 shell** —— 避免注入）。
    #[serde(default)]
    pub args: Vec<String>,
    /// 通道：`""` / `stdio`（缺省）/ `http` / `both`
    #[serde(default)]
    pub transport: String,
    /// 仅 `http` / `both`：`0`（缺省）= 由进程自选并经 ready 行上报；非 0 = 写死该端口。
    #[serde(default)]
    pub port: u16,
    /// 消息协议：`""` / `ndjson`（缺省，唯一支持）
    #[serde(default)]
    pub protocol: String,
    /// 工作目录（相对插件目录，**不得越界**）。缺省 = 插件目录。
    #[serde(default)]
    pub cwd: String,
    /// 附加环境变量（在宿主环境之上**只增不删**）。
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// 宿主加载插件时是否自动起（缺省 false = 插件调 API 时才起）。
    #[serde(default)]
    pub autostart: bool,
    /// 崩溃重启策略：`""` / `never`（缺省）/ `on-failure`（最多 3 次、指数退避）。
    #[serde(default)]
    pub restart: String,
    /// 可执行文件校验和（64 位十六进制，可选；写了就必须对上）。
    #[serde(default)]
    pub sha256: String,
    /// **授权串其他本机应用**的额外端口白名单（缺省空 = 只放行自己上报的那个端口）。
    #[serde(default)]
    pub allow_local_ports: Vec<u16>,
    /// 授权访问**任意** loopback 端口（信任卡上会显著标注）。缺省 false。
    #[serde(default)]
    pub allow_local_any: bool,
    /// **就绪超时（秒，缺省 10）**：宿主起进程后等「就绪信号」的上限。
    /// `http` 通道下有两个就绪信号，**任一**成立即可：① 进程吐出 NDJSON `{"method":"ready"}`
    /// （自研 sidecar 的握手约定）；② **清单写的固定 `port` 可连** —— 第三方 http 服务
    /// （`llama-server` 等 OpenAI 兼容服务）**不会说我们的握手行**，只在模型加载完后开始监听。
    /// 大模型冷启要几十秒 ⇒ 清单里写大些（如 180），否则会被误判成失败并杀掉进程。
    #[serde(default = "default_ready_timeout_sec")]
    pub ready_timeout_sec: u64,
    /// **单进程内存上限（MiB，0 = 不限制）**：E.6（2026-10-18）为每个 sidecar 建独立
    /// Job Object 时使用。只约束该插件自己的进程树，不影响宿主 / agent 的全局 job。
    #[serde(default, rename = "memoryLimitMiB")]
    pub memory_limit_mib: u32,
    /// **job 内最大进程数（0 = 不限制）**：同上，防止 sidecar 无限 fork。
    #[serde(default)]
    pub process_limit: u32,
}

/// `sidecar.readyTimeoutSec` 的缺省值（秒）—— 与旧行为一致。
fn default_ready_timeout_sec() -> u64 {
    10
}

/// `sidecar.readyTimeoutSec` 上限（1 小时；够任何本地服务冷启动）。
const MAX_SIDECAR_READY_TIMEOUT_SEC: u64 = 3600;

/// sidecar.env 条数上限。
const MAX_SIDECAR_ENV: usize = 32;
/// sidecar.allowLocalPorts 条数上限。
const MAX_SIDECAR_ALLOW_PORTS: usize = 16;

/// sidecar 内存上限的合法区间（MiB）：0 = 不限制；非 0 至少 32 MiB
/// （再小会把最简单的运行时也杀掉），最大 32 GiB。
const MIN_SIDECAR_MEMORY_MIB: u32 = 32;
const MAX_SIDECAR_MEMORY_MIB: u32 = 32768;
/// sidecar job 内进程数上限（0 = 不限制；非 0 最多 64 —— 正常插件不会开这么多）。
const MAX_SIDECAR_PROCESS_LIMIT: u32 = 64;

// ── 开发工具链（轨道 H，2026-10-18）───────────────────────────────────────
//
// 这一段**永不随安装自动执行**：工具链要用户逐次授权（按指纹），构建命令要用户点「运行」。
// 落点固定在 `<插件目录>\.toolchain\<profile>\` ⇒ 卸载删目录即回收，守住「卸载 = 完全不存在」。
// **不是沙箱**：装工具链 = 本机可执行代码 + 构建期脚本执行，授权只是策略级边界。

/// 工具链 profile 上限（H.5：先只做「单文件构建器」与「Node + 包管理器」两档）。
pub const MAX_TOOLCHAINS: usize = 2;

/// `dev.build.args` 上限。
const MAX_BUILD_ARGS: usize = 32;

/// 允许的裸命令名（只能来自工具链 binDir；宿主不读全局 PATH 里的任意程序）。
const DEV_COMMAND_ALLOWLIST: &[&str] = &[
    "node",
    "npm",
    "npx",
    "pnpm",
    "yarn",
    "bun",
    "deno",
    "cargo",
    "rustc",
    "python",
    "python3",
    "pip",
    "pwsh",
    "powershell",
    "makensis",
];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PluginDevSpec {
    #[serde(default)]
    pub toolchains: Vec<ToolchainProfile>,
    #[serde(default)]
    pub build: Option<PluginBuildSpec>,
}

/// 一个可安装的工具链 profile。`url` 必须 https；`sha256` **必填**。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainProfile {
    /// profile 标识（同时是 `.toolchain\<id>\` 的目录名）
    pub id: String,
    /// https 直链
    pub url: String,
    /// 压缩包 / 文件本体的 sha256（64 位十六进制，必填）
    #[serde(default)]
    pub sha256: String,
    /// 相对插件目录的落点：`file` = 文件路径；`archive` = 解压目标目录
    #[serde(default)]
    pub dest: String,
    /// `file` / `archive`（缺省 `file`）
    #[serde(default)]
    pub kind: String,
    /// 版本号（只用于展示与授权指纹）
    #[serde(default)]
    pub version: String,
    /// 装完后加入 PATH 的目录（相对插件目录；archive 通常是解压根）
    #[serde(default)]
    pub bin_dir: String,
}

/// 构建命令：在插件目录内执行，**永不自动运行**。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PluginBuildSpec {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// 工作目录（相对插件目录；空 = 插件目录）
    #[serde(default)]
    pub cwd: String,
}

/// 开发段的授权指纹：`id + profile(id/version/sha256/url/dest/kind/binDir) + build`。
/// 任一处变化 ⇒ 旧授权作废、必须重新授权。
pub fn dev_fingerprint(plugin_id: &str, dev: &PluginDevSpec) -> String {
    let mut s = String::new();
    s.push_str(plugin_id.trim());
    for p in &dev.toolchains {
        s.push('\x1f');
        s.push_str(&format!(
            "{}|{}|{}|{}|{}|{}|{}",
            p.id.trim(),
            p.version.trim(),
            p.sha256.trim().to_ascii_lowercase(),
            p.url.trim(),
            p.dest.trim(),
            p.kind.trim(),
            p.bin_dir.trim()
        ));
    }
    if let Some(b) = &dev.build {
        s.push('\x1e');
        s.push_str(b.command.trim());
        for a in &b.args {
            s.push('\x1f');
            s.push_str(a);
        }
        s.push('\x1f');
        s.push_str(b.cwd.trim());
    }
    hex_sha256(s.as_bytes())
}

/// 相对路径是否落在 `.toolchain\` 下（`/` 与 `\` 两种写法都认）。
fn under_toolchain_dir(rel: &str) -> bool {
    let norm = rel.trim().replace('\\', "/");
    norm.starts_with(".toolchain/")
}

/// 校验 `dev` 段。能静态判定的一律拒绝整包（同清单其它字段）。
fn validate_dev(dev: &PluginDevSpec) -> Result<(), String> {
    if dev.toolchains.len() > MAX_TOOLCHAINS {
        return Err(format!(
            "dev.toolchains 过多（{} 条，上限 {MAX_TOOLCHAINS}）",
            dev.toolchains.len()
        ));
    }
    let mut ids = std::collections::HashSet::new();
    for p in &dev.toolchains {
        if !is_safe_id(p.id.trim()) {
            return Err(format!("dev.toolchains[].id 非法：{}", p.id));
        }
        if !ids.insert(p.id.trim()) {
            return Err(format!("dev.toolchains[].id 重复：{}", p.id));
        }
        if !p.url.trim().starts_with("https://") {
            return Err(format!(
                "dev.toolchains[{}].url 必须是 https：{}",
                p.id, p.url
            ));
        }
        // 工具链是**可执行代码**：没有哈希就不发。迁移期例外不适用于开发环境。
        if !is_sha256_hex(&p.sha256) {
            return Err(format!(
                "dev.toolchains[{}].sha256 必须是 64 位十六进制（工具链必须钉死哈希）",
                p.id
            ));
        }
        let kind = p.kind.trim();
        if !matches!(kind, "" | "file" | "archive") {
            return Err(format!(
                "dev.toolchains[{}].kind 只支持 file / archive，收到：{kind}",
                p.id
            ));
        }
        if p.dest.trim().is_empty() || safe_join(Path::new("."), p.dest.trim()).is_none() {
            return Err(format!(
                "dev.toolchains[{}].dest 必须是插件目录内的相对路径",
                p.id
            ));
        }
        // 落点必须在 `.toolchain\` 下：卸载删插件目录即回收，不留宿主级残留。
        if !under_toolchain_dir(p.dest.trim()) {
            return Err(format!(
                "dev.toolchains[{}].dest 必须在 .toolchain\\ 下：{}",
                p.id, p.dest
            ));
        }
        if !p.bin_dir.trim().is_empty()
            && (safe_join(Path::new("."), p.bin_dir.trim()).is_none()
                || !under_toolchain_dir(p.bin_dir.trim()))
        {
            return Err(format!(
                "dev.toolchains[{}].binDir 必须在插件目录的 .toolchain\\ 下",
                p.id
            ));
        }
    }
    if let Some(b) = &dev.build {
        let cmd = b.command.trim();
        if cmd.is_empty() || cmd.contains('\0') {
            return Err("dev.build.command 不能为空".into());
        }
        let looks_like_path = cmd.contains('/') || cmd.contains('\\') || cmd.starts_with('.');
        if looks_like_path {
            if safe_join(Path::new("."), cmd).is_none() {
                return Err(format!("dev.build.command 越界：{cmd}"));
            }
        } else {
            let lower = cmd.to_ascii_lowercase();
            if !DEV_COMMAND_ALLOWLIST.contains(&lower.as_str()) {
                return Err(format!(
                    "dev.build.command 只允许包内相对路径或常见构建器名（node/npm/…），收到：{cmd}"
                ));
            }
        }
        if b.args.len() > MAX_BUILD_ARGS {
            return Err(format!(
                "dev.build.args 过多（{} 条，上限 {MAX_BUILD_ARGS}）",
                b.args.len()
            ));
        }
        for a in &b.args {
            if a.contains('\0') || a.chars().count() > 1024 {
                return Err("dev.build.args 含非法参数".into());
            }
        }
        if !b.cwd.trim().is_empty() && safe_join(Path::new("."), b.cwd.trim()).is_none() {
            return Err(format!("dev.build.cwd 越界：{}", b.cwd));
        }
    }
    Ok(())
}

/// 校验 `sidecar` 段。**能静态判定的一律拒绝整包**（同清单其它字段的口径）。
///
/// 「禁止越界」复用 `safe_join`（与 `dependencies.dest`、`entry` 同判据）——
/// `command` / `cwd` 都只允许插件目录内的相对路径。
fn validate_sidecar(spec: &SidecarSpec, permissions: &[String]) -> Result<(), String> {
    // 1) 必须与 process.spawn 同时出现（fail-closed：不声明能力的 sidecar 一律拒）
    if !permissions.iter().any(|p| p.trim() == "process.spawn") {
        return Err("声明了 sidecar 却缺少能力 process.spawn（两者必须同时出现）".into());
    }
    // 2) command：非空 + 插件目录内的相对路径
    let cmd = spec.command.trim();
    if cmd.is_empty() {
        return Err("sidecar 缺少 command（可执行文件路径）".into());
    }
    if safe_join(Path::new("."), cmd).is_none() {
        return Err(format!("sidecar.command 必须是插件目录内的相对路径：{cmd}"));
    }
    // 3) transport / protocol / restart 枚举
    let t = spec.transport.trim();
    if !matches!(t, "" | "stdio" | "http" | "both") {
        return Err(format!(
            "sidecar.transport 只支持 stdio / http / both，收到：{t}"
        ));
    }
    let p = spec.protocol.trim();
    if !matches!(p, "" | "ndjson") {
        return Err(format!("sidecar.protocol 只支持 ndjson，收到：{p}"));
    }
    let r = spec.restart.trim();
    if !matches!(r, "" | "never" | "on-failure") {
        return Err(format!(
            "sidecar.restart 只支持 never / on-failure，收到：{r}"
        ));
    }
    // 3.1) 就绪超时：只接受 1..=3600（0 会被解释成「立刻超时」，明显是笔误）
    if spec.ready_timeout_sec == 0 || spec.ready_timeout_sec > MAX_SIDECAR_READY_TIMEOUT_SEC {
        return Err(format!(
            "sidecar.readyTimeoutSec 需在 1..={MAX_SIDECAR_READY_TIMEOUT_SEC} 秒之间，收到：{}",
            spec.ready_timeout_sec
        ));
    }
    // 3.2) 资源闸（E.6）：0 = 不限制；非 0 必须在合理区间内
    if spec.memory_limit_mib != 0
        && !(MIN_SIDECAR_MEMORY_MIB..=MAX_SIDECAR_MEMORY_MIB).contains(&spec.memory_limit_mib)
    {
        return Err(format!(
            "sidecar.memoryLimitMiB 需为 0 或 {MIN_SIDECAR_MEMORY_MIB}..={MAX_SIDECAR_MEMORY_MIB}，收到：{}",
            spec.memory_limit_mib
        ));
    }
    if spec.process_limit > MAX_SIDECAR_PROCESS_LIMIT {
        return Err(format!(
            "sidecar.processLimit 需为 0..={MAX_SIDECAR_PROCESS_LIMIT}，收到：{}",
            spec.process_limit
        ));
    }
    // 4) cwd：不写 = 插件目录；写了就必须在插件目录内
    let cwd = spec.cwd.trim();
    if !cwd.is_empty() && safe_join(Path::new("."), cwd).is_none() {
        return Err(format!("sidecar.cwd 必须在插件目录内（不得越界）：{cwd}"));
    }
    // 5) env：只增不删，键值不许带 NUL
    if spec.env.len() > MAX_SIDECAR_ENV {
        return Err(format!(
            "sidecar.env 过多（{} 条，上限 {MAX_SIDECAR_ENV}）",
            spec.env.len()
        ));
    }
    for (k, v) in &spec.env {
        if k.trim().is_empty() || k.contains('\0') || v.contains('\0') {
            return Err(format!("sidecar.env 含非法键值：{k}"));
        }
    }
    // 6) sha256：写了就必须是 64 位十六进制
    let s = spec.sha256.trim();
    if !s.is_empty() && (s.len() != 64 || !s.chars().all(|c| c.is_ascii_hexdigit())) {
        return Err(format!("sidecar.sha256 必须是 64 位十六进制：{s}"));
    }
    // 7) 授权端口白名单
    if spec.allow_local_ports.len() > MAX_SIDECAR_ALLOW_PORTS {
        return Err(format!(
            "sidecar.allowLocalPorts 过多（{} 条，上限 {MAX_SIDECAR_ALLOW_PORTS}）",
            spec.allow_local_ports.len()
        ));
    }
    if spec.allow_local_ports.iter().any(|x| *x == 0) {
        return Err("sidecar.allowLocalPorts 不得含 0".into());
    }
    Ok(())
}

/// 依赖安装的进度（2026-09-30 加，为「大依赖要能看到进度」）。
///
/// 宿主**只报事实**（阶段 + 已下/总字节），**不算速度** —— 速度是「两次上报之间的差值」，
/// 只有拿着时钟的那一端（前端）算才有意义；宿主在这里引入时间概念只会多一份要同步的状态。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DependencyProgress {
    /// 插件 id（前端的进度条按它过滤：同时只该有一个在装，但事件是全局的）
    pub id: String,
    /// 当前阶段：`download`（下载压缩包）/ `extract`（解压）—— 前端据此换文案
    pub phase: String,
    /// 已下载 / 待下载字节（`total = 0` 表示服务端没给 Content-Length）
    pub downloaded: u64,
    pub total: u64,
    /// 第几条 / 共几条依赖（前端显示「依赖 1/2」；`total = 0` 表示当前不是依赖阶段）
    pub index: usize,
    pub count: usize,
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
    if !matches!(d.kind.as_str(), "" | "file" | "archive") {
        return Err(format!(
            "依赖 type 只支持 file / archive / npm，收到：{}",
            d.kind
        ));
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
    /// **依赖就位**（2026-10-15）：清单 + 入口通过校验，且声明的 `sidecar.command`
    /// （本机依赖，如 `bin/llama-server.exe`）**真的存在**。见 `plugin_ready`。
    ///
    /// 与 `valid` 分开是为了如实区分两种「不能用」：`valid=false` = 包本身坏了；
    /// `valid=true` 但 `ready=false` = 包没问题、**只是依赖还没下来**（界面显示「未就绪 / 待下载」
    /// 并提供下载入口，而不是放行后让底层报「可执行文件不存在」）。
    pub ready: bool,
    /// `valid = false` 时的原因（原样显示在面板上，不猜）
    pub error: String,
    /// 这个插件声明的依赖（2026-09-28）。界面上只是「它要拉什么」的提示，
    /// 真正的拉取在 `install_dependencies`（装插件时顺带完成）。
    #[serde(default)]
    pub dependencies: Vec<PluginDependency>,
    /// 这个插件声明的宿主能力（2026-09-28）。界面上如实列出（见 `PluginManifest::permissions`）。
    #[serde(default)]
    pub permissions: Vec<String>,
    /// 这个插件要**复用哪些内置（基础）插件的挂载监听**（2026-10-05）。
    /// 前端 `attach.ts` 会逐个调基础插件那套 `attachXxxListeners(root)`。
    #[serde(default)]
    pub reuse: Vec<String>,
    /// 能力身份（2026-10-18）：宿主按它定位插件；`id` 仍是目录名。
    #[serde(default)]
    pub provides: Vec<String>,
    /// 渲染方式与外观级别（2026-10-18）。
    #[serde(default)]
    pub render: PluginRenderSpec,
    /// 这个插件声明的 sidecar（2026-10-06）。界面上如实列出「它要起哪个进程」
    /// （信任卡里还会再列一遍 command / args / sha256）。
    #[serde(default)]
    pub sidecar: Option<SidecarSpec>,
    /// 这个插件声明的开发工具链（2026-10-18）。设置页据此显示「开发工具链」分区；
    /// **未授权时不能装、不能构建**（账本在 `config\plugin-toolchain.json`）。
    #[serde(default)]
    pub dev: Option<PluginDevSpec>,
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
    if fs::read_dir(&old)
        .map(|mut it| it.next().is_none())
        .unwrap_or(false)
    {
        let _ = fs::remove_dir(&old);
    }
    if moved > 0 {
        crate::log::info(format!(
            "旧 plugins\\ 目录已迁移 {moved} 个插件到 Modules\\"
        ));
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
    // `reuse`：只校验 id 形状与条数；「这个 id 是不是一个真的基础插件」由前端判定
    // （基础插件名单是前端概念，见 kinds.ts 的 BASE_PLUGIN_IDS）—— 宿主不重复一份名单。
    if m.reuse.len() > MAX_REUSE {
        return Err(format!(
            "reuse 条目过多（{} 条，上限 {MAX_REUSE}）",
            m.reuse.len()
        ));
    }
    for r in &m.reuse {
        if !is_safe_id(r.trim()) {
            return Err(format!("reuse 里的 id 非法（只允许 [a-z0-9._-]）：{r}"));
        }
    }
    // provides（2026-10-18）：形状与条数校验；跨插件冲突由宿主 warn（E.1）。
    if m.provides.len() > MAX_PROVIDES {
        return Err(format!(
            "provides 条目过多（{} 条，上限 {MAX_PROVIDES}）",
            m.provides.len()
        ));
    }
    for p in &m.provides {
        if !is_safe_id(p.trim()) {
            return Err(format!("provides 里的身份非法（只允许 [a-z0-9._-]）：{p}"));
        }
    }
    // render.slots（2026-10-18）：常驻锚点只对 iframe 引擎开放 —— DOM 插件没有独立文档，
    // 宿主无法把它的样式 / 脚本限制在某个槽位内。重复声明按错误处理（不做去重，避免静默）。
    if !m.render.slots.is_empty() && m.render.engine != RenderEngine::Iframe {
        return Err("render.slots 只支持 render.engine=iframe".into());
    }
    let mut seen_slots = std::collections::HashSet::new();
    for slot in &m.render.slots {
        if !seen_slots.insert(*slot) {
            return Err(format!("render.slots 重复声明：{slot:?}"));
        }
    }
    // sidecar（2026-10-06）：声明了就必须同时有 process.spawn，且 command/cwd 不得越界
    if let Some(spec) = &m.sidecar {
        validate_sidecar(spec, &m.permissions)?;
    }
    // dev（2026-10-18，轨道 H）：工具链必须 https + 钉死 sha256；构建命令不得越界。
    if let Some(dev) = &m.dev {
        validate_dev(dev)?;
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
    /// 插件 zip 的 sha256（64 位十六进制，大小写不敏感）。
    ///
    /// 2026-10-18 起由 `publish-plugins.ps1` 生成；**迁移期**：缺省仍可安装但 warn 留痕，
    /// 写了就必须对上（供应链完整性，见 H.11 / E.5）。
    #[serde(default)]
    pub sha256: String,
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
        // sha256 可以不写（迁移期），写了就必须是 64 位十六进制 —— 写错的哈希等于没有哈希，
        // 继续放行只会让「校验过」变成一句假话。
        if !e.sha256.trim().is_empty() && !is_sha256_hex(&e.sha256) {
            crate::log::warn(format!(
                "插件索引里的 {} 的 sha256 不是 64 位十六进制，已跳过",
                e.id
            ));
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
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| format!("不是合法的 zip：{e}"))?;
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
///
/// 依赖阶段的进度经 `on_dep_progress` 回传（2026-09-30）—— 插件包本体（几十 KB~几十 MB）
/// 由调用方自己下载、自己报进度；这里只管**落盘之后**那些依赖的下载/解压，
/// 大块头（PaddleOCR 的 88MB `.7z`）都在这一步。
pub fn install_from_bytes_with_progress(
    bytes: &[u8],
    plugins_root: &Path,
    on_dep_progress: &mut dyn FnMut(DependencyProgress),
) -> Result<String, String> {
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
        let text = fs::read_to_string(&manifest_path).map_err(|e| format!("读清单失败：{e}"))?;
        let manifest = parse_manifest(&text)?;
        let plugin_root = manifest_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| staging.clone());
        let entry_path = safe_join(&plugin_root, &manifest.entry)
            .ok_or_else(|| format!("entry 越界：{}", manifest.entry))?;
        if !entry_path.is_file() {
            return Err(format!("清单里写的入口不存在：{}", manifest.entry));
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
    if let Err(e) = install_dependencies_with_progress(
        &target,
        &manifest.id,
        &manifest.dependencies,
        on_dep_progress,
    ) {
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

/// 依赖下载的中止标志（2026-10-15）。
///
/// 为什么用一个全局 `AtomicBool`：**同一时刻只该有一次依赖安装**（前端按插件串行发起），
/// 所以就一个开关够了 —— 再按 id 维护一张表只是多一份要同步的状态（本地模型那次是 2.33 GiB，
/// 用户必须能中途叫停，不能干等）。
static CANCEL_DEP_INSTALL: AtomicBool = AtomicBool::new(false);

/// 请求中止当前这次依赖安装。下载循环每读一块检查一次；幂等。
pub fn cancel_dependency_install() {
    CANCEL_DEP_INSTALL.store(true, Ordering::Relaxed);
}

fn dep_install_cancelled() -> bool {
    CANCEL_DEP_INSTALL.load(Ordering::Relaxed)
}

// ── 安装单飞闸（2026-10-27）─────────────────────────────────────────
//
// 为什么必须有：**暂停**的实现是「宿主线程原地等」，所以「暂停之后又点一次下载 / 继续」
// 会**再起一个**下载线程 —— 两个线程写同一个 `.part`、各自算 offset，界面上就出现
// 「一快一慢两种进度」（快的是后起的那个），而且谁先读完谁把半截文件改名走 ⇒
// 「秒更新完毕但其实没装完」。这道闸把它们挡在入口。
//
// 粒度 = **一次安装**（插件包本体 + 它自己的依赖算同一次）；依赖补齐 / 工具链安装各算一次。
static INSTALL_BUSY: AtomicBool = AtomicBool::new(false);

/// 安装闸的持有凭证；`drop` 时释放。
#[derive(Debug)]
pub struct InstallBusyGuard(());

impl Drop for InstallBusyGuard {
    fn drop(&mut self) {
        INSTALL_BUSY.store(false, Ordering::SeqCst);
    }
}

/// 占用安装闸；已有一次在进行时返回 Err（**原样报给界面**，由调用方决定文案）。
///
/// 注意：**暂停中的那次仍然占着闸** —— 恢复要走 `plugin_install_pause(false)`，
/// 不能再发一次安装（那正是「两种进度」的成因）。
pub fn try_begin_install(what: &str) -> Result<InstallBusyGuard, String> {
    if INSTALL_BUSY.swap(true, Ordering::SeqCst) {
        let st = install_state();
        let hint = if st.active && st.paused {
            "那一次已暂停：到对应那一行点「继续」接着下"
        } else {
            "等它结束，或先取消"
        };
        return Err(format!("已有一次安装在进行（{what}）—— {hint}"));
    }
    Ok(InstallBusyGuard(()))
}

/// 当前是否有安装占着闸（测试 / 诊断用）。
pub fn install_busy() -> bool {
    INSTALL_BUSY.load(Ordering::SeqCst)
}

/// 逐条安装清单里的依赖（`dependencies[]`）。**任意一条失败即中止**，由调用方决定是否回滚。
///
/// 顺序执行、不并发：依赖条数是个位数，并发带来的收益抵不过「并发失败时哪一半落了盘」的复杂度。
/// 进度经 `on_progress` 回传（2026-09-30）；`plugin_id` 只用于填进 `DependencyProgress`
/// （前端的进度条按它过滤事件），校验 / 落盘逻辑与这条无关。
pub fn install_dependencies_with_progress(
    plugin_dir: &Path,
    plugin_id: &str,
    deps: &[PluginDependency],
    on_progress: &mut dyn FnMut(DependencyProgress),
) -> Result<(), String> {
    if deps.is_empty() {
        return Ok(());
    }
    // 每次安装开始时清掉中止标志：否则上一次的「取消」会立刻把这次也掐掉
    CANCEL_DEP_INSTALL.store(false, Ordering::Relaxed);
    if deps.len() > MAX_DEP_ENTRIES {
        return Err(format!("依赖条目过多（{} 条）", deps.len()));
    }
    for (i, d) in deps.iter().enumerate() {
        let what = if d.is_npm() {
            let pkg = if d.package.trim().is_empty() {
                d.url.trim()
            } else {
                d.package.trim()
            };
            format!("npm:{pkg}")
        } else {
            d.url.clone()
        };
        // 清单在 `parse_manifest` 已校验过一次；这里再校验一次是**双保险** ——
        // `install_dependencies` 是 pub，将来可能被「只补依赖」的入口直接调用。
        validate_dependency(d).map_err(|e| format!("第 {} 条依赖（{what}）非法：{e}", i + 1))?;
        let index = i + 1;
        let count = deps.len();
        // 界面的「本次安装」快照按**当前文件**记 —— 切页签 / 关面板回来时据此把进度挂回那一行
        begin_install_state("deps", plugin_id, &d.url);
        let mut report = |phase: &str, downloaded: u64, total: u64| {
            on_progress(DependencyProgress {
                id: plugin_id.to_string(),
                phase: phase.to_string(),
                downloaded,
                total,
                index,
                count,
            });
        };
        let r = if d.is_npm() {
            install_npm_dependency(plugin_dir, d)
        } else if d.is_archive() {
            install_archive_dependency(plugin_dir, d, &mut report)
        } else {
            install_file_dependency(plugin_dir, d, &mut report)
        };
        if let Err(e) = r {
            finish_install_state();
            return Err(format!("第 {} 条依赖（{what}）安装失败：{e}", i + 1));
        }
    }
    finish_install_state();
    Ok(())
}

/// 下载一个依赖文件到插件目录内（`file` 形态）。
///
/// 三道闸与插件包同源：**只 https**、**体积上限**、**落点必须留在插件目录内**（`safe_join`）；
/// 另外多一道 **sha256**（清单写了就必须对上）。先写 `.part` 再改名 —— 半截文件不能被当成装好的。
///
/// **2026-10-09：改为流式落盘**。原实现把整包读进 `Vec<u8>` 再写盘 —— 本地决策模型换成 3B 级
/// （约 2.5 GiB）后那是一次 2.5 GiB 的分配，安装过程会被顶爆。现在边读边写 `.part`、同时
/// 增量算 sha256，内存占用回到常量级（64 KiB 缓冲）。
fn install_file_dependency(
    plugin_dir: &Path,
    d: &PluginDependency,
    report: &mut dyn FnMut(&str, u64, u64),
) -> Result<(), String> {
    let dest =
        safe_join(plugin_dir, &d.dest).ok_or_else(|| format!("dest 越界，已拒绝：{}", d.dest))?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("建依赖目录失败：{e}"))?;
    }
    let want = d.sha256.trim();
    // 2026-10-27：改走**可续传**下载（半截文件放 `temp\downloads\`）——
    // 关面板 / 切页签 / 退出应用后重来都能接着下；旧实现直接写 `dest.part`，
    // 插件目录一旦被升级流程整体替换，断点就跟着没了。
    let part = part_path_for(&d.url, want);
    fetch_resumable(&d.url, &part, MAX_DEP_BYTES, want, report)?;
    if want.is_empty() {
        crate::log::warn(format!(
            "依赖 {} 没有 sha256，已按「只校验 https」安装 —— 作者最好补上校验和",
            d.url
        ));
    } else {
        let got = sha256_file(&part)?;
        if !got.eq_ignore_ascii_case(want) {
            remove_part(&part);
            return Err(format!("sha256 不匹配（期望 {want}，实得 {got}）"));
        }
    }
    if let Err(e) = fs::rename(&part, &dest) {
        // 跨卷时 `rename` 会失败（用户可能把插件目录指到别的盘）⇒ 退化成复制
        fs::copy(&part, &dest).map_err(|e2| {
            remove_part(&part);
            format!("落盘依赖失败：{e}（复制也失败：{e2}）")
        })?;
        let _ = fs::remove_file(&part);
    }
    let _ = fs::remove_file(part_meta_path(&part));
    Ok(())
}

/// 压缩包的两种形态（按**魔数**识别，不看扩展名 —— 扩展名是作者随手写的，魔数是文件自己说的）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveFormat {
    Zip,
    SevenZ,
}

/// 按头部魔数判断压缩格式：`PK\x03\x04`（zip）/ `7z¼¯'`（7z）。
fn detect_archive_format(bytes: &[u8]) -> Result<ArchiveFormat, String> {
    const ZIP_MAGIC: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
    const SEVENZ_MAGIC: [u8; 6] = [0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c];
    if bytes.starts_with(&SEVENZ_MAGIC) {
        return Ok(ArchiveFormat::SevenZ);
    }
    if bytes.starts_with(&ZIP_MAGIC) {
        return Ok(ArchiveFormat::Zip);
    }
    Err("依赖压缩包既不是 zip 也不是 7z（按魔数判断）".into())
}

/// `archive` 形态的依赖：下载压缩包 → 校验 sha256 → 解到插件目录内的 staging → 原子替换 `dest` 目录。
///
/// 安全判据与插件包同源（见文件头）：路径穿越逐条拒、解压后总量与条目数有上限、先 staging 再改名。
/// **7z 那条必须自己给 extract_fn** —— sevenz-rust 的默认解压直接 `dest.join(entry.name())`，
/// 对 `..\..\x` 不做任何拦截（见其 `default_entry_extract_fn`），拿它解第三方包等于把
/// 「路径穿越」这道闸整个去掉。zip 那条复用 `extract_zip`（已经是安全的）。
fn install_archive_dependency(
    plugin_dir: &Path,
    d: &PluginDependency,
    report: &mut dyn FnMut(&str, u64, u64),
) -> Result<(), String> {
    let dest =
        safe_join(plugin_dir, &d.dest).ok_or_else(|| format!("dest 越界，已拒绝：{}", d.dest))?;
    let want = d.sha256.trim();
    // 2026-10-27：与 `file` 形态同一条可续传下载（压缩包 149 MB 量级，断点最值钱）
    let part = part_path_for(&d.url, want);
    fetch_resumable(&d.url, &part, MAX_DEP_BYTES, want, report)?;
    if want.is_empty() {
        crate::log::warn(format!(
            "压缩包依赖 {} 没有 sha256，已按「只校验 https」安装 —— 作者最好补上校验和",
            d.url
        ));
    } else {
        let got = sha256_file(&part)?;
        if !got.eq_ignore_ascii_case(want) {
            remove_part(&part);
            return Err(format!("sha256 不匹配（期望 {want}，实得 {got}）"));
        }
    }
    let bytes = fs::read(&part).map_err(|e| {
        remove_part(&part);
        format!("读下载文件失败：{e}")
    })?;
    remove_part(&part);
    let format = detect_archive_format(&bytes)?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|t| t.as_millis())
        .unwrap_or(0);
    let staging = plugin_dir.join(format!(".dep-staging-{stamp}"));
    let _ = fs::remove_dir_all(&staging);
    report("extract", 0, 0);

    let extracted = extract_archive(&bytes, format, &staging);
    if let Err(e) = extracted {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }

    // 上游常把整棵树包在一个顶层目录里（PaddleOCR 的 `.7z` 就是 `PaddleOCR-json_v1.4.1/`）。
    // 只有「唯一一个子目录、包里没有别的文件」时才把它摊平 —— 与 `install_from_bytes`
    // 对插件包的处置同源（那里也是认这一种形状），摊平后 dest 下直接是引擎 exe + models/。
    let root = flatten_single_top_dir(&staging);

    // 原子替换 dest：旧的先改名成备份，新的搬进去，成功才删备份、失败搬回来。
    let backup = plugin_dir.join(format!(".dep-old-{stamp}"));
    let had_old = dest.exists();
    if had_old {
        let _ = fs::remove_dir_all(&backup);
        if let Err(e) = fs::rename(&dest, &backup) {
            let _ = fs::remove_dir_all(&staging);
            return Err(format!("备份旧依赖失败（未改动已装内容）：{e}"));
        }
    }
    if let Some(parent) = dest.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            let _ = fs::remove_dir_all(&staging);
            if had_old {
                let _ = fs::rename(&backup, &dest);
            }
            return Err(format!("建依赖目录失败：{e}"));
        }
    }
    if let Err(e) = fs::rename(&root, &dest) {
        let _ = fs::remove_dir_all(&staging);
        if had_old {
            let _ = fs::rename(&backup, &dest);
        }
        return Err(format!("落盘依赖失败：{e}"));
    }
    let _ = fs::remove_dir_all(&staging);
    if had_old {
        let _ = fs::remove_dir_all(&backup);
    }
    Ok(())
}

/// 把压缩包解到 `staging`（建目录 + 按格式分派）。抽出来只为让单测能直接打这条路径
/// （zip 穿越、7z 穿越两套判据都在它下游）。
fn extract_archive(bytes: &[u8], format: ArchiveFormat, staging: &Path) -> Result<(), String> {
    fs::create_dir_all(staging).map_err(|e| format!("建解压目录失败：{e}"))?;
    match format {
        ArchiveFormat::Zip => {
            extract_zip(std::io::Cursor::new(bytes), staging, MAX_DEP_EXTRACT_BYTES)
        }
        ArchiveFormat::SevenZ => extract_sevenz(bytes, staging),
    }
}

/// 若 `root` 下**只有唯一一个子目录、且没有任何文件**，返回那个子目录；否则返回 `root`。
/// 这是上游打包最常见的形状（`pkg-v1.2/…`），摊平后调用方的路径少一层。
fn flatten_single_top_dir(root: &Path) -> PathBuf {
    let Ok(entries) = fs::read_dir(root) else {
        return root.to_path_buf();
    };
    let mut only_dir: Option<PathBuf> = None;
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() {
            return root.to_path_buf();
        }
        if only_dir.is_some() {
            return root.to_path_buf();
        }
        only_dir = Some(p);
    }
    only_dir.unwrap_or_else(|| root.to_path_buf())
}

/// 解 7z 到 `staging`，**逐条校验落点 + 累计解压量 + 条目数**（理由见调用方注释）。
///
/// 用 `decompress_with_extract_fn` 而不是 `decompress_file`：后者把 `dest.join(entry.name())`
/// 直接交给默认 extract_fn，`..\..\evil` 会被写到 dest 外面去。这里每条都过 `safe_join`。
fn extract_sevenz(bytes: &[u8], staging: &Path) -> Result<(), String> {
    let mut written: u64 = 0;
    let mut count: usize = 0;
    let result = sevenz_rust::decompress_with_extract_fn(
        std::io::Cursor::new(bytes),
        staging,
        |entry, reader, _dest| {
            count += 1;
            if count > MAX_DEP_EXTRACT_ENTRIES {
                return Err(sevenz_rust::Error::other(format!(
                    "压缩包内条目过多（超过 {MAX_DEP_EXTRACT_ENTRIES} 个）"
                )));
            }
            // **用 entry.name() 自己拼**，不用 sevenz-rust 预拼好的 _dest（那个没校验过）
            let Some(path) = safe_join(staging, entry.name()) else {
                return Err(sevenz_rust::Error::other(format!(
                    "压缩包内含越界路径，已拒绝安装：{}",
                    entry.name()
                )));
            };
            if entry.is_directory() {
                fs::create_dir_all(&path).map_err(sevenz_rust::Error::io)?;
                return Ok(true);
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(sevenz_rust::Error::io)?;
            }
            let mut out = fs::File::create(&path).map_err(sevenz_rust::Error::io)?;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = reader.read(&mut buf).map_err(sevenz_rust::Error::io)?;
                if n == 0 {
                    break;
                }
                written += n as u64;
                if written > MAX_DEP_EXTRACT_BYTES {
                    return Err(sevenz_rust::Error::other(format!(
                        "解压后体积超过上限（{} MB），已拒绝安装",
                        MAX_DEP_EXTRACT_BYTES / 1024 / 1024
                    )));
                }
                out.write_all(&buf[..n]).map_err(sevenz_rust::Error::io)?;
            }
            Ok(true)
        },
    );
    result.map_err(|e| format!("7z 解压失败：{e}"))
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
        return Err(format!(
            "npm 退出码 {}：{tail}",
            out.status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

// ── 可续传下载 + 暂停（2026-10-27）─────────────────────────────────
//
// 用户诉求（2026-10-27）：① 下载中可以**暂停 / 继续**；② 关掉设置面板、切到别的分页、
// 甚至**强制退出 Lunac**，再回来都要从**断点**接着下，而不是从 0 重来。
//
// 做法：所有下载都落到 `temp\downloads\<url+sha256 的 sha256 前 16 位>.part`，旁边放一份
// `.part.json`（url / etag / total / offset / sha256）。重新开始时按 `.part` 的**实际长度**
// 发 `Range: bytes=<offset>-`：
//   · 206 ⇒ 追加写；200（服务端不认 Range，或 `If-Range` 不匹配）⇒ 丢断点从头来；
//   · 416 ⇒ 断点比远端还大，丢掉重来；
//   · **暂停** = 关掉这条连接、原地等（不读流，也就不占着 socket），继续时重新发 Range；
//   · **取消** = 删 `.part` + `.part.json`（与旧语义一致）。
// sha256 只能在**全部下完**后整份重算（增量哈希没法跨进程接上），代价是一次本地顺序读。

/// 下载暂存目录：`<exe 根>\temp\downloads`。
///
/// 放 `temp\` 而不是插件目录，两个理由：① 插件目录在「安装 / 升级」时会被整体替换，
/// 半截文件不该跟着搬；② 断点要在**插件还没装好**时也活着（强制退出后重来）。
pub fn downloads_dir() -> PathBuf {
    crate::storage::lunac_root_dir()
        .join("temp")
        .join("downloads")
}

/// 一个下载断点的元数据（与 `.part` 同目录、同名 + `.json`）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PartMeta {
    url: String,
    /// 归属的插件 id（包本体阶段为空）。重启后界面靠它把「继续下载」挂回对应的行。
    #[serde(default)]
    id: String,
    /// 服务端 ETag（可能为空）。有它就带 `If-Range`，避免「远端换了文件还往后拼」。
    #[serde(default)]
    etag: String,
    /// 服务端声明的总字节；0 = 不知道。
    #[serde(default)]
    total: u64,
    /// 已写入 `.part` 的字节数。
    #[serde(default)]
    offset: u64,
    /// 清单里的 sha256（空 = 清单没写）。换版本 / 换文件时旧断点据此作废。
    #[serde(default)]
    sha256: String,
}

/// 界面上的「本次安装」进度快照（离屏 / 切页签 / 重开面板后用它重新挂回）。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallState {
    pub active: bool,
    /// `package`（插件包本体）/ `deps`（补依赖）
    pub kind: String,
    /// 插件 id（插件包本体那段还没解析出清单，给空串）
    pub id: String,
    /// 当前正在下的那个文件 —— 前端按它把进度挂回**对应的那一行**
    pub url: String,
    pub phase: String,
    pub downloaded: u64,
    pub total: u64,
    pub paused: bool,
    /// 磁盘上有可续传的半截文件（下次点「安装 / 继续」会接着下）
    pub resumable: bool,
}

/// 磁盘上还剩的断点（强制退出后重开应用时，界面仍能显示「继续下载」）。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingDownload {
    pub url: String,
    /// 归属的插件 id（包本体阶段为空）
    pub id: String,
    pub downloaded: u64,
    pub total: u64,
}

static INSTALL_STATE: Mutex<Option<InstallState>> = Mutex::new(None);
static INSTALL_PAUSED: AtomicBool = AtomicBool::new(false);

fn mutate_state(f: impl FnOnce(&mut InstallState)) {
    if let Ok(mut g) = INSTALL_STATE.lock() {
        f(g.get_or_insert_with(InstallState::default));
    }
}

/// 暂停 / 继续当前这次下载。没有活动下载时也安全（只置标志）。
pub fn set_install_paused(paused: bool) {
    INSTALL_PAUSED.store(paused, Ordering::Relaxed);
    mutate_state(|s| s.paused = paused);
}

/// 当前安装状态（没有活动下载时 `active = false`）。
pub fn install_state() -> InstallState {
    INSTALL_STATE
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default()
}

/// 磁盘上还剩哪些断点。`downloaded` 取 `.part` 的**实际长度**（不信 json 里的旧值）。
pub fn pending_downloads() -> Vec<PendingDownload> {
    let Ok(entries) = fs::read_dir(downloads_dir()) else {
        return Vec::new();
    };
    let mut out: Vec<PendingDownload> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                return None;
            }
            let meta = read_part_meta(&p)?;
            let part = p.with_extension("");
            let n = fs::metadata(&part).ok()?.len();
            (n > 0).then_some(PendingDownload {
                url: meta.url,
                id: meta.id,
                downloaded: n,
                total: meta.total,
            })
        })
        .collect();
    out.sort_by(|a, b| a.url.cmp(&b.url));
    out
}

fn begin_install_state(kind: &str, id: &str, url: &str) {
    INSTALL_PAUSED.store(false, Ordering::Relaxed);
    if let Ok(mut g) = INSTALL_STATE.lock() {
        *g = Some(InstallState {
            active: true,
            kind: kind.to_string(),
            id: id.to_string(),
            url: url.to_string(),
            phase: "download".into(),
            ..Default::default()
        });
    }
}

fn finish_install_state() {
    INSTALL_PAUSED.store(false, Ordering::Relaxed);
    mutate_state(|s| {
        s.active = false;
        s.paused = false;
        s.resumable = false;
    });
}

/// 插件包本体开始下载（`commands.rs` 用；前端凭 `kind = "package"` + `url` 把进度挂回市场那一行）。
pub fn begin_package_install(url: &str) {
    begin_install_state("package", "", url);
}

/// 插件包本体这一段结束（成功 / 失败都要调，否则界面会一直停在「安装中」）。
pub fn finish_package_install() {
    finish_install_state();
}

/// 暂停时**关掉连接、原地等**。返回 Err = 等的时候被取消。
fn wait_while_paused() -> Result<(), String> {
    while INSTALL_PAUSED.load(Ordering::Relaxed) {
        if dep_install_cancelled() {
            INSTALL_PAUSED.store(false, Ordering::Relaxed);
            return Err("已取消下载".into());
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    Ok(())
}

fn part_meta_path(part: &Path) -> PathBuf {
    PathBuf::from(format!("{}.json", part.display()))
}

fn read_part_meta(p: &Path) -> Option<PartMeta> {
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}

fn write_part_meta(p: &Path, m: &PartMeta) {
    if let Ok(s) = serde_json::to_string(m) {
        let _ = fs::write(p, s);
    }
}

fn remove_part(part: &Path) {
    let _ = fs::remove_file(part);
    let _ = fs::remove_file(part_meta_path(part));
}

/// 断点文件名：`temp\downloads\<sha256(url + 清单 sha256) 前 16 位>.part`。
///
/// 把清单里的 sha256 也算进去：作者换版本时（url 不变、哈希变）旧断点自动作废，
/// 不会把上一版的前半段和这一版的后半段拼在一起。
fn part_path_for(url: &str, sha256_want: &str) -> PathBuf {
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    h.update(b"\n");
    h.update(sha256_want.trim().to_ascii_lowercase().as_bytes());
    let tag = hex_digest(h.finalize().as_slice());
    downloads_dir().join(format!("{}.part", &tag[..16]))
}

/// `Content-Range: bytes 100-999/1000` → `1000`（取不到就给 None）。
fn content_range_total(resp: &reqwest::blocking::Response) -> Option<u64> {
    let v = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)?
        .to_str()
        .ok()?;
    v.rsplit('/').next()?.trim().parse().ok()
}

/// 发一个 https GET（`offset > 0` 时带 `Range` + 可选 `If-Range`）。
///
/// 与 `open_https_stream` 的区别只在这里：**416 单独给一个可识别的错误**，
/// 好让调用方丢掉断点重来，而不是把整个安装判死。
fn open_https_range(
    url: &str,
    offset: u64,
    etag: &str,
) -> Result<reqwest::blocking::Response, String> {
    if !url.starts_with("https://") {
        return Err("只允许 https 的地址".into());
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Client error: {e}"))?;
    let mut req = client.get(url);
    if offset > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        if !etag.is_empty() {
            req = req.header(reqwest::header::IF_RANGE, etag);
        }
    }
    let resp = req.send().map_err(|e| format!("下载失败：{e}"))?;
    let st = resp.status();
    if st.is_success() {
        return Ok(resp);
    }
    if st == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        return Err("HTTP 416：断点无效".into());
    }
    Err(format!("HTTP {}：下载失败", st.as_u16()))
}

/// 整个文件的 sha256（小写十六进制）。**不能增量算**：断点续传跨进程，没有可接的哈希状态。
fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = fs::File::open(path).map_err(|e| format!("读下载文件失败：{e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("读下载文件失败：{e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_digest(hasher.finalize().as_slice()))
}

/// 把一个 https 文件下到 `part`（可续传、可暂停），成功后**保留** `part` 交给调用方处置。
///
/// 调用方负责：校验 sha256（`sha256_file`）、搬到自己要的位置、`remove_part(part)`。
fn fetch_resumable(
    url: &str,
    part: &Path,
    limit: u64,
    sha256_want: &str,
    report: &mut dyn FnMut(&str, u64, u64),
) -> Result<(), String> {
    if let Some(parent) = part.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("建下载目录失败：{e}"))?;
    }
    let meta_path = part_meta_path(part);
    let want = sha256_want.trim().to_ascii_lowercase();
    // 归属（供重启后把「继续下载」挂回市场里对应的那一行）：包本体阶段为空
    let owner = install_state().id;
    let mut meta =
        read_part_meta(&meta_path).filter(|m| m.url == url && m.sha256.eq_ignore_ascii_case(&want));
    let mut offset = match &meta {
        Some(m) if m.offset > 0 => fs::metadata(part).map(|x| x.len()).unwrap_or(0),
        _ => 0,
    };
    if offset == 0 {
        remove_part(part);
        meta = None;
    }
    let over = || format!("超过上限（{} MB）", limit / 1024 / 1024);
    let mut retried = false;
    // 「连接提前结束」的重试次数（防死循环：对端一直只给前面一截时不能无限续）
    let mut short_reads = 0u32;

    loop {
        wait_while_paused()?;
        if dep_install_cancelled() {
            remove_part(part);
            return Err("已取消下载".into());
        }
        let etag = meta.as_ref().map(|m| m.etag.clone()).unwrap_or_default();
        let mut resp = match open_https_range(url, offset, &etag) {
            Ok(r) => r,
            Err(e) if e.contains("416") && !retried => {
                retried = true;
                offset = 0;
                remove_part(part);
                meta = None;
                continue;
            }
            Err(e) => return Err(e),
        };

        let mut total = meta.as_ref().map(|m| m.total).unwrap_or(0);
        if offset > 0 && resp.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            if let Some(t) = content_range_total(&resp) {
                total = t;
            }
        } else if offset > 0 {
            // 服务端不认 Range / If-Range 不匹配 ⇒ 从头来（不能往后拼，否则文件是坏的）
            offset = 0;
            remove_part(part);
            meta = None;
            total = resp.content_length().unwrap_or(0);
        } else {
            total = resp.content_length().unwrap_or(0);
        }
        if total > limit {
            remove_part(part);
            return Err(over());
        }
        let etag_now = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        write_part_meta(
            &meta_path,
            &PartMeta {
                url: url.to_string(),
                id: owner.clone(),
                etag: etag_now,
                total,
                offset,
                sha256: want.clone(),
            },
        );
        mutate_state(|s| {
            s.phase = "download".into();
            s.downloaded = offset;
            s.total = total;
            s.resumable = offset > 0;
            s.paused = false;
        });

        let mut file = if offset > 0 {
            fs::OpenOptions::new()
                .append(true)
                .open(part)
                .map_err(|e| format!("打开断点文件失败：{e}"))?
        } else {
            fs::File::create(part).map_err(|e| format!("建临时文件失败：{e}"))?
        };
        let mut buf = [0u8; 64 * 1024];
        let mut paused = false;
        loop {
            if dep_install_cancelled() {
                drop(file);
                remove_part(part);
                return Err("已取消下载".into());
            }
            // 暂停 = **断开这条连接**（不读流），外层重新发 Range 接上
            if INSTALL_PAUSED.load(Ordering::Relaxed) {
                paused = true;
                break;
            }
            let n = match resp.read(&mut buf) {
                Ok(n) => n,
                Err(e) => {
                    drop(file);
                    // 连接断了：断点留着，下次（或用户点继续）从 offset 接
                    write_part_meta(
                        &meta_path,
                        &PartMeta {
                            url: url.to_string(),
                            id: owner.clone(),
                            etag: String::new(),
                            total,
                            offset,
                            sha256: want.clone(),
                        },
                    );
                    return Err(format!("读取出错：{e}"));
                }
            };
            if n == 0 {
                break;
            }
            offset += n as u64;
            if offset > limit {
                drop(file);
                remove_part(part);
                return Err(over());
            }
            if let Err(e) = file.write_all(&buf[..n]) {
                drop(file);
                remove_part(part);
                return Err(format!("写下载文件失败：{e}"));
            }
            report("download", offset, total);
            mutate_state(|s| {
                s.downloaded = offset;
                s.total = total;
                s.resumable = offset > 0;
            });
        }
        if let Err(e) = file.flush() {
            remove_part(part);
            return Err(format!("写下载文件失败：{e}"));
        }
        drop(file);
        write_part_meta(
            &meta_path,
            &PartMeta {
                url: url.to_string(),
                id: owner.clone(),
                etag: meta.as_ref().map(|m| m.etag.clone()).unwrap_or_default(),
                total,
                offset,
                sha256: want.clone(),
            },
        );
        mutate_state(|s| s.resumable = offset > 0);
        if paused {
            mutate_state(|s| s.paused = true);
            continue;
        }
        // 正常读完：total 未知时以「读到 EOF」为准；**已知总数就必须一字节不差**
        //（少一截绝不能当成功 —— 那会表现为「秒更新完毕、其实没下完」）
        if total == 0 || offset == total {
            return Ok(());
        }
        if offset > total {
            remove_part(part);
            return Err(format!(
                "下载出现多余字节（{offset} / {total}），已丢弃断点重来"
            ));
        }
        // total 有值却没读够（连接被提前关）⇒ 续一次；连续 5 次仍不够就如实失败
        short_reads += 1;
        if short_reads > 5 {
            remove_part(part);
            return Err(format!(
                "下载反复中断（已重试 5 次，只拿到 {offset} / {total} 字节）"
            ));
        }
    }
}

/// 下载插件包本体到内存（可续传、可暂停）。上限沿用 `MAX_ARCHIVE_BYTES`。
pub fn fetch_package_bytes(
    url: &str,
    limit: u64,
    sha256_want: &str,
    on_progress: &mut dyn FnMut(&str, u64, u64),
) -> Result<Vec<u8>, String> {
    let part = part_path_for(url, sha256_want);
    fetch_resumable(url, &part, limit, sha256_want, on_progress)?;
    let bytes = fs::read(&part).map_err(|e| format!("读下载文件失败：{e}"))?;
    remove_part(&part);
    Ok(bytes)
}

/// 发一个 https GET 并返回响应流（非 2xx 直接报错）。超时口径见 `download_bytes` 的注释。
///
/// 抽出来是为了让「依赖下载」与 commands.rs 里的「插件包下载」共用同一套超时口径 ——
/// 两处各写一遍 `.connect_timeout(...)`，改一处漏一处的教训本仓已经有过。
pub fn open_https_stream(url: &str) -> Result<reqwest::blocking::Response, String> {
    if !url.starts_with("https://") {
        return Err("只允许 https 的地址".into());
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Client error: {e}"))?;
    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("下载失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}：下载失败", resp.status().as_u16()));
    }
    Ok(resp)
}

/// sha256 的十六进制小写表示（用于依赖校验和比对）。
pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes).as_slice())
}

/// 是不是一个合法的 sha256 十六进制串（64 位、只含 0-9a-fA-F）。
pub fn is_sha256_hex(s: &str) -> bool {
    let s = s.trim();
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 校验下载到的插件包。`expected` 为空 = 迁移期不校验（`Ok(false)`，调用方负责 warn）；
/// 非空就必须对上，否则整包拒绝（不落盘、不覆盖旧版本）。
pub fn verify_package_sha256(bytes: &[u8], expected: &str) -> Result<bool, String> {
    let want = expected.trim();
    if want.is_empty() {
        return Ok(false);
    }
    if !is_sha256_hex(want) {
        return Err(format!("sha256 必须是 64 位十六进制：{want}"));
    }
    let got = hex_sha256(bytes);
    if !got.eq_ignore_ascii_case(want) {
        return Err(format!("sha256 不匹配（期望 {want}，实得 {got}）"));
    }
    Ok(true)
}

/// 把一段摘要字节转成十六进制小写（`Sha256::finalize()` / `Sha256::digest()` 的结果）。
fn hex_digest(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 依赖就位判定（2026-10-15）：声明了 `sidecar` 的插件，其 `sidecar.command`
/// （通常是 `dependencies[]` 下下来的本机依赖，如 `bin/llama-server.exe`）**必须真的存在**。
///
/// **只对声明了 sidecar 的插件收紧**：没有 sidecar 的插件（含自带「引擎缺失 → 下载」提示的
/// OCR）恒为就绪 —— 免得把「依赖缺失但界面上另有出口」的插件也一并标成未就绪。
///
/// 纯函数（只看 `dir` 与 `spec`）：判据可单测，不必真的起进程。
pub fn sidecar_command_ready(dir: &Path, spec: &SidecarSpec) -> bool {
    let cmd = spec.command.trim();
    if cmd.is_empty() {
        // 空 command 不会通过 `validate_sidecar`；这里按「无约束」处理，不因它误判未就绪
        return true;
    }
    matches!(safe_join(dir, cmd), Some(p) if p.is_file())
}

/// 已装插件是否**就绪**（清单 + 入口 + 声明的 `sidecar.command` 都就位）。判据同 `sidecar_command_ready`。
fn plugin_ready(dir: &Path, sidecar: Option<&SidecarSpec>) -> bool {
    match sidecar {
        Some(s) => sidecar_command_ready(dir, s),
        None => true,
    }
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
            ready: false,
            error: String::new(),
            dependencies: Vec::new(),
            permissions: Vec::new(),
            reuse: Vec::new(),
            provides: Vec::new(),
            render: PluginRenderSpec::default(),
            sidecar: None,
            dev: None,
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
                    item.reuse = m.reuse.clone();
                    item.provides = m.provides.clone();
                    item.render = m.render.clone();
                    item.sidecar = m.sidecar.clone();
                    item.dev = m.dev.clone();
                    match safe_join(&dir, &m.entry) {
                        Some(p) if p.is_file() => {
                            item.entry_path = p.to_string_lossy().to_string();
                            // iframe 引擎额外要求 `ui/index.html`（D.1）；缺了就是坏包，
                            // 不能让用户看到「装上了但一片空白」。
                            if m.render.engine == RenderEngine::Iframe
                                && !dir.join("ui").join("index.html").is_file()
                            {
                                item.error = "render.engine=iframe 但缺少 ui/index.html".into();
                            } else {
                                item.valid = true;
                                // 依赖就位判定（2026-10-15）：见 `plugin_ready`
                                item.ready = plugin_ready(&dir, m.sidecar.as_ref());
                            }
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
    // `provides` 冲突：只 warn，不拒装；按 id 升序第一个作为解析结果（E.1）。
    let mut owners: HashMap<String, String> = HashMap::new();
    for item in &out {
        for key in &item.provides {
            match owners.get(key) {
                None => {
                    owners.insert(key.clone(), item.id.clone());
                }
                Some(first) if first != &item.id => {
                    crate::log::warn(format!(
                        "插件 {} 与 {} 都声明 provides:{key}，按 id 升序取 {}",
                        item.id, first, first
                    ));
                }
                _ => {}
            }
        }
    }
    out
}

/// 按能力身份解析插件目录（E.1）：**精确 id / 目录优先，再按 `provides` 扫**。
///
/// `id = 目录名` 仍是硬契约；这里只解决「按能力找插件」，不改变落点。
pub fn resolve_plugin_dir(root: &Path, key: &str) -> Option<PathBuf> {
    if !is_safe_id(key) {
        return None;
    }
    let exact = root.join(key);
    if exact.is_dir() {
        if let Ok(text) = fs::read_to_string(exact.join(MANIFEST_FILE)) {
            if let Ok(m) = parse_manifest(&text) {
                if m.id == key {
                    return Some(exact);
                }
            }
        }
    }
    list_installed(root)
        .into_iter()
        .find(|item| item.valid && item.provides.iter().any(|p| p == key))
        .map(|item| PathBuf::from(item.dir))
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
    let real_dir = dir
        .canonicalize()
        .map_err(|e| format!("插件目录不可用：{e}"))?;
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
        let p =
            std::env::temp_dir().join(format!("lunac-test-plugins-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// 造一个 zip：`files` 是 (包内路径, 内容)
    fn zip_bytes(files: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
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

    /// 装一份包（不关心依赖进度）—— 生产路径由 commands.rs 传真回调，测试里一律吞掉。
    fn install(bytes: &[u8], root: &Path) -> Result<String, String> {
        install_from_bytes_with_progress(bytes, root, &mut |_| {})
    }

    /// 依赖就位判定（2026-10-15）：**声明了 sidecar 的插件**，其 `sidecar.command` 文件真的在
    /// 才算就绪；没有 sidecar 的插件恒就绪（不误伤那批依赖缺失但界面另有出口的插件）。
    #[test]
    fn sidecar_command_ready_drives_plugin_readiness() {
        let root = tmp_root("ready");
        let dir = root.join("demo");
        fs::create_dir_all(dir.join("bin")).unwrap();

        let spec = |cmd: &str| SidecarSpec {
            command: cmd.to_string(),
            ..Default::default()
        };
        // 声明了 sidecar、可执行文件还没下来 ⇒ 未就绪
        assert!(!sidecar_command_ready(&dir, &spec("bin/llama-server.exe")));
        // 依赖就位 ⇒ 就绪
        fs::write(dir.join("bin").join("llama-server.exe"), b"MZ").unwrap();
        assert!(sidecar_command_ready(&dir, &spec("bin/llama-server.exe")));
        // 越界的 command 一律不就绪（safe_join 拦下）
        assert!(!sidecar_command_ready(&dir, &spec("../evil.exe")));

        // 没有 sidecar 的插件恒就绪 —— 它们不因这条判据被标成未就绪
        let no_side = parse_manifest(&good_manifest("plain")).unwrap();
        assert!(plugin_ready(&dir, no_side.sidecar.as_ref()));

        // 有 sidecar：command 就位 ⇒ 就绪；删掉 ⇒ 立刻未就绪（id/入口都没变）
        let m = parse_manifest(
            r#"{"id":"lm","entry":"index.js","permissions":["process.spawn"],
                "sidecar":{"command":"bin/llama-server.exe"}}"#,
        )
        .unwrap();
        assert!(plugin_ready(&dir, m.sidecar.as_ref()));
        fs::remove_file(dir.join("bin").join("llama-server.exe")).unwrap();
        assert!(!plugin_ready(&dir, m.sidecar.as_ref()));

        let _ = fs::remove_dir_all(&root);
    }

    /// sidecar（2026-10-06，L12 档 1）：**必须与 `process.spawn` 同时出现**，
    /// 且 `command` / `cwd` 不得越出插件目录（fail-closed）。
    #[test]
    fn sidecar_requires_process_spawn_and_stays_in_plugin_dir() {
        let base = r#"{"id":"s","entry":"index.js","permissions":["process.spawn"],
            "sidecar":{"command":"bin/tool.exe","transport":"both","port":0,
            "restart":"on-failure","sha256":"SHA"}}"#;
        // sha256 必须是 64 位十六进制
        assert!(
            parse_manifest(&base.replace("SHA", "ab")).is_err(),
            "sha256 位数不对必须拒"
        );
        let ok = base.replace("SHA", &"a".repeat(64));
        assert!(
            parse_manifest(&ok).is_ok(),
            "合法 sidecar 应通过：{:?}",
            parse_manifest(&ok).err()
        );

        // 缺 process.spawn ⇒ 拒装（不能靠「声明即生效」）
        let no_perm = ok.replace(r#""permissions":["process.spawn"],"#, "");
        assert!(
            parse_manifest(&no_perm).is_err(),
            "sidecar 缺 process.spawn 必须拒装"
        );

        // command 越界 / 绝对路径 ⇒ 拒
        for bad in ["../evil.exe", "C:/Windows/System32/cmd.exe", "/bin/sh"] {
            let t = ok.replace("bin/tool.exe", bad);
            assert!(parse_manifest(&t).is_err(), "command 越界应拒：{bad}");
        }
        // cwd 越界 ⇒ 拒
        let bad_cwd = ok.replace(
            r#""transport":"both""#,
            r#""cwd":"../..","transport":"both""#,
        );
        assert!(parse_manifest(&bad_cwd).is_err(), "cwd 越界应拒");
        // transport / restart 枚举
        assert!(
            parse_manifest(&ok.replace(r#""transport":"both""#, r#""transport":"tcp""#)).is_err()
        );
        assert!(
            parse_manifest(&ok.replace(r#""restart":"on-failure""#, r#""restart":"always""#))
                .is_err()
        );
    }

    /// 资源闸（2026-10-18，E.6）：`memoryLimitMiB` / `processLimit` 的 0 = 不限制，
    /// 非 0 必须落在合理区间（过小会误杀、过大等于没闸）。
    #[test]
    fn sidecar_job_limits_are_positive() {
        let base = r#"{"id":"s","entry":"index.js","permissions":["process.spawn"],
            "sidecar":{"command":"bin/tool.exe","memoryLimitMiB":256,"processLimit":4}}"#;
        let m = parse_manifest(base).unwrap();
        let s = m.sidecar.unwrap();
        assert_eq!(s.memory_limit_mib, 256);
        assert_eq!(s.process_limit, 4);
        // 0 = 不限制（合法，保持旧行为）
        let unlimited = base
            .replace(r#""memoryLimitMiB":256"#, r#""memoryLimitMiB":0"#)
            .replace(r#""processLimit":4"#, r#""processLimit":0"#);
        let s = parse_manifest(&unlimited).unwrap().sidecar.unwrap();
        assert_eq!(s.memory_limit_mib, 0);
        assert_eq!(s.process_limit, 0);
        // 过小 / 过大 / 进程数过多
        for bad in [
            (r#""memoryLimitMiB":256"#, r#""memoryLimitMiB":8"#),
            (r#""memoryLimitMiB":256"#, r#""memoryLimitMiB":65536"#),
            (r#""processLimit":4"#, r#""processLimit":65"#),
        ] {
            assert!(
                parse_manifest(&base.replace(bad.0, bad.1)).is_err(),
                "应被拒：{}",
                bad.1
            );
        }
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
            let text = format!(r#"{{"id":"demo","entry":"{bad}"}}"#);
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
        let bytes = zip_bytes(&[
            ("lunac-plugin.json", &good_manifest("demo")),
            ("index.js", &big),
        ]);
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
        let id = install(&bytes, &root).unwrap();
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
        assert_eq!(install(&bytes2, &root).unwrap(), "demo-pet");
        assert!(fs::read_to_string(root.join("demo-pet").join("index.js"))
            .unwrap()
            .contains("v: 2"));
        // 清单写的入口不存在 ⇒ 拒绝，且**旧版本原样保留**、不留任何残渣
        let bad = zip_bytes(&[("lunac-plugin.json", &good_manifest("demo-pet"))]);
        let err = install(&bad, &root).unwrap_err();
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
        assert_eq!(install(&bytes, &root).unwrap(), "lunac-pet");
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
        fs::write(
            root.join("half-installed").join(MANIFEST_FILE),
            "{ not json",
        )
        .unwrap();
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
        let out =
            parse_index(r#"[{"id":"solo","name":"Solo","url":"https://e.com/a.zip"}]"#).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].description.is_empty() && out[0].version.is_empty());
    }

    /// 插件包 sha256（2026-10-18，E.5）：索引里的哈希要么合法、要么整条丢掉；
    /// 安装前校验必须真的逐字节比对（大小写不敏感），空期望 = 迁移期不校验。
    #[test]
    fn index_sha256_is_validated_and_packages_are_verified() {
        let good = "a".repeat(64);
        let entry = format!(
            r#"[{{"id":"solo","name":"Solo","url":"https://e.com/a.zip","sha256":"{good}"}}]"#
        );
        assert_eq!(parse_index(&entry).unwrap().len(), 1);
        // 写错长度 / 非十六进制 ⇒ 这条不进入市场（不把「校验过」变成假话）
        let bad = r#"[{"id":"solo","name":"Solo","url":"https://e.com/a.zip","sha256":"abc"}]"#;
        assert!(parse_index(bad).unwrap().is_empty());
        let bad_char = format!(
            r#"[{{"id":"solo","name":"Solo","url":"https://e.com/a.zip","sha256":"{}"}}]"#,
            "z".repeat(64)
        );
        assert!(parse_index(&bad_char).unwrap().is_empty());

        // 空期望 = 迁移期：不校验（调用方 warn），不是「校验失败」
        assert!(!verify_package_sha256(b"abc", "").unwrap());
        // 实算一致（大小写不敏感）；差一个字节就拒
        let h = hex_sha256(b"abc");
        assert!(verify_package_sha256(b"abc", &h).unwrap());
        assert!(verify_package_sha256(b"abc", &h.to_uppercase()).unwrap());
        assert!(verify_package_sha256(b"abd", &h).is_err());
        // 期望值本身写错 ⇒ 明确报格式错误，而不是伪装成「不匹配」
        assert!(verify_package_sha256(b"abc", "not-a-hash").is_err());
    }

    /// 依赖字段（2026-09-28）：好条目过、坏条目整包拒。
    #[test]
    fn manifest_validates_dependencies() {
        let ok = r#"{"id":"music","entry":"index.js","dependencies":[
            {"type":"file","url":"https://e.com/librespot.exe","dest":"bin/librespot.exe","sha256":"AB12"},
            {"type":"npm","package":"@scope/pkg","version":"^2.0.0"},
            {"url":"https://e.com/model.bin","dest":"model.bin"},
            {"type":"archive","url":"https://e.com/engine.7z","dest":"paddle-ocr","sha256":"CD34"}
        ]}"#;
        let m = parse_manifest(ok).unwrap();
        assert_eq!(m.dependencies.len(), 4);
        assert!(!m.dependencies[0].is_npm());
        assert!(m.dependencies[1].is_npm());
        assert!(m.dependencies[3].is_archive());
        // archive 的 dest 一律按**目录**看：`paddle-ocr` 这种单段路径合法，越界的照样拒
        assert!(validate_dependency(&m.dependencies[3]).is_ok());

        // 逐条坏：明文 http / dest 越界 / 缺 dest / npm 缺包名 / 未知 type / archive 越界
        let bad = [
            r#"[{"url":"http://e.com/a.exe","dest":"a.exe"}]"#,
            r#"[{"url":"https://e.com/a.exe","dest":"../a.exe"}]"#,
            r#"[{"url":"https://e.com/a.exe"}]"#,
            r#"[{"type":"npm"}]"#,
            r#"[{"type":"npm","package":"--registry=http://evil"}]"#,
            r#"[{"type":"zip","url":"https://e.com/a.zip"}]"#,
            r#"[{"type":"archive","url":"https://e.com/a.7z","dest":"../escape"}]"#,
            r#"[{"type":"archive","url":"http://e.com/a.7z","dest":"engine"}]"#,
        ];
        for b in bad {
            let text = format!(r#"{{"id":"demo","entry":"index.js","dependencies":{b}}}"#);
            assert!(parse_manifest(&text).is_err(), "应被拒：{b}");
        }
    }

    /// `archive` 形态的识别与摊平（2026-09-30）：魔数说了算 + 唯一顶层目录才摊平。
    #[test]
    fn archive_dependency_detects_format_and_flattens_single_root() {
        // 魔数：7z 与 zip 各认一份，别的（含空）一律拒
        assert_eq!(
            detect_archive_format(&[0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c, 0x00]).unwrap(),
            ArchiveFormat::SevenZ
        );
        assert_eq!(
            detect_archive_format(&[0x50, 0x4b, 0x03, 0x04, 0x00]).unwrap(),
            ArchiveFormat::Zip
        );
        assert!(detect_archive_format(b"not an archive").is_err());
        assert!(detect_archive_format(&[]).is_err());

        // 唯一顶层目录 ⇒ 摊平；只要掺一个文件或多一个目录就不摊
        let root = tmp_root("flatten");
        let single = root.join("single");
        fs::create_dir_all(single.join("PaddleOCR-json_v1.4.1")).unwrap();
        assert_eq!(
            flatten_single_top_dir(&single),
            single.join("PaddleOCR-json_v1.4.1")
        );
        fs::write(single.join("stray.txt"), "x").unwrap();
        assert_eq!(flatten_single_top_dir(&single), single);
        let two = root.join("two");
        fs::create_dir_all(two.join("a")).unwrap();
        fs::create_dir_all(two.join("b")).unwrap();
        assert_eq!(flatten_single_top_dir(&two), two);
        let _ = fs::remove_dir_all(&root);
    }

    /// `archive` 依赖走的是**与插件包同一套**穿越判据：zip 里的 `../evil` 必须在写盘前被拦下。
    #[test]
    fn archive_extraction_refuses_entries_outside_the_staging_dir() {
        let root = tmp_root("dep-archive");
        let staging = root.join("staging");
        let bytes = zip_bytes(&[("../evil.exe", "MZ"), ("engine.exe", "MZ")]);
        let err = extract_archive(&bytes, ArchiveFormat::Zip, &staging).unwrap_err();
        assert!(err.contains("越界路径"), "{err}");
        assert!(!root.join("evil.exe").exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// 声明的宿主能力（2026-09-28）：合法的过、格式非法的整包拒、**不认识的只 warn 不拒**。
    #[test]
    fn manifest_validates_permissions() {
        let ok =
            r#"{"id":"ocr","entry":"index.js","permissions":["layout.takeover","window.resize"]}"#;
        let m = parse_manifest(ok).unwrap();
        assert_eq!(m.permissions.len(), 2);
        // 不认识的名字（更新版宿主的能力）**不影响安装**
        let fwd = r#"{"id":"demo","entry":"index.js","permissions":["some.future.cap"]}"#;
        assert!(parse_manifest(fwd).is_ok());
        // 格式非法 / 写不下：整包拒
        for bad in [
            r#"[""]"#,
            r#"["UPPER"]"#,
            r#"["has space"]"#,
            r#"["bad/slash"]"#,
        ] {
            let text = format!(r#"{{"id":"demo","entry":"index.js","permissions":{bad}}}"#);
            assert!(parse_manifest(&text).is_err(), "应被拒：{bad}");
        }
        let many = (0..(MAX_PERMISSIONS + 1))
            .map(|i| format!("\"cap{i}\""))
            .collect::<Vec<_>>()
            .join(",");
        let text = format!(r#"{{"id":"demo","entry":"index.js","permissions":[{many}]}}"#);
        assert!(parse_manifest(&text).is_err());
    }

    /// `reuse`（2026-10-05，复用基础插件监听）：合法 id 过、形状非法 / 条数超限整包拒。
    #[test]
    fn manifest_validates_reuse() {
        let ok = r#"{"id":"demo","entry":"index.js","reuse":["settings","quick-launch"]}"#;
        let m = parse_manifest(ok).unwrap();
        assert_eq!(
            m.reuse,
            vec!["settings".to_string(), "quick-launch".to_string()]
        );
        // 不写这一段 = 不复用（合法）
        assert!(parse_manifest(r#"{"id":"demo","entry":"index.js"}"#)
            .unwrap()
            .reuse
            .is_empty());
        // 形状非法：整包拒
        for bad in [r#"["UPPER"]"#, r#"["has space"]"#, r#"["bad/slash"]"#] {
            let text = format!(r#"{{"id":"demo","entry":"index.js","reuse":{bad}}}"#);
            assert!(parse_manifest(&text).is_err(), "应被拒：{bad}");
        }
        let many = (0..(MAX_REUSE + 1))
            .map(|i| format!("\"p{i}\""))
            .collect::<Vec<_>>()
            .join(",");
        let text = format!(r#"{{"id":"demo","entry":"index.js","reuse":[{many}]}}"#);
        assert!(parse_manifest(&text).is_err());
    }

    /// `provides` 与 `render`（2026-10-18，插件框架重构 E.1 / E.3）：
    /// 缺省 = 空身份 + dom/inherit；形状非法 / 未知枚举整包拒。
    #[test]
    fn manifest_validates_provides_and_render() {
        let base = parse_manifest(r#"{"id":"demo","entry":"index.js"}"#).unwrap();
        assert!(base.provides.is_empty());
        assert_eq!(base.render.engine, RenderEngine::Dom);
        assert_eq!(base.render.appearance, RenderAppearance::Inherit);

        let ok = r#"{"id":"demo","entry":"index.js","provides":["comic.reader","novel.reader"],
            "render":{"engine":"iframe","appearance":"custom"}}"#;
        let m = parse_manifest(ok).unwrap();
        assert_eq!(m.provides.len(), 2);
        assert_eq!(m.render.engine, RenderEngine::Iframe);
        assert_eq!(m.render.appearance, RenderAppearance::Custom);

        for bad in [r#"["UPPER"]"#, r#"["has space"]"#, r#"["bad/slash"]"#] {
            let text = format!(r#"{{"id":"demo","entry":"index.js","provides":{bad}}}"#);
            assert!(parse_manifest(&text).is_err(), "应被拒：{bad}");
        }
        let many = (0..(MAX_PROVIDES + 1))
            .map(|i| format!("\"p{i}\""))
            .collect::<Vec<_>>()
            .join(",");
        let text = format!(r#"{{"id":"demo","entry":"index.js","provides":[{many}]}}"#);
        assert!(parse_manifest(&text).is_err());

        assert!(
            parse_manifest(r#"{"id":"demo","entry":"index.js","render":{"engine":"native"}}"#)
                .is_err()
        );
        assert!(parse_manifest(
            r#"{"id":"demo","entry":"index.js","render":{"appearance":"fancy"}}"#
        )
        .is_err());
    }

    /// 常驻锚点（2026-10-18，C.4.1）：`render.slots` 只支持 iframe 引擎；
    /// 槽位名是枚举（未知值直接拒），重复声明也拒。
    #[test]
    fn manifest_validates_render_slots() {
        let m = parse_manifest(
            r#"{"id":"demo","entry":"index.js",
                "render":{"engine":"iframe","slots":["chat-sidebar","settings-section"]}}"#,
        )
        .unwrap();
        assert_eq!(
            m.render.slots,
            vec![RenderSlot::ChatSidebar, RenderSlot::SettingsSection]
        );
        // 缺省 = 不挂常驻锚点
        let base =
            parse_manifest(r#"{"id":"demo","entry":"index.js","render":{"engine":"iframe"}}"#)
                .unwrap();
        assert!(base.render.slots.is_empty());
        // DOM 引擎无法把插件限制在槽位内 ⇒ 整包拒
        assert!(parse_manifest(
            r#"{"id":"demo","entry":"index.js","render":{"slots":["chat-sidebar"]}}"#
        )
        .is_err());
        // 未知槽位 / 重复声明
        assert!(parse_manifest(
            r#"{"id":"demo","entry":"index.js","render":{"engine":"iframe","slots":["left-panel"]}}"#
        )
        .is_err());
        assert!(parse_manifest(
            r#"{"id":"demo","entry":"index.js","render":{"engine":"iframe","slots":["chat-sidebar","chat-sidebar"]}}"#
        )
        .is_err());
    }

    /// 开发工具链（2026-10-18，E.4）：https + 必填 sha256 + 落点不越界；
    /// 指纹对 profile / build 的任一变化都敏感。
    #[test]
    fn manifest_validates_dev_toolchain_and_build() {
        let hash = "a".repeat(64);
        let ok = format!(
            r#"{{"id":"demo","entry":"index.js","dev":{{
                "toolchains":[{{"id":"node","url":"https://e.com/node.zip","sha256":"{hash}",
                    "dest":".toolchain/node","kind":"archive","version":"20.11.1","binDir":".toolchain/node"}}],
                "build":{{"command":"npm","args":["run","build"],"cwd":""}}}}}}"#
        );
        let m = parse_manifest(&ok).unwrap();
        let dev = m.dev.expect("dev 段应被解析");
        assert_eq!(dev.toolchains.len(), 1);
        assert_eq!(dev.toolchains[0].version, "20.11.1");
        let fp = dev_fingerprint("demo", &dev);
        assert_eq!(fp.len(), 64);
        // 任一字段变化 ⇒ 指纹变化（旧授权自动失效）
        let mut changed = dev.clone();
        changed.toolchains[0].version = "20.12.0".into();
        assert_ne!(fp, dev_fingerprint("demo", &changed));
        let mut changed_build = dev.clone();
        changed_build
            .build
            .as_mut()
            .unwrap()
            .args
            .push("--release".into());
        assert_ne!(fp, dev_fingerprint("demo", &changed_build));

        for bad in [
            // 缺哈希
            r#"{"id":"demo","entry":"index.js","dev":{"toolchains":[{"id":"n","url":"https://e.com/n.zip","dest":"n"}]}}"#.to_string(),
            // 明文 http
            format!(r#"{{"id":"demo","entry":"index.js","dev":{{"toolchains":[{{"id":"n","url":"http://e.com/n.zip","sha256":"{hash}","dest":"n"}}]}}}}"#),
            // dest 越界
            format!(r#"{{"id":"demo","entry":"index.js","dev":{{"toolchains":[{{"id":"n","url":"https://e.com/n.zip","sha256":"{hash}","dest":"../n"}}]}}}}"#),
            // dest 不在 .toolchain\ 下（否则卸载无法回收）
            format!(r#"{{"id":"demo","entry":"index.js","dev":{{"toolchains":[{{"id":"n","url":"https://e.com/n.zip","sha256":"{hash}","dest":"bin/n"}}]}}}}"#),
            // 构建命令越界
            format!(r#"{{"id":"demo","entry":"index.js","dev":{{"build":{{"command":"../../evil.exe"}}}}}}"#),
            // 不在白名单的裸命令
            format!(r#"{{"id":"demo","entry":"index.js","dev":{{"build":{{"command":"calc"}}}}}}"#),
        ] {
            assert!(parse_manifest(&bad).is_err(), "应被拒：{bad}");
        }
    }

    /// iframe 引擎必须有 `ui/index.html`（D.1）；缺了就是坏包，不能放行。
    #[test]
    fn iframe_engine_requires_ui_entry() {
        let root = tmp_root("iframe-entry");
        let dir = root.join("demo");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(MANIFEST_FILE),
            r#"{"id":"demo","entry":"index.js","render":{"engine":"iframe"}}"#,
        )
        .unwrap();
        fs::write(dir.join("index.js"), "export default {}").unwrap();

        let before = list_installed(&root);
        assert_eq!(before.len(), 1);
        assert!(!before[0].valid);
        assert!(
            before[0].error.contains("ui/index.html"),
            "{}",
            before[0].error
        );

        fs::create_dir_all(dir.join("ui")).unwrap();
        fs::write(dir.join("ui").join("index.html"), "<!doctype html>").unwrap();
        let after = list_installed(&root);
        assert!(after[0].valid, "{}", after[0].error);
        assert_eq!(after[0].render.engine, RenderEngine::Iframe);
        let _ = fs::remove_dir_all(&root);
    }

    /// `provides` 能力定位（E.1）：精确 id 优先；否则按 id 升序取第一个声明者。
    #[test]
    fn resolve_plugin_dir_prefers_id_then_provides() {
        let root = tmp_root("provides-resolve");
        for (id, provides) in [("demo-a", "comic.reader"), ("demo-b", "comic.reader")] {
            let dir = root.join(id);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join(MANIFEST_FILE),
                format!(r#"{{"id":"{id}","entry":"index.js","provides":["{provides}"]}}"#),
            )
            .unwrap();
            fs::write(dir.join("index.js"), "export default {}").unwrap();
        }
        assert_eq!(
            resolve_plugin_dir(&root, "comic.reader"),
            Some(root.join("demo-a"))
        );
        assert_eq!(
            resolve_plugin_dir(&root, "demo-b"),
            Some(root.join("demo-b"))
        );
        assert_eq!(resolve_plugin_dir(&root, "../escape"), None);
        assert_eq!(resolve_plugin_dir(&root, "missing.key"), None);
        let _ = fs::remove_dir_all(&root);
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
        // 缺省：`alwaysOnTop` 没写 ⇒ `None`（由宿主 `default_always_on_top` 按窗口 id 决定，
        // 只有桌宠模型窗默认置顶）
        assert_eq!(s.always_on_top, None);
        // 清单显式写值时优先于宿主默认（两个方向都能覆盖）
        let off = r#"{"id":"pet","entry":"index.js","window":{"alwaysOnTop":false}}"#;
        assert_eq!(
            parse_manifest(off).unwrap().window.unwrap().always_on_top,
            Some(false)
        );
        let on = r#"{"id":"memo","entry":"index.js","window":{"alwaysOnTop":true}}"#;
        assert_eq!(
            parse_manifest(on).unwrap().window.unwrap().always_on_top,
            Some(true)
        );

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
        let err =
            install_dependencies_with_progress(&plugin, "demo", &[dep], &mut |_| {}).unwrap_err();
        assert!(err.contains("越界"), "{err}");
        assert!(!root.join("escape.exe").exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// 安装单飞闸（2026-10-27）：同一时刻只允许一次安装；drop 后必须能再次占用。
    /// 这条闸是「两种进度 / 秒完成但没装完」的根因修复 —— 没有它，暂停后再点一次安装
    /// 会起第二个下载线程，两个线程写同一个 `.part`。
    #[test]
    fn install_gate_is_exclusive_and_released_on_drop() {
        assert!(!install_busy(), "测试开始前闸应是空的");
        let guard = try_begin_install("测试").expect("第一次应能占用");
        let err = try_begin_install("测试").expect_err("第二次应被拒");
        assert!(err.contains("已有一次安装在进行"), "{err}");
        drop(guard);
        assert!(!install_busy(), "drop 后必须释放");
    }

    /// 探针插件（`scripts/probe-plugin/`，§待办 3-10 的 dev 自检件）必须**始终符合清单规范**：
    /// 它是 iframe / 两个常驻锚点的验收夹具，规范一变（引擎枚举 / 槽位名 / iframe 必须有
    /// `ui/index.html`）这里先红，避免「夹具过时 ⇒ 验收结论不可信」。
    #[test]
    fn probe_plugin_fixture_matches_the_manifest_rules() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/probe-plugin");
        let text = fs::read_to_string(root.join(MANIFEST_FILE))
            .expect("读不到 scripts/probe-plugin/lunac-plugin.json");
        let m = parse_manifest(&text).expect("探针清单应通过校验");
        assert_eq!(m.id, "probe");
        assert_eq!(m.render.engine, RenderEngine::Iframe);
        assert_eq!(
            m.render.slots,
            vec![RenderSlot::ChatSidebar, RenderSlot::SettingsSection]
        );
        assert!(root.join(&m.entry).is_file(), "清单声明的入口必须存在");
        assert!(
            root.join("ui").join("index.html").is_file(),
            "iframe 引擎必须有 ui/index.html"
        );
    }

    /// 断点文件名（2026-10-27，可续传）：同 url + 同清单哈希 ⇒ 恒定；
    /// 清单哈希变了 ⇒ 换名字（旧断点自动作废，不会被拼进新版本）。
    #[test]
    fn part_path_is_stable_per_url_and_manifest_hash() {
        let a = part_path_for("https://example.com/a.zip", "AA");
        let b = part_path_for("https://example.com/a.zip", "aa");
        let c = part_path_for("https://example.com/a.zip", "BB");
        let d = part_path_for("https://example.com/b.zip", "AA");
        assert_eq!(a, b, "大小写不同的同一哈希应指向同一断点");
        assert_ne!(a, c, "清单哈希变了 ⇒ 旧断点不能接着用");
        assert_ne!(a, d, "url 变了 ⇒ 另一个断点");
        assert!(a.to_string_lossy().ends_with(".part"));
        assert!(a.parent().unwrap().ends_with("downloads"));
    }

    /// 断点元数据落盘 / 读回（2026-10-27）：重启后靠它知道「这个 .part 属于谁、下到哪」；
    /// 老文件没有 `id` 字段也必须能读（`#[serde(default)]`）。
    #[test]
    fn part_meta_roundtrips_and_tolerates_old_files() {
        let root = tmp_root("part-meta");
        let p = root.join("x.part.json");
        write_part_meta(
            &p,
            &PartMeta {
                url: "https://e/x".into(),
                id: "pet".into(),
                etag: "W/\"1\"".into(),
                total: 10,
                offset: 4,
                sha256: "AB".into(),
            },
        );
        let m = read_part_meta(&p).expect("应能读回");
        assert_eq!(
            (m.url.as_str(), m.id.as_str(), m.total, m.offset),
            ("https://e/x", "pet", 10, 4)
        );
        fs::write(
            &p,
            r#"{"url":"https://e/y","etag":"","total":0,"offset":0,"sha256":""}"#,
        )
        .unwrap();
        let old = read_part_meta(&p).expect("老断点也要能读");
        assert_eq!(old.id, "");
        let _ = fs::remove_dir_all(&root);
    }
}
