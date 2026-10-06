// media_lib.rs
// 本地音乐**媒体库**（2026-10-01）—— backlog L9「本地音乐解析」的第一刀。
//
// ── 范围（刻意划得很窄，别顺手扩）──────────────────────────────────
// 只做「媒体库」那一层：**目录扫描 + 标签 / 时长 / 封面 + 浏览 + 筛选 + 播放列表解析**。
//   · **不做播放**：解码出声在 `player.rs`（本地文件也要过 DSP，backlog L9 明写
//     「别做两套播放内核」）⇒ 这里一个解码器都不引（无 rodio / symphonia / ffmpeg）。
//   · **不引 FFmpeg 全家桶**：读标签用纯 Rust 的 `lofty`，它**不解码音频**就能给出
//     时长与内嵌封面 —— 这正是「不要 FFmpeg」能成立的原因。
//   · **播放列表是「读文件」不是「建索引」**（2026-10-01 第二批）：`.m3u` / `.m3u8` /
//     `.pls` 打开时才解析，**不入 SQLite**。理由：播放列表是用户手里的文件（可能指向
//     库外的目录、随时会被改），把它抄进库里就有了两份真相。
//
// ── 落点（与全仓同一条纪律：一切在 exe 根下）──────────────────────
//   · 索引与「用户添加的目录」= `<exe 根>\ModuleData\music\media.db`（业务数据）
//   · 封面缓存                  = `<exe 根>\temp\music-covers\<sha256 前 16 位>.<ext>`（可删缓存）
//   删掉 media.db = 忘掉整个本地库（与「删 music.json = 忘掉账号」同一口径）；
//   删掉 music-covers 只是下次浏览要重新抽一次封面，索引不受影响。
//
// ── 两条实现纪律 ────────────────────────────────────────────────
//   ① **扩展名只做初筛，内容才是判据**：先按扩展名过掉明显不是音频的东西（省掉对
//      每一个 .txt 调一次解析器），再交给 `lofty` 真正解析 —— 解析失败的算
//      `failed` 并跳过。**绝不**只看扩展名就入库（那个 .mp3 可能是个 txt）。
//   ② **扫描永远在后台线程**：一旦开始就立刻返回，进度由 `media_scan_status` 轮询读。
//      绝不在命令里同步扫完 —— 那正是本仓「一搜就卡」的旧病（见 file_indexer.rs 头注释）。
//
// ── 增量扫描 ────────────────────────────────────────────────────
// 判据是 **(size, mtime_ms) 都相同**：相同就整条跳过（连标签都不读），并记进「本次见到」
// 集合；扫描结束把该根下**没见到**的行删掉（文件被删 / 改名都走这一条）。
// 代价是「只改标签不动文件」不会触发更新 —— 那是刻意换来的速度（重扫一个大目录
// 从「重解析每一首」降到「只 stat」）。

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use rusqlite::{params_from_iter, types::Value, Connection};

// ── 常量 ──────────────────────────────────────────────────────────

/// 允许入库的**音频**扩展名（`lofty` 认得的那些容器）。**只做初筛**，见头注释第 ① 条。
const AUDIO_EXTS: &[&str] = &[
    "mp3", "flac", "m4a", "m4b", "mp4", "aac", "ogg", "oga", "opus", "spx", "wav", "wave",
    "aif", "aiff", "aifc", "ape", "wv", "mpc",
];

/// 允许入库的**视频**扩展名（2026-10-01 用户第 8 条：本地播放要支持视频）。
///
/// **这一份名单是「能真的出画面 + 出声音」的交集，不是「常见的视频格式」**：
///   · 画面由插件窗里的 `<video>`（WebView2 = Chromium）解 —— 它认的容器是
///     mp4 / m4v / mov（H.264 + AAC 是它的原生组合）；
///   · 声音仍由宿主 `player.rs` 的 rodio 解 —— rodio 默认特性里就有
///     `symphonia-isomp4` + `symphonia-aac`（见它的 Cargo.toml `mp4 = [...]`），
///     所以上面这三个容器里的音轨**同一条链路就能出声**。
///
/// **刻意不收的三种，理由逐条写清**（别按「常见格式」把它们加进来）：
///   · `mkv` / `avi`：Chromium **不出画面**（它不支持这两个容器）⇒ 加进来只会得到
///     「点了没反应」或一条没有画面的音频流；
///   · `webm`：画面能出，但它的音轨绝大多数是 **Opus**，而 rodio 的默认特性里
///     **没有 opus 解码**（`symphonia-all` 才有）⇒ 会变成「有画面没声音」——
///     那是最难查的一类静默失败，宁可不收。
/// 想要播这几种的用户，走**已有的转换插件**（ffmpeg）转成 mp4。
const VIDEO_EXTS: &[&str] = &["mp4", "m4v", "mov"];

/// 目录遍历深度上限（相对根）—— 防「指向 C:\ 的根」把整台机器走穿。
const MAX_DEPTH: usize = 16;
/// 单次扫描**候选文件**上限。到了就停（记进日志），防止一次误点扫描把内存吃光。
const MAX_CANDIDATES: usize = 200_000;
/// 封面字节上限：超过就只记「有封面但太大、不入缓存」，不给 WebView 塞几 MB 的图。
const COVER_MAX_BYTES: usize = 3 * 1024 * 1024;
/// 每次查询最多返回多少条（IPC 体积闸；被截断时 `total` 仍是真的，界面据此说明）。
const MAX_QUERY_LIMIT: u32 = 2000;
// 扫描**刻意不包一个大事务**（原打算每 300 条提交一次，写起来是个易错的状态机）：
// 库里是 WAL + `synchronous=NORMAL`，每条语句自成一个事务也**不 fsync**（只往 WAL 追加
// 一帧），万条量级多花的不过一秒上下；换来的是「一边扫一边能浏览」这件用户看得见的事，
// 以及没有「中途出错留下半个事务」那种要靠 ROLLBACK 兜的烂摊子。

// ── DTO（给前端的形状；字段名走 serde 默认的 snake_case，与 music.rs 一致）──

#[derive(Debug, Serialize, Default)]
pub struct MediaRootDto {
    pub path: String,
    /// 目录名（末段）—— 界面上那行显示它，完整路径放 title。
    pub name: String,
    pub tracks: i64,
    pub last_scan_at: i64,
    /// 目录现在还在不在（被删 / 拔盘的根要能看出来，别让用户对着 0 首发愣）。
    pub missing: bool,
}

#[derive(Debug, Serialize, Default)]
pub struct MediaTrackDto {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: i64,
    /// **已缓存封面的绝对路径**（空串 = 这首没有封面）。前端用 `convertFileSrc` 变成
    /// 能在 `<img src>` 里用的 URL（asset 协议，CSP 的 img-src 已放行）。
    pub cover: String,
    pub ext: String,
    pub size: i64,
    /// **要出画面的视频**（2026-10-01 用户第 8 条）。判据只有一条：扩展名在 `VIDEO_EXTS` 里
    /// （`is_video_ext`）—— 前端据此决定要不要开那块跟随宿主的画面层。
    /// **不落库**：它是从 `ext` 现算的（多存一列就会与 `VIDEO_EXTS` 漂移）。
    pub video: bool,
}

#[derive(Debug, Serialize, Default)]
pub struct MediaTracksDto {
    /// 命中总数（**不受 limit 影响**）—— 被截断时界面要如实说明。
    pub total: i64,
    pub items: Vec<MediaTrackDto>,
}

