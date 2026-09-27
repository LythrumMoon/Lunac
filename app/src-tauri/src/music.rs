// src-tauri/src/music.rs
// 音乐插件（歌词 + Spotify 播放控制）的宿主命令 —— 2026-09-27
//
// **为什么这件事必须由宿主做**：前端插件跑在 WebView 里，`tauri.conf.json` 的 CSP 是
// `default-src 'self' https://asset.localhost` —— 不含任何远端域，插件里的 `fetch()`
// 会被直接拦掉（与「插件市场索引必须由宿主去拉」同一条理由，见 ai-spec §11 规则 67 的第 ⑥ 条）。
// 所以凡是联网的动作都只能落在这里。
//
// 两条能力：
//   ① **Spotify 播放控制** —— 官方只提供 OAuth 2.0（Authorization Code + PKCE，桌面端走
//      环回地址，RFC 8252），**没有**「账号密码异地登入」这类接口。播放控制另需 **Premium**
//      账号 + `user-modify-playback-state`；读状态需 `user-read-playback-state` /
//      `user-read-currently-playing`。控制的永远是「当前活跃的 Spotify Connect 设备」。
//   ② **歌词抓取** —— LRCLIB（免费、无需 key；要求带 `User-Agent`，并遵守 429 的
//      `Retry-After`）。返回 `syncedLyrics` 是 `[mm:ss.xx]` 逐行的 LRC。
//
// **阻塞纪律（code-rules 预检 #35 / §4.6）**：本模块每个命令体里都有 `reqwest::blocking`
// 或监听等待 ⇒ 一律走 `async` 薄壳 + `run_blocking`，绝不留在主线程上冻窗口。
// 唯一的例外是 `spotify_connect`：它**只 bind 端口然后立刻返回**（把 accept + 换 token
// 交给独立线程），所以命令体里没有等待 —— 这一点写在它自己的注释里。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

use crate::commands::run_blocking;

// ── 常量 ──────────────────────────────────────────────────────────

const SPOTIFY_AUTHORIZE: &str = "https://accounts.spotify.com/authorize";
const SPOTIFY_TOKEN: &str = "https://accounts.spotify.com/api/token";
const SPOTIFY_API: &str = "https://api.spotify.com/v1";
const LRCLIB: &str = "https://lrclib.net/api";

/// 需要的授权面：读播放态 + 改播放态 + 读当前曲目。
const SCOPES: &str = "user-read-playback-state user-modify-playback-state user-read-currently-playing";

/// 环回回调的默认端口。**固定端口**：Spotify 的 Redirect URI 要求逐字符精确匹配
/// （官方唯一豁免是「环回 IP 字面量可动态分配端口」，但仪表盘里注册的是一个具体字符串，
/// 固定端口最不容易出错），所以面板上会把完整回调地址显示出来给用户照抄。
/// 8899 是刻意避开的：8788/8789 是 Lunac 自己的本地服务端口，9222 是 WebView2 调试口。
const DEFAULT_PORT: u16 = 8899;

/// LRCLIB 明确要求带 `User-Agent`（不带会被拒）。
const UA: &str = concat!(
    "Lunac/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/LythrumMoon/Lunac)"
);

/// URL query 的编码集：**RFC 3986 的 `unreserved`（`A-Za-z0-9-._~`）原样保留**，其余编码。
/// 不要用 `NON_ALPHANUMERIC` —— 它会把 `-` 编成 `%2D`、`.` 编成 `%2E`，虽然解码后等价
/// （Spotify 的 redirect_uri 比对也是比**解码后**的值），但生成的 URL 没法肉眼核对，
/// 排查 OAuth 时很吃亏。
const QUERY_ENC: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

fn qenc(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, QUERY_ENC).to_string()
}

fn http() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))
}

// ── 配置（<exe 根>\config\music.json）─────────────────────────────
//
// 与 `ai.json` 同级、同口径：**应用配置**落 `config\`，业务数据才进 `ModuleData`。
// 令牌与 client_id 一起放在这里（本机文件，删除本文件即等于「忘记账号」）。

