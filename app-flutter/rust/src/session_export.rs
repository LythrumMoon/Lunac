//! 会话轨迹导出（`docs/todo.md` §待办 3-28.12）。
//!
//! 一份会话 = 消息列表（`messages`）+ 过程快照（`ChatModel.stepsJson()` 的 `steps`），
//! 这里把它拍平成 **NDJSON 事件流**：一行一个事件，靠 `type` 区分
//! `session` / `system` / `context` / `message` / `step` / `turn`。与「查看过程」渲染出的
//! 步骤**同一份数据**，但保留来源字段（user / assistant / tool / thinking），可以直接
//! `jq` / `grep`。
//!
//! SYSTEM / CONTEXT 两栏（§待办 3-28.12 ①）：新会话按 `promptTimeline` **逐轮**导出
//! （每轮一条 `system` + 一条 `context`，带 `turn`）；老会话只有最新一次快照，保持原样。
//!
//! 纪律（与账本同口径）：
//!   · 只读一份已经在内存 / 已落盘的会话，**不发任何请求**（导出 ≠ 重放）；
//!   · 每个字符串进盘前过 `security_ledger::mask_secrets()` —— 脱敏规则只有一处；
//!   · 单字段截到 4000 字符并带 `truncated`，单次最多 2000 行、预览最多 256 KiB；
//!   · 落点 `<exe 根>\ModuleData\exports\session-<id>-<UTC>.jsonl`（临时文件 + rename）。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// 单次导出最多多少行（一份 200 条消息 + 若干步骤的会话远用不到）。
const MAX_LINES: usize = 2_000;
/// 单个字段最多多少**字符**（不是字节；截断后带标记）。
const MAX_FIELD_CHARS: usize = 4_000;
/// 「轨迹」预览的最大字节数（界面里渲染整段文本，不能无界）。
const MAX_PREVIEW_BYTES: usize = 256 * 1024;

fn export_dir(root: &Path) -> PathBuf {
    root.join("ModuleData").join("exports")
}

/// 只保留 `[A-Za-z0-9_-]`，其余一律换成 `_`（防路径拼串）。
fn safe_id(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .take(64)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.trim_matches('_').is_empty() {
        "session".to_string()
    } else {
        cleaned
    }
}

/// 脱敏 + 截断（顺序固定：先脱敏再截断，截断标记里的长度是原文长度）。
fn field(text: &str) -> (String, bool) {
    let masked = crate::security_ledger::mask_secrets(text);
    let len = masked.chars().count();
    if len <= MAX_FIELD_CHARS {
        return (masked, false);
    }
    let kept: String = masked.chars().take(MAX_FIELD_CHARS).collect();
    (format!("{kept}…(truncated: {len} chars)"), true)
}

