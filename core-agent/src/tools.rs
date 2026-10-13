// core-agent/src/tools.rs
// 内置工具：Read / Write / Edit / NotebookEdit / Cmd / PowerShell / Glob / Grep / WebSearch / WebFetch / AskUserQuestion / TodoWrite
//
// 工具名保持 PascalCase —— 前端 main.ts 对 "Cmd" / "PowerShell" 有专门的
// 命令展示与危险命令分类分支（agentToolArgsDelta / classifyRequest /
// findLastCmdGroup），改名会破坏既有 UI 契约。
//
// **2026-10-05 改名（用户要求）**：原 `Bash` 改名成 `Cmd` —— 它在 Windows 上本来就是
// `cmd /C`（见 `cmd()`），叫 `Bash` 是误导。旧名**不再注册** ⇒ 用户 `hooks.json` 里写
// `"matcher": "Bash"` 的钩子、以及 `--disallowedTools Bash` 都会失效，需改成 `Cmd`。
//
// 权限策略（P1 无审批通道，按启动参数静态裁决；P2 接入 can_use_tool 后
// 改为「先问前端，再执行」）：
//   · --permission-mode plan      → 只读：Write / Edit / Cmd / PowerShell 直接拒绝
//   · 其余档位（默认 acceptEdits）→ 内置工具全开
//   · LUNAC_WORKSPACE_LOCKED=1    → 文件类工具限制在工作区内，越界拒绝
//   · --dangerously-skip-permissions → 忽略工作区锁
//
// 本文件为 Lunac 自研实现，不派生自任何第三方源码。

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use serde_json::{json, Value};

/// 单条工具输出的**内联预算**（字符）：超过就把全文落盘、只内联「头 + 尾 + 路径」。
/// 旧实现是硬截断，超出部分对模型永久消失 —— 见 `apply_budget`。
const SPILL_THRESHOLD: usize = 12_000;
/// 预览保留的头部字符数
const INLINE_HEAD_CHARS: usize = 8_000;
/// 预览保留的尾部字符数（构建/报错的结论通常压在末尾，尾部比中部值钱）
const INLINE_TAIL_CHARS: usize = 2_000;
/// 落盘输出的保留天数（与落盘日志同口径）
const KEEP_DAYS: u64 = 7;
/// 落盘正文的字节上限 —— **必须低于 Read / Grep 的 `MAX_TEXT_BYTES`(2MB) 文件门槛**，
/// 否则模型拿不回自己落盘的文件（那两个工具遇到超大文件是直接拒绝/跳过）。
const MAX_SPILL_BYTES: usize = 1_500_000;
/// Read 一次最多返回的行数
const MAX_READ_LINES: usize = 2000;
/// Glob / Grep 的结果条数上限
const MAX_ENTRIES: usize = 200;
/// 文本文件大小上限（超过视为二进制/不适合阅读）
const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;
/// 子进程 stdout/stderr 的捕获上限
const MAX_PIPE_BYTES: usize = 512 * 1024;
/// Cmd 默认/最大超时（毫秒）
const CMD_TIMEOUT_MS: u64 = 120_000;
const CMD_MAX_TIMEOUT_MS: u64 = 600_000;

/// **Read 读到图片时的哨兵**（2026-10-05，用户要求「Read 遇图不要返回乱码」）。
///
/// 形态：`哨兵 + 绝对路径 + '\n' + 人/模型可读的说明`。`read()` 只负责贴这个头；
/// 真正把它换成 `image` 块的是上层 `main.rs` 的 `tool_result_block`（同一份常量，
/// 两处**共用这个 `pub const`**，不许各写一份字面量 —— 那是典型的必然漂移）。
pub const IMAGE_SENTINEL: &str = "[[lunac-image]]\n";

/// **子进程退出之后**，再等读线程把管道读干（EOF）的宽限期（2026-10-03）。
///
/// 正常情况下子进程一退出、写端就关了，EOF 立刻到、读线程几毫秒内结束。但如果命令里
/// **detached 启动了一个常驻进程**（ComfyUI / 后台服务），那个进程会继承写端 ⇒ EOF 永不
/// 到来。此时**绝不能无限等**：宽限期一到就按「转后台」移交（见 `run_shell` 末段与
/// `join_within`）。3 秒足够把 OS 管道缓冲里剩下的那点输出读完。
pub const OUTPUT_JOIN_GRACE: Duration = Duration::from_secs(3);
/// Glob 递归深度上限
const MAX_DEPTH: usize = 12;
/// WebFetch：单次抓取的响应体积上限、超时、重定向上限、URL 长度上限
const FETCH_MAX_BYTES: u64 = 10 * 1024 * 1024;
const FETCH_TIMEOUT_SECS: u64 = 60;
const FETCH_MAX_REDIRECTS: usize = 10;
const MAX_URL_CHARS: usize = 2000;
/// WebFetch 的 UA —— 明确标识自己是 Lunac，不冒充其它客户端
const FETCH_USER_AGENT: &str = concat!("Lunac/", env!("CARGO_PKG_VERSION"));

// ── WebSearch ────────────────────────────────────────────────────
//
// 两级：主源 = 可配置的搜索 API（需 key），兜底 = 抓 Bing / 百度结果页（无需 key）。
//
// 为什么兜底不用 DuckDuckGo：实测本机（国内）连不上 html.duckduckgo.com（15s 超时，
// lite 版同样超时），「不配 key 也能搜」会变成空话；Bing（www / cn 均可达）与百度均通。
const BOCHA_ENDPOINT: &str = "https://api.bochaai.com/v1/web-search";
const TAVILY_ENDPOINT: &str = "https://api.tavily.com/search";
const EXA_ENDPOINT: &str = "https://api.exa.ai/search";
const FIRECRAWL_ENDPOINT: &str = "https://api.firecrawl.dev/v2/search";
const BING_SEARCH_ENDPOINT: &str = "https://www.bing.com/search";
const BAIDU_SEARCH_ENDPOINT: &str = "https://www.baidu.com/s";
/// 抓取类兜底源的最小间隔（秒）—— 官方未公开限流值，社区经验 1 req/s 是安全上限
const SCRAPE_MIN_INTERVAL_SECS: f64 = 1.1;
const SEARCH_TIMEOUT_SECS: u64 = 20;
const SEARCH_MAX_BYTES: u64 = 4 * 1024 * 1024;
const SEARCH_DEFAULT_COUNT: usize = 5;
const SEARCH_MAX_COUNT: usize = 10;
/// 单条摘要的字符上限 —— Exa 的 `text` 会带回整页正文，不截断一条就能吃光预算
const SEARCH_SNIPPET_CHARS: usize = 400;
/// 抓取类兜底源的 UA：Bing / 百度只对浏览器 UA 返回正常结果页（非浏览器 UA 给降级空壳）
const SCRAPE_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
/// 抓取类兜底源共用的 `Accept` —— 用浏览器那一串。**百度会拿它判「是不是浏览器」**：
/// 少一个 `Accept` 就回 1488 字节的验证页（见 `scrape_get` 的注释）。
const SCRAPE_ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8";

/// 目录遍历时跳过的常见重目录（避免 Glob/Grep 卡在依赖上）。
/// **2026-10-12 起 `pub(crate)`**：子目录记忆懒加载（`main.rs` 的 `eligible_subdir`）复用这份
/// 忽略名单，不另发明第二张表（同一套约定）。
pub(crate) const SKIP_DIRS: [&str; 13] = [
    ".git", "node_modules", "target", "dist", "out", ".next", ".nuxt", ".venv", "venv",
    "__pycache__", ".idea", ".vscode", "build",
];

/// 记忆作用域（2026-10-13）：主 AI 的**全局**长期记忆 vs **桌宠私有**的成长笔记。
///
/// 由**本轮的来源**决定（前端在 user 消息里带 `source`，见 main.rs 的 `turn_memory_scope`）：
/// 桌宠发起的那一轮读写桌宠私有那一份，其余一轮用全局那一份 —— 宿主侧两份各一个文件
/// （见 `mcp_server::memory_path_for`），互不注入（作用域隔离）。
///
/// 放在 `Ctx` 上而不是单开一个参数：`Ctx` 已被子代理 / 后台复盘 fork **克隆继承**（见其注释），
/// 于是在桌宠轮里派生的复盘 / 子代理**自动写回同一作用域**，不必逐层传参。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemoryScope {
    Global,
    Pet,
}

impl MemoryScope {
    /// 传给宿主桥方法 `lunac/memory_*` 的 `scope` 值（宿主侧 [`MemoryScope::parse`] 认这一对）。
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryScope::Global => "global",
            MemoryScope::Pet => "pet",
        }
    }
}

/// 工具执行上下文（由 main.rs 依据启动参数构建）。
///
/// `Clone` 是为了后台复盘 fork：它在**另一条线程**上跑（见 main.rs 的 `run_review_fork`），
/// 拿不到主循环那份 `&Ctx`（也不是 `'static` 借用）。克隆一份只含路径与两个布尔量，
/// 代价可忽略 —— 但语义上必须是**同一份工作区视图**，否则 fork 里的读写在另一个边界下。
#[derive(Clone)]
pub struct Ctx {
    /// 进程工作目录 —— src-tauri 用它传「AI 工作区」
    pub cwd: PathBuf,
    /// 可访问目录：`--add-dir` 追加的 + 工具输出落盘目录（见 `prepare_output_dir`）
    pub add_dirs: Vec<PathBuf>,
    /// `--permission-mode plan` → 只读（**启动时定死**的用户档位）
    pub read_only: bool,
    /// 越界拦截开关（工作区已配置时为真）
    pub locked: bool,
    /// **有效工作目录的覆盖层**（2026-10-11，`EnterWorktree` / `ExitWorktree`）。
    ///
    /// `cwd` 是**进程级常量**（工作区根，也是系统提示词里那条），
    /// 本字段是**独立的一层覆盖**：`EnterWorktree` 成功后设成 worktree 的绝对路径，
    /// `ExitWorktree` 清回 `None`。**只有三类「寻址」用途读它**（见 [`effective_cwd`]）：
    /// 相对路径从哪解析（[`resolve`]）、shell 在哪个目录跑（[`run_shell`]）、
    /// Glob / Grep 的默认根。**不重建系统提示词**（规则 18 前缀不变量；工具返回文本负责把
    /// 新目录告诉模型）。
    ///
    /// ⚠️ **落点在数据根、不在工作区里**（2026-10-11 改）：worktree 建在
    /// `<exe 根>\temp\worktrees\`（见 [`worktree_root`]），**已不在 `cwd` 之内** —— 故该根
    /// 目录由 main.rs 并进 `Ctx.add_dirs`（与 `output_dir` / `image_dir` 同一机制），否则
    /// `guard` 会把 worktree 内的写直接拒掉。**只放行这一个应用自己的目录**，不放宽
    /// 用户工作区的边界。
    ///
    /// 为什么是 `Arc<Mutex<..>>`：与 `plan_phase` 同 —— `Ctx` 是 `Clone` 且要跨线程
    ///（只读并行批用 `&Ctx`、后台复盘 / 子代理拿克隆），值语义会各持一份。
    pub worktree: Arc<Mutex<Option<PathBuf>>>,
    /// **计划相位**（2026-09-20，原 backlog A7）：模型自己调 `EnterPlanMode` 声明
    /// 「只出计划、不动手」，`ExitPlanMode` 被用户批准后解除。
    ///
    /// 为什么在这里、为什么是 `Arc<AtomicBool>`：
    ///   · 它要和 `read_only` **在同一个地方被同一个判据读到**（见 `write_blocked`），
    ///     否则「哪些工具算写类」这份知识会散到 `main.rs` 的五个调用点上去；
    ///   · `Ctx` 是 `Clone` 且要能跨线程（只读工具并行批用 `&Ctx`、后台复盘 fork 拿
    ///     克隆）⇒ 值语义的 `bool` 会各持一份、`Cell` 会破 `Sync`，只有
    ///     `Arc<AtomicBool>` 既共享状态又满足 `Send + Sync`；
    ///   · 派生出去的子代理 / fork 技能 / 后台复盘**自动继承**计划相位 —— 正是想要的：
    ///     计划相位里它们同样不该写。
    ///
    /// 与 `read_only` 的区别（**两回事，别混**）：`read_only` 是**用户**在设置里选的边界，
    /// 要重启 agent 才变，且只读档下写类工具**永远**被拒；计划相位是**模型自己**的临时承诺，
    /// 进程内即时生效，`ExitPlanMode` 被批准后立刻解除。两者的**拒**共用同一个出口。
    pub plan_phase: Arc<AtomicBool>,
    /// **实时输出回调**（2026-09-30，用户要求「命令卡的输出能边跑边看到」）：
    /// 命令类工具（Cmd / PowerShell）把 stdout / stderr 的分片**边跑边**交给宿主
    /// （宿主逐行转发成 `cli-output`，前端追加到命令卡上）。
    ///
    /// 只有 `run_one_tool` 会为**某一次调用**装上它 —— 闭包里绑死那次调用的
    /// `tool_use_id`（见 main.rs 的 `tool_output_sink`）。其余构造点（单测、后台复盘
    /// fork、子代理）一律保持 `None`，行为与改造前逐字节一致（**不新增任何事件**）。
    ///
    /// 类型必须 `Send + Sync`：drain 是在**独立线程**上读管道的，回调要跨线程持有。
    pub tool_output: Option<ToolOutputSink>,
    /// **本次调用的实时控制块**（2026-10-01，用户要求「命令卡上加后台运行 / 停止」）。
    ///
    /// 只有 `Cmd` / `PowerShell` 会被装上：`run_one_tool` 为每一次调用建一个并登记进
    /// `main::SHELL_CONTROLS`（键 = `tool_use_id`），用户在前端点按钮时由 stdin 上的
    /// `tool_control` 消息按 id 找到它、置位标志；`run_shell` 的**轮询每一拍**都看这两个
    /// 标志 —— 这是运行中的工具**唯一**能被协作式中断的地方。
    ///
    /// 其余构造点（单测 / 子代理 / 后台复盘）一律 `None`，行为与改造前逐字节一致
    /// —— 与 `tool_output` 同一条纪律：**不给它就不新增任何能力**（也顺带防住「子代理
    /// 里的命令被主对话的按钮误杀」这种串台）。
    pub shell_control: Option<Arc<ShellControl>>,
    /// **本轮的记忆作用域**（2026-10-13，见 [`MemoryScope`]）：main.rs 在每个 user 轮开始前
    /// 按「本轮来源」写进这份 `Ctx`；`Remember` 用它决定写全局还是桌宠私有那一份。
    ///
    /// 为什么在 `Ctx` 上：子代理 / 后台复盘 fork 拿的是**克隆**（见本结构体注释）
    /// ⇒ 它们自动继承本轮作用域，桌宠轮里派生的复盘也写桌宠那一份，不必逐层传参。
    /// `Copy` 值语义足够 —— 它是**每轮**的事实，不需要 `Arc` 共享。
    pub memory_scope: MemoryScope,
}

/// 实时输出回调：`(stream, chunk)`，`stream ∈ {"stdout", "stderr"}`。
pub type ToolOutputSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// 「这条命令还能不能被打断」的控制块（2026-10-01）。
///
/// **两个独立 `AtomicBool`，不是一个枚举**：两者由不同的按钮置位、在 `run_shell`
/// 循环的**不同位置**消费，而且「转后台」一旦生效就要把 `Child` 的所有权移走、
/// 「停止」则就地 kill —— 各自独立最好读，也不会出现「谁先谁后」的歧义。
///
/// 置位方只有一处（main.rs 的 `route_tool_control`），消费方只有一处（`run_shell`）；
/// 两个标志都**只置位、不复位** —— 一次调用对应一个控制块，用完即弃。
#[derive(Default)]
pub struct ShellControl {
    /// 这次调用的 `tool_use_id`。**带着它一起走**（而不是让调用方另记一份）：
    /// 转后台之后 `run_shell` 已经不在了，后台线程要拿它去登记「后台命令列表」、
    /// 并在完成时按它找回前端那张卡片。
    pub tool_use_id: String,
    /// 用户点了「后台运行」⇒ `run_shell` 把 `Child` 与两个读线程移交出去、立刻返回
    /// （**不 kill** —— 那正是「后台」的意思）。
    pub background: AtomicBool,
    /// 用户点了「停止」⇒ `run_shell`（或接管后的后台线程）kill 子进程，并如实回一句。
    pub stop: AtomicBool,
    /// **已经移交给后台**（`run_shell` 亲手置位，就在它 `return` 之前）。
    ///
    /// 存在的唯一理由：`run_one_tool` 收尾时要决定**要不要把这条从
    /// `SHELL_CONTROLS` 里注销** —— 跑完 / 被停的必须注销（否则表里留僵尸），
    /// 而转后台的**绝不能**注销（命令还在跑，用户仍要从待办清单里取消它，
    /// 而取消走的就是那张表）。注销它的是后台收尾线程。
    pub handed_off: AtomicBool,
    /// **交互式输入通道**（2026-10-05，用户要求「命令卡改成可键入的 cmd/PowerShell 终端」）：
    /// 子进程的 stdin 写端。只有装了控制块（= 有前端在看着那次调用）时才会是 `Some`
    /// —— 单测 / 子代理 / 后台复盘的子进程 stdin 仍是 `Stdio::null()`，行为逐字节不变。
    ///
    /// 前端在终端里键入的字符经 `tool_control`（`action:"stdin"`）流到这里，由
    /// `write_stdin` 落笔。`Mutex` 是必须的：写方在**主循环线程**（`route_tool_control`），
    /// 而读/移交方在**工具线程**（`run_shell`）—— 两边都要碰它。
    ///
    /// **不做行编辑**：这里只是把字节原样灌进管道，没有 ConPTY，所以方向键 / Tab 补全 /
    /// 退格这类由控制台完成的编辑动作**不会**生效（前端只做本地回显）。这是本轮明确选定的
    /// 边界（用户选「stdin 转发」而非「真 ConPTY」）。
    pub stdin: Mutex<Option<ChildStdin>>,
}

impl ShellControl {
    pub fn new(tool_use_id: &str) -> Self {
        Self { tool_use_id: tool_use_id.to_string(), ..Default::default() }
    }

    /// 装上子进程的 stdin 写端（`run_shell` spawn 成功之后立刻调用一次）。
    pub fn set_stdin(&self, w: ChildStdin) {
        if let Ok(mut g) = self.stdin.lock() {
            *g = Some(w);
        }
    }

    /// 把用户键入的字节写给子进程。返回是否真的送达。
    ///
    /// 进程已退出（管道写端已关）时返回 `false` 并**丢掉写端**，避免后续每次按键都撞墙。
    /// 注意：这是**同步写**，若用户粘贴的量超过管道缓冲（Windows 约 64KB）且子进程不读，
    /// 调用方会短暂阻塞 —— 手敲字符的量级不受影响。
    pub fn write_stdin(&self, data: &[u8]) -> bool {
        use std::io::Write;
        if data.is_empty() {
            return false;
        }
        if let Ok(mut g) = self.stdin.lock() {
            if let Some(w) = g.as_mut() {
                if w.write_all(data).is_ok() {
                    let _ = w.flush();
                    return true;
                }
                *g = None;
            }
        }
        false
    }
}

/// 「写类操作现在能不能做」的统一判据（两档共用一个出口，差别只在措辞）。
///
/// 参数 `what` = 工具名（或「派子代理」这类动作名），只用于把拒绝理由说清楚。
/// 返回值 = **拒绝原因**（`None` = 放行）。措辞必须分开，因为**用户该做的动作不同**：
///   · 只读档 → 去设置里把安全档位改成「项目」（要重启 agent）；
///   · 计划相位 → 先出计划、调 `ExitPlanMode` 让用户批准。
/// 说错这一句，模型会去调一个在当前档位下永远不可能成功的动作。
pub fn write_blocked(ctx: &Ctx, what: &str) -> Option<String> {
    if ctx.read_only {
        return Some(format!(
            "{what} is disabled in read-only (plan) mode — the user must switch the security \
             profile to \"project\" in Lunac settings to allow writes."
        ));
    }
    if ctx.plan_phase.load(Ordering::Relaxed) {
        return Some(format!(
            "{what} is disabled while a plan is pending approval. Present the plan with \
             ExitPlanMode and wait for the user to approve it before changing anything."
        ));
    }
    None
}

// ── 工具定义（Anthropic Messages API 的 tools schema）─────────────

