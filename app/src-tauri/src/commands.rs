// src-tauri/src/commands.rs
// Tauri IPC commands — agent subprocess management and message relay.
// Note: Simple AI chat is handled directly by chat.rs (no proxy needed).
// agent.exe（自研 core-agent，见 docs/ai-spec.md §3.5）是 Agent 后端，
// 可选、按需经 start_cli 拉起。

use crate::AppState;
use crate::proxy_server;
use crate::cli_bridge;
use crate::windows_ocr;
use crate::paddle_ocr;
use serde::{Deserialize, Serialize};
use std::env;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::thread;
use tauri::{AppHandle, Emitter, State};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StatusPayload {
    pub state: String,
    pub message: String,
    /// Agent process instance id. Every agent.exe spawn gets a fresh id; the
    /// frontend ignores "closed" events whose instance doesn't match the
    /// currently-known one — a stale close from a stopped CLI can no longer
    /// tear down a newly started session.
    #[serde(default)]
    pub instance: u32,
}

/// Returns a fresh, monotonically increasing CLI instance id.
pub fn next_cli_instance() -> u32 {
    static CLI_INSTANCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    CLI_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

#[derive(Debug, Serialize, Clone)]
pub struct CliOutput {
    pub line: String,
}

// ── Path helpers ──────────────────────────────────────────────────

/// 自研 agent 后端二进制名（core-agent 的 cargo 产物）。
const AGENT_EXE: &str = "agent.exe";

/// Returns the directory containing the compiled agent binary (`agent.exe`).
/// Priority:
///   0. **dev 布局**（`…/src-tauri/target/{debug,release}`）先看 `<repo>/core-agent/target/{release,debug}`
///   1. exe_dir/resources/  — Tauri 打包资源解压目录
///   2. exe_dir/            — 便携版 / NSIS 安装根（agent.exe 与 lunac.exe 同目录）
///   3. dev: <repo>/core-agent/target/{release,debug}  — cargo 产物
///   4. dev: <repo>/core    — 兜底（历史 agent.exe 所在目录）
fn core_dir() -> std::path::PathBuf {
    let exe_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    // ── dev 布局必须优先（2026-09-17 修，不得回退）──────────────────────────
    // `src-tauri/target/{debug,release}/agent.exe` 是 **Tauri 构建时**按 tauri.conf.json 的
    // `bundle.resources`（`../../core-agent/target/release/agent.exe` → `agent.exe`，map 形式）
    // 从 core-agent **平铺过来的快照**，只在**构建 lunac 时**才刷新；它命中下面第 2 条
    // 「agent.exe 与 lunac.exe 同级」，于是会**遮蔽** core-agent 的更新构建。
    // 这条路径会静默失效：改完 core-agent → `cargo build --release` → **只重启 lunac.exe**
    // （没有重新构建 lunac ⇒ 资源不会重新平铺）⇒ 跑起来的还是旧 agent.exe，且**毫无提示**。
    // 实测代价：`set_history` 协议 09-17 就进了 core-agent 源码，dev 实际跑的却是 09-15 的
    // agent.exe（日志里只剩 agent 侧一句「忽略输入类型: Some("set_history")」），用户侧表现为
    // 「恢复历史后追问，AI 完全不记得上文」—— 排查时一度怀疑数据库与前端回灌。
    // 用两级目录名（`target` + `src-tauri`）判定「是否仓库内的构建目录」，避免误伤便携版；
    // 那里没有 cargo 产物时照旧往下走，行为与从前一致。
    let is_dev_layout = exe_dir
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        == Some("target")
        && exe_dir
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            == Some("src-tauri");
    if is_dev_layout {
        let repo_root = exe_dir.join("..").join("..").join("..").join("..");
        for rel in ["core-agent/target/release", "core-agent/target/debug"] {
            let dir = repo_root.join(rel);
            if dir.join(AGENT_EXE).exists() {
                return dir.canonicalize().unwrap_or(dir);
            }
        }
    }

    // Release: Tauri's bundle extraction directory
    let resources_dir = exe_dir.join("resources");
    if resources_dir.join(AGENT_EXE).exists() {
        return resources_dir;
    }

    // Release: 便携版 / NSIS 安装根（agent.exe 与 lunac.exe 同级）
    if exe_dir.join(AGENT_EXE).exists() {
        return exe_dir;
    }

    // Dev: navigate up from src-tauri/target/{debug,release}/ to the repo root
    let repo_root = exe_dir.join("..").join("..").join("..").join("..");
    for rel in ["core-agent/target/release", "core-agent/target/debug"] {
        let dir = repo_root.join(rel);
        if dir.join(AGENT_EXE).exists() {
            return dir.canonicalize().unwrap_or(dir);
        }
    }

    repo_root
        .join("core")
        .canonicalize()
        .unwrap_or_else(|_| std::env::current_dir().unwrap().join("..").join("core"))
}

// ── Subprocess management ─────────────────────────────────────────

// ── Windows Job Object：子进程严格绑定 lunac.exe 生命周期 ────────
// spawn 的子进程（agent.exe / llama-server.exe）由 OS 记录父子关系，
// 任务管理器"进程"页展开 Lunac 分组即可看到（CREATE_NO_WINDOW 只是
// 不弹控制台，进程本身可见）。但父进程崩溃时子进程会变孤儿；
// Job Object + KILL_ON_JOB_CLOSE 保证 lunac.exe 以任何方式退出
// （含崩溃/taskkill）时，Windows 内核自动终止 job 内全部子进程。
#[cfg(target_os = "windows")]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attrs: *mut std::ffi::c_void, name: *const u16) -> isize;
        fn SetInformationJobObject(
            job: isize,
            class: u32,
            info: *const std::ffi::c_void,
            len: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: isize, process: isize) -> i32;
    }

    static JOB: OnceLock<isize> = OnceLock::new();

    fn handle() -> isize {
        *JOB.get_or_init(|| unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job != 0 {
                let mut info = ExtendedLimits::default();
                info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    job,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    &info as *const _ as *const std::ffi::c_void,
                    std::mem::size_of::<ExtendedLimits>() as u32,
                );
            }
            job // handle 永不关闭 — 进程退出时由内核关闭并触发 KILL
        })
    }

    /// 将子进程加入 job。失败不致命（极老系统不支持嵌套 job），仅记录日志。
    pub fn assign(child: &std::process::Child) {
        let job = handle();
        if job == 0 {
            return;
        }
        unsafe {
            if AssignProcessToJobObject(job, child.as_raw_handle() as isize) == 0 {
                eprintln!("[job] AssignProcessToJobObject failed (pid {})", child.id());
            }
        }
    }
}

// ── Agent HTTP bridge (VSCode extension) ─────────────────────────
// Same logic as start_cli but without Tauri AppHandle/State dependencies.
// Called by agent_server.rs via POST /agent/start.
// The desktop app can also drive the agent via Tauri IPC on the same agent.exe process.

pub fn start_agent_http() -> Result<String, String> {
    if cli_bridge::is_running() {
        return Ok("already running".into());
    }

    let (api_url, api_key, model) = ai_credentials()?;

    proxy_server::stop();
    configure_agent_env(&api_url, &api_key, &model);

    let dir = core_dir();
    let agent_exe = dir.join(AGENT_EXE);
    if !agent_exe.exists() {
        return Err(format!("agent.exe not found at {}", agent_exe.display()));
    }

    // Ripgrep vendor path
    let rg_dir = dir.join("utils").join("vendor").join("ripgrep").join("x64-win32");
    if rg_dir.join("rg.exe").exists() {
        let rg_str = rg_dir.display().to_string();
        let old_path = env::var("PATH").unwrap_or_default();
        if !old_path.contains(&rg_str) {
            env::set_var("PATH", format!("{};{}", rg_str, old_path));
        }
        env::set_var("USE_BUILTIN_RIPGREP", "0");
    }

    let agent_path = agent_exe.to_string_lossy().to_string();
    let mut args: Vec<String> = vec![
        "--print".into(),
        "--verbose".into(),
        "--input-format".into(), "stream-json".into(),
        "--output-format".into(), "stream-json".into(),
        "--include-partial-messages".into(),
        "--permission-prompt-tool".into(), "stdio".into(),
    ];

    // MCP bridge
    {
        let lunac_exe = std::env::current_exe()
            .unwrap_or_else(|_| std::path::PathBuf::from("lunac.exe"));
        args.push("--mcp-server".into());
        args.push(format!("stdio:{}", lunac_exe.display()));
    }
    cli_args_for_profile("project", &dir, &mut args);
    args.push(".".into());

    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let mut child = spawn_child(&agent_path, &args_refs, &dir, &[])?;

    let stdin = child.stdin.take();
    if let Some(stdin) = stdin {
        cli_bridge::set_stdin(stdin);
    }

    // stdout reader → broadcast to SSE subscribers
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                if let Ok(line) = line {
                    let trimmed = line.trim().to_string();
                    if !trimmed.is_empty() && trimmed.starts_with('{') {
                        cli_bridge::broadcast_output(trimmed);
                    }
                }
            }
        });
    }

    // stderr → dev terminal only (no Tauri AppHandle available)
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(line) = line {
                    if !line.trim().is_empty() {
                        crate::log::warn(format!("[agent stderr] {line}"));
                        eprintln!("[cli-http] {}", line);
                    }
                }
            }
        });
    }

    cli_bridge::set_process(child);
    Ok("Agent started".into())
}

fn spawn_child(
    program: &str,
    args: &[&str],
    cwd: &std::path::Path,
    envs: &[(&str, String)],
) -> Result<Child, String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x0800_0000); // CREATE_NO_WINDOW — no console popup
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let child = cmd.spawn().map_err(|e| format!("Failed to spawn {}: {}", program, e))?;
        job::assign(&child); // 绑定生命周期：lunac 退出 → 子进程必死
        Ok(child)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.spawn()
            .map_err(|e| format!("Failed to spawn {}: {}", program, e))
    }
}

// ── CLI process ───────────────────────────────────────────────────

/// 手动禁用的工具名 → `--disallowedTools` 参数（第 19 点 — 前缀缓存优化：
/// tools 数组更短，前缀里不再有永远用不上的工具定义）。
///
/// 这里**不再内置任何默认黑名单**：旧 CLI 时代那批名字（`ToolSearch` /
/// `LSP` / `Brief` …）对自研 agent.exe 全是空转项，而 agent.exe 侧的过滤是
/// 按名字精确比较的 —— 万一用户的自定义 MCP 工具正好叫这些名字，会被无声
/// 禁用。名单完全交前台（`set_tool_blacklist`）决定，UI 候选见 main.ts
/// 的 TOOL_BLACKLIST_CANDIDATES。
fn tool_blacklist_args(custom: &[String]) -> Vec<String> {
    let mut list: Vec<String> = Vec::new();
    for name in custom {
        let name = name.trim();
        if !name.is_empty() && !list.iter().any(|b| b == name) {
            list.push(name.to_string());
        }
    }
    if list.is_empty() {
        return Vec::new();
    }
    let mut args = vec!["--disallowedTools".to_string()];
    args.extend(list);
    args
}


/// Security profiles map to CLI permission modes + directory scoping.
/// - "safe":  read-only, no writes anywhere (plan mode)
/// - "project": auto-approve edits in project dir (acceptEdits + --add-dir)
/// - "full":   bypass all permission checks (dangerous, admin only)
fn cli_args_for_profile<'a>(
    profile: &str,
    cwd: &'a std::path::Path,
    args: &'a mut Vec<String>,
) {
    match profile {
        "safe" => {
            args.push("--permission-mode".into());
            args.push("plan".into());
        }
        "full" => {
            args.push("--dangerously-skip-permissions".into());
            // Still scope to project dir as safety net
            args.push("--add-dir".into());
            args.push(cwd.display().to_string());
        }
        _ => { // "project" (default)
            args.push("--permission-mode".into());
            args.push("acceptEdits".into());
            args.push("--add-dir".into());
            args.push(cwd.display().to_string());
        }
    }
}

