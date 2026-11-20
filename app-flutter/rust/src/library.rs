//! 图书馆式检索库（2026-11-20）：把「对话历史 / 长期记忆」写成同一套
//! **日期 → 对话 → 任务 → 操作** 结构，供 `SessionSearch` / `MemorySearch` 按任务与标签取用。
//!
//! 为什么单独一层：JSON 快照（`chat-history.v2.json`）与 `MEMORY.md` 仍是**正文真相源**
//! （agent 注入 / 用户直接看文件都依赖它们），但它们不适合「按标签 + 按任务只取一小段」。
//! 本模块只做**索引**：正文一变就重建对应范围，检索时只返回命中的操作，不把整份历史
//! 塞进上下文。索引与正文同根（`<exe 根>/ModuleData/library/library.db`），不跨机同步。
//!
//! 结构照「图书馆」四层，字段与查询一一对应：
//!   · `date`        —— 哪一天（本地时区 `YYYY-MM-DD`）；
//!   · `session_id`  —— 哪个对话（同一份 `id` 与历史 UI 一致）；
//!   · `task_id`     —— 该对话里的哪个任务（一条用户提问 = 一个任务）；
//!   · `operation`   —— 该任务里的哪次操作（思考 / 工具 / 回复 / 记忆条目）。
//!
//! `priority` 只表达「同一命中里谁更值得先给模型看」（用户提问 > 最终回复 > 工具调用 >
//! 工具结果 > 思考），不参与排序之外的任何决策；`tags` 是模型按任务取用时的第一道过滤。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection};
use serde_json::Value;

/// 对话历史在索引里的 scope（与记忆的 `global` / `pet` / `skill:<key>` 分开）。
pub const CHAT_SCOPE: &str = "chat";

/// 单条操作写进索引时的正文上限（与前端 `stepsJson` 的单条上限同量级）。
const MAX_TEXT_CHARS: usize = 4_000;
/// 每条操作最多保留的标签数（标签只是索引，不替代正文）。
const MAX_TAGS: usize = 12;
/// 检索时最多从库里捞出的候选行数（先粗筛，再在内存里精排）。
const MAX_CANDIDATES: usize = 4_000;

/// 索引库路径：`<exe 根>/ModuleData/library/library.db`。
pub fn db_path(root: &Path) -> PathBuf {
    root.join("ModuleData").join("library").join("library.db")
}

/// 一条命中（已带完整四层路径，调用方不需要再回查会话）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub date: String,
    pub session_id: String,
    pub session_title: String,
    pub task_id: String,
    pub task_title: String,
    pub role: String,
    pub kind: String,
    pub text: String,
    pub tags: Vec<String>,
    pub priority: i64,
    pub created_at: u64,
}

/// 检索库的只读体量快照（设置 / 日志用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    pub entries: i64,
    pub sessions: i64,
    pub tasks: i64,
    pub db_bytes: u64,
}

impl Stats {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "entries": self.entries,
            "sessions": self.sessions,
            "tasks": self.tasks,
            "dbBytes": self.db_bytes,
        })
    }
}

/// 检索条件：`text` 为空 = 「按日期 / 对话 / 任务 / 标签浏览最近的」。
pub struct Query<'a> {
    pub text: &'a str,
    pub tags: &'a [String],
    pub date: Option<&'a str>,
    pub session: Option<&'a str>,
    pub task: Option<&'a str>,
    pub limit: usize,
    pub scope: &'a str,
}

fn open(root: &Path) -> Result<Connection, String> {
    match open_once(root) {
        Ok(conn) => Ok(conn),
        Err(reason) => rebuild_corrupt(root, &reason),
    }
}

fn open_once(root: &Path) -> Result<Connection, String> {
    let path = db_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建检索库目录失败（{}）: {e}", parent.display()))?;
    }
    let conn = Connection::open(&path).map_err(|e| format!("打开检索库失败: {e}"))?;
    // 与 chat_history 同口径：WAL 让「保存会话」与「检索」不互相卡死。
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;");
    let _ = conn.busy_timeout(std::time::Duration::from_millis(3000));
    init_schema(&conn)?;
    Ok(conn)
}

