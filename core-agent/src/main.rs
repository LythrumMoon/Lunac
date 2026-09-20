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
//            content 里可再加图片块（A8）：{"type":"image","source":{"type":"file",
//             "path":"C:\\…\\a.png"}} —— 只传路径，字节由本进程读出来转 base64
//             （media_type 由魔术字节判定，发送方不必给）
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
//      以 `mcp__<名>` 接进请求体（实现见 mcp.rs）；resources 读侧两件条件注册（A3）
//   ✅ P4 技能：`LUNAC_SKILLS_DIR`（=<exe 根>\skills）下的 <key>/SKILL.md，
//      系统提示词列出清单，模型调 Skill 工具取正文（实现见 skills.rs）
//   ✅ 长期记忆 + 每 N 轮的后台复盘 fork（A4，2026-09-20）：`Remember` 工具 + 启动时
//      冻结快照注入，见本文件「长期记忆」与「后台复盘 fork」两节
//   ✅ 技能 fork / 自带资源（A5，2026-09-20）：`context: fork` 的技能派子代理执行，
//      技能目录内的脚本与资源随 Skill 返回附上（见 skills.rs）；remote 模式不移植
//   ✅ 图片附件（A8，2026-09-20）：user 消息里的 `image` 块按路径读字节转 base64 块，
//      见本文件「图片附件」一节；开关与来源在宿主 / 前端，PDF 仍走路径文本
//
// 本文件为 Lunac 自研实现，不派生自任何第三方源码。

use std::cell::Cell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};

mod bash_safety;
mod content_safety;
mod hooks;
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
/// 工具轮次打满时的「收口」指令。
///
/// **落点必须让 history 的形状与块数量都不变** —— 即拼进上一条 `tool_result` 的
/// **文本内部**，不得单独 `push` 一条新的 `user` 消息、也不得在消息的 content 数组里
/// 追加 `text` 块（2026-09-20 单变量实测，不得回退）：
///
/// | history 形态（第 17 轮，末条） | 条数 | 第 17 轮 `read` | 整次提问命中率 |
/// |---|---|---|---|
/// | `user(tool_result)`，无 hint（对照组） | 33 | **11776** | **91.7%** |
/// | `user(tool_result)` + 新 `user(hint)`（原实现） | 34 | 2560 | 87.2% |
/// | `user(tool_result, text)`（追加块） | 33 | 2560 | 85.9% |
///
/// 所以**不是**「消息条数 +1」的问题，而是「末条消息多出一个内容块」：只要动到
/// 前缀末尾的块结构，端点侧的前缀缓存单元就整体失配，`read` 掉到只剩
/// `system + tools`（≈2560 tokens，即「公共前缀检测」落盘的那个单元）。
/// 纯文本多轮探针（无 `tool_use`/`tool_result`、无 `tools` 参数）里追加同样的双
/// `user` 消息**不会**崩塌，所以这是它与工具消息形态的交互。
const TOOL_BUDGET_HINT: &str =
    "Tool budget exhausted. Stop calling tools and give the user your final answer now.";

// ── 子代理（A1：`Agent` 工具）────────────────────────────────────
//
// 模型调 `Agent` ⇒ 新开一个**空白消息数组**跑独立的工具循环，**只把最终文本**
// 回灌成 `tool_result`。中间那些 Read / Grep 的原始输出留在子代理自己的上下文里，
// 不进主对话 —— 这正是它省上下文的地方。
//
// 五条硬约束（backlog A1，不得放宽）：
//   ① **独立上下文**：子代理用自己的 `history`，与主对话零共享；
//   ② **结果可归因**：回灌文本与事件都带 `task_id`。它仍是**进程内存态**计数器
//      （`TASK_SEQ`）—— 因为它会出现在给模型看的回灌文本里，短才好读；跨进程的唯一性
//      由 `session_id()`（A11 起是真值，见下）在 `system/init` / `result` 与启动日志里提供；
//   ③ **并发 = 花钱**：三重闸门 —— 串行执行（`tools::parallel_safe("Agent")` 为
//      false）+ 轮次上限 + token 预算封顶；
//   ④ **进度回前端**：`task_started` / `task_progress` / `task_done`；
//   ⑤ **防递归**：子代理的工具集里**剔掉 `Agent`** —— 否则一个任务能派无限层子代理。

/// 子代理自己的工具往返上限。比主对话的 `MAX_TOOL_ROUNDS`(16) 小：子代理是
/// 「查清一件事」而不是「做完整个任务」，8 轮足够，也把失控成本压住。
const MAX_SUBAGENT_ROUNDS: usize = 8;
/// 单个子代理的 token 预算封顶（输入 + 输出 + 缓存命中的**全部吞吐**）。
/// 超了就把**已有结论**交回去，而不是继续烧钱 —— 见约束③。
const SUBAGENT_BUDGET_TOKENS: u64 = 300_000;
/// 回灌给主对话的子代理报告字符上限。子代理的返回值**不走** `tools::apply_budget`
/// （那条路只覆盖内置工具），不在这里收口的话，一份长报告能直接把主对话上下文顶爆。
const SUBAGENT_REPORT_CHARS: usize = 8_000;
/// 子代理的系统提示词（**角色段**）。英文的原因同 `TRIMMED_MARKER`：这是给模型看的元信息，不进 i18n。
///
/// 它在 `main()` 里被拼成完整提示词时**还会接上环境块与技能清单**（见那里的 `subagent_system`）
/// —— 光给这一段的子代理不知道自己的工作目录在哪，也不知道有哪些技能可用（2026-09-20 复查补）。
const SUBAGENT_SYSTEM: &str = "\
You are a subagent: a fresh, isolated assistant instance launched by another assistant \
to carry out ONE self-contained task with your own tools.

Rules:
- You do NOT share the caller's conversation. Everything you need is in the task below; \
if something is missing, state the assumption you made instead of asking.
- Work until the task is done, then reply with your FINAL REPORT only. The caller sees \
nothing but that report -- no tool output, no intermediate reasoning.
- The report must be self-contained and concrete: answer the question, cite the exact \
file paths / line ranges / commands you relied on, and flag anything you could not verify.
- Do not ask the user questions; you have no interactive channel.";
/// 进程内存态的 task 序号 —— 它要出现在给模型看的回灌文本里，短才好读；
/// 跨进程的唯一性由 `session_id()` 补上（约束②）
static TASK_SEQ: AtomicU64 = AtomicU64::new(0);

// ── 后台复盘 fork（A4：长期记忆的**触发点**）──────────────────────
//
// 按**轮次门槛**触发：每完成 `nudge_interval`（默认 10，Hermes 的 `nudge_interval`
// 默认值）次用户提问，就派一个后台 fork 去复盘这段对话，把值得留的结论用 `Remember`
// 写进长期记忆。
//
// **不引定时器**（backlog 硬约束）：按轮次门槛触发。定时器会在用户什么都没干的时候空转
// （每次都真花钱），而「跑不跑」与「这段时间有没有值得留的东西」完全无关。
//
// 「后台」的确切含义：**在提问之间**跑。`run_query` 返回（答案已交付前端）之后才 spawn，
// 下一次提问进来之前 join —— 所以它**不占任何一次回答的时延**，用户读答案/打字的那几秒
// 正好把复盘跑完。它不是「随时并发」：agent 的 stdout 契约是逐行 JSON，两条线程同时
// `emit` 会交错（见 run_review_fork 的注释）。
//
// 为什么值得：取回侧（`SessionSearch` + 往期会话索引）2026-09-19 就通了，但**一直只有
// 取回、没有写回** —— 用户每次都要重新交代一遍相同的偏好与约定。

/// 轮次门槛的开关/取值：`0` 关闭，其余为间隔（提问数）。默认 10。
const NUDGE_INTERVAL_ENV: &str = "LUNAC_NUDGE_INTERVAL";
const DEFAULT_NUDGE_INTERVAL: u64 = 10;

/// 复盘 fork 的轮次上限与 token 预算。比子代理（8 轮 / 30 万）更小：它只做
/// 「判断 + 写几条」，正常一两轮就结束；上限只是防它失控烧钱。
const MAX_REVIEW_ROUNDS: usize = 4;
const REVIEW_BUDGET_TOKENS: u64 = 120_000;
/// 送进复盘的对话快照上限（字符）。复盘看的是**结论**，不需要逐字原文 ——
/// 快照太大只会让这次后台调用变贵。
const REVIEW_TRANSCRIPT_CHARS: usize = 6_000;
/// 一次复盘最多写几条记忆（写在提示词里）。每 10 轮跑一次，贪多必然灌进一堆琐事。
const REVIEW_MAX_ITEMS: usize = 3;

/// 复盘 fork 的**白名单工具集**（backlog A4 原文：「只放白名单工具（写记忆 / 改技能）」）。
///
/// 逐件的理由：
///   · `Remember` / `Write` / `Edit` —— 写记忆与改技能，是它被派出来的**唯一目的**；
///   · `Read` / `Glob` / `Grep` —— 要核对「这条结论到底对不对」时得能查证，
///     否则它只能凭快照猜，而猜错的东西会一直留在记忆里；
///   · `Skill` —— 读技能正文（改技能前至少先看看现在写的是什么）。
/// **不在名单里的一律拿不到**（`Bash` / `PowerShell` / `WebFetch` / `Agent` / MCP 工具…）：
/// 这是一个**无人值守**的后台进程，工具面必须最小。`Agent` 不在名单里也就顺带防了递归。
const REVIEW_TOOL_WHITELIST: [&str; 7] =
    ["Read", "Glob", "Grep", "Write", "Edit", "Skill", "Remember"];

/// 复盘 fork 的系统提示词（**角色段**）。与 `SUBAGENT_SYSTEM` 一样是给模型看的元信息，
/// 不进 i18n。它在 `main()` 里拼一次（角色段 + 环境块 + 技能清单），所有复盘共享同一段前缀。
const REVIEW_SYSTEM: &str = "\
You are Lunac's background reviewer. A conversation has just finished and you are deciding what, \
if anything, deserves to be kept in the user's LONG-TERM MEMORY.

Long-term memory means: durable facts that stay true across conversations — the user's stated \
preferences and working style, project conventions, how their environment is set up, decisions \
that were reached and the reason behind them. It is loaded into the assistant's context at the \
start of every future session.

Do NOT write: one-off details of the finished task, progress or to-do state, anything already \
present in the memory shown below, or anything you are merely guessing. A wrong memory is worse \
than a missing one, because it silently misleads every future session.

Rules:
- Judge first, act second. Most conversations contain NOTHING worth remembering — that is the \
normal outcome.
- If nothing qualifies, reply with exactly `NOTHING_TO_REMEMBER` and nothing else. Do not call \
any tool in that case.
- Otherwise save at most a few facts, one per `Remember` call, each a concise self-contained \
sentence in the user's own language. No paths or IDs that will be meaningless later.
- You may use Read / Glob / Grep to verify a fact before saving it. You may fix one of the \
user's skills under the skills directory with Edit / Write, but ONLY when the user's own words \
in the conversation clearly established that it is wrong or should be improved; never \
speculatively.
- Reply with one short line summarising what you saved (or `NOTHING_TO_REMEMBER`). Nobody reads \
this reply except the log.";

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

// ── 摘要式压缩（backlog §8.2）───────────────────────────────────────
//
// 与上面「机械压缩」（瘦身 / 丢弃）的关系：机械压缩**不额外调模型**，是默认且唯一
// 无条件执行的那条路。摘要压缩是它的**可选补强** —— 当丢弃档真的把一大段历史扔了，
// 与其只留一句 `TRIMMED_MARKER`，不如花**一次** API 调用把它压成摘要留下来。
//
// 三条硬约束（backlog §8.2 的「Lunac 硬约束」，不得放宽）：
//   ① 真实花钱与耗时 ⇒ 只在**丢弃档 / 400 兜底档**触发（0.85 的瘦身档不碰），
//      且被丢的内容少于 `SUMMARY_MIN_INPUT_CHARS` 时**直接跳过**（不值得为一点点内容付费）；
//   ② 改写前缀 ⇒ 缓存整段作废 ⇒ 触发频次要尽量低。「宁可压得晚也不要压得勤」：
//      它只挂在本来就必然会 drain 的那两档上，**不额外制造压缩时机**；
//   ③ 不引 session 分裂（摘要请求与主对话用**同一个** `session_id`，见 `session_id()`）。
//
/// 开关：`0` / `false` / `off` / `no` 关闭。**默认开**（只在丢弃档触发，本身已很稀有）。
const SUMMARY_COMPACT_ENV: &str = "LUNAC_SUMMARY_COMPACT";
/// 送进摘要模型的原文上限（字符）。超出时**从最近的往老的取** —— 被丢区间里越靠近现在越相关。
/// 单轮成本上限就靠它兜：约 24k 字符 ≈ 6k token 输入。
const SUMMARY_MAX_INPUT_CHARS: usize = 24_000;
/// 低于这个体量不值得为它花一次调用
const SUMMARY_MIN_INPUT_CHARS: usize = 4_000;
/// 摘要输出上限。**压得短是刻意的**：摘要是要长期留在上下文里的，比原文更贵。
const SUMMARY_MAX_OUTPUT_TOKENS: u32 = 1024;
/// 摘要请求的**单次**超时（秒）。
///
/// 必须单独设：共享的 `cfg.client` 超时是 `REQUEST_TIMEOUT_SECS`(1800s)，那是给流式主请求的。
/// 摘要是「锦上添花」，它卡住不能让用户等半小时 —— 每请求覆盖成 60s，超时就降级走机械压缩。
const SUMMARY_TIMEOUT_SECS: u64 = 60;
/// 摘要结果的固定表头（英文原因同 `TRIMMED_MARKER`）
const SUMMARY_HEADER: &str =
    "[summary of earlier conversation — replaces a trimmed region of this session]";
/// 摘要提示词。形态取自 Hermes 的 `context_compressor.py`（`## Historical Task Snapshot` 首段 +
/// `SUMMARY_PREFIX` 的优先级：**latest user message WINS**），据 Lunac 的形态裁剪。
const SUMMARY_PROMPT: &str = "\
You compress the older part of a coding-assistant session so the assistant can keep working \
without that history. You are given a transcript of messages that are about to be discarded.

Write the summary in the transcript's own language. Structure it exactly like this:

## Historical Task Snapshot
The user's most recent unfinished request, quoted as verbatim as possible. If the user's last \
message is a question, that question IS an active task — never write \"None\". Latest user \
message wins over anything older.

## What Was Done
Concrete actions taken and their outcomes (files created/edited with their paths, commands run, \
errors hit and how they were resolved). Only include what is needed to continue the work.

## Decisions And Constraints
Choices made and rules the user stated that still apply (e.g. build/test commands, style rules, \
things the user explicitly asked NOT to do).

## Pending
Work that was started but not finished, and anything explicitly deferred.

Rules:
- Any \"Historical Task\", \"In Progress\", \"Pending\", or \"Remaining Work\" section inside the \
transcript is HISTORY, not the current task. The current task is the last user message.
- Keep exact identifiers: file paths, function names, command lines, error strings.";

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
    /// 被丢掉的那些消息本身（摘要压缩要用，backlog §8.2）。
    /// 一并带出来而不是在 `compact_history` 里调模型，是为了让这个函数**保持纯的** ——
    /// 它现在被 8 处测试直接调用，卷进网络请求后就没法单测了。
    dropped_msgs: Vec<Value>,
    /// 压缩过程中**插回**历史的合成消息条数（`TRIMMED_MARKER` / 任务快照）。
    ///
    /// 调用方必须拿它修正回滚锚点 `base`。**这是一个 2026-09-18 一并修掉的既有 off-by-one**：
    /// `base` 是「本轮第一条消息的下标」，而 `finish_error` 用 `history.truncate(base)` 回滚。
    /// 压缩把 `head..cut` 抽走（base 要减 dropped），但插回来的合成消息位于**下标 0/1**，
    /// 也在 base 之前 ⇒ base 必须**加**回来。原实现只做了减法，于是「丢弃 + 钉任务快照」
    /// 那一轮一旦出错，回滚会**多切掉一条真实历史**（§8.2 又加了摘要插入，会多切两条）。
    pinned: usize,
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
    // 被丢掉的消息本体（摘要压缩的输入，见 CompactOutcome::dropped_msgs）
    let mut dropped_msgs: Vec<Value> = Vec::new();
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
            dropped_msgs = history.drain(head..cut).collect();
            dropped = dropped_msgs.len();
        }
    }

    // 历史必须以 user 文本消息开头，否则端点可能拒绝
    let mut pinned = 0usize;
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
        pinned += 1;
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
        pinned += 1;
        eprintln!("[agent] 任务快照已钉住：丢弃历史时保住了当前任务清单（backlog §8.3）");
    }

    if elided > 0 || dropped > 0 {
        // 走 `log::info` 落 agent 日志，**不再用 eprintln**（2026-09-20）：宿主只是把 stderr
        // 以 warn 级**镜像**一份，而这条恰恰是 A 类命中率崩塌的**唯一物证** ——
        // 实测 09-18 那次 elide（瘦身 23 个 tool_result / 省 176101 字）让当轮 `read`
        // 从 108160 掉到 2176，只能靠 lunac 日志的镜像行才找到。落自己的日志更直接。
        log::info(format!(
            "上下文压缩：瘦身 {elided} 个 tool_result（省 {elided_chars} 字），丢弃 {dropped} 条旧消息"
        ));
        emit(json!({
            "type": "system",
            "subtype": "context_compacted",
            "elided": elided,
            "dropped": dropped,
        }));
    }
    CompactOutcome {
        elided,
        dropped,
        dropped_msgs,
        pinned,
    }
}

/// 摘要压缩是否启用（backlog §8.2）。默认开，只有显式关才关。
fn summary_compact_enabled() -> bool {
    match std::env::var(SUMMARY_COMPACT_ENV) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        Err(_) => true,
    }
}

