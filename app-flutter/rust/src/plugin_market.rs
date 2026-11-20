//! 插件市场（v2）：拉远端索引 / 下载 https zip / sha256 / 解压到 `<exe 根>/Modules/<id>` / 卸载。
//!
//! 落点与 v1 **完全一致**（`<exe 根>/Modules/<id>/`），所以两代应用看到的是同一份已装插件。
//! 校验判据也照搬 v1 `app/src-tauri/src/plugin_market.rs`（这里只保留市场本身需要的那部分）：
//!   · 只收 https，且拒绝 loopback / 内网字面量 —— 请求由宿主代发，不能变成绕过 CSP 的旁路；
//!   · 压缩包（32MB）与解压后总量（192MB）都有上限（zip bomb）；
//!   · 逐条拒绝绝对路径 / `..` / 空段 / 超长路径（路径穿越）；
//!   · `id` 只允许安全字符（它直接当目录名）；
//!   · 同 id 视为「重装 / 升级」：旧目录先改名备份，落盘任一步失败就搬回来；
//!   · 先解到 `.staging-*` 再改名进正式目录，失败即清理。
//!
//! **与 v1 的差异（如实登记）**：
//!   · 依赖支持 `file` / `archive`（zip / 7z，按魔数识别）/ `npm` 三种形态，与 v1 同口径；
//!   · 下载可**暂停 / 继续 / 断点续传**：半截文件放 `<exe 根>/temp/downloads/*.part` +
//!     `.part.json`，重发 `Range` / `If-Range` 接上；关面板 / 强杀应用后仍能接着下
//!     （v1 `fetch_resumable` 的移植，见 `docs/todo.md` §待办 3-18）；
//!   · 安装进度不再走事件推送（v2 没有插件事件总线）：`install_state()` 给一份快照，
//!     Dart 侧 500ms 轮询 —— 语义与 v1 `plugin_install_state` 完全一致。
//!
//! 安全边界与 v1 同口径：这里防的是**事故**（写坏路径、把盘塞满），不是防恶意 ——
//! 插件是用户自己选择安装的可执行代码，装上就等同本机权限。

use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 插件清单文件名（可放在包根，也可放在 zip 里的单层子目录下）。
pub const MANIFEST_FILE: &str = "lunac-plugin.json";
/// 远端索引（与 v1 同一个地址）。
pub const INDEX_URL: &str =
    "https://raw.githubusercontent.com/LythrumMoon/lunac-plugins/main/index.json";
/// 索引正文上限（当前只有几百字节，1MB 已极宽松）。
pub const MAX_INDEX_BYTES: u64 = 1024 * 1024;
/// 索引条目数上限。
pub const MAX_INDEX_ENTRIES: usize = 500;
/// 插件包大小上限。
pub const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024;
/// 解压后总量上限（zip bomb 防护）。
pub const MAX_TOTAL_BYTES: u64 = 192 * 1024 * 1024;
/// 包内条目数上限。
pub const MAX_ENTRIES: usize = 4000;
/// 单文件依赖体积上限（流式落盘，不进内存）。
pub const MAX_DEP_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// 依赖条数上限。
pub const MAX_DEP_ENTRIES: usize = 32;
/// `archive` 依赖**解压后**总量上限（1 GiB）。比插件包那档（192 MB）宽：
/// 这条走的是「引擎 + 模型」，按 192 MB 卡会让它永远装不上（v1 同值）。
pub const MAX_DEP_EXTRACT_BYTES: u64 = 1024 * 1024 * 1024;
/// `archive` 依赖的条目数上限（模型目录动辄上千个小文件，比插件包那档放宽）。
pub const MAX_DEP_EXTRACT_ENTRIES: usize = 20000;
/// 声明的宿主能力条数上限（只做提示，不认识的名字不拒装）。
pub const MAX_PERMISSIONS: usize = 16;
/// id 长度上限。
const MAX_ID_CHARS: usize = 64;

// ── 数据结构 ──────────────────────────────────────────────────────

/// 市场索引里的一条：只描述「去哪下」，包的真相在包自己的清单里。
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
    /// https 的插件 zip 地址。
    pub url: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub homepage: String,
    /// 插件 zip 的 sha256（64 位十六进制，大小写不敏感）。空 = 迁移期不校验。
    #[serde(default)]
    pub sha256: String,
}

/// 包内清单（字段与 v1 对齐；v2 只用得到这些）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PluginManifest {
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
    /// ESM 入口（相对清单所在目录）。缺省 `index.js`。
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub dependencies: Vec<PluginDependency>,
    #[serde(default)]
    pub permissions: Vec<String>,
    /// 插件自带的本机进程（v1 `SidecarSpec` 的同款契约，2026-11-15 移植）。
    ///
    /// **必须与 `permissions: ["process.spawn"]` 同时出现**：能 spawn 任意二进制的
    /// 插件能力无上界，不能靠「声明即生效」，要过独立信任门
    /// （`config/plugin-trusted.json`，指纹 = 插件 id + command + sha256）。
    /// 这就是「AI 开发的插件可以用任何语言 / 框架」的落点：插件把自己的进程交给宿主管，
    /// UI 由宿主画（v2 不加载第三方 WebView）。
    #[serde(default)]
    pub sidecar: Option<SidecarSpec>,
    /// 开发期能力声明（轨道 H，`docs/todo.md` §A.2）。
    ///
    /// **不随安装自动执行**：`toolchains[].install` 与 `build.command` 都只在用户
    /// 授权后才运行，且 `build` **永不自动运行**。卸载插件 = 删目录 ⇒ `.toolchain\` 一起回收。
    #[serde(default)]
    pub dev: Option<DevSpec>,
}

/// `dev` 段（轨道 H）：插件声明「编译 / 打包自己需要什么」。
///
/// 字段与 `agent-templates/modules/README.md` §开发工具链**逐字一致**（那是插件作者看的
/// 契约，不能在这里另立一套）。这是**声明**，不是授权 —— 授权记录只有一份
/// (`config/plugin-toolchain.json`，由 `plugin_toolchain.rs` 读写）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct DevSpec {
    #[serde(default)]
    pub toolchains: Vec<ToolchainProfile>,
    #[serde(default)]
    pub build: Option<BuildSpec>,
}

/// 一个**可下载**的工具链 profile（装到 `dest`，`dest` 必须在 `.toolchain\` 下）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainProfile {
    /// 同时是 `.toolchain\<id>.json` 标记的名字；合法字符同插件 id（不许重复）。
    #[serde(default)]
    pub id: String,
    /// 仅用于展示与指纹（可空）。
    #[serde(default)]
    pub version: String,
    /// **必须 https**。
    #[serde(default)]
    pub url: String,
    /// **必填**：工具链是可执行代码，没有哈希就不发。
    #[serde(default)]
    pub sha256: String,
    /// `file`（单个文件，缺省）/ `archive`（解压到 `dest`）。
    #[serde(default)]
    pub kind: String,
    /// 相对插件目录的落点，**必须在 `.toolchain\` 下**。
    #[serde(default)]
    pub dest: String,
    /// 装完后加进构建 `PATH` 的目录（相对插件目录，**也必须在 `.toolchain\` 下**）。
    #[serde(default)]
    pub bin_dir: String,
}

/// 构建声明：`command` 是**包内相对路径**或**白名单裸命令**；参数不经 shell。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct BuildSpec {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// 工作目录（相对插件目录，不得越界；空 = 插件目录）。
    #[serde(default)]
    pub cwd: String,
}

