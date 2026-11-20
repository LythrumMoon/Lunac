//! 音乐插件的宿主侧（移植 v1 `app/src-tauri/src/media_lib.rs` 的「本地媒体库」那一层）。
//!
//! 范围（刻意划得很窄）：
//!   · **只读本地磁盘**：扫曲库目录 → `lofty` 读标签 / 时长 / 内嵌封面；
//!   · **只读同名 `.lrc`**：解析成 `[{timeMs, text}]`，供面板按播放进度滚动高亮；
//!   · **自定义队列**：`config/music-queues.json`（多条歌单）+ `.m3u/.m3u8/.pls` 导入；
//!   · **不出声**：解码与播放按用户裁决交给 Flutter 的 `just_audio` + `just_audio_windows`。
//!
//! v1 的在线部分（Spotify 播放控制 / LRCLIB 歌词）需要外部账号与网络，
//! v2 **不伪造**：C ABI 只暴露本地媒体库与本地歌词，面板把在线入口如实标成「本版未接入」。
//!
//! 路径口径（与 v1 一致）：
//!   · v1 用户添加的曲库目录 = `<exe 根>\ModuleData\music\media.db` 的 `roots` 表；
//!   · 封面缓存            = `<exe 根>\temp\music-covers\`（可删缓存，内容哈希做文件名）；
//!   · 没有 v1 库时回落到系统音乐目录（Windows `%USERPROFILE%\Music`）。
//!
//! 纪律：任何失败都降级成一句人话（不 panic）；测试只碰临时目录，不扫真实音乐库。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 允许入库的音频扩展名（与 v1 `media_lib.rs:44-47` 同一份名单；只做初筛）。
const AUDIO_EXTS: &[&str] = &[
    "mp3", "flac", "m4a", "m4b", "mp4", "aac", "ogg", "oga", "opus", "spx", "wav", "wave", "aif",
    "aiff", "aifc", "ape", "wv", "mpc",
];

/// 目录遍历深度上限（防「指向 C:\ 的根」把整台机器走穿；v1 同口径）。
const MAX_DEPTH: usize = 16;
/// 单次返回的曲目上限。面板一次画不了更多，扫到就停（避免误加一个大目录时卡住）。
const MAX_TRACKS: usize = 1000;
/// 封面字节上限（与 v1 一致）：超过就不缓存，免得把几 MB 的图塞给界面。
const COVER_MAX_BYTES: usize = 3 * 1024 * 1024;

// ── JSON 形状 ─────────────────────────────────────────────────────

#[derive(Debug, Serialize, Default, Clone)]
pub struct MusicTrack {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// 毫秒；`0` = 标签里读不到时长（界面不画假的进度）。
    pub duration_ms: i64,
    /// 已缓存封面的**绝对路径**（空串 = 这首没有封面）。
    pub cover: String,
}

#[derive(Debug, Serialize, Default, Clone)]
pub struct LyricLine {
    #[serde(rename = "timeMs")]
    pub time_ms: i64,
    pub text: String,
}

// ── 曲库位置 ──────────────────────────────────────────────────────

/// 系统默认音乐目录（Windows `%USERPROFILE%\Music`，其它平台 `$HOME/Music`）。
pub fn user_music_dir() -> Option<PathBuf> {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    } else {
        std::env::var_os("HOME").map(PathBuf::from)
    }?;
    Some(home.join("Music"))
}

/// 读 v1 媒体库里用户添加过的曲库目录（只读打开，**不创建任何文件**）。
///
/// v1 的库是 `<exe 根>\ModuleData\music\media.db`，`roots.path` 存的就是目录绝对路径
/// （见 v1 `media_lib.rs:412-416`）。库不存在 / 打不开 / 没这张表都当「没有」。
pub fn legacy_roots(exe_root: &Path) -> Vec<PathBuf> {
    let db = exe_root.join("ModuleData").join("music").join("media.db");
    if !db.is_file() {
        return Vec::new();
    }
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let _ = conn.execute_batch("PRAGMA busy_timeout=1000;");
    let Ok(mut stmt) = conn.prepare("SELECT path FROM roots ORDER BY added_at") else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |row| row.get::<_, String>(0));
    let Ok(rows) = rows else {
        return Vec::new();
    };
    rows.flatten()
        .map(|path| PathBuf::from(path.trim()))
        .filter(|path| !path.as_os_str().is_empty())
        .collect()
}

/// 本次扫描的曲库目录：**只认用户添加过的目录**（与 v1 `media_lib.rs` 同口径：
/// v1 没有「隐式默认目录」，新装是空的，必须由用户添加）。按小写路径去重并滤掉不存在的。
pub fn library_roots(exe_root: &Path) -> Vec<PathBuf> {
    let roots = legacy_roots(exe_root);
    let mut seen = std::collections::HashSet::new();
    roots
        .into_iter()
        .filter(|path| {
            if !path.is_dir() {
                return false;
            }
            let key = path.to_string_lossy().to_lowercase();
            seen.insert(key)
        })
        .collect()
}

