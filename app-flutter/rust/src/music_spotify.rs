//! Spotify 在线库（移植 v1 `app/src-tauri/src/music.rs` 的 Web API 那一段）。
//!
//! **范围**：OAuth PKCE 授权 → 令牌刷新 → 播放状态 / 播放控制 / 歌单 / 收藏 / 搜索 /
//! 设备 / 播放设备接管。**不含本机播放**：按用户 2026-11-19 的裁决，v2 不启动
//! librespot 子进程，也不再把本机 PCM 接进调音链（实现已移除，不做半套）。
//!
//! 凭据落 `config/music.json`（与 v1 同一个文件、同一套 snake_case 键名）：
//! `client_id` / `port` / `access_token` / `refresh_token` / `expires_at` / `display_name` /
//! `avatar`。**不认识的键原样保留**（`serde_json::Value` 就地增删），这样 v1 写下的文件不会被抹掉。
//!
//! 纪律（逐条来自 v1，少一条就对应一个具体坏表现）：
//!   ① **429 硬闸**：命中限流就把引擎停到 `Retry-After` 到期，期间所有请求直接返回
//!      `ERR_RATE_LIMITED:<秒>`（面板显示「等 N 秒」，并提供手动恢复按钮）；
//!   ② **刷新失败 = 令牌失效**：清空本地令牌并回 `ERR_NOT_CONNECTED`，让面板退回未连接
//!      （否则用户看到的是一个反复 401 的死面板）；
//!   ③ **Spotify 刷新时可能不返回新的 refresh_token** ⇒ 保留旧的（清掉等于把用户踢下线）；
//!   ④ 环回回调只认 `state` 对得上的那一次，端口用配置里的（0 回落默认）。
//!
//! 可测性：PKCE（RFC 7636 测试向量）、回调解析、安全的 id / URI 校验、JSON → DTO 映射、
//! 429 闸门状态机都是纯函数；网络请求只在运行时发生。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SPOTIFY_AUTHORIZE: &str = "https://accounts.spotify.com/authorize";
const SPOTIFY_TOKEN: &str = "https://accounts.spotify.com/api/token";
const SPOTIFY_API: &str = "https://api.spotify.com/v1";
/// 内置 Client ID（PKCE 公开客户端，无 client secret）。
const BUILTIN_CLIENT_ID: &str = "590875b9724848a284198bfe1bd85588";
const SCOPES: &str = "user-read-playback-state user-modify-playback-state user-read-currently-playing playlist-read-private playlist-read-collaborative user-library-read user-follow-read user-top-read";
const DEFAULT_PORT: u16 = 8899;
const UA: &str = "Lunac/2 (+https://lunac.pet)";
/// 出网请求窗口（用于「是不是我们在刷接口」的观测）。
const REQ_WINDOW: Duration = Duration::from_secs(30);
const REQ_WINDOW_WARN: u32 = 60;
const LIKED_LIMIT: i64 = 50;
/// 授权回调最长等多久（v1 是「一直等」；这里给个上限，别留一个永远挂着的线程）。
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(180);

// ══════════════════════════════════════════════════════════════════
//  配置（config/music.json）
// ══════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpotifyConfig {
    pub client_id: String,
    pub port: u16,
    pub access_token: String,
    pub refresh_token: String,
    /// access_token 的到期时刻（unix 秒）；0 = 未知（当作已过期）。
    pub expires_at: i64,
    pub display_name: String,
    pub avatar: String,
}

impl Default for SpotifyConfig {
    fn default() -> Self {
        Self {
            client_id: BUILTIN_CLIENT_ID.to_string(),
            port: DEFAULT_PORT,
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: 0,
            display_name: String::new(),
            avatar: String::new(),
        }
    }
}

impl SpotifyConfig {
    pub fn effective_port(&self) -> u16 {
        if self.port == 0 {
            DEFAULT_PORT
        } else {
            self.port
        }
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.effective_port())
    }

    pub fn client_id(&self) -> String {
        let trimmed = self.client_id.trim();
        if trimmed.is_empty() {
            BUILTIN_CLIENT_ID.to_string()
        } else {
            trimmed.to_string()
        }
    }

    pub fn has_token(&self) -> bool {
        !self.access_token.is_empty() || !self.refresh_token.is_empty()
    }
}

pub fn config_path(root: &Path) -> PathBuf {
    root.join("config").join("music.json")
}

fn load_config(root: &Path) -> SpotifyConfig {
    match std::fs::read_to_string(config_path(root)) {
        Ok(text) => serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default(),
        Err(_) => SpotifyConfig::default(),
    }
}

/// 只改我们认识的键，**其余字段原样保留**（例如 v1 写下的 `librespot_*`：本版不再读它，
/// 但也不抹掉用户的旧配置）。
fn save_config(root: &Path, config: &SpotifyConfig) -> Result<(), String> {
    let path = config_path(root);
    let mut value = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
            .unwrap_or_else(|_| serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    };
    if !value.is_object() {
        value = serde_json::json!({});
    }
    let object = value.as_object_mut().expect("刚保证是对象");
    let ours = serde_json::to_value(config).map_err(|e| format!("序列化音乐配置失败：{e}"))?;
    if let Some(items) = ours.as_object() {
        for (key, item) in items {
            object.insert(key.clone(), item.clone());
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建 config 目录失败：{e}"))?;
    }
    let text = serde_json::to_string_pretty(&value).map_err(|e| format!("序列化失败：{e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写不进 {}：{e}", path.display()))
}

/// 面板保存「自己的 Client ID / 回调端口」（v1 `music_config_set` 的两个字段）。
///
/// 换 Client ID 或端口时**清掉旧令牌**：令牌绑在 `(client_id, redirect_uri)` 上，
/// 留着只会让面板显示「已连接」而请求全部 401（v1 同一条纪律）。
/// 两个参数都是 `Option`：`None` = 别动，避免「只改端口」把用户填的 Client ID 抹掉。
pub fn save_settings(
    root: &Path,
    client_id: Option<&str>,
    port: Option<u16>,
) -> Result<(), String> {
    let mut config = load_config(root);
    // 比较**生效值**：`client_id` 为空 = 用内置值，与显式写入内置值是同一件事。
    let before_client = config.client_id();
    let before_port = config.effective_port();
    if let Some(id) = client_id {
        let id = id.trim();
        if id.is_empty() {
            return Err("Client ID 不能为空（要恢复内置值请清空 config/music.json 的 client_id）".into());
        }
        config.client_id = id.to_string();
    }
    if let Some(port) = port {
        if !(1024..=65535).contains(&port) {
            return Err(format!("端口不合法：{port}（要 1024–65535）"));
        }
        config.port = port;
    }
    if config.client_id() != before_client || config.effective_port() != before_port {
        config.access_token.clear();
        config.refresh_token.clear();
        config.expires_at = 0;
        config.display_name.clear();
        config.avatar.clear();
    }
    save_config(root, &config)
}

// ══════════════════════════════════════════════════════════════════
//  PKCE / 时间
// ══════════════════════════════════════════════════════════════════
/// `code_verifier`（RFC 7636 §4.1：43–128 字符，字母数字 + `-._~`）。
pub fn random_verifier() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut bytes = [0u8; 64];
    // `getrandom` 是既有依赖（SS 的 salt 也用它）：拿系统熵，别自造 PRNG。
    if getrandom::getrandom(&mut bytes).is_err() {
        // 极端情况下退回时间 + 地址熵（仍然不是固定值，绝不把 PKCE 退化成常量）。
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x1234_5678);
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = (nanos.rotate_left(index as u32) ^ (index as u64 * 0x9E37_79B9)) as u8;
        }
    }
    bytes
        .iter()
        .map(|byte| ALPHABET[(*byte as usize) % ALPHABET.len()] as char)
        .collect()
}

/// `code_challenge = base64url(sha256(verifier))`，无填充（S256）。
pub fn code_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 授权页 URL（纯函数：单测比对参数，不联网）。
pub fn authorize_url(config: &SpotifyConfig, verifier: &str, state: &str) -> String {
    format!(
        "{SPOTIFY_AUTHORIZE}?response_type=code&client_id={}&scope={}&redirect_uri={}&code_challenge_method=S256&code_challenge={}&state={}",
        url_encode(&config.client_id()),
        url_encode(SCOPES),
        url_encode(&config.redirect_uri()),
        code_challenge(verifier),
        url_encode(state),
    )
}

fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// ══════════════════════════════════════════════════════════════════
//  429 硬闸 + 出网观测（v1 2026-10-02 口径）
// ══════════════════════════════════════════════════════════════════

struct StopState {
    /// 硬闸在什么时候解除（unix 秒）；0 = 没停。
    until: i64,
    reason: String,
}

static STOP: Mutex<StopState> = Mutex::new(StopState {
    until: 0,
    reason: String::new(),
});
static REQ_COUNT: AtomicU32 = AtomicU32::new(0);
static REQ_WINDOW_START: AtomicI64 = AtomicI64::new(0);

fn retry_remaining_secs() -> u64 {
    let Ok(guard) = STOP.lock() else {
        return 0;
    };
    let now = now_secs();
    if guard.until <= now {
        return 0;
    }
    (guard.until - now) as u64
}

fn trip_stop(reason: &str, retry_after_secs: u64) {
    let until = now_secs() + retry_after_secs.max(1) as i64;
    if let Ok(mut guard) = STOP.lock() {
        guard.until = until;
        guard.reason = reason.to_string();
    }
}

fn clear_stop() {
    if let Ok(mut guard) = STOP.lock() {
        guard.until = 0;
        guard.reason.clear();
    }
}

fn stop_json() -> serde_json::Value {
    let remaining = retry_remaining_secs();
    let reason = STOP
        .lock()
        .map(|guard| guard.reason.clone())
        .unwrap_or_default();
    serde_json::json!({
        "active": remaining > 0,
        "reason": reason,
        "retryAfterSecs": remaining,
    })
}

/// 记一次出网请求；返回窗口内的计数（用于日志观测，不参与限流）。
fn note_request() -> u32 {
    let now = now_secs();
    let start = REQ_WINDOW_START.load(Ordering::Relaxed);
    if now - start >= REQ_WINDOW.as_secs() as i64 {
        REQ_WINDOW_START.store(now, Ordering::Relaxed);
        REQ_COUNT.store(1, Ordering::Relaxed);
        return 1;
    }
    REQ_COUNT.fetch_add(1, Ordering::Relaxed) + 1
}

fn http() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败：{e}"))
}

/// 所有 Spotify 请求的唯一出口：先查硬闸，再发；429 时落闸并回 `ERR_RATE_LIMITED:<秒>`。
fn spotify_send(
    request: reqwest::blocking::RequestBuilder,
) -> Result<reqwest::blocking::Response, String> {
    let remaining = retry_remaining_secs();
    if remaining > 0 {
        return Err(format!("ERR_RATE_LIMITED:{remaining}"));
    }
    let count = note_request();
    if count == REQ_WINDOW_WARN {
        eprintln!("[lunac-music] 30 秒内已发 {count} 次 Spotify 请求，接近限流");
    }
    let response = request
        .header("User-Agent", UA)
        .send()
        .map_err(|e| format!("请求 Spotify 失败：{e}"))?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|text| text.trim().parse::<u64>().ok())
            .unwrap_or(30);
        trip_stop("Spotify 限流（429）", retry);
        return Err(format!("ERR_RATE_LIMITED:{retry}"));
    }
    Ok(response)
}

// ══════════════════════════════════════════════════════════════════
//  令牌
// ══════════════════════════════════════════════════════════════════

#[derive(Debug, Default, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: i64,
}

/// 拿一个可用的 access_token（快过期先刷新）。
fn ensure_access_token(root: &Path, config: &mut SpotifyConfig) -> Result<String, String> {
    if !config.access_token.is_empty() && config.expires_at > now_secs() + 60 {
        return Ok(config.access_token.clone());
    }
    if config.refresh_token.is_empty() {
        return Err("ERR_NOT_CONNECTED".into());
    }
    let client = http()?;
    let response = client
        .post(SPOTIFY_TOKEN)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", config.refresh_token.as_str()),
            ("client_id", config.client_id().as_str()),
        ])
        .send()
        .map_err(|e| format!("刷新令牌失败：{e}"))?;
    let status = response.status();
    let text = response.text().unwrap_or_default();
    if !status.is_success() {
        // 刷新被拒 = 令牌已失效：清空本地令牌，让面板退回「未连接」。
        config.access_token.clear();
        config.refresh_token.clear();
        config.expires_at = 0;
        let _ = save_config(root, config);
        return Err("ERR_NOT_CONNECTED".into());
    }
    let token: TokenResponse =
        serde_json::from_str(&text).map_err(|e| format!("令牌响应解析失败：{e}"))?;
    if token.access_token.is_empty() {
        return Err("ERR_NOT_CONNECTED".into());
    }
    config.access_token = token.access_token;
    // Spotify 刷新时可能不返回新的 refresh_token ⇒ 保留旧的。
    if !token.refresh_token.is_empty() {
        config.refresh_token = token.refresh_token;
    }
    config.expires_at = now_secs() + token.expires_in.max(60);
    save_config(root, config)?;
    Ok(config.access_token.clone())
}

/// 带一次自动刷新的调用：401 时刷令牌后重试一次。
fn with_token<T>(
    root: &Path,
    call: impl Fn(&str) -> Result<T, String>,
) -> Result<T, String> {
    let mut config = load_config(root);
    let token = ensure_access_token(root, &mut config)?;
    match call(&token) {
        Ok(value) => Ok(value),
        Err(reason) if reason == "ERR_UNAUTHORIZED" => {
            config.access_token.clear();
            config.expires_at = 0;
            let token = ensure_access_token(root, &mut config)?;
            call(&token)
        }
        Err(reason) => Err(reason),
    }
}

fn api_get_json(root: &Path, url: &str) -> Result<serde_json::Value, String> {
    with_token(root, |token| {
        let client = http()?;
        let response = spotify_send(client.get(url).bearer_auth(token))?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err("ERR_UNAUTHORIZED".into());
        }
        json_or_reason(response, url)
    })
}

fn api_send(
    root: &Path,
    method: &str,
    url: &str,
    body: Option<serde_json::Value>,
) -> Result<(), String> {
    with_token(root, |token| {
        let client = http()?;
        let mut request = match method {
            "PUT" => client.put(url),
            "POST" => client.post(url),
            _ => client.post(url),
        }
        .bearer_auth(token);
        if let Some(payload) = body.clone() {
            request = request.json(&payload);
        }
        let response = spotify_send(request)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err("ERR_UNAUTHORIZED".into());
        }
        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status();
        let text = response.text().unwrap_or_default();
        Err(format!("Spotify 返回 {status}：{}", short(&text)))
    })
}

fn json_or_reason(
    response: reqwest::blocking::Response,
    url: &str,
) -> Result<serde_json::Value, String> {
    let status = response.status();
    let text = response.text().unwrap_or_default();
    if status.is_success() {
        if text.trim().is_empty() {
            return Ok(serde_json::json!({}));
        }
        return serde_json::from_str(&text)
            .map_err(|e| format!("Spotify 响应解析失败（{url}）：{e}"));
    }
    Err(format!("Spotify 返回 {status}：{}", short(&text)))
}

fn short(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() > 200 {
        trimmed.chars().take(200).collect()
    } else {
        trimmed.to_string()
    }
}

// ══════════════════════════════════════════════════════════════════
//  环回回调（PKCE 授权）
// ══════════════════════════════════════════════════════════════════

/// `GET /callback?code=..&state=.. HTTP/1.1` → 请求目标。
pub fn parse_request_target(line: &str) -> Option<String> {
    let mut parts = line.split_whitespace();
    parts.next()?;
    parts.next().map(str::to_string)
}

/// 从请求目标里取查询参数（**不做 URL 解码**：code / state 都是 url-safe 字符集）。
pub fn query_param(target: &str, key: &str) -> Option<String> {
    let query = target.split_once('?')?.1;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some((name, value)) if name == key => return Some(value.to_string()),
            _ => continue,
        }
    }
    None
}

