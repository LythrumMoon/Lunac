// core-agent/src/subagent_session.rs
// 子代理角色常驻（§待办 3-22 分片 4，2026-10-31）。
//
// 为什么要有它：`subagent_role` 已经把「派子代理」收口成五个可枚举的角色，但每次 `Agent`
// 调用仍然**从零开始** —— 同一个 review 角色上一轮刚读过的文件、刚跑过的测试，下一轮全忘。
// 本模块给「同一角色」留一份**对话存储**：下次同角色派发时把上一段对话接在前面，
// 子代理于是能接着自己上次的结论干。
//
// 与 `checkpoint` 的关系（**不是第二套持久化**）：
//   · 目录挂在检查点根下（`<LUNAC_CHECKPOINT_DIR 或 exe>\ModuleData\tasks\sessions\roles\`），
//     开关也是同一个 —— 只有 `--checkpoint` / `--resume` / `--continue` 时才读写；
//   · 检查点存的是**主对话** history，这里存的是**每个角色一条**子代理 history，互不覆盖。
//
// 四条硬口径：
//   · **按回合裁，不从中间切**：每个回合以 `user` 提问开头、以 `assistant` 报告结尾；
//     超上限时整回合丢弃最旧的 ⇒ 拼起来永远是一条协议合法的 history
//     （绝不出现「开头是 tool_result」「tool_use 没有结果」这种半截对话）。
//   · **不完整就不写**：最后一条不是 `assistant` 且拿不到报告文本时直接拒绝落盘，
//     宁可不常驻，也不留一份下次接不上的文件。
//   · **有上限**：每角色最多 `MAX_TURNS` 个回合、单文件 `MAX_BYTES`；连一个回合都放不下 ⇒
//     放弃写入（不截断、不写半份）。
//   · **失败不致命**：读不到 / 解析失败由调用方按「新会话」继续，只 warn ——
//     常驻是增强，不该让一次派发整个失败。
//
// 边界如实记：常驻会话**不脱敏**，里面是子代理自己看过的工具结果（代码 / 配置原文）。
// 它落在本机模块数据目录，与 `chat.db` 同级信任域；只在用户显式开检查点时产生。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::checkpoint;
use crate::log;

/// 存储格式版本。字段语义变化时递增；读到不认识的版本直接拒绝（不猜、不迁移）。
pub const VERSION: u32 = 1;
/// 每个角色保留的**回合**数（一次派发 = 一个回合）。
///
/// 为什么是回合而不是消息条数：裁消息会切断 `tool_use` / `tool_result` 的配对，
/// 接上去就是一份非法 history。回合边界天然安全。
pub const MAX_TURNS: usize = 4;
/// 单角色文件上限。超了先丢最旧回合；只剩一个回合仍超 ⇒ 放弃写。
pub const MAX_BYTES: usize = 512 * 1024;
/// 角色会话目录名（挂在检查点目录下）。
pub const DIR_NAME: &str = "roles";

/// 一个角色的常驻会话。
#[derive(Debug, Serialize, Deserialize)]
struct Store {
    version: u32,
    role: String,
    updated_ms: i64,
    /// 每次派发一个回合：`[user 提问, assistant …, assistant 报告]`。
    #[serde(default)]
    turns: Vec<Vec<Value>>,
}

/// 角色会话目录：`<检查点目录>\roles`。
pub fn dir() -> PathBuf {
    checkpoint::dir().join(DIR_NAME)
}

/// 读出某角色此前的对话（展平后的 history，可直接接在新提问前面）。
///
/// 文件不存在 = `Ok(vec![])`（第一次派发，属正常形态）；损坏 / 版本不符 / 回合残缺都返回可读
/// `Err`，由调用方决定记 warn 还是当作新会话。
pub fn load(role: &str) -> Result<Vec<Value>, String> {
    load_in(&dir(), role)
}

/// 把这一轮的完整 history 追加成该角色的新回合。返回落盘路径。
///
/// `report` 只在一个窄场景用到：`run_subagent` 撞预算 / 撞轮数时 history 可能停在
/// `tool_result` 上，此时用最终报告补一条 `assistant` 收尾 —— 否则这份记录下次接不上。
pub fn save(role: &str, history: &[Value], report: &str) -> Result<PathBuf, String> {
    save_in(&dir(), role, history, report, now_ms(), MAX_BYTES)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 角色名进文件名前只保留安全字符。
///
/// 角色来自 `Role::as_str()`（固定五个短名），正常不会触发替换；仍收死一层，
/// 免得将来有人把模型给的原始字符串直接传进来。
fn sanitize(role: &str) -> String {
    let s: String = role
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "general".to_string()
    } else {
        s
    }
}

fn path_in(dir: &Path, role: &str) -> PathBuf {
    dir.join(format!("{}.json", sanitize(role)))
}

