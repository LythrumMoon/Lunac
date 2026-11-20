// src/mihomo.rs
//! **代理配置 + 自研内核的生命周期**（2026-10-03 建立；2026-11-01 收口为 native-only）。
//!
//! ## 现在这里还管什么
//!
//! * `config\proxy.json` 的 `mihomo` 段：订阅 / 手填分享链接 / 模式 / 端口 / `engine`。
//! * 内核的启停、状态、系统代理的指向与还原（备份 / 还原纪律全在 `system_proxy.rs`）。
//! * 节点列表 / 测速 / 切换 —— 全部转发给 `native_proxy`。
//!
//! ## 不再管什么（2026-11-01，§待办 3-26 K3/K4）
//!
//! mihomo 子进程、它的 `config.yaml`、Clash REST 控制口、以及「下载内核」这条路**已全部删除**：
//!
//! * 缺省内核 = `native`（编进宿主，见 `src/native_proxy/`），新安装不需要任何外部文件；
//! * 旧配置里**显式**写着 `engine: "mihomo"` 时，启动 / 切换都会返回一句可操作的错误
//!   （[`LEGACY_MIHOMO`]），**不静默降级**，也不假装还能跑；
//! * 面板上不再有「下载内核」入口，日志页读的是**宿主日志**（自研引擎的日志就在那里）。
//!
//! ## 与 `system_proxy.rs` 的分工（别混）
//!
//! * `system_proxy.rs`：**Windows 的代理设置**（注册表三值 + 通知 + 备份 / 还原）。
//! * 本模块：**代理能力本身**（内核进程 + 配置 + 节点）。
//!
//! 两者只在「启用」这一个动作上握手：内核起来 ⇒ 把系统代理指向 `127.0.0.1:<mixed-port>`；
//! 内核停 ⇒ 还原系统代理。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 默认混合端口。**刻意与 mihomo 的 7890 一致**（用户从别处迁过来时不用改端口）。
///
/// 端口不由用户配置：`native_proxy::start` 在这个默认值被占用时会自动挑一个空闲端口并
/// 写回配置 —— 面板只展示实际端口，不给输入框。
const DEFAULT_MIXED_PORT: u16 = 7890;

/// 保留字段的缺省值（Clash 控制口已随 mihomo 退场，仅用于配置格式兼容）。
const DEFAULT_CONTROLLER_PORT: u16 = 9090;

/// 旧配置显式写了 `engine: "mihomo"` 时的唯一提示。**启动 / 切换两条路都用它**，
/// 免得两处文案漂移。
const LEGACY_MIHOMO: &str = "当前版本已不再内置 mihomo 内核：请把 config\\proxy.json 的 engine 改成 native（或删掉该字段）后重启";

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
    /// Clash 控制口端口。**已不再使用**（K3 删掉了控制口），保留是为了老配置能原样读回。
    #[serde(default = "default_controller_port")]
    pub controller_port: u16,
    /// Clash API secret。**已不再使用**，同样只为老配置兼容。
    #[serde(default)]
    pub secret: String,
    /// `rule` / `global` / `direct`：自研引擎用它决定规则回退（启动时展开，运行中不可热切）。
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
    /// 分享链接（`ss://` / `trojan://` / `vless://` / `vmess://` / `anytls://` / `hysteria2://`）。
    /// **解析只走 `native_proxy::clash::parse_link`**（单一真相源，与真正联网的是同一套）。
    #[serde(default)]
    pub links: Vec<String>,
    /// 规则模式下的**名单语义**：`blacklist`（名单内直连）/ `whitelist`（名单内强制代理）。
    /// 只在 `mode = rule` 时生效；订阅规则与本机网段规则仍在两侧生效。
    #[serde(default = "default_rule_mode")]
    pub rule_mode: String,
    /// 规则名单域名（一行一个，`DOMAIN-SUFFIX` 匹配）。默认给一份常见国内站点清单。
    #[serde(default = "default_rule_domains")]
    pub rule_domains: Vec<String>,
    /// TUN 开关（**固定 false**：需管理员 + wintun，已裁决后置）。
    #[serde(default)]
    pub tun: bool,
    /// 用哪个内核：`native`（缺省，自研） / `mihomo`（**已不再内置**，显式写死时启动即报错）。
    ///
    /// 空串 = 沿用磁盘上的旧值（前端没有这个控件时不该把它改回缺省）；
    /// 新配置落盘时按缺省 = `native` 处理（见 `engine_name`）。
    #[serde(default)]
    pub engine: String,
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
fn default_rule_mode() -> String {
    "blacklist".to_string()
}

