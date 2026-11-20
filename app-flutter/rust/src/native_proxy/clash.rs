//! 订阅 / 分享链接解析（重建 v1 `native_proxy/clash.rs`）。
//!
//! 支持（与 v1 账本第 14 行一致）：`ss://`、`trojan://`、`vless://`、`vmess://`、
//! `anytls://`、`hysteria2://`、「整段 base64 的链接列表」，以及 **Clash / mihomo YAML**
//! 订阅的 `proxies:` 段（v2 自 2026-11-15 起用 `yaml-rust2` 真解析，不再按字符串猜）。
//! `tuic://` / `ssr://` / 未知插件 / 未知混淆 / 未重建的传输一律**跳过并给出原因**（v1 同口径）。
//! 传输：`ws`（`network: ws` / `type=ws`）、`http`（`network: http` + `http-opts` /
//! `type=http`）、`h2`（`network: h2` / `type=h2`）、`grpc`（`network: grpc` /
//! `type=grpc&serviceName=…`）、`xhttp`（`network: xhttp` + `xhttp-opts` / `type=xhttp`）
//! 已重建。

use base64::Engine as _;
use std::collections::HashMap;
use url::Url;

use super::{
    ExtraTransport, HttpOptions, Node, Protocol, RealityOptions, SsObfs, TlsOptions, WsOptions,
};

/// 十六进制 → 字节（奇数长度 / 非十六进制字符 ⇒ `None`）。
fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.len() % 2 != 0 {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}

/// 分享链接里的 REALITY 参数（`security=reality&pbk=…&sid=…&spx=…`）。
///
/// 与 v1 同口径：`pbk` 必须是 32 字节 X25519 公钥；`sid` 是十六进制短 ID（≤ 8 字节）。
/// 任何一项不合法都**如实报错**，绝不静默退回裸 TLS —— 那会把流量送到错误的对端。
fn reality_from_query(url: &Url, host: &str) -> Result<Option<RealityOptions>, String> {
    let security = query_first(url, "security").unwrap_or_default();
    let is_reality = security.trim().eq_ignore_ascii_case("reality");
    let raw_key = query_first(url, "pbk").unwrap_or_default();
    if !is_reality && raw_key.trim().is_empty() {
        return Ok(None);
    }
    if raw_key.trim().is_empty() {
        return Err("REALITY 链接缺 pbk（public key）".to_string());
    }
    let public_key =
        b64_decode(&raw_key).ok_or_else(|| format!("REALITY pbk 不是合法 base64：{raw_key}"))?;
    if public_key.len() != 32 {
        return Err(format!(
            "REALITY pbk 解码后不是 32 字节（实际 {}）",
            public_key.len()
        ));
    }
    let short_id = query_first(url, "sid").unwrap_or_default();
    let short_id =
        hex_bytes(&short_id).ok_or_else(|| format!("REALITY sid 必须是十六进制：{short_id}"))?;
    if short_id.len() > 8 {
        return Err(format!(
            "REALITY sid 最多 8 字节（实际 {}）",
            short_id.len()
        ));
    }
    let _ = host;
    Ok(Some(RealityOptions {
        public_key,
        short_id,
        spider_x: query_first(url, "spx").unwrap_or_default(),
    }))
}

/// Clash YAML 里的 REALITY 参数（`reality-opts: {public-key, short-id}`；短 ID 也可能写在顶层）。
fn clash_reality(item: &yaml_rust2::Yaml, server: &str) -> Result<Option<RealityOptions>, String> {
    let Some(opts) = yaml_get(item, "reality-opts") else {
        return Ok(None);
    };
    let raw_key = yaml_text(opts, "public-key")
        .or_else(|| yaml_text(opts, "publicKey"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "REALITY 节点缺 public-key（pbk）".to_string())?;
    let public_key = b64_decode(&raw_key)
        .ok_or_else(|| format!("REALITY public-key 不是合法 base64：{raw_key}"))?;
    if public_key.len() != 32 {
        return Err(format!(
            "REALITY public-key 解码后不是 32 字节（实际 {}）",
            public_key.len()
        ));
    }
    let raw_short = yaml_text(opts, "short-id")
        .or_else(|| yaml_text(opts, "shortId"))
        .or_else(|| yaml_text(item, "short-id"))
        .unwrap_or_default();
    let short_id = hex_bytes(&raw_short)
        .ok_or_else(|| format!("REALITY short-id 必须是十六进制：{raw_short}"))?;
    if short_id.len() > 8 {
        return Err(format!(
            "REALITY short-id 最多 8 字节（实际 {}）",
            short_id.len()
        ));
    }
    let _ = server;
    Ok(Some(RealityOptions {
        public_key,
        short_id,
        spider_x: yaml_text(item, "spider-x").unwrap_or_default(),
    }))
}

/// 订阅里的一个 `rule-providers:` 条目（`RULE-SET,<name>,<policy>` 指向它）。
///
/// 依据 mihomo 文档（`/en/config/rule-providers/`）：`type` = http / file / inline，
/// `behavior` = domain / ipcidr / classical，`format` = yaml / text / mrs。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuleProvider {
    pub kind: String,
    pub behavior: String,
    pub format: String,
    pub url: String,
    pub path: String,
    pub interval: u64,
    pub payload: Vec<String>,
}

/// 一次订阅解析的结果：可用节点 + 每条被跳过项的原因（面板 / 日志逐条展示）。
///
/// `rules` 是 Clash YAML 顶层 `rules:` 段（v1 用 mihomo 时这些规则直接生效；
/// 现在由 `mod.rs::parse_config` 展开进引擎规则表 —— 不认识的写法逐条记「已跳过」）。
pub struct ParsedSubscription {
    pub nodes: Vec<Node>,
    pub skipped: Vec<String>,
    pub rules: Vec<String>,
    /// 顶层 `rule-providers:`（`RULE-SET` 展开时按名字查）。
    pub providers: HashMap<String, RuleProvider>,
    pub note: String,
}

fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .replace('-', "+")
        .replace('_', "/");
    let trimmed = cleaned.trim_end_matches('=');
    base64::engine::general_purpose::STANDARD
        .decode(format!(
            "{trimmed}{}",
            "=".repeat((4 - trimmed.len() % 4) % 4)
        ))
        .ok()
        .or_else(|| {
            base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(trimmed)
                .ok()
        })
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(value) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// URL 里的主机名：IPv6 去掉方括号（`[::1]` → `::1`）。
///
/// 分享链接里 IPv6 必须写成 `[2001:db8::1]`，但 `Node.server` 要送给
/// `ToSocketAddrs` / SNI，带方括号会解析失败 —— 统一在这里剥掉。
fn url_host(url: &Url) -> Option<String> {
    url.host_str()
        .map(|host| host.trim().trim_matches(['[', ']']).to_string())
}

fn frag_name(url: &Url) -> String {
    let raw = url.fragment().unwrap_or("");
    let name = percent_decode(raw);
    if name.trim().is_empty() {
        format!(
            "{}:{}",
            url.host_str().unwrap_or(""),
            url.port().unwrap_or(0)
        )
    } else {
        name.trim().to_string()
    }
}

fn query_first(url: &Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.to_string())
}

fn truthy(value: Option<String>) -> bool {
    matches!(
        value
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// JSON 里的布尔位（`true` / `1` / `"1"` / `"true"` 都算真）。
fn json_flag(json: &serde_json::Value, key: &str) -> bool {
    match json.get(key) {
        Some(serde_json::Value::Bool(value)) => *value,
        Some(serde_json::Value::Number(number)) => number.as_i64().unwrap_or(0) != 0,
        Some(serde_json::Value::String(text)) => truthy(Some(text.clone())),
        _ => false,
    }
}

/// 解析一段订阅文本（链接列表 / 整段 base64）。
pub fn parse_subscription(text: &str) -> ParsedSubscription {
    let mut nodes = Vec::new();
    let mut skipped = Vec::new();
    let mut note = String::new();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return ParsedSubscription {
            nodes,
            skipped,
            rules: Vec::new(),
            providers: HashMap::new(),
            note,
        };
    }
    if looks_like_clash(trimmed) {
        let (parsed, reasons, rules, providers) = parse_clash_yaml(trimmed);
        return ParsedSubscription {
            nodes: parsed,
            skipped: reasons,
            rules,
            providers,
            note: "订阅按 Clash YAML（proxies:）解析".to_string(),
        };
    }
    let body = if !trimmed.contains("://") {
        match b64_decode(trimmed).and_then(|bytes| String::from_utf8(bytes).ok()) {
            Some(decoded) => {
                note = "订阅按 base64 解码".to_string();
                decoded
            }
            None => {
                skipped.push(
                    "订阅内容既不是链接、也不是可解码的 base64、也没有 proxies: 段".to_string(),
                );
                return ParsedSubscription {
                    nodes,
                    skipped,
                    rules: Vec::new(),
                    providers: HashMap::new(),
                    note,
                };
            }
        }
    } else {
        trimmed.to_string()
    };
    // base64 里包一份 YAML 的订阅也存在（少数机场这么发）。
    if looks_like_clash(&body) {
        let (parsed, reasons, rules, providers) = parse_clash_yaml(&body);
        return ParsedSubscription {
            nodes: parsed,
            skipped: reasons,
            rules,
            providers,
            note: format!("{note}，再按 Clash YAML 解析"),
        };
    }
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        match parse_link(line) {
            Ok(node) => nodes.push(node),
            Err(reason) => skipped.push(reason),
        }
    }
    ParsedSubscription {
        nodes,
        skipped,
        rules: Vec::new(),
        providers: HashMap::new(),
        note,
    }
}

/// 是否是一份 Clash / mihomo 配置（看有没有顶层 `proxies:` 段）。
fn looks_like_clash(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim_start().starts_with("proxies:"))
}

