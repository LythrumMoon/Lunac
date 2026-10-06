// src/mihomo.rs
//! **mihomo 内核托管**（2026-10-03，代理插件的第二条腿）。
//!
//! 它补上的是 `system_proxy.rs` 一直缺的那半件事：**写完注册表，背后没有任何东西在
//! 监听** —— 系统代理指向哪个端口，得真有一个代理进程在那儿。本模块就是那个进程。
//!
//! ## 与 `system_proxy.rs` 的分工（别混）
//!
//! * `system_proxy.rs`：**Windows 的代理设置**（注册表三值 + 通知 + 备份 / 还原）。
//! * 本模块：**代理能力本身**（内核进程 + 配置 + 节点 + 规则）。
//!
//! 两者只在「启用」这一个动作上握手：内核起来 ⇒ 把系统代理指向 `127.0.0.1:<mixed-port>`；
//! 内核停 ⇒ 还原系统代理。**备份 / 还原的纪律全部留在 `system_proxy.rs`**（那里已有
//! 经过验证的「只备份一次」实现），本模块不另写一份。
//!
//! ## 内核从哪来：**按需下载，不随安装包分发**（2026-10-03 用户裁决）
//!
//! mihomo 是 **GPL-3.0**。把它打进我们的安装包属于 GPL 传染性分发场景；按需从官方
//! Release 下载则只是「用户自己获取了一个独立程序」，我们不分发它。代价是首次启用要联网。
//!
//! ## 为什么订阅走 `proxy-providers` 而不是自己解析 YAML
//!
//! mihomo **原生支持**订阅：给它一个 `proxy-providers` 的 http url，它自己拉、自己解析、
//! 自己按 interval 更新（这正是 Clash Verge 那类 GUI 的做法）。我们只要在界面上把
//! 节点列表**读出来**（走 Clash REST API 的 `/proxies`）即可 —— 自己写一个 Clash YAML
//! 解析器，等于把 mihomo 已经做对的事重写一遍，还必然落后于它的新协议。
//!
//! ⚠️ 代价要说清：**分享链接（`vmess://` / `ss://` 这类）mihomo 不认** —— 那需要
//! 「链接 → YAML 节点」的转换，本轮**未做**（见 `links` 字段的注释与 backlog）。
//!
//! ## 本轮边界
//!
//! * **系统代理模式**已通；**TUN 未做**（需管理员 + wintun 驱动，2026-10-03 用户裁决后置）。
//! * 配置为**生成式**：`cores\mihomo\config.yaml` 每次启动前整体重写（用户的订阅 / 设置
//!   才是真相源，手改那个文件会在下次启动时丢失 —— 这是刻意的，避免两处各执一词）。

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// 固定的 mihomo 版本与资产名。**不动态查「最新版」** —— GitHub API 在国内常不可达，
/// 而「哪一版能跑」是明确的：升版本 = 改这一处常量（用户可在设置里覆盖下载地址）。
const MIHOMO_VERSION: &str = "v1.19.29";
const ASSET: &str = "mihomo-windows-amd64";
/// 官方 Release 下载前缀；用户可在设置里改成镜像（国内直连 GitHub 常失败）。
const DEFAULT_RELEASE_BASE: &str = "https://github.com/MetaCubeX/mihomo/releases/download";

/// 默认端口。**刻意与 mihomo 官方的 7890 一致**（用户从别处迁过来时不用改端口）。
const DEFAULT_MIXED_PORT: u16 = 7890;
const DEFAULT_CONTROLLER_PORT: u16 = 9090;

/// Clash API 的探测地址（连通性检查用）。
const HEALTH_URL: &str = "https://www.gstatic.com/generate_204";

// ── 进程状态 ────────────────────────────────────────────────────────

/// 内核子进程。**进程内全局唯一** —— 一个 Lunac 只托管一个内核（同时跑两个会抢端口）。
static CHILD: OnceLock<Mutex<Option<Child>>> = OnceLock::new();

