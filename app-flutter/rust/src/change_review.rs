//! 会话内「AI 改过的文件」快照 / 审阅 / 撤销（2026-11-14，用户要求）。
//!
//! 数据目录：`<exe 根>/temp/change-review/`
//!   · `<key>.orig` —— 改动**前**的完整字节（原本不存在 = 空文件）
//!   · `<key>.meta` —— `{"path":"C:\\…","existed":true|false,"turn":N}`
//!
//! `key` = `turn` 前缀 + FNV-1a 64(小写绝对路径)。用小写是因为 Windows 路径
//! 大小写不敏感 —— 同一文件的不同写法必须落到同一份快照。
//!
//! **按回合快照（与 v1 `sessionFileSnapshots` 同口径）**：`turn` = 采集时已完成的助手
//! 消息条数。同一回合里同一文件只采一次；不同回合各留一份「改前内容」——
//! 这样「回退到第 K 回合」才能把该回合之后的改动逐层还原到回退点（见 [`revert_since`]）。
//!
//! 口径：
//!   · `snapshot` 按 `(turn, path)` 去重，不覆盖已有；
//!   · `diff` 用**最早**那份原件（会话开头 → 现在）；
//!   · `revert`：原本存在 ⇒ 写回最早原件；原本不存在（AI 新建）⇒ 删除当前文件；
//!     撤完把该文件所有回合的快照一起丢掉（回到本次会话开头）；
//!   · `revert_since(kept)`：只还原 `turn >= kept` 的快照，按回合**倒序**逐条写回
//!     —— 同一文件被改过多次时，最后落在回退点那一刻的内容（v1 `revertFileSnapshots`）；
//!   · `accept`：删掉快照（用户确认保留改动），文件本身不动。
//!
//! **不碰网络、不碰注册表**；只读写 `<exe 根>/temp/change-review/` 与用户明确
//! 批准过的那几个文件。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use serde_json::Value;

/// 单侧行数超过它就不再跑 LCS（避免 O(n·m) 把界面卡住），退化为整块增删。
const DIFF_MAX_LINES: usize = 1200;
/// 返回给前端的最大行数（超出截断并置 `truncated`）。
const DIFF_MAX_OUTPUT: usize = 4000;

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn hash_key(path: &str) -> String {
    format!("{:016x}", fnv1a64(path.to_lowercase().as_bytes()))
}

/// 快照文件名前缀：`turn` 为 0（旧版本 / 未给回合）时沿用纯哈希名，便于迁移旧快照。
fn key_for(path: &str, turn: u32) -> String {
    if turn == 0 {
        hash_key(path)
    } else {
        format!("{turn:04}-{}", hash_key(path))
    }
}

fn store_dir() -> PathBuf {
    crate::exe_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("temp")
        .join("change-review")
}

fn meta_path(store: &Path, key: &str) -> PathBuf {
    store.join(format!("{key}.meta"))
}

fn orig_path(store: &Path, key: &str) -> PathBuf {
    store.join(format!("{key}.orig"))
}

/// 一条快照：回合号 / 文件名 key / 元数据。
struct Snap {
    turn: u32,
    key: String,
    meta: Value,
}

impl Snap {
    fn path(&self) -> &str {
        self.meta.get("path").and_then(Value::as_str).unwrap_or("")
    }

