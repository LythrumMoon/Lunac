// src-tauri/src/updater.rs
// 应用自更新（检查 → 下载 → 静默安装）—— 2026-09-30
//
// **为什么不用 `tauri-plugin-updater`**：官方插件的安装那一步假定「产物由 `tauri build`
// 的 bundler 生成」（它要 updater 专用格式 + minisign 签名）。本仓的安装包是**手搓的**
// `scripts\lunac-installer.nsi` + `build-release.ps1`（要往包里塞 PaddleOCR、VSCode 扩展、
// 插件暂存目录 —— tauri bundler 表达不了这些）。所以这里只借它的思路，动作自己完成：
// 下载 Setup.exe → 校验 → 拉起 `Setup.exe /S` → 退出，让安装器接手。
//
// **整条链路有三个前提，缺一个就会静默失败**（都在 2026-09-30 一并补齐，见 lunac-installer.nsi）：
//   ① 安装器认得上一版的安装目录 —— `InstallDirRegKey` + 写 `InstallLocation`。
//      没有它，静默安装会落到 `%LOCALAPPDATA%\Lunac`，把用户装成**两份**。
//   ② 安装器在覆盖前停掉正在运行的实例 —— 核心段开头那两条 `taskkill`。
//      否则 `lunac.exe` 被占用：交互式安装弹「Error opening file for writing」，静默安装直接失败。
//   ③ 装完把用户带回应用 —— `.onInstSuccess`（**仅 `/S`** 时 `Exec`）。
//
// **校验只有 sha256**（用户 2026-09-30 裁决）：`latest.json` 本身走 GitHub 的 https，
// 信任根就是 GitHub。将来要上签名（minisign / Authenticode），加在 `download()` 里
// 校验那一步即可 —— 别把这层判断散到调用方去。
//
// **更新源**用 `/releases/latest/download/<asset>` 这个固定入口，而不是 raw 分支：
// raw 有 CDN 缓存，刚发布的版本可能读到旧的；这个入口由 GitHub 自己重定向到
// 「最新一个**非预发布** Release」的同名资产 —— 发布流程因此只需要上传，不用改代码。

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};

use crate::commands::run_blocking;
// 本仓的 `log` 是**自己的模块**（`src/log.rs`，落 `<exe 根>\temp\logs\`），不是 log crate ——
// 写 `log::warn(..)` 时必须先把它引进作用域。
use crate::log;

/// 版本清单的固定入口（见文件头「更新源」）。
pub const MANIFEST_URL: &str =
    "https://github.com/LythrumMoon/Lunac/releases/latest/download/latest.json";

/// 清单是纯文本小文件；超了说明拿到的不是清单（例如被某个首页顶替了）。
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// 安装包上限：0.9.7 实测 **15.9 MB**（PaddleOCR-json 已于 2026-09-30 移出安装包 ——
/// 出包前是 110.3 MB —— 改由 `ocr` 插件按需下载），留足余量。超了直接拒绝，别把磁盘写满。
const MAX_INSTALLER_BYTES: u64 = 512 * 1024 * 1024;
/// 检查更新要快 —— 启动后是静默跑的，挂太久没意义。
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
/// **建连**上限。只管「连不上」，与「传了多少、传了多久」无关 —— 下载安装包那条路
/// 已经不设总超时了（见 `http_stream` 的说明）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// 进度事件的最小间隔（字节）。每 256 KB 推一次：110 MB 约 440 次，进度条够平滑又不刷爆 IPC。
const PROGRESS_STEP: u64 = 256 * 1024;

/// 下载 / 校验 / 安装三个阶段共用一条事件（前端只挂一个监听器）。
const UPDATE_EVENT: &str = "update-progress";

// ── 版本清单（Release 资产 latest.json）────────────────────────────
//
// 形状与 `scripts\publish-release.ps1` 生成的那份一一对应，改动要两边一起改。

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UpdateManifest {
    /// 形如 `0.9.8`（不带 `v`）。与 `Cargo.toml` / `tauri.conf.json` 的版本同一套规则。
    pub version: String,
    /// 更新说明（可空）；界面上原样展示，不做 Markdown 渲染。
    #[serde(default)]
    pub notes: String,
    /// 安装包直链（**必须 https**，见 `validate_manifest`）。
    #[serde(default)]
    pub url: String,
    /// 安装包的 sha256（小写十六进制，64 位）。**必需** —— 没有它整份清单作废。
    #[serde(default)]
    pub sha256: String,
    /// 安装包字节数；0 = 清单没写（那就只靠 sha256 兜底）。
    #[serde(default)]
    pub size: u64,
}