/// 库文件损坏（断电 / 磁盘错误 / 手工改坏）时挪到一边重建。
///
/// 索引是正文的派生层：宁可空库重来，也不能让检索 / 保存历史一直报错。
/// 下次 `chat_history::load_json` 的 `sync_sessions` 会按会话正文把索引补回来。
fn rebuild_corrupt(root: &Path, reason: &str) -> Result<Connection, String> {
    let lower = reason.to_ascii_lowercase();
    // 只有「确实是坏库」才重建；权限 / 磁盘满等问题换个空库也没用，原样报错。
    let corrupt = lower.contains("not a database")
        || lower.contains("malformed")
        || lower.contains("file is encrypted")
        || lower.contains("database disk image");
    if !corrupt {
        return Err(reason.to_string());
    }
    let path = db_path(root);
    if !path.exists() {
        return Err(reason.to_string());
    }
    let stamp = crate::cron::now_ms();
    let backup = path.with_file_name(format!("library.db.corrupt-{stamp}"));
    std::fs::rename(&path, &backup).map_err(|e| {
        format!(
            "检索库损坏且无法备份（{}）: {e}；原始错误: {reason}",
            path.display()
        )
    })?;
    // WAL / SHM 跟着主文件一起挪走，避免新库误读旧日志。
    for suffix in ["-wal", "-shm"] {
        let from = path.with_file_name(format!("library.db{suffix}"));
        if from.exists() {
            let to = backup.with_file_name(format!(
                "library.db.corrupt-{stamp}{suffix}"
            ));
            let _ = std::fs::rename(from, to);
        }
    }
    eprintln!(
        "[lunac-library] 检索库损坏，已重建（备份：{}）：{reason}",
        backup.display()
    );
    open_once(root)
}

fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS entries (
            id            TEXT PRIMARY KEY,
            scope         TEXT NOT NULL,
            date          TEXT NOT NULL,
            session_id    TEXT NOT NULL,
            session_title TEXT NOT NULL,
            task_id       TEXT NOT NULL,
            task_title    TEXT NOT NULL,
            operation_id  TEXT NOT NULL,
            ordinal       INTEGER NOT NULL,
            role          TEXT NOT NULL,
            kind          TEXT NOT NULL,
            text          TEXT NOT NULL,
            tags          TEXT NOT NULL,
            priority      INTEGER NOT NULL,
            created_at    INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS entries_scope   ON entries(scope);
        CREATE INDEX IF NOT EXISTS entries_date    ON entries(date);
        CREATE INDEX IF NOT EXISTS entries_session ON entries(session_id);
        CREATE INDEX IF NOT EXISTS entries_task    ON entries(task_id);
        -- 每个会话只重建一次：updatedAt 相同且已有索引就跳过。
        CREATE TABLE IF NOT EXISTS session_index (
            session_id TEXT PRIMARY KEY,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        -- 全文检索：CJK 必须用 trigram（unicode61 会把中文整段当一个 token）。
        CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
            text, task_title, tags,
            content='entries', content_rowid='rowid', tokenize='trigram'
        );
        CREATE TRIGGER IF NOT EXISTS entries_ai AFTER INSERT ON entries BEGIN
            INSERT INTO entries_fts(rowid, text, task_title, tags)
            VALUES (new.rowid, new.text, new.task_title, new.tags);
        END;
        CREATE TRIGGER IF NOT EXISTS entries_ad AFTER DELETE ON entries BEGIN
            INSERT INTO entries_fts(entries_fts, rowid, text, task_title, tags)
            VALUES ('delete', old.rowid, old.text, old.task_title, old.tags);
        END;
        CREATE TRIGGER IF NOT EXISTS entries_au AFTER UPDATE ON entries BEGIN
            INSERT INTO entries_fts(entries_fts, rowid, text, task_title, tags)
            VALUES ('delete', old.rowid, old.text, old.task_title, old.tags);
            INSERT INTO entries_fts(rowid, text, task_title, tags)
            VALUES (new.rowid, new.text, new.task_title, new.tags);
        END;
        "#,
    )
    .map_err(|e| format!("初始化检索库失败: {e}"))?;
    // 旧库第一次升级到 FTS：一次性重建，之后靠触发器增量维护。
    let ready: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM meta WHERE key = 'fts_ready'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("读取检索库版本失败: {e}"))?;
    if ready == 0 {
        conn.execute("INSERT INTO entries_fts(entries_fts) VALUES('rebuild')", [])
            .map_err(|e| format!("重建全文索引失败: {e}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('fts_ready', '1')",
            [],
        )
        .map_err(|e| format!("记录全文索引版本失败: {e}"))?;
    }
    Ok(())
}

/// 重建一份**对话会话**的四层索引（`upsert` 后调用）。
pub fn index_session(root: &Path, session: &Value) -> Result<(), String> {
    let mut conn = open(root)?;
    index_session_in(&mut conn, session)
}

