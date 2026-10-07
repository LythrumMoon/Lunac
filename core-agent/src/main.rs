// core-agent/src/main.rs
// Lunac 自研 agent 核心 —— P1：内置工具循环（Read/Write/Edit/Cmd/Glob/Grep）
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
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};

mod bash_safety;
mod content_safety;
mod hooks;
mod image;
mod log;
mod mcp;
mod mcp_oauth;
mod peers;
mod skills;
mod tools;

/// 单次回复的 token 上限（无思考时的基线）
const BASE_MAX_TOKENS: u32 = 8192;
/// 请求总超时（含流式读取整段响应）
const REQUEST_TIMEOUT_SECS: u64 = 1800;
const CONNECT_TIMEOUT_SECS: u64 = 30;
/// 一轮用户提问内最多允许的「模型→工具→模型」往返次数。
///
/// ⚠️ **2026-10-01 从 16 放宽到 64，且它不再是主要闸门**（用户要求）。两条依据：
///   ① 实测（`release` 版 `temp\logs\agent-*.log`）：9 次提问里 **3 次把 16 打满**
///      （其中一次还是连续两问都打满），被打满时任务明显没做完 —— 用户只能紧接着
///      再发一问，而新提问要**全价重发**一遍上下文（实测 `命中率=11.5%`），
///      总账比一次跑完更贵；
///   ② 轮次本身**不是**成本的好代理：一次提问的钱取决于**累计 token**，而 16 轮里
///      可能全是小调用、也可能 5 轮就烧掉几十万。所以闸门换成 `TURN_BUDGET_TOKENS`
///      （见下），轮次退化成**纯兜底**（防「预算判据失效 + 模型失控」这类组合失效）。
/// 业界口径参照：LangChain `max_iterations=15`、OpenManus 20/30、Ralph loop 10 ——
/// 没有一家是「无上限」，所以我们也不取消，只把它从主闸门降级。
const MAX_TOOL_ROUNDS: usize = 64;
/// 工具轮次打满时的「收口」指令。
///
/// **落点必须让 history 的形状与块数量都不变** —— 即拼进上一条 `tool_result` 的
/// **文本内部**，不得单独 `push` 一条新的 `user` 消息、也不得在消息的 content 数组里
/// 追加 `text` 块（2026-09-20 单变量实测，不得回退）。
///
/// 下表的「第 17 轮」是**当年 `MAX_TOOL_ROUNDS = 16` 时的第 17 次请求**（= 打满后的
/// 收口轮）；结论与那个数字**无关** —— 它量的是「动到前缀末尾的块结构会怎样」：
///
/// | history 形态（打满后的收口轮，末条） | 条数 | 该轮 `read` | 整次提问命中率 |
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
///
/// 另一条**零代价**的落点（2026-10-01 卡住检测用到）：拼进**刚 push、还没发出去过**
/// 的那条 `tool_result` —— 它不在任何已缓存前缀里，改它一个字都不废。
const TOOL_BUDGET_HINT: &str =
    "Tool budget exhausted. Stop calling tools and give the user your final answer now.";

// ── 卡住检测（2026-10-01，用户要求）────────────────────────────────
//
// 动机来自同一批日志：进程#2 的 问#1 打了 16 轮，其中 6 次是 `SessionSearch` 在
// **反复换关键词瞎搜**（`Live2d`→`ComfyUI`→`任务`→`安装`→`帮你`→`E盘`）——
// 那不是「任务长」，是**原地打转**。靠放宽轮次/预算治不了它（只会让它多烧几轮），
// 得单独判、单独介入。
//
// ⚠️ **当前判据覆盖不了上面那个 6 连搜索，只覆盖「调用与结果都重复」那一类。**
// 那 6 次的 query 与结果**每轮都不同**（换一个词搜一次），属「低效探索」而不是
// 「机械重复」；要判它得看「这一轮有没有新信息进来」，那是语义判断，本轮**不做**。
// 如实记在这里，免得以后误以为「日志里那个案例已经被这条判据解决了」——
// 它现在的价值是兜住另一类真实形态：模型把同一条命令 / 同一个查询**原样**再发一遍。
//
// 形态照 OpenManus 的 `is_stuck()`（把最近几轮的 assistant 消息去重，只剩一条 ⇒ 卡死），
// 但我们比的是**工具结果**而不是模型的话 —— 结果才是「有没有进展」的物证。
// 三条**同时**成立才判（缺一不判，免得把「连读 4 个文件」这种正常推进误判）：
//   · 最近 `STUCK_ROUNDS` 轮**每轮都只调用一个工具**；
//   · 这 4 轮的工具名**相同**；
//   · 这 4 轮的**结果文本头部相同**（`STUCK_FP_CHARS` 个字符）。
const STUCK_ROUNDS: usize = 4;
/// 结果指纹取多少个字符。**取头部而不是整段**：搜索结果这类文本的头部格式固定
/// （`Found N messages …`），整段比对会因为命中条数不同而永远不相等，反而抓不到。
/// 头部又足以区分「读的是不同文件」（路径就在开头）。
const STUCK_FP_CHARS: usize = 120;
/// 判定卡住后给模型的一次提示。**先提示、再收口**（用户 2026-10-01 定的处置）：
/// 误判的代价只是一句提示，而「再试一下就能通」的情况不少，直接掐掉太亏。
const STUCK_HINT: &str = "You have called the same tool repeatedly and the results are not \
changing — you are not making progress. Stop repeating it: either change your approach \
(a different tool, a more specific query, or reading the actual file), or give the user \
your best answer so far and state clearly what is still missing.";

// ── 单次提问的成本预算（2026-10-01，用户要求）──────────────────────
//
// 闸门口径 = 该提问内**所有请求**的 `in + read + create + out` 累计（与子代理的
// `SUBAGENT_BUDGET_TOKENS` 同一口径，也与前端 `usage-*.jsonl` 的归并口径一致）。
// 用累计 token 而不是轮数，是因为**轮数不是成本的好代理**（见 `MAX_TOOL_ROUNDS`）。
//
// 撞到预算**不直接停**，先「压缩续命」：`compact_history(Force)` + 摘要压缩 ⇒
// 历史变短 ⇒ 后续每轮更便宜，然后**清零计点继续跑**（形态取自 Claude Code 的
// auto-compact：它也是压完继续，而不是让用户重新开一轮）。
//
// ⚠️ **两件必须说清的事**：
//   ① 压缩**不退还已经花掉的钱** —— 它买的是「继续跑的机会」，不是「退款」。
//      所以续命本质上是把闸门**渐进地抬高一档**，`MAX_BUDGET_EXTENSIONS` 才是真上限。
//   ② 压缩**必须真的把体积降下来**才算续命成功。`Force` 档已经压到只留最近 2 条，
//      若它 `elided + dropped == 0`，说明体积在**对话本身**（不是工具结果）——
//      再压就是白废一次缓存 + 白花一次摘要钱，那种情况**直接收口**。
const TURN_BUDGET_ENV: &str = "LUNAC_TURN_BUDGET_TOKENS";
/// 默认 50 万 —— 与子代理的 `SUBAGENT_BUDGET_TOKENS`(30 万) 同量级，但主对话
/// 通常是长任务，所以给得更宽。
const DEFAULT_TURN_BUDGET_TOKENS: u64 = 500_000;
/// 撞预算后最多续命几次（用户 2026-10-01 定 2 次）。最坏累计 ≈ 3 × 预算。
const MAX_BUDGET_EXTENSIONS: usize = 2;
/// 预算用尽时的收口指令。与 `TOOL_BUDGET_HINT` 同款措辞、同一个落点纪律。
const TURN_BUDGET_HINT: &str =
    "This turn's cost budget is used up. Stop calling tools and give the user your final \
answer now, listing what is done and what remains.";

// ── 端点侧缓存的冷/热（2026-10-01，用户要求）──────────────────────
//
// 由来：`compact_history` 的收益判据（`worthwhile_elisions`）算的是「省下的 vs
// **被作废的**」，而「被作废」只有在**端点侧那份缓存还热着**的时候才是真代价。
// 距上次请求已经很久（超过本 TTL）时，那份缓存**本来就过期了** —— 此时动历史
// **零额外代价**，正是 Claude Code `microCompact` 的「冷路径」：
// 缓存冷 ⇒ 直接改内容（反正要重算）；缓存热 ⇒ 绝不动。
//
// 所以这条判据的作用是**单向放宽**：冷 ⇒ 不受水位与滞回约束，可以每轮清一次
// （「每轮微清理」）；热 ⇒ 保持原有全部约束。任何情况下都不会**更**激进地压热缓存。
const CACHE_TTL_ENV: &str = "LUNAC_CACHE_TTL_SECS";
/// 默认 **1 小时**（2026-10-02 上调，原为 300s）。
///
/// **为什么必须上调**（release 实测，见 backlog M2-16）：判据的原意是「缓存已过期 ⇒ 动
/// 历史零代价」，但它依赖的 TTL 是**我们猜的**。原默认 300s 的后果是：用户只要停手
/// 5 分钟以上，下一轮就会被判成「冷」并做 `ElideCold` 微清理 —— 可是日志里那次 elide
/// 之后 `read` 仍有 **6528**（≈ system+tools 量级），说明端点那份前缀缓存**并没有全冷**，
/// 是**我们自己**把 history 段打掉的。当天 06:35 / 06:50 / 06:55 / 09:24 / 09:59 各来一次，
/// 命中率当场掉到 21%–63%。
///
/// 端点（DeepSeek 自动前缀缓存 / Anthropic ephemeral）的实际寿命是**小时级**，不是分钟级
/// —— 所以「保守取小」在这里是**反的**：取小不是少几次免费压缩，而是**反复白废热缓存**。
/// 现在取 1 小时（仍远小于端点寿命 ⇒ 只在用户真的离开很久后才走这条冷路径）。
/// 需要更激进的自测可以用 `LUNAC_CACHE_TTL_SECS` 覆盖。
const DEFAULT_CACHE_TTL_SECS: u64 = 3600;

// ── 工具分级瘦身（2026-10-01，用户要求）────────────────────────────
//
// 原先的判据是 `tool_result > ELIDE_TOOL_RESULT_CHARS` **一刀切**，于是
// `Agent` 子代理的报告也会被瘦掉 —— 那是**不可重现**的东西（子代理的工作过程
// 不进主对话，报告是唯一产物），瘦掉等于让模型彻底失忆。
//
// 判断依据照 Claude Code 的 `COMPACTABLE_TOOLS`：**只瘦「用同样的参数再跑一次
// 就能拿回来」的结果**。白名单之外的（`Agent`、`Skill`、`SessionSearch`、`Remember`
// 以及全部 `mcp__*`）一律保留。
const ELIDABLE_TOOLS: &[&str] = &[
    "Read", "Grep", "Glob", "Cmd", "PowerShell", "WebFetch", "WebSearch",
];

/// 该工具的结果是否**可重现**（⇒ 可安全瘦身）。
///
/// `None`（认不出工具名）**一律不瘦** —— 分不清就按「不可重现」处理，宁可多留体积。
/// 纯函数，便于单测钉住「`Agent` / `mcp__*` 绝不进白名单」这条纪律。
fn elidable_tool(name: Option<&str>) -> bool {
    name.map(|n| ELIDABLE_TOOLS.contains(&n)).unwrap_or(false)
}

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
/// **不在名单里的一律拿不到**（`Cmd` / `PowerShell` / `WebFetch` / `Agent` / MCP 工具…）：
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
/// 同一批**子代理**（`Agent` / fork 技能）的最大并发数（A14，2026-09-20）。
///
/// 与 `TOOL_PARALLELISM` **刻意不同值**，理由完全不同：只读批受限于本地 IO，
/// 子代理批受限于**钱与端点**。每个子代理的预算是 `SUBAGENT_BUDGET_TOKENS`（30 万），
/// 3 个同时跑就是最坏 90 万 token 一起烧；再往上加，一次对话的花费会变成猜不到的量级。
/// 3 也是「模型一轮里真能拆出的互不依赖任务数」的常见上限 —— 再多它自己就该分批。
const SUBAGENT_PARALLELISM: usize = 3;
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
/// 端点侧前缀缓存的**命中折扣**：命中价 = miss 价 ÷ N（当前供应商 DeepSeek 的公开价差是 1/50）。
///
/// **只当相对权重用**（这里不做任何金额计算）：本仓「价格不写进代码」那条纪律针对的是
/// 成本面板要展示的单价（`config\pricing.json`，A12）；这个常数只决定闸门的松紧，
/// 配大了只是更保守、不会算出错的金额。
const CACHE_HIT_DISCOUNT: u64 = 50;
/// 瘦身一次至少要能在**后续这么多次请求**里回本才动手（保守下界）。
///
/// 为什么是 50：一次提问常常就有 10–17 次请求，而瘦身省下的体积是**永久**的
/// （被瘦身的那条消息此后一直是短文本，跟着会话活下去）⇒ 受益面远不止本轮的剩余轮数。
/// 取与折扣同量级是最保守的写法，于是判据收敛成一句好记的话：**省下的要多于废掉的**。
/// 详见 `worthwhile_elisions()` 的推导与 `docs/ai-spec.md` §11 规则 23。
const ELIDE_PAYBACK_REQUESTS: u64 = 50;
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

/// 单次提问的成本预算（token）。`0` = 关掉这道闸门（只留轮次兜底）。
fn turn_budget_tokens() -> u64 {
    match std::env::var(TURN_BUDGET_ENV) {
        Ok(v) => v.trim().parse::<u64>().unwrap_or(DEFAULT_TURN_BUDGET_TOKENS),
        Err(_) => DEFAULT_TURN_BUDGET_TOKENS,
    }
}

/// 端点侧前缀缓存的 TTL（秒）。低于 30 秒的取值视为笔误，退回默认。
fn cache_ttl() -> u64 {
    std::env::var(CACHE_TTL_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|n| *n >= 30)
        .unwrap_or(DEFAULT_CACHE_TTL_SECS)
}