/// 返给前端的检查结果。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// 当前运行的版本（`Cargo.toml` 的 version，构建时嵌进来）。
    pub current: String,
    pub has_update: bool,
    /// 最新版本；`has_update = false` 时等于 `current` 之外的值没有意义，前端别拿它显示。
    pub latest: String,
    pub notes: String,
    pub size: u64,
}

/// 进度事件载荷。`total = 0` 表示上游没给 `Content-Length`（进度条切成不确定态）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    /// `downloading` / `verifying` / `installing`
    pub phase: String,
    pub downloaded: u64,
    pub total: u64,
}

// ── 配置（<exe 根>\config\update.json）────────────────────────────
//
// 与 `ai.json` / `music.json` 同级、同口径：**应用配置**落 `config\`，业务数据才进 `ModuleData`。

fn config_path() -> PathBuf {
    crate::storage::lunac_root_dir().join("config").join("update.json")
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateConfig {
    /// 启动后静默检查一次（默认**开**：没有任何副作用，只是问一句）。
    ///
    /// 2026-10-05：设置面板把本字段与 `auto_install` **合并成一个「自动更新」开关**
    /// （用户要求）—— 面板读写时两字段同真同假；这里保留两个字段是为了不动 `update.json`
    /// 的文件格式。默认值同步改成 `auto_install = true`（用户选「默认开：检查+自动安装」）。
    pub check_on_startup: bool,
    /// 发现新版就自动下载并安装，不再问（默认**开**，见上）。
    pub auto_install: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self { check_on_startup: true, auto_install: true }
    }
}

/// 读不出来（文件不存在 / 被手工改坏）一律当默认值 —— 更新配置坏掉不该让应用起不来。
fn load_config() -> UpdateConfig {
    match fs::read_to_string(config_path()) {
        Ok(text) => parse_json_lossy(&text).unwrap_or_default(),
        Err(_) => UpdateConfig::default(),
    }
}

/// 落盘。写失败**如实报错**（与 `set_ai_config` / `music_config_set` 同一条纪律：
/// 「面板看着保存成功、重启后设置没了」是最难查的一类）。
fn save_config(cfg: &UpdateConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| {
            log::warn(format!("updater: create config dir failed: {e}"));
            "配置目录不可写 —— 请检查安装目录权限".to_string()
        })?;
    }
    let text = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化失败：{e}"))?;
    fs::write(&path, text).map_err(|e| {
        log::warn(format!("updater: write config failed: {e}"));
        "保存更新设置失败 —— 请检查安装目录权限".to_string()
    })
}

// ── 版本比较 ─────────────────────────────────────────────────────

/// `x.y.z` → `[x, y, z]`。允许前置 `v`、允许只写两段（第三段补 0）；
/// 带后缀（`0.9.8-rc1`）或非数字一律 `None`。
fn parse_version(v: &str) -> Option<[u64; 3]> {
    let v = v.trim().trim_start_matches('v');
    let mut it = v.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some([major, minor, patch])
}

/// `latest` 是否比 `current` 新。
///
/// **解析不出来一律返回 `false`** —— 宁可漏报一次，也不要因为上游写了个奇怪版本号
/// 就反复弹更新（更糟的是静默模式下把同一个版本装第二遍）。
fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

// ── 网络 ─────────────────────────────────────────────────────────

fn http(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        // GitHub 对没有 UA 的客户端会挑刺；带上版本号也方便在日志里认人。
        .user_agent(concat!("Lunac/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| {
            log::warn(format!("updater: build http client failed: {e}"));
            "网络组件初始化失败 —— 请重启应用".to_string()
        })
}

/// 下载**安装包**用的客户端（2026-09-30）：**只有建连超时，没有总超时**。
///
/// 原来这里是一个 600s 的总上限，用户要求取消 —— 安装包 110MB，慢链路（或挂着代理）下
/// 判超时的不是「挂了」而是「慢」，而判超时的代价是整包白下（下一次要从头开始）。
/// 与插件依赖那条路同一口径（见 `plugin_market::open_https_stream`）。
///
/// 代价说清楚：传输中途对端彻底不出声时这里不会主动掐断（`reqwest::blocking` 没有
/// `read_timeout`），只能靠 TCP keepalive 兜底。用户选的就是「宁可等，也别半路判死」。
fn http_stream() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("Lunac/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| {
            log::warn(format!("updater: build http client failed: {e}"));
            "网络组件初始化失败 —— 请重启应用".to_string()
        })
}