fn index_session_in(conn: &mut Connection, session: &Value) -> Result<(), String> {
    let Some(id) = session
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Err("会话缺少 id".to_string());
    };
    let updated = session
        .get("updatedAt")
        .and_then(Value::as_u64)
        .or_else(|| session.get("createdAt").and_then(Value::as_u64))
        .unwrap_or_else(crate::cron::now_ms);
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "未命名对话".to_string());
    let (date, _) = crate::cron::local_stamp(updated);
    let rows = session_rows(session, &title);

    let tx = conn
        .transaction()
        .map_err(|e| format!("打开索引事务失败: {e}"))?;
    tx.execute(
        "DELETE FROM entries WHERE scope = ?1 AND session_id = ?2",
        params![CHAT_SCOPE, id],
    )
    .map_err(|e| format!("清理旧索引失败: {e}"))?;
    for (task_index, (task_title, ops)) in rows.iter().enumerate() {
        let task_id = format!("{id}:t{task_index}");
        for (op_index, (kind, role, text)) in ops.iter().enumerate() {
            let operation_id = format!("{task_id}:o{op_index}");
            let tags = extract_tags(&format!("{task_title} {kind} {text}"));
            tx.execute(
                "INSERT OR REPLACE INTO entries
                 (id, scope, date, session_id, session_title, task_id, task_title,
                  operation_id, ordinal, role, kind, text, tags, priority, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    format!("{CHAT_SCOPE}:{id}:t{task_index}:o{op_index}"),
                    CHAT_SCOPE,
                    date,
                    id,
                    title,
                    task_id,
                    task_title,
                    operation_id,
                    op_index as i64,
                    role,
                    kind,
                    clip(text),
                    serde_json::to_string(&tags).unwrap_or_else(|_| "[]".to_string()),
                    priority(role, kind),
                    updated as i64,
                ],
            )
            .map_err(|e| format!("写检索索引失败: {e}"))?;
        }
    }
    tx.execute(
        "INSERT OR REPLACE INTO session_index(session_id, updated_at) VALUES (?1, ?2)",
        params![id, updated as i64],
    )
    .map_err(|e| format!("记录会话索引版本失败: {e}"))?;
    tx.commit().map_err(|e| format!("提交检索索引失败: {e}"))
}

/// 删除一份会话的全部索引（`delete` 后调用）。
pub fn remove_session(root: &Path, id: &str) -> Result<(), String> {
    let conn = open(root)?;
    conn.execute(
        "DELETE FROM entries WHERE scope = ?1 AND session_id = ?2",
        params![CHAT_SCOPE, id],
    )
    .map_err(|e| format!("删除会话索引失败: {e}"))?;
    let _ = conn.execute("DELETE FROM session_index WHERE session_id = ?1", params![id]);
    Ok(())
}

/// 检索库体量快照（只读）。
pub fn stats(root: &Path) -> Result<Stats, String> {
    let conn = open(root)?;
    stats_in(&conn, root)
}

fn stats_in(conn: &Connection, root: &Path) -> Result<Stats, String> {
    let entries: i64 = conn
        .query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))
        .map_err(|e| format!("统计检索条目失败: {e}"))?;
    let sessions: i64 = conn
        .query_row("SELECT COUNT(DISTINCT session_id) FROM entries", [], |row| row.get(0))
        .map_err(|e| format!("统计对话数失败: {e}"))?;
    let tasks: i64 = conn
        .query_row("SELECT COUNT(DISTINCT task_id) FROM entries", [], |row| row.get(0))
        .map_err(|e| format!("统计任务数失败: {e}"))?;
    let db_bytes = std::fs::metadata(db_path(root))
        .map(|meta| meta.len())
        .unwrap_or(0);
    Ok(Stats {
        entries,
        sessions,
        tasks,
        db_bytes,
    })
}

/// 日常维护：清掉已删会话的孤儿索引；库超过 32 MiB 时 `VACUUM` 回收页。
///
/// 返回维护后的体量快照；失败只返回错误，不影响正文保存。
pub fn maintain(root: &Path) -> Result<Stats, String> {
    let conn = open(root)?;
    conn.execute(
        "DELETE FROM entries
          WHERE scope = ?1
            AND session_id NOT IN (SELECT session_id FROM session_index)",
        params![CHAT_SCOPE],
    )
    .map_err(|e| format!("清理孤儿索引失败: {e}"))?;
    conn.execute(
        "DELETE FROM session_index
          WHERE session_id NOT IN (SELECT DISTINCT session_id FROM entries WHERE scope = ?1)",
        params![CHAT_SCOPE],
    )
    .map_err(|e| format!("清理空会话索引失败: {e}"))?;
    let before = stats_in(&conn, root)?;
    if before.db_bytes > 32 * 1024 * 1024 {
        conn.execute_batch("PRAGMA optimize; VACUUM;")
            .map_err(|e| format!("整理检索库失败: {e}"))?;
        return stats_in(&conn, root);
    }
    Ok(before)
}

