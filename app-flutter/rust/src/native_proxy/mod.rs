//! 自研代理引擎（重建 v1 `app/src-tauri/src/native_proxy/`）。
//!
//! **背景**：v1 的 21 个文件随误删丢失、快照也不可恢复；本目录按
//! [docs/acceptance-map.md](../../../../docs/acceptance-map.md) 的能力账本重写，
//! 先做「能真跑起来」的主干，再按账本逐项补齐。
//!
//! 架构（与 v1 同形）：一个本地混合端口（SOCKS5 + HTTP CONNECT 同端口）→ 规则判定
//! （本机 / 私网直连）→ 选中节点的协议握手 → 双向透传。
//!
//! 本版已重建：
//!   · 入站 SOCKS5 CONNECT / HTTP CONNECT / HTTP 绝对 URI；UDP ASSOCIATE（直连 + SS-SIP004 /
//!     SS-2022 / Trojan / VLESS（`cmd=UDP`，Vision 走 Mux+XUDP）/ VMess（`cmd=UDP`，
//!     一个分块 = 一个数据报）/ AnyTLS（UoT v2）上游，见 `udp.rs` + `uot.rs`）；
//!   · 规则：DOMAIN / DOMAIN-SUFFIX / DOMAIN-KEYWORD / IP-CIDR / IP-CIDR6 / PROCESS-NAME /
//!     **GEOIP（本地 MaxMind DB）** / MATCH + 本机直连；
//!   · 订阅：base64 链接列表 + 分享链接 + Clash / mihomo YAML 的 `proxies:` 段与顶层 `rules:`；
//!   · 协议：SS AEAD（aes-128-gcm / aes-256-gcm / chacha20-ietf-poly1305）、
//!     **SS-2022（SIP022：blake3 子密钥 + 带类型 / 时间戳 / 请求 salt 的头块）**、Trojan、
//!     VLESS（含 **Vision**：flow addon + 首帧填充 / 下行解填充）、
//!     AnyTLS（认证 + 会话帧 + 上游默认 paddingScheme，含 `cmdUpdatePaddingScheme` 采纳）、
//!     VMess（AEAD 头 + 标准分块体，`alterId=0`）；
//!   · 传输：TCP / TLS / WebSocket / **HTTP 混淆（`network: http`）**；测速 = 经节点的 HTTP 探针；
//!   · SIP003 插件：`simple-obfs`（`obfs=http` / `obfs=tls`，见 `ss_plugin.rs`）与
//!     `v2ray-plugin` 的 websocket 模式（折成 WS 传输）。
//!
//! 本版未重建（会**如实返回原因**，不静默降级）：无 —— REALITY 已于 2026-11-15 重建
//! （`reality.rs`：自写 TLS 1.3 + SessionId 认证 + 临时证书 HMAC 校验）。
//! （SS-2022 多用户不支持 UDP 是 SIP023 的规定，不是缺口 —— 见 `ss2022.rs` 的拒绝原因。）
//! 传输已重建：TCP / TLS / WebSocket / HTTP 混淆 / **h2 / gRPC / XHTTP（stream-one + stream-up）**
//! （见 `h2tunnel.rs`）。
//! Hysteria2 已重建（TCP + UDP：QUIC + HTTP/3 认证 + Salamander / Gecko + `UDPMessage` 分片，见 `hysteria2.rs`）。
//! TUIC v5 已重建（TCP + UDP：QUIC + TLS exporter 认证 + `Connect` / `Packet` / `Dissociate`，见 `tuic.rs`）。
//! 清单见 docs/todo.md 的「自研代理引擎重建」一节。

pub mod anytls;
pub mod clash;
pub mod geoip;
pub mod h2tunnel;
pub mod hysteria2;
pub mod mux;
pub mod process;
pub mod quic;
pub mod reality;
pub mod rules;
pub mod ruleset;
pub mod server;
pub mod ss;
pub mod ss2022;
pub mod ss_plugin;
pub mod transport;
pub mod trojan;
pub mod tuic;
pub mod udp;
pub mod uot;
pub mod vless;
pub mod vmess;
pub mod xudp;

use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 默认混合端口（与 mihomo 的 7890 一致：用户从别处迁过来不用改端口）。
pub const DEFAULT_PORT: u16 = 7890;

/// TLS 选项（分享链接 → 节点）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TlsOptions {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub sni: String,
    #[serde(default)]
    pub allow_insecure: bool,
    #[serde(default)]
    pub alpn: Vec<String>,
    /// 自定义根证书（PEM 文本，可多条拼接；Clash 的 `ca-str`）。
    ///
    /// **追加**到系统根证书之外（不是替换）—— 用户通常是要多信一份自签 CA，
    /// 而不是不信任公网证书。空 = 只用系统根证书。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ca: String,
}

/// WebSocket 选项（`network: ws`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WsOptions {
    pub path: String,
    pub host: String,
}

/// HTTP 混淆选项（`network: http`）。
///
/// 握手 = 先发一个普通 HTTP 请求（`GET <path> HTTP/1.1` + `Host`），服务端回
/// `HTTP/1.1 200` 之后整条连接就变成裸字节流。空串按 Xray 的默认值处理
/// （method=GET / path=/ / host=节点地址）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpOptions {
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub host: String,
}

/// HTTP/2 系传输（与 `ws` / `http` 互斥）：`network: h2` / `grpc` / `xhttp`。
///
/// 共同点：都是**一条 HTTP/2 流承载整条代理连接**（TLS 的 ALPN 必须是 `h2`）。
/// 差别只在请求头与数据封装：
///   · `h2`    —— `POST <path>`，之后请求体 / 响应体就是裸字节；
///   · `grpc`  —— `POST /<serviceName>/Tun`，`content-type: application/grpc`，
///     每条数据都要套一层 gRPC 消息头（`[1 字节压缩标志][4 字节大端长度]`）；
///   · `xhttp` —— Xray 的 SplitHTTP：`stream-one` 用一条 POST 双向承载；
///     `stream-up` 用 `POST <path>/<session>` 上传 + `GET <path>/<session>` 下载，
///     且每个请求都要带 `Referer: …?x_padding=<100..1000 个字符>`（服务端会校验）。
#[derive(Clone, Debug, PartialEq)]
pub enum ExtraTransport {
    H2 {
        host: String,
        path: String,
    },
    Grpc {
        service_name: String,
    },
    XHttp {
        host: String,
        path: String,
        mode: String,
    },
}

impl ExtraTransport {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::H2 { .. } => "h2",
            Self::Grpc { .. } => "grpc",
            Self::XHttp { .. } => "xhttp",
        }
    }
}

/// SIP003 `simple-obfs` 的混淆参数（`obfs=http` / `obfs=tls`）。
///
/// 它夹在「TCP」与「SS 的 salt / 地址头」之间：`obfs=tls` 是**假 TLS**，
/// 不能与真 TLS 叠加（因此这种节点的 `tls.enabled` 恒为 false）。
#[derive(Clone, Debug, PartialEq)]
pub struct SsObfs {
    /// `http` 或 `tls`（已小写）。
    pub mode: String,
    /// `obfs-host`：写进 HTTP 的 `Host:` / 假 ClientHello 的 SNI。
    pub host: String,
}

/// 节点协议。字段都是「握手必需」的那几个；实现状态见 [`Protocol::implemented`]。
#[derive(Clone, Debug, PartialEq)]
pub enum Protocol {
    Ss {
        method: String,
        password: String,
        /// SIP003 `simple-obfs`（`obfs=http` / `obfs=tls`）；`None` = 裸 SS。
        obfs: Option<SsObfs>,
    },
    Trojan {
        password: String,
    },
    Vless {
        uuid: String,
        flow: String,
    },
    Vmess {
        uuid: String,
        alter_id: u32,
        security: String,
        /// `Opt A`（认证长度）：YAML `authenticated-length`，默认关（与 mihomo 一致）。
        authenticated_length: bool,
    },
    AnyTls {
        password: String,
    },
    Hysteria2 {
        password: String,
        obfs: String,
        obfs_password: String,
        insecure: bool,
    },
    /// TUIC v5（QUIC）：uuid + password；`udp_relay_mode` 只实现了 `native`。
    Tuic {
        uuid: String,
        password: String,
        udp_relay_mode: String,
    },
}

impl Protocol {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Ss { .. } => "ss",
            Self::Trojan { .. } => "trojan",
            Self::Vless { .. } => "vless",
            Self::Vmess { .. } => "vmess",
            Self::AnyTls { .. } => "anytls",
            Self::Hysteria2 { .. } => "hysteria2",
            Self::Tuic { .. } => "tuic",
        }
    }

    /// 本版是否已有可用的拨号实现（没有的会在切换时报一句人话）。
    pub fn implemented(&self) -> bool {
        matches!(
            self,
            Self::Ss { .. }
                | Self::Trojan { .. }
                | Self::Vless { .. }
                | Self::Vmess { .. }
                | Self::AnyTls { .. }
                | Self::Hysteria2 { .. }
                | Self::Tuic { .. }
        )
    }
}

