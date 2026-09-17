// core-agent/src/main.rs
// Lunac 自研 agent 核心 —— P1：内置工具循环（Read/Write/Edit/Bash/Glob/Grep）
//
// 本程序是 lunac 桌面端的唯一 agent 后端：遵守既有 stream-json 契约
// （见 docs/ai-spec.md §3、app/src/main.ts 对 `cli-output` 的逐行解析），
// 由 src-tauri 的 start_cli 拉起（二进制落点见 core_dir()）。
//
// 契约速查
//   env    LUNAC_AGENT_BASE_URL（完整端点，请求再拼 /v1/messages）
//          LUNAC_AGENT_TOKEN     （鉴权必须走 Bearer；用 x-api-key 会被
//                                 兼容端点判 401）
//          LUNAC_AGENT_MODEL
//   args   --add-dir <dir>（可重复）工作区外追加目录
//          --permission-mode plan → 只读
//          --dangerously-skip-permissions → 忽略工作区锁
//          --permission-prompt-tool stdio → 写类工具先发 can_use_tool 请前端审批
//          --disallowedTools <name…>      → 这些工具不进请求体
//          --mcp-server stdio:<exe 路径>  → 拉起该 exe 的 MCP server，接入其工具（P3）
//          其余 CLI 风格参数接受并忽略
//   stdin  每行一条 JSON
//            {"type":"user","session_id":"","message":{"role":"user",
//             "content":[{"type":"text","text":"…"}]},"parent_tool_use_id":null}
//            {"type":"control_response","response":{"subtype":"success",
//             "request_id":"…","response":{"behavior":"allow"|"deny",…}}}
//   stdout 每行一条 JSON（非 JSON 行会被前端忽略）
//            {"type":"system","subtype":"init","tools":[…]}
//            {"type":"stream_event","event":{…content_block_delta…}}
//            {"type":"assistant","message":{"content":[…]}}   ← 含 tool_use
//            {"type":"user","message":{"content":[{tool_result}]}}
//            {"type":"control_request","request":{"subtype":"can_use_tool",…}}
//            {"type":"result","subtype":"success","usage":{…}}
//
// 已实现范围
//   ✅ P0 多轮上下文、SSE 增量打字、用量上报、错误回传
//   ✅ P0 思考开关（开/关 → thinking 形态 + 400 降级）
//   ✅ P1 内置工具 + tool_use/tool_result 往返循环（工具实现见 tools.rs）
//   ✅ P2 权限审批：写类工具发 can_use_tool → 阻塞等 control_response（超时按拒绝）
//   ✅ 上下文预算 + 压缩：按端点实测体积走瘦身/丢弃两级水位，400 超限再强制压缩重试
//   ✅ P3 MCP 工具桥：连 `lunac.exe --mcp-server`，把 <exe 根>\tools\*.json 的用户工具
//      以 `mcp__<名>` 接进请求体（实现见 mcp.rs）
//   ✅ P4 技能：`LUNAC_SKILLS_DIR`（=<exe 根>\skills）下的 <key>/SKILL.md，
//      系统提示词列出清单，模型调 Skill 工具取正文（实现见 skills.rs）
//   ❌ 技能 fork / remote 模式、MCP resources
//
// 本文件为 Lunac 自研实现，不派生自任何第三方源码。

use std::cell::Cell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

mod bash_safety;
mod log;
mod mcp;
mod skills;
mod tools;

/// 单次回复的 token 上限（无思考时的基线）
const BASE_MAX_TOKENS: u32 = 8192;
/// 请求总超时（含流式读取整段响应）
const REQUEST_TIMEOUT_SECS: u64 = 1800;
const CONNECT_TIMEOUT_SECS: u64 = 30;
/// 一轮用户提问内最多允许的「模型→工具→模型」往返次数
const MAX_TOOL_ROUNDS: usize = 16;
/// 同一批只读工具的最大并发数。本地读取是毫秒级，4 够用；再加高只会先撞上
/// 端点/搜索源的限流与磁盘争用（见 `plan_tool_batches`）。
const TOOL_PARALLELISM: usize = 4;
/// 审批等待上限：超时按拒绝处理，并通知前端撤掉卡片（避免 UI 丢了以后永久挂住）
const APPROVAL_TIMEOUT_SECS: u64 = 300;

// ── 瞬时失败重试（2026-09）───────────────────────────────────────
//
// 这**不是**「重新生成回答」，而是**请求级**重试：只在**还没读到响应体之前**
// 退避重试，所以永远不会产生重复内容。覆盖范围刻意收窄到「重试有意义」的两类：
//   · 网络层：连接失败 / 连接重置 / 超时（reqwest 的 builder 错误除外）
//   · 429 限流、5xx 服务端故障（含 529「过载」）
// 下面这些**不**走这里，各有专门分支：
//   · 400 + thinking 相关 → 沿思考降级链换形态再试（已存在）
//   · 400 + 上下文超限   → 强制压缩后再试（已存在）
//   · 其余 4xx           → 重试也不会变（鉴权 / 参数错），直接报错
const MAX_API_RETRIES: usize = 3;
/// 首次退避 1s，之后 2s / 4s（指数增长）
const RETRY_BASE_MS: u64 = 1_000;
/// 退避上限（也用来夹住端点给的 `Retry-After`，免得界面长时间无响应）
const RETRY_MAX_MS: u64 = 30_000;

// ── 上下文预算 ───────────────────────────────────────────────────
//
// 端点的上下文窗口是硬限制，超了就是 400；而每轮失败都会把 history 整体回滚，
// 所以不管理体积的话对话会「越用越死」——压不进去就再也发不出去。
//
// 两级处理，都不额外调用模型（省 token、无副作用、可离线推理）：
//   ① 瘦身：把较旧轮次里的大块 tool_result 就地替换成占位串
//      （文件内容 / Grep 结果通常是体积大头，且对后续推理价值递减）
//   ② 丢弃：仍然超预算时，从最老的整条消息开始丢，只保留尾部若干条
// 另对「上下文超限」这类 400 做一次强制压缩后重试，兜住估算误差。

/// 上下文预算（token），可用环境变量覆盖；低于下限的取值视为无效
const MAX_CONTEXT_ENV: &str = "LUNAC_MAX_CONTEXT_TOKENS";
const DEFAULT_MAX_CONTEXT_TOKENS: u64 = 128_000;
const MIN_CONTEXT_TOKENS: u64 = 8_000;
/// 超过预算的该比例 → 先瘦身；超过更高水位 → 直接丢弃
///
/// 水位定得高是刻意的：**每次压缩都会改变请求前缀，端点侧的 KV 缓存整段作废**。
/// 压缩发生得越少，长会话的命中率越高 —— 详情见 docs/ai-spec.md §11 规则 23。
const ELIDE_RATIO: f64 = 0.85;
const DROP_RATIO: f64 = 0.95;
/// 距上次压缩之后至少要再长「预算 × 该比例」才允许第二次动历史（滞回）。
///
/// 没有这道闸，每轮都会有一两条旧消息跨过保留尾部被瘦身 → 前缀每轮都变、
/// 缓存每轮归零（实测是命中率的最大来源）。有它之后压缩变成「成批、间隔足够远」。
const COMPACT_MIN_GROWTH: f64 = 0.15;
/// 压缩时始终保留最近的消息条数
const COMPACT_KEEP_TAIL: usize = 8;
/// tool_result 内容超过该字符数才算「值得瘦身的大块」
const ELIDE_TOOL_RESULT_CHARS: usize = 2_000;
/// 瘦身档的「值不值得」闸门：**可省体积至少要占当前上下文这么大比例**，否则宁可不压。
///
/// 为什么需要：瘦身是**就地改写较早的消息**，端点侧从被改的那条起就再也匹配不上
/// 已落盘的缓存前缀单元（DS 的命中要求「完整匹配缓存前缀单元」）—— 省一点点却让
/// 后面整段失效是**净亏**。旧实现只要水位过 0.85 就压，于是长会话里频繁出现
/// 「省了 2% 体积、废掉 60% 前缀」。**只加在瘦身档**（可选档）；丢弃档与 400
/// 兜底档是安全刚需，照旧无条件压（见 ai-spec §11 规则 23）。
const ELIDE_MIN_SAVINGS_RATIO: f64 = 0.05;
/// 估算字符→token 的经验系数（不引 tokenizer；同 Hermes 的估算口径）
const CHARS_PER_TOKEN: f64 = 4.0;
/// 单条用户输入上限（防一次粘贴把整个窗口顶爆）
const MAX_USER_CHARS: usize = 100_000;
/// 历史被丢弃后插在开头的提示（保证历史以 user 文本消息开头）
const TRIMMED_MARKER: &str =
    "[earlier conversation was trimmed to fit the model context window]";
/// 任务快照的固定表头（backlog §8.3）。英文的原因同 `TRIMMED_MARKER`：这些是**给模型看的
/// 元信息**，不是给用户看的 UI 文案，所以不进 i18n。
const TASK_SNAPSHOT_HEADER: &str =
    "[current task list — pinned so it survives context compaction; keep working on these]";

fn max_context_tokens() -> u64 {
    std::env::var(MAX_CONTEXT_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|n| *n >= MIN_CONTEXT_TOKENS)
        .unwrap_or(DEFAULT_MAX_CONTEXT_TOKENS)
}

/// 该消息是否是 tool_result 载体（这类消息不能作为历史开头）
fn is_tool_result_msg(msg: &Value) -> bool {
    msg.get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
        })
        .unwrap_or(false)
}

/// 该消息里是否含 `TodoWrite` 的 tool_use（backlog §8.3 的任务快照要用）。
fn has_todo_write(msg: &Value) -> bool {
    msg.get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks.iter().any(|b| {
                b.get("type").and_then(Value::as_str) == Some("tool_use")
                    && b.get("name").and_then(Value::as_str) == Some("TodoWrite")
            })
        })
        .unwrap_or(false)
}

/// 把**最近一条** `TodoWrite` 的清单渲染成一段纯文本快照（backlog §8.3「任务快照」）。
///
/// 为什么取「最近一条」而不是累积：`TodoWrite` 的契约就是**每次发完整清单、覆盖上一份**
/// （见 tools.rs 的工具描述），所以最后一条即当前真相。
/// 返回 `None` = 历史里没有 `TodoWrite`，或清单为空 / 结构不对。
fn latest_todo_snapshot(history: &[Value]) -> Option<String> {
    for msg in history.iter().rev() {
        let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
            continue;
        };
        for block in blocks.iter().rev() {
            if block.get("type").and_then(Value::as_str) != Some("tool_use")
                || block.get("name").and_then(Value::as_str) != Some("TodoWrite")
            {
                continue;
            }
            let todos = block
                .get("input")
                .and_then(|i| i.get("todos"))
                .and_then(Value::as_array)?;
            if todos.is_empty() {
                return None;
            }
            let mut out = String::from(TASK_SNAPSHOT_HEADER);
            for (i, t) in todos.iter().enumerate() {
                let content = t.get("content").and_then(Value::as_str).unwrap_or("").trim();
                if content.is_empty() {
                    continue;
                }
                let status = t.get("status").and_then(Value::as_str).unwrap_or("pending");
                out.push_str(&format!("\n{}. [{}] {}", i + 1, status, content));
            }
            return Some(out);
        }
    }
    None
}

