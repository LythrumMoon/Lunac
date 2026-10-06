//! 会话回退用的「文件内容快照」（2026-09-30）。
//!
//! 用户要求：按对话里的**回退按钮**时，除了回退对话与 agent 上下文，还要把 agent
//! 改过的磁盘文件一起回退到那个对话点，并在动手前提示「已更改文件将会回退」。
//!
//! 为什么这两条命令落在宿主侧：前端要在**写类工具调用刚发出、尚未执行**的那一刻拿到
//! 文件原内容，而前端没有读任意路径文本的能力 —— `read_tool_file` 只认 MCP 工具目录，
//! `check_file_exists` 只回一个 bool。这里只开最小两条：读一份文本、把一份文本写回去。
//!
//! **纪律（不得回退）**：
//! - **只认真实文件**：`snapshot_read_text` 对目录 / 不存在的路径返回 `existed: false`；
//!   读取本身失败（权限等）按 `Err` 抛出，让前端**放弃**这个文件的快照 —— 宁可少一个
//!   可还原的文件，也不要给用户一个「以为能还原、其实还原不了」的假承诺。
//! - **只处理 UTF-8 文本且限量**：非 UTF-8（二进制）或超过 `MAX_SNAPSHOT_BYTES` 的文件
//!   返回 `existed: true, content: null`。这个组合**表示「原文件在，但内容没留下来」**。
//! - **`existed: true` + `content: null` 一律拒绝写回**：否则会把「没读出来」当成
//!   「内容为空」而把文件清空 —— 这是本模块最危险的一条，判据写在命令里，不靠调用方自觉。
//! - **`existed: false` = 文件本来不存在 ⇒ 回退就是删掉它**（只删文件，不碰目录）。

use std::path::Path;

/// 单个快照的文本上限。超过就放弃还原：写回半截内容比不还原更糟。
const MAX_SNAPSHOT_BYTES: u64 = 512 * 1024;

/// `snapshot_read_text` 的回执：**是否存在**，以及**内容是否留下来了**。
/// `existed: true, content: null` = 文件在，但太大 / 不是 UTF-8 文本 ⇒ 不可还原。
#[derive(serde::Serialize)]
pub struct SnapshotFileDto {
    pub existed: bool,
    pub content: Option<String>,
}

/// 读一份文本用于快照。**不改动任何东西**（纯读）。
#[tauri::command]
pub fn snapshot_read_text(path: String) -> Result<SnapshotFileDto, String> {
    let p = Path::new(&path);
    if !p.is_file() {
        return Ok(SnapshotFileDto { existed: false, content: None });
    }
    let len = std::fs::metadata(p).map(|m| m.len()).unwrap_or(u64::MAX);
    if len > MAX_SNAPSHOT_BYTES {
        return Ok(SnapshotFileDto { existed: true, content: None });
    }
    match std::fs::read(p) {
        Ok(bytes) => Ok(SnapshotFileDto {
            existed: true,
            // 非 UTF-8 ⇒ None（前端据此拒绝还原这个文件）
            content: String::from_utf8(bytes).ok(),
        }),
        Err(e) => Err(format!("读取失败: {e}")),
    }
}

/// 把一份快照写回磁盘（回退的最后一步）。
///
/// - `existed: false` ⇒ 删除该文件（原状态下它不存在）；
/// - `existed: true` ⇒ `content` 必须存在，写回原内容（父目录不在就建）。
#[tauri::command]
pub fn snapshot_restore_text(
    path: String,
    content: Option<String>,
    existed: bool,
) -> Result<(), String> {
    let p = Path::new(&path);
    if !existed {
        if p.exists() {
            std::fs::remove_file(p).map_err(|e| format!("删除失败: {e}"))?;
        }
        return Ok(());
    }
    let text = content.ok_or_else(|| "快照内容不可用（二进制或超出上限），拒绝还原".to_string())?;
    if let Some(dir) = p.parent() {
        if !dir.as_os_str().is_empty() && !dir.exists() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
        }
    }
    std::fs::write(p, text).map_err(|e| format!("写回失败: {e}"))
}