/// 默认名单 = 常见国内站点（**黑名单**语义：这些域名直连，其余按订阅规则走代理）。
///
/// 刻意只放「按域名能判断」的条目，不引 GEOIP 数据库（自研引擎第一版不引）。
/// `cn` 用后缀规则覆盖所有 `.cn`；其余是主要站点及其 CDN 域名。
fn default_rule_domains() -> Vec<String> {
    const LIST: &[&str] = &[
        "cn",
        "baidu.com",
        "bdstatic.com",
        "bdimg.com",
        "qq.com",
        "gtimg.com",
        "qpic.cn",
        "tencent.com",
        "weixin.qq.com",
        "weixin.com",
        "wechat.com",
        "qcloud.com",
        "myqcloud.com",
        "qcloudimg.com",
        "tenpay.com",
        "bilibili.com",
        "biliapi.net",
        "hdslb.com",
        "bilivideo.com",
        "taobao.com",
        "tmall.com",
        "alicdn.com",
        "alipay.com",
        "alipayobjects.com",
        "alibaba.com",
        "aliyun.com",
        "aliyuncs.com",
        "jd.com",
        "jd.hk",
        "360buyimg.com",
        "sina.com.cn",
        "sina.cn",
        "weibo.com",
        "weibo.cn",
        "sohu.com",
        "163.com",
        "126.com",
        "126.net",
        "netease.com",
        "yeah.net",
        "youku.com",
        "iqiyi.com",
        "qiyi.com",
        "douyin.com",
        "iesdouyin.com",
        "bytedance.com",
        "toutiao.com",
        "snssdk.com",
        "pstatp.com",
        "kuaishou.com",
        "gifshow.com",
        "xiaohongshu.com",
        "xhscdn.com",
        "zhihu.com",
        "zhimg.com",
        "douban.com",
        "doubanio.com",
        "meituan.com",
        "meituan.net",
        "dianping.com",
        "ctrip.com",
        "qunar.com",
        "12306.cn",
        "xiaomi.com",
        "mi.com",
        "miui.com",
        "huawei.com",
        "hicloud.com",
        "oppo.com",
        "vivo.com",
        "lenovo.com.cn",
        "pinduoduo.com",
        "yangkeduo.com",
        "suning.com",
        "ele.me",
        "amap.com",
        "autonavi.com",
        "kugou.com",
        "kuwo.cn",
        "ximalaya.com",
        "csdn.net",
        "cnblogs.com",
        "gitee.com",
        "oschina.net",
        "yuque.com",
        "wps.cn",
        "kingsoft.com",
    ];
    LIST.iter().map(|s| (*s).to_string()).collect()
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
            links: Vec::new(),
            rule_mode: default_rule_mode(),
            rule_domains: default_rule_domains(),
            tun: false,
            engine: "native".to_string(),
        }
    }
}

/// 归一化后的内核名（对外只有 `mihomo` / `native` 两个值）。
///
/// **缺省 = `native`**（§待办 3-26 K1）：新配置 / 旧配置里没有这个字段时都走自研。
/// 只有**显式**写了 `mihomo` 的配置会拿到 `mihomo` —— 那条路现在只剩「如实报错」。
pub fn engine_name(cfg: &MihomoConfig) -> &'static str {
    match cfg.engine.trim().to_ascii_lowercase().as_str() {
        "mihomo" => "mihomo",
        _ => "native",
    }
}

/// 模式归一化（面板只给三档；手改配置文件写了别的值就回落到 `rule`）。
fn sanitize_mode(mode: &str) -> &'static str {
    match mode.trim().to_ascii_lowercase().as_str() {
        "global" => "global",
        "direct" => "direct",
        _ => "rule",
    }
}

// ── 状态 ────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct NodeView {
    pub index: usize,
    pub name: String,
    pub kind: String,
}