/// 判断 400 是否由「上下文超限」引起 —— 只有这类才值得压缩后重试
fn context_related_error(detail: &str) -> bool {
    let d = detail.to_ascii_lowercase();
    d.contains("context") || d.contains("too long") || d.contains("input length")
}

/// 压缩强度。三档对应三个调用场景，区别只在「允不允许丢整条消息」。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Compact {
    /// 只瘦身：把旧的大块 `tool_result` 换成占位串。挤不出来就原样返回 ——
    /// 宁可等到达丢弃水位，也不在这个水位上改历史结构（那等于白丢一次缓存）。
    Elide,
    /// 水位到顶：先瘦身；确实挤不出来（体积在对话本身）才整条丢弃。
    Drop,
    /// 400 兜底：直接丢弃 —— 已被端点判超限，没时间再试一轮。
    Force,
}

/// 压缩结果：改了前缀的条数（瘦身）与丢掉的条数。
/// 调用方据此决定**是否推进滞回时钟** —— 「扫了一圈但决定不动」不算压缩过。
struct CompactOutcome {
    elided: usize,
    dropped: usize,
}

/// 压缩历史。`measured_tokens` = 上一轮实测的上下文体积（0 = 未知），
/// 只喂给瘦身档的「值不值得」闸门（见 ELIDE_MIN_SAVINGS_RATIO）。
fn compact_history(
    history: &mut Vec<Value>,
    mode: Compact,
    measured_tokens: u64,
) -> CompactOutcome {
    let force = mode == Compact::Force;
    // ① 瘦身：只动尾部以外的消息，正在用的最近几轮保持原样。
    //    强制模式下连尾部也瘦（只留最近 2 条）—— 否则尾部若塞了多个超大
    //    tool_result，光靠「丢弃更老的消息」根本压不下来。
    let elide_keep = if force { 2 } else { COMPACT_KEEP_TAIL };
    let tail_start = history.len().saturating_sub(elide_keep);

    // 先**只统计**能省多少，再决定动不动手（原因见 ELIDE_MIN_SAVINGS_RATIO）。
    let mut candidates: Vec<(usize, usize, usize)> = Vec::new();
    for (mi, msg) in history.iter().enumerate().take(tail_start) {
        let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
            continue;
        };
        for (bi, block) in blocks.iter().enumerate() {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            if let Some(Value::String(s)) = block.get("content") {
                let n = s.chars().count();
                if n > ELIDE_TOOL_RESULT_CHARS {
                    candidates.push((mi, bi, n));
                }
            }
        }
    }
    let savable: usize = candidates.iter().map(|(_, _, n)| *n).sum();
    let needed = (measured_tokens as f64 * ELIDE_MIN_SAVINGS_RATIO * CHARS_PER_TOKEN) as usize;
    let skip_elide = mode == Compact::Elide && savable < needed;

    let mut elided = 0usize;
    let mut elided_chars = 0usize;
    if skip_elide {
        eprintln!(
            "[agent] 跳过瘦身：可省 {savable} 字 < 阈值 {needed} 字（上下文≈{measured_tokens} tokens）\
             —— 省下的体积不够抵偿前缀失效，等长够了再压"
        );
    } else {
        for (mi, bi, n) in &candidates {
            let Some(block) = history
                .get_mut(*mi)
                .and_then(|m| m.get_mut("content"))
                .and_then(Value::as_array_mut)
                .and_then(|a| a.get_mut(*bi))
            else {
                continue;
            };
            block["content"] = json!(format!("[elided: {n} chars dropped to save context]"));
            elided += 1;
            elided_chars += n;
        }
    }

    // ② 丢弃：强制模式，或「已到丢弃水位却挤不出来」（说明体积在对话本身）。
    //    只瘦身档永不丢弃 —— 在 0.85 水位上丢整条消息会把缓存一次性废掉，
    //    而按 0.95 水位多等一会儿完全来得及。
    //    始终保留开头那条用户提问 —— 它是任务目标，丢了模型就不知道要干什么。
    let can_drop = match mode {
        Compact::Elide => false,
        Compact::Drop => elided == 0,
        Compact::Force => true,
    };
    let mut dropped = 0usize;
    // 任务快照（backlog §8.3）：`TodoWrite` 的清单躺在**历史中段**，而它恰恰是「当前在做什么」
    // 的唯一载体 —— drop 一压就没了，模型随后就会跑偏（这正是 §8.3 要解决的问题）。
    // 所以**在 drain 之前**先把最近一条抄下来（源马上就不存在了），压缩完再钉回历史开头。
    // 只在「被丢的区间里真的含 TodoWrite」时抄 —— 否则与幸存的那份重复，反而干扰模型。
    let mut pinned_task_snapshot: Option<String> = None;
    if can_drop {
        let head = if history
            .first()
            .map(|m| {
                m.get("role").and_then(Value::as_str) == Some("user") && !is_tool_result_msg(m)
            })
            .unwrap_or(false)
        {
            1
        } else {
            0
        };
        let mut cut = history.len().saturating_sub(COMPACT_KEEP_TAIL).max(head);
        // 不能以 tool_result 开头（它必须紧跟对应的 tool_use）
        while cut < history.len() && is_tool_result_msg(&history[cut]) {
            cut += 1;
        }
        if cut > head {
            if history[head..cut].iter().any(has_todo_write) {
                pinned_task_snapshot = latest_todo_snapshot(history);
            }
            history.drain(head..cut);
            dropped = cut - head;
        }
    }

    // 历史必须以 user 文本消息开头，否则端点可能拒绝
    let starts_ok = history
        .first()
        .map(|m| {
            m.get("role").and_then(Value::as_str) == Some("user") && !is_tool_result_msg(m)
        })
        .unwrap_or(false);
    if !starts_ok {
        history.insert(
            0,
            json!({ "role": "user", "content": [{ "type": "text", "text": TRIMMED_MARKER }] }),
        );
    }

    // 把任务快照钉回去：**插在第一条之后**，不抢「开头那条用户提问 = 任务目标」的位置
    // （丢弃逻辑刻意保留 head 那条，就是为了这个）。
    // 它是一条**纯文本 user 消息**，与 `tool_use` / `tool_result` 的配对结构完全解耦 ——
    // 所以不会像「原样保留那条工具消息」那样把配对拆坏（端点是硬校验的）。
    // 副作用：插在靠前位置 ⇒ 从这条之后的前缀缓存作废。但这只在**已经丢弃过**的轮次才发生，
    // 那一轮本来就已经因为 drain 把缓存废掉了，不算额外损失。
    if let Some(snapshot) = pinned_task_snapshot {
        history.insert(
            1.min(history.len()),
            json!({ "role": "user", "content": [{ "type": "text", "text": snapshot }] }),
        );
        eprintln!("[agent] 任务快照已钉住：丢弃历史时保住了当前任务清单（backlog §8.3）");
    }

    if elided > 0 || dropped > 0 {
        eprintln!(
            "[agent] 上下文压缩：瘦身 {elided} 个 tool_result（省 {elided_chars} 字），丢弃 {dropped} 条旧消息"
        );
        emit(json!({
            "type": "system",
            "subtype": "context_compacted",
            "elided": elided,
            "dropped": dropped,
        }));
    }
    CompactOutcome { elided, dropped }
}

/// 有工具之后，系统提示词改为鼓励「先看再改」的最小操作风格。
/// 技能清单（P4）在 main() 里追加到本提示词之后，见 skills::listing()。
const SYSTEM_PROMPT: &str = "You are Lunac's built-in assistant, running inside a Windows desktop launcher. \
Answer in the user's language and keep it concise. \
You can inspect and modify the local machine with the provided tools: prefer Read/Glob/Grep \
before editing, make the smallest change that solves the problem, and say what you changed. \
Relative paths resolve against your working directory.";

/// 固定的「人格 + 文风」块 —— 系统提示词的第二段（2026-09-17，方案 B）。
///
/// **为什么搬到这里**（原先由前端 `buildSystemPromptHint()` 拼在**每条用户消息最前面**）：
/// 这两段共 1153 字符 ≈ 288 token。位置决定了它**每次提问都必然未命中** —— 新的用户消息
/// 是全新内容，天生不在上一轮的缓存前缀里。实测闲聊类提问的首请求未命中量
/// `in = 236 / 289 / 313` token，与这 288 token 几乎相等（问题本身只占几十 token），
/// 即**首请求未命中的约 90% 就是它**。挪进系统提示词后它成为固定前缀的一部分
/// （进程内逐字节不变，见 ai-spec §11 规则 18），从此**永远命中**。
///
/// 硬约束：**这段必须与请求内容无关**。任何按 query / 时间 / 环境变化的东西都不能进来 ——
/// 那会让系统提示词每轮都变，把整个固定前缀的缓存打掉（这正是原先那份不能留在这里的原因：
/// 它按关键词条件拼接）。关键词条件块（调试方法论 / TDD / 代码审查）**仍留在用户消息里**，
/// 它们本来就随 query 变，且只占自己那几十 token。
const PERSONA_AND_STYLE: &str = r#"## Personality (fixed — always apply)
You are "Lunac", a sharp, fast desktop AI assistant built into a launcher. Stay in character every turn.
- Thinking style: before acting, briefly structure your reasoning as Context → Analysis → Decision, then execute. Do not second-guess after deciding.
- Speaking style: calm and direct, like a senior engineer explaining to a peer. Short varied sentences, first-person "I", concrete nouns and verbs.
- No filler: never use "stands as / testament / delve / tapestry / moreover / furthermore / in conclusion / great question / I hope this helps". Have a clear opinion and recommend the single best option rather than listing everything.
- Always reply in the user's language.

## Output Style
Remove AI writing patterns: no "stands as / testament / pivotal / crucial / underscoring / delve / tapestry / landscape / fostering / moreover / furthermore / in conclusion". No emoji decorations. No "I hope this helps / let me know / great question". No boldface headers in lists. Use simple "is/are/has" instead of "serves as/stands as/represents". Vary sentence rhythm. Have opinions."#;

/// 环境说明（接在 SYSTEM_PROMPT 之后）。
///
/// 为什么需要它：只说「你是 Lunac 的助手」不够 —— 模型在答「我自己的 skills 在哪」这类
/// 问题时只能从文件系统反推，一旦工作区（默认是用户主目录）里躺着**别的 agent 框架**的
/// 目录（`~/.hermes/skills` 之类），它就会把那个框架当成宿主，整个思考过程都锁死在
/// 「我在 hermes 里」上。这里明确给出：宿主是谁、工作目录的绝对路径、**Lunac 自己的
/// 技能目录在哪**，并显式禁止「用磁盘上的文件反推宿主」。
///
/// 稳定性要求（ai-spec §11 规则 18）：内容必须在一次会话内逐字节不变，否则前缀缓存失效
/// —— cwd 与技能目录在 agent 进程生命周期内都是常量，满足该条件。
fn env_block(cwd: &std::path::Path) -> String {
    let skills_dir = std::env::var("LUNAC_SKILLS_DIR")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(not configured)".into());
    format!(
        "\n\nEnvironment:\n\
         - Host: Lunac, a Windows desktop launcher. You are Lunac's built-in agent — not a \
         component of any other agent framework, and not running inside one.\n\
         - Working directory (absolute): {}\n\
         - Lunac's own skills live in: {} — each skill is a folder containing SKILL.md. When the \
         user says \"my skills\", \"我自己的 skills\" or similar, they mean the skills of this app \
         (the ones listed below, if any) or this directory.\n\
         - Other files on disk are ordinary files. If the workspace happens to contain another \
         agent/tool framework's repository, config or skills, do not treat it as Lunac's setup \
         and do not answer as if you were that product.",
        cwd.display(),
        skills_dir
    )
}


