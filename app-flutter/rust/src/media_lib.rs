//! 本地媒体库的 **v1 形态**（移植 `app/src-tauri/src/media_lib.rs` 的数据层）：
//! SQLite（`ModuleData/music/media.db`）+ **后台扫描线程** + 条件查询 + 删曲目。
//!
//! 为什么需要这一层（`music.rs` 已经有了同步全量扫描）：
//!   · v1 的曲库是**有索引**的 —— 打开面板只查库，不重新解析几千个文件的标签；
//!   · 扫描是**后台线程**：界面能拿 `scanned / total`，进度条是真的；
//!   · **重扫很快**：`size + mtime` 都没变的行直接跳过（`unchanged`）；
//!   · 删一首歌要**连索引与封面缓存一起删**（v1 `media_delete_file` 的安全边界照搬）。
//!
//! 与 v1 的关系：
//!   · 库文件路径、`roots` / `tracks` 两张表的结构**与 v1 逐列一致**（换回 v1 不丢索引）；
//!   · `tracks.cover` 存**文件名**（v1 同口径），对外的 JSON 再展开成绝对路径；
//!   · 返回 JSON 的键用 camelCase（与 v2 其它模块一致）。
//!
//! 纪律：扫描失败只把原因写进 `scan_status.error`（不 panic、不吞掉已经入库的行）；
//! 删文件只认**库里已有的路径**（挡住「拿任意路径删别人文件」）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::music;

/// 目录遍历深度上限（防「指向 C:\ 的根」把整台机器走穿；与 `music.rs` 同口径）。
const MAX_DEPTH: usize = 16;

// ── 扫描状态（轮询用）──────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
struct ScanState {
    running: bool,
    scanned: u64,
    /// 候选文件总数（先数一遍再解析，这样进度条是真的）。0 = 还没数完。
    total: u64,
    added: u64,
    updated: u64,
    removed: u64,
    /// 本次**没重新解析**的（size + mtime 都没变）—— 它才是「重扫很快」的来源。
    unchanged: u64,
    /// 扩展名像音频、但标签解析失败（坏文件 / 假扩展名）。
    failed: u64,
    error: String,
    done_at: i64,
    current: String,
}

impl ScanState {
    const fn new() -> Self {
        Self {
            running: false,
            scanned: 0,
            total: 0,
            added: 0,
            updated: 0,
            removed: 0,
            unchanged: 0,
            failed: 0,
            error: String::new(),
            done_at: 0,
            current: String::new(),
        }
    }
}

static SCAN: Mutex<ScanState> = Mutex::new(ScanState::new());

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn cover_dir(exe_root: &Path) -> PathBuf {
    exe_root.join("temp").join("music-covers")
}

// ── 库结构 ────────────────────────────────────────────────────────

/// 打开库并补齐 `tracks` 表（`roots` 表由 `music::open_db_rw` 建）。
fn open_db(exe_root: &Path) -> Result<rusqlite::Connection, String> {
    let conn = music::open_db_rw(exe_root)?;
    // 与 v1 同一口径：WAL + 等锁 —— 扫描写、界面读是并发的。
    let _ = conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=3000;",
    );
    init_schema(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
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
    .map_err(|e| format!("建媒体库表失败：{e}"))
}

// ── 根目录 ────────────────────────────────────────────────────────

