// core-agent/src/mcp_oauth.rs
// MCP 远端服务器的 OAuth 2.1 授权（2026-10-01）。
//
// 为什么需要它：MCP 规范把**远端服务器的授权**规定成 OAuth 2.1 —— 服务端对未授权的请求回
// 401 + `WWW-Authenticate: Bearer resource_metadata="…"`；客户端去读那份**受保护资源元数据**
// （RFC 9728），再读**授权服务器元数据**（RFC 8414），然后**动态注册**一个客户端（RFC 7591），
// 最后用 **Authorization Code + PKCE** 换令牌。没有这条路，用户只能自己抓一个长期令牌塞进
// `headers` —— 那既不是规范做法，也**拿不到 refresh_token**（过期就得再抓一次）。
//
// **形态：全部在 agent 侧做完，宿主与前端一行都不用改**（这是刻意的）：
//   起一个 127.0.0.1 的**环回端口**当 redirect_uri → `cmd /C start <url>` 拉起系统浏览器
//   → 阻塞等回调（上限 `INTERACTIVE_TIMEOUT`）→ 用 code 换令牌 → 落盘。
//   多一层 UI 只会多一个「授权到一半窗口被关了」的状态，而 OAuth 本来就是 agent 与远端
//   服务器之间的事。
//
// **凭据落盘**：`config\mcp-tokens.json`（与 `config\mcp.json` **同目录**，从 `LUNAC_MCP_FILE`
// 推父目录 ⇒ 不新增环境变量）。**单独一个文件**是为了绝不改写用户手写的 `mcp.json`。
// 安全等级与 `config\ai.json` 里的 API key 相同（都在用户自己的数据根下）。
//
// **只在「连上那一步」做交互式授权**：`tools/call` 阶段拿到 401 只**刷新**、不开浏览器
// （一次工具调用弹一个浏览器窗口是最糟糕的体验）。刷新也失败就如实报错，让用户重启 agent
// 重新走一次授权。
//
// 边界（有意不做，不是漏做）：
//   · **Device Code Flow** —— MCP 没要求，且多一段「去另一个页面输代码」的交互；
//   · **`private_key_jwt` / mTLS 客户端认证** —— 桌面客户端没有密钥托管，用不上；
//   · **令牌加密存储** —— 见上，与现有 API key 同级；
//   · **`resource_metadata` 提示** —— 我们不去解析 401 的 `WWW-Authenticate` 头，而是按
//     规范从 URL 推 `/.well-known/oauth-protected-resource`（两种拼法都试，见 `well_known`）。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write as IoWrite};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 交互式授权的等待上限：用户要在浏览器里点「同意」。到点就放弃这一次（不影响其它服务器）。
pub const INTERACTIVE_TIMEOUT: Duration = Duration::from_secs(180);
/// 令牌提前多久算过期 —— 留出一次请求的余量，别卡在「刚好这一秒过期」上。
const EXPIRY_SKEW_SECS: u64 = 60;
/// 一次元数据 / 换令牌请求的超时。
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

// ── 令牌与存储 ─────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// unix 秒。**0 = 服务器没给 `expires_in`** ⇒ 当作「不知道到期时间」，
    /// 靠 401 触发刷新（而不是当成「永不过期」）。
    #[serde(default)]
    pub expires_at: u64,
    /// 换令牌时要用（刷新时也用它，不重新发现一次）。
    #[serde(default)]
    pub token_endpoint: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    /// RFC 8707 的 `resource`（= MCP 服务器 URL）。签发与刷新都要**原样**带上，
    /// 否则拿到的令牌 audience 不对，服务器照样回 401。
    #[serde(default)]
    pub resource: String,
}

