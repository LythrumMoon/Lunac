//! `RULE-SET` 展开（docs/todo.md §3-29 G13 的最后一截）。
//!
//! 依据 mihomo 文档（2026-11-18 核对，不凭记忆）：
//!   · 规则集定义 `/en/config/rule-providers/`：`type` = `http` / `file` / `inline`，
//!     `behavior` = `domain` / `ipcidr` / `classical`，`format` = `yaml` / `text` / `mrs`；
//!   · 域名通配 `/en/handbook/syntax/`：`+.x` 匹配 `x` 本身与所有子域；`.x` / `*.x` 只匹配子域。
//!
//! 落地口径（每条都写进 `docs/acceptance-map.md`）：
//!   · `+.x` ⇒ `DOMAIN-SUFFIX,x`（与 mihomo 语义一致）；
//!   · `.x` / `*.x` ⇒ `DOMAIN-SUFFIX,x`（**近似**：引擎只有后缀匹配，会连带命中 `x` 本身与更深层级）；
//!   · 其余 ⇒ `DOMAIN,x`；`ipcidr` ⇒ `IP-CIDR` / `IP-CIDR6`；
//!   · `classical` ⇒ 取前两段并补上 `RULE-SET` 的策略位；
//!   · `format: mrs` 未支持 ⇒ 该条 `RULE-SET` 跳过并记原因（不猜二进制格式）。
//!
//! 缓存：`<缓存目录>/<md5(url)>.ruleset`。**有缓存就直接用**（过期了也只是起一个后台线程刷新，
//! 不挡引擎启动 —— 启动是 Dart 侧的同步 FFI，卡在这里就是卡面板）；只有完全没有缓存时才同步
//! 拉一次（5s 超时）。拉取失败且无缓存 ⇒ 跳过并记原因（`forbidden.md` §禁用 1：不假装生效）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::clash::RuleProvider;
use super::ss::md5;

/// 默认缓存目录：`<exe 根>/ModuleData/proxy/rulesets`（与其它模块的 `ModuleData` 同源）。
pub fn default_cache_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("ModuleData").join("proxy").join("rulesets");
        }
    }
    std::env::temp_dir().join("lunac-rulesets")
}

/// 把规则行里的 `RULE-SET` 就地展开；返回 `(展开后的规则行, 跳过原因)`。
///
/// 顺序保持不变（先匹配先赢）：`RULE-SET` 展开出的规则落在它原来的位置。
pub fn expand(
    rules: &[String],
    providers: &HashMap<String, RuleProvider>,
    cache_dir: &Path,
) -> (Vec<String>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    let mut loaded: HashMap<String, Result<Vec<String>, String>> = HashMap::new();
    for line in rules {
        let parts: Vec<&str> = line.split(',').map(str::trim).collect();
        if !parts
            .first()
            .map(|head| head.eq_ignore_ascii_case("RULE-SET"))
            .unwrap_or(false)
        {
            out.push(line.clone());
            continue;
        }
        let name = parts.get(1).copied().unwrap_or("");
        let policy = parts.get(2).copied().unwrap_or("PROXY");
        if name.is_empty() {
            notes.push(format!("RULE-SET 缺少规则集名：{line}（已跳过）"));
            continue;
        }
        let Some(provider) = providers.get(name) else {
            notes.push(format!(
                "RULE-SET,{name}：订阅里没有同名 rule-provider（已跳过）"
            ));
            continue;
        };
        if provider.behavior.is_empty() {
            notes.push(format!(
                "RULE-SET,{name}：rule-provider 没有 behavior（已跳过）"
            ));
            continue;
        }
        if !matches!(
            provider.behavior.as_str(),
            "domain" | "ipcidr" | "classical"
        ) {
            notes.push(format!(
                "RULE-SET,{name}：未知 behavior「{}」（已跳过）",
                provider.behavior
            ));
            continue;
        }
        let entries = loaded
            .entry(name.to_string())
            .or_insert_with(|| load_entries(provider, cache_dir));
        match entries {
            Ok(entries) => {
                let mut ignored = 0usize;
                for entry in entries {
                    match expand_entry(&provider.behavior, entry, policy) {
                        Some(rule) => out.push(rule),
                        None => ignored += 1,
                    }
                }
                if ignored > 0 {
                    notes.push(format!(
                        "RULE-SET,{name}：{ignored} 条规则集条目认不出（已跳过）"
                    ));
                }
            }
            Err(reason) => {
                notes.push(format!("RULE-SET,{name}：{reason}（已跳过）"));
            }
        }
    }
    (out, notes)
}