/// 把一句提示**拼进最后一条 `tool_result` 的文本内部**（不动消息条数、不动块数量）。
///
/// 这是 `TOOL_BUDGET_HINT` / `STUCK_HINT` / `TURN_BUDGET_HINT` 共用的落点 —— 形态由
/// 2026-09-20 的单变量实测钉死（见 `TOOL_BUDGET_HINT` 上方那张表）：新增一条消息或
/// 新增一个内容块，都会让端点侧的前缀缓存整段失配。
///
/// 返回是否真的拼上了；历史末尾不是工具消息时拼不上（调用方据此决定要不要把
/// 「已提示」置位 —— 拼不上就置位，会让模型永远得不到提示）。
fn append_hint_to_last_tool_result(history: &mut [Value], hint: &str) -> bool {
    let Some(arr) = history
        .last_mut()
        .and_then(|m| m.get_mut("content"))
        .and_then(Value::as_array_mut)
    else {
        return false;
    };
    let Some(tr) = arr
        .iter_mut()
        .find(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
    else {
        return false;
    };
    let Some(prev) = tr.get("content").and_then(Value::as_str) else {
        return false;
    };
    tr["content"] = json!(format!("{prev}\n\n{hint}"));
    true
}

/// 一轮的工具结果指纹（卡住检测用）：结果文本**去空格后的前 `STUCK_FP_CHARS` 个字符**。
///
/// 为什么取头部不取整段：搜索结果这类文本的头部格式固定（`Found N messages …`），
/// 整段比对会因为命中条数不同而永远不相等 ⇒ 抓不到「换了关键词但一无所获」这个形态。
/// 而头部又足以区分「读的是不同文件」（路径就在开头），不会误伤正常推进。
///
/// 多工具轮把各结果拼起来（`\u{1}` 分隔）—— 调用方只在**单工具轮**才把它入窗口。
fn result_fingerprint(texts: &[String]) -> String {
    texts
        .iter()
        .map(|t| {
            t.trim()
                .chars()
                .filter(|c| !c.is_whitespace())
                .take(STUCK_FP_CHARS)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\u{1}")
}

/// 卡住判据（纯函数，便于单测）：窗口里是否四项同工具、同指纹。
///
/// 窗口由调用方维护成「最近 `STUCK_ROUNDS` 轮，每轮都是单工具轮」；
/// 这里只回答最后那个问题 —— 它们是不是在**重复同一件事**。
fn is_stuck(window: &std::collections::VecDeque<(String, String)>) -> bool {
    if window.len() < STUCK_ROUNDS {
        return false;
    }
    let Some((first_name, first_fp)) = window.front() else {
        return false;
    };
    window
        .iter()
        .all(|(name, fp)| name == first_name && fp == first_fp)
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
    /// **走成本模型**（`worthwhile_elisions`）：因为此时缓存多半是热的，动一下要付代价。
    Elide,
    /// **冷缓存档**（2026-10-01 新增）：「每轮微清理」走这条。
    ///
    /// 与 `Elide` 的唯一区别：**不**走成本模型、**不**受水位与滞回约束。
    /// 前提由调用方保证 —— 只有「距上次请求已超过缓存 TTL」才允许用它，
    /// 那时端点侧那份缓存本来就过期了，Δ 的代价是 0，判据恒成立，算了也白算。
    ElideCold,
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

/// 瘦身档的成本模型算出来的一个方案（`picked` 为空 = 判定「不值得压」）。
///
/// `sigma` / `delta` / `net` 是**判据本身用的那几个数**，一并带出来只为日志 ——
/// 事后复盘时不必再猜「当时为什么压了 / 为什么没压」。
struct ElidePlan {
    picked: Vec<(usize, usize, usize)>,
    /// 省下的字符数（永久省）
    sigma: usize,
    /// 被作废的后缀字符数（改完要全价重发一次的那部分）
    delta: usize,
    /// `σ × ELIDE_PAYBACK_REQUESTS − Δ × CACHE_HIT_DISCOUNT`（> 0 才值得动手）
    net: i128,
}

/// 瘦身档的**成本模型**（A15，2026-09-21）：从候选里挑出「值得动」的那一段。
///
/// 收益与代价都用**字符数**（两侧同系数，所以不必换算成 token）：
///   σ = 省下的字符 —— 这条消息此后每次请求都少发这些，而且**永久**（会话继续活着）
///   Δ = 被作废的后缀字符 —— `history[k..]` 瘦身之后再剩下的部分：改过之后它与端点侧
///       已落盘的缓存对不上，要**全价（miss）重发一次**，命中价是它的 1/50
/// 判据（两个常数的出处见各自注释）：
///   `σ × ELIDE_PAYBACK_REQUESTS > Δ × CACHE_HIT_DISCOUNT`
/// 白话：**省下的体积要真的多于被作废的体积**，否则就是「省小废大」。
///
/// 为什么只需要枚举一个下标 k：给定「最靠前被瘦身的那个块」k，**把 k 之后的候选全瘦掉
/// 永远不比只瘦一部分差** —— σ 变大、Δ 变小，两头都改善。所以自由度只有一个：k 定在哪。
/// 这里在候选下标上枚举（后缀和 O(1) 取值），取净收益最大的那个。
///
/// 注意这条判据管的是「**要不要**压」；`Drop` / `Force` 两档是防 400 的安全刚需，
/// 照旧**无条件**压，不走这里（见 `compact_history()` 的调用点）。
fn worthwhile_elisions(history: &[Value], candidates: &[(usize, usize, usize)]) -> ElidePlan {
    let empty = ElidePlan { picked: Vec::new(), sigma: 0, delta: 0, net: 0 };
    if candidates.is_empty() {
        return empty;
    }
    let n = history.len();
    // 逐条消息的 JSON 字符数：Δ 要算「k 之后剩下的全部」，不只是候选块本身
    let msg_chars: Vec<usize> = history
        .iter()
        .map(|m| serde_json::to_string(m).unwrap_or_default().chars().count())
        .collect();
    let mut suffix_msg = vec![0usize; n + 1];
    for i in (0..n).rev() {
        suffix_msg[i] = suffix_msg[i + 1].saturating_add(msg_chars[i]);
    }
    // 候选块先按消息聚合，再做后缀和（同一条消息里可能不止一个可瘦的块）
    let mut cand_at = vec![0usize; n + 1];
    for (mi, _, chars) in candidates {
        if *mi < n {
            cand_at[*mi] = cand_at[*mi].saturating_add(*chars);
        }
    }
    let mut suffix_cand = vec![0usize; n + 1];
    for i in (0..n).rev() {
        suffix_cand[i] = suffix_cand[i + 1].saturating_add(cand_at[i]);
    }

    let mut best: Option<(usize, usize, usize, i128)> = None; // (k, σ, Δ, net)
    for &(mi, _, _) in candidates {
        if best.map(|(k, _, _, _)| k) == Some(mi) {
            continue;
        }
        let sigma = suffix_cand[mi];
        let delta = suffix_msg[mi].saturating_sub(sigma);
        let net = sigma as i128 * ELIDE_PAYBACK_REQUESTS as i128
            - delta as i128 * CACHE_HIT_DISCOUNT as i128;
        if best.map(|(_, _, _, b)| net > b).unwrap_or(true) {
            best = Some((mi, sigma, delta, net));
        }
    }
    match best {
        Some((k, sigma, delta, net)) if net > 0 => ElidePlan {
            picked: candidates
                .iter()
                .cloned()
                .filter(|(mi, _, _)| *mi >= k)
                .collect(),
            sigma,
            delta,
            net,
        },
        Some((_, sigma, delta, net)) => ElidePlan { picked: Vec::new(), sigma, delta, net },
        None => empty,
    }
}

/// 压缩历史。`measured_tokens` = 上一轮实测的上下文体积（0 = 未知），只进日志。
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

    // ── 工具分级索引（2026-10-01）：`tool_use_id` → 工具名 ──────────────
    // `tool_result` 块自己只带 `tool_use_id`，工具名在**前一条 assistant 消息**的
    // `tool_use` 块里 ⇒ 先扫一遍建索引，才谈得上「哪一类结果可以瘦」。
    // 用 owned `String` 而不是 `&str`：下面要拿 `&mut history`，借用活不到那时。
    let mut tool_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for msg in history.iter() {
        let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            if let (Some(id), Some(name)) = (
                block.get("id").and_then(Value::as_str),
                block.get("name").and_then(Value::as_str),
            ) {
                tool_names.insert(id.to_string(), name.to_string());
            }
        }
    }

    // 先**只统计**、再由成本模型决定动不动手（推导见 `worthwhile_elisions()`）。
    let mut candidates: Vec<(usize, usize, usize)> = Vec::new();
    for (mi, msg) in history.iter().enumerate().take(tail_start) {
        let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
            continue;
        };
        for (bi, block) in blocks.iter().enumerate() {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            // 工具分级（2026-10-01）：只瘦**可重现**的结果。`Agent` 子代理的报告
            // 与全部 `mcp__*` 结果都是不可重现的 —— 瘦掉就是永久失忆。
            let tool_name = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .and_then(|id| tool_names.get(id))
                .map(String::as_str);
            if !elidable_tool(tool_name) {
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
    // 成本模型只在**瘦身档**生效（它是可选档：压不压都能跑）。
    // `Drop`（0.95 水位）与 `Force`（400 兜底）是**安全刚需** —— 不压就可能撞上限，
    // 所以这两档照旧无条件瘦，不看收益；Force 还连尾部也瘦（`elide_keep = 2`）。
    let plan = if mode == Compact::Elide {
        worthwhile_elisions(history, &candidates)
    } else {
        let sigma: usize = candidates.iter().map(|(_, _, n)| *n).sum();
        ElidePlan { picked: candidates.clone(), sigma, delta: 0, net: 0 }
    };
    let skip_elide = mode == Compact::Elide && plan.picked.is_empty();

    let mut elided = 0usize;
    let mut elided_chars = 0usize;
    if skip_elide {
        // 判为「省小废大」：**一个字都不动**（动了就是白废一段前缀缓存）。
        // 与旧实现的关键差别：旧闸门只看「可省体积占上下文的比例」，看不见代价的
        // 位置 —— 于是「省 2%、废 60%」这种压法也能过闸（A15 的起因）。
        //
        // 数字写成 `sigma=/delta=/net=` 这种 `key=value`（与 A16 的 `请求前缀` 行同风格）：
        // 中文散文给人看，键值给机器取 —— `target\hooktest\e2e-a15.ps1` 就是靠它们
        // 断言的（PS 5.1 脚本是 ANSI，脚本里没法写中文模式串）。
        log::info(format!(
            "跳过瘦身 sigma={} delta={} net={} ctx={} tokens \
             —— 判为省小废大（省下的不比废掉的多），一个字都不动",
            plan.sigma, plan.delta, plan.net, measured_tokens
        ));
    } else {
        for (mi, bi, n) in &plan.picked {
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
        if mode == Compact::Elide && elided > 0 {
            // 与上面那条「上下文压缩」汇总行配套：这里把**决策依据**也落一行，
            // 事后复盘不必再猜「当时为什么压这一段」（A15 的埋点口径）。
            log::info(format!(
                "瘦身决策 sigma={} delta={} net={} ctx={} tokens —— 省大废小，动手（命中折扣 {}×）",
                elided_chars, plan.delta, plan.net, measured_tokens, CACHE_HIT_DISCOUNT
            ));
        }
        if mode == Compact::ElideCold && elided > 0 {
            // 冷路径没有「判据」可记，只需留痕说明**为什么敢不看代价** ——
            // 事后复盘时它要与 `cache_cold=true` 那条日志配套看。
            log::info(format!(
                "冷缓存微清理：瘦身 {elided} 个可重现工具结果（省 {elided_chars} 字）\
                 —— 缓存已过期，此时动历史零额外代价"
            ));
        }
    }

    // ② 丢弃：强制模式，或「已到丢弃水位却挤不出来」（说明体积在对话本身）。
    //    只瘦身档永不丢弃 —— 在 0.85 水位上丢整条消息会把缓存一次性废掉，
    //    而按 0.95 水位多等一会儿完全来得及。
    //    始终保留开头那条用户提问 —— 它是任务目标，丢了模型就不知道要干什么。
    let can_drop = match mode {
        // `ElideCold` 与 `Elide` 同：**永不丢整条消息**。冷缓存只免掉「动历史」的
        // 那笔缓存代价，并不改变「0.85 水位上丢消息会一次性废掉整段前缀」这个判断。
        Compact::Elide | Compact::ElideCold => false,
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
                // `content` 可能是字符串，也可能是块数组（Read 读到图片时是
                // `[text, image]`，见 `tool_result_block`）—— 图片只渲染成 `[image]`，
                // 不把 base64 灌进摘要输入（既没用又极贵）。
                let raw = b.get("content");
                let joined = match raw {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Array(arr)) => arr
                        .iter()
                        .map(|x| match x.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                x.get("text").and_then(Value::as_str).unwrap_or("").to_string()
                            }
                            Some("image") => "[image]".to_string(),
                            _ => String::new(),
                        })
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" "),
                    _ => String::new(),
                };
                let t = joined
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
///
/// **2026-09-29 加「批量化」那句**（L6 压请求次数）：一次提问实测 7~10 次请求，每次
/// 「模型→工具→模型」往返都要重发一遍前缀。而 `plan_tool_batches()` 早就能把**连续**的
/// 只读调用放进**同一个响应**里并发执行（上限 `TOOL_PARALLELISM`）——**一次请求带 N 个
/// `tool_use`**，不是发 N 份请求。省的是往返次数，不是并发度：原先只差一句提示词，
/// 模型每轮只发一个调用，白跑了好几趟。
const SYSTEM_PROMPT: &str = "You are Lunac's built-in assistant, running inside a Windows desktop launcher. \
Answer in the user's language and keep it concise. \
You can inspect and modify the local machine with the provided tools: prefer Read/Glob/Grep \
before editing, make the smallest change that solves the problem, and say what you changed. \
Batch independent lookups: when several reads or searches do not depend on one another, issue \
them as multiple tool calls in the SAME response instead of one per turn -- they run \
concurrently and each saved round-trip is a re-sent prefix you do not pay for. \
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
- Language (hard rule): answer in the language the user wrote in. A Chinese question means the whole reply is Chinese — code, commands, paths, and API/tool names stay as they are, but never write the prose in another language. If the user switches language, switch with them.

## Output Style
Remove AI writing patterns: no "stands as / testament / pivotal / crucial / underscoring / delve / tapestry / landscape / fostering / moreover / furthermore / in conclusion". No emoji decorations. No "I hope this helps / let me know / great question". No boldface headers in lists. Use simple "is/are/has" instead of "serves as/stands as/represents". Vary sentence rhythm. Have opinions.
Spend output tokens only on the answer (2026-09-29, L6): no preamble before tool calls, no announcing what you are about to do, no restating the plan, no recap of what you just read. Answer only what was asked -- skip extra findings and unrequested analysis. Never paste back content the user can already see. Keep the closing note to what changed, in one line."#;

/// 用户人格 / 自定义提示词的来源文件（宿主注入的绝对路径，L2，2026-09-21）。
///
/// **为什么是「文件」而不是 hooks**：人格是**固定块** —— 必须进系统提示词的固定前缀（规则 18/23）。
/// hooks 是**控制通道**（放行 / 拒绝 / 提示行），它的输出今天不进模型；用它承载人格等于给同一个
/// 机制加第二个职责，还会引进来一条权限面（「谁能写 `hooks.json` 谁就能改系统指令」）。
/// 详见 ai-spec §3.5「人格 / 自定义提示词」。
const PERSONA_ENV: &str = "LUNAC_PERSONA_FILE";
/// 用户人格段的表头。英文的原因同 `TRIMMED_MARKER`：这是**给模型看的元信息**，不进 i18n。
/// 它同时是判据锚点 —— 单测与 e2e 都靠它判断「这段进没进提示词」。
const USER_PERSONA_HEADER: &str = "## User-defined persona (from config\\persona.md — always apply)";
/// 人格文本上限（字符）。**宿主侧 `storage::MAX_PERSONA_CHARS` 是同值硬校验**，改一处要改两处。
///
/// 为什么必须有上限：这段进的是**每次请求都要发的固定前缀**，塞一篇长文等于给每一轮都加一笔
/// 固定成本，而它并不随任务变化。这里截断只是防御（比如用户绕过面板直接改文件）。
const MAX_PERSONA_CHARS: usize = 8_000;

/// 读用户人格文本 —— **只在启动时调一次**（结果进固定前缀，进程内不再变）。
///
/// 返回空串的三种情况都等价于「用户没配过」：环境变量没给 / 文件不存在 / 读不出来。
/// 只有第三种落一行 warn（前两种是正常状态：出厂没有这个文件）。
fn read_user_persona() -> String {
    let path = std::env::var(PERSONA_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(path) = path else {
        return String::new();
    };
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let text = raw.trim();
            if text.chars().count() > MAX_PERSONA_CHARS {
                log::warn(format!(
                    "用户人格超过 {MAX_PERSONA_CHARS} 字符（{path}）—— 固定前缀不适合放长文，已截断"
                ));
                return text.chars().take(MAX_PERSONA_CHARS).collect();
            }
            text.to_string()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            log::warn(format!("用户人格读不出（{path}）：{e} —— 本轮只用内置人格"));
            String::new()
        }
    }
}

/// 把用户人格文本拼成系统提示词的**固定追加段**：空白输入 → 空串（一个字节都不加）。
///
/// 位置刻意是「内置人格段之后、`Environment:` 之前」：内置段保住产品底线约束（文风 / 禁 emoji 这类
/// 不该被用户改掉），用户段紧跟其后；两段都在固定前缀里 ⇒ 永远命中缓存。
fn persona_block(persona: &str) -> String {
    let text = persona.trim();
    if text.is_empty() {
        return String::new();
    }
    format!("\n\n{USER_PERSONA_HEADER}\n{text}")
}

// ── 项目记忆（AGENTS.md，2026-10-01）────────────────────────────────
//
// 为什么需要它：`MEMORY.md`（`Remember` 工具）是**跨项目**的长期记忆 —— 「这个仓库要
// 怎么构建、目录有什么规矩、哪些东西不许动」混进去会被别的项目读到；而这些恰恰是
// 每进一个仓库就要重新说一遍的东西。业界已把 **`AGENTS.md`** 当成这件事的约定
// （OpenAI 主导、多家 IDE / agent 采纳），所以直接读工作目录下那一份，不另发明文件名。
//
// **只在启动时读一次**：它进的是系统提示词固定前缀（规则 18 / 23），热读会把整段缓存
// 打掉 —— 改完 `AGENTS.md` 要重启 agent（或切换一次思考开关）才生效，与 persona 同一条纪律。
//
// **不做向上递归**：「取工作目录下那一份」是一条明确规则；往上找会引出「哪一级算数 /
// 找到几份算几份」这类没有答案的问题。要覆盖子目录就让用户在自己那级写。
//
// **与「没这个功能」逐字节等价**：没有文件 / 文件是空白 ⇒ 一个字节都不加（不给空行、
// 不给表头），与 persona 的空白口径一致。
const AGENTS_MD_FILE: &str = "AGENTS.md";
/// 项目记忆段的表头。英文的原因同 `USER_PERSONA_HEADER`：这是**给模型看的元信息**。
/// 它同时是判据锚点 —— 单测靠它判断「这段进没进提示词」。
const AGENTS_MD_HEADER: &str =
    "## Project instructions (from AGENTS.md in the working directory — always apply)";
/// 项目记忆上限（字符）。理由同 `MAX_PERSONA_CHARS`：这段进**每次请求都要发的固定前缀**，
/// 截断只是防御（用户可能往 `AGENTS.md` 里堆一整篇开发手册）。
const MAX_AGENTS_MD_CHARS: usize = 8_000;

/// 读工作目录下的 `AGENTS.md`。没有这个文件是**正常状态**（大多数目录都没有）⇒ `None`。
fn read_agents_md(cwd: &std::path::Path) -> Option<String> {
    let path = cwd.join(AGENTS_MD_FILE);
    match std::fs::read_to_string(&path) {
        Ok(raw) => Some(raw),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            log::warn(format!("项目记忆读不出（{}）：{e}", path.display()));
            None
        }
    }
}

/// 把 `AGENTS.md` 正文拼成固定追加段。抽出来是为了能单测「空白 → 空串」与「超长截断」。
fn project_block_from(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }
    let text: String = if text.chars().count() > MAX_AGENTS_MD_CHARS {
        log::warn(format!(
            "AGENTS.md 超过 {MAX_AGENTS_MD_CHARS} 字符 —— 固定前缀不适合放长文，已截断"
        ));
        text.chars().take(MAX_AGENTS_MD_CHARS).collect()
    } else {
        text.to_string()
    };
    format!("\n\n{AGENTS_MD_HEADER}\n{text}")
}

/// 项目记忆固定段（读盘 + 拼装 + 落一行日志）。拿不到就返回空串，绝不阻断启动。
fn project_block(cwd: &std::path::Path) -> String {
    let Some(raw) = read_agents_md(cwd) else {
        return String::new();
    };
    let block = project_block_from(&raw);
    if !block.is_empty() {
        log::info(format!(
            "项目记忆已注入系统提示词固定段：{} 字（{}）",
            block.chars().count(),
            cwd.join(AGENTS_MD_FILE).display()
        ));
    }
    block
}

/// 主系统提示词的装配 = 角色段 + 人格段 + 环境块 + 技能清单 —— **只在启动时调一次**
/// （`persona` 由 `read_user_persona()` 给出）。
///
/// 抽成函数的唯一理由：让「用户人格**只**进主提示词」这条不变量能被单测钉住
/// （见 `user_persona_reaches_only_the_main_prompt`）。
///
/// ⚠️ **刻意不含「往期会话索引」与「长期记忆」**（2026-10-06 挪出，M2-16 方案 D）：
/// 那两块随会话 / 记忆增长而变，留在 `system` 里会让**每次重启后** system 前缀整个变掉
/// ⇒ 端点侧缓存从第 0 个 token 起全 miss（实测一天里 system hash 变了 4 次，每次重启后
/// 首问命中率只有 11.7%–62%）。现在它们作为**一条 `user` 消息**注在 history 开头
/// （见 [`context_block_message`]），system 从此**跨重启逐字节稳定**。
fn build_system_prompt(persona: &str, env: &str, skills: &str) -> String {
    format!(
        "{SYSTEM_PROMPT}\n\n{PERSONA_AND_STYLE}{}{env}{skills}",
        persona_block(persona)
    )
}

/// 上下文块（往期会话索引 + 长期记忆）的表头。
///
/// 它同时是**幂等注入的判据**（见 [`is_context_block_msg`]）：`set_history`（回退 / 恢复
/// 会话）会整份换掉 history，注过的那条随之消失 —— 下一问必须能认出「没有它」并补回去，
/// 否则表现为「回退之后模型突然不记得长期记忆」。
const CONTEXT_BLOCK_HEADER: &str = "# Background context (fixed for this session)";

/// 把「往期会话索引」与「长期记忆」拼成**一条 `user` 消息**（两者都空 ⇒ `None`）。
///
/// **为什么是 `user` 消息、且注在 history 开头**（2026-10-06，M2-16 方案 D，三个位置逐一体检）：
///   · 留在 `system` ⇒ 重启后 system 变 ⇒ 端点缓存从 0 起全 miss（**这正是要修的**）；
///   · 注在**消息尾** ⇒ 踩 `TOOL_BUDGET_HINT` 里那条**实测红线**：动到前缀**末尾**的块结构
///     会让端点缓存单元整体失配（`read` 从 11776 掉到 2560，2026-09-20 实测、不得回退）；
///   · 注在**开头** ⇒ 只动一次、之后结构恒定，不碰那条红线，且换来 system 稳定。
///
/// **代价（如实记）**：模型把它们当**用户说的话**看，不再有「系统级背景」的权威感 ——
/// 段内措辞已按「背景，不属于当前请求」写好，但仍不如 system 里硬。
fn context_block_message(history_block: &str, memory_block: &str) -> Option<Value> {
    let h = history_block.trim();
    let m = memory_block.trim();
    if h.is_empty() && m.is_empty() {
        return None;
    }
    let text = format!("{CONTEXT_BLOCK_HEADER}\n{h}\n{m}");
    Some(json!({
        "role": "user",
        "content": [{ "type": "text", "text": text }],
    }))
}

