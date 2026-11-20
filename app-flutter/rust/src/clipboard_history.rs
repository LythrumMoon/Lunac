//! 剪贴板历史：`<exe 根>/ModuleData/history/clipboard-history.json`。
//!
//! 移植 v1 `storage.rs` 的三条命令（v2 走 C ABI，不经 Tauri IPC）：
//! `load_clipboard_history` / `append_clipboard_entry` / `save_clipboard_history`。
//! 文件位置与 **JSON 字段名**（`clip_type` / `text` / `file_paths` / `time`）与 v1
//! 完全一致 —— M6 数据迁移时可以直接把 v1 那份拷过来。
//!
//! 「粘贴进搜索框时顺手记一条」是**主窗口行为**（v1 2026-09-29 的结论）：剪贴板历史
//! 插件可能根本没装，所以记录不依赖插件，由 `main.dart` 的唤出处理直接调本模块。
//!
//! 纪律（与 v1 逐条对齐）：
//!   · 去重：文本按全文、文件条目按「类型 + 全文」；
//!   · 置顶：命中的旧条目先删掉再插到最前，最多保留 30 条；
//!   · 单条文本超过 2000 个 **UTF-16 码元** 不收（JS `length` 的口径，emoji 不折算）；
//!   · 写盘用临时文件 + rename，写坏不会毁掉整份历史。

use std::path::{Path, PathBuf};

/// 最多保留的条数（与 v1 `MAX_CLIP_ITEMS` 同值；前端插件那边也是 30）。
const MAX_ITEMS: usize = 30;
/// 单条文本的上限（UTF-16 码元数，与 v1 同口径）。
const MAX_TEXT_UNITS: usize = 2000;

const CLIP_FILE: &str = "clipboard-history.json";

/// 历史文件路径（与 v1 `history_dir().join("clipboard-history.json")` 同名）。
pub fn history_path(root: &Path) -> PathBuf {
    root.join("ModuleData")
        .join("history")
        .join(CLIP_FILE)
}

/// 读全部条目（JSON 数组字符串；文件不存在 / 读坏 = `[]`）。
pub fn load_json(root: &Path) -> String {
    serde_json::to_string(&serde_json::Value::Array(read_entries(root)))
        .unwrap_or_else(|_| "[]".to_string())
}

/// 记一条（去重 → 置顶 → 截断到 30 条）。
///
/// 空文本 / 超过 2000 码元 = **静默忽略**（不是错误）：调用方是每次唤出的热路径，
/// 不该因为一次超长剪贴板就在日志里刷错误。
pub fn append(root: &Path, text: &str) -> Result<(), String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if trimmed.encode_utf16().count() > MAX_TEXT_UNITS {
        return Ok(());
    }
    let is_file = is_windows_path(trimmed);
    let mut entries = read_entries(root);
    // 去重：文件条目按「类型 + 全文」，文本条目按全文（与 v1 同判据）。
    if let Some(index) = entries.iter().position(|entry| {
        let entry_file = is_file_type(entry);
        if is_file {
            entry_file && entry_text(entry) == trimmed
        } else {
            !entry_file && entry_text(entry) == trimmed
        }
    }) {
        entries.remove(index);
    }
    entries.insert(
        0,
        serde_json::json!({
            "clip_type": if is_file { "file" } else { "text" },
            "text": trimmed,
            "file_paths": if is_file { vec![trimmed.to_string()] } else { Vec::<String>::new() },
            "time": now_millis(),
        }),
    );
    entries.truncate(MAX_ITEMS);
    write_entries(root, &entries)
}

/// 把一条已存在的条目提到最前（面板里「点一下就复制」之后调用，与 v1 一致）。
///
/// 找不到 = no-op（条目可能刚被清空）；`time` 保持不变（v1 也只换位置）。
pub fn promote(root: &Path, text: &str) -> Result<(), String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let mut entries = read_entries(root);
    let Some(index) = entries
        .iter()
        .position(|entry| entry_text(entry) == trimmed)
    else {
        return Ok(());
    };
    if index == 0 {
        return Ok(());
    }
    let entry = entries.remove(index);
    entries.insert(0, entry);
    write_entries(root, &entries)
}

/// 清空历史（面板「清空」；调用方负责二次确认）。
pub fn clear(root: &Path) -> Result<(), String> {
    write_entries(root, &[])
}