/// REALITY 参数（v1 `reality.rs` 的 `RealityOptions`；从分享链接 / YAML 的 `reality-opts` 来）。
///
/// 三个字段都来自节点配置，**缺一不可**：`public_key` 是服务端的 X25519 公钥（32 字节），
/// `short_id` 写在 ClientHello 的 SessionId 里（0–8 字节），`spider_x` 只在「认证失败、
/// 被当成真实网站访问」时用来爬路径 —— 我们对失败一律如实报错，不发蜘蛛请求。
#[derive(Clone, Debug, PartialEq)]
pub struct RealityOptions {
    pub public_key: Vec<u8>,
    pub short_id: Vec<u8>,
    pub spider_x: String,
}

/// 一个节点。
#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    pub server: String,
    pub port: u16,
    pub protocol: Protocol,
    pub tls: TlsOptions,
    pub ws: Option<WsOptions>,
    /// HTTP 混淆（`network: http`）；与 `ws` 互斥（同时给会报错）。
    pub http: Option<HttpOptions>,
    /// HTTP/2 系传输（`network: h2` / `grpc`）；与 `ws` / `http` 互斥。
    pub extra: Option<ExtraTransport>,
    /// REALITY（`security=reality` / YAML 的 `reality-opts`）。有它时**不走** native-tls。
    pub reality: Option<RealityOptions>,
    /// Xray 的 `mux.cool` 多路复用（分享链接 `mux=1`；只对 VLESS / VMess / Trojan 生效）。
    ///
    /// Clash 的 `smux:` 是**另一套不兼容的协议**，不从那里读（见 `mux.rs` 文件头）。
    pub mux: bool,
}

impl Node {
    /// 传输层（抽屉里展示的那一列）：`ws` / `http` / `h2` / `grpc` / `xhttp` / `tls` / `tcp`。
    ///
    /// SIP003 的 `simple-obfs` 与 Hysteria2 的 QUIC 也在这里如实标出来。
    pub fn transport(&self) -> &'static str {
        if let Some(extra) = &self.extra {
            return extra.kind();
        }
        if self.ws.is_some() {
            return "ws";
        }
        if self.http.is_some() {
            return "http";
        }
        match &self.protocol {
            Protocol::Hysteria2 { .. } | Protocol::Tuic { .. } => "quic",
            Protocol::Ss {
                obfs: Some(obfs), ..
            } => match obfs.mode.as_str() {
                "tls" => "obfs-tls",
                _ => "obfs-http",
            },
            _ if self.tls.enabled => "tls",
            _ => "tcp",
        }
    }
}

/// 引擎模式（面板「模式」两档；`direct` 只为兼容旧配置保留）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Rule,
    Global,
    Direct,
}

impl Mode {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "rule" | "rules" => Some(Self::Rule),
            "global" => Some(Self::Global),
            "direct" => Some(Self::Direct),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Global => "global",
            Self::Direct => "direct",
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Rule => 0,
            Self::Global => 1,
            Self::Direct => 2,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Global,
            2 => Self::Direct,
            _ => Self::Rule,
        }
    }
}

/// 一次入站请求最后去了哪里（日志区展示用）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Route {
    Node,
    Direct,
    Rejected,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Direct => "direct",
            Self::Rejected => "rejected",
        }
    }
}

/// 一条请求日志（环形缓冲最多 [`REQUEST_LOG_CAP`] 条）。
#[derive(Clone, Debug)]
pub struct RequestEntry {
    pub host: String,
    pub port: u16,
    pub route: Route,
    /// 节点名 / 命中的规则 / 拒绝原因（人话）。
    pub detail: String,
}

/// 请求日志环形缓冲的上限（与 v1 的 500 条一致）。
pub const REQUEST_LOG_CAP: usize = 500;

/// 引擎配置（从面板传入的 JSON 解析而来）。
pub struct EngineConfig {
    pub nodes: Vec<Node>,
    pub skipped: Vec<String>,
    pub active: usize,
    pub port: u16,
    pub rules: Vec<rules::Rule>,
    pub mode: Mode,
    /// 面板原始名单模式；解析时已展开进 `rules`，保留供诊断与单测。
    #[allow(dead_code)]
    pub rule_mode: String,
    /// 面板原始名单域名；解析时已展开进 `rules`，保留供诊断与单测。
    #[allow(dead_code)]
    pub rule_domains: Vec<String>,
    /// 见 [`Shared::fallback_proxy`]（黑名单 ⇒ `true`，白名单 ⇒ `false`）。
    pub fallback_proxy: bool,
    /// 与 `nodes` 一一对应的来源（`订阅` / `分享链接`），日志与抽屉里展示。
    pub sources: Vec<String>,
}

/// 解析面板传入的 JSON：
/// ```json
/// { "links": ["ss://…", "trojan://…"], "port": 7890, "active": 0,
///   "mode": "rule", "ruleMode": "blacklist", "ruleDomains": ["baidu.com"],
///   "rules": ["DOMAIN-SUFFIX,example.com", "MATCH,PROXY"] }
/// ```
/// `links` 也可以是整段 base64 订阅（放在 `subscription` 字段里）。
/// `ruleDomains` 展开成**插件名单层**并排在订阅 `rules` 之前（先匹配先赢）。
pub fn parse_config(payload: &str) -> Result<EngineConfig, String> {
    let value: serde_json::Value = if payload.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(payload).map_err(|e| format!("引擎配置不是合法 JSON：{e}"))?
    };
    let port = value
        .get("port")
        .and_then(|v| v.as_u64())
        .map(|v| v as u16)
        .filter(|v| *v != 0)
        .unwrap_or(DEFAULT_PORT);
    let active = value.get("active").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let mode = value
        .get("mode")
        .and_then(|v| v.as_str())
        .and_then(Mode::parse)
        .unwrap_or(Mode::Rule);
    let rule_mode = match value
        .get("ruleMode")
        .and_then(|v| v.as_str())
        .unwrap_or("blacklist")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "whitelist" => "whitelist",
        _ => "blacklist",
    }
    .to_string();
    let rule_domains: Vec<String> = value
        .get("ruleDomains")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut nodes: Vec<Node> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut sources: Vec<String> = Vec::new();
    // Clash 订阅顶层的 `rules:`（v1 用 mihomo 时直接生效；现在展开进规则表）。
    let mut subscription_rules: Vec<String> = Vec::new();
    // 顶层 `rule-providers:`：只给 `RULE-SET` 展开用，命名不冲突（同名后者覆盖）。
    let mut providers: HashMap<String, clash::RuleProvider> = HashMap::new();

    if let Some(subscription) = value.get("subscription").and_then(|v| v.as_str()) {
        let parsed = clash::parse_subscription(subscription);
        if !parsed.note.is_empty() {
            eprintln!("订阅解析：{}", parsed.note);
        }
        sources.extend(vec!["订阅".to_string(); parsed.nodes.len()]);
        nodes.extend(parsed.nodes);
        skipped.extend(parsed.skipped);
        subscription_rules.extend(parsed.rules);
        providers.extend(parsed.providers);
    }
    if let Some(links) = value.get("links").and_then(|v| v.as_array()) {
        for item in links {
            let Some(link) = item.as_str() else {
                continue;
            };
            match clash::parse_link(link) {
                Ok(node) => {
                    nodes.push(node);
                    sources.push("分享链接".to_string());
                }
                Err(reason) => skipped.push(reason),
            }
        }
    }
    let rule_lines: Vec<String> = value
        .get("rules")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    // 插件名单层在前，然后是调用方显式给的 `rules`，最后是订阅 YAML 自带的 `rules:`（先匹配先赢）。
    // `RULE-SET` 在这些行里就地展开（`ruleset.rs`），展开失败逐条记原因、不猜。
    let mut rule_list = rules::domain_rules(&rule_mode, &rule_domains);
    let raw_rules: Vec<String> = rule_lines.into_iter().chain(subscription_rules).collect();
    let (expanded, notes) = ruleset::expand(&raw_rules, &providers, &ruleset::default_cache_dir());
    skipped.extend(notes);
    for line in expanded {
        match rules::parse_rule(&line) {
            Some(rule) => rule_list.push(rule),
            None => skipped.push(format!("未知规则：{line}（已跳过）")),
        }
    }
    Ok(EngineConfig {
        nodes,
        skipped,
        active,
        port,
        rules: rule_list,
        mode,
        rule_mode: rule_mode.clone(),
        rule_domains,
        fallback_proxy: rule_mode != "whitelist",
        sources,
    })
}