#[derive(Serialize)]
pub struct MihomoStatus {
    /// 自研内核编进宿主 ⇒ 对 native 恒为 true；显式 mihomo 配置为 false（它已不存在）。
    pub installed: bool,
    /// **引擎**（本机监听口）是否在跑。打开代理面板就会静默拉起它，用于测延迟；
    /// 它与下面的 `enabled`（服务 / 系统代理）是两件事（2026-11-03 拆开）。
    pub running: bool,
    /// **服务**（系统代理）是否开启。面板的开关与搜索区状态泡泡框都看这个值。
    pub enabled: bool,
    pub version: String,
    pub mixed_port: u16,
    pub controller_port: u16,
    pub config_path: String,
    pub log_path: String,
    /// 配置里订阅数（界面据此提示「还没订阅，节点列表会是空的」）。
    pub subscriptions: usize,
    /// 手填节点（分享链接）。顺序与 `links` 一致。
    pub nodes: Vec<NodeView>,
    /// 当前生效的内核：`native` / `mihomo`（后者只会在旧配置上出现）。
    pub engine: String,
    /// 当前模式（配置值；运行中改不了）。
    pub mode: String,
    /// 配置里的 TUN 开关（引擎未支持 TUN，恒为 false）。
    pub tun: bool,
    /// 运行中的累计流量与连接数（界面用它算速率）。
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub connections: u64,
    pub active: u64,
    /// 当前选中的节点名（没跑时为空）。
    pub selected: String,
}

pub fn status(cfg: &MihomoConfig) -> MihomoStatus {
    let engine = engine_name(cfg);
    let st = crate::native_proxy::status();
    MihomoStatus {
        // native 不需要安装；mihomo 已不再内置 —— 两者都不能让界面出现「下载内核」。
        installed: engine == "native",
        running: is_running(),
        enabled: cfg.enabled,
        version: if engine == "native" {
            "native".to_string()
        } else {
            String::new()
        },
        mixed_port: cfg.mixed_port,
        controller_port: cfg.controller_port,
        // 不再生成 config.yaml（K3）；留空而不是给一个不存在的路径。
        config_path: String::new(),
        log_path: latest_host_log()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        subscriptions: cfg.subscriptions.len(),
        nodes: manual_nodes(cfg),
        engine: engine.to_string(),
        mode: cfg.mode.clone(),
        tun: false,
        bytes_up: st.bytes_up,
        bytes_down: st.bytes_down,
        connections: st.connections,
        active: st.active,
        selected: st.selected,
    }
}

/// 当前内核是否在跑（只有自研引擎这一种可能）。
pub fn is_running() -> bool {
    crate::native_proxy::is_running()
}

/// 把 `links` 里的每条分享链接解析成可渲染的节点。**顺序与 `links` 一一对应**
/// （界面按这个下标删除）；解析失败的那条以 `kind = "invalid"` 占位，让用户能在界面上删掉它。
fn manual_nodes(cfg: &MihomoConfig) -> Vec<NodeView> {
    cfg.links
        .iter()
        .enumerate()
        .map(|(index, link)| match crate::native_proxy::clash::parse_link(link) {
            Ok(node) => NodeView {
                index,
                name: node.name,
                kind: node.kind.label().to_string(),
            },
            Err(_) => NodeView {
                index,
                name: String::new(),
                kind: "invalid".to_string(),
            },
        })
        .collect()
}

// ── 日志（读宿主日志；自研引擎的日志就在那里）─────────────────────────

/// 最新一份宿主日志（`temp\logs\*.log`，按修改时间取最新的那份）。
fn latest_host_log() -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(crate::log::log_dir()).ok()?.flatten() {
        let path = entry.path();
        let is_log = path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("log"));
        if !is_log {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, p)| p)
}

