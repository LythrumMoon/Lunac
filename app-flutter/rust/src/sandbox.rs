//! AI 沙箱的「隔离盘」后端（预览窗用）。
//!
//! **它是什么**：把 agent 的工作目录 `<exe 根>/temp/transStorage` 当成一块**固定额度的
//! 独立磁盘**来展示 —— 用量 / 额度 / 文件清单 / 只读预览 / 一键清空。
//!
//! **它不是什么**：不是 OS 级隔离（见 `docs/sandbox-research.md`）。边界来自 agent 的
//! 启动参数（`agent.rs`：cwd + `--add-dir` 都只有这个目录，权限档 `acceptEdits`），
//! 本模块只读地统计与预览，**不拦截**任何写入 —— 所以界面上不能写「虚拟机 / 隔离环境」，
//! 只能写「沙箱盘（受控工作目录）」并如实标出额度与用量。
//!
//! 固定额度：`<exe 根>/config/sandbox.json` 的 `quotaBytes`，缺省 512 MiB（可改，64 MiB–8 GiB）。
//! 超额度只**如实告警**（agent 仍在写），由用户决定清理；不做静默删除。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use serde::Serialize;

use crate::plugin_market::safe_join;

/// 额度缺省值：512 MiB（「一块小盘」的量级，够模型跑脚本 / 生成图片）。
pub const DEFAULT_QUOTA_BYTES: u64 = 512 * 1024 * 1024;
/// 额度的下限 / 上限（防止配置写成 0 或几个 TB）。
pub const MIN_QUOTA_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_QUOTA_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// 单次预览最多读多少字节（超出只给前 256 KiB，并标明截断）。
pub const MAX_PREVIEW_BYTES: u64 = 256 * 1024;
/// 清单最多返回多少条（界面是滚动列表，500 条足够看清「模型写了什么」）。
const MAX_ENTRIES: usize = 500;
/// 统计用量时最多走多少个条目（防沙箱里堆了十万文件把 UI 拖死）。
const MAX_SCAN_ENTRIES: u64 = 20_000;
/// 清单最多递归几层。
const MAX_DEPTH: u32 = 4;
/// 状态里「最近改动」最多列几条（预览窗顶部那块紧凑列表）。
const MAX_RECENT: usize = 8;

/// agent 的工作目录 = 沙箱盘根。**唯一真相源**：`agent.rs` 也走这里。
pub fn workdir(exe_root: &Path) -> PathBuf {
    exe_root.join("temp").join("transStorage")
}

/// 确保沙箱盘存在，返回它的绝对路径。
pub fn prepare(exe_root: &Path) -> Result<PathBuf, String> {
    let dir = workdir(exe_root);
    fs::create_dir_all(&dir).map_err(|e| format!("创建沙箱盘失败（{}）：{e}", dir.display()))?;
    Ok(dir)
}

/// 固定额度（配置可改，缺省 512 MiB）。
pub fn quota_bytes(exe_root: &Path) -> u64 {
    let path = exe_root.join("config").join("sandbox.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return DEFAULT_QUOTA_BYTES;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return DEFAULT_QUOTA_BYTES;
    };
    let raw = value.get("quotaBytes").and_then(|v| v.as_u64());
    match raw {
        Some(bytes) => bytes.clamp(MIN_QUOTA_BYTES, MAX_QUOTA_BYTES),
        None => DEFAULT_QUOTA_BYTES,
    }
}