/// 裸命令白名单（**逐字取 README §开发工具链**）。
///
/// 只用于「`build.command` 不带路径」的情形；带 `/` / `\` / 以 `.` 开头的一律按
/// 包内相对路径解析。**名单只有这一处**：校验与执行都读它。
pub const DEV_BARE_COMMANDS: [&str; 15] = [
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
/// `dev.toolchains` 条数上限（README：最多 2 条）。
pub const MAX_DEV_PROFILES: usize = 2;
/// `build.args` 条数上限。
pub const MAX_DEV_BUILD_ARGS: usize = 32;
/// 单个构建参数的字符数上限。
pub const MAX_DEV_ARG_CHARS: usize = 1024;

/// 插件自带进程的声明（字段名与 v1 完全一致，老清单可直接用）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SidecarSpec {
    /// 可执行文件路径（**相对插件目录**，不许绝对路径 / 不许含 `..`）。
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
    /// 消息协议：`""` / `ndjson`（缺省，唯一支持）。
    #[serde(default)]
    pub protocol: String,
    /// 工作目录（相对插件目录，**不得越界**）。缺省 = 插件目录。
    #[serde(default)]
    pub cwd: String,
    /// 附加环境变量（在宿主环境之上**只增不删**）。
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// 宿主加载插件时是否自动起（v2 目前只在面板里手动起；字段原样保留）。
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
    /// 就绪超时（秒，缺省 10）。
    #[serde(default = "default_ready_timeout_sec")]
    pub ready_timeout_sec: u64,
    /// 单进程内存上限（MiB，0 = 不限制）；用独立 Job Object 生效。
    ///
    /// 键名沿用 v1 的 `memoryLimitMiB`（**大写 B**），不能用默认 camelCase 推导。
    #[serde(default, rename = "memoryLimitMiB")]
    pub memory_limit_mib: u32,
    /// job 内最大进程数（0 = 不限制）。
    #[serde(default)]
    pub process_limit: u32,
}

fn default_ready_timeout_sec() -> u64 {
    10
}

/// `sidecar.readyTimeoutSec` 上限（1 小时；够任何本地服务冷启动）。
const MAX_SIDECAR_READY_TIMEOUT_SEC: u64 = 3600;
/// `sidecar.env` 条数上限。
const MAX_SIDECAR_ENV: usize = 32;
/// `sidecar.allowLocalPorts` 条数上限。
const MAX_SIDECAR_ALLOW_PORTS: usize = 16;
/// sidecar 内存上限的合法区间（MiB）：0 = 不限制；非 0 至少 32 MiB，最大 32 GiB。
const MIN_SIDECAR_MEMORY_MIB: u32 = 32;
const MAX_SIDECAR_MEMORY_MIB: u32 = 32768;
/// sidecar job 内进程数上限（0 = 不限制；非 0 最多 64）。
const MAX_SIDECAR_PROCESS_LIMIT: u32 = 64;

/// 已装插件里透出的 sidecar 摘要（面板据此显示「可运行」与信任卡内容）。
#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SidecarSummary {
    pub command: String,
    pub args: Vec<String>,
    pub transport: String,
    pub port: u16,
    pub autostart: bool,
    pub restart: String,
    pub sha256: String,
    pub allow_local_ports: Vec<u16>,
    pub allow_local_any: bool,
    pub ready_timeout_sec: u64,
    /// 与 v1 同键名（大写 B）。
    #[serde(rename = "memoryLimitMiB")]
    pub memory_limit_mib: u32,
    pub process_limit: u32,
}

impl From<&SidecarSpec> for SidecarSummary {
    fn from(spec: &SidecarSpec) -> Self {
        SidecarSummary {
            command: spec.command.clone(),
            args: spec.args.clone(),
            transport: spec.transport.clone(),
            port: spec.port,
            autostart: spec.autostart,
            restart: spec.restart.clone(),
            sha256: spec.sha256.clone(),
            allow_local_ports: spec.allow_local_ports.clone(),
            allow_local_any: spec.allow_local_any,
            ready_timeout_sec: spec.ready_timeout_sec,
            memory_limit_mib: spec.memory_limit_mib,
            process_limit: spec.process_limit,
        }
    }
}

fn default_entry() -> String {
    "index.js".to_string()
}

/// 一条依赖。`type` 缺省 = `file`。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PluginDependency {
    /// `file`（缺省）/ `archive` / `npm`
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub package: String,
    #[serde(default)]
    pub version: String,
    /// `file`：相对插件目录的落点；`archive`：解压目标目录。
    #[serde(default)]
    pub dest: String,
    #[serde(default)]
    pub sha256: String,
}

impl PluginDependency {
    fn kind_or_file(&self) -> &str {
        let kind = self.kind.trim();
        if kind.is_empty() {
            "file"
        } else {
            kind
        }
    }
}

/// 已装插件（返回给 Dart 的形状）。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPlugin {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub icon: String,
    pub homepage: String,
    pub entry: String,
    /// 插件目录绝对路径。
    pub dir: String,
    /// 入口文件绝对路径；坏包为空。
    pub entry_path: String,
    /// 清单与入口都通过校验。
    pub valid: bool,
    /// `valid = false` 时的原因（原样显示，不猜）。
    pub error: String,
    /// 清单声明的依赖条数（界面提示「它要拉什么」）。
    pub deps: usize,
    /// 清单声明的宿主能力（原样列出）。
    pub permissions: Vec<String>,
    /// 清单声明了 `sidecar`（= 插件带自己的本机进程，可用任何语言写）。
    pub has_sidecar: bool,
    /// sidecar 摘要；没声明时为 null。
    pub sidecar: Option<SidecarSummary>,
}

// ── 路径与 id ────────────────────────────────────────────────────

/// 插件根目录：`<exe 根>/Modules`（与 v1 同名同位置）。
pub fn plugins_dir(root: &Path) -> PathBuf {
    root.join("Modules")
}

/// id 合法性 —— 它直接当目录名用，所以只允许安全字符。
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

/// 入口必须是相对路径、以 `.js` / `.mjs` 结尾、不含 `..`。
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

/// 把 zip 里的条目名规范化成 root 下的路径；任何越界都返回 None。
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

/// 只允许公网 https：拒绝明文 http、IP 字面量、localhost / 内网后缀。
pub fn validate_public_https_url(raw: &str) -> Result<(), String> {
    let url = raw.trim();
    let Some(rest) = url.strip_prefix("https://") else {
        return Err("只允许 https 地址".into());
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("");
    let host = if let Some(inner) = authority.strip_prefix('[') {
        inner.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    let host = host.trim();
    if host.is_empty() {
        return Err("地址缺少主机名".into());
    }
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Err(format!("不允许 IP 字面量地址（含 loopback / 内网）：{host}"));
    }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".internal")
        || lower.ends_with(".local")
        || lower.ends_with(".lan")
        || lower.ends_with(".home")
    {
        return Err(format!("不允许本机 / 内网主机名：{host}"));
    }
    Ok(())
}

// ── 校验 ─────────────────────────────────────────────────────────

