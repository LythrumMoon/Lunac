// core-agent/src/tools.rs
// 内置工具：Read / Write / Edit / Bash / PowerShell / Glob / Grep / WebSearch / WebFetch / AskUserQuestion / TodoWrite
//
// 工具名保持 PascalCase —— 前端 main.ts 对 "Bash" / "PowerShell" 有专门的
// 命令展示与危险命令分类分支（agentToolArgsDelta / classifyRequest /
// findLastBashGroup），改名会破坏既有 UI 契约。
//
// 权限策略（P1 无审批通道，按启动参数静态裁决；P2 接入 can_use_tool 后
// 改为「先问前端，再执行」）：
//   · --permission-mode plan      → 只读：Write / Edit / Bash / PowerShell 直接拒绝
//   · 其余档位（默认 acceptEdits）→ 内置工具全开
//   · LUNAC_WORKSPACE_LOCKED=1    → 文件类工具限制在工作区内，越界拒绝
//   · --dangerously-skip-permissions → 忽略工作区锁
//
// 本文件为 Lunac 自研实现，不派生自任何第三方源码。

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

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
/// Bash 默认/最大超时（毫秒）
const BASH_TIMEOUT_MS: u64 = 120_000;
const BASH_MAX_TIMEOUT_MS: u64 = 600_000;
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

/// 目录遍历时跳过的常见重目录（避免 Glob/Grep 卡在依赖上）
const SKIP_DIRS: [&str; 13] = [
    ".git", "node_modules", "target", "dist", "out", ".next", ".nuxt", ".venv", "venv",
    "__pycache__", ".idea", ".vscode", "build",
];

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

/// 十三个内置工具的 schema；`disallowed`（来自 `--disallowedTools`）里的
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
            "name": "Bash",
            "description": "Run a shell command (cmd /C on Windows) in the working directory \
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
                directory and return its combined output. Use this instead of Bash on Windows when \
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
            "name": "SessionSearch",
            "description": "Search the user's OWN past chat sessions in this app (a local \
                full-text index over earlier conversations) and return matching messages \
                grouped by session. Use it whenever the user refers to an earlier \
                conversation — \"last time\", \"we discussed that before\", \"上次\", \
                \"之前聊过的\" — because you cannot recall past sessions from memory. The \
                history index in your system prompt lists only titles and opening lines; \
                this tool reads what was actually said. Read-only and local: it never \
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
                so make the task description precise. It cannot launch further subagents.",
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
                write-class tool (Write / Edit / Bash / PowerShell / Agent / MCP tools) is \
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
pub const BRIDGE_TOOLS: [&str; 4] = [
    "SessionSearch",
    "ListMcpResourcesTool",
    "ReadMcpResourceTool",
    "Remember",
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

/// 是否需要先过用户审批（P2 的 `can_use_tool`）。
///
/// 只读四件（`Read`/`Glob`/`Grep`/`WebFetch`）里的前三件不需要 —— 工作区锁
/// 已是硬边界；`WebFetch` 不算，它是**唯一会把数据发往外部**的内置工具，
/// 由前端 `classifyRequest()` 决定「白名单自动放行」还是「弹卡片」。
/// 写类四件（`Write`/`Edit`/`Bash`/`PowerShell`）一律先问 —— 前端会自行处理
/// 「白名单 / 内置安全前缀自动放行」与「危险命令只给手动确认」，所以 agent
/// 侧不做二次判断，问就完了。
/// `AskUserQuestion` 也必须问：**交互本身就是它的功能**（答案经审批卡的
/// `updatedInput` 回传，不问就拿不到答案）。
/// `TodoWrite` **不问**：它只改前端那块待办面板，不碰本机任何东西。
/// 走桥的两件只读工具（`SessionSearch` / resources 读侧）**也不问** —— 它们只读本机
/// 自己的数据（会话库 / 用户的工具定义文件），与 `Read`/`Grep` 同级；**不构成先例**：
/// 判据仍是「执行会不会改变本机或把数据带出」。
///
/// `plan` 档下的例外见 [`gated_in_read_only`]。
pub fn needs_approval(name: &str) -> bool {
    matches!(
        name,
        "Write"
            | "Edit"
            | "Bash"
            | "PowerShell"
            | "WebSearch"
            | "WebFetch"
            | "AskUserQuestion"
            // `Remember` **要问**：它写的是**跨会话长期记忆**，而记忆会在用户没看见的
            // 时候被注入以后的每一次对话 —— 副作用比「改一个文件」更持久。
            // 问不代表每次都弹卡：前端按运行方式决定（自动档静默放行），复盘 fork 的
            // 写入也走同一条路（见 main.rs 的 `run_review_fork`）。
            | "Remember"
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
    )
}

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
///   · `Write` / `Edit` / `Bash` / `PowerShell` —— 有副作用，且「写文件 → 读该文件」
///     的相对顺序必须保持（并行批绝不允许跨越它们，见 `plan_tool_batches`）
///   · `Skill` —— **两种模式一读一写，按最坏的那种算**（2026-09-20 A5）：
///     inline 技能确实只读 `SKILL.md`，但 `context: fork` 的技能会派生一个**能写文件、
///     能跑命令**的子代理去发自己的 API 请求。白名单是**只看名字**的，判不出是哪一种，
///     所以整体串行 —— 代价只是少一次并行，判错的代价是并发发 API + 并发写文件。
///   · MCP 工具 —— 副作用未知，且共用一条 stdio JSON-RPC 通道
///   · `SessionSearch` —— **只读，但同样串行**：它也走那条 stdio 通道
///     （`Bridge::request` 是单线程「发一条、按 id 等一条」，并发只会互相排队甚至错配）。
///     同理还有 resources 读侧的 `ListMcpResourcesTool` / `ReadMcpResourceTool`
///     与写入侧的 `Remember`
///     —— **判据是「要不要走桥」，不是「是不是只读」**（见 [`BRIDGE_TOOLS`]）。
///   · `AskUserQuestion` —— 要等人回答，并发弹问没有意义
///   · `ExitPlanMode` —— 同 `AskUserQuestion`：它的结果**就是**用户的那一次裁决，
///     并发弹两张计划卡只会让「批准了哪一份」变得无法回答；顺带它还会改 `plan_phase`
///     这个全局标志（2026-09-20 A7）
pub fn parallel_safe(name: &str) -> bool {
    matches!(name, "Read" | "Glob" | "Grep" | "WebSearch" | "WebFetch" | "TodoWrite")
}

// ── 分发 ─────────────────────────────────────────────────────────

/// 执行一个工具调用。Err 会成为 is_error=true 的 tool_result
/// （模型看得见，可自行纠正），而不是中断整轮对话。
pub fn run(ctx: &Ctx, name: &str, input: &Value) -> Result<String, String> {
    match name {
        "Read" => read(ctx, input),
        "Write" => write(ctx, input),
        "Edit" => edit(ctx, input),
        "Bash" => bash(ctx, input),
        "PowerShell" => powershell(ctx, input),
        "Glob" => glob(ctx, input),
        "Grep" => grep(ctx, input),
        "WebSearch" => web_search(input),
        "WebFetch" => webfetch(input),
        "AskUserQuestion" => ask_user_question(input),
        "TodoWrite" => todo_write(input),
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
        normalize(&ctx.cwd.join(p))
    }
}

/// 工作区锁生效时，路径必须落在 cwd 或 `--add-dir` 之内。
fn guard(ctx: &Ctx, path: &Path) -> Result<(), String> {
    if !ctx.locked {
        return Ok(());
    }
    let inside = std::iter::once(&ctx.cwd)
        .chain(ctx.add_dirs.iter())
        .any(|root| path.starts_with(root));
    if inside {
        Ok(())
    } else {
        Err(format!(
            "Access denied: {} is outside the workspace ({})",
            path.display(),
            ctx.cwd.display()
        ))
    }
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
            "{} is too large ({} bytes, limit {MAX_TEXT_BYTES}) — read a slice with Bash instead",
            path.display(),
            meta.len()
        ));
    }

    let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
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