/// 设置固定额度：写 `<exe 根>/config/sandbox.json` 的 `quotaBytes`（夹取到 64 MiB–8 GiB）。
///
/// **只改 `quotaBytes` 一个键**，保留文件里的其它字段；写失败返回一句人话。
/// 返回 JSON：`{ ok, quotaBytes, error }`。
pub fn set_quota_json(exe_root: &Path, bytes: u64) -> String {
    let quota = bytes.clamp(MIN_QUOTA_BYTES, MAX_QUOTA_BYTES);
    let path = exe_root.join("config").join("sandbox.json");
    let mut value = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let Some(object) = value.as_object_mut() else {
        return serde_json::json!({
            "ok": false,
            "quotaBytes": quota_bytes(exe_root),
            "error": "config/sandbox.json 不是 JSON 对象，未改动",
        })
        .to_string();
    };
    object.insert("quotaBytes".to_string(), serde_json::json!(quota));
    if let Some(dir) = path.parent() {
        if let Err(e) = fs::create_dir_all(dir) {
            return serde_json::json!({
                "ok": false,
                "quotaBytes": quota_bytes(exe_root),
                "error": format!("建不了 config 目录：{e}"),
            })
            .to_string();
        }
    }
    let text = match serde_json::to_string_pretty(&value) {
        Ok(text) => text,
        Err(e) => {
            return serde_json::json!({
                "ok": false,
                "quotaBytes": quota_bytes(exe_root),
                "error": format!("序列化失败：{e}"),
            })
            .to_string()
        }
    };
    if let Err(e) = fs::write(&path, text) {
        return serde_json::json!({
            "ok": false,
            "quotaBytes": quota_bytes(exe_root),
            "error": format!("写 config/sandbox.json 失败：{e}"),
        })
        .to_string();
    }
    serde_json::json!({ "ok": true, "quotaBytes": quota, "error": "" }).to_string()
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SandboxStatus {
    pub path: String,
    pub exists: bool,
    pub used_bytes: u64,
    pub quota_bytes: u64,
    pub file_count: u64,
    pub dir_count: u64,
    /// 条目太多，用量是「至少这么多」（扫描预算用完）。
    pub truncated: bool,
    /// 最近改动的文件（按 mtime 倒序，最多 [`MAX_RECENT`] 条）——
    /// 让预览窗像「虚拟机的磁盘活动」一样能看出 AI 刚写了什么。
    pub recent: Vec<SandboxRecent>,
    pub error: String,
}

/// 最近改动的一条（相对路径 + 大小 + mtime）。
#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SandboxRecent {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub modified_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxEntry {
    /// 相对沙箱根、以 `/` 分隔（预览 / 删除都按它回传）。
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// 修改时间（毫秒时间戳；读不到为 0）。
    pub modified_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxList {
    pub path: String,
    pub entries: Vec<SandboxEntry>,
    pub used_bytes: u64,
    pub quota_bytes: u64,
    pub truncated: bool,
    pub error: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxFile {
    pub ok: bool,
    /// `text` / `image` / `binary`
    pub kind: String,
    pub path: String,
    pub name: String,
    pub size: u64,
    pub truncated: bool,
    /// 文本内容（只有 `kind == "text"` 时有；已过 UTF-8 容错）。
    pub text: String,
    pub error: String,
}

fn modified_ms(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 目录里第一层看得见的条目（目录在前，各自按名字排序）。
fn sorted_children(dir: &Path) -> Vec<fs::DirEntry> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<fs::DirEntry> = read
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            // 跳过 Windows 的隐藏系统文件（desktop.ini 之类），它们不是模型写的。
            !name.starts_with('.') && !name.eq_ignore_ascii_case("desktop.ini")
        })
        .collect();
    out.sort_by(|a, b| {
        let ad = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let bd = b.file_type().map(|t| t.is_dir()).unwrap_or(false);
        bd.cmp(&ad)
            .then_with(|| a.file_name().to_string_lossy().to_lowercase().cmp(
                &b.file_name().to_string_lossy().to_lowercase(),
            ))
    });
    out
}

/// 统计用量（有扫描预算；超预算时 `truncated = true`）。
fn scan_usage(root: &Path) -> (u64, u64, u64, bool) {
    let mut used: u64 = 0;
    let mut files: u64 = 0;
    let mut dirs: u64 = 0;
    let mut visited: u64 = 0;
    let mut truncated = false;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in sorted_children(&dir) {
            visited += 1;
            if visited > MAX_SCAN_ENTRIES {
                truncated = true;
                return (used, files, dirs, truncated);
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                dirs += 1;
                stack.push(entry.path());
            } else if meta.is_file() {
                files += 1;
                used += meta.len();
            }
        }
    }
    (used, files, dirs, truncated)
}

/// 收集最近改动的文件（按 mtime 倒序；预算与用量统计同一套上限）。
fn scan_recent(root: &Path) -> Vec<SandboxRecent> {
    let mut out: Vec<SandboxRecent> = Vec::new();
    let mut visited: u64 = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in sorted_children(&dir) {
            visited += 1;
            if visited > MAX_SCAN_ENTRIES {
                out.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
                out.truncate(MAX_RECENT);
                return out;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let modified = modified_ms(&meta);
            if modified == 0 {
                continue;
            }
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| path.to_string_lossy().to_string());
            out.push(SandboxRecent {
                path: rel,
                name: entry.file_name().to_string_lossy().to_string(),
                size: meta.len(),
                modified_ms: modified,
            });
        }
    }
    out.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    out.truncate(MAX_RECENT);
    out
}

pub fn status_json(exe_root: &Path) -> String {
    let dir = workdir(exe_root);
    let exists = dir.is_dir();
    let (used, files, dirs, truncated) = if exists {
        scan_usage(&dir)
    } else {
        (0, 0, 0, false)
    };
    let recent = if exists {
        scan_recent(&dir)
    } else {
        Vec::new()
    };
    let status = SandboxStatus {
        path: dir.to_string_lossy().to_string(),
        exists,
        used_bytes: used,
        quota_bytes: quota_bytes(exe_root),
        file_count: files,
        dir_count: dirs,
        truncated,
        recent,
        error: String::new(),
    };
    serde_json::to_string(&status).unwrap_or_else(|_| "{}".to_string())
}

