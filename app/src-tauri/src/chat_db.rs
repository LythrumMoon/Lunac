//! 会话历史的 SQLite 存储（backlog §8.5 第一步，2026-09-17）。
//!
//! 为什么从 `chat-history.json` 换过来：旧实现是**单个 JSON 文件**，前端每次保存都把
//! 整个会话列表（含全部消息与过程快照）序列化后整体重写一遍 —— 历史越长，全量读改写
//! 的代价越大；而且「往期对话可检索」这件事在 JSON 上根本做不了。换 SQLite 后写入变成
//! 「事务里按会话重建」，并顺带建好 **FTS5 全文索引**，为后续的往期对话检索留通道。
//!
//! 两张 FTS 表是**有意提前建**的（backlog §8.5）：事后给已有数据补索引需要一次容易被
//! 漏掉的回填，而触发器同步是声明式的、不会漂。CJK 之所以要单独一张 `tokenize='trigram'`
//! 表：FTS5 默认分词器（unicode61）对中文不切词，中文子串检索在默认表上永远命中 0，
//! 必须走 trigram（照 Hermes `state.db` 的做法）。
//!
//! 对外暴露 `load` / `save` / `import_legacy`（存取与迁移）以及
//! `search`（往期会话检索，2026-09-19）——
//! 数据形状（`ChatSession`）留在 `storage.rs` —— 本模块不定义业务类型，
//! 免得出现两份「会话长什么样」的真相。

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::storage::{ChatMessage, ChatSession};

/// 打开（必要时创建）数据库文件。**每次调用各开一条连接**：会话读写在用户操作频率上
/// 是低频的（保存 = 一轮对话结束），不值得为省一次 open 引入跨线程共享连接与锁。
fn open(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| format!("打开会话库失败: {e}"))?;
    // WAL：读写不互斥，避免「保存会话」与「读取会话」撞车时其中一方直接 database is locked。
    // 用 execute_batch 而不是 pragma_update：`PRAGMA journal_mode` 会返回一行结果，
    // rusqlite 的 pragma_update 走 execute，遇到返回行的语句会报 ExecuteReturnedResults
    // —— 那样 WAL 会被静默地设不上。
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;");
    // 即便走了 WAL，检查点等场景仍可能短暂持锁：等一会儿比立刻失败好。
    let _ = conn.busy_timeout(std::time::Duration::from_millis(3000));
    Ok(conn)
}