/// 首次启动 / 旧版本升级后把现有 JSON 会话补进索引；已是同一版本的会话不重写。
pub fn sync_sessions(root: &Path, sessions: &[Value]) -> Result<(), String> {
    let mut conn = open(root)?;
    let mut known: HashMap<String, u64> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT session_id, updated_at FROM session_index")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (id, updated) = row.map_err(|e| e.to_string())?;
            known.insert(id, updated);
        }
    }
    let live: HashSet<String> = sessions
        .iter()
        .filter_map(|session| session.get("id").and_then(Value::as_str))
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    for (stale, _) in known.iter() {
        if !live.contains(stale) {
            let _ = conn.execute(
                "DELETE FROM entries WHERE scope = ?1 AND session_id = ?2",
                params![CHAT_SCOPE, stale],
            );
            let _ = conn.execute(
                "DELETE FROM session_index WHERE session_id = ?1",
                params![stale],
            );
        }
    }
    for session in sessions {
        let Some(id) = session.get("id").and_then(Value::as_str) else {
            continue;
        };
        let updated = session
            .get("updatedAt")
            .and_then(Value::as_u64)
            .or_else(|| session.get("createdAt").and_then(Value::as_u64))
            .unwrap_or(0);
        if known.get(id).is_some_and(|current| *current == updated) {
            continue;
        }
        if let Err(reason) = index_session_in(&mut conn, session) {
            eprintln!("[lunac-library] 会话索引失败（{id}）: {reason}");
        }
    }
    Ok(())
}

/// 把一份**长期记忆正文**（`MEMORY.md` + 页）重建为同结构的索引。
///
/// `scope` = `global` / `pet` / `skill:<key>`；记忆条目没有「用户提问」，任务层用
/// 页名（默认页记作「长期记忆」），操作层是一行 bullet。标签来自 bullet 正文。
pub fn index_memory_text(root: &Path, scope: &str, date: &str, text: &str) -> Result<(), String> {
    let mut conn = open(root)?;
    replace_memory_scope(&mut conn, scope, date, text)
}

/// 记忆正文 → 索引：只 upsert 当前条目、删除已消失的条目。
///
/// 旧实现每次先 `DELETE scope` 再全量插入；记忆是追加式写入，这样会反复重写
/// 整个 scope（24k 字时尤其明显）。现在按 `scope:p<page>:o<op>` 的稳定 id 做差量。
fn replace_memory_scope(
    conn: &mut Connection,
    scope: &str,
    date: &str,
    text: &str,
) -> Result<(), String> {
    let entries = memory_rows(text);
    let mut wanted: HashSet<String> = HashSet::new();
    let mut rows: Vec<(String, String, String, String, String, Vec<String>, i64)> = Vec::new();
    for (task_index, (page, bullets)) in entries.iter().enumerate() {
        let task_id = format!("{scope}:p{task_index}");
        let task_title = if page == "MEMORY" {
            "长期记忆".to_string()
        } else {
            page.clone()
        };
        for (op_index, bullet) in bullets.iter().enumerate() {
            let id = format!("{scope}:p{task_index}:o{op_index}");
            let entry_date = provenance_date(bullet).unwrap_or_else(|| date.to_string());
            let tags = extract_tags(&format!("{task_title} {bullet}"));
            wanted.insert(id.clone());
            rows.push((
                id,
                entry_date,
                task_id.clone(),
                task_title.clone(),
                clip(bullet),
                tags,
                op_index as i64,
            ));
        }
    }

    let existing: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT id FROM entries WHERE scope = ?1")
            .map_err(|e| format!("读取记忆索引失败: {e}"))?;
        let rows = stmt
            .query_map(params![scope], |row| row.get::<_, String>(0))
            .map_err(|e| format!("读取记忆索引失败: {e}"))?;
        rows.flatten().collect()
    };

    let tx = conn
        .transaction()
        .map_err(|e| format!("打开记忆索引事务失败: {e}"))?;
    for stale in existing {
        if !wanted.contains(&stale) {
            tx.execute("DELETE FROM entries WHERE id = ?1", params![stale])
                .map_err(|e| format!("删除旧记忆索引失败: {e}"))?;
        }
    }
    for (id, entry_date, task_id, task_title, body, tags, op_index) in rows {
        // 先删后插：稳定 id 重新命中时也能让 FTS 的 delete/insert 触发器都跑到。
        tx.execute("DELETE FROM entries WHERE id = ?1", params![id])
            .map_err(|e| format!("更新记忆索引失败: {e}"))?;
        tx.execute(
            "INSERT INTO entries
             (id, scope, date, session_id, session_title, task_id, task_title,
              operation_id, ordinal, role, kind, text, tags, priority, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                id,
                scope,
                entry_date,
                scope,
                task_title,
                task_id,
                task_title,
                format!("{task_id}:o{op_index}"),
                op_index,
                "memory",
                "memory",
                body,
                serde_json::to_string(&tags).unwrap_or_else(|_| "[]".to_string()),
                priority("memory", "memory"),
                crate::cron::now_ms() as i64,
            ],
        )
        .map_err(|e| format!("写记忆索引失败: {e}"))?;
    }
    tx.commit().map_err(|e| format!("提交记忆索引失败: {e}"))
}