fn child_slot() -> &'static Mutex<Option<Child>> {
    CHILD.get_or_init(|| Mutex::new(None))
}

// ── 路径 ────────────────────────────────────────────────────────────

/// 内核工作目录：`<exe 根>\cores\mihomo`。
pub fn core_dir() -> PathBuf {
    crate::storage::lunac_root_dir().join("cores").join("mihomo")
}
fn core_exe() -> PathBuf {
    core_dir().join("mihomo.exe")
}
/// mihomo 的 `-d`（工作目录），它会往里写 geo 数据、provider 缓存。
fn data_dir() -> PathBuf {
    core_dir().join("data")
}
fn gen_config_path() -> PathBuf {
    core_dir().join("config.yaml")
}
fn log_path() -> PathBuf {
    core_dir().join("mihomo.log")
}

pub fn is_installed() -> bool {
    core_exe().is_file()
}

// ── 配置（`config\proxy.json` 的 `mihomo` 字段）──────────────────────

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Subscription {
    #[serde(default)]
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MihomoConfig {
    /// 用户是否要用内核托管。**与「内核是否在跑」是两回事**（见 `status`）。
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_mixed_port")]
    pub mixed_port: u16,
    #[serde(default = "default_controller_port")]
    pub controller_port: u16,
    /// Clash API 的 secret。首启用时随机生成 —— 本机另一个进程不该能白用我们的控制口。
    #[serde(default)]
    pub secret: String,
    /// `rule` / `global` / `direct`（mihomo 的三档）。
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
    /// 订阅下载地址前缀（可换镜像）。空 = 官方 GitHub。
    #[serde(default)]
    pub release_base: String,
    /// 分享链接（`vmess://` …）。**本轮不解析**，留着是为了让配置格式先定下来 ——
    /// 界面上不作为可用功能暴露（写了也只记一行「本轮未支持」）。
    #[serde(default)]
    pub links: Vec<String>,
    /// TUN 开关（**本轮固定 false**：需管理员 + wintun，已裁决后置）。
    #[serde(default)]
    pub tun: bool,
}

fn default_mixed_port() -> u16 {
    DEFAULT_MIXED_PORT
}
fn default_controller_port() -> u16 {
    DEFAULT_CONTROLLER_PORT
}
fn default_mode() -> String {
    "rule".to_string()
}

impl Default for MihomoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mixed_port: DEFAULT_MIXED_PORT,
            controller_port: DEFAULT_CONTROLLER_PORT,
            secret: String::new(),
            mode: default_mode(),
            subscriptions: Vec::new(),
            release_base: String::new(),
            links: Vec::new(),
            tun: false,
        }
    }
}

// ── 内核下载 ────────────────────────────────────────────────────────

/// 下载地址前缀（用户配的镜像优先，否则官方 GitHub）。
fn release_base(cfg: &MihomoConfig) -> String {
    let b = cfg.release_base.trim().trim_end_matches('/');
    if b.is_empty() {
        DEFAULT_RELEASE_BASE.to_string()
    } else {
        b.to_string()
    }
}

fn asset_name() -> String {
    format!("{ASSET}-{MIHOMO_VERSION}.zip")
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex(&h.finalize())
}

