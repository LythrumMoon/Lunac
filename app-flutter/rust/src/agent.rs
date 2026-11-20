//! M2 对话主链路：拉起 `agent.exe`（core-agent）并用 stream-json 通信。
//!
//! 协议与 v1 **完全一致**（docs/flutter-migration.md §一 规则 2：不改 agent 协议）：
//!   · 参数：`--print --verbose --input-format stream-json --output-format stream-json
//!     --include-partial-messages --permission-prompt-tool stdio --permission-mode acceptEdits
//!     --add-dir <cwd> .`
//!     （`--print` 与 `--input-format stream-json` 同时存在时仍是常驻模式）；
//!   · stdin：每行一条 JSON（`user` / `control_response` / `set_history` / `pause` …）；
//!   · stdout：每行一条 JSON 事件（`system/*` / `stream_event` / `assistant` / `user` /
//!     `tool_output` / `usage_delta` / `result` / `control_request` / `control_cancel_request`）；
//!     非 `{` 开头的调试行丢弃；
//!   · 凭据与端点来自 `<exe 根>/config/ai.json`（与 v1 同一份），注入
//!     `LUNAC_AGENT_BASE_URL` / `LUNAC_AGENT_TOKEN` / `LUNAC_AGENT_MODEL`。
//!
//! 与 Dart 的边界：本文件只做进程与行缓冲；事件语义（文本增量 / 工具卡 / 审批卡）由
//! Dart 侧的 `chat_model.dart` 解释。Dart 用 `lunac_agent_poll` 取事件（20–50ms 一次，
//! 队列空时只是一次 Mutex 读，不产生 IO）。

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Mutex, OnceLock};

/// agent 可执行文件名（与 v1 `AGENT_EXE` 一致）。
const AGENT_EXE: &str = "agent.exe";

/// 事件队列上限：Dart 侧极端卡顿时丢最旧的（聊天记录另有 UI 状态，不会因此丢内容）。
const MAX_QUEUED_EVENTS: usize = 4000;

/// AI 配置（`<exe 根>/config/ai.json`，与 v1 `storage::AiConfig` 字段同名）。
#[derive(Debug, Clone, Default)]
pub struct AiConfig {
    pub provider: String,
    pub url: String,
    pub key: String,
    pub model: String,
    pub agent_url: String,
    pub search_provider: String,
    pub search_key: String,
}

impl AiConfig {
    /// 真正喂给 agent 的端点：`agent_url` 显式优先，否则去掉末尾 `/v1` 再拼 `/anthropic`。
    pub fn endpoint(&self) -> String {
        agent_endpoint(&self.url, &self.agent_url)
    }

    /// 配置是否可用（key 与端点都必须有；model 缺省 deepseek-flash）。
    pub fn validate(&self) -> Result<(), String> {
        if self.key.trim().is_empty() {
            return Err("AI 未配置：config/ai.json 缺少 key".to_string());
        }
        if self.endpoint().trim().is_empty() {
            return Err("AI 未配置：config/ai.json 缺少 url".to_string());
        }
        Ok(())
    }
}

struct AgentState {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    events: VecDeque<String>,
}

static STATE: OnceLock<Mutex<AgentState>> = OnceLock::new();

fn state() -> &'static Mutex<AgentState> {
    STATE.get_or_init(|| {
        Mutex::new(AgentState {
            child: None,
            stdin: None,
            events: VecDeque::new(),
        })
    })
}

/// agent 是否在跑（已退出 / 未启动都算 false；顺手回收僵尸进程）。
pub fn running() -> bool {
    let Ok(mut guard) = state().lock() else {
        return false;
    };
    match guard.child.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(None) => true,
            _ => {
                guard.child = None;
                guard.stdin = None;
                false
            }
        },
        None => false,
    }
}

