// core-agent/src/checkpoint.rs
// S2 长任务托管：检查点（2026-10-22，最小闭环）。
//
// 为什么现在才做：`arhive/agent-feature-backlog.md` §4.1.0 早就判定「agent 侧没有会话持久化」
// （history 是进程内 `Vec`、会话 id 由进程生成，都不落盘）⇒ `--resume`、跨回合续跑、
// 任务级预算的持久化三件事全都卡在同一个前置上。本模块只做这一层**最小的持久化**：
//
//   · 每个用户回合结束后、以及每批工具结果落进 history 之后，把 `history` + 提问序号 +
//     任务级预算累计 + 审批上下文写成一份 JSON 检查点；
//   · 启动时 `--resume <session-id|latest>` 读回来，接着跑；
//   · 默认**不写**（只有显式 `--checkpoint` / `--resume` 才落盘）—— 常驻 stdio 由宿主
//     每次提问都重启进程，默认落盘既没意义也白占磁盘。
//
// 落点：`<exe 根>\ModuleData\tasks\sessions\`（S2 计划里的 `ModuleData\tasks\`；
// 与 `temp\conclusions` 的「agent 自己的数据根」同一套推导），可用
// `LUNAC_CHECKPOINT_DIR` 覆盖（测试 / 便携场景）。
//
// 三条硬口径：
//   · **原子写**：先写 `.tmp` 再 rename；Windows 上 rename 不覆盖，先删旧再改名；
//   · **有上限**：单份 > 32 MiB 放弃写入（图片附件 base64 可能把 history 撑得很大），
//     每个会话只留最近 `KEEP` 份、全局最多 `KEEP_TOTAL` 份（按文件名里的毫秒时间戳排序，
//     不依赖 mtime）；
//   · **不静默失败**：读写失败一律返回可读 `Err`，由调用方决定 warn 还是退出。
//
// 边界如实记：检查点**不脱敏**（`mask_secrets` 会改写工具结果里的代码 / 配置原文，
// 续跑后模型看到的内容就与真实文件不一致）。它落在本机模块数据目录，与 `chat.db`
// 同级信任域；只在用户显式开启时才产生。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::log;

/// 检查点格式版本。字段语义变化时递增；读到不认识的版本直接拒绝（不猜、不迁移）。
pub const VERSION: u32 = 1;
/// 每个会话最多保留的检查点份数（2026-10-22 起从「全局 20」改为**按会话**）。
///
/// 为什么必须按会话：中间态每批工具结果后写一份，cron job 之间又共用同一个目录 ——
/// 全局 20 份时，一条工具轮数多的 job 会把别的 job 的检查点挤掉，续跑会话随机丢失。
/// 按会话各留 20 份后，每个 job / 交互会话都能稳定续上。
pub const KEEP: usize = 20;
/// 全局总份数上限（安全阀）：会话再多也不让目录无限长。
pub const KEEP_TOTAL: usize = 200;
/// 单份检查点的大小上限。超过就放弃写入（宁可不续跑，也不把磁盘写爆）。
pub const MAX_BYTES: usize = 32 * 1024 * 1024;
/// 检查点目录的覆盖入口（测试 / 便携）。
pub const DIR_ENV: &str = "LUNAC_CHECKPOINT_DIR";

/// 审批 / 工作区上下文（2026-10-22）：续跑时用来**对比**当前进程与检查点是否同一套
/// 权限口径。
///
/// **2026-10-31 起改为「最严者胜」的单向合并**（见 [`ApprovalContext::tightest`]）：
/// 原先「只记录、不自动恢复」的理由仍然成立 —— 静默沿用 `--dangerously-skip-permissions`
/// 或 `acceptEdits` 等于替用户放宽权限；但反方向同样危险：`--continue` 忘了重传
/// `--permission-mode plan` 时，续跑会悄悄回到可写档。现在的规则 = **只往更严的方向并，
/// 绝不放宽**，并且每次都把「收紧了什么」写进日志 / stderr。
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalContext {
    /// `--permission-mode` 原值；空串 = 未传（默认可写档）。
    #[serde(default)]
    pub permission_mode: String,
    /// `--dangerously-skip-permissions`。
    #[serde(default)]
    pub skip_permissions: bool,
    /// `--permission-prompt-tool stdio`（写类工具先发审批卡）。
    #[serde(default)]
    pub ask_permission: bool,
    /// `--disallowedTools` 列表。
    #[serde(default)]
    pub disallowed: Vec<String>,
    /// `--add-dir` 列表（原样字符串，不保证能 canonicalize）。
    #[serde(default)]
    pub add_dirs: Vec<String>,
    /// 进程工作目录（工具相对路径的基准）。
    #[serde(default)]
    pub cwd: String,
}