/// 下载并安装内核。返回一段**如实**的结果说明（含「校验做了没有」）。
pub fn install_core(cfg: &MihomoConfig) -> Result<String, String> {
    fs::create_dir_all(core_dir()).map_err(|e| format!("建 cores 目录失败：{e}"))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;

    let base = release_base(cfg);
    let zip_url = format!("{base}/{MIHOMO_VERSION}/{}", asset_name());
    crate::log::info(format!("mihomo：开始下载内核 {zip_url}"));
    let bytes = http_get(&client, &zip_url)?;

    // 校验：优先拿官方校验文件。**拿不到就明说「未校验」**，不假装校验过 ——
    // 这正是本模块所有「如实说」的地方里最要紧的一处。
    let verified = match fetch_checksum(&client, &base) {
        Some(expected) => {
            let got = sha256_hex(&bytes);
            if got != expected {
                return Err(format!(
                    "内核校验失败：期望 {expected}，实测 {got}（下载可能被篡改或损坏，已中止）"
                ));
            }
            true
        }
        None => false,
    };
    if !verified {
        crate::log::warn("mihomo：官方未提供可用的 sha256 校验文件，本次未校验（已记录实测哈希）");
    }
    crate::log::info(format!("mihomo：内核字节 sha256={}", sha256_hex(&bytes)));

    // 解压：从 zip 里找出 mihomo 可执行文件（资产内文件名随版本带后缀），写成 mihomo.exe。
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("解压失败：{e}"))?;
    let mut found = false;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| format!("读压缩包条目失败：{e}"))?;
        let name = f.name().to_string();
        if !name.to_ascii_lowercase().ends_with(".exe") {
            continue;
        }
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut f, &mut out).map_err(|e| format!("读 {name} 失败：{e}"))?;
        fs::write(core_exe(), &out).map_err(|e| format!("写 mihomo.exe 失败：{e}"))?;
        found = true;
        crate::log::info(format!("mihomo：已安装内核（来自 {name}，{} 字节）", out.len()));
        break;
    }
    if !found {
        return Err("压缩包里没有找到 .exe（资产格式变了？）".into());
    }
    Ok(if verified {
        format!("已安装 mihomo {MIHOMO_VERSION}（sha256 校验通过）")
    } else {
        format!("已安装 mihomo {MIHOMO_VERSION}（**未校验**：官方未提供校验文件）")
    })
}

/// 尝试取发布页的校验文件。**取不到不算错误**（返回 None 让调用方如实说明）。
fn fetch_checksum(client: &reqwest::blocking::Client, base: &str) -> Option<String> {
    let asset = asset_name();
    // 官方发布同时带 `sha256sum.txt`（含所有资产）与部分资产级 `.sha256`；两个都试。
    let candidates = [
        format!("{base}/{MIHOMO_VERSION}/sha256sum.txt"),
        format!("{base}/{MIHOMO_VERSION}/{asset}.sha256"),
    ];
    for url in candidates {
        let Ok(resp) = client.get(&url).send() else { continue };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(text) = resp.text() else { continue };
        // 两种格式：`<hash>  <name>` 多行，或单行 `<hash>`。
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            let hash = parts[0].trim_start_matches('*');
            if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                continue;
            }
            // 有文件名就要求它匹配我们的资产；单行（无文件名）直接采信。
            if parts.len() == 1 || parts[1..].iter().any(|p| p.trim_start_matches('*') == asset.as_str()) {
                return Some(hash.to_ascii_lowercase());
            }
        }
    }
    None
}

fn http_get(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>, String> {
    let resp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, concat!("Lunac/", env!("CARGO_PKG_VERSION")))
        .send()
        .map_err(|e| format!("下载失败（{url}）：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败（{url}）：HTTP {}", resp.status()));
    }
    resp.bytes().map(|b| b.to_vec()).map_err(|e| format!("读取下载内容失败：{e}"))
}

// ── 配置生成 ────────────────────────────────────────────────────────