/// 是不是一个合法的 sha256 十六进制串（64 位）。
pub fn is_sha256_hex(s: &str) -> bool {
    let s = s.trim();
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(64);
    for b in Sha256::digest(bytes) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 清单指纹（轨道 H 的授权单位）：清单原文的 sha256（去掉 BOM）。
///
/// 任何字段变更（哪怕只改一行描述）都会换指纹 ⇒ 旧授权自动失效、必须重新确认。
#[allow(dead_code)] // 保留给「清单指纹」诊断；当前安装路径直接比对解析后的字段。
pub fn manifest_fingerprint(text: &str) -> String {
    hex_sha256(text.trim_start_matches('\u{feff}').as_bytes())
}

/// `dev` 段里的相对路径规范化：允许 `./` 前缀与 `.`（README 的示例就写 `"cwd": "."`），
/// 其余仍过 [`safe_join`]（挡绝对路径 / `..` / 盘符）。
///
/// 空路径（或 `.`）返回**空 PathBuf** —— 语义是「插件目录本身」。
pub fn dev_relative(raw: &str) -> Option<PathBuf> {
    let text = raw.trim().replace('\\', "/");
    let stripped = text.trim_start_matches("./");
    if stripped.is_empty() || stripped == "." {
        return Some(PathBuf::new());
    }
    safe_join(Path::new("."), stripped)
}

/// 校验下载到的插件包：`expected` 为空 = 迁移期不校验（Ok(false)）。
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

/// 解析并逐条校验市场索引。坏条目只丢自己（一条写错的 URL 不该让整个市场变空）。
pub fn parse_index(text: &str) -> Result<Vec<PluginIndexEntry>, String> {
    let text = text.trim_start_matches('\u{feff}');
    let raw: Vec<PluginIndexEntry> =
        serde_json::from_str(text).map_err(|e| format!("插件索引不是合法 JSON：{e}"))?;
    let mut out: Vec<PluginIndexEntry> = Vec::new();
    for e in raw {
        if !is_safe_id(&e.id) {
            eprintln!("[lunac-market] 索引里有一条 id 非法，已跳过：{}", e.id);
            continue;
        }
        if !e.url.starts_with("https://") {
            eprintln!("[lunac-market] 索引里的 {} 不是 https 地址，已跳过", e.id);
            continue;
        }
        if e.name.trim().is_empty() {
            eprintln!("[lunac-market] 索引里的 {} 没有名字，已跳过", e.id);
            continue;
        }
        if !e.sha256.trim().is_empty() && !is_sha256_hex(&e.sha256) {
            eprintln!(
                "[lunac-market] 索引里的 {} 的 sha256 不是 64 位十六进制，已跳过",
                e.id
            );
            continue;
        }
        if out.iter().any(|o| o.id == e.id) {
            eprintln!("[lunac-market] 索引里的 {} 重复，保留先出现的那条", e.id);
            continue;
        }
        out.push(e);
        if out.len() >= MAX_INDEX_ENTRIES {
            eprintln!("[lunac-market] 索引超过 {MAX_INDEX_ENTRIES} 条，其余已忽略");
            break;
        }
    }
    Ok(out)
}

/// 解析并校验清单：不通过就一个字段都不落盘（调用方据此拒绝整包）。
pub fn parse_manifest(text: &str) -> Result<PluginManifest, String> {
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
    if let Some(spec) = &m.sidecar {
        validate_sidecar(spec, &m.permissions)?;
    }
    if let Some(dev) = &m.dev {
        validate_dev(dev)?;
    }
    Ok(m)
}

/// 校验 `dev` 段（轨道 H）：**能静态判定的一律拒绝整包**（与 sidecar 同口径）。
///
/// 判据逐条对应 `agent-templates/modules/README.md` §开发工具链的表格：
/// 条数 / id / https / sha256 必填 / kind 枚举 / `dest`·`binDir` 必须在 `.toolchain\` 下 /
/// `build.command` 是包内路径或白名单裸命令 / args 有上限 / cwd 不得越界。
fn validate_dev(dev: &DevSpec) -> Result<(), String> {
    if dev.toolchains.len() > MAX_DEV_PROFILES {
        return Err(format!(
            "dev.toolchains 过多（{} 条，上限 {MAX_DEV_PROFILES}）",
            dev.toolchains.len()
        ));
    }
    let mut seen: Vec<String> = Vec::new();
    for profile in &dev.toolchains {
        let id = profile.id.trim();
        if !is_safe_id(id) {
            return Err(format!(
                "dev.toolchains[].id 非法（合法字符同插件 id）：{id}"
            ));
        }
        if seen.iter().any(|item| item == id) {
            return Err(format!("dev.toolchains 里有重复的 id：{id}"));
        }
        seen.push(id.to_string());
        validate_public_https_url(&profile.url)?;
        if !is_sha256_hex(&profile.sha256) {
            return Err(format!(
                "工具链 {id} 缺少合法的 sha256（64 位十六进制；可执行代码没有哈希就不发）"
            ));
        }
        let kind = profile.kind.trim();
        if !matches!(kind, "" | "file" | "archive") {
            return Err(format!("工具链 {id} 的 kind 只支持 file / archive，收到：{kind}"));
        }
        check_under_toolchain(&profile.dest, &format!("工具链 {id} 的 dest"))?;
        let bin = profile.bin_dir.trim();
        if !bin.is_empty() {
            check_under_toolchain(bin, &format!("工具链 {id} 的 binDir"))?;
        }
    }
    if let Some(build) = &dev.build {
        let command = build.command.trim();
        if command.is_empty() {
            return Err("dev.build.command 不能为空".to_string());
        }
        let relative = command.contains('/') || command.contains('\\') || command.starts_with('.');
        if relative {
            if dev_relative(command).is_none() {
                return Err(format!("dev.build.command 必须是包内相对路径：{command}"));
            }
        } else if !DEV_BARE_COMMANDS.contains(&command) {
            return Err(format!(
                "dev.build.command 只允许包内相对路径或这些裸命令：{}（收到 {command}）",
                DEV_BARE_COMMANDS.join(" / ")
            ));
        }
        if build.args.len() > MAX_DEV_BUILD_ARGS {
            return Err(format!(
                "dev.build.args 过多（{} 条，上限 {MAX_DEV_BUILD_ARGS}）",
                build.args.len()
            ));
        }
        for arg in &build.args {
            if arg.chars().count() > MAX_DEV_ARG_CHARS {
                return Err(format!(
                    "dev.build.args 的某个参数过长（上限 {MAX_DEV_ARG_CHARS} 字符）"
                ));
            }
            if arg.contains('\0') {
                return Err("dev.build.args 含 NUL".to_string());
            }
        }
        // 包内相对路径（README 的示例写 `"cwd": "."` ⇒ 插件目录本身）。
        let cwd = build.cwd.trim();
        if !cwd.is_empty() && dev_relative(cwd).is_none() {
            return Err(format!("dev.build.cwd 必须在插件目录内（不得越界）：{cwd}"));
        }
    }
    Ok(())
}

/// `dest` / `binDir` 必须在 `.toolchain\` 下（README 的硬约束）。
///
/// 先过 `safe_join`（挡绝对路径 / `..` / 盘符），再要求**首段**是 `.toolchain`。
fn check_under_toolchain(path: &str, what: &str) -> Result<(), String> {
    let text = path.trim();
    if text.is_empty() {
        return Err(format!("{what} 不能为空"));
    }
    if dev_relative(text).is_none() {
        return Err(format!("{what} 必须是插件目录内的相对路径：{text}"));
    }
    let normalized = text.replace('\\', "/");
    let normalized = normalized.trim_start_matches("./");
    let first = normalized.split('/').next().unwrap_or("");
    if first != ".toolchain" {
        return Err(format!("{what} 必须在 .toolchain\\ 下：{text}"));
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
    // 3.1) 就绪超时：只接受 1..=3600
    if spec.ready_timeout_sec == 0 || spec.ready_timeout_sec > MAX_SIDECAR_READY_TIMEOUT_SEC {
        return Err(format!(
            "sidecar.readyTimeoutSec 需在 1..={MAX_SIDECAR_READY_TIMEOUT_SEC} 秒之间，收到：{}",
            spec.ready_timeout_sec
        ));
    }
    // 3.2) 资源闸：0 = 不限制；非 0 必须在合理区间内
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
    if !s.is_empty() && !is_sha256_hex(s) {
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

/// 按 id / 目录名定位已装插件目录（v1 `resolve_plugin_dir` 的同口径）。
///
/// 先看「目录名 == id」，再扫清单里的 `id` —— 目录名与清单 id 不一致时（手工拷贝 /
/// 旧版本）仍能定位，**但永远只在插件根目录内找**。
pub fn resolve_plugin_dir(root: &Path, key: &str) -> Option<PathBuf> {
    let key = key.trim();
    if !is_safe_id(key) {
        return None;
    }
    let exact = root.join(key);
    if exact.join(MANIFEST_FILE).is_file() {
        return Some(exact);
    }
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(text) = fs::read_to_string(dir.join(MANIFEST_FILE)) else {
            continue;
        };
        if let Ok(m) = parse_manifest(&text) {
            if m.id.trim() == key {
                return Some(dir);
            }
        }
    }
    None
}

fn validate_dependency(d: &PluginDependency) -> Result<(), String> {
    let kind = d.kind_or_file();
    if !matches!(kind, "file" | "archive" | "npm") {
        return Err(format!("不认识的依赖类型：{kind}"));
    }
    if kind != "npm" {
        if d.url.trim().is_empty() {
            return Err("依赖缺少 url".into());
        }
        validate_public_https_url(&d.url)?;
    } else if d.package.trim().is_empty() && d.url.trim().is_empty() {
        return Err("npm 依赖缺少包名".into());
    }
    if !d.sha256.trim().is_empty() && !is_sha256_hex(&d.sha256) {
        return Err("依赖的 sha256 必须是 64 位十六进制".into());
    }
    if !d.dest.trim().is_empty() && safe_join(Path::new("."), &d.dest).is_none() {
        return Err(format!("依赖落点越界：{}", d.dest));
    }
    Ok(())
}

// ── zip 解压 ─────────────────────────────────────────────────────

/// 解压 zip 到 `dest`（`dest` 不存在会创建）。
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

/// 在解压出来的目录里找清单：包根或唯一的一层子目录（GitHub 的 zip 会给一个顶层目录）。
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
            if found.is_some() {
                return Err(format!("包里有不止一份 {MANIFEST_FILE}，无法判断用哪份"));
            }
            found = Some(cand);
        }
    }
    found.ok_or_else(|| format!("包里没有 {MANIFEST_FILE}"))
}