/// 曲库库文件的绝对路径（与 v1 同路径，读写同一份 `media.db`）。
///
/// `pub(crate)`：`media_lib.rs`（v1 形态的 SQLite + 后台扫描）也用这一份库文件。
pub(crate) fn media_db_path(exe_root: &Path) -> PathBuf {
    exe_root.join("ModuleData").join("music").join("media.db")
}

/// 目录规范化：与 v1 `media_lib.rs:191-203` 逐字一致 —— 去首尾空白、去尾分隔符；
/// 盘符根（`D:`）补回 `D:\`。`roots.path` 是主键，两种写法必须是同一条记录。
pub fn normalize_root(input: &str) -> String {
    let t = input.trim().trim_end_matches(['\\', '/']);
    if t.len() == 2 && t.ends_with(':') {
        return format!("{t}\\");
    }
    if t.is_empty() {
        input.trim().to_string()
    } else {
        t.to_string()
    }
}

/// 打开可写的媒体库（按需建目录与 `roots` 表；表结构与 v1 `media_lib.rs:412-416` 一致）。
pub(crate) fn open_db_rw(exe_root: &Path) -> Result<rusqlite::Connection, String> {
    let db = media_db_path(exe_root);
    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录失败：{e}"))?;
    }
    let conn = rusqlite::Connection::open(&db).map_err(|e| format!("打开曲库失败：{e}"))?;
    let _ = conn.execute_batch("PRAGMA busy_timeout=3000;");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS roots (
           path         TEXT PRIMARY KEY,
           added_at     INTEGER NOT NULL,
           last_scan_at INTEGER NOT NULL DEFAULT 0
         );",
    )
    .map_err(|e| format!("建表失败：{e}"))?;
    Ok(conn)
}

/// 添加曲库目录（重复添加幂等；目录必须真实存在）。
pub fn add_root(exe_root: &Path, input: &str) -> Result<String, String> {
    let path = normalize_root(input);
    if path.is_empty() {
        return Err("目录为空".to_string());
    }
    if !Path::new(&path).is_dir() {
        return Err(format!("目录不存在：{path}"));
    }
    let conn = open_db_rw(exe_root)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    conn.execute(
        "INSERT OR IGNORE INTO roots (path, added_at) VALUES (?1, ?2)",
        rusqlite::params![path, now],
    )
    .map_err(|e| format!("写入曲库失败：{e}"))?;
    Ok(path)
}

/// 移除曲库目录（不存在也算成功，幂等）。
pub fn remove_root(exe_root: &Path, input: &str) -> Result<(), String> {
    let path = normalize_root(input);
    let conn = open_db_rw(exe_root)?;
    conn.execute("DELETE FROM roots WHERE path = ?1", rusqlite::params![path])
        .map_err(|e| format!("删除曲库失败：{e}"))?;
    Ok(())
}

// ── 扫描 ──────────────────────────────────────────────────────────

/// 扫描结果：曲目 + 实际扫过的目录 + 读目录时的错误（用于如实说明为什么是空的）。
#[derive(Debug, Default)]
pub struct ScanOutcome {
    pub tracks: Vec<MusicTrack>,
    pub roots: Vec<String>,
    pub errors: Vec<String>,
}