fn start_cli_process(
    state: &AppState,
    app: AppHandle,
    core_dir: &std::path::Path,
) -> Result<(), String> {
    if cli_bridge::is_running() {
        return Ok(()); // already running
    }

    let profile = state.security_profile
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| "project".into());

    // AI workspace: the directory the agent may operate in (cwd + --add-dir).
    // 未配置工作区时用 `default_work_dir()` = `<exe 根>\temp\transStorage`（2026-09-17 改）：
    // agent 的相对路径与模型写的临时脚本都落在应用自己的数据根下，dev / release 天然隔离。
    // **不再回退用户主目录** —— 回退会让 `_tmp_*.py` 这类草稿直接堆在 `C:\Users\<用户名>` 根下。
    // 全系统访问权不受影响：workspace 为空时**仍然不设** `LUNAC_WORKSPACE_LOCKED`，
    // 读写工作目录之外的文件照旧走 ask/审批卡，而不是被拒。
    // When a workspace IS configured, LUNAC_WORKSPACE_LOCKED=1 is passed to
    // the CLI so file reads/writes OUTSIDE the workspace are denied outright
    // (instead of the default "ask" for the whole-system mode).
    let (workdir, workspace_locked) = {
        let ws = state.workspace.lock().map(|g| g.clone()).unwrap_or_default();
        let ws = ws.trim().to_string();
        if ws.is_empty() {
            (default_work_dir(), false)
        } else {
            (std::path::PathBuf::from(ws), true)
        }
    };

    let agent_exe = core_dir.join(AGENT_EXE);
    if !agent_exe.exists() {
        return Err(format!("agent.exe not found at {}", agent_exe.display()));
    }
    let agent_path = agent_exe.to_string_lossy().to_string();

    // Args for the compiled standalone agent (no "bun run" prefix needed)
    let mut args: Vec<String> = vec![
        "--print".into(),
        "--verbose".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        // Emit stream_event lines (content_block_delta etc.) for live
        // typing in the frontend; without it only whole messages arrive.
        "--include-partial-messages".into(),
        // Route permission "ask" decisions to the frontend via the
        // can_use_tool control_request protocol (approval cards in UI).
        // Without this the agent decides everything silently by itself.
        "--permission-prompt-tool".into(),
        "stdio".into(),
    ];

    // ── MCP bridge: connect agent.exe to lunac.exe's built-in MCP server.
    // agent.exe parses --mcp-server stdio:<path> and spawns
    // lunac.exe --mcp-server as a child process.
    // The MCP server reads user-defined tools from <exe 根>\tools\
    {
        let lunac_exe = std::env::current_exe()
            .unwrap_or_else(|_| std::path::PathBuf::from("lunac.exe"));
        args.push("--mcp-server".into());
        args.push(format!("stdio:{}", lunac_exe.display()));
    }
    cli_args_for_profile(&profile, &workdir, &mut args);
    args.push(".".into());

    // ── 工具黑名单（第 19 点）：默认保守 + 用户自定义，剔除不需要的
    // 工具以缩减请求体尾部 tools schema（缓存命中率优化）。
    // 注意：必须放在 "."（路径参数）之后 —— commander 的 <tools...>
    // variadic 会贪婪消费到下一个 --flag，若放在前面会把 "." 吞成工具名。
    let blacklist = {
        let custom = state
            .tool_blacklist
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        tool_blacklist_args(&custom)
    };
    args.extend(blacklist);

    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    // Workspace lock: when a workspace is configured, the CLI must deny
    // access outside it (filesystem.ts reads this env var).
    let mut envs: Vec<(&str, String)> = Vec::new();
    if workspace_locked {
        envs.push(("LUNAC_WORKSPACE_LOCKED", "1".into()));
    }
    // 已安装技能固定目录 → agent.exe（core-agent 经 LUNAC_SKILLS_DIR 扫描
    // <dir>/<技能名>/SKILL.md），与 lunac 设置「技能扩展」管理的目录一致。
    envs.push(("LUNAC_SKILLS_DIR", lunac_skills_dir().to_string_lossy().to_string()));
    // 日志目录 → agent.exe：与宿主写同一份目录（<exe 根>\temp\logs），排障只需
    // 看一个地方。agent 没拿到该变量时会自行回退到 <agent.exe 目录>\temp\logs
    // （见 core-agent/src/log.rs）。
    envs.push(("LUNAC_LOG_DIR", crate::log::log_dir().to_string_lossy().to_string()));
    // WebSearch 主源（服务商 + key）→ agent.exe（core-agent 经
    // LUNAC_SEARCH_PROVIDER / LUNAC_SEARCH_KEY 读取）；未配置时 agent
    // 直接走无 key 的 Bing / 百度兜底源。
    if let Ok(p) = env::var("AI_SEARCH_PROVIDER") {
        let p = p.trim();
        if !p.is_empty() {
            envs.push(("LUNAC_SEARCH_PROVIDER", p.to_lowercase()));
        }
    }
    if let Ok(k) = env::var("AI_SEARCH_KEY") {
        let k = k.trim();
        if !k.is_empty() {
            envs.push(("LUNAC_SEARCH_KEY", k.to_string()));
        }
    }

    // 思考开关 → agent.exe 环境变量（只有开 / 关两档，见 docs/ai-spec.md §3.5）。
    // core-agent 依据 LUNAC_THINKING 决定 thinking 形态（off = disabled，其余 = 开），
    // 并对不接受该字段的端点自动走 400 降级链，无需宿主再干预。
    // **不再传「思考预算」** —— 实测端点不 enforce budget_tokens，传数字只会造成
    // 「三档真的不一样」的错觉（2026-09-15 收敛两档）。
    let thinking_mode = state
        .thinking_mode
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| "on".into());
    envs.push((
        "LUNAC_THINKING",
        if thinking_mode == "off" { "off" } else { "on" }.into(),
    ));

    // 权限 hooks（A9）→ agent.exe：**无条件给路径**（文件还不存在也给）。
    // 「文件不在 = 没配 hooks」这个判据只有 agent 一处（它按 mtime 热重载同一份文件），
    // 于是用户在设置面板新建/改 `enabled` 都能被立刻捕获 —— 若宿主只在「文件已存在」时
    // 才注入，用户这次开的开关就得等下次 spawn agent 才生效（一类「开了没反应」的坑）。
    envs.push((
        "LUNAC_HOOKS_FILE",
        crate::storage::hooks_config_path().to_string_lossy().to_string(),
    ));

    crate::log::info(format!(
        "agent spawn: workdir={} args=[{}]",
        workdir.display(),
        args.join(" ")
    ));
    let mut child = spawn_child(&agent_path, &args_refs, &workdir, &envs)?;

    // Unique instance id for this agent spawn — the frontend uses it to
    // discard stale "closed" events after a stop/restart.
    let instance = next_cli_instance();

    // Extract stdin handle before moving child → store for send_message
    let stdin = child.stdin.take();

    // Relay stdout → frontend Tauri events + HTTP SSE bridge
    // Filter: only emit lines that are valid JSON (ignore debug output)
    if let Some(stdout) = child.stdout.take() {
        let app_clone = app.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if let Ok(line) = line {
                    let trimmed = line.trim().to_string();
                    if !trimmed.is_empty() && trimmed.starts_with('{') {
                        let _ = app_clone.emit("cli-output", CliOutput { line: trimmed.clone() });
                        // Also broadcast to HTTP SSE subscribers (VSCode extension)
                        cli_bridge::broadcast_output(trimmed);
                    }
                }
            }
            // stdout pipe closed → CLI process exited
            let _ = app_clone.emit("cli-status", StatusPayload {
                state: "closed".into(),
                message: "CLI process exited".into(),
                instance,
            });
        });
    }

    // Relay stderr → frontend + dev terminal (debugging visibility)
    if let Some(stderr) = child.stderr.take() {
        let app_clone = app.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(line) = line {
                    if !line.trim().is_empty() {
                        // release 是 GUI 子系统、没有控制台，eprintln 线上看不到 —— 必须落盘
                        crate::log::warn(format!("[agent stderr] {line}"));
                        eprintln!("[cli] {}", line);
                        let _ = app_clone.emit("cli-stderr", line);
                    }
                }
            }
        });
    }

    // Store child (for kill access) and stdin (for send_message)
    // Both in AppState (Tauri) and cli_bridge (HTTP server)
    if let Some(stdin) = stdin {
        cli_bridge::set_stdin(stdin);
    }
    cli_bridge::set_process(child);

    // Signal frontend that CLI stdin is ready to receive messages.
    // The CLI buffers stdin input while it finishes loading plugins and
    // scanning --add-dir . Once initialized, it processes the buffered
    // message. system/init arrives later (per-query-turn) and is used
    // by the frontend for status display only — NOT for cliReady.
    if let Err(e) = app.emit("cli-status", StatusPayload {
        state: "stdout".into(),
        message: "CLI stdin ready".into(),
        instance,
    }) {
        eprintln!("[start_cli] Failed to emit stdout status: {}", e);
    }

    Ok(())
}

/// 解析 AI 凭据（`.env` 由 main.rs 载入进程环境）。
/// 返回 (api_url, api_key, model)。
fn ai_credentials() -> Result<(String, String, String), String> {
    let api_url = env::var("AI_API_URL")
        .or_else(|_| env::var("DEEPSEEK_URL"))
        .unwrap_or_else(|_| "https://api.deepseek.com".into());
    let api_key = env::var("AI_API_KEY")
        .or_else(|_| env::var("DEEPSEEK_API_KEY"))
        .map_err(|_| "No AI_API_KEY configured".to_string())?;
    // 兜底模型：仅在 `AI_MODEL` 完全没配（既无 config\ai.json 也无 .env）时生效。
    // 取 flash 而不是 pro：这类「什么都没配」的场景基本只剩测试与首次冒烟，
    // 按 ai-spec §11 规则 16 一律用 flash（更快更省，只验链路）。
    let model = env::var("AI_MODEL").unwrap_or_else(|_| "deepseek-flash".into());
    Ok((api_url, api_key, model))
}

/// 把凭据注入自研 agent 后端（core-agent）读取的环境变量。
/// 鉴权必须走 `authorization: Bearer`；用 `x-api-key` 会被兼容端点判 401。
/// 另注入 WebSearch 主源的「服务商 + key」（空 = 只走无 key 的 Bing/百度兜底）。
fn configure_agent_env(api_url: &str, api_key: &str, model: &str) {
    let agent_url = agent_endpoint(api_url, env::var("AI_AGENT_URL").ok().as_deref());
    // 记一行「这次喂给 agent 的是哪把 key」（末 4 位 + 端点 + 模型）：
    // 与 `AI 配置来源=…` 那行对照，「改了 key 仍 401」不用再翻 WebView2 的 leveldb。
    crate::log::info(crate::log::mask_secrets(&format!(
        "agent 凭据: endpoint={agent_url} model={model} key_tail={}",
        crate::log::key_tail(api_key),
    )));
    env::set_var("LUNAC_AGENT_BASE_URL", agent_url);
    env::set_var("LUNAC_AGENT_TOKEN", api_key);
    env::set_var("LUNAC_AGENT_MODEL", model);
    match env::var("AI_SEARCH_PROVIDER") {
        Ok(p) if !p.trim().is_empty() => env::set_var("LUNAC_SEARCH_PROVIDER", p.trim().to_lowercase()),
        _ => env::remove_var("LUNAC_SEARCH_PROVIDER"),
    }
    match env::var("AI_SEARCH_KEY") {
        Ok(k) if !k.trim().is_empty() => env::set_var("LUNAC_SEARCH_KEY", k.trim()),
        _ => env::remove_var("LUNAC_SEARCH_KEY"),
    }
}