fn opt_str(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// 把一条会话拍平成事件行；`Ok((lines, truncated))`。
fn build_lines(session: &Value) -> Result<(Vec<String>, bool), String> {
    let object = session
        .as_object()
        .ok_or_else(|| "会话必须是 JSON 对象".to_string())?;
    let id = object.get("id").and_then(Value::as_str).unwrap_or_default();
    if id.trim().is_empty() {
        return Err("会话缺少 id".to_string());
    }
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let steps = object
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let turns = object
        .get("turnDurations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if messages.is_empty() && steps.is_empty() {
        return Err("这条会话还是空的：没有消息也没有过程快照".to_string());
    }

    let (exported_at, _) = crate::security_ledger::utc_now();
    let (title, _) = field(&opt_str(session, "title"));
    let mut lines: Vec<String> = Vec::new();
    let mut truncated = false;

    // ① 头：一眼看出这是哪条会话、由什么导出的。
    lines.push(
        json!({
            "v": 1,
            "type": "session",
            "id": id,
            "title": title,
            "createdAt": object.get("createdAt").cloned().unwrap_or(Value::Null),
            "updatedAt": object.get("updatedAt").cloned().unwrap_or(Value::Null),
            "exportedAt": exported_at,
            "messages": messages.len(),
            "steps": steps.len(),
        })
        .to_string(),
    );

    // ①bis SYSTEM / CONTEXT（§待办 3-28.12 ①）：优先按**逐轮时间线**导出（每轮一行）；
    // 老会话只有最新一次的 `system` / `context`，保持原样，不假造轮次。
    let timeline = object
        .get("promptTimeline")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !timeline.is_empty() {
        for entry in &timeline {
            let turn = entry.get("turn").and_then(Value::as_u64).unwrap_or(0);
            if let Some(system) = entry.get("system").filter(|value| value.is_object()) {
                let sha = opt_str(system, "sha256");
                if !sha.is_empty() {
                    lines.push(
                        json!({
                            "type": "system",
                            "turn": turn,
                            "chars": system.get("chars").and_then(Value::as_u64).unwrap_or(0),
                            "sha256": field(&sha).0,
                        })
                        .to_string(),
                    );
                }
            }
            if let Some(context) = entry.get("context").filter(|value| value.is_object()) {
                let sha = opt_str(context, "sha256");
                lines.push(
                    json!({
                        "type": "context",
                        "turn": turn,
                        "present": context.get("present").and_then(Value::as_bool).unwrap_or(false),
                        "chars": context.get("chars").and_then(Value::as_u64).unwrap_or(0),
                        "sha256": field(&sha).0,
                    })
                    .to_string(),
                );
            }
        }
    } else {
        if let Some(system) = object.get("system").filter(|value| value.is_object()) {
            let sha = opt_str(system, "sha256");
            if !sha.is_empty() {
                lines.push(
                    json!({
                        "type": "system",
                        "chars": system.get("chars").and_then(Value::as_u64).unwrap_or(0),
                        "sha256": field(&sha).0,
                    })
                    .to_string(),
                );
            }
        }
        if let Some(context) = object.get("context").filter(|value| value.is_object()) {
            let sha = opt_str(context, "sha256");
            lines.push(
                json!({
                    "type": "context",
                    "present": context.get("present").and_then(Value::as_bool).unwrap_or(true),
                    "chars": context.get("chars").and_then(Value::as_u64).unwrap_or(0),
                    "sha256": field(&sha).0,
                })
                .to_string(),
            );
        }
    }

    // ② 消息：宿主落盘的正文（`role` 只有 user / assistant）。
    for (index, message) in messages.iter().enumerate() {
        if lines.len() >= MAX_LINES {
            truncated = true;
            break;
        }
        let (text, clipped) = field(&opt_str(message, "content"));
        truncated |= clipped;
        lines.push(
            json!({
                "type": "message",
                "index": index,
                "role": opt_str(message, "role"),
                "text": text,
                "truncated": clipped,
            })
            .to_string(),
        );
    }

    // ③ 过程快照：思考块 / 工具卡 / 工具输出 / 文件改写 …（顺序即发生顺序）。
    for (index, step) in steps.iter().enumerate() {
        if lines.len() >= MAX_LINES {
            truncated = true;
            break;
        }
        let (detail, detail_clipped) = field(&opt_str(step, "detail"));
        let (text, text_clipped) = field(&opt_str(step, "text"));
        truncated |= detail_clipped || text_clipped;
        let mut line = json!({
            "type": "step",
            "index": index,
            "kind": opt_str(step, "kind"),
        });
        if let Some(map) = line.as_object_mut() {
            for (key, value) in [
                ("name", json!(opt_str(step, "name"))),
                ("id", json!(opt_str(step, "id"))),
                ("detail", json!(detail)),
                ("text", json!(text)),
            ] {
                let empty = value.as_str().map(str::is_empty).unwrap_or(true);
                if !empty {
                    map.insert(key.to_string(), value);
                }
            }
            if detail_clipped || text_clipped {
                map.insert("truncated".to_string(), json!(true));
            }
        }
        lines.push(line.to_string());
    }

    // ④ 每轮耗时（有就带上：复盘「哪一轮慢」用得上）。
    for (index, duration) in turns.iter().enumerate() {
        if lines.len() >= MAX_LINES {
            truncated = true;
            break;
        }
        lines.push(
            json!({
                "type": "turn",
                "index": index,
                "durationMs": duration,
            })
            .to_string(),
        );
    }

    if truncated {
        lines.push(
            json!({
                "type": "notice",
                "text": format!("…(truncated: 已到 {MAX_LINES} 行 / 单字段 {MAX_FIELD_CHARS} 字符上限)"),
            })
            .to_string(),
        );
    }
    Ok((lines, truncated))
}

/// 「轨迹」预览：只回事件行，不落盘（界面里只看不改）。
///
/// 返回 `{ ok, id, lines:[…], count, bytes, truncated, error }`。
pub fn trace_json(request: &Value) -> String {
    match build_lines(request) {
        Ok((lines, truncated)) => {
            // 预览有第二道上限：渲染整段文本，256 KiB 之外不再给。
            let mut kept: Vec<String> = Vec::new();
            let mut bytes = 0usize;
            let mut capped = truncated;
            for line in lines {
                if bytes + line.len() + 1 > MAX_PREVIEW_BYTES {
                    capped = true;
                    break;
                }
                bytes += line.len() + 1;
                kept.push(line);
            }
            json!({
                "ok": true,
                "id": opt_str(request, "id"),
                "lines": kept,
                "count": kept.len(),
                "bytes": bytes,
                "truncated": capped,
                "error": "",
            })
            .to_string()
        }
        Err(reason) => json!({
            "ok": false,
            "id": opt_str(request, "id"),
            "lines": [],
            "count": 0,
            "bytes": 0,
            "truncated": false,
            "error": reason,
        })
        .to_string(),
    }
}

/// 导出成 `.jsonl`：返回 `{ ok, path, count, bytes, truncated, error }`。
pub fn export_json(root: &Path, request: &Value) -> String {
    let (lines, truncated) = match build_lines(request) {
        Ok(value) => value,
        Err(reason) => return export_error(&reason),
    };
    let dir = export_dir(root);
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return export_error(&format!("建不了导出目录（{}）：{error}", dir.display()));
    }
    let (stamp, _) = crate::security_ledger::utc_now();
    let file_stamp = stamp.replace(':', "-");
    let path = dir.join(format!(
        "session-{}-{file_stamp}.jsonl",
        safe_id(&opt_str(request, "id"))
    ));
    let mut body = lines.join("\n");
    body.push('\n');
    let tmp = path.with_extension("jsonl.tmp");
    if let Err(error) = std::fs::write(&tmp, &body) {
        return export_error(&format!(
            "写不进 {}：{error}；请确认目录可写",
            tmp.display()
        ));
    }
    if let Err(error) = std::fs::rename(&tmp, &path) {
        return export_error(&format!("替换 {} 失败：{error}", path.display()));
    }
    json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "count": lines.len(),
        "bytes": body.len(),
        "truncated": truncated,
        "error": "",
    })
    .to_string()
}