/// 宿主日志尾部（代理插件「日志」页）。
pub fn read_log(max_lines: usize) -> String {
    let Some(path) = latest_host_log() else {
        return String::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

// ── 启停 ────────────────────────────────────────────────────────────

/// 启动内核。**调用方负责把配置存盘**。
///
/// 显式 `engine: "mihomo"` ⇒ 直接返回 [`LEGACY_MIHOMO`]（K4：不静默降级）；
/// 其余一律走自研引擎（`native_proxy::start`）。
pub fn start(cfg: &mut MihomoConfig) -> Result<String, String> {
    if engine_name(cfg) == "mihomo" {
        return Err(LEGACY_MIHOMO.to_string());
    }
    crate::native_proxy::start(cfg)
}

/// 停止内核（幂等）。**不碰系统代理** —— 那一步由命令层显式做。
pub fn stop() -> Result<String, String> {
    crate::native_proxy::stop()
}

/// 退出路径调用：把内核带走（否则 Lunac 关了、代理还开着，用户以为网络坏了）。
pub fn shutdown_on_exit() {
    if let Err(e) = stop() {
        crate::log::warn(format!("退出：停止内核失败：{e}"));
    }
}

// ── tauri 命令 ──────────────────────────────────────────────────────

/// 当前内核状态（安装 / 运行 / 端口 / 订阅数 / 节点）。
#[tauri::command]
pub async fn mihomo_status() -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(|| Ok(status(&crate::system_proxy::load_config().mihomo))).await
}

/// 保存内核设置（订阅 / 模式）。端口不再由用户配置：
/// 前端传 `0`，宿主保留旧值（端口最终由 `native_proxy::start` 决定）。
///
/// `secret` 与 `links` **不接受前端回传**：前者是旧控制口的凭据（前端根本不该看到），
/// 后者由 `mihomo_node_add` / `mihomo_node_del` 单独管理 —— 都从磁盘上的旧值续上，
/// 避免「保存订阅一次就把手填节点清空」。
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
        // 显式传 mihomo = 旧客户端 / 手改配置：本版本不再支持，直接拒绝，
        // **不要**静默改写成 native（用户会以为自己的配置被吃了）。
        if incoming.engine.trim().eq_ignore_ascii_case("mihomo") {
            return Err(LEGACY_MIHOMO.to_string());
        }
        // 前端没有内核选择控件时 `engine` 是空串 ⇒ 沿用旧值。
        if incoming.engine.trim().is_empty() {
            incoming.engine = pc.mihomo.engine.clone();
        }
        // 端口 0 = 「由宿主决定」→ 保留旧值；真正挑空闲端口发生在启动时。
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

/// 添加一个手填节点（分享链接）。先解析校验，不合法**一个字都不写**。
///
/// 校验器 = `native_proxy::clash::parse_link` —— 与真正联网用的是同一套解析，
/// 避免「界面说能加、引擎却连不上」这种两套真相源。
#[tauri::command]
pub async fn mihomo_node_add(link: String) -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(move || {
        let link = link.trim().to_string();
        crate::native_proxy::clash::parse_link(&link)?;
        let mut pc = crate::system_proxy::load_config();
        if pc.mihomo.links.iter().any(|l| l.trim() == link) {
            return Err("这条节点已经在列表里了".into());
        }
        pc.mihomo.links.push(link);
        crate::system_proxy::save_config(&pc)?;
        Ok(status(&pc.mihomo))
    })
    .await
}

/// 自定义服务器（2026-11-01 代理面板）：表单 → 分享链接 → 与手填节点同一条存储路径。
///
/// 凭据对 ss / trojan / anytls / hysteria2 是密码，对 vless / vmess 是 UUID；
/// ss 另可选加密方式（缺省 `aes-256-gcm`）。拼完由 `build_custom_link` 自证可解析。
#[tauri::command]
pub async fn mihomo_node_add_custom(
    name: String,
    server: String,
    port: u16,
    kind: String,
    credential: String,
    cipher: Option<String>,
) -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(move || {
        let link = crate::native_proxy::clash::build_custom_link(
            &name,
            &server,
            port,
            &kind,
            &credential,
            cipher.as_deref().unwrap_or(""),
        )?;
        let mut pc = crate::system_proxy::load_config();
        if pc.mihomo.links.iter().any(|l| l.trim() == link) {
            return Err("这条节点已经在列表里了".into());
        }
        pc.mihomo.links.push(link);
        crate::system_proxy::save_config(&pc)?;
        Ok(status(&pc.mihomo))
    })
    .await
}

/// 删除一个手填节点（按 `status().nodes[].index`）。
#[tauri::command]
pub async fn mihomo_node_del(index: usize) -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(move || {
        let mut pc = crate::system_proxy::load_config();
        if index >= pc.mihomo.links.len() {
            return Err("这个节点已经不在列表里了".into());
        }
        pc.mihomo.links.remove(index);
        crate::system_proxy::save_config(&pc)?;
        Ok(status(&pc.mihomo))
    })
    .await
}