/// 一份检查点。字段只增不改；旧文件缺 `task_budget_used` 时按 0 读。
#[derive(Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub version: u32,
    /// 被恢复进程沿用的会话 id（`--resume` 后 stdout 的 `session_id` 与它一致）。
    pub session_id: String,
    /// 已完成的用户提问数（恢复后从 `+1` 继续编号）。
    pub question_seq: u64,
    /// 写入时刻（epoch 毫秒）；同时编码进文件名，用于 `latest` 与清理排序。
    pub ts_ms: i64,
    /// 任务级预算的累计用量（S2 切片；恢复后继续累加，不因重启清零）。
    #[serde(default)]
    pub task_budget_used: u64,
    /// 写入这份检查点时的审批 / 工作区口径（缺省 = 旧检查点，按全默认比对）。
    #[serde(default)]
    pub approval: ApprovalContext,
    /// 完整的 Anthropic 形态消息（含 `tool_use` / `tool_result` / 图片块）。
    #[serde(default)]
    pub history: Vec<Value>,
}

impl ApprovalContext {
    /// 与另一份上下文比较，返回**可读差异**（空 = 完全一致）。
    ///
    /// 只做展示用：`--resume` 拿到差异后告警，但**不改变**本次进程的权限口径。
    pub fn diff(&self, other: &Self) -> Vec<String> {
        fn mode(m: &str) -> &str {
            if m.trim().is_empty() {
                "(default)"
            } else {
                m.trim()
            }
        }
        fn set(items: &[String]) -> Vec<String> {
            let mut v: Vec<String> = items.iter().map(|s| s.trim().to_string()).collect();
            v.sort();
            v.dedup();
            v
        }
        let mut out = Vec::new();
        if mode(&self.permission_mode) != mode(&other.permission_mode) {
            out.push(format!(
                "permission-mode {} → {}",
                mode(&self.permission_mode),
                mode(&other.permission_mode)
            ));
        }
        if self.skip_permissions != other.skip_permissions {
            out.push(format!(
                "skip-permissions {} → {}",
                self.skip_permissions, other.skip_permissions
            ));
        }
        if self.ask_permission != other.ask_permission {
            out.push(format!(
                "permission-prompt {} → {}",
                self.ask_permission, other.ask_permission
            ));
        }
        if set(&self.disallowed) != set(&other.disallowed) {
            out.push(format!(
                "disallowedTools [{}] → [{}]",
                set(&self.disallowed).join(","),
                set(&other.disallowed).join(",")
            ));
        }
        if set(&self.add_dirs) != set(&other.add_dirs) {
            out.push(format!(
                "add-dir [{}] → [{}]",
                set(&self.add_dirs).join(","),
                set(&other.add_dirs).join(",")
            ));
        }
        if !self.cwd.is_empty() && !other.cwd.is_empty() && self.cwd != other.cwd {
            out.push(format!("cwd {} → {}", self.cwd, other.cwd));
        }
        out
    }