// ── 思考开关：跨模型自适应 ───────────────────────────────────────
//
// lunac 的思考只有**开 / 关**两档（2026-09-15 由 fast/think/deep 收敛而来），
// 由 src-tauri 经 `LUNAC_THINKING` 在 spawn 时传入（`off` = 关，其余 = 开）。
//
// 为什么不再做「思考力度」档位：实测本端点**没有力度旋钮** ——
//   · `budget_tokens` 给 1 / 1024 / 32768，思考量完全一样（预算不被 enforce）
//   · `output_config.effort` / `reasoning_effort` 被当未知字段静默忽略
//   · 不发 `thinking` 字段时端点默认就在思考
// 三档在端点上本就退化成两态，UI 也就不该假装有三档。
//
// 但各供应商的兼容端点对 `thinking` 字段的接受度不一样：
//   · DeepSeek   —— 只认 enabled / disabled，传 adaptive 会 400
//   · 原生 Messages 端点的新模型 —— 要求 adaptive，传 enabled+budget 可能 400
//   · Kimi 等兼容层 —— 可能整个字段都不支持，带了就 400
//
// 所以这里**不硬编码模型名单**：按 env 决定首选形态，遇到「与 thinking
// 相关的 400」就沿降级链自动重试一次，并把最终可用的形态缓存在进程内，
// 后续轮次不再试错。

/// 开档随请求附带的思考预算。端点不 enforce 它时该值无意义（实测如此）；
/// 但真会 enforce 的端点要一个合法值，取 8192 是因为它还满足
/// `budget_tokens < max_tokens`（见 `max_tokens_for`）。
const THINKING_BUDGET: u32 = 8192;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Thinking {
    /// `{"type":"disabled"}` —— 关档
    Disabled,
    /// `{"type":"enabled","budget_tokens":THINKING_BUDGET}` —— 开档首选形态
    Budget(u32),
    /// `{"type":"adaptive"}` —— 原生 Messages 端点的新模型（降级链第二站）
    Adaptive,
    /// 完全不发该字段，交由端点默认（降级链终点）
    Omit,
}

/// 纯函数形态（`from_env` 只负责取值），便于单测。
/// 只有明确的「关」才算关，其余（含未设置 / 值不认识）一律当开 ——
/// 与前端默认档一致，也让「注入漏了」退化成可用，而不是静默把思考关掉。
fn thinking_from(value: Option<&str>) -> Thinking {
    match value.map(str::trim) {
        Some("off") | Some("0") | Some("false") => Thinking::Disabled,
        _ => Thinking::Budget(THINKING_BUDGET),
    }
}

impl Thinking {
    fn from_env() -> Self {
        thinking_from(std::env::var("LUNAC_THINKING").ok().as_deref())
    }

    fn to_json(self) -> Option<Value> {
        match self {
            Thinking::Disabled => Some(json!({ "type": "disabled" })),
            Thinking::Budget(n) => Some(json!({ "type": "enabled", "budget_tokens": n })),
            Thinking::Adaptive => Some(json!({ "type": "adaptive" })),
            Thinking::Omit => None,
        }
    }

    /// 降级链。注意 Disabled **不会**退到 Adaptive —— 那等于反过来把思考打开。
    fn next(self) -> Option<Self> {
        match self {
            Thinking::Disabled => Some(Thinking::Omit),
            Thinking::Budget(_) => Some(Thinking::Adaptive),
            Thinking::Adaptive => Some(Thinking::Omit),
            Thinking::Omit => None,
        }
    }
}

/// 端点要求 `budget_tokens < max_tokens`，故开档必须抬高 max_tokens，
/// 否则 8192 的预算配 8192 的上限会被判参数非法。
fn max_tokens_for(plan: Thinking) -> u32 {
    match plan {
        Thinking::Budget(n) => (n + 4096).max(BASE_MAX_TOKENS),
        _ => BASE_MAX_TOKENS,
    }
}

/// 判断 400 是否与思考参数有关 —— 只有这类才值得降级重试，
/// 避免把「模型名不存在」这种无关 400 也白重试一遍。
fn thinking_related_error(detail: &str) -> bool {
    let d = detail.to_ascii_lowercase();
    d.contains("thinking") || d.contains("adaptive") || d.contains("budget_tokens")
}

/// 哪些 HTTP 状态值得退避重试：429 限流、5xx 服务端故障（含 529「过载」）。
/// **4xx 不在此列** —— 除了上面两个专门分支，其余 4xx 重试也不会变。
fn retryable_status(code: u16) -> bool {
    code == 429 || code == 529 || (500..=599).contains(&code)
}

/// 退避时长（毫秒）：优先采用端点给的 `Retry-After`，否则 1s → 2s → 4s… 并加抖动。
/// 抖动是防止多个副本同时重试形成尖峰；取系统时间的亚秒位，不引 rand（见 §11 规则 20）。
fn retry_delay_ms(attempt: usize, retry_after_secs: Option<u64>) -> u64 {
    if let Some(secs) = retry_after_secs {
        return secs.saturating_mul(1000).min(RETRY_MAX_MS);
    }
    let shift = attempt.saturating_sub(1).min(5) as u32;
    let base = RETRY_BASE_MS.saturating_mul(1u64 << shift);
    let jitter = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()) % 250)
        .unwrap_or(0);
    (base + jitter).min(RETRY_MAX_MS)
}

/// 上报一次「瞬时失败 → 退避重试」：前端据此显示状态文案（`system/api_retry`），
/// 同时写进 agent 自己的落盘日志。**进日志的文本必须过 `mask_secrets`**（规则 20）。
fn emit_api_retry(attempt: usize, status: Option<u16>, delay_ms: u64, reason: &str) {
    emit(json!({
        "type": "system",
        "subtype": "api_retry",
        "attempt": attempt,
        "max_retries": MAX_API_RETRIES,
        "error_status": status,
        "delay_ms": delay_ms,
    }));
    log::warn(format!(
        "api retry {attempt}/{MAX_API_RETRIES} after {delay_ms}ms (status={}): {}",
        status.map(|s| s.to_string()).unwrap_or_else(|| "-".into()),
        log::mask_secrets(reason)
    ));
}


// ── 运行配置 ─────────────────────────────────────────────────────

struct Cfg {
    client: reqwest::blocking::Client,
    endpoint: String,
    token: String,
    model: String,
    /// 当前生效的思考形态；400 降级后会被就地改写并缓存（Cell 便于 &Cfg 共享）
    thinking: Cell<Thinking>,
    /// 上一轮请求实测的上下文体积（token）。跨轮保留：新的一轮要在发请求
    /// 之前先按它判断水位，否则第一发就可能超限。
    last_input: Cell<u64>,
    /// 上次压缩时的实测体积 —— 滞回基准（见 `COMPACT_MIN_GROWTH`）。
    /// 0 表示本进程还没压缩过。
    last_compact: Cell<u64>,
}