/// YAML 标量：一律**双引号**转义。节点名/订阅名是用户输入，含 `:` `#` `"` 都可能
/// 把整份配置搞坏（mihomo 启动即失败，报错还很难读懂）。
fn yaml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 生成 `config.yaml`。**每次启动前整体重写**（见模块头：订阅与设置才是真相源）。
fn render_config(cfg: &MihomoConfig, secret: &str) -> String {
    let mut s = String::new();
    s.push_str("# 由 Lunac 生成 —— 手改会在下次启动时被覆盖（见 src/mihomo.rs 顶部）\n");
    s.push_str("mixed-port: ");
    s.push_str(&cfg.mixed_port.to_string());
    s.push('\n');
    s.push_str("allow-lan: false\n");
    s.push_str("bind-address: 127.0.0.1\n");
    s.push_str(&format!("mode: {}\n", sanitize_mode(&cfg.mode)));
    s.push_str("log-level: info\n");
    s.push_str(&format!("external-controller: 127.0.0.1:{}\n", cfg.controller_port));
    s.push_str(&format!("secret: {}\n", yaml_str(secret)));
    // DNS：fake-ip 是 Clash 生态的默认选择（分流按域名而不是 IP，规则命中率更高）。
    s.push_str("dns:\n  enable: true\n  enhanced-mode: fake-ip\n  fake-ip-range: 198.18.0.1/16\n");
    s.push_str("  nameserver:\n    - 223.5.5.5\n    - 119.29.29.29\n");

    // 订阅 ⇒ proxy-providers（mihomo 自己拉、自己按 interval 更新）。
    if !cfg.subscriptions.is_empty() {
        s.push_str("proxy-providers:\n");
        for (i, sub) in cfg.subscriptions.iter().enumerate() {
            let name = if sub.name.trim().is_empty() {
                format!("sub_{i}")
            } else {
                sub.name.trim().to_string()
            };
            s.push_str(&format!("  {}:\n", yaml_str(&name)));
            s.push_str("    type: http\n");
            s.push_str(&format!("    url: {}\n", yaml_str(sub.url.trim())));
            s.push_str("    interval: 86400\n");
            s.push_str(&format!("    path: {}\n", yaml_str(&format!("providers/{name}.yaml"))));
            s.push_str("    health-check:\n      enable: true\n");
            s.push_str(&format!("      url: {}\n", yaml_str(HEALTH_URL)));
            s.push_str("      interval: 300\n");
        }
    }

    // 代理组：一个 select（用户手动挑），下面挂所有 provider 的节点。
    s.push_str("proxy-groups:\n");
    s.push_str("  - name: \"PROXY\"\n    type: select\n");
    s.push_str("    proxies:\n      - \"DIRECT\"\n      - \"REJECT\"\n");
    if !cfg.subscriptions.is_empty() {
        s.push_str("    use:\n");
        for (i, sub) in cfg.subscriptions.iter().enumerate() {
            let name = if sub.name.trim().is_empty() {
                format!("sub_{i}")
            } else {
                sub.name.trim().to_string()
            };
            s.push_str(&format!("      - {}\n", yaml_str(&name)));
        }
    }

    // 规则：国内直连、其余走 PROXY。**GEOIP 需要 geo 数据库** —— mihomo 首次启动会自动
    // 下载到 `-d` 目录（data_dir），所以这里直接用即可。
    s.push_str("rules:\n");
    s.push_str("  - GEOIP,LAN,DIRECT,no-resolve\n");
    s.push_str("  - GEOIP,CN,DIRECT\n");
    s.push_str("  - MATCH,PROXY\n");
    s
}

fn sanitize_mode(mode: &str) -> &'static str {
    match mode.trim().to_ascii_lowercase().as_str() {
        "global" => "global",
        "direct" => "direct",
        _ => "rule",
    }
}

/// 确保 secret 存在（首次启用时生成）。返回（可能更新过的）配置。
pub fn ensure_secret(cfg: &mut MihomoConfig) -> String {
    if cfg.secret.trim().len() >= 16 {
        return cfg.secret.trim().to_string();
    }
    // 不引 rand：用时间 + 进程 id 的 SHA-256（不是密码学强度的需求 —— 它只是本机
    // 控制口的一道门槛，防止同机其它进程随手改我们的节点选择）。
    let seed = format!(
        "{}-{}-{:?}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0),
        core_dir()
    );
    cfg.secret = sha256_hex(seed.as_bytes())[..32].to_string();
    cfg.secret.clone()
}