    /// 续跑时的「最严者胜」合并（2026-10-31）：返回**本次进程真正生效**的口径 + 收紧说明。
    ///
    /// 调用点是 `--resume` / `--continue` 恢复之后、建 `Ctx` 之前：把返回的上下文写回
    /// `CliArgs`，后续一切判定（`read_only` / 审批策略 / 工具池 / 目录闸）自然跟着变。
    ///
    /// 每条规则都只往**更严**的方向并，绝不放宽：
    ///   · `permission_mode`：任一侧是 `plan` ⇒ `plan`（否则保留本次命令行给的值）；
    ///   · `skip-permissions`：**两侧都为真**才生效（检查点那次没开，这次也不替你开）；
    ///   · 审批卡（`ask_permission`）：任一侧要求就要求；
    ///   · `disallowedTools`：取**并集**（禁得更多 = 更严）；
    ///   · `--add-dir`：取**交集**（少一个目录 = 更严）。⚠️ 只有检查点自己记过目录时才交集
    ///     —— 旧检查点没有该字段（空），照交集处理会让老会话续跑突然「工具全都 Access denied」。
    ///
    /// `cwd` 不参与合并：进程 cwd 由启动方决定，改它是另一件事（也不在 `--resume` 的语义里）。
    ///
    /// 返回的 `Vec<String>` 只列**相对 `current` 真的收紧了**的项，供调用方原样写日志；
    /// 空 = 本次口径已经不比检查点松，保持原样。
    pub fn tightest(&self, current: &Self) -> (Self, Vec<String>) {
        fn norm(items: &[String]) -> Vec<String> {
            let mut v: Vec<String> = items
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            v.sort();
            v.dedup();
            v
        }
        fn is_plan(mode: &str) -> bool {
            mode.trim().eq_ignore_ascii_case("plan")
        }

        let mut out = current.clone();
        let mut notes: Vec<String> = Vec::new();

        if is_plan(&self.permission_mode) && !is_plan(&out.permission_mode) {
            out.permission_mode = "plan".to_string();
            notes.push("permission-mode 沿用检查点的 plan（只读）".to_string());
        }

        if !self.skip_permissions && out.skip_permissions {
            out.skip_permissions = false;
            notes.push("skip-permissions 不沿用（检查点那次也没开）".to_string());
        }

        if self.ask_permission && !out.ask_permission {
            out.ask_permission = true;
            notes.push("补上审批卡（检查点要求走审批）".to_string());
        }

        let mut banned = norm(&out.disallowed);
        let from_cp = norm(&self.disallowed);
        let extra: Vec<String> = from_cp
            .iter()
            .filter(|d| !banned.contains(d))
            .cloned()
            .collect();
        if !extra.is_empty() {
            banned.extend(extra.iter().cloned());
            banned.sort();
            banned.dedup();
            notes.push(format!("disallowedTools 并入 [{}]", extra.join(",")));
            // 只在真有新增时才改写：没变就保留命令行原样的顺序（不制造看不见的重排）。
            out.disallowed = banned;
        }

        let recorded = norm(&self.add_dirs);
        if !recorded.is_empty() {
            let keep: Vec<String> = out
                .add_dirs
                .iter()
                .filter(|d| recorded.contains(&d.trim().to_string()))
                .cloned()
                .collect();
            if norm(&keep) != norm(&out.add_dirs) {
                let dropped: Vec<String> = out
                    .add_dirs
                    .iter()
                    .filter(|d| !recorded.contains(&d.trim().to_string()))
                    .cloned()
                    .collect();
                notes.push(format!("add-dir 收窄（去掉 {}）", dropped.join(",")));
                out.add_dirs = keep;
            }
        }

        (out, notes)
    }
}