fn music_config_path() -> PathBuf {
    crate::storage::lunac_root_dir().join("config").join("music.json")
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MusicConfig {
    #[serde(default)]
    pub client_id: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// access_token 的到期时刻（unix 秒）。0 = 未知（当作已过期，会先刷一次）。
    #[serde(default)]
    pub expires_at: i64,
    /// 授权后抓到一次的账号名，仅用于面板上显示「已连接为 xxx」。
    #[serde(default)]
    pub display_name: String,
}

impl Default for MusicConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            port: DEFAULT_PORT,
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: 0,
            display_name: String::new(),
        }
    }
}

impl MusicConfig {
    fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.effective_port())
    }
    /// 端口兜底：文件被手工改过（0 / 空）时回落到默认端口，别让回调地址变成 `:0`。
    fn effective_port(&self) -> u16 {
        if self.port == 0 {
            DEFAULT_PORT
        } else {
            self.port
        }
    }
    fn has_token(&self) -> bool {
        !self.access_token.is_empty() || !self.refresh_token.is_empty()
    }
}

fn load_config() -> MusicConfig {
    match std::fs::read_to_string(music_config_path()) {
        Ok(text) => serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default(),
        Err(_) => MusicConfig::default(),
    }
}

/// 落盘。**凭据类文件写失败必须如实报错** —— 否则会重演「面板看着保存成功、
/// 重启后登录状态没了」这种最难查的问题（与 `set_ai_config` 同一条纪律）。
fn save_config(cfg: &MusicConfig) -> Result<(), String> {
    let path = music_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("写不进 {}：{e}", path.display()))
}

/// 面板可见的配置视图。**绝不回传令牌**（前端不需要，也就没有泄露面）。
#[derive(Debug, Serialize)]
pub struct MusicConfigDto {
    pub client_id: String,
    pub port: u16,
    pub redirect_uri: String,
    pub connected: bool,
    pub display_name: String,
}

fn config_dto(cfg: &MusicConfig) -> MusicConfigDto {
    MusicConfigDto {
        client_id: cfg.client_id.clone(),
        port: cfg.effective_port(),
        redirect_uri: cfg.redirect_uri(),
        connected: cfg.has_token(),
        display_name: cfg.display_name.clone(),
    }
}

// ── PKCE ─────────────────────────────────────────────────────────

/// 生成 `code_verifier`（RFC 7636 §4.1：43–128 字符，字母数字 + `-._~`）。
///
/// **不引 `rand`**：本机依赖树里没有它，而 `std` 的 `RandomState` 是**由操作系统播种**的
/// （`HashMap` 的抗碰撞哈希种子来自 OS 熵），再混入纳秒时间戳与一个自增计数 ⇒ 每次调用
/// 都不同且不可预测。够 PKCE 用（verifier 只在本地环回流程里活几十秒），且零新依赖。
/// 不要退回「固定字符串」或「只用时间戳」——那等于把 PKCE 的防护面丢掉。
fn random_verifier() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    static CTR: AtomicU64 = AtomicU64::new(0);

    let mut out = String::with_capacity(64);
    let mut n = CTR.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    while out.len() < 64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(n);
        h.write_u128(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.write_usize(out.len());
        let v = h.finish();
        for i in 0..8 {
            if out.len() >= 64 {
                break;
            }
            out.push(ALPHABET[((v >> (i * 8)) & 0xff) as usize % ALPHABET.len()] as char);
        }
        n = n.wrapping_add(0x9E37_79B9_7F4A_7C15);
    }
    out
}

/// `code_challenge = base64url(sha256(verifier))`，无填充（S256，RFC 7636 §4.2）。
fn code_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── Spotify 令牌 ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: i64,
}