/// 建表 / 建索引 / 建触发器。全部 `IF NOT EXISTS` ⇒ 幂等，每次打开都跑一遍也不怕，
/// 省掉一套 `PRAGMA user_version` 版本簿记（现在只有 v1，簿记是纯负担）。
fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            id         TEXT PRIMARY KEY,
            -- pos 保留「前端数组里的次序」。旧 JSON 是数组，顺序即语义（列表按它的顺序
            -- 渲染），而 SQLite 没有隐式顺序 —— 不显式存就退化成「按插入/主键顺序」这种
            -- 实现细节。
            pos        INTEGER NOT NULL DEFAULT 0,
            title      TEXT    NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL DEFAULT 0,
            -- 最后一次**内容变化**的时间（2026-11-05）：历史列表按它排序，跨天继续的
            -- 会话不会被当成「前一天」。旧库由下面的 ALTER 补列（0 ⇒ 前端回落 created_at）。
            updated_at INTEGER NOT NULL DEFAULT 0,
            -- 会话状态（2026-11-05）：'' / running / interrupted / done。
            -- 后端只在内容变化时补 'done'，中间态由前端显式上报。
            status     TEXT    NOT NULL DEFAULT '',
            usage      TEXT,
            steps      TEXT,
            -- 待办清单时间线（2026-10-02，JSON 文本）——见 storage.rs 的 SessionTodo。
            -- 与 usage / steps 同一纪律：可选结构走一列 JSON，不各开一堆列。
            todos      TEXT,
            -- 会话来源（2026-10-10）：'' = 普通聊天，'pet' = 桌宠发起。两条线程分开存，
            -- 前端历史 UI 据此分页。旧库由下面的 ALTER 补列。
            source     TEXT NOT NULL DEFAULT '',
            -- 生成这段对话时的人格展示名（2026-10-24）：历史回放用它署名，
            -- 避免「改了人格预设名，旧会话的署名跟着变」。旧库由 ALTER 补列。
            persona    TEXT NOT NULL DEFAULT ''
        );

        -- 有隐式 rowid（FTS 表用它对应行），(session_id, idx) 保证会话内消息有序且不重。
        CREATE TABLE IF NOT EXISTS messages (
            session_id TEXT    NOT NULL,
            idx        INTEGER NOT NULL,
            role       TEXT    NOT NULL,
            content    TEXT    NOT NULL,
            PRIMARY KEY (session_id, idx)
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts  USING fts5(content);
        CREATE VIRTUAL TABLE IF NOT EXISTS messages_trgm USING fts5(content, tokenize='trigram');

        -- 触发器同步：写 messages 就自动进两张索引表，调用方不必记得手动插索引
        -- （漏一次就是「搜不到刚聊过的内容」这种极难排查的问题）。
        -- 没有 UPDATE 触发器：保存路径是全删全插，不存在就地更新。
        CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
            INSERT INTO messages_fts(rowid, content)  VALUES (new.rowid, new.content);
            INSERT INTO messages_trgm(rowid, content) VALUES (new.rowid, new.content);
        END;
        CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
            DELETE FROM messages_fts  WHERE rowid = old.rowid;
            DELETE FROM messages_trgm WHERE rowid = old.rowid;
        END;
        "#,
    )
    .map_err(|e| format!("初始化会话库失败: {e}"))?;

    // 加列迁移（2026-10-02）：`CREATE TABLE IF NOT EXISTS` 对**已存在**的表**不会补列** ——
    // 老库（0.9.20 及以前建的）必须显式 ALTER，否则上面的 todos 列只在新库里存在。
    // 列已存在时 SQLite 报 `duplicate column name`，忽略即可 ——
    // 这比引一套 `PRAGMA user_version` 版本簿记轻得多（现在仍只有 v1，簿记是纯负担）。
    let _ = conn.execute("ALTER TABLE sessions ADD COLUMN todos TEXT", []);

    // 加列迁移（2026-10-10）：`source` 列同上 —— 老库没有它，必须显式 ALTER 补上。
    // 带 `NOT NULL DEFAULT ''` 是为了让**已有行**读出来是空串（= 普通聊天），与
    // storage.rs 的 `#[serde(default)]` 语义一致；列已存在时报 duplicate column，忽略即可。
    let _ = conn.execute(
        "ALTER TABLE sessions ADD COLUMN source TEXT NOT NULL DEFAULT ''",
        [],
    );

    // 加列迁移（2026-10-24）：人格展示名。老库没有它 ⇒ 读出来是空串，前端回落当前名。
    let _ = conn.execute(
        "ALTER TABLE sessions ADD COLUMN persona TEXT NOT NULL DEFAULT ''",
        [],
    );

    // 加列迁移（2026-11-05）：最后活动时间与会话状态。老库补列后 `updated_at = 0`，
    // 读取侧据此回落到 `created_at`（不在这里写值，避免每次打开库都改历史行）。
    let _ = conn.execute(
        "ALTER TABLE sessions ADD COLUMN updated_at INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE sessions ADD COLUMN status TEXT NOT NULL DEFAULT ''",
        [],
    );

    Ok(())
}

/// 当前毫秒时间戳（`updated_at` 用）。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 一段消息的内容指纹。只用于「这次保存有没有改到会话内容」，不参与任何业务判断。
fn hash_messages(msgs: &[ChatMessage]) -> u64 {
    let mut h = DefaultHasher::new();
    for m in msgs {
        m.role.hash(&mut h);
        m.content.hash(&mut h);
    }
    h.finish()
}