    fn existed(&self) -> bool {
        self.meta
            .get("existed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

/// 扫出全部快照（按回合升序）。坏文件逐条跳过 —— 一个坏 meta 不该让整个列表失效。
fn all_snaps(store: &Path) -> Vec<Snap> {
    let mut snaps = Vec::new();
    let Ok(entries) = fs::read_dir(store) else {
        return snaps;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(key) = name.strip_suffix(".meta") else {
            continue;
        };
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let turn = meta.get("turn").and_then(Value::as_u64).unwrap_or(0) as u32;
        if meta.get("path").and_then(Value::as_str).is_none() {
            continue;
        }
        snaps.push(Snap {
            turn,
            key: key.to_string(),
            meta,
        });
    }
    snaps.sort_by(|a, b| a.turn.cmp(&b.turn).then_with(|| a.key.cmp(&b.key)));
    snaps
}

/// 某个文件的所有快照（按回合升序）。
fn snaps_for(store: &Path, path: &str) -> Vec<Snap> {
    all_snaps(store)
        .into_iter()
        .filter(|snap| snap.path().eq_ignore_ascii_case(path))
        .collect()
}

/// 最早那份快照（`diff` / 单文件 `revert` / `pending` 都用它）。
fn earliest(store: &Path, path: &str) -> Option<Snap> {
    snaps_for(store, path).into_iter().next()
}

/// 采一份「改动前」快照（同一回合同一文件只采一次，不覆盖已有）。
fn snapshot_in(store: &Path, path: &str, turn: u32) -> Result<(), String> {
    let key = key_for(path, turn);
    if meta_path(store, &key).is_file() {
        return Ok(());
    }
    fs::create_dir_all(store).map_err(|e| format!("建快照目录失败：{e}"))?;
    let existed = Path::new(path).is_file();
    let bytes = if existed {
        fs::read(path).map_err(|e| format!("读原文件失败：{e}"))?
    } else {
        Vec::new()
    };
    fs::write(orig_path(store, &key), bytes).map_err(|e| format!("写快照失败：{e}"))?;
    let meta = json!({ "path": path, "existed": existed, "turn": turn });
    fs::write(meta_path(store, &key), meta.to_string())
        .map_err(|e| format!("写快照标记失败：{e}"))?;
    Ok(())
}

/// 一行差异：`same` / `add` / `del`。
fn diff_text(old: &str, new: &str) -> (Vec<(char, String)>, usize, usize, bool) {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();

    let mut prefix = 0usize;
    while prefix < a.len() && prefix < b.len() && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < a.len() - prefix
        && suffix < b.len() - prefix
        && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let mid_a = &a[prefix..a.len() - suffix];
    let mid_b = &b[prefix..b.len() - suffix];

    let mut lines: Vec<(char, String)> = Vec::new();
    for line in &a[..prefix] {
        lines.push((' ', (*line).to_string()));
    }

    let (mut added, mut removed) = (0usize, 0usize);
    let coarse = mid_a.len() > DIFF_MAX_LINES || mid_b.len() > DIFF_MAX_LINES;
    if coarse {
        removed = mid_a.len();
        added = mid_b.len();
        for line in mid_a {
            lines.push(('-', (*line).to_string()));
        }
        for line in mid_b {
            lines.push(('+', (*line).to_string()));
        }
    } else {
        // LCS DP：先算长度，再回溯出逐行结果。
        let n = mid_a.len();
        let m = mid_b.len();
        let stride = m + 1;
        let mut dp = vec![0u32; (n + 1) * stride];
        for x in (0..n).rev() {
            for y in (0..m).rev() {
                dp[x * stride + y] = if mid_a[x] == mid_b[y] {
                    dp[(x + 1) * stride + y + 1] + 1
                } else {
                    dp[(x + 1) * stride + y].max(dp[x * stride + y + 1])
                };
            }
        }
        let (mut x, mut y) = (0usize, 0usize);
        while x < n && y < m {
            if mid_a[x] == mid_b[y] {
                lines.push((' ', mid_a[x].to_string()));
                x += 1;
                y += 1;
            } else if dp[(x + 1) * stride + y] >= dp[x * stride + y + 1] {
                lines.push(('-', mid_a[x].to_string()));
                removed += 1;
                x += 1;
            } else {
                lines.push(('+', mid_b[y].to_string()));
                added += 1;
                y += 1;
            }
        }
        while x < n {
            lines.push(('-', mid_a[x].to_string()));
            removed += 1;
            x += 1;
        }
        while y < m {
            lines.push(('+', mid_b[y].to_string()));
            added += 1;
            y += 1;
        }
    }

    for line in &a[a.len() - suffix..] {
        lines.push((' ', (*line).to_string()));
    }

    let truncated = lines.len() > DIFF_MAX_OUTPUT;
    if truncated {
        lines.truncate(DIFF_MAX_OUTPUT);
    }
    (lines, added, removed, truncated)
}

fn kind_of(sign: char) -> &'static str {
    match sign {
        '+' => "add",
        '-' => "del",
        _ => "same",
    }
}

fn diff_in(store: &Path, path: &str) -> String {
    let Some(snap) = earliest(store, path) else {
        return json!({ "ok": false, "error": "没有改动快照（这个文件不是 AI 改的）" }).to_string();
    };
    let meta = &snap.meta;
    let old_bytes = fs::read(orig_path(store, &snap.key)).unwrap_or_default();
    let new_bytes = fs::read(path).unwrap_or_default();
    if String::from_utf8(old_bytes.clone()).is_err() || String::from_utf8(new_bytes.clone()).is_err()
    {
        return json!({
            "ok": true,
            "existed": meta["existed"].as_bool().unwrap_or(false),
            "added": 0,
            "removed": 0,
            "truncated": false,
            "binary": true,
            "lines": [],
        })
        .to_string();
    }
    let old = String::from_utf8_lossy(&old_bytes);
    let new = String::from_utf8_lossy(&new_bytes);
    let (lines, added, removed, truncated) = diff_text(&old, &new);
    let rows: Vec<Value> = lines
        .into_iter()
        .map(|(sign, text)| json!({ "kind": kind_of(sign), "text": text }))
        .collect();
    json!({
        "ok": true,
        "existed": meta["existed"].as_bool().unwrap_or(false),
        "added": added,
        "removed": removed,
        "truncated": truncated,
        "binary": false,
        "lines": rows,
    })
    .to_string()
}

fn drop_snapshot(store: &Path, key: &str) -> usize {
    let mut removed = 0;
    for file in [meta_path(store, key), orig_path(store, key)] {
        if fs::remove_file(file).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 丢掉某个文件的**全部**回合快照（单文件撤销 / 接受之后）。
fn drop_all_for_path(store: &Path, path: &str) -> usize {
    let mut removed = 0;
    for snap in snaps_for(store, path) {
        removed += drop_snapshot(store, &snap.key);
    }
    removed
}

fn accept_in(store: &Path, path: &str) -> String {
    if path.is_empty() {
        let mut removed = 0;
        if let Ok(entries) = fs::read_dir(store) {
            for entry in entries.flatten() {
                if fs::remove_file(entry.path()).is_ok() {
                    removed += 1;
                }
            }
        }
        return json!({ "ok": true, "removed": removed }).to_string();
    }
    json!({ "ok": true, "removed": drop_all_for_path(store, path) }).to_string()
}

fn revert_in(store: &Path, path: &str) -> String {
    if path.is_empty() {
        let mut restored = 0;
        let mut deleted = 0;
        let mut failed = 0;
        let mut seen: Vec<String> = Vec::new();
        // 同一文件有多回合快照时只按**最早**那份还原一次（= 回到会话开头）。
        for snap in all_snaps(store) {
            let p = snap.path().to_string();
            if p.is_empty() || seen.iter().any(|s| s.eq_ignore_ascii_case(&p)) {
                continue;
            }
            seen.push(p.clone());
            let (r, d, fail) = revert_one(store, &p);
            restored += r;
            deleted += d;
            if fail {
                failed += 1;
            }
        }
        return json!({
            "ok": failed == 0,
            "restored": restored,
            "deleted": deleted,
            "failed": failed,
            "error": if failed == 0 { String::new() } else { format!("{failed} 个文件还原失败") },
        })
        .to_string();
    }
    let (restored, deleted, failed) = revert_one(store, path);
    json!({
        "ok": !failed,
        "restored": restored,
        "deleted": deleted,
        "failed": if failed { 1 } else { 0 },
        "error": if failed { "还原失败（见日志）".to_string() } else { String::new() },
    })
    .to_string()
}

/// 返回 (restored, deleted, failed)。用**最早**那份原件，并把该文件所有快照一起丢掉。
fn revert_one(store: &Path, path: &str) -> (usize, usize, bool) {
    let Some(snap) = earliest(store, path) else {
        return (0, 0, true);
    };
    match restore_snap(store, &snap) {
        Ok((restored, deleted)) => {
            let _ = drop_all_for_path(store, path);
            (restored, deleted, false)
        }
        Err(()) => (0, 0, true),
    }
}

/// 把一条快照写回磁盘：原本存在 ⇒ 写回原字节；原本不存在 ⇒ 删掉当前文件。
fn restore_snap(store: &Path, snap: &Snap) -> Result<(usize, usize), ()> {
    let path = snap.path();
    if path.is_empty() {
        return Err(());
    }
    if snap.existed() {
        let Ok(bytes) = fs::read(orig_path(store, &snap.key)) else {
            return Err(());
        };
        if fs::write(path, bytes).is_err() {
            return Err(());
        }
        Ok((1, 0))
    } else {
        if Path::new(path).is_file() && fs::remove_file(path).is_err() {
            return Err(());
        }
        Ok((0, 1))
    }
}

/// 只还原 `turn >= kept` 的快照（v1 `revertFileSnapshots`）。
///
/// 按回合**倒序**逐条写回：同一文件被多轮改过时，最后落在**回退点那一刻**的内容；
/// 被还原过的那几条快照随即删除，但更早回合的快照保留（那部分改动没被回退）。
fn revert_since_in(store: &Path, kept: u32) -> String {
    let mut doomed: Vec<Snap> = all_snaps(store)
        .into_iter()
        .filter(|snap| snap.turn >= kept)
        .collect();
    doomed.sort_by(|a, b| b.turn.cmp(&a.turn).then_with(|| a.key.cmp(&b.key)));
    let (mut restored, mut deleted, mut failed) = (0usize, 0usize, 0usize);
    let mut paths: Vec<String> = Vec::new();
    for snap in doomed {
        match restore_snap(store, &snap) {
            Ok((r, d)) => {
                restored += r;
                deleted += d;
                let _ = drop_snapshot(store, &snap.key);
                let p = snap.path().to_string();
                if !paths.iter().any(|s| s.eq_ignore_ascii_case(&p)) {
                    paths.push(p);
                }
            }
            Err(()) => failed += 1,
        }
    }
    json!({
        "ok": failed == 0,
        "restored": restored,
        "deleted": deleted,
        "failed": failed,
        "paths": paths,
        "error": if failed == 0 { String::new() } else { format!("{failed} 条快照还原失败") },
    })
    .to_string()
}

// ── 会话隔离（v1 `snapshots.rs` 的 per-session frames）────────────
//
// v1 把快照按会话落成 `ModuleData/history/frames/<session>.json`：切回历史会话时
// 只载入那一份 ⇒ **回退 A 会话不会误改 B 会话动过的文件**。v2 之前全用根目录，
// 两个会话的快照混在一起（同一个 `turn` 号会互相覆盖 / 误还原）。这里按会话分目录，
// 与 v1 的隔离语义对齐；文件格式仍是 v2 自己的 `.meta` + `.orig`（不迁移旧文件）。

/// 会话 id 校验：只允许 `[A-Za-z0-9_-]`、长度 1..=64（会拼进路径，必须挡住 `..`）。
fn safe_session(session: &str) -> Result<&str, String> {
    if session.is_empty() || session.len() > 64 {
        return Err(format!("非法会话 id（长度 {}）", session.len()));
    }
    if !session
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("非法会话 id（只允许字母 / 数字 / _ / -）".into());
    }
    Ok(session)
}

/// 一个会话的快照仓：`<root>/<session>/`；未给会话 ⇒ 老布局（root 本身）。
fn store_for_in(root: &Path, session: &str) -> Result<PathBuf, String> {
    let session = session.trim();
    if session.is_empty() {
        return Ok(root.to_path_buf());
    }
    Ok(root.join(safe_session(session)?))
}

/// 这次要扫的全部仓：给了会话 ⇒ 只有那一个；没给 ⇒ root + 每个会话子目录
/// （沙箱预览窗的「全部应用 / 撤销全部」用这条，与 v1 的全局动作同语义）。
fn stores_for_in(root: &Path, session: &str) -> Result<Vec<PathBuf>, String> {
    let session = session.trim();
    if !session.is_empty() {
        return Ok(vec![store_for_in(root, session)?]);
    }
    let mut stores = vec![root.to_path_buf()];
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stores.push(entry.path());
            }
        }
    }
    Ok(stores)
}