/// 拿到一个可用的 access_token：快过期（或未知）就先刷新一次。
///
/// 刷新失败按「令牌已失效」处理：清掉本地令牌并报 `ERR_NOT_CONNECTED`，让面板退回
/// 「未连接」状态 —— 这样用户看到的是「重新登录」，而不是一个反复报 401 的死面板。
fn ensure_access_token(cfg: &mut MusicConfig) -> Result<String, String> {
    if cfg.client_id.trim().is_empty() {
        return Err("ERR_NO_CLIENT_ID".into());
    }
    if !cfg.access_token.is_empty() && cfg.expires_at > now_secs() + 60 {
        return Ok(cfg.access_token.clone());
    }
    if cfg.refresh_token.is_empty() {
        return Err("ERR_NOT_CONNECTED".into());
    }
    let client = http()?;
    let resp = client
        .post(SPOTIFY_TOKEN)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", cfg.refresh_token.as_str()),
            ("client_id", cfg.client_id.trim()),
        ])
        .send()
        .map_err(|e| format!("刷新令牌失败：{e}"))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        cfg.access_token.clear();
        cfg.refresh_token.clear();
        cfg.expires_at = 0;
        let _ = save_config(cfg);
        crate::log::warn(&format!("spotify: 刷新令牌被拒（{status}）⇒ 已清空本地令牌"));
        return Err("ERR_NOT_CONNECTED".into());
    }
    let t: TokenResponse = serde_json::from_str(&text).map_err(|e| format!("令牌响应解析失败：{e}"))?;
    if t.access_token.is_empty() {
        return Err("ERR_NOT_CONNECTED".into());
    }
    cfg.access_token = t.access_token;
    // Spotify 刷新时**可能不返回**新的 refresh_token ⇒ 保留旧的（清掉就等于把用户踢下线）。
    if !t.refresh_token.is_empty() {
        cfg.refresh_token = t.refresh_token;
    }
    cfg.expires_at = now_secs() + t.expires_in.max(60);
    save_config(cfg)?;
    Ok(cfg.access_token.clone())
}

