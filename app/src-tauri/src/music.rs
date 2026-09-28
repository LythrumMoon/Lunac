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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

use crate::commands::run_blocking;
use std::process::{Child, Stdio};
use std::sync::Mutex;

// ── 常量 ──────────────────────────────────────────────────────────

const SPOTIFY_AUTHORIZE: &str = "https://accounts.spotify.com/authorize";
const SPOTIFY_TOKEN: &str = "https://accounts.spotify.com/api/token";
const SPOTIFY_API: &str = "https://api.spotify.com/v1";
const LRCLIB: &str = "https://lrclib.net/api";

/// 网易云音乐（**公开只读**接口，2026-09-27 在目标机器上实测可用）。
///
/// **必须带 `Referer` + `User-Agent`**：不带 Referer 时搜索接口直接返回
/// `{"msg":"参数错误","code":400}`（实测，且与 UA 无关）。这是非官方接口，
/// 只用于第 2 级歌词兜底；拿不到就如实回落成「没歌词」，绝不报错打断面板。
const NETEASE_API: &str = "https://music.163.com/api";
/// 网易云的防盗链校验认这个头，缺了就是 400。
const NETEASE_REFERER: &str = "https://music.163.com/";

/// 内置的 Client ID —— **留给仓库主人填自己的那一个**（2026-09-27 加）。
///
/// **为什么要有它**：Spotify 的 OAuth 要求每个「应用」有自己的 Client ID，而
/// Development Mode 的规则（2026-02-11 起）是：应用所有者必须有 Premium、**每个应用
/// 最多 5 个授权用户**、名单要在仪表盘里逐个加白（详见 ai-spec §4.6）。让每个用户
/// 自己去 developer.spotify.com 建应用，对「只想登自己的账号听歌」的人是纯负担。
/// 内置一个 ID 后，用户点一下「登录 Spotify」就能用。
///
/// **但它只能服务 5 个人**（含所有者自己）—— 这是 Spotify 的硬规则，不是实现选择。
/// 要超过 5 人必须申请 Extended Quota Mode，而那条路只发给月活 ≥ 25 万的注册组织，
/// 个人拿不到。所以**自填入口必须保留**（`music_config_set` + 面板上的输入框），
/// 不能因为有了内置值就把那条路堵掉。
///
/// 空串 = 没有内置值，面板此时直接把凭据字段展开（与加这个常量之前的行为一致）。
/// 填法：在 developer.spotify.com 建一个 Web API 应用，把
/// `http://127.0.0.1:8899/callback`（或你自己改过的端口）加进它的 Redirect URI，
/// 然后把 Client ID 粘到这里。**不要**把 Client Secret 放进来 —— 桌面端走 PKCE 公开
/// 客户端流程，不需要它，放进来反而多一个泄露面。
///
/// **已填入**（2026-09-27，仓库主人的应用）：填上之后面板会走
/// `music.setup_builtin` 那段普通用户文案，并把凭据字段默认收起。
/// Client ID 是公开标识（PKCE 流程里它就出现在授权 URL 上），不是密钥。
pub const BUILTIN_CLIENT_ID: &str = "590875b9724848a284198bfe1bd85588";

/// 音乐插件在插件窗口体系里的 id（`plugin_window::label_for` 会变成 `plugin-music`）。
const MUSIC_PLUGIN_ID: &str = "music";

/// 需要授权面：读播放态 + 改播放态 + 读当前曲目 + 读用户歌单（含私有/协作）+ 读收藏（「我喜欢的歌曲」）
/// + 读关注的歌手（Library 的「歌手」那一栏）。
///
/// **后两个 scope 是 2026-09-27 加的**（歌单列）：scope 变了 ⇒ 已授权的令牌**不会**
/// 自动带上新权限，用户必须重新登录一次 Spotify。这与「换 Client ID / 端口要清令牌」
/// 是同一类事，但这里是 Spotify 侧的限制，只能在面板上如实提示。
///
/// **`user-library-read` 是 2026-09-27 晚加的**（「我喜欢的歌曲」）：Spotify 把收藏夹
/// **不放在 `/me/playlists` 里**（那里只有用户建的/关注的歌单），它走 `/me/tracks`，
/// 而那条端点没有这个 scope 就 `403`（实测）。所以：**不扩 scope 就永远看不到它**，
/// 别去歌单列表里找。
///
/// **`user-follow-read` 是 2026-09-28 加的**（Library 的歌手栏）：`/me/following` 缺它同样
/// `403`。加它的代价与上面那条一样 —— **又要重新登录一次**（用户已确认接受）。
/// 同一天加的「电台 / 专辑」两栏**不需要新 scope**（都走 `user-library-read`）。
const SCOPES: &str = "user-read-playback-state user-modify-playback-state user-read-currently-playing playlist-read-private playlist-read-collaborative user-library-read user-follow-read";

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