/// 解析 Clash YAML 的 `proxies:` 段（以及顶层 `rules:` / `rule-providers:`）。
///
/// 返回 `(节点, 跳过原因, 规则文本, 规则集)`。规则按原文顺序返回，不在这里判定
/// 「认不认识」—— 判定归 `rules::parse_rule`（同一处口径）；`RULE-SET` 的展开在 `ruleset.rs`。
fn parse_clash_yaml(
    text: &str,
) -> (
    Vec<Node>,
    Vec<String>,
    Vec<String>,
    HashMap<String, RuleProvider>,
) {
    let mut nodes = Vec::new();
    let mut skipped = Vec::new();
    let document = match yaml_rust2::YamlLoader::load_from_str(text) {
        Ok(documents) => match documents.into_iter().next() {
            Some(document) => document,
            None => {
                skipped.push("Clash YAML 是空的".to_string());
                return (nodes, skipped, Vec::new(), HashMap::new());
            }
        },
        Err(error) => {
            skipped.push(format!("Clash YAML 解析失败：{error}"));
            return (nodes, skipped, Vec::new(), HashMap::new());
        }
    };
    // 规则段先取（节点缺失时也要把规则带出去：订阅可以只给规则不给节点）。
    let rules: Vec<String> = yaml_get(&document, "rules")
        .and_then(|value| value.as_vec())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|text| text.trim().to_string()))
                .filter(|text| !text.is_empty() && !text.starts_with('#'))
                .collect()
        })
        .unwrap_or_default();
    let providers = parse_rule_providers(&document, &mut skipped);
    let Some(proxies) = yaml_get(&document, "proxies").and_then(|value| value.as_vec()) else {
        skipped.push("Clash YAML 里没有 proxies: 段".to_string());
        return (nodes, skipped, rules, providers);
    };
    for (index, item) in proxies.iter().enumerate() {
        match clash_node(item) {
            Ok(node) => nodes.push(node),
            Err(reason) => skipped.push(format!("proxies 第 {} 条：{reason}", index + 1)),
        }
    }
    (nodes, skipped, rules, providers)
}

/// 顶层 `rule-providers:`：只取 `RULE-SET` 展开需要的字段，不做任何联网动作。
fn parse_rule_providers(
    document: &yaml_rust2::Yaml,
    skipped: &mut Vec<String>,
) -> HashMap<String, RuleProvider> {
    let mut providers = HashMap::new();
    let Some(section) = yaml_get(document, "rule-providers") else {
        return providers;
    };
    let Some(entries) = section.as_hash() else {
        skipped.push("rule-providers 不是键值段（已忽略）".to_string());
        return providers;
    };
    for (key, item) in entries {
        let Some(name) = key.as_str().map(str::trim).filter(|name| !name.is_empty()) else {
            continue;
        };
        let provider = RuleProvider {
            kind: yaml_text(item, "type")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase(),
            behavior: yaml_text(item, "behavior")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase(),
            format: yaml_text(item, "format")
                .unwrap_or_else(|| "yaml".to_string())
                .trim()
                .to_ascii_lowercase(),
            url: yaml_text(item, "url")
                .unwrap_or_default()
                .trim()
                .to_string(),
            path: yaml_text(item, "path")
                .unwrap_or_default()
                .trim()
                .to_string(),
            interval: yaml_text(item, "interval")
                .and_then(|text| text.trim().parse::<u64>().ok())
                .unwrap_or(0),
            payload: yaml_get(item, "payload")
                .and_then(|value| value.as_vec())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(|text| text.trim().to_string()))
                        .filter(|text| !text.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        };
        providers.insert(name.to_string(), provider);
    }
    providers
}

fn yaml_get<'a>(node: &'a yaml_rust2::Yaml, key: &str) -> Option<&'a yaml_rust2::Yaml> {
    node.as_hash()?.iter().find_map(|(k, v)| {
        if k.as_str() == Some(key) {
            Some(v)
        } else {
            None
        }
    })
}

/// 规则集文件里的 `payload:` 列表（`behavior` 只看条目，策略由 `RULE-SET` 决定）。
pub fn yaml_payload(node: &yaml_rust2::Yaml) -> Option<Vec<String>> {
    let items = yaml_get(node, "payload")?.as_vec()?;
    let lines = items
        .iter()
        .filter_map(|item| item.as_str().map(|text| text.trim().to_string()))
        .filter(|text| !text.is_empty() && !text.starts_with('#'))
        .collect::<Vec<String>>();
    Some(lines)
}

fn yaml_text(node: &yaml_rust2::Yaml, key: &str) -> Option<String> {
    match yaml_get(node, key)? {
        yaml_rust2::Yaml::String(value) => Some(value.clone()),
        yaml_rust2::Yaml::Integer(value) => Some(value.to_string()),
        yaml_rust2::Yaml::Real(value) => Some(value.clone()),
        yaml_rust2::Yaml::Boolean(value) => Some(value.to_string()),
        _ => None,
    }
}

fn yaml_flag(node: &yaml_rust2::Yaml, key: &str) -> bool {
    match yaml_get(node, key) {
        Some(yaml_rust2::Yaml::Boolean(value)) => *value,
        Some(yaml_rust2::Yaml::Integer(value)) => *value != 0,
        Some(yaml_rust2::Yaml::String(value)) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "true" | "1" | "yes"
        ),
        _ => false,
    }
}

/// YAML 标量或数组的第一个非空字符串（Clash 的 `path` / `Host` 两种写法都合法）。
fn yaml_first_text(value: &yaml_rust2::Yaml) -> Option<String> {
    if let Some(list) = value.as_vec() {
        return list
            .iter()
            .find_map(|item| item.as_str().map(|text| text.trim().to_string()))
            .filter(|text| !text.is_empty());
    }
    value
        .as_str()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// YAML 的 `ws-opts` / 旧式 `ws-path` + `ws-headers`、`http-opts`，以及 `h2-opts` / `grpc-opts`。
fn clash_transport(
    node: &yaml_rust2::Yaml,
    server: &str,
) -> Result<
    (
        Option<WsOptions>,
        Option<HttpOptions>,
        Option<ExtraTransport>,
    ),
    String,
> {
    let network = yaml_text(node, "network")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match network.as_str() {
        "" | "tcp" | "none" => Ok((None, None, None)),
        "ws" => {
            let opts = yaml_get(node, "ws-opts");
            let path = opts
                .and_then(|value| yaml_text(value, "path"))
                .or_else(|| yaml_text(node, "ws-path"))
                .unwrap_or_default();
            let host = opts
                .and_then(|value| yaml_get(value, "headers"))
                .and_then(ws_host_header)
                .or_else(|| yaml_get(node, "ws-headers").and_then(ws_host_header))
                .unwrap_or_default();
            Ok((
                Some(WsOptions {
                    path: if path.trim().is_empty() {
                        "/".to_string()
                    } else {
                        path.trim().to_string()
                    },
                    host: if host.trim().is_empty() {
                        server.to_string()
                    } else {
                        host.trim().to_string()
                    },
                }),
                None,
                None,
            ))
        }
        "http" => {
            let opts = yaml_get(node, "http-opts");
            let method = opts
                .and_then(|value| yaml_text(value, "method"))
                .unwrap_or_default();
            let path = opts
                .and_then(|value| yaml_get(value, "path"))
                .and_then(yaml_first_text)
                .unwrap_or_default();
            let host = opts
                .and_then(|value| yaml_get(value, "headers"))
                .and_then(ws_host_header)
                .unwrap_or_default();
            Ok((
                None,
                Some(HttpOptions {
                    method: method.trim().to_string(),
                    path: if path.trim().is_empty() {
                        "/".to_string()
                    } else {
                        path.trim().to_string()
                    },
                    host: if host.trim().is_empty() {
                        server.to_string()
                    } else {
                        host.trim().to_string()
                    },
                }),
                None,
            ))
        }
        // `network: h2` —— POST 到 `h2-opts.path`（缺省 `/`），之后整条流是裸字节。
        "h2" => {
            let opts = yaml_get(node, "h2-opts");
            let path = opts
                .and_then(|value| yaml_get(value, "path"))
                .and_then(yaml_first_text)
                .unwrap_or_default();
            let host = opts
                .and_then(|value| yaml_get(value, "host"))
                .and_then(yaml_first_text)
                .unwrap_or_default();
            Ok((
                None,
                None,
                Some(ExtraTransport::H2 {
                    host: if host.trim().is_empty() {
                        server.to_string()
                    } else {
                        host.trim().to_string()
                    },
                    path: if path.trim().is_empty() {
                        "/".to_string()
                    } else {
                        path.trim().to_string()
                    },
                }),
            ))
        }
        // `network: grpc` —— POST 到 `/<grpc-service-name>/Tun`，每条数据套 gRPC 消息头。
        "grpc" => {
            let service = yaml_get(node, "grpc-opts")
                .and_then(|value| yaml_text(value, "grpc-service-name"))
                .unwrap_or_default();
            Ok((
                None,
                None,
                Some(ExtraTransport::Grpc {
                    service_name: if service.trim().is_empty() {
                        "GunService".to_string()
                    } else {
                        service.trim().to_string()
                    },
                }),
            ))
        }
        // `network: xhttp` —— Xray SplitHTTP（`stream-one` / `stream-up`）。
        "xhttp" => {
            let opts = yaml_get(node, "xhttp-opts");
            let path = opts
                .and_then(|value| yaml_get(value, "path"))
                .and_then(yaml_first_text)
                .or_else(|| yaml_text(node, "path"))
                .unwrap_or_default();
            let host = opts
                .and_then(|value| yaml_get(value, "host"))
                .and_then(yaml_first_text)
                .or_else(|| yaml_text(node, "host"))
                .unwrap_or_default();
            let mode = opts
                .and_then(|value| yaml_text(value, "mode"))
                .or_else(|| yaml_text(node, "mode"))
                .unwrap_or_default();
            Ok((
                None,
                None,
                Some(ExtraTransport::XHttp {
                    host: if host.trim().is_empty() {
                        server.to_string()
                    } else {
                        host.trim().to_string()
                    },
                    path: if path.trim().is_empty() {
                        "/".to_string()
                    } else {
                        path.trim().to_string()
                    },
                    mode: mode.trim().to_ascii_lowercase(),
                }),
            ))
        }
        other => Err(format!("传输 {other} 本版尚未重建（已跳过）")),
    }
}

fn ws_host_header(headers: &yaml_rust2::Yaml) -> Option<String> {
    ["Host", "host"]
        .iter()
        .find_map(|key| yaml_get(headers, key).and_then(yaml_first_text))
}

/// mihomo 的 `ca-str`（内联 PEM）→ 自定义根证书。
///
/// **刻意不支持 `ca`（文件路径）**：订阅是外部输入，允许它指定任意本地路径
/// 就等于给订阅开了一个「读盘探针」；要装自签 CA 请把 PEM 贴进 `ca-str`。
fn clash_tls(node: &yaml_rust2::Yaml, default_enabled: bool, server: &str) -> TlsOptions {
    let enabled = if yaml_get(node, "tls").is_some() {
        yaml_flag(node, "tls")
    } else {
        default_enabled
    };
    let sni = yaml_text(node, "sni")
        .or_else(|| yaml_text(node, "servername"))
        .unwrap_or_default();
    let alpn = yaml_get(node, "alpn")
        .and_then(|value| value.as_vec())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|text| text.trim().to_string()))
                .filter(|text| !text.is_empty())
                .collect()
        })
        .unwrap_or_default();
    TlsOptions {
        enabled,
        sni: if sni.trim().is_empty() {
            server.to_string()
        } else {
            sni.trim().to_string()
        },
        allow_insecure: yaml_flag(node, "skip-cert-verify"),
        alpn,
        ca: yaml_text(node, "ca-str").unwrap_or_default(),
    }
}