/// 把一份 AI 配置**原样**注入进程环境变量（`ai.json` 存在时它说了算）。
///
/// 空串语义（与设置面板一致）：`provider` / `agent_url` / WebSearch 两项空 = 清掉变量。
/// `key` 空也**不回落 `.env`** —— 否则「文件是唯一真相源」又变成两份各执一词，
/// 正是 2026-09-15 那次 401 的成因（详见 storage.rs 顶部的注释）。
fn apply_ai_config(cfg: &crate::storage::AiConfig) {
    if cfg.provider.trim().is_empty() {
        env::remove_var("AI_PROVIDER");
    } else {
        env::set_var("AI_PROVIDER", cfg.provider.trim());
    }
    env::set_var("AI_API_URL", cfg.url.trim());
    env::set_var("AI_API_KEY", cfg.key.trim());
    env::set_var("AI_MODEL", cfg.model.trim());
    if cfg.agent_url.trim().is_empty() {
        env::remove_var("AI_AGENT_URL");
    } else {
        env::set_var("AI_AGENT_URL", cfg.agent_url.trim());
    }
    if cfg.search_provider.trim().is_empty() {
        env::remove_var("AI_SEARCH_PROVIDER");
    } else {
        env::set_var("AI_SEARCH_PROVIDER", cfg.search_provider.trim().to_lowercase());
    }
    if cfg.search_key.trim().is_empty() {
        env::remove_var("AI_SEARCH_KEY");
    } else {
        env::set_var("AI_SEARCH_KEY", cfg.search_key.trim());
    }
    // 图片输入开关（A8）：只在真开时留一个 `AI_VISION=1`，关着就**删掉变量** ——
    // `get_ai_config` 用它回读，缺变量即「关」（与 `.env` 也能写 `AI_VISION=1` 兼容）。
    if cfg.vision {
        env::set_var("AI_VISION", "1");
    } else {
        env::remove_var("AI_VISION");
    }
    if cfg.key.trim().is_empty() {
        crate::log::warn("AI key 为空 ⇒ 对话必然 401，请在设置面板填入 key");
    }
}

/// 启动时应用设置面板保存的 AI 配置（`<exe 根>\config\ai.json`）。
///
/// **AI 凭据的唯一真相源**：没有该文件时才用 `.env`（及其缺省值）。
/// 这里取代了旧的前端 localStorage 回灌 —— 那条路会在启动时把 localStorage 里的旧值
/// **覆盖**进 env，于是用户改了 `.env` 完全不生效（2026-09-15 的 401 即此），
/// 而且生效的是哪一份无从判断。两条路都记一行日志（含 key 末 4 位与来源）。
pub fn apply_saved_ai_config() {
    match crate::storage::load_ai_config() {
        Some(cfg) => {
            apply_ai_config(&cfg);
            crate::log::info(crate::log::mask_secrets(&format!(
                "AI 配置来源=config\\ai.json provider={} url={} model={} key_tail={}",
                cfg.provider,
                cfg.url,
                cfg.model,
                crate::log::key_tail(&cfg.key),
            )));
        }
        None => {
            let key = env::var("AI_API_KEY")
                .or_else(|_| env::var("DEEPSEEK_API_KEY"))
                .unwrap_or_default();
            crate::log::info(crate::log::mask_secrets(&format!(
                "AI 配置来源=.env（无 config\\ai.json）url={} model={} key_tail={}",
                env::var("AI_API_URL").unwrap_or_else(|_| "(默认)".into()),
                env::var("AI_MODEL").unwrap_or_else(|_| "(默认)".into()),
                crate::log::key_tail(&key),
            )));
        }
    }
}

/// 构造 agent 后端要连接的端点。
/// api_url 若带末尾 `/v1`（OpenAI 兼容风格的地址栏/预设），先剥离再拼
/// 供应商的兼容路由 `/anthropic`（外部路由，非本项目命名），避免出现
/// `/v1/anthropic` 双重路径；explicit 优先（`AI_AGENT_URL`）。
fn agent_endpoint(api_url: &str, explicit: Option<&str>) -> String {
    if let Some(a) = explicit {
        let a = a.trim();
        if !a.is_empty() {
            return a.to_string();
        }
    }
    let base = api_url.trim_end_matches('/');
    let root = match base.strip_suffix("/v1") {
        // 只剥纯末尾 /v1；host-only（http(s)://api.xxx）不会被误伤
        Some(r) if r.contains("://") => r,
        _ => base,
    };
    format!("{}/anthropic", root)
}

// ── Tauri Commands (IPC) ──────────────────────────────────────────

#[tauri::command]
pub async fn start_cli(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let (api_url, api_key, model) = ai_credentials()?;

    // 直连供应商的兼容端点（自带工具调用链）。内置代理 proxy_server.rs
    // 已停用 —— 它的 Anthropic→OpenAI 翻译会丢掉 `tools`，模型永远发不出
    // tool_use，只能退化成文本式 XML 工具调用。
    proxy_server::stop();
    configure_agent_env(&api_url, &api_key, &model);

    let dir = core_dir();
    let agent_exe = dir.join(AGENT_EXE);
    if !agent_exe.exists() {
        proxy_server::stop();
        return Err(format!("agent.exe not found at {}", agent_exe.display()));
    }

    // ripgrep：把随附目录（<core>\utils\vendor\ripgrep\x64-win32）前置进 PATH，
    // 并强制走系统 PATH 模式。
    let rg_dir = dir.join("utils").join("vendor").join("ripgrep").join("x64-win32");
    if rg_dir.join("rg.exe").exists() {
        let rg_str = rg_dir.display().to_string();
        let old_path = env::var("PATH").unwrap_or_default();
        if !old_path.contains(&rg_str) {
            env::set_var("PATH", format!("{};{}", rg_str, old_path));
        }
        env::set_var("USE_BUILTIN_RIPGREP", "0");
    }

    // Only emit starting if the agent is not already running — avoid spurious
    // cliReady=false with no follow-up stdout. ensure_agent_running()
    // (called before ai-mode-changed) already handles the first spawn.
    let already_running = cli_bridge::is_running();
    if !already_running {
        app.emit("cli-status", StatusPayload {
            state: "starting".into(),
            message: format!("Mode agent, agent.exe at {}", dir.display()),
            instance: 0, // informational — not tied to a specific spawn
        }).ok();
    }

    start_cli_process(&state, app.clone(), &dir)?;
    Ok("Agent started".into())
}

#[tauri::command]
pub async fn stop_cli(_state: State<'_, AppState>) -> Result<String, String> {
    crate::log::info("stop_cli: killing agent + proxy");
    cli_bridge::kill_and_cleanup();
    proxy_server::stop();
    Ok("All processes stopped".into())
}

#[tauri::command]
pub async fn send_message(_state: State<'_, AppState>, message: String) -> Result<String, String> {
    cli_bridge::write_to_cli(&message)?;
    Ok("Message sent".into())
}

#[tauri::command]
pub async fn get_status(_state: State<'_, AppState>) -> Result<StatusPayload, String> {
    let cli_running = cli_bridge::is_running();
    let state_str = if cli_running { "ready" } else { "idle" };
    let message = format!(
        "CLI: {}",
        if cli_running { "running" } else { "stopped" },
    );
    Ok(StatusPayload { state: state_str.into(), message, instance: 0 })
}

// ── Security profile ────────────────────────────────────────────

#[tauri::command]
pub async fn set_security_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    profile: String,
    restart: bool,
) -> Result<String, String> {
    // Validate profile
    if !["safe", "project", "full"].contains(&profile.as_str()) {
        return Err(format!("Invalid profile: {}. Use safe/project/full.", profile));
    }

    // Update stored profile
    {
        let mut guard = state.security_profile.lock().map_err(|e| e.to_string())?;
        *guard = profile.clone();
    }

    // 档位通过命令行参数在 agent.exe spawn 时生效，所以切换必须重启 agent。
    // restart=false（startup 同步）只存值、不拉起 CLI —— 与 set_thinking_mode
    // 同一套约定，保持懒启动。
    if restart {
        crate::log::info(format!("security profile → {profile}, restarting agent"));
        cli_bridge::kill_and_cleanup();
        ensure_agent_running(&state, &app)?;
    }

    Ok(format!("Security profile set to: {}", profile))
}

// ── Start Menu apps ─────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct AppEntry {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default = "default_source")]
    pub source: String,
}

/// Helper for serde(default=...)
#[allow(dead_code)]
fn default_source() -> String {
    "start_menu".into()
}

#[tauri::command]
pub fn list_start_menu_apps() -> Vec<AppEntry> {
    crate::app_indexer::apps()
        .into_iter()
        .map(|a| AppEntry {
            name: a.name,
            path: a.path,
            icon: a.icon,
            source: a.source,
        })
        .collect()
}

/// Search apps by fuzzy query — returns top matches for the search bar.
///
/// `(async)` = 抛到工作线程执行：Tauri 的**同步**命令跑在主线程上，而本命令
/// 每次击键都会被调用一次，一旦主线程被拖住就是「搜索卡住」（窗口唤出/隐藏
/// 期间尤其明显）。放到工作线程后，无论什么动作都不会阻塞界面。
#[tauri::command(async)]
pub fn search_apps(query: String, limit: usize) -> Vec<AppEntry> {
    crate::app_indexer::search_apps(&query, limit)
        .into_iter()
        .map(|a| AppEntry {
            name: a.name,
            path: a.path,
            icon: a.icon,
            source: a.source,
        })
        .collect()
}

/// Launch an app by path (.exe, .lnk, folder, or URL).
#[tauri::command]
pub fn launch_app(path: String) -> Result<(), String> {
    crate::app_indexer::launch_app(&path)
}

// ── 详细搜索（双击搜索栏进入的大界面，见 docs/ai-spec.md §2.1.2）──

/// 文件搜索：**只读内存索引，永不同步扫盘**（与 search_apps 同纪律 —— 一搜就卡
/// 的根源就是在搜索路径上扫目录）。索引未就绪时返回空数组，前端看
/// `file_index_status` 决定显示「正在建立索引」。
/// `(async)` 同理：搜索每次击键都会调，必须抛到工作线程。
#[tauri::command(async)]
pub fn search_files(query: String, kind: Option<String>, limit: usize) -> Vec<crate::file_indexer::FileHit> {
    crate::file_indexer::search(&query, kind.as_deref(), limit)
}

/// 索引状态：条数 / 是否在扫 / 落盘时间 / 是否被上限截断 / 扫了哪些根。
#[tauri::command]
pub fn file_index_status() -> crate::file_indexer::IndexStatus {
    crate::file_indexer::status()
}

/// 强制重建文件索引（后台线程，立即返回）。
#[tauri::command]
pub fn refresh_file_index() {
    crate::file_indexer::refresh_in_background();
}

/// 系统设置页 + 系统动作目录（几十条，前端一次拉走本地过滤）。
#[tauri::command]
pub fn system_catalog() -> Vec<crate::system_catalog::CatalogItem> {
    crate::system_catalog::all()
}

/// 打开一个 Windows 设置页（只接受 `ms-settings:` 前缀）。
#[tauri::command]
pub fn open_setting(target: String) -> Result<(), String> {
    crate::system_catalog::open_setting(&target)
}