/// 把一条历史消息渲染成摘要模型看得懂的纯文本。工具调用只留「做了什么」的骨架
/// （名字 + 关键入参），工具结果按需截断 —— 摘要是压缩，不是搬运。
fn render_one_message_for_summary(msg: &Value) -> String {
    let role = msg.get("role").and_then(Value::as_str).unwrap_or("?");
    let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    for b in blocks {
        match b.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => {
                let t = b.get("text").and_then(Value::as_str).unwrap_or("").trim();
                if !t.is_empty() {
                    parts.push(t.to_string());
                }
            }
            "tool_use" => {
                let name = b.get("name").and_then(Value::as_str).unwrap_or("?");
                let input = b
                    .get("input")
                    .map(|i| i.to_string())
                    .unwrap_or_default()
                    .chars()
                    .take(RENDER_TOOL_INPUT_CHARS)
                    .collect::<String>();
                parts.push(format!("[tool_use {name}] {input}"));
            }
            "tool_result" => {
                let t = b
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .chars()
                    .take(RENDER_TOOL_RESULT_CHARS)
                    .collect::<String>();
                if !t.is_empty() {
                    parts.push(format!("[tool_result] {t}"));
                }
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("{role}: {}", parts.join("\n"))
}

/// 送进摘要模型的单条工具入参 / 结果上限（字符）
const RENDER_TOOL_INPUT_CHARS: usize = 400;
const RENDER_TOOL_RESULT_CHARS: usize = 600;

/// 把将被丢弃的消息渲染成摘要输入，**总量封顶在 `SUMMARY_MAX_INPUT_CHARS`**。
///
/// 从**最近的往老的**取：被丢区间里越靠近现在的内容越相关，预算不够时优先保它；
/// 最后再翻转回时间顺序（模型读起来才是顺的）。
fn render_dropped_for_summary(dropped: &[Value]) -> String {
    let mut chosen: Vec<String> = Vec::new();
    let mut budget = SUMMARY_MAX_INPUT_CHARS;
    for msg in dropped.iter().rev() {
        let line = render_one_message_for_summary(msg);
        let n = line.chars().count();
        if line.is_empty() {
            continue;
        }
        if n > budget {
            // 单条就超预算：不再往前找（再老的更不重要），保留已选的即可
            break;
        }
        budget -= n;
        chosen.push(line);
    }
    chosen.reverse();
    chosen.join("\n\n")
}

/// 调模型把被丢弃的历史压成摘要（backlog §8.2）。
///
/// **失败一律返回 `None`，绝不把错误往上抛** —— 摘要压缩是「锦上添花」，它失败时必须
/// 让调用方照旧走纯机械压缩，而不是让整轮对话挂掉（对照规则 25 的瞬时失败处理思路）。
fn summarize_dropped(cfg: &Cfg, dropped: &[Value]) -> Option<String> {
    let transcript = render_dropped_for_summary(dropped);
    // 不值得为一点点内容花一次调用
    if transcript.chars().count() < SUMMARY_MIN_INPUT_CHARS {
        eprintln!(
            "[agent] 跳过摘要压缩：被丢内容仅 {} 字 < 阈值 {SUMMARY_MIN_INPUT_CHARS} 字",
            transcript.chars().count()
        );
        return None;
    }
    let body = json!({
        "model": cfg.model,
        "max_tokens": SUMMARY_MAX_OUTPUT_TOKENS,
        // **非流式**：摘要是内部产物，不必往前端流 —— 也不该占用 stream_event 通道
        "stream": false,
        "system": SUMMARY_PROMPT,
        "messages": [{ "role": "user", "content": [{ "type": "text", "text": transcript }] }],
    });
    let started = Instant::now();
    let resp = cfg
        .client
        .post(&cfg.endpoint)
        // 单请求覆盖超时：共享客户端是 1800s（给流式主请求），摘要不能占用那么久
        .timeout(Duration::from_secs(SUMMARY_TIMEOUT_SECS))
        .header("authorization", format!("Bearer {}", cfg.token))
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send();
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            log::warn(format!("摘要压缩失败（网络层）：{e}"));
            return None;
        }
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let detail: String = resp
            .text()
            .unwrap_or_default()
            .trim()
            .chars()
            .take(200)
            .collect();
        log::warn(format!("摘要压缩失败：HTTP {status} {detail}"));
        return None;
    }
    let v: Value = match resp.json() {
        Ok(v) => v,
        Err(e) => {
            log::warn(format!("摘要压缩失败（响应不是 JSON）：{e}"));
            return None;
        }
    };
    let text = v
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let text = text.trim();
    if text.is_empty() {
        log::warn("摘要压缩失败：模型返回空摘要");
        return None;
    }
    let out_tokens = v
        .get("usage")
        .and_then(|u| u.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    log::info(format!(
        "摘要压缩：{} 条旧消息 / {} 字 → 摘要 {} 字（输出 {out_tokens} tokens，耗时 {}ms）",
        dropped.len(),
        transcript.chars().count(),
        text.chars().count(),
        started.elapsed().as_millis()
    ));
    Some(format!("{SUMMARY_HEADER}\n{text}"))
}

/// 摘要压缩的落点：把摘要钉回历史开头（`compact_history` 之后调用）。
///
/// 与任务快照同一个位置策略 —— **插在第一条之后**，不抢「开头那条用户提问 = 任务目标」。
/// 两者同时存在时的顺序是 [摘要][任务快照]：摘要是「过去发生了什么」，任务快照是「现在要做什么」，
/// 读起来正好由远及近。
fn pin_summary_of_dropped(cfg: &Cfg, history: &mut Vec<Value>, dropped: &[Value]) -> bool {
    if !summary_compact_enabled() || dropped.is_empty() {
        return false;
    }
    let Some(summary) = summarize_dropped(cfg, dropped) else {
        return false;
    };
    history.insert(
        1.min(history.len()),
        json!({ "role": "user", "content": [{ "type": "text", "text": summary }] }),
    );
    true
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

// ── 往期会话索引（A2，2026-09-19）──────────────────────────────────
//
// 会话库（`<exe 根>\ModuleData\history\chat.db`）本来就带着 FTS5 索引，但从建库起
// **一直没有调用方**。这里把它接上：启动时取一份「最近若干条会话」的窄表，拼进系统提示词，
// 让模型知道「用户以前聊过什么」，从而在用户说「上次那个」时去调 `SessionSearch` 检索正文。
//
// **为什么是「启动时取一次」而不是每轮取**：这段进的是系统提示词，而系统提示词一旦
// 变化，端点侧的整段前缀缓存就作废（ai-spec §11 规则 18 / 23）。每轮都重取 = 每轮都
// cache-miss，代价远大于这点信息量。所以做成**冻结快照**：进程生命周期内逐字节不变，
// 本次会话新存的会话只落盘，下次重启 agent 才可见。

/// 开关：`0` / `false` / `off` / `no` 关闭。**默认开**（写法与 `LUNAC_SUMMARY_COMPACT` 一致）。
const HISTORY_INDEX_ENV: &str = "LUNAC_HISTORY_INDEX";

/// 往期会话索引是否启用。
fn history_index_enabled() -> bool {
    match std::env::var(HISTORY_INDEX_ENV) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        Err(_) => true,
    }
}

/// 取「往期会话索引」固定段（带前导空行，直接拼进系统提示词）。
///
/// `search_tool_on` = `SessionSearch` 没有被 `--disallowedTools` 裁掉。**必须传**：
/// 索引段里明确写着「去调 `SessionSearch` 读正文」，工具被禁用还注入索引，等于
/// 让模型去调一个不存在的工具（与技能清单遇 `Skill` 被禁就不列是同一个道理）。
///
/// 拿不到就返回**空串**，任何情况都不报错、不阻断启动：索引是锦上添花 ——
/// 桥没接通、库是空的（全新安装）、方法报错，都只是「这段不注入」而已。
fn history_index_block(bridge: Option<&mut mcp::Bridge>, search_tool_on: bool) -> String {
    if !search_tool_on {
        eprintln!("[agent] SessionSearch 已被 --disallowedTools 禁用：不注入往期会话索引");
        return String::new();
    }
    if !history_index_enabled() {
        eprintln!("[agent] 往期会话索引已关闭（{HISTORY_INDEX_ENV}=0）");
        return String::new();
    }
    let Some(b) = bridge else {
        return String::new();
    };
    match b.history_index() {
        Ok(text) => {
            let text = text.trim();
            if text.is_empty() {
                return String::new();
            }
            eprintln!("[agent] 往期会话索引已注入（{} 字）", text.chars().count());
            format!("\n\n{text}")
        }
        Err(e) => {
            eprintln!("[agent] 往期会话索引不可用（跳过）: {e}");
            String::new()
        }
    }
}


// ── 长期记忆（A4，2026-09-20）────────────────────────────────────
//
// 与**往期会话索引**（A2）是两层不同的东西，别混：
//   · 往期会话索引 = `chat.db` 里的**原始流水**。注入的只是一张「标题 + 首句」的窄表，
//     正文要靠模型自己调 `SessionSearch` 现查；
//   · 长期记忆 = 从流水里**提炼出来的少量结论**（用户偏好 / 项目约定 / 踩过的坑），
//     整段注入（服务端上限 6000 字符）。
// 为什么结论要注入、而流水只给索引：结论是每轮都可能用到的背景（现查既慢又贵），
// 而流水太长、只能现查。
//
// 写入侧（`Remember` 工具 + 每 N 轮的后台复盘 fork）见 `main()` 的装配与 `run_review_fork`。
//
// 冻结快照纪律（规则 18 / 53，与往期会话索引同）：**只在启动时读一次**。会话中新写的
// 记忆**只落盘**，本次进程的系统提示词逐字节不变；下次重启 agent 才可见。
// 反面做法（「保存记忆后刷新提示词」）会让每一轮都 cache-miss，代价远大于这点信息量。

/// 开关：`0` / `false` / `off` / `no` 关闭（写法与 `LUNAC_HISTORY_INDEX` 一致）。**默认开**。
const MEMORY_ENV: &str = "LUNAC_MEMORY";

fn memory_enabled() -> bool {
    match std::env::var(MEMORY_ENV) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        Err(_) => true,
    }
}

/// 注入侧长度上限。与 MCP 服务端 `MAX_MEMORY_CHARS` 同值（6000 字符 ≈ 1500 token，
/// 占 128k 预算的 1.2%）—— 服务端管「写」，这里管「发」；用户手工把文件改大时，
/// 兜底截断仍然由这一层负责。
const MEMORY_INJECT_CHARS: usize = 6_000;

/// 取当前长期记忆文本；没有 / 空 / 出错都返回 `None`（**不报错** —— 记忆是锦上添花）。
///
/// 两个调用方：启动时的注入（`memory_block`）与每次复盘前的「当前记忆」（放进复盘的
/// 提示词，好让它不重复写已有条目）。
fn fetch_memory(bridge: Option<&mut mcp::Bridge>) -> Option<String> {
    let b = bridge?;
    match b.memory_read() {
        Ok(t) => {
            let t = t.trim();
            // 空记忆时服务端回的那句是**回执文案**，不是记忆内容
            if t.is_empty() || t == "(long-term memory is empty)" {
                None
            } else {
                Some(t.to_string())
            }
        }
        Err(e) => {
            eprintln!("[agent] 长期记忆读取失败（当作没有）: {e}");
            None
        }
    }
}