/// 把 YAML 的 `plugin` + `plugin-opts` 折成 SIP003 的 `name;key=value` 规格，
/// 交给 [`parse_ss_plugin`] 复用同一套「支持 / 拒绝」判定。
fn clash_ss_plugin(node: &yaml_rust2::Yaml) -> Option<String> {
    let plugin = yaml_text(node, "plugin")?;
    let plugin = plugin.trim();
    if plugin.is_empty() {
        return None;
    }
    let mut spec = if plugin.eq_ignore_ascii_case("obfs") {
        "obfs-local".to_string()
    } else {
        plugin.to_string()
    };
    if let Some(opts) = yaml_get(node, "plugin-opts") {
        for key in ["mode", "host", "path"] {
            if let Some(value) = yaml_text(opts, key) {
                if !value.trim().is_empty() {
                    spec.push_str(&format!(";{key}={}", value.trim()));
                }
            }
        }
        if yaml_flag(opts, "tls") && !spec.split(';').any(|part| part.trim() == "tls") {
            spec.push_str(";tls");
        }
    }
    Some(spec)
}

/// 一条 Clash `proxies` 项 → [`Node`]；不支持的形状返回一句人话原因。
fn clash_node(item: &yaml_rust2::Yaml) -> Result<Node, String> {
    if item.as_hash().is_none() {
        return Err("节点不是键值对".to_string());
    }
    let kind = yaml_text(item, "type")
        .map(|value| value.trim().to_ascii_lowercase())
        .ok_or_else(|| "节点缺 type".to_string())?;
    let server = yaml_text(item, "server")
        .unwrap_or_default()
        .trim()
        .to_string();
    if server.is_empty() {
        return Err("节点缺 server".to_string());
    }
    let raw_port = yaml_text(item, "port").unwrap_or_default();
    let port = raw_port
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("节点端口不是单个数字（{raw_port}）"))?;
    if port == 0 {
        return Err("节点端口是 0".to_string());
    }
    let name = yaml_text(item, "name")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{kind} {server}:{port}"));
    match kind.as_str() {
        "ss" => {
            let method = yaml_text(item, "cipher")
                .or_else(|| yaml_text(item, "method"))
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "ss 节点缺 cipher".to_string())?;
            let password =
                yaml_text(item, "password").ok_or_else(|| "ss 节点缺 password".to_string())?;
            let spec = clash_ss_plugin(item);
            let (ws, tls, obfs) = parse_ss_plugin(spec.as_deref().unwrap_or(""), &server)?;
            Ok(Node {
                name,
                server,
                port,
                protocol: Protocol::Ss {
                    method,
                    password,
                    obfs,
                },
                tls,
                ws,
                http: None,
                extra: None,
                reality: None,
                mux: false,
            })
        }
        "trojan" => {
            let password =
                yaml_text(item, "password").ok_or_else(|| "trojan 节点缺 password".to_string())?;
            let tls = clash_tls(item, true, &server);
            let (ws, http, extra) = clash_transport(item, &server)?;
            let mut tls = h2_tls(tls, &extra, &server);
            let reality = clash_reality(item, &server)?;
            if reality.is_some() {
                if ws.is_some() || http.is_some() || extra.is_some() {
                    return Err("REALITY 只支持裸 TCP（不能与 ws / http / h2 叠用）".to_string());
                }
                tls.enabled = true;
            }
            Ok(Node {
                name,
                server,
                port,
                protocol: Protocol::Trojan { password },
                tls,
                ws,
                http,
                extra,
                reality,
                mux: false,
            })
        }
        "vless" => {
            let uuid = yaml_text(item, "uuid").ok_or_else(|| "vless 节点缺 uuid".to_string())?;
            if uuid.trim().is_empty() {
                return Err("vless 节点 uuid 是空的".to_string());
            }
            let flow = yaml_text(item, "flow").unwrap_or_default();
            let tls = clash_tls(item, false, &server);
            let (ws, http, extra) = clash_transport(item, &server)?;
            let mut tls = h2_tls(tls, &extra, &server);
            let reality = clash_reality(item, &server)?;
            if reality.is_some() {
                if ws.is_some() || http.is_some() || extra.is_some() {
                    return Err("REALITY 只支持裸 TCP（不能与 ws / http / h2 叠用）".to_string());
                }
                tls.enabled = true;
            }
            Ok(Node {
                name,
                server,
                port,
                protocol: Protocol::Vless {
                    uuid: uuid.trim().to_string(),
                    flow: flow.trim().to_string(),
                },
                tls,
                ws,
                http,
                extra,
                reality,
                mux: false,
            })
        }
        "vmess" => {
            let uuid = yaml_text(item, "uuid").ok_or_else(|| "vmess 节点缺 uuid".to_string())?;
            if uuid.trim().is_empty() {
                return Err("vmess 节点 uuid 是空的".to_string());
            }
            let alter_id = yaml_text(item, "alterId")
                .unwrap_or_default()
                .trim()
                .parse::<u32>()
                .unwrap_or(0);
            let security = yaml_text(item, "cipher")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "auto".to_string());
            // `Opt A`（认证长度）：mihomo 的 `authenticated-length`，默认关。
            let authenticated_length = yaml_text(item, "authenticated-length")
                .map(|value| {
                    let flag = value.trim().to_ascii_lowercase();
                    flag == "true" || flag == "1" || flag == "yes"
                })
                .unwrap_or(false);
            let tls = clash_tls(item, false, &server);
            let (ws, http, extra) = clash_transport(item, &server)?;
            let tls = h2_tls(tls, &extra, &server);
            Ok(Node {
                name,
                server,
                port,
                protocol: Protocol::Vmess {
                    uuid: uuid.trim().to_string(),
                    alter_id,
                    security,
                    authenticated_length,
                },
                tls,
                ws,
                http,
                extra,
                reality: None,
                mux: false,
            })
        }
        "anytls" => {
            let password =
                yaml_text(item, "password").ok_or_else(|| "anytls 节点缺 password".to_string())?;
            let tls = clash_tls(item, true, &server);
            Ok(Node {
                name,
                server,
                port,
                protocol: Protocol::AnyTls { password },
                tls,
                ws: None,
                http: None,
                extra: None,
                reality: None,
                mux: false,
            })
        }
        "hysteria2" | "hy2" => {
            let password = yaml_text(item, "password")
                .or_else(|| yaml_text(item, "auth"))
                .ok_or_else(|| "hysteria2 节点缺 password".to_string())?;
            let obfs = yaml_text(item, "obfs")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if !obfs.is_empty() && obfs != "salamander" && obfs != "gecko" {
                return Err(format!("未知 hysteria2 混淆：{obfs}（已跳过）"));
            }
            let obfs_password = yaml_text(item, "obfs-password").unwrap_or_default();
            let insecure = yaml_flag(item, "skip-cert-verify");
            Ok(Node {
                name,
                server: server.clone(),
                port,
                protocol: Protocol::Hysteria2 {
                    password,
                    obfs,
                    obfs_password,
                    insecure,
                },
                tls: TlsOptions {
                    enabled: true,
                    sni: yaml_text(item, "sni")
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty())
                        .unwrap_or(server),
                    allow_insecure: insecure,
                    alpn: Vec::new(),
                    ca: yaml_text(item, "ca-str").unwrap_or_default(),
                },
                ws: None,
                http: None,
                extra: None,
                reality: None,
                mux: false,
            })
        }
        "tuic" => {
            let uuid = yaml_text(item, "uuid")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "tuic 节点缺 uuid".to_string())?;
            // v5 是 uuid + password；空密码也允许（部分服务端就是这么配的）。
            let password = yaml_text(item, "password").unwrap_or_default();
            let udp_relay_mode = yaml_text(item, "udp-relay-mode")
                .or_else(|| yaml_text(item, "udp_relay_mode"))
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            Ok(Node {
                name,
                server: server.clone(),
                port,
                protocol: Protocol::Tuic {
                    uuid,
                    password,
                    udp_relay_mode,
                },
                // TUIC 是 QUIC ⇒ TLS 恒开；`alpn` / `skip-cert-verify` / `sni` 走同一条读取路径。
                tls: clash_tls(item, true, &server),
                ws: None,
                http: None,
                extra: None,
                reality: None,
                mux: false,
            })
        }
        other => Err(format!("Clash 节点类型 {other} 本版尚未重建（已跳过）")),
    }
}

/// 解析单条分享链接；失败返回一句人话原因（逐条展示用）。
pub fn parse_link(link: &str) -> Result<Node, String> {
    let raw = link.trim();
    let scheme = raw
        .split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
        .ok_or_else(|| format!("不是分享链接：{raw}"))?;
    match scheme.as_str() {
        "ss" => parse_ss(raw),
        "trojan" => parse_trojan(raw),
        "vless" => parse_vless(raw),
        "vmess" => parse_vmess(raw),
        "anytls" => parse_anytls(raw),
        "hysteria2" | "hy2" => parse_hysteria2(raw),
        "tuic" => parse_tuic(raw),
        other => Err(format!("不支持的协议：{other}（已跳过）")),
    }
}