fn list_roots(conn: &rusqlite::Connection) -> Result<Vec<serde_json::Value>, String> {
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
            let name = Path::new(&path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            Ok(serde_json::json!({
                "path": path,
                "name": name,
                "tracks": row.get::<_, i64>(2)?,
                "lastScanAt": row.get::<_, i64>(1)?,
                // 被删 / 拔盘的根要能看出来，别让用户对着 0 首发愣。
                "missing": !Path::new(&path).is_dir(),
            }))
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

fn roots_payload(conn: &rusqlite::Connection) -> String {
    match list_roots(conn) {
        Ok(roots) => serde_json::json!({ "ok": true, "error": "", "roots": roots }).to_string(),
        Err(e) => serde_json::json!({ "ok": false, "error": e, "roots": [] }).to_string(),
    }
}

/// 库里的目录列表（含每个目录的曲目数与「目录还在不在」）。
pub fn roots_json(exe_root: &Path) -> String {
    match open_db(exe_root) {
        Ok(conn) => roots_payload(&conn),
        Err(e) => serde_json::json!({ "ok": false, "error": e, "roots": [] }).to_string(),
    }
}

/// 添加一个媒体目录（**不立刻扫描** —— 扫描由用户点，与 v1 同口径）。
pub fn add_root_json(exe_root: &Path, input: &str) -> String {
    let path = music::normalize_root(input);
    if path.is_empty() {
        return serde_json::json!({ "ok": false, "error": "目录为空", "roots": [] }).to_string();
    }
    let target = Path::new(&path);
    if !target.is_absolute() {
        return serde_json::json!({ "ok": false, "error": "只接受绝对路径", "roots": [] })
            .to_string();
    }
    if !target.is_dir() {
        return serde_json::json!({
            "ok": false, "error": format!("目录不存在：{path}"), "roots": [],
        })
        .to_string();
    }
    let conn = match open_db(exe_root) {
        Ok(conn) => conn,
        Err(e) => return serde_json::json!({ "ok": false, "error": e, "roots": [] }).to_string(),
    };
    if let Err(e) = conn.execute(
        "INSERT OR IGNORE INTO roots (path, added_at) VALUES (?1, ?2)",
        rusqlite::params![path, now_ms()],
    ) {
        return serde_json::json!({
            "ok": false, "error": format!("写入曲库失败：{e}"), "roots": [],
        })
        .to_string();
    }
    crate::log::info(&format!("media: 已添加媒体目录 {path}"));
    roots_payload(&conn)
}

/// 移除一个媒体目录（**连同它的曲目一起**）；扫描进行中拒绝。
pub fn remove_root_json(exe_root: &Path, input: &str) -> String {
    if SCAN.lock().map(|s| s.running).unwrap_or(false) {
        return serde_json::json!({ "ok": false, "error": "ERR_SCAN_RUNNING", "roots": [] })
            .to_string();
    }
    let path = music::normalize_root(input);
    let mut conn = match open_db(exe_root) {
        Ok(conn) => conn,
        Err(e) => return serde_json::json!({ "ok": false, "error": e, "roots": [] }).to_string(),
    };
    let result = (|| -> Result<(), String> {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM tracks WHERE root = ?1", rusqlite::params![path])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM roots WHERE path = ?1", rusqlite::params![path])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    })();
    match result {
        Ok(_) => {
            crate::log::info(&format!("media: 已移除媒体目录 {path}"));
            roots_payload(&conn)
        }
        Err(e) => serde_json::json!({ "ok": false, "error": e, "roots": [] }).to_string(),
    }
}

// ── 扫描（后台线程）──────────────────────────────────────────────

fn status_json_value(state: &ScanState) -> serde_json::Value {
    serde_json::json!({
        "running": state.running,
        "scanned": state.scanned,
        "total": state.total,
        "added": state.added,
        "updated": state.updated,
        "removed": state.removed,
        "unchanged": state.unchanged,
        "failed": state.failed,
        "error": state.error,
        "doneAt": state.done_at,
        "current": state.current,
    })
}

/// 扫描进度（界面每 500ms 轮询一次；不扫时拿到的是上一次的结果）。
pub fn scan_status_json() -> String {
    let state = SCAN
        .lock()
        .map(|s| s.clone())
        .unwrap_or_else(|e| e.into_inner().clone());
    status_json_value(&state).to_string()
}

/// 开始扫描（后台线程，立刻返回）。`only` 空 = 扫全部。
pub fn scan_start_json(exe_root: &Path, only: &str) -> String {
    {
        let mut state = SCAN.lock().unwrap_or_else(|e| e.into_inner());
        if state.running {
            return serde_json::json!({
                "ok": false, "error": "ERR_SCAN_RUNNING", "status": status_json_value(&state),
            })
            .to_string();
        }
        *state = ScanState {
            running: true,
            ..ScanState::new()
        };
    }
    let root = exe_root.to_path_buf();
    let only = {
        let trimmed = music::normalize_root(only);
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    };
    std::thread::spawn(move || {
        let outcome = scan_all(&root, only);
        let mut state = SCAN.lock().unwrap_or_else(|e| e.into_inner());
        state.running = false;
        state.done_at = now_ms();
        state.current.clear();
        if let Err(e) = outcome {
            state.error = e;
        }
    });
    let state = SCAN
        .lock()
        .map(|s| s.clone())
        .unwrap_or_else(|e| e.into_inner().clone());
    serde_json::json!({ "ok": true, "error": "", "status": status_json_value(&state) })
        .to_string()
}

/// 一个候选文件（先数一遍再解析 ⇒ 进度条的分母是真的）。
struct Candidate {
    path: PathBuf,
    size: i64,
    mtime: i64,
}

fn mtime_of(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn collect_candidates(dir: &Path, depth: usize, out: &mut Vec<Candidate>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // 不跟符号链接：可能指回上层形成环。
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_candidates(&path, depth + 1, out);
            continue;
        }
        if !music::is_audio_path(&path) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        out.push(Candidate {
            path,
            size: meta.len() as i64,
            mtime: mtime_of(&meta),
        });
    }
}