/// 扫描若干目录（纯文件系统操作，可单测）。
pub fn scan_roots(roots: &[PathBuf], cover_dir: &Path) -> ScanOutcome {
    let mut out = ScanOutcome {
        roots: roots
            .iter()
            .map(|root| root.display().to_string())
            .collect(),
        ..Default::default()
    };
    for root in roots {
        walk(root, 0, cover_dir, &mut out);
        if out.tracks.len() >= MAX_TRACKS {
            break;
        }
    }
    // 与 v1 曲目表同一排序口径：专辑 → 标题（大小写不敏感）。
    out.tracks.sort_by(|a, b| {
        a.album
            .to_lowercase()
            .cmp(&b.album.to_lowercase())
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    out.tracks.truncate(MAX_TRACKS);
    out
}

fn walk(dir: &Path, depth: usize, cover_dir: &Path, out: &mut ScanOutcome) {
    if depth > MAX_DEPTH || out.tracks.len() >= MAX_TRACKS {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            out.errors
                .push(format!("读取目录失败：{}（{e}）", dir.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        if out.tracks.len() >= MAX_TRACKS {
            return;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // 不跟符号链接：可能指回上层形成环。
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            walk(&path, depth + 1, cover_dir, out);
            continue;
        }
        if !is_audio_path(&path) {
            continue;
        }
        out.tracks.push(read_track(&path, cover_dir));
    }
}

pub(crate) fn is_audio_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTS.iter().any(|known| ext.eq_ignore_ascii_case(known)))
        .unwrap_or(false)
}

/// 读一首的标签；`lofty` 解析失败时**不丢曲目**，退回「文件名 + 空标签」（v1 同口径）。
pub(crate) fn read_track(path: &Path, cover_dir: &Path) -> MusicTrack {
    let mut track = MusicTrack {
        path: path.display().to_string(),
        title: title_from_path(path),
        ..Default::default()
    };
    if let Some((title, artist, album, duration_ms, cover)) = read_tags(path, cover_dir) {
        if !title.trim().is_empty() {
            track.title = title;
        }
        track.artist = artist;
        track.album = album;
        track.duration_ms = duration_ms;
        track.cover = cover;
    }
    track
}

fn read_tags(
    path: &Path,
    cover_dir: &Path,
) -> Option<(String, String, String, i64, String)> {
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::probe::Probe;
    use lofty::tag::Accessor;

    let tagged = Probe::open(path)
        .ok()?
        .guess_file_type()
        .ok()?
        .read()
        .ok()?;
    let duration_ms = tagged.properties().duration().as_millis() as i64;
    let mut title = String::new();
    let mut artist = String::new();
    let mut album = String::new();
    let mut cover = String::new();
    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        title = tag.title().map(|s| s.to_string()).unwrap_or_default();
        artist = tag.artist().map(|s| s.to_string()).unwrap_or_default();
        album = tag.album().map(|s| s.to_string()).unwrap_or_default();
        // 封面：只取第一张（多张内嵌图里第一张历来是正面图；v1 同口径）。
        if let Some(pic) = tag.pictures().first() {
            let data = pic.data();
            if !data.is_empty() && data.len() <= COVER_MAX_BYTES {
                let ext = pic
                    .mime_type()
                    .map(|m| mime_to_ext(&m.to_string()))
                    .unwrap_or("jpg");
                cover = save_cover(cover_dir, data, ext).unwrap_or_default();
            }
        }
    }
    Some((title, artist, album, duration_ms, cover))
}

/// 没有标签时的标题兜底：文件名（不含扩展名）。
fn title_from_path(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| {
            path.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        })
}

fn mime_to_ext(mime: &str) -> &'static str {
    let mime = mime.to_ascii_lowercase();
    if mime.contains("png") {
        "png"
    } else if mime.contains("gif") {
        "gif"
    } else if mime.contains("bmp") {
        "bmp"
    } else if mime.contains("webp") {
        "webp"
    } else {
        "jpg"
    }
}

/// 把封面写进 `<exe 根>\temp\music-covers\`，返回绝对路径。
/// 内容哈希做文件名 ⇒ 同一张专辑封面只落一份盘（几十首共用一张图是常态）。
fn save_cover(dir: &Path, bytes: &[u8], ext: &str) -> Result<String, String> {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    let name = format!("{:016x}.{ext}", hasher.finish());
    let full = dir.join(&name);
    if full.is_file() {
        return Ok(full.display().to_string());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("创建封面目录失败：{e}"))?;
    // 先写临时文件再改名：半截的图被界面读到会「这张封面永远破图」。
    let tmp = dir.join(format!("{name}.part"));
    std::fs::write(&tmp, bytes).map_err(|e| format!("写封面失败：{e}"))?;
    std::fs::rename(&tmp, &full).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("封面改名失败：{e}")
    })?;
    Ok(full.display().to_string())
}

/// `lunac_music_library` 的 JSON：`tracks[]` / `roots[]` / `suggestedRoot` / `error`。
///
/// `suggestedRoot` 只是**空态时的推荐目录**（`%USERPROFILE%\Music`），不会被自动入库 ——
/// v1 的曲库必须由用户显式添加目录，这里保持同一口径。
pub fn library_json(exe_root: &Path) -> String {
    let roots = library_roots(exe_root);
    let cover_dir = exe_root.join("temp").join("music-covers");
    let outcome = scan_roots(&roots, &cover_dir);
    let error = if outcome.tracks.is_empty() && !outcome.errors.is_empty() {
        outcome.errors.join("；")
    } else {
        String::new()
    };
    let suggested = user_music_dir()
        .filter(|dir| dir.is_dir())
        .map(|dir| dir.display().to_string())
        .unwrap_or_default();
    serde_json::json!({
        "tracks": outcome.tracks,
        "roots": outcome.roots,
        "suggestedRoot": suggested,
        "error": error,
    })
    .to_string()
}

// ── LRC 歌词 ──────────────────────────────────────────────────────