/// 执行一个系统动作（只认 `system_catalog()` 里的动作 id）。
#[tauri::command]
pub fn run_system_action(id: String) -> Result<(), String> {
    crate::system_catalog::run_action(&id)
}

/// **以管理员身份运行**一个系统动作（`runas`，会弹 UAC）。
///
/// 只认 `system_catalog()` 里的动作 id，且**只有 `CatalogItem::elevatable == true`
/// 的那些能执行**（判据与目录同源，见 `system_catalog::run_action_elevated`）——
/// 前端多传一个 id 也只会拿到 Err，不会变成「任意命令的提权执行入口」。
/// `(async)`：`ShellExecuteW(runas)` 会阻塞等 UAC 交互，绝不能占主线程。
#[tauri::command(async)]
pub fn run_system_action_elevated(id: String) -> Result<(), String> {
    crate::system_catalog::run_action_elevated(&id)
}

/// **以管理员身份运行**任意启动目标（.exe / .lnk / .msc / .cpl / URL）。
/// 详细搜索里对「应用 / 文件 / 插件命令」结果的提权入口（Shift+Enter）。
/// 与 `launch_app` 同一信任边界（入参来自我们自己的 WebView 前端），
/// 额外的一层闸门是 UAC 本身。`(async)` 理由同 `run_system_action_elevated`。
#[tauri::command(async)]
pub fn launch_app_elevated(path: String) -> Result<(), String> {
    let raw = path.trim();
    if raw.is_empty() {
        return Err("路径为空".into());
    }
    crate::app_indexer::launch_elevated(raw, None)
}

/// 在资源管理器中**定位并选中**一个文件（backlog §8.1「被改动文件的路径追踪」）。
///
/// 入参来自模型在 `Write` / `Edit` 的 `tool_use` 里给出的 `file_path`，属**外部输入**，
/// 因此纪律与 `system_catalog::run_action` 完全一致：
///   ① **必须绝对路径**（相对路径的基准是 agent 的工作目录，不是用户的直觉）；
///   ② **必须真实存在**（不存在的路径 explorer 会静默退化成「打开文档目录」，用户会以为点错了）；
///   ③ **只作为 `Command` 的单个参数传入，全程不经 shell** —— 路径里的 `&` / `|` / `"` 都
///      不可能被解释成命令行语法。反面写法是 `cmd /c explorer /select,…`，那才是注入面。
///
/// 为什么用 `explorer.exe /select,`：这是「打开所在文件夹**并选中该文件**」唯一的系统级
/// 做法；只打开文件夹不满足用户「点击可进入这个文件相对应文件夹」的诉求。
/// `(async)` 与剪贴板命令同理：explorer 启动是同步系统调用，放工作线程避免卡住主线程。
#[tauri::command(async)]
pub fn reveal_in_explorer(path: String) -> Result<(), String> {
    let raw = path.trim();
    if raw.is_empty() {
        return Err("路径为空".into());
    }
    let p = std::path::PathBuf::from(raw);
    if !p.is_absolute() {
        return Err(format!("只接受绝对路径：{raw}"));
    }
    if !p.exists() {
        return Err(format!("路径不存在：{raw}"));
    }
    // `/select,<目录>` 带尾反斜杠时 explorer 会当成「打开该盘根」，先去掉。
    // 长度 <= 3 的是盘符根（`C:\`），去掉就变成 `C:`（当前目录），必须保留。
    let mut target = p.display().to_string();
    while target.len() > 3 && (target.ends_with('\\') || target.ends_with('/')) {
        target.pop();
    }
    std::process::Command::new("explorer.exe")
        .arg(format!("/select,{target}"))
        .spawn()
        .map_err(|e| format!("无法启动资源管理器：{e}"))?;
    crate::log::info(format!("reveal_in_explorer: {target}"));
    Ok(())
}

/// Add a custom app to the registry.
#[tauri::command]
pub fn add_custom_app(name: String, path: String) -> Result<String, String> {
    crate::app_indexer::add_custom_app(&name, &path)?;
    Ok(format!("Added: {} ({})", name, path))
}

/// Remove a custom app from the registry.
#[tauri::command]
pub fn remove_custom_app(path: String) -> Result<String, String> {
    crate::app_indexer::remove_custom_app(&path)?;
    Ok(format!("Removed: {}", path))
}

/// List all registered custom apps.
#[tauri::command]
pub fn list_custom_apps() -> Vec<AppEntry> {
    crate::app_indexer::list_custom_apps()
        .into_iter()
        .map(|a| AppEntry {
            name: a.name,
            path: a.path,
            icon: a.icon,
            source: a.source,
        })
        .collect()
}

/// Extract the system icon for a file path, returned as base64 PNG data URL.
#[tauri::command]
pub fn get_app_icon(path: String) -> Result<Option<String>, String> {
    Ok(crate::icon_extractor::extract_icon_base64(&path))
}

/// 文件缩略图（详细搜索右侧预览区）：图片真解码、其余回落系统类型图标。
/// `max` = 长边像素上限。`(async)` —— 解码 + 缩放是 CPU 密集的，
/// 不能占着主线程（详细搜索每移动一次选中行就会调一次）。
#[tauri::command(async)]
pub fn get_file_thumbnail(path: String, max: u32) -> Result<Option<String>, String> {
    Ok(crate::icon_extractor::extract_thumbnail_base64(&path, max))
}

// ── Query / recording / plugin / detached / chips state ──────────

#[tauri::command]
pub fn set_query_state(empty: bool) {
    crate::hotkey::QUERY_EMPTY.store(empty, std::sync::atomic::Ordering::SeqCst);
}

#[tauri::command]
pub fn set_recording_state(recording: bool) {
    crate::hotkey::RECORDING.store(recording, std::sync::atomic::Ordering::SeqCst);
}

#[tauri::command]
pub fn set_ui_mode(mode: String) {
    // 认不出的值（含 "main" 与拼错的）一律归 Main：Main 至少还有「query/chips 空
    // ⇒ 隐藏」的兜底，比误留在「大界面层」（Esc 永远只 emit clear、永远隐藏不掉）安全。
    let m = match mode.as_str() {
        "plugin" => crate::hotkey::UI_MODE_PLUGIN,
        "detail" => crate::hotkey::UI_MODE_DETAIL,
        _ => crate::hotkey::UI_MODE_MAIN,
    };
    crate::hotkey::UI_MODE.store(m, std::sync::atomic::Ordering::SeqCst);
}

#[tauri::command]
pub fn set_chips_empty(empty: bool) {
    crate::hotkey::CHIPS_EMPTY.store(empty, std::sync::atomic::Ordering::SeqCst);
}

#[tauri::command]
pub fn set_detached(detached: bool) {
    crate::hotkey::DETACHED.store(detached, std::sync::atomic::Ordering::SeqCst);
}

/// Update AI provider config at runtime (from the settings plugin).
///
/// **唯一真相源**是 `<exe 根>\config\ai.json`（写盘 + 注入进程 env，见 storage.rs）。
/// env 随即被 `configure_agent_env` 读走传给 agent.exe；`.env` 只在没有该文件时生效。
/// `agent_url` 覆盖 base+"/anthropic"，供非标准路由的供应商使用（如智谱 GLM）；空 = 默认规则。
#[tauri::command]
pub async fn set_ai_config(
    provider: String,
    url: String,
    key: String,
    model: String,
    agent_url: Option<String>,
    search_provider: Option<String>,
    search_key: Option<String>,
    vision: Option<bool>,
) -> Result<String, String> {
    if url.trim().is_empty() || model.trim().is_empty() {
        return Err("url / model must not be empty".into());
    }
    let cfg = crate::storage::AiConfig {
        provider: provider.trim().to_string(),
        url: url.trim().to_string(),
        key: key.trim().to_string(),
        model: model.trim().to_string(),
        agent_url: agent_url.unwrap_or_default().trim().to_string(),
        // WebSearch 主源：前端每次都回传输入框当前值，空串 = 用户清掉了（按删除处理，
        // 否则会一直用旧值）。
        search_provider: search_provider.unwrap_or_default().trim().to_lowercase(),
        search_key: search_key.unwrap_or_default().trim().to_string(),
        // 图片输入开关：缺省（老前端不带这个参数）按**关**处理 —— 与「默认不发图片块」
        // 的取向一致，不会因为漏传参数就把图片发给一个可能不支持的端点。
        vision: vision.unwrap_or(false),
    };
    // 先落盘再注入 env。落盘失败必须如实报错 —— 否则会重演「面板像是保存成功、
    // 重启后又变回旧值」这种最难查的问题。
    crate::storage::save_ai_config(&cfg)?;
    apply_ai_config(&cfg);
    crate::log::info(crate::log::mask_secrets(&format!(
        "AI 配置已保存（设置面板 → config\\ai.json）provider={} url={} model={} vision={} key_tail={}",
        cfg.provider,
        cfg.url,
        cfg.model,
        cfg.vision,
        crate::log::key_tail(&cfg.key),
    )));
    Ok("AI config updated".into())
}

/// Return current AI config from env vars.
#[tauri::command]
pub fn get_ai_config() -> serde_json::Value {
    serde_json::json!({
        "provider": env::var("AI_PROVIDER").unwrap_or_default(),
        "base_url": env::var("AI_API_URL").unwrap_or_default(),
        "model": env::var("AI_MODEL").unwrap_or_default(),
        "api_key": env::var("AI_API_KEY").unwrap_or_default(),
        "agent_url": env::var("AI_AGENT_URL").unwrap_or_default(),
        "search_provider": env::var("AI_SEARCH_PROVIDER").unwrap_or_default(),
        "search_key": env::var("AI_SEARCH_KEY").unwrap_or_default(),
        // 图片输入开关（A8）：缺变量 = 关。前端据此决定要不要把图片附件发成 `image` 块。
        "vision": env::var("AI_VISION").map(|v| v.trim() == "1").unwrap_or(false),
    })
}

// ── 权限 hooks（A9，2026-09-20）────────────────────────────────────
//
// 用户脚本的挂载点（`PreToolUse` / `PostToolUse` / `UserPromptSubmit` 等 8 个事件，
// 契约见 ai-spec §3.5「权限 hooks」）。宿主这一侧只做设置面板需要的最小面：
// 读状态、写开关、打开配置文件 —— **不做任何 hook 判定**，那些全在 agent 侧。
//
// 两个口径：
//  · **开关就是文件里的 `enabled` 字段**（缺省 `true`）。不另存一份前端状态：
//    「面板上显示开着、agent 实际没跑」这种不一致一旦出现就极难查。
//  · **agent 按 mtime 热重载**这个文件 ⇒ 改配置**即时生效、不必重启 agent**；
//    所以宿主只在 spawn 时注入文件路径（`LUNAC_HOOKS_FILE`），不注入任何开关值。

/// 解析 hooks.json。**容忍 UTF-8 BOM** —— 编辑器把 JSON 存成「UTF-8 带 BOM」是常事，
/// 而 serde_json 见到 BOM 会判整份配置非法（agent 侧同样做了这个容忍，判据一致）。
fn parse_hooks_json(text: &str) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::from_str(text.trim_start_matches('\u{feff}'))
}

/// 读 hooks 配置状态（设置面板用）：路径 / 是否存在 / 是否启用 / 语法错误。
#[tauri::command]
pub fn get_hooks_config() -> serde_json::Value {
    let path = crate::storage::hooks_config_path();
    let text = crate::storage::load_hooks_text().ok().flatten();
    let (enabled, error) = match text.as_deref() {
        None => (false, None),
        Some(t) => match parse_hooks_json(t) {
            Ok(v) if v.is_object() => (
                v.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true),
                None,
            ),
            Ok(_) => (false, Some("hooks.json 顶层必须是一个对象".to_string())),
            Err(e) => (false, Some(format!("hooks.json 语法错误：{e}"))),
        },
    };
    serde_json::json!({
        "path": path.display().to_string(),
        "exists": text.is_some(),
        "enabled": enabled,
        "error": error,
    })
}