fn message_role(msg: &Value) -> &str {
    msg.get("role").and_then(Value::as_str).unwrap_or("")
}

/// 一个回合是否**协议完整**：提问开头（user）、报告收尾（assistant）。
///
/// 只判首尾：中间每一步由 `run_subagent` 自己配对写回（`tool_use` 后面必有 `tool_result`），
/// 这里再逐条校验等于把那份不变量维护两遍。
fn turn_is_complete(turn: &[Value]) -> bool {
    !turn.is_empty()
        && message_role(&turn[0]) == "user"
        && message_role(turn.last().unwrap()) == "assistant"
}

/// 读一份存储；文件不存在返回空存储（不是错误）。
fn read_store(dir: &Path, role: &str) -> Result<Store, String> {
    let path = path_in(dir, role);
    if !path.exists() {
        return Ok(Store {
            version: VERSION,
            role: sanitize(role),
            updated_ms: 0,
            turns: Vec::new(),
        });
    }
    let body =
        fs::read_to_string(&path).map_err(|e| format!("读角色会话失败 {}: {e}", path.display()))?;
    let store: Store = serde_json::from_str(&body)
        .map_err(|e| format!("解析角色会话失败 {}: {e}", path.display()))?;
    if store.version != VERSION {
        return Err(format!(
            "角色会话版本不兼容：{}（文件 v{}，当前需要 v{}）",
            path.display(),
            store.version,
            VERSION
        ));
    }
    Ok(store)
}

pub fn load_in(dir: &Path, role: &str) -> Result<Vec<Value>, String> {
    let store = read_store(dir, role)?;
    let total = store.turns.len();
    let mut out: Vec<Value> = Vec::new();
    let mut dropped = 0usize;
    for turn in store.turns {
        if turn_is_complete(&turn) {
            out.extend(turn);
        } else {
            dropped += 1;
        }
    }
    if dropped > 0 {
        log::warn(format!(
            "角色会话 {} 有 {dropped}/{total} 个回合不完整（缺 assistant 收尾），已跳过",
            sanitize(role)
        ));
    }
    Ok(out)
}

fn save_in(
    dir: &Path,
    role: &str,
    history: &[Value],
    report: &str,
    ts_ms: i64,
    max_bytes: usize,
) -> Result<PathBuf, String> {
    let mut turn: Vec<Value> = history.to_vec();
    // 撞预算 / 撞轮数的收尾：history 可能停在 tool_result 上。补一条 assistant 报告
    // —— 补不上就拒绝落盘（见模块头「不完整就不写」）。
    if !turn.is_empty() && message_role(turn.last().unwrap()) != "assistant" {
        let tail = report.trim();
        if !tail.is_empty() {
            turn.push(serde_json::json!({
                "role": "assistant",
                "content": [{ "type": "text", "text": tail }],
            }));
        }
    }
    if !turn_is_complete(&turn) {
        return Err(format!(
            "角色会话 {} 这一轮缺完整首尾（user 提问 / assistant 报告），不落盘",
            sanitize(role)
        ));
    }

    // 读旧存储：损坏时**不当场报错**（否则这一轮白干），保留现场为 `.bad` 后重开。
    let mut turns = match read_store(dir, role) {
        Ok(s) => s.turns,
        Err(e) => {
            log::warn(format!("{e}；本轮按新会话落盘"));
            let path = path_in(dir, role);
            let bad = path.with_extension("json.bad");
            if let Err(err) = fs::rename(&path, &bad) {
                log::warn(format!("保留损坏会话失败 {}: {err}", path.display()));
            }
            Vec::new()
        }
    };
    turns.push(turn);
    if turns.len() > MAX_TURNS {
        turns.drain(..turns.len() - MAX_TURNS);
    }

    loop {
        let body = serde_json::to_string(&Store {
            version: VERSION,
            role: sanitize(role),
            updated_ms: ts_ms,
            turns: turns.clone(),
        })
        .map_err(|e| format!("序列化角色会话失败: {e}"))?;
        if body.len() <= max_bytes {
            let path = path_in(dir, role);
            write_atomic(dir, &path, &body)?;
            log::info(format!(
                "角色会话落盘：{}（{} 回合）",
                path.display(),
                turns.len()
            ));
            return Ok(path);
        }
        if turns.len() > 1 {
            turns.remove(0);
            continue;
        }
        return Err(format!(
            "角色会话 {} 单回合就超过 {max_bytes} 字节，放弃写入（不写半份）",
            sanitize(role)
        ));
    }
}

