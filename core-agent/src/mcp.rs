// core-agent/src/mcp.rs
// Lunac 自研 agent 后端 —— P3：MCP 工具桥（stdio client + 远端 Streamable HTTP）
//
// src-tauri 在 spawn agent.exe 时会带 `--mcp-server stdio:<lunac.exe 路径>`。
// 本模块据此把 lunac.exe 以 `--mcp-server` 拉起 —— 那个进程会拦截该参数、
// 进入 stdio MCP server 模式，读取 `<exe 根>\tools\*.json` 里的用户自定义
// 工具（handler 有 shell / http / builtin 三种，实现在 src-tauri/mcp_server.rs）。
//
// 握手顺序（MCP 2025-06-18）：
//   initialize → notifications/initialized → tools/list → （模型调用时）tools/call
//
// 除工具调用外，本桥还承载两组**自定义方法**（`lunac/history_index` / `lunac/history_search`
// 与 `lunac/memory_read` / `lunac/memory_write`）—— 它们**不列进 `tools/list`**，因此不出现在
// 用户的工具列表里、也不弹审批卡；支撑的是内置工具 `SessionSearch`（往期会话检索，2026-09-19）
// 与 `Remember`（长期记忆，2026-09-20）。
// 不列进去的理由见 `app/src-tauri/src/mcp_server.rs` 的 `handle_history_method`。
//
// 另有两条**标准** MCP 方法由本模块直接用：`resources/list` / `resources/read`（A3，2026-09-20），
// 支撑条件注册的两件只读工具 `ListMcpResourcesTool` / `ReadMcpResourceTool`。
// `prompts/list` / `prompts/get`（A13，2026-10-04）同理，支撑 `ListMcpPromptsTool` / `GetMcpPromptTool`。
//
// **服务端反向请求**（A13）：宿主那条 stdio 桥（`lunac.exe --mcp-server`）还会**主动**向客户端
// 发两条标准请求 —— `roots/list`（客户端答工作区根，服务端拿它当 shell handler 的 cwd）与
// `elicitation/create`（客户端弹卡收集用户输入，供工具 JSON 里声明了 `elicit` 的用户工具用）。
// 这两条只在**声明了对应能力**的桥上发生；远端 HTTP 服务器不声明、也不处理（见 `handshake`）。
//
// 工具名统一加 `mcp__` 前缀，避免与内置工具重名；前缀必须稳定，因为
// 前端审批卡的「始终允许」是按完整工具名记进 localStorage 白名单的。
//
// 桥是尽力而为：连不上 / 握手失败只往 stderr 记一行，内置工具照常工作
// （往期会话索引也只是不注入，不影响任何其它能力）。
//
// ── 远端服务器（Streamable HTTP，2026-10-01）─────────────────────────
//
// 除宿主那条 stdio 桥之外，用户还可以在 `<exe 根>\config\mcp.json` 里声明**远端 MCP 服务器**
// （宿主任一情况都注入 `LUNAC_MCP_FILE` 这个路径，文件不存在 = 没配）。多条服务器由
// [`McpSet`] 汇总：它对上层是一个「有没有工具 / 调哪个工具」的集合，内部按工具名路由。
//
// **传输只做 Streamable HTTP**（规范里的 `POST` + `application/json` / `text/event-stream`
// 两种回包；会话靠 `Mcp-Session-Id` 头续）。**不做**已废弃的 HTTP+SSE 双端点传输。
//
// 授权两种，由配置的 `auth` 字段选（缺省 `static`）：
//   · `static` —— 凭据只能写死在 `headers` 里（长期 Bearer，用户自己去服务器那边抓）；
//   · `oauth`  —— OAuth 2.1（动态客户端注册 + Authorization Code + PKCE），令牌落
//                 `config\mcp-tokens.json`，全程在 agent 侧完成（宿主与前端一行不改）。
// **交互式授权只发生在「连服务器那一步」**；`tools/call` 拿到 401 只刷新令牌、**不开浏览器**
// （一次工具调用弹一个窗口是最糟的体验）。细节与边界见 `mcp_oauth.rs` 的文件头。
//
// **明文 http 只放行本机**（`localhost` / `127.0.0.1` / `[::1]`）：本地 MCP server 常用 http，
// 而远端 http 会把 `headers` 里的凭据明文发出去。与「插件市场只收 https」同一条判断。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use crate::mcp_oauth;

const PREFIX: &str = "mcp__";
/// 协商的协议版本。A13 起升到 `2025-06-18`（elicitation 是该版本引入的能力；
/// roots/prompts 两版都有，统一到一个版本号最省事）。
const PROTOCOL_VERSION: &str = "2025-06-18";
/// 握手（initialize / tools/list）等待上限
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// 单次 tools/call 上限（shell handler 自身 60s 超时，http 可能更慢）
const CALL_TIMEOUT: Duration = Duration::from_secs(180);
/// 工具名限字符集与 ≤64 长度（Anthropic 兼容端点的校验），故原名上限要减去前缀
const MAX_RAW_NAME: usize = 64 - PREFIX.len();
/// 远端服务器配置文件的路径来源（宿主**无条件**注入，文件不存在 = 没配）。
const MCP_FILE_ENV: &str = "LUNAC_MCP_FILE";
/// 项目级 MCP 配置的文件名（`<工作目录>\<这个名字>`，q3）。
const PROJECT_MCP_FILE: &str = ".mcp.json";
/// 单台服务器最多接入的工具数。一份坏清单（或恶意服务器）不该把固定前缀撑爆 ——
/// tools 数组是每次请求都要发的一部分。
const MAX_TOOLS_PER_SERVER: usize = 100;

/// 是否为本桥接进来的 MCP 工具（按名字前缀判定）
pub fn is_mcp(name: &str) -> bool {
    name.starts_with(PREFIX)
}

// ── 普通 stdio 服务器配置（`.mcp.json`，2026-10-06，q3 第 1 步）────────
//
// 本仓原有的 stdio 连接只有**宿主那条桥**一种形态（`stdio:<lunac.exe>`，写死 `--mcp-server`
// 参数、且 `lunac: true`）。项目级 `.mcp.json` 里的服务器是**任意命令**（`npx` / `python` …），
// 所以这里把「怎么起进程」与「起了之后怎么聊」拆开：连接内核 [`Bridge::connect_stdio_cmd`]
// 只认一个已经装好的 `Command`，spec 解析（宿主桥）与配置解析（.mcp.json）各自构造它。
//
// ⚠️ 这条路上的服务器**不是** Lunac 自己的 server ⇒ `lunac: false`：不声明 roots /
// elicitation 能力、不发 `lunac/*` 自定义方法（同远端 HTTP 服务器那条口径）。

/// 一台**普通** stdio MCP 服务器（来自 `.mcp.json` 的 `mcpServers` 条目）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StdioServerConfig {
    pub name: String,
    /// 可执行文件（裸名走 PATH 查，与 `Command::new` 同一口径）。
    pub command: String,
    pub args: Vec<String>,
    /// 追加 / 覆盖到子进程环境的键值（**继承父进程环境**，只改这里列的）。
    pub env: Vec<(String, String)>,
}

// ── 远端服务器配置（`config\mcp.json`）─────────────────────────────

/// 一台远端 MCP 服务器。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    pub name: String,
    pub url: String,
    /// 逐条加在请求头上的键值（`Authorization: Bearer …` 这类）。
    /// **`auth: oauth` 时其余头仍然会带上**（有的服务器既要 OAuth 又要自定义头，比如租户 id），
    /// 只有配置里的 `Authorization` 会被忽略 —— 改由令牌填，同一个头出现两份会被服务器拒。
    pub headers: Vec<(String, String)>,
    /// 授权方式。默认 [`AuthMode::Static`]（只用上面的 `headers`）。
    pub auth: AuthMode,
}

/// 远端服务器的授权方式（`config\mcp.json` 的 `auth` 字段）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMode {
    /// 只用配置里的静态 `headers` —— 用户自己去服务器那边抓一个长期令牌填进来。
    /// **默认**：不配 `auth` 就是这个，绝不会突然弹一个浏览器。
    Static,
    /// OAuth 2.1（`"auth": "oauth"`）：没有有效令牌时，**在连服务器那一步**开浏览器授权，
    /// 令牌与 refresh_token 落 `config\mcp-tokens.json`。见 `mcp_oauth.rs`。
    OAuth,
}