#[derive(Debug, Serialize, Default, Clone)]
pub struct ScanStatusDto {
    pub running: bool,
    pub scanned: u64,
    /// 候选文件总数（先数一遍再解析，这样进度条是真的）。0 = 还没数完。
    pub total: u64,
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    /// 本次**没重新解析**的（size+mtime 都没变）—— 它才是「重扫很快」的来源。
    pub unchanged: u64,
    /// 扩展名像音频、但 `lofty` 解析不了（坏文件 / 假扩展名）。
    pub failed: u64,
    pub error: String,
    pub done_at: i64,
}

// ── 落点 ──────────────────────────────────────────────────────────

fn media_dir() -> PathBuf {
    crate::storage::module_data_dir().join("music")
}

fn db_path() -> PathBuf {
    media_dir().join("media.db")
}

fn covers_dir() -> PathBuf {
    crate::storage::lunac_root_dir().join("temp").join("music-covers")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── 纯函数（能单测的都放这里）────────────────────────────────────

/// 扩展名是不是「可能装音频」（小写比较；无扩展名 = 不是）。
fn is_audio_ext(ext: &str) -> bool {
    let e = ext.trim_start_matches('.').to_ascii_lowercase();
    AUDIO_EXTS.contains(&e.as_str())
}

/// 扩展名是不是**要出画面的视频**（`VIDEO_EXTS`）。
///
/// ⚠️ `mp4` 在**两张表里都有** —— 这是有意的：它既是音频容器（只有音轨的 m4a 式 mp4），
/// 也是视频容器。所以「是不是视频」不能只看它在不在 `AUDIO_EXTS` 里，
/// 而是**只看这一条**（前端拿 `MediaTrackDto.video` 决定要不要开画面层）。
///
/// `pub(crate)`：`player.rs` 的 `PlayerDto.video` 也用它 —— **这张表只有一份**
/// （在那里再抄一遍，就是「加了一个格式、只有一侧认」的经典静默失效）。
pub(crate) fn is_video_ext(ext: &str) -> bool {
    let e = ext.trim_start_matches('.').to_ascii_lowercase();
    VIDEO_EXTS.contains(&e.as_str())
}

/// 能不能进媒体库（音频 ∪ 视频）。**两张表分开留着**的理由见 `is_video_ext`。
fn is_indexable_ext(ext: &str) -> bool {
    is_audio_ext(ext) || is_video_ext(ext)
}

/// 把用户给的目录规范化成**唯一形态**：去首尾空白、去尾分隔符。
///
/// 为什么必须规范化：`roots` 的主键就是它 —— `D:\Music\` 与 `D:\Music` 是同一个目录，
/// 两条记录会让「同一批文件入库两次、删一个根还剩一半」。
fn normalize_root(input: &str) -> String {
    let t = input.trim().trim_end_matches(['\\', '/']);
    // 盘符根（`D:` / `D:\`）去尾之后会变成 `D:`（在 Windows 上那是「D 盘的当前目录」，
    // 语义与 `D:\` 不同）⇒ 单独补回来。
    if t.len() == 2 && t.ends_with(':') {
        return format!("{t}\\");
    }
    if t.is_empty() {
        input.trim().to_string()
    } else {
        t.to_string()
    }
}

/// 没有标签时的标题兜底：文件名（不含扩展名）。
fn title_from_path(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default())
}

/// 会不会是「同一份封面」：字节级内容哈希（同专辑几十首共用一张图 ⇒ 只写一个文件）。
fn cover_file_name(bytes: &[u8], ext: &str) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let hex = format!("{:x}", h.finalize());
    format!("{}.{}", &hex[..16], ext)
}

/// 封面 MIME → 文件扩展名。用字符串包含判断而不是枚举匹配：`lofty` 的 `MimeType`
/// 是它自己的枚举，写法随版本可能变；这里只需要「能不能让浏览器认出来」。
fn mime_to_ext(mime: &str) -> &'static str {
    let m = mime.to_ascii_lowercase();
    if m.contains("png") {
        "png"
    } else if m.contains("gif") {
        "gif"
    } else if m.contains("bmp") {
        "bmp"
    } else if m.contains("webp") {
        "webp"
    } else {
        "jpg"
    }
}

/// 增量判据：**两个都要相同**才算没变。
///
/// 只比 mtime 会在「复制文件保留原时间戳」时漏更新；只比 size 会在「标签改了但长度
/// 恰好没变」时漏更新。两个一起比是这一层的取舍（见文件头注释）。
fn is_unchanged(old_size: i64, old_mtime: i64, size: i64, mtime: i64) -> bool {
    old_size == size && old_mtime == mtime && old_mtime != 0
}

/// 给 `LIKE` 用的转义：`%` / `_` / `\` 都要转义，并配 `ESCAPE '\'`。
///
/// 不转义的话，用户搜一个 `%` 会命中整库（`LIKE '%%%'`），而被当成「搜索坏了」。
fn escape_like(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 8);
    for c in q.chars() {
        match c {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

// ── 播放列表解析（.m3u / .m3u8 / .pls，2026-10-01 第二批）──────────
//
// **为什么自己写而不引库**：这两种格式的「有用部分」就是**一列路径** ——
// m3u 是「除空行与 `#` 开头的行以外的每一行」，pls 是「`FileN=` 后面的值」。
// 引一个播放列表 crate 换来的是一整套 `#EXTINF` 元数据模型，而这里的标题 / 时长
// 一律**以音频文件自己的标签为准**（`lofty`）—— 借用列表文件里那行手写标注反而
// 会有两份可能不一致的真相。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaylistFormat {
    M3u,
    Pls,
}

/// 认得这三种后缀（`.m3u8` 与 `.m3u` 是**同一套语法**，区别只在编码约定）。
fn playlist_format(path: &Path) -> Option<PlaylistFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "m3u" | "m3u8" => Some(PlaylistFormat::M3u),
        "pls" => Some(PlaylistFormat::Pls),
        _ => None,
    }
}

/// 把播放列表文件的字节解成文本。**三种编码都要认**：
///   ① UTF-8（含 BOM）—— `.m3u8` 与现在的导出工具；
///   ② UTF-16（含 BOM，LE / BE）—— Windows 记事本「另存为 Unicode」的产物；
///   ③ **ANSI / GBK** —— 老播放器写出来的中文路径。
/// ③ 是关键的一条：不认它的话中文路径会解成乱码，表现是「播放列表里一首都不显示」，
/// 而用户完全看不出这是编码问题（`encoding_rs` 见 Cargo.toml 的说明）。
fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, false);
    }
    // 无 BOM：先按 UTF-8 严格解（成功就说明本来就是它），失败才当 GBK。
    // GB18030 是 GBK 的超集，中文环境这两者够用。
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::GBK.decode(bytes).0.into_owned(),
    }
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| {
            if little_endian {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

/// 解出一列**原始条目**（文件里的写法，还没解析成绝对路径），顺序即文件里的顺序。
fn parse_playlist_entries(text: &str, fmt: PlaylistFormat) -> Vec<String> {
    match fmt {
        PlaylistFormat::M3u => parse_m3u(text),
        PlaylistFormat::Pls => parse_pls(text),
    }
}

/// m3u：`#` 开头的是指令 / 注释（`#EXTM3U`、`#EXTINF:秒数,标题`），其余非空行是路径。
fn parse_m3u(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect()
}

/// pls：INI 形态，只认 `FileN=`。**按 N 排序** —— 文件里的行序不保证与序号一致
/// （有些工具会在末尾追加而不是插入）。
fn parse_pls(text: &str) -> Vec<String> {
    let mut out: Vec<(u32, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('[') || line.starts_with(';') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let key = k.trim().to_ascii_lowercase();
        let Some(n) = key.strip_prefix("file") else { continue };
        // `file` 后面必须**全是数字**：不然 `filename=` 这类键也会被当成一首歌。
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(idx) = n.parse::<u32>() else { continue };
        let v = v.trim();
        if v.is_empty() {
            continue;
        }
        out.push((idx, v.to_string()));
    }
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, v)| v).collect()
}