impl Tokens {
    /// 是否该刷新：有 refresh_token，且（已知到期且快到了）。
    /// 到期时间未知（`expires_at == 0`）时**不主动刷** —— 只有 401 才刷（少一次无用请求）。
    fn needs_refresh(&self) -> bool {
        self.refresh_token.is_some() && self.expires_at > 0 && now_secs() + EXPIRY_SKEW_SECS >= self.expires_at
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 令牌文件的路径：`mcp.json` 的同级 `mcp-tokens.json`。
/// `LUNAC_MCP_FILE` 没给（独立烟测）时返回 `None` ⇒ OAuth 不可用（只用 `headers`）。
pub fn store_path(mcp_file: Option<&str>) -> Option<PathBuf> {
    let f = mcp_file?.trim();
    if f.is_empty() {
        return None;
    }
    let p = Path::new(f);
    Some(p.parent().unwrap_or(Path::new(".")).join("mcp-tokens.json"))
}

/// 读一个服务器的令牌。文件不在 / 读不出来 / 解析失败一律 `None`（= 需要授权）。
fn load_all(path: &Path) -> HashMap<String, Tokens> {
    match std::fs::read_to_string(path) {
        Ok(t) => serde_json::from_str(t.trim_start_matches('\u{feff}')).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

pub fn load(path: &Path, key: &str) -> Option<Tokens> {
    load_all(path).remove(key)
}

/// 写一个服务器的令牌。**读-改-写**（不是整体覆盖）：多台服务器各自持有自己的那份，
/// 整体覆盖会让「刚授权完 A、又授权 B」把 A 抹掉。
pub fn store(path: &Path, key: &str, tokens: &Tokens) -> Result<(), String> {
    let mut all = load_all(path);
    all.insert(key.to_string(), tokens.clone());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败: {e}"))?;
    }
    let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("写令牌失败: {e}"))
}

// ── 刷新（不弹浏览器的那条路）──────────────────────────────────────

/// 按需刷新：没到期就原样返回。刷新失败**不删**旧令牌（进程内还能继续试）。
pub fn ensure_fresh(path: &Path, key: &str, tokens: Tokens) -> Tokens {
    if !tokens.needs_refresh() {
        return tokens;
    }
    match refresh(&tokens) {
        Ok(fresh) => {
            eprintln!("[agent] MCP OAuth：{} 的令牌已刷新", key);
            if let Err(e) = store(path, key, &fresh) {
                eprintln!("[agent] MCP OAuth：刷新后的令牌写不进去（{e}）—— 本次会话仍然可用");
            }
            fresh
        }
        Err(e) => {
            eprintln!("[agent] MCP OAuth：令牌刷新失败（{e}）—— 继续用旧的，401 时再报");
            tokens
        }
    }
}

/// 用 `refresh_token` 换一个新的 access_token。
pub fn refresh(t: &Tokens) -> Result<Tokens, String> {
    let Some(rt) = t.refresh_token.as_deref() else {
        return Err("没有 refresh_token".into());
    };
    if t.token_endpoint.is_empty() {
        return Err("令牌记录里没有 token_endpoint".into());
    }
    let client = http_client()?;
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", rt),
    ];
    if !t.client_id.is_empty() {
        form.push(("client_id", t.client_id.as_str()));
    }
    if let Some(sec) = t.client_secret.as_deref() {
        form.push(("client_secret", sec));
    }
    if !t.resource.is_empty() {
        form.push(("resource", t.resource.as_str()));
    }
    let resp = client
        .post(&t.token_endpoint)
        .form(&form)
        .send()
        .map_err(|e| format!("刷新请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        let snip: String = text.chars().take(200).collect();
        return Err(format!("刷新被拒 HTTP {} {}", status.as_u16(), snip.trim()));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("刷新回包不是 JSON: {e}"))?;
    Ok(merge_token_response(&v, t))
}

/// 把一次 token 响应并进既有记录（**同一个函数供「首次换令牌」与「刷新」共用**，
/// 免得两处对 `expires_in` / `refresh_token` 缺省的处置漂移）。
fn merge_token_response(v: &Value, prev: &Tokens) -> Tokens {
    let access = v
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let expires_at = match v.get("expires_in").and_then(Value::as_u64) {
        Some(s) => now_secs() + s,
        None => 0, // 服务器没给 ⇒ 不猜
    };
    Tokens {
        access_token: access,
        // **刷新可能不返回新的 refresh_token** ⇒ 保留旧的那份（清掉等于把用户踢下线）
        refresh_token: v
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| prev.refresh_token.clone()),
        expires_at,
        token_endpoint: prev.token_endpoint.clone(),
        client_id: v
            .get("client_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| prev.client_id.clone()),
        client_secret: prev.client_secret.clone(),
        resource: prev.resource.clone(),
    }
}

// ── 交互式授权（会开浏览器 & 阻塞）─────────────────────────────────

/// 完整的交互式授权。**只有在「连服务器那一步」调它**。
///
/// 顺序刻意是「**先起环回端口、再注册客户端**」：动态注册要把 `redirect_uris` 报上去，
/// 而端口是系统随机分的（`127.0.0.1:0`）。
pub fn authorize(server_url: &str, timeout: Duration) -> Result<Tokens, String> {
    // 1) 环回端口 + 回调地址
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("起环回端口失败（{e}）—— 无法做 OAuth 授权"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("读环回端口失败: {e}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("设置端口属性失败: {e}"))?;

    // 2) 发现：受保护资源元数据 → 授权服务器元数据
    let client = http_client()?;
    let resource = discover_resource(&client, server_url)?;
    let meta = discover_as(&client, &resource.authorization_server)?;
    let auth_endpoint = meta
        .get("authorization_endpoint")
        .and_then(Value::as_str)
        .ok_or("授权服务器元数据里没有 authorization_endpoint")?
        .to_string();
    let token_endpoint = meta
        .get("token_endpoint")
        .and_then(Value::as_str)
        .ok_or("授权服务器元数据里没有 token_endpoint")?
        .to_string();

    // 3) 动态注册（RFC 7591）。没有 registration_endpoint 就没法自动拿 client_id
    //    —— 如实报错，让用户改用静态 headers，而不是绕一个半截的路。
    let (client_id, client_secret) = match meta.get("registration_endpoint").and_then(Value::as_str) {
        Some(reg) => register_client(&client, reg, &redirect_uri)?,
        None => {
            return Err(
                "这台授权服务器不支持动态客户端注册（没有 registration_endpoint）\
                 —— 请改用配置里的 headers（静态令牌）"
                    .into(),
            )
        }
    };

    // 4) PKCE
    let verifier = random_b64(32);
    let challenge = s256(&verifier);
    let state = random_b64(16);

    // 5) 授权 URL：RFC 8707 的 `resource` 必须带上（MCP 规范要求）
    let mut auth_url = reqwest::Url::parse(&auth_endpoint)
        .map_err(|e| format!("authorization_endpoint 不是合法 URL: {e}"))?;
    {
        let mut q = auth_url.query_pairs_mut();
        q.append_pair("response_type", "code");
        q.append_pair("client_id", &client_id);
        q.append_pair("redirect_uri", &redirect_uri);
        q.append_pair("code_challenge", &challenge);
        q.append_pair("code_challenge_method", "S256");
        q.append_pair("state", &state);
        q.append_pair("resource", server_url);
        if let Some(scopes) = meta.get("scopes_supported").and_then(Value::as_array) {
            // 只在与服务器声明的 scope 有交集时才带 —— 不猜、也不硬塞
            let list: Vec<&str> = scopes.iter().filter_map(Value::as_str).collect();
            if !list.is_empty() {
                // 默认只申请前几个（列表可能很长，全带上会吓到用户 / 被服务器拒）
                q.append_pair("scope", &list.iter().take(6).copied().collect::<Vec<_>>().join(" "));
            }
        }
    }

    // 6) 开浏览器
    eprintln!("[agent] MCP OAuth：请在浏览器里完成授权（最多等 {}s）", timeout.as_secs());
    open_in_browser(auth_url.as_str())?;

    // 7) 等回调
    let code = wait_for_code(&listener, &state, timeout)?;

    // 8) 换令牌
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("client_id", client_id.as_str()),
        ("code_verifier", verifier.as_str()),
        ("resource", server_url),
    ];
    if let Some(sec) = client_secret.as_deref() {
        form.push(("client_secret", sec));
    }
    let resp = client
        .post(&token_endpoint)
        .form(&form)
        .send()
        .map_err(|e| format!("换令牌请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        let snip: String = text.chars().take(200).collect();
        return Err(format!("换令牌被拒 HTTP {} {}", status.as_u16(), snip.trim()));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("令牌回包不是 JSON: {e}"))?;
    let base = Tokens {
        token_endpoint,
        client_id,
        client_secret,
        resource: server_url.to_string(),
        ..Default::default()
    };
    let tokens = merge_token_response(&v, &base);
    if tokens.access_token.is_empty() {
        return Err("令牌回包里没有 access_token".into());
    }
    Ok(tokens)
}

/// 受保护资源元数据（RFC 9728）→ 取第一个授权服务器 issuer。
fn discover_resource(client: &reqwest::blocking::Client, server_url: &str) -> Result<ResourceMeta, String> {
    for candidate in well_known(server_url) {
        if let Ok(v) = get_json(client, &candidate) {
            let issuer = v
                .get("authorization_servers")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
                .or_else(|| v.get("authorization_server").and_then(Value::as_str));
            if let Some(iss) = issuer {
                return Ok(ResourceMeta {
                    authorization_server: iss.trim_end_matches('/').to_string(),
                });
            }
            return Err(format!(
                "{candidate} 里没有 authorization_servers —— 服务器没按规范声明授权服务器"
            ));
        }
    }
    Err(format!(
        "找不到受保护资源元数据（试过 {}）—— 这台服务器可能不用 OAuth",
        well_known(server_url).join(" / ")
    ))
}

struct ResourceMeta {
    authorization_server: String,
}

/// 授权服务器元数据（RFC 8414；顺带兼容 OIDC 的 `/.well-known/openid-configuration`）。
fn discover_as(client: &reqwest::blocking::Client, issuer: &str) -> Result<Value, String> {
    let base = issuer.trim_end_matches('/');
    let oidc = format!("{base}/.well-known/openid-configuration");
    // issuer 不是合法 URL 时这条路径给不出候选 —— 那就只试 OIDC 那条（别拿空串去发请求）
    let oauth = match reqwest::Url::parse(base) {
        Ok(u) => {
            let origin = origin_of(&u);
            let path = u.path();
            if path == "/" || path.is_empty() {
                format!("{origin}/.well-known/oauth-authorization-server")
            } else {
                format!("{origin}/.well-known/oauth-authorization-server{path}")
            }
        }
        Err(_) => String::new(),
    };
    for candidate in [oauth, oidc] {
        if candidate.is_empty() {
            continue;
        }
        if let Ok(v) = get_json(client, &candidate) {
            return Ok(v);
        }
    }
    Err(format!("读不到授权服务器元数据（{issuer}）"))
}

/// 动态客户端注册（RFC 7591）。公开客户端（`token_endpoint_auth_method: none`）+ PKCE。
fn register_client(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    redirect_uri: &str,
) -> Result<(String, Option<String>), String> {
    let body = json!({
        "client_name": "Lunac",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let resp = client
        .post(endpoint)
        .json(&body)
        .send()
        .map_err(|e| format!("客户端注册请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        let snip: String = text.chars().take(200).collect();
        return Err(format!("客户端注册被拒 HTTP {} {}", status.as_u16(), snip.trim()));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("注册回包不是 JSON: {e}"))?;
    let id = v
        .get("client_id")
        .and_then(Value::as_str)
        .ok_or("注册回包里没有 client_id")?
        .to_string();
    let secret = v
        .get("client_secret")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok((id, secret))
}

/// 等浏览器把 `?code=…` 打回来。**非阻塞轮询 + 截止时间**（比线程 + channel 简单，
/// 而且超时行为一眼可见）。非 `/callback` 的请求（浏览器会顺带要 favicon）直接忽略、继续等。
fn wait_for_code(listener: &TcpListener, state: &str, timeout: Duration) -> Result<String, String> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(found) = handle_callback(stream, state) {
                    return found;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => return Err(format!("环回端口出错: {e}")),
        }
    }
    Err(format!("等授权回调超时（{}s）", timeout.as_secs()))
}

/// 处理一个回调请求。返回 `Some(Ok(code))` / `Some(Err(原因))`；不是回调就 `None`。
fn handle_callback(mut stream: TcpStream, state: &str) -> Option<Result<String, String>> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let path = line.split_whitespace().nth(1).unwrap_or("");
    if !path.starts_with("/callback") {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n");
        return None;
    }
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    let params: HashMap<String, String> = query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), percent_decode(v)))
        .collect();

    // 回执页：**content-length 必须按 body 的实际字节数算**（写错会让浏览器一直转圈）
    let page = |title: &str, body: &str| -> Vec<u8> {
        let html = format!(
            "<!doctype html><meta charset=\"utf-8\"><title>{title}</title>\
             <body style=\"font-family:system-ui;padding:40px\"><h3>{title}</h3><p>{body}</p></body>"
        );
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{html}",
            html.len()
        )
        .into_bytes()
    };