/// 带一次自动刷新的 GET：401 时刷令牌后重试一次。
fn api_get(cfg: &mut MusicConfig, url: &str) -> Result<reqwest::blocking::Response, String> {
    let token = ensure_access_token(cfg)?;
    let client = http()?;
    let resp = client
        .get(url)
        .bearer_auth(&token)
        .send()
        .map_err(|e| format!("请求 Spotify 失败：{e}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        cfg.expires_at = 0; // 强制刷新
        cfg.access_token.clear();
        let token = ensure_access_token(cfg)?;
        let client = http()?;
        return client
            .get(url)
            .bearer_auth(&token)
            .send()
            .map_err(|e| format!("请求 Spotify 失败：{e}"));
    }
    Ok(resp)
}

// ── 环回回调服务器 ────────────────────────────────────────────────

/// 从 `GET /callback?code=..&state=..` 里取出查询串。
fn parse_request_target(line: &str) -> Option<String> {
    // 形如：GET /callback?code=xxx&state=yyy HTTP/1.1
    let mut parts = line.split_whitespace();
    parts.next()?; // method
    parts.next().map(|s| s.to_string())
}

fn query_param(target: &str, key: &str) -> Option<String> {
    let q = target.split_once('?')?.1;
    for pair in q.split('&') {
        let (k, v) = match pair.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        if k == key {
            let decoded = percent_encoding::percent_decode_str(v).decode_utf8_lossy().to_string();
            return Some(decoded);
        }
    }
    None
}

fn respond(stream: &mut std::net::TcpStream, title: &str, body: &str) {
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head>\
         <body style=\"font-family:system-ui;background:#1e1e2e;color:#cdd6f4;display:flex;\
         align-items:center;justify-content:center;height:100vh;margin:0\">\
         <div style=\"text-align:center\"><h2 style=\"font-weight:600\">{title}</h2>\
         <p style=\"color:#a6adc8\">{body}</p></div></body></html>"
    );
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.as_bytes().len(),
        html
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

/// 授权结果的回报（前端 listen `spotify-auth`）。
#[derive(Debug, Serialize, Clone)]
pub struct AuthEvent {
    pub ok: bool,
    pub message: String,
}

/// 起环回服务器、等一次回调、换令牌、落盘、发事件。
///
/// 跑在**独立线程**里：accept 会一直阻塞到用户在浏览器里点完授权，绝不能占住命令调用方。
fn run_callback_server(app: AppHandle, listener: TcpListener, cfg: MusicConfig, verifier: String) {
    std::thread::spawn(move || {
        let redirect = cfg.redirect_uri();
        let (code, mut err) = match listener.accept() {
            Ok((mut stream, _)) => {
                let target = {
                    let mut reader = BufReader::new(&mut stream);
                    let mut line = String::new();
                    let _ = reader.read_line(&mut line);
                    parse_request_target(&line)
                };
                let mut code: Option<String> = None;
                let mut err: Option<String> = None;
                let mut state_ok = false;
                if let Some(target) = target.as_deref() {
                    if let Some(c) = query_param(target, "code") {
                        code = Some(c);
                    }
                    if let Some(e) = query_param(target, "error") {
                        err = Some(e);
                    }
                    // state 校验：不是我们发出去的那个就丢弃（防跨站伪造回调）。
                    state_ok = query_param(target, "state").as_deref() == Some(cfg_state(&verifier).as_str());
                }
                if err.is_none() && code.is_some() && !state_ok {
                    err = Some("state_mismatch".into());
                }
                match (&code, &err) {
                    (_, Some(e)) => respond(&mut stream, "授权失败", &format!("Spotify 返回：{e}")),
                    (Some(_), None) => respond(&mut stream, "授权成功", "可以关闭这个页面，回到 Lunac 继续。"),
                    (None, None) => respond(&mut stream, "回调异常", "没有收到授权码。"),
                }
                (code, err)
            }
            Err(e) => (None, Some(format!("回调监听失败：{e}"))),
        };

        let message = if let Some(e) = err.take() {
            crate::log::warn(&format!("spotify: 授权未完成（{e}）"));
            format!("授权未完成：{e}")
        } else if let Some(code) = code {
            match exchange_code(&cfg, &code, &verifier) {
                Ok(saved) => {
                    crate::log::info(&format!(
                        "spotify: 已授权（redirect={}，账号={}）",
                        redirect,
                        if saved.display_name.is_empty() { "未知" } else { saved.display_name.as_str() }
                    ));
                    String::new()
                }
                Err(e) => e,
            }
        } else {
            "授权未完成".to_string()
        };

        let _ = app.emit(
            "spotify-auth",
            AuthEvent {
                ok: message.is_empty(),
                message,
            },
        );
    });
}

/// state 由 verifier 派生（只有本进程知道），既做 CSRF 校验又不额外存一份状态。
fn cfg_state(verifier: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"lunac-spotify-state:");
    h.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(&h.finalize()[..12])
}

/// 用授权码换令牌，并把令牌与账号名落到 `config\music.json`。
fn exchange_code(cfg: &MusicConfig, code: &str, verifier: &str) -> Result<MusicConfig, String> {
    let client = http()?;
    let resp = client
        .post(SPOTIFY_TOKEN)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", cfg.redirect_uri().as_str()),
            ("client_id", cfg.client_id.trim()),
            ("code_verifier", verifier),
        ])
        .send()
        .map_err(|e| format!("换取令牌失败：{e}"))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!("换取令牌被拒（{status}）：{}", text.chars().take(200).collect::<String>()));
    }
    let t: TokenResponse = serde_json::from_str(&text).map_err(|e| format!("令牌响应解析失败：{e}"))?;
    if t.access_token.is_empty() {
        return Err("令牌响应里没有 access_token".into());
    }
    let mut saved = cfg.clone();
    saved.access_token = t.access_token;
    saved.refresh_token = t.refresh_token;
    saved.expires_at = now_secs() + t.expires_in.max(60);
    saved.display_name = fetch_display_name(&saved).unwrap_or_default();
    save_config(&saved)?;
    Ok(saved)
}

/// 取账号显示名；拿不到不算失败（`user-read-private` 未授权时这里会 403）。
fn fetch_display_name(cfg: &MusicConfig) -> Option<String> {
    let client = http().ok()?;
    let resp = client
        .get(format!("{SPOTIFY_API}/me"))
        .bearer_auth(&cfg.access_token)
        .send()
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().ok()?;
    v.get("display_name")
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| v.get("id").and_then(|d| d.as_str()).map(|s| s.to_string()))
}

