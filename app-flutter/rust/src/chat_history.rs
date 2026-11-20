//! 对话历史（v2 自管）：`<exe 根>/ModuleData/history/chat-history.v2.json`。
//!
//! 为什么先不上 SQLite：v1 用 `chat.db`（rusqlite + FTS5）承载历史检索；v2 当前只需要
//! 「列出 / 恢复 / 删除」三件事，JSON 文件足够且零新依赖。会话形状与 v1 的 `ChatSession`
//! 同名同义（`id / title / createdAt / updatedAt / messages[{role,content}]`），并原样
//! 透传 `steps`（过程快照：思考块 / 工具卡 / 已修改的文件，见 `ChatModel.stepsJson`）——
//! 宿主不解析它，只保证不丢；M6 做数据迁移时可以整体导入。
//!
//! 纪律：
//!   · 一次 upsert 一条会话（按 `id` 覆盖），按 `updatedAt` 降序，最多保留 100 条；
//!   · 单条消息截断到 20000 字、每会话最多 200 条（防无界增长）；
//!   · 写盘用临时文件 + rename，写坏不会毁掉整份历史。

use std::path::{Path, PathBuf};

/// 最多保留的会话数。
const MAX_SESSIONS: usize = 100;
/// 每个会话最多保留的消息数（超出丢最旧的）。
const MAX_MESSAGES_PER_SESSION: usize = 200;
/// 单条消息最多保留的字符数。
const MAX_MESSAGE_CHARS: usize = 20_000;

/// 历史文件路径。
pub fn history_path(root: &Path) -> PathBuf {
    root.join("ModuleData")
        .join("history")
        .join("chat-history.v2.json")
}

/// 读全部会话（JSON 数组字符串；文件不存在 / 读坏 = `[]`）。
///
/// 顺手把 JSON 补进 `library.db` 的四层索引（日期 → 对话 → 任务 → 操作）：
/// 旧版本没有索引库时，第一次打开历史即可检索；同一 `updatedAt` 不会重复重建。
pub fn load_json(root: &Path) -> String {
    let sessions = read_sessions(root);
    if let Err(reason) = crate::library::sync_sessions(root, &sessions) {
        eprintln!("[lunac-history] 同步检索索引失败: {reason}");
    }
    serde_json::to_string(&serde_json::Value::Array(sessions))
        .unwrap_or_else(|_| "[]".to_string())
}

/// 写入 / 覆盖一条会话（按 `id`）。
pub fn upsert(root: &Path, session: &serde_json::Value) -> Result<(), String> {
    let Some(object) = session.as_object() else {
        return Err("会话必须是 JSON 对象".to_string());
    };
    let Some(id) = object
        .get("id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
    else {
        return Err("会话缺少 id".to_string());
    };
    let mut sessions = read_sessions(root);
    sessions.retain(|item| item.get("id").and_then(|value| value.as_str()) != Some(id));
    let mut session = session.clone();
    trim_session(&mut session);
    sessions.push(session);
    sessions.sort_by_key(|item| {
        std::cmp::Reverse(item.get("updatedAt").and_then(|value| value.as_u64()).unwrap_or(0))
    });
    sessions.truncate(MAX_SESSIONS);
    let saved = sessions
        .iter()
        .find(|item| item.get("id").and_then(|value| value.as_str()) == Some(id))
        .cloned();
    write_sessions(root, &sessions)?;
    // 索引是正文的派生层：写失败不该让「历史已经保存成功」变成报错（检索会暂时退化）。
    if let Some(saved) = saved {
        if let Err(reason) = crate::library::index_session(root, &saved) {
            eprintln!("[lunac-history] 重建会话索引失败（{id}）: {reason}");
        }
    }
    Ok(())
}

/// 删除一条会话；不存在也算成功。
pub fn delete(root: &Path, id: &str) -> Result<(), String> {
    let mut sessions = read_sessions(root);
    let before = sessions.len();
    sessions.retain(|item| item.get("id").and_then(|value| value.as_str()) != Some(id));
    if sessions.len() == before {
        return Ok(());
    }
    write_sessions(root, &sessions)?;
    if let Err(reason) = crate::library::remove_session(root, id) {
        eprintln!("[lunac-history] 删除会话索引失败（{id}）: {reason}");
    }
    Ok(())
}

fn read_sessions(root: &Path) -> Vec<serde_json::Value> {
    let Ok(text) = std::fs::read_to_string(history_path(root)) else {
        return Vec::new();
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Array(items)) => {
            items.into_iter().filter(|item| item.is_object()).collect()
        }
        _ => Vec::new(),
    }
}