/// 解析 LRC 文本（纯函数；单测覆盖多时间标签 / 坏行 / BOM）。
///
/// 时间标签口径与 v1 `music.ts:1407-1421` 一致：`[mm:ss]` / `[mm:ss.xx]` / `[mm:ss:xx]`，
/// 分钟 1–3 位、秒 1–2 位、小数 1–3 位；一行可以有多个标签（同一句歌词落在多个时刻）。
/// 元数据行（`[ar:…]`）与没有正文的行直接跳过。
pub fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = Vec::new();
    for raw in text.split(['\n', '\r']) {
        let mut stamps = Vec::new();
        let mut body = String::new();
        let mut rest = raw;
        while let Some(open) = rest.find('[') {
            body.push_str(&rest[..open]);
            let tail = &rest[open + 1..];
            let Some(close) = tail.find(']') else {
                // 没有闭合方括号：剩下整段当正文（坏行不丢字）。
                body.push('[');
                body.push_str(tail);
                rest = "";
                break;
            };
            if let Some(time_ms) = parse_timestamp(&tail[..close]) {
                stamps.push(time_ms);
            }
            rest = &tail[close + 1..];
        }
        body.push_str(rest);
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        for time_ms in stamps {
            lines.push(LyricLine {
                time_ms,
                text: body.to_string(),
            });
        }
    }
    lines.sort_by_key(|line| line.time_ms);
    lines
}

/// `[mm:ss]` / `[mm:ss.xx]` / `[mm:ss:xx]` → 毫秒；不是时间标签（如 `ar` / `ti`）返回 None。
fn parse_timestamp(tag: &str) -> Option<i64> {
    let tag = tag.trim();
    let (minutes, rest) = tag.split_once(':')?;
    if minutes.is_empty() || minutes.len() > 3 || !minutes.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (seconds, fraction) = match rest.find(['.', ':']) {
        Some(index) => (&rest[..index], Some(&rest[index + 1..])),
        None => (rest, None),
    };
    if seconds.is_empty() || seconds.len() > 2 || !seconds.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let fraction_ms = match fraction {
        Some(raw) => {
            if raw.is_empty() || raw.len() > 3 || !raw.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            // v1 的口径：右补 0 到 3 位（`.5` = 500ms，`.50` = 500ms）。
            let padded = format!("{raw:0<3}");
            padded.parse::<i64>().ok()?
        }
        None => 0,
    };
    Some(
        minutes.parse::<i64>().ok()? * 60_000
            + seconds.parse::<i64>().ok()? * 1_000
            + fraction_ms,
    )
}

/// 按字节解码 LRC：UTF-8（含 BOM）/ UTF-16（含 BOM）优先，非法 UTF-8 按 GBK 兜底。
pub fn decode_lyrics_bytes(bytes: &[u8]) -> String {
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        let (text, _, _) = encoding.decode(&bytes[bom_len..]);
        return text.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            let (text, _, _) = encoding_rs::GBK.decode(bytes);
            text.into_owned()
        }
    }
}

/// 找同名 `.lrc`（同目录；扩展名与文件名都大小写不敏感）。
pub fn find_lyrics_file(audio: &Path) -> Option<PathBuf> {
    let stem = audio.file_stem()?.to_string_lossy().to_string();
    let dir = audio.parent()?;
    let direct = dir.join(format!("{stem}.lrc"));
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        let is_lrc = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("lrc"));
        let same_stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(&stem));
        (is_lrc && same_stem).then_some(path)
    })
}

/// `lunac_music_lyrics` 的 JSON：`lines[]` / `error`。
pub fn lyrics_json(audio_path: &str) -> String {
    let trimmed = audio_path.trim();
    if trimmed.is_empty() {
        return serde_json::json!({
            "lines": [],
            "error": "没有要读取歌词的曲目",
        })
        .to_string();
    }
    let audio = Path::new(trimmed);
    if !audio.is_file() {
        return serde_json::json!({
            "lines": [],
            "error": "这个文件不在了 —— 可能被移动或删除，重新扫描一次",
        })
        .to_string();
    }
    let Some(lrc) = find_lyrics_file(audio) else {
        return serde_json::json!({ "lines": [], "error": null }).to_string();
    };
    let bytes = match std::fs::read(&lrc) {
        Ok(bytes) => bytes,
        Err(e) => {
            return serde_json::json!({
                "lines": [],
                "error": format!("读取歌词文件失败：{}（{e}）", lrc.display()),
            })
            .to_string();
        }
    };
    let lines = parse_lrc(&decode_lyrics_bytes(&bytes));
    serde_json::json!({ "lines": lines, "error": null }).to_string()
}