/// 这条消息是不是我们注入的「上下文块」（幂等判据，见 [`CONTEXT_BLOCK_HEADER`]）。
fn is_context_block_msg(msg: &Value) -> bool {
    msg.get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks.iter().any(|b| {
                b.get("text")
                    .and_then(Value::as_str)
                    .map(|t| t.starts_with(CONTEXT_BLOCK_HEADER))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// 子代理 / 复盘的系统提示词（角色段 + 环境块 + 技能清单）：**刻意不含用户人格**。
///
/// 它们是内部产物（子代理报告 / 复盘写记忆），用户的文风口吻在那里没有意义；而多一份文本
/// 就是每个并发子代理都要重发一次的固定成本。
fn build_subagent_system(env: &str, skills: &str) -> String {
    format!("{SUBAGENT_SYSTEM}{env}{skills}")
}

fn build_review_system(env: &str, skills: &str) -> String {
    format!("{REVIEW_SYSTEM}{env}{skills}")
}

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
    // 插件（界面件）目录 + 写插件的规范文档（2026-09-28）。**必须在这里写明**：
    // 模型不会凭空知道 Lunac 的插件放在哪、该照什么格式写 —— 不写这段，
    // 「让 Lunac 自己做个插件」它只会去猜，然后写出一堆装不上的东西。
    // 与 skills 那条同理：同为「应用自己的目录」，在 agent 进程生命周期内是常量 ⇒ 满足前缀缓存要求。
    let modules_dir = std::env::var("LUNAC_MODULES_DIR")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(not configured)".into());
    let modules_line = if modules_dir == "(not configured)" {
        String::new()
    } else {
        format!(
            "- Lunac's plugins (UI modules) live in: {} — each plugin is a folder with a manifest \
             `lunac-plugin.json` plus an ESM entry (usually `index.js`). The authoring spec is in \
             that folder's README.md ({}). To create or fix a plugin for the user, read that file \
             first and follow it exactly; you can write there directly (no dev build needed).\n\
             - The user sees new plugins after they open 设置 → 插件 → 重新扫描 (no restart needed).\n\
             ",
            modules_dir,
            std::path::Path::new(&modules_dir).join("README.md").display()
        )
    };
    format!(
        "\n\nEnvironment:\n\
         - Host: Lunac, a Windows desktop launcher. You are Lunac's built-in agent — not a \
         component of any other agent framework, and not running inside one.\n\
         - Working directory (absolute): {}\n\
         - Lunac's own skills live in: {} — each skill is a folder containing SKILL.md. When the \
         user says \"my skills\", \"我自己的 skills\" or similar, they mean the skills of this app \
         (the ones listed below, if any) or this directory.\n\
         {}\
         - Other files on disk are ordinary files. If the workspace happens to contain another \
         agent/tool framework's repository, config or skills, do not treat it as Lunac's setup \
         and do not answer as if you were that product.",
        cwd.display(),
        skills_dir,
        modules_line
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
fn history_index_block(bridge: Option<&mut mcp::McpSet>, search_tool_on: bool) -> String {
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
fn fetch_memory(bridge: Option<&mut mcp::McpSet>) -> Option<String> {
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
fn memory_block(bridge: Option<&mut mcp::McpSet>, remember_tool_on: bool) -> String {
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

/// 子代理 / 后台复盘 fork 的累计用量（跨线程共享，2026-09-29）。
///
/// **为什么必须补记**：子代理（`Agent` 工具、fork 技能）与后台复盘各自发自己的 API 请求，
/// 平台照常计费，但 `run_subagent` 的返回值只带最终文本 ⇒ 它们的 token **从来没进过
/// `result.usage`**。实测证据（`usage_data_2026-09-29`）：平台同一 key 记 24 次请求，
/// 本地 `usage-*.jsonl` 只有 17 次 ⇒ 本地账系统性偏低，拿它判断「改哪儿能省钱」是在错的
/// 基数上做决定。
///
/// **为什么挂在 `Cfg` 上而不是往返回值里塞**：`run_subagent` 的三条路径
/// （`Agent` 工具 / fork 技能 / 后台复盘）都只拿得到 `&Cfg`；改签名要一路穿到
/// `run_agent_tool` / `run_forked_skill` / 并行批。而并行子代理批里每个子代理跑的是各自的
/// `Cfg::detached()` 副本 ⇒ 这个字段必须是 `Arc` 才跨得过线程（`detached()` 克隆它）。
///
/// **归并时机**：`run_query` 成功收尾时**取走并归零**（与 `result.usage`「本次提问的绝对值」
/// 同一口径）。后台复盘在提问之间跑完，其用量因此落在「收尾时已经跑完的那一问」或下一问上
/// —— 复盘本来就没有更细的归属，记进某一问比丢掉它准。
#[derive(Default)]
struct SubagentUsage {
    input: AtomicU64,
    cache_read: AtomicU64,
    cache_create: AtomicU64,
    output: AtomicU64,
    /// 逐请求明细，每条形态与 `result.usage.requests[]` 一致
    requests: Mutex<Vec<Value>>,
}

impl SubagentUsage {
    /// 记一轮子代理请求（`run_subagent` 每轮解析出 `usage` 后调用一次）
    fn record(&self, input: u64, cache_read: u64, cache_create: u64, output: u64) {
        self.input.fetch_add(input, Ordering::Relaxed);
        self.cache_read.fetch_add(cache_read, Ordering::Relaxed);
        self.cache_create.fetch_add(cache_create, Ordering::Relaxed);
        self.output.fetch_add(output, Ordering::Relaxed);
        if let Ok(mut v) = self.requests.lock() {
            v.push(json!({
                "in": input, "read": cache_read, "create": cache_create, "out": output,
            }));
        }
        // 子代理的请求同样**逐条上报**（2026-10-06）：主循环那条 `usage_delta` 只覆盖主循环
        // 自己的请求，而子代理 / 复盘烧的钱一样会被「回合被中断」吞掉。前端只累计、不分账，
        // 所以形状与主循环那条完全一致。
        emit(json!({
            "type": "usage_delta",
            "usage": { "in": input, "read": cache_read, "create": cache_create, "out": output },
        }));
    }

    /// 取走累计值并归零，返回 `(input, cache_read, cache_create, output, requests)`。
    /// 锁毒化时只丢明细、不让主循环跟着崩 —— 明细是归因用的，不该有这种否决权。
    fn take(&self) -> (u64, u64, u64, u64, Vec<Value>) {
        let reqs = match self.requests.lock() {
            Ok(mut v) => std::mem::take(&mut *v),
            Err(_) => Vec::new(),
        };
        (
            self.input.swap(0, Ordering::Relaxed),
            self.cache_read.swap(0, Ordering::Relaxed),
            self.cache_create.swap(0, Ordering::Relaxed),
            self.output.swap(0, Ordering::Relaxed),
            reqs,
        )
    }
}

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
    /// 子代理 / 后台复盘的累计用量（见 `SubagentUsage`）。`Arc` 是刻意的：
    /// `detached()` 出来的副本必须与主循环**记同一本账** —— 并行子代理批正是用副本跑的。
    sub: Arc<SubagentUsage>,
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
            sub: Arc::new(SubagentUsage::default()),
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
    /// `sub` 是**唯一被共享的字段**（`Arc::clone`）：子代理与复盘烧的 token 是同一笔账，
    /// 各记各的等于把它们的用量丢在副本里（2026-09-29）。
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
            sub: Arc::clone(&self.sub),
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

/// 主循环空闲轮的间隔（2026-10-06）：只为及时发现「**回合外**改了 MCP 配置」——
/// 收到 user 消息那条路要等用户先提问才发现，那已经太晚（工具表已定死，只能白答一轮再重启）。
/// 1s 落在「用户改完文件 → 按下提问」的典型间隔之内；空闲时每拍两次 `metadata()`，开销可忽略。
const MCP_WATCH_TICK: Duration = Duration::from_millis(1000);

/// MCP 配置改动 → 交给前端在合适的时机重启 agent（2026-10-06）。
///
/// **一个进程只报一次**：报过就把 watch 丢掉（`Option` 置 `None`）—— 之后连这个函数都不会再
/// 被调用，前端也不会重复重启。新进程重新拍快照，只有**又**改了才会再报。
///
/// `idle = true` 表示「这拍是 agent 空闲时打的」，前端的处置**不同**：
///   · `idle: true` ⇒ 此刻没有回合在跑，**立即重启**（回合外改动应当零代价生效，不必白答一轮）；
///   · 不带 `idle`  ⇒ 这拍是收到 user 消息时打的（回合刚开），**等本回合 `result` 收尾**再重启。
/// 这个判据不依赖前端的 `isStreaming` / `agentState`：agent 侧的空闲分支**只可能**在真空闲时
/// 进入（`run_query` 同步阻塞主循环，期间根本不会超时醒来），所以 `idle` 是可靠的。
fn mcp_config_changed_event(watch: &mut Option<mcp::ConfigWatch>, idle: bool) -> Option<Value> {
    let changed = watch.as_mut()?.changed();
    if changed.is_empty() {
        return None;
    }
    *watch = None; // 只报一次 ⇒ 之后不再碰文件系统
    log::info(format!(
        "MCP 配置已改动（{}）—— 空闲={idle}，交由前端重启 agent",
        changed.join(" / ")
    ));
    Some(json!({
        "type": "system",
        "subtype": "mcp_config_changed",
        "files": changed,
        "idle": idle,
    }))
}

fn emit_stream_event(event: Value) {
    emit(json!({ "type": "stream_event", "event": event }));
}

/// 命令类工具的**实时输出**回传（2026-09-30）：把一次工具调用的 stdout / stderr 分片
/// 逐条 emit 出去；宿主只转发以 `{` 开头的行 ⇒ 一行一条 JSON，前端按 `tool_use_id`
/// 找到那张命令卡并**追加**输出（见 app/src/main.ts 的 `cli-output` 监听）。
///
/// **为什么是一个回调**：真正读管道的是 `tools.rs` 的 drain 线程，而 `emit()` 是本文件的
/// 私有函数 —— 用 `tools::Ctx::tool_output` 把两者缝起来，闭包里绑死 `tool_use_id`，
/// 于是 `tools.rs` 完全不必知道协议长什么样（它只认识 `sink(stream, chunk)`）。
fn tool_output_sink(tool_use_id: &str) -> tools::ToolOutputSink {
    let id = tool_use_id.to_string();
    Arc::new(move |stream: &str, chunk: &str| {
        emit(json!({
            "type": "tool_output",
            "tool_use_id": id,
            "stream": stream,
            "chunk": chunk,
        }));
    })
}

// ── 运行中命令的实时控制 + 后台运行（2026-10-01，用户要求）──────────────
//
// 三条通路都拿 `tool_use_id` 当键（前端卡片上挂着 `data-tool-id`，多一层映射就多一
// 处能对不上的地方）：
//   · `SHELL_CONTROLS`  —— **正在跑**的命令（`run_shell` 每拍看它的两个标志）
//   · `BACKGROUND_CMDS` —— **已转后台**的命令（前端「待办清单」里那一区就是它）
//   · `BACKGROUND_DONE` —— 已完成、但**还没交给模型**的输出（攒着，下一轮带上）

/// 转后台时**一次性移交**的那一包东西（`run_shell` 的 `background` 分支构造它）。
///
/// 打成一个结构体而不是七参数：这里的字段全是「从 `run_shell` 手里拿走的所有权」，
/// 打包后调用点一眼能看出「这几样东西换了个主人」。
pub struct BackgroundJob {
    pub child: std::process::Child,
    /// 两个读线程。`Option` 是因为**「子进程退出后才转后台」**那条路（2026-10-03，管道被
    /// detached 进程攥着）可能已经有一个读线程结束了 —— 结束的没必要再移交。
    pub h_out: Option<std::thread::JoinHandle<String>>,
    pub h_err: Option<std::thread::JoinHandle<String>>,
    /// 解释器名（只进日志）
    pub prog: String,
    /// 命令原文（给用户看的那一行；入库前会截断）
    pub label: String,
    pub ctl: Arc<tools::ShellControl>,
}

/// 正在执行的 shell 调用：`tool_use_id` → 控制块。
///
/// 只在 `run_one_tool` 里登记 / 注销，且只为 `Cmd` / `PowerShell` —— 其余调用点
/// （单测、子代理、后台复盘）拿到的 `Ctx::shell_control` 是 `None`，走不进来。
/// 走 `LazyLock` 而不是 `static Mutex<HashMap>`：`HashMap::new()` 在本仓的工具链上
/// **不是 const fn**，直接当 static 初值编译不过（`Mutex::new` 与 `Vec::new` 是）。
static SHELL_CONTROLS: std::sync::LazyLock<Mutex<HashMap<String, Arc<tools::ShellControl>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// 已转后台的命令：后台 id → 给前端画那一行用的描述。同上走 `LazyLock`。
static BACKGROUND_CMDS: std::sync::LazyLock<Mutex<HashMap<String, BgCmd>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// 已完成、还没交给模型的后台输出（**按完成顺序**）。下一轮请求之前被取走。
static BACKGROUND_DONE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// 后台命令的自增号（`bg_<pid>_<seq>`，形态同 `next_request_id`）
static BG_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 一条后台命令给前端看的描述。
struct BgCmd {
    /// 发起它的那次工具调用 —— 前端按它把那张卡片标成「已在后台运行」
    tool_use_id: String,
    /// 命令原文（已截断）
    label: String,
}

/// 接管一条要转后台的命令：**立刻**返回它的后台 id，收尾放到新线程里。
///
/// 三条纪律：
///   ① **不 kill** —— 「后台」的意思就是让它继续跑；
///   ② 收尾线程**仍在看 `ctl.stop`** —— 用户在「待办清单」里点取消时走的是**同一条**
///      通路（`route_tool_control` 按 id 置位），不必再单开一套「怎么把它杀掉」；
///   ③ 输出进 `BACKGROUND_DONE` **攒着**，不直接喂给模型 —— 主循环是单线程串行的，
///      从别的线程往里塞消息会撞坏 `emit` 的「stdout 一行一条 JSON」契约。
pub fn hand_off_to_background(job: BackgroundJob) -> String {
    let BackgroundJob { mut child, h_out, h_err, prog, label, ctl } = job;
    let id = format!("bg_{}_{}", std::process::id(), BG_SEQ.fetch_add(1, Ordering::Relaxed));
    let shown: String = label.chars().take(120).collect();
    if let Ok(mut m) = BACKGROUND_CMDS.lock() {
        m.insert(id.clone(), BgCmd { tool_use_id: ctl.tool_use_id.clone(), label: shown.clone() });
    }
    emit(json!({
        "type": "system",
        "subtype": "background_started",
        "id": id,
        "tool_use_id": ctl.tool_use_id,
        "label": shown,
    }));
    log::info(format!("shell[{prog}] 转后台 id={id} cmd={shown}"));

    let id2 = id.clone();
    std::thread::spawn(move || {
        // 等它退出 —— 每一拍看 `stop`：取消走的就是这个标志（与前台那套是同一份语义）
        let mut cancelled = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break Some(st),
                Ok(None) => {
                    if ctl.stop.load(Ordering::Relaxed) {
                        let _ = child.kill();
                        let _ = child.wait();
                        cancelled = true;
                        break None;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(_) => break None,
            }
        };
        // **有界**收尾：读线程等的是管道 EOF，而写端可能被别的进程攥着（detached 常驻进程）
        // ⇒ 无限 `join()` 会把这条后台命令永远挂在「运行中」。宽限期内读不完就放弃它的输出
        //（那部分早已经实时流回传过），让这条后台命令正常收口。
        let stdout = tools::join_within(h_out, tools::OUTPUT_JOIN_GRACE);
        let stderr = tools::join_within(h_err, tools::OUTPUT_JOIN_GRACE);
        let code = status.and_then(|s| s.code());

        let mut text = format!("[background command {id2}] {label}\n");
        if cancelled {
            text.push_str("(cancelled by the user — process killed)\n");
        }
        match code {
            Some(c) => text.push_str(&format!("exit code: {c}\n")),
            None if !cancelled => text.push_str("exit code: (none)\n"),
            None => {}
        }
        if !stdout.trim().is_empty() {
            text.push_str(&stdout);
            if !text.ends_with('\n') {
                text.push('\n');
            }
        }
        if !stderr.trim().is_empty() {
            text.push_str("[stderr]\n");
            text.push_str(&stderr);
        }

        // ── 后台输出**也必须过预算**（2026-10-02 补，backlog M2-16）──────────────
        // 这条路径原先把 stdout + stderr **原样拼接**就交给模型，**一次 `apply_budget`
        // 都没走**（前台工具结果走的是 `run_one_tool` 里那个唯一出口）。而它拼进的是
        // **已有 `tool_result` 的文本内部** ⇒ 等于按字节直接进上下文。
        // 实测事故：一条 `EdgeMap` 逐像素循环的 PowerShell 刷出 **512 KB stderr**
        // （524342B），history 一步从 `361,129 字` 涨到 **`841,816 字`**；此后每一轮都
        // 贴着丢弃水位、压缩每轮触发 ⇒ 前缀每轮全废（4 次请求 ≈ 71 万 miss token）。
        // 超过阈值时与前台同款：全文落盘，上下文只留头 + 尾 + 说明（模型要全文自己
        // `Read` / `Grep` 那个落盘文件）。
        let text = tools::apply_budget(&format!("{prog} (background)"), text);

        // 这里记的是**原始**字节数（诊断依据），过预算与否看上面 `apply_budget` 自己那行日志。
        log::info(format!(
            "shell[{prog}] 后台完成 id={id2} exit={code:?} cancelled={cancelled} \
             stdout={}B stderr={}B",
            stdout.len(),
            stderr.len()
        ));
        if let Ok(mut m) = BACKGROUND_CMDS.lock() {
            m.remove(&id2);
        }
        if let Ok(mut m) = SHELL_CONTROLS.lock() {
            m.remove(&ctl.tool_use_id);
        }
        // 攒着，等下一轮请求带上（见 `take_background_results`）
        if let Ok(mut v) = BACKGROUND_DONE.lock() {
            v.push(text);
        }
        emit(json!({
            "type": "system",
            "subtype": "background_done",
            "id": id2,
            "tool_use_id": ctl.tool_use_id,
            "label": label,
            "exit_code": code,
            "cancelled": cancelled,
        }));
    });
    id
}

/// 处理前端发来的 `tool_control`（2026-10-01）。返回是否认领了这条消息。
///
/// 动作：`background` = 把**正在跑**的某条命令转后台；`stop` = 把某条命令杀掉
/// （后台的也算 —— 它仍在看同一个标志）；`stdin` = 把用户在终端里键入的字符
/// （2026-10-05，`data` 字段）灌进那条命令子进程的 stdin。**认不出来的动作一律忽略**
/// （认领但不报错）：这条通路以后还会加动作，老前端发来未知动作不该把 agent 搞挂。
fn route_tool_control(msg: &Value) -> bool {
    if msg.get("type").and_then(Value::as_str) != Some("tool_control") {
        return false;
    }
    let action = msg.get("action").and_then(Value::as_str).unwrap_or("");
    let id = msg.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
    if id.is_empty() {
        return true;
    }
    let ctl = SHELL_CONTROLS.lock().ok().and_then(|m| m.get(id).cloned());
    match (action, ctl) {
        ("background", Some(c)) => {
            c.background.store(true, Ordering::Relaxed);
            log::info(format!("tool_control: 命令 {id} 转后台"));
        }
        ("stop", Some(c)) => {
            c.stop.store(true, Ordering::Relaxed);
            log::info(format!("tool_control: 命令 {id} 停止"));
        }
        ("stdin", Some(c)) => {
            // 交互式终端（2026-10-05）：把用户键入的字符灌进子进程 stdin。
            // **不逐次记日志** —— 每个按键一行会把 agent 日志冲垮。
            let data = msg.get("data").and_then(Value::as_str).unwrap_or("");
            c.write_stdin(data.as_bytes());
        }
        _ => log::warn(format!(
            "tool_control: 动作 `{action}` 找不到对应的运行中命令（id={id}）—— 忽略"
        )),
    }
    true
}

/// 取走「已完成但还没交给模型」的后台输出（按完成顺序）。没有就返回空。
fn take_background_results() -> Vec<String> {
    BACKGROUND_DONE
        .lock()
        .map(|mut v| std::mem::take(&mut *v))
        .unwrap_or_default()
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
//           "subtype":"can_use_tool","tool_name":"Cmd","input":{…},"tool_use_id":"…"}}
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

/// 写入类工具的**安全告警载荷**（A17，2026-10-01）：`[{rule, line}]`，没有命中就是空数组。
///
/// **它与审批卡那份是同源的**（同一个 `written_payload` + `content_safety::analyze`）：
/// 两处各写一遍必然漂移 —— 用户在审批卡上看见命中、结果卡上却什么都没有，比不标更糟。
/// 单独算一次（而不是把审批的结果捎过来）是**有意**的：hook 预先放行时 `open_approval`
/// 根本不会被调用，那条路也必须有告警。
///
/// 只覆盖 `Write` / `Edit`。`Cmd` / `PowerShell` 扫的是**命令**、走 `bash_safety`，
/// 那是另一套规则集（见 §13.1「两个方向别混」）。
fn security_warnings(tool_name: &str, input: &Value) -> Value {
    if !matches!(tool_name, "Write" | "Edit") {
        return json!([]);
    }
    match written_payload(tool_name, input) {
        Some(text) => content_safety::analyze(text).json_hits(),
        None => json!([]),
    }
}

/// 只登记 + 发请求，不阻塞 —— 一批工具先全部发出，前端才能把连续
/// Cmd 合并成一行（`findLastCmdGroup`）再让用户一次性决定。
///
/// `task_id`（A14）：**来自子代理内部**的审批要标明归属（`task-1` / `skill-2` …），
/// 主循环自己发起的调用传 `None` —— 那时不写该键，前端按普通卡渲染。
/// 理由：A14 起一轮里可能有**多个子代理同时**在等审批，前端拿到一串卡片若不带归属，
/// 用户点「全部允许」就分不清自己放行了谁的命令；批量卡也才有按任务分行/分组的依据。
fn open_approval(
    tool_name: &str,
    tool_use_id: &str,
    input: &Value,
    task_id: Option<&str>,
) -> Pending {
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
    // 子代理内部的审批：标明属于哪个子任务（主循环的调用没有这一项）
    if let Some(tid) = task_id {
        request["task_id"] = json!(tid);
    }
    // 命令类工具附上**执行侧**的静态安全分析（见 bash_safety.rs）：
    // 前端只拿到命令字符串，正则挡不住引号拼接 / 包装器 / 变量 / 串联的后半段。
    // 这里只**上报判定**、不代替前端决策 —— 前端仍是「自动放行 / 弹审批」的唯一决策点。
    if matches!(tool_name, "Cmd" | "PowerShell") {
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

/// 处理服务端反向发来的 `elicitation/create`（A13）：把「向用户提问」这件事接到
/// **既有的审批通道**上 —— 前端照常收 `control_request` / 回 `control_response`，
/// agent 侧不必为它另开一条协议。
///
/// 与 `can_use_tool` 的区别只在 `request.subtype`（`elicitation`）与载荷形状：
/// `message` + `requestedSchema`（JSON Schema，前端据此渲染表单字段）。
/// 回包复用 `respondPermission` 的 `updatedInput` —— 它就是用户填进表单的值。
///
/// 返回 MCP 规范要求的 elicitation 结果：`accept` + `content`（用户填的值）/
/// `decline`（用户点了拒绝或超时）。
fn open_elicitation(params: &Value) -> Value {
    let request_id = next_request_id();
    let (tx, rx) = mpsc::channel::<Value>();
    if let Ok(mut reg) = pending_approvals().lock() {
        reg.insert(request_id.clone(), tx);
    }
    log::debug("等待 elicitation 用户输入");
    emit(json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "elicitation",
            "message": params.get("message").cloned().unwrap_or(json!("")),
            "requestedSchema": params.get("requestedSchema").cloned().unwrap_or(json!({})),
        }
    }));
    // 复用审批的等待/超时/取消逻辑：original 传空对象，`updatedInput` 非空即用户填的值。
    match await_approval(Pending { request_id, rx }, &json!({})) {
        Decision::Allow(content) => json!({ "action": "accept", "content": content }),
        Decision::Deny(..) => json!({ "action": "decline" }),
    }
}

/// 项目 MCP 的**信任询问**（q3 第 3 步）：把「这个项目的 `.mcp.json` 要在本机跑这些命令，
/// 允许吗」接到**既有的审批通道**上 —— 前端照常收 `control_request` / 回 `control_response`，
/// agent 侧不为它另开协议（与 `open_elicitation` 同一条路子）。
///
/// 与 `can_use_tool` 的区别只在 `request.subtype`（`mcp_trust`）与载荷形状：
/// `project`（项目路径）+ `servers`（每台的 `name` / `transport` / `target` / `fingerprint`）。
/// 回包复用 `respondPermission` 的 `behavior`；`updatedInput.remember: true` = 用户选了
/// 「始终信任」（要写进信任记录）。**超时按拒绝处理**（与工具审批同一条纪律）。
///
/// 返回 `(是否放行, 是否记住)`。
fn ask_mcp_trust(project: &str, servers: &[mcp::ProjectServerInfo]) -> (bool, bool) {
    let request_id = next_request_id();
    let (tx, rx) = mpsc::channel::<Value>();
    if let Ok(mut reg) = pending_approvals().lock() {
        reg.insert(request_id.clone(), tx);
    }
    let list: Vec<Value> = servers
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "transport": s.transport,
                "target": s.target,
                "fingerprint": s.fingerprint,
            })
        })
        .collect();
    log::info(format!(
        "项目 MCP 信任询问：{} 台服务器（{project}）",
        servers.len()
    ));
    emit(json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "mcp_trust",
            "project": project,
            "servers": list,
        }
    }));
    match await_approval(Pending { request_id, rx }, &json!({})) {
        Decision::Allow(v) => {
            let remember = v.get("remember").and_then(Value::as_bool).unwrap_or(false);
            (true, remember)
        }
        Decision::Deny(..) => (false, false),
    }
}