// ── 安装 / 卸载 ──────────────────────────────────────────────────

/// 安装一份 zip 字节流，返回插件 id。同 id 视为重装 / 升级（失败回滚）。
#[cfg(test)]
pub fn install_from_bytes(bytes: &[u8], plugins_root: &Path) -> Result<String, String> {
    install_from_bytes_with_progress(bytes, plugins_root)
}

/// 同 `install_from_bytes`，额外把依赖安装的进度写进 `INSTALL_STATE`（界面轮询读取）。
pub fn install_from_bytes_with_progress(
    bytes: &[u8],
    plugins_root: &Path,
) -> Result<String, String> {
    if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(format!(
            "压缩包超过上限（{} MB）",
            MAX_ARCHIVE_BYTES / 1024 / 1024
        ));
    }
    fs::create_dir_all(plugins_root).map_err(|e| format!("建插件目录失败：{e}"))?;
    let stamp = nonce();
    let staging = plugins_root.join(format!(".staging-{stamp}"));
    let _ = fs::remove_dir_all(&staging);

    // 第一步：解压 + 校验 + 依赖。**这一步还没碰过任何已装插件**，失败只需清 staging。
    let prepared = (|| -> Result<(PluginManifest, PathBuf), String> {
        extract_zip(std::io::Cursor::new(bytes), &staging, MAX_TOTAL_BYTES)?;
        let manifest_path = find_manifest(&staging)?;
        let text = fs::read_to_string(&manifest_path).map_err(|e| format!("读清单失败：{e}"))?;
        let manifest = parse_manifest(&text)?;
        let plugin_root = manifest_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| staging.clone());
        let entry_path = safe_join(&plugin_root, &manifest.entry)
            .ok_or_else(|| format!("entry 越界：{}", manifest.entry))?;
        if !entry_path.is_file() {
            return Err(format!("清单里写的入口不存在：{}", manifest.entry));
        }
        install_dependencies(
            &plugin_root,
            &manifest.id,
            &manifest.dependencies,
            &downloads_dir(plugins_root),
        )?;
        Ok((manifest, plugin_root))
    })();
    let (manifest, plugin_root) = match prepared {
        Ok(v) => v,
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
    };

    // 第二步：旧版本挪去备份 → 新目录改名进正式位置 → 成功才删备份。
    let target = plugins_root.join(&manifest.id);
    let backup = plugins_root.join(format!(".old-{stamp}"));
    let had_old = target.exists();
    if had_old {
        let _ = fs::remove_dir_all(&backup);
        if let Err(e) = fs::rename(&target, &backup) {
            let _ = fs::remove_dir_all(&staging);
            return Err(format!("备份旧版本失败（未改动已装插件）：{e}"));
        }
    }
    let inner = plugins_root.join(format!(".staging-{stamp}-inner"));
    let placed = (|| -> Result<(), String> {
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
    if had_old {
        let _ = fs::remove_dir_all(&backup);
    }
    Ok(manifest.id)
}

/// 时间戳 + 进程号：同一毫秒内重复安装也不会撞名。
fn nonce() -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{stamp}-{}", std::process::id())
}

/// 逐条装依赖（落点在插件目录内）。任一条失败 = 整次安装失败。
///
/// 顺序执行、不并发：依赖条数是个位数，并发带来的收益抵不过「并发失败时哪一半落了盘」的复杂度。
/// 进度写进 `INSTALL_STATE`（界面按 `kind = "deps"` + `index / count` 显示「第 2/3 条」）。
fn install_dependencies(
    plugin_dir: &Path,
    plugin_id: &str,
    deps: &[PluginDependency],
    downloads: &Path,
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
        let what = if d.kind_or_file() == "npm" {
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
        // 本函数也可能被「只补依赖」的入口直接调用。
        validate_dependency(d).map_err(|e| format!("第 {} 条依赖（{what}）非法：{e}", i + 1))?;
        begin_install_state("deps", plugin_id, &d.url, i + 1, deps.len());
        let r = match d.kind_or_file() {
            "npm" => install_npm_dependency(plugin_dir, d),
            "archive" => install_archive_dependency(plugin_dir, d, downloads),
            _ => install_file_dependency(plugin_dir, d, downloads),
        };
        if let Err(e) = r {
            finish_install_state();
            return Err(format!("第 {} 条依赖（{what}）安装失败：{e}", i + 1));
        }
    }
    finish_install_state();
    Ok(())
}