/// 一条规则集条目 → 一条引擎规则行；认不出 ⇒ `None`。
pub fn expand_entry(behavior: &str, entry: &str, policy: &str) -> Option<String> {
    let text = entry.trim().trim_matches('\'').trim_matches('"').trim();
    if text.is_empty() || text.starts_with('#') || text.starts_with("//") {
        return None;
    }
    match behavior {
        "domain" => {
            for prefix in ["+.", ".", "*."] {
                if let Some(rest) = text.strip_prefix(prefix) {
                    let rest = rest.trim().trim_end_matches('.');
                    if rest.is_empty() {
                        return None;
                    }
                    return Some(format!("DOMAIN-SUFFIX,{rest},{policy}"));
                }
            }
            Some(format!("DOMAIN,{text},{policy}"))
        }
        "ipcidr" => {
            if text.contains(':') {
                Some(format!("IP-CIDR6,{text},{policy}"))
            } else {
                Some(format!("IP-CIDR,{text},{policy}"))
            }
        }
        "classical" => {
            let mut parts = text.split(',').map(str::trim);
            let head = parts.next()?;
            let value = parts.next()?;
            if head.is_empty() || value.is_empty() {
                return None;
            }
            // classical 的条目只有「类型,值」，策略位由 RULE-SET 这一行决定。
            Some(format!("{head},{value},{policy}"))
        }
        _ => None,
    }
}

/// 按 provider 的 type / format 取出条目文本。
fn load_entries(provider: &RuleProvider, cache_dir: &Path) -> Result<Vec<String>, String> {
    if provider.format == "mrs" {
        return Err("MRS 规则集未支持（需要 mihomo convert-ruleset）".to_string());
    }
    let text = match provider.kind.as_str() {
        "inline" => {
            return Ok(parse_payload_lines(
                &provider.payload.join("\n"),
                &provider.format,
            ))
        }
        "file" => {
            if provider.path.is_empty() {
                return Err("type=file 的 rule-provider 缺少 path".to_string());
            }
            let path = resolve_path(&provider.path);
            std::fs::read_to_string(&path)
                .map_err(|e| format!("读取规则集 {} 失败：{e}", path.display()))?
        }
        "http" => {
            if provider.url.is_empty() {
                return Err("type=http 的 rule-provider 缺少 url".to_string());
            }
            let cache = cache_path(cache_dir, &provider.url);
            let interval = if provider.interval == 0 {
                86_400
            } else {
                provider.interval
            };
            if let Ok(text) = std::fs::read_to_string(&cache) {
                if !is_fresh(&cache, interval) {
                    // 过期不阻塞启动：先把旧内容用上，后台再刷新缓存（下次启动生效）。
                    refresh_in_background(provider.url.clone(), cache);
                }
                text
            } else {
                // 完全没有缓存：同步拉一次（超时 5s，别把启动卡太久）。
                let text = fetch(&provider.url, Duration::from_secs(5))?;
                if let Some(dir) = cache.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&cache, &text);
                text
            }
        }
        other => {
            return Err(format!(
                "未知的规则集 type「{other}」（只支持 http / file / inline）"
            ))
        }
    };
    Ok(parse_payload_lines(&text, &provider.format))
}