fn bump<F: FnOnce(&mut ScanState)>(change: F) {
    let mut state = SCAN.lock().unwrap_or_else(|e| e.into_inner());
    change(&mut state);
}

fn scan_all(exe_root: &Path, only: Option<String>) -> Result<(), String> {
    let conn = open_db(exe_root)?;
    let roots: Vec<String> = match &only {
        Some(path) => vec![path.clone()],
        None => {
            let mut stmt = conn
                .prepare("SELECT path FROM roots ORDER BY added_at")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        }
    };
    if roots.is_empty() {
        return Err("曲库还没有添加任何目录".to_string());
    }

    let covers = cover_dir(exe_root);
    let mut candidates: Vec<Candidate> = Vec::new();
    for root in &roots {
        collect_candidates(Path::new(root), 0, &mut candidates);
    }
    let total = candidates.len() as u64;
    bump(|state| state.total = total);

    let mut seen: Vec<String> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let path_text = candidate.path.display().to_string();
        bump(|state| {
            state.scanned += 1;
            state.current = path_text.clone();
        });
        seen.push(path_text.clone());

        // 库里那行还在、size + mtime 也没变 ⇒ 不重解析（「重扫很快」的来源）。
        let existing: Option<(i64, i64)> = conn
            .query_row(
                "SELECT size, mtime FROM tracks WHERE path = ?1",
                rusqlite::params![&path_text],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        if let Some((size, mtime)) = existing {
            if size == candidate.size && mtime == candidate.mtime {
                bump(|state| state.unchanged += 1);
                continue;
            }
        }

        let track = music::read_track(&candidate.path, &covers);
        let ext = candidate
            .path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        // 库里存**文件名**（v1 同口径）；对外的 JSON 再展开成绝对路径。
        let cover_name = Path::new(&track.cover)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        // 读标签失败不算错：`read_track` 已经退回「文件名 + 空标签」。
        // 真正解析不了的文件（扩展名像音频、读不出任何信息）照样入库 ——
        // 用户至少能在列表里看到它，而不是「明明在盘上却怎么都扫不出来」。
        let inserted = conn.execute(
            "INSERT INTO tracks
               (path, root, title, artist, album, album_artist, track_no, year, genre,
                duration_ms, ext, size, mtime, cover)
             VALUES (?1, ?2, ?3, ?4, ?5, '', 0, 0, '', ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(path) DO UPDATE SET
               root = excluded.root,
               title = excluded.title,
               artist = excluded.artist,
               album = excluded.album,
               duration_ms = excluded.duration_ms,
               ext = excluded.ext,
               size = excluded.size,
               mtime = excluded.mtime,
               cover = excluded.cover",
            rusqlite::params![
                &path_text,
                &root_of(&roots, &candidate.path),
                &track.title,
                &track.artist,
                &track.album,
                track.duration_ms,
                &ext,
                candidate.size,
                candidate.mtime,
                &cover_name,
            ],
        );
        match inserted {
            Ok(_) => bump(|state| {
                if existing.is_some() {
                    state.updated += 1;
                } else {
                    state.added += 1;
                }
            }),
            Err(e) => {
                crate::log::warn(&format!("media: 入库失败 {path_text}: {e}"));
                bump(|state| state.failed += 1);
            }
        }
    }

    // 收尾：这些根下**盘上已经没有了**的行要删掉（否则永远是一条点不开的幽灵条目）。
    let seen_set: std::collections::HashSet<&str> =
        seen.iter().map(|s| s.as_str()).collect();
    for root in &roots {
        let mut stmt = conn
            .prepare("SELECT path, cover FROM tracks WHERE root = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params![root], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        let mut gone: Vec<(String, String)> = Vec::new();
        for row in rows.flatten() {
            if !seen_set.contains(row.0.as_str()) && !Path::new(&row.0).is_file() {
                gone.push(row);
            }
        }
        drop(stmt);
        for (path, cover) in gone {
            let _ = conn.execute(
                "DELETE FROM tracks WHERE path = ?1",
                rusqlite::params![&path],
            );
            if !cover.is_empty() {
                let _ = std::fs::remove_file(covers.join(&cover));
            }
            bump(|state| state.removed += 1);
        }
    }

    for root in &roots {
        let _ = conn.execute(
            "UPDATE roots SET last_scan_at = ?2 WHERE path = ?1",
            rusqlite::params![root, now_ms()],
        );
    }
    crate::log::info(&format!(
        "media: 扫描完成（{} 个根，{} 个候选）",
        roots.len(),
        total
    ));
    Ok(())
}

/// 这个文件属于哪个根（长根优先：`D:\Music` 与 `D:\Music\Live` 同时存在时取更具体的）。
fn root_of(roots: &[String], path: &Path) -> String {
    let text = path.to_string_lossy().to_lowercase();
    let mut best: Option<&String> = None;
    for root in roots {
        let key = root.to_lowercase();
        if text.starts_with(&key) {
            match best {
                Some(current) if current.len() >= root.len() => {}
                _ => best = Some(root),
            }
        }
    }
    best.cloned()
        .unwrap_or_else(|| roots.first().cloned().unwrap_or_default())
}

// ── 查曲目 / 删曲目 ──────────────────────────────────────────────

/// 查曲目：`root` 空 = 全库；`query` 走标题 / 歌手 / 专辑 / 专辑歌手的模糊匹配。
///
/// 返回 `{ok,error,total,items[]}`；`total` 是**命中总数**（不受 `limit` 影响，
/// 被截断时界面要如实说明）。
pub fn tracks_json(exe_root: &Path, root: &str, query: &str, limit: u32) -> String {
    let conn = match open_db(exe_root) {
        Ok(conn) => conn,
        Err(e) => {
            return serde_json::json!({ "ok": false, "error": e, "total": 0, "items": [] })
                .to_string()
        }
    };
    let root = music::normalize_root(root);
    let pattern = format!("%{}%", query.trim());
    let empty_query = query.trim().is_empty();
    let where_sql = "WHERE (?1 = '' OR root = ?1)
                       AND (?2 = 1 OR title LIKE ?3 OR artist LIKE ?3
                            OR album LIKE ?3 OR album_artist LIKE ?3)";
    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM tracks {where_sql}"),
            rusqlite::params![&root, empty_query, &pattern],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let covers = cover_dir(exe_root);
    let mut stmt = match conn.prepare(&format!(
        "SELECT path, title, artist, album, duration_ms, ext, size, cover
           FROM tracks {where_sql}
          ORDER BY album COLLATE NOCASE, title COLLATE NOCASE
          LIMIT ?4"
    )) {
        Ok(stmt) => stmt,
        Err(e) => {
            return serde_json::json!({ "ok": false, "error": e.to_string(), "total": 0, "items": [] })
                .to_string()
        }
    };
    let rows = stmt.query_map(
        rusqlite::params![&root, empty_query, &pattern, limit as i64],
        |row| {
            let cover_name: String = row.get(7)?;
            let ext: String = row.get(5)?;
            let cover = if cover_name.is_empty() {
                String::new()
            } else {
                covers.join(&cover_name).display().to_string()
            };
            Ok(serde_json::json!({
                "path": row.get::<_, String>(0)?,
                "title": row.get::<_, String>(1)?,
                "artist": row.get::<_, String>(2)?,
                "album": row.get::<_, String>(3)?,
                "durationMs": row.get::<_, i64>(4)?,
                "ext": ext,
                "size": row.get::<_, i64>(6)?,
                "cover": cover,
                // 要出画面的视频：与转换面板**同一份**扩展名单。
                "video": crate::convert::is_video_ext(&ext),
            }))
        },
    );
    let items: Vec<serde_json::Value> = match rows {
        Ok(rows) => rows.flatten().collect(),
        Err(e) => {
            return serde_json::json!({ "ok": false, "error": e.to_string(), "total": 0, "items": [] })
                .to_string()
        }
    };
    serde_json::json!({ "ok": true, "error": "", "total": total, "items": items }).to_string()
}

/// 从库里删除**单个文件**（同时删磁盘文件、库中那一行、以及它的封面缓存）。
///
/// 安全边界（缺一就有「拿任意路径删别人文件」的口子）：
///   ① **只认库里已有的曲目路径**（查不到直接报错、不删任何东西）；
///   ② **只删文件**，绝不删目录；
///   ③ 目标文件已不存在时**仍然**清索引与封面（按「已清理」处理，不报错）。
pub fn delete_file_json(exe_root: &Path, path: &str) -> String {
    if SCAN.lock().map(|s| s.running).unwrap_or(false) {
        return serde_json::json!({ "ok": false, "error": "ERR_SCAN_RUNNING" }).to_string();
    }
    let conn = match open_db(exe_root) {
        Ok(conn) => conn,
        Err(e) => return serde_json::json!({ "ok": false, "error": e }).to_string(),
    };
    let cover: String = match conn.query_row(
        "SELECT cover FROM tracks WHERE path = ?1",
        rusqlite::params![path],
        |row| row.get(0),
    ) {
        Ok(cover) => cover,
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            return serde_json::json!({
                "ok": false, "error": format!("曲目不在库中：{path}"),
            })
            .to_string()
        }
        Err(e) => return serde_json::json!({ "ok": false, "error": e.to_string() }).to_string(),
    };
    // 先删库行：界面立刻不再列出这首。
    if let Err(e) = conn.execute("DELETE FROM tracks WHERE path = ?1", rusqlite::params![path]) {
        return serde_json::json!({ "ok": false, "error": format!("删除索引失败：{e}") })
            .to_string();
    }
    let target = Path::new(path);
    if target.is_file() {
        if let Err(e) = std::fs::remove_file(target) {
            crate::log::warn(&format!("media: 删除文件失败 {path}: {e}"));
        }
    } else {
        crate::log::info(&format!("media: 目标文件已不存在，仅清理索引 {path}"));
    }
    if !cover.is_empty() {
        let cover_path = cover_dir(exe_root).join(&cover);
        if let Err(e) = std::fs::remove_file(&cover_path) {
            crate::log::warn(&format!(
                "media: 删除封面缓存失败 {}: {e}",
                cover_path.display()
            ));
        }
    }
    crate::log::info(&format!("media: 已删除文件 {path}"));
    serde_json::json!({ "ok": true, "error": "" }).to_string()
}

// ── 播放列表（v1 `media_playlist`：读文件，不入库）────────────────────

/// 弹系统的「选择播放列表」对话框；取消返回空串。
pub fn pick_playlist_json() -> String {
    let path = music::pick_playlist_file();
    serde_json::json!({ "ok": !path.is_empty(), "path": path }).to_string()
}

/// 打开一份播放列表文件（`.m3u` / `.m3u8` / `.pls`）→ 它的曲目。
///
/// **与媒体库是两条路**：只读那个文件，不入库、不要求目录已添加
/// （播放列表常常指向别处的目录）—— 与 v1 同口径。返回的形状与 `tracks_json` 一致
/// （`{ok,error,name,total,items[]}`），于是面板那套渲染 / 点播一行都不用改。
pub fn playlist_json(exe_root: &Path, path: &str) -> String {
    let file = Path::new(path);
    let ext = file
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(ext.as_str(), "m3u" | "m3u8" | "pls") {
        return serde_json::json!({
            "ok": false, "error": "ERR_NOT_PLAYLIST", "name": "", "total": 0, "items": [],
        })
        .to_string();
    }
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(e) => {
            return serde_json::json!({
                "ok": false,
                "error": format!("读取播放列表失败：{e}"),
                "name": "", "total": 0, "items": [],
            })
            .to_string()
        }
    };
    let name = file
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    let paths = music::parse_playlist(
        &music::decode_lyrics_bytes(&bytes),
        file.parent(),
    );
    let covers = cover_dir(exe_root);
    // 库里有的直接取（快）；没有的现场读标签（不入库，与 v1 同口径）。
    let conn = open_db(exe_root).ok();
    let mut items: Vec<serde_json::Value> = Vec::new();
    for entry in paths {
        let from_db = conn.as_ref().and_then(|conn| {
            conn.query_row(
                "SELECT title, artist, album, duration_ms, ext, size, cover
                   FROM tracks WHERE path = ?1",
                rusqlite::params![&entry],
                |row| {
                    let cover_name: String = row.get(6)?;
                    let ext: String = row.get(4)?;
                    let cover = if cover_name.is_empty() {
                        String::new()
                    } else {
                        covers.join(&cover_name).display().to_string()
                    };
                    Ok(serde_json::json!({
                        "path": entry,
                        "title": row.get::<_, String>(0)?,
                        "artist": row.get::<_, String>(1)?,
                        "album": row.get::<_, String>(2)?,
                        "durationMs": row.get::<_, i64>(3)?,
                        "ext": ext,
                        "size": row.get::<_, i64>(5)?,
                        "cover": cover,
                        "video": crate::convert::is_video_ext(&ext),
                    }))
                },
            )
            .ok()
        });
        if let Some(item) = from_db {
            items.push(item);
            continue;
        }
        let target = Path::new(&entry);
        if !target.is_file() {
            continue;
        }
        let track = music::read_track(target, &covers);
        let ext = target
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        items.push(serde_json::json!({
            "path": entry,
            "title": track.title,
            "artist": track.artist,
            "album": track.album,
            "durationMs": track.duration_ms,
            "ext": ext,
            "size": target.metadata().map(|m| m.len() as i64).unwrap_or(0),
            "cover": track.cover,
            "video": crate::convert::is_video_ext(&ext),
        }));
    }
    let total = items.len() as i64;
    serde_json::json!({
        "ok": true, "error": "", "name": name, "total": total, "items": items,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-media-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn insert_track(conn: &rusqlite::Connection, path: &str, root: &str, title: &str, artist: &str) {
        conn.execute(
            "INSERT INTO tracks (path, root, title, artist, album, duration_ms, ext, size, mtime, cover)
             VALUES (?1, ?2, ?3, ?4, 'Album', 1000, 'mp3', 10, 20, 'c.jpg')",
            rusqlite::params![path, root, title, artist],
        )
        .unwrap();
    }

    #[test]
    fn roots_report_missing_directories_and_track_counts() {
        let root = temp_root("roots");
        let music_dir = root.join("Music");
        std::fs::create_dir_all(&music_dir).unwrap();
        let conn = open_db(&root).unwrap();
        let dir = music_dir.display().to_string();
        conn.execute(
            "INSERT INTO roots (path, added_at) VALUES (?1, 1)",
            rusqlite::params![&dir],
        )
        .unwrap();
        insert_track(&conn, "x/a.mp3", &dir, "A", "Singer");
        let payload: serde_json::Value = serde_json::from_str(&roots_json(&root)).unwrap();
        let roots = payload["roots"].as_array().unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0]["tracks"], 1);
        assert_eq!(roots[0]["missing"], false);
        assert_eq!(roots[0]["name"], "Music");
    }

    #[test]
    fn tracks_query_filters_by_root_and_text_and_reports_the_full_total() {
        let root = temp_root("tracks");
        let conn = open_db(&root).unwrap();
        conn.execute(
            "INSERT INTO roots (path, added_at) VALUES ('D:/A', 1), ('D:/B', 2)",
            [],
        )
        .unwrap();
        insert_track(&conn, "D:/A/1.mp3", "D:/A", "夜曲", "周杰伦");
        insert_track(&conn, "D:/A/2.mp3", "D:/A", "晴天", "周杰伦");
        insert_track(&conn, "D:/B/3.mp3", "D:/B", "别的", "别人");
        drop(conn);

        let all: serde_json::Value = serde_json::from_str(&tracks_json(&root, "", "", 10)).unwrap();
        assert_eq!(all["total"], 3);
        let page: serde_json::Value = serde_json::from_str(&tracks_json(&root, "", "", 1)).unwrap();
        assert_eq!(page["total"], 3, "total 不受 limit 影响");
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        let by_root: serde_json::Value =
            serde_json::from_str(&tracks_json(&root, "D:/B", "", 10)).unwrap();
        assert_eq!(by_root["total"], 1);
        let by_text: serde_json::Value =
            serde_json::from_str(&tracks_json(&root, "", "周杰伦", 10)).unwrap();
        assert_eq!(by_text["total"], 2);
    }

    #[test]
    fn deleting_an_unknown_path_changes_nothing() {
        let root = temp_root("delete-unknown");
        let conn = open_db(&root).unwrap();
        insert_track(&conn, "D:/A/1.mp3", "D:/A", "A", "B");
        drop(conn);
        let payload: serde_json::Value =
            serde_json::from_str(&delete_file_json(&root, "D:/A/nope.mp3")).unwrap();
        assert_eq!(payload["ok"], false);
        assert!(payload["error"].as_str().unwrap().contains("不在库中"));
        let again: serde_json::Value = serde_json::from_str(&tracks_json(&root, "", "", 10)).unwrap();
        assert_eq!(again["total"], 1, "不该删掉任何东西");
    }

    #[test]
    fn deleting_a_known_track_removes_the_row_and_the_cover_cache() {
        let root = temp_root("delete-known");
        let covers = cover_dir(&root);
        std::fs::create_dir_all(&covers).unwrap();
        let cover = covers.join("c.jpg");
        std::fs::write(&cover, b"x").unwrap();
        let audio = root.join("a.mp3");
        std::fs::write(&audio, b"x").unwrap();
        let path = audio.display().to_string();
        let conn = open_db(&root).unwrap();
        insert_track(&conn, &path, &root.display().to_string(), "A", "B");
        drop(conn);

        let payload: serde_json::Value =
            serde_json::from_str(&delete_file_json(&root, &path)).unwrap();
        assert_eq!(payload["ok"], true);
        assert!(!audio.exists(), "磁盘文件要删掉");
        assert!(!cover.exists(), "封面缓存要删掉");
        let left: serde_json::Value = serde_json::from_str(&tracks_json(&root, "", "", 10)).unwrap();
        assert_eq!(left["total"], 0);
    }

    #[test]
    fn removing_a_root_drops_its_rows() {
        let root = temp_root("remove-root");
        let dir = root.join("Music");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.display().to_string();
        let conn = open_db(&root).unwrap();
        conn.execute(
            "INSERT INTO roots (path, added_at) VALUES (?1, 1)",
            rusqlite::params![&path],
        )
        .unwrap();
        insert_track(&conn, "x/a.mp3", &path, "A", "B");
        drop(conn);

        let payload: serde_json::Value =
            serde_json::from_str(&remove_root_json(&root, &path)).unwrap();
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["roots"].as_array().unwrap().len(), 0);
        let left: serde_json::Value = serde_json::from_str(&tracks_json(&root, "", "", 10)).unwrap();
        assert_eq!(left["total"], 0, "删根要连同它的曲目一起删");
    }

    #[test]
    fn scan_status_starts_idle() {
        let payload: serde_json::Value = serde_json::from_str(&scan_status_json()).unwrap();
        assert_eq!(payload["running"], false);
        assert!(payload["total"].as_u64().is_some());
    }

    #[test]
    fn playlist_reads_files_without_touching_the_library() {
        let root = temp_root("playlist");
        let music_dir = root.join("Music");
        std::fs::create_dir_all(&music_dir).unwrap();
        let a = music_dir.join("a.mp3");
        let b = music_dir.join("b.mp3");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"y").unwrap();
        let list = music_dir.join("mix.m3u8");
        std::fs::write(
            &list,
            format!("#EXTM3U\n{}\n{}\n", a.display(), b.display()),
        )
        .unwrap();

        let payload: serde_json::Value =
            serde_json::from_str(&playlist_json(&root, &list.display().to_string())).unwrap();
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["total"], 2);
        assert_eq!(payload["name"], "mix.m3u8");
        // 读播放列表**不入库**（v1 同口径）。
        let left: serde_json::Value = serde_json::from_str(&tracks_json(&root, "", "", 10)).unwrap();
        assert_eq!(left["total"], 0);
    }

    #[test]
    fn playlist_rejects_other_extensions() {
        let root = temp_root("playlist-bad");
        let file = root.join("x.txt");
        std::fs::write(&file, "x").unwrap();
        let payload: serde_json::Value =
            serde_json::from_str(&playlist_json(&root, &file.display().to_string())).unwrap();
        assert_eq!(payload["ok"], false);
        assert_eq!(payload["error"], "ERR_NOT_PLAYLIST");
    }
}