fn write_atomic(dir: &Path, path: &Path, body: &str) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("建角色会话目录失败 {}: {e}", dir.display()))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, body.as_bytes())
        .map_err(|e| format!("写临时角色会话失败 {}: {e}", tmp.display()))?;
    // Windows 的 rename 不覆盖已存在目标：先删旧再改名（与 checkpoint 同一套纪律）。
    if path.exists() {
        fs::remove_file(path).map_err(|e| format!("替换旧角色会话失败 {}: {e}", path.display()))?;
    }
    fs::rename(&tmp, path).map_err(|e| format!("角色会话改名失败 {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 每个用例一个独立目录，避免并行测试互相看到对方的文件。
    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "lunac-role-session-{}-{}-{tag}",
            std::process::id(),
            now_ms()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).expect("create temp role dir");
        d
    }

    fn one_turn(question: &str, report: &str) -> Vec<Value> {
        vec![
            json!({"role":"user","content":[{"type":"text","text":question}]}),
            json!({"role":"assistant","content":[{"type":"text","text":report}]}),
        ]
    }

    #[test]
    fn saved_turn_is_read_back_in_order() {
        let dir = temp_dir("roundtrip");
        let history = one_turn("q1", "a1");
        let path = save_in(&dir, "review", &history, "a1", 1_000, MAX_BYTES).expect("save");
        assert!(path.exists());
        assert_eq!(load_in(&dir, "review").unwrap(), history, "逐字段还原");
        assert!(
            load_in(&dir, "coding").unwrap().is_empty(),
            "不同角色互不串台"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn turns_are_appended_and_the_oldest_is_dropped_at_the_cap() {
        let dir = temp_dir("cap");
        for i in 1..=MAX_TURNS + 1 {
            let (q, a) = (format!("q{i}"), format!("a{i}"));
            save_in(
                &dir,
                "explore",
                &one_turn(&q, &a),
                &a,
                i as i64 * 1_000,
                MAX_BYTES,
            )
            .unwrap();
        }
        let got = load_in(&dir, "explore").unwrap();
        assert_eq!(got.len(), MAX_TURNS * 2, "只留最近 {MAX_TURNS} 个回合");
        assert_eq!(
            got[0],
            json!({"role":"user","content":[{"type":"text","text":"q2"}]}),
            "最旧的回合被整回合丢弃（不从中间切）"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_turn_that_stops_at_a_tool_result_is_closed_with_the_report() {
        let dir = temp_dir("close");
        // 撞预算的形态：最后一条是 tool_result（user），报告只在返回值里。
        let history = vec![
            json!({"role":"user","content":[{"type":"text","text":"q"}]}),
            json!({"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{}}]}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}),
        ];
        save_in(&dir, "coding", &history, "partial report", 1_000, MAX_BYTES).expect("save");
        let got = load_in(&dir, "coding").unwrap();
        assert_eq!(
            message_role(got.last().unwrap()),
            "assistant",
            "必须补上报告收尾"
        );
        assert!(
            got.last().unwrap().to_string().contains("partial report"),
            "补的是这一轮的真实报告"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unclosable_turn_is_refused_instead_of_written_halfway() {
        let dir = temp_dir("refuse");
        let history = vec![json!({"role":"user","content":[{"type":"text","text":"q"}]})];
        let err = save_in(&dir, "review", &history, "   ", 1_000, MAX_BYTES)
            .expect_err("没有报告可补时必须拒绝");
        assert!(err.contains("缺完整首尾"), "{err}");
        assert!(!path_in(&dir, "review").exists(), "拒绝时不得留下半份文件");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_and_wrong_version_files_are_reported() {
        let dir = temp_dir("corrupt");
        fs::write(path_in(&dir, "review"), b"not json").unwrap();
        assert!(load_in(&dir, "review").is_err(), "坏 JSON 必须报错");
        fs::write(
            path_in(&dir, "coding"),
            br#"{"version":99,"role":"coding","updated_ms":1,"turns":[]}"#,
        )
        .unwrap();
        let err = load_in(&dir, "coding").expect_err("版本不符必须拒绝");
        assert!(err.contains("版本不兼容"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn incomplete_turns_on_disk_are_skipped_on_read() {
        let dir = temp_dir("skip");
        fs::write(
            path_in(&dir, "review"),
            br#"{"version":1,"role":"review","updated_ms":1,"turns":[
                [{"role":"user","content":[]},{"role":"assistant","content":[]}],
                [{"role":"user","content":[]}]
            ]}"#,
        )
        .unwrap();
        let got = load_in(&dir, "review").unwrap();
        assert_eq!(got.len(), 2, "残缺的那个回合整段跳过，好的照常接上");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_oversize_turn_is_refused_rather_than_truncated() {
        let dir = temp_dir("oversize");
        let history = one_turn("q", &"x".repeat(200));
        let err = save_in(&dir, "review", &history, "a", 1_000, 64).expect_err("超限必须拒绝");
        assert!(err.contains("放弃写入"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }
}