fn export_error(reason: &str) -> String {
    json!({
        "ok": false,
        "path": "",
        "count": 0,
        "bytes": 0,
        "truncated": false,
        "error": reason,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lunac-session-export-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    fn session() -> Value {
        json!({
            "id": "sess-1",
            "title": "手柄图",
            "createdAt": 1_700_000_000_000u64,
            "updatedAt": 1_700_000_100_000u64,
            "messages": [
                { "role": "user", "content": "帮我看一下" },
                { "role": "assistant", "content": "好的" },
            ],
            "steps": [
                { "kind": "thinking", "text": "先读文件" },
                { "kind": "tool", "name": "Read", "id": "toolu_1", "detail": "a.rs", "text": "内容" },
            ],
            "turnDurations": [1_500, 800],
        })
    }

    #[test]
    fn a_session_becomes_one_line_per_event() {
        let (lines, truncated) = build_lines(&session()).expect("build");
        assert!(!truncated);
        assert_eq!(lines.len(), 1 + 2 + 2 + 2, "头 + 消息 + 步骤 + 回合耗时");
        let parsed: Vec<Value> = lines
            .iter()
            .map(|line| serde_json::from_str(line).expect("每行都是合法 JSON"))
            .collect();
        assert_eq!(parsed[0]["type"], "session");
        assert_eq!(parsed[0]["messages"], 2);
        assert_eq!(parsed[0]["steps"], 2);
        assert_eq!(parsed[1]["type"], "message");
        assert_eq!(parsed[1]["role"], "user");
        assert_eq!(parsed[2]["text"], "好的");
        assert_eq!(parsed[3]["kind"], "thinking");
        assert_eq!(parsed[4]["name"], "Read");
        assert_eq!(parsed[4]["id"], "toolu_1");
        assert_eq!(parsed[5]["type"], "turn");
        assert_eq!(parsed[5]["durationMs"], 1_500);
        assert!(parsed[0]["exportedAt"].as_str().unwrap().ends_with('Z'));
    }

    /// SYSTEM / CONTEXT 两栏（§待办 3-28.12 ①）：有就排在消息之前，只有哈希与字符数；
    /// 老会话没有这两项时行数与原来一致（不多出空行）。
    #[test]
    fn system_and_context_lines_precede_the_messages() {
        let mut value = session();
        value["system"] = json!({"chars": 1234, "sha256": "a".repeat(64)});
        value["context"] = json!({"present": true, "chars": 88, "sha256": "b".repeat(64)});
        let (lines, truncated) = build_lines(&value).expect("build");
        assert!(!truncated);
        let parsed: Vec<Value> = lines
            .iter()
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect();
        assert_eq!(parsed[1]["type"], "system");
        assert_eq!(parsed[1]["chars"], 1234);
        assert_eq!(parsed[1]["sha256"].as_str().unwrap().len(), 64);
        assert_eq!(parsed[2]["type"], "context");
        assert_eq!(parsed[2]["present"], true);
        assert_eq!(parsed[2]["chars"], 88);
        assert_eq!(parsed[3]["type"], "message");
        assert_eq!(
            build_lines(&session()).expect("build").0.len(),
            7,
            "没有 system / context 的会话不能被多塞行"
        );
    }

    /// 逐轮时间线（§待办 3-28.12 ①）：每轮导出一对 system / context 行（带 `turn`）；
    /// 同时存在的 `system` / `context` 只是最新一次的兼容字段，不重复导出。
    #[test]
    fn a_prompt_timeline_exports_one_pair_of_lines_per_turn() {
        let mut value = session();
        value["promptTimeline"] = json!([
            {
                "turn": 1,
                "system": {"chars": 10, "sha256": "a".repeat(64)},
                "context": {"present": false, "chars": 0, "sha256": ""},
            },
            {
                "turn": 2,
                "system": {"chars": 12, "sha256": "c".repeat(64)},
                "context": {"present": true, "chars": 5, "sha256": "b".repeat(64)},
            },
        ]);
        value["system"] = json!({"chars": 12, "sha256": "c".repeat(64)});
        value["context"] = json!({"present": true, "chars": 5, "sha256": "b".repeat(64)});
        let (lines, _) = build_lines(&value).expect("build");
        let parsed: Vec<Value> = lines
            .iter()
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect();
        assert_eq!(parsed[1]["type"], "system");
        assert_eq!(parsed[1]["turn"], 1);
        assert_eq!(parsed[2]["type"], "context");
        assert_eq!(parsed[2]["turn"], 1);
        assert_eq!(parsed[2]["present"], false);
        assert_eq!(parsed[3]["type"], "system");
        assert_eq!(parsed[3]["turn"], 2);
        assert_eq!(parsed[4]["type"], "context");
        assert_eq!(parsed[4]["present"], true);
        assert_eq!(parsed[5]["type"], "message", "时间线行仍排在消息之前");
        assert_eq!(
            parsed
                .iter()
                .filter(|line| line["type"] == "system")
                .count(),
            2,
            "兼容用的 system 字段不能再多导一行"
        );
    }
    /// 凭据在导出里必须被打码（与账本同一份 `mask_secrets`）。
    #[test]
    fn secrets_are_masked_before_exporting() {
        let mut value = session();
        value["messages"][0]["content"] =
            json!("用这个 key：api_key=sk-abcdef123456 和 Authorization: Bearer tok_9");
        let (lines, _) = build_lines(&value).expect("build");
        let joined = lines.join("\n");
        assert!(
            !joined.contains("sk-abcdef123456"),
            "明文 key 不能出门：{joined}"
        );
        assert!(!joined.contains("tok_9"), "Bearer 值不能出门：{joined}");
        assert!(joined.contains("***"), "要看得出被打码过");
        // 打码后仍是合法 JSON（不能因为替换把行弄坏）。
        for line in &lines {
            serde_json::from_str::<Value>(line).expect("每行都是合法 JSON");
        }
    }

    #[test]
    fn an_oversize_field_is_truncated_with_a_marker() {
        let mut value = session();
        value["messages"][1]["content"] = json!("x".repeat(MAX_FIELD_CHARS + 10));
        let (lines, truncated) = build_lines(&value).expect("build");
        assert!(truncated);
        let parsed: Value = serde_json::from_str(&lines[2]).expect("json");
        assert_eq!(parsed["truncated"], true);
        let text = parsed["text"].as_str().unwrap();
        assert!(text.contains("truncated"));
        assert!(text.chars().count() < MAX_FIELD_CHARS + 60);
        assert!(lines.last().unwrap().contains("\"notice\""));
    }

    #[test]
    fn export_writes_a_jsonl_file_under_module_data() {
        let root = temp_root("write");
        let raw = export_json(&root, &session());
        let result: Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(result["ok"], true, "{}", result["error"]);
        let path = PathBuf::from(result["path"].as_str().unwrap());
        assert!(path.starts_with(root.join("ModuleData").join("exports")));
        assert!(path.to_string_lossy().ends_with(".jsonl"));
        assert!(
            !path.with_extension("jsonl.tmp").exists(),
            "临时文件必须已改名"
        );
        let body = std::fs::read_to_string(&path).expect("读回来");
        assert_eq!(
            body.lines().count(),
            result["count"].as_u64().unwrap() as usize
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn trace_previews_without_writing_a_file() {
        let root = temp_root("trace");
        let raw = trace_json(&session());
        let result: Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(result["ok"], true);
        assert_eq!(result["count"], 7);
        assert!(!root.join("ModuleData").exists(), "预览不落盘");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bad_sessions_are_rejected() {
        for bad in [
            json!({}),
            json!({ "id": "  " }),
            json!({ "id": "x", "messages": [] }),
        ] {
            let reason = build_lines(&bad).expect_err("必须拒绝");
            assert!(!reason.is_empty());
        }
        let not_object = trace_json(&json!("nope"));
        let parsed: Value = serde_json::from_str(&not_object).expect("json");
        assert_eq!(parsed["ok"], false);
        assert_eq!(parsed["lines"], json!([]));
    }
}