// ── Bash / PowerShell ────────────────────────────────────────────
//
// 两个工具除了「怎么起进程」之外完全一样：并发读干管道防死锁、超时 kill、
// 结果拼成「exit code + stdout + stderr」再走同一个上限截断，故共用 run_shell。

fn bash(ctx: &Ctx, input: &Value) -> Result<String, String> {
    if let Some(why) = write_blocked(ctx, "Bash") {
        return Err(why);
    }
    let command = str_arg(input, "command")?;
    let timeout = timeout_arg(input);

    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(&command);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(&command);
        c
    };
    run_shell(ctx, &mut cmd, timeout)
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
    let mut cmd = Command::new("powershell");
    cmd.arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(format!(
            "$OutputEncoding=[Text.Encoding]::UTF8;[Console]::OutputEncoding=[Text.Encoding]::UTF8;{command}"
        ));
    run_shell(ctx, &mut cmd, timeout)
}

fn timeout_arg(input: &Value) -> u64 {
    input
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(BASH_TIMEOUT_MS)
        .min(BASH_MAX_TIMEOUT_MS)
}

/// 起进程 → 读干输出 → 超时 kill → 拼结果文本。
fn run_shell(ctx: &Ctx, cmd: &mut Command, timeout: u64) -> Result<String, String> {
    let prog = cmd.get_program().to_string_lossy().to_string();
    cmd.current_dir(&ctx.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // CREATE_NO_WINDOW —— GUI 宿主下不加会闪黑框
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            // spawn 失败（解释器不存在 / 被拦）是最需要事后取证的一类错误
            let msg = format!("spawn failed: {e}");
            crate::log::warn(format!("shell[{prog}] {msg}"));
            return Err(msg);
        }
    };
    let out_pipe: Option<ChildStdout> = child.stdout.take();
    let err_pipe = child.stderr.take();
    // 必须并发读干管道，否则子进程写满缓冲区后会卡死
    let h_out = thread::spawn(move || drain(out_pipe));
    let h_err = thread::spawn(move || drain(err_pipe));

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if started.elapsed() > Duration::from_millis(timeout) {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("wait failed: {e}")),
        }
    };

    let stdout = h_out.join().unwrap_or_default();
    let stderr = h_err.join().unwrap_or_default();

    let mut out = String::new();
    if timed_out {
        out.push_str(&format!("(timed out after {timeout} ms — process killed)\n"));
    }
    match status.and_then(|s| s.code()) {
        Some(code) => out.push_str(&format!("exit code: {code}\n")),
        None if !timed_out => out.push_str("exit code: (none)\n"),
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
        "shell[{prog}] exit={} timeout={} stdout={}B stderr={}B",
        code.map_or_else(|| "-".into(), |c| c.to_string()),
        timed_out,
        stdout.len(),
        stderr.len()
    ));
    if !stderr.trim().is_empty() {
        crate::log::warn(format!("shell[{prog}] stderr:\n{stderr}"));
    }

    Ok(out)
}