/// `LUNAC_MCP_FILE` 指向的路径（宿主**无条件**注入；空 / 缺失 = 没配远端服务器）。
fn remote_config_path() -> Option<PathBuf> {
    std::env::var(MCP_FILE_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// MCP 配置改动监视（2026-10-06，q3 后续）—— **只报告，不自己重启**。
///
/// 为什么需要它：`config\mcp.json` 与 `<工作目录>\.mcp.json` 都在 **agent 启动时读一次**
/// （工具表进固定前缀，§11 规则 18）⇒ 改了必须重启 agent 才生效，且**不能**热重载。
/// 而「重启 agent」agent 自己做不到（它是宿主的子进程），所以这里只做
/// 「惰性 mtime 比对 + 上报一个 `system` 事件」，由前端在**本回合结束后**重启。
///
/// 比对思路与 `hooks.rs` 的 mtime 热重载同源（每次事件 stat 一次，零线程零依赖），
/// 但结论相反：那边 mtime 一变就地换配置，这边只能请前端重启。
pub struct ConfigWatch {
    /// `(展示名, 路径, 启动时 mtime)` —— `None` = 启动时文件不存在。
    files: Vec<(String, PathBuf, Option<SystemTime>)>,
    /// 已经报过（**只报一次**）：每回合都报会让前端反复重启。
    notified: bool,
}

impl ConfigWatch {
    /// 在 agent 启动时拍一次快照 —— **必须与真正读配置的时机一致**，否则会把「启动前刚改过」
    /// 误判成「启动后被改」（那会白重启一次）。
    pub fn snapshot(cwd: &Path) -> Self {
        Self::snapshot_with(remote_config_path(), cwd)
    }

    /// 抽出「远端路径可注入」的版本，供单测隔离 `LUNAC_MCP_FILE` 环境变量。
    fn snapshot_with(remote: Option<PathBuf>, cwd: &Path) -> Self {
        let mut files = Vec::new();
        if let Some(p) = remote {
            let m = mtime_of(&p);
            files.push(("config\\mcp.json".to_string(), p, m));
        }
        let p = cwd.join(PROJECT_MCP_FILE);
        let m = mtime_of(&p);
        files.push((PROJECT_MCP_FILE.to_string(), p, m));
        Self { files, notified: false }
    }

    /// 启动后被改过的文件展示名。**只报一次** —— 报过之后再调一律返回空。
    /// 判据是 mtime 而不是内容：只看「有没有被写过」，不解析、不比对字节。
    pub fn changed(&mut self) -> Vec<String> {
        if self.notified {
            return Vec::new();
        }
        let hit: Vec<String> = self
            .files
            .iter()
            .filter(|(_, p, was)| mtime_of(p) != *was)
            .map(|(name, _, _)| name.clone())
            .collect();
        if !hit.is_empty() {
            self.notified = true;
        }
        hit
    }
}

/// 读 `LUNAC_MCP_FILE` 指向的配置。**任何失败都只是「没有远端服务器」**，不阻断启动。
pub fn load_config() -> Vec<ServerConfig> {
    let Some(path) = remote_config_path() else {
        return Vec::new();
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        // 文件不在 = 没配（正常状态，出厂就没有这个文件）
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            eprintln!(
                "[agent] MCP 配置读不出（{}）：{e} —— 本轮只有宿主那条桥",
                path.display()
            );
            return Vec::new();
        }
    };
    let (servers, warnings) = parse_config(&text);
    for w in &warnings {
        eprintln!("[agent] MCP 配置：{w}");
    }
    servers
}

/// 解析配置文本，返回 `(合法条目, 逐条告警)`。
///
/// **坏条目只丢自己**（与插件市场索引同一条纪律）：一条写错 URL 的服务器不该让整份配置失效。
/// 抽成纯函数是为了能单测「http 只放行本机」「缺字段跳过」这两条判据。
fn parse_config(text: &str) -> (Vec<ServerConfig>, Vec<String>) {
    let mut out = Vec::new();
    let mut warn = Vec::new();
    let root: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return (out, vec![format!("语法不是合法 JSON（{e}），整份忽略")]),
    };
    let Some(list) = root.get("servers").and_then(Value::as_array) else {
        return (out, vec!["缺少 servers 数组，整份忽略".into()]);
    };
    for (i, item) in list.iter().enumerate() {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let url = item
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() || url.is_empty() {
            warn.push(format!("第 {} 条缺少 name / url，已跳过", i + 1));
            continue;
        }
        if !url_allowed(&url) {
            warn.push(format!(
                "{name}：只接受 https（明文 http 仅限本机 localhost / 127.0.0.1），已跳过"
            ));
            continue;
        }
        let Some(auth) = parse_auth(item) else {
            // 写错 `auth` 却不吭声，用户会以为配上了 —— 与「坏条目只丢自己」同一条纪律
            warn.push(format!(
                "{name}：auth 只认 \"static\" / \"oauth\"（现在是 {}），已跳过",
                item.get("auth").map(Value::to_string).unwrap_or_default()
            ));
            continue;
        };
        let headers = item
            .get("headers")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        out.push(ServerConfig {
            name,
            url,
            headers,
            auth,
        });
    }
    (out, warn)
}

/// 解析 `auth` 字段。缺省（不写 / `null` / 空串 / `"static"`）= [`AuthMode::Static`]，
/// 用户不主动写 `oauth` 就**绝不会**突然弹一个浏览器。
/// 识别不了的取值返回 `None` ⇒ 该条目跳过（不静默按 Static 处理）。
fn parse_auth(item: &Value) -> Option<AuthMode> {
    match item.get("auth") {
        None | Some(Value::Null) => Some(AuthMode::Static),
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() || s.eq_ignore_ascii_case("static") {
                Some(AuthMode::Static)
            } else if s.eq_ignore_ascii_case("oauth") {
                Some(AuthMode::OAuth)
            } else {
                None
            }
        }
        Some(_) => None,
    }
}