/// 失败原因只给「用户能做什么」：状态码 / URL / 响应体只进日志（code-rules 预检 #40 ③）。
fn net_hint(op: &str, e: &reqwest::Error) -> String {
    log::warn(format!("updater: {op} failed: {e}"));
    if e.is_timeout() {
        "连接超时 —— 请检查网络后重试".to_string()
    } else if e.is_connect() {
        "连不上更新服务器 —— 请检查网络或代理".to_string()
    } else {
        "获取更新信息失败 —— 请稍后重试".to_string()
    }
}

/// 解析 JSON 前剥掉 BOM。**serde_json 不认 BOM**（会报 `expected value, line 1 column 1`），
/// 而清单是脚本生成的、编辑器/管道都可能在开头留一个。
fn parse_json_lossy<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(text.trim_start_matches('\u{feff}'))
}

fn fetch_manifest() -> Result<UpdateManifest, String> {
    let client = http(CHECK_TIMEOUT)?;
    let resp = client.get(MANIFEST_URL).send().map_err(|e| net_hint("检查更新", &e))?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        // 仓库还没发过 Release（或最新那个是预发布）。这不是故障，如实说。
        log::warn("updater: latest.json 404 —— 还没发布过 Release？");
        return Err("还没有发布过可更新的版本".to_string());
    }
    if !resp.status().is_success() {
        log::warn(format!("updater: manifest HTTP {}", resp.status()));
        return Err("获取更新信息失败 —— 请稍后重试".to_string());
    }
    if resp.content_length().is_some_and(|len| len > MAX_MANIFEST_BYTES) {
        log::warn("updater: manifest too large");
        return Err("更新信息异常 —— 请到 GitHub 手动下载".to_string());
    }

    // 读的时候再兜一次上限：Content-Length 可以撒谎，也可能根本没给。
    let mut buf = Vec::new();
    resp.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| {
            log::warn(format!("updater: read manifest failed: {e}"));
            "更新信息读取失败 —— 请稍后重试".to_string()
        })?;
    if buf.len() as u64 > MAX_MANIFEST_BYTES {
        log::warn("updater: manifest exceeded cap while reading");
        return Err("更新信息异常 —— 请到 GitHub 手动下载".to_string());
    }

    let text = String::from_utf8_lossy(&buf);
    let manifest: UpdateManifest = parse_json_lossy(&text).map_err(|e| {
        log::warn(format!("updater: manifest parse failed: {e}"));
        "更新信息格式不对 —— 请到 GitHub 手动下载".to_string()
    })?;
    Ok(manifest)
}

/// 清单是**指向可执行文件的指令**，所以下之前先按最小必要校验一遍：
/// 版本号可解析、直链是 https 且落在 GitHub 自己的域、sha256 像那么回事。
/// 少任何一条就拒绝 —— 清单坏了宁可不更新，也不要拉一个来路不明的东西回来跑。
fn validate_manifest(m: &UpdateManifest) -> Result<(), String> {
    if parse_version(&m.version).is_none() {
        log::warn(format!("updater: bad version in manifest: {}", m.version));
        return Err("更新信息异常（版本号不对）—— 请到 GitHub 手动下载".to_string());
    }
    if !m.url.starts_with("https://") {
        log::warn("updater: manifest url is not https");
        return Err("更新信息异常（下载地址不安全）—— 请到 GitHub 手动下载".to_string());
    }
    // 只认 GitHub 自己的域：清单若被改动过，也换不到别的站点去。
    // `github.com/.../releases/download/...` 会 302 到 `objects.githubusercontent.com`，
    // 但清单里写的是**重定向之前**的那个地址，所以两个都放行只会放宽判据、不会放宽实际来源。
    let host_ok = m.url.starts_with("https://github.com/")
        || m.url.starts_with("https://objects.githubusercontent.com/")
        || m.url.starts_with("https://githubusercontent.com/");
    if !host_ok {
        log::warn("updater: manifest url host is not github");
        return Err("更新信息异常（下载来源不对）—— 请到 GitHub 手动下载".to_string());
    }
    let hex_ok = m.sha256.len() == 64 && m.sha256.chars().all(|c| c.is_ascii_hexdigit());
    if !hex_ok {
        log::warn("updater: manifest sha256 missing or malformed");
        return Err("更新信息异常（缺少校验值）—— 请到 GitHub 手动下载".to_string());
    }
    Ok(())
}