/// 条目 → 绝对路径。两种写法都有：绝对路径、与播放列表同目录的相对路径。
/// `file://` 前缀（少数导出工具会写）也一并剥掉 —— 不剥的话它会被当成相对路径，
/// 永远找不到，而且**一首都不报错**。
fn resolve_playlist_entry(entry: &str, base: &Path) -> PathBuf {
    let raw = entry.strip_prefix("file://").unwrap_or(entry);
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        p
    } else {
        base.join(p)
    }
}

// ── 库结构 ────────────────────────────────────────────────────────

fn open(path: &Path) -> Result<Connection, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建媒体库目录失败: {e}"))?;
    }
    let conn = Connection::open(path).map_err(|e| format!("打开媒体库失败: {e}"))?;
    // 与 chat_db 同一口径：WAL + 等锁。**扫描写、界面读是并发的**，没有这两条就会
    // 在扫描中途「浏览本地库」时报 database is locked。
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=3000;");
    init_schema(&conn)?;
    Ok(conn)
}

fn open_db() -> Result<Connection, String> {
    open(&db_path())
}

fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS roots (
          path         TEXT PRIMARY KEY,
          added_at     INTEGER NOT NULL,
          last_scan_at INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS tracks (
          path         TEXT PRIMARY KEY,
          root         TEXT NOT NULL,
          title        TEXT NOT NULL DEFAULT '',
          artist       TEXT NOT NULL DEFAULT '',
          album        TEXT NOT NULL DEFAULT '',
          album_artist TEXT NOT NULL DEFAULT '',
          track_no     INTEGER NOT NULL DEFAULT 0,
          year         INTEGER NOT NULL DEFAULT 0,
          genre        TEXT NOT NULL DEFAULT '',
          duration_ms  INTEGER NOT NULL DEFAULT 0,
          ext          TEXT NOT NULL DEFAULT '',
          size         INTEGER NOT NULL DEFAULT 0,
          mtime        INTEGER NOT NULL DEFAULT 0,
          cover        TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_tracks_root   ON tracks(root);
        CREATE INDEX IF NOT EXISTS idx_tracks_album  ON tracks(album);
        CREATE INDEX IF NOT EXISTS idx_tracks_artist ON tracks(artist);
        "#,
    )
    .map_err(|e| format!("建媒体库表失败: {e}"))
}

// ── 根目录的增删查 ────────────────────────────────────────────────

fn list_roots_on(conn: &Connection) -> Result<Vec<MediaRootDto>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT r.path, r.last_scan_at,
                    (SELECT COUNT(*) FROM tracks t WHERE t.root = r.path)
             FROM roots r ORDER BY r.added_at",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let path: String = row.get(0)?;
            Ok(MediaRootDto {
                name: Path::new(&path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone()),
                missing: !Path::new(&path).is_dir(),
                path,
                last_scan_at: row.get(1)?,
                tracks: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

fn add_root_on(conn: &Connection, path: &str) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO roots (path, added_at) VALUES (?1, ?2)",
        rusqlite::params![path, now_ms()],
    )
    .map_err(|e| format!("添加目录失败: {e}"))?;
    Ok(())
}

/// 删根：**连同它的曲目一起删**（留着就是永不失效的幽灵条目）。
fn remove_root_on(conn: &mut Connection, path: &str) -> Result<usize, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM tracks WHERE root = ?1", rusqlite::params![path])
        .map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM roots WHERE path = ?1", rusqlite::params![path])
        .map_err(|e| e.to_string())?;
    let n = tx.changes() as usize;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

// ── 曲目读写 ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct TrackRow {
    path: String,
    root: String,
    title: String,
    artist: String,
    album: String,
    album_artist: String,
    track_no: i64,
    year: i64,
    genre: String,
    duration_ms: i64,
    ext: String,
    size: i64,
    mtime: i64,
    cover: String,
}

fn upsert_track(conn: &Connection, t: &TrackRow) -> Result<(), String> {
    conn.execute(
        r#"INSERT INTO tracks
             (path, root, title, artist, album, album_artist, track_no, year, genre,
              duration_ms, ext, size, mtime, cover)
           VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
           ON CONFLICT(path) DO UPDATE SET
             root=excluded.root, title=excluded.title, artist=excluded.artist,
             album=excluded.album, album_artist=excluded.album_artist,
             track_no=excluded.track_no, year=excluded.year, genre=excluded.genre,
             duration_ms=excluded.duration_ms, ext=excluded.ext,
             size=excluded.size, mtime=excluded.mtime, cover=excluded.cover"#,
        rusqlite::params![
            t.path, t.root, t.title, t.artist, t.album, t.album_artist, t.track_no, t.year,
            t.genre, t.duration_ms, t.ext, t.size, t.mtime, t.cover
        ],
    )
    .map_err(|e| format!("写入曲目失败: {e}"))?;
    Ok(())
}