/// 检查点目录：`LUNAC_CHECKPOINT_DIR` 优先，否则 `<exe 根>\ModuleData\tasks\sessions`。
pub fn dir() -> PathBuf {
    if let Ok(d) = std::env::var(DIR_ENV) {
        let d = d.trim();
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ModuleData")
        .join("tasks")
        .join("sessions")
}

/// 写一份检查点（用当前时刻）。返回落盘路径。
pub fn save(
    session_id: &str,
    question_seq: u64,
    task_budget_used: u64,
    approval: &ApprovalContext,
    history: &[Value],
) -> Result<PathBuf, String> {
    save_capped(
        &dir(),
        now_ms(),
        MAX_BYTES,
        session_id,
        question_seq,
        task_budget_used,
        approval,
        history,
    )
}

/// 读一份检查点：`spec` = `latest` 或会话 id（也接受完整文件名）。
pub fn load(spec: &str) -> Result<(Checkpoint, PathBuf), String> {
    load_at(&dir(), spec)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 会话 id 进文件名前只保留安全字符（id 由本进程生成，正常不会触发替换）。
fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn file_name(ts_ms: i64, session_id: &str) -> String {
    format!("{:013}-{}.json", ts_ms.max(0), sanitize(session_id))
}

/// 测试用：显式给时间戳（避免依赖真实时钟与 mtime 分辨率）。
#[cfg(test)]
fn save_at(
    dir: &Path,
    ts_ms: i64,
    session_id: &str,
    question_seq: u64,
    task_budget_used: u64,
    history: &[Value],
) -> Result<PathBuf, String> {
    save_capped(
        dir,
        ts_ms,
        MAX_BYTES,
        session_id,
        question_seq,
        task_budget_used,
        &ApprovalContext::default(),
        history,
    )
}

/// 带大小上限的写法（上限做成参数，测试用 1 KiB 覆盖「超限拒绝」而不真的分配 32 MiB）。
#[allow(clippy::too_many_arguments)]
fn save_capped(
    dir: &Path,
    ts_ms: i64,
    max_bytes: usize,
    session_id: &str,
    question_seq: u64,
    task_budget_used: u64,
    approval: &ApprovalContext,
    history: &[Value],
) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("建检查点目录失败 {}: {e}", dir.display()))?;
    let cp = Checkpoint {
        version: VERSION,
        session_id: session_id.to_string(),
        question_seq,
        ts_ms,
        task_budget_used,
        approval: approval.clone(),
        history: history.to_vec(),
    };
    let body = serde_json::to_string(&cp).map_err(|e| format!("序列化检查点失败: {e}"))?;
    if body.len() > max_bytes {
        return Err(format!(
            "检查点过大（{} 字节 > {}），放弃写入；history 含图片附件时可先移除附件再续跑",
            body.len(),
            max_bytes
        ));
    }
    let path = dir.join(file_name(ts_ms, session_id));
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, body.as_bytes()).map_err(|e| format!("写临时检查点失败 {}: {e}", tmp.display()))?;
    // Windows 的 rename 不覆盖已存在目标：同一毫秒重复写时要先删旧文件（仍在同一目录内，
    // 删除失败就如实失败，不把半个文件当成功）。
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("替换旧检查点失败 {}: {e}", path.display()))?;
    }
    fs::rename(&tmp, &path).map_err(|e| format!("检查点改名失败 {}: {e}", path.display()))?;
    prune(dir, KEEP, KEEP_TOTAL);
    log::info(format!(
        "检查点写入：{}（第 {} 问 / history {} 条 / 任务预算累计 {}）",
        path.display(),
        question_seq,
        history.len(),
        task_budget_used
    ));
    Ok(path)
}

fn load_at(dir: &Path, spec: &str) -> Result<(Checkpoint, PathBuf), String> {
    let path = resolve(dir, spec)?;
    let body = fs::read_to_string(&path)
        .map_err(|e| format!("读检查点失败 {}: {e}", path.display()))?;
    let cp: Checkpoint =
        serde_json::from_str(&body).map_err(|e| format!("解析检查点失败 {}: {e}", path.display()))?;
    if cp.version != VERSION {
        return Err(format!(
            "检查点版本不兼容：{}（文件 v{}，当前需要 v{}）",
            path.display(),
            cp.version,
            VERSION
        ));
    }
    log::info(format!(
        "从检查点恢复：{}（会话 {} / 第 {} 问 / history {} 条）",
        path.display(),
        cp.session_id,
        cp.question_seq,
        cp.history.len()
    ));
    Ok((cp, path))
}