// ── 下载 ─────────────────────────────────────────────────────────

fn update_dir() -> PathBuf {
    crate::storage::lunac_root_dir().join("temp").join("update")
}

fn installer_path(version: &str) -> PathBuf {
    update_dir().join(format!("Lunac-{version}-Setup.exe"))
}

fn emit_progress(app: &AppHandle, phase: &str, downloaded: u64, total: u64) {
    let _ = app.emit(
        UPDATE_EVENT,
        UpdateProgress { phase: phase.to_string(), downloaded, total },
    );
}

/// 下载安装包到 `<exe 根>\temp\update\`，边下边算 sha256，下完校验。
/// **不写「已经下过就复用」的捷径**：半成品与旧版本都可能躺在那里（见开头的清理）。
fn download(app: &AppHandle, m: &UpdateManifest) -> Result<PathBuf, String> {
    let dir = update_dir();
    fs::create_dir_all(&dir).map_err(|e| {
        log::warn(format!("updater: create update dir failed: {e}"));
        "安装目录不可写 —— 请检查权限后重试".to_string()
    })?;
    // 这个目录只服务自更新一件事，所以整目录清空：
    // 既不留下上一次中断的半成品，也不让旧版本的安装包白占 110 MB。
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }

    let dest = installer_path(&m.version);
    let client = http_stream()?;
    let mut resp = client.get(&m.url).send().map_err(|e| net_hint("下载安装包", &e))?;
    if !resp.status().is_success() {
        log::warn(format!("updater: installer HTTP {}", resp.status()));
        return Err("安装包暂时取不到 —— 请稍后重试，或到 GitHub 手动下载".to_string());
    }

    let total = resp.content_length().unwrap_or(m.size);
    if total > MAX_INSTALLER_BYTES {
        log::warn(format!("updater: installer too large: {total}"));
        return Err("安装包体积异常 —— 请到 GitHub 手动下载".to_string());
    }

    let mut file = File::create(&dest).map_err(|e| {
        log::warn(format!("updater: create installer file failed: {e}"));
        "无法写入安装包 —— 请检查安装目录权限".to_string()
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut done: u64 = 0;
    let mut last_emit: u64 = 0;

    loop {
        let n = resp.read(&mut buf).map_err(|e| {
            log::warn(format!("updater: download read failed at {done} bytes: {e}"));
            let _ = fs::remove_file(&dest);
            "下载中断 —— 请检查网络后重试".to_string()
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| {
            log::warn(format!("updater: download write failed at {done} bytes: {e}"));
            let _ = fs::remove_file(&dest);
            "写入安装包失败 —— 请检查磁盘空间与权限".to_string()
        })?;
        done += n as u64;
        if done - last_emit >= PROGRESS_STEP {
            last_emit = done;
            emit_progress(app, "downloading", done, total);
        }
    }
    if let Err(e) = file.flush() {
        log::warn(format!("updater: flush failed: {e}"));
    }
    drop(file);
    emit_progress(app, "downloading", done, total);

    // 大小对不上：清单写了就用它兜一道（sha256 才是硬判据，这条只是早失败、早清理）
    if m.size > 0 && done != m.size {
        log::warn(format!("updater: size mismatch got={done} want={}", m.size));
        let _ = fs::remove_file(&dest);
        return Err("安装包不完整 —— 请重试".to_string());
    }

    emit_progress(app, "verifying", done, total);
    let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !got.eq_ignore_ascii_case(&m.sha256) {
        log::warn(format!("updater: sha256 mismatch got={got} want={}", m.sha256));
        let _ = fs::remove_file(&dest);
        return Err("安装包校验失败 —— 文件可能不完整，请重试".to_string());
    }
    log::info(format!("updater: downloaded {} ({} bytes)", dest.display(), done));
    Ok(dest)
}

// ── 交棒给安装器 ─────────────────────────────────────────────────

/// 拉起 `Setup.exe /S`。静默开关是 NSIS 的 `/S`（**大小写敏感**）。
///
/// **额外传 `/D=<当前 exe 所在目录>`**：NSIS 的 `/D` 直接设定 `$INSTDIR`，**优先于
/// `InstallDirRegKey` 读到的结果** ⇒ 更新恒定落回**正在运行的这个目录**。只靠注册表那条不够：
/// 0.9.7 之前的安装器从没写过 `InstallLocation`，所以**第一次自更新**会回落到缺省目录
/// `%LOCALAPPDATA%\Lunac`，把用户装成两份（新目录里一份旧数据都没有）。
///
/// `/D` 必须**是最后一个参数、且不能带引号**（NSIS 原话：`must not contain any quotes,
/// even if the path contains spaces`）⇒ 只能用 `raw_arg` 原样追加，不能让 `Command` 去转义。
fn spawn_silent_install(setup: &Path, install_dir: &Path) -> Result<(), String> {
    let mut cmd = std::process::Command::new(setup);
    cmd.arg("/S");
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.raw_arg(format!("/D={}", install_dir.display()));
    }
    #[cfg(not(target_os = "windows"))]
    let _ = install_dir;
    cmd.spawn()
        .map_err(|e| {
            log::warn(format!("updater: spawn installer failed: {e}"));
            "无法启动安装程序 —— 请手动运行那个安装包".to_string()
        })
        .map(|_| ())
}

/// **顺序不能反**（2026-09-30）：安装器一上来就 `taskkill /im lunac.exe`（见 NSI 核心段），
/// 若先拉安装器，我们会在清理做到一半时被强杀 —— 首当其冲的是 librespot：它不在安装器的
/// 杀名单里，会变成孤儿进程继续占着播放设备（下次开机它还占着，只能靠重启）。
/// 所以：先**有序**收掉自己的常驻进程，再拉安装器，最后退出。
fn handover(app: &AppHandle, setup: &Path) -> Result<(), String> {
    // 装到**我们正在运行的那个目录**（见 spawn_silent_install 的说明）。
    let install_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .ok_or_else(|| {
            log::warn("updater: cannot resolve current exe dir");
            "定位不了安装目录 —— 请手动运行那个安装包".to_string()
        })?;

    crate::cli_bridge::kill_and_cleanup();
    crate::agent_server::stop();
    crate::music::kill_librespot();

    spawn_silent_install(setup, &install_dir)?;
    emit_progress(app, "installing", 0, 0);
    // 退出即交棒：安装器会覆盖 lunac.exe，装完由 NSI 的 `.onInstSuccess` 把我们重新拉起。
    // 注意：**成功路径上前端永远收不到这个命令的 resolve** —— 这是设计，不是 bug。
    app.exit(0);
    Ok(())
}

// ── Tauri 命令 ───────────────────────────────────────────────────

#[tauri::command]
pub fn update_current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
pub fn update_config_get() -> UpdateConfig {
    load_config()
}

#[tauri::command]
pub fn update_config_set(config: UpdateConfig) -> Result<(), String> {
    save_config(&config)
}

/// 检查更新。**拿不到清单就是 Err**，由调用方决定要不要显示：
/// 启动时那次静默检查把 Err 丢掉（网络不通不该打扰用户），手动点按钮的那次如实显示。
#[tauri::command]
pub async fn update_check() -> Result<UpdateStatus, String> {
    run_blocking(|| {
        let current = env!("CARGO_PKG_VERSION").to_string();
        let manifest = fetch_manifest()?;
        validate_manifest(&manifest)?;
        let has_update = is_newer(&current, &manifest.version);
        log::info(format!(
            "updater: check current={current} latest={} has_update={has_update}",
            manifest.version
        ));
        Ok(UpdateStatus {
            current,
            has_update,
            latest: manifest.version,
            notes: manifest.notes,
            size: manifest.size,
        })
    })
    .await
}

/// 下载 + 校验 + 静默安装 + 退出。版本由**宿主自己重新拉一次清单**确认，
/// 不接受前端传来的版本号 —— 那是可以被改的输入，而这一步会把一个 exe 装进用户机器。
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    run_blocking(move || {
        // 开发构建不许自更新：它会往 `target\debug\` 里装一份安装版，把开发环境搅乱
        // （与 `auto_start` 拒绝注册开机项同一条理由，见 auto_start.rs 的 `is_dev_build`）。
        if crate::auto_start::is_dev_build() {
            log::warn("updater: refused — dev build");
            return Err("开发构建不支持自更新 —— 请改用安装版".to_string());
        }
        let current = env!("CARGO_PKG_VERSION").to_string();
        let manifest = fetch_manifest()?;
        validate_manifest(&manifest)?;
        if !is_newer(&current, &manifest.version) {
            return Err("已经是最新版本".to_string());
        }
        let setup = download(&app, &manifest)?;
        handover(&app, &setup)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing_accepts_repo_style_and_rejects_junk() {
        assert_eq!(parse_version("0.9.7"), Some([0, 9, 7]));
        assert_eq!(parse_version("v1.2.3"), Some([1, 2, 3]));
        assert_eq!(parse_version(" 0.9.7 "), Some([0, 9, 7]));
        // 只写两段：第三段补 0（本仓版本号一直是三段，这条是容错）
        assert_eq!(parse_version("1.4"), Some([1, 4, 0]));
        // 预发布 / 非数字：解析不出来 ⇒ 不认
        assert_eq!(parse_version("0.9.7-rc1"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("abc"), None);
    }

    #[test]
    fn newer_requires_strictly_greater_and_never_guesses() {
        assert!(is_newer("0.9.7", "0.9.8"));
        assert!(is_newer("0.9.7", "0.10.0"));
        assert!(is_newer("0.9.7", "1.0.0"));
        // 同版本 / 更旧版本：都不是更新（同版本那条尤其重要 —— 否则静默模式会装第二遍）
        assert!(!is_newer("0.9.7", "0.9.7"));
        assert!(!is_newer("0.9.8", "0.9.7"));
        // 任一侧解析不出来 ⇒ 一律 false，宁可漏报
        assert!(!is_newer("0.9.7", "0.9.8-rc1"));
        assert!(!is_newer("dev", "0.9.8"));
    }

    fn good_manifest() -> UpdateManifest {
        UpdateManifest {
            version: "0.9.8".into(),
            notes: "fix".into(),
            url: "https://github.com/LythrumMoon/Lunac/releases/download/v0.9.8/Lunac-0.9.8-Setup.exe".into(),
            sha256: "a".repeat(64),
            size: 123,
        }
    }

    #[test]
    fn manifest_validation_rejects_unsafe_sources() {
        assert!(validate_manifest(&good_manifest()).is_ok());

        // 明文 http
        let mut m = good_manifest();
        m.url = m.url.replace("https://", "http://");
        assert!(validate_manifest(&m).is_err());

        // 不是 GitHub 的域 —— 清单被改也换不到别处去
        let mut m = good_manifest();
        m.url = "https://evil.example.com/Lunac-0.9.8-Setup.exe".into();
        assert!(validate_manifest(&m).is_err());

        // sha256 缺失 / 长度不对 / 非十六进制
        for bad in ["", "abc", &"z".repeat(64)] {
            let mut m = good_manifest();
            m.sha256 = bad.to_string();
            assert!(validate_manifest(&m).is_err(), "sha256={bad} 应被拒");
        }

        // 版本号解析不出来
        let mut m = good_manifest();
        m.version = "latest".into();
        assert!(validate_manifest(&m).is_err());
    }

    /// 清单缺字段（老清单 / 生成脚本改了）不能 panic，缺的走默认值；
    /// 带 BOM 也照样能解析（走 `parse_json_lossy`，serde_json 自己是不认 BOM 的）。
    #[test]
    fn manifest_parsing_tolerates_bom_and_missing_fields() {
        let m: UpdateManifest = parse_json_lossy("\u{feff}{\"version\":\"0.9.8\"}").unwrap();
        assert_eq!(m.version, "0.9.8");
        assert_eq!(m.size, 0);
        assert!(m.sha256.is_empty());
        // 空的 sha256 过不了校验（见上一条测试）—— 缺字段不会崩，但装不了
        assert!(validate_manifest(&m).is_err());
        // 没剥 BOM 就该失败 —— 钉住「BOM 必须由 parse_json_lossy 处理」这个前提
        assert!(serde_json::from_str::<UpdateManifest>("\u{feff}{\"version\":\"0.9.8\"}").is_err());
    }

    /// 默认必须是「启动检查开、自动安装**开**」（2026-10-05 合并后的「自动更新」默认开）：
    /// 默认值就等于用户没做选择时的行为。
    #[test]
    fn default_config_enables_auto_update() {
        let c = UpdateConfig::default();
        assert!(c.check_on_startup);
        assert!(c.auto_install);
    }

    /// 配置的字段名与前端 `UpdateConfig` 一一对应（serde 直接序列化，改名要两边一起改）。
    #[test]
    fn config_uses_camel_case_on_the_wire() {
        let text = serde_json::to_string(&UpdateConfig::default()).unwrap();
        assert!(text.contains("\"checkOnStartup\""), "{text}");
        assert!(text.contains("\"autoInstall\""), "{text}");
    }
}
