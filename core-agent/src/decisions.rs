// core-agent/src/decisions.rs
// 本地决策账本（S1 首个切片，2026-10-15）—— 给「本地只读预筛」留痕。
//
// 为什么单独落一份、不进 `chat.db`：本地判定是**旁路**产物，不是「用户↔助手」的对话真相。
// 混进会话消息会污染回放 / 摘要 / 用量统计（docs/forbidden.md §禁用 61 明令禁止）。故本
// 模块按 `conclusions.rs` 的同一套落点推导，写到自己的一份追加式 JSONL 里：一份三用 ——
// 审计、评测集真值来源、将来的微调数据源（见 archive/agent-feature-backlog.md §4.1.7）。
//
// 落点：`<exe 根>\temp\decisions\<scope>.jsonl`（**按 scope 分家**，与 `temp\conclusions` 同源，
// 都从 `log::log_dir()` 的父目录派生 —— 不另写一套路径推导，见 docs/code-standards.md §规范 2）。
//
// 三条硬纪律：
//   · **只存哈希不存原文**：输入用 `log::hash64` 留指纹，账本里没有用户原文；
//   · **落盘前一律过 `log::mask_secrets`**：连哈希行也过一遍，绝不成为新的凭据面；
//   · **失败只 warn、绝不影响主流程**：建目录 / 打开 / 写入任一步失败都只是丢一条留痕，
//     绝不能把「记账」做成「拦路」（照 `conclusions::append` 的纪律）。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::log;
use crate::tools::MemoryScope;

/// 一条决策留痕（JSONL 的一行）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DecideRecord {
    /// epoch 秒。
    pub ts: i64,
    /// 决策类型：`prescreen-review`（每 K 轮：值不值得派后台复盘）/ `compact-prescreen`（压缩事件）。
    pub kind: String,
    /// 判定输入的 FNV-1a 64 位哈希（16 位十六进制）—— **只存哈希，不存原文**。
    pub input_hash: String,
    /// 判定结果：`run` / `skip`。
    pub verdict: String,
    /// 本次判定耗时（毫秒）。
    pub latency_ms: u64,
    /// 出判定的本地模型名（启发式桩为 `heuristic`）。
    pub model: String,
    /// 机器可读的原因码。
    pub reason: String,
    /// 是否走了 fail-open 后备路径（后端故障）。
    pub fallback: bool,
    /// 作用域（`global` / `pet`）—— 冗余一份便于人工排查（文件本身已按 scope 分家）。
    pub scope: String,
    /// 写入时的第几次用户提问（0 = 手动压缩等无提问号场景）。
    pub turn: u64,
    /// 会话标识（复用 agent 既有的 `session_id()`）。
    pub session_id: String,
}

// ── 落点与 I/O ───────────────────────────────────────────────────

/// 账本目录：`<exe 根>\temp\decisions`。
///
/// **复用 `conclusions::default_dir()` 的同一套推导**（它 = `log::log_dir()` 的父目录下
/// `conclusions`）；这里只把末段换成 `decisions`，不再自己拼一遍 `log_dir()`。
pub fn default_dir() -> PathBuf {
    crate::conclusions::default_dir()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("decisions")
}

/// 某一作用域的账本文件路径（按 scope 分家）。
pub fn path_for(dir: &Path, scope: MemoryScope) -> PathBuf {
    dir.join(format!("{}.jsonl", scope.as_str()))
}

/// 追加写入一条留痕（append-only JSONL）。失败只 warn、绝不影响主流程。
///
/// 日志只记**类型与判定**，**不记输入原文**（原文本来就没进账本，只有哈希）。
pub fn append(dir: &Path, scope: MemoryScope, rec: &DecideRecord) {
    let line = match serde_json::to_string(rec) {
        Ok(l) => l,
        Err(e) => {
            log::warn(format!("决策账本序列化失败（已跳过）：{e}"));
            return;
        }
    };
    // 落盘前统一脱敏：哈希行理论上不含凭据，但 `model` / `reason` 等字段来自环境或模型，
    // 一律过一遍 `mask_secrets`，让账本**永不**成为新的凭据面（docs/forbidden.md §禁用 6）。
    let masked = log::mask_secrets(&line);
    if let Err(e) = std::fs::create_dir_all(dir) {
        log::warn(format!("决策账本建目录失败（{}）：{e}", dir.display()));
        return;
    }
    let path = path_for(dir, scope);
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut f) => {
            if let Err(e) = f.write_all(masked.as_bytes()).and_then(|_| f.write_all(b"\n")).and_then(|_| f.flush()) {
                log::warn(format!("决策账本写入失败（{}）：{e}", path.display()));
            }
        }
        Err(e) => log::warn(format!("决策账本打开失败（{}）：{e}", path.display())),
    }
}

