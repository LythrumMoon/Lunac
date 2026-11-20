//! 在线歌词（移植 v1 `app/src-tauri/src/music.rs` 的「诗句」那一段）。
//!
//! 三级取词（v1 2026-09-27 口径，逐条对齐）：
//!   1. **LRCLIB `/api/get`**：歌名 + 歌手（+ 专辑 + 时长）精确命中；
//!   2. **LRCLIB `/api/search`**：优先带时间轴的那条，其次第一条有词的；
//!   3. **网易云兜底**：公开只读接口（**必须带 `Referer`**），冷门 / 中文歌在 LRCLIB
//!      常年没有条目。
//!
//! 纪律（与 v1 同口径，少一条就对应一个具体坏表现）：
//!   · **任何一级命中就返回**；三级全落空 → `found:false`（面板显示「没找到歌词」，**不是错误**）；
//!   · LRCLIB 的 `send` 失败**重试一次**（公开只读端点，偶发链路抖动；用户对此无能为力），
//!     但 **429 不重试**，把 `Retry-After` 原样带回（`ERR_RATE_LIMITED:<秒>`），由面板提示等待；
//!   · 网易云是非官方接口 ⇒ 全程失败即 `None`，绝不把「主源没找到 + 兜底挂了」升级成报错。
//!
//! 为什么单独一个文件：`music.rs` 的边界是「只读本地磁盘」，网络请求不该混进去；
//! 调用方是 `lunac_music_lyrics_online`（Dart 在 worker isolate 里调，不冻 UI）。

use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::StatusCode;

use crate::music::parse_lrc;

const LRCLIB: &str = "https://lrclib.net/api";
const NETEASE_API: &str = "https://music.163.com/api";
const NETEASE_REFERER: &str = "https://music.163.com/";
/// 与 v1 同一个 UA 形状（LRCLIB 会按 UA 做限流统计）。
const UA: &str = "Lunac/2 (+https://lunac.pet)";

/// 单次请求超时：这是同步 FFI，卡住会连带面板等不到结果（v1 用在 async 里感受不到）。
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

fn http() -> Result<Client, String> {
    Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败：{e}"))
}

/// 百分号编码（`application/x-www-form-urlencoded` 的保守版）：
/// 只放行 RFC 3986 的 unreserved 字符，其余（含中文 / 空格 / `&`）一律 `%XX`。
pub fn qenc(text: &str) -> String {
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

/// 归一化歌名用于比对：丢掉括号里的内容与所有非字母数字字符，并转小写。
///
/// 「Creep (现场版)」必须与「Creep」对得上 —— 而 LRCLIB 命中不了的时候，
/// 恰恰就是这些带后缀的版本（v1 两个源共用同一套口径）。
pub fn norm_title(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for ch in text.chars() {
        match ch {
            '(' | '（' | '[' | '【' => depth += 1,
            ')' | '）' | ']' | '】' => depth -= 1,
            _ if depth <= 0 => {
                if ch.is_alphanumeric() {
                    out.extend(ch.to_lowercase());
                }
            }
            _ => {}
        }
    }
    out
}

/// 一次取词请求（面板传进来的 JSON：`{title, artist, album, duration}`）。
#[derive(Debug, Default, Clone, Copy)]
pub struct Query<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    /// 秒；`0` = 不知道时长（不参与 `/api/get` 的匹配）。
    pub duration: f64,
}

/// LRCLIB `/api/get` 的 URL（纯函数：单测直接比对，不联网）。
pub fn lrclib_get_url(query: &Query<'_>) -> String {
    let mut url = format!(
        "{LRCLIB}/get?track_name={}&artist_name={}",
        qenc(query.title.trim()),
        qenc(query.artist.trim())
    );
    let album = query.album.trim();
    if !album.is_empty() {
        url.push_str(&format!("&album_name={}", qenc(album)));
    }
    if query.duration > 0.0 {
        url.push_str(&format!("&duration={}", query.duration.round() as i64));
    }
    url
}

pub fn lrclib_search_url(query: &Query<'_>) -> String {
    format!(
        "{LRCLIB}/search?track_name={}&artist_name={}",
        qenc(query.title.trim()),
        qenc(query.artist.trim())
    )
}