/// 明文 `http://` 是否放行：**只给本机**。
fn url_allowed(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("https://") {
        return true;
    }
    let Some(rest) = lower.strip_prefix("http://") else {
        return false;
    };
    // 剥 userinfo 与端口（`user@host:1234/path?x`）。IPv6 是 `[::1]:3000` 这种带方括号的，
    // 不能按第一个 `:` 切 —— 那样会把它切成 `[`。
    let host = rest.split(['/', '?']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = if let Some(inner) = host.strip_prefix('[') {
        inner.split(']').next().unwrap_or("")
    } else {
        host.split(':').next().unwrap_or(host)
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

// ── 项目级 MCP 配置（`<工作目录>\.mcp.json`，2026-10-06，q3 第 2 步）────
//
// 主流 MCP 客户端（Claude Code / Cursor / VS Code…）用这一份文件声明**项目自带**的服务器，
// 形态是 `{ "mcpServers": { "<名字>": { … } } }` —— 与宿主注入的 `config\mcp.json`
// （`{ "servers": [ … ] }`，**数组**）不是一个格式，所以这里独立解析、再汇成同一对结构。
//
// 单条按「有没有 command」二分：
//   · 有 `command` ⇒ **stdio**（`command` + `args` + `env`）；
//   · 否则有 `url` ⇒ **Streamable HTTP**（`type` 缺省或 `"http"`，`headers` 照用）。
//     `type: "sse"`（已废弃的双端点传输）**跳过并留 warn** —— 我们没实现它，
//     硬按 http 处理只会连不上、还让人以为是服务器那边的问题。
//
// **它是数据不是指令**（同插件市场索引那条纪律）：坏条目只丢自己；整份读不出 / 语法坏
// 也只是「这个项目没有额外服务器」，绝不阻断启动。

/// `.mcp.json` 解析出的服务器，按传输分成两组（各自与既有结构对齐）。
#[derive(Debug, Default)]
pub struct ProjectServers {
    pub stdio: Vec<StdioServerConfig>,
    pub http: Vec<ServerConfig>,
}

/// 一台项目服务器在**信任卡**上要展示的内容（用户要能看清「它到底要跑什么」）。
#[derive(Debug, Clone)]
pub struct ProjectServerInfo {
    pub name: String,
    /// `"stdio"` / `"http"`
    pub transport: String,
    /// stdio = `command arg1 arg2 …`；http = `url`
    pub target: String,
    pub fingerprint: String,
}

impl ProjectServers {
    pub fn is_empty(&self) -> bool {
        self.stdio.is_empty() && self.http.is_empty()
    }

    /// 全部服务器（两组合并），供信任门判定与信任卡展示。
    pub fn servers(&self) -> Vec<ProjectServerInfo> {
        let mut out = Vec::new();
        for s in &self.stdio {
            let mut target = s.command.clone();
            for a in &s.args {
                target.push(' ');
                target.push_str(a);
            }
            out.push(ProjectServerInfo {
                name: s.name.clone(),
                transport: "stdio".into(),
                target,
                fingerprint: s.fingerprint(),
            });
        }
        for s in &self.http {
            out.push(ProjectServerInfo {
                name: s.name.clone(),
                transport: "http".into(),
                target: s.url.clone(),
                fingerprint: s.fingerprint(),
            });
        }
        out
    }
}

/// 读 `<cwd>\.mcp.json`。任何失败都只是「这个项目没有额外服务器」。
pub fn load_project_config(cwd: &Path) -> ProjectServers {
    let path = cwd.join(PROJECT_MCP_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        // 文件不在 = 这个项目没配（绝大多数项目都是这个状态）
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ProjectServers::default(),
        Err(e) => {
            eprintln!("[agent] 项目 MCP 配置读不出（{}）：{e}", path.display());
            return ProjectServers::default();
        }
    };
    let (servers, warnings) = parse_project_config(&text);
    for w in &warnings {
        eprintln!("[agent] 项目 MCP 配置：{w}");
    }
    servers
}

/// 解析 `.mcp.json` 文本，返回 `(合法条目, 逐条告警)`。
/// 抽成纯函数是为了能单测「stdio / http 二分」「sse 跳过」「坏条目只丢自己」这几条判据。
fn parse_project_config(text: &str) -> (ProjectServers, Vec<String>) {
    let mut out = ProjectServers::default();
    let mut warn = Vec::new();
    let root: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return (out, vec![format!("语法不是合法 JSON（{e}），整份忽略")]),
    };
    let Some(map) = root.get("mcpServers").and_then(Value::as_object) else {
        return (out, vec!["缺少 mcpServers 对象，整份忽略".into()]);
    };
    for (key, item) in map {
        let name = key.trim().to_string();
        if name.is_empty() {
            warn.push("有一条服务器的名字是空串，已跳过".into());
            continue;
        }
        let command = item
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let url = item
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        // `command` 优先：两种都写时按 stdio 处理（与主流客户端一致）。
        if !command.is_empty() {
            let args = item
                .get("args")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            let env = item
                .get("env")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            out.stdio.push(StdioServerConfig {
                name,
                command,
                args,
                env,
            });
            continue;
        }
        if url.is_empty() {
            warn.push(format!("{name}：既没有 command 也没有 url，已跳过"));
            continue;
        }
        // `type` 缺省 = http；认不出的取值跳过（不静默按 http 处理）。
        let ty = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if ty == "sse" {
            warn.push(format!(
                "{name}：type \"sse\" 是已废弃的双端点传输，本应用不支持，已跳过"
            ));
            continue;
        }
        if !ty.is_empty() && ty != "http" {
            warn.push(format!(
                "{name}：type 只认 \"http\"（或不写），收到 \"{ty}\"，已跳过"
            ));
            continue;
        }
        if !url_allowed(&url) {
            warn.push(format!(
                "{name}：只接受 https（明文 http 仅限本机 localhost / 127.0.0.1），已跳过"
            ));
            continue;
        }
        let Some(auth) = parse_auth(item) else {
            warn.push(format!("{name}：auth 只认 \"static\" / \"oauth\"，已跳过"));
            continue;
        };
        let headers = item
            .get("headers")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        out.http.push(ServerConfig {
            name,
            url,
            headers,
            auth,
        });
    }
    (out, warn)
}

/// 一台服务器的信任指纹（16 位十六进制）。
///
/// 覆盖**所有会改变行为的字段**：改了 `command` / `args` / `env`（或 `url` / `headers`）
/// 指纹就变 ⇒ 用户必须重新信任。这正是「信任门」要防的篡改：`.mcp.json` 跟着仓库走，
/// 别人改一行 `args` 就能让本机跑别的命令。
fn fingerprint_of(tag: &str, name: &str, parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(tag.as_bytes());
    h.update([0x1f]);
    h.update(name.as_bytes());
    for p in parts {
        h.update([0x1f]);
        h.update(p.as_bytes());
    }
    h.finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl StdioServerConfig {
    /// 信任指纹：覆盖 `command` / `args` / `env`（任一项改了即失效）。
    pub fn fingerprint(&self) -> String {
        let env = self
            .env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\u{1f}");
        fingerprint_of(
            "stdio",
            &self.name,
            &[&self.command, &self.args.join("\u{1f}"), &env],
        )
    }
}

impl ServerConfig {
    /// 信任指纹：覆盖 `url` / `headers`（任一项改了即失效）。
    pub fn fingerprint(&self) -> String {
        let headers = self
            .headers
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\u{1f}");
        fingerprint_of("http", &self.name, &[&self.url, &headers])
    }
}

// ── 项目 MCP 的信任记录（`config\mcp-trusted.json`，2026-10-06，q3 第 3 步）──
//
// 项目 MCP 要**在本机拉起任意命令** ⇒ **未经用户信任绝不连**。这份文件就是那份信任。
// 形态 `{ "<项目路径>": ["<指纹>", …] }`：**按项目分桶** —— 同名服务器在不同项目里
// 是两台不同的东西，分桶让「换项目不用重新审一遍」「删掉某项目的信任不影响别处」同时成立。
//
// 落盘纪律与 `mcp-tokens.json` 相同：**另起一个文件**，绝不去改写用户手写的 `mcp.json`；
// 路径从 `LUNAC_MCP_FILE` 推（不新增环境变量）。
//
// ⚠️ 它是**数据**：读不出 / 坏掉一律当「没有信任记录」（全不信任），**绝不反过来当已信任**。

/// 信任记录的文件名（与 `mcp.json` 同目录）。
const TRUST_FILE: &str = "mcp-trusted.json";

/// 项目 MCP 的信任记录。`path` 为 `None` = 拿不到可写路径（单跑烟测）——
/// 此时判据全 false、写操作如实报错，**绝不悄悄放行**。
pub struct TrustStore {
    path: Option<PathBuf>,
    map: HashMap<String, Vec<String>>,
}

impl TrustStore {
    /// 读信任记录。任何失败都只是「没有信任记录」。
    pub fn load() -> Self {
        let path = trust_path();
        let map = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<HashMap<String, Vec<String>>>(&t).ok())
            .unwrap_or_default();
        TrustStore { path, map }
    }

    /// 这个项目下的这台服务器（指纹）是否已被信任。
    pub fn is_trusted(&self, project: &str, fingerprint: &str) -> bool {
        self.map
            .get(project)
            .map_or(false, |v| v.iter().any(|x| x == fingerprint))
    }

    /// 记入并落盘。已存在的不重复追加。
    pub fn trust(&mut self, project: &str, fingerprints: &[String]) -> Result<(), String> {
        if fingerprints.is_empty() {
            return Ok(());
        }
        let entry = self.map.entry(project.to_string()).or_default();
        for fp in fingerprints {
            if !entry.iter().any(|x| x == fp) {
                entry.push(fp.clone());
            }
        }
        let path = self
            .path
            .as_ref()
            .ok_or("没有可写的信任记录路径（LUNAC_MCP_FILE 未给）")?;
        let text = serde_json::to_string_pretty(&self.map).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("写 {}: {e}", path.display()))
    }
}

/// 信任记录的路径（从 `LUNAC_MCP_FILE` 推父目录，与 `mcp-tokens.json` 同一手法）。
fn trust_path() -> Option<PathBuf> {
    let raw = std::env::var(MCP_FILE_ENV).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Some(Path::new(raw).parent()?.join(TRUST_FILE))
}

/// 为 `auth: oauth` 的服务器拿一份可用令牌：文件里有就按需刷新，没有就走一次浏览器授权。
///
/// 令牌文件的位置从 `LUNAC_MCP_FILE` 推（同目录的 `mcp-tokens.json`）⇒ 不新增环境变量。
/// `LUNAC_MCP_FILE` 没给（独立烟测）时 OAuth 不可用 —— 如实报错，不悄悄降级成匿名请求。
fn acquire_oauth(cfg: &ServerConfig) -> Result<OAuthState, String> {
    let mcp_file = std::env::var(MCP_FILE_ENV).ok();
    let path = mcp_oauth::store_path(mcp_file.as_deref())
        .ok_or("配了 auth: oauth，但没有配置文件路径，令牌无处可存")?;
    let key = cfg.name.clone();
    let tokens = match mcp_oauth::load(&path, &key) {
        Some(t) => mcp_oauth::ensure_fresh(&path, &key, t),
        None => {
            let t = mcp_oauth::authorize(&cfg.url, mcp_oauth::INTERACTIVE_TIMEOUT)?;
            if let Err(e) = mcp_oauth::store(&path, &key, &t) {
                // 存不下只是「下次启动要重新授权」，不因为这个把服务器整台丢掉
                eprintln!("[agent] MCP OAuth：{} 的令牌写不进 {path:?}（{e}）—— 本次会话可用", cfg.name);
            }
            t
        }
    };
    Ok(OAuthState { path, key, tokens })
}

// ── 单条连接 ───────────────────────────────────────────────────────

enum Kind {
    Stdio {
        child: Child,
        stdin: ChildStdin,
        rx: Receiver<Value>,
    },
    Http(HttpTransport),
}

struct HttpTransport {
    client: reqwest::blocking::Client,
    url: String,
    headers: Vec<(String, String)>,
    /// `initialize` 回包给的 `Mcp-Session-Id`，后续每个请求都要带回去。
    session: Option<String>,
    /// `auth: oauth` 时的令牌管理（`static` 时为 `None`，只用 `headers`）。
    oauth: Option<OAuthState>,
}

/// 一台远端服务器的 OAuth 令牌状态。
struct OAuthState {
    /// 令牌文件（`config\mcp-tokens.json`，与 `mcp.json` 同级）。
    path: PathBuf,
    /// 在令牌文件里的键 —— 用**服务器名**，与 `mcp.json` 里那条一一对应。
    key: String,
    tokens: mcp_oauth::Tokens,
}

impl OAuthState {
    /// 401 之后刷新令牌并落盘。**绝不开浏览器** —— 走到这里是在一次工具调用途中。
    fn refresh(&mut self) -> Result<(), String> {
        let fresh = mcp_oauth::refresh(&self.tokens)?;
        if let Err(e) = mcp_oauth::store(&self.path, &self.key, &fresh) {
            // 写盘失败不该让这一次调用黄掉，但这台服务器下次启动会再走一遍浏览器授权
            eprintln!("[agent] MCP OAuth：刷新后的令牌写不进去（{e}）—— 本次会话仍然可用");
        }
        self.tokens = fresh;
        Ok(())
    }
}

impl HttpTransport {
    /// 组装并发出一次 POST（静态 `headers` + 会话头 + OAuth 的 `Authorization`）。
    ///
    /// OAuth 模式下**忽略配置里静态的 `Authorization` 头**（同一个头出现两份会被服务器拒，
    /// 而以令牌为准才是用户配 `auth: oauth` 的本意）；其余自定义头照常带。
    fn send(
        &self,
        body: &Value,
        timeout: Option<Duration>,
    ) -> Result<reqwest::blocking::Response, String> {
        let payload = serde_json::to_string(body).map_err(|e| e.to_string())?;
        let mut req = self
            .client
            .post(&self.url)
            .header("content-type", "application/json")
            // 规范要求同时接受两种回包（服务器二选一）
            .header("accept", "application/json, text/event-stream")
            .body(payload);
        if let Some(t) = timeout {
            req = req.timeout(t);
        }
        for (k, v) in &self.headers {
            if self.oauth.is_some() && k.eq_ignore_ascii_case("authorization") {
                continue;
            }
            req = req.header(k.as_str(), v.as_str());
        }
        if let Some(s) = &self.session {
            req = req.header("mcp-session-id", s.as_str());
        }
        if let Some(oa) = &self.oauth {
            if !oa.tokens.access_token.is_empty() {
                req = req.header("authorization", format!("Bearer {}", oa.tokens.access_token));
            }
        }
        req.send().map_err(|e| format!("请求失败: {e}"))
    }
}

/// 处理一次服务端 `elicitation/create`（入参 = `{message, requestedSchema}`）。
/// 返回回包的 `result`：`{action:"accept", content:{…}}` / `{action:"decline"}` / `{action:"cancel"}`。
///
/// 由**宿主**（`main.rs`）提供 —— 弹卡与等用户输入要走审批通道，那是宿主层的职责，
/// 传输层只负责把结果塞回 JSON-RPC 回包（见 `handle_server_request`）。
pub type ElicitHandler = Box<dyn FnMut(&Value) -> Value>;

pub struct Bridge {
    kind: Kind,
    next_id: u64,
    /// 请求体里的工具名 → MCP 侧原名
    names: HashMap<String, String>,
    /// 已转成 Anthropic 工具 schema 的定义，直接并入请求体
    defs: Vec<Value>,
    /// 这条是不是**宿主那条 Lunac stdio 服务**（它另带 `lunac/*` 自定义方法）。
    /// 远端服务器为 `false` —— 那些方法只有 Lunac 自己的 server 实现。
    lunac: bool,
    /// 服务端 `roots/list` 要回的根（`file:///` URI）。**仅宿主 stdio 桥**填充；
    /// 远端 HTTP 服务器不声明 roots 能力，恒为空。
    roots: Vec<String>,
    /// `elicitation/create` 的处理钩子（宿主弹卡）。**仅宿主 stdio 桥**有。
    elicit: Option<ElicitHandler>,
}

impl Bridge {
    /// 连宿主那条 stdio 桥。`spec` 形如 `stdio:<lunac.exe 路径>`。
    /// `disallowed` 来自 `--disallowedTools`：命中的用户工具不接进来。
    ///
    /// `roots` / `elicit`（A13）只对**宿主这条桥**有意义：服务端据此把工作区根当 shell
    /// handler 的 cwd、并在工具声明了 `elicit` 时向用户弹卡收集输入。
    pub fn connect_stdio(
        spec: &str,
        disallowed: &[String],
        taken: &[String],
        roots: Vec<String>,
        elicit: Option<ElicitHandler>,
    ) -> Result<Self, String> {
        let exe = spec
            .strip_prefix("stdio:")
            .ok_or_else(|| format!("不支持的 MCP 传输: {spec}"))?
            .trim();
        if exe.is_empty() {
            return Err("MCP server 路径为空".into());
        }
        // 宿主桥的形态是写死的：把同一份 exe 以 `--mcp-server` 拉起。
        let mut cmd = Command::new(exe);
        cmd.arg("--mcp-server");
        Self::connect_stdio_cmd(cmd, exe, true, disallowed, taken, roots, elicit)
    }

    /// 连一台**普通** stdio MCP 服务器（`.mcp.json` 的 `mcpServers` 条目，q3 第 1 步）。
    ///
    /// 与 [`Self::connect_stdio`] 只差两点：进程怎么起（`command` + `args` + `env`，
    /// 而不是写死的 `--mcp-server`），以及 `lunac: false` —— 它不是 Lunac 自己的 server，
    /// 因此不声明 roots / elicitation 能力、也不发 `lunac/*` 自定义方法。
    pub fn connect_stdio_server(
        cfg: &StdioServerConfig,
        disallowed: &[String],
        taken: &[String],
    ) -> Result<Self, String> {
        let command = cfg.command.trim();
        if command.is_empty() {
            return Err(format!("{}: 缺少 command", cfg.name));
        }
        let mut cmd = Command::new(command);
        cmd.args(&cfg.args);
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        Self::connect_stdio_cmd(cmd, &cfg.name, false, disallowed, taken, Vec::new(), None)
    }

    /// stdio 连接的**内核**：拉起进程、接管道、起读线程、握手。
    ///
    /// `label` 只用于错误信息（宿主桥给 exe 路径，普通服务器给服务器名 —— 用户要能一眼
    /// 看出是哪一台没连上）。`lunac` 决定握手里声明的能力与我们发的自定义方法，见字段注释。
    fn connect_stdio_cmd(
        mut cmd: Command,
        label: &str,
        lunac: bool,
        disallowed: &[String],
        taken: &[String],
        roots: Vec<String>,
        elicit: Option<ElicitHandler>,
    ) -> Result<Self, String> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // MCP server 的日志直接并进本进程 stderr（上游会转发到前端/终端）；
            // stdout 必须独占给 JSON-RPC，不能混入任何别的输出。
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().map_err(|e| format!("spawn {label}: {e}"))?;
        let stdin = child.stdin.take().ok_or("MCP stdin 不可用")?;
        let stdout = child.stdout.take().ok_or("MCP stdout 不可用")?;

        // 读线程：stdout 每行一条 JSON-RPC 消息 → mpsc，主线程按 id 取用
        let (tx, rx) = mpsc::channel::<Value>();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(trimmed) {
                    Ok(v) => {
                        if tx.send(v).is_err() {
                            break;
                        }
                    }
                    Err(e) => eprintln!("[agent] MCP 非 JSON 输出（已忽略）: {e}"),
                }
            }
        });

        let mut bridge = Bridge {
            kind: Kind::Stdio { child, stdin, rx },
            next_id: 0,
            names: HashMap::new(),
            defs: Vec::new(),
            lunac,
            roots,
            elicit,
        };
        bridge.handshake(disallowed, taken)?;
        Ok(bridge)
    }

    /// 连一台远端 MCP 服务器（Streamable HTTP）。
    pub fn connect_http(
        cfg: &ServerConfig,
        disallowed: &[String],
        taken: &[String],
    ) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(CALL_TIMEOUT)
            .build()
            .map_err(|e| format!("建 HTTP client 失败: {e}"))?;
        // 交互式授权只在这里发生（连服务器那一步）—— 走到 handshake 时手上必然已有一份令牌
        let oauth = match cfg.auth {
            AuthMode::Static => None,
            AuthMode::OAuth => Some(acquire_oauth(cfg)?),
        };
        let mut bridge = Bridge {
            kind: Kind::Http(HttpTransport {
                client,
                url: cfg.url.clone(),
                headers: cfg.headers.clone(),
                session: None,
                oauth,
            }),
            next_id: 0,
            names: HashMap::new(),
            defs: Vec::new(),
            lunac: false,
            // 远端服务器不声明 roots / elicitation 能力（见 `handshake`），也就不会收到
            // 这两条反向请求 —— 这里留空即可。
            roots: Vec::new(),
            elicit: None,
        };
        bridge
            .handshake(disallowed, taken)
            .map_err(|e| format!("{}: {e}", cfg.name))?;
        Ok(bridge)
    }

    /// 两种传输共用的握手：initialize → notifications/initialized → tools/list。
    fn handshake(&mut self, disallowed: &[String], taken: &[String]) -> Result<(), String> {
        // 能力声明（A13）：只有宿主那条 stdio 桥支持**服务端反向请求**（`roots/list` /
        // `elicitation/create`）。远端 HTTP 服务器声明了也没人处理（一次 POST 一收一发，
        // 没有反向通道），干脆不声明 —— 免得它们发来一条我们答不上的请求。
        let capabilities = if self.lunac {
            json!({
                "roots": { "listChanged": false },
                "elicitation": {},
            })
        } else {
            json!({})
        };
        self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": capabilities,
                "clientInfo": { "name": "lunac-agent", "version": "0.2.0" },
            }),
            HANDSHAKE_TIMEOUT,
        )?;
        self.notify("notifications/initialized", json!({}))?;
        let listed = self.request("tools/list", json!({}), HANDSHAKE_TIMEOUT)?;
        self.adopt_tools(listed.get("tools"), disallowed, taken);
        Ok(())
    }

    pub fn defs(&self) -> &[Value] {
        &self.defs
    }

    /// 这条连接是不是宿主那条 Lunac stdio 服务（决定 `lunac/*` 自定义方法往哪发）。
    pub fn is_lunac(&self) -> bool {
        self.lunac
    }

    /// 本连接接进来的工具名里有没有 `name`（用于多服务器路由）。
    pub fn owns(&self, name: &str) -> bool {
        self.names.contains_key(name)
    }

    /// 调用一个 MCP 工具，返回给模型看的文本。
    /// 上游 `isError=true` 时转成 Err —— 调用方会包成 is_error 的 tool_result。
    pub fn call(&mut self, name: &str, input: &Value) -> Result<String, String> {
        let raw = self
            .names
            .get(name)
            .cloned()
            .ok_or_else(|| format!("未知 MCP 工具: {name}"))?;
        let res = self.request(
            "tools/call",
            json!({ "name": raw, "arguments": input }),
            CALL_TIMEOUT,
        )?;
        self.result_text(&res)
    }

    /// 取「往期会话索引」固定段（**自定义方法**，不列进 `tools/list`，见 mcp_server.rs）。
    ///
    /// 只在 agent 启动时调**一次**，结果拼进系统提示词：系统提示词在一次会话内必须
    /// 逐字节不变（ai-spec §11 规则 18），中途新存的会话只落盘、不改本进程的提示词。
    pub fn history_index(&mut self) -> Result<String, String> {
        let res = self.request("lunac/history_index", json!({}), HANDSHAKE_TIMEOUT)?;
        self.result_text(&res)
    }

    /// 检索往期会话正文 —— `SessionSearch` 工具的后端（同一个自定义方法通道）。
    pub fn history_search(&mut self, query: &str, limit: u32) -> Result<String, String> {
        let res = self.request(
            "lunac/history_search",
            json!({ "query": query, "limit": limit }),
            CALL_TIMEOUT,
        )?;
        self.result_text(&res)
    }

    /// 读长期记忆（A4）—— 启动时注入系统提示词、复盘 fork 开跑前读一次。
    ///
    /// 空记忆（全新安装）时服务端返回 `(long-term memory is empty)`，注入侧据此跳过。
    pub fn memory_read(&mut self) -> Result<String, String> {
        let res = self.request("lunac/memory_read", json!({}), HANDSHAKE_TIMEOUT)?;
        self.result_text(&res)
    }

    /// 写长期记忆（A4）—— `Remember` 工具的后端。
    ///
    /// `replace` = 整体替换（整理合并时用），默认按条目追加；服务端的去重与上限
    /// 报错都会以 `isError` 回来 ⇒ 这里变成 `Err`，模型看得见原因，可自行改小重写。
    pub fn memory_write(&mut self, content: &str, replace: bool) -> Result<String, String> {
        let res = self.request(
            "lunac/memory_write",
            json!({ "content": content, "replace": replace }),
            CALL_TIMEOUT,
        )?;
        self.result_text(&res)
    }

    /// 列出 MCP resources（A3）—— `ListMcpResourcesTool` 的后端。
    ///
    /// 规范回包是 `{resources:[{uri,name,mimeType}]}`，**与 `tools/call` 的 `content`
    /// 形状不同**，所以不能走 `result_text`。这里在客户端把它渲染成一张紧凑的表：
    /// 模型读表比读 JSON 省 token，且 uri 原样给出（`resources/read` 要拿它当参数）。
    pub fn list_resources(&mut self) -> Result<String, String> {
        let res = self.request("resources/list", json!({}), HANDSHAKE_TIMEOUT)?;
        let Some(list) = res.get("resources").and_then(Value::as_array) else {
            return Err("MCP resources/list 回包缺少 resources 数组".into());
        };
        if list.is_empty() {
            return Ok("No MCP resource is available: the user has no custom tool definition \
                       file (`tools\\*.json`) yet."
                .into());
        }
        let mut out = format!("MCP resources ({}):\n", list.len());
        for r in list {
            out.push_str(&format!(
                "- {}  {}  {}\n",
                r.get("name").and_then(Value::as_str).unwrap_or("(unnamed)"),
                r.get("uri").and_then(Value::as_str).unwrap_or("(no uri)"),
                r.get("mimeType").and_then(Value::as_str).unwrap_or(""),
            ));
        }
        out.push_str("\nPass one of these URIs to ReadMcpResourceTool to see the file.\n");
        Ok(out)
    }

    /// 读一个 MCP resource（A3）—— `ReadMcpResourceTool` 的后端。
    ///
    /// 回包形状是 `{contents:[{uri,mimeType,text|blob}]}`（同样不是 `content`）。
    /// `blob`（二进制）只报大小**不渲染 base64** —— 那种内容进上下文既没用又极贵。
    pub fn read_resource(&mut self, uri: &str) -> Result<String, String> {
        let res = self.request("resources/read", json!({ "uri": uri }), CALL_TIMEOUT)?;
        let Some(items) = res.get("contents").and_then(Value::as_array) else {
            return Err("MCP resources/read 回包缺少 contents 数组".into());
        };
        if items.is_empty() {
            return Err(format!("resource {uri} 没有任何内容"));
        }
        let mut out = String::new();
        for c in items {
            out.push_str(&format!(
                "Resource: {}\n",
                c.get("uri").and_then(Value::as_str).unwrap_or(uri)
            ));
            if let Some(m) = c.get("mimeType").and_then(Value::as_str) {
                if !m.is_empty() {
                    out.push_str(&format!("MIME: {m}\n"));
                }
            }
            out.push_str("---\n");
            if let Some(t) = c.get("text").and_then(Value::as_str) {
                out.push_str(t);
                out.push('\n');
            } else if let Some(b) = c.get("blob").and_then(Value::as_str) {
                out.push_str(&format!(
                    "(binary resource: {} base64 chars, not rendered)\n",
                    b.len()
                ));
            } else {
                out.push_str("(empty)\n");
            }
        }
        Ok(out)
    }

    /// 列出 MCP prompts（A13）—— `ListMcpPromptsTool` 的后端。
    ///
    /// 回包 `{prompts:[{name,description}]}`，形状与 resources 不同，同样渲染成紧凑表：
    /// `name` 原样给出（`prompts/get` 要拿它当参数）。
    pub fn list_prompts(&mut self) -> Result<String, String> {
        let res = self.request("prompts/list", json!({}), HANDSHAKE_TIMEOUT)?;
        let Some(list) = res.get("prompts").and_then(Value::as_array) else {
            return Err("MCP prompts/list 回包缺少 prompts 数组".into());
        };
        if list.is_empty() {
            return Ok("No MCP prompt is available: the user has no prompt template \
                       (`prompts\\*.md`) yet."
                .into());
        }
        let mut out = format!("MCP prompts ({}):\n", list.len());
        for p in list {
            out.push_str(&format!(
                "- {}  {}\n",
                p.get("name").and_then(Value::as_str).unwrap_or("(unnamed)"),
                p.get("description").and_then(Value::as_str).unwrap_or(""),
            ));
        }
        out.push_str("\nPass one of these names to GetMcpPromptTool to render it.\n");
        Ok(out)
    }

    /// 取一个 MCP prompt（A13）—— `GetMcpPromptTool` 的后端。
    ///
    /// 回包 `{description, messages:[{role,content:{type:"text",text:…}}]}`：服务端已经
    /// 把 `{{key}}` 模板按 `arguments` 填好，这里只要把各条文本拼给模型即可。
    pub fn get_prompt(&mut self, name: &str, arguments: &Value) -> Result<String, String> {
        let res = self.request(
            "prompts/get",
            json!({ "name": name, "arguments": arguments }),
            CALL_TIMEOUT,
        )?;
        let Some(msgs) = res.get("messages").and_then(Value::as_array) else {
            return Err("MCP prompts/get 回包缺少 messages 数组".into());
        };
        let mut out = String::new();
        if let Some(d) = res.get("description").and_then(Value::as_str) {
            if !d.is_empty() {
                out.push_str(&format!("Prompt: {d}\n---\n"));
            }
        }
        for m in msgs {
            let role = m.get("role").and_then(Value::as_str).unwrap_or("user");
            out.push_str(&format!("[{role}]\n"));
            out.push_str(&content_text(m.get("content")));
            out.push('\n');
        }
        Ok(out)
    }

    /// 解析 `{content:[{type:"text",…}], isError}` 形态的回包。
    /// `tools/call` 与上面两个自定义方法共用它 —— 两边的回包形状是刻意做成一样的。
    fn result_text(&self, res: &Value) -> Result<String, String> {
        let text = content_text(res.get("content"));
        if res.get("isError").and_then(Value::as_bool).unwrap_or(false) {
            Err(text)
        } else {
            Ok(text)
        }
    }

    /// 把 `tools/list` 的结果转成请求体的工具 schema。
    ///
    /// **按工具名排序后再入列**：`read_dir` 的返回顺序不保证稳定，而 tools
    /// 数组是请求前缀的一部分 —— 顺序一变，端点侧的前缀缓存整段失效。
    ///
    /// `taken` = **别的连接**已经占用的名字（多服务器时由 `McpSet` 逐台累积传进来）；
    /// 不传就会两台服务器各挑出同一个 `mcp__search`，合并后互相覆盖。
    fn adopt_tools(&mut self, tools: Option<&Value>, disallowed: &[String], taken: &[String]) {
        let Some(list) = tools.and_then(Value::as_array) else {
            return;
        };
        let mut accepted: Vec<(String, String, Value)> = Vec::new();
        // 已占用的名字（含本批已接受的）—— 名字在循环结束后才写回 self，
        // 所以这里得自己维护一份，否则重名工具会撞车；起步先带上别的连接占用的。
        let mut used: Vec<String> = taken.to_vec();
        used.extend(self.names.keys().cloned());
        for tool in list.iter().take(MAX_TOOLS_PER_SERVER) {
            let Some(raw) = tool.get("name").and_then(Value::as_str) else {
                continue;
            };
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let name = unique_name(&used, raw);
            // 黑名单按原名或带前缀名任一命中即不接入
            if disallowed.iter().any(|d| d == raw || d == &name) {
                eprintln!("[agent] MCP 工具 {name} 已被 --disallowedTools 禁用");
                continue;
            }
            used.push(name.clone());
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let schema = tool
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
            accepted.push((
                name.clone(),
                raw.to_string(),
                json!({ "name": name, "description": description, "input_schema": schema }),
            ));
        }
        if list.len() > MAX_TOOLS_PER_SERVER {
            eprintln!(
                "[agent] MCP 服务器给了 {} 个工具，只接入前 {MAX_TOOLS_PER_SERVER} 个",
                list.len()
            );
        }
        accepted.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, raw, def) in accepted {
            self.names.insert(name, raw);
            self.defs.push(def);
        }
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let body = json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        });
        // HTTP 与 stdio 的收发形态完全不同，先分流再借字段（避免同时可变借用 self.kind）。
        if matches!(self.kind, Kind::Http(_)) {
            return self.request_http(method, id, &body, timeout);
        }
        let Kind::Stdio { child, stdin, rx } = &mut self.kind else {
            unreachable!("上面已判过 Http")
        };
        write_line(stdin, &body)?;
        loop {
            match rx.recv_timeout(timeout) {
                Ok(msg) => {
                    // 服务端**反向请求**（A13）：带 `method`（且带 `id`）的不是我们的响应，
                    // 而是服务端主动发来的 —— `roots/list` / `elicitation/create`。
                    // 就地应答后继续等，**不能丢**（丢了 elicitation 会让工具卡死到超时）。
                    if let Some(m) = msg.get("method").and_then(Value::as_str) {
                        if let Some(rid) = msg.get("id").cloned() {
                            let reply = answer_server_request(
                                m,
                                rid,
                                msg.get("params"),
                                &self.roots,
                                &mut self.elicit,
                            );
                            write_line(stdin, &reply)?;
                        }
                        // 无 id 的是通知，按规范丢弃
                        continue;
                    }
                    // 只认自己的 id
                    if msg.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    return unwrap_result(method, &msg);
                }
                Err(RecvTimeoutError::Timeout) => {
                    let _ = child.kill();
                    return Err(format!("MCP {method} 超时（{}s）", timeout.as_secs()));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!("MCP 连接已关闭（{method}）"))
                }
            }
        }
    }

    /// Streamable HTTP 的一次请求：POST 出去，回包可能是 `application/json`（一条消息），
    /// 也可能是 `text/event-stream`（SSE 流，里有若干条消息，取 id 对上的那条）。
    fn request_http(
        &mut self,
        method: &str,
        id: u64,
        body: &Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let (msg, session) = self.http_post(body, Some(timeout))?;
        if let Some(s) = session {
            if let Kind::Http(h) = &mut self.kind {
                h.session = Some(s);
            }
        }
        let msg = msg.ok_or_else(|| {
            format!("MCP {method} 没有返回结果（服务器只回了 202 / 空 body）")
        })?;
        if msg.get("id").and_then(Value::as_u64) != Some(id) {
            // SSE 里可能只有服务端通知（没有我们那条响应）
            return Err(format!("MCP {method} 的回包里没有 id={id} 的响应"));
        }
        unwrap_result(method, &msg)
    }

    /// 一次 POST。返回 `(响应消息, 服务器新给的 session id)`。
    /// `timeout = None` 时用 client 自带的超时（通知那条路）。
    ///
    /// **401 只在 OAuth 模式下重试一次**：先刷新令牌（**不开浏览器**）再原样重发。
    /// 刷新不出来就如实报错 —— 用户重启 agent 时会重新走一遍浏览器授权。
    fn http_post(
        &mut self,
        body: &Value,
        timeout: Option<Duration>,
    ) -> Result<(Option<Value>, Option<String>), String> {
        let Kind::Http(h) = &mut self.kind else {
            return Err("不是 HTTP 传输".into());
        };
        let mut resp = h.send(body, timeout)?;
        if resp.status().as_u16() == 401 {
            if let Some(oa) = h.oauth.as_mut() {
                oa.refresh()?;
                resp = h.send(body, timeout)?;
            }
        }
        read_http_response(resp)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let body = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        if matches!(self.kind, Kind::Http(_)) {
            // 通知没有 id，服务器按规范回 202；拿不到 body 是**正常**的，不报错。
            self.http_post(&body, Some(HANDSHAKE_TIMEOUT))?;
            return Ok(());
        }
        let Kind::Stdio { stdin, .. } = &mut self.kind else {
            unreachable!()
        };
        write_line(stdin, &body)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // stdio 那条是子进程，退出时一并收掉；HTTP 没有进程要收。
        if let Kind::Stdio { child, .. } = &mut self.kind {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// 应答一条**服务端反向请求**（仅 stdio 桥上会出现），返回要写回的一整条 JSON-RPC 消息。
///
/// · `roots/list`        —— 回工作区根（`roots` 空则回空数组，规范允许）
/// · `elicitation/create`—— 交给宿主钩子弹卡；没有钩子就回 `cancel`（工具因此不执行）
/// · 其余方法            —— 回 `-32601 Method not found`
fn answer_server_request(
    method: &str,
    id: Value,
    params: Option<&Value>,
    roots: &[String],
    elicit: &mut Option<ElicitHandler>,
) -> Value {
    match method {
        "roots/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "roots": roots.iter().map(|u| json!({ "uri": u })).collect::<Vec<_>>()
            }
        }),
        "elicitation/create" => {
            let result = match elicit.as_mut() {
                Some(f) => f(params.unwrap_or(&Value::Null)),
                None => json!({ "action": "cancel" }),
            };
            json!({ "jsonrpc": "2.0", "id": id, "result": result })
        }
        other => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("Method not found: {other}") }
        }),
    }
}