// ── 命令：配置 ────────────────────────────────────────────────────

/// 读面板需要的配置视图（不回传令牌）。
#[tauri::command]
pub async fn music_config_get() -> Result<MusicConfigDto, String> {
    run_blocking(|| Ok(config_dto(&load_config()))).await
}

/// 保存 Client ID / 端口。**换 Client ID 或端口时清掉旧令牌** —— 令牌是绑在
/// (client_id, redirect_uri) 上的，留着只会让面板显示「已连接」而请求全 401。
#[tauri::command]
pub async fn music_config_set(client_id: String, port: Option<u16>) -> Result<MusicConfigDto, String> {
    run_blocking(move || {
        let mut cfg = load_config();
        let new_id = client_id.trim().to_string();
        let new_port = match port {
            Some(p) if p >= 1024 => p,
            _ => cfg.effective_port(),
        };
        if new_id != cfg.client_id || new_port != cfg.effective_port() {
            cfg.access_token.clear();
            cfg.refresh_token.clear();
            cfg.expires_at = 0;
            cfg.display_name.clear();
        }
        cfg.client_id = new_id;
        cfg.port = new_port;
        save_config(&cfg)?;
        crate::log::info(&format!(
            "music: 配置已保存（client_id={}，redirect={}）",
            if cfg.client_id.is_empty() { "空" } else { "已填" },
            cfg.redirect_uri()
        ));
        Ok(config_dto(&cfg))
    })
    .await
}

// ── 命令：OAuth ───────────────────────────────────────────────────

/// 起环回监听并返回**要打开的授权地址**。前端拿到后交给 `open()`（系统浏览器）。
///
/// 本命令**刻意不用 `run_blocking`**：它只做「bind 端口 + 生成 PKCE + 起线程」，
/// 没有等待（accept 在独立线程里），所以调用方不会卡。bind 失败（端口被占）当场报错，
/// 而不是等到用户点完授权才发现收不到回调。
#[tauri::command]
pub async fn spotify_connect(app: AppHandle) -> Result<String, String> {
    let cfg = load_config();
    if cfg.client_id.trim().is_empty() {
        return Err("ERR_NO_CLIENT_ID".into());
    }
    let port = cfg.effective_port();
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("ERR_PORT_BUSY: 127.0.0.1:{port} —— {e}"))?;

    let verifier = random_verifier();
    let challenge = code_challenge(&verifier);
    let state = cfg_state(&verifier);
    let url = format!(
        "{SPOTIFY_AUTHORIZE}?response_type=code&client_id={}&scope={}&redirect_uri={}&state={}&code_challenge_method=S256&code_challenge={}",
        percent_encoding::utf8_percent_encode(cfg.client_id.trim(), percent_encoding::NON_ALPHANUMERIC),
        percent_encoding::utf8_percent_encode(SCOPES, percent_encoding::NON_ALPHANUMERIC),
        percent_encoding::utf8_percent_encode(&cfg.redirect_uri(), percent_encoding::NON_ALPHANUMERIC),
        state,
        challenge,
    );

    crate::log::info(&format!("spotify: 已起环回监听 127.0.0.1:{port}，等待浏览器回调"));
    run_callback_server(app, listener, cfg, verifier);
    Ok(url)
}

/// 退出登录：清空本地令牌（`user-revoke` 需要额外接口，这里只做本地忘记）。
#[tauri::command]
pub async fn spotify_disconnect() -> Result<(), String> {
    run_blocking(|| {
        let mut cfg = load_config();
        cfg.access_token.clear();
        cfg.refresh_token.clear();
        cfg.expires_at = 0;
        cfg.display_name.clear();
        save_config(&cfg)?;
        crate::log::info("spotify: 已退出登录（本地令牌已清空）");
        Ok(())
    })
    .await
}

// ── 命令：播放状态与控制 ──────────────────────────────────────────