/// 把管道读干（超上限后继续读但丢弃，防止子进程阻塞）
fn drain<R: Read>(pipe: Option<R>) -> String {
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if buf.len() < MAX_PIPE_BYTES {
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

// ── Glob ─────────────────────────────────────────────────────────

fn glob(ctx: &Ctx, input: &Value) -> Result<String, String> {
    let pattern = str_arg(input, "pattern")?;
    let base = match input.get("path").and_then(Value::as_str) {
        Some(p) if !p.trim().is_empty() => resolve(ctx, p),
        _ => ctx.cwd.clone(),
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
        _ => ctx.cwd.clone(),
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

/// GET 一个结果页并按上限读回（抓取类兜底源共用）。浏览器 UA + `Accept-Language`
/// 是必须的 —— Bing / 百度对非浏览器 UA 会返回降级空壳。
fn scrape_get(url: &str, params: &[(&str, &str)]) -> Result<String, String> {
    let client = http_client(SEARCH_TIMEOUT_SECS)?;
    let resp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, SCRAPE_USER_AGENT)
        .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .query(params)
        .send()
        .map_err(|e| e.to_string())?;

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
    let body = scrape_get(BING_SEARCH_ENDPOINT, &[("q", query), ("format", "rss")])?;
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
    let body = scrape_get(BING_SEARCH_ENDPOINT, &[("q", query)])?;
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
    let body = scrape_get(BAIDU_SEARCH_ENDPOINT, &[("wd", query), ("rn", "10")])?;
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
            plan_phase: Arc::new(AtomicBool::new(false)),
        }
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
        assert_eq!(total, 15, "内置工具应为 15 件（12 件原有 + Agent + 计划相位两件）");

        let cut = defs(&["Agent".to_string()]);
        assert!(
            !names(&cut).contains(&"Agent".to_string()),
            "disallowed 必须能裁掉 Agent"
        );
        assert_eq!(cut.len(), total - 1);
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
        let why = write_blocked(&ctx, "Bash").expect("只读档必须拦住写类");
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

    /// `Agent` 的审批 / 并行 / 只读三条口径（A1 约束③）：
    /// 要问（派生的是能写文件的代理）、必须串行（并发派子代理 = 并发花钱 + 争抢文件）、
    /// 只读档不放行。
    #[test]
    fn agent_is_gated_and_serial() {
        assert!(needs_approval("Agent"), "派子代理这个决定要用户确认");
        assert!(!parallel_safe("Agent"), "子代理必须串行执行");
        assert!(!gated_in_read_only("Agent"), "只读档不放行 Agent");
    }

    /// resources 读侧两件（A3）的口径：**不在 `defs()` 里**（条件注册）、**必须走桥**、
    /// 免审批、必须串行。
    #[test]
    fn mcp_resource_tools_are_bridge_only_and_conditional() {
        let all = defs(&[]);
        for n in ["ListMcpResourcesTool", "ReadMcpResourceTool"] {
            assert!(
                !names(&all).contains(&n.to_string()),
                "{n} 不该出现在 defs() 里：它是条件注册的（用户没有工具文件时，它在固定前缀里纯占位）"
            );
            assert!(needs_bridge(n), "{n} 必须走 MCP 桥");
            assert!(!needs_approval(n), "{n} 只读本机自己的文件，与 Read 同级");
            assert!(!parallel_safe(n), "{n} 走单线程 stdio 桥，必须串行");
        }
        // 条件注册用的 schema 必须是一等公民形状
        for t in [list_resources_tool(), read_resource_tool()] {
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

    /// 没超预算的结果必须**逐字节原样**回给模型（不许多出任何说明文字、不许碰磁盘）
    #[test]
    fn under_budget_is_untouched() {
        let s = "ok".repeat(1_000); // 2000 字符 < 12k
        assert_eq!(apply_budget("Bash", s.clone()), s);
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
        assert_eq!(safe_name("Bash"), "Bash");
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
            "Bash",
            "PowerShell",
            "AskUserQuestion",
            // 技能按最坏模式算：fork 会派生能写文件、发 API 的子代理（A5）
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
        let out = apply_budget("Bash", body.clone());
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
}