/// 自定义服务器字段 → 分享链接（v1 `mihomo_node_add_custom` 的等价物）。
///
/// 面板只收集字段，**链接格式与校验都在 Rust 这一处**（与订阅走同一个
/// [`clash::parse_link`]）—— Dart 侧不复制协议编码。
///
/// 入参 JSON：`{"kind":"ss|trojan|vless","name":"…","server":"…","port":8388,
/// "password":"…","method":"aes-256-gcm","uuid":"…"}`。
pub fn build_custom_link_json(payload: &str) -> String {
    let value: serde_json::Value = match serde_json::from_str(payload) {
        Ok(value) => value,
        Err(e) => {
            return serde_json::json!({ "ok": false, "link": "", "error": format!("自定义服务器参数不是合法 JSON：{e}") })
                .to_string()
        }
    };
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let kind = text("kind").to_ascii_lowercase();
    let name = {
        let raw = text("name");
        if raw.is_empty() {
            "自定义节点".to_string()
        } else {
            raw
        }
    };
    let server = text("server");
    let port = value
        .get("port")
        .and_then(|item| item.as_u64())
        .unwrap_or(0);
    // vless / vmess 的凭据在 `uuid`；tuic 两个都要；其余只有密码。
    let uuid = text("uuid");
    let secret = if matches!(kind.as_str(), "vless" | "vmess") {
        uuid.clone()
    } else {
        text("password")
    };
    let method = {
        let raw = text("method");
        if raw.is_empty() {
            "aes-256-gcm".to_string()
        } else {
            raw
        }
    };
    let fail = |reason: String| {
        serde_json::json!({ "ok": false, "link": "", "error": reason }).to_string()
    };
    if server.is_empty() {
        return fail("先填写服务器地址".to_string());
    }
    if !(1..=65535).contains(&port) {
        return fail("端口要在 1–65535 之间".to_string());
    }
    if matches!(kind.as_str(), "tuic") {
        if uuid.is_empty() {
            return fail("tuic 需要 UUID".to_string());
        }
        if secret.is_empty() {
            return fail("tuic 需要密码".to_string());
        }
    } else if secret.is_empty() {
        return fail(match kind.as_str() {
            "vless" | "vmess" => format!("{kind} 需要 UUID"),
            _ => "先填写密码".to_string(),
        });
    }
    // IPv6 在 URL 里必须带方括号（用户多半直接贴 `2001:db8::1`）。
    let host = if server.contains(':') && !server.starts_with('[') {
        format!("[{server}]")
    } else {
        server.clone()
    };
    let fragment = encode_url_component(&name);
    let password = encode_url_component(&secret);
    let user = encode_url_component(&uuid);
    // `mux.cool` 只对 VLESS / VMess / Trojan 有意义；其它协议忽略这一位。
    let mux = value
        .get("mux")
        .map(|item| match item {
            serde_json::Value::Bool(flag) => *flag,
            serde_json::Value::Number(number) => number.as_i64().unwrap_or(0) != 0,
            serde_json::Value::String(text) => matches!(
                text.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            ),
            _ => false,
        })
        .unwrap_or(false);
    let link = match kind.as_str() {
        // SIP002：`ss://base64url(method:password)@host:port#name`
        "ss" => {
            use base64::Engine as _;
            let userinfo = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(format!("{method}:{secret}").as_bytes());
            format!("ss://{userinfo}@{host}:{port}#{fragment}")
        }
        "trojan" => {
            let query = if mux { "?mux=1" } else { "" };
            format!("trojan://{password}@{host}:{port}{query}#{fragment}")
        }
        "vless" => {
            let query = if mux {
                "?encryption=none&mux=1"
            } else {
                "?encryption=none"
            };
            format!("vless://{user}@{host}:{port}{query}#{fragment}")
        }
        // vmess 分享链接是整段 base64 的 JSON（与订阅侧 `parse_vmess` 同一份字段）。
        "vmess" => {
            use base64::Engine as _;
            let json = serde_json::json!({
                "v": "2",
                "ps": name,
                "add": server,
                "port": port,
                "id": secret,
                "aid": 0,
                "scy": "auto",
                "net": "tcp",
                "type": "none",
                "host": "",
                "path": "",
                "tls": "",
                "mux": if mux { "1" } else { "" },
            });
            let payload = base64::engine::general_purpose::STANDARD.encode(json.to_string());
            format!("vmess://{payload}")
        }
        "anytls" => format!("anytls://{password}@{host}:{port}#{fragment}"),
        "hysteria2" | "hy2" => format!("hysteria2://{password}@{host}:{port}#{fragment}"),
        // TUIC v5：`tuic://uuid:password@host:port#name`（两个凭据都在 userinfo 里）。
        "tuic" => format!("tuic://{user}:{password}@{host}:{port}#{fragment}"),
        other => {
            return fail(format!(
                "自定义服务器还不支持 {other}（可选 ss / trojan / vless / vmess / anytls / hysteria2 / tuic）"
            ));
        }
    };
    // 关键：用**订阅同一个解析器**回读，字段拼错在加入清单前就报出来。
    match clash::parse_link(&link) {
        Ok(node) => serde_json::json!({
            "ok": true,
            "link": link,
            "name": node.name,
            "kind": node.protocol.kind(),
            "error": "",
        })
        .to_string(),
        Err(reason) => fail(reason),
    }
}

/// 百分号转义（分享链接的备注 / 密码里可能出现 `#`、`&`、空格）。
///
/// **不用** `form_urlencoded`：它把空格编成 `+`，而订阅侧的 `percent_decode` 只认 `%XX`，
/// 备注会多出一个加号。这里只放行 RFC 3986 的 unreserved 字符。
fn encode_url_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 目标地址编码（SOCKS5 / Trojan / VLESS 共用的「ATYP + addr + port」）。
pub fn encode_target(host: &str, port: u16) -> Vec<u8> {
    let text = host.trim().trim_matches(['[', ']']);
    let mut out = Vec::with_capacity(32);
    if let Ok(ip) = text.parse::<Ipv4Addr>() {
        out.push(0x01);
        out.extend_from_slice(&ip.octets());
    } else if let Ok(ip) = text.parse::<Ipv6Addr>() {
        out.push(0x04);
        out.extend_from_slice(&ip.octets());
    } else {
        let bytes = text.as_bytes();
        let len = bytes.len().min(255);
        out.push(0x03);
        out.push(len as u8);
        out.extend_from_slice(&bytes[..len]);
    }
    out.extend_from_slice(&port.to_be_bytes());
    out
}

/// 入站连接共享的状态（accept 线程与每个连接线程只读它）。
pub struct Shared {
    pub nodes: Vec<Node>,
    pub rules: Vec<rules::Rule>,
    pub active: AtomicUsize,
    pub alive: AtomicBool,
    /// 面板的模式（规则 / 全局 / 直连）。原子量：切模式不必重启监听。
    pub mode: AtomicU8,
    /// 与 `nodes` 一一对应的来源（`订阅` / `分享链接`），日志区展示。
    pub sources: Vec<String>,
    /// `rule` 模式下「一条规则都没命中」时是否走节点。
    ///
    /// 与插件名单层的语义绑在一起（§待办 3-27 第三轮）：
    ///   · 黑名单（默认）= 名单内直连、**名单外走节点** ⇒ `true`；
    ///   · 白名单 = 名单内走节点、**名单外直连** ⇒ `false`。
    pub fallback_proxy: bool,
    /// 累计上行 / 下行字节（面板每 2s 取一次做差算速率）。
    pub up: AtomicU64,
    pub down: AtomicU64,
    /// `GEOIP` 规则用的 MaxMind DB；没有规则需要它时为 `None`（不白读 6 MB）。
    pub geoip: Option<geoip::GeoIp>,
}

impl Shared {
    pub fn active_node(&self) -> Option<&Node> {
        self.nodes.get(self.active.load(Ordering::Relaxed))
    }

    /// 一条连接的去向 + 人话原因（日志区与拨号共用同一份判定，避免两处口径漂移）。
    ///
    /// 判定顺序：
    ///   1. 本机 / 私网 ⇒ 直连；
    ///   2. `direct` 模式 ⇒ 全部直连；`global` 模式 ⇒ 全部走节点；
    ///   3. `rule` 模式：规则表第一条命中者生效；一个都不命中时按
    ///      [`Shared::fallback_proxy`] 兜底（黑名单 ⇒ 走节点，白名单 ⇒ 直连）。
    ///      规则表为空时同样走这条兜底 —— 只贴几条链接（黑名单默认）就是「除本机 / 私网外全走节点」。
    pub fn decide_route(&self, host: &str, process: Option<&str>) -> (Route, String) {
        if rules::is_lan(host) {
            return (Route::Direct, "本机 / 私网".to_string());
        }
        match self.mode() {
            Mode::Direct => return (Route::Direct, "全局直连".to_string()),
            Mode::Global => return self.node_route(None),
            Mode::Rule => {}
        }
        let literal = host.parse().ok();
        if let Some(rule) =
            rules::decide_rule_with_geoip(&self.rules, host, literal, process, self.geoip.as_ref())
        {
            return if rule.proxy {
                self.node_route(Some(rule.label()))
            } else {
                (Route::Direct, format!("命中 {}", rule.label()))
            };
        }
        if self.fallback_proxy {
            self.node_route(None)
        } else {
            (Route::Direct, "名单外（白名单模式）".to_string())
        }
    }

    /// 走节点时的人话描述：`订阅 · 香港 01`（有命中规则时再带上规则）。
    fn node_route(&self, rule: Option<String>) -> (Route, String) {
        let index = self.active.load(Ordering::Relaxed);
        match self.nodes.get(index) {
            Some(node) => {
                let source = self
                    .sources
                    .get(index)
                    .map(String::as_str)
                    .unwrap_or("订阅");
                let detail = match rule {
                    Some(label) => format!("{source} · {}（命中 {label}）", node.name),
                    None => format!("{source} · {}", node.name),
                };
                (Route::Node, detail)
            }
            None => (
                Route::Rejected,
                "规则要求走节点，但引擎里没有节点".to_string(),
            ),
        }
    }

    /// 是否走节点（[`Shared::decide_route`] 的布尔投影）。
    #[cfg(test)]
    pub fn should_proxy(&self, host: &str, process: Option<&str>) -> bool {
        self.decide_route(host, process).0 == Route::Node
    }