/// 列沙箱盘（目录在前；最多 4 层 / 500 条）。
pub fn list_json(exe_root: &Path) -> String {
    let dir = workdir(exe_root);
    let mut entries: Vec<SandboxEntry> = Vec::new();
    let mut truncated = false;
    if dir.is_dir() {
        let mut stack: Vec<(PathBuf, String, u32)> = vec![(dir.clone(), String::new(), 0)];
        while let Some((current, prefix, depth)) = stack.pop() {
            if depth >= MAX_DEPTH {
                truncated = true;
                continue;
            }
            for entry in sorted_children(&current) {
                if entries.len() >= MAX_ENTRIES {
                    truncated = true;
                    break;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let rel = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                let is_dir = meta.is_dir();
                entries.push(SandboxEntry {
                    path: rel.clone(),
                    name,
                    is_dir,
                    size: if is_dir { 0 } else { meta.len() },
                    modified_ms: modified_ms(&meta),
                });
                if is_dir {
                    stack.push((entry.path(), rel, depth + 1));
                }
            }
            if entries.len() >= MAX_ENTRIES {
                break;
            }
        }
    }
    // 广度优先的栈顺序会把子目录条目插到后面；按路径排一次，界面更稳定。
    entries.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
    let (used, _, _, scan_truncated) = if dir.is_dir() {
        scan_usage(&dir)
    } else {
        (0, 0, 0, false)
    };
    let list = SandboxList {
        path: dir.to_string_lossy().to_string(),
        entries,
        used_bytes: used,
        quota_bytes: quota_bytes(exe_root),
        truncated: truncated || scan_truncated,
        error: String::new(),
    };
    serde_json::to_string(&list).unwrap_or_else(|_| "{}".to_string())
}

fn file_kind(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    for ext in [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico"] {
        if lower.ends_with(ext) {
            return "image";
        }
    }
    "text"
}

/// 只读预览一个文件（相对沙箱根；越界一律拒绝）。
pub fn read_json(exe_root: &Path, rel: &str) -> String {
    let root = workdir(exe_root);
    let fail = |reason: String| {
        serde_json::to_string(&SandboxFile {
            ok: false,
            kind: "binary".into(),
            path: rel.to_string(),
            name: rel.rsplit('/').next().unwrap_or(rel).to_string(),
            size: 0,
            truncated: false,
            text: String::new(),
            error: reason,
        })
        .unwrap_or_else(|_| "{}".to_string())
    };
    let Some(candidate) = safe_join(&root, rel) else {
        return fail("路径越出沙箱盘，已拒绝".into());
    };
    let Ok(real_root) = root.canonicalize() else {
        return fail("沙箱盘不存在".into());
    };
    let Ok(real_file) = candidate.canonicalize() else {
        return fail("文件不存在".into());
    };
    if !real_file.starts_with(&real_root) {
        return fail("路径越出沙箱盘，已拒绝".into());
    }
    let Ok(meta) = fs::metadata(&real_file) else {
        return fail("读文件信息失败".into());
    };
    if !meta.is_file() {
        return fail("不是文件".into());
    }
    let name = real_file
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let kind = file_kind(&name);
    if kind == "image" {
        // 图片只回路径，界面直接 Image.file（不走 base64，省内存）。
        return serde_json::to_string(&SandboxFile {
            ok: true,
            kind: kind.into(),
            path: real_file.to_string_lossy().to_string(),
            name,
            size: meta.len(),
            truncated: false,
            text: String::new(),
            error: String::new(),
        })
        .unwrap_or_else(|_| "{}".to_string());
    }
    let Ok(mut file) = fs::File::open(&real_file) else {
        return fail("打开文件失败".into());
    };
    let mut buffer = Vec::new();
    if let Err(e) = file
        .by_ref()
        .take(MAX_PREVIEW_BYTES)
        .read_to_end(&mut buffer)
    {
        return fail(format!("读取失败：{e}"));
    }
    let truncated = meta.len() > MAX_PREVIEW_BYTES;
    match String::from_utf8(buffer) {
        Ok(text) => serde_json::to_string(&SandboxFile {
            ok: true,
            kind: "text".into(),
            path: real_file.to_string_lossy().to_string(),
            name,
            size: meta.len(),
            truncated,
            text,
            error: String::new(),
        })
        .unwrap_or_else(|_| "{}".to_string()),
        Err(_) => {
            // 不是 UTF-8 文本：如实标 binary，不硬转码（GBK 等交给外部程序）。
            serde_json::to_string(&SandboxFile {
                ok: true,
                kind: "binary".into(),
                path: real_file.to_string_lossy().to_string(),
                name,
                size: meta.len(),
                truncated: false,
                text: String::new(),
                error: String::new(),
            })
            .unwrap_or_else(|_| "{}".to_string())
        }
    }
}

/// 清空沙箱盘（删全部子项，保留根目录）。返回 JSON：`ok` / `removed` / `failed` / `error`。
pub fn clear_json(exe_root: &Path) -> String {
    let root = workdir(exe_root);
    if !root.is_dir() {
        return serde_json::json!({ "ok": true, "removed": 0, "failed": 0, "error": "" })
            .to_string();
    }
    let mut removed = 0u64;
    let mut failed = 0u64;
    let mut first_error = String::new();
    for entry in sorted_children(&root) {
        let path = entry.path();
        let result = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        match result {
            Ok(()) => removed += 1,
            Err(e) => {
                failed += 1;
                if first_error.is_empty() {
                    first_error = format!("{}：{e}", path.display());
                }
            }
        }
    }
    serde_json::json!({
        "ok": failed == 0,
        "removed": removed,
        "failed": failed,
        "error": first_error,
    })
    .to_string()
}

// ── 文件操作（预览区＝「小 VSCode」：新建 / 重命名 / 删除）──────────────
//
// 三条纪律（与只读预览同源）：
//   · 一切路径都走 `safe_join` + `canonicalize` 双闸（`..` / 符号链接都出不去）；
//   · **不静默覆盖**：目标已存在一律拒绝（新建 / 重命名都一样），让用户先改名；
//   · 目录只允许「删空目录」：递归删一棵树不可撤销，宁可让用户自己清空再删。

/// 新建文本文件的体积上限（新文件就是用户手动敲的那点内容）。
pub const MAX_WRITE_BYTES: u64 = 1024 * 1024;

fn op_result(ok: bool, path: &str, error: String) -> String {
    serde_json::json!({ "ok": ok, "path": path, "error": error }).to_string()
}

/// 沙箱根的 canonicalize 形式（不存在就建）。
fn sandbox_root(exe_root: &Path) -> Result<PathBuf, String> {
    let root = prepare(exe_root)?;
    root.canonicalize().map_err(|_| "沙箱盘不存在".to_string())
}

/// 把相对路径解析成沙箱内的绝对路径（只做第一道闸：`safe_join`）。
fn target_in(root_real: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim().replace('\\', "/");
    let rel = rel.trim_matches('/');
    if rel.is_empty() {
        return Err("路径为空".into());
    }
    safe_join(root_real, rel).ok_or_else(|| "路径越出沙箱盘，已拒绝".to_string())
}

/// 第二道闸：父目录必须真实存在且落在沙箱内（挡符号链接 / 大小写盘符差异）。
fn ensure_parent_inside(root_real: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().ok_or_else(|| "路径不合法".to_string())?;
    let real_parent = parent
        .canonicalize()
        .map_err(|_| "上级目录不存在".to_string())?;
    if !real_parent.starts_with(root_real) {
        return Err("路径越出沙箱盘，已拒绝".into());
    }
    Ok(())
}

/// 新建文本文件。**同名已存在就拒绝**（不覆盖模型刚写的东西）。
pub fn write_json(exe_root: &Path, rel: &str, text: &str) -> String {
    if text.len() as u64 > MAX_WRITE_BYTES {
        return op_result(
            false,
            rel,
            format!("内容超过 {} KiB 上限", MAX_WRITE_BYTES / 1024),
        );
    }
    let root = match sandbox_root(exe_root) {
        Ok(root) => root,
        Err(error) => return op_result(false, rel, error),
    };
    let target = match target_in(&root, rel) {
        Ok(target) => target,
        Err(error) => return op_result(false, rel, error),
    };
    if target.exists() {
        return op_result(false, rel, "同名文件已存在（不会覆盖）".into());
    }
    if let Err(error) = ensure_parent_inside(&root, &target) {
        return op_result(false, rel, error);
    }
    match fs::write(&target, text) {
        Ok(()) => op_result(true, &target.to_string_lossy(), String::new()),
        Err(e) => op_result(false, rel, format!("写入失败：{e}")),
    }
}

/// 新建目录（上级目录必须已存在；同名已存在就拒绝）。
pub fn mkdir_json(exe_root: &Path, rel: &str) -> String {
    let root = match sandbox_root(exe_root) {
        Ok(root) => root,
        Err(error) => return op_result(false, rel, error),
    };
    let target = match target_in(&root, rel) {
        Ok(target) => target,
        Err(error) => return op_result(false, rel, error),
    };
    if target.exists() {
        return op_result(false, rel, "同名条目已存在".into());
    }
    if let Err(error) = ensure_parent_inside(&root, &target) {
        return op_result(false, rel, error);
    }
    match fs::create_dir(&target) {
        Ok(()) => op_result(true, &target.to_string_lossy(), String::new()),
        Err(e) => op_result(false, rel, format!("创建目录失败：{e}")),
    }
}

/// 重命名 / 移动（同一沙箱内；目标不存在）。
pub fn rename_json(exe_root: &Path, from: &str, to: &str) -> String {
    let root = match sandbox_root(exe_root) {
        Ok(root) => root,
        Err(error) => return op_result(false, from, error),
    };
    let src = match target_in(&root, from) {
        Ok(src) => src,
        Err(error) => return op_result(false, from, error),
    };
    let dst = match target_in(&root, to) {
        Ok(dst) => dst,
        Err(error) => return op_result(false, from, error),
    };
    let Ok(real_src) = src.canonicalize() else {
        return op_result(false, from, "源条目不存在".into());
    };
    if !real_src.starts_with(&root) || real_src == root {
        return op_result(false, from, "路径越出沙箱盘，已拒绝".into());
    }
    if dst.exists() {
        return op_result(false, from, "目标已存在（不会覆盖）".into());
    }
    if let Err(error) = ensure_parent_inside(&root, &dst) {
        return op_result(false, from, error);
    }
    match fs::rename(&real_src, &dst) {
        Ok(()) => op_result(true, &dst.to_string_lossy(), String::new()),
        Err(e) => op_result(false, from, format!("重命名失败：{e}")),
    }
}

/// 删除文件 / 空目录（沙箱根不可删；非空目录如实拒绝）。
pub fn delete_json(exe_root: &Path, rel: &str) -> String {
    let root = match sandbox_root(exe_root) {
        Ok(root) => root,
        Err(error) => return op_result(false, rel, error),
    };
    let target = match target_in(&root, rel) {
        Ok(target) => target,
        Err(error) => return op_result(false, rel, error),
    };
    let Ok(real) = target.canonicalize() else {
        return op_result(false, rel, "条目不存在".into());
    };
    if !real.starts_with(&root) || real == root {
        return op_result(false, rel, "不能删除沙箱盘根目录".into());
    }
    let result = if real.is_dir() {
        fs::remove_dir(&real).map_err(|_| "目录非空（先清空再删）".to_string())
    } else {
        fs::remove_file(&real).map_err(|e| format!("删除失败：{e}"))
    };
    match result {
        Ok(()) => op_result(true, rel, String::new()),
        Err(error) => op_result(false, rel, error),
    }
}

// ── 预览窗标签队列（AI 工具 → 聊天面板 → 预览窗）─────────────────────
//
// 聊天面板与预览窗可能跑在**不同的 FlutterEngine**，拿不到彼此的内存；但
// `lunac_core.dll` 是同一进程里的一份静态区，所以用进程内队列转交：
// 聊天面板收到 agent 的 `Preview` 工具事件后 `push`，预览窗每秒 `take` 一次。
const MAX_PREVIEW_TABS: usize = 16;
static PREVIEW_TABS: Mutex<Vec<serde_json::Value>> = Mutex::new(Vec::new());

/// 入队一条预览标签请求：`{"title":"…","kind":"app|file|url|workspace","target":"…"}`。
pub fn preview_push(json: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return serde_json::json!({ "ok": false, "error": "预览请求不是合法 JSON" }).to_string();
    };
    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let kind = value
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("app")
        .trim();
    let target = value
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if title.is_empty() {
        return serde_json::json!({ "ok": false, "error": "预览标签缺少 title" }).to_string();
    }
    if !matches!(kind, "app" | "file" | "url" | "workspace" | "change") {
        return serde_json::json!({
            "ok": false,
            "error": format!("未知的预览类型 {kind}"),
        })
        .to_string();
    }
    if kind != "workspace" && target.is_empty() {
        return serde_json::json!({ "ok": false, "error": "预览标签缺少 target" }).to_string();
    }
    let Ok(mut list) = PREVIEW_TABS.lock() else {
        return serde_json::json!({ "ok": false, "error": "预览队列不可用" }).to_string();
    };
    if list.len() >= MAX_PREVIEW_TABS {
        list.remove(0);
    }
    list.push(serde_json::json!({
        "title": title,
        "kind": kind,
        "target": target,
    }));
    serde_json::json!({ "ok": true }).to_string()
}