fn parse_ss(link: &str) -> Result<Node, String> {
    let url = Url::parse(link).map_err(|e| format!("ss 链接解析失败：{e}"))?;
    let host = url_host(&url).ok_or_else(|| "ss 链接缺主机".to_string())?;
    let port = url.port().ok_or_else(|| "ss 链接缺端口".to_string())?;
    let mut userinfo = percent_decode(url.username());
    if url.password().is_none() {
        // 形态 `ss://base64(method:password)@host:port`
        if let Some(decoded) = b64_decode(&userinfo).and_then(|bytes| String::from_utf8(bytes).ok())
        {
            userinfo = decoded;
        }
    }
    let (method, password) = if let Some(pw) = url.password() {
        (userinfo.clone(), percent_decode(pw))
    } else {
        userinfo
            .split_once(':')
            .map(|(m, p)| (m.to_string(), p.to_string()))
            .ok_or_else(|| "ss 链接缺 method:password".to_string())?
    };
    let (plugin_ws, plugin_tls, plugin_obfs) = parse_ss_plugin(
        query_first(&url, "plugin").unwrap_or_default().as_str(),
        &host,
    )?;
    Ok(Node {
        name: frag_name(&url),
        server: host,
        port,
        protocol: Protocol::Ss {
            method,
            password,
            obfs: plugin_obfs,
        },
        tls: plugin_tls,
        ws: plugin_ws,
        http: None,
        extra: None,
        reality: None,
        mux: false,
    })
}

/// SIP003 插件（`ss://…?plugin=…` 或 Clash YAML 的 `plugin` + `plugin-opts`）。
///
/// 已重建：
///   · `v2ray-plugin` 的 **websocket** 模式 —— 它就是把整条 SS 流塞进 WS 二进制帧，
///     所以直接复用 `transport.rs` 的 RFC 6455 客户端；`tls` 是一个裸标志（无值）；
///   · `simple-obfs`（`obfs=http` / `obfs=tls`）—— 返回 [`SsObfs`]，由 `ss_plugin.rs`
///     在 SS 层下面套一层（假 TLS 不能与真 TLS 叠加，所以这里的 `TlsOptions` 恒为关闭）。
/// 未重建：其它插件 —— 如实返回原因，不静默降级。
fn parse_ss_plugin(
    spec: &str,
    server: &str,
) -> Result<(Option<WsOptions>, TlsOptions, Option<SsObfs>), String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Ok((None, TlsOptions::default(), None));
    }
    let mut parts = spec.split(';');
    let name = parts.next().unwrap_or("").trim();
    let mut options: Vec<(String, String)> = Vec::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('=') {
            Some((key, value)) => {
                options.push((key.trim().to_ascii_lowercase(), value.trim().to_string()))
            }
            None => options.push((part.to_ascii_lowercase(), String::new())),
        }
    }
    let get = |key: &str| {
        options
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    let has = |key: &str| options.iter().any(|(k, _)| k == key);
    match name {
        "v2ray-plugin" | "xray-plugin" => {
            let mode = get("mode").unwrap_or_else(|| "websocket".to_string());
            if mode.to_ascii_lowercase() != "websocket" {
                return Err(format!(
                    "SS 插件 {name} 的 mode={mode} 本版尚未重建（只做了 websocket，已跳过）"
                ));
            }
            let path = get("path").unwrap_or_else(|| "/".to_string());
            let ws_host = get("host").unwrap_or_else(|| server.to_string());
            let tls_on = has("tls");
            let tls = TlsOptions {
                enabled: tls_on,
                sni: if tls_on {
                    ws_host.clone()
                } else {
                    String::new()
                },
                allow_insecure: false,
                alpn: Vec::new(),
                ca: String::new(),
            };
            Ok((
                Some(WsOptions {
                    path: if path.trim().is_empty() {
                        "/".to_string()
                    } else {
                        path
                    },
                    host: ws_host,
                }),
                tls,
                None,
            ))
        }
        "obfs-local" | "simple-obfs" => {
            // 链接里是 `obfs=http` / `obfs-host=…`；Clash YAML 折过来的是 `mode=` / `host=`。
            let mode = get("obfs")
                .or_else(|| get("mode"))
                .unwrap_or_else(|| "http".to_string())
                .trim()
                .to_ascii_lowercase();
            if mode != "http" && mode != "tls" {
                return Err(format!(
                    "SS 插件 simple-obfs 的 obfs={mode} 不支持（只认 http / tls；已跳过）"
                ));
            }
            let host = get("obfs-host")
                .or_else(|| get("host"))
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| server.to_string());
            // 假 TLS 不能与真 TLS 叠加 ⇒ 这里不启用 `TlsOptions`。
            Ok((None, TlsOptions::default(), Some(SsObfs { mode, host })))
        }
        other => Err(format!("SS 插件 {other} 本版尚未重建（已跳过）")),
    }
}

fn parse_trojan(link: &str) -> Result<Node, String> {
    let url = Url::parse(link).map_err(|e| format!("trojan 链接解析失败：{e}"))?;
    let host = url_host(&url).ok_or_else(|| "trojan 链接缺主机".to_string())?;
    let port = url.port().unwrap_or(443);
    let password = percent_decode(url.username());
    if password.is_empty() {
        return Err("trojan 链接缺密码".to_string());
    }
    let mut tls = tls_from_query(&url, &host);
    tls.enabled = true; // trojan 默认走 TLS
    let (ws, http, extra) = transport_from_query(&url);
    let mut tls = h2_tls(tls, &extra, &host);
    let reality = reality_from_query(&url, &host)?;
    if reality.is_some() {
        if ws.is_some() || http.is_some() || extra.is_some() {
            return Err("REALITY 只支持裸 TCP（不能与 ws / http / h2 叠用）".to_string());
        }
        tls.enabled = true;
    }
    Ok(Node {
        name: frag_name(&url),
        server: host,
        port,
        protocol: Protocol::Trojan { password },
        tls,
        ws,
        http,
        extra,
        reality,
        // v2rayN 的分享链接用 `mux=1` 表示「走 mux.cool」（不是 Clash 的 smux）。
        mux: truthy(query_first(&url, "mux")),
    })
}

fn parse_vless(link: &str) -> Result<Node, String> {
    let url = Url::parse(link).map_err(|e| format!("vless 链接解析失败：{e}"))?;
    let host = url_host(&url).ok_or_else(|| "vless 链接缺主机".to_string())?;
    let port = url.port().ok_or_else(|| "vless 链接缺端口".to_string())?;
    let uuid = percent_decode(url.username());
    if uuid.is_empty() {
        return Err("vless 链接缺 uuid".to_string());
    }
    let flow = query_first(&url, "flow").unwrap_or_default();
    let tls = tls_from_query(&url, &host);
    let (ws, http, extra) = transport_from_query(&url);
    let mut tls = h2_tls(tls, &extra, &host);
    let reality = reality_from_query(&url, &host)?;
    if reality.is_some() {
        if ws.is_some() || http.is_some() || extra.is_some() {
            return Err("REALITY 只支持裸 TCP（不能与 ws / http / h2 叠用）".to_string());
        }
        tls.enabled = true;
    }
    Ok(Node {
        name: frag_name(&url),
        server: host,
        port,
        protocol: Protocol::Vless { uuid, flow },
        tls,
        ws,
        http,
        extra,
        reality,
        // v2rayN 的分享链接用 `mux=1` 表示「走 mux.cool」（不是 Clash 的 smux）。
        mux: truthy(query_first(&url, "mux")),
    })
}