fn store_for(session: &str) -> Result<PathBuf, String> {
    store_for_in(&store_dir(), session)
}

fn stores_for(session: &str) -> Result<Vec<PathBuf>, String> {
    stores_for_in(&store_dir(), session)
}

/// 跨仓挑「最早的那份快照」所在的仓（`diff` / `pending` 未给会话时用）。
fn earliest_store(stores: &[PathBuf], path: &str) -> Option<PathBuf> {
    let mut best: Option<(u32, PathBuf)> = None;
    for store in stores {
        let Some(snap) = earliest(store, path) else {
            continue;
        };
        if best.as_ref().map(|(turn, _)| snap.turn < *turn).unwrap_or(true) {
            best = Some((snap.turn, store.clone()));
        }
    }
    best.map(|(_, store)| store)
}

/// 把多个仓的 `revert` 结果合起来（字段同形相加，`ok` 取与）。
fn merge_reverts(results: &[Value]) -> String {
    let mut restored = 0u64;
    let mut deleted = 0u64;
    let mut failed = 0u64;
    let mut ok = true;
    let mut paths: Vec<Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for value in results {
        restored += value["restored"].as_u64().unwrap_or(0);
        deleted += value["deleted"].as_u64().unwrap_or(0);
        failed += value["failed"].as_u64().unwrap_or(0);
        ok &= value["ok"].as_bool().unwrap_or(false);
        if let Some(list) = value["paths"].as_array() {
            paths.extend(list.iter().cloned());
        }
        let error = value["error"].as_str().unwrap_or("");
        if !error.is_empty() {
            errors.push(error.to_string());
        }
    }
    json!({
        "ok": ok,
        "restored": restored,
        "deleted": deleted,
        "failed": failed,
        "paths": paths,
        "error": errors.join("；"),
    })
    .to_string()
}