/// 定位 `agent.exe`：环境变量（显式覆盖）→ 安装目录 / 资源目录 / 仓库开发产物里**最新**的那个。
///
/// 2026-11-15 改口径：以前是「安装目录优先、仓库产物 release 优先」，于是两份旧产物
/// （安装目录里手工拷的副本、`target/release` 停在几个月前）会静默顶掉刚构建好的
/// `target/debug/agent.exe` —— 用户看到的就是「明明修过，行为还是旧的」。
/// 现在改为**按 mtime 取最新**（`LUNAC_AGENT_EXE` 仍然无条件优先）。
pub fn find_agent_exe(root: &Path) -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("LUNAC_AGENT_EXE") {
        let path = PathBuf::from(explicit.trim());
        if path.is_file() {
            return Some(path);
        }
    }
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut consider = |candidate: PathBuf| {
        if !candidate.is_file() {
            return;
        }
        let stamp = std::fs::metadata(&candidate)
            .and_then(|meta| meta.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        if best.as_ref().map(|(old, _)| stamp > *old).unwrap_or(true) {
            best = Some((stamp, candidate));
        }
    };
    consider(root.join(AGENT_EXE));
    consider(root.join("resources").join(AGENT_EXE));
    // 开发布局：宿主在 `<repo>/app-flutter/build/windows/x64/runner/Debug`，
    // 向上最多 16 级找 `<repo>/core-agent/target/{release,debug}/agent.exe`。
    let mut cursor = Some(root);
    for _ in 0..16 {
        let Some(dir) = cursor else {
            break;
        };
        for profile in ["release", "debug"] {
            consider(
                dir.join("core-agent")
                    .join("target")
                    .join(profile)
                    .join(AGENT_EXE),
            );
        }
        cursor = dir.parent();
    }
    best.map(|(_, picked)| picked)
}

/// 配置文件候选路径（按优先级）：`LUNAC_AI_CONFIG` → `<exe 根>/config/ai.json` →
/// 逐级向上的 `config/ai.json`（开发时可直接把 v1 的配置放仓库根）。
fn config_candidates(root: &Path) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(explicit) = std::env::var("LUNAC_AI_CONFIG") {
        if !explicit.trim().is_empty() {
            candidates.push(PathBuf::from(explicit.trim()));
        }
    }
    candidates.push(root.join("config").join("ai.json"));
    let mut cursor = Some(root);
    for _ in 0..16 {
        let Some(dir) = cursor else {
            break;
        };
        let candidate = dir.join("config").join("ai.json");
        if !candidates.iter().any(|c| c == &candidate) {
            candidates.push(candidate);
        }
        cursor = dir.parent();
    }
    candidates
}

/// 保存时写到哪里：`LUNAC_AI_CONFIG`（显式覆盖）否则 `<exe 根>/config/ai.json`。
fn write_config_path(root: &Path) -> PathBuf {
    if let Ok(explicit) = std::env::var("LUNAC_AI_CONFIG") {
        if !explicit.trim().is_empty() {
            return PathBuf::from(explicit.trim());
        }
    }
    root.join("config").join("ai.json")
}

/// 读原始配置（第一个存在的合法 JSON）；没有 / 全坏时返回 `{}`。
pub fn config_value(root: &Path) -> serde_json::Value {
    for path in config_candidates(root) {
        if !path.is_file() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            return value;
        }
    }
    serde_json::Value::Object(serde_json::Map::new())
}

/// 读 AI 配置：`LUNAC_AI_CONFIG` → `<exe 根>/config/ai.json` → 逐级向上的 `config/ai.json`。
pub fn load_config(root: &Path) -> Result<(AiConfig, PathBuf), String> {
    for path in config_candidates(root) {
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("读取 AI 配置失败（{}）: {e}", path.display()))?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("AI 配置不是合法 JSON（{}）: {e}", path.display()))?;
        let read = |key: &str| -> String {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let mut config = AiConfig {
            provider: read("provider"),
            url: read("url"),
            key: read("key"),
            model: read("model"),
            agent_url: read("agent_url"),
            search_provider: read("search_provider"),
            search_key: read("search_key"),
        };
        if config.model.is_empty() {
            config.model = "deepseek-flash".to_string();
        }
        config.validate()?;
        return Ok((config, path));
    }
    Err("AI 未配置：找不到 config/ai.json（可在「设置 → AI 模型」里直接填写）".to_string())
}