/// 开关 hooks（写 `hooks.json` 的 `enabled` 字段，**用户写的其它字段原样保留**）。
#[tauri::command]
pub fn set_hooks_enabled(enabled: bool) -> Result<String, String> {
    let mut v = match crate::storage::load_hooks_text()? {
        None => serde_json::json!({ "hooks": {} }),
        Some(t) => match parse_hooks_json(&t) {
            Ok(v) if v.is_object() => v,
            Ok(_) => return Err("hooks.json 顶层必须是一个对象，请先修好它".into()),
            // 语法坏掉时**不覆盖**：用户很可能正在编辑器里改这份文件，
            // 按一下开关就把他写了一半的内容整段丢掉是最糟的处理。
            Err(e) => return Err(format!("hooks.json 语法错误，已保留原文件：{e}")),
        },
    };
    v["enabled"] = serde_json::json!(enabled);
    let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    crate::storage::save_hooks_text(&format!("{text}\n"))?;
    crate::log::info(format!("hooks 开关：enabled={enabled}"));
    Ok(if enabled {
        "hooks enabled".into()
    } else {
        "hooks disabled".into()
    })
}

/// 确保 hooks.json 存在，返回它的路径（缺文件时落一份骨架）。
///
/// **打开交给前端**（设置面板本来就在用 `@tauri-apps/plugin-shell` 的 `open`，见
/// 主题目录那行）：宿主不必为了「打开一个文件」再拉一次外部进程，
/// 也避免 `Shell::open` 在新版里被标弃用。
#[tauri::command]
pub fn hooks_file_path() -> Result<String, String> {
    Ok(crate::storage::ensure_hooks_file()?.display().to_string())
}

// ── AI Workspace ──────────────────────────────────────────────────
// Simple mode removed (2026-08-04): the app runs Agent (agent.exe) only.
// The workspace is the directory the agent may operate in — used as the
// CLI working directory and --add-dir scope. Empty string = user home dir
// (whole system reachable; edits elsewhere go through ask approval).

/// Set the agent workspace directory. Empty string resets to the default
/// (user home dir). When the CLI is already running the caller should stop it
/// (`stop_cli`) so the next start picks up the new workspace.
#[tauri::command]
pub fn set_workspace(path: String, state: State<'_, AppState>) -> Result<String, String> {
    let trimmed = path.trim();
    let resolved = if trimmed.is_empty() {
        String::new()
    } else {
        let p = std::path::Path::new(trimmed);
        let canonical = p
            .canonicalize()
            .map_err(|e| format!("Workspace not found: {}", e))?;
        if !canonical.is_dir() {
            return Err("Workspace must be a directory".into());
        }
        canonical.display().to_string()
    };
    {
        let mut guard = state.workspace.lock().map_err(|e| e.to_string())?;
        *guard = resolved.clone();
    }
    Ok(if resolved.is_empty() {
        "Workspace reset to default (user home)".into()
    } else {
        format!("Workspace set: {}", resolved)
    })
}

#[tauri::command]
pub fn get_workspace(state: State<'_, AppState>) -> String {
    state.workspace.lock().map(|g| g.clone()).unwrap_or_default()
}

/// Set the user-custom tool blacklist (第 19 点缓存优化)。
/// 下次 agent.exe 启动时生效（tool_blacklist_args）——调用方负责 stop_cli + start_cli。
#[tauri::command]
pub fn set_tool_blacklist(
    state: State<'_, AppState>,
    blacklist: Vec<String>,
) -> Result<String, String> {
    let mut b = state
        .tool_blacklist
        .lock()
        .map_err(|_| "Failed to lock AppState".to_string())?;
    *b = blacklist;
    Ok("Tool blacklist updated".into())
}

/// Internal: start the agent (direct provider connection). Does NOT start local model (caller decides).
fn ensure_agent_running(state: &AppState, app: &AppHandle) -> Result<(), String> {
    // Already running? (cli_bridge owns the process since the HTTP bridge refactor)
    if cli_bridge::is_running() {
        return Ok(());
    }

    let (api_url, api_key, model) = ai_credentials()?;

    // 直连供应商的兼容端点 —— 与 start_cli() 一致。内置代理会丢掉
    // `tools` 数组，模型永远发不出 tool_use，只能返回原始 XML 工具文本
    // （`<toolcall ...>`），在对话气泡里渲染成乱码。
    proxy_server::stop();
    configure_agent_env(&api_url, &api_key, &model);

    let dir = core_dir();
    let agent_exe = dir.join(AGENT_EXE);
    if !agent_exe.exists() {
        proxy_server::stop();
        return Err(format!("agent.exe not found at {}", agent_exe.display()));
    }

    // ripgrep：与 start_cli() 相同 —— 随附目录前置进 PATH 并强制走系统 PATH 模式。
    let rg_dir = dir.join("utils").join("vendor").join("ripgrep").join("x64-win32");
    if rg_dir.join("rg.exe").exists() {
        let rg_str = rg_dir.display().to_string();
        let old_path = env::var("PATH").unwrap_or_default();
        if !old_path.contains(&rg_str) {
            env::set_var("PATH", format!("{};{}", rg_str, old_path));
        }
        env::set_var("USE_BUILTIN_RIPGREP", "0");
    }

    app.emit("cli-status", StatusPayload {
        state: "starting".into(),
        message: format!("Agent CLI at {}", dir.display()),
        instance: 0, // informational — not tied to a specific spawn
    }).ok();

    start_cli_process(state, app.clone(), &dir)?;
    Ok(())
}

#[tauri::command]
pub async fn set_ai_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    mode: String,
) -> Result<String, String> {
    if mode != "agent" {
        return Err("Only agent mode is supported (simple mode removed)".into());
    }

    let current = state.ai_mode.lock().map_err(|e| e.to_string())?.clone();
    if current == mode {
        return Ok("Already in agent mode".into());
    }

    {
        let mut guard = state.ai_mode.lock().map_err(|e| e.to_string())?;
        *guard = mode.clone();
    }

    ensure_agent_running(&state, &app)?;
    Ok("Switched to agent mode".into())
}

#[tauri::command]
pub async fn set_thinking_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    mode: String,
    restart: bool,
) -> Result<String, String> {
    if !["on", "off"].contains(&mode.as_str()) {
        return Err(format!("Invalid thinking mode: {}. Use on/off.", mode));
    }

    {
        let mut guard = state.thinking_mode.lock().map_err(|e| e.to_string())?;
        *guard = mode.clone();
    }

    // 思考开关通过环境变量在 agent.exe spawn 时生效。restart=true（用户显式
    // 切换）才重启 CLI；startup 同步只存值、不拉起 CLI（保持懒启动）。
    if restart {
        cli_bridge::kill_and_cleanup();
        ensure_agent_running(&state, &app)?;
    }
    Ok(format!("Thinking mode set to: {}", mode))
}

// ── Hotkey configuration (from settings plugin) ──────────────────

/// Update the global hotkey combo. The LL hook in hotkey.rs picks up
/// the new key immediately (atomic swap). Persisted to disk.
#[tauri::command]
pub fn set_hotkey_combo(combo: String) -> Result<String, String> {
    crate::hotkey::parse_and_set_hotkey(&combo)?;
    Ok(format!("Hotkey set to: {}", combo))
}

/// Get the current hotkey combo string for display in settings.
#[tauri::command]
pub fn get_hotkey_combo() -> String {
    crate::hotkey::get_current_hotkey_string()
}

/// Check if a file exists at the given path (used by quick-launch plugin).
#[tauri::command]
pub fn check_file_exists(path: String) -> bool {
    std::path::Path::new(&path).exists()
}

// ── Auto-start commands ──────────────────────────────────────────

/// Enable or disable launch at Windows startup via the HKCU Run registry key
/// (see auto_start.rs). Pass { enabled: true } to enable,
/// { enabled: false } to disable.
#[tauri::command]
pub fn set_auto_start(enabled: bool) -> Result<String, String> {
    if enabled {
        crate::auto_start::enable_auto_start()?;
        Ok("Auto-start enabled".into())
    } else {
        crate::auto_start::disable_auto_start()?;
        Ok("Auto-start disabled".into())
    }
}

/// 读取自启状态与**实际生效机制**（`task` / `run` / `both` / `none`）。
///
/// UI 只用 `enabled` 拨开关 —— 机制名是纯实现术语，不做展示（见 ai-spec §11 规则 1）。
/// `mechanism` 保留给落盘日志：计划任务与 HKCU Run 的触发时间实测差约 60 秒，
/// 「开机后要等很久」这类反馈只能靠它判断实际走的是哪条（见 ai-spec §9.1 难点 2）。
#[tauri::command]
pub fn get_auto_start_info() -> Result<crate::auto_start::AutoStartInfo, String> {
    crate::auto_start::auto_start_info()
}

// ── User tool definitions (MCP bridge) ────────────────────────────

/// List user-defined tool files from <exe 根>\tools\
#[tauri::command]
pub fn list_tool_files() -> Result<Vec<ToolFileEntry>, String> {
    let dir = mcp_tools_dir();
    let mut files = Vec::new();

    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
    }

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "json") {
                let content = std::fs::read_to_string(&path)
                    .unwrap_or_default();
                let tool: Result<crate::mcp_server::ToolDef, _> =
                    serde_json::from_str(&content);
                files.push(ToolFileEntry {
                    filename: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    name: tool.as_ref().map(|t| t.name.clone()).unwrap_or_default(),
                    description: tool
                        .as_ref()
                        .map(|t| t.description.clone())
                        .unwrap_or_default(),
                    valid: tool.is_ok(),
                });
            }
        }
    }

    Ok(files)
}

/// Read a user-defined tool file
#[tauri::command]
pub fn read_tool_file(filename: String) -> Result<String, String> {
    let path = mcp_tools_dir().join(&filename);
    if !path.exists() {
        return Err(format!("Tool file not found: {}", filename));
    }
    std::fs::read_to_string(&path).map_err(|e| format!("Read error: {}", e))
}

/// Save a user-defined tool file
#[tauri::command]
pub fn save_tool_file(filename: String, content: String) -> Result<String, String> {
    // Basic validation: must be valid JSON with required fields
    let _tool: crate::mcp_server::ToolDef =
        serde_json::from_str(&content).map_err(|e| format!("Invalid tool JSON: {}", e))?;

    let dir = mcp_tools_dir();
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    }

    let path = dir.join(&filename);
    std::fs::write(&path, &content).map_err(|e| format!("Write error: {}", e))?;

    Ok(format!("Tool saved: {}", filename))
}