fn parse_vmess(link: &str) -> Result<Node, String> {
    let payload = link
        .strip_prefix("vmess://")
        .ok_or_else(|| "不是 vmess 链接".to_string())?;
    let decoded = b64_decode(payload).ok_or_else(|| "vmess 链接不是合法 base64".to_string())?;
    let json: serde_json::Value =
        serde_json::from_slice(&decoded).map_err(|e| format!("vmess JSON 解析失败：{e}"))?;
    let text = |key: &str| {
        json.get(key)
            .and_then(|value| match value {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .unwrap_or_default()
    };
    let server = text("add");
    let port: u16 = text("port")
        .parse()
        .map_err(|_| "vmess 缺端口".to_string())?;
    let uuid = text("id");
    if server.is_empty() || uuid.is_empty() {
        return Err("vmess 缺 host / id".to_string());
    }
    let alter_id: u32 = text("aid").parse().unwrap_or(0);
    let security = {
        let scy = text("scy");
        if scy.is_empty() {
            "auto".to_string()
        } else {
            scy
        }
    };
    let net = text("net").to_ascii_lowercase();
    let tls_on = text("tls").eq_ignore_ascii_case("tls");
    let host = text("host");
    let path = text("path");
    let sni = text("sni");
    let (ws, http, extra) = if net == "ws" {
        (
            Some(WsOptions {
                path: if path.is_empty() {
                    "/".to_string()
                } else {
                    path
                },
                host: if host.is_empty() {
                    server.clone()
                } else {
                    host
                },
            }),
            None,
            None,
        )
    } else if net == "http" {
        (
            None,
            Some(HttpOptions {
                method: String::new(),
                path: if path.is_empty() {
                    "/".to_string()
                } else {
                    path
                },
                host: if host.is_empty() {
                    server.clone()
                } else {
                    host
                },
            }),
            None,
        )
    } else if net == "grpc" {
        // vmess JSON 的 `path` 在 grpc 下就是服务名（Xray 同口径）。
        (
            None,
            None,
            Some(ExtraTransport::Grpc {
                service_name: if path.trim().is_empty() {
                    "GunService".to_string()
                } else {
                    path
                },
            }),
        )
    } else if net == "h2" {
        (
            None,
            None,
            Some(ExtraTransport::H2 {
                host: if host.is_empty() {
                    server.clone()
                } else {
                    host
                },
                path: if path.is_empty() {
                    "/".to_string()
                } else {
                    path
                },
            }),
        )
    } else if net.is_empty() || net == "tcp" {
        (None, None, None)
    } else {
        return Err(format!("vmess 传输 {net} 本版尚未重建（已跳过）"));
    };
    let tls = h2_tls(
        TlsOptions {
            enabled: tls_on,
            sni: if sni.is_empty() { String::new() } else { sni },
            // v2rayN / v2rayNG 导出的 vmess JSON 用 `allowInsecure`（布尔 / "1" / "true"）；
            // Clash 迁移过来的是 `skip-cert-verify`。自签证书的节点全靠这一位。
            allow_insecure: json_flag(&json, "allowInsecure")
                || json_flag(&json, "skip-cert-verify"),
            ..TlsOptions::default()
        },
        &extra,
        &server,
    );
    Ok(Node {
        name: {
            let ps = text("ps");
            if ps.trim().is_empty() {
                format!("vmess {server}:{port}")
            } else {
                ps
            }
        },
        server,
        port,
        protocol: Protocol::Vmess {
            uuid,
            alter_id,
            security,
            // vmess:// 的 JSON 里没有 `Opt A` 的通用字段，分享链接一律按关处理。
            authenticated_length: false,
        },
        tls,
        ws,
        http,
        extra,
        reality: None,
        // v2rayN 导出的 vmess JSON 用 `mux: "1"` 表示「走 mux.cool」。
        mux: json_flag(&json, "mux"),
    })
}

fn parse_anytls(link: &str) -> Result<Node, String> {
    let url = Url::parse(link).map_err(|e| format!("anytls 链接解析失败：{e}"))?;
    let host = url_host(&url).ok_or_else(|| "anytls 链接缺主机".to_string())?;
    let port = url.port().unwrap_or(443);
    let password = percent_decode(url.username());
    if password.is_empty() {
        return Err("anytls 链接缺密码".to_string());
    }
    let mut tls = tls_from_query(&url, &host);
    tls.enabled = true;
    Ok(Node {
        name: frag_name(&url),
        server: host,
        port,
        protocol: Protocol::AnyTls { password },
        tls,
        ws: None,
        http: None,
        extra: None,
        reality: None,
        mux: false,
    })
}

fn parse_hysteria2(link: &str) -> Result<Node, String> {
    let url = Url::parse(link).map_err(|e| format!("hysteria2 链接解析失败：{e}"))?;
    let host = url_host(&url).ok_or_else(|| "hysteria2 链接缺主机".to_string())?;
    let port = url.port().unwrap_or(443);
    let password = if !url.username().is_empty() {
        percent_decode(url.username())
    } else {
        query_first(&url, "auth").unwrap_or_default()
    };
    if password.is_empty() {
        return Err("hysteria2 链接缺密码".to_string());
    }
    let obfs = query_first(&url, "obfs")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !obfs.is_empty() && obfs != "salamander" && obfs != "gecko" {
        return Err(format!("未知 hysteria2 混淆：{obfs}（已跳过）"));
    }
    let obfs_password = query_first(&url, "obfs-password").unwrap_or_default();
    let insecure = truthy(query_first(&url, "insecure"));
    let sni = query_first(&url, "sni").unwrap_or_else(|| host.clone());
    Ok(Node {
        name: frag_name(&url),
        server: host,
        port,
        protocol: Protocol::Hysteria2 {
            password,
            obfs,
            obfs_password,
            insecure,
        },
        tls: TlsOptions {
            enabled: true,
            sni,
            allow_insecure: insecure,
            alpn: Vec::new(),
            ca: ca_from_query(&url),
        },
        ws: None,
        http: None,
        extra: None,
        reality: None,
        mux: false,
    })
}

/// TUIC v5 分享链接：`tuic://uuid:password@host:port?sni=…&alpn=h3&allow_insecure=1#name`。
///
/// v4 的「只有 token」形态（`tuic://token@host`）**不支持**：v5 的认证令牌由 uuid + password
/// 经 TLS exporter 现算，服务端不认静态 token。
fn parse_tuic(link: &str) -> Result<Node, String> {
    let url = Url::parse(link).map_err(|e| format!("tuic 链接解析失败：{e}"))?;
    let host = url_host(&url).ok_or_else(|| "tuic 链接缺主机".to_string())?;
    let port = url.port().unwrap_or(443);
    let uuid = percent_decode(url.username());
    if uuid.trim().is_empty() {
        return Err("tuic 链接缺 uuid".to_string());
    }
    let password = match url.password() {
        Some(value) => percent_decode(value),
        None => return Err("tuic 链接缺 uuid:password（v4 的纯 token 形态不支持）".to_string()),
    };
    let udp_relay_mode = query_first(&url, "udp_relay_mode")
        .or_else(|| query_first(&url, "udp-relay-mode"))
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let insecure = truthy(query_first(&url, "allow_insecure"))
        || truthy(query_first(&url, "insecure"))
        || truthy(query_first(&url, "allowInsecure"));
    let alpn = query_first(&url, "alpn")
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let sni = query_first(&url, "sni").unwrap_or_else(|| host.clone());
    Ok(Node {
        name: frag_name(&url),
        server: host,
        port,
        protocol: Protocol::Tuic {
            uuid: uuid.trim().to_string(),
            password,
            udp_relay_mode,
        },
        tls: TlsOptions {
            enabled: true,
            sni,
            allow_insecure: insecure,
            alpn,
            ca: ca_from_query(&url),
        },
        ws: None,
        http: None,
        extra: None,
        reality: None,
        mux: false,
    })
}

fn tls_from_query(url: &Url, host: &str) -> TlsOptions {
    let security = query_first(url, "security")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let enabled = security == "tls" || security == "reality";
    let sni = query_first(url, "sni")
        .or_else(|| query_first(url, "peer"))
        .unwrap_or_else(|| host.to_string());
    let allow_insecure =
        truthy(query_first(url, "allowInsecure")) || truthy(query_first(url, "insecure"));
    let alpn = query_first(url, "alpn")
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    TlsOptions {
        enabled,
        sni,
        allow_insecure,
        alpn,
        ca: ca_from_query(url),
    }
}

/// 分享链接里的自定义根证书（`ca=` / `ca-str=`）。
///
/// 链接里的换行必须转义成 `%0A`，很多客户端干脆整段 base64 再塞进 `ca=`，
/// 所以这两种写法都接受；不是 PEM 的输入会原样返回，由 TLS 层如实报「PEM 格式不对」。
fn ca_from_query(url: &Url) -> String {
    let raw = query_first(url, "ca")
        .or_else(|| query_first(url, "ca-str"))
        .unwrap_or_default();
    let raw = raw.trim().to_string();
    if raw.is_empty() || raw.contains("BEGIN CERTIFICATE") {
        return raw;
    }
    b64_decode(&raw)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|text| text.contains("BEGIN CERTIFICATE"))
        .unwrap_or(raw)
}

fn transport_from_query(
    url: &Url,
) -> (
    Option<WsOptions>,
    Option<HttpOptions>,
    Option<ExtraTransport>,
) {
    let kind = query_first(url, "type")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let path = query_first(url, "path").unwrap_or_else(|| "/".to_string());
    let path = if path.is_empty() {
        "/".to_string()
    } else {
        path
    };
    let host = query_first(url, "host").unwrap_or_default();
    match kind.as_str() {
        "ws" => (Some(WsOptions { path, host }), None, None),
        "http" => (
            None,
            Some(HttpOptions {
                method: query_first(url, "method").unwrap_or_default(),
                path,
                host,
            }),
            None,
        ),
        // `type=grpc&serviceName=gun`：gRPC 的服务名走单独的查询参数。
        "grpc" => (
            None,
            None,
            Some(ExtraTransport::Grpc {
                service_name: query_first(url, "serviceName")
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| "GunService".to_string()),
            }),
        ),
        "h2" => (None, None, Some(ExtraTransport::H2 { host, path })),
        // `type=xhttp&path=/x&host=h&mode=stream-up`（Xray SplitHTTP）。
        "xhttp" => (
            None,
            None,
            Some(ExtraTransport::XHttp {
                host,
                path,
                mode: query_first(url, "mode")
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase(),
            }),
        ),
        _ => (None, None, None),
    }
}