/// 二十一个内置工具的 schema；`disallowed`（来自 `--disallowedTools`）里的
/// 名字不进入请求体 —— 数组更短，也少一轮缓存失效。
pub fn defs(disallowed: &[String]) -> Vec<Value> {
    let all = vec![
        json!({
            "name": "Read",
            "description": "Read a UTF-8 text file from the local filesystem. Returns the \
                content with 1-based line numbers. Use offset/limit for large files.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute or workspace-relative path" },
                    "offset": { "type": "integer", "description": "First line to return (1-based)" },
                    "limit": { "type": "integer", "description": "Maximum number of lines" }
                },
                "required": ["file_path"]
            }
        }),
        json!({
            "name": "Write",
            "description": "Write a file (creates parent directories). Overwrites existing content.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["file_path", "content"]
            }
        }),
        json!({
            "name": "Edit",
            "description": "Replace an exact string in a file. Fails when old_string is missing \
                or appears more than once (set replace_all to replace every occurrence).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": { "type": "string" },
                    "old_string": { "type": "string" },
                    "new_string": { "type": "string" },
                    "replace_all": { "type": "boolean", "description": "Replace every occurrence" }
                },
                "required": ["file_path", "old_string", "new_string"]
            }
        }),
        json!({
            "name": "NotebookEdit",
            "description": "Edit one cell of a Jupyter notebook (.ipynb) in place. Target a \
                cell by cell_id (preferred) or by its 0-based cell_number, then pick edit_mode: \
                replace (default) rewrites that cell's source, insert adds a new cell at that \
                position (cell_type is required; with cell_id the new cell goes right after \
                it), delete removes the cell. Use this instead of Write/Edit for .ipynb — it \
                understands the notebook JSON structure, so the other cells, metadata and \
                nbformat are left untouched. new_source is the COMPLETE new body of the cell \
                (the whole code or markdown), not a fragment.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "notebook_path": { "type": "string", "description": "Absolute or workspace-relative path to the .ipynb file" },
                    "cell_id": { "type": "string", "description": "The target cell's id (preferred). For insert, the new cell is placed right after this cell." },
                    "cell_number": { "type": "integer", "description": "0-based index of the target cell. For insert, the new cell is inserted at this index (a value equal to the cell count appends)." },
                    "new_source": { "type": "string", "description": "The cell's complete new source. Required for replace and insert; ignored for delete." },
                    "cell_type": { "type": "string", "enum": ["code", "markdown"], "description": "Cell type. Required for insert; for replace it switches the cell's type when given." },
                    "edit_mode": { "type": "string", "enum": ["replace", "insert", "delete"], "description": "What to do with the cell (default replace)" }
                },
                "required": ["notebook_path", "new_source"]
            }
        }),
        json!({
            "name": "Cmd",
            "description": "Run a Windows cmd.exe command (cmd /C) in the working directory \
                and return its combined output. Long-running commands are killed on timeout.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "timeout": { "type": "integer", "description": "Timeout in milliseconds (default 120000, max 600000)" }
                },
                "required": ["command"]
            }
        }),
        json!({
            "name": "PowerShell",
            "description": "Run a PowerShell command (powershell -NoProfile -Command) in the working \
                directory and return its combined output. Use this instead of Cmd on Windows when \
                you need cmdlets or .ps1 syntax. Long-running commands are killed on timeout.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "timeout": { "type": "integer", "description": "Timeout in milliseconds (default 120000, max 600000)" }
                },
                "required": ["command"]
            }
        }),
        json!({
            "name": "Glob",
            "description": "Find files by glob pattern (** recurses, * does not cross directories). \
                Returns matching file paths.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "e.g. \"**/*.rs\" or \"src/*.ts\"" },
                    "path": { "type": "string", "description": "Base directory (defaults to the working directory)" }
                },
                "required": ["pattern"]
            }
        }),
        json!({
            "name": "Grep",
            "description": "Search file contents with a Rust regular expression. Returns \
                path:line:content for every match.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regular expression" },
                    "path": { "type": "string", "description": "File or directory (defaults to the working directory)" },
                    "glob": { "type": "string", "description": "Only search files matching this pattern, e.g. \"*.ts\"" },
                    "ignore_case": { "type": "boolean" }
                },
                "required": ["pattern"]
            }
        }),
        json!({
            "name": "WebSearch",
            "description": "Search the web and get ranked results (title, URL, snippet). Use \
                this to find pages when you do not know the URL, then WebFetch the promising \
                ones for the full text. Returns a small JSON-free text list; results may be \
                stale or wrong, so cite the URLs you actually used.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query" },
                    "count": { "type": "integer", "description": format!("Number of results, 1-{SEARCH_MAX_COUNT} (default {SEARCH_DEFAULT_COUNT})") }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "WebFetch",
            "description": "Fetch a URL and return its readable text (HTML is converted to \
                plain text, script/style/comments removed). Use this to read documentation or \
                a web page. Only single documents can be fetched — the page is returned in \
                full, so you can answer any question about it yourself.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Absolute http(s) URL to fetch" },
                    "prompt": { "type": "string", "description": "What you want to learn from the page (advisory only — the full text is returned regardless)" }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "AskUserQuestion",
            "description": "Ask the user 1-4 multiple-choice questions and wait for the answer. \
                Use it when you are genuinely blocked on a decision only the user can make \
                (ambiguous requirement, a choice between approaches). Do not use it to ask for \
                permission or to confirm an action — just say it in plain text.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 4,
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": { "type": "string", "description": "The complete question to ask" },
                                "header": { "type": "string", "description": "Very short label, at most 12 characters" },
                                "options": {
                                    "type": "array",
                                    "minItems": 2,
                                    "maxItems": 4,
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": { "type": "string", "description": "Concise choice text, 1-5 words" },
                                            "description": { "type": "string", "description": "What this choice means" }
                                        },
                                        "required": ["label"]
                                    }
                                },
                                "multiSelect": { "type": "boolean", "description": "Allow picking more than one option (default false)" }
                            },
                            "required": ["question", "header", "options"]
                        }
                    },
                    "answers": {
                        "type": "object",
                        "description": "Filled in by Lunac when the user answers — never set this yourself"
                    }
                },
                "required": ["questions"]
            }
        }),
        json!({
            "name": "TodoWrite",
            "description": "Create or update the task list shown to the user. Send the COMPLETE \
                list every time — it replaces the previous one. Keep at most one entry \
                in_progress, and flip an entry to completed the moment it is done. Use it for \
                multi-step work so the user can follow along; skip it for a single trivial step.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": { "type": "string", "description": "Imperative form, e.g. \"Run the tests\"" },
                                "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] },
                                "activeForm": { "type": "string", "description": "Present continuous form, e.g. \"Running the tests\"" }
                            },
                            "required": ["content", "status", "activeForm"]
                        }
                    }
                },
                "required": ["todos"]
            }
        }),
        json!({
            "name": "PetControl",
            "description": "Control the on-screen desktop pet avatar (its Live2D model) so it \
                reacts physically while you talk. Use it to trigger one of the user's \
                configured pet hotkeys, play a saved preset motion, or directly set a model \
                parameter / expression. The available names (hotkey key strings, preset names, \
                parameter ids, expression names) are listed in your context when the user is \
                talking to the pet — only use names from that list. The pet's reaction is \
                purely visual: after calling, do NOT mention the tool name, the parameter \
                names or values, or any technical detail in your reply — just keep talking \
                naturally, as if the pet simply moved on its own.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["hotkey", "motion", "param", "expression", "artmeshColor"],
                        "description": "hotkey = run a configured hotkey by its key string; \
                            motion = play a saved preset motion by name; param = set a model \
                            parameter; expression = switch an expression on or off; \
                            artmeshColor = tint one ArtMesh (a part of the artwork) with a color."
                    },
                    "name": {
                        "type": "string",
                        "description": "hotkey: the key string (e.g. \"Ctrl+Numpad1\"). motion: \
                            the preset name. param: the parameter id. expression: the \
                            expression name. artmeshColor: the ArtMesh (drawable) id."
                    },
                    "value": {
                        "type": "number",
                        "description": "param: the value to set. expression: 1 to switch on, \
                            0 to switch off (default 1). artmeshColor: the RGB color packed as \
                            0xRRGGBB (0xFFFFFF = original, no tint). Ignored for hotkey / motion."
                    }
                },
                "required": ["action", "name"]
            }
        }),
        json!({
            "name": "SessionSearch",
            "description": "Search the user's OWN past chat sessions in this app (a local \
                full-text index over earlier conversations) and return matching messages \
                grouped by session. Use it whenever the user refers to an earlier \
                conversation — \"last time\", \"we discussed that before\", \"上次\", \
                \"之前聊过的\" — because you cannot recall past sessions from memory. \
                Nothing about earlier sessions is shown to you automatically, so this tool \
                is the only way to read what was actually said. Read-only and local: it never \
                touches the network.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Keyword or phrase to look for. Chinese substrings work; keep it short and specific."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum matching messages to return (default 10, max 30)"
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "Agent",
            "description": "Launch a subagent: an isolated assistant instance with its OWN \
                context window that works on ONE self-contained task using the same tools, \
                then reports back. Use it when a task needs many exploratory tool calls \
                (searching, reading lots of files, comparing options) whose raw output you \
                do NOT want to keep in this conversation — the subagent's intermediate tool \
                output never enters your context, only its final report does. Also useful to \
                investigate something without derailing your current line of work. \
                The subagent cannot see this conversation and cannot ask the user questions, \
                so `prompt` must be completely self-contained. It can read and modify files, \
                so make the task description precise. It cannot launch further subagents. \
                Several Agent calls issued in the SAME message run CONCURRENTLY (up to 3 at \
                a time), so when you have independent tasks, dispatch them together in one \
                message rather than one at a time; reports come back in the order you issued \
                the calls.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "description": {
                        "type": "string",
                        "description": "Short (3-5 words) label for the task, shown to the user"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The complete task for the subagent: what to do, which paths or commands to use, and what its report must contain. It sees nothing but this string."
                    }
                },
                "required": ["description", "prompt"]
            }
        }),
        json!({
            "name": "EnterPlanMode",
            "description": "Switch to PLAN MODE for the rest of this task: from now on every \
                write-class tool (Write / Edit / Cmd / PowerShell / Agent / MCP tools) is \
                REFUSED, so you can only read and reason. Use it when the user asks for a \
                plan first (\"先给我计划\", \"don't change anything yet\", \"plan this out\"), \
                or when the task is large enough that agreeing on an approach up front is \
                worth it. While in plan mode: read the code, then present the finished plan \
                with ExitPlanMode — do NOT try to write anything, it will fail.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "reason": {
                        "type": "string",
                        "description": "One short sentence for the user: why planning first (shown in the UI)"
                    }
                },
                "required": []
            }
        }),
        json!({
            "name": "ExitPlanMode",
            "description": "Present your plan and ask the user to approve it. This is the ONLY \
                way out of plan mode: the user sees the plan in an approval card and either \
                approves it (writes are re-enabled and you execute it task by task) or rejects \
                it (you stay in plan mode — revise the plan and call this again). Pass the \
                COMPLETE plan as `plan`, in markdown, following the plan format: goal / \
                architecture, then bite-sized tasks, each with exact file paths and the exact \
                commands to run.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "plan": {
                        "type": "string",
                        "description": "The full plan in markdown. Exact file paths and exact commands — no vague wording."
                    }
                },
                "required": ["plan"]
            }
        }),
        json!({
            "name": "ListPeers",
            "description": "List the sibling subagents currently running alongside you in the same \
                parallel batch, as their task id + description. Your own entry is marked `(you)`. \
                Use it to see who else is working, and to get the exact id that SendMessage needs. \
                Only agents launched together in one message are visible; an agent that has already \
                finished is gone from the list. When you are the main agent (not a subagent) this is \
                normally empty.",
            "input_schema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "SendMessage",
            "description": "Send a short text note to a sibling subagent, addressed by the task id \
                from ListPeers. It is delivered to that agent at the start of its next round and read \
                as a user message. Use it to hand off a finding so a sibling does not redo the same \
                work, or to ask it to narrow its scope. The note must be self-contained — the sibling \
                cannot see your conversation. It fails if the target has already finished (there is \
                no mailbox for a dead agent).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "to": { "type": "string", "description": "Target task id, exactly as ListPeers shows it (e.g. task-2)" },
                    "content": { "type": "string", "description": "The note to deliver. Keep it short and self-contained." }
                },
                "required": ["to", "content"]
            }
        }),
        json!({
            "name": "EnterWorktree",
            "description": "Create a git worktree for the current repository and switch your \
                effective working directory into it, so you can work on an isolated branch \
                without touching the main checkout. The worktree is created under the app's own \
                data root (\"<exe root>\\temp\\worktrees\\<repo>-<name>\", outside your workspace, \
                so it never shows up as an untracked dir in the repo) and gets a NEW branch named \
                <name>. After this call, relative paths and shell commands resolve inside the \
                worktree (absolute paths are unchanged, and the original workspace stays \
                readable). Fails clearly if the workspace is not a git repository, or if you are \
                already inside a worktree (call ExitWorktree first). This tool never forces or \
                deletes anything.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Worktree and branch name (ASCII letters, digits, '-', '_', '/' or '.'; must not contain '..'). The worktree dir becomes \"<exe root>\\temp\\worktrees\\<repo>-<name>\" (repo = the main checkout's directory name)." }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "ExitWorktree",
            "description": "Leave the git worktree you entered with EnterWorktree and switch your \
                effective working directory back to the original workspace root. This is \
                conservative: it does NOT delete the worktree or its branch — they are left on \
                disk for you (or the user) to remove later with \"git worktree remove <path>\". \
                Fails if you are not currently inside a worktree.",
            "input_schema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "SendUserFile",
            "description": "Present a local file to the user as a card in the conversation: its \
                name, path, size and type, with buttons to open it or reveal it in the file \
                manager. Use it when the user asks for a file (a report, an export, a generated \
                artifact) and you want to hand it over explicitly instead of merely mentioning \
                the path in your reply. The path is resolved and checked exactly like Read (the \
                workspace boundary applies), and a missing file is reported as a readable error — \
                nothing is presented until the file exists. Only metadata is delivered; with \
                preview=true a short text excerpt is attached for text files (large files and \
                binaries are never sent in full).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute or workspace-relative path to the file to hand over" },
                    "preview": { "type": "boolean", "description": "Attach a short text excerpt for text files (default false)" }
                },
                "required": ["file_path"]
            }
        }),
        json!({
            "name": "PushNotification",
            "description": "Send a desktop notification to the user (a title and a body) — a short \
                heads-up about something that happened while they were not looking at the app (a \
                long task finished, an error needs attention). Use it sparingly: it interrupts. \
                Do not use it to reply, to ask a question, or for anything that belongs in the \
                conversation. It is rate-limited — a second call within a few seconds is refused.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short notification title (one line)" },
                    "body": { "type": "string", "description": "Notification body text" }
                },
                "required": ["title", "body"]
            }
        }),
    ];

    all.into_iter()
        .filter(|d| {
            let name = d.get("name").and_then(Value::as_str).unwrap_or("");
            !disallowed.iter().any(|x| x == name)
        })
        .collect()
}

/// 工具名列表（system/init 上报给前端）
pub fn names(tools: &[Value]) -> Vec<String> {
    tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// **必须走 MCP 桥**才能跑的工具名（A3/A4，2026-09-20）。
///
/// 四件里只有 `SessionSearch` 在 `defs()` 恒返回的那张表里（内置一等公民，桥没接通时
/// 调用会**如实报错**而不是编造结果）；另外三件都由 `main.rs` **条件注册** ——
/// resources 读侧要「桥真的接上了用户工具」才追加，`Remember` 要「桥接通了」才追加。
///
/// 集中在一处是为了三件事**不会各写各的**：
///   ① `dispatch_tool` 的「无桥就早退」（给模型的错误文案要一致）；
///   ② `subagent_tool_defs()` 要把它们剔掉（子代理不接桥 ⇒ 留着就是保证失败）；
///   ③ 启动时的条件注册。
/// **新增这类工具只改这个数组**，别在三处分别硬编码字符串。
pub const BRIDGE_TOOLS: [&str; 8] = [
    "SessionSearch",
    "ListMcpResourcesTool",
    "ReadMcpResourceTool",
    "ListMcpPromptsTool",
    "GetMcpPromptTool",
    "Remember",
    // 桌宠自我区（2026-10-13，人格地基）：两条都走宿主桥的 `lunac/pet_growth_write` /
    // `lunac/persona_suggestion_write`（**只追加**）。进这张表即自动：① 子代理里被剔掉
    // （见 `subagent_tool_defs`）；② 没桥时早退报同一句错（不静默失败）。
    "PetGrowth",
    "PetPersonaSuggest",
];

/// 该工具是否必须走 MCP 桥（见 [`BRIDGE_TOOLS`]）。
pub fn needs_bridge(name: &str) -> bool {
    BRIDGE_TOOLS.contains(&name)
}

/// `Remember` 的 schema（A4，长期记忆的**写入侧**）。**不进 `defs()`** —— 由 `main.rs`
/// 在桥接通时条件追加（没桥就写不进去，注册了就是一件必然失败的工具）。
///
/// 与 `TodoWrite` 的区别要说清（模型最容易混）：`TodoWrite` 只改**本回合**的待办面板，
/// 回合结束即失去意义；`Remember` 写的是**跨会话**的长期记忆，进程重启后仍会注入。
pub fn remember_tool() -> Value {
    json!({
        "name": "Remember",
        "description": "Save ONE durable fact to the user's long-term memory (a small local \
            file that is loaded into your context at the start of every future session). Use \
            it for things that stay true across conversations and that you would otherwise \
            have to be told again: the user's stated preferences, project conventions, how \
            their environment is set up, decisions reached and why. Do NOT use it for \
            one-off details, for anything already in the long-term memory you were given, or \
            for the current task's progress (use TodoWrite for that). Write one concise \
            self-contained fact per call, in the user's own language.",
        "input_schema": {
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "One self-contained fact to remember (a sentence or two)."
                },
                "replace": {
                    "type": "boolean",
                    "description": "Replace the ENTIRE memory with `content` instead of appending. Only for consolidating/rewriting when the memory is full or messy."
                }
            },
            "required": ["content"]
        }
    })
}

/// `PetGrowth` 的 schema（2026-10-13，人格地基）。**不进 `defs()`** —— 由 `main.rs`
/// 在桥接通时条件追加；且**只在桌宠发起的那一轮**可调用（其余轮会被 `dispatch_tool` 拒）。
///
/// 它写的是**桌宠自己的成长笔记**（关于「自己该怎么表现」的观察 / 习惯），与 `Remember`
/// （用户的长期记忆）是两回事：成长笔记只注入桌宠轮、**只追加**、不可改删历史（改删只由用户
/// 在界面里逐条做）。
pub fn pet_growth_tool() -> Value {
    json!({
        "name": "PetGrowth",
        "description": "Append ONE short note to your own growth log as the desktop pet — a \
            durable observation or habit about how you should behave (tone, pace, what you \
            noticed works). This is your private self-notes, not the user's memory. Use it only \
            during a desktop-pet turn, for something worth carrying into future pet \
            conversations. Append-only — you cannot edit or delete past notes. Write one concise \
            self-contained line per call, in the language you speak to the user.",
        "input_schema": {
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "One self-contained observation about how to behave (a sentence)."
                }
            },
            "required": ["content"]
        }
    })
}

/// `PetPersonaSuggest` 的 schema（2026-10-13，人格地基）。**不进 `defs()`**，条件注册，
/// **只在桌宠轮可调用**。
///
/// ⚠️ 它**改不了核心人格**：只会把一条「修订建议」写进固定路径的建议区，由用户之后在界面里
/// **接受 → 合并**（事前确认，非「已阅」）。AI 侧没有任何直接写 `pet-persona.md` 的路径。
pub fn persona_suggestion_tool() -> Value {
    json!({
        "name": "PetPersonaSuggest",
        "description": "Propose ONE change to your own core persona (the desktop pet's persona). \
            You CANNOT edit the persona yourself — this only records a suggestion for the user to \
            review and merge later (they decide). Use it during a desktop-pet turn when you notice \
            the persona should say something differently. Write one concise, actionable proposal \
            per call, in the user's language.",
        "input_schema": {
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "One concrete proposed change to the persona (a sentence)."
                }
            },
            "required": ["content"]
        }
    })
}

/// `ListMcpResourcesTool` 的 schema（A3）。**不进 `defs()`** —— 它由 `main.rs`
/// 在「桥接上了用户工具」时才条件追加（理由见那里的注释）。
pub fn list_resources_tool() -> Value {
    json!({
        "name": "ListMcpResourcesTool",
        "description": "List the MCP resources this app exposes. Right now that is the \
            user's own custom tool definition files (`tools\\*.json`) — one resource per \
            file. Use it when you need to know which user-defined tools exist, then \
            ReadMcpResourceTool to look at one. Read-only and local: it never touches \
            the network. Returns an empty list when the user has no custom tools.",
        "input_schema": {
            "type": "object",
            "properties": {},
            "required": []
        }
    })
}

/// `ReadMcpResourceTool` 的 schema（A3）。同上，条件注册。
pub fn read_resource_tool() -> Value {
    json!({
        "name": "ReadMcpResourceTool",
        "description": "Read one MCP resource by `uri` (get the URI from \
            ListMcpResourcesTool). This is how you inspect a user-defined tool definition \
            when you need more than its schema — e.g. which shell command or HTTP endpoint \
            its handler actually uses. Only resources under this app's own tools directory \
            can be read; anything else is refused. Read-only and local.",
        "input_schema": {
            "type": "object",
            "properties": {
                "uri": {
                    "type": "string",
                    "description": "Resource URI, exactly as returned by ListMcpResourcesTool (e.g. file:///C:/…/tools/deploy.json). A bare file name such as `deploy` also works."
                }
            },
            "required": ["uri"]
        }
    })
}

/// `ListMcpPromptsTool` 的 schema（A13）。**不进 `defs()`** —— 由 `main.rs` 在
/// 「桥接上了用户工具」时条件追加（理由同 resources 读侧：没桥就必然失败）。
///
/// prompts 与 resources 的区别：**resources 是「给模型看的资料」，prompts 是
/// 「用户写好的提示词模板」**（`<exe 根>\prompts\*.md`）。后者被取用时展开成一段
/// 可直接执行的指令文本 —— 相当于用户预先备好的 slash 命令。
pub fn list_prompts_tool() -> Value {
    json!({
        "name": "ListMcpPromptsTool",
        "description": "List the MCP prompts (reusable prompt templates) this app exposes. \
            They come from the user's own prompt files (`prompts\\*.md`) and are ready-made \
            instructions the user prepared for tasks they repeat. Use it to discover what is \
            available, then GetMcpPromptTool to render one into concrete instructions. \
            Read-only and local: it never touches the network. Returns an empty list when \
            the user has no prompt templates.",
        "input_schema": {
            "type": "object",
            "properties": {},
            "required": []
        }
    })
}

/// `GetMcpPromptTool` 的 schema（A13）。同上，条件注册。
pub fn get_prompt_tool() -> Value {
    json!({
        "name": "GetMcpPromptTool",
        "description": "Render one MCP prompt (get its name from ListMcpPromptsTool) into \
            concrete instructions, filling any `{{placeholders}}` with the `arguments` you \
            pass. Use this when the user asks for a task that matches one of the available \
            prompts — the returned text is the user's own prepared instructions. Read-only \
            and local.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Prompt name, exactly as returned by ListMcpPromptsTool."
                },
                "arguments": {
                    "type": "object",
                    "description": "Values for the prompt's `{{placeholders}}` (key → string). Omit or pass {} when the prompt has none."
                }
            },
            "required": ["name"]
        }
    })
}

/// 是否需要先过用户审批（P2 的 `can_use_tool`）。
///
/// 只读四件（`Read`/`Glob`/`Grep`/`WebFetch`）里的前三件不需要 —— 工作区锁
/// 已是硬边界；`WebFetch` 不算，它是**唯一会把数据发往外部**的内置工具，
/// 由前端 `classifyRequest()` 决定「白名单自动放行」还是「弹卡片」。
/// 写类四件（`Write`/`Edit`/`Cmd`/`PowerShell`）一律先问 —— 前端会自行处理
/// 「白名单 / 内置安全前缀自动放行」与「危险命令只给手动确认」，所以 agent
/// 侧不做二次判断，问就完了。
/// `AskUserQuestion` 也必须问：**交互本身就是它的功能**（答案经审批卡的
/// `updatedInput` 回传，不问就拿不到答案）。
/// `TodoWrite` **不问**：它只改前端那块待办面板，不碰本机任何东西。
/// `PetControl` **不问**（2026-10-10）：它只让桌宠形象动一下（前端收到 `tool_use` 后广播给
/// 桌宠窗），agent 侧无任何本机副作用 —— 与 `TodoWrite` 同一条判据。
/// 走桥的只读工具（`SessionSearch` / resources 读侧 / prompts 读侧）**也不问** —— 它们只读
/// 本机自己的数据（会话库 / 用户写的工具定义与提示词模板），与 `Read`/`Grep` 同级；
/// **不构成先例**：判据仍是「执行会不会改变本机或把数据带出」。
///
/// `plan` 档下的例外见 [`gated_in_read_only`]。
pub fn needs_approval(name: &str) -> bool {
    matches!(
        name,
        "Write"
            | "Edit"
            // `NotebookEdit`（2026-10-11）**要问**：它是写类 —— 改的是磁盘上的 `.ipynb`。
            // 与 `Edit` 同一条判据（编辑器侧凭据扫描见 main.rs 的 `written_payload`）。
            | "NotebookEdit"
            | "Cmd"
            | "PowerShell"
            | "WebSearch"
            | "WebFetch"
            | "AskUserQuestion"
            // `Remember` **要问**：它写的是**跨会话长期记忆**，而记忆会在用户没看见的
            // 时候被注入以后的每一次对话 —— 副作用比「改一个文件」更持久。
            // 问不代表每次都弹卡：前端按运行方式决定（自动档静默放行），复盘 fork 的
            // 写入也走同一条路（见 main.rs 的 `run_review_fork`）。
            | "Remember"
            // `PetGrowth`（2026-10-13 人格地基）**要问** —— 与 `Remember` **同一条判据**：
            // 它写的是**跨会话、会在用户没看见时被注入以后每一轮**的持久文本（成长笔记注入
            // 桌宠轮）；「只追加 / 封顶 / 落盘前脱敏」都不改变「持久副作用」这个事实，
            // 故与 `Remember` 一视同仁（前端仍按运行方式决定）。
            // ⚠️ `PetPersonaSuggest` **刻意不在这里**（2026-10-13）：它**只写一条「建议」**、
            // **不改任何会被注入的内容** —— 建议要用户之后亲手「接受 → 合并」才生效，与
            // `Remember`「直接写进以后每轮上下文」判然不同。判据：**只写建议 ≠ 直接改人格**；
            // 若这里也弹卡，就对「改人格」这道门问两次（建议时一次、合并时一次），第一道可省。
            | "PetGrowth"
            // `Agent` **要问**：它自己不直接碰本机，但它派生的是一个**能写文件、能跑命令**
            // 的子代理 —— 「派一个代理出去干活」这个决定本身值得确认。
            // 子代理内部的每次写操作**仍会各自再走一次审批**（见 `run_subagent`），
            // 所以这不是「一次批准、后面全放行」。
            | "Agent"
            // `ExitPlanMode` **要问**（2026-09-20 A7）：**这张卡就是它的产品** ——
            // 用户必须在卡上读到整份计划再裁决（允许 = 解除计划相位、放行写类；
            // 拒绝 = 留在计划相位继续改）。它**没有任何本机副作用**，走审批通道与
            // `Skill` fork 同型：**为的是那个「允许 / 拒绝」的裁决点，不是因为有危害**。
            | "ExitPlanMode"
            // `ImageGen`（A13，2026-10-03）**要问**：① 它把 prompt 与参考图发往外部服务
            // （与 `WebFetch` 同类的外部数据出口），② 它写本机文件，③ **按张计费** ——
            // 花的是用户的钱，更该在花之前让用户看见。
            | "ImageGen"
            // `EnterWorktree` / `ExitWorktree`（A13，2026-10-11）**要问**：它们的语义是
            // 「改仓库状态」—— 前者建一个 git worktree + 一条新分支、并把有效工作目录切过去；
            // 后者把有效工作目录切回来。都视作**写类**（与 `Write` / `Edit` 同级判据），
            // 只读档 / 计划相位下由 [`write_blocked`] 直接拒（见 `run()`），不进
            // [`gated_in_read_only`]（只读档该拒、而不是「问了再做」）。
            | "EnterWorktree"
            | "ExitWorktree"
    )
}

/// `SendUserFile` / `PushNotification`（A13，2026-10-11）**刻意不在这里** —— **都不需要审批**：
///   · `SendUserFile` 只是**读**一个本地文件的元信息（resolve + stat，与 `Read` 同源路径处理），
///     既不改本机、也不把内容带出本机（可选预览也只在**本地**界面上显示）。既有约定里
///     **`Read` 不需要审批** ⇒ 与它同级，本工具同样不需要；若给它加审批，反而会变成「绕过
///     `Read` 审批的旁路」的反面误读（真正的旁路是**读得到但审不到**，这里既不读全文也不外发）。
///   · `PushNotification` 只是向前端发一条 `system` 事件，前端弹一条**本地**通知 —— 无本机副作用、
///     无外部出口，与 `TodoWrite` / `PetControl` 同级（都免审批）。
/// 两件在**只读档 / 计划相位**下也都**放行**（读元信息 / 弹本机通知都不是「动手」）⇒ 也**不进**
/// [`gated_in_read_only`]（那一列是「只读档仍要问」的交互类工具，见下）。
///
/// `EnterPlanMode` **刻意不在这里**（2026-09-20 A7）：它只把 agent 进程内的一个标志
/// 置真、再给前端发一条状态事件 —— 既不碰本机也不改前端数据，比 `TodoWrite` 还轻
/// （连面板都不重绘）。问它等于让用户批准「我要开始思考了」。
/// 真正需要用户点头的是**出口**（`ExitPlanMode`），见上面的分支。

/// `Skill` **刻意不在这里**（2026-09-20 A5）：它两种模式的副作用完全不同 ——
/// inline 是纯读 `SKILL.md`（不该弹卡），fork 才派生能写文件、跑命令的子代理（该弹卡），
/// 而判据藏在**入参**（`skill` 指向哪个技能）里，只看名字判不出来。
/// 带输入的版本见 main.rs 的 `needs_approval_with()`。
///
/// 只读（plan）档下**仍需**审批的工具。
///
/// 只读档对写类工具免于询问，是因为它们会被 `run()` 直接拒绝（问了白问）。
/// 下面这三件在只读档是**放行**的，且都必须经过前端交互：
///   · `WebSearch` —— 会把查询词发往外部搜索源
///   · `WebFetch` —— 唯一的外部数据出口（`Read` 到的文件内容能拼进 URL 带出）
///   · `AskUserQuestion` —— 交互就是它的功能；不问等于拿不到答案
pub fn gated_in_read_only(name: &str) -> bool {
    matches!(name, "WebSearch" | "WebFetch" | "AskUserQuestion")
}

