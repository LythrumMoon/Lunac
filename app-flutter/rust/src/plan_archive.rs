//! 计划留档（`docs/todo.md` §待办 3-28.3）：原始计划 + 用户改后的计划 + 一句备注。
//!
//! **为什么要在宿主落一份**：编辑发生在审批卡上，agent 只收到「改后」的那份文本 ——
//! 事后想回答「这次批准到底删了哪几步 / 加了什么」就只能靠这份文件（安全账本只记
//! 「批准了」，不记计划正文）。
//!
//! 位置与 v1 同口径：`<exe 根>/ModuleData/plans/plan-YYYYMMDD-HHMMSSZ.json`。
//! 单条上限 256 KiB：计划是文本，超过这个量级多半是误传（防止把整份日志塞进来）。

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::security_ledger::utc_now;

/// 单份计划（原文 / 改后 / 备注各自）的体积上限。
const MAX_PLAN_BYTES: usize = 256 * 1024;

fn plans_dir(root: &Path) -> PathBuf {
    root.join("ModuleData").join("plans")
}

/// 与 v1 `storage::save_plan_md` 同形的文件名主干：`2026-11-15_071530Z`。
///
/// 唯一的差异是**时区**：v1 的时间戳由前端按本地时间生成，宿主这里只有 UTC
/// （std 不带时区库），所以后缀带 `Z` 明示。文件用途是人读，不影响任何程序读取。
fn md_stamp(iso: &str) -> String {
    iso.replace('T', "_").replace(':', "")
}

/// 落一份计划留档，返回**给人看的那份**路径。
///
/// 同一目录下写**同一时间戳的两个文件**（与 v1 并存，不是替代）：
///   · `<YYYY-MM-DD_HHMMSS>.md` —— 用户批准的那份计划正文（v1 `save_plan_md` 同形）；
///   · `<YYYY-MM-DD_HHMMSS>.json` —— 原文 / 改后 / 备注 / `changed`，
///     供事后追「到底删了哪几步」。
/// 两份都空 ⇒ 报错（没有内容就不该留下空文件）；一份为空是合法的
/// （例如模型只给了标题、用户把步骤全删了但留了备注）。
pub fn archive(root: &Path, original: &str, edited: &str, note: &str) -> Result<PathBuf, String> {
    let original = original.trim();
    let edited = edited.trim();
    let note = note.trim();
    if original.is_empty() && edited.is_empty() {
        return Err("计划为空，没有可留档的内容".into());
    }
    for (label, text) in [("原始计划", original), ("改后计划", edited), ("备注", note)] {
        if text.len() > MAX_PLAN_BYTES {
            return Err(format!("{label}超过 {MAX_PLAN_BYTES} 字节，拒绝留档"));
        }
    }
    let dir = plans_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建计划目录失败: {e}"))?;
    let (stamp, _) = utc_now();
    let json_path = dir.join(format!("{}.json", md_stamp(&stamp)));
    let body = json!({
        "v": 1,
        "ts": stamp,
        "original": original,
        "edited": edited,
        "note": note,
        "changed": original != edited,
    });
    let text = serde_json::to_string_pretty(&body).map_err(|e| e.to_string())?;
    std::fs::write(&json_path, text).map_err(|e| format!("写入计划留档失败: {e}"))?;
    // v1 同形的 markdown：批准的那份正文（没批准正文时退回备注，不至于空文件）。
    let human = if edited.is_empty() { note } else { edited };
    if human.is_empty() {
        return Ok(json_path);
    }
    let md_path = dir.join(format!("{}.md", md_stamp(&stamp)));
    std::fs::write(&md_path, human).map_err(|e| format!("写入计划正文失败: {e}"))?;
    Ok(md_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-plan-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn archive_keeps_original_edited_and_note() {
        let root = temp_root("keep");
        let path = archive(&root, "1. 原步骤\n2. 第二步", "1. 只做第一步", "别动数据库")
            .expect("留档该成功");
        // 返回值是给人看的那份 markdown（v1 `save_plan_md` 同形）。
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("md"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("md 该在"),
            "1. 只做第一步"
        );
        // 同目录的 JSON 存原文 / 改后 / 备注。
        let json_path = path.with_extension("json");
        let text = std::fs::read_to_string(&json_path).expect("json 该在");
        let value: serde_json::Value = serde_json::from_str(&text).expect("必须是合法 JSON");
        assert_eq!(value["original"], "1. 原步骤\n2. 第二步");
        assert_eq!(value["edited"], "1. 只做第一步");
        assert_eq!(value["note"], "别动数据库");
        assert_eq!(value["changed"], true);
        assert!(path.starts_with(root.join("ModuleData").join("plans")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn note_only_archive_skips_the_plan_markdown_but_keeps_the_json() {
        // 用户把步骤全关掉、只留一句备注：md 用备注兜底，JSON 照常留档。
        let root = temp_root("note-only");
        let path = archive(&root, "1. 原步骤", "", " 先别动数据库 ").expect("留档该成功");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "先别动数据库");
        assert!(path.with_extension("json").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_plans_are_rejected() {
        let root = temp_root("empty");
        assert!(archive(&root, "  ", "", "").is_err(), "两份都空不该留下文件");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn oversized_plans_are_rejected() {
        let root = temp_root("big");
        let huge = "x".repeat(MAX_PLAN_BYTES + 1);
        assert!(archive(&root, &huge, "1. 小计划", "").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