/// 把多个仓的 `accept` 结果合起来（`removed` 相加）。
fn merge_accepts(results: &[Value]) -> String {
    let mut removed = 0u64;
    let mut ok = true;
    for value in results {
        removed += value["removed"].as_u64().unwrap_or(0);
        ok &= value["ok"].as_bool().unwrap_or(false);
    }
    json!({ "ok": ok, "removed": removed }).to_string()
}

// ── FFI 入口（lib.rs 包装）──────────────────────────────────────

pub fn snapshot(session: &str, path: &str, turn: u32) -> String {
    let path = path.trim();
    if path.is_empty() {
        return json!({ "ok": false, "error": "路径为空" }).to_string();
    }
    let store = match store_for(session) {
        Ok(store) => store,
        Err(e) => return json!({ "ok": false, "error": e }).to_string(),
    };
    match snapshot_in(&store, path, turn) {
        Ok(()) => json!({ "ok": true }).to_string(),
        Err(e) => json!({ "ok": false, "error": e }).to_string(),
    }
}

pub fn diff(session: &str, path: &str) -> String {
    let path = path.trim();
    let stores = match stores_for(session) {
        Ok(stores) => stores,
        Err(e) => return json!({ "ok": false, "error": e }).to_string(),
    };
    match earliest_store(&stores, path) {
        Some(store) => diff_in(&store, path),
        None => diff_in(&store_dir(), path),
    }
}