/// 该工具是否**只读且互不干扰** —— 可以与其他只读工具**并行**执行（见 main.rs
/// 的 `plan_tool_batches`）。
///
/// 判据是「不碰本机可写状态、不依赖与别的调用的先后」：
///   · `Read` / `Glob` / `Grep` —— 纯读本地
///   · `WebSearch` / `WebFetch` —— 纯网络读取（慢的就是它们；无副作用）
///   · `TodoWrite` —— 只回一段待办清单文本，不碰本机
///
/// 以下**一律串行**，别往这里加：
///   · `Write` / `Edit` / `Cmd` / `PowerShell` —— 有副作用，且「写文件 → 读该文件」
///     的相对顺序必须保持（并行批绝不允许跨越它们，见 `plan_tool_batches`）
///   · `Skill` —— **两种模式一读一写，按最坏的那种算**（2026-09-20 A5）：
///     inline 技能确实只读 `SKILL.md`，但 `context: fork` 的技能会派生一个**能写文件、
///     能跑命令**的子代理去发自己的 API 请求。白名单是**只看名字**的，判不出是哪一种，
///     所以整体串行 —— 代价只是少一次并行，判错的代价是并发发 API + 并发写文件。
///   · MCP 工具 —— 副作用未知，且共用一条 stdio JSON-RPC 通道
///   · `SessionSearch` —— **只读，但同样串行**：它也走那条 stdio 通道
///     （`Bridge::request` 是单线程「发一条、按 id 等一条」，并发只会互相排队甚至错配）。
///     同理还有 resources / prompts 读侧的 `ListMcpResourcesTool` / `ReadMcpResourceTool`
///     / `ListMcpPromptsTool` / `GetMcpPromptTool` 与写入侧的 `Remember`
///     —— **判据是「要不要走桥」，不是「是不是只读」**（见 [`BRIDGE_TOOLS`]）。
///   · `AskUserQuestion` —— 要等人回答，并发弹问没有意义
///   · `ExitPlanMode` —— 同 `AskUserQuestion`：它的结果**就是**用户的那一次裁决，
///     并发弹两张计划卡只会让「批准了哪一份」变得无法回答；顺带它还会改 `plan_phase`
///     这个全局标志（2026-09-20 A7）
pub fn parallel_safe(name: &str) -> bool {
    matches!(
        name,
        "Read" | "Glob" | "Grep" | "WebSearch" | "WebFetch" | "TodoWrite"
        // `PetControl`（桌宠动作，2026-10-10）：agent 侧无副作用（效果全在前端广播），
        // 与 `TodoWrite` 同级 ⇒ 收进白名单（前端按到达顺序执行，动作不会乱序）。
        | "PetControl"
        // `ListPeers`（A13，2026-10-05）：纯读进程内的 peer 登记处，不写盘、不发请求 ⇒
        // 收进白名单。`SendMessage` **刻意不收** —— 它会改共享的收件箱（有副作用），
        // 与白名单「只收纯读工具」的口径不符。
        | "ListPeers"
    )
}

/// `SendUserFile` / `PushNotification`（A13，2026-10-11）**刻意不进**上面这张只读并行白名单：
///   · 两件都要在 `main.rs` 的 `dispatch_tool` 里发 `system` 事件（卡片 / 通知），**串行执行**
///     才能保证事件与周围工具卡的出现顺序确定（并行批在 worker 线程里跑）；
///   · `PushNotification` 还要读写进程级节流时间戳（`LAST_PUSH_NOTIFICATION`），并发会绕过节流。
/// 代价只是少一次并行 —— 它们的实际开销是 stat / 一次锁，可忽略。

// ── 分发 ─────────────────────────────────────────────────────────

/// 执行一个工具调用。Err 会成为 is_error=true 的 tool_result
/// （模型看得见，可自行纠正），而不是中断整轮对话。
pub fn run(ctx: &Ctx, name: &str, input: &Value) -> Result<String, String> {
    match name {
        "Read" => read(ctx, input),
        "Write" => write(ctx, input),
        "Edit" => edit(ctx, input),
        // NotebookEdit（2026-10-11）：写类 —— 按 cell 编辑 `.ipynb`，拦法与 `Edit` 同源。
        "NotebookEdit" => notebook_edit(ctx, input),
        "Cmd" => cmd(ctx, input),
        "PowerShell" => powershell(ctx, input),
        "Glob" => glob(ctx, input),
        "Grep" => grep(ctx, input),
        "WebSearch" => web_search(input),
        "WebFetch" => webfetch(input),
        "AskUserQuestion" => ask_user_question(input),
        "TodoWrite" => todo_write(input),
        // PetControl（桌宠动作，2026-10-10）：**不碰本机、不落盘、免审批** —— agent 侧只回一句
        // 回执，真正的效果由前端执行（见 `pet_control` 的函数注释）。
        "PetControl" => pet_control(input),
        // 多代理通信（A13，2026-10-05）：**无本机副作用** —— 只读写进程内的 peer 登记处，
        // 因此免审批、只读档放行（`needs_approval` / `gated_in_read_only` 里都没有它们）。
        "ListPeers" => list_peers(input),
        "SendMessage" => send_message(input),
        // SendUserFile / PushNotification（A13，2026-10-11）：效果全在前端（一张文件卡 / 一条
        // 桌面通知），这里只回回执；结构化载荷由 `main.rs` 的 `dispatch_tool` 发成 `system`
        // 事件（`tools.rs` 拿不到 `emit`）。两件都免审批（见 `needs_approval`）。
        "SendUserFile" => send_user_file(ctx, input).map(|(text, _)| text),
        "PushNotification" => push_notification(input).map(|(text, _)| text),
        // ImageGen（A13，2026-10-03）：**写类** —— 它往 `temp\images\` 落图。
        // 只读档 / 计划相位的拦法与 Write 同源（`write_blocked`），但**不**进
        // `gated_in_read_only`：只读档下它该被**拒绝**，而不是「问了再做」。
        "ImageGen" => match write_blocked(ctx, "ImageGen") {
            Some(why) => Err(why),
            None => crate::image::run(input),
        },
        // EnterWorktree / ExitWorktree（A13，2026-10-11）：**写类** —— 会改仓库状态
        // （建 worktree + 分支、切换有效工作目录）。只读档 / 计划相位的拦法与 Write 同源
        // （`write_blocked`），不进 `gated_in_read_only`（只读档该直接拒，而不是「问了再做」）。
        "EnterWorktree" => match write_blocked(ctx, "EnterWorktree") {
            Some(why) => Err(why),
            None => enter_worktree(ctx, input),
        },
        "ExitWorktree" => match write_blocked(ctx, "ExitWorktree") {
            Some(why) => Err(why),
            None => exit_worktree(ctx),
        },
        other => Err(format!("Unknown tool: {other}")),
    }
}

/// 工具参数摘要：单行、截断、**脱敏** —— 供日志使用。
/// 参数里可能带 API key（WebSearch）或整段文件内容（Write），不脱敏就等于
/// 把凭据写进日志文件，用户一贴出来就泄漏了。
pub fn summarize_args(input: &Value) -> String {
    let raw = input.to_string();
    let masked = crate::log::mask_secrets(&raw);
    crate::log::truncate_chars(&masked, 400).replace('\r', " ").replace('\n', " ")
}

// ── 路径解析与越界拦截 ───────────────────────────────────────────

fn str_arg(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing required parameter \"{key}\""))
}

/// 词法规范化（不要求路径存在，故不能用 canonicalize）：
/// 去掉 `.` 与 `..`，避免 `..\..\` 绕过工作区前缀判断。
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn resolve(ctx: &Ctx, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        normalize(p)
    } else {
        normalize(&effective_cwd(ctx).join(p))
    }
}

/// 当前**有效工作目录**：进了 worktree 就是它，否则就是进程工作区根 `cwd`（见 `Ctx::worktree`）。
///
/// 只用于「相对路径解析 / shell 工作目录 / 默认遍历根」这三类**寻址**用途；
/// **不用于** [`guard`] —— 边界恒为 `cwd` + `add_dirs`。worktree 落在数据根
/// （`<exe 根>\temp\worktrees`，见 [`worktree_root`]），那**不在 `cwd` 之内**，
/// 由启动时并进的 `add_dirs` 放行（与 `output_dir` / `image_dir` 同法）；
/// 原始工作区里的文件也照旧可达。
fn effective_cwd(ctx: &Ctx) -> PathBuf {
    ctx.worktree
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| ctx.cwd.clone())
}

// ── 子目录记忆懒加载的「触达判据」（2026-10-12，§待办 3-1a 子目录级）──────
//
// 背景：agent **没有**「此刻在动哪个目录」的状态（工具只收到显式路径），而子目录记忆
// 只能**懒加载**（碰到才注、且只进可变层，绝不动固定前缀 —— 见 main.rs）。所以判据收在
// 「一次工具调用**显式带**的路径 → 它所属的目录」这一处，纯函数、可判定、可单测。

/// 一次「文件类」工具调用的路径参数语义：`File` = 值指向文件（取父目录）；`Dir` = 值本身是目录。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    File,
    Dir,
}

/// 子目录记忆的**深度上限**（触碰目录相对 cwd 的层数）。防御异常深的路径；正常项目远到不了。
pub const MAX_SUBDIR_MEMORY_DEPTH: usize = 8;

/// 工具名 → 其路径参数的语义（`None` = 这次调用**不指向任何目录**，即不算「触碰」）。
///
/// **触发判据（可判定、可解释，写进文档）**：只认**显式路径参数**的这几件 ——
///   · `Read` / `Write` / `Edit` / `SendUserFile`：`file_path`（文件 ⇒ 取父目录）；
///   · `NotebookEdit`：`notebook_path`（文件 ⇒ 取父目录）；
///   · `Glob` / `Grep`：`path`（根目录，缺省 = 工作目录 ⇒ 等于 cwd、自然不合格）。
/// **刻意不解析 `Cmd` / `PowerShell` 的命令行**（`cd` 藏在字符串里、无法可靠判定），
/// 故 shell 一律不改「当前触达目录」——它是可判定性与安全性的取舍，写进 docs/todo.md §3-1a。
pub fn path_arg_kind(name: &str) -> Option<PathKind> {
    match name {
        "Read" | "Write" | "Edit" | "NotebookEdit" | "SendUserFile" => Some(PathKind::File),
        "Glob" | "Grep" => Some(PathKind::Dir),
        _ => None,
    }
}

/// 该工具调用里路径参数的键名（与 `path_arg_kind` 同源，一一对应）。
pub fn path_arg_key(name: &str) -> Option<&'static str> {
    match name {
        "Read" | "Write" | "Edit" | "SendUserFile" => Some("file_path"),
        "NotebookEdit" => Some("notebook_path"),
        "Glob" | "Grep" => Some("path"),
        _ => None,
    }
}

/// 一次工具调用「被触碰的目录」的**绝对路径**（`None` = 不指向目录 / 无路径参数 / 空路径）。
/// 文件类取父目录、目录类取自身；相对路径按 `cwd` 解析（与工作区根同源，不含 worktree 覆盖 ——
/// 见 `eligible_subdir` 里「边界恒为 cwd」的说明）。
pub fn touched_dir(cwd: &Path, name: &str, input: &Value) -> Option<PathBuf> {
    let key = path_arg_key(name)?;
    let raw = input.get(key)?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    let p = Path::new(raw);
    let p = if p.is_absolute() {
        normalize(p)
    } else {
        normalize(&cwd.join(p))
    };
    match path_arg_kind(name)? {
        PathKind::File => p.parent().map(Path::to_path_buf),
        PathKind::Dir => Some(p),
    }
}

/// 触碰到的目录是不是一个**够格的「子目录」**（可注入真值的纯判据，便于单测）。
///
/// 三条同时成立才算：
///   ① 在 `cwd` **之内**且**不等于** `cwd`（严格更深）—— 向上那一层（cwd 及父级）由固定前缀负责；
///   ② 相对 `cwd` 的**任一层目录名**都不在忽略名单 `SKIP_DIRS` 里（`.git` / `node_modules` /
///      `target` … ⇒ 连它们的子目录也一并跳过，不只看最后一段）；
///   ③ 层数 ≤ `MAX_SUBDIR_MEMORY_DEPTH`。
pub fn eligible_subdir(cwd: &Path, dir: &Path) -> bool {
    let Ok(rel) = dir.strip_prefix(cwd) else {
        return false;
    };
    if rel.as_os_str().is_empty() {
        return false; // 就是 cwd 本身
    }
    let comps: Vec<Component> = rel.components().collect();
    if comps.len() > MAX_SUBDIR_MEMORY_DEPTH {
        return false;
    }
    !comps.iter().any(|c| match c {
        Component::Normal(n) => n
            .to_str()
            .map(|s| SKIP_DIRS.iter().any(|d| s.eq_ignore_ascii_case(d)))
            .unwrap_or(false),
        _ => false,
    })
}

/// 工作区锁生效时，路径必须落在 cwd 或 `--add-dir` 之内。
fn guard(ctx: &Ctx, path: &Path) -> Result<(), String> {
    if !ctx.locked {
        return Ok(());
    }
    let inside = std::iter::once(&ctx.cwd)
        .chain(ctx.add_dirs.iter())
        .any(|root| path.starts_with(root));
    // 技能目录是**用户明确要 AI 能写**的那一个例外（见 `inside_skills_dir`）。
    if inside || inside_skills_dir(path) {
        Ok(())
    } else {
        Err(format!(
            "Access denied: {} is outside the workspace ({})",
            path.display(),
            ctx.cwd.display()
        ))
    }
}

/// 路径是否落在**技能目录**之内（`LUNAC_SKILLS_DIR` = `<exe 根>\skills`）。
///
/// **为什么单独开这道口子**（2026-10-06，q3 第 5 步）：用户 2026-10-05 明确要
/// 「让 AI 在运行时自行查找或生成 skill，**动手前提醒用户**」。未锁工作区时这本来就成立
/// （工作区之外的 `Write` 照旧弹审批卡）；但**设了工作区**之后 `guard` 会**硬拒**
/// （不是弹卡）⇒ AI 连问都问不到，只剩一句干巴巴的 `Access denied`，与用户的诉求正好相反。
///
/// ⚠️ 这里**只放行、不代替审批**：`Write` / `Edit` 在 [`needs_approval`] 里**恒真**
/// （与路径无关），所以写技能文件照常走审批卡 —— 用户点头才落盘。这正是「动手前提醒」
/// 的落点；**不许**把它改成静默写入。
///
/// 判据与 `guard` 主路径同一条口径：**词法规范化后按组件比**。`resolve` 已经去过 `.` / `..`，
/// 所以 `<exe 根>\skills\..\config\ai.json` 会先被化成 `<exe 根>\config\ai.json`，落不进这里。
fn inside_skills_dir(path: &Path) -> bool {
    let Ok(raw) = std::env::var("LUNAC_SKILLS_DIR") else {
        return false;
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return false;
    }
    path_is_within(path, Path::new(raw))
}

/// 「`path` 是否在 `root` 之内」的**纯判据**（抽出来是为了能单测，且不碰进程级环境变量）。
/// 两边都过 [`normalize`]；空 `root` 一律 false（没配就等于不开这道口子）。
fn path_is_within(path: &Path, root: &Path) -> bool {
    let root = normalize(root);
    !root.as_os_str().is_empty() && path.starts_with(&root)
}

// ── Read ─────────────────────────────────────────────────────────

fn read(ctx: &Ctx, input: &Value) -> Result<String, String> {
    let path = resolve(ctx, &str_arg(input, "file_path")?);
    guard(ctx, &path)?;

    let meta = fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.is_dir() {
        return Err(format!(
            "{} is a directory — use Glob or Grep instead",
            path.display()
        ));
    }
    if meta.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "{} is too large ({} bytes, limit {MAX_TEXT_BYTES}) — read a slice with Cmd instead",
            path.display(),
            meta.len()
        ));
    }

    let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    // 图片不能按 UTF-8 读 —— PNG/JPEG 会变成一整片 `�PNG…` 乱码（2026-10-05 用户报的问题）。
    // 只贴「哨兵 + 绝对路径」，图片本身由 `main.rs` 的 `tool_result_block` 换成 image 块
    // 交给视觉模型（复用 A8 的魔术字节判定与体积上限，见 `IMAGE_SENTINEL`）。
    if let Some(media) = crate::image_media_type(&bytes) {
        return Ok(format!(
            "{IMAGE_SENTINEL}{}\n[image {media}, {} KB] The picture itself is attached to this \
             tool result as an image block — look at it directly; this line is only metadata.\n",
            path.display(),
            bytes.len() / 1024,
        ));
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();

    let offset = input
        .get("offset")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(MAX_READ_LINES as u64)
        .min(MAX_READ_LINES as u64) as usize;

    let start = (offset - 1).min(lines.len());
    let end = start.saturating_add(limit).min(lines.len());

    let mut out = String::new();
    for (i, line) in lines[start..end].iter().enumerate() {
        out.push_str(&format!("{}\t{}\n", start + i + 1, line));
    }
    if out.is_empty() {
        out = format!("(empty file: {} lines total)", lines.len());
    }
    Ok(out)
}

// ── Write ────────────────────────────────────────────────────────

/// 写入内容的静态安全分析（2026-09-20，原 backlog A6；规则集见 `content_safety.rs`）。
///
/// 命中就往返回文本里附一句，**如实告知模型**。为什么工具侧也要报一次：审批卡上那
/// 份是给**用户**看的，模型看不到 —— 不报的话它不知道用户为什么被多问了一次，
/// 下次还会照着写同样的东西。
fn secret_note(text: &str) -> String {
    let report = crate::content_safety::analyze(text);
    if report.is_clean() {
        return String::new();
    }
    format!(
        "\n[static analysis flagged possible credentials in this content: {}. \
         If it is a real secret, prefer an environment variable or a gitignored file.]",
        report.summary()
    )
}

fn write(ctx: &Ctx, input: &Value) -> Result<String, String> {
    if let Some(why) = write_blocked(ctx, "Write") {
        return Err(why);
    }
    let path = resolve(ctx, &str_arg(input, "file_path")?);
    guard(ctx, &path)?;
    let content = str_arg(input, "content")?;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
    }
    fs::write(&path, content.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!(
        "Wrote {} bytes to {}",
        content.len(),
        path.display()
    ))
}

// ── Edit ─────────────────────────────────────────────────────────

fn edit(ctx: &Ctx, input: &Value) -> Result<String, String> {
    if let Some(why) = write_blocked(ctx, "Edit") {
        return Err(why);
    }
    let path = resolve(ctx, &str_arg(input, "file_path")?);
    guard(ctx, &path)?;

    let old = str_arg(input, "old_string")?;
    if old.is_empty() {
        return Err("old_string must not be empty".into());
    }
    let new = str_arg(input, "new_string")?;
    let replace_all = input
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let count = text.matches(&old).count();
    if count == 0 {
        return Err(format!("old_string not found in {}", path.display()));
    }
    if count > 1 && !replace_all {
        return Err(format!(
            "old_string appears {count} times in {} — add more context or set replace_all=true",
            path.display()
        ));
    }

    let updated = if replace_all {
        text.replace(&old, &new)
    } else {
        text.replacen(&old, &new, 1)
    };
    fs::write(&path, updated.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!(
        "Edited {}: replaced {count} occurrence(s){}",
        path.display(),
        // 只扫**写进去的** `new` —— `old` 是被删掉的内容，扫它没有意义
        secret_note(&new)
    ))
}

// ── NotebookEdit ─────────────────────────────────────────────────

/// `NotebookEdit` 的三种编辑模式（Claude Code 口径）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum NotebookMode {
    Replace,
    Insert,
    Delete,
}

/// 按 cell 编辑 Jupyter notebook（`.ipynb`）—— **写类**，拦法与 `Edit` 同源。
///
/// **为什么单开一件、不拿 `Edit` 改 `.ipynb`**：notebook 是结构化 JSON
/// （`cells[]` + `metadata` + `nbformat`），字符串替换在转义后的 `source` 数组上极易改错
/// 结构；本工具按 cell 定位、**只动目标 cell**，其余原样保留。
///
/// **结构与风格**：解析成 `serde_json::Value` 改完再序列化。key 顺序天然是字典序 ——
/// 这正是 Jupyter `nbformat` 的落盘口径（`json.dumps(..., indent=1, sort_keys=True)`）；
/// 缩进由 [`detect_indent`] 沿用原文件，末尾换行也照原样保留 ⇒ 未改动的部分逐字节不变。
fn notebook_edit(ctx: &Ctx, input: &Value) -> Result<String, String> {
    if let Some(why) = write_blocked(ctx, "NotebookEdit") {
        return Err(why);
    }
    let path = resolve(ctx, &str_arg(input, "notebook_path")?);
    guard(ctx, &path)?;

    let mode = match input.get("edit_mode").and_then(Value::as_str).unwrap_or("replace") {
        "replace" => NotebookMode::Replace,
        "insert" => NotebookMode::Insert,
        "delete" => NotebookMode::Delete,
        other => {
            return Err(format!(
                "edit_mode must be \"replace\", \"insert\" or \"delete\", got \"{other}\""
            ))
        }
    };
    let new_source = input.get("new_source").and_then(Value::as_str).unwrap_or("");
    let cell_type = input.get("cell_type").and_then(Value::as_str);
    if let Some(ct) = cell_type {
        if ct != "code" && ct != "markdown" {
            return Err(format!("cell_type must be \"code\" or \"markdown\", got \"{ct}\""));
        }
    }
    if mode == NotebookMode::Insert && cell_type.is_none() {
        return Err("cell_type is required when edit_mode is \"insert\"".into());
    }
    // replace / insert 必须有正文（空串 = 清空是合法的）；字段整个缺失多半是模型漏带。
    if mode != NotebookMode::Delete && input.get("new_source").is_none() {
        return Err("new_source is required for replace / insert".into());
    }
    let cell_id = input.get("cell_id").and_then(Value::as_str).filter(|s| !s.is_empty());
    let cell_number = match input.get("cell_number").and_then(Value::as_i64) {
        Some(n) if n >= 0 => Some(n as usize),
        Some(n) => return Err(format!("cell_number must be >= 0, got {n}")),
        None => None,
    };

    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let trailing_nl = text.ends_with('\n');
    let indent = detect_indent(&text);
    let mut doc: Value = serde_json::from_str(&text).map_err(|e| {
        format!("{} is not a valid notebook (JSON parse failed): {e}", path.display())
    })?;
    if !doc.is_object() {
        return Err(format!(
            "{} is not a Jupyter notebook (top level is not a JSON object)",
            path.display()
        ));
    }
    // cell 是否带 id（nbformat 4.5+ 要求；老 notebook 没有）—— 插入新 cell 时据此决定要不要
    // 生成 id，免得给老格式塞一个它不认的字段。
    let uses_ids = doc.get("nbformat_minor").and_then(Value::as_u64).map_or(false, |m| m >= 5)
        || doc
            .get("cells")
            .and_then(Value::as_array)
            .map_or(false, |a| a.iter().any(|c| c.get("id").is_some()));

    let (summary, written) = {
        let cells = doc.get_mut("cells").and_then(Value::as_array_mut).ok_or_else(|| {
            format!("{} is not a Jupyter notebook (no \"cells\" array)", path.display())
        })?;
        match mode {
            NotebookMode::Delete => {
                let i = locate_cell(cells, cell_id, cell_number, false, &path)?;
                cells.remove(i);
                (
                    format!(
                        "Deleted cell {i} from {} — {} cell(s) remain",
                        path.display(),
                        cells.len()
                    ),
                    String::new(),
                )
            }
            NotebookMode::Replace => {
                let i = locate_cell(cells, cell_id, cell_number, false, &path)?;
                {
                    let obj = cells[i].as_object_mut().ok_or_else(|| {
                        format!("cell {i} in {} is not a JSON object", path.display())
                    })?;
                    // 沿用原 cell 的 source 写法（列表 / 字符串）；缺省按 Jupyter 的列表写。
                    let as_list = !matches!(obj.get("source"), Some(Value::String(_)));
                    obj.insert("source".into(), source_value(new_source, as_list));
                    if let Some(ct) = cell_type {
                        obj.insert("cell_type".into(), Value::String(ct.into()));
                    }
                }
                (
                    format!(
                        "Replaced the source of cell {i} in {} — {} cell(s) total",
                        path.display(),
                        cells.len()
                    ),
                    new_source.to_string(),
                )
            }
            NotebookMode::Insert => {
                let i = locate_cell(cells, cell_id, cell_number, true, &path)?;
                let ct = cell_type.unwrap_or("code");
                cells.insert(i, build_notebook_cell(ct, new_source, uses_ids));
                (
                    format!(
                        "Inserted a {ct} cell at position {i} in {} — {} cell(s) total",
                        path.display(),
                        cells.len()
                    ),
                    new_source.to_string(),
                )
            }
        }
    };

    // 序列化：沿用原缩进，末尾换行照原样保留 ⇒ 未改动的行逐字节不变。
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(indent.as_slice());
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    doc.serialize(&mut ser)
        .map_err(|e| format!("failed to serialize {}: {e}", path.display()))?;
    let mut out = String::from_utf8(buf)
        .map_err(|e| format!("serialized notebook for {} is not valid UTF-8: {e}", path.display()))?;
    if trailing_nl {
        out.push('\n');
    }
    fs::write(&path, out.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!("{summary}{}", secret_note(&written)))
}

/// 解析目标 cell 的下标。`for_insert` 时允许等于 `cells.len()`（追加），且 `cell_id` 的
/// 语义是「插到该 cell **之后**」；replace / delete 则必须是已存在的 cell。
fn locate_cell(
    cells: &[Value],
    cell_id: Option<&str>,
    cell_number: Option<usize>,
    for_insert: bool,
    path: &Path,
) -> Result<usize, String> {
    if let Some(id) = cell_id {
        let pos = cells
            .iter()
            .position(|c| c.get("id").and_then(Value::as_str) == Some(id))
            .ok_or_else(|| format!("cell id \"{id}\" not found in {}", path.display()))?;
        return Ok(if for_insert { pos + 1 } else { pos });
    }
    let n = cells.len();
    match cell_number {
        Some(k) if for_insert && k <= n => Ok(k),
        Some(k) if !for_insert && k < n => Ok(k),
        Some(k) => Err(format!(
            "cell_number {k} is out of range for {} ({n} cell(s))",
            path.display()
        )),
        None if for_insert => Ok(n),
        None => Err(format!(
            "{}: specify cell_id or cell_number to select a cell",
            path.display()
        )),
    }
}

