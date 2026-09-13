// src-tauri/src/storage.rs
// File-based persistent storage for chat history, clipboard history and memo.
// 数据目录（2026-09 修订）：全部缓存与业务/插件数据统一放「exe 安装根目录」，
// 即可执行文件所在目录，使应用数据结构清晰、随卸载一并清除：
//   - <exe_dir>\ModuleData\history\chat-history.json / clipboard-history.json
//   - <exe_dir>\ModuleData\memo\memo.json（备忘录，含图片 images\<id>\）
//   - <exe_dir>\ModuleData\custom\app_registry.json（自定义启动项）
//   - <exe_dir>\temp\webview-data（WebView2 用户数据/缓存，见 main.rs）
//   - <exe_dir>\temp\app-index-cache.json（应用扫描缓存，见 app_indexer.rs）
//   - <exe_dir>\skills、<exe_dir>\tools、<exe_dir>\config、<exe_dir>\paddle-ocr
// 旧版本数据曾放在 %LOCALAPPDATA%\Lunac(-dev)，首次启动由
// migrate_legacy_localappdata() 整体搬移后删除。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// 应用数据根目录 = 可执行文件所在目录（exe 安装根）。
///   release → 安装目录；dev → target\debug。dev/release 数据因此天然隔离。
/// 所有数据落盘模块都应调用本函数，禁止各自硬编码路径。
pub fn lunac_root_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 业务数据根目录：<exe_dir>\ModuleData
pub fn module_data_dir() -> PathBuf {
    lunac_root_dir().join("ModuleData")
}

// ── 旧数据整体迁移（%LOCALAPPDATA%\Lunac(-dev) → exe 根）──────────────
// 迁移后会删除旧目录（用户决策）。幂等：仅当目标 ModuleData 尚未存在时才复制；
// 若已存在则直接清理旧目录，避免每次启动重复搬移。

fn legacy_localappdata_candidates() -> Vec<PathBuf> {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    if local.is_empty() {
        return Vec::new();
    }
    // 旧 release/默认目录在前，debug 专用目录在后（历史顺序上两者都可能存在）
    vec![
        PathBuf::from(&local).join("Lunac"),
        PathBuf::from(&local).join("Lunac-dev"),
    ]
}

/// 递归复制目录内容（跳过无法读取的文件，迁移为 best-effort）。
fn copy_dir_recursive(src: &Path, dst: &Path) {
    let entries = match fs::read_dir(src) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            let _ = fs::create_dir_all(&to);
            copy_dir_recursive(&from, &to);
        } else {
            let _ = fs::copy(&from, &to);
        }
    }
}

/// 首次启动：把 %LOCALAPPDATA%\Lunac(-dev) 下旧数据整体搬入 exe 根，
/// 成功后删除旧目录（双清理的一部分，旧版本遗留由此顺带清除）。
pub fn migrate_legacy_localappdata() {
    let target = lunac_root_dir();
    const SUBS: [&str; 6] = [
        "ModuleData",
        "skills",
        "tools",
        "config",
        "paddle-ocr",
        "temp/webview-data",
    ];
    for legacy in legacy_localappdata_candidates() {
        if !legacy.exists() || legacy == target {
            continue;
        }
        // 目标尚未有数据 → 搬移；已有则视为已迁移
        if !target.join("ModuleData").exists() {
            for sub in SUBS {
                let src = legacy.join(sub);
                if src.exists() {
                    let dst = target.join(sub);
                    if let Some(parent) = dst.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    copy_dir_recursive(&src, &dst);
                }
            }
        }
        // 仅当新根数据已就位（ModuleData 已存在）或旧根已空时才删除旧目录，
        // 避免目标目录不可写（如安装在只读位置）时误删仍有效的数据。
        let copied = target.join("ModuleData").exists();
        let legacy_now_empty = fs::read_dir(&legacy)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false);
        if copied || legacy_now_empty {
            let _ = fs::remove_dir_all(&legacy);
        }
    }
}

fn data_dir() -> PathBuf {
    lunac_root_dir()
}

/// History subdirectory: <exe_dir>\ModuleData\history
fn history_dir() -> PathBuf {
    module_data_dir().join("history")
}

fn ensure_history_dir() -> std::io::Result<()> {
    fs::create_dir_all(history_dir())
}

/// 旧版目录（迁移前）：<exe_dir>\history（极早期布局，兼容用）
fn legacy_history_dir() -> PathBuf {
    data_dir().join("history")
}