/// 保存 AI 配置（设置面板用）：与现有文件**合并**，只覆盖传入的字段，
/// 不丢掉 `search_provider` / `image_*` 等 v1 写的其它键。
pub fn save_config(root: &Path, update: &serde_json::Value) -> Result<(), String> {
    let Some(patch) = update.as_object() else {
        return Err("AI 配置必须是 JSON 对象".to_string());
    };
    let path = write_config_path(root);
    // 以「当前生效的配置」为底（可能是祖先目录里 v1 的那份），再叠加本次修改。
    let mut base = config_value(root);
    if !base.is_object() {
        base = serde_json::Value::Object(serde_json::Map::new());
    }
    let object = base.as_object_mut().expect("object ensured above");
    for (key, value) in patch {
        object.insert(key.clone(), value.clone());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建配置目录失败（{}）: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&base)
        .map_err(|e| format!("序列化 AI 配置失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写配置失败（{}）: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("替换配置失败（{}）: {e}", path.display()))?;
    Ok(())
}

/// `api_url` → agent 端点（与 v1 `commands::agent_endpoint` 逐字一致）。
pub fn agent_endpoint(api_url: &str, explicit: &str) -> String {
    let explicit = explicit.trim();
    if !explicit.is_empty() {
        return explicit.to_string();
    }
    let base = api_url.trim().trim_end_matches('/');
    let root = match base.strip_suffix("/v1") {
        Some(rest) if rest.contains("://") => rest,
        _ => base,
    };
    if root.is_empty() {
        return String::new();
    }
    format!("{root}/anthropic")
}

/// 常驻进程与 cron 单发共用的启动信息。
///
/// 为什么要有这一层：定时任务要拉**同一套** env（端点 / key / 模型 / 技能目录 / Modules /
/// cron 目录 / 人格 / 日志），如果各自复制一份，迟早会漂（仓储纪律：同类逻辑只留一处）。
pub struct LaunchSpec {
    pub exe: PathBuf,
    pub workdir: PathBuf,
    pub prefs: crate::chat_settings::ChatPrefs,
    pub config_path: PathBuf,
    pub model: String,
    pub env: Vec<(&'static str, String)>,
}

/// 组装 agent 子进程的工作目录与环境变量（启动失败的原因原样返回）。
pub fn launch_spec(root: &Path) -> Result<LaunchSpec, String> {
    let exe = find_agent_exe(root)
        .ok_or_else(|| "找不到 agent.exe（先运行 scripts\\build-core.ps1 构建）".to_string())?;
    let (config, config_path) = load_config(root)?;
    config.validate()?;

    // 「更多设置」的持久化值（v1 的 localStorage 在 v2 落 `config/chat.json`）。
    let prefs = crate::chat_settings::load(root);
    // 工作目录：用户选过工作区就用它（并启用工作区锁），否则用 `<exe 根>/temp/transStorage`
    // （与 v1 `default_work_dir()` 一致）。`sandbox::prepare` 仍要跑一次 —— 沙箱预览窗
    // 统计的是那块盘，两者互不影响。
    let _ = crate::sandbox::prepare(root)?;
    let (workdir, workspace_locked) = crate::chat_settings::resolve_workspace(root, &prefs);

    let mut env: Vec<(&'static str, String)> = Vec::new();
    // 工作区锁（v1 硬边界）：**只有配置过工作区才注入**。agent 侧 `locked` 还要与
    // `--dangerously-skip-permissions`（「系统」档）取与 ⇒ 三档行为与 v1 逐条一致。
    if workspace_locked {
        env.push(("LUNAC_WORKSPACE_LOCKED", "1".into()));
    }
    // 凭据 / 端点（v1 `configure_agent_env` 的三个必填项）。
    env.push(("LUNAC_AGENT_BASE_URL", config.endpoint()));
    env.push(("LUNAC_AGENT_TOKEN", config.key.clone()));
    env.push(("LUNAC_AGENT_MODEL", config.model.clone()));
    // 思考开关（v1 `set_thinking_mode`）：core-agent **只认 `off`**，其余值都算开。
    env.push((
        "LUNAC_THINKING",
        if prefs.thinking { "on" } else { "off" }.to_string(),
    ));
    // 第二模型预审（§待办 3-28.14）：agent 启动时读一次；开关关 ⇒ 不产生任何请求。
    // 端点由 `LUNAC_SECOND_REVIEW_URL` / `LUNAC_LOCAL_MODEL_URL` 决定（agent 侧解析）。
    env.push((
        "LUNAC_SECOND_REVIEW",
        if prefs.second_review { "on" } else { "off" }.to_string(),
    ));
    // 用户人格 / 自定义提示词：无条件给路径（文件不存在 = 只用内置人格）。
    // agent 启动时读一次，所以面板保存后要重启 agent 才生效（v1 同语义）。
    env.push((
        "LUNAC_PERSONA_FILE",
        crate::chat_settings::persona_path(root).display().to_string(),
    ));
    // 技能级记忆（§待办 3-28.10 ①）：开关 + 落点目录。关着时 agent 不读也不写技能命名空间
    // （`Remember` 照旧写本轮作用域那一份），行为与改造前逐字节一致。
    env.push((
        "LUNAC_SKILL_MEMORY",
        if prefs.skill_memory { "on" } else { "off" }.to_string(),
    ));
    env.push((
        "LUNAC_MEMORY_DIR",
        root.join("ModuleData").join("memory").display().to_string(),
    ));
    // PTC / Code Mode（§待办 3-28.15）：关着时 `RunScript` 不进工具表。
    env.push((
        "LUNAC_PTC",
        if prefs.ptc_mode { "on" } else { "off" }.to_string(),
    ));
    if !config.provider.is_empty() {
        env.push(("LUNAC_AI_PROVIDER", config.provider.to_lowercase()));
    }
    if !config.search_provider.is_empty() {
        env.push((
            "LUNAC_SEARCH_PROVIDER",
            config.search_provider.to_lowercase(),
        ));
    }
    if !config.search_key.is_empty() {
        env.push(("LUNAC_SEARCH_KEY", config.search_key.clone()));
    }
    // 目录类变量（与 v1 `agent_envs` 同路径口径；目录不存在时 agent 自己按空处理）。
    let tasks = root.join("ModuleData").join("tasks");
    let _ = std::fs::create_dir_all(&tasks);
    env.push((
        "LUNAC_SKILLS_DIR",
        root.join("skills").display().to_string(),
    ));
    env.push((
        "LUNAC_MODULES_DIR",
        root.join("Modules").display().to_string(),
    ));
    env.push(("LUNAC_CRON_DIR", tasks.display().to_string()));
    env.push(("LUNAC_TASKS_DIR", tasks.display().to_string()));
    env.push((
        "LUNAC_LOG_DIR",
        root.join("temp").join("logs").display().to_string(),
    ));

    Ok(LaunchSpec {
        exe,
        workdir,
        prefs,
        config_path,
        model: config.model,
        env,
    })
}

/// 启动常驻 agent（幂等：已在运行直接返回 Ok）。
pub fn start(root: &Path) -> Result<(), String> {
    if running() {
        return Ok(());
    }
    let spec = launch_spec(root)?;

    let mut args: Vec<String> = vec![
        "--print".into(),
        "--verbose".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--include-partial-messages".into(),
        "--permission-prompt-tool".into(),
        "stdio".into(),
    ];
    // 权限档（文件边界）与 v1 `cli_args_for_profile` 逐条一致：
    //   safe → plan（只读）；project → acceptEdits + 工作目录；full → 跳过审批 + 工作目录。
    match spec.prefs.profile.as_str() {
        "safe" => {
            args.push("--permission-mode".into());
            args.push("plan".into());
        }
        "full" => {
            args.push("--dangerously-skip-permissions".into());
            args.push("--add-dir".into());
            args.push(spec.workdir.display().to_string());
        }
        _ => {
            args.push("--permission-mode".into());
            args.push("acceptEdits".into());
            args.push("--add-dir".into());
            args.push(spec.workdir.display().to_string());
        }
    }
    if !spec.prefs.tool_blacklist.is_empty() {
        args.push("--disallowedTools".into());
        args.extend(spec.prefs.tool_blacklist.iter().cloned());
    }
    // 宿主 MCP 桥：agent 以 `<本 exe> --mcp-server` 拉起宿主进程，长期记忆 / 往期会话 /
    // 桌宠成长笔记等桥工具都走这条 stdio 通道（v1 `mcp_server.rs` 同协议）。
    //
    // 主程序自己就是 MCP 服务端（`main.dart` 在 Flutter 初始化**之前**早退进这条分支）。
    // 拿不到 current_exe 就不传 —— 少桥只少这些工具，对话本身不受影响。
    if let Ok(host) = std::env::current_exe() {
        args.push("--mcp-server".into());
        args.push(format!("stdio:{}", host.display()));
    }
    args.push(".".into());

    let mut cmd = Command::new(&spec.exe);
    cmd.args(&args)
        .current_dir(&spec.workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：不弹控制台窗口。
        cmd.creation_flags(0x0800_0000);
    }
    for (key, value) in &spec.env {
        cmd.env(key, value);
    }

    eprintln!(
        "[lunac-agent] spawn {} (config={}, model={}, cwd={})",
        spec.exe.display(),
        spec.config_path.display(),
        spec.model,
        spec.workdir.display()
    );
    crate::log::info(format!(
        "agent spawn {} (config={}, model={}, cwd={})",
        spec.exe.display(),
        spec.config_path.display(),
        spec.model,
        spec.workdir.display()
    ));

    let mut child = cmd.spawn().map_err(|e| {
        // release 没有控制台 ⇒ 这条必须落盘，否则「点了没反应」永远查不到原因。
        crate::log::error(format!("agent.exe 启动失败：{e}"));
        format!("启动 agent.exe 失败: {e}")
    })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "agent.exe 没有可写的 stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "agent.exe 没有可读的 stdout".to_string())?;
    let stderr = child.stderr.take();

    // stdout：逐行入队（只收 JSON 行；错误 / 调试输出走 stderr）。
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if !trimmed.starts_with('{') {
                continue;
            }
            if let Ok(mut guard) = state().lock() {
                if guard.events.len() >= MAX_QUEUED_EVENTS {
                    guard.events.pop_front();
                }
                guard.events.push_back(trimmed.to_string());
            }
            // VSCode 扩展的 SSE 桥（v1 `cli_bridge::broadcast_output` 的等价物）：
            // 与 Dart 的 `lunac_agent_poll` 队列**并行**，两边互不抢占。
            crate::vscode_bridge::broadcast(trimmed);
        }
    });
    // stderr：只进宿主日志，不进协议。
    if let Some(stderr) = stderr {
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                eprintln!("[lunac-agent] {line}");
                crate::log::warn(format!("[agent] {line}"));
            }
        });
    }

    let mut guard = state()
        .lock()
        .map_err(|_| "agent 状态锁损坏".to_string())?;
    // 上一轮残留的事件不能带进新会话。
    guard.events.clear();
    guard.child = Some(child);
    guard.stdin = Some(stdin);
    Ok(())
}