fn respond(mut stream: std::net::TcpStream, title: &str, body: &str) {
    use std::io::Write;
    let html = format!(
        "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><title>{title}</title>\
         <style>body{{font-family:system-ui,sans-serif;background:#111;color:#eee;display:flex;\
         align-items:center;justify-content:center;height:100vh;margin:0}}\
         div{{text-align:center;line-height:1.7}}</style></head><body><div><h2>{title}</h2>\
         <p>{body}</p></div></body></html>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// 授权流程的中间态（`connect` 起线程后由回调写入）。
static PENDING: Mutex<Option<String>> = Mutex::new(None);

fn open_browser(url: &str) -> Result<(), String> {
    // `cmd /C start "" <url>`：空标题是为了不让 URL 被当成窗口标题。
    let status = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .status()
        .map_err(|e| format!("打不开默认浏览器：{e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("打不开默认浏览器（请手动复制授权链接）".to_string())
    }
}

#[derive(Debug, Deserialize)]
struct AccountResponse {
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    images: Vec<serde_json::Value>,
}

fn exchange_code(
    root: &Path,
    config: &mut SpotifyConfig,
    code: &str,
    verifier: &str,
) -> Result<(), String> {
    let client = http()?;
    let response = client
        .post(SPOTIFY_TOKEN)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", config.redirect_uri().as_str()),
            ("client_id", config.client_id().as_str()),
            ("code_verifier", verifier),
        ])
        .send()
        .map_err(|e| format!("换取令牌失败：{e}"))?;
    let status = response.status();
    let text = response.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!("换取令牌被拒（{status}）：{}", short(&text)));
    }
    let token: TokenResponse =
        serde_json::from_str(&text).map_err(|e| format!("令牌响应解析失败：{e}"))?;
    if token.access_token.is_empty() {
        return Err("换取令牌的响应里没有 access_token".to_string());
    }
    config.access_token = token.access_token;
    if !token.refresh_token.is_empty() {
        config.refresh_token = token.refresh_token;
    }
    config.expires_at = now_secs() + token.expires_in.max(60);
    if let Ok(account) = serde_json::from_str::<AccountResponse>(
        &client
            .get(format!("{SPOTIFY_API}/me"))
            .bearer_auth(&config.access_token)
            .header("User-Agent", UA)
            .send()
            .and_then(|response| response.text())
            .unwrap_or_default(),
    ) {
        config.display_name = account.display_name;
        config.avatar = account
            .images
            .first()
            .and_then(|image| image.get("url"))
            .and_then(|url| url.as_str())
            .unwrap_or_default()
            .to_string();
    }
    save_config(root, config)
}

// ══════════════════════════════════════════════════════════════════
//  DTO 映射（纯函数）
// ══════════════════════════════════════════════════════════════════

pub fn is_safe_id(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric())
}

/// `spotify:track:<id>` / `spotify:playlist:<id>` / `spotify:album:<id>` 等。
pub fn is_safe_uri(text: &str) -> bool {
    match text.split(':').collect::<Vec<_>>().as_slice() {
        ["spotify", "track", id] => is_safe_id(id),
        ["spotify", "episode", id] => is_safe_id(id),
        ["spotify", kind, id] if *kind == "playlist" || *kind == "album" || *kind == "artist" => {
            is_safe_id(id)
        }
        _ => false,
    }
}

/// `spotify:track:<id>`（**单曲**，`offset` 只接受这一种）。
fn is_track_uri(text: &str) -> bool {
    match text.split(':').collect::<Vec<_>>().as_slice() {
        ["spotify", "track", id] => is_safe_id(id),
        _ => false,
    }
}