/// 清空某个 scope 的记忆索引（`/memory clear` / 技能记忆清空后调用）。
pub fn clear_scope(root: &Path, scope: &str) -> Result<(), String> {
    let conn = open(root)?;
    conn.execute("DELETE FROM entries WHERE scope = ?1", params![scope])
        .map_err(|e| format!("清空检索索引失败: {e}"))?;
    Ok(())
}

/// 检索。先按 scope / 日期 / 对话 / 任务粗筛，再按标签 + 关键词 + 优先级精排。
pub fn search(root: &Path, query: &Query<'_>) -> Result<Vec<Hit>, String> {
    let conn = open(root)?;
    let terms = query_terms(query.text);
    // FTS5 trigram 对 <3 字符无法建索引；短查询继续走后面的 LIKE 路径。
    let fts_ids: Option<HashSet<i64>> = if terms.join("").chars().count() >= 3 {
        let fts_query = terms
            .iter()
            .map(|term| format!("\"{}\"", term.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut ids = HashSet::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT rowid FROM entries_fts WHERE entries_fts MATCH ? LIMIT ?",
        ) {
            if let Ok(rows) = stmt.query_map(
                params![fts_query, MAX_CANDIDATES as i64],
                |row| row.get::<_, i64>(0),
            ) {
                for row in rows.flatten() {
                    ids.insert(row);
                }
            }
        }
        if ids.is_empty() { None } else { Some(ids) }
    } else {
        None
    };
    let mut sql = String::from(
        "SELECT date, session_id, session_title, task_id, task_title, role, kind, text,
                tags, priority, created_at, entries.rowid
           FROM entries WHERE scope = ?",
    );
    let mut values: Vec<SqlValue> = vec![SqlValue::Text(query.scope.to_string())];
    if let Some(date) = query.date.map(str::trim).filter(|v| !v.is_empty()) {
        sql.push_str(" AND date = ?");
        values.push(SqlValue::Text(date.to_string()));
    }
    if let Some(session) = query.session.map(str::trim).filter(|v| !v.is_empty()) {
        sql.push_str(" AND (session_id = ? OR session_title LIKE ?)");
        values.push(SqlValue::Text(session.to_string()));
        values.push(SqlValue::Text(format!("%{session}%")));
    }
    if let Some(task) = query.task.map(str::trim).filter(|v| !v.is_empty()) {
        sql.push_str(" AND (task_id = ? OR task_title LIKE ?)");
        values.push(SqlValue::Text(task.to_string()));
        values.push(SqlValue::Text(format!("%{task}%")));
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ?");
    values.push(SqlValue::Integer(MAX_CANDIDATES as i64));

    let mut stmt = conn.prepare(&sql).map_err(|e| format!("准备检索失败: {e}"))?;
    let rows = stmt
        .query_map(params_from_iter(values.iter()), |row| {
            let tags: String = row.get(8)?;
            Ok((
                Hit {
                    date: row.get(0)?,
                    session_id: row.get(1)?,
                    session_title: row.get(2)?,
                    task_id: row.get(3)?,
                    task_title: row.get(4)?,
                    role: row.get(5)?,
                    kind: row.get(6)?,
                    text: row.get(7)?,
                    tags: parse_tags(&tags),
                    priority: row.get(9)?,
                    created_at: row.get::<_, i64>(10)? as u64,
                },
                row.get::<_, i64>(11)?,
            ))
        })
        .map_err(|e| format!("读取检索结果失败: {e}"))?;

    let filter_tags: Vec<String> = query
        .tags
        .iter()
        .map(|tag| tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect();
    let limit = query.limit.clamp(1, 30);
    let mut scored: Vec<(f64, Hit)> = Vec::new();
    for row in rows {
        let (hit, rowid) = row.map_err(|e| format!("读取检索结果失败: {e}"))?;
        if let Some(ids) = &fts_ids {
            if !ids.contains(&rowid) {
                continue;
            }
        }
        let lower_text = hit.text.to_lowercase();
        let lower_title = hit.task_title.to_lowercase();
        let mut tag_hits = 0usize;
        for tag in &filter_tags {
            if hit.tags.iter().any(|item| item.to_lowercase() == *tag) {
                tag_hits += 1;
            }
        }
        if !filter_tags.is_empty() && tag_hits == 0 {
            continue;
        }
        let mut text_hits = 0usize;
        if !terms.is_empty() {
            for term in &terms {
                text_hits += lower_text.matches(term).count();
                if lower_title.contains(term) {
                    text_hits += 2;
                }
            }
            if text_hits == 0 {
                continue;
            }
        }
        let score = tag_hits as f64 * 8.0
            + text_hits as f64
            + hit.priority as f64 / 100.0;
        scored.push((score, hit));
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.1.created_at.cmp(&a.1.created_at))
    });
    Ok(scored.into_iter().take(limit).map(|(_, hit)| hit).collect())
}

/// 标签提取：英文 / 数字词（≥ 2 位）与中文连续片段（≤ 16 字），去重、限 12 个。
///
/// 它只服务索引，不是分词器：宁可多给几个粗标签，也不把整段正文塞进标签列。
pub fn extract_tags(text: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "and", "for", "with", "that", "this", "from", "have", "you", "are", "was",
        "were", "will", "can", "not", "but", "all", "any", "use", "into", "out", "然后", "就是",
        "这个", "那个", "我们", "你们", "他们", "可以", "需要", "一个", "没有",
    ];
    let mut tags = Vec::new();
    let mut push = |tag: String| {
        let tag = tag.trim().to_lowercase();
        if tag.is_empty() || tags.len() >= MAX_TAGS {
            return;
        }
        if !tags.iter().any(|item| item == &tag) {
            tags.push(tag);
        }
    };
    let mut word = String::new();
    let mut cjk = String::new();
    let flush_word = |word: &mut String, push: &mut dyn FnMut(String)| {
        if word.chars().count() >= 2 && !STOP.contains(&word.as_str()) {
            push(word.clone());
        }
        word.clear();
    };
    let flush_cjk = |cjk: &mut String, push: &mut dyn FnMut(String)| {
        let count = cjk.chars().count();
        if count >= 2 {
            if count <= 16 {
                push(cjk.clone());
            } else {
                push(cjk.chars().take(8).collect());
                push(cjk.chars().skip(count - 8).collect());
            }
        }
        cjk.clear();
    };
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '+' | '#' | '.') {
            if !cjk.is_empty() {
                flush_cjk(&mut cjk, &mut push);
            }
            word.push(ch);
        } else if is_cjk(ch) {
            flush_word(&mut word, &mut push);
            cjk.push(ch);
        } else {
            flush_word(&mut word, &mut push);
            flush_cjk(&mut cjk, &mut push);
        }
    }
    flush_word(&mut word, &mut push);
    flush_cjk(&mut cjk, &mut push);
    tags
}