impl Cfg {
    fn from_env() -> Result<Self, String> {
        let base = std::env::var("LUNAC_AGENT_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .ok_or("LUNAC_AGENT_BASE_URL is not set")?;
        let token = std::env::var("LUNAC_AGENT_TOKEN")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .ok_or("LUNAC_AGENT_TOKEN is not set")?;
        let model = std::env::var("LUNAC_AGENT_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .ok_or("LUNAC_AGENT_MODEL is not set")?;

        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .map_err(|e| format!("build http client: {e}"))?;

        Ok(Self {
            client,
            endpoint: format!("{}/v1/messages", base.trim_end_matches('/')),
            token,
            model,
            thinking: Cell::new(Thinking::from_env()),
            last_input: Cell::new(0),
            last_compact: Cell::new(0),
        })
    }
}

// ── stdout 输出（前端逐行 JSON.parse）────────────────────────────

fn emit(line: Value) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{line}");
    let _ = lock.flush();
}

fn emit_stream_event(event: Value) {
    emit(json!({ "type": "stream_event", "event": event }));
}

/// system/init —— 前端据此把 agentState 从 starting 推进到 idle。
/// 真实 CLI 每轮查询都会发一次，这里保持一致；`tools` 上报本轮可用工具名。
fn emit_init(model: &str, tool_names: &[String]) {
    emit(json!({
        "type": "system",
        "subtype": "init",
        "session_id": "",
        "model": model,
        "tools": tool_names,
    }));
}

// ── 权限审批（P2：can_use_tool control 协议）──────────────────────
//
// 协议（前端 app/src/main.ts 与 vscode-extension/src/chatView.ts 都按这个形状解析）：
//   发出：{"type":"control_request","request_id":"…","request":{
//           "subtype":"can_use_tool","tool_name":"Bash","input":{…},"tool_use_id":"…"}}
//   收回：{"type":"control_response","response":{"subtype":"success",
//           "request_id":"…","response":{"behavior":"allow","updatedInput":{…}}}}
//         behavior=deny 时另带 message / interrupt。
//
// stdin 读取线程负责分发（查询线程此时正阻塞等回包），所以用一个
// request_id → Sender 的登记表把两边接起来。

fn pending_approvals() -> &'static Mutex<HashMap<String, mpsc::Sender<Value>>> {
    static REG: OnceLock<Mutex<HashMap<String, mpsc::Sender<Value>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_request_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    format!("req_{}_{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed))
}

/// 把一条 `control_response` 投递给正在等它的工具调用。
/// 返回 false 表示没有匹配的等待者（交给主循环按普通消息忽略）。
fn route_control_response(msg: &Value) -> bool {
    let Some(req_id) = msg
        .pointer("/response/request_id")
        .and_then(Value::as_str)
    else {
        return false;
    };
    let sender = pending_approvals()
        .lock()
        .ok()
        .and_then(|reg| reg.get(req_id).cloned());
    match sender {
        Some(tx) => {
            let _ = tx.send(msg.clone());
            true
        }
        None => {
            eprintln!("[agent] 收到无对应请求的 control_response（{req_id}），已忽略");
            false
        }
    }
}

enum Decision {
    /// allow；值为最终输入（前端回 `updatedInput: {}` 表示「用原参数」）
    Allow(Value),
    /// deny：拒绝原因 + 是否中断整轮
    Deny(String, bool),
}

/// 一个已发出、等待回包的审批请求
struct Pending {
    request_id: String,
    rx: mpsc::Receiver<Value>,
}

/// 只登记 + 发请求，不阻塞 —— 一批工具先全部发出，前端才能把连续
/// Bash 合并成一行（`findLastBashGroup`）再让用户一次性决定。
fn open_approval(tool_name: &str, tool_use_id: &str, input: &Value) -> Pending {
    let request_id = next_request_id();
    let (tx, rx) = mpsc::channel::<Value>();
    if let Ok(mut reg) = pending_approvals().lock() {
        reg.insert(request_id.clone(), tx);
    }
    eprintln!("[agent] 等待审批 {tool_name}");
    let mut request = json!({
        "subtype": "can_use_tool",
        "tool_name": tool_name,
        "input": input,
        "tool_use_id": tool_use_id,
    });
    // 命令类工具附上**执行侧**的静态安全分析（见 bash_safety.rs）：
    // 前端只拿到命令字符串，正则挡不住引号拼接 / 包装器 / 变量 / 串联的后半段。
    // 这里只**上报判定**、不代替前端决策 —— 前端仍是「自动放行 / 弹审批」的唯一决策点。
    if matches!(tool_name, "Bash" | "PowerShell") {
        if let Some(cmd) = input.get("command").and_then(Value::as_str) {
            let report = bash_safety::analyze(cmd);
            if !report.is_clean() {
                log::warn(format!(
                    "静态安全分析 {tool_name}: dangerous=[{}] opaque=[{}]",
                    report.dangerous.join("、"),
                    report.opaque.join("、"),
                ));
            }
            request["analysis"] = json!({
                "dangerous": report.dangerous,
                "opaque": report.opaque,
            });
        }
    }
    emit(json!({
        "type": "control_request",
        "request_id": request_id,
        "request": request,
    }));
    Pending { request_id, rx }
}

/// 阻塞等回包（带超时）
fn await_approval(pending: Pending, original: &Value) -> Decision {
    let reply = pending.rx.recv_timeout(Duration::from_secs(APPROVAL_TIMEOUT_SECS));
    if let Ok(mut reg) = pending_approvals().lock() {
        reg.remove(&pending.request_id);
    }

    let Ok(msg) = reply else {
        eprintln!("[agent] 审批超时，按拒绝处理");
        emit(json!({
            "type": "control_cancel_request",
            "request_id": pending.request_id,
        }));
        return Decision::Deny(format!("审批超时（{APPROVAL_TIMEOUT_SECS} 秒无响应）"), true);
    };

    let inner = &msg["response"]["response"];
    match inner.get("behavior").and_then(Value::as_str).unwrap_or("deny") {
        "allow" => {
            match inner.get("updatedInput") {
                // 非空对象 = 用户/前端改过参数；空对象 = 按原参数执行
                Some(v)
                    if v.is_object()
                        && v.as_object().map(|o| !o.is_empty()).unwrap_or(false) =>
                {
                    Decision::Allow(v.clone())
                }
                _ => Decision::Allow(original.clone()),
            }
        }
        _ => {
            eprintln!("[agent] 用户拒绝");
            Decision::Deny(
                inner
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("User denied this action in Lunac")
                    .to_string(),
                inner.get("interrupt").and_then(Value::as_bool).unwrap_or(false),
            )
        }
    }
}

// ── 启动参数 ─────────────────────────────────────────────────────
//
// src-tauri 传的是上游 CLI 风格命令行，这里只挑对 P1/P2 有意义的几个：
//   --add-dir <dir>                工作区之外追加可访问目录（可重复）
//   --permission-mode <mode>       plan = 只读；acceptEdits/其他 = 可写
//   --dangerously-skip-permissions 忽略工作区锁（full 档）
//   --permission-prompt-tool stdio 审批走 stdout/stdin 的 control 协议（P2）
//   --disallowedTools <name…>      variadic：这些工具不进请求体
// 其余（--print / --verbose / --input-format …）一律接受并忽略。
#[derive(Default)]
struct CliArgs {
    add_dirs: Vec<PathBuf>,
    permission_mode: String,
    skip_permissions: bool,
    ask_permission: bool,
    disallowed: Vec<String>,
    /// `--mcp-server stdio:<exe 路径>` —— 上游把 lunac.exe 路径交给我们，
    /// 由 agent 侧拉起它的 MCP server（见 mcp.rs）
    mcp_server: Option<String>,
}

impl CliArgs {
    fn parse(args: &[String]) -> Self {
        let mut out = CliArgs::default();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--add-dir" => {
                    if let Some(v) = args.get(i + 1) {
                        out.add_dirs.push(PathBuf::from(v));
                        i += 1;
                    }
                }
                "--permission-mode" => {
                    if let Some(v) = args.get(i + 1) {
                        out.permission_mode = v.clone();
                        i += 1;
                    }
                }
                "--permission-prompt-tool" => {
                    // 只有 stdio 才能把审批路由到前端；没这个开关就别问，
                    // 否则会对着没人应答的通道干等（VSCode 扩展/桌面端都会传）
                    if let Some(v) = args.get(i + 1) {
                        out.ask_permission = v.starts_with("stdio");
                        i += 1;
                    }
                }
                "--dangerously-skip-permissions" => out.skip_permissions = true,
                "--mcp-server" => {
                    if let Some(v) = args.get(i + 1) {
                        out.mcp_server = Some(v.clone());
                        i += 1;
                    }
                }
                "--disallowedTools" => {
                    // variadic：吃到下一个 --flag 为止（与上游 commander 语义一致）
                    let mut j = i + 1;
                    while j < args.len() && !args[j].starts_with("--") {
                        out.disallowed.push(args[j].clone());
                        j += 1;
                    }
                    i = j.saturating_sub(1);
                }
                _ => {}
            }
            i += 1;
        }
        out
    }
}

// ── 入口 ─────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("agent 0.2.0 (lunac self-developed agent core, P1)");
        return;
    }
    // 落盘日志（<exe 根>\temp\logs\agent-YYYY-MM-DD.log）。release 是 GUI 子系统、
    // 没有控制台，下面所有 eprintln 线上都拿不到 —— 出问题只能靠这个文件回溯。
    log::init("agent");
    let cli = CliArgs::parse(&args);
    // 落盘目录（<exe 根>\temp\tool-outputs）必须进可访问范围：单条工具输出超预算时
    // 全文落在那里，模型要自己 Read/Grep 取回 —— 工作区锁会拦工作区外的路径。
    let output_dir = tools::prepare_output_dir();
    log::info(format!("工具输出落盘目录: {}", output_dir.display()));
    let mut add_dirs = cli.add_dirs.clone();
    add_dirs.push(output_dir);
    let tools_ctx = tools::Ctx {
        cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        add_dirs,
        read_only: cli.permission_mode == "plan",
        locked: std::env::var("LUNAC_WORKSPACE_LOCKED").ok().as_deref() == Some("1")
            && !cli.skip_permissions,
    };

    // P3 MCP 工具桥：把 <exe 根>\tools\*.json 的用户工具接进工具池。
    // 连不上只是少一批工具 —— 七件内置工具必须照常可用，所以这里只记一行。
    // plan（只读）档不接：MCP 工具的 handler 能跑 shell / 发 HTTP，接进来也只会
    // 每次都被拒绝，还会让工具清单随档位漂移。
    let mut mcp_bridge = if tools_ctx.read_only {
        eprintln!("[agent] 只读（plan）档：不接入 MCP 工具");
        None
    } else {
        match cli.mcp_server.as_deref() {
            Some(spec) => match mcp::Bridge::connect(spec, &cli.disallowed) {
                Ok(b) => {
                    eprintln!(
                        "[agent] MCP 桥已接通，用户工具 {} 个: [{}]",
                        b.defs().len(),
                        tools::names(b.defs()).join(",")
                    );
                    Some(b)
                }
                Err(e) => {
                    eprintln!("[agent] MCP 桥未接通（继续用内置工具）: {e}");
                    None
                }
            },
            None => None,
        }
    };

    // P4 技能：`LUNAC_SKILLS_DIR`（=<exe 根>\skills）下的 <key>/SKILL.md。
    // 渐进披露 —— 系统提示词只列 key + 描述，正文由 `Skill` 工具按需加载；
    // 读不出来就当作没有技能，不影响其它工具。
    let skills = skills::load();
    let skills_on = !skills.is_empty() && !cli.disallowed.iter().any(|d| d == "Skill");
    if !skills.is_empty() {
        eprintln!(
            "[agent] 技能 {} 个{}: [{}]",
            skills.len(),
            if skills_on { "" } else { "（已被 --disallowedTools 禁用）" },
            skills
                .iter()
                .map(|s| s.key.as_str())
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    let system_prompt = format!(
        "{SYSTEM_PROMPT}\n\n{PERSONA_AND_STYLE}{}{}",
        env_block(&tools_ctx.cwd),
        skills::listing(if skills_on { &skills } else { &[] })
    );

    let mut tool_defs = tools::defs(&cli.disallowed);
    let mut tool_names = tools::names(&tool_defs);
    if skills_on {
        tool_defs.push(skills::tool_def());
        tool_names.push("Skill".into());
    }
    if let Some(b) = &mcp_bridge {
        tool_defs.extend(b.defs().iter().cloned());
        tool_names.extend(tools::names(b.defs()));
    }

    // ── 固定前缀体积自检 ───────────────────────────────────────────
    // 每轮请求都要重发的常量只有三块：system（身份 + 环境块 + 技能清单）、
    // tools（内置 + Skill + MCP/用户工具）。**前缀缓存优化前必须先知道这三块各有多大**
    // ——否则改了也不知道省在哪（字符数 ÷ 4 ≈ token，与 Hermes 的估算口径一致）。
    {
        let sys_chars = system_prompt.chars().count();
        let tools_chars: usize = tool_defs
            .iter()
            .filter_map(|d| serde_json::to_string(d).ok())
            .map(|s| s.chars().count())
            .sum();
        let ctx = max_context_tokens();
        let total_tokens = (sys_chars + tools_chars) as f64 / CHARS_PER_TOKEN;
        log::info(format!(
            "固定前缀 system={sys_chars}字 tools={}个/{tools_chars}字 合计≈{} tokens（budget={ctx}，占 {:.1}%）",
            tool_defs.len(),
            total_tokens.round() as u64,
            total_tokens / ctx as f64 * 100.0,
        ));
    }

    eprintln!(
        "[agent] P1–P4 就绪 cwd={} 工具=[{}]{}{}",
        tools_ctx.cwd.display(),
        tool_names.join(","),
        if tools_ctx.read_only { " 只读模式" } else { "" },
        if !cli.ask_permission {
            ""
        } else if tools_ctx.read_only {
            // 只读档只有 WebSearch / WebFetch / AskUserQuestion 需要审批
            // （写类工具直接被拒）
            " 网络与提问需审批"
        } else {
            " 写操作需审批"
        }
    );

    let cfg = match Cfg::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[agent] {e}");
            emit(json!({
                "type": "result",
                "subtype": "error_during_execution",
                "is_error": true,
                "num_turns": 0,
                "result": e,
                "session_id": "",
            }));
            std::process::exit(1);
        }
    };

    log::info(format!(
        "cfg: endpoint={} model={} token={}",
        log::mask_secrets(&cfg.endpoint),
        cfg.model,
        if cfg.token.is_empty() { "(empty)" } else { "(set)" }
    ));

    // stdin 读取线程 → 查询线程（mpsc 解耦）。
    // 解耦的意义：查询线程在等审批回包时会阻塞，stdin 必须另有线程持续 drain，
    // 否则 control_response 根本读不到（这正是 P2 的前提）。
    let (tx, rx) = mpsc::channel::<Value>();
    thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(trimmed) {
                Ok(v) => {
                    // 审批回包直接投给等它的工具调用，不进主队列
                    if route_control_response(&v) {
                        continue;
                    }
                    if tx.send(v).is_err() {
                        break;
                    }
                }
                Err(e) => eprintln!("[agent] 忽略非法 JSON 输入行: {e}"),
            }
        }
        // stdin 关闭 ⇒ 上游（lunac.exe）已退出
    });

    // system/init 由 run_query 每轮发出（前端据此推进 agentState），启动时不必发
    let mut history: Vec<Value> = Vec::new();
    for msg in rx {
        match msg.get("type").and_then(Value::as_str) {
            Some("user") => {
                let prompt = extract_user_text(&msg);
                if prompt.trim().is_empty() {
                    continue;
                }
                run_query(
                    &cfg,
                    &mut history,
                    &prompt,
                    &tools_ctx,
                    &tool_defs,
                    &tool_names,
                    cli.ask_permission,
                    mcp_bridge.as_mut(),
                    &skills,
                    &system_prompt,
                );
            }
            // 会话历史整体替换（宿主 → agent，2026-09-17）。
            // 用途：① 回退历史后让 agent 的上下文与界面**保留下来的那部分**一致。
            // 旧实现靠宿主侧 `stop_cli` + `start_cli` 把上下文整个清空，代价是保留下来
            // 的上文也一起丢了 —— 用户表现为「回退后引用不到上文」。
            // ② 从磁盘恢复一个旧会话后把它的消息灌回来（恢复只重建了 DOM，agent 侧为空）。
            // **只替换历史、不触发模型调用**：这不是一次提问，不该产生回答与 token。
            Some("set_history") => {
                let msgs = msg
                    .get("messages")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                history = normalize_history(msgs);
                eprintln!("[agent] set_history: {} 条", history.len());
                log::info(format!("set_history: {} 条", history.len()));
                emit(json!({
                    "type": "system",
                    "subtype": "history_set",
                    "messages": history.len(),
                }));
            }
            // 没被认领的 control_response（如请求已超时）在这里丢弃即可
            Some("control_response") => {}
            other => eprintln!("[agent] 忽略输入类型: {other:?}"),
        }
    }
    eprintln!("[agent] stdin closed, exiting");
    log::info("=== agent exit (stdin closed) ===");
}