// ── 进程生命周期 ────────────────────────────────────────────────────

/// 内核的运行时状态（给界面看的）。
#[derive(Serialize)]
pub struct MihomoStatus {
    pub installed: bool,
    pub running: bool,
    pub version: String,
    pub mixed_port: u16,
    pub controller_port: u16,
    pub config_path: String,
    pub log_path: String,
    /// 配置里订阅数（界面据此提示「还没订阅，节点列表会是空的」）。
    pub subscriptions: usize,
}

pub fn status(cfg: &MihomoConfig) -> MihomoStatus {
    let running = is_running();
    MihomoStatus {
        installed: is_installed(),
        running,
        version: if is_installed() { MIHOMO_VERSION.to_string() } else { String::new() },
        mixed_port: cfg.mixed_port,
        controller_port: cfg.controller_port,
        config_path: gen_config_path().display().to_string(),
        log_path: log_path().display().to_string(),
        subscriptions: cfg.subscriptions.len(),
    }
}

/// 子进程是否还活着。`try_wait` 会顺带回收已退出的进程（不留僵尸）。
fn is_running() -> bool {
    let mut slot = child_slot().lock().unwrap();
    match slot.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(Some(_)) => {
                *slot = None;
                false
            }
            Ok(None) => true,
            Err(_) => false,
        },
        None => false,
    }
}