fn write_line(stdin: &mut ChildStdin, msg: &Value) -> Result<(), String> {
    let mut line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .map_err(|e| format!("写 MCP stdin 失败: {e}"))?;
    stdin
        .flush()
        .map_err(|e| format!("flush MCP stdin 失败: {e}"))
}

/// 从一条 JSON-RPC 回包里取 `result`，`error` 转成可读的 Err。
fn unwrap_result(method: &str, msg: &Value) -> Result<Value, String> {
    if let Some(err) = msg.get("error") {
        let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
        let text = err.get("message").and_then(Value::as_str).unwrap_or("");
        return Err(format!("MCP {method} 报错 {code}: {text}"));
    }
    msg.get("result")
        .cloned()
        .ok_or_else(|| format!("MCP {method} 回包既无 result 也无 error"))
}

/// 读一次 HTTP 回包：`Mcp-Session-Id` 头 + 两种 body 形态（`application/json` 一条消息 /
/// `text/event-stream` 若干条消息）。抽成自由函数是为了让「重新发送」那条路能复用。
fn read_http_response(
    resp: reqwest::blocking::Response,
) -> Result<(Option<Value>, Option<String>), String> {
    let status = resp.status();
    let session = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = resp.text().map_err(|e| format!("读回包失败: {e}"))?;
    if !status.is_success() {
        // 截断响应体：别把一整页 HTML 灌进日志与错误文本
        let snip: String = text.chars().take(200).collect();
        return Err(format!("HTTP {} {}", status.as_u16(), snip.trim()));
    }
    if text.trim().is_empty() {
        return Ok((None, session)); // 通知型（202 Accepted）没有 body
    }
    if ctype.contains("text/event-stream") {
        return Ok((parse_sse(&text), session));
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(v) => Ok((Some(v), session)),
        Err(e) => Err(format!("回包不是合法 JSON（{e}）")),
    }
}