/// 把宿主送来的「用户 / 助手」纯文本消息列表转成 API 历史。
///
/// 三条规则：
/// 1. **只认 `user` / `assistant`**，认不出的角色直接丢 —— 宁可少一条，也不要把
///    端点不认的形状发出去换回一个 400。
/// 2. **丢掉空文本**：空 content 会让部分端点报错，也没有语义。
/// 3. **合并连续同角色**：宿主手里只有每条消息的 `role` + 纯文本（工具调用细节不落
///    前端），而 Anthropic 形态的 `messages` 要求 role 交替 —— 回退到某条用户消息后
///    紧接着的新提问，会与它构成两条连续 `user` ⇒ 端点 400。合并成一条既合法、语义
///    又不变（同角色的相邻文本本来就是一段连续输入）。
fn normalize_history(msgs: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in msgs {
        let role = match m.get("role").and_then(Value::as_str) {
            Some("user") => "user",
            Some("assistant") => "assistant",
            _ => continue,
        };
        let text = m
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if text.is_empty() {
            continue;
        }
        if let Some(last) = out.last_mut() {
            if last.get("role").and_then(Value::as_str) == Some(role) {
                if let Some(arr) = last.get_mut("content").and_then(Value::as_array_mut) {
                    arr.push(json!({ "type": "text", "text": text }));
                    continue;
                }
            }
        }
        out.push(json!({
            "role": role,
            "content": [{ "type": "text", "text": text }],
        }));
    }
    out
}

