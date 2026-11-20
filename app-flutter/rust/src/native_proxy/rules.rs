//! 路由规则（重建 v1 `native_proxy/rules.rs` 的能力子集）。
//!
//! v1 的账本（docs/acceptance-map.md 第 13 行）只承诺三类规则 + 本机 / 私网直连：
//! `DOMAIN` / `DOMAIN-SUFFIX` / `DOMAIN-KEYWORD` / `IP-CIDR` / `IP-CIDR6` / `MATCH`。
//! 2026-11-15 追加 `PROCESS-NAME`（按**发起连接的本机进程**分流，见 `process.rs`）；
//! 2026-11-16 追加 `GEOIP`（需要本地 MaxMind DB，见 `geoip.rs`；缺库时规则不命中）；
//! 2026-11-18 `RULE-SET` 由 `ruleset.rs` 就地展开成上面这些普通规则（本文件仍只管单行规则）。
//!
//! 判定顺序（与 Clash 相同，也是 v1 的口径）：
//!   1. 目标是本机 / 私网 / `localhost` ⇒ **直连**（否则 Lunac 自己的本地服务会被绕进节点）；
//!   2. 规则表**第一条命中**的生效；
//!   3. 都不命中 ⇒ 直连（保守；v1 的「没有 MATCH 就走直连」）。
//!
//! `IP-CIDR` / `GEOIP` 只对**字面量 IP** 目标生效；目标是域名时不做 DNS 预解析 —— 这条是
//! v1 的明确取舍（预解析会把「域名走代理」变成「IP 走代理」，还会额外泄漏 DNS）。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use super::geoip::GeoIp;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleKind {
    Domain,
    DomainSuffix,
    DomainKeyword,
    IpCidr4 {
        network: Ipv4Addr,
        prefix: u8,
    },
    IpCidr6 {
        network: Ipv6Addr,
        prefix: u8,
    },
    /// `GEOIP,CN`：按**目标 IP** 的归属国家分流（值统一小写，比较时忽略大小写）。
    GeoIp,
    /// `PROCESS-NAME,chrome.exe`：按入站连接所属进程分流（值统一小写，比较时忽略大小写）。
    ProcessName,
    Match,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub kind: RuleKind,
    /// 规则值（域名 / 关键字）；IP 规则与 MATCH 为空串。**统一小写**。
    pub value: String,
    /// 命中后是否走节点：`true` = 走节点，`false` = 直连。
    /// 来自规则第三段（`DOMAIN,x,DIRECT`）与插件的黑名单层（名单内直连）。
    pub proxy: bool,
}

impl Rule {
    /// 这条规则「写出来长什么样」（日志区展示命中的是哪一条）。
    pub fn label(&self) -> String {
        let policy = if self.proxy { "PROXY" } else { "DIRECT" };
        match &self.kind {
            RuleKind::Domain => format!("DOMAIN,{},{}", self.value, policy),
            RuleKind::DomainSuffix => format!("DOMAIN-SUFFIX,{},{}", self.value, policy),
            RuleKind::DomainKeyword => format!("DOMAIN-KEYWORD,{},{}", self.value, policy),
            RuleKind::IpCidr4 { network, prefix } => {
                format!("IP-CIDR,{network}/{prefix},{policy}")
            }
            RuleKind::IpCidr6 { network, prefix } => {
                format!("IP-CIDR6,{network}/{prefix},{policy}")
            }
            RuleKind::GeoIp => format!("GEOIP,{},{policy}", self.value.to_ascii_uppercase()),
            RuleKind::ProcessName => format!("PROCESS-NAME,{},{}", self.value, policy),
            RuleKind::Match => format!("MATCH,{policy}"),
        }
    }
}