/// 取走全部待开的预览标签（排空）。
pub fn preview_take() -> String {
    let Ok(mut list) = PREVIEW_TABS.lock() else {
        return serde_json::json!({ "tabs": [] }).to_string();
    };
    let tabs = std::mem::take(&mut *list);
    serde_json::json!({ "tabs": tabs }).to_string()
}

// ── 「添加到对话」队列（预览窗 → 聊天面板 / 主窗）────────────────────
//
// 与预览标签队列同一条理由：预览窗与聊天面板可能在不同 FlutterEngine。
// 谁先轮询到就谁消费：聊天面板开着就加进附件栏；没开则主窗拿去开对话。
const MAX_CHAT_ATTACH: usize = 32;
static CHAT_ATTACH: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// 入队若干绝对路径：`{"paths":["C:\\…"]}`。
pub fn chat_attach_push(json: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return serde_json::json!({ "ok": false, "error": "请求不是合法 JSON" }).to_string();
    };
    let Some(items) = value.get("paths").and_then(|v| v.as_array()) else {
        return serde_json::json!({ "ok": false, "error": "缺少 paths 数组" }).to_string();
    };
    let Ok(mut list) = CHAT_ATTACH.lock() else {
        return serde_json::json!({ "ok": false, "error": "附件队列不可用" }).to_string();
    };
    let mut added = 0usize;
    for item in items {
        let Some(path) = item.as_str().map(str::trim).filter(|p| !p.is_empty()) else {
            continue;
        };
        if list.iter().any(|existing| existing.eq_ignore_ascii_case(path)) {
            continue;
        }
        if list.len() >= MAX_CHAT_ATTACH {
            list.remove(0);
        }
        list.push(path.to_string());
        added += 1;
    }
    serde_json::json!({ "ok": true, "added": added }).to_string()
}