/// Delete a user-defined tool file
#[tauri::command]
pub fn delete_tool_file(filename: String) -> Result<String, String> {
    let path = mcp_tools_dir().join(&filename);
    if !path.exists() {
        return Err(format!("Tool file not found: {}", filename));
    }
    std::fs::remove_file(&path).map_err(|e| format!("Delete error: {}", e))?;
    Ok(format!("Tool deleted: {}", filename))
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolFileEntry {
    pub filename: String,
    pub name: String,
    pub description: String,
    pub valid: bool,
}

fn mcp_tools_dir() -> std::path::PathBuf {
    crate::storage::lunac_root_dir().join("tools")
}

// ── 已安装技能（设置「技能扩展」）──────────────────────────────
// 固定技能目录：<exe_dir>\skills，布局 <技能名>/SKILL.md。
// agent.exe 通过 LUNAC_SKILLS_DIR 环境变量读取同一目录；
// lunac 前端在此列出 / 新建 / 编辑 / 删除。

/// 固定技能根目录（与 MCP tools 同级，均在 exe 安装根下）。
pub fn lunac_skills_dir() -> std::path::PathBuf {
    crate::storage::lunac_root_dir().join("skills")
}

#[derive(Debug, Serialize, Clone)]
pub struct InstalledSkill {
    pub name: String,
    pub description: String,
    /// 子目录 key（目录名），读写/删除以它为标识
    pub key: String,
    /// skill 所在目录完整路径（前端「打开」定位用）
    pub dir: String,
}

/// 从文本解析 SKILL.md frontmatter 的 name / description（兼容无 frontmatter）。
fn parse_skill_md_str(content: &str) -> (String, String) {
    let mut name = String::new();
    let mut desc = String::new();
    if let Some(rest) = content.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                let line = line.trim();
                if let Some(v) = line.strip_prefix("name:") {
                    name = v.trim().trim_matches('"').trim_matches('\'').to_string();
                } else if let Some(v) = line.strip_prefix("description:") {
                    desc = v.trim().trim_matches('"').trim_matches('\'').to_string();
                }
            }
        }
    }
    (name, desc)
}

fn parse_skill_md(path: &std::path::Path) -> (String, String) {
    parse_skill_md_str(&std::fs::read_to_string(path).unwrap_or_default())
}

/// 技能名 → 目录 key（小写 kebab，安全字符）。
fn slugify_skill_key(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            last_dash = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            if last_dash {
                continue;
            }
            last_dash = true;
            out.push('-');
        }
    }
    let s = out.trim_matches('-').to_string();
    if s.is_empty() {
        "untitled-skill".to_string()
    } else {
        s
    }
}

/// 校验并解析 key 为技能目录（防目录穿越：key 只允许安全 slug）。
fn skill_dir_for(key: &str) -> Result<std::path::PathBuf, String> {
    let key = slugify_skill_key(key);
    if key.is_empty() {
        return Err("Invalid skill key".into());
    }
    Ok(lunac_skills_dir().join(key))
}

/// 列出固定技能目录中的已安装技能。
#[tauri::command]
pub fn list_installed_skills() -> Vec<InstalledSkill> {
    let root = lunac_skills_dir();
    let mut out = Vec::new();
    if !root.is_dir() {
        return out;
    }
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let skill_md = dir.join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }
            let key = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if key.is_empty() {
                continue;
            }
            let (name, desc) = parse_skill_md(&skill_md);
            out.push(InstalledSkill {
                name: if name.is_empty() { key.clone() } else { name },
                description: desc,
                key,
                dir: dir.to_string_lossy().to_string(),
            });
        }
    }
    out
}

/// 读取某个已安装技能的 SKILL.md 全文（编辑用）。
#[tauri::command]
pub fn read_skill_file(key: String) -> Result<String, String> {
    let dir = skill_dir_for(&key)?;
    let path = dir.join("SKILL.md");
    std::fs::read_to_string(&path).map_err(|e| format!("Read error: {}", e))
}

/// 保存（覆盖更新）某个已安装技能的 SKILL.md。
#[tauri::command]
pub fn save_skill_file(key: String, content: String) -> Result<String, String> {
    let dir = skill_dir_for(&key)?;
    if !content.trim_start().starts_with("---") {
        return Err("SKILL.md 需以 --- frontmatter 开头（含 name/description）".into());
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("SKILL.md"), &content).map_err(|e| format!("Write error: {}", e))?;
    Ok(key)
}

/// 新建 / 粘贴导入技能：内容写入 <技能目录>/<key>/SKILL.md，key 由 frontmatter.name 派生。
#[tauri::command]
pub fn import_skill_content(content: String) -> Result<String, String> {
    if !content.trim_start().starts_with("---") {
        return Err("SKILL.md 需以 --- frontmatter 开头（含 name）".into());
    }
    let (name, _) = parse_skill_md_str(&content);
    if name.trim().is_empty() {
        return Err("SKILL.md frontmatter 缺少 name".into());
    }
    let key = slugify_skill_key(&name);
    let dir = skill_dir_for(&key)?;
    if dir.exists() {
        return Err(format!("技能“{}”已存在，可直接编辑", name));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("SKILL.md"), &content).map_err(|e| format!("Write error: {}", e))?;
    Ok(key)
}

/// 从 raw SKILL.md URL 安装技能到固定目录。
#[tauri::command]
pub fn install_skill_from_url(url: String) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("Client error: {}", e))?;
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("Download failed: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}: download failed", resp.status().as_u16()));
    }
    let body = resp.text().map_err(|e| format!("Read error: {}", e))?;
    if !body.trim_start().starts_with("---") {
        return Err("该 URL 内容不是 SKILL.md（缺少 --- frontmatter）".into());
    }
    let (name, _) = parse_skill_md_str(&body);
    let key = if name.trim().is_empty() {
        let stem = url.rsplit('/').next().unwrap_or("");
        let stem = stem.trim_end_matches(".md").trim_end_matches(".MD");
        slugify_skill_key(stem)
    } else {
        slugify_skill_key(&name)
    };
    let dir = skill_dir_for(&key)?;
    if dir.exists() {
        return Err(format!("技能“{}”已存在，可直接编辑", key));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("SKILL.md"), &body).map_err(|e| format!("Write error: {}", e))?;
    Ok(key)
}

/// 删除技能目录（递归）。
#[tauri::command]
pub fn delete_skill(key: String) -> Result<String, String> {
    let dir = skill_dir_for(&key)?;
    if !dir.exists() {
        return Err("Skill not found".into());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("Delete error: {}", e))?;
    Ok(key)
}

/// Download a tool definition JSON from a URL and save to the tools directory.
/// Returns the saved filename on success.
#[tauri::command]
pub fn download_tool_from_url(url: String) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Client error: {}", e))?;

    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("Download failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}: download failed", resp.status().as_u16()));
    }

    let body = resp
        .text()
        .map_err(|e| format!("Read error: {}", e))?;

    // Validate as ToolDef
    let _tool: crate::mcp_server::ToolDef =
        serde_json::from_str(&body).map_err(|e| format!("Invalid tool JSON: {}", e))?;

    // Derive filename from tool name, fallback to URL stem
    let filename = if let Ok(tool) = serde_json::from_str::<crate::mcp_server::ToolDef>(&body) {
        let base = tool.name.replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "-");
        format!("{}.json", base)
    } else {
        let stem = url
            .split('/')
            .last()
            .unwrap_or("tool")
            .split('?')
            .next()
            .unwrap_or("tool");
        if stem.ends_with(".json") { stem.to_string() } else { format!("{}.json", stem) }
    };

    let dir = mcp_tools_dir();
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    }

    let path = dir.join(&filename);
    std::fs::write(&path, &body).map_err(|e| format!("Write error: {}", e))?;

    Ok(filename)
}

// ── Windows OCR ──────────────────────────────────────────────────
/// Recognize text from an image file using Windows 10/11 built-in OCR.
#[tauri::command]
pub fn run_ocr(path: String) -> Result<String, String> {
    windows_ocr::recognize_image(&path)
}

// ── PaddleOCR-json ──────────────────────────────────────────────
/// Recognize text from an image file using PaddleOCR-json (PP-OCRv4 model).
/// Supports Chinese, English, Japanese, Korean, Cyrillic.
/// Much higher accuracy than Windows OCR, especially for Chinese text.
/// lang: "chs" (default, Chinese+English), "cht", "en", "japan", "korean", "cyrillic"
#[tauri::command]
pub fn run_paddle_ocr(path: String, lang: Option<String>) -> Result<String, String> {
    let lang = lang.unwrap_or_else(|| "chs".into());
    paddle_ocr::recognize_image(&path, &lang)
}

// ── OCR 引擎（PaddleOCR-json）按需安装 ───────────────────────────
//
// 引擎体积大（.7z 约 88MB / 解压后约 300MB），不随仓库分发。缺失时前端
// 弹出「下载并安装」入口 → 本命令后台下载解压到 <exe 根>\paddle-ocr。
// 进度与结果通过事件回传，避免长耗时的 IPC 阻塞。

/// 安装是否进行中（防重入：下载是长任务，重复触发会浪费带宽并产生竞态）
static OCR_ENGINE_INSTALLING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 引擎是否已就绪。
#[tauri::command]
pub fn ocr_engine_status() -> bool {
    paddle_ocr::engine_installed()
}

/// 下载并安装 OCR 引擎（后台线程执行，立即返回）。
/// 事件：
///   `ocr-engine-progress` { downloaded, total }  下载进度（total=0 表示未知）
///   `ocr-engine-ready`    ()                     安装成功
///   `ocr-engine-error`    String                 失败原因
#[tauri::command]
pub fn ocr_engine_install(app: AppHandle) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    if OCR_ENGINE_INSTALLING.swap(true, Ordering::SeqCst) {
        return Err("OCR 引擎正在安装中，请稍候".into());
    }
    let handle = app.clone();
    thread::spawn(move || {
        let result = paddle_ocr::install_engine(|downloaded, total| {
            let _ = handle.emit(
                "ocr-engine-progress",
                serde_json::json!({ "downloaded": downloaded, "total": total }),
            );
        });
        OCR_ENGINE_INSTALLING.store(false, Ordering::SeqCst);
        match result {
            Ok(()) => {
                let _ = handle.emit("ocr-engine-ready", ());
            }
            Err(e) => {
                let _ = handle.emit("ocr-engine-error", e);
            }
        }
    });
    Ok(())
}

// ── Window control ──────────────────────────────────────────────
/// Directly hide the Lunac window via ShowWindow(SW_HIDE).
/// Bypasses Tauri's Window API to avoid any IPC queue delays.
/// Used as a fast-path double-insurance alongside win.hide() in launchApp.
#[tauri::command]
pub fn hide_lunac() {
    crate::hotkey::hide_window();
}

// ── 前端日志（window.onerror / unhandledrejection）───────────────
/// 前端把未捕获的 JS 错误转发到这里落盘。前端在 release 下同样没有控制台，
/// DevTools 平时也不会开 —— 不落盘就等于「用户看到报错、我们什么都查不到」。
/// 只收错误与警告：这是错误通道，不是通用日志通道（避免刷屏与体积失控）。
#[tauri::command]
pub fn log_frontend(level: String, message: String) {
    let msg = crate::log::mask_secrets(&message);
    match level.as_str() {
        "warn" => crate::log::warn(format!("[frontend] {msg}")),
        _ => crate::log::error(format!("[frontend] {msg}")),
    }
}

// ── Temp image save (for clipboard OCR) ──────────────────────────
/// 临时图片文件的**唯一**路径：`lunac_<tag>_<pid>_<毫秒>_<序号>.<ext>`。
///
/// 为什么不能只带 pid（A8，2026-09-20 修）：同一进程里连续两次粘贴会算到**同一个**
/// 路径 —— 旧 chip 还指着它，内容却已经被第二次粘贴覆盖。做图片内容块时这会被放大成
/// 「同一张附件在两次提问里内容不同」，所以在源头就让它唯一。
fn temp_image_path(tag: &str, ext: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("lunac_{tag}_{}_{ms}_{n}.{ext}", std::process::id()))
}

