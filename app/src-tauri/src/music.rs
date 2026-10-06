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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
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
///
/// **`user-top-read` 是 2026-10-01 加的**（用户第 4 条「发现」栏）：`/me/top/artists`
/// 与 `/me/top/tracks` 缺它一律 `403`。代价同上 —— **老用户要再重新登录一次**。
///
/// **「新发行」（`/browse/new-releases`）已于 2026-10-02 整条删除**：它属于 2024-11-27
/// （收紧）与 2026-02-06（Dev Mode 再砍一轮）两批下线，对个人应用**永久 403**，
/// 官方无替代端点、无等待名单。release 日志实测它稳定回 `403 Forbidden` —— 留着就是
/// 一个每次打开都必然失败的入口，用户口径是「**完全解决，而不是加个限流**」。
const SCOPES: &str = "user-read-playback-state user-modify-playback-state user-read-currently-playing playlist-read-private playlist-read-collaborative user-library-read user-follow-read user-top-read";

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

/// 进程内共用的 HTTP 客户端。**必须复用**：
///
/// 每次 `Client::builder().build()` 都是一个**全新的连接池**（新 TLS 会话、新握手），
/// 而音乐面板的每个动作 / 每秒那轮进度轮询都要打一次 Web API ——
/// 2026-09-29 实测延迟里很大一块就是反复重建客户端与 TLS 握手。
/// 复用一个进程级 client 后，同域名的连接会被池子接走（keep-alive），
/// 顺带把「每次动作到功能之间那段延迟」压下来。
static HTTP_CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();

fn http() -> Result<reqwest::blocking::Client, String> {
    HTTP_CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(12))
                .build()
                .map_err(|e| format!("HTTP client: {e}"))
        })
        .clone()
}

// ── Spotify Web API 的「429 硬闸 + 手动恢复」（2026-10-02，取代原「定时冷却」）──
//
// **为什么不是冷却**：release 日志实测（`<exe 根>\temp\logs\lunac-2026-10-02.log`）
// 那组 **31 条** `429 QUOTA_EXCEEDED` 有两个成因，缺一不可：
//   ① **触发**：音乐面板常驻的 **1Hz `GET /me/player` 轮询**（music.ts 的
//      `startPolling`）持续把 **app 级滚动 30s 配额**顶在临界点 —— 任何额外请求
//      （设备查询 / 播放命令 / 队列）都会把它推过线。**真凭**：日志里 15:04:03 打开
//      面板、15:04:05 的**第一次**设备查询就已经是 429，而此前该进程没有任何 Spotify
//      调用 ⇒ 配额是**在那之前**就被持续轮询耗光的。
//   ② **放大器**：Spotify 在 429 期间**继续请求会把封禁窗口越推越长**。所以
//      「等 N 秒自动重试」本身就是错的 —— 那 N 秒里的每一次轮询都在加刑。
// 另有**盲区**：`/me/player` 的 429 以前**不落日志**（`spotify_status` 走裸 `api_get`，
// 非 2xx 直接 return、不调 `log_api_reject`）⇒ 只看得见 devices 的 429，看起来像
// 「devices 端点自己的问题」。已一并补上（见 `spotify_status`）。
//
// 现在的口径（用户 2026-10-02 定）：**一旦出现 429，就拉闸、一个请求都不再发**，
// 直到**用户手动恢复**（`spotify_resume`）。**没有自动过期** —— 什么时候重试由用户
// 决定，而不是由我们猜一个秒数（猜错了就是继续加刑）。
static SPOTIFY_STOPPED: AtomicBool = AtomicBool::new(false);
/// 拉闸原因（给面板显示）。只在 `SPOTIFY_STOPPED = true` 时有意义。
static SPOTIFY_STOP_REASON: Mutex<Option<String>> = Mutex::new(None);
/// 429 响应里 `Retry-After` 的**原值**（秒）；`0` = 响应里没这个头。
///
/// **为什么要读它**（2026-10-03 加）：Spotify 的 429 不是「等 30 秒就好」—— 实测同一个
/// token 打 `/me/player*` 拿到的是 `Retry-After: 45000+`（≈12.5 小时），而且它**换 token
/// 也不重置**（按 app 计的端点级处罚）。所以恢复时机只能由 Spotify 说了算，不能由我们猜。
/// 这也是「按 `Retry-After` 停够」的依据：默认就一直停到它到期。
static SPOTIFY_RETRY_AFTER_SECS: AtomicU64 = AtomicU64::new(0);
/// 「最早可重试时刻」的 unix 毫秒（`SPOTIFY_RETRY_AFTER_SECS` 换出来的绝对时间）。
/// `0` = 没有建议（没有 `Retry-After`）。面板按它算倒计时。
static SPOTIFY_RETRY_AT_MS: AtomicU64 = AtomicU64::new(0);

/// 距「最早可重试时刻」还剩多少秒；`0` = 已到期或没有建议。
fn retry_remaining_secs() -> u64 {
    let at = SPOTIFY_RETRY_AT_MS.load(Ordering::SeqCst);
    if at == 0 {
        return 0;
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    at.saturating_sub(now_ms) / 1000
}

/// 闸是不是拉着；`Some(原因)` = 拉着（`None` = 正常，可以发请求）。
fn spotify_stop_state() -> Option<String> {
    if !SPOTIFY_STOPPED.load(Ordering::SeqCst) {
        return None;
    }
    Some(
        SPOTIFY_STOP_REASON
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or_default(),
    )
}

/// 拉闸（**幂等**；已拉着就不重复记日志）。`retry_after_secs` 来自 429 的 `Retry-After`
/// 响应头（0 = 没有这个头），它决定面板上那个倒计时。
fn trip_spotify_stop(reason: &str, retry_after_secs: u64) {
    if SPOTIFY_STOPPED.swap(true, Ordering::SeqCst) {
        return;
    }
    SPOTIFY_RETRY_AFTER_SECS.store(retry_after_secs, Ordering::SeqCst);
    // 「最早可重试时刻」= 现在 + Retry-After。没有这个头就置 0（面板不显示倒计时，
    // 但仍保持拉闸 —— 恢复时机交给用户，不去猜一个秒数）。
    let at_ms = if retry_after_secs == 0 {
        0
    } else {
        SystemTime::now()
            .checked_add(Duration::from_secs(retry_after_secs))
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    };
    SPOTIFY_RETRY_AT_MS.store(at_ms, Ordering::SeqCst);
    if let Ok(mut g) = SPOTIFY_STOP_REASON.lock() {
        *g = Some(reason.to_string());
    }
    // **只记这一行**。之后所有请求都被闸挡在本地、不出网，所以不会再有日志 ——
    // 这正好治好了原来「每重试一次记一条、31 条噪声」的病。
    // `Retry-After` 必须**如实写进日志**：它可能是 12 小时这种量级，而「等 30 秒再试」
    // 的直觉正好会让人一秒一次地继续打（那就是原来把处罚越推越长的机制）。
    let wait = if retry_after_secs > 0 {
        format!("；Spotify 要求等 {retry_after_secs}s（Retry-After）")
    } else {
        String::new()
    };
    crate::log::warn(&format!(
        "spotify: 收到 429 ⇒ 已停止所有 Spotify 请求（{reason}{wait}）；默认停到 Retry-After 到期，也可由用户在面板上手动恢复"
    ));
}

/// 用户手动恢复：清闸。**只由 `spotify_resume` 命令调**。
fn clear_spotify_stop() {
    SPOTIFY_STOPPED.store(false, Ordering::SeqCst);
    SPOTIFY_RETRY_AFTER_SECS.store(0, Ordering::SeqCst);
    SPOTIFY_RETRY_AT_MS.store(0, Ordering::SeqCst);
    if let Ok(mut g) = SPOTIFY_STOP_REASON.lock() {
        *g = None;
    }
}

// ── 观测：30 秒窗口内的出网请求数（2026-10-02 加）────────────────────
//
// **为什么需要**：上面那个「真因」以前**完全看不见** —— 成功请求不记日志、
// `/me/player` 的失败也不记。有了这个计数，「拉闸那一刻我们有多吵」才有据可查。
// 平时**零噪音**：只在明显异常（30s 内 ≥60 次，平均 ≥2/s ⇒ 有失控的调用方）时
// 提前记一行；拉闸时那行日志会带上当时的窗口计数。
static REQ_WINDOW: Mutex<Option<(Instant, u32)>> = Mutex::new(None);
const REQ_WINDOW_SECS: u64 = 30;
/// 早警阈值：30s 内 60 次 ⇒ 平均 2/s。正常面板（轮询 3s + 偶发操作）约 10–15 次。
const REQ_WINDOW_WARN: u32 = 60;

/// 记一次**真正出网**的请求，返回本窗口内的累计数（含本次）。
fn note_request() -> u32 {
    let mut g = REQ_WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let n = match g.as_mut() {
        Some((start, n)) if now.duration_since(*start).as_secs() < REQ_WINDOW_SECS => {
            *n += 1;
            *n
        }
        _ => {
            *g = Some((now, 1));
            1
        }
    };
    if n == REQ_WINDOW_WARN {
        crate::log::warn(&format!(
            "spotify: {REQ_WINDOW_SECS}s 内已发出 {n} 次 Web API 请求（平均 ≥2/s）—— 有调用方在刷"
        ));
    }
    n
}

/// 发一个 Spotify **Web API** 请求 —— 429 硬闸的**唯一关口**。
///
/// 所有打 `api.spotify.com` 的请求都必须从这里过（`api_get` / `api_send` /
/// `spotify_control` 三处）。少走一处就是「闸拉着期间还有一条路在续命」，
/// 而那正是 429 停不下来的机制。账号域名（`accounts.spotify.com` 的换令牌 /
/// 刷新令牌）**刻意不走这里**：那是另一套配额，被 Web API 的闸挡住会让用户连
/// 「重新登录」都做不了。
fn spotify_send(
    req: reqwest::blocking::RequestBuilder,
) -> Result<reqwest::blocking::Response, String> {
    if spotify_stop_state().is_some() {
        return Err("ERR_SPOTIFY_STOPPED".into());
    }
    let n = note_request();
    let resp = req.send().map_err(|e| format!("请求 Spotify 失败：{e}"))?;
    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        // `Retry-After` 是**秒**（Spotify 实测如此）。解析不出来就按 0 处理 —— 仍然拉闸，
        // 只是不显示倒计时。**必须在这里读**：`resp` 之后要被调用方消费掉（`text()`）。
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        trip_spotify_stop(
            &format!(
                "{}（本 {}s 窗口内第 {} 次请求）",
                resp.status(),
                REQ_WINDOW_SECS,
                n
            ),
            retry_after,
        );
    }
    Ok(resp)
}