// ── 会话帧落盘（2026-10-01）─────────────────────────────────────────
//
// 「回退到某条消息时把 agent 改过的文件一起还原」这件事原本**只在当前这次对话里有效**：
// 快照数组活在内存，一进历史记录就丢（那是当时的**明确约束**，见 ai-spec §11 规则 64）。
// 本轮改成落盘，落在 `<ModuleData>\history\frames\<session_id>.json`。
//
// **为什么不写进 chat.db**（规则 64 点名的第一件事就是体积）：`chat.db` 是「对话流水」，
// 而快照里装的是**用户文件的原内容**（单文件最多 512KB、一个会话可能几十条）；
// 混进去会把一个纯文本库撑成几百 MB，而且每次「保存会话」都要重写整张表。
// 放独立文件还顺带解决第二件事：**回退时就地裁剪**（见下）。
//
// 三条纪律：
// - **session id 必须逐字符校验**（`safe_session_id`）：它来自前端，直接拼路径等于开一个
//   目录穿越的口子（`../../`）。
// - **先写 `.part` 再改名**：半截 JSON 被读回来会解析失败 ⇒ 表现成「历史会话永远回退不了」，
//   而且一句报错都没有。
// - **会话没了就删**（`prune_frames`）：被删掉的历史会话不该在盘上留着用户文件副本。
//
// 「回退后快照跟着裁剪」这条与内存版一致：前端在 `revertFileSnapshots()` 里过滤完
// 再调 `snapshots_save` 写回 —— 所以**盘上那份永远等于内存那份**，不存在
// 「界面说能还原、盘上其实没有」的分裂。

/// 一条待还原的快照（与前端 `FileSnapshot` 逐字段对应）。
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct FrameDto {
    /// 采集时已完成的助手消息条数（= 回合下标）
    pub turn: u32,
    pub path: String,
    pub existed: bool,
    /// 原内容；`existed && content === null` = 读不出来（二进制 / 超上限）⇒ 拒绝还原
    pub content: Option<String>,
}

fn frames_root() -> std::path::PathBuf {
    crate::storage::module_data_dir().join("history").join("frames")
}

/// 校验 session id：只允许 `[A-Za-z0-9_-]`、长度 1..=64。
///
/// 这条**不是防御性编程**：id 由前端生成并回传，拼进文件路径前必须证明它不可能含
/// 分隔符 / `..` / 盘符。抽成纯函数就是为了能单测这条判据。
fn safe_session_id(id: &str) -> Result<&str, String> {
    if id.is_empty() || id.len() > 64 {
        return Err(format!("非法会话 id（长度 {}）", id.len()));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("非法会话 id（只允许字母 / 数字 / _ / -）".into());
    }
    Ok(id)
}