/// `file` 形态：下载一个文件到插件目录内（只 https / 体积上限 / 落点必须留在目录内 / sha256）。
fn install_file_dependency(
    plugin_dir: &Path,
    d: &PluginDependency,
    downloads: &Path,
) -> Result<(), String> {
    let dest =
        safe_join(plugin_dir, &d.dest).ok_or_else(|| format!("dest 越界，已拒绝：{}", d.dest))?;
    if d.sha256.trim().is_empty() {
        crate::log::warn(format!(
            "依赖 {} 没有 sha256，已按「只校验 https」安装 —— 作者最好补上校验和",
            d.url
        ));
    }
    download_file(downloads, &d.url, &dest, &d.sha256)
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

/// `archive` 形态：下载压缩包 → 校验 sha256 → 解到插件目录内的 staging → 原子替换 `dest`。
fn install_archive_dependency(
    plugin_dir: &Path,
    d: &PluginDependency,
    downloads: &Path,
) -> Result<(), String> {
    let dest =
        safe_join(plugin_dir, &d.dest).ok_or_else(|| format!("dest 越界，已拒绝：{}", d.dest))?;
    let want = d.sha256.trim();
    let part = part_path_for(downloads, &d.url, want);
    fetch_resumable(&d.url, &part, MAX_DEP_BYTES, want)?;
    if !want.is_empty() {
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
    mutate_state(|s| s.phase = "extract".into());

    if let Err(e) = extract_archive(&bytes, format, &staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }

    // 上游常把整棵树包在一个顶层目录里（PaddleOCR 的 `.7z` 就是 `PaddleOCR-json_v1.4.1/`）。
    // 只有「唯一一个子目录、包里没有别的文件」时才把它摊平。
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

/// 把压缩包解到 `staging`（建目录 + 按格式分派）。
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

/// 解 7z 到 `staging`，**逐条校验落点 + 累计解压量 + 条目数**。
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
/// **依赖用户机器上有 Node.js**。找不到 npm 时**如实报错**（不静默跳过：那样插件会在运行到
/// 一半时才炸）。这是「AI 开发的插件可以用别的框架 / 语言」的一条现实路径：插件作者用
/// npm 生态的库，宿主负责把它装进插件目录。
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

/// 卸载插件：id 必须安全、目录必须在插件根内（canonicalize 双保险）。
pub fn uninstall(id: &str, plugins_root: &Path) -> Result<(), String> {
    if !is_safe_id(id) {
        return Err(format!("插件 id 非法：{id}"));
    }
    let dir = plugins_root.join(id);
    if !dir.is_dir() {
        return Err("插件不存在".into());
    }
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

/// 扫插件目录，列出全部已安装插件（坏包也列出来，原因原样带回）。
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
            version: String::new(),
            icon: String::new(),
            homepage: String::new(),
            entry: String::new(),
            dir: dir.to_string_lossy().to_string(),
            entry_path: String::new(),
            valid: false,
            error: String::new(),
            deps: 0,
            permissions: Vec::new(),
            has_sidecar: false,
            sidecar: None,
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
                    item.version = m.version.clone();
                    item.icon = m.icon.clone();
                    item.homepage = m.homepage.clone();
                    item.entry = m.entry.clone();
                    item.deps = m.dependencies.len();
                    item.permissions = m.permissions.clone();
                    item.has_sidecar = m.sidecar.is_some();
                    item.sidecar = m.sidecar.as_ref().map(SidecarSummary::from);
                    match safe_join(&dir, &m.entry) {
                        Some(p) if p.is_file() => {
                            item.entry_path = p.to_string_lossy().to_string();
                            item.valid = true;
                        }
                        Some(_) => {
                            item.error = format!("入口文件不存在：{}", m.entry);
                        }
                        None => item.error = format!("入口路径越界：{}", m.entry),
                    }
                }
                Err(reason) => item.error = reason,
            },
            Err(e) => item.error = format!("读清单失败：{e}"),
        }
        out.push(item);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

// ── 网络 ────────────────────────────────────────────────────────

fn http() -> Result<reqwest::blocking::Client, String> {
    // 与 v1 `open_https_stream` 同口径：只卡建连，不设总超时 ——
    // 慢的从来不是传输，总超时只会误杀「建连慢但会成功」的那几次。
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("HTTP 客户端创建失败：{e}"))
}

/// 拉远端索引并解析。
pub fn fetch_index() -> Result<Vec<PluginIndexEntry>, String> {
    let bytes = fetch_bytes(INDEX_URL, MAX_INDEX_BYTES)?;
    let text = String::from_utf8(bytes).map_err(|e| format!("插件索引不是 UTF-8：{e}"))?;
    parse_index(&text)
}

/// 下载一个 https 资源到内存（带上限）。
fn fetch_bytes(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    validate_public_https_url(url)?;
    let resp = http()?
        .get(url)
        .send()
        .map_err(|e| format!("下载失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}：下载失败", resp.status().as_u16()));
    }
    if let Some(len) = resp.content_length() {
        if len > limit {
            return Err(format!("超过上限（{} KB）", limit / 1024));
        }
    }
    let mut bytes: Vec<u8> = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    let mut resp = resp;
    loop {
        let n = resp.read(&mut buf).map_err(|e| format!("读取失败：{e}"))?;
        if n == 0 {
            break;
        }
        if bytes.len() as u64 + n as u64 > limit {
            return Err(format!("超过上限（{} KB）", limit / 1024));
        }
        bytes.extend_from_slice(&buf[..n]);
    }
    Ok(bytes)
}

// ── 安装控制面（暂停 / 取消 / 单飞闸 / 进度快照）──────────────────────
//
// v1 的实现照搬（`docs/todo.md` §待办 3-18）：暂停 = **断开当前连接、原地等**，
// 继续时重发 `Range` 接上；取消 = 删 `.part`；单飞闸防「暂停后又点一次安装」时
// 两个线程写同一个 `.part`（那会同时出现两种进度，且半截文件被谁先读完谁改名走）。

/// 依赖下载的中止标志。同一时刻只该有一次依赖安装（界面按插件串行发起），一个开关够用。
static CANCEL_DEP_INSTALL: AtomicBool = AtomicBool::new(false);
/// 暂停标志（暂停中的那次**仍然占着闸**）。
static INSTALL_PAUSED: AtomicBool = AtomicBool::new(false);
/// 单飞闸：一次安装（插件包本体 + 它自己的依赖算同一次）。
static INSTALL_BUSY: AtomicBool = AtomicBool::new(false);
/// 当前那次安装的进度快照（离屏 / 切页签 / 重开面板后用它挂回界面）。
static INSTALL_STATE: Mutex<Option<InstallState>> = Mutex::new(None);

/// 界面上的「本次安装」进度快照（字段与 v1 同名同义）。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallState {
    pub active: bool,
    /// `package`（插件包本体）/ `deps`（补依赖）
    pub kind: String,
    /// 插件 id（包本体那段还没解析出清单，给空串）
    pub id: String,
    /// 当前正在下的那个文件 —— 界面按它把进度挂回**对应的那一行**
    pub url: String,
    pub phase: String,
    pub downloaded: u64,
    pub total: u64,
    pub paused: bool,
    /// 磁盘上有可续传的半截文件（下次点「安装 / 继续」会接着下）
    pub resumable: bool,
    /// 依赖序号 / 总数（1 起；`package` 阶段都是 0）
    pub index: usize,
    pub count: usize,
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

/// 一个下载断点的元数据（与 `.part` 同目录、同名 + `.json`）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PartMeta {
    url: String,
    /// 归属的插件 id（包本体阶段为空）；重启后界面靠它把「继续下载」挂回对应的行。
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

/// 安装闸的持有凭证；`drop` 时释放。
#[derive(Debug)]
pub struct InstallBusyGuard(());

impl Drop for InstallBusyGuard {
    fn drop(&mut self) {
        INSTALL_BUSY.store(false, Ordering::SeqCst);
    }
}

/// 占用安装闸；已有一次在进行时返回 Err（**原样报给界面**，由调用方决定文案）。
pub fn try_begin_install(what: &str) -> Result<InstallBusyGuard, String> {
    if INSTALL_BUSY.swap(true, Ordering::SeqCst) {
        let st = install_state();
        let hint = if st.active && st.paused {
            "那一次已暂停：点「继续」接着下"
        } else {
            "等它结束，或先取消"
        };
        return Err(format!("已有一次安装在进行（{what}）—— {hint}"));
    }
    Ok(InstallBusyGuard(()))
}