/// 全量写入：**在一个事务里**删掉所有旧数据，再按入参顺序插入。
///
/// 语义与旧的「整体重写 JSON」严格一致（前端传的是它内存里的**完整**列表，含被删掉的
/// 会话），所以这里不做差异比对 —— 那样得在存储层猜前端的「哪条被删了」语义，更脆。
/// 事务保证中途失败不会留下「会话还在、消息没了」的半截状态。
///
/// **`updated_at` / `status`（2026-11-05）**：`updated_at` 只在消息内容真的变化时推进，
/// 否则每次全量重写都会把所有会话的时间盖成同一刻，排序失去意义。`status` 同理：
/// 前端给了就尊重，没给时 —— 内容变了补 `done`，没变则保留旧值。
fn write_all(conn: &Connection, sessions: &[ChatSession]) -> Result<(), String> {
    // 先抓旧快照：id → (消息指纹, updated_at, status)。只在保存这一刻做一次。
    let mut prev: HashMap<String, (u64, u64, String)> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT id, updated_at, status FROM sessions")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                ))
            })
            .map_err(|e| e.to_string())?;
        for r in rows {
            let (id, ts, st) = r.map_err(|e| e.to_string())?;
            prev.insert(id, (0, ts, st));
        }
    }
    {
        let mut mstmt = conn
            .prepare("SELECT session_id, role, content FROM messages ORDER BY session_id ASC, idx ASC")
            .map_err(|e| e.to_string())?;
        let mrows = mstmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    ChatMessage {
                        role: r.get::<_, String>(1)?,
                        content: r.get::<_, String>(2)?,
                    },
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut by_id: HashMap<String, Vec<ChatMessage>> = HashMap::new();
        for r in mrows {
            let (sid, msg) = r.map_err(|e| e.to_string())?;
            by_id.entry(sid).or_default().push(msg);
        }
        for (id, msgs) in by_id {
            if let Some(e) = prev.get_mut(&id) {
                e.0 = hash_messages(&msgs);
            }
        }
    }

    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;

    let r = (|| -> Result<(), String> {
        // messages 先删（触发器顺手清索引），再删 sessions —— 顺序不能反。
        conn.execute("DELETE FROM messages", [])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM sessions", [])
            .map_err(|e| e.to_string())?;

        let mut ins_s = conn
            .prepare(
                "INSERT INTO sessions(id, pos, title, created_at, usage, steps, todos, source, persona, updated_at, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )
            .map_err(|e| e.to_string())?;
        let mut ins_m = conn
            .prepare("INSERT INTO messages(session_id, idx, role, content) VALUES (?1, ?2, ?3, ?4)")
            .map_err(|e| e.to_string())?;

        for (pos, s) in sessions.iter().enumerate() {
            // usage / steps / todos 是可选结构 → 统一存 JSON 文本（None 写 NULL），读回来再按
            // Option 反序列化；不为这几个字段各开一堆列。
            let usage = s
                .usage
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e| e.to_string())?;
            let steps = s
                .steps
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e| e.to_string())?;
            let todos = s
                .todos
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e| e.to_string())?;

            let hash = hash_messages(&s.messages);
            let (updated_at, status) = match prev.get(&s.id) {
                // 内容没变：时间与状态都沿用旧的（前端显式给了则尊重）。
                Some((h, ts, st)) if *h == hash => {
                    let ts = if s.updated_at > 0 {
                        s.updated_at
                    } else if *ts > 0 {
                        *ts
                    } else {
                        s.created_at
                    };
                    let st = if s.status.is_empty() {
                        st.clone()
                    } else {
                        s.status.clone()
                    };
                    (ts, st)
                }
                // 内容变了 / 新会话：时间推进；状态没给就记为 done。
                // 这里**不读 `s.updated_at`** —— 前端保存时会带着旧值，读它会把时间钉死在过去。
                _ => {
                    let st = if s.status.is_empty() {
                        "done".to_string()
                    } else {
                        s.status.clone()
                    };
                    (now_ms(), st)
                }
            };

            ins_s
                .execute(rusqlite::params![
                    s.id,
                    pos as i64,
                    s.title,
                    s.created_at as i64,
                    usage,
                    steps,
                    todos,
                    s.source,
                    s.persona,
                    updated_at as i64,
                    status
                ])
                .map_err(|e| e.to_string())?;

            for (idx, m) in s.messages.iter().enumerate() {
                ins_m
                    .execute(rusqlite::params![s.id, idx as i64, m.role, m.content])
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    })();

    match r {
        Ok(()) => conn.execute_batch("COMMIT").map_err(|e| e.to_string()),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// 按 `pos` 读回全部会话，消息按 `idx` 归并回各自会话。
fn read_all(conn: &Connection) -> Result<Vec<ChatSession>, String> {
    let mut out: Vec<ChatSession> = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT id, title, created_at, usage, steps, todos, source, persona, updated_at, status FROM sessions ORDER BY pos ASC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                let usage: Option<String> = row.get(3)?;
                let steps: Option<String> = row.get(4)?;
                let todos: Option<String> = row.get(5)?;
                Ok(ChatSession {
                    id: row.get::<_, String>(0)?,
                    title: row.get::<_, String>(1)?,
                    created_at: row.get::<_, i64>(2)? as u64,
                    // 老库的 updated_at 为 0 ⇒ 回落到 created_at，历史列表仍有可排序的值。
                    updated_at: {
                        let ts = row.get::<_, i64>(8)? as u64;
                        if ts == 0 {
                            row.get::<_, i64>(2)? as u64
                        } else {
                            ts
                        }
                    },
                    status: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
                    // 单条记录坏掉不该让整个历史列表读不出来 ⇒ 解析失败按 None 处理。
                    usage: usage.and_then(|s| serde_json::from_str(&s).ok()),
                    steps: steps.and_then(|s| serde_json::from_str(&s).ok()),
                    todos: todos.and_then(|s| serde_json::from_str(&s).ok()),
                    // 老库该列为 NULL / 老记录没这个字段 ⇒ 一律空串（= 普通聊天）。
                    source: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                    persona: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                    messages: Vec::new(),
                })
            })
            .map_err(|e| e.to_string())?;
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
    }

    let mut by_id: HashMap<String, Vec<ChatMessage>> = HashMap::new();
    {
        let mut mstmt = conn
            .prepare(
                "SELECT session_id, role, content FROM messages ORDER BY session_id ASC, idx ASC",
            )
            .map_err(|e| e.to_string())?;
        let mrows = mstmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    ChatMessage {
                        role: row.get::<_, String>(1)?,
                        content: row.get::<_, String>(2)?,
                    },
                ))
            })
            .map_err(|e| e.to_string())?;
        for r in mrows {
            let (sid, msg) = r.map_err(|e| e.to_string())?;
            by_id.entry(sid).or_default().push(msg);
        }
    }
    for s in out.iter_mut() {
        if let Some(msgs) = by_id.remove(&s.id) {
            s.messages = msgs;
        }
    }
    Ok(out)
}