/// 解析一行规则；无法识别 ⇒ `None`（调用方跳过并记一行日志，v1 同口径）。
pub fn parse_rule(line: &str) -> Option<Rule> {
    let text = line.trim();
    if text.is_empty() || text.starts_with('#') || text.starts_with("//") {
        return None;
    }
    // `DOMAIN,example.com` / `DOMAIN-SUFFIX,example.com,DIRECT`（第三段策略位只用于
    // 「是否走节点」，这里忽略具体策略名，命中即「走节点」）。
    let mut parts = text.split(',');
    let head = parts.next()?.trim().to_ascii_uppercase();
    // `MATCH,DIRECT` 的策略位在**第二段**（没有规则值）—— 与其它规则不同，先分叉。
    if head == "MATCH" || head == "FINAL" {
        let policy = parts.next().unwrap_or("").trim().to_ascii_uppercase();
        return Some(Rule {
            kind: RuleKind::Match,
            value: String::new(),
            proxy: policy != "DIRECT",
        });
    }
    let value = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    // 第三段是策略位（Clash 写法）：`DIRECT` ⇒ 命中直连，其余（PROXY / 省略）⇒ 走节点。
    let policy = parts.next().unwrap_or("").trim().to_ascii_uppercase();
    let proxy = policy != "DIRECT";
    let kind = match head.as_str() {
        "DOMAIN" => RuleKind::Domain,
        "DOMAIN-SUFFIX" => RuleKind::DomainSuffix,
        "DOMAIN-KEYWORD" => RuleKind::DomainKeyword,
        "PROCESS-NAME" => RuleKind::ProcessName,
        "GEOIP" => {
            // 国家码必须是两位字母（`GEOIP,CN`）；其余写法当未知规则跳过，不猜。
            if value.len() != 2 || !value.chars().all(|c| c.is_ascii_alphabetic()) {
                return None;
            }
            RuleKind::GeoIp
        }
        "IP-CIDR" | "IP-CIDR6" => {
            let (network, prefix) = parse_cidr(&value)?;
            match network {
                IpAddr::V4(network) => RuleKind::IpCidr4 { network, prefix },
                IpAddr::V6(network) => RuleKind::IpCidr6 { network, prefix },
            }
        }
        _ => return None,
    };
    if value.is_empty() {
        return None;
    }
    Some(Rule { kind, value, proxy })
}

/// 把插件的「规则配置文件」展开成规则层：**黑名单 ⇒ 名单内直连；白名单 ⇒ 名单内走节点**。
///
/// 这一层必须排在订阅规则之前（`decide` 先匹配先赢），否则订阅里的宽规则会先吞掉。
/// 值按 `DOMAIN-SUFFIX` 展开（`cn` 覆盖 `baidu.com` 这类子域），`#` / `//` 行忽略，
/// 允许 `*.example.com` 写法（去掉 `*.` 后等价）。
pub fn domain_rules(rule_mode: &str, domains: &[String]) -> Vec<Rule> {
    let whitelist = rule_mode.eq_ignore_ascii_case("whitelist");
    let mut out = Vec::new();
    for raw in domains {
        let text = raw.trim();
        if text.is_empty() || text.starts_with('#') || text.starts_with("//") {
            continue;
        }
        let value = text
            .trim_start_matches("*.")
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if value.is_empty() || value.contains('/') || value.contains(' ') {
            continue;
        }
        out.push(Rule {
            kind: RuleKind::DomainSuffix,
            value,
            proxy: whitelist,
        });
    }
    out
}

/// 解析 `1.2.3.0/24` / `fd00::/8`；前缀越界或地址非法 ⇒ None。
fn parse_cidr(text: &str) -> Option<(IpAddr, u8)> {
    let (addr, prefix) = text.split_once('/')?;
    let ip: IpAddr = addr.trim().parse().ok()?;
    let prefix: u8 = prefix.trim().parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    if prefix > max {
        return None;
    }
    Some((ip, prefix))
}

/// 本机 / 私网 / 回环：一律直连（v1 `lan_rules()` 的语义）。
pub fn is_lan(host: &str) -> bool {
    let text = host.trim().trim_matches(['[', ']']).to_ascii_lowercase();
    if text.is_empty() {
        return true;
    }
    if text == "localhost" || text.ends_with(".localhost") {
        return true;
    }
    match text.parse::<IpAddr>() {
        Ok(ip) => ip_is_private(ip),
        Err(_) => false,
    }
}

fn ip_is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || (octets[0] == 100 && (64..=127).contains(&octets[1])) // CGNAT 100.64/10
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10
        }
    }
}

/// 是否走节点（`true` = 代理，`false` = 直连）。
///
/// `process` = 发起这条连接的本机进程名（`chrome.exe`；查不到就 `None`）。
/// `PROCESS-NAME` 规则在 `None` 时一律**不命中** —— 查不到不等于「不是它」。
#[cfg(test)]
pub fn decide(
    rules: &[Rule],
    host: &str,
    literal_ip: Option<IpAddr>,
    process: Option<&str>,
) -> bool {
    decide_with_geoip(rules, host, literal_ip, process, None)
}