/// 单独开关**系统代理**（内核保持运行）。
///
/// `mihomo_start` 会把系统代理一并指向内核（首次启用最自然的动作）；这个命令让用户
/// 在不停止内核的前提下把系统代理关掉 / 再打开 —— 面板上那两个开关因此互相独立。
#[tauri::command]
pub async fn mihomo_system_proxy_set(enabled: bool) -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(move || {
        let mut pc = crate::system_proxy::load_config();
        if enabled {
            if !is_running() {
                return Err("内核还没运行：先启用内核".into());
            }
            if pc.mihomo.mixed_port == 0 {
                return Err("内核端口还没分配：重启一次内核".into());
            }
            let server = format!("http://127.0.0.1:{}", pc.mihomo.mixed_port);
            // 内置默认绕过表（含 `<local>`）—— 不传 bypass 让它走 DEFAULT_BYPASS，
            // 否则 Lunac 自己（127.0.0.1:8788/8789）会被送去代理，自己把自己绕死。
            crate::system_proxy::enable_system_proxy(&mut pc, &server, "")?;
        } else {
            crate::system_proxy::restore_system_proxy(&mut pc)?;
        }
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
        // 打开面板时会静默拉起引擎（2026-11-03）⇒ 这里必须容忍「已经跑着」，
        // 否则用户点「开启服务」会报「引擎已经在运行」，服务永远开不起来。
        if is_running() {
            crate::log::info("代理：引擎已就绪，直接开启服务");
        } else {
            let msg = start(&mut pc.mihomo)?;
            crate::log::info(msg);
        }
        let server = format!("http://127.0.0.1:{}", pc.mihomo.mixed_port);
        crate::system_proxy::enable_system_proxy(&mut pc, &server, "")?;
        pc.mihomo.enabled = true;
        crate::system_proxy::save_config(&pc)?;
        Ok(status(&pc.mihomo))
    })
    .await
}

/// **关闭服务**（还原系统代理）但**保留引擎**（幂等）。
///
/// 2026-11-03 拆开「引擎」与「服务」：面板开着时要能继续测节点延迟，所以关掉服务
/// 不再顺手杀引擎；引擎的真正收尾点是「面板关闭且服务未开」（`mihomo_engine_stop`）
/// 与 App 退出（`shutdown_on_exit`）。
#[tauri::command]
pub async fn mihomo_stop() -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(move || {
        let mut pc = crate::system_proxy::load_config();
        // 没有备份时它是空操作（不会抹掉用户自己的代理设置），所以无需先判是否在跑。
        crate::system_proxy::restore_system_proxy(&mut pc)?;
        pc.active.clear();
        pc.mihomo.enabled = false;
        crate::system_proxy::save_config(&pc)?;
        Ok(status(&pc.mihomo))
    })
    .await
}

/// **静默启动引擎**：只开本机监听口，不动系统代理、不改「服务开关」。
///
/// 用途 = 打开代理面板时先拉起它，这样「测试连通性」不必先开启服务。
/// 已在跑 ⇒ 直接返回当前状态（幂等）。
#[tauri::command]
pub async fn mihomo_engine_start() -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(move || {
        let mut pc = crate::system_proxy::load_config();
        if !is_running() {
            let msg = start(&mut pc.mihomo)?;
            crate::log::info(msg);
            // `start` 会回填实际监听端口（0 = 由宿主挑）⇒ 必须落盘，
            // 否则下一次 `status` 报的还是旧端口。
            crate::system_proxy::save_config(&pc)?;
        }
        Ok(status(&pc.mihomo))
    })
    .await
}

/// **只停止引擎**（幂等）。调用方保证此时服务已关（否则等于偷偷断网）。
#[tauri::command]
pub async fn mihomo_engine_stop() -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(move || {
        if is_running() {
            stop()?;
        }
        Ok(status(&crate::system_proxy::load_config().mihomo))
    })
    .await
}

/// 切换内核。**必须先停用**。
///
/// 只有 `native` 能切过去；`mihomo` 已不再内置 ⇒ 返回 [`LEGACY_MIHOMO`]
/// （给旧配置一条明确的出路：改成 native 后重启）。
#[tauri::command]
pub async fn mihomo_set_engine(engine: String) -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(move || {
        let wanted = engine.trim().to_ascii_lowercase();
        if wanted != "native" {
            return Err(if wanted == "mihomo" {
                LEGACY_MIHOMO.to_string()
            } else {
                format!("不认识的内核：{wanted}")
            });
        }
        if is_running() {
            return Err("先停用当前内核，再切换".into());
        }
        let mut pc = crate::system_proxy::load_config();
        pc.mihomo.engine = "native".to_string();
        crate::system_proxy::save_config(&pc)?;
        crate::log::info("proxy：内核切换为 native");
        Ok(status(&pc.mihomo))
    })
    .await
}