/// 读取文件，若新位置不存在则尝试从旧 history 目录迁移一次。
fn read_with_legacy_migration(file: &str) -> Result<Option<String>, String> {
    let new_path = history_dir().join(file);
    if new_path.exists() {
        let json = fs::read_to_string(&new_path).map_err(|e| e.to_string())?;
        return Ok(Some(json));
    }
    let legacy_path = legacy_history_dir().join(file);
    if legacy_path.exists() {
        let json = fs::read_to_string(&legacy_path).map_err(|e| e.to_string())?;
        // 迁移：写入新位置（旧文件保留，由用户/清理策略决定）
        fs::create_dir_all(history_dir()).map_err(|e| e.to_string())?;
        fs::write(&new_path, &json).map_err(|e| e.to_string())?;
        return Ok(Some(json));
    }
    Ok(None)
}

// ── Chat History ──────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatSession {
    pub id: String,
    pub title: String,
    pub messages: Vec<ChatMessage>,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
}

const CHAT_FILE: &str = "chat-history.json";

#[tauri::command]
pub fn save_chat_sessions(sessions: Vec<ChatSession>) -> Result<(), String> {
    ensure_history_dir().map_err(|e| e.to_string())?;
    let path = history_dir().join(CHAT_FILE);
    let json = serde_json::to_string_pretty(&sessions).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn load_chat_sessions() -> Result<Vec<ChatSession>, String> {
    match read_with_legacy_migration(CHAT_FILE)? {
        Some(json) => serde_json::from_str(&json).map_err(|e| e.to_string()),
        None => Ok(vec![]),
    }
}

// ── Clipboard History ─────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClipEntry {
    /// "text" for plain text entries, "file" for file path entries
    #[serde(default = "default_clip_type")]
    pub clip_type: String,
    /// Plain text (for text-type entries; may also hold inline text for file-type)
    #[serde(default)]
    pub text: String,
    /// File paths (for file-type entries)
    #[serde(default)]
    pub file_paths: Vec<String>,
    pub time: u64,
}

fn default_clip_type() -> String { "text".to_string() }

const CLIP_FILE: &str = "clipboard-history.json";

#[tauri::command]
pub fn save_clipboard_history(entries: Vec<ClipEntry>) -> Result<(), String> {
    ensure_history_dir().map_err(|e| e.to_string())?;
    let path = history_dir().join(CLIP_FILE);
    let json = serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn load_clipboard_history() -> Result<Vec<ClipEntry>, String> {
    match read_with_legacy_migration(CLIP_FILE)? {
        Some(json) => serde_json::from_str(&json).map_err(|e| e.to_string()),
        None => Ok(vec![]),
    }
}

// ── 用量日志（ModuleData\usage\usage-YYYY-MM-DD.jsonl，与平台对账用）────
//
// 每次用户提问一行。**口径**：一行 = 一次提问的合计（含提问内所有工具往返），
// 而供应商平台按「每次 API 请求」记行 —— 一次带工具的提问在平台上就是多行，
// 对账时把同一时间窗的平台各行相加。
//
// 只追加不重写：文件天然按天分片、可被任何工具解析（jq/脚本），且不必担心
// 并发写坏。文件名用**本地日期**（由前端传入）：Rust 侧没有 chrono，不为一句
// 时区换算引入新依赖。

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UsageRecord {
    /// 本地时钟的 epoch 毫秒（前端 Date.now()）
    pub ts: u64,
    /// 产生这条记录的模型名（来自 agent 的 system/init）
    pub model: String,
    pub input: u64,
    pub output: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    #[serde(rename = "cacheCreate")]
    pub cache_create: u64,
    /// 本次提问内 agent 报告的历史压缩次数（瘦身 tool_result / 丢弃旧消息）。
    /// 压缩会改写请求前缀 → 端点侧缓存作废，是命中率的**断裂型**失效来源，
    /// 与「新内容天生没被上一轮缓存覆盖」的自然未命中分开统计用。
    /// 旧记录没有这两个字段，读时按 0（`serde(default)`）。
    #[serde(default)]
    pub elided: u64,
    #[serde(default)]
    pub dropped: u64,
}

fn usage_dir() -> PathBuf {
    module_data_dir().join("usage")
}

/// 只接受严格的 `YYYY-MM-DD` —— 文件名来自前端，必须挡住路径拼串
fn usage_log_path(date: &str) -> Result<PathBuf, String> {
    let b = date.as_bytes();
    let ok = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() });
    if !ok {
        return Err(format!("invalid date: {date}"));
    }
    Ok(usage_dir().join(format!("usage-{date}.jsonl")))
}

#[tauri::command]
pub fn append_usage_log(date: String, record: UsageRecord) -> Result<(), String> {
    let path = usage_log_path(&date)?;
    fs::create_dir_all(usage_dir()).map_err(|e| e.to_string())?;
    let line = serde_json::to_string(&record).map_err(|e| e.to_string())?;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(f, "{line}").map_err(|e| e.to_string())
}