    pub fn mode(&self) -> Mode {
        Mode::from_code(self.mode.load(Ordering::Relaxed))
    }

    /// 切模式（运行中即刻生效，已在跑的连接不受影响）。
    pub fn set_mode(&self, mode: Mode) {
        self.mode.store(mode.code(), Ordering::Relaxed);
    }

    /// 规则表里是否有 `PROCESS-NAME`（没有就不必枚举 TCP 表）。
    pub fn needs_process(&self) -> bool {
        self.rules
            .iter()
            .any(|rule| matches!(rule.kind, rules::RuleKind::ProcessName))
    }
}

/// 请求日志（全局单例：引擎本身也是单例，与 v1 的 `mihomo_requests` 同形）。
fn request_log() -> &'static Mutex<VecDeque<RequestEntry>> {
    static LOG: OnceLock<Mutex<VecDeque<RequestEntry>>> = OnceLock::new();
    LOG.get_or_init(|| Mutex::new(VecDeque::with_capacity(REQUEST_LOG_CAP)))
}

/// 记一条请求（超过 [`REQUEST_LOG_CAP`] 就丢最旧的）。
///
/// `server.rs::dial` 与 `udp.rs` 共用 —— 界面上的日志区只读这一份，不许另记一份。
pub fn record_request(host: &str, port: u16, route: Route, detail: String) {
    let Ok(mut log) = request_log().lock() else {
        return;
    };
    push_entry(
        &mut log,
        RequestEntry {
            host: host.to_string(),
            port,
            route,
            detail,
        },
    );
}

/// 环形缓冲的入队规则：满了就丢最旧的（单独抽出来，单测不必碰全局状态）。
fn push_entry(log: &mut VecDeque<RequestEntry>, entry: RequestEntry) {
    while log.len() >= REQUEST_LOG_CAP {
        log.pop_front();
    }
    log.push_back(entry);
}

fn clear_requests() {
    if let Ok(mut log) = request_log().lock() {
        log.clear();
    }
}

/// 最近 `limit` 条请求日志（按时间正序，新条目在末尾）。启动新引擎时清空、停止后保留。
pub fn requests_json(limit: usize) -> String {
    let log = match request_log().lock() {
        Ok(log) => log,
        Err(_) => return "[]".to_string(),
    };
    let start = log.len().saturating_sub(limit);
    let items: Vec<serde_json::Value> = log
        .iter()
        .skip(start)
        .map(|entry| {
            serde_json::json!({
                "host": entry.host,
                "port": entry.port,
                "route": entry.route.as_str(),
                "detail": entry.detail,
            })
        })
        .collect();
    serde_json::Value::Array(items).to_string()
}

struct EngineState {
    running: bool,
    port: u16,
    active: usize,
    error: String,
    nodes: Vec<Node>,
    /// 与 `nodes` 一一对应的来源（`订阅` / `分享链接`），抽屉里展示。
    sources: Vec<String>,
    skipped: Vec<String>,
    mode: Mode,
    shared: Option<Arc<Shared>>,
}

fn state() -> &'static Mutex<EngineState> {
    static STATE: OnceLock<Mutex<EngineState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(EngineState {
            running: false,
            port: 0,
            active: 0,
            error: String::new(),
            nodes: Vec::new(),
            sources: Vec::new(),
            skipped: Vec::new(),
            mode: Mode::Rule,
            shared: None,
        })
    })
}

fn status_json_locked(guard: &EngineState) -> String {
    let nodes: Vec<serde_json::Value> = guard
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            serde_json::json!({
                "index": index,
                "name": node.name,
                "kind": node.protocol.kind(),
                "server": node.server,
                "port": node.port,
                "transport": node.transport(),
                "source": guard.sources.get(index).map(String::as_str).unwrap_or(""),
                "implemented": node.protocol.implemented(),
            })
        })
        .collect();
    // 速率由界面按两次采样的差算；这里只给累计值（停止后归零，避免跨会话串数）。
    let (up, down) = match &guard.shared {
        Some(shared) => (
            shared.up.load(Ordering::Relaxed),
            shared.down.load(Ordering::Relaxed),
        ),
        None => (0, 0),
    };
    serde_json::json!({
        "supported": true,
        "running": guard.running,
        "port": guard.port,
        "active": guard.active,
        "mode": guard.mode.as_str(),
        "traffic": { "up": up, "down": down },
        "nodes": nodes,
        "skipped": guard.skipped,
        "error": guard.error,
    })
    .to_string()
}

pub fn status_json() -> String {
    match state().lock() {
        Ok(guard) => status_json_locked(&guard),
        Err(_) => serde_json::json!({ "supported": true, "running": false, "port": 0,
            "active": 0, "nodes": [], "skipped": [], "error": "引擎状态锁不可用" })
        .to_string(),
    }
}

/// 启动引擎：解析链接 → 绑定端口 → 起 accept 线程。返回状态 JSON。
pub fn start_json(payload: &str) -> String {
    let config = match parse_config(payload) {
        Ok(config) => config,
        Err(reason) => {
            if let Ok(mut guard) = state().lock() {
                guard.error = reason.clone();
            }
            return serde_json::json!({ "ok": false, "error": reason }).to_string();
        }
    };
    if config.nodes.is_empty() {
        let reason = if config.skipped.is_empty() {
            "自研引擎没有可用节点：订阅里没有任何受支持的链接".to_string()
        } else {
            format!("自研引擎没有可用节点：{}", config.skipped.join("；"))
        };
        if let Ok(mut guard) = state().lock() {
            guard.error = reason.clone();
            guard.skipped = config.skipped;
        }
        return serde_json::json!({ "ok": false, "error": reason }).to_string();
    }

    // `GEOIP` 规则需要本地 MaxMind DB：只在确实有这类规则时才读（GeoLite2-Country 约 6 MB）。
    // 缺库 / 坏库时**如实**记进跳过原因 —— 规则继续留在表里，但不会命中。
    let mut skipped = config.skipped;
    let geoip = if config
        .rules
        .iter()
        .any(|rule| matches!(rule.kind, rules::RuleKind::GeoIp))
    {
        match geoip::GeoIp::load_default() {
            Ok(db) => Some(db),
            Err(reason) => {
                eprintln!("[lunac-proxy] GEOIP 规则未生效：{reason}");
                skipped.push(format!("GEOIP 规则未生效：{reason}"));
                None
            }
        }
    } else {
        None
    };

    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(_) => {
            return serde_json::json!({ "ok": false, "error": "引擎状态锁不可用" }).to_string()
        }
    };
    if guard.running {
        return serde_json::json!({ "ok": false, "error": "引擎已经在运行；先停用再启动" })
            .to_string();
    }
    let (listener, port, note) = match TcpListener::bind(("127.0.0.1", config.port)) {
        Ok(listener) => (listener, config.port, String::new()),
        Err(_) => match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => match listener.local_addr() {
                Ok(addr) => (
                    listener,
                    addr.port(),
                    format!("端口 {} 被占用，改用空闲端口 {}", config.port, addr.port()),
                ),
                Err(e) => {
                    return serde_json::json!({ "ok": false, "error": format!("读取监听端口失败：{e}") })
                        .to_string()
                }
            },
            Err(e) => {
                return serde_json::json!({ "ok": false, "error": format!("监听失败：{e}") })
                    .to_string()
            }
        },
    };
    if let Err(e) = listener.set_nonblocking(true) {
        return serde_json::json!({ "ok": false, "error": format!("设置非阻塞失败：{e}") })
            .to_string();
    }
    let active = config.active.min(config.nodes.len() - 1);
    let shared = Arc::new(Shared {
        nodes: config.nodes.clone(),
        rules: config.rules,
        active: AtomicUsize::new(active),
        alive: AtomicBool::new(true),
        mode: AtomicU8::new(config.mode.code()),
        sources: config.sources.clone(),
        fallback_proxy: config.fallback_proxy,
        up: AtomicU64::new(0),
        down: AtomicU64::new(0),
        geoip,
    });
    clear_requests();
    guard.running = true;
    guard.port = port;
    guard.active = active;
    guard.mode = config.mode;
    guard.error = note.clone();
    guard.nodes = config.nodes;
    guard.sources = config.sources;
    guard.skipped = skipped;
    guard.shared = Some(shared.clone());
    let status = status_json_locked(&guard);
    drop(guard);

    thread::Builder::new()
        .name("lunac-native-proxy".to_string())
        .spawn(move || accept_loop(listener, shared))
        .ok();
    serde_json::json!({ "ok": true, "error": "", "status": serde_json::from_str::<serde_json::Value>(&status).unwrap_or_default() })
        .to_string()
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    while shared.alive.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let shared = shared.clone();
                let _ = thread::Builder::new()
                    .name("lunac-native-proxy-conn".to_string())
                    .spawn(move || server::handle_client(stream, shared));
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(30));
            }
            Err(_) => break,
        }
    }
}