fn read_entries(root: &Path) -> Vec<serde_json::Value> {
    let Ok(text) = std::fs::read_to_string(history_path(root)) else {
        return Vec::new();
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Array(items)) => items
            .into_iter()
            .filter(|item| item.is_object() && item.get("text").is_some())
            .collect(),
        _ => Vec::new(),
    }
}

fn write_entries(root: &Path, entries: &[serde_json::Value]) -> Result<(), String> {
    let path = history_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建历史目录失败（{}）: {e}", parent.display()))?;
    }
    let text =
        serde_json::to_string_pretty(entries).map_err(|e| format!("序列化剪贴板历史失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)
        .map_err(|e| format!("写剪贴板历史失败（{}）: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("替换剪贴板历史失败（{}）: {e}", path.display()))?;
    Ok(())
}

fn entry_text(entry: &serde_json::Value) -> &str {
    entry.get("text").and_then(|value| value.as_str()).unwrap_or("")
}

fn is_file_type(entry: &serde_json::Value) -> bool {
    entry
        .get("clip_type")
        .and_then(|value| value.as_str())
        .map(|value| value == "file")
        .unwrap_or(false)
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// `D:\...` / `C:/...` 这类「盘符 + 冒号 + 斜杠」开头的绝对路径
/// —— 与 v1 的正则 `^[A-Za-z]:[\\/]` 同义（ASCII 盘符，不让中文进来）。
fn is_windows_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-clipboard-history-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    fn read(root: &Path) -> Vec<serde_json::Value> {
        let value: serde_json::Value = serde_json::from_str(&load_json(root)).unwrap();
        value.as_array().unwrap().clone()
    }

    #[test]
    fn missing_file_reads_as_empty() {
        let root = temp_root("missing");
        assert!(read(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 去重 → 置顶；文件条目与同文本文条目互不影响。
    #[test]
    fn append_dedupes_and_moves_to_front() {
        let root = temp_root("dedupe");
        append(&root, "alpha").unwrap();
        append(&root, "beta").unwrap();
        append(&root, "alpha").unwrap();

        let items = read(&root);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["text"], "alpha");
        assert_eq!(items[0]["clip_type"], "text");
        assert!(items[0]["time"].as_u64().unwrap() > 0);
        assert_eq!(items[1]["text"], "beta");

        // 文件路径记成 file 类型，且带 file_paths。
        append(&root, r"D:\Docs\report.pdf").unwrap();
        let items = read(&root);
        assert_eq!(items[0]["clip_type"], "file");
        assert_eq!(items[0]["file_paths"][0], r"D:\Docs\report.pdf");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 上限 30 条；超长文本（>2000 UTF-16 码元）与空串静默忽略。
    #[test]
    fn cap_and_length_rules() {
        let root = temp_root("cap");
        for index in 0..(MAX_ITEMS + 5) {
            append(&root, &format!("item-{index}")).unwrap();
        }
        let items = read(&root);
        assert_eq!(items.len(), MAX_ITEMS);
        assert_eq!(items[0]["text"], format!("item-{}", MAX_ITEMS + 4));

        append(&root, "   ").unwrap();
        append(&root, &"x".repeat(MAX_TEXT_UNITS + 1)).unwrap();
        assert_eq!(read(&root).len(), MAX_ITEMS);

        // 恰好 2000 码元要收（边界）。
        append(&root, &"y".repeat(MAX_TEXT_UNITS)).unwrap();
        assert_eq!(read(&root)[0]["text"], "y".repeat(MAX_TEXT_UNITS));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn promote_moves_an_existing_entry_and_tolerates_missing() {
        let root = temp_root("promote");
        append(&root, "a").unwrap();
        append(&root, "b").unwrap();
        assert_eq!(read(&root)[0]["text"], "b");

        promote(&root, "a").unwrap();
        assert_eq!(read(&root)[0]["text"], "a");

        // 不存在的条目 / 空串 = no-op，不报错。
        promote(&root, "nope").unwrap();
        promote(&root, "  ").unwrap();
        assert_eq!(read(&root).len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_empties_the_file_and_bad_json_is_tolerated() {
        let root = temp_root("clear");
        append(&root, "a").unwrap();
        clear(&root).unwrap();
        assert!(read(&root).is_empty());

        std::fs::write(history_path(&root), "{not json").unwrap();
        assert!(read(&root).is_empty());
        // 读坏文件后仍能正常追加（重新写成合法 JSON）。
        append(&root, "b").unwrap();
        assert_eq!(read(&root)[0]["text"], "b");
        let _ = std::fs::remove_dir_all(&root);
    }
}