/// 读某一首已入库的 `(size, mtime)`（增量扫描用）。没入库返回 `None`。
fn existing_stamp(conn: &Connection, path: &str) -> Option<(i64, i64)> {
    conn.query_row(
        "SELECT size, mtime FROM tracks WHERE path = ?1",
        rusqlite::params![path],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

fn to_dto(r: &TrackRow) -> MediaTrackDto {
    MediaTrackDto {
        path: r.path.clone(),
        title: r.title.clone(),
        artist: r.artist.clone(),
        album: r.album.clone(),
        duration_ms: r.duration_ms,
        cover: if r.cover.is_empty() {
            String::new()
        } else {
            covers_dir().join(&r.cover).display().to_string()
        },
        ext: r.ext.clone(),
        size: r.size,
        video: is_video_ext(&r.ext),
    }
}

/// 给一个**任意路径**做一份 `MediaTrackDto`（播放列表用）。
///
/// 两条路：**已入库的**直接用那一行（有封面缓存、不必再读一遍标签）；**没入库的**
/// 现读 —— 播放列表可以指向库外的目录（用户从别人那儿拷来的一份 `.m3u` 就是这种），
/// 那些文件没有理由不入库，也不该因此就不显示。
///
/// 返回 `None` 表示「这不是一首能播的音频」（扩展名不认识 / `lofty` 解析不了）。
/// **静默跳过是刻意的**：一份播放列表里有一行是坏的（被删、被改名）不该让整份
/// 都打不开 —— 与扫描里 `failed` 那条口径一致。
fn track_dto_for_path(conn: &Connection, path: &Path) -> Option<MediaTrackDto> {
    let key = path.display().to_string();
    if let Ok(r) = conn.query_row(
        "SELECT path, root, title, artist, album, album_artist, track_no, year, genre,
                duration_ms, ext, size, mtime, cover
         FROM tracks WHERE path = ?1",
        rusqlite::params![key],
        row_to_track,
    ) {
        return Some(to_dto(&r));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !is_indexable_ext(&ext) {
        return None;
    }
    let video = is_video_ext(&ext);
    // 视频容器的标签大概率读不出来（同扫描那处的理由）⇒ 用只有文件名那条兜底，
    // 而不是把这一行从播放列表里**静默丢掉**
    let t = match read_tags(path) {
        Ok(t) => t,
        Err(_) if video => video_fallback_tags(path),
        Err(_) => return None,
    };
    let size = std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
    Some(MediaTrackDto {
        path: key,
        title: t.title,
        artist: t.artist,
        album: t.album,
        duration_ms: t.duration_ms,
        cover: if t.cover.is_empty() {
            String::new()
        } else {
            covers_dir().join(&t.cover).display().to_string()
        },
        ext,
        size,
        video,
    })
}

/// 读一份播放列表文件（`.m3u` / `.m3u8` / `.pls`）。**同步实现**，命令那层套
/// `run_blocking`（要读盘，还要为每首读一次标签）。
///
/// 连接由调用方给（与 `query_tracks_on` 同一形态）：判断「这一首在不在库里」要查
/// `tracks` 表，而**连接只该开一次** —— 一份播放列表几百首，每首开一次库就是几百次
/// WAL 打开。
fn read_playlist(conn: &Connection, path: &str) -> Result<MediaTracksDto, String> {
    let p = PathBuf::from(path);
    let fmt = playlist_format(&p).ok_or_else(|| "ERR_NOT_PLAYLIST".to_string())?;
    let bytes = std::fs::read(&p).map_err(|e| format!("打不开 {}：{e}", p.display()))?;
    let entries = parse_playlist_entries(&decode_text(&bytes), fmt);
    let base = p.parent().map(Path::to_path_buf).unwrap_or_default();

    let cap = MAX_QUERY_LIMIT as usize;
    let mut items: Vec<MediaTrackDto> = Vec::new();
    let mut total: i64 = 0;
    let mut skipped: u64 = 0;
    for e in &entries {
        let full = resolve_playlist_entry(e, &base);
        if !full.is_file() {
            skipped += 1;
            continue;
        }
        match track_dto_for_path(conn, &full) {
            Some(d) => {
                total += 1;
                if items.len() < cap {
                    items.push(d);
                }
            }
            None => skipped += 1,
        }
    }
    if skipped > 0 {
        crate::log::info(&format!(
            "media: 播放列表 {} 里有 {skipped} 条找不到或不是音频，已跳过",
            p.display()
        ));
    }
    Ok(MediaTracksDto { total, items })
}

/// 查曲目：`root` 空 = 全库；`query` 空 = 不筛。
///
/// **总数单独查**（不受 limit 影响）：界面要靠它说「共 N 首，只列了前 M 首」，
/// 静默截断会让用户以为库里只有 2000 首。
fn query_tracks_on(
    conn: &Connection,
    root: &str,
    query: &str,
    limit: u32,
) -> Result<MediaTracksDto, String> {
    let mut where_sql = String::from("WHERE 1=1");
    let mut args: Vec<Value> = Vec::new();
    if !root.is_empty() {
        where_sql.push_str(" AND root = ?");
        args.push(Value::Text(root.to_string()));
    }
    if !query.trim().is_empty() {
        where_sql.push_str(
            " AND (title LIKE ? ESCAPE '\\' OR artist LIKE ? ESCAPE '\\' \
               OR album LIKE ? ESCAPE '\\' OR album_artist LIKE ? ESCAPE '\\')",
        );
        let pat = format!("%{}%", escape_like(query.trim()));
        for _ in 0..4 {
            args.push(Value::Text(pat.clone()));
        }
    }

    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM tracks {where_sql}"),
            params_from_iter(args.iter()),
            |r| r.get(0),
        )
        .map_err(|e| format!("查询曲目总数失败: {e}"))?;

    let cap = limit.clamp(1, MAX_QUERY_LIMIT);
    let sql = format!(
        "SELECT path, root, title, artist, album, album_artist, track_no, year, genre,
                duration_ms, ext, size, mtime, cover
         FROM tracks {where_sql}
         ORDER BY album, track_no, title COLLATE NOCASE
         LIMIT ?"
    );
    let mut all_args = args.clone();
    all_args.push(Value::Integer(cap as i64));

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params_from_iter(all_args.iter()), row_to_track)
        .map_err(|e| e.to_string())?;
    let items = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .iter()
        .map(to_dto)
        .collect();

    Ok(MediaTracksDto { total, items })
}

fn row_to_track(r: &rusqlite::Row<'_>) -> rusqlite::Result<TrackRow> {
    Ok(TrackRow {
        path: r.get(0)?,
        root: r.get(1)?,
        title: r.get(2)?,
        artist: r.get(3)?,
        album: r.get(4)?,
        album_artist: r.get(5)?,
        track_no: r.get(6)?,
        year: r.get(7)?,
        genre: r.get(8)?,
        duration_ms: r.get(9)?,
        ext: r.get(10)?,
        size: r.get(11)?,
        mtime: r.get(12)?,
        cover: r.get(13)?,
    })
}

// ── 标签解析（lofty）──────────────────────────────────────────────

struct ParsedTags {
    title: String,
    artist: String,
    album: String,
    album_artist: String,
    track_no: i64,
    year: i64,
    genre: String,
    duration_ms: i64,
    /// `(字节, 缓存文件扩展名)` —— 已经在 `save_cover` 里落盘了才给。
    cover: String,
}

/// 视频文件**没有可解析标签时**的兜底字段（2026-10-01 用户第 8 条）。
///
/// 为什么必须有：`lofty` 是**音频**标签库，对 `.mov`（以及部分 `.mp4`）会直接 `Err` ——
/// 不兜底的话用户的视频会**静默地不出现**在媒体库里（扫描时算 `failed` 跳过，
/// 界面上什么都没少也什么都没多，根本无从查起）。
///
/// 兜底出来的**只有文件名**：`title` = 主文件名，其余全空、**时长 0**。
/// 时长 0 正好复用现有口径 —— 界面不画进度条、不给拖（见 `player.rs` 的 `duration_ms == 0`），
/// 不画假的；真的时长要解码器才知道，而那是播放时的事。
fn video_fallback_tags(path: &Path) -> ParsedTags {
    ParsedTags {
        title: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        artist: String::new(),
        album: String::new(),
        album_artist: String::new(),
        track_no: 0,
        year: 0,
        genre: String::new(),
        duration_ms: 0,
        cover: String::new(),
    }
}

/// 读一首的标签。**这是「内容才是判据」那一步**：扩展名可能是假的，能不能解析出
/// 音频属性才是真的（`lofty` 认不出容器就会 `Err`，调用方记 `failed`）。
fn read_tags(path: &Path) -> Result<ParsedTags, String> {
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::probe::Probe;
    use lofty::tag::{Accessor, ItemKey};

    let tagged = Probe::open(path)
        .map_err(|e| format!("打不开: {e}"))?
        .guess_file_type()
        .map_err(|e| format!("认不出容器: {e}"))?
        .read()
        .map_err(|e| format!("解析失败: {e}"))?;

    let props = tagged.properties();
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let mut out = ParsedTags {
        title: String::new(),
        artist: String::new(),
        album: String::new(),
        album_artist: String::new(),
        track_no: 0,
        year: 0,
        genre: String::new(),
        duration_ms: props.duration().as_millis() as i64,
        cover: String::new(),
    };

    if let Some(t) = tag {
        out.title = t.title().map(|s| s.to_string()).unwrap_or_default();
        out.artist = t.artist().map(|s| s.to_string()).unwrap_or_default();
        out.album = t.album().map(|s| s.to_string()).unwrap_or_default();
        out.genre = t.genre().map(|s| s.to_string()).unwrap_or_default();
        out.track_no = t.track().unwrap_or(0) as i64;
        // 年份与专辑歌手都没有 `Accessor` 快捷方法（不是每种格式都有这两项）⇒ 走通用键。
        // `get_string` 收的是**值**不是引用（lofty 0.25 的签名），别顺手加 `&`。
        out.year = t
            .get_string(ItemKey::Year)
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0);
        out.album_artist = t
            .get_string(ItemKey::AlbumArtist)
            .map(|s| s.to_string())
            .unwrap_or_default();
        // 封面：只取第一张（多张内嵌图里第一张历来是正面图）。
        if let Some(pic) = t.pictures().first() {
            let data = pic.data();
            if !data.is_empty() && data.len() <= COVER_MAX_BYTES {
                let ext = pic
                    .mime_type()
                    .map(|m| mime_to_ext(&format!("{m}")))
                    .unwrap_or("jpg");
                out.cover = save_cover(data, ext).unwrap_or_default();
            }
        }
    }

    if out.title.trim().is_empty() {
        out.title = title_from_path(path);
    }
    if out.album_artist.trim().is_empty() {
        out.album_artist = out.artist.clone();
    }
    Ok(out)
}