/// 「Spotify 请求闸」的状态（`stopped` + 原因 + `Retry-After` 倒计时）。
#[derive(Debug, Serialize, Default)]
pub struct SpotifyApiStateDto {
    pub stopped: bool,
    pub reason: String,
    /// 「最早可重试时刻」的 unix 毫秒；`0` = 没有建议（响应里没有 `Retry-After`）。
    /// 面板按它**本地**算倒计时（给绝对时刻而不是「还剩几秒」，因为倒计时要能自己往下走）。
    /// 它**不阻止**用户手动恢复 —— 用户明确要求保留这个出口。
    pub retry_at_ms: u64,
}

/// 面板查询闸的状态。**面板挂载时查一次** —— 闸是进程级的，面板关掉再打开
/// 必须还能看到它拉着（否则用户以为恢复正常了，其实一条都发不出去）。
#[tauri::command]
pub async fn spotify_api_state() -> SpotifyApiStateDto {
    match spotify_stop_state() {
        Some(reason) => SpotifyApiStateDto {
            stopped: true,
            reason,
            retry_at_ms: SPOTIFY_RETRY_AT_MS.load(Ordering::SeqCst),
        },
        None => SpotifyApiStateDto::default(),
    }
}

/// **手动恢复**（面板上那个「恢复」按钮）：清掉 429 硬闸，之后的请求正常发出。
///
/// 默认口径是「**停到 `Retry-After` 到期**」（见 `trip_spotify_stop`），但这是**用户
/// 明确要求保留的出口** —— 所以这里不挡：想提前试就让他试。若确实是在到期前提前放闸，
/// 日志里如实记下「提前了多少秒」，方便事后判断封禁有没有被推长。
#[tauri::command]
pub async fn spotify_resume() -> SpotifyApiStateDto {
    let left = retry_remaining_secs();
    clear_spotify_stop();
    if left > 0 {
        crate::log::warn(&format!(
            "spotify: 用户手动恢复（比 Retry-After 建议时刻提前 {left}s）⇒ 已放开 429 硬闸"
        ));
    } else {
        crate::log::warn("spotify: 用户手动恢复 ⇒ 已放开 429 硬闸");
    }
    SpotifyApiStateDto::default()
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
    /// **串流质量**（librespot 的 `-b/--bitrate`，单位 kbps）。
    ///
    /// 取值只允许 `LIBRESPOT_BITRATES` 里那三个；**0 = 老配置里没有这个键**，
    /// 由 `effective_librespot_bitrate()` 压回默认档（同 `port` 的兜底写法）。
    /// 改这一项**要重启 librespot 才生效**（参数是启动时读的，面板会提示）。
    #[serde(default)]
    pub librespot_bitrate: u16,
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
            librespot_bitrate: DEFAULT_LIBRESPOT_BITRATE,
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
    /// 质量档兜底：老配置没有这个键（= 0）、或被手工改成了三个档之外的值 ⇒ 回默认档。
    ///
    /// **不能让越界值直接进命令行** —— librespot 拿到不认识的位数会直接启动失败，
    /// 而表现是「本机播放点了没反应」，比面板上少一个勾难查得多。
    fn effective_librespot_bitrate(&self) -> u16 {
        if LIBRESPOT_BITRATES.contains(&self.librespot_bitrate) {
            self.librespot_bitrate
        } else {
            DEFAULT_LIBRESPOT_BITRATE
        }
    }
}

/// 配置的内存缓存（进程内唯一真相源之一，另一个是真磁盘文件）。
///
/// **为什么必须有**：`load_config()` 在音乐模块里被调 30+ 次，其中大半在每秒那轮
/// 进度轮询的路径上（`spotify_status` → 取 token → `load_config`）。每次读盘 + 反序列化
/// 在「每次动作都很慢」体感里是实打实的一份。这里读一次就记住，
/// **`save_config` 写盘成功后同步更新缓存** —— 否则同一进程内会读到旧值。
static CONFIG_CACHE: Mutex<Option<MusicConfig>> = Mutex::new(None);

fn load_config() -> MusicConfig {
    if let Ok(guard) = CONFIG_CACHE.lock() {
        if let Some(cfg) = guard.as_ref() {
            return cfg.clone();
        }
    }
    let cfg = match std::fs::read_to_string(music_config_path()) {
        Ok(text) => serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default(),
        Err(_) => MusicConfig::default(),
    };
    if let Ok(mut guard) = CONFIG_CACHE.lock() {
        *guard = Some(cfg.clone());
    }
    cfg
}

/// 落盘。**凭据类文件写失败必须如实报错** —— 否则会重演「面板看着保存成功、
/// 重启后登录状态没了」这种最难查的问题（与 `set_ai_config` 同一条纪律）。
/// 写成功后**同步内存缓存**（见 `CONFIG_CACHE`）。
fn save_config(cfg: &MusicConfig) -> Result<(), String> {
    let path = music_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("写不进 {}：{e}", path.display()))?;
    if let Ok(mut guard) = CONFIG_CACHE.lock() {
        *guard = Some(cfg.clone());
    }
    Ok(())
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
    /// 串流质量（kbps）。面板上那三档互斥按钮读它；越界值已在 `config_dto` 里兜过底。
    pub librespot_bitrate: u16,
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
        // 回给面板的必须是**兜过底**的值：否则面板会把 0 显示成「没有一档被选中」
        librespot_bitrate: cfg.effective_librespot_bitrate(),
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
    // 走 `spotify_send`：限流冷却与 429 退避的**唯一关口**（见那段注释）。
    let resp = spotify_send(client.get(url).bearer_auth(&token))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        cfg.expires_at = 0; // 强制刷新
        cfg.access_token.clear();
        let token = ensure_access_token(cfg)?;
        let client = http()?;
        return spotify_send(client.get(url).bearer_auth(&token));
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
/// `librespot_*` 几个是 `Option`：**面板只传它真正编辑过的那个**（`None` = 别动），
/// 否则「改个端口」会把用户填的 librespot 路径一起抹掉。
///
/// `librespot_bitrate` 给的值必须落在 `LIBRESPOT_BITRATES` 里，否则**忽略**（保持原值）——
/// 与 `port` 的处理同一口径：非法输入不许把好值冲掉。
#[tauri::command]
pub async fn music_config_set(
    client_id: String,
    port: Option<u16>,
    librespot_path: Option<String>,
    librespot_proxy: Option<String>,
    librespot_bitrate: Option<u16>,
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
        if let Some(b) = librespot_bitrate {
            if LIBRESPOT_BITRATES.contains(&b) {
                cfg.librespot_bitrate = b;
            }
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
            // **必须落盘**（2026-10-02 补的盲区）：这条以前只 `return`，于是 `/me/player`
            // 的 429 **在日志里一个字都没有** ——「1Hz 轮询把 app 配额顶爆」这个真因
            // 因此一直查不出来，看起来像「只有 devices 端点有问题」。现在它和别的端点
            // 一样留痕（有了 429 硬闸之后最多也就这一两行，不会刷屏）。
            log_api_reject("读播放状态", &format!("{SPOTIFY_API}/me/player"), code, &body);
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
            // 选歌可以从「列表里第一台」开始播（见 `resolve_play_device`）。
            "play" => {
                let dev = resolve_play_device(&mut cfg, None);
                ("PUT", play_url(dev.as_deref()))
            }
            // 控制动作**一律不带 `device_id`**（2026-10-02 重修，见下面的 `no_active` 分支）：
            // Spotify 的口径是「给一个**非活跃**设备 id 就回 `403 Restriction violated`」，
            // 所以正确的打法是**对准活跃设备**；一台活跃设备都没有时**先转移**再重发。
            other => {
                let base = match other {
                    "pause" => format!("{SPOTIFY_API}/me/player/pause"),
                    "next" => format!("{SPOTIFY_API}/me/player/next"),
                    "previous" => format!("{SPOTIFY_API}/me/player/previous"),
                    "seek" => format!(
                        "{SPOTIFY_API}/me/player/seek?position_ms={}",
                        value.unwrap_or(0.0).max(0.0) as i64
                    ),
                    "volume" => format!(
                        "{SPOTIFY_API}/me/player/volume?volume_percent={}",
                        value.unwrap_or(50.0).clamp(0.0, 100.0) as i64
                    ),
                    _ => return Err(format!("未知的播放控制动作：{other}")),
                };
                let method = if matches!(other, "next" | "previous") {
                    "POST"
                } else {
                    "PUT"
                };
                (method, base)
            }
        };
        // 同 `api_get` / `api_send`：所有 Web API 请求都过 `spotify_send`
        //（少一处就有一条路在冷却期间继续续命）。
        let resp = spotify_send(
            (match method {
                "PUT" => client.put(&url),
                _ => client.post(&url),
            })
            .bearer_auth(&token)
            .header("Content-Length", "0"),
        )?;
        // Spotify 成功时返回 204（无内容），所以只判状态码。
        if !resp.status().is_success() {
            let code = resp.status();
            let body = resp.text().unwrap_or_default();
            // **404 `NO_ACTIVE_DEVICE` ⇒ 当前一台活跃设备都没有**。官方口径：要操作一台
            // 非活跃设备，**必须先 `PUT /me/player` 把它转移成活跃设备**。把本机 Lunac
            // 扶上去再**重发一次**（转移用 `play:false`，不顺手把歌放起来）。
            // 这一步**取代了**之前「给控制命令塞 device_id」的写法 —— 那条路对非活跃设备
            // 回的是 `403 Restriction violated`（release 日志实测 13:28:58），比 404 更难懂。
            // **只重试一次**：第二次还不成功，就把它自己的真原因报出来。
            if code == reqwest::StatusCode::NOT_FOUND && body.contains("NO_ACTIVE_DEVICE") {
                if let Some(id) = local_device_id(&mut cfg) {
                    // `transfer_to` 成功会自己作废设备快照（见它的注释）。
                    if transfer_to(&mut cfg, &id, false).is_ok() {
                        let resp2 = spotify_send(
                            (match method {
                                "PUT" => client.put(&url),
                                _ => client.post(&url),
                            })
                            .bearer_auth(&token)
                            .header("Content-Length", "0"),
                        )?;
                        if resp2.status().is_success() {
                            return Ok(());
                        }
                        let code2 = resp2.status();
                        let body2 = resp2.text().unwrap_or_default();
                        log_api_reject(
                            &format!("播放控制 {action}（转移后重发）"),
                            &url,
                            code2,
                            &body2,
                        );
                        return Err(format!(
                            "播放控制被拒（{code2}）：{}",
                            body2.chars().take(200).collect::<String>()
                        ));
                    }
                }
            }
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
    let resp = spotify_send(req.bearer_auth(&token))?;
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
///
/// `device_id` 由前端传「当前认下来的那台」（`music_autoconfigure` 的结果）；留空时
/// 宿主自己解析一次（见 `resolve_play_device`）—— 两条路都必须有，否则面板还没
/// 自动就位时点一首歌就又会落到「没有活跃设备」上。
#[tauri::command]
pub async fn spotify_play_context(
    context_uri: String,
    offset_uri: Option<String>,
    device_id: Option<String>,
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
        let dev = resolve_play_device(&mut cfg, device_id.as_deref());
        api_send(&mut cfg, "PUT", &play_url(dev.as_deref()), Some(body))
    })
    .await
}