/// 带 GeoIP 库的 [`decide`]（引擎里用这一条；库缺失时 `GEOIP` 规则不命中）。
#[cfg(test)]
pub fn decide_with_geoip(
    rules: &[Rule],
    host: &str,
    literal_ip: Option<IpAddr>,
    process: Option<&str>,
    geoip: Option<&GeoIp>,
) -> bool {
    decide_rule_with_geoip(rules, host, literal_ip, process, geoip).map_or(false, |rule| rule.proxy)
}

/// 命中的**第一条**规则（`None` = 没有命中，或目标是本机 / 私网）。
///
/// 供日志区显示「命中了哪条规则」；判定顺序与 [`decide`] 完全同一份实现。
#[cfg(test)]
pub fn decide_rule<'a>(
    rules: &'a [Rule],
    host: &str,
    literal_ip: Option<IpAddr>,
    process: Option<&str>,
) -> Option<&'a Rule> {
    decide_rule_with_geoip(rules, host, literal_ip, process, None)
}

/// 带 GeoIP 库的 [`decide_rule`]。
pub fn decide_rule_with_geoip<'a>(
    rules: &'a [Rule],
    host: &str,
    literal_ip: Option<IpAddr>,
    process: Option<&str>,
    geoip: Option<&GeoIp>,
) -> Option<&'a Rule> {
    if is_lan(host) {
        return None;
    }
    // IP 规则只认「目标本身就是 IP」。调用方即使传了 IP，只要 host 是域名也一律忽略 ——
    // 否则等于偷偷做了 DNS 预解析（违背本模块头部的纪律）。
    let literal_ip = if host
        .trim()
        .trim_matches(['[', ']'])
        .parse::<IpAddr>()
        .is_ok()
    {
        literal_ip
    } else {
        None
    };
    let domain = host.trim().trim_end_matches('.').to_ascii_lowercase();
    for rule in rules {
        let hit = match &rule.kind {
            RuleKind::Domain => !domain.is_empty() && domain == rule.value,
            // `DOMAIN-SUFFIX,example.com` 只覆盖 `example.com` 与其子域；
            // 必须带点边界，否则 `notexample.com` 也会被吞进去（Clash 同口径）。
            RuleKind::DomainSuffix => {
                !domain.is_empty()
                    && (domain == rule.value
                        || (domain.ends_with(&rule.value)
                            && domain.as_bytes()[domain.len() - rule.value.len() - 1] == b'.'))
            }
            RuleKind::DomainKeyword => !rule.value.is_empty() && domain.contains(&rule.value),
            RuleKind::IpCidr4 { network, prefix } => match literal_ip {
                Some(IpAddr::V4(ip)) => cidr4_match(*network, *prefix, ip),
                _ => false,
            },
            RuleKind::IpCidr6 { network, prefix } => match literal_ip {
                Some(IpAddr::V6(ip)) => cidr6_match(*network, *prefix, ip),
                _ => false,
            },
            // 没有库 / 目标不是 IP ⇒ 不命中（不猜「可能不是该国」）。
            RuleKind::GeoIp => match (literal_ip, geoip) {
                (Some(ip), Some(db)) => db
                    .country_code(ip)
                    .map(|code| code.eq_ignore_ascii_case(&rule.value))
                    .unwrap_or(false),
                _ => false,
            },
            RuleKind::ProcessName => process
                .map(|name| name.eq_ignore_ascii_case(&rule.value))
                .unwrap_or(false),
            RuleKind::Match => true,
        };
        if hit {
            return Some(rule);
        }
    }
    None
}

fn cidr4_match(network: Ipv4Addr, prefix: u8, ip: Ipv4Addr) -> bool {
    if prefix == 0 {
        return true;
    }
    let mask = u32::MAX << (32 - prefix);
    (u32::from(network) & mask) == (u32::from(ip) & mask)
}