fn query_terms(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|term| term.trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .collect()
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
    )
}

fn priority(role: &str, kind: &str) -> i64 {
    match (role, kind) {
        ("user", _) => 100,
        (_, "assistant") => 85,
        (_, "toolUse") => 70,
        (_, "toolResult") => 55,
        (_, "error") => 45,
        (_, "thinking") => 35,
        (_, "notice") => 30,
        ("memory", _) => 60,
        _ => 20,
    }
}

fn clip(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX_TEXT_CHARS {
        return trimmed.to_string();
    }
    let kept: String = trimmed.chars().take(MAX_TEXT_CHARS).collect();
    format!("{kept}…")
}

fn parse_tags(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

/// 一条会话 → `[(任务标题, [(kind, role, text)])]`。
///
/// 优先用前端已落盘的 `steps`（它含思考 / 工具），但**用户提问以 `messages` 为准**：
/// 旧版 `steps` 可能因体积上限丢掉最早的提问，而「用户问了什么」不能从索引里消失。
fn session_rows(session: &Value, title: &str) -> Vec<(String, Vec<(String, String, String)>)> {
    let messages: Vec<&Value> = session
        .get("messages")
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default();
    let steps: Vec<&Value> = session
        .get("steps")
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default();
    let step_users = steps
        .iter()
        .filter(|step| step.get("kind").and_then(Value::as_str) == Some("user"))
        .count();
    let message_users = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .count();
    let use_steps = !steps.is_empty() && step_users >= message_users;

    let mut tasks: Vec<(String, Vec<(String, String, String)>)> = Vec::new();
    if use_steps {
        for step in steps {
            let kind = step
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("notice")
                .to_string();
            let text = step_text(step);
            if kind == "user" {
                let task_title = summarize(&text, title);
                tasks.push((task_title, vec![(kind, "user".to_string(), text)]));
                continue;
            }
            if text.trim().is_empty() {
                continue;
            }
            let role = match kind.as_str() {
                "assistant" => "assistant",
                "toolUse" | "toolResult" => "assistant",
                _ => "operation",
            };
            if tasks.is_empty() {
                tasks.push((title.to_string(), Vec::new()));
            }
            tasks.last_mut().unwrap().1.push((kind, role.to_string(), text));
        }
    } else {
        for message in messages {
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("assistant");
            let text = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if role == "user" {
                let task_title = summarize(&text, title);
                tasks.push((task_title, vec![(role.to_string(), role.to_string(), text)]));
                continue;
            }
            if text.trim().is_empty() {
                continue;
            }
            if tasks.is_empty() {
                tasks.push((title.to_string(), Vec::new()));
            }
            tasks.last_mut().unwrap().1.push((
                role.to_string(),
                role.to_string(),
                text,
            ));
        }
    }
    if tasks.is_empty() {
        tasks.push((
            title.to_string(),
            vec![("notice".to_string(), "operation".to_string(), title.to_string())],
        ));
    }
    tasks
}

fn step_text(step: &Value) -> String {
    if let Some(text) = step.get("text").and_then(Value::as_str) {
        if !text.trim().is_empty() {
            return text.to_string();
        }
    }
    if let Some(detail) = step.get("detail").and_then(Value::as_str) {
        if !detail.trim().is_empty() {
            let name = step.get("name").and_then(Value::as_str).unwrap_or("工具");
            return format!("{name} {detail}");
        }
    }
    step.get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default()
}

/// 任务标题（与前端 `summarizeTaskTitle` 同口径）：首行 + 压空白 + 48 字截断。
pub fn summarize(text: &str, fallback: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let line = line
        .trim_start_matches(|c: char| {
            c.is_whitespace() || matches!(c, '#' | '-' | '*' | '>' | '`' | '1'..='9' | '.')
        })
        .trim();
    let mut compact = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        compact = fallback.trim().to_string();
    }
    if compact.is_empty() {
        compact = "未命名任务".to_string();
    }
    if compact.chars().count() > 48 {
        compact = format!("{}…", compact.chars().take(48).collect::<String>());
    }
    compact
}