/// 列出目录里全部检查点，按文件名（= 毫秒时间戳前缀）升序。
fn list(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.len() > 18 && n.ends_with(".json"))
                .unwrap_or(false)
        })
        .collect();
    out.sort_by(|a, b| {
        a.file_name()
            .and_then(|n| n.to_str())
            .cmp(&b.file_name().and_then(|n| n.to_str()))
    });
    out
}

fn resolve(dir: &Path, spec: &str) -> Result<PathBuf, String> {
    let entries = list(dir);
    if entries.is_empty() {
        return Err(format!("目录里没有检查点：{}", dir.display()));
    }
    if spec == "latest" {
        return entries
            .last()
            .cloned()
            .ok_or_else(|| format!("目录里没有检查点：{}", dir.display()));
    }
    let spec = spec.trim();
    let want_file = format!("{spec}.json");
    // 完整文件名 / 13 位时间戳前缀的文件名都按精确匹配处理；否则按 `-<session>.json` 后缀匹配。
    if let Some(p) = entries.iter().find(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == spec || n == want_file)
            .unwrap_or(false)
    }) {
        return Ok(p.clone());
    }
    let suffix = format!("-{}.json", sanitize(spec));
    entries
        .iter()
        .rev()
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(&suffix))
                .unwrap_or(false)
        })
        .cloned()
        .ok_or_else(|| format!("找不到会话 {spec} 的检查点（目录 {}）", dir.display()))
}