/// 读取某一作用域的全部留痕（坏行跳过 —— 账本是锦上添花，读不出就当没有，绝不阻断主流程）。
///
/// 仅供**测试 / 验收**读取（生产只追加、不回读）：故 `#[cfg(test)]`（真实反映它只在测试里被
/// 用到；这不是 `allow(dead_code)` 豁免，而是「本函数不编进发布二进制」）。
#[cfg(test)]
pub fn load(dir: &Path, scope: MemoryScope) -> Vec<DecideRecord> {
    let path = path_for(dir, scope);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out: Vec<DecideRecord> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(rec) = serde_json::from_str::<DecideRecord>(line) {
            out.push(rec);
        }
    }
    out
}

/// 记录条数（供验收精确计数用；坏行不计）。同 [`load`]：仅测试 / 验收用。
#[cfg(test)]
pub fn count(dir: &Path, scope: MemoryScope) -> usize {
    load(dir, scope).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(ts: i64, verdict: &str) -> DecideRecord {
        DecideRecord {
            ts,
            kind: "prescreen-review".into(),
            input_hash: format!("{:016x}", log::hash64("样例输入")),
            verdict: verdict.into(),
            latency_ms: 12,
            model: "local".into(),
            reason: "model-run".into(),
            fallback: false,
            scope: MemoryScope::Global.as_str().into(),
            turn: 5,
            session_id: "sess-1".into(),
        }
    }

    /// 追加 + 读取 + 精确计数：三条写进去，`count` 必须是 3（验收靠它精确断言）。
    #[test]
    fn append_is_reloadable_and_count_is_exact() {
        let dir = std::env::temp_dir().join(format!("lunac-dec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for ts in [1, 2, 3] {
            append(&dir, MemoryScope::Global, &rec(ts, "run"));
        }
        let loaded = load(&dir, MemoryScope::Global);
        assert_eq!(loaded.len(), 3);
        assert_eq!(count(&dir, MemoryScope::Global), 3);
        assert_eq!(loaded[0], rec(1, "run"));
        // 按 scope 分家：global 那份不掺 pet。
        assert!(load(&dir, MemoryScope::Pet).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 坏行跳过：手工塞一行非法 JSON，读取不 panic、只跳过它。
    #[test]
    fn load_skips_corrupt_lines() {
        let dir = std::env::temp_dir().join(format!("lunac-dec-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = path_for(&dir, MemoryScope::Global);
        let good = serde_json::to_string(&rec(7, "skip")).unwrap();
        std::fs::write(&path, format!("{good}\n{{ 这不是 JSON\n\n")).unwrap();
        let loaded = load(&dir, MemoryScope::Global);
        assert_eq!(loaded.len(), 1, "坏行必须被跳过，好行仍可读");
        assert_eq!(loaded[0].ts, 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 落盘前过脱敏：喂含 `sk-...` 的输入，账本里**只有哈希**、绝无明文凭据。
    #[test]
    fn secrets_never_reach_the_ledger() {
        let dir = std::env::temp_dir().join(format!("lunac-dec-mask-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let secret = "帮我看下 sk-live-99887766 这个 key";
        let mut r = rec(1, "run");
        r.input_hash = format!("{:016x}", log::hash64(secret));
        // 即便把原文误塞进 reason（模拟脏数据），append 也必须把它抹掉。
        r.reason = format!("raw={secret}");
        append(&dir, MemoryScope::Global, &r);
        let blob = std::fs::read_to_string(path_for(&dir, MemoryScope::Global)).unwrap();
        assert!(!blob.contains("99887766"), "凭据不得落进账本：{blob}");
        assert!(blob.contains(&format!("{:016x}", log::hash64(secret))), "哈希应在");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 写盘失败（把目录指到一个已存在的**文件**上）只 warn、不 panic、不阻断。
    #[test]
    fn write_failure_is_non_fatal() {
        let file = std::env::temp_dir().join(format!("lunac-dec-file-{}", std::process::id()));
        std::fs::write(&file, b"not a dir").unwrap();
        // 目录 = 该文件 ⇒ create_dir_all 必失败；此处只要不 panic 即可。
        append(&file, MemoryScope::Global, &rec(1, "run"));
        assert_eq!(count(&file, MemoryScope::Global), 0);
        let _ = std::fs::remove_file(&file);
    }
}