/// 一条歌词结果（JSON 形状见 [`to_json`]）。
#[derive(Debug, Default, Clone)]
pub struct OnlineLyrics {
    pub found: bool,
    pub instrumental: bool,
    /// `lrclib` / `netease`（面板展示来源，用户能看出对不对）。
    pub source: &'static str,
    pub track: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub synced: Option<String>,
    pub plain: Option<String>,
}

impl OnlineLyrics {
    /// `lines` 由 [`crate::music::parse_lrc`] 解析 —— 与本地 LRC 走同一条时间轴口径。
    pub fn to_json(&self, error: Option<&str>) -> String {
        let lines = self
            .synced
            .as_deref()
            .map(parse_lrc)
            .unwrap_or_default();
        serde_json::json!({
            "found": self.found,
            "instrumental": self.instrumental,
            "source": self.source,
            "track": self.track,
            "artist": self.artist,
            "album": self.album,
            "duration": self.duration,
            "lines": lines,
            "plain": self.plain,
            "error": error,
        })
        .to_string()
    }
}

/// LRCLIB 的一条记录 → `OnlineLyrics`（`found` 恒为 true；调用方判断有没有词）。
fn from_lrclib(value: &serde_json::Value) -> OnlineLyrics {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|item| item.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let non_empty = |key: &str| {
        value
            .get(key)
            .and_then(|item| item.as_str())
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
    };
    OnlineLyrics {
        found: true,
        instrumental: value
            .get("instrumental")
            .and_then(|item| item.as_bool())
            .unwrap_or(false),
        source: "lrclib",
        track: text("trackName"),
        artist: text("artistName"),
        album: text("albumName"),
        duration: value
            .get("duration")
            .and_then(|item| item.as_f64())
            .unwrap_or(0.0),
        synced: non_empty("syncedLyrics"),
        plain: non_empty("plainLyrics"),
    }
}