/// 把一段源文本转成 notebook 的 `source` 取值。`as_list` = Jupyter 的规范写法（按行拆成
/// 字符串数组，除最后一段外每段以 `\n` 结尾）；否则写成单个字符串（沿用原 cell 的写法）。
fn source_value(text: &str, as_list: bool) -> Value {
    if as_list {
        Value::Array(
            text.split_inclusive('\n')
                .map(|line| Value::String(line.to_string()))
                .collect(),
        )
    } else {
        Value::String(text.to_string())
    }
}

/// 造一个新的 notebook cell。字段顺序与 Jupyter 一致 —— `serde_json` 的 Map 是字典序，
/// 正好与 `nbformat` 的 `sort_keys=True` 落盘口径相同。
fn build_notebook_cell(cell_type: &str, source: &str, uses_ids: bool) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("cell_type".into(), Value::String(cell_type.to_string()));
    if cell_type == "code" {
        m.insert("execution_count".into(), Value::Null);
    }
    if uses_ids {
        m.insert("id".into(), Value::String(random_cell_id()));
    }
    m.insert("metadata".into(), json!({}));
    if cell_type == "code" {
        m.insert("outputs".into(), json!([]));
    }
    m.insert("source".into(), source_value(source, true));
    Value::Object(m)
}

/// 新 cell 的 `id`（8 字节随机 → 16 位十六进制）。只在 notebook 用 id 时才调用。
/// getrandom 失败退回时间 / pid 熵（与 mcp_oauth 的兜底同款；id 只需唯一、不需密码学强度）。
fn random_cell_id() -> String {
    let mut buf = [0u8; 8];
    if getrandom::getrandom(&mut buf).is_err() {
        let mut seed = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ (std::process::id() as u64).rotate_left(17);
        for b in buf.iter_mut() {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *b = (seed >> 33) as u8;
        }
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// 取原文件的缩进风格（第一个缩进行的前导空白）。Jupyter 落盘恒为 1 个空格；这里照抄原文件，
/// 凡是标准的 notebook 都能沿用，避免整文件重排。解析不出（单行 / 非 pretty）时退回 1 空格。
fn detect_indent(text: &str) -> Vec<u8> {
    for line in text.lines().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let ws: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        if !ws.is_empty() {
            return ws.into_bytes();
        }
        break; // 顶级键没有缩进 ⇒ 不是 pretty 输出，用默认
    }
    b" ".to_vec()
}

// ── Cmd / PowerShell ────────────────────────────────────────────
//
// 两个工具除了「怎么起进程」之外完全一样：并发读干管道防死锁、超时 kill、
// 结果拼成「exit code + stdout + stderr」再走同一个上限截断，故共用 run_shell。

fn cmd(ctx: &Ctx, input: &Value) -> Result<String, String> {
    if let Some(why) = write_blocked(ctx, "Cmd") {
        return Err(why);
    }
    let command = str_arg(input, "command")?;
    let timeout = timeout_arg(input);

    // 必须用 `raw_arg` 原样拼命令行：`Command::arg` 会按 MSVC 的引号规则把命令里的
    // `"` 转义成 `\"`（命令含空格时必然触发），而 `cmd /C` 的引号语义是 cmd **自己**
    // 解释的、不认这个转义 ⇒ 反斜杠原样进了结果（实测 `echo "a b"` 输出 `\"a b\"`，
    // 拿引号包参数的真实命令如 `git commit -m "..."` 会整条变形）。
    // 同一坑在 A9 的 hooks 上踩过一次，见 `hooks.rs` 的 `run_one`。
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd");
        c.raw_arg("/C").raw_arg(&command);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.arg("-c").arg(&command);
        c
    };
    run_shell(ctx, &mut cmd, timeout, &command)
}

fn powershell(ctx: &Ctx, input: &Value) -> Result<String, String> {
    if let Some(why) = write_blocked(ctx, "PowerShell") {
        return Err(why);
    }
    let command = str_arg(input, "command")?;
    let timeout = timeout_arg(input);

    // `-NoProfile` 跳过用户 profile（更干净也更快）；`-NonInteractive` 防
    // 脚本卡在 Read-Host 之类的地方干等到超时。
    // 前缀的两行是编码兜底：重定向到管道时 PowerShell 5.1 按控制台的
    // ANSI 码页输出（中文 Windows = GBK），而我们把管道当 UTF-8 解码，
    // 不切 UTF-8 的话中文输出会整片变成替换字符。
    //
    // **这里刻意用 `arg` 而不是 `raw_arg`**：与 `Cmd` 的 `cmd /C` 不同，
    // `powershell.exe` 的解析器认得 MSVC 那套 `\"` 转义（`Write-Output "a b"`
    // 实测输出正确），改成 raw_arg 反而要自己拼整条命令行、徒增风险。
    // 断言见单测 `quoted_shell_arguments_survive_the_command_line`。
    let mut cmd = Command::new("powershell");
    cmd.arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(format!(
            "$OutputEncoding=[Text.Encoding]::UTF8;[Console]::OutputEncoding=[Text.Encoding]::UTF8;{command}"
        ));
    run_shell(ctx, &mut cmd, timeout, &command)
}

fn timeout_arg(input: &Value) -> u64 {
    input
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(CMD_TIMEOUT_MS)
        .min(CMD_MAX_TIMEOUT_MS)
}

/// 起进程 → 读干输出 → 超时 kill → 拼结果文本。
/// `label` = 命令原文（截断后给「待办清单」里的后台区显示）—— 只用于转后台那一条路，
/// 其余情况一个字都不用。传进来而不是从 `cmd` 反推：`cmd` 是拼好的 `Command`，
/// 从它取原文要跨平台拆引号，得不偿失。
fn run_shell(ctx: &Ctx, cmd: &mut Command, timeout: u64, label: &str) -> Result<String, String> {
    let prog = cmd.get_program().to_string_lossy().to_string();
    // 交互式终端（2026-10-05）：只有装了控制块（= 有前端在看着这次调用）才把 stdin
    // 接成管道，让用户能把键入送进子进程；否则保持 `Stdio::null()`，与改造前逐字节一致
    //（单测 / 子代理 / 后台复盘都走后者）。
    let interactive = ctx.shell_control.is_some();
    // 有效工作目录（进了 worktree 就是 worktree 里，见 `effective_cwd`）——
    // 先绑定再借用，别让临时值只在链式调用里活。
    let shell_cwd = effective_cwd(ctx);
    cmd.current_dir(&shell_cwd)
        .stdin(if interactive { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // CREATE_NO_WINDOW —— GUI 宿主下不加会闪黑框
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }

    // 三者都包一层 `Option`：转后台时要把**所有权移走**（交给后台注册表），而
    // 借用检查器不认识「移走之后不会再走回来」这件事 ⇒ 直接 move 会被判 use-after-move。
    let mut child = match cmd.spawn() {
        Ok(c) => Some(c),
        Err(e) => {
            // spawn 失败（解释器不存在 / 被拦）是最需要事后取证的一类错误
            let msg = format!("spawn failed: {e}");
            crate::log::warn(format!("shell[{prog}] {msg}"));
            return Err(msg);
        }
    };
    // 记下 PID（2026-10-08，用户要求「AI 跑的命令要能在任务管理器里找到」）：起的是
    // `cmd /C ...` / `powershell -Command ...`，**真正干活的程序常常是它的孙进程**，而
    // 任务管理器默认只显示映像名、不显示命令行 —— 只报「跑完了」等于没有可核对的锚点。
    // 这一行让「某条命令当时是什么 pid、还在不在」有据可查（转后台那条路还会带上 pid）。
    if let Some(c) = child.as_ref() {
        crate::log::info(format!("shell[{prog}] spawn pid={}", c.id()));
    }
    let out_pipe: Option<ChildStdout> = child.as_mut().and_then(|c| c.stdout.take());
    let err_pipe = child.as_mut().and_then(|c| c.stderr.take());
    // 交互式终端（2026-10-05）：把子进程 stdin 的写端交给控制块 —— 前端键入经
    // `tool_control` 找到它、由 `write_stdin` 落笔。不装控制块时 stdin 是 null，
    // `take()` 得到 `None`，这里整段是空操作。
    if let (Some(ctl), Some(w)) = (ctx.shell_control.as_ref(), child.as_mut().and_then(|c| c.stdin.take()))
    {
        ctl.set_stdin(w);
    }
    // 必须并发读干管道，否则子进程写满缓冲区后会卡死。
    // 实时回传（2026-09-30）：`ctx.tool_output` 只在 `run_one_tool` 里装上（见 Ctx 注释），
    // 没有它时 drain 的行为与改造前完全一致 —— 只是多一个恒为 None 的参数。
    let sink_out = ctx.tool_output.clone();
    let sink_err = ctx.tool_output.clone();
    let mut h_out = Some(thread::spawn(move || drain(out_pipe, sink_out, "stdout")));
    let mut h_err = Some(thread::spawn(move || drain(err_pipe, sink_err, "stderr")));

    let started = Instant::now();
    let mut timed_out = false;
    let mut stopped_by_user = false;
    let mut backgrounded: Option<String> = None;
    let status = loop {
        // ── 用户实时指令（2026-10-01）──────────────────────────────
        // **每一拍**都看这两个标志 —— 这是运行中的命令**唯一**能被协作式中断的地方。
        // 控制块只在 `run_one_tool` 里装（见 `Ctx::shell_control`）：单测 / 子代理 /
        // 后台复盘拿到的都是 `None`，走不到这里，行为与改造前逐字节一致。
        if let Some(ctl) = &ctx.shell_control {
            if ctl.stop.load(Ordering::Relaxed) {
                if let Some(c) = child.as_mut() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                stopped_by_user = true;
                break None;
            }
            if ctl.background.load(Ordering::Relaxed) {
                // 转后台 = 把 `Child` 与两个读线程**移交**出去（本函数从此不再持有它们），
                // 这里立刻返回。**不 kill** —— 那正是「后台」的意思。
                // 先置「已移交」，再交出去 —— `run_one_tool` 收尾时按它决定**不注销**
                // 这张登记（见 `ShellControl::handed_off`）。
                ctl.handed_off.store(true, Ordering::Relaxed);
                backgrounded = Some(crate::hand_off_to_background(crate::BackgroundJob {
                    child: child.take().expect("转后台时 child 必然还在"),
                    h_out: h_out.take(),
                    h_err: h_err.take(),
                    prog: prog.clone(),
                    label: label.to_string(),
                    ctl: Arc::clone(ctl),
                }));
                break None;
            }
        }
        let Some(c) = child.as_mut() else {
            break None;
        };
        match c.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if started.elapsed() > Duration::from_millis(timeout) {
                    let _ = c.kill();
                    let _ = c.wait();
                    timed_out = true;
                    break None;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("wait failed: {e}")),
        }
    };

    // ── 等读线程收尾：**有界**，且这一等期间仍然响应「停止 / 后台」（2026-10-03 修）────
    // 子进程退出 **≠** 输出读完：`drain` 要等管道 **EOF**。若命令里 **detached 启动了一个
    // 常驻进程**（ComfyUI / 后台服务），它会继承写端 ⇒ EOF 永不到来，而原先那句无条件
    // `h.join()` 会**永久阻塞**。更糟的是：上面那个控制循环此时已经退出 ⇒「停止 / 后台」
    // 两个标志**再没人看** —— 用户点了「后台运行」也毫无反应。
    // 实测（2026-10-03）：`tool_control: … 转后台` 06:43:57 就置了位，命令一直到 06:44
    // 被重启都没反应，工具调用永远不返回。
    // ⇒ 这一等循环同时看三件事：**读线程结束没有 / 用户控制 / 宽限期**。
    let join_start = Instant::now();
    let mut hand_off_now = false;
    loop {
        if let Some(ctl) = &ctx.shell_control {
            if ctl.stop.load(Ordering::Relaxed) {
                stopped_by_user = true;
                break;
            }
            if ctl.background.load(Ordering::Relaxed) {
                hand_off_now = true;
                break;
            }
        }
        let out_done = match &h_out {
            Some(h) => h.is_finished(),
            None => true,
        };
        let err_done = match &h_err {
            Some(h) => h.is_finished(),
            None => true,
        };
        if out_done && err_done {
            break;
        }
        if join_start.elapsed() >= OUTPUT_JOIN_GRACE {
            // 管道被别的进程攥着（或输出迟迟读不完）⇒ 按「转后台」移交，模型先被放走。
            hand_off_now = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    if hand_off_now {
        // 子进程**已退出**，这里移交的是「还没读完的两个读线程」（`child` 仍在手上、
        // 但 `try_wait` 已见过它退出，交给后台只是让收尾线程按同一套逻辑收口）。
        if let (Some(child), Some(ctl)) = (child.take(), ctx.shell_control.clone()) {
            ctl.handed_off.store(true, Ordering::Relaxed);
            backgrounded = Some(crate::hand_off_to_background(crate::BackgroundJob {
                child,
                h_out: h_out.take(),
                h_err: h_err.take(),
                prog: prog.clone(),
                label: label.to_string(),
                ctl,
            }));
        }
    }

    // 转后台：`child` / 两个读线程的所有权已经交出去了 ⇒ **不能再 join、也不能拼结果**
    //（拼了就是两份输出，而且这一份必然是空壳）。如实告诉模型「它还在跑、别等它」。
    if let Some(id) = backgrounded {
        crate::log::info(format!("shell[{prog}] 已转后台 id={id}"));
        return Ok(format!(
            "Command moved to the background (id={id}). It is still running; its output will be \
             delivered to you automatically once it finishes. Do NOT wait or poll for it — \
             carry on with other work. The user can cancel it from the task list."
        ));
    }

    // 走到这里：要么两个读线程都结束了，要么是「用户停止」/宽限期（那两个分支不会继续等）。
    // **只 join 已经结束的** —— 没结束的（管道被别的进程攥着）绝不能再阻塞。
    let stdout = match h_out {
        Some(h) if h.is_finished() => h.join().unwrap_or_default(),
        _ => String::new(),
    };
    let stderr = match h_err {
        Some(h) if h.is_finished() => h.join().unwrap_or_default(),
        _ => String::new(),
    };

    let mut out = String::new();
    // 「用户停止」与「超时」**必须分开报**：前者是用户的决定（模型该换个做法，或者
    // 先问一句再动手），后者是命令自己的问题（该去查为什么慢）。混成一句话会让模型
    // 诊断错方向 —— 它会把「用户不要这么干」读成「这条命令有毛病，我再试一次」。
    if stopped_by_user {
        out.push_str("(stopped by the user — process killed; do not retry this command as-is)\n");
    }
    if timed_out {
        out.push_str(&format!("(timed out after {timeout} ms — process killed)\n"));
    }
    match status.and_then(|s| s.code()) {
        Some(code) => out.push_str(&format!("exit code: {code}\n")),
        None if !timed_out && !stopped_by_user => out.push_str("exit code: (none)\n"),
        None => {}
    }
    if !stdout.trim().is_empty() {
        out.push_str(&stdout);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    if !stderr.trim().is_empty() {
        out.push_str("[stderr]\n");
        out.push_str(&stderr);
    }
    if out.trim().is_empty() {
        out = "(no output)".into();
    }

    // shell 类工具的事后取证：退出码 / 是否超时 / 输出规模 / stderr 原文
    // （命令行本身由 main.rs 的 run_tool 在调用前后记录，这里补执行结果）
    let code = status.and_then(|s| s.code());
    crate::log::info(format!(
        "shell[{prog}] exit={} timeout={} stopped={} stdout={}B stderr={}B",
        code.map_or_else(|| "-".into(), |c| c.to_string()),
        timed_out,
        stopped_by_user,
        stdout.len(),
        stderr.len()
    ));
    if !stderr.trim().is_empty() {
        crate::log::warn(format!("shell[{prog}] stderr:\n{stderr}"));
    }

    Ok(out)
}

/// **有界**地收一个读线程的结果：`grace` 内结束就返回它的输出，否则返回空串（绝不阻塞）。
///
/// 存在的理由只有一条：`JoinHandle::join` 等的是**管道 EOF**，而写端可能被**别的进程**
/// 攥着（命令里 detached 启动的常驻进程）⇒ 无条件 `join()` 会永久挂住。后台收尾线程用
/// 它来防止「一条后台命令把收尾线程永远占住」（见 `main.rs::hand_off_to_background`）。
pub fn join_within(h: Option<thread::JoinHandle<String>>, grace: Duration) -> String {
    let Some(h) = h else { return String::new() };
    let deadline = Instant::now() + grace;
    while !h.is_finished() {
        if Instant::now() >= deadline {
            return String::new();
        }
        thread::sleep(Duration::from_millis(20));
    }
    h.join().unwrap_or_default()
}

/// 把管道读干（超上限后继续读但丢弃，防止子进程阻塞）
///
/// `sink` 非空时**边读边回传**（实时输出，2026-09-30）：命令还在跑，前端就能看到输出。
/// 回传上限同样是 `MAX_PIPE_BYTES`（与最终结果一份口径）—— 再多的部分既不进结果也不进
/// 实时流，避免一条 `Get-Content` 巨型文件把 webview 灌爆。
fn drain<R: Read>(pipe: Option<R>, sink: Option<ToolOutputSink>, stream: &'static str) -> String {
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    let mut buf: Vec<u8> = Vec::new();
    let mut sent = 0usize;
    // 跨 read 的分片会让多字节字符被劈成两半 —— 用这个夹具把「半截字符」留到下一片，
    // 避免实时流里出现一片替换字符（最终结果不受影响，它走 buf 整块解码）。
    let mut carry = Utf8Carry::default();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if let Some(cb) = &sink {
                    if sent < MAX_PIPE_BYTES {
                        let text = carry.push(&chunk[..n]);
                        if !text.is_empty() {
                            sent += text.len();
                            cb(stream, &text);
                        }
                    }
                }
                if buf.len() < MAX_PIPE_BYTES {
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
        }
    }
    if let Some(cb) = &sink {
        let tail = carry.flush();
        if !tail.is_empty() {
            cb(stream, &tail);
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// UTF-8 分片夹具：只吐「确定收全了」的那部分，半截多字节字符留到下一片。
#[derive(Default)]
struct Utf8Carry {
    pending: Vec<u8>,
}

impl Utf8Carry {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        match std::str::from_utf8(&self.pending) {
            Ok(s) => {
                let out = s.to_string();
                self.pending.clear();
                out
            }
            Err(e) => {
                let valid = e.valid_up_to();
                let out = String::from_utf8_lossy(&self.pending[..valid]).into_owned();
                self.pending.drain(..valid);
                // ≥4 字节还凑不出一个完整字符 ⇒ 不是「半截」，是真乱码：老实吐替换字符，
                // 否则这段字节会一直卡在夹具里把后续输出也堵住。
                if self.pending.len() >= 4 {
                    let lossy = String::from_utf8_lossy(&self.pending).into_owned();
                    self.pending.clear();
                    return out + &lossy;
                }
                out
            }
        }
    }

    fn flush(&mut self) -> String {
        let out = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        out
    }
}

// ── Glob ─────────────────────────────────────────────────────────

fn glob(ctx: &Ctx, input: &Value) -> Result<String, String> {
    let pattern = str_arg(input, "pattern")?;
    let base = match input.get("path").and_then(Value::as_str) {
        Some(p) if !p.trim().is_empty() => resolve(ctx, p),
        _ => effective_cwd(ctx),
    };
    guard(ctx, &base)?;

    // glob crate 以 `/` 为分隔符、`\` 是转义符 —— 必须转成正斜杠
    let joined = if Path::new(&pattern).is_absolute() {
        pattern.clone()
    } else {
        format!("{}/{}", base.display(), pattern)
    }
    .replace('\\', "/");

    let opts = glob::MatchOptions {
        case_sensitive: false,
        require_literal_separator: true,
        require_literal_leading_dot: true,
    };
    let entries = glob::glob_with(&joined, opts).map_err(|e| format!("bad pattern: {e}"))?;

    let mut hits: Vec<String> = Vec::new();
    for entry in entries {
        if let Ok(p) = entry {
            if p.is_file() {
                hits.push(p.display().to_string());
                if hits.len() >= MAX_ENTRIES {
                    break;
                }
            }
        }
    }
    if hits.is_empty() {
        return Ok(format!("No files matched {pattern}"));
    }
    hits.sort();
    Ok(hits.join("\n"))
}

// ── Grep ─────────────────────────────────────────────────────────

fn grep(ctx: &Ctx, input: &Value) -> Result<String, String> {
    let pattern = str_arg(input, "pattern")?;
    let base = match input.get("path").and_then(Value::as_str) {
        Some(p) if !p.trim().is_empty() => resolve(ctx, p),
        _ => effective_cwd(ctx),
    };
    guard(ctx, &base)?;

    let ignore_case = input
        .get("ignore_case")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let filter = input
        .get("glob")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);

    let re = regex::RegexBuilder::new(&pattern)
        .case_insensitive(ignore_case)
        .build()
        .map_err(|e| format!("bad regex: {e}"))?;

    let mut files: Vec<PathBuf> = Vec::new();
    if base.is_file() {
        files.push(base.clone());
    } else {
        walk(&base, 0, &mut files);
    }

    let mut hits: Vec<String> = Vec::new();
    'files: for file in files {
        if let Some(f) = &filter {
            if !filter_match(f, &file, &base) {
                continue;
            }
        }
        let Ok(meta) = fs::metadata(&file) else { continue };
        if meta.len() > MAX_TEXT_BYTES {
            continue;
        }
        let Ok(bytes) = fs::read(&file) else { continue };
        if is_binary(&bytes) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (i, line) in text.lines().enumerate() {
            if re.is_match(line) {
                hits.push(format!("{}:{}:{}", file.display(), i + 1, line.trim_end()));
                if hits.len() >= MAX_ENTRIES {
                    break 'files;
                }
            }
        }
    }

    if hits.is_empty() {
        return Ok(format!("No matches for {pattern}"));
    }
    Ok(hits.join("\n"))
}

// ── WebSearch ────────────────────────────────────────────────────

/// 一条搜索结果
struct SearchHit {
    title: String,
    url: String,
    snippet: String,
}

/// 主源服务商（`LUNAC_SEARCH_PROVIDER`，小写比较）。
///
/// **不做「猜服务商」**：key 只发给用户明确选中的那一家 —— 猜错等于把密钥递给无关的
/// 第三方服务器。未选 / 未知一律退回兜底源，并如实说明原因。
#[derive(Clone, Copy)]
enum SearchProvider {
    Bocha,
    Tavily,
    Exa,
    Firecrawl,
}

impl SearchProvider {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "bocha" => Some(Self::Bocha),
            "tavily" => Some(Self::Tavily),
            "exa" => Some(Self::Exa),
            "firecrawl" => Some(Self::Firecrawl),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Bocha => "bocha",
            Self::Tavily => "tavily",
            Self::Exa => "exa",
            Self::Firecrawl => "firecrawl",
        }
    }
}

/// 网页搜索。**主源 + 兜底**，两级都不调用模型：
///   ① 主源 —— `LUNAC_SEARCH_PROVIDER` 选定的搜索 API（需 `LUNAC_SEARCH_KEY`）
///   ② 兜底 —— Bing RSS → Bing HTML → 百度 HTML（无 key，见 `scraped_search`）
///
/// 两级的失败原因会一并回给模型（`[fallback] …`），否则「为什么结果这么差」
/// 在对话里无从诊断。`plan`（只读）档允许调用（网络只读），但**照常审批** ——
/// 查询串是外部数据出口，见 `gated_in_read_only`。
fn web_search(input: &Value) -> Result<String, String> {
    let query = str_arg(input, "query")?.trim().to_string();
    if query.is_empty() {
        return Err("query is empty".into());
    }
    let count = match input.get("count").and_then(Value::as_u64) {
        Some(n) => (n as usize).clamp(1, SEARCH_MAX_COUNT),
        None => SEARCH_DEFAULT_COUNT,
    };

    let provider = std::env::var("LUNAC_SEARCH_PROVIDER").unwrap_or_default();
    let provider = provider.trim().to_ascii_lowercase();
    let key = std::env::var("LUNAC_SEARCH_KEY").unwrap_or_default();
    let key = key.trim().to_string();
    let mut notes: Vec<String> = Vec::new();

    // ① 主源：只有「服务商 + key」都配齐才发请求
    if provider.is_empty() {
        notes.push("未选择搜索服务商（设置 · AI · 搜索服务商），已直接走兜底源".into());
    } else if key.is_empty() {
        notes.push("未配置搜索 API 密钥（设置 · AI · 搜索 API 密钥），已直接走兜底源".into());
    } else {
        match SearchProvider::parse(&provider) {
            None => notes.push(format!(
                "未知的搜索服务商 `{provider}`（可选 bocha / tavily / exa / firecrawl）"
            )),
            Some(sp) => match provider_search(sp, &query, count, &key) {
                Ok(hits) if !hits.is_empty() => return Ok(format_hits(&query, sp.name(), &hits)),
                Ok(_) => notes.push(format!("{} 返回 0 条结果", sp.name())),
                Err(e) => notes.push(format!("{} 失败：{e}", sp.name())),
            },
        }
    }

    // ② 兜底：抓 Bing / 百度结果页
    match scraped_search(&query, count) {
        Ok((source, hits)) => {
            let mut out = format_hits(&query, source, &hits);
            out.push_str(&format!("\n\n[fallback] {}", notes.join("；")));
            Ok(out)
        }
        Err(e) => {
            notes.push(e);
            Err(format!("WebSearch failed: {}", notes.join("；")))
        }
    }
}

/// 主源分发：四家只差 endpoint / 鉴权头 / 响应字段名
fn provider_search(
    sp: SearchProvider,
    query: &str,
    count: usize,
    key: &str,
) -> Result<Vec<SearchHit>, String> {
    match sp {
        SearchProvider::Bocha => bocha_search(query, count, key),
        SearchProvider::Tavily => tavily_search(query, count, key),
        SearchProvider::Exa => exa_search(query, count, key),
        SearchProvider::Firecrawl => firecrawl_search(query, count, key),
    }
}