/// 读取某天的全部记录；文件不存在返回空表。单行损坏只跳过该行（半个写入
/// 的文件不该让整个面板失效）。
#[tauri::command]
pub fn read_usage_log(date: String) -> Result<Vec<UsageRecord>, String> {
    let path = usage_log_path(&date)?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    Ok(text
        .lines()
        .filter_map(|l| serde_json::from_str::<UsageRecord>(l).ok())
        .collect())
}

// ── 备忘录（ModuleData\memo\memo.json）──────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MemoEntry {
    pub id: String,
    pub text: String,
    /// 用户自定义检索标识（如 #项目A / travel-plan），可为空
    #[serde(default)]
    pub tag: String,
    /// 随备忘录保存的图片绝对路径（2026-09）
    #[serde(default)]
    pub images: Vec<String>,
    pub ts: u64,
}

fn memo_dir() -> PathBuf {
    module_data_dir().join("memo")
}

fn memo_images_dir(id: &str) -> PathBuf {
    memo_dir().join("images").join(id)
}

fn memo_file() -> PathBuf {
    memo_dir().join("memo.json")
}

#[tauri::command]
pub fn memo_save_entries(entries: Vec<MemoEntry>) -> Result<(), String> {
    fs::create_dir_all(memo_dir()).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?;
    fs::write(memo_file(), json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn memo_load_entries() -> Result<Vec<MemoEntry>, String> {
    let path = memo_file();
    if !path.exists() {
        return Ok(vec![]);
    }
    let json = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&json).map_err(|e| e.to_string())
}

/// 将备忘录粘贴的图片（dataURL）写入 ModuleData\memo\images\<id>\<index>.png|jpg，
/// 返回该图片的绝对路径（前端用 convertFileSrc 显像）。
#[tauri::command]
pub fn memo_save_image(id: String, index: usize, data_url: String) -> Result<String, String> {
    let mime = data_url
        .split("data:")
        .nth(1)
        .and_then(|s| s.split(';').next())
        .and_then(|s| s.split(',').next())
        .unwrap_or("image/png");
    let ext = match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/bmp" => "bmp",
        "image/webp" | "image/gif" => "png",
        "image/tiff" | "image/tif" => "tiff",
        _ => "png",
    };
    let b64 = data_url
        .split(',')
        .nth(1)
        .ok_or("Invalid data URL format")?;
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        b64,
    )
    .map_err(|e| format!("Base64 decode failed: {e}"))?;

    let dir = memo_images_dir(&id);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("{}.{}", index, ext));
    fs::write(&file, &bytes).map_err(|e| format!("Write memo image failed: {e}"))?;
    Ok(file.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(cache_read: u64) -> UsageRecord {
        UsageRecord {
            ts: 1_700_000_000_000,
            model: "deepseek-flash".into(),
            input: 212,
            output: 3,
            cache_read,
            cache_create: 0,
        }
    }

    /// 文件名来自前端 —— 必须挡住跨目录拼串与非 `YYYY-MM-DD` 形态
    #[test]
    fn usage_log_path_only_accepts_iso_date() {
        for bad in ["", "2026-9-1", "2026/09/12", "../2026-09-12", "2026-09-12x", "2026-09-1"] {
            assert!(usage_log_path(bad).is_err(), "should reject `{bad}`");
        }
        assert!(usage_log_path("2026-09-12").is_ok());
        assert!(usage_log_path("1970-01-01").is_ok());
    }

    /// 字段名是前端 `read_usage_log` 的消费契约（cacheRead/cacheCreate 驼峰）
    #[test]
    fn usage_record_json_shape() {
        let line = serde_json::to_string(&rec(1536)).unwrap();
        assert_eq!(
            line,
            r#"{"ts":1700000000000,"model":"deepseek-flash","input":212,"output":3,"cacheRead":1536,"cacheCreate":0}"#
        );
    }

    #[test]
    fn usage_log_append_read_and_tolerate_broken_line() {
        let date = "1970-01-01"; // 固定的远古日期，不与真实用量混在一起
        let path = usage_log_path(date).unwrap();
        let _ = fs::remove_file(&path);

        append_usage_log(date.into(), rec(1536)).unwrap();
        append_usage_log(date.into(), rec(7000)).unwrap();
        let got = read_usage_log(date.into()).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].cache_read, 7000);

        // 半截写入不该让整个面板失效
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{{\"ts\":1,").unwrap();
        assert_eq!(read_usage_log(date.into()).unwrap().len(), 2);

        let _ = fs::remove_file(&path);
        assert!(read_usage_log("1970-01-02".into()).unwrap().is_empty());
    }
}