/// 节点列表（自研引擎：节点即订阅 / 手填链接展开后的结果）。
#[tauri::command]
pub async fn mihomo_proxies() -> Result<Value, String> {
    crate::commands::run_blocking(|| Ok(crate::native_proxy::proxies_json())).await
}

/// 各订阅的节点数。自研引擎没有 provider 概念（订阅在启动时一次性拉平），恒为空表。
#[derive(Serialize)]
pub struct ProviderStat {
    pub name: String,
    pub nodes: usize,
}

#[tauri::command]
pub async fn mihomo_providers() -> Result<Vec<ProviderStat>, String> {
    crate::commands::run_blocking(|| Ok(Vec::new())).await
}

/// 订阅里的服务器清单（含协议 / 地址 / 端口 / 传输 / 来源订阅）。
///
/// 引擎没跑时返回最近一次「订阅更新」的缓存；缓存为空时**至少列出手填节点**
/// （它们只依赖 `config\proxy.json`，不需要联网更新）。
#[tauri::command]
pub async fn mihomo_servers() -> Result<Vec<crate::native_proxy::ServerView>, String> {
    crate::commands::run_blocking(|| {
        let cached = crate::native_proxy::servers();
        if !cached.is_empty() {
            return Ok(cached);
        }
        let pc = crate::system_proxy::load_config();
        Ok(manual_server_views(&pc.mihomo))
    })
    .await
}

/// 手填链接 → 抽屉行（来源标「手动」= 空串）。解析失败的那条跳过（`manual_nodes` 仍会
/// 在旧状态里以 `invalid` 占位，但抽屉不给一个点了必定失败的入口）。
fn manual_server_views(cfg: &MihomoConfig) -> Vec<crate::native_proxy::ServerView> {
    cfg.links
        .iter()
        .enumerate()
        .filter_map(|(index, link)| {
            let node = crate::native_proxy::clash::parse_link(link).ok()?;
            Some(crate::native_proxy::ServerView {
                index,
                name: node.name.clone(),
                kind: node.kind.label().to_string(),
                server: node.server.clone(),
                port: node.port,
                transport: crate::native_proxy::transport_label(&node),
                source: String::new(),
                selected: false,
            })
        })
        .collect()
}

/// 「服务器订阅更新」：拉订阅并解析，不改系统代理。返回节点数。
///
/// 2026-11-03：打开面板会静默拉起引擎，而 `native_proxy::refresh` 要求引擎停着 ——
/// 所以这里**临时**停一下引擎、拉完立刻拉回去。服务开着时不碰引擎（那等于断网），
/// 如实让用户先关闭服务。
#[tauri::command]
pub async fn mihomo_refresh_subs() -> Result<usize, String> {
    crate::commands::run_blocking(|| {
        let mut pc = crate::system_proxy::load_config();
        let was_running = is_running();
        if was_running {
            if pc.mihomo.enabled {
                return Err("服务开着：先关闭服务，再更新订阅".into());
            }
            stop()?;
        }
        let result = crate::native_proxy::refresh(&pc.mihomo);
        if was_running {
            // 无论拉取成功与否都要把引擎恢复，否则测延迟会静默失效。
            match start(&mut pc.mihomo) {
                Ok(msg) => {
                    crate::log::info(msg);
                    if let Err(e) = crate::system_proxy::save_config(&pc) {
                        crate::log::warn(format!("订阅更新后保存端口失败：{e}"));
                    }
                }
                Err(e) => crate::log::warn(format!("订阅更新后重启引擎失败：{e}")),
            }
        }
        result
    })
    .await
}

/// 最近的入站请求（界面日志用；含目标域名与走向）。
#[tauri::command]
pub async fn mihomo_requests(limit: Option<usize>) -> Result<Vec<crate::native_proxy::RequestView>, String> {
    let limit = limit.unwrap_or(200).clamp(1, 500);
    crate::commands::run_blocking(move || Ok(crate::native_proxy::requests(limit))).await
}

/// 当前出口信息（国家 / 地区 / 城市 / IP），经引擎代理请求，失败如实返回 `ok=false`。
#[derive(Serialize)]
pub struct ExitInfo {
    pub ok: bool,
    pub ip: String,
    pub country: String,
    pub region: String,
    pub city: String,
    pub error: String,
}