/// 记忆正文 → `[(页名, [bullet])]`；默认页在第一组。
fn memory_rows(text: &str) -> Vec<(String, Vec<String>)> {
    let mut rows: Vec<(String, Vec<String>)> = Vec::new();
    let mut page = "MEMORY".to_string();
    let mut current: Option<(String, String)> = None;
    let flush = |rows: &mut Vec<(String, Vec<String>)>,
                 current: &mut Option<(String, String)>| {
        if let Some((target, body)) = current.take() {
            let body = body.trim().to_string();
            if body.is_empty() {
                return;
            }
            if let Some((_, items)) = rows.iter_mut().find(|(name, _)| name == &target) {
                items.push(body);
            } else {
                rows.push((target, vec![body]));
            }
        }
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("## ") {
            flush(&mut rows, &mut current);
            page = rest.trim().to_string();
            if page.is_empty() {
                page = "MEMORY".to_string();
            }
            continue;
        }
        if let Some(bullet) = line.strip_prefix("- ") {
            flush(&mut rows, &mut current);
            current = Some((page.clone(), bullet.trim().to_string()));
            continue;
        }
        if line == "-" {
            flush(&mut rows, &mut current);
            current = Some((page.clone(), String::new()));
            continue;
        }
        if let Some((_, body)) = current.as_mut() {
            let continuation = line.trim_start();
            if line.starts_with(' ') || line.starts_with('\t') {
                if !continuation.is_empty() {
                    body.push('\n');
                    body.push_str(continuation);
                }
            }
        }
    }
    flush(&mut rows, &mut current);
    rows
}

