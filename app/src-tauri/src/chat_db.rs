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
//! 对外只暴露 `load` / `save` / `import_legacy`，数据形状（`ChatSession`）留在
//! `storage.rs` —— 本模块不定义业务类型，免得出现两份「会话长什么样」的真相。

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
}