/// 首次查询前把项目 `.mcp.json` 的服务器接进来（q3 第 3 步）。
///
/// **为什么推迟到这里**：信任询问要走审批通道，而那条通道要 stdin 读取线程就绪 ——
/// 启动时（`McpSet::connect` 那一刻）发 `control_request` 没人回。第一次用户消息到达时
/// stdin 早已在跑，此刻问才问得着。
///
/// 三条判据：
///   ① 没有项目服务器 / 只读（plan）档 ⇒ 什么都不做（只读档本来就不把 MCP 工具放进工具池）；
///   ② 全部已在信任记录里 ⇒ 直接接，不打扰用户；
///   ③ 否则弹**一次**信任卡 —— 同意则接（选了「始终信任」就记进信任记录），拒绝则本次会话不接。
///
/// 接上之后把新工具 append 进工具池。**工具表是固定前缀**，所以这一步必须发生在
/// **第一问发出之前**（之后不再变）—— 这正是它挂在「首次查询前」而不是「随时」的原因。
fn connect_project_servers(
    cwd: &Path,
    servers: &mcp::ProjectServers,
    trust: &mut mcp::TrustStore,
    bridge: &mut Option<mcp::McpSet>,
    tool_defs: &mut Vec<Value>,
    tool_names: &mut Vec<String>,
    disallowed: &[String],
    read_only: bool,
) {
    if servers.is_empty() || read_only {
        return;
    }
    let project = cwd.to_string_lossy().to_string();
    let infos = servers.servers();
    let all_trusted = infos
        .iter()
        .all(|s| trust.is_trusted(&project, &s.fingerprint));
    let mut allowed = all_trusted;
    if all_trusted {
        // 「没弹卡」有两种可能，日志里必须能分开：**这一条** = 已在信任记录里（正常的
        // 静默接通）；**既没有这一条、也没有下面的「信任询问」** = 配置压根没被读到，
        // 或当前进程读的还是旧配置（`.mcp.json` 只在 agent 启动时读一次 ⇒ 改完必须
        // 重启 agent）。2026-10-06 实测正是后者，而当时两条信息分居两个日志文件。
        log::info(format!("项目 MCP：{} 台已在信任记录里，直接接通", infos.len()));
    } else {
        let (ok, remember) = ask_mcp_trust(&project, &infos);
        allowed = ok;
        if ok && remember {
            let fps: Vec<String> = infos.iter().map(|s| s.fingerprint.clone()).collect();
            if let Err(e) = trust.trust(&project, &fps) {
                // 记不住只是「下次要重新问」——不该让这一次的信任白费
                log::warn(format!("项目 MCP 信任记录写不进：{e}"));
            }
        }
    }
    if !allowed {
        log::info("项目 MCP：未获信任，本次会话不接（改信任后重启 agent 生效）");
        return;
    }
    let added = bridge
        .get_or_insert_with(mcp::McpSet::empty)
        .add_project(servers, disallowed);
    if added.is_empty() {
        return;
    }
    tool_names.extend(tools::names(&added));
    tool_defs.extend(added);
}