/// 每个会话只留最新 `keep` 份，并且全局不超过 `keep_total` 份；删不掉只 warn
/// （旧文件留着不影响正确性）。
///
/// 文件名里的会话 id 取第一个 `-` 之后的部分（`<13 位毫秒>-<session>.json`）。
fn prune(dir: &Path, keep: usize, keep_total: usize) {
    let entries = list(dir);
    let mut per_session: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut kept = 0usize;
    let mut drop: Vec<PathBuf> = Vec::new();
    // 从新到旧扫：先满足各会话的份数，再受全局上限约束。
    for p in entries.iter().rev() {
        let session = p
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.split_once('-').map(|(_, s)| s.to_string()))
            .unwrap_or_default();
        let seen = per_session.entry(session).or_insert(0);
        if *seen >= keep || kept >= keep_total {
            drop.push(p.clone());
        } else {
            *seen += 1;
            kept += 1;
        }
    }
    for p in drop {
        if let Err(e) = fs::remove_file(&p) {
            log::warn(format!("清理旧检查点失败 {}: {e}", p.display()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 每个用例一个独立目录，避免并行测试互相看到对方的文件。
    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "lunac-checkpoint-{}-{}-{tag}",
            std::process::id(),
            now_ms()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).expect("create temp checkpoint dir");
        d
    }

    #[test]
    fn roundtrip_keeps_history_and_budget() {
        let dir = temp_dir("roundtrip");
        let history = vec![
            json!({"role":"user","content":[{"type":"text","text":"hi"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"hello"}]}),
        ];
        let path = save_at(&dir, 1_000, "sess_1_1000", 3, 777, &history).expect("save");
        assert!(path.exists());
        let (cp, got) = load_at(&dir, "sess_1_1000").expect("load");
        assert_eq!(got, path);
        assert_eq!(cp.version, VERSION);
        assert_eq!(cp.session_id, "sess_1_1000");
        assert_eq!(cp.question_seq, 3);
        assert_eq!(cp.task_budget_used, 777);
        assert_eq!(cp.history, history, "history 必须逐字段还原");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn latest_picks_the_newest_and_ids_resolve_exactly() {
        let dir = temp_dir("latest");
        save_at(&dir, 1_000, "sess_a", 1, 0, &[json!({"role":"user","content":[]})]).unwrap();
        save_at(&dir, 2_000, "sess_b", 2, 10, &[json!({"role":"user","content":[]})]).unwrap();
        let (cp, _) = load_at(&dir, "latest").expect("latest");
        assert_eq!(cp.session_id, "sess_b");
        let (cp, _) = load_at(&dir, "sess_a").expect("by id");
        assert_eq!(cp.question_seq, 1);
        assert!(load_at(&dir, "sess_missing").is_err(), "不存在的会话要报错而不是猜一个");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_keeps_the_newest_entries_per_session() {
        let dir = temp_dir("prune");
        for i in 1..=5 {
            save_at(&dir, i * 1_000, "sess_same", i as u64, 0, &[]).unwrap();
        }
        prune(&dir, 2, 100);
        let entries = list(&dir);
        assert_eq!(entries.len(), 2, "同一会话只应留下 2 份");
        let (cp, _) = load_at(&dir, "latest").expect("latest after prune");
        assert_eq!(cp.session_id, "sess_same");
        assert_eq!(cp.question_seq, 5);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_keeps_sessions_independent_and_caps_the_total() {
        let dir = temp_dir("prune-multi");
        for s in 1..=3 {
            // 每个会话 3 份，时间戳交错，确保排序靠时间而不是写入顺序。
            for i in 1..=3u64 {
                save_at(&dir, (i as i64) * 1_000 + s as i64, &format!("sess_{s}"), i, 0, &[]).unwrap();
            }
        }
        prune(&dir, 2, 100);
        assert_eq!(list(&dir).len(), 6, "3 个会话 × 每个 2 份");
        for s in 1..=3 {
            let (cp, _) = load_at(&dir, &format!("sess_{s}")).expect("每个会话都能续上");
            assert_eq!(cp.question_seq, 3, "留下的是最新一份");
        }

        prune(&dir, 2, 3);
        assert_eq!(list(&dir).len(), 3, "全局上限优先收口");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_and_wrong_version_are_reported() {
        let dir = temp_dir("corrupt");
        fs::write(dir.join("0000000001000-sess_bad.json"), b"not json").unwrap();
        assert!(load_at(&dir, "latest").is_err(), "坏 JSON 必须报错");
        fs::write(
            dir.join("0000000002000-sess_old.json"),
            br#"{"version":99,"session_id":"sess_old","question_seq":1,"ts_ms":2000,"history":[]}"#,
        )
        .unwrap();
        let err = load_at(&dir, "latest").expect_err("版本不符必须拒绝");
        assert!(err.contains("版本不兼容"), "错误要说明是版本问题: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversize_checkpoint_is_refused_not_truncated() {
        let dir = temp_dir("oversize");
        let history = vec![json!({"role":"user","content":[{"type":"text","text":"0123456789"}]})];
        let err = save_capped(
            &dir,
            1_000,
            64,
            "sess_big",
            1,
            0,
            &ApprovalContext::default(),
            &history,
        )
        .expect_err("超限必须拒绝");
        assert!(err.contains("检查点过大"), "{err}");
        assert!(list(&dir).is_empty(), "拒绝时不得留下半份文件");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn approval_context_roundtrips_and_reports_differences() {
        let dir = temp_dir("approval");
        let saved = ApprovalContext {
            permission_mode: "plan".into(),
            skip_permissions: false,
            ask_permission: true,
            disallowed: vec!["Write".into(), "Edit".into()],
            add_dirs: vec!["D:\\work".into()],
            cwd: "D:\\proj".into(),
        };
        save_capped(&dir, 1_000, MAX_BYTES, "sess_ap", 1, 0, &saved, &[]).unwrap();
        let (cp, _) = load_at(&dir, "sess_ap").expect("load");
        assert_eq!(cp.approval, saved, "审批上下文必须逐字段还原");

        // 顺序不同、重复项不算差异（比较的是集合语义）。
        let same = ApprovalContext {
            disallowed: vec!["Edit".into(), "Write".into(), "Edit".into()],
            ..saved.clone()
        };
        assert!(saved.diff(&same).is_empty(), "工具名顺序 / 重复不算差异");

        let changed = ApprovalContext {
            permission_mode: String::new(),
            skip_permissions: true,
            ..saved.clone()
        };
        let diff = saved.diff(&changed);
        assert!(
            diff.iter().any(|d| d.contains("permission-mode plan → (default)")),
            "{diff:?}"
        );
        assert!(
            diff.iter().any(|d| d.contains("skip-permissions false → true")),
            "{diff:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_checkpoint_without_approval_field_reads_as_default() {
        let dir = temp_dir("approval-legacy");
        fs::write(
            dir.join("0000000003000-sess_legacy.json"),
            br#"{"version":1,"session_id":"sess_legacy","question_seq":1,"ts_ms":3000,"history":[]}"#,
        )
        .unwrap();
        let (cp, _) = load_at(&dir, "sess_legacy").expect("旧检查点应能读");
        assert_eq!(cp.approval, ApprovalContext::default(), "缺字段按默认读");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 「最严者胜」：检查点里的 plan / 审批卡 / 禁用名单都要能把本次口径**收紧**。
    #[test]
    fn tightest_adopts_stricter_checkpoint_fields() {
        let cp = ApprovalContext {
            permission_mode: "plan".into(),
            skip_permissions: false,
            ask_permission: true,
            disallowed: vec!["Write".into(), "Edit".into()],
            add_dirs: vec!["D:\\work".into()],
            cwd: "D:\\proj".into(),
        };
        let current = ApprovalContext {
            permission_mode: String::new(), // 忘了重传 plan
            skip_permissions: true,         // 这次想要 skip
            ask_permission: false,
            disallowed: vec!["Bash".into()],
            add_dirs: vec!["D:\\work".into(), "D:\\extra".into()],
            cwd: "D:\\proj".into(),
        };
        let (effective, notes) = cp.tightest(&current);
        assert_eq!(effective.permission_mode, "plan", "检查点的只读档必须沿用");
        assert!(!effective.skip_permissions, "检查点没开 skip ⇒ 这次也不开");
        assert!(effective.ask_permission, "检查点要求走审批 ⇒ 补上");
        assert_eq!(
            effective.disallowed,
            vec!["Bash".to_string(), "Edit".to_string(), "Write".to_string()],
            "禁用名单取并集（排序 + 去重）"
        );
        assert_eq!(
            effective.add_dirs,
            vec!["D:\\work".to_string()],
            "add-dir 取交集（多出来的目录要摘掉）"
        );
        assert_eq!(effective.cwd, "D:\\proj", "cwd 不参与合并");
        assert_eq!(notes.len(), 5, "五类收紧都要有可读说明：{notes:?}");
    }

    /// 反向：本次比检查点更严（或完全相同）时**一项都不许放宽**，也不该报「收紧」。
    #[test]
    fn tightest_never_loosens_a_stricter_current_run() {
        let cp = ApprovalContext {
            permission_mode: String::new(),
            skip_permissions: false,
            ask_permission: false,
            disallowed: vec!["Write".into()],
            add_dirs: vec![],
            cwd: String::new(),
        };
        let current = ApprovalContext {
            permission_mode: "plan".into(),
            skip_permissions: false,
            ask_permission: true,
            disallowed: vec!["Write".into(), "Edit".into()],
            add_dirs: vec!["D:\\work".into()],
            cwd: String::new(),
        };
        let (effective, notes) = cp.tightest(&current);
        assert_eq!(effective, current, "本次更严 ⇒ 原样保留");
        assert!(notes.is_empty(), "没收紧就不该有说明：{notes:?}");
    }

    /// 旧检查点（没记 `--add-dir`）**不能**把本次目录交集成空 —— 否则老会话续跑会
    /// 突然「工具全都 Access denied」。
    #[test]
    fn tightest_keeps_current_dirs_when_the_checkpoint_recorded_none() {
        let cp = ApprovalContext::default();
        let current = ApprovalContext {
            add_dirs: vec!["D:\\work".into()],
            ..ApprovalContext::default()
        };
        let (effective, notes) = cp.tightest(&current);
        assert_eq!(effective.add_dirs, vec!["D:\\work".to_string()]);
        assert!(notes.is_empty(), "{notes:?}");
    }
}