/// 往 agent 的 stdin 写一条 JSON（自动补换行并 flush）。
pub fn send(line: &str) -> Result<(), String> {
    let mut guard = state().lock().map_err(|_| "agent 状态锁损坏".to_string())?;
    let Some(stdin) = guard.stdin.as_mut() else {
        return Err("agent 未启动".to_string());
    };
    if let Err(e) = writeln!(stdin, "{}", line.trim_end()).and_then(|_| stdin.flush()) {
        guard.stdin = None;
        return Err(format!("写入 agent 失败: {e}"));
    }
    Ok(())
}

/// 取走队首一条事件（没有则 None）。Dart 侧轮询用。
pub fn poll() -> Option<String> {
    state().lock().ok()?.events.pop_front()
}

/// 停掉 agent（退出应用 / 重启对话时调用）。
pub fn stop() {
    let Ok(mut guard) = state().lock() else {
        return;
    };
    guard.stdin = None;
    if let Some(mut child) = guard.child.take() {
        crate::log::info(format!("agent stop pid={}", child.id()));
        let _ = child.kill();
        let _ = child.wait();
    }
    guard.events.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lunac-agent-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn endpoint_strips_v1_and_appends_anthropic() {
        assert_eq!(
            agent_endpoint("https://api.deepseek.com/v1", ""),
            "https://api.deepseek.com/anthropic"
        );
        assert_eq!(
            agent_endpoint("https://api.deepseek.com", ""),
            "https://api.deepseek.com/anthropic"
        );
        assert_eq!(
            agent_endpoint("https://x.test/v1/", ""),
            "https://x.test/anthropic"
        );
        // 显式 agent_url 优先。
        assert_eq!(
            agent_endpoint("https://ignored", " https://explicit.test/anthropic "),
            "https://explicit.test/anthropic"
        );
        // 空 url 不会拼出 "/anthropic"。
        assert_eq!(agent_endpoint("", ""), "");
    }

    #[test]
    fn config_is_read_from_exe_root_and_validated() {
        let dir = temp_dir("config");
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::write(
            dir.join("config").join("ai.json"),
            r#"{"provider":"deepseek","url":"https://api.deepseek.com/v1","key":"sk-test","model":""}"#,
        )
        .unwrap();
        let (config, path) = load_config(&dir).expect("config must load");
        assert_eq!(config.provider, "deepseek");
        assert_eq!(config.model, "deepseek-flash", "空 model 回落默认");
        assert_eq!(config.endpoint(), "https://api.deepseek.com/anthropic");
        assert!(path.ends_with("config/ai.json"));

        // 缺 key 时给出可读原因，而不是启动一个必然 401 的 agent。
        std::fs::write(
            dir.join("config").join("ai.json"),
            r#"{"url":"https://api.deepseek.com","key":""}"#,
        )
        .unwrap();
        assert!(load_config(&dir).unwrap_err().contains("key"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_config_merges_and_preserves_unknown_fields() {
        let dir = temp_dir("save");
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::write(
            dir.join("config").join("ai.json"),
            r#"{"provider":"old","key":"sk-old","image_model":"img","search_key":"s"}"#,
        )
        .unwrap();

        save_config(
            &dir,
            &serde_json::json!({
                "provider": "deepseek",
                "url": "https://api.deepseek.com",
                "key": "sk-new",
                "model": "deepseek-chat",
                "vision": true,
            }),
        )
        .expect("save must succeed");

        let raw = std::fs::read_to_string(dir.join("config").join("ai.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["provider"], "deepseek");
        assert_eq!(value["key"], "sk-new");
        assert_eq!(value["image_model"], "img", "未编辑的字段必须原样保留");
        assert_eq!(value["search_key"], "s");
        assert_eq!(value["vision"], true);

        let (config, _) = load_config(&dir).expect("saved config must load");
        assert_eq!(config.model, "deepseek-chat");
        assert_eq!(config.endpoint(), "https://api.deepseek.com/anthropic");

        // 非对象输入被拒绝，不写坏文件。
        assert!(save_config(&dir, &serde_json::json!([1, 2])).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 取「最新」的那份：安装目录里的旧副本 / target/release 的旧产物都不许顶掉新构建。
    #[test]
    fn agent_exe_takes_the_freshest_build() {
        let dir = temp_dir("find");
        let root_exe = dir.join(AGENT_EXE);
        std::fs::write(&root_exe, b"stub").unwrap();
        assert_eq!(find_agent_exe(&dir), Some(root_exe.clone()));

        let nested = dir.join("app-flutter").join("build").join("Debug");
        std::fs::create_dir_all(&nested).unwrap();
        let repo_release = dir
            .join("core-agent")
            .join("target")
            .join("release")
            .join(AGENT_EXE);
        std::fs::create_dir_all(repo_release.parent().unwrap()).unwrap();
        std::fs::write(&repo_release, b"stub").unwrap();

        // 刚构建的 debug 比安装目录那份、也比 release 新 ⇒ 用它。
        std::thread::sleep(std::time::Duration::from_millis(20));
        let repo_debug = dir
            .join("core-agent")
            .join("target")
            .join("debug")
            .join(AGENT_EXE);
        std::fs::create_dir_all(repo_debug.parent().unwrap()).unwrap();
        std::fs::write(&repo_debug, b"stub").unwrap();
        assert_eq!(find_agent_exe(&nested), Some(repo_debug));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 实跑：需要 `core-agent/target/{release,debug}/agent.exe`。
    /// 用 `cargo test -- --ignored --nocapture` 手动跑；断言拿到第一行 JSON 事件
    /// （凭据是假的，只验证「spawn + stdout 行队列」这条管道）。
    #[test]
    #[ignore = "需要本机 core-agent 构建产物"]
    fn live_spawn_emits_stream_json() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo = manifest
            .parent()
            .and_then(|p| p.parent())
            .expect("repo root")
            .to_path_buf();
        let agent = ["release", "debug"]
            .iter()
            .map(|profile| {
                repo.join("core-agent")
                    .join("target")
                    .join(profile)
                    .join(AGENT_EXE)
            })
            .find(|path| path.is_file());
        let Some(agent) = agent else {
            eprintln!("skip: agent.exe not built");
            return;
        };

        let root = temp_dir("live");
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config").join("ai.json"),
            r#"{"provider":"test","url":"http://127.0.0.1:9/v1","key":"sk-dummy","model":"test-model"}"#,
        )
        .unwrap();
        std::env::set_var("LUNAC_AGENT_EXE", &agent);
        start(&root).expect("agent must start");
        // init 在「收到第一条用户消息」时才发（与真实 CLI 每轮一致），
        // 所以这里先写一条最小用户消息；端点不可达 ⇒ 至少会回来一条 result。
        send(r#"{"type":"user","session_id":"","message":{"role":"user","content":[{"type":"text","text":"hi"}]},"parent_tool_use_id":null}"#)
            .expect("write to agent stdin");

        let mut line = None;
        for _ in 0..200 {
            if let Some(event) = poll() {
                line = Some(event);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        stop();
        std::env::remove_var("LUNAC_AGENT_EXE");
        let _ = std::fs::remove_dir_all(&root);

        let line = line.expect("expected at least one JSON event");
        assert!(line.contains("\"type\""), "not a protocol event: {line}");
    }
}