/// 从上游消息里取出纯文本。content 既可能是 block 数组，也可能是裸字符串。
fn extract_user_text(msg: &Value) -> String {
    match msg.pointer("/message/content") {
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(Value::as_str) == Some("text") {
                    b.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

// ── 单轮查询（含工具循环）────────────────────────────────────────
//
// 一轮 =「请求 → 流式解析 → 若有 tool_use 就执行并回灌 tool_result → 再请求」，
// 直到模型不再调用工具（或达到 MAX_TOOL_ROUNDS）为止，最后发一条 result。
// 失败时把 history 回滚到本轮开始前，避免半截对话污染后续上下文。

#[derive(Default)]
struct Block {
    kind: String,
    id: String,
    name: String,
    /// text / thinking 的累积内容
    text: String,
    /// tool_use 的 input_json_delta 累积
    json: String,
    /// 端点直接在 content_block_start 里给全量 input 时用它
    input: Value,
}

/// 需要审批的工具：内置写类四件（Write/Edit/Bash/PowerShell）+ WebSearch /
/// WebFetch（会把数据发往外部）+ AskUserQuestion（交互本身就是它的功能）
/// + 全部 MCP 工具 —— 后者的 handler 能跑 shell / 发 HTTP，且定义来自
/// 用户 JSON，agent 侧无权替用户判断安全性，一律交前端卡片决定。
/// `TodoWrite` 不在其中：它只改前端那块待办面板，不碰本机任何东西。
fn needs_approval(name: &str) -> bool {
    tools::needs_approval(name) || mcp::is_mcp(name)
}

/// 分发一次工具调用：技能（P4）→ MCP 桥（P3）→ 内置实现（P1）。
///
/// 三类工具都从这里过，所以**日志也打在这里**：名称 + 参数摘要（脱敏）+ 结果或
/// 错误 + 耗时。线上出问题（例如某个 PowerShell 调用报错）时，这是唯一能事后
/// 还原「模型到底让工具干了什么、工具回给它什么」的地方。
fn run_tool(
    tctx: &tools::Ctx,
    mcp_bridge: Option<&mut mcp::Bridge>,
    skill_list: &[skills::Skill],
    name: &str,
    input: &Value,
) -> Result<String, String> {
    let started = Instant::now();
    let args = tools::summarize_args(input);
    // 单条工具输出预算的唯一出口：超预算落盘全文、只内联头尾（见 tools::apply_budget）。
    // 必须在这里做 —— 只有这一层同时拿到工具名与未经裁剪的完整输出。
    let result = dispatch_tool(tctx, mcp_bridge, skill_list, name, input)
        .map(|out| tools::apply_budget(name, out));
    let ms = started.elapsed().as_millis();

    match &result {
        Ok(out) => {
            log::info(format!(
                "tool {name} ok ({ms}ms, out {} chars) args={args}",
                out.chars().count()
            ));
            log::debug(format!("tool {name} output:\n{out}"));
        }
        Err(e) => log::warn(format!("tool {name} FAILED ({ms}ms) args={args} :: {e}")),
    }
    result
}

fn dispatch_tool(
    tctx: &tools::Ctx,
    mcp_bridge: Option<&mut mcp::Bridge>,
    skill_list: &[skills::Skill],
    name: &str,
    input: &Value,
) -> Result<String, String> {
    // 结果**不在这里**截断 —— 单条输出预算统一由 run_tool 的
    // `tools::apply_budget` 收口（那里才知道工具名，超长要落盘）。
    if name == "Skill" {
        return skills::run(skill_list, input);
    }
    if !mcp::is_mcp(name) {
        return tools::run(tctx, name, input);
    }
    // plan（只读）档：MCP 工具同样不许动手
    if tctx.read_only {
        return Err("MCP tools are disabled in read-only (plan) mode".into());
    }
    let Some(bridge) = mcp_bridge else {
        return Err(format!("MCP bridge is not connected, cannot call {name}"));
    };
    bridge.call(name, input)
}

/// 组装回灌给模型的 `tool_result` 块。顺序必须与 `tool_use` 原顺序一致
/// （并行只改变执行时机，不改变回灌顺序）。
fn tool_result_block(id: &str, text: String, is_error: bool) -> Value {
    let mut blk = json!({ "type": "tool_result", "tool_use_id": id, "content": text });
    if is_error {
        blk["is_error"] = json!(true);
    }
    blk
}

/// 执行单条工具调用 → (回灌文本, 是否错误)。
/// `denied` 非空 = 用户在审批卡上拒绝，直接回错误文本、不执行。
fn run_one_tool(
    tctx: &tools::Ctx,
    mcp_bridge: Option<&mut mcp::Bridge>,
    skill_list: &[skills::Skill],
    name: &str,
    run_input: &Value,
    denied: Option<&str>,
) -> (String, bool) {
    if let Some(msg) = denied {
        return (format!("Error: {msg}"), true);
    }
    eprintln!("[agent] 执行工具 {name}");
    match run_tool(tctx, mcp_bridge, skill_list, name, run_input) {
        Ok(s) => (s, false),
        Err(e) => (format!("Error: {e}"), true),
    }
}

/// 把一轮的工具调用切成执行批：**连续的**只读调用合成一批（批内并行，见
/// `tools::parallel_safe`），其余各自成批（串行）。返回 `(是否并行, 下标区间)`，
/// 区间按原顺序无缝覆盖全部调用。
///
/// **为什么必须是「连续」段**：只读批绝不允许跨越写类调用 —— 否则
/// 「写 A → 读 A」会被重排成「读 A（旧内容）→ 写 A」，错得无声无息。
/// 单元素的只读段不标并行：省一次线程 spawn，行为与串行完全一致。
fn plan_tool_batches(calls: &[(String, String, Value)]) -> Vec<(bool, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < calls.len() {
        if tools::parallel_safe(&calls[i].1) {
            let start = i;
            while i < calls.len() && tools::parallel_safe(&calls[i].1) {
                i += 1;
            }
            out.push((i - start > 1, start..i));
        } else {
            out.push((false, i..i + 1));
            i += 1;
        }
    }
    out
}

fn run_query(
    cfg: &Cfg,
    history: &mut Vec<Value>,
    prompt: &str,
    tctx: &tools::Ctx,
    tool_defs: &[Value],
    tool_names: &[String],
    ask_permission: bool,
    mut mcp_bridge: Option<&mut mcp::Bridge>,
    skill_list: &[skills::Skill],
    system_prompt: &str,
) {
    let started = Instant::now();
    emit_init(&cfg.model, tool_names);

    // 单条输入过长就直接截断：一次粘贴可能把整个上下文窗口顶爆，
    // 与其让端点 400 不如先留个明确的截断标记。
    let prompt_owned;
    let prompt = if prompt.chars().count() > MAX_USER_CHARS {
        eprintln!("[agent] 用户输入过长（> {MAX_USER_CHARS} 字符），已截断");
        prompt_owned = format!(
            "{}\n\n[truncated: input exceeded {MAX_USER_CHARS} chars]",
            prompt.chars().take(MAX_USER_CHARS).collect::<String>()
        );
        prompt_owned.as_str()
    } else {
        prompt
    };

    // 回滚锚点：本轮压入的所有消息（user / assistant / tool_result）都在其后。
    // 压缩会丢掉历史开头的消息，锚点需同步左移（见 compact_history 的返回值）。
    let mut base = history.len();
    history.push(json!({
        "role": "user",
        "content": [{ "type": "text", "text": prompt }],
    }));

    let budget = max_context_tokens();

    // 整轮累计用量（跨多次往返；前端按累计值做差，故不能只报最后一次）
    let mut in_tokens: u64 = 0;
    let mut out_tokens: u64 = 0;
    let mut cache_read: u64 = 0;
    let mut cache_create: u64 = 0;
    // 每次 API 请求的用量明细（**对账粒度**）：平台上「一次带工具的提问」就是**多行**，
    // 而本地按提问只落一行 → 命中率无法逐行对齐（见 ai-spec §11 规则 23 的粒度差提醒）。
    // 这里把每次请求的 in/read/create/out 也带上，前端写进 usage-*.jsonl。
    let mut req_log: Vec<Value> = Vec::new();
    let mut cur_in: u64 = 0;
    let mut cur_read: u64 = 0;
    let mut cur_create: u64 = 0;
    let mut cur_out: u64 = 0;
    let mut final_text = String::new();
    let mut turns = 0usize;
    let mut rounds = 0usize;
    let mut hint_sent = false;

    loop {
        turns += 1;

        // ── 上下文水位检查（发请求之前）─────────────────────────
        // 用上一轮实测的输入体积作基准（比按字符估算准），过了 ELIDE 水位
        // 先瘦身、过了 DROP 水位才允许丢弃。
        //
        // 滞回（`last_compact`）：瘦身档必须间隔足够远才允许再动历史 ——
        // 否则每轮都有旧消息跨过保留尾部被瘦身，前缀每轮都变、缓存每轮归零。
        // 丢弃档（0.95）不受滞回约束：到了那个水位不压就可能 400，安全性优先。
        let measured = cfg.last_input.get();
        if measured > 0 {
            let ratio = measured as f64 / budget as f64;
            let grew_enough =
                measured > cfg.last_compact.get() + (budget as f64 * COMPACT_MIN_GROWTH) as u64;
            if ratio > DROP_RATIO {
                let out = compact_history(history, Compact::Drop, measured);
                base = base.saturating_sub(out.dropped);
                cfg.last_input.set(0);
                cfg.last_compact.set(measured);
            } else if ratio > ELIDE_RATIO && grew_enough {
                let out = compact_history(history, Compact::Elide, measured);
                base = base.saturating_sub(out.dropped);
                cfg.last_input.set(0);
                // 闸门判定「不值得」时什么都没改 —— 那时**不推进滞回时钟**，
                // 否则会白等一个 15% 增长窗口才重新评估（压缩次数统计也不会被污染）。
                if out.elided > 0 {
                    cfg.last_compact.set(measured);
                }
            }
        }

        // 兜底：本轮内被 400 判为上下文超限时，强制压缩后再试一次
        let mut compacted_for_retry = false;
        // 本轮已用掉的「瞬时失败重试」次数（每轮重置；见 MAX_API_RETRIES）
        let mut transient_attempt = 0usize;

        // 发送（思考形态可降级重试 / 瞬时失败退避重试）：只有「与 thinking 相关的
        // 400」才沿降级链前进一次，并把可用形态写回 cfg 缓存，后续轮次不再试错。
        let resp = loop {
            let plan = cfg.thinking.get();
            let mut body = json!({
                "model": cfg.model,
                "max_tokens": max_tokens_for(plan),
                "stream": true,
                "system": system_prompt,
                "messages": history,
            });
            if !tool_defs.is_empty() {
                body["tools"] = json!(tool_defs);
            }
            if let Some(t) = plan.to_json() {
                body["thinking"] = t;
            }

            // ── 请求前缀指纹（2026-09-17，ai-spec §11 规则 23 的归因埋点）──────
            // 要回答的问题：同会话内跨提问时，上一次请求明明命中到 `read=N`，下一次的
            // `read` 却回落到 2048 / 2304（实测 `usage-2026-09-16.jsonl`：记录 6 末次
            // `read=3712` → 记录 7 首次 `read=2304`，丢 1408；记录 8→9 丢 3584）。
            // 静态读用量日志**无法判定**这是「本侧前缀被改写」还是「端点侧淘汰了已落盘
            // 的缓存单元」—— 两者的 read 曲线一模一样。
            // 所以每次请求前把三块前缀的**指纹 + 体积**落一行（只记哈希不记原文，
            // 前缀里可能含用户文件内容，哈希天然脱敏）：
            //   · 三块指纹与上一次逐字节相同而 read 掉了 ⇒ 端点侧淘汰，本侧无责；
            //   · 某一块指纹变了 ⇒ 本侧改了前缀，直接去那一块找原因
            //     （system = 身份/环境块/技能清单，tools = 工具 schema，history = 历史）。
            // 放在 send 之前：重试路径 `continue` 回来会再记一行，正好能看出「同一次
            // 提问的哪次尝试前缀变了」。
            {
                let tools_json = serde_json::to_string(&tool_defs).unwrap_or_default();
                let hist_json = serde_json::to_string(history).unwrap_or_default();
                log::info(format!(
                    "请求前缀 #{} system={:016x}/{}字 tools={:016x}/{}字 history={:016x}/{}条/{}字",
                    turns,
                    log::hash64(system_prompt),
                    system_prompt.chars().count(),
                    log::hash64(&tools_json),
                    tools_json.chars().count(),
                    log::hash64(&hist_json),
                    history.len(),
                    hist_json.chars().count(),
                ));
            }

            let sent = cfg
                .client
                .post(&cfg.endpoint)
                .header("authorization", format!("Bearer {}", cfg.token))
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .json(&body)
                .send();

            let r = match sent {
                Ok(r) => r,
                Err(e) => {
                    // 网络层瞬时故障（连接失败 / 重置 / 超时）→ 退避重试。
                    // builder 类错误（URL 非法、TLS 配置错）重试也不会成功，直接失败。
                    if !e.is_builder() && transient_attempt < MAX_API_RETRIES {
                        transient_attempt += 1;
                        let delay = retry_delay_ms(transient_attempt, None);
                        emit_api_retry(transient_attempt, None, delay, &e.to_string());
                        thread::sleep(Duration::from_millis(delay));
                        continue;
                    }
                    return finish_error(
                        history,
                        base,
                        &format!("请求失败: {e}"),
                        started,
                        turns,
                    )
                }
            };
            if r.status().is_success() {
                break r;
            }

            let status = r.status();
            // `Retry-After`（秒）由端点给出时优先采用；非数字形态（HTTP-date）忽略
            let retry_after = r
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok());
            let detail = r.text().unwrap_or_default();
            if status == 400 && thinking_related_error(&detail) {
                if let Some(next) = plan.next() {
                    eprintln!("[agent] 端点不接受 thinking={plan:?} → 降级为 {next:?}");
                    cfg.thinking.set(next);
                    continue;
                }
            }
            // 上下文超限 → 强制压缩后再试一次（水位估算失准时靠这条兜底）
            if status == 400 && !compacted_for_retry && context_related_error(&detail) {
                compacted_for_retry = true;
                let out = compact_history(history, Compact::Force, cfg.last_input.get());
                base = base.saturating_sub(out.dropped);
                cfg.last_compact.set(cfg.last_input.get());
                cfg.last_input.set(0);
                eprintln!("[agent] 端点回报上下文超限 → 压缩 {} 条后重试", out.dropped);
                continue;
            }
            // 429 / 5xx：端点侧瞬时故障 → 退避重试（尊重 Retry-After）
            if retryable_status(status.as_u16()) && transient_attempt < MAX_API_RETRIES {
                transient_attempt += 1;
                let delay = retry_delay_ms(transient_attempt, retry_after);
                let brief: String = detail.trim().chars().take(200).collect();
                emit_api_retry(transient_attempt, Some(status.as_u16()), delay, &brief);
                thread::sleep(Duration::from_millis(delay));
                continue;
            }
            let detail: String = detail.trim().chars().take(800).collect();
            return finish_error(history, base, &format!("HTTP {status}: {detail}"), started, turns);
        };

        // ── 流式解析：把每个 content block 收齐 ──────────────────
        let mut blocks: Vec<Block> = Vec::new();
        let mut api_error: Option<String> = None;

        // SSE：只关心 `data:` 载荷；`event:`/空行/注释行一律跳过
        let reader = BufReader::new(resp);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    // 读流出错（连接被掐断 / 超时）。这里**不能**退避重试 —— 前面已经
                    // 把部分内容流给前端了，重来会产生重复文本。至少留一条可查的日志。
                    log::warn(format!("SSE 流中断，本轮回复可能不完整: {e}"));
                    break;
                }
            };
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(ev) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            let idx = || ev.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;

            match ev.get("type").and_then(Value::as_str).unwrap_or("") {
                "message_start" => {
                    let u = &ev["message"]["usage"];
                    let req_in = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                    let req_read = u
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let req_create = u
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    // 真实上下文体积 = 三类输入之和（input_tokens 不含缓存那两项），
                    // 水位检查用这个值比按字符估算准得多。
                    cfg.last_input.set(req_in + req_read + req_create);
                    in_tokens += req_in;
                    cache_read += req_read;
                    cache_create += req_create;
                    // 本次请求的明细（message_stop 时整条推入 req_log）
                    cur_in = req_in;
                    cur_read = req_read;
                    cur_create = req_create;
                    cur_out = 0;
                }
                "content_block_start" => {
                    let cb = &ev["content_block"];
                    let ty = cb
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("text")
                        .to_string();
                    let i = idx();
                    if blocks.len() <= i {
                        blocks.resize_with(i + 1, Block::default);
                    }
                    let mut blk = Block {
                        kind: ty.clone(),
                        ..Block::default()
                    };
                    match ty.as_str() {
                        "thinking" => emit_stream_event(json!({
                            "type": "content_block_start",
                            "index": i,
                            "content_block": { "type": "thinking" },
                        })),
                        "tool_use" => {
                            blk.id = cb
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            blk.name = cb
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            blk.input = cb.get("input").cloned().unwrap_or(Value::Null);
                            emit_stream_event(json!({
                                "type": "content_block_start",
                                "index": i,
                                "content_block": {
                                    "type": "tool_use",
                                    "id": blk.id,
                                    "name": blk.name,
                                },
                            }));
                        }
                        _ => emit_stream_event(json!({
                            "type": "content_block_start",
                            "index": i,
                            "content_block": { "type": "text", "text": "" },
                        })),
                    }
                    blocks[i] = blk;
                }
                "content_block_delta" => {
                    let delta = &ev["delta"];
                    let i = idx();
                    if blocks.len() <= i {
                        blocks.resize_with(i + 1, Block::default);
                    }
                    match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                        "text_delta" => {
                            let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
                            if blocks[i].kind.is_empty() {
                                blocks[i].kind = "text".into();
                            }
                            blocks[i].text.push_str(text);
                            emit_stream_event(json!({
                                "type": "content_block_delta",
                                "index": i,
                                "delta": { "type": "text_delta", "text": text },
                            }));
                        }
                        "thinking_delta" => {
                            let thinking = delta
                                .get("thinking")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            if blocks[i].kind.is_empty() {
                                blocks[i].kind = "thinking".into();
                            }
                            blocks[i].text.push_str(thinking);
                            emit_stream_event(json!({
                                "type": "content_block_delta",
                                "index": i,
                                "delta": { "type": "thinking_delta", "thinking": thinking },
                            }));
                        }
                        "input_json_delta" => {
                            let partial = delta
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            if blocks[i].kind.is_empty() {
                                blocks[i].kind = "tool_use".into();
                            }
                            blocks[i].json.push_str(partial);
                            emit_stream_event(json!({
                                "type": "content_block_delta",
                                "index": i,
                                "delta": { "type": "input_json_delta", "partial_json": partial },
                            }));
                        }
                        _ => {}
                    }
                }
                "content_block_stop" => {
                    emit_stream_event(json!({ "type": "content_block_stop", "index": idx() }));
                }
                "message_delta" => {
                    if let Some(n) = ev["usage"].get("output_tokens").and_then(Value::as_u64) {
                        out_tokens += n;
                        // Anthropic 的 message_delta.output_tokens 是**本条消息的累计值**，
                        // 所以这里是赋值不是累加（同一条消息可能来多次 delta）。
                        cur_out = n;
                    }
                }
                "message_stop" => {
                    // 一次 API 请求 = 一条对账明细（与平台用量页逐行对齐）
                    req_log.push(json!({
                        "in": cur_in,
                        "read": cur_read,
                        "create": cur_create,
                        "out": cur_out,
                    }));
                    emit_stream_event(json!({ "type": "message_stop" }));
                }
                "error" => {
                    api_error = Some(
                        ev.pointer("/error/message")
                            .and_then(Value::as_str)
                            .unwrap_or("未知流错误")
                            .to_string(),
                    );
                }
                _ => {}
            }
        }

        if let Some(err) = api_error {
            return finish_error(history, base, &err, started, turns);
        }

        // ── 组装本轮块 ──────────────────────────────────────────
        // display = 发给前端展示（含 thinking）；hist = 写回上下文
        // （不含 thinking —— 端点要求 thinking 块带 signature，回灌会 400）
        let mut display: Vec<Value> = Vec::new();
        let mut hist: Vec<Value> = Vec::new();
        let mut calls: Vec<(String, String, Value)> = Vec::new();

        for b in &blocks {
            match b.kind.as_str() {
                "thinking" if !b.text.is_empty() => {
                    display.push(json!({ "type": "thinking", "thinking": b.text }));
                }
                "text" if !b.text.is_empty() => {
                    final_text = b.text.clone();
                    let blk = json!({ "type": "text", "text": b.text });
                    display.push(blk.clone());
                    hist.push(blk);
                }
                "tool_use" => {
                    // 优先用 input_json_delta 累积出来的完整 JSON —— 兼容端点
                    // （如 DeepSeek 的 Anthropic 层）会在 content_block_start
                    // 里先塞一个占位 `"input": {}`，只看它会拿到空参数。
                    let input = if !b.json.trim().is_empty() {
                        match serde_json::from_str::<Value>(&b.json) {
                            Ok(v) if v.is_object() => v,
                            Ok(_) => json!({}),
                            Err(e) => {
                                eprintln!("[agent] tool_use input 不是合法 JSON（{e}）");
                                json!({})
                            }
                        }
                    } else if b.input.is_object() {
                        b.input.clone()
                    } else {
                        json!({})
                    };
                    let blk = json!({
                        "type": "tool_use",
                        "id": b.id,
                        "name": b.name,
                        "input": input,
                    });
                    display.push(blk.clone());
                    hist.push(blk);
                    calls.push((b.id.clone(), b.name.clone(), input));
                }
                _ => {}
            }
        }

        // assistant 整包：前端在没有任何增量时拿它兜底，同时作为后续轮次的上下文
        emit(json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": display },
        }));
        if !hist.is_empty() {
            history.push(json!({ "role": "assistant", "content": hist }));
        }

        // 没有工具调用 ⇒ 本轮结束
        if calls.is_empty() {
            break;
        }

        // ── 执行工具 → 回灌 tool_result ─────────────────────────
        // 写类工具先请用户审批（P2）：一批先全部发出，前端才能把连续 Bash
        // 合并成一行（findLastBashGroup）一次决定；随后按顺序阻塞等回包。
        // 工具报错不中断整轮：转成 is_error=true 的 tool_result，模型可自行纠正。
        //
        // plan（只读）档的豁免只对写类工具有效 —— 它们会被 tools::run 直接拒绝，
        // 问了也是白问。放行的那三件（WebSearch / WebFetch / AskUserQuestion）
        // 必须照问：搜索查询词与抓取的目标 URL 都是外部出口，后者的答案只能
        // 从卡片上取（见 gated_in_read_only）。
        let mut pendings: Vec<Option<Pending>> = Vec::with_capacity(calls.len());
        for (id, name, input) in &calls {
            let ask = ask_permission
                && needs_approval(name)
                && (!tctx.read_only || tools::gated_in_read_only(name));
            if ask {
                pendings.push(Some(open_approval(name, id, input)));
            } else {
                pendings.push(None);
            }
        }

        // 先把审批**按原顺序**全部解完（审批是阻塞等用户，且顺序不能乱：前端按
        // 「未应答行」合并同一批命令，乱序会打乱卡片与合并结果）。
        let mut interrupted = false;
        let mut run_inputs: Vec<Value> = Vec::with_capacity(calls.len());
        let mut denieds: Vec<Option<String>> = Vec::with_capacity(calls.len());
        for ((_id, _name, input), pending) in calls.iter().zip(pendings.into_iter()) {
            let mut run_input = input.clone();
            let mut denied: Option<String> = None;
            if let Some(p) = pending {
                match await_approval(p, input) {
                    Decision::Allow(approved) => run_input = approved,
                    Decision::Deny(msg, stop) => {
                        interrupted |= stop;
                        denied = Some(msg);
                    }
                }
            }
            run_inputs.push(run_input);
            denieds.push(denied);
        }

        // 按批执行：**连续的**只读调用并行（上限 TOOL_PARALLELISM），其余串行。
        // 结果一律按下标回填 ⇒ 回灌顺序恒等于 tool_use 的原顺序。
        let mut slots: Vec<Option<(String, bool)>> = vec![None; calls.len()];
        for (parallel, range) in plan_tool_batches(&calls) {
            if !parallel {
                let i = range.start;
                slots[i] = Some(run_one_tool(
                    tctx,
                    mcp_bridge.as_deref_mut(),
                    skill_list,
                    &calls[i].1,
                    &run_inputs[i],
                    denieds[i].as_deref(),
                ));
                continue;
            }
            // 只读批里只可能出现内置工具（白名单见 tools::parallel_safe）⇒ 不需要
            // MCP 桥，也就绕开了 `&mut Bridge` 无法跨线程共享的问题。
            // 并发上限靠**分块 + 块内 join** 实现，不引信号量。
            // 先把三个只读切片取成 `&`（引用是 Copy，`move` 闭包拷进去的是引用
            // 本身，不会把 `calls` 整个移走 —— 后面还要用它组装回灌结果）。
            let (calls_ref, inputs_ref, denieds_ref) = (&calls, &run_inputs, &denieds);
            log::info(format!(
                "只读工具并行批 {} 条（并发上限 {TOOL_PARALLELISM}）: {}",
                range.len(),
                range
                    .clone()
                    .map(|i| calls[i].1.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            let mut start = range.start;
            while start < range.end {
                let end = (start + TOOL_PARALLELISM).min(range.end);
                let done: Vec<(usize, String, bool)> = thread::scope(|s| {
                    let handles: Vec<_> = (start..end)
                        .map(|i| {
                            (
                                i,
                                s.spawn(move || {
                                    let (text, is_error) = run_one_tool(
                                        tctx,
                                        None,
                                        skill_list,
                                        &calls_ref[i].1,
                                        &inputs_ref[i],
                                        denieds_ref[i].as_deref(),
                                    );
                                    (i, text, is_error)
                                }),
                            )
                        })
                        .collect();
                    handles
                        .into_iter()
                        .map(|(i, h)| {
                            h.join()
                                .unwrap_or_else(|_| (i, "Error: tool worker panicked".into(), true))
                        })
                        .collect()
                });
                for (i, text, is_error) in done {
                    slots[i] = Some((text, is_error));
                }
                start = end;
            }
        }

        let results: Vec<Value> = calls
            .iter()
            .zip(slots)
            .map(|((id, _name, _input), slot)| {
                let (text, is_error) =
                    slot.unwrap_or_else(|| ("Error: tool was not executed".into(), true));
                tool_result_block(id, text, is_error)
            })
            .collect();
        let tool_msg = json!({ "role": "user", "content": results });
        emit(json!({ "type": "user", "message": tool_msg.clone() }));
        history.push(tool_msg);

        // 用户点了「拒绝并中断」→ 结果已回灌，本轮到此为止
        if interrupted {
            eprintln!("[agent] 用户要求中断本轮");
            break;
        }

        rounds += 1;
        if rounds >= MAX_TOOL_ROUNDS {
            if hint_sent {
                eprintln!("[agent] 工具轮次达上限，提前收尾");
                break;
            }
            // 给模型一次「收口」的机会：只出文本，不再调工具
            hint_sent = true;
            history.push(json!({
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "Tool budget exhausted. Stop calling tools and give the user your final answer now.",
                }],
            }));
        }
    }

    emit(json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "duration_ms": started.elapsed().as_millis() as u64,
        "num_turns": turns,
        "result": final_text,
        "session_id": "",
        "total_cost_usd": 0.0,
        "usage": {
            "input_tokens": in_tokens,
            "output_tokens": out_tokens,
            "cache_read_input_tokens": cache_read,
            "cache_creation_input_tokens": cache_create,
            // 每次 API 请求一行（顺序 = 请求顺序）。前端写进本地用量日志，
            // 与 DeepSeek 平台用量页按请求对账；旧消费者忽略即可。
            "requests": req_log,
        },
    }));
}