/// LRCLIB 的 GET：`send` 失败重试一次；429 立刻返回 `ERR_RATE_LIMITED:<秒>`。
fn lrclib_get(client: &Client, url: &str) -> Result<(StatusCode, String), String> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match client.get(url).header("User-Agent", UA).send() {
            Ok(response) => {
                let status = response.status();
                if status == StatusCode::TOO_MANY_REQUESTS {
                    let retry = response
                        .headers()
                        .get("retry-after")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("?")
                        .to_string();
                    return Err(format!("ERR_RATE_LIMITED:{retry}"));
                }
                let text = response.text().unwrap_or_default();
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

/// 搜索结果里任一条的歌手名是否对得上（双向 `contains`，容忍「Feat.」这类拼接）。
fn netease_artist_matches(song: &serde_json::Value, artist: &str) -> bool {
    let want = norm_title(artist);
    if want.is_empty() {
        return true;
    }
    song.get("artists")
        .and_then(|value| value.as_array())
        .map(|list| {
            list.iter().any(|item| {
                let name = norm_title(
                    item.get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default(),
                );
                !name.is_empty() && (name.contains(&want) || want.contains(&name))
            })
        })
        .unwrap_or(false)
}

fn netease_get_json(client: &Client, url: &str) -> Result<serde_json::Value, String> {
    let response = client
        .get(url)
        .header("Referer", NETEASE_REFERER)
        .header("User-Agent", UA)
        .send()
        .map_err(|e| format!("请求网易云失败：{e}"))?;
    if !response.status().is_success() {
        return Err(format!("网易云返回 {}", response.status()));
    }
    response
        .json()
        .map_err(|e| format!("网易云响应解析失败：{e}"))
}

/// 第 3 级：网易云（只返回 `synced` —— 它给的就是 `[mm:ss.xx]` 逐行 LRC）。
fn netease_lyrics(client: &Client, query: &Query<'_>) -> Option<OnlineLyrics> {
    let keyword = if query.artist.trim().is_empty() {
        query.title.trim().to_string()
    } else {
        format!("{} {}", query.title.trim(), query.artist.trim())
    };
    let search_url = format!(
        "{NETEASE_API}/search/get/web?csrf_token=&s={}&type=1&offset=0&total=true&limit=10",
        qenc(keyword.trim())
    );
    let value = netease_get_json(client, &search_url).ok()?;
    let songs = value.pointer("/result/songs").and_then(|v| v.as_array())?;
    let want = norm_title(query.title);
    // 挑选顺序：歌名 + 歌手都对得上 → 只对得上歌名 → 第一条。
    // 最后那条兜底是有意的：宁可给一个「可能不是这个版本」的歌词，也比空着强
    // （面板会显示来源与曲名，用户看得出来对不对）。
    let pick = songs
        .iter()
        .find(|song| {
            let name = song
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            norm_title(name) == want && netease_artist_matches(song, query.artist)
        })
        .or_else(|| {
            songs.iter().find(|song| {
                norm_title(
                    song.get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default(),
                ) == want
            })
        })
        .or_else(|| songs.first())?;
    let id = pick.get("id").and_then(|value| value.as_i64())?;
    let lyric_value = netease_get_json(
        client,
        &format!("{NETEASE_API}/song/lyric?id={id}&lv=-1&kv=-1&tv=-1"),
    )
    .ok()?;
    let lrc = lyric_value
        .pointer("/lrc/lyric")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if lrc.is_empty() {
        return None;
    }
    let artist = pick
        .get("artists")
        .and_then(|value| value.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|item| item.get("name").and_then(|value| value.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    Some(OnlineLyrics {
        found: true,
        instrumental: false,
        source: "netease",
        track: pick
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or(query.title)
            .to_string(),
        artist,
        album: pick
            .pointer("/album/name")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string(),
        duration: pick
            .get("duration")
            .and_then(|value| value.as_f64())
            .map(|ms| ms / 1000.0)
            .unwrap_or(0.0),
        synced: Some(lrc),
        plain: None,
    })
}

/// 三级取词（不联网的单测只覆盖 URL / 归一化 / 解析；联网部分靠真机验收）。
fn lookup(query: &Query<'_>) -> Result<OnlineLyrics, String> {
    let client = http()?;
    // 1) 精确命中
    let (status, text) = lrclib_get(&client, &lrclib_get_url(query))?;
    if status.is_success() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            let lyrics = from_lrclib(&value);
            if lyrics.synced.is_some() || lyrics.plain.is_some() {
                return Ok(lyrics);
            }
            // 命中了一条「只有纯音乐标记」的记录：按纯音乐返回（v1 同口径）。
            if lyrics.instrumental {
                return Ok(lyrics);
            }
        }
    } else if status != StatusCode::NOT_FOUND {
        return Err(format!("LRCLIB 返回 {status}"));
    }

    // 2) 搜索回落
    let (search_status, search_text) = lrclib_get(&client, &lrclib_search_url(query))?;
    if !search_status.is_success() {
        // LRCLIB 整条链路不可用时也给兜底源一次机会 —— 兜底的意义正在于此。
        return Ok(netease_lyrics(&client, query).unwrap_or_default());
    }
    let hits: Vec<serde_json::Value> = serde_json::from_str(&search_text).unwrap_or_default();
    let best = hits
        .iter()
        .find(|value| {
            value
                .get("syncedLyrics")
                .and_then(|item| item.as_str())
                .is_some_and(|text| !text.trim().is_empty())
        })
        .or_else(|| {
            hits.iter().find(|value| {
                value
                    .get("plainLyrics")
                    .and_then(|item| item.as_str())
                    .is_some_and(|text| !text.trim().is_empty())
            })
        });
    if let Some(value) = best {
        return Ok(from_lrclib(value));
    }

    // 3) 第 2 级兜底：网易云；拿不到就如实「没歌词」。
    Ok(netease_lyrics(&client, query).unwrap_or_default())
}

/// `lunac_music_lyrics_online` 的入口：入参 JSON `{title, artist, album, duration}`。
pub fn online_lyrics_json(payload: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|item| item.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let title = text("title");
    let artist = text("artist");
    let album = text("album");
    let duration = value
        .get("duration")
        .and_then(|item| item.as_f64())
        .unwrap_or(0.0);
    if title.trim().is_empty() {
        return OnlineLyrics::default().to_json(Some("没有歌名，无法搜索歌词"));
    }
    let query = Query {
        title: &title,
        artist: &artist,
        album: &album,
        duration,
    };
    match lookup(&query) {
        Ok(lyrics) if lyrics.found => lyrics.to_json(None),
        Ok(_) => OnlineLyrics::default().to_json(None),
        Err(reason) => OnlineLyrics::default().to_json(Some(&reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding_escapes_space_and_cjk() {
        assert_eq!(qenc("abc-1._~"), "abc-1._~");
        assert_eq!(qenc("a b"), "a%20b");
        assert_eq!(qenc("A&B"), "A%26B");
        assert_eq!(qenc("中"), "%E4%B8%AD");
    }

    #[test]
    fn norm_title_drops_brackets_and_case() {
        assert_eq!(norm_title("Creep (现场版)"), "creep");
        assert_eq!(norm_title("Creep [Live] 【Remaster】"), "creep");
        assert_eq!(norm_title("Hello, World!"), "helloworld");
        assert_eq!(norm_title(""), "");
    }

    #[test]
    fn get_url_only_adds_optional_params_when_present() {
        let minimal = Query {
            title: "Creep",
            artist: "Radiohead",
            album: "",
            duration: 0.0,
        };
        assert_eq!(
            lrclib_get_url(&minimal),
            "https://lrclib.net/api/get?track_name=Creep&artist_name=Radiohead"
        );
        let full = Query {
            title: "Creep",
            artist: "Radiohead",
            album: "Pablo Honey",
            duration: 238.6,
        };
        let url = lrclib_get_url(&full);
        assert!(url.contains("&album_name=Pablo%20Honey"));
        assert!(url.ends_with("&duration=239"), "时长四舍五入到秒：{url}");
    }

    #[test]
    fn lrclib_record_maps_synced_and_plain_lines() {
        let value = serde_json::json!({
            "trackName": "Creep",
            "artistName": "Radiohead",
            "albumName": "Pablo Honey",
            "duration": 238.0,
            "syncedLyrics": "[00:12.00]When you were here before\n[00:18.00]Couldn't look you in the eye",
            "plainLyrics": "When you were here before\nCouldn't look you in the eye",
        });
        let lyrics = from_lrclib(&value);
        assert!(lyrics.found);
        assert_eq!(lyrics.source, "lrclib");
        assert_eq!(lyrics.track, "Creep");
        assert_eq!(lyrics.album, "Pablo Honey");
        let json: serde_json::Value = serde_json::from_str(&lyrics.to_json(None)).unwrap();
        assert_eq!(json["lines"][0]["timeMs"], 12000);
        assert_eq!(json["lines"][0]["text"], "When you were here before");
        assert_eq!(json["lines"][1]["timeMs"], 18000);
        assert_eq!(json["plain"], "When you were here before\nCouldn't look you in the eye");
        assert!(json["error"].is_null());
    }

    #[test]
    fn instrumental_record_is_found_without_lines() {
        let value = serde_json::json!({ "instrumental": true, "trackName": "Intro" });
        let lyrics = from_lrclib(&value);
        let json: serde_json::Value = serde_json::from_str(&lyrics.to_json(None)).unwrap();
        assert_eq!(json["found"], true);
        assert_eq!(json["instrumental"], true);
        assert_eq!(json["lines"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn netease_artist_match_tolerates_joined_names() {
        let song = serde_json::json!({
            "artists": [ { "name": "Radiohead" }, { "name": "Feat. Someone" } ]
        });
        assert!(netease_artist_matches(&song, "radiohead"));
        assert!(netease_artist_matches(&song, "Radiohead & Co"));
        assert!(!netease_artist_matches(&song, "Coldplay"));
        // 没给歌手 ⇒ 不参与筛选（v1 同口径）。
        assert!(netease_artist_matches(&song, ""));
    }

    #[test]
    fn empty_title_reports_a_reason_without_network() {
        let json: serde_json::Value =
            serde_json::from_str(&online_lyrics_json(r#"{"title":"   "}"#)).unwrap();
        assert_eq!(json["found"], false);
        assert_eq!(json["lines"].as_array().map(Vec::len), Some(0));
        assert!(json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("歌名"));
    }
}