/// 停止引擎。
pub fn stop_json() -> String {
    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(_) => {
            return serde_json::json!({ "ok": false, "error": "引擎状态锁不可用" }).to_string()
        }
    };
    if let Some(shared) = guard.shared.take() {
        shared.alive.store(false, Ordering::Relaxed);
    }
    // AnyTLS / TUIC / mux 的复用连接不跟监听线程一起收：停引擎时显式关掉，不等空闲超时。
    anytls::close_all_sessions();
    tuic::close_all_sessions();
    mux::close_all_sessions();
    guard.running = false;
    guard.port = 0;
    guard.error.clear();
    let status = status_json_locked(&guard);
    serde_json::json!({ "ok": true, "error": "", "status": serde_json::from_str::<serde_json::Value>(&status).unwrap_or_default() })
        .to_string()
}

/// 切换节点（运行中直接生效，不必重启监听）。
pub fn select_json(index: usize) -> String {
    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(_) => {
            return serde_json::json!({ "ok": false, "error": "引擎状态锁不可用" }).to_string()
        }
    };
    if index >= guard.nodes.len() {
        return serde_json::json!({ "ok": false, "error": "节点序号越界" }).to_string();
    }
    guard.active = index;
    if let Some(shared) = &guard.shared {
        shared.active.store(index, Ordering::Relaxed);
    }
    let status = status_json_locked(&guard);
    serde_json::json!({ "ok": true, "error": "", "status": serde_json::from_str::<serde_json::Value>(&status).unwrap_or_default() })
        .to_string()
}

/// 切换模式（面板两档：`rule` 规则模式 / `global` 全局代理）。运行中即刻生效。
pub fn set_mode_json(text: &str) -> String {
    let Some(mode) = Mode::parse(text) else {
        return serde_json::json!({ "ok": false, "error": format!("不认识的模式：{text}") })
            .to_string();
    };
    if mode == Mode::Direct {
        return serde_json::json!({ "ok": false, "error": "面板只提供规则模式与全局代理两档" })
            .to_string();
    }
    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(_) => {
            return serde_json::json!({ "ok": false, "error": "引擎状态锁不可用" }).to_string()
        }
    };
    guard.mode = mode;
    if let Some(shared) = &guard.shared {
        shared.set_mode(mode);
    }
    let status = status_json_locked(&guard);
    serde_json::json!({ "ok": true, "error": "", "status": serde_json::from_str::<serde_json::Value>(&status).unwrap_or_default() })
        .to_string()
}

/// 节点时延：**经节点**对一个直连可达的明文 HTTP 站发 `HEAD /`，量到**第一个响应字节**。
///
/// 与 v1 同口径（2026-11-03）：目标是 `www.baidu.com:80`，失败**如实报错，不回退 TCP 建连**
/// （TCP 建连只证明「能连到节点服务器」，数字会偏乐观）。
pub fn delay_json(index: usize) -> String {
    let node = {
        let guard = match state().lock() {
            Ok(guard) => guard,
            Err(_) => {
                return serde_json::json!({
                    "ok": false,
                    "error": "引擎状态锁不可用",
                    "ms": 0,
                    "scope": "http",
                })
                .to_string()
            }
        };
        match guard.nodes.get(index) {
            Some(node) => node.clone(),
            None => {
                return serde_json::json!({
                    "ok": false,
                    "error": "节点序号越界",
                    "ms": 0,
                    "scope": "http",
                })
                .to_string()
            }
        }
    };
    let started = std::time::Instant::now();
    match probe_through_node(&node) {
        Ok(()) => serde_json::json!({
            "ok": true,
            "error": "",
            "ms": started.elapsed().as_millis() as u64,
            "scope": "http",
        })
        .to_string(),
        Err(e) => serde_json::json!({
            "ok": false,
            "error": format!("经节点取 http://{PROBE_HOST}/ 失败：{e}"),
            "ms": started.elapsed().as_millis() as u64,
            "scope": "http",
        })
        .to_string(),
    }
}

/// 测速探针的目标：直连可达、明文 80、只量首个响应字节（v1 同款）。
const PROBE_HOST: &str = "www.baidu.com";
/// 探针读超时（拨号本身另有 [`transport::DIAL_TIMEOUT`]）。
const PROBE_READ_TIMEOUT: Duration = Duration::from_secs(10);

fn probe_through_node(node: &Node) -> Result<(), String> {
    let target = server::Target {
        host: PROBE_HOST.to_string(),
        port: 80,
    };
    let mut stream = server::dial_node(node, &target)?;
    http_probe(&mut stream, PROBE_HOST, PROBE_READ_TIMEOUT)
}

/// 在已连通的流上发 `HEAD /`，读到**第一个字节**即算成功（状态码本身不参与判定）。
fn http_probe(
    stream: &mut transport::BoxStream,
    host: &str,
    timeout: Duration,
) -> Result<(), String> {
    use std::io::{Read, Write};

    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| format!("设置读超时失败：{e}"))?;
    let request = format!(
        "HEAD / HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Lunac-Probe/1.0\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("发探针失败：{e}"))?;
    let mut first = [0u8; 1];
    stream
        .read_exact(&mut first)
        .map_err(|e| format!("等首个响应字节失败：{e}"))?;
    Ok(())
}

/// 节点清单（面板展示用）。
pub fn proxies_json() -> String {
    let guard = match state().lock() {
        Ok(guard) => guard,
        Err(_) => return "[]".to_string(),
    };
    let items: Vec<serde_json::Value> = guard
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            serde_json::json!({
                "index": index,
                "name": node.name,
                "kind": node.protocol.kind(),
                "server": node.server,
                "port": node.port,
                "transport": node.transport(),
                "source": guard.sources.get(index).map(String::as_str).unwrap_or(""),
                "implemented": node.protocol.implemented(),
            })
        })
        .collect();
    serde_json::Value::Array(items).to_string()
}

/// 出口地区（面板中央区那一行）：**经当前节点**取 `ipinfo.io/country` 的明文结果。
///
/// 与 v1 同口径（经引擎请求 ipinfo）；引擎没跑 / 没节点 / 请求失败都如实报错，
/// 不拿本机 IP 冒充出口地区。
pub fn exit_json() -> String {
    let node = {
        let guard = match state().lock() {
            Ok(guard) => guard,
            Err(_) => {
                return serde_json::json!({ "ok": false, "country": "", "error": "引擎状态锁不可用" })
                    .to_string()
            }
        };
        if !guard.running {
            return serde_json::json!({ "ok": false, "country": "", "error": "引擎未运行" })
                .to_string();
        }
        match guard
            .shared
            .as_ref()
            .and_then(|shared| shared.active_node())
        {
            Some(node) => node.clone(),
            None => {
                return serde_json::json!({ "ok": false, "country": "", "error": "当前没有节点" })
                    .to_string()
            }
        }
    };
    match exit_country_via_node(&node) {
        Ok(country) => {
            serde_json::json!({ "ok": true, "country": country, "error": "" }).to_string()
        }
        Err(reason) => {
            serde_json::json!({ "ok": false, "country": "", "error": reason }).to_string()
        }
    }
}

/// 经节点请求 `ipinfo.io/country`（明文 80，与 v1 同款探针思路）。
fn exit_country_via_node(node: &Node) -> Result<String, String> {
    use std::io::{Read, Write};

    let target = server::Target {
        host: EXIT_PROBE_HOST.to_string(),
        port: 80,
    };
    let mut stream = server::dial_node(node, &target)?;
    stream
        .set_read_timeout(Some(PROBE_READ_TIMEOUT))
        .map_err(|e| format!("设置读超时失败：{e}"))?;
    stream
        .write_all(
            format!(
                "GET /country HTTP/1.1\r\nHost: {EXIT_PROBE_HOST}\r\nUser-Agent: Lunac-Proxy/1.0\r\nAccept: text/plain\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .map_err(|e| format!("发出口探测失败：{e}"))?;
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                raw.extend_from_slice(&chunk[..read]);
                if let Some(country) = parse_country_response(&raw) {
                    return Ok(country);
                }
                if raw.len() > 32 * 1024 {
                    break;
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // 超时前没解出完整正文 ⇒ 如实说没拿到。
                break;
            }
            Err(e) => return Err(format!("读出口探测响应失败：{e}")),
        }
    }
    parse_country_response(&raw).ok_or_else(|| "出口探测没有拿到国家代码".to_string())
}

/// 出口探测的主机（明文 80 的 `GET /country`）。
const EXIT_PROBE_HOST: &str = "ipinfo.io";