fn cidr6_match(network: Ipv6Addr, prefix: u8, ip: Ipv6Addr) -> bool {
    if prefix == 0 {
        return true;
    }
    let mask = u128::MAX << (128 - prefix);
    (u128::from(network) & mask) == (u128::from(ip) & mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_rules_match_exact_suffix_and_keyword() {
        let rules: Vec<Rule> = [
            "DOMAIN,exact.test",
            "DOMAIN-SUFFIX,example.com",
            "DOMAIN-KEYWORD,google",
        ]
        .iter()
        .filter_map(|line| parse_rule(line))
        .collect();
        assert!(decide(&rules, "exact.test", None, None));
        assert!(decide(&rules, "a.b.example.com", None, None));
        assert!(decide(&rules, "www.google.co.jp", None, None));
        assert!(!decide(&rules, "notexample.com", None, None));
        assert!(!decide(&rules, "other.test", None, None));
    }

    #[test]
    fn ip_rules_only_apply_to_literal_ips() {
        // 用公网段做断言：10/8、fd00::/8 属于私网，按第 1 条纪律永远是直连。
        let rules: Vec<Rule> = ["IP-CIDR,1.2.3.0/24", "IP-CIDR6,2001:db8::/32"]
            .iter()
            .filter_map(|line| parse_rule(line))
            .collect();
        assert!(decide(
            &rules,
            "1.2.3.4",
            Some("1.2.3.4".parse().unwrap()),
            None
        ));
        assert!(!decide(
            &rules,
            "a.test",
            Some("1.2.3.4".parse().unwrap()),
            None
        ));
        assert!(decide(
            &rules,
            "2001:db8::1",
            Some("2001:db8::1".parse().unwrap()),
            None
        ));
        // 家族隔离：v4 规则不命中 v6 目标。
        assert!(!decide(
            &rules,
            "2001:db9::1",
            Some("2001:db9::1".parse().unwrap()),
            None
        ));
        // 私网即便命中规则也直连。
        let lan: Vec<Rule> = ["IP-CIDR,10.0.0.0/8"]
            .iter()
            .filter_map(|line| parse_rule(line))
            .collect();
        assert!(!decide(
            &lan,
            "10.1.2.3",
            Some("10.1.2.3".parse().unwrap()),
            None
        ));
    }

    #[test]
    fn lan_and_loopback_always_direct() {
        assert!(!decide(&[], "127.0.0.1", None, None));
        assert!(!decide(&[], "localhost", None, None));
        assert!(!decide(&[], "192.168.1.5", None, None));
        assert!(!decide(&[], "[::1]", None, None));
        assert!(!decide(&[], "100.64.0.1", None, None));
    }

    #[test]
    fn match_is_the_fallback() {
        let rules: Vec<Rule> = ["DOMAIN-SUFFIX,corp", "MATCH,PROXY"]
            .iter()
            .filter_map(|line| parse_rule(line))
            .collect();
        assert!(decide(&rules, "a.corp", None, None));
        assert!(decide(&rules, "anything.test", None, None));
        assert!(!decide(&rules, "127.0.0.1", None, None));
    }

    #[test]
    fn process_name_rules_match_the_owning_process_only_when_known() {
        let rules: Vec<Rule> = ["PROCESS-NAME,chrome.exe"]
            .iter()
            .filter_map(|line| parse_rule(line))
            .collect();
        assert!(decide(&rules, "a.test", None, Some("Chrome.exe")));
        assert!(!decide(&rules, "a.test", None, Some("firefox.exe")));
        // 查不到进程 ⇒ 不命中（否则等于把所有未知应用都塞进节点）。
        assert!(!decide(&rules, "a.test", None, None));
    }

    #[test]
    fn unknown_rules_are_skipped() {
        // `GEOIP` 现在能解析（2026-11-16）；坏的国家码仍按未知规则跳过。
        assert!(parse_rule("GEOIP,CN").is_some());
        assert!(parse_rule("GEOIP,CHN").is_none());
        assert!(parse_rule("GEOIP").is_none());
        assert!(parse_rule("RULE-SET,foo").is_none());
        assert!(parse_rule("IP-CIDR,10.0.0.0/99").is_none());
        assert!(parse_rule("# comment").is_none());
    }

    #[test]
    fn geoip_rules_need_a_database_and_a_literal_ip() {
        let rules: Vec<Rule> = ["GEOIP,CN"]
            .iter()
            .filter_map(|line| parse_rule(line))
            .collect();
        assert_eq!(rules[0].label(), "GEOIP,CN,PROXY");
        // 没有库 ⇒ 不命中（不是「猜它不是 CN」）。
        assert!(!decide(
            &rules,
            "1.2.3.4",
            Some("1.2.3.4".parse().unwrap()),
            None
        ));
        // 域名不命中（不做 DNS 预解析）。
        assert!(!decide(
            &rules,
            "example.com",
            Some("1.2.3.4".parse().unwrap()),
            None
        ));
    }

    #[test]
    fn cidr_edges() {
        assert!(cidr4_match(
            "0.0.0.0".parse().unwrap(),
            0,
            "8.8.8.8".parse().unwrap()
        ));
        assert!(cidr4_match(
            "10.0.0.0".parse().unwrap(),
            8,
            "10.255.0.1".parse().unwrap()
        ));
        assert!(!cidr4_match(
            "10.0.0.0".parse().unwrap(),
            8,
            "11.0.0.1".parse().unwrap()
        ));
        assert!(cidr6_match(
            "fd00::".parse().unwrap(),
            8,
            "fd12::1".parse().unwrap()
        ));
        assert!(!cidr6_match(
            "fd00::".parse().unwrap(),
            8,
            "fe00::1".parse().unwrap()
        ));
    }

    /// 第三段策略位：`DIRECT` 命中直连；省略 / `PROXY` 命中走节点。
    #[test]
    fn policy_field_decides_which_way_a_hit_goes() {
        let rules: Vec<Rule> = [
            "DOMAIN-SUFFIX,corp.test,DIRECT",
            "DOMAIN-SUFFIX,go.test,PROXY",
            "DOMAIN,plain.test",
        ]
        .iter()
        .filter_map(|line| parse_rule(line))
        .collect();
        assert!(!decide(&rules, "a.corp.test", None, None));
        assert!(decide(&rules, "a.go.test", None, None));
        assert!(decide(&rules, "plain.test", None, None));
        // 命中哪一条也要能取出来（日志区显示规则用）。
        assert_eq!(
            decide_rule(&rules, "a.corp.test", None, None)
                .expect("必须命中")
                .value,
            "corp.test"
        );
        assert!(decide_rule(&rules, "127.0.0.1", None, None).is_none());
    }

    /// `MATCH` 的策略位在第二段（Clash 写法），不能当规则值读。
    #[test]
    fn match_policy_is_the_second_field() {
        let proxy = parse_rule("MATCH,PROXY").unwrap();
        assert_eq!(proxy.label(), "MATCH,PROXY");
        assert!(proxy.proxy);
        let direct = parse_rule("MATCH,DIRECT").unwrap();
        assert_eq!(direct.label(), "MATCH,DIRECT");
        assert!(!direct.proxy);
        // 省略策略位 ⇒ 与 v1 一致，命中即走节点。
        let bare = parse_rule("MATCH").unwrap();
        assert!(bare.proxy);
        assert_eq!(bare.value, "");
    }

    /// 插件「规则配置文件」：黑名单 ⇒ 名单内直连；白名单 ⇒ 名单内走节点。
    #[test]
    fn domain_rules_expand_blacklist_and_whitelist() {
        let domains = vec![
            "baidu.com".to_string(),
            "*.qq.com".to_string(),
            "  # 注释".to_string(),
            "  ".to_string(),
            "bad/value".to_string(),
        ];
        let black = domain_rules("blacklist", &domains);
        assert_eq!(black.len(), 2, "注释 / 空行 / 非法值必须跳过");
        assert!(black.iter().all(|rule| !rule.proxy));
        assert!(!decide(&black, "www.baidu.com", None, None));
        assert!(!decide(&black, "a.qq.com", None, None));
        // 黑名单只覆盖名单内；名单外照常走（交给后面的规则 / 默认直连）。
        assert!(decide_rule(&black, "example.org", None, None).is_none());

        let white = domain_rules("whitelist", &domains);
        assert!(white.iter().all(|rule| rule.proxy));
        assert!(decide(&white, "www.baidu.com", None, None));
        assert!(decide_rule(&white, "example.org", None, None).is_none());
    }

    /// 插件名单层必须排在订阅规则之前：宽规则不能把名单先吞掉。
    #[test]
    fn plugin_layer_wins_over_subscription_rules() {
        let mut rules = domain_rules("blacklist", &["intl.test".to_string()]);
        rules.extend(["MATCH,PROXY"].iter().filter_map(|line| parse_rule(line)));
        // 名单内：直连（尽管后面有 MATCH,PROXY）。
        assert!(!decide(&rules, "x.intl.test", None, None));
        // 名单外：落到 MATCH ⇒ 走节点。
        assert!(decide(&rules, "other.test", None, None));
    }
}