// ── 自定义队列（v1 `music.ts` 的 `LocalQueue`，2026-10-11 的多条版）──
//
// v1 把队列存在 localStorage、条目是**曲目快照**；v2 没有 WebView，落
// `config/music-queues.json`，条目只存**路径**（播放时按路径回查曲库，曲库没扫到
// 的路径仍保留在队列里 —— 与 v1 快照的「不因目录改动而丢歌」口径一致）。

/// 一条自定义队列。字段名与 v1 `LocalQueue` 一致（`id` / `name` / `items`）；
/// v2 的 `items` 改成字符串路径数组，免得把整份 `MediaTrackDto` 抄进配置。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct LocalQueue {
    pub id: String,
    pub name: String,
    pub paths: Vec<String>,
}

/// 队列条数 / 单条曲目数的上限（防止坏文件把面板画死）。
const MAX_QUEUES: usize = 200;
const MAX_QUEUE_ITEMS: usize = 5000;

/// 队列配置文件：`<exe 根>/config/music-queues.json`。
pub fn queues_path(exe_root: &Path) -> PathBuf {
    exe_root.join("config").join("music-queues.json")
}

/// 收敛队列：空 id 补 `queue-N`、id 去重（小写比较）、名称去空白、路径去重。
fn normalize_queues(list: Vec<LocalQueue>) -> Vec<LocalQueue> {
    let mut seen_ids = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (index, mut queue) in list.into_iter().enumerate() {
        queue.id = queue.id.trim().to_string();
        if queue.id.is_empty() {
            queue.id = format!("queue-{}", index + 1);
        }
        if !seen_ids.insert(queue.id.to_lowercase()) {
            continue;
        }
        queue.name = queue.name.trim().to_string();
        let mut seen_paths = std::collections::HashSet::new();
        queue.paths = queue
            .paths
            .into_iter()
            .map(|path| path.trim().to_string())
            .filter(|path| !path.is_empty())
            .filter(|path| seen_paths.insert(path.to_lowercase()))
            .take(MAX_QUEUE_ITEMS)
            .collect();
        out.push(queue);
        if out.len() >= MAX_QUEUES {
            break;
        }
    }
    out
}

/// 读自定义队列；缺文件 / 坏 JSON = 空表（不抛错、不建文件）。
pub fn read_queues(exe_root: &Path) -> Vec<LocalQueue> {
    let Ok(text) = std::fs::read_to_string(queues_path(exe_root)) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    // 兼容「裸数组」与 `{ "queues": [...] }` 两种写法。
    let list = value.get("queues").cloned().unwrap_or(value);
    let Ok(queues) = serde_json::from_value::<Vec<LocalQueue>>(list) else {
        return Vec::new();
    };
    normalize_queues(queues)
}

/// `lunac_music_queues` 的 JSON：`queues[]` / `error`。
pub fn queues_json(exe_root: &Path) -> String {
    serde_json::json!({ "queues": read_queues(exe_root), "error": "" }).to_string()
}

/// 覆盖写全部队列（读-解析-收敛-临时文件-改名）。返回收敛后的队列。
pub fn save_queues(exe_root: &Path, text: &str) -> Result<Vec<LocalQueue>, String> {
    let value =
        serde_json::from_str::<serde_json::Value>(text).map_err(|_| "队列配置不是合法 JSON")?;
    let list = value.get("queues").cloned().unwrap_or(value);
    let queues = serde_json::from_value::<Vec<LocalQueue>>(list)
        .map_err(|_| "队列配置格式不对（queues 必须是数组）")?;
    let queues = normalize_queues(queues);
    let path = queues_path(exe_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录失败：{e}"))?;
    }
    let body = serde_json::to_string_pretty(&serde_json::json!({ "queues": queues }))
        .map_err(|e| format!("序列化失败：{e}"))?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, body).map_err(|e| format!("写队列失败：{e}"))?;
    std::fs::rename(&temp, &path).map_err(|e| format!("写队列失败：{e}"))?;
    Ok(queues)
}

/// 把播放列表里的一行解析成绝对路径（相对条目按播放列表所在目录拼）。
fn resolve_playlist_entry(entry: &str, base: Option<&Path>) -> Option<String> {
    let mut raw = entry.trim();
    if raw.is_empty() {
        return None;
    }
    // `file:///D:/x.mp3` → `D:/x.mp3`（只做最常见的前缀剥离，不猜 percent-encoding）。
    if let Some(rest) = raw.strip_prefix("file:///").or_else(|| raw.strip_prefix("file://")) {
        raw = rest;
    }
    // 本地队列只收本地文件：`http(s)://` 之类一律跳过（v1 本地队列同口径）。
    if raw.contains("://") {
        return None;
    }
    let path = PathBuf::from(raw);
    let resolved = if path.is_absolute() {
        path
    } else if let Some(base) = base {
        base.join(path)
    } else {
        path
    };
    is_audio_path(&resolved).then(|| resolved.to_string_lossy().to_string())
}