/// 取走全部待加入对话的文件（排空）。
pub fn chat_attach_take() -> String {
    let Ok(mut list) = CHAT_ATTACH.lock() else {
        return serde_json::json!({ "paths": [] }).to_string();
    };
    let paths = std::mem::take(&mut *list);
    serde_json::json!({ "paths": paths }).to_string()
}

// ── 工作区指纹（命令类产出识别，§待办 3-28.13 ②，2026-11-16）──────────
//
// 为什么需要它：`changedFilesOf` 只认 `Write` / `Edit` 的精确路径；`Cmd` 里
// `python gen.py` / `> out.txt` / `Out-File` 产生的文件完全看不见，产出物卡就会漏。
// 口径：**抓帧只看元数据**（相对路径 → `大小:改动时间`），前后各抓一次、比出
// 新增 / 改动 / 删除 —— 不做 AST 解析（命令行的写目标是不可静态判定的）。
//
// 边界（如实记）：
//   · 抓帧是**采样**：跳过依赖 / 构建目录，条目上限 2000、深度 6；`truncated=true`
//     时界面不能宣称「没有别的改动」；
//   · 只用于「列出来 + 看 diff」；**不产生可还原快照**（改动已经发生，没有前像），
//     所以这些文件在界面上标「命令产出」而不是「可还原」；
//   · 目录很大时抓帧会慢（毫秒到百毫秒级）⇒ 只在命令类工具前后各抓一次。