#[derive(Debug, Serialize, Default)]
pub struct TrackDto {
    pub id: String,
    pub name: String,
    pub artists: String,
    pub album: String,
    pub cover: String,
    pub duration_ms: i64,
}

#[derive(Debug, Serialize, Default)]
pub struct PlayerDto {
    /// 本地有令牌（不代表有活跃设备）。
    pub connected: bool,
    /// 有活跃的 Connect 设备。
    pub active: bool,
    pub playing: bool,
    pub progress_ms: i64,
    pub volume_percent: i64,
    pub device: String,
    pub track: Option<TrackDto>,
}

/// 读当前播放态。无活跃设备时 Spotify 返回 204 ⇒ `active=false`，不算错误。
#[tauri::command]
pub async fn spotify_status() -> Result<PlayerDto, String> {
    run_blocking(|| {
        let mut cfg = load_config();
        if !cfg.has_token() {
            return Err("ERR_NOT_CONNECTED".into());
        }
        let resp = api_get(&mut cfg, &format!("{SPOTIFY_API}/me/player"))?;
        if resp.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(PlayerDto {
                connected: true,
                ..Default::default()
            });
        }
        if !resp.status().is_success() {
            let code = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(format!(
                "读播放状态被拒（{code}）：{}",
                body.chars().take(200).collect::<String>()
            ));
        }
        let v: serde_json::Value = resp.json().map_err(|e| format!("播放状态解析失败：{e}"))?;

        let artists = v
            .pointer("/item/artists")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.get("name").and_then(|n| n.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let cover = v
            .pointer("/item/album/images")
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|x| x.get("url").and_then(|u| u.as_str()))
            .unwrap_or_default()
            .to_string();
        let track = v.get("item").filter(|i| !i.is_null()).map(|i| TrackDto {
            id: i.get("id").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            name: i.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            artists,
            album: i
                .pointer("/album/name")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            cover,
            duration_ms: i.get("duration_ms").and_then(|x| x.as_i64()).unwrap_or(0),
        });

        Ok(PlayerDto {
            connected: true,
            active: track.is_some(),
            playing: v.get("is_playing").and_then(|x| x.as_bool()).unwrap_or(false),
            progress_ms: v.get("progress_ms").and_then(|x| x.as_i64()).unwrap_or(0),
            volume_percent: v
                .pointer("/device/volume_percent")
                .and_then(|x| x.as_i64())
                .unwrap_or(-1),
            device: v
                .pointer("/device/name")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            track,
        })
    })
    .await
}

/// 播放控制。`value` 的含义随 `action` 变：`seek` = 毫秒，`volume` = 0–100。
#[tauri::command]
pub async fn spotify_control(action: String, value: Option<f64>) -> Result<(), String> {
    run_blocking(move || {
        let mut cfg = load_config();
        let token = ensure_access_token(&mut cfg)?;
        let client = http()?;
        let (method, url) = match action.as_str() {
            "play" => ("PUT", format!("{SPOTIFY_API}/me/player/play")),
            "pause" => ("PUT", format!("{SPOTIFY_API}/me/player/pause")),
            "next" => ("POST", format!("{SPOTIFY_API}/me/player/next")),
            "previous" => ("POST", format!("{SPOTIFY_API}/me/player/previous")),
            "seek" => (
                "PUT",
                format!(
                    "{SPOTIFY_API}/me/player/seek?position_ms={}",
                    value.unwrap_or(0.0).max(0.0) as i64
                ),
            ),
            "volume" => (
                "PUT",
                format!(
                    "{SPOTIFY_API}/me/player/volume?volume_percent={}",
                    value.unwrap_or(50.0).clamp(0.0, 100.0) as i64
                ),
            ),
            other => return Err(format!("未知的播放控制动作：{other}")),
        };
        let req = match method {
            "PUT" => client.put(&url),
            _ => client.post(&url),
        };
        let resp = req
            .bearer_auth(&token)
            .header("Content-Length", "0")
            .send()
            .map_err(|e| format!("发送播放控制失败：{e}"))?;
        // Spotify 成功时返回 204（无内容），所以只判状态码。
        if !resp.status().is_success() {
            let code = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(format!(
                "播放控制被拒（{code}）：{}",
                body.chars().take(200).collect::<String>()
            ));
        }
        Ok(())
    })
    .await
}