/// 从某一首开始播（队列项点击）。
///
/// **Spotify 没有「删除队列项」接口**（队列只有读 + 加到下一首），这是 API 的硬限制。
/// 用 `{uris:[uri]}` 播放等价于「从这首开始，丢掉它之前的队列项」—— 能拿到的最接近的效果，
/// 面板上按这个语义标注，不假装能删中间某一项（见 music.ts 的队列区）。
#[tauri::command]
pub async fn spotify_play_uri(uri: String, device_id: Option<String>) -> Result<(), String> {
    run_blocking(move || {
        let u = uri.trim().to_string();
        if !is_safe_track_uri(&u) {
            return Err("ERR_BAD_URI".into());
        }
        let mut cfg = load_config();
        let dev = resolve_play_device(&mut cfg, device_id.as_deref());
        api_send(
            &mut cfg,
            "PUT",
            &play_url(dev.as_deref()),
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
pub async fn spotify_play_uris(uris: Vec<String>, device_id: Option<String>) -> Result<(), String> {
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
        let dev = resolve_play_device(&mut cfg, device_id.as_deref());
        api_send(
            &mut cfg,
            "PUT",
            &play_url(dev.as_deref()),
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

/// 你最常听的歌手（`/me/top/artists`，近 4 周）。
/// **需要 `user-top-read`**（见 `SCOPES`）—— 老令牌没有它 ⇒ 403 `Insufficient client scope`，
/// 面板会如实把那句话显示成这一栏的内容（不是「这一栏是空的」）。
#[tauri::command]
pub async fn spotify_top_artists() -> Result<Vec<LibraryItemDto>, String> {
    run_blocking(|| {
        let mut cfg = load_config();
        if !cfg.has_token() {
            return Err("ERR_NOT_CONNECTED".into());
        }
        let v = api_get_json(
            &mut cfg,
            &format!("{SPOTIFY_API}/me/top/artists?limit=50&time_range=short_term"),
        )?;
        Ok(library_items(&v, "/items", "artist"))
    })
    .await
}

/// 你最常听的歌曲（`/me/top/tracks`，近 4 周）。返回形状与 `/me/tracks`（收藏夹）同源，
/// 所以**播放也只能逐条走 `spotify_play_uris`** —— 它没有可播的 `context_uri`。
#[tauri::command]
pub async fn spotify_top_tracks() -> Result<Vec<TrackDto>, String> {
    run_blocking(|| {
        let mut cfg = load_config();
        if !cfg.has_token() {
            return Err("ERR_NOT_CONNECTED".into());
        }
        let v = api_get_json(
            &mut cfg,
            &format!("{SPOTIFY_API}/me/top/tracks?limit=50&time_range=short_term"),
        )?;
        let items = v
            .get("items")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter(|x| !x.is_null())
                    // 本机文件 / 下架曲目没有可播 uri ⇒ 滤掉，不给「点了播不了」的死条目
                    .map(track_from_json)
                    .filter(|t| is_safe_track_uri(&t.uri))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(items)
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
///
/// `Clone` 是**设备快照**（`DEVICE_SNAP`）要的：快照命中时要把整份列表复制出去。
#[derive(Debug, Serialize, Default, Clone)]
pub struct DeviceDto {
    pub id: String,
    pub name: String,
    /// `Computer` / `Smartphone` / `Speaker` / `TV` / `Automobile`…（前端只当图标位用）
    pub kind: String,
    /// 正在收声的那一台。**切换设备时要知道「现在在哪」**，所以必须回传。
    pub active: bool,
    pub volume_percent: i64,
}

/// 拉设备列表。`spotify_devices`（工具条那个「设备」按钮）与播放目标解析
/// （`resolve_play_device` / `music_autoconfigure`）共用这一份解析 —— 三处各写一遍
/// 字段一改名就必然漏掉一处（`items.total` 那次吃过这个亏）。
fn fetch_devices(cfg: &mut MusicConfig) -> Result<Vec<DeviceDto>, String> {
    let v = api_get_json(cfg, &format!("{SPOTIFY_API}/me/player/devices"))?;
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
}

// ── 设备列表快照（2026-10-02 加，429 的**根治**）─────────────────────
//
// **为什么要有它**：`GET /me/player/devices` 是所有 Spotify 端点里最容易撞配额的一条
// —— release 日志实测（`<exe 根>\temp\logs\lunac-2026-10-02.log`）里 **31 条 429
// `QUOTA_EXCEEDED` 全部是它**，而且一次面板打开就能打出十条（自动就位 + 解析播放目标
// + 解析控制目标 + 面板拉设备列表，全在一两秒内各要一次）。设备列表在秒级内几乎不变，
// 那些请求**全是真的重复** —— 之前加的冷却只是把噪音压下去，这里才是把请求数压下去。
//
// **持锁做请求 ⇒ 天然单飞**：同一时刻只允许一个真正的网络请求，其余调用者在锁上排队，
// 拿到锁之后会先看到刚写回、足够新的快照，直接复用、不再出网。
static DEVICE_SNAP: Mutex<Option<(Instant, Vec<DeviceDto>)>> = Mutex::new(None);

/// 快照的保鲜期。**故意短**：控制命令（尤其 pause 的 404 分支）要的是「此刻谁是活跃
/// 设备」，太久会拿陈旧状态去操作。1.5s 足够把同一轮操作里的重复请求合并干净。
const DEVICE_SNAP_TTL: Duration = Duration::from_millis(1500);

/// 读设备列表（带快照）。`force_fresh = true` **不吃缓存** —— 等 librespot 注册的
/// 轮询、以及刚吃过 404 要定位本机设备时必须这样，否则看到的是启动前的旧列表。
fn devices_shared(cfg: &mut MusicConfig, force_fresh: bool) -> Result<Vec<DeviceDto>, String> {
    let mut g = DEVICE_SNAP.lock().unwrap_or_else(|e| e.into_inner());
    if !force_fresh {
        if let Some((at, devs)) = g.as_ref() {
            if at.elapsed() <= DEVICE_SNAP_TTL {
                return Ok(devs.clone());
            }
        }
    }
    let devs = fetch_devices(cfg)?;
    *g = Some((Instant::now(), devs.clone()));
    Ok(devs)
}

/// 我们刚改过设备状态（`transfer_to`）⇒ 作废快照，避免下一次读到「旧的活跃设备」。
fn invalidate_devices() {
    *DEVICE_SNAP.lock().unwrap_or_else(|e| e.into_inner()) = None;
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
        // 走快照：面板打开时会连拉好几次设备列表，1.5s 内只出一次网。
        devices_shared(&mut cfg, false)
    })
    .await
}

/// 这台设备是不是我们的本机播放（librespot）。
///
/// **只能按名字认**：librespot 上报的 `type` 也是 `Computer`，与 Spotify 桌面端
/// **完全一样**，按类型分不开（设备 id 每次启动都变，也不能存下来比）。所以设备名
/// `LIBRESPOT_DEVICE_NAME` 是唯一的标识 —— 改那个常量就等于换一台「新设备」。
fn is_local_device(d: &DeviceDto) -> bool {
    d.name == LIBRESPOT_DEVICE_NAME
}

/// 播放前定「在哪台设备上放」（2026-09-30）。
///
/// **为什么必须有它**：`PUT /me/player/play` 不带 `device_id` 时，Spotify 落到
/// **当前活跃设备**。桌面端没开、librespot 没起（或起来了但没被转移过）时，
/// **一台活跃设备都没有** ⇒ `404 NO_ACTIVE_DEVICE`，面板上的表现就是
/// 「桌面端不在时选不了歌」。所以每次播放都要**自己点一台设备**，不能指望「活跃设备」。
///
/// 优先级：**显式指定 → 活跃的 → 本机播放（Lunac）→ 列表里的第一台**。
/// 一台都没有时返回 `None`，调用方照旧不带 `device_id` —— 那时那句
/// `NO_ACTIVE_DEVICE` 才是准确的错误信息，比我们自己编一个更有用。
fn resolve_play_device(cfg: &mut MusicConfig, explicit: Option<&str>) -> Option<String> {
    if let Some(id) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        if is_safe_spotify_id(id) {
            return Some(id.to_string());
        }
    }
    let devs = devices_shared(cfg, false).ok()?;
    if let Some(d) = devs.iter().find(|d| d.active) {
        return Some(d.id.clone());
    }
    if let Some(d) = devs.iter().find(|d| is_local_device(d)) {
        return Some(d.id.clone());
    }
    devs.first().map(|d| d.id.clone())
}

/// `/me/player/play` 的 URL。
///
/// **`device_id` 是查询参数，不是 body 字段**：塞进 body 会被 Spotify 静默忽略、
/// 行为退化成「还是落在活跃设备上」，症状与没传一模一样（很难查）。
fn play_url(device_id: Option<&str>) -> String {
    match device_id {
        Some(id) => format!("{SPOTIFY_API}/me/player/play?device_id={id}"),
        None => format!("{SPOTIFY_API}/me/player/play"),
    }
}

/// 本机播放（librespot）那台设备的 id —— 控制类命令在「没有活跃设备」时要把播放转移给它。
///
/// **为什么需要它**（2026-10-02 重修控制路径）：`pause / next / previous / seek / volume`
/// **不带 `device_id`** 时只认**活跃设备**，而 librespot 与 Spotify 的长连接每 ~36 分钟
/// 断一次、重连后 librespot 自己报 `active device is <>`，此后控制端点条条
/// `404 NO_ACTIVE_DEVICE`（用户看到的就是「听久了突然控制不动了」）。官方口径是
/// 「要操作非活跃设备，**必须先 `transferPlayback`**」，所以这里拿到 Lunac 的 id、
/// 转移过去再重发（见 `spotify_control` 的 404 分支）。
///
/// **只按名字认**（`is_local_device`），**绝不去挑「列表里第一台」**：把 `pause` 打到
/// 用户的手机上（一台与我们无关的设备）是**静默打在错的地方**，比明确报错更糟。
/// 一台都对不上时返回 `None`，让 Spotify 那句 `NO_ACTIVE_DEVICE` 原样报出来。
fn local_device_id(cfg: &mut MusicConfig) -> Option<String> {
    // **强制刷新**：刚吃过 404，要的是**此刻**的设备列表，不能吃快照。
    let devs = devices_shared(cfg, true).ok()?;
    devs.iter()
        .find(|d| is_local_device(d))
        .map(|d| d.id.clone())
}

/// 把播放转移到某一台设备（`PUT /me/player` 的 body）。
///
/// 成功即**作废设备快照**：`active` 标记刚被我们改过，缓存里的那份已经不对了。
/// 放在这里而不是每个调用点，是为了「以后再加一处 transfer 也不会漏」。
fn transfer_to(cfg: &mut MusicConfig, device_id: &str, play: bool) -> Result<(), String> {
    let r = api_send(
        cfg,
        "PUT",
        &format!("{SPOTIFY_API}/me/player"),
        Some(serde_json::json!({ "device_ids": [device_id], "play": play })),
    );
    if r.is_ok() {
        invalidate_devices();
    }
    r
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
        transfer_to(&mut cfg, &device_id, play)
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

/// 串流质量的可选档（librespot 的 `-b/--bitrate`）。
///
/// **只认这三个值**：本机这份 0.8.0 的 `--help` 原文是
/// `Bitrate (kbps) {96|160|320}. Defaults to 160.` —— 所以面板不做自由输入，
/// 也不接受其它数字（`effective_librespot_bitrate()` 会把越界值压回默认档）。
pub const LIBRESPOT_BITRATES: [u16; 3] = [96, 160, 320];

/// 默认档 = **320（最高）**。
///
/// 刻意**不同于 librespot 自己的默认 160**：用户装「本机播放」是为了在这台机器上出声，
/// 默认给最高档才符合预期；要省流量的人去面板下调（这也正是这个设置项存在的理由）。
pub const DEFAULT_LIBRESPOT_BITRATE: u16 = 320;

/// 音频缓存上限（librespot 的 `-M/--cache-size-limit`）。
///
/// **必须给**：不给就是**无上限**（dev 环境实测已堆到 104.7 MB），而 320kbps 下
/// 「每天听 1 小时」一个月就能到 GB 级。2G ≈ 一个多月的收听量，超出部分由 librespot
/// 自己按 LRU 淘汰 —— 用户不必为了「它到底会占多大」去猜。
const LIBRESPOT_CACHE_LIMIT: &str = "2G";

/// 正在跑的 librespot 子进程。`None` = 没在跑。
///
/// 存住 `Child` 才能 kill；**`Mutex<Option<Child>>` 用 `const` 初始化**（不需要 OnceLock）。
static LIBRESPOT: Mutex<Option<Child>> = Mutex::new(None);

/// librespot 的凭据 / 音频缓存目录（`-c`）。放在 `music.json` 旁边：
/// ① 与令牌同处一地，用户备份 / 清账号时不会漏；② 下次启动直接复用，**不用再登一次**。
fn librespot_cache_dir() -> PathBuf {
    music_config_path().with_file_name("librespot-cache")
}

/// librespot 的凭据文件（登录一次的产物）。
///
/// **落点由 `-C/--system-cache` 决定，不是 `-c`**：`-c` 只管音频缓存，凭据与音量归
/// `--system-cache`。两个参数都指到同一个目录（见 `librespot_start_inner`），
/// 于是凭据恒在 `<exe 根>\config\librespot-cache\credentials.json` —— 这条路径同时是
/// 「手工放一份凭据」那条退路的说明（ai-spec §4.6 已知缺口 1）。
fn librespot_credentials_path() -> PathBuf {
    librespot_cache_dir().join("credentials.json")
}

/// librespot OAuth 回调端口（`-K/--oauth-port`）。
///
/// **必须显式给**：不给时它 bind 的是端口 0（随机），回调地址就成了
/// `http://127.0.0.1:<随机端口>/login` —— 而 Spotify 要求 `redirect_uri` 与后台登记的
/// **完全一致**，随机端口永远登记不上。固定成 8898 之后，用户只要在 Spotify 开发者后台
/// 把这个地址加进 Redirect URIs 一次即可（与 Web API 那条 8899/callback 互不影响）。
/// 8898 也是 librespot 文档里的惯用值（8899 已被本仓的 Web API 环回占用）。
const LIBRESPOT_OAUTH_PORT: u16 = 8898;

/// 找 librespot 可执行文件。顺序：**配置里填的 → 与 Lunac 同目录 → 插件目录 → cargo 的 bin 目录**。
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
    // 市场装的音乐插件把它当**依赖**拉下来时落在插件目录里（清单 `dependencies` 的 dest，
    // 见 plugin_market::install_dependencies）。这条是终端用户机器上的**正规落点** ——
    // 没它的话「插件装好了、本机播放却找不到 exe」，用户只能自己去填路径。
    // 排在 exe 同目录之后：那一份是随包发的（更可控），插件目录这份是后装的。
    cands.push(
        crate::plugin_market::plugins_dir()
            .join("music")
            .join("bin")
            .join("librespot.exe"),
    );

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
    /// **有没有凭据**（`librespot-cache\credentials.json`）。
    ///
    /// 这是「本机播放能不能真的用」的硬门槛（2026-09-30）：没有凭据的 librespot
    /// **不会成为一台已登录的 Connect 设备**（本机的 mDNS 又不通，退化的本地发现也走不了），
    /// 于是设备列表里永远没有「Lunac」、播放请求条条 `404 NO_ACTIVE_DEVICE`。
    /// 面板据此把弹层里那行换成「首次登录」，而不是给一个点了也没用的开关。
    pub has_credentials: bool,
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
        has_credentials: librespot_credentials_path().is_file(),
        path: path.map(|p| p.display().to_string()).unwrap_or_default(),
    }
}

/// 读当前状态（面板打开设备弹层时调它）。
#[tauri::command]
pub async fn librespot_status() -> Result<LibrespotDto, String> {
    run_blocking(|| Ok(librespot_dto(&load_config()))).await
}

/// librespot 的**完整参数表**（纯函数 —— 起进程那一段没法断言，参数表能）。
///
/// 每一条都有理由，**删任何一条之前先看清注释**；新增参数请同步补
/// `librespot_args_pin_every_flag` 的断言，否则下次有人「顺手清理」就没了。
///
/// `cache` 由调用方给（`librespot_cache_dir()`），便于单测传临时路径。
fn librespot_args(cfg: &MusicConfig, cache: &std::path::Path, with_login: bool) -> Vec<String> {
    let cache_dir = cache.display().to_string();
    let mut a: Vec<String> = vec![
        "--name".into(),
        LIBRESPOT_DEVICE_NAME.into(),
        "-c".into(),
        cache_dir.clone(),
        // **凭据与音量归 `-C`（system-cache），不是 `-c`**：`-c` 只管音频缓存。
        // 之前只给了 `-c`，凭据就会落到 librespot 自己的默认位置、**不在我们文档写的路径上**
        // （`<exe 根>\config\librespot-cache\credentials.json`）—— 那正是「手工放一份凭据」
        // 那条退路一直对不上的原因。两个参数指向同一目录，落点就唯一了。
        "-C".into(),
        cache_dir,
        // **`-t` 必须显式给**（2026-09-30 补）：不给时「边下边放的临时文件」走 librespot
        // 自己的默认位置 —— 那份默认我们没能从二进制里确证落在哪（`--help` 只写
        // "Path to a directory where files will be temporarily stored while downloading"，
        // 没写默认值）。显式钉在我们自己的目录下，这条不确定性就没了。
        "-t".into(),
        cache.join("tmp").display().to_string(),
        // **`-M` 必须显式给**：不给就是**无上限**（见 LIBRESPOT_CACHE_LIMIT）。
        "-M".into(),
        LIBRESPOT_CACHE_LIMIT.into(),
        // 质量档。**改动只在启动时生效** ⇒ 面板上调完要重启 librespot。
        //
        // 刻意**不给** `-R/--initial-volume` 或 `-E/--volume-ctrl`：音量交给 Web API 那个
        // 音量条（`spotify_set_volume`），不用它本地的软音量 —— 两套音量会互相打脸。
        "--bitrate".into(),
        cfg.effective_librespot_bitrate().to_string(),
    ];
    // ── `-B pipe -f F32`（2026-10-04）：把 librespot 的输出改成**裸 PCM 走 stdout** ──
    // 这样音频才流经我们进程，被 `live_audio.rs` 套上**同一条调音链**（见那个模块的文件头）。
    // 三条都要记住，改这段之前先读：
    //   ① **只在非登录那次加**（`!with_login`）：登录流程里 `librespot-oauth` 的
    //      `set_auth_url()` 会 `println!("Browse to: {auth_url}")` —— 那也是 **stdout**。
    //      那行 ASCII 混进 PCM 轻则一声爆音，重则让后面的 f32 **整条错位 4 字节**（全是噪声）。
    //      所以**首次授权那一次**仍用 librespot 自己的后端出声（此时调音不生效，授权完
    //      重启一次就切到这条路上）。
    //   ② **必须常开、不能按调音总开关决定**：调音开关是**运行时**状态，而启动参数只读一次
    //      ⇒ 按开关决定参数会让用户每次开关调音都得重启 librespot（掉线 + 重新 Connect）。
    //      常开的代价是「在线音乐的播放链路归我们」—— 直通时我们只是不做 DSP。
    //   ③ `-f F32` 显式给：librespot 内部是 f64，F32 少一次多余的量化，也不依赖默认档。
    if !with_login {
        a.push("-B".into());
        a.push("pipe".into());
        a.push("-f".into());
        a.push("F32".into());
    }
    // **`-x` 也归这张表**（2026-10-02 收口）：代理是启动参数、进程中途改不了，所以
    // 「改代理」= 落盘 + 重起（见 `librespot_set_proxy`）。此前它是在
    // `librespot_start_inner` 里**另外追加**的 ⇒ 「参数表只有一个出题人」这条纪律
    // （预检 #46 ①）在它身上不成立，单测钉不到、被「顺手清理」也看不见。
    //
    // 形态是 **HTTP 代理**，不是 socks5 —— 这不是我们的偏好，是本机那份 0.8.0
    // `--help` 的原文：`-x, --proxy URL  HTTP proxy to use when connecting.`
    // （官方 docker 镜像的 `PROXY` 变量同样写明 "should be an HTTP proxy in the form
    // http://ip:port"）。填 `socks5://` 的坏表现是「日志里有 Using proxy、就是连不上」，
    // 查起来毫无线索 ⇒ 由 `validate_librespot_proxy` 在写盘前挡住。
    let proxy = cfg.librespot_proxy.trim();
    if !proxy.is_empty() {
        a.push("-x".into());
        a.push(proxy.to_string());
    }
    if with_login {
        // `-K` 必须显式给：不给时回调端口是随机的，而 Spotify 要求 `redirect_uri`
        // 与后台登记**完全一致** ⇒ 随机端口永远登记不上（见 LIBRESPOT_OAUTH_PORT）。
        a.push("-j".into());
        a.push("-K".into());
        a.push(LIBRESPOT_OAUTH_PORT.to_string());
    }
    a
}

/// 起本机播放（`librespot_start` / `librespot_login` / `music_autoconfigure` **共用这一份**）。
/// 已经活着就直接回状态（点两次不该起两个进程）。
///
/// `with_login = true` 时带 `-j/--enable-oauth`：**一台机器只需要走一次**，否则它没有凭据、
/// 压根不会成为一台已登录的 Connect 设备（见 `LibrespotDto::has_credentials`）。
/// 带上之后 **librespot 自己会打开系统浏览器**跳到 Spotify 授权页（它链接了 webbrowser），
/// 用户在浏览器里点一次授权，凭据随后落到 `-C` 指定的目录；此后再正常启动就有凭据了。
fn librespot_start_inner(cfg: &MusicConfig, with_login: bool) -> Result<LibrespotDto, String> {
    let cur = librespot_dto(cfg);
    if cur.running {
        return Ok(cur);
    }
    let exe = find_librespot(cfg).ok_or_else(|| "ERR_NO_LIBRESPOT".to_string())?;
    let cache = librespot_cache_dir();
    std::fs::create_dir_all(&cache).map_err(|e| format!("建不了 librespot 缓存目录：{e}"))?;
    // 临时目录也先建出来：librespot 自己也会建，但失败时它只 warn，表现成「下载莫名其妙报错」。
    let _ = std::fs::create_dir_all(cache.join("tmp"));

    let mut cmd = std::process::Command::new(&exe);
    cmd.args(librespot_args(cfg, &cache, with_login));
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    // （`-x` 已经由 `librespot_args` 出题，这里**不许**再追加一份 —— 两份必然漂移）
    // 不加这个会**闪一个黑色控制台窗口**（librespot 默认带 console subsystem）
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| format!("起不了 librespot：{e}"))?;
    // **绑到 lunac.exe 的生命周期上**（预检 #57，2026-10-02）：librespot 是插件起的
    // 独立进程，正常路径由 `on_plugin_window_destroyed` / `kill_librespot` 显式杀，
    // 但那两条都要求「我们自己的代码还在跑」。宿主崩溃 / 被强杀时它们没机会执行，
    // 子进程就成了孤儿（本机实测过 2 个：父进程已不在、仍占着 Spotify 里那台 `Lunac`）。
    // Job Object 是唯一能在那个时刻动手的东西 —— 内核替我们收。
    crate::child_job::assign(&child);
    // ── stdout 是**音频**、stderr 是**日志**（2026-10-04 起）────────────────────
    // 非登录启动带了 `-B pipe`（见 `librespot_args`）⇒ stdout 是一条**裸 PCM** 流。
    // 所以这里**不许**再把 stdout 当文本排空：那会既往日志里灌乱码、又让 librespot
    // 因为管道塞满而卡住（表现是「在放但没声音」，且日志里什么都不显示）。
    // 实测依据（本机 0.8.0）：`-B pipe` 启动 8 秒，stdout **0 字节**，stderr 里是
    // `Using StdoutSink (pipe) with format: F32` —— librespot 的日志一律走 stderr。
    // （登录那次没带 `-B pipe`，stdout 是空的；照样交给 `live_audio` 也无害。）
    if let Some(e) = child.stderr.take() {
        drain_librespot_log(e);
    }
    if let Some(o) = child.stdout.take() {
        if let Err(err) = crate::live_audio::start(o) {
            crate::log::warn(format!("librespot: 在线音频链没接上（{err}）—— 这一轮不做调音"));
        }
    }
    *LIBRESPOT.lock().map_err(|e| e.to_string())? = Some(child);
    crate::log::info(&format!(
        "librespot: 已启动本机播放（{}）{}",
        exe.display(),
        if with_login { "，等待浏览器里的授权（-j）" } else { "" }
    ));
    Ok(librespot_dto(cfg))
}

/// 停本机播放的内层（命令与 `librespot_login` 共用）。**先暂停再杀** —— 见本节开头第 4 条。
fn librespot_stop_inner(cfg: &MusicConfig) -> LibrespotDto {
    // 暂停失败不算错：可能本来就没在播，或者令牌刚过期。
    if cfg.has_token() {
        let mut c = cfg.clone();
        let _ = api_send(&mut c, "PUT", &format!("{SPOTIFY_API}/me/player/pause"), None);
    }
    if let Ok(mut slot) = LIBRESPOT.lock() {
        if let Some(mut child) = slot.take() {
            let _ = child.kill();
            let _ = child.wait();
            crate::log::info("librespot: 已停止本机播放");
        }
    }
    // 在线播放链跟着一起收（**顺序是「先杀进程、再收链」**：reader 线程卡在
    // `read()` 上，进程一走 stdout 才 EOF；这里只负责把输出流还回系统）。
    crate::live_audio::stop();
    librespot_dto(cfg)
}

/// 起本机播放（用户点设备弹层里那个开关）。
#[tauri::command]
pub async fn librespot_start() -> Result<LibrespotDto, String> {
    run_blocking(|| librespot_start_inner(&load_config(), false)).await
}

/// **首次登录**（`-j/--enable-oauth`）—— 一台机器只需要走一次。
///
/// 为什么必须要有这个入口：**没有凭据的 librespot 压根不是一台已登录的 Connect 设备**
/// （本机 mDNS 还不通，连退化的本地发现都走不了）⇒ 设备列表里永远没有「Lunac」、
/// 点歌条条 `404 NO_ACTIVE_DEVICE`。2026-09-30 在用户的 release 安装上实测就是这个状态
/// （缓存里只有空的 `files\`，见 ai-spec §4.6 已知缺口 1）。
///
/// librespot 会**自己打开系统浏览器**跳到 Spotify 授权页；授权完成后凭据落盘，
/// **同一个进程会继续以设备的身份跑**，所以这里不等它、也不轮询 ——
/// 面板那边照旧用 `librespot_status` / `spotify_devices` 看结果。
///
/// 前置条件（代码改不掉的那一条）：Spotify 要求 `redirect_uri` 与开发者后台登记**完全一致**，
/// 所以该 Client ID 的应用里得先把 `http://127.0.0.1:8898/login` 加进 Redirect URIs。
#[tauri::command]
pub async fn librespot_login() -> Result<LibrespotDto, String> {
    run_blocking(|| {
        let cfg = load_config();
        let cur = librespot_dto(&cfg);
        if !cur.available {
            return Err("ERR_NO_LIBRESPOT".into());
        }
        // 已经有凭据 ⇒ 不必再登（按钮此时也不该出现，这里兜一下）。
        if cur.has_credentials {
            return Ok(cur);
        }
        // 正在跑的那份是「没凭据」的：**同一个进程不能中途补登**，先停掉再用 `-j` 起。
        librespot_stop_inner(&cfg);
        librespot_start_inner(&cfg, true)
    })
    .await
}

/// librespot 的 `-x` 只认 **HTTP 代理**（`http://host:port` / `https://host:port`）。
///
/// 判据来自本机那份 0.8.0 的 `--help` 原文（`-x, --proxy URL  HTTP proxy to use when
/// connecting.`）与官方 docker 镜像的 `PROXY` 说明（"should be an HTTP proxy in the form
/// http://ip:port"）。**必须挡住 socks5**：放过去的表现是「写盘成功、日志里有
/// `Using proxy`、就是连不上」，而那种错查起来毫无线索。空串 = 直连，合法。
///
/// ⚠️ 这条校验**只加在这一条命令上**（`librespot_set_proxy`，即代理插件那条**程序生成**的
/// 路径）：音乐插件里那个手填框（`music_config_set`）历史上不做校验，现在收紧会把某个
/// 用户已经存着的值变成「保存失败」—— 那是另一件事，不在本轮改。
fn validate_librespot_proxy(proxy: &str) -> Result<(), String> {
    let s = proxy.trim();
    if s.is_empty() {
        return Ok(());
    }
    if s.chars().any(|c| c.is_whitespace()) {
        return Err("代理地址里不能有空格或换行".into());
    }
    if !s.starts_with("http://") && !s.starts_with("https://") {
        return Err(format!(
            "librespot 只认 HTTP 代理（形如 http://127.0.0.1:7890），收到：{s}"
        ));
    }
    Ok(())
}

/// 改**本机播放的代理**并**按需重起**（2026-10-02，代理插件「一键联动」用）。
///
/// 为什么要有这条命令、而不是让代理插件直接调 `music_config_set`：代理是 librespot 的
/// **启动参数**，进程中途改不了 ⇒ 「落盘 + 重起」必须一起做，而重起只有宿主做得到
/// （进程在宿主手里，见 ai-spec §4.6）。把这两件事放在插件里做，就会长出第二份
/// 「librespot 什么时候该重起」的判断。
///
/// 三条纪律：
/// ① **值没变就直接返回**（不重起）—— 重起会打断正在放的那一首，而「没变化」是这条
///    命令最常见的一次调用（插件每次渲染都可能回填一遍）；
/// ② **没在跑就只落盘**（下次启动自然带上 `-x`）；
/// ③ 重起用 `with_login = false`：能起在跑就说明凭据已经有了，不该再弹一次浏览器授权。
#[tauri::command]
pub async fn librespot_set_proxy(proxy: String) -> Result<LibrespotDto, String> {
    run_blocking(move || {
        validate_librespot_proxy(&proxy)?;
        let mut cfg = load_config();
        let want = proxy.trim().to_string();
        if cfg.librespot_proxy == want {
            return Ok(librespot_dto(&cfg));
        }
        cfg.librespot_proxy = want;
        save_config(&cfg)?;
        crate::log::info(&format!(
            "librespot: 代理已改为 {}",
            if cfg.librespot_proxy.is_empty() {
                "直连".to_string()
            } else {
                cfg.librespot_proxy.clone()
            }
        ));
        if !librespot_dto(&cfg).running {
            return Ok(librespot_dto(&cfg));
        }
        // 先停再起：`-x` 只认启动那一刻的值。
        librespot_stop_inner(&cfg);
        librespot_start_inner(&cfg, false)
    })
    .await
}

/// 「播放在哪」自动就位的结果（`music_autoconfigure`）。
#[derive(Debug, Serialize, Default)]
pub struct AutoconfigDto {
    /// `official`（桌面端 / 手机 / 音箱等非 Lunac 设备）/ `local`（本机 librespot）/ `none`。
    pub source: String,
    pub device_id: String,
    pub device_name: String,
    pub device_kind: String,
    /// **本机事实**：Spotify 桌面客户端进程在跑。设备列表还没认到它时这条也有用。
    pub desktop_running: bool,
    /// 这一次调用**把 librespot 拉起来了**（之前没跑）—— 面板据此提示一句。
    pub started_local: bool,
}

/// 打开音乐面板时调一次：把「播放在哪」定下来，必要时**自动拉起本机播放**。
///
/// 规则（用户 2026-09-30 定）：
///   ① **已有活跃设备** ⇒ 原样认它（那是用户自己的现状，不去抢）；
///   ② 没有活跃设备、但有**官方客户端**（设备列表里非 `Lunac` 的那几台：桌面端 / 手机 /
///      音箱 / TV）⇒ 转移过去 —— 这就是「有官方端就用官方端」；
///   ③ 桌面端进程在跑但**还没注册成设备** ⇒ 只回报、**不起 librespot**：两个设备抢同一个
///      会话是 `NO_ACTIVE_DEVICE` 的经典成因（见本节开头第 4 条），等下一次轮询更稳；
///   ④ 一个官方端都没有 ⇒ 拉起 librespot → 等它注册（最多约 6 秒）→ 转移过去；
///      **拉不起来**（没有 exe / 起不来）时 `source = "start_failed"`（2026-09-30 从 `none`
///      里分出来）—— 面板据此说「手动打开一下」，而 `none` 留给「压根没设备」那条路。
///      细分这一步是有用的：用户看到「没有可用设备」会去开 Spotify，而真相是
///      「我们试过了、没起起来」，该做的动作是去设备列表里手动点一下。
///
/// **为什么非要「转移」这一步**：`spotify_control`（播放/暂停/切歌/进度/音量）打的都是
/// 不带 `device_id` 的端点，只认**活跃设备**；只是「设备列表里有 Lunac」并不够。
///
/// `allow_local = false` 表示**用户在本次会话里手动关过本机播放** —— 那就不要第 ④ 步，
/// 否则「我刚关掉的进程，重开面板又被拉起来」会被当成 bug（用户没要求重启它）。
#[tauri::command]
pub async fn music_autoconfigure(allow_local: Option<bool>) -> Result<AutoconfigDto, String> {
    run_blocking(move || {
        let allow_local = allow_local.unwrap_or(true);
        let mut cfg = load_config();
        if !cfg.has_token() {
            return Err("ERR_NOT_CONNECTED".into());
        }
        let desktop = spotify_desktop_running();
        // 走快照：面板挂载与「刚授权成功」会各触发一次自动就位，这一份足够新即可。
        let devs = devices_shared(&mut cfg, false).unwrap_or_default();

        // ① 已有活跃设备：用户的现状，直接认（不转移、不动他正在听的东西）。
        if let Some(d) = devs.iter().find(|d| d.active) {
            let local = is_local_device(d);
            return Ok(AutoconfigDto {
                source: if local { "local" } else { "official" }.into(),
                device_id: d.id.clone(),
                device_name: d.name.clone(),
                device_kind: d.kind.clone(),
                desktop_running: desktop,
                started_local: false,
            });
        }

        // ② 官方客户端优先（有活跃的不算，上面已经返回）。
        if let Some(d) = devs.iter().find(|d| !is_local_device(d)) {
            // 转移只为「让控制端点有个活跃设备」，**不顺手放歌**（那会覆盖用户的暂停）。
            let _ = transfer_to(&mut cfg, &d.id, false);
            return Ok(AutoconfigDto {
                source: "official".into(),
                device_id: d.id.clone(),
                device_name: d.name.clone(),
                device_kind: d.kind.clone(),
                desktop_running: desktop,
                started_local: false,
            });
        }

        // ③ 桌面端进程在跑但设备还没注册上：别去抢，留一句说明。
        if desktop {
            return Ok(AutoconfigDto {
                source: "official".into(),
                device_name: "Spotify".into(),
                desktop_running: true,
                ..Default::default()
            });
        }

        // ④ 一个官方端都没有 ⇒ 本机播放兜底（用户手动关过就跳过，见 doc 注释）。
        if !allow_local {
            return Ok(AutoconfigDto {
                source: "none".into(),
                desktop_running: false,
                ..Default::default()
            });
        }
        // **没凭据就别启动了**（2026-09-30）：没有凭据的 librespot 不是一台已登录的
        // Connect 设备，起来也不会出现在设备列表里 —— 白白多个进程，还把用户引向
        // 「明明在跑却选不了歌」。如实回报 `needs_login`，让面板给出那个一次性的登录入口。
        if !librespot_credentials_path().is_file() {
            return Ok(AutoconfigDto {
                source: if librespot_dto(&cfg).available { "needs_login" } else { "none" }.into(),
                desktop_running: false,
                ..Default::default()
            });
        }
        let before = librespot_dto(&cfg).running;
        let started_local = if before {
            false
        } else {
            match librespot_start_inner(&cfg, false) {
                Ok(_) => true,
                // 找不到可执行文件 / 起不来 ⇒ 如实回报，别假装就绪。
                // **单独一个 source**：面板那句话要指向「手动打开本机播放」，
                // 而不是「没有可用设备」（后者会把用户引去开 Spotify 桌面端）。
                Err(_) => {
                    return Ok(AutoconfigDto {
                        source: "start_failed".into(),
                        desktop_running: false,
                        ..Default::default()
                    })
                }
            }
        };

        // librespot 注册成设备要几秒 ⇒ 有界轮询（约 5.6s）。**只在面板打开时做一次**，
        // 不在每秒的轮询里 —— 那时它只会白白多打 Web API。
        //
        // **退避 + 4 次（2026-10-02 从 8×800ms 改）**：固定 800ms 的 8 连击是配额被打爆的
        // 直接来源。改成次数更少、间隔递增（0.8→1.2→1.6→2.0s），总时长与原量级相当，
        // 但峰值请求数砍半。每次**强制刷新**（`force_fresh = true`）—— 这个循环存在的
        // 理由就是要看到「刚注册上」的设备，绝不能被快照挡住。
        for i in 0..4u64 {
            std::thread::sleep(Duration::from_millis(800 + i * 400));
            match devices_shared(&mut cfg, true) {
                Ok(list) => {
                    if let Some(d) = list.iter().find(|d| is_local_device(d)) {
                        let _ = transfer_to(&mut cfg, &d.id, false);
                        return Ok(AutoconfigDto {
                            source: "local".into(),
                            device_id: d.id.clone(),
                            device_name: d.name.clone(),
                            device_kind: d.kind.clone(),
                            desktop_running: false,
                            started_local,
                        });
                    }
                }
                // **闸一拉下就立刻停**（2026-10-02）：后面每一次都会当场被挡回来，
                // 接着轮询只是白等几秒（用户盯着「正在启用本机播放…」）。
                // 只认这一种错：网络抖动**必须继续轮询**，那正是这个循环存在的理由。
                Err(e) if e == "ERR_SPOTIFY_STOPPED" => break,
                Err(_) => {}
            }
        }
        // 起来了但没等到注册：**不算失败** —— 播放时 `resolve_play_device` 会再认一次。
        Ok(AutoconfigDto {
            source: "local".into(),
            device_name: LIBRESPOT_DEVICE_NAME.into(),
            desktop_running: false,
            started_local,
            ..Default::default()
        })
    })
    .await
}

/// 停本机播放（面板上那个开关）。
#[tauri::command]
pub async fn librespot_stop() -> Result<LibrespotDto, String> {
    run_blocking(|| Ok(librespot_stop_inner(&load_config()))).await
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
    // 在线播放链也要收 —— 漏了它，`lunac.exe` 退出后那条输出流还占着默认设备
    //（Job Object 只收子进程，收不掉我们自己这条 rodio 流）。
    crate::live_audio::stop();
}

/// **音乐插件的悬浮窗一关，本机播放就跟着收掉**（2026-09-30，用户定；由 `main.rs`
/// 的 `Destroyed` 分支调用）。
///
/// 为什么是「关窗即停」而不是「留着继续放」：librespot 是我们起的一个独立进程，
/// 窗口关掉之后**界面上再没有任何地方能控制它**（音量 / 暂停 / 切歌都在那个窗口里），
/// 用户能看到的只剩「明明退出了，设备列表里还挂着一台叫 `Lunac` 的设备」——
/// 下次开机前它一直在，而且正是 `NO_ACTIVE_DEVICE` 那类怪象的温床。
/// 所以窗口的生命周期就是本机播放的生命周期：**开窗（必要时）自动拉起，关窗带走**。
///
/// 判据只看 label（`plugin-music`）：`plugin_window::open()` 的复用是**按 label** 的
/// （`label_for(plugin_id)`），一个 label 永远只装它自己那个插件，不会串台。
///
/// 与 `kill_librespot()` 的分工：那个管「**退程序**」（主窗口 `Destroyed` / 更新器接管），
/// 这个管「**关窗**」，两条互补，缺一个就会留下孤儿进程。
///
/// 判据单独抽成一个函数**只为能单测**：label 由「前缀常量 + 插件 id」拼成，
/// 哪天改了任一半（或音乐插件换了 id），这条绳就悄悄断了 —— 表现是「关窗后进程还在」，
/// 而那种失败**一点声音都没有**（正是本仓最怕的一类）。
fn is_music_window_label(label: &str) -> bool {
    label == crate::plugin_window::label_for(MUSIC_PLUGIN_ID)
}

pub fn on_plugin_window_destroyed(app: &tauri::AppHandle, label: &str) {
    if !is_music_window_label(label) {
        return;
    }
    // **音乐主窗关了 ⇒ 频响曲线窗（附属窗）一起带走**（2026-10-03）。
    // 留着它没有任何意义：曲线画的是**正在播的那条链**，而播放随主窗关闭一起停了
    // （下面两条）—— 一扇只能显示陈旧曲线的窗，比没有更糟。
    let closed = crate::plugin_window::close_keyed(app, MUSIC_PLUGIN_ID);
    if !closed.is_empty() {
        crate::log::info(&format!("music: 音乐插件窗已关闭 ⇒ 一并关掉附属窗 {closed:?}"));
    }
    // **同一扇窗关掉两样东西**：本机播放（librespot，Spotify）与本地文件播放
    // （`player.rs`，rodio）。两者生命周期完全相同（界面没了就该一起收），
    // 所以挂在同一个 label 判据下 —— 放到 `main.rs` 的 `Destroyed` 里会**没有判据**：
    // 那条分支只认「是不是插件窗」，关掉 OCR / 转换的窗也会把音乐停掉。
    // 各自的「没在跑就不动」判据在函数内部，这里不必重复。
    if crate::player::stop_if_running() {
        crate::log::info("music: 音乐插件窗已关闭 ⇒ 已停掉本地文件播放（rodio）");
    }
    // 没在跑就**什么也不做、也不打日志** —— 这条回调对每个插件窗都走一遍，
    // 记一句「已停掉本机播放」而其实什么都没停，日志就成了噪音（查问题时反而误导）。
    let running = LIBRESPOT.lock().map(|s| s.is_some()).unwrap_or(false);
    if !running {
        return;
    }
    kill_librespot();
    crate::log::info("music: 音乐插件窗已关闭 ⇒ 已停掉本机播放（librespot）");
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

// ── Spotify 桌面端「进程在跑」判定（2026-09-27；2026-10-05 去掉轮询自动弹窗）──
//
// **判据用「进程存在」而不是「正在播放」**：后者要先有 OAuth 令牌 —— 未登录时永远
// 判不出「在播放」，于是第一次使用的用户永远等不到结果。进程判据没有这个前置。
//
// **2026-10-05 用户决定删掉「每 3s 轮询 + Spotify 一启动就自动弹音乐插件」那套检测
// 机制**（它是本仓唯一「无条件、永不停止」的常驻线程，用户口径「让他能抓取 Spotify
// 正在播放的音乐并且控制就可以」）。这里只保留 `spotify_desktop_running()` 这一
// **一次性**判定，供 `music_autoconfigure` 第 ③ 步使用：「桌面端进程在跑但还没注册成
// 设备 ⇒ 别起 librespot 抢同一个会话」。**没有任何后台轮询、没有自动弹窗** ——
// 面板只在用户主动打开时才跑这一次。

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

// ── 单元测试 ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 429 硬闸是**粘性**的：拉了不会自己放开，只有 `clear_spotify_stop` 才解除
    /// （2026-10-02 取代原「定时冷却」，用户口径：一旦 429 就停止请求 + 用户手动恢复）。
    ///
    /// 这条钉的是「不再自动重试」这件事 —— 自动过期正是原来那 31 条 429 的成因
    /// （Spotify 在 429 期间继续请求会把封禁窗口越推越长）。
    #[test]
    fn spotify_stop_latch_is_sticky_until_manually_cleared() {
        clear_spotify_stop();
        assert!(spotify_stop_state().is_none(), "初值必须是「闸没拉下」");
        assert_eq!(retry_remaining_secs(), 0, "没拉闸时不该有倒计时");

        // `Retry-After` 是**唯一**的恢复依据（2026-10-03 加）：实测 Spotify 对
        // `/me/player*` 会给 12 小时量级的值，所以必须原样记下来、并换成一个倒计时。
        trip_spotify_stop("429 Too Many Requests（本 30s 窗口内第 7 次请求）", 45_000);
        let reason = spotify_stop_state().expect("拉闸后必须读得到原因");
        assert!(reason.contains("429"), "原因要能显示给用户，实得 {reason}");
        assert_eq!(SPOTIFY_RETRY_AFTER_SECS.load(Ordering::SeqCst), 45_000);
        assert!(
            retry_remaining_secs() > 44_000,
            "倒计时要按 Retry-After 算，实得 {}",
            retry_remaining_secs()
        );

        // **幂等**：再拉一次不改变原因 / 倒计时（也不该重复记日志）。
        trip_spotify_stop("别的原因", 0);
        assert_eq!(
            spotify_stop_state().as_deref(),
            Some("429 Too Many Requests（本 30s 窗口内第 7 次请求）"),
            "已拉着时再拉不该覆盖原因"
        );
        assert_eq!(
            SPOTIFY_RETRY_AFTER_SECS.load(Ordering::SeqCst),
            45_000,
            "已拉着时再拉不该覆盖倒计时"
        );

        // 只有手动清才放开，且倒计时一起清掉。
        clear_spotify_stop();
        assert!(spotify_stop_state().is_none(), "手动恢复后必须放开");
        assert_eq!(retry_remaining_secs(), 0, "清闸必须把倒计时一起清掉");
    }

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
    fn play_url_carries_device_id_as_query_param() {
        // `device_id` 必须是**查询参数**：塞进 JSON body 会被 Spotify 静默忽略，
        // 症状与「根本没传」一模一样（还是落在活跃设备上 → 没有活跃设备就 404），
        // 所以这条断言防的是「有人好心把它挪进 body」。
        assert!(play_url(None).ends_with("/me/player/play"));
        assert!(play_url(Some("abc123")).ends_with("/me/player/play?device_id=abc123"));
    }

    #[test]
    fn local_device_is_identified_by_name_only() {
        // librespot 上报的 type 也是 `Computer`，与 Spotify 桌面端**完全一样**
        // ⇒ 按类型分不开，只能按设备名认（改 LIBRESPOT_DEVICE_NAME 等于换台新设备）。
        let mk = |name: &str| DeviceDto {
            id: "x".into(),
            name: name.into(),
            kind: "Computer".into(),
            active: false,
            volume_percent: 0,
        };
        assert!(is_local_device(&mk(LIBRESPOT_DEVICE_NAME)));
        assert!(!is_local_device(&mk("Spotify")));
        assert!(!is_local_device(&mk("")));
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

    /// 参数表是**纯函数**，所以能直接断言 —— 起进程那一段没法测，参数表能。
    /// 「有人顺手删掉一个参数」正是这里最容易出的事（每一条都在注释里写了理由）。
    fn strs(v: &[String]) -> Vec<&str> {
        v.iter().map(|s| s.as_str()).collect()
    }

    #[test]
    fn librespot_args_pin_every_flag() {
        let cache = std::path::Path::new(r"D:\Lunac\config\librespot-cache");
        let a = librespot_args(&MusicConfig::default(), cache, false);
        assert_eq!(
            strs(&a),
            vec![
                "--name",
                "Lunac",
                "-c",
                r"D:\Lunac\config\librespot-cache",
                "-C",
                r"D:\Lunac\config\librespot-cache",
                "-t",
                r"D:\Lunac\config\librespot-cache\tmp",
                "-M",
                "2G",
                "--bitrate",
                "320",
                // 在线音乐的音频必须流经我们进程才谈得上调音（见 `live_audio.rs`）
                "-B",
                "pipe",
                "-f",
                "F32",
            ]
        );

        // 首次登录那次额外带 OAuth 两件（`-K` 不给就登记不上，见 LIBRESPOT_OAUTH_PORT）
        let b = librespot_args(&MusicConfig::default(), cache, true);
        assert_eq!(&strs(&b)[b.len() - 3..], &["-j", "-K", "8898"]);
        // **登录那次绝不能带 `-B pipe`**：librespot-oauth 会把 "Browse to: <url>" 打到
        // **stdout**，混进 PCM 会让后面的 f32 整条错位（全是噪声）。这条是静默的 ——
        // 少了它，表现是「首次授权后再放歌是一堆电流声」，很难往参数上想。
        assert!(!strs(&b).contains(&"-B"));
        assert!(!strs(&b).contains(&"pipe"));

        // 代理（2026-10-02 收进这张表）：空 = 一个字节都不出现；非空 = 紧跟 `-B pipe -f F32`
        // 之后、且**必须排在 `-j/-K` 之前**（否则上面那条「最后三个是 -j -K 8898」会失效）。
        let mut with_proxy = MusicConfig::default();
        with_proxy.librespot_proxy = "http://127.0.0.1:7890".into();
        let c = librespot_args(&with_proxy, cache, true);
        let i = c.iter().position(|s| s == "-x").expect("有代理就必须有 -x");
        assert_eq!(&strs(&c)[i..i + 2], &["-x", "http://127.0.0.1:7890"]);
        assert_eq!(&strs(&c)[c.len() - 3..], &["-j", "-K", "8898"]);

        // 非登录 + 代理：三样都在，且顺序是 `-B pipe -f F32` → `-x` → （无 -j）
        let d = librespot_args(&with_proxy, cache, false);
        assert_eq!(
            &strs(&d)[12..],
            &["-B", "pipe", "-f", "F32", "-x", "http://127.0.0.1:7890"]
        );
    }

    /// `-x` 只认 HTTP 代理（本机 0.8.0 `--help` 原文与官方 docker 的 `PROXY` 说明）。
    /// 这条判据漏了是**静默**的：socks5 能写盘、日志里也有 `Using proxy`，就是连不上。
    #[test]
    fn librespot_proxy_only_accepts_an_http_proxy() {
        // 空 = 直连，合法（这是「关掉代理」那条路的形态）
        assert!(validate_librespot_proxy("").is_ok());
        assert!(validate_librespot_proxy("   ").is_ok());
        assert!(validate_librespot_proxy("http://127.0.0.1:7890").is_ok());
        assert!(validate_librespot_proxy("https://proxy.example.com:8443").is_ok());

        // socks5 是**最容易被以为能用**的那种（本地客户端的 7891 常是 socks5 口）
        let e = validate_librespot_proxy("socks5://127.0.0.1:7891").unwrap_err();
        assert!(e.contains("HTTP 代理"), "错误要说清只认 HTTP 代理：{e}");
        assert!(e.contains("socks5"), "要把他填的那个值念出来：{e}");
        assert!(validate_librespot_proxy("127.0.0.1:7890").is_err(), "缺 scheme");
        assert!(validate_librespot_proxy("http://a b:1").is_err(), "带空格");
    }

    /// 「关窗即停 librespot」那条绳：label 由「前缀常量 + 插件 id」拼出来，
    /// **只有音乐那一扇窗算数** —— 别的插件窗被关掉不该动音频进程。
    /// 这条判据一旦对不上是**完全静默的**（界面关了、进程还在，没有任何提示），
    /// 所以把「前缀 + id」两个半截都钉死在这里。
    #[test]
    fn only_the_music_window_label_stops_local_playback() {
        // 两半各钉一次：左边走常量拼接（改 id 会被逮到），右边写死字面量（改前缀会被逮到）
        assert!(is_music_window_label(&crate::plugin_window::label_for(MUSIC_PLUGIN_ID)));
        assert!(is_music_window_label("plugin-music"));
        // `plugin-music-curve` = **频响曲线窗**（2026-10-03 的附属窗）：它必须是**另一个**
        // 判定 —— 关掉曲线窗不该把 librespot / 本地播放一起杀掉（那是主窗的事）。
        for other in [
            "plugin-memo",
            "plugin-pet",
            "plugin-music-2",
            "plugin-music-curve",
            "main",
            "plugin-",
        ] {
            assert!(
                !is_music_window_label(other),
                "{other} 不是音乐插件窗，不该停掉本机播放"
            );
        }
    }

    /// `-t` 是 2026-09-30 补的：不给时「边下边放的临时文件」走 librespot 自己的默认，
    /// 而那份默认落在哪我们没能从二进制里确证 ⇒ 显式钉在我们自己的缓存根下。
    #[test]
    fn temp_dir_never_escapes_our_own_cache_directory() {
        let cache = librespot_cache_dir();
        let a = librespot_args(&MusicConfig::default(), &cache, false);
        let i = a.iter().position(|s| s == "-t").unwrap();
        let tmp = std::path::PathBuf::from(&a[i + 1]);
        assert!(
            tmp.starts_with(&cache),
            "临时目录必须落在我们自己的缓存根下，收到 {}",
            tmp.display()
        );
        assert!(a.iter().any(|s| s == "-M"), "-M 不给就是无上限");
    }

    #[test]
    fn bitrate_comes_from_config_and_never_leaves_the_allowed_set() {
        let cache = std::path::Path::new("C:\\tmp\\c");
        let bitrate_arg = |cfg: &MusicConfig| {
            let a = librespot_args(cfg, cache, false);
            let i = a.iter().position(|s| s == "--bitrate").unwrap();
            a[i + 1].clone()
        };

        // 默认 = 最高档（**刻意不同于 librespot 自己的默认 160**，见 DEFAULT_LIBRESPOT_BITRATE）
        assert_eq!(bitrate_arg(&MusicConfig::default()), "320");

        let mut cfg = MusicConfig::default();
        cfg.librespot_bitrate = 96;
        assert_eq!(bitrate_arg(&cfg), "96");

        // 老配置（没有这个键 ⇒ 0）与手工改坏的值都压回默认档 ——
        // 越界值进了命令行会让 librespot 直接启动失败，表现成「点了没反应」，很难查。
        for bad in [0u16, 1, 128, 999] {
            let mut cfg = MusicConfig::default();
            cfg.librespot_bitrate = bad;
            assert_eq!(bitrate_arg(&cfg), "320", "bitrate={bad} 应压回默认档");
        }
    }

    #[test]
    fn old_config_without_bitrate_falls_back_to_the_default_tier() {
        let cfg: MusicConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.librespot_bitrate, 0, "缺键 ⇒ serde 的 default 0");
        assert_eq!(cfg.effective_librespot_bitrate(), DEFAULT_LIBRESPOT_BITRATE);
        // 面板拿到的必须是**兜过底**的值，否则三档按钮一个都不亮
        assert_eq!(
            config_dto(&cfg).librespot_bitrate,
            DEFAULT_LIBRESPOT_BITRATE
        );
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