pub fn accept(session: &str, path: &str) -> String {
    let path = path.trim();
    let stores = match stores_for(session) {
        Ok(stores) => stores,
        Err(e) => return json!({ "ok": false, "error": e }).to_string(),
    };
    if stores.len() == 1 {
        return accept_in(&stores[0], path);
    }
    let results: Vec<Value> = stores
        .iter()
        .map(|store| {
            serde_json::from_str(&accept_in(store, path)).unwrap_or(Value::Null)
        })
        .collect();
    merge_accepts(&results)
}

pub fn revert(session: &str, path: &str) -> String {
    let path = path.trim();
    let stores = match stores_for(session) {
        Ok(stores) => stores,
        Err(e) => return json!({ "ok": false, "error": e }).to_string(),
    };
    if stores.len() == 1 {
        return revert_in(&stores[0], path);
    }
    let results: Vec<Value> = stores
        .iter()
        .map(|store| {
            serde_json::from_str(&revert_in(store, path)).unwrap_or(Value::Null)
        })
        .collect();
    merge_reverts(&results)
}

/// 回退到第 `kept` 回合：只还原 `turn >= kept` 的快照（v1 回退按钮的文件还原）。
///
/// 给了会话 ⇒ 只看那个会话的快照（**不会误还原别的会话改过的文件**）；
/// 没给 ⇒ 扫全部仓（沙箱预览窗的「撤销全部」）。
pub fn revert_since(session: &str, kept: u32) -> String {
    let stores = match stores_for(session) {
        Ok(stores) => stores,
        Err(e) => return json!({ "ok": false, "error": e, "restored": 0, "deleted": 0,
                                "failed": 0, "paths": [] })
        .to_string(),
    };
    if stores.len() == 1 {
        return revert_since_in(&stores[0], kept);
    }
    let results: Vec<Value> = stores
        .iter()
        .map(|store| {
            serde_json::from_str(&revert_since_in(store, kept)).unwrap_or(Value::Null)
        })
        .collect();
    merge_reverts(&results)
}