/// 落盘一个会话的快照表。空表 = **删掉文件**（别在盘上留一个空壳）。
#[tauri::command]
pub fn snapshots_save(session_id: String, frames: Vec<FrameDto>) -> Result<(), String> {
    let id = safe_session_id(&session_id)?.to_string();
    let dir = frames_root();
    let path = dir.join(format!("{id}.json"));
    if frames.is_empty() {
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("建目录失败: {e}"))?;
    let text = serde_json::to_string(&frames).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{id}.json.part"));
    std::fs::write(&tmp, text).map_err(|e| format!("写失败: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("改名失败: {e}"))
}

/// 读一个会话的快照表。没有文件 = **空表**（不是错误：全新会话就是没快照）。
#[tauri::command]
pub fn snapshots_load(session_id: String) -> Result<Vec<FrameDto>, String> {
    let id = safe_session_id(&session_id)?;
    let path = frames_root().join(format!("{id}.json"));
    match std::fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("快照解析失败: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("快照读不出: {e}")),
    }
}

/// 删掉不在 `keep` 里的会话帧（连同 `.part` 半成品）。
///
/// 由 `save_chat_sessions` 在落盘成功后调用 —— 那里是**唯一**知道「现在还剩哪些会话」的地方；
/// 散在前端的删除路径里迟早漏一处，表现是「删了历史，用户文件副本还躺在盘上」。
/// 本函数**不返回错误**：清理失败不该让会话保存失败。
pub fn prune_frames(keep: &[String]) {
    let dir = frames_root();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let keep_files: std::collections::HashSet<String> = keep
        .iter()
        .filter(|s| safe_session_id(s).is_ok())
        .map(|s| format!("{s}.json"))
        .collect();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if keep_files.contains(&name) {
            continue;
        }
        let p = e.path();
        if p.is_file() {
            let _ = std::fs::remove_file(&p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例一个独立临时目录（同进程并行跑也不会互相踩）。
    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lunac-snap-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// 不存在的路径 ⇒ `existed: false`（**不是** Err）：回退时的动作是「把它删掉」。
    #[test]
    fn missing_file_reads_as_not_existed() {
        let dir = tmp_dir("missing");
        let p = dir.join("nope.txt");
        let dto = snapshot_read_text(p.to_string_lossy().into_owned()).expect("read ok");
        assert!(!dto.existed);
        assert!(dto.content.is_none());
    }

    /// 目录按「不是文件」处理 —— 否则 `read` 会 Err，前端会把目录也记成可还原项。
    #[test]
    fn directory_reads_as_not_existed() {
        let dir = tmp_dir("dir");
        let dto = snapshot_read_text(dir.to_string_lossy().into_owned()).expect("read ok");
        assert!(!dto.existed);
    }

    /// **最重要的一条**：`existed: true` + `content: null` 必须拒绝写回。
    /// 放行它等于把「没读出来」当成「内容为空」，会把用户的文件清空。
    #[test]
    fn restore_refuses_when_content_is_missing() {
        let dir = tmp_dir("refuse");
        let p = dir.join("keep.txt");
        std::fs::write(&p, "原文").unwrap();
        let err = snapshot_restore_text(p.to_string_lossy().into_owned(), None, true)
            .expect_err("必须拒绝");
        assert!(err.contains("拒绝还原"), "错误信息要能说明原因: {err}");
        // 文件必须原封不动
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "原文");
    }

    /// `existed: false` ⇒ 回退到「它还不存在」的状态 = 删掉它。
    #[test]
    fn restore_deletes_file_that_did_not_exist() {
        let dir = tmp_dir("delete");
        let p = dir.join("created-by-agent.txt");
        std::fs::write(&p, "agent 写的").unwrap();
        snapshot_restore_text(p.to_string_lossy().into_owned(), None, false).expect("delete ok");
        assert!(!p.exists());
        // 幂等：文件已经不在了也不报错（回退可能被点两次）
        snapshot_restore_text(p.to_string_lossy().into_owned(), None, false).expect("delete again");
    }

    /// 读 → 改 → 写回 = 逐字节回到原状（含中文与换行）。
    #[test]
    fn read_then_restore_round_trips() {
        let dir = tmp_dir("roundtrip");
        let p = dir.join("a.txt");
        let original = "第一行\r\n第二行——含破折号\n";
        std::fs::write(&p, original).unwrap();
        let dto = snapshot_read_text(p.to_string_lossy().into_owned()).expect("read ok");
        assert!(dto.existed);
        assert_eq!(dto.content.as_deref(), Some(original));
        std::fs::write(&p, "被 agent 改过的内容").unwrap();
        snapshot_restore_text(p.to_string_lossy().into_owned(), dto.content, true).expect("restore ok");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), original);
    }

    /// **目录穿越的口子**：session id 由前端回传，拼进文件路径前必须逐字符证明它安全。
    /// 放行 `../` 就等于让「保存快照」这个动作能往盘上任意位置写一个带用户文件内容的 JSON。
    #[test]
    fn session_ids_that_could_escape_are_rejected() {
        for good in ["abc", "sess_123-xy", "A1_b-2", &"x".repeat(64)] {
            assert!(safe_session_id(good).is_ok(), "{good} 应当合法");
        }
        for bad in [
            "",
            "../evil",
            "..\\evil",
            "a/b",
            "a\\b",
            "C:evil",
            "a b",
            "a.json",
            &"x".repeat(65),
        ] {
            assert!(safe_session_id(bad).is_err(), "{bad:?} 必须拒");
        }
    }
}