/// 把目录转成 MCP roots 要的 `file:///` URI（Windows 路径的反斜杠换成正斜杠）。
/// 只用于把工作区根回给服务端当 shell handler 的 cwd —— 不做 URL 百分号编码，
/// 因为桥两端都是本机进程、路径由本进程给出（不走网络）。
fn dir_to_file_uri(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
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
    if matches!(tool, "Cmd" | "PowerShell") {
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
    // 出图落盘目录（A13，2026-10-03）：`<exe 根>\temp\images`。与 output_dir 同理进可访问
    // 范围 —— 不进的话，模型「生成完想再 Read 看一眼」会被工作区锁直接拒掉。
    let image_dir = image::prepare_image_dir();
    log::info(format!("出图落盘目录: {}", image_dir.display()));
    add_dirs.push(image_dir);
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
    // 插件目录（<exe 根>\Modules，2026-09-28）同理进可访问范围：Lunac 要能**自己写插件**
    // （写 `<id>\lunac-plugin.json` + `index.js`），而它在工作区之外 —— 不进这个名单，
    // 工作区锁会直接拒掉写入，「自建插件」就永远做不成。
    // 注意这只是**放行应用自己的目录**，没有放宽用户工作区的边界。
    if let Ok(dir) = std::env::var("LUNAC_MODULES_DIR") {
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
        // 实时输出回传**不在这里装**：它按「每一次工具调用」装上（见 run_one_tool），
        // 因为回调要绑死那一次的 tool_use_id。实时控制（后台运行 / 停止）同理。
        tool_output: None,
        shell_control: None,
    };

    // P3 MCP 工具桥：把 <exe 根>\tools\*.json 的用户工具接进工具池。
    // 连不上只是少一批工具 —— 内置工具必须照常可用，所以这里只记一行。
    //
    // **plan（只读）档也建桥，但不把 `mcp__*` 工具放进工具池**（2026-09-19 调整）：
    // 建桥的理由是桥上还挂着两个**自定义方法**（`lunac/history_index` /
    // `lunac/history_search`），它们支撑只读工具 `SessionSearch` —— 只读档查自己的历史
    // 完全正当。不列工具的理由没变：MCP 工具在只读档一律会被 `dispatch_tool` 拒掉，
    // 列出来只会让工具清单随档位漂移、白占固定前缀。
    // 远端 MCP 服务器（2026-10-01）：配置写在 `config\mcp.json`，宿主任一情况都注入
    // 路径（`LUNAC_MCP_FILE`，文件不存在 = 没配）。与宿主那条 stdio 桥一起汇进 `McpSet`：
    // 上层只认这一个集合（有没有工具 / 调哪个工具），内部按工具名路由到对应的连接。
    let remote_servers = mcp::load_config();
    // 项目级 MCP（q3 第 2/3 步，2026-10-06）：读 `<工作目录>\.mcp.json`，但**不在这里连** ——
    // 项目服务器要过信任门，而信任询问要走审批通道、那条通道要 stdin 就绪（见
    // `connect_project_servers`）。这里只把配置读进来，真正的连接推迟到**首次查询前**。
    let project_servers = mcp::load_project_config(&tools_ctx.cwd);
    // MCP 配置改动监视（2026-10-06）：拍一次**启动快照**，之后收到 user 消息时比对 mtime，
    // 变了就上报 `system/mcp_config_changed`，由**前端在回合结束后**重启 agent。
    // 为什么是「上报 + 前端重启」而不是 agent 自己重启：agent 是宿主的子进程、重启不了自己；
    // 工具表又是固定前缀，配置改了没法热更（详见 `mcp::ConfigWatch`）。
    // ⚠️ 用 `Option` 包着：**报过一次就置 `None`**（见 `mcp_config_changed_event`）——
    // 「只检测一次」是字面意思：发现并上报之后，主循环连这个函数都不再调。
    let mut mcp_config_watch = Some(mcp::ConfigWatch::snapshot(&tools_ctx.cwd));
    if !project_servers.is_empty() {
        // ⚠️ 这里必须走 `log::info`（**agent 日志**）而不是 eprintln（宿主日志）：
        // 这条与下面那句「信任询问」是同一件事的两半，分居两个日志文件时，用户问
        // 「为什么改了 `.mcp.json` 没弹卡」就无从查起 —— 2026-10-06 实测踩到：改完文件
        // 没重启 agent（配置只在启动时读一次），只能靠文件 mtime 反推。
        // **带上指纹**：拿它与 `config\mcp-trusted.json` 里那份一比，立刻能判断
        // 「这次进程读到的配置是新的还是旧的」。
        let infos = project_servers.servers();
        log::info(format!(
            "项目 MCP 配置：{} 台待过信任门 —— {}",
            infos.len(),
            infos
                .iter()
                .map(|s| format!("{}[{}] {} fp={}", s.name, s.transport, s.target, s.fingerprint))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    let mut mcp_trust = mcp::TrustStore::load();
    if !remote_servers.is_empty() {
        eprintln!(
            "[agent] MCP 配置里有 {} 台远端服务器: [{}]",
            remote_servers.len(),
            remote_servers
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    // A13：把宿主这条 stdio 桥要用的两样东西备好 ——
    //   · `roots`   —— 工作区根（`file:///` URI），服务端拿它当 shell handler 的 cwd；
    //   · `elicit`  —— 服务端反向发 `elicitation/create` 时走的弹卡钩子（接审批通道）。
    // 两者只对宿主那条桥有意义：远端 HTTP 服务器不声明这两项能力（见 mcp.rs 的 handshake）。
    let bridge_roots = vec![dir_to_file_uri(&tools_ctx.cwd)];
    let bridge_elicit: Option<mcp::ElicitHandler> =
        Some(Box::new(|params: &Value| open_elicitation(params)));
    let mcp_set = mcp::McpSet::connect(
        cli.mcp_server.as_deref(),
        &remote_servers,
        &cli.disallowed,
        bridge_roots,
        bridge_elicit,
    );
    if !mcp_set.is_empty() {
        let defs = mcp_set.defs();
        eprintln!(
            "[agent] MCP 已接通，用户工具 {} 个{}{}",
            defs.len(),
            if tools_ctx.read_only {
                "（只读档：不接入工具池）"
            } else {
                ""
            },
            tools::names(&defs).join(",")
        );
    }
    // `Remember` / `SessionSearch` / resources 读侧走的是**宿主那条 stdio 桥**
    // （`lunac/*` 自定义方法与 `tools\*.json` 都在它那边）。判据必须落在「有没有那条桥」上，
    // 不能只看集合空不空 —— 只配了远端服务器时集合非空，但那些方法必然失败。
    let has_lunac_bridge = mcp_set.has_lunac();
    // 集合整体为空 = 一条连接都没成 ⇒ 与旧实现一样按「没有桥」处理（下游全是 `Option` 语义）。
    let mut mcp_bridge = if mcp_set.is_empty() { None } else { Some(mcp_set) };

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
        && has_lunac_bridge
        && !cli.disallowed.iter().any(|d| d == "Remember");
    let memory_section = memory_block(mcp_bridge.as_mut(), remember_on);
    // 用户人格 / 自定义提示词（L2，2026-09-21）：与 skills / 索引 / 记忆同款的**启动时读一次**。
    // 宿主经 `LUNAC_PERSONA_FILE` 给出 `config\persona.md` 的路径；面板上保存后要**重启 agent**
    // 才生效 —— 热读会让系统提示词每轮都变、把整个固定前缀的缓存打掉（规则 18/23），
    // 而「重启才生效」这件事在面板上如实写明了。
    let user_persona = read_user_persona();
    if !user_persona.is_empty() {
        log::info(format!(
            "用户人格已注入系统提示词固定段：{} 字（{}）",
            user_persona.chars().count(),
            std::env::var(PERSONA_ENV).unwrap_or_default()
        ));
    }
    // 三块共享的段落先算一次（都是纯函数、与请求无关）：主提示词 / 子代理 / 复盘各自拼装，
    // 但环境块与技能清单**逐字节同源**，不各算一遍（规则 18 的「固定前缀」纪律）。
    //
    // 项目记忆（`AGENTS.md`，2026-10-01）**拼进 env_section**，不另开一路：三份提示词都取
    // 这一份字符串 ⇒ 子代理也拿得到项目约定（它同样在改文件），且三处逐字节一致。
    // 没有 `AGENTS.md` / 文件为空 ⇒ `project_block()` 返回空串，这一行与改造前逐字节相同。
    let env_section = format!(
        "{}{}",
        env_block(&tools_ctx.cwd),
        project_block(&tools_ctx.cwd)
    );
    let skills_section = skills::listing(if skills_on { &skills } else { &[] });
    let system_prompt = build_system_prompt(&user_persona, &env_section, &skills_section);
    // 上下文块（往期会话索引 + 长期记忆）：**不进 system**（2026-10-06，M2-16 方案 D，
    // 位置与代价见 `context_block_message`）。它由 `run_query` 幂等注入 history 开头。
    let ctx_block = context_block_message(&history_block, &memory_section);
    // 复盘 fork 的系统提示词 = 角色段 + 环境块 + 技能清单（与子代理同构，**不含记忆**：
    // 当前记忆每次都不同，放进复盘的**用户消息**里，系统提示词才能跨次逐字节相同）。
    let review_system = build_review_system(&env_section, &skills_section);
    // 子代理的系统提示词 = 角色段 + 环境块 + 技能清单。**与主提示词同源、只构建一次**，
    // 因此所有子代理共享一段逐字节相同的前缀（ai-spec §11 规则 18）。
    //
    // 为什么必须补后两块（2026-09-20 复查）：原实现只发 `SUBAGENT_SYSTEM` 角色段，于是
    //   ① 子代理**不知道自己的工作目录绝对路径** —— 它看不到主对话的环境块，只能指望
    //      调用方在 `prompt` 里手抄一遍 cwd（探针里模型确实手抄了，但不能指望每次都记得；
    //      抄错了它还会照着一个不存在的位置去 Glob）；
    //   ② `Skill` 在子代理的工具集里（技能是纯本地能力，与 MCP 桥无关），而**技能清单
    //      原本只写在主提示词里** —— 不给清单，等于给一串不知道有哪些钥匙的钥匙串。
    let subagent_system = build_subagent_system(&env_section, &skills_section);

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
    // 出图（A13，2026-10-03）：**条件注册** —— 宿主配了 `config\ai.json` 的 image_model
    // 才会注入 `LUNAC_IMAGE_MODEL`。没配就注册，等于在固定前缀里放一件必然失败的工具
    // （与 `Remember` / resources 读侧同一条纪律，见 §11 规则 18 ⑤）。
    if image::enabled() && !cli.disallowed.iter().any(|d| d == "ImageGen") {
        tool_defs.push(image::tool_def());
        tool_names.push("ImageGen".into());
    }
    if let Some(b) = &mcp_bridge {
        // 只读（plan）档**不把 MCP 工具放进工具池**（桥仍然戴着，供 SessionSearch 用）——
        // 理由见上面建桥处的注释。
        if !tools_ctx.read_only {
            tool_defs.extend(b.defs());
            tool_names.extend(tools::names(&b.defs()));
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
        for def in [
            tools::list_resources_tool(),
            tools::read_resource_tool(),
            tools::list_prompts_tool(),
            tools::get_prompt_tool(),
        ] {
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
                    // 运行中命令的实时指令（2026-10-01，`tool_control`）：同样是「不占
                    // 对话、只改某个正在跑的东西的状态」，所以也不进主队列 ——
                    // 进队列只会让它排在用户消息后面，等轮到它时那条命令早跑完了。
                    if route_tool_control(&v) {
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
    // ── 跨提问的前缀归因状态（A16，2026-09-21）─────────────────────────
    // `prev_hist_hashes` = 上一次请求的 history **逐条**指纹，必须**跨提问**存活：
    // 原先它声明在 `run_query` 内部（局部）⇒ 每一问的首请求都看不到上一问末请求的
    // 指纹，日志里只剩 `公共前缀=0/0条`，于是「两次提问之间本侧有没有改写 history」
    // 这件事**在日志里根本没有证据**（这正是 A16 一开始卡住的地方）。
    // `question_seq` = 第几次用户提问（刻意**不复用** `questions`：那个只为复盘门槛
    // 计数、递增时机不同），只进日志，让离线时能一眼认出「本问首请求」。
    let mut prev_hist_hashes: Vec<u64> = Vec::new();
    let mut question_seq: u64 = 0;
    // ── 后台复盘（A4）的装配状态 ────────────────────────────────────
    // `questions` = 已完成的**用户提问**数（不是工具轮次：一次提问内部可以有 16 轮工具
    // 往返，按那个计数会在一次长提问中途触发复盘，而那时复盘看到的还是半截对话）。
    // `pending` = 上一次复盘还没收回来。**收的时机是「处理下一条消息之前」，且只收
    // 已经跑完的**（见 `collect_review`）—— 复盘是后台事务，不许它决定前台时延。
    let mut questions: u64 = 0;
    let mut review_seq: u64 = 0;
    let mut pending_review: Option<thread::JoinHandle<Result<String, String>>> = None;
    // 项目 MCP 的信任门**只过一次**（首次真实提问前）：接上后工具表就是固定前缀的一部分，
    // 之后不再变（见 `connect_project_servers`）。
    let mut project_trust_done = false;
    // 主循环改成「带超时的接收」（2026-10-06）：`for msg in rx` 在空闲时会一直阻塞，而
    // 「**回合外**改了 MCP 配置」只有靠**空闲醒来**才能发现 —— 否则要等用户下次提问才发现，
    // 那时工具表已定死，只能白答一轮再重启（实测踩到，见 ai-spec §20.1）。
    // 超时分支不处理任何消息，只做一次 watcher 比对（报过后 `Option` 已置 `None`，零成本）；
    // 断开与原「迭代器结束」语义相同。
    loop {
        let msg = match rx.recv_timeout(MCP_WATCH_TICK) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => {
                if let Some(ev) = mcp_config_changed_event(&mut mcp_config_watch, true) {
                    emit(ev);
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
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
                // ── MCP 配置改动（回合内兜底，2026-10-06）──
                // 正常路径是**主循环空闲轮**先发现（`idle: true`，前端立即重启）；这一条是兜底：
                // 「改完文件后不到一个 tick 就提问」那类空档没被空闲轮抓到的情况。它带
                // `idle: false` ⇒ 前端会等本回合 `result` 收尾再重启（工具表已定死，就地重建的
                // 代价与风险 —— 拆桥 / 重弹信任卡 —— 远大于「答完这一回合再重启」）。
                if let Some(ev) = mcp_config_changed_event(&mut mcp_config_watch, false) {
                    emit(ev);
                }
                // ── 项目 MCP：首次查询前过信任门并接通（q3 第 3 步）──
                // **只做一次**（`project_trust_done`）：工具表是固定前缀，接上后就不再变。
                // 放在这里（而不是启动时）是因为信任询问要走审批通道、那条通道要 stdin 就绪。
                if !project_trust_done {
                    project_trust_done = true;
                    connect_project_servers(
                        &tools_ctx.cwd,
                        &project_servers,
                        &mut mcp_trust,
                        &mut mcp_bridge,
                        &mut tool_defs,
                        &mut tool_names,
                        &cli.disallowed,
                        tools_ctx.read_only,
                    );
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
                question_seq += 1;
                run_query(
                    &cfg,
                    &mut history,
                    &mut prev_hist_hashes,
                    question_seq,
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
                    ctx_block.as_ref(),
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
pub(crate) fn image_media_type(bytes: &[u8]) -> Option<&'static str> {
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

/// 需要审批的工具：内置写类四件（Write/Edit/Cmd/PowerShell）+ WebSearch /
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
    mcp_bridge: Option<&mut mcp::McpSet>,
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
fn session_search(bridge: Option<&mut mcp::McpSet>, input: &Value) -> Result<String, String> {
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
    /// 多代理通信（A13，2026-10-05）：`Some(描述)` = 把这次 fork 登记进 peer 登记处
    /// （同批兄弟可 `ListPeers` 看到它、`SendMessage` 投给它）；`None` = 不登记。
    /// 前台子代理（`Agent` / fork 技能）传 `Some`，**后台复盘传 `None`** —— 它在提问
    /// 之间单独跑、没有兄弟，也不该出现在别人的列表里。
    peer: Option<&'a str>,
}

/// 子代理退出时把 peer 摘掉（RAII）。`thread::scope` 的 `join()` 会吞掉 worker panic ⇒
/// 手写注销会留下僵尸 peer，之后 `SendMessage` 就一直往一个没人读的收件箱里投。
struct PeerReg(String);

impl Drop for PeerReg {
    fn drop(&mut self) {
        peers::bus().unregister(&self.0);
    }
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
    mut bridge: Option<&mut mcp::McpSet>,
    prompt: &str,
) -> Result<String, String> {
    let task_id = spec.task_id;
    let max_rounds = spec.max_rounds;
    let budget_tokens = spec.budget_tokens;
    // 多代理通信（A13，2026-10-05）：把本线程标记为「正在跑 <task_id>」——
    // `ListPeers` / `SendMessage` 靠它认出「我是谁」（主循环线程上为 `None` = 主代理）。
    // guard 在返回时还原：串行批里子代理就跑在主线程上，必须还原成「主代理」，
    // 否则主循环后续调用 ListPeers 会把自己当成某个已结束的子代理。
    let _me = peers::enter(task_id);
    // 登记为存活 peer（`spec.peer` 为 `None` 时不登记，见字段注释）。登记点放在**这里**
    // 而不是各调用方：`Agent` 与 fork 技能会合进同一个并发批（A14），两者都必须登记，
    // 否则会出现「A 看得见 B、B 却看不见 A」的不对称。
    let _peer_reg = spec.peer.map(|desc| {
        peers::bus().register(task_id, desc);
        PeerReg(task_id.to_string())
    });
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
        // 多代理通信（A13，2026-10-05）：先把兄弟投来的消息读进本子代理的 history
        // （每轮取一次，取走即清空）再发请求，这样本轮就能看到它。按 §11 规则 54 的口径，
        // 消息只进**本子代理**的 history，绝不回灌主对话 —— 通道只存在于兄弟之间。
        for (from, content) in peers::bus().drain(task_id) {
            log::info(format!(
                "子代理 {task_id} 收到 {from} 的消息（{} 字）",
                content.chars().count()
            ));
            history.push(json!({
                "role": "user",
                "content": [{ "type": "text", "text": format!("[message from {from}]\n{content}") }],
            }));
        }
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
            let (r_in, r_out, r_read, r_create) = (
                n("input_tokens"),
                n("output_tokens"),
                n("cache_read_input_tokens"),
                n("cache_creation_input_tokens"),
            );
            spent += r_in + r_out + r_read + r_create;
            // 同时记进**主循环那本账**（2026-09-29）：子代理的 token 平台照收钱，
            // 此前却完全没进 `result.usage` ⇒ 本地用量日志与账单对不上。
            cfg.sub.record(r_in, r_read, r_create, r_out);
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
                            _ => match await_approval(
                                open_approval(name, id, input, Some(task_id)),
                                input,
                            ) {
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
            // 前台子代理登记进 peer 登记处（同批兄弟可互相 ListPeers / SendMessage）
            peer: Some(desc),
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
/// 里面可以写任意 `Cmd`，放它出去等于把「只读」这个承诺交给第三方的 md 文件去守。
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
///   · `ask_permission` 随前端运行方式：技能里写 `Cmd` / `Write` 时，子代理内部**逐次**
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
            // fork 技能与 `Agent` 会合进同一个并发批 ⇒ 同样登记，否则会出现
            // 「Agent 看得见 fork 技能、fork 技能看不见 Agent」的不对称。
            peer: Some(skill.key.as_str()),
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
    // **为什么复盘要自己连一条桥**：主循环那条 `&mut McpSet` 归主线程所有，跨线程借用
    // 做不到；而复盘要写的 `Remember` 走的是桥上的 `lunac/memory_write` —— 那是长期记忆
    // **唯一**的写入通道（不给 agent 侧另开一条私有文件通道，避免两套真相源）。
    // 代价：多起一个 `lunac.exe --mcp-server` 子进程（握完手就退出），只在每 N 轮一次。
    //
    // **只连 stdio 那条**（`&[]` 远端服务器）：复盘只用到 `lunac/memory_write`，
    // 为它去连一批远端服务器是纯浪费（而且那些服务器根本没有这个方法）。
    let mut bridge = match job.bridge_spec.as_deref() {
        Some(spec) => {
            let set = mcp::McpSet::connect(Some(spec), &[], &[], Vec::new(), None);
            if set.has_lunac() {
                Some(set)
            } else {
                log::warn("复盘 fork 的 MCP 桥没连上（只剩判断，写不进记忆）");
                None
            }
        }
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
            // 复盘不登记为 peer：它在提问之间单独跑、没有兄弟，也不该出现在别人的列表里
            peer: None,
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
Read-class tools still work; every write-class tool (Write / Edit / Cmd / PowerShell / Agent / \
MCP tools) is REFUSED until then. Gather what you need with Read / Glob / Grep, then present the \
COMPLETE plan with ExitPlanMode.";

const PLAN_MODE_OFF_NOTE: &str = "The user approved your plan. Plan mode is OFF — write-class \
tools work again. Execute the plan task by task in the order you wrote it, and keep the user \
posted with TodoWrite. If reality contradicts the plan (a file is not where you expected), say \
so instead of improvising silently.";

fn dispatch_tool(
    tctx: &tools::Ctx,
    mcp_bridge: Option<&mut mcp::McpSet>,
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
            // prompts 读侧（A13）：与 resources 读侧同型 —— 只读本机自己的
            // 提示词模板（`<exe 根>\prompts\*.md`）。
            "ListMcpPromptsTool" => bridge.list_prompts(),
            "GetMcpPromptTool" => {
                let prompt_name = input
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if prompt_name.is_empty() {
                    Err("缺少 name 参数：先调 ListMcpPromptsTool 拿一个 prompt 名".into())
                } else {
                    let args = input.get("arguments").cloned().unwrap_or(json!({}));
                    bridge.get_prompt(&prompt_name, &args)
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
///
/// **Read 读到图片时**（2026-10-05，用户要求）：`tools::read` 会在文本最前面放
/// `tools::IMAGE_SENTINEL` + 绝对路径，这里把它换成**真正的 image 块** —— 视觉模型据此
/// 直接「看」到图，而不是读一串被 UTF-8 解坏的二进制。建块失败（超限 / 其实不是图片）
/// 就退回纯文本，绝不把 tool_result 弄空。
fn tool_result_block(id: &str, text: String, is_error: bool) -> Value {
    if !is_error {
        if let Some(rest) = text.strip_prefix(tools::IMAGE_SENTINEL) {
            if let Some((path_line, note)) = rest.split_once('\n') {
                let blk =
                    json!({ "type": "image", "source": { "type": "file", "path": path_line.trim() } });
                return match load_image_block(&blk) {
                    Ok(image) => json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": [
                            { "type": "text", "text": note.trim() },
                            image,
                        ],
                    }),
                    Err(_) => json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": note.trim(),
                    }),
                };
            }
        }
    }
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
    mcp_bridge: Option<&mut mcp::McpSet>,
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
    // 命令类工具的实时输出（2026-09-30）：为**这一次调用**装上一个绑死 `tool_use_id` 的
    // 回传回调，前端因此能边跑边看到输出。只给 Cmd / PowerShell 装 —— 其余工具没有
    // 「边跑边出」这回事，装上就只是白白克隆一次 Ctx。
    let mut call_ctx = tctx.clone();
    if matches!(name, "Cmd" | "PowerShell") {
        call_ctx.tool_output = Some(tool_output_sink(tool_use_id));
        // 实时控制块（2026-10-01）：**先登记再跑** —— 用户点「后台运行 / 停止」时，
        // `route_tool_control` 就是靠这张表按 `tool_use_id` 找回这条命令的。
        // 只为 shell 类工具建：别的工具没有「跑一半可以打断」这回事。
        let ctl = Arc::new(tools::ShellControl::new(tool_use_id));
        if let Ok(mut m) = SHELL_CONTROLS.lock() {
            m.insert(tool_use_id.to_string(), Arc::clone(&ctl));
        }
        call_ctx.shell_control = Some(ctl);
    }
    let (mut text, is_error) = match run_tool(&call_ctx, mcp_bridge, skill_list, name, run_input) {
        Ok(s) => (s, false),
        Err(e) => (format!("Error: {e}"), true),
    };
    // 收尾注销：跑完 / 被停的必须摘掉（否则表里留僵尸），**转后台的那条绝不能摘** ——
    // 命令还在跑，用户仍要从待办清单里取消它，而取消走的就是这张表。
    // 转后台那条由后台收尾线程负责摘（见 `hand_off_to_background`）。
    if let Some(ctl) = &call_ctx.shell_control {
        if !ctl.handed_off.load(Ordering::Relaxed) {
            if let Ok(mut m) = SHELL_CONTROLS.lock() {
                m.remove(tool_use_id);
            }
        }
    }

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

/// 跑一个「自己发 API 请求」的调用（`Agent` / fork 技能），返回 `(tool_result 文本, is_error)`。
///
/// 存在的理由：串行批与子代理并行批**必须走同一条实现**。此前这段 `if forked { … } else
/// if Agent { … }` 长在批循环里，A14 要再写一遍并发版 —— 两处各判一次「哪个是 fork」，
/// 迟早一边改一边忘。现在判定收口到 `subagent_call()`、执行收口到这里。
fn run_subagent_call(
    cfg: &Cfg,
    tctx: &tools::Ctx,
    subagent_defs: &[Value],
    skill_list: &[skills::Skill],
    subagent_system: &str,
    ask_permission: bool,
    kind: SubagentCall<'_>,
    input: &Value,
    denied: Option<&str>,
) -> (String, bool) {
    match kind {
        SubagentCall::Fork(sk) => run_forked_skill(
            cfg,
            tctx,
            subagent_defs,
            skill_list,
            subagent_system,
            ask_permission,
            sk,
            input,
            denied,
        ),
        SubagentCall::Agent => run_agent_tool(
            cfg,
            tctx,
            subagent_defs,
            skill_list,
            subagent_system,
            ask_permission,
            input,
            denied,
        ),
    }
}

/// 一批工具调用的执行方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchKind {
    /// 一个一个来（写类 / 命令 / MCP / 单个调用）。顺序即语义，不能重排。
    Serial,
    /// 批内并行 —— **只读工具**（`tools::parallel_safe` 白名单）。
    ReadOnly,
    /// 批内并行 —— **子代理**（`Agent` / fork 技能，A14）：各自发自己的 API 请求、
    /// 跑独立工具循环。并发上限 `SUBAGENT_PARALLELISM`（比只读批低得多，见常量注释）。
    Subagent,
}

/// 一次调用的「自己发 API 请求」归属：`Agent` 与 fork 技能是**同一族**
/// （都要 `cfg` + 独立循环），所以判定与执行都必须走同一份结论 —— 各判各的必然漂移。
#[derive(Clone, Copy)]
enum SubagentCall<'a> {
    Agent,
    Fork(&'a skills::Skill),
}

/// 判定一次调用是不是子代理，并**解析出**它的形态（`None` = 普通工具调用）。
///
/// `Skill` 只有 **fork 模式**才算（`context: fork` 会派能写文件、能跑命令的子代理）；
/// inline 技能只是把 md 正文交回主循环，与 `Read` 同级 ⇒ 走普通通道。
/// 判据来源与 `needs_approval_with()` 一致 —— 都是「看入参里指向哪个技能」。
fn subagent_call<'a>(
    name: &str,
    input: &Value,
    skills: &'a [skills::Skill],
) -> Option<SubagentCall<'a>> {
    if name == "Agent" {
        return Some(SubagentCall::Agent);
    }
    if name == "Skill" {
        let want = input.get("skill").and_then(Value::as_str).unwrap_or("");
        return skills::find(skills, want).filter(|s| s.fork).map(SubagentCall::Fork);
    }
    None
}

/// 把一轮的工具调用切成执行批：**连续的**只读调用 / **连续的**子代理调用各自合成一批
/// （批内并行），其余各自成批（串行）。返回 `(批类型, 下标区间)`，区间按原顺序无缝覆盖全部调用。
///
/// **为什么必须是「连续」段**：批内会被重排，所以绝不允许跨越写类调用 —— 否则
/// 「写 A → 读 A」会被重排成「读 A（旧内容）→ 写 A」，错得无声无息。子代理同理由：
/// 它可能写文件（`Agent` 有 `Write` / `Cmd`），与前后调用之间存在真实的先后依赖。
/// 单元素段不标并行：省一次线程 spawn，行为与串行完全一致。
///
/// `subagents[i]` = 第 i 个调用解析出的子代理形态（`subagent_call()` 的结论，**只算一次**）：
/// 判定与执行共用它，避免「规划说是串行、执行却走了另一条分支」这类漂移。
fn plan_tool_batches(
    calls: &[(String, String, Value)],
    subagents: &[Option<SubagentCall<'_>>],
) -> Vec<(BatchKind, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < calls.len() {
        if subagents[i].is_some() {
            let start = i;
            while i < calls.len() && subagents[i].is_some() {
                i += 1;
            }
            out.push((if i - start > 1 { BatchKind::Subagent } else { BatchKind::Serial }, start..i));
        } else if tools::parallel_safe(&calls[i].1) {
            let start = i;
            while i < calls.len() && subagents[i].is_none() && tools::parallel_safe(&calls[i].1) {
                i += 1;
            }
            out.push((if i - start > 1 { BatchKind::ReadOnly } else { BatchKind::Serial }, start..i));
        } else {
            out.push((BatchKind::Serial, i..i + 1));
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

/// 两串「history **逐条**指纹」的公共前缀长度 —— A16 判据的核心（2026-09-21）。
///
/// 读法（与 `请求前缀` 行的 `公共前缀=N/M条` 同一语义）：
///   · `N == prev.len()` ⇒ **纯追加**：本侧没有就地改写历史，`read` 若掉了是端点侧的事；
///   · `N <  prev.len()` ⇒ **本侧在第 N 条改写了历史**，其后整段前缀缓存必然失效，
///     顺着 `上下文压缩` 行找是哪一档压的。
///
/// 之所以单独成函数：它的 `prev` 必须**跨提问**存活（由 `main()` 持有、`&mut` 传进来）。
/// 早先它是 `run_query` 的局部变量 ⇒ 每一问的首请求都拿不到上一问末请求的指纹，
/// 日志只会打 `0/0条`，「两次提问之间本侧有没有改写 history」就**永远无法判定**
/// —— 这正是 A16 一开始卡住的地方。
fn common_prefix_len(prev: &[u64], cur: &[u64]) -> usize {
    prev.iter().zip(cur.iter()).take_while(|(a, b)| a == b).count()
}

/// 归并一次请求的**输入侧**用量 `(input, cache_read, cache_creation)`。
///
/// 为什么需要它（2026-10-07 实测）：Anthropic 原生把三类输入放在 `message_start` 的
/// `message.usage` 里；但**中转端点可能在那里全填 0**，真值只在 `message_delta` 的
/// `usage` 上（实测 `api.moonobscura.com`：`message_start` 三类全 0，`message_delta`
/// 才给出 `input_tokens`）。只认 `message_start` 的话，输入 / 命中 / 写入三列**永远是 0**，
/// 命中率与金额一起失真（面板上就是用户看到的「没有数值」）。
///
/// 判据是「**有没有量**」而不是端点身份：`start` 有任何非零 ⇒ 用它（原生路径逐字节不变）；
/// 三类全 0 ⇒ 回落到 `delta`。因此对同一条 delta 重复归并是**幂等**的（第二次 `merged`
/// 等于现值，调用方据此跳过），不会把一次请求的输入计两遍。
fn merge_input_usage(start: (u64, u64, u64), delta: (u64, u64, u64)) -> (u64, u64, u64) {
    if start.0 + start.1 + start.2 > 0 {
        start
    } else {
        delta
    }
}

fn run_query(
    cfg: &Cfg,
    history: &mut Vec<Value>,
    // 上一次请求的 history 逐条指纹（**跨提问**存活，见 `main()` 里那处声明的注释）：
    // 每问的首请求因此能和**上一问的末请求**比，A16 的判据才立得住。
    prev_hist_hashes: &mut Vec<u64>,
    // 第几次用户提问（只进日志：`请求前缀` / `请求用量` 两行都带它）
    question_seq: u64,
    prompt: &str,
    raw_images: &[Value],
    tctx: &tools::Ctx,
    tool_defs: &[Value],
    tool_names: &[String],
    ask_permission: bool,
    mut mcp_bridge: Option<&mut mcp::McpSet>,
    skill_list: &[skills::Skill],
    system_prompt: &str,
    subagent_system: &str,
    // 上下文块（往期会话索引 + 长期记忆，见 `context_block_message`）。**不在 system 里**
    // —— 它随本问幂等注入 history 开头（2026-10-06，M2-16 方案 D）。
    ctx_block: Option<&Value>,
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

    // ── 上下文块幂等注入（2026-10-06，M2-16 方案 D）────────────────────
    // 注在 history **开头**、且**只注一次**（位置选择的理由见 `context_block_message`）。
    // 幂等判据是表头：`set_history`（回退 / 恢复会话）会整份换掉 history，注过的那条随之
    // 消失 ⇒ 下一问要能认出来并补回去，否则表现为「回退之后模型突然不记得长期记忆」。
    // 压缩若把它当旧消息丢掉，也会在这里补回（代价：那一次提问的前缀在此处失配一次）。
    // **必须排在 `base` 之前**：`base` 是「本轮压入的消息」的起点，注入的这条不属于本轮。
    if let Some(cb) = ctx_block {
        if !history.iter().any(is_context_block_msg) {
            history.insert(0, cb.clone());
        }
    }

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

    // ── 本次提问的成本预算（2026-10-01）──────────────────────────
    // `turn_tokens` 累计该提问内**所有**请求的 in+read+create+out（口径见
    // `DEFAULT_TURN_BUDGET_TOKENS` 的注释）。撞到后先「压缩续命」再清零计点，
    // `budget_extensions` 数的是续过几次（真上限在它，不在预算值）。
    let turn_budget = turn_budget_tokens();
    let mut turn_tokens: u64 = 0;
    let mut budget_extensions = 0usize;

    // ── 端点侧缓存冷/热（2026-10-01）────────────────────────────
    // 上次**发出请求**的时刻（不是响应返回的时刻：缓存是发出去那一刻开始算的）。
    // `None` = 本次提问还没发过请求 —— 那时保守按「热」处理（不激进压）。
    let mut last_req_at: Option<std::time::Instant> = None;
    let cache_ttl_secs = cache_ttl();

    // ── 卡住检测窗口（2026-10-01）──────────────────────────────
    // 每个元素 = 一轮的（唯一工具名, 结果指纹）。`push/take` 都按轮来，
    // 判据见 `STUCK_ROUNDS` 那一段。`stuck_hint_sent` 保证**只提示一次**。
    let mut stuck_window: std::collections::VecDeque<(String, String)> =
        std::collections::VecDeque::new();
    let mut stuck_hint_sent = false;
    // ── B 类崩塌归因（2026-09-20）：history 的**逐条**指纹 ──────────────
    // 整体 hash 只能回答「history 变没变」；而 history 每轮必然变（只追加也会变），
    // 所以它对「本侧有没有就地改写」**没有分辨力**。实测三条记录里，第 17 轮的
    // `read` 从 48000 掉到 8448，而同轮 history 只多了 3 条 / 528 字 —— 此时整体
    // hash 变了、字数也变了，按旧埋点完全无法定性是本侧改写还是端点侧淘汰。
    // 逐条指纹给出判据：`公共前缀/上轮条数`
    //   · 等于上轮条数 ⇒ 纯追加，本侧无责（read 掉了就是端点侧）；
    //   · 小于上轮条数 ⇒ **本侧就地改写了历史**，改在第 N 条，其后整段缓存必然失效。
    // **A16 起它跨提问存活**（由 `main()` 持有、按 `&mut` 传进来）：每一问的首请求
    // 直接与上一问的末请求比 —— 「两次提问之间本侧改没改 history」才能被判出来，
    // 局部声明时这里永远是 `0/0条`。
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

        // ── 端点侧缓存还热不热（2026-10-01）──────────────────────
        // 距上次**发出请求**已超过 TTL ⇒ 端点侧那份前缀缓存本来就过期了 ⇒ 此时
        // 动历史**零额外代价**，可以让「每轮微清理」上场、并免掉滞回。
        // ⚠️ 这条判据只**单向放宽**：冷的时候更敢压，热的时候绝不比原来更激进。
        // `None`（本次提问还没发过请求）按「热」处理 —— 没有依据就别激进。
        let cache_cold = last_req_at
            .map(|t| t.elapsed().as_secs() >= cache_ttl_secs)
            .unwrap_or(false);

        // ── 上下文水位检查（发请求之前）─────────────────────────
        // 用上一轮实测的输入体积作基准（比按字符估算准），过了 ELIDE 水位
        // 先瘦身、过了 DROP 水位才允许丢弃。
        //
        // 滞回（`last_compact`）：瘦身档必须间隔足够远才允许再动历史 ——
        // 否则每轮都有旧消息跨过保留尾部被瘦身，前缀每轮都变、缓存每轮归零。
        // 丢弃档（0.95）不受滞回约束：到了那个水位不压就可能 400，安全性优先。
        let measured = cfg.last_input.get();
        // 本轮是否真压动过 —— 预算续命拿它避免「重复压一次、压不动了误判成压不动」
        let mut compacted_this_round = false;
        if measured > 0 {
            let ratio = measured as f64 / budget as f64;
            let grew_enough =
                measured > cfg.last_compact.get() + (budget as f64 * COMPACT_MIN_GROWTH) as u64;
            if ratio > DROP_RATIO && grew_enough {
                // ⚠️ 丢弃档**同样要过滞回**（2026-10-02 补，原为「安全性优先，不受滞回约束」）。
                // 实测事故（backlog M2-16）：体积在**对话本身**（一条 83 万字的 tool_result
                // 落在保留尾部里）时，`Drop` 每次只能丢 2 条、体积根本不降 ⇒ 水位实测值
                // 一直贴着 0.95 ⇒ **每轮都触发一次**，而每次都会 drain + 钉快照 ⇒ **前缀每轮
                // 全废**。日志连出四条「本侧就地改写了历史」，4 次请求 `read` 只剩 4352
                // （命中率 7.4% → 2.3%）。
                // 现在与瘦身档同一条判据：**history 必须比上次压缩时再长大 15% 才允许再压**。
                // 真正的 400 兜底仍在（下面「上下文超限」那条强制 `Compact::Force`）——
                // 那是**按端点实际报错**触发，比「按我们猜的水位反复压」可靠得多。
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
                compacted_this_round = out.elided + out.dropped > 0;
            } else if cache_cold {
                // ── 冷缓存微清理（2026-10-01，用户要求的「每轮微清理」）──────
                // 缓存已过期 ⇒ **不看水位、不看滞回、不算成本模型**（Δ 的代价是 0，
                // 判据恒成立，算了也白算）。这正是 Claude Code `microCompact` 的
                // 冷路径：缓存本来就没了，此刻改内容不额外花钱。
                //
                // 为什么敢「每轮」都做：动作范围被三重收窄 —— 只动 `COMPACT_KEEP_TAIL`
                // 之外的、只动**可重现**工具的结果（`elidable_tool`）、只动超过
                // `ELIDE_TOOL_RESULT_CHARS` 的大块。所以它是**渐进**的，不是一次清空。
                let out = compact_history(history, Compact::ElideCold, measured);
                base = base.saturating_sub(out.dropped) + out.pinned;
                cfg.last_input.set(0);
                compacted_this_round = out.elided + out.dropped > 0;
                if out.elided > 0 {
                    cfg.last_compact.set(measured);
                }
            } else if ratio > ELIDE_RATIO && grew_enough {
                let out = compact_history(history, Compact::Elide, measured);
                base = base.saturating_sub(out.dropped) + out.pinned;
                cfg.last_input.set(0);
                // 闸门判定「不值得」时什么都没改 —— 那时**不推进滞回时钟**，
                // 否则会白等一个 15% 增长窗口才重新评估（压缩次数统计也不会被污染）。
                if out.elided > 0 {
                    cfg.last_compact.set(measured);
                }
                compacted_this_round = out.elided + out.dropped > 0;
            }
        }

        // ── 单次提问的成本预算（2026-10-01）──────────────────────
        // 放在水位检查**之后**：先把「可能撞 400」的隐患处理掉，再谈省钱。
        // 首次进入时 `turn_tokens == 0`（它在请求完成后才累加）⇒ 天然不触发。
        if turn_budget > 0 && turn_tokens >= turn_budget {
            if hint_sent {
                // 已经给过收口机会、模型还在调工具 ⇒ 硬收（与轮次档同一形态）
                eprintln!("[agent] 提问成本预算用尽且已给过收口机会，提前收尾");
                log::info(format!(
                    "成本预算用尽且收口提示无效，硬收尾（累计 {turn_tokens} tokens）"
                ));
                break;
            }
            // 先试「压缩续命」：Force 档压下去 ⇒ 历史变短 ⇒ 后续每轮更便宜 ⇒ 清零计点继续。
            // **必须真的压动了**才算数 —— 压不动说明体积在对话本身（不是工具结果），
            // 再压只是白废一次缓存 + 白花一次摘要钱（见 `MAX_BUDGET_EXTENSIONS` 的注释）。
            let compressed = if compacted_this_round {
                true
            } else {
                let measured_now = cfg.last_input.get();
                fire_plain_hook("PreCompact", &tctx.cwd, json!({ "trigger": "budget" }));
                let out = compact_history(history, Compact::Force, measured_now);
                base = base.saturating_sub(out.dropped) + out.pinned;
                if pin_summary_of_dropped(cfg, history, &out.dropped_msgs) {
                    base += 1;
                }
                cfg.last_input.set(0);
                cfg.last_compact.set(measured_now);
                out.elided + out.dropped > 0
            };
            if compressed && budget_extensions < MAX_BUDGET_EXTENSIONS {
                budget_extensions += 1;
                turn_tokens = 0;
                emit(json!({
                    "type": "system",
                    "subtype": "budget_extended",
                    "times": budget_extensions,
                    "max": MAX_BUDGET_EXTENSIONS,
                }));
                log::info(format!(
                    "成本预算撞顶（{turn_budget} tokens）⇒ 压缩续命 第 \
                     {budget_extensions}/{MAX_BUDGET_EXTENSIONS} 次，计点清零继续"
                ));
            } else {
                // 续不动了：给模型一次收口机会（与 `TOOL_BUDGET_HINT` 同一落点纪律）
                hint_sent = true;
                append_hint_to_last_tool_result(history, TURN_BUDGET_HINT);
                emit(json!({
                    "type": "system",
                    "subtype": "budget_exhausted",
                    "extensions": budget_extensions,
                }));
                log::info(format!(
                    "成本预算用尽（{}），要求模型收口作答",
                    if compressed {
                        format!("续命已用满 {budget_extensions}/{MAX_BUDGET_EXTENSIONS}")
                    } else {
                        "且压不动（体积在对话本身）".to_string()
                    }
                ));
            }
        }

        // 记下「请求发出的时刻」—— 下一轮拿它判端点侧那份前缀缓存还热不热
        // （2026-10-01）。用**发出**而不是响应返回：缓存是发出去那一刻开始算的，
        // 而一轮里工具可能跑几十秒，用返回时刻会把 TTL 凭空缩短。
        last_req_at = Some(std::time::Instant::now());

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
                let cur_common = common_prefix_len(prev_hist_hashes, &cur_hashes);
                let prev_len = prev_hist_hashes.len();
                *prev_hist_hashes = cur_hashes;
                common_log.push(cur_common);
                log::info(format!(
                    "请求前缀 问#{} #{} system={:016x}/{}字 tools={:016x}/{}字 history={:016x}/{}条/{}字 公共前缀={}/{}条{}",
                    question_seq,
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
                    // 成本预算计点（2026-10-01）：口径 = in+read+create+out，
                    // 与子代理 `SUBAGENT_BUDGET_TOKENS` 及前端 usage 归并口径一致。
                    turn_tokens += req_in + req_read + req_create;
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
                    if let Some(u) = ev.get("usage") {
                        if let Some(n) = u.get("output_tokens").and_then(Value::as_u64) {
                            out_tokens += n;
                            // 成本预算计点（2026-10-01）：输出也计入本次提问的花费
                            turn_tokens += n;
                            // Anthropic 的 message_delta.output_tokens 是**本条消息的累计值**，
                            // 所以这里是赋值不是累加（同一条消息可能来多次 delta）。
                            cur_out = n;
                        }
                        // 输入侧兜底（2026-10-07，见 `merge_input_usage`）：原生端点在
                        // `message_start` 给了量 ⇒ 这里幂等跳过；**中转端点**（实测
                        // api.moonobscura.com）在那里全填 0、真值只在这条上 ⇒ 在此采用，
                        // 否则输入 / 命中 / 写入三列永远是 0。
                        let d = (
                            u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
                            u.get("cache_read_input_tokens").and_then(Value::as_u64).unwrap_or(0),
                            u.get("cache_creation_input_tokens").and_then(Value::as_u64).unwrap_or(0),
                        );
                        let m = merge_input_usage((cur_in, cur_read, cur_create), d);
                        if m != (cur_in, cur_read, cur_create) {
                            in_tokens += m.0.saturating_sub(cur_in);
                            cache_read += m.1.saturating_sub(cur_read);
                            cache_create += m.2.saturating_sub(cur_create);
                            turn_tokens += (m.0 + m.1 + m.2)
                                .saturating_sub(cur_in + cur_read + cur_create);
                            cfg.last_input.set(m.0 + m.1 + m.2);
                            cur_in = m.0;
                            cur_read = m.1;
                            cur_create = m.2;
                        }
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
                    // 逐请求**当场上报**（2026-10-06）：回合被取消 / 中断 / agent 被强杀时拿不到
                    // 收尾的 `result.usage`，前端就一条账都不落 —— 而平台照计费。实测 2026-10-02：
                    // agent 日志 389 次请求、平台 401 次、本地 `usage-*.jsonl` 只有 308 次（有一轮
                    // 36 次请求整轮没落账）。前端把这条累计起来；**正常收尾仍以 `result.usage`
                    // 为准**（那是权威值，还含子代理），只有异常收尾才用累计值兜底。
                    emit(json!({
                        "type": "usage_delta",
                        "usage": {
                            "in": cur_in,
                            "read": cur_read,
                            "create": cur_create,
                            "out": cur_out,
                        },
                    }));
                    // 逐请求命中率 + 本侧是否改写前缀：两截证据落在同一行，离线即可归因，
                    // 不必再去前端 usage-*.jsonl 里对齐（2026-09-20，规则 23 的归因埋点）
                    let ctx = cur_in + cur_read + cur_create;
                    log::info(format!(
                        "请求用量 问#{} #{} in={} read={} create={} out={} 命中率={:.1}% 公共前缀={}条",
                        question_seq,
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
        // 写类工具先请用户审批（P2）：一批先全部发出，前端才能把连续 Cmd
        // 合并成一行（findLastCmdGroup）一次决定；随后按顺序阻塞等回包。
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
                            _ => pending = Some(open_approval(name, id, input, None)),
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

        // 按批执行：**连续的**只读调用 / **连续的**子代理调用各自并行（上限分别是
        // `TOOL_PARALLELISM` / `SUBAGENT_PARALLELISM`），其余串行。
        // 结果一律按下标回填 ⇒ 回灌顺序恒等于 tool_use 的原顺序。
        //
        // 「谁是自己发 API 请求的那种调用」**只判定一次**（`subagent_call()`），规划与执行
        // 共用这份结论 —— 此前判定散在批循环里，A14 再加一处并发分支就会出现
        // 「规划按串行、执行按并行」的漂移。
        let subagent_kinds: Vec<Option<SubagentCall<'_>>> = calls
            .iter()
            .map(|(_id, name, input)| subagent_call(name, input, skill_list))
            .collect();
        let mut slots: Vec<Option<(String, bool)>> = vec![None; calls.len()];
        for (kind, range) in plan_tool_batches(&calls, &subagent_kinds) {
            if kind == BatchKind::Subagent {
                // ── 子代理并行批（A14）──
                // 每个子代理一条线程、**各自一份 `Cfg`**：`Cfg` 里有 `Cell`（思考形态、
                // 实测体积缓存）⇒ 不是 `Sync`，`&Cfg` 过不了线程边界。用给后台复盘准备的
                // `detached()`；它复制的是**当前已跑通并缓存**的思考形态，所以子代理不会
                // 退回到一个端点不认的形态（同 `run_subagent` 的既有做法）。
                // 必须在 spawn **之前**建好 —— 建 client 要 `&Cfg`，而它只活在主线程上。
                let mut jobs: Vec<(usize, Cfg, SubagentCall<'_>)> = Vec::with_capacity(range.len());
                let mut spawn_err: Option<String> = None;
                for i in range.clone() {
                    match cfg.detached() {
                        Ok(c) => jobs.push((i, c, subagent_kinds[i].expect("并行批内必有形态"))),
                        Err(e) => {
                            spawn_err = Some(e);
                            break;
                        }
                    }
                }
                if let Some(e) = spawn_err {
                    // 建不出 http client（极罕见）⇒ 这一批**退回串行**用主 cfg 跑完：
                    // 宁可慢，也不能把调用静默丢成空结果。
                    log::warn(format!("子代理并行批准备失败（{e}），退回串行执行"));
                    for i in range.clone() {
                        slots[i] = Some(run_subagent_call(
                            cfg,
                            tctx,
                            &subagent_defs,
                            skill_list,
                            subagent_system,
                            ask_permission,
                            subagent_kinds[i].expect("并行批内必有形态"),
                            &run_inputs[i],
                            denieds[i].as_deref(),
                        ));
                    }
                    continue;
                }
                // 引用是 Copy，`move` 闭包拷进去的是引用本身（不会把 `subagent_defs`
                // 整个移走 —— 后面几批还要用）。
                let (defs_ref, skills_ref, sys_ref, inputs_ref, denieds_ref): (
                    &[Value],
                    &[skills::Skill],
                    &str,
                    &Vec<Value>,
                    &Vec<Option<String>>,
                ) = (&subagent_defs, skill_list, subagent_system, &run_inputs, &denieds);
                log::info(format!(
                    "子代理并行批 {} 条（并发上限 {SUBAGENT_PARALLELISM}）: {}",
                    range.len(),
                    range
                        .clone()
                        .map(|i| calls[i].1.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                let mut jobs = jobs.into_iter();
                loop {
                    let chunk: Vec<(usize, Cfg, SubagentCall<'_>)> =
                        jobs.by_ref().take(SUBAGENT_PARALLELISM).collect();
                    if chunk.is_empty() {
                        break;
                    }
                    let done: Vec<(usize, String, bool)> = thread::scope(|s| {
                        let handles: Vec<_> = chunk
                            .into_iter()
                            .map(|(i, my_cfg, k)| {
                                (
                                    i,
                                    s.spawn(move || {
                                        let (text, is_error) = run_subagent_call(
                                            &my_cfg,
                                            tctx,
                                            defs_ref,
                                            skills_ref,
                                            sys_ref,
                                            ask_permission,
                                            k,
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
                                h.join().unwrap_or_else(|_| {
                                    (i, "Error: subagent worker panicked".into(), true)
                                })
                            })
                            .collect()
                    });
                    // 回填仍按下标 ⇒ 报告回灌顺序恒等于 tool_use 原顺序（与只读批同一条纪律）
                    for (i, text, is_error) in done {
                        slots[i] = Some((text, is_error));
                    }
                }
                continue;
            }
            if kind == BatchKind::ReadOnly {
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
                continue;
            }
            // ── 串行批 ──
            let i = range.start;
            slots[i] = Some(match subagent_kinds[i] {
                Some(k) => run_subagent_call(
                    cfg,
                    tctx,
                    &subagent_defs,
                    skill_list,
                    subagent_system,
                    ask_permission,
                    k,
                    &run_inputs[i],
                    denieds[i].as_deref(),
                ),
                None => run_one_tool(
                    tctx,
                    mcp_bridge.as_deref_mut(),
                    skill_list,
                    &calls[i].0,
                    &calls[i].1,
                    &run_inputs[i],
                    denieds[i].as_deref(),
                ),
            });
        }

        let mut results: Vec<Value> = calls
            .iter()
            .zip(slots)
            .map(|((id, _name, _input), slot)| {
                let (text, is_error) =
                    slot.unwrap_or_else(|| ("Error: tool was not executed".into(), true));
                tool_result_block(id, text, is_error)
            })
            .collect();

        // ── 后台结果回流（2026-10-01，用户选的形态：「攒着，下次工具调用时带上」）──
        // 已跑完的后台命令，输出在这里**随 tool_result 一起**交给模型。
        // **不单独 push 一条消息**：那会改变 `history` 末尾的消息形态，把端点侧前缀缓存
        // 整段打掉（见 `TOOL_BUDGET_HINT` 上方那张表）。拼进**已有**的 tool_result 内部
        // 则只有它自己那条变 —— 而它本来就是这一轮新产生、还没被缓存过的内容。
        //
        // ⚠️ 已知边界（如实记）：模型**不再调工具**（直接作答收尾）时这一轮没有
        // tool_result 可挂，那条后台输出会留在队列里，等到**下一次**有工具调用时才交付。
        let bg = take_background_results();
        if !bg.is_empty() && !results.is_empty() {
            let extra = bg.join("\n\n");
            let last = results.last_mut().expect("上面刚判过非空");
            if let Some(prev) = last.get("content").and_then(Value::as_str) {
                last["content"] = json!(format!("{prev}\n\n{extra}"));
                log::info(format!("后台结果已随 tool_result 交给模型（{} 条）", bg.len()));
            }
        }
        // ── 卡住检测 · 第 1 步：更新窗口并判定（2026-10-01）──────────
        // 窗口只收**单工具轮**：一轮里模型肯并行调多个工具，说明它还在推进，
        // 不可能是「原地打转」。指纹用刚落座的这批结果文本（判据见 `STUCK_ROUNDS`）。
        let fp_texts: Vec<String> = results
            .iter()
            .filter_map(|b| b.get("content").and_then(Value::as_str).map(str::to_string))
            .collect();
        let stuck_now = if calls.len() == 1 {
            stuck_window.push_back((calls[0].1.to_string(), result_fingerprint(&fp_texts)));
            while stuck_window.len() > STUCK_ROUNDS {
                stuck_window.pop_front();
            }
            !stuck_hint_sent && is_stuck(&stuck_window)
        } else {
            stuck_window.clear();
            false
        };

        let tool_msg = json!({ "role": "user", "content": results });
        let mut tool_event = json!({ "type": "user", "message": tool_msg.clone() });
        // ── A17：写入类工具的安全告警随结果一起下发（2026-10-01）─────────
        // **只挂事件、不挂 `message`**：`message` 要原样进 `history` 并发给端点，
        // 塞一个非标准键等于给端点送一个它不认识的东西（端点是按块严格校验的）。
        let warns: Vec<Value> = calls
            .iter()
            .filter_map(|(id, name, input)| {
                let hits = security_warnings(name, input);
                if hits.as_array().map(|a| a.is_empty()).unwrap_or(true) {
                    None
                } else {
                    Some(json!({ "tool_use_id": id, "hits": hits }))
                }
            })
            .collect();
        if !warns.is_empty() {
            tool_event["security_warnings"] = json!(warns);
        }
        emit(tool_event);
        history.push(tool_msg);

        // ── 卡住检测 · 第 2 步：提示（2026-10-01）──────────────────
        // 落点：**刚 push、还没发出去过**的那条 `tool_result` —— 它不在任何已缓存
        // 前缀里，改它一个字都不废（见 `TOOL_BUDGET_HINT` 上方那条「零代价落点」）。
        // 只提示一次（`stuck_hint_sent`）：之后若仍无进展，交给预算 / 轮次闸门收口 ——
        // 反复提示只会往上下文里灌噪音，还白花钱。
        if stuck_now && append_hint_to_last_tool_result(history, STUCK_HINT) {
            stuck_hint_sent = true;
            stuck_window.clear();
            emit(json!({
                "type": "system",
                "subtype": "stuck_detected",
                "tool": calls[0].1,
                "rounds": STUCK_ROUNDS,
            }));
            log::info(format!(
                "卡住检测：连续 {STUCK_ROUNDS} 轮只调用 `{}` 且结果头部相同，已提示模型换思路",
                calls[0].1
            ));
        }

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
            append_hint_to_last_tool_result(history, TOOL_BUDGET_HINT);
            // 界面要**如实**说一句（2026-10-01 用户要求）：原先只有 stderr 一行，
            // 用户看到的是「AI 自己停了」，不知道发生了什么、也不知道可以继续追问。
            emit(json!({
                "type": "system",
                "subtype": "rounds_exhausted",
                "rounds": MAX_TOOL_ROUNDS,
            }));
            log::info(format!(
                "工具轮次达兜底上限（{MAX_TOOL_ROUNDS} 轮），要求模型收口作答"
            ));
        }
    }

    // 子代理 / 后台复盘的用量并入本次提问（2026-09-29）。**不归并 = 本地账少一截**：
    // 平台照收它们的钱，而 `result.usage` 此前只有主循环那几轮（实测平台 24 次请求
    // vs 本地 17 次）。`take()` 顺带归零，保证「本次提问的绝对值」这个口径不被带到下问。
    // 顺序说明：它们在提问内并发完成，追加到 `requests[]` 末尾的先后由线程调度决定；
    // 平台对账按「条数 + 合计」对齐，不依赖顺序。
    let (sub_in, sub_read, sub_create, sub_out, sub_reqs) = cfg.sub.take();
    let sub_requests = sub_reqs.len();
    if sub_in + sub_read + sub_create + sub_out > 0 {
        log::info(format!(
            "子代理/复盘用量并入本次提问 in={sub_in} read={sub_read} create={sub_create} \
             out={sub_out} 请求={sub_requests}"
        ));
    }
    in_tokens += sub_in;
    cache_read += sub_read;
    cache_create += sub_create;
    out_tokens += sub_out;
    req_log.extend(sub_reqs);

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
            // **其中**来自子代理 / 后台复盘的部分 —— 已包含在上面四个数里，再报一份只为
            // 归因（「这问的钱有多少是子代理烧的」）。消费者**不得**把它再加一次。
            "subagent": {
                "input_tokens": sub_in,
                "output_tokens": sub_out,
                "cache_read_input_tokens": sub_read,
                "cache_creation_input_tokens": sub_create,
                "requests": sub_requests,
            },
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
            tool_output: None,
            shell_control: None,
        }
    }

    /// 输入用量归并（2026-10-07）：原生端点用 `message_start` 的量；中转端点在
    /// `message_start` 里把三类输入全填 0、真值只在 `message_delta` 上 ⇒ 必须回落，
    /// 否则面板的输入 / 命中 / 写入三列恒为 0（用户报的「没有数值」）。
    #[test]
    fn merge_input_usage_prefers_start_and_falls_back_to_delta() {
        // 原生：start 有量 ⇒ 原样采用，delta 再给什么都不覆盖
        assert_eq!(merge_input_usage((100, 20, 0), (0, 0, 0)), (100, 20, 0));
        assert_eq!(merge_input_usage((100, 20, 0), (999, 999, 999)), (100, 20, 0));
        // 中转：start 全 0 ⇒ 采用 delta 的真值
        assert_eq!(merge_input_usage((0, 0, 0), (31, 5, 2)), (31, 5, 2));
        // 两边都空 ⇒ 保持全 0（旧 agent / 不报用量的端点）
        assert_eq!(merge_input_usage((0, 0, 0), (0, 0, 0)), (0, 0, 0));
    }

    /// A17：结果卡上的安全告警载荷 —— 判据与审批卡**同源**（`written_payload` + `analyze`）。
    /// 四条边界：`Write` 取 `content`、`Edit` 取 `new_string`（**不取 `old_string`**：
    /// 那是要被删掉的内容，扫它会把「正在清理凭据」标成可疑）、非写入类工具一律空、
    /// 干净内容也是空（前端据此决定「有没有这个键」）。
    #[test]
    fn security_warnings_only_cover_written_payloads() {
        let secret = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n";
        let hits = |v: Value| v.as_array().cloned().unwrap_or_default();

        assert_eq!(
            hits(security_warnings(
                "Write",
                &json!({ "file_path": "a.env", "content": secret })
            ))
            .len(),
            1,
            "Write 扫 content"
        );
        assert_eq!(
            hits(security_warnings(
                "Edit",
                &json!({ "old_string": secret, "new_string": "safe" })
            ))
            .len(),
            0,
            "删掉凭据的那次 Edit 不该被标成可疑"
        );
        assert_eq!(
            hits(security_warnings("Edit", &json!({ "new_string": secret }))).len(),
            1,
            "Edit 扫 new_string"
        );
        assert!(hits(security_warnings("Cmd", &json!({ "command": "echo hi" }))).is_empty());
        assert!(hits(security_warnings("Write", &json!({ "content": "fn main() {}" }))).is_empty());
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

    /// `公共前缀` 这一列（A16 判据）必须能分辨「纯追加」与「就地改写」。
    ///
    /// 这是**唯一**能判定「两次提问之间本侧动了 history 没有」的东西：整体 hash 只回答
    /// 「变没变」，而 history 每轮必然变（只追加也会变）。所以两条边界都要钉住：
    /// 纯追加 ⇒ 等于上轮条数；在第 k 条改写 ⇒ 恰好等于 k（其后整段缓存必然失效）。
    #[test]
    fn common_prefix_counts_only_byte_identical_messages() {
        assert_eq!(common_prefix_len(&[], &[]), 0);
        assert_eq!(common_prefix_len(&[], &[1, 2, 3]), 0);
        // 纯追加：旧的全在，只是后面多了几条
        assert_eq!(common_prefix_len(&[7, 8], &[7, 8, 9]), 2);
        // 就地改写第 1 条（0-based 下标 1）⇒ 只剩第 0 条还相同
        assert_eq!(common_prefix_len(&[7, 8, 9], &[7, 42, 9]), 1);
        // 连第 0 条都变了（前缀整体漂移，例如 system/cwd 参与拼接）
        assert_eq!(common_prefix_len(&[7, 8], &[42, 8]), 0);
        // cur 比 prev 短（裁剪 / 丢弃）也必须如实报，不许当真前缀
        assert_eq!(common_prefix_len(&[7, 8, 9], &[7]), 1);
    }

    /// 子代理用量账本：累加正确、`take()` 取走即归零（2026-09-29）。
    ///
    /// 钉两件事：① 多轮累加不丢；② `take()` 之后归零 —— `result.usage` 的口径是
    /// 「本次提问的绝对值」，归零漏了就会把上一问的子代理用量重复计进下一问。
    #[test]
    fn subagent_usage_accumulates_and_take_resets() {
        let u = SubagentUsage::default();
        assert_eq!(u.take(), (0, 0, 0, 0, Vec::new()));

        u.record(100, 200, 0, 300);
        u.record(1, 2, 3, 4);
        let (input, read, create, output, reqs) = u.take();
        assert_eq!((input, read, create, output), (101, 202, 3, 304));
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[1], json!({"in": 1, "read": 2, "create": 3, "out": 4}));

        // 取走即归零（下一问从空账开始）
        assert_eq!(u.take(), (0, 0, 0, 0, Vec::new()));
    }

    /// `detached()` 必须**共享**子代理用量账本（2026-09-29）。
    ///
    /// 并行子代理批里每个子代理跑的是 `Cfg::detached()` 出来的副本（见 `run_query` 的
    /// `BatchKind::Subagent` 分支）；这个字段若跟着副本各建一份，并行那批的用量就永远回不到
    /// 主循环 —— 账面上看着「修好了」，实际只在串行路径生效。这条钉的就是那个沉默的失效。
    #[test]
    fn detached_shares_the_subagent_usage_ledger() {
        let cfg = cfg_pointing_at("http://127.0.0.1:1/v1/messages");
        let copy = cfg.detached().expect("detached");
        copy.sub.record(1, 2, 3, 4);
        let (input, read, create, output, reqs) = cfg.sub.take();
        assert_eq!((input, read, create, output), (1, 2, 3, 4));
        assert_eq!(reqs.len(), 1);
    }

    /// 系统提示词的**前缀缓存不变量**（2026-09-17 方案 B 的守门测试）。
    ///
    /// 方案 B 把「人格 + 文风」两块从**用户消息**（每问重发、必未命中）搬进了**系统提示词**
    /// （固定前缀的一部分、永远命中）。搬错了地方 —— 比如塞进任何按 query 拼的字符串 ——
    /// 就会让系统提示词每轮都变，把整个固定前缀的缓存打掉，比原来更糟。所以这里钉两件事：
    /// ① 同一 cwd 下逐字节可复现；② 两块文案真的在里面。
    #[test]
    fn system_prompt_is_stable_and_carries_persona() {
        let env = env_block(std::path::Path::new("C:/work"));
        let build = || build_system_prompt("", &env, "");
        let a = build();
        assert_eq!(a, build(), "同一 cwd 下系统提示词必须逐字节相同（否则前缀缓存每轮作废）");
        assert!(a.contains("## Personality (fixed — always apply)"), "人格块丢了");
        assert!(a.contains("## Output Style"), "文风块丢了");
        // 「回复语言跟随用户提问语言」是**产品底线**（2026-10-06 用户明确要求：中文提问 ⇒
        // 除必要的英文（代码 / 命令 / API 名）外整段中文）。它曾被写成一句很弱的
        // `Always reply in the user's language.`，模型并不总遵守 ⇒ 钉住这条更强的措辞。
        assert!(
            a.contains("answer in the language the user wrote in"),
            "「回复语言跟随用户提问语言」这条丢了"
        );
        assert!(!a.contains('{'), "残留了未被 format! 替换的占位符");
        assert!(!a.contains(USER_PERSONA_HEADER), "没配用户人格时不该多出这一段");
    }

    /// 「往期会话索引 + 长期记忆」**不在 system 里、只在上下文块里**（2026-10-06，M2-16 方案 D）。
    ///
    /// 这是方案 D 的核心不变量，两个方向都要钉住：
    /// ① **反面** —— 它们不在 `build_system_prompt` 的产物里（否则 system 随会话 / 记忆增长而变，
    ///    每次重启后端点缓存从 0 起全 miss，正是要修的病灶）；
    /// ② **正面** —— `context_block_message` 把它们合成**一条 `user` 消息**、且表头可被
    ///    `is_context_block_msg` 认出（幂等注入的判据）。两者皆空 ⇒ `None`（不加空壳）。
    #[test]
    fn index_and_memory_live_in_the_context_block_not_the_system_prompt() {
        let env = env_block(std::path::Path::new("C:/work"));
        let system = build_system_prompt("", &env, "");
        assert!(
            !system.contains(CONTEXT_BLOCK_HEADER),
            "上下文块表头不该出现在 system 里"
        );

        // 两者皆空 ⇒ 没有上下文块（不凭空多一条消息）
        assert!(context_block_message("", "   ").is_none(), "两块都空应返回 None");

        let cb = context_block_message("HIST-INDEX", "LONG-MEM").expect("非空时应生成块");
        assert_eq!(cb.get("role").and_then(Value::as_str), Some("user"), "必须是 user 消息");
        let text = cb
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .expect("应有 text 块");
        assert!(text.starts_with(CONTEXT_BLOCK_HEADER), "必须以表头开头（幂等判据）");
        assert!(text.contains("HIST-INDEX") && text.contains("LONG-MEM"), "两块正文都在");
        assert!(is_context_block_msg(&cb), "自产的块必须能被幂等判据认出");

        // 普通用户消息不能被误判成上下文块（否则幂等逻辑会漏注）
        let normal = json!({ "role": "user", "content": [{ "type": "text", "text": "你好" }] });
        assert!(!is_context_block_msg(&normal), "普通用户消息不该被认成上下文块");

        // 只有一块非空也照样生成
        assert!(context_block_message("HIST", "").is_some());
        assert!(context_block_message("", "MEM").is_some());
    }

    /// 用户人格（L2）：**逐字进主提示词、位置在内置人格之后、且只进主提示词**。
    ///
    /// 三条都是硬约束，各对应一个真实代价：
    /// ① 逐字 —— 用户写的就是模型看到的（不做任何转义 / 改写）；
    /// ② 位置 —— 内置段在前（保住「文风 / 禁 emoji」这类产品底线），用户段接在其后、
    ///    环境块之前；两段都在固定前缀里 ⇒ 永远命中缓存；
    /// ③ 只进主提示词 —— 子代理 / 复盘是内部产物，多一份就是每个并发子代理都要重发的固定成本。
    #[test]
    fn user_persona_reaches_only_the_main_prompt() {
        let env = env_block(std::path::Path::new("C:/work"));
        let persona = "Always answer in 中文。我是团队里的测试同学。\n- 短句优先\n- 保留 {like this} 字面量";
        let main = build_system_prompt(persona, &env, "SKILLS");
        assert!(main.contains(USER_PERSONA_HEADER), "用户人格段的表头丢了");
        assert!(main.contains(persona), "用户人格正文必须逐字进提示词");
        assert!(
            main.find("## Personality").unwrap() < main.find(USER_PERSONA_HEADER).unwrap(),
            "用户段必须接在内置人格之后"
        );
        assert!(
            main.find(USER_PERSONA_HEADER).unwrap() < main.find("Environment:").unwrap(),
            "用户段必须排在环境块之前"
        );
        // 花括号是**字面量**（persona 是作为 `format!` 的参数注入的，不参与格式解析）
        assert!(main.contains("{like this}"), "persona 里的花括号被当成占位符了");
        // 子代理 / 复盘刻意不含
        assert!(!build_subagent_system(&env, "SKILLS").contains(USER_PERSONA_HEADER));
        assert!(!build_subagent_system(&env, "SKILLS").contains(persona));
        assert!(!build_review_system(&env, "SKILLS").contains(USER_PERSONA_HEADER));
        assert!(!build_review_system(&env, "SKILLS").contains(persona));
    }

    /// 空白人格 = 完全不存在（不加表头、不加换行）—— 否则没配过的用户也会多一段空壳，
    /// 而那是**每次请求都要发**的字节。
    #[test]
    fn blank_persona_adds_nothing_to_the_prompt() {
        assert_eq!(persona_block(""), "");
        assert_eq!(persona_block("   \n\t  "), "");
        let env = env_block(std::path::Path::new("C:/work"));
        let with_blank = build_system_prompt("  \n ", &env, "");
        assert_eq!(
            with_blank,
            build_system_prompt("", &env, ""),
            "全空白的人格必须与「没配」逐字节等价"
        );
        // 两端空白去掉后再注入（避免用户在面板里多敲的空行变成固定前缀里的噪声）
        let one = persona_block("  Be terse.  ");
        assert!(one.contains("Be terse.") && !one.ends_with(' ') && !one.ends_with('\n'));
    }

    /// 项目记忆（`AGENTS.md`，2026-10-01）：逐字进提示词、三份提示词同源、空文件零字节。
    #[test]
    fn agents_md_reaches_all_three_prompts_verbatim() {
        let body = "# 项目约定\n- 构建：`npm run build`\n- 保留 {braces} 字面量";
        let block = project_block_from(body);
        assert!(block.contains(AGENTS_MD_HEADER), "表头丢了");
        assert!(block.contains(body), "正文必须逐字进提示词");
        assert!(block.contains("{braces}"), "花括号是字面量，不该被当占位符");
        // 与 env_block 拼在一起后，三份提示词都取同一份字符串 ⇒ 都拿得到
        let env = format!("{}{}", env_block(std::path::Path::new("C:/work")), block);
        for p in [
            build_system_prompt("", &env, ""),
            build_subagent_system(&env, ""),
            build_review_system(&env, ""),
        ] {
            assert!(p.contains(AGENTS_MD_HEADER), "项目记忆必须三份提示词都在");
        }
    }

    /// 没有 `AGENTS.md` / 文件全空白 = 完全不存在：不加表头、不加换行。
    /// 否则每个没有这个文件的目录都会凭空多一段**每轮都要发**的空壳。
    #[test]
    fn blank_agents_md_adds_nothing_to_the_prompt() {
        assert_eq!(project_block_from(""), "");
        assert_eq!(project_block_from("  \n\t  "), "");
        // 纯函数层再钉一次「没文件 ⇒ 空串」（读到不存在路径）
        let missing = std::env::temp_dir().join("lunac-agents-md-does-not-exist-9231");
        assert_eq!(project_block(&missing), "");
    }

    /// `AGENTS.md` 超长必须截断 —— 它进的是每次请求都要发的固定前缀。
    #[test]
    fn oversized_agents_md_is_truncated() {
        let long = "x".repeat(MAX_AGENTS_MD_CHARS + 500);
        let block = project_block_from(&long);
        let body = block.split(AGENTS_MD_HEADER).nth(1).unwrap().trim_start_matches('\n');
        assert_eq!(body.chars().count(), MAX_AGENTS_MD_CHARS, "必须截到上限");
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
            ("Cmd", json!({"command": "echo hi"})),
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
    /// ① 白名单**不含**任何能改本机其它东西的工具（`Cmd` / `PowerShell` / `Agent` / MCP）；
    /// ② 白名单里的每一件都在 `defs()` 或条件注册里真的存在（写错名字 = 静默少一件）。
    #[test]
    fn review_tool_whitelist_is_read_and_memory_only() {
        for forbidden in ["Cmd", "PowerShell", "WebFetch", "WebSearch", "Agent", "TodoWrite"] {
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

    /// 造一段历史：`tool_chars` 大小的 tool_result 躺在**瘦身范围**里，其后是
    /// `COMPACT_KEEP_TAIL` 条 `tail_chars` 大小的文本消息（都在保留尾部里）。
    ///
    /// 尾部大小是构造「净亏」局面的旋钮：尾部**不是候选**（不在瘦身范围），瘦身也不动它，
    /// 所以它只进 Δ —— `tail_chars × COMPACT_KEEP_TAIL > tool_chars` 就是「省小废大」。
    /// 2026-10-01 起这段历史**必须带工具名**：工具分级瘦身靠 `tool_use_id` 反查
    /// `tool_use.name`，而配不出名字的结果一律不瘦（`elidable_tool(None) == false`）。
    /// 这不是测试的权宜之计 —— 真实历史里端点**硬校验** `tool_use`/`tool_result` 的配对，
    /// 所以造数据必须照真实形态来。副作用：tool_result 的下标从 1 变成 **2**。
    fn history_for_elide(tool_chars: usize, tail_chars: usize) -> (Vec<Value>, Value) {
        history_for_elide_tool("Read", tool_chars, tail_chars)
    }

    /// 同上，但指定工具名 —— 用来钉「哪些工具的结果可瘦」这条纪律。
    fn history_for_elide_tool(
        tool_name: &str,
        tool_chars: usize,
        tail_chars: usize,
    ) -> (Vec<Value>, Value) {
        let big = json!("x".repeat(tool_chars));
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"hi"}]}),
            json!({"role":"assistant","content":[
                {"type":"tool_use","id":"tu1","name":tool_name,"input":{"file_path":"x"}}
            ]}),
            json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"tu1","content":big.clone()}
            ]}),
        ];
        for i in 0..COMPACT_KEEP_TAIL {
            history.push(json!({"role":"user","content":[
                {"type":"text","text":format!("t{i}:{}", "y".repeat(tail_chars))}
            ]}));
        }
        (history, big)
    }

    /// 成本模型（A15）：**省下的 < 废掉的** ⇒ 一个字都不许动。
    ///
    /// 旧闸门只看「可省体积 ÷ 上下文体积」，看不见**代价的位置** —— 这里刻意造出
    /// 「总量很小、但被改的那条后面拖着一整段」（省 3000 / 废 2.4 万）：旧闸门会放行，
    /// 新模型必须否决 —— 放行就是白废一段前缀缓存（A15 的起因）。
    #[test]
    fn elide_is_skipped_when_it_costs_more_than_it_saves() {
        let (mut history, big) = history_for_elide(3_000, 3_000);
        let out = compact_history(&mut history, Compact::Elide, 100_000);
        assert_eq!(out.elided, 0, "省 3000 废 2.4 万 ⇒ 判为省小废大，不该动手");
        assert_eq!(out.dropped, 0, "瘦身档永不丢弃整条消息");
        assert_eq!(history[2]["content"][0]["content"], big, "历史必须逐字节原样保留");
    }

    /// 成本模型：**省下的 > 废掉的** ⇒ 动手。
    #[test]
    fn elide_proceeds_when_it_saves_more_than_it_costs() {
        let (mut history, big) = history_for_elide(30_000, 1_000);
        let out = compact_history(&mut history, Compact::Elide, 100_000);
        assert_eq!(out.elided, 1);
        assert_ne!(history[2]["content"][0]["content"], big);
        let text = history[2]["content"][0]["content"].as_str().unwrap();
        assert!(text.starts_with("[elided:"), "瘦身后要留下占位标记：{text}");
    }

    /// 成本模型只动**净收益最大**的那一段：靠前那条大块若「连累的后缀太长」就留着不动。
    ///
    /// 这正是 backlog 候选手段②「瘦身从靠近尾部开始」的自然结果 —— 枚举「最靠前被瘦身的块」
    /// 取净收益最大的 k，不必另加规则。
    #[test]
    fn elide_picks_the_segment_with_the_best_net_gain() {
        // 两条大结果都要**配出工具名**（2026-10-01 起的工具分级，见 history_for_elide 的注）
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"hi"}]}),
            json!({"role":"assistant","content":[
                {"type":"tool_use","id":"a1","name":"Read","input":{"file_path":"a"}}
            ]}),
            json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"a1","content":"a".repeat(3_000)}
            ]}),
            // 中间夹一段**非候选**的大文本：动上面那条要连它一起作废 ⇒ 不划算
            json!({"role":"assistant","content":[{"type":"text","text":"z".repeat(50_000)}]}),
            json!({"role":"assistant","content":[
                {"type":"tool_use","id":"b1","name":"Read","input":{"file_path":"b"}}
            ]}),
            json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"b1","content":"b".repeat(30_000)}
            ]}),
        ];
        for i in 0..COMPACT_KEEP_TAIL {
            history.push(json!({"role":"user","content":[{"type":"text","text":format!("t{i}")}]}));
        }
        let out = compact_history(&mut history, Compact::Elide, 100_000);
        assert_eq!(out.elided, 1, "只该动净收益更大的那一条（靠后那条）");
        assert_eq!(
            history[2]["content"][0]["content"].as_str().map(str::len),
            Some(3_000),
            "靠前那条要原样留着 —— 动它得连 5 万字一起作废"
        );
        assert!(
            history[5]["content"][0]["content"].as_str().unwrap().starts_with("[elided:"),
            "靠后那条（后缀短 ⇒ 净收益胜出）应当被瘦身"
        );
    }

    /// `Drop` / `Force` 是防 400 的安全刚需 —— **不走成本模型**：净亏也照压。
    #[test]
    fn drop_and_force_ignore_the_cost_model() {
        let (mut history, big) = history_for_elide(3_000, 3_000);
        let out = compact_history(&mut history, Compact::Drop, 100_000);
        assert_eq!(out.elided, 1, "Drop 是水位刚需，不许被成本模型拦下");
        assert_eq!(out.dropped, 0, "砍得动 tool_result 时不必丢整条消息");
        assert_ne!(history[2]["content"][0]["content"], big);

        let (mut history, _) = history_for_elide(3_000, 3_000);
        let out = compact_history(&mut history, Compact::Force, 100_000);
        assert_eq!(out.elided, 1, "Force 是 400 兜底，不许被成本模型拦下");
    }

    /// 工具分级瘦身（2026-10-01）：**只瘦可重现的结果**。
    ///
    /// 三组数据刻意造得完全一样（同体积、同净亏局面），唯一的变量是**工具名** ——
    /// 于是断言失败时能直接读出「是哪一类被判错了」。第三条是对照组：证明前两条
    /// 不是「什么都没发生」（否则一个恒真的 bug 也能让这个用例通过）。
    ///
    /// 用 `ElideCold` 而不是 `Drop` 验证分级本身：冷档**永不丢整条消息**，
    /// 于是「没瘦」只能归因于分级判据，不会与「整条被丢」混在一起
    /// （后者的行为单独由 `drop_still_evicts_what_the_grade_refuses_to_slim` 钉住）。
    #[test]
    fn tool_grade_keeps_subagent_reports_but_slims_reproducible_results() {
        // 子代理报告**不可重现**：子代理的中间过程不进主对话，报告是唯一产物
        let (mut history, big) = history_for_elide_tool("Agent", 30_000, 1_000);
        let out = compact_history(&mut history, Compact::ElideCold, 100_000);
        assert_eq!(out.elided, 0, "压掉子代理报告 = 永久失忆，代价远高于省下的体积");
        assert_eq!(out.dropped, 0, "冷档永不丢整条消息");
        assert_eq!(history[2]["content"][0]["content"], big, "报告必须逐字节原样留着");

        // MCP 工具同理（`mcp__*` 永远不在白名单里）
        let (mut history, big) = history_for_elide_tool("mcp__github__search", 30_000, 1_000);
        let out = compact_history(&mut history, Compact::ElideCold, 100_000);
        assert_eq!(out.elided, 0, "MCP 结果同样不可重现");
        assert_eq!(history[2]["content"][0]["content"], big);

        // 同一局面换成 `Read` ⇒ 该瘦
        let (mut history, big) = history_for_elide_tool("Read", 30_000, 1_000);
        let out = compact_history(&mut history, Compact::ElideCold, 100_000);
        assert_eq!(out.elided, 1, "Read 的结果重跑一次就能拿回来，该瘦");
        assert_ne!(history[2]["content"][0]["content"], big);
    }

    /// ⚠️ **已知权衡，如实钉住**（2026-10-01）：`Drop` 档是 0.95 水位的**安全刚需** ——
    /// 「挤不出来（瘦不动）就丢整条消息」。所以**分级拦不住丢弃**：不可瘦的结果在
    /// `Drop` 档下仍然会被整条丢掉。
    ///
    /// 这个取舍是有意的：不压就可能撞 400，而 400 会让**整轮**失败、回滚重来，
    /// 比丢掉一份报告更糟。要改成「Drop 时优先丢可重现的那些」，得给丢弃也加一层
    /// 分级排序 —— 目前不值得那个复杂度。**写在这里是为了防止以后误以为
    /// 「子代理报告永不丢」**（分级只在瘦身那一层生效）。
    #[test]
    fn drop_still_evicts_what_the_grade_refuses_to_slim() {
        let (mut history, _) = history_for_elide_tool("Agent", 30_000, 1_000);
        let out = compact_history(&mut history, Compact::Drop, 100_000);
        assert_eq!(out.elided, 0, "分级不许瘦它");
        assert!(out.dropped > 0, "瘦不动 ⇒ Drop 档转而丢整条消息（安全刚需）");
    }

    /// 白名单是**闭集**：认不出名字的（`None`）一律不瘦 —— 分不清就按「不可重现」处理。
    #[test]
    fn elidable_tool_whitelist_is_closed() {
        for ok in ["Read", "Grep", "Glob", "Cmd", "PowerShell", "WebFetch", "WebSearch"] {
            assert!(elidable_tool(Some(ok)), "{ok} 可重现，应当可瘦");
        }
        for no in ["Agent", "Skill", "TodoWrite", "Remember", "SessionSearch", "mcp__x__y"] {
            assert!(!elidable_tool(Some(no)), "{no} 不可重现，绝不许瘦");
        }
        assert!(!elidable_tool(None), "认不出工具名 ⇒ 保守不瘦");
    }

    /// 冷缓存档（2026-10-01）：缓存已过期 ⇒ **不走成本模型**，净亏也照瘦；
    /// 但「永不丢整条消息」这条纪律与 `Elide` 一致。
    #[test]
    fn cold_elide_ignores_the_cost_model_and_never_drops() {
        // 与 `elide_is_skipped_when_it_costs_more_than_it_saves` **同一组数据**
        // （省 3000 / 废 2.4 万）：热档判「省小废大」不动手，冷档必须动手 ——
        // 差别只该来自「端点那份缓存还在不在」，不该来自别的地方。
        let (mut history, _) = history_for_elide(3_000, 3_000);
        let out = compact_history(&mut history, Compact::Elide, 100_000);
        assert_eq!(out.elided, 0, "热档：省小废大 ⇒ 一个字都不动");

        let (mut history, big) = history_for_elide(3_000, 3_000);
        let out = compact_history(&mut history, Compact::ElideCold, 100_000);
        assert_eq!(out.elided, 1, "冷档：缓存本来就过期了，Δ 的代价是 0 ⇒ 照瘦");
        assert_eq!(out.dropped, 0, "冷档同样永不丢整条消息");
        let text = history[2]["content"][0]["content"].as_str().unwrap();
        assert!(text.starts_with("[elided:"), "冷档的占位串形状与热档一致：{text}");
        assert_ne!(history[2]["content"][0]["content"], big);
    }

    /// 卡住判据（2026-10-01）：**同工具 + 同指纹**才判；差一个字都不算。
    #[test]
    fn stuck_needs_same_tool_and_same_fingerprint() {
        let w = |items: &[(&str, &str)]| -> std::collections::VecDeque<(String, String)> {
            items.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
        };
        // 不够长 ⇒ 不判（前几轮本来就没有「重复」可言）
        assert!(!is_stuck(&w(&[("Read", "x"), ("Read", "x"), ("Read", "x")])));
        // 四条同工具同指纹 ⇒ 卡住
        assert!(is_stuck(&w(&[("Read", "x"), ("Read", "x"), ("Read", "x"), ("Read", "x")])));
        // 工具名不同 ⇒ 模型在换手段，不判
        assert!(!is_stuck(&w(&[("Read", "x"), ("Grep", "x"), ("Read", "x"), ("Read", "x")])));
        // 结果不同 ⇒ 在读不同的东西，不判（「连读 4 个文件」就是这个形态，绝不能误伤）
        assert!(!is_stuck(&w(&[("Read", "a"), ("Read", "b"), ("Read", "c"), ("Read", "d")])));

        // 指纹取**头部去空白**：不同文件的开头就不同 ⇒ 不会被误判成同一次调用；
        // 而同一份结果的空白差异不该造成两副面孔（落盘/trim 的差别是噪音）
        let fp_a = result_fingerprint(&["/a/b.rs\nline1\nline2".to_string()]);
        let fp_b = result_fingerprint(&["/a/c.rs\nline1\nline2".to_string()]);
        let fp_c = result_fingerprint(&["  /a/b.rs  \n line1 \nline2 ".to_string()]);
        assert_ne!(fp_a, fp_b);
        assert_eq!(fp_a, fp_c);
    }

    /// 提示的落点（2026-10-01）：**拼进最后一条 `tool_result` 的文本内部** ——
    /// 消息条数与内容块数量都不能变（2026-09-20 单变量实测钉死的形态）。
    #[test]
    fn hint_is_appended_inside_the_last_tool_result() {
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"hi"}]}),
            json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"t1","content":"OLD"}
            ]}),
        ];
        let before = history.len();
        assert!(append_hint_to_last_tool_result(&mut history, "HINT"));
        assert_eq!(history.len(), before, "不许新增消息");
        let blocks = history[1]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 1, "不许新增内容块");
        assert_eq!(blocks[0]["type"].as_str(), Some("tool_result"), "块类型不许变");
        assert_eq!(blocks[0]["content"].as_str(), Some("OLD\n\nHINT"));

        // 末尾不是工具消息 ⇒ 拼不上。调用方据此**不置**「已提示」，
        // 否则那句提示永远发不出去（而状态位已经消耗掉了）。
        let mut plain = vec![json!({"role":"user","content":[{"type":"text","text":"hi"}]})];
        assert!(!append_hint_to_last_tool_result(&mut plain, "HINT"));
    }

    /// 判据本体是纯函数：**位置**决定收益（σ）与代价（Δ），钉死四种局面。
    #[test]
    fn worthwhile_elisions_decides_by_positions() {
        let text = |s: &str| json!({"role":"user","content":[{"type":"text","text":s}]});
        let tool = |n: usize| {
            json!({"role":"user","content":[{"type":"tool_result","content":"x".repeat(n)}]})
        };

        // ① 没有候选 → 什么都不做
        let p = worthwhile_elisions(&[text("hi")], &[]);
        assert!(p.picked.is_empty() && p.sigma == 0 && p.delta == 0 && p.net == 0);

        // ② 单候选、后面拖着一大段非候选 ⇒ 净亏：否决，但把数字带出来供日志
        let h = vec![text("hi"), tool(3_000), text(&"z".repeat(50_000))];
        let p = worthwhile_elisions(&h, &[(1, 0, 3_000)]);
        assert!(p.net < 0, "省 3000 废 5 万 ⇒ 净亏：{}", p.net);
        assert!(p.picked.is_empty(), "净亏时不许挑出任何一段");
        assert_eq!(p.sigma, 3_000);
        assert!(p.delta > 50_000, "Δ 要把后缀里那段非候选也算进去：{}", p.delta);

        // ③ 后缀里两条候选：最好的 k 在最靠前 ⇒ k 之后**全瘦**，σ 是后缀候选之和
        let h = vec![text("hi"), tool(3_000), tool(30_000)];
        let p = worthwhile_elisions(&h, &[(1, 0, 3_000), (2, 0, 30_000)]);
        assert!(p.net > 0);
        assert_eq!(p.sigma, 33_000);
        assert_eq!(p.picked.len(), 2);

        // ④ 两条候选中间夹一大段非候选 ⇒ 最优的 k 落在**后面**那条
        let h = vec![text("hi"), tool(3_000), text(&"z".repeat(200_000)), tool(30_000)];
        let p = worthwhile_elisions(&h, &[(1, 0, 3_000), (3, 0, 30_000)]);
        assert_eq!(p.picked, vec![(3, 0, 30_000)], "只挑净收益最大的那一段");
        assert_eq!(p.sigma, 30_000);
        assert!(p.net > 0);
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
            sub: Arc::new(SubagentUsage::default()),
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
            {"type":"tool_use","id":"t1","name":"Cmd","input":{"command": long}},
            {"type":"tool_result","tool_use_id":"t1","content": long},
        ]});
        let rendered = render_one_message_for_summary(&msg);
        assert!(rendered.starts_with("assistant: [tool_use Cmd]"), "{rendered}");
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

    /// 技能调用（`Skill` 的入参指向哪个技能决定它是不是子代理）
    fn skill_call(key: &str) -> (String, String, Value) {
        ("id".into(), "Skill".into(), json!({ "skill": key }))
    }

    /// 把一轮调用解析成 `plan_tool_batches()` 要的 `subagents` 切片（判定只算一次）
    fn subagents<'a>(
        calls: &[(String, String, Value)],
        skills: &'a [skills::Skill],
    ) -> Vec<Option<SubagentCall<'a>>> {
        calls.iter().map(|(_, n, i)| subagent_call(n, i, skills)).collect()
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
        let none = subagents(&calls, &[]);
        let batches = plan_tool_batches(&calls, &none);

        // 区间必须无缝覆盖 [0, len)
        assert_eq!(batches[0].1.start, 0);
        assert_eq!(batches.last().unwrap().1.end, calls.len());
        for w in batches.windows(2) {
            assert_eq!(w[0].1.end, w[1].1.start, "相邻批之间不许有空隙/重叠");
        }

        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0], (BatchKind::ReadOnly, 0..2), "Read+Grep 连续只读 → 并行批");
        assert_eq!(batches[1], (BatchKind::Serial, 2..3), "Write 必须独占一批");
        assert_eq!(batches[2], (BatchKind::ReadOnly, 3..5));
    }

    /// 单元素的只读段不标并行（省一次线程 spawn，行为与串行完全一致）
    #[test]
    fn single_read_only_call_is_not_marked_parallel() {
        for calls in [
            vec![call("Read")],
            vec![call("Cmd"), call("Read")],
            vec![call("Read"), call("Cmd")],
        ] {
            let none = subagents(&calls, &[]);
            for (kind, _) in plan_tool_batches(&calls, &none) {
                assert_eq!(kind, BatchKind::Serial, "批内只有一个只读调用时不该标并行");
            }
        }
        assert!(plan_tool_batches(&[], &[]).is_empty());
    }

    /// `Agent` 与 fork 技能是**同一族**（都要 `cfg` + 独立工具循环），
    /// 所以连续的它们合成**同一个** `Subagent` 批；inline 技能不是子代理。
    #[test]
    fn consecutive_subagents_form_one_parallel_batch() {
        let skills = vec![
            skills::test_skill("forked", "---\nname: forked\ncontext: fork\n---\nbody"),
            skills::test_skill("inline", "---\nname: inline\n---\nbody"),
        ];

        // 两个 Agent 相邻 ⇒ 一个 Subagent 批
        let calls = vec![call("Agent"), call("Agent")];
        let subs = subagents(&calls, &skills);
        assert_eq!(plan_tool_batches(&calls, &subs), vec![(BatchKind::Subagent, 0..2)]);

        // fork 技能与 Agent 同类，相邻也是同一批（三种写法混着来）
        let calls = vec![call("Agent"), skill_call("forked"), skill_call("forked")];
        let subs = subagents(&calls, &skills);
        assert_eq!(plan_tool_batches(&calls, &subs), vec![(BatchKind::Subagent, 0..3)]);

        // inline 技能与 Read 同级 ⇒ 不是子代理；未知技能回落普通通道（由 Skill 自己报错）
        let calls = vec![skill_call("inline"), skill_call("nope")];
        let subs = subagents(&calls, &skills);
        assert!(subs.iter().all(Option::is_none), "inline / 未知技能不得当子代理");
    }

    /// 单个子代理不标并行（与只读批同一条不变量：单元素段省掉线程 spawn）
    #[test]
    fn single_subagent_is_not_marked_parallel() {
        let calls = vec![call("Agent")];
        let subs = subagents(&calls, &[]);
        assert_eq!(plan_tool_batches(&calls, &subs), vec![(BatchKind::Serial, 0..1)]);
    }

    /// 子代理可能写文件 / 跑命令 ⇒ 与前后调用之间存在**真实的先后依赖**，
    /// 子代理批与只读批都必须被别的调用打断，谁也不许跨过去重排。
    #[test]
    fn subagent_batches_break_on_either_side() {
        let skills = vec![skills::test_skill("f", "---\nname: f\ncontext: fork\n---\nbody")];
        // Agent, Read, Agent, fork ⇒ 两个单元素串行批 + 末尾的子代理批
        let calls = vec![call("Agent"), call("Read"), call("Agent"), skill_call("f")];
        let subs = subagents(&calls, &skills);
        assert_eq!(
            plan_tool_batches(&calls, &subs),
            vec![
                (BatchKind::Serial, 0..1),
                (BatchKind::Serial, 1..2),
                (BatchKind::Subagent, 2..4),
            ]
        );

        // 反过来：只读批也不许跨过中间的 Agent
        let calls = vec![call("Read"), call("Grep"), call("Agent"), call("Read"), call("Glob")];
        let subs = subagents(&calls, &skills);
        assert_eq!(
            plan_tool_batches(&calls, &subs),
            vec![
                (BatchKind::ReadOnly, 0..2),
                (BatchKind::Serial, 2..3),
                (BatchKind::ReadOnly, 3..5),
            ]
        );
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

    /// Read 遇图哨兵：`read()` 返回的标记文本会被 `tool_result_block` 换成 `[text, image]`
    /// 块；路径不可读时退回纯文本，绝不把「哨兵 + 乱码」泄给模型。
    #[test]
    fn read_image_sentinel_becomes_image_block() {
        let dir = std::env::temp_dir().join(format!("lunac-read-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png_bytes: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 7, 7, 7];
        let png = dir.join("shot.png");
        std::fs::write(&png, &png_bytes).unwrap();

        let note = "[image image/png, 0 KB] look at it directly; this line is only metadata.";
        let text = format!("{}{}\n{}\n", tools::IMAGE_SENTINEL, png.display(), note);
        let blk = tool_result_block("t1", text, false);
        assert_eq!(blk["type"], "tool_result");
        let content = blk["content"].as_array().expect("哨兵命中应产出块数组");
        assert_eq!(content.len(), 2, "先是说明文本，再是图片块");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], note);
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["media_type"], "image/png");

        // 路径不存在 ⇒ 退回纯文本
        let missing =
            format!("{}{}\n note\n", tools::IMAGE_SENTINEL, dir.join("nope.png").display());
        let blk = tool_result_block("t2", missing, false);
        assert!(blk["content"].is_string(), "读图失败应退回字符串内容");

        // 普通文本照旧、is_error 时不走哨兵
        assert_eq!(tool_result_block("t3", "hello".into(), false)["content"], "hello");
        let as_err = format!("{}{}\n note\n", tools::IMAGE_SENTINEL, png.display());
        assert!(tool_result_block("t4", as_err, true)["content"].is_string());

        std::fs::remove_dir_all(&dir).ok();
    }
}
