//! 备忘录（v1 `memo` 插件的原生等价物）：`<exe 根>/ModuleData/memo/memo.json`。
//!
//! 数据形状与 v1 **逐字段一致**（`id` / `text` / `tag` / `images` / `ts`），M6 可直接
//! 迁移 v1 的 `memo.json`。
//!   · `images` 是绝对路径数组；2026-11-14 起补齐「粘贴图片」：读系统剪贴板的
//!     `CF_DIB`，转成 24 位 BMP 落到 `ModuleData/memo/images/<id>/`（见
//!     `save_clipboard_image`）。拖放新增图片仍未接（要先把主窗的 drop 通道分派给面板）；
//!   · 新条目插到最前，编辑既有条目保持原位并更新 `ts`（与 v1 的 `list.unshift` 同序）；
//!   · 标识（`tag`）用于搜索：**精确 / 前缀匹配**（v1 的 `pushIndex` 口径）。
//!
//! 写盘用临时文件 + rename，写坏不会毁掉整份备忘录。

use std::path::{Path, PathBuf};

const MEMO_FILE: &str = "memo.json";

/// 备忘录文件路径（与 v1 `ModuleData/memo/memo.json` 同名）。
pub fn memo_path(root: &Path) -> PathBuf {
    root.join("ModuleData").join("memo").join(MEMO_FILE)
}

/// 读全部条目（JSON 数组字符串；文件不存在 / 读坏 = `[]`）。
pub fn load_json(root: &Path) -> String {
    serde_json::to_string(&serde_json::Value::Array(read_entries(root)))
        .unwrap_or_else(|_| "[]".to_string())
}

/// 整体保存（面板「清空全部」与批量写入都走这里）。
pub fn save_json(root: &Path, entries: &serde_json::Value) -> Result<(), String> {
    let Some(array) = entries.as_array() else {
        return Err("备忘录必须是 JSON 数组".to_string());
    };
    let cleaned: Vec<serde_json::Value> = array
        .iter()
        .filter(|item| valid_entry(item).is_some())
        .cloned()
        .collect();
    write_entries(root, &cleaned)
}

/// 新增 / 覆盖一条（按 `id`）。新条目插到最前；既有条目保持原位。
pub fn upsert(root: &Path, entry: &serde_json::Value) -> Result<(), String> {
    if valid_entry(entry).is_none() {
        return Err("备忘录缺少 id 或 text".to_string());
    }
    let id = entry_id(entry).unwrap_or_default().to_string();
    let mut entries = read_entries(root);
    match entries
        .iter()
        .position(|item| entry_id(item) == Some(id.as_str()))
    {
        Some(index) => entries[index] = entry.clone(),
        None => entries.insert(0, entry.clone()),
    }
    write_entries(root, &entries)
}

/// 删除一条；不存在也算成功。
pub fn delete(root: &Path, id: &str) -> Result<(), String> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let mut entries = read_entries(root);
    let before = entries.len();
    entries.retain(|item| entry_id(item) != Some(trimmed));
    if entries.len() == before {
        return Ok(());
    }
    write_entries(root, &entries)
}

/// 清空全部（调用方负责二次确认）。
pub fn clear(root: &Path) -> Result<(), String> {
    write_entries(root, &[])
}

/// 按标识做「精确 / 前缀」匹配（v1 的检索口径），最多 5 条、按文件顺序。
///
/// 返回 `(id, tag)`；查询为空时不返回任何东西（空查询不该把备忘录全列出来）。
pub fn search_tags(root: &Path, query: &str) -> Vec<(String, String)> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for entry in read_entries(root) {
        let Some(tag) = entry
            .get("tag")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let lowered = tag.to_lowercase();
        if lowered == needle || lowered.starts_with(&needle) {
            if let Some(id) = entry_id(&entry) {
                hits.push((id.to_string(), tag.to_string()));
                if hits.len() >= 5 {
                    break;
                }
            }
        }
    }
    hits
}

fn read_entries(root: &Path) -> Vec<serde_json::Value> {
    let Ok(text) = std::fs::read_to_string(memo_path(root)) else {
        return Vec::new();
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Array(items)) => items
            .into_iter()
            .filter(|item| valid_entry(item).is_some())
            .collect(),
        _ => Vec::new(),
    }
}