/// 解析 SSE 文本，返回**第一条带 `id` 的消息**（服务端通知没有 id，跳过）。
///
/// SSE 的事件以空行分隔，一个事件的若干条 `data:` 行要**按行拼**成一份负载
/// （规范如此；把每个 `data:` 行都当独立 JSON 会在多行负载上解析失败）。
fn parse_sse(text: &str) -> Option<Value> {
    let mut data: Vec<String> = Vec::new();
    let mut flush = |data: &mut Vec<String>| -> Option<Value> {
        if data.is_empty() {
            return None;
        }
        let payload = data.join("\n");
        data.clear();
        serde_json::from_str::<Value>(&payload).ok()
    };
    for line in text.lines() {
        if line.is_empty() {
            if let Some(v) = flush(&mut data) {
                if v.get("id").map_or(false, |i| !i.is_null()) {
                    return Some(v);
                }
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        }
        // event: / id: / retry: / `:` 注释行一律忽略
    }
    let last = flush(&mut data);
    last.filter(|v| v.get("id").map_or(false, |i| !i.is_null()))
}

// ── 多连接集合（宿主 stdio 桥 + N 台远端服务器）──────────────────────

/// 上层只认这一个东西：有没有工具、调哪个工具。
pub struct McpSet {
    bridges: Vec<Bridge>,
}

impl McpSet {
    /// 连宿主那条 stdio 桥（`spec` 为 `None` 则跳过）+ 配置里的每台远端服务器。
    /// **逐台尽力而为**：任何一台连不上都只记一行，其余照常。
    ///
    /// `roots` / `elicit`（A13）只交给**宿主那条 stdio 桥** —— 服务端要用它们准备 shell
    /// handler 的 cwd 与 elicitation 弹卡。远端 HTTP 服务器用不上（也不声明这两项能力）。
    pub fn connect(
        spec: Option<&str>,
        servers: &[ServerConfig],
        disallowed: &[String],
        roots: Vec<String>,
        elicit: Option<ElicitHandler>,
    ) -> McpSet {
        let mut bridges: Vec<Bridge> = Vec::new();
        let mut taken: Vec<String> = Vec::new();
        if let Some(spec) = spec {
            match Bridge::connect_stdio(spec, disallowed, &taken, roots, elicit) {
                Ok(b) => {
                    taken.extend(b.names.keys().cloned());
                    bridges.push(b);
                }
                Err(e) => eprintln!("[agent] MCP 桥未接通（继续用内置工具）: {e}"),
            }
        }
        for cfg in servers {
            match Bridge::connect_http(cfg, disallowed, &taken) {
                Ok(b) => {
                    taken.extend(b.names.keys().cloned());
                    eprintln!(
                        "[agent] MCP 远端服务器 {} 已接通（工具 {} 个）",
                        cfg.name,
                        b.defs().len()
                    );
                    bridges.push(b);
                }
                Err(e) => eprintln!("[agent] MCP 远端服务器 {e} 未接通（跳过）"),
            }
        }
        McpSet { bridges }
    }

    /// 空集合（启动时一条连接都没成时的形态；`add_project` 可以在它上面增量挂）。
    pub fn empty() -> Self {
        McpSet {
            bridges: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.bridges.is_empty()
    }

    /// 连上**项目** `.mcp.json` 的服务器，返回**新接进来的工具定义**（调用方 append 进工具池）。
    ///
    /// 与 [`Self::connect`] 分开的理由见 q3 第 3 步：项目服务器要先过**信任门**，而信任
    /// 询问要在 stdin 就绪之后才发得出去（启动时发没人回）⇒ 这一步推迟到**首次查询前**。
    /// 逐台尽力而为，单台失败只记一行、不影响其余（与 `connect` 同一条纪律）。
    pub fn add_project(&mut self, servers: &ProjectServers, disallowed: &[String]) -> Vec<Value> {
        // 跨服务器去重的「已占用工具名」从**现有连接**起算 —— 项目工具与宿主工具重名时，
        // 宿主那份在前、优先（与 `connect` 里 stdio 桥在前同理）。
        let mut taken: Vec<String> = self.bridges.iter().flat_map(|b| b.names.keys().cloned()).collect();
        let mut added: Vec<Value> = Vec::new();
        for cfg in &servers.stdio {
            match Bridge::connect_stdio_server(cfg, disallowed, &taken) {
                Ok(b) => {
                    taken.extend(b.names.keys().cloned());
                    eprintln!(
                        "[agent] 项目 MCP（stdio）{} 已接通（工具 {} 个）",
                        cfg.name,
                        b.defs().len()
                    );
                    added.extend(b.defs().iter().cloned());
                    self.bridges.push(b);
                }
                Err(e) => eprintln!("[agent] 项目 MCP（stdio）{e} 未接通（跳过）"),
            }
        }
        for cfg in &servers.http {
            match Bridge::connect_http(cfg, disallowed, &taken) {
                Ok(b) => {
                    taken.extend(b.names.keys().cloned());
                    eprintln!(
                        "[agent] 项目 MCP（http）{} 已接通（工具 {} 个）",
                        cfg.name,
                        b.defs().len()
                    );
                    added.extend(b.defs().iter().cloned());
                    self.bridges.push(b);
                }
                Err(e) => eprintln!("[agent] 项目 MCP（http）{e} 未接通（跳过）"),
            }
        }
        added
    }

    /// 有没有连上**宿主那条 stdio 桥**。`lunac/*` 自定义方法与 `tools\*.json` 的用户工具
    /// 都只在它那边 —— 「`Remember` 该不该注册 / 索引该不该注入」必须看这个，而不是
    /// 「集合空不空」（只配了远端服务器时集合非空，但那些方法必然失败）。
    pub fn has_lunac(&self) -> bool {
        self.bridges.iter().any(|b| b.is_lunac())
    }

    /// 全部连接的工具定义（按连接顺序拼接，**连接顺序是稳定的**：stdio 在前，
    /// 远端按配置里的先后）。顺序即前缀 —— 别在渲染时再排序，那会与已发出的缓存对不上。
    pub fn defs(&self) -> Vec<Value> {
        self.bridges.iter().flat_map(|b| b.defs().iter().cloned()).collect()
    }

    /// 调用一个工具：按名字路由到拥有它的那条连接。
    pub fn call(&mut self, name: &str, input: &Value) -> Result<String, String> {
        let Some(b) = self.bridges.iter_mut().find(|b| b.owns(name)) else {
            return Err(format!("未知 MCP 工具: {name}"));
        };
        b.call(name, input)
    }

    /// `lunac/*` 自定义方法只在宿主那条 stdio 桥上 —— 远端服务器没有它们。
    fn lunac(&mut self) -> Result<&mut Bridge, String> {
        self.bridges
            .iter_mut()
            .find(|b| b.is_lunac())
            .ok_or_else(|| "MCP bridge is not connected".to_string())
    }

    pub fn history_index(&mut self) -> Result<String, String> {
        self.lunac()?.history_index()
    }

    pub fn history_search(&mut self, query: &str, limit: u32) -> Result<String, String> {
        self.lunac()?.history_search(query, limit)
    }

    pub fn memory_read(&mut self) -> Result<String, String> {
        self.lunac()?.memory_read()
    }

    pub fn memory_write(&mut self, content: &str, replace: bool) -> Result<String, String> {
        self.lunac()?.memory_write(content, replace)
    }

    /// resources 读侧走**宿主那条桥**：用户工具定义（`tools\*.json`）在那边，
    /// 远端服务器的 resources 今天不列给模型（`ReadMcpResourceTool` 是围绕
    /// 「用户自己的工具清单」设计的）。
    pub fn list_resources(&mut self) -> Result<String, String> {
        self.lunac()?.list_resources()
    }

    pub fn read_resource(&mut self, uri: &str) -> Result<String, String> {
        self.lunac()?.read_resource(uri)
    }

    /// prompts 读侧同样走**宿主那条桥**（模板在 `<exe 根>\prompts\*.md`，只有它那边有）。
    pub fn list_prompts(&mut self) -> Result<String, String> {
        self.lunac()?.list_prompts()
    }

    pub fn get_prompt(&mut self, name: &str, arguments: &Value) -> Result<String, String> {
        self.lunac()?.get_prompt(name, arguments)
    }
}

/// `mcp__<清洗后的原名>`，重名追加 `_2`。
/// 前缀必须稳定（前端「始终允许」白名单按完整名字记），清洗规则改动等于废掉用户白名单。
fn unique_name(taken: &[String], raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_RAW_NAME)
        .collect();
    let mut name = format!("{PREFIX}{cleaned}");
    let mut n = 2;
    while taken.iter().any(|t| t == &name) {
        name = format!("{PREFIX}{cleaned}_{n}");
        n += 1;
    }
    if cleaned != raw {
        eprintln!("[agent] MCP 工具名 {raw} → {name}（含非法字符或超长）");
    }
    name
}

/// MCP 的 `content` 是 block 数组（`[{type:"text",text:"…"}]`），
/// 非文本块（image 等）退化成 JSON 文本，至少让模型看得见有东西。
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|b| {
                b.get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| b.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 明文 http 只放行本机 —— 远端 http 会把 `headers` 里的凭据明文发出去。
    #[test]
    fn plain_http_is_local_only() {
        assert!(url_allowed("https://mcp.example.com/mcp"));
        assert!(url_allowed("HTTP://LOCALHOST:8080/mcp"));
        assert!(url_allowed("http://127.0.0.1:3000"));
        assert!(url_allowed("http://[::1]:3000/mcp"));
        assert!(!url_allowed("http://mcp.example.com/mcp"), "远端明文 http 必须拒");
        assert!(!url_allowed("ftp://example.com"));
        assert!(!url_allowed("file:///etc/passwd"));
        assert!(!url_allowed("mcp.example.com"), "没有 scheme 的一律拒");
    }

    /// 坏条目只丢自己：缺字段 / 明文远端 http 跳过，合法条目照常返回。
    #[test]
    fn bad_entries_drop_only_themselves() {
        let text = r#"{
          "servers": [
            { "name": "ok", "url": "https://a.example/mcp", "headers": { "Authorization": "Bearer x" } },
            { "name": "", "url": "https://b.example/mcp" },
            { "name": "insecure", "url": "http://b.example/mcp" },
            { "url": "https://c.example/mcp" }
          ]
        }"#;
        let (servers, warn) = parse_config(text);
        assert_eq!(servers.len(), 1, "只有第一条合法");
        assert_eq!(servers[0].name, "ok");
        assert_eq!(servers[0].headers.len(), 1);
        assert_eq!(warn.len(), 3, "三条坏条目各留一条告警");
    }

    /// `auth` 缺省 = Static（**不主动写 `oauth` 就绝不会弹浏览器**）；识别不了的值跳过该条目
    /// 并留告警 —— 写错却静默按 Static 处理，用户会以为 OAuth 生效了。
    #[test]
    fn auth_defaults_to_static_and_bad_values_drop_the_entry() {
        let text = r#"{
          "servers": [
            { "name": "absent", "url": "https://a.example/mcp" },
            { "name": "null", "url": "https://b.example/mcp", "auth": null },
            { "name": "oauth", "url": "https://c.example/mcp", "auth": "OAuth" },
            { "name": "typo", "url": "https://d.example/mcp", "auth": "outh" },
            { "name": "wrong-type", "url": "https://e.example/mcp", "auth": 1 }
          ]
        }"#;
        let (servers, warn) = parse_config(text);
        assert_eq!(servers.len(), 3, "缺省 / null / oauth 三条合法");
        assert_eq!(servers[0].auth, AuthMode::Static);
        assert_eq!(servers[1].auth, AuthMode::Static);
        assert_eq!(servers[2].auth, AuthMode::OAuth);
        assert_eq!(warn.len(), 2, "两条非法取值各留一条告警");
    }

    /// 整份配置坏掉时返回空表 + 一条告警，绝不 panic（配置是用户手写的）。
    #[test]
    fn broken_config_yields_no_servers() {
        assert!(parse_config("{ not json").0.is_empty());
        assert!(parse_config("[]").0.is_empty());
        assert_eq!(parse_config("{}").1.len(), 1, "缺少 servers 要如实说");
    }

    /// SSE 回包：取 id 对上的那条；多行 `data:` 要拼成一份负载；通知（无 id）跳过。
    #[test]
    fn sse_response_takes_the_message_with_an_id() {
        let stream = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/x\"}\n\n\
                      event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\n\
                      data: \"result\":{\"ok\":true}}\n\n";
        let v = parse_sse(stream).expect("要取到带 id 的那条");
        assert_eq!(v.get("id").and_then(Value::as_u64), Some(7));
        assert_eq!(v["result"]["ok"].as_bool(), Some(true));
        // 只有通知 ⇒ 没有可取的消息
        assert!(parse_sse("data: {\"jsonrpc\":\"2.0\",\"method\":\"x\"}\n\n").is_none());
    }

    /// 多连接路由：按名字找到拥有它的那条连接；找不到就如实报错。
    #[test]
    fn set_routes_calls_by_tool_name() {
        // 不真连网：直接造一个 set，用私有字段塞一个假连接
        let mut set = McpSet { bridges: Vec::new() };
        // 没有连接时，任何调用都报「未知工具」而不是 panic
        assert!(set.call("mcp__nope", &json!({})).is_err());
        assert!(set.defs().is_empty());
        assert!(set.is_empty());
        assert!(set.lunac().is_err(), "没有宿主桥时没有 lunac 方法可用");
        set.bridges.clear();
    }

    /// `.mcp.json` 的 `mcpServers` 按「有没有 command」二分：有 command = stdio，
    /// 否则有 url = http；`args` / `env` / `headers` 原样落进对应结构。
    #[test]
    fn project_config_splits_stdio_and_http() {
        let text = r#"{
          "mcpServers": {
            "fs": {
              "command": "npx",
              "args": ["-y", "@modelcontextprotocol/server-filesystem"],
              "env": { "FOO": "bar" }
            },
            "remote": {
              "type": "http",
              "url": "https://mcp.example.com/mcp",
              "headers": { "Authorization": "Bearer x" }
            }
          }
        }"#;
        let (p, warn) = parse_project_config(text);
        assert!(warn.is_empty(), "两条都合法，不该有告警：{warn:?}");
        assert_eq!(p.stdio.len(), 1);
        assert_eq!(p.http.len(), 1);
        let s = &p.stdio[0];
        assert_eq!(s.name, "fs");
        assert_eq!(s.command, "npx");
        assert_eq!(s.args.len(), 2);
        assert_eq!(s.env, vec![("FOO".to_string(), "bar".to_string())]);
        assert_eq!(p.http[0].name, "remote");
        assert_eq!(p.http[0].auth, AuthMode::Static, "没写 auth 默认 static");
        assert_eq!(p.http[0].headers.len(), 1);
    }

    /// 缺省 `type` = http；`type: "sse"`（已废弃的双端点传输）与认不出的取值**跳过并留告警**
    /// —— 不静默按 http 处理，否则连不上还让人以为是服务器那边的问题。
    #[test]
    fn project_config_rejects_sse_and_unknown_types() {
        let text = r#"{
          "mcpServers": {
            "default-http": { "url": "https://a.example/mcp" },
            "old-sse": { "type": "sse", "url": "https://b.example/mcp" },
            "typo": { "type": "htp", "url": "https://c.example/mcp" }
          }
        }"#;
        let (p, warn) = parse_project_config(text);
        assert_eq!(p.http.len(), 1, "只有缺省 type 那条合法");
        assert_eq!(p.http[0].name, "default-http");
        assert_eq!(warn.len(), 2, "sse 与拼错各留一条告警");
    }

    /// 坏条目只丢自己；整份坏掉 / 缺 `mcpServers` 也只是「没有项目服务器」，绝不 panic。
    #[test]
    fn project_config_bad_entries_drop_only_themselves() {
        let text = r#"{
          "mcpServers": {
            "empty": {},
            "insecure": { "url": "http://remote.example/mcp" },
            "ok": { "command": "node", "args": ["server.js"] }
          }
        }"#;
        let (p, warn) = parse_project_config(text);
        assert_eq!(p.stdio.len(), 1, "只有 ok 合法");
        assert_eq!(p.stdio[0].name, "ok");
        assert_eq!(warn.len(), 2, "空条目与明文远端各一条告警");
        assert!(parse_project_config("{ not json").0.stdio.is_empty());
        assert_eq!(parse_project_config("{}").1.len(), 1, "缺 mcpServers 要如实说");
    }

    /// 配置改动监视：只看「文件有没有被动过」，且**只报一次**。
    /// 用「文件从不存在 → 存在」当变化源（`None → Some`），不依赖 mtime 的时钟精度，
    /// 避免在快速写盘时出现偶发失败。
    #[test]
    fn config_watch_reports_once_and_covers_new_files() {
        let dir = std::env::temp_dir().join(format!("lunac-config-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = dir.join(PROJECT_MCP_FILE);
        let _ = std::fs::remove_file(&project);

        // 传 `None` 当远端路径：隔离 `LUNAC_MCP_FILE` 环境变量，测试只盯 `.mcp.json`。
        let mut w = ConfigWatch::snapshot_with(None, &dir);
        assert!(w.changed().is_empty(), "没动过就不该报");

        std::fs::write(&project, r#"{"mcpServers":{}}"#).unwrap();
        assert_eq!(
            w.changed(),
            vec![PROJECT_MCP_FILE.to_string()],
            "新建文件也算改动"
        );
        assert!(w.changed().is_empty(), "只报一次，不能每回合都报");

        let _ = std::fs::remove_file(&project);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