/// 主源共用的 POST + JSON。非 2xx 把正文前 200 字符带回 —— 401（key 无效）、
/// 429（超额度）、402（欠费）的原因都在正文里，否则模型只看到一句「HTTP 401」。
fn post_json(
    endpoint: &str,
    auth_header: &str,
    auth_value: &str,
    body: &Value,
) -> Result<Value, String> {
    let client = http_client(SEARCH_TIMEOUT_SECS)?;
    let resp = client
        .post(endpoint)
        .header(auth_header, auth_value)
        .header(reqwest::header::USER_AGENT, FETCH_USER_AGENT)
        .json(body)
        .send()
        .map_err(|e| e.to_string())?;

    let status = resp.status();
    let (text, _) = read_body_capped(resp, SEARCH_MAX_BYTES)?;
    if !status.is_success() {
        let brief: String = text.trim().chars().take(200).collect();
        return Err(format!("HTTP {status} {brief}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("bad json: {e}"))
}

/// 一条 JSON 结果 → `SearchHit`。标题/摘要各家字段名不一，按给定顺序取第一个存在的；
/// URL 缺失或不是 http(s) 的条目直接丢弃（给模型的链接必须能点开）。
fn json_hit(v: &Value, title_key: &str, url_key: &str, snippet_keys: &[&str]) -> Option<SearchHit> {
    let url = v.get(url_key).and_then(Value::as_str)?.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    let text_at = |keys: &[&str]| -> String {
        keys.iter()
            .find_map(|k| v.get(*k).and_then(Value::as_str))
            .map(inline_text)
            .unwrap_or_default()
    };
    Some(SearchHit {
        title: text_at(&[title_key]),
        url: url.to_string(),
        snippet: truncate_chars(&text_at(snippet_keys), SEARCH_SNIPPET_CHARS),
    })
}

fn json_hits(
    items: Option<&Vec<Value>>,
    title_key: &str,
    url_key: &str,
    snippet_keys: &[&str],
) -> Vec<SearchHit> {
    items
        .map(|arr| {
            arr.iter()
                .filter_map(|r| json_hit(r, title_key, url_key, snippet_keys))
                .collect()
        })
        .unwrap_or_default()
}

/// 博查 Web Search：`POST /v1/web-search` + `Authorization: Bearer BOCHA-…`。
/// 国内直连、中文结果最好；`summary:true` 让每条结果多带一段较长的摘要。
/// 响应兼容 Bing Search API 的形状 `webPages.value[]`，外面包了一层 `data`。
fn bocha_search(query: &str, count: usize, key: &str) -> Result<Vec<SearchHit>, String> {
    let v = post_json(
        BOCHA_ENDPOINT,
        reqwest::header::AUTHORIZATION.as_str(),
        &format!("Bearer {key}"),
        &json!({ "query": query, "summary": true, "freshness": "noLimit", "count": count }),
    )?;
    let items = v
        .pointer("/data/webPages/value")
        .or_else(|| v.pointer("/webPages/value"))
        .and_then(Value::as_array);
    Ok(json_hits(items, "name", "url", &["summary", "snippet"]))
}

/// Tavily Search：`POST /search` + `Authorization: Bearer tvly-…`。
/// 只取 `results[].title/url/content`；`include_answer`/`include_raw_content`
/// 一律关掉 —— 摘要是另一个模型生成的，我们不替主模型做判断，且按 token 计费。
fn tavily_search(query: &str, count: usize, key: &str) -> Result<Vec<SearchHit>, String> {
    let v = post_json(
        TAVILY_ENDPOINT,
        reqwest::header::AUTHORIZATION.as_str(),
        &format!("Bearer {key}"),
        &json!({
            "query": query,
            "max_results": count,
            "search_depth": "basic",
            "include_answer": false,
            "include_raw_content": false,
            "include_images": false,
        }),
    )?;
    Ok(json_hits(
        v.get("results").and_then(Value::as_array),
        "title",
        "url",
        &["content"],
    ))
}

/// Exa Search：`POST /search` + `x-api-key`。`text:true` 会带回整页正文，
/// 摘要必须按 `SEARCH_SNIPPET_CHARS` 截断。
fn exa_search(query: &str, count: usize, key: &str) -> Result<Vec<SearchHit>, String> {
    let v = post_json(
        EXA_ENDPOINT,
        "x-api-key",
        key,
        &json!({ "query": query, "numResults": count, "text": true }),
    )?;
    Ok(json_hits(
        v.get("results").and_then(Value::as_array),
        "title",
        "url",
        &["text", "summary"],
    ))
}

/// Firecrawl Search：`POST /v2/search` + `Authorization: Bearer fc-…`，
/// 结果在 `data.web[]`，`description` 即默认的 Highlights 摘要。
fn firecrawl_search(query: &str, count: usize, key: &str) -> Result<Vec<SearchHit>, String> {
    let v = post_json(
        FIRECRAWL_ENDPOINT,
        reqwest::header::AUTHORIZATION.as_str(),
        &format!("Bearer {key}"),
        &json!({ "query": query, "limit": count }),
    )?;
    Ok(json_hits(
        v.pointer("/data/web").and_then(Value::as_array),
        "title",
        "url",
        &["description"],
    ))
}

/// 统一的 HTTP 客户端（超时由调用方给；重定向上限与 WebFetch 一致）
fn http_client(timeout_secs: u64) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(reqwest::redirect::Policy::limited(FETCH_MAX_REDIRECTS))
        .build()
        .map_err(|e| format!("build http client: {e}"))
}

/// 读取响应体并封顶（多读 1 字节判断是否被截断），返回 (文本, 是否截断)
fn read_body_capped(
    resp: reqwest::blocking::Response,
    cap: u64,
) -> Result<(String, bool), String> {
    let mut buf = Vec::new();
    resp.take(cap + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read response body: {e}"))?;
    let truncated = buf.len() as u64 > cap;
    if truncated {
        buf.truncate(cap as usize);
    }
    Ok((String::from_utf8_lossy(&buf).into_owned(), truncated))
}

/// 无 key 兜底：Bing RSS → Bing HTML → 百度 HTML，成功时返回 (来源标识, 结果)。
///
/// 三家都是**抓结果页而不是 API**（Bing / 百度都没有公开免费的 SERP API），所以：
///   · 只在上游不可用或失败时调用
///   · 进程内强制 ≥ `SCRAPE_MIN_INTERVAL_SECS` 间隔
///   · 解析不出结果就如实报错 —— 绝不返回空列表冒充「没有结果」
/// 三家都失败时把各自原因拼成一条 Err，便于诊断是哪家挂了。
fn scraped_search(query: &str, count: usize) -> Result<(&'static str, Vec<SearchHit>), String> {
    let mut errs: Vec<String> = Vec::new();

    match bing_rss_search(query, count) {
        Ok(hits) if !hits.is_empty() => return Ok(("bing rss (兜底)", hits)),
        Ok(_) => errs.push("Bing RSS 返回 0 条结果".into()),
        Err(e) => errs.push(format!("Bing RSS 失败：{e}")),
    }
    match bing_html_search(query, count) {
        Ok(hits) if !hits.is_empty() => return Ok(("bing html (兜底)", hits)),
        Ok(_) => errs.push("Bing HTML 返回 0 条结果".into()),
        Err(e) => errs.push(format!("Bing HTML 失败：{e}")),
    }
    match baidu_search(query, count) {
        Ok(hits) if !hits.is_empty() => return Ok(("baidu html (兜底)", hits)),
        Ok(_) => errs.push("百度返回 0 条结果".into()),
        Err(e) => errs.push(format!("百度失败：{e}")),
    }

    Err(errs.join("；"))
}

/// GET 一个结果页并按上限读回（抓取类兜底源共用）。
///
/// 浏览器 UA + `Accept-Language` + `Accept` 是必须的 —— Bing / 百度对非浏览器请求
/// 都会返回降级空壳；`referer` 给定时再带上它。
///
/// **这一套头必须凑齐（2026-09-29 实测，百度解析「失效」的真正原因）**：
/// 百度对「像浏览器的请求」才发结果页，对不像的**回一页 1488 字节的
/// `百度安全验证`（mkdjump 跳转页，正文只有「网络不给力，请稍后重试」）——
/// **HTTP 200、status 检查拦不住**，正文里却一个结果块都没有 ⇒ 解析器报「疑似改版或被反爬」，
/// 看起来像百度改版了，其实是我们自己少带了头。而且它认的是**组合**：
/// 只补 `Accept` 或只补 `Referer` 实测仍是那 1488 字节的跳转页，两种一起带才稳定出结果
/// （连跑 3 次都是 5 条）。判据一句话：**正文 ≈ 1488 字节 = 被反爬，别去改解析正则**。
fn scrape_get(
    url: &str,
    params: &[(&str, &str)],
    referer: Option<&str>,
) -> Result<String, String> {
    let client = http_client(SEARCH_TIMEOUT_SECS)?;
    let mut rb = client
        .get(url)
        .header(reqwest::header::USER_AGENT, SCRAPE_USER_AGENT)
        .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .header(reqwest::header::ACCEPT, SCRAPE_ACCEPT)
        .query(params);
    if let Some(r) = referer {
        rb = rb.header(reqwest::header::REFERER, r);
    }
    let resp = rb.send().map_err(|e| e.to_string())?;

    let status = resp.status();
    let (body, _) = read_body_capped(resp, SEARCH_MAX_BYTES)?;
    // 202 / 429 是「被限流」而不是「没结果」—— 显式区分，模型才知道该稍后重试
    if status.as_u16() == 202 || status.as_u16() == 429 {
        return Err(format!("HTTP {status}（被限流，稍后重试）"));
    }
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    Ok(body)
}

/// 兜底①：Bing 的 RSS 输出（`?q=…&format=rss`）。
///
/// 比抓 HTML 稳得多：干净 XML、`<link>` 直接就是真实 URL（没有跳转壳）、字段固定。
fn bing_rss_search(query: &str, count: usize) -> Result<Vec<SearchHit>, String> {
    throttle_scrape();
    let body = scrape_get(BING_SEARCH_ENDPOINT, &[("q", query), ("format", "rss")], None)?;
    if !body.contains("<item") {
        return Err("响应里没有 <item>（疑似改版或被反爬）".into());
    }
    Ok(parse_bing_rss(&body, count))
}

/// 从 RSS 的 `<item>` 里抠结果。通道级的 `<title>/<link>/<description>` 长得一模一样，
/// 所以必须先切出 item 块、再在块内取字段。
fn parse_bing_rss(xml: &str, count: usize) -> Vec<SearchHit> {
    let Ok(item_re) = regex::Regex::new(r"(?s)<item\s*>(.*?)</item>") else {
        return Vec::new();
    };
    let Ok(title_re) = regex::Regex::new(r"(?s)<title>(.*?)</title>") else {
        return Vec::new();
    };
    let Ok(link_re) = regex::Regex::new(r"(?s)<link>(.*?)</link>") else {
        return Vec::new();
    };
    let Ok(desc_re) = regex::Regex::new(r"(?s)<description>(.*?)</description>") else {
        return Vec::new();
    };
    let field = |re: &regex::Regex, s: &str| -> String {
        re.captures(s)
            .and_then(|c| c.get(1))
            .map(|m| xml_text(m.as_str()))
            .unwrap_or_default()
    };

    item_re
        .captures_iter(xml)
        .take(count)
        .filter_map(|c| {
            let item = c.get(1).map(|m| m.as_str()).unwrap_or("");
            let url = field(&link_re, item);
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return None;
            }
            Some(SearchHit {
                title: inline_text(&field(&title_re, item)),
                url,
                snippet: inline_text(&field(&desc_re, item)),
            })
        })
        .collect()
}

/// XML 文本节点：剥掉 `<![CDATA[ … ]]>` 外壳（Bing 偶有）再 trim
fn xml_text(s: &str) -> String {
    let s = s.trim();
    s.strip_prefix("<![CDATA[")
        .and_then(|x| x.strip_suffix("]]>"))
        .unwrap_or(s)
        .trim()
        .to_string()
}

/// 兜底②：Bing 的 HTML 结果页（RSS 被关掉或改版时的后备）。
/// 结构是 `<li class="b_algo">` 里 `<h2><a href="…">标题</a></h2>` + `b_caption` 的 `<p>` 摘要。
fn bing_html_search(query: &str, count: usize) -> Result<Vec<SearchHit>, String> {
    throttle_scrape();
    let body = scrape_get(BING_SEARCH_ENDPOINT, &[("q", query)], None)?;
    if !body.contains("b_algo") {
        return Err("响应里没有 b_algo（疑似改版或被反爬）".into());
    }
    Ok(parse_bing_html(&body, count))
}

fn parse_bing_html(html: &str, count: usize) -> Vec<SearchHit> {
    let Ok(anchor) =
        regex::Regex::new(r#"(?s)<h2[^>]*>\s*<a\s+[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#)
    else {
        return Vec::new();
    };
    let Ok(caption) = regex::Regex::new(r#"(?s)b_caption[^>]*>\s*<p[^>]*>(.*?)</p>"#) else {
        return Vec::new();
    };

    anchor
        .captures_iter(html)
        .take(count)
        .filter_map(|c| {
            let url = decode_entities(c.get(1)?.as_str()).trim().to_string();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return None;
            }
            // 摘要在锚点之后的兄弟节点里，就近找（不整页搜，避免配到别的结果上）
            let after = &html[c.get(0)?.end()..];
            let snippet = caption
                .captures(head_chars(after, 2048))
                .and_then(|m| m.get(1))
                .map(|m| inline_text(m.as_str()))
                .unwrap_or_default();
            Some(SearchHit {
                title: inline_text(c.get(2)?.as_str()),
                url,
                snippet,
            })
        })
        .collect()
}

/// 兜底③：百度 HTML。作为最后一道 —— 中文长尾查询命中率好，但页面脏得多：
/// 结果块是 `<div class="result c-container" … mu="真实 URL" …>`（`mu` 就是目标地址，
/// 不必去跟 `baidu.com/link?url=` 的 302），标题在 `<h3>` 里；摘要字段不稳定
/// （实测 `c-abstract` 命中 0），所以**只给标题与 URL，不编摘要**。
fn baidu_search(query: &str, count: usize) -> Result<Vec<SearchHit>, String> {
    throttle_scrape();
    // 百度认「像浏览器的一整套头」，见 `scrape_get` 的注释（少带就回 1488 字节的验证页）。
    let body = scrape_get(
        BAIDU_SEARCH_ENDPOINT,
        &[("wd", query), ("rn", "10")],
        Some("https://www.baidu.com/"),
    )?;
    if !body.contains("result c-container") {
        return Err("响应里没有 result c-container（疑似改版或被反爬）".into());
    }
    Ok(parse_baidu(&body, count))
}

fn parse_baidu(html: &str, count: usize) -> Vec<SearchHit> {
    let Ok(block) = regex::Regex::new(r#"class="result c-container"#) else {
        return Vec::new();
    };
    let Ok(mu) = regex::Regex::new(r#"mu="([^"]+)""#) else {
        return Vec::new();
    };
    let Ok(h3) = regex::Regex::new(r"(?s)<h3[^>]*>(.*?)</h3>") else {
        return Vec::new();
    };

    // 每块的有效范围 = 本块起点 → 下一块起点（末块到页尾）
    let starts: Vec<usize> = block.find_iter(html).map(|m| m.start()).collect();
    starts
        .iter()
        .take(count)
        .filter_map(|&start| {
            let rest = &html[start..];
            let end = starts
                .iter()
                .find(|&&s| s > start)
                .map(|&s| s - start)
                .unwrap_or(rest.len());
            let area = head_chars(rest, end);
            // `mu` 是容器自身的属性，紧跟在 class 之后（实测在首 300 字符内）
            let url = mu
                .captures(head_chars(area, 600))
                .and_then(|c| c.get(1))
                .map(|m| decode_entities(m.as_str()))
                .unwrap_or_default();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return None;
            }
            Some(SearchHit {
                title: h3.captures(area)
                    .and_then(|c| c.get(1))
                    .map(|m| inline_text(m.as_str()))
                    .unwrap_or_default(),
                url,
                snippet: String::new(),
            })
        })
        .collect()
}

/// 进程内抓取节流（多轮工具调用串行，所以一把锁即可）—— 兜底是「蹭」别人的结果页，
/// 宁可慢也不能把对方惹毛。
fn throttle_scrape() {
    static LAST: OnceLock<Mutex<Instant>> = OnceLock::new();
    let m = LAST.get_or_init(|| Mutex::new(Instant::now() - Duration::from_secs(60)));
    if let Ok(mut last) = m.lock() {
        let elapsed = last.elapsed().as_secs_f64();
        if elapsed < SCRAPE_MIN_INTERVAL_SECS {
            thread::sleep(Duration::from_secs_f64(SCRAPE_MIN_INTERVAL_SECS - elapsed));
        }
        *last = Instant::now();
    }
}

/// 取前 n 个字符（按 UTF-8 边界切 —— 结果页里全是中文，按字节切会 panic）
fn head_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// 按字符（不是字节）截断并加省略号
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// 行内文本（标题/摘要）：剥标签 + 解实体 + 把连续空白压成一个空格
fn inline_text(s: &str) -> String {
    let text = if s.contains('<') { html_to_text(s) } else { decode_entities(s) };
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn format_hits(query: &str, source: &str, hits: &[SearchHit]) -> String {
    let mut out = format!("Query: {query}\nSource: {source}\n");
    for (i, h) in hits.iter().enumerate() {
        out.push_str(&format!("\n{}. {}", i + 1, h.title));
        out.push_str(&format!("\n   {}", h.url));
        if !h.snippet.is_empty() {
            out.push_str(&format!("\n   {}", h.snippet));
        }
        out.push('\n');
    }
    out
}

// ── WebFetch ─────────────────────────────────────────────────────

/// 抓一个 URL，把页面正文转成纯文本回给模型。
///
/// 与旧 CLI 的两点差异（都是有意为之）：
///   · **不做二次模型摘要** —— 旧实现把 markdown 交给 Haiku 按 `prompt` 提炼。
///     我们直接把正文回给主模型（它本来就能读），省一次往返，也不绑死某个
///     供应商的小模型；`prompt` 因此只是提示性的，不影响返回值。
///   · **不做域名预检** —— 旧实现请求 `api.anthropic.com/api/web/domain_info`
///     拿 `can_fetch`，我们没有那个服务。安全性交给审批（见 `needs_approval`）
///     与前端白名单，agent 侧不假装自己能判断域名安不安全。
///
/// `plan`（只读）档**允许**调用：它是网络只读，不改本机任何东西。
fn webfetch(input: &Value) -> Result<String, String> {
    let url = normalize_url(&str_arg(input, "url")?)?;

    let client = http_client(FETCH_TIMEOUT_SECS)?;

    let resp = client
        .get(&url)
        .header(reqwest::header::USER_AGENT, FETCH_USER_AGENT)
        .header(
            reqwest::header::ACCEPT,
            "text/html, text/plain, text/markdown, application/json;q=0.9, */*;q=0.8",
        )
        .send()
        .map_err(|e| format!("WebFetch failed: {e}"))?;

    let status = resp.status();
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    let (body, oversized) = read_body_capped(resp, FETCH_MAX_BYTES)?;

    if !status.is_success() {
        return Err(format!("WebFetch {url} → HTTP {status}"));
    }

    let text = if ctype.contains("html") || looks_like_html(&body) {
        html_to_text(&body)
    } else {
        body.trim().to_string()
    };
    if text.is_empty() {
        return Err(format!(
            "WebFetch {url} → empty body (content-type: {})",
            if ctype.is_empty() { "unknown" } else { &ctype }
        ));
    }

    let mut out = format!("URL: {url}\nStatus: {status}\n\n{text}");
    if oversized {
        out.push_str(&format!(
            "\n\n… (body truncated at {FETCH_MAX_BYTES} bytes)"
        ));
    }
    Ok(out)
}

/// URL 规范化：去空白、`http` 升级为 `https`（与旧 CLI 一致，避免明文抓取）、
/// 长度封顶。不解析 host —— 拦截交给审批，这里只把明显不合法的挡掉。
fn normalize_url(raw: &str) -> Result<String, String> {
    let url = raw.trim();
    if url.is_empty() {
        return Err("url is empty".into());
    }
    if url.chars().count() > MAX_URL_CHARS {
        return Err(format!("url is longer than {MAX_URL_CHARS} characters"));
    }
    let url = match url.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_string(),
    };
    if !url.starts_with("https://") {
        return Err(format!("only http/https URLs are supported: {url}"));
    }
    Ok(url)
}

/// 响应体前若干字符里出现 html 特征 —— 有些站点不返回正确的 content-type
fn looks_like_html(body: &str) -> bool {
    let head: String = body.chars().take(512).collect::<String>().to_ascii_lowercase();
    head.contains("<!doctype html")
        || head.contains("<html")
        || head.contains("<head")
        || head.contains("<body")
}

/// HTML → 纯文本。不引入 DOM / turndown 依赖：去掉脚本样式与注释、把块级
/// 标签当换行、剥掉其余标签、解实体 —— 够读文档，不追求渲染级还原。
fn html_to_text(html: &str) -> String {
    // 注意：Rust 正则**不支持反向引用**，所以开闭标签只能各自列一遍
    let Ok(drop) = regex::Regex::new(
        r"(?is)<(?:script|style|noscript|svg|template)\b[^>]*>.*?</(?:script|style|noscript|svg|template)\s*>",
    ) else {
        return html.trim().to_string();
    };
    let Ok(comment) = regex::Regex::new(r"(?s)<!--.*?-->") else {
        return html.trim().to_string();
    };
    let Ok(block) = regex::Regex::new(
        r"(?i)<(?:br|hr|/p|/div|/li|/tr|/h[1-6]|/section|/article|/pre|/blockquote|/table)\b[^>]*>",
    ) else {
        return html.trim().to_string();
    };
    let Ok(tag) = regex::Regex::new(r"(?s)<[^>]*>") else {
        return html.trim().to_string();
    };

    let stripped = drop.replace_all(html, " ");
    let stripped = comment.replace_all(&stripped, " ");
    let stripped = block.replace_all(&stripped, "\n");
    let stripped = tag.replace_all(&stripped, "");
    collapse_lines(&decode_entities(&stripped))
}

/// 逐行 trim、空行折叠为一个、去掉首尾空行 —— HTML 剥完会剩大量空白
fn collapse_lines(s: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut pending_blank = false;
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() {
            pending_blank = !out.is_empty();
            continue;
        }
        if pending_blank {
            out.push("");
            pending_blank = false;
        }
        out.push(line);
    }
    out.join("\n")
}

/// HTML 实体解码。只覆盖正文高频实体与数字实体，未知实体保持原样 ——
/// 全表（2000+ 项）不值得为「读文档」这个场景引入。
fn decode_entities(s: &str) -> String {
    let Ok(re) =
        regex::Regex::new(r"&(?:#[0-9]{1,7}|#[xX][0-9a-fA-F]{1,6}|[a-zA-Z][a-zA-Z0-9]{1,31});")
    else {
        return s.to_string();
    };
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for m in re.find_iter(s) {
        out.push_str(&s[last..m.start()]);
        match entity_value(m.as_str()) {
            Some(v) => out.push_str(&v),
            None => out.push_str(m.as_str()),
        }
        last = m.end();
    }
    out.push_str(&s[last..]);
    out
}

/// `&amp;` 形态的实体 → 实际字符；无法识别返回 None（调用方保留原样）
fn entity_value(ent: &str) -> Option<String> {
    let body = ent.strip_prefix('&')?.strip_suffix(';')?;
    if let Some(num) = body.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse::<u32>().ok()?,
        };
        return char::from_u32(code).map(String::from);
    }
    let c = match body {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" | "QUOT" => '"',
        "apos" => '\'',
        "nbsp" | "ensp" | "emsp" | "thinsp" => ' ',
        "shy" => return Some(String::new()),
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "minus" => '−',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "bull" => '•',
        "middot" => '·',
        "dagger" => '†',
        "sect" => '§',
        "para" => '¶',
        "deg" => '°',
        "plusmn" => '±',
        "times" => '×',
        "divide" => '÷',
        "frac12" => '½',
        "permil" => '‰',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "ge" => '≥',
        "le" => '≤',
        "ne" => '≠',
        "asymp" => '≈',
        "infin" => '∞',
        "larr" => '←',
        "uarr" => '↑',
        "rarr" => '→',
        "darr" => '↓',
        "check" => '✓',
        "star" => '★',
        _ => return None,
    };
    Some(c.to_string())
}