/// h2 / grpc 的 TLS 前提：ALPN 必须是 `h2`，且这条传输**永远**在 TLS 之上（与 Xray 同口径）。
fn h2_tls(mut tls: TlsOptions, extra: &Option<ExtraTransport>, server: &str) -> TlsOptions {
    if extra.is_some() {
        tls.enabled = true;
        if tls.sni.trim().is_empty() {
            tls.sni = server.to_string();
        }
        if !tls.alpn.iter().any(|item| item == "h2") {
            tls.alpn = vec!["h2".to_string()];
        }
    }
    tls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ss_link_reads_method_and_password() {
        // userinfo = base64("aes-256-gcm:pw")
        let userinfo = base64::engine::general_purpose::STANDARD.encode("aes-256-gcm:pw");
        let node = parse_link(&format!(
            "ss://{userinfo}@example.com:8388#%E8%8A%82%E7%82%B91"
        ))
        .unwrap();
        assert_eq!(node.server, "example.com");
        assert_eq!(node.port, 8388);
        assert_eq!(node.name, "节点1");
        match node.protocol {
            Protocol::Ss {
                method,
                password,
                obfs,
            } => {
                assert_eq!(method, "aes-256-gcm");
                assert_eq!(password, "pw");
                assert!(obfs.is_none(), "裸 ss 链接不该带插件");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn trojan_link_reads_tls_sni_and_ws() {
        let node = parse_link(
            "trojan://p%40ss@example.com:443?sni=edge.example.com&type=ws&path=%2Fws&allowInsecure=1#T",
        )
        .unwrap();
        assert_eq!(node.name, "T");
        assert_eq!(node.port, 443);
        assert!(node.tls.enabled);
        assert!(node.tls.allow_insecure);
        assert_eq!(node.tls.sni, "edge.example.com");
        assert_eq!(node.ws.as_ref().unwrap().path, "/ws");
        match node.protocol {
            Protocol::Trojan { password } => assert_eq!(password, "p@ss"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn vless_link_reads_flow_and_ws() {
        let node = parse_link(
            "vless://11111111-2222-3333-4444-555555555555@example.com:8443?encryption=none&security=tls&sni=a.example.com&type=ws&path=%2Fv&host=a.example.com&flow=xtls-rprx-vision#V",
        )
        .unwrap();
        assert!(node.tls.enabled);
        assert_eq!(node.tls.sni, "a.example.com");
        assert_eq!(node.ws.as_ref().unwrap().host, "a.example.com");
        match node.protocol {
            Protocol::Vless { uuid, flow } => {
                assert_eq!(uuid, "11111111-2222-3333-4444-555555555555");
                assert_eq!(flow, "xtls-rprx-vision");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn share_links_read_the_mux_flag_and_default_to_off() {
        // v2rayN 的 `mux=1`（VLESS / Trojan 的 query、VMess 的 JSON 字段）。
        let vless = parse_link(
            "vless://11111111-2222-3333-4444-555555555555@example.com:443?encryption=none&mux=1#V",
        )
        .unwrap();
        assert!(vless.mux);
        let trojan = parse_link("trojan://p@example.com:443?mux=1#T").unwrap();
        assert!(trojan.mux);

        // 没有这一位 / 明确写 0 ⇒ 关（默认行为不变）。
        assert!(!parse_link("trojan://p@example.com:443#T").unwrap().mux);
        assert!(!parse_link(
            "vless://11111111-2222-3333-4444-555555555555@example.com:443?encryption=none&mux=0#V"
        )
        .unwrap()
        .mux);

        let json = serde_json::json!({
            "v": "2", "ps": "vm", "add": "example.com", "port": "443",
            "id": "11111111-2222-3333-4444-555555555555", "aid": "0",
            "net": "tcp", "mux": "1"
        });
        let payload = base64::engine::general_purpose::STANDARD.encode(json.to_string());
        assert!(parse_link(&format!("vmess://{payload}")).unwrap().mux);
    }

    #[test]
    fn vless_reality_link_and_yaml_are_parsed_and_ws_is_rejected() {
        // pbk 是 32 字节 X25519 公钥的 base64url（无填充）；sid 是十六进制短 ID
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0x11u8; 32]);
        let link = format!(
            "vless://11111111-2222-3333-4444-555555555555@example.com:443?encryption=none&security=reality&sni=a.example.com&pbk={key}&sid=0123abcd&flow=xtls-rprx-vision#R"
        );
        let node = parse_link(&link).unwrap();
        assert!(node.tls.enabled, "REALITY 必须走 TLS 层（我们自写的那个）");
        assert!(node.ws.is_none() && node.http.is_none() && node.extra.is_none());
        let reality = node.reality.as_ref().expect("必须解析出 reality 参数");
        assert_eq!(reality.public_key, vec![0x11u8; 32]);
        assert_eq!(reality.short_id, vec![0x01, 0x23, 0xab, 0xcd]);

        // 与 ws 叠用 ⇒ 解析阶段就如实拒绝（不静默降级成裸 TLS）
        let bad = format!(
            "vless://11111111-2222-3333-4444-555555555555@example.com:443?security=reality&pbk={key}&sid=01&type=ws&path=%2Fv"
        );
        let message = parse_link(&bad).unwrap_err();
        assert!(message.contains("REALITY"), "{message}");

        // 缺 pbk / pbk 长度不对 ⇒ 逐条给原因
        assert!(parse_link(
            "vless://11111111-2222-3333-4444-555555555555@example.com:443?security=reality&sid=01"
        )
        .unwrap_err()
        .contains("pbk"));
        let short_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0x11u8; 16]);
        assert!(parse_link(&format!(
            "vless://11111111-2222-3333-4444-555555555555@example.com:443?security=reality&pbk={short_key}&sid=01"
        ))
        .unwrap_err()
        .contains("32 字节"));

        // YAML：`reality-opts` + `skip-cert-verify`
        let yaml = format!(
            "proxies:\n  - {{name: R, type: vless, server: example.com, port: 443, uuid: 11111111-2222-3333-4444-555555555555, tls: true, servername: a.example.com, reality-opts: {{public-key: {key}, short-id: 'ab01'}}}}\n"
        );
        let parsed = parse_subscription(&yaml);
        assert_eq!(parsed.nodes.len(), 1, "跳过原因：{:?}", parsed.skipped);
        let reality = parsed.nodes[0]
            .reality
            .as_ref()
            .expect("YAML 也要解析 reality");
        assert_eq!(reality.short_id, vec![0xab, 0x01]);
        assert!(parsed.nodes[0].tls.enabled);
    }

    #[test]
    fn vmess_link_reads_json_payload_and_keeps_altered_id() {
        let json = serde_json::json!({
            "v": "2", "ps": "vm", "add": "example.com", "port": "443",
            "id": "11111111-2222-3333-4444-555555555555", "aid": "0",
            "net": "ws", "type": "none", "host": "h.example.com", "path": "/w",
            "tls": "tls", "sni": "s.example.com"
        });
        let payload = base64::engine::general_purpose::STANDARD.encode(json.to_string());
        let node = parse_link(&format!("vmess://{payload}")).unwrap();
        assert_eq!(node.name, "vm");
        assert!(node.tls.enabled);
        assert_eq!(node.tls.sni, "s.example.com");
        assert_eq!(node.ws.as_ref().unwrap().path, "/w");

        // `aid≠0` 走老式 MD5 头（2026-11-15 重建），解析阶段不再拒绝。
        let altered = serde_json::json!({
            "add": "example.com", "port": "443",
            "id": "11111111-2222-3333-4444-555555555555", "aid": "2"
        });
        let payload = base64::engine::general_purpose::STANDARD.encode(altered.to_string());
        let node = parse_link(&format!("vmess://{payload}")).unwrap();
        match node.protocol {
            Protocol::Vmess { alter_id, .. } => assert_eq!(alter_id, 2),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn hysteria2_reads_obfs_and_insecure() {
        let node = parse_link(
            "hysteria2://pw@example.com:8443?sni=h.example.com&insecure=1&obfs=gecko&obfs-password=ob#H",
        )
        .unwrap();
        assert!(node.tls.allow_insecure);
        assert_eq!(node.tls.sni, "h.example.com");
        match node.protocol {
            Protocol::Hysteria2 {
                password,
                obfs,
                obfs_password,
                ..
            } => {
                assert_eq!(password, "pw");
                assert_eq!(obfs, "gecko");
                assert_eq!(obfs_password, "ob");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(parse_link("hysteria2://pw@example.com:8443?obfs=nope").is_err());
    }

    #[test]
    fn unsupported_schemes_and_plugins_are_skipped_with_reason() {
        assert!(parse_link("tuic://pw@example.com:443").is_err());
        assert!(parse_link("ss://YWVzLTI1Ni1nY206cHc@example.com:8388?plugin=kcptun").is_err());
    }

    #[test]
    fn subscription_accepts_base64_and_skips_bad_lines() {
        let lines = "ss://YWVzLTI1Ni1nY206cHc@a.com:1#A\ntuic://pw@b.com:2\n";
        let text = base64::engine::general_purpose::STANDARD.encode(lines);
        let parsed = parse_subscription(&text);
        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.skipped.len(), 1);
        assert!(parsed.skipped[0].contains("tuic"));
    }

    #[test]
    fn ss_v2ray_plugin_websocket_maps_to_ws_and_tls() {
        let userinfo = base64::engine::general_purpose::STANDARD.encode("aes-256-gcm:pw");
        // plugin = "v2ray-plugin;mode=websocket;host=cdn.example.com;path=/ws;tls"
        let link = format!(
            "ss://{userinfo}@example.com:8388?plugin=v2ray-plugin%3Bmode%3Dwebsocket%3Bhost%3Dcdn.example.com%3Bpath%3D%2Fws%3Btls"
        );
        let node = parse_link(&link).unwrap();
        let ws = node.ws.expect("v2ray-plugin ws 模式必须映射成 WS 传输");
        assert_eq!(ws.path, "/ws");
        assert_eq!(ws.host, "cdn.example.com");
        assert!(node.tls.enabled, "插件带 tls 标志 ⇒ 必须走 TLS");
        assert_eq!(node.tls.sni, "cdn.example.com");
        // 不带 tls ⇒ 明文 WS。
        let plain = format!(
            "ss://{userinfo}@example.com:8388?plugin=v2ray-plugin%3Bmode%3Dwebsocket%3Bpath%3D%2Fv2"
        );
        let node = parse_link(&plain).unwrap();
        assert!(!node.tls.enabled);
        assert_eq!(node.ws.expect("默认 mode 即 websocket").path, "/v2");
    }

    #[test]
    fn ss_simple_obfs_maps_to_obfs_options() {
        let userinfo = base64::engine::general_purpose::STANDARD.encode("aes-256-gcm:pw");
        let node = parse_link(&format!(
            "ss://{userinfo}@example.com:8388?plugin=obfs-local%3Bobfs%3Dhttp%3Bobfs-host%3Dbing.com"
        ))
        .expect("simple-obfs 现在应当能解析");
        match node.protocol {
            Protocol::Ss { obfs, .. } => {
                let obfs = obfs.expect("必须带上 obfs 参数");
                assert_eq!(obfs.mode, "http");
                assert_eq!(obfs.host, "bing.com");
            }
            other => panic!("unexpected {other:?}"),
        }
        // 假 TLS 不能与真 TLS 叠加。
        assert!(!node.tls.enabled);
        assert!(node.ws.is_none());

        // `obfs=tls` 一样能解析；缺 obfs-host 时退回节点域名。
        let node = parse_link(&format!(
            "ss://{userinfo}@example.com:8388?plugin=obfs-local%3Bobfs%3Dtls"
        ))
        .unwrap();
        match node.protocol {
            Protocol::Ss { obfs, .. } => {
                let obfs = obfs.expect("必须带上 obfs 参数");
                assert_eq!(obfs.mode, "tls");
                assert_eq!(obfs.host, "example.com");
            }
            other => panic!("unexpected {other:?}"),
        }

        // 不认识的 obfs 模式必须明确拒绝。
        let error = parse_link(&format!(
            "ss://{userinfo}@example.com:8388?plugin=obfs-local%3Bobfs%3Dquic"
        ))
        .err()
        .expect("未知 obfs 必须拒绝");
        assert!(error.contains("obfs=quic"), "{error}");

        // v2ray-plugin 的非 ws 模式也要明确拒绝，而不是静默当成 ws。
        let error = parse_link(&format!(
            "ss://{userinfo}@example.com:8388?plugin=v2ray-plugin%3Bmode%3Dquic"
        ))
        .err()
        .expect("mode=quic 必须明确拒绝");
        assert!(error.contains("mode=quic"), "{error}");
    }

    /// 一份接近真实机场的 Clash 配置（六种协议 + ws 的两种写法都在里面）。
    const CLASH_SAMPLE: &str = r#"
port: 7890
proxies:
  - name: "ss1"
    type: ss
    server: 1.2.3.4
    port: 8388
    cipher: aes-256-gcm
    password: pw
    udp: true
  - name: "tj"
    type: trojan
    server: t.example.com
    port: 443
    password: tpw
    sni: t.example.com
    skip-cert-verify: true
    network: ws
    ws-opts:
      path: /trojan
      headers:
        Host: cdn.example.com
  - name: vless1
    type: vless
    server: v.example.com
    port: 443
    uuid: 11111111-2222-3333-4444-555555555555
    tls: true
    servername: v.example.com
    network: ws
    ws-opts:
      path: /vless
      headers:
        Host: cdn.example.com
  - name: vmess1
    type: vmess
    server: m.example.com
    port: 443
    uuid: 11111111-2222-3333-4444-555555555555
    alterId: 0
    tls: true
    servername: m.example.com
    network: ws
    ws-opts:
      path: /vmess
      headers:
        Host: cdn.example.com
  - name: at
    type: anytls
    server: a.example.com
    port: 8443
    password: apw
    sni: a.example.com
    skip-cert-verify: true
  - name: hy
    type: hysteria2
    server: h.example.com
    port: 8443
    password: hpw
    obfs: salamander
    obfs-password: obspw
    sni: h.example.com
    skip-cert-verify: true
proxy-groups:
  - name: PROXY
    type: select
    proxies: [ss1, tj]
rules:
  - MATCH,PROXY
"#;

    #[test]
    fn clash_yaml_parses_the_common_proxy_types() {
        let parsed = parse_subscription(CLASH_SAMPLE);
        assert!(parsed.note.contains("Clash YAML"), "{}", parsed.note);
        assert!(parsed.skipped.is_empty(), "{:?}", parsed.skipped);
        assert_eq!(parsed.nodes.len(), 6);

        let ss = &parsed.nodes[0];
        assert_eq!(ss.name, "ss1");
        assert_eq!(ss.server, "1.2.3.4");
        assert_eq!(ss.port, 8388);
        assert_eq!(
            ss.protocol,
            Protocol::Ss {
                method: "aes-256-gcm".to_string(),
                password: "pw".to_string(),
                obfs: None,
            }
        );
        assert!(!ss.tls.enabled);
        assert!(ss.ws.is_none());

        let trojan = &parsed.nodes[1];
        assert_eq!(
            trojan.protocol,
            Protocol::Trojan {
                password: "tpw".to_string()
            }
        );
        assert!(trojan.tls.enabled, "trojan 默认走 TLS");
        assert_eq!(trojan.tls.sni, "t.example.com");
        assert!(
            trojan.tls.allow_insecure,
            "skip-cert-verify 要落到 allow_insecure"
        );
        let ws = trojan.ws.as_ref().expect("network: ws ⇒ 必须有 WS 选项");
        assert_eq!(ws.path, "/trojan");
        assert_eq!(ws.host, "cdn.example.com");

        let vless = &parsed.nodes[2];
        assert_eq!(
            vless.protocol,
            Protocol::Vless {
                uuid: "11111111-2222-3333-4444-555555555555".to_string(),
                flow: String::new()
            }
        );
        assert!(vless.tls.enabled);
        assert_eq!(vless.tls.sni, "v.example.com");
        assert_eq!(vless.ws.as_ref().unwrap().path, "/vless");

        let vmess = &parsed.nodes[3];
        assert_eq!(
            vmess.protocol,
            Protocol::Vmess {
                uuid: "11111111-2222-3333-4444-555555555555".to_string(),
                alter_id: 0,
                security: "auto".to_string(),
                authenticated_length: false,
            }
        );
        assert!(vmess.tls.enabled);
        assert_eq!(vmess.ws.as_ref().unwrap().host, "cdn.example.com");

        let anytls = &parsed.nodes[4];
        assert_eq!(
            anytls.protocol,
            Protocol::AnyTls {
                password: "apw".to_string()
            }
        );
        assert!(anytls.tls.enabled);
        assert!(anytls.tls.allow_insecure);

        let hy = &parsed.nodes[5];
        assert_eq!(
            hy.protocol,
            Protocol::Hysteria2 {
                password: "hpw".to_string(),
                obfs: "salamander".to_string(),
                obfs_password: "obspw".to_string(),
                insecure: true
            }
        );
        assert_eq!(hy.tls.sni, "h.example.com");
    }

    #[test]
    fn clash_yaml_accepts_flow_style_and_legacy_ws_keys() {
        let text = r#"
proxies:
  - {name: "flow-ss", type: ss, server: f.example.com, "port": "8388", cipher: chacha20-ietf-poly1305, password: fpw}
  - name: legacy-ws
    type: trojan
    server: l.example.com
    port: 443
    password: lpw
    network: ws
    ws-path: /legacy
    ws-headers:
      Host: legacy.example.com
"#;
        let parsed = parse_subscription(text);
        assert_eq!(parsed.nodes.len(), 2, "{:?}", parsed.skipped);
        assert_eq!(parsed.nodes[0].port, 8388, "端口写成字符串也要认");
        assert_eq!(
            parsed.nodes[0].protocol,
            Protocol::Ss {
                method: "chacha20-ietf-poly1305".to_string(),
                password: "fpw".to_string(),
                obfs: None,
            }
        );
        let ws = parsed.nodes[1]
            .ws
            .as_ref()
            .expect("旧式 ws-path / ws-headers");
        assert_eq!(ws.path, "/legacy");
        assert_eq!(ws.host, "legacy.example.com");
    }

    #[test]
    fn clash_yaml_reads_http_network() {
        let text = r#"
proxies:
  - name: h
    type: trojan
    server: h.example.com
    port: 8443
    password: hpw
    sni: h.example.com
    skip-cert-verify: true
    network: http
    http-opts:
      method: POST
      path:
        - /tunnel
      headers:
        Host:
          - cdn.example.com
"#;
        let parsed = parse_subscription(text);
        assert!(parsed.skipped.is_empty(), "{:?}", parsed.skipped);
        let node = &parsed.nodes[0];
        assert!(node.ws.is_none(), "network: http 不该产生 ws");
        let http = node
            .http
            .as_ref()
            .expect("network: http ⇒ 必须有 http 选项");
        assert_eq!(http.method, "POST");
        assert_eq!(http.path, "/tunnel", "path 是数组时取第一项");
        assert_eq!(http.host, "cdn.example.com", "Host 是数组时取第一项");
    }

    /// VMess `authenticated-length`（`Opt A`）：默认关，写 `true` 才开。
    #[test]
    fn clash_yaml_reads_vmess_authenticated_length() {
        let text = r#"
proxies:
  - name: a
    type: vmess
    server: a.example.com
    port: 443
    uuid: 11111111-2222-3333-4444-555555555555
    cipher: auto
    authenticated-length: true
  - name: b
    type: vmess
    server: b.example.com
    port: 443
    uuid: 11111111-2222-3333-4444-555555555555
"#;
        let parsed = parse_subscription(text);
        assert!(parsed.skipped.is_empty(), "{:?}", parsed.skipped);
        match &parsed.nodes[0].protocol {
            Protocol::Vmess {
                authenticated_length,
                ..
            } => assert!(*authenticated_length, "写了 true 就要开"),
            other => panic!("意外的协议 {other:?}"),
        }
        match &parsed.nodes[1].protocol {
            Protocol::Vmess {
                authenticated_length,
                ..
            } => assert!(!*authenticated_length, "没写就是关"),
            other => panic!("意外的协议 {other:?}"),
        }
    }

    #[test]
    fn vless_link_reads_http_transport() {
        let node = parse_link(
            "vless://11111111-2222-3333-4444-555555555555@example.com:8443?type=http&path=%2Fh&host=a.example.com#H",
        )
        .unwrap();
        assert!(node.ws.is_none());
        let http = node.http.as_ref().expect("type=http ⇒ http 选项");
        assert_eq!(http.path, "/h");
        assert_eq!(http.host, "a.example.com");
        assert_eq!(http.method, "", "链接没给 method ⇒ 交给传输层默认 GET");
    }

    #[test]
    fn clash_top_level_rules_are_returned_in_source_order() {
        let text = r#"
proxies:
  - name: a
    type: ss
    server: a.com
    port: 8388
    cipher: aes-128-gcm
    password: pw
rules:
  - DOMAIN-SUFFIX,example.com,DIRECT
  - GEOIP,CN,DIRECT
  - RULE-SET,cn
  - "MATCH,PROXY"
  - 123
"#;
        let parsed = parse_subscription(text);
        assert_eq!(parsed.nodes.len(), 1);
        assert!(parsed.skipped.is_empty(), "{:?}", parsed.skipped);
        assert_eq!(
            parsed.rules,
            vec![
                "DOMAIN-SUFFIX,example.com,DIRECT",
                "GEOIP,CN,DIRECT",
                "RULE-SET,cn",
                "MATCH,PROXY",
            ],
            "规则按原文顺序、非字符串项忽略"
        );
    }

    #[test]
    fn tuic_v5_credentials_and_relay_mode_are_read_from_links_and_yaml() {
        let node = parse_link(
            "tuic://11111111-2222-3333-4444-555555555555:pw@t.example.com:443?sni=cdn.example.com&alpn=h3&udp_relay_mode=native&allow_insecure=1#T",
        )
        .expect("tuic v5 链接现在应当能解析");
        assert_eq!(node.server, "t.example.com");
        assert_eq!(node.port, 443);
        assert!(node.tls.enabled);
        assert_eq!(node.tls.sni, "cdn.example.com");
        assert_eq!(node.tls.alpn, vec!["h3".to_string()]);
        assert!(node.tls.allow_insecure);
        match node.protocol {
            Protocol::Tuic {
                uuid,
                password,
                udp_relay_mode,
            } => {
                assert_eq!(uuid, "11111111-2222-3333-4444-555555555555");
                assert_eq!(password, "pw");
                assert_eq!(udp_relay_mode, "native");
            }
            other => panic!("unexpected {other:?}"),
        }
        // v4 的纯 token 形态要给出明确原因（不是「不支持 tuic」）。
        let reason = parse_link("tuic://just-a-token@t.example.com:443").unwrap_err();
        assert!(reason.contains("uuid:password"), "{reason}");

        let yaml = r#"
proxies:
  - name: t1
    type: tuic
    server: t.example.com
    port: 443
    uuid: 11111111-2222-3333-4444-555555555555
    password: pw
    skip-cert-verify: true
    alpn: [h3]
    udp-relay-mode: native
"#;
        let parsed = parse_subscription(yaml);
        assert!(parsed.skipped.is_empty(), "{:?}", parsed.skipped);
        assert_eq!(parsed.nodes.len(), 1);
        assert!(parsed.nodes[0].tls.allow_insecure);
        assert_eq!(parsed.nodes[0].tls.alpn, vec!["h3".to_string()]);
    }

    #[test]
    fn clash_yaml_skips_unsupported_entries_with_a_reason() {
        let text = r#"
proxies:
  - name: tuic1
    type: tuic
    server: t.example.com
    port: 443
    uuid: 11111111-2222-3333-4444-555555555555
    password: x
  - name: reality1
    type: vless
    server: r.example.com
    port: 443
    uuid: 11111111-2222-3333-4444-555555555555
    tls: true
    reality-opts:
      public-key: abc
      short-id: def
  - name: grpc1
    type: trojan
    server: g.example.com
    port: 443
    password: g
    network: grpc
    grpc-opts:
      grpc-service-name: gun
  - name: obfs1
    type: ss
    server: o.example.com
    port: 8388
    cipher: aes-128-gcm
    password: o
    plugin: obfs
    plugin-opts:
      mode: http
      host: bing.com
  - name: port-range
    type: hysteria2
    server: h.example.com
    port: 443,20000-30000
    password: h
  - name: v6
    type: ss
    server: "2001:db8::1"
    port: 8388
    cipher: aes-128-gcm
    password: v6
"#;
        let parsed = parse_subscription(text);
        assert_eq!(parsed.nodes.len(), 4, "tuic / obfs / IPv6 / grpc 四条可用");
        // obfs 节点现在是可用节点（不再是「已跳过」）。
        let obfs = parsed
            .nodes
            .iter()
            .find(|node| node.name == "obfs1")
            .expect("simple-obfs 节点必须保留");
        match &obfs.protocol {
            Protocol::Ss { obfs, .. } => {
                let obfs = obfs.as_ref().expect("必须带 obfs 参数");
                assert_eq!(obfs.mode, "http");
                assert_eq!(obfs.host, "bing.com");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(parsed.nodes.iter().any(|node| node.server == "2001:db8::1"));
        // grpc 节点（2026-11-15 起可用）：服务名来自 `grpc-opts.grpc-service-name`，
        // 且 TLS 被强制打开并把 ALPN 设成 h2。
        let grpc = parsed
            .nodes
            .iter()
            .find(|node| node.name == "grpc1")
            .expect("grpc 节点必须保留");
        assert_eq!(
            grpc.extra,
            Some(ExtraTransport::Grpc {
                service_name: "gun".to_string()
            })
        );
        assert!(grpc.tls.enabled);
        assert_eq!(grpc.tls.alpn, vec!["h2".to_string()]);
        assert_eq!(parsed.skipped.len(), 2, "{:?}", parsed.skipped);
        assert!(parsed.skipped.iter().any(|line| line.contains("REALITY")));
        assert!(parsed.skipped.iter().any(|line| line.contains("端口")));
    }

    #[test]
    fn h2_and_grpc_are_parsed_from_links_and_yaml() {
        // 分享链接：`type=grpc&serviceName=…`。
        let node = parse_link(
            "vless://11111111-2222-3333-4444-555555555555@g.example.com:443?security=tls&type=grpc&serviceName=gun&sni=edge.example.com#G",
        )
        .expect("vless grpc 链接");
        assert_eq!(
            node.extra,
            Some(ExtraTransport::Grpc {
                service_name: "gun".to_string()
            })
        );
        assert!(node.tls.enabled);
        assert_eq!(node.tls.alpn, vec!["h2".to_string()]);
        assert_eq!(node.tls.sni, "edge.example.com");

        // 分享链接：`type=h2&host=…&path=…`。
        let node = parse_link(
            "trojan://pw@h.example.com:443?type=h2&host=cdn.example.com&path=%2Ftunnel#H",
        )
        .expect("trojan h2 链接");
        assert_eq!(
            node.extra,
            Some(ExtraTransport::H2 {
                host: "cdn.example.com".to_string(),
                path: "/tunnel".to_string()
            })
        );
        assert!(node.tls.enabled);
        assert_eq!(node.tls.alpn, vec!["h2".to_string()]);

        // Clash YAML：`network: h2` + `h2-opts`。
        let parsed = parse_subscription(
            r#"
proxies:
  - name: h2y
    type: trojan
    server: h2.example.com
    port: 443
    password: pw
    network: h2
    h2-opts:
      host: cdn.example.com
      path: /h2path
"#,
        );
        assert!(parsed.skipped.is_empty(), "{:?}", parsed.skipped);
        let node = &parsed.nodes[0];
        assert_eq!(
            node.extra,
            Some(ExtraTransport::H2 {
                host: "cdn.example.com".to_string(),
                path: "/h2path".to_string()
            })
        );
        assert!(node.tls.enabled, "h2 必须强制在 TLS 之上");
    }

    #[test]
    fn xhttp_is_parsed_from_links_and_yaml() {
        // 分享链接：`type=xhttp&path=…&host=…&mode=stream-up`。
        let node = parse_link(
            "vless://11111111-2222-3333-4444-555555555555@x.example.com:443?security=tls&type=xhttp&path=%2Fsplit&host=cdn.example.com&mode=stream-up#X",
        )
        .expect("vless xhttp 链接");
        assert_eq!(
            node.extra,
            Some(ExtraTransport::XHttp {
                host: "cdn.example.com".to_string(),
                path: "/split".to_string(),
                mode: "stream-up".to_string(),
            })
        );
        assert!(node.tls.enabled);
        assert_eq!(node.tls.alpn, vec!["h2".to_string()]);

        // Clash YAML：`network: xhttp` + `xhttp-opts`（缺省 mode 为空 = stream-one）。
        let parsed = parse_subscription(
            r#"
proxies:
  - name: xy
    type: trojan
    server: x.example.com
    port: 443
    password: pw
    network: xhttp
    xhttp-opts:
      host: cdn.example.com
      path: /xh
"#,
        );
        assert!(parsed.skipped.is_empty(), "{:?}", parsed.skipped);
        let node = &parsed.nodes[0];
        assert_eq!(
            node.extra,
            Some(ExtraTransport::XHttp {
                host: "cdn.example.com".to_string(),
                path: "/xh".to_string(),
                mode: String::new(),
            })
        );
        assert!(node.tls.enabled, "xhttp 必须强制在 TLS 之上");
        assert_eq!(node.tls.alpn, vec!["h2".to_string()]);
    }

    #[test]
    fn clash_yaml_supports_v2ray_plugin_and_base64_wrapping() {
        let text = r#"
proxies:
  - name: ws-plugin
    type: ss
    server: p.example.com
    port: 8388
    cipher: aes-256-gcm
    password: ppw
    plugin: v2ray-plugin
    plugin-opts:
      mode: websocket
      host: cdn.example.com
      path: /ws
      tls: true
"#;
        let parsed = parse_subscription(text);
        assert_eq!(parsed.nodes.len(), 1, "{:?}", parsed.skipped);
        let node = &parsed.nodes[0];
        assert!(node.tls.enabled, "plugin-opts.tls ⇒ 走 TLS");
        assert_eq!(node.tls.sni, "cdn.example.com");
        let ws = node.ws.as_ref().expect("v2ray-plugin websocket ⇒ WS 传输");
        assert_eq!(ws.path, "/ws");
        assert_eq!(ws.host, "cdn.example.com");

        // 少数机场把 YAML 也 base64 一层。
        let wrapped = base64::engine::general_purpose::STANDARD.encode(text);
        let parsed = parse_subscription(&wrapped);
        assert_eq!(parsed.nodes.len(), 1, "{:?}", parsed.skipped);
        assert!(parsed.note.contains("base64"));
        assert!(parsed.note.contains("Clash YAML"));
    }

    #[test]
    fn non_clash_text_still_reports_what_is_wrong() {
        let parsed = parse_subscription("hello world");
        assert!(parsed.nodes.is_empty());
        assert_eq!(parsed.skipped.len(), 1);
        assert!(parsed.skipped[0].contains("proxies:"));
    }

    /// 分享链接里的 IPv6 写成 `[2001:db8::1]`，`Node.server` 必须去掉方括号
    /// （否则 `ToSocketAddrs` / SNI 都解析不了）。
    #[test]
    fn ipv6_servers_lose_the_url_brackets() {
        let node = parse_link("trojan://pw@[2001:db8::1]:443#v6").unwrap();
        assert_eq!(node.server, "2001:db8::1");
        assert_eq!(node.port, 443);
        let vless =
            parse_link("vless://b831381d-6324-4d53-ad4f-8cda48b30811@[::1]:8443#v6").unwrap();
        assert_eq!(vless.server, "::1");
    }

    #[test]
    fn a_node_can_carry_a_custom_root_certificate() {
        // Clash 的 `ca-str`（内联 PEM，`|` 块标量）→ `TlsOptions.ca`。
        let yaml = "proxies:\n  - name: t\n    type: trojan\n    server: example.com\n    port: 443\n    password: pw\n    ca-str: |\n      -----BEGIN CERTIFICATE-----\n      MIIB\n      -----END CERTIFICATE-----\n";
        let parsed = parse_subscription(yaml);
        assert!(parsed.skipped.is_empty(), "{:?}", parsed.skipped);
        assert_eq!(parsed.nodes.len(), 1);
        assert!(parsed.nodes[0].tls.ca.contains("BEGIN CERTIFICATE"));
    }

    #[test]
    fn a_share_link_can_carry_a_custom_root_certificate() {
        let pem = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----";
        let escaped: String = url::form_urlencoded::byte_serialize(pem.as_bytes()).collect();
        let node = parse_link(&format!("trojan://pw@example.com:443?ca={escaped}#T")).unwrap();
        assert!(node.tls.ca.contains("BEGIN CERTIFICATE"));

        // 整段 base64 的 PEM 也接受（很多客户端为避免换行就这么导出）。
        let packed = base64::engine::general_purpose::STANDARD.encode(pem);
        let packed: String = url::form_urlencoded::byte_serialize(packed.as_bytes()).collect();
        let node = parse_link(&format!("trojan://pw@example.com:443?ca={packed}#T")).unwrap();
        assert!(node.tls.ca.contains("BEGIN CERTIFICATE"));
    }
}