/// 取「长期记忆」固定段（带前导空行，直接拼进系统提示词）。
///
/// `remember_tool_on` = `Remember` 真的在工具池里。**必须传**：段里写着「用 `Remember`
/// 工具追加」，工具不在还这么写，等于指挥模型去调一个不存在的工具（与往期会话索引遇
/// `SessionSearch` 被禁就不注入是同一条纪律）。
///
/// 拿不到就返回**空串**，任何情况都不报错、不阻断启动。
fn memory_block(bridge: Option<&mut mcp::Bridge>, remember_tool_on: bool) -> String {
    if !memory_enabled() {
        eprintln!("[agent] 长期记忆已关闭（{MEMORY_ENV}=0）");
        return String::new();
    }
    let Some(text) = fetch_memory(bridge) else {
        return String::new();
    };
    // 超限时保留**较早**的部分（`truncate_chars` 留头部）：较早的条目是更established的
    // 约定，尾部的新条目丢掉时至少下次还能再写一次。正常路径到不了这里（服务端写入
    // 已按同值封顶），只有用户手工把文件改大才会触发。
    let body = if text.chars().count() > MEMORY_INJECT_CHARS {
        crate::log::truncate_chars(&text, MEMORY_INJECT_CHARS)
    } else {
        text
    };
    let tail = if remember_tool_on {
        "- Add a fact with the `Remember` tool (one concise fact per call). Do not use it for \
         one-off details or for the current task's progress."
    } else {
        "- (The `Remember` tool is disabled in this session, so this memory can only be \
         extended outside this app.)"
    };
    eprintln!("[agent] 长期记忆已注入（{} 字）", body.chars().count());
    format!(
        "\n\n# Long-term memory\n\
         Durable facts saved in earlier sessions. Treat them as background you already know; \
         they are not part of the current request.\n\
         {tail}\n\n{body}",
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

    /// 给**后台复盘线程**用的独立副本：同一个端点 / 凭据 / 模型 / 已解析的思考形态，
    /// 但 HTTP 客户端与两个 `Cell` 都是新的。
    ///
    /// 为什么不能把 `&Cfg` 传进线程：`Cfg` 含 `Cell` ⇒ 不是 `Sync`，`&Cfg` 无法跨线程。
    /// 也不能把主循环那份 move 进去再还回来 —— 那要求主循环在复盘期间放弃 `cfg`，
    /// 徒增一类状态（「cfg 现在在谁手上」）。
    ///
    /// `thinking` 取**当前已跑通并缓存**的那个形态（400 降级的结果）：复盘不必再走一遍
    /// 降级链 —— 能问到第 10 轮，说明主请求至少成功过一次（同子代理的做法）。
    /// 两个 `Cell` 归零是对的：复盘是独立的短对话，没有「上一轮实测体积」可言。
    fn detached(&self) -> Result<Cfg, String> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .map_err(|e| format!("build http client: {e}"))?;
        Ok(Cfg {
            client,
            endpoint: self.endpoint.clone(),
            token: self.token.clone(),
            model: self.model.clone(),
            thinking: Cell::new(self.thinking.get()),
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

/// 本进程的**会话 id**（A11，2026-09-20）：进程启动时生成一次，此后全程不变。
///
/// 形态 `sess_<pid>_<启动时刻 epoch 毫秒>`：进程内唯一、可读、**不引 chrono / uuid**。
/// **它的语义是「一次 agent 运行」**，不是「一段对话」：宿主重启 agent（换模型 / 换思考档 /
/// 换工作区 / 回退时取消流式……）就是新会话。历史仍由前端经 `set_history` 灌进来
/// （§11 规则 30）—— 这个 id 不改那套机制，它只负责**归因**：stdout 的每条消息、agent 落盘
/// 日志、前端的用量记录三者从此能对上「哪些东西属于同一次 agent 运行」。
///
/// 为什么要有个 id 而不是继续用 `""`：`""` 让「本轮属于哪次运行」在日志里无法区分 ——
/// 排查「两轮之间发生了什么」时只能靠时间戳猜。
///
/// **覆盖范围（别误读）**：带 session 归因的是 `system/init`、`result`（成功与出错两条）、
/// 权限 hook 的 payload 与启动日志行。`task_id`（子代理 / 技能）**仍是**进程内短计数
/// `task-1` / `skill-1` —— 它会出现在给模型看的回灌文本里，短才好读（见约束②），
/// 所以不往里塞 session 前缀。
fn session_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        format!("sess_{}_{}", std::process::id(), ms)
    })
}

/// system/init —— 前端据此把 agentState 从 starting 推进到 idle。
/// 真实 CLI 每轮查询都会发一次，这里保持一致；`tools` 上报本轮可用工具名。
fn emit_init(model: &str, tool_names: &[String]) {
    emit(json!({
        "type": "system",
        "subtype": "init",
        "session_id": session_id(),
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

/// 从写入类工具的入参里取出**将被写进磁盘的文本**（审批卡的凭据扫描用它）。
///
/// `Write` 取 `content`；`Edit` 取 `new_string` —— **不取 `old_string`**：那是要被删掉的
/// 内容，扫它会把「正在清理凭据」的操作也标成可疑，正好反了。
/// 返回 `None` = 入参缺字段（工具执行时会自己报参数错误，这里不重复判定）。
fn written_payload<'a>(tool_name: &str, input: &'a Value) -> Option<&'a str> {
    let key = if tool_name == "Edit" { "new_string" } else { "content" };
    input.get(key).and_then(Value::as_str)
}

/// 只登记 + 发请求，不阻塞 —— 一批工具先全部发出，前端才能把连续
/// Bash 合并成一行（`findLastBashGroup`）再让用户一次性决定。
fn open_approval(tool_name: &str, tool_use_id: &str, input: &Value) -> Pending {
    let request_id = next_request_id();
    let (tx, rx) = mpsc::channel::<Value>();
    if let Ok(mut reg) = pending_approvals().lock() {
        reg.insert(request_id.clone(), tx);
    }
    // 降级为 debug（2026-09-19 日志精简）：**每个需要审批的工具调用**都会走到这里，
    // 而它此前走 eprintln ⇒ 宿主 `commands.rs` 把 `[agent stderr]` 以 **warn** 级落盘，
    // 实测占 dev 日志行数的三分之一。审批是否发出在前端卡片上是可见的，
    // 这里只在排查「审批卡住」时才需要 —— 打开 `LUNAC_LOG_LEVEL=debug` 即可。
    // 审批的**结果**（超时 / 用户拒绝）仍是 warn 级，不受影响。
    log::debug(format!("等待审批 {tool_name}"));
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
                // A10：**可证只读**结论。前端的「白名单」档据此自动放行（取代原先那个
                // 看不见重定向/管道的前缀表）。false 只表示「不给自动放行」，仍然弹卡。
                "readonly": report.readonly,
            });
        }
    }
    // 写入类工具附上**写入内容**的静态安全分析（2026-09-20，原 backlog A6；见 content_safety.rs）。
    // 只做「凭据 / 密钥泄漏」这一类 —— 审批卡是用户唯一能看见内容的地方，
    // 把可疑点抽出来显式标出，否则「扫一眼就点允许」等于没有审批。
    // 前端把它当「必须人看一档」处理：不自动放行、也不给「始终允许」。
    if matches!(tool_name, "Write" | "Edit") {
        if let Some(text) = written_payload(tool_name, input) {
            let report = content_safety::analyze(text);
            if !report.is_clean() {
                log::warn(format!("写入内容静态分析 {tool_name}: {}", report.summary()));
            }
            request["analysis"] = json!({ "secrets": report.json_hits() });
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
        // warn 级落 agent 日志（原来是 eprintln ⇒ 宿主以 warn 级镜像一遍，两处重复）
        log::warn(format!("审批超时（{APPROVAL_TIMEOUT_SECS}s），按拒绝处理"));
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
            log::info("用户在审批卡上拒绝（结果会回灌给模型）");
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

// ── 权限 hooks（A9，2026-09-20）───────────────────────────────────
//
// 用户在 `config\hooks.json` 里挂脚本，在 8 个事件上介入（契约与逐条纪律见
// docs/ai-spec.md §3.5「权限 hooks」与 §11 规则 61）。**事件的实际落点**：
//
// | 事件 | 落点 |
// |---|---|
// | `SessionStart` | `main()` 里 cfg 就绪之后、收 stdin 之前 |
// | `UserPromptSubmit` | stdin 循环的 `user` 分支，进 `run_query` 之前（可拦下整轮） |
// | `PreToolUse` | 主循环与子代理循环的「执行工具」段，**全部**工具调用都过一遍 |
// | `PermissionRequest` | 只在「本来要弹审批卡」的那一刻（hook 可代答） |
// | `PostToolUse` | `run_one_tool()` 内部，工具跑完之后（文本拼进 `tool_result` 内部） |
// | `PreCompact` | `compact_history()` 的三处调用点之前 |
// | `Stop` | `run_query()` 成功收尾、`result` 发出之后 |
// | `SessionEnd` | `main()` 退出前（stdin 关闭） |
//
// 所有「跑 + 上报」都收在下面这几个函数里，免得纪律散到 8 处。

/// hook 对一次工具调用的裁决。
enum HookGate {
    /// 没有 hook 表态 —— 照原逻辑（该弹卡就弹卡）
    Pass,
    /// hook 明确放行 —— 等价用户白名单：跳过审批卡，但**安全分析命中仍要弹**
    Allow,
    /// hook 明确拒绝 —— 工具不执行，原因以 is_error 的 tool_result 回给模型
    Deny(String),
}

/// 把一次 hook 运行的产出送到该去的地方：落盘日志 + 前端的 `system/hook_note`。
/// `tool` 只有工具类事件才有。
fn report_hook_run(event: &str, tool: Option<&str>, run: &hooks::Run) {
    if run.is_quiet() {
        return;
    }
    for it in &run.items {
        let line = format!(
            "hook {event}{} [{}] {}（{}）",
            tool.map(|t| format!(" {t}")).unwrap_or_default(),
            it.kind,
            it.text,
            it.command,
        );
        match it.kind {
            "error" => log::warn(line),
            _ => log::info(line),
        }
    }
    emit(json!({
        "type": "system",
        "subtype": "hook_note",
        // 字段名用 `hook_event`（不是 `event`）：`event` 已经被 `stream_event` 占用
        // （那是个对象），前端同一个 interface 里两个形状会打架。
        "hook_event": event,
        "tool_name": tool,
        "items": run
            .items
            .iter()
            .map(|i| json!({ "kind": i.kind, "text": i.text, "command": i.command }))
            .collect::<Vec<_>>(),
    }));
}

/// 工具类事件的 payload（与 Claude Code 同形：`tool_name` / `tool_input` / `tool_use_id`）。
fn hook_tool_payload(event: &str, cwd: &Path, tool: &str, id: &str, input: &Value) -> Value {
    json!({
        "session_id": session_id(),
        "hook_event_name": event,
        "cwd": cwd.display().to_string(),
        "tool_name": tool,
        "tool_input": input,
        "tool_use_id": id,
    })
}

/// 工具类事件的统一裁决口（PreToolUse / PermissionRequest 共用）。
fn hook_tool_gate(event: &str, cwd: &Path, tool: &str, id: &str, input: &Value) -> HookGate {
    let h = hooks::current();
    if h.is_empty() {
        return HookGate::Pass;
    }
    let run = hooks::fire(&h, event, &hook_tool_payload(event, cwd, tool, id, input));
    report_hook_run(event, Some(tool), &run);
    match run.decision {
        hooks::Decision::Pass => HookGate::Pass,
        hooks::Decision::Allow => HookGate::Allow,
        hooks::Decision::Deny(r) => HookGate::Deny(r),
    }
}

/// hook 说「放行」时还要过一遍**静态安全分析**：命中危险命令 / 写入内容里的凭据，
/// 就仍然要弹卡。这是 ai-spec 规则 14「任何一道闸门都不得为了少点一次同意而放宽」
/// 在 hooks 上的落点 —— hook 的 allow 只等于用户白名单，不等于绕过安全分析。
///
/// 口径与前端「自动」档一致：`opaque`（判不定）**不**强制弹卡，`dangerous` / 凭据命中才强制。
fn hook_allow_needs_card(tool: &str, input: &Value) -> bool {
    if matches!(tool, "Bash" | "PowerShell") {
        if let Some(cmd) = input.get("command").and_then(Value::as_str) {
            return !bash_safety::analyze(cmd).dangerous.is_empty();
        }
    }
    if matches!(tool, "Write" | "Edit") {
        if let Some(text) = written_payload(tool, input) {
            return !content_safety::analyze(text).is_clean();
        }
    }
    false
}

/// 非工具类事件的统一入口：构造 payload → 跑 → 上报，返回这次运行（调用方看 `decision`）。
fn fire_plain_hook(event: &str, cwd: &Path, payload: Value) -> hooks::Run {
    let h = hooks::current();
    if h.is_empty() {
        return hooks::Run::default();
    }
    let mut payload = payload;
    payload["session_id"] = json!(session_id());
    payload["hook_event_name"] = json!(event);
    payload["cwd"] = json!(cwd.display().to_string());
    let run = hooks::fire(&h, event, &payload);
    report_hook_run(event, None, &run);
    run
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
    // 技能目录（A4，2026-09-20）也要进可访问范围：复盘 fork 的白名单里有「改技能」
    // （`Write` / `Edit`），而技能目录在**工作区之外** —— 不进这个名单，最外层的工作区锁
    // 会把它直接拒掉（不是弹审批，是拒），「改技能」那半就永远不会发生。
    // 与 `output_dir` 同理：进的是**应用自己的目录**，不是把用户的工作区边界放宽。
    if let Ok(dir) = std::env::var("LUNAC_SKILLS_DIR") {
        let dir = PathBuf::from(dir.trim());
        if !dir.as_os_str().is_empty() {
            add_dirs.push(dir);
        }
    }
    let tools_ctx = tools::Ctx {
        cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        add_dirs,
        read_only: cli.permission_mode == "plan",
        locked: std::env::var("LUNAC_WORKSPACE_LOCKED").ok().as_deref() == Some("1")
            && !cli.skip_permissions,
        // 计划相位（A7）：**进程内**的状态，起手恒为「不在计划相位」——
        // 只读档是另一回事（那是用户档位，见 `read_only` 的注释）。
        plan_phase: Arc::new(AtomicBool::new(false)),
    };

    // P3 MCP 工具桥：把 <exe 根>\tools\*.json 的用户工具接进工具池。
    // 连不上只是少一批工具 —— 内置工具必须照常可用，所以这里只记一行。
    //
    // **plan（只读）档也建桥，但不把 `mcp__*` 工具放进工具池**（2026-09-19 调整）：
    // 建桥的理由是桥上还挂着两个**自定义方法**（`lunac/history_index` /
    // `lunac/history_search`），它们支撑只读工具 `SessionSearch` —— 只读档查自己的历史
    // 完全正当。不列工具的理由没变：MCP 工具在只读档一律会被 `dispatch_tool` 拒掉，
    // 列出来只会让工具清单随档位漂移、白占固定前缀。
    let mut mcp_bridge = match cli.mcp_server.as_deref() {
        Some(spec) => match mcp::Bridge::connect(spec, &cli.disallowed) {
            Ok(b) => {
                eprintln!(
                    "[agent] MCP 桥已接通，用户工具 {} 个{}{}",
                    b.defs().len(),
                    if tools_ctx.read_only {
                        "（只读档：不接入工具池，仅供 SessionSearch 用）"
                    } else {
                        ""
                    },
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
    // 往期会话索引（A2，2026-09-19）：**只在启动时取一次**，拼进固定前缀。
    // 「启动之后新存的会话不会出现在这里」这条纪律写在 `history_index_block` 的注释里。
    let session_search_on = !cli.disallowed.iter().any(|d| d == "SessionSearch");
    let history_block = history_index_block(mcp_bridge.as_mut(), session_search_on);
    // 长期记忆（A4，2026-09-20）：与上一行同为**启动时取一次**的冻结快照。
    // 「写侧工具是否真的在池里」先算出来 —— 注入文案与条件注册必须同源，否则会出现
    // 「提示词让模型调 Remember，但它不在工具表里」这种自相矛盾的组合。
    let remember_on = memory_enabled()
        && mcp_bridge.is_some()
        && !cli.disallowed.iter().any(|d| d == "Remember");
    let memory_section = memory_block(mcp_bridge.as_mut(), remember_on);
    let system_prompt = format!(
        "{SYSTEM_PROMPT}\n\n{PERSONA_AND_STYLE}{}{}{}{}",
        env_block(&tools_ctx.cwd),
        skills::listing(if skills_on { &skills } else { &[] }),
        history_block,
        memory_section
    );
    // 复盘 fork 的系统提示词 = 角色段 + 环境块 + 技能清单（与子代理同构，**不含记忆**：
    // 当前记忆每次都不同，放进复盘的**用户消息**里，系统提示词才能跨次逐字节相同）。
    let review_system = format!(
        "{REVIEW_SYSTEM}{}{}",
        env_block(&tools_ctx.cwd),
        skills::listing(if skills_on { &skills } else { &[] })
    );
    // 子代理的系统提示词 = 角色段 + 环境块 + 技能清单。**与主提示词同源、只构建一次**，
    // 因此所有子代理共享一段逐字节相同的前缀（ai-spec §11 规则 18）。
    //
    // 为什么必须补后两块（2026-09-20 复查）：原实现只发 `SUBAGENT_SYSTEM` 角色段，于是
    //   ① 子代理**不知道自己的工作目录绝对路径** —— 它看不到主对话的环境块，只能指望
    //      调用方在 `prompt` 里手抄一遍 cwd（探针里模型确实手抄了，但不能指望每次都记得；
    //      抄错了它还会照着一个不存在的位置去 Glob）；
    //   ② `Skill` 在子代理的工具集里（技能是纯本地能力，与 MCP 桥无关），而**技能清单
    //      原本只写在主提示词里** —— 不给清单，等于给一串不知道有哪些钥匙的钥匙串。
    let subagent_system = format!(
        "{SUBAGENT_SYSTEM}{}{}",
        env_block(&tools_ctx.cwd),
        skills::listing(if skills_on { &skills } else { &[] })
    );

    let mut tool_defs = tools::defs(&cli.disallowed);
    let mut tool_names = tools::names(&tool_defs);
    if skills_on {
        tool_defs.push(skills::tool_def());
        tool_names.push("Skill".into());
    }
    // 长期记忆的写入侧（A4）：**条件注册**，判据与上面那段注入文案同源（`remember_on`）。
    // 桥没接通就写不进去 —— 注册了也只是一件必然失败的工具，白占固定前缀。
    if remember_on {
        tool_defs.push(tools::remember_tool());
        tool_names.push("Remember".into());
    }
    if let Some(b) = &mcp_bridge {
        // 只读（plan）档**不把 MCP 工具放进工具池**（桥仍然戴着，供 SessionSearch 用）——
        // 理由见上面建桥处的注释。
        if !tools_ctx.read_only {
            tool_defs.extend(b.defs().iter().cloned());
            tool_names.extend(tools::names(b.defs()));
        }
    }

    // MCP resources 读侧两件（A3，2026-09-20）：**条件注册** —— 只在桥真的接上了用户工具
    // 时才追加（与 `Skill` 同理）。
    //
    // 为什么必须条件化：出厂时 `<exe 根>\tools\` 只有 README 与 `*.example`，**一个可加载的
    // 工具都没有** ⇒ `resources/list` 恒为空表。这时把它们注册进去，等于在**每一次请求的
    // 固定前缀**里放两件永远查不到东西的占位工具 —— 正是 §11 规则 18 ⑤ 批评的「空转项」。
    //
    // 判据用 `b.defs()` 而不是另发一次 `resources/list`：两者同源（都来自 `tools\*.json`），
    // 而 `defs()` 握手时已经拿在手里 —— 少一次启动 RPC。
    let has_user_tools = mcp_bridge.as_ref().map_or(false, |b| !b.defs().is_empty());
    if has_user_tools {
        for def in [tools::list_resources_tool(), tools::read_resource_tool()] {
            let name = def
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if cli.disallowed.iter().any(|d| d == &name) {
                continue;
            }
            tool_defs.push(def);
            tool_names.push(name);
        }
    }

    // 复盘 fork 的工具集（A4）：从**本轮的工具池**里挑白名单子集 —— 于是它天然继承
    // `--disallowedTools` 与两处条件注册（没装技能 ⇒ 没有 `Skill`；没桥 ⇒ 没有 `Remember`）。
    // 这也顺带阻止了递归：`Agent` 不在白名单里，复盘看不到它。
    let review_defs: Vec<Value> = tool_defs
        .iter()
        .filter(|d| {
            d.get("name")
                .and_then(Value::as_str)
                .map_or(false, |n| REVIEW_TOOL_WHITELIST.contains(&n))
        })
        .cloned()
        .collect();
    let review_setup = ReviewSetup {
        defs: review_defs,
        skills: skills.clone(),
        system: review_system,
        ask_permission: cli.ask_permission,
        // `--mcp-server` 的原样值：复盘拿它连自己那条桥（只为此用 `lunac/memory_write`）
        bridge_spec: cli.mcp_server.clone(),
        // 关卡：`remember_on` 为假（没桥 / 记忆被关 / `Remember` 被黑名单裁掉）时把间隔
        // 设成 0 = **不跑复盘**。理由：它的**唯一产品是记忆条目** —— 写不进去还每 N 轮
        // 花一次 API 调用，是纯粹的浪费（`should_review` 把 `interval == 0` 当关闭）。
        interval: if remember_on { nudge_interval() } else { 0 },
    };

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
        "[agent] P1–P4 就绪 session={} cwd={} 工具=[{}]{}{}",
        session_id(),
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
                "session_id": session_id(),
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

    // ── SessionStart hook（A9）──
    // 落点：配置就绪、还没开始收 stdin。此时发出的 stdout 事件前端**可能还没挂上监听**
    // （宿主刚 spawn 出 agent ⇒ 这一条的产出以落盘日志为主，前端可见为辅）。
    fire_plain_hook("SessionStart", &tools_ctx.cwd, json!({ "model": cfg.model }));

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
    // ── 后台复盘（A4）的装配状态 ────────────────────────────────────
    // `questions` = 已完成的**用户提问**数（不是工具轮次：一次提问内部可以有 16 轮工具
    // 往返，按那个计数会在一次长提问中途触发复盘，而那时复盘看到的还是半截对话）。
    // `pending` = 上一次复盘还没收回来。**收的时机是「处理下一条消息之前」，且只收
    // 已经跑完的**（见 `collect_review`）—— 复盘是后台事务，不许它决定前台时延。
    let mut questions: u64 = 0;
    let mut review_seq: u64 = 0;
    let mut pending_review: Option<thread::JoinHandle<Result<String, String>>> = None;
    for msg in rx {
        collect_review(&mut pending_review);
        match msg.get("type").and_then(Value::as_str) {
            Some("user") => {
                let images = extract_user_images(&msg);
                let mut prompt = extract_user_text(&msg);
                if prompt.trim().is_empty() && images.is_empty() {
                    continue;
                }
                // 纯图片提问（一个字都没写）：补一句最小文本，保证 history 仍以 user
                // 文本消息开头（不变量见 §11 规则 18 ①）。前端当前总会带上
                // `[Attached files]` 包装文本，这条是给别的调用方兜底。
                if prompt.trim().is_empty() {
                    prompt = "(the user attached image(s) with no text)".to_string();
                }
                // ── UserPromptSubmit hook（A9）──
                // 能拦下**整轮提问**：提问不进历史、不烧 token。拦下的原因必须让用户看见
                // （`result.subtype=hook_blocked` ⇒ 前端按错误行渲染并退回 idle），
                // 否则表现就是「发了消息没反应」，最像卡死。
                if let hooks::Decision::Deny(reason) =
                    fire_plain_hook(
                        "UserPromptSubmit",
                        &tools_ctx.cwd,
                        json!({ "prompt": prompt }),
                    )
                    .decision
                {
                    log::warn(format!("UserPromptSubmit hook 拦下本轮提问：{reason}"));
                    emit(json!({
                        "type": "result",
                        "subtype": "hook_blocked",
                        "is_error": true,
                        "duration_ms": 0,
                        "num_turns": 0,
                        "result": format!("被 UserPromptSubmit hook 拦下：{reason}"),
                        "session_id": session_id(),
                        "total_cost_usd": 0.0,
                    }));
                    continue;
                }
                run_query(
                    &cfg,
                    &mut history,
                    &prompt,
                    &images,
                    &tools_ctx,
                    &tool_defs,
                    &tool_names,
                    cli.ask_permission,
                    mcp_bridge.as_mut(),
                    &skills,
                    &system_prompt,
                    &subagent_system,
                );
                // 复盘的三个前置：① 到轮次门槛；② 不是只读（plan）档、也不在计划相位
                // （复盘的唯一产品就是一条 `Remember` 写入，那两档下 `write_blocked` 会
                // 直接拒 ⇒ 派出去也是白烧一次 API）；③ 上一轮那次已经收干净。
                // 「当前记忆」在主线程读（那条桥归主线程）—— 复盘靠它避免重复写。
                questions += 1;
                if pending_review.is_none()
                    && !tools_ctx.read_only
                    && !tools_ctx.plan_phase.load(Ordering::Relaxed)
                    && should_review(questions, review_setup.interval)
                {
                    let memory = fetch_memory(mcp_bridge.as_mut()).unwrap_or_default();
                    review_seq += 1;
                    pending_review =
                        review_setup.spawn(&cfg, &tools_ctx, &history, &memory, review_seq);
                }
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
                // 只留 log::info（agent 日志）—— 原来这里还多一行同文本的 eprintln，
                // 而宿主会把 stderr 以 warn 级镜像进自己的日志 ⇒ 同一件事两个文件各一条。
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
    // 同 set_history：两处同文本，只留 agent 日志那一份
    // 退出前收掉**已完成**的复盘（还在跑的直接放弃：进程要退出了，等它没有意义 ——
    // 它写的是记忆文件，半途停下也只是丢掉这一条）。`collect_review` 不阻塞。
    if !collect_review(&mut pending_review) {
        if pending_review.is_some() {
            log::info("退出：后台复盘仍在运行，不等它");
        }
    }
    log::info("=== agent exit (stdin closed) ===");
    // ── SessionEnd hook（A9）──
    // 上游（lunac.exe）已退出 / 主动关了 stdin ⇒ 这个 agent 进程的生命周期结束。
    // 同样是「只做事、拦不住」的一类：脚本可以用它清理临时资源、发通知。
    fire_plain_hook("SessionEnd", &tools_ctx.cwd, json!({}));
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

// ── 图片附件（A8，2026-09-20）────────────────────────────────────
//
// stdin 的 user 消息里可以带 `{"type":"image","source":{"type":"file","path":"…"}}`
// 块 —— **只传路径、不传字节**：
//   · 前端三种来源（粘贴 / 拖拽 / 文件对话框）本来就已经落成路径，不必再读一遍文件；
//   · 几 MB 的 base64 不必过 IPC 管道，也不占前端的字符串内存。
// 字节由这里按路径读出来、转成端点要的 base64 块（契约见 ai-spec §3.5「图片附件」）。
//
// **开关在前端**（设置面板「模型支持图片输入」，落在 `config\ai.json`）：发给不支持
// 视觉的端点（如 DeepSeek 官方端点）会 400 ⇒ 默认关、由用户显式打开。agent 侧因此
// 不做二次判定：收到就发，只有「读不出来」才如实上报（`system/attachment_note`）。
//
/// 一条消息里的图片张数上限（端点侧另有总量限制，这里先卡住明显失控的情况）
const MAX_IMAGE_FILES: usize = 10;
/// 单张图片的原始字节上限。端点的 5 MB 通常按 base64 后算，而 base64 膨胀约 4/3
/// ⇒ 原始字节卡在 3.5 MB 以内才不会越线。
const MAX_IMAGE_BYTES: u64 = 3_500_000;

/// 按**魔术字节**判图片类型（不信扩展名：改过名的文件、剪贴板落盘的 `.png` 都可能
/// 是别的东西）。只认端点支持的四种 —— 其余（BMP / TIFF / ICO …）宁可退回「路径文本」
/// 那条老路，也不发一个必然被端点拒的块。
fn image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// 一条 `image` 块里声明的路径（报错时用来指认是哪张图）。
fn image_src_path(block: &Value) -> String {
    block
        .pointer("/source/path")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// 取出消息里的图片块（原样，未解析）—— 与 `extract_user_text` 读同一份 `content`。
fn extract_user_images(msg: &Value) -> Vec<Value> {
    msg.pointer("/message/content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("image"))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// 把一条 `image` 块（`source.type = "file"`）变成端点要的 base64 块。
///
/// 刻意**不检查工作区锁 / 只读档**：路径来自**用户显式选中的附件**，不是模型自己找
/// 出来的（模型的 `Read` 照旧受锁约束）—— 而剪贴板图片本来就落在 `%TEMP%`，套锁会让
/// 最主要的那条用法直接失效（见 ai-spec §11 规则 60）。
fn load_image_block(block: &Value) -> Result<Value, String> {
    let src = block.get("source").ok_or("image block has no source")?;
    if src.get("type").and_then(Value::as_str) != Some("file") {
        return Err("unsupported source (only type=file is accepted)".into());
    }
    let path = image_src_path(block);
    if path.is_empty() {
        return Err("empty path".into());
    }
    let meta = std::fs::metadata(&path).map_err(|e| format!("cannot stat: {e}"))?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "{} KB exceeds the {} KB per-image limit",
            meta.len() / 1024,
            MAX_IMAGE_BYTES / 1024
        ));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("read failed: {e}"))?;
    let media =
        image_media_type(&bytes).ok_or("not a PNG / JPEG / GIF / WebP (checked by magic bytes)")?;
    Ok(json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": media,
            "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
        },
    }))
}

/// 把消息里的图片块批量解析成端点块。
///
/// 返回 `(可用块, 未能发出的说明)`。**失败的那些不静默丢弃** —— 调用方会把它们汇总成
/// `system/attachment_note` 如实告诉用户「这张图没发出去、为什么」，否则用户只看到
/// 「模型说它看不到图」而毫无线索。原顺序保持（端点侧图文顺序有语义）。
fn collect_image_blocks(raw: &[Value]) -> (Vec<Value>, Vec<Value>) {
    let mut blocks = Vec::new();
    let mut notes = Vec::new();
    for (i, b) in raw.iter().enumerate() {
        let path = image_src_path(b);
        if i >= MAX_IMAGE_FILES {
            notes.push(json!({
                "path": path,
                "reason": format!("more than {MAX_IMAGE_FILES} images in one message"),
            }));
            continue;
        }
        match load_image_block(b) {
            Ok(block) => blocks.push(block),
            Err(reason) => notes.push(json!({ "path": path, "reason": reason })),
        }
    }
    (blocks, notes)
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

/// `SessionSearch` 单次返回的条数默认值与上限。
/// **上限与 src-tauri 侧 `HISTORY_SEARCH_MAX_LIMIT` 必须一致**（两边各自 clamp）——
/// 取 30 是为了不让一次检索把整段上下文吃掉。
const SESSION_SEARCH_DEFAULT_LIMIT: u64 = 10;
const SESSION_SEARCH_MAX_LIMIT: u64 = 30;

/// `SessionSearch`：往期会话检索（2026-09-19，A2）。
///
/// 实现在 src-tauri 的 MCP server 侧（会话库归它所有，检索 SQL 的唯一真相源是
/// `chat_db.rs`），这里只做参数校验与转发。它**与 MCP 工具共用同一条桥**，但走的是
/// `lunac/history_search` **自定义方法** ⇒ 不弹审批卡（见 `tools::needs_approval` 的说明）。
fn session_search(bridge: Option<&mut mcp::Bridge>, input: &Value) -> Result<String, String> {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if query.is_empty() {
        return Err("缺少 query 参数：需要传入要在往期会话里检索的关键词".into());
    }
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(SESSION_SEARCH_DEFAULT_LIMIT)
        .clamp(1, SESSION_SEARCH_MAX_LIMIT) as u32;
    let Some(b) = bridge else {
        // 桥没接通（没传 --mcp-server，或 lunac.exe 起不来）⇒ 如实报错，**不编造结果**。
        return Err("往期会话检索不可用：MCP 桥未接通".into());
    };
    b.history_search(query, limit)
}

/// 一次 fork 的形态。
///
/// `run_subagent` 的骨架被**两个调用方**共用（`Agent` 工具派出的子代理、每 N 轮一次的
/// 后台复盘），差异全在这几个字段里。做成结构体而不是继续加形参：这两条路径一共有
/// 八个不同的值，全塞进形参表就成了「十三个位置参数、其中四个是 bool」的调用。
struct ForkSpec<'a> {
    /// 归因 id（子代理 `task-1`…；复盘 `review-1`…），也进错误/收口文案
    task_id: &'a str,
    /// 系统提示词：调用方在启动时拼一次，同一种 fork 之间**逐字节相同**
    system: &'a str,
    /// 工具集（子代理 = `subagent_tool_defs()`；复盘 = 白名单子集）
    defs: &'a [Value],
    skills: &'a [skills::Skill],
    ask_permission: bool,
    max_rounds: usize,
    budget_tokens: u64,
    /// 是否回前端 `task_progress`。子代理**要**（用户在等它）；后台复盘**不要** ——
    /// 它在提问之间跑，而前端收到这个事件只会把状态栏改成「子任务运行中」且**没有**
    /// 恢复的时机（`task_done` 前端不处理），复盘结束时状态栏就停在错的文案上。
    emit_progress: bool,
}

/// 跑一个子代理，返回它的最终文本（`Err` = 整条链路失败）。
///
/// **走非流式**（`stream: false`）：主对话只要结果，中间过程不进 UI，一次取回整包
/// 还能省掉一整套 SSE 解析分支。代价是子代理内部没有逐字流式 —— 这是刻意的。
///
/// 与主循环的关系：**工具审批照做**（传 `spec.ask_permission`）—— `<Agent>` 本身的调用
/// 已经让用户批过一次，但那是「批准派一个子代理」，不是「预先批准它接下来要做的
/// 每一件事」；子代理内部的写操作仍逐次走 `can_use_tool`。
///
/// `bridge` 由调用方给（2026-09-20，A4 起本函数被**后台复盘 fork** 复用）：
/// `Agent` 工具传 `None`（子代理不接桥，见约束⑤的邻居）；复盘 fork 传**它自己那条**桥
/// —— 它只有一个方法要用：`Remember` 背后的 `lunac/memory_write`。
fn run_subagent(
    cfg: &Cfg,
    tctx: &tools::Ctx,
    spec: &ForkSpec,
    mut bridge: Option<&mut mcp::Bridge>,
    prompt: &str,
) -> Result<String, String> {
    let task_id = spec.task_id;
    let max_rounds = spec.max_rounds;
    let budget_tokens = spec.budget_tokens;
    // 写类档位直接拒绝（只读档 / 计划相位）：子代理会写文件、跑命令 —— 它靠这个干活。
    // 派生出去的子代理**共享同一份 `plan_phase`**（`Arc`），所以这里拒的是「派出去」
    // 这个动作本身；它内部的每次写操作各自也还会再被拦一次（`tools::run` 那一层）。
    if let Some(why) = tools::write_blocked(tctx, "Agent") {
        return Err(why);
    }
    let mut history: Vec<Value> = vec![json!({
        "role": "user",
        "content": [{ "type": "text", "text": prompt }],
    })];
    let mut spent: u64 = 0;
    let mut last_text = String::new();

    for round in 1..=max_rounds {
        // 生成参数与主循环**同源**（2026-09-20 复查修）。原先这里是「固定
        // `max_tokens: 4096` + 完全不发 `thinking`」，两个后果都不对：
        //   ① 端点**不发该字段 ≠ 关思考**（实测默认就是开的，见 ai-spec §3.5），而思考
        //      文本算在 `max_tokens` 里 ⇒ 4096 很容易被思考吃光，报告被截断成
        //      `(subagent … finished without writing a report)`；
        //   ② 用户显式关掉思考（`LUNAC_THINKING=off`）时子代理却照旧思考 —— 开关失效。
        // 直接复用 `cfg.thinking`：里面存的是主循环**已经跑通并缓存**的形态（400 降级
        // 的结果），所以子代理不必再走一遍降级链 —— 能派子代理，说明主请求至少成功过一次。
        let plan = cfg.thinking.get();
        let mut body = json!({
            "model": cfg.model,
            "max_tokens": max_tokens_for(plan),
            "stream": false,
            "system": spec.system,
            "messages": history,
        });
        // 与主循环同样的守卫：`tools: []` 在部分端点是非法参数；工具被
        // `--disallowedTools` 全裁掉时就是这个形态（子代理只剩「直接答」一条路）。
        if !spec.defs.is_empty() {
            body["tools"] = json!(spec.defs);
        }
        if let Some(t) = plan.to_json() {
            body["thinking"] = t;
        }
        let resp = cfg
            .client
            .post(&cfg.endpoint)
            .header("authorization", format!("Bearer {}", cfg.token))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| format!("子代理请求发送失败: {e}"))?;
        let status = resp.status();
        let raw = resp
            .text()
            .map_err(|e| format!("子代理响应读取失败: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "子代理请求失败（HTTP {status}）: {}",
                crate::log::truncate_chars(&raw, 400)
            ));
        }
        let parsed: Value =
            serde_json::from_str(&raw).map_err(|e| format!("子代理响应不是合法 JSON: {e}"))?;

        // 用量累计：命中缓存的也算真实吞吐（它同样占预算，只是单价低）
        if let Some(u) = parsed.get("usage") {
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            spent += n("input_tokens")
                + n("output_tokens")
                + n("cache_read_input_tokens")
                + n("cache_creation_input_tokens");
        }

        // 拆包：只要 text 与 tool_use —— thinking 等块**不回灌**（回灌会 400，
        // 同主循环的规矩），但文本块要 push 回 history，否则端点看不到自己说过的话。
        let mut blocks: Vec<Value> = Vec::new();
        let mut calls: Vec<(String, String, Value)> = Vec::new();
        if let Some(arr) = parsed.get("content").and_then(Value::as_array) {
            for b in arr {
                match b.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => {
                        if let Some(t) = b.get("text").and_then(Value::as_str) {
                            if !t.trim().is_empty() {
                                last_text = t.to_string();
                            }
                        }
                        blocks.push(b.clone());
                    }
                    "tool_use" => {
                        let id = b.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                        let name = b.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                        let input = b.get("input").cloned().unwrap_or_else(|| json!({}));
                        blocks.push(b.clone());
                        calls.push((id, name, input));
                    }
                    _ => {}
                }
            }
        }
        if !blocks.is_empty() {
            history.push(json!({ "role": "assistant", "content": blocks }));
        }

        if calls.is_empty() {
            return Ok(if last_text.trim().is_empty() {
                format!("(subagent {task_id} finished without writing a report)")
            } else {
                last_text
            });
        }

        // 预算闸门：**先检查再执行下一批工具**，把「已经烧掉多少」如实算进去
        if spent > budget_tokens {
            return Ok(format!(
                "[subagent {task_id}: hit the {budget_tokens}-token budget at round \
                 {round}; reporting what it has so far]\n\n{last_text}"
            ));
        }

        // 工具串行执行（子代理里不做只读并行：收益小于「多一层并发状态」的复杂度）
        let mut results: Vec<Value> = Vec::with_capacity(calls.len());
        for (id, name, input) in &calls {
            if spec.emit_progress {
                emit(json!({
                    "type": "system", "subtype": "task_progress",
                    "task_id": task_id, "round": round, "tool": name,
                }));
            }
            // hooks（A9）：子代理里的每次工具调用**同样**过 PreToolUse / PermissionRequest
            // —— 「派代理」那一次批准只覆盖派人这个动作，不覆盖它内部每次写操作（规则 14）。
            let denied = match hook_tool_gate("PreToolUse", &tctx.cwd, name, id, input) {
                HookGate::Deny(r) => Some(format!("Denied by PreToolUse hook: {r}")),
                gate => {
                    let allowed =
                        matches!(gate, HookGate::Allow) && !hook_allow_needs_card(name, input);
                    if !allowed
                        && spec.ask_permission
                        && tools::needs_approval(name)
                        && (!tctx.read_only || tools::gated_in_read_only(name))
                    {
                        match hook_tool_gate("PermissionRequest", &tctx.cwd, name, id, input) {
                            HookGate::Deny(r) => {
                                Some(format!("Denied by PermissionRequest hook: {r}"))
                            }
                            HookGate::Allow if !hook_allow_needs_card(name, input) => None,
                            _ => match await_approval(open_approval(name, id, input), input) {
                                Decision::Allow(_) => None,
                                Decision::Deny(msg, _) => Some(msg),
                            },
                        }
                    } else {
                        None
                    }
                }
            };
            let (text, is_error) = run_one_tool(
                tctx,
                // `Agent` 工具传的是 `None`（子代理不接桥：桥是单线程 stdio 通道，主循环还
                // 持有它）；复盘 fork 传的是**它自己那条桥**（见 `run_review_fork`），
                // 所以这里不能写死。
                bridge.as_deref_mut(),
                spec.skills,
                id,
                name,
                input,
                denied.as_deref(),
            );
            results.push(tool_result_block(id, text, is_error));
        }
        history.push(json!({ "role": "user", "content": results }));
    }

    Ok(format!(
        "[subagent {task_id}: hit its {max_rounds}-round ceiling; partial report \
         follows]\n\n{last_text}"
    ))
}

/// `Agent` 工具的执行入口：校验参数 → 派子代理 → 把报告包成模型能读的 `tool_result`。
fn run_agent_tool(
    cfg: &Cfg,
    tctx: &tools::Ctx,
    subagent_defs: &[Value],
    skill_list: &[skills::Skill],
    subagent_system: &str,
    ask_permission: bool,
    input: &Value,
    denied: Option<&str>,
) -> (String, bool) {
    if let Some(msg) = denied {
        return (format!("User denied this tool call: {msg}"), true);
    }
    let prompt = input
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if prompt.is_empty() {
        return (
            "缺少 prompt 参数：Agent 需要一个自包含的任务说明（子代理看不到本对话）".into(),
            true,
        );
    }
    let desc = input
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("subtask")
        .trim();
    let task_id = format!("task-{}", TASK_SEQ.fetch_add(1, Ordering::Relaxed) + 1);
    emit(json!({
        "type": "system", "subtype": "task_started",
        "task_id": task_id, "description": desc,
    }));
    log::info(format!("子代理 {task_id} 启动：{desc}"));

    let started = Instant::now();
    let out = run_subagent(
        cfg,
        tctx,
        &ForkSpec {
            task_id: &task_id,
            system: subagent_system,
            defs: subagent_defs,
            skills: skill_list,
            ask_permission,
            max_rounds: MAX_SUBAGENT_ROUNDS,
            budget_tokens: SUBAGENT_BUDGET_TOKENS,
            emit_progress: true,
        },
        // 子代理**不接 MCP 桥**（硬约束：桥是单线程 stdio 通道，主循环还持有它；
        // 且一接就是全量用户工具 = 不受控的副作用面）。它的工具集里也因此剔掉了
        // 依赖桥的那几件，见 `subagent_tool_defs`。
        None,
        prompt,
    );
    emit(json!({
        "type": "system", "subtype": "task_done",
        "task_id": task_id, "ok": out.is_ok(),
        "ms": started.elapsed().as_millis() as u64,
    }));
    let ms = started.elapsed().as_millis();
    match out {
        Ok(text) => {
            log::info(format!("子代理 {task_id} 完成（{ms}ms，报告 {} 字）", text.chars().count()));
            let clipped = crate::log::truncate_chars(&text, SUBAGENT_REPORT_CHARS);
            (
                format!("[{task_id}] subagent report:\n\n{clipped}"),
                false,
            )
        }
        Err(e) => {
            log::warn(format!("子代理 {task_id} 失败（{ms}ms）: {e}"));
            (format!("[{task_id}] subagent failed: {e}"), true)
        }
    }
}

// ── fork 技能的实现（A5，2026-09-20）────────────────────────────────
//
// 技能有两种模式（frontmatter 的 `context: fork`）：inline 把正文注入主对话让主循环照做，
// fork 则**派一个子代理去执行**、只把报告带回来。判据与执行路径都抄旧 CLI
// （`core/skills/loadSkillsDir.ts` 的 `executionContext`、`core/tools/SkillTool/SkillTool.ts`
// 的 `executeForkedSkill()`）。

/// fork 技能能不能跑。
///
/// **只读档 / 计划相位一律拒绝** —— 与 `Agent` 同理，而且理由更强：技能正文是**用户装的**、
/// 里面可以写任意 `Bash`，放它出去等于把「只读」这个承诺交给第三方的 md 文件去守。
/// （inline 技能不看这道闸：它只把 md 正文交回主循环，与 `Read` 同级。）
fn fork_skill_allows(tctx: &tools::Ctx) -> Result<(), String> {
    match tools::write_blocked(tctx, "fork skills") {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// 按 `allowed-tools` 从子代理工具集里挑出这次 fork 能用的工具。
///
/// **只在传入的 `subagent_defs` 里挑**，所以技能无法借白名单把 `Agent` / 走桥的 /
/// `mcp__*` 弄进来 —— 那三类已被 `subagent_tool_defs()` 剔除，**判据只有一处**。
/// 白名单为空 = 不限制；非空但一个都没匹配上 = **一件也不给**（fail-closed：白名单写错时
/// 宁可让子代理只靠推理，也不能悄悄放开成全集）。
fn fork_skill_tools(skill: &skills::Skill, subagent_defs: &[Value]) -> Vec<Value> {
    if skill.allowed_tools.is_empty() {
        return subagent_defs.to_vec();
    }
    subagent_defs
        .iter()
        .filter(|d| {
            let n = d.get("name").and_then(Value::as_str).unwrap_or("");
            skill.allowed_tools.iter().any(|w| w == n)
        })
        .cloned()
        .collect()
}

/// `Skill` 工具的 **fork 特判**：派子代理执行技能，返回它的报告。
///
/// 与 `run_agent_tool` 共用 `run_subagent()` + `ForkSpec` —— 这正是 A4 当初把八个差异收进
/// 结构体的回报：第三个调用方只多出「工具集来自技能自己」这一项。四点差异：
///   · 工具集 = `fork_skill_tools()`（技能 frontmatter 的 `allowed-tools`）；
///   · 任务 = **技能正文**（`$ARGUMENTS` 已替换），前置一句「照它做完并报告」；
///   · `emit_progress = true`：这是**用户/模型主动发起**的，进度必须回前端。
///     与 A4 的后台复盘相反 —— 那个是无人值守，发进度会让状态栏停在错的文案上；
///   · `ask_permission` 随前端运行方式：技能里写 `Bash` / `Write` 时，子代理内部**逐次**
///     再走 `can_use_tool`，**不是「批了技能就等于批了它要做的一切」**（同 `Agent`）。
fn run_forked_skill(
    cfg: &Cfg,
    tctx: &tools::Ctx,
    subagent_defs: &[Value],
    skill_list: &[skills::Skill],
    subagent_system: &str,
    ask_permission: bool,
    skill: &skills::Skill,
    input: &Value,
    denied: Option<&str>,
) -> (String, bool) {
    if let Some(msg) = denied {
        return (format!("User denied this tool call: {msg}"), true);
    }
    if let Err(e) = fork_skill_allows(tctx) {
        return (format!("[{e}]"), true);
    }

    let args = input.get("args").and_then(Value::as_str).unwrap_or("");
    // 子代理看不到主对话，也看不到主对话里那份技能清单 —— 自带的脚本 / 参考资料必须
    // 跟着任务说明一起给它，否则「技能写了要用 scripts/x.py」它只会去猜路径。
    let task = format!("{}{}", skills::instruction(skill, args), skills::resources_note(skill));
    let defs = fork_skill_tools(skill, subagent_defs);
    if !skill.allowed_tools.is_empty() && defs.is_empty() {
        log::warn(format!(
            "fork 技能 {} 的 allowed-tools（{}）在内置工具里一个都没匹配上 —— 子代理将无工具可用",
            skill.key,
            skill.allowed_tools.join(", ")
        ));
    }

    let task_id = format!("skill-{}", TASK_SEQ.fetch_add(1, Ordering::Relaxed) + 1);
    emit(json!({
        "type": "system", "subtype": "task_started",
        "task_id": task_id, "description": skill.key,
    }));
    log::info(format!(
        "fork 技能 {} 启动（工具 {} 件：{}）",
        skill.key,
        defs.len(),
        defs.iter()
            .filter_map(|d| d.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(",")
    ));

    let started = Instant::now();
    let out = run_subagent(
        cfg,
        tctx,
        &ForkSpec {
            task_id: &task_id,
            system: subagent_system,
            defs: &defs,
            // 子代理能读**其它技能**（组合很自然）；fork 技能的递归由 `skills::run()`
            // 的 fork 守卫挡住 —— 那里会明确回「不能在 inline 路径加载」。
            skills: skill_list,
            ask_permission,
            max_rounds: MAX_SUBAGENT_ROUNDS,
            budget_tokens: SUBAGENT_BUDGET_TOKENS,
            emit_progress: true,
        },
        None,
        &format!("Execute this skill end to end, then report the result.\n\n{task}"),
    );
    emit(json!({
        "type": "system", "subtype": "task_done",
        "task_id": task_id, "ok": out.is_ok(),
        "ms": started.elapsed().as_millis() as u64,
    }));
    let ms = started.elapsed().as_millis();
    match out {
        Ok(text) => {
            log::info(format!(
                "fork 技能 {} 完成（{ms}ms，报告 {} 字）",
                skill.key,
                text.chars().count()
            ));
            let clipped = crate::log::truncate_chars(&text, SUBAGENT_REPORT_CHARS);
            // 措辞要**与 inline 明确区分**：inline 回的是「照着做的指令」，
            // 这里回的是「已经做完了，这是结果」。含糊的话模型会把报告当指令再执行一遍。
            (
                format!(
                    "Skill \"{}\" ran in sub-agent [{task_id}] — the work is DONE; \
                     this is its report:\n\n{clipped}",
                    skill.key
                ),
                false,
            )
        }
        Err(e) => {
            log::warn(format!("fork 技能 {} 失败（{ms}ms）: {e}", skill.key));
            (
                format!("Skill \"{}\" failed in sub-agent [{task_id}]: {e}", skill.key),
                true,
            )
        }
    }
}

// ── 后台复盘 fork 的实现（A4）──────────────────────────────────────
//
// 装配在 `main()` 的提问循环里（门槛计数 + spawn/join），判断与落盘在这里。

/// 轮次门槛：`LUNAC_NUDGE_INTERVAL`（`0` = 关闭，其余为间隔），默认 `DEFAULT_NUDGE_INTERVAL`。
/// 非法值（不是数字）回落默认 —— 一个打错的开关不该让复盘彻底消失。
fn nudge_interval() -> u64 {
    std::env::var(NUDGE_INTERVAL_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_NUDGE_INTERVAL)
}

/// 第 `questions` 次提问结束后该不该复盘（纯函数，便于单测）。
/// `interval == 0` = 关；`questions == 0`（还没问过）永远不触发。
fn should_review(questions: u64, interval: u64) -> bool {
    interval > 0 && questions > 0 && questions % interval == 0
}

/// 把最近的对话压成给复盘看的快照：`角色: 文本` 若干行，**从最早处**丢到 `max_chars` 内。
///
/// 三条取舍：
///   · 工具调用只留一行摘要（名字 + 入参摘要），工具结果只留前 300 字 —— 复盘要判的是
///     「有没有值得长期留着的事实」，不是复核这一次干了什么；
///   · 从**最早**处丢：最近的几轮才是判据，开头那句「帮我改个 bug」对记忆没有价值；
///   · 只认 `text` / `tool_use` / `tool_result` 三类块，其余（thinking 等）不进去。
fn review_transcript(history: &[Value], max_chars: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    for msg in history {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("");
        let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
            continue;
        };
        for b in blocks {
            match b.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => {
                    let t = b.get("text").and_then(Value::as_str).unwrap_or("").trim();
                    if !t.is_empty() {
                        lines.push(format!("{role}: {t}"));
                    }
                }
                "tool_use" => {
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("tool");
                    let args = b
                        .get("input")
                        .map(tools::summarize_args)
                        .unwrap_or_default();
                    lines.push(format!("{role} → {name}({args})"));
                }
                "tool_result" => {
                    let t = b.get("content").and_then(Value::as_str).unwrap_or("").trim();
                    if !t.is_empty() {
                        lines.push(format!(
                            "tool result: {}",
                            crate::log::truncate_chars(t, 300)
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    let mut keep: Vec<&str> = Vec::new();
    let mut used = 0usize;
    for line in lines.iter().rev() {
        let n = line.chars().count() + 1;
        if used + n > max_chars {
            break;
        }
        used += n;
        keep.push(line);
    }
    keep.reverse();
    keep.join("\n")
}

/// 复盘的**用户消息**：当前记忆 + 对话快照 + 指令。
///
/// 「当前记忆」放进用户消息而不是系统提示词：系统提示词要**跨次复盘逐字节相同**
/// （同一种 fork 共享一段前缀，规则 18），而记忆每次都不同。
fn review_prompt(transcript: &str, memory: &str) -> String {
    let memory = memory.trim();
    let memory = if memory.is_empty() {
        "(nothing yet)"
    } else {
        memory
    };
    format!(
        "The conversation below just finished. Decide what, if anything, is worth keeping in \
         long-term memory, then act.\n\n\
         ## Long-term memory right now\n{memory}\n\n\
         ## The conversation (oldest first; tool calls and results are trimmed)\n{transcript}\n\n\
         ## Your task\nSave at most {REVIEW_MAX_ITEMS} durable facts with the `Remember` tool, \
         or reply exactly `NOTHING_TO_REMEMBER` if nothing qualifies."
    )
}

/// 一次复盘的输入。**全部在主线程备齐**（快照、配置副本、工具集、桥的 spec），
/// 线程里不再碰主循环的任何状态 —— 这也是为什么 `Cfg` 要在外面 `detached()` 一份。
struct ReviewJob {
    /// 独立副本：线程不能借 `&Cfg`（它含 `Cell`，不是 `Sync`）
    cfg: Cfg,
    tctx: tools::Ctx,
    defs: Vec<Value>,
    skills: Vec<skills::Skill>,
    system: String,
    ask_permission: bool,
    /// 复盘**自己那条**桥的 spec（`--mcp-server` 的原样值）。`None` = 没有桥，
    /// 那 `Remember` 也不在 `defs` 里（启动时就没注册），复盘只剩「判断」这一半。
    bridge_spec: Option<String>,
    task_id: String,
    prompt: String,
}

/// 复盘线程体：连自己那条桥 → 跑 `run_subagent` 的骨架 → 返回它自己那行结论。
fn run_review_fork(job: ReviewJob) -> Result<String, String> {
    // **为什么复盘要自己连一条桥**：主循环那条 `&mut Bridge` 归主线程所有，跨线程借用
    // 做不到；而复盘要写的 `Remember` 走的是桥上的 `lunac/memory_write` —— 那是长期记忆
    // **唯一**的写入通道（不给 agent 侧另开一条私有文件通道，避免两套真相源）。
    // 代价：多起一个 `lunac.exe --mcp-server` 子进程（握完手就退出），只在每 N 轮一次。
    let mut bridge = match job.bridge_spec.as_deref() {
        Some(spec) => match mcp::Bridge::connect(spec, &[]) {
            Ok(b) => Some(b),
            Err(e) => {
                log::warn(format!("复盘 fork 的 MCP 桥没连上（只剩判断，写不进记忆）: {e}"));
                None
            }
        },
        None => None,
    };
    run_subagent(
        &job.cfg,
        &job.tctx,
        &ForkSpec {
            task_id: &job.task_id,
            system: &job.system,
            defs: &job.defs,
            skills: &job.skills,
            ask_permission: job.ask_permission,
            max_rounds: MAX_REVIEW_ROUNDS,
            budget_tokens: REVIEW_BUDGET_TOKENS,
            // 不回前端进度：复盘在提问之间跑，前端没有恢复状态栏的时机（见字段注释）
            emit_progress: false,
        },
        bridge.as_mut(),
        &job.prompt,
    )
}

/// 复盘 fork 的**启动期常量部分**：工具集 / 技能 / 系统提示词 / 审批开关 / 桥的 spec /
/// 门槛间隔。每一样都在启动时算好后复用 —— 提问循环里只补「这一次的工作面」
/// （历史快照、当前记忆、配置副本）。
struct ReviewSetup {
    defs: Vec<Value>,
    skills: Vec<skills::Skill>,
    system: String,
    ask_permission: bool,
    /// `--mcp-server` 的原样值：复盘用它连**自己那条**桥（见 `run_review_fork`）
    bridge_spec: Option<String>,
    /// 轮次门槛（提问数）；`0` = 关闭
    interval: u64,
}

impl ReviewSetup {
    /// 装配一次复盘并派到后台线程。`None` = 这次没派（配置副本建不出来）。
    ///
    /// `memory` 由调用方在主线程读好（`fetch_memory`）—— 线程不碰主循环那条桥。
    fn spawn(
        &self,
        cfg: &Cfg,
        tctx: &tools::Ctx,
        history: &[Value],
        memory: &str,
        seq: u64,
    ) -> Option<thread::JoinHandle<Result<String, String>>> {
        let cfg = match cfg.detached() {
            Ok(c) => c,
            Err(e) => {
                log::warn(format!("后台复盘跳过：{e}"));
                return None;
            }
        };
        let transcript = review_transcript(history, REVIEW_TRANSCRIPT_CHARS);
        if transcript.trim().is_empty() {
            log::info("后台复盘跳过：对话快照为空");
            return None;
        }
        let job = ReviewJob {
            cfg,
            tctx: tctx.clone(),
            defs: self.defs.clone(),
            skills: self.skills.clone(),
            system: self.system.clone(),
            ask_permission: self.ask_permission,
            bridge_spec: self.bridge_spec.clone(),
            task_id: format!("review-{seq}"),
            prompt: review_prompt(&transcript, memory),
        };
        log::info(format!("后台复盘 {} 启动（快照 {} 字）", job.task_id, transcript.chars().count()));
        Some(thread::spawn(move || run_review_fork(job)))
    }
}

/// 收一次后台复盘：结论只进日志 —— 它的**产物是记忆文件本身**，不往主对话里塞任何东西
/// （往主对话塞就等于「复盘改了用户看得见的上下文」，与「后台」这个前提冲突）。
fn join_review(handle: thread::JoinHandle<Result<String, String>>) {
    match handle.join() {
        Ok(Ok(text)) => {
            let head = text.trim().lines().next().unwrap_or("").trim().to_string();
            log::info(format!(
                "后台复盘完成：{}",
                crate::log::truncate_chars(&head, 200)
            ));
        }
        Ok(Err(e)) => log::warn(format!("后台复盘失败：{e}")),
        Err(_) => log::warn("后台复盘线程 panic（已忽略，不影响主对话）"),
    }
}

/// 该收就收：**跑完了**才 join，还在跑就原样交还。
///
/// 为什么不无条件 join：`REQUEST_TIMEOUT_SECS` 是 30 分钟，一次卡住的复盘会把用户的
/// **下一次提问**一起卡在 `join` 上。而复盘是后台事务，没道理让它决定前台时延 ——
/// 用户该问就问，复盘在背后写完自己结束（它的写盘与主对话没有共享状态：
/// `ReviewJob` 全是克隆出来的）。不 join 也不会有两条复盘并发：`pending` 非空就不再派。
fn collect_review(
    pending: &mut Option<thread::JoinHandle<Result<String, String>>>,
) -> bool {
    let Some(h) = pending.as_ref() else {
        return false;
    };
    if !h.is_finished() {
        return false;
    }
    let h = pending.take().expect("上面刚判过是 Some");
    join_review(h);
    true
}

// ── 计划相位（A7）：两件工具的**回执**（给模型看）──────────────────
//
// 为什么这两件工具的状态机写在 `main.rs` 而不是 `tools.rs`：状态本身在 `tools::Ctx`
// 里（这样写类判据只收口一次，见 `tools::write_blocked`），但**进 / 出的时机与
// 「通知前端」是主循环的事** —— `tools.rs` 是纯工具实现层，拿不到 `emit`。
//
// 免责声明给模型的那一段（「计划格式」）只在 schema 描述里写了一次，这里不重复
// 抄一遍格式要求：重复的格式说明会在上下文里出现两份，改一份忘一份。
const PLAN_MODE_ON_NOTE: &str = "Plan mode is ON — this lasts until the user approves a plan. \
Read-class tools still work; every write-class tool (Write / Edit / Bash / PowerShell / Agent / \
MCP tools) is REFUSED until then. Gather what you need with Read / Glob / Grep, then present the \
COMPLETE plan with ExitPlanMode.";

const PLAN_MODE_OFF_NOTE: &str = "The user approved your plan. Plan mode is OFF — write-class \
tools work again. Execute the plan task by task in the order you wrote it, and keep the user \
posted with TodoWrite. If reality contradicts the plan (a file is not where you expected), say \
so instead of improvising silently.";

fn dispatch_tool(
    tctx: &tools::Ctx,
    mcp_bridge: Option<&mut mcp::Bridge>,
    skill_list: &[skills::Skill],
    name: &str,
    input: &Value,
) -> Result<String, String> {
    // 结果**不在这里**截断 —— 单条输出预算统一由 run_tool 的
    // `tools::apply_budget` 收口（那里才知道工具名，超长要落盘）。

    // 计划相位的入口（A7）：只置标志 + 通知前端，**没有本机副作用**，因此免审批。
    // 幂等（模型重复调用无妨）：置真就是「置真」，没有计数要维护。
    if name == "EnterPlanMode" {
        tctx.plan_phase.store(true, Ordering::Relaxed);
        emit(json!({
            "type": "system",
            "subtype": "plan_mode",
            "state": "on",
            "reason": input.get("reason").and_then(Value::as_str).unwrap_or("").trim(),
        }));
        return Ok(PLAN_MODE_ON_NOTE.into());
    }
    // 计划相位的出口（A7）：走到这里说明审批卡已经**批准**过（被拒的调用根本到不了
    // 这一层 —— `run_one_tool` 的 `denied` 早退把它变成了 is_error 的 tool_result）。
    if name == "ExitPlanMode" {
        // 只读档下**退出计划模式没有意义**：写类工具被用户档位永久拒绝，批准了计划也
        // 执行不了 —— 说「批准后就能写」是把模型引到一个永远失败的动作上。
        // 所以直接拒，并让它改用正文交代计划（用户仍能读到计划，只是不能在这里批准）。
        if tctx.read_only {
            return Err(
                "ExitPlanMode is disabled in read-only (plan) mode — the user's security profile \
                 forbids writes, so approving a plan could not enable anything. Put the plan in \
                 your reply as text instead; to have it executed the user must switch the profile \
                 to \"project\" in Lunac settings."
                    .into(),
            );
        }
        tctx.plan_phase.store(false, Ordering::Relaxed);
        emit(json!({ "type": "system", "subtype": "plan_mode", "state": "off" }));
        return Ok(PLAN_MODE_OFF_NOTE.into());
    }
    if name == "Skill" {
        return skills::run(skill_list, input);
    }
    // 往期会话检索（A2）：走 MCP 桥的**自定义方法**，因此要在 `is_mcp` 分支之前处理
    // （它不带 `mcp__` 前缀，也不会被 `needs_approval` 判成需审批）。
    if name == "SessionSearch" {
        return session_search(mcp_bridge, input);
    }
    // 走桥的工具（A3/A4）：同样走自定义方法 / 标准方法，所以也要在 `is_mcp` 之前处理。
    // **读类在 plan（只读）档放行** —— 与 `SessionSearch` 同理：只读本机自己的数据
    // （用户的工具定义文件），不是「动手工具」；**写类的 `Remember` 则必须自己拒**
    // （见下面的分支：这条早退绕过了 `tools::run` 的只读拦截）。
    if tools::needs_bridge(name) {
        let Some(bridge) = mcp_bridge else {
            return Err(format!("MCP bridge is not connected, cannot call {name}"));
        };
        return match name {
            "ListMcpResourcesTool" => bridge.list_resources(),
            "ReadMcpResourceTool" => {
                let uri = input
                    .get("uri")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if uri.is_empty() {
                    Err("缺少 uri 参数：先调 ListMcpResourcesTool 拿一个 uri".into())
                } else {
                    bridge.read_resource(uri)
                }
            }
            // 长期记忆的写入侧（A4）。**写类判据必须在这里补一次** —— 它是写操作，
            // 但上面那条 `needs_bridge` 早退把 `tools::run` 里的拦截绕过去了，
            // 不在这一层补，只读档 / 计划相位就能改长期记忆（2026-09-20 A7 起共用
            // `tools::write_blocked`，两档的措辞自动分开）。
            "Remember" => {
                if let Some(why) = tools::write_blocked(tctx, "Remember") {
                    Err(why)
                } else {
                    let content = input
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim();
                    if content.is_empty() {
                        Err("缺少 content 参数：要记的内容不能为空".into())
                    } else {
                        let replace = input
                            .get("replace")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        bridge.memory_write(content, replace)
                    }
                }
            }
            // 显式兜底（**不要**写成 `_ => read_resource`）：`BRIDGE_TOOLS` 里新加一件却忘了
            // 在这里接线时，宁可回一句「实现缺失」，也不要静默地按 read_resource 去执行。
            other => Err(format!(
                "{other} 的桥实现缺失：tools::BRIDGE_TOOLS 与 dispatch_tool 不同步"
            )),
        };
    }
    if !mcp::is_mcp(name) {
        return tools::run(tctx, name, input);
    }
    // 写类档位判据（只读档 / 计划相位）：MCP 工具同样不许动手。
    // 判据与内置写类共用一处（`tools::write_blocked`），措辞自动分档。
    if let Some(why) = tools::write_blocked(tctx, "MCP tools") {
        return Err(why);
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
    tool_use_id: &str,
    name: &str,
    run_input: &Value,
    denied: Option<&str>,
) -> (String, bool) {
    if let Some(msg) = denied {
        return (format!("Error: {msg}"), true);
    }
    // 2026-09-19 日志精简：这里原来有一行 `eprintln!("[agent] 执行工具 {name}")`。
    // 它是**纯重复** —— 同一件事在 `run_tool()` 里已经由 `tool {name} ok (…ms, out … chars)
    // args=…` 记全了（连耗时和参数摘要都有），而它走 eprintln ⇒ 宿主把它以 **warn**
    // 级再镜像一遍。实测这一行 + `等待审批` 合计占 dev 日志行数的三分之一。
    let (mut text, is_error) = match run_tool(tctx, mcp_bridge, skill_list, name, run_input) {
        Ok(s) => (s, false),
        Err(e) => (format!("Error: {e}"), true),
    };

    // ── PostToolUse hook（A9）──
    // 工具**已经跑完**，所以这里没有「拦」这回事：hook 的 stdout（以及退出码 2 的反馈）
    // 一律**拼进 `tool_result` 的文本内部**回给模型 —— content 数组的形状与块数量都不变
    // （规则 23），前端也照常能从 `system/hook_note` 看到。子代理与 fork 技能走的也是
    // 这里，所以它们的工具调用同样有 PostToolUse。
    let h = hooks::current();
    if !h.is_empty() {
        let mut payload =
            hook_tool_payload("PostToolUse", &tctx.cwd, name, tool_use_id, run_input);
        // 回灌文本可能很长（已由 apply_budget 截断过），这里再取一段给 hook：
        // hook 关心的是「发生了什么」，不必看全文。
        let brief: String = text.chars().take(4000).collect();
        payload["tool_response"] = json!({ "is_error": is_error, "text": brief });
        let run = hooks::fire(&h, "PostToolUse", &payload);
        report_hook_run("PostToolUse", Some(name), &run);
        if let Some(note) = run.model_note() {
            text = format!("{text}\n\n[PostToolUse hook]\n{note}");
        }
    }
    (text, is_error)
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

/// 子代理的工具集：从本轮的 `tool_defs` 里**剔掉三类它用不了的**。
///
/// | 剔掉的 | 为什么 |
/// |---|---|
/// | `Agent` | 防无限递归（约束⑤）—— 子代理不能再派子代理 |
/// | `EnterPlanMode` / `ExitPlanMode`（A7） | 计划相位是**主循环**的状态。子代理**问不了用户**（它的提示词就是这么写的），`ExitPlanMode` 在里面只会弹一张无人能负责的卡；而 `EnterPlanMode` 若被子代理调用，等于它替主代理改了全局相位 —— 一个只该由主循环做的决定 |
/// | 走 MCP 桥的（`tools::needs_bridge`） | 子代理**不接 MCP 桥**（`run_one_tool(…, None, …)`：桥是 `&mut` 单线程 stdio 通道，主循环还持有它）。留在工具表里就是**保证失败**：`mcp__*` 的 `needs_approval` 恒真 ⇒ 会先弹一张注定白问的审批卡，然后回 `MCP bridge is not connected`；`SessionSearch` / resources 读侧同样无桥可用 |
///
/// 桥那一类走 `tools::BRIDGE_TOOLS` 这个**唯一真相源**，不在这里另写字符串 ——
/// 2026-09-20 复查发现原实现只剔了 `Agent`，与工具循环里「子代理的工具集里没有 `mcp__*`」
/// 那句注释不符；集中到一处后就不会再各改各的。守门单测 `subagent_tool_defs_…` 钉住。
fn subagent_tool_defs(tool_defs: &[Value]) -> Vec<Value> {
    tool_defs
        .iter()
        .filter(|d| {
            let name = d.get("name").and_then(Value::as_str).unwrap_or("");
            !matches!(name, "Agent" | "EnterPlanMode" | "ExitPlanMode")
                && !tools::needs_bridge(name)
                && !mcp::is_mcp(name)
        })
        .cloned()
        .collect()
}

/// 审批判据的**带输入版本**（2026-09-20 A5）。
///
/// `tools::needs_approval()` 只看名字，而 `Skill` 的两种模式副作用完全不同：
///   · inline 技能是**纯读**（把 md 正文交回主循环，与 `Read` 同级）⇒ 不该弹卡；
///   · fork 技能会派一个**能写文件、能跑命令**的子代理 ⇒ 与 `Agent` 同理，
///     「派一个代理出去干活」这个决定本身值得确认。
/// 判据差别藏在输入里（`skill` 指向哪个技能），所以只能在这一层补 —— 不要为此把
/// `Skill` 整个塞进 `tools::needs_approval()`，那会让 inline 技能每次都白弹一张卡。
///
/// 前一半走本文件的 `needs_approval()`（内置写类 + MCP），**不要**直接调
/// `tools::needs_approval()` —— 那会漏掉「全部 MCP 工具一律问」这条。
fn needs_approval_with(name: &str, input: &Value, skills: &[skills::Skill]) -> bool {
    if needs_approval(name) {
        return true;
    }
    if name == "Skill" {
        let want = input.get("skill").and_then(Value::as_str).unwrap_or("");
        return matches!(skills::find(skills, want), Some(s) if s.fork);
    }
    false
}

fn run_query(
    cfg: &Cfg,
    history: &mut Vec<Value>,
    prompt: &str,
    raw_images: &[Value],
    tctx: &tools::Ctx,
    tool_defs: &[Value],
    tool_names: &[String],
    ask_permission: bool,
    mut mcp_bridge: Option<&mut mcp::Bridge>,
    skill_list: &[skills::Skill],
    system_prompt: &str,
    subagent_system: &str,
) {
    let started = Instant::now();
    emit_init(&cfg.model, tool_names);

    // 图片附件（A8）：先按路径解析成 base64 块，再在 init 之后**如实上报**没能发出的那些
    // （放在 init 之后是因为前端的状态机要由 init 推进到本轮，提示行才有地方落）。
    let (image_blocks, image_notes) = collect_image_blocks(raw_images);
    if !image_notes.is_empty() {
        for n in &image_notes {
            log::warn(format!(
                "附件未随本轮发送: {}（{}）",
                n.get("path").and_then(Value::as_str).unwrap_or("?"),
                n.get("reason").and_then(Value::as_str).unwrap_or("?"),
            ));
        }
        emit(json!({ "type": "system", "subtype": "attachment_note", "skipped": image_notes }));
    }
    if !image_blocks.is_empty() {
        let b64_kb: usize = image_blocks
            .iter()
            .map(|b| {
                b.pointer("/source/data")
                    .and_then(Value::as_str)
                    .map(str::len)
                    .unwrap_or(0)
            })
            .sum::<usize>()
            .div_ceil(1024);
        log::info(format!(
            "图片附件 {} 张（base64 合计约 {} KB）随本轮提问发出",
            image_blocks.len(),
            b64_kb
        ));
    }

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
    // 文本在前、图片按原顺序在后（端点侧图文顺序有语义；且 history 第一条**必须**是
    // 文本块 —— 前缀缓存与前端的标题推导都吃这条不变量，见 §11 规则 18 ①）。
    let mut user_content: Vec<Value> = vec![json!({ "type": "text", "text": prompt })];
    user_content.extend(image_blocks);
    history.push(json!({
        "role": "user",
        "content": user_content,
    }));

    let budget = max_context_tokens();

    // 子代理的工具集：剔掉 `Agent`（防递归）与两件**必须有 MCP 桥才能跑**的工具。
    // 在循环外算一次：它被所有子代理复用，因此每个子代理的 `tools` 数组逐字节一致
    // —— 多个子代理之间于是能共享端点侧的 prompt cache（它们是各自独立的会话，
    // 但前缀同形）。
    let subagent_defs: Vec<Value> = subagent_tool_defs(tool_defs);

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
    // ── B 类崩塌归因（2026-09-20）：history 的**逐条**指纹 ──────────────
    // 整体 hash 只能回答「history 变没变」；而 history 每轮必然变（只追加也会变），
    // 所以它对「本侧有没有就地改写」**没有分辨力**。实测三条记录里，第 17 轮的
    // `read` 从 48000 掉到 8448，而同轮 history 只多了 3 条 / 528 字 —— 此时整体
    // hash 变了、字数也变了，按旧埋点完全无法定性是本侧改写还是端点侧淘汰。
    // 逐条指纹给出判据：`公共前缀/上轮条数`
    //   · 等于上轮条数 ⇒ 纯追加，本侧无责（read 掉了就是端点侧）；
    //   · 小于上轮条数 ⇒ **本侧就地改写了历史**，改在第 N 条，其后整段缓存必然失效。
    let mut prev_hist_hashes: Vec<u64> = Vec::new();
    // 每次请求前算出的「与上一轮 history 逐字节相同的条数」，按请求顺序累积
    // （`message_stop` 时取最后一条随用量落盘）。用 Vec 而不是标量：一来避开
    // 「赋初值后必被覆盖」的 unused_assignments 警告，二来重试路径（send 前
    // `continue` 回来会再算一次）天然留下多条，正好能看出是哪次尝试断的。
    let mut common_log: Vec<usize> = Vec::new();
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
                // PreCompact hook（A9）：压缩**之前**发，让用户脚本能记录「这次压了什么水位」
                fire_plain_hook("PreCompact", &tctx.cwd, json!({ "trigger": "drop" }));
                let out = compact_history(history, Compact::Drop, measured);
                base = base.saturating_sub(out.dropped) + out.pinned;
                // 摘要压缩（backlog §8.2）：丢弃档是它**唯一**的常规触发点 ——
                // 这一档本来就必然会 drain 并废掉缓存，多留一段摘要是净赚。
                // 它又插了一条合成消息 ⇒ 回滚锚点再 +1（见 CompactOutcome::pinned）。
                if pin_summary_of_dropped(cfg, history, &out.dropped_msgs) {
                    base += 1;
                }
                cfg.last_input.set(0);
                cfg.last_compact.set(measured);
            } else if ratio > ELIDE_RATIO && grew_enough {
                let out = compact_history(history, Compact::Elide, measured);
                base = base.saturating_sub(out.dropped) + out.pinned;
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
                // 逐条指纹 → 与上一轮的公共前缀条数（B 类归因的判据，见变量声明处注释）
                let cur_hashes: Vec<u64> = history
                    .iter()
                    .map(|m| log::hash64(&serde_json::to_string(m).unwrap_or_default()))
                    .collect();
                let cur_common = prev_hist_hashes
                    .iter()
                    .zip(cur_hashes.iter())
                    .take_while(|(a, b)| a == b)
                    .count();
                let prev_len = prev_hist_hashes.len();
                prev_hist_hashes = cur_hashes;
                common_log.push(cur_common);
                log::info(format!(
                    "请求前缀 #{} system={:016x}/{}字 tools={:016x}/{}字 history={:016x}/{}条/{}字 公共前缀={}/{}条{}",
                    turns,
                    log::hash64(system_prompt),
                    system_prompt.chars().count(),
                    log::hash64(&tools_json),
                    tools_json.chars().count(),
                    log::hash64(&hist_json),
                    history.len(),
                    hist_json.chars().count(),
                    cur_common,
                    prev_len,
                    if prev_len > 0 && cur_common < prev_len {
                        " ← 本侧就地改写了历史（其后整段前缀缓存必然失效）"
                    } else {
                        ""
                    },
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
                fire_plain_hook("PreCompact", &tctx.cwd, json!({ "trigger": "force" }));
                let out = compact_history(history, Compact::Force, cfg.last_input.get());
                base = base.saturating_sub(out.dropped) + out.pinned;
                // 兜底档也做摘要 —— 这一档丢弃量最大（连尾部都瘦），摘要在此时最值钱
                if pin_summary_of_dropped(cfg, history, &out.dropped_msgs) {
                    base += 1;
                }
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
                    // 逐请求命中率 + 本侧是否改写前缀：两截证据落在同一行，离线即可归因，
                    // 不必再去前端 usage-*.jsonl 里对齐（2026-09-20，规则 23 的归因埋点）
                    let ctx = cur_in + cur_read + cur_create;
                    log::info(format!(
                        "请求用量 #{} in={} read={} create={} out={} 命中率={:.1}% 公共前缀={}条",
                        req_log.len(),
                        cur_in,
                        cur_read,
                        cur_create,
                        cur_out,
                        if ctx > 0 {
                            cur_read as f64 / ctx as f64 * 100.0
                        } else {
                            0.0
                        },
                        common_log.last().copied().unwrap_or(0),
                    ));
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
        // ── PreToolUse hooks（A9）──
        // 这一遍覆盖**全部**工具调用（不只是要审批的那些）：hook 的 deny 对只读工具
        // 同样有效。它与工作区锁、审批卡是「与」关系，谁都不能把谁抵掉。
        let hook_gates: Vec<HookGate> = calls
            .iter()
            .map(|(id, name, input)| hook_tool_gate("PreToolUse", &tctx.cwd, name, id, input))
            .collect();

        let mut pendings: Vec<Option<Pending>> = Vec::with_capacity(calls.len());
        let mut hook_denied: Vec<Option<String>> = Vec::with_capacity(calls.len());
        for ((id, name, input), gate) in calls.iter().zip(hook_gates) {
            let mut pending = None;
            let mut denied = None;
            match gate {
                HookGate::Deny(r) => denied = Some(format!("Denied by PreToolUse hook: {r}")),
                _ => {
                    // hook 放行 ⇒ 只剩「安全分析命中」这一种情况还能把卡叫回来
                    let allowed = matches!(gate, HookGate::Allow)
                        && !hook_allow_needs_card(name, input);
                    let ask = !allowed
                        && ask_permission
                        && needs_approval_with(name, input, skill_list)
                        && (!tctx.read_only || tools::gated_in_read_only(name));
                    if ask {
                        // PermissionRequest（A9）：只在「本来就要弹卡」的这一刻问 ——
                        // 它能替用户答这一问（allow = 免卡，deny = 直接拒）
                        match hook_tool_gate("PermissionRequest", &tctx.cwd, name, id, input) {
                            HookGate::Deny(r) => {
                                denied = Some(format!("Denied by PermissionRequest hook: {r}"))
                            }
                            HookGate::Allow if !hook_allow_needs_card(name, input) => {}
                            _ => pending = Some(open_approval(name, id, input)),
                        }
                    }
                }
            }
            pendings.push(pending);
            hook_denied.push(denied);
        }

        // 先把审批**按原顺序**全部解完（审批是阻塞等用户，且顺序不能乱：前端按
        // 「未应答行」合并同一批命令，乱序会打乱卡片与合并结果）。
        let mut interrupted = false;
        let mut run_inputs: Vec<Value> = Vec::with_capacity(calls.len());
        let mut denieds: Vec<Option<String>> = Vec::with_capacity(calls.len());
        for (i, ((_id, _name, input), pending)) in calls.iter().zip(pendings.into_iter()).enumerate() {
            let mut run_input = input.clone();
            // hook 的拒绝先落座；审批卡只可能再补一个拒绝，不可能把它翻回来
            let mut denied: Option<String> = hook_denied[i].clone();
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
                // `Agent` 特判：它要**发自己的 API 请求**并跑独立循环，因此需要 `cfg`，
                // 而 `dispatch_tool` 拿不到 —— 只能在这一层接。它也**不接 MCP 桥**
                // （桥是 `&mut` 单线程 stdio 通道，主循环还持有它；子代理的工具集里
                // 因此已经剔掉了 `mcp__*` 与 `SessionSearch`，见 `subagent_tool_defs`）。
                //
                // `Skill` 的 **fork 模式同族**（2026-09-20 A5）：它同样要发 API、跑独立
                // 循环，所以只能在这里接。**inline 技能不走这条路** —— 它只是把 md 正文
                // 交回主循环（与 `Read` 同级），照常落进下面的 `run_one_tool`。
                let forked = if calls[i].1 == "Skill" {
                    let want = run_inputs[i]
                        .get("skill")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    skills::find(skill_list, want).filter(|s| s.fork)
                } else {
                    None
                };
                slots[i] = Some(if let Some(sk) = forked {
                    run_forked_skill(
                        cfg,
                        tctx,
                        &subagent_defs,
                        skill_list,
                        subagent_system,
                        ask_permission,
                        sk,
                        &run_inputs[i],
                        denieds[i].as_deref(),
                    )
                } else if calls[i].1 == "Agent" {
                    run_agent_tool(
                        cfg,
                        tctx,
                        &subagent_defs,
                        skill_list,
                        subagent_system,
                        ask_permission,
                        &run_inputs[i],
                        denieds[i].as_deref(),
                    )
                } else {
                    run_one_tool(
                        tctx,
                        mcp_bridge.as_deref_mut(),
                        skill_list,
                        &calls[i].0,
                        &calls[i].1,
                        &run_inputs[i],
                        denieds[i].as_deref(),
                    )
                });
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
                                        &calls_ref[i].0,
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
            // 给模型一次「收口」的机会：只出文本，不再调工具。
            // 落点见 `TOOL_BUDGET_HINT` 的注释 —— 拼进**上一条 `tool_result` 的文本内部**，
            // content 数组的形状与块数量都不变。第 17 轮照常发出（这一轮 `hint_sent`
            // 是刚置上的，`break` 要等下一轮进来才命中），模型因此仍有机会收口。
            hint_sent = true;
            if let Some(arr) = history
                .last_mut()
                .and_then(|m| m.get_mut("content"))
                .and_then(Value::as_array_mut)
            {
                if let Some(tr) = arr
                    .iter_mut()
                    .find(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
                {
                    if let Some(prev) = tr.get("content").and_then(Value::as_str) {
                        tr["content"] = json!(format!("{prev}\n\n{TOOL_BUDGET_HINT}"));
                    }
                }
            }
        }
    }

    emit(json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "duration_ms": started.elapsed().as_millis() as u64,
        "num_turns": turns,
        "result": final_text,
        "session_id": session_id(),
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

    // ── Stop hook（A9）──
    // 只在**成功收尾**时发（出错走 finish_error，本轮是被放弃的，不该报「回答完成」）。
    // 它拦不住任何东西（回答已经产出），用途是用户脚本的收尾动作：统计、通知、清理。
    fire_plain_hook("Stop", &tctx.cwd, json!({}));
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
        "session_id": session_id(),
        "total_cost_usd": 0.0,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 计划相位 / 档位判据的单测用最小上下文（与 `tools.rs` 里那份同形）：
    /// 只关心两个标志，不碰磁盘。
    fn test_ctx() -> tools::Ctx {
        tools::Ctx {
            cwd: PathBuf::from("."),
            add_dirs: Vec::new(),
            read_only: false,
            locked: false,
            plan_phase: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 会话 id（A11）：形态 `sess_<pid>_<毫秒>`，且**同进程内恒定**。
    ///
    /// 恒定这件事是硬要求：它要能和前端用量记录、agent 落盘日志对上「同一次运行」——
    /// 若每次取值都新生成（比如误用 `now()` 当场拼），那这些记录就永远对不上，
    /// 比用 `""` 还糟（看起来有值、实际无意义）。
    #[test]
    fn session_id_is_real_and_stable() {
        let id = session_id();
        assert!(id.starts_with("sess_"), "{id}");
        let rest = id.trim_start_matches("sess_");
        let (pid, ms) = rest.split_once('_').expect("形态必须是 sess_<pid>_<毫秒>");
        assert_eq!(pid.parse::<u32>().unwrap(), std::process::id());
        assert!(ms.parse::<u128>().unwrap() > 1_600_000_000_000, "毫秒时间戳不合理：{ms}");
        // 第二次取值必须逐字节相同
        assert_eq!(id, session_id());
    }

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

    /// 子代理工具集的守门测试（2026-09-20 复查补，A7 扩了一类）。
    ///
    /// 原实现只剔了 `Agent`，把 `mcp__*` 与 `SessionSearch` 一起发给了子代理 —— 而子代理
    /// **不接 MCP 桥**，那两件事在里面必然失败（`mcp__*` 还会先弹一张注定白问的审批卡）。
    /// 这条测试钉住「剔哪三类、留哪些、顺序不变」。
    #[test]
    fn subagent_tool_defs_drops_agent_and_bridge_only_tools() {
        // 模拟主对话的工具池：内置（含 SessionSearch 与计划相位两件）+ 条件注册的
        // Skill / Remember + 两个 MCP 用户工具
        let mut defs = tools::defs(&[]);
        defs.push(skills::tool_def());
        defs.push(tools::remember_tool());
        defs.push(json!({
            "name": "mcp__deploy", "description": "x",
            "input_schema": { "type": "object", "properties": {} },
        }));
        defs.push(json!({
            "name": "mcp__notify", "description": "x",
            "input_schema": { "type": "object", "properties": {} },
        }));

        let names: Vec<String> = subagent_tool_defs(&defs)
            .iter()
            .filter_map(|d| d.get("name").and_then(Value::as_str).map(String::from))
            .collect();

        assert!(!names.contains(&"Agent".to_string()), "防递归：子代理不能再派子代理");
        assert!(
            !names.contains(&"SessionSearch".to_string()),
            "它走 MCP 桥，子代理没有桥"
        );
        assert!(
            !names.contains(&"Remember".to_string()),
            "它走 MCP 桥（`lunac/memory_write`），子代理没有桥"
        );
        for plan_tool in ["EnterPlanMode", "ExitPlanMode"] {
            assert!(
                !names.contains(&plan_tool.to_string()),
                "{plan_tool} 改的是主循环的全局相位，且子代理问不了用户"
            );
        }
        assert!(
            !names.iter().any(|n| n.starts_with("mcp__")),
            "MCP 工具在子代理里必然失败（无桥 + 白问一次审批）"
        );
        assert!(
            names.contains(&"Read".to_string()) && names.contains(&"Write".to_string()),
            "内置工具必须原样保留"
        );
        assert!(names.contains(&"Skill".to_string()), "技能是纯本地能力，不依赖 MCP 桥");

        // 顺序即前缀（ai-spec §11 规则 18）：剔除后必须保持原有相对顺序
        // 过滤条件用 `tools::needs_bridge()` 而不是再抄一遍工具名 —— 走桥的清单只有
        // `BRIDGE_TOOLS` 一处真相源，抄一遍就等于给「以后新增走桥工具」留了个漏改点。
        let expected: Vec<String> = defs
            .iter()
            .filter_map(|d| d.get("name").and_then(Value::as_str).map(String::from))
            .filter(|n| {
                !matches!(n.as_str(), "Agent" | "EnterPlanMode" | "ExitPlanMode")
                    && !tools::needs_bridge(n)
                    && !n.starts_with("mcp__")
            })
            .collect();
        assert_eq!(names, expected, "剔除不得改变工具顺序");
    }

    /// 计划相位的状态机（A7）：只有两件工具能翻转它，且**出口只认批准**。
    ///
    /// 「被拒」那条路径不在这里测 —— 它压根到不了 `dispatch_tool`：`run_one_tool` 见到
    /// `denied` 就直接回错误文本（见那两处的注释与 `await_approval`）。
    #[test]
    fn plan_phase_flips_only_through_the_two_tools() {
        let ctx = test_ctx();
        let plan = json!({"plan": "# 实现计划\n\n### Task 1: …"});

        // 出口在没进过计划相位时也能用（幂等：置假就是置假）
        assert!(!ctx.plan_phase.load(Ordering::Relaxed));
        assert!(dispatch_tool(&ctx, None, &[], "ExitPlanMode", &plan).is_ok());
        assert!(!ctx.plan_phase.load(Ordering::Relaxed));

        // 入口：免审批，且立刻生效
        let note = dispatch_tool(&ctx, None, &[], "EnterPlanMode", &json!({"reason": "任务较大"}))
            .expect("进入计划模式不该失败");
        assert!(ctx.plan_phase.load(Ordering::Relaxed), "EnterPlanMode 必须立刻生效");
        assert!(
            note.contains("ExitPlanMode"),
            "回执要告诉模型怎么出去，实际是：{note}"
        );

        // 计划相位里写类一律被拒，且拒因指向「出计划、等批准」
        for (n, args) in [
            ("Write", json!({"file_path": "a.txt", "content": "x"})),
            ("Edit", json!({"file_path": "a.txt", "old_string": "a", "new_string": "b"})),
            ("Bash", json!({"command": "echo hi"})),
            ("PowerShell", json!({"command": "echo hi"})),
        ] {
            let err = dispatch_tool(&ctx, None, &[], n, &args).expect_err("计划相位必须拦住写类");
            assert!(
                err.contains("ExitPlanMode"),
                "{n} 的拒因该指向 ExitPlanMode（用户该做的是批准，不是改设置），实际是：{err}"
            );
        }
        // 免审批那些照常可用（否则「先看代码再出计划」就做不成了）
        assert!(dispatch_tool(&ctx, None, &[], "Glob", &json!({"pattern": "*"})).is_ok());

        // 出口：批准后解除
        let note = dispatch_tool(&ctx, None, &[], "ExitPlanMode", &plan).expect("批准后必须能出去");
        assert!(!ctx.plan_phase.load(Ordering::Relaxed), "ExitPlanMode 批准后必须解除");
        assert!(note.contains("approved"), "回执要说明计划已获批，实际是：{note}");
    }

    /// 只读档（用户在设置里选的「只读」）与计划相位是**两回事**，在出口上尤其明显：
    /// 只读档下 `ExitPlanMode` 是**被拒**的 —— 批准了计划也执行不了，说「批准后就能写」
    /// 会把模型引到一个必然失败的动作上。
    #[test]
    fn read_only_profile_refuses_the_plan_exit() {
        let mut ctx = test_ctx();
        ctx.read_only = true;
        // 只读档下仍可**进入**计划相位（它不碰本机），但必须由正文交代计划
        assert!(dispatch_tool(&ctx, None, &[], "EnterPlanMode", &json!({})).is_ok());
        let err = dispatch_tool(&ctx, None, &[], "ExitPlanMode", &json!({"plan": "# p"}))
            .expect_err("只读档下计划相位退不出去");
        assert!(
            err.contains("settings"),
            "要做的是去设置里改档位，拒因必须这么说，实际是：{err}"
        );
        assert!(ctx.plan_phase.load(Ordering::Relaxed), "被拒时相位不得被清掉");
    }

    /// 轮次门槛（A4）。三条边界：`interval == 0` 是**关闭**而不是「每次都跑」；
    /// 还没问过（0）不触发；到整数倍才触发。
    #[test]
    fn nudge_gate_fires_on_multiples_only() {
        for q in 1..10u64 {
            assert!(!should_review(q, 10), "第 {q} 问不该复盘");
        }
        for q in [10u64, 20, 30] {
            assert!(should_review(q, 10), "第 {q} 问该复盘");
        }
        assert!(!should_review(0, 10), "一次都没问过时不触发");
        assert!(!should_review(10, 0), "interval=0 是关闭");
        assert!(should_review(1, 1), "interval=1 时每次都触发（允许，但代价自负）");
    }

    /// 复盘快照（A4）：只认三类块、工具结果有上限、超预算时**从最早处**丢。
    #[test]
    fn review_transcript_trims_from_the_oldest_end() {
        let history = vec![
            json!({ "role": "user", "content": [{ "type": "text", "text": "ANCIENT" }] }),
            json!({ "role": "assistant", "content": [
                { "type": "thinking", "thinking": "不该出现" },
                { "type": "tool_use", "id": "t1", "name": "Read", "input": { "file_path": "x" } },
            ] }),
            json!({ "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "t1", "content": "y".repeat(500) },
            ] }),
            json!({ "role": "user", "content": [{ "type": "text", "text": "RECENT" }] }),
        ];
        let full = review_transcript(&history, 10_000);
        assert!(full.contains("ANCIENT") && full.contains("RECENT"), "预算足够时全都要在");
        assert!(full.contains("→ Read("), "工具调用要留一行摘要");
        assert!(!full.contains("不该出现"), "thinking 不进快照");
        assert!(full.contains("y…(truncated)"), "过长的工具结果要截断");
        assert!(full.contains("RECENT") && full.find("ANCIENT").unwrap() < full.find("RECENT").unwrap());

        // 预算压到只放得下最后一行 ⇒ 最早的那条必须被丢掉
        let tiny = review_transcript(&history, 20);
        assert!(tiny.contains("RECENT"), "最近的内容必须保住");
        assert!(!tiny.contains("ANCIENT"), "超预算时从最早处丢");
    }

    /// 复盘的提示词（A4）：当前记忆进的是**用户消息**（不是系统提示词），
    /// 空记忆要显示成占位符而不是空白。
    #[test]
    fn review_prompt_carries_memory_and_limit() {
        let p = review_prompt("user: 聊了点东西", "- 用户偏好中文\n");
        assert!(p.contains("用户偏好中文"));
        assert!(p.contains("user: 聊了点东西"));
        assert!(p.contains(&REVIEW_MAX_ITEMS.to_string()), "要写明最多写几条");
        assert!(p.contains("NOTHING_TO_REMEMBER"), "要给出「什么都不用记」的出口");
        assert!(review_prompt("x", "   ").contains("(nothing yet)"));
    }

    /// 复盘工具集（A4）= 从工具池里挑白名单子集。这条测试钉住两件事：
    /// ① 白名单**不含**任何能改本机其它东西的工具（`Bash` / `PowerShell` / `Agent` / MCP）；
    /// ② 白名单里的每一件都在 `defs()` 或条件注册里真的存在（写错名字 = 静默少一件）。
    #[test]
    fn review_tool_whitelist_is_read_and_memory_only() {
        for forbidden in ["Bash", "PowerShell", "WebFetch", "WebSearch", "Agent", "TodoWrite"] {
            assert!(
                !REVIEW_TOOL_WHITELIST.contains(&forbidden),
                "{forbidden} 不该进无人值守的后台复盘"
            );
        }
        // 模拟工具池：内置 + 条件注册的三件
        let mut pool = tools::defs(&[]);
        pool.push(skills::tool_def());
        pool.push(tools::remember_tool());
        pool.push(tools::read_resource_tool());
        let picked: Vec<String> = pool
            .iter()
            .filter_map(|d| d.get("name").and_then(Value::as_str).map(String::from))
            .filter(|n| REVIEW_TOOL_WHITELIST.contains(&n.as_str()))
            .collect();
        // 这个池子里 7 件白名单工具**全在**（内置 5 + 条件注册的 `Skill` / `Remember`）
        assert_eq!(picked.len(), REVIEW_TOOL_WHITELIST.len(), "白名单与工具池对不上：{picked:?}");
        for n in &picked {
            assert!(
                tools::needs_bridge(n) || ["Read", "Glob", "Grep", "Write", "Edit", "Skill"].contains(&n.as_str()),
                "{n} 不在预期的白名单工具里"
            );
        }
        assert!(!picked.contains(&"Agent".to_string()), "防递归");
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

    // ── 摘要式压缩（backlog §8.2）─────────────────────────────────────

    /// 测试用的 `Cfg`。端点指向一个必然连不上的本机端口 —— 只用于「**不该发请求**」的断言：
    /// 万一哪天守卫被改坏、请求真发出去了，用例会因连接失败而变红，而不是悄悄通过。
    fn cfg_for_tests() -> Cfg {
        cfg_pointing_at("http://127.0.0.1:1/v1/messages")
    }

    fn cfg_pointing_at(endpoint: &str) -> Cfg {
        Cfg {
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("build test client"),
            endpoint: endpoint.to_string(),
            token: "test".to_string(),
            model: "test-model".to_string(),
            thinking: Cell::new(Thinking::Disabled),
            last_input: Cell::new(0),
            last_compact: Cell::new(0),
        }
    }

    /// 起一个**只服务一次请求**的本地 HTTP stub，返回 (端点 URL, 收到的请求体)。
    ///
    /// 为什么值得写：摘要压缩唯一真正没被单测覆盖的东西就是**它发出去的 HTTP 形状**
    /// —— 非流式、不带 tools、system 是提示词、messages 是单条 user。真打端点要花钱且不确定，
    /// 而用一个 stub 就能把这些**逐字节断言**下来，且进常规 `cargo test`（零成本、不联网）。
    fn start_stub_server(response_body: String) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind stub");
        let addr = listener.local_addr().expect("stub addr");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            let mut reader = std::io::BufReader::new(sock.try_clone().expect("clone sock"));
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) == 0 {
                    break;
                }
                let t = line.trim_end().to_string();
                if t.is_empty() {
                    break;
                }
                let lower = t.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; content_length];
            use std::io::Read as _;
            let _ = reader.read_exact(&mut body);
            let _ = tx.send(String::from_utf8_lossy(&body).to_string());

            use std::io::Write as _;
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
        });
        (format!("http://{addr}/v1/messages"), rx)
    }

    /// 摘要压缩的**请求形状 + 响应解析**（零成本、不联网）。
    ///
    /// 覆盖：`stream:false` / `max_tokens` / `system` 是提示词 / **不带 tools** / messages 是
    /// 单条 user 且内容是渲染后的原文；以及响应侧 `content[].text` 抽取 + `SUMMARY_HEADER` 前缀。
    #[test]
    fn summary_request_shape_and_response_parsing() {
        let canned = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "## Historical Task Snapshot\n继续验证 §8.2"}
            ],
            "usage": {"input_tokens": 100, "output_tokens": 42}
        })
        .to_string();
        let (endpoint, rx) = start_stub_server(canned);
        let cfg = cfg_pointing_at(&endpoint);

        // 造一段够长的被丢历史（必须过 SUMMARY_MIN_INPUT_CHARS）
        let dropped: Vec<Value> = (0..6)
            .map(|i| {
                json!({"role":"assistant","content":[{
                    "type":"text","text":format!("m{i}:{}", "x".repeat(1_000))
                }]})
            })
            .collect();

        let out = summarize_dropped(&cfg, &dropped).expect("stub 返回正常时应当拿到摘要");
        assert!(out.starts_with(SUMMARY_HEADER), "摘要必须带固定表头：{out}");
        assert!(
            out.contains("继续验证 §8.2"),
            "必须用响应的 content[].text：{out}"
        );

        let sent: Value =
            serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).expect("应当收到请求"))
                .expect("请求体是 JSON");
        assert_eq!(sent["stream"], json!(false), "摘要请求必须是非流式");
        assert_eq!(sent["max_tokens"], json!(SUMMARY_MAX_OUTPUT_TOKENS));
        assert_eq!(
            sent["system"].as_str(),
            Some(SUMMARY_PROMPT),
            "system 必须是摘要提示词"
        );
        assert!(sent.get("tools").is_none(), "摘要请求不该带 tools");
        assert_eq!(sent["messages"].as_array().map(Vec::len), Some(1));
        assert_eq!(sent["messages"][0]["role"].as_str(), Some("user"));
        let text = sent["messages"][0]["content"][0]["text"]
            .as_str()
            .expect("渲染后的原文要放在 text 块里");
        for kept in ["m5:", "m4:", "m3:"] {
            assert!(text.contains(kept), "送进去的应当是被丢的原文（含 {kept}）");
        }
    }

    /// 端点返回非 2xx / 非 JSON / 空摘要时，一律降级为 `None`（不能让整轮对话失败）。
    #[test]
    fn summary_failures_degrade_instead_of_erroring() {
        // 非 JSON 的 200
        let (endpoint, _rx) = start_stub_server("not json at all".to_string());
        assert!(summarize_dropped(&cfg_pointing_at(&endpoint), &long_dropped()).is_none());

        // 200 但 content 为空
        let (endpoint, _rx) = start_stub_server(
            json!({"content": [], "usage": {"output_tokens": 0}}).to_string(),
        );
        assert!(summarize_dropped(&cfg_pointing_at(&endpoint), &long_dropped()).is_none());

        // 200 但文本全空白
        let (endpoint, _rx) = start_stub_server(
            json!({"content": [{"type": "text", "text": "   \n  "}]}).to_string(),
        );
        assert!(summarize_dropped(&cfg_pointing_at(&endpoint), &long_dropped()).is_none());

        // 连不上（端口 1）
        assert!(summarize_dropped(&cfg_for_tests(), &long_dropped()).is_none());
    }

    fn long_dropped() -> Vec<Value> {
        (0..6)
            .map(|i| {
                json!({"role":"assistant","content":[{
                    "type":"text","text":format!("m{i}:{}", "x".repeat(1_000))
                }]})
            })
            .collect()
    }

    /// **真打端点**验「非流式摘要请求被接受」+ 摘要内容符合模板（需要外网 + 真实凭据）。
    ///
    /// 为什么 stub 用例不够：stub 只能证明**我们发出去的形状自洽**，证明不了**真端点接受它**
    /// —— 例如端点是否允许 `stream:false`、是否强制要求 `tools`、是否只认 SSE、`system` 是否被接受。
    /// 这是摘要压缩唯一必须靠真实端点才能验的部分。
    ///
    /// 跑法（沿用 `fallback_scrapers` 的范式：唯一依赖真实凭据的摘要用例，故意不进常规 `cargo test`）：
    ///
    /// ```text
    /// $env:LUNAC_AGENT_BASE_URL="https://api.deepseek.com"
    /// $env:LUNAC_AGENT_TOKEN="sk-..."
    /// $env:LUNAC_AGENT_MODEL="deepseek-flash"
    /// cd core-agent; cargo test summary_compaction_against_the_real_endpoint -- --ignored --nocapture
    /// ```
    ///
    /// 注意：`## Historical Task Snapshot` 里那句 sentinel 是**模型逐字引用**的检查项。
    /// 它偶尔会因模型改写而失败 —— 那是**模型行为**，不是代码 bug；此时改为人工看 `--nocapture` 的打印。
    #[test]
    #[ignore = "需要外网 + 真实凭据：直打 /v1/messages 验非流式摘要请求"]
    fn summary_compaction_against_the_real_endpoint() {
        let cfg = match Cfg::from_env() {
            Ok(c) => c,
            Err(e) => {
                println!("[summary-live] 跳过：{e}（需要 LUNAC_AGENT_BASE_URL / TOKEN / MODEL）");
                return;
            }
        };

        // 拟真的一段待丢历史：早先的任务描述 + 一次工具调用 + 若干轮往返 + **最后一条用户输入**
        let sentinel = "LUNAC-SENTINEL-8421";
        let mut dropped = vec![
            json!({"role":"user","content":[{"type":"text","text":
                "帮我把渲染层架构决策写进 docs 里，并把优先级调到最高。"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":
                "好，我先读一遍现有规范再动笔。"}]}),
            json!({"role":"assistant","content":[{
                "type":"tool_use","id":"t1","name":"Read",
                "input":{"file_path":"d:/cc/claude-code-cli-master/docs/ai-spec.md"}}]}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"t1",
                "content":"（此处是 20000 字规范正文，演示用省略）"}]}),
        ];
        for i in 0..12 {
            dropped.push(json!({"role":"assistant","content":[{"type":"text","text":
                format!("第 {i} 步：核对 §{} 的表述，确认与既定决策一致。{}", i, "内容".repeat(60))}]}));
        }
        dropped.push(json!({"role":"user","content":[{"type":"text","text":format!(
            "先停下。现在改成去验证摘要压缩，记住这个标记 {sentinel}，别丢。"
        )}]}));

        let transcript_len: usize = dropped
            .iter()
            .map(|m| render_one_message_for_summary(m).chars().count())
            .sum();
        assert!(
            transcript_len >= SUMMARY_MIN_INPUT_CHARS,
            "用例自身的输入太短（{transcript_len} 字），改长一点才有意义"
        );

        let summary = summarize_dropped(&cfg, &dropped).expect("真端点应当接受非流式摘要请求");
        println!(
            "[summary-live] 原文 {transcript_len} 字 → 摘要 {} 字：\n{summary}",
            summary.chars().count()
        );

        assert!(summary.starts_with(SUMMARY_HEADER), "必须带固定表头：{summary}");
        assert!(
            summary.contains("## Historical Task Snapshot"),
            "提示词要求首段固定为该标题：{summary}"
        );
        assert!(summary.chars().count() > 80, "摘要不该是空壳：{summary}");
        assert!(
            summary.contains(sentinel),
            "「latest user message WINS」要求逐字留住最后一条用户输入，但摘要里没有 {sentinel}：{summary}"
        );
    }

    /// 被丢弃的消息必须**原样带出来** —— 摘要压缩的输入就是它。
    #[test]
    fn dropped_messages_are_handed_to_the_caller_for_summarising() {
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"帮我改脚本"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"好"}]}),
        ];
        for i in 0..COMPACT_KEEP_TAIL * 2 {
            history.push(json!({"role":"assistant","content":[{"type":"text","text":format!("m{i}")}]}));
        }
        let out = compact_history(&mut history, Compact::Force, 0);
        assert!(out.dropped > 0, "Force 档必须真的丢了消息");
        assert_eq!(
            out.dropped_msgs.len(),
            out.dropped,
            "带出来的消息条数必须与 dropped 一致"
        );
        assert!(
            out.dropped_msgs
                .iter()
                .any(|m| m["content"][0]["text"].as_str() == Some("好")),
            "带出来的应当是**被丢掉的那段**（含那条 assistant 消息）"
        );
    }

    /// 纯机械压缩返回 `pinned = 0`；钉了任务快照才为 1。
    ///
    /// 这个计数是回滚锚点 `base` 的修正项 —— 漏掉它，`finish_error` 的
    /// `history.truncate(base)` 会多切掉一条真实历史。
    #[test]
    fn pinned_counts_the_synthetic_messages_inserted() {
        // 不带 TodoWrite：不应插入任何合成消息
        let mut plain = vec![json!({"role":"user","content":[{"type":"text","text":"hi"}]})];
        for i in 0..COMPACT_KEEP_TAIL * 2 {
            plain.push(json!({"role":"assistant","content":[{"type":"text","text":format!("m{i}")}]}));
        }
        let out = compact_history(&mut plain, Compact::Force, 0);
        assert_eq!(out.pinned, 0, "没插合成消息时 pinned 必须是 0");

        // 带 TodoWrite：会钉回一条快照 ⇒ pinned == 1
        let mut with_todo = vec![
            json!({"role":"user","content":[{"type":"text","text":"帮我改脚本"}]}),
            json!({"role":"assistant","content":[{
                "type":"tool_use","id":"t1","name":"TodoWrite",
                "input":{"todos":[{"content":"a","status":"pending","activeForm":"a"}]}
            }]}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}),
        ];
        for i in 0..COMPACT_KEEP_TAIL * 2 {
            with_todo.push(json!({"role":"assistant","content":[{"type":"text","text":format!("m{i}")}]}));
        }
        let out = compact_history(&mut with_todo, Compact::Force, 0);
        assert!(out.dropped > 0);
        assert_eq!(out.pinned, 1, "钉了任务快照 ⇒ pinned 必须是 1");
    }

    /// 渲染进摘要的原文：**保留最近的内容**、按时间顺序、总量封顶。
    #[test]
    fn summary_input_keeps_the_newest_and_stays_under_the_cap() {
        let dropped: Vec<Value> = (0..10)
            .map(|i| {
                json!({"role":"assistant","content":[{
                    "type":"text","text":format!("m{i}:{}", "x".repeat(5_000))
                }]})
            })
            .collect();
        let rendered = render_dropped_for_summary(&dropped);

        assert!(
            rendered.chars().count() <= SUMMARY_MAX_INPUT_CHARS,
            "必须封顶在 {SUMMARY_MAX_INPUT_CHARS} 字以内，实际 {}",
            rendered.chars().count()
        );
        // 只剩 4 条放得下（4×~5005 = 20020），应当是被丢段里**最新的**那 4 条
        for kept in ["m9:", "m8:", "m7:", "m6:"] {
            assert!(rendered.contains(kept), "应当保留 {kept}");
        }
        assert!(!rendered.contains("m5:"), "超预算时应当丢掉更老的，不该包含 m5");

        // 时间顺序：老的在前
        let pos = |s: &str| rendered.find(s).unwrap();
        assert!(pos("m6:") < pos("m7:") && pos("m7:") < pos("m8:") && pos("m8:") < pos("m9:"));
    }

    /// 工具调用在摘要输入里只留骨架（名字 + 截断的入参），空消息直接跳过。
    #[test]
    fn summary_input_skeletonises_tools_and_skips_empty_messages() {
        let long = "y".repeat(5_000);
        let msg = json!({"role":"assistant","content":[
            {"type":"tool_use","id":"t1","name":"Bash","input":{"command": long}},
            {"type":"tool_result","tool_use_id":"t1","content": long},
        ]});
        let rendered = render_one_message_for_summary(&msg);
        assert!(rendered.starts_with("assistant: [tool_use Bash]"), "{rendered}");
        assert!(rendered.contains("[tool_result]"));
        assert!(
            rendered.chars().count()
                < RENDER_TOOL_INPUT_CHARS + RENDER_TOOL_RESULT_CHARS + 60,
            "工具入参 / 结果都必须截断，实际 {} 字",
            rendered.chars().count()
        );

        assert_eq!(render_one_message_for_summary(&json!({"role":"user","content":[]})), "");
        assert_eq!(render_one_message_for_summary(&json!({"role":"user"})), "");
    }

    /// 没有可丢的内容时，摘要压缩**一条网络请求都不该发**。
    #[test]
    fn nothing_is_summarised_when_nothing_was_dropped() {
        let cfg = cfg_for_tests();
        let mut history = vec![json!({"role":"user","content":[{"type":"text","text":"hi"}]})];
        assert!(
            !pin_summary_of_dropped(&cfg, &mut history, &[]),
            "dropped 为空必须直接返回 false（不发请求）"
        );
        assert_eq!(history.len(), 1, "不该改动历史");
    }

    /// 内容太短时跳过 —— 不值得为一点点东西花一次调用（单轮成本上限的一道闸）。
    #[test]
    fn tiny_dropped_regions_are_not_summarised() {
        let cfg = cfg_for_tests();
        let small = vec![json!({"role":"assistant","content":[{"type":"text","text":"很短"}]})];
        assert!(
            summarize_dropped(&cfg, &small).is_none(),
            "低于 SUMMARY_MIN_INPUT_CHARS 时不该调模型"
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

    /// 图片附件（A8）：**只认魔术字节**、按路径读字节转 base64、失败与超限都如实上报。
    #[test]
    fn image_blocks_are_resolved_by_path_and_limited() {
        // 类型判定只看字节，不看扩展名
        assert_eq!(
            image_media_type(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
            Some("image/png")
        );
        assert_eq!(image_media_type(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(image_media_type(b"GIF89a....."), Some("image/gif"));
        assert_eq!(
            image_media_type(b"RIFF____WEBPVP8 "),
            Some("image/webp"),
            "WebP 要同时看 RIFF 与 WEBP 两处"
        );
        assert_eq!(image_media_type(b"BM......"), None, "BMP 不该被放行");
        assert_eq!(image_media_type(b"II*\0"), None, "TIFF 不该被放行");
        assert_eq!(image_media_type(b"not an image"), None);

        let dir = std::env::temp_dir().join(format!("lunac-img-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png_bytes: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
        let png = dir.join("real.png");
        std::fs::write(&png, &png_bytes).unwrap();
        let liar = dir.join("liar.png"); // 扩展名是 png，内容不是
        std::fs::write(&liar, b"not an image").unwrap();

        let block = |path: String| json!({ "type": "image", "source": { "type": "file", "path": path } });
        let raw = vec![
            block(png.to_string_lossy().to_string()),
            block(liar.to_string_lossy().to_string()),
            block(dir.join("missing.png").to_string_lossy().to_string()),
            block(dir.to_string_lossy().to_string()), // 目录
            json!({ "type": "image", "source": { "type": "url", "url": "https://x/y.png" } }),
        ];
        let (blocks, notes) = collect_image_blocks(&raw);

        assert_eq!(blocks.len(), 1, "五条里只有那张真 PNG 能过");
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(blocks[0]["source"]["type"], "base64");
        assert_eq!(blocks[0]["source"]["media_type"], "image/png");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(blocks[0]["source"]["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded, png_bytes, "base64 必须能原样还原字节");

        assert_eq!(notes.len(), 4, "四种失败各自成一条说明（不静默丢）");
        assert!(
            notes
                .iter()
                .all(|n| n.get("reason").and_then(Value::as_str).is_some_and(|r| !r.is_empty())),
            "每条说明都要有非空 reason"
        );

        // 张数上限：超出的进说明，已收下的顺序不变
        let many: Vec<Value> = (0..MAX_IMAGE_FILES + 2)
            .map(|_| block(png.to_string_lossy().to_string()))
            .collect();
        let (blocks, notes) = collect_image_blocks(&many);
        assert_eq!(blocks.len(), MAX_IMAGE_FILES);
        assert_eq!(notes.len(), 2);
        assert!(
            notes[0]["reason"].as_str().unwrap_or_default().contains("more than"),
            "超出上限的说明要写清是张数问题"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