fn write_sessions(root: &Path, sessions: &[serde_json::Value]) -> Result<(), String> {
    let path = history_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建历史目录失败（{}）: {e}", parent.display()))?;
    }
    let text = serde_json::to_string(sessions)
        .map_err(|e| format!("序列化对话历史失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)
        .map_err(|e| format!("写对话历史失败（{}）: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("替换对话历史失败（{}）: {e}", path.display()))?;
    Ok(())
}

/// 往期会话检索（`SessionSearch` 的后端）。
///
/// 2026-11-20 起不再逐条扫 JSON：改为查 `library.db` 的四层索引
/// （日期 → 对话 → 任务 → 操作），只返回命中的操作。模型可以按 `tags` / `date` /
/// `session` / `task` 收窄范围，不必把整段历史重新读进上下文。
#[cfg(test)]
pub fn search_text(root: &Path, query: &str, limit: usize) -> String {
    search_text_filtered(root, query, &[], None, None, None, limit)
}

/// 带过滤条件的检索（字段与 `SessionSearch` 的可选参数一一对应）。
#[allow(clippy::too_many_arguments)]
pub fn search_text_filtered(
    root: &Path,
    query: &str,
    tags: &[String],
    date: Option<&str>,
    session: Option<&str>,
    task: Option<&str>,
    limit: usize,
) -> String {
    if query.trim().is_empty() && tags.is_empty() {
        return "No past message matched \"\".\n\
                (Searched this app's own chat history only — not files on disk.)"
            .to_string();
    }
    let limit = limit.clamp(1, 30);
    let request = crate::library::Query {
        text: query,
        tags,
        date,
        session,
        task,
        limit,
        scope: crate::library::CHAT_SCOPE,
    };
    let hits = match crate::library::search(root, &request) {
        Ok(hits) => hits,
        Err(reason) => {
            return format!(
                "Past-session search is unavailable: {reason}\n\
                 (Searched this app's own chat history only — not files on disk.)"
            );
        }
    };
    if hits.is_empty() {
        let scope = if date.is_some() || session.is_some() || task.is_some() {
            " with the given filters"
        } else {
            ""
        };
        return format!(
            "No past message matched \"{query}\"{scope}.\n\
             (Searched this app's own chat history only — not files on disk.)"
        );
    }
    render_hits(query, &hits)
}

/// 把命中按「日期 → 对话 → 任务」分组，操作只留命中点附近的片段。
fn render_hits(query: &str, hits: &[crate::library::Hit]) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut out = if query.trim().is_empty() {
        format!(
            "{} recent operation(s) from the local library (newest first).\n\
             Library index: date → conversation → task → operation. Only the listed \
             operations are returned; nothing else is added to context.",
            hits.len()
        )
    } else {
        format!(
            "{} matching operation(s) for \"{query}\" in past conversations (newest first).\n\
             Library index: date → conversation → task → operation. Only the listed \
             operations are returned; re-run with narrower tags / date / session / task \
             if you need more.",
            hits.len()
        )
    };
    let mut groups: Vec<(String, String, Vec<String>)> = Vec::new();
    for hit in hits {
        let key = format!(
            "{}\u{1f}{}\u{1f}{}",
            hit.date, hit.session_id, hit.task_id
        );
        let position = groups.iter().position(|(existing, _, _)| existing == &key);
        let index = match position {
            Some(index) => index,
            None => {
                let age = now.saturating_sub(hit.created_at) / 86_400_000;
                let age = match age {
                    0 => "today".to_string(),
                    1 => "1d ago".to_string(),
                    n => format!("{n}d ago"),
                };
                let session_title = if hit.session_title.trim().is_empty() {
                    "(untitled)"
                } else {
                    hit.session_title.trim()
                };
                groups.push((
                    key,
                    format!("## {} · {}  [{age}]", hit.date, session_title),
                    Vec::new(),
                ));
                groups.len() - 1
            }
        };
        let lines = &mut groups[index].2;
        if !lines.iter().any(|line| line.starts_with("### ")) {
            let tags = if hit.tags.is_empty() {
                String::new()
            } else {
                format!(
                    "  [tags: {}]",
                    hit.tags.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
                )
            };
            lines.push(format!("### {}{tags}", hit.task_title));
        }
        let needle = query
            .split_whitespace()
            .next()
            .map(str::to_lowercase)
            .unwrap_or_default();
        let text = if needle.is_empty() {
            snippet(&hit.text, 0, 0)
        } else {
            let lower = hit.text.to_lowercase();
            match lower.find(&needle) {
                Some(byte_pos) => {
                    snippet(&hit.text, lower[..byte_pos].chars().count(), needle.chars().count())
                }
                None => snippet(&hit.text, 0, 0),
            }
        };
        lines.push(format!("- {}: {text}", hit.role));
    }
    for (_, header, lines) in groups {
        out.push_str("\n\n");
        out.push_str(&header);
        out.push('\n');
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// 命中点附近的片段（按**字符**切，最多 200 字；前后各留一点上下文）。
fn snippet(content: &str, start_char: usize, needle_chars: usize) -> String {
    let chars: Vec<char> = content.chars().collect();
    let start = start_char.saturating_sub(60);
    let end = (start_char + needle_chars + 140).min(chars.len());
    let mut text: String = chars[start..end].iter().collect();
    text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > 200 {
        text = text.chars().take(200).collect();
    }
    if start > 0 {
        text.insert_str(0, "…");
    }
    if end < chars.len() {
        text.push('…');
    }
    text
}

fn trim_session(session: &mut serde_json::Value) {
    let Some(messages) = session
        .get_mut("messages")
        .and_then(|value| value.as_array_mut())
    else {
        return;
    };
    for message in messages.iter_mut() {
        let Some(content) = message.get("content").and_then(|value| value.as_str()) else {
            continue;
        };
        if content.chars().count() > MAX_MESSAGE_CHARS {
            let kept: String = content.chars().take(MAX_MESSAGE_CHARS).collect();
            message["content"] = serde_json::Value::String(format!("{kept}…"));
        }
    }
    let overflow = messages.len().saturating_sub(MAX_MESSAGES_PER_SESSION);
    if overflow > 0 {
        // 2026-11-20 用户报障：历史回写后「用户的问题会消失」。根因是长会话只留最后
        // 200 条，把**最早那条提问**一起 drain 掉了。提问是整段任务的锚点，不能丢：
        // 裁剪时单独把它留下（仍保持 200 条上限），其余按原顺序取最近的。
        let first_user = messages
            .iter()
            .position(|message| message.get("role").and_then(|v| v.as_str()) == Some("user"))
            .filter(|index| *index < overflow)
            .map(|index| messages[index].clone());
        messages.drain(0..overflow);
        if let Some(question) = first_user {
            // 插回开头后条数会多 1：丢掉紧接着的那条（最旧的保留项），
            // 保持 200 条上限，同时不牺牲最新的回复。
            messages.insert(0, question);
            messages.remove(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lunac-history-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    fn session(id: &str, updated: u64) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "title": format!("title-{id}"),
            "createdAt": 1,
            "updatedAt": updated,
            "messages": [
                {"role": "user", "content": "你好"},
                {"role": "assistant", "content": "你好呀"},
            ],
        })
    }

    #[test]
    fn upsert_load_and_delete_round_trip() {
        let root = temp_root("roundtrip");
        upsert(&root, &session("a", 10)).unwrap();
        upsert(&root, &session("b", 20)).unwrap();

        let loaded: serde_json::Value = serde_json::from_str(&load_json(&root)).unwrap();
        let items = loaded.as_array().unwrap();
        assert_eq!(items.len(), 2);
        // 按 updatedAt 降序：最近活动的 b 在前。
        assert_eq!(items[0]["id"], "b");
        assert_eq!(items[1]["id"], "a");

        // 覆盖同一 id 不会变成两条。
        let mut again = session("a", 30);
        again["title"] = serde_json::json!("updated");
        upsert(&root, &again).unwrap();
        let loaded: serde_json::Value = serde_json::from_str(&load_json(&root)).unwrap();
        let items = loaded.as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], "a");
        assert_eq!(items[0]["title"], "updated");

        delete(&root, "a").unwrap();
        let loaded: serde_json::Value = serde_json::from_str(&load_json(&root)).unwrap();
        assert_eq!(loaded.as_array().unwrap().len(), 1);
        // 删除不存在的 id 是 no-op。
        delete(&root, "missing").unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn invalid_sessions_are_rejected_and_file_survives() {
        let root = temp_root("invalid");
        upsert(&root, &session("a", 10)).unwrap();
        assert!(upsert(&root, &serde_json::json!({"title": "no id"})).is_err());
        assert!(upsert(&root, &serde_json::json!([1, 2])).is_err());
        let loaded: serde_json::Value = serde_json::from_str(&load_json(&root)).unwrap();
        assert_eq!(loaded.as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn process_steps_are_passed_through_untouched() {
        // 过程快照由前端生成，宿主不解析：upsert → load 必须逐字段不丢。
        let root = temp_root("steps");
        let mut value = session("a", 10);
        value["steps"] = serde_json::json!([
            {"kind": "thinking", "text": "先看文件"},
            {"kind": "toolUse", "name": "Edit", "id": "t1",
             "detail": r#"{"file_path":"D:\\cc\\a.dart"}"#},
            {"kind": "toolResult", "id": "t1", "text": "ok"},
        ]);
        upsert(&root, &value).unwrap();

        let loaded: serde_json::Value = serde_json::from_str(&load_json(&root)).unwrap();
        let steps = loaded[0]["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[1]["name"], "Edit");
        assert_eq!(steps[2]["text"], "ok");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn first_user_question_survives_message_trim() {
        let root = temp_root("keep-first-question");
        let mut messages = vec![serde_json::json!({
            "role": "user",
            "content": "最早的问题：设置窗口拖不动",
        })];
        for index in 0..(MAX_MESSAGES_PER_SESSION + 4) {
            messages.push(serde_json::json!({
                "role": if index % 2 == 0 { "assistant" } else { "user" },
                "content": format!("第 {index} 条"),
            }));
        }
        let value = serde_json::json!({
            "id": "keep-first",
            "updatedAt": 1,
            "messages": messages,
        });
        upsert(&root, &value).unwrap();
        let loaded: serde_json::Value = serde_json::from_str(&load_json(&root)).unwrap();
        let messages = loaded[0]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), MAX_MESSAGES_PER_SESSION);
        assert_eq!(messages[0]["content"], "最早的问题：设置窗口拖不动");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn search_uses_the_library_hierarchy_and_content_titles() {
        let root = temp_root("search-library");
        let value = serde_json::json!({
            "id": "s1",
            "title": "设置窗口拖动",
            "createdAt": 1_700_000_000_000u64,
            "updatedAt": 1_700_000_000_000u64,
            "messages": [
                {"role": "user", "content": "设置界面打开时标题栏拖不动"},
                {"role": "assistant", "content": "修好了"}
            ],
            "steps": [
                {"kind": "user", "text": "设置界面打开时标题栏拖不动"},
                {"kind": "thinking", "text": "先看搜索页的拖拽层"},
                {"kind": "toolUse", "name": "Edit", "id": "t1",
                 "detail": r#"{"file_path":"D:\\cc\\a.dart"}"#},
                {"kind": "toolResult", "id": "t1", "text": "ok"},
                {"kind": "assistant", "text": "修好了"}
            ],
        });
        upsert(&root, &value).unwrap();
        let text = search_text(&root, "拖不动", 10);
        assert!(text.contains("设置界面打开时标题栏拖不动"));
        assert!(text.contains("### "));
        assert!(!text.contains("任务 1"), "标题不得再用任务编号：{text}");
        // 标签过滤：同一套库、同一套四层路径。
        let tags = crate::library::extract_tags("设置界面打开时标题栏拖不动");
        let filtered = search_text_filtered(&root, "", &tags, None, None, None, 10);
        assert!(filtered.contains("设置界面打开时标题栏拖不动"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn long_messages_and_sessions_are_trimmed() {
        let root = temp_root("trim");
        let messages: Vec<serde_json::Value> = (0..(MAX_MESSAGES_PER_SESSION + 5))
            .map(|index| {
                serde_json::json!({
                    "role": "user",
                    "content": format!("{index}-{}", "x".repeat(10)),
                })
            })
            .collect();
        let long: String = "y".repeat(MAX_MESSAGE_CHARS + 100);
        let mut value = serde_json::json!({
            "id": "long",
            "updatedAt": 1,
            "messages": messages,
        });
        value["messages"].as_array_mut().unwrap().push(serde_json::json!({
            "role": "assistant",
            "content": long,
        }));
        upsert(&root, &value).unwrap();

        let loaded: serde_json::Value = serde_json::from_str(&load_json(&root)).unwrap();
        let messages = loaded[0]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), MAX_MESSAGES_PER_SESSION);
        let last = messages.last().unwrap()["content"].as_str().unwrap();
        assert!(last.ends_with('…'));
        let _ = std::fs::remove_dir_all(&root);
    }
}