/// 单选答案是字符串，多选是字符串数组，统一成可读文本
fn answer_text(a: &Value) -> String {
    match a {
        Value::Array(items) => items
            .iter()
            .map(|x| match x.as_str() {
                Some(s) => s.to_string(),
                None => x.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", "),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ── AskUserQuestion ──────────────────────────────────────────────

/// 选项由模型给，**答案由前端经审批卡的 `updatedInput` 塞回来**（见 ai-spec
/// §3.5 的 `can_use_tool` 协议：非空 `updatedInput` 覆盖原参数）。本工具自己
/// 只做一件事：把 `answers` 排成模型好读的一段文本当 tool_result。
///
/// 没有 `answers` 就**报错**而不是假装用户答了 —— 那说明这条调用没经过交互
/// 通道（没传 `--permission-prompt-tool stdio`，或用户点了卡片的「全部允许」
/// 但没选）。报错能让模型改用文本提问，编一个假答案则会污染后续推理。
fn ask_user_question(input: &Value) -> Result<String, String> {
    let Some(answers) = input.get("answers").and_then(Value::as_object) else {
        return Err(
            "No answer was collected — the interactive channel is unavailable. \
             Ask the user in plain text instead."
                .into(),
        );
    };
    if answers.is_empty() {
        return Err("No answer was collected — the user submitted nothing.".into());
    }

    // 先按模型给的题目顺序输出（`answers` 的键就是题目原文），
    // 再把没对上题的答案补在后面，免得前端改了题面时答案凭空消失
    let mut out = String::from("User has answered your questions:");
    let mut done: Vec<&str> = Vec::new();
    if let Some(list) = input.get("questions").and_then(Value::as_array) {
        for q in list {
            let Some(text) = q.get("question").and_then(Value::as_str) else {
                continue;
            };
            if let Some(a) = answers.get(text) {
                out.push_str(&format!("\n\"{text}\" = \"{}\"", answer_text(a)));
                done.push(text);
            }
        }
    }
    for (q, a) in answers {
        if !done.contains(&q.as_str()) {
            out.push_str(&format!("\n\"{q}\" = \"{}\"", answer_text(a)));
        }
    }
    Ok(out)
}

// ── TodoWrite ────────────────────────────────────────────────────

/// `TodoWrite` 不改本机、不落盘、也不需要审批：模型每次都发**完整**清单，
/// 前端直接拿 `tool_use` 里的参数画那块待办面板（见 main.ts `renderTodoPanel`）。
/// 这里只回一段确认文本 + 清单快照，让模型在后续轮次里能重新读到进度 —— 工具
/// 自己**不维护状态**（清单的唯一真相就是模型最近一条 `tool_use`），所以进程
/// 重启、多会话并行都不会串味。
fn todo_write(input: &Value) -> Result<String, String> {
    let Some(list) = input.get("todos").and_then(Value::as_array) else {
        return Err("missing required parameter \"todos\"".into());
    };

    let mut out = String::from(
        "Todos have been modified successfully. Keep the list up to date — at most one entry \
         in_progress, and flip an entry to completed as soon as it is done.",
    );
    for (i, todo) in list.iter().enumerate() {
        let content = todo.get("content").and_then(Value::as_str).unwrap_or("");
        // 状态回显成规范名（模型可能给别的词，别把它原样带回去造成漂移）
        let status = match todo.get("status").and_then(Value::as_str) {
            Some("completed") => "completed",
            Some("in_progress") => "in_progress",
            _ => "pending",
        };
        out.push_str(&format!("\n{}. [{}] {}", i + 1, status, content));
    }
    Ok(out)
}

// ── PetControl（桌宠动作，2026-10-10）─────────────────────────────

/// `PetControl` 不改本机、不落盘、也不需要审批 —— 与 `TodoWrite` 同型：**效果全在前端**。
///
/// 真正的动作由 `main.ts` 完成：它读到这条 `tool_use` 的入参就向桌宠窗**广播** `pet-control`
/// （`{type:"ai-action", action, name, value}`），由**持有 Live2D 实例**的桌宠窗落到模型上
/// （设置窗没有实例，这与热键「测试」同一条分工）。agent 侧因此只校验入参、回一句回执；
/// **不做**任何模型/文件操作，也不去猜桌宠窗在不在（前端是 fire-and-forget）。
///
/// **为什么不做成 MCP builtin**：tools/call 在 `lunac.exe --mcp-server` **另一个进程**里跑，
/// 拿不到主进程里那扇桌宠窗（无法 `emit`）；而内置工具走 stdout 流回前端，天然带 `tool_use`。
fn pet_control(input: &Value) -> Result<String, String> {
    let action = input
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let name = input.get("name").and_then(Value::as_str).unwrap_or("").trim();
    if action.is_empty() || name.is_empty() {
        return Err("PetControl needs both \"action\" and \"name\".".into());
    }
    match action {
        "hotkey" | "motion" | "param" | "expression" | "artmeshColor" => {}
        other => {
            return Err(format!(
                "Unsupported PetControl action \"{other}\" — use hotkey / motion / param / \
                 expression / artmeshColor."
            ))
        }
    }
    Ok(format!(
        "Pet action delivered to the avatar ({action} \"{name}\"). Stay in character — do not \
         describe this call or any parameter in your reply."
    ))
}

/// `ListPeers`（A13 多代理通信，2026-10-05）：列出**同批并发**的兄弟子代理。
///
/// 主代理调用时通常为空 —— 子代理批期间主循环被阻塞（见 `peers.rs` 的模块注释），
/// 所以「主代理看谁在跑」这个窗口只存在于没有子代理在跑的轮次。
fn list_peers(_input: &Value) -> Result<String, String> {
    let me = crate::peers::current();
    let peers = crate::peers::bus().list();
    if peers.is_empty() {
        return Ok("No peer subagents are currently running.".into());
    }
    let mut out = String::from("Sibling subagents currently running:");
    for (id, desc) in peers {
        let you = if me.as_deref() == Some(id.as_str()) {
            " (you)"
        } else {
            ""
        };
        out.push_str(&format!("\n- {id}{you} — {desc}"));
    }
    Ok(out)
}

/// `SendMessage`（A13 多代理通信，2026-10-05）：把一段文本投给某个存活兄弟。
///
/// `from` 取线程本地的当前 peer（主循环线程上是 `None` ⇒ 记作 `main`），投递语义与
/// 拒绝条件（目标不存在 / 收件箱满 / 内容为空）全在 `peers::PeerBus::send` 一处收口。
fn send_message(input: &Value) -> Result<String, String> {
    let to = input.get("to").and_then(Value::as_str).unwrap_or("").trim();
    if to.is_empty() {
        return Err("缺少 to 参数：目标代理的 task id（先调 ListPeers 查）".into());
    }
    let content = input.get("content").and_then(Value::as_str).unwrap_or("");
    let from = crate::peers::current().unwrap_or_else(|| "main".to_string());
    let n = crate::peers::bus().send(&from, to, content)?;
    Ok(format!(
        "Message delivered to {to} ({n} chars). That agent will read it at the start of its next round."
    ))
}

// ── SendUserFile / PushNotification（A13，2026-10-11）─────────────────
//
// 两件都是「**agent → 用户的呈递**」：对用户可见的效果**全在前端**（一张文件卡 / 一条
// 桌面通知），agent 侧只做参数校验与元信息计算。真正的渲染由 `main.rs` 在
// `dispatch_tool` 里发 `system/user_file` / `system/push_notification` 事件、前端
// `main.ts` 落卡 / 弹通知 —— 与 `PetControl` 同型（`tools.rs` 是纯实现层、拿不到 `emit`）。
//
// **审批判定（与既有约定一致）**：两件都**不需要审批**，理由逐条见 `needs_approval`。

/// `SendUserFile` 预览读取的**字节上限**：超过就不读、不给预览（大文件绝不整份塞进事件）。
const FILE_PREVIEW_MAX_BYTES: u64 = 128 * 1024;
/// 预览截断到的**字符上限**。
const FILE_PREVIEW_CHARS: usize = 1200;
/// `PushNotification` 标题 / 正文的字符上限（防止一条通知把界面撑爆）。
const PUSH_TITLE_MAX_CHARS: usize = 120;
const PUSH_BODY_MAX_CHARS: usize = 500;
/// `PushNotification` 的最短间隔（毫秒）—— **防刷屏**：间隔内的第二次调用直接拒。
pub const PUSH_NOTIFICATION_MIN_INTERVAL_MS: u128 = 3000;
/// 上一次成功发出通知的时刻（进程级；`PushNotification` 串行执行，见 `parallel_safe`）。
static LAST_PUSH_NOTIFICATION: Mutex<Option<Instant>> = Mutex::new(None);

/// 按扩展名把文件归一类（只影响前端显示的一行**类型标签**，不参与任何安全判定）。
/// 刻意**不读内容** —— 元信息要便宜；图片/二进制只给类型与大小，不给正文。
fn classify_media_kind(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tif" | "tiff"
        | "avif" | "heic" => "image",
        "pdf" => "pdf",
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "zst" => "archive",
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "wma" | "opus" => "audio",
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "wmv" | "flv" | "m4v" => "video",
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py" | "go" | "java" | "c"
        | "cc" | "cpp" | "h" | "hpp" | "cs" | "rb" | "php" | "swift" | "kt" | "sh" | "ps1"
        | "bat" | "cmd" | "sql" | "html" | "htm" | "css" | "scss" | "less" | "json" | "yaml"
        | "yml" | "toml" | "xml" | "md" | "markdown" | "ipynb" => "code",
        "txt" | "log" | "csv" | "tsv" | "ini" | "cfg" | "conf" | "text" => "text",
        _ => "other",
    }
}

/// 该类型是否允许附文本预览（只对纯文本 / 代码放行；图片、二进制一律不给正文）。
fn previewable_kind(kind: &str) -> bool {
    kind == "text" || kind == "code"
}

/// `SendUserFile`：把本地文件呈递给用户。
///
/// **路径与安全**：`resolve` + `guard` 与 `Read` **逐行同源**（相对路径按有效工作目录解析、
/// 越界在工作区锁下拒绝）。不存在的文件返回**可读错误**（`<path>: <os error>`，与 `Read`
/// 同一措辞），**绝不静默成功**。
///
/// **返回**：`(给模型的回执文本, 进 `system/user_file` 事件的载荷)` —— 载荷字段：
/// `path` / `name` / `size`（字节）/ `kind`（见 `classify_media_kind`），`preview` 仅在
/// 显式要求且为文本类时才有。
pub fn send_user_file(ctx: &Ctx, input: &Value) -> Result<(String, Value), String> {
    let path = resolve(ctx, &str_arg(input, "file_path")?);
    guard(ctx, &path)?;

    let meta = fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.is_dir() {
        return Err(format!(
            "{} is a directory — SendUserFile hands over a single file (use Glob/Grep for \
             directory trees)",
            path.display()
        ));
    }
    let size = meta.len();
    let kind = classify_media_kind(&path);

    // 预览是**可选**的、且严格有界：只对文本类、且不超过字节上限的文件读一次、截断到字符上限。
    // 大文件 / 二进制一律不读 —— 元信息已经足够（前端有「打开 / 在资源管理器中显示」）。
    let mut preview = String::new();
    let want_preview = input.get("preview").and_then(Value::as_bool).unwrap_or(false);
    if want_preview && previewable_kind(kind) && size <= FILE_PREVIEW_MAX_BYTES {
        if let Ok(bytes) = fs::read(&path) {
            let text = String::from_utf8_lossy(&bytes);
            preview = text.chars().take(FILE_PREVIEW_CHARS).collect();
        }
    }

    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let mut payload = json!({
        "path": path.display().to_string(),
        "name": name,
        "size": size,
        "kind": kind,
    });
    if !preview.is_empty() {
        payload["preview"] = json!(preview);
    }

    let receipt = format!(
        "Presented \"{}\" ({} bytes, {}) to the user as a card in the conversation.{}",
        path.display(),
        size,
        kind,
        if preview.is_empty() {
            String::new()
        } else {
            " A short text preview is attached.".to_string()
        }
    );
    Ok((receipt, payload))
}

/// 节流判据（**纯函数**，便于单测）：`None` = 允许发；`Some(remaining_ms)` = 还要等多久。
fn push_notification_wait_ms(now: Instant, last: Option<Instant>) -> Option<u128> {
    let last = last?;
    let elapsed = now.saturating_duration_since(last).as_millis();
    if elapsed >= PUSH_NOTIFICATION_MIN_INTERVAL_MS {
        None
    } else {
        Some(PUSH_NOTIFICATION_MIN_INTERVAL_MS - elapsed)
    }
}

/// `PushNotification`：给用户弹一条桌面通知（标题 + 正文）。
///
/// **防滥用**：进程级**节流** —— 距上一次成功发出 < `PUSH_NOTIFICATION_MIN_INTERVAL_MS`
/// （3 秒）时直接拒，并如实告知还要等多久；标题 / 正文各自截断到上限。
///
/// **返回**：`(回执, 进 `system/push_notification` 事件的载荷)`，载荷字段 = `title` / `body`。
pub fn push_notification(input: &Value) -> Result<(String, Value), String> {
    let title_raw = input.get("title").and_then(Value::as_str).unwrap_or("").trim();
    let body_raw = input.get("body").and_then(Value::as_str).unwrap_or("").trim();
    if title_raw.is_empty() {
        return Err("PushNotification needs a non-empty \"title\".".into());
    }
    if body_raw.is_empty() {
        return Err("PushNotification needs a non-empty \"body\".".into());
    }

    // 节流：**先判后置位**（占位即算发出，失败不占位）。锁中毒视为不可用（不静默放行刷屏）。
    {
        let mut last = LAST_PUSH_NOTIFICATION
            .lock()
            .map_err(|_| "PushNotification throttle is unavailable (lock poisoned).".to_string())?;
        if let Some(remain) = push_notification_wait_ms(Instant::now(), *last) {
            return Err(format!(
                "PushNotification is rate-limited: a notification was sent less than {PUSH_NOTIFICATION_MIN_INTERVAL_MS} ms \
                 ago — wait {remain} ms before sending another."
            ));
        }
        *last = Some(Instant::now());
    }

    let title: String = title_raw.chars().take(PUSH_TITLE_MAX_CHARS).collect();
    let body: String = body_raw.chars().take(PUSH_BODY_MAX_CHARS).collect();
    let payload = json!({ "title": title, "body": body });
    Ok((format!("Notification sent: \"{title}\"."), payload))
}

// ── EnterWorktree / ExitWorktree（A13，2026-10-11）──────────────────
//
// 「有效工作目录」的覆盖层见 `Ctx::worktree` / `effective_cwd`。这两个工具只做两件事：
// 建 worktree 并置覆盖、清覆盖。**保守边界**：绝不 `--force`、绝不删用户的 worktree /
// 分支（`ExitWorktree` 只切回、不动磁盘）。

/// 在 `dir` 下跑一次 git：成功返回 stdout（trim），失败返回 stderr（trim）。
fn run_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW —— 同 Cmd / PowerShell，GUI 下不闪黑框
    }
    let out = cmd
        .output()
        .map_err(|e| format!("failed to run git (is it installed?): {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            format!("git {} failed", args.first().copied().unwrap_or(""))
        } else {
            err
        })
    }
}

/// worktree / 分支名的合法字符（同时挡住路径穿越与非法 git ref 名）。
fn valid_worktree_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'))
}

/// worktree 目录名 = `<仓库目录名>-<name>`（2026-10-11）。
///
/// **为什么要仓库前缀**：worktree 统一落在数据根的共享目录 `<exe 根>\temp\worktrees\` 下，
/// 若目录名只用 `<name>`，**两个不同仓库用同一个名字就会撞名**（第二次 `git worktree add`
/// 会因目标目录已存在而被拒）。用**仓库根的目录名**前缀即可区分「哪个仓库 + 哪个名字」。
/// 分支名仍是 `<name>`（分支是每仓库独立的 ref，不会跨仓库撞名；同一仓库内同名再进仍按
/// 「目录已存在 ⇒ 转达 git 原文、不 `--force`」处理）。
///
/// `repo` 取仓库根路径（`git rev-parse --show-toplevel`）的末段；取不到时回退 `repo`。
fn worktree_dir_name(repo: &str, name: &str) -> String {
    let r = Path::new(repo)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("repo");
    format!("{r}-{name}")
}

/// `EnterWorktree`（A13，2026-10-11；落点 2026-10-11 改到数据根）：为当前仓库建一个 git
/// worktree，并把**有效工作目录**切过去（见 `Ctx::worktree` / `effective_cwd`）。
///
/// 落点 = `<exe 根>\temp\worktrees\<仓库目录名>-<name>`（见 [`worktree_root`] /
/// [`worktree_dir_name`]）—— **在用户仓库之外**，避免在仓库里显出未跟踪目录；该根目录由
/// main.rs 并进 `add_dirs` 放行（否则工作区锁会拒 worktree 内的写）。
/// 分支 = `git worktree add -b <name> <dir>` 新建。失败一律**状态不变**：
///   · 已在 worktree 里 ⇒ 报错要求先 `ExitWorktree`（可重入 / 幂等）；
///   · 非 git 仓库 / 找不到仓库根 ⇒ 报错；
///   · 目标目录已存在 / 分支已存在 ⇒ 转达 git 原文，**不 `--force`**。
///
/// 可观测：成功后写 INFO 日志，并把新 cwd 放进返回文本让模型知道相对路径从此落在哪
///（系统提示词**不重建** —— 规则 18 前缀不变量）。
fn enter_worktree(ctx: &Ctx, input: &Value) -> Result<String, String> {
    let name = input.get("name").and_then(Value::as_str).unwrap_or("").trim();
    if !valid_worktree_name(name) {
        return Err(
            "EnterWorktree needs a \"name\" of ASCII letters, digits, '-', '_', '/' or '.' \
             (and no '..')."
                .into(),
        );
    }
    // 已在某个 worktree 里 ⇒ 明确报错（可重入：不重复建、不乱改状态）。
    if let Ok(g) = ctx.worktree.lock() {
        if let Some(cur) = g.as_ref() {
            return Err(format!(
                "Already inside a worktree at {} — call ExitWorktree before entering another.",
                cur.display()
            ));
        }
    }
    // 必须是 git 仓库（`rev-parse --show-toplevel` 在非仓库里失败）。
    let toplevel = run_git(&ctx.cwd, &["rev-parse", "--show-toplevel"]).map_err(|e| {
        format!(
            "Not a git repository (or git is unavailable) at {}: {e}",
            ctx.cwd.display()
        )
    })?;
    if toplevel.is_empty() {
        return Err(format!("Not a git repository at {}.", ctx.cwd.display()));
    }
    let dir = worktree_root().join(worktree_dir_name(&toplevel, name));
    if dir.exists() {
        return Err(format!(
            "Worktree path already exists: {} — remove it or pick another name.",
            dir.display()
        ));
    }
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    let dir_s = dir.to_string_lossy().to_string();
    run_git(&ctx.cwd, &["worktree", "add", "-b", name, &dir_s])
        .map_err(|e| format!("git worktree add failed: {e}"))?;
    if let Ok(mut g) = ctx.worktree.lock() {
        *g = Some(dir.clone());
    }
    crate::log::info(format!(
        "EnterWorktree：有效工作目录切到 {}（新分支 {}，仓库根 {}）",
        dir.display(),
        name,
        toplevel
    ));
    Ok(format!(
        "Entered a git worktree. Your effective working directory is now:\n{}\n\
         (new branch \"{}\"; repo root {}). The worktree lives under the app's data root, \
         outside your workspace — so it does not show up as an untracked directory in the repo. \
         Relative paths and shell commands now resolve inside this worktree; absolute paths are \
         unchanged and the original workspace stays readable. Call ExitWorktree to go back to {}. \
         Nothing is forced or deleted — the worktree and its branch are left on disk; remove it \
         later with `git worktree remove <path>` if you no longer need it.",
        dir.display(),
        name,
        toplevel,
        ctx.cwd.display()
    ))
}

/// `ExitWorktree`（A13，2026-10-11）：离开 `EnterWorktree` 建的 worktree，把**有效工作目录**
/// 切回原来的工作区根。**保守**：不删 worktree、不删分支（留给用户 `git worktree remove`）；
/// 未进入就调用 ⇒ 清晰报错，状态不变。
fn exit_worktree(ctx: &Ctx) -> Result<String, String> {
    let cur = match ctx.worktree.lock() {
        Ok(g) => g.clone(),
        Err(_) => None,
    };
    let Some(cur) = cur else {
        return Err("Not currently inside a worktree — nothing to exit.".into());
    };
    if let Ok(mut g) = ctx.worktree.lock() {
        *g = None;
    }
    crate::log::info(format!(
        "ExitWorktree：有效工作目录回到 {}（worktree {} 保留在磁盘，未删除）",
        ctx.cwd.display(),
        cur.display()
    ));
    Ok(format!(
        "Left the worktree. Your effective working directory is back to:\n{}\n\
         The worktree at {} and its branch were LEFT ON DISK (nothing deleted). Remove it later \
         with `git worktree remove <path>` when you no longer need it.",
        ctx.cwd.display(),
        cur.display()
    ))
}

/// 递归收集文本文件（跳过 SKIP_DIRS，深度与数量封顶）
fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_ENTRIES * 5 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(&path, depth + 1, out);
        } else if ft.is_file() {
            out.push(path);
        }
    }
}

/// 文件过滤器：含 `/` 的按相对路径匹配，否则只比对文件名
fn filter_match(filter: &str, path: &Path, base: &Path) -> bool {
    let filter = filter.replace('\\', "/");
    if filter.contains('/') {
        let rel = path
            .strip_prefix(base)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        glob::Pattern::new(&filter)
            .map(|p| p.matches(&rel))
            .unwrap_or(false)
    } else {
        let name = path.file_name().map(|s| s.to_string_lossy().to_string());
        match (glob::Pattern::new(&filter), name) {
            (Ok(p), Some(n)) => p.matches(&n),
            _ => false,
        }
    }
}

/// 前 8KB 有 NUL 字节即视为二进制
fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|&b| b == 0)
}

/// 单条工具结果的**预算出口**（唯一出口，由 main.rs `run_tool` 调用 —— 只有那里
/// 同时知道工具名与完整输出）。
///
/// 超过 `SPILL_THRESHOLD` 字符时把**全文**落到 `output_dir()`，上下文里只内联
/// 「头 + 尾 + 行数 + 路径」；模型要全文就自己 `Read`（带 `offset`/`limit`）或
/// `Grep` 那个文件。旧实现是硬截断：超出部分对模型**永久消失**（只有
/// `LUNAC_LOG_LEVEL=debug` 能在日志里翻到），长构建日志/整页抓取经常因此表现成
/// 「像是什么都没输出」。
///
/// 落盘失败（磁盘满 / 无权限）时仍按内联预算收口，只把末尾说明换成「已丢弃」——
/// 宁可让模型知道内容不全，也不能把十几万字符塞进上下文。
pub fn apply_budget(name: &str, body: String) -> String {
    let total = body.chars().count();
    if total <= SPILL_THRESHOLD {
        return body;
    }
    let lines = body.lines().count().max(1);
    let (write_body, cut) = capped_body(&body);
    let note = match spill(name, write_body) {
        Some(path) => {
            // 落盘是「模型看不到全文」这件事的关键线索，必须留痕（只记路径与体积，不记内容）
            crate::log::info(format!(
                "tool {name} 输出超预算（{total} 字符 / {lines} 行，{}）→ {}",
                if cut { "按 1.5MB 上限截断" } else { "全文已落盘" },
                path.display()
            ));
            let path = path.display();
            if cut {
                format!(
                    "[output saved to {path} — only the first {} chars (file cap); use Read or Grep on it]",
                    write_body.chars().count()
                )
            } else {
                format!("[full output saved to {path} — use Read (with offset/limit) or Grep on it]")
            }
        }
        None => "[全文落盘失败，超出部分已丢弃]".to_string(),
    };
    preview(&body, total, lines, &note)
}

/// 落盘正文按字节封顶（切在字符边界上）。返回 (正文, 是否被砍)。
fn capped_body(body: &str) -> (&str, bool) {
    if body.len() <= MAX_SPILL_BYTES {
        return (body, false);
    }
    let mut end = MAX_SPILL_BYTES;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    (&body[..end], true)
}

/// 预览文本（纯函数，便于单测）：头 `INLINE_HEAD_CHARS` + 省略提示 + 尾 `INLINE_TAIL_CHARS` + 说明。
fn preview(body: &str, total: usize, lines: usize, note: &str) -> String {
    let head: String = body.chars().take(INLINE_HEAD_CHARS).collect();
    let tail: String = body
        .chars()
        .skip(total.saturating_sub(INLINE_TAIL_CHARS))
        .collect();
    let omitted = total.saturating_sub(INLINE_HEAD_CHARS + INLINE_TAIL_CHARS);
    format!("{head}\n\n… [{omitted} chars omitted — {total} chars / {lines} lines in total]\n\n{tail}\n\n{note}")
}

/// 落盘目录：`<exe 根>\temp\tool-outputs`。
///
/// 不新增环境变量 —— `log::log_dir()` 已经实现了「宿主注入 `LUNAC_LOG_DIR`，
/// 否则回退 `<agent.exe 目录>\temp\logs`」，取它的父目录即可同时覆盖两种情况。
pub fn output_dir() -> PathBuf {
    crate::log::log_dir()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tool-outputs")
}

/// 启动时调用：建目录、清 7 天前的落盘输出，返回目录路径 —— main.rs 要把它
/// 塞进 `Ctx.add_dirs`，否则工作区锁（默认 project 档）会让模型读不到自己的全量输出。
pub fn prepare_output_dir() -> PathBuf {
    let dir = output_dir();
    match fs::create_dir_all(&dir) {
        Ok(()) => purge_old(&dir),
        Err(e) => crate::log::warn(format!(
            "tool-outputs: 建目录失败 {}: {e}",
            dir.display()
        )),
    }
    dir
}

/// worktree 落点根：`<exe 根>\temp\worktrees`（2026-10-11，落点从工作区根改到数据根）。
///
/// 与 [`output_dir`] / `image::image_dir` 同法 —— **不新增环境变量**，从 `log::log_dir()`
/// 的父目录派生（`LUNAC_LOG_DIR` 优先，否则回退 `<agent.exe 目录>\temp\logs`），取父目录即
/// `<exe 根>\temp`。
///
/// **为什么不建在 `<cwd>\.lunac-worktrees`**：那会在用户仓库里显出一个未跟踪目录。改到
/// 应用自己的数据根后，用户仓库保持干净（对应 `EnterWorktree` 的迁移，见 [`enter_worktree`]）。
pub fn worktree_root() -> PathBuf {
    crate::log::log_dir()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("worktrees")
}

/// 启动时调用：建目录并返回路径 —— main.rs 要把它塞进 `Ctx.add_dirs`。
///
/// 理由同 [`prepare_output_dir`]：worktree 落在**工作区之外**（数据根），不进 `add_dirs`
/// 的话，默认 project 档的工作区锁（`guard`）会把 worktree 内的写直接拒掉。
pub fn prepare_worktree_dir() -> PathBuf {
    let dir = worktree_root();
    if let Err(e) = fs::create_dir_all(&dir) {
        crate::log::warn(format!("worktrees: 建目录失败 {}: {e}", dir.display()));
    }
    dir
}

/// 清理超过 `KEEP_DAYS` 的落盘输出（与落盘日志同口径，按修改时间）。失败一律忽略。
fn purge_old(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let Some(cutoff) = SystemTime::now().checked_sub(Duration::from_secs(KEEP_DAYS * 86_400))
    else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if modified < cutoff {
            let _ = fs::remove_file(&path);
        }
    }
}

/// 落盘文件的进程内序号：只读工具并行后会**同一毫秒落多个文件**，毫秒精度不够用。
static SPILL_SEQ: AtomicUsize = AtomicUsize::new(0);