#[tauri::command]
pub async fn mihomo_exit_info() -> Result<ExitInfo, String> {
    crate::commands::run_blocking(|| {
        if !is_running() {
            return Ok(ExitInfo {
                ok: false,
                ip: String::new(),
                country: String::new(),
                region: String::new(),
                city: String::new(),
                error: "引擎未运行".into(),
            });
        }
        let pc = crate::system_proxy::load_config();
        let port = pc.mihomo.mixed_port;
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).map_err(|e| e.to_string())?)
            .build()
            .map_err(|e| format!("build http client: {e}"))?;
        let value: Value = match client
            .get("https://ipinfo.io/json")
            .header(reqwest::header::USER_AGENT, "lunac-proxy")
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.json())
        {
            Ok(v) => v,
            Err(e) => {
                return Ok(ExitInfo {
                    ok: false,
                    ip: String::new(),
                    country: String::new(),
                    region: String::new(),
                    city: String::new(),
                    error: e.to_string(),
                })
            }
        };
        Ok(ExitInfo {
            ok: true,
            ip: text_field(&value, "ip"),
            country: text_field(&value, "country"),
            region: text_field(&value, "region"),
            city: text_field(&value, "city"),
            error: String::new(),
        })
    })
    .await
}

fn text_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// TUN 开关。**当前引擎没有 TUN**（需要 wintun 驱动 + 用户态 TCP/IP 栈）——
/// 打开时如实报错，绝不假装切换成功（`forbidden.md` §禁用 1）。
#[tauri::command]
pub async fn mihomo_set_tun(enabled: bool) -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(move || {
        let mut pc = crate::system_proxy::load_config();
        if enabled {
            pc.mihomo.tun = false;
            crate::system_proxy::save_config(&pc)?;
            return Err(
                "TUN 模式需要 wintun 驱动与用户态 TCP/IP 栈，当前自研引擎尚未支持；请使用系统代理（全局 / 规则）"
                    .into(),
            );
        }
        pc.mihomo.tun = false;
        crate::system_proxy::save_config(&pc)?;
        Ok(status(&pc.mihomo))
    })
    .await
}

/// 当前进程是否已提权（TUN 的管理员闸门）。
#[tauri::command]
pub fn mihomo_is_elevated() -> bool {
    process_is_elevated()
}

#[cfg(target_os = "windows")]
fn process_is_elevated() -> bool {
    #[link(name = "shell32")]
    extern "system" {
        fn IsUserAnAdmin() -> i32;
    }
    unsafe { IsUserAnAdmin() != 0 }
}

#[cfg(not(target_os = "windows"))]
fn process_is_elevated() -> bool {
    false
}

/// 以管理员身份重启 Lunac（TUN 警告框点「同意」后调用）。
///
/// 顺序：先起一个隐藏 PowerShell，**等当前 PID 退出**再 `Start-Process -Verb RunAs`
/// —— 直接先提权会撞上单实例守卫（旧进程还在，新进程立刻退出）。
///
/// **dev（debug）不自动重启**（2026-11-02，用户实测「退出后拉不起来」）：
/// dev 的界面来自 `devUrl = http://localhost:5173` 的 vite，而 vite 是
/// `npm run tauri:dev` 的子进程 —— 旧 app 退出后整条管线被收掉；再从提权进程里另起
/// 一套 `npm run tauri:dev` 会撞端口 / 构建锁，实测无法可靠拉起。而 debug 构建是
/// console 子系统，直接重启 exe 还会弹一个命令行窗口。
/// 所以 dev 只返回**可操作提示**、不退出进程；release 仍自动重启自身 exe。
#[tauri::command]
pub async fn mihomo_relaunch_elevated(app: tauri::AppHandle) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err(
            "开发版不支持自动提权重启（会丢掉 npm run tauri:dev 的 vite 服务且无法自动恢复）。\
             请关闭 Lunac，用【以管理员身份运行】的终端重新执行 npm run tauri:dev，再打开 TUN。"
                .into(),
        );
    }
    let exe = std::env::current_exe().map_err(|e| format!("读取自身路径失败：{e}"))?;
    let pid = std::process::id();
    let script = format!(
        "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; Start-Process -FilePath '{}' -Verb RunAs -WindowStyle Hidden",
        exe.display()
    );
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
        .spawn()
        .map_err(|e| format!("启动提权进程失败：{e}"))?;
    crate::log::info("proxy：用户同意后以管理员身份重启（TUN 闸门）");
    app.exit(0);
    Ok(())
}