/// 读取全部会话（库文件不存在会被创建为空库）。
pub fn load(path: &Path) -> Result<Vec<ChatSession>, String> {
    let conn = open(path)?;
    init_schema(&conn)?;
    read_all(&conn)
}

/// 全量覆盖写入。
pub fn save(path: &Path, sessions: &[ChatSession]) -> Result<(), String> {
    let conn = open(path)?;
    init_schema(&conn)?;
    write_all(&conn, sessions)
}

/// 一次性把旧的 `chat-history.json` 内容导入空库。
///
/// 只由「库文件不存在」这条路径调用 ⇒ 不会覆盖新数据。返回导入的会话数。
/// 旧 JSON 解析失败时返回 Err，调用方保持「读不出来就返回空列表」的老行为
/// （**不删旧文件**：用户仍可手工抢救，卸载也不该顺手毁数据）。
pub fn import_legacy(path: &Path, json: &str) -> Result<usize, String> {
    let sessions: Vec<ChatSession> =
        serde_json::from_str(json).map_err(|e| format!("旧会话 JSON 解析失败: {e}"))?;
    save(path, &sessions)?;
    Ok(sessions.len())
}

// ── 往期会话检索（2026-09-19，A2）──────────────────────────────────
//
// 两张 FTS 索引表从 2026-09-17 起就随写入用触发器维护着，但**一直没有调用方**
// （见模块头注释）。本节是那个调用方：给 agent 一个只读的「往期会话检索」。
//
// **为什么 FTS 与 LIKE 两条路都要**：FTS5 的 trigram 分词器要求查询词 **≥3 字符**，
// 而中文常用词大量是 2 字（「缓存」「命中」「热键」）—— 只走 trigram 会**静默漏掉**
// 这类查询（不报错、只是永远 0 条），是最难排查的那种「功能看起来在、其实没生效」。
// 所以短词落到 `LIKE` 全表扫；会话库是个人规模（万条消息级），一次 LIKE 完全可以接受。

/// 一条检索命中（未渲染成给模型看的文本）。
pub struct HistoryHit {
    pub session_id: String,
    pub title: String,
    pub created_at: u64,
    pub role: String,
    pub idx: i64,
    pub content: String,
}

/// 单条命中内联的正文上限（字符）。一条 assistant 消息可能是一整篇长文，
/// 全量回灌会把上下文一次吃掉 —— 模型要更多可以换个词再搜一次。
const HIT_CONTENT_CHARS: usize = 700;