/// Save a base64 data URL as a temporary image file, return the path.
/// Detects image format from the MIME type in the data URL and uses the
/// correct file extension (png/jpg/bmp) for PaddleOCR-json compatibility.
#[tauri::command]
pub fn save_temp_image(data_url: String) -> Result<String, String> {
    // Extract MIME type: data:image/<fmt>;base64,... or data:image/<fmt>,...
    let mime = data_url
        .split("data:")
        .nth(1)
        .and_then(|s| s.split(';').next())
        .and_then(|s| s.split(',').next())
        .unwrap_or("image/png");
    let ext = match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/bmp" => "bmp",
        "image/webp" => "png",     // PaddleOCR-json does not support webp natively
        "image/gif" => "png",       // PaddleOCR-json does not support gif natively
        "image/tiff" | "image/tif" => "tiff",
        _ => "png",                 // default: PNG (canvas.toDataURL always outputs PNG)
    };

    // Parse base64: strip the header part (data:image/...;base64,)
    let b64 = data_url
        .split(',')
        .nth(1)
        .ok_or("Invalid data URL format")?;
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        b64,
    )
    .map_err(|e| format!("Base64 decode failed: {e}"))?;

    let tmp_path = temp_image_path("ocr", ext);
    std::fs::write(&tmp_path, &bytes)
        .map_err(|e| format!("Write temp file failed: {e}"))?;

    Ok(tmp_path.to_string_lossy().to_string())
}

/// Delete a temporary image file (cleanup after OCR).
#[tauri::command]
pub fn delete_temp_image(path: String) -> Result<String, String> {
    let p = std::path::Path::new(&path);
    // Only delete files in the temp directory (safety guard)
    let tmp_dir = std::env::temp_dir();
    if p.starts_with(&tmp_dir) && p.is_file() {
        std::fs::remove_file(p).map_err(|e| format!("Delete temp file failed: {e}"))?;
        Ok("Deleted".into())
    } else {
        Ok("Skipped (not in temp)".into())
    }
}

// ── Clipboard file paths (CF_HDROP) ────────────────────────────
/// Read image file paths from the clipboard (CF_HDROP format).
/// Handles the case where user copies image files from Explorer
/// rather than image data. Returns Vec of paths ending in common
/// image extensions. Returns empty Vec if no image files on clipboard.
///
/// `(async)` = 抛到工作线程执行。**本命令只做 `OpenClipboard(0)` 开头的纯 Win32 FFI**
/// （hWndNewOwner 传 0 = 关联当前任务，不依赖任何窗口句柄），因此没有线程亲和性，
/// 放到工作线程是安全的。为什么必须这么做：同步命令跑在 Tauri 主线程上，而本命令在
/// 唤出的那一刻就会被 `triggerJSClipboardRead()` 调用 —— 高负载下主线程一旦被拖住，
/// 「剪贴板 → 合成 input → 去抖 → 重跑搜索」这条唯一的重算链整体推后，用户看到的就是
/// 「唤出后停在上次搜索结果」的静态帧（与 L843 那里同因，见 ai-spec §11 规则 4）。
#[tauri::command(async)]
pub fn read_clipboard_files() -> Vec<String> {
    read_clipboard_image_files()
}

/// Read ALL file paths from the clipboard (CF_HDROP format), not just
/// images — used by the paste handler so AI gets the real source path
/// instead of WebView2's `C:\fakepath\...` (点14/17).
#[tauri::command]
pub fn read_clipboard_file_paths() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        use std::ptr;

        #[link(name = "user32")]
        extern "system" {
            fn OpenClipboard(hWndNewOwner: isize) -> i32;
            fn CloseClipboard() -> i32;
            fn GetClipboardData(uFormat: u32) -> isize;
        }

        #[link(name = "shell32")]
        extern "system" {
            fn DragQueryFileW(hDrop: isize, iFile: u32, lpszFile: *mut u16, cch: u32) -> u32;
        }

        const CF_HDROP: u32 = 15;

        let mut paths = Vec::new();
        unsafe {
            if OpenClipboard(0) == 0 {
                return paths;
            }
            let h = GetClipboardData(CF_HDROP);
            if h != 0 {
                let count = DragQueryFileW(h, 0xFFFFFFFF, ptr::null_mut(), 0) as usize;
                for i in 0..count {
                    let len = DragQueryFileW(h, i as u32, ptr::null_mut(), 0) as usize;
                    if len == 0 { continue; }
                    let mut buf: Vec<u16> = vec![0; len + 1];
                    DragQueryFileW(h, i as u32, buf.as_mut_ptr(), buf.len() as u32);
                    if let Some(path) = OsString::from_wide(&buf[..len]).into_string().ok() {
                        paths.push(path);
                    }
                }
            }
            CloseClipboard();
        }
        paths
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}



#[cfg(target_os = "windows")]
fn read_clipboard_image_files() -> Vec<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::ptr;

    const IMAGE_EXTS: &[&str] = &[
        ".png", ".jpg", ".jpeg", ".bmp", ".tiff", ".tif", ".webp", ".gif", ".ico",
        ".PNG", ".JPG", ".JPEG", ".BMP", ".TIFF", ".TIF", ".WEBP", ".GIF", ".ICO",
    ];

    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(hWndNewOwner: isize) -> i32;
        fn CloseClipboard() -> i32;
        fn GetClipboardData(uFormat: u32) -> isize;
    }

    #[link(name = "shell32")]
    extern "system" {
        fn DragQueryFileW(hDrop: isize, iFile: u32, lpszFile: *mut u16, cch: u32) -> u32;
    }

    const CF_HDROP: u32 = 15;

    let mut paths = Vec::new();

    unsafe {
        if OpenClipboard(0) == 0 {
            return paths;
        }

        let h = GetClipboardData(CF_HDROP);
        if h != 0 {
            let count = DragQueryFileW(h, 0xFFFFFFFF, ptr::null_mut(), 0) as usize;
            for i in 0..count {
                let len = DragQueryFileW(h, i as u32, ptr::null_mut(), 0) as usize;
                if len == 0 { continue; }
                let mut buf: Vec<u16> = vec![0; len + 1];
                DragQueryFileW(h, i as u32, buf.as_mut_ptr(), buf.len() as u32);
                if let Some(path) = OsString::from_wide(&buf[..len]).into_string().ok() {
                    if IMAGE_EXTS.iter().any(|ext| path.ends_with(ext)) {
                        paths.push(path);
                    }
                }
            }
        }

        CloseClipboard();
    }

    paths
 }

#[cfg(not(target_os = "windows"))]
fn read_clipboard_image_files() -> Vec<String> {
    Vec::new()
}

// ── Clipboard backup image (Raw DIB → BMP) ────────────────────
/// 纯 Win32 FFI 剪贴板图片读取，不依赖 GDI、image crate 或外部进程。
///
/// ShareX 等截图工具在剪贴板上放 CF_DIB（BI_BITFIELDS 压缩）和/或
/// CF_DIBV5，但不一定放 CF_BITMAP。CF_DIB 本质上是去掉 BMP 文件头
/// 的完整位图数据 —— 只需前插 14 字节 BITMAPFILEHEADER 即得合法 BMP。
///
/// 此方案与 arboard 无冲突（不同剪贴板格式），不触发任何 clipboard lock。
/// 成功返回临时 BMP 文件路径，失败返回空字符串。
///
/// `(async)` 的理由同 `read_clipboard_files`：本命令（`clipboard_dib_to_bmp` /
/// `clipboard_png_to_file`）同样是「`OpenClipboard(0)` + GetClipboardData + 写临时文件」
/// 的纯 FFI + 文件 IO，无线程亲和性，但**体积可能很大**（一张 4K 截图展开成 BMP
/// 有几十 MB），同步跑在主线程上是一次实打实的卡顿源。
#[tauri::command(async)]
pub fn read_clipboard_backup_image() -> String {
    #[cfg(not(target_os = "windows"))]
    { String::new() }

    #[cfg(target_os = "windows")]
    {
        // DIB (BMP/BI_BITFIELDS/BI_JPEG/BI_PNG) first, then registered "PNG".
        clipboard_dib_to_bmp().or_else(clipboard_png_to_file).unwrap_or_default()
    }
}

/// Some apps (Chrome/Edge, WeChat, QQ) place PNG data via the *registered*
/// "PNG" clipboard format without a usable CF_DIB. Read it directly so the
/// OCR entry still shows for those screenshots.
#[cfg(target_os = "windows")]
fn clipboard_png_to_file() -> Option<String> {
    #[link(name = "user32")]
    extern "system" {
        fn RegisterClipboardFormatW(lpszFormat: *const u16) -> u32;
        fn OpenClipboard(hWndNewOwner: isize) -> i32;
        fn CloseClipboard() -> i32;
        fn GetClipboardData(uFormat: u32) -> isize;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalLock(hMem: isize) -> isize;
        fn GlobalUnlock(hMem: isize) -> i32;
        fn GlobalSize(hMem: isize) -> usize;
    }

    struct CloseClipboardOnDrop;
    impl Drop for CloseClipboardOnDrop {
        fn drop(&mut self) { unsafe { CloseClipboard(); } }
    }

    unsafe {
        const PNG_NAME: &[u16] = &[0x50, 0x4e, 0x47, 0x00]; // L"PNG"
        let cf_png = RegisterClipboardFormatW(PNG_NAME.as_ptr());
        if cf_png == 0 { return None; }
        if OpenClipboard(0) == 0 { return None; }
        let _guard = CloseClipboardOnDrop;

        let h = GetClipboardData(cf_png);
        if h == 0 { return None; }
        let size = GlobalSize(h);
        if size < 8 { return None; } // PNG signature is 8 bytes
        let ptr = GlobalLock(h);
        if ptr == 0 { return None; }
        let bytes = std::slice::from_raw_parts(ptr as *const u8, size);
        // Validate PNG signature before trusting the payload
        if bytes.len() < 8 || bytes[0..8] != [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a] {
            GlobalUnlock(h);
            return None;
        }
        let png: Vec<u8> = bytes.to_vec();
        GlobalUnlock(h);

        let fingerprint = dib_fingerprint(&png);
        let temp_path = temp_image_path("clip", "png");
        std::fs::write(&temp_path, &png).ok()?;
        Some(format!("{}|{}", temp_path.to_string_lossy(), fingerprint))
    }
}