    if params.get("state").map(String::as_str) != Some(state) {
        // state 不匹配 = 不是我们发起的那次（或者被第三方打回来了）⇒ 必须拒
        let _ = stream.write_all(&page("授权失败", "state 不匹配，已忽略这次回调。"));
        return Some(Err("回调的 state 与本次授权不一致".into()));
    }
    if let Some(err) = params.get("error") {
        let desc = params.get("error_description").cloned().unwrap_or_default();
        let _ = stream.write_all(&page("授权失败", &format!("{err} {desc}")));
        return Some(Err(format!("服务器拒绝了授权：{err} {desc}")));
    }
    match params.get("code") {
        Some(code) if !code.is_empty() => {
            let _ = stream.write_all(&page("授权完成", "可以关闭这个页面，回到 Lunac 继续。"));
            Some(Ok(code.clone()))
        }
        _ => {
            let _ = stream.write_all(&page("授权失败", "没有拿到授权码。"));
            Some(Err("回调里没有 code".into()))
        }
    }
}

// ── 小工具 ─────────────────────────────────────────────────────────

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("建 HTTP client 失败: {e}"))
}

fn get_json(client: &reqwest::blocking::Client, url: &str) -> Result<Value, String> {
    let resp = client
        .get(url)
        .header("accept", "application/json")
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let text = resp.text().map_err(|e| e.to_string())?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())
}