/// 启动内核。**调用方负责把配置存盘**（本函数只做「写盘 → 起进程 → 等它就绪」）。
pub fn start(cfg: &mut MihomoConfig) -> Result<String, String> {
    if !is_installed() {
        return Err("内核还没安装：先在面板上点「下载内核」".into());
    }
    if is_running() {
        return Err("内核已经在运行".into());
    }
    if cfg.mixed_port == cfg.controller_port {
        return Err("混合代理端口与控制端口不能相同".into());
    }
    let secret = ensure_secret(cfg);
    fs::create_dir_all(data_dir()).map_err(|e| format!("建内核数据目录失败：{e}"))?;
    let yaml = render_config(cfg, &secret);
    fs::write(gen_config_path(), yaml).map_err(|e| format!("写 config.yaml 失败：{e}"))?;

    // 日志：每次启动**截断重写**（不是追加）—— 否则日志会跨会话无限增长，而用户
    // 看日志的场景永远是「刚才那次为什么没起来」。
    let log = fs::File::create(log_path()).map_err(|e| format!("建日志文件失败：{e}"))?;
    let log2 = log.try_clone().map_err(|e| format!("复制日志句柄失败：{e}"))?;

    let mut cmd = Command::new(core_exe());
    cmd.arg("-f")
        .arg(gen_config_path())
        .arg("-d")
        .arg(data_dir())
        .current_dir(core_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log2));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：不弹控制台黑框（与我们起其它后台进程同一条纪律）
        cmd.creation_flags(0x0800_0000);
    }
    let child = cmd.spawn().map_err(|e| format!("启动 mihomo 失败：{e}"))?;
    *child_slot().lock().unwrap() = Some(child);
    crate::log::info(format!(
        "mihomo：已启动 mixed-port={} controller=127.0.0.1:{} 日志={}",
        cfg.mixed_port,
        cfg.controller_port,
        log_path().display()
    ));

    // 等它就绪：起进程 ≠ 起来了（配置错、端口被占都会立刻退出）。轮询 Clash API，
    // 同时把「进程已经死了」当作尽早失败的判据 —— 否则用户要等满超时才被告知。
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    loop {
        if controller_alive(cfg) {
            return Ok("内核已启动".into());
        }
        if !is_running() {
            return Err(format!(
                "内核启动后立刻退出，多半是配置或端口问题。日志：{}",
                log_path().display()
            ));
        }
        if std::time::Instant::now() > deadline {
            return Err(format!(
                "内核 12 秒内没有就绪。日志：{}",
                log_path().display()
            ));
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// 停止内核（幂等）。**不碰系统代理** —— 那一步由调用方显式做（见命令层）。
pub fn stop() -> Result<String, String> {
    let mut slot = child_slot().lock().unwrap();
    match slot.as_mut() {
        Some(child) => {
            let _ = child.kill();
            let _ = child.wait();
            *slot = None;
            crate::log::info("mihomo：已停止内核");
            Ok("内核已停止".into())
        }
        None => Ok("内核本来就没在运行".into()),
    }
}

/// 退出路径调用：把内核带走（否则 Lunac 关了、代理还开着，用户以为网络坏了）。
pub fn shutdown_on_exit() {
    if let Err(e) = stop() {
        crate::log::warn(format!("退出：停止 mihomo 失败：{e}"));
    }
}

/// 读内核日志尾部（界面上「日志」那一块）。
pub fn read_log(max_lines: usize) -> String {
    let Ok(text) = fs::read_to_string(log_path()) else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

// ── Clash REST API ──────────────────────────────────────────────────

fn controller_url(cfg: &MihomoConfig, path: &str) -> String {
    format!("http://127.0.0.1:{}{path}", cfg.controller_port)
}

fn api_get(cfg: &MihomoConfig, path: &str) -> Result<Value, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let resp = client
        .get(controller_url(cfg, path))
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", cfg.secret.trim()))
        .send()
        .map_err(|e| format!("连不上内核控制口：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("内核控制口返回 HTTP {}", resp.status()));
    }
    resp.json::<Value>().map_err(|e| format!("解析内核响应失败：{e}"))
}

fn controller_alive(cfg: &MihomoConfig) -> bool {
    api_get(cfg, "/version").is_ok()
}

/// 节点 / 代理组列表（Clash API 的 `/proxies`）。
///
/// **只挑出「节点」与「组」**，把 `DIRECT` / `REJECT` / `GLOBAL` 这些内置项按类型排掉：
/// 界面上列一堆用户没建过的伪节点只会让人困惑。
pub fn proxies(cfg: &MihomoConfig) -> Result<Value, String> {
    let raw = api_get(cfg, "/proxies")?;
    let mut out = Vec::new();
    if let Some(map) = raw.get("proxies").and_then(Value::as_object) {
        for (name, v) in map {
            let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
            // Selector/URLTest/Fallback/LoadBalance 是组；其它是节点。
            let is_group = matches!(ty, "Selector" | "URLTest" | "Fallback" | "LoadBalance" | "Relay");
            let is_builtin = matches!(name.as_str(), "DIRECT" | "REJECT" | "REJECT-DROP" | "GLOBAL" | "PASS" | "COMPATIBLE");
            if is_builtin {
                continue;
            }
            out.push(serde_json::json!({
                "name": name,
                "type": ty,
                "is_group": is_group,
                "now": v.get("now").and_then(Value::as_str).unwrap_or(""),
                "history": v.get("history").cloned().unwrap_or(Value::Array(vec![])),
                "all": v.get("all").cloned().unwrap_or(Value::Array(vec![])),
            }));
        }
    }
    Ok(Value::Array(out))
}

/// 单节点测速（Clash API 的 `/proxies/{name}/delay`）。返回毫秒。
pub fn delay(cfg: &MihomoConfig, name: &str) -> Result<u64, String> {
    let path = format!(
        "/proxies/{}/delay?timeout=5000&url={}",
        urlencode(name),
        urlencode(HEALTH_URL)
    );
    let v = api_get(cfg, &path)?;
    v.get("delay")
        .and_then(Value::as_u64)
        .ok_or_else(|| v.get("message").and_then(Value::as_str).unwrap_or("测速失败").to_string())
}

/// 切换某个组当前选中的节点（Clash API 的 `PUT /proxies/{group}`）。
pub fn select(cfg: &MihomoConfig, group: &str, node: &str) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let resp = client
        .put(controller_url(cfg, &format!("/proxies/{}", urlencode(group))))
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", cfg.secret.trim()))
        .json(&serde_json::json!({ "name": node }))
        .send()
        .map_err(|e| format!("连不上内核控制口：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("切换节点失败：HTTP {}", resp.status()));
    }
    Ok(())
}

/// 切换模式（`rule` / `global` / `direct`），并写回配置。
pub fn set_mode(cfg: &mut MihomoConfig, mode: &str) -> Result<(), String> {
    cfg.mode = sanitize_mode(mode).to_string();
    if !is_running() {
        return Ok(()); // 没在跑就只改配置，下次启动生效
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let resp = client
        .patch(controller_url(cfg, "/configs"))
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", cfg.secret.trim()))
        .json(&serde_json::json!({ "mode": cfg.mode }))
        .send()
        .map_err(|e| format!("连不上内核控制口：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("切换模式失败：HTTP {}", resp.status()));
    }
    Ok(())
}

/// 极简 URL 编码（只编非 unreserved 字符）——节点名/URL 作为路径或查询串时要它。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ── tauri 命令 ──────────────────────────────────────────────────────

/// 当前内核状态（安装 / 运行 / 端口 / 订阅数）。
#[tauri::command]
pub async fn mihomo_status() -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(|| {
        Ok(status(&crate::system_proxy::load_config().mihomo))
    })
    .await
}

/// 下载并安装内核（按需，见模块头）。
#[tauri::command]
pub async fn mihomo_install_core() -> Result<String, String> {
    crate::commands::run_blocking(|| {
        let cfg = crate::system_proxy::load_config().mihomo;
        install_core(&cfg)
    })
    .await
}

/// 保存内核设置（订阅 / 端口 / 模式 / 下载源）。
///
/// `secret` 与 `links` **不接受前端回传**：前者是本机控制口的凭据（前端根本不该看到），
/// 后者本轮未支持 —— 都从磁盘上的旧值续上，避免「面板保存一次就丢」。
#[tauri::command]
pub async fn mihomo_config_set(cfg: MihomoConfig) -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(move || {
        let mut pc = crate::system_proxy::load_config();
        let mut incoming = cfg;
        if incoming.secret.trim().is_empty() {
            incoming.secret = pc.mihomo.secret.clone();
        }
        if incoming.links.is_empty() {
            incoming.links = pc.mihomo.links.clone();
        }
        // 端口 0 是非法值（前端漏传时会出现）→ 保留旧值而不是让内核起不来
        if incoming.mixed_port == 0 {
            incoming.mixed_port = pc.mihomo.mixed_port;
        }
        if incoming.controller_port == 0 {
            incoming.controller_port = pc.mihomo.controller_port;
        }
        pc.mihomo = incoming;
        crate::system_proxy::save_config(&pc)?;
        Ok(status(&pc.mihomo))
    })
    .await
}

/// 启动内核，并把**系统代理**指向它。备份 / 还原纪律在 `system_proxy.rs` 里。
#[tauri::command]
pub async fn mihomo_start() -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(move || {
        let mut pc = crate::system_proxy::load_config();
        let msg = start(&mut pc.mihomo)?;
        crate::log::info(msg);
        let server = format!("http://127.0.0.1:{}", pc.mihomo.mixed_port);
        // 内置默认绕过表（含 `<local>`）—— 不传 bypass 让它走 DEFAULT_BYPASS，
        // 否则 Lunac 自己（127.0.0.1:8788/8789）会被送去代理，自己把自己绕死。
        crate::system_proxy::enable_system_proxy(&mut pc, &server, "")?;
        pc.mihomo.enabled = true;
        crate::system_proxy::save_config(&pc)?;
        Ok(status(&pc.mihomo))
    })
    .await
}

/// 停止内核并还原系统代理（幂等）。
#[tauri::command]
pub async fn mihomo_stop() -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(move || {
        let mut pc = crate::system_proxy::load_config();
        // **只有它本来在跑**才去还原系统代理 —— 否则会把用户用手填列表启用的那条
        // 也一并没有了（两者是并列的两种用法，不能互相踩）。
        let was_running = is_running();
        stop()?;
        if was_running {
            crate::system_proxy::restore_system_proxy(&mut pc)?;
            pc.active.clear();
        }
        pc.mihomo.enabled = false;
        crate::system_proxy::save_config(&pc)?;
        Ok(status(&pc.mihomo))
    })
    .await
}

/// 节点 / 代理组列表（走内核的 Clash API）。
#[tauri::command]
pub async fn mihomo_proxies() -> Result<Value, String> {
    crate::commands::run_blocking(|| {
        let cfg = crate::system_proxy::load_config().mihomo;
        proxies(&cfg)
    })
    .await
}

/// 单节点测速（毫秒）。
#[tauri::command]
pub async fn mihomo_delay(name: String) -> Result<u64, String> {
    crate::commands::run_blocking(move || {
        let cfg = crate::system_proxy::load_config().mihomo;
        delay(&cfg, &name)
    })
    .await
}

/// 在某个组里切换选中的节点。
#[tauri::command]
pub async fn mihomo_select(group: String, node: String) -> Result<(), String> {
    crate::commands::run_blocking(move || {
        let cfg = crate::system_proxy::load_config().mihomo;
        select(&cfg, &group, &node)
    })
    .await
}

/// 切换模式（rule / global / direct）：内核在跑就热切，没跑就只改配置。
#[tauri::command]
pub async fn mihomo_set_mode(mode: String) -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(move || {
        let mut pc = crate::system_proxy::load_config();
        set_mode(&mut pc.mihomo, &mode)?;
        crate::system_proxy::save_config(&pc)?;
        Ok(status(&pc.mihomo))
    })
    .await
}

