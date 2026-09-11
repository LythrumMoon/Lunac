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
