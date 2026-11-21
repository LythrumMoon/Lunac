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
//          --unattended-allow <names>     → 单发逐工具放行（只认 Write/Edit/NotebookEdit，
//                                           逗号分隔 / 可重复；常驻模式忽略）
//          --permission-prompt-tool stdio → 写类工具先发 can_use_tool 请前端审批
//          --disallowedTools <name…>      → 这些工具不进请求体
//          --mcp-server stdio:<exe 路径>  → 拉起该 exe 的 MCP server，接入其工具（P3）
//          --print / -p [query]           → 单发：跑恰好一轮提问后退出（见下）
//          --output-format text|json|stream-json → 单发模式的 stdout 形态（默认 text）
//          --include-partial-messages     → 单发 stream-json 下额外逐条输出流式增量
//                                           （stream_event；默认关，见 emit_stream_event）
//          --input-format stream-json     → 常驻 stdio 流式输入模式的标志（宿主一直传它）
//          --checkpoint                   → 每个用户回合结束后落一份检查点（默认关）
//          --resume <session-id|latest>   → 启动时从检查点恢复 history / 提问序号 / 任务预算
//                                           （见 checkpoint.rs；裸 --resume = latest）
//          --continue                     → 单发下 = 恢复最近检查点 + 用内置「继续未完成任务」
//                                           提问（不写 query 时）；同时隐含 --checkpoint
//          其余 CLI 风格参数接受并忽略
//
//   单发模式（--print）：只在「带 --print 且未显式声明 --input-format stream-json」时生效
//   —— 后者是上游常驻 stdio 流式输入模式（宿主的 start_cli_process 一直这么传），
//   必须保持常驻，绝不能退化成单发。query 来源：--print 紧跟的那个非 flag 值；
//   没给就从 stdin 读一次（整段当一条 query）。单发无审批回包通道 ⇒ 需审批的工具
//   默认直接拒绝（非 0 退出）；例外 = 用户用 --unattended-allow 逐工具点名的写类三件
//   （凭据扫描命中照旧拒），或用 --dangerously-skip-permissions 整体承担。详见 CliArgs::single_shot。
//   stdin  每行一条 JSON
//            {"type":"user","session_id":"","message":{"role":"user",
//             "content":[{"type":"text","text":"…"}]},"parent_tool_use_id":null}
//            content 里可再加图片块（A8）：{"type":"image","source":{"type":"file",
//             "path":"C:\\…\\a.png"}} —— 只传路径，字节由本进程读出来转 base64
//             （media_type 由魔术字节判定，发送方不必给）
//            {"type":"user",…,"steer":"insert"}  → **中途插入**（§待办 3-28.1）：只在
//             回合的轮首/轮末安全点注入 history，不中断当前工具批、不重跑已完成工具。
//             回合已结束时收到 ⇒ 丢弃并发 {"type":"system","subtype":"steer_dropped"}。
//            {"type":"pause"}                    → **暂停**：下一个安全点收口，强制落检查点，
//             发 result.subtype="paused"（不是错误）。
//            {"type":"control_response","response":{"subtype":"success",
//             "request_id":"…","response":{"behavior":"allow"|"deny",…}}}
//   stdout 每行一条 JSON（非 JSON 行会被前端忽略）
//            {"type":"system","subtype":"init","tools":[…]}
//            {"type":"stream_event","event":{…content_block_delta…}}
//            {"type":"assistant","message":{"content":[…]}}   ← 含 tool_use
//            {"type":"user","message":{"content":[{tool_result}]}}
//            {"type":"control_request","request":{"subtype":"can_use_tool",…}}
//            {"type":"result","subtype":"success"|"paused","usage":{…}}
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
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};

mod bash_safety;
// S2 长任务托管的检查点（2026-10-22）：每回合落一份 history + 提问序号 + 任务级预算累计，
// `--resume <session-id|latest>` 读回；默认不写（只有显式开关才落盘）。见 `checkpoint.rs`。
mod checkpoint;
// 能力路由（S1 第五切片，2026-10-24）：判定本轮「纯对话 / 直接用工具 / 派子代理」。
// **只记账、无下游消费者**（改工具数组会废固定前缀缓存，见模块头注释）。
mod capability_route;
mod conclusions;
mod content_safety;
// 决策驱动压缩（C 臂 MVP，2026-10-24）：在「已决定要瘦身」时让本地模型再判「能不能丢」。
// 只作用于 `Compact::Elide`（0.85 水位）；`Drop` / `Force` 等安全刚需档完全不经过它。
// 开关 `LUNAC_LOCAL_CURATE`（默认关）—— 它会改变压缩选择，必须能独立于 `LUNAC_LOCAL_DECIDE` 关掉。
mod curation;
// 本地「只读预筛」决策层（S1 首个切片，2026-10-15）：消费点 = 主循环里「是否派后台复盘 fork」
// （另在四个压缩触发点只记账）。见 `decide.rs` 头注释；开关 `LUNAC_LOCAL_DECIDE` 默认关，
// 未配 `LUNAC_LOCAL_MODEL_URL` 时整层透明关闭（行为与不接线逐字节一致）。
mod decide;
// 本地决策账本：独立追加式 JSONL，**不进 chat.db**（docs/forbidden.md §禁用 61）。
mod decisions;
mod hooks;
mod image;
// `ImageMeasure`（§待办 3-19 第一步，2026-10-30）：确定性像素测量，替模型「猜坐标」。
// 只读 / 本地 / 免审批；绘制那半见 `image_annotate`。
mod image_measure;
// `ImageAnnotate`（§待办 3-19 第二步，2026-10-30）：按规格 JSON 画尺寸线 / 箭头 / 中文文本，
// 输出 PNG。**写类**（要审批、只读档拒）；绘制走系统 GDI+，不随包分发字体。
mod image_annotate;
// `Screenshot`（§待。3-28.5 第一步，2026-11-16）：GDI 抓虚拟桌。/ 像素矩形 。PNG 落临时目录。
// **要审。*（隐私），计划相位也要问；不做后。/ 周期抓屏。
mod screenshot;
// `ImageDiff`（§待。3-28.5 第二步，2026-11-16）：golden 图逐像素比对，给占。/ 包围。/
// 平均绝对差。只读、免审批；尺寸不同直接报错（绝不缩放）。
mod image_diff;
mod log;
// LSP 诊断工具（A13 剩余能力，2026-10-23）：起本机语言服务器、取**一个文件**的诊断。
// 只读 / 本地 / 免审批 / 失败 fail-open（见 `lsp.rs` 文件头）；不碰固定前缀，工具描述随请求体走。
mod lsp;
mod mcp;
mod mcp_oauth;
// 记忆候选抽取（S1 第二切片，2026-10-15）：从回合里抽「值得记住的事实」，**只落账本**
// （绝不写 MEMORY.md / 绝不改任何注入）。见 `memory_candidates.rs` 头注释；开关复用
// `LUNAC_LOCAL_DECIDE`（默认关），摊销口径 = 每 K 轮 + 压缩事件（与 S1 首切片同源）。
mod memory_candidates;
// 长期记忆的只读检索内核（§待办 3-9 B 项，2026-10-17）：纯函数切 bullet + 检索排序。
// 正文仍由 `fetch_memory`（既有 `lunac/memory_read` 桥）读入；本模块不读盘、不写盘、
// 不做任何注入，也不产生自动调用点。
mod memory_search;
mod peers;
// 压缩指针回注（2026-10-27，§待办 3-16 P1-7）：把被压缩掉的**可重跑**工具调用渲染成
// `[elided pointers]` 回注到 history 的可变层 —— 模型知道「查过什么」之后还知道「怎么再查」。
// 来源是 §待办 3-12 实验 A 组的交付物（`D:\test\A\ptrblock`），见模块头注释。
mod ptrblock;
// PTC / Code Mode（§待。3-28.15。026-11-19）：内嵌 QuickJS 跑模型写的只读批处理脚本。
// 默认关（`LUNAC_PTC=on` 才注。`RunScript`）；绑定表只。Read / Glob / Grep，硬预算
// 50 。· 60s · 256 KiB 中间结果。*不是 OS 级沙。*，见 `ptc.rs` 头注释。
mod ptc;
// 检索查询改写（S1 第三切片，2026-10-24）：把一条检索查询扩成额外关键词，**只做增量召回**
// （原命中不减少、顺序不动）。消费点 = `MemorySearch`；开关复用 `LUNAC_LOCAL_DECIDE`（默认关），
// 故障 / 通道忙一律 fail-open 用原查询。见 `query_rewrite.rs` 头注释。
mod query_rewrite;
mod resource_hub_tools;
// 安全预筛（S1 第四切片，2026-10-24）：给「需人工确认的工具调用」打危险度档位，**只记账**。
// 不改审批 / 不放行 / 不拦截（审批唯一决策点仍在前端，见 docs/forbidden.md §禁用 1）。
// 生产入口 `maybe_safety_prescreen` 一律 fire-and-forget（后台线程），不在工具路径上等待。
mod safety_prescreen;
// 第二模型预审（§待。3-28.14。026-11-16）：挂在「本来就要弹人工卡」之前，
// 输出只有 `deny` / `escalate` 两态，**永不 allow**；故。/ 超时 / 认不出一律交给人。
// 开。`LUNAC_SECOND_REVIEW`（默认关，缺省与不接线逐字节一致）。
mod second_review;
mod skills;
// 子代理角色（2026-10-24）：工具白名单 + 角色提示词的单一真相源。
mod subagent_role;
// 角色常驻（§待办 3-22 分片 4，2026-10-31）：同一角色复用上一段子代理对话。
// 只挂在检查点开关下（`--checkpoint` / `--resume` / `--continue`），默认不写盘。
mod subagent_session;
mod tools;
// WebBrowser（A13 最后一项，2026-10-23）：起本机无头 Edge/Chrome 渲染一次、取跑完 JS 的 DOM。
// 零新依赖（不引 Playwright）；外部网络出口 ⇒ 要审批（见 `tools.rs` 的判据）。
mod webbrowser;

/// 单次回复的 token 上限（无思考时的基线）
const BASE_MAX_TOKENS: u32 = 8192;
/// 请求总超时（含流式读取整段响应）
const REQUEST_TIMEOUT_SECS: u64 = 1800;
const CONNECT_TIMEOUT_SECS: u64 = 30;
/// 收口轮）；结论与那个数字**无关** —— 它量的是「动到前缀末尾的块结构会怎样」：
///
/// 追加 `text` 块（2026-09-20 单变量实测，不得回退）。
///   ① 实测（`release` 版 `temp\logs\agent-*.log`）：9 次提问里 **3 次把 16 打满**
/// 一轮用户提问内最多允许的「模型→工具→模型」往返次数。
/// 收口轮）；结论与那个数字**无关** —— 它量的是「动到前缀末尾的块结构会怎样」：
/// 下表的「第 17 轮」是**当年 `MAX_TOOL_ROUNDS = 16` 时的第 17 次请求**（= 打满后的
///
/// 一轮用户提问内最多允许的「模型→工具→模型」往返次数。
/// 收口轮）；结论与那个数字**无关** —— 它量的是「动到前缀末尾的块结构会怎样」：
const MAX_TOOL_ROUNDS_ENV: &str = "LUNAC_MAX_TOOL_ROUNDS";
/// 一轮用户提问内最多允许的「模型→工具→模型」往返次数。
const DEFAULT_MAX_TOOL_ROUNDS: usize = 0;

/// 收口轮）；结论与那个数字**无关** —— 它量的是「动到前缀末尾的块结构会怎样」：
fn max_tool_rounds() -> usize {
    match std::env::var(MAX_TOOL_ROUNDS_ENV) {
        Ok(v) => v.trim().parse::<usize>().unwrap_or(DEFAULT_MAX_TOOL_ROUNDS),
        Err(_) => DEFAULT_MAX_TOOL_ROUNDS,
    }
}
/// 一轮用户提问内最多允许的「模型→工具→模型」往返次数。
///
/// **落点必须让 history 的形状与块数量都不变** —— 即拼进上一条 `tool_result` 的
/// **文本内部**，不得单独 `push` 一条新的 `user` 消息、也不得在消息的 content 数组里
/// 追加 `text` 块（2026-09-20 单变量实测，不得回退）。
///
/// 下表的「第 17 轮」是**当年 `MAX_TOOL_ROUNDS = 16` 时的第 17 次请求**（= 打满后的
///      （见下），轮次退化成**纯兜底**（防「预算判据失效 + 模型失控」这类组合失效）。
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
//   · 这 4 轮的工具名**相同**；
//   · 最近 `STUCK_ROUNDS` 轮**每轮都只调用一个工具**；
//   · 这 4 轮的工具名**相同**；
//   · 这 4 轮的**结果文本头部相同**（`STUCK_FP_CHARS` 个字符）。
const STUCK_ROUNDS: usize = 4;
/// 头部又足以区分「读的是不同文件」（路径就在开头）。
/// （`Found N messages …`），整段比对会因为命中条数不同而永远不相等，反而抓不到。
/// 头部又足以区分「读的是不同文件」（路径就在开头）。
const STUCK_FP_CHARS: usize = 120;
/// 判定卡住后给模型的一次提示。**先提示、再收口**（用户 2026-10-01 定的处置）：
/// 误判的代价只是一句提示，而「再试一下就能通」的情况不少，直接掐掉太亏。
const STUCK_HINT: &str = "You have called the same tool repeatedly and the results are not \
changing — you are not making progress. Stop repeating it: either change your approach \
(a different tool, a more specific query, or reading the actual file), or give the user \
your best answer so far and state clearly what is still missing.";

// ── 单次提问的成本预算（2026-10-01；2026-11-02 改为「加权计点」）──────
//
// 为什么要有它：`STUCK_ROUNDS` 的判据要求「结果文本头部相同」，可错误文本里常带路径。
// 时间戳、随。id 。同一条命令连。5 轮也判不出来；模型在两个工具间来回切换时更是永远
// 不触发。实测（temp/logs）里出现过「Cmd 连续失败 7 轮、每轮错误文本不同」的空转。
// 这里只看 `is_error`，与文本内容无关，两条护栏互补：
//   · 连续 `TOOL_FAIL_HINT_ROUNDS` 。*只调一个工具且失败** 。提示一次（换方。/ 如实说）。
//   · 连续 `TOOL_FAIL_STOP_ROUNDS` 轮仍失败 。硬收口（已完成改动都保留，不让空转烧钱）。
// 只统。*单工具轮**：并行调多个工具说明模型仍在推进，不算「卡在一个工具上」。
const TOOL_FAIL_HINT_ROUNDS: usize = 2;
const TOOL_FAIL_STOP_ROUNDS: usize = 4;
const TOOL_FAIL_HINT: &str = "This tool has now failed several rounds in a row. Do not retry it \
with the same idea again: switch to another tool, change the arguments materially, or stop and \
tell the user plainly what is blocking you and what you have already verified.";

// 缓存命中（2026-11-01 全天 37.29M 里 34.98M），按面值计点会让 50 万预算在上下文只占
//
// 闸门口径 = 该提问内**所有请。*的加权计点：`in + create + out` 全额，`read`（缓存命中）
// 。`CACHE_READ_BUDGET_WEIGHT_PCT` 计。为什么改：实测长会话 **93.8%** 的输。token 。
// **只续命、不压缩**（2026-11-02）—— 压缩必然丢掉 31–55 条消息 + 一次摘要调用 +
// 1/3 时就撞顶 —。那天 3 小时里因此压。58 次。账面用量（in/read/create/out）不变，
// 前端 `usage-*.jsonl` 的对账口径也不变，只有这道闸的计点换了权重。
//
//      所以续命本质上是把闸门**渐进地抬高一档**，`MAX_BUDGET_EXTENSIONS` 才是真上限。
//      再压就是白废一次缓存 + 白花一次摘要钱，那种情况**直接收口**。
// 整段前缀缓存作废，而实测那些压缩发生时上下文只。128k 。20.7%。2.1%，压完反而让
// 模型失去中段对话、开始重做。只有上下文真的接近上限才做 `compact_history(Force)`。
//
// ⚠️ **两件必须说清的事**：
// ⚠️ **两件必须说清的事**：
//      所以续命本质上是把闸门**渐进地抬高一档**，`MAX_BUDGET_EXTENSIONS` 才是真上限。
//   ② 真要做压缩时**必须真的把体积降下来**才算数。`Force` 档已经压到只留最近 2 条，
//      若它 `elided + dropped == 0`，说明体积在**对话本身**（不是工具结果）——
//      再压就是白废一次缓存 + 白花一次摘要钱，那种情况**直接收口**。
const TURN_BUDGET_ENV: &str = "LUNAC_TURN_BUDGET_TOKENS";
/// 撞预算后最多续命几次（用户 2026-10-01 定 2 次）。最坏累计 ≈ 3 × 预算。
///
/// 证据（`temp\logs\agent-2026-11-10.log`）：默认 50 万计点在长任务里一晚撞。5 次，
/// 每次都触。`Compact::Force` 丢掉 101。84 条旧消息 —。而当时上下文只占 66% / 81% /
/// 96% / 98%，其。66% / 81% 两次根本没到水位，压完模型就丢掉中段对话、开始重做。
/// 现在压缩只在真实水位。.85 瘦身 / 0.95 丢弃）与 400 兜底时发生；需要成本闸门时
/// 默认 50 万 —— 与子代理的 `SUBAGENT_BUDGET_TOKENS`(30 万) 同量级，但主对话
const DEFAULT_TURN_BUDGET_TOKENS: u64 = 0;
/// 撞预算后最多续命几次（用户 2026-10-01 定 2 次）。最坏累计 ≈ 3 × 预算。
/// **只用于「没有未完成的任务清单」的普通问答**；有计划且计划未完成时**不数次数**
/// （2026-11-03 用户口径「任务完成前不因预算退出」，见 [`budget_extension_allowed`]）。
const MAX_BUDGET_EXTENSIONS: usize = 2;
/// 上下文占上下文预算低于这个比例 ⇒ 预算撞顶时**跳过压缩**、直接续命（2026-11-02）。
/// 预算计点（`budget_points`）与子代理预算（`subagent_usage_units`）都从它折算。
/// 不再各写一份魔数。
///
/// 取值来源：`config\pricing.json` 预置表（`app-flutter/rust/src/usage.rs` 。
/// `DEFAULT_PRICING_JSON`，DeepSeek 官方价目，元 / 百万 token）—。
/// **只影响计点，不改账面用量**（`in` / `read` / `out` 原样上报）。
/// 口径写的，对现端点把缓存命中**高估。5 。*，长会话。3.8% 命中）会提前撞顶。
/// **只影响计点，不改账面用量**（`in` / `read` / `out` 原样上报）。
const CACHE_READ_PRICE_PCT: u64 = 2;
/// 上下文占上下文预算低于这个比例 ⇒ 预算撞顶时**跳过压缩**、直接续命（2026-11-02）。
/// 判据来自实测：58 次压缩全部发生在 20.7%–52.1% 之间，没有一次是水位逼出来的。
const BUDGET_EXTEND_ROOMY_RATIO: f64 = 0.6;
/// 同一提问内「预算触发」的强制压缩次数上限。轮首水位档与 400 兜底**不在此列**
/// （那两档是安全刚需，不受此限）。
const MAX_BUDGET_COMPACTS_PER_TURN: usize = 1;
/// 两次预算压缩之间至少相隔的轮数（防「压完立刻再撞、再压」）。
const BUDGET_COMPACT_MIN_ROUNDS: usize = 6;
/// 预算用尽时的收口指令。与 `TOOL_BUDGET_HINT` 同款措辞、同一个落点纪律。
const TURN_BUDGET_HINT: &str =
    "This turn's cost budget is used up. Stop calling tools and give the user your final \
answer now, listing what is done and what remains.";
/// 硬收尾时补进**最后一条 `tool_result`** 的断点说明（2026-11-03）。
///
/// 为什么需要：硬收尾只改 `result.result`（给用户看的那句），而 agent 自己的 `history`
/// 里最后一条助手正文仍是**半截**。用户接着说「继续」时，模型只看到一条戛然而止的助手
/// 里最后一条助手正文仍是**半截**。用户接着说「继续」时，模型只看到一条戛然而止的助手
/// 这里把断点与「不要重复」写进模型下一轮一定会读到的位置（尾部新内容，不碰前缀缓存）。
const CUT_OFF_NOTE: &str =
    "[Lunac harness, not tool output] The previous reply was cut off before the task finished. \
If the user asks you to continue, resume from exactly where it stopped. Do NOT repeat or \
restate any text you already produced above.";

// ── 任务级预算（S2 长任务托管第一刀，2026-10-22）─────────────────────
//
// **与回合预算的区别**：`TURN_BUDGET_TOKENS` 是「一次用户提问」的软闸 —— 撞顶后压缩续命、
// 再清零计点；有计划且计划未完成时不数续命次数，只有「上下文再也压不动 / 不宽裕」才停。
// 本闸是「同一个 agent 进程（= 一段连续任务）」的**跨提问累计**：用户连着追问十轮，
// 钱是加在一起算的，撞顶后**不再续命**（否则续命就把任务级上限也一起抬走了）。
//
// 本切片（有意的最小档，边界如实记）：
//   · **只测量 + 收紧**：撞顶后不硬拒绝新提问（S2 的目标是「预算不再终止计划」，
//     真正的「断点续跑」需要检查点 / 调度层，本切片不做）；
//     真正的「断点续跑」需要检查点 / 调度层，本切片不做）；
//   · 撞顶。*所有后续提问禁用预算续。* 。每问最多一个回合预算，不再膨胀。7 倍；
//   · 口径与回合预算**逐字一致**（主请求的 `in + read + create + out`；不含子代理 /
//     复盘 fork —— 它们的预算各自独立，见 `SUBAGENT_BUDGET_TOKENS` / `REVIEW_BUDGET_TOKENS`）；
//   · `0` = 关闭（默认）—— 任务级是**整段任务**的口径，不能默认打开。
const TASK_BUDGET_ENV: &str = "LUNAC_TASK_BUDGET_TOKENS";
const DEFAULT_TASK_BUDGET_TOKENS: u64 = 0;
/// 任务级预算撞顶时的收口指令。与 `TURN_BUDGET_HINT` 同款措辞、同一个落点纪律。
///
/// **必须显式写明「来自 harness、不是工具输出」**：这句话是拼在最后一条 `tool_result`
/// 文本尾部的（零代价落点，见 `append_hint_to_last_tool_result`），紧跟在 `Read` 的文件
/// 内容后面 ⇒ 2026-10-22 实测模型把它当成「文件里夹带的伪造指令」并特意声明不执行。
/// 加上来源限定后语义才不会被误读成被读取内容的一部分。
const TASK_BUDGET_HINT: &str =
    "The Lunac harness (not the tool output above) reports that the task-level cost budget \
for this session is used up. Stop calling tools and give the user your final answer now, \
listing what is done and what remains.";

/// 任务级预算计数器：**跨提问存活**（由 `main()` 持有、按 `&mut` 传进 `run_query`）。
///
/// `used` 只累加主请求的用量；`warned` 保证「撞顶」这件事整个进程只上报一次
/// （跨提问仍是同一条预算，重复提示只会把状态栏刷成噪音）。
struct TaskBudget {
    limit: u64,
    used: u64,
    warned: bool,
}

impl TaskBudget {
    fn new(limit: u64) -> Self {
        Self { limit, used: 0, warned: false }
    }

    /// 记一笔用量（与 `turn_tokens` 的三个累加点逐处对齐）。
    fn record(&mut self, tokens: u64) {
        self.used = self.used.saturating_add(tokens);
    }

    /// `limit == 0` ⇒ 整条闸门关闭（逐字节保持旧行为）。
    fn over(&self) -> bool {
        self.limit > 0 && self.used >= self.limit
    }

    fn describe(&self) -> String {
        format!("{}/{}", self.used, self.limit)
    }
}

/// 解析任务级预算上限。坏值 / 空值一律退回默认（= 关闭），与 `turn_budget_tokens` 同风格 ——
/// 宁可不开闸，也不要让一个笔误的环境变量把长任务掐死。
fn parse_task_budget(raw: Option<&str>) -> u64 {
    match raw {
        Some(v) => v.trim().parse::<u64>().unwrap_or(DEFAULT_TASK_BUDGET_TOKENS),
        None => DEFAULT_TASK_BUDGET_TOKENS,
    }
}

fn task_budget_tokens() -> u64 {
    parse_task_budget(std::env::var(TASK_BUDGET_ENV).ok().as_deref())
}

/// 回合预算撞顶后能否续命。
///
/// 语义（2026-11-03 用户口径「任务完成前不因预算退出」）：
/// * **有计划且未完成** ⇒ 预算只当节流阀：忽略续命次数上限、忽略任务级预算撞顶，
///   只要上下文还能安全继续（压动了 / 仍宽裕）就一直续；
/// * **没有未完成计划** ⇒ 保持旧行为（`MAX_BUDGET_EXTENSIONS` 次上限、任务级一票否决），
///   只要上下文还能安全继续（压动了 / 仍宽裕）就一直续；
fn budget_extension_allowed(
    compressed: bool,
    roomy: bool,
    extensions: usize,
    cap: usize,
    task_over: bool,
    plan_unfinished: bool,
) -> bool {
    if plan_unfinished {
        return compressed || roomy;
    }
    (compressed || roomy) && !task_over && extensions < cap
}

/// 预算撞顶时**是否值得做一次 `Compact::Force`**（纯函数，便于单测）。
///
/// 三条否决理由：
///   · `compacted` —— 本轮轮首已经压过（同一轮再压一次只是白废缓存）；
///   · `roomy` —— 上下文还没到 60%，压了只会丢消息（2026-11-02 的主修复）；
///   · 本问已压次数到顶 / 距上次压缩不足 `BUDGET_COMPACT_MIN_ROUNDS` 轮。
fn should_budget_compact(compacted: bool, roomy: bool, done: usize, rounds_since: usize) -> bool {
    !compacted
        && !roomy
        && done < MAX_BUDGET_COMPACTS_PER_TURN
        && (done == 0 || rounds_since >= BUDGET_COMPACT_MIN_ROUNDS)
}

/// 把一次请求的用量折成「预算计点」。**唯一实现** —— 三个累加点都走它
/// （主请求、`message_delta` 补报、后台复盘并入）。缓存命中按权重折算，
/// 账面用量不受影响。
fn budget_points(input: u64, read: u64, create: u64) -> u64 {
    input
        .saturating_add(create)
        .saturating_add(read.saturating_mul(CACHE_READ_PRICE_PCT) / 100)
}

// ── 端点侧缓存的冷/热（2026-10-01，用户要求）──────────────────────
//
// `result` 事件里的费用原先恒为 `0.0`（字段名 `total_cost_usd`，前端零引用）。
// 现在。*同一张价目表**真算，字段改名为 `total_cost`（`{amount, currency, basis}`）：
//   · 币种。**CNY**（`pricing.json` 的单位是「元 / 百万 token」），叫 `_usd` 就是错的。
//     而换算美元需要汇。—。项目里没有、也不该编一个；
//   · 单价取自 `app-flutter/rust/src/usage.rs` 。`DEFAULT_PRICING_JSON`（DeepSeek 官方价目。
//     非高峰基础价）：`input=1 / cache_read=0.02 / cache_write=0 / output=4`。
//   · **不含峰谷 / 节假日价**（那要按日期与小时查 `time_windows`）⇒ 这个数只。*下界**。
//     权威值仍是宿主的 `usage.rs` + `pricing.json`（按天、按时段、按模型计价）。
const PRICE_INPUT_PER_MTOK: f64 = 1.0;
const PRICE_CACHE_READ_PER_MTOK: f64 = 0.02;
const PRICE_CACHE_WRITE_PER_MTOK: f64 = 0.0;
const PRICE_OUTPUT_PER_MTOK: f64 = 4.0;
const PRICE_CURRENCY: &str = "CNY";
/// `basis` 只写一句人话，说明「这个金额是怎么来的、为什么可能偏低」。
const PRICE_BASIS: &str = "pricing.json 预置基础价（不含峰谷），宿主 usage.rs 为权威";

/// 这次提问的总费用（`pricing.json` 的币种，单位「元」）。四项分别按每百。token 单价折算。
///
/// 纯函数、整数入参：同一份用量永远得到同一个数（与 `budget_points` 的纪律一致）。
fn usage_cost(input: u64, cache_read: u64, cache_create: u64, output: u64) -> f64 {
    let per_m = |tokens: u64, price: f64| tokens as f64 * price / 1_000_000.0;
    per_m(input, PRICE_INPUT_PER_MTOK)
        + per_m(cache_read, PRICE_CACHE_READ_PER_MTOK)
        + per_m(cache_create, PRICE_CACHE_WRITE_PER_MTOK)
        + per_m(output, PRICE_OUTPUT_PER_MTOK)
}

/// `result` 事件里的 `total_cost` 字段（四项用。`{amount, currency, basis}`）。
fn cost_json(input: u64, cache_read: u64, cache_create: u64, output: u64) -> Value {
    json!({
        "amount": usage_cost(input, cache_read, cache_create, output),
        "currency": PRICE_CURRENCY,
        "basis": PRICE_BASIS,
    })
}

// ── 端点侧缓存的冷/热（2026-10-01，用户要求）──────────────────────
//
// 由来：`compact_history` 的收益判据（`worthwhile_elisions`）算的是「省下的 vs
// **被作废的**」，而「被作废」只有在**端点侧那份缓存还热着**的时候才是真代价。
// 距上次请求已经很久（超过本 TTL）时，那份缓存**本来就过期了** —— 此时动历史
// **零额外代价**，正是 Claude Code `microCompact` 的「冷路径」：
// 缓存。直接改内容（反正要重算）；缓存热 。绝不动。
//
// 所以这条判据的作用是**单向放宽**：冷 ⇒ 不受水位与滞回约束，可以每轮清一次
// （「每轮微清理」）；热 。保持原有全部约束。任何情况下都不。*。*激进地压热缓存。
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
// 不进主对。—。这正是它省上下文的地方。
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

/// 8 轮把「明明没花完钱」的任务提前截断成了部分报告。
///
/// **2026-10-16 由 8 提高到 20**。依据：实测 13/13 个子代理**全部**在 8 轮撞顶，而
/// `SUBAGENT_BUDGET_TOKENS`（30 万）平均只用了约 40% ⇒ 卡住它们的是**轮数**不是钱，
/// 8 轮把「明明没花完钱」的任务提前截断成了部分报告。
///
/// 提到 20 后**真正的钱闸换成 token 预算**：按 8 轮约 12 万 token（= 30 万的 40%）
/// 反推，线性外推约在第 20 轮触顶；但子代理上下文逐轮增长、后段每轮更贵，实际会
/// 更早触发，**大致第 13–15 轮**。⇒ 单个子代理成本仍封顶在 30 万 token 量级，
/// 轮数退化成**纯兜底**（防「预算判据失效 + 模型失控」的组合失效），与主对话
/// `MAX_TOOL_ROUNDS` 的分工一致。后台复盘**不跟这个值走**（保持 `MAX_REVIEW_ROUNDS = 4`）。
const MAX_SUBAGENT_ROUNDS: usize = 20;
/// 单个子代理的预算封顶（**加权单位**，不是原始 token 数 —— 2026-10-24 改）。
///
/// 为什么改：旧口径把 `cache_read` 按原价计进预算，而它的单价只有 miss 的 1/50。
/// 实测 9 个子代理里 6 个在第 11–13 轮、~30 万原始 token 处被掐断，交回 134–345 字的残片；
/// 按平台单价折算，它们实际只花了很小一笔。现在按「钱」计：
/// `in×1 + out×4 + cache_read×2% + cache_create×1.25`（缓存命中比例见
/// `CACHE_READ_PRICE_PCT`，与 `pricing.json` 同源），
/// 300_000 单位 ≈ 30 万未命中输入 token 的成本量级，缓存命中不再吃掉额度。
const SUBAGENT_BUDGET_TOKENS: u64 = 300_000;
/// 子代理**倒数第二轮**的收口指令：拼进最后一条 `tool_result`，提醒它「下一轮就是最后一轮，
/// 用完整报告收口」，避免拖到轮数打满才被硬截成部分报告（2026-10-16）。
///
/// 落点纪律同 `TURN_BUDGET_HINT`：**拼进已有 `tool_result` 的文本内部**，不动消息条数 /
/// 块结构。此刻那条 `tool_result` 是上一轮刚 push、**还没发出去过**的（不在任何已缓存前缀
/// 里）⇒ 改它对子代理自己的前缀缓存**零代价**，且全程**只注入一次**。它只进子代理自己的
/// history，**不碰主对话前缀**。
const SUBAGENT_WRAP_HINT: &str =
    "You are approaching your final round or your budget limit. Stop exploring and write the \
final report NOW, in exactly three parts: (1) Conclusion -- what you found or did, a few \
sentences; (2) Evidence -- exact file:line or command -> result for each claim; (3) Not \
verified -- what you did not check and the assumptions you made. Do not retell your steps \
and do not paste raw tool output.";
/// 预算用量 → 加权单位（2026-10-24）。权重与平台单价同源：
/// 未命中输入 1、输出 4、缓存写入 1.25、**缓存命中 1/50**。
///
/// 用整数运算（`read / 50`）而不是浮点：预算是闸门，同一份用量必须永远得到同一个整数，
/// 否则「刚好在边界」的两次运行会一次过、一次不过。
fn subagent_usage_units(input: u64, output: u64, cache_read: u64, cache_create: u64) -> u64 {
    input
        .saturating_add(output.saturating_mul(4))
        .saturating_add(cache_read.saturating_mul(CACHE_READ_PRICE_PCT) / 100)
        .saturating_add(cache_create.saturating_mul(5) / 4)
}

/// 收口线：预算用到 80% 就注入一次「写完整报告」的提示（不等轮数倒数第二）。
const SUBAGENT_BUDGET_WRAP_PERCENT: u64 = 80;

/// 是否已到收口线（按加权预算）。
fn subagent_budget_wrap_due(spent: u64, budget: u64) -> bool {
    budget > 0 && spent.saturating_mul(100) >= budget.saturating_mul(SUBAGENT_BUDGET_WRAP_PERCENT)
}

/// 撞预算时补进 `tool_result` 的说明：既告诉子代理别再调工具，也告诉它这一轮不执行。
const SUBAGENT_BUDGET_SKIP: &str =
    "Not executed: this subagent reached its budget. Do not call more tools. Write the final \
report now, including what was completed, what was not, and any evidence you already have.";
/// 回灌给主对话的子代理报告字符上限。子代理的返回值**不走** `tools::apply_budget`
/// （那条路只覆盖内置工具），不在这里收口的话，一份长报告能直接把主对话上下文顶爆。
///
/// 2026-10-27：**按角色分级**，见 `subagent_role::Role::report_chars()`（explore 1.5k /
/// review 2k / coding 与 research 3k / general 8k）。这里保留 8,000 只作两处的缺省：
/// `Role::General` 与**fork 技能**（技能没有角色，维持旧行为）。
const SUBAGENT_REPORT_CHARS: usize = 8_000;
/// 子代理**内部过程转发**的**单条**内容上限（字符）。转发的初衷是让用户看到「它在做什么」，
/// 且（2026-10-16 用户要求「让用户清楚看到子代理干了什么」）**不再把内容截短** —— 旧值
/// 4000 会在单块文本（thinking / text / tool 入参 / tool_result）稍长时就切掉，用户看不到全貌。
/// 新上限 **200_000**（与前端单命令终端缓冲 `TERM_BUF_MAX=200_000` 同量级），仍保留
/// `log::truncate_chars` + `…(truncated)` 作为**极端兜底**（绝不静默丢），只是现实里够不着。
/// 超限的代价评估：前端本就对渲染做二次收敛（`.tool-out` 只显示前 600 字符、思考展开走
/// `THINK_INLINE_MAX`、终端缓冲封顶 200k），所以放开这里不会把 DOM 撑爆；涨的是单行 IPC 体积。
const SUBAGENT_EVENT_ITEM_CHARS: usize = 200_000;
/// 子代理**内部过程转发**的**整段**上限（字符，跨所有轮累计）。一个多轮、每轮跑命令的子代理
/// 能产出几十万字符；完全不设上限有风险（单行 IPC / 前端内存），故给一个与**轮数上限匹配**的
/// 足够大上限 —— 否则轮数越多越容易在正常任务上撞顶（旧值 `2_000_000` ≈ 10 条满额块，而轮数
/// 已从 8 提到 `MAX_SUBAGENT_ROUNDS = 20`，20 轮下更易撞顶）。
///
/// **取值依据（2026-10-16）**：`MAX_SUBAGENT_ROUNDS` 轮 × 每轮最多 2 条转发消息（assistant
/// 内容块 + `tool_result`）× 单条上限 `SUBAGENT_EVENT_ITEM_CHARS` = `20 × 2 × 200_000`
/// = **8_000_000**。绝大多数子代理远达不到，只有病态外溢才触发兜底。仍保留整段上限（而非
/// 取消）：它是**极端兜底**，防「模型失控 + 逐块都不超单条上限」的组合把单行 IPC / 前端内存
/// 一起顶爆。用尽后只发一条 `truncated:true` 标记，之后不再转发（明确标记，不静默丢）。
const SUBAGENT_EVENT_TOTAL_CHARS: usize = 8_000_000;
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
- Write that report in exactly three parts, in this order:
  1) Conclusion -- what you found or did, in a few sentences;
  2) Evidence -- the exact `file:line` (or `command -> result`) behind each claim;
  3) Not verified -- what you did NOT check, plus any assumption you had to make.
- Do not retell your steps, do not paste raw tool output, and do not restate the task.
- Do not ask the user questions; you have no interactive channel.";
/// 进程内存态的 task 序号 —— 它要出现在给模型看的回灌文本里，短才好读；
/// 跨进程的唯一性由 `session_id()` 补上（约束②）
static TASK_SEQ: AtomicU64 = AtomicU64::new(0);

// ── 后台复盘 fork（A4：长期记忆的**触发点**）──────────────────────
//
// 触发条件（2026-10-11 方案 B：由「进程内每 N 轮」改为「按会话 + 按时间」；
//           2026-10-13 折中：每会话配额**按作用域各一份**，时间闸仍共享）：
//   ① 本轮确实完成了一次用户提问（判定点就在 user 分支、run_query 返回之后）；
//   ② 距上次复盘 ≥ REVIEW_MIN_GAP_SECS（30 分钟）—— **两侧共享**同一个「上次时间」，
//      任一作用域刚复盘过，另一侧 30 分钟内也不跑；
//   ③ **本轮 scope** 在本次会话（= 本 agent 进程）尚未复盘过（每会话每 scope ≤ 1 次）。
// 三条全满足才派一个后台 fork 去复盘这段对话，把值得留的结论用 Remember 写进长期记忆。
//
// 为什么放弃「进程内每 N 轮」：旧的 questions 是**进程内局部变量**，而 agent 频繁重启
// （换模型 / 换档位 / 新对话 / 中断 / App 重启都会重启 agent）⇒ 计数归零，阈值（默认 10）
// 永远够不着，写入侧事实上从未触发过（2026-10-11 运行时证据）。改成「按会话 + 按时间」后，
// 门槛不再依赖单进程活多久。
//
// 频次硬上限（用户成本敏感）：**每会话每作用域 ≤ 1 次**（会话 = 本进程）+ **距上次复盘
// ≥ 30 分钟**（时间戳落盘、跨进程重启有效、两侧共享）⇒ 一次触发只 spawn 一个后台线程、
// 无循环重。单次 。1 次云调用。
//
// **不引定时器**（backlog 硬约束）：定时器会在用户什么都没干的时候空转（每次都真花钱）。
//
// 「后台」的确切含义：**在提问之间**跑。run_query 返回（答案已交付前端）之后才 spawn，
// 下一次提问进来之前 join —— 所以它**不占任何一次回答的时延**，用户读答案/打字的那几秒
// 正好把复盘跑完。它不是「随时并发」：agent 的 stdout 契约是逐行 JSON，两条线程同时
// emit 会交错（见 run_review_fork 的注释）。
//
// 为什么值得：取回侧（SessionSearch）2026-09-19 就通了，但**一直只有取回、没有写回**
// —— 用户每次都要重新交代一遍相同的偏好与约定。

/// 距上次复盘的最小间隔（秒）。方案 B 的「按时间」那一半 —— 与「每会话 ≤ 1 次」叠加，
/// 共同封顶复盘频次。取 30 分钟：一次复盘真调模型（预算 12 万 token），用户成本敏感，
/// 一天至多几次足够；再密只是把同样的结论反复写。
const REVIEW_MIN_GAP_SECS: i64 = 30 * 60;

/// 复盘 fork 的轮次上限与 token 预算。比子代理（20 轮 / 30 万）更小：它只做
/// 「判断 + 写几条」，正常一两轮就结束；上限只是防它失控烧钱。
const MAX_REVIEW_ROUNDS: usize = 4;
const REVIEW_BUDGET_TOKENS: u64 = 120_000;
/// 送进复盘的对话快照上限（字符）。复盘看的是**结论**，不需要逐字原文 ——
/// 快照太大只会让这次后台调用变贵。
const REVIEW_TRANSCRIPT_CHARS: usize = 6_000;
/// 一次复盘最多写几条记忆（写在提示词里）。复盘频次低（每会话至多一次），贪多必然灌进一堆琐事。
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

/// 复盘 fork 的系统提示词（**角色段**）。与 SUBAGENT_SYSTEM 一样是给模型看的元信息，
/// 不进 i18n。它在 main() 里拼一次（角色段 + 环境块 + 技能清单），所有复盘共享同一段前缀。
///
/// 2026-10-11（方案甲）：从泛化复盘升级为**结构化抽取** —— 写明抽取分类、不抽清单与
/// **禁写凭据**四类口径；触发条件、频次、工具集、写回通道一行未动。这段只出现在复盘
/// 自己那一次请求的 system 上（见 build_review_system 的唯一调用点），与主对话的固定
/// 前缀不是同一段，改它不影响主对话缓存。
const REVIEW_SYSTEM: &str = "\
You are Lunac's background reviewer. A conversation has just finished and you are extracting \
what, if anything, deserves to be kept in the user's LONG-TERM MEMORY. This is structured \
extraction with a fixed scope, not a free-form retrospective.

Long-term memory means: durable facts that stay true across conversations and that nobody should \
have to be told twice. It is loaded into the assistant's context at the start of every future \
session.

Extract ONLY these categories:
- User preferences and working style: stated language, tone, tools, habits.
- Project conventions and rules: how this project is built, tested, laid out, or named.
- Environment and toolchain facts: which tools or SDKs are installed, exact versions, and paths \
that matter later.
- Background and decisions for long-running work: what was decided, and the reason behind it.

Do NOT extract:
- One-off details of the task just finished, or a play-by-play of what was done.
- Anything that can be read back from the repository or files at any time (existing code, file \
contents) — it does not need to be remembered.
- Temporary state, progress, or to-do items.
- Anything already present in the memory shown below.
- Chit-chat, and anything you are merely guessing. A wrong memory is worse than a missing one, \
because it silently misleads every future session.

NEVER write credentials. If the conversation contains any API key, token, password, private key, \
connection string, or similar secret, do NOT write it to memory under any circumstance, and not \
even partially. Leave it out entirely.

Rules:
- Judge first, act second. Most conversations contain NOTHING worth remembering — that is the \
normal outcome.
- If nothing qualifies, reply with exactly `NOTHING_TO_REMEMBER` and nothing else. Do not call \
any tool in that case, and do not explain or summarise why — that reply is a counter line, not \
a report (2026-10-27, §待办 3-16 P0-3).
- Otherwise save at most a few facts, one per `Remember` call, each a concise self-contained \
sentence in the user's own language. No paths or IDs that will be meaningless later.
- The memory shown to you may be split into pages (headings like `## build`). When a fact belongs \
with an existing page, pass that page's name as `page` so related facts stay together; otherwise \
omit `page`.
- If a new fact contradicts or supersedes something already in the memory, do NOT keep both: \
rewrite that part with ONE `Remember` call using `replace: true` (same `page` when the old fact \
lives in a page), so exactly one of the two claims survives. Two contradictory bullets are worse \
than one slightly stale bullet.
- You may use Read / Glob / Grep to verify a fact before saving it. You may fix one of the \
user's skills under the skills directory with Edit / Write, but ONLY when the user's own words \
in the conversation clearly established that it is wrong or should be improved; never \
speculatively.
- When you did save something, reply with ONE short line naming what you saved. Nobody reads \
this reply except the log — never paste the conversation back into it.";

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
// 下面这些**不**走这里，各有专门分支：
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
// 另对「上下文超限」这类 400 做一次强制压缩后重试，兜住估算误差。
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
/// 没有这道闸，每轮都会有一两条旧消息跨过保留尾部被瘦身 → 前缀每轮都变、
const COMPACT_MIN_GROWTH: f64 = 0.15;
/// 压缩时始终保留最近的消息条数。*下限**，见 `compact_keep_tail`。
const COMPACT_KEEP_TAIL: usize = 8;
/// 尾部保留。**token 预算** 占上下文预算的比例（2026-11-15）。
///
/// 固定条数的问题：它不认识「一条消息有多大」—。保留 8 条可能只。200 token，也可能。
/// tool_result 内容超过该字符数才算「值得瘦身的大块」
/// tool_result 内容超过该字符数才算「值得瘦身的大块」
/// 改成按预算从新到旧累加：越靠后的消息越可能正在用，先保它们；体量到顶就停手。
/// **下限仍是 `COMPACT_KEEP_TAIL` 。*（短消息场景与旧行为逐字节一致），上限也只有。
/// 。这个改动只会让尾。*变短**，不会变长。
const COMPACT_TAIL_TOKENS_RATIO: f64 = 0.25;
/// tool_result 内容超过该字符数才算「值得瘦身的大块」
const COMPACT_KEEP_MIN: usize = 2;
/// tool_result 内容超过该字符数才算「值得瘦身的大块」
const ELIDE_TOOL_RESULT_CHARS: usize = 2_000;

// ── append-only 全量（P6 全量形态，2026-10-24）────────────────────────
//
// 目标（`docs/todo.md` §待办 3-6 补 P6）：0.95 水位上**不删消息** —— 先把**全部**可瘦块
// （含尾部）换成指针，折算后仍可能撞预算才回落到今天的 `Drop`。价值 = 「不丢消息」：
// 被瘦的块可重跑取回，且同时写进结论索引（`conclusions::extract_from_elided`）；
// 而 `Drop` 丢掉的消息只剩摘要。代价 = 尾部也被动 ⇒ 当前任务正用的工具结果要重读一次。
//
// **默认关**（`LUNAC_COMPACT_APPEND_ONLY`）：它改变 0.95 水位的行为，而生产 128k 下
// 「Drop 真实触发频率」**至今没有实测数据**（todo 里记的前置动作）。关着时本档不可达 ⇒
// 行为与改造前逐字节一致。

/// 开关：`1/true/on/yes`（口径同 `LUNAC_DUMP_API` / `LUNAC_LOCAL_DECIDE`）。进程内只读一次。
const APPEND_ONLY_ENV: &str = "LUNAC_COMPACT_APPEND_ONLY";

/// 折算用的**保守**比例：**4 字符 ≈ 1 token**。
///
/// 刻意取大（= 少算省下的 token）：估少了只会**更早**回落到 `Drop`（安全）；估多了才会
/// 「以为够、结果下一请求撞 400」。中文实际约 1 字符/token，这里留了 4 倍余量。
const APPEND_ONLY_CHARS_PER_TOKEN: usize = 4;

fn compact_append_only_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| log::is_truthy(&std::env::var(APPEND_ONLY_ENV).unwrap_or_default()))
}

/// append-only 的**回落判据**：保守折算后仍会顶到预算 ⇒ 必须丢整条（否则下一请求直接 400）。
///
/// `measured_tokens == 0`（体积未知）⇒ 同样回落：宁可少省一次，也不拿 400 赌。
fn append_only_must_drop(measured_tokens: u64, elided_chars: usize) -> bool {
    if measured_tokens == 0 {
        return true;
    }
    let freed = (elided_chars / APPEND_ONLY_CHARS_PER_TOKEN) as u64;
    measured_tokens.saturating_sub(freed) >= max_context_tokens()
}

// ── 语义分层压缩（S0「压缩语义化」，2026-10-14）────────────────────
//
// 现有压缩是**体积驱动**的（水位 / 尾部条数），不看「这段内容是什么」。本轮把它升级为
// **语义分层**：给每条消息判一个层（`classify_message_layer`，**确定性纯函数**、不调模型、
// 不动固定前缀），压缩时按层决策「丢谁」。原始口径见
// `docs/archive/agent-feature-backlog.md:1278`：「工作记忆保细节 / 情景记忆压成结论 / 语义记忆入长期」。
//
// 六层（L1–L6）：
//   · L1 用户诉求 / 任务目标            —— 保原文（历史 head 那条用户文本永不丢；中段用户文本
//                                          由摘要 / 结论索引保原文兜住）
//   · L2 已达成的结论与决策              —— 结构化摘要 + 入结论索引
//   · L3 未决问题 / 待办                 —— 摘要 + 任务快照钉回（`TodoWrite` 快照）
//   · L4 失败证据（`is_error` 的失败尝试）—— 永不因体积丢弃（保留 or 进摘要 / 索引）
//   · L5 可重取的原始材料                —— 只留指针（可 elide；重跑即得）
//   · L6 纯过程性噪声                    —— 优先丢弃
//
// ⚠️ 判层必须是**确定性纯函数**：禁止让模型判层 —— 那会新增一次调用 + 动固定前缀
//    （见 `forbidden.md` §禁用 45）。
//
// `LAYER_DROP_ORDER` = 丢层优先序（靠前的先丢）。**L1 / L4 不在表里** ⇒ 它们不可被丢：
//   · L1 = `head` 那条用户文本（丢弃逻辑始终保留它）+ 中段用户文本（进摘要 / 索引）；
//   · L4 = 失败证据，`Drop` 档把边界钉在它之前（保留在 history），`Force` 档若被丢则必进
//     摘要 / 索引（`conclusions::extract_from_dropped` 有专门的 `status=failed` 条目）。
const LAYER_DROP_ORDER: [Layer; 4] = [Layer::L6, Layer::L5, Layer::L3, Layer::L2];

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
//   。改写前缀 。缓存整段作废 。触发频次要尽量低。「宁可压得晚也不要压得勤」：
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
///
/// 语义分层对应（S0「压缩语义化」，2026-10-14）：本提示词的四段**已天然对应** L1–L3 ——
///   · `## Historical Task Snapshot` → **L1**（用户诉求 / 任务目标，保原文）；
///   · `## What Was Done` / `## Decisions And Constraints` → **L2**（已达成的结论与决策）；
///   · `## Pending` → **L3**（未决问题 / 待办）。
/// 故本轮**不改提示词内容、不改调用次数**（它本就是压缩时那**唯一**一次模型调用）。
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

/// 结论索引**检索 / 注入**的上限。
///
/// 用户裁决：**不加上下文预算**（Chroma《Context Rot》：输入越长性能越不稳）—— 这里只是给
/// 「一次注入」一个硬上限（条数 + 总字符），**与 `MAX_CONTEXT` / 预算常量无关**，不改任何水位。
const CONCLUSIONS_MAX_ITEMS: usize = 5;
const CONCLUSIONS_INJECT_CHARS: usize = 1_200;
/// 指针块（`[elided pointers]`）的注入上限（2026-10-27，§待办 3-16 P1-7）。
///
/// 与结论索引同档：它是**路标**不是内容 —— 一行一次调用，1.2k 字符够放二三十条。
/// 给得再宽只会把「重查的线索」变成第二份上下文负担。
const PTRBLOCK_MAX_CHARS: usize = 1_200;

/// 追加到摘要提示词末尾的「结构化结论段」指令。
///
/// **零新增调用**：它复用压缩摘要那**一次**模型调用（`summarize_dropped`），只要求模型在摘要
/// 之后多吐一个机器可读段；本函数与 `conclusions::INDEX_*` 共用同一对标记常量（单一真相源，
/// 见 `forbidden.md` §禁用 62）。这段只出现在摘要请求自己的 system 上，**不碰主对话固定前缀**。
fn summary_index_instruction() -> String {
    format!(
        "\n\nFinally, append a machine-readable list of the conclusions worth recovering later.\n\
         Output it EXACTLY between these two marker lines, as a JSON array and nothing else between them:\n\n\
         {begin}\n\
         [{{\"status\":\"verified|failed|rejected\",\"conclusion\":\"one sentence\",\"keywords\":[\"exact\",\"identifiers\"],\"refs\":[{{\"kind\":\"file|url|query|commit|command\",\"value\":\"...\"}}],\"reason\":\"only when status is failed\"}}]\n\
         {end}\n\n\
         Rules for this block:\n\
         - Prefer exact identifiers in keywords: file paths, function names, URLs, error codes, commands.\n\
         - Add a ref for anything that can be re-opened or re-run.\n\
         - Record every FAILED attempt with status \"failed\" and a short reason. Never omit a failure \
         -- it is what tells the assistant what NOT to repeat.\n\
         - Use \"rejected\" for a conclusion later evidence disproved; keep it, do not drop it.\n\
         - If nothing qualifies, output an empty array [].\n\
         - Do not wrap the block in code fences.",
        begin = conclusions::INDEX_BEGIN,
        end = conclusions::INDEX_END,
    )
}

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
/// 整段比对会因为命中条数不同而永远不相等 ⇒ 抓不到「换了关键词但一无所获」这个形态。
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

/// 工具连续失败计数的推进（纯函数，便于单测）。
///
/// `round` = 本轮「单工具且失败」的工具名；任何其他情况（成功、并行、无工具）传 `None`
/// 。计数清零。返。`None` 表示清零，`Some((name, n))` 表示该工具已连续失败 `n` 轮。
fn next_fail_streak(current: Option<(String, usize)>, round: Option<&str>) -> Option<(String, usize)> {
    let name = round?;
    Some(match current {
        Some((last, count)) if last == name => (last, count + 1),
        _ => (name.to_string(), 1),
    })
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

/// 任务清单里是否还有**未完成项**（`pending` / `in_progress`）。
///
/// 判据就是任务快照的渲染格式 `N. [status] content`（见 `latest_todo_snapshot`）——
/// 不去解析 `TodoWrite` 的原始 JSON，因为快照是压缩后**唯一幸存**的那份（两者同源）。
/// **没有清单 ⇒ false**（保守：没计划就不放宽预算闸门）。
///
/// 用途见 [`budget_extension_allowed`]：预算撞顶时，只有「计划未完成」才允许继续续命
/// （且此时不数次数，只要求上下文还能安全继续）。
fn todo_has_unfinished(history: &[Value]) -> bool {
    match latest_todo_snapshot(history) {
        Some(s) => s
            .lines()
            .any(|l| l.contains("[pending]") || l.contains("[in_progress]")),
        None => false,
    }
}

/// 判断 400 是否由「上下文超限」引起 —— 只有这类才值得压缩后重试
///
/// 2026-11-14：长任务偏离目标的根因之一就是 `Compact::Force` 把中段用户修正一起丢。
/// （`temp\logs\agent-2026-11-10.log` 一次丢 101。84 条），模型随后不知道「先做完资产
/// 判断 400 是否由「上下文超限」引起 —— 只有这类才值得压缩后重试
const USER_CONTRACT_HEADER: &str = "[Lunac harness] 用户在本会话里明确提出过的要求（原文摘录，\
按时间顺序）。回答与动手之前先对照这一段；不要因为中段对话被压缩就偏离它。";

/// 收集一批消息里**用户亲口提的要求**：`role=user` 。text 块。
///
/// 排除：工具回灌（`tool_result`）、压缩占位、以。harness 自己注入的块。
/// 判断 400 是否由「上下文超限」引起 —— 只有这类才值得压缩后重试
fn user_contract_lines(history: &[Value]) -> Vec<String> {
    fn push_line(out: &mut Vec<String>, line: &str) {
        let line = line.trim();
        if line.is_empty() || line == TRIMMED_MARKER || line.starts_with("[Lunac harness]") {
            return;
        }
        if !out.iter().any(|x| x == line) {
            out.push(line.to_string());
        }
    }
    let mut out: Vec<String> = Vec::new();
    for msg in history {
        if msg.get("role").and_then(Value::as_str) != Some("user") || is_tool_result_msg(msg) {
            continue;
        }
        match msg.get("content") {
            Some(Value::String(text)) => push_line(&mut out, text),
            Some(Value::Array(blocks)) => {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) != Some("text") {
                        continue;
                    }
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        push_line(&mut out, text);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// 把用户要求渲染成可钉回历史的纯文本；没有要求 / 全部超长 。`None`。
///
/// 判断 400 是否由「上下文超限」引起 —— 只有这类才值得压缩后重试
fn user_contract_snapshot(history: &[Value]) -> Option<String> {
    let mut out = String::from(USER_CONTRACT_HEADER);
    let mut total = 0usize;
    for line in user_contract_lines(history) {
        let one = line.replace('\n', " ");
        let clipped: String = one.chars().take(400).collect();
        let cost = clipped.chars().count();
        if total + cost > 6000 {
            break;
        }
        total += cost;
        out.push_str("\n- ");
        out.push_str(&clipped);
    }
    if total == 0 {
        None
    } else {
        Some(out)
    }
}

/// 本轮契约锚点。026-11-15，用户报障「多次纠正后仍回不到目标」）。
///
/// 判断 400 是否由「上下文超限」引起 —— 只有这类才值得压缩后重试
/// cache-miss；用户消息本来就是新的，多一个短文本块只影响本轮尾部。
/// 判断 400 是否由「上下文超限」引起 —— 只有这类才值得压缩后重试
const GOAL_ANCHOR_HEADER: &str = "[this turn's contract]";
const GOAL_ANCHOR_CHARS: usize = 400;

/// 判断 400 是否由「上下文超限」引起 —— 只有这类才值得压缩后重试
fn narrows_scope(text: &str) -> bool {
    const MARKERS: [&str; 12] = [
        "只做", "只处理", "只要", "只改", "不要了", "不要", "别再", "不用", "别", "而不是",
        "only", "don't",
    ];
    let lower = text.to_ascii_lowercase();
    MARKERS
        .iter()
        .any(|marker| text.contains(marker) || lower.contains(marker))
}

/// 生成锚点文本；空消息 / 纯空。`None`（不凭空加块）。
fn goal_anchor(prompt: &str) -> Option<String> {
    let text = prompt.trim();
    if text.is_empty() {
        return None;
    }
    let clipped: String = text.chars().take(GOAL_ANCHOR_CHARS).collect();
    let mut out = format!(
        "{GOAL_ANCHOR_HEADER}\nThe user's newest message (authoritative for this turn; \
it overrides earlier plans, todos and your own printed judgements):\n{clipped}"
    );
    if narrows_scope(text) {
        out.push_str(
            "\nThis message narrows or reverses the scope: do ONLY what it names. \
Stop and say so before touching anything outside it.",
        );
    }
    Some(out)
}

/// 判断 400 是否由「上下文超限」引起 —— 只有这类才值得压缩后重试
fn is_user_contract_msg(msg: &Value) -> bool {
    msg.get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks.iter().any(|block| {
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|text| text.starts_with(USER_CONTRACT_HEADER))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
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
    /// **append-only 全量**（2026-10-24，P6 全量形态；**默认关**，见 [`compact_append_only_enabled`]）。
    ///
    /// 0.95 水位上**不删消息**：先把**全部**可重现大块（连尾部一起，只留最近 2 条）换成指针，
    /// 再按 [`append_only_must_drop`] 折算 —— 折算后仍可能撞预算才回落到今天的 `Drop`。
    /// 与 `Force` 的区别：`Force` 是「已经撞了 400，必须压下去」，本档是「尽量不丢」。
    AppendOnly,
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
    /// 被**瘦身**（elide）的可重现工具调用（合成 tool_use 消息：工具名 + 入参）。
    ///
    /// 带出来供结论索引写「查过哪里」的指针条目（见 `conclusions::extract_from_elided`）——
    /// 本函数同样**不写盘、不调模型**，保持纯的。
    elided_msgs: Vec<Value>,
    /// 压缩过程中**插回**历史的合成消息条数（`TRIMMED_MARKER` / 任务快照）。
    ///
    /// 调用方必须拿它修正回滚锚点 `base`。**这是一个 2026-09-18 一并修掉的既有 off-by-one**：
    /// `base` 是「本轮第一条消息的下标」，而 `finish_error` 用 `history.truncate(base)` 回滚。
    /// 压缩把 `head..cut` 抽走（base 要减 dropped），但插回来的合成消息位于**下标 0/1**，
    /// 也在 base 之前 ⇒ base 必须**加**回来。原实现只做了减法，于是「丢弃 + 钉任务快照」
    /// 那一轮一旦出错，回滚会**多切掉一条真实历史**（§8.2 又加了摘要插入，会多切两条）。
    pinned: usize,
    /// 这次压缩一。*省下多少字符**（瘦身的原文字数 + 被丢消息的正文）。
    ///
    /// 只用于压缩后。`last_input` 估一个下界（。`compaction_estimate`）：没有它就只能
    /// 。0，。0 会被 `should_budget_compact` 当成「不宽敞」，导致刚压完又压一次。
    freed_chars: usize,
}

/// 瘦身档的成本模型算出来的一个方案（`picked` 为空 = 判定「不值得压」）。
///
/// `sigma` / `delta` / `net` 是**判据本身用的那几个数**，一并带出来只为日志 ——
/// 事后复盘时不必再猜「当时为什么压了 / 为什么没压」。
#[derive(Clone, PartialEq, Eq, Debug)]
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

/// 消息的语义分层（S0「压缩语义化」，2026-10-14）。判层是**确定性纯函数**，不调模型、不动前缀。
///
/// 层的含义见 `LAYER_DROP_ORDER` 上方的注释；这里的判据（自上而下，命中即返回）：
///   · `is_error` 的 `tool_result` → L4（失败证据，永不因体积丢）；
///   · 可重取工具（`elidable_tool`）的结果 → L5（重跑即得，只留指针）；
///   · 其余 `tool_result`（`Agent` / `mcp__*` 等不可重取批量结果）→ L6；
///   · 含 `TodoWrite` 的 assistant → L3（未决待办）；
///   · 含其它 `tool_use` 的 assistant → L2（工作结论 / 决策）；
///   · 用户非合成文本 → L1（诉求 / 任务目标，保原文）；
///   · 其余（assistant 纯文本 / 合成消息 / 无法归类 / 空消息）→ L6（纯过程性噪声）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Layer {
    L1,
    L2,
    L3,
    L4,
    L5,
    L6,
}

/// 判一条消息属于哪一层。
///
/// `tool_name` = 该消息相关工具的真实名字（`tool_result` 由 `tool_use_id` 反查；assistant 的
/// `tool_use` 就是它自己；认不出给 `None`）。**纯函数**：同一输入恒同一输出，可单测钉住。
fn classify_message_layer(msg: &Value, tool_name: Option<&str>) -> Layer {
    let role = msg.get("role").and_then(Value::as_str).unwrap_or("");
    let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
        // 没有块结构：用户消息按诉求（L1）兜底，其余按噪声（L6）。
        return if role == "user" { Layer::L1 } else { Layer::L6 };
    };
    // ① tool_result 主导：失败证据 > 可重取材料 > 不可重取批量结果。
    let mut has_result = false;
    for b in blocks {
        if b.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        has_result = true;
        if b.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
            return Layer::L4;
        }
    }
    if has_result {
        return if elidable_tool(tool_name) { Layer::L5 } else { Layer::L6 };
    }
    // ② assistant 的工具调用：TodoWrite = 未决待办（L3）、TaskPhase = 阶段结论（L3），
    //    其余 = 工作结论 / 决策（L2）。
    let mut has_tool_use = false;
    for b in blocks {
        if b.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        has_tool_use = true;
        // TodoWrite = 未决待办；TaskPhase = 阶段标题 / 小总结（长跑里最容易被压缩掉的
        // 「当前做到哪、上一步结论是什么」）。两者都按 L3 保住。
        if matches!(
            b.get("name").and_then(Value::as_str),
            Some("TodoWrite") | Some("TaskPhase")
        ) {
            return Layer::L3;
        }
    }
    if has_tool_use {
        return Layer::L2;
    }
    // ③ 用户非合成文本 = 诉求 / 任务目标（保原文）。合成消息（以 `[` / `#` 开头的上下文块 /
    //    摘要头 / 任务快照 / 结论注入块）不算「用户的话」。
    if role == "user" {
        for b in blocks {
            if b.get("type").and_then(Value::as_str) != Some("text") {
                continue;
            }
            let t = b.get("text").and_then(Value::as_str).unwrap_or("").trim();
            if !t.is_empty() && !t.starts_with('[') && !t.starts_with('#') {
                return Layer::L1;
            }
        }
    }
    // ④ 其余（assistant 纯文本 / 合成消息 / 无法归类）→ L6（纯过程性噪声）。
    Layer::L6
}

/// 解析一条消息的工具名后判层：`tool_result` 的 `tool_use_id` 到工具名的映射由调用方传入
/// （`compact_history` 已经建好一份，复用即可，不另扫一遍）。
fn message_layer(msg: &Value, tool_names: &std::collections::HashMap<String, String>) -> Layer {
    let tool_name = msg
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks.iter().find_map(|b| {
                if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                    b.get("tool_use_id").and_then(Value::as_str)
                } else {
                    None
                }
            })
        })
        .and_then(|id| tool_names.get(id))
        .map(String::as_str);
    classify_message_layer(msg, tool_name)
}

/// 层感知的丢弃边界：返回收缩后的 `cut`，使得被丢区间 `[head, cut)` **不含**保留层
/// （不在 `LAYER_DROP_ORDER` 里的 L1 / L4）。
///
/// 三条纪律（对应用户裁决的 Drop 硬约束）：
///   · **保连续**：只收缩边界（`cut` 左移），被丢的仍是**连续一段** `[head, cut)` —— 不做
///     非连续抽取（约束②：不把 drain 打散成不连续区间）；
///   · **保配对**：保留侧不能以 `tool_result` 开头 —— 若 `cut` 落在 `tool_result` 上，回退到它
///     配对的 `tool_use`（`tool_use` 恒紧邻其 `tool_result` 之前）一起保留（约束①）；
///   · **保 L1/L4**：命中保留层即把边界钉在它之前，于是该层消息与其后的内容全部保留（约束③）。
///
/// 找不到保留层 ⇒ 原样返回 `natural_cut`（等于不改边界）。
fn layer_aware_drop_cut(
    history: &[Value],
    tool_names: &std::collections::HashMap<String, String>,
    head: usize,
    natural_cut: usize,
) -> usize {
    let mut cut = natural_cut;
    for i in head..natural_cut {
        if !LAYER_DROP_ORDER.contains(&message_layer(&history[i], tool_names)) {
            cut = i;
            break;
        }
    }
    if cut == natural_cut {
        return cut;
    }
    // 保配对：保留侧不能以 tool_result 开头 ⇒ 连同其 tool_use 一起保留。
    while cut > head && is_tool_result_msg(&history[cut]) {
        cut -= 1;
    }
    // 兜底：万一仍以 tool_result 开头（理论上不会），让到下一个安全边界。
    while cut < natural_cut && is_tool_result_msg(&history[cut]) {
        cut += 1;
    }
    cut
}

// ── Drop 档确定性护栏（P6 小补丁，2026-10-24）────────────────────────────
//
// `Drop`（0.95 水位）是「不压就可能撞 400」的安全刚需，**不能**像 `Elide` 那样「命中护栏就
// 一条都不压」（压缩收益归零 ⇒ 照样撞上限）。这里做的是**让路式**护栏：
//   · 受保护 = `curation::protected_reason` 命中（具名事实行 / 口令行 / 叙述型约束）；
//   · 先把丢弃边界收缩。*第一。*受保护消息之。—。连续段约束下「跳过一条消息」的唯一
//     形态就是把右边界钉在它之前（`[head, cut)` 仍是一段连续区间，见 `layer_aware_drop_cut`
//     的约束②；非连续抽取属于「append-only 全量」那一档，不在本例）；
//   · **只有受保护消息就在 `head`**（前面没有任何可丢的纯噪声，收缩等于不丢）时才让路，
//     记一行「护栏被迫让路」并按今天的口径丢干净 —— 丢零条只会白废前缀缓存。
// 最坏情况与不接线逐字节一致；真正的兜底仍是无条件 `Compact::Force`（预算 / 400），
// 它不经过本函数，被丢的事实照样进摘要 / 结论索引（`restore_after_compaction`）。

/// 一条消息的**正文**（`text` 块与 `tool_result` 的 `content`）。
/// 只看正文、不看 JSON 键名 —— 理由同 [`recent_tail_text`]（键名里的字符会假命中）。
fn message_text(msg: &Value) -> String {
    let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
        return msg
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
    };
    let mut out = String::new();
    for b in blocks {
        for key in ["text", "content"] {
            if let Some(s) = b.get(key).and_then(Value::as_str) {
                out.push_str(s);
                out.push('\n');
            }
        }
    }
    out
}

/// Drop 档护栏：把丢弃边界收缩到第一条受保护消息之前。返回新的 `cut`。
///
/// 受保护消息就在 `head`（前面没有任何可丢的纯噪声）⇒ **原样返回** `natural_cut`，
/// 行为与不接线时逐字节一致。
fn guarded_drop_cut(history: &[Value], head: usize, natural_cut: usize) -> usize {
    let end = natural_cut.min(history.len());
    let mut wall: Option<(usize, &'static str)> = None;
    let mut protected = 0usize;
    for i in head..end {
        if let Some(reason) = curation::protected_reason(&message_text(&history[i])) {
            protected += 1;
            if wall.is_none() {
                wall = Some((i, reason));
            }
        }
    }
    let Some((mut wall_at, reason)) = wall else {
        return natural_cut;
    };
    // 保配对：边界不能停在 `tool_result` 上（它必须紧跟对应的 `tool_use`）—— 与
    // `layer_aware_drop_cut` 同一条纪律，宁可多留一条 `tool_use`。
    while wall_at > head && is_tool_result_msg(&history[wall_at]) {
        wall_at -= 1;
    }
    if wall_at <= head {
        // 收缩后一条也丢不掉 ⇒ 让路：Drop 的本职是清出空间，丢零条只会白废前缀缓存。
        log::info(format!(
            "Drop 档护栏被迫让路：受保护 {protected} 条（{reason}）就在可丢区间开头，\
             仍按原边界丢 {} 条",
            end.saturating_sub(head)
        ));
        return natural_cut;
    }
    log::info(format!(
        "Drop 档护栏：保护 {protected} 条（{reason}），丢弃边界 {natural_cut} → {wall_at}\
         （{} 条 → {} 条）",
        end.saturating_sub(head),
        wall_at - head
    ));
    wall_at
}

// ── C 臂：决策驱动瘦身（2026-10-24）────────────────────────────────────
//
// 只在 `Compact::Elide` 分支调用（见 `compact_history` 里 `plan` 的构造处）。
// **不碰 `Drop` / `ElideCold` / `Force`** —— 那三档是「不压就可能撞上限」的安全刚需。

/// **确定性护栏**（2026-10-24，不依赖本地模型）：把「具名事实行 / 口令行」整条从瘦身名单里拿掉。
///
/// 理由：真机实测 C 臂在 T5 上 0/3，根因是 4B 把「90 行同质流水里夹一行 `code word: …`」判成
/// 噪声（带 `Read` 行号前缀时尤其明显）。这类判断**不该交给小模型** —— 命中即保留，
/// 宁多花 token，不丢事实。护栏先于 C 臂执行（C 臂只看剩下的候选）。
///
/// 只作用于 `Compact::Elide`（可选档）与 `Compact::AppendOnly`：`ElideCold` / `Drop` / `Force`
/// 是「不压就可能撞 400」的安全刚需，**必须**保持无条件瘦身。护栏改变省下的体积 ⇒ 重算
/// `sigma` / `net`（`net` 只在**有成本模型**的 `Elide` 档有意义，见 [`GuardMode`]）。
fn guard_elision_plan(plan: ElidePlan, history: &[Value]) -> ElidePlan {
    guard_elision_plan_impl(plan, history, GuardMode::Elide)
}

/// 护栏在哪一档生效 —— 两档的**保护范围**不同，不是同一个开关的两种写法：
#[derive(Clone, Copy, PartialEq, Eq)]
enum GuardMode {
    /// `Elide`（0.85 可选档）：事实行 + **最近引用过的文件**都保护。这一档有成本模型，
    /// 少瘦几条只影响「省多少」，所以宁多保护（文件正在被讨论 ⇒ 别动它的内容）。
    Elide,
    /// `AppendOnly`（0.95 全量档）：**只保护事实行**（具名事实 / 口令 / 叙述型约束）。
    ///
    /// 为什么不保护「最近引用过的文件」：这一档**不丢消息**、只把可重现的结果换成指针，
    /// 而尾部瘦身是它唯一的腾空间手段 —— 而尾部块的文件路径必然出现在紧邻的 `tool_use` 里，
    /// 若照样保护，尾部块永远瘦不掉、append-only 就退化成「一律回落 Drop」，开关等于白开。
    /// 为什么不保护「最近引用过的文件」：这一档**不丢消息**、只把可重现的结果换成指针，
    /// 另外这一档**没有成本模型**（`delta` / `net` 恒 0），故不重算 / 不按 `net ≤ 0` 清空。
    AppendOnly,
}

/// 护栏的唯一实现。
fn guard_elision_plan_impl(mut plan: ElidePlan, history: &[Value], mode: GuardMode) -> ElidePlan {
    if plan.picked.is_empty() {
        return plan;
    }
    let before = plan.picked.len();
    let recent = recent_tail_text(history);
    let mut kept = Vec::with_capacity(before);
    let mut protected = 0usize;
    let mut reasons: Vec<&'static str> = Vec::new();
    for (mi, bi, n) in plan.picked.iter().copied() {
        let reason = curation::protected_reason(&tool_result_text(history, mi, bi)).or_else(|| {
            if mode != GuardMode::Elide {
                return None;
            }
            let path = candidate_path(history, mi, bi)?;
            path_referenced_in(&recent, &path).then_some("recently-referenced")
        });
        match reason {
            Some(r) => {
                protected += 1;
                if !reasons.contains(&r) {
                    reasons.push(r);
                }
            }
            None => kept.push((mi, bi, n)),
        }
    }
    if protected == 0 {
        return plan;
    }
    plan.picked = kept;
    plan.sigma = plan.picked.iter().map(|(_, _, n)| *n).sum();
    if mode == GuardMode::Elide {
        plan.net = plan.sigma as i128 * ELIDE_PAYBACK_REQUESTS as i128
            - plan.delta as i128 * CACHE_HIT_DISCOUNT as i128;
        if plan.net <= 0 {
            plan.picked.clear();
        }
    }
    log::info(format!(
        "确定性护栏：保护 {protected} 条（{}），瘦身名单 {before} → {} 条",
        reasons.join("/"),
        plan.picked.len()
    ));
    plan
}

/// 取一条候选背后那次工具调用的入参（`tool_result.tool_use_id` → 前面的 `tool_use`）。
fn tool_use_input_for<'a>(history: &'a [Value], mi: usize, bi: usize) -> Option<&'a Value> {
    let id = history
        .get(mi)?
        .get("content")?
        .as_array()?
        .get(bi)?
        .get("tool_use_id")?
        .as_str()?;
    for m in history[..mi].iter().rev() {
        let Some(blocks) = m.get("content").and_then(Value::as_array) else {
            continue;
        };
        for b in blocks {
            if b.get("type").and_then(Value::as_str) == Some("tool_use")
                && b.get("id").and_then(Value::as_str) == Some(id)
            {
                return b.get("input");
            }
        }
    }
    None
}

/// 末尾 `COMPACT_KEEP_TAIL` 条消息的**文本**（只取正文与路径，不取 JSON 键名）——
/// 用来判「这个文件还在被讨论」。⚠️ 不能直接 `to_string(history)`：JSON 键名里就有 `x`、
/// `text` 等字符，会让单字符路径（测试里的 `"x"`）假命中。
fn recent_tail_text(history: &[Value]) -> String {
    let start = history.len().saturating_sub(COMPACT_KEEP_TAIL);
    let mut out = String::new();
    for m in &history[start..] {
        let Some(blocks) = m.get("content").and_then(Value::as_array) else {
            continue;
        };
        for b in blocks {
            for key in ["text", "content"] {
                if let Some(s) = b.get(key).and_then(Value::as_str) {
                    out.push_str(s);
                    out.push('\n');
                }
            }
            if let Some(p) = b.get("input").and_then(|i| i.get("file_path")).and_then(Value::as_str)
            {
                out.push_str(p);
                out.push('\n');
            }
        }
    }
    out
}

/// 候选对应的文件路径（`Read` / `Write` / `Edit` 的 `file_path`）。
fn candidate_path(history: &[Value], mi: usize, bi: usize) -> Option<String> {
    let input = tool_use_input_for(history, mi, bi)?;
    let p = input
        .get("file_path")
        .or_else(|| input.get("path"))?
        .as_str()?;
    if p.is_empty() {
        None
    } else {
        Some(p.to_string())
    }
}

/// 路径（或其文件名）是否出现在最近一轮文本里。**宁多保护**：文件名命中即算引用
/// （避免「用户说 00-notes.md、记录里是 notes/00-notes.md」这种前缀不一致）。
fn path_referenced_in(recent: &str, path: &str) -> bool {
    if path.chars().count() >= 4 && recent.contains(path) {
        return true;
    }
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    base.chars().count() >= 4 && recent.contains(base)
}

/// 生产入口：开关关 / 端点未配 / 无可判候选 ⇒ **原样返回**（与不接线逐字节一致）。
fn curate_elision_plan(plan: ElidePlan, history: &[Value]) -> ElidePlan {
    if plan.picked.is_empty() || !curation::enabled() {
        return plan;
    }
    let Some(cfg) = decide::LocalHttpConfig::from_env() else {
        return plan;
    };
    let judged = plan.picked.len().min(curation::MAX_ITEMS);
    let items: Vec<String> = plan.picked[..judged]
        .iter()
        .map(|(mi, bi, _)| {
            curation::sample_item(&tool_result_text(history, *mi, *bi), curation::ITEM_CHARS)
        })
        .collect();
    let cur = curation::curate(&curation::LocalCurator::new(cfg), &items);
    apply_curation(plan, &cur, judged)
}

/// 取一条候选的文本（`tool_result` 的 `content` 字符串；取不到 ⇒ 空串）。
fn tool_result_text(history: &[Value], mi: usize, bi: usize) -> String {
    history
        .get(mi)
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .and_then(|a| a.get(bi))
        .and_then(|b| b.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// **纯函数**：把策展结果落到瘦身计划上（可单测，不碰网络）。
///
/// `judged` = 真正送判的候选条数（前 N 条）；其后的候选**不做判定、照旧**。
///   · `fallback`（故障 / 通道忙）⇒ **原样返回**（体积驱动说了算）；
///   · 模型说「都可丢」⇒ 名单不变；
///   · 模型说「部分保留」⇒ 只把那些从名单里拿掉，并**重算** `sigma` / `net`（日志口径不飘）。
fn apply_curation(mut plan: ElidePlan, cur: &curation::Curation, judged: usize) -> ElidePlan {
    if cur.fallback || judged == 0 {
        return plan;
    }
    let mut picked = curation::elision_candidates_after_curation(&plan.picked[..judged], cur);
    picked.extend_from_slice(&plan.picked[judged..]);
    let before = plan.picked.len();
    plan.sigma = picked.iter().map(|(_, _, n)| *n).sum();
    plan.net = plan.sigma as i128 * ELIDE_PAYBACK_REQUESTS as i128
        - plan.delta as i128 * CACHE_HIT_DISCOUNT as i128;
    plan.picked = picked;
    // ⚠️ C 臂改变了「省下的体积」⇒ **必须重跑成本判据**：过滤后若已「省小废大」
    // （`net <= 0`），就一条都不瘦 —— 否则会出现「日志写着 net 为负、却照样动手」
    // （2026-10-24 真机：瘦身 5→4 条后 net=-194800 仍压了 4 条）。
    if plan.net <= 0 {
        plan.picked.clear();
    }
    if plan.picked.len() != before {
        log::info(format!(
            "决策策展：瘦身名单 {} → {} 条（模型认为其余仍有用，保留原文）",
            before,
            plan.picked.len()
        ));
    }
    plan
}

/// 一条消息的近似**字符。*（只算模型真读到的部分：文本、`content` 字符串、工具入参）。
///
/// 不数 JSON 键名 / 结构符号：那些每个请求都一样，算进来只会让估值随消息条数漂移。
fn message_chars(msg: &Value) -> usize {
    if let Some(text) = msg.get("content").and_then(Value::as_str) {
        return text.chars().count();
    }
    let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
        return 0;
    };
    let mut total = 0usize;
    for block in blocks {
        for key in ["text", "content"] {
            if let Some(text) = block.get(key).and_then(Value::as_str) {
                total += text.chars().count();
            }
        }
        if let Some(input) = block.get("input") {
            total += input.to_string().chars().count();
        }
    }
    total
}

/// 压缩时尾部保留多少条。*。token 预算从新到旧。*。026-11-15，替代固定条数）。
///
/// 口径。`COMPACT_TAIL_TOKENS_RATIO`：累加到最后一条超出预算就停，但不少于
/// `COMPACT_KEEP_MIN` 条、不多于 `COMPACT_KEEP_TAIL` 条。短消息（以及全部单测里。
/// 构造消息）累加不到预算 。结果恒为 `COMPACT_KEEP_TAIL`，与改造前逐字节一致。
fn compact_keep_tail(history: &[Value]) -> usize {
    if history.is_empty() {
        return 0;
    }
    let budget = (max_context_tokens() as f64 * COMPACT_TAIL_TOKENS_RATIO) as u64;
    let mut chars = 0usize;
    let mut kept = 0usize;
    for msg in history.iter().rev().take(COMPACT_KEEP_TAIL) {
        chars += message_chars(msg);
        kept += 1;
        let tokens = (chars / APPEND_ONLY_CHARS_PER_TOKEN) as u64;
        if kept >= COMPACT_KEEP_MIN && tokens >= budget {
            break;
        }
    }
    kept.max(COMPACT_KEEP_MIN).min(history.len())
}

/// 压缩之后。`last_input` 。*保守估计**。026-11-15，原先是。0）。
///
/// 。0 的语义是「体积未知」，。`should_budget_compact` 把未知当成「不宽敞。
/// 压缩历史。`measured_tokens` = 上一轮实测的上下文体积（0 = 未知），只进日志。
/// 白丢一批消息。这里按「实测。省下的字。/ 4」估一个下界：
/// 省下的字符按 4 字符/token 折算（与 `APPEND_ONLY_CHARS_PER_TOKEN` 同口径）。
/// 。*宁可高估剩余体积**（高估只会更早再压，不会。400）。
fn compaction_estimate(measured: u64, freed_chars: usize) -> u64 {
    let freed = (freed_chars / APPEND_ONLY_CHARS_PER_TOKEN) as u64;
    measured.saturating_sub(freed)
}

/// 压缩历史。`measured_tokens` = 上一轮实测的上下文体积（0 = 未知），只进日志。
fn compact_history(
    history: &mut Vec<Value>,
    mode: Compact,
    measured_tokens: u64,
) -> CompactOutcome {
    let force = mode == Compact::Force;
    let append_only = mode == Compact::AppendOnly;
    // 尾部保留条数：按 token 预算取（。`compact_keep_tail`）。在**改动历史之前**算一。—。
    //    强制模式下连尾部也瘦（只留最近 2 条）—— 否则尾部若塞了多个超大
    let tail_keep = compact_keep_tail(history);
    //    强制模式下连尾部也瘦（只留最近 2 条）—— 否则尾部若塞了多个超大
    //    强制模式下连尾部也瘦（只留最近 2 条）—— 否则尾部若塞了多个超大
    //    tool_result，光靠「丢弃更老的消息」根本压不下来。
    //    append-only 档同样瘦到只剩 2 条：它**不丢消息**，只能靠瘦尾部把体积让出来。
    let elide_keep = if force || append_only { 2 } else { tail_keep };
    let tail_start = history.len().saturating_sub(elide_keep);

    // ── 工具分级索引（2026-10-01）：`tool_use_id` → 工具名 ──────────────
    // `tool_result` 块自己只带 `tool_use_id`，工具名在**前一条 assistant 消息**的
    // `tool_use` 块里 ⇒ 先扫一遍建索引，才谈得上「哪一类结果可以瘦」。
    // 用 owned `String` 而不是 `&str`：下面要拿 `&mut history`，借用活不到那时。
    let mut tool_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    // 同一趟扫描顺手把入参也存下：瘦身时要把「查过的指针」抄给结论索引（见 `elided_msgs`）。
    let mut tool_inputs: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
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
                tool_inputs
                    .insert(id.to_string(), block.get("input").cloned().unwrap_or(Value::Null));
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
            // 语义分层（S0，第 1 步）：elide 候选**只限 L5（可重取的原始材料）**。等价于
            // 「可重现工具的结果」，但多一条硬规矩 —— **失败证据（L4）永不瘦身**：`is_error` 的
            // `tool_result` 即使工具本身可重取（如 `Cmd` 报错），也判 L4 而非 L5（`status=failed`
            // 是模型纠错所需的证据，见 `conclusions.rs` 的 Manus 口径）。
            if classify_message_layer(msg, tool_name) != Layer::L5 {
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
        let plan = worthwhile_elisions(history, &candidates);
        // 确定性护栏（2026-10-24）：具名事实 / 口令行**先**拿掉，不交给本地模型判
        // （C 臂在 T5 上 0/3 的根因就是这个判断交给 4B 做的）。护栏与开关无关、始终生效。
        let plan = guard_elision_plan(plan, history);
        // C 臂（2026-10-24）：在「成本模型已判定值得瘦身」之后插一道本地判定 ——
        // 模型说「这段还能丢」才留在瘦身名单里。开关默认关 ⇒ 原样返回（逐字节一致）。
        curate_elision_plan(plan, history)
    } else if append_only {
        // append-only：**不看**成本模型（这一档是「尽量不丢消息」，压得越多越好），
        // 。*仍过确定性护。* —。事实。/ 口令。/ 叙述型约束照旧不换掉。
        // 没有成本模型 ⇒ `delta` / `net` 恒 0，护栏不能按 `net ≤ 0` 清空名单。
        let sigma: usize = candidates.iter().map(|(_, _, n)| *n).sum();
        let plan = ElidePlan { picked: candidates.clone(), sigma, delta: 0, net: 0 };
        guard_elision_plan_impl(plan, history, GuardMode::AppendOnly)
    } else {
        let sigma: usize = candidates.iter().map(|(_, _, n)| *n).sum();
        ElidePlan { picked: candidates.clone(), sigma, delta: 0, net: 0 }
    };
    let skip_elide = mode == Compact::Elide && plan.picked.is_empty();

    let mut elided = 0usize;
    let mut elided_chars = 0usize;
    // 被瘦身的工具调用（合成 tool_use 消息），供结论索引写指针条目（见 `CompactOutcome`）。
    let mut elided_msgs: Vec<Value> = Vec::new();
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
             —— 判为省小废大（省下的不比废掉的多），一个字都不动。",
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
            // 抄下被瘦身的是哪个工具、带什么入。—。结论索引据此写「查过哪里」的指针条目。
            if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                if let Some(name) = tool_names.get(id) {
                    let input = tool_inputs.get(id).cloned().unwrap_or(Value::Null);
                    elided_msgs.push(json!({"role": "assistant", "content": [
                        {"type": "tool_use", "id": id, "name": name, "input": input}
                    ]}));
                }
            }
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
                 —。缓存已过期，此时动历史零额外代价"
            ));
        }
    }

    // ② 丢弃：强制模式，或「已到丢弃水位却挤不出来」（说明体积在对话本身）。
    //    只瘦身档永不丢弃 —— 在 0.85 水位上丢整条消息会把缓存一次性废掉，
    //    而按 0.95 水位多等一会儿完全来得及。
    //    只瘦身档永不丢弃 —— 在 0.85 水位上丢整条消息会把缓存一次性废掉，
    let can_drop = match mode {
        // `ElideCold` 与 `Elide` 同：**永不丢整条消息**。冷缓存只免掉「动历史」的
        // 那笔缓存代价，并不改变「0.85 水位上丢消息会一次性废掉整段前缀」这个判断。
        Compact::Elide | Compact::ElideCold => false,
        Compact::Drop => elided == 0,
        // append-only：**默认不丢**；只有保守折算后仍会顶到预算才回落（见 `append_only_must_drop`）。
        Compact::AppendOnly => append_only_must_drop(measured_tokens, elided_chars),
        Compact::Force => true,
    };
    let mut dropped = 0usize;
    // 被丢消息的正文总量（只用于 `freed_chars` 。压缩后的体积估计。
    let mut dropped_msgs_chars = 0usize;
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
        let mut cut = history.len().saturating_sub(tail_keep).max(head);
        // 不能以 tool_result 开头（它必须紧跟对应的 tool_use）
        while cut < history.len() && is_tool_result_msg(&history[cut]) {
            cut += 1;
        }
        // 语义分层（S0，第 2 步）：`Drop` 档改「按层丢」—— 把保留层（L1 / L4）钉在保留侧。
        // **只收缩连续段的右边界**（`head..cut` 仍是一段连续区间），不把 drain 打散。
        // `Force`（400 兜底）**不走层感知**：它是「不压就撞上限」的安全刚需，必须能压下去；
        // 其被丢的 L4 仍进摘要 / 索引（`restore_after_compaction`），证据不消失。
        // append-only 的**回落**要走与 `Drop` 完全相同的一套（层感知 + 护栏）：它就是
        // 「今天的 Drop」，不是第三套丢弃策略。
        if matches!(mode, Compact::Drop | Compact::AppendOnly) {
            cut = layer_aware_drop_cut(history.as_slice(), &tool_names, head, cut);
            // P6 小补丁：再让「具名事实 / 口令行 / 叙述型约束」留在保留侧；收缩不足一半
            // 时自动让路（原样返回 `cut`），最坏情况与不接线逐字节一致。`Force` 不走这里。
            cut = guarded_drop_cut(history.as_slice(), head, cut);
        }
        if cut > head {
            if history[head..cut].iter().any(has_todo_write) {
                pinned_task_snapshot = latest_todo_snapshot(history);
            }
            dropped_msgs = history.drain(head..cut).collect();
            dropped = dropped_msgs.len();
            dropped_msgs_chars = dropped_msgs.iter().map(message_chars).sum();
        }
    }
    // 用户要求快照：只。*丢过消息**的轮次钉回。
    // 2026-11-15：只。tool_result、没丢消息的轮次**不插消息** —。插在 history 靠前
    // 位置会从这条起作废整段前缀缓存（见下方「副作用」注释），而这类压缩发生得很频繁。
    // 「没丢消息也要重新强调目标」改由每轮贴在用户消息里。`goal_anchor` 承担（尾部，不碰前缀）。
    let pinned_user_contract = if dropped > 0 {
        user_contract_snapshot(&dropped_msgs)
    } else {
        None
    };
    if append_only {
        // 落一行「这一档到底做了什么」：瘦了多少、折算后离预算多远、有没有被迫丢消息。
        let freed = (elided_chars / APPEND_ONLY_CHARS_PER_TOKEN) as u64;
        log::info(format!(
            "append-only：瘦身 {elided} 块（省 {elided_chars} 字 ≈ {freed} token），折算后 ~{} / {} ⇒ {}",
            measured_tokens.saturating_sub(freed),
            max_context_tokens(),
            if dropped > 0 {
                format!("仍可能撞上限，回落 Drop 丢 {dropped} 条")
            } else {
                "不丢消息".to_string()
            }
        ));
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

    // 副作用：插在靠前位置 ⇒ 从这条之后的前缀缓存作废。但这只在**已经丢弃过**的轮次才发生，
    // （丢弃逻辑刻意保留 head 那条，就是为了这个）。
    // 它是一条**纯文本 user 消息**，与 `tool_use` / `tool_result` 的配对结构完全解耦 ——
    // 所以不会像「原样保留那条工具消息」那样把配对拆坏（端点是硬校验的）。
    // 副作用：插在靠前位置 。从这条之后的前缀缓存作废。但这只。*已经丢弃。*的轮次才发生。
    // 那一轮本来就已经因为 drain 把缓存废掉了，不算额外损失。
    let mut pin_at = 1.min(history.len());
    if pinned_user_contract.is_some() {
    // 所以不会像「原样保留那条工具消息」那样把配对拆坏（端点是硬校验的）。
        history.retain(|m| !is_user_contract_msg(m));
        pin_at = 1.min(history.len());
    }
    if let Some(contract) = pinned_user_contract {
        history.insert(
            pin_at,
            json!({ "role": "user", "content": [{ "type": "text", "text": contract }] }),
        );
        pin_at += 1;
        pinned += 1;
        log::info("用户要求快照已钉住：压缩丢消息时保住原始目标与中途修改");
    }
    if let Some(snapshot) = pinned_task_snapshot {
        history.insert(
            pin_at.min(history.len()),
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
        elided_msgs,
        pinned,
        freed_chars: elided_chars + dropped_msgs_chars,
    }
}

/// 主循环认的 stdin 输入类型（2026-10-11 起新增 compact）。
///
/// 抽成枚举只为可单测「哪些类型被识别」；判定与主循环那处 match **逐字一致** ——
/// 不带 compact 时行为与改造前逐字节相同，未知类型仍落 Unknown（照旧打「忽略输入类型」）。
enum StdinKind<'a> {
    User,
    SetHistory,
    ControlResponse,
    Compact,
    /// 暂停当前回合（§待办 3-28.1）：只在**轮首安全点**生效，不在工具批中途打断。
    Pause,
    Unknown(Option<&'a str>),
}

fn stdin_kind(msg: &Value) -> StdinKind<'_> {
    match msg.get("type").and_then(Value::as_str) {
        Some("user") => StdinKind::User,
        Some("set_history") => StdinKind::SetHistory,
        Some("control_response") => StdinKind::ControlResponse,
        Some("compact") => StdinKind::Compact,
        Some("pause") => StdinKind::Pause,
        other => StdinKind::Unknown(other),
    }
}

/// 回合运行中收到的消息该怎么处理（§待办 3-28.1「中途三态控制」）。
///
/// 判定集中在 [`drain_steering`] 一处：只有显式带 `steer = "insert"` 的 `user`
/// 才会被注入，其余消息原样进 `backlog` 交给主循环 —— 这样其他调用方（agent_server /
/// 桌宠）在回合中误发的消息不会被吞掉，仍在回合结束后按原语义执行。
struct Steering {
    /// 要注入本回合的 user 消息（text, 原始 image 块）。
    inserts: Vec<(String, Vec<Value>)>,
    /// 收到 `{"type":"pause"}` ⇒ 在轮首停住并落检查点。
    paused: bool,
}

/// 轮首安全点：把 stdin 队列里**已到达**的消息分流。
///
/// 为什么只能在轮首做：此刻上一批 `tool_use` 的 `tool_result` 已经全部回灌，
/// history 是静止的 —— 注入一条 user 消息不会拆坏配对；而工具批中途插消息会直接
/// 触发端点的 `tool_use` / `tool_result` 校验失败。
fn drain_steering(rx: &mpsc::Receiver<Value>, backlog: &mut VecDeque<Value>) -> Steering {
    let mut inserts = Vec::new();
    let mut paused = false;
    loop {
        match rx.try_recv() {
            Ok(msg) => match stdin_kind(&msg) {
                StdinKind::User
                    if msg.get("steer").and_then(Value::as_str) == Some("insert") =>
                {
                    let mut text = extract_user_text(&msg);
                    let images = extract_user_images(&msg);
                    if text.trim().is_empty() && images.is_empty() {
                        continue;
                    }
                    if text.trim().is_empty() {
                        text = "(the user attached image(s) with no text)".to_string();
                    }
                    inserts.push((text, images));
                }
                StdinKind::Pause => paused = true,
                // 非插入类消息一律留给主循环（顺序与到达顺序一致）。
                _ => backlog.push_back(msg),
            },
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => break,
        }
    }
    Steering { inserts, paused }
}

/// 把一批中途插入压进 history，并上报统计事件。返回实际插入的条数。
fn apply_steering(
    inserts: Vec<(String, Vec<Value>)>,
    history: &mut Vec<Value>,
    question_seq: u64,
    steer_count: &mut usize,
) -> usize {
    let n = inserts.len();
    for (text, images) in inserts {
        push_steering_message(history, &text, &images);
        *steer_count += 1;
        log::info(format!(
            "steer=q{question_seq} #{} chars={}",
            *steer_count,
            text.chars().count()
        ));
        // 只发一条统计事件：界面自己已经把用户那句话渲染出来了，不在这里重复正文。
        emit(json!({
            "type": "system",
            "subtype": "steer_inserted",
            "chars": text.chars().count() as u64,
        }));
    }
    n
}

/// 把一条中途插入的 user 消息压进 history。文本长度沿用单条提问的上限。
fn push_steering_message(history: &mut Vec<Value>, text: &str, raw_images: &[Value]) {
    let text = if text.chars().count() > MAX_USER_CHARS {
        text.chars().take(MAX_USER_CHARS).collect::<String>()
    } else {
        text.to_string()
    };
    let (image_blocks, skipped) = collect_image_blocks(raw_images);
    for n in &skipped {
        log::warn(format!(
            "中途插入的附件未发出: {}（{}）",
            n.get("path").and_then(Value::as_str).unwrap_or("?"),
            n.get("reason").and_then(Value::as_str).unwrap_or("?"),
        ));
    }
    let mut content: Vec<Value> = vec![json!({ "type": "text", "text": text })];
    content.extend(image_blocks);
    history.push(json!({ "role": "user", "content": content }));
}

/// 手动压缩一次（stdin 输入类型 compact，2026-10-11）。
///
/// **复用自动压缩的同一条路径**（PreCompact hook -> compact_history(Force) -> 摘要压缩）——
/// 它只是 Force 档的第三个调用点（前两个：预算续命、400 兜底），不再抄一份压缩逻辑。
///
/// 护栏：
///   · 空历史：**不**调 compact_history。Force 档会给「开头不是 user 文本消息」的历史插一条
///     TRIMMED_MARKER 合成消息，空历史会因此被无中生有地塞一条 ⇒ 直接回 elided/dropped 均为 0
///     的回执，既不崩也不污染历史。
///   · read_only / plan 档：**允许**压缩。它只改内存里的 history，不写盘、不新增落点；自动路径
///     （预算闸 / 400 兜底）同样不看 read_only ⇒ 这里保持同一语义（PreCompact hook 照发）。
///   · 压不动（elided + dropped 均为 0）：compact_history 内部**只在真压动时才发**
///     context_compacted，这里补发一条 0/0 的 —— 前端「已请求压缩」的占位一定能收到明确回执。
///   · 并发：本函数在**主循环**里调用；跑回合时主循环正阻塞在 run_query ⇒ 压缩期间到达的
///     compact 只会在 stdin 队列里排队、**等本回合收尾后才执行**（不打断流式 / 审批）。选最安全的排队。
///
/// 返回「需要**补发**的回执事件」（None = compact_history 已经发过，别重复发）+ 「是否真压动过」
/// （调用方据此在压缩边界刷新记忆快照，§待办 3-9 C）。抽成函数便于单测：空历史 / 压不动
/// 两条路径都不联网、不发 stdout。
fn manual_compact(
    cfg: &Cfg,
    history: &mut Vec<Value>,
    cwd: &Path,
    scope: tools::MemoryScope,
) -> (Option<Value>, bool) {
    if history.is_empty() {
        log::info("手动压缩：历史为空，无可压缩内容");
        return (Some(json!({
            "type": "system",
            "subtype": "context_compacted",
            "elided": 0,
            "dropped": 0,
        })), false);
    }
    fire_plain_hook("PreCompact", cwd, json!({ "trigger": "manual" }));
    let measured = cfg.last_input.get();
    // 本地只读预筛（S1）：压缩事件点上**只记账，绝不改压缩行为**（手动压缩无提问号 ⇒ turn=0）。
    prescreen_compact("manual", &format!("measured={measured}"), scope, 0);
    // 记忆候选抽取（S1 第二切片）：压缩事件同样触发一次；**只落账本**、不改压缩行为、不改注入。
    extract_candidates(history, scope, 0);
    let out = compact_history(history, Compact::Force, measured);
    // 摘要压缩 + 结论索引写入 + 检索重注入：与自动路径同款（Force 档丢弃量最大，最值钱）。
    // 返回值只用来修正调用方的回滚锚点，本路径没有回滚锚点 ⇒ 忽略。
    let _ = restore_after_compaction(
        cfg,
        history,
        &out.dropped_msgs,
        &out.elided_msgs,
        scope,
        0,
        &conclusions::default_dir(),
    );
    // last_input 清零（与自动路径一致）：下一次请求重新按实测水位判断。
    cfg.last_input.set(0);
    if out.elided + out.dropped > 0 {
        // 只有真压动才推进滞回时钟 —— 「扫了一圈但没动」不该白等一个增长窗口。
        cfg.last_compact.set(measured);
    }
    log::info(format!(
        "手动压缩：瘦身 {} 个 tool_result，丢弃 {} 条旧消息",
        out.elided, out.dropped
    ));
    if out.elided + out.dropped == 0 {
        return (Some(json!({
            "type": "system",
            "subtype": "context_compacted",
            "elided": 0,
            "dropped": 0,
        })), false);
    }
    (None, true)
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
fn summarize_dropped(
    cfg: &Cfg,
    dropped: &[Value],
    meta: &conclusions::Meta,
) -> Option<(String, Vec<conclusions::Conclusion>)> {
    let transcript = render_dropped_for_summary(dropped);
    // 不值得为一点点内容花一次调用
    if transcript.chars().count() < SUMMARY_MIN_INPUT_CHARS {
        eprintln!(
            "[agent] 跳过摘要压缩：被丢内容仅 {} 字 < 阈值 {SUMMARY_MIN_INPUT_CHARS} 字",
            transcript.chars().count()
        );
        return None;
    }
    // 摘要提示。= 原摘要指。+ 结构化结论段指令。*同一次调用产出两。*，零新增请求）。
    let system_prompt = format!("{SUMMARY_PROMPT}{}", summary_index_instruction());
    let mut body = json!({
        "model": cfg.model,
        "max_tokens": SUMMARY_MAX_OUTPUT_TOKENS,
        // **非流式**：摘要是内部产物，不必往前端流 —— 也不该占用 stream_event 通道
        "stream": false,
        "system": system_prompt,
        "messages": [{ "role": "user", "content": [{ "type": "text", "text": transcript }] }],
        // 2026-11-02：摘要**必须关思考**。不关时端点默认就思考，1024 的输出额度会被
        // 思考吃光 ⇒ 返回空摘要（2026-11-01 实测 8/49 次）。DeepSeek 认 disabled；
        // 不认该字段的兼容层会在下面按 400 去掉它重试一次。
        "thinking": { "type": "disabled" },
    });
    let started = Instant::now();
    // 调试转储（LUNAC_DUMP_API）：摘要请求非流式，request/response 各一行。摘要用量本就
    // 不落任何账，req_id 用时间戳生成、仅供人工区分（不与 usage 明细对应）。
    let dump_on = log::dump_api_enabled();
    let dump_rid = if dump_on {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        format!("summarize-{ms}")
    } else {
        String::new()
    };
    // 一次摘要最多三次尝试（三条互斥的重试理由，各自只放一次）：
    //   ① 端点不接受 `thinking=disabled` ⇒ 去掉该字段；
    //   ② 返回空摘要 ⇒ max_tokens 翻倍；
    //   ③ 输出触顶（out_tokens == max_tokens）⇒ 结构化结论段在输出末尾，极可能被截断 ⇒ 翻倍重试。
    // ③ 是 2026-11-04 补的：实测 3 次摘要压缩全部「结构化结论 0 条」且输出恰好 1024 tokens，
    // 模型根本没机会写到末尾的 JSON 段；此前只在「空摘要」时重试，截断完全看不见。
    let mut max_tokens = SUMMARY_MAX_OUTPUT_TOKENS;
    let mut retried_thinking = false;
    let mut retried_empty = false;
    let mut retried_truncation = false;
    let (text, out_tokens) = loop {
        body["max_tokens"] = json!(max_tokens);
        if dump_on {
            log::dump_api("request", &dump_rid, &cfg.model, None, None, &body);
        }
        let resp = model_post(cfg)
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
                if dump_on {
                    log::dump_api(
                        "response",
                        &dump_rid,
                        &cfg.model,
                        None,
                        Some(&e.to_string()),
                        &Value::Null,
                    );
                }
                log::warn(format!("摘要压缩失败（网络层）：{e}"));
                return None;
            }
        };
        let status = resp.status();
        if !status.is_success() {
            let detail: String = resp
                .text()
                .unwrap_or_default()
                .trim()
                .chars()
                .take(200)
                .collect();
            if dump_on {
                log::dump_api(
                    "response",
                    &dump_rid,
                    &cfg.model,
                    Some(status.as_u16()),
                    None,
                    &Value::String(detail.clone()),
                );
            }
            if status == 400 && thinking_related_error(&detail) && !retried_thinking {
                retried_thinking = true;
                if let Some(obj) = body.as_object_mut() {
                    obj.remove("thinking");
                }
                log::warn("摘要压缩：端点不接受 thinking=disabled，去掉该字段重试一次");
                continue;
            }
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
        if dump_on {
            log::dump_api("response", &dump_rid, &cfg.model, Some(status.as_u16()), None, &v);
        }
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
        let out_tokens = v
            .get("usage")
            .and_then(|u| u.get("output_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if text.trim().is_empty() {
            if !retried_empty {
                retried_empty = true;
                max_tokens = max_tokens.saturating_mul(2);
                log::warn("摘要压缩：模型返回空摘要，max_tokens 翻倍重试一次");
                continue;
            }
            log::warn("摘要压缩失败：模型返回空摘要");
            return None;
        }
        // 触顶判定（2026-11-04）：结构化结论段在输出**末尾**，模型恰好写满 max_tokens 时
        // 它最先被截掉。不判这一条，日志只会留下「结构化结论 0 条」，看起来像模型没照做。
        // `>=` 而不是 `==`：有的兼容层会把 output_tokens 报得比请求上限略大。
        if out_tokens >= u64::from(max_tokens) && !retried_truncation {
            retried_truncation = true;
            let prev = max_tokens;
            max_tokens = max_tokens.saturating_mul(2);
            log::warn(format!(
                "摘要压缩：输出触顶（{out_tokens}/{prev} tokens）⇒ max_tokens 提到 {max_tokens} 重试一次"
            ));
            continue;
        }
        break (text, out_tokens);
    };
    let text = text.trim();
    // 切出结构化结论段：摘要正文里**不留**它（钉回历史的只有摘要）。拿不到段 ⇒ 结论为空，
    // 调用方退到确定性抽取（见 `restore_after_compaction`）。
    let (summary, entries) = conclusions::parse_model_block(text, meta);
    if summary.is_empty() {
        log::warn("摘要压缩失败：模型返回空摘要（去掉结构化段后为空）");
        return None;
    }
    // 「0 条」不能只留一行 INFO（2026-11-04）：它意味着模型侧的结论索引这一轮完全缺位，
    // 调用方会退到确定性抽取（能力更弱）。让它在日志里显眼，才能发现截断复发。
    if entries.is_empty() {
        log::warn(
            "摘要压缩：结构化结论 0 条（模型未按标记输出，或重试后仍被截断）—— 本轮退到确定性抽取",
        );
    }
    log::info(format!(
        "摘要压缩：{} 条旧消息 / {} 字 → 摘要 {} 字 / 结构化结论 {} 条（输出 {out_tokens} tokens，耗时 {}ms）",
        dropped.len(),
        transcript.chars().count(),
        summary.chars().count(),
        entries.len(),
        started.elapsed().as_millis()
    ));
    Some((format!("{SUMMARY_HEADER}\n{summary}"), entries))
}

/// 压缩后的收尾：摘要钉回 + **可恢复结论索引**写入 + 检索并重注入。
///
/// 与任务快照同一个位置策略 —— 合成消息**插在第一条之后**，不抢「开头那条用户提问」的位置。
/// 三者同时存在时的顺序是 [摘要][结论索引][任务快照]：摘要 = 「过去发生了什么」，结论索引 =
/// 「可复用的结论」，任务快照 = 「现在要做什么」，读起来正好由远及近。
/// **全部落在可变层（history），绝不进 system / 固定前缀**（见 `forbidden.md` §禁用 45）。
///
/// 四件事都只在这里做（**单一入口**，被 Drop / Force 四个调用点复用，见 §禁用 62）：
///   ① 摘要（或确定性兜底清单）。**2026-10-10 起拿不到模型摘要也必须退到
///      `fallback_digest_of_dropped`**：`compact_history` 是先 drain 再让模型写摘要，原先模型
///      摘要失败就 `return false` ⇒ 被丢的几十条永久没有顶替物，模型手里只剩「开头那条最早的
///      提问」+ 尾部，于是把**最早那个诉求**当成当前任务往下做（用户报障原话：「只要压缩上下文
///      直接回到了上一次人格修改」）。
///      提问」+ 尾部，于是把**最早那个诉求**当成当前任务往下做（用户报障原话：「只要压缩上下文
///      抽取。被瘦身（elide）的可重现结果也写指针条目。**失败条目不删**（Manus「Keep the Wrong
///      Stuff In」：删掉失败会移除模型纠错所需的证据）—— 用 `status=failed` + `reason` 标注。
///   ③ 检索 + 重注入：按「最近诉求 + 任务快照关键词 + 被丢区间标识符」查询，重排后取 top-N，
///      拼成一段注入到**紧随压缩摘要之后**（可变层，避开 lost-in-the-middle）。
///   ④ 指针回注（2026-10-27，§待办 3-16 P1-7）：被丢区间里的**可重跑**调用（Read / Grep /
///      Glob / Cmd / PowerShell / WebFetch / WebSearch）列成 `[elided pointers]`，让模型
///      知道「怎么再查一遍」而不只是「查过什么」。只对 `dropped` 做（瘦身档参数还在原位）。
///
/// 返回**插入的合成消息条数**（摘要 0/1 + 结论块 0/1 + 指针块 0/1）—— 调用方拿它修正回滚锚点 `base`。
fn restore_after_compaction(
    cfg: &Cfg,
    history: &mut Vec<Value>,
    dropped: &[Value],
    elided: &[Value],
    scope: tools::MemoryScope,
    turn: u64,
    dir: &Path,
) -> usize {
    let meta = conclusions::Meta {
        ts: now_epoch_secs(),
        scope,
        turn,
        session_id: session_id(),
    };
    let mut entries: Vec<conclusions::Conclusion> = Vec::new();
    let mut summary_text: Option<String> = None;
    if !dropped.is_empty() {
        let (text, model_entries) = if summary_compact_enabled() {
            match summarize_dropped(cfg, dropped, &meta) {
                Some((s, e)) => (s, Some(e)),
                None => {
                    let d = fallback_digest_of_dropped(dropped);
                    log::warn(format!(
                        "摘要压缩不可用（模型摘要失败 / 输入过短）⇒ 退到确定性兜底清单（{} 字），\
                         保证被丢弃的 {} 条有顶替物",
                        d.chars().count(),
                        dropped.len()
                    ));
                    (d, None)
                }
            }
        } else {
            (fallback_digest_of_dropped(dropped), None)
        };
        if !text.trim().is_empty() {
            summary_text = Some(text);
        }
        // 优先用模型结构化段；拿不到（或段为空）⇒ 确定性抽取（**兜底路径也写索引**）。
        entries = match model_entries {
            Some(e) if !e.is_empty() => e,
            _ => conclusions::extract_from_dropped(dropped, &meta),
        };
    }
    entries.extend(conclusions::extract_from_elided(elided, &meta));
    conclusions::append(dir, scope, &entries);

    let mut added = 0usize;
    if let Some(text) = summary_text {
        history.insert(
            1.min(history.len()),
            json!({ "role": "user", "content": [{ "type": "text", "text": text }] }),
        );
        added += 1;
    }
    // 检索 + 重注入：只在**真的丢了内容**（Drop / Force）时做 —— Elide / ElideCold 不丢整条，
    // 无「结论被丢」可言，也就没有重注入的必要（用户裁决：检索时机 = 压缩完成后 Drop/Force）。
    if !dropped.is_empty() {
        let query = build_conclusion_query(history, dropped, elided);
        let picked = conclusions::retrieve(
            dir,
            scope,
            &query,
            meta.ts,
            CONCLUSIONS_MAX_ITEMS,
            CONCLUSIONS_INJECT_CHARS,
        );
        let block = conclusions::render_injection(&picked);
        if !block.is_empty() {
            // 紧随摘要之后（没有摘要时紧随 head 之后）。
            let at = (if added > 0 { 2usize } else { 1usize }).min(history.len());
            history.insert(
                at,
                json!({ "role": "user", "content": [{ "type": "text", "text": block }] }),
            );
            log::info(format!(
                "结论索引注入：{} 条 / {} 字，落在可变层（紧随压缩摘要之后）",
                picked.len(),
                block.chars().count()
            ));
            added += 1;
        }
    }
    // ③ 指针回注（2026-10-27，§待办 3-16 P1-7）：把**被丢掉**的可重跑调用列成
    // `[elided pointers]`。只在 `dropped` 非空时做 —— 瘦身档的结果原位只剩一个占位符，
    // 参数与工具名都还在，模型自己看得到，再列一遍只是白占预算。
    if !dropped.is_empty() {
        let calls = collect_pointer_calls(dropped);
        if !calls.is_empty() {
            let recent = recent_user_requests(history, 2);
            let pointers = ptrblock::render_pointers(&calls, &recent, PTRBLOCK_MAX_CHARS);
            if !pointers.is_empty() {
                let at = (1 + added).min(history.len());
                history.insert(
                    at,
                    json!({ "role": "user", "content": [{ "type": "text", "text": pointers }] }),
                );
                log::info(format!(
                    "指针回注：{} 条可重跑调用 / {} 字（紧随结论索引，可变层）",
                    pointers.lines().count().saturating_sub(1),
                    pointers.chars().count()
                ));
                added += 1;
            }
        }
    }
    added
}

/// 从被丢弃的消息里收集**可重跑**的调用，供 `ptrblock` 渲染。
///
/// 只看 `tool_use` 块（`tool_result` 自己不带参数）；`input` 摊平成键值对，非字符串值走
/// JSON 序列化。**顺序是确定的**：`serde_json` 的 Map 是 BTreeMap（未开 `preserve_order`），
/// 键按字典序 —— 指针块进 history 后若每轮不一样，就成了「自己打掉自己前缀缓存」的新来源。
fn collect_pointer_calls(dropped: &[Value]) -> Vec<ptrblock::Elided> {
    let mut out: Vec<ptrblock::Elided> = Vec::new();
    for msg in dropped {
        let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
            continue;
        };
        for b in blocks {
            if b.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let Some(name) = b.get("name").and_then(Value::as_str) else {
                continue;
            };
            let mut input: Vec<(String, String)> = Vec::new();
            if let Some(Value::Object(map)) = b.get("input") {
                for (k, v) in map {
                    let text = match v {
                        Value::String(s) => s.clone(),
                        Value::Null => String::new(),
                        other => serde_json::to_string(other).unwrap_or_default(),
                    };
                    input.push((k.clone(), text));
                }
            }
            out.push(ptrblock::Elided {
                tool: name.to_string(),
                input,
            });
        }
    }
    out
}

/// 最近 N 条**真实用户诉求**（跳过合成消息：以 `[` / `#` 开头的上下文块、摘要、结论块）。
///
/// 两个消费者共用：结论检索的查询串（`build_conclusion_query`）与指针块的「相关性排序」
/// （`ptrblock`）。抽出来是为了让「什么算用户说的话」只有一处判据 —— 两处各写一份，
/// 迟早有一处把注入的合成块也当成用户诉求。
fn recent_user_requests(history: &[Value], limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if limit == 0 {
        return out;
    }
    for msg in history.iter().rev() {
        if out.len() >= limit {
            break;
        }
        if msg.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
            continue;
        };
        for b in blocks {
            if b.get("type").and_then(Value::as_str) != Some("text") {
                continue;
            }
            let t = b.get("text").and_then(Value::as_str).unwrap_or("").trim();
            if t.is_empty() || t.starts_with('[') || t.starts_with('#') {
                continue;
            }
            out.push(t.chars().take(300).collect());
            break;
        }
    }
    out
}

/// 构造检索查询串：**最近的用户诉求 + 任务快照关键词 + 被丢弃 / 被瘦身区间的标识符**。
///
/// 设计说明（为什么这么拼）：
///   · 最近的用户诉求 = 「现在要做什么」，是检索的相关性锚点（取尾部最近 2 条用户文本）；
///   · 任务快照 = 当前清单（`TodoWrite`），补上「用户没再重复、但仍在做」的目标；
///   · 被丢区间的标识符 = 让「文件名 / 函数名 / 命令」这类精确标识符能把对应结论拉回来。
fn build_conclusion_query(history: &[Value], dropped: &[Value], elided: &[Value]) -> String {
    let mut parts: Vec<String> = recent_user_requests(history, 2);
    if let Some(snap) = latest_todo_snapshot(history) {
        parts.push(snap);
    }
    let mut ids: Vec<String> = Vec::new();
    // 除标识符外也带上被丢区间的**原文片段**：BM25 是词面匹配，标识符未必覆盖普通词
    // （如错误文本里的 cannot / find / macro），带上原文才能把相关结论拉回来。
    let mut raw: Vec<String> = Vec::new();
    for msg in dropped.iter().chain(elided.iter()) {
        let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
            continue;
        };
        for b in blocks {
            match b.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        ids.extend(conclusions::extract_identifiers(t));
                        raw.push(t.chars().take(200).collect());
                    }
                }
                Some("tool_use") => {
                    if let Some(i) = b.get("input") {
                        let args = tools::summarize_args(i);
                        ids.extend(conclusions::extract_identifiers(&args));
                        raw.push(args.chars().take(200).collect());
                    }
                }
                _ => {}
            }
        }
    }
    ids.sort();
    ids.dedup();
    ids.truncate(40);
    raw.truncate(40);
    parts.extend(ids);
    parts.extend(raw);
    parts.join(" ")
}

/// 里程碑写入（结论索引的**第二个写入点**，用户裁决「里程碑 / 压缩边界」里的里程碑）：
/// 一轮提问收尾（`run_query` 返回）后，从最近的对话窗口里**确定性**抽取结论写进索引。
///
/// 为什么选「轮末」而不是「每次工具调用 / 每轮」：轮末是**里程碑**（一轮诉求完成）；工具调用
/// 级别会频繁变动前缀、制造噪音、打爆 prompt cache（用户裁决红线）。本写入**不调模型**，且
/// `append` 去重 ⇒ 反复抽到同一结论不会让索引膨胀。
///
/// 与压缩边界那次写入的分工：里程碑把结论**提前**落盘（在它们可能被丢弃之前）；压缩边界那次
/// 负责补上「里程碑之后又被丢掉的那一段」（见 `restore_after_compaction`）。
fn flush_turn_conclusions(scope: tools::MemoryScope, history: &[Value], turn: u64) {
    if history.is_empty() {
        return;
    }
    // 只看最近一个窗口（去重保证不重复写）；不扫全历史，避免每次轮末都做重活。
    let start = history.len().saturating_sub(COMPACT_KEEP_TAIL * 2);
    let meta = conclusions::Meta {
        ts: now_epoch_secs(),
        scope,
        turn,
        session_id: session_id(),
    };
    let entries = conclusions::extract_from_dropped(&history[start..], &meta);
    conclusions::append(&conclusions::default_dir(), scope, &entries);
}

/// 摘要压缩的**确定性兜底清单**（不调模型、不会失败）。
///
/// 取**被丢区间里的用户文本消息**（「用户一路想要什么」的最直接载体），逐条压成一行、
/// 按原序排列，并显式声明「最新的那条用户消息才是当前任务、最早那条只是背景」——
/// 按原序排列，并显式声明「最新的那条用户消息才是当前任务、最早那条只是背景」——
/// 按原序排列，并显式声明「最新的那条用户消息才是当前任务、最早那条只是背景」——
/// 我们的合成消息（`TRIMMED_MARKER` / 摘要头 / 任务快照 / 本兜底自身）都以 `[` 开头，一律跳过。
fn fallback_digest_of_dropped(dropped: &[Value]) -> String {
    const MAX_ASKS: usize = 40;
    const ASK_CHARS: usize = 140;
    let mut asks: Vec<String> = Vec::new();
    let mut tool_results = 0usize;
    for msg in dropped {
        let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
            continue;
        };
        let is_user = msg.get("role").and_then(Value::as_str) == Some("user");
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_result") => tool_results += 1,
                Some("text") if is_user => {
                    let Some(t) = block.get("text").and_then(Value::as_str) else {
                        continue;
                    };
                    let one = t.split_whitespace().collect::<Vec<_>>().join(" ");
                    if one.is_empty() || one.starts_with('[') {
                        continue; // 合成消息，不是用户的话
                    }
                    asks.push(one.chars().take(ASK_CHARS).collect());
                }
                _ => {}
            }
        }
    }
    if asks.is_empty() && tool_results == 0 {
        return String::new();
    }
    let total = asks.len();
    let shown = total.min(MAX_ASKS);
    let mut out = String::from(
        "[digest of a trimmed region — the user's requests over that span, in order. \
         The LATEST user message of this session is the current task; the earliest is background only.]",
    );
    for (i, a) in asks.iter().take(shown).enumerate() {
        out.push_str(&format!("\n{}. {a}", i + 1));
    }
    if total > shown {
        out.push_str(&format!("\n…(+{} earlier requests omitted)", total - shown));
    }
    if tool_results > 0 {
        out.push_str(&format!(
            "\n({tool_results} tool results from that span were dropped; re-read or re-run as needed)"
        ));
    }
    out
}

/// 有工具之后，系统提示词改为鼓励「先看再改」的最小操作风格。
/// 技能清单（P4）在 main() 里追加到本提示词之后，见 skills::listing()。
///
/// **2026-09-29 加「批量化」那句**（L6 压请求次数）：一次提问实测 7~10 次请求，每次
/// 「模型→工具→模型」往返都要重发一遍前缀。而 `plan_tool_batches()` 早就能把**连续**的
/// 只读调用放进**同一个响应**里并发执行（上限 `TOOL_PARALLELISM`）——**一次请求带 N 个
/// `tool_use`**，不是发 N 份请求。省的是往返次数，不是并发度：原先只差一句提示词，
/// 模型每轮只发一个调用，白跑了好几趟。
///
/// **2026-10-07 加「Edit 优先于 Write」**（L6 成本归因与降本 · 输出侧）：当日用量对账显示
/// **Lunac 的账单里输出 token 占 61.7%**（同口径 Trae 只占 35.7%），而 agent 的输出大头
/// 不是散文、是**工具参数** —— 对已存在的文件全量重写一次就是几千 token，改两行却要
/// 复述整个文件。`Edit`（exact-string replace）本就是这个形态，缺的只是「优先用它」这句话。
/// 参照：search-replace 已是 13 个主流 agent 里 6 个的默认编辑形态。
const SYSTEM_PROMPT: &str = "You are Lunac's built-in assistant, running inside a Windows desktop launcher. \
Answer in the user's language and keep it concise. \
You can inspect and modify the local machine with the provided tools: prefer Read/Glob/Grep \
before editing, make the smallest change that solves the problem, and say what you changed. \
To change an existing file, use Edit -- patch only the lines that differ. Reserve Write for files \
you are creating and for whole-file rewrites you actually intend; re-emitting an unchanged file \
is pure cost. \
Batch independent lookups: when several reads or searches do not depend on one another, issue \
them as multiple tool calls in the SAME response instead of one per turn -- they run \
concurrently and each saved round-trip is a re-sent prefix you do not pay for. \
After you edit code, run the project's own check (build, tests or lint) when the repo has one, \
and report the exact command plus its result. If no such check exists, say what you verified \
and what you could not -- never call an edit verified on reasoning alone. \
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
///
/// **2026-10-08 加「阶段小结」（用户要求，L6 的唯一例外）**：让模型在**多轮工具任务**里
/// 每段思考后说一行结论、并在最终答复里把这几行汇总。为什么它值得花这点输出 token：
/// 思考块**不回灌**（回灌会 400，见 `hist.push` 那一段）⇒ 一轮工具往返之后，模型的推理
/// 在上下文里**凭空消失**，只能从 tool_result 重新推。而 `text` 块**会**进历史
/// （main.rs 的 `hist.push(blk)`），所以那一行小结是**把决策带进下一轮的唯一载体**。
/// 范围刻意收窄到「多于一次工具往返」：单步问答与闲聊一律不加（那才是 L6 要压的铺垫）。
const PERSONA_AND_STYLE: &str = r#"## Personality (fixed — always apply)
You are "Lunac", a sharp, fast desktop AI assistant built into a launcher. Stay in character every turn.
- Thinking style: before acting, briefly structure your reasoning as Context → Analysis → Decision, then execute. Do not second-guess after deciding.
- Speaking style: calm and direct, like a senior engineer explaining to a peer. Short varied sentences, first-person "I", concrete nouns and verbs.
- No filler: never use "stands as / testament / delve / tapestry / moreover / furthermore / in conclusion / great question / I hope this helps". Have a clear opinion and recommend the single best option rather than listing everything.
- Language (hard rule): answer in the language the user wrote in. A Chinese question means the whole reply is Chinese — code, commands, paths, and API/tool names stay as they are, but never write the prose in another language. If the user switches language, switch with them.

## Output Style
Remove AI writing patterns: no "stands as / testament / pivotal / crucial / underscoring / delve / tapestry / landscape / fostering / moreover / furthermore / in conclusion". No emoji decorations. No "I hope this helps / let me know / great question". No boldface headers in lists. Use simple "is/are/has" instead of "serves as/stands as/represents". Vary sentence rhythm. Have opinions.
Spend output tokens only on the answer (2026-09-29, L6): no preamble before tool calls, no announcing what you are about to do, no restating the plan, no recap of what you just read. Answer only what was asked -- skip extra findings and unrequested analysis. Never paste back content the user can already see. Keep the closing note to what changed, in one line.
Phase summaries (2026-10-08 -- the single exception to the rule above): when a task needs more than one tool round, close each round with ONE short line -- the conclusion you reached, not a plan and not a promise. Your thinking is dropped from the context between rounds; that line is kept, so it is what carries the decision into the next round. When the task is done, sum those lines up in 2-4 lines at the start of the final answer. A single-step answer emits none of this.

## Latest instruction wins (hard rule, 2026-11-15)
The user's newest message outranks everything you decided earlier: your Todo list, the one-line judgements you already printed in this conversation, and the plan you are halfway through. When it narrows, corrects or reverses the goal, first rewrite the Todo list (drop the abandoned steps), state in ONE line which step you are dropping, then work from the new goal. "Do not second-guess after deciding" applies only to decisions the user has not touched. Never keep executing the old plan while saying you accepted the correction.

## Task completion (hard rule, 2026-11-03)
If a request needs more than one step, first write a Todo plan covering every step (TodoWrite), then execute the whole plan in this same turn. Do not stop to report progress, do not hand the user a partial result, and never ask the user to send "continue" or to re-send the same request in pieces. Only end a turn before the plan is complete when you are truly blocked (missing credentials, permission denied, or a tool failure you cannot work around) -- then say exactly what is blocking and what you tried. When the user does say "continue", resume from the first unfinished item and never repeat text that is already in the conversation.

## Task phases (hard rule, 2026-11-21)
The chat panel renders a fixed layout per todo: [small task bar] -> [commands] -> [small summary]. Keep it aligned with TaskPhase: the first action of a multi-step turn is TaskPhase with state "start" and the title set to the first todo item (no prose before it); after that todo's commands finish, call TaskPhase with state "done" and a one-or-two-sentence summary of what changed and what it proves, then mark the todo completed in TodoWrite. Use the exact todo text as the title and never rename an earlier phase. The final assistant message is the big summary: do not repeat it inside TaskPhase, and do not treat a TaskPhase summary as the final answer. Skip TaskPhase for a trivial single-step reply.

## Visible decisions (hard rule, 2026-11-03)
When a step turns on a judgement that changes what you do next -- delete or keep, change or leave, install or skip, route A or route B, ask the user or decide yourself -- state that judgement in ONE short sentence in the visible reply, before you act on it. Never leave it only in thinking: thinking is collapsed, so a user who does not expand it sees you silently doing something else. Keep it to the conclusion plus the reason (e.g. "Nothing to delete in the module folder, so I am continuing with the rest."). Do not restate it afterwards, do not turn it into a plan, and do not add a second sentence of justification unless the user could reasonably disagree.

## Asking the user (hard rule, 2026-11-03)
Before calling AskUserQuestion, first write ONE short paragraph as normal text: what you were doing, where you are now, and what exactly you need the user to decide. That paragraph is the user's only context -- the question card alone is not enough. After the tool returns, continue from that paragraph (state the outcome and keep going); never repeat the summary and never append a separate closing summary that only re-says it.

## Readable answers (hard rule, 2026-11-05)
The reply is read in a chat panel, not in a document viewer.
- Make the first line the conclusion, in one sentence. Then give only the detail needed to act on it.
- Use **bold** for the few words that change a decision (a file name, a risk, a required action). Never bold whole paragraphs, and never wrap a heading in **.
- Never use **...** as a section label (for example **Not done** or **Current state**). Write that as a short natural sentence ("Not done:", "Current state:") or just start the next paragraph -- the chat renderer drops the asterisks, so a bold label only looks like a stray marker.
- When the answer has tabular data (comparisons, per-part metrics, before/after), use a markdown pipe table instead of prose lists; keep cells short.
- Local absolute paths are fine as plain text or inline code -- the chat panel renders them as clickable chips that open the file.
- A detail block longer than about 15 lines goes into a fenced code block or is left out; never paste raw logs or full file dumps into the prose.
- If you need something from the user, end with exactly one question. Do not hide it inside a numbered checklist.
- For any reported number (conflicts, changes, matches), also state how it was verified (double-run, coverage, sample) or mark it as an estimate.
- Never write "verified / fixed / effective" without naming the evidence (file system, target log, test). If you only reasoned it out, say it is an inference.

## Evidence discipline (hard rule, 2026-11-05)
- Numbers: every reported count (conflicts, matches, changed files) names how it was verified -- rerun twice with identical output, coverage of the full set, or a stated sample rate. Scripts expose limits, truncation and sampling as explicit parameters and echo them; never leave a hidden magic cap.
- Artifacts: after writing or exporting a file, verify the file itself (re-read it, hash it, or diff it against the input) -- never the in-memory object that produced it. "The exporter reported success" is not evidence that the output changed.
- Done means the acceptance metric moved: before declaring success, restate the user's criterion and show the measured before/after. If the metric did not improve, the task is not done, no matter how many checks passed.
- First-hand rules: when the question is how a program loads, sorts, merges or applies something, read the executable strings, the program's own log, or its config parser output before reasoning. If first-hand evidence is unavailable, label the conclusion as an inference.
- Coordinates: before acting on "move up/down" or "first/last", write the mapping between the user's UI order and the on-disk order (for example "top of panel = last line of file") and verify it once with a count or log line. Do not act until that mapping is checked.
- Visual claims: without a screenshot or an executable check, report candidate causes only. Never state that a rendering issue is fixed, or blame a specific asset, on reasoning alone.
- Files owned by another running program: check the process and file lock before writing. If a program may flush its in-memory state back to disk, ask the user to close it first.
- Repeated scans: write the first scan result under the task's temp directory and reuse it with mtime checks; never rescan the same tree every round. Experiment artifacts and backups belong under `<exe root>\temp\transStorage\<task>\`, never inside the directory being modified."#;

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

// ── 项目记忆（AGENTS.md，2026-10-01；多层向上递归，2026-10-11）──────
//
// 为什么需要它：MEMORY.md（Remember 工具）是跨项目的长期记忆 —— 「这个仓库要怎么构建、
// 目录有什么规矩、哪些东西不许动」混进去会被别的项目读到；而这些恰恰是每进一个仓库就要
// 重新说一遍的东西。业界已把 AGENTS.md 当成这件事的约定，所以直接读这个名字，不另发明。
//
// 分层规则（2026-10-11，「哪一级算数」的裁定）：
//   · 从工作目录逐级向上到**盘根**，每一级只要有 AGENTS.md 就收（见 collect_agents_md）；
//   · 越靠近工作目录的越具。排在越后，冲突时它说了算（「都注入、后来者优先」）。
//   · 越靠近工作目录的越具体 ⇒ 排在越后，冲突时它说了算（「都注入、后来者优先」）；
//   · 越靠近工作目录的越具体 ⇒ 排在越后，冲突时它说了算（「都注入、后来者优先」）；
//
// **只在启动时读一次**：它进的是系统提示词固定前缀，热读会把整段缓存打掉 —— 改完
// AGENTS.md 要重启 agent（或切换一次思考开关）才生效，与 persona 同一条纪律。工作目录在
// **只在启动时读一次**：它进的是系统提示词固定前缀，热读会把整段缓存打掉 —— 改完
//
// **不做「子目录级」的按需加载**：那要先知道「模型此刻在动哪个目录」，而 agent 没有这个
// 状态（工具只收到显式路径），只能靠每次工具调用去扫、把结果拼进当轮消息 —— 那属于可变层
// （不进固定前缀，故不直接废缓存），但扫描时机与注入位置的代价要单独设计。故本层只做
// 「启动时确定集合」，子目录级留在 todo.md 作为独立子项（见 §待办 3-1）。
//
// **与「没这个功能」逐字节等价**：一个 AGENTS.md 都没有 ⇒ 一个字节都不加。
const AGENTS_MD_FILE: &str = "AGENTS.md";
/// 项目记忆段的表头。英文的原因同 USER_PERSONA_HEADER：这是给模型看的元信息。
/// 它同时是判据锚点 —— 单测靠它判断「这段进没进提示词」。
const AGENTS_MD_HEADER: &str = "## Project instructions (from AGENTS.md — always apply)";
/// 单份 AGENTS.md 上限（字符）。理由同 MAX_PERSONA_CHARS：这段进每次请求都要发的固定
/// 前缀，截断只是防御（用户可能往 AGENTS.md 里堆一整篇开发手册）。
const MAX_AGENTS_MD_CHARS: usize = 8_000;
/// 多层项目记忆的**总**上限（字符）—— 向上递归可能凑出好几份，不能让它们无限叠加。
const MAX_AGENTS_TOTAL_CHARS: usize = 16_000;
/// 向上递归的层级上限（防御：异常深的目录树 / 符号链接环）。
const MAX_AGENTS_MD_LEVELS: usize = 12;

/// 读某一级目录下的 AGENTS.md。没有这个文件是**正常状态**（大多数目录都没有）⇒ None。
fn read_agents_md(dir: &std::path::Path) -> Option<String> {
    let path = dir.join(AGENTS_MD_FILE);
    match std::fs::read_to_string(&path) {
        Ok(raw) => Some(raw),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            log::warn(format!("项目记忆读不出（{}）：{e}", path.display()));
            None
        }
    }
}

/// 收集「工作目录 → 盘根」链路上每一级的 AGENTS.md（**浅→深**，即先父级后本级）。
///
/// 只收**非空白**的正文；一个都没有 ⇒ 空 vec。文件集合在进程生命周期内固定 ⇒ 前缀稳定。
/// 依赖 std 的 Path::parent() 逐级上溯（Windows 上到 C:\ 为止，其 parent 为 None）。
fn collect_agents_md(cwd: &std::path::Path) -> Vec<(PathBuf, String)> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut cur = Some(cwd.to_path_buf());
    while let Some(d) = cur {
        dirs.push(d.clone());
        cur = d.parent().map(std::path::Path::to_path_buf);
        if dirs.len() >= MAX_AGENTS_MD_LEVELS {
            break;
        }
    }
    dirs.reverse(); // 浅 → 深
    let mut files = Vec::new();
    for d in dirs {
        if let Some(raw) = read_agents_md(&d) {
            if !raw.trim().is_empty() {
                files.push((d.join(AGENTS_MD_FILE), raw));
            }
        }
    }
    files
}

/// 把「浅→深」的项目记忆拼成固定追加段（纯函数，便于单测）。
///
/// 单份文件时输出与单层实现**逐字节相同**（不带来源标签）；多份时每份前面加一行来源路径，
/// 单份文件时输出与单层实现**逐字节相同**（不带来源标签）；多份时每份前面加一行来源路径，
fn project_block_from_chain(files: &[(PathBuf, String)]) -> String {
    // 逐份 trim + 单份截断，丢掉空文件
    let mut present: Vec<(String, String)> = Vec::new();
    for (path, raw) in files {
        let text = raw.trim();
        if text.is_empty() {
            continue;
        }
        let text: String = if text.chars().count() > MAX_AGENTS_MD_CHARS {
            log::warn(format!(
                "AGENTS.md 超过 {MAX_AGENTS_MD_CHARS} 字符（{}）—— 固定前缀不适合放长文，已截断",
                path.display()
            ));
            text.chars().take(MAX_AGENTS_MD_CHARS).collect()
        } else {
            text.to_string()
        };
        present.push((path.display().to_string(), text));
    }
    if present.is_empty() {
        return String::new();
    }
    if present.len() == 1 {
        return format!("\n\n{AGENTS_MD_HEADER}\n{}", present[0].1);
    }
    // 多份：从深到浅累加预算，超出的（更浅的）丢掉
    let mut kept: Vec<&(String, String)> = Vec::new();
    let mut total = 0usize;
    let mut dropped = 0usize;
    for item in present.iter().rev() {
        let n = item.1.chars().count();
        if total + n > MAX_AGENTS_TOTAL_CHARS {
            dropped += 1;
            continue;
        }
        total += n;
        kept.push(item);
    }
    kept.reverse(); // 浅 → 深
    if dropped > 0 {
        log::warn(format!(
            "项目记忆共 {} 份、超过总上限 {MAX_AGENTS_TOTAL_CHARS} 字符 —— 已丢掉最浅的 {dropped} 份（保留更具体的）",
            present.len()
        ));
    }
    let mut s = format!("\n\n{AGENTS_MD_HEADER}");
    for (path, text) in kept {
        s.push_str(&format!("\n\n### {path}\n{text}"));
    }
    s
}

/// 项目记忆固定段（读盘 + 拼装 + 落一行日志）。一份都没有就返回空串，绝不阻断启动。
///
/// 日志列出**这次用了哪几层**（可观测），路径是排障时「到底读了哪些文件」的凭据。
fn project_block(cwd: &std::path::Path) -> String {
    let files = collect_agents_md(cwd);
    let block = project_block_from_chain(&files);
    if !block.is_empty() {
        log::info(format!(
            "项目记忆已注入系统提示词固定段：{} 份、{} 字 [{}]",
            files.len(),
            block.chars().count(),
            files
                .iter()
                .map(|(p, _)| p.display().to_string())
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    block
}

// ── 子目录记忆：懒加载（2026-10-12，§待办 3-1a 的「子目录级」那一半）────────
//
// 语义（§3-1a 原话）：agent **实际触碰到某个子目录**时，才把**该子目录自己的记忆文件**
// （AGENTS.md）注入上下文；**没碰到的目录永远不注入**。固定前缀那层（向上递归）行为不变。
//
// 为什么只能懒加载、且**必须进可变层**：固定前缀一旦多一个字，整段缓存从第 0 个 token 起
// 全 miss（每加一个目录就废一次缓存，成本暴涨）——这正是当初没做的根因。所以本层的内容
// **只拼进「本轮刚产生、还没被发出去过」的 tool_result 文本内部**（同 `TOOL_BUDGET_HINT` /
// 后台输出回流那条落点纪律）：不动 `system`、不动消息条数、不动 content 块数量
// ⇒ 固定前缀逐字节不变，且改动只落在尾部新内容上（ai-spec 规则 23 的「零代价落点」）。
//
// 触达判据 / 忽略名单 / 深度上限见 `tools::path_arg_kind` / `tools::eligible_subdir`。
// 去重 = **每会话每个子目录只注一次**（判据取自 `history` 里的来源行，天然能扛 set_history
// 回退 —— 回退后该条随之消失，下次触碰到会补回，与 `context_block_message` 同一条纪律）。

/// 子目录记忆段的表头。英文原因同 AGENTS_MD_HEADER：给模型看的元信息。
/// 它同时是「本会话已注入几个子目录」的计数锚点（见 [`subdir_memory_count`]）。
const SUBDIR_MEMORY_HEADER: &str =
    "## Directory instructions (from AGENTS.md — loaded because you accessed this directory)";
/// 单份子目录 AGENTS.md 上限（字符）。理由同 MAX_AGENTS_MD_CHARS：懒加载同样进上下文。
const MAX_SUBDIR_MEMORY_CHARS: usize = 8_000;
/// 每会话最多懒加载几个子目录（防御「一路深挖、把可变层灌满」）。超过即不再注入、落一行 warn。
const MAX_SUBDIR_MEMORY_DIRS: usize = 16;

/// 把某个子目录的 AGENTS.md 拼成一段**注入文本**（读一个文件；空文件 / 读不出 ⇒ None）。
///
/// 形态与固定前缀那层的「多份」分支一致：`### <来源路径>` 加正文，便于模型判读来源。
fn subdir_memory_block(dir: &Path) -> Option<String> {
    let raw = read_agents_md(dir)?;
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    let text: String = if text.chars().count() > MAX_SUBDIR_MEMORY_CHARS {
        log::warn(format!(
            "子目录 AGENTS.md 超过 {MAX_SUBDIR_MEMORY_CHARS} 字符（{}）—— 已截断",
            dir.join(AGENTS_MD_FILE).display()
        ));
        text.chars().take(MAX_SUBDIR_MEMORY_CHARS).collect()
    } else {
        text.to_string()
    };
    Some(format!(
        "\n\n{SUBDIR_MEMORY_HEADER}\n### {}\n{text}",
        dir.join(AGENTS_MD_FILE).display()
    ))
}

/// 取出这条消息里所有「模型看得见的文本」（`text` 块 / `tool_result` 的文本）—— 纯文本判据用。
fn message_visible_texts(msg: &Value) -> Vec<&str> {
    msg.get("content")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .flat_map(|b| {
                    // text 块走 `text`；tool_result 的正文走 `content`（字符串形态）
                    [b.get("text").and_then(Value::as_str), b.get("content").and_then(Value::as_str)]
                        .into_iter()
                        .flatten()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 本会话是否**已经注入过**某个子目录的记忆（去重判据 = history 里含该目录的来源行）。
fn subdir_memory_injected(history: &[Value], dir: &Path) -> bool {
    let marker = format!("### {}", dir.join(AGENTS_MD_FILE).display());
    history
        .iter()
        .any(|m| message_visible_texts(m).iter().any(|t| t.contains(&marker)))
}

/// 本会话已注入的子目录**个数**（判据 = 表头出现次数），用于 `MAX_SUBDIR_MEMORY_DIRS` 上限。
fn subdir_memory_count(history: &[Value]) -> usize {
    history
        .iter()
        .map(|m| {
            message_visible_texts(m)
                .iter()
                .filter(|t| t.contains(SUBDIR_MEMORY_HEADER))
                .count()
        })
        .sum()
}

/// 把一段文本**拼进某条 `tool_result` 的文本内部**（不新增消息 / 不新增内容块，规则 23）。
/// `content` 不是字符串（如图片块数组）⇒ 不拼、返回 false。
fn append_text_to_tool_result(block: &mut Value, text: &str) -> bool {
    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
        return false;
    }
    let Some(prev) = block.get("content").and_then(Value::as_str) else {
        return false;
    };
    block["content"] = json!(format!("{prev}{text}"));
    true
}

/// 对本轮工具调用做**子目录记忆懒加载**：只对**真正触碰过**的合格子目录，把其 AGENTS.md
/// 拼进该调用自身的 `tool_result` 文本内部（不新增消息 / 块 ⇒ 固定前缀一字不动）。
///
/// 判据链：调用未被拒 → `tools::touched_dir` 取触达目录 → `tools::eligible_subdir` 过筛
/// （在 cwd 内 / 更深 / 不在忽略名单 / 不超深度）→ 本轮与 `history` 双重去重 → 会话目录数
/// 未达上限 → 文件非空。
fn inject_subdir_memory(
    cwd: &Path,
    history: &[Value],
    calls: &[(String, String, Value)],
    run_inputs: &[Value],
    denieds: &[Option<String>],
    results: &mut [Value],
) {
    let mut injected_this_round: Vec<PathBuf> = Vec::new();
    let mut count = subdir_memory_count(history);
    let mut capped = false;
    for (i, (_id, name, _input)) in calls.iter().enumerate() {
        // 被审批 / hook 拒掉的调用没真正触碰到任何东西 ⇒ 不注入
        if denieds.get(i).map_or(false, |d| d.is_some()) {
            continue;
        }
        let Some(input) = run_inputs.get(i) else {
            continue;
        };
        let Some(dir) = tools::touched_dir(cwd, name, input) else {
            continue;
        };
        if !tools::eligible_subdir(cwd, &dir) {
            continue;
        }
        if injected_this_round.contains(&dir) || subdir_memory_injected(history, &dir) {
            continue; // 同一目录每会话只注一次（本轮 + 历史双重去重）
        }
        if count >= MAX_SUBDIR_MEMORY_DIRS {
            capped = true;
            continue;
        }
        let Some(block) = subdir_memory_block(&dir) else {
            continue;
        };
        if append_text_to_tool_result(&mut results[i], &block) {
            log::info(format!(
                "子目录记忆已懒加载注入（进可变层、不碰固定前缀）：{}（{} 字）",
                dir.join(AGENTS_MD_FILE).display(),
                block.chars().count()
            ));
            injected_this_round.push(dir);
            count += 1;
        }
    }
    if capped {
        log::warn(format!(
            "子目录记忆已达每会话上限 {MAX_SUBDIR_MEMORY_DIRS} 个目录，本轮新触碰的未注入"
        ));
    }
}

/// 主系统提示词的装配 = 角色段 + 人格段 + 环境块 + 技能清单 —— **只在启动时调一次**
/// （`persona` 由 `read_user_persona()` 给出）。
///
/// 抽成函数的唯一理由：让「用户人格**只**进主提示词」这条不变量能被单测钉住
/// （见 `user_persona_reaches_only_the_main_prompt`）。
///
/// ⚠️ **刻意不含「长期记忆」**（2026-10-06 挪出，M2-16 方案 D）：它随记忆增长而变，留在
/// `system` 里会让**每次重启后** system 前缀整个变掉 ⇒ 端点侧缓存从第 0 个 token 起全
/// miss（实测一天里 system hash 变了 4 次，每次重启后首问命中率只有 11.7%–62%）。现在它
/// 作为**一条 `user` 消息**注在 history 开头（见 [`context_block_message`]），system 从此
/// **跨重启逐字节稳定**。
fn build_system_prompt(persona: &str, env: &str, skills: &str) -> String {
    format!(
        "{SYSTEM_PROMPT}\n\n{PERSONA_AND_STYLE}{}{env}{skills}",
        persona_block(persona)
    )
}

/// 上下文块（长期记忆）的表头。
///
/// 它同时是**幂等注入的判据**（见 [`is_context_block_msg`]）：`set_history`（回退 / 恢复
/// 会话）会整份换掉 history，注过的那条随之消失 —— 下一问必须能认出「没有它」并补回去，
/// 否则表现为「回退之后模型突然不记得长期记忆」。
const CONTEXT_BLOCK_HEADER: &str = "# Background context (fixed for this session)";

/// 把「长期记忆」拼成**一条 `user` 消息**（空 ⇒ `None`）。
///
/// **2026-10-07**：往期会话索引**不再进这里**（原先与记忆同块）—— 新建对话必须与上个对话
/// 在模型侧隔离。要回忆往期会话，模型自己调 `SessionSearch`。
///
/// **为什么是 `user` 消息、且注在 history 开头**（2026-10-06，M2-16 方案 D，三个位置逐一体检）：
///   · 留在 `system` ⇒ 重启后 system 变 ⇒ 端点缓存从 0 起全 miss（**这正是要修的**）；
///   · 注在**消息尾** ⇒ 踩 `TOOL_BUDGET_HINT` 里那条**实测红线**：动到前缀**末尾**的块结构
///     会让端点缓存单元整体失配（`read` 从 11776 掉到 2560，2026-09-20 实测、不得回退）；
///   · 注在**开头** ⇒ 只动一次、之后结构恒定，不碰那条红线，且换来 system 稳定。
///
/// **代价（如实记）**：模型把它当**用户说的话**看，不再有「系统级背景」的权威感 ——
/// 段内措辞已按「背景，不属于当前请求」写好，但仍不如 system 里硬。
fn context_block_message(memory_block: &str) -> Option<Value> {
    let m = memory_block.trim();
    if m.is_empty() {
        return None;
    }
    let text = format!("{CONTEXT_BLOCK_HEADER}\n{m}");
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

/// 上下文块的字符数（只用于日志 / 取证；拿不到就 0）。
fn context_block_chars(block: &Value) -> usize {
    block
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .map(|t| t.chars().count())
        .unwrap_or(0)
}

/// **压缩边界刷新长期记忆快照**（`todo.md` §待办 3-9 C，2026-10-23）。
///
/// **为什么这时允许重读**：规则 56 禁止的是「写完记忆就刷新」「每轮重读一次记忆文件」——
/// 那会让每一轮都 cache-miss。压缩边界不同：`compact_history` 已经就地改写了 history
/// （丢整条 / 瘦 tool_result），前缀从改动点起本来就作废；此刻重读一次记忆、把开头那条
/// 上下文块换成最新内容，**不额外废掉任何还活着的缓存**，却能让「会话中途写的记忆」
/// 不必重启 agent 就可见（规则 56 原先明写的副作用）。
///
/// **仍然禁止**：每轮重读、`Remember` 写成功后立刻刷新、把块挪回 `system`（`forbidden.md` §禁用 45）。
///
/// `snapshot` = 进程内那份上下文块（全局轮跨提问持有；刷新结果写回它，下一轮才不退回旧内容）。
/// 返回是否真的换了内容（调用方只用来判断要不要多说一句话）。
fn refresh_memory_snapshot(
    history: &mut [Value],
    snapshot: &mut Option<Value>,
    bridge: Option<&mut mcp::McpSet>,
    remember_tool_on: bool,
    pet_growth_on: bool,
    scope: tools::MemoryScope,
) -> bool {
    // 本来就没有块（记忆被关 / 桥不在 / 连引导都不可得）⇒ 不凭空造一个：那等于改注入策略。
    if snapshot.is_none() {
        return false;
    }
    let mut bridge = bridge;
    let fresh = match scope {
        tools::MemoryScope::Global => {
            context_block_message(&memory_block(bridge.as_deref_mut(), remember_tool_on, scope))
        }
        // 与主循环的桌宠轮同款：私有记忆段 + 成长笔记段拼在同一个块里。
        tools::MemoryScope::Pet => {
            let section = memory_block(bridge.as_deref_mut(), remember_tool_on, scope);
            let growth = growth_block(bridge.as_deref_mut(), pet_growth_on);
            context_block_message(&format!("{section}{growth}"))
        }
    };
    apply_refreshed_snapshot(history, snapshot, fresh)
}

/// 把「刷新后的块」落到进程内快照与 history 开头（不联网、只碰入参，便于单测）。
///
/// 三种情况：
///   · `fresh` 为空（记忆被清空 / 桥临时不可用）⇒ **保留旧块**：删掉它会把 history 的第一条
///     换掉、还会让本会话少一份背景；旧块虽旧，仍是这个进程已经注入过的事实（冻结快照的语义）。
///   · `fresh` 与旧块逐字节相同 ⇒ 什么都不做（不制造无意义的前缀变更）。
///   · 不同 ⇒ 写回 `snapshot`；history 第一条若还是上下文块就**原地替换**（长度不变 ⇒
///     回滚锚点 `base` 不受影响）。第一条已被 `set_history` 换掉时只更新快照 ——
///     下一轮 `run_query` 的幂等注入会把它放到开头。
fn apply_refreshed_snapshot(
    history: &mut [Value],
    snapshot: &mut Option<Value>,
    fresh: Option<Value>,
) -> bool {
    let Some(fresh) = fresh else { return false };
    let Some(old) = snapshot.as_ref() else { return false };
    if *old == fresh {
        return false;
    }
    let old_chars = context_block_chars(old);
    *snapshot = Some(fresh.clone());
    if let Some(first) = history.first_mut() {
        if is_context_block_msg(first) {
            *first = fresh.clone();
        }
    }
    log::info(format!(
        "压缩边界：长期记忆快照已刷新（{old_chars} → {} 字，可变层原地替换）",
        context_block_chars(&fresh)
    ));
    true
}

/// 子代理 / 复盘的系统提示词（角色段 + 环境块 + 技能清单）：**刻意不含用户人格**。
///
/// 它们是内部产物（子代理报告 / 复盘写记忆），用户的文风口吻在那里没有意义；而多一份文本
/// 它们是内部产物（子代理报告 / 复盘写记忆），用户的文风口吻在那里没有意义；而多一份文本
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
    // 模型不会凭空知道 Lunac 的插件放在哪、该照什么格式写 —— 不写这段，
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
             - If a plugin needs a toolchain / SDK / build step, declare it in the manifest's \
             `dev` section and ask the user to approve it in 设置 → 插件 → 开发工具链. Never \
             download toolchains or run build commands on your own.\n\
             - The user sees new plugins after they open 设置 → 插件 → 重新扫描 (no restart needed).\n\
             ",
            modules_dir,
            std::path::Path::new(&modules_dir).join("README.md").display()
        )
    };
    // 「东西该放哪」的四分法（2026-10-07）。**必须写死这块**：模型做 PS 插件时把整个项目
    // 直接建在了 `skills\` 下 —— 它只看到「有这么个目录、能写」，就顺手用了。
    // 四类目录各有唯一用途、宿主都会扫；放错 = 要么不生效，要么污染能力清单。
    // tools\ 与 skills\ 同根（`<exe 根>\tools`），从 skills_dir 的父目录推出来，不另开环境变量。
    let tools_dir = if skills_dir == "(not configured)" {
        "(not configured)".to_string()
    } else {
        std::path::Path::new(&skills_dir)
            .parent()
            .map(|p| p.join("tools").display().to_string())
            .unwrap_or_else(|| "(not configured)".into())
    };
    let placement_line = format!(
        "- Where to put what you create — this app has FOUR directories, each with ONE purpose; \
         never mix them and never invent a fifth:\n\
         ○ Lunac plugin (a UI module for this app): {modules_dir}\n\
         ○ AI skill (an instruction folder FOR YOU: a folder containing SKILL.md): {skills_dir}\n\
         ○ MCP tool (a tool definition this app exposes to you as `mcp__*`): {tools_dir}\n\
         ○ Anything else — standalone project / tool / script / experiment that is NOT one of the \
         three above (e.g. a Photoshop CEP panel, a Python generator, a helper CLI): create it \
         under the working directory above (or a folder under it). Do NOT put it in `skills\\`, \
         `Modules\\` or `tools\\`.\n\
         Rule of thumb: unless it is (a) loaded by Lunac's search panel, (b) an instruction file \
         for you, or (c) a tool definition for you to call, it does not belong in those three.\n"
    );
    format!(
        "\n\nEnvironment:\n\
         - Host: Lunac, a Windows desktop launcher. You are Lunac's built-in agent — not a \
         component of any other agent framework, and not running inside one.\n\
         - Working directory (absolute): {}\n\
         - Lunac's own skills live in: {} — each skill is a folder containing SKILL.md. When the \
         user says \"my skills\", \"我自己的 skills\" or similar, they mean the skills of this app \
         (the ones listed below, if any) or this directory.\n\
         {}\
         {}\
         - Other files on disk are ordinary files. If the workspace happens to contain another \
         agent/tool framework's repository, config or skills, do not treat it as Lunac's setup \
         and do not answer as if you were that product.",
        cwd.display(),
        skills_dir,
        modules_line,
        placement_line
    )
}

// ── 长期记忆（A4，2026-09-20）────────────────────────────────────
//
// **注意**：往期会话索引（A2）自 2026-10-07 起**不再注入** —— 新建对话必须与上个对话在
// 模型侧隔离（每次重启都会把刚结束的那个会话列进索。思考里带上个对话的痕迹，用户报。
// 就是这个）。取回侧保留：要回忆往期会话，模型自己调 `SessionSearch`（`lunac/history_search`）。
//
// 长期记忆 = 从流水里**提炼出来的少量结论**（用户偏好 / 项目约定 / 踩过的坑），整段注入
// （服务端口径：正文额度 6000 字符 + 总长硬顶 9000 字符）。结论是每轮都可能用到的背景
// （现查既慢又贵），所以注入。
//
// 写入侧（`Remember` 工具 + 每 N 轮的后台复盘 fork）见 `main()` 的装配与 `run_review_fork`。
//
// 冻结快照纪律（规则 18 / 53）：**只在启动时读一次**。会话中新写的
// 记忆**只落盘**，本次进程的系统提示词逐字节不变；下次重启 agent 才可见。
// 反面做法（「保存记忆后刷新提示词」）会让每一轮都 cache-miss，代价远大于这点信息量。

/// 开关：`0` / `false` / `off` / `no` 关闭（与 `LUNAC_SUMMARY_COMPACT` 同写法）。**默认开**。
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

/// 注入模式（A/B 实验开关）：`LUNAC_MEMORY_MODE=full|index`。
///
/// · `full`（默认）：整份记忆正文注入（24k 字上限），行为与改造前逐字节一致。
/// · `index`：只注入「页名 + 每条首行摘要」的目录（`MEMORY_INDEX_CHARS` 硬上限），
///   并提示模型在依赖记忆前调用 `MemorySearch` 读命中条目。
///
/// 只影响**注入**，不影响写入 / 检索 / 复盘：两种模式读的是同一份 `MEMORY.md`，
/// 所以同一份数据可以直接对比上下文体积与答对率。非法值按 `full` 处理（不因笔误
/// 悄悄改变线上行为）。
const MEMORY_MODE_ENV: &str = "LUNAC_MEMORY_MODE";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemoryMode {
    Full,
    Index,
}

impl MemoryMode {
    fn as_str(self) -> &'static str {
        match self {
            MemoryMode::Full => "full",
            MemoryMode::Index => "index",
        }
    }
}

fn parse_memory_mode(raw: Option<&str>) -> MemoryMode {
    match raw.map(str::trim) {
        Some(value) if value.eq_ignore_ascii_case("index") => MemoryMode::Index,
        _ => MemoryMode::Full,
    }
}

fn memory_mode() -> MemoryMode {
    parse_memory_mode(std::env::var(MEMORY_MODE_ENV).ok().as_deref())
}

/// index 目录总长硬上限（远小于 `MEMORY_INJECT_CHARS`）：目录只负责「告诉模型有什么」，
/// 具体条目由 `MemorySearch` 取。
const MEMORY_INDEX_CHARS: usize = 2_000;
/// 目录里每条只保留首行前这么多字符。
const MEMORY_INDEX_ENTRY_CHARS: usize = 80;

/// 把完整记忆正文压成目录：`## 页名` + 每条 bullet 的首行摘要。
///
/// 输入形态见 `ModuleData/memory`（`MEMORY.md` + `memory/pages/<page>.md`，读取时拼成
/// `## page` 小节）。这里只做**展示压缩**：不改条目、不跨页合并、不猜语义；超过
/// `MEMORY_INDEX_CHARS` 就停止并标注还有多少条未展开。返回空串 = 没有可索引的 bullet
/// （调用方会退回整段注入，避免老格式记忆被目录模式吞掉）。
fn memory_index_text(body: &str) -> String {
    let total = body
        .lines()
        .filter(|line| {
            line.strip_prefix("- ")
                .is_some_and(|rest| !rest.trim().is_empty())
        })
        .count();
    if total == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut current_page = String::new();
    let mut emitted_page = String::new();
    let mut shown = 0usize;
    let mut truncated = false;
    for line in body.lines() {
        if let Some(page) = line.strip_prefix("## ") {
            current_page = page.trim().to_string();
            continue;
        }
        if line.starts_with("→ refs:") {
            continue;
        }
        let Some(entry) = line.strip_prefix("- ") else {
            continue;
        };
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if truncated {
            continue;
        }
        let mut piece = String::new();
        if current_page != emitted_page && !current_page.is_empty() {
            piece.push_str(&format!("## {current_page}\n"));
        }
        piece.push_str("- ");
        let mut chars = entry.chars();
        let preview: String = chars.by_ref().take(MEMORY_INDEX_ENTRY_CHARS).collect();
        piece.push_str(&preview);
        if chars.next().is_some() {
            piece.push('…');
        }
        piece.push('\n');
        if out.chars().count() + piece.chars().count() > MEMORY_INDEX_CHARS {
            truncated = true;
            continue;
        }
        out.push_str(&piece);
        emitted_page = current_page.clone();
        shown += 1;
    }
    if shown < total {
        out.push_str(&format!(
            "- … ({} more entries are not shown here; use `MemorySearch` with a keyword to read them)\n",
            total - shown
        ));
    }
    out
}

fn log_memory_mode_once(mode: MemoryMode) {
    static LOGGED: OnceLock<()> = OnceLock::new();
    if LOGGED.set(()).is_ok() {
        crate::log::info(format!("记忆注入模式：{}（{MEMORY_MODE_ENV}）", mode.as_str()));
    }
}

/// 注入侧长度上限。与服务端 `memory.rs::MAX_TOTAL_CHARS` 同值（2026-11-20 额度调高：
/// 9000 → 24000 字符；正文额度 16000、单条 4000）。服务端按「正文额度 + 总长硬顶」管「写」，
/// 这里按同一个**总长硬顶**管「发」⇒ 服务端允许落盘的文件能**整份**注入，不会在注入时
/// 被二次截断；用户手工把文件改大时，兜底截断仍然由这一层负责。
// 2026-11-20 用户要求额度调高：与服务端 `memory.rs` 的 `MAX_TOTAL_CHARS` 保持同值
// （9000 → 24000）。服务端允许落盘多少，这里就整份注入多少；按需检索仍走 `MemorySearch`。
const MEMORY_INJECT_CHARS: usize = 24_000;

/// 按注入上限截断（**唯一的注入封顶实现**，记忆与成长笔记共用）：超 `MEMORY_INJECT_CHARS`
/// 就截到头。抽成纯函数是为了可单测（§65：往 history 里塞东西的每条路径都要有硬上限）。
fn cap_inject(text: &str) -> String {
    if text.chars().count() > MEMORY_INJECT_CHARS {
        crate::log::truncate_chars(text, MEMORY_INJECT_CHARS)
    } else {
        text.to_string()
    }
}

/// 取当前长期记忆文本；没有 / 空 / 出错都返回 `None`（**不报错** —— 记忆是锦上添花）。
///
/// `scope` = 读哪一份（全局 / 桌宠私有）。两个调用方：启动 / 按轮注入（`memory_block`）
/// 与每次复盘前的「当前记忆」（放进复盘的提示词，好让它不重复写已有条目）。
fn fetch_memory(bridge: Option<&mut mcp::McpSet>, scope: tools::MemoryScope) -> Option<String> {
    let b = bridge?;
    match b.memory_read(scope.as_str()) {
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

/// 取某一作用域的「记忆注入段」（带前导空行，拼进注入块正文）。
///
/// `remember_tool_on` = `Remember` 真的在工具池里。**必须传**：段里写着「用 `Remember`
/// 工具追加」，工具不在还这么写，等于指挥模型去调一个不存在的工具（与往期会话索引遇
/// `SessionSearch` 被禁就不注入是同一条纪律）。
///
/// `scope` 决定读哪一份（全局 / 桌宠私有）：两份各一个文件、各一段，**互不掺入**。
///
/// 拿不到就返回**空串**，任何情况都不报错、不阻断启动。
fn memory_block(
    bridge: Option<&mut mcp::McpSet>,
    remember_tool_on: bool,
    scope: tools::MemoryScope,
) -> String {
    if !memory_enabled() {
        eprintln!("[agent] 长期记忆已关闭（{MEMORY_ENV}=0）");
        return String::new();
    }
    let mode = memory_mode();
    log_memory_mode_once(mode);
    // 记忆正文（可能为空）。*空记忆不再直接返回空。*：改造前那样写，全新安装时提示词
    // 里完全没有 Remember 引导，模型不知道可以主动写、而唯一能播种的复盘又没触发 ⇒ 冷启动
    // 死锁（2026-10-11 修，方案 D）。改为「正文可空、引导照发」。
    //
    // 超限时保留**较早**的部分（truncate_chars 留头部）：较早的条目是更 established 的约定，
    // 尾部的新条目丢掉时至少下次还能再写一次。正常路径到不了这里（服务端写入已按同值封顶），
    // 尾部的新条目丢掉时至少下次还能再写一次。正常路径到不了这里（服务端写入已按同值封顶），
    let raw = fetch_memory(bridge, scope).unwrap_or_default();
    // 老格式 / 没有 bullet 时目录会空：退回整段注入，**连说明文案也退回 full**，
    // 否则会出现「下面只是目录」但实际贴了整份正文的自相矛盾提示。
    let (text_mode, body) = match mode {
        MemoryMode::Full => (MemoryMode::Full, cap_inject(&raw)),
        MemoryMode::Index => {
            let index = memory_index_text(&raw);
            if index.is_empty() && !raw.trim().is_empty() {
                eprintln!("[agent] 记忆目录为空（无 bullet），本次退回整段注入");
                (MemoryMode::Full, cap_inject(&raw))
            } else {
                (MemoryMode::Index, index)
            }
        }
    };
    let block = memory_block_text_mode(text_mode, remember_tool_on, &body, scope);
    if block.is_empty() {
        return block;
    }
    // 全局那两条日志文案**逐字节不变**（e2e / 日志检索都按它认冷启动）；桌宠用同构文案。
    let label = match scope {
        tools::MemoryScope::Global => "长期记忆",
        tools::MemoryScope::Pet => "桌宠私有记忆",
    };
    if body.is_empty() {
        eprintln!("[agent] {label}为空，已注入 Remember 引导（冷启动）");
    } else {
        eprintln!("[agent] {label}已注入（{} 字）", body.chars().count());
    }
    block
}

/// 组装记忆注入段（纯函数：只依赖「写侧引导是否可用」「记忆正文」「作用域」，便于单测）。
///
/// 缓存纪律（2026-10-11 方案 D 明确）：表头与引导句是**固定前缀**，记忆正文永远拼在它们
/// 之后（顺序固定为 表头 + 引导 + 空行 + 正文）。于是「空记忆 → 有记忆」时，块首到引导末尾
/// 逐字节不变 —— 端点按最长公共前缀命中，只有正文那一段 miss，而不是从块首整段作废。
/// 反面做法（正文插到引导之前 / 引导里回显正文）会让**每写一次记忆就打掉一次缓存**。
///
/// 表头按 `scope` 区分（全局 / 桌宠各一段）：全局措辞**逐字节不变**（缓存不变量）。
///
/// 空记忆 + 写侧不可用 ⇒ 空串：没有正文可注、也没有可写的工具，拿一句「记忆是空的」去占
#[cfg(test)]
fn memory_block_text(remember_tool_on: bool, body: &str, scope: tools::MemoryScope) -> String {
    memory_block_text_mode(MemoryMode::Full, remember_tool_on, body, scope)
}

/// 与 `memory_block_text` 相同，但按注入模式决定「如何说明这份正文」。
///
/// `full` 分支必须与改造前**逐字节一致**（缓存不变量）；`index` 只在 provenance 与 tail 之间
/// 多一行固定说明（模式在进程启动时固定，不会逐轮改写前缀）。
fn memory_block_text_mode(
    mode: MemoryMode,
    remember_tool_on: bool,
    body: &str,
    scope: tools::MemoryScope,
) -> String {
    if body.is_empty() && !remember_tool_on {
        return String::new();
    }
    let tail = if remember_tool_on {
        "- Add a fact with the `Remember` tool (one concise fact per call). Do not use it for \
         one-off details or for the current task's progress."
    } else {
        "- (The `Remember` tool is disabled in this session, so this memory can only be \
         extended outside this app.)"
    };
    let header = match scope {
        tools::MemoryScope::Global => "# Long-term memory",
        tools::MemoryScope::Pet => "# Long-term memory (desktop pet — private notes)",
    };
    // 来源 / 时间说明行（B2 于 2026-10-13 引入；B1 于 2026-10-17 改为描述逐条前缀）：
    // **固定前缀的一部分**（逐字节稳定、随 scope 不同），说明新格式与日期语义 ——
    // `- [YYYY-MM-DD · scope]` 的日期 = 该条事实的**最后确认时间**（不是初次得知）；
    // 无前缀 = 更早的、未记录日期的背景。不掺任何运行时变量（缓存前缀纪律）。
    let provenance = match scope {
        tools::MemoryScope::Global => "- Entries saved by the main assistant may start with \
            [YYYY-MM-DD · global]: the date is when the fact was last confirmed, not when it was \
            first learned. An entry without that prefix is undated background recorded earlier.",
        tools::MemoryScope::Pet => "- Entries in the desktop pet's own private memory may start \
            with [YYYY-MM-DD · pet]: the date is when the note was last confirmed, not when it \
            was first learned. An entry without that prefix is undated background recorded earlier.",
    };
    let index_note = match mode {
        MemoryMode::Full => "",
        MemoryMode::Index => "- The entries below are only an INDEX (page names plus shortened \
            first lines), not the full memory. Before relying on a remembered fact, call the \
            `MemorySearch` tool with a keyword or phrase to read the matching full entries.\n",
    };
    format!(
        "\n\n{header}\n\
         Durable facts saved in earlier sessions. Treat them as background you already know; \
         they are not part of the current request.\n\
         {provenance}\n\
         {index_note}{tail}\n\n{body}",
    )
}

/// 取桌宠成长笔记文本（人格地基）；没有 / 空 / 出错都返回 `None`（**不报错**）。
///
/// 与 `fetch_memory` 同理：成长为桌宠自我笔记，读不到就当没有，绝不阻断桌宠轮。
fn fetch_growth(bridge: Option<&mut mcp::McpSet>) -> Option<String> {
    let b = bridge?;
    match b.pet_growth_read() {
        Ok(t) => {
            let t = t.trim();
            if t.is_empty() || t == "(pet growth notes are empty)" {
                None
            } else {
                Some(t.to_string())
            }
        }
        Err(e) => {
            eprintln!("[agent] 桌宠成长笔记读取失败（当作没有）: {e}");
            None
        }
    }
}

/// 取桌宠成长笔记的注入段（带前导空行；拿不到 ⇒ 空串，不报错、不阻断）。
///
/// `pet_growth_on` = `PetGrowth` 真的在工具池里（段里写着「用 `PetGrowth` 追加」，工具不在还
/// 这么写，等于指挥模型去调一个不存在的工具）。注入侧上限**复用 `cap_inject`**
/// （= `MEMORY_INJECT_CHARS`，同一份真相源，§62 不新增第三份常量）。
fn growth_block(bridge: Option<&mut mcp::McpSet>, pet_growth_on: bool) -> String {
    let body = fetch_growth(bridge).map(|text| cap_inject(&text)).unwrap_or_default();
    let block = growth_block_text(pet_growth_on, &body);
    if !block.is_empty() {
        if body.is_empty() {
            eprintln!("[agent] 桌宠成长笔记为空，已注入 PetGrowth 引导（冷启动）");
        } else {
            eprintln!("[agent] 桌宠成长笔记已注入（{} 字）", body.chars().count());
        }
    }
    block
}

/// 组装成长笔记注入段（纯函数：只依赖「写侧是否可用」与正文，便于单测）。
///
/// 缓存纪律与 `memory_block_text` 同款：表头 + 引导是**固定前缀**，正文永远拼在其后 ——
/// 「空 → 有」时只有正文那一段 miss，不从块首整段作废。成长笔记只在**桌宠轮**注入
/// （见提问循环里 `turn_block` 的 Pet 分支），与全局记忆块拼在同一个上下文块里。
fn growth_block_text(pet_growth_on: bool, body: &str) -> String {
    if body.is_empty() && !pet_growth_on {
        return String::new();
    }
    let tail = if pet_growth_on {
        "- Add one with the `PetGrowth` tool (one concise note per call)."
    } else {
        "- (The `PetGrowth` tool is disabled in this session, so these notes can only be \
           extended outside this app.)"
    };
    format!(
        "\n\n# Growth notes (desktop pet — how you've chosen to behave)\n\
         Notes you wrote in earlier desktop-pet sessions about your own manner and habits. \
         Treat them as your own established style; they are not part of the current request.\n\
         {tail}\n\n{body}",
    )
}

/// 「桌宠发起的那一轮」在 user 消息里的来源标记（前端 `currentTurnFromPet` ⇒ `source:"pet"`，
/// 见 `app/src/main.ts` 的发送点）。与 `thinking:"off"` 同款：**只认这一个值**，其余（含缺省）
/// 一律全局 —— 识别不出就退回全局，绝不因为一个坏字段把一轮变成桌宠轮。
const TURN_SOURCE_PET: &str = "pet";

/// 由本轮 user 消息的 `source` 推出记忆作用域（缺省 / 其它值 = 全局）。
fn turn_memory_scope(msg: &Value) -> tools::MemoryScope {
    match msg.get("source").and_then(Value::as_str) {
        Some(v) if v.eq_ignore_ascii_case(TURN_SOURCE_PET) => tools::MemoryScope::Pet,
        _ => tools::MemoryScope::Global,
    }
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
/// 只有明确的「关」才算关，其余（含未设置 / 值不认识）一律当开 ——
fn thinking_from(value: Option<&str>) -> Thinking {
    match value.map(str::trim) {
        Some("off") | Some("0") | Some("false") => Thinking::Disabled,
        _ => Thinking::Budget(THINKING_BUDGET),
    }
}

/// 「按轮思考覆盖」（2026-10-09）：用户消息里带 `"thinking":"off"` ⇒ 该轮**强制关** thinking，
/// 与全局开关（`LUNAC_THINKING` / `cfg.thinking`）无关。**只认 `off`** —— 其余值（含 `on`）
/// 一律返回 `None` = 不覆盖、用全局档。目的只有一个：让桌宠那一轮不烧推理 token。
fn thinking_override_from(value: Option<&str>) -> Option<Thinking> {
    match value.map(str::trim) {
        Some("off") | Some("0") | Some("false") => Some(Thinking::Disabled),
        _ => None,
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

/// 思考档在 `budget_tokens` 之外留给**答复本体**的余量。
///
/// `budget_tokens` 只约束「想多久」；**想的 token 与答复 / 工具参数的 token 共用
/// `max_tokens` 这一个池子**，所以 `budget + 4096` 等于把答复挤到 4096 以内。
/// 2026-10-07 的实测把这条钉死了：当天 `usage-2026-10-07.jsonl` 里 **2 次请求的输出正好
/// 落到 12288**（= 8192 预算 + 4096），即**撞上上限被截断** —— 一次没写完，模型得重来，
/// 是纯浪费（写出去的半截照样按输出计费）。端点侧上限是 384K，余量给到 16384 仍远低于它。
const THINKING_HEADROOM_TOKENS: u32 = 16_384;

/// 端点要求 `budget_tokens < max_tokens`，故开档必须抬高 max_tokens，
/// 否则 8192 的预算配 8192 的上限会被判参数非法。
///
/// ⚠️ 这个值**只是上限、不是目标** —— 抬高它不直接增加花费（计费按实际生成量），
/// 只是不再把「思考 + 答复」挤断。关档路径仍用 `BASE_MAX_TOKENS`：那条路径上没有思考
/// 抢池子，收紧上限正是「答案本分」纪律的一部分。
fn max_tokens_for(plan: Thinking) -> u32 {
    match plan {
        Thinking::Budget(n) => (n + THINKING_HEADROOM_TOKENS).max(BASE_MAX_TOKENS),
        _ => BASE_MAX_TOKENS,
    }
}

/// 判断 400 是否与思考参数有关 —— 只有这类才值得降级重试，
/// 判断 400 是否与思考参数有关 —— 只有这类才值得降级重试，
fn thinking_related_error(detail: &str) -> bool {
    let d = detail.to_ascii_lowercase();
    d.contains("thinking") || d.contains("adaptive") || d.contains("budget_tokens")
}

// ── 提示缓存断点。026-11-15）────────────────────────────────────────
//
// 依据：Anthropic 的提示缓存是「按前缀**字节**命中」，断点。
// `cache_control: {"type":"ephemeral"}` 显式标；。**tools 排在 system 之前**。
// 所以最后一。tool 上的断点会把 system + 全部 tools 一起缓存下来。
// 现状（改之前）：`system` 发的是字符串，全仓没有任。`cache_control` 。换到 Anthropic
// 原生端点。system + tools（约 2.5k。k token）每轮全价。
//
// 兼容性：DeepSeek 。Anthropic 兼容端点**会忽。* `cache_control`（官方文。
// `api-docs.deepseek.com/guides/anthropic_api` 与第三方接入指南都写明），它的缓存是
// 自动的，所以默认开启对我们现在的端点是零风险；万一某个端点直接 400，`run_query`
// 会摘掉断点重试一次（。400 分支），不会把用户卡死。
/// 是否给请求加缓存断点：`LUNAC_PROMPT_CACHE=0|off|false` 关闭，缺省开启。
fn prompt_cache_enabled() -> bool {
    prompt_cache_from(std::env::var(PROMPT_CACHE_ENV).ok().as_deref())
}

const PROMPT_CACHE_ENV: &str = "LUNAC_PROMPT_CACHE";

/// 环境变量解析（纯函数，便于单测）。
fn prompt_cache_from(raw: Option<&str>) -> bool {
    match raw.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if v == "0" || v == "off" || v == "false" || v == "no" => false,
        _ => true,
    }
}

/// 请求体的 `system` 字段：开启断点时。*块数。*（Anthropic 只认块上。cache_control），
/// 关闭时保持原来的字符串形态（字节级与前缀指纹完全一致）。
fn system_value(system_prompt: &str, cache: bool) -> Value {
    if !cache {
        return json!(system_prompt);
    }
    json!([{
        "type": "text",
        "text": system_prompt,
        "cache_control": { "type": "ephemeral" },
    }])
}

/// 请求体的 `tools` 字段：开启断点时。`cache_control` 打在**最后一。*工具。
/// （tools 。system 之前，这个断点把 system + 全部 tools 一起纳入缓存）。
fn tools_value(defs: &[Value], cache: bool) -> Value {
    if !cache {
        return json!(defs);
    }
    let mut list = defs.to_vec();
    if let Some(last) = list.last_mut() {
        if let Some(obj) = last.as_object_mut() {
            obj.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
        }
    }
    json!(list)
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

/// 子代理 **或** 后台复盘的累计用量（跨线程共享，2026-09-29）。
///
/// 2026-10-27（§待办 3-16 P2-8）起**一个实例只服务一类**：`Cfg.sub` 记子代理
/// （`Agent` / fork 技能）、`Cfg.review` 记后台复盘。两者形状相同，只是账本分开 ——
/// 「钱花在探索还是收口上」必须能分开回答。
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
        // 自己的请求，而子代理 / 复盘烧的钱一样会被「回合被中断」吞掉。前端只累计、不分账，
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
    /// 订阅式网关的附加请求头（占位符已替换，见 [`resolve_extra_headers`]）：
    /// `model_post` 逐条加上。空 Vec = 行为与改造前逐字节一致。
    extra_headers: Vec<(String, String)>,
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
    ///
    /// 2026-10-27（§待办 3-16 P2-8）起拆成两本：`sub` = 子代理（`Agent` / fork 技能），
    /// `review` = 后台复盘。拆开才能回答「钱花在探索上还是收口上」。
    sub: Arc<SubagentUsage>,
    review: Arc<SubagentUsage>,
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

        let extra_headers = resolve_extra_headers(&model);

        Ok(Self {
            client,
            endpoint: format!("{}/v1/messages", base.trim_end_matches('/')),
            token,
            extra_headers,
            model,
            thinking: Cell::new(Thinking::from_env()),
            last_input: Cell::new(0),
            last_compact: Cell::new(0),
            sub: Arc::new(SubagentUsage::default()),
            review: Arc::new(SubagentUsage::default()),
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
            extra_headers: self.extra_headers.clone(),
            model: self.model.clone(),
            thinking: Cell::new(self.thinking.get()),
            last_input: Cell::new(0),
            last_compact: Cell::new(0),
            sub: Arc::clone(&self.sub),
            review: Arc::clone(&self.review),
        })
    }
}

// ── 订阅式网关的附加请求头（2026-11-21）─────────────────────────────
//
// 为什么要有这一层：主请求只发 `authorization` / `anthropic-version` /
// `content-type` 三个头（见四处 `model_post` 调用点），而有些**订阅制**网关在自家的
// Anthropic 兼容接口上还要求自己的会话头 —— opencode 的 Go 版缺 `x-opencode-session`
// 直接回 `MissingSessionID`。与其为每家供应商在源码里加分支（加一家就得重编一次），
// 这里开一个配置化的通用出口：
//
// ```json
// { "extra_headers": { "x-opencode-session": "${session}" } }
// ```
//
// 值里可用占位符：`${session}` = 本进程会话 id（`session_id()`），`${model}` = 当前
// 模型名，`${env:NAME}` = 同名环境变量（未设置 ⇒ 整条头丢弃）。头名大小写不敏感，
// 保留头（见 [`RESERVED_HEADERS`]）跳过并告警。

/// 请求构造处自己决定的头，配置层不该覆盖它们。
///
/// `authorization` / `anthropic-version` / `content-type` 与请求体强绑定，`accept`
/// 决定流式与否（`text/event-stream`）—— 让配置文件改它们等于给「请求长什么样」开
/// 第二个真相来源，出问题时两处都要怀疑。
const RESERVED_HEADERS: [&str; 4] =
    ["authorization", "anthropic-version", "content-type", "accept"];

/// 解析一份「头名 → 头值」JSON 对象（纯函数，单测不碰 env）。
///
/// 值接受 string / number / bool（后两者按 `to_string` 处理）；`null`、空串、空头名
/// 整条跳过 —— 一个空的头值只会让服务端 400，不如当成「没配」。
fn parse_extra_headers(raw: &str, source: &str) -> Result<Vec<(String, String)>, String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|e| format!("{source} 不是合法 JSON：{e}"))?;
    let Some(object) = value.as_object() else {
        return Err(format!("{source} 必须是 JSON 对象（头名 → 头值）"));
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, value) in object {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if RESERVED_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
            log::warn(format!("{source}：{name} 由请求构造处决定，已忽略这条配置"));
            continue;
        }
        let text = match value {
            Value::String(s) => s.trim().to_string(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => {
                log::warn(format!("{source}：{name} 的值不是字符串 / 数字 / 布尔，已忽略"));
                continue;
            }
        };
        if text.is_empty() {
            log::warn(format!("{source}：{name} 的值为空，已忽略"));
            continue;
        }
        out.push((name.to_string(), text));
    }
    Ok(out)
}

/// 把占位符替换成真值：`${session}` / `${model}` / `${env:NAME}`。
///
/// 认不出的占位符**原样保留**（多半是用户打错，留在头值里比悄悄吞掉更容易看出来）；
/// 但 `${env:NAME}` 指向的环境变量不存在或为空 ⇒ 返回 `None`，调用方据此**整条丢弃**
/// 这个头：往请求里塞字面量 `${env:OPENCODE_KEY}` 只会换来 401，不如不带。
fn substitute_placeholders(value: &str, session: &str, model: &str) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    let mut dropped = false;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            // 没有闭合 ⇒ 后面的部分当纯文本（不猜用户想写什么）。
            out.push_str(&rest[start..]);
            return if dropped { None } else { Some(out) };
        };
        let name = &after[..end];
        match name {
            "session" => out.push_str(session),
            "model" => out.push_str(model),
            other if other.starts_with("env:") => {
                let key = other[4..].trim();
                match std::env::var(key) {
                    Ok(v) if !v.trim().is_empty() => out.push_str(v.trim()),
                    _ => dropped = true,
                }
            }
            other => {
                out.push_str("${");
                out.push_str(other);
                out.push('}');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    if dropped {
        None
    } else {
        Some(out)
    }
}

/// 附加请求头的**原始**配置（占位符未替换），进程内只解析一次。
///
/// 两个来源，前者优先：
///   ① 环境变量 `LUNAC_AGENT_EXTRA_HEADERS`（JSON 对象，宿主可注入）；
///   ② `<exe 根>\config\ai.json` 的 `extra_headers` 字段 —— 根由 `LUNAC_SKILLS_DIR`
///      （`<根>\skills`）的父目录反推。留这一条是因为宿主那层是 Flutter 侧编译的，
///      只重编 `agent.exe` 的人也该能用上这个能力（改配置即生效，不必动宿主）。
fn raw_extra_headers() -> &'static Vec<(String, String)> {
    static CACHE: OnceLock<Vec<(String, String)>> = OnceLock::new();
    CACHE.get_or_init(|| {
        if let Ok(raw) = std::env::var("LUNAC_AGENT_EXTRA_HEADERS") {
            let raw = raw.trim();
            if !raw.is_empty() {
                match parse_extra_headers(raw, "LUNAC_AGENT_EXTRA_HEADERS") {
                    Ok(headers) => return headers,
                    Err(e) => log::warn(format!("{e}；改读 config\\ai.json 的 extra_headers")),
                }
            }
        }
        ai_json_extra_headers().unwrap_or_default()
    })
}

/// 从 `<exe 根>\config\ai.json` 的 `extra_headers` 读一份。读不到一律 `None`（静默）：
/// 没配这件事是最常见的情况，不该每次启动都留一行告警。
fn ai_json_extra_headers() -> Option<Vec<(String, String)>> {
    let skills_dir = std::env::var("LUNAC_SKILLS_DIR").ok()?;
    let root = PathBuf::from(skills_dir.trim()).parent()?.to_path_buf();
    read_extra_headers_from(&root.join("config").join("ai.json"))
}

/// [`ai_json_extra_headers`] 的纯文件半边（单测直接喂路径，不碰 env）。
fn read_extra_headers_from(path: &Path) -> Option<Vec<(String, String)>> {
    let text = std::fs::read_to_string(path).ok()?;
    // 用记事本 / PowerShell 5.1 手改过 ai.json 会带 UTF-8 BOM，serde_json 见到它会报
    // 「expected value at line 1 column 1」—— 这里先剥掉，免得「配置明明写了却不生效」。
    let text = text.trim_start_matches('\u{feff}');
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            log::warn(format!(
                "{} 不是合法 JSON（{e}），extra_headers 已忽略",
                path.display()
            ));
            return None;
        }
    };
    let section = value.get("extra_headers")?;
    if !section.is_object() {
        log::warn(format!(
            "{}：extra_headers 必须是 JSON 对象（头名 → 头值），已忽略",
            path.display()
        ));
        return None;
    }
    let raw = serde_json::to_string(section).ok()?;
    let headers = parse_extra_headers(&raw, &path.display().to_string()).ok()?;
    if headers.is_empty() {
        return None;
    }
    log::info(format!(
        "附加请求头 {} 条（来自 {}）",
        headers.len(),
        path.display()
    ));
    Some(headers)
}

/// 本次运行的附加头：占位符已替换，拿不到值的整条丢弃。
fn resolve_extra_headers(model: &str) -> Vec<(String, String)> {
    let session = session_id();
    raw_extra_headers()
        .iter()
        .filter_map(|(name, value)| match substitute_placeholders(value, session, model) {
            Some(resolved) if !resolved.trim().is_empty() => {
                Some((name.clone(), resolved.trim().to_string()))
            }
            _ => {
                log::warn(format!("附加请求头 {name} 解析后为空或缺环境变量，已丢弃"));
                None
            }
        })
        .collect()
}

/// 发往模型端点的 POST 构造器（摘要 / 复盘 / 主请求共用同一个客户端与端点）。
///
/// 附加头在这里统一挂上：四处调用点（摘要、主请求、流式、单发）只认这一个出口，
/// 免得「加了新供应商的头，漏改一处」—— 那类 bug 只在某个分支上出现，极难发现。
fn model_post(cfg: &Cfg) -> reqwest::blocking::RequestBuilder {
    let mut builder = cfg.client.post(&cfg.endpoint);
    for (name, value) in &cfg.extra_headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    builder
}

#[cfg(test)]
mod extra_headers_tests {
    use super::{parse_extra_headers, read_extra_headers_from, substitute_placeholders};

    /// 临时目录里放一份 `ai.json`，返回 (目录, 路径)。BOM 由调用方决定是否加。
    fn write_ai_json(name: &str, body: &[u8], with_bom: bool) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("lunac-extra-headers-{name}"));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("ai.json");
        let mut bytes = Vec::new();
        if with_bom {
            bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        bytes.extend_from_slice(body);
        std::fs::write(&path, &bytes).expect("write ai.json");
        (dir, path)
    }

    #[test]
    fn reads_extra_headers_from_ai_json_even_with_bom() {
        let (dir, path) = write_ai_json(
            "bom",
            br#"{"provider":"opencode","extra_headers":{"x-opencode-session":"${session}"}}"#,
            true,
        );
        let got = read_extra_headers_from(&path).expect("BOM 不该让配置失效");
        assert_eq!(
            got,
            vec![("x-opencode-session".to_string(), "${session}".to_string())]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn absent_section_or_broken_json_is_silently_none() {
        let (dir, path) = write_ai_json("absent", br#"{"provider":"deepseek"}"#, false);
        assert!(read_extra_headers_from(&path).is_none());
        let _ = std::fs::remove_dir_all(dir);

        let (dir, path) = write_ai_json("broken", br#"{ "extra_headers": "#, false);
        assert!(read_extra_headers_from(&path).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn parses_object_and_keeps_custom_headers() {
        let got = parse_extra_headers(
            r#"{"x-opencode-session":"${session}","x-b":"2","x-c":true}"#,
            "test",
        )
        .unwrap();
        assert_eq!(
            got,
            vec![
                ("x-b".to_string(), "2".to_string()),
                ("x-c".to_string(), "true".to_string()),
                ("x-opencode-session".to_string(), "${session}".to_string()),
            ]
        );
    }

    #[test]
    fn rejects_non_object_and_bad_json() {
        assert!(parse_extra_headers("[]", "test").is_err());
        assert!(parse_extra_headers("{", "test").is_err());
    }

    #[test]
    fn drops_reserved_and_empty_values() {
        let got = parse_extra_headers(
            r#"{"authorization":"Bearer x","Anthropic-Version":"2023-06-01","content-type":"x",
                "accept":"text/event-stream","x-empty":"  ","x-null":null,"x-ok":"v"}"#,
            "test",
        )
        .unwrap();
        assert_eq!(got, vec![("x-ok".to_string(), "v".to_string())]);
    }

    #[test]
    fn substitutes_session_and_model() {
        let got = substitute_placeholders("s=${session};m=${model}", "sess_1_2", "m-1").unwrap();
        assert_eq!(got, "s=sess_1_2;m=m-1");
    }

    #[test]
    fn keeps_unknown_placeholder_verbatim() {
        let got = substitute_placeholders("a${nope}b", "s", "m").unwrap();
        assert_eq!(got, "a${nope}b");
    }

    #[test]
    fn missing_env_placeholder_drops_the_header() {
        let got = substitute_placeholders("Bearer ${env:LUNAC_TEST_ABSENT_HEADER_VAR}", "s", "m");
        assert_eq!(got, None);
    }

    #[test]
    fn present_env_placeholder_is_inlined_and_trimmed() {
        std::env::set_var("LUNAC_TEST_PRESENT_HEADER_VAR", "  k-42  ");
        let got = substitute_placeholders("${env:LUNAC_TEST_PRESENT_HEADER_VAR}", "s", "m");
        assert_eq!(got, Some("k-42".to_string()));
    }
}

// ── stdout 输出（前端逐行 JSON.parse）────────────────────────────

/// 单发模式（--print）的 stdout 形态（2026-10-11）。
/// 常驻模式恒为 StreamJson（现行为，逐字节不变）——只有单发才可能切到 Text / Json。
#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputMode {
    /// 现有事件流：每行一条 JSON 事件（常驻模式与 --output-format stream-json 都用它）。
    StreamJson,
    /// 只把最终助手文本原样打到 stdout，不带任何 JSON 包装。
    Text,
    /// 只把**一条** result JSON 打到 stdout（含最终文本 + usage + 是否 error）。
    Json,
}

/// 单发模式（无审批回包通道）下「需审批工具」的处置策略（2026-10-11）。
/// 常驻模式恒为 Stdio（现状，逐字节不变）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ApprovalPolicy {
    /// 常驻：需审批的工具发 control_request，等前端回 control_response（现状）。
    Stdio,
    /// 单发默认：需审批的工具**直接拒绝**，绝不阻塞等待（无回包通道，等就是卡死）。
    UnattendedDeny,
    /// 单发 + --dangerously-skip-permissions：需审批的工具**直接放行**（用户显式承担）。
    UnattendedAllow,
}

static OUTPUT_MODE: AtomicU8 = AtomicU8::new(0);
static APPROVAL_POLICY: AtomicU8 = AtomicU8::new(0);
/// 角色常驻（§待办 3-22 分片 4）：是否读写「每个角色一份」的子代理会话。
///
/// 取值 = 本进程是否开检查点（`--checkpoint` / `--resume` / `--continue`），在 `main()` 里
/// 定型一次。为什么用静态而不是往 `Ctx` 加字段：这个开关与「派子代理」这件事同生命周期，
/// 且 `run_agent_tool` 在并发批里跑（`Fn` 闭包 + `&Ctx`），加字段要动所有构造点，收益为 0。
static SUBAGENT_RESIDENT: AtomicBool = AtomicBool::new(false);

/// 单发模式下「本轮失败（需非 0 退出）」的标志：任何 is_error 的 result 或
/// 因单发策略被拒的审批都会置它。常驻模式只写不读，对外不可见。
static SINGLE_SHOT_FAILED: AtomicBool = AtomicBool::new(false);

/// 单发（--print）的退出码（2026-10-22，S2 调度契约）。
///
///   · `0` = 正常结束，且没有未完成的 `TodoWrite` 清单；
///   · `1` = 本轮失败（`is_error` / 审批被拒 / 网络等）；
///   · `3` = **正常结束，但清单还有 `pending` / `in_progress`**。只有带检查点
///     （`--checkpoint` / `--resume` / `--continue`）时才可能出现 —— 外部调度器据此
///     再跑一次 `--continue`（见 `scripts\agent-task-loop.ps1`）。
///
/// **为什么 `3` 要挂在「可续跑」上**：不带检查点的单发没有上文可接，给出 `3` 只会让
/// 调用方以为「再跑一次就能接上」。默认路径保持 0 / 1 两态 ⇒ 既有调用方逐字节不变。
fn single_shot_exit_code(failed: bool, unfinished: bool, resumable: bool) -> i32 {
    if failed {
        1
    } else if resumable && unfinished {
        3
    } else {
        0
    }
}

/// `--include-partial-messages`（CLI 输出面）：`stream_event` 增量行是否落 stdout。
///
/// **初始值 = true 是「常驻现行为」**：宿主 / 前端一直靠这些增量做打字机效果
/// （`app/src/main.ts` 的 `cliSawStreamDelta` 是它的双渲染守卫），本任务**绝不改宿主协议**
/// —— 常驻模式不碰这个量，于是逐字节不变。只有单发（--print）会在 `main()` 里按 CLI 开关
/// 把它置成传入值；该开关**默认关** ⇒ 单发默认不出增量，只出最终完整消息。
static INCLUDE_PARTIAL_MESSAGES: AtomicBool = AtomicBool::new(true);

fn set_output_mode(m: OutputMode) {
    OUTPUT_MODE.store(m as u8, Ordering::Relaxed);
}

fn output_mode() -> OutputMode {
    match OUTPUT_MODE.load(Ordering::Relaxed) {
        1 => OutputMode::Text,
        2 => OutputMode::Json,
        _ => OutputMode::StreamJson,
    }
}

fn set_approval_policy(p: ApprovalPolicy) {
    APPROVAL_POLICY.store(p as u8, Ordering::Relaxed);
}

fn approval_policy() -> ApprovalPolicy {
    match APPROVAL_POLICY.load(Ordering::Relaxed) {
        1 => ApprovalPolicy::UnattendedDeny,
        2 => ApprovalPolicy::UnattendedAllow,
        _ => ApprovalPolicy::Stdio,
    }
}

fn set_subagent_resident(on: bool) {
    SUBAGENT_RESIDENT.store(on, Ordering::Relaxed);
}

/// 角色常驻是否可用（见 `SUBAGENT_RESIDENT`）。关闭时 `Agent` 行为与改造前逐字节一致。
fn subagent_resident() -> bool {
    SUBAGENT_RESIDENT.load(Ordering::Relaxed)
}

/// `--unattended-allow` 能放行的**唯一**工具集合（2026-10-27，§待办 3-1d 阶段 2B）。
///
/// 为什么写死这三件、而不是「用户给什么就放什么」：
///   · `Cmd` / `PowerShell` 的危险命令静态分析只挂在**审批卡**上；无人值守时没有卡，
///     放行它们等于绕过「危险命令任何档位都要人工确认」（`forbidden.md` §禁用 1.2）；
///   · `Agent` / `CronCreate` / `TaskEnqueue` / 全部 `mcp__*` 会放大成本或再派生任务；
///   · `WebFetch` / `WebSearch` 是外部出口，prompt 注入一旦成功就会变成磁盘副作用。
/// ⇒ 白名单**只覆盖写文件三件**，其余一律按 `UnattendedDeny` 拒绝。
const UNATTENDED_WRITE_TOOLS: &[&str] = &["Write", "Edit", "NotebookEdit"];

/// 进程级白名单（单发启动时定一次，进程内恒定）。
static UNATTENDED_ALLOW: OnceLock<Vec<String>> = OnceLock::new();

/// 把命令行的 `--unattended-allow` 收成**有效白名单**：逗号 / 重复都给，非法名字丢弃并留痕。
///
/// 纯函数（测试直接调它）：不读全局、不看环境。
fn effective_unattended_allow(requested: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in requested {
        for part in raw.split(',') {
            let name = part.trim();
            if name.is_empty() {
                continue;
            }
            if !UNATTENDED_WRITE_TOOLS.contains(&name) {
                log::warn(format!(
                    "--unattended-allow 忽略 {name}：无人值守只放行 {}（命令类 / 外部出口 / 派生任务一律拒绝）",
                    UNATTENDED_WRITE_TOOLS.join(", ")
                ));
                continue;
            }
            if !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

fn set_unattended_allow(list: Vec<String>) {
    if !list.is_empty() {
        log::info(format!(
            "无人值守白名单：{}（其余需审批的工具仍按默认拒绝）",
            list.join(", ")
        ));
    }
    let _ = UNATTENDED_ALLOW.set(list);
}

/// 该工具是否被用户的 `--unattended-allow` 显式放行。
///
/// **只看名单**：是否真的跳过审批由调用点决定（仅 `UnattendedDeny` 分支会问它），
/// 因此常驻模式（`Stdio`）即便带了参数也不会放宽任何东西。
fn unattended_allows(name: &str) -> bool {
    UNATTENDED_ALLOW
        .get()
        .map(|list| list.iter().any(|n| n == name))
        .unwrap_or(false)
}

fn set_include_partial_messages(on: bool) {
    INCLUDE_PARTIAL_MESSAGES.store(on, Ordering::Relaxed);
}

fn include_partial_messages() -> bool {
    INCLUDE_PARTIAL_MESSAGES.load(Ordering::Relaxed)
}

fn emit(line: Value) {
    // 任何 is_error 的 result 都意味着「本轮失败」⇒ 单发模式据此非 0 退出。
    if line.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
        SINGLE_SHOT_FAILED.store(true, Ordering::Relaxed);
    }
    match output_mode() {
        // 常驻 / stream-json：每行一条 JSON 事件（现状）
        OutputMode::StreamJson => write_stdout_line(&line),
        // 单发 json：只落**最终 result** 一行（形状沿用既有 result 事件），其余事件不出 stdout
        OutputMode::Json => {
            if line.get("type").and_then(Value::as_str) == Some("result") {
                write_stdout_line(&line);
            }
        }
        // 单发 text：只把最终助手文本原样打出去（不带 JSON 包装）
        OutputMode::Text => {
            if line.get("type").and_then(Value::as_str) == Some("result") {
                if let Some(t) = line.get("result").and_then(Value::as_str) {
                    write_stdout_text(t);
                }
            }
        }
    }
}

fn write_stdout_line(v: &Value) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{v}");
    let _ = lock.flush();
}

/// 把一段文本原样写到 stdout（末尾没有换行就补一个）。
fn write_stdout_text(t: &str) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = write!(lock, "{t}");
    if !t.ends_with('\n') {
        let _ = writeln!(lock);
    }
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

/// 组装一条 `stream_event` 增量行；`--include-partial-messages` 关闭时返回 `None`
/// （单发默认关 ⇒ 不出增量，只出最终完整消息；常驻恒开 ⇒ 逐字节不变）。
///
/// 抽成纯函数是为了单测能直接钉住「开关 gating + 行的形状」，不必去捕获进程 stdout。
/// 形状沿用既有 stream-json 形态（`{"type":"stream_event","event":{…}}`）—— 既有消费者
/// （前端 `cli-output` 的 `data.type === "stream_event"`）按此解析，**字段一个不改**。
fn stream_event_line(event: Value) -> Option<Value> {
    if !include_partial_messages() {
        return None;
    }
    Some(json!({ "type": "stream_event", "event": event }))
}

fn emit_stream_event(event: Value) {
    // 每行 flush 由 write_stdout_line 保证（emit → write_stdout_line）：无头管道下增量必须
    // 当场可见，不能被行缓冲吞。—。否则「流式」名不副实。
    if let Some(line) = stream_event_line(event) {
        emit(line);
    }
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

// ── 后台子代理（§待办 3-28.7。026-11-15）────────────────────────────
//
// 为什么要有它：`Agent` 派出的子代理原先与本回合同生共死 —。回合收尾前它必须跑完。
// 20 轮的长活只能让主回合干等，用户也没法「先干别的，让它继续跑」。
// `background: true` 把整。`run_subagent` 挪到一条独立线程：
//   · 立刻回一。tool_result（模型知道「已派出」），本回合照常收尾。
//   · 状态进活动面板（`task_started` / `task_progress` / `task_done` 。`background: true`）；
//   · 报告攒进 `BACKGROUND_DONE`，在**下一次有工具调用的回。*并进 tool_result
//     —。与后台命令同一条通路，不新开第二套「异步结果怎么喂给模型」的机制。
//   · 三道闸一个不少：轮数 `MAX_SUBAGENT_ROUNDS`、预。`SUBAGENT_BUDGET_TOKENS`。
//     工具审批照旧（无人应。300s 按拒绝，。`APPROVAL_TIMEOUT_SECS`）；
//   · 用户可一键终止（活动面板 。`tool_control` 。`stop_task`），每轮 / 每次工具调用前生效。

/// 正在跑的后台子代理：`task_id` 。状态。
static BG_TASKS: std::sync::LazyLock<Mutex<HashMap<String, BgTask>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// 一条后台子代理给前。/ 日志看的状态。
struct BgTask {
    /// 用户可见的短标签（`Agent` 。`description`。
    label: String,
    /// 角色名（`general` / `explore` …）
    role: String,
    /// 起点（epoch 毫秒。
    started_ms: i64,
    /// 终止标志（`route_tool_control` 置位，`run_subagent` 读）
    cancel: Arc<AtomicBool>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 当前在跑的后台子代理数（日志 / 单测用）。
fn background_subagent_count() -> usize {
    BG_TASKS.lock().map(|m| m.len()).unwrap_or(0)
}

/// 「本轮是否已追加过一条成长笔记」（2026-10-13 人格地基）：`PetGrowth` **每轮 ≤ 1 条**。
/// 每个 user 轮开始时复位（见提问循环里 `PET_GROWTH_USED_THIS_TURN.store(false, ..)`）；
/// 用进程级静态（与 `LAST_PUSH_NOTIFICATION` / `SHELL_CONTROLS` 同款）——「每轮」是主循环事实，
/// 不必塞进 `Ctx`。子代理拿不到桥（`PetGrowth` 在 `BRIDGE_TOOLS` 里被剔掉），故不受影响。
static PET_GROWTH_USED_THIS_TURN: AtomicBool = AtomicBool::new(false);

/// 一条后台命令给前端看的描述。
struct BgCmd {
    /// 发起它的那次工具调用 —— 前端按它把那张卡片标成「已在后台运行」。
    /// 当前只在登记时写入、由 id 键索引，字段本身暂未回读。
    #[allow(dead_code)]
    tool_use_id: String,
    /// 命令原文（已截断），保留给后续诊断展示。
    #[allow(dead_code)]
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
    // 命令进程的 PID（2026-10-08，用户要求「AI 跑的命令要能在任务管理器里找到」）：
    // 起的是 `cmd /C ...`，**真正干活的程序常常是它的孙进程**，而任务管理器默认只显示
    // 映像名（`cmd.exe`）、不显示命令行 ⇒ 用户找不到。把 PID 一起交出去，前端那一行就能
    // 显示「PID xxxxx」，用户按这个号在任务管理器里精确定位。
    let pid = child.id();
    let shown: String = label.chars().take(120).collect();
    if let Ok(mut m) = BACKGROUND_CMDS.lock() {
        m.insert(id.clone(), BgCmd { tool_use_id: ctl.tool_use_id.clone(), label: shown.clone() });
    }
    emit(json!({
        "type": "system",
        "subtype": "background_started",
        "id": id,
        "pid": pid,
        "tool_use_id": ctl.tool_use_id,
        "label": shown,
    }));
    log::info(format!("shell[{prog}] 转后台 id={id} pid={pid} cmd={shown}"));

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
    // 后台子代理的终止（§待。3-28.7）：。`task_id` 寻址（兼容老前端只。tool_use_id），
    // 置位。`run_subagent` 在下一个检查点收工。*先于**。id 判断 —。这条动作不需。tool_use_id。
    if action == "stop_task" {
        let task = msg.get("task_id").and_then(Value::as_str).unwrap_or(id);
        let found = BG_TASKS.lock().ok().and_then(|m| {
            m.get(task).map(|t| {
                (
                    t.label.clone(),
                    t.role.clone(),
                    t.started_ms,
                    Arc::clone(&t.cancel),
                )
            })
        });
        match found {
            Some((label, role, started_ms, flag)) => {
                flag.store(true, Ordering::Relaxed);
                log::info(format!(
                    "tool_control: 后台子代理 {task}（{label}，role={role}，已跑 {}s）收到终止请求",
                    (now_ms() - started_ms).max(0) / 1000
                ));
            }
            None => log::warn(format!(
                "tool_control: stop_task 找不到在跑的后台子代理（task={task}）—— 忽略"
            )),
        }
        return true;
    }
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

/// 本进程的会话 id。`--resume` 时由检查点里的原 id 提前写入（见 `adopt_session_id`），
/// 于是续跑进程的 `system/init` / `result` / 日志归因与被打断的那次**用同一个 id**。
static SESSION_ID: OnceLock<String> = OnceLock::new();

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
    SESSION_ID.get_or_init(|| {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        format!("sess_{}_{}", std::process::id(), ms)
    })
}

/// 把本进程的会话 id 钉成检查点里的那个。**必须在任何 `session_id()` 调用之前**执行，
/// 否则 `OnceLock` 已定型，返回可读错误（调用方在启动阶段处理）。
fn adopt_session_id(id: String) -> Result<(), String> {
    SESSION_ID
        .set(id)
        .map_err(|_| "会话 id 已被占用（--resume 必须在首次使用前完成）".to_string())
}

/// system/init —— 前端据此把 agentState 从 starting 推进到 idle。
///
/// 超出 `LISTING_BUDGET_CHARS` 的技。*不会进系统提示词**，模型根本不知道它们存在 —。
/// 所以这件事必须可观测（启动日志 + 界面提示），不能只留在清单末尾那句英文里。
static SKILLS_LISTING: OnceLock<skills::ListingStats> = OnceLock::new();

/// `--mcp-server` 的原样值（进程级只读）。
///
/// §待办 3-28.10 ①：技。fork 子代理默。*不接。*（主循环那条 `&mut McpSet` 归主线程），
/// 但开启「技能记忆」后它的 `Remember` 必须写到宿主。做法与复盘 fork 相同 —。
/// **自建一条短命桥**（多起一。`lunac.exe --mcp-server`，握完手就退出）。
static MCP_SERVER_SPEC: OnceLock<String> = OnceLock::new();

fn set_mcp_server_spec(spec: Option<&str>) {
    if let Some(spec) = spec.map(str::trim).filter(|s| !s.is_empty()) {
        let _ = MCP_SERVER_SPEC.set(spec.to_string());
    }
}

/// 技能记忆写入用的桥；连不上就返。`None`（调用方照旧跑，只是写不进记忆）。
fn connect_memory_bridge() -> Option<mcp::McpSet> {
    let spec = MCP_SERVER_SPEC.get()?;
    let set = mcp::McpSet::connect(
        Some(spec.as_str()),
        &[],
        &[],
        Vec::new(),
        None,
        sampling_handler_for,
    );
    if set.has_lunac() {
        Some(set)
    } else {
        log::warn("技能记忆：MCP 桥没连上，Remember 写不进技能命名空间");
        None
    }
}

/// system/init —— 前端据此把 agentState 从 starting 推进到 idle。
/// 真实 CLI 每轮查询都会发一次，这里保持一致；`tools` 上报本轮可用工具名。
fn emit_init(model: &str, tool_names: &[String], system_prompt: &str, ctx_block: Option<&Value>) {
    let skills_stats = SKILLS_LISTING.get().copied().unwrap_or_default();
    let mut payload = json!({
        "type": "system",
        "subtype": "init",
        "session_id": session_id(),
        "model": model,
        "tools": tool_names,
        // §待办 3-28.8 上下文水位条：把**同一份常。*发给界面 —。阈值只有一处定。
        // （`ELIDE_RATIO` / `DROP_RATIO`），前端不复制数字、也不自己估预算。
        "max_context_tokens": max_context_tokens(),
        "elide_ratio": ELIDE_RATIO,
        "drop_ratio": DROP_RATIO,
        // §待办 3-28.11 技能清单预算：`omitted > 0` = 有技能没进清单（界面该给提示）。
        "skills_listing": {
            "total": skills_stats.total,
            "listed": skills_stats.listed,
            "omitted": skills_stats.omitted(),
            "budget_chars": skills::LISTING_BUDGET_CHARS,
        },
    });
    if let Some(map) = payload.as_object_mut() {
        if let Value::Object(stats) = init_prompt_stats(system_prompt, ctx_block) {
            for (key, value) in stats {
                map.insert(key, value);
            }
        }
    }
    emit(payload);
}

/// `system/init` 。**SYSTEM / CONTEXT** 两栏（§待。3-28.12 ①）。
///
/// **口径决定。026-11-16。*：只。*字符。+ SHA-256 + 是否存在**，不发正。—。
/// 系统提示词里可能含用户的人格 / 记忆原文，界面没有逐字阅读它的需求；
/// 真要核对完整请求体可开 `LUNAC_DUMP_API`（那条调试链路本来就有）。
/// 哈希让「前缀缓存为什么失效」可被界面看见：同一份提示词 。同一串哈希。
fn init_prompt_stats(system_prompt: &str, ctx_block: Option<&Value>) -> Value {
    let (context_chars, context_sha, present) = match ctx_block {
        Some(block) => {
            let text = block.to_string();
            (
                text.chars().count() as u64,
                sha256_hex(text.as_bytes()),
                true,
            )
        }
        None => (0, String::new(), false),
    };
    json!({
        "system_prompt": {
            "chars": system_prompt.chars().count() as u64,
            "sha256": sha256_hex(system_prompt.as_bytes()),
        },
        "context_block": {
            "present": present,
            "chars": context_chars,
            "sha256": context_sha,
        },
    })
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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

/// **暂停时正在等审批**。026-11-14）：stdin 线程收到 pause 时把它置位，让本。
/// `result.subtype` 。`paused` 而不。`success`（deny 本身只负责把阻塞解开）。
static PAUSED_AT_APPROVAL: AtomicBool = AtomicBool::new(false);

/// 把「暂停」送进正在等审批的工具调用：逐条。`deny + interrupt`，`await_approval`
/// 会立刻返。整批工具被拒、本轮在回灌结果后收口。
///
/// 返回 false = 当前没有待答审批（pause 照旧进主队列，由轮首 / 轮末安全点处理）。
/// 为什么必须逐条：agent 。*按序**逐个阻塞等回包的，漏一条整批都醒不过来。
fn deny_all_pending_approvals(reason: &str) -> bool {
    let senders: Vec<(String, mpsc::Sender<Value>)> = match pending_approvals().lock() {
        Ok(reg) => reg.iter().map(|(id, tx)| (id.clone(), tx.clone())).collect(),
        Err(_) => return false,
    };
    if senders.is_empty() {
        return false;
    }
    for (request_id, tx) in senders {
        let _ = tx.send(json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": {
                    "behavior": "deny",
                    "message": reason,
                    "interrupt": true,
                    "toolUseID": "",
                },
            },
        }));
    }
    true
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
/// `NotebookEdit`（2026-10-11）取 `new_source`；`delete` 档没有这个字段 ⇒ 返回 `None`，
/// 不扫（删 cell 不写入任何新内容）。
/// 返回 `None` = 入参缺字段（工具执行时会自己报参数错误，这里不重复判定）。
fn written_payload<'a>(tool_name: &str, input: &'a Value) -> Option<&'a str> {
    let key = match tool_name {
        "Edit" => "new_string",
        "NotebookEdit" => "new_source",
        _ => "content",
    };
    input.get(key).and_then(Value::as_str)
}

/// 写入类工具的**安全告警载荷**（A17，2026-10-01）：`[{rule, line}]`，没有命中就是空数组。
///
/// **它与审批卡那份是同源的**（同一个 `written_payload` + `content_safety::analyze`）：
/// 两处各写一遍必然漂移 —— 用户在审批卡上看见命中、结果卡上却什么都没有，比不标更糟。
/// 单独算一次（而不是把审批的结果捎过来）是**有意**的：hook 预先放行时 `open_approval`
/// 两处各写一遍必然漂移 —— 用户在审批卡上看见命中、结果卡上却什么都没有，比不标更糟。
///
/// 只覆盖 `Write` / `Edit` / `NotebookEdit`。`Cmd` / `PowerShell` 扫的是**命令**、走
/// `bash_safety`，那是另一套规则集（见 §13.1「两个方向别混」）。
fn security_warnings(tool_name: &str, input: &Value) -> Value {
    if !matches!(tool_name, "Write" | "Edit" | "NotebookEdit") {
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
    // 只做「凭据 / 密钥泄漏」这一类 —— 审批卡是用户唯一能看见内容的地方，
    // 只做「凭据 / 密钥泄漏」这一类 —— 审批卡是用户唯一能看见内容的地方，
    if matches!(tool_name, "Write" | "Edit" | "NotebookEdit") {
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

// ── MCP sampling（服务端反向调用模型，默认声明）────────────────────
//
// 处理服务端 `sampling/createMessage`：**复用既有模型调用入口** —— 与摘要压缩同一套
// 「非流式 Messages 请求 + `usage` 归因」的写法（见 `summarize_dropped`），不新开 HTTP 通道。
//
// **执行顺序**（2026-10-11 用户裁决，本任务题眼）：
//   ① 无审批通道（单发 `--print`）⇒ **明确拒绝** —— `--dangerously-skip-permissions` 也
//      **不能**放行（它只让「工具审批」直接过，对 sampling 这道高底线闸无效）；
//   ② **高底线审批卡**：用户**明确同意**才继续（不可被 hook 的 `allow` / 白名单 / 自动档跳过；
//      卡上只给「允许（本会话）/ 拒绝」，**没有**「始终允许」）。**本会话记住**：用户点过一次
//      「允许」后，**本会话内该服务器**再请求 sampling 直接放行、不再弹卡（进程重启即忘）；
//      拒绝**不**记入，故下一次仍要卡。经 **HTTP 反向通道**到达的请求弹**警告样式**的卡；
//   ③ **成本护栏（第二层兜底）**：单次 token 预算 + 会话内次数上限，超限即拒并说明。
//
// 任何一条不满足都**明确拒绝**（`mcp_sampling_create` 返回 Err，由 `mcp.rs` 的
// `answer_server_request` 包成带说明的 JSON-RPC error），绝不静默挂起。
// 「本会话记住」只跳过**审批卡**这一道；①③ 两道照旧（单发仍拒、预算与次数仍计）。

/// 单次 sampling 的 token 预算上限（请求里的 `maxTokens` 超过它即拒）。
const MCP_SAMPLING_MAX_TOKENS_ENV: &str = "LUNAC_MCP_SAMPLING_MAX_TOKENS";
const MCP_SAMPLING_DEFAULT_MAX_TOKENS: u64 = 4096;
/// 会话内（进程级）sampling 次数上限。
const MCP_SAMPLING_MAX_CALLS_ENV: &str = "LUNAC_MCP_SAMPLING_MAX_CALLS";
const MCP_SAMPLING_DEFAULT_MAX_CALLS: u64 = 8;
/// 单次 sampling 的 HTTP 超时（与摘要压缩同量级；不占共享客户端的 1800s）。
const MCP_SAMPLING_TIMEOUT_SECS: u64 = 120;

/// sampling 用的独立 `Cfg`（`detached()` 出来、与主循环**共享** `sub` 用量账）。
///
/// 为什么走全局：`Bridge` 里的钩子是 `'static`，捕获不了主循环那份 `&Cfg`（含 `Cell`、非
/// `Sync`）；而钩子在 `McpSet::connect` 时就装好，那时主 `cfg` 还没建（见 `main`）。故
/// `cfg` 建好后放一份进来，钩子取用。未就绪时（握手阶段就收到 sampling）如实报「未就绪」。
static MCP_SAMPLING_CFG: Mutex<Option<Cfg>> = Mutex::new(None);
/// 本会话已处理的 sampling 次数（进程重启即归零）。
static MCP_SAMPLING_CALLS: AtomicU64 = AtomicU64::new(0);
/// 本会话已同意 sampling 的服务器集合（**仅记本会话**：进程重启即清空）。键见
/// [`sampling_approval_key`]。**只在用户显式点过「允许」之后写入**（拒绝不写）——
/// 这是「用户同意一次、本会话内该服务器不再弹卡」的唯一来源，绝不由自动放行写入。
static MCP_SAMPLING_APPROVED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// sampling 授权「本会话记住」的键 = 传输 + 服务器名。
///
/// **为什么带上传输**：stdio 与 HTTP 是两条不同的信任路径（后者是服务器**经 HTTP 主动推来**的
/// 反向请求）。若只按名字记，一个与已同意的 stdio 服务器**同名**的恶意 HTTP 服务器就会被静默
/// 放行、连警告卡都不弹 —— 故两者分开记。
fn sampling_approval_key(server: &str, via_http: bool) -> String {
    format!("{}\u{1}{}", if via_http { "http" } else { "stdio" }, server)
}

/// 为某台服务器构造一个 sampling 处理钩子（每台一份，闭包捕获服务器名与传输用于审批卡）。
/// 直接作为 [`mcp::SamplingFactoryFn`] 传给 `McpSet` —— 见 `McpSet::connect` / `add_project`。
/// `via_http` = 这条是不是**远端 HTTP 服务器**（其 sampling 经响应 SSE 流的反向通道到达）⇒
/// 审批卡渲染成**警告样式**。
fn sampling_handler_for(server: &str, via_http: bool) -> Option<mcp::SamplingHandler> {
    let name = server.to_string();
    Some(Box::new(move |params: &Value| {
        mcp_sampling_create(&name, params, via_http)
    }))
}

/// 读一个正整数 env（缺失 / 非法 / ≤0 一律回落默认值）。
fn mcp_sampling_u64(env: &str, default: u64) -> u64 {
    std::env::var(env)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

/// 从一条 Messages 回包的 `usage` 取四类 token（缺字段 = 0）；顺序与 `SubagentUsage::record` 对齐。
fn sampling_usage_quad(u: Option<&Value>) -> (u64, u64, u64, u64) {
    let n = |k: &str| {
        u.and_then(|v| v.get(k))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    (
        n("input_tokens"),
        n("cache_read_input_tokens"),
        n("cache_creation_input_tokens"),
        n("output_tokens"),
    )
}

/// MCP `SamplingMessage[]` → Anthropic `messages[]`（只认 text / image 两种内容块）。
fn mcp_sampling_messages(params: &Value) -> Result<Vec<Value>, String> {
    let arr = params
        .get("messages")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| "sampling refused: missing or empty `messages`".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for m in arr {
        let role = match m.get("role").and_then(Value::as_str) {
            Some("user") => "user",
            Some("assistant") => "assistant",
            other => return Err(format!("sampling refused: bad message role {other:?}")),
        };
        let content = m
            .get("content")
            .ok_or_else(|| "sampling refused: message has no content".to_string())?;
        out.push(json!({ "role": role, "content": [mcp_sampling_content_block(content)?] }));
    }
    Ok(out)
}

/// 单个 MCP 内容块 → Anthropic 内容块。认不出的类型**一律拒** —— 宁可拒，也不把不明内容当文本塞进去。
fn mcp_sampling_content_block(content: &Value) -> Result<Value, String> {
    match content.get("type").and_then(Value::as_str) {
        Some("text") => Ok(json!({
            "type": "text",
            "text": content.get("text").and_then(Value::as_str).unwrap_or(""),
        })),
        Some("image") => Ok(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": content.get("mimeType").and_then(Value::as_str).unwrap_or("image/png"),
                "data": content.get("data").and_then(Value::as_str).unwrap_or(""),
            }
        })),
        other => Err(format!(
            "sampling refused: unsupported content type {other:?} (only text/image supported)"
        )),
    }
}

/// 高底线审批卡：服务端要借本机模型（`sampling`）时才问。
///
/// 走的是与 `open_elicitation` / `ask_mcp_trust` **同一条**审批通道（`control_request` /
/// `control_response`），但载荷形状与语义是独立的：`subtype = "mcp_sampling"`。
///
/// **本会话记住**：用户点过「允许」后，本会话内**同一服务器**（键含传输，见
/// [`sampling_approval_key`]）再请求 sampling **直接放行、不再弹卡**；拒绝**不**记入，
/// 故下一次仍要卡。集合只写在这里（用户显式允许），进程重启即清空。
///
/// **不可被自动放行跳过**（本任务核心约束）：
///   · `--dangerously-skip-permissions` —— 调用方在进入本函数前已按 `approval_policy()`
///     把它挡在门外（`UnattendedAllow` 与 `UnattendedDeny` 同样拒绝），到不了这里；
///   · hook 的 `allow` —— hooks 只挂在「工具调用」路径（`PreToolUse` / `PermissionRequest`），
///     sampling 是**服务端反向请求**、不是工具调用，根本不经过 hooks；
///   · 白名单 / `always-allow` —— 本卡只给「允许（本会话）/ 拒绝」两个按钮，**不提供**「始终允许」，
///     前端也不会把它交给 `classifyRequest()`（那条路只管 `can_use_tool`）；
///   · 自动档 —— 不经过 `classifyRequest()`，本卡照弹（除非本会话已记住，那是**用户此前显式允许**的）。
///
/// 返回 `true` = 用户显式点了「允许」（或本会话此前已允许该服务器）；
/// `false` = 用户拒绝 / 超时 / 卡被撤（一律按拒绝）。
fn open_sampling_approval(server: &str, params: &Value, via_http: bool) -> bool {
    let key = sampling_approval_key(server, via_http);
    // 本会话已同意过该服务器 ⇒ 直接放行、不再弹卡（**只由用户显式点过「允许」写入**）。
    if MCP_SAMPLING_APPROVED
        .lock()
        .map(|s| s.iter().any(|k| k == &key))
        .unwrap_or(false)
    {
        log::info(format!(
            "MCP sampling：本会话已同意（{key}），直接放行、不再弹卡"
        ));
        return true;
    }
    let request_id = next_request_id();
    let (tx, rx) = mpsc::channel::<Value>();
    if let Ok(mut reg) = pending_approvals().lock() {
        reg.insert(request_id.clone(), tx);
    }
    let message_count = params
        .get("messages")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    let max_tokens = params.get("maxTokens").and_then(Value::as_u64).unwrap_or(0);
    let label = if server.trim().is_empty() { "MCP" } else { server };
    log::info(format!(
        "MCP sampling 授权询问：server={label} via_http={via_http} messages={message_count} maxTokens={max_tokens}"
    ));
    emit(json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "mcp_sampling",
            "server": label,
            "messageCount": message_count,
            "maxTokens": max_tokens,
            // 反向（HTTP）路径标记：前端据此渲染**警告样式**的卡（与 stdio 常规卡区分）。
            "viaHttp": via_http,
        }
    }));
    let allowed = matches!(
        await_approval(Pending { request_id, rx }, &json!({})),
        Decision::Allow(_)
    );
    if allowed {
        if let Ok(mut s) = MCP_SAMPLING_APPROVED.lock() {
            if !s.iter().any(|k| k == &key) {
                s.push(key);
            }
        }
    }
    allowed
}

/// 处理一次服务端 `sampling/createMessage`（护栏与复用点见本节文件头注释）。
///
/// 入参 = MCP `CreateMessageRequest` 的 `params`（`{messages, systemPrompt?, maxTokens, …}`）；
/// 返回 `Ok(CreateMessageResult)`，或 `Err(拒绝原因)`（会变成带说明的 JSON-RPC error）。
fn mcp_sampling_create(server: &str, params: &Value, via_http: bool) -> Result<Value, String> {
    // 高底线闸①：没有审批回包通道（单发 `--print` 的 `UnattendedDeny` / `UnattendedAllow`）
    // ⇒ 直接拒。**关键**：`UnattendedAllow`（= `--dangerously-skip-permissions`）在这里与
    // `UnattendedDeny` **同等**被拒 —— 借用户的模型这件事不可被 skip-permissions 静默放行。
    if approval_policy() != ApprovalPolicy::Stdio {
        SINGLE_SHOT_FAILED.store(true, Ordering::Relaxed);
        log::warn("单发模式（--print）无审批通道，sampling 直接拒绝");
        return Err(
            "sampling refused: no user-approval channel (single-shot / --print mode); \
             borrowing the model needs an interactive consent this mode cannot provide"
                .into(),
        );
    }
    // 高底线闸②：弹卡（用户明确同意才继续）。*本会话记。*：本会话内已同意过该服务。
    // ⇒ 在 `open_sampling_approval` 内部直接放行、不再弹卡（只跳过卡，不跳过①③两道）。
    if !open_sampling_approval(server, params, via_http) {
        return Err("sampling refused: the user denied this request".into());
    }

    // 成本护栏（第二层兜底，见本节文件头）：单次 token 预算 + 会话内次数上限。
    let budget = mcp_sampling_u64(MCP_SAMPLING_MAX_TOKENS_ENV, MCP_SAMPLING_DEFAULT_MAX_TOKENS);
    let max_calls = mcp_sampling_u64(MCP_SAMPLING_MAX_CALLS_ENV, MCP_SAMPLING_DEFAULT_MAX_CALLS);

    // 把请求翻成 Anthropic Messages 形状（**不带 tools**，见护栏③）。翻不动就拒。
    let msgs = mcp_sampling_messages(params)?;
    let in_chars: usize = msgs.iter().map(|m| m.to_string().chars().count()).sum();

    // 护栏②：成本上限 —— 单次 token 预算 + 会话内次数上限，任一超限即拒并说明。
    let requested = params
        .get("maxTokens")
        .and_then(Value::as_u64)
        .unwrap_or(budget);
    if requested > budget {
        return Err(format!(
            "sampling refused: requested maxTokens={requested} exceeds the per-call budget \
             {budget} ({MCP_SAMPLING_MAX_TOKENS_ENV})"
        ));
    }
    // 输入侧粗算（≈4 字符/token，与固定前缀那条口径同）：服务端不该拿超大 prompt 烧钱。
    if in_chars > (budget as usize) * 4 {
        return Err(format!(
            "sampling refused: input too large (~{in_chars} chars) for the {budget}-token budget"
        ));
    }
    let n = MCP_SAMPLING_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    if n > max_calls {
        return Err(format!(
            "sampling refused: session call limit reached ({max_calls}, {MCP_SAMPLING_MAX_CALLS_ENV})"
        ));
    }

    // 采样上下文未就绪 = 握手期就收到了 sampling ⇒ 明确报错，绝不干等。
    let guard = MCP_SAMPLING_CFG
        .lock()
        .map_err(|_| "sampling refused: model context is poisoned".to_string())?;
    let cfg = guard.as_ref().ok_or_else(|| {
        "sampling refused: model context is not ready yet \
         (request arrived before startup finished)"
            .to_string()
    })?;

    // 组装请求体。**不带 `tools`** ⇒ 护栏③：模型不可能产生 tool_use，服务端无法借
    // sampling 驱动本机工具。服务端的 `modelPreferences` **一律忽略** —— 模型只能由本机
    // 配置决定（成本 / 安全，服务端不得指定我们要花多少钱）。
    let mut body = json!({
        "model": cfg.model,
        "max_tokens": requested,
        "stream": false,
        "messages": msgs,
    });
    if let Some(sys) = params.get("systemPrompt").and_then(Value::as_str) {
        if !sys.trim().is_empty() {
            body["system"] = json!(sys);
        }
    }
    if let Some(t) = params.get("temperature").and_then(Value::as_f64) {
        body["temperature"] = json!(t);
    }
    if let Some(s) = params.get("stopSequences") {
        if s.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            body["stop_sequences"] = s.clone();
        }
    }

    // 调试转储（LUNAC_DUMP_API）：非流式，request/response 各一行，req_id = `sampling-<ms>`。
    let dump_on = log::dump_api_enabled();
    let dump_rid = if dump_on {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let rid = format!("sampling-{ms}");
        log::dump_api("request", &rid, &cfg.model, None, None, &body);
        rid
    } else {
        String::new()
    };

    let resp = model_post(cfg)
        .timeout(Duration::from_secs(MCP_SAMPLING_TIMEOUT_SECS))
        .header("authorization", format!("Bearer {}", cfg.token))
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send();
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            if dump_on {
                log::dump_api(
                    "response",
                    &dump_rid,
                    &cfg.model,
                    None,
                    Some(&e.to_string()),
                    &Value::Null,
                );
            }
            return Err(format!("sampling model call failed: {e}"));
        }
    };
    let status = resp.status();
    let raw = resp
        .text()
        .map_err(|e| format!("sampling response read failed: {e}"))?;
    if dump_on {
        log::dump_api(
            "response",
            &dump_rid,
            &cfg.model,
            Some(status.as_u16()),
            None,
            &Value::String(raw.clone()),
        );
    }
    if !status.is_success() {
        return Err(format!(
            "sampling model call failed (HTTP {status}): {}",
            log::truncate_chars(&raw, 200)
        ));
    }
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| format!("sampling response is not JSON: {e}"))?;

    // 护栏④：用量归因 —— 记进**主循环那本账**（`SubagentUsage` ⇒ `usage_delta` +
    // `result.usage.subagent`），并在日志里标明来源是 MCP 服务端的反向调用
    //（成本不能记到用户头上还不说）。
    let (r_in, r_read, r_create, r_out) = sampling_usage_quad(parsed.get("usage"));
    cfg.sub.record(r_in, r_read, r_create, r_out);
    log::info(format!(
        "MCP sampling 用量：来源=MCP 服务端反向调用（会话第 {n}/{max_calls} 次）\
         in={r_in} read={r_read} create={r_create} out={r_out}"
    ));

    let text = parsed
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
        return Err("sampling model returned no text".into());
    }

    Ok(json!({
        "role": "assistant",
        "content": { "type": "text", "text": text },
        "model": cfg.model,
        "stopReason": "endTurn",
    }))
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
    // 单发模式没有审批回包通道 ⇒ 直接拒绝，绝不对着无人应答的通道干等（会卡满超时）。
    if approval_policy() != ApprovalPolicy::Stdio {
        log::warn("单发模式（--print）无审批通道，elicitation 直接拒绝");
        return json!({ "action": "decline" });
    }
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
///   。否则。*一。*信任。—。同意则接（选了「始终信任」就记进信任记录），拒绝则本次会话不接。
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
        // 「没弹卡」有两种可能，日志里必须能分开：**这一条** = 已在信任记录里（正常的
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
        .add_project(servers, disallowed, sampling_handler_for);
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
    // `CronCreate` / `TaskEnqueue`（2026-10-17 / 2026-10-22）：**无论 hook 是否 allow，
    // 都必须走审批卡**。用户裁定「所有运行模式都强制弹卡」；hook 的 allow 只等于
    // 「跳过这一张卡」，不能把「持续产生 API 调用 + 写持久化任务」变成静默放行。
    if tool == "CronCreate" || tool == "TaskEnqueue" {
        return true;
    }
    if matches!(tool, "Cmd" | "PowerShell") {
        if let Some(cmd) = input.get("command").and_then(Value::as_str) {
            return !bash_safety::analyze(cmd).dangerous.is_empty();
        }
    }
    if matches!(tool, "Write" | "Edit" | "NotebookEdit") {
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
//   --unattended-allow <names>     单发逐工具放行（只认 Write/Edit/NotebookEdit，逗号分隔、可重复）
//   --permission-prompt-tool stdio 审批走 stdout/stdin 的 control 协议（P2）
//   --disallowedTools <name…>      variadic：这些工具不进请求体
//   --print / -p [query]           单发：跑恰好一轮提问后退出（见 single_shot）
//   --output-format <fmt>          单发模式的 stdout 形态（text / json / stream-json）
//   --include-partial-messages     单发 stream-json 下额外逐条输出流式增量（默认关）
//   --input-format stream-json     常驻 stdio 流式输入模式标志（宿主一直传它）
// 其余（--verbose …）一律接受并忽略。

/// `--continue` 的内置提问（2026-10-22，S2 跨回合续跑的最小闭环）。
///
/// 语义 = 「恢复检查点 + 让模型接着做」，**不要求用户重述任务**。它只解决「下一次提问说什么」，
/// 真正的调度（定时 / 循环调用）由外部（cron、CI、脚本）负责 —— 本进程不做自循环，
/// 避免无人在场时无限烧钱。
const CONTINUE_PROMPT: &str = "Continue the unfinished task from the restored conversation. \
Start from the first unfinished step, reuse what is already done, and keep going until every \
step is complete. Do NOT repeat or restate any text that is already in the conversation; only \
emit the new work and the final result. If everything is already done, summarize what changed \
in a few lines and say so.";

#[derive(Default)]
struct CliArgs {
    add_dirs: Vec<PathBuf>,
    permission_mode: String,
    skip_permissions: bool,
    /// `--unattended-allow Write,Edit,NotebookEdit`（2026-10-27，§待办 3-1d 阶段 2B）：
    /// 单发（无审批回包通道）下**逐工具**放行，而不是整批 skip-permissions。
    /// 可重复给、也可一条逗号分隔；超出写类三件的名字一律丢弃（见 `effective_unattended_allow`）。
    unattended_allow: Vec<String>,
    ask_permission: bool,
    disallowed: Vec<String>,
    /// `--mcp-server stdio:<exe 路径>` —— 上游把 lunac.exe 路径交给我们，
    /// 由 agent 侧拉起它的 MCP server（见 mcp.rs）
    mcp_server: Option<String>,
    /// `--print` / `-p`：单发模式。是否真生效还要看 `input_stream_json`（见 single_shot）。
    print: bool,
    /// `--print` 紧跟的那个非 flag 值（就是 query）。没给 = 从 stdin 读一次。
    print_query: Option<String>,
    /// `--output-format`（`text` / `json` / `stream-json`），默认 `text`。只在单发模式生效。
    output_format: String,
    /// `--include-partial-messages`：单发 stream-json 下额外逐条输出流式增量事件
    /// （`stream_event`）。默认 **关**；常驻模式不受它影响（见 `emit_stream_event`）。
    include_partial_messages: bool,
    /// `--input-format stream-json` —— 常驻 stdio 流式输入模式的标志（宿主 / e2e 脚本都传它）。
    input_stream_json: bool,
    /// `--checkpoint`：每个用户回合结束后把 history 落成检查点（默认关，见 `checkpoint.rs`）。
    checkpoint: bool,
    /// `--resume <session-id|latest>`：启动时恢复检查点（history / 提问序号 / 任务预算）。
    /// 裸 `--resume` 视为 `latest`。
    resume: Option<String>,
    /// `--continue`：单发下隐含 `--resume latest` + `--checkpoint`，且**没显式给 query** 时
    /// 用 `CONTINUE_PROMPT` 作为本轮提问（跨回合续跑的最小闭环）。
    continue_task: bool,
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
                "--unattended-allow" => {
                    if let Some(v) = args.get(i + 1) {
                        out.unattended_allow.push(v.clone());
                        i += 1;
                    }
                }
                "--print" | "-p" => {
                    out.print = true;
                    // 只有**紧跟在后面且不是 flag** 的那个 token 才算 query —— 否则
                    // `--print --verbose …`（宿主形态）会把 --verbose 误当提问。
                    if let Some(v) = args.get(i + 1) {
                        if !v.starts_with('-') {
                            out.print_query = Some(v.clone());
                            i += 1;
                        }
                    }
                }
                "--output-format" => {
                    if let Some(v) = args.get(i + 1) {
                        out.output_format = v.clone();
                        i += 1;
                    }
                }
                "--include-partial-messages" => out.include_partial_messages = true,
                "--input-format" => {
                    if let Some(v) = args.get(i + 1) {
                        out.input_stream_json = v == "stream-json";
                        i += 1;
                    }
                }
                "--checkpoint" => out.checkpoint = true,
                "--continue" => out.continue_task = true,
                "--resume" => {
                    // 值可省略（= latest）。**只吃「看起来像会话 id」的下一 token** ——
                    // `--print "<query>" --resume` 这类形态里的 query 已经挂在 --print 上了，
                    // 但别的 flag 后面也可能跟普通词，宁可认不出就按 latest，也不吞参数。
                    let mut value = "latest".to_string();
                    if let Some(v) = args.get(i + 1) {
                        if v == "latest" || v.starts_with("sess_") || v.ends_with(".json") {
                            value = v.clone();
                            i += 1;
                        }
                    }
                    out.resume = Some(value);
                }
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
        if out.output_format.is_empty() {
            out.output_format = "text".to_string();
        }
        out
    }

    /// 单发模式（--print）是否生效。
    ///
    /// 例外：`--print` + `--input-format stream-json` = 上游常驻 stdio 流式输入模式
    /// （宿主的 `start_cli_process` 一直这么传），必须保持常驻、绝不能退化成单发。
    /// 于是判据是「带 --print 且未显式声明 stream-json 输入」。
    fn single_shot(&self) -> bool {
        self.print && !self.input_stream_json
    }
}

// ── 入口 ─────────────────────────────────────────────────────────

/// 当前进程的审批 / 工作区口径，随检查点一起落盘。
///
/// 只记 `cli` 里的显式口径 + 进程 cwd（不含 `output_dir` 这类内部追加目录），
/// 否则每次运行的临时路径都会制造假差异。
fn approval_context(cli: &CliArgs) -> checkpoint::ApprovalContext {
    checkpoint::ApprovalContext {
        permission_mode: cli.permission_mode.clone(),
        skip_permissions: cli.skip_permissions,
        ask_permission: cli.ask_permission,
        disallowed: cli.disallowed.clone(),
        add_dirs: cli.add_dirs.iter().map(|p| p.display().to_string()).collect(),
        cwd: std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    }
}

/// 把（已按「最严者胜」合并过的）审批口径写回命令行参数 —— 恢复之后的**唯一**生效点。
///
/// 为什么写回 `CliArgs` 而不是另建一份：`read_only` / 审批策略 / 工具池 / 工作区边界
/// 全都从 `cli` 的这几个字段派生（见 `main` 里建 `Ctx` 的那段）⇒ 写回这里等于让所有
/// 下游判定自动跟着收紧，**不存在「某处忘了看新上下文」的第二种写法**。
fn apply_approval_context(cli: &mut CliArgs, ctx: &checkpoint::ApprovalContext) {
    cli.permission_mode = ctx.permission_mode.clone();
    cli.skip_permissions = ctx.skip_permissions;
    cli.ask_permission = ctx.ask_permission;
    cli.disallowed = ctx.disallowed.clone();
    cli.add_dirs = ctx.add_dirs.iter().map(PathBuf::from).collect();
}

/// `--analyze` 的输出：`{dangerous, opaque, readonly}`（与 `can_use_tool` 。`analysis`
/// 同形，宿主的预演直接把它喂给同一套原因码判定）。
fn analyze_json(command: &str) -> String {
    let report = bash_safety::analyze(command);
    serde_json::json!({
        "dangerous": report.dangerous,
        "opaque": report.opaque,
        "readonly": report.readonly,
    })
    .to_string()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("agent 0.2.0 (lunac self-developed agent core, P1)");
        return;
    }
    // 判定预演（白箱账。W0）：`agent --analyze "<命令>"` 只跑 `bash_safety::analyze`
    // 并输。JSON。*不执行命令、不读配置、不落日。*。宿主的「安。预演」用它，
    // 保证预演与真实审批用的是同一套判定（不把规则抄第二遍）。
    if let Some(index) = args.iter().position(|a| a == "--analyze") {
        match args.get(index + 1) {
            Some(command) => {
                println!("{}", analyze_json(command));
                return;
            }
            None => {
                eprintln!("[agent] --analyze 需要一条命令");
                std::process::exit(2);
            }
        }
    }
    // 落盘日志（<exe 根>\temp\logs\agent-YYYY-MM-DD.log）。release 是 GUI 子系统、
    // 没有控制台，下面所有 eprintln 线上都拿不到 —— 出问题只能靠这个文件回溯。
    log::init("agent");
    let mut cli = CliArgs::parse(&args);
    // §待办 3-28.10 ①：技。fork 在开启技能记忆时要用它自建一条短命桥。
    set_mcp_server_spec(cli.mcp_server.as_deref());
    // 单发模式（--print）在解析后立刻定型：全局 stdout 形态与审批策略只有单发才改，
    // 常驻模式保持恒默认（StreamJson / Stdio）⇒ 行为逐字节不变。
    let single_shot = cli.single_shot();
    // `--continue` 只在单发下有意义（常驻模式下「继续」由宿主 / 用户驱动）。
    if cli.continue_task && !single_shot {
        log::warn("--continue 只在单发（--print）下生效，本进程已忽略");
    }
    let continue_ok = cli.continue_task && single_shot;
    // ── S2 检查点 / 恢复（2026-10-22）────────────────────────────
    // 默认**不写**：只有 `--checkpoint` / `--resume` / `--continue` 才落盘（见 checkpoint.rs）。
    // 失败**不静默回落**到空历史 —— 用户明确要求续跑，读不到就该拿到非 0 退出码，
    // 否则会以「没有上文」的姿态继续花钱（与 release 缺失时 exit 2 同一条纪律）。
    let checkpointing = cli.checkpoint || cli.resume.is_some() || continue_ok;
    // 角色常驻（§待办 3-22 分片 4）与检查点同一个开关：不落检查点的进程也不落角色会话。
    set_subagent_resident(checkpointing);
    let resume_spec: Option<String> = if continue_ok && cli.resume.is_none() {
        Some("latest".to_string())
    } else {
        cli.resume.clone()
    };
    let resumed = if let Some(spec) = resume_spec.as_deref() {
        match checkpoint::load(spec) {
            Ok((cp, _)) => {
                if let Err(e) = adopt_session_id(cp.session_id.clone()) {
                    eprintln!("[agent] --resume 失败：{e}");
                    log::warn(format!("--resume 失败：{e}"));
                    std::process::exit(2);
                }
                Some(cp)
            }
            Err(e) => {
                eprintln!("[agent] --resume {spec} 失败：{e}");
                log::warn(format!("--resume {spec} 失败：{e}"));
                std::process::exit(2);
            }
        }
    } else {
        None
    };
    // 审批 / 工作区口径随检查点落盘；恢复时按「**最严者胜**」单向合并（2026-10-31）——
    // 任一字段只往更严的方向并，绝不放宽：
    //   · 检查点那次是 `plan`，这次没重传 ⇒ **仍然只读**（防「--continue 忘带档位 = 悄悄放开写」）；
    //   · 检查点那次没开 skip-permissions ⇒ 这次也不替你开；
    //   · 检查点要求走审批卡 ⇒ 补上；
    //   · `--disallowedTools` 取并集、`--add-dir` 取交集（旧检查点没记目录时不做交集）。
    // 每次真的收紧了什么，都原样写进 stderr + agent 日志，不让它变成看不见的行为差异。
    let mut approval = approval_context(&cli);
    if let Some(cp) = &resumed {
        let current = approval.clone();
        let diffs = cp.approval.diff(&current);
        if !diffs.is_empty() {
            eprintln!(
                "[agent] --resume 提示：审批 / 工作区口径与检查点不同（左 = 检查点，右 = 本次；只收紧不放宽）：{}",
                diffs.join("；")
            );
            log::warn(format!("--resume 口径不一致：{}", diffs.join("；")));
        }
        let (tightened, notes) = cp.approval.tightest(&current);
        if !notes.is_empty() {
            apply_approval_context(&mut cli, &tightened);
            approval = approval_context(&cli);
            eprintln!(
                "[agent] --resume 已按「最严者胜」收紧本次口径：{}",
                notes.join("；")
            );
            log::warn(format!("--resume 收紧口径：{}", notes.join("；")));
        }
    }
    let print_query: Option<String> = if single_shot {
        match cli.print_query.clone() {
            Some(q) => Some(q),
            // `--continue` 没给 query ⇒ 用内置「接着做」提问，**不读 stdin**
            // （调度器 / CI 里 stdin 往往是关闭的，读一次会立刻得到空串）。
            None if continue_ok => Some(CONTINUE_PROMPT.to_string()),
            None => {
                // --print 没带值 ⇒ 从 stdin 读**一次**（整段当一条 query，读到 EOF 为止）。
                let mut buf = String::new();
                match std::io::stdin().read_to_string(&mut buf) {
                    Ok(_) => Some(buf),
                    Err(e) => {
                        eprintln!("[agent] --print 读取 stdin 失败: {e}");
                        Some(String::new())
                    }
                }
            }
        }
    } else {
        None
    };
    if single_shot {
        set_output_mode(match cli.output_format.as_str() {
            "json" => OutputMode::Json,
            "stream-json" => OutputMode::StreamJson,
            "text" => OutputMode::Text,
            other => {
                eprintln!("[agent] 未知 --output-format: {other}（按 text 处理）");
                OutputMode::Text
            }
        });
        // 单发才按 CLI 开关决定是否出 `stream_event` 增量行（默认关）；常驻不碰这个量
        // ⇒ 常驻恒 true、逐字节不变（宿主协议不动）。text / json 形态本就把非 result 行
        // 挡在 emit() 里，故「仅 stream-json 才看得到增量」这层由既有过滤自然满足。
        set_include_partial_messages(cli.include_partial_messages);
        set_approval_policy(if cli.skip_permissions {
            ApprovalPolicy::UnattendedAllow
        } else {
            ApprovalPolicy::UnattendedDeny
        });
        // 逐工具放行（2026-10-27，阶段 2B）：只在单发生效；常驻有审批卡，不需要也不接受。
        set_unattended_allow(effective_unattended_allow(&cli.unattended_allow));
    } else if cli.skip_permissions && !cli.ask_permission {
        // 常驻但**没有审批通道**（headless / CI / 直接喂 stdin 的服务）：skip 必须生效，
        // 否则每个写类工具都会干等 300 秒。有 `--permission-prompt-tool stdio` 时仍由
        // 前端审批（桌面端路径不变）。
        set_approval_policy(ApprovalPolicy::UnattendedAllow);
        log::warn("--dangerously-skip-permissions：常驻模式没有审批通道，已按无人值守放行");
    } else if cli.skip_permissions {
        log::warn(
            "--dangerously-skip-permissions 在常驻模式由前端审批；无人值守请去掉 --permission-prompt-tool stdio",
        );
    } else if !cli.unattended_allow.is_empty() {
        log::warn(
            "--unattended-allow 只在单发（--print）下生效：常驻模式有审批卡，参数已忽略",
        );
    }
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
    // worktree 落盘根（A13，2026-10-11）：`<exe 根>\temp\worktrees`。与 output_dir / image_dir
    // 同理进可访问范围 —— 它**在用户工作区之外**（落点从 `<cwd>\.lunac-worktrees` 改到数据根，
    // 就是为了别在用户仓库里显出未跟踪目录），不进这个名单，工作区锁（`guard`）会把 worktree
    // 内的写直接拒掉。**只放行这一个应用自己的目录**，不放宽用户工作区的边界。
    let worktree_dir = tools::prepare_worktree_dir();
    log::info(format!("worktree 落盘根: {}", worktree_dir.display()));
    add_dirs.push(worktree_dir);
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
    // 注意这只。*放行应用自己的目。*，没有放宽用户工作区的边界。
    if let Ok(dir) = std::env::var("LUNAC_MODULES_DIR") {
        let dir = PathBuf::from(dir.trim());
        if !dir.as_os_str().is_empty() {
            add_dirs.push(dir);
        }
    }
    // 定时任务 / 任务队列目录（<exe 根>\ModuleData\tasks，2026-10-17 / 2026-10-22）：
    // `CronList` 读 `cron.json` / `cron-state.json`，`TaskList` 读 `queue.json`；
    // 工作区已配置时它也在工作区之外 —— 与 `LUNAC_MODULES_DIR` 同一机制进可访问范围，
    // 否则工作区锁会让读取被拒。两个变量通常指向同一目录，去重后只放行一次。
    // **只放行这一个目录**，不放宽整个 `ModuleData` / `config`。宿主未注入时跳过（不猜路径）。
    for var in ["LUNAC_CRON_DIR", "LUNAC_TASKS_DIR"] {
        if let Ok(dir) = std::env::var(var) {
            let dir = PathBuf::from(dir.trim());
            if !dir.as_os_str().is_empty() && !add_dirs.contains(&dir) {
                add_dirs.push(dir);
            }
        }
    }
    let mut tools_ctx = tools::Ctx {
        cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        add_dirs,
        read_only: cli.permission_mode == "plan",
        locked: std::env::var("LUNAC_WORKSPACE_LOCKED").ok().as_deref() == Some("1")
            && !cli.skip_permissions,
        // 读禁目录（2026-10-13，桌宠模型目录保护）：宿主注入 `LUNAC_DENY_READ_DIRS`
        //（多路径 `;` 分隔、绝对路径；空则不注入）。启动时解析一次、进程内恒定 ——
        // 与 `locked` 同为「启动时定死」的策略边界。**只拦 Read / Glob / Grep**（见 tools::guard_read）。
        deny_read_dirs: tools::parse_deny_read_dirs(
            &std::env::var("LUNAC_DENY_READ_DIRS").unwrap_or_default(),
        ),
        // 有效工作目录的覆盖层（`EnterWorktree` / `ExitWorktree`，2026-10-11）：起手不在
        // 任何 worktree 里 ⇒ `None`（有效工作目录 = `cwd`）。见 `tools::Ctx::worktree`。
        worktree: Arc::new(Mutex::new(None)),
        // 计划相位（A7）：**进程内**的状态，起手恒为「不在计划相位」——
        // 只读档是另一回事（那是用户档位，见 `read_only` 的注释）。
        plan_phase: Arc::new(AtomicBool::new(false)),
        // 实时输出回传**不在这里装**：它按「每一次工具调用」装上（见 run_one_tool），
        // 因为回调要绑死那一次的 tool_use_id。实时控制（后台运行 / 停止）同理。
        tool_output: None,
        shell_control: None,
        // 记忆作用域（2026-10-13）：起手全局；每个 user 轮开始前按本轮来源重设
        //（见提问循环里的 `tctx.memory_scope = turn_memory_scope(...)`）。
        memory_scope: tools::MemoryScope::Global,
        skill_scope: None,
    };

    // P3 MCP 工具桥：把 <exe 根>\tools\*.json 的用户工具接进工具池。
    // 连不上只是少一批工。—。内置工具必须照常可用，所以这里只记一行。
    //
    // **plan（只读）档也建桥，但不把 `mcp__*` 工具放进工具池**（2026-09-19 调整）：
    // 建桥的理由是桥上还挂着一个**自定义方法** `lunac/history_search`，它支撑只读工具
    // `SessionSearch` —— 只读档查自己的历史完全正当。不列工具的理由没变：MCP 工具在只读档
    // 一律会被 `dispatch_tool` 拒掉，列出来只会让工具清单随档位漂移、白占固定前缀。
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
        // 这条与下面那句「信任询问」是同一件事的两半，分居两个日志文件时，用户问
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
    // sampling（服务端反向调用模型）：**默认声明**，每一台连上的服务器都装一份钩子
    // （宿主桥 / `.mcp.json` 外部 stdio / 远端 HTTP）—— 由 `sampling_handler_for` 每台现场构造。
    // 钩子体 `mcp_sampling_create` 含「高底线审批卡 + 成本护栏」；未就绪时明确拒绝。
    log::info(format!(
        "MCP sampling 默认声明：单次上限 {} tokens、会话上限 {} 次（用户同意一次即本会话内记住，重启即忘）",
        mcp_sampling_u64(MCP_SAMPLING_MAX_TOKENS_ENV, MCP_SAMPLING_DEFAULT_MAX_TOKENS),
        mcp_sampling_u64(MCP_SAMPLING_MAX_CALLS_ENV, MCP_SAMPLING_DEFAULT_MAX_CALLS),
    ));
    let mcp_set = mcp::McpSet::connect(
        cli.mcp_server.as_deref(),
        &remote_servers,
        &cli.disallowed,
        bridge_roots,
        bridge_elicit,
        sampling_handler_for,
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
    // 不能只看集合空不。—。只配了远端服务器时集合非空，但那些方法必然失败。
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
    // 往期会话索引（A2）**自 2026-10-07 起不再注入**：新建对话必须与上个对话在模型侧隔离
    // （每次重启都会把刚结束的那个会话列进索引，模型思考里就带上个对话的痕迹 —— 用户报的
    // 就是这个）。取回侧保留：模型需要时自己调 `SessionSearch`（`lunac/history_search`）。
    // 长期记忆（A4，2026-09-20）：**启动时取一次**的全局冻结快照（桌宠私有那一份按轮现读，
    // 见提问循环里的 `turn_block`）。「写侧工具是否真的在池里」先算出来 —— 注入文案与条件
    // 注册必须同源，否则会出现「提示词让模型调 Remember，但它不在工具表里」这种自相矛盾的组合。
    let remember_on = memory_enabled()
        && has_lunac_bridge
        && !cli.disallowed.iter().any(|d| d == "Remember");
    // 桌宠自我区（2026-10-13 人格地基）：两件写入侧工具**条件注册** —— 与 `Remember` 同判据
    // （没桥就写不进去，注册了也只是必然失败的占位）。运行时**只认桌宠轮**（见 dispatch_tool）。
    let pet_growth_on = has_lunac_bridge && !cli.disallowed.iter().any(|d| d == "PetGrowth");
    let persona_suggest_on =
        has_lunac_bridge && !cli.disallowed.iter().any(|d| d == "PetPersonaSuggest");
    let memory_section = memory_block(mcp_bridge.as_mut(), remember_on, tools::MemoryScope::Global);
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
    let (skills_section, skills_stats) = skills::listing_with_stats(if skills_on { &skills } else { &[] });
    // §待办 3-28.11：超预算 。启动日志一。warn（总数 / 已列 / 未列 + 怎么改）。
    // 并把统计留给 `system/init`（界面据此提示）。没超时零噪音。
    if skills_stats.truncated() {
        log::warn(format!(
            "技能清单超出预算（{} 字符）：共 {} 个技能，只列出 {} 个，{} 个未进系统提示词 —— \
             模型看不到它们；缩短 description 或删掉不用的技能即可全部列出",
            skills::LISTING_BUDGET_CHARS,
            skills_stats.total,
            skills_stats.listed,
            skills_stats.omitted()
        ));
    }
    let _ = SKILLS_LISTING.set(skills_stats);
    let system_prompt = build_system_prompt(&user_persona, &env_section, &skills_section);
    // 上下文块（长期记忆）：**不进 system**（2026-10-06，M2-16 方案 D，
    // 位置与代价见 `context_block_message`）。它由 `run_query` 幂等注入 history 开头。
    //
    // `mut` 只为「压缩边界刷新」（`refresh_memory_snapshot`，`todo.md` §待办 3-9 C）：
    // 会话中途新写的记忆不必重启 agent 才能看见 —— 压缩本来就是「前缀作废」的时刻，
    // 此刻重读一次记忆不额外废缓存。*跨提问持有的就是这一。*，刷新结果必须写回它。
    // 否则下一轮又会把旧块重新注入、记忆来回闪。
    let mut ctx_block = context_block_message(&memory_section);
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
    // 桌宠自我区（2026-10-13 人格地基）：与 `Remember` 同款**条件注册**（判据见上面 `pet_*_on`）。
    // 运行时只认桌宠轮 —— 注册在池里是为了让桌宠轮看得见，非桌宠轮调用会被 `dispatch_tool` 拒。
    if pet_growth_on {
        tool_defs.push(tools::pet_growth_tool());
        tool_names.push("PetGrowth".into());
    }
    if persona_suggest_on {
        tool_defs.push(tools::persona_suggestion_tool());
        tool_names.push("PetPersonaSuggest".into());
    }
    // PTC / Code Mode（§待。3-28.15）：**条件注册** —。宿主开。`LUNAC_PTC=on` 才注册。
    // 关着时模型完全看不到 `RunScript`（注册了就等于在固定前缀里放一件不该出现的工具）。
    if ptc::enabled() && !cli.disallowed.iter().any(|d| d == "RunScript") {
        tool_defs.push(tools::run_script_tool());
        tool_names.push("RunScript".into());
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
            // 。name 稳定排序。026-11-15）：顺序来自桥的注册顺序（读 `tools\*.json` 。
            // 目录遍历顺序），跨进。/ 跨机器可能不。冷启动首轮的前缀与上一进程不一致，
            // 缓存首轮必然全价。排序后同一组工具在任何机器上都拼出同一段前缀。
            let mut defs = b.defs();
            defs.sort_by_key(|d| {
                d.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            });
            tool_names.extend(tools::names(&defs));
            tool_defs.extend(defs);
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

    // 资源库来源工具（A5，2026-10-18）：**只在宿主桥可用时注册** —— 四个工具里两个
    // 走桥（来源目录在宿主），另两个虽本地但只有在「资源库」这套工作流里才有意义。
    // 没桥时注册 = 固定前缀里的空转项（同 resources 读侧的条件注册纪律）。
    if mcp_bridge
        .as_ref()
        .map_or(false, |b| b.has_lunac())
    {
        for def in resource_hub_tools::tool_defs() {
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
        // 写侧是否可用：remember_on 为假（没桥 / 记忆被关 / `Remember` 被黑名单裁掉）时置假
        // = **不跑复盘**。理由：它的**唯一产品是记忆条目** —— 写不进去还每会话花一次 API
        // 调用，是纯粹的浪费（`should_review` 把 `enabled == false` 当关闭）。
        enabled: remember_on,
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
        // 带上 system 的**指纹**：子目录记忆懒加载（2026-10-12）只进可变层、**绝不碰固定前缀**，
        // 这条哈希与「请求前缀」行里的 system 哈希同源（`log::hash64`）—— 两者相等即证据「前缀未变」。
        log::info(format!(
            "固定前缀 system={sys_chars}字/{:016x} tools={}个/{tools_chars}字 合计≈{} tokens（budget={ctx}，占 {:.1}%）",
            log::hash64(&system_prompt),
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

    // MCP sampling 的采样上下文（`detached()` 的副本，与主循环共享 `sub` 用量账）——
    // 钩子在 `McpSet::connect` 时就装好，但那时 `cfg` 还没建，故这一步补上。
    match cfg.detached() {
        Ok(c) => {
            if let Ok(mut g) = MCP_SAMPLING_CFG.lock() {
                *g = Some(c);
            }
        }
        Err(e) => log::warn(format!(
            "MCP sampling 上下文准备失败（sampling 将直接拒绝）：{e}"
        )),
    }

    // ── SessionStart hook（A9）──
    // 落点：配置就绪、还没开始收 stdin。此时发出的 stdout 事件前端**可能还没挂上监听**
    // （宿主刚 spawn 出 agent ⇒ 这一条的产出以落盘日志为主，前端可见为辅）。
    fire_plain_hook("SessionStart", &tools_ctx.cwd, json!({ "model": cfg.model }));

    // stdin 读取线程 → 查询线程（mpsc 解耦）。
    // 解耦的意义：查询线程在等审批回包时会阻塞，stdin 必须另有线程持续 drain，
    // 否则 control_response 根本读不到（这正是 P2 的前提）。
    let (tx, rx) = mpsc::channel::<Value>();
    if single_shot {
        // 单发（--print）：不读 stdin 循环，直接把这一条 query 投进队列、随后**丢掉投递端**
        // —— 主循环处理完这一条后下一拍就收到 Disconnected，与「stdin 关闭」同路优雅退出
        // （复用同一套收尾：collect_review / SessionEnd / 退出码判定）。
        let q = print_query.clone().unwrap_or_default();
        if q.trim().is_empty() {
            eprintln!("[agent] --print 单发模式没有 query（既未给 --print 后的值，stdin 也为空）");
            emit(json!({
                "type": "result",
                "subtype": "error_during_execution",
                "is_error": true,
                "num_turns": 0,
                "result": "--print requires a query argument or non-empty stdin",
                "session_id": session_id(),
            }));
            std::process::exit(2);
        }
        let _ = tx.send(json!({
            "type": "user",
            "session_id": "",
            "message": { "role": "user", "content": [{ "type": "text", "text": q }] },
            "parent_tool_use_id": null,
        }));
        drop(tx);
    } else {
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
                        // 暂停。026-11-14）：如果此刻正卡在审批上，先把所有待答审。
                        // 按「拒绝并中断」送回去，。await_approval 立刻醒来。
                        // pause 本身**不再进主队列** —。否则它会留到下一轮，刚开始就暂停。
                        if v.get("type").and_then(Value::as_str) == Some("pause")
                            && deny_all_pending_approvals("用户暂停了对话")
                        {
                            PAUSED_AT_APPROVAL.store(true, Ordering::SeqCst);
                            continue;
                        }
                        // 审批回包直接投给等它的工具调用，不进主队列
                        if route_control_response(&v) {
                            continue;
                        }
                        // 运行中命令的实时指令（2026-10-01，`tool_control`）：同样是「不占
                        // 对话、只改某个正在跑的东西的状态」，所以也不进主队列 ——
                        // 对话、只改某个正在跑的东西的状态」，所以也不进主队列 ——
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
    }

    // system/init 由 run_query 每轮发出（前端据此推进 agentState），启动时不必发
    // `--resume`：history 直接接着上次的检查点跑（没有该开关时为空，行为逐字节不变）。
    let mut history: Vec<Value> = resumed
        .as_ref()
        .map(|cp| cp.history.clone())
        .unwrap_or_default();
    // ── 跨提问的前缀归因状态（A16，2026-09-21）─────────────────────────
    // `prev_hist_hashes` = 上一次请求的 history **逐条**指纹，必须**跨提问**存活：
    // 原先它声明在 `run_query` 内部（局部）⇒ 每一问的首请求都看不到上一问末请求的
    // 指纹，日志里只剩 `公共前缀=0/0条`，于是「两次提问之间本侧有没有改写 history」
    // 这件事**在日志里根本没有证据**（这正是 A16 一开始卡住的地方）。
    // `question_seq` = 第几次用户提问（只进日志，让离线时能一眼认出「本问首请求」）。
    let mut prev_hist_hashes: Vec<u64> = Vec::new();
    // 提问序号接着检查点编号（`--resume`）；否则从 0 开始。
    let mut question_seq: u64 = resumed.as_ref().map(|cp| cp.question_seq).unwrap_or(0);
    // 任务级预算（S2 第一刀，2026-10-22）：**跨提问存活**，`LUNAC_TASK_BUDGET_TOKENS`
    // 缺省 0 = 关闭。放在主循环外是语义要求 —— 每一问各自 new 一份就退化成回合预算。
    // `--resume` 时把检查点里的累计值接上（否则「跨进程续跑」会把已花掉的钱一笔勾销）；
    // 当前进程没有开任务级预算（limit=0）则保持关闭，不让旧文件把闸门偷偷打开。
    let mut task_budget = TaskBudget::new(task_budget_tokens());
    if let Some(cp) = &resumed {
        if task_budget.limit > 0 {
            task_budget.used = cp.task_budget_used;
        }
    }
    // ── 后台复盘（A4）的装配状态 ────────────────────────────────────
    // `pending_review` = 上一次复盘还没收回来。**收的时机是「处理下一条消息之前」，且只收
    // 已经跑完的**（见 `collect_review`）—— 复盘是后台事务，不许它决定前台时延。
    // `reviewed_this_session` = 本次会话（= 本进程）是否已复盘过，**按作用域各一份**
    // （global / pet；即「每会话每 scope ≤ 1 次」，2026-10-13 配额折中）。`last_review_ts` =
    // 上次复盘时间（epoch 秒），**两侧共享同一个值**，启动时从 temp\review-last 读一次 ——
    // ── 后台复盘（A4）的装配状态 ────────────────────────────────────
    let mut review_seq: u64 = 0;
    let mut pending_review: Option<thread::JoinHandle<Result<String, String>>> = None;
    let mut reviewed_this_session = [false, false];
    let mut last_review_ts: Option<i64> = load_last_review_ts();
    // 项目 MCP 的信任门**只过一次**（首次真实提问前）：接上后工具表就是固定前缀的一部分，
    // 之后不再变（见 `connect_project_servers`）。
    let mut project_trust_done = false;
    // 主循环改成「带超时的接收」（2026-10-06）：`for msg in rx` 在空闲时会一直阻塞，而
    // 「**回合外**改了 MCP 配置」只有靠**空闲醒来**才能发现 —— 否则要等用户下次提问才发现，
    // 那时工具表已定死，只能白答一轮再重启（实测踩到，见 ai-spec §20.1）。
    // 超时分支不处理任何消息，只做一次 watcher 比对（报过后 `Option` 已置 `None`，零成本）；
    // 断开与原「迭代器结束」语义相同。
    //
    // 冷启动预热（S1，2026-10-15）：进主循环**之前**丢一个后台线程预热本地判定端点 ——
    // 趁用户还没提问，把 `llama-server` 的冷启开销吃掉，让「启动后第一次真实判定」落在
    // `LOCAL_DECIDE_TIMEOUT`（当前 3000ms）内。**绝不阻塞启动 / 主循环**；默认关 ⇒ 零请求。
    spawn_local_decide_warmup();
    // 回合中被 `run_query` 分流出来的消息（非插入类）暂存在这里，回合结束后按原顺序处理，
    // 保证「不丢、不乱序」（§待办 3-28.1）。
    let mut backlog: VecDeque<Value> = VecDeque::new();
    loop {
        let msg = match backlog.pop_front() {
            Some(m) => m,
            None => match rx.recv_timeout(MCP_WATCH_TICK) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(ev) = mcp_config_changed_event(&mut mcp_config_watch, true) {
                        emit(ev);
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            },
        };
        collect_review(&mut pending_review);
        match stdin_kind(&msg) {
            StdinKind::User => {
                // 回合**已结束**才到的「插入」消息（通常撞上预算 / 轮次硬收口，没赶上安全点）：
                // 不在无回合状态下偷偷跑它 —— 前端没有回合状态，回答渲染不出来。如实告知，
                // 由用户决定是否重发；不带 `steer` 的普通 user 完全不受影响。
                if msg.get("steer").and_then(Value::as_str) == Some("insert") {
                    let text = extract_user_text(&msg);
                    log::warn(format!(
                        "回合已结束，丢弃未消费的插入消息：{} 字",
                        text.chars().count()
                    ));
                    emit(json!({
                        "type": "system",
                        "subtype": "steer_dropped",
                        "chars": text.chars().count() as u64,
                    }));
                    continue;
                }
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
                // 单发模式跳过：信任门要弹卡等回包，而无头单发没有回包通道（见 unattended_deny_reason）。
                if !single_shot && !project_trust_done {
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
                    }));
                    continue;
                }
                question_seq += 1;
                // 「按轮思考覆盖」（2026-10-09，用户口径）：**桌宠那一轮一律关 thinking**。
                // 前端在该轮消息里带 `"thinking":"off"`；缺省（普通聊天）= 用全局开关
                // （`cfg.thinking`）。语义**只认 `off`**（其余值 = 不覆盖）—— 目的就是「按轮关」，
                // 不需要「按轮开」（全局开关已经能开）。
                let thinking_override =
                    thinking_override_from(msg.get("thinking").and_then(Value::as_str));
                // 记忆作用域（2026-10-13）：桌宠发起的那一轮读写**桌宠私有**那一份，其余用
                // 全局。写进 `tools_ctx`（`Remember` / 子代理 / 后台复盘 fork 都从它取），
                // 并据此决定本轮注入哪一份记忆 —— 两份**互不注入**。
                let turn_scope = turn_memory_scope(&msg);
                tools_ctx.memory_scope = turn_scope;
                // 成长笔记的「每轮 ≤ 1 条」闸随本轮复位（见 `PET_GROWTH_USED_THIS_TURN`）。
                PET_GROWTH_USED_THIS_TURN.store(false, Ordering::Relaxed);
                // 全局轮用进程内的**快照本体**；桌宠轮**按轮现读**（桌宠笔记 / 成长笔记是
                // 它会话里现长出来的，冻结会让「刚记的当轮看不见」）。
                //
                // 2026-10-23（§待办 3-9 C）：全局轮不再传 `ctx_block` 的副本 —— 压缩边界
                // `refresh_memory_snapshot` 会把刷新结果写回它，副本会让下一轮退回旧快照。
                // 桌宠轮仍是一个只在本轮有效的局部块（下一轮反正重读）。
                // 桌宠轮仍是一个只在本轮有效的局部块（下一轮反正重读）。
                let mut pet_block: Option<Value>;
                let block_ref: &mut Option<Value> =
                    if matches!(turn_scope, tools::MemoryScope::Global) {
                        &mut ctx_block
                    } else {
                        let section = memory_block(mcp_bridge.as_mut(), remember_on, turn_scope);
                        let growth = growth_block(mcp_bridge.as_mut(), pet_growth_on);
                        pet_block = context_block_message(&format!("{section}{growth}"));
                        &mut pet_block
                    };
                run_query(
                    &cfg,
                    &mut history,
                    &mut prev_hist_hashes,
                    question_seq,
                    checkpointing,
                    &approval,
                    &mut task_budget,
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
                    block_ref,
                    thinking_override,
                    // 中途三态控制（§待办 3-28.1）：轮首分流 stdin —— 插入的消息进本轮
                    // history，暂停在安全点收口；其余消息暂存 backlog 交给主循环。
                    &rx,
                    &mut backlog,
                );
                // S2 检查点（2026-10-22）：回合收尾后落一份（含本轮新长出来的 history 与
                // 任务级预算累计）。默认关；写失败只 warn —— 续跑是增强，不能反过来中断对话。
                save_checkpoint(
                    checkpointing,
                    question_seq,
                    task_budget.used,
                    &approval,
                    &history,
                );
                // 里程碑写入（结论索引）：一轮提问收尾后把最近窗口的结论确定性落盘（**不调模型**），
                // 在它们可能被压缩丢掉之前先保住。见 `flush_turn_conclusions`。
                flush_turn_conclusions(turn_scope, &history, question_seq);
                // 记忆候选抽取（S1 第二切片，2026-10-15）：**每 K 轮**触发一次 —— 复用 S1 的
                // 摊销口径（`prescreen_due` + `decide::amortize_k`），**不新造计时器**。
                // **只落账本**：绝不写 MEMORY.md / 绝不改任何注入（见 `extract_candidates`）。
                // 单发不抽（进程马上退出、白等一次本地调用）；开关关 / 未配端点 ⇒ 零请求零账本行。
                if !single_shot && prescreen_due(question_seq, decide::amortize_k()) {
                    extract_candidates(&history, turn_scope, question_seq);
                }
                // 能力路由（S1 第五切片，2026-10-24）：同样复用这套摊销，但**错开一轮**
                // （`question_seq + 1`）⇒ 不在同一轮里连续打两次本地端点。**只记账**：
                // 路由结果没有任何消费者，绝不影响本轮的模型请求或工具选择。
                if !single_shot && prescreen_due(question_seq + 1, decide::amortize_k()) {
                    route_capability(&prompt, turn_scope, question_seq);
                }
                // 复盘的四个前置（方案 B，2026-10-11；2026-10-13 折中把每会话配额按 scope 拆）：
                // ① 本轮确实完成了一次用户提问（本代码位置即保证 —— 它在 run_query 返回之后，
                // 不在工具轮中途）；② 不是只读（plan）档、也不在计划相位（复盘的唯一产品就是
                // 一条 `Remember` 写入，那两档下 `write_blocked` 会直接拒 ⇒ 派出去也是白烧一次
                // API）；③ 写侧可用 + 距上次复盘 ≥ 30 分钟（时间闸两侧共享）+ **本轮 scope 这一份**
                // 的「每会话 ≤ 1 次」还没消耗（见 should_review）；④ 上一轮那次已经收干净
                // （pending 非空就不再派，防两条复盘并发）。
                // 「当前记忆」在主线程读（那条桥归主线程）—— 复盘靠它避免重复写。
                // 单发模式不派复盘：进程马上就要退出，后台线程会被直接杀掉，
                // 那次云调用纯属白烧（单发的语义就是「跑一轮、拿结果、退出」）。
                if !single_shot
                    && pending_review.is_none()
                    && !tools_ctx.read_only
                    && !tools_ctx.plan_phase.load(Ordering::Relaxed)
                    && should_review(
                        review_setup.enabled,
                        reviewed_this_session,
                        last_review_ts,
                        now_epoch_secs(),
                        turn_scope,
                    )
                {
                    // 「当前记忆」按**本轮作用域**读（桌宠轮读桌宠私有那份）：复盘据此避免
                    // 重复写、且写回的也是同一份（`tools_ctx.memory_scope` 已随本轮设好，
                    // 「当前记忆」按**本轮作用域**读（桌宠轮读桌宠私有那份）：复盘据此避免
                    let memory = fetch_memory(mcp_bridge.as_mut(), turn_scope).unwrap_or_default();
                    review_seq += 1;
                    // spawn 前先吃掉**本轮 scope** 的会话配额（无论派没派成，本 scope 本会话都
                    // 不再重试）——「每会话每 scope ≤ 1 次」是硬上限。只有**真派出去**了才记
                    // 「上次复盘时间」（写侧发生了一次云调用，下一次要隔 30 分钟）；派不出去
                    // （配置副本建不出 / 快照为空）不记时间，于是下个会话仍可再来一次。
                    // ⚠️ 本地只读预筛判 SKIP 时**照样**吃配额（就在这一行）——保持幂等语义，
                    // （配置副本建不出 / 快照为空）不记时间，于是下个会话仍可再来一次。
                    reviewed_this_session[turn_scope.index()] = true;
                    // ── 本地只读预筛（S1 首个切片，2026-10-15）──
                    // 只在「每 K 轮」到点时判定（K = LUNAC_LOCAL_DECIDE_K，默认 5）。
                    // SKIP ⇒ 本轮**不派**复盘（省一次云调用）；None（整层关闭 / 未配端点 /
                    // 未到点）/ RUN / 任何故障 ⇒ **照旧派**（fail-open）。判定只影响控制流与
                    // 账本，**绝不改 history / 固定前缀**（见 `prescreen`）。
                    let verdict = if decide::enabled()
                        && prescreen_due(question_seq, decide::amortize_k())
                    {
                        prescreen(&prompt, "prescreen-review", turn_scope, question_seq)
                    } else {
                        None
                    };
                    if allow_review_spawn(verdict) {
                        if let Some(handle) =
                            review_setup.spawn(&cfg, &tools_ctx, &history, &memory, review_seq)
                        {
                            pending_review = Some(handle);
                            let now = now_epoch_secs();
                            last_review_ts = Some(now);
                            save_last_review_ts(now);
                        }
                    } else {
                        log::info(format!(
                            "本地只读预筛判 SKIP：本轮（q{question_seq}）不派后台复盘（配额已吃）"
                        ));
                    }
                }
            }
            // 会话历史整体替换（宿主 → agent，2026-09-17）。
            // 用途：① 回退历史后让 agent 的上下文与界面**保留下来的那部分**一致。
            // 旧实现靠宿主侧 `stop_cli` + `start_cli` 把上下文整个清空，代价是保留下来
            // 的上文也一起丢。—。用户表现为「回退后引用不到上文」。
            // ② 从磁盘恢复一个旧会话后把它的消息灌回来（恢复只重建了 DOM，agent 侧为空）。
            // **只替换历史、不触发模型调用**：这不是一次提问，不该产生回答与 token。
            StdinKind::SetHistory => {
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
                // 回退 / 恢复会话也会整份换掉 history —— 检查点必须跟着换，否则
                // `--resume` 会把用户已经回退掉的内容又续回来。
                save_checkpoint(checkpointing, question_seq, task_budget.used, &approval, &history);
            }
            // 没被认领的 control_response（如请求已超时）在这里丢弃即可
            StdinKind::ControlResponse => {}
            // 空闲时收到 pause：没有正在跑的回合，无事可做（不回错 —— 前端可能与本轮
            // 的 result 抢占几毫秒，属于正常竞态）。
            StdinKind::Pause => {
                log::info("收到 pause，但当前没有运行中的回合（忽略）");
            }
            // 手动压缩一次（2026-10-11，stdin 输入 {"type":"compact"}）：复用自动压缩的
            // Force 路径，压动与否都保证回一条 system/context_compacted（前端已有该事件渲染）。
            StdinKind::Compact => {
                let (receipt, compacted) =
                    manual_compact(&cfg, &mut history, &tools_ctx.cwd, tools_ctx.memory_scope);
                if let Some(ev) = receipt {
                    emit(ev);
                }
                // 手动压缩同样是一个压缩边界（§待办 3-9 C）：真压动过就重读一次记忆，
                // 让「会话中途写的记忆」不必重启 agent 就能被下一问看见。
                if compacted {
                    refresh_memory_snapshot(
                        &mut history,
                        &mut ctx_block,
                        mcp_bridge.as_mut(),
                        remember_on,
                        pet_growth_on,
                        tools_ctx.memory_scope,
                    );
                }
            }
            StdinKind::Unknown(other) => eprintln!("[agent] 忽略输入类型: {other:?}"),
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
    log::info(if single_shot {
        "=== agent exit (single-shot) ==="
    } else {
        "=== agent exit (stdin closed) ==="
    });
    // ── SessionEnd hook（A9）──
    // 上游（lunac.exe）已退出 / 主动关了 stdin ⇒ 这个 agent 进程的生命周期结束。
    // 同样是「只做事、拦不住」的一类：脚本可以用它清理临时资源、发通知。
    fire_plain_hook("SessionEnd", &tools_ctx.cwd, json!({}));
    // 单发（--print）退出码：成功 0；失败 1；带检查点时「清单未完成」= 3
    // （任何 is_error 的 result 或未放行的需审批工具都会置 SINGLE_SHOT_FAILED）。
    if single_shot {
        let leftover = todo_has_unfinished(&history);
        let code = single_shot_exit_code(
            SINGLE_SHOT_FAILED.load(Ordering::Relaxed),
            leftover,
            checkpointing,
        );
        if code == 3 {
            log::info("单发退出码 3：TodoWrite 清单未完成（可用 --continue 续跑）");
        }
        std::process::exit(code);
    }
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

/// 单发模式（--print，无审批回包通道）下，一个「需审批的工具」被拒绝时的原因文本。
///
/// 为什么默认拒绝而不是默认放行：单发没有前端回 control_response 的通道，若照常发
/// control_request 就会**静默挂起**到 APPROVAL_TIMEOUT_SECS（300s）才按拒绝收场 ——
/// 用户看到的是「卡死」。默认拒绝 + 明确告知 + 非 0 退出，是最安全的默认：
/// 放行写类工具是危险动作，绝不能因为「无人值守」就默默降级为自动允许。
/// 确需放行的场景（如自建的无头流水线）用 `--unattended-allow Write,Edit,NotebookEdit`
/// 逐工具点名（只覆盖写文件三件），或用 `--dangerously-skip-permissions` 整体承担。
fn unattended_deny_reason(tool_name: &str) -> String {
    SINGLE_SHOT_FAILED.store(true, Ordering::Relaxed);
    eprintln!(
        "[agent] 单发模式（--print）无审批通道，已拒绝需审批的工具: {tool_name}\
         （如确需放行，加 --unattended-allow {tool_name}；命令类只能改用 --dangerously-skip-permissions）"
    );
    format!(
        "Denied: {tool_name} needs user approval, but --print single-shot mode has no approval \
         channel. Re-run with --unattended-allow {tool_name} (write tools only) if this tool is \
         intended, or --dangerously-skip-permissions for the whole session."
    )
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
    // 安全预筛（S1 第四切片，2026-10-24）：**只记账**、一律后台线程 —— 不在工具路径上等待，
    // 也不改审批 / 结果。开关、判据（`needs_approval`）、采样都在 `maybe_safety_prescreen` 收口。
    maybe_safety_prescreen(name, input);
    let started = Instant::now();
    // `MemorySearch` 的**查询词可能含敏感词** ⇒ 日志只留长度 + 哈希（见
    // `memory_search_log_args` 的理由）；其余工具仍走既有脱敏摘要。
    let args = if name == "MemorySearch" {
        memory_search_log_args(input)
    } else {
        tools::summarize_args(input)
    };
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
    // 2026-11-20：图书馆索引（日期 → 对话 → 任务 → 操作）的可选收窄条件。
    // 只做形状校验与透传，检索规则归宿主（单一真相源）。
    let mut params = serde_json::json!({ "query": query, "limit": limit });
    if let Some(tags) = input.get("tags").and_then(Value::as_array) {
        let tags: Vec<&str> = tags
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .collect();
        if !tags.is_empty() {
            params["tags"] = serde_json::json!(tags);
        }
    }
    for key in ["date", "session", "task"] {
        if let Some(text) = input.get(key).and_then(Value::as_str).map(str::trim) {
            if !text.is_empty() {
                params[key] = serde_json::json!(text);
            }
        }
    }
    let Some(b) = bridge else {
        // 桥没接通（没传 --mcp-server，或 lunac.exe 起不来）⇒ 如实报错，**不编造结果**。
        return Err("往期会话检索不可用：MCP 桥未接通".into());
    };
    b.history_search(&params)
}

/// `MemorySearch` 单次返回条数的默认值与上限。
///
/// 与 `SESSION_SEARCH_*` 分开定义：记忆文件整份封顶 9000 字符（`MEMORY_INJECT_CHARS` 或
/// 服务端总长硬顶），条目数天然有限；这里仍设 30 的硬上限，是为了防「用户手工把文件改大」时一次检索
/// 把整段上下文吃掉（与 `SessionSearch` 同一口径，模型只需记一套条数）。
const MEMORY_SEARCH_DEFAULT_LIMIT: u64 = 10;
const MEMORY_SEARCH_MAX_LIMIT: u64 = 30;

/// `MemorySearch` 的 scope 日志标签：只认白名单，未知一律 `invalid`。
///
/// 日志里不落任意输入（同「查询只记长度 / 哈希」的理由）；这里只输出固定枚举值。
fn memory_search_scope_label(scope: &str) -> &'static str {
    match scope.trim().to_ascii_lowercase().as_str() {
        "" | "all" | "both" => "all",
        "global" => "global",
        "pet" => "pet",
        _ => "invalid",
    }
}

/// `MemorySearch` 的日志参数摘要：**刻意不落查询全文**。
///
/// 理由：查询词可能含用户敏感信息（地址 / 项目名 / 凭据片段）；日志常被用户贴出来求助，
/// 记全文等于把它们带出门。长度 + FNV 哈希足以做「同一查询重复了几次 / 是不是同一个词」
/// 的归因，又不可还原原文（与 `log::hash64` 的既有用途同口径）。
fn memory_search_log_args(input: &Value) -> String {
    let q = input.get("query").and_then(Value::as_str).unwrap_or("");
    let scope = input.get("scope").and_then(Value::as_str).unwrap_or("");
    format!(
        "query_len={} query_hash={:016x} scope={}",
        q.chars().count(),
        log::hash64(q),
        memory_search_scope_label(scope)
    )
}

/// `MemorySearch`：长期记忆的只读检索（§待办 3-9 B 项，2026-10-17）。
///
/// **只读**：正文走**既有** `fetch_memory`（`lunac/memory_read` 桥），检索内核是纯函数
/// `memory_search::search`；不写文件、不调 `Remember`、不改注入路径、不产生自动调用点。
/// 失败一律回**空结果 + reason**（桥不可用 / 记忆关闭 / 无匹配 / 参数不合法），不 panic、
/// 不抛硬错误 —— 检索不该阻断任何一轮对话。
fn memory_search(bridge: Option<&mut mcp::McpSet>, input: &Value) -> String {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(MEMORY_SEARCH_DEFAULT_LIMIT)
        .clamp(1, MEMORY_SEARCH_MAX_LIMIT) as usize;
    // scope 缺省 = 两份都查；`all` / `both` 是同一语义的显式写法。
    let scope_arg = input
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let scope_label = memory_search_scope_label(&scope_arg);

    let (hits, reason): (Vec<(tools::MemoryScope, memory_search::Hit)>, &str) = if query.is_empty()
    {
        (Vec::new(), "missing-query")
    } else if !memory_enabled() {
        (Vec::new(), "memory-disabled")
    } else {
        let scopes: Option<Vec<tools::MemoryScope>> = match scope_arg.as_str() {
            "" | "all" | "both" => Some(vec![tools::MemoryScope::Global, tools::MemoryScope::Pet]),
            "global" => Some(vec![tools::MemoryScope::Global]),
            "pet" => Some(vec![tools::MemoryScope::Pet]),
            // 未知 scope 不猜：回空结果 + 明确原因，模型可自行改成合法值重试。
            _ => None,
        };
        match scopes {
            None => (Vec::new(), "invalid-scope"),
            Some(scopes) => match bridge {
                None => (Vec::new(), "bridge-unavailable"),
                Some(b) => {
                    // 先按 scope 读一次正文（复用既有读取函数；错误 / 空记忆都折成 None，
                    // 与注入侧同口径）。保留 bullets 是为了随后可能做的「扩展词补充检索」。
                    let mut per_scope: Vec<(tools::MemoryScope, Vec<String>)> = Vec::new();
                    for scope in scopes {
                        if let Some(raw) = fetch_memory(Some(&mut *b), scope) {
                            per_scope.push((scope, memory_search::split_bullets(&raw)));
                        }
                    }
                    let had_text = !per_scope.is_empty();
                    let mut hits: Vec<(tools::MemoryScope, memory_search::Hit)> = Vec::new();
                    for (scope, bullets) in &per_scope {
                        for hit in memory_search::search(bullets, query, limit) {
                            hits.push((*scope, hit));
                        }
                    }
                    // 跨 scope 用**同一把**排序尺子（`rank_cmp`），再统一截到 limit。
                    hits.sort_by(|a, b| memory_search::rank_cmp(&a.1, &b.1));
                    hits.truncate(limit);
                    // **补充召回**（S1 第三切片，2026-10-24）：只在原查询**没填满 limit** 时才问
                    // 本地模型要扩展词；扩展命中**只追加、绝不挤掉原命中**（见 `merge_expansion_hits`）。
                    // 开关关 / 未配端点 ⇒ `None`（零请求、零账本行）；故障 ⇒ 空扩展（fail-open）。
                    if hits.len() < limit && !per_scope.is_empty() {
                        if let Some(exp) = rewrite_memory_query(query) {
                            if !exp.terms.is_empty() {
                                let expanded = exp.terms.join(" ");
                                let mut extra: Vec<(tools::MemoryScope, memory_search::Hit)> =
                                    Vec::new();
                                for (scope, bullets) in &per_scope {
                                    for hit in memory_search::search(bullets, &expanded, limit) {
                                        extra.push((*scope, hit));
                                    }
                                }
                                merge_expansion_hits(&mut hits, extra, limit);
                            }
                        }
                    }
                    let reason = if hits.is_empty() {
                        if had_text {
                            "no-match"
                        } else {
                            "empty-or-missing"
                        }
                    } else {
                        "ok"
                    };
                    (hits, reason)
                }
            },
        }
    };

    // 只记条数 / scope / 原因 / 查询长度与哈希 —— **绝不记查询全文**（理由见上）。
    log::info(format!(
        "MemorySearch 命中 {} 条（scope={scope_label}，reason={reason}，query_len={}，query_hash={:016x}）",
        hits.len(),
        query.chars().count(),
        log::hash64(query)
    ));
    let results: Vec<Value> = hits
        .iter()
        .map(|(scope, hit)| {
            json!({
                "scope": scope.as_str(),
                "text": hit.text,
                "score": {
                    "matched_terms": hit.matched_terms,
                    "query_terms": hit.query_terms,
                    "occurrences": hit.occurrences,
                },
                "why": format!(
                    "matched {}/{} terms, {} occurrence(s)",
                    hit.matched_terms, hit.query_terms, hit.occurrences
                ),
            })
        })
        .collect();
    json!({ "results": results, "reason": reason }).to_string()
}

/// 把「扩展词检索到的命中」并进原命中：**只追加、绝不挤掉原命中**（S1 第三切片）。
///
/// 三条规矩：
///   · 同 scope 下同一条 bullet（`text` 相同）已在原命中里 ⇒ 丢弃扩展那一条（原查询的分数更可信）；
///   · 新追加的那一段用**同一把**尺子 `rank_cmp` 排序（不跨段重排，原命中的顺序逐条不变）；
///   · 最后统一截到 `limit` —— 调用点保证进入时 `orig.len() < limit`，故原命中一条都不会被截掉。
fn merge_expansion_hits(
    orig: &mut Vec<(tools::MemoryScope, memory_search::Hit)>,
    extra: Vec<(tools::MemoryScope, memory_search::Hit)>,
    limit: usize,
) {
    let base = orig.len();
    for (scope, hit) in extra {
        if orig.iter().any(|(s, h)| *s == scope && h.text == hit.text) {
            continue;
        }
        orig.push((scope, hit));
    }
    orig[base..].sort_by(|a, b| memory_search::rank_cmp(&a.1, &b.1));
    orig.truncate(limit);
}

/// 一次检索查询改写：跑改写 + 记决策账本，**永远返回结果**（可注入 rewriter，便于单测）。
///
/// 账本只落**扩展词的 hash + 长度**与输入的 hash，**绝不落原文**（查询可能含敏感信息）。
/// `turn` 记 0：`MemorySearch` 是模型在回合中间调用的工具，拿不到「第几次用户提问」
/// （与手动压缩同为无提问号场景，见 `decisions::DecideRecord::turn` 的注释）。
fn run_query_rewrite(
    r: &dyn query_rewrite::Rewriter,
    model: &str,
    query: &str,
    dir: &Path,
) -> query_rewrite::Expansion {
    let t0 = Instant::now();
    let out = query_rewrite::expand(r, query);
    let latency_ms = t0.elapsed().as_millis() as u64;
    let verdict = if out.fallback {
        "fallback"
    } else if out.terms.is_empty() {
        "empty"
    } else {
        "ok"
    };
    let candidates: Vec<decisions::CandidateRef> = out
        .terms
        .iter()
        .map(|t| decisions::CandidateRef {
            hash: format!("{:016x}", log::hash64(t)),
            len: t.chars().count() as u32,
        })
        .collect();
    let rec = decisions::DecideRecord {
        ts: now_epoch_secs(),
        kind: "query-rewrite".to_string(),
        input_hash: format!("{:016x}", log::hash64(query)),
        verdict: verdict.to_string(),
        latency_ms,
        model: model.to_string(),
        reason: out.reason.to_string(),
        fallback: out.fallback,
        scope: tools::MemoryScope::Global.as_str().to_string(),
        turn: 0,
        session_id: session_id().to_string(),
        candidate_count: Some(candidates.len() as u32),
        candidates: Some(candidates),
        tool: None,
    };
    decisions::append(dir, tools::MemoryScope::Global, &rec);
    out
}

/// 生产入口：**开关关 / 未配端点 ⇒ `None`**（零请求、零账本行）。
///
/// 开启时**总是记一行账本**（含 fail-open / 空扩展），口径与 `prescreen` /
/// `extract_candidates` 一致 —— 留痕比静默更有用，也保住「每次调用恰好一行」的不变量。
fn rewrite_memory_query(query: &str) -> Option<query_rewrite::Expansion> {
    if !decide::enabled() {
        return None;
    }
    let cfg = decide::LocalHttpConfig::from_env()?;
    let model = cfg.model.clone();
    let dir = decisions::default_dir();
    Some(run_query_rewrite(
        &query_rewrite::LocalRewrite::new(cfg),
        &model,
        query,
        &dir,
    ))
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
    /// 是否把子代理的**内部过程**（思考 / 文本 / 工具调用与结果）按 `task_event` 事件转发到前端。
    ///
    /// **`Agent` 工具调用（`run_agent_tool`）与 fork 技能（`run_forked_skill`）为 `true`**，
    /// **后台复盘（`run_review_fork`）为 `false`**。这是「静默不变量」的**唯一判据**：`false` 时
    /// `run_subagent` 连转发器都不构造，一个 `task_event` 都不发（复盘是无人值守的静默 fork）。
    /// 取值由调用方经具名常量给出（`AGENT_EMIT_SUBTASK` / `FORK_SKILL_EMIT_SUBTASK` /
    /// `REVIEW_EMIT_SUBTASK`），便于单测钉住「谁转发、谁静默」。
    ///
    /// 与 `emit_progress` **不是同一个概念**，故不合并：fork 技能的 `emit_progress` 是 `true`
    /// （进度要回前端），其内部过程现在**也**转发（进前端保留的 `.subtask-panel` 行）；后台复盘
    /// 两者都是 `false`。两个判据若合成一份就会走偏。
    emit_subtask: bool,
    /// 多代理通信（A13，2026-10-05）：`Some(描述)` = 把这次 fork 登记进 peer 登记处
    /// （同批兄弟可 `ListPeers` 看到它、`SendMessage` 投给它）；`None` = 不登记。
    /// 前台子代理（`Agent` / fork 技能）传 `Some`，**后台复盘传 `None`** —— 它在提问
    /// 之间单独跑、没有兄弟，也不该出现在别人的列表里。
    peer: Option<&'a str>,
    /// 用量记进哪本账（2026-10-27，§待办 3-16 P2-8）。
    ///
    /// 子代理（`Agent` / fork 技能）与**后台复盘**烧的是同一张卡，但回答的是两个问题：
    /// 返回 `Err("已被用户终止")`。前台两条路径传 `None`（它们的生命周期就是工具调用本身。
    /// 用户要停就停整个回合）。*为什么不。`&'a AtomicBool`**：标志的持有者是后台线程
    /// 自己，借用会把它和 `spec` 的生命周期绑死。
    cancel: Option<Arc<AtomicBool>>,
    /// 用量记进哪本账（2026-10-27，§待。3-16 P2-8）。
    ///
    /// 子代理（`Agent` / fork 技能）与**后台复盘**烧的是同一张卡，但回答的是两个问题：
    /// 「钱花在探索上还是收口上」。此前两者混在一条日志里，实验复盘时无法归因
    /// （见 `cache-hit-experiment.md` §6.4 第 9 条）。
    usage: UsageKind,
}

/// fork 的用量归属。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageKind {
    /// `Agent` 工具 / fork 技能：用户看得见的那种「派出去干活」。
    Subagent,
    /// 提问之间的后台复盘 fork：无人值守，只写长期记忆。
    Review,
}

/// 子代理退出时把 peer 摘掉（RAII）。`thread::scope` 的 `join()` 会吞掉 worker panic ⇒
/// 手写注销会留下僵尸 peer，之后 `SendMessage` 就一直往一个没人读的收件箱里投。
struct PeerReg(String);

impl Drop for PeerReg {
    fn drop(&mut self) {
        peers::bus().unregister(&self.0);
    }
}

// ── 子代理内部过程转发（E1，2026-10-16）────────────────────────────
//
// 把子代理**内部**的思考 / 文本 / 工具调用与结果，按发生顺序转成 `task_event` 事件发给前端，
// 让「`Agent` 工具卡」展开后能看到它在干什么（E2 负责渲染，本步只铺数据通道）。
//
// 三条纪律：
//   · **复用既有 `system` 事件通道**（`subtype: "task_event"`），不另开 IPC；块内字段名与主流程
//     的 `assistant` / `user` 事件**逐字对齐**（`message.content[].type` 等），前端解析可尽量复用。
//   · **绝不进主对话**：这些事件只写给前端，`run_subagent` 自己的 history / 主循环的 history
//     一个字都不改 ⇒ 固定前缀缓存不变量不受影响（§规范 6）。
//   · **静默不变量**：转发与否只由 `ForkSpec::emit_subtask` 决定（`Agent` 工具调用与 fork 技能
//     为 `true`；后台复盘为 `false` ⇒ 零事件）。

/// 子代理内部过程转发的**唯一闸门**：`emit_subtask` 为真才构造转发器，否则返回 `None`
/// ⇒ `run_subagent` 既不构造也不发送，一个 `task_event` 都不出（静默不变量）。
fn subtask_forwarder(emit_subtask: bool) -> Option<SubtaskForward> {
    emit_subtask.then(SubtaskForward::new)
}

/// 各调用方的 `ForkSpec::emit_subtask` 策略 —— **单一真相源**，可被单测钉住：
/// `Agent` 工具调用与 fork 技能都转发内部过程；后台复盘保持静默（无人值守的后台过程零事件）。
const AGENT_EMIT_SUBTASK: bool = true;
const FORK_SKILL_EMIT_SUBTASK: bool = true;
const REVIEW_EMIT_SUBTASK: bool = false;

/// 把任意 JSON 值按字符上限收口（内部过程转发的单条上限）。字符串直接 `log::truncate_chars`
/// （带 `…(truncated)` 标记）；对象 / 数组序列化后若超限，退化成**带标记的字符串** —— 宁可
/// 让前端按文本渲染，也不静默丢内容。未超限则原样返回。
fn truncate_json_value(v: Value, max: usize) -> Value {
    match v {
        Value::String(s) => Value::String(log::truncate_chars(&s, max)),
        other => {
            let s = serde_json::to_string(&other).unwrap_or_default();
            if s.chars().count() <= max {
                other
            } else {
                Value::String(log::truncate_chars(&s, max))
            }
        }
    }
}

/// `tool_use` 入参的收口：**保持对象 / 数组结构**，只把超长的字符串叶子截断（带标记）。
///
/// 与 `truncate_json_value`（会把整个对象退化成字符串）**刻意不同**：入参里的 `file_path` /
/// `notebook_path` 之类短字段必须原样活下来，前端（`changedFilePathFromArgs` 走 `JSON.parse`）
/// 才能解析出改动文件、采到回退用的原件快照（E4 需求：子代理改的文件也进「更改文件」且可回退）。
/// 退化成字符串会让 `JSON.parse` 失败 ⇒ 路径丢失 ⇒ 回退不了。
fn truncate_tool_input(v: Value, max: usize) -> Value {
    match v {
        Value::String(s) => Value::String(log::truncate_chars(&s, max)),
        Value::Array(a) => {
            Value::Array(a.into_iter().map(|x| truncate_tool_input(x, max)).collect())
        }
        Value::Object(m) => {
            Value::Object(m.into_iter().map(|(k, x)| (k, truncate_tool_input(x, max))).collect())
        }
        other => other,
    }
}

/// 把子代理内部的一块内容块转成前端可渲染的形态。字段名与主流程同名事件**逐字对齐**：
/// `thinking` / `text` / `tool_use`（`id` / `name` / `input`）/ `tool_result`（`tool_use_id` /
/// `content` / `is_error`）。未知类型原样透传（前端按缺省处理）。
fn forward_content_block(blk: &Value) -> Value {
    match blk.get("type").and_then(Value::as_str).unwrap_or("") {
        "thinking" => json!({
            "type": "thinking",
            "thinking": truncate_json_value(
                blk.get("thinking").cloned().unwrap_or(Value::Null),
                SUBAGENT_EVENT_ITEM_CHARS,
            ),
        }),
        "text" => json!({
            "type": "text",
            "text": truncate_json_value(
                blk.get("text").cloned().unwrap_or(Value::Null),
                SUBAGENT_EVENT_ITEM_CHARS,
            ),
        }),
        "tool_use" => json!({
            "type": "tool_use",
            "id": blk.get("id").cloned().unwrap_or(Value::Null),
            "name": blk.get("name").cloned().unwrap_or(Value::Null),
            // 入参走 `truncate_tool_input`（保结构、只截断超长字符串叶子）：`file_path` 等短字段必须
            // 活下来，前端才能解析出改动文件、采到回退原件（见函数注释）。
            "input": truncate_tool_input(
                blk.get("input").cloned().unwrap_or(Value::Null),
                SUBAGENT_EVENT_ITEM_CHARS,
            ),
        }),
        "tool_result" => {
            let mut out = json!({
                "type": "tool_result",
                "tool_use_id": blk.get("tool_use_id").cloned().unwrap_or(Value::Null),
                "content": truncate_json_value(
                    blk.get("content").cloned().unwrap_or(Value::Null),
                    SUBAGENT_EVENT_ITEM_CHARS,
                ),
            });
            if blk.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
                out["is_error"] = json!(true);
            }
            out
        }
        _ => blk.clone(),
    }
}

/// 子代理内部过程转发的**状态机**：单条（`forward_content_block`）+ 整段（`left`）双上限。
///
/// 纯逻辑：`events()` 只算出「要发什么」（不碰 stdout，单测直接钉它），`send()` 才 `emit`。
struct SubtaskForward {
    /// 整段剩余预算（字符）。`SUBAGENT_EVENT_TOTAL_CHARS` 起算，随每条事件的实际体积递减。
    left: usize,
    /// 是否已发过「整段预算用尽」标记（只发一次，避免刷屏）。
    notified: bool,
}

impl SubtaskForward {
    fn new() -> Self {
        Self { left: SUBAGENT_EVENT_TOTAL_CHARS, notified: false }
    }

    /// 算出这条内部消息要 emit 的事件列表（0 / 1 / 2 条），**保序**。事件契约（E2 照此渲染）：
    ///
    /// ```text
    /// {"type":"system","subtype":"task_event","task_id":"task-1",
    ///  "message":{"role":"assistant","content":[ {type:thinking|text|tool_use,…} ]}}
    /// {"type":"system","subtype":"task_event","task_id":"task-1",
    ///  "message":{"role":"user","content":[ {type:tool_result,…} ]}}
    /// ```
    ///
    /// 整段预算用尽后追加一条 `{"type":"system","subtype":"task_event","task_id":…,"truncated":true}`
    /// 且**之后不再出内容事件**（明确标记，不静默丢）。
    fn events(&mut self, task_id: &str, role: &str, content: &[Value]) -> Vec<Value> {
        let mut out = Vec::new();
        if content.is_empty() {
            return out;
        }
        if self.left == 0 {
            if !self.notified {
                self.notified = true;
                out.push(json!({
                    "type": "system", "subtype": "task_event",
                    "task_id": task_id, "truncated": true,
                }));
            }
            return out;
        }
        let blocks: Vec<Value> = content.iter().map(forward_content_block).collect();
        let size = serde_json::to_string(&blocks).map(|s| s.chars().count()).unwrap_or(0);
        if size > self.left {
            // 剩余预算放不下这一。封顶并留标记，不再转发后续内。
            self.left = 0;
            self.notified = true;
            out.push(json!({
                "type": "system", "subtype": "task_event",
                "task_id": task_id, "truncated": true,
            }));
            return out;
        }
        self.left -= size;
        out.push(json!({
            "type": "system", "subtype": "task_event",
            "task_id": task_id,
            "message": { "role": role, "content": blocks },
        }));
        out
    }

    /// 发送出口：把 `events()` 的结果逐条 `emit`（保持顺序）。
    fn send(&mut self, task_id: &str, role: &str, content: &[Value]) {
        for ev in self.events(task_id, role, content) {
            emit(ev);
        }
    }
}

/// `run_subagent` 的产出：最终文本 + 是否「部分报告」。
///
/// `partial = true` 只在两种**撞墙**情形置位：撞轮数上限（`max_rounds`）或撞 token 预算
/// （`budget_tokens`）—— 此时交回的是「做到一半的结论」，不是任务的自然收尾。调用方据此把
/// `task_done` 事件标上 `partial: true` 并在日志里留痕（前端据此提示用户，另见前端任务）。
struct SubagentOutcome {
    text: String,
    partial: bool,
    /// 本轮花掉的加权单位（`subagent_usage_units` 的累计）。
    ///
    /// 只有后台子代理会读它：它活过了发起它的那个回合，日志里必须能回答
    /// 「谁在跑 / 烧了多少」（§待办 3-28.7 的验收口径）。前台路径不看（`..` 解构）。
    spent: u64,
    /// 本轮的完整子代理 history（含接上的旧会话、本轮提问与最终报告）。
    ///
    /// 目前只有一个消费者：`run_agent_tool` 把它追加成「角色常驻」的一条回合
    /// （见 `subagent_session`）。其余调用方（fork 技能 / 后台复盘）不看它。
    history: Vec<Value>,
}

/// 撞墙上限的**日志单一真相源**：带齐 `task_id` + 撞的是哪种上限（`rounds` / `tokens`）
/// + 已花费的 token 与轮次。抽成纯函数是为了让「撞墙必留日志」这条可被单测钉住
/// （轮数与 token 两条路径共用同一份措辞）。
fn subagent_wall_note(task_id: &str, wall: &str, limit: u64, round: usize, spent: u64) -> String {
    format!(
        "子代理 {task_id} 撞 {wall} 上限（limit={limit}，第 {round} 轮，已花 {spent} 加权单位），交回部分报告"
    )
}

/// 收口提示的注入时机：**恰在倒数第二轮**（`round == max_rounds - 1`）注入一次。
///
/// 抽成纯函数便于单测钉住「只在这一轮、只注入一次」；`max_rounds < 2` 时不注入
/// （只有一轮的 fork 没有「最后一轮」可言，注了反而误导）。
fn subagent_wrap_hint_due(round: usize, max_rounds: usize) -> bool {
    max_rounds >= 2 && round + 1 == max_rounds
}

/// 子代理的起始 history：**旧会话（可选）+ 本轮提问**。
///
/// 原来直接。`truncate_chars` 从尾部砍：三段式报告的最后一段恰。"Not verified"（主线程
/// 最需要的「哪些没验证」），最先被砍掉。这里改成按 `## ` 分段、每段各分一份配额，
/// 子代理的起始 history：**旧会话（可选）+ 本轮提问**。
fn clip_report(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let lines: Vec<&str> = text.lines().collect();
    let is_heading = |line: &str| line.trim_start().starts_with("## ");
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_heading(line))
        .map(|(index, _)| index)
        .collect();
    if starts.is_empty() {
        // 没有小标。「保头」就是原先的行为（带 `…` 标记）。
        return crate::log::truncate_chars(text, max);
    }
    // 。= 前言（可选）+ 每个 `## ` 起一段。
    let mut segments: Vec<(usize, usize)> = Vec::new();
    if starts[0] > 0 {
        segments.push((0, starts[0]));
    }
    for (index, start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(lines.len());
        segments.push((*start, end));
    }
    let note = "\n\n…[report truncated; every section head kept]";
    let note_len = note.chars().count();
    let headings: usize = segments
        .iter()
        .map(|(start, _)| {
            if is_heading(lines[*start]) {
                lines[*start].chars().count() + 1
            } else {
                0
            }
        })
        .sum();
    let quota = max.saturating_sub(note_len + headings) / segments.len().max(1);

    let mut out = String::new();
    for (start, end) in segments {
        let block = &lines[start..end];
        let Some(first) = block.first() else { continue };
        let heading = is_heading(first);
        if heading {
            out.push_str(first);
            out.push('\n');
        }
        let body_start = usize::from(heading);
        let mut used = 0usize;
        for line in &block[body_start..] {
            let remain = quota.saturating_sub(used);
            if remain == 0 {
                break;
            }
            let len = line.chars().count();
            if len + 1 <= remain {
                out.push_str(line);
                out.push('\n');
                used += len + 1;
            } else {
                // 单行就超过本段配额（长段。/ 单行 JSON）：保留开头，别整段丢掉。
                let head: String = line.chars().take(remain.saturating_sub(1)).collect();
                out.push_str(&head);
                out.push('…');
                out.push('\n');
                break;
            }
        }
    }
    out.push_str(note);
    out
}

/// 子代理的起始 history：**旧会话（可选）+ 本轮提问**。
///
/// 抽成纯函数是为了让「常驻会话怎么接上去」可被单测钉住：旧消息必须**原样前置**、
/// 本轮提问必须恰好是最后一条 —— 顺序错了子代理会把上一轮的报告当成本轮的答案。
fn subagent_seed_history(resumed: &[Value], prompt: &str) -> Vec<Value> {
    let mut history: Vec<Value> = Vec::with_capacity(resumed.len() + 1);
    history.extend_from_slice(resumed);
    history.push(json!({
        "role": "user",
        "content": [{ "type": "text", "text": prompt }],
    }));
    history
}

/// 跑一个子代理，返回它的产出（`Err` = 整条链路失败）。
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
///
/// `resumed`：角色常驻带进来的旧会话（§待办 3-22 分片 4）。传空切片 = 旧行为逐字节不变
/// （fork 技能与后台复盘都传空 —— 它们不是「同一角色的重复派发」）。
fn run_subagent(
    cfg: &Cfg,
    tctx: &tools::Ctx,
    spec: &ForkSpec,
    mut bridge: Option<&mut mcp::McpSet>,
    prompt: &str,
    resumed: &[Value],
) -> Result<SubagentOutcome, String> {
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
    let mut history: Vec<Value> = subagent_seed_history(resumed, prompt);
    let mut spent: u64 = 0;
    let mut last_text = String::new();
    // 收口提示只注入一次（轮数倒数第二 **或** 预算到 80%，先到者生效）。
    let mut hinted = false;
    // 撞预算后的**纯报告轮**：请求里不带 tools，只让模型把已有结论写完。
    let mut report_only = false;
    // 上下文超限只压缩一次（2026-11-15，见下面。400 分支）。
    let mut compacted_context = false;
    // 内部过程转发器（仅「用户可见的 `Agent` 工具调用」为 `Some`；复盘 / fork 技能为 `None`
    // ⇒ 全程一个 `task_event` 都不发，静默不变量）。
    let mut fwd = subtask_forwarder(spec.emit_subtask);
    // 后台子代理的取消点（§待办 3-28.7）：每轮开头、每次工具执行前各看一次。
    // 只看标志、不做抢。—。正在飞的那一。HTTP 请求会先自然结束（最。`REQUEST_TIMEOUT_SECS`）。
    let cancelled = || {
        spec.cancel
            .as_ref()
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
    };

    for round in 1..=max_rounds {
        if cancelled() {
            log::warn(format!("子代理 {task_id} 被用户终止（第 {round} 轮开始前）"));
            return Err(format!(
                "已被用户终止（后台子代理 {task_id}，第 {round} 轮开始前）",
            ));
        }
        // 收口提示（2026-10-16 / 2026-10-24）：轮数倒数第二轮 **或** 预算到 80% 时注入一次。
        // 落点纪律同 `TURN_BUDGET_HINT`：拼进已有 `tool_result` 内部，不动消息条数。
        if !hinted
            && (subagent_wrap_hint_due(round, max_rounds)
                || subagent_budget_wrap_due(spent, budget_tokens))
            && append_hint_to_last_tool_result(&mut history, SUBAGENT_WRAP_HINT)
        {
            hinted = true;
        }
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
        // 纯报告轮**刻意不带 tools** —— 这是「撞预算后必须交完整报告」的硬保证。
        if !spec.defs.is_empty() && !report_only {
            body["tools"] = json!(spec.defs);
        }
        if let Some(t) = plan.to_json() {
            body["thinking"] = t;
        }
        // 调试转储（LUNAC_DUMP_API）：子代理非流式，request/response 各一行。
        // req_id = sub-<task_id>-r<round> —— 子代理用量落在 result.usage.subagent.requests[]
        // （那是个扁平列表、按完成顺序排，按下标对不准），req_id 自带 task_id 与轮次更精确。
        let dump_on = log::dump_api_enabled();
        let dump_rid = if dump_on {
            let rid = format!("sub-{task_id}-r{round}");
            log::dump_api("request", &rid, &cfg.model, None, None, &body);
            rid
        } else {
            String::new()
        };
        // 子代理请求的瞬时失败重试。026-11-15）：子代理动辄跑 20 轮、每轮都烧钱。
        // 一次瞬。429 / 5xx 丢掉整单等于前面所有轮次的 token 全部白费。只。
        // `retryable_status` 与网络层错误重试，次数比主循环更小（子代理不是关键路径）。
        const SUBAGENT_MAX_RETRIES: usize = 2;
        let mut sub_attempt = 0usize;
        let (status, raw) = loop {
            let sent = model_post(cfg)
                .header("authorization", format!("Bearer {}", cfg.token))
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send();
            let resp = match sent {
                Ok(r) => r,
                Err(e) => {
                    if sub_attempt < SUBAGENT_MAX_RETRIES {
                        sub_attempt += 1;
                        let delay = retry_delay_ms(sub_attempt, None);
                        log::warn(format!(
                            "子代理 {task_id} 请求发送失败（{e}），{delay}ms 后重试（第 {sub_attempt} 次）"
                        ));
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                        continue;
                    }
                    if dump_on {
                        log::dump_api(
                            "response",
                            &dump_rid,
                            &cfg.model,
                            None,
                            Some(&e.to_string()),
                            &Value::Null,
                        );
                    }
                    return Err(format!("子代理请求发送失。 {e}"));
                }
            };
            let status = resp.status();
            let raw = match resp.text() {
                Ok(raw) => raw,
                Err(e) => {
                    if sub_attempt < SUBAGENT_MAX_RETRIES {
                        sub_attempt += 1;
                        let delay = retry_delay_ms(sub_attempt, None);
                        log::warn(format!(
                            "子代。{task_id} 响应读取失败（{e}），{delay}ms 后重试（。{sub_attempt} 次）"
                        ));
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                        continue;
                    }
                    return Err(format!("子代理响应读取失败: {e}"));
                }
            };
            if dump_on {
                log::dump_api(
                    "response",
                    &dump_rid,
                    &cfg.model,
                    Some(status.as_u16()),
                    None,
                    &Value::String(raw.clone()),
                );
            }
            if !status.is_success() && retryable_status(status.as_u16()) {
                if sub_attempt < SUBAGENT_MAX_RETRIES {
                    sub_attempt += 1;
                    let delay = retry_delay_ms(sub_attempt, None);
                    log::warn(format!(
                        "子代理 {task_id} 收到 HTTP {status}（可重试），{delay}ms 后重试（第 {sub_attempt} 次）"
                    ));
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                    continue;
                }
            }
            break (status, raw);
        };
        // 端点回报上下文超。压缩一次后重试。026-11-15）。子代理一。20 轮、每轮都。
        // tool_result，比主循环更容易撞端点上下文上限；原实现直接整单失败，前面轮次的
        // token 全白花。只压缩一次：第二次仍 400 就按真实错误返回。
        if status == 400 && !compacted_context && context_related_error(&raw) {
            compacted_context = true;
            let out = compact_history(&mut history, Compact::Force, 0);
            crate::fire_plain_hook("PreCompact", &tctx.cwd, json!({ "trigger": "subagent" }));
            log::warn(format!(
                "子代理 {task_id} 端点回报上下文超限，压缩 {} 条后重试",
                out.dropped
            ));
            continue;
        }
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
            spent += subagent_usage_units(r_in, r_out, r_read, r_create);
            // 同时记进**主循环那本账**（2026-09-29）：子代理的 token 平台照收钱，
            // 此前却完全没进 `result.usage` ⇒ 本地用量日志与账单对不上。
            // 2026-10-27 起按 `spec.usage` 分账（子代理 / 复盘各一本）。
            match spec.usage {
                UsageKind::Subagent => cfg.sub.record(r_in, r_read, r_create, r_out),
                UsageKind::Review => cfg.review.record(r_in, r_read, r_create, r_out),
            }
        }

        // 拆包：只要 text 与 tool_use —— thinking 等块**不回灌**（回灌会 400，
        // 同主循环的规矩），但文本块要 push 回 history，否则端点看不到自己说过的话。
        let mut blocks: Vec<Value> = Vec::new();
        let mut calls: Vec<(String, String, Value)> = Vec::new();
        // 转发给前端的展示块（含 thinking —— history 不放它，但「让用户看到思考」要它）。
        // 形状与主循环的 `display` 同构，前端可复用同一套块渲染。
        let mut display: Vec<Value> = Vec::new();
        if let Some(arr) = parsed.get("content").and_then(Value::as_array) {
            for b in arr {
                match b.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => {
                        if let Some(t) = b.get("text").and_then(Value::as_str) {
                            if !t.trim().is_empty() {
                                last_text = t.to_string();
                                display.push(json!({ "type": "text", "text": t }));
                            }
                        }
                        blocks.push(b.clone());
                    }
                    "thinking" => {
                        if let Some(t) = b.get("thinking").and_then(Value::as_str) {
                            if !t.is_empty() {
                                display.push(json!({ "type": "thinking", "thinking": t }));
                            }
                        }
                    }
                    "tool_use" => {
                        let id = b.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                        let name = b.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                        let input = b.get("input").cloned().unwrap_or_else(|| json!({}));
                        blocks.push(b.clone());
                        display.push(json!({
                            "type": "tool_use", "id": id, "name": name, "input": input.clone(),
                        }));
                        calls.push((id, name, input));
                    }
                    _ => {}
                }
            }
        }
        if !blocks.is_empty() {
            history.push(json!({ "role": "assistant", "content": blocks }));
        }

        // 内部过程转发：本轮的助手消息（思考 / 文本 / 工具调用）按发生顺序发出。
        // **只 emit，不进任何 history** —— 主对话前缀缓存不变量因此不受影响。
        if let Some(f) = fwd.as_mut() {
            f.send(task_id, "assistant", &display);
        }

        if calls.is_empty() {
            let raw = if last_text.trim().is_empty() {
                format!("(subagent {task_id} finished without writing a report)")
            } else {
                last_text
            };
            // 纯报告轮 = 预算撞墙后的收尾：报告本身是完整的，但任务是被预算截断的，
            // 因此 `partial` 仍为真（日志 / 前端据此提示「部分完成」）。
            let text = if report_only {
                format!(
                    "[subagent {task_id}: budget reached at round {round}; final report]\n\n{raw}"
                )
            } else {
                raw
            };
            return Ok(SubagentOutcome {
                text,
                partial: report_only,
                spent,
                history,
            });
        }

        // 预算闸门。*先检查再执行下一批工。*，把「已经烧掉多少」如实算进去。
        // 撞墙不再直接返回上一条 `last_text`（那是 134–345 字的残片）：给每个未执行的
        // tool_use 补一条 `tool_result`（保持协议合法），然后进入**纯报告轮**。
        if spent > budget_tokens {
            // 撞墙要能被看见（2026-10-16）：日志留痕 + `task_done.partial`（见调用方）。
            log::warn(subagent_wall_note(task_id, "budget", budget_tokens, round, spent));
            let skipped: Vec<Value> = calls
                .iter()
                .map(|(id, _, _)| {
                    json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": SUBAGENT_BUDGET_SKIP,
                        "is_error": true,
                    })
                })
                .collect();
            history.push(json!({ "role": "user", "content": skipped }));
            if report_only {
                // 理论上不可达（纯报告轮不带 tools），但别让异常输入变成死循环。
                return Ok(SubagentOutcome {
                    text: last_text,
                    partial: true,
                    spent,
                    history,
                });
            }
            report_only = true;
            continue;
        }

        // 工具串行执行（子代理里不做只读并行：收益小于「多一层并发状态」的复杂度）
        let mut results: Vec<Value> = Vec::with_capacity(calls.len());
        for (id, name, input) in &calls {
            // 取消点②：每件工具执行前 —。撞在「一。5 个写操作」中间也不会一路写到底。
            if cancelled() {
                log::warn(format!(
                    "子代理 {task_id} 被用户终止（第 {round} 轮，工具 {name} 执行前）"
                ));
                return Err(format!(
                    "已被用户终止（后台子代理 {task_id}，第 {round} 轮，工具 {name} 之前）",
                ));
            }
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
                    let needs_card = tools::needs_approval(name)
                        && (!tctx.read_only || tools::gated_in_read_only(name));
                    if !allowed && needs_card {
                        match approval_policy() {
                            // 单发 + 用户显式点名的写类工具（阶段 2B）：放行；但凭据扫描
                            // 命中（`hook_allow_needs_card`）时照旧拒绝 —— 无人值守绝不能
                            // 把「写了密钥」这件事静默放过去。
                            ApprovalPolicy::UnattendedDeny
                                if unattended_allows(name)
                                    && !hook_allow_needs_card(name, input) =>
                            {
                                None
                            }
                            ApprovalPolicy::UnattendedDeny => Some(unattended_deny_reason(name)),
                            ApprovalPolicy::UnattendedAllow => None,
                            ApprovalPolicy::Stdio if spec.ask_permission => {
                                match hook_tool_gate(
                                    "PermissionRequest",
                                    &tctx.cwd,
                                    name,
                                    id,
                                    input,
                                ) {
                                    HookGate::Deny(r) => {
                                        Some(format!("Denied by PermissionRequest hook: {r}"))
                                    }
                                    HookGate::Allow if !hook_allow_needs_card(name, input) => None,
                                    // 第二模型预审（§待。3-28.14）：子代理里的每次写操作
                                    // 与主循环同一条闸 —。deny / escalate，永。allow。
                                    _ => match second_review::gate(name, input) {
                                        Some(reason) => Some(reason),
                                        None => match await_approval(
                                            open_approval(name, id, input, Some(task_id)),
                                            input,
                                        ) {
                                            Decision::Allow(_) => None,
                                            Decision::Deny(msg, _) => Some(msg),
                                        },
                                    },
                                }
                            }
                            ApprovalPolicy::Stdio => None,
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
        // 内部过程转发：本轮的工具结果按调用顺序发出（在写回子代理 history **之前**借用，
        // 之后 `results` 被 move 进 json!）。同样只 emit、不进主对话。
        if let Some(f) = fwd.as_mut() {
            f.send(task_id, "user", &results);
        }
        history.push(json!({ "role": "user", "content": results }));
    }

    // 打满轮数上限：撞墙要能被看见（2026-10-16）—— 日志留痕 + `task_done.partial`（见调用方）。
    log::warn(subagent_wall_note(task_id, "rounds", max_rounds as u64, max_rounds, spent));
    Ok(SubagentOutcome {
        text: format!(
            "[subagent {task_id}: hit its {max_rounds}-round ceiling; partial report \
             follows]\n\n{last_text}"
        ),
        partial: true,
        spent,
        history,
    })
}

/// 由 `run_subagent` 的产出构造 `task_done` 事件（`Agent` 工具与 fork 技能共用）。
///
/// `partial` 字段（2026-10-16 新增）：撞轮数上限 / token 预算时为 `true` —— 交回的是
/// **部分报告**。前端据此提示用户（字段名与语义登记在协议契约表；前端兼容缺字段，
/// 缺失时按「非部分报告」处理）。`ok` 仍按 `Result::is_ok`（撞墙是 `Ok` 分支 ⇒ `ok` 为真）。
fn task_done_event(task_id: &str, out: &Result<SubagentOutcome, String>, ms: u64) -> Value {
    json!({
        "type": "system", "subtype": "task_done",
        "task_id": task_id, "ok": out.is_ok(),
        "partial": matches!(out, Ok(o) if o.partial),
        "ms": ms,
        // 后台子代理（§待办 3-28.7）：前端据此把这一行的「运行中」改成「已完成」，
        // 并给出「报告将在下一轮并入」的提示。前台永。false。
        "background": false,
    })
}

/// 后台子代理的 `task_done`：与前台同一形状，只。`background: true` / `cancelled` / `spent`。
///
/// `spent` 是这一趟烧掉的加权单位（`subagent_usage_units` 累计）—。后台子代理活过了发起它的
/// 回合，日志与界面上必须能回答「谁在跑 / 花了多少」。
fn background_task_done_event(
    task_id: &str,
    out: &Result<SubagentOutcome, String>,
    ms: u64,
    cancelled: bool,
) -> Value {
    json!({
        "type": "system", "subtype": "task_done",
        "task_id": task_id, "ok": out.is_ok(),
        "partial": matches!(out, Ok(o) if o.partial),
        "ms": ms,
        "background": true,
        "cancelled": cancelled,
        "spent": match out { Ok(o) => o.spent, Err(_) => 0 },
    })
}

/// `Agent` 工具的执行入口：校验参数 → 派子代理 → 把报告包成模型能读的 `tool_result`。
///
/// 只花**子代。*的上下文。6k 。一个中。`SKILL.md` 的量级），不碰主对话前缀。
/// 超预算时后面的技。*整条跳过并写。*，不静默。—。调用方与日志都能看出少了什么。
const SUBAGENT_SKILL_BUDGET_CHARS: usize = 16_000;

/// `Agent` 工具的执行入口：校验参数 → 派子代理 → 把报告包成模型能读的 `tool_result`。
///
/// · 空列。空表。
/// · 未知 key 。`Err` 列出可用。key。*不能静默忽略**：调用方以为它加载了，实际没有）。
fn preloaded_skill_refs<'a>(
    skills: &'a [skills::Skill],
    keys: &[String],
) -> Result<Vec<&'a skills::Skill>, String> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let available: Vec<&str> = skills.iter().map(|skill| skill.key.as_str()).collect();
    let mut picked: Vec<&skills::Skill> = Vec::new();
    for key in keys {
        let Some(skill) = skills.iter().find(|skill| skill.key == *key) else {
            return Err(format!(
                "未知的技。key「{key}」；可用的有：{}",
                if available.is_empty() {
                    "（没有装任何技能）".to_string()
                } else {
                    available.join(" / ")
                }
            ));
        };
        if !picked.iter().any(|seen| seen.key == skill.key) {
            picked.push(skill);
        }
    }
    Ok(picked)
}

/// 把点名的技能正文拼成一段注入子代理系统提示词的区块。
///
/// · 空列。空串（调用方不追加任何东西，行为与改造前逐字节一致）。
/// · 正文来自 `skills::instruction(skill, "")` —。`Skill` 工具注入同一份正文，
///   `$ARGUMENTS` 在预加载场景没有实参，替换成空串。
/// · 超预算：能放多少放多少（按字符截断并。`。truncated)`），后面的整条跳过并记一行。
fn render_preloaded_skills(preloaded: &[&skills::Skill]) -> String {
    if preloaded.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "# Preloaded skills\n\
         The caller preloaded these skills for you. Treat them as part of the task \
         instructions; their procedures apply to your work in this run.\n",
    );
    let mut used = out.chars().count();
    for skill in preloaded {
        let header = format!("\n## {}\n{}\n", skill.key, skill.description.trim());
        let body = skills::instruction(skill, "");
        let header_chars = header.chars().count();
        if used + header_chars >= SUBAGENT_SKILL_BUDGET_CHARS {
            out.push_str(&format!(
                "\n## {}\n(omitted: preloaded-skill budget reached)\n",
                skill.key
            ));
            continue;
        }
        out.push_str(&header);
        used += header_chars;
        let remain = SUBAGENT_SKILL_BUDGET_CHARS - used;
        let body_chars = body.chars().count();
        if body_chars <= remain {
            out.push_str(&body);
            if !body.ends_with('\n') {
                out.push('\n');
            }
            used += body_chars;
        } else {
            let head: String = body.chars().take(remain.saturating_sub(24)).collect();
            out.push_str(&head);
    out.push_str("\n…(truncated)\n");
            used = SUBAGENT_SKILL_BUDGET_CHARS;
        }
    }
    out
}

/// 技能级记忆区块（§待。3-28.10 ①）：只拼进**该技。fork 子代。*的系统提示词。
///
/// 措辞点明两件事：。这是**上一次跑这个技。*留下的笔记（不是本轮用户要求）；
/// 。更新它要。`Remember`，落点自动就是这个技能自己的命名空间。
fn render_skill_memory(key: &str, text: &str) -> String {
    format!(
        "\n\n# This skill's private memory\n\
         These notes were saved by earlier runs of the skill \"{key}\". Use them as \
         background for this run. To update them, call Remember 。inside this skill \
         run, its entries are stored in this skill's own namespace (not the main \
         long-term memory).\n\n{text}"
    )
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
    // 角色（2026-10-24）：缺省 / 未知值 = `general`，工具集与提示词都保持改造前的行为。
    let role = subagent_role::Role::parse(
        input.get("role").and_then(Value::as_str).unwrap_or(""),
    );
    let mut role_defs = subagent_role_defs(subagent_defs, role);
    let role_system = if role.prompt().is_empty() {
        subagent_system.to_string()
    } else {
        format!("{subagent_system}\n{}", role.prompt())
    };
    // 预加载技能（§待办 3-28.10）：把指定技能的正文拼进**子代。*的系统提示词。
    // 只改子代理自己的上下文，主对话前缀一个字不动（前缀缓存不受影响）。
    let requested: Vec<String> = input
        .get("skills")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let role_system = match preloaded_skill_refs(skill_list, &requested) {
        Ok(picked) if !picked.is_empty() => {
            // 技能级 `allowed-tools`（§待。3-28.10 ②）：与 fork 技。*同一份判。*
            // （`allowed_tools_subset`）—。预加载的技能说「只准用这几个工具」，
            // 这次子代理就只能用这几个；多条技能取并集；都没写 = 不限制。
            let before = role_defs.len();
            role_defs = allowed_tools_subset(&picked, &role_defs);
            if role_defs.len() != before {
                log::info(format!(
                    "子代理工具集按预加载技能的 allowed-tools 收窄：{before} → {} 件",
                    role_defs.len()
                ));
            }
            if role_defs.is_empty() {
                log::warn(format!(
                    "预加载技能 {} 的 allowed-tools 在内置工具里一个都没匹配上 —— 子代理将无工具可用",
                    requested.join(", ")
                ));
            }
            format!("{role_system}\n\n{}", render_preloaded_skills(&picked))
        }
        Ok(_) => role_system,
        Err(reason) => return (format!("Agent 参数有误：{reason}"), true),
    };
    let task_id = format!("task-{}", TASK_SEQ.fetch_add(1, Ordering::Relaxed) + 1);
    // 角色常驻（§待办 3-22 分片 4）：具名角色 + 检查点打开时，把上一段同角色会话接在前面。
    // 读失败一律按「新会话」继续（只 warn）—— 常驻是增强，不该让这次派发整个失败。
    let resumed = if subagent_resident() && role.is_resident() {
        match subagent_session::load(role.as_str()) {
            Ok(h) if !h.is_empty() => {
                log::info(format!(
                    "子代理角色 {} 接上上一段会话（{} 条消息）",
                    role.as_str(),
                    h.len()
                ));
                h
            }
            Ok(_) => Vec::new(),
            Err(e) => {
                log::warn(format!(
                    "读取角色 {} 的常驻会话失败，按新会话继续：{e}",
                    role.as_str()
                ));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    // 后台模式（§待。3-28.7）：整条 `run_subagent` 挪到独立线程，本回合立刻收尾。
    if input.get("background").and_then(Value::as_bool).unwrap_or(false) {
        return start_background_agent(
            cfg,
            tctx,
            task_id,
            desc.to_string(),
            role,
            role_defs,
            skill_list.to_vec(),
            role_system,
            ask_permission,
            prompt.to_string(),
            resumed,
        );
    }
    emit(json!({
        "type": "system", "subtype": "task_started",
        "task_id": task_id, "description": desc, "role": role.as_str(),
    }));
    log::info(format!("子代理 {task_id} 启动（role={}）：{desc}", role.as_str()));
    let started = Instant::now();
    let out = run_subagent(
        cfg,
        tctx,
        &ForkSpec {
            task_id: &task_id,
            system: &role_system,
            defs: &role_defs,
            skills: skill_list,
            ask_permission,
            max_rounds: MAX_SUBAGENT_ROUNDS,
            budget_tokens: SUBAGENT_BUDGET_TOKENS,
            emit_progress: true,
            // 用户可见的 `Agent` 工具调用 ⇒ 转发内部过程（E1 数据通道，E2 前端渲染）
            emit_subtask: AGENT_EMIT_SUBTASK,
            // 前台子代理登记进 peer 登记处（同批兄弟可互相 ListPeers / SendMessage）
            peer: Some(desc),
            // 前台：生命周期就是这次工具调用，不需要单独的中断标志
            cancel: None,
            usage: UsageKind::Subagent,
        },
        // 子代理**不接 MCP 桥**（硬约束：桥是单线程 stdio 通道，主循环还持有它；
        // 且一接就是全量用户工具 = 不受控的副作用面）。它的工具集里也因此剔掉了
        // 依赖桥的那几件，见 `subagent_tool_defs`。
        None,
        prompt,
        &resumed,
    );
    emit(task_done_event(&task_id, &out, started.elapsed().as_millis() as u64));
    let ms = started.elapsed().as_millis();
    match out {
        Ok(SubagentOutcome { text, history, .. }) => {
            log::info(format!("子代理 {task_id} 完成（{ms}ms，报告 {} 字）", text.chars().count()));
            // 常驻落盘（失败只 warn：这次工作已经做完，不能因为存不下就报失败）。
            if subagent_resident() && role.is_resident() {
                if let Err(e) = subagent_session::save(role.as_str(), &history, &text) {
                    log::warn(format!("保存角色 {} 的常驻会话失败：{e}", role.as_str()));
                }
            }
            // 按角色收口（2026-10-27，§待办 3-16 P0-1）：explore 1.5k / review 2k /
            // coding·research 3k / general 8k。`role` 就在这里，不必另存。
            let cap = role.report_chars();
            if text.chars().count() > cap {
                log::info(format!(
                    "子代理 {task_id} 报告超角色上限（role={}，{cap} 字），已裁剪",
                    role.as_str()
                ));
            }
            let clipped = clip_report(&text, cap);
            // 复核回执（§待办 3-22 分片 5）：只读 reviewer 改不了代码，报告要能被主线程
            // 直接决策 —— 回执把「有多少 file:line 证据 / 哪些未验证」放在报告**前面**。
            let body = if role == subagent_role::Role::Review {
                subagent_role::review_receipt(&clipped)
            } else {
                clipped
            };
            (
                format!("[{task_id}] subagent report:\n\n{body}"),
                false,
            )
        }
        Err(e) => {
            log::warn(format!("子代理 {task_id} 失败（{ms}ms）: {e}"));
            (format!("[{task_id}] subagent failed: {e}"), true)
        }
    }
}

/// 把一。`Agent` 派发挪到后台线程（§待。3-28.7）。
///
/// 与前。`run_agent_tool` **共用同一。`run_subagent`**，只换三样东西：
///   。入口在独立线程（`Cfg::detached()` + `Ctx` 克隆，两者都是现成的跨线程形态）。
///   。`ForkSpec::cancel` 给一个可。`tool_control` 置位的标志；
///   。报告不回本次 tool_result，而是攒进 `BACKGROUND_DONE`（后台命令同一条通路）。
///
/// 交给模型。tool_result 只说「已派出」—。**不假装任务已完成**，也不把预算 / 轮数
/// 说成「一定会跑完」。
#[allow(clippy::too_many_arguments)]
fn start_background_agent(
    cfg: &Cfg,
    tctx: &tools::Ctx,
    task_id: String,
    desc: String,
    role: subagent_role::Role,
    defs: Vec<Value>,
    skills: Vec<skills::Skill>,
    system: String,
    ask_permission: bool,
    prompt: String,
    resumed: Vec<Value>,
) -> (String, bool) {
    // 只读。/ 计划相位：与前台同一道闸 —。「挪到后台」不能变成绕过只读的旁路。
    if let Some(why) = tools::write_blocked(tctx, "Agent") {
        return (format!("[{task_id}] subagent failed: {why}"), true);
    }
    let owned_cfg = match cfg.detached() {
        Ok(owned) => owned,
        Err(e) => return (format!("[{task_id}] subagent failed: {e}"), true),
    };
    let mut owned_ctx = tctx.clone();
    // 后台线程不属于任何主回合：实时输出回调与命令控制块都绑着主回合的 `tool_use_id`。
    // 带过去会把它的命令输出画到主回合那张卡上，还可能被主回合收尾时误注销。
    owned_ctx.tool_output = None;
    owned_ctx.shell_control = None;
    let cancel = Arc::new(AtomicBool::new(false));
    let started_ms = now_ms();
    if let Ok(mut m) = BG_TASKS.lock() {
        m.insert(
            task_id.clone(),
            BgTask {
                label: desc.clone(),
                role: role.as_str().to_string(),
                started_ms,
                cancel: Arc::clone(&cancel),
            },
        );
    }
    emit(json!({
        "type": "system", "subtype": "task_started",
        "task_id": task_id, "description": desc, "role": role.as_str(),
        "background": true, "started_ms": started_ms,
    }));
    log::info(format!(
            "后台子代理 {task_id} 启动（role={}，在跑 {} 个）：{desc}",
        role.as_str(),
        background_subagent_count()
    ));

    let tid = task_id.clone();
    let label = desc.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let out = run_subagent(
            &owned_cfg,
            &owned_ctx,
            &ForkSpec {
                task_id: &tid,
                system: &system,
                defs: &defs,
                skills: &skills,
                ask_permission,
                max_rounds: MAX_SUBAGENT_ROUNDS,
                budget_tokens: SUBAGENT_BUDGET_TOKENS,
                emit_progress: true,
                emit_subtask: AGENT_EMIT_SUBTASK,
                // 不登记为 peer：后台子代理会活过发起它的回合，兄弟批在收尾时就解散了，
                // 留着只会。`ListPeers` 看见一个没有兄弟的僵尸。
                peer: None,
                cancel: Some(Arc::clone(&cancel)),
                usage: UsageKind::Subagent,
            },
            // 同前台：后台子代理也不接 MCP 桥（桥是单线。stdio 通道，主循环持有它）
            None,
            &prompt,
            &resumed,
        );
        let ms = started.elapsed().as_millis() as u64;
        let cancelled = cancel.load(Ordering::Relaxed);
        emit(background_task_done_event(&tid, &out, ms, cancelled));
        match &out {
            Ok(SubagentOutcome {
                text,
                spent,
                history,
                ..
            }) => {
                log::info(format!(
            "后台子代理 {tid} 完成（{ms}ms，{spent} 加权单位，报告 {} 字）：{label}",
                    text.chars().count()
                ));
                // 角色常驻与前台同口径；被取消的那次不落盘（它多半停在半句话上）。
                if !cancelled && subagent_resident() && role.is_resident() {
                    if let Err(e) = subagent_session::save(role.as_str(), history, text) {
                        log::warn(format!("保存角色 {} 的常驻会话失败：{e}", role.as_str()));
                    }
                }
                let cap = role.report_chars();
                let clipped = clip_report(text, cap);
                let body = if role == subagent_role::Role::Review {
                    subagent_role::review_receipt(&clipped)
                } else {
                    clipped
                };
                if let Ok(mut v) = BACKGROUND_DONE.lock() {
                    v.push(format!(
                        "[background subagent {tid}] {label}\n\
                         (started in an earlier turn; you could not ask it questions)\n\n{body}"
                    ));
                }
            }
            Err(e) => {
                log::warn(format!(
            "后台子代理 {tid} 失败（{ms}ms，cancelled={cancelled}）：{e}"
                ));
                if !cancelled {
                    if let Ok(mut v) = BACKGROUND_DONE.lock() {
                        v.push(format!(
                            "[background subagent {tid}] {label}\n\
                             failed: {e}\n\
                             (it may have been stopped, run out of budget, or hit an API error)"
                        ));
                    }
                }
            }
        }
        if let Ok(mut m) = BG_TASKS.lock() {
            m.remove(&tid);
        }
        log::info(format!(
            "后台子代理 {tid} 退出（仍在跑 {} 个）",
            background_subagent_count()
        ));
    });

    (
        format!(
            "[{task_id}] background subagent started (role={}). It keeps working after this \
             turn ends; its report is appended to a later turn automatically, and the user can \
             stop it from the activity panel.",
            role.as_str()
        ),
        false,
    )
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
/// **同一份判。*服务两条路径（§待。3-28.10 ②）。
///   · fork 技能（`run_forked_skill`）—。单条技能；
/// `mcp__*` 弄进来 —— 那三类已被 `subagent_tool_defs()` 剔除，**判据只有一处**。
///
/// 白名单为空 = 不限制；非空但一个都没匹配上 = **一件也不给**（fail-closed：白名单写错时
/// 宁可让子代理只靠推理，也不能悄悄放开成全集）。*只在传入。defs 里挑**，所以技能无法。
/// `mcp__*` 弄进来 —— 那三类已被 `subagent_tool_defs()` 剔除，**判据只有一处**。
fn allowed_tools_subset(skills: &[&skills::Skill], defs: &[Value]) -> Vec<Value> {
    let mut allowed: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut declared = false;
    for skill in skills {
        if skill.allowed_tools.is_empty() {
            continue;
        }
        declared = true;
        allowed.extend(skill.allowed_tools.iter().map(String::as_str));
    }
    if !declared {
        return defs.to_vec();
    }
    defs.iter()
        .filter(|d| {
            let name = d.get("name").and_then(Value::as_str).unwrap_or("");
            allowed.contains(name)
        })
        .cloned()
        .collect()
}

fn fork_skill_tools(skill: &skills::Skill, subagent_defs: &[Value]) -> Vec<Value> {
    allowed_tools_subset(&[skill], subagent_defs)
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
///
/// `emit_subtask = true`（E1 扩展，2026-10-16）：内部过程（思考 / 工具 / 命令）同样转发 —— fork
/// 技能没有 `Agent` 卡，前端在保留的 `.subtask-panel` 行里展开查看（复用与 `Agent` 框格同一套渲染）。
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
    // §待办 3-28.10 。技能级记忆：只在宿主开关打开时读。/ 生效。
    // 注入点在这里（fork 子代理自己的 system），主对话前缀一个字都不动。
    let skill_memory_on = skills::skill_memory_enabled();
    let memory_text = if skill_memory_on {
        skills::skill_memory_text(&skill.key)
    } else {
        None
    };
    let fork_system = match &memory_text {
        Some(text) => {
            log::info(format!(
        "fork 技能 {} 注入技能级记忆 {} 字符",
                skill.key,
                text.chars().count()
            ));
            format!("{subagent_system}{}", render_skill_memory(&skill.key, text))
        }
        None => subagent_system.to_string(),
    };
    // 同一个开关决。`Remember` 写哪一份：技。fork 里写技能命名空间，不污染全局。
    let fork_ctx_storage;
    let fork_ctx = if skill_memory_on {
        fork_ctx_storage = {
            let mut ctx = tctx.clone();
            ctx.skill_scope = Some(skill.key.clone());
            ctx
        };
        &fork_ctx_storage
    } else {
        tctx
    };
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
    // 技能记忆要写到宿主（`lunac/memory_write`）⇒ **只在开开关时**自建一条短命桥。
    // 与复。fork 同一模式；关开关时行为与改造前逐字节一致（不连、不 spawn）。
    let mut skill_bridge = if skill_memory_on {
        connect_memory_bridge()
    } else {
        None
    };
    let out = run_subagent(
        cfg,
        fork_ctx,
        &ForkSpec {
            task_id: &task_id,
            system: &fork_system,
            defs: &defs,
            // 子代理能读**其它技能**（组合很自然）；fork 技能的递归由 `skills::run()`
            // 的 fork 守卫挡住 —— 那里会明确回「不能在 inline 路径加载」。
            skills: skill_list,
            ask_permission,
            max_rounds: MAX_SUBAGENT_ROUNDS,
            budget_tokens: SUBAGENT_BUDGET_TOKENS,
            emit_progress: true,
            // fork 技能的内部过程**也转发**（E1 扩展，2026-10-16）：它没有 `Agent` 卡，前端在保留的
            // `.subtask-panel` 行里可展开查看（复用与 `Agent` 框格同一套块渲染）。
            emit_subtask: FORK_SKILL_EMIT_SUBTASK,
            // fork 技能与 `Agent` 会合进同一个并发批 ⇒ 同样登记，否则会出现
            // 「Agent 看得见 fork 技能、fork 技能看不见 Agent」的不对称。
            peer: Some(skill.key.as_str()),
            // fork 技能跑在本次工具调用里，不需要单独的中断标志
            cancel: None,
            // fork 技能是「用户看得见的一次派活」⇒ 记子代理账，不记复盘账。
            usage: UsageKind::Subagent,
        },
        skill_bridge.as_mut(),
        &format!("Execute this skill end to end, then report the result.\n\n{task}"),
        // fork 技能不是「同一角色的重复派发」⇒ 不接常驻会话，行为与改造前一致。
        &[],
    );
    emit(task_done_event(&task_id, &out, started.elapsed().as_millis() as u64));
    let ms = started.elapsed().as_millis();
    match out {
        Ok(SubagentOutcome { text, .. }) => {
            log::info(format!(
                "fork 技能 {} 完成（{ms}ms，报告 {} 字）",
                skill.key,
                text.chars().count()
            ));
            let clipped = clip_report(&text, SUBAGENT_REPORT_CHARS);
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
// 装配在 main() 的提问循环里（触发判定 + spawn/join），判断与落盘在这里。

/// 当前时间（epoch 秒）。只用 std（本 crate 不引 chrono）。
fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 「上次复盘时间」的持久化落点：`<exe 根>\temp\review-last`（单行 epoch 秒）。
///
/// 选型理由（2026-10-11 方案 B）——「跨进程重启」这一条决定了必须落盘：
///   · **不写进 MEMORY.md**：那是给模型看的记忆正文（进固定前缀），塞时间戳既污染记忆、
///     又让每次变化都动摇上下文块的缓存前缀；
///   · **不新增宿主通道**：memory 桥只有 memory_read / memory_write 两个方法，为一条时间戳
///     加方法要同时动宿主与协议；让宿主 spawn 时注入「上次时间」则要求宿主知道复盘何时跑过
/// 读不出来 / 解析失败一律当作「从未复盘」—— 宁可多复盘一次，也不要因为读到脏数据而永不复盘。
///   · agent 本就往 `<exe 根>\temp\` 写自己的东西（images / logs 都从 log_dir 的父目录派生，
///     见 image_dir），这条沿用同一推导：零新增依赖、零宿主改动、不碰 ModuleData。
/// 读不出来 / 解析失败一律当作「从未复盘」—— 宁可多复盘一次，也不要因为读到脏数据而永不复盘。
fn review_state_path() -> PathBuf {
    crate::log::log_dir()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("review-last")
}

fn load_last_review_ts() -> Option<i64> {
    let text = std::fs::read_to_string(review_state_path()).ok()?;
    text.trim().parse::<i64>().ok()
}

fn save_last_review_ts(ts: i64) {
    let path = review_state_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, format!("{ts}\n")) {
        // 写不进去只是「下次可能重复复盘一次」，不该影响主流程（同记忆读取失败的纪律）。
        log::warn(format!("写复盘时间戳失败（下次可能重复复盘一次）: {e}"));
    }
}

/// 现在该不该复盘（纯函数，便于单测）：四个条件**全满足**才为真。
///
///   · `enabled` —— 写侧可用（没桥 / 记忆关 / Remember 被黑名单裁掉时为假：复盘写不进
///     任何东西，只会白烧一次 API）；
///   · `fired_per_scope` —— 「每会话 ≤ 1 次」的配额，**按作用域各一份**（数组下标见
///     `MemoryScope::index`）。本函数只看**本轮 `scope` 那一份**；
///   · `last_ts` —— 上次复盘时间（epoch 秒；None = 从未）。**两侧共享同一个值**（30 分钟
///     时间闸不按作用域拆）：任一作用域刚复盘过，另一侧 30 分钟内也不跑，防总成本翻倍；
///   · `now_ts` —— 当前时间（epoch 秒）。
///
/// 第 4 个条件（「本轮确实完成了一次用户提问」）由**调用点**保证 —— 本函数只在 user 分支、
/// run_query 返回之后被调用，不会在一次提问的工具轮中途触发。
/// 时钟回拨（now_ts < last_ts）用 saturating_sub 得到 0，恒 < 间隔 ⇒ 当作「还没到时间」，
/// 走安全侧（宁可少复盘，不要时钟一乱就狂调模型）。
fn should_review(
    enabled: bool,
    fired_per_scope: [bool; 2],
    last_ts: Option<i64>,
    now_ts: i64,
    scope: tools::MemoryScope,
) -> bool {
    enabled
        && !fired_per_scope[scope.index()]
        && last_ts.map_or(true, |t| now_ts.saturating_sub(t) >= REVIEW_MIN_GAP_SECS)
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
///
/// 2026-10-11（方案甲）：指令段升级为结构化抽取口径（分类写在这里、禁写清单在两个提示词
/// 里各写一份）；出口 NOTHING_TO_REMEMBER 与 REVIEW_MAX_ITEMS 两条上限语义不变。
fn review_prompt(transcript: &str, memory: &str) -> String {
    let memory = memory.trim();
    let memory = if memory.is_empty() {
        "(nothing yet)"
    } else {
        memory
    };
    format!(
        "The conversation below just finished. Extract what, if anything, is worth keeping in \
         long-term memory, then act.\n\n\
         ## Long-term memory right now\n{memory}\n\n\
         ## The conversation (oldest first; tool calls and results are trimmed)\n{transcript}\n\n\
         ## Your task\nSave at most {REVIEW_MAX_ITEMS} durable facts that match the categories in \
         your instructions, with the `Remember` tool, one per call.\n\
         Never write credentials (API keys, tokens, passwords, private keys, connection strings): \
         if the conversation contains any, leave them out entirely — not even partially.\n\
         If nothing qualifies, reply exactly `NOTHING_TO_REMEMBER` and call no tool — no \
         explanation, no step list, no summary of the conversation."
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
            let set = mcp::McpSet::connect(Some(spec), &[], &[], Vec::new(), None, sampling_handler_for);
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
            // 静默不变量：复盘 fork 一个 `task_event` 都不发（无人值守的后台过程）
            emit_subtask: REVIEW_EMIT_SUBTASK,
            // 复盘不登记为 peer：它在提问之间单独跑、没有兄弟，也不该出现在别人的列表里
            peer: None,
            // 复盘是无人值守的短 fork，没有「用户终止」这回事
            cancel: None,
            // 后台复盘单独记一本账（§待办 3-16 P2-8）。
            usage: UsageKind::Review,
        },
        bridge.as_mut(),
        &job.prompt,
        // 后台复盘每次都从零跑（它自己就是「无条件重读一遍」的语义）。
        &[],
    )
    // 复盘只把最终文本交给调用方（`join_review` 只记日志）：`partial` 对复盘无消费者，
    // 撞墙的日志已在 `run_subagent` 内按 `review-*` 的 task_id 落过。
    .map(|o| o.text)
}

/// 复盘 fork 的**启动期常量部分**：工具集 / 技能 / 系统提示词 / 审批开关 / 桥的 spec /
/// 写侧可用性。每一样都在启动时算好后复用 —— 提问循环里只补「这一次的工作面」
/// 写侧可用性。每一样都在启动时算好后复用 —— 提问循环里只补「这一次的工作面」
struct ReviewSetup {
    defs: Vec<Value>,
    skills: Vec<skills::Skill>,
    system: String,
    ask_permission: bool,
    /// `--mcp-server` 的原样值：复盘用它连**自己那条**桥（见 `run_review_fork`）
    bridge_spec: Option<String>,
    /// 写侧是否可用（= remember_on）；假 ⇒ 不跑复盘（见装配处注释）
    enabled: bool,
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
            let trimmed = text.trim();
            // 空转（无新增。*只写一行计。*，不把模型那句正文转录进。—。复盘绝大多数
            // 轮次都是空转，日志里该一眼看出「这次没写记忆」（2026-10-27，§待办 3-16 P0-3）。
            if trimmed.contains("NOTHING_TO_REMEMBER") {
                log::info("后台复盘完成：无新增（计数行 · 未写记忆）");
            } else {
                let head = trimmed.lines().next().unwrap_or("").trim().to_string();
                log::info(format!(
                    "后台复盘完成：{}",
                    crate::log::truncate_chars(&head, 200)
                ));
            }
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

// ── 本地只读预筛的接线（S1 首个切片，2026-10-15）──────────────────────
//
// 语义与摊销口径见 `decide.rs` 头注释。三条铁律（**首切片**）：
//   · **只读**：判定只影响**控制流与账本**，**绝不**改 history / `system` / `tool_defs` /
//     上下文块 ⇒ **不动主请求固定前缀**、不废缓存（docs/code-standards.md §规范 6）；
//   · **默认关 / 未配端点 / 故障 ⇒ 与不接线逐字节一致**（fail-open 落在 `decide::decide`）；
//   · **压缩事件只记账**：四个触发点上只写账本，**绝不改压缩行为**。

/// 是否本轮该做「每 K 轮」预筛：**第 1 轮** + 之后每 K 轮一次。
///
/// 口径 `(turn - 1) % K == 0`（`turn` 从 1 起，即 `question_seq`）。K=1 ⇒ 退化为每轮；
/// K ≥ 轮数 ⇒ 只在首轮判一次（第 2..K 轮的 `(turn-1) < K` 恒不整除）。
fn prescreen_due(turn: u64, k: u64) -> bool {
    k > 0 && turn >= 1 && (turn - 1) % k == 0
}

/// 一次只读预筛：跑判定 + 记账，**永远返回判定**（可注入 decider，便于单测）。
///
/// 输入只用于留**哈希**（`log::hash64`），账本里没有原文（`decisions::append` 再过一遍脱敏）。
fn run_prescreen(
    d: &dyn decide::Decider,
    model: &str,
    input: &str,
    kind: &str,
    scope: tools::MemoryScope,
    turn: u64,
    dir: &Path,
) -> decide::Action {
    let t0 = Instant::now();
    let decision = decide::decide(d, input);
    let latency_ms = t0.elapsed().as_millis() as u64;
    let rec = decisions::DecideRecord {
        ts: now_epoch_secs(),
        kind: kind.to_string(),
        input_hash: format!("{:016x}", log::hash64(input)),
        verdict: decision.action.as_str().to_string(),
        latency_ms,
        model: model.to_string(),
        reason: decision.reason.to_string(),
        fallback: decision.fallback,
        scope: scope.as_str().to_string(),
        turn,
        session_id: session_id().to_string(),
        candidate_count: None,
        candidates: None,
        tool: None,
    };
    decisions::append(dir, scope, &rec);
    decision.action
}

/// 生产入口：**整层关闭 / 未配端点 ⇒ `None`**（调用方据此走现状路径，逐字节不变）。
fn prescreen(input: &str, kind: &str, scope: tools::MemoryScope, turn: u64) -> Option<decide::Action> {
    if !decide::enabled() {
        return None;
    }
    // 未配 `LUNAC_LOCAL_MODEL_URL` ⇒ 整层透明关闭（既有行为，别破坏）。
    let cfg = decide::LocalHttpConfig::from_env()?;
    let dir = decisions::default_dir();
    // 形态 A（零依赖确定性快判）：明显不值得复盘的回合（斜杠命令 / 无实质内容）直接 Skip，
    // 连本地模型都不必问（省一次模型调用）。这也是 `Heuristic` 在生产里的唯一用途，判据同源。
    if decide::obviously_not_reviewable(input) {
        return Some(run_prescreen(&decide::Heuristic, "heuristic", input, kind, scope, turn, &dir));
    }
    // 形态 B：本地模型。任何故障 ⇒ fail-open（照旧派），落在 `decide::decide`。
    // 经 `ChannelGated` 包一层：本地模型通道（判定 / 抽取 / 预热**共用**）忙时**立即 busy-skip**、
    // 绝不排队 —— `llama-server` 是 `-np 1` 单并发槽，排队只会把两个请求都拖到各自超时。
    let model = cfg.model.clone();
    let d = decide::ChannelGated::new(decide::LocalHttp::new(cfg));
    Some(run_prescreen(&d, &model, input, kind, scope, turn, &dir))
}

/// 压缩事件的只读预筛：**只记账，绝不改压缩行为**（四个压缩触发点各调一次）。
fn prescreen_compact(trigger: &str, detail: &str, scope: tools::MemoryScope, turn: u64) {
    if !decide::enabled() {
        return;
    }
    let input = format!("compact-trigger={trigger} {detail}");
    let _ = prescreen(&input, "compact-prescreen", scope, turn);
}

/// 预筛结果是否允许派后台复盘：**只有明确 `Skip` 才拦**（`None` / `Run` / 故障 ⇒ 照旧派）。
fn allow_review_spawn(verdict: Option<decide::Action>) -> bool {
    !matches!(verdict, Some(decide::Action::Skip))
}

/// 冷启动预热（S1，2026-10-15）：**agent 启动时**后台对本地端点发**一次**极短请求。
///
/// 为什么需要：`llama-server` 冷启后**第一次**判定耗时 **>1500ms**（撞 [`decide::LOCAL_DECIDE_TIMEOUT`]）
/// ⇒ 启动后首次预筛静默 fail-open（行为安全，但等于没生效）；热机后正常。预热把模型加载 /
/// 首次 KV / 连接吃进缓存，使首次真实判定落在超时内（上限 2026-10-24 放宽到 3000ms）。
///
/// 三条硬约束：① **后台线程，绝不阻塞启动 / 主循环**（本函数立刻返回）；
/// ② 只在「开关开 + 端点已配」时预热（[`decide::warmup_config`]），**默认关 ⇒ 零预热、零请求**；
/// ③ 失败静默（[`decide::warmup_with`] 内部只记 info，绝不上抛）。**只打本地端点**，绝不碰云端点。
/// 预热**不进决策账本**、**不影响预筛语义与消费点**、**不动固定前缀**。
/// ⚠️ 预热用自己的更宽上限（[`decide::WARMUP_TIMEOUT`]）而非判定那条上限 —— 冷启第一次推理
/// 本就更慢，沿用判定上限会把预热自身提前掐掉（理由见该常量注释）。
fn spawn_local_decide_warmup() {
    let Some(cfg) = decide::warmup_config() else {
        return; // 默认关 / 未配端点 ⇒ 不预热（零请求）
    };
    thread::spawn(move || {
        // 预热与判定 / 抽取**共用同一把本地通道锁**（否则预热仍会和判定抢那唯一一个并发槽）：
        // 拿不到锁 ⇒ 立即跳过（`warmup_with` 把 `Err(BUSY_SKIP)` 记成一行 info，静默无害）。
        let d = decide::ChannelGated::new(decide::LocalHttp::new(cfg));
        decide::warmup_with(&d);
    });
}

// ── 记忆候选抽取的接线（S1 第二切片，2026-10-15）──────────────────────
//
// 语义见 `memory_candidates.rs` 头注释。三条铁律（与 S1 首切片同源）：
//   · **只落账本**：候选只写决策账本，**绝不**写 `MEMORY.md` / **绝不**改 history / `system` /
//     上下文块 / **不做任何注入** ⇒ **开关开与关，可见行为逐字节不变**（只有账本多一种 `kind`）；
//   · **默认关 / 未配端点 / 故障 ⇒ 与不接线逐字节一致**（fail-open 落在 `memory_candidates::extract`）；
//   · **摊销 = 每 K 轮 + 压缩事件**：复用 `decide::amortize_k()` / `prescreen_due` 与既有接线点，
//     **不新造计时器、不新开账本**（docs/forbidden.md §禁用 62）。

/// 喂给本地抽取的回合快照上限（字符）。**硬上限**：本地 4B 的 prefill / 上下文都吃紧，
/// 比复盘快照（`REVIEW_TRANSCRIPT_CHARS` = 6000）更短；history→文本复用 `review_transcript`
/// （**同一套机制**，不另写一份 history 序列化）。
const CANDIDATE_TRANSCRIPT_CHARS: usize = 4_000;

/// 一次记忆候选抽取：跑抽取 + 记账，**永远返回结果**（可注入 extractor，便于单测）。
///
/// **只落账本、不落原文**：每候选只换算成 `hash` + `len`（见 `decisions::CandidateRef`）；
/// 输入只留 `hash`（`log::hash64`）。账本再过一遍 `decisions::append` 的脱敏。
fn run_memory_extraction(
    e: &dyn memory_candidates::Extractor,
    model: &str,
    input: &str,
    scope: tools::MemoryScope,
    turn: u64,
    dir: &Path,
) -> memory_candidates::Extraction {
    let t0 = Instant::now();
    let out = memory_candidates::extract(e, input);
    let latency_ms = t0.elapsed().as_millis() as u64;
    let verdict = if out.fallback {
        "fallback"
    } else if out.facts.is_empty() {
        "empty"
    } else {
        "ok"
    };
    let candidates: Vec<decisions::CandidateRef> = out
        .facts
        .iter()
        .map(|f| decisions::CandidateRef {
            hash: format!("{:016x}", log::hash64(f)),
            len: f.chars().count() as u32,
        })
        .collect();
    let rec = decisions::DecideRecord {
        ts: now_epoch_secs(),
        kind: "memory-candidates".to_string(),
        input_hash: format!("{:016x}", log::hash64(input)),
        verdict: verdict.to_string(),
        latency_ms,
        model: model.to_string(),
        reason: out.reason.to_string(),
        fallback: out.fallback,
        scope: scope.as_str().to_string(),
        turn,
        session_id: session_id().to_string(),
        candidate_count: Some(candidates.len() as u32),
        candidates: Some(candidates),
        tool: None,
    };
    decisions::append(dir, scope, &rec);
    out
}

/// 生产入口：**开关关 / 未配端点 ⇒ 不做任何事**（零请求、零账本行）。
///
/// 单发（`--print`）不在调用点触发：进程马上退出、白等一次本地调用（同理见复盘）。
/// 本函数自身**同步**执行（与 S1 预筛同款）—— 单次消耗由 [`memory_candidates::LOCAL_EXTRACT_TIMEOUT`]
/// 兜底（抽取比判定慢一个量级，用的是**更宽但仍硬**的独立上限，理由见该常量）。
fn extract_candidates(history: &[Value], scope: tools::MemoryScope, turn: u64) {
    extract_candidates_into(&decisions::default_dir(), history, scope, turn);
}

/// [`extract_candidates`] 的可测内核（账本目录**显式传入**，便于单测断言「默认关 ⇒ 零账本行」）。
fn extract_candidates_into(dir: &Path, history: &[Value], scope: tools::MemoryScope, turn: u64) {
    if !decide::enabled() {
        return;
    }
    let Some(cfg) = decide::LocalHttpConfig::from_env() else {
        return;
    };
    let model = cfg.model.clone();
    let input = review_transcript(history, CANDIDATE_TRANSCRIPT_CHARS);
    // 经 `ChannelGated` 包一层：本地模型通道（判定 / 抽取 / 预热**共用**）忙时**立即 busy-skip**、
    // 绝不排队（抽取上限 5s 比判定 1.5s 宽，真排队时两条都等到超时 ⇒ 双双 fail-open）。
    let _ = run_memory_extraction(
        &memory_candidates::ChannelGated::new(memory_candidates::LocalExtract::new(cfg)),
        &model,
        &input,
        scope,
        turn,
        dir,
    );
}

// ── 安全预筛的接线（S1 第四切片，2026-10-24）──────────────────────────
//
// 语义见 `safety_prescreen.rs` 头注释。三条铁律（与前三个切片同源）：
//   · **只记账**：不改审批 / 不放行 / 不拦截 / 不改任何工具结果 ⇒ 开关开与关，
//     用户可见行为逐字节一致（只有账本多一种 `kind`）；
//   · **默认关 / 未配端点 / 故障 ⇒ 与不接线逐字节一致**（fail-open 落在 `safety_prescreen::screen`）；
//   · **不阻塞工具路径**：一律 `thread::spawn`（fire-and-forget），与预热同款。

/// 「需审批的工具调用」的**进程级采样序号**（安全预筛每 K 次判一次）。
///
/// 为什么用进程级而不是「本轮第几次」：`run_tool` 是自由函数、拿不到轮号；本切片是采样留痕，
/// 序号跨轮连续不改变语义。**单测直接测纯函数** `prescreen_due`，不依赖这个计数器。
static SAFETY_TOOL_SEQ: AtomicU64 = AtomicU64::new(0);

/// 生产入口（在 `run_tool` 里、工具**执行前**调用）：决定要不要派一次后台预筛。
///
/// 三条前置（任一不满足 ⇒ 直接返回，零线程零请求）：① 开关开 + 端点已配；
/// ② 该工具确实「需人工确认」（复用 `needs_approval`，**不另抄名单**）；
/// ③ 采样到点（每 K 次，K 复用 `decide::amortize_k()`）。
///
/// ⚠️ `AskUserQuestion` **排除**：它是交互 UI，不碰本机、也不产生可评估的危险度。
fn maybe_safety_prescreen(name: &str, input: &Value) {
    if !decide::enabled() || !needs_approval(name) || name == "AskUserQuestion" {
        return;
    }
    let seq = SAFETY_TOOL_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    if !prescreen_due(seq, decide::amortize_k()) {
        return;
    }
    let Some(cfg) = decide::LocalHttpConfig::from_env() else {
        return;
    };
    let model = cfg.model.clone();
    let tool = name.to_string();
    let text = safety_prescreen_input(name, input);
    // fire-and-forget：**绝不在工具路径上等待**（判定上限 1.5s，会拖慢每次写类操作）。
    thread::spawn(move || {
        let dir = decisions::default_dir();
        let _ = run_safety_prescreen(
            &safety_prescreen::LocalScreen::new(cfg),
            &model,
            &tool,
            &text,
            &dir,
        );
    });
}

/// 预筛输入：工具名 + **已脱敏**的参数摘要（复用 `tools::summarize_args`：脱敏 + 400 字截断）。
///
/// 不另造序列化：摘要与日志里那份同源，避免「日志看到 A、判定用的是 B」。
fn safety_prescreen_input(name: &str, input: &Value) -> String {
    format!("tool={name} {}", tools::summarize_args(input))
}

/// 一次安全预筛：跑判定 + 记账，**永远返回结果**（可注入 screener，便于单测）。
fn run_safety_prescreen(
    s: &dyn safety_prescreen::Screener,
    model: &str,
    tool: &str,
    input: &str,
    dir: &Path,
) -> safety_prescreen::Verdict {
    let t0 = Instant::now();
    let v = safety_prescreen::screen(s, input);
    let latency_ms = t0.elapsed().as_millis() as u64;
    let rec = decisions::DecideRecord {
        ts: now_epoch_secs(),
        kind: "safety-prescreen".to_string(),
        input_hash: format!("{:016x}", log::hash64(input)),
        verdict: v.risk.as_str().to_string(),
        latency_ms,
        model: model.to_string(),
        reason: v.reason.to_string(),
        fallback: v.fallback,
        scope: tools::MemoryScope::Global.as_str().to_string(),
        turn: 0,
        session_id: session_id().to_string(),
        candidate_count: None,
        candidates: None,
        tool: Some(tool.to_string()),
    };
    decisions::append(dir, tools::MemoryScope::Global, &rec);
    v
}

// ── 能力路由的接线（S1 第五切片，2026-10-24）────────────────────────────
//
// 语义见 `capability_route.rs` 头注释。三条铁律：
//   · **只记账**：路由结果**没有任何消费者**（改工具数组会废固定前缀缓存、往消息里塞提示会动
//     主请求前缀，两条都违反 S1 硬约束）⇒ 开关开与关，可见行为逐字节一致；
//   · **默认关 / 未配端点 / 故障 ⇒ 与不接线逐字节一致**（fail-open 落在 `capability_route::route`）；
//   · **在回合收尾判定**（用户已在看回复），同步执行、不 spawn、不阻塞模型请求。

/// 路由判定的输入上限：用户提问可能很长，本地 4B 的 prefill 吃紧 ⇒ 只取头部。
/// 与复盘快照（6000）/ 候选抽取（4000）分开定义：这里判的是「意图」，几百字足够。
const ROUTE_INPUT_CHARS: usize = 800;

/// 一次能力路由：跑判定 + 记账，**永远返回结果**（可注入 router，便于单测）。
fn run_capability_route(
    r: &dyn capability_route::Router,
    model: &str,
    prompt: &str,
    scope: tools::MemoryScope,
    turn: u64,
    dir: &Path,
) -> capability_route::Routing {
    let input = log::truncate_chars(prompt.trim(), ROUTE_INPUT_CHARS);
    let t0 = Instant::now();
    let out = capability_route::route(r, &input);
    let latency_ms = t0.elapsed().as_millis() as u64;
    let rec = decisions::DecideRecord {
        ts: now_epoch_secs(),
        kind: "capability-route".to_string(),
        input_hash: format!("{:016x}", log::hash64(&input)),
        verdict: out.route.as_str().to_string(),
        latency_ms,
        model: model.to_string(),
        reason: out.reason.to_string(),
        fallback: out.fallback,
        scope: scope.as_str().to_string(),
        turn,
        session_id: session_id().to_string(),
        candidate_count: None,
        candidates: None,
        tool: None,
    };
    decisions::append(dir, scope, &rec);
    out
}

/// 生产入口：**开关关 / 未配端点 ⇒ 不做任何事**（零请求、零账本行）。
fn route_capability(prompt: &str, scope: tools::MemoryScope, turn: u64) {
    if !decide::enabled() {
        return;
    }
    let Some(cfg) = decide::LocalHttpConfig::from_env() else {
        return;
    };
    let model = cfg.model.clone();
    let dir = decisions::default_dir();
    let _ = run_capability_route(
        &capability_route::LocalRouter::new(cfg),
        &model,
        prompt,
        scope,
        turn,
        &dir,
    );
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

/// `PetGrowth` / `PetPersonaSuggest` 的**前置判据**（不碰桥，故可单测）：通过则返回要写的内容。
///
/// 三关（任一不过即拒，**不写任何东西**）：
///   ① **仅桌宠轮**（`tctx.memory_scope == Pet`）—— 非桌宠轮调用一律拒；
///   ② 写类档位（只读 / 计划相位）直接拒（与 `Remember` 同源 `write_blocked`）；
///   ③ `PetGrowth` **每轮 ≤ 1 条**（`PET_GROWTH_USED_THIS_TURN`）；内容非空。
fn pet_self_write_gate<'a>(
    tctx: &tools::Ctx,
    name: &str,
    input: &'a Value,
) -> Result<&'a str, String> {
    if tctx.memory_scope != tools::MemoryScope::Pet {
        return Err(format!(
            "{name} is only available during a desktop-pet turn (the user message must carry \
             source=\"pet\"); it does not apply to normal chat."
        ));
    }
    if let Some(why) = tools::write_blocked(tctx, name) {
        return Err(why);
    }
    let content = input.get("content").and_then(Value::as_str).unwrap_or("").trim();
    if content.is_empty() {
        return Err("缺少 content 参数：要记的内容不能为空".into());
    }
    if name == "PetGrowth" && PET_GROWTH_USED_THIS_TURN.swap(true, Ordering::Relaxed) {
        return Err("每轮最多追加一条成长笔记（本轮已追加过一条）。".into());
    }
    Ok(content)
}

fn dispatch_tool(
    tctx: &tools::Ctx,
    mcp_bridge: Option<&mut mcp::McpSet>,
    skill_list: &[skills::Skill],
    name: &str,
    input: &Value,
) -> Result<String, String> {
    // 结果**不在这里**截断 —— 单条输出预算统一由 run_tool 的
    // `tools::apply_budget` 收口（那里才知道工具名，超长要落盘）。

    // `CronCreate` 的**执行前硬闸**（2026-10-17 阶段 1）：即使用户加了
    // `--dangerously-skip-permissions`（= `UnattendedAllow`），也不得静默放行。
    // 它持续产生 API 调用并写持久化定义，用户已裁定「所有运行模式都强制弹卡」；
    // 单发 / 跳过审批的路径没有回包通道，因此只能**明确拒绝**（不是静默挂起）。
    // 常驻 stdio 路径（`Stdio`）的审批在前面的 `needs_approval_with` 已发出请求，
    // 走到这里说明用户已在卡上允许（或 hook allow，但 `hook_allow_needs_card` 对
    // CronCreate 恒为 true，故 hook 也翻不过去）。
    if name == "CronCreate" && approval_policy() != ApprovalPolicy::Stdio {
        return Err(
            "CronCreate requires an interactive approval card: it creates a recurring, \
             cost-producing task. It cannot be created from --print / \
             --dangerously-skip-permissions runs; ask the user to create it from an \
             interactive session instead."
                .into(),
        );
    }
    // `TaskEnqueue`（2026-10-22，S2 通用任务队列）：同一条硬闸 —— 单发 / skip-permissions
    // 没有回包通道，后台长任务必须由交互会话里的一次明确批准来创建。
    if name == "TaskEnqueue" && approval_policy() != ApprovalPolicy::Stdio {
        return Err(
            "TaskEnqueue requires an interactive approval card: it queues cost-producing \
             background work. It cannot be created from --print / \
             --dangerously-skip-permissions runs; ask the user to enqueue it from an \
             interactive session instead."
                .into(),
        );
    }

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
        // 只读档下**退出计划模式没有意义**：写类工具被用户档位永久拒绝，批准了计划也
        // 执行不了 —— 说「批准后就能写」是把模型引到一个永远失败的动作上。
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
        // §待办 3-28.3：前端可以在批准前逐条改计。—。改过的计划经审批回包。
        // `updatedInput`（`{plan, edited: true}`）覆盖到这里。*它就是用户批准的那一。*。
        // 不把它交回模型，用户删掉的步骤照样会被执行，那张卡等于装饰。
        let edited_plan = input.get("plan").and_then(Value::as_str).unwrap_or("").trim();
        if input.get("edited").and_then(Value::as_bool) == Some(true) && !edited_plan.is_empty() {
            return Ok(format!(
                "{PLAN_MODE_OFF_NOTE}\n\nThe user edited this plan before approving it. Treat \
        this list as the authoritative tasks — do not run anything the user removed:\n\
                 {edited_plan}"
            ));
        }
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
    // 长期记忆的只读检索（§待办 3-9 B 项）：同样走桥的自定义方法（`lunac/memory_read`），
    // 因此也要在 `is_mcp` 之前处理。**失败回空结果而不是硬错误**（桥不可用 / 记忆为空 /
    // 无匹配都带 reason）—— 与 `SessionSearch` 的「没桥就如实报错」不同，检索是锦上添花，
    // 不该为一条查不到的记忆把工具结果标成 is_error。
    if name == "MemorySearch" {
        return Ok(memory_search(mcp_bridge, input));
    }
    // 资源库来源（A5，2026-10-18）：读 / 写各走桥；校验 / 诊断是本地（读文件 + 一次请求）。
    // 与 MemorySearch 同一条纪律 —— 必须在 `needs_bridge` 之前处理（它们不带 `mcp__` 前缀）。
    if name == "ResourceHubSearch" {
        let Some(bridge) = mcp_bridge else {
            return Err(format!("MCP bridge is not connected, cannot call {name}"));
        };
        let raw = bridge.resource_hub_sources()?;
        return Ok(resource_hub_tools::filter_sources(&raw, input));
    }
    if name == "SourceValidate" {
        let path = input
            .get("file_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if path.is_empty() {
            return Err("缺少 file_path 参数".into());
        }
        return resource_hub_tools::validate_file(tctx, path).map(|v| v.to_string());
    }
    if name == "SourceDiagnose" {
        return resource_hub_tools::diagnose(tctx, input).map(|v| v.to_string());
    }
    // A7 对话入口（2026-10-27）：纯数据，不联网、不开窗、不写盘 ⇒ 只读档 / 单发都能用。
    // 真正的开窗由前端点结果卡时走宿主 `open_plugin_window`（工具不替用户弹窗）。
    if name == "ResourceHubOpen" {
        return resource_hub_tools::open_request(input);
    }
    if name == "SourceInstall" {
        // 写类：只读档 / 计划相位直接拒（这条早退绕过了 `tools::run` 的统一拦截）。
        if let Some(why) = tools::write_blocked(tctx, "SourceInstall") {
            return Err(why);
        }
        let path = input
            .get("file_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if path.is_empty() {
            return Err("缺少 file_path 参数".into());
        }
        let source = resource_hub_tools::read_source_file(tctx, path)?;
        let report = resource_hub_tools::validate_source(&source);
        if !report.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            return Err(format!("来源校验未通过，未安装：{report}"));
        }
        let Some(bridge) = mcp_bridge else {
            return Err(format!("MCP bridge is not connected, cannot call {name}"));
        };
        let force = input
            .get("force")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        return bridge.resource_hub_install(&source, force);
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
            // **写哪一份由本轮作用域决定**（`tctx.memory_scope`，桌宠轮 = 桌宠私有）。
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
                        // 记忆页（§待办 3-24）：只透传，形状由宿主 `parse_memory_page` 唯一校验
                        // （agent 侧不另抄一份规则，否则两处判据迟早漂移）。
                        let page = input
                            .get("page")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty());
                        bridge.memory_write(
                            content,
                            replace,
                            // 技。fork 里优先写技能命名空间（§待办 3-28.10 ①）。
                            // 写全局会污染主对话每轮都注入的那一份。
                            if tctx.skill_scope.is_some() { "skill" } else { tctx.memory_scope.as_str() },
                            tctx.skill_scope.as_deref(),
                            page,
                        )
                    }
                }
            }
            // 桌宠自我区（2026-10-13 人格地基）：两条都**只追加**、**只在桌宠轮可用**、
            // `PetGrowth` 每轮 ≤ 1 条。三关判据抽在 `pet_self_write_gate`（可单测，不碰桥）。
            // ⚠️ 这里**没有**任何写核心人格的分支 —— 建议只能进建议区，合并由用户在界面上做。
            "PetGrowth" | "PetPersonaSuggest" => match pet_self_write_gate(tctx, name, input) {
                Err(why) => Err(why),
                Ok(content) if name == "PetGrowth" => bridge.pet_growth_write(content),
                Ok(content) => bridge.persona_suggestion_write(content),
            },
            // 显式兜底（**不要**写成 `_ => read_resource`）：`BRIDGE_TOOLS` 里新加一件却忘了
            // 在这里接线时，宁可回一句「实现缺失」，也不要静默地按 read_resource 去执行。
            other => Err(format!(
                "{other} 的桥实现缺失：tools::BRIDGE_TOOLS 与 dispatch_tool 不同步"
            )),
        };
    }
    // SendUserFile / PushNotification（A13，2026-10-11）：效果全在**前端**（一张文件卡 /
    // 一条桌面通知）。`tools.rs` 是纯实现层、拿不到 `emit`，所以结构化载荷在这里发成
    // `system` 事件，前端 `main.ts` 据此落卡 / 弹通知 —— 与 `PetControl` 同型。
    // 两件都免审批（判据见 `tools::needs_approval`），在只读档 / 计划相位下也放行。
    if name == "SendUserFile" {
        let (text, payload) = tools::send_user_file(tctx, input)?;
        emit(json!({ "type": "system", "subtype": "user_file", "file": payload }));
        return Ok(text);
    }
    if name == "PushNotification" {
        let (text, payload) = tools::push_notification(input)?;
        emit(json!({ "type": "system", "subtype": "push_notification", "notification": payload }));
        return Ok(text);
    }
    // Preview（预览窗标签。026-11-14）：。SendUserFile 同型 —。效果全在前端。
    // `tools.rs` 拿不。`emit`，所以这里补一。`system/preview_open`。
    // v2 聊天面板据此把标签放进预览窗队列；v1 前端不认识这。subtype，会忽略（不影响回答）。
    if name == "Preview" {
        let text = tools::preview_tab(input)?;
        emit(json!({ "type": "system", "subtype": "preview_open", "tab": input }));
        return Ok(text);
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
/// | `EnterWorktree` / `ExitWorktree`（2026-10-11） | 有效工作目录是**主循环**的状态（`Ctx::worktree` 跨线程共享）。子代理若改它，会把主代理的相对路径解析一起带走 —— 同 `EnterPlanMode` 的判据 |
/// | `PetControl`（2026-10-10） | 它的效果由**主进程前端**读**助手流**里的 `tool_use` 后广播给桌宠窗执行；子代理的 tool_use 只以 `task_event` 转发（纯展示，不触发效果处理器）⇒ 在里面调它是**保证的空操作** |
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
            !matches!(
                name,
                // `PetControl`（2026-10-10）：效果由**主进程前端**读**助手流**里的 `tool_use` 后
                // 广播给桌宠窗；子代理的 tool_use 只以 `task_event` 转发（纯展示，**不触发**
                // 前端的效果处理器）⇒ 在里面调它仍是**保证的空操作**，与走桥那一类同理，剔掉。
                "Agent" | "EnterPlanMode" | "ExitPlanMode" | "PetControl"
                    // `EnterWorktree` / `ExitWorktree`（2026-10-11）：有效工作目录是**主循环**的
                    // 状态（`Ctx::worktree` 跨线程共享），子代理改它会把主代理的相对路径解析
                    // 一起带走 —— 同 `EnterPlanMode` 的判据，剔掉。
                    | "EnterWorktree" | "ExitWorktree"
                    // `SendUserFile` / `PushNotification`（2026-10-11）：效果是**主进程前端**
                    // 的卡片 / 桌面通知（由 `dispatch_tool` 发 `system` 事件驱动）；子代理的
                    // 工具调用只以 `task_event` 转发（纯展示）⇒ 在里面调它只会**凭空冒出一张
                    // 无上下文的卡**。同 `PetControl` 的判据，剔掉。
                    | "SendUserFile" | "PushNotification"
                    // `CronList`（2026-10-17 阶段 0）：cron job 是**用户级**的持续成本配置，
                    // 子代理「同批并发、跑完即散」，不该让它在里面读到 / 引用定时任务；
                    // 阶段 1 的 `CronCreate` / `CronDelete` 同样在这里被剔掉。
                    | "CronList" | "CronCreate" | "CronDelete"
                    // 通用任务队列（2026-10-22）：同样是**用户级**后台成本配置，
                    // 子代理里既不该读也不该入队 / 取消。
                    | "TaskEnqueue" | "TaskList" | "TaskCancel"
            )
                && !tools::needs_bridge(name)
                && !mcp::is_mcp(name)
        })
        .cloned()
        .collect()
}

/// 按角色裁工具（2026-10-24）。**在 `subagent_tool_defs()` 之后执行**：
/// 先剔掉 Agent / 桥 / 定时任务等（那一层是安全底线），再按角色白名单收窄。
/// `General` 不裁 ⇒ 老调用方的工具数组逐字节不变。
fn subagent_role_defs(defs: &[Value], role: subagent_role::Role) -> Vec<Value> {
    if role.allowed_tools().is_none() {
        return defs.to_vec();
    }
    defs.iter()
        .filter(|d| {
            let name = d.get("name").and_then(Value::as_str).unwrap_or("");
            role.allows(name)
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

// ── 「先解释后执行」闸（2026-11-05，§待办 1-16 第 1 条）──────────────
//
// 判据只做一次（纯函数、可单测）：用户消息里出现明确的**顺序词**时，本回合视为
// 「只读计划轮」—— 写类 / 命令类 / 派子代理 / MCP 一律在**审批卡之前**拒掉，
// 模型只能输出说明与一个「是否继续」的问句。下一轮用户确认后自然解除（闸是逐轮的
// 局部变量，不写全局状态、不新增 stream-json 事件）。
//
// 为什么不用 `needs_approval_with` 直接当判据：它把 `WebSearch` / `WebFetch` /
// `AskUserQuestion` 也算「需审批」，但那三件是**读 / 交互**，不属于「动手」。

/// 用户消息里的顺序词。命中任意一条 ⇒ 本轮先解释。
const EXPLAIN_FIRST_MARKERS: &[&str] = &[
    "先解释",
    "先说清楚",
    "先说明",
    "先讲清楚",
    "先回答",
    "先告诉我",
    "先告知",
    "先讲",
    "才能继续",
    "再继续改",
    "先别改",
    "先不要改",
    "先别动",
    "先不要动",
    "先不动手",
    "explain first",
    "before you continue",
    "before continuing",
    "ask before",
];

const EXPLAIN_FIRST_NEGATIONS: &[&str] = &[
    "不用先解释",
    "不需要先解释",
    "无需先解释",
    "不用解释",
    "不用先说明",
    "直接改",
];

/// 本条用户消息是否要求「先解释、再决定是否继续」。
fn explain_first_requested(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    if EXPLAIN_FIRST_NEGATIONS.iter().any(|m| lower.contains(m)) {
        return false;
    }
    EXPLAIN_FIRST_MARKERS.iter().any(|m| lower.contains(m))
}

/// 只读计划轮里是否应当拒绝这个工具。
///
/// 放行：`Read` / `Glob` / `Grep` 等只读工具，`TodoWrite` / `SendUserFile` /
/// `PushNotification` 等无本机副作用的工具，以及 `WebSearch` / `WebFetch` /
/// `WebBrowser` / `AskUserQuestion` / `EnterPlanMode`。
/// 拒绝：写文件、命令、`Agent`（能间接写）、fork 技能、写记忆、worktree、图片生成，
/// 以及无法判断读写的 MCP 工具（按外部副作用保守处理）。
fn explain_first_blocks(name: &str, input: &Value, skills: &[skills::Skill]) -> bool {
    if matches!(
        name,
        "WebSearch" | "WebFetch" | "WebBrowser" | "AskUserQuestion" | "EnterPlanMode"
    ) {
        return false;
    }
    if name == "CronDelete" {
        return true;
    }
    if name.starts_with("mcp__") {
        return true;
    }
    needs_approval_with(name, input, skills)
}

/// 被「先解释」闸拒绝时的文案。必须明确告诉模型**下一步该做什么**，
/// 否则它会换一个写类工具再试一遍。
fn explain_first_denial(
    explain_first: bool,
    name: &str,
    input: &Value,
    skills: &[skills::Skill],
) -> Option<String> {
    if !explain_first || !explain_first_blocks(name, input, skills) {
        return None;
    }
    Some(format!(
        "Denied for this turn: the user asked you to explain first. Do not call {name} (or any \
         other write / command / agent / MCP tool) now. Explain what you found and what you \
         intend to change, then ask the user whether to continue. Wait for their confirmation \
         before acting."
    ))
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

/// 前缀稳定性的告警线（2026-10-27，§待办 3-16 P1-4）：相邻请求的 history 公共前缀
/// 掉到这个百分比以下就 `warn`。
///
/// 为什么是 90：纯追加时公共前缀**恒等于**上轮条数（100%）；唯一的正当下跌是压缩
/// （`compacted_this_round` / `compacted_for_retry`，那两档只记 info）。夹在中间的
/// 「只改了一条」也该报 —— 它同样会让其后整段缓存作废。
const PREFIX_STABILITY_MIN_PERCENT: usize = 90;
/// 少于这么多条不判：一问刚开始时 history 只有两三条，任何一次重排都会显得「塌了」。
const PREFIX_STABILITY_MIN_ITEMS: usize = 8;

/// 公共前缀是否已跌破告警线；`Some(百分比)` = 该告警（或压缩时记 info），`None` = 正常。
///
/// 抽成纯函数是为了可单测：阈值与「条数太少不判」这两条口径不依赖运行时状态。
fn prefix_stability_break(cur_common: usize, prev_len: usize) -> Option<usize> {
    if prev_len < PREFIX_STABILITY_MIN_ITEMS {
        return None;
    }
    // 先算百分比再比较：整数除法会向下取整，边界（正好 90%）判为稳定，符合「90% 以下才告警」。
    let pct = cur_common.saturating_mul(100) / prev_len;
    (pct < PREFIX_STABILITY_MIN_PERCENT).then_some(pct)
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

/// 落一份检查点（回合末尾与每批工具结果后共用）。
///
/// 写失败只 warn —— 续跑是增强能力，不能反过来中断对话。
fn save_checkpoint(
    enabled: bool,
    question_seq: u64,
    task_budget_used: u64,
    approval: &checkpoint::ApprovalContext,
    history: &[Value],
) {
    if !enabled {
        return;
    }
    if let Err(e) = checkpoint::save(session_id(), question_seq, task_budget_used, approval, history) {
        log::warn(format!("检查点写入失败：{e}"));
    }
}

/// 一轮对话的**内部相位**（2026-11-05，§待办 1-17 第 1 条）。
///
/// 只在 core-agent 内部与落盘日志里使用，**不进 stream-json**（协议不变，v2 也无需改）。
///
/// 完整链路：`Idle → Planning(只读) → AwaitingUser → Done/Interrupted`，或
/// `Idle → Executing → Done/Interrupted`。
///   · `Planning`：用户要求「先解释」的只读计划轮；写类工具一律拒绝并转 `AwaitingUser`。
///   · `AwaitingUser`：已拦下写操作、等用户确认；本轮剩余写操作继续拒（不能换个工具再试）。
///   · `Executing`：正常执行轮，写类工具按审批档位走。
///   · `Interrupted`：用户暂停 / 拒绝并中断；`Done`：正常收口（含预算 / 轮次硬收口）。
///
/// 为什么没有 `reviewing`：后台复盘（`run_review_fork`）是**轮次之外的异步 fork**，
/// 不改变本轮相位，故不塞进这张表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnPhase {
    Idle,
    Planning,
    AwaitingUser,
    Executing,
    Done,
    Interrupted,
}

impl TurnPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Planning => "Planning",
            Self::AwaitingUser => "AwaitingUser",
            Self::Executing => "Executing",
            Self::Done => "Done",
            Self::Interrupted => "Interrupted",
        }
    }

    /// 只有 `Executing` 允许写；其余相位都只读（`AwaitingUser` 尤其如此 ——
    /// 用户还没答应继续，模型不能换个工具再试）。
    fn allows_writes(self) -> bool {
        self == Self::Executing
    }

    /// 合法迁移表（§待办 1-17 第 1 条）。`Done` / `Interrupted` 是终态。
    ///
    /// **不把 `Planning → Executing` 写进去**：一旦本轮被判为「先解释」，同一轮内
    /// 用户还没回答，不能靠模型自己「解释完了」就切到可写；确认一定来自下一条用户消息
    /// （新的一轮从 `Idle` 重新开始）。
    fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Idle, Self::Planning | Self::Executing)
                | (
                    Self::Planning,
                    Self::AwaitingUser | Self::Done | Self::Interrupted
                )
                | (Self::AwaitingUser, Self::Done | Self::Interrupted)
                | (Self::Executing, Self::Done | Self::Interrupted)
        )
    }
}

/// 相位迁移只在**变化时**记一行，避免每轮刷日志。
/// 非法迁移照旧执行（不能让状态机把对话卡死），但必须留 warn 供事后对账。
fn set_turn_phase(cur: &mut TurnPhase, next: TurnPhase, question_seq: u64) {
    if *cur == next {
        return;
    }
    if cur.can_transition_to(next) {
        log::info(format!(
            "回合状态 q{question_seq}: {} → {}",
            cur.as_str(),
            next.as_str()
        ));
    } else {
        log::warn(format!(
            "回合状态 q{question_seq}: 非法迁移 {} → {}（已强制执行）",
            cur.as_str(),
            next.as_str()
        ));
    }
    *cur = next;
}

fn run_query(
    cfg: &Cfg,
    history: &mut Vec<Value>,
    // 上一次请求的 history 逐条指纹（**跨提问**存活，见 `main()` 里那处声明的注释）：
    // 每问的首请求因此能和**上一问的末请求**比，A16 的判据才立得住。
    prev_hist_hashes: &mut Vec<u64>,
    // 第几次用户提问（只进日志：`请求前缀` / `请求用量` 两行都带它）
    question_seq: u64,
    // S2 检查点是否开启（`--checkpoint` / `--resume` / `--continue`）；也决定中间态是否落盘。
    checkpointing: bool,
    // 审批 / 工作区口径（随中间态检查点一起落盘）。
    approval: &checkpoint::ApprovalContext,
    // 任务级预算（跨提问累计，见 `TaskBudget`）。`0` = 关闭 ⇒ 本参数全程只记录不拦。
    task_budget: &mut TaskBudget,
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
    // 上下文块（长期记忆，见 `context_block_message`）。**不在 system 里**
    // —— 它随本问幂等注入 history 开头（2026-10-06，M2-16 方案 D）。
    // 可变：压缩边界会重读记忆并**写回**它（`refresh_memory_snapshot`，§待办 3-9 C）——
    // 全局轮传的是主循环跨提问持有的那一份，写回后下一轮才不会退回旧快照。
    ctx_block: &mut Option<Value>,
    // 「按轮思考覆盖」（2026-10-09）：`Some(_)` = 本条提问**只用这个形态**（不读全局开关；
    // 400 降级也只在本轮内前进、**不写回** `cfg.thinking` 缓存）；`None` = 用全局开关（现状）。
    thinking_override: Option<Thinking>,
    // ── 中途三态控制（§待办 3-28.1）──────────────────────────────
    // stdin 的接收端：**只在轮首** `try_recv` 一次（见 `drain_steering`）。不用
    // `recv_timeout` —— 回合不能被「没有输入」拖慢。
    steer_rx: &mpsc::Receiver<Value>,
    // 回合中分流出来的**非插入**消息（set_history / 普通 user / …）：本回合不处理，
    // 原样留给主循环，保证不丢、不乱序。
    steer_backlog: &mut VecDeque<Value>,
) {
    let started = Instant::now();
    // 本条提问的思考覆盖（可变：400 降级时在本轮内推进）。
    let mut thinking_override = thinking_override;
    emit_init(&cfg.model, tool_names, system_prompt, ctx_block.as_ref());

    // 写侧工具是否真的在池里（= 条件注册的结果；压缩边界刷新记忆时，注入文案里的
    // 「用 `Remember` 追加」必须与它同源，见 §11 规则 56「停注与注册必须同源」）。
    let remember_on = tool_names.iter().any(|n| n == "Remember");
    let pet_growth_on = tool_names.iter().any(|n| n == "PetGrowth");

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

    // 「先解释后执行」闸（2026-11-05）：逐轮判定，不写全局状态、不新增事件；
    // 下一轮用户确认后自然解除（新的 prompt 重新计算）。
    let explain_first = explain_first_requested(prompt);
    // 回合状态机：每轮从 Idle 起步，再进「只读计划轮（先解释）」或「正常执行轮」。
    let mut turn_phase = TurnPhase::Idle;
    set_turn_phase(
        &mut turn_phase,
        if explain_first {
            TurnPhase::Planning
        } else {
            TurnPhase::Executing
        },
        question_seq,
    );
    if explain_first {
        log::info("explain-first 闸：本轮按只读计划轮处理（写类工具会在审批前被拒）");
    }

    // ── 上下文块幂等注入（2026-10-06，M2-16 方案 D）────────────────────
    // 注在 history **开头**（位置选择的理由见 `context_block_message`）。
    // 做法：先摘掉任何在册的旧注入块、再把本轮这块放到开头 —— 于是
    //   · 全局轮（块内容**只在压缩边界**才变）：稳态下与「只注一次」逐字节等价，缓存不变量不受影响；
    // 做法：先摘掉任何在册的旧注入块、再把本轮这块放到开头 —— 于是
    //   · `set_history`（回退 / 恢复会话）整份换掉 history 后，这里能把它补回去
    // 做法：先摘掉任何在册的旧注入块、再把本轮这块放到开头 —— 于是
    // **必须排在 `base` 之前**：`base` 是「本轮压入的消息」的起点，注入的这条不属于本轮。
    if let Some(cb) = ctx_block.as_ref() {
        history.retain(|m| !is_context_block_msg(m));
        history.insert(0, cb.clone());
    }

    // 回滚锚点：本轮压入的所有消息（user / assistant / tool_result）都在其后。
    // 压缩会丢掉历史开头的消息，锚点需同步左移（见 compact_history 的返回值）。
    let mut base = history.len();
    // 文本在前、图片按原顺序在后（端点侧图文顺序有语义；且 history 第一条**必须**是
    // 文本块 —— 前缀缓存与前端的标题推导都吃这条不变量，见 §11 规则 18 ①）。
    let mut user_content: Vec<Value> = vec![json!({ "type": "text", "text": prompt })];
    // 2026-11-15：本轮契约锚点（用户最新要。+ 「它优先于旧计划」）。空消息不加。
    if let Some(anchor) = goal_anchor(prompt) {
        user_content.push(json!({ "type": "text", "text": anchor }));
    }
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

    // ── 本次提问的成本预算（2026-10-01；2026-11-02 改加权计点）──────────
    // `turn_tokens` 累计该提问内所有请求的**加权计点**（口径见
    // `DEFAULT_TURN_BUDGET_TOKENS` 与 `budget_points` 的注释）。撞到后先「续命」再
    // 清零计点；`budget_extensions` 数的是续过几次（真上限在它，不在预算值）。
    let turn_budget = turn_budget_tokens();
    let mut turn_tokens: u64 = 0;
    let mut budget_extensions = 0usize;
    // 预算触发的强制压缩：本问已用几次 / 上一次发生在第几轮（见 `should_budget_compact`）。
    let mut budget_compacts = 0usize;
    let mut last_budget_compact_round = 0usize;

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
    // ── 工具连续失败计数。026-11-15）─────────────────────────────
    // `(工具。 连续失败轮数)`；只统计「单工具轮且结果。is_error」。判据与处置。
    // `TOOL_FAIL_HINT_ROUNDS` 那一段。`fail_hint_sent` 同样保证提示只发一次。
    let mut fail_streak: Option<(String, usize)> = None;
    let mut fail_hint_sent = false;
    // ── 提示缓存断点。026-11-15）──────────────────────────────────
    // 端点若明确拒。`cache_control`。00），这里会被。false 并在本次提问内保。—。
    // 避免每轮都拿同一。400 再试一次。新提问重新按开关取值。
    let mut cache_markers = prompt_cache_enabled();
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
    // 直接与上一问的末请求比 —— 「两次提问之间本侧改没改 history」才能被判出来，
    // （`message_stop` 时取最后一条随用量落盘）。用 Vec 而不是标量：一来避开
    // 「赋初值后必被覆盖」的 unused_assignments 警告，二来重试路径（send 前
    // `continue` 回来会再算一次）天然留下多条，正好能看出是哪次尝试断的。
    let mut common_log: Vec<usize> = Vec::new();
    let mut final_text = String::new();
    let mut turns = 0usize;
    let mut rounds = 0usize;
    let mut hint_sent = false;
    // 中途三态控制（§待办 3-28.1）：本轮是否被用户暂停 / 插入了多少条。
    let mut paused_now = false;
    let mut steer_count = 0usize;
    // 上一轮在审批上被暂停留下的标记不能带进这一轮。
    PAUSED_AT_APPROVAL.store(false, Ordering::SeqCst);

    // 调试转储开关（LUNAC_DUMP_API，默认关）：关时本问全程零开销 —— 下面的 req_id 拼接
    // 与报文序列化都被它短路（见 log::dump_api）。
    let dump_on = log::dump_api_enabled();

    // ── 任务级预算（S2 第一刀，2026-10-22）────────────────────────
    // 本问**开始前**是否已经撞顶：区分「本问内刚撞」（要收口）与「早先就撞了」
    // 本问**开始前**是否已经撞顶：区分「本问内刚撞」（要收口）与「早先就撞了」
    let task_over_at_start = task_budget.over();
    let mut task_over_hint_sent = false;
    if task_over_at_start {
        // 撞顶也可能发生在**上一问的最后一个请求**上（那时循环已结束、收口闸没机会跑）。
        // 所以「一次性上报」在这里补一次：否则用户永远看不到任何信号。
        if !task_budget.warned {
            task_budget.warned = true;
            emit(json!({
                "type": "system",
                "subtype": "budget_exhausted",
                "extensions": 0,
            }));
            log::info(format!(
                "任务级预算撞顶（{} tokens），本进程不再续命预算",
                task_budget.describe()
            ));
        }
        log::warn(format!(
            "任务级预算已用尽（{} tokens），本回合禁用预算续命（仍可作答）",
            task_budget.describe()
        ));
    }

    loop {
        // ── 轮首安全点：中途插入 / 暂停（§待办 3-28.1）──────────────
        // 位置不变量与压缩同一处：上一批工。*全部回灌。*、下一次请。*还没。*。
        // 此处 history 是静止的 ⇒ 注入一条 user 消息不会拆坏 `tool_use` / `tool_result` 配对；
        // 反过来在工具批中途插消息会被端点直接判 400。
        let steer = drain_steering(steer_rx, steer_backlog);
        apply_steering(steer.inserts, history, question_seq, &mut steer_count);
        if steer.paused {
            paused_now = true;
        }
        if paused_now {
            // 暂停 = 在安全点收口，**不回溯**已完成的工作、不丢 history；强制落一份检查点，
            // 进程退出后 `--resume` 仍能接着跑（不依赖宿主是否传了 --checkpoint）。
            log::info(format!("pause=q{question_seq} at=round_head（轮首安全点收口）"));
            set_turn_phase(&mut turn_phase, TurnPhase::Interrupted, question_seq);
            save_checkpoint(true, question_seq, task_budget.used, approval, history);
            emit(json!({ "type": "system", "subtype": "paused" }));
            break;
        }
        turns += 1;
        // 本次请求的转储关联 id：q<第几次提问>-r<本问第几个请求>。r 的序号与
        // result.usage.requests[] 同序（本问第 K 个请求 = requests[K-1]，同为本问第 K 条
        // usage_delta）；同一次请求的重试再加 -a<n> 区分（a1 = 首次尝试）。
        let dump_seq = req_log.len() + 1;
        let mut dump_attempt: u32 = 0;

        // ── 端点侧缓存还热不热（2026-10-01）──────────────────────
        // 距上次**发出请求**已超过 TTL ⇒ 端点侧那份前缀缓存本来就过期了 ⇒ 此时
        // 动历。*零额外代。*，可以让「每轮微清理」上场、并免掉滞回。
        // 动历史**零额外代价**，可以让「每轮微清理」上场、并免掉滞回。
        // `None`（本次提问还没发过请求）按「热」处理 —— 没有依据就别激进。
        let cache_cold = last_req_at
            .map(|t| t.elapsed().as_secs() >= cache_ttl_secs)
            .unwrap_or(false);

        // ── 上下文水位检查（发请求之前）─────────────────────────
        // 用上一轮实测的输入体积作基准（比按字符估算准），过了 ELIDE 水位
        // 先瘦身、过了 DROP 水位才允许丢弃。
        //
        // ── 上下文压缩（**轮首**，2026-10-27 明文化 §待办 3-16 P1-6）──────────
        //
        // **时机不变量**：这段只在「上一批工具已经跑完、下一次请求还没发」的位置执行
        // （`run_query` 的轮循环顶部）。三个理由，任何一个都要求它留在这里：
        //   · 此刻 history 是**静止**的 —— 批次中途压会把同一轮里还没回灌的
        //     `tool_use` / `tool_result` 配对拆坏（端点硬校验）；
        //   · 压缩必然改写前缀，紧跟着就发请求 ⇒ 只废一次；要是压在两次工具调用之间，
        //     那一轮剩下的调用就全都落在「改过的前缀」上，白废多份缓存；
        //   · `cfg.last_input` 只有在上一次请求返回后才更新，轮首读到的才是「刚测过的水位」。
        // ⚠️ **别把它挪进工具批里**：C 组实测紧邻压缩的请求出现 miss 尖峰（9,858 / 8,497）。
        //
        // **顺序不变量**：`compact_history` 内部是「① 先瘦可重取的 tool_result →
        // ② 只在瘦不下来时才丢整条消息」（`can_drop = (elided == 0)`，见那里的注释）。
        // 反过来先丢消息会把还能用的证据一起丢掉，且一条都省不回来。
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
                // 本地只读预筛（S1）：压缩事件点上**只记账，绝不改压缩行为**。
                prescreen_compact("drop", &format!("measured={measured}"), tctx.memory_scope, question_seq);
                // 记忆候选抽取（S1 第二切片）：压缩事件同样触发一次；**只落账本**、不改压缩行为。
                extract_candidates(history, tctx.memory_scope, question_seq);
                // append-only 全量（P6 全量形态，**默认关**）：同一水位触发，但先只瘦不丢；
                // 折算后仍可能撞预算才回落 Drop（回落发生在 `compact_history` 内部 ⇒ 事件只发一次）。
                let mode = if compact_append_only_enabled() {
                    Compact::AppendOnly
                } else {
                    Compact::Drop
                };
                let out = compact_history(history, mode, measured);
                base = base.saturating_sub(out.dropped) + out.pinned;
                // 摘要压缩（backlog §8.2）+ 结论索引写入 + 检索重注入：丢弃档是「结论被丢」的
                // 主战场，三者都在 `restore_after_compaction` 里做（单一入口）。
                // 返回值 = 插回历史的合成消息条数（摘要 / 结论块）⇒ 回滚锚点要加回来。
                base += restore_after_compaction(
                    cfg,
                    history,
                    &out.dropped_msgs,
                    &out.elided_msgs,
                    tctx.memory_scope,
                    question_seq,
                    &conclusions::default_dir(),
                );
                cfg.last_input.set(compaction_estimate(measured, out.freed_chars));
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
                // 瘦身档不丢整条 ⇒ 无摘要、无重注入（返回 0）；只把「查过哪里」的指针写进结论索引。
                base += restore_after_compaction(
                    cfg,
                    history,
                    &out.dropped_msgs,
                    &out.elided_msgs,
                    tctx.memory_scope,
                    question_seq,
                    &conclusions::default_dir(),
                );
                cfg.last_input.set(compaction_estimate(measured, out.freed_chars));
                compacted_this_round = out.elided + out.dropped > 0;
                if out.elided > 0 {
                    cfg.last_compact.set(measured);
                }
            } else if ratio > ELIDE_RATIO && grew_enough {
                let out = compact_history(history, Compact::Elide, measured);
                base = base.saturating_sub(out.dropped) + out.pinned;
                // 同冷档：瘦身不丢整条 ⇒ 无摘要、无重注入（返回 0）；只写「查过哪里」的指针条目。
                base += restore_after_compaction(
                    cfg,
                    history,
                    &out.dropped_msgs,
                    &out.elided_msgs,
                    tctx.memory_scope,
                    question_seq,
                    &conclusions::default_dir(),
                );
                cfg.last_input.set(compaction_estimate(measured, out.freed_chars));
                // 闸门判定「不值得」时什么都没改 —— 那时**不推进滞回时钟**，
                // 否则会白等一个 15% 增长窗口才重新评估（压缩次数统计也不会被污染）。
                if out.elided > 0 {
                    cfg.last_compact.set(measured);
                }
                compacted_this_round = out.elided + out.dropped > 0;
            }
        }

        // ── 压缩边界刷新长期记忆快照（§待办 3-9 C，2026-10-23）──────────────
        // 只在**本轮真压动过**时重读：没压动 ⇒ 前缀逐字节未变，重读只会白废一次缓存
        // （规则 56 仍禁止「每轮重读一次记忆文件」）。压动过 ⇒ 前缀从改动点起本来就作废，
        // 此刻重读不额外损失，且会话中途新写的记忆当轮即可见、不必重启 agent。
        if compacted_this_round {
            refresh_memory_snapshot(
                history,
                ctx_block,
                mcp_bridge.as_deref_mut(),
                remember_on,
                pet_growth_on,
                tctx.memory_scope,
            );
        }

        // ── 任务级预算（S2 第一刀，2026-10-22）──────────────────────
        // 只在「**本问内**刚撞顶」时收口：要么提示一次（拼进最后一条 tool_result，
        // 与 `TURN_BUDGET_HINT` 同一落点纪律），要么上次提示后模型仍在调工具 ⇒ 硬收。
        // 早先就撞顶（`task_over_at_start`）时这里不插手 —— 新提问仍要能作答，
        // 只是续命已在下面那条闸里被永久关闭。
        //
        // 2026-11-03 用户口径：**任务清单还没做完 ⇒ 预算不终止对话**。此时只保留
        // 「撞顶」这条日志/事件，不提示收口、不硬收；是否继续由下面 `budget_extension_allowed`
        // 按「上下文是否还安全」决定。
        let task_over = task_budget.over();
        let plan_unfinished = todo_has_unfinished(history);
        if task_over && !task_over_at_start && !plan_unfinished {
            if task_over_hint_sent {
                eprintln!("[agent] 任务级预算用尽且已给过收口机会，提前收尾");
                log::info(format!(
                    "任务级预算用尽且收口提示无效，硬收尾（累计 {} tokens）",
                    task_budget.describe()
                ));
                // 2026-11-02：同 `append_hard_stop` —— 不让对话静默停住。
                append_hard_stop(
                    &mut final_text,
                    &format!(
                        "本轮已停止：任务级成本预算用尽（{}）。已完成的改动都保留。",
                        task_budget.describe()
                    ),
                );
                emit(json!({
                    "type": "system",
                    "subtype": "budget_hard_stop",
                    "scope": "task",
                    "tokens": task_budget.used,
                }));
                // 断点说明：否则用户说「继续」时模型会把上一段重播一遍（见 `CUT_OFF_NOTE`）。
                append_hint_to_last_tool_result(history, CUT_OFF_NOTE);
                set_turn_phase(&mut turn_phase, TurnPhase::Done, question_seq);
                break;
            }
            task_over_hint_sent = true;
            if !append_hint_to_last_tool_result(history, TASK_BUDGET_HINT) {
                log::warn("任务级预算用尽但历史末尾不是工具消息，收口提示未能挂载");
            }
            // 与回合预算共用「本问已提示」标志：一句话只提示一次，不叠加两种收口。
            hint_sent = true;
            if !task_budget.warned {
                task_budget.warned = true;
                emit(json!({
                    "type": "system",
                    "subtype": "budget_exhausted",
                    "extensions": budget_extensions,
                }));
                log::info(format!(
                    "任务级预算撞顶（{} tokens），本进程不再续命预算",
                    task_budget.describe()
                ));
            }
        }

        // ── 单次提问的成本预算（2026-10-01；2026-11-02 改加权计点 + 免压缩续命）──
        // 放在水位检查**之后**：先把「可能撞 400」的隐患处理掉，再谈省钱。
        // 首次进入时 `turn_tokens == 0`（它在请求完成后才累加）⇒ 天然不触发。
        if turn_budget > 0 && turn_tokens >= turn_budget {
            // 预算撞顶先决定「这次要不要压缩」（2026-11-02）：
            //   · 上下文还不到 60% ⇒ 压缩只会白丢消息（实测 58 次全落在这个区间）；
            //   · 本问已压过一次 / 距上次不足 `BUDGET_COMPACT_MIN_ROUNDS` 轮 ⇒ 不再压；
            //   · 本轮轮首已经压过 ⇒ 直接算「压动过」。
            // 不压缩时续命依然成立（`roomy`）—— 这正是把「续命」与「压缩」解耦的地方。
            let measured_now = cfg.last_input.get();
            let roomy = measured_now > 0
                && (measured_now as f64) < (budget as f64) * BUDGET_EXTEND_ROOMY_RATIO;
            let rounds_since_last = rounds.saturating_sub(last_budget_compact_round);
            let compressed = if compacted_this_round {
                true
            } else if should_budget_compact(false, roomy, budget_compacts, rounds_since_last) {
                fire_plain_hook("PreCompact", &tctx.cwd, json!({ "trigger": "budget" }));
                prescreen_compact("budget", &format!("measured={measured_now}"), tctx.memory_scope, question_seq);
                // 记忆候选抽取（S1 第二切片）：压缩事件同样触发一次；**只落账本**、不改压缩行为。
                extract_candidates(history, tctx.memory_scope, question_seq);
                log::info(format!(
                    "成本预算撞顶且上下文占 {:.0}%（measured={measured_now}），执行强制压缩",
                    measured_now as f64 / budget as f64 * 100.0
                ));
                let out = compact_history(history, Compact::Force, measured_now);
                base = base.saturating_sub(out.dropped) + out.pinned;
                base += restore_after_compaction(
                    cfg,
                    history,
                    &out.dropped_msgs,
                    &out.elided_msgs,
                    tctx.memory_scope,
                    question_seq,
                    &conclusions::default_dir(),
                );
                cfg.last_input.set(compaction_estimate(measured_now, out.freed_chars));
                cfg.last_compact.set(measured_now);
                budget_compacts += 1;
                last_budget_compact_round = rounds;
                // 同上：这一档也是压缩边界（§待办 3-9 C）。`compacted_this_round` 只在
                // 上面那段里被更新，这里是它之外的第二个 Force 调用点，得自己刷。
                if out.elided + out.dropped > 0 {
                    refresh_memory_snapshot(
                        history,
                        ctx_block,
                        mcp_bridge.as_deref_mut(),
                        remember_on,
                        pet_growth_on,
                        tctx.memory_scope,
                    );
                }
                out.elided + out.dropped > 0
            } else {
                false
            };
            // 「计划未完成 ⇒ 预算只当**节流阀**，不当**终止符**」（2026-10-09 用户口径
            // 「预算撞顶不中止」，见 backlog §4.1.3）。判据 = 最近的 TodoWrite 清单里还有
            // `pending` / `in_progress` 的项。**检查点就是压缩本身** —— `Compact::Force`
            // 会把最近一条清单钉回历史（`latest_todo_snapshot`），所以续跑不丢任务方向。
            // 对**没有计划**的提问保持原上限（2 次），避免把普通问答也变成无底洞。
            let unfinished = todo_has_unfinished(history);
            let cap = MAX_BUDGET_EXTENSIONS;
            if budget_extension_allowed(
                compressed,
                roomy,
                budget_extensions,
                cap,
                task_over,
                unfinished,
            ) {
                budget_extensions += 1;
                turn_tokens = 0;
                // 新的一段预。又要重新给一次收口机会（旧实现不重置，导。
                // 新的一段预算 ⇒ 又要重新给一次收口机会（旧实现不重置，导致
                hint_sent = false;
                emit(json!({
                    "type": "system",
                    "subtype": "budget_extended",
                    "times": budget_extensions,
                    "max": cap,
                    "task_unfinished": unfinished,
                }));
                log::info(format!(
                    "成本预算撞顶（{turn_budget} 计点；上下文 {:.0}%，{}）⇒ {}续命第 \
                     {budget_extensions} 次，计点清零继续",
                    measured_now as f64 / budget as f64 * 100.0,
                    if compressed { "已压缩" } else { "跳过压缩" },
                    if unfinished {
                        "计划未完成，不设续命上限"
                    } else {
                        ""
                    }
                ));
            } else if hint_sent {
                // 已经给过收口机会、这次也确实续不动了 ⇒ 硬收（与轮次档同一形态）
                eprintln!("[agent] 提问成本预算用尽且已给过收口机会，提前收尾");
                log::info(format!(
                    "成本预算用尽且收口提示无效，硬收尾（累计 {turn_tokens} 计点）"
                ));
                // 2026-11-02：硬收尾不能静默 —— 见 `append_hard_stop` 的头注释。
                append_hard_stop(
                    &mut final_text,
                    &format!(
                        "本轮已停止：提问成本预算用尽（累计 {turn_tokens} 计点，续命已用满 \
                         {budget_extensions} 次）。已完成的改动都保留。"
                    ),
                );
                emit(json!({
                    "type": "system",
                    "subtype": "budget_hard_stop",
                    "scope": "turn",
                    "tokens": turn_tokens,
                    "extensions": budget_extensions,
                }));
                // 断点说明：否则用户说「继续」时模型会把上一段重播一遍（见 `CUT_OFF_NOTE`）。
                append_hint_to_last_tool_result(history, CUT_OFF_NOTE);
                set_turn_phase(&mut turn_phase, TurnPhase::Done, question_seq);
                break;
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
                    if task_over {
                        "任务级预算已撞顶，本进程不再续命".to_string()
                    } else if !unfinished && budget_extensions >= cap {
                        format!("续命已用满 {budget_extensions}/{cap}")
                    } else {
                        // 还有额度却续不动：上下文不宽裕、本轮又压不。体积在对话本身。
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
        // 本次请求（含重试尝试）的转储关联 id：循环内赋值，循环外（流式响应转储）要读。
        let mut dump_rid = String::new();

        // 发送（思考形态可降级重试 / 瞬时失败退避重试）：只有「与 thinking 相关的
        // 400」才沿降级链前进一次，并把可用形态写回 cfg 缓存，后续轮次不再试错。
        let resp = loop {
            // 按轮覆盖优先（2026-10-09）：桌宠轮 = `Disabled`，与全局开关无关。
            let plan = thinking_override.unwrap_or_else(|| cfg.thinking.get());
            let mut body = json!({
                "model": cfg.model,
                "max_tokens": max_tokens_for(plan),
                "stream": true,
                "system": system_value(system_prompt, cache_markers),
                "messages": history,
            });
            if !tool_defs.is_empty() {
                body["tools"] = tools_value(&tool_defs, cache_markers);
            }
            if let Some(t) = plan.to_json() {
                body["thinking"] = t;
            }

            // 调试转储：本次发送的请求体（已脱敏；请求头一个都不记，见 log::dump_api）。
            // req_id 供与 usage_delta / result.usage.requests[] 对账（同序）。
            if dump_on {
                dump_attempt += 1;
                dump_rid = if dump_attempt == 1 {
                    format!("q{question_seq}-r{dump_seq}")
                } else {
                    format!("q{question_seq}-r{dump_seq}-a{dump_attempt}")
                };
                log::dump_api("request", &dump_rid, &cfg.model, None, None, &body);
            }

            // ── 请求前缀指纹（2026-09-17，ai-spec §11 规则 23 的归因埋点）──────
            // 要回答的问题：同会话内跨提问时，上一次请求明明命中到 `read=N`，下一次的
            // `read` 却回落到 2048 / 2304（实测 `usage-2026-09-16.jsonl`：记录 6 末次
            // `read=3712` → 记录 7 首次 `read=2304`，丢 1408；记录 8→9 丢 3584）。
            // 静态读用量日志**无法判定**这是「本侧前缀被改写」还是「端点侧淘汰了已落盘
            // 的缓存单元」—— 两者的 read 曲线一模一样。
            // 所以每次请求前把三块前缀的**指纹 + 体积**落一行（只记哈希不记原文，
            // 前缀里可能含用户文件内容，哈希天然脱敏）：
            // 的缓存单元」—— 两者的 read 曲线一模一样。
            // 的缓存单元」—— 两者的 read 曲线一模一样。
            //     （system = 身份/环境块/技能清单，tools = 工具 schema，history = 历史）。
            // 放在 send 之前：重试路径 `continue` 回来会再记一行，正好能看出「同一次
            // 前缀里可能含用户文件内容，哈希天然脱敏）：
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
                // ── 前缀稳定性门禁（2026-10-27，§待办 3-16 P1-4）──────────────
                // 上面那行只是**记账**，人得主动去读日志；这里在跌破 90% 时主动告警。
                // 压缩是**正当**的下跌（`上下文压缩` 那几档），故只记 info；没压缩却塌了
                // 才是真的问题 —— 去查是哪一处就地改写了 history。
                if let Some(pct) = prefix_stability_break(cur_common, prev_len) {
                    let compacted = compacted_this_round || compacted_for_retry;
                    let detail = format!(
                        "prefix={pct}%（{cur_common}/{prev_len} 条，问#{question_seq} #{}）",
                        turns
                    );
                    if compacted {
                        log::info(format!("前缀稳定性：{detail} —— 本轮刚压缩，预期内"));
                    } else {
                        log::warn(format!(
                            "前缀稳定性：{detail} —— 未压缩却跌破 {PREFIX_STABILITY_MIN_PERCENT}%，\
                             本侧就地改写了 history（其后整段缓存作废）"
                        ));
                    }
                }
            }

            let sent = model_post(cfg)
                .header("authorization", format!("Bearer {}", cfg.token))
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .json(&body)
                .send();

            let r = match sent {
                Ok(r) => r,
                Err(e) => {
                    if dump_on {
                        log::dump_api(
                            "response",
                            &dump_rid,
                            &cfg.model,
                            None,
                            Some(&e.to_string()),
                            &Value::Null,
                        );
                    }
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
            if dump_on {
                log::dump_api(
                    "response",
                    &dump_rid,
                    &cfg.model,
                    Some(status.as_u16()),
                    None,
                    &Value::String(detail.clone()),
                );
            }
            if status == 400 && thinking_related_error(&detail) {
                if let Some(next) = plan.next() {
                    eprintln!("[agent] 端点不接受 thinking={plan:?} → 降级为 {next:?}");
                    // 按轮覆盖。*只在本轮。*前进（不污染全局缓存 —。否则桌宠轮的降级。
                    // 把普通聊天已跑通的形态改掉）；全局档照旧写回缓存，后续提问不再试错。
                    match thinking_override.as_mut() {
                        Some(ov) => *ov = next,
                        None => cfg.thinking.set(next),
                    }
                    continue;
                }
            }
            // 上下文超。强制压缩后再试一次（水位估算失准时靠这条兜底。
            if status == 400 && !compacted_for_retry && context_related_error(&detail) {
                compacted_for_retry = true;
                fire_plain_hook("PreCompact", &tctx.cwd, json!({ "trigger": "force" }));
                prescreen_compact("force", &format!("measured={}", cfg.last_input.get()), tctx.memory_scope, question_seq);
                // 记忆候选抽取（S1 第二切片）：压缩事件同样触发一次；**只落账本**、不改压缩行为。
                extract_candidates(history, tctx.memory_scope, question_seq);
                let out = compact_history(history, Compact::Force, cfg.last_input.get());
                base = base.saturating_sub(out.dropped) + out.pinned;
                // 兜底档也做「摘要 + 结论索引 + 重注入」—— 这一档丢弃量最大（连尾部都瘦），
                // 兜底档也做「摘要 + 结论索引 + 重注入」—— 这一档丢弃量最大（连尾部都瘦），
                base += restore_after_compaction(
                    cfg,
                    history,
                    &out.dropped_msgs,
                    &out.elided_msgs,
                    tctx.memory_scope,
                    question_seq,
                    &conclusions::default_dir(),
                );
                let measured_force = cfg.last_input.get();
                cfg.last_compact.set(measured_force);
                cfg.last_input
                    .set(compaction_estimate(measured_force, out.freed_chars));
                // 压缩边界刷新记忆快照（§待办 3-9 C）：与上面两档同一条口径 —— 只有真压动才重读。
                if out.elided + out.dropped > 0 {
                    refresh_memory_snapshot(
                        history,
                        ctx_block,
                        mcp_bridge.as_deref_mut(),
                        remember_on,
                        pet_growth_on,
                        tctx.memory_scope,
                    );
                }
                eprintln!("[agent] 端点回报上下文超限 → 压缩 {} 条后重试", out.dropped);
                continue;
            }
            // 缓存断点被端点拒。摘掉断点重试一次（2026-11-15）。
            // 顺序放在 thinking / 上下文两条专门分支之后：那两条各有自己的降级语义。
            // 不能被这里吃掉。只降级一次：第二次仍。400 就按真实错误返回。
            if status == 400
                && cache_markers
                && !thinking_related_error(&detail)
                && !context_related_error(&detail)
            {
                cache_markers = false;
                let brief: String = detail.trim().chars().take(200).collect();
                log::warn(format!(
                "端点不接受 cache_control，本次提问改为不带缓存断点重试：{brief}"
                ));
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
        // 成功响应（2xx）的状态码，供转储用；下面 BufReader::new(resp) 会消费 resp。
        let resp_status = resp.status().as_u16();

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
                    // 成本预算计点（2026-11-02）：`in + create + out` 全额，`read` 按
                    // `CACHE_READ_BUDGET_WEIGHT_PCT` 折算 —— 见 `budget_points`。
                    let points = budget_points(req_in, req_read, req_create);
                    turn_tokens += points;
                    // 任务级预算（2026-10-22）：同一笔用量同时累进**跨提问**的计数器。
                    task_budget.record(points);
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
                            // 任务级预算：输出同样计入跨提问的累计（口径与上一行逐字一致）。
                            task_budget.record(n);
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
                            // 只补「新增的计点」：用同一套加权口径对新旧值求差。
                            let added = budget_points(m.0, m.1, m.2)
                                .saturating_sub(budget_points(cur_in, cur_read, cur_create));
                            turn_tokens += added;
                            // 任务级预算：中转端点补报的输入量同样计入（与上一行同口径）。
                            task_budget.record(added);
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

        // 调试转储：流式响应记「累积后的最终报文」（content 块 + usage），不记原始 SSE 行。
        // 为什么不记原始行：一轮回复的 text/thinking 增量可达数千行，tool_use 的
        // input_json_delta 更是逐字符事件，原始行会把转储撑大几十倍且几乎不可读；最终报文
        // 既能看清端点回了什么，又天然被体积上限兜住。需要逐字时序时另有 stream.jsonl。
        if dump_on {
            let content: Vec<Value> = blocks
                .iter()
                .filter_map(|b| match b.kind.as_str() {
                    "text" if !b.text.is_empty() => Some(json!({ "type": "text", "text": b.text })),
                    "thinking" if !b.text.is_empty() => {
                        Some(json!({ "type": "thinking", "thinking": b.text }))
                    }
                    "tool_use" => Some(json!({
                        "type": "tool_use", "id": b.id, "name": b.name, "input_json": b.json,
                    })),
                    _ => None,
                })
                .collect();
            let resp_body = json!({
                "content": content,
                "usage": { "in": cur_in, "read": cur_read, "create": cur_create, "out": cur_out },
            });
            log::dump_api("response", &dump_rid, &cfg.model, Some(resp_status), None, &resp_body);
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

        // 没有工具调用 ⇒ 本轮本应结束。但若用户在这段**最终回答流式期间**插入了消息，
        // 就在这个安全点注入并再跑一轮（同一个回合、同一份 result）：否则消息会掉到
        // 主循环里，变成一次「前端没有回合状态」的自动提问（§待办 3-28.1）。
        if calls.is_empty() {
            let steer = drain_steering(steer_rx, steer_backlog);
            let injected = apply_steering(steer.inserts, history, question_seq, &mut steer_count);
            if steer.paused {
                paused_now = true;
            }
            if paused_now {
                log::info(format!("pause=q{question_seq} at=round_end（轮末安全点收口）"));
                set_turn_phase(&mut turn_phase, TurnPhase::Interrupted, question_seq);
                save_checkpoint(true, question_seq, task_budget.used, approval, history);
                emit(json!({ "type": "system", "subtype": "paused" }));
                break;
            }
            if injected > 0 {
                continue;
            }
            set_turn_phase(&mut turn_phase, TurnPhase::Done, question_seq);
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
            // 「先解释」闸：必须在 **hook 之后、审批卡之前** 判 —— 否则前端会先弹出
            // 一张写类工具卡，用户点了允许也执行不了（与只读档同一个优先级）。
            // 相位不是 `Executing` 就一律按只读轮处理（`AwaitingUser` 也一样：
            // 用户还没答应继续，模型不能换个工具再试）。
            let mut explain_denial = if turn_phase.allows_writes() {
                None
            } else {
                explain_first_denial(true, name, input, skill_list)
            };
            match gate {
                HookGate::Deny(r) => denied = Some(format!("Denied by PreToolUse hook: {r}")),
                _ if explain_denial.is_some() => {
                    log::info(format!(
                        "explain-first 闸：拒绝 {name}（用户要求先解释，再决定是否继续）"
                    ));
                    set_turn_phase(&mut turn_phase, TurnPhase::AwaitingUser, question_seq);
                    denied = explain_denial.take();
                }
                _ => {
                    // hook 放行 ⇒ 只剩「安全分析命中」这一种情况还能把卡叫回来
                    let allowed = matches!(gate, HookGate::Allow)
                        && !hook_allow_needs_card(name, input);
                    let needs_card = needs_approval_with(name, input, skill_list)
                        && (!tctx.read_only || tools::gated_in_read_only(name));
                    if !allowed && needs_card {
                        match approval_policy() {
                            // 单发：没有回包通道 ⇒ 需审批的工具直接拒（绝不静默挂起）。
                            // 例外 = 用户 `--unattended-allow` 点名的写类工具（阶段 2B），
                            // 且凭据扫描必须干净（`hook_allow_needs_card` 为假）。
                            ApprovalPolicy::UnattendedDeny
                                if unattended_allows(name)
                                    && !hook_allow_needs_card(name, input) => {}
                            ApprovalPolicy::UnattendedDeny => {
                                denied = Some(unattended_deny_reason(name))
                            }
                            // 单发 + --dangerously-skip-permissions：用户显式承担 ⇒ 放行、不弹卡
                            ApprovalPolicy::UnattendedAllow => {}
                            // 常驻：现状（需 stdio 审批开关）
                            ApprovalPolicy::Stdio if ask_permission => {
                                // PermissionRequest（A9）：只在「本来就要弹卡」的这一刻问 ——
                                // 它能替用户答这一问（allow = 免卡，deny = 直接拒）
                                match hook_tool_gate("PermissionRequest", &tctx.cwd, name, id, input)
                                {
                                    HookGate::Deny(r) => {
                                        denied =
                                            Some(format!("Denied by PermissionRequest hook: {r}"))
                                    }
                                    HookGate::Allow if !hook_allow_needs_card(name, input) => {}
                                    // 第二模型预审（§待。3-28.14）：只可能「拒」或「交给人」。
                                    // 。与用户点拒绝同一条路（理由进工具结果 / 对话流）。
                                    // 交给。照旧弹卡。*没有 allow 这一。*。
                                    _ => match second_review::gate(name, input) {
                                        Some(reason) => denied = Some(reason),
                                        None => {
                                            pending = Some(open_approval(name, id, input, None))
                                        }
                                    },
                                }
                            }
                            ApprovalPolicy::Stdio => {}
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
        // 共用这份结论 —— 此前判定散在批循环里，A14 再加一处并发分支就会出现
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

        // ── 子目录记忆懒加载（2026-10-12，§待办 3-1a 子目录级）──────────────
        // 对本轮**真正触碰过**的合格子目录，把其 AGENTS.md 拼进**对应 tool_result 的文本内部**
        // （不新增消息 / 块 ⇒ 固定前缀一字不动；判据链与落点纪律见 `inject_subdir_memory`）。
        inject_subdir_memory(
            &tctx.cwd,
            history.as_slice(),
            &calls,
            &run_inputs,
            &denieds,
            &mut results,
        );

        // ── 后台结果回流（2026-10-01，用户选的形态：「攒着，下次工具调用时带上」）──
        // 已跑完的后台命令，输出在这里**随 tool_result 一起**交给模型。
        // **不单独 push 一条消息**：那会改变 `history` 末尾的消息形态，把端点侧前缀缓存
        // 整段打掉（见 `TOOL_BUDGET_HINT` 上方那张表）。拼进**已有**的 tool_result 内部
        // 则只有它自己那条。—。而它本来就是这一轮新产生、还没被缓存过的内容。
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

        // ── 工具连续失败熔断。026-11-15）────────────────────────────
        // 与上面的卡住检。*互补**：这里只。`is_error`，不看结果文本（错误文本每轮都不。
        // 。`is_stuck` 抓不到）。判据与处置。`TOOL_FAIL_HINT_ROUNDS` 那一段。
        let round_all_failed = !results.is_empty()
            && results.iter().all(|block| {
                block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            });
        if calls.len() == 1 && round_all_failed {
            fail_streak = next_fail_streak(fail_streak, Some(calls[0].1.as_str()));
        } else {
            fail_streak = next_fail_streak(fail_streak, None);
        }
        let fail_rounds = fail_streak.as_ref().map(|(_, count)| *count).unwrap_or(0);
        if fail_rounds >= TOOL_FAIL_HINT_ROUNDS && !fail_hint_sent {
            fail_hint_sent = true;
            if append_hint_to_last_tool_result(history, TOOL_FAIL_HINT) {
                emit(json!({
                    "type": "system",
                    "subtype": "tool_fail_circuit",
                    "tool": calls[0].1,
                    "rounds": fail_rounds,
                }));
                log::info(format!(
                    "工具失败熔断：`{}` 已连续 {fail_rounds} 轮失败，已提示模型换方法或如实说明",
                    calls[0].1
                ));
            }
        }
        if fail_rounds >= TOOL_FAIL_STOP_ROUNDS {
            let name = calls[0].1.clone();
            append_hard_stop(
                &mut final_text,
                &format!(
                    "本轮已停止：工具 `{name}` 连续 {fail_rounds} 轮失败。已完成的改动都保留；\
                     把上面的错误原文告诉用户，或让用户补齐前置条件后再试。",
                ),
            );
            emit(json!({
                "type": "system",
                "subtype": "tool_fail_hard_stop",
                "tool": name,
                "rounds": fail_rounds,
            }));
            append_hint_to_last_tool_result(history, CUT_OFF_NOTE);
            log::info(format!(
                "工具失败熔断：`{name}` 连续 {fail_rounds} 轮失败，硬收口",
            ));
        // 也不在上一份检查点里，模型会重新发起）。
            save_checkpoint(
                checkpointing,
                question_seq,
                task_budget.used,
                approval,
                history.as_slice(),
            );
            set_turn_phase(&mut turn_phase, TurnPhase::Done, question_seq);
            break;
        }

        // S2 检查点（2026-10-22，中间态）：这一批工具结果已经完整写进 history
        // ⇒ 立刻落一份。进程随后被杀 / 崩溃时，续跑能看到这批结果、**不会重跑**
        // 已完成的工具；最多重跑「尚未回灌结果」的那一批（那批的 assistant 消息
        // 也不在上一份检查点里，模型会重新发起）。
        save_checkpoint(checkpointing, question_seq, task_budget.used, approval, history.as_slice());

        // 用户点了「拒绝并中断」→ 结果已回灌，本轮到此为止
        if interrupted {
            eprintln!("[agent] 用户要求中断本轮");
            // 这次中断若是「审批卡弹出时点暂停」触发的，如实把本轮标记。paused。
            if PAUSED_AT_APPROVAL.swap(false, Ordering::SeqCst) {
                paused_now = true;
                emit(json!({ "type": "system", "subtype": "paused" }));
            }
            set_turn_phase(&mut turn_phase, TurnPhase::Interrupted, question_seq);
            break;
        }

        rounds += 1;
        // 默认 0 = 不设上限。026-11-14 用户口径：完成前不因工具额度暂停）。
        let max_rounds = max_tool_rounds();
        if max_rounds > 0 && rounds >= max_rounds {
            if hint_sent {
                eprintln!("[agent] 工具轮次达上限，提前收尾");
                // 2026-11-02：轮次硬收尾也要给用户一句话（同 `append_hard_stop`）。
                append_hard_stop(
                    &mut final_text,
                    &format!(
                        "本轮已停止：工具轮次达到兜底上限（{max_rounds} 轮）。已完成的改动都保留。",
                    ),
                );
                emit(json!({
                    "type": "system",
                    "subtype": "rounds_hard_stop",
                    "rounds": max_rounds,
                }));
                // 同预算硬收尾：把断点写进模型下一轮读得到的位置（见 `CUT_OFF_NOTE`）。
                append_hint_to_last_tool_result(history, CUT_OFF_NOTE);
                set_turn_phase(&mut turn_phase, TurnPhase::Done, question_seq);
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
                "rounds": max_rounds,
            }));
            log::info(format!(
                "工具轮次达兜底上限（{max_rounds} 轮），要求模型收口作答",
            ));
        }
    }

    // 子代理 / 后台复盘的用量并入本次提问（2026-09-29）。**不归并 = 本地账少一截**：
    // 平台照收它们的钱，而 `result.usage` 此前只有主循环那几轮（实测平台 24 次请求
    // vs 本地 17 次）。`take()` 顺带归零，保证「本次提问的绝对值」这个口径不被带到下问。
    // 顺序说明：它们在提问内并发完成，追加到 `requests[]` 末尾的先后由线程调度决定；
    // 平台对账按「条数 + 合计」对齐，不依赖顺序。
    //
    // 2026-10-27（§待办 3-16 P2-8）：**拆成两本账、两行日志** —— 混在一行时无法回答
    // 「钱花在探索（子代理）还是收口（复盘）上」。
    let (sub_in, sub_read, sub_create, sub_out, sub_reqs) = cfg.sub.take();
    let (rev_in, rev_read, rev_create, rev_out, rev_reqs) = cfg.review.take();
    let sub_requests = sub_reqs.len();
    let rev_requests = rev_reqs.len();
    if sub_in + sub_read + sub_create + sub_out > 0 {
        log::info(format!(
            "子代理用量并入本次提问 in={sub_in} read={sub_read} create={sub_create} \
             out={sub_out} 请求={sub_requests}"
        ));
    }
    if rev_in + rev_read + rev_create + rev_out > 0 {
        log::info(format!(
            "复盘用量并入本次提问 in={rev_in} read={rev_read} create={rev_create} \
             out={rev_out} 请求={rev_requests}"
        ));
    }
    in_tokens += sub_in + rev_in;
    cache_read += sub_read + rev_read;
    cache_create += sub_create + rev_create;
    out_tokens += sub_out + rev_out;
    req_log.extend(sub_reqs);
    req_log.extend(rev_reqs);

    emit(json!({
        "type": "result",
        // 暂停是**正常收口**（不是错误）：前端据 subtype 显示「已暂停」，而不是错误行。
        "subtype": if paused_now { "paused" } else { "success" },
        "is_error": false,
        "duration_ms": started.elapsed().as_millis() as u64,
        "num_turns": turns,
        "result": final_text,
        "session_id": session_id(),
        "total_cost": cost_json(in_tokens, cache_read, cache_create, out_tokens),
        "usage": {
            "input_tokens": in_tokens,
            "output_tokens": out_tokens,
            "cache_read_input_tokens": cache_read,
            "cache_creation_input_tokens": cache_create,
            // 归因（「这问的钱有多少是子代理烧的」）。消费者**不得**把它再加一次。
            // 归因（「这问的钱有多少是子代理烧的」）。消费者**不得**把它再加一次。
            // 2026-10-27 起拆成 `subagent` / `review` 两份（§待办 3-16 P2-8）。
            "subagent": {
                "input_tokens": sub_in,
                "output_tokens": sub_out,
                "cache_read_input_tokens": sub_read,
                "cache_creation_input_tokens": sub_create,
                "requests": sub_requests,
            },
            "review": {
                "input_tokens": rev_in,
                "output_tokens": rev_out,
                "cache_read_input_tokens": rev_read,
                "cache_creation_input_tokens": rev_create,
                "requests": rev_requests,
            },
            // 每次 API 请求一行（顺序 = 请求顺序）。前端写进本地用量日志，
            // 与 DeepSeek 平台用量页按请求对账；旧消费者忽略即可。
            "requests": req_log,
        },
    }));

    // ── Stop hook（A9）──
    // 只在**成功收尾**时发（出错走 finish_error，本轮是被放弃的，不该报「回答完成」）。
    // 暂停同样不算「回答完成」：用户主动按下的收口，脚本不该收到 Stop（§待办 3-28.1）。
    if !paused_now {
        fire_plain_hook("Stop", &tctx.cwd, json!({}));
    }
}

/// 预算 / 轮次硬收尾时补一句**用户可见的结论**。
///
/// 2026-11-02：日志里多次出现「成本预算用尽且收口提示无效，硬收尾」后对话直接停住 ——
/// 最后一轮如果只有工具调用、没有文本，`final_text` 是空串，`result.result` 也就是空，
/// 前端没有任何可显示的内容（用户看到的是「AI 无缘无故不说了」）。
/// 这里统一在硬收尾前把原因写进 `final_text`（不覆盖已有正文，只追加），
/// 同时由调用点 emit `system/<scope>_hard_stop` 让界面**当场**显示一条旁注。
fn append_hard_stop(final_text: &mut String, reason: &str) {
    if !final_text.trim().is_empty() {
        final_text.push_str("\n\n");
    }
    final_text.push_str(reason);
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
        // 费用字段**故意不给**：这轮是失败收尾，本函数拿不到累计用量（`in/read/out` 。
        // `run_query` 的局部变量里）⇒ 。0 等于谎报「没花钱」。宿主按 `usage_delta`
        // 逐请求落账，仍然能算出真实花费。
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── S1 首个切片：本地只读预筛的接线行为证据（2026-10-15）────────────────
    //
    // 用可注入的假 `Decider` **精确计数**（不许「看起来对」）。控制流与 `main()` 里那段同构：
    // 只有「每 K 轮」到点才判，`allow_review_spawn` 决定派不派。基线 = 「不接线」（每轮都派）。

    struct FixedSkip;
    impl decide::Decider for FixedSkip {
        fn decide(&self, _i: &str) -> Result<decide::Decision, String> {
            Ok(decide::Decision::skip("stub-skip"))
        }
    }
    struct FixedRun;
    impl decide::Decider for FixedRun {
        fn decide(&self, _i: &str) -> Result<decide::Decision, String> {
            Ok(decide::Decision::run("stub-run"))
        }
    }
    struct FixedErr;
    impl decide::Decider for FixedErr {
        fn decide(&self, _i: &str) -> Result<decide::Decision, String> {
            Err("endpoint down".into())
        }
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lunac-pre-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// 模型化「每 K 轮 gate 复盘 spawn」的控制流，返回 (派发次数, 账本条数)。
    fn simulate(d: &dyn decide::Decider, k: u64, n: u64, dir: &Path) -> (usize, usize) {
        let mut spawns = 0usize;
        for turn in 1..=n {
            let verdict = if prescreen_due(turn, k) {
                Some(run_prescreen(
                    d,
                    "test-model",
                    &format!("turn-{turn}"),
                    "prescreen-review",
                    tools::MemoryScope::Global,
                    turn,
                    dir,
                ))
            } else {
                None
            };
            if allow_review_spawn(verdict) {
                spawns += 1;
            }
        }
        (spawns, decisions::count(dir, tools::MemoryScope::Global))
    }

    /// ① 调用次数精确：N=23、K=5 ⇒ 账本**恰好 5 条**（轮 1/6/11/16/21）。
    #[test]
    fn prescreen_is_amortized_every_k_turns_exactly() {
        let dir = tmp_dir("k5");
        let (_, count) = simulate(&FixedRun, 5, 23, &dir);
        assert_eq!(count, 5, "N=23,K=5 必须恰好判 5 次");
        let due: Vec<u64> = (1..=23).filter(|t| prescreen_due(*t, 5)).collect();
        assert_eq!(due, vec![1, 6, 11, 16, 21]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ② SKIP 真的改变行为：固定回 SKIP（每轮都判，K=1）⇒ 派发**零次**；固定回 RUN ⇒ 与不接线一致。
    #[test]
    fn skip_suppresses_spawn_and_run_matches_baseline() {
        let d_skip = tmp_dir("skip");
        let (spawn_skip, n_skip) = simulate(&FixedSkip, 1, 23, &d_skip);
        assert_eq!(spawn_skip, 0, "SKIP ⇒ 后台复盘派发零次");
        assert_eq!(n_skip, 23, "K=1 ⇒ 每轮都记账");

        let d_run = tmp_dir("run");
        let (spawn_run, _) = simulate(&FixedRun, 1, 23, &d_run);
        assert_eq!(spawn_run, 23, "RUN ⇒ 派发次数与不接线（每轮都派）一致");
        let _ = std::fs::remove_dir_all(&d_skip);
        let _ = std::fs::remove_dir_all(&d_run);
    }

    /// ③ 失败 = 零差异：假 `Decider` 回 `Err` ⇒ 派发次数、history、固定前缀指纹与不接线**全等**。
    #[test]
    fn backend_error_is_identical_to_unwired() {
        let dir = tmp_dir("err");
        let history = vec![
            json!({"role":"user","content":[{"type":"text","text":"帮我把 parse_args 改对"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"好"}]}),
        ];
        let system = "SYS-FIXED-PREFIX";
        let tool_defs = vec![json!({"name":"Read"}), json!({"name":"Edit"})];
        let fingerprint = |h: &[Value]| {
            log::hash64(&format!(
                "{system}\u{1}{}",
                serde_json::to_string(&(h, &tool_defs)).unwrap()
            ))
        };
        let before = fingerprint(&history);
        let (spawn_err, _) = simulate(&FixedErr, 5, 23, &dir);
        assert_eq!(spawn_err, 23, "Err ⇒ fail-open ⇒ 与不接线（每轮都派）一致");
        assert_eq!(fingerprint(&history), before, "固定前缀指纹必须逐字节不变");
        assert_eq!(history.len(), 2, "history 不被判定改写");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ④ 账本脱敏：喂含 sk-... 的输入 ⇒ 账本文件不含明文（只留哈希）。
    #[test]
    fn ledger_never_holds_plaintext_secret() {
        let dir = tmp_dir("mask");
        run_prescreen(
            &FixedRun,
            "test-model",
            "用 sk-live-99887766 试一下",
            "prescreen-review",
            tools::MemoryScope::Global,
            1,
            &dir,
        );
        let blob =
            std::fs::read_to_string(decisions::path_for(&dir, tools::MemoryScope::Global)).unwrap();
        assert!(!blob.contains("99887766"), "凭据不得落进账本：{blob}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⑤ 写账本失败不影响主流程：目录不可写（指到一个文件）⇒ 只 warn、判定照常返回、不 panic。
    #[test]
    fn ledger_write_failure_does_not_break_prescreen() {
        let file = std::env::temp_dir().join(format!("lunac-pre-file-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let action = run_prescreen(
            &FixedSkip,
            "test-model",
            "hi",
            "prescreen-review",
            tools::MemoryScope::Global,
            1,
            &file,
        );
        assert_eq!(action, decide::Action::Skip, "记账失败也必须正常返回判定");
        let _ = std::fs::remove_file(&file);
    }

    // ── S1 第三切片：检索查询改写的接线行为证据（2026-10-24）──────────────────
    //
    // 两条最关键的不变量：① 扩展命中**只追加**、原命中一条不丢、顺序不变；
    // ② 账本只落 hash / 长度，绝不含查询与扩展词的原文。

    struct FixedTerms(&'static [&'static str]);
    impl query_rewrite::Rewriter for FixedTerms {
        fn rewrite(&self, _q: &str) -> Result<Vec<String>, String> {
            Ok(self.0.iter().map(|s| s.to_string()).collect())
        }
    }
    struct TermBoom;
    impl query_rewrite::Rewriter for TermBoom {
        fn rewrite(&self, _q: &str) -> Result<Vec<String>, String> {
            Err("endpoint down".into())
        }
    }

    fn hit(text: &str, matched: usize) -> memory_search::Hit {
        memory_search::Hit {
            text: text.to_string(),
            matched_terms: matched,
            query_terms: 3,
            occurrences: matched,
        }
    }

    /// 原命中不足 limit 时补充：原命中顺序逐条不变、总数补到 limit、同 scope 同文本不重复。
    #[test]
    fn merge_expansion_hits_appends_without_reordering_originals() {
        let mut orig = vec![
            (tools::MemoryScope::Global, hit("orig-1", 2)),
            (tools::MemoryScope::Global, hit("orig-2", 1)),
        ];
        let extra = vec![
            (tools::MemoryScope::Global, hit("extra-1", 1)),
            // 原命中里已有 ⇒ 丢弃扩展那一条（原分数更可信）。
            (tools::MemoryScope::Global, hit("orig-1", 3)),
            // 另一个 scope 的同文本是不同条目 ⇒ 保留。
            (tools::MemoryScope::Pet, hit("orig-1", 1)),
            (tools::MemoryScope::Global, hit("extra-2", 1)),
        ];
        merge_expansion_hits(&mut orig, extra, 5);
        assert_eq!(orig.len(), 5, "两条原命中 + 三条不重复的扩展");
        assert_eq!(orig[0].1.text, "orig-1", "原命中顺序不能动");
        assert_eq!(orig[1].1.text, "orig-2");
        assert_eq!(orig[2].0, tools::MemoryScope::Pet, "跨 scope 的同文本各自保留");
    }

    /// limit 封顶：扩展再多也不越过 limit，且原命中仍全在。
    #[test]
    fn merge_expansion_hits_respects_limit() {
        let mut orig = vec![(tools::MemoryScope::Global, hit("orig", 3))];
        let extra: Vec<_> = (0..8)
            .map(|i| (tools::MemoryScope::Global, hit(&format!("e{i}"), 1)))
            .collect();
        merge_expansion_hits(&mut orig, extra, 3);
        assert_eq!(orig.len(), 3);
        assert_eq!(orig[0].1.text, "orig", "原命中必须保留");
    }

    /// 账本：恰好一行、`kind=query-rewrite`、只有 hash 与长度，**绝无原文**。
    #[test]
    fn run_query_rewrite_writes_one_ledger_line_without_plaintext() {
        let dir = tmp_dir("rw");
        let out = run_query_rewrite(
            &FixedTerms(&["indentation", "space-prefs"]),
            "test-model",
            "四空格缩进",
            &dir,
        );
        assert_eq!(out.terms.len(), 2);
        assert!(!out.fallback);
        let recs = decisions::load(&dir, tools::MemoryScope::Global);
        assert_eq!(recs.len(), 1, "每次改写恰好一行账本");
        assert_eq!(recs[0].kind, "query-rewrite");
        assert_eq!(recs[0].verdict, "ok");
        assert_eq!(recs[0].candidate_count, Some(2));
        assert_eq!(recs[0].turn, 0, "MemorySearch 无提问号");
        let blob =
            std::fs::read_to_string(decisions::path_for(&dir, tools::MemoryScope::Global)).unwrap();
        assert!(!blob.contains("indentation"), "扩展词原文不得落账本：{blob}");
        assert!(!blob.contains("四空格缩进"), "查询原文不得落账本：{blob}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 后端故障 ⇒ fail-open（空扩展）且账本标 fallback；检索照原查询走。
    #[test]
    fn run_query_rewrite_failure_is_fail_open_and_logged() {
        let dir = tmp_dir("rw-boom");
        let out = run_query_rewrite(&TermBoom, "test-model", "四空格缩进", &dir);
        assert!(out.terms.is_empty());
        assert!(out.fallback);
        let recs = decisions::load(&dir, tools::MemoryScope::Global);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].verdict, "fallback");
        assert!(recs[0].fallback);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 写账本失败不影响返回：目录指到一个文件 ⇒ 只 warn、改写结果照常返回。
    #[test]
    fn ledger_write_failure_does_not_break_rewrite() {
        let file = std::env::temp_dir().join(format!("lunac-rw-file-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let out = run_query_rewrite(&FixedTerms(&["a"]), "test-model", "q", &file);
        assert_eq!(out.terms, vec!["a"]);
        let _ = std::fs::remove_file(&file);
    }

    /// 默认关 / 未配端点 ⇒ 补充召回整层不介入（零请求、零账本行，检索与改造前逐字节一致）。
    #[test]
    fn query_rewrite_transparent_when_disabled() {
        if decide::enabled() || decide::LocalHttpConfig::from_env().is_some() {
            eprintln!("环境已设 LUNAC_LOCAL_DECIDE / LUNAC_LOCAL_MODEL_URL ⇒ 跳过本断言");
            return;
        }
        assert!(
            rewrite_memory_query("四空格缩进").is_none(),
            "默认关 / 未配端点时整层不介入"
        );
    }

    // ── S1 第四切片：安全预筛的接线行为证据（2026-10-24）──────────────────────
    //
    // 本切片**只记账** ⇒ 证据全在账本侧：一行、带工具名、原文不落盘、故障记 unknown。

    struct FixedRisk(safety_prescreen::Risk);
    impl safety_prescreen::Screener for FixedRisk {
        fn screen(&self, _i: &str) -> Result<safety_prescreen::Risk, String> {
            Ok(self.0)
        }
    }
    struct RiskBoom;
    impl safety_prescreen::Screener for RiskBoom {
        fn screen(&self, _i: &str) -> Result<safety_prescreen::Risk, String> {
            Err("endpoint down".into())
        }
    }

    /// 预筛输入 = 工具名 + **已脱敏**摘要（凭据不得进输入文本）。
    #[test]
    fn safety_prescreen_input_carries_tool_name_and_is_masked() {
        let input = json!({"command":"用 sk-live-99887766 跑一下"});
        let text = safety_prescreen_input("PowerShell", &input);
        assert!(text.starts_with("tool=PowerShell "), "必须带工具名：{text}");
        assert!(!text.contains("99887766"), "凭据必须被脱敏：{text}");
    }

    /// 账本：恰好一行、`kind=safety-prescreen`、带工具名与档位，**命令原文不落盘**。
    #[test]
    fn run_safety_prescreen_writes_one_ledger_line_with_tool() {
        let dir = tmp_dir("safety");
        let input = safety_prescreen_input("Cmd", &json!({"command":"Remove-Item -Recurse -Force D:\\tmp"}));
        let v = run_safety_prescreen(
            &FixedRisk(safety_prescreen::Risk::High),
            "test-model",
            "Cmd",
            &input,
            &dir,
        );
        assert_eq!(v.risk, safety_prescreen::Risk::High);
        assert!(!v.fallback);
        let recs = decisions::load(&dir, tools::MemoryScope::Global);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].kind, "safety-prescreen");
        assert_eq!(recs[0].tool.as_deref(), Some("Cmd"));
        assert_eq!(recs[0].verdict, "high");
        assert_eq!(recs[0].turn, 0);
        let blob =
            std::fs::read_to_string(decisions::path_for(&dir, tools::MemoryScope::Global)).unwrap();
        assert!(!blob.contains("Remove-Item"), "命令原文不得落账本：{blob}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 后端故障 ⇒ 账本记 `unknown` + `fallback`（**不得**冒充 `low`）。
    #[test]
    fn run_safety_prescreen_failure_logs_unknown() {
        let dir = tmp_dir("safety-boom");
        let v = run_safety_prescreen(&RiskBoom, "test-model", "Write", "tool=Write {}", &dir);
        assert_eq!(v.risk, safety_prescreen::Risk::Unknown);
        assert!(v.fallback);
        let recs = decisions::load(&dir, tools::MemoryScope::Global);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].verdict, "unknown");
        assert!(recs[0].fallback);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── S1 第五切片：能力路由的接线行为证据（2026-10-24）──────────────────────
    //
    // 本切片**无消费者** ⇒ 证据只有两条：账本一行、提问原文不落盘；输入按 `ROUTE_INPUT_CHARS` 截断。

    struct FixedRoute(capability_route::Route);
    impl capability_route::Router for FixedRoute {
        fn route(&self, _i: &str) -> Result<capability_route::Route, String> {
            Ok(self.0)
        }
    }

    /// 账本：恰好一行、`kind=capability-route`、带档位，**提问原文不落盘**。
    #[test]
    fn run_capability_route_writes_one_ledger_line_without_plaintext() {
        let dir = tmp_dir("route");
        let out = run_capability_route(
            &FixedRoute(capability_route::Route::Delegate),
            "test-model",
            "通读整个仓库，找出所有直接调用 reqwest 的地方",
            tools::MemoryScope::Global,
            7,
            &dir,
        );
        assert_eq!(out.route, capability_route::Route::Delegate);
        let recs = decisions::load(&dir, tools::MemoryScope::Global);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].kind, "capability-route");
        assert_eq!(recs[0].verdict, "delegate");
        assert_eq!(recs[0].turn, 7);
        let blob =
            std::fs::read_to_string(decisions::path_for(&dir, tools::MemoryScope::Global)).unwrap();
        assert!(!blob.contains("reqwest"), "提问原文不得落账本：{blob}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 超长提问按 `ROUTE_INPUT_CHARS` 截断后再算哈希（口径与本地模型看到的输入一致）。
    #[test]
    fn run_capability_route_truncates_the_input() {
        let dir = tmp_dir("route-long");
        let long = "长".repeat(ROUTE_INPUT_CHARS + 200);
        let _ = run_capability_route(
            &FixedRoute(capability_route::Route::Work),
            "test-model",
            &long,
            tools::MemoryScope::Global,
            1,
            &dir,
        );
        let recs = decisions::load(&dir, tools::MemoryScope::Global);
        assert_eq!(recs.len(), 1);
        let expected = log::truncate_chars(long.trim(), ROUTE_INPUT_CHARS);
        assert_eq!(recs[0].input_hash, format!("{:016x}", log::hash64(&expected)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── C 臂：决策驱动瘦身的纯函数证据（2026-10-24）────────────────────────
    //
    // 只测 `apply_curation`（纯函数、不碰网络）；`curate_elision_plan` 的开关 / 端点判断
    // 由 `curation::enabled()` 与 `LocalHttpConfig::from_env()` 共同决定，默认关 ⇒ 原样返回。

    fn plan_with(picked: Vec<(usize, usize, usize)>, delta: usize) -> ElidePlan {
        let sigma = picked.iter().map(|(_, _, n)| *n).sum();
        ElidePlan {
            sigma,
            delta,
            net: sigma as i128 * ELIDE_PAYBACK_REQUESTS as i128
                - delta as i128 * CACHE_HIT_DISCOUNT as i128,
            picked,
        }
    }

    /// 构造一条 `tool_result` 消息（护栏只看 `content` 字符串）。
    fn tool_result_msg(text: &str) -> Value {
        json!({"role": "user", "content": [{"type": "tool_result", "content": text}]})
    }

    /// 护栏：命中具名事实的候选被移出名单，`sigma` 按实际重算。
    #[test]
    fn guard_removes_named_fact_item_and_recomputes_sigma() {
        let h = vec![
            tool_result_msg("code word: ORBITAL-FERRET-9"),
            tool_result_msg(&"x".repeat(50_000)),
        ];
        let plan = plan_with(vec![(0, 0, 2_000), (1, 0, 50_000)], 30);
        let out = guard_elision_plan(plan, &h);
        assert_eq!(out.picked, vec![(1, 0, 50_000)]);
        assert_eq!(out.sigma, 50_000);
        assert!(out.net > 0);
    }

    /// 护栏把「省下的体积」削到净亏 ⇒ 一条都不瘦（与 C 臂同一条成本判据）。
    #[test]
    fn guard_clears_the_plan_when_net_turns_negative() {
        let h = vec![
            tool_result_msg("PROJECT_RULE_ALPHA: Indentation must be 4 spaces"),
            tool_result_msg(&"y".repeat(1_000)),
        ];
        let plan = plan_with(vec![(0, 0, 5_000), (1, 0, 1_000)], 10_000);
        let out = guard_elision_plan(plan, &h);
        assert!(out.net <= 0, "夹具必须让 net 变负：{}", out.net);
        assert!(out.picked.is_empty());
    }

    /// 没有命中 ⇒ 逐字段原样（护栏不得改变任何东西）。
    #[test]
    fn guard_is_identity_when_nothing_is_protected() {
        let h = vec![tool_result_msg(&"z".repeat(50_000))];
        let plan = plan_with(vec![(0, 0, 50_000)], 100);
        assert_eq!(guard_elision_plan(plan.clone(), &h), plan);
    }

    /// 构造一条 `tool_use` 消息（护栏靠它拿候选的文件路径）。
    fn tool_use_msg(id: &str, file_path: &str) -> Value {
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": id, "name": "Read", "input": {"file_path": file_path}}
        ]})
    }

    /// 构造一条带 `tool_use_id` 的 `tool_result` 消息。
    fn tool_result_for(id: &str, text: &str) -> Value {
        json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": id, "content": text}
        ]})
    }

    /// 最近一轮提到过的文件 ⇒ 不瘦身；只提到别的文件 ⇒ 照旧。
    #[test]
    fn guard_protects_files_referenced_in_the_recent_tail() {
        let mut h = vec![
            tool_use_msg("t1", "notes/a.md"),
            tool_result_for("t1", &"x".repeat(50_000)),
            tool_use_msg("t2", "notes/c.md"),
            tool_result_for("t2", &"y".repeat(30_000)),
        ];
        for i in 0..COMPACT_KEEP_TAIL {
            h.push(json!({"role": "user", "content": [{"type": "text", "text": format!("继续看 notes/a.md 第 {i} 段")}]}));
        }
        let plan = plan_with(vec![(1, 0, 50_000), (3, 0, 30_000)], 100);
        let out = guard_elision_plan(plan, &h);
        assert_eq!(out.picked, vec![(3, 0, 30_000)], "被引用的 a.md 应保留");
    }

    /// 最近一轮只提到别的文件 ⇒ 逐字段原样。
    #[test]
    fn guard_does_not_protect_unrelated_files() {
        let mut h = vec![
            tool_use_msg("t1", "notes/a.md"),
            tool_result_for("t1", &"x".repeat(50_000)),
        ];
        for i in 0..COMPACT_KEEP_TAIL {
            h.push(json!({"role": "user", "content": [{"type": "text", "text": format!("继续看 notes/b.md 第 {i} 段")}]}));
        }
        let plan = plan_with(vec![(1, 0, 50_000)], 100);
        assert_eq!(guard_elision_plan(plan.clone(), &h), plan);
    }

    /// fail-open ⇒ **逐字段原样**（体积驱动说了算，C 臂不得改任何东西）。
    #[test]
    fn apply_curation_fail_open_is_identity() {
        let plan = plan_with(vec![(1, 0, 100), (2, 0, 50)], 30);
        let out = apply_curation(plan.clone(), &curation::Curation::busy_skip(), 2);
        assert_eq!(out, plan, "fail-open 必须与输入完全一致");
    }

    /// 模型说前 2 条里只有 1 可丢 ⇒ 名单保留 [1] + **未送判的尾部原样**。
    #[test]
    fn apply_curation_keeps_only_approved_and_preserves_unjudged_tail() {
        let plan = plan_with(vec![(1, 0, 100), (2, 0, 50), (3, 0, 10)], 30);
        let cur = curation::Curation { drop: vec![1], fallback: false, reason: "ok" };
        let out = apply_curation(plan, &cur, 2);
        assert_eq!(out.picked, vec![(1, 0, 100), (3, 0, 10)], "第 2 条被模型保留、第 3 条未送判照旧");
    }

    /// 名单变短后 `sigma` / `net` 必须按**实际**重算（日志口径不飘）。
    #[test]
    fn apply_curation_recomputes_sigma_and_net() {
        let plan = plan_with(vec![(1, 0, 100), (2, 0, 50)], 30);
        let cur = curation::Curation { drop: vec![1], fallback: false, reason: "ok" };
        let out = apply_curation(plan.clone(), &cur, 2);
        assert_eq!(out.sigma, 100, "只剩第 1 条 ⇒ sigma 重算为 100");
        assert_eq!(
            out.net,
            100 * ELIDE_PAYBACK_REQUESTS as i128 - plan.delta as i128 * CACHE_HIT_DISCOUNT as i128
        );
    }

    /// 模型说「一条都不能丢」⇒ 名单清空（调用点据此跳过瘦身，一个字都不动）。
    #[test]
    fn apply_curation_keep_all_empties_the_plan() {
        let plan = plan_with(vec![(1, 0, 100)], 30);
        let cur = curation::Curation { drop: vec![], fallback: false, reason: "keep-all" };
        let out = apply_curation(plan, &cur, 1);
        assert!(out.picked.is_empty());
    }

    /// **过滤后必须重跑成本判据**：省下的体积变小、`net <= 0` ⇒ 一条都不瘦
    /// （否则会「省小废大」地压，日志里 net 为负却照样动手）。
    #[test]
    fn apply_curation_skips_when_net_turns_negative() {
        // 原本 1000 字可省、delta=30 ⇒ 明显划算；模型只允许丢 1 字的小块 ⇒ net 变负。
        let plan = plan_with(vec![(1, 0, 1000), (2, 0, 1)], 30);
        let cur = curation::Curation { drop: vec![2], fallback: false, reason: "ok" };
        let out = apply_curation(plan, &cur, 2);
        assert!(out.net <= 0, "夹具必须让 net 变负：{}", out.net);
        assert!(out.picked.is_empty(), "省小废大 ⇒ 不瘦身");
    }

    /// ⑥ K 边界：K=1 退化为每轮；K ≥ N 只在首轮判一次。
    #[test]
    fn amortize_k_boundaries() {
        assert!((1..=23).all(|t| prescreen_due(t, 1)), "K=1 ⇒ 每轮都触发");
        let due: Vec<u64> = (1..=23).filter(|t| prescreen_due(*t, 100)).collect();
        assert_eq!(due, vec![1], "K≥N ⇒ 只判首轮");
        let d1 = tmp_dir("k1");
        let (_, c1) = simulate(&FixedRun, 1, 5, &d1);
        assert_eq!(c1, 5, "K=1 ⇒ 5 轮判 5 次");
        let d2 = tmp_dir("kbig");
        let (_, c2) = simulate(&FixedRun, 100, 23, &d2);
        assert_eq!(c2, 1, "K≥N ⇒ 只有 1 条账本");
        let _ = std::fs::remove_dir_all(&d1);
        let _ = std::fs::remove_dir_all(&d2);
    }

    /// ⑦ 默认关等同不接线：未设开关 / 未配端点 ⇒ `prescreen` 返回 `None`、照旧派。
    #[test]
    fn prescreen_transparent_when_disabled() {
        if decide::enabled() || decide::LocalHttpConfig::from_env().is_some() {
            eprintln!("环境已设 LUNAC_LOCAL_DECIDE / LUNAC_LOCAL_MODEL_URL ⇒ 跳过本断言");
            return;
        }
        let v = prescreen("随便一句", "prescreen-review", tools::MemoryScope::Global, 1);
        assert!(v.is_none(), "默认关 / 未配端点时整层不介入");
        assert!(allow_review_spawn(v), "None ⇒ 照旧派（与不接线逐字节一致）");
    }

    /// ⑧ 预热**不在预筛路径上**：N 次预筛 ⇒ 计数桩恰好被问 N 次（没有额外的预热请求混进来）。
    /// 「恰好预热一次」由 `spawn_local_decide_warmup` 的**唯一启动调用点** + `decide.rs` 的
    /// `warmup_asks_the_backend_exactly_once` 共同保证。
    #[test]
    fn prescreen_path_never_triggers_a_warmup_request() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        struct Counting(Arc<AtomicUsize>);
        impl decide::Decider for Counting {
            fn decide(&self, _i: &str) -> Result<decide::Decision, String> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(decide::Decision::run("counted"))
            }
        }
        let n = Arc::new(AtomicUsize::new(0));
        let dir = tmp_dir("nowarm");
        let (spawns, _) = simulate(&Counting(n.clone()), 1, 7, &dir); // K=1 ⇒ 7 轮判 7 次
        assert_eq!(n.load(Ordering::SeqCst), 7, "预筛恰好问 7 次，无预热请求混入");
        assert_eq!(spawns, 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── S1 第二切片：记忆候选抽取的接线行为证据（2026-10-15）────────────────
    //
    // 与上面 S1 首切片同构：可注入的假 `Extractor` **精确计数**、账本只落 hash + 长度、
    // 开关关 / 故障 ⇒ 主流程零差异、**绝不落用户原文**。

    /// 固定回一条候选 / 恒失败 / 回显输入的三种桩。
    struct OneFact;
    impl memory_candidates::Extractor for OneFact {
        fn extract(&self, _i: &str) -> Result<Vec<String>, String> {
            Ok(vec!["用户偏好 4 空格缩进".into()])
        }
    }
    struct CandBoom;
    impl memory_candidates::Extractor for CandBoom {
        fn extract(&self, _i: &str) -> Result<Vec<String>, String> {
            Err("endpoint down".into())
        }
    }
    /// 故意把**输入原文**当候选返回（用于证明账本绝不落原文）。
    struct EchoInput;
    impl memory_candidates::Extractor for EchoInput {
        fn extract(&self, i: &str) -> Result<Vec<String>, String> {
            Ok(vec![i.to_string()])
        }
    }

    /// 模型化「每 K 轮 + 每个压缩事件各触发一次」的接线，返回账本条数。
    fn simulate_memory(d: &dyn memory_candidates::Extractor, k: u64, n: u64, compactions: usize, dir: &Path) -> usize {
        for turn in 1..=n {
            if prescreen_due(turn, k) {
                run_memory_extraction(d, "test-model", &format!("turn-{turn}"), tools::MemoryScope::Global, turn, dir);
            }
        }
        for c in 0..compactions {
            run_memory_extraction(d, "test-model", &format!("compact-{c}"), tools::MemoryScope::Global, 0, dir);
        }
        decisions::count(dir, tools::MemoryScope::Global)
    }

    /// ① 摊销**精确计数**：N=23、K=5 ⇒ 恰好 5 条（轮 1/6/11/16/21，与 S1 首切片同口径）；
    /// 再叠 3 个压缩事件 ⇒ 5 + 3 = 8 条（压缩事件各触发一次）。
    #[test]
    fn memory_extraction_is_amortized_every_k_turns_and_per_compaction() {
        let dir = tmp_dir("cand-k5");
        assert_eq!(simulate_memory(&OneFact, 5, 23, 0, &dir), 5, "N=23,K=5 必须恰好抽 5 次");
        let _ = std::fs::remove_dir_all(&dir);
        let dir2 = tmp_dir("cand-k5c");
        assert_eq!(simulate_memory(&OneFact, 5, 23, 3, &dir2), 8, "每压缩事件各 +1");
        let _ = std::fs::remove_dir_all(&dir2);
    }

    /// ② 账本内容正确：`kind=memory-candidates`、`candidate_count` 与 `candidates` 长度一致、
    /// 每候选带 hash + 长度。
    #[test]
    fn memory_extraction_records_kind_and_candidate_refs() {
        let dir = tmp_dir("cand-kind");
        run_memory_extraction(&OneFact, "test-model", "user: 记住 4 空格", tools::MemoryScope::Global, 7, &dir);
        let recs = decisions::load(&dir, tools::MemoryScope::Global);
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.kind, "memory-candidates");
        assert_eq!(r.verdict, "ok");
        assert_eq!(r.turn, 7);
        assert_eq!(r.candidate_count, Some(1));
        let c = &r.candidates.as_ref().unwrap()[0];
        assert_eq!(c.hash, format!("{:016x}", log::hash64("用户偏好 4 空格缩进")));
        assert_eq!(c.len, "用户偏好 4 空格缩进".chars().count() as u32);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ③ **绝不落原文**：即便模型把**输入原文**当候选返回，账本里也搜不到那个独特字符串
    /// （只落 hash 与长度）。
    #[test]
    fn memory_extraction_ledger_never_holds_plaintext() {
        let dir = tmp_dir("cand-mask");
        let unique = "独特字符串-ZZZUNIQUE-12345";
        run_memory_extraction(
            &EchoInput,
            "test-model",
            &format!("user: 请记住 {unique} 这件事"),
            tools::MemoryScope::Global,
            1,
            &dir,
        );
        let blob = std::fs::read_to_string(decisions::path_for(&dir, tools::MemoryScope::Global)).unwrap();
        assert!(!blob.contains(unique), "账本绝不能含用户原文：{blob}");
        assert!(!blob.contains("ZZZUNIQUE"), "账本绝不能含用户原文：{blob}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ④ **故障 / 不合 schema = 主流程零差异**：假 `Extractor` 恒 `Err` ⇒ 记账走 `fallback=true`
    /// 且候选为空，**history / system / tool_defs 指纹逐字节不变**、无 panic。
    #[test]
    fn memory_extraction_backend_error_keeps_main_flow_identical() {
        let dir = tmp_dir("cand-err");
        let history = vec![
            json!({"role":"user","content":[{"type":"text","text":"帮我把 parse_args 改对"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"好"}]}),
        ];
        let system = "SYS-FIXED-PREFIX";
        let tool_defs = vec![json!({"name":"Read"}), json!({"name":"Edit"})];
        let fingerprint = |h: &[Value]| {
            log::hash64(&format!(
                "{system}\u{1}{}",
                serde_json::to_string(&(h, &tool_defs)).unwrap()
            ))
        };
        let before = fingerprint(&history);
        let out = run_memory_extraction(&CandBoom, "test-model", "user: hi", tools::MemoryScope::Global, 1, &dir);
        assert!(out.facts.is_empty(), "故障必须丢弃候选");
        assert!(out.fallback, "必须标 fallback");
        assert_eq!(fingerprint(&history), before, "固定前缀指纹必须逐字节不变");
        assert_eq!(history.len(), 2, "history 不被抽取改写");
        let recs = decisions::load(&dir, tools::MemoryScope::Global);
        assert_eq!(recs.len(), 1, "故障也留一条 fallback 账本行（与 S1 同口径）");
        assert!(recs[0].fallback);
        assert_eq!(recs[0].candidate_count, Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⑤ **默认关 ⇒ 零请求、零账本行**：未设开关 / 未配端点时 `extract_candidates` 直接返回。
    #[test]
    fn memory_extraction_transparent_when_disabled() {
        if decide::enabled() || decide::LocalHttpConfig::from_env().is_some() {
            eprintln!("环境已设 LUNAC_LOCAL_DECIDE / LUNAC_LOCAL_MODEL_URL ⇒ 跳过本断言");
            return;
        }
        let dir = tmp_dir("cand-off");
        let history = vec![json!({"role":"user","content":[{"type":"text","text":"帮我记住 4 空格缩进"}]})];
        // 走**显式目录**的可测内核：默认关 ⇒ 提前返回，**零账本行**（不碰真实数据根）。
        extract_candidates_into(&dir, &history, tools::MemoryScope::Global, 1);
        assert_eq!(decisions::count(&dir, tools::MemoryScope::Global), 0, "默认关 ⇒ 不得写任何候选账本行");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **端到端**（要外部环境，默认 `#[ignore]`）：起真的 `llama-server`、设 `LUNAC_LOCAL_MODEL_URL`
    /// 后跑 `cargo test -- --ignored --nocapture`。真的走一次「记忆候选抽取 + 记账」并打印账本样例行。
    /// **未设环境变量时直接跳过**（只打印一行），不会把常规测试搞红。
    #[test]
    #[ignore = "需要本地 llama-server：设 LUNAC_LOCAL_MODEL_URL 后跑 cargo test -- --ignored"]
    fn memory_extraction_end_to_end_writes_a_candidate_line() {
        let Some(cfg) = decide::LocalHttpConfig::from_env() else {
            eprintln!("跳过：未设 LUNAC_LOCAL_MODEL_URL");
            return;
        };
        let model = cfg.model.clone();
        let dir = std::env::temp_dir().join(format!("lunac-cand-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let input = "user: 我们项目里的 Rust 代码一律用 cargo clippy 必须 0 warning，提交前必须跑 npm run verify。\nassistant: 明白，以后改 Rust 都先跑 clippy。";
        let out = run_memory_extraction(
            &memory_candidates::LocalExtract::new(cfg),
            &model,
            input,
            tools::MemoryScope::Global,
            1,
            &dir,
        );
        let line = std::fs::read_to_string(decisions::path_for(&dir, tools::MemoryScope::Global)).unwrap();
        eprintln!("真机候选条数 = {}", out.facts.len());
        eprintln!("账本样例 = {}", line.trim());
        assert!(!line.trim().is_empty(), "必须写出账本样例行");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **端到端**（要外部环境，默认 `#[ignore]`）：起真的 `llama-server`、设 `LUNAC_LOCAL_MODEL_URL`
    /// 后跑 `cargo test -- --ignored --nocapture`。真的走一次「本地只读预筛 + 记账」，打印账本样例行。
    /// **未设环境变量时直接跳过**（只打印一行），不会把常规测试搞红。
    #[test]
    #[ignore = "需要本地 llama-server：设 LUNAC_LOCAL_MODEL_URL 后跑 cargo test -- --ignored"]
    fn prescreen_end_to_end_writes_a_ledger_line() {
        let Some(cfg) = decide::LocalHttpConfig::from_env() else {
            eprintln!("跳过：未设 LUNAC_LOCAL_MODEL_URL");
            return;
        };
        let model = cfg.model.clone();
        let d = decide::LocalHttp::new(cfg);
        let dir = std::env::temp_dir().join(format!("lunac-pre-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let action = run_prescreen(
            &d,
            &model,
            "帮我看下 src/main.rs 这个解析报错",
            "prescreen-review",
            tools::MemoryScope::Global,
            1,
            &dir,
        );
        let line =
            std::fs::read_to_string(decisions::path_for(&dir, tools::MemoryScope::Global)).unwrap();
        eprintln!("真机判定 = {action:?}");
        eprintln!("账本样例 = {}", line.trim());
        assert!(!line.trim().is_empty(), "必须写出账本样例行");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 单发模式（--print）的参数判定（2026-10-11）。
    /// 钉住四件事：① 带值形态把紧跟的非 flag 当 query；② 不带值形态（后面是别的 flag）
    /// 不误吃、且仍判定为单发；③ `--print` + `--input-format stream-json` = 常驻（宿主形态），
    /// 绝不能退化成单发；④ --output-format 默认 text。
    #[test]
    fn cli_print_single_shot_detection() {
        let parse = |v: &[&str]| CliArgs::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());

        let a = parse(&["--print", "Reply with the single word: PONG"]);
        assert!(a.print && a.single_shot());
        assert_eq!(a.print_query.as_deref(), Some("Reply with the single word: PONG"));
        assert_eq!(a.output_format, "text");

        // 短选项 + 后面的参数不被误当 query
        let b = parse(&["-p", "--output-format", "json"]);
        assert!(b.print && b.single_shot());
        assert_eq!(b.print_query, None);
        assert_eq!(b.output_format, "json");

        // 宿主形态：--print 后面紧跟的是 flag ⇒ 不吃；带 --input-format stream-json ⇒ 常驻
        let c = parse(&[
            "--print",
            "--verbose",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            ".",
        ]);
        assert!(c.print && c.input_stream_json);
        assert!(!c.single_shot());
        assert_eq!(c.print_query, None);
    }

    /// `--checkpoint` / `--resume` 的解析（S2，2026-10-22）。钉住三件事：
    /// 默认都不开；`--resume <id|latest>` 取到值；裸 `--resume` 视为 latest 且**不吞** query。
    #[test]
    fn cli_checkpoint_and_resume_parse() {
        let parse = |v: &[&str]| CliArgs::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());

        let a = parse(&["--print", "q"]);
        assert!(!a.checkpoint && a.resume.is_none(), "默认不写、不续跑");

        let b = parse(&["--checkpoint", "--print", "q", "--resume", "sess_1_2"]);
        assert!(b.checkpoint);
        assert_eq!(b.resume.as_deref(), Some("sess_1_2"));
        assert_eq!(b.print_query.as_deref(), Some("q"), "query 不被后面的 --resume 抢走");

        let c = parse(&["--print", "q", "--resume", "latest"]);
        assert_eq!(c.resume.as_deref(), Some("latest"));

        let d = parse(&["--print", "q", "--resume"]);
        assert_eq!(d.resume.as_deref(), Some("latest"), "裸 --resume = latest");
        assert_eq!(d.print_query.as_deref(), Some("q"));

        // `--continue`：解析层只置标志；「隐含 resume latest + 内置提问」在 main 里合成。
        let e = parse(&["--print", "--continue"]);
        assert!(e.continue_task);
        assert_eq!(e.print_query, None, "没有显式 query ⇒ 交给 continue 的内置提问");
        let f = parse(&["--print", "do X", "--continue"]);
        assert!(f.continue_task);
        assert_eq!(f.print_query.as_deref(), Some("do X"), "显式 query 优先");
    }

    /// 单发退出码的调度契约（2026-10-22）：失败优先；「未完成」只有在**可续跑**时才给 3。
    #[test]
    fn single_shot_exit_code_distinguishes_failure_and_unfinished() {
        assert_eq!(single_shot_exit_code(false, false, false), 0);
        assert_eq!(single_shot_exit_code(false, true, false), 0, "没有检查点不承诺可续跑");
        assert_eq!(single_shot_exit_code(false, true, true), 3);
        assert_eq!(single_shot_exit_code(false, false, true), 0, "清单完成就算成功");
        assert_eq!(single_shot_exit_code(true, true, true), 1, "失败优先于未完成");
    }

    /// `--include-partial-messages` 的解析与默认值（2026-10-14）。
    /// 钉住三件事：① 默认关（不传 ⇒ false）；② 传了即 true；③ 与 `--output-format stream-json`
    /// 并存不互斥，且不吞 query / output-format。
    #[test]
    fn cli_include_partial_messages_defaults_off_and_parses() {
        let parse = |v: &[&str]| CliArgs::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());

        // 默认关：不给开关 ⇒ false（单发默认不出增量行）
        let off = parse(&["--print", "--output-format", "stream-json", "hi"]);
        assert!(off.single_shot());
        assert!(!off.include_partial_messages);

        // 给了开关 ⇒ true，query / output-format 均不受影响
        let on = parse(&[
            "--print",
            "hi",
            "--output-format",
            "stream-json",
            "--include-partial-messages",
        ]);
        assert!(on.single_shot());
        assert!(on.include_partial_messages);
        assert_eq!(on.output_format, "stream-json");
        assert_eq!(on.print_query.as_deref(), Some("hi"));
    }

    /// `--unattended-allow`（2026-10-27，§待办 3-1d 阶段 2B）：解析 + **只收敛到写类三件**。
    ///
    /// 这条钉住的是安全边界本身：命令类 / 外部出口 / 派生任务 / MCP 即便被写进命令行，
    /// 也绝不能进白名单 —— 无人值守不能靠模型（或调用方）自觉。
    #[test]
    fn unattended_allow_parses_and_only_covers_write_tools() {
        let parse = |v: &[&str]| CliArgs::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());

        // 不给开关 ⇒ 空名单（与改造前逐字节一致）
        let off = parse(&["--print", "hi"]);
        assert!(off.single_shot());
        assert!(off.unattended_allow.is_empty());
        assert!(effective_unattended_allow(&off.unattended_allow).is_empty());

        // 逗号 + 重复都给，收敛后去重且保持顺序
        let on = parse(&[
            "--print",
            "hi",
            "--unattended-allow",
            "Write,Edit",
            "--unattended-allow",
            "NotebookEdit,Write",
        ]);
        assert_eq!(
            on.unattended_allow,
            vec!["Write,Edit".to_string(), "NotebookEdit,Write".to_string()]
        );
        assert_eq!(
            effective_unattended_allow(&on.unattended_allow),
            vec!["Write".to_string(), "Edit".to_string(), "NotebookEdit".to_string()]
        );

        // 命令类 / 外部出口 / 派生任务 / MCP 一律丢弃
        let bad = effective_unattended_allow(&[
            "Cmd,PowerShell,WebFetch,WebSearch,Agent,CronCreate,TaskEnqueue,mcp__files__read"
                .to_string(),
        ]);
        assert!(bad.is_empty(), "{bad:?}");

        // 名字只认精确写法（大小写不敏感地放行等于放宽边界）
        assert!(effective_unattended_allow(&["write".to_string()]).is_empty());
        assert_eq!(
            effective_unattended_allow(&[" Write ".to_string()]),
            vec!["Write".to_string()]
        );
    }

    /// 「先解释后执行」闸（2026-11-05）：顺序词命中 ⇒ 写类拒绝、只读放行。
    ///
    /// 用例里的原话来自 2026-11-04 mod 会话，正是这个闸要覆盖的场景。
    #[test]
    fn explain_first_gate_detects_phrases_and_blocks_writes() {
        assert!(explain_first_requested("所以你必须先解释然后才能继续改"));
        // 明确否定不误拦
        assert!(!explain_first_requested("不用先解释，直接改"));
        // 普通提问不触发
        assert!(!explain_first_requested("帮我看看这个报错"));

        let empty = json!({});
        for name in [
            "Write",
            "Edit",
            "NotebookEdit",
            "Cmd",
            "PowerShell",
            "Agent",
            "Remember",
            "EnterWorktree",
            "ImageGen",
            "CronCreate",
            "CronDelete",
            "mcp__foo__bar",
        ] {
            assert!(
                explain_first_blocks(name, &empty, &[]),
                "{name} 在解释轮应被拒绝"
            );
            assert!(
                explain_first_denial(true, name, &empty, &[]).is_some(),
                "{name} 应产出拒绝文案"
            );
        }

        for name in [
            "Read",
            "Glob",
            "Grep",
            "TodoWrite",
            "SendUserFile",
            "PushNotification",
            "WebSearch",
            "WebFetch",
            "WebBrowser",
            "AskUserQuestion",
            "EnterPlanMode",
        ] {
            assert!(
                !explain_first_blocks(name, &empty, &[]),
                "{name} 在解释轮应放行"
            );
        }

        // 闸关 ⇒ 一律不拦（默认行为逐字节不变）
        assert!(explain_first_denial(false, "Write", &empty, &[]).is_none());
    }

    /// 回合相位（2026-11-05）：只有 `Executing` 允许写，其余相位都只读。
    /// 同时钉住完整迁移表：`Idle` 只能进 `Planning` / `Executing`；
    /// `Done` / `Interrupted` 是终态；`Planning` 不能自己切到 `Executing`。
    #[test]
    fn turn_phase_only_executing_allows_writes() {
        assert!(TurnPhase::Executing.allows_writes());
        for p in [
            TurnPhase::Idle,
            TurnPhase::Planning,
            TurnPhase::AwaitingUser,
            TurnPhase::Done,
            TurnPhase::Interrupted,
        ] {
            assert!(!p.allows_writes(), "{} 不该允许写", p.as_str());
        }
        assert_eq!(TurnPhase::Planning.as_str(), "Planning");

        // 合法链路：两条主路径 + 中断分支
        for (from, to) in [
            (TurnPhase::Idle, TurnPhase::Planning),
            (TurnPhase::Idle, TurnPhase::Executing),
            (TurnPhase::Planning, TurnPhase::AwaitingUser),
            (TurnPhase::Planning, TurnPhase::Done),
            (TurnPhase::Planning, TurnPhase::Interrupted),
            (TurnPhase::AwaitingUser, TurnPhase::Done),
            (TurnPhase::AwaitingUser, TurnPhase::Interrupted),
            (TurnPhase::Executing, TurnPhase::Done),
            (TurnPhase::Executing, TurnPhase::Interrupted),
        ] {
            assert!(
                from.can_transition_to(to),
                "{} → {} 应合法",
                from.as_str(),
                to.as_str()
            );
        }

        // 非法迁移：终态不能复活；「先解释」轮不能自己升级成可写轮
        for (from, to) in [
            (TurnPhase::Planning, TurnPhase::Executing),
            (TurnPhase::Done, TurnPhase::Executing),
            (TurnPhase::Done, TurnPhase::Planning),
            (TurnPhase::Interrupted, TurnPhase::Executing),
            (TurnPhase::Interrupted, TurnPhase::Done),
            (TurnPhase::AwaitingUser, TurnPhase::Executing),
        ] {
            assert!(
                !from.can_transition_to(to),
                "{} → {} 必须判非法",
                from.as_str(),
                to.as_str()
            );
        }
    }

    /// `--include-partial-messages` 对 `stream_event` 增量的 gating + 行形状（2026-10-14）。
    /// 关 = 不出（默认，绝不新增噪音）；开 = 出一行 `{"type":"stream_event","event":{…}}`
    /// （既有 stream-json 形态，字段不动）。用纯函数断言，避免捕获进程 stdout。
    #[test]
    fn include_partial_messages_gates_stream_event_and_keeps_shape() {
        let delta = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "hi" },
        });

        // 开：出增量行，形状 = 既有 {"type":"stream_event","event":{…}}
        set_include_partial_messages(true);
        let line = stream_event_line(delta.clone()).expect("开启时应当出增量行");
        assert_eq!(line["type"], json!("stream_event"));
        assert_eq!(line["event"]["type"], json!("content_block_delta"));
        assert_eq!(line["event"]["delta"]["text"], json!("hi"));

        // 关：不出（None）
        set_include_partial_messages(false);
        assert!(
            stream_event_line(delta).is_none(),
            "关闭时不得出增量行（默认关）"
        );

        // 复位成常驻默认（true），避免影响同进程其它用例
        set_include_partial_messages(true);
    }

    /// 计划相位 / 档位判据的单测用最小上下文（与 `tools.rs` 里那份同形）：
    /// 只关心两个标志，不碰磁盘。
    fn test_ctx() -> tools::Ctx {
        tools::Ctx {
            cwd: PathBuf::from("."),
            add_dirs: Vec::new(),
            read_only: false,
            locked: false,
            deny_read_dirs: Vec::new(),
            worktree: Arc::new(Mutex::new(None)),
            plan_phase: Arc::new(AtomicBool::new(false)),
            tool_output: None,
            shell_control: None,
            memory_scope: tools::MemoryScope::Global,
            skill_scope: None,
        }
    }

    /// 桌宠自我区（2026-10-13 人格地基）的前置判据：**仅桌宠轮**、写类档位拒、
    /// `PetGrowth` **每轮 ≤ 1 条**、内容非空。判据纯函数（不碰桥），故直接单测。
    #[test]
    fn pet_self_write_gate_is_pet_only_and_caps_growth_per_turn() {
        // 进程级静态：本用例自己摆，别依赖别处的复位。
        PET_GROWTH_USED_THIS_TURN.store(false, Ordering::Relaxed);

        // 非桌宠轮（作用域 = 全局）⇒ 两件都拒（① ）
        let global = test_ctx();
        for n in ["PetGrowth", "PetPersonaSuggest"] {
            let e = pet_self_write_gate(&global, n, &json!({ "content": "x" })).unwrap_err();
            assert!(e.contains("desktop-pet"), "{n} 非桌宠轮应被拒：{e}");
        }

        // 桌宠轮（作用域 = Pet）⇒ 放行，返回 trim 后的内容
        let mut pet = test_ctx();
        pet.memory_scope = tools::MemoryScope::Pet;
        assert_eq!(
            pet_self_write_gate(&pet, "PetPersonaSuggest", &json!({ "content": "  建议  " })).unwrap(),
            "建议"
        );

        // 空内容 ⇒ 拒
        assert!(pet_self_write_gate(&pet, "PetGrowth", &json!({ "content": "   " })).is_err());

        // ③ 每轮 ≤ 1 条：连着两次 `PetGrowth` ⇒ 第二次必拒；复位后（= 下一轮）又能写
        assert!(pet_self_write_gate(&pet, "PetGrowth", &json!({ "content": "第一次" })).is_ok());
        let e = pet_self_write_gate(&pet, "PetGrowth", &json!({ "content": "第二次" })).unwrap_err();
        assert!(e.contains("最多追加一条"), "{e}");
        PET_GROWTH_USED_THIS_TURN.store(false, Ordering::Relaxed);
        assert!(pet_self_write_gate(&pet, "PetGrowth", &json!({ "content": "下一轮" })).is_ok());

        // 写类档位（只读 / 计划相位）⇒ 拒（与 `Remember` 同源 `write_blocked`）（② ）
        let mut ro = test_ctx();
        ro.memory_scope = tools::MemoryScope::Pet;
        ro.read_only = true;
        assert!(pet_self_write_gate(&ro, "PetPersonaSuggest", &json!({ "content": "x" })).is_err());

        PET_GROWTH_USED_THIS_TURN.store(false, Ordering::Relaxed);
    }

    /// ⑧ 成长笔记注入**封顶**（复用 `MEMORY_INJECT_CHARS`）且**不进 system**（§45）：
    /// 超长截到上限、两次截断逐字节相同（前缀稳定），并作为 history 开头的上下文块注入。
    #[test]
    fn growth_notes_are_capped_and_live_in_the_context_block() {
        let huge = "字".repeat(MEMORY_INJECT_CHARS + 5_000);
        let capped = cap_inject(&huge);
        // 封顶 = 截到上限 + 一小段截断标记（`…(truncated)`）；关键是**有硬上限**、不再随文件增长。
        assert!(
            capped.chars().count() <= MEMORY_INJECT_CHARS + 16,
            "必须封顶（上限 + 截断标记）：{}",
            capped.chars().count()
        );
        assert!(capped.ends_with("…(truncated)"), "截断必须留可辨识标记");
        assert_eq!(cap_inject(&huge), capped, "两次截断必须逐字节相同（前缀稳定）");
        assert_eq!(cap_inject("短笔记"), "短笔记", "不超上限时原样");

        // 空正文 + 写侧不可用 ⇒ 不加空壳
        assert_eq!(growth_block_text(false, ""), "");
        // 有正文 ⇒ 表头 + 引导在前、正文永远拼在其后（空→有只 miss 正文那一段）
        let blk = growth_block_text(true, "被夸时轻轻晃一下");
        assert!(blk.contains("Growth notes") && blk.contains("PetGrowth"));
        assert!(
            blk.rfind("被夸时轻轻晃一下").unwrap() > blk.find("PetGrowth").unwrap(),
            "正文必须拼在引导之后"
        );

        // 合成 history 开头的一条 user 消息，表头可被幂等判据认出；空 ⇒ None
        assert!(context_block_message("   ").is_none());
        let cb = context_block_message(&blk).expect("非空时应生成块");
        let text = cb.pointer("/content/0/text").and_then(Value::as_str).unwrap();
        assert!(text.starts_with(CONTEXT_BLOCK_HEADER));
        assert!(is_context_block_msg(&cb), "成长笔记块必须能被幂等判据认出");

        // §45：成长笔记**不得进 system 固定前缀**（否则每长一条就整段 miss）
        let env = env_block(std::path::Path::new("C:/work"));
        let system = build_system_prompt("", &env, "");
        assert!(!system.contains("Growth notes"), "成长笔记不得进 system 固定前缀");
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
    /// 边界：`Write` 取 `content`、`Edit` 取 `new_string`（**不取 `old_string`**：
    /// 那是要被删掉的内容，扫它会把「正在清理凭据」标成可疑）、`NotebookEdit` 取
    /// `new_source`（delete 档没有该字段 ⇒ 不扫）、非写入类工具一律空、
    /// 干净内容也是空（前端据此决定「有没有这个键」）。
    #[test]
    fn security_warnings_only_cover_written_payloads() {
        // 夹具拆段拼接：拼接后仍是一段 PEM 私钥头（判据照样命中），源码里则不成形。
        let secret = concat!("-----BEGIN RSA ", "PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n");
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
        assert_eq!(
            hits(security_warnings(
                "NotebookEdit",
                &json!({ "notebook_path": "n.ipynb", "new_source": secret })
            ))
            .len(),
            1,
            "NotebookEdit 扫 new_source"
        );
        assert!(
            hits(security_warnings(
                "NotebookEdit",
                &json!({ "notebook_path": "n.ipynb", "edit_mode": "delete" })
            ))
            .is_empty(),
            "delete 档没有 new_source ⇒ 不扫"
        );
        assert!(hits(security_warnings("Cmd", &json!({ "command": "echo hi" }))).is_empty());
        assert!(hits(security_warnings("Write", &json!({ "content": "fn main() {}" }))).is_empty());
    }

    /// `CronCreate`（2026-10-17 阶段 1）：**hook 的 allow 不能替用户跳过审批卡**。
    ///
    /// 用户裁定「所有运行模式都强制弹卡」；这条钉住 `hook_allow_needs_card` 对它的恒真。
    #[test]
    fn cron_create_always_needs_a_card_even_after_hook_allow() {
        assert!(
            hook_allow_needs_card("CronCreate", &json!({ "prompt": "read files" })),
            "CronCreate 必须始终要审批卡（hook allow 也不行）"
        );
        assert!(
            hook_allow_needs_card("TaskEnqueue", &json!({ "prompt": "read files" })),
            "TaskEnqueue 必须始终要审批卡（hook allow 也不行）"
        );
        assert!(!hook_allow_needs_card("CronDelete", &json!({ "id": "cron_1" })));
        assert!(!hook_allow_needs_card("TaskCancel", &json!({ "id": "task_1" })));
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

    /// 前缀稳定性门禁（2026-10-27，§待办 3-16 P1-4）：只在「够长 + 跌破 90%」时告警。
    #[test]
    fn prefix_stability_gate_ignores_short_histories_and_the_90_percent_edge() {
        // 条数不够不判 —— 一问刚开始的重排是正常现象
        assert_eq!(prefix_stability_break(0, PREFIX_STABILITY_MIN_ITEMS - 1), None);
        // 纯追加（公共前缀 == 上轮条数）永远稳定
        assert_eq!(prefix_stability_break(40, 40), None);
        // 正好 90% 判稳定（口径是「90% 以下才告警」）
        assert_eq!(prefix_stability_break(9, 10), None);
        assert_eq!(prefix_stability_break(18, 20), None);
        // 跌破 90% 就要报：80% / 85% / 全塌
        assert_eq!(prefix_stability_break(8, 10), Some(80));
        assert_eq!(prefix_stability_break(17, 20), Some(85));
        assert_eq!(prefix_stability_break(0, 20), Some(0));
        // 空历史不产生除零 panic
        assert_eq!(prefix_stability_break(0, 0), None);
    }

    /// 子代理与复盘的账本**互不串账**（2026-10-27，§待办 3-16 P2-8）。
    ///
    /// 拆开的唯一目的就是能回答「钱花在探索还是收口上」；两本账若共享同一个 `Arc`
    /// （或 `detached()` 时漏 clone），拆了也是白拆 —— 这条钉住「分别记账 + 跨副本共享」。
    #[test]
    fn subagent_and_review_ledgers_stay_separate() {
        let cfg = cfg_pointing_at("http://127.0.0.1:1/v1/messages");
        let copy = cfg.detached().expect("detached");
        cfg.sub.record(1, 2, 3, 4);
        copy.review.record(10, 20, 30, 40);

        // detached 副本与主 cfg 共享同一对 Arc：从副本记的也必须能被主 cfg 取走
        let (ri, rr, rc, ro, rreqs) = cfg.review.take();
        assert_eq!((ri, rr, rc, ro), (10, 20, 30, 40), "复盘的用量必须在 review 账本里");
        assert_eq!(rreqs.len(), 1);

        let (si, sr, sc, so, sreqs) = cfg.sub.take();
        assert_eq!((si, sr, sc, so), (1, 2, 3, 4), "子代理的用量必须在 sub 账本里");
        assert_eq!(sreqs.len(), 1);

        // 两边都已归零（`take()` 的口径 = 本次提问的绝对值）
        assert_eq!(cfg.sub.take().0, 0);
        assert_eq!(cfg.review.take().0, 0);
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

    /// 工具 schema 的**键序确定性**（2026-10-27，§待办 3-16 P1-5）。
    ///
    /// 依赖的是「`serde_json` 未开 `preserve_order` ⇒ Map 是 BTreeMap ⇒ 键按字典序」。
    /// 这条测试同时钉两件事：① `tools::defs()` 连续两次序列化逐字节相同；② 那个前提仍成立
    /// —— 一旦有人打开 `preserve_order`，第二条会立刻失败，提醒他键序已经变成「按构造顺序」，
    /// 前缀缓存的稳定性不再是免费的。
    #[test]
    fn tool_schema_json_is_byte_stable_under_btreemap_ordering() {
        let a = serde_json::to_string(&tools::defs(&[])).unwrap_or_default();
        let b = serde_json::to_string(&tools::defs(&[])).unwrap_or_default();
        assert_eq!(a, b, "工具 schema 必须逐字节可复现（键序漂移会让整段固定前缀失效）");
        assert!(!a.is_empty(), "工具表不该为空");

        // 前提守卫：反向书写的两个键必须序列化成字典序
        let obj = json!({ "b": 1, "a": 2 });
        assert_eq!(
            serde_json::to_string(&obj).unwrap_or_default(),
            r#"{"a":2,"b":1}"#,
            "serde_json 的 Map 必须是 BTreeMap（见 core-agent/Cargo.toml 的注释）"
        );
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

    /// 「阶段小结」是 L6 输出纪律的**唯一例外**，且必须写明**只对多轮工具任务生效**（2026-10-08）。
    ///
    /// 两条都要钉住，因为它们互为前提：
    /// ① **例外本身在**（`Phase summaries` 那句）—— 用户要的「每段思考后一次小输出」就靠它；
    /// ② **范围收窄也在**（`single-step` 那句）—— 一旦丢了，模型会给每个单步问答都加一段小结，
    ///    正好把 2026-09-29 的 L6 降本成果吃回去（那个例外之所以能开，就是因为只覆盖多轮任务）。
    #[test]
    fn phase_summaries_are_scoped_to_multi_round_tasks() {
        assert!(
            PERSONA_AND_STYLE.contains("Phase summaries"),
            "「阶段小结」这条丢了 —— 每段思考后的小输出就没人要求了"
        );
        assert!(
            PERSONA_AND_STYLE.contains("more than one tool round"),
            "「阶段小结」的范围没说清（多轮工具任务），模型可能给单步问答也加"
        );
        assert!(
            PERSONA_AND_STYLE.contains("A single-step answer emits none of this"),
            "「单步问答不加」这条丢了 —— 会把 L6 的降本成果吃回去"
        );
        // L6 主体不得被这次改动顺手删掉（例外是加在它之后的，不是替代它）。
        assert!(
            PERSONA_AND_STYLE.contains("no preamble before tool calls"),
            "L6 的输出纪律主体丢了"
        );
    }

    /// 「重要判断要看得见」与「提问前先给一段上下文」两条（2026-11-03，§待办 1-7 / 1-10）。
    ///
    /// 它们都要求**进可见正文**（不是 thinking），因为思考块默认收起 —— 丢了就等于回到
    /// 「用户只看到一个孤立的提问卡 / 一段沉默的继续」。
    #[test]
    fn visible_decision_and_pre_question_summary_rules_are_present() {
        assert!(
            PERSONA_AND_STYLE.contains("## Visible decisions"),
            "「重要判断必须写进可见正文」这条丢了"
        );
        assert!(
            PERSONA_AND_STYLE.contains("Never leave it only in thinking"),
            "没说清「不能只放在 thinking 里」—— 思考块是收起的，用户看不到"
        );
        assert!(
            PERSONA_AND_STYLE.contains("## Asking the user"),
            "「提问前先给一段上下文」这条丢了"
        );
        assert!(
            PERSONA_AND_STYLE.contains("Before calling AskUserQuestion"),
            "没写明触发点（调用 AskUserQuestion 之前）"
        );
        // 与「一口气做完」叠加：不得因此停下来等确认。
        assert!(
            PERSONA_AND_STYLE.contains("never repeat the summary"),
            "没写「后续不得重复这段总结」—— 会变成同一段话说两遍"
        );
    }

    /// 「回答可读性」硬规则（2026-11-05，§待办 1-19 第 6 条）：
    /// 结论先行、重点才加粗、长细节走代码块、只问一个问题、数字要带验证口径、
    /// 没证据不许写「已验证」。
    ///
    /// 这些是**内容侧**的约束；前端渲染（加粗 / 代码块）由 §待办 1-19 的另一半负责。
    #[test]
    fn readable_answer_rules_are_present() {
        for needle in [
            "## Readable answers",
            "first line the conclusion",
            "Use **bold** for the few words that change a decision",
            "A detail block longer than about 15 lines",
            "end with exactly one question",
            "state how it was verified",
            "say it is an inference",
        ] {
            assert!(
                PERSONA_AND_STYLE.contains(needle),
                "可读性规则缺了一句：{needle}"
            );
        }
    }

    /// 「取证纪律」（2026-11-05，§待办 1-16 第 2/4/5/6/7 条）：
    /// 数字带口径、一手来源优先、坐标映射先验证、视觉结论要实机证据、
    /// 外部托管文件先查进程、重复扫描要落盘复用。
    #[test]
    fn evidence_discipline_rules_are_present() {
        for needle in [
            "## Evidence discipline",
            "rerun twice with identical output",
            "never leave a hidden magic cap",
            "read the executable strings",
            "top of panel = last line of file",
            "report candidate causes only",
            "check the process and file lock",
            "never rescan the same tree every round",
            "temp\\transStorage",
        ] {
            assert!(
                PERSONA_AND_STYLE.contains(needle),
                "取证纪律缺了一句：{needle}"
            );
        }
    }

    /// 「长期记忆」**不在 system 里、只在上下文块里**（2026-10-06，M2-16 方案 D）。
    ///
    /// 两个方向都要钉住：
    /// ① **反面** —— 它不在 `build_system_prompt` 的产物里（否则 system 随记忆增长而变，
    ///    每次重启后端点缓存从 0 起全 miss，正是要修的病灶）；
    /// ② **正面** —— `context_block_message` 把它合成**一条 `user` 消息**、且表头可被
    ///    `is_context_block_msg` 认出（幂等注入的判据）。空 ⇒ `None`（不加空壳）。
    ///
    /// **2026-10-07**：往期会话索引不再注入（新对话与上个对话模型侧隔离），本块只剩记忆。
    #[test]
    fn memory_lives_in_the_context_block_not_the_system_prompt() {
        let env = env_block(std::path::Path::new("C:/work"));
        let system = build_system_prompt("", &env, "");
        assert!(
            !system.contains(CONTEXT_BLOCK_HEADER),
            "上下文块表头不该出现在 system 里"
        );

        // 空记忆 ⇒ 没有上下文块（不凭空多一条消息）
        assert!(context_block_message("   ").is_none(), "空记忆应返回 None");

        let cb = context_block_message("LONG-MEM").expect("非空时应生成块");
        assert_eq!(cb.get("role").and_then(Value::as_str), Some("user"), "必须是 user 消息");
        let text = cb
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .expect("应有 text 块");
        assert!(text.starts_with(CONTEXT_BLOCK_HEADER), "必须以表头开头（幂等判据）");
        assert!(text.contains("LONG-MEM"), "记忆正文在");
        assert!(is_context_block_msg(&cb), "自产的块必须能被幂等判据认出");

        // 普通用户消息不能被误判成上下文块（否则幂等逻辑会漏注）
        let normal = json!({ "role": "user", "content": [{ "type": "text", "text": "你好" }] });
        assert!(!is_context_block_msg(&normal), "普通用户消息不该被认成上下文块");
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

    /// 项目记忆（AGENTS.md）：逐字进提示词、三份提示词同源、空文件零字节。
    #[test]
    fn agents_md_reaches_all_three_prompts_verbatim() {
        let body = "# 项目约定\n- 构建：npm run build\n- 保留 {braces} 字面量";
        let block =
            project_block_from_chain(&[(PathBuf::from("C:/work/AGENTS.md"), body.to_string())]);
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

    /// 没有 AGENTS.md / 文件全空白 = 完全不存在：不加表头、不加换行。
    /// 否则每个没有这个文件的目录都会凭空多一段**每轮都要发**的空壳。
    #[test]
    fn blank_agents_md_adds_nothing_to_the_prompt() {
        assert_eq!(project_block_from_chain(&[]), "");
        assert_eq!(
            project_block_from_chain(&[(PathBuf::from("x/AGENTS.md"), "  \n\t  ".to_string())]),
            ""
        );
    }

    /// 单份 AGENTS.md 超长必须截断 —— 它进的是每次请求都要发的固定前缀。
    #[test]
    fn oversized_agents_md_is_truncated() {
        let long = "x".repeat(MAX_AGENTS_MD_CHARS + 500);
        let block = project_block_from_chain(&[(PathBuf::from("x/AGENTS.md"), long)]);
        let body = block.split(AGENTS_MD_HEADER).nth(1).unwrap().trim_start_matches('\n');
        assert_eq!(body.chars().count(), MAX_AGENTS_MD_CHARS, "必须截到上限");
    }

    /// 多层 / 向上递归（2026-10-11）：工作目录 → 盘根 链路上每一级的 AGENTS.md 都收到，
    /// 顺序**浅→深**（越深越具体、排在越后）；并证明同一状态下逐字节可复现（前缀缓存纪律）。
    #[test]
    fn multi_level_agents_md_is_chained_shallow_to_deep_and_stable() {
        let root = std::env::temp_dir().join(format!("lunac-agents-multi-{}", std::process::id()));
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(root.join(AGENTS_MD_FILE), "ROOT-RULE").unwrap();
        std::fs::write(sub.join(AGENTS_MD_FILE), "SUB-RULE").unwrap();

        let files = collect_agents_md(&sub);
        assert_eq!(
            files.len(),
            2,
            "两级都应收到：{:?}",
            files.iter().map(|(p, _)| p).collect::<Vec<_>>()
        );
        assert_eq!(files[0].1.trim(), "ROOT-RULE", "先浅（父级）");
        assert_eq!(files[1].1.trim(), "SUB-RULE", "后深（本级）");

        let block = project_block(&sub);
        let ri = block.find("ROOT-RULE").expect("父级规则在");
        let si = block.find("SUB-RULE").expect("本级规则在");
        assert!(ri < si, "越深越具体 ⇒ 排在越后");
        // 同一状态下两次调用逐字节一致（不改文件、不重扫 ⇒ 固定前缀不抖）
        assert_eq!(block, project_block(&sub), "同一状态下前缀必须逐字节相同");
        std::fs::remove_dir_all(&root).ok();
    }

    /// 总量上限：多份叠加超过总预算时丢**最浅**的，保留更贴近工作目录的那份。
    #[test]
    fn agents_md_chain_drops_shallowest_when_over_total_budget() {
        // 两份各顶到单文件上限：8000 + 8000 = 16000 = 总上限，两份都要
        let shallow = "a".repeat(MAX_AGENTS_MD_CHARS);
        let deep = "b".repeat(MAX_AGENTS_MD_CHARS);
        let two = project_block_from_chain(&[
            (PathBuf::from("C:/p/AGENTS.md"), shallow.clone()),
            (PathBuf::from("C:/p/q/AGENTS.md"), deep.clone()),
        ]);
        assert!(two.contains(&shallow) && two.contains(&deep), "刚好到上限时两份都留");
        // 再加一份就会超：最浅的那份被丢掉，更深的两份保留
        let deeper = "c".repeat(MAX_AGENTS_MD_CHARS);
        let three = project_block_from_chain(&[
            (PathBuf::from("C:/p/AGENTS.md"), shallow.clone()),
            (PathBuf::from("C:/p/q/AGENTS.md"), deep.clone()),
            (PathBuf::from("C:/p/q/r/AGENTS.md"), deeper.clone()),
        ]);
        assert!(!three.contains(&shallow), "最浅的那份应被丢掉");
        assert!(three.contains(&deep) && three.contains(&deeper), "更深的应保留");
    }

    // ── 子目录记忆懒加载：注入层单测（2026-10-13，§待办 3-1a 子目录级）──────────
    //
    // 判据层（`touched_dir` / `eligible_subdir`）已在 `tools.rs` 单测覆盖；这里测**注入层**
    // ——`subdir_memory_block` / `subdir_memory_injected` / `subdir_memory_count` /
    // `append_text_to_tool_result` / `inject_subdir_memory` 的落点与去重。全部用临时目录夹具。

    /// 造一个「Read 一条子目录里的文件」调用（触达目录 = 该文件的父目录）。
    fn read_call(file: &Path) -> (String, String, Value) {
        (
            "t1".to_string(),
            "Read".to_string(),
            json!({ "file_path": file.display().to_string() }),
        )
    }

    /// 单一子目录命中 ⇒ 注入进**对应那条** `tool_result`，带表头与来源行 `### <绝对路径>`；
    /// 原结果文本保留在前、未新增消息 / 块（固定前缀纪律）。
    #[test]
    fn subdir_memory_hit_is_injected_into_its_tool_result() {
        let root = std::env::temp_dir().join(format!("lunac-subdir-hit-{}", std::process::id()));
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(AGENTS_MD_FILE), "SUB-RULE").unwrap();

        let calls = vec![read_call(&sub.join("a.txt"))];
        let run_inputs = vec![calls[0].2.clone()];
        let denieds = vec![None];
        let mut results = vec![tool_result_block("t1", "ORIGINAL".into(), false)];

        inject_subdir_memory(&root, &[], &calls, &run_inputs, &denieds, &mut results);

        let content = results[0].get("content").and_then(Value::as_str).unwrap();
        assert!(content.starts_with("ORIGINAL"), "原结果文本必须保留在前：{content}");
        assert!(content.contains(SUBDIR_MEMORY_HEADER), "要带子目录记忆表头：{content}");
        let marker = format!("### {}", sub.join(AGENTS_MD_FILE).display());
        assert!(content.contains(&marker), "要带来源行 {marker}：{content}");
        assert!(content.contains("SUB-RULE"), "要带上该目录的 AGENTS.md 正文");
        assert!(
            content.find(SUBDIR_MEMORY_HEADER).unwrap() < content.find(&marker).unwrap(),
            "表头应在来源行之前"
        );
        // 未新增消息 / 块，content 仍是字符串（图块数组那种才不拼）。
        assert_eq!(results.len(), 1);
        assert!(results[0].get("content").unwrap().is_string());

        std::fs::remove_dir_all(&root).ok();
    }

    /// 同一目录每会话只注一次；且 `set_history` 回退（history 换成不含来源行的版本）后能补回
    /// ——与 `context_block_message` 同一条纪律（去重判据活在 history 里，回退即随之消失）。
    #[test]
    fn subdir_memory_is_injected_once_per_session_and_restored_after_set_history() {
        let root = std::env::temp_dir().join(format!("lunac-subdir-dedup-{}", std::process::id()));
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(AGENTS_MD_FILE), "SUB-RULE").unwrap();

        let calls = vec![read_call(&sub.join("a.txt"))];
        let run_inputs = vec![calls[0].2.clone()];
        let denieds = vec![None];

        // 第一次：注入，并把这条带来源行的结果当作「已留在 history 里」。
        let mut first = vec![tool_result_block("t1", "ORIGINAL".into(), false)];
        inject_subdir_memory(&root, &[], &calls, &run_inputs, &denieds, &mut first);
        assert!(first[0]["content"].as_str().unwrap().contains("SUB-RULE"));
        let history = vec![json!({ "role": "user", "content": [ first[0].clone() ] })];

        // 第二次（同会话）：history 已含该目录来源行 ⇒ 不再注入。
        let mut again = vec![tool_result_block("t2", "ORIGINAL2".into(), false)];
        inject_subdir_memory(&root, &history, &calls, &run_inputs, &denieds, &mut again);
        assert!(
            !again[0]["content"].as_str().unwrap().contains("SUB-RULE"),
            "同目录每会话只注一次"
        );

        // set_history 回退：history 换成不含该来源行的版本 ⇒ 下次触碰能补回。
        let rolled_back = vec![json!({ "role": "user", "content": [
            { "type": "text", "text": "hi" }
        ] })];
        let mut restored = vec![tool_result_block("t3", "ORIGINAL3".into(), false)];
        inject_subdir_memory(&root, &rolled_back, &calls, &run_inputs, &denieds, &mut restored);
        assert!(
            restored[0]["content"].as_str().unwrap().contains("SUB-RULE"),
            "回退后应能补回"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// 已达每会话目录数上限 ⇒ 停注（`inject_subdir_memory` 同时落一行 warn；本用例只管
    /// 「不注入」这个可断言的结果，warn 是日志副作用、不在单测断言范围）。
    #[test]
    fn subdir_memory_stops_at_session_dir_cap() {
        let root = std::env::temp_dir().join(format!("lunac-subdir-cap-{}", std::process::id()));
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(AGENTS_MD_FILE), "SUB-RULE").unwrap();

        let calls = vec![read_call(&sub.join("a.txt"))];
        let run_inputs = vec![calls[0].2.clone()];
        let denieds = vec![None];

        // 造 `MAX_SUBDIR_MEMORY_DIRS` 个「已注入过」的历史块（计数判据 = 表头出现次数）。
        let history: Vec<Value> = (0..MAX_SUBDIR_MEMORY_DIRS)
            .map(|i| {
                json!({ "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": format!("old{i}"),
                      "content": format!("{SUBDIR_MEMORY_HEADER}\n### fake/{i}") },
                ]})
            })
            .collect();
        assert_eq!(subdir_memory_count(&history), MAX_SUBDIR_MEMORY_DIRS);

        let mut results = vec![tool_result_block("t1", "ORIGINAL".into(), false)];
        inject_subdir_memory(&root, &history, &calls, &run_inputs, &denieds, &mut results);
        assert_eq!(
            results[0]["content"].as_str().unwrap(),
            "ORIGINAL",
            "达每会话上限后不得再注入"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// 不合格目录（超深度 / 在 `SKIP_DIRS` / 等于 cwd）一律不注 —— 注入层必须过
    /// `tools::eligible_subdir` 这道筛。每例都真放了 AGENTS.md，确保「没注入」是筛掉了、
    /// 不是「文件不存在」。
    #[test]
    fn subdir_memory_skips_ineligible_dirs() {
        let root = std::env::temp_dir().join(format!("lunac-subdir-skip-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();

        // 超深度：cwd 下 9 层（> MAX_SUBDIR_MEMORY_DEPTH = 8）
        let mut deep = root.clone();
        for i in 0..9 {
            deep = deep.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join(AGENTS_MD_FILE), "DEEP-RULE").unwrap();

        // 在忽略名单里（`node_modules` 之下的子目录也一并跳过）
        let skipped = root.join("node_modules").join("pkg");
        std::fs::create_dir_all(&skipped).unwrap();
        std::fs::write(skipped.join(AGENTS_MD_FILE), "SKIP-RULE").unwrap();

        // 等于 cwd 本身（那是固定前缀那一层，不归懒加载管）
        std::fs::write(root.join(AGENTS_MD_FILE), "CWD-RULE").unwrap();

        let cases = [
            ("超深度", deep.join("a.txt"), "DEEP-RULE"),
            ("SKIP_DIRS", skipped.join("a.txt"), "SKIP-RULE"),
            ("等于 cwd", root.join("a.txt"), "CWD-RULE"),
        ];
        for (label, file, marker) in cases {
            let calls = vec![read_call(&file)];
            let run_inputs = vec![calls[0].2.clone()];
            let denieds = vec![None];
            let mut results = vec![tool_result_block("t1", "ORIGINAL".into(), false)];
            inject_subdir_memory(&root, &[], &calls, &run_inputs, &denieds, &mut results);
            assert_eq!(results[0]["content"].as_str().unwrap(), "ORIGINAL", "{label} 不应注入");
            assert!(
                !results[0]["content"].as_str().unwrap().contains(marker),
                "{label} 不应注入"
            );
        }

        std::fs::remove_dir_all(&root).ok();
    }

    /// 子目录注入与后台结果回流**共存在同一块** `tool_result` 文本里：现顺序是子目录先
    /// （`inject_subdir_memory`）、后台结果后（`run_query` 里 `results.last_mut()` 三行内联追加）。
    /// 后台那段落点是内联在 `run_query` 里的、不是可单测的纯函数 ⇒ 这里复刻它那三行追加，
    /// 验证「同一块字符串上两次追加互不覆盖、顺序不串」（能测的最小子集，未改生产代码）。
    #[test]
    fn subdir_memory_and_background_result_share_one_tool_result() {
        let root = std::env::temp_dir().join(format!("lunac-subdir-bg-{}", std::process::id()));
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(AGENTS_MD_FILE), "SUB-RULE").unwrap();

        let calls = vec![read_call(&sub.join("a.txt"))];
        let run_inputs = vec![calls[0].2.clone()];
        let denieds = vec![None];
        let mut results = vec![tool_result_block("t1", "ORIGINAL".into(), false)];

        inject_subdir_memory(&root, &[], &calls, &run_inputs, &denieds, &mut results);
        // 复刻 run_query 的后台回流落点（拼进最后一条 tool_result 的文本内部）。
        let bg = "[background] BUILD DONE";
        let last = results.last_mut().expect("非空");
        let prev = last.get("content").and_then(Value::as_str).unwrap().to_string();
        last["content"] = json!(format!("{prev}\n\n{bg}"));

        let content = results[0]["content"].as_str().unwrap();
        assert!(content.contains("SUB-RULE"), "子目录块仍在：{content}");
        assert!(content.contains(bg), "后台结果也在同一块：{content}");
        assert!(
            content.find("SUB-RULE").unwrap() < content.find(bg).unwrap(),
            "顺序须是子目录先、后台结果后"
        );
        assert_eq!(results.len(), 1, "仍是同一块，未新增消息 / 块");

        std::fs::remove_dir_all(&root).ok();
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
        for wt_tool in ["EnterWorktree", "ExitWorktree"] {
            assert!(
                !names.contains(&wt_tool.to_string()),
                "{wt_tool} 改的是主循环的有效工作目录（跨线程共享），子代理改它会把主代理的解析带走"
            );
        }
        for fe_tool in ["SendUserFile", "PushNotification"] {
            assert!(
                !names.contains(&fe_tool.to_string()),
                "{fe_tool} 的效果是主进程前端的卡片 / 通知，子代理里调用只会冒出一张无上下文的卡"
            );
        }
        assert!(
            !names.contains(&"CronList".to_string())
                && !names.contains(&"CronCreate".to_string())
                && !names.contains(&"CronDelete".to_string()),
            "定时任务三件是用户级持续成本配置，子代理不接"
        );
        for task_tool in ["TaskEnqueue", "TaskList", "TaskCancel"] {
            assert!(
                !names.contains(&task_tool.to_string()),
                "{task_tool} 是用户级后台任务配置，子代理里不读也不入队"
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
                !matches!(
                    n.as_str(),
                    "Agent"
                        | "EnterPlanMode"
                        | "ExitPlanMode"
                        | "PetControl"
                        | "EnterWorktree"
                        | "ExitWorktree"
                        | "SendUserFile"
                        | "PushNotification"
                        | "CronList"
                        | "CronCreate"
                        | "CronDelete"
                        | "TaskEnqueue"
                        | "TaskList"
                        | "TaskCancel"
                )
                    && !tools::needs_bridge(n)
                    && !n.starts_with("mcp__")
            })
            .collect();
        assert_eq!(names, expected, "剔除不得改变工具顺序");
    }

    /// 角色裁工具（2026-10-24）：在 `subagent_tool_defs()` 之后执行，`general` 不裁。
    #[test]
    fn subagent_role_defs_narrow_tool_pool_per_role() {
        let mut defs = tools::defs(&[]);
        defs.push(skills::tool_def());
        let base = subagent_tool_defs(&defs);

        let names = |role: super::subagent_role::Role| -> Vec<String> {
            subagent_role_defs(&base, role)
                .iter()
                .filter_map(|d| d.get("name").and_then(Value::as_str).map(String::from))
                .collect()
        };

        assert_eq!(names(super::subagent_role::Role::General), {
            base.iter()
                .filter_map(|d| d.get("name").and_then(Value::as_str).map(String::from))
                .collect::<Vec<_>>()
        }, "general 必须逐字节保持旧工具池");

        let explore = names(super::subagent_role::Role::Explore);
        assert!(explore.contains(&"Read".into()));
        assert!(!explore.contains(&"Write".into()) && !explore.contains(&"Cmd".into()));

        let coding = names(super::subagent_role::Role::Coding);
        assert!(coding.contains(&"Write".into()) && coding.contains(&"PowerShell".into()));
        assert!(!coding.contains(&"WebFetch".into()));

        let review = names(super::subagent_role::Role::Review);
        assert!(review.contains(&"Cmd".into()));
        assert!(!review.contains(&"Write".into()), "review 不许改文件");

        let research = names(super::subagent_role::Role::Research);
        assert!(research.contains(&"WebSearch".into()));
        assert!(!research.contains(&"Write".into()) && !research.contains(&"Cmd".into()));
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

    /// 复盘触发（A4，2026-10-11 方案 B：按会话 + 按时间）。四类边界：
    /// 写侧不可用（enabled=false）永不触发；本会话已复盘过（≤ 1 次）不再触发；
    /// 距上次不到 30 分钟不触发、到点触发；从未复盘过（None）且写侧可用则触发；
    /// 时钟回拨（last_ts 在未来）按「还没到时间」处理。
    #[test]
    fn review_gate_is_per_session_and_time_capped() {
        use tools::MemoryScope;
        let gap = REVIEW_MIN_GAP_SECS;
        let now = 1_700_000_000i64;
        let none = [false, false];
        // 正常触发：写侧可用、本会话没复盘过、从未复盘 or 已过间隔。
        assert!(
            should_review(true, none, None, now, MemoryScope::Global),
            "从未复盘 + 本会话未复盘 ⇒ 触发"
        );
        assert!(
            should_review(true, none, Some(now - gap), now, MemoryScope::Global),
            "恰好满 30 分钟 ⇒ 触发"
        );
        // 不到间隔 ⇒ 不触发（差 1 秒也不行）。
        assert!(
            !should_review(true, none, Some(now - gap + 1), now, MemoryScope::Global),
            "差 1 秒不满间隔 ⇒ 不触发"
        );
        // 每会话 ≤ 1 次 ⇒ 本会话已复盘过一律不触发（即便过了很久）。
        assert!(
            !should_review(true, [true, false], Some(now - gap * 10), now, MemoryScope::Global),
            "本会话已复盘过 ⇒ 不再触发"
        );
        // 写侧不可用（没桥 / 记忆关 / Remember 被裁）⇒ 永不触发。
        assert!(
            !should_review(false, none, None, now, MemoryScope::Global),
            "写侧不可用 ⇒ 不触发"
        );
        // 时钟回拨（last_ts 在未来）按「没到时间」处理，不狂调模型。
        assert!(
            !should_review(true, none, Some(now + 3600), now, MemoryScope::Global),
            "时钟回拨 ⇒ 当作没到时间"
        );
    }

    /// 复盘配额按作用域拆（2026-10-13 折中）：**每会话每 scope ≤ 1 次**、**30 分钟时间闸共享**。
    /// 关键差异：一个作用域复盘过，不再压制另一个作用域的「会话配额」——但它仍受共享时间闸约束。
    #[test]
    fn review_gate_splits_session_quota_by_scope_and_shares_time_gate() {
        use tools::MemoryScope;
        let gap = REVIEW_MIN_GAP_SECS;
        let now = 1_700_000_000i64;

        // 时间闸已放行（None = 从未复盘）时，两侧配额独立：
        // · 全新会话 ⇒ global / pet 各自都能触发一次
        assert!(should_review(true, [false, false], None, now, MemoryScope::Global));
        assert!(should_review(true, [false, false], None, now, MemoryScope::Pet));
        // · global 已复盘 ⇒ global 不再触发，pet 首启**仍可**触发（旧实现会一起压制）
        assert!(
            !should_review(true, [true, false], None, now, MemoryScope::Global),
            "global 已复盘 ⇒ global 本会话不再触发"
        );
        assert!(
            should_review(true, [true, false], None, now, MemoryScope::Pet),
            "global 已复盘不得压制 pet 的会话配额"
        );
        // · 反向亦然
        assert!(
            !should_review(true, [false, true], None, now, MemoryScope::Pet),
            "pet 已复盘 ⇒ pet 本会话不再触发"
        );
        assert!(
            should_review(true, [false, true], None, now, MemoryScope::Global),
            "pet 已复盘不得压制 global 的会话配额"
        );

        // 30 分钟时间闸**两侧共享**：任一作用域刚复盘过（last_ts = now），另一侧即便会话配额
        // 未消耗，也在 30 分钟内不跑（防总成本翻倍）。
        assert!(!should_review(true, [true, false], Some(now), now, MemoryScope::Pet));
        assert!(!should_review(true, [false, true], Some(now), now, MemoryScope::Global));
        // 满 30 分钟后恢复：pet 那一份未消耗 ⇒ 可触发；差 1 秒仍被共享闸挡住。
        assert!(should_review(true, [true, false], Some(now - gap), now, MemoryScope::Pet));
        assert!(!should_review(true, [true, false], Some(now - gap + 1), now, MemoryScope::Pet));
    }

    /// 长期记忆的冷启动死锁回归（方案 D，2026-10-11）：记忆为空但写侧可用时**仍要注入
    /// Remember 引导**（否则全新安装永远学不会主动写）。引导是固定前缀、正文拼在其后。
    #[test]
    fn memory_block_injects_guidance_when_memory_is_empty() {
        use tools::MemoryScope;
        // 空记忆 + 写侧可用 ⇒ 注入表头 + 引导（没有正文）。
        let empty = memory_block_text(true, "", MemoryScope::Global);
        assert!(empty.contains("# Long-term memory"), "空记忆也要注入表头");
        assert!(
            empty.contains("Add a fact with the `Remember` tool"),
            "空记忆也要注入 Remember 引导（冷启动死锁的回归防线）"
        );
        // 空记忆 + 写侧不可用 ⇒ 无可注入（不拿一句空话污染前缀）。
        assert!(
            memory_block_text(false, "", MemoryScope::Global).is_empty(),
            "空记忆 + 写侧不可用 ⇒ 空串"
        );
        // 有正文时行为不变，且正文必须落在引导**之后**（缓存前缀纪律：前缀逐字节稳定）。
        let full = memory_block_text(true, "- prefers tabs", MemoryScope::Global);
        assert!(full.contains("Add a fact with the `Remember` tool"));
        assert!(full.ends_with("- prefers tabs"), "正文必须拼在引导之后");
        // 逐字节稳定性：空记忆的块必须是「有正文的块」的前缀（引导段一字不差）。
        assert!(
            full.starts_with(&empty),
            "空记忆的块必须是有记忆块的前缀（引导段不随正文变）"
        );
    }

    /// 记忆注入 A/B 开关（2026-11-21）：`full` 默认、逐字节兼容；`index` 只注入
    /// 「页名 + 首行摘要」目录并提示用 `MemorySearch` 取正文。
    #[test]
    fn memory_mode_switch_and_index_directory() {
        use tools::MemoryScope;
        // 解析：缺省 / 空 / full / 非法值都落回 full（笔误不能悄悄改线上行为）。
        assert_eq!(parse_memory_mode(None), MemoryMode::Full);
        assert_eq!(parse_memory_mode(Some("")), MemoryMode::Full);
        assert_eq!(parse_memory_mode(Some("full")), MemoryMode::Full);
        assert_eq!(parse_memory_mode(Some("INDEX")), MemoryMode::Index);
        assert_eq!(parse_memory_mode(Some(" index ")), MemoryMode::Index);
        assert_eq!(parse_memory_mode(Some("weird")), MemoryMode::Full);

        // 目录：保留页名与每条首行，丢掉续行 / refs；条目文字不改。
        let body = "- [2026-11-20 · global] 构建用 cargo\n## build\n- 先跑 flutter test\n- 记忆额度 24000\n→ refs: other\n";
        let index = memory_index_text(body);
        assert!(index.contains("- [2026-11-20 · global] 构建用 cargo"));
        assert!(index.contains("## build"));
        assert!(index.contains("- 先跑 flutter test"));
        assert!(!index.contains("→ refs:"), "refs 行不进目录");

        // 长条目压成首行摘要 ⇒ 目录远小于正文。
        let long = format!("- {}\n", "x".repeat(500));
        let short = memory_index_text(&long);
        assert!(short.chars().count() < 200, "长条目必须被截短：{short}");
        assert!(short.contains('…'));

        // 预算：超限时停止并标注还有多少条未展开。
        let many = (0..300)
            .map(|n| format!("- entry-{n}\n"))
            .collect::<String>();
        let capped = memory_index_text(&many);
        assert!(
            capped.chars().count() <= MEMORY_INDEX_CHARS + 200,
            "目录必须有硬上限：{} 字",
            capped.chars().count()
        );
        assert!(capped.contains("more entries are not shown"), "{capped}");

        // index 注入段：MemorySearch 说明 + 目录；full 段不得出现该说明（缓存逐字节兼容）。
        let full_block = memory_block_text_mode(MemoryMode::Full, true, &index, MemoryScope::Global);
        let idx_block = memory_block_text_mode(MemoryMode::Index, true, &index, MemoryScope::Global);
        assert!(idx_block.contains("call the `MemorySearch` tool"), "{idx_block}");
        assert!(!full_block.contains("MemorySearch"), "{full_block}");
        assert!(idx_block.ends_with(&index), "目录必须拼在引导之后");
        let empty = memory_block_text_mode(MemoryMode::Index, true, "", MemoryScope::Global);
        assert!(
            idx_block.starts_with(&empty),
            "空目录的块必须是有目录块的前缀（固定前缀纪律）"
        );
        // 空记忆 + 写侧可用：index 模式仍要有 Remember 冷启动引导。
        assert!(empty.contains("Add a fact with the `Remember` tool"), "{empty}");
    }

    /// A/B 的体积侧回归：写满额度（约 24k 字）的真实形状语料，index 注入必须显著小于 full。
    ///
    /// 这条测试只证明「上下文占用」这一半；答对率仍要真实会话（见 docs/todo.md §6-3）。
    #[test]
    fn index_mode_shrinks_a_full_memory_block() {
        use tools::MemoryScope;
        let mut body = String::new();
        for page in 0..6 {
            body.push_str(&format!("## page-{page}\n"));
            for n in 0..10 {
                body.push_str(&format!(
                    "- [2026-11-20 · global] 条目 {}：{}\n",
                    page * 10 + n,
                    "记忆内容".repeat(85)
                ));
            }
        }
        let full_body = cap_inject(&body);
        let index_body = memory_index_text(&body);
        let full_block =
            memory_block_text_mode(MemoryMode::Full, true, &full_body, MemoryScope::Global);
        let index_block =
            memory_block_text_mode(MemoryMode::Index, true, &index_body, MemoryScope::Global);
        let full_chars = full_block.chars().count();
        let index_chars = index_block.chars().count();
        eprintln!(
            "A/B 注入体积（合成语料）：full={full_chars} 字，index={index_chars} 字（目录 {} 字）",
            index_body.chars().count()
        );
        assert!(
            index_body.chars().count() <= MEMORY_INDEX_CHARS + 200,
            "目录必须有硬上限：{} 字",
            index_body.chars().count()
        );
        assert!(
            index_chars * 5 < full_chars,
            "index 至少要压到 full 的 1/5：full={full_chars} index={index_chars}"
        );
    }

    /// 作用域（2026-10-13）：全局与桌宠各一段、表头可区分（互不掺入）；本轮作用域由
    /// user 消息的 `source` 推出，**只认 `pet`**，其余（含缺省 / 坏值）一律全局。
    #[test]
    fn memory_scope_is_selected_by_turn_source() {
        use tools::MemoryScope;
        // 表头区分：全局段不含桌宠后缀；桌宠段带上私有说明。
        let g = memory_block_text(true, "- a", MemoryScope::Global);
        let p = memory_block_text(true, "- a", MemoryScope::Pet);
        assert!(g.contains("# Long-term memory\n"), "{g}");
        assert!(
            p.contains("desktop pet — private notes"),
            "桌宠段要有可区分的表头：{p}"
        );
        assert!(!g.contains("private notes"), "全局段不得混入桌宠措辞");

        // source 解析：只认 `pet`（大小写不敏感），其余一律全局。
        assert_eq!(
            turn_memory_scope(&json!({ "type": "user", "source": "pet" })),
            MemoryScope::Pet
        );
        assert_eq!(
            turn_memory_scope(&json!({ "type": "user", "source": "PET" })),
            MemoryScope::Pet
        );
        assert_eq!(
            turn_memory_scope(&json!({ "type": "user", "thinking": "off" })),
            MemoryScope::Global,
            "缺省 source ⇒ 全局"
        );
        assert_eq!(
            turn_memory_scope(&json!({ "type": "user", "source": "chat" })),
            MemoryScope::Global,
            "无法识别 ⇒ 退回全局（绝不误判成桌宠轮）"
        );
    }

    /// 记忆块的「来源 / 时间」说明行（B2，2026-10-13）：属**固定前缀**（同 scope 逐字节稳定）、
    /// 位于正文之前（正文仍拼在其后）、按 scope 措辞点明归属（桌宠段点明「桌宠私有」）。
    #[test]
    fn memory_block_provenance_line_is_byte_stable_and_precedes_body() {
        use tools::MemoryScope;
        // 同 scope 两次调用完全相等（固定前缀不变量：不随轮次 / 内容变化）。
        let a = memory_block_text(true, "- prefers tabs", MemoryScope::Global);
        let b = memory_block_text(true, "- prefers tabs", MemoryScope::Global);
        assert_eq!(a, b, "同 scope 两次调用必须逐字节相同（前缀缓存不变量）");
        // 说明行在正文之前：空记忆块是有记忆块的前缀（正文不混进前缀）。
        let empty = memory_block_text(true, "", MemoryScope::Global);
        assert!(a.starts_with(&empty), "正文必须拼在说明行之后");
        assert!(a.ends_with("- prefers tabs"), "正文仍在最末，bullet 格式未变");
        // 全局段点明「主 AI 保存」；桌宠段点明「桌宠私有」；两段说明行不同。
        assert!(a.contains("main assistant"), "全局段要说明来源：{a}");
        assert!(!a.contains("desktop pet"), "全局段不得混入桌宠措辞");
        let p = memory_block_text(true, "- prefers tabs", MemoryScope::Pet);
        assert!(p.contains("desktop pet's own private memory"), "桌宠段要点明私有：{p}");
        assert_ne!(a, p, "两侧说明行不同");
        // 说明行描述 B1 的新格式（日期 = 最后确认）并交代「无前缀 = 更早的背景」。
        assert!(a.contains("[YYYY-MM-DD · global]"), "全局段要写清前缀格式：{a}");
        assert!(p.contains("[YYYY-MM-DD · pet]"), "桌宠段要写清前缀 scope：{p}");
        assert!(a.contains("last confirmed"), "要交代日期 = 最后确认时间");
        assert!(a.contains("undated"), "要交代无前缀者视为更早的背景");
    }

    /// 压缩边界刷新（§待办 3-9 C）：新块与旧块不同 ⇒ 写回快照 + **原地**替换 history 开头
    /// （长度不变，回滚锚点 `base` 不受影响）。
    #[test]
    fn snapshot_refresh_replaces_head_block_in_place() {
        let old = context_block_message("- OLD-MEM").expect("非空记忆应生成块");
        let mut history = vec![
            old.clone(),
            json!({ "role": "user", "content": [{ "type": "text", "text": "hi" }] }),
        ];
        let mut snapshot = Some(old);
        let fresh = context_block_message("- NEW-MEM");
        assert!(apply_refreshed_snapshot(&mut history, &mut snapshot, fresh));

        assert_eq!(history.len(), 2, "原地替换，不改变消息条数（锚点 base 不动）");
        assert!(is_context_block_msg(&history[0]), "开头仍是可识别的上下文块");
        let expected = format!("{CONTEXT_BLOCK_HEADER}\n- NEW-MEM");
        assert_eq!(
            history[0].pointer("/content/0/text").and_then(Value::as_str),
            Some(expected.as_str()),
            "开头块的正文必须换成新记忆"
        );
        assert_eq!(snapshot.as_ref(), Some(&history[0]), "快照要写回（下一轮不能退回旧块）");
    }

    /// 刷新拿到空块（记忆被清空 / 桥临时不可用）⇒ **保留旧块**：删块会换掉 history 第一条、
    /// 还让本会话少一份已注入的背景。
    #[test]
    fn snapshot_refresh_keeps_old_block_when_memory_is_empty() {
        let old = context_block_message("- OLD-MEM").expect("非空记忆应生成块");
        let mut history = vec![old.clone()];
        let mut snapshot = Some(old.clone());
        assert!(!apply_refreshed_snapshot(&mut history, &mut snapshot, None));
        assert_eq!(history, vec![old.clone()], "历史逐字节不变");
        assert_eq!(snapshot, Some(old), "快照保持不变");
    }

    /// 内容没变（本次压缩与上次刷新之间没有新记忆）⇒ 什么都不做，不制造无意义的前缀变更。
    #[test]
    fn snapshot_refresh_is_a_noop_when_content_is_unchanged() {
        let old = context_block_message("- SAME-MEM").expect("非空记忆应生成块");
        let mut history = vec![old.clone()];
        let mut snapshot = Some(old);
        assert!(!apply_refreshed_snapshot(
            &mut history,
            &mut snapshot,
            context_block_message("- SAME-MEM")
        ));
        assert_eq!(history.len(), 1);
    }

    /// 本来就没有上下文块（记忆被关 / 桥不在）⇒ 不凭空造一个：那等于改注入策略。
    /// 这里故意传 `bridge=None` —— 提前返回也保证**不去读记忆**。
    #[test]
    fn snapshot_refresh_does_nothing_without_an_existing_block() {
        let mut history = vec![json!({ "role": "user", "content": [
            { "type": "text", "text": "hi" }
        ] })];
        let before = history.clone();
        let mut snapshot: Option<Value> = None;
        assert!(!refresh_memory_snapshot(
            &mut history,
            &mut snapshot,
            None,
            true,
            false,
            tools::MemoryScope::Global
        ));
        assert!(snapshot.is_none(), "不得凭空生成块");
        assert_eq!(history, before, "历史不得被改动");
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
        // P0-3（2026-10-27）：空转时**只回计数行**，不许解释 / 复述 / 列步骤 ——
        // 复盘绝大多数轮次都是空转，一句解释就是白花的输出 token。
        assert!(p.contains("no explanation"), "空转出口必须明令禁止解释");
        assert!(REVIEW_SYSTEM.contains("counter line"), "系统提示词同样要写明空转是计数行");
        assert!(
            !REVIEW_SYSTEM.contains("summarising what you saved"),
            "旧的「概括你存了什么」措辞会让空转也写一段正文"
        );
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

    /// 压缩的**顺序口径**（§待办 3-16 P1-6）：「先瘦可重取的 tool_result，再（只在瘦不下
    /// 来时）丢整条消息」。`can_drop = (elided == 0)` 那一行就是这条口径的实现 ——
    /// 这次瘦身成功 ⇒ 一条消息都不许丢，否则等于把刚省下来的又白丢一遍。
    #[test]
    fn drop_mode_elides_first_and_only_drops_when_nothing_was_elided() {
        let (mut history, _big) = history_for_elide(30_000, 1_000);
        let out = compact_history(&mut history, Compact::Drop, 100_000);
        assert!(out.elided > 0, "Drop 档先瘦可重取的大块 tool_result");
        assert_eq!(out.dropped, 0, "瘦下来了就不该再丢整条消息");
        assert!(
            history.iter().any(|m| {
                m["content"][0]["content"]
                    .as_str()
                    .map(|s| s.starts_with("[elided:"))
                    .unwrap_or(false)
            }),
            "原位置应换成指针块，而不是把消息删掉"
        );
    }

    /// ⚠️ **已知权衡，如实钉住**（2026-10-01；2026-10-14 补注）：`Drop` 档是 0.95 水位的
    /// **安全刚需** —— 「挤不出来（瘦不动）就丢整条消息」。所以**工具分级（elidable 那一层）
    /// 拦不住丢弃**：不可瘦的结果在 `Drop` 档下仍然可能被整条丢掉。
    ///
    /// 2026-10-14 的语义分层（S0）给 Drop **加了一层按层排序**，但它只保护 **L1 / L4**
    /// （用户诉求 / 失败证据）；`Agent` 报告这类**不可重取批量结果 = L6**，仍属「优先丢弃」
    /// 的层 ⇒ 本用例的结论不变。**写在这里是为了防止以后误以为「子代理报告永不丢」**。
    #[test]
    fn drop_still_evicts_what_the_grade_refuses_to_slim() {
        let (mut history, _) = history_for_elide_tool("Agent", 30_000, 1_000);
        let out = compact_history(&mut history, Compact::Drop, 100_000);
        assert_eq!(out.elided, 0, "分级不许瘦它");
        assert!(out.dropped > 0, "瘦不动 ⇒ Drop 档转而丢整条消息（安全刚需）");
    }

    /// 语义分层（S0，第 2 步）：`Drop` **优先丢 L6 噪声、保 L4 失败证据**。
    ///
    /// 造法：head 用户诉求 → 两条噪声 → 一次 `is_error` 的失败尝试 → 尾部。
    /// 若不按层丢，`natural_cut` 会把失败证据一起丢掉；按层丢则边界收缩到失败证据之前。
    #[test]
    fn drop_prefers_noise_over_failed_evidence() {
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"帮我改脚本"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"n1"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"n2"}]}),
            json!({"role":"assistant","content":[
                {"type":"tool_use","id":"b1","name":"Cmd","input":{"command":"cargo test"}}
            ]}),
            json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"b1","is_error":true,"content":"error: boom"}
            ]}),
            json!({"role":"assistant","content":[{"type":"text","text":"继续"}]}),
        ];
        for i in 0..COMPACT_KEEP_TAIL {
            history.push(json!({"role":"assistant","content":[{"type":"text","text":format!("t{i}")}]}));
        }
        // len = 14；natural_cut = 14 - 8 = 6（> 失败证据所在的下标 4）
        let out = compact_history(&mut history, Compact::Drop, 100_000);
        assert!(out.dropped > 0, "前面的噪声应当被丢掉：{}", out.dropped);
        assert!(
            history.iter().any(|m| m["content"][0]["is_error"].as_bool() == Some(true)),
            "失败证据（L4）必须留在 history 里，不因体积丢弃"
        );
        assert!(
            !history.iter().any(|m| m["content"][0]["text"].as_str() == Some("n1"))
                && !history.iter().any(|m| m["content"][0]["text"].as_str() == Some("n2")),
            "前面的噪声（L6）应当已被丢掉"
        );
    }

    /// 语义分层（S0）：**L4 不因体积消失** —— 失败证据躺在深处、`natural_cut` 越过它时，
    /// `Drop` 档必须把边界收缩到它之前（保留在 history）。
    #[test]
    fn failed_evidence_survives_compaction() {
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"把 parse_args 改对"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"先看看"}]}),
            json!({"role":"assistant","content":[
                {"type":"tool_use","id":"b1","name":"Cmd","input":{"command":"cargo test --bins"}}
            ]}),
            json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"b1","is_error":true,
                 "content":"error: cannot find macro parse_args_panics"}
            ]}),
        ];
        // 后面垫足够多的噪声，使 natural_cut 落在失败证据之后（下标 3）。
        for i in 0..(COMPACT_KEEP_TAIL + 4) {
            history.push(json!({"role":"assistant","content":[{"type":"text","text":format!("m{i}")}]}));
        }
        // len = 16；natural_cut = 8（越过失败证据）
        let out = compact_history(&mut history, Compact::Drop, 100_000);
        assert!(
            history.iter().any(|m| m["content"][0]["is_error"].as_bool() == Some(true)),
            "L4 不因体积消失：必须保留在 history（或进摘要 / 索引）"
        );
        assert!(out.dropped > 0, "其余噪声仍应被丢弃：{}", out.dropped);
    }

    /// 语义分层（S0）：**用户诉求（L1）不被丢** —— head 那条 + 中段那条都要活过 `Drop`。
    #[test]
    fn pinned_user_request_survives() {
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"原始诉求：修渲染层"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"n1"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"n2"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"n3"}]}),
            json!({"role":"user","content":[{"type":"text","text":"第二个诉求：别动 docs"}]}),
        ];
        for i in 0..COMPACT_KEEP_TAIL {
            history.push(json!({"role":"assistant","content":[{"type":"text","text":format!("t{i}")}]}));
        }
        // len = 13；natural_cut = 5（包含中段用户诉求）
        let out = compact_history(&mut history, Compact::Drop, 100_000);
        assert!(out.dropped > 0, "前面的噪声应当被丢");
        assert_eq!(
            history[0]["content"][0]["text"].as_str(),
            Some("原始诉求：修渲染层"),
            "head 用户诉求必须保留在 0 号位"
        );
        assert!(
            history
                .iter()
                .any(|m| m["content"][0]["text"].as_str() == Some("第二个诉求：别动 docs")),
            "中段用户诉求（L1）也不因体积丢弃"
        );
    }

    /// 语义分层（S0）：**配对完整** —— 边界恰好落在 `tool_result` 上时必须退到安全的配对边界，
    /// 被丢段不会产生「`tool_result` 无对应 `tool_use`」的孤儿。
    #[test]
    fn drop_does_not_orphan_tool_results() {
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"hi"}]}),
            json!({"role":"assistant","content":[
                {"type":"tool_use","id":"a1","name":"Read","input":{"file_path":"a"}}
            ]}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"a1","content":"A"}]}),
            json!({"role":"assistant","content":[
                {"type":"tool_use","id":"a2","name":"Read","input":{"file_path":"b"}}
            ]}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"a2","content":"B"}]}),
            json!({"role":"assistant","content":[
                {"type":"tool_use","id":"a3","name":"Read","input":{"file_path":"c"}}
            ]}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"a3","content":"C"}]}),
        ];
        for i in 0..(COMPACT_KEEP_TAIL - 3) {
            history.push(json!({"role":"assistant","content":[{"type":"text","text":format!("t{i}")}]}));
        }
        // len = 12；natural_cut = 12 - 8 = 4，正好指向 a2 的 tool_result（配对边界用例）
        let mut h = history.clone();
        let out = compact_history(&mut h, Compact::Drop, 100_000);
        assert!(out.dropped > 0, "这个用例必须真的丢过东西，否则没测到配对");
        // 配对完整：history 里任何 tool_result 都必须有对应的 tool_use。
        let uses: std::collections::HashSet<String> = h
            .iter()
            .filter_map(|m| m.get("content").and_then(Value::as_array))
            .flatten()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .filter_map(|b| b.get("id").and_then(Value::as_str).map(str::to_string))
            .collect();
        for m in &h {
            let Some(blocks) = m.get("content").and_then(Value::as_array) else {
                continue;
            };
            for b in blocks {
                if b.get("type").and_then(Value::as_str) != Some("tool_result") {
                    continue;
                }
                let id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
                assert!(uses.contains(id), "tool_result {id} 成了孤儿（无配对 tool_use）");
            }
        }
    }

    /// P6 小补丁（Drop 档护栏）：**具名事实行不因体积丢弃** —— 边界收缩到它之前，
    /// 且连带保住配对的 `tool_use`（不留孤儿）。
    #[test]
    fn drop_guard_keeps_protected_fact_line() {
        let noise = |tag: &str| {
            json!({"role":"assistant","content":[
                {"type":"text","text":format!("{tag}:{}", "a".repeat(100))}
            ]})
        };
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"整理审计记录"}]}),
            noise("n1"),
            noise("n2"),
            noise("n3"),
            json!({"role":"assistant","content":[
                {"type":"tool_use","id":"tu1","name":"Read","input":{"file_path":"audit.log"}}
            ]}),
            json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"tu1",
                 "content":"code word: ORBITAL-FERRET-9"}
            ]}),
            noise("n4"),
            noise("n5"),
        ];
        for i in 0..COMPACT_KEEP_TAIL {
            history.push(json!({"role":"assistant","content":[{"type":"text","text":format!("t{i}")}]}));
        }
        // len = 16；natural_cut = 8；受保护消息在下标 5 ⇒ 边界收缩到 5，再因配对退到 4。
        let out = compact_history(&mut history, Compact::Drop, 100_000);
        assert_eq!(out.dropped, 3, "只丢受保护消息之前的噪声");
        assert!(
            history.iter().any(|m| {
                m["content"][0]["content"]
                    .as_str()
                    .map(|s| s.contains("ORBITAL-FERRET-9"))
                    .unwrap_or(false)
            }),
            "具名事实行必须留在 history 里"
        );
        assert!(
            history.iter().any(|m| m["content"][0]["id"].as_str() == Some("tu1")),
            "配对的 tool_use 必须一起保留（不留孤儿）"
        );
    }

    /// P6 小补丁：无标签叙述型约束（P1 的形态）在 Drop 档同样受保护。
    #[test]
    fn drop_guard_keeps_bare_narrative_constraint() {
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"改渲染"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"n1"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"Indentation must be 4 spaces"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"n2"}]}),
        ];
        for i in 0..COMPACT_KEEP_TAIL {
            history.push(json!({"role":"assistant","content":[{"type":"text","text":format!("t{i}")}]}));
        }
        // len = 12；natural_cut = 4；受保护消息在下标 2 ⇒ 边界收缩到 2。
        let out = compact_history(&mut history, Compact::Drop, 100_000);
        assert_eq!(out.dropped, 1, "只丢下标 1 那条噪声");
        assert!(
            history
                .iter()
                .any(|m| m["content"][0]["text"].as_str() == Some("Indentation must be 4 spaces")),
            "叙述型约束必须留在 history 里"
        );
    }

    /// P6 小补丁：受保护消息就在可丢区间**开头** ⇒ 让路（收缩等于一条也丢不掉），
    /// 行为回到「按今天的口径丢干净」，绝不让 Drop 变成空转。
    #[test]
    fn drop_guard_yields_when_the_wall_is_at_head() {
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"hi"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"code word: X"}]}),
        ];
        for i in 0..10 {
            history.push(json!({"role":"assistant","content":[
                {"type":"text","text":format!("m{i}:{}", "z".repeat(500))}
            ]}));
        }
        for i in 0..COMPACT_KEEP_TAIL {
            history.push(json!({"role":"assistant","content":[{"type":"text","text":format!("t{i}")}]}));
        }
        // len = 20；natural_cut = 12；受保护消息就在 head=1 ⇒ 让路，dropped = 11。
        let out = compact_history(&mut history, Compact::Drop, 100_000);
        assert_eq!(out.dropped, 11, "让路后仍按原边界丢");
        assert!(
            !history.iter().any(|m| {
                m["content"][0]["text"]
                    .as_str()
                    .map(|s| s.contains("code word"))
                    .unwrap_or(false)
            }),
            "让路时受保护消息按原口径被丢（最坏情况 = 不接线）"
        );
    }

    /// P6 小补丁：没有受保护内容时**逐字节不变**（`dropped` 与不接线一致）。
    #[test]
    fn drop_guard_is_a_noop_on_ordinary_noise() {
        let mut history = vec![json!({"role":"user","content":[{"type":"text","text":"hi"}]})];
        for i in 0..7 {
            history.push(json!({"role":"assistant","content":[{"type":"text","text":format!("n{i}")}]}));
        }
        for i in 0..COMPACT_KEEP_TAIL {
            history.push(json!({"role":"assistant","content":[{"type":"text","text":format!("t{i}")}]}));
        }
        // len = 16；natural_cut = 8；head = 1 ⇒ dropped = 7。
        let out = compact_history(&mut history, Compact::Drop, 100_000);
        assert_eq!(out.dropped, 7, "无命中时护栏不改变任何行为");
        assert_eq!(history.len(), 9);
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

    /// append-only（P6 全量形态）：折算后不会撞预算 ⇒ **一条消息都不丢**，只把大块换成指针。
    #[test]
    fn append_only_keeps_every_message_when_elision_is_enough() {
        let (mut history, big) = history_for_elide_tool("Read", 30_000, 1_000);
        let len_before = history.len();
        let out = compact_history(&mut history, Compact::AppendOnly, 120_000);
        assert_eq!(out.elided, 1, "可重现大块应被换成指针");
        assert_eq!(out.dropped, 0, "折算后仍有余量 ⇒ 不丢消息");
        assert_eq!(history.len(), len_before, "append-only 不改变消息条数");
        assert_ne!(history[2]["content"][0]["content"], big);
    }

    /// append-only：折算后**仍会撞预算** ⇒ 回落到今天的 `Drop`（层感知 + 护栏都照旧）。
    #[test]
    fn append_only_falls_back_to_drop_when_the_estimate_still_overflows() {
        let (mut history, _) = history_for_elide_tool("Read", 30_000, 1_000);
        let len_before = history.len();
        let out = compact_history(&mut history, Compact::AppendOnly, 5_000_000);
        assert_eq!(out.elided, 1, "回落之前仍先把能瘦的瘦掉");
        assert!(out.dropped > 0, "折算后仍超预算必须丢整条（否则下一请求直接 400）");
        assert!(history.len() < len_before);
    }

    /// append-only 与 `Elide` 的关键差别：**尾部也瘦**（只保最近 2 条）。
    /// 大块落在 `len-3` ⇒ `Elide`（保 8 条）不碰它，append-only 会瘦它。
    #[test]
    fn append_only_elides_inside_the_normal_keep_tail() {
        let big = json!("z".repeat(30_000));
        let build = || {
            let mut h = vec![json!({"role":"user","content":[{"type":"text","text":"hi"}]})];
            for i in 0..6 {
                h.push(json!({"role":"assistant","content":[{"type":"text","text":format!("n{i}")}]}));
            }
            h.push(json!({"role":"assistant","content":[
                {"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"f.txt"}}
            ]}));
            h.push(json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"t1","content":big.clone()}
            ]}));
            for i in 0..2 {
                h.push(json!({"role":"assistant","content":[{"type":"text","text":format!("d{i}")}]}));
            }
            // len = 11；大块在下标 8 = len-3（在 `Elide` 的保留尾部内、在 append-only 的 2 条外）
            h
        };
        let mut plain = build();
        let out = compact_history(&mut plain, Compact::Elide, 100_000);
        assert_eq!(out.elided, 0, "Elide 保最近 8 条 ⇒ 不碰尾部的大块");
        assert_eq!(plain[8]["content"][0]["content"], big);

        let mut append = build();
        let out = compact_history(&mut append, Compact::AppendOnly, 100_000);
        assert_eq!(out.elided, 1, "append-only 只保最近 2 条 ⇒ 尾部的大块也换成指针");
        assert_ne!(append[8]["content"][0]["content"], big);
        assert_eq!(out.dropped, 0);
    }

    /// append-only 同样受**确定性护栏**保护：含具名事实的块不换掉（宁多花 token）。
    /// 没有成本模型 ⇒ 护栏**不得**按 `net ≤ 0` 把名单清空（那是 `Elide` 的口径）。
    #[test]
    fn append_only_still_respects_the_fact_guard() {
        let mut h = vec![json!({"role":"user","content":[{"type":"text","text":"hi"}]})];
        for i in 0..7 {
            h.push(json!({"role":"assistant","content":[{"type":"text","text":format!("n{i}")}]}));
        }
        h.push(json!({"role":"assistant","content":[
            {"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"audit.log"}}
        ]}));
        // 必须够大才会进候选（`ELIDE_TOOL_RESULT_CHARS`），事实行藏在填充里。
        let text = format!("{}\ncode word: ORBITAL-FERRET-9", "x".repeat(3_000));
        h.push(json!({"role":"user","content":[
            {"type":"tool_result","tool_use_id":"t1","content":text}
        ]}));
        for i in 0..2 {
            h.push(json!({"role":"assistant","content":[{"type":"text","text":format!("d{i}")}]}));
        }
        // len = 12；事实块在下标 9 = len-3 ⇒ 在 append-only 的可瘦区间内，但命中护栏。
        let out = compact_history(&mut h, Compact::AppendOnly, 120_000);
        assert_eq!(out.elided, 0, "命中护栏 ⇒ 不换掉");
        assert_eq!(out.dropped, 0, "折算后仍有余量 ⇒ 也不丢消息（宁多花 token）");
        assert!(h[9]["content"][0]["content"]
            .as_str()
            .map(|s| s.contains("ORBITAL-FERRET-9"))
            .unwrap_or(false));
    }

    /// append-only 的回落判据是**保守**的：体积未知（`measured == 0`）⇒ 直接回落；
    /// 折算用 4 字符 ≈ 1 token（估少不估多）。
    #[test]
    fn append_only_must_drop_is_conservative() {
        assert!(append_only_must_drop(0, 400_000), "体积未知 ⇒ 回落，不拿 400 赌");
        assert!(!append_only_must_drop(100_000, 0), "100k < 128k ⇒ 不回落");
        assert!(append_only_must_drop(200_000, 0), "200k ≥ 128k ⇒ 必须回落");
        // 省 40 万字 ≈ 10 万 token（4 字/token）⇒ 200k - 100k = 100k < 128k ⇒ 不回落。
        assert!(!append_only_must_drop(200_000, 400_000));
    }

    /// 语义分层（S0）：`classify_message_layer` 是**确定性纯函数**，六层各一例。
    #[test]
    fn layer_classification_covers_every_layer() {
        // L1：用户非合成文本 = 诉求 / 任务目标（保原文）
        let l1 = json!({"role":"user","content":[{"type":"text","text":"帮我把 parse_args 改对"}]});
        assert_eq!(classify_message_layer(&l1, None), Layer::L1);
        // 合成消息（以 `[` 开头）不算「用户的话」⇒ 落 L6
        let synthetic = json!({"role":"user","content":[{"type":"text","text":"[summary of earlier]"}]});
        assert_eq!(classify_message_layer(&synthetic, None), Layer::L6);

        // L2：assistant 的工具调用（工作结论 / 决策）
        let l2 = json!({"role":"assistant","content":[
            {"type":"tool_use","id":"a","name":"Read","input":{"file_path":"x"}}
        ]});
        assert_eq!(classify_message_layer(&l2, Some("Read")), Layer::L2);

        // L3：TodoWrite 的未决待办
        let l3 = json!({"role":"assistant","content":[
            {"type":"tool_use","id":"t","name":"TodoWrite","input":{"todos":[]}}
        ]});
        assert_eq!(classify_message_layer(&l3, Some("TodoWrite")), Layer::L3);

        // L3：TaskPhase 的阶段标题 / 小总结（2026-11-21，长跑里最容易被压缩掉）
        let l3_phase = json!({"role":"assistant","content":[
            {"type":"tool_use","id":"p","name":"TaskPhase","input":{"state":"done","summary":"ok"}}
        ]});
        assert_eq!(
            classify_message_layer(&l3_phase, Some("TaskPhase")),
            Layer::L3
        );

        // L4：`is_error` 的失败证据 —— 即使工具本身可重取（Cmd）也判 L4，**不是** L5
        let l4 = json!({"role":"user","content":[
            {"type":"tool_result","tool_use_id":"b","is_error":true,"content":"error: boom"}
        ]});
        assert_eq!(classify_message_layer(&l4, Some("Cmd")), Layer::L4);

        // L5：可重取工具的结果（重跑即得）
        let l5 = json!({"role":"user","content":[
            {"type":"tool_result","tool_use_id":"r","content":"file body"}
        ]});
        assert_eq!(classify_message_layer(&l5, Some("Read")), Layer::L5);
        // 不可重取（Agent / MCP）的结果**不是** L5（照旧不可瘦身）
        assert_eq!(classify_message_layer(&l5, Some("Agent")), Layer::L6);

        // L6：assistant 纯文本 = 纯过程性噪声
        let l6 = json!({"role":"assistant","content":[{"type":"text","text":"好的，我来处理"}]});
        assert_eq!(classify_message_layer(&l6, None), Layer::L6);
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

    /// 「预算撞顶不中止」的闸门判据（2026-10-09）：**只有清单里还有 `pending` /
    /// `in_progress` 才放宽**（见 `budget_extension_allowed`）。钉住四条，
    /// 最后一条最要紧 —— 没有计划时必须**保守**，不能把普通问答也变成无底洞。
    #[test]
    fn budget_gate_opens_only_when_the_plan_is_unfinished() {
        let mk = |status: &str| {
            json!({"role":"assistant","content":[{
                "type":"tool_use","id":"t","name":"TodoWrite",
                "input":{"todos":[{"content":"做完这个","status":status,"activeForm":"做"}]}
            }]})
        };
        assert!(todo_has_unfinished(&[mk("pending")]), "pending ⇒ 计划未完成");
        assert!(todo_has_unfinished(&[mk("in_progress")]), "in_progress ⇒ 计划未完成");
        assert!(!todo_has_unfinished(&[mk("completed")]), "全 completed ⇒ 计划已完成");
        assert!(!todo_has_unfinished(&[]), "没有清单 ⇒ 不放宽（保守）");
    }

    /// 任务级预算（S2 第一刀，2026-10-22）：钉住「跨提问累计」「0 = 关闭」「坏值退回关闭」
    /// 三条 —— 任何一条被改坏，都会让用户的环境变量变成「悄悄掐死长任务」。
    #[test]
    fn task_budget_accumulates_across_turns_and_is_off_by_default() {
        // 0 = 关闭：记录多少都不算撞顶（旧行为逐字节不变）。
        let mut off = TaskBudget::new(0);
        off.record(10_000_000);
        assert!(!off.over(), "limit=0 必须等同于「没有这条闸门」");

        // 跨提问累计：两笔分别来自不同提问，加在一起才撞顶。
        let mut on = TaskBudget::new(1_000);
        on.record(600);
        assert!(!on.over(), "单笔未到上限不撞顶");
        on.record(400);
        assert!(on.over(), "跨提问累计到上限即撞顶");
        on.record(1);
        assert!(on.over(), "撞顶后保持（不会因为再记一笔而回落）");

        // 饱和：异常大的数值不 panic、不回绕。
        let mut sat = TaskBudget::new(1);
        sat.record(u64::MAX);
        sat.record(u64::MAX);
        assert!(sat.over());
    }

    #[test]
    fn task_budget_env_parsing_falls_back_to_off() {
        assert_eq!(parse_task_budget(None), 0, "未设置 ⇒ 关闭");
        assert_eq!(parse_task_budget(Some("  ")), 0, "空值 ⇒ 关闭");
        assert_eq!(parse_task_budget(Some("abc")), 0, "坏值 ⇒ 关闭（不掐长任务）");
        assert_eq!(parse_task_budget(Some(" 250000 ")), 250_000, "合法值去空白");
    }

    /// 没有未完成计划时：任务级撞顶一票否决、续命次数用满即停（旧语义）。
    #[test]
    fn task_budget_over_disables_extensions_without_plan() {
        assert!(
            budget_extension_allowed(true, false, 0, 6, false, false),
            "正常：压动了且还有额度"
        );
        assert!(
            !budget_extension_allowed(true, false, 0, 6, true, false),
            "任务级撞顶后即使压动了也不续命"
        );
        assert!(
            !budget_extension_allowed(false, false, 0, 6, false, false),
            "压不动不续命"
        );
        assert!(
            !budget_extension_allowed(true, false, 6, 6, false, false),
            "额度用满不续命"
        );
    }

    /// 2026-11-03 用户口径：**计划未完成 ⇒ 预算不终止** ——
    /// 任务级撞顶、续命次数用满都不再否决，只看上下文是否还能安全继续。
    #[test]
    fn unfinished_plan_ignores_budget_caps_but_respects_context() {
        assert!(
            budget_extension_allowed(true, false, 99, 6, true, true),
            "计划未完成：压动了就继续，忽略次数与任务级预算"
        );
        assert!(
            budget_extension_allowed(false, true, 99, 2, true, true),
            "计划未完成：上下文宽裕就继续"
        );
        assert!(
            !budget_extension_allowed(false, false, 0, 6, false, true),
            "压不动且上下文不宽裕 ⇒ 仍要收口（否则会 400）"
        );
    }

    /// 2026-11-02：上下文仍宽裕时，**不压缩也必须能续命** —— 这正是把「续命」与
    /// 「强制压缩」解耦的判据。否则又退化成「一撞预算就丢 31–55 条消息」。
    #[test]
    fn roomy_context_allows_extension_without_compaction() {
        assert!(
            budget_extension_allowed(false, true, 0, 2, false, false),
            "上下文宽裕：不压缩也续命"
        );
        assert!(
            !budget_extension_allowed(false, true, 0, 2, true, false),
            "任务级撞顶仍然一票否决（无计划时）"
        );
        assert!(
            !budget_extension_allowed(false, true, 2, 2, false, false),
            "次数用满仍然不续命（无计划时）"
        );
    }

    /// 预算压缩的三条否决理由：本轮已压过 / 上下文宽裕 / 本问额度用满。
    /// （轮距规则只在额度 > 1 时才生效；当前 `MAX_BUDGET_COMPACTS_PER_TURN = 1`，
    /// 它作为把上限调大时的护栏保留。）
    #[test]
    fn budget_compaction_is_throttled_and_skipped_when_roomy() {
        assert!(should_budget_compact(false, false, 0, 0), "首次且上下文紧张 ⇒ 压");
        assert!(!should_budget_compact(true, false, 0, 0), "本轮已压过 ⇒ 不再压");
        assert!(!should_budget_compact(false, true, 0, 0), "上下文宽裕 ⇒ 不压");
        assert!(
            !should_budget_compact(false, false, MAX_BUDGET_COMPACTS_PER_TURN, 99),
            "本问额度用满 ⇒ 不压"
        );
    }

    /// 预算计点：缓存命中按 `CACHE_READ_PRICE_PCT`（当前 2%）折算，账面用量不变。
    #[test]
    fn cache_read_tokens_are_discounted_in_budget_points() {
        assert_eq!(budget_points(1000, 0, 0), 1000, "未命中全额");
        assert_eq!(
            budget_points(0, 1000, 0),
            1000 * CACHE_READ_PRICE_PCT / 100,
            "命中按权重折算"
        );
        assert_eq!(budget_points(100, 50, 10), 111, "in+create 全额，read 折算");
        assert_eq!(budget_points(0, 9, 0), 0, "不足一个折算单位时为 0（整数除）");
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
            extra_headers: Vec::new(),
            model: "test-model".to_string(),
            thinking: Cell::new(Thinking::Disabled),
            last_input: Cell::new(0),
            last_compact: Cell::new(0),
            sub: Arc::new(SubagentUsage::default()),
            review: Arc::new(SubagentUsage::default()),
        }
    }

    /// 结论索引用例的最小 `Meta`（不碰磁盘 / 网络）。
    fn test_meta() -> conclusions::Meta<'static> {
        conclusions::Meta {
            ts: 1_000,
            scope: tools::MemoryScope::Global,
            turn: 1,
            session_id: "sess-test",
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

    /// 多请求桩：按顺序为每个新连接返回一条响应，并把每个请求体发回。
    ///
    /// 单请求桩（上面的 `start_stub_server`）只能验证一次往返；摘要的「触顶重试」需要
    /// 第二个请求，所以单独加这一个，不改既有用例的语义。
    fn start_stub_sequence(responses: Vec<String>) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind stub");
        let addr = listener.local_addr().expect("stub addr");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for response_body in responses {
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
            }
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

        let (out, entries) =
            summarize_dropped(&cfg, &dropped, &test_meta()).expect("stub 返回正常时应当拿到摘要");
        assert!(out.starts_with(SUMMARY_HEADER), "摘要必须带固定表头：{out}");
        assert!(
            out.contains("继续验证 §8.2"),
            "必须用响应的 content[].text：{out}"
        );
        assert!(entries.is_empty(), "响应里没有结构化段 ⇒ 结论为空，调用方退到确定性抽取");

        let sent: Value =
            serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).expect("应当收到请求"))
                .expect("请求体是 JSON");
        assert_eq!(sent["stream"], json!(false), "摘要请求必须是非流式");
        assert_eq!(sent["max_tokens"], json!(SUMMARY_MAX_OUTPUT_TOKENS));
        let system = sent["system"].as_str().expect("system 必须是字符串");
        assert!(system.starts_with(SUMMARY_PROMPT), "system 必须以摘要提示词开头");
        assert!(
            system.contains(conclusions::INDEX_BEGIN)
                && system.contains(conclusions::INDEX_END),
            "摘要提示词末尾必须追加结构化结论段指令（否则索引没有模型侧来源）"
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

    /// 输出触顶必须重试一次（2026-11-04）。
    ///
    /// 结构化结论段在输出**末尾**，模型写满 `max_tokens` 时最先被截掉；此前只在「空摘要」
    /// 时重试，于是每次压缩都留下「结构化结论 0 条」而不自知。这条用例同时钉住：
    /// ① 第一次请求用默认上限；② 第二次请求上限翻倍；③ 重试后能解析出结论。
    #[test]
    fn summary_retries_once_when_output_hits_max_tokens() {
        let truncated = json!({
            "id": "m1",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "## Historical Task Snapshot\n继续验证"}],
            "usage": {"input_tokens": 100, "output_tokens": SUMMARY_MAX_OUTPUT_TOKENS}
        })
        .to_string();
        let ok = json!({
            "id": "m2",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": format!(
                "## Historical Task Snapshot\n继续验证\n\n{}\n[{{\"status\":\"verified\",\"conclusion\":\"缓存命中率已核对\",\"keywords\":[\"cache\"]}}]\n{}",
                conclusions::INDEX_BEGIN, conclusions::INDEX_END
            )}],
            "usage": {"input_tokens": 100, "output_tokens": 200}
        })
        .to_string();

        let (endpoint, rx) = start_stub_sequence(vec![truncated, ok]);
        let cfg = cfg_pointing_at(&endpoint);
        let (out, entries) =
            summarize_dropped(&cfg, &long_dropped(), &test_meta()).expect("重试后应当拿到摘要");
        assert!(!out.contains("缓存命中率已核对"), "结构化段不该留在摘要正文：{out}");
        assert_eq!(entries.len(), 1, "重试后结构化结论必须解析出来");

        let first: Value = serde_json::from_str(
            &rx.recv_timeout(Duration::from_secs(5))
                .expect("第一次请求"),
        )
        .expect("请求体是 JSON");
        let second: Value = serde_json::from_str(
            &rx.recv_timeout(Duration::from_secs(5))
                .expect("重试请求"),
        )
        .expect("请求体是 JSON");
        assert_eq!(first["max_tokens"], json!(SUMMARY_MAX_OUTPUT_TOKENS));
        assert_eq!(
            second["max_tokens"],
            json!(SUMMARY_MAX_OUTPUT_TOKENS * 2),
            "触顶后必须把 max_tokens 翻倍再试"
        );
    }

    /// 端点返回非 2xx / 非 JSON / 空摘要时，一律降级为 `None`（不能让整轮对话失败）。
    #[test]
    fn summary_failures_degrade_instead_of_erroring() {
        let meta = test_meta();
        // 非 JSON 的 200
        let (endpoint, _rx) = start_stub_server("not json at all".to_string());
        assert!(summarize_dropped(&cfg_pointing_at(&endpoint), &long_dropped(), &meta).is_none());

        // 200 但 content 为空
        let (endpoint, _rx) = start_stub_server(
            json!({"content": [], "usage": {"output_tokens": 0}}).to_string(),
        );
        assert!(summarize_dropped(&cfg_pointing_at(&endpoint), &long_dropped(), &meta).is_none());

        // 200 但文本全空白
        let (endpoint, _rx) = start_stub_server(
            json!({"content": [{"type": "text", "text": "   \n  "}]}).to_string(),
        );
        assert!(summarize_dropped(&cfg_pointing_at(&endpoint), &long_dropped(), &meta).is_none());

        // 连不上（端口 1）
        assert!(summarize_dropped(&cfg_for_tests(), &long_dropped(), &meta).is_none());
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

        let (summary, index_entries) =
            summarize_dropped(&cfg, &dropped, &test_meta()).expect("真端点应当接受非流式摘要请求");
        println!(
            "[summary-live] 原文 {transcript_len} 字 → 摘要 {} 字 / 结构化结论 {} 条：\n{summary}",
            summary.chars().count(),
            index_entries.len()
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

    /// 没有可丢 / 可瘦的内容时，`restore_after_compaction` 一条网络请求都不发、一条合成消息都不插。
    #[test]
    fn restore_does_nothing_when_nothing_was_dropped_or_elided() {
        let cfg = cfg_for_tests();
        let dir = std::env::temp_dir().join(format!("lunac-concl-nop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut history = vec![json!({"role":"user","content":[{"type":"text","text":"hi"}]})];
        let added = restore_after_compaction(
            &cfg,
            &mut history,
            &[],
            &[],
            tools::MemoryScope::Global,
            1,
            &dir,
        );
        assert_eq!(added, 0, "无丢无瘦 ⇒ 不插合成消息、不发请求");
        assert_eq!(history.len(), 1, "不该改动历史");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 内容太短时跳过 —— 不值得为一点点东西花一次调用（单轮成本上限的一道闸）。
    #[test]
    fn tiny_dropped_regions_are_not_summarised() {
        let cfg = cfg_for_tests();
        let small = vec![json!({"role":"assistant","content":[{"type":"text","text":"很短"}]})];
        assert!(
            summarize_dropped(&cfg, &small, &test_meta()).is_none(),
            "低于 SUMMARY_MIN_INPUT_CHARS 时不该调模型"
        );
    }

    /// 造一段「Force 档必然 drain 掉中段的结论 / 失败尝试 / 文件指针」的历史。
    fn history_with_droppable_conclusions() -> Vec<Value> {
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"把 core-agent 的 parse_args 改对，文件在 src/main.rs"}]}),
            json!({"role":"assistant","content":[
                {"type":"tool_use","id":"b1","name":"Cmd","input":{"command":"cargo test --bins"}}
            ]}),
            json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"b1","is_error":true,"content":"error: cannot find macro parse_args_panics"}
            ]}),
        ];
        for i in 0..COMPACT_KEEP_TAIL * 2 {
            history.push(json!({"role":"assistant","content":[{"type":"text","text":format!("tail{i}")}]}));
        }
        history
    }

    /// **端到端（写入 + 检索 + 注入）**：触发一次真实压缩 ⇒ 索引出现条目（**失败那条带 `reason`**、
    /// 指针字段正确），压缩后检索命中并把结论块注入到可变层。
    ///
    /// 走**确定性路径**（dropped 不足 `SUMMARY_MIN_INPUT_CHARS` ⇒ 摘要调用被跳过、不联网）。
    #[test]
    fn compaction_writes_index_and_reinjects_conclusions() {
        let cfg = cfg_for_tests();
        let dir = std::env::temp_dir().join(format!("lunac-concl-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut history = history_with_droppable_conclusions();
        let out = compact_history(&mut history, Compact::Force, 0);
        assert!(out.dropped > 0, "Force 档必须真的丢了中段");

        let added = restore_after_compaction(
            &cfg,
            &mut history,
            &out.dropped_msgs,
            &out.elided_msgs,
            tools::MemoryScope::Global,
            1,
            &dir,
        );
        assert!(added >= 1, "至少要把摘要 / 兜底清单钉回");

        let entries = conclusions::load(&dir, tools::MemoryScope::Global);
        let failed = entries
            .iter()
            .find(|e| e.status == conclusions::STATUS_FAILED)
            .expect("失败尝试必须进索引（不得删 —— Manus：保留纠错证据）");
        assert!(!failed.reason.is_empty(), "失败条目必须带 reason");
        assert!(
            failed.reason.contains("parse_args_panics"),
            "reason 应当是错误文本：{}",
            failed.reason
        );
        assert!(
            entries.iter().any(|e| e
                .refs
                .iter()
                .any(|r| r.kind == "command" && r.value.contains("cargo test"))),
            "失败的 Cmd 要留 command 指针"
        );

        // 检索 + 注入：历史里出现结论块（可变层，紧随摘要之后，不在 0 号位）。
        let injected = history
            .iter()
            .position(|m| {
                m["content"][0]["text"]
                    .as_str()
                    .map(|t| t.starts_with(conclusions::INJECT_HEADER))
                    .unwrap_or(false)
            })
            .expect("压缩后必须检索并注入结论块");
        assert!(injected >= 1, "注入块不能在 0 号位（那是 head / 固定上下文块）");

        // ③ 指针回注（§待办 3-16 P1-7）：被丢区间里的**可重跑**调用要列出来，
        // 且排在结论索引之后（[摘要][结论][指针] 的固定顺序）。
        let pointers = history
            .iter()
            .position(|m| {
                m["content"][0]["text"]
                    .as_str()
                    .map(|t| t.starts_with(ptrblock::HEADER))
                    .unwrap_or(false)
            })
            .expect("被丢区间含可重跑调用 ⇒ 必须回注指针块");
        assert!(pointers > injected, "指针块必须排在结论索引之后");
        let text = history[pointers]["content"][0]["text"].as_str().unwrap_or("");
        assert!(
            text.contains("Cmd cargo test --bins (re-runnable)"),
            "指针块要带上原命令：{text}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **前缀不变量**：压缩 + 结论注入**只动可变层（history）** —— `system` 提示词（固定前缀）与
    /// 历史开头的固定上下文块逐字节不变（`code-standards.md` §规范 6 / `forbidden.md` §禁用 45）。
    #[test]
    fn compaction_and_injection_leave_the_fixed_prefix_untouched() {
        let cfg = cfg_for_tests();
        let dir = std::env::temp_dir().join(format!("lunac-concl-prefix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // 历史开头放一条固定上下文块（模拟 `context_block_message` 注入的那条）。
        let ctx = context_block_message(
            "# Background context (fixed for this session)\n- prefers tabs",
        )
        .unwrap();
        let mut history = vec![ctx.clone()];
        history.extend(history_with_droppable_conclusions());
        let system_before = SYSTEM_PROMPT.to_string();

        let out = compact_history(&mut history, Compact::Force, 0);
        let _ = restore_after_compaction(
            &cfg,
            &mut history,
            &out.dropped_msgs,
            &out.elided_msgs,
            tools::MemoryScope::Global,
            1,
            &dir,
        );

        // ① `system` 提示词（真的固定前缀）逐字节不变 —— 本函数根本不接收它，这里把意图钉住。
        assert_eq!(SYSTEM_PROMPT, system_before);
        // ② 历史开头那条固定上下文块逐字节不变（压缩保留 head；注入块不占 0 号位）。
        assert_eq!(history[0], ctx, "history[0] 必须是原样的固定上下文块");
        // ③ 结论块是可变层里的**独立 user 消息**（带固定表头）。
        let injected = history
            .iter()
            .find(|m| {
                m["content"][0]["text"]
                    .as_str()
                    .map(|t| t.starts_with(conclusions::INJECT_HEADER))
                    .unwrap_or(false)
            })
            .expect("结论块必须注入");
        assert_eq!(injected["role"].as_str(), Some("user"), "注入块是可变层的 user 文本消息");

        // ④ 语义分层（S0）的 `Drop` 档也走一遍：前缀不变量必须一样成立。
        //    造一段「噪声 ×3 → 用户诉求(L1) → 失败证据(L4) → 尾部」的历史，Drop 只丢噪声。
        let mut h2 = vec![ctx.clone()];
        for i in 0..3 {
            h2.push(json!({"role":"assistant","content":[{"type":"text","text":format!("noise{i}")}]}));
        }
        h2.push(json!({"role":"user","content":[{"type":"text","text":"继续修 parse_args"}]}));
        h2.push(json!({"role":"assistant","content":[
            {"type":"tool_use","id":"b9","name":"Cmd","input":{"command":"cargo test"}}
        ]}));
        h2.push(json!({"role":"user","content":[
            {"type":"tool_result","tool_use_id":"b9","is_error":true,"content":"error: boom"}
        ]}));
        for i in 0..COMPACT_KEEP_TAIL {
            h2.push(json!({"role":"assistant","content":[{"type":"text","text":format!("t{i}")}]}));
        }
        let drop_out = compact_history(&mut h2, Compact::Drop, 0);
        assert!(drop_out.dropped > 0, "Drop 档应当丢掉了噪声");
        assert_eq!(SYSTEM_PROMPT, system_before);
        assert_eq!(h2[0], ctx, "Drop 档下 history[0] 仍必须是原样的固定上下文块");
        assert!(
            h2.iter().any(|m| m["content"][0]["is_error"].as_bool() == Some(true)),
            "Drop 档下失败证据（L4）仍在（前缀不变量与保证据同时成立）"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **零新增调用**：压缩时结构化的结论提取**复用摘要那一次调用** —— 同一份模型响应既产出钉回
    /// 历史的摘要，又产出进索引的结论；全程**只有一次** API 请求。
    #[test]
    fn structured_conclusions_reuse_the_single_summary_call() {
        let body = format!(
            "## Historical Task Snapshot\n继续改 parse_args\n\n{}\n\
             [{{\"status\":\"failed\",\"conclusion\":\"试了 cargo test 不行\",\"reason\":\"cannot find macro\",\"keywords\":[\"parse_args\"],\"refs\":[{{\"kind\":\"file\",\"value\":\"src/main.rs\"}}]}}]\n{}",
            conclusions::INDEX_BEGIN,
            conclusions::INDEX_END
        );
        let canned =
            json!({"content":[{"type":"text","text":body}],"usage":{"output_tokens":10}}).to_string();
        let (endpoint, rx) = start_stub_server(canned);
        let cfg = cfg_pointing_at(&endpoint);
        let dir = std::env::temp_dir().join(format!("lunac-concl-call-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // dropped 要够长（>= `SUMMARY_MIN_INPUT_CHARS`）才触发那一次调用。
        let dropped: Vec<Value> = (0..6)
            .map(|i| {
                json!({"role":"assistant","content":[{
                    "type":"text","text":format!("m{i}:{}", "x".repeat(1_000))
                }]})
            })
            .collect();
        let mut history = vec![json!({"role":"user","content":[{"type":"text","text":"继续"}]})];

        let _ = restore_after_compaction(
            &cfg,
            &mut history,
            &dropped,
            &[],
            tools::MemoryScope::Global,
            1,
            &dir,
        );

        // ① 摘要钉回（来自这次调用）
        assert!(
            history
                .iter()
                .any(|m| m["content"][0]["text"]
                    .as_str()
                    .map(|t| t.starts_with(SUMMARY_HEADER))
                    .unwrap_or(false)),
            "摘要必须钉回"
        );
        // ② 结构化结论落进索引 —— **同一份响应既产摘要又产结论 = 复用那一次调用**
        let entries = conclusions::load(&dir, tools::MemoryScope::Global);
        assert!(
            entries.iter().any(|e| e.status == conclusions::STATUS_FAILED
                && e.refs.iter().any(|r| r.value == "src/main.rs")),
            "结构化段必须进索引（否则模型侧结论提取没接上）"
        );
        // ③ 那一次请求确实发生（stub 只服务一次 ⇒ 它就是全部模型调用）
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok(), "必须发出且只发一次摘要请求");

        let _ = std::fs::remove_dir_all(&dir);
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

        // 思考与答复**共用一个 max_tokens 池子**：预算之外必须真给答复留出 HEADROOM，
        // 否则「思考用满 + 大段代码」就会撞上限被截断。实测 2026-10-07 撞过 12288（旧值
        // = 8192 + 4096），不得回退到那个数。
        assert_eq!(
            max_tokens_for(Thinking::Budget(THINKING_BUDGET)),
            THINKING_BUDGET + THINKING_HEADROOM_TOKENS
        );
        assert!(THINKING_HEADROOM_TOKENS > 4096, "余量被改回会被截断的旧值");
        // 关档（无思考抢池子）仍用手收紧的基线
        assert_eq!(max_tokens_for(Thinking::Disabled), BASE_MAX_TOKENS);

        // 关档的降级终点是「不发字段」，绝不退到 adaptive（那等于反手把思考打开）
        assert_eq!(Thinking::Disabled.next(), Some(Thinking::Omit));
        assert_eq!(Thinking::Disabled.to_json(), Some(json!({ "type": "disabled" })));
    }

    /// 「按轮思考覆盖」（2026-10-09）：**只认 `off`** —— 其余值一律 `None`（用全局开关）。
    /// 桌宠轮靠它强制关 thinking；按轮「开」刻意不支持（全局开关已经能开）。
    #[test]
    fn per_turn_thinking_override_only_turns_things_off() {
        for off in ["off", " 0 ", "false"] {
            assert_eq!(
                thinking_override_from(Some(off)),
                Some(Thinking::Disabled),
                "{off} 应为「按轮关」"
            );
        }
        for other in ["on", "", "8192", "whatever"] {
            assert_eq!(thinking_override_from(Some(other)), None, "{other:?} 不该覆盖");
        }
        assert_eq!(thinking_override_from(None), None, "不带字段 ⇒ 用全局开关");
    }

    /// stdin 输入类型判定（2026-10-11）：compact 必须被识别；既有三种不变；
    /// 未知 / 缺 type 仍落 Unknown（主循环照旧打「忽略输入类型」）。
    #[test]
    fn stdin_input_types_are_classified() {
        assert!(matches!(stdin_kind(&json!({"type":"user"})), StdinKind::User));
        assert!(matches!(
            stdin_kind(&json!({"type":"set_history"})),
            StdinKind::SetHistory
        ));
        assert!(matches!(
            stdin_kind(&json!({"type":"control_response"})),
            StdinKind::ControlResponse
        ));
        assert!(matches!(
            stdin_kind(&json!({"type":"compact"})),
            StdinKind::Compact
        ));
        assert!(matches!(
            stdin_kind(&json!({"type":"pause"})),
            StdinKind::Pause
        ));
        assert!(matches!(
            stdin_kind(&json!({"type":"foo"})),
            StdinKind::Unknown(Some("foo"))
        ));
        assert!(matches!(stdin_kind(&json!({})), StdinKind::Unknown(None)));
    }

    /// 中途三态控制（§待办 3-28.1）：只有显式 `steer=insert` 的 user 被注入；
    /// 其余消息（普通 user / compact）原样进 backlog；pause 只置位、不进 backlog。
    #[test]
    fn steering_splits_inserts_pause_and_backlog_in_arrival_order() {
        let (tx, rx) = mpsc::channel();
        tx.send(json!({
            "type": "user",
            "steer": "insert",
            "message": { "role": "user", "content": [{ "type": "text", "text": "换方向" }] },
        }))
        .unwrap();
        tx.send(json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": "普通提问" }] },
        }))
        .unwrap();
        tx.send(json!({"type":"pause"})).unwrap();
        tx.send(json!({"type":"compact"})).unwrap();
        let mut backlog = VecDeque::new();
        let s = drain_steering(&rx, &mut backlog);
        assert_eq!(s.inserts.len(), 1);
        assert_eq!(s.inserts[0].0, "换方向");
        assert!(s.paused);
        assert_eq!(backlog.len(), 2, "非插入消息必须原样留给主循环");
        assert!(matches!(stdin_kind(&backlog[0]), StdinKind::User));
        assert!(matches!(stdin_kind(&backlog[1]), StdinKind::Compact));

        // 插入消息压进 history 后必须是**新的一条 user**（不改上一条 tool_result）。
        let mut history = vec![json!({"role":"user","content":[
            {"type":"tool_result","tool_use_id":"t1","content":"ok"}
        ]})];
        push_steering_message(&mut history, "换方向", &[]);
        assert_eq!(history.len(), 2);
        assert_eq!(history[1]["role"], "user");
        assert_eq!(history[1]["content"][0]["text"], "换方向");
        assert_eq!(history[0]["content"][0]["tool_use_id"], "t1");
    }

    /// 空历史手动压缩：必须回一条 elided/dropped 均为 0 的 context_compacted，
    /// 且**不得**被塞进 TRIMMED_MARKER 之类的合成消息（Force 档对空历史会做那件事）。
    #[test]
    fn manual_compact_on_empty_history_receipts_zero_without_pollution() {
        let cfg = cfg_for_tests();
        let mut history: Vec<Value> = Vec::new();
        let (ev, compacted) =
            manual_compact(&cfg, &mut history, &std::env::temp_dir(), tools::MemoryScope::Global);
        let ev = ev.expect("空历史也必须补发一条回执");
        assert!(!compacted, "空历史不算压动过（不该触发记忆快照刷新）");
        assert_eq!(ev["type"], "system");
        assert_eq!(ev["subtype"], "context_compacted");
        assert_eq!(ev["elided"], 0);
        assert_eq!(ev["dropped"], 0);
        assert!(history.is_empty(), "空历史不得被插进合成消息");
    }

    /// 无可压缩内容时的手动压缩：历史逐字节不变，但仍补发 0/0 回执
    /// （compact_history 只在真压动时才发事件，回执缺口由 manual_compact 补上）。
    #[test]
    fn manual_compact_receipts_zero_when_nothing_to_compress() {
        let cfg = cfg_for_tests();
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"hi"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"hello"}]}),
        ];
        let before = history.clone();
        let (ev, compacted) =
            manual_compact(&cfg, &mut history, &std::env::temp_dir(), tools::MemoryScope::Global);
        let ev = ev.expect("压不动也要补发回执");
        assert!(!compacted, "没压动就不该触发记忆快照刷新");
        assert_eq!(ev["type"], "system");
        assert_eq!(ev["subtype"], "context_compacted");
        assert_eq!(ev["elided"], 0);
        assert_eq!(ev["dropped"], 0);
        assert_eq!(history, before, "没有可压缩内容时历史必须逐字节不变");
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

    // ── E1 子代理内部过程转发：数据通道契约（2026-10-16）──────────────────
    //
    // 纯逻辑单测：`SubtaskForward::events()` 不碰 stdout，直接钉住「标识 / 类别 / 保序 /
    // 单条与整段上限 / 静默」五条不变量。真实发送走 `send()`（薄到只有一句 `for emit`）。

    /// 一条 assistant 内部消息 → 一个 `task_event`：带 `task_id`，块类型与顺序原样保留，
    /// 字段名与主流程 `assistant` 事件**逐字对齐**（`message.content[].type` 等）。
    #[test]
    fn subtask_forward_tags_identifier_and_preserves_order() {
        let mut f = SubtaskForward::new();
        let evs = f.events(
            "task-7",
            "assistant",
            &[
                json!({"type":"thinking","thinking":"先想一下"}),
                json!({"type":"text","text":"说一句"}),
                json!({"type":"tool_use","id":"tu1","name":"Read","input":{"file_path":"a.rs"}}),
            ],
        );
        assert_eq!(evs.len(), 1, "一条内部消息 = 一个 task_event");
        let ev = &evs[0];
        assert_eq!(ev["type"], "system");
        assert_eq!(ev["subtype"], "task_event");
        assert_eq!(ev["task_id"], "task-7", "每条都必须带子任务标识");
        assert_eq!(ev["message"]["role"], "assistant");
        let c = ev["message"]["content"].as_array().unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0]["type"], "thinking");
        assert_eq!(c[0]["thinking"], "先想一下");
        assert_eq!(c[1]["type"], "text");
        assert_eq!(c[2]["type"], "tool_use");
        assert_eq!(c[2]["name"], "Read");
        assert_eq!(c[2]["input"]["file_path"], "a.rs");
    }

    /// 工具结果消息 → user 角色 `task_event`，逐块带 `tool_use_id`；`is_error` 只在为真时出现。
    #[test]
    fn subtask_forward_carries_tool_results() {
        let mut f = SubtaskForward::new();
        let evs = f.events(
            "task-1",
            "user",
            &[
                json!({"type":"tool_result","tool_use_id":"tu1","content":"ok"}),
                json!({"type":"tool_result","tool_use_id":"tu2","content":"boom","is_error":true}),
            ],
        );
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0]["task_id"], "task-1");
        assert_eq!(evs[0]["message"]["role"], "user");
        let c = evs[0]["message"]["content"].as_array().unwrap();
        assert_eq!(c.len(), 2, "按调用顺序原样保留");
        assert_eq!(c[0]["type"], "tool_result");
        assert_eq!(c[0]["tool_use_id"], "tu1");
        assert!(c[0].get("is_error").is_none(), "成功结果不带 is_error");
        assert_eq!(c[1]["tool_use_id"], "tu2");
        assert_eq!(c[1]["is_error"], true);
    }

    /// 单条超长文本 ⇒ 按 `SUBAGENT_EVENT_ITEM_CHARS` 截断并带 `…(truncated)` 标记（不静默丢）。
    #[test]
    fn subtask_forward_truncates_long_text_with_marker() {
        let mut f = SubtaskForward::new();
        let long = "x".repeat(SUBAGENT_EVENT_ITEM_CHARS + 500);
        let evs = f.events("task-1", "assistant", &[json!({"type":"text","text":long})]);
        let text = evs[0]["message"]["content"][0]["text"].as_str().unwrap();
        assert!(text.ends_with("…(truncated)"), "超长必须带明确标记: {text}");
        assert_eq!(text.chars().count(), SUBAGENT_EVENT_ITEM_CHARS + "…(truncated)".chars().count());
    }

    /// 超大 tool_use 入参 ⇒ **保持对象结构**、只截断超长字符串叶子；短字段（如 `file_path`）
    /// 原样存活 —— 前端据此解析改动文件、采到回退原件（E4）。
    #[test]
    fn subtask_forward_keeps_tool_input_shape_and_path() {
        let mut f = SubtaskForward::new();
        let big = "z".repeat(SUBAGENT_EVENT_ITEM_CHARS * 2);
        let evs = f.events(
            "task-1",
            "assistant",
            &[json!({
                "type":"tool_use","id":"t","name":"Write",
                "input":{"file_path":"C:\\a\\b.rs","content":big}
            })],
        );
        let input = &evs[0]["message"]["content"][0]["input"];
        assert!(input.is_object(), "入参必须保持对象，否则路径解析不出来: {input}");
        assert_eq!(input["file_path"], "C:\\a\\b.rs", "短路径字段须原样存活");
        let content = input["content"].as_str().unwrap();
        assert!(content.ends_with("…(truncated)"), "超长内容须带标记: {content}");
        assert_eq!(
            content.chars().count(),
            SUBAGENT_EVENT_ITEM_CHARS + "…(truncated)".chars().count()
        );
    }

    /// 整段超预算 ⇒ 发**一条** `truncated:true` 标记后**不再出内容事件**（封顶 + 明确标记）。
    #[test]
    fn subtask_forward_total_budget_marks_then_stops() {
        let mut f = SubtaskForward::new();
        let chunk = "y".repeat(SUBAGENT_EVENT_ITEM_CHARS * 3);
        let mut content_events = 0;
        let mut markers = 0;
        for _ in 0..20 {
            for ev in f.events(
                "task-1",
                "assistant",
                &[
                    json!({"type":"text","text":chunk}),
                    json!({"type":"text","text":chunk}),
                    json!({"type":"text","text":chunk}),
                ],
            ) {
                if ev.get("truncated").and_then(Value::as_bool).unwrap_or(false) {
                    markers += 1;
                } else {
                    assert_eq!(markers, 0, "标记之后不得再出内容事件");
                    content_events += 1;
                }
            }
        }
        assert!(content_events > 0, "预算内应有内容事件");
        assert_eq!(markers, 1, "整段预算用尽只发一条标记");
    }

    /// 整段上限须与「子代理轮数上限」**匹配**（防止后人再把整段上限调回与轮数脱节的过小值）：
    /// 至少要覆盖 `MAX_SUBAGENT_ROUNDS` 轮 × 每轮最多 2 条转发消息 × 单条上限
    /// `SUBAGENT_EVENT_ITEM_CHARS`。轮数从 8 提到 20 后，旧的 2_000_000（≈10 条满额块）会在
    /// 正常的多轮子代理上更早撞顶 ⇒ 本断言把「上限 ↔ 轮数」的匹配关系钉住。
    #[test]
    fn subtask_forward_total_budget_matches_max_rounds() {
        assert!(
            SUBAGENT_EVENT_TOTAL_CHARS >= MAX_SUBAGENT_ROUNDS * 2 * SUBAGENT_EVENT_ITEM_CHARS,
            "整段上限须覆盖「轮数上限 × 每轮 2 条 × 单条上限」：{} < {}",
            SUBAGENT_EVENT_TOTAL_CHARS,
            MAX_SUBAGENT_ROUNDS * 2 * SUBAGENT_EVENT_ITEM_CHARS,
        );
    }

    /// 转发策略（单一真相源）：`Agent` 工具调用与 fork 技能都开（前端都有落点）、后台复盘保持静默
    ///（静默不变量：无人值守的后台过程零 `task_event`）。
    #[test]
    fn subtask_forward_policy_per_path() {
        assert!(
            subtask_forwarder(AGENT_EMIT_SUBTASK).is_some(),
            "用户可见的 Agent 工具调用必须转发"
        );
        assert!(
            subtask_forwarder(FORK_SKILL_EMIT_SUBTASK).is_some(),
            "fork 技能现在也转发内部过程"
        );
        assert!(
            subtask_forwarder(REVIEW_EMIT_SUBTASK).is_none(),
            "后台复盘必须保持零 task_event"
        );
    }

    // ── 子代理撞墙：轮数上限 / 收口提示 / 部分报告可见性（2026-10-16）──────────

    /// 行为证据 ①＋④：默认轮数 = 20；复盘 fork 仍 4 轮且保持静默。
    #[test]
    fn subagent_round_ceiling_is_20_and_review_stays_4_silent() {
        assert_eq!(MAX_SUBAGENT_ROUNDS, 20, "子代理的轮数上限应为 20");
        assert_eq!(MAX_REVIEW_ROUNDS, 4, "复盘 fork 的轮数上限必须保持 4");
        assert!(!REVIEW_EMIT_SUBTASK, "复盘 fork 必须保持静默（零 task_event）");
        assert!(AGENT_EMIT_SUBTASK && FORK_SKILL_EMIT_SUBTASK, "前台两条路径都转发");
    }

    /// 行为证据 ②：收口提示**恰在倒数第二轮**（`round == max_rounds - 1`）注入一次，
    /// 且只拼进那一轮的 `tool_result` 文本内部，别的消息一个字不动。
    #[test]
    fn wrap_hint_fires_once_on_penultimate_round_only() {
        let max = MAX_SUBAGENT_ROUNDS;
        let due: Vec<usize> =
            (1..=max).filter(|r| subagent_wrap_hint_due(*r, max)).collect();
        assert_eq!(due, vec![max - 1], "只在倒数第二轮触发，且只触发一次");

        // 模拟每轮入口：只有 round == max-1 那一轮末条 tool_result 被改。
        let mut history = vec![
            json!({"role":"user","content":[{"type":"text","text":"task"}]}),
            json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"t1","content":"R1"}
            ]}),
        ];
        for round in 1..=max {
            if subagent_wrap_hint_due(round, max) {
                assert!(append_hint_to_last_tool_result(&mut history, SUBAGENT_WRAP_HINT));
            }
        }
        let blocks = history[1]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 1, "不许新增内容块");
        let text = blocks[0]["content"].as_str().unwrap();
        assert!(text.starts_with("R1\n\n"), "是拼进既有 tool_result 文本内部");
        // 按**常量原文**计数：改收口语的措辞不该让这条测试失效（2026-10-27 三段式收口）。
        assert_eq!(text.matches(SUBAGENT_WRAP_HINT).count(), 1, "全程只注入一次");
        assert_eq!(history[0]["content"][0]["text"], "task", "别的消息不许动");

        // 只有一轮的 fork 没有「最后一轮」可言 ⇒ 不注入。
        assert!(!subagent_wrap_hint_due(1, 1));
        assert!(!subagent_wrap_hint_due(0, 1));
    }

    /// 预算口径（2026-10-24）：缓存命中只按 1/50 计，输出按 4 倍计。
    /// 旧口径下 5M cache_read 会把 300k 预算一口气吃光；新口径只算 100k。
    #[test]
    fn subagent_budget_weights_cache_read_and_output() {
        assert_eq!(subagent_usage_units(1_000, 0, 0, 0), 1_000);
        assert_eq!(subagent_usage_units(0, 100, 0, 0), 400);
        assert_eq!(subagent_usage_units(0, 0, 5_000, 0), 100);
        assert_eq!(subagent_usage_units(0, 0, 0, 400), 500);
        assert_eq!(subagent_usage_units(0, 0, 5_000_000, 0), 100_000);
    }

    /// 收口线：预算用掉 80% 即触发（与轮数倒数第二取先到者）。
    #[test]
    fn budget_wrap_fires_at_eighty_percent() {
        let budget = SUBAGENT_BUDGET_TOKENS;
        assert!(!subagent_budget_wrap_due(0, budget));
        assert!(!subagent_budget_wrap_due(budget * 79 / 100, budget));
        assert!(subagent_budget_wrap_due(budget * 80 / 100, budget));
        assert!(subagent_budget_wrap_due(budget, budget));
        assert!(!subagent_budget_wrap_due(10, 0), "零预算不应反复触发");
    }

    /// 角色常驻（§待办 3-22 分片 4）：旧会话**原样前置**、本轮提问**恰好是最后一条**。
    /// 顺序错了子代理会把上一轮的报告当成本轮的答案。
    #[test]
    fn subagent_seed_history_appends_the_prompt_after_the_resumed_turns() {
        let prompt = "next task";
        let fresh = subagent_seed_history(&[], prompt);
        assert_eq!(fresh.len(), 1, "没有旧会话时与改造前完全一致");
        assert_eq!(fresh[0]["role"], "user");
        assert_eq!(fresh[0]["content"][0]["text"], prompt);

        let prior = vec![
            json!({"role":"user","content":[{"type":"text","text":"q1"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"a1"}]}),
        ];
        let seeded = subagent_seed_history(&prior, prompt);
        assert_eq!(&seeded[..2], &prior[..], "旧消息必须逐字段前置");
        assert_eq!(seeded.len(), 3);
        assert_eq!(seeded[2]["role"], "user");
        assert_eq!(seeded[2]["content"][0]["text"], prompt);
    }

    /// 行为证据 ③（值部分）：`task_done.partial` 只在撞墙（`Ok` 且 `partial=true`）时为真；
    /// 自然收尾（`partial=false`）与整体失败（`Err`）都不是 `true`。
    #[test]
    fn task_done_event_marks_partial_only_on_wall() {
        let full: Result<SubagentOutcome, String> = Ok(SubagentOutcome {
            text: "done".into(),
            partial: false,
            history: Vec::new(),
            spent: 0,
        });
        let ev = task_done_event("task-1", &full, 12);
        assert_eq!(ev["type"], "system");
        assert_eq!(ev["subtype"], "task_done");
        assert_eq!(ev["task_id"], "task-1");
        assert_eq!(ev["ok"], true);
        assert_eq!(ev["partial"], false, "未撞墙时 partial 不为 true");
        assert_eq!(ev["ms"], 12);

        let wall: Result<SubagentOutcome, String> = Ok(SubagentOutcome {
            text: "partial".into(),
            partial: true,
            history: Vec::new(),
            spent: 0,
        });
        let ev = task_done_event("task-2", &wall, 34);
        assert_eq!(ev["ok"], true, "撞墙仍是 Ok 分支 ⇒ ok 为真");
        assert_eq!(ev["partial"], true, "撞墙时 partial 必须为 true");

        let err: Result<SubagentOutcome, String> = Err("boom".into());
        let ev = task_done_event("task-3", &err, 5);
        assert_eq!(ev["ok"], false);
        assert_eq!(ev["partial"], false, "失败不是「部分报告」");
    }

    /// 行为证据 ③（日志部分）：撞墙日志的单一真相源带齐 `task_id`、上限种类与用量
    /// （轮数 / token 两条路径共用），使「撞墙必留日志」可被单测钉住。
    #[test]
    fn subagent_wall_note_names_task_limit_and_usage() {
        let rounds =
            subagent_wall_note("task-7", "rounds", MAX_SUBAGENT_ROUNDS as u64, MAX_SUBAGENT_ROUNDS, 123_456);
        assert!(rounds.contains("task-7"), "{rounds}");
        assert!(rounds.contains("rounds"), "要写清撞的是轮数上限: {rounds}");
        assert!(rounds.contains("123456"), "要带已花 token: {rounds}");

        let tokens = subagent_wall_note("task-7", "budget", SUBAGENT_BUDGET_TOKENS, 14, 300_001);
        assert!(tokens.contains("budget"), "要写清撞的是预算: {tokens}");
        assert!(tokens.contains("300001"), "要带已花 token: {tokens}");
        assert!(tokens.contains("14"), "要带触发轮次: {tokens}");
    }

    /// 2026-11-02：硬收尾必须留下**非空**的 final_text（之前模型只调工具时它是空串，
    /// `result.result` 为空 ⇒ 前端没有任何可显示的内容，用户看到「无缘无故停了」）。
    #[test]
    fn hard_stop_reason_is_never_lost() {
        let mut empty = String::new();
        append_hard_stop(&mut empty, "本轮已停止：成本预算用尽。");
        assert_eq!(empty, "本轮已停止：成本预算用尽。");

        let mut with_text = "已经写好的正文".to_string();
        append_hard_stop(&mut with_text, "本轮已停止：工具轮次达到上限。");
        assert!(with_text.starts_with("已经写好的正文"), "不能覆盖已有正文: {with_text}");
        assert!(with_text.contains("工具轮次"), "要追加停止原因: {with_text}");
    }
}