/// 把封面写进 `<exe 根>\temp\music-covers\`，返回**文件名**（库里存文件名、不存全路径 ——
/// 全路径里带盘符与安装位置，换台机器就对不上了）。
///
/// 内容哈希做文件名 ⇒ 同一张专辑封面只落一份盘（几十首共用一张图是常态）。
fn save_cover(bytes: &[u8], ext: &str) -> Result<String, String> {
    let name = cover_file_name(bytes, ext);
    let dir = covers_dir();
    let full = dir.join(&name);
    if full.is_file() {
        return Ok(name);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建封面目录失败: {e}"))?;
    // 先写临时文件再改名：半截的图被 `<img>` 读到会「这张封面永远破图」。
    let tmp = dir.join(format!("{name}.part"));
    std::fs::write(&tmp, bytes).map_err(|e| format!("写封面失败: {e}"))?;
    std::fs::rename(&tmp, &full).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("封面改名失败: {e}")
    })?;
    Ok(name)
}

// ── 扫描 ──────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct ScanState {
    running: bool,
    scanned: u64,
    total: u64,
    added: u64,
    updated: u64,
    removed: u64,
    unchanged: u64,
    failed: u64,
    error: String,
    done_at: i64,
}

fn scan_state() -> &'static Mutex<ScanState> {
    static S: OnceLock<Mutex<ScanState>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(ScanState::default()))
}

fn status_dto() -> ScanStatusDto {
    match scan_state().lock() {
        Ok(s) => ScanStatusDto {
            running: s.running,
            scanned: s.scanned,
            total: s.total,
            added: s.added,
            updated: s.updated,
            removed: s.removed,
            unchanged: s.unchanged,
            failed: s.failed,
            error: s.error.clone(),
            done_at: s.done_at,
        },
        Err(_) => ScanStatusDto::default(),
    }
}

struct Candidate {
    path: PathBuf,
    size: i64,
    mtime: i64,
}