/// 指纹最多收录多少个条目（超过就标 `truncated`）。
const FINGERPRINT_MAX_ENTRIES: usize = 2000;
/// 指纹最多递归几层（足够看清项目根下的产出，又不至于扫穿整个 node_modules）。
const FINGERPRINT_MAX_DEPTH: usize = 6;
/// 指纹跳过的目录（依赖 / 构建产物：它们的变化不是「模型的产出」）。
const FINGERPRINT_SKIP_DIRS: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    "node_modules",
    "target",
    "build",
    "dist",
    "out",
    ".dart_tool",
    ".next",
    ".nuxt",
    "__pycache__",
    ".venv",
    "venv",
    ".mypy_cache",
    ".pytest_cache",
    ".gradle",
    ".idea",
];

/// 抓一次工作区指纹：`{ok, truncated, entries: {相对路径: "大小:改动ms"}}`。
///
/// 路径统一用 `\` 分隔（Windows 口径），与 `_absoluteAgentPath` 的拼接一致。
pub fn fingerprint_json(dir: &Path) -> String {
    if !dir.is_dir() {
        return serde_json::json!({
            "ok": false,
            "truncated": false,
            "error": format!("工作区不存在：{}", dir.display()),
            "entries": {},
        })
        .to_string();
    }
    let mut entries = serde_json::Map::new();
    let mut visited: usize = 0;
    let mut truncated = false;
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    'walk: while let Some((current, depth)) = stack.pop() {
        let Ok(read) = fs::read_dir(&current) else {
            continue;
        };
        for entry in read.flatten() {
            visited += 1;
            if visited > FINGERPRINT_MAX_ENTRIES {
                truncated = true;
                break 'walk;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                let skip = FINGERPRINT_SKIP_DIRS
                    .iter()
                    .any(|d| d.eq_ignore_ascii_case(&name));
                if !skip && depth + 1 <= FINGERPRINT_MAX_DEPTH {
                    stack.push((entry.path(), depth + 1));
                }
                continue;
            }
            if !meta.is_file() {
                continue; // 符号链接 / 设备文件不跟
            }
            let path = entry.path();
            let Ok(rel) = path.strip_prefix(dir) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('/', "\\");
            entries.insert(
                rel,
                serde_json::json!(format!("{}:{}", meta.len(), modified_ms(&meta))),
            );
        }
    }
    serde_json::json!({
        "ok": true,
        "truncated": truncated,
        "entries": entries,
    })
    .to_string()
}

