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
//! `search` / `recent_sessions` / `render_digest`（往期会话检索，2026-09-19）——
//! 数据形状（`ChatSession`）留在 `storage.rs` —— 本模块不定义业务类型，
//! 免得出现两份「会话长什么样」的真相。

use std::collections::HashMap;
use std::path::Path;

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
            usage      TEXT,
            steps      TEXT
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
    .map_err(|e| format!("初始化会话库失败: {e}"))
}

/// 全量写入：**在一个事务里**删掉所有旧数据，再按入参顺序插入。
///
/// 语义与旧的「整体重写 JSON」严格一致（前端传的是它内存里的**完整**列表，含被删掉的
/// 会话），所以这里不做差异比对 —— 那样得在存储层猜前端的「哪条被删了」语义，更脆。
/// 事务保证中途失败不会留下「会话还在、消息没了」的半截状态。
fn write_all(conn: &Connection, sessions: &[ChatSession]) -> Result<(), String> {
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
                "INSERT INTO sessions(id, pos, title, created_at, usage, steps)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .map_err(|e| e.to_string())?;
        let mut ins_m = conn
            .prepare("INSERT INTO messages(session_id, idx, role, content) VALUES (?1, ?2, ?3, ?4)")
            .map_err(|e| e.to_string())?;

        for (pos, s) in sessions.iter().enumerate() {
            // usage / steps 是可选结构 → 统一存 JSON 文本（None 写 NULL），读回来再按
            // Option 反序列化；不为这两个字段各开一堆列。
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
            ins_s
                .execute(rusqlite::params![
                    s.id,
                    pos as i64,
                    s.title,
                    s.created_at as i64,
                    usage,
                    steps
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
            .prepare("SELECT id, title, created_at, usage, steps FROM sessions ORDER BY pos ASC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                let usage: Option<String> = row.get(3)?;
                let steps: Option<String> = row.get(4)?;
                Ok(ChatSession {
                    id: row.get::<_, String>(0)?,
                    title: row.get::<_, String>(1)?,
                    created_at: row.get::<_, i64>(2)? as u64,
                    // 单条记录坏掉不该让整个历史列表读不出来 ⇒ 解析失败按 None 处理。
                    usage: usage.and_then(|s| serde_json::from_str(&s).ok()),
                    steps: steps.and_then(|s| serde_json::from_str(&s).ok()),
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
            .prepare("SELECT session_id, role, content FROM messages ORDER BY session_id ASC, idx ASC")
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

/// 一条会话的摘要（供「往期会话索引」注入用）。
pub struct SessionBrief {
    pub id: String,
    pub title: String,
    pub created_at: u64,
    pub msgs: usize,
    /// 首条 user 消息（已截断）—— 标题常是自动生成的泛泛之词，
    /// 「这条会话到底在问什么」看开头第一句最准。
    pub opening: String,
}

/// 取最近 N 条**非空**会话的摘要，按创建时间新→旧。
pub fn recent_sessions(path: &Path, limit: usize) -> Result<Vec<SessionBrief>, String> {
    let conn = open(path)?;
    init_schema(&conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.title, s.created_at,
                    (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id),
                    COALESCE((SELECT substr(m2.content, 1, 300) FROM messages m2
                              WHERE m2.session_id = s.id AND m2.role = 'user'
                              ORDER BY m2.idx ASC LIMIT 1), '')
             FROM sessions s
             ORDER BY s.created_at DESC, s.pos ASC
             LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![limit as i64], |row| {
            Ok(SessionBrief {
                id: row.get(0)?,
                title: row.get(1)?,
                created_at: row.get::<_, i64>(2)? as u64,
                msgs: row.get::<_, i64>(3)?.max(0) as usize,
                opening: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        let b = r.map_err(|e| e.to_string())?;
        // 空会话（只有标题、没有消息）对模型没有任何价值，纯占前缀预算。
        if b.msgs > 0 {
            out.push(b);
        }
    }
    Ok(out)
}

/// 单行里标题 / 开场白的字符上限。
const DIGEST_TITLE_CHARS: usize = 60;
const DIGEST_OPENING_CHARS: usize = 120;

/// 把会话摘要渲染成**系统提示词里的固定段**。
///
/// 三条纪律：
/// ① **英文**：与 `env_block` / 技能清单位于同一段，模型看到的身份与环境说明都是英文；
/// ② **日期用相对天数**：Rust 标准库没有时区表（项目为此不引 chrono，见 `log.rs`），
///    绝对日期只能是 UTC，会出现「本地已是今天、UTC 还是昨天」的错位；相对天数是纯差值，
///    与本地时区无关 —— 而「上次 / 前几天」这类指代要的正是相对先后；
/// ③ **总预算硬截断**：这段进的是**每轮都发**的固定前缀，必须封顶（预算由调用方给）。
pub fn render_digest(briefs: &[SessionBrief], budget_chars: usize, now_ms: u64) -> String {
    if briefs.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "## Past sessions (newest first)\n\
         These are the user's earlier conversations in this app. This list carries only titles \
         and opening lines — you do NOT know what was actually discussed. Call the \
         `SessionSearch` tool to read a session's real content, and never claim to remember \
         something you have not read.\n",
    );
    let mut omitted = 0usize;
    for b in briefs {
        let line = format!(
            "\n- [{}] {} ({} msgs) — {}",
            age_label(b.created_at, now_ms),
            clip(&b.title, DIGEST_TITLE_CHARS),
            b.msgs,
            clip(&b.opening, DIGEST_OPENING_CHARS)
        );
        if out.chars().count() + line.chars().count() > budget_chars {
            omitted += 1;
            continue;
        }
        out.push_str(&line);
    }
    if omitted > 0 {
        out.push_str(&format!(
            "\n\n({omitted} older session(s) omitted — narrow the question and search instead.)"
        ));
    }
    out
}

/// 相对天数标签。`created_at` 是毫秒时间戳（前端 `Date.now()` 口径）。
fn age_label(created_at: u64, now_ms: u64) -> String {
    let days = now_ms.saturating_sub(created_at) / 86_400_000;
    match days {
        0 => "today".into(),
        1 => "1d ago".into(),
        n => format!("{n}d ago"),
    }
}

/// 按**字符**（不是字节）截断并加省略号 —— 中文按字节截会切出半个字。
fn clip(s: &str, max_chars: usize) -> String {
    // 标题 / 开场白里的换行会把「一行一条」的列表结构打散，先压成单行。
    let flat: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    if flat.chars().count() <= max_chars {
        return flat.to_string();
    }
    let head: String = flat.chars().take(max_chars).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SessionUsage;

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
            usage: Some(SessionUsage {
                hit: 7,
                ..Default::default()
            }),
            steps: None,
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
        write_all(&c, &[session("s1", "标题", "帮我看看缓存命中率", "cache hit rate is low")]).unwrap();

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

        let n = import_legacy(&path, r#"[{"id":"x","title":"t","createdAt":1,"messages":[{"role":"user","content":"hi"}]}]"#).unwrap();
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

    /// 每个测试一个独立库文件（`search` / `recent_sessions` 的入参是路径）。
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
        save(&p, &[session("s1", "标题", "帮我看看缓存命中率", "cache hit rate")]).unwrap();

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
        save(&p, &[session("old", "旧", "独一无二的关键字甲乙丙", "旧回答")]).unwrap();
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
        assert_eq!(hits.len(), 1, "% 必须按字面匹配（只有带百分号的那条算命中）");
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

    /// 空会话不进索引（没有信息量，纯占固定前缀预算）。
    #[test]
    fn digest_skips_empty_sessions_and_labels_relative_days() {
        let p = tmp_db("digest");
        let mut empty = session("e", "空会话", "x", "y");
        empty.messages.clear();
        save(&p, &[empty, at("n", "有内容", 0, "开场白第一句", "回答")]).unwrap();

        let briefs = recent_sessions(&p, 10).unwrap();
        assert_eq!(briefs.len(), 1);
        assert_eq!(briefs[0].id, "n");
        assert_eq!(briefs[0].msgs, 2);

        let text = render_digest(&briefs, 4000, 86_400_000);
        assert!(text.contains("[1d ago] 有内容 (2 msgs) — 开场白第一句"), "实际: {text}");
        let _ = std::fs::remove_file(&p);
    }

    /// 固定前缀里的这段必须**封顶**，并把被截掉的行数如实写出来。
    #[test]
    fn digest_respects_budget() {
        let briefs: Vec<SessionBrief> = (0..50)
            .map(|i| SessionBrief {
                id: format!("s{i}"),
                title: format!("会话{i}"),
                created_at: 0,
                msgs: 2,
                opening: "开场白".into(),
            })
            .collect();

        let text = render_digest(&briefs, 600, 0);
        assert!(text.chars().count() <= 700, "必须封顶在预算附近，实际 {}", text.chars().count());
        assert!(text.contains("older session(s) omitted"), "被截掉的行数要如实说明");

        let full = render_digest(&briefs[..1], 4000, 0);
        assert!(!full.contains("omitted"), "没截断就不该出现省略提示");
    }

    /// 超长标题 / 带换行的开场白：按字符截断、换行压成空格（不能打散「一行一条」）。
    #[test]
    fn digest_clips_long_fields_and_flattens_newlines() {
        let b = SessionBrief {
            id: "s".into(),
            title: "标题".repeat(40),
            created_at: 0,
            msgs: 3,
            opening: "第一行\n第二行".into(),
        };
        let text = render_digest(std::slice::from_ref(&b), 4000, 5 * 86_400_000);
        assert!(text.contains("[5d ago]"));
        assert!(text.contains('…'), "超长标题必须截断");
        assert!(text.contains("— 第一行 第二行"), "开场白的换行要压成空格");
        assert_eq!(text.matches("\n- ").count(), 1, "一条会话只能占一行");
    }
}