// ── 命令：歌词（LRCLIB）───────────────────────────────────────────

#[derive(Debug, Serialize, Default)]
pub struct LyricsDto {
    pub found: bool,
    pub instrumental: bool,
    pub track: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    /// LRC（`[mm:ss.xx]` 逐行）。没有逐行时间轴时为 `None`。
    pub synced: Option<String>,
    /// 纯文本歌词。
    pub plain: Option<String>,
}

fn lyrics_from_json(v: &serde_json::Value) -> LyricsDto {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
    LyricsDto {
        found: true,
        instrumental: v.get("instrumental").and_then(|x| x.as_bool()).unwrap_or(false),
        track: s("trackName"),
        artist: s("artistName"),
        album: s("albumName"),
        duration: v.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0),
        synced: v
            .get("syncedLyrics")
            .and_then(|x| x.as_str())
            .filter(|x| !x.trim().is_empty())
            .map(|x| x.to_string()),
        plain: v
            .get("plainLyrics")
            .and_then(|x| x.as_str())
            .filter(|x| !x.trim().is_empty())
            .map(|x| x.to_string()),
    }
}

/// 打 LRCLIB 的 GET，**失败重试一次**。
///
/// 为什么要重试：这是公开只读端点（GET、无副作用、无鉴权），偶发的 `send error`
/// 来自链路/代理抖动，而用户对此无能为力 —— 直接甩一句英文报错最没意义。
/// 2026-09-27 实测就遇到过一次 `error sending request`，紧接着重试即成功。
/// **只重试 send 失败**：429 是服务端明确要求退避的信号，立刻重试只会更糟（那条走
/// `ERR_RATE_LIMITED`，由面板提示用户等几秒）。
fn lrclib_get(url: &str) -> Result<(reqwest::StatusCode, String), String> {
    let client = http()?;
    let mut attempt = 0;
    loop {
        attempt += 1;
        match client.get(url).header("User-Agent", UA).send() {
            Ok(resp) => {
                let status = resp.status();
                // 429 时如实把 `Retry-After` 带出来 —— 面板要能告诉用户「等几秒再试」，
                // 而不是干瘪一句「失败」（LRCLIB 的公开限流策略就是靠这个头）。
                if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    let retry = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("?")
                        .to_string();
                    return Err(format!("ERR_RATE_LIMITED:{retry}"));
                }
                let text = resp.text().unwrap_or_default();
                return Ok((status, text));
            }
            Err(e) => {
                if attempt >= 2 {
                    return Err(format!("请求 LRCLIB 失败：{e}"));
                }
                std::thread::sleep(Duration::from_millis(350));
            }
        }
    }
}