/// 解析 `.m3u` / `.m3u8` / `.pls`（v1 的导入口径：只认音频扩展名、去重、跳过注释）。
///
/// `.pls` 的 `FileN=...` 行与 `.m3u` 的裸路径行共用同一个循环：认 `FileN` 前缀，
/// 其余整行当路径。
pub fn parse_playlist(text: &str, base: Option<&Path>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let entry = match line.split_once('=') {
            Some((key, value)) if key.trim().to_ascii_lowercase().starts_with("file") => {
                value.trim()
            }
            _ => line,
        };
        if let Some(path) = resolve_playlist_entry(entry, base) {
            if seen.insert(path.to_lowercase()) {
                out.push(path);
            }
        }
    }
    out
}

/// 弹系统的「选择播放列表」对话框（单选 `.m3u/.m3u8/.pls`）；取消返回空串。
pub(crate) fn pick_playlist_file() -> String {
    #[cfg(windows)]
    unsafe {
        use windows::core::{PCWSTR, PWSTR};
        use windows::Win32::UI::Controls::Dialogs::{
            GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST,
            OPENFILENAMEW,
        };
        let filter: Vec<u16> =
            "播放列表 (*.m3u;*.m3u8;*.pls)\0*.m3u;*.m3u8;*.pls\0所有文件 (*.*)\0*.*\0\0"
                .encode_utf16()
                .collect();
        let title: Vec<u16> = "选择播放列表\0".encode_utf16().collect();
        let mut buffer = vec![0u16; 4096];
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: PWSTR(buffer.as_mut_ptr()),
            nMaxFile: buffer.len() as u32,
            lpstrTitle: PCWSTR(title.as_ptr()),
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        if !GetOpenFileNameW(&mut ofn).as_bool() {
            return String::new();
        }
        let len = buffer.iter().position(|c| *c == 0).unwrap_or(0);
        String::from_utf16_lossy(&buffer[..len])
    }
    #[cfg(not(windows))]
    {
        String::new()
    }
}

/// 导入播放列表：弹框 → 解析 → 返回 `{ ok, canceled, name, paths[], error }`。
pub fn import_playlist_json() -> String {
    let picked = pick_playlist_file();
    if picked.trim().is_empty() {
        return serde_json::json!({
            "ok": false, "canceled": true, "name": "", "paths": [], "error": "",
        })
        .to_string();
    }
    let playlist = Path::new(&picked);
    let name = playlist
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let bytes = match std::fs::read(playlist) {
        Ok(bytes) => bytes,
        Err(e) => {
            return serde_json::json!({
                "ok": false, "canceled": false, "name": name, "paths": [],
                "error": format!("读取播放列表失败：{e}"),
            })
            .to_string();
        }
    };
    let paths = parse_playlist(&decode_lyrics_bytes(&bytes), playlist.parent());
    if paths.is_empty() {
        return serde_json::json!({
            "ok": false, "canceled": false, "name": name, "paths": [],
            "error": "播放列表里没有可用的音频文件",
        })
        .to_string();
    }
    serde_json::json!({
        "ok": true, "canceled": false, "name": name, "paths": paths, "error": "",
    })
    .to_string()
}