fn write_entries(root: &Path, entries: &[serde_json::Value]) -> Result<(), String> {
    let path = memo_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建备忘录目录失败（{}）: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(entries).map_err(|e| format!("序列化备忘录失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写备忘录失败（{}）: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("替换备忘录失败（{}）: {e}", path.display()))?;
    Ok(())
}

/// 条目至少要有一个非空 `id` 字符串和一个 `text` 字符串（`tag` / `images` / `ts` 可缺）。
fn valid_entry(entry: &serde_json::Value) -> Option<()> {
    if !entry.is_object() {
        return None;
    }
    let id = entry.get("id").and_then(|value| value.as_str())?;
    if id.trim().is_empty() {
        return None;
    }
    entry.get("text").and_then(|value| value.as_str())?;
    Some(())
}

fn entry_id(entry: &serde_json::Value) -> Option<&str> {
    entry
        .get("id")
        .and_then(|value| value.as_str())
        .map(str::trim)
}

// ── 粘贴图片（2026-11-14）──────────────────────────────────────────────
//
// 复用 `clipboard_payload` 的「CF_DIB → BMP」读取与编码（它已经处理了 CF_DIBV5、
// 调色板 / 掩码、行对齐与坏头拒绝），这里只把临时文件挪进备忘录自己的图片目录。

/// 把剪贴板里的图片存成 `<root>/ModuleData/memo/images/<id>/<ts>.bmp`。
#[cfg(windows)]
pub fn save_clipboard_image(root: &Path, memo_id: &str) -> Result<String, String> {
    let id = memo_id.trim();
    if id.is_empty() {
        return Err("先保存为备忘录，再粘贴图片".to_string());
    }
    if id.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Err("备忘录标识不合法".to_string());
    }
    let staged = crate::clipboard_payload::save_image_now()
        .ok_or_else(|| "剪贴板里没有图片".to_string())?;

    let dir = root.join("ModuleData").join("memo").join("images").join(id);
    std::fs::create_dir_all(&dir).map_err(|error| format!("创建图片目录失败：{error}"))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("{stamp}.bmp"));
    // 同盘 rename；跨盘 / 占用时退回复制 + 删临时文件。
    if std::fs::rename(&staged, &path).is_err() {
        std::fs::copy(&staged, &path).map_err(|error| format!("写图片失败：{error}"))?;
        let _ = std::fs::remove_file(&staged);
    }
    Ok(path.to_string_lossy().to_string())
}

#[cfg(not(windows))]
pub fn save_clipboard_image(_root: &Path, _memo_id: &str) -> Result<String, String> {
    Err("当前平台不支持读取剪贴板图片".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("lunac-memo-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    fn entry(id: &str, text: &str, tag: &str, ts: u64) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "text": text,
            "tag": tag,
            "images": [],
            "ts": ts,
        })
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

    /// 新条目插到最前；编辑既有条目保持原位并更新内容。
    #[test]
    fn upsert_inserts_at_front_and_keeps_position_on_edit() {
        let root = temp_root("upsert");
        upsert(&root, &entry("a", "first", "", 1)).unwrap();
        upsert(&root, &entry("b", "second", "", 2)).unwrap();
        assert_eq!(read(&root)[0]["id"], "b");

        upsert(&root, &entry("a", "first edited", "tag-a", 3)).unwrap();
        let items = read(&root);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], "b");
        assert_eq!(items[1]["id"], "a");
        assert_eq!(items[1]["text"], "first edited");
        assert_eq!(items[1]["tag"], "tag-a");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 非法条目整体拒绝；`images` 等未编辑字段原样保留。
    #[test]
    fn invalid_entries_are_rejected_and_unknown_fields_survive() {
        let root = temp_root("invalid");
        assert!(upsert(&root, &serde_json::json!({"text": "no id"})).is_err());
        assert!(upsert(&root, &serde_json::json!([1, 2])).is_err());

        let mut value = entry("a", "text", "", 1);
        value["images"] = serde_json::json!(["D:/x/a.png"]);
        value["extra"] = serde_json::json!(true);
        upsert(&root, &value).unwrap();
        let items = read(&root);
        assert_eq!(items[0]["images"][0], "D:/x/a.png");
        assert_eq!(items[0]["extra"], true);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_and_clear_round_trip() {
        let root = temp_root("delete");
        upsert(&root, &entry("a", "A", "", 1)).unwrap();
        upsert(&root, &entry("b", "B", "", 2)).unwrap();

        delete(&root, "a").unwrap();
        assert_eq!(read(&root).len(), 1);
        // 删除不存在 / 空 id 都是 no-op。
        delete(&root, "nope").unwrap();
        delete(&root, "  ").unwrap();
        assert_eq!(read(&root).len(), 1);

        clear(&root).unwrap();
        assert!(read(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 标识检索：精确 / 前缀（大小写不敏感），空查询不返回；空标识不参与。
    #[test]
    fn search_tags_matches_exact_and_prefix_only() {
        let root = temp_root("search");
        upsert(&root, &entry("a", "A", "work", 1)).unwrap();
        upsert(&root, &entry("b", "B", "workshop", 2)).unwrap();
        upsert(&root, &entry("c", "C", "home", 3)).unwrap();
        upsert(&root, &entry("d", "D", "   ", 4)).unwrap();

        let hits = search_tags(&root, "work");
        let ids: Vec<&str> = hits.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["b", "a"]); // 文件顺序：b 在前
        assert_eq!(search_tags(&root, "WORK").len(), 2);
        assert_eq!(search_tags(&root, "work").first().unwrap().1, "workshop");
        // 只做前缀：中间匹配不算（"shop" 不命中 workshop）。
        assert!(search_tags(&root, "shop").is_empty());
        assert!(search_tags(&root, "").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 坏 JSON 不破坏后续写入。
    #[test]
    fn bad_json_is_tolerated() {
        let root = temp_root("badjson");
        std::fs::create_dir_all(memo_path(&root).parent().unwrap()).unwrap();
        std::fs::write(memo_path(&root), "{not json").unwrap();
        assert!(read(&root).is_empty());
        upsert(&root, &entry("a", "A", "", 1)).unwrap();
        assert_eq!(read(&root)[0]["id"], "a");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 没有 id 时明确提示「先保存」；带路径分隔符的 id 直接拒绝（不会去建目录）。
    #[test]
    fn clipboard_image_requires_a_safe_saved_id() {
        let root = temp_root("clip");
        let error = save_clipboard_image(&root, "  ").unwrap_err();
        assert!(error.contains("先保存"));
        let error = save_clipboard_image(&root, "a/b").unwrap_err();
        assert!(error.contains("不合法"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