/// 从 HTTP 响应里抠出正文的国家代码（`JP` / `US` …）。
///
/// 纯函数：`None` = 响应还没收全 / 状态码不是 200 / 正文不像两个字母的国家代码。
fn parse_country_response(raw: &[u8]) -> Option<String> {
    let split = raw.windows(4).position(|window| window == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let status = head.lines().next()?.split_whitespace().nth(1)?;
    if status != "200" {
        return None;
    }
    let body = String::from_utf8_lossy(&raw[split + 4..])
        .trim()
        .to_string();
    let code = body.lines().next()?.trim().to_ascii_uppercase();
    if code.len() == 2 && code.chars().all(|c| c.is_ascii_uppercase()) {
        Some(code)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_target_uses_socks_address_types() {
        let ipv4 = encode_target("1.2.3.4", 80);
        assert_eq!(ipv4[0], 0x01);
        assert_eq!(&ipv4[1..5], &[1, 2, 3, 4]);
        assert_eq!(&ipv4[5..7], &80u16.to_be_bytes());

        let domain = encode_target("example.com", 443);
        assert_eq!(domain[0], 0x03);
        assert_eq!(domain[1], 11);
        assert_eq!(&domain[2..13], b"example.com");

        let ipv6 = encode_target("::1", 8080);
        assert_eq!(ipv6[0], 0x04);
        assert_eq!(ipv6.len(), 1 + 16 + 2);
    }

    #[test]
    fn parse_config_collects_nodes_and_skips() {
        let payload = serde_json::json!({
            "links": [
                "ss://YWVzLTI1Ni1nY206cHc@a.com:8388#A",
                "tuic://pw@b.com:443",
            ],
            "port": 7891,
            "active": 0,
            "rules": ["MATCH,PROXY", "GEOIP,CN"],
        })
        .to_string();
        let config = parse_config(&payload).unwrap();
        assert_eq!(config.nodes.len(), 1);
        assert_eq!(config.port, 7891);
        assert_eq!(config.rules.len(), 2, "MATCH 与 GEOIP 都是可解析规则");
        assert_eq!(config.skipped.len(), 1, "只剩 tuic v4 那条被跳过");
    }

    #[test]
    fn subscription_rules_are_expanded_after_the_plugin_name_layer() {
        let subscription = "proxies:\n  - name: a\n    type: ss\n    server: a.com\n    port: 8388\n    cipher: aes-128-gcm\n    password: pw\nrules:\n  - GEOIP,CN,DIRECT\n  - RULE-SET,cn\n  - MATCH,PROXY\n";
        let payload = serde_json::json!({
            "subscription": subscription,
            "ruleDomains": ["intl.test"],
            "ruleMode": "blacklist",
        })
        .to_string();
        let config = parse_config(&payload).unwrap();
        assert_eq!(config.nodes.len(), 1);
        assert_eq!(
            config
                .rules
                .iter()
                .map(|rule| rule.label())
                .collect::<Vec<_>>(),
            vec![
                "DOMAIN-SUFFIX,intl.test,DIRECT".to_string(),
                "GEOIP,CN,DIRECT".to_string(),
                "MATCH,PROXY".to_string(),
            ],
            "名单层在前，订阅 rules 在后；RULE-SET 按未知规则跳过"
        );
        assert!(
            config
                .skipped
                .iter()
                .any(|line| line.contains("RULE-SET,cn")),
            "{:?}",
            config.skipped
        );
    }

    #[test]
    fn rule_set_entries_expand_from_inline_providers() {
        let subscription = "\
proxies:
  - name: a
    type: ss
    server: a.com
    port: 8388
    cipher: aes-128-gcm
    password: pw
rule-providers:
  ads:
    type: inline
    behavior: domain
    payload:
      - '+.ads.example'
      - 'tracker.example'
  lan:
    type: inline
    behavior: ipcidr
    payload:
      - '10.0.0.0/8'
rules:
  - RULE-SET,ads,REJECT
  - RULE-SET,lan,DIRECT
  - MATCH,PROXY
";
        let config =
            parse_config(&serde_json::json!({ "subscription": subscription }).to_string()).unwrap();
        assert_eq!(
            config
                .rules
                .iter()
                .map(|rule| rule.label())
                .collect::<Vec<_>>(),
            vec![
                "DOMAIN-SUFFIX,ads.example,PROXY".to_string(),
                "DOMAIN,tracker.example,PROXY".to_string(),
                "IP-CIDR,10.0.0.0/8,DIRECT".to_string(),
                "MATCH,PROXY".to_string(),
            ],
            "RULE-SET 必须就地展开（顺序不变）"
        );
        assert!(config.skipped.is_empty(), "{:?}", config.skipped);
    }

    /// 测试用的最小节点（只用来让「走节点」有落点）。
    fn test_node() -> Node {
        clash::parse_link("ss://YWVzLTI1Ni1nY206cHc@a.com:8388#A").unwrap()
    }

    /// 构造 Shared 的默认形态：规则模式、黑名单兜底、无来源、计数归零。
    fn test_shared(nodes: Vec<Node>, rules: Vec<rules::Rule>) -> Shared {
        Shared {
            nodes,
            rules,
            active: AtomicUsize::new(0),
            alive: AtomicBool::new(true),
            mode: AtomicU8::new(Mode::Rule.code()),
            sources: Vec::new(),
            fallback_proxy: true,
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            geoip: None,
        }
    }

    #[test]
    fn empty_rules_proxy_everything_except_lan() {
        let shared = test_shared(vec![test_node()], Vec::new());
        assert!(shared.should_proxy("example.com", None));
        assert!(!shared.should_proxy("127.0.0.1", None));
        assert!(!shared.needs_process());
    }

    #[test]
    fn explicit_rules_first_match_wins() {
        let rules = vec![rules::parse_rule("DOMAIN-SUFFIX,corp").unwrap()];
        let shared = test_shared(vec![test_node()], rules.clone());
        assert!(shared.should_proxy("a.corp", None));
        // 黑名单兜底：名单外走节点（与 v1 §待办 3-27 的 `fallback=Proxy` 同口径）。
        assert!(shared.should_proxy("other.test", None));
        // 白名单兜底：名单外直连。
        let mut whitelist = test_shared(vec![test_node()], rules);
        whitelist.fallback_proxy = false;
        assert!(whitelist.should_proxy("a.corp", None));
        assert!(!whitelist.should_proxy("other.test", None));
    }

    #[test]
    fn a_process_rule_turns_on_the_process_lookup() {
        let rules: Vec<rules::Rule> =
            ["PROCESS-NAME,chrome.exe", "PROCESS-NAME,firefox.exe,DIRECT"]
                .iter()
                .filter_map(|line| rules::parse_rule(line))
                .collect();
        let shared = test_shared(vec![test_node()], rules);
        assert!(shared.needs_process());
        assert!(shared.should_proxy("a.test", Some("chrome.exe")));
        assert!(!shared.should_proxy("a.test", Some("firefox.exe")));
        // 两条进程规则都不命中 ⇒ 落到黑名单兜底（走节点）。
        assert!(shared.should_proxy("a.test", Some("edge.exe")));
        // 查不到进程 ≠ 不是它：进程规则不命中，仍然按兜底走。
        assert!(shared.should_proxy("a.test", None));
    }

    /// 面板参数：模式 / 名单类型 / 名单 → 规则层排在最前，来源也一并带回。
    #[test]
    fn parse_config_reads_mode_and_puts_the_plugin_rule_layer_first() {
        let payload = serde_json::json!({
            "links": ["ss://YWVzLTI1Ni1nY206cHc@a.com:8388#A"],
            "mode": "global",
            "ruleMode": "whitelist",
            "ruleDomains": ["only.test", "# 注释"],
            "rules": ["MATCH,DIRECT"],
        })
        .to_string();
        let config = parse_config(&payload).unwrap();
        assert_eq!(config.mode, Mode::Global);
        assert_eq!(config.rule_mode, "whitelist");
        assert_eq!(config.rule_domains.len(), 2);
        assert!(!config.fallback_proxy, "白名单 ⇒ 名单外直连");
        assert_eq!(config.sources, vec!["分享链接".to_string()]);
        // 名单层（白名单 ⇒ 名单内走节点）在前，订阅规则在后。
        assert_eq!(config.rules.len(), 2);
        assert_eq!(config.rules[0].label(), "DOMAIN-SUFFIX,only.test,PROXY");
        assert_eq!(config.rules[1].label(), "MATCH,DIRECT");
    }

    /// 自定义服务器 → 分享链接：生成的链接必须能被订阅的同一个解析器读回来。
    #[test]
    fn custom_server_builds_a_link_the_parser_accepts() {
        let ss: serde_json::Value = serde_json::from_str(&build_custom_link_json(
            r#"{"kind":"ss","name":"我的 节点#1","server":"1.2.3.4","port":8388,
               "password":"p@ss","method":"aes-256-gcm"}"#,
        ))
        .unwrap();
        assert_eq!(ss["ok"], true, "{ss}");
        let node = clash::parse_link(ss["link"].as_str().unwrap()).unwrap();
        assert_eq!(node.name, "我的 节点#1", "备注里的 # 不能把链接截断");
        assert_eq!(node.server, "1.2.3.4");
        assert_eq!(node.port, 8388);
        match node.protocol {
            Protocol::Ss {
                method, password, ..
            } => {
                assert_eq!(method, "aes-256-gcm");
                assert_eq!(password, "p@ss");
            }
            other => panic!("解析回来的不是 ss：{other:?}"),
        }

        let trojan: serde_json::Value = serde_json::from_str(&build_custom_link_json(
            r#"{"kind":"trojan","name":"t","server":"example.com","port":443,"password":"p@ss:w/rd"}"#,
        ))
        .unwrap();
        assert_eq!(trojan["ok"], true, "{trojan}");
        let node = clash::parse_link(trojan["link"].as_str().unwrap()).unwrap();
        assert_eq!(node.protocol.kind(), "trojan");

        let vless: serde_json::Value = serde_json::from_str(&build_custom_link_json(
            r#"{"kind":"vless","name":"v6","server":"2001:db8::1","port":8443,
               "uuid":"b831381d-6324-4d53-ad4f-8cda48b30811"}"#,
        ))
        .unwrap();
        assert_eq!(vless["ok"], true, "{vless}");
        let node = clash::parse_link(vless["link"].as_str().unwrap()).unwrap();
        assert_eq!(node.protocol.kind(), "vless");
        assert_eq!(node.server, "2001:db8::1", "IPv6 地址要能读回来");

        let vmess: serde_json::Value = serde_json::from_str(&build_custom_link_json(
            r#"{"kind":"vmess","name":"vm","server":"example.com","port":443,
               "uuid":"b831381d-6324-4d53-ad4f-8cda48b30811"}"#,
        ))
        .unwrap();
        assert_eq!(vmess["ok"], true, "{vmess}");
        let node = clash::parse_link(vmess["link"].as_str().unwrap()).unwrap();
        assert_eq!(node.name, "vm");
        match node.protocol {
            Protocol::Vmess { uuid, alter_id, .. } => {
                assert_eq!(uuid, "b831381d-6324-4d53-ad4f-8cda48b30811");
                assert_eq!(alter_id, 0, "自定义表单只生成 AEAD（aid=0）");
            }
            other => panic!("解析回来的不是 vmess：{other:?}"),
        }

        let anytls: serde_json::Value = serde_json::from_str(&build_custom_link_json(
            r#"{"kind":"anytls","name":"a","server":"example.com","port":443,"password":"p@ss:w/rd"}"#,
        ))
        .unwrap();
        assert_eq!(anytls["ok"], true, "{anytls}");
        let node = clash::parse_link(anytls["link"].as_str().unwrap()).unwrap();
        assert_eq!(node.protocol.kind(), "anytls");
        assert!(node.tls.enabled, "anytls 必须走 TLS");

        let hy2: serde_json::Value = serde_json::from_str(&build_custom_link_json(
            r#"{"kind":"hysteria2","name":"h","server":"example.com","port":8443,"password":"p@ss:w/rd"}"#,
        ))
        .unwrap();
        assert_eq!(hy2["ok"], true, "{hy2}");
        let node = clash::parse_link(hy2["link"].as_str().unwrap()).unwrap();
        match node.protocol {
            Protocol::Hysteria2 { password, .. } => {
                assert_eq!(password, "p@ss:w/rd", "密码里的 : 与 / 必须原样读回")
            }
            other => panic!("解析回来的不是 hysteria2：{other:?}"),
        }

        let tuic: serde_json::Value = serde_json::from_str(&build_custom_link_json(
            r#"{"kind":"tuic","name":"t","server":"example.com","port":443,
               "uuid":"b831381d-6324-4d53-ad4f-8cda48b30811","password":"p@ss:w/rd"}"#,
        ))
        .unwrap();
        assert_eq!(tuic["ok"], true, "{tuic}");
        let node = clash::parse_link(tuic["link"].as_str().unwrap()).unwrap();
        match node.protocol {
            Protocol::Tuic {
                uuid,
                password,
                udp_relay_mode,
            } => {
                assert_eq!(uuid, "b831381d-6324-4d53-ad4f-8cda48b30811");
                assert_eq!(password, "p@ss:w/rd");
                assert!(udp_relay_mode.is_empty(), "表单不写 udp-relay-mode");
            }
            other => panic!("解析回来的不是 tuic：{other:?}"),
        }
    }

    /// 自定义表单的 `mux` → 分享链接里的 `mux=1` → 解析回 `Node.mux`。
    #[test]
    fn custom_links_can_enable_mux_cool() {
        let vless: serde_json::Value = serde_json::from_str(&build_custom_link_json(
            r#"{"kind":"vless","name":"v","server":"a.com","port":443,
               "uuid":"b831381d-6324-4d53-ad4f-8cda48b30811","mux":true}"#,
        ))
        .unwrap();
        assert_eq!(vless["ok"], true, "{vless}");
        let node = clash::parse_link(vless["link"].as_str().unwrap()).unwrap();
        assert!(node.mux, "vless 链接应带回 mux=1");

        let trojan: serde_json::Value = serde_json::from_str(&build_custom_link_json(
            r#"{"kind":"trojan","name":"t","server":"a.com","port":443,"password":"p","mux":"1"}"#,
        ))
        .unwrap();
        let node = clash::parse_link(trojan["link"].as_str().unwrap()).unwrap();
        assert!(node.mux);

        let vmess: serde_json::Value = serde_json::from_str(&build_custom_link_json(
            r#"{"kind":"vmess","name":"m","server":"a.com","port":443,
               "uuid":"b831381d-6324-4d53-ad4f-8cda48b30811","mux":true}"#,
        ))
        .unwrap();
        let node = clash::parse_link(vmess["link"].as_str().unwrap()).unwrap();
        assert!(node.mux, "vmess JSON 的 mux 字段要能读回");

        // 不勾选时链接里不带 mux，解析回 false。
        let plain: serde_json::Value = serde_json::from_str(&build_custom_link_json(
            r#"{"kind":"vless","name":"v","server":"a.com","port":443,
               "uuid":"b831381d-6324-4d53-ad4f-8cda48b30811"}"#,
        ))
        .unwrap();
        let node = clash::parse_link(plain["link"].as_str().unwrap()).unwrap();
        assert!(!node.mux);
    }

    /// 字段不合法 ⇒ 在加入清单之前就给一句人话。
    #[test]
    fn custom_server_rejects_bad_fields_with_reasons() {
        let cases = [
            (
                r#"{"kind":"ss","server":"","port":8388,"password":"x"}"#,
                "服务器地址",
            ),
            (
                r#"{"kind":"ss","server":"a.com","port":70000,"password":"x"}"#,
                "端口",
            ),
            (
                r#"{"kind":"ss","server":"a.com","port":8388,"password":""}"#,
                "密码",
            ),
            (r#"{"kind":"vless","server":"a.com","port":443}"#, "UUID"),
            (
                r#"{"kind":"tuic","server":"a.com","port":443,"password":"x"}"#,
                "UUID",
            ),
            (
                r#"{"kind":"tuic","server":"a.com","port":443,
                   "uuid":"b831381d-6324-4d53-ad4f-8cda48b30811"}"#,
                "密码",
            ),
            (
                r#"{"kind":"wireguard","server":"a.com","port":443,"password":"x"}"#,
                "不支持",
            ),
        ];
        for (payload, expected) in cases {
            let reply: serde_json::Value =
                serde_json::from_str(&build_custom_link_json(payload)).unwrap();
            assert_eq!(reply["ok"], false, "{payload}");
            let error = reply["error"].as_str().unwrap_or("");
            assert!(error.contains(expected), "{payload} ⇒ {error}");
        }
        // 坏 JSON 也要能读。
        let broken: serde_json::Value =
            serde_json::from_str(&build_custom_link_json("{not json")).unwrap();
        assert_eq!(broken["ok"], false);
        assert!(broken["error"].as_str().unwrap().contains("不是合法 JSON"));
    }

    /// 模式优先级：`global` 盖过规则表，`direct` 全部直连；本机永远是直连。
    #[test]
    fn mode_global_and_direct_override_the_rule_table() {
        let rules = vec![rules::parse_rule("MATCH,DIRECT").unwrap()];
        let shared = test_shared(vec![test_node()], rules);
        assert!(!shared.should_proxy("example.com", None));
        shared.set_mode(Mode::Global);
        assert!(shared.should_proxy("example.com", None));
        assert!(!shared.should_proxy("127.0.0.1", None));
        assert_eq!(
            shared.decide_route("example.com", None).0,
            Route::Node,
            "全局模式下必须走节点"
        );
        shared.set_mode(Mode::Direct);
        assert!(!shared.should_proxy("example.com", None));
        assert_eq!(shared.decide_route("example.com", None).1, "全局直连");
        assert_eq!(shared.mode(), Mode::Direct);
    }

    /// 请求日志：只留最近 [`REQUEST_LOG_CAP`] 条，丢最旧的。
    #[test]
    fn request_log_keeps_only_the_latest_entries() {
        let mut log: VecDeque<RequestEntry> = VecDeque::new();
        for index in 0..(REQUEST_LOG_CAP + 5) {
            push_entry(
                &mut log,
                RequestEntry {
                    host: format!("h{index}.test"),
                    port: 443,
                    route: Route::Direct,
                    detail: "直连".to_string(),
                },
            );
        }
        assert_eq!(log.len(), REQUEST_LOG_CAP);
        assert_eq!(log.front().unwrap().host, "h5.test");
        assert_eq!(
            log.back().unwrap().host,
            format!("h{}.test", REQUEST_LOG_CAP + 4)
        );
    }

    /// 出口探测的响应解析：只要 200 + 两个字母的国家代码。
    #[test]
    fn country_response_needs_200_and_a_two_letter_body() {
        let ok = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nJP\n";
        assert_eq!(parse_country_response(ok).as_deref(), Some("JP"));
        // 正文还没收全 / 不是国家代码 / 状态码不对 ⇒ None（调用方继续读或如实报错）。
        assert!(parse_country_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nJ").is_none());
        assert!(parse_country_response(b"HTTP/1.1 200 OK\r\n\r\nhello").is_none());
        assert!(parse_country_response(b"HTTP/1.1 301 Moved\r\n\r\n").is_none());
        assert!(parse_country_response(b"garbage").is_none());
    }

    /// 界面读的那份 JSON：字段名 / 路由取值必须和面板约定一致。
    #[test]
    fn requests_json_reports_recorded_entries() {
        let host = "log-json-unique.test";
        record_request(host, 8443, Route::Node, "订阅 · 测试节点".to_string());
        let items: serde_json::Value =
            serde_json::from_str(&requests_json(REQUEST_LOG_CAP)).unwrap();
        let hit = items
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["host"] == host)
            .expect("刚记的条目必须能读到");
        assert_eq!(hit["port"], 8443);
        assert_eq!(hit["route"], "node");
        assert_eq!(hit["detail"], "订阅 · 测试节点");
    }

    /// 测速探针：请求行 / Host 头逐字对，且只要对端回**第一个字节**就算成功
    /// （4xx 也算 —— 探针量的是「经节点能不能拿到响应」，不是网页内容）。
    #[test]
    fn probe_sends_a_head_request_and_accepts_any_status() {
        use std::io::{Read, Write};
        use std::net::{Shutdown, TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 512];
            let read = stream.read(&mut buffer).unwrap();
            let text = String::from_utf8_lossy(&buffer[..read]).to_string();
            assert!(text.starts_with("HEAD / HTTP/1.1\r\n"), "{text}");
            assert!(text.contains("Host: probe.example\r\n"), "{text}");
            assert!(text.contains("Connection: close\r\n"), "{text}");
            stream
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            // 本机 loopback 上「写完就 drop」偶尔以 RST 收场，先半关再留一点时间。
            let _ = stream.shutdown(Shutdown::Write);
            thread::sleep(Duration::from_millis(50));
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let mut stream: transport::BoxStream = Box::new(tcp);
        http_probe(&mut stream, "probe.example", Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
    }

    /// 对端一句话都不回就关 ⇒ 必须如实报「等首个响应字节失败」，不能当成功。
    #[test]
    fn probe_reports_a_closed_connection() {
        use std::io::Read;
        use std::net::{Shutdown, TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // 先把请求读掉，再半关：这样失败点稳定落在「等首个响应字节」而不是写。
            let mut buffer = [0u8; 512];
            let _ = stream.read(&mut buffer);
            let _ = stream.shutdown(Shutdown::Both);
        });

        let tcp = TcpStream::connect(addr).unwrap();
        let mut stream: transport::BoxStream = Box::new(tcp);
        let error = http_probe(&mut stream, "probe.example", Duration::from_secs(1))
            .err()
            .expect("对端直接关闭必须报错");
        assert!(error.contains("首个响应字节"), "{error}");
        handle.join().unwrap();
    }

    /// 时延应答的口径字段必须是 `http`（不是旧的 `tcp`）。
    #[test]
    fn delay_reports_the_http_scope() {
        let reply: serde_json::Value = serde_json::from_str(&delay_json(9999)).unwrap();
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["scope"], "http");
        assert!(reply["error"].as_str().unwrap().contains("序号越界"));
    }

    /// **真机验收（`#[ignore]`，需要外部服务器）**：面板那条链路走一遍 ——
    /// 启动（带插件名单层）→ 经本地端口发一个明文 HTTP 请求 → 请求日志能读到这次访问
    /// → 切全局模式 → 停止。验证的是「界面看到的东西」与「引擎实际做的事」是同一份。
    ///
    /// ```powershell
    /// $env:LUNAC_LIVE_LINKS = "d:\cc\lunac-master\temp\live-links.txt"
    /// cargo test --lib live_panel -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn live_panel_flow_records_requests_and_switches_mode() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::time::Duration;

        let path = match std::env::var("LUNAC_LIVE_LINKS") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => {
                eprintln!("跳过：没有设置 LUNAC_LIVE_LINKS");
                return;
            }
        };
        let text = std::fs::read_to_string(&path).expect("读不到链接文件");
        let candidates: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|line| line.contains("://") && clash::parse_link(line).is_ok())
            .take(5)
            .collect();
        assert!(!candidates.is_empty(), "链接文件里没有可解析的分享链接");

        let port = 17890u16;
        let mut last_error = String::from("没有可用的节点");
        let mut passed = false;
        for link in candidates {
            let payload = serde_json::json!({
                "links": [link],
                "port": port,
                "mode": "rule",
                // 名单给一条与目标无关的域名：黑名单语义下不会命中 baidu.com。
                "ruleMode": "blacklist",
                "ruleDomains": ["intranet.invalid"],
            })
            .to_string();
            let started: serde_json::Value = serde_json::from_str(&start_json(&payload)).unwrap();
            // 断言放在停止之后：失败也不把引擎留在运行态。
            let outcome = (|| -> Result<(), String> {
                if started["ok"] != true {
                    return Err(format!("启动失败：{}", started["error"]));
                }
                let mut client = TcpStream::connect(("127.0.0.1", port))
                    .map_err(|e| format!("连本地端口失败：{e}"))?;
                client
                    .set_read_timeout(Some(Duration::from_secs(20)))
                    .map_err(|e| format!("设置读超时失败：{e}"))?;
                client
                    .write_all(
                        b"GET http://www.baidu.com/ HTTP/1.1\r\nHost: www.baidu.com\r\nConnection: close\r\n\r\n",
                    )
                    .map_err(|e| format!("发请求失败：{e}"))?;
                let mut first = [0u8; 1];
                client
                    .read_exact(&mut first)
                    .map_err(|e| format!("读响应失败：{e}"))?;

                let log: serde_json::Value = serde_json::from_str(&requests_json(50)).unwrap();
                let hit = log
                    .as_array()
                    .and_then(|items| {
                        items
                            .iter()
                            .find(|item| item["host"] == "www.baidu.com" && item["route"] == "node")
                    })
                    .ok_or_else(|| format!("请求日志里没有这次访问：{log}"))?;
                let detail = hit["detail"].as_str().unwrap_or("");
                if !detail.contains("分享链接") {
                    return Err(format!("日志里的来源不对：{detail}"));
                }

                let switched: serde_json::Value =
                    serde_json::from_str(&set_mode_json("global")).unwrap();
                if switched["ok"] != true || switched["status"]["mode"] != "global" {
                    return Err(format!("模式没有切到 global：{switched}"));
                }
                Ok(())
            })();
            let stopped: serde_json::Value = serde_json::from_str(&stop_json()).unwrap();
            if stopped["ok"] != true {
                panic!("停止失败：{stopped}");
            }
            match outcome {
                Ok(()) => {
                    passed = true;
                    break;
                }
                Err(reason) => {
                    eprintln!("该节点未通过（换下一条）：{reason}");
                    last_error = reason;
                }
            }
        }
        if !passed {
            panic!("面板链路验收失败：{last_error}");
        }
        println!("PASS 面板链路：启动 → 经节点请求 → 日志可见 → 模式热切 → 停止");
    }

    /// **真机验收（`#[ignore]`，需要外部服务器）**：读 `LUNAC_LIVE_LINKS` 指向的链接列表，
    /// 逐节点经引擎拨号到 `www.baidu.com:80` 并发一个 HEAD，打印耗时。
    ///
    /// 用法（链接文件不进仓库）：
    /// ```powershell
    /// $env:LUNAC_LIVE_LINKS = "d:\cc\lunac-master\temp\live-links.txt"
    /// cargo test --lib live_nodes -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn live_nodes_all_protocols() {
        use std::io::{Read, Write};
        use std::time::{Duration, Instant};

        let path = match std::env::var("LUNAC_LIVE_LINKS") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => {
                eprintln!("跳过：没有设置 LUNAC_LIVE_LINKS");
                return;
            }
        };
        let text = std::fs::read_to_string(&path).expect("读不到链接文件");
        let parsed = clash::parse_subscription(&text);
        println!(
            "解析：{} 个节点，跳过 {} 条",
            parsed.nodes.len(),
            parsed.skipped.len()
        );
        for reason in &parsed.skipped {
            println!("  跳过原因：{reason}");
        }
        let mut passed = 0usize;
        for node in &parsed.nodes {
            let started = Instant::now();
            let outcome = server::dial_node(
                node,
                &server::Target {
                    host: "www.baidu.com".to_string(),
                    port: 80,
                },
            )
            .and_then(|mut stream| {
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .map_err(|e| format!("设置超时失败：{e}"))?;
                stream
                    .write_all(b"HEAD http://www.baidu.com/ HTTP/1.1\r\nHost: www.baidu.com\r\nConnection: close\r\n\r\n")
                    .map_err(|e| format!("写请求失败：{e}"))?;
                let mut first = [0u8; 1];
                match stream.read(&mut first) {
                    Ok(0) => Err("对端关闭（没拿到任何字节）".to_string()),
                    Ok(_) => Ok(first[0]),
                    Err(e) => Err(format!("读响应失败：{e}")),
                }
            });
            let millis = started.elapsed().as_millis();
            match outcome {
                Ok(_) => {
                    passed += 1;
                    println!(
                        "PASS  {:<28} {:>6} ms  {}",
                        node.name,
                        millis,
                        node.protocol.kind()
                    );
                }
                Err(reason) => {
                    println!(
                        "FAIL  {:<28} {:>6} ms  {} => {}",
                        node.name,
                        millis,
                        node.protocol.kind(),
                        reason
                    );
                }
            }
        }
        println!("结果：{passed}/{} 通过", parsed.nodes.len());
        assert!(passed > 0, "一个节点都没通，先看上面的 FAIL 原因");
    }
}