/// 弹多选框选音频文件（新建队列用）。返回 `{ ok, paths[], error }`。
pub fn pick_audio_files_json() -> String {
    let paths = crate::convert::pick_inputs("audio");
    serde_json::json!({ "ok": !paths.is_empty(), "paths": paths, "error": "" }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lunac-music-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn times(lines: &[LyricLine]) -> Vec<i64> {
        lines.iter().map(|line| line.time_ms).collect()
    }

    #[test]
    fn parse_lrc_supports_multiple_timestamps_and_bad_lines() {
        let text = "[00:01.50]第一句\n\
                    [00:02]第二句\n\
                    bad line without timestamp\n\
                    [00:05.5][00:10.25]重复句\n\
                    [ar:歌手]\n\
                    [00:03.005]毫秒三位";
        let lines = parse_lrc(text);
        assert_eq!(
            times(&lines),
            vec![1500, 2000, 3005, 5500, 10250],
            "多时间标签要拆成多行并按时间排序"
        );
        assert_eq!(lines[0].text, "第一句");
        assert_eq!(lines[3].text, "重复句");
        assert_eq!(lines[4].text, "重复句");
        // 坏行与元数据行都跳过。
        assert!(lines.iter().all(|line| !line.text.contains("bad line")));
        assert!(lines.iter().all(|line| !line.text.contains("歌手")));
    }

    #[test]
    fn parse_lrc_handles_bom_crlf_and_colon_fraction() {
        let text = "\u{feff}[00:00:50]冒号小数\r\n[00:04]第二行\r\n";
        let lines = parse_lrc(text);
        assert_eq!(times(&lines), vec![500, 4000]);
        assert_eq!(lines[0].text, "冒号小数");
    }

    #[test]
    fn lrc_bytes_decode_utf8_bom_utf16_and_gbk() {
        let mut utf8 = vec![0xEF, 0xBB, 0xBF];
        utf8.extend_from_slice("[00:01]晴天".as_bytes());
        assert_eq!(decode_lyrics_bytes(&utf8), "[00:01]晴天");

        let mut utf16 = vec![0xFF, 0xFE];
        for unit in "[00:01]夜曲".encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_lyrics_bytes(&utf16), "[00:01]夜曲");

        let (gbk, _, _) = encoding_rs::GBK.encode("[00:02]稻香");
        assert_eq!(decode_lyrics_bytes(&gbk), "[00:02]稻香");
    }

    #[test]
    fn find_lyrics_file_matches_same_stem_case_insensitively() {
        let dir = temp_dir("lyrics");
        let audio = dir.join("Song.MP3");
        std::fs::write(&audio, b"fake").unwrap();
        std::fs::write(dir.join("song.LRC"), "[00:01]hi").unwrap();

        let found = find_lyrics_file(&audio).expect("find lrc");
        // Windows 文件系统大小写不敏感：直接命中的路径可能带「请求时的大小写」，
        // 这里只校验它确实是同名那份歌词。
        assert_eq!(
            found.file_name().unwrap().to_string_lossy().to_lowercase(),
            "song.lrc"
        );

        let missing = dir.join("other.mp3");
        std::fs::write(&missing, b"fake").unwrap();
        assert!(find_lyrics_file(&missing).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_roots_keeps_unparseable_audio_with_filename_fallback() {
        let dir = temp_dir("scan");
        let album = dir.join("Album");
        std::fs::create_dir_all(&album).unwrap();
        // 假文件：lofty 解不出标签，但扫描要退回「文件名」而不是丢掉。
        std::fs::write(dir.join("晴天.mp3"), b"not really audio").unwrap();
        std::fs::write(album.join("夜曲.flac"), b"not really audio").unwrap();
        std::fs::write(dir.join("readme.txt"), b"ignored").unwrap();

        let outcome = scan_roots(&[dir.clone()], &dir.join("covers"));
        let titles: Vec<&str> = outcome.tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["夜曲", "晴天"], "按专辑/标题排序，且非音频被过滤");
        assert!(outcome.tracks.iter().all(|t| t.duration_ms == 0));
        assert!(outcome.tracks.iter().all(|t| t.cover.is_empty()));
        assert_eq!(outcome.roots.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lyrics_json_reads_same_name_lrc_and_reports_missing_file() {
        let dir = temp_dir("lyrics-json");
        let audio = dir.join("song.mp3");
        std::fs::write(&audio, b"fake").unwrap();
        std::fs::write(dir.join("song.lrc"), "[00:01.20]第一句\n[00:03]第二句\n").unwrap();

        let parsed: serde_json::Value =
            serde_json::from_str(&lyrics_json(&audio.display().to_string())).unwrap();
        assert!(parsed["error"].is_null());
        assert_eq!(parsed["lines"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["lines"][0]["timeMs"], 1200);
        assert_eq!(parsed["lines"][0]["text"], "第一句");

        // 没有同名 .lrc：空数组 + 无错误（面板显示「未找到歌词」）。
        let no_lrc = dir.join("other.mp3");
        std::fs::write(&no_lrc, b"fake").unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&lyrics_json(&no_lrc.display().to_string())).unwrap();
        assert!(parsed["lines"].as_array().unwrap().is_empty());
        assert!(parsed["error"].is_null());

        // 文件不存在：一句人话。
        let parsed: serde_json::Value =
            serde_json::from_str(&lyrics_json(&dir.join("gone.mp3").display().to_string())).unwrap();
        assert!(parsed["error"].as_str().is_some_and(|e| e.contains("不在了")));
        assert!(parsed["lines"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn library_json_has_the_contract_fields() {
        let dir = temp_dir("library");
        std::fs::write(dir.join("Track.mp3"), b"not really audio").unwrap();
        let cover_dir = dir.join("covers");
        let outcome = scan_roots(&[dir.clone()], &cover_dir);
        let json = serde_json::to_string(&serde_json::json!({
            "tracks": outcome.tracks,
            "roots": outcome.roots,
            "error": "",
        }))
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let tracks = parsed["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 1);
        for key in ["path", "title", "artist", "album", "duration_ms", "cover"] {
            assert!(tracks[0].get(key).is_some(), "missing {key}");
        }
        assert_eq!(parsed["roots"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalize_root_matches_v1_rules() {
        assert_eq!(normalize_root(r"D:\Music\"), r"D:\Music");
        assert_eq!(normalize_root("  D:\\Music\\\\  "), r"D:\Music");
        assert_eq!(normalize_root("D:/Music/"), "D:/Music");
        assert_eq!(normalize_root("D:"), r"D:\");
        assert_eq!(normalize_root("D:\\"), r"D:\");
        assert_eq!(normalize_root("   "), "");
    }

    #[test]
    fn add_and_remove_root_round_trip_on_the_v1_database() {
        let exe = temp_dir("roots-db");
        let lib = exe.join("Music");
        std::fs::create_dir_all(&lib).unwrap();

        // 首次添加会按需建 <exe>/ModuleData/music/media.db 与 roots 表。
        let stored = add_root(&exe, &format!("{}\\", lib.display())).unwrap();
        assert_eq!(stored, lib.display().to_string());
        assert_eq!(library_roots(&exe), vec![lib.clone()]);

        // 重复添加（含尾分隔符的不同写法）不产生第二条。
        add_root(&exe, &lib.display().to_string()).unwrap();
        assert_eq!(library_roots(&exe).len(), 1);

        // 不存在的目录拒绝入库。
        let missing = exe.join("nope");
        assert!(add_root(&exe, &missing.display().to_string()).is_err());

        // 移除后曲库为空；再删一次也成功（幂等）。
        remove_root(&exe, &lib.display().to_string()).unwrap();
        assert!(library_roots(&exe).is_empty());
        remove_root(&exe, &lib.display().to_string()).unwrap();

        let _ = std::fs::remove_dir_all(&exe);
    }

    #[test]
    fn parse_playlist_handles_m3u_m3u8_pls_and_file_urls() {
        let base = Path::new(r"D:\Music\lists");
        let m3u = "#EXTM3U\n\
                   #EXTINF:123,歌手 - 歌名\n\
                   D:\\Music\\a.mp3\n\
                   a.mp3\n\
                   ..\\b.flac\n\
                   notes.txt\n\
                   https://example.com/c.mp3\n";
        let paths = parse_playlist(m3u, Some(base));
        assert_eq!(
            paths,
            vec![
                r"D:\Music\a.mp3".to_string(),
                r"D:\Music\lists\a.mp3".to_string(),
                r"D:\Music\lists\..\b.flac".to_string(),
            ],
            "注释 / 非音频行剔除，相对路径按列表目录拼"
        );

        // .pls 的 FileN= 行
        let pls = "[playlist]\nNumberOfEntries=2\nFile1=D:\\Music\\x.ogg\nFile2=y.wav\nVersion=2\n";
        assert_eq!(
            parse_playlist(pls, Some(base)),
            vec![
                r"D:\Music\x.ogg".to_string(),
                r"D:\Music\lists\y.wav".to_string()
            ]
        );

        // file:/// 前缀（Windows 盘符）
        let url = "file:///D:/Music/z.mp3\n";
        assert_eq!(parse_playlist(url, None), vec!["D:/Music/z.mp3".to_string()]);
    }

    #[test]
    fn queues_round_trip_normalizes_and_rejects_bad_json() {
        let exe = temp_dir("queues");
        assert!(read_queues(&exe).is_empty(), "缺文件 = 空表");

        let saved = save_queues(
            &exe,
            &serde_json::json!({
                "queues": [
                    { "id": "", "name": "  我的歌单  ", "paths": ["D:/a.mp3", "D:/A.MP3", " "] },
                    { "id": "q-2", "name": "通勤", "paths": ["D:/b.flac"] },
                    { "id": "Q-2", "name": "重复 id", "paths": [] },
                ]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(saved.len(), 2, "重复 id 的小写比较要去掉后一条");
        assert_eq!(saved[0].id, "queue-1", "空 id 按序号补");
        assert_eq!(saved[0].name, "我的歌单");
        assert_eq!(saved[0].paths, vec!["D:/a.mp3".to_string()]);
        assert_eq!(saved[1].id, "q-2");

        // 落盘后可读回（字段名 camelCase，路径保持原样）。
        let text = std::fs::read_to_string(queues_path(&exe)).unwrap();
        assert!(text.contains("\"queues\""));
        assert_eq!(read_queues(&exe).len(), 2);

        // 坏 JSON 整条拒绝，盘上内容不变。
        let before = std::fs::read_to_string(queues_path(&exe)).unwrap();
        assert!(save_queues(&exe, "{oops").is_err());
        assert!(save_queues(&exe, r#"{"queues": 3}"#).is_err());
        assert_eq!(std::fs::read_to_string(queues_path(&exe)).unwrap(), before);

        let _ = std::fs::remove_dir_all(&exe);
    }
}