#[cfg(target_os = "windows")]
fn clipboard_dib_to_bmp() -> Option<String> {
    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(hWndNewOwner: isize) -> i32;
        fn CloseClipboard() -> i32;
        fn GetClipboardData(uFormat: u32) -> isize;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalLock(hMem: isize) -> isize;
        fn GlobalUnlock(hMem: isize) -> i32;
        fn GlobalSize(hMem: isize) -> usize;
    }

    const CF_DIB: u32 = 8;
    const CF_DIBV5: u32 = 17;

    // ── RAII guard: CloseClipboard() ALWAYS runs, even on early returns ──
    // A leaked clipboard lock blocks every other app from copying (and, in
    // pathological cases, stalls clipboard-owner message pumps). The unsafe
    // block below returns early on every validation failure — the guard
    // guarantees we never leave the clipboard open.
    struct CloseClipboardOnDrop;
    impl Drop for CloseClipboardOnDrop {
        fn drop(&mut self) {
            unsafe { CloseClipboard(); }
        }
    }

    unsafe {
        // 1. Open clipboard
        if OpenClipboard(0) == 0 { return None; }
        let _guard = CloseClipboardOnDrop;

        // 2. Try CF_DIBV5 first, then CF_DIB
        let h_dibv5 = GetClipboardData(CF_DIBV5);
        let h_dib = GetClipboardData(CF_DIB);
        let h_used = if h_dibv5 != 0 { h_dibv5 } else { h_dib };
        if h_used == 0 { return None; }

        // 3. Lock and validate BEFORE any bulk copy. GlobalSize returns the
        //    allocation size of the HGLOBAL; a broken handle (or an owner that
        //    lied about the format) must never feed an out-of-bounds read.
        let size = GlobalSize(h_used);
        // DIB header is at least 40 bytes (BITMAPINFOHEADER)
        if size < 40 { return None; }
        let ptr = GlobalLock(h_used);
        if ptr == 0 { return None; }
        let bytes = std::slice::from_raw_parts(ptr as *const u8, size);

        // Header sanity check — reject obviously corrupt DIBs.
        // biSize must be >= 40 and <= remaining buffer.
        let bi_size = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let bi_compression = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let bi_bit_count = u16::from_le_bytes([bytes[14], bytes[15]]);
        if bi_size < 40 || bi_size > size {
            GlobalUnlock(h_used);
            return None;
        }
        // Compression must be one of the values the pipeline understands.
        // BI_RGB=0, BI_RLE8=1, BI_RLE4=2, BI_BITFIELDS=3, BI_JPEG=4, BI_PNG=5
        if bi_compression > 5 {
            GlobalUnlock(h_used);
            return None;
        }
        // biBitCount: 1/4/8/16/24/32 are legal DIB values.
        if !matches!(bi_bit_count, 1 | 4 | 8 | 16 | 24 | 32) {
            GlobalUnlock(h_used);
            return None;
        }

        // 4. Copy the validated region to an owned Vec, then unlock.
        let dib: Vec<u8> = bytes.to_vec();
        GlobalUnlock(h_used);

        // 4a. Compute content fingerprint for JS-side dedup
        let fingerprint = dib_fingerprint(&dib);

        // 4b. BI_JPEG (4) / BI_PNG (5): the DIB pixel payload IS a JPEG/PNG
        //     file stream. Wrapping it in a BMP header would corrupt it —
        //     save the raw encoded data with the right extension instead.
        //     (Chrome/Edge and several screenshot tools place PNG/JPEG this way.)
        if bi_compression == 4 || bi_compression == 5 {
            let ext = if bi_compression == 4 { "jpg" } else { "png" };
            let payload_start = bi_size as usize;
            if payload_start >= dib.len() { return None; }
            let payload: Vec<u8> = dib[payload_start..].to_vec();
            let temp_path = temp_image_path("clip", ext);
            std::fs::write(&temp_path, &payload).ok()?;
            return Some(format!("{}|{}", temp_path.to_string_lossy(), fingerprint));
        }

        // 5. Calculate bfOffBits: BMP header + DIB header size (incl. masks for BI_BITFIELDS)
        let mut dib_header_size: u32 = bi_size as u32;
        if bi_compression == 3 { dib_header_size += 12; }
        else if bi_bit_count <= 8 { dib_header_size += (1u32 << bi_bit_count) * 4; }
        let pixel_offset: u32 = 14 + dib_header_size;
        let file_size: u32 = (14 + size) as u32;

        // 6. Build BMP: BITMAPFILEHEADER(14) + DIB
        let mut bmp_data = Vec::with_capacity(file_size as usize);
        bmp_data.extend_from_slice(b"BM");
        bmp_data.extend_from_slice(&file_size.to_le_bytes());
        bmp_data.extend_from_slice(&0u32.to_le_bytes());
        bmp_data.extend_from_slice(&pixel_offset.to_le_bytes());
        bmp_data.extend_from_slice(&dib);

        // 7. Save
        let temp_path = temp_image_path("clip", "bmp");
        std::fs::write(&temp_path, &bmp_data).ok()?;

        // Return path|fingerprint for JS dedup
        Some(format!("{}|{}", temp_path.to_string_lossy(), fingerprint))
    }
}

/// Simple FNV-1a hash of first 256 DIB bytes + total size → hex string for dedup.
#[cfg(target_os = "windows")]
fn dib_fingerprint(dib: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    let sample = &dib[..dib.len().min(256)];
    for &b in sample {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= dib.len() as u64;
    format!("{:016x}", h)
}

// ── VSCode 依附 ───────────────────────────────────────────────
/// Launch VSCode in the current or home directory.
/// Also attempts to install the Lunac VSCode extension if the .vsix is
/// bundled alongside the executable (release/Lunac/resources/lunac.vsix).
/// Called from the AI detached window's "Attach to VSCode" button.
#[tauri::command]
pub fn open_in_vscode() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        let dir = std::env::current_dir()
            .unwrap_or_else(|_| dirs_current_user_home());

        // Find the VSCode "code" executable — try multiple known paths
        let code_path = find_vscode();
        let code = code_path.as_deref().unwrap_or("code");

        // Try to install the Lunac VSCode extension if bundled .vsix exists
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));
        if let Some(base) = &exe_dir {
            // Check: exe_dir/resources/  (Tauri bundle) or exe_dir/ (NSIS flat)
            for candidates in &[
                base.join("resources").join("lunac.vsix"),
                base.join("lunac.vsix"),
            ] {
                if candidates.exists() {
                    let _ = Command::new("cmd")
                        .args(["/c", code, "--install-extension", &candidates.to_string_lossy()])
                        .creation_flags(0x0800_0000)
                        .spawn();
                    break;
                }
            }
        }

        // Launch VSCode with the workspace directory
        Command::new("cmd")
            .args(["/c", code, "."])
            .current_dir(&dir)
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| format!(
                "VSCode not found. Install from https://code.visualstudio.com\nPath tried: {} (error: {})",
                code, e
            ))?;

        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("VSCode launch not supported on this platform".into())
    }
}

#[cfg(target_os = "windows")]
fn find_vscode() -> Option<String> {
    // Priority order: PATH → system-wide → user install
    let candidates = [
        "code",
        r"C:\Program Files\Microsoft VS Code\bin\code.cmd",
        r"C:\Program Files (x86)\Microsoft VS Code\bin\code.cmd",
    ];

    for c in &candidates {
        use std::os::windows::process::CommandExt;
        if std::process::Command::new("cmd")
            .args(["/c", "where", c])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW — 探测命令不弹 cmd 窗口
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Some(c.to_string());
        }
    }

    // Try LOCALAPPDATA user install
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let user_code = format!("{}\\Programs\\Microsoft VS Code\\bin\\code.cmd", local);
        if std::path::Path::new(&user_code).exists() {
            return Some(user_code);
        }
    }

    None
}

/// Open a specific file in VSCode at an optional line number.
/// Available to Agent mode tools via IPC.
#[tauri::command]
pub fn open_file_in_vscode(path: String, line: Option<u32>) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        let code = find_vscode().unwrap_or_else(|| "code".to_string());
        let target = if let Some(l) = line {
            format!("{}:{}", path, l)
        } else {
            path
        };

        Command::new("cmd")
            .args(["/c", &code, "--goto", &target])
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| format!("Failed to open file in VSCode: {}", e))?;

        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (path, line);
        Err("Not supported on this platform".into())
    }
}

fn dirs_current_user_home() -> std::path::PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// 未配置工作区时 agent 的**默认工作目录**：`<exe 根>\temp\transStorage`。
///
/// 为什么不再回退到用户主目录（2026-09-17 用户要求）：agent 的相对路径以 cwd 为基准，
/// 于是模型写的临时脚本会直接堆在 `C:\Users\<用户名>` **根下** —— 实测一次 docx 任务
/// 就留下了 `_tmp_dump_docx.py` / `_tmp_fmt_docx.py` / `_tmp_test_docx.py` / `_tmp_imgs.py`
/// 四个文件，用户主目录被当成草稿本用。改到应用自己的数据根下，与
/// `temp\logs` / `temp\tool-outputs` / `temp\webview-data` 同级，**卸载时随目录一起清掉**。
///
/// - `<exe 根>` 走 `storage::lunac_root_dir()` ⇒ dev 落 `target\debug\temp\transStorage`、
///   release 落安装目录（如 `D:\Lunac\temp\transStorage`），两个环境天然隔离。
/// - **必须在这里建目录**：`spawn_child()` 用 `Command::current_dir(cwd)`，目录不存在时
///   spawn 直接失败 ⇒ agent 起不来、整个 AI 面板不可用（所以不能"用到再建"）。
/// - 建不出来（权限等）时**回退用户主目录**：宁可文件仍写到主目录，也不能让 AI 起不来。
///
/// 全系统访问权**不受影响**：workspace 为空时仍然不设 `LUNAC_WORKSPACE_LOCKED`
/// （见 `start_cli` 里的说明），读写工作目录之外的文件照旧弹审批卡而不是被拒。
fn default_work_dir() -> std::path::PathBuf {
    let dir = crate::storage::lunac_root_dir()
        .join("temp")
        .join("transStorage");
    match std::fs::create_dir_all(&dir) {
        Ok(()) => dir,
        Err(e) => {
            let fallback = dirs_current_user_home();
            crate::log::warn(format!(
                "默认工作目录创建失败：{}（{}），回退到 {}",
                dir.display(),
                e,
                fallback.display()
            ));
            fallback
        }
    }
}

// ── 系统语言检测 ──────────────────────────────────────────────
/// Returns a BCP-47 language tag matching the Windows display language.
/// Used by the frontend to initialize the UI language on startup.
#[tauri::command]
pub fn get_system_language() -> String {
    #[cfg(target_os = "windows")]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetUserDefaultUILanguage() -> u16;
        }
        unsafe { lang_id_to_tag(GetUserDefaultUILanguage()) }
    }
    #[cfg(not(target_os = "windows"))]
    {
        "en".into()
    }
}

#[cfg(target_os = "windows")]
fn lang_id_to_tag(lang_id: u16) -> String {
    match lang_id {
        0x0804 => "zh-CN".into(),  // Chinese (Simplified, PRC)
        0x0404 => "zh-TW".into(),  // Chinese (Traditional, Taiwan)
        0x0c04 | 0x1404 | 0x1004 => "zh-HK".into(), // Chinese (HK/Macau/Singapore)
        0x0409 | 0x0809 | 0x0c09 | 0x1009 | 0x1409 | 0x1809 | 0x1c09 | 0x2009 | 0x2409 | 0x2809 | 0x2c09 | 0x3009 | 0x3409 => "en".into(), // English (all variants)
        0x0411 => "ja".into(),      // Japanese
        0x0412 => "ko".into(),      // Korean
        0x0407 => "de".into(),      // German
        0x040c => "fr".into(),      // French
        0x0410 => "it".into(),      // Italian
        0x0c0a => "es".into(),      // Spanish
        0x0416 => "pt-BR".into(),   // Portuguese (Brazil)
        0x0419 => "ru".into(),      // Russian
        0x0401 => "ar".into(),      // Arabic
        0x041d => "sv".into(),      // Swedish
        0x041f => "tr".into(),      // Turkish
        0x0413 => "nl".into(),      // Dutch
        0x0414 => "nb".into(),      // Norwegian
        0x0415 => "pl".into(),      // Polish
        0x0405 => "cs".into(),      // Czech
        0x040e => "hu".into(),      // Hungarian
        0x040b => "fi".into(),      // Finnish
        0x0406 => "da".into(),      // Danish
        0x0408 => "el".into(),      // Greek
        0x040d => "he".into(),      // Hebrew
        0x0421 => "id".into(),      // Indonesian
        0x041a => "hr".into(),      // Croatian
        0x0418 => "ro".into(),      // Romanian
        0x041b => "sk".into(),      // Slovak
        0x0424 => "sl".into(),      // Slovenian
        0x0422 => "uk".into(),      // Ukrainian
        0x0425 => "et".into(),      // Estonian
        0x0426 => "lv".into(),      // Latvian
        0x0427 => "lt".into(),      // Lithuanian
        0x0429 => "fa".into(),      // Persian
        0x041e => "th".into(),      // Thai
        0x042a => "vi".into(),      // Vietnamese
        _ => "en".into(),           // Fallback to English
    }
}