/// Client ID 的默认值 = 内置值。**用 `serde(default = ...)` 而不是 `#[serde(default)]`**：
/// 两者差别就在「老配置文件里没有这个字段」时 —— 前者补上内置值，后者补空串。
/// 面板不允许保存空 Client ID（前端会拒），所以「文件里是空」只可能来自老版本，
/// 补内置值正是想要的。
fn default_client_id() -> String {
    BUILTIN_CLIENT_ID.to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MusicConfig {
    #[serde(default = "default_client_id")]
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
    /// 头像 URL（顶部工具条）。与 `display_name` 同一时刻抓、同一份用途。
    #[serde(default)]
    pub avatar: String,
    /// **本机播放**（librespot）的可执行文件路径（2026-09-28）。空 = 自动探测
    /// （见 `find_librespot`）。**填了这个就不用等用户去动环境变量。**
    #[serde(default)]
    pub librespot_path: String,
    /// librespot 的 HTTP 代理（`-x`）。空 = 直连。
    ///
    /// **为什么留这个口子**：2026-09-28 实测，音频密钥那条 socket 走不走得通
    /// **取决于网络路径** —— 同一台机器上，某次直连刷满 `Audio key response timeout`，
    /// 换上另一套网络配置后直连又完全正常，挂代理也完全正常。所以「必须代理」不能写死，
    /// 也不能假设「直连永远行」。
    #[serde(default)]
    pub librespot_proxy: String,
}

impl Default for MusicConfig {
    fn default() -> Self {
        Self {
            client_id: default_client_id(),
            port: DEFAULT_PORT,
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: 0,
            display_name: String::new(),
            avatar: String::new(),
            librespot_path: String::new(),
            librespot_proxy: String::new(),
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
    /// 头像 URL（顶部工具条用）。授权那一次从 `/me` 的 `images[0].url` 抓一次存下来。
    pub avatar: String,
    /// 当前用的是不是**内置** Client ID（不是用户自己填的那个）。
    ///
    /// 面板据此决定要不要把凭据字段收起来（见 `music.ts` 的 `renderSetup`）：
    /// 用内置值时用户只需要点「登录」，不必看见 Redirect URI 那一串。
    pub builtin: bool,
    /// 本机播放（librespot）的两个设置，面板要能编辑（见 `librespot_path` 的说明）。
    pub librespot_path: String,
    pub librespot_proxy: String,
}

fn config_dto(cfg: &MusicConfig) -> MusicConfigDto {
    MusicConfigDto {
        client_id: cfg.client_id.clone(),
        port: cfg.effective_port(),
        redirect_uri: cfg.redirect_uri(),
        connected: cfg.has_token(),
        display_name: cfg.display_name.clone(),
        avatar: cfg.avatar.clone(),
        builtin: !BUILTIN_CLIENT_ID.is_empty() && cfg.client_id == BUILTIN_CLIENT_ID,
        librespot_path: cfg.librespot_path.clone(),
        librespot_proxy: cfg.librespot_proxy.clone(),
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
    if let Some((name, avatar)) = fetch_account(&saved) {
        saved.display_name = name;
        saved.avatar = avatar;
    }
    save_config(&saved)?;
    Ok(saved)
}

/// 取账号展示信息（显示名 + 头像 URL）；拿不到不算失败（`user-read-private` 未授权时这里会 403）。
///
/// **头像给顶部工具条**（2026-09-27）：`/me` 的 `images` 是**大 → 小**排序，取第 0 张。
/// 面板按 28px 显示，但存大图是为了高 DPI 屏；URL 本身很短，不值得为此再压一次。
fn fetch_account(cfg: &MusicConfig) -> Option<(String, String)> {
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
    let name = v
        .get("display_name")
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| v.get("id").and_then(|d| d.as_str()).map(|s| s.to_string()))
        .unwrap_or_default();
    let avatar = v
        .pointer("/images/0/url")
        .and_then(|d| d.as_str())
        .unwrap_or_default()
        .to_string();
    Some((name, avatar))
}

// ── 命令：配置 ────────────────────────────────────────────────────

/// 读面板需要的配置视图（不回传令牌）。
#[tauri::command]
pub async fn music_config_get() -> Result<MusicConfigDto, String> {
    run_blocking(|| Ok(config_dto(&load_config()))).await
}

/// 保存 Client ID / 端口 / 本机播放的两个设置。**换 Client ID 或端口时清掉旧令牌** ——
/// 令牌是绑在 (client_id, redirect_uri) 上的，留着只会让面板显示「已连接」而请求全 401。
///
/// `librespot_*` 两个是 `Option`：**面板只传它真正编辑过的那个**（`None` = 别动），
/// 否则「改个端口」会把用户填的 librespot 路径一起抹掉。
#[tauri::command]
pub async fn music_config_set(
    client_id: String,
    port: Option<u16>,
    librespot_path: Option<String>,
    librespot_proxy: Option<String>,
) -> Result<MusicConfigDto, String> {
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
        if let Some(p) = librespot_path {
            cfg.librespot_path = p.trim().to_string();
        }
        if let Some(p) = librespot_proxy {
            cfg.librespot_proxy = p.trim().to_string();
        }
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

#[derive(Debug, Serialize, Default, Clone)]
pub struct TrackDto {
    pub id: String,
    /// `spotify:track:…`。播放 / 队列项点击都要用它（比只给 id 少一处拼接）。
    pub uri: String,
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
    /// 随机播放开关（`shuffle_state`）。与 `repeat` 一起决定控制条上那个三态按钮。
    pub shuffle: bool,
    /// `off` / `context`（列表循环）/ `track`（单曲循环）—— `repeat_state` 原样。
    pub repeat: String,
    /// 当前播放上下文（`context.uri`，形如 `spotify:playlist:…`）。
    /// 用来在歌单列里标出「正在播的就是这个歌单」；本地播放 / 队列播放时为空。
    pub context_uri: String,
}

/// 我的歌单（歌单列用）。
#[derive(Debug, Serialize, Default)]
pub struct PlaylistDto {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub cover: String,
    /// 曲目数（Spotify 的 `tracks.total`，第三方歌单可能不精确，原样显示）。
    pub total: i64,
    pub owner: String,
}

/// 播放队列：当前曲 + 即将播放的若干首。
#[derive(Debug, Serialize, Default)]
pub struct QueueDto {
    pub current: Option<TrackDto>,
    pub items: Vec<TrackDto>,
}

/// 从 Spotify 的 track / episode 对象取我们需要的字段。
///
/// **episode 要单独兜**：播客没有 `artists`，`album` 也是空的 —— 拿 `show.name` 当
/// 「歌手」位，否则队列里的播客会显示成一行空白（用户不知道那是什么）。
fn track_from_json(i: &serde_json::Value) -> TrackDto {
    let is_episode = i.get("type").and_then(|x| x.as_str()) == Some("episode");
    let artists = if is_episode {
        i.pointer("/show/name")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    } else {
        // 与专辑副标题共用同一条「`artists` 拼名字」的逻辑（`artists_joined`）
        artists_joined(i)
    };
    let cover = i
        .pointer("/album/images")
        .or_else(|| i.pointer("/images"))
        .and_then(|a| a.as_array())
        .and_then(|arr| arr.first())
        .and_then(|x| x.get("url").and_then(|u| u.as_str()))
        .unwrap_or_default()
        .to_string();
    let album = if is_episode {
        i.pointer("/show/publisher")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    } else {
        i.pointer("/album/name")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    TrackDto {
        id: i.get("id").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        uri: i.get("uri").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        name: i.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        artists,
        album,
        cover,
        duration_ms: i.get("duration_ms").and_then(|x| x.as_i64()).unwrap_or(0),
    }
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

        let track = v.get("item").filter(|i| !i.is_null()).map(track_from_json);

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
            // 这两个字段**一直读**：控制条上的三态按钮靠它们回显真实状态，
            // 不自己记「我上次设成了什么」（用户在 Spotify 里改了就会漂移）。
            shuffle: v.get("shuffle_state").and_then(|x| x.as_bool()).unwrap_or(false),
            repeat: v
                .get("repeat_state")
                .and_then(|x| x.as_str())
                .unwrap_or("off")
                .to_string(),
            context_uri: v
                .pointer("/context/uri")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
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
            // 落盘带上**动作名**：`Restriction violated` / `No active device` 这类拒因
            // 必须能对上「用户点的是哪个按钮」，面板上那句话留不住。
            log_api_reject(&format!("播放控制 {action}"), &url, code, &body);
            return Err(format!(
                "播放控制被拒（{code}）：{}",
                body.chars().take(200).collect::<String>()
            ));
        }
        Ok(())
    })
    .await
}

// ── 命令：歌单 / 队列 / 播放模式（2026-09-27）──────────────────────
//
// 这一节全部为「歌单列 + 播放列表 + 控制条三态」服务，数据源是 Spotify Web API
// 的 `/me/playlists`、`/playlists/{id}/tracks`、`/me/player/queue`、
// `/me/player/shuffle|repeat`。**两条 API 硬限制**（决定了这里没有某些命令）：
//   · **没有「智能随机」接口** —— Smart Shuffle 只是客户端的本地功能，
//     `GET /me/player` 只给 `shuffle_state`(bool) + `repeat_state`(off/context/track)；
//   · **没有「删除队列项」接口** —— 队列只有 GET（读）与 POST（加到下一首）。

/// Spotify 的 id 只有 `[A-Za-z0-9]`。**这不是洁癖**：id 会拼进 URL 的路径段，
/// 不校验就等于把「改请求路径」的能力交给了外部输入。
fn is_safe_spotify_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// 允许播放的上下文类型：歌单 / 专辑 / 艺人 / 收藏夹。
const CONTEXT_KINDS: [&str; 4] = ["playlist", "album", "artist", "collection"];

/// `spotify:<kind>:<id>` 形状校验（`kind` 白名单 + id 字符集）。
fn is_safe_context_uri(uri: &str) -> bool {
    match uri.splitn(3, ':').collect::<Vec<_>>().as_slice() {
        ["spotify", kind, id] => CONTEXT_KINDS.contains(kind) && is_safe_spotify_id(id),
        _ => false,
    }
}

/// 可单曲播放的 uri：`spotify:track:<id>` / `spotify:episode:<id>`。
fn is_safe_track_uri(uri: &str) -> bool {
    match uri.splitn(3, ':').collect::<Vec<_>>().as_slice() {
        ["spotify", kind, id] => (*kind == "track" || *kind == "episode") && is_safe_spotify_id(id),
        _ => false,
    }
}

/// 记一条「Spotify 拒了这条请求」。
///
/// **为什么要落盘**：用户报「操作被拒绝」时，面板上那句话会被下一次轮询/重绘覆盖掉，
/// 而 Spotify 的响应体里恰好带着全部线索（`message` + `reason`：`PREMIUM_REQUIRED` /
/// `NO_ACTIVE_DEVICE` / `RESTRICTION_VIOLATED`…）。不记下来就只能靠猜。
/// 只记**端点路径**（我们调的 URL 都不带令牌）+ 状态码 + 响应体前 200 字。
fn log_api_reject(what: &str, url: &str, code: reqwest::StatusCode, body: &str) {
    let path = url.strip_prefix(SPOTIFY_API).unwrap_or(url);
    crate::log::warn(&format!(
        "spotify: {what} 被拒（{code}）{path} → {}",
        body.chars().take(200).collect::<String>()
    ));
}

/// 带一次自动刷新的 GET → JSON（401 重试由 `api_get` 负责）。
fn api_get_json(cfg: &mut MusicConfig, url: &str) -> Result<serde_json::Value, String> {
    let resp = api_get(cfg, url)?;
    if !resp.status().is_success() {
        let code = resp.status();
        let body = resp.text().unwrap_or_default();
        log_api_reject("读", url, code, &body);
        return Err(format!(
            "请求被拒（{code}）：{}",
            body.chars().take(200).collect::<String>()
        ));
    }
    resp.json().map_err(|e| format!("响应解析失败：{e}"))
}

/// 无 body 的 PUT / POST（播放控制那一类，成功是 204）。
///
/// **`Content-Length: 0` 只在没有 body 时手动写**：那是修「空 body 的 PUT 被拒」的，
/// 而有 JSON body 时必须让 reqwest 自己算长度（手写成 0 会把请求弄坏）。
fn api_send(
    cfg: &mut MusicConfig,
    method: &str,
    url: &str,
    body: Option<serde_json::Value>,
) -> Result<(), String> {
    let token = ensure_access_token(cfg)?;
    let client = http()?;
    let req = match (method, body) {
        ("PUT", Some(b)) => client.put(url).json(&b),
        ("POST", Some(b)) => client.post(url).json(&b),
        ("PUT", None) => client.put(url).header("Content-Length", "0"),
        _ => client.post(url).header("Content-Length", "0"),
    };
    let resp = req
        .bearer_auth(&token)
        .send()
        .map_err(|e| format!("请求 Spotify 失败：{e}"))?;
    if !resp.status().is_success() {
        let code = resp.status();
        let t = resp.text().unwrap_or_default();
        log_api_reject(method, url, code, &t);
        return Err(format!(
            "操作被拒（{code}）：{}",
            t.chars().take(200).collect::<String>()
        ));
    }
    Ok(())
}

/// 从 Spotify 的 playlist 对象取面板需要的字段。**没有 id 的返回 `None`** ——
/// 本地文件 / 播客收藏这类条目既没 id 也播不了，列出来只是噪声。
///
/// **抽成函数是因为搜索也要用同一套解析**（2026-09-27 加搜索框）：两处各写一份，
/// 字段一改名就必然漏掉一处 —— `tracks.total` → `items.total` 那次就是这么吃过亏的。
fn playlist_from_json(p: &serde_json::Value) -> Option<PlaylistDto> {
    let id = p.get("id").and_then(|x| x.as_str())?;
    Some(PlaylistDto {
        id: id.to_string(),
        uri: p.get("uri").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        name: p.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        cover: images_first(p),
        // 曲目数：**2026-09-27 实测 Spotify 已把它从 `tracks.total`
        // 改名成 `items.total`**（新版 playlist 对象里 `tracks` 整个没了）。
        // 两个都读：老应用/缓存里可能还有旧形状。
        total: p
            .pointer("/items/total")
            .or_else(|| p.pointer("/tracks/total"))
            .and_then(|x| x.as_i64())
            .unwrap_or(0),
        owner: p
            .pointer("/owner/display_name")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

/// 我的歌单（歌单列）。`limit=50` 是 Spotify 的上限，面板上不做分页。
///
/// **收藏夹（「我喜欢的歌曲」）不在这个列表里**：它是独立的 saved-tracks 资源，
/// 走 `/me/tracks`（见 `spotify_liked`）。别在这里找它 —— 找不到是对的。
#[tauri::command]
pub async fn spotify_playlists() -> Result<Vec<PlaylistDto>, String> {
    run_blocking(|| {
        let mut cfg = load_config();
        if !cfg.has_token() {
            return Err("ERR_NOT_CONNECTED".into());
        }
        let v = api_get_json(&mut cfg, &format!("{SPOTIFY_API}/me/playlists?limit=50"))?;
        Ok(v.get("items")
            .and_then(|x| x.as_array())
            .map(|arr| arr.iter().filter_map(playlist_from_json).collect::<Vec<_>>())
            .unwrap_or_default())
    })
    .await
}

/// 歌单里的曲目（展开某个歌单时调用）。取前 100 条 —— 面板不做分页。
///
/// **走 `/playlists/{id}/items`，不是 `/tracks`**（2026-09-27 实测 + 定案）：
/// Spotify 把歌单端点搬到了 `/items`，**老路径 `/tracks` 现在直接 `403 Forbidden`** ——
/// 症状是「歌单能列出来、点开一首歌都没有」，而且**与 scope 无关**（重新授权也一样，
/// 因为这不是权限问题，是端点没了）。同一个改名还牵动两处字段形状：
///   · 条目里那一层从 `track` 改名成 **`item`**（`{added_at, added_by, is_local, item:{…}}`）；
///   · 歌单对象里的计数从 `tracks.total` 改名成 `items.total`（见 `spotify_playlists`）。
/// 两个名字都兜着读：老应用/灰度期可能还是旧形状。
#[tauri::command]
pub async fn spotify_playlist_tracks(playlist_id: String) -> Result<Vec<TrackDto>, String> {
    run_blocking(move || {
        if !is_safe_spotify_id(playlist_id.trim()) {
            return Err("ERR_BAD_ID".into());
        }
        let mut cfg = load_config();
        let v = api_get_json(
            &mut cfg,
            &format!("{SPOTIFY_API}/playlists/{}/items?limit=100", playlist_id.trim()),
        )?;
        let out = v
            .get("items")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    // 条目是 `{added_at, item:{…}}`（旧形状是 `track`），null = 已下架；
                    // 本机音乐（`is_local`）没有 `spotify:` uri，点了也播不了 ⇒ 一并滤掉，
                    // 免得面板里出现一行「点了没反应」的死条目。
                    .filter_map(|it| it.get("item").or_else(|| it.get("track")))
                    .filter(|t| !t.is_null())
                    .map(track_from_json)
                    .filter(|t| is_safe_track_uri(&t.uri))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(out)
    })
    .await
}

/// 播放队列：当前曲 + 即将播放（Spotify 一次最多给 20 条）。
#[tauri::command]
pub async fn spotify_queue() -> Result<QueueDto, String> {
    run_blocking(|| {
        let mut cfg = load_config();
        if !cfg.has_token() {
            return Err("ERR_NOT_CONNECTED".into());
        }
        let v = api_get_json(&mut cfg, &format!("{SPOTIFY_API}/me/player/queue"))?;
        Ok(QueueDto {
            current: v
                .get("currently_playing")
                .filter(|x| !x.is_null())
                .map(track_from_json),
            items: v
                .get("queue")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter(|x| !x.is_null())
                        .map(track_from_json)
                        .collect()
                })
                .unwrap_or_default(),
        })
    })
    .await
}

/// 播放一个上下文（歌单 / 专辑 / 收藏夹），`offset_uri` 给定时从那一首开始。
#[tauri::command]
pub async fn spotify_play_context(
    context_uri: String,
    offset_uri: Option<String>,
) -> Result<(), String> {
    run_blocking(move || {
        let uri = context_uri.trim().to_string();
        if !is_safe_context_uri(&uri) {
            return Err("ERR_BAD_URI".into());
        }
        let mut cfg = load_config();
        let mut body = serde_json::json!({ "context_uri": uri });
        if let Some(u) = offset_uri.map(|u| u.trim().to_string()).filter(|u| is_safe_track_uri(u)) {
            body["offset"] = serde_json::json!({ "uri": u });
        }
        api_send(&mut cfg, "PUT", &format!("{SPOTIFY_API}/me/player/play"), Some(body))
    })
    .await
}

/// 从某一首开始播（队列项点击）。
///
/// **Spotify 没有「删除队列项」接口**（队列只有读 + 加到下一首），这是 API 的硬限制。
/// 用 `{uris:[uri]}` 播放等价于「从这首开始，丢掉它之前的队列项」—— 能拿到的最接近的效果，
/// 面板上按这个语义标注，不假装能删中间某一项（见 music.ts 的队列区）。
#[tauri::command]
pub async fn spotify_play_uri(uri: String) -> Result<(), String> {
    run_blocking(move || {
        let u = uri.trim().to_string();
        if !is_safe_track_uri(&u) {
            return Err("ERR_BAD_URI".into());
        }
        let mut cfg = load_config();
        api_send(
            &mut cfg,
            "PUT",
            &format!("{SPOTIFY_API}/me/player/play"),
            Some(serde_json::json!({ "uris": [u] })),
        )
    })
    .await
}

/// 控制条的随机三态：`off`（关）/ `shuffle`（随机）/ `repeat_one`（单曲循环）。
///
/// 三态分别落到 Spotify 的两个字段上：`shuffle` 真假 + `repeat`（off / track）。
/// **不提供「智能随机」** —— Spotify Web API 没有任何 Smart Shuffle 接口（读都读不到），
/// 它只是手机/桌面客户端的本地功能。做不出来，也不假装能做。
#[tauri::command]
pub async fn spotify_set_play_mode(mode: String) -> Result<(), String> {
    run_blocking(move || {
        let mut cfg = load_config();
        let (shuffle, repeat) = match mode.as_str() {
            "off" => ("false", "off"),
            "shuffle" => ("true", "off"),
            "repeat_one" => ("false", "track"),
            other => return Err(format!("未知的播放模式：{other}")),
        };
        api_send(
            &mut cfg,
            "PUT",
            &format!("{SPOTIFY_API}/me/player/shuffle?state={shuffle}"),
            None,
        )?;
        api_send(
            &mut cfg,
            "PUT",
            &format!("{SPOTIFY_API}/me/player/repeat?state={repeat}"),
            None,
        )
    })
    .await
}

/// 「我喜欢的歌曲」一次拉多少首。**50 是 `/me/tracks` 的单页上限** ——
/// 面板不做分页，所以这就是面板上能看到（也能一次播）的全部。
const LIKED_LIMIT: i64 = 50;

/// 收藏夹（「我喜欢的歌曲」）。
///
/// **Spotify 不把它放在 `/me/playlists` 里**：那里只有用户建的 / 关注的歌单，
/// 收藏夹是独立的 saved-tracks 资源，走 `/me/tracks`。2026-09-27 实测：
/// 该账号 `/me/playlists` 返回 8 个歌单、**一个收藏夹都没有**；而 `/me/tracks`
/// 在缺 `user-library-read` 时直接 `403` —— 所以面板上那一条是**前端合成的**，
/// 不是从歌单列表里筛出来的（别再去歌单列表里找了）。
///
/// 条目形状与歌单曲目同一类风险（`track` → `item` 改过名），两个名字都兜着读。
#[derive(Debug, Serialize, Default)]
pub struct LikedDto {
    /// Spotify 报的收藏总数（可能大于 `items.len()`，见 `LIKED_LIMIT`）。
    pub total: i64,
    pub items: Vec<TrackDto>,
}

#[tauri::command]
pub async fn spotify_liked() -> Result<LikedDto, String> {
    run_blocking(|| {
        let mut cfg = load_config();
        if !cfg.has_token() {
            return Err("ERR_NOT_CONNECTED".into());
        }
        let v = api_get_json(&mut cfg, &format!("{SPOTIFY_API}/me/tracks?limit={LIKED_LIMIT}"))?;
        let items = v
            .get("items")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    // 条目是 `{added_at, track:{…}}`（改过名的话是 `item`）
                    .filter_map(|it| it.get("track").or_else(|| it.get("item")))
                    .filter(|t| !t.is_null())
                    .map(track_from_json)
                    // 本机音乐 / 已下架没有可播的 `spotify:` uri ⇒ 滤掉
                    .filter(|t| is_safe_track_uri(&t.uri))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let total = v.get("total").and_then(|x| x.as_i64()).unwrap_or(items.len() as i64);
        Ok(LikedDto { total, items })
    })
    .await
}

/// 一次播一串曲目。
///
/// **收藏夹没有可播的 `context_uri`**（`spotify:collection` 不在 Web API 的合法上下文里），
/// 「我喜欢的歌曲」只能走 `uris`。上限 100 条 = Web API 一次能给的最大值，
/// 而我们最多也就 `LIKED_LIMIT`(50) 条，够用。
#[tauri::command]
pub async fn spotify_play_uris(uris: Vec<String>) -> Result<(), String> {
    run_blocking(move || {
        let list: Vec<String> = uris
            .into_iter()
            .map(|u| u.trim().to_string())
            .filter(|u| is_safe_track_uri(u))
            .take(100)
            .collect();
        if list.is_empty() {
            return Err("ERR_BAD_URI".into());
        }
        let mut cfg = load_config();
        api_send(
            &mut cfg,
            "PUT",
            &format!("{SPOTIFY_API}/me/player/play"),
            Some(serde_json::json!({ "uris": list })),
        )
    })
    .await
}

/// 搜索结果（顶部工具条那个搜索框 + 搜索详细页）。
///
/// **五类一起返回**（2026-09-28 扩）：详细页是按「歌曲 / 歌手 / 专辑 / 歌单 / 播客」
/// 分页签的，一次请求凑齐四类比按页签发五次请求省得多，也不会出现「切页签才转圈」。
/// limit=10 是 Development Mode 的上限（见 ai-spec §4.6 的硬限制那一节）。
#[derive(Debug, Serialize, Default)]
pub struct SearchDto {
    pub tracks: Vec<TrackDto>,
    pub playlists: Vec<PlaylistDto>,
    pub artists: Vec<LibraryItemDto>,
    pub albums: Vec<LibraryItemDto>,
    pub shows: Vec<LibraryItemDto>,
}

/// 全局搜索：曲目 / 歌单 / 歌手 / 专辑 / 播客各取前 10。
///
/// **query 必须走 `qenc`**：空格、`&`、中日文都会把 query string 拆坏
/// （与 OAuth 那处同一条纪律）。空 query 直接拒，不去打 Spotify。
#[tauri::command]
pub async fn spotify_search(query: String) -> Result<SearchDto, String> {
    run_blocking(move || {
        let q = query.trim();
        if q.is_empty() {
            return Err("ERR_NO_QUERY".into());
        }
        let mut cfg = load_config();
        if !cfg.has_token() {
            return Err("ERR_NOT_CONNECTED".into());
        }
        let v = api_get_json(
            &mut cfg,
            &format!(
                "{SPOTIFY_API}/search?type=track,playlist,album,artist,show&limit=10&q={}",
                qenc(q)
            ),
        )?;
        let tracks = v
            .pointer("/tracks/items")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter(|t| !t.is_null())
                    .map(track_from_json)
                    .filter(|t| is_safe_track_uri(&t.uri))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let playlists = v
            .pointer("/playlists/items")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter(|p| !p.is_null()).filter_map(playlist_from_json).collect::<Vec<_>>())
            .unwrap_or_default();
        Ok(SearchDto {
            tracks,
            playlists,
            // 搜索返回的是**完整对象**（带 `images` / `artists` / `total_tracks`），
            // 与 `/me/*` 那几条的形状一致 ⇒ 复用同一套解析（见 `library_items`）。
            artists: library_items(&v, "/artists/items", "artist"),
            albums: library_items(&v, "/albums/items", "album"),
            shows: library_items(&v, "/shows/items", "show"),
        })
    })
    .await
}

// ── 命令：Library（歌单 / 专辑 / 歌手 / 电台）+ 条目的曲目 + 设备（2026-09-28）──
//
// 这一组是「默认面板按 Spotify 复刻」的宿主侧：左侧栏要四类库资源、中间大区要
// 「选中项的全部曲目」、工具条要一个设备按钮。三条 API 事实先写在这里，免得下次再猜：
//
//   1. **四类资源各有各的端点**：歌单 `/me/playlists`、专辑 `/me/albums`、
//      关注的歌手 `/me/following?type=artist`、收藏的播客 `/me/shows`。
//      收藏夹（我喜欢的歌曲）是第五条路，走 `/me/tracks`（见 `spotify_liked`）。
//   2. **专辑 / 电台的端点不带图**：`/albums/{id}/tracks` 返回的是
//      **Simplified Track Object**，里面**没有 `album` 字段** ⇒ 每行的封面是空的。
//      所以主区的页面头必须自己把封面画出来（Spotify 也是这么做的：专辑页的每行
//      只有一个序号，不看封面）。
//   3. **电台（show）没有可播上下文**：`context_uri` 只认 album / artist / playlist
//      ⇒ `LibraryItemDto.uri` 对 show 留空，前端改走 `spotify_play_uris`（逐集播）。

/// 侧栏与主区共用的一张卡片。
///
/// 为什么不复用 `PlaylistDto`：它的 `owner` / `total` 是歌单专有语义，歌手身上没有这两样，
/// 混用会让「有没有值」的判定散成一片。为什么不拆成四个 DTO：这四种在面板上的**呈现
/// 完全一样**（方图 + 标题 + 一行副标题 + 点击进主区），差别只在 `kind`（决定主区去拉
/// 哪条端点、能不能用 `context_uri` 播）。
#[derive(Debug, Serialize, Default)]
pub struct LibraryItemDto {
    pub id: String,
    /// 可播上下文（`spotify:playlist:` / `spotify:album:` / `spotify:artist:`）。
    /// **电台留空** —— 见本段开头第 3 条。
    pub uri: String,
    pub name: String,
    pub cover: String,
    /// 一行副标题：专辑=歌手、歌手=流派（常为空，前端兜「歌手」）、电台=发布方。
    pub subtitle: String,
    /// 条目数（专辑的总曲数 / 电台的总集数）；歌手没有 ⇒ 0。
    pub total: i64,
}

/// 取 `images[0].url`。曲目 / 歌单 / 库条目三处都要，抽出来免得漏掉一处
/// （`images` 是**大→小**排序，第 0 张就是列表缩图要的那张）。
fn images_first(v: &serde_json::Value) -> String {
    v.get("images")
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|x| x.get("url").and_then(|u| u.as_str()))
        .unwrap_or_default()
        .to_string()
}

/// `artists:[{name}]` → `"A, B"`。曲目（歌手列）与专辑（副标题）共用。
fn artists_joined(o: &serde_json::Value) -> String {
    o.get("artists")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.get("name").and_then(|n| n.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// 把一段「库条目」数组解析成面板卡片。
///
/// **同一套解析要服务三种来源**（`/me/albums`、`/me/shows`、以及搜索返回的裸数组），
/// 它们的差别只有两层：① 数组挂在哪个指针下（`/items` 还是 `/artists/items`）；
/// ② `/me/*` 的元素外面包了一层 `{added_at, album|show}`，而搜索返回的是**裸对象**。
/// 所以这里统一 `or_else(...).unwrap_or(el)` 剥一层，剥不到就用它自己。
fn library_items(v: &serde_json::Value, pointer: &str, kind: &str) -> Vec<LibraryItemDto> {
    let arr = match v.pointer(pointer).and_then(|x| x.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter(|x| !x.is_null())
        .filter_map(|el| {
            let o = el
                .get("album")
                .or_else(|| el.get("show"))
                .or_else(|| el.get("artist"))
                .unwrap_or(el);
            let id = o.get("id").and_then(|x| x.as_str())?;
            if !is_safe_spotify_id(id) {
                return None;
            }
            let (uri, subtitle, total) = match kind {
                "artist" => (
                    format!("spotify:artist:{id}"),
                    // 流派常常是空数组 ⇒ 回空串，前端兜「歌手」这条 i18n 文案
                    o.get("genres")
                        .and_then(|g| g.as_array())
                        .and_then(|g| g.first())
                        .and_then(|s| s.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    0,
                ),
                "album" => (
                    format!("spotify:album:{id}"),
                    artists_joined(o),
                    o.get("total_tracks").and_then(|x| x.as_i64()).unwrap_or(0),
                ),
                _ => (
                    // show：故意留空（不可作 context_uri）
                    String::new(),
                    o.get("publisher").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                    o.get("total_episodes").and_then(|x| x.as_i64()).unwrap_or(0),
                ),
            };
            Some(LibraryItemDto {
                id: id.to_string(),
                uri,
                name: o.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                cover: images_first(o),
                subtitle,
                total,
            })
        })
        .collect()
}

/// 关注的歌手。**需要 `user-follow-read`**（缺它 403，见 `SCOPES` 的说明）。
#[tauri::command]
pub async fn spotify_artists() -> Result<Vec<LibraryItemDto>, String> {
    run_blocking(|| {
        let mut cfg = load_config();
        if !cfg.has_token() {
            return Err("ERR_NOT_CONNECTED".into());
        }
        let v = api_get_json(
            &mut cfg,
            &format!("{SPOTIFY_API}/me/following?type=artist&limit=50"),
        )?;
        Ok(library_items(&v, "/artists/items", "artist"))
    })
    .await
}

/// 收藏的专辑。`user-library-read` 就够（与收藏夹同一个 scope）。
#[tauri::command]
pub async fn spotify_albums() -> Result<Vec<LibraryItemDto>, String> {
    run_blocking(|| {
        let mut cfg = load_config();
        if !cfg.has_token() {
            return Err("ERR_NOT_CONNECTED".into());
        }
        let v = api_get_json(&mut cfg, &format!("{SPOTIFY_API}/me/albums?limit=50"))?;
        Ok(library_items(&v, "/items", "album"))
    })
    .await
}

/// 收藏的播客 / 电台。同样只需要 `user-library-read`。
#[tauri::command]
pub async fn spotify_shows() -> Result<Vec<LibraryItemDto>, String> {
    run_blocking(|| {
        let mut cfg = load_config();
        if !cfg.has_token() {
            return Err("ERR_NOT_CONNECTED".into());
        }
        let v = api_get_json(&mut cfg, &format!("{SPOTIFY_API}/me/shows?limit=50"))?;
        Ok(library_items(&v, "/items", "show"))
    })
    .await
}

/// 主区那一列曲目：按 `kind` 去拉对应资源。
///
/// **`kind` 必须白名单**（它拼进 URL 路径），`id` 走 `is_safe_spotify_id`。
/// 四条端点的响应形状**不一样**，但落到**元素**上都只有两种形态：
///   · 歌单：`{added_at, track|item:{…}}`（两条路径的包装，见端点改名那一节）
///   · 专辑 / 电台 / 歌手：直接就是 track / episode 对象
/// 另外**歌手的数组键是 `tracks` 而不是 `items`**（`/top-tracks`）。
/// ⇒ 统一「先剥包装、再 `track_from_json`」，不写四份解析。
#[tauri::command]
pub async fn spotify_item_tracks(kind: String, id: String) -> Result<Vec<TrackDto>, String> {
    run_blocking(move || {
        if !is_safe_spotify_id(&id) {
            return Err("ERR_BAD_ID".into());
        }
        let mut cfg = load_config();
        if !cfg.has_token() {
            return Err("ERR_NOT_CONNECTED".into());
        }
        let url = match kind.as_str() {
            // ⚠️ 歌单是 `/items`：老 `/tracks` 现在直接 403（与 scope 无关）
            "playlist" => format!("{SPOTIFY_API}/playlists/{id}/items?limit=100"),
            "album" => format!("{SPOTIFY_API}/albums/{id}/tracks?limit=50"),
            // 歌手没有「曲目」这种资源 ⇒ 用热门曲。`market` 是必填项，`from_token`
            // 让服务端按**当前令牌的地区**挑（`/me` 已不再返回 country，别去读它 —— 实测）。
            "artist" => format!("{SPOTIFY_API}/artists/{id}/top-tracks?market=from_token"),
            "show" => format!("{SPOTIFY_API}/shows/{id}/episodes?limit=50"),
            _ => return Err("ERR_BAD_KIND".into()),
        };
        let v = api_get_json(&mut cfg, &url)?;
        let items = v
            .get("items")
            .or_else(|| v.get("tracks"))
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter(|x| !x.is_null())
                    .map(|el| el.get("track").or_else(|| el.get("item")).unwrap_or(el))
                    .map(track_from_json)
                    // 下架 / 本机文件没有可播 uri ⇒ 滤掉，不给「点了播不了」的死条目
                    .filter(|t| is_safe_track_uri(&t.uri))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(items)
    })
    .await
}

/// 一台可控制的 Connect 设备。
#[derive(Debug, Serialize, Default)]
pub struct DeviceDto {
    pub id: String,
    pub name: String,
    /// `Computer` / `Smartphone` / `Speaker` / `TV` / `Automobile`…（前端只当图标位用）
    pub kind: String,
    /// 正在收声的那一台。**切换设备时要知道「现在在哪」**，所以必须回传。
    pub active: bool,
    pub volume_percent: i64,
}

/// 可控制的设备列表（工具条那个「设备」按钮）。
///
/// **没有设备时返回空数组，不是错误**：那正是要显示「本机播放 / 去别的设备上开一下」
/// 这两种引导的场景。写成 `Err` 的话面板会弹一条报错，而报错不能点、引导能点。
#[tauri::command]
pub async fn spotify_devices() -> Result<Vec<DeviceDto>, String> {
    run_blocking(|| {
        let mut cfg = load_config();
        if !cfg.has_token() {
            return Err("ERR_NOT_CONNECTED".into());
        }
        let v = api_get_json(&mut cfg, &format!("{SPOTIFY_API}/me/player/devices"))?;
        Ok(v.get("devices")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter(|d| !d.is_null())
                    .filter_map(|d| {
                        // 设备 id 是 40 位十六进制（不是 base62 的 Spotify id），
                        // 但字符集是它的子集 ⇒ 同一条白名单够用
                        let id = d.get("id").and_then(|x| x.as_str())?;
                        if !is_safe_spotify_id(id) {
                            return None;
                        }
                        Some(DeviceDto {
                            id: id.to_string(),
                            name: d.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                            kind: d.get("type").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                            active: d.get("is_active").and_then(|x| x.as_bool()).unwrap_or(false),
                            volume_percent: d.get("volume_percent").and_then(|x| x.as_i64()).unwrap_or(0),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default())
    })
    .await
}

/// 把播放转移到某一台设备（`PUT /me/player` 的 `device_ids`）。
///
/// `play=true` 时 Spotify **继续播当前那一首**（不是从头开始）；没有当前曲目时它自己忽略。
/// 本机播放（librespot）刚起来时正好用 `true` 把「刚才在桌面端/手机听的那首」接过来。
#[tauri::command]
pub async fn spotify_transfer(device_id: String, play: bool) -> Result<(), String> {
    run_blocking(move || {
        if !is_safe_spotify_id(&device_id) {
            return Err("ERR_BAD_ID".into());
        }
        let mut cfg = load_config();
        api_send(
            &mut cfg,
            "PUT",
            &format!("{SPOTIFY_API}/me/player"),
            Some(serde_json::json!({ "device_ids": [device_id], "play": play })),
        )
    })
    .await
}

// ── 本机播放：librespot 子进程（2026-09-28）─────────────────────────
//
// 目标（用户 2026-09-28 选定要做）：**登入一次之后，不依赖 Spotify 桌面端也能出声**。
// 为什么只能用 librespot：官方没有「桌面端自己发声」的接口；Web Playback SDK 要
// EME/Widevine（WebView2 上不做）。librespot 是社区实现的 Connect 接收器 ——
// 它在 Spotify 眼里就是**另一台设备**，所以我们照旧用 Web API 控制它。
//
// 四条硬事实（全是实测，别照直觉改）：
//   1. **必须 Premium**，免费账号直接拒（与播放控制同一条门槛）。
//   2. **音频密钥走的是另一条到 AP 的 socket**：那条路不通时表现为
//      `Audio key response timeout` → `continuing without decryption` → 一堆
//      `invalid mpeg audio header`（拿到的是**没解密**的字节）。`-x/--proxy` 实测
//      **连这条 socket 一起走**（日志里 `librespot_core::socket] Using proxy` 出现两次）
//      ⇒ 这就是 `librespot_proxy` 存在的理由。
//   3. **它是独立进程**：WebView 里起不了进程，只能宿主起停。
//   4. **半死的它会占着会话**、诱发 `NO_ACTIVE_DEVICE`（2026-09-27 踩过）
//      ⇒ 停的时候**先 pause 再 kill**，别只 kill。

/// 设备名。它在 Spotify 的设备列表里就是这个字符串 —— 用户要能一眼认出「这是我们自己的」。
const LIBRESPOT_DEVICE_NAME: &str = "Lunac";

/// 正在跑的 librespot 子进程。`None` = 没在跑。
///
/// 存住 `Child` 才能 kill；**`Mutex<Option<Child>>` 用 `const` 初始化**（不需要 OnceLock）。
static LIBRESPOT: Mutex<Option<Child>> = Mutex::new(None);

/// librespot 的凭据 / 音频缓存目录（`-c`）。放在 `music.json` 旁边：
/// ① 与令牌同处一地，用户备份 / 清账号时不会漏；② 下次启动直接复用，**不用再登一次**。
fn librespot_cache_dir() -> PathBuf {
    music_config_path().with_file_name("librespot-cache")
}

/// 找 librespot 可执行文件。顺序：**配置里填的 → 与 Lunac 同目录 → cargo 的 bin 目录**。
///
/// 找不到**不是错**（面板据此把开关置灰并说明），所以这里回 `Option` 而不是 `Result`。
/// 不扫 PATH：那是「静默拿到一个别的版本」的路子，宁可让用户填一次路径。
fn find_librespot(cfg: &MusicConfig) -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = Vec::new();
    let filled = cfg.librespot_path.trim();
    if !filled.is_empty() {
        cands.push(PathBuf::from(filled));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // 打包后它会和主程序放在一起（`tauri.conf.json` 的 resources 落到同一目录）
            cands.push(dir.join("librespot.exe"));
        }
    }
    // `cargo install librespot` 的标准落点（开发机上最省事的一条）
    if let Some(home) = std::env::var_os("USERPROFILE") {
        cands.push(PathBuf::from(home).join(".cargo").join("bin").join("librespot.exe"));
    }
    cands.into_iter().find(|p| p.is_file())
}

/// 把子进程的 stdout / stderr **各自丢进一个线程排空**。
///
/// **必须排空**：两个管道都塞满时会双向死锁（与 `convert.rs` 的纪律 #36 ① 同一条）。
/// 顺手转进本仓日志 —— librespot 那句 `Audio key response timeout` 是**唯一**能诊断
/// 「在跑但没声音」的线索，丢了就只能靠猜。
fn drain_librespot_log<R: std::io::Read + Send + 'static>(r: R) {
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines() {
            match line {
                Ok(l) if !l.trim().is_empty() => crate::log::info(&format!("librespot: {l}")),
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });
}

/// 面板可见的本机播放状态。
#[derive(Debug, Serialize, Default)]
pub struct LibrespotDto {
    /// 进程在跑（**由 `try_wait` 现算**，不是我们自己记的标志位 —— 它可能自己崩了）。
    pub running: bool,
    /// 找得到可执行文件。false 时面板要把开关置灰并说清「去哪填路径」。
    pub available: bool,
    /// 实际用的路径（面板上显示出来，便于诊断「填了但没生效」）。
    pub path: String,
}

/// 现算状态。**顺手回收死掉的句柄**：`try_wait` 报「已退出」时把槽清空，
/// 否则下次点「开始」会看到一个已经死掉、却仍被认作在跑的句柄。
fn librespot_dto(cfg: &MusicConfig) -> LibrespotDto {
    let path = find_librespot(cfg);
    let mut running = false;
    if let Ok(mut slot) = LIBRESPOT.lock() {
        if let Some(child) = slot.as_mut() {
            match child.try_wait() {
                Ok(None) => running = true,
                // 已退出 / 问不出来 ⇒ 清掉，别留着一个假的「在跑」
                _ => *slot = None,
            }
        }
    }
    LibrespotDto {
        running,
        available: path.is_some(),
        path: path.map(|p| p.display().to_string()).unwrap_or_default(),
    }
}

/// 读当前状态（面板打开设备弹层时调它）。
#[tauri::command]
pub async fn librespot_status() -> Result<LibrespotDto, String> {
    run_blocking(|| Ok(librespot_dto(&load_config()))).await
}

/// 起本机播放。已经活着就直接回状态（点两次不该起两个进程）。
#[tauri::command]
pub async fn librespot_start() -> Result<LibrespotDto, String> {
    run_blocking(|| {
        let cfg = load_config();
        if librespot_dto(&cfg).running {
            return Ok(librespot_dto(&cfg));
        }
        let exe = find_librespot(&cfg).ok_or_else(|| "ERR_NO_LIBRESPOT".to_string())?;
        let cache = librespot_cache_dir();
        std::fs::create_dir_all(&cache).map_err(|e| format!("建不了 librespot 缓存目录：{e}"))?;

        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--name")
            .arg(LIBRESPOT_DEVICE_NAME)
            .arg("-c")
            .arg(&cache)
            // 320 = Premium 的最高档。音量交给 Web API 那个音量条，不用它本地的软音量。
            .arg("--bitrate")
            .arg("320")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let proxy = cfg.librespot_proxy.trim();
        if !proxy.is_empty() {
            cmd.arg("-x").arg(proxy);
        }
        // 不加这个会**闪一个黑色控制台窗口**（librespot 默认带 console subsystem）
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| format!("起不了 librespot：{e}"))?;
        if let Some(o) = child.stdout.take() {
            drain_librespot_log(o);
        }
        if let Some(e) = child.stderr.take() {
            drain_librespot_log(e);
        }
        *LIBRESPOT.lock().map_err(|e| e.to_string())? = Some(child);
        crate::log::info(&format!(
            "librespot: 已启动本机播放（{}）",
            exe.display()
        ));
        Ok(librespot_dto(&cfg))
    })
    .await
}

/// 停本机播放。**先暂停再杀** —— 见本节开头第 4 条。
#[tauri::command]
pub async fn librespot_stop() -> Result<LibrespotDto, String> {
    run_blocking(|| {
        let mut cfg = load_config();
        // 暂停失败不算错：可能本来就没在播，或者令牌刚过期。
        if cfg.has_token() {
            let _ = api_send(&mut cfg, "PUT", &format!("{SPOTIFY_API}/me/player/pause"), None);
        }
        if let Ok(mut slot) = LIBRESPOT.lock() {
            if let Some(mut child) = slot.take() {
                let _ = child.kill();
                let _ = child.wait();
                crate::log::info("librespot: 已停止本机播放");
            }
        }
        Ok(librespot_dto(&cfg))
    })
    .await
}

/// 退出清理：**宿主退出必须带走 librespot**。
///
/// 不带走的话，下次开机它还占着那台设备，用户会看到一台永远连不上的「Lunac」，
/// 而在任务管理器里也未必认得出那是谁（进程名是 librespot.exe）。
pub fn kill_librespot() {
    if let Ok(mut slot) = LIBRESPOT.lock() {
        if let Some(mut child) = slot.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ── 命令：歌词（LRCLIB + 网易云兜底）─────────────────────────────

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

/// 按「歌名 + 歌手（+ 专辑/时长）」取歌词：LRCLIB 精确 → LRCLIB 搜索 → **网易云兜底**。
///
/// 为什么要三级：`/api/get` 要求 `album_name` + `duration` 基本吻合，Spotify 报的时长
/// 与 LRCLIB 库里的录音版本常有 1–3 秒差 ⇒ 直接 404；搜索能救回大部分，但冷门 / 中文歌
/// 在 LRCLIB 上常年没有条目 —— 于是加了第 3 级（用户 2026-09-27 明确要求有兜底平台）。
/// **任何一级命中就返回**，全部落空才返回 `found:false`（面板显示「没找到歌词」，不报错）。
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

        // 2) LRCLIB 搜索回落
        let s_url = format!("{LRCLIB}/search?track_name={}&artist_name={}", enc(title), enc(artist));
        let (s_status, s_text) = lrclib_get(&s_url)?;
        if !s_status.is_success() {
            // LRCLIB 整条链路不可用时也要给兜底源一次机会 —— 兜底的意义正在于此。
            return Ok(netease_lyrics(title, artist).unwrap_or_default());
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
        if let Some(v) = best {
            return Ok(lyrics_from_json(v));
        }

        // 3) 第 2 级兜底：网易云（公开只读，需 Referer）。拿不到就如实「没歌词」。
        Ok(netease_lyrics(title, artist).unwrap_or_default())
    })
    .await
}

// ── 第 2 级歌词来源：网易云（公开只读）────────────────────────────
//
// 用户 2026-09-27 要求「Spotify 没有就去另一些平台抓取作为兜底」，选定网易云。
// 两步：搜索拿 song id → 取 LRC。两个接口都**必须带 `Referer`**（实测缺了返回
// `{"msg":"参数错误","code":400}`）。非官方接口 ⇒ 全程**失败即 `None`**，
// 绝不把一个「主源没找到、兜底源也挂了」的状态升级成一次报错。

/// 归一化歌名用于比对：丢掉括号里的内容与所有非字母数字字符，并转小写。
///
/// 「Creep (现场版)」必须与「Creep」对得上 —— 而 LRCLIB 命中不了的时候，
/// 恰恰就是这些带后缀的版本（`norm_title` 在两个源里共用同一套口径）。
fn norm_title(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' | '（' | '[' | '【' => depth += 1,
            ')' | '）' | ']' | '】' => depth -= 1,
            _ if depth <= 0 => {
                if c.is_alphanumeric() {
                    out.extend(c.to_lowercase());
                }
            }
            _ => {}
        }
    }
    out
}

/// 搜索结果里的任一条歌手名是否与我们要找的对得上（双向 `contains`，容忍
/// 「Feat.」这类拼接与简繁差异之外的常见写法）。
fn netease_artist_matches(song: &serde_json::Value, artist: &str) -> bool {
    let want = norm_title(artist);
    if want.is_empty() {
        return true;
    }
    song.get("artists")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter().any(|a| {
                let n = norm_title(a.get("name").and_then(|x| x.as_str()).unwrap_or(""));
                !n.is_empty() && (n.contains(&want) || want.contains(&n))
            })
        })
        .unwrap_or(false)
}

/// 网易云的 GET（带 Referer / UA）→ JSON。
fn netease_get_json(client: &reqwest::blocking::Client, url: &str) -> Result<serde_json::Value, String> {
    let resp = client
        .get(url)
        .header("Referer", NETEASE_REFERER)
        .header("User-Agent", UA)
        .send()
        .map_err(|e| format!("请求网易云失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("网易云返回 {}", resp.status()));
    }
    resp.json().map_err(|e| format!("网易云响应解析失败：{e}"))
}

/// 取网易云的歌词（只返回 `synced`：网易云给的就是 `[mm:ss.xx]` 逐行 LRC）。
fn netease_lyrics(title: &str, artist: &str) -> Option<LyricsDto> {
    let client = http().ok()?;
    let kw = if artist.trim().is_empty() {
        title.to_string()
    } else {
        format!("{title} {artist}")
    };
    let search_url = format!(
        "{NETEASE_API}/search/get/web?csrf_token=&s={}&type=1&offset=0&total=true&limit=10",
        qenc(kw.trim())
    );
    let v = netease_get_json(&client, &search_url).ok()?;
    let songs = v.pointer("/result/songs").and_then(|x| x.as_array())?;
    let want = norm_title(title);
    // 挑选顺序：歌名 + 歌手都对得上 → 只对得上歌名 → 第一条。
    // 最后那条兜底是有意的：宁可给一个「可能不是这个版本」的歌词，也比空着强
    // （面板上会显示歌词来源与曲名，用户看得出来对不对）。
    let pick = songs
        .iter()
        .find(|s| {
            let n = s.get("name").and_then(|x| x.as_str()).unwrap_or("");
            norm_title(n) == want && netease_artist_matches(s, artist)
        })
        .or_else(|| {
            songs.iter().find(|s| {
                norm_title(s.get("name").and_then(|x| x.as_str()).unwrap_or("")) == want
            })
        })
        .or_else(|| songs.first())?;
    let id = pick.get("id").and_then(|x| x.as_i64())?;
    let lv = netease_get_json(
        &client,
        &format!("{NETEASE_API}/song/lyric?id={id}&lv=-1&kv=-1&tv=-1"),
    )
    .ok()?;
    let lrc = lv
        .pointer("/lrc/lyric")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if lrc.is_empty() {
        return None;
    }
    let joined = pick
        .get("artists")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.get("name").and_then(|x| x.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    Some(LyricsDto {
        found: true,
        instrumental: false,
        track: pick
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or(title)
            .to_string(),
        artist: joined,
        album: pick
            .pointer("/album/name")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        duration: pick
            .get("duration")
            .and_then(|x| x.as_f64())
            .map(|ms| ms / 1000.0)
            .unwrap_or(0.0),
        synced: Some(lrc),
        plain: None,
    })
}

// ── Spotify 桌面端探测 + 自动弹出（2026-09-27）────────────────────
//
// 用户的触发条件（逐字）：「这个抓取的时机是在 lunac 应用和 spotify 应用同时存在 …
// 这个时候自动弹出这个音乐插件」，并在追问中选定「只要 Spotify 在运行就弹」。
//
// **判据用「进程存在」而不是「正在播放」**：后者要先有 OAuth 令牌 —— 未登录时永远
// 判不出「在播放」，于是第一次使用的用户永远等不到弹出。进程判据没有这个前置，
// 而且它正是用户说的「两个应用同时存在」。

/// 上一次轮询时 Spotify 是否在跑 —— 自动弹出**只在边沿触发**，见 `spawn_spotify_watcher`。
static SPOTIFY_WAS_RUNNING: AtomicBool = AtomicBool::new(false);

/// 轮询间隔。3 秒 = 「点开 Spotify 后几乎立刻看到面板」与「白烧 CPU」的折中：
/// 探测只是读一次进程快照，**不 spawn 进程、不发网络请求**。
const SPOTIFY_POLL_SECS: u64 = 3;

/// Spotify 桌面客户端的进程名。官网安装包与 Microsoft Store 版**都是这个名字**
/// （Store 版的路径在 `WindowsApps` 下，但映像名一致）。用数组是为了将来加别名时
/// 不动匹配逻辑。
const SPOTIFY_EXE: [&str; 1] = ["Spotify.exe"];

/// 映像名是否算 Spotify 桌面客户端。**忽略大小写**：系统上报的大小写不保证。
/// （与 `spotify_desktop_running()` 分开是为了能单测 —— 那个函数要真读进程表。）
fn exe_name_matches(name: &str) -> bool {
    SPOTIFY_EXE.iter().any(|x| name.eq_ignore_ascii_case(x))
}

/// 本机是否在跑 Spotify 桌面客户端。
///
/// 非 Windows 恒 `false`：本仓只发 Windows 包（`tauri.conf.json` 的 target 与
/// `winreg` / `windows` 依赖都是 Windows 专属）。
pub fn spotify_desktop_running() -> bool {
    #[cfg(target_os = "windows")]
    {
        win_proc::process_names().iter().any(|n| exe_name_matches(n))
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// 进程枚举（raw FFI）。**刻意不引 `sysinfo` 这类新依赖**，也不 spawn `tasklist`：
/// 后者每 3 秒起一个进程、输出还是本地化文本（`tasklist /FI` 无匹配时打的那句
/// 「信息: 没有运行的任务…」随系统语言变，按文本判会误判）。`hotkey.rs` 里已经有一批
/// 手写 `extern "system"` 的同类写法（`user32` / `kernel32` / `shell32`），这里是同一套。
#[cfg(target_os = "windows")]
mod win_proc {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

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

    /// 当前所有进程的**映像名**（不含路径，如 `Spotify.exe`）。读不到就返回空表 ——
    /// 调用方只关心「有没有」，把失败当「没在跑」比抛错更合适（拿不到快照不该打断任何事）。
    pub fn process_names() -> Vec<String> {
        let mut out = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == 0 || snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            // `dwSize` 必须在第一次调用前填好，否则 Process32FirstW 直接失败
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut entry) != 0 {
                loop {
                    let len = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(MAX_PATH);
                    out.push(
                        OsString::from_wide(&entry.szExeFile[..len])
                            .to_string_lossy()
                            .to_string(),
                    );
                    if Process32NextW(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
        out
    }
}

/// 起一个后台轮询线程：Spotify 桌面端**从「没在跑」变成「在跑」**的那一刻，
/// 开一个音乐插件窗（`plugin-music`）。
///
/// 三条刻意的设计：
///
/// 1. **只在边沿触发**（`!running || was` ⇒ 跳过）。若按「只要在跑就确保窗口存在」写，
///    用户刚关掉窗口就会被 3 秒后的下一轮重新拎出来 —— 关掉等于关不掉。
///    边沿语义下「关掉」是有效的：要它再弹，得让 Spotify 退出再启动。
/// 2. **已经开着就不动**（`is_open`）。`plugin_window::open()` 的复用路径会
///    `set_focus()`，从后台线程定时抢焦点是最不该发生的事。
/// 3. **`silent_start` 抑制首轮**：开机自启（`--background`）时把「上一轮」预置成
///    `true`，于是「开机时 Spotify 已经在跑」不构成边沿 —— 那一刻用户并没有在用
///    Lunac，弹出窗口与「静默自启」的设计相冲（见 main.rs 里 `--background` 的说明）。
///    Spotify 之后关掉再开仍是一次真边沿，照常弹。
pub fn spawn_spotify_watcher(app: AppHandle, silent_start: bool) {
    SPOTIFY_WAS_RUNNING.store(silent_start, Ordering::SeqCst);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(SPOTIFY_POLL_SECS));
        let running = spotify_desktop_running();
        let was = SPOTIFY_WAS_RUNNING.swap(running, Ordering::SeqCst);
        if !running || was {
            continue;
        }
        if crate::plugin_window::is_open(&app, MUSIC_PLUGIN_ID) {
            continue;
        }
        match crate::plugin_window::open(&app, MUSIC_PLUGIN_ID, "") {
            Ok(()) => crate::log::info("spotify: 桌面客户端在运行 ⇒ 已自动打开音乐插件窗"),
            Err(e) => crate::log::warn(&format!("spotify: 自动打开音乐插件窗失败（{e}）")),
        }
    });
}

// ── 单元测试 ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spotify_exe_name_match_ignores_case() {
        assert!(exe_name_matches("Spotify.exe"));
        // 系统上报的大小写不保证，不能按字面比
        assert!(exe_name_matches("spotify.exe"));
        assert!(exe_name_matches("SPOTIFY.EXE"));
        // 不能连「名字里含 spotify」的都算（`SpotifyWebHelper.exe` 是另一回事，
        // 且进程名必须整体相等）
        assert!(!exe_name_matches("SpotifyWebHelper.exe"));
        assert!(!exe_name_matches(""));
        assert!(!exe_name_matches("notspotify.exe"));
    }

    #[test]
    fn builtin_client_id_makes_config_default_consistent() {
        // 内置值为空时：内置判定必须为假（面板照旧展开凭据字段）
        let dto = config_dto(&MusicConfig::default());
        assert_eq!(dto.builtin, !BUILTIN_CLIENT_ID.is_empty());
        if !BUILTIN_CLIENT_ID.is_empty() {
            assert_eq!(dto.client_id, BUILTIN_CLIENT_ID);
        }
    }

    #[test]
    fn missing_client_id_field_falls_back_to_builtin() {
        // 老配置文件里没有 client_id 字段 ⇒ 补内置值（不是空串）
        let cfg: MusicConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.client_id, BUILTIN_CLIENT_ID);
    }

    #[test]
    fn norm_title_drops_brackets_and_punctuation() {
        // LRCLIB 命中不了的时候，恰恰就是这些带后缀的版本 —— 必须能和主名对上
        assert_eq!(norm_title("Creep (现场版)"), "creep");
        assert_eq!(norm_title("Creep"), "creep");
        assert_eq!(norm_title("Creep【Live】"), "creep");
        assert_eq!(norm_title("Don't Stop Me Now!"), "dontstopmenow");
        assert_eq!(norm_title("夜曲 - 周杰伦"), "夜曲周杰伦");
    }

    #[test]
    fn spotify_id_and_uri_whitelists_are_narrow() {
        // id 会拼进 URL 路径段，字符集必须收窄
        assert!(is_safe_spotify_id("37i9dQZF1DXcBWIGoYBM5M"));
        assert!(!is_safe_spotify_id(""));
        assert!(!is_safe_spotify_id("a/b"));
        assert!(!is_safe_spotify_id("a?x=1"));
        assert!(!is_safe_spotify_id("../me/player"));
        assert!(!is_safe_spotify_id(&"a".repeat(65)));

        assert!(is_safe_context_uri("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M"));
        assert!(is_safe_context_uri("spotify:collection:tracks"));
        // 不在白名单的上下文类型：那是「随便塞一个 uri」的入口
        assert!(!is_safe_context_uri("spotify:user:someone"));
        assert!(!is_safe_context_uri("spotify:playlist:"));
        assert!(!is_safe_context_uri("http://evil/x"));
        assert!(!is_safe_context_uri("spotify:playlist:a:b"));

        assert!(is_safe_track_uri("spotify:track:4iV5W9uYEdYUVa79Axb7Rh"));
        assert!(is_safe_track_uri("spotify:episode:512ojhOuo1ktJprKbVcKyQ"));
        assert!(!is_safe_track_uri("spotify:album:4iV5W9uYEdYUVa79Axb7Rh"));
        assert!(!is_safe_track_uri("spotify:track:../../me"));
    }

    #[test]
    fn track_from_json_maps_track_and_episode() {
        let t: serde_json::Value = serde_json::from_str(
            r#"{"type":"track","id":"a1","uri":"spotify:track:a1","name":"Nude",
                "duration_ms":200000,"artists":[{"name":"Radiohead"},{"name":"X"}],
                "album":{"name":"In Rainbows","images":[{"url":"http://img/1"}]}}"#,
        )
        .unwrap();
        let dto = track_from_json(&t);
        assert_eq!(dto.uri, "spotify:track:a1");
        assert_eq!(dto.artists, "Radiohead, X");
        assert_eq!(dto.album, "In Rainbows");
        assert_eq!(dto.cover, "http://img/1");

        // 播客没有 artists / album：必须用 show 兜住，否则队列里是一行空白
        let e: serde_json::Value = serde_json::from_str(
            r#"{"type":"episode","id":"e1","uri":"spotify:episode:e1","name":"EP",
                "duration_ms":1000,"images":[{"url":"http://img/e"}],
                "show":{"name":"Some Show","publisher":"Pub"}}"#,
        )
        .unwrap();
        let dto = track_from_json(&e);
        assert_eq!(dto.artists, "Some Show");
        assert_eq!(dto.album, "Pub");
        assert_eq!(dto.cover, "http://img/e");
    }

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