/// 按「歌名 + 歌手（+ 专辑/时长）」取歌词：先精确 `/api/get`，404 再 `/api/search` 取首条。
///
/// 为什么要两步：`/api/get` 要求 `album_name` + `duration` 基本吻合，Spotify 报的时长
/// 与 LRCLIB 库里的录音版本常有 1–3 秒差 ⇒ 直接 404。回落搜索是主流做法。
#[tauri::command]
pub async fn lyrics_get(
    title: String,
    artist: String,
    album: Option<String>,
    duration: Option<f64>,
) -> Result<LyricsDto, String> {
    run_blocking(move || {
        let enc = qenc;
        let title = title.trim();
        let artist = artist.trim();
        if title.is_empty() {
            return Err("ERR_NO_QUERY".into());
        }

        let mut url = format!("{LRCLIB}/get?track_name={}&artist_name={}", enc(title), enc(artist));
        if let Some(a) = album.as_deref().filter(|a| !a.trim().is_empty()) {
            url.push_str(&format!("&album_name={}", enc(a)));
        }
        if let Some(d) = duration.filter(|d| *d > 0.0) {
            url.push_str(&format!("&duration={}", d.round() as i64));
        }
        let (status, text) = lrclib_get(&url)?;
        if status.is_success() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                let dto = lyrics_from_json(&v);
                if dto.synced.is_some() || dto.plain.is_some() {
                    return Ok(dto);
                }
                // 命中了一条「只有纯音乐标记」的记录：不当作找到，继续走搜索。
                if dto.instrumental {
                    return Ok(dto);
                }
            }
        } else if status != reqwest::StatusCode::NOT_FOUND {
            return Err(format!("LRCLIB 返回 {status}"));
        }

        // 2) 搜索回落
        let s_url = format!("{LRCLIB}/search?track_name={}&artist_name={}", enc(title), enc(artist));
        let (s_status, s_text) = lrclib_get(&s_url)?;
        if !s_status.is_success() {
            return Ok(LyricsDto::default());
        }
        let arr: Vec<serde_json::Value> = serde_json::from_str(&s_text).unwrap_or_default();
        // 优先「有时间轴的」，其次第一条有词的。
        let best = arr
            .iter()
            .find(|v| {
                v.get("syncedLyrics")
                    .and_then(|x| x.as_str())
                    .map(|x| !x.trim().is_empty())
                    .unwrap_or(false)
            })
            .or_else(|| {
                arr.iter().find(|v| {
                    v.get("plainLyrics")
                        .and_then(|x| x.as_str())
                        .map(|x| !x.trim().is_empty())
                        .unwrap_or(false)
                })
            });
        Ok(best.map(lyrics_from_json).unwrap_or_default())
    })
    .await
}

/// 手动搜歌词（面板上的搜索框）。返回若干候选供用户切换。
#[tauri::command]
pub async fn lyrics_search(q: String) -> Result<Vec<LyricsDto>, String> {
    run_blocking(move || {
        let q = q.trim();
        if q.is_empty() {
            return Err("ERR_NO_QUERY".into());
        }
        let url = format!("{LRCLIB}/search?q={}", qenc(q));
        let (status, text) = lrclib_get(&url)?;
        if !status.is_success() {
            return Err(format!("LRCLIB 返回 {status}"));
        }
        let arr: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap_or_default();
        Ok(arr.iter().take(20).map(lyrics_from_json).collect())
    })
    .await
}

// ── 单元测试 ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_uri_uses_fixed_port_and_loopback_ip() {
        let cfg = MusicConfig::default();
        // Spotify 明确不接受 localhost，必须是 127.0.0.1 字面量。
        assert_eq!(cfg.redirect_uri(), "http://127.0.0.1:8899/callback");
        assert!(!cfg.redirect_uri().contains("localhost"));
    }

    #[test]
    fn zero_port_falls_back_to_default() {
        let cfg = MusicConfig { port: 0, ..Default::default() };
        assert_eq!(cfg.effective_port(), DEFAULT_PORT);
    }

    #[test]
    fn verifier_shape_matches_rfc7636() {
        let v = random_verifier();
        assert_eq!(v.len(), 64);
        assert!(v.len() >= 43 && v.len() <= 128);
        assert!(v.chars().all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c)));
        // 两次必须不同（否则等于没有熵）
        assert_ne!(v, random_verifier());
    }

    #[test]
    fn challenge_is_s256_base64url_no_pad() {
        // RFC 7636 附录 B 的官方样例
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(code_challenge(verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        // base64url 不带填充
        assert!(!code_challenge(verifier).contains('='));
    }

    #[test]
    fn parses_callback_query() {
        let target = parse_request_target("GET /callback?code=abc%2Fd&state=xyz HTTP/1.1").unwrap();
        assert_eq!(query_param(&target, "code").unwrap(), "abc/d");
        assert_eq!(query_param(&target, "state").unwrap(), "xyz");
        assert!(query_param(&target, "nope").is_none());
    }

    #[test]
    fn state_is_derived_from_verifier_and_differs() {
        let a = cfg_state("verifier-a");
        let b = cfg_state("verifier-b");
        assert_ne!(a, b);
        assert_eq!(a, cfg_state("verifier-a"));
    }
}