/// `play` 的请求体（纯函数，便于单测）：三种形态与 v1 的三条命令逐一对应。
///
/// · `{uris:[…]}`  = v1 `spotify_play_uris`（整列播放）；
/// · `{uri}`        = v1 `spotify_play_uri`（从这一首开始，等价 `{uris:[uri]}`）；
/// · `{context, offset?}` = v1 `spotify_play_context` —— `offset` **只接受单曲 URI**，
///   非法时**静默忽略**（v1 的 `filter(is_safe_track_uri)` 就是这个口径，不报错）。
///
/// 三种都给时优先级 = uris > context > uri。
pub fn play_body(value: &serde_json::Value) -> Result<serde_json::Value, String> {
    let field = |key: &str| {
        value
            .get(key)
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let uris: Vec<String> = value
        .get("uris")
        .and_then(|item| item.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_str().map(|text| text.trim().to_string()))
                .filter(|text| !text.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if !uris.is_empty() {
        if !uris.iter().all(|item| is_safe_uri(item)) {
            return Err("曲目 URI 非法".to_string());
        }
        return Ok(serde_json::json!({ "uris": uris }));
    }
    let context = field("context");
    if !context.is_empty() {
        if !is_safe_uri(&context) {
            return Err("上下文 URI 非法".to_string());
        }
        let mut body = serde_json::json!({ "context_uri": context });
        let offset = field("offset");
        if is_track_uri(&offset) {
            body["offset"] = serde_json::json!({ "uri": offset });
        }
        return Ok(body);
    }
    let uri = field("uri");
    if !uri.is_empty() {
        if !is_safe_uri(&uri) {
            return Err("曲目 URI 非法".to_string());
        }
        return Ok(serde_json::json!({ "uris": [uri] }));
    }
    Ok(serde_json::json!({}))
}

pub fn track_json(item: &serde_json::Value) -> serde_json::Value {
    let text = |key: &str| {
        item.get(key)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
    };
    let artists = item
        .get("artists")
        .and_then(|value| value.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|artist| artist.get("name").and_then(|value| value.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let album = item.get("album");
    let image = album
        .and_then(|value| value.get("images"))
        .and_then(|value| value.as_array())
        .and_then(|list| list.first())
        .and_then(|value| value.get("url"))
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    serde_json::json!({
        "id": text("id"),
        "uri": text("uri"),
        "name": text("name"),
        "artists": artists,
        "album": album
            .and_then(|value| value.get("name"))
            .and_then(|value| value.as_str())
            .unwrap_or_default(),
        "cover": image,
        "durationMs": item.get("duration_ms").and_then(|value| value.as_i64()).unwrap_or(0),
        "progressMs": item.get("progress_ms").and_then(|value| value.as_i64()).unwrap_or(0),
        "isPlaying": item.get("is_playing").and_then(|value| value.as_bool()).unwrap_or(false),
    })
}

pub fn playlist_json(item: &serde_json::Value) -> serde_json::Value {
    let image = item
        .get("images")
        .and_then(|value| value.as_array())
        .and_then(|list| list.first())
        .and_then(|value| value.get("url"))
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    serde_json::json!({
        "id": item.get("id").and_then(|value| value.as_str()).unwrap_or_default(),
        "uri": item.get("uri").and_then(|value| value.as_str()).unwrap_or_default(),
        "name": item.get("name").and_then(|value| value.as_str()).unwrap_or_default(),
        "cover": image,
        "tracks": item
            .pointer("/tracks/total")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        "owner": item
            .pointer("/owner/display_name")
            .and_then(|value| value.as_str())
            .unwrap_or_default(),
    })
}

fn items_of(value: &serde_json::Value) -> Vec<serde_json::Value> {
    value
        .get("items")
        .and_then(|items| items.as_array())
        .cloned()
        .unwrap_or_default()
}

/// 一张「库里的条目」（歌手 / 专辑 / 播客）—— v1 `LibraryItemDto` 的同形状。
///
/// `/me/*` 的元素外面包了一层 `{added_at, album|show|artist}`，搜索返回的是裸对象；
/// 统一先剥一层再读，与 v1 `library_items` 同口径。
pub fn library_item_json(item: &serde_json::Value, kind: &str) -> serde_json::Value {
    let object = item
        .get("album")
        .or_else(|| item.get("show"))
        .or_else(|| item.get("artist"))
        .unwrap_or(item);
    let text = |value: &serde_json::Value, key: &str| {
        value
            .get(key)
            .and_then(|item| item.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let id = text(object, "id");
    let image = object
        .get("images")
        .and_then(|value| value.as_array())
        .and_then(|list| list.first())
        .and_then(|value| value.get("url"))
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let (uri, subtitle, total) = match kind {
        "artist" => (
            format!("spotify:artist:{id}"),
            object
                .get("genres")
                .and_then(|value| value.as_array())
                .and_then(|list| list.first())
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            0,
        ),
        "album" => (
            format!("spotify:album:{id}"),
            object
                .get("artists")
                .and_then(|value| value.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|artist| artist.get("name").and_then(|v| v.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
            object
                .get("total_tracks")
                .and_then(|value| value.as_i64())
                .unwrap_or(0),
        ),
        _ => (
            // show：故意留空（不是可播的 context_uri）。
            String::new(),
            text(object, "publisher"),
            object
                .get("total_episodes")
                .and_then(|value| value.as_i64())
                .unwrap_or(0),
        ),
    };
    serde_json::json!({
        "id": id,
        "uri": uri,
        "name": text(object, "name"),
        "cover": image,
        "subtitle": subtitle,
        "total": total,
        "kind": kind,
    })
}

/// 从 `/me/*` 响应里取列表并映射成 `library_item_json`（指针由调用方给）。
fn library_items(value: &serde_json::Value, pointer: &str, kind: &str) -> Vec<serde_json::Value> {
    value
        .pointer(pointer)
        .and_then(|items| items.as_array())
        .map(|list| {
            list.iter()
                .filter(|item| !item.is_null())
                .map(|item| library_item_json(item, kind))
                .filter(|item| !item["id"].as_str().unwrap_or_default().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// 「主区那一列曲目」的端点：`kind` 必须白名单（它拼进 URL 路径）。
///
/// 与 v1 `spotify_item_tracks` 同口径 —— 歌单用 **`/items`**（老 `/tracks` 现在 403）。
fn item_tracks_url(kind: &str, id: &str) -> Result<String, String> {
    if !is_safe_id(id) {
        return Err("资源 id 非法".to_string());
    }
    Ok(match kind {
        "playlist" => format!("{SPOTIFY_API}/playlists/{id}/items?limit=100"),
        "album" => format!("{SPOTIFY_API}/albums/{id}/tracks?limit=50"),
        // 歌手没有「曲目」资源 ⇒ 用热门曲；market 必填，from_token 按当前令牌地区挑。
        "artist" => format!("{SPOTIFY_API}/artists/{id}/top-tracks?market=from_token"),
        "show" => format!("{SPOTIFY_API}/shows/{id}/episodes?limit=50"),
        other => return Err(format!("未知的资源类型：{other}")),
    })
}

/// 把「一列曲目」的响应拍平：先剥 `{track|item}` 包装，再滤掉不可播的。
///
/// 下架 / 本机文件没有可播 `spotify:` uri ⇒ 一律滤掉，不给「点了播不了」的死条目。
fn tracks_from(value: &serde_json::Value, array_key: &str) -> Vec<serde_json::Value> {
    value
        .get(array_key)
        .and_then(|items| items.as_array())
        .map(|list| {
            list.iter()
                .filter(|item| !item.is_null())
                .map(|item| {
                    item.get("track")
                        .or_else(|| item.get("item"))
                        .unwrap_or(item)
                })
                .filter(|item| !item.is_null())
                .map(track_json)
                .filter(|item| is_safe_uri(item["uri"].as_str().unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default()
}

/// v1 `spotify_set_play_mode` 的三态映射（不提供「智能随机」—— Web API 没有该接口）。
fn play_mode_states(mode: &str) -> Result<(&'static str, &'static str), String> {
    match mode {
        "off" => Ok(("false", "off")),
        "shuffle" => Ok(("true", "off")),
        "repeat_one" => Ok(("false", "track")),
        other => Err(format!("未知的播放模式：{other}")),
    }
}

// ══════════════════════════════════════════════════════════════════
//  对外 JSON 口子
// ══════════════════════════════════════════════════════════════════

/// 面板状态：配置 + 是否已连接 + 429 闸门。
pub fn state_json(root: &Path) -> String {
    let config = load_config(root);
    serde_json::json!({
        "configured": !config.client_id().is_empty(),
        "connected": config.has_token(),
        // 是否在用内置 Client ID（面板据此把凭据字段收起来，v1 `MusicConfigDto.builtin` 同口径）
        "builtin": config.client_id.trim().is_empty()
            || config.client_id.trim() == BUILTIN_CLIENT_ID,
        "clientId": config.client_id(),
        "port": config.effective_port(),
        "redirectUri": config.redirect_uri(),
        "displayName": config.display_name,
        "avatar": config.avatar,
        "stop": stop_json(),
        "pending": PENDING.lock().map(|guard| guard.is_some()).unwrap_or(false),
        "path": config_path(root).display().to_string(),
    })
    .to_string()
}

/// 开始授权：起回调线程 + 打开浏览器，**立即返回**授权链接（面板轮询 `state` 看结果）。
pub fn connect_json(root: &Path) -> String {
    if PENDING.lock().map(|guard| guard.is_some()).unwrap_or(false) {
        return serde_json::json!({ "ok": false, "error": "上一次授权还没结束，请先在浏览器里完成或等它超时" })
            .to_string();
    }
    let config = load_config(root);
    let state = random_verifier();
    let verifier = random_verifier();
    let url = authorize_url(&config, &verifier, &state);
    if let Ok(mut guard) = PENDING.lock() {
        *guard = Some(state.clone());
    }
    let listener = match std::net::TcpListener::bind(("127.0.0.1", config.effective_port())) {
        Ok(listener) => listener,
        Err(e) => {
            if let Ok(mut guard) = PENDING.lock() {
                *guard = None;
            }
            return serde_json::json!({
                "ok": false,
                "error": format!(
                    "端口 {} 被占用，授权回调起不来：{e}（改 config/music.json 的 port 后重试）",
                    config.effective_port()
                ),
            })
            .to_string();
        }
    };
    let _ = listener.set_nonblocking(true);
    let root_owned = root.to_path_buf();
    std::thread::Builder::new()
        .name("lunac-spotify-oauth".to_string())
        .spawn(move || {
            let started = Instant::now();
            loop {
                if started.elapsed() > CALLBACK_TIMEOUT {
                    if let Ok(mut guard) = PENDING.lock() {
                        *guard = None;
                    }
                    eprintln!("[lunac-music] Spotify 授权超时（{CALLBACK_TIMEOUT:?}）");
                    return;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        use std::io::Read;
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        let mut buffer = [0u8; 4096];
                        let read = stream.read(&mut buffer).unwrap_or(0);
                        let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                        let target = parse_request_target(request.lines().next().unwrap_or(""))
                            .unwrap_or_default();
                        let got_state = query_param(&target, "state").unwrap_or_default();
                        let expected = PENDING
                            .lock()
                            .ok()
                            .and_then(|guard| guard.clone())
                            .unwrap_or_default();
                        if got_state != expected || expected.is_empty() {
                            respond(stream, "授权失败", "state 不匹配（请重新从 Lunac 发起授权）");
                            continue;
                        }
                        let Some(code) = query_param(&target, "code") else {
                            respond(stream, "授权失败", "回调里没有 code（可能被用户取消）");
                            if let Ok(mut guard) = PENDING.lock() {
                                *guard = None;
                            }
                            return;
                        };
                        let mut config = load_config(&root_owned);
                        match exchange_code(&root_owned, &mut config, &code, &verifier) {
                            Ok(()) => {
                                respond(stream, "已连接 Spotify", "可以回到 Lunac 继续了。");
                                if let Ok(mut guard) = PENDING.lock() {
                                    *guard = None;
                                }
                                return;
                            }
                            Err(reason) => {
                                respond(stream, "授权失败", &reason);
                                if let Ok(mut guard) = PENDING.lock() {
                                    *guard = None;
                                }
                                return;
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(120));
                    }
                    Err(_) => {
                        if let Ok(mut guard) = PENDING.lock() {
                            *guard = None;
                        }
                        return;
                    }
                }
            }
        })
        .ok();
    match open_browser(&url) {
        Ok(()) => serde_json::json!({ "ok": true, "error": "", "url": url }).to_string(),
        Err(reason) => serde_json::json!({ "ok": false, "error": reason, "url": url }).to_string(),
    }
}

pub fn disconnect_json(root: &Path) -> String {
    let mut config = load_config(root);
    config.access_token.clear();
    config.refresh_token.clear();
    config.expires_at = 0;
    config.display_name.clear();
    config.avatar.clear();
    match save_config(root, &config) {
        Ok(()) => serde_json::json!({ "ok": true, "error": "" }).to_string(),
        Err(reason) => serde_json::json!({ "ok": false, "error": reason }).to_string(),
    }
}

// ── 「播放在哪」自动就位（v1 `music_autoconfigure` / `spotify_desktop_running`） ──

/// Spotify 桌面端的映像名（v1 `SPOTIFY_EXE` 同款）。
const SPOTIFY_EXE: [&str; 1] = ["Spotify.exe"];

/// 本机所有进程的映像名（手写 kernel32 FFI，与 v1 `win_proc.rs` 的 `list()` 同款）。
///
/// **为什么不用 `windows` crate**：新增 `Win32_System_Diagnostics_ToolHelp` feature 会让
/// `windows` 重新编译，而本机是 `x86_64-pc-windows-gnu` 且没有 `dlltool.exe`
/// （实测 `error calling dlltool 'dlltool.exe': program not found`）⇒ 只能直接声明
/// 已经链接好的 kernel32 符号。读不到就当空表（调用方只关心「有没有」）。
#[cfg(windows)]
fn process_names() -> Vec<String> {
    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const INVALID_HANDLE_VALUE: isize = -1;
    /// `MAX_PATH`。`szExeFile` 是定长数组，进程映像名（不含路径）不可能超过它。
    const MAX_PATH: usize = 260;

    #[repr(C)]
    #[allow(non_snake_case)]
    struct PROCESSENTRY32W {
        dwSize: u32,
        cntUsage: u32,
        th32ProcessID: u32,
        th32DefaultHeapID: usize,
        th32ModuleID: u32,
        cntThreads: u32,
        th32ParentProcessID: u32,
        pcPriClassBase: i32,
        dwFlags: u32,
        szExeFile: [u16; MAX_PATH],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> isize;
        fn Process32FirstW(snapshot: isize, entry: *mut PROCESSENTRY32W) -> i32;
        fn Process32NextW(snapshot: isize, entry: *mut PROCESSENTRY32W) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    let mut names = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == 0 || snapshot == INVALID_HANDLE_VALUE {
            return names;
        }
        // `dwSize` 必须在第一次调用前填好，否则 Process32FirstW 直接失败。
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(MAX_PATH);
                names.push(String::from_utf16_lossy(&entry.szExeFile[..len]));
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    names
}

/// 本机是否在跑 Spotify 桌面客户端（非 Windows 恒 `false`，与 v1 同口径）。
pub fn spotify_desktop_running() -> bool {
    #[cfg(windows)]
    {
        process_names()
            .iter()
            .any(|name| SPOTIFY_EXE.iter().any(|exe| name.eq_ignore_ascii_case(exe)))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn device_field(item: &serde_json::Value, key: &str) -> String {
    item.get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

/// 设备是否活跃：原始 API 用 `is_active`，面板整理后的形状用 `active`，两种都认。
fn is_active_device(item: &serde_json::Value) -> bool {
    item.get("is_active")
        .and_then(|value| value.as_bool())
        .or_else(|| item.get("active").and_then(|value| value.as_bool()))
        .unwrap_or(false)
}

/// 「播放在哪」的纯决策（单测直接跑它）。
///
/// 本机播放（librespot）已按 2026-11-19 的裁决移除，所以这里只处理官方设备：
///   ① 已有活跃设备 ⇒ 认它（那是用户自己的现状，不去抢）；
///   ② 没有活跃设备但有设备 ⇒ 转移过去（`needsTransfer`，**不放歌**）；
///   ③ 桌面端进程在跑但没注册成设备 ⇒ 只回报，别去抢；
///   ④ 一台都没有 ⇒ `none` + 一句可操作说明。
pub fn decide_autoconfig(
    devices: &[serde_json::Value],
    desktop_running: bool,
) -> serde_json::Value {
    if let Some(active) = devices.iter().find(|item| is_active_device(item)) {
        return serde_json::json!({
            "source": "official",
            "deviceId": device_field(active, "id"),
            "deviceName": device_field(active, "name"),
            "deviceKind": device_field(active, "type"),
            "desktopRunning": desktop_running,
            "needsTransfer": false,
            "note": "",
        });
    }
    if let Some(device) = devices.first() {
        return serde_json::json!({
            "source": "official",
            "deviceId": device_field(device, "id"),
            "deviceName": device_field(device, "name"),
            "deviceKind": device_field(device, "type"),
            "desktopRunning": desktop_running,
            "needsTransfer": true,
            "note": "已切到可用设备（不会自动放歌）",
        });
    }
    if desktop_running {
        return serde_json::json!({
            "source": "official",
            "deviceId": "",
            "deviceName": "Spotify",
            "deviceKind": "Computer",
            "desktopRunning": true,
            "needsTransfer": false,
            "note": "Spotify 桌面端在运行，但还没出现在设备列表里：先用它播一首，或点「刷新设备」",
        });
    }
    serde_json::json!({
        "source": "none",
        "deviceId": "",
        "deviceName": "",
        "deviceKind": "",
        "desktopRunning": false,
        "needsTransfer": false,
        "note": "没有可用播放设备：先打开 Spotify 桌面端或手机端，再点「刷新设备」。",
    })
}

/// 通用调用：`{action, ...}` → `{ok, error, data}`。
///
/// `action` 清单见本文件顶部与 docs/todo.md；不认识的 action **明确报错**而不是静默成功。
pub fn call_json(root: &Path, payload: &str) -> String {
    let value: serde_json::Value = match serde_json::from_str(payload) {
        Ok(value) => value,
        Err(e) => {
            return serde_json::json!({ "ok": false, "error": format!("入参不是合法 JSON：{e}") })
                .to_string()
        }
    };
    let action = value
        .get("action")
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .to_string();
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|item| item.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let result: Result<serde_json::Value, String> = match action.as_str() {
        "status" => api_get_json(root, &format!("{SPOTIFY_API}/me/player")),
        "playlists" => api_get_json(root, &format!("{SPOTIFY_API}/me/playlists?limit=50")).map(
            |value| serde_json::json!({ "items": items_of(&value).iter().map(playlist_json).collect::<Vec<_>>() }),
        ),
        "playlist_tracks" => {
            let id = text("id");
            item_tracks_url("playlist", &id).and_then(|url| {
                api_get_json(root, &url)
                    .map(|value| serde_json::json!({ "items": tracks_from(&value, "items") }))
            })
        }
        // v1 的「你的音乐库」五栏 + 主区一列曲目（2026-11-15 补）。
        "library" => {
            let kind = text("kind");
            let (url, pointer) = match kind.as_str() {
                "artists" => (
                    format!("{SPOTIFY_API}/me/following?type=artist&limit=50"),
                    "/artists/items",
                ),
                "albums" => (format!("{SPOTIFY_API}/me/albums?limit=50"), "/items"),
                "shows" => (format!("{SPOTIFY_API}/me/shows?limit=50"), "/items"),
                "top_artists" => (
                    format!("{SPOTIFY_API}/me/top/artists?limit=50&time_range=short_term"),
                    "/items",
                ),
                "top_tracks" => (
                    format!("{SPOTIFY_API}/me/top/tracks?limit=50&time_range=short_term"),
                    "/items",
                ),
                other => {
                    return serde_json::json!({
                        "ok": false,
                        "error": format!(
                            "未知的库类型：{other}（只支持 artists / albums / shows / top_artists / top_tracks）"
                        ),
                        "data": null,
                    })
                    .to_string()
                }
            };
            let item_kind = if kind == "albums" {
                "album"
            } else if kind == "shows" {
                "show"
            } else {
                "artist"
            };
            api_get_json(root, &url).map(|value| {
                if kind == "top_tracks" {
                    serde_json::json!({ "items": [], "tracks": tracks_from(&value, "items") })
                } else {
                    serde_json::json!({
                        "items": library_items(&value, pointer, item_kind),
                        "tracks": [],
                    })
                }
            })
        }
        "item_tracks" => {
            let kind = text("kind");
            let id = text("id");
            item_tracks_url(&kind, &id).and_then(|url| {
                api_get_json(root, &url).map(|value| {
                    // 歌手的热门曲挂在 `tracks`，其余在 `items`。
                    let key = if kind == "artist" { "tracks" } else { "items" };
                    serde_json::json!({ "items": tracks_from(&value, key) })
                })
            })
        }
        "queue" => api_get_json(root, &format!("{SPOTIFY_API}/me/player/queue")).map(|value| {
            serde_json::json!({
                "current": value.get("currently_playing").map(track_json),
                "items": items_of(&value).iter().map(track_json).collect::<Vec<_>>(),
            })
        }),
        "liked" => api_get_json(
            root,
            &format!("{SPOTIFY_API}/me/tracks?limit={LIKED_LIMIT}"),
        )
        .map(|value| {
            serde_json::json!({
                // 条目包装可能是 `track` 或新形状 `item`；不可播的一律滤掉。
                "items": tracks_from(&value, "items"),
            })
        }),
        "devices" => api_get_json(root, &format!("{SPOTIFY_API}/me/player/devices")).map(
            |value| {
                serde_json::json!({
                    "items": value
                        .get("devices")
                        .and_then(|items| items.as_array())
                        .map(|list| list
                            .iter()
                            .map(|item| serde_json::json!({
                                "id": item.get("id").and_then(|v| v.as_str()).unwrap_or_default(),
                                "name": item.get("name").and_then(|v| v.as_str()).unwrap_or_default(),
                                "type": item.get("type").and_then(|v| v.as_str()).unwrap_or_default(),
                                "active": item.get("is_active").and_then(|v| v.as_bool()).unwrap_or(false),
                                "volume": item.get("volume_percent").and_then(|v| v.as_i64()).unwrap_or(0),
                            }))
                            .collect::<Vec<_>>())
                        .unwrap_or_default(),
                })
            },
        ),
        // v1 `music_autoconfigure` 的 v2 等价物（2026-11-15）：把「播放在哪」定下来。
        // 本机播放（librespot）已按 2026-11-19 的裁决移除 ⇒ 只认官方设备：
        // ① 有活跃设备就认它；② 否则转移到第一台设备（只转移、不放歌）；
        // ③ 桌面端进程在跑但没注册 ⇒ 只如实回报；④ 一台都没有 ⇒ `none` + 一句可操作说明。
        "autoconfigure" => {
            let desktop = spotify_desktop_running();
            api_get_json(root, &format!("{SPOTIFY_API}/me/player/devices")).map(|value| {
                let devices: Vec<serde_json::Value> = value
                    .get("devices")
                    .and_then(|items| items.as_array())
                    .cloned()
                    .unwrap_or_default();
                let decision = decide_autoconfig(&devices, desktop);
                if decision
                    .get("needsTransfer")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    let id = decision
                        .get("deviceId")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    if !id.is_empty() {
                        // 只转移、不放歌（v1 同口径：不要覆盖用户正在听 / 已暂停的东西）。
                        let _ = api_send(
                            root,
                            "PUT",
                            &format!("{SPOTIFY_API}/me/player"),
                            Some(serde_json::json!({ "device_ids": [id], "play": false })),
                        );
                    }
                }
                decision
            })
        }
        "search" => {
            let query = text("q");
            if query.trim().is_empty() {
                Err("搜索关键词是空的".to_string())
            } else {
                api_get_json(
                    root,
                    &format!(
                        "{SPOTIFY_API}/search?type=track,playlist&limit=10&q={}",
                        url_encode(query.trim())
                    ),
                )
                .map(|value| {
                    serde_json::json!({
                        "tracks": items_of(value.pointer("/tracks").unwrap_or(&serde_json::json!({})))
                            .iter()
                            .map(track_json)
                            .collect::<Vec<_>>(),
                        "playlists": items_of(value.pointer("/playlists").unwrap_or(&serde_json::json!({})))
                            .iter()
                            .map(playlist_json)
                            .collect::<Vec<_>>(),
                    })
                })
            }
        }
        "control" => {
            // v1 `spotify_control`：play / pause / next / previous / seek / volume。
            let what = text("what");
            let body = match what.as_str() {
                "seek" => serde_json::json!({
                    "position_ms": value.get("value").and_then(|v| v.as_i64()).unwrap_or(0)
                }),
                "volume" => serde_json::json!({
                    "volume_percent": value.get("value").and_then(|v| v.as_i64()).unwrap_or(50).clamp(0, 100)
                }),
                _ => serde_json::json!({}),
            };
            let url = match what.as_str() {
                "pause" => format!("{SPOTIFY_API}/me/player/pause"),
                "next" => format!("{SPOTIFY_API}/me/player/next"),
                "previous" => format!("{SPOTIFY_API}/me/player/previous"),
                "seek" => format!("{SPOTIFY_API}/me/player/seek"),
                "volume" => format!("{SPOTIFY_API}/me/player/volume"),
                "play" => format!("{SPOTIFY_API}/me/player/play"),
                other => return serde_json::json!({
                    "ok": false,
                    "error": format!("未知的播放控制：{other}（只支持 play / pause / next / previous / seek / volume）"),
                })
                .to_string(),
            };
            api_send(root, "PUT", &url, Some(body)).map(|_| serde_json::json!({}))
        }
        // v1 `spotify_set_play_mode`：三态 → shuffle + repeat 两个字段。
        "play_mode" => {
            let mode = text("mode");
            play_mode_states(&mode).and_then(|(shuffle, repeat)| {
                api_send(
                    root,
                    "PUT",
                    &format!("{SPOTIFY_API}/me/player/shuffle?state={shuffle}"),
                    None,
                )?;
                api_send(
                    root,
                    "PUT",
                    &format!("{SPOTIFY_API}/me/player/repeat?state={repeat}"),
                    None,
                )
                .map(|_| serde_json::json!({}))
            })
        }
        "play" => {
            // 三种形态见 `play_body`：`{uris}` / `{uri}` / `{context, offset?}`。
            let body = match play_body(&value) {
                Ok(body) => body,
                Err(reason) => {
                    return serde_json::json!({ "ok": false, "error": reason }).to_string()
                }
            };
            let device = text("device");
            let mut url = format!("{SPOTIFY_API}/me/player/play");
            if !device.is_empty() {
                url.push_str(&format!("?device_id={}", url_encode(&device)));
            }
            api_send(root, "PUT", &url, Some(body)).map(|_| serde_json::json!({}))
        }
        "transfer" => {
            let device = text("device");
            if device.trim().is_empty() {
                Err("缺 device（先点「刷新设备」）".to_string())
            } else {
                api_send(
                    root,
                    "PUT",
                    &format!("{SPOTIFY_API}/me/player"),
                    Some(serde_json::json!({
                        "device_ids": [device],
                        "play": value.get("play").and_then(|v| v.as_bool()).unwrap_or(true),
                    })),
                )
                .map(|_| serde_json::json!({}))
            }
        }
        "resume" => {
            clear_stop();
            Ok(serde_json::json!({}))
        }
        other => {
            return serde_json::json!({
                "ok": false,
                "error": format!("未知的 Spotify 操作：{other}"),
            })
            .to_string()
        }
    };
    match result {
        Ok(data) => serde_json::json!({ "ok": true, "error": "", "data": data }).to_string(),
        Err(reason) => serde_json::json!({ "ok": false, "error": reason, "data": null }).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-music-spotify-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `play` 的三种形态（v1 的三条命令）：uris / 单曲 / 上下文 + offset。
    /// `offset` 只认单曲 URI，非法时**静默忽略**（v1 `filter(is_safe_track_uri)` 同口径）。
    #[test]
    fn play_body_matches_the_three_v1_commands() {
        // v1 `spotify_play_uris`
        let body = play_body(&serde_json::json!({
            "uris": ["spotify:track:a1", "spotify:track:b2"]
        }))
        .unwrap();
        assert_eq!(body, serde_json::json!({ "uris": ["spotify:track:a1", "spotify:track:b2"] }));

        // v1 `spotify_play_uri`：单曲 → `{uris:[uri]}`
        let body = play_body(&serde_json::json!({ "uri": "spotify:track:c3" })).unwrap();
        assert_eq!(body, serde_json::json!({ "uris": ["spotify:track:c3"] }));

        // v1 `spotify_play_context`：带上单曲 offset
        let body = play_body(&serde_json::json!({
            "context": "spotify:playlist:p1",
            "offset": "spotify:track:d4"
        }))
        .unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "context_uri": "spotify:playlist:p1",
                "offset": { "uri": "spotify:track:d4" }
            })
        );

        // offset 不是单曲（专辑 / 播客 / 乱七八糟）⇒ 忽略 offset，但上下文照播
        for bad in ["spotify:album:x", "spotify:episode:y", "not-a-uri"] {
            let body = play_body(&serde_json::json!({
                "context": "spotify:album:a9",
                "offset": bad
            }))
            .unwrap();
            assert_eq!(body, serde_json::json!({ "context_uri": "spotify:album:a9" }), "{bad}");
        }

        // 非法 URI 必须如实报错（不能把坏 URI 发给端点）
        assert!(play_body(&serde_json::json!({ "uris": ["bad"] })).is_err());
        assert!(play_body(&serde_json::json!({ "context": "bad" })).is_err());
        assert!(play_body(&serde_json::json!({ "uri": "bad" })).is_err());

        // 什么都没给 ⇒ 空体（等价「继续播当前上下文」）
        assert_eq!(play_body(&serde_json::json!({})).unwrap(), serde_json::json!({}));
    }

    /// 「播放在哪」三条规则（v1 `music_autoconfigure` ①②③ + 没有设备时如实说明）。
    #[test]
    fn autoconfig_prefers_the_active_device() {
        let devices = vec![
            serde_json::json!({"id": "d1", "name": "我的手机", "type": "Smartphone", "is_active": false}),
            serde_json::json!({"id": "d2", "name": "我的电脑", "type": "Computer", "is_active": true}),
        ];
        let decision = decide_autoconfig(&devices, true);
        assert_eq!(decision["source"], "official");
        assert_eq!(decision["deviceId"], "d2");
        assert_eq!(decision["needsTransfer"], false);
        assert_eq!(decision["desktopRunning"], true);
    }

    #[test]
    fn autoconfig_transfers_to_the_first_device_without_playing() {
        let devices = vec![
            serde_json::json!({"id": "d1", "name": "音箱", "type": "Speaker", "is_active": false}),
            serde_json::json!({"id": "d2", "name": "桌面端", "type": "Computer", "is_active": false}),
        ];
        let decision = decide_autoconfig(&devices, true);
        assert_eq!(decision["source"], "official");
        assert_eq!(decision["deviceId"], "d1");
        assert_eq!(decision["needsTransfer"], true, "必须转移，否则播放会 404 NO_ACTIVE_DEVICE");
    }

    #[test]
    fn autoconfig_reports_desktop_running_without_a_registered_device() {
        let decision = decide_autoconfig(&[], true);
        assert_eq!(decision["source"], "official");
        assert_eq!(decision["desktopRunning"], true);
        assert_eq!(decision["needsTransfer"], false);
        assert!(decision["note"].as_str().unwrap().contains("设备列表"));
    }

    #[test]
    fn autoconfig_says_no_device_honestly_instead_of_pretending() {
        let decision = decide_autoconfig(&[], false);
        assert_eq!(decision["source"], "none");
        assert_eq!(decision["desktopRunning"], false);
        assert!(
            decision["note"].as_str().unwrap().contains("刷新设备"),
            "没有设备时必须给一句可操作说明，不能假装就绪"
        );
    }

    #[test]
    fn pkce_challenge_matches_rfc7636_vector() {
        // RFC 7636 附录 B 的测试向量。
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            code_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifier_is_long_enough_and_url_safe() {
        let verifier = random_verifier();
        assert_eq!(verifier.len(), 64);
        assert!(verifier
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "-._~".contains(ch)));
        // 两次调用必须不同（否则 PKCE 就退化成常量了）。
        assert_ne!(verifier, random_verifier());
    }

    #[test]
    fn authorize_url_carries_pkce_and_redirect() {
        let config = SpotifyConfig::default();
        let url = authorize_url(&config, "verifier-abc", "state-xyz");
        assert!(url.starts_with("https://accounts.spotify.com/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("code_challenge={}", code_challenge("verifier-abc"))));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8899%2Fcallback"));
        assert!(url.contains("client_id=590875b9724848a284198bfe1bd85588"));
        assert!(url.contains("state=state-xyz"));
    }

    #[test]
    fn callback_target_and_query_are_parsed() {
        let line = "GET /callback?code=AQD123&state=abc HTTP/1.1";
        let target = parse_request_target(line).unwrap();
        assert_eq!(target, "/callback?code=AQD123&state=abc");
        assert_eq!(query_param(&target, "code").as_deref(), Some("AQD123"));
        assert_eq!(query_param(&target, "state").as_deref(), Some("abc"));
        assert_eq!(query_param(&target, "missing"), None);
        assert_eq!(query_param("no-query", "code"), None);
        assert_eq!(parse_request_target("garbage"), None);
    }

    #[test]
    fn safe_ids_and_uris_reject_paths_and_scripts() {
        assert!(is_safe_id("4uLU6hMCjMI75M1A2tKUQC"));
        assert!(!is_safe_id("../../etc/passwd"));
        assert!(!is_safe_id("abc?x=1"));
        assert!(!is_safe_id(""));
        assert!(is_safe_uri("spotify:track:4uLU6hMCjMI75M1A2tKUQC"));
        assert!(is_safe_uri("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M"));
        assert!(!is_safe_uri("spotify:track:../x"));
        assert!(!is_safe_uri("http://evil.example"));
        assert!(!is_safe_uri("spotify:user:abc"));
    }

    #[test]
    fn track_and_playlist_json_map_the_fields_the_panel_draws() {
        let track = serde_json::json!({
            "id": "t1",
            "uri": "spotify:track:t1",
            "name": "Creep",
            "artists": [ { "name": "Radiohead" }, { "name": "Guest" } ],
            "album": { "name": "Pablo Honey", "images": [ { "url": "https://img/1.jpg" } ] },
            "duration_ms": 238000,
            "progress_ms": 12000,
            "is_playing": true,
        });
        let mapped = track_json(&track);
        assert_eq!(mapped["name"], "Creep");
        assert_eq!(mapped["artists"], "Radiohead, Guest");
        assert_eq!(mapped["album"], "Pablo Honey");
        assert_eq!(mapped["cover"], "https://img/1.jpg");
        assert_eq!(mapped["durationMs"], 238000);
        assert_eq!(mapped["isPlaying"], true);

        let playlist = serde_json::json!({
            "id": "p1",
            "uri": "spotify:playlist:p1",
            "name": "通勤",
            "images": [ { "url": "https://img/p.jpg" } ],
            "tracks": { "total": 42 },
            "owner": { "display_name": "me" },
        });
        let mapped = playlist_json(&playlist);
        assert_eq!(mapped["name"], "通勤");
        assert_eq!(mapped["tracks"], 42);
        assert_eq!(mapped["owner"], "me");
        assert_eq!(mapped["cover"], "https://img/p.jpg");
    }

    #[test]
    fn config_round_trip_keeps_unknown_keys() {
        let root = temp_root("config");
        let path = config_path(&root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // v1 写下的文件里有我们不认识的键，落盘时必须原样保留。
        std::fs::write(
            &path,
            r#"{"client_id":"abc","port":8898,"access_token":"t","refresh_token":"r",
                "expires_at":123,"display_name":"me","avatar":"a",
                "v1_extra":"keep-me","v1_extra_bitrate":160}"#,
        )
        .unwrap();
        let mut config = load_config(&root);
        assert_eq!(config.client_id, "abc");
        assert_eq!(config.effective_port(), 8898);
        assert!(config.has_token());
        config.display_name = "改名".to_string();
        save_config(&root, &config).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"v1_extra\": \"keep-me\""));
        assert!(text.contains("\"display_name\": \"改名\""));
        assert!(text.contains("\"v1_extra_bitrate\": 160"));
    }

    #[test]
    fn missing_or_zero_port_falls_back_to_default() {
        let config = SpotifyConfig {
            port: 0,
            ..SpotifyConfig::default()
        };
        assert_eq!(config.effective_port(), DEFAULT_PORT);
        assert_eq!(config.redirect_uri(), format!("http://127.0.0.1:{DEFAULT_PORT}/callback"));
        let empty = SpotifyConfig {
            client_id: "  ".to_string(),
            ..SpotifyConfig::default()
        };
        assert_eq!(empty.client_id(), BUILTIN_CLIENT_ID);
    }

    #[test]
    fn rate_limit_gate_blocks_then_expires() {
        clear_stop();
        assert_eq!(retry_remaining_secs(), 0);
        trip_stop("测试", 2);
        assert!(retry_remaining_secs() > 0);
        let state: serde_json::Value = serde_json::from_str(&stop_json().to_string()).unwrap();
        assert_eq!(state["active"], true);
        assert_eq!(state["reason"], "测试");
        clear_stop();
        assert_eq!(retry_remaining_secs(), 0);
        // `resume` 走的是 clear_stop。
        let reply: serde_json::Value =
            serde_json::from_str(&call_json(&temp_root("resume"), r#"{"action":"resume"}"#))
                .unwrap();
        assert_eq!(reply["ok"], true);
    }

    #[test]
    fn unknown_action_is_reported_not_silently_ok() {
        let root = temp_root("unknown");
        let reply: serde_json::Value =
            serde_json::from_str(&call_json(&root, r#"{"action":"teleport"}"#)).unwrap();
        assert_eq!(reply["ok"], false);
        assert!(reply["error"].as_str().unwrap_or_default().contains("teleport"));
    }

    #[test]
    fn connect_without_free_port_reports_why() {
        let root = temp_root("connect");
        // 先把配置里的端口占住：回调必须能绑上，否则就地报原因（不留一个必然失败的后台线程）。
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            config_path(&root),
            format!(r#"{{"client_id":"abc","port":{port}}}"#),
        )
        .unwrap();
        let reply: serde_json::Value = serde_json::from_str(&connect_json(&root)).unwrap();
        assert_eq!(reply["ok"], false);
        assert!(reply["error"].as_str().unwrap_or_default().contains("被占用"));
        drop(listener);
    }

    #[test]
    fn library_items_unwrap_me_wrappers_and_skip_broken_entries() {
        let value = serde_json::json!({
            "items": [
                {"added_at":"x","album":{"id":"alb1","name":"A","artists":[{"name":"X"},{"name":"Y"}],
                                          "images":[{"url":"https://img/1"}],"total_tracks":9}},
                {"added_at":"x","album":null},
                {"added_at":"x","album":{"name":"没有 id"}}
            ]
        });
        let items = library_items(&value, "/items", "album");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "alb1");
        assert_eq!(items[0]["uri"], "spotify:album:alb1");
        assert_eq!(items[0]["subtitle"], "X, Y");
        assert_eq!(items[0]["total"], 9);
        assert_eq!(items[0]["cover"], "https://img/1");

        // 关注歌手挂在 `/artists/items`，且是裸对象（不再包一层）。
        let followed = serde_json::json!({
            "artists": {"items": [{"id":"art1","name":"歌手","genres":["pop"]}]}
        });
        let artists = library_items(&followed, "/artists/items", "artist");
        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0]["uri"], "spotify:artist:art1");
        assert_eq!(artists[0]["subtitle"], "pop");

        // 播客没有可播的 context_uri（与 v1 同口径：故意留空）。
        let shows = serde_json::json!({
            "items": [{"added_at":"x","show":{"id":"show1","name":"播客","publisher":"P","total_episodes":3}}]
        });
        let list = library_items(&shows, "/items", "show");
        assert_eq!(list[0]["uri"], "");
        assert_eq!(list[0]["subtitle"], "P");
        assert_eq!(list[0]["total"], 3);
    }

    #[test]
    fn item_tracks_endpoints_match_v1_and_are_whitelisted() {
        // 歌单走 `/items`（老的 `/tracks` 现在 403 —— v1 2026-09 实测）。
        assert!(item_tracks_url("playlist", "p1")
            .unwrap()
            .contains("/playlists/p1/items"));
        assert!(item_tracks_url("album", "a1")
            .unwrap()
            .contains("/albums/a1/tracks"));
        assert!(item_tracks_url("artist", "r1")
            .unwrap()
            .contains("/artists/r1/top-tracks?market=from_token"));
        assert!(item_tracks_url("show", "s1")
            .unwrap()
            .contains("/shows/s1/episodes"));
        // 非白名单 / 越界 id 一律拒绝（kind 拼进 URL 路径）。
        assert!(item_tracks_url("episode", "x").is_err());
        assert!(item_tracks_url("playlist", "../x").is_err());
    }

    #[test]
    fn tracks_from_accepts_both_wrappers_and_drops_unplayable_items() {
        let value = serde_json::json!({
            "items": [
                {"added_at":"x","item":{"id":"t1","uri":"spotify:track:t1","name":"一",
                                        "artists":[{"name":"A"}],"duration_ms":1000,
                                        "album":{"name":"Al","images":[{"url":"u"}]}}},
                {"added_at":"x","track":{"id":"t2","uri":"spotify:track:t2","name":"二",
                                         "artists":[{"name":"B"}],"duration_ms":2000,
                                         "album":{"name":"Al"}}},
                // 本机文件 / 下架：没有可播 uri ⇒ 滤掉。
                {"added_at":"x","track":{"id":"t3","uri":"","name":"本地"}},
                {"added_at":"x","track":null}
            ]
        });
        let items = tracks_from(&value, "items");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], "t1");
        assert_eq!(items[1]["id"], "t2");
    }

    #[test]
    fn play_mode_maps_three_states_like_v1() {
        assert_eq!(play_mode_states("off").unwrap(), ("false", "off"));
        assert_eq!(play_mode_states("shuffle").unwrap(), ("true", "off"));
        assert_eq!(play_mode_states("repeat_one").unwrap(), ("false", "track"));
        assert!(play_mode_states("smart").is_err());
    }

    /// 面板保存 Client ID / 端口：换值必须清令牌（否则面板显示「已连接」但请求全 401）。
    #[test]
    fn save_settings_clears_tokens_only_when_identity_changes() {
        let root = temp_root("config-save");
        let mut config = SpotifyConfig {
            client_id: "my-own-client".to_string(),
            port: 8899,
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 999,
            display_name: "someone".into(),
            avatar: "https://x/a.png".into(),
            ..Default::default()
        };
        save_config(&root, &config).expect("save");

        // 同值：令牌保留
        save_settings(&root, Some("my-own-client"), Some(8899)).expect("same");
        let after = load_config(&root);
        assert_eq!(after.access_token, "a");
        assert_eq!(after.display_name, "someone");

        // 只改端口：令牌清空、其余字段保留
        save_settings(&root, None, Some(9000)).expect("port");
        let after = load_config(&root);
        assert!(after.access_token.is_empty());
        assert!(after.refresh_token.is_empty());
        assert_eq!(after.expires_at, 0);
        assert!(after.display_name.is_empty());
        assert_eq!(after.effective_port(), 9000);
        assert_eq!(after.client_id, "my-own-client", "没传的字段不许被动");

        // 非法端口 / 空 Client ID：一个字都不写
        config.access_token = "kept".into();
        save_config(&root, &config).expect("save2");
        assert!(save_settings(&root, None, Some(80)).is_err());
        assert!(save_settings(&root, Some("   "), None).is_err());
        assert_eq!(load_config(&root).access_token, "kept");

        // 不认识的键（v1 写下的旧配置）原样保留
        let path = config_path(&root);
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        value["v1_unknown_key"] = serde_json::json!("keep-me");
        std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
        save_settings(&root, Some("another-client"), Some(9000)).expect("client");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("v1_unknown_key"), "未知键不能被抹掉：{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 空 `client_id` 与显式内置值都算 `builtin`（面板据此收起凭据字段）。
    #[test]
    fn state_reports_builtin_client_id() {
        let root = temp_root("builtin");
        let state: serde_json::Value = serde_json::from_str(&state_json(&root)).unwrap();
        assert_eq!(state["builtin"], serde_json::json!(true));
        assert_eq!(state["clientId"], serde_json::json!(BUILTIN_CLIENT_ID));
        assert_eq!(state["configured"], serde_json::json!(true));

        let mut config = load_config(&root);
        config.client_id = BUILTIN_CLIENT_ID.to_string();
        save_config(&root, &config).unwrap();
        let state: serde_json::Value = serde_json::from_str(&state_json(&root)).unwrap();
        assert_eq!(state["builtin"], serde_json::json!(true));

        config.client_id = "my-own-client".into();
        save_config(&root, &config).unwrap();
        let state: serde_json::Value = serde_json::from_str(&state_json(&root)).unwrap();
        assert_eq!(state["builtin"], serde_json::json!(false));
        assert_eq!(state["clientId"], serde_json::json!("my-own-client"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