/// 递归收集候选文件。**不跟随符号链接**（`file_type()` 对 reparse point 报 symlink）
/// —— 这一条同时解决「链接环」与「一个链接指回上层目录导致重复扫」。
fn collect_candidates(dir: &Path, depth: usize, out: &mut Vec<Candidate>, failed: &mut u64) {
    if depth > MAX_DEPTH || out.len() >= MAX_CANDIDATES {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            // 权限不足 / 目录刚被删：**跳过但记账**，别让一个坏目录毁掉整次扫描。
            *failed += 1;
            return;
        }
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_CANDIDATES {
            return;
        }
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_symlink() {
            continue;
        }
        let path = entry.path();
        if ft.is_dir() {
            collect_candidates(&path, depth + 1, out, failed);
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        // ① 扩展名初筛（省掉对每个 .txt 调一次解析器）
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !is_indexable_ext(&ext) {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        out.push(Candidate {
            path,
            size: meta.len() as i64,
            mtime,
        });
    }
}

/// 扫一个根。**在后台线程里跑**（见 `media_scan_start`）。
fn scan_root(conn: &mut Connection, root: &str) -> Result<(u64, u64, u64, u64, u64), String> {
    let root_path = PathBuf::from(root);
    let mut failed_scan = 0u64;
    let mut cands: Vec<Candidate> = Vec::new();
    collect_candidates(&root_path, 0, &mut cands, &mut failed_scan);

    // 先数一遍（= 候选数）再解析：这样进度条是真进度，不是「转了多久」。
    if let Ok(mut s) = scan_state().lock() {
        s.total = cands.len() as u64;
        s.scanned = 0;
        s.failed = failed_scan;
    }

    let mut seen: HashSet<String> = HashSet::with_capacity(cands.len());
    let (mut added, mut updated, mut unchanged, mut failed) = (0u64, 0u64, 0u64, failed_scan);

    for c in &cands {
        let path_str = c.path.display().to_string();
        seen.insert(path_str.clone());

        if let Some((old_size, old_mtime)) = existing_stamp(conn, &path_str) {
            if is_unchanged(old_size, old_mtime, c.size, c.mtime) {
                unchanged += 1;
                if let Ok(mut s) = scan_state().lock() {
                    s.scanned += 1;
                    s.unchanged = unchanged;
                }
                continue;
            }
        }

        let existed = existing_stamp(conn, &path_str).is_some();
        let ext = c
            .path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        // 视频容器（`.mov`、部分 `.mp4`）`lofty` 常常直接 `Err` ⇒ 用「只有文件名」那条兜底。
        // **不兜底的话用户的视频会静默地不出现**（扫描时算 `failed` 跳过，界面上什么都没少、
        // 也什么都没多，根本无从查起）。判据卡得很死：只有扩展名在 `VIDEO_EXTS` 里才兜底。
        let tags = match read_tags(&c.path) {
            Ok(t) => Some(t),
            Err(_) if is_video_ext(&ext) => Some(video_fallback_tags(&c.path)),
            // 「扩展名像音频但解析不了」是**正常情况**（封面图 / 说明文件被改了扩展名都在
            // 这一类）⇒ 只计数，不刷日志（否则一个 5000 首的库能刷几千行日志）。
            Err(_) => {
                failed += 1;
                None
            }
        };
        if let Some(t) = tags {
            let row = TrackRow {
                path: path_str.clone(),
                root: root.to_string(),
                title: t.title,
                artist: t.artist,
                album: t.album,
                album_artist: t.album_artist,
                track_no: t.track_no,
                year: t.year,
                genre: t.genre,
                duration_ms: t.duration_ms,
                ext,
                size: c.size,
                mtime: c.mtime,
                cover: t.cover,
            };
            if let Err(e) = upsert_track(conn, &row) {
                crate::log::warn(&format!("media: 入库失败 {path_str}（{e}）"));
                failed += 1;
            } else if existed {
                updated += 1;
            } else {
                added += 1;
            }
        }

        if let Ok(mut s) = scan_state().lock() {
            s.scanned += 1;
            s.added = added;
            s.updated = updated;
            s.failed = failed;
        }
    }

    // 这一根下没见到的行 = 文件被删 / 改名 ⇒ 清掉（**只清这个根**，别的根不动）。
    let mut removed = 0u64;
    {
        let mut stmt = conn
            .prepare("SELECT path FROM tracks WHERE root = ?1")
            .map_err(|e| e.to_string())?;
        let olds = stmt
            .query_map(rusqlite::params![root], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        for p in olds {
            if !seen.contains(&p) {
                conn.execute("DELETE FROM tracks WHERE path = ?1", rusqlite::params![p])
                    .map_err(|e| e.to_string())?;
                removed += 1;
            }
        }
    }
    conn.execute(
        "UPDATE roots SET last_scan_at = ?1 WHERE path = ?2",
        rusqlite::params![now_ms(), root],
    )
    .map_err(|e| e.to_string())?;

    Ok((added, updated, unchanged, removed, failed))
}

/// 扫全部根（或指定的一个）。
fn scan_all(only: Option<String>) {
    let result = (|| -> Result<(u64, u64, u64, u64, u64), String> {
        let mut conn = open_db()?;
        let roots: Vec<String> = match &only {
            Some(r) => vec![r.clone()],
            None => {
                let mut stmt = conn
                    .prepare("SELECT path FROM roots ORDER BY added_at")
                    .map_err(|e| e.to_string())?;
                // 先把收集结果落成一个具名局部量：`query_map` 的返回值借用 `stmt`，
                // 直接当块尾表达式会被判「临时值先于 `stmt` 析构」（E0597）。
                let rows = stmt
                    .query_map([], |r| r.get::<_, String>(0))
                    .map_err(|e| e.to_string())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                rows
            }
        };
        let (mut a, mut u, mut un, mut rm, mut f) = (0u64, 0u64, 0u64, 0u64, 0u64);
        for r in &roots {
            let (ra, ru, run, rrm, rf) = scan_root(&mut conn, r)?;
            a += ra;
            u += ru;
            un += run;
            rm += rrm;
            f += rf;
        }
        Ok((a, u, un, rm, f))
    })();

    if let Ok(mut s) = scan_state().lock() {
        s.running = false;
        s.done_at = now_ms();
        match result {
            Ok(_) => s.error.clear(),
            Err(e) => {
                crate::log::warn(&format!("media: 扫描失败（{e}）"));
                s.error = e;
            }
        }
    }
}

// ── 命令 ──────────────────────────────────────────────────────────

/// 库里的目录列表（含每个目录的曲目数与「目录还在不在」）。
#[tauri::command]
pub async fn media_roots() -> Result<Vec<MediaRootDto>, String> {
    crate::commands::run_blocking(|| {
        let conn = open_db()?;
        list_roots_on(&conn)
    })
    .await
}

/// 添加一个媒体目录（**不立刻扫描** —— 扫描由用户点，或界面上那句「开始扫描」触发）。
///
/// 为什么要分开：一个大目录扫起来要几十秒，点「添加」就卡住是坏体验；
/// 而且用户常常一次加好几个目录，逐个扫一遍纯属浪费。
#[tauri::command]
pub async fn media_add_root(path: String) -> Result<Vec<MediaRootDto>, String> {
    crate::commands::run_blocking(move || {
        let norm = normalize_root(&path);
        if norm.is_empty() {
            return Err("目录为空".into());
        }
        let p = Path::new(&norm);
        if !p.is_absolute() {
            return Err("只接受绝对路径".into());
        }
        if !p.is_dir() {
            return Err(format!("目录不存在：{norm}"));
        }
        let conn = open_db()?;
        add_root_on(&conn, &norm)?;
        crate::log::info(&format!("media: 已添加媒体目录 {norm}"));
        list_roots_on(&conn)
    })
    .await
}

/// 移除一个媒体目录（**连同它的曲目一起**）。正在扫描时拒绝 —— 半路删根会让
/// 那次扫描的收尾把「已经删掉的行」再删一遍，还会把 `last_scan_at` 写回一个不存在的根。
#[tauri::command]
pub async fn media_remove_root(path: String) -> Result<Vec<MediaRootDto>, String> {
    crate::commands::run_blocking(move || {
        if scan_state().lock().map(|s| s.running).unwrap_or(false) {
            return Err("ERR_SCAN_RUNNING".into());
        }
        let norm = normalize_root(&path);
        let mut conn = open_db()?;
        let n = remove_root_on(&mut conn, &norm)?;
        crate::log::info(&format!("media: 已移除媒体目录 {norm}（同时删掉 {n} 行索引）"));
        list_roots_on(&conn)
    })
    .await
}

/// 开始扫描（后台线程，立刻返回）。`root` 空 = 扫全部。
#[tauri::command]
pub fn media_scan_start(root: Option<String>) -> Result<ScanStatusDto, String> {
    {
        let mut s = scan_state().lock().map_err(|e| e.to_string())?;
        if s.running {
            return Err("ERR_SCAN_RUNNING".into());
        }
        *s = ScanState {
            running: true,
            done_at: 0,
            ..Default::default()
        };
    }
    let only = root.map(|r| normalize_root(&r)).filter(|r| !r.is_empty());
    std::thread::spawn(move || scan_all(only));
    Ok(status_dto())
}

/// 扫描进度（界面每 500ms 轮询一次；不扫的时候也调它，拿到的是上一次的结果）。
#[tauri::command]
pub fn media_scan_status() -> ScanStatusDto {
    status_dto()
}

/// 查曲目。`root` 空 = 全库；`query` 走标题 / 歌手 / 专辑 / 专辑歌手的模糊匹配。
#[tauri::command]
pub async fn media_tracks(
    root: Option<String>,
    query: Option<String>,
    limit: Option<u32>,
) -> Result<MediaTracksDto, String> {
    crate::commands::run_blocking(move || {
        let conn = open_db()?;
        query_tracks_on(
            &conn,
            root.as_deref().unwrap_or(""),
            query.as_deref().unwrap_or(""),
            limit.unwrap_or(1000),
        )
    })
    .await
}

/// 打开一份播放列表文件（`.m3u` / `.m3u8` / `.pls`）→ 它的曲目。
///
/// **与媒体库是两条路**：这一条**只读那个文件**，不入库、不要求目录已添加
/// （播放列表常常指向别处的目录）。返回的形状复用 `MediaTracksDto`，
/// 于是界面那套渲染 / 点播一行都不用改。
#[tauri::command]
pub async fn media_playlist(path: String) -> Result<MediaTracksDto, String> {
    crate::commands::run_blocking(move || {
        let conn = open_db()?;
        read_playlist(&conn, &path)
    })
    .await
}

// ── 单测 ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        init_schema(&c).unwrap();
        c
    }

    fn track(path: &str, root: &str, title: &str, artist: &str, album: &str) -> TrackRow {
        TrackRow {
            path: path.into(),
            root: root.into(),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            album_artist: artist.into(),
            duration_ms: 1000,
            ext: "mp3".into(),
            size: 10,
            mtime: 1,
            ..Default::default()
        }
    }

    /// 扩展名初筛：认得那些容器、别把小写/大写搞混、**别把「音频」认成随便什么**。
    #[test]
    fn only_audio_like_extensions_get_probed() {
        for e in ["mp3", "MP3", ".Flac", "m4a", "opus", "wav"] {
            assert!(is_audio_ext(e), "{e} 应该算音频扩展名");
        }
        for e in ["txt", "jpg", "pdf", "lrc", "", "mp3x", "xmp3"] {
            assert!(!is_audio_ext(e), "{e} 不该被当成音频");
        }
    }

    /// 视频扩展名（2026-10-01 用户第 8 条）：**它有自己的一张表**，`mp4` 在两张表里都在。
    #[test]
    fn video_extensions_are_their_own_table() {
        for e in ["mp4", ".MP4", "m4v", "mov"] {
            assert!(is_video_ext(e), "{e} 应该算视频");
            assert!(is_indexable_ext(e), "{e} 应该能进媒体库");
        }
        // **音频不等于视频** —— 这是「画面层该不该开」的判据，认错了就是「放 mp3 也弹一块黑屏」
        for e in ["mp3", "flac", "wav", "m4a", "opus"] {
            assert!(!is_video_ext(e), "{e} 不是视频");
        }
        // 刻意不收的三种（Chromium 不出画面 / 音轨是 Opus）：连库都不该进
        for e in ["mkv", "avi", "webm", "wmv", "flv"] {
            assert!(!is_video_ext(e), "{e} 刻意不收，理由见 VIDEO_EXTS 的注释");
            assert!(!is_indexable_ext(e), "{e} 不该进媒体库");
        }
    }

    /// 视频兜底：**只留文件名**，其余全空 —— 尤其 `duration_ms = 0`
    /// （界面据此不画进度条、不给拖，不画假的）。
    #[test]
    fn video_fallback_only_keeps_the_file_name() {
        let t = video_fallback_tags(Path::new(r"D:\Movies\我的假期.MOV"));
        assert_eq!(t.title, "我的假期");
        assert_eq!(t.duration_ms, 0);
        assert!(t.artist.is_empty() && t.album.is_empty() && t.cover.is_empty());
    }

    /// DTO 的 `video` 字段：`to_dto` **从 `ext` 现算**（不落库 ⇒ 不会有第二份真相）。
    #[test]
    fn the_dto_reports_video_from_the_extension_alone() {
        let mut row = track(r"D:\Movies\a.mp4", r"D:\Movies", "a", "", "");
        row.ext = "mp4".into();
        assert!(to_dto(&row).video);
        row.ext = "MP4".into();
        assert!(to_dto(&row).video, "扩展名的大小写不该影响判据");
        row.ext = "mp3".into();
        assert!(!to_dto(&row).video);
    }

    /// 根目录规范化：同一个目录只允许有一种写法（主键就是它）。
    #[test]
    fn root_path_is_normalized_to_one_form() {
        assert_eq!(normalize_root(r"D:\Music\"), r"D:\Music");
        assert_eq!(normalize_root("  D:\\Music\\\\  "), r"D:\Music");
        assert_eq!(normalize_root("D:/Music/"), "D:/Music");
        // 盘符根不能把尾分隔符去掉（`D:` 在 Windows 上是「D 盘的当前目录」，另一回事）
        assert_eq!(normalize_root("D:\\"), "D:\\");
        assert_eq!(normalize_root("D:"), "D:\\");
    }

    /// 增量判据：两个都没变才算没变；**旧的 mtime 为 0 一律当「要重扫」**
    /// （0 = 这条是历史脏数据，宁可多解析一次）。
    #[test]
    fn incremental_needs_both_size_and_mtime() {
        assert!(is_unchanged(10, 100, 10, 100));
        assert!(!is_unchanged(10, 100, 11, 100), "长度变了要重扫");
        assert!(!is_unchanged(10, 100, 10, 101), "时间变了要重扫");
        assert!(!is_unchanged(0, 0, 10, 100), "旧记录没有时间戳 ⇒ 必须重扫");
    }

    /// 封面文件名：**内容相同就同名**（同专辑几十首只落一份盘），内容不同就不同名。
    #[test]
    fn cover_name_is_content_addressed() {
        let a = cover_file_name(b"cover-bytes-1", "jpg");
        let b = cover_file_name(b"cover-bytes-1", "jpg");
        let c = cover_file_name(b"cover-bytes-2", "jpg");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.ends_with(".jpg") && a.len() == 16 + 4);
        assert_eq!(cover_file_name(b"x", "png").rsplit('.').next(), Some("png"));
    }

    #[test]
    fn cover_mime_maps_to_a_browser_friendly_extension() {
        assert_eq!(mime_to_ext("image/png"), "png");
        assert_eq!(mime_to_ext("PNG"), "png");
        assert_eq!(mime_to_ext("image/jpeg"), "jpg");
        assert_eq!(mime_to_ext("application/octet-stream"), "jpg");
    }

    /// `LIKE` 转义：用户搜 `%` 不该命中整库、搜 `_` 不该当通配符。
    #[test]
    fn like_wildcards_are_escaped() {
        assert_eq!(escape_like("100%"), "100\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
        assert_eq!(escape_like("c:\\x"), "c:\\\\x");
        assert_eq!(escape_like("普通歌名"), "普通歌名");
    }

    /// 根目录的增删：**删根要连曲目一起删**（留一半就是幽灵条目）。
    #[test]
    fn removing_a_root_takes_its_tracks_with_it() {
        let mut c = mem();
        add_root_on(&c, r"D:\Music").unwrap();
        add_root_on(&c, r"E:\Songs").unwrap();
        add_root_on(&c, r"D:\Music").unwrap(); // 重复添加不报错、也不产生第二条
        upsert_track(&c, &track(r"D:\Music\a.mp3", r"D:\Music", "A", "X", "Al")).unwrap();
        upsert_track(&c, &track(r"E:\Songs\b.mp3", r"E:\Songs", "B", "Y", "Bl")).unwrap();

        let roots = list_roots_on(&c).unwrap();
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].tracks, 1);

        remove_root_on(&mut c, r"D:\Music").unwrap();
        let roots = list_roots_on(&c).unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].path, r"E:\Songs");
        let left = query_tracks_on(&c, "", "", 100).unwrap();
        assert_eq!(left.total, 1, "只该剩下另一个根的那首");
    }

    /// 查询：按根筛 / 按词筛 / `total` 不被 limit 截断（界面要靠它说「共 N 首」）。
    #[test]
    fn query_filters_and_reports_the_real_total() {
        let c = mem();
        add_root_on(&c, r"D:\Music").unwrap();
        for (p, t, ar, al) in [
            (r"D:\Music\1.mp3", "晴天", "周杰伦", "叶惠美"),
            (r"D:\Music\2.mp3", "夜曲", "周杰伦", "十一月的萧邦"),
            (r"D:\Music\3.mp3", "Bohemian Rhapsody", "Queen", "A Night at the Opera"),
        ] {
            upsert_track(&c, &track(p, r"D:\Music", t, ar, al)).unwrap();
        }

        assert_eq!(query_tracks_on(&c, "", "", 100).unwrap().total, 3);
        assert_eq!(query_tracks_on(&c, r"D:\Music", "", 100).unwrap().total, 3);
        assert_eq!(query_tracks_on(&c, r"E:\None", "", 100).unwrap().total, 0);

        let by_artist = query_tracks_on(&c, "", "周杰伦", 100).unwrap();
        assert_eq!(by_artist.total, 2);
        let by_album = query_tracks_on(&c, "", "Opera", 100).unwrap();
        assert_eq!(by_album.total, 1);
        assert_eq!(by_album.items[0].title, "Bohemian Rhapsody");

        // limit 只截 items，不动 total
        let capped = query_tracks_on(&c, "", "", 2).unwrap();
        assert_eq!(capped.total, 3);
        assert_eq!(capped.items.len(), 2);
        // limit=0 也要给一条（clamp 到 1），不能返回空
        assert_eq!(query_tracks_on(&c, "", "", 0).unwrap().items.len(), 1);
    }

    /// upsert 的语义：同一条路径再来一次是**更新**，不是插第二条。
    #[test]
    fn upsert_is_keyed_by_path() {
        let c = mem();
        add_root_on(&c, r"D:\Music").unwrap();
        let mut t = track(r"D:\Music\a.mp3", r"D:\Music", "旧标题", "X", "Al");
        upsert_track(&c, &t).unwrap();
        t.title = "新标题".into();
        upsert_track(&c, &t).unwrap();
        let r = query_tracks_on(&c, "", "", 10).unwrap();
        assert_eq!(r.total, 1);
        assert_eq!(r.items[0].title, "新标题");
        assert_eq!(existing_stamp(&c, r"D:\Music\a.mp3"), Some((10, 1)));
        assert_eq!(existing_stamp(&c, r"D:\Music\none.mp3"), None);
    }

    /// 没有标签时标题兜底用文件名 —— 但**绝不返回空标题**（界面上会变成一行空白）。
    #[test]
    fn title_falls_back_to_the_file_stem() {
        assert_eq!(title_from_path(Path::new(r"D:\M\Artist - Song.mp3")), "Artist - Song");
        assert_eq!(title_from_path(Path::new(r"D:\M\无扩展名")), "无扩展名");
        assert_eq!(title_from_path(Path::new("song.flac")), "song");
    }

    /// 扫描状态默认是「没在跑、没结果」—— 界面首次打开时看到的就是它。
    #[test]
    fn scan_status_starts_idle() {
        let s = status_dto();
        assert!(!s.running);
        assert_eq!(s.total, 0);
        assert!(s.error.is_empty());
    }

    /// 候选收集：不认的扩展名不进去、目录能递归进去、`MAX_DEPTH` 之外不再往下。
    #[test]
    fn candidates_only_include_probed_extensions() {
        let base = std::env::temp_dir().join("lunac-media-scan-test");
        let sub = base.join("album");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(base.join("a.mp3"), b"x").unwrap();
        std::fs::write(base.join("cover.jpg"), b"x").unwrap();
        std::fs::write(base.join("notes.txt"), b"x").unwrap();
        std::fs::write(sub.join("b.flac"), b"x").unwrap();

        let mut out = Vec::new();
        let mut failed = 0u64;
        collect_candidates(&base, 0, &mut out, &mut failed);
        let names: Vec<String> = out
            .iter()
            .map(|c| c.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"a.mp3".to_string()));
        assert!(names.contains(&"b.flac".to_string()), "子目录要递归进去");
        assert!(!names.contains(&"cover.jpg".to_string()));
        assert!(!names.contains(&"notes.txt".to_string()));
        assert_eq!(out.len(), 2);

        let _ = std::fs::remove_dir_all(&base);
    }

    // ── 播放列表解析（2026-10-01 第二批）──────────────────────────

    /// 后缀只认那三种（大小写都算），别的一个不接。
    #[test]
    fn only_the_three_playlist_extensions_are_recognised() {
        assert_eq!(playlist_format(Path::new(r"D:\a.m3u")), Some(PlaylistFormat::M3u));
        assert_eq!(playlist_format(Path::new(r"D:\a.M3U8")), Some(PlaylistFormat::M3u));
        assert_eq!(playlist_format(Path::new(r"D:\a.pls")), Some(PlaylistFormat::Pls));
        assert_eq!(playlist_format(Path::new(r"D:\a.mp3")), None);
        assert_eq!(playlist_format(Path::new(r"D:\noext")), None);
    }

    /// m3u：`#` 指令行与空行都不是路径；**顺序即文件里的顺序**（播放列表的顺序是内容）。
    #[test]
    fn m3u_keeps_only_real_entries_in_order() {
        let text = "#EXTM3U\n\
                    #EXTINF:260,周杰伦 - 晴天\n\
                    D:\\Music\\晴天.mp3\n\
                    \n\
                    sub\\夜曲.flac\n\
                    #EXTINF:200,夜曲\n\
                    D:/Music/夜曲.flac\n";
        let got = parse_m3u(text);
        assert_eq!(
            got,
            vec![r"D:\Music\晴天.mp3", r"sub\夜曲.flac", "D:/Music/夜曲.flac"]
        );
    }

    /// pls：只认 `FileN=`，**按 N 排序**（行序不保证）；`filename=` 这类键要挡住。
    #[test]
    fn pls_reads_filen_and_sorts_by_index() {
        let text = "[playlist]\n\
                    NumberOfEntries=2\n\
                    File2=D:\\Music\\b.mp3\n\
                    Title2=b\n\
                    File1=D:\\Music\\a.mp3\n\
                    filename=不是一首歌\n\
                    FileX=D:\\Music\\c.mp3\n\
                    File3=\n";
        let got = parse_pls(text);
        assert_eq!(got, vec![r"D:\Music\a.mp3", r"D:\Music\b.mp3"]);
    }

    /// 条目 → 绝对路径：绝对路径原样、相对路径拼到播放列表所在目录、`file://` 要剥掉。
    #[test]
    fn entries_resolve_against_the_playlist_directory() {
        let base = Path::new(r"D:\Lists");
        assert_eq!(resolve_playlist_entry(r"D:\Music\a.mp3", base), PathBuf::from(r"D:\Music\a.mp3"));
        assert_eq!(resolve_playlist_entry("sub/a.mp3", base), PathBuf::from(r"D:\Lists\sub\a.mp3"));
        assert_eq!(
            resolve_playlist_entry("file://D:/Music/a.mp3", base),
            PathBuf::from("D:/Music/a.mp3")
        );
    }

    /// 三种编码都要认（`.m3u8` 是 UTF-8、记事本「Unicode」是 UTF-16、老播放器是 GBK）。
    /// **GBK 这条是重点**：不认它的话中文路径会变乱码，界面上一首都不显示。
    #[test]
    fn playlist_text_decodes_utf8_utf16_and_gbk() {
        // UTF-8（带 BOM）
        let mut utf8_bom = vec![0xEF, 0xBB, 0xBF];
        utf8_bom.extend_from_slice("D:\\音乐\\晴天.mp3".as_bytes());
        assert_eq!(decode_text(&utf8_bom), r"D:\音乐\晴天.mp3");

        // UTF-16LE（带 BOM）—— 记事本「另存为 Unicode」就是它
        let mut utf16 = vec![0xFF, 0xFE];
        for u in "D:\\音乐\\晴天.mp3".encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_text(&utf16), r"D:\音乐\晴天.mp3");

        // GBK（无 BOM）—— 老播放器导出的 `.m3u`
        let (gbk, _, _) = encoding_rs::GBK.encode("D:\\音乐\\晴天.mp3");
        assert_eq!(decode_text(&gbk), r"D:\音乐\晴天.mp3");

        // 纯 ASCII 的 UTF-8 走的是同一条「先严格解」的路
        assert_eq!(decode_text(b"D:\\Music\\a.mp3"), r"D:\Music\a.mp3");
    }

    /// 端到端（真读一份临时文件）：UTF-8 的 m3u + 一首真 WAV ⇒ 至少解出一首。
    #[test]
    fn a_playlist_reads_its_existing_tracks() {
        let base = std::env::temp_dir().join("lunac-media-playlist-test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // 一份能解出时长/采样率的最小 WAV（与 player.rs 那条用的是同一个写法）
        let wav = base.join("a.wav");
        let samples: [i16; 8] = [0, 3000, 6000, 3000, 0, -3000, -6000, -3000];
        let data_len = (samples.len() * 2) as u32;
        let mut b = Vec::new();
        b.extend(b"RIFF");
        b.extend((36 + data_len).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(8000u32.to_le_bytes());
        b.extend((8000u32 * 2).to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(b"data");
        b.extend(data_len.to_le_bytes());
        for s in samples {
            b.extend(s.to_le_bytes());
        }
        std::fs::write(&wav, b).unwrap();
        // 相对路径 + 一条指向不存在的文件（必须被跳过，而不是让整份打不开）
        let list = base.join("test.m3u8");
        std::fs::write(&list, "#EXTM3U\na.wav\ngone.mp3\n").unwrap();

        // 用内存库：这条测试不该去碰真机上的 media.db（`read_playlist` 要查 tracks 表）
        let c = mem();
        let dto = read_playlist(&c, &list.display().to_string()).unwrap();
        assert_eq!(dto.total, 1, "坏的那一条要被跳过");
        assert_eq!(dto.items.len(), 1);
        assert!(dto.items[0].path.ends_with("a.wav"), "{}", dto.items[0].path);
        assert_eq!(dto.items[0].title, "a", "没有标签 ⇒ 用文件名兜底");

        // 不是播放列表的文件要明确报错（界面据此说人话），而不是给一份空列表
        assert_eq!(
            read_playlist(&c, &wav.display().to_string()).unwrap_err(),
            "ERR_NOT_PLAYLIST"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