/// 全文写盘，返回路径；文件名 `{毫秒}-{序号}-{工具名}.txt`。
fn spill(name: &str, body: &str) -> Option<PathBuf> {
    let dir = output_dir();
    if let Err(e) = fs::create_dir_all(&dir) {
        crate::log::warn(format!("tool-outputs: 建目录失败 {}: {e}", dir.display()));
        return None;
    }
    let millis = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let seq = SPILL_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{millis}-{seq}-{}.txt", safe_name(name)));
    match fs::write(&path, body) {
        Ok(()) => Some(path),
        Err(e) => {
            crate::log::warn(format!("tool-outputs: 写 {} 失败: {e}", path.display()));
            None
        }
    }
}

/// 工具名 → 文件名安全片段（MCP 工具名形如 `mcp__x`，仍统一过滤）
fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只验证档位判据的最小上下文：不碰磁盘，两个标志都由测试自己摆。
    fn test_ctx() -> Ctx {
        Ctx {
            cwd: PathBuf::from("."),
            add_dirs: Vec::new(),
            read_only: false,
            locked: false,
            // 单测默认不在 worktree 里（有效工作目录 = cwd）：`effective_cwd` 返回 `.`。
            worktree: Arc::new(Mutex::new(None)),
            plan_phase: Arc::new(AtomicBool::new(false)),
            // 单测不装实时回传：drain 的行为回到「只读干管道」。
            // 实时控制（后台运行 / 停止）同样不装 —— `None` 时 `run_shell` 的循环里那
            // 两个标志一次都不会被读到，行为与改造前逐字节一致。
            tool_output: None,
            shell_control: None,
            // 单测默认全局作用域；需要桌宠作用域时由用例自己摆（见 main.rs 的作用域注入）。
            memory_scope: MemoryScope::Global,
        }
    }

    /// `join_within` 是「不无限等管道 EOF」的那把闸（2026-10-03）。三条都要钉住：
    /// ① 线程很快结束时**如实拿到它的输出**；② 线程迟迟不结束时在宽限期附近返回空串
    ///（绝不阻塞到线程结束）——这正是「detached 常驻进程攥着管道」的形态；
    /// ③ `None` 直接空串（子进程退出后才转后台时可能已经丢了一个读线程）。
    #[test]
    fn join_within_bounds_the_wait_and_keeps_fast_results() {
        let fast = thread::spawn(|| "done".to_string());
        assert_eq!(join_within(Some(fast), Duration::from_secs(1)), "done");

        let slow = thread::spawn(|| {
            thread::sleep(Duration::from_secs(5));
            "late".to_string()
        });
        let t0 = Instant::now();
        let got = join_within(Some(slow), Duration::from_millis(100));
        let dt = t0.elapsed();
        assert_eq!(got, "", "宽限期内没结束应当返回空串");
        assert!(dt < Duration::from_secs(2), "只该等宽限期，实际等了 {dt:?}");

        assert_eq!(join_within(None, Duration::from_millis(10)), "");
    }

    /// 实时输出（2026-09-30）：管道分片把多字节字符劈成两半时，**不许**吐替换字符 ——
    /// 半截字符要留到下一片（`Utf8Carry`）。劈开的地方在真实场景里必然发生：一次
    /// `read` 只拿 8192 字节，中文输出随时会落在字符中间。
    #[test]
    fn utf8_carry_never_splits_a_character() {
        let bytes = "进度：50%".as_bytes();
        // 在「进」这个 3 字节字符的中间切开
        let (a, b) = bytes.split_at(4);
        let mut carry = Utf8Carry::default();
        let first = carry.push(a);
        assert!(!first.contains('\u{FFFD}'), "第一片不许出现替换字符：{first:?}");
        let second = carry.push(b);
        assert_eq!(format!("{first}{second}"), "进度：50%");
        assert_eq!(carry.flush(), "");

        // 收尾：真的收不全的残字节由 flush 兜住（宁可吐替换字符也不能把它丢掉）
        let mut carry = Utf8Carry::default();
        let _ = carry.push(&"进".as_bytes()[..1]);
        assert_eq!(carry.flush().chars().next(), Some('\u{FFFD}'));
    }

    /// Windows 下 `Cmd` **必须用 `raw_arg` 原样拼命令行**。
    ///
    /// `Command::arg` 会按 MSVC 的引号规则把参数里的 `"` 转义成 `\"`，而 `cmd /C`
    /// 的引号语义是 `cmd` 自己解释的、**不认这个转义** ⇒ 命令含空格 + 引号时整条变形
    /// （`echo "a b"` 会原样吐出 `\"a b\"`，多一层的反斜杠直接进了结果）。
    /// 这是 A9 在 hooks 上踩到的坑，复查后确认 `Cmd` 同样中招。
    ///
    /// **`PowerShell` 实测不受影响**（它的解析器认 `\"`）—— 所以那边刻意**不**改成
    /// `raw_arg`：改它就得自己重新拼一遍完整命令行，白白引入新风险。这条断言一并钉住，
    /// 防止以后有人为了「两处写法一致」把它改坏。
    #[cfg(windows)]
    #[test]
    fn quoted_shell_arguments_survive_the_command_line() {
        let ctx = test_ctx();

        let out = cmd(&ctx, &json!({ "command": "echo \"a b\"" })).expect("Cmd 应当能跑");
        assert!(out.contains("a b"), "Cmd 的引号参数被改写：{out}");
        assert!(!out.contains("\\\""), "Cmd 的输出里出现了 MSVC 转义痕迹：{out}");

        let out = powershell(&ctx, &json!({ "command": "Write-Output \"a b\"" }))
            .expect("PowerShell 应当能跑");
        assert!(out.contains("a b"), "PowerShell 的引号参数被改写：{out}");
        assert!(
            !out.contains("\\\""),
            "PowerShell 的输出里出现了 MSVC 转义痕迹：{out}"
        );
    }

    /// `Agent`（A1 子代理）必须注册进内置工具表，且能被 `--disallowedTools` 正常裁掉。
    /// 这条顺带钉住「内置工具总数」—— 数量真的变了就该有人来这里改数字，而不是悄悄漂移。
    #[test]
    fn agent_tool_is_registered_and_filterable() {
        let all = defs(&[]);
        let total = all.len();
        assert!(
            names(&all).contains(&"Agent".to_string()),
            "Agent 必须在内置工具表里"
        );
        assert!(
            names(&all).contains(&"PetControl".to_string()),
            "PetControl（桌宠动作）必须在内置工具表里"
        );
        assert!(
            names(&all).contains(&"NotebookEdit".to_string()),
            "NotebookEdit（notebook 编辑）必须在内置工具表里"
        );
        assert!(
            names(&all).contains(&"EnterWorktree".to_string())
                && names(&all).contains(&"ExitWorktree".to_string()),
            "EnterWorktree / ExitWorktree（worktree，2026-10-11）必须在内置工具表里"
        );
        assert!(
            names(&all).contains(&"SendUserFile".to_string())
                && names(&all).contains(&"PushNotification".to_string()),
            "SendUserFile / PushNotification（agent→用户呈递，2026-10-11）必须在内置工具表里"
        );
        assert_eq!(
            total, 23,
            "内置工具应为 23 件（21 件 + SendUserFile + PushNotification，2026-10-11）"
        );

        let cut = defs(&["Agent".to_string()]);
        assert!(
            !names(&cut).contains(&"Agent".to_string()),
            "disallowed 必须能裁掉 Agent"
        );
        assert_eq!(cut.len(), total - 1);
    }

    /// `NotebookEdit`（2026-10-11）的口径：
    ///   · 在 `defs()` 里（无条件注册，与 `Edit` 同族）；
    ///   · **要审批**（写盘）、不进只读并行白名单、只读档不放行；
    ///   · schema 钉住 Claude Code 口径（`notebook_path` + `new_source` 必填）。
    #[test]
    fn notebook_edit_tool_is_registered_and_gated() {
        let all = defs(&[]);
        assert!(names(&all).contains(&"NotebookEdit".to_string()));
        assert!(needs_approval("NotebookEdit"), "写盘要审批");
        assert!(!parallel_safe("NotebookEdit"), "写盘不进只读并行白名单");
        assert!(!gated_in_read_only("NotebookEdit"), "只读档不放行");

        let t = all
            .iter()
            .find(|d| d.get("name").and_then(Value::as_str) == Some("NotebookEdit"))
            .expect("NotebookEdit 必须在 defs() 里");
        let req = t
            .pointer("/input_schema/required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(req.contains(&json!("notebook_path")) && req.contains(&json!("new_source")));

        // 只读档：直接拒，且措辞指向「去设置里改档位」（file 未读就拒，故路径不存在也无妨）
        let mut ctx = test_ctx();
        ctx.read_only = true;
        let why = notebook_edit(&ctx, &json!({ "notebook_path": "x.ipynb", "new_source": "" }))
            .expect_err("只读档必须拒");
        assert!(why.contains("settings"), "拒因应指向去设置里改档位：{why}");
    }

    /// `EnterWorktree` / `ExitWorktree`（2026-10-11）的口径：
    ///   · 都在 `defs()` 里（无条件注册）；
    ///   · 都按**写类**归类（`needs_approval` 真、不进只读并行白名单、不进 `gated_in_read_only`）；
    ///   · 只读档下由 `run()` 的分派闸直接拒（不碰 git）；
    ///   · `effective_cwd` 在未进 worktree 时 = `cwd`、进了 = worktree 路径；
    ///   · `ExitWorktree` 未进入就调用 ⇒ 清晰报错。
    #[test]
    fn worktree_tools_are_registered_and_gated() {
        let all = defs(&[]);
        for n in ["EnterWorktree", "ExitWorktree"] {
            assert!(names(&all).contains(&n.to_string()), "{n} 必须在 defs() 里");
            assert!(needs_approval(n), "{n} 改仓库状态，按写类要审批");
            assert!(!parallel_safe(n), "{n} 改共享的有效工作目录，必须串行");
            assert!(!gated_in_read_only(n), "{n} 只读档该直接拒（不是问了再做）");
        }

        // 只读档：`run()` 的分派闸直接拒（不会走到 git），措辞指向去设置里改档位。
        let mut ro = test_ctx();
        ro.read_only = true;
        let why = run(&ro, "EnterWorktree", &json!({ "name": "wt1" }))
            .expect_err("只读档必须拒 EnterWorktree");
        assert!(why.contains("settings"), "拒因应指向去设置里改档位：{why}");
        let why = run(&ro, "ExitWorktree", &json!({}))
            .expect_err("只读档必须拒 ExitWorktree");
        assert!(why.contains("settings"), "拒因应指向去设置里改档位：{why}");

        // 有效工作目录：默认 = cwd；置覆盖后 = worktree 路径。
        let ctx = test_ctx();
        assert_eq!(effective_cwd(&ctx), ctx.cwd);
        *ctx.worktree.lock().unwrap() = Some(PathBuf::from("C:/x/wt"));
        assert_eq!(effective_cwd(&ctx), PathBuf::from("C:/x/wt"));

        // 未进入就退出 ⇒ 清晰报错、状态不变。
        let plain = test_ctx();
        let why = exit_worktree(&plain).expect_err("不在 worktree 里退出必须报错");
        assert!(why.contains("Not currently inside a worktree"), "{why}");
        assert!(effective_cwd(&plain) == plain.cwd, "报错后状态不变");

        // 已在 worktree 里再进 ⇒ 报错要求先退出（不碰 git）。
        let inwt = test_ctx();
        *inwt.worktree.lock().unwrap() = Some(PathBuf::from("C:/x/wt"));
        let why = enter_worktree(&inwt, &json!({ "name": "other" }))
            .expect_err("已在 worktree 里再进必须报错");
        assert!(why.contains("Already inside a worktree"), "{why}");

        // 名称非法 / 空 ⇒ 在碰 git 之前就报错。
        let c = test_ctx();
        assert!(enter_worktree(&c, &json!({ "name": "" })).is_err());
        assert!(enter_worktree(&c, &json!({ "name": "../escape" })).is_err());
    }

    /// worktree 目录名带**仓库前缀**（防跨仓库撞名，2026-10-11）：`<仓库目录名>-<name>`；
    /// 仓库根末段取不到时回退 `repo`。分支名不加前缀（分支是每仓库独立的 ref）。
    #[test]
    fn worktree_dir_name_prefixes_the_repo() {
        assert_eq!(worktree_dir_name("D:/a/b/myproj", "wt1"), "myproj-wt1");
        assert_eq!(
            worktree_dir_name(r"D:\x\repo-name", "feat/one"),
            "repo-name-feat/one"
        );
        // 末段取不到（空 / 根）⇒ 回退 `repo`
        assert_eq!(worktree_dir_name("", "wt"), "repo-wt");
        assert_eq!(worktree_dir_name("/", "wt"), "repo-wt");
    }

    /// `SendUserFile`（2026-10-11）：路径处理与 `Read` 同源、返回**可读的元信息**；
    /// 不存在的文件 / 目录都要**报错**（绝不静默成功）。
    #[test]
    fn send_user_file_reports_metadata_and_errors() {
        let dir = std::env::temp_dir().join(format!("lunac-userfile-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("report.txt");
        fs::write(&file, "hello world\n").unwrap();

        let ctx = test_ctx();
        let (receipt, payload) = send_user_file(
            &ctx,
            &json!({ "file_path": file.to_str().unwrap() }),
        )
        .expect("存在的文件应当成功");
        assert!(receipt.contains("Presented"), "回执应说明已呈递：{receipt}");
        assert_eq!(payload["kind"], json!("text"));
        assert_eq!(payload["name"], json!("report.txt"));
        assert_eq!(payload["size"], json!(12));
        // 未要求预览 ⇒ 不带 preview 字段（大文件 / 二进制默认不塞内容）
        assert!(payload.get("preview").is_none(), "默认不应有 preview：{payload}");

        // 分类：按扩展名
        let img = dir.join("pic.png");
        fs::write(&img, [0x89u8, 0x50, 0x4E, 0x47]).unwrap();
        let (_, ip) = send_user_file(&ctx, &json!({ "file_path": img.to_str().unwrap() })).unwrap();
        assert_eq!(ip["kind"], json!("image"));

        // 不存在的文件 ⇒ 可读错误（含路径），**不是** Ok
        let missing = dir.join("nope.txt");
        let err = send_user_file(&ctx, &json!({ "file_path": missing.to_str().unwrap() }))
            .expect_err("不存在的文件必须报错");
        assert!(err.contains("nope.txt"), "错误里应能看出是哪个文件：{err}");

        // 目录 ⇒ 明确拒绝（本工具只递单个文件）
        let err = send_user_file(&ctx, &json!({ "file_path": dir.to_str().unwrap() }))
            .expect_err("目录必须被拒");
        assert!(err.contains("is a directory"), "应当说明是目录：{err}");

        let _ = fs::remove_dir_all(&dir);
    }

    /// `SendUserFile` 的可选预览**严格有界**：文本被截断到字符上限，二进制 / 图片**永不**带正文。
    #[test]
    fn send_user_file_preview_is_bounded_and_binary_excluded() {
        let dir = std::env::temp_dir().join(format!("lunac-userfile-prev-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let big = dir.join("big.txt");
        let text: String = "x".repeat(FILE_PREVIEW_CHARS + 500);
        fs::write(&big, &text).unwrap();

        let ctx = test_ctx();
        let (_, payload) = send_user_file(
            &ctx,
            &json!({ "file_path": big.to_str().unwrap(), "preview": true }),
        )
        .unwrap();
        let prev = payload["preview"].as_str().expect("文本应带预览");
        assert_eq!(prev.chars().count(), FILE_PREVIEW_CHARS, "预览必须截断到上限");

        // 二进制（.png）：即使 preview=true 也不给正文
        let bin = dir.join("blob.png");
        fs::write(&bin, vec![0u8; 64]).unwrap();
        let (_, payload) =
            send_user_file(&ctx, &json!({ "file_path": bin.to_str().unwrap(), "preview": true }))
                .unwrap();
        assert!(payload.get("preview").is_none(), "二进制不应带预览：{payload}");
        assert_eq!(payload["kind"], json!("image"));

        let _ = fs::remove_dir_all(&dir);
    }

    /// `PushNotification` 的节流判据（纯函数）：间隔内要等、到点即放行。
    #[test]
    fn push_notification_wait_ms_is_a_pure_gate() {
        let t = Instant::now();
        assert!(push_notification_wait_ms(t, None).is_none(), "首次一定放行");
        assert_eq!(
            push_notification_wait_ms(t, Some(t - Duration::from_millis(1))),
            Some(PUSH_NOTIFICATION_MIN_INTERVAL_MS - 1),
            "刚发过应当要等剩余时间"
        );
        assert!(
            push_notification_wait_ms(t, Some(t - Duration::from_millis(PUSH_NOTIFICATION_MIN_INTERVAL_MS as u64)))
                .is_none(),
            "到点即放行"
        );
    }

    /// `PushNotification`：首次成功且标题/正文被截断，**紧接着的第二次被节流拒绝**（防刷屏）。
    /// 用 `LAST_PUSH_NOTIFICATION` 复位保证这条断言与其他单测互不影响。
    #[test]
    fn push_notification_throttles_and_truncates() {
        if let Ok(mut g) = LAST_PUSH_NOTIFICATION.lock() {
            *g = None;
        }
        let long_title = "T".repeat(PUSH_TITLE_MAX_CHARS + 50);
        let long_body = "B".repeat(PUSH_BODY_MAX_CHARS + 50);
        let (receipt, payload) = push_notification(
            &json!({ "title": long_title, "body": long_body }),
        )
        .expect("首次通知应当成功");
        assert_eq!(payload["title"].as_str().unwrap().chars().count(), PUSH_TITLE_MAX_CHARS);
        assert_eq!(payload["body"].as_str().unwrap().chars().count(), PUSH_BODY_MAX_CHARS);
        assert!(receipt.contains("Notification sent"), "回执：{receipt}");

        // 紧接着再来一条 ⇒ 被节流（同一次测试内，无并发干扰）
        let err = push_notification(&json!({ "title": "again", "body": "again" }))
            .expect_err("间隔内第二次必须被拒");
        assert!(err.contains("rate-limited"), "应当说明被节流：{err}");

        // 空标题 / 空正文也要拒
        if let Ok(mut g) = LAST_PUSH_NOTIFICATION.lock() {
            *g = None;
        }
        assert!(push_notification(&json!({ "title": "", "body": "x" })).is_err());
        assert!(push_notification(&json!({ "title": "x", "body": "  " })).is_err());
        if let Ok(mut g) = LAST_PUSH_NOTIFICATION.lock() {
            *g = None;
        }
    }

    /// `NotebookEdit` 必须**只动目标 cell**，并保住 `nbformat` / `metadata` / 其它 cell，
    /// 缩进沿用原文件（避免整文件 diff 噪声）。
    #[test]
    fn notebook_edit_replaces_only_the_target_cell() {
        let dir = std::env::temp_dir().join(format!("lunac-nbedit-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("nb.ipynb");
        // 手写一份 Jupyter 落盘风格（indent=1、sort_keys）的 fixture
        let original = [
            "{",
            " \"cells\": [",
            "  {",
            "   \"cell_type\": \"code\",",
            "   \"execution_count\": null,",
            "   \"id\": \"keep-me\",",
            "   \"metadata\": {},",
            "   \"outputs\": [],",
            "   \"source\": [",
            "    \"print(1)\\n\"",
            "   ]",
            "  },",
            "  {",
            "   \"cell_type\": \"markdown\",",
            "   \"id\": \"edit-me\",",
            "   \"metadata\": {},",
            "   \"source\": [",
            "    \"old\\n\"",
            "   ]",
            "  }",
            " ],",
            " \"metadata\": {},",
            " \"nbformat\": 4,",
            " \"nbformat_minor\": 5",
            "}",
            "",
        ]
        .join("\n");
        fs::write(&path, &original).unwrap();

        let ctx = test_ctx();
        let out = notebook_edit(
            &ctx,
            &json!({
                "notebook_path": path.to_str().unwrap(),
                "cell_id": "edit-me",
                "new_source": "line1\nline2\n"
            }),
        )
        .expect("NotebookEdit 应当成功");
        assert!(out.contains("Replaced"), "回执应说明做了什么：{out}");

        let after = fs::read_to_string(&path).unwrap();
        let doc: Value = serde_json::from_str(&after).unwrap();
        // 目标 cell 变了
        assert_eq!(doc.pointer("/cells/1/source"), Some(&json!(["line1\n", "line2\n"])));
        assert_eq!(
            doc.pointer("/cells/1/cell_type").and_then(Value::as_str),
            Some("markdown")
        );
        // 其它 cell / 顶层字段原样
        assert_eq!(doc.pointer("/cells/0/source"), Some(&json!(["print(1)\n"])));
        assert_eq!(doc.pointer("/cells/0/id").and_then(Value::as_str), Some("keep-me"));
        assert_eq!(doc.pointer("/cells/0/execution_count"), Some(&Value::Null));
        assert_eq!(doc.pointer("/nbformat"), Some(&json!(4)));
        assert_eq!(doc.pointer("/metadata"), Some(&json!({})));
        // 风格：缩进沿用 1 空格、末尾换行保留、未改动的 cell 逐字节不变
        assert!(after.contains("\n \"cells\": [\n"), "缩进应沿用原文件：{after}");
        assert!(after.ends_with("}\n"), "末尾换行应保留");
        assert!(
            after.contains("    \"print(1)\\n\"\n"),
            "未改动 cell 应逐字节不变：{after}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// `NotebookEdit` 的 insert / delete 两档：结构、id、其它 cell 同样保住。
    #[test]
    fn notebook_edit_inserts_and_deletes_cells() {
        let dir = std::env::temp_dir().join(format!("lunac-nbedit-id-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("nb.ipynb");
        let original = [
            "{",
            " \"cells\": [",
            "  {",
            "   \"cell_type\": \"code\",",
            "   \"execution_count\": null,",
            "   \"id\": \"c0\",",
            "   \"metadata\": {},",
            "   \"outputs\": [],",
            "   \"source\": [",
            "    \"a\"",
            "   ]",
            "  }",
            " ],",
            " \"metadata\": {},",
            " \"nbformat\": 4,",
            " \"nbformat_minor\": 5",
            "}",
            "",
        ]
        .join("\n");
        fs::write(&path, &original).unwrap();
        let ctx = test_ctx();

        // insert：在 index 0 插一个 markdown cell（用 id 的 notebook ⇒ 新 cell 也要带 id）
        notebook_edit(
            &ctx,
            &json!({
                "notebook_path": path.to_str().unwrap(),
                "cell_type": "markdown",
                "cell_number": 0,
                "new_source": "# Title",
                "edit_mode": "insert"
            }),
        )
        .expect("insert 应当成功");
        let doc: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            doc.pointer("/cells/0/cell_type").and_then(Value::as_str),
            Some("markdown")
        );
        assert_eq!(doc.pointer("/cells/0/source"), Some(&json!(["# Title"])));
        assert!(
            doc.pointer("/cells/0/id")
                .and_then(Value::as_str)
                .map_or(false, |s| !s.is_empty()),
            "用 id 的 notebook，新 cell 必须带 id"
        );
        assert_eq!(
            doc.pointer("/cells/1/id").and_then(Value::as_str),
            Some("c0"),
            "原 cell 后移且不变"
        );

        // delete：按 id 删掉 c0
        notebook_edit(
            &ctx,
            &json!({ "notebook_path": path.to_str().unwrap(), "cell_id": "c0", "edit_mode": "delete" }),
        )
        .expect("delete 应当成功");
        let doc: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            doc.pointer("/cells").and_then(Value::as_array).map(|a| a.len()),
            Some(1)
        );
        assert_eq!(
            doc.pointer("/cells/0/cell_type").and_then(Value::as_str),
            Some("markdown")
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// `NotebookEdit` 的错误必须清晰、不崩：缺文件 / 非 notebook / 找不到 cell / 参数非法。
    #[test]
    fn notebook_edit_reports_clear_errors() {
        let dir = std::env::temp_dir().join(format!("lunac-nbedit-err-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let ctx = test_ctx();

        let missing = dir.join("nope.ipynb");
        let why = notebook_edit(
            &ctx,
            &json!({ "notebook_path": missing.to_str().unwrap(), "new_source": "" }),
        )
        .expect_err("缺文件要报错");
        assert!(why.contains("nope.ipynb"), "{why}");

        let bad = dir.join("bad.ipynb");
        fs::write(&bad, "not json").unwrap();
        let why = notebook_edit(
            &ctx,
            &json!({ "notebook_path": bad.to_str().unwrap(), "new_source": "" }),
        )
        .expect_err("非法 JSON 要报错");
        assert!(why.contains("not a valid notebook"), "{why}");

        let empty = dir.join("empty.json");
        fs::write(&empty, "{\"foo\": 1}").unwrap();
        let why = notebook_edit(
            &ctx,
            &json!({ "notebook_path": empty.to_str().unwrap(), "new_source": "x" }),
        )
        .expect_err("没有 cells 要报错");
        assert!(why.contains("cells"), "{why}");

        let nb = dir.join("ok.ipynb");
        fs::write(
            &nb,
            "{\n \"cells\": [],\n \"metadata\": {},\n \"nbformat\": 4,\n \"nbformat_minor\": 5\n}\n",
        )
        .unwrap();
        let why = notebook_edit(
            &ctx,
            &json!({ "notebook_path": nb.to_str().unwrap(), "cell_id": "ghost", "new_source": "x" }),
        )
        .expect_err("找不到 cell 要报错");
        assert!(why.contains("ghost"), "{why}");

        let why = notebook_edit(
            &ctx,
            &json!({ "notebook_path": nb.to_str().unwrap(), "new_source": "x", "edit_mode": "rename" }),
        )
        .expect_err("非法 edit_mode 要报错");
        assert!(why.contains("edit_mode"), "{why}");

        let why = notebook_edit(
            &ctx,
            &json!({ "notebook_path": nb.to_str().unwrap(), "new_source": "x", "edit_mode": "insert" }),
        )
        .expect_err("insert 缺 cell_type 要报错");
        assert!(why.contains("cell_type"), "{why}");

        let _ = fs::remove_dir_all(&dir);
    }

    /// 多代理通信两件（A13，2026-10-05）的口径：
    ///   · 都在 `defs()` 里（无条件注册 —— 与 `Agent` 同族，模型要先看得到才可能在
    ///     子代理里用；子代理工具集由 `subagent_tool_defs()` 从主池推导 ⇒ 必须在主池里）；
    ///   · **都免审批**（只读写进程内的 peer 登记处，不碰本机、不发请求）；
    ///   · `ListPeers` 并行（纯读）、`SendMessage` 串行（改共享收件箱）；
    ///   · 都能被 `--disallowedTools` 裁掉；
    ///   · 都**不在** `gated_in_read_only` 里（只读档放行，与 `SessionSearch` 同级）。
    #[test]
    fn peer_tools_are_registered_and_ungated() {
        let all = defs(&[]);
        let ns = names(&all);
        for n in ["ListPeers", "SendMessage"] {
            assert!(ns.contains(&n.to_string()), "{n} 必须在内置工具表里");
            assert!(!needs_approval(n), "{n} 免审批（只读/纯内存，无本机副作用）");
            assert!(!gated_in_read_only(n), "{n} 只读档放行");
            assert!(!needs_bridge(n), "{n} 不走桥");
            let cut = defs(&[n.to_string()]);
            assert!(!names(&cut).contains(&n.to_string()), "disallowed 必须能裁掉 {n}");
        }
        assert!(parallel_safe("ListPeers"), "ListPeers 是纯读，进只读并行白名单");
        assert!(!parallel_safe("SendMessage"), "SendMessage 改共享收件箱，不并行");
    }

    /// 计划相位两件（A7）的口径：
    ///   · 都在 `defs()` 里（**无条件注册** —— 计划相位是运行期才翻转的，工具表却是
    ///     请求体里的固定前缀，事后没法增删，所以只能靠执行侧硬拒）；
    ///   · `EnterPlanMode` **免审批**（比 TodoWrite 还轻）、`ExitPlanMode` **要审批**
    ///     （那张卡就是它的产品）；
    ///   · 都不并行（前者改全局标志，后者等人回答）；
    ///   · 都能被 `--disallowedTools` 裁掉（不想让模型自作主张进计划模式的用户可以关）。
    #[test]
    fn plan_mode_tools_are_registered_and_gated() {
        let all = defs(&[]);
        for n in ["EnterPlanMode", "ExitPlanMode"] {
            assert!(
                names(&all).contains(&n.to_string()),
                "{n} 必须无条件注册：现在不放行只说明「此刻不在计划相位」，而不是这件工具不存在"
            );
            assert!(!parallel_safe(n), "{n} 必须串行");
            let cut = defs(&[n.to_string()]);
            assert_eq!(cut.len(), all.len() - 1, "disallowed 必须能裁掉 {n}");
        }
        assert!(
            !needs_approval("EnterPlanMode"),
            "进入计划模式只改一个进程内标志，不值得弹卡"
        );
        assert!(
            needs_approval("ExitPlanMode"),
            "批准计划必须过审批卡 —— 卡上那份计划是用户唯一的决策依据"
        );
        // 只读档不是「仍需审批」那一类：只读档下计划相位**退出也没意义**
        //（写类工具被用户档位永久拒绝），所以 `ExitPlanMode` 在只读档是被**拒**的，
        // 与这三件「照常放行、只补审批」的工具不同。
        assert!(!gated_in_read_only("ExitPlanMode"), "只读档不放行退出计划模式");
        assert!(!gated_in_read_only("EnterPlanMode"), "它本来就不问，不属这张表");
    }

    /// `write_blocked()` 是「写类能不能做」的唯一出口（2026-09-20 A7）：
    /// 两档的**拒因措辞必须分开**（用户该做的动作不同），放行时返回 `None`。
    #[test]
    fn write_blocked_names_the_action_the_user_must_take() {
        let mut ctx = test_ctx();
        assert_eq!(write_blocked(&ctx, "Write"), None, "默认档位下写类放行");

        ctx.plan_phase.store(true, Ordering::Relaxed);
        let why = write_blocked(&ctx, "Write").expect("计划相位必须拦住写类");
        assert!(
            why.contains("ExitPlanMode") && why.contains("approve"),
            "计划相位的拒因要指向「出计划、等批准」，实际是：{why}"
        );
        assert!(
            !why.contains("settings"),
            "别把计划相位说成「去改设置」——用户改设置也解不开它"
        );

        ctx.plan_phase.store(false, Ordering::Relaxed);
        ctx.read_only = true;
        let why = write_blocked(&ctx, "Cmd").expect("只读档必须拦住写类");
        assert!(
            why.contains("settings") && why.contains("project"),
            "只读档的拒因要指向「去设置里改档位」，实际是：{why}"
        );
        assert!(
            !why.contains("ExitPlanMode"),
            "只读档下批准计划也解不开写类的封锁，不能把模型引到那儿去"
        );

        // 两档同时成立时，报**用户档位**那条 —— 它是更硬的那道闸（计划相位解开了也没用）
        ctx.plan_phase.store(true, Ordering::Relaxed);
        let why = write_blocked(&ctx, "Edit").expect("两档叠加仍是拒");
        assert!(why.contains("settings"), "两档叠加时应报只读档，实际是：{why}");
    }

    /// 技能目录是工作区锁下**唯一**的放行例外（2026-10-06，q3 第 5 步）——
    /// 用户要「AI 运行时自建 skill，动手前提醒」，而锁下硬拒会让它连问都问不到。
    /// 四条判据：① 目录内放行；② **同前缀但不是子目录**（`skills-evil`）不放行；
    /// ③ `..` 绕过被 `normalize` 挡掉；④ 空 root 等于不开这道口子。
    #[test]
    fn skills_dir_is_the_only_workspace_lock_exception() {
        let root = Path::new(r"D:\lunac\skills");
        assert!(path_is_within(
            &normalize(Path::new(r"D:\lunac\skills\pomodoro\SKILL.md")),
            root
        ));
        assert!(path_is_within(&normalize(root), root), "目录本身也算在内");
        assert!(
            !path_is_within(
                &normalize(Path::new(r"D:\lunac\skills-evil\x.md")),
                root
            ),
            "同前缀但不是子目录 ⇒ 必须拒（按组件比，不是字符串前缀）"
        );
        assert!(
            !path_is_within(
                &normalize(Path::new(r"D:\lunac\skills\..\config\ai.json")),
                root
            ),
            "`..` 绕出 skills 之后不能还算是技能目录（resolve 已经 normalize 过）"
        );
        assert!(
            !path_is_within(&normalize(Path::new(r"D:\lunac\config\ai.json")), root),
            "完全不相干的路径不能过"
        );
        assert!(
            !path_is_within(Path::new(r"D:\lunac\skills\x.md"), Path::new("")),
            "没配技能目录 ⇒ 不开这道口子"
        );
    }

    /// `Agent` 的审批 / 并行 / 只读三条口径（A1 约束③；**A14 起「并行」一条已改**）：
    /// 要问（派生的是能写文件的代理）、**不进只读并行白名单**、只读档不放行。
    ///
    /// 关于第二条：A14 让「同一轮里的多个子代理」并发跑（上限 `SUBAGENT_PARALLELISM = 3`），
    /// 但那条路走的是 main.rs 的 `BatchKind::Subagent`（每线程一份 `detached` 的 `Cfg`），
    /// **刻意不并入 `parallel_safe` 这张只读白名单** —— 白名单里全是「不写盘、不发请求」的
    /// 纯读工具，而子代理两样都干。合进去会让「只读批」这个前提失效。
    #[test]
    fn agent_is_gated_and_serial() {
        assert!(needs_approval("Agent"), "派子代理这个决定要用户确认");
        assert!(!parallel_safe("Agent"), "子代理不走只读并行白名单（并发另有 BatchKind::Subagent）");
        assert!(!gated_in_read_only("Agent"), "只读档不放行 Agent");
    }

    /// 走桥的只读两族（A3 resources / A13 prompts）口径：**不在 `defs()` 里**（条件注册）、
    /// **必须走桥**、免审批、必须串行。
    #[test]
    fn mcp_read_side_tools_are_bridge_only_and_conditional() {
        let all = defs(&[]);
        for n in [
            "ListMcpResourcesTool",
            "ReadMcpResourceTool",
            "ListMcpPromptsTool",
            "GetMcpPromptTool",
        ] {
            assert!(
                !names(&all).contains(&n.to_string()),
                "{n} 不该出现在 defs() 里：它是条件注册的（用户没有工具文件时，它在固定前缀里纯占位）"
            );
            assert!(needs_bridge(n), "{n} 必须走 MCP 桥");
            assert!(!needs_approval(n), "{n} 只读本机自己的文件，与 Read 同级");
            assert!(!parallel_safe(n), "{n} 走单线程 stdio 桥，必须串行");
        }
        // 条件注册用的 schema 必须是一等公民形状
        for t in [
            list_resources_tool(),
            read_resource_tool(),
            list_prompts_tool(),
            get_prompt_tool(),
        ] {
            assert!(t.get("name").and_then(Value::as_str).is_some(), "缺 name");
            assert!(t.get("description").and_then(Value::as_str).is_some(), "缺 description");
            assert!(t.get("input_schema").map_or(false, Value::is_object), "缺 input_schema");
        }
        // `SessionSearch` 是「走桥但在 defs() 里」的那一个 —— 两边都要成立
        assert!(names(&all).contains(&"SessionSearch".to_string()));
        assert!(needs_bridge("SessionSearch"));
        // 反向：普通内置工具不得被误判成走桥
        assert!(!needs_bridge("Read") && !needs_bridge("Agent"));
    }

    /// `Remember`（A4 长期记忆的写入侧）的口径：**不在 `defs()` 里**（条件注册）、
    /// **必须走桥**、**要审批**（跨会话副作用，比改一个文件更持久）、必须串行。
    ///
    /// 它与 resources 读侧那两件**刻意不同**的一点是「要不要问」—— 那两件是只读，
    /// 这件是写，别为了「统一」把它改成免审批。
    #[test]
    fn remember_is_bridge_only_gated_and_serial() {
        let all = defs(&[]);
        assert!(
            !names(&all).contains(&"Remember".to_string()),
            "Remember 不该出现在 defs() 里：没桥就写不进去，注册了也是必然失败"
        );
        assert!(needs_bridge("Remember"));
        assert!(needs_approval("Remember"), "写长期记忆要用户确认（前端按运行方式决定放不放行）");
        assert!(!gated_in_read_only("Remember"), "只读档不放行 Remember");
        assert!(!parallel_safe("Remember"), "走单线程 stdio 桥，必须串行");

        let t = remember_tool();
        assert_eq!(t.get("name").and_then(Value::as_str), Some("Remember"));
        assert!(t.get("description").and_then(Value::as_str).is_some(), "缺 description");
        let required = t
            .pointer("/input_schema/required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(required.contains(&json!("content")), "content 必填");
    }

    /// 桌宠自我区（2026-10-13 人格地基）两件口径：**不在 `defs()` 里**（条件注册）、
    /// **必须走桥**、必须串行；且**没有任何一件能写核心人格**
    /// —— 桥只给「只追加」两条，合并只能走宿主命令（AI 侧到不了）。
    ///
    /// 「要不要弹卡」两件**刻意不同**：
    ///   · `PetGrowth` **要**（与 `Remember` 同判据：写的是跨会话、会被注入以后每一轮的持久文本）。
    ///   · `PetPersonaSuggest` **不要** —— 它**只写一条「建议」**、**不改任何会被注入的内容**，
    ///     真要生效还需用户之后亲手「接受 → 合并」。判据：**只写建议 ≠ 直接改人格**，
    ///     故第一道门可省（若这里也弹卡，就对「改人格」问两次）。
    #[test]
    fn pet_self_tools_are_bridge_only_serial_and_append_only() {
        let all = defs(&[]);
        for n in ["PetGrowth", "PetPersonaSuggest"] {
            assert!(!names(&all).contains(&n.to_string()), "{n} 不该出现在 defs() 里（条件注册）");
            assert!(needs_bridge(n), "{n} 必须走 MCP 桥");
            assert!(!gated_in_read_only(n), "{n} 不在「只读档仍要问」那张表里");
            assert!(!parallel_safe(n), "{n} 走单线程 stdio 桥，必须串行");
        }
        // 审批口径两件不同：成长笔记要（与 Remember 同判据），人格建议不要（只写建议、不改注入内容）
        assert!(
            needs_approval("PetGrowth") && needs_approval("Remember"),
            "PetGrowth 写跨会话持久文本，与 Remember 同判据：要审批"
        );
        assert!(
            !needs_approval("PetPersonaSuggest"),
            "PetPersonaSuggest 只写「建议」、不改任何注入内容，等用户合并才生效 ⇒ 不弹卡"
        );
        // schema 是一等公民形状，且**只有 content**（没有「改核心人格」那种入参形态）
        for t in [pet_growth_tool(), persona_suggestion_tool()] {
            assert!(t.get("name").and_then(Value::as_str).is_some(), "缺 name");
            assert!(t.get("description").and_then(Value::as_str).is_some(), "缺 description");
            let required = t
                .pointer("/input_schema/required")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(required, vec![json!("content")], "只应有 content 一个必填项");
        }
        // 「AI 无改核心人格路径」的 agent 侧证据：能到达宿主的工具**恰好**是这些（新增一件
        // 都意味着 AI 多了一条到宿主的路径，必须在这里显式登记）；**没有**任何写人格的一件。
        assert_eq!(
            BRIDGE_TOOLS,
            [
                "SessionSearch",
                "ListMcpResourcesTool",
                "ReadMcpResourceTool",
                "ListMcpPromptsTool",
                "GetMcpPromptTool",
                "Remember",
                "PetGrowth",
                "PetPersonaSuggest",
            ],
            "走桥工具清单变更必须显式登记（AI 到宿主的每条路径都在这里）"
        );
    }

    /// 没超预算的结果必须**逐字节原样**回给模型（不许多出任何说明文字、不许碰磁盘）
    #[test]
    fn under_budget_is_untouched() {
        let s = "ok".repeat(1_000); // 2000 字符 < 12k
        assert_eq!(apply_budget("Cmd", s.clone()), s);
    }

    /// 超预算：头尾都留、省略量算得对、多字节字符不许切在半路（切错会 panic）
    #[test]
    fn preview_keeps_head_and_tail_on_char_boundaries() {
        let body: String = "中文😀".chars().cycle().take(20_000).collect();
        let total = body.chars().count();
        let out = preview(&body, total, 42, "[note]");

        let head: String = body.chars().take(INLINE_HEAD_CHARS).collect();
        let tail: String = body.chars().skip(total - INLINE_TAIL_CHARS).collect();
        let omitted = total - INLINE_HEAD_CHARS - INLINE_TAIL_CHARS;

        assert!(out.starts_with(&head), "必须保留头部");
        assert!(out.ends_with("[note]"), "说明必须在最末");
        assert!(out.contains(&tail), "必须保留尾部");
        assert!(out.contains(&format!(
            "{omitted} chars omitted — {total} chars / 42 lines in total"
        )));
        // 内联体积必须真受控：头 + 尾 + 说明（说明约 150 字符）
        assert!(out.chars().count() < SPILL_THRESHOLD, "内联不该超过预算");
    }

    /// 工具名可能带 MCP 前缀/奇怪字符，文件名片段只允许字母数字与 - _
    #[test]
    fn spill_file_name_is_sanitized() {
        assert_eq!(safe_name("mcp__my-tool"), "mcp__my-tool");
        assert_eq!(safe_name("Cmd"), "Cmd");
        assert_eq!(safe_name("a/b:c*d"), "a_b_c_d");
    }

    /// 并行白名单：只读的可以并行；写类 / 命令 / MCP / 交互 / 技能一律串行
    #[test]
    fn only_read_only_tools_may_run_in_parallel() {
        for n in ["Read", "Glob", "Grep", "WebSearch", "WebFetch", "TodoWrite"] {
            assert!(parallel_safe(n), "{n} 应可并行");
        }
        for n in [
            "Write",
            "Edit",
            "Cmd",
            "PowerShell",
            "AskUserQuestion",
            // 技能按最坏模式算：fork 会派生能写文件、发 API 的子代理（A5）。
            // A14 的「多子代理并发」同样**不从这里放行** —— 它按入参解析出 fork（见
            // main.rs 的 `subagent_call()`），走 `BatchKind::Subagent` 那条专用批。
            "Skill",
            "mcp__fetch", // MCP 工具名带前缀，副作用未知且共用一条通道
            "Unknown",
        ] {
            assert!(!parallel_safe(n), "{n} 不该并行");
        }
    }

    /// 落盘正文必须留在 Read / Grep 能打开的体积内，且切在字符边界上
    #[test]
    fn spill_body_is_byte_capped_on_a_char_boundary() {
        let small = "x".repeat(1_000);
        assert_eq!(capped_body(&small), (small.as_str(), false));

        // 多字节字符铺满：砍点必然落在字符中间，必须回退到边界（否则 panic）
        let big: String = "中".repeat(MAX_SPILL_BYTES); // 3 字节/字 → 远超上限
        let (cut, was_cut) = capped_body(&big);
        assert!(was_cut);
        assert!(cut.len() <= MAX_SPILL_BYTES);
        assert!(cut.len() > MAX_SPILL_BYTES - 4, "只该回退几个字节，不该砍多");
        assert!(big.starts_with(cut));
    }

    /// 真落盘一次（唯一的碰盘测试）：文件写在 output_dir() 下、内容与全文逐字一致、
    /// 预览里给出的路径就是它 —— 也就是模型拿来 Read/Grep 的那条路径。
    #[test]
    fn spill_writes_the_full_body_into_the_output_dir() {
        let body: String = "行\n".repeat(SPILL_THRESHOLD); // 远超内联预算
        let out = apply_budget("Cmd", body.clone());
        let dir = output_dir();

        let path = fs::read_dir(&dir)
            .expect("落盘目录应已在写入前创建")
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.extension().and_then(|e| e.to_str()) == Some("txt")
                    && fs::read_to_string(p).map(|t| t == body).unwrap_or(false)
            })
            .expect("找不到刚落的文件，或文件内容与全文不一致");

        assert!(out.contains(&path.display().to_string()), "预览必须附落盘路径");
        assert!(out.ends_with("]"), "说明必须在最末");
        let _ = fs::remove_file(&path);
    }

    /// 兜底源**真机联网**验证 —— 手动跑：`cargo test -- --ignored --nocapture`
    ///
    /// 为什么不进常规 `cargo test`：依赖外网 + 对方页面结构，离线 / CI 必挂，跑一次还要
    /// 几秒（进程内 1.1s 节流）。但它**必须可一键重跑**：Bing / 百度都没有公开免费的
    /// SERP API，兜底全是抓结果页，**对方一改版就会静默退化成 0 条** —— 只有真跑才看得见。
    ///
    /// 验收口径（2026-09-17 用户定）：**四家付费服务商（bocha / tavily / exa / firecrawl）
    /// 的「成功」路径不验证** —— 预算原因拿不到可用 key，那条路径只保证「请求形状 + 错误
    /// 透传 + 回落」正确（已验）。**只要兜底链在，WebSearch 在未配 key 时就是可用的**，
    /// 这就是本测试要守的东西。
    #[test]
    #[ignore = "需要外网：真抓 Bing RSS / Bing HTML / 百度结果页"]
    fn fallback_scrapers_still_parse_live_pages() {
        let q = "Rust 异步编程";
        let sources: [(&str, fn(&str, usize) -> Result<Vec<SearchHit>, String>); 3] = [
            ("bing rss", bing_rss_search),
            ("bing html", bing_html_search),
            ("baidu", baidu_search),
        ];

        // 每家只打一次：兜底是「蹭」别人结果页，重复请求容易触发限流（202/429）。
        let mut results: Vec<(&str, Result<usize, String>)> = Vec::new();
        for (name, f) in sources {
            let r = match f(q, 5) {
                Ok(hits) => Ok(hits.len()),
                Err(e) => Err(e),
            };
            results.push((name, r));
        }
        for (name, r) in &results {
            match r {
                Ok(n) => println!("[fallback-verify] {name}: OK {n} 条"),
                Err(e) => println!("[fallback-verify] {name}: FAILED {e}"),
            }
        }

        // 「至少一级可用」等价于 `scraped_search()` 能返回结果 —— 那条链就是按顺序取
        // 第一个非空，不必再整个重跑一遍（会平白多三轮请求）。
        assert!(
            results.iter().any(|(_, r)| matches!(r, Ok(n) if *n > 0)),
            "兜底三级全部不可用 ⇒ 未配 key 时 WebSearch 彻底失效：{results:?}"
        );
        // 另外单独钉住 Bing RSS：文档标注它「比抓 HTML 稳得多」（干净 XML、`<link>` 就是
        // 真实 URL），是整条链的**首选**。不单独钉的话，「它挂了但百度还活着」会被上面
        // 那条「至少一级」掩盖过去 —— 结果能用但质量/来源会悄悄降级。
        let rss_ok = results
            .iter()
            .find(|(n, _)| *n == "bing rss")
            .map(|(_, r)| matches!(r, Ok(n) if *n > 0))
            .unwrap_or(false);
        assert!(rss_ok, "Bing RSS 兜底挂了（UA / Accept-Language 失效或对方改版）：{results:?}");
    }

    /// 子目录记忆懒加载的**触发判据**（2026-10-12，§待办 3-1a 子目录级）：
    /// 只认显式路径参数 —— 文件类取父目录、目录类取自身、其余工具与缺参一律 None。
    #[test]
    fn touched_dir_is_derived_only_from_explicit_path_args() {
        let cwd = Path::new(r"D:\ws");
        // Read/Write/Edit（file_path）→ 父目录；相对路径按 cwd 解析
        assert_eq!(
            touched_dir(cwd, "Read", &json!({ "file_path": r"D:\ws\sub\a.txt" })),
            Some(PathBuf::from(r"D:\ws\sub"))
        );
        assert_eq!(
            touched_dir(cwd, "Edit", &json!({ "file_path": r"sub\deep\b.txt" })),
            Some(PathBuf::from(r"D:\ws\sub\deep"))
        );
        // NotebookEdit 的 notebook_path 同理；SendUserFile 的 file_path 同理
        assert_eq!(
            touched_dir(cwd, "NotebookEdit", &json!({ "notebook_path": r"D:\ws\nb\x.ipynb" })),
            Some(PathBuf::from(r"D:\ws\nb"))
        );
        assert_eq!(
            touched_dir(cwd, "SendUserFile", &json!({ "file_path": r"D:\ws\out\report.txt" })),
            Some(PathBuf::from(r"D:\ws\out"))
        );
        // Glob/Grep 的 path 本身是目录
        assert_eq!(
            touched_dir(cwd, "Grep", &json!({ "path": r"D:\ws\sub" })),
            Some(PathBuf::from(r"D:\ws\sub"))
        );
        // `..` 被 normalize 吃掉，不会伪造出一个「更深」的目录
        assert_eq!(
            touched_dir(cwd, "Read", &json!({ "file_path": r"D:\ws\sub\..\a.txt" })),
            Some(PathBuf::from(r"D:\ws"))
        );
        // 缺参 / 空参 / 非字符串 → None
        assert_eq!(touched_dir(cwd, "Read", &json!({})), None);
        assert_eq!(touched_dir(cwd, "Read", &json!({ "file_path": "   " })), None);
        assert_eq!(touched_dir(cwd, "Read", &json!({ "file_path": 5 })), None);
        // 不解析命令行的工具 → None（Cmd/PowerShell 不改「当前触达目录」，见 path_arg_kind 注释）
        assert_eq!(touched_dir(cwd, "Cmd", &json!({ "command": "cd sub && dir" })), None);
        assert_eq!(touched_dir(cwd, "PowerShell", &json!({ "command": "ls" })), None);
        // 工具名不在判据表里 → None
        assert_eq!(touched_dir(cwd, "Agent", &json!({ "file_path": r"D:\ws\sub\x" })), None);
    }

    /// 「够格子目录」的三条判据：在 cwd 内且严格更深 / 任一层不在忽略名单 / 深度不超上限。
    #[test]
    fn eligible_subdir_enforces_boundary_ignores_and_depth() {
        let cwd = Path::new(r"D:\ws");
        // 正例
        assert!(eligible_subdir(cwd, Path::new(r"D:\ws\sub")));
        assert!(eligible_subdir(cwd, Path::new(r"D:\ws\a\b\c")));
        // 边界：cwd 自己、工作区之外、同前缀但不是子目录、父级/盘根
        assert!(!eligible_subdir(cwd, cwd), "cwd 本身不算子目录（那是固定前缀那一层）");
        assert!(!eligible_subdir(cwd, Path::new(r"D:\other\sub")), "工作区之外不算");
        assert!(!eligible_subdir(cwd, Path::new(r"D:\ws-evil")), "同前缀但不是子目录");
        assert!(!eligible_subdir(cwd, Path::new(r"D:\")), "父级/盘根不算");
        // 忽略名单：命中**任一层**即跳过（含其子目录）
        for bad in [
            ".git", "node_modules", "target", "dist", "out", "build", "__pycache__",
        ] {
            assert!(
                !eligible_subdir(cwd, &Path::new(r"D:\ws").join(bad)),
                "{bad} 在忽略名单里"
            );
            assert!(
                !eligible_subdir(cwd, &Path::new(r"D:\ws").join(bad).join("deep")),
                "{bad} 的子目录同样跳过"
            );
        }
        assert!(
            !eligible_subdir(cwd, Path::new(r"D:\ws\Node_Modules")),
            "忽略名单大小写无关"
        );
        // 深度上限：恰好到上限可以、再多一层不行
        let at_max: PathBuf = (1..=MAX_SUBDIR_MEMORY_DEPTH)
            .map(|i| format!("d{i}"))
            .collect();
        let over: PathBuf = (1..=MAX_SUBDIR_MEMORY_DEPTH + 1)
            .map(|i| format!("d{i}"))
            .collect();
        assert!(eligible_subdir(cwd, &Path::new(r"D:\ws").join(at_max)), "恰好到上限可以");
        assert!(!eligible_subdir(cwd, &Path::new(r"D:\ws").join(over)), "超过深度上限要跳过");
    }
}