/// 对**生效中的工作区**抓指纹（`chat_settings::resolve_workspace` 的单一判定）。
pub fn workspace_fingerprint_json(exe_root: &Path) -> String {
    let prefs = crate::chat_settings::load(exe_root);
    let (dir, _) = crate::chat_settings::resolve_workspace(exe_root, &prefs);
    fingerprint_json(&dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "lunac-v2-sandbox-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn fingerprint_captures_new_files_and_skips_heavy_dirs() {
        let root = tmp_root("fingerprint");
        let dir = root.join("ws");
        fs::create_dir_all(dir.join("node_modules").join("pkg")).unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), "one").unwrap();
        fs::write(dir.join("sub").join("keep.md"), "k").unwrap();
        fs::write(dir.join("node_modules").join("pkg").join("i.js"), "x").unwrap();

        let before: serde_json::Value = serde_json::from_str(&fingerprint_json(&dir)).unwrap();
        assert_eq!(before["ok"], true);
        assert_eq!(before["truncated"], false);
        let entries = before["entries"].as_object().unwrap();
        assert!(entries.contains_key("a.txt"));
        assert!(entries.contains_key("sub\\keep.md"));
        assert!(
            !entries.keys().any(|k| k.starts_with("node_modules")),
            "依赖目录不进指纹：{entries:?}"
        );

        fs::write(dir.join("b.txt"), "new").unwrap();
        fs::write(dir.join("a.txt"), "one-two").unwrap();
        let after: serde_json::Value = serde_json::from_str(&fingerprint_json(&dir)).unwrap();
        let entries = after["entries"].as_object().unwrap();
        assert!(entries.contains_key("b.txt"), "新增文件必须出现");
        assert_ne!(
            before["entries"]["a.txt"], after["entries"]["a.txt"],
            "改过的文件指纹必须变化"
        );

        let missing: serde_json::Value =
            serde_json::from_str(&fingerprint_json(&dir.join("nope"))).unwrap();
        assert_eq!(missing["ok"], false);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn workdir_is_trans_storage_and_prepare_creates_it() {
        let root = tmp_root("prepare");
        let dir = prepare(&root).unwrap();
        assert_eq!(dir, root.join("temp").join("transStorage"));
        assert!(dir.is_dir());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn quota_defaults_and_is_clamped() {
        let root = tmp_root("quota");
        assert_eq!(quota_bytes(&root), DEFAULT_QUOTA_BYTES);
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config").join("sandbox.json"),
            r#"{"quotaBytes": 1}"#,
        )
        .unwrap();
        assert_eq!(quota_bytes(&root), MIN_QUOTA_BYTES);
        fs::write(
            root.join("config").join("sandbox.json"),
            r#"{"quotaBytes": 123456789}"#,
        )
        .unwrap();
        assert_eq!(quota_bytes(&root), 123456789);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn set_quota_clamps_and_keeps_other_keys() {
        let root = tmp_root("setquota");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config").join("sandbox.json"),
            r#"{"note":"keep","quotaBytes":1}"#,
        )
        .unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&set_quota_json(&root, 700)).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["quotaBytes"], MIN_QUOTA_BYTES);
        assert_eq!(quota_bytes(&root), MIN_QUOTA_BYTES);

        let raw: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(root.join("config").join("sandbox.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(raw["note"], "keep", "其它字段必须保留");

        // 超出上限也夹回去，不拒绝。
        set_quota_json(&root, u64::MAX);
        assert_eq!(quota_bytes(&root), MAX_QUOTA_BYTES);

        // 目录不存在时也会建出来。
        let fresh = tmp_root("setquota-fresh");
        let value: serde_json::Value =
            serde_json::from_str(&set_quota_json(&fresh, 1024 * 1024 * 1024)).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(quota_bytes(&fresh), 1024 * 1024 * 1024);
        let _ = fs::remove_dir_all(&fresh);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn list_reports_files_and_usage() {
        let root = tmp_root("list");
        let dir = prepare(&root).unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), b"hello").unwrap();
        fs::write(dir.join("sub").join("b.py"), b"print(1)").unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&list_json(&root)).unwrap();
        let entries = value["entries"].as_array().unwrap();
        let paths: Vec<&str> = entries
            .iter()
            .map(|e| e["path"].as_str().unwrap())
            .collect();
        assert!(paths.contains(&"a.txt"));
        assert!(paths.contains(&"sub"));
        assert!(paths.contains(&"sub/b.py"));
        assert_eq!(value["usedBytes"].as_u64().unwrap(), 13);
        assert_eq!(value["quotaBytes"].as_u64().unwrap(), DEFAULT_QUOTA_BYTES);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn read_previews_text_and_rejects_traversal() {
        let root = tmp_root("read");
        let dir = prepare(&root).unwrap();
        fs::write(dir.join("note.md"), b"# hi").unwrap();
        let text: serde_json::Value =
            serde_json::from_str(&read_json(&root, "note.md")).unwrap();
        assert_eq!(text["ok"], true);
        assert_eq!(text["kind"], "text");
        assert_eq!(text["text"], "# hi");

        for bad in ["../config/sandbox.json", "C:/Windows/win.ini", "/etc/passwd"] {
            let value: serde_json::Value =
                serde_json::from_str(&read_json(&root, bad)).unwrap();
            assert_eq!(value["ok"], false, "越界必须拒绝：{bad}");
        }
        let missing: serde_json::Value =
            serde_json::from_str(&read_json(&root, "nope.txt")).unwrap();
        assert_eq!(missing["ok"], false);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_removes_children_but_keeps_the_root() {
        let root = tmp_root("clear");
        let dir = prepare(&root).unwrap();
        fs::write(dir.join("x.txt"), b"x").unwrap();
        fs::create_dir_all(dir.join("d")).unwrap();
        fs::write(dir.join("d").join("y.txt"), b"y").unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&clear_json(&root)).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["removed"].as_u64().unwrap(), 2);
        assert!(dir.is_dir());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn status_reflects_usage() {
        let root = tmp_root("status");
        let dir = prepare(&root).unwrap();
        fs::write(dir.join("a.bin"), vec![0u8; 1024]).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&status_json(&root)).unwrap();
        assert_eq!(value["exists"], true);
        assert_eq!(value["usedBytes"].as_u64().unwrap(), 1024);
        assert_eq!(value["fileCount"].as_u64().unwrap(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn status_lists_recent_files_newest_first() {
        let root = tmp_root("recent");
        let dir = prepare(&root).unwrap();
        fs::write(dir.join("old.txt"), b"old").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::create_dir_all(dir.join("out")).unwrap();
        fs::write(dir.join("out").join("new.txt"), b"new").unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&status_json(&root)).unwrap();
        let recent = value["recent"].as_array().unwrap();
        assert_eq!(recent.len(), 2, "两个文件都要列出来");
        assert_eq!(recent[0]["name"], "new.txt", "最近改动的排最前");
        assert_eq!(recent[0]["path"], "out/new.txt", "相对路径用 / 分隔");
        assert_eq!(recent[0]["size"].as_u64().unwrap(), 3);
        assert!(recent[0]["modifiedMs"].as_u64().unwrap() > 0);
        assert_eq!(recent[1]["name"], "old.txt");

        // 空沙箱也要给空数组（界面按「没有新文件」渲染）。
        let empty = tmp_root("recent-empty");
        prepare(&empty).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&status_json(&empty)).unwrap();
        assert!(value["recent"].as_array().unwrap().is_empty());
        let _ = fs::remove_dir_all(&empty);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn preview_queue_roundtrips_and_rejects_junk() {
        // 先排空，避免与其他用例并行时互相干扰。
        let _ = preview_take();

        let bad = preview_push("not json");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&bad).unwrap()["ok"],
            false
        );
        let no_title = preview_push(r#"{"kind":"app","target":"notepad.exe"}"#);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&no_title).unwrap()["ok"],
            false
        );
        let bad_kind = preview_push(r#"{"title":"x","kind":"video","target":"a.mp4"}"#);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&bad_kind).unwrap()["ok"],
            false
        );

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&preview_push(
                r#"{"title":"预览: 记事本","kind":"app","target":"notepad.exe"}"#
            ))
            .unwrap()["ok"],
            true
        );
        let taken = preview_take();
        let tabs = serde_json::from_str::<serde_json::Value>(&taken).unwrap();
        assert_eq!(tabs["tabs"].as_array().unwrap().len(), 1);
        assert_eq!(tabs["tabs"][0]["title"], "预览: 记事本");
        assert_eq!(tabs["tabs"][0]["kind"], "app");
        // 取走即排空。
        let again = preview_take();
        assert!(serde_json::from_str::<serde_json::Value>(&again).unwrap()["tabs"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    /// 「添加到对话」队列：去重（大小写不敏感）/ 空串跳过 / 取走即排空。
    #[test]
    fn chat_attach_queue_dedupes_and_drains() {
        let _ = chat_attach_take();
        let bad: serde_json::Value = serde_json::from_str(&chat_attach_push("not json")).unwrap();
        assert_eq!(bad["ok"], false);
        let missing: serde_json::Value =
            serde_json::from_str(&chat_attach_push(r#"{"path":"C:\\a.txt"}"#)).unwrap();
        assert_eq!(missing["ok"], false);

        let pushed: serde_json::Value = serde_json::from_str(&chat_attach_push(
            r#"{"paths":["C:\\a.txt","c:\\A.TXT","   ","C:\\b.txt"]}"#,
        ))
        .unwrap();
        assert_eq!(pushed["added"], 2, "同一条路径（大小写不同）只算一次");

        let taken: serde_json::Value = serde_json::from_str(&chat_attach_take()).unwrap();
        assert_eq!(taken["paths"].as_array().unwrap().len(), 2);
        let again: serde_json::Value = serde_json::from_str(&chat_attach_take()).unwrap();
        assert!(again["paths"].as_array().unwrap().is_empty());
    }

    fn ok_of(json: &str) -> (bool, String) {
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        (
            value["ok"].as_bool().unwrap_or(false),
            value["error"].as_str().unwrap_or("").to_string(),
        )
    }

    /// 预览区「小 VSCode」的三类操作：建目录 → 建文件 → 改名 → 删。
    #[test]
    fn file_ops_create_rename_and_delete_inside_the_sandbox() {
        let exe_root = tmp_root("ops");
        let (ok, error) = ok_of(&mkdir_json(&exe_root, "notes"));
        assert!(ok, "建目录该成功：{error}");

        let (ok, error) = ok_of(&write_json(&exe_root, "notes/a.txt", "hello"));
        assert!(ok, "建文件该成功：{error}");
        let path = workdir(&exe_root).join("notes").join("a.txt");
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");

        // 同名不覆盖：再建一次必须失败，内容保持原样。
        let (ok, error) = ok_of(&write_json(&exe_root, "notes/a.txt", "world"));
        assert!(!ok && error.contains("已存在"), "不该覆盖已存在文件：{error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");

        let (ok, error) = ok_of(&rename_json(&exe_root, "notes/a.txt", "notes/b.txt"));
        assert!(ok, "改名该成功：{error}");
        assert!(!path.exists());
        assert_eq!(
            fs::read_to_string(workdir(&exe_root).join("notes").join("b.txt")).unwrap(),
            "hello"
        );

        // 非空目录删不掉（如实说原因），空目录可以。
        let (ok, error) = ok_of(&delete_json(&exe_root, "notes"));
        assert!(!ok && error.contains("非空"), "非空目录该被拒绝：{error}");
        assert!(ok_of(&delete_json(&exe_root, "notes/b.txt")).0);
        assert!(ok_of(&delete_json(&exe_root, "notes")).0);
        assert!(!workdir(&exe_root).join("notes").exists());
    }

    /// 三道闸：越界 → 上级目录不存在 → 根目录不可删 / 体积上限。
    #[test]
    fn file_ops_reject_escapes_missing_parents_and_oversize() {
        let exe_root = tmp_root("ops-gates");
        for bad in ["../evil.txt", "a/../../evil.txt", r"..\evil.txt"] {
            let (ok, error) = ok_of(&write_json(&exe_root, bad, "x"));
            assert!(!ok, "{bad} 必须被拒绝");
            assert!(
                error.contains("越出") || error.contains("路径"),
                "{bad} 的拒因该指向越界，实际：{error}"
            );
        }
        // 上级目录不存在 ⇒ 不偷偷 recursive 创建。
        let (ok, error) = ok_of(&write_json(&exe_root, "nope/a.txt", "x"));
        assert!(!ok && error.contains("上级目录"), "实际：{error}");
        // 根目录不能删。
        let (ok, error) = ok_of(&delete_json(&exe_root, "."));
        assert!(!ok, "根目录不该被删：{error}");
        // 超过 1 MiB 直接拒。
        let huge = "x".repeat(MAX_WRITE_BYTES as usize + 1);
        let (ok, error) = ok_of(&write_json(&exe_root, "big.txt", &huge));
        assert!(!ok && error.contains("上限"), "实际：{error}");
        assert!(!workdir(&exe_root).join("big.txt").exists());
    }
}