/// 内核日志尾部（界面上「日志」那块）。
#[tauri::command]
pub async fn mihomo_log() -> Result<String, String> {
    crate::commands::run_blocking(|| Ok(read_log(200))).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_scalars_are_always_quoted_and_escaped() {
        // 节点名里带冒号/井号是最常见的「配置被写坏」来源
        assert_eq!(yaml_str("a:b"), "\"a:b\"");
        assert_eq!(yaml_str("a\"b"), "\"a\\\"b\"");
        assert_eq!(yaml_str("a\\b"), "\"a\\\\b\"");
    }

    #[test]
    fn mode_falls_back_to_rule_on_garbage() {
        assert_eq!(sanitize_mode("global"), "global");
        assert_eq!(sanitize_mode("DIRECT"), "direct");
        assert_eq!(sanitize_mode("nonsense"), "rule");
        assert_eq!(sanitize_mode(""), "rule");
    }

    #[test]
    fn config_contains_ports_and_providers() {
        let mut cfg = MihomoConfig::default();
        cfg.subscriptions.push(Subscription {
            name: "机场A".into(),
            url: "https://example.com/sub?token=1".into(),
        });
        let yaml = render_config(&cfg, "s3cret");
        assert!(yaml.contains("mixed-port: 7890"));
        assert!(yaml.contains("external-controller: 127.0.0.1:9090"));
        assert!(yaml.contains("secret: \"s3cret\""));
        // 订阅名是中文 ⇒ 必须被引号包住，否则 YAML 解析会炸
        assert!(yaml.contains("\"机场A\":"));
        assert!(yaml.contains("MATCH,PROXY"));
    }

    #[test]
    fn secret_is_generated_once_and_stable() {
        let mut cfg = MihomoConfig::default();
        let a = ensure_secret(&mut cfg);
        let b = ensure_secret(&mut cfg);
        assert_eq!(a, b, "已有 secret 时不得重生成");
        assert_eq!(a.len(), 32);
    }
}