/// 检索往期会话消息。排序 = 会话（新→旧），会话内按 `idx`。
pub fn search(path: &Path, query: &str, limit: usize) -> Result<Vec<HistoryHit>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let conn = open(path)?;
    init_schema(&conn)?;

    // FTS 语法 / 分词器对某些查询串会**直接报错**（例如全是标点、没有可索引的 token）。
    // 那种情况不该让整次检索失败 —— 落到 LIKE 照样能给出结果。
    match search_fts(&conn, q, limit) {
        Ok(hits) if !hits.is_empty() => Ok(hits),
        _ => search_like(&conn, q, limit),
    }
}

/// FTS 路径：trigram（CJK 子串）与 unicode61（英文 / 代码词）**任一命中**即算。
///
/// 查询串必须包成**带引号的短语**（内部的 `"` 双写转义）：不包的话 FTS5 会把它当查询
/// 语法解析，`AND` / `*` / `-` / `(` 这类字符会让整条语句报错。
fn search_fts(conn: &Connection, q: &str, limit: usize) -> Result<Vec<HistoryHit>, String> {
    let phrase = format!("\"{}\"", q.replace('"', "\"\""));
    let mut stmt = conn
        .prepare(
            "SELECT m.session_id, s.title, s.created_at, m.role, m.idx, substr(m.content, 1, ?3)
             FROM messages m JOIN sessions s ON s.id = m.session_id
             WHERE m.rowid IN (SELECT rowid FROM messages_trgm WHERE messages_trgm MATCH ?1)
                OR m.rowid IN (SELECT rowid FROM messages_fts  WHERE messages_fts  MATCH ?1)
             ORDER BY s.created_at DESC, m.idx ASC
             LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(
            rusqlite::params![phrase, limit as i64, HIT_CONTENT_CHARS as i64],
            |row| {
                Ok(HistoryHit {
                    session_id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: row.get::<_, i64>(2)? as u64,
                    role: row.get(3)?,
                    idx: row.get(4)?,
                    content: row.get(5)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// LIKE 路径（兜底）：FTS 未命中或查询词太短（<3 字符）时用。
///
/// 通配符必须转义 —— 否则用户搜 `100%` 会变成「匹配任意结尾」，`a_b` 会连 `axb` 一起命中。
fn search_like(conn: &Connection, q: &str, limit: usize) -> Result<Vec<HistoryHit>, String> {
    let escaped = q
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let mut stmt = conn
        .prepare(
            "SELECT m.session_id, s.title, s.created_at, m.role, m.idx, substr(m.content, 1, ?3)
             FROM messages m JOIN sessions s ON s.id = m.session_id
             WHERE m.content LIKE ?1 ESCAPE '\\'
             ORDER BY s.created_at DESC, m.idx ASC
             LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(
            rusqlite::params![pattern, limit as i64, HIT_CONTENT_CHARS as i64],
            |row| {
                Ok(HistoryHit {
                    session_id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: row.get::<_, i64>(2)? as u64,
                    role: row.get(3)?,
                    idx: row.get(4)?,
                    content: row.get(5)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{SessionTodo, SessionUsage};

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        init_schema(&c).unwrap();
        c
    }

    fn session(id: &str, title: &str, user_text: &str, asst_text: &str) -> ChatSession {
        ChatSession {
            id: id.into(),
            title: title.into(),
            created_at: 123,
            updated_at: 0,
            status: String::new(),
            usage: Some(SessionUsage {
                hit: 7,
                ..Default::default()
            }),
            steps: None,
            todos: None,
            persona: String::new(),
            source: String::new(),
            messages: vec![
                ChatMessage {
                    role: "user".into(),
                    content: user_text.into(),
                },
                ChatMessage {
                    role: "assistant".into(),
                    content: asst_text.into(),
                },
            ],
        }
    }

    /// **必测**：backlog §8.5 明确要求「立项前先验证该构建确实带 FTS5」。
    /// bundled 版 SQLite 有没有编进 FTS5 是构建配置问题 —— 不实测就可能等到线上
    /// `CREATE VIRTUAL TABLE … USING fts5` 直接报 no such module 才发现。
    #[test]
    fn fts5_is_compiled_in() {
        let c = mem();
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM pragma_compile_options WHERE compile_options LIKE '%FTS5%'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        assert!(n > 0, "bundled SQLite 未启用 FTS5");
    }

    /// 中文子串检索必须靠 trigram 表：默认 unicode61 对中文不切词。
    /// 这条同时锁住「两张表都建了、触发器确实在同步」—— 只建表不插索引也会在这里挂。
    #[test]
    fn cjk_substring_searchable_via_trigram() {
        let c = mem();
        write_all(
            &c,
            &[session(
                "s1",
                "标题",
                "帮我看看缓存命中率",
                "cache hit rate is low",
            )],
        )
        .unwrap();

        let cjk: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM messages_trgm WHERE messages_trgm MATCH ?1",
                rusqlite::params!["\"缓存命中\""],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cjk, 1, "trigram 表未索引到中文消息");

        // ASCII 走默认表，能按词命中（证明两张索引都活着）
        let ascii: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM messages_fts WHERE messages_fts MATCH ?1",
                rusqlite::params!["\"rate\""],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ascii, 1);
    }

    /// 往返：写入后读回必须逐字段一致（含会话顺序、消息顺序、usage 快照、消息为空的会话）。
    #[test]
    fn roundtrip_preserves_order_and_fields() {
        let c = mem();
        let a = session("a", "第一个", "问题一", "回答一");
        let mut b = session("b", "第二个", "问题二", "回答二");
        b.messages.clear();
        b.usage = None;
        write_all(&c, &[a, b]).unwrap();

        let back = read_all(&c).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].id, "a");
        assert_eq!(back[1].id, "b", "pos 必须保住入参数组顺序");
        assert_eq!(back[0].messages.len(), 2);
        assert_eq!(back[0].messages[0].role, "user");
        assert_eq!(back[0].messages[1].content, "回答一");
        assert_eq!(back[0].usage.as_ref().map(|u| u.hit), Some(7));
        assert!(back[1].messages.is_empty());
        assert!(back[1].usage.is_none());
    }

    /// 来源列要能原样往返（2026-10-10）：桌宠会话 `"pet"`、普通会话 `""` ——
    /// 历史 UI 的分页全靠这一列，丢了就分不开两条线程。
    #[test]
    fn source_roundtrip_preserves_pet_and_plain() {
        let c = mem();
        let plain = session("plain", "普通", "问题", "回答");
        let mut pet = session("pet", "桌宠", "你看到了什么", "我看到你在写代码");
        pet.source = "pet".into();
        write_all(&c, &[plain, pet]).unwrap();

        let back = read_all(&c).unwrap();
        assert_eq!(back[0].source, "");
        assert_eq!(back[1].source, "pet");
    }

    /// 待办清单时间线要能原样往返（2026-10-02）——「回退后任务列表不丢」全靠这一列。
    /// 同时钉住「不解构」这条纪律：条目里多余字段也必须原样带回来。
    #[test]
    fn todos_roundtrip_preserves_timeline() {
        let c = mem();
        let mut s = session("a", "带待办", "问题", "回答");
        s.todos = Some(vec![
            SessionTodo {
                turn: 0,
                todos: vec![serde_json::json!({"content": "第一步", "status": "completed"})],
            },
            SessionTodo {
                turn: 2,
                // `note` 是渲染不看的字段 —— Rust 侧不该把它吃掉。
                todos: vec![serde_json::json!({
                    "content": "第二步", "status": "in_progress", "note": "keep me"
                })],
            },
        ]);
        write_all(&c, &[s]).unwrap();

        let back = read_all(&c).unwrap();
        let t = back[0].todos.as_ref().expect("todos 必须读得回来");
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].turn, 0);
        assert_eq!(t[1].turn, 2);
        assert_eq!(t[1].todos[0]["note"], "keep me");
        assert_eq!(t[1].todos[0]["status"], "in_progress");
    }

    /// 加列迁移：老库（没有 todos / source 列）打开后必须被补上，且已有数据原样还在。
    /// 这是 0.9.20 → 新版升级路径上唯一会碰老库的地方。
    #[test]
    fn legacy_db_gets_todos_column_added() {
        let c = Connection::open_in_memory().unwrap();
        // 模拟 0.9.20 及以前建的库：有 sessions 表，但**没有** todos / source 列。
        c.execute_batch(
            "CREATE TABLE sessions (
                 id TEXT PRIMARY KEY, pos INTEGER NOT NULL DEFAULT 0, title TEXT NOT NULL DEFAULT '',
                 created_at INTEGER NOT NULL DEFAULT 0, usage TEXT, steps TEXT);
             INSERT INTO sessions(id, pos, title, created_at) VALUES ('old', 0, '老会话', 1);",
        )
        .unwrap();

        init_schema(&c).unwrap();

        let back = read_all(&c).unwrap();
        assert_eq!(back.len(), 1, "迁移不能弄丢老会话");
        assert_eq!(back[0].id, "old");
        assert!(back[0].todos.is_none(), "老记录没有待办 → None");
        assert_eq!(back[0].source, "", "老记录没有来源 → 空串（普通聊天）");
        assert_eq!(
            back[0].persona, "",
            "老记录没有署名 → 空串（前端回落当前名）"
        );
        assert_eq!(
            back[0].updated_at, 1,
            "老记录没有 updated_at ⇒ 回落 created_at，历史仍可排序"
        );
        assert_eq!(back[0].status, "", "老记录没有状态 → 空串（未知）");
    }

    /// `updated_at` / `status`（2026-11-05）：内容不变不推进时间；内容变了推进；
    /// 前端显式给的状态优先；没给时新内容记 `done`。
    #[test]
    fn updated_at_advances_only_when_content_changes() {
        let c = mem();
        let mut s = session("s1", "标题", "问", "答");
        s.status = "interrupted".into();
        write_all(&c, &[s]).unwrap();
        let first = read_all(&c).unwrap().remove(0);
        let t1 = first.updated_at;
        assert!(t1 > 0, "新会话必须有 updated_at");
        assert_eq!(first.status, "interrupted");

        // 同一内容再存一次（前端没传 updated_at / status）⇒ 沿用旧值
        let again = session("s1", "标题", "问", "答");
        write_all(&c, &[again]).unwrap();
        let same = read_all(&c).unwrap().remove(0);
        assert_eq!(same.updated_at, t1, "内容没变时 updated_at 不得推进");
        assert_eq!(
            same.status, "interrupted",
            "内容没变时 status 不得被覆盖"
        );

        // 内容变化 ⇒ 推进，且状态缺省记 done
        std::thread::sleep(std::time::Duration::from_millis(5));
        let changed = session("s1", "标题", "问", "答（改了）");
        write_all(&c, &[changed]).unwrap();
        let moved = read_all(&c).unwrap().remove(0);
        assert!(
            moved.updated_at > t1,
            "内容变化后 updated_at 必须推进，实际 {} <= {}",
            moved.updated_at,
            t1
        );
        assert_eq!(moved.status, "done", "新内容且未给状态 ⇒ done");
    }

    /// 人格署名（2026-10-24）：写进去什么、读回来就是什么；
    /// 且同一会话被覆盖写时由前端传入的值决定（不在存储层猜）。
    #[test]
    fn persona_survives_roundtrip_and_overwrite() {
        let c = mem();
        let mut s = session("p1", "标题", "问", "答");
        s.persona = "严谨工程师".into();
        write_all(&c, &[s]).unwrap();
        assert_eq!(read_all(&c).unwrap()[0].persona, "严谨工程师");

        let mut s2 = session("p1", "标题", "问", "答");
        s2.persona = "幽默助手".into();
        write_all(&c, &[s2]).unwrap();
        assert_eq!(read_all(&c).unwrap()[0].persona, "幽默助手");
    }

    /// 覆盖写 = 上一次的内容必须彻底消失（含 FTS 索引里的行）——
    /// 否则「已删掉的会话还能被搜到」，是典型的索引与数据不同步故障。
    #[test]
    fn overwrite_clears_old_rows_and_index() {
        let c = mem();
        write_all(&c, &[session("old", "旧", "旧问题", "旧回答")]).unwrap();
        write_all(&c, &[session("new", "新", "新问题", "新回答")]).unwrap();

        let sessions: i64 = c
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 1);
        let idx: i64 = c
            .query_row("SELECT COUNT(*) FROM messages_trgm", [], |r| r.get(0))
            .unwrap();
        assert_eq!(idx, 2, "旧会话的 2 条消息索引必须被触发器删掉");
        assert_eq!(read_all(&c).unwrap()[0].id, "new");
    }

    /// 导入旧 JSON：解析失败必须报错且**不动库**（库里已有数据时不能被清空）。
    #[test]
    fn import_legacy_rejects_bad_json_without_touching_data() {
        let dir = std::env::temp_dir().join(format!("lunac-chat-db-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("chat.db");
        let _ = std::fs::remove_file(&path);

        assert!(import_legacy(&path, "{ not json").is_err());
        assert!(load(&path).unwrap().is_empty(), "坏 JSON 不该写入任何数据");

        let n = import_legacy(
            &path,
            r#"[{"id":"x","title":"t","createdAt":1,"messages":[{"role":"user","content":"hi"}]}]"#,
        )
        .unwrap();
        assert_eq!(n, 1);
        assert_eq!(load(&path).unwrap()[0].id, "x");

        // 再导入一次坏 JSON：已有数据保持不变
        assert!(import_legacy(&path, "oops").is_err());
        assert_eq!(load(&path).unwrap().len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    // ── 往期会话检索（A2）─────────────────────────────────────────

    /// 指定 `created_at` 的会话（默认 helper 固定写 123，索引测试要控制先后）。
    fn at(id: &str, title: &str, created_at: u64, user: &str, asst: &str) -> ChatSession {
        let mut s = session(id, title, user, asst);
        s.created_at = created_at;
        s
    }

    /// 每个测试一个独立库文件（`search` 的入参是路径）。
    fn tmp_db(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lunac-chatdb-{}-{tag}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("chat.db");
        let _ = std::fs::remove_file(&p);
        p
    }

    /// ≥3 字符的中文子串走 trigram；英文 / 代码词走 unicode61。
    #[test]
    fn search_finds_cjk_substring_and_ascii_word() {
        let p = tmp_db("cjk");
        save(
            &p,
            &[session(
                "s1",
                "标题",
                "帮我看看缓存命中率",
                "cache hit rate",
            )],
        )
        .unwrap();

        let hits = search(&p, "缓存命中", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].role, "user");
        assert_eq!(hits[0].session_id, "s1");

        assert_eq!(search(&p, "rate", 10).unwrap().len(), 1);
        let _ = std::fs::remove_file(&p);
    }

    /// **关键回归**：2 字中文查询必须命中。
    ///
    /// trigram 分词器要求查询词 ≥3 字符 ⇒ 只走 FTS 时「缓存」这种 2 字词**永远 0 条**
    /// （不报错、静默失效）。而 2 字词恰恰是中文里最典型的查询形态，所以 `search()`
    /// 必须保留 `LIKE` 兜底。这条测试就是钉住那个兜底。
    #[test]
    fn search_falls_back_to_like_for_short_cjk_query() {
        let p = tmp_db("short");
        save(&p, &[session("s1", "标题", "帮我看看缓存命中率", "ok")]).unwrap();
        let hits = search(&p, "缓存", 10).unwrap();
        assert_eq!(hits.len(), 1, "2 字中文查询被漏掉 = 兜底路径失效");
        let _ = std::fs::remove_file(&p);
    }

    /// 覆盖写之后，已删除会话的内容不能再被搜到（索引与数据同步）。
    #[test]
    fn search_does_not_resurrect_deleted_sessions() {
        let p = tmp_db("sync");
        save(
            &p,
            &[session("old", "旧", "独一无二的关键字甲乙丙", "旧回答")],
        )
        .unwrap();
        assert_eq!(search(&p, "关键字甲乙丙", 10).unwrap().len(), 1);

        save(&p, &[session("new", "新", "换了一条会话", "新回答")]).unwrap();
        assert_eq!(
            search(&p, "关键字甲乙丙", 10).unwrap().len(),
            0,
            "旧会话的索引行没被触发器清掉"
        );
        let _ = std::fs::remove_file(&p);
    }

    /// LIKE 兜底路径必须转义通配符：不转义的话 `%` 会变成「匹配任意结尾」。
    #[test]
    fn search_treats_like_wildcards_literally() {
        let p = tmp_db("wild");
        save(
            &p,
            &[
                at("a", "A", 20, "进度 100% 完成", "x"),
                at("b", "B", 10, "进度 1000 完成", "y"),
            ],
        )
        .unwrap();
        let hits = search(&p, "%", 10).unwrap();
        assert_eq!(
            hits.len(),
            1,
            "% 必须按字面匹配（只有带百分号的那条算命中）"
        );
        assert_eq!(hits[0].session_id, "a");
        let _ = std::fs::remove_file(&p);
    }

    /// 检索结果按「会话新→旧」排序，且会话内保持消息顺序。
    #[test]
    fn search_orders_sessions_newest_first() {
        let p = tmp_db("order");
        save(
            &p,
            &[
                at("old", "旧", 100, "热键不起作用", "a"),
                at("new", "新", 200, "热键又坏了", "b"),
            ],
        )
        .unwrap();
        let hits = search(&p, "热键", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].session_id, "new");
        assert_eq!(hits[1].session_id, "old");
        let _ = std::fs::remove_file(&p);
    }
}