/// bullet 行首的 `[YYYY-MM-DD · ...]` 日期；没有就返回 None。
fn provenance_date(text: &str) -> Option<String> {
    let rest = text.trim_start().strip_prefix('[')?;
    let (inner, _) = rest.split_once(']')?;
    let date = inner.split_once(" · ").map(|(date, _)| date).unwrap_or(inner);
    let bytes = date.as_bytes();
    (bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| matches!(i, 4 | 7) || b.is_ascii_digit()))
    .then(|| date.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lunac-library-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    fn session(id: &str, updated: u64) -> Value {
        serde_json::json!({
            "id": id,
            "title": "设置窗口拖动",
            "createdAt": updated,
            "updatedAt": updated,
            "messages": [
                {"role": "user", "content": "设置界面打开时标题栏拖不动"},
                {"role": "assistant", "content": "修 search_page 的拖拽层"},
            ],
            "steps": [
                {"kind": "user", "text": "设置界面打开时标题栏拖不动"},
                {"kind": "thinking", "text": "先看 search_page 里设置面板怎么占满"},
                {"kind": "toolUse", "name": "Edit", "id": "t1",
                 "detail": r#"{"file_path":"D:\\cc\\a.dart"}"#},
                {"kind": "toolResult", "id": "t1", "text": "ok"},
                {"kind": "assistant", "text": "修好了"}
            ],
        })
    }

    #[test]
    fn chat_index_keeps_the_four_level_hierarchy() {
        let root = temp_root("hierarchy");
        let value = session("s1", 1_700_000_000_000);
        index_session(&root, &value).unwrap();

        let hits = search(
            &root,
            &Query {
                text: "拖不动",
                tags: &[],
                date: None,
                session: None,
                task: None,
                limit: 10,
                scope: CHAT_SCOPE,
            },
        )
        .unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].session_id, "s1");
        assert_eq!(hits[0].task_title, "设置界面打开时标题栏拖不动");
        assert!(!hits[0].task_title.contains("任务 1"));
        assert_eq!(hits[0].date.len(), 10, "日期用本地 YYYY-MM-DD");
        assert!(hits[0].text.contains("拖不动"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tag_filter_and_priority_prefer_user_questions() {
        let root = temp_root("tags");
        index_session(&root, &session("s1", 1_700_000_000_000)).unwrap();
        let tags = extract_tags("设置界面打开时标题栏拖不动");
        let hits = search(
            &root,
            &Query {
                text: "",
                tags: &tags,
                date: None,
                session: None,
                task: None,
                limit: 10,
                scope: CHAT_SCOPE,
            },
        )
        .unwrap();
        assert_eq!(hits.first().map(|hit| hit.role.as_str()), Some("user"));
        assert!(hits
            .iter()
            .all(|hit| hit.task_title == "设置界面打开时标题栏拖不动"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stats_and_maintenance_report_and_clean_the_library() {
        let root = temp_root("stats");
        index_session(&root, &session("s1", 1_700_000_000_000)).unwrap();
        let stats = stats(&root).unwrap();
        assert!(stats.entries > 0);
        assert_eq!(stats.sessions, 1);
        assert!(stats.tasks >= 1);
        assert!(stats.db_bytes > 0);
        let after = maintain(&root).unwrap();
        assert!(after.entries > 0, "维护不能误删正常会话的索引");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_library_is_backed_up_and_rebuilt() {
        let root = temp_root("corrupt");
        let path = db_path(&root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // 不是 SQLite 文件：旧实现会让 stats / 检索 / 保存一直报错。
        std::fs::write(&path, b"not a sqlite database").unwrap();

        let fresh = stats(&root).unwrap();
        assert_eq!(fresh.entries, 0);
        assert_eq!(fresh.sessions, 0);

        let backups = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("library.db.corrupt-")
            })
            .count();
        assert_eq!(backups, 1, "损坏库应备份一份而不是静默删除");

        // 重建后的库可正常写入。
        index_session(&root, &session("s1", 1_700_000_000_000)).unwrap();
        assert_eq!(stats(&root).unwrap().sessions, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_index_uses_the_same_schema_and_can_be_cleared() {
        let root = temp_root("memory");
        let text = "- [2026-11-20 · global] 构建用 cargo\n  - 第二行补充\n## build\n- 先跑 flutter test\n";
        index_memory_text(&root, "global", "2026-11-20", text).unwrap();
        let hits = search(
            &root,
            &Query {
                text: "cargo",
                tags: &[],
                date: None,
                session: None,
                task: None,
                limit: 10,
                scope: "global",
            },
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].date, "2026-11-20");
        assert!(hits[0].text.contains("第二行补充"));
        assert!(hits
            .iter()
            .any(|hit| hit.task_title == "长期记忆" || hit.task_title == "build"));
        clear_scope(&root, "global").unwrap();
        assert!(search(
            &root,
            &Query {
                text: "",
                tags: &[],
                date: None,
                session: None,
                task: None,
                limit: 10,
                scope: "global",
            },
        )
        .unwrap()
        .is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tags_are_deduped_and_limited() {
        let tags = extract_tags("Rust rust Rust 缓存命中 缓存命中 transformer transformer");
        assert!(tags.contains(&"rust".to_string()));
        assert!(tags.contains(&"transformer".to_string()));
        assert!(tags.len() <= MAX_TAGS);
        assert_eq!(tags.iter().filter(|tag| *tag == "rust").count(), 1);
    }
}