/// 当前是否有安装占着闸（诊断用）。
pub fn install_busy() -> bool {
    INSTALL_BUSY.load(Ordering::SeqCst)
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

/// 请求中止当前这次依赖安装。下载循环每读一块检查一次；幂等。
///
/// 取消只删 `.part`（与 v1 同语义）；插件目录里已落的文件由整次安装回滚负责。
pub fn cancel_dependency_install() {
    CANCEL_DEP_INSTALL.store(true, Ordering::Relaxed);
    INSTALL_PAUSED.store(false, Ordering::Relaxed);
    mutate_state(|s| s.paused = false);
}

fn dep_install_cancelled() -> bool {
    CANCEL_DEP_INSTALL.load(Ordering::Relaxed)
}

fn mutate_state(f: impl FnOnce(&mut InstallState)) {
    if let Ok(mut g) = INSTALL_STATE.lock() {
        f(g.get_or_insert_with(InstallState::default));
    }
}

fn begin_install_state(kind: &str, id: &str, url: &str, index: usize, count: usize) {
    INSTALL_PAUSED.store(false, Ordering::Relaxed);
    if let Ok(mut g) = INSTALL_STATE.lock() {
        *g = Some(InstallState {
            active: true,
            kind: kind.to_string(),
            id: id.to_string(),
            url: url.to_string(),
            phase: "download".into(),
            index,
            count,
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

/// 下载暂存目录：`<exe 根>/temp/downloads`（由插件根 `<exe 根>/Modules` 反推）。
///
/// 放 `temp/` 而不是插件目录，两个理由（v1 同）：① 插件目录在「安装 / 升级」时会被整体
/// 替换，半截文件不该跟着搬；② 断点要在**插件还没装好**时也活着（强制退出后重来）。
pub fn downloads_dir(plugins_root: &Path) -> PathBuf {
    let base = plugins_root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| plugins_root.to_path_buf());
    base.join("temp").join("downloads")
}

/// 磁盘上还剩哪些断点。`downloaded` 取 `.part` 的**实际长度**（不信 json 里的旧值）。
pub fn pending_downloads(plugins_root: &Path) -> Vec<PendingDownload> {
    let Ok(entries) = fs::read_dir(downloads_dir(plugins_root)) else {
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

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 断点文件名：`temp/downloads/<sha256(url + 清单 sha256) 前 16 位>.part`。
///
/// 把清单里的 sha256 也算进去：作者换版本时（url 不变、哈希变）旧断点自动作废，
/// 不会把上一版的前半段和这一版的后半段拼在一起。
fn part_path_for(downloads: &Path, url: &str, sha256_want: &str) -> PathBuf {
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    h.update(b"\n");
    h.update(sha256_want.trim().to_ascii_lowercase().as_bytes());
    let tag = hex_digest(h.finalize().as_slice());
    downloads.join(format!("{}.part", &tag[..16]))
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
/// 与普通下载的区别只在这里：**416 单独给一个可识别的错误**，好让调用方丢掉断点重来，
/// 而不是把整个安装判死。
fn open_https_range(
    url: &str,
    offset: u64,
    etag: &str,
) -> Result<reqwest::blocking::Response, String> {
    validate_public_https_url(url)?;
    let mut req = http()?.get(url);
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
fn fetch_package_bytes(
    plugins_root: &Path,
    url: &str,
    sha256_want: &str,
) -> Result<Vec<u8>, String> {
    let part = part_path_for(&downloads_dir(plugins_root), url, sha256_want);
    fetch_resumable(url, &part, MAX_ARCHIVE_BYTES, sha256_want)?;
    let bytes = fs::read(&part).map_err(|e| format!("读下载文件失败：{e}"))?;
    remove_part(&part);
    Ok(bytes)
}

/// 流式下载单个依赖到 `dest`（可续传 + 可选 sha256 校验；`.part` 在 `temp/downloads`）。
fn download_file(
    downloads: &Path,
    url: &str,
    dest: &Path,
    sha256: &str,
) -> Result<(), String> {
    let want = sha256.trim();
    if !want.is_empty() && !is_sha256_hex(want) {
        return Err(format!("sha256 必须是 64 位十六进制：{want}"));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("建依赖目录失败：{e}"))?;
    }
    let part = part_path_for(downloads, url, want);
    fetch_resumable(url, &part, MAX_DEP_BYTES, want)?;
    if !want.is_empty() {
        let got = sha256_file(&part)?;
        if !got.eq_ignore_ascii_case(want) {
            remove_part(&part);
            return Err(format!("sha256 不匹配（期望 {want}，实得 {got}）"));
        }
    }
    if fs::rename(&part, dest).is_err() {
        // 跨卷时 `rename` 会失败（用户可能把插件目录指到别的盘）⇒ 退化成复制
        fs::copy(&part, dest).map_err(|e| {
            remove_part(&part);
            format!("落盘依赖失败：{e}")
        })?;
        let _ = fs::remove_file(&part);
    }
    let _ = fs::remove_file(part_meta_path(&part));
    Ok(())
}

/// 从 https zip URL 安装：下载（可续传 / 可暂停）→ sha256 → 解压 → 落盘（返回插件 id）。
///
/// 这是界面唯一入口，所以**单飞闸**在这里占：同一次安装（包本体 + 它自己的依赖）算一次；
/// 暂停中的那次仍然占着闸 —— 恢复必须走 `set_install_paused(false)`，不能再发一次安装。
pub fn install_from_url(url: &str, sha256: &str, plugins_root: &Path) -> Result<String, String> {
    let _gate = try_begin_install("插件包")?;
    validate_public_https_url(url)?;
    let expected = sha256.trim();
    if !expected.is_empty() && !is_sha256_hex(expected) {
        return Err(format!("sha256 必须是 64 位十六进制：{expected}"));
    }
    begin_install_state("package", "", url, 0, 0);
    let result = (|| -> Result<String, String> {
        let bytes = fetch_package_bytes(plugins_root, url, expected)?;
        verify_package_sha256(&bytes, expected)?;
        install_from_bytes_with_progress(&bytes, plugins_root)
    })();
    finish_install_state();
    result
}

/// 给**已经装好**的插件补装清单里的依赖（v1 `install_plugin_dependencies` 的等价物）。
///
/// 用途：插件升级后依赖没跟着装、或者上次依赖下载被取消。返回装了几条。
pub fn install_dependencies_for(id: &str, plugins_root: &Path) -> Result<usize, String> {
    let _gate = try_begin_install("插件依赖")?;
    let dir = resolve_plugin_dir(plugins_root, id)
        .ok_or_else(|| format!("插件不存在：{id}"))?;
    let text = fs::read_to_string(dir.join(MANIFEST_FILE))
        .map_err(|e| format!("读清单失败：{e}"))?;
    let manifest = parse_manifest(&text)?;
    if manifest.dependencies.is_empty() {
        return Err("这个插件的清单里没有声明依赖".into());
    }
    install_dependencies(
        &dir,
        &manifest.id,
        &manifest.dependencies,
        &downloads_dir(plugins_root),
    )?;
    Ok(manifest.dependencies.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn tmp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "lunac-v2-market-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

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

    #[test]
    fn index_skips_bad_entries_and_keeps_the_first_duplicate() {
        let text = r#"[
            {"id":"ok","name":"A","url":"https://example.com/a.zip"},
            {"id":"bad id","name":"B","url":"https://example.com/b.zip"},
            {"id":"plain","name":"C","url":"http://example.com/c.zip"},
            {"id":"noname","name":"  ","url":"https://example.com/d.zip"},
            {"id":"ok","name":"A2","url":"https://example.com/a2.zip"},
            {"id":"hashbad","name":"E","url":"https://example.com/e.zip","sha256":"abc"}
        ]"#;
        let list = parse_index(text).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "A");
        assert!(parse_index("{not json").is_err());
    }

    #[test]
    fn manifest_rejects_unsafe_ids_and_entries() {
        assert!(parse_manifest(&good_manifest("demo")).is_ok());
        for bad in ["../evil", ".hidden", "UP", "a b"] {
            assert!(
                parse_manifest(&good_manifest(bad)).is_err(),
                "id 应被拒：{bad}"
            );
        }
        let bad_entry = good_manifest("demo").replace("index.js", "../evil.js");
        assert!(parse_manifest(&bad_entry).is_err());
        let not_js = good_manifest("demo").replace("index.js", "index.html");
        assert!(parse_manifest(&not_js).is_err());
    }

    #[test]
    fn sidecar_requires_process_spawn_and_stays_inside_the_plugin() {
        // 与 v1 同口径：sidecar 必须与 process.spawn 同时出现。
        let without_perm = r#"{"id":"demo","entry":"index.js",
            "sidecar":{"command":"bin/demo.exe"}}"#;
        let err = parse_manifest(without_perm).expect_err("缺 process.spawn 应拒");
        assert!(err.contains("process.spawn"), "{err}");

        let ok = r#"{"id":"demo","entry":"index.js","permissions":["process.spawn"],
            "sidecar":{"command":"bin/demo.exe","args":["--x"],"transport":"stdio",
                       "restart":"on-failure","readyTimeoutSec":15,
                       "memoryLimitMiB":512,"processLimit":4,
                       "allowLocalPorts":[11434]}}"#;
        let m = parse_manifest(ok).expect("合法 sidecar");
        let spec = m.sidecar.expect("sidecar");
        assert_eq!(spec.ready_timeout_sec, 15);
        assert_eq!(spec.memory_limit_mib, 512);
        assert_eq!(spec.allow_local_ports, vec![11434]);

        // 越界 / 坏枚举 / 坏端口，一律拒绝整包。
        for bad in [
            r#""sidecar":{"command":"../demo.exe"}"#,
            r#""sidecar":{"command":"bin/demo.exe","transport":"udp"}"#,
            r#""sidecar":{"command":"bin/demo.exe","restart":"always"}"#,
            r#""sidecar":{"command":"bin/demo.exe","readyTimeoutSec":0}"#,
            r#""sidecar":{"command":"bin/demo.exe","memoryLimitMiB":8}"#,
            r#""sidecar":{"command":"bin/demo.exe","allowLocalPorts":[0]}"#,
            r#""sidecar":{"command":"bin/demo.exe","sha256":"abc"}"#,
            r#""sidecar":{"command":"bin/demo.exe","cwd":"../x"}"#,
        ] {
            let text = format!(
                r#"{{"id":"demo","entry":"index.js","permissions":["process.spawn"],{bad}}}"#
            );
            assert!(parse_manifest(&text).is_err(), "应被拒：{bad}");
        }
    }

    #[test]
    fn local_model_manifest_from_the_market_still_parses() {
        // 真清单形状（`app/plugin-dist/local-model/lunac-plugin.json` 的 sidecar 段）。
        let text = r#"{"id":"local-model","entry":"index.js","permissions":["process.spawn"],
            "sidecar":{"processLimit":8,"memoryLimitMiB":8192,"autostart":false,
                       "command":"bin/llama-server.exe","transport":"http","port":8090,
                       "readyTimeoutSec":180,
                       "args":["-m","models/x.gguf","--port","8090"]}}"#;
        let m = parse_manifest(text).expect("市场清单应可解析");
        let spec = m.sidecar.expect("sidecar");
        assert_eq!(spec.command, "bin/llama-server.exe");
        assert_eq!(spec.port, 8090);
        assert_eq!(spec.ready_timeout_sec, 180);
        assert_eq!(spec.process_limit, 8);
    }

    #[test]
    fn resolve_plugin_dir_prefers_exact_dir_then_scans_ids() {
        let root = tmp_root("resolve");
        // 目录名 == id。
        fs::create_dir_all(root.join("alpha")).unwrap();
        fs::write(root.join("alpha").join(MANIFEST_FILE), good_manifest("alpha")).unwrap();
        // 目录名 ≠ id（手工拷贝 / 旧版本）。
        fs::create_dir_all(root.join("folder")).unwrap();
        fs::write(root.join("folder").join(MANIFEST_FILE), good_manifest("beta")).unwrap();
        assert_eq!(
            resolve_plugin_dir(&root, "alpha").map(|p| p.file_name().unwrap().to_string_lossy().to_string()),
            Some("alpha".to_string())
        );
        assert_eq!(
            resolve_plugin_dir(&root, "beta").map(|p| p.file_name().unwrap().to_string_lossy().to_string()),
            Some("folder".to_string())
        );
        assert!(resolve_plugin_dir(&root, "missing").is_none());
        // 非法 id 不扫盘。
        assert!(resolve_plugin_dir(&root, "../alpha").is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dependencies_only_accept_file_urls_and_stay_inside_the_plugin() {
        let with_dep = |dep: &str| {
            format!(r#"{{"id":"demo","entry":"index.js","dependencies":[{dep}]}}"#)
        };
        assert!(parse_manifest(&with_dep(
            r#"{"url":"https://example.com/x.exe","dest":"bin/x.exe"}"#
        ))
        .is_ok());
        for bad in [
            r#"{"url":"http://example.com/x.exe","dest":"bin/x.exe"}"#,
            r#"{"url":"https://example.com/x.exe","dest":"../x.exe"}"#,
            r#"{"url":"https://example.com/x.exe","sha256":"abc"}"#,
            r#"{"type":"weird","url":"https://example.com/x"}"#,
        ] {
            assert!(
                parse_manifest(&with_dep(bad)).is_err(),
                "依赖应被拒：{bad}"
            );
        }
    }

    #[test]
    fn safe_join_rejects_traversal() {
        let root = Path::new("C:/plugins/demo");
        assert!(safe_join(root, "index.js").is_some());
        assert!(safe_join(root, "bin/x.exe").is_some());
        for bad in ["../x", "a/../../b", "/abs", "C:/x", "a//b", "a/ "] {
            assert!(safe_join(root, bad).is_none(), "应被拒：{bad}");
        }
    }

    #[test]
    fn install_extracts_to_modules_and_upgrade_keeps_only_the_new_one() {
        let root = tmp_root("install");
        let zip = zip_bytes(&[
            ("demo/lunac-plugin.json", &good_manifest("demo")),
            ("demo/index.js", "export default {}"),
        ]);
        let id = install_from_bytes(&zip, &root).unwrap();
        assert_eq!(id, "demo");
        assert!(root.join("demo").join("index.js").is_file());
        // 升级：同名 id 覆盖，旧内容不保留
        let zip2 = zip_bytes(&[
            ("demo/lunac-plugin.json", &good_manifest("demo")),
            ("demo/index.js", "export default {v:2}"),
            ("demo/new.txt", "n"),
        ]);
        assert_eq!(install_from_bytes(&zip2, &root).unwrap(), "demo");
        let body = fs::read_to_string(root.join("demo").join("index.js")).unwrap();
        assert!(body.contains("v:2"));
        // 坏包：缺入口 / 越界路径都拒装，且不留 staging
        let bad = zip_bytes(&[("demo/lunac-plugin.json", &good_manifest("demo"))]);
        assert!(install_from_bytes(&bad, &root).is_err());
        let evil = zip_bytes(&[
            ("demo/lunac-plugin.json", &good_manifest("demo")),
            ("demo/index.js", "x"),
            ("../evil.txt", "x"),
        ]);
        assert!(install_from_bytes(&evil, &root).is_err());
        let leftovers: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "不应留下 staging：{leftovers:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn list_and_uninstall_report_broken_packages_honestly() {
        let root = tmp_root("list");
        let ok = zip_bytes(&[
            ("good/lunac-plugin.json", &good_manifest("good")),
            ("good/index.js", "x"),
        ]);
        install_from_bytes(&ok, &root).unwrap();
        fs::create_dir_all(root.join("broken")).unwrap();
        fs::write(root.join("broken").join(MANIFEST_FILE), "{oops").unwrap();

        let list = list_installed(&root);
        assert_eq!(list.len(), 2);
        let good = list.iter().find(|p| p.id == "good").unwrap();
        assert!(good.valid && good.error.is_empty());
        let broken = list.iter().find(|p| p.id == "broken").unwrap();
        assert!(!broken.valid && !broken.error.is_empty());

        assert!(uninstall("good", &root).is_ok());
        assert!(!root.join("good").exists());
        assert!(uninstall("good", &root).is_err());
        assert!(uninstall("../evil", &root).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sha256_check_rejects_mismatch_and_bad_format() {
        let data = b"hello";
        assert!(matches!(verify_package_sha256(data, ""), Ok(false)));
        let good = hex_sha256(data);
        assert!(matches!(verify_package_sha256(data, &good), Ok(true)));
        assert!(verify_package_sha256(data, &"0".repeat(64)).is_err());
        assert!(verify_package_sha256(data, "abc").is_err());
    }

    // ── 安装管线（续传 / 暂停 / 单飞闸 / 依赖形态）──────────────────────

    #[test]
    fn part_path_is_stable_per_url_and_manifest_hash() {
        let dir = Path::new("C:/x/temp/downloads");
        let a = part_path_for(dir, "https://e.com/p.zip", "AA".repeat(32).as_str());
        let b = part_path_for(dir, "https://e.com/p.zip", "aa".repeat(32).as_str());
        // sha256 大小写不敏感：同一份清单不该产生两个断点。
        assert_eq!(a, b);
        // 换 url 或换哈希（作者发新版本）⇒ 断点必须换名，旧断点不能续到新文件上。
        assert_ne!(a, part_path_for(dir, "https://e.com/q.zip", ""));
        assert_ne!(a, part_path_for(dir, "https://e.com/p.zip", ""));
        assert!(a.to_string_lossy().ends_with(".part"));
        // 名字只有 16 位十六进制 + `.part`（目录里还要放 `.part.json`）。
        let name = a.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(name.len(), 16 + ".part".len());
    }

    #[test]
    fn part_meta_roundtrip_tolerates_older_files() {
        let root = tmp_root("partmeta");
        let part = root.join("x.part");
        let meta = PartMeta {
            url: "https://e.com/p.zip".into(),
            id: "demo".into(),
            etag: "\"abc\"".into(),
            total: 100,
            offset: 40,
            sha256: "f".repeat(64),
        };
        write_part_meta(&part_meta_path(&part), &meta);
        let back = read_part_meta(&part_meta_path(&part)).expect("应能读回");
        assert_eq!(back.url, meta.url);
        assert_eq!(back.offset, 40);
        assert_eq!(back.total, 100);
        // 老文件（缺字段）也要能读 —— 否则升级后旧断点会让整个下载直接报错。
        fs::write(part_meta_path(&part), r#"{"url":"https://e.com/old"}"#).unwrap();
        let old = read_part_meta(&part_meta_path(&part)).expect("缺字段应可读");
        assert_eq!(old.offset, 0);
        assert_eq!(old.total, 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn pending_downloads_reports_the_actual_part_length() {
        let root = tmp_root("pending");
        let downloads = downloads_dir(&root);
        fs::create_dir_all(&downloads).unwrap();
        let part = downloads.join("deadbeefdeadbeef.part");
        fs::write(&part, b"0123456789").unwrap();
        // json 里的 offset 是旧值（20），界面必须按 `.part` 的实际长度显示（10）。
        fs::write(
            part_meta_path(&part),
            r#"{"url":"https://e.com/p.zip","id":"demo","total":100,"offset":20}"#,
        )
        .unwrap();
        let list = pending_downloads(&root);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].downloaded, 10);
        assert_eq!(list[0].total, 100);
        assert_eq!(list[0].id, "demo");
        // 只有 json 没有 `.part`（已经被搬走）⇒ 不算断点。
        fs::remove_file(&part).unwrap();
        assert!(pending_downloads(&root).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn install_gate_is_exclusive_and_released_on_drop() {
        let guard = try_begin_install("测试").expect("第一次应成功");
        assert!(install_busy());
        let err = try_begin_install("测试").expect_err("第二次应被拒");
        assert!(err.contains("已有一次安装在进行"), "{err}");
        drop(guard);
        assert!(!install_busy());
        // 释放后能再占（否则一次失败会永久锁死市场）。
        assert!(try_begin_install("测试").is_ok());
    }

    #[test]
    fn install_state_tracks_pause_and_finish() {
        begin_install_state("deps", "demo", "https://e.com/x.bin", 2, 3);
        let st = install_state();
        assert!(st.active);
        assert_eq!(st.kind, "deps");
        assert_eq!(st.id, "demo");
        assert_eq!(st.index, 2);
        assert_eq!(st.count, 3);
        set_install_paused(true);
        assert!(install_state().paused);
        finish_install_state();
        let done = install_state();
        assert!(!done.active && !done.paused && !done.resumable);
        // 收尾后暂停标志也要清掉，否则下一次安装一开始就停住。
        assert!(!INSTALL_PAUSED.load(Ordering::Relaxed));
    }

    #[test]
    fn archive_dependencies_detect_format_by_magic_and_reject_others() {
        let zip = zip_bytes(&[("a.txt", "x")]);
        assert_eq!(detect_archive_format(&zip), Ok(ArchiveFormat::Zip));
        let sevenz_magic = [0x37u8, 0x7a, 0xbc, 0xaf, 0x27, 0x1c, 0, 0];
        assert_eq!(
            detect_archive_format(&sevenz_magic),
            Ok(ArchiveFormat::SevenZ)
        );
        // 扩展名是 `.zip` 但内容不是 ⇒ 按内容判，拒。
        assert!(detect_archive_format(b"not an archive").is_err());
    }

    #[test]
    fn archive_extraction_flattens_a_single_top_level_folder() {
        let root = tmp_root("archive");
        let bytes = zip_bytes(&[("pkg-v1/bin/engine.exe", "MZ"), ("pkg-v1/models/a.bin", "x")]);
        let staging = root.join("staging");
        extract_archive(&bytes, ArchiveFormat::Zip, &staging).unwrap();
        let flat = flatten_single_top_dir(&staging);
        assert_eq!(flat, staging.join("pkg-v1"));
        assert!(flat.join("bin").join("engine.exe").is_file());
        // 有多个子目录（或带散文件）时不摊平 —— 摊平会丢东西。
        fs::write(staging.join("readme.txt"), "x").unwrap();
        assert_eq!(flatten_single_top_dir(&staging), staging);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_dependency_kinds_are_rejected_before_any_io() {
        let root = tmp_root("depkind");
        let deps = vec![PluginDependency {
            kind: "tar".into(),
            url: "https://e.com/x.tar".into(),
            ..Default::default()
        }];
        let err = install_dependencies(&root, "demo", &deps, &downloads_dir(&root))
            .expect_err("未知类型应拒");
        assert!(err.contains("不认识的依赖类型"), "{err}");
        // 依赖为空 ⇒ 直接成功（不发起任何下载，也不改全局安装状态）。
        assert!(install_dependencies(&root, "demo", &[], &downloads_dir(&root)).is_ok());
        let _ = fs::remove_dir_all(&root);
    }
}