pub fn pending(session: &str, path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return json!({ "pending": false }).to_string();
    }
    let stores = match stores_for(session) {
        Ok(stores) => stores,
        Err(e) => return json!({ "pending": false, "error": e }).to_string(),
    };
    json!({ "pending": earliest_store(&stores, path).is_some() }).to_string()
}

/// 清掉已不存在会话的快照目录（v1 `prune_frames(keep)`）。
///
/// 删会话是用户动作，不该把用户文件副本留在盘上；`keep` = 当前仍有会话的 id 列表。
pub fn prune(keep: &[String]) -> String {
    prune_in(&store_dir(), keep)
}

fn prune_in(root: &Path, keep: &[String]) -> String {
    let mut removed = 0usize;
    let Ok(entries) = fs::read_dir(root) else {
        return json!({ "ok": true, "removed": 0 }).to_string();
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if keep.iter().any(|id| id == &name) {
            continue;
        }
        if fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    json!({ "ok": true, "removed": removed }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_store() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("lunac-change-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn snapshot_diff_and_revert_roundtrip() {
        let store = temp_store();
        let file = store.join("a.txt");
        fs::write(&file, "one\ntwo\nthree\n").unwrap();
        let path = file.to_string_lossy().to_string();

        snapshot_in(&store, &path, 1).unwrap();
        // 同一回合再采一次不新增；下一回合会另存一份（改前内容）。
        fs::write(&file, "one\nTWO\nthree\nfour\n").unwrap();
        snapshot_in(&store, &path, 1).unwrap();
        snapshot_in(&store, &path, 2).unwrap();
        assert_eq!(snaps_for(&store, &path).len(), 2);

        let diff: Value = serde_json::from_str(&diff_in(&store, &path)).unwrap();
        assert_eq!(diff["ok"], true);
        assert_eq!(diff["added"], 2, "TWO + four");
        assert_eq!(diff["removed"], 1, "two");
        let rows = diff["lines"].as_array().unwrap();
        assert!(rows.iter().any(|r| r["kind"] == "del" && r["text"] == "two"));
        assert!(rows.iter().any(|r| r["kind"] == "add" && r["text"] == "four"));

        let reverted: Value = serde_json::from_str(&revert_in(&store, &path)).unwrap();
        assert_eq!(reverted["ok"], true);
        assert_eq!(reverted["restored"], 1);
        assert_eq!(fs::read_to_string(&file).unwrap(), "one\ntwo\nthree\n");
        assert!(snaps_for(&store, &path).is_empty());

        let _ = fs::remove_dir_all(&store);
    }

    #[test]
    fn revert_since_restores_only_the_discarded_turns() {
        let store = temp_store();
        let file = store.join("layer.txt");
        let path = file.to_string_lossy().to_string();
        // 逐回合改同一个文件：每回合采一份「改前内容」。
        for (turn, text) in [(1, "v1\n"), (2, "v2\n"), (3, "v3\n")] {
            fs::write(&file, text).unwrap();
            snapshot_in(&store, &path, turn).unwrap();
        }
        fs::write(&file, "v4\n").unwrap();

        // 回退到第 2 回合之前 ⇒ 依次还原 turn3 / turn2 的原件，落在 v2。
        let result: Value = serde_json::from_str(&revert_since_in(&store, 2)).unwrap();
        assert_eq!(result["ok"], true);
        assert!(result["paths"].as_array().unwrap().len() == 1);
        assert_eq!(fs::read_to_string(&file).unwrap(), "v2\n");
        // 被还原的那两回合快照已丢弃，更早的 turn1 仍在（那部分改动没被回退）。
        let left = snaps_for(&store, &path);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].turn, 1);

        // 再整文件撤销 ⇒ 回到会话开头 v1。
        let reverted: Value = serde_json::from_str(&revert_in(&store, &path)).unwrap();
        assert_eq!(reverted["restored"], 1);
        assert_eq!(fs::read_to_string(&file).unwrap(), "v1\n");
        assert!(snaps_for(&store, &path).is_empty());

        let _ = fs::remove_dir_all(&store);
    }

    #[test]
    fn created_file_is_deleted_on_revert() {
        let store = temp_store();
        let file = store.join("new.txt");
        let path = file.to_string_lossy().to_string();
        snapshot_in(&store, &path, 1).unwrap();
        fs::write(&file, "hello\n").unwrap();

        let diff: Value = serde_json::from_str(&diff_in(&store, &path)).unwrap();
        assert_eq!(diff["existed"], false);
        assert_eq!(diff["added"], 1);

        let reverted: Value = serde_json::from_str(&revert_in(&store, &path)).unwrap();
        assert_eq!(reverted["deleted"], 1);
        assert!(!file.exists());

        let _ = fs::remove_dir_all(&store);
    }

    #[test]
    fn accept_drops_snapshot_without_touching_the_file() {
        let store = temp_store();
        let file = store.join("b.txt");
        fs::write(&file, "old\n").unwrap();
        let path = file.to_string_lossy().to_string();
        snapshot_in(&store, &path, 1).unwrap();
        fs::write(&file, "mid\n").unwrap();
        snapshot_in(&store, &path, 2).unwrap();
        fs::write(&file, "new\n").unwrap();

        let accepted: Value = serde_json::from_str(&accept_in(&store, &path)).unwrap();
        assert_eq!(accepted["removed"], 4, "两份快照各 2 个文件");
        assert!(snaps_for(&store, &path).is_empty());
        assert_eq!(fs::read_to_string(&file).unwrap(), "new\n");

        let _ = fs::remove_dir_all(&store);
    }

    #[test]
    fn unknown_file_has_no_snapshot() {
        let store = temp_store();
        let diff: Value = serde_json::from_str(&diff_in(&store, "C:\\nope\\x.txt")).unwrap();
        assert_eq!(diff["ok"], false);
        let _ = fs::remove_dir_all(&store);
    }

    #[test]
    fn line_diff_handles_prefix_and_suffix() {
        let (lines, added, removed, truncated) = diff_text("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!((added, removed, truncated), (1, 1, false));
        let text: Vec<String> = lines.into_iter().map(|(_, t)| t).collect();
        assert_eq!(text, vec!["a", "b", "B", "c"]);
    }

    /// v1 `snapshots.rs` 的 per-session 隔离：每个会话一个仓，互相看不到。
    #[test]
    fn session_stores_are_separate_and_validated() {
        let root = temp_store();
        let a = store_for_in(&root, "sess-1").unwrap();
        let b = store_for_in(&root, "sess-2").unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with(&root) && b.starts_with(&root));
        // 未给会话 ⇒ 老布局（root 本身）。
        assert_eq!(store_for_in(&root, "  ").unwrap(), root);
        // 会拼进路径的 id 必须挡住 `..` / 分隔符。
        assert!(store_for_in(&root, "../evil").is_err());
        assert!(store_for_in(&root, "a\\b").is_err());
        assert!(store_for_in(&root, &"x".repeat(65)).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    /// 回退只看**本会话**的快照：另一个会话改过的同号回合文件不受影响。
    #[test]
    fn revert_since_only_touches_the_given_session() {
        let root = temp_store();
        let store_a = store_for_in(&root, "sess-a").unwrap();
        let store_b = store_for_in(&root, "sess-b").unwrap();
        let file_a = root.join("a.txt");
        let file_b = root.join("b.txt");
        fs::write(&file_a, "a-original\n").unwrap();
        fs::write(&file_b, "b-original\n").unwrap();
        let path_a = file_a.to_string_lossy().to_string();
        let path_b = file_b.to_string_lossy().to_string();
        snapshot_in(&store_a, &path_a, 2).unwrap();
        snapshot_in(&store_b, &path_b, 2).unwrap();
        fs::write(&file_a, "a-changed\n").unwrap();
        fs::write(&file_b, "b-changed\n").unwrap();

        // 在 A 会话回退到第 2 回合：只还原 a.txt。
        let result: Value = serde_json::from_str(&revert_since_in(&store_a, 2)).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(fs::read_to_string(&file_a).unwrap(), "a-original\n");
        assert_eq!(
            fs::read_to_string(&file_b).unwrap(),
            "b-changed\n",
            "B 会话的改动不该被 A 的回退还原"
        );
        // B 的快照仍在（下次切回 B 还能回退）。
        assert_eq!(snaps_for(&store_b, &path_b).len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    /// 删会话时把它的快照目录一起清掉（v1 `prune_frames`）。
    #[test]
    fn prune_drops_frames_of_deleted_sessions_only() {
        let root = temp_store();
        let keep = store_for_in(&root, "sess-keep").unwrap();
        let gone = store_for_in(&root, "sess-gone").unwrap();
        fs::create_dir_all(&keep).unwrap();
        fs::create_dir_all(&gone).unwrap();
        fs::write(gone.join("x.meta"), "{}").unwrap();

        let result: Value = serde_json::from_str(&prune_in(
            &root,
            &["sess-keep".to_string()],
        ))
        .unwrap();
        assert_eq!(result["removed"], 1);
        assert!(keep.exists(), "还在的会话不能被删");
        assert!(!gone.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