/// `https://host[:port]`
fn origin_of(u: &reqwest::Url) -> String {
    let mut s = format!("{}://{}", u.scheme(), u.host_str().unwrap_or(""));
    if let Some(p) = u.port() {
        s.push_str(&format!(":{p}"));
    }
    s
}

/// 受保护资源元数据的候选地址（规范：把 `/.well-known/oauth-protected-resource`
/// **插在 path 之前**；但确实有实现只挂在 origin 上 —— 两种都试，取先成功的那个）。
fn well_known(server_url: &str) -> Vec<String> {
    let Ok(u) = reqwest::Url::parse(server_url) else {
        return Vec::new();
    };
    let origin = origin_of(&u);
    let path = u.path().trim_end_matches('/');
    let mut out = Vec::new();
    if path.is_empty() {
        out.push(format!("{origin}/.well-known/oauth-protected-resource"));
    } else {
        out.push(format!("{origin}/.well-known/oauth-protected-resource{path}"));
    }
    let plain = format!("{origin}/.well-known/oauth-protected-resource");
    if !out.contains(&plain) {
        out.push(plain);
    }
    out
}

/// 用系统默认浏览器打开一个 URL。Windows 下必须走 `cmd /C start`，
/// 且**拼命令行要用 `raw_arg`**（`arg` 会把引号按 MSVC 规则转义，含 `&` 的授权 URL 整条失败）。
fn open_in_browser(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new("cmd");
        cmd.arg("/C")
            .raw_arg(format!("start \"\" \"{url}\""))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        cmd.spawn().map_err(|e| format!("打不开浏览器: {e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        Err("当前平台没有实现「打开浏览器」".into())
    }
}

/// `n` 字节的密码学随机数 → base64url（无填充）。
fn random_b64(n: usize) -> String {
    let mut buf = vec![0u8; n];
    // getrandom 失败是极罕见的环境问题；退回时间 + 地址熵（够本地环回流程用，
    // 但仍**不能**当密码学强度 —— 所以失败时留一行 warn）。
    if getrandom::getrandom(&mut buf).is_err() {
        eprintln!("[agent] MCP OAuth：系统随机源不可用，退回到时间熵（本次授权仍是环回 + PKCE）");
        let mut seed = now_secs()
            ^ (std::process::id() as u64).rotate_left(17)
            ^ (&buf as *const _ as u64);
        for b in buf.iter_mut() {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *b = (seed >> 33) as u8;
        }
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// PKCE 的 S256 挑战 = base64url(sha256(verifier))。
fn s256(verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(h.finalize())
}

/// 极简 percent-decode（只处理回调里的 `code` / `state` / `error_description`）。
/// **不引 urlencoding**：这一层只需认 `%XX` 与 `+`。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PKCE 的 S256 必须与 RFC 7636 附录 B 的向量一致（这是唯一能自证的判据）。
    #[test]
    fn s256_matches_rfc7636_vector() {
        assert_eq!(
            s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    /// 令牌文件路径 = `mcp.json` 同级；没给配置路径时 OAuth 整体不可用。
    #[test]
    fn store_path_sits_next_to_the_config() {
        let p = store_path(Some(r"D:\Lunac\config\mcp.json")).unwrap();
        assert!(p.ends_with("mcp-tokens.json"));
        assert_eq!(p.parent().unwrap(), Path::new(r"D:\Lunac\config"));
        assert!(store_path(None).is_none());
        assert!(store_path(Some("   ")).is_none());
    }

    /// `expires_in` 缺省必须记成 0（= 不知道），**不能**当成「永不过期」去猜一个值。
    #[test]
    fn expires_in_defaults_to_unknown() {
        let base = Tokens {
            token_endpoint: "https://as.example/token".into(),
            client_id: "cid".into(),
            resource: "https://mcp.example/mcp".into(),
            ..Default::default()
        };
        let t = merge_token_response(&json!({ "access_token": "at" }), &base);
        assert_eq!(t.expires_at, 0);
        assert!(!t.needs_refresh(), "到期时间未知时不该主动刷（交给 401）");
        assert_eq!(t.token_endpoint, "https://as.example/token");
        assert_eq!(t.resource, "https://mcp.example/mcp");
    }

    /// **刷新不带新 refresh_token 时必须保留旧的** —— 清掉等于把用户踢下线，
    /// 下次就得重新走一遍浏览器授权。
    #[test]
    fn refresh_keeps_the_old_refresh_token_when_absent() {
        let base = Tokens {
            access_token: "old".into(),
            refresh_token: Some("rt-1".into()),
            expires_at: now_secs() + 10, // 已到期（含 60s 余量）
            token_endpoint: "https://as.example/token".into(),
            client_id: "cid".into(),
            resource: "https://mcp.example/mcp".into(),
            ..Default::default()
        };
        assert!(base.needs_refresh(), "有 refresh_token 且快到期 ⇒ 该刷");
        let t = merge_token_response(&json!({ "access_token": "new", "expires_in": 3600 }), &base);
        assert_eq!(t.access_token, "new");
        assert_eq!(t.refresh_token.as_deref(), Some("rt-1"), "旧的 refresh_token 必须留着");
        assert!(t.expires_at > now_secs() + 3000);
        assert!(!t.needs_refresh());
    }

    /// 受保护资源元数据的候选顺序：先「把 well-known 插在 path 之前」，再退回 origin 根。
    #[test]
    fn well_known_tries_path_first_then_origin() {
        let both = well_known("https://mcp.example.com/api/mcp");
        assert_eq!(
            both,
            vec![
                "https://mcp.example.com/.well-known/oauth-protected-resource/api/mcp",
                "https://mcp.example.com/.well-known/oauth-protected-resource",
            ]
        );
        // 没有 path 时只留一条，不重复
        assert_eq!(
            well_known("https://mcp.example.com"),
            vec!["https://mcp.example.com/.well-known/oauth-protected-resource"]
        );
        // 带端口要保留
        assert!(well_known("http://127.0.0.1:8801/mcp")[0]
            .starts_with("http://127.0.0.1:8801/.well-known/"));
    }

    #[test]
    fn percent_decode_handles_plus_and_escapes() {
        assert_eq!(percent_decode("a%2Fb+c"), "a/b c");
        assert_eq!(percent_decode("bad%"), "bad%");
        assert_eq!(percent_decode("plain"), "plain");
    }
}