/// `format: text` ⇒ 一行一条；`format: yaml` ⇒ 优先取 `payload:` 列表，认不出再按文本行。
fn parse_payload_lines(text: &str, format: &str) -> Vec<String> {
    if format != "text" {
        if let Ok(documents) = yaml_rust2::YamlLoader::load_from_str(text) {
            if let Some(document) = documents.into_iter().next() {
                if let Some(items) = super::clash::yaml_payload(&document) {
                    return items;
                }
            }
        }
    }
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with("//"))
        .map(str::to_string)
        .collect()
}

/// 相对路径按 `<exe 根>` 解析（mihomo 用 `-d` 指定的 HomeDir，这里是同一层语义）。
fn resolve_path(raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        return path;
    }
    match std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        Some(root) => root.join(path),
        None => path,
    }
}

fn cache_path(cache_dir: &Path, url: &str) -> PathBuf {
    let digest = md5(url.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    cache_dir.join(format!("{digest}.ruleset"))
}

fn is_fresh(path: &Path, interval_secs: u64) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|age| age.as_secs() < interval_secs)
        .unwrap_or(false)
}

fn fetch(url: &str, timeout: Duration) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .user_agent(format!("lunac/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("HTTP 客户端创建失败：{e}"))?;
    let response = client
        .get(url)
        .send()
        .map_err(|e| format!("拉取规则集失败：{e}"))?;
    if !response.status().is_success() {
        return Err(format!("拉取规则集返回 HTTP {}", response.status()));
    }
    response
        .text()
        .map_err(|e| format!("读取规则集响应失败：{e}"))
}

/// 缓存过期时后台刷新（不挡引擎启动；失败只记日志，旧内容继续用）。
fn refresh_in_background(url: String, cache: PathBuf) {
    let _ = std::thread::Builder::new()
        .name("lunac-ruleset".to_string())
        .spawn(move || match fetch(&url, Duration::from_secs(15)) {
            Ok(text) => {
                if let Some(dir) = cache.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(error) = std::fs::write(&cache, text) {
                    eprintln!("规则集缓存写入失败：{error}");
                }
            }
            Err(reason) => eprintln!("规则集后台刷新失败：{reason}"),
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn provider(kind: &str, behavior: &str, format: &str) -> RuleProvider {
        RuleProvider {
            kind: kind.to_string(),
            behavior: behavior.to_string(),
            format: format.to_string(),
            ..RuleProvider::default()
        }
    }

    #[test]
    fn domain_entries_follow_the_documented_wildcards() {
        assert_eq!(
            expand_entry("domain", "+.google.com", "PROXY").as_deref(),
            Some("DOMAIN-SUFFIX,google.com,PROXY")
        );
        assert_eq!(
            expand_entry("domain", ".google.com", "DIRECT").as_deref(),
            Some("DOMAIN-SUFFIX,google.com,DIRECT")
        );
        assert_eq!(
            expand_entry("domain", "*.google.com", "DIRECT").as_deref(),
            Some("DOMAIN-SUFFIX,google.com,DIRECT")
        );
        assert_eq!(
            expand_entry("domain", "'exact.example'", "PROXY").as_deref(),
            Some("DOMAIN,exact.example,PROXY")
        );
        assert_eq!(expand_entry("domain", "+.", "PROXY"), None);
        assert_eq!(expand_entry("domain", "# 注释", "PROXY"), None);
    }

    #[test]
    fn ipcidr_and_classical_entries_keep_their_shape() {
        assert_eq!(
            expand_entry("ipcidr", "10.0.0.0/8", "DIRECT").as_deref(),
            Some("IP-CIDR,10.0.0.0/8,DIRECT")
        );
        assert_eq!(
            expand_entry("ipcidr", "fd00::/8", "DIRECT").as_deref(),
            Some("IP-CIDR6,fd00::/8,DIRECT")
        );
        assert_eq!(
            expand_entry("classical", "DOMAIN-SUFFIX,ads.com,REJECT", "PROXY").as_deref(),
            Some("DOMAIN-SUFFIX,ads.com,PROXY")
        );
        assert_eq!(expand_entry("classical", "只有一段", "PROXY"), None);
    }

    #[test]
    fn inline_provider_expands_in_place_and_unknown_names_are_reported() {
        let mut providers = HashMap::new();
        let mut inline = provider("inline", "domain", "yaml");
        inline.payload = vec!["+.ads.example".to_string(), "tracker.example".to_string()];
        providers.insert("ads".to_string(), inline);

        let rules = vec![
            "DOMAIN,before.example,DIRECT".to_string(),
            "RULE-SET,ads,REJECT".to_string(),
            "MATCH,PROXY".to_string(),
            "RULE-SET,missing,PROXY".to_string(),
        ];
        let (expanded, notes) = expand(&rules, &providers, &std::env::temp_dir());
        assert_eq!(
            expanded,
            vec![
                "DOMAIN,before.example,DIRECT".to_string(),
                "DOMAIN-SUFFIX,ads.example,REJECT".to_string(),
                "DOMAIN,tracker.example,REJECT".to_string(),
                "MATCH,PROXY".to_string(),
            ]
        );
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("missing"), "{}", notes[0]);
    }

    #[test]
    fn an_http_provider_is_fetched_once_then_served_from_cache() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/rules.yaml", listener.local_addr().unwrap());
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&hits);
        let body = "payload:\n  - '+.cdn.example'\n  - 'img.example'\n";
        let handle = thread::spawn(move || {
            // 最多等 5s（测试失败时别把线程永远挂在 accept 上）。
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            listener.set_nonblocking(true).unwrap();
            loop {
                if std::time::Instant::now() > deadline {
                    return;
                }
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let mut request = [0u8; 1024];
                        let _ = socket.read(&mut request);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/yaml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = socket.write_all(response.as_bytes());
                        return;
                    }
                    Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return,
                }
            }
        });

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("lunac-ruleset-test-{stamp}"));
        let mut providers = HashMap::new();
        let mut http = provider("http", "domain", "yaml");
        http.url = url;
        http.interval = 3600;
        providers.insert("http".to_string(), http);
        let rules = vec!["RULE-SET,http,DIRECT".to_string()];

        let (expanded, notes) = expand(&rules, &providers, &dir);
        assert_eq!(
            expanded,
            vec![
                "DOMAIN-SUFFIX,cdn.example,DIRECT".to_string(),
                "DOMAIN,img.example,DIRECT".to_string(),
            ],
            "{notes:?}"
        );
        // 第二次在 interval 内：只读缓存，不再发请求。
        let (again, _) = expand(&rules, &providers, &dir);
        assert_eq!(again, expanded);
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        let _ = std::fs::remove_file(cache_path(&dir, &providers["http"].url));
        let _ = std::fs::remove_dir(&dir);
        handle.join().unwrap();
    }

    #[test]
    fn mrs_and_unknown_behaviors_are_reported_not_guessed() {
        let mut providers = HashMap::new();
        let mut mrs = provider("inline", "domain", "mrs");
        mrs.payload = vec!["+.a.example".to_string()];
        providers.insert("mrs".to_string(), mrs);
        let mut weird = provider("inline", "geosite", "yaml");
        weird.payload = vec!["x".to_string()];
        providers.insert("weird".to_string(), weird);

        let rules = vec![
            "RULE-SET,mrs,DIRECT".to_string(),
            "RULE-SET,weird,DIRECT".to_string(),
        ];
        let (expanded, notes) = expand(&rules, &providers, &std::env::temp_dir());
        assert!(expanded.is_empty());
        assert_eq!(notes.len(), 2);
        assert!(notes[0].contains("MRS"), "{}", notes[0]);
        assert!(notes[1].contains("geosite"), "{}", notes[1]);
    }
}