/// 当前是否是开发版（debug 构建）。前端据此决定 TUN 警告框的文案：
/// dev 不给「同意并重启」（见 [`mihomo_relaunch_elevated`] 的说明），只提示手动操作。
#[tauri::command]
pub fn mihomo_is_dev_build() -> bool {
    cfg!(debug_assertions)
}

/// 单节点测速（毫秒）：**经节点**发一次 HTTP 探针，量到第一个响应字节的耗时。
#[tauri::command]
pub async fn mihomo_delay(name: String) -> Result<u64, String> {
    crate::commands::run_blocking(move || crate::native_proxy::delay(&name)).await
}

/// 切换当前节点（自研引擎没有「组」的概念，`group` 参数只为前端兼容保留）。
#[tauri::command]
pub async fn mihomo_select(group: String, node: String) -> Result<(), String> {
    crate::commands::run_blocking(move || {
        let _ = group;
        crate::native_proxy::select(&node)
    })
    .await
}

/// 切换模式（rule / global / direct）。自研引擎的模式在启动时展开，运行中**不能热切**。
#[tauri::command]
pub async fn mihomo_set_mode(mode: String) -> Result<MihomoStatus, String> {
    crate::commands::run_blocking(move || {
        let mut pc = crate::system_proxy::load_config();
        if is_running() {
            return Err("自研引擎的模式在启动时确定：先停用、改模式、再启用".into());
        }
        pc.mihomo.mode = sanitize_mode(&mode).to_string();
        crate::system_proxy::save_config(&pc)?;
        Ok(status(&pc.mihomo))
    })
    .await
}

/// 宿主日志尾部（代理插件「日志」页；自研引擎的日志就在宿主日志里）。
#[tauri::command]
pub async fn mihomo_log() -> Result<String, String> {
    crate::commands::run_blocking(|| Ok(read_log(200))).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_falls_back_to_rule_on_garbage() {
        assert_eq!(sanitize_mode("global"), "global");
        assert_eq!(sanitize_mode("DIRECT"), "direct");
        assert_eq!(sanitize_mode("nonsense"), "rule");
        assert_eq!(sanitize_mode(""), "rule");
    }

    #[test]
    fn default_engine_is_native() {
        // §待办 3-26 K1：新配置一律自研，不再依赖 mihomo.exe
        assert_eq!(MihomoConfig::default().engine, "native");
        assert_eq!(engine_name(&MihomoConfig::default()), "native");
    }

    #[test]
    fn blank_engine_means_native_but_explicit_mihomo_survives() {
        // 旧配置里没有 engine 字段 ⇒ serde 给空串 ⇒ 走缺省 native
        let blank = MihomoConfig {
            engine: String::new(),
            ..Default::default()
        };
        assert_eq!(engine_name(&blank), "native");
        // 显式写 mihomo 的旧配置仍被识别出来（随后启动会如实报错，见下一条）
        let legacy = MihomoConfig {
            engine: "  MiHoMo ".into(),
            ..Default::default()
        };
        assert_eq!(engine_name(&legacy), "mihomo");
        // 大小写 / 前后空白不影响 native
        let native = MihomoConfig {
            engine: " NATIVE ".into(),
            ..Default::default()
        };
        assert_eq!(engine_name(&native), "native");
    }

    #[test]
    fn legacy_mihomo_engine_is_rejected_with_actionable_message() {
        // K4：不静默降级，错误里必须给出可操作的那一步（改成 native / 删字段）
        let mut cfg = MihomoConfig {
            engine: "mihomo".into(),
            ..Default::default()
        };
        let err = start(&mut cfg).expect_err("mihomo 已不再内置，必须报错");
        assert!(err.contains("native"), "错误文案要告诉用户改用 native：{err}");
        assert_eq!(err, LEGACY_MIHOMO);
    }

    #[test]
    fn manual_nodes_keep_index_alignment_and_mark_invalid() {
        let cfg = MihomoConfig {
            links: vec![
                "trojan://pw@a.example.com:443#A".to_string(),
                "not-a-link".to_string(),
            ],
            ..Default::default()
        };
        let nodes = manual_nodes(&cfg);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].index, 0);
        assert_eq!(nodes[0].kind, "trojan");
        assert_eq!(nodes[1].index, 1);
        assert_eq!(nodes[1].kind, "invalid");
    }
}