/// 出错时收尾：把本轮压入的所有消息（user / assistant / tool_result）
/// 全部丢弃，避免半截对话污染后续上下文。
fn finish_error(
    history: &mut Vec<Value>,
    base: usize,
    msg: &str,
    started: Instant,
    turns: usize,
) {
    eprintln!("[agent] {msg}");
    history.truncate(base);
    emit(json!({
        "type": "result",
        "subtype": "error_during_execution",
        "is_error": true,
        "duration_ms": started.elapsed().as_millis() as u64,
        "num_turns": turns,
        "result": msg,
        "session_id": "",
        "total_cost_usd": 0.0,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 系统提示词的**前缀缓存不变量**（2026-09-17 方案 B 的守门测试）。
    ///
    /// 方案 B 把「人格 + 文风」两块从**用户消息**（每问重发、必未命中）搬进了**系统提示词**
    /// （固定前缀的一部分、永远命中）。搬错了地方 —— 比如塞进任何按 query 拼的字符串 ——
    /// 就会让系统提示词每轮都变，把整个固定前缀的缓存打掉，比原来更糟。所以这里钉两件事：
    /// ① 同一 cwd 下逐字节可复现；② 两块文案真的在里面。
    #[test]
    fn system_prompt_is_stable_and_carries_persona() {
        let build = || {
            format!(
                "{SYSTEM_PROMPT}\n\n{PERSONA_AND_STYLE}{}",
                env_block(std::path::Path::new("C:/work"))
            )
        };
        let a = build();
        assert_eq!(a, build(), "同一 cwd 下系统提示词必须逐字节相同（否则前缀缓存每轮作废）");
        assert!(a.contains("## Personality (fixed — always apply)"), "人格块丢了");
        assert!(a.contains("## Output Style"), "文风块丢了");
        assert!(!a.contains('{'), "残留了未被 format! 替换的占位符");
    }

    /// 回灌历史的三条规则各测一次。重点在**合并连续同角色**：回退到一条用户消息后
    /// 紧接着的新提问会构成两条连续 `user`，不合并就是端点 400（这就是本次要修的场景）。
    #[test]
    fn normalize_history_merges_and_filters() {
        let msgs = vec![
            json!({ "role": "user", "content": "Q1" }),
            json!({ "role": "assistant", "content": "A1" }),
            // 回退到 Q2 后紧接着追问 → 连续两条 user，必须合并成一条
            json!({ "role": "user", "content": "Q2" }),
            json!({ "role": "user", "content": "Q2 追问" }),
            // 认不出的角色 / 空文本都要丢掉
            json!({ "role": "system", "content": "ignored" }),
            json!({ "role": "assistant", "content": "   " }),
        ];
        let out = normalize_history(msgs);
        assert_eq!(out.len(), 3, "user/assistant 交替 + 两条 user 合并");

        // 合并后仍是交替的 role 序列（端点的硬要求）
        let roles: Vec<&str> = out
            .iter()
            .map(|m| m.get("role").and_then(Value::as_str).unwrap())
            .collect();
        assert_eq!(roles, vec!["user", "assistant", "user"]);

        // 被合并的那条两段文本都在，且都是合法 text block
        let blocks = out[2].get("content").and_then(Value::as_array).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].get("text").and_then(Value::as_str), Some("Q2"));
        assert_eq!(blocks[1].get("text").and_then(Value::as_str), Some("Q2 追问"));
        assert_eq!(
            blocks[0].get("type").and_then(Value::as_str),
            Some("text"),
            "必须是 type+text 的 block 形状，不能把裸字符串塞进 content 数组"
        );
    }

    /// 造一段历史：开头是用户提问，中间塞一条大 tool_result，尾部留 COMPACT_KEEP_TAIL 条。
    fn history_with_big_tool_result(chars: usize) -> (Vec<Value>, Value) {
        let big = json!("x".repeat(chars));
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"hi"}]}),
            json!({"role":"user","content":[{"type":"tool_result","content":big.clone()}]}),
        ];
        for i in 0..COMPACT_KEEP_TAIL {
            history.push(json!({"role":"user","content":[{"type":"text","text":format!("t{i}")}]}));
        }
        (history, big)
    }

    /// 瘦身档的「值不值得」闸门：省得不够多就不许动历史（动了会让其后整段前缀失效）
    #[test]
    fn elide_is_skipped_when_savings_are_too_small() {
        let (mut history, big) = history_with_big_tool_result(3_000);
        // 上下文 10 万 token → 阈值 = 100000 × 0.05 × 4 = 20000 字 ≫ 可省 3000 字
        let out = compact_history(&mut history, Compact::Elide, 100_000);
        assert_eq!(out.elided, 0, "省得不够多时不该瘦身");
        assert_eq!(history[1]["content"][0]["content"], big, "历史必须原样保留");

        // 上下文小到阈值（1000 token → 200 字）以下 → 允许瘦身
        let out = compact_history(&mut history, Compact::Elide, 1_000);
        assert_eq!(out.elided, 1);
        assert_ne!(history[1]["content"][0]["content"], big);
    }

    /// 丢弃档 / 400 兜底档是安全刚需 —— 不受上面那道闸门约束
    #[test]
    fn drop_mode_ignores_the_savings_gate() {
        let (mut history, big) = history_with_big_tool_result(3_000);
        let out = compact_history(&mut history, Compact::Drop, 100_000);
        assert_eq!(out.elided, 1);
        assert_eq!(out.dropped, 0, "砍得动 tool_result 时不必丢整条消息");
        assert_ne!(history[1]["content"][0]["content"], big);

        let (mut history, _) = history_with_big_tool_result(3_000);
        let out = compact_history(&mut history, Compact::Force, 100_000);
        assert_eq!(out.elided, 1);
    }

    /// backlog §8.3：丢弃历史时，「当前任务清单」必须活下来（否则模型会跑偏）。
    #[test]
    fn task_snapshot_survives_a_drop() {
        // 历史：用户提问（head，丢弃逻辑刻意保留它）→ TodoWrite → 它的 tool_result → 一堆后续消息。
        // 后续消息要够多，保证 drop 区间 [head, cut) 覆盖到 TodoWrite 那两条。
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"帮我改脚本"}]}),
            json!({"role":"assistant","content":[{
                "type":"tool_use","id":"t1","name":"TodoWrite",
                "input":{"todos":[
                    {"content":"改 build-release.ps1","status":"completed","activeForm":"改脚本"},
                    {"content":"继续 docs 未完成任务","status":"in_progress","activeForm":"做任务"}
                ]}
            }]}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}),
        ];
        for i in 0..COMPACT_KEEP_TAIL * 2 {
            history.push(json!({"role":"assistant","content":[{"type":"text","text":format!("m{i}")}]}));
        }

        let out = compact_history(&mut history, Compact::Force, 0);
        assert!(out.dropped > 0, "Force 档必须真的丢了消息，否则这个用例没测到东西");
        assert!(
            !history.iter().any(has_todo_write),
            "原始 TodoWrite 消息应当已被丢弃 —— 快照是唯一的幸存者"
        );

        let snap = history
            .iter()
            .find(|m| {
                m["content"][0]["text"]
                    .as_str()
                    .map(|t| t.contains(TASK_SNAPSHOT_HEADER))
                    .unwrap_or(false)
            })
            .expect("应当把任务快照钉回历史");
        assert_eq!(snap["role"].as_str(), Some("user"), "快照必须是 user 文本消息（配对结构无关）");
        let text = snap["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("继续 docs 未完成任务"), "快照要带上清单内容：{text}");
        assert!(text.contains("[in_progress]"), "快照要保留状态：{text}");
        assert!(text.contains("[completed]"), "快照要保留状态：{text}");

        // 位置：插在第一条之后，第一条仍是那次提问（head 的语义不能被抢走）
        assert_eq!(history[0]["content"][0]["text"].as_str(), Some("帮我改脚本"));
    }

    /// 没丢东西时**不插**快照 —— 原清单还在历史里，插了就是重复。
    #[test]
    fn task_snapshot_is_not_pinned_when_nothing_is_dropped() {
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"hi"}]}),
            json!({"role":"assistant","content":[{
                "type":"tool_use","id":"t1","name":"TodoWrite",
                "input":{"todos":[{"content":"a","status":"pending","activeForm":"a"}]}
            }]}),
        ];
        let out = compact_history(&mut history, Compact::Elide, 0);
        assert_eq!(out.dropped, 0);
        assert!(
            !history.iter().any(|m| m["content"][0]["text"]
                .as_str()
                .map(|t| t.contains(TASK_SNAPSHOT_HEADER))
                .unwrap_or(false)),
            "没丢消息时不该插快照"
        );
    }

    /// 取「最近一条」清单；空清单 / 空历史 / 结构不对都返回 None。
    #[test]
    fn task_snapshot_takes_the_latest_list() {
        let older = json!({"role":"assistant","content":[{
            "type":"tool_use","id":"a","name":"TodoWrite",
            "input":{"todos":[{"content":"旧的","status":"pending","activeForm":"旧的"}]}
        }]});
        let newer = json!({"role":"assistant","content":[{
            "type":"tool_use","id":"b","name":"TodoWrite",
            "input":{"todos":[{"content":"新的","status":"in_progress","activeForm":"新的"}]}
        }]});
        let snap = latest_todo_snapshot(&[older, newer]).expect("应当取到快照");
        assert!(snap.contains("新的") && !snap.contains("旧的"), "必须取最近一条：{snap}");

        assert!(latest_todo_snapshot(&[]).is_none(), "空历史没有快照");
        assert!(
            latest_todo_snapshot(&[json!({"role":"assistant","content":[{
                "type":"tool_use","id":"c","name":"TodoWrite","input":{"todos":[]}
            }]})])
            .is_none(),
            "空清单不该产出快照"
        );
    }

    #[test]
    fn retries_only_transient_statuses() {
        // 限流与服务端故障才重试（529 是部分兼容端点表示「过载」的写法）
        for code in [429u16, 500, 502, 503, 504, 529, 599] {
            assert!(retryable_status(code), "{code} 应当重试");
        }
        // 4xx（除 429）重试也不会变：鉴权/参数错、以及专门分支处理的 400
        for code in [400u16, 401, 403, 404, 422, 499] {
            assert!(!retryable_status(code), "{code} 不该重试");
        }
    }

    #[test]
    fn backoff_grows_and_honours_retry_after() {
        // 指数增长 + 抖动（0..250ms）
        let first = retry_delay_ms(1, None);
        let second = retry_delay_ms(2, None);
        assert!((1_000..1_250).contains(&first), "首次退避应≈1s，实得 {first}");
        assert!((2_000..2_250).contains(&second), "第二次应≈2s，实得 {second}");

        // 端点给的 Retry-After 优先，且被上限夹住
        assert_eq!(retry_delay_ms(1, Some(7)), 7_000);
        assert_eq!(retry_delay_ms(1, Some(9_999)), RETRY_MAX_MS);

        // 很大的 attempt 不会溢出、也不会超过上限
        assert_eq!(retry_delay_ms(64, None), RETRY_MAX_MS);
    }

    /// 思考只有开/关两态：只有明确的「关」才是关，认不出来的值一律当开
    #[test]
    fn thinking_is_a_two_state_switch() {
        for off in ["off", " 0 ", "false"] {
            assert_eq!(thinking_from(Some(off)), Thinking::Disabled, "{off} 应为关");
        }
        for on in ["on", "", "8192", "whatever"] {
            assert_eq!(
                thinking_from(Some(on)),
                Thinking::Budget(THINKING_BUDGET),
                "{on:?} 应为开"
            );
        }
        // 注入漏了（env 不存在）→ 开，与前端默认档一致
        assert_eq!(thinking_from(None), Thinking::Budget(THINKING_BUDGET));

        // 开档的 budget 必须严格小于它抬起来的 max_tokens，否则端点判参数非法
        assert!(THINKING_BUDGET < max_tokens_for(Thinking::Budget(THINKING_BUDGET)));

        // 关档的降级终点是「不发字段」，绝不退到 adaptive（那等于反手把思考打开）
        assert_eq!(Thinking::Disabled.next(), Some(Thinking::Omit));
        assert_eq!(Thinking::Disabled.to_json(), Some(json!({ "type": "disabled" })));
    }

    fn call(name: &str) -> (String, String, Value) {
        ("id".into(), name.into(), json!({}))
    }

    /// 批次切分：区间无缝覆盖全部调用，且**只读批绝不跨越写类调用**
    #[test]
    fn batches_never_span_a_writing_call() {
        let calls = vec![
            call("Read"),
            call("Grep"),
            call("Write"), // 写类：必须打断只读批
            call("Read"),
            call("Glob"),
        ];
        let batches = plan_tool_batches(&calls);

        // 区间必须无缝覆盖 [0, len)
        assert_eq!(batches[0].1.start, 0);
        assert_eq!(batches.last().unwrap().1.end, calls.len());
        for w in batches.windows(2) {
            assert_eq!(w[0].1.end, w[1].1.start, "相邻批之间不许有空隙/重叠");
        }

        assert_eq!(batches.len(), 3);
        assert!(batches[0].0, "Read+Grep 连续只读 → 并行批");
        assert_eq!(batches[0].1, 0..2);
        assert!(!batches[1].0, "Write 必须独占一批");
        assert_eq!(batches[1].1, 2..3);
        assert!(batches[2].0);
        assert_eq!(batches[2].1, 3..5);
    }

    /// 单元素的只读段不标并行（省一次线程 spawn，行为与串行完全一致）
    #[test]
    fn single_read_only_call_is_not_marked_parallel() {
        for calls in [
            vec![call("Read")],
            vec![call("Bash"), call("Read")],
            vec![call("Read"), call("Bash")],
        ] {
            for (parallel, _) in plan_tool_batches(&calls) {
                assert!(!parallel, "批内只有一个只读调用时不该标并行");
            }
        }
        assert!(plan_tool_batches(&[]).is_empty());
    }
}
