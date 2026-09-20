//! 权限 hooks（原 backlog A9，2026-09-20）
//!
//! 用户在 `config\hooks.json` 里挂自己的脚本，在 8 个事件上介入 agent 的行为：
//! `SessionStart` / `UserPromptSubmit` / `PreToolUse` / `PostToolUse` /
//! `PermissionRequest` / `PreCompact` / `Stop` / `SessionEnd`。
//! 契约（事件口径、payload、退出码与决策 JSON、失败语义）见
//! `docs/ai-spec.md` §3.5「权限 hooks」与 §11 规则 61。
//!
//! 四条贯穿始终的纪律：
//!  · **只认显式拒绝**：只有 hook 明确说「不许」（退出码 2，或 stdout 里
//!    `{"decision":"deny"}`）才拦；超时 / 崩溃 / 输出看不懂一律**放行但可见**
//!    （`kind=error` 的条目会被 main.rs 原样送到前端）—— 静默丢弃是最坏的一种。
//!  · **不放宽两道硬闸**：hook 的 `allow` 只等于「跳过审批卡」。工作区锁与静态安全
//!    分析（危险命令 / 密钥命中）仍原样生效，判据在 main.rs（见 ai-spec 规则 14）。
//!  · **配置改完即时生效**：按文件 mtime 重读；**解析失败保留上一份有效配置**
//!    （配置写坏不该让工具链停摆），失败原因落 agent 日志。
//!  · **同时只有一个 hook 的裁决生效**：多个命中时按配置顺序全部执行，
//!    第一个 `deny` 胜出（deny 优先于 allow）。
//!
//! hooks 的**输入契约**与 Claude Code 同形（`hook_event_name` / `tool_name` /
//! `tool_input` / `tool_use_id` / `cwd`，外加环境变量 `LUNAC_HOOK_EVENT`），
//! 便于用户把已有的 hook 脚本搬过来；**输出契约**只认一个极简形状
//! （`{"decision":"allow"|"deny","reason":…,"additionalContext":…}`），不做第二套方言。

use crate::log;
use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// 支持的 8 个事件 —— 每个都在 core-agent 里有**确切落点**，不空跑。
pub const EVENTS: [&str; 8] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "PreCompact",
    "Stop",
    "SessionEnd",
];

/// 能「拦」的事件。其余事件里退出码 2 只表示「把这段文字交给模型 / 展示给用户」
/// （PostToolUse 的工具已经跑完了，拦不住）。
pub fn can_block(event: &str) -> bool {
    matches!(event, "PreToolUse" | "PermissionRequest" | "UserPromptSubmit")
}

/// 这三个事件的 `matcher` 比对的才是**工具名**，其余事件的 `matcher` 无意义
/// （写了会被当配置警告报出来，不会静默不匹配）。
fn matcher_is_meaningful(event: &str) -> bool {
    matches!(event, "PreToolUse" | "PostToolUse" | "PermissionRequest")
}

/// 单条 hook 的默认超时（秒）。用户可用 `timeout` 覆盖，上限 `MAX_TIMEOUT_SECS`。
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_TIMEOUT_SECS: u64 = 600;
/// 每条提示文本的上限（用户脚本的 stdout 可能很长，回前端与回模型都要有个头）。
const MAX_ITEM_CHARS: usize = 2000;
/// 单条 hook 的 stdout / stderr 读取上限（与 tools.rs 的管道纪律同源：读满即丢）。
const MAX_OUTPUT_BYTES: usize = 512 * 1024;
const POLL_MS: u64 = 25;

// ── 配置 ─────────────────────────────────────────────────────────

struct Cmd {
    command: String,
    timeout: Duration,
}

struct Group {
    event: String,
    /// 只有工具类事件的组才有 matcher；`None` = 全匹配
    matcher: Option<regex::Regex>,
    commands: Vec<Cmd>,
}

/// 一份解析好的 hooks 配置（**不可变快照**，热重载时整份换掉）。
pub struct Hooks {
    enabled: bool,
    groups: Vec<Group>,
    /// 读得出来但用不了的地方（未知事件名 / 正则编译失败 / type 不是 command …）
    pub warnings: Vec<String>,
    /// 读不出或解析失败的原因（此时 `groups` 为空）
    pub error: Option<String>,
}

impl Hooks {
    /// 没有任何可执行的东西 —— 热路径上据此直接返回，连 payload 都不必构造。
    pub fn is_empty(&self) -> bool {
        !self.enabled || self.groups.is_empty()
    }

    /// 供日志：`event 组数` 概览
    pub fn summary(&self) -> String {
        let mut by_event: Vec<String> = Vec::new();
        for e in EVENTS {
            let n: usize = self
                .groups
                .iter()
                .filter(|g| g.event == e)
                .map(|g| g.commands.len())
                .sum();
            if n > 0 {
                by_event.push(format!("{e}×{n}"));
            }
        }
        format!(
            "{}{}",
            if self.enabled { "" } else { "（已停用）" },
            by_event.join(" ")
        )
    }

    /// 解析 `hooks.json` 的文本。**永不 panic**：任何读不懂的地方都落进
    /// `error` / `warnings`，交给调用方决定怎么呈现。
    pub fn parse(text: &str) -> Hooks {
        // 容忍 UTF-8 BOM：编辑器把 hooks.json 存成「UTF-8 带 BOM」是常事，
        // 而 serde_json 见到 BOM 会判整份配置非法（⇒ hooks 全静默不跑，最难查）。
        let text = text.trim_start_matches('\u{feff}');
        let mut warnings: Vec<String> = Vec::new();
        let fail = |e: String| Hooks {
            enabled: false,
            groups: Vec::new(),
            warnings: Vec::new(),
            error: Some(e),
        };
        let v: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => return fail(format!("hooks.json 不是合法 JSON：{e}")),
        };
        if !v.is_object() {
            return fail("hooks.json 顶层必须是一个对象".into());
        }
        // `enabled` 缺省 = 开：**文件存在本身就表示用户配了 hooks**（Claude Code 没有
        // 这个字段）。设置面板的开关会显式写入该字段，关掉时是 `"enabled": false`。
        let enabled = v.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        let mut groups: Vec<Group> = Vec::new();

        let Some(table) = v.get("hooks").and_then(Value::as_object) else {
            if v.get("hooks").is_some() {
                return fail("hooks.json 的 `hooks` 必须是一个对象".into());
            }
            // 没有 hooks 段 = 空配置（合法，等价于什么都没配）
            return Hooks {
                enabled,
                groups,
                warnings,
                error: None,
            };
        };

        for (event, arr) in table {
            if !EVENTS.contains(&event.as_str()) {
                warnings.push(format!(
                    "不认识的事件 `{event}`（支持：{}）—— 已忽略",
                    EVENTS.join(" / ")
                ));
                continue;
            }
            let Some(list) = arr.as_array() else {
                warnings.push(format!("`{event}` 的值必须是数组 —— 已忽略"));
                continue;
            };
            for (gi, g) in list.iter().enumerate() {
                let matcher = match g
                    .get("matcher")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    None => None,
                    Some(_) if !matcher_is_meaningful(event) => {
                        warnings.push(format!(
                            "`{event}` 不支持 matcher（它比对的才是工具名）—— 该组按全匹配处理"
                        ));
                        None
                    }
                    // 正则编译失败 ⇒ **这一组永不匹配** + 警告。绝不能退回「全匹配」：
                    // 那会把「写错的 matcher」变成「对所有工具生效」。
                    Some(m) => match regex::Regex::new(m) {
                        Ok(re) => Some(re),
                        Err(e) => {
                            warnings.push(format!(
                                "`{event}[{gi}]` 的 matcher 不是合法正则（{m}）：{e} —— 该组已忽略"
                            ));
                            continue;
                        }
                    },
                };
                let mut commands: Vec<Cmd> = Vec::new();
                for (hi, h) in g
                    .get("hooks")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                {
                    let ty = h.get("type").and_then(Value::as_str).unwrap_or("command");
                    if ty != "command" {
                        warnings.push(format!(
                            "`{event}[{gi}].hooks[{hi}]` 只支持 type=\"command\"（收到 {ty}）—— 已跳过"
                        ));
                        continue;
                    }
                    let Some(command) = h
                        .get("command")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    else {
                        warnings.push(format!("`{event}[{gi}].hooks[{hi}]` 缺 command —— 已跳过"));
                        continue;
                    };
                    let secs = h
                        .get("timeout")
                        .and_then(Value::as_u64)
                        .unwrap_or(DEFAULT_TIMEOUT_SECS)
                        .clamp(1, MAX_TIMEOUT_SECS);
                    commands.push(Cmd {
                        command: command.to_string(),
                        timeout: Duration::from_secs(secs),
                    });
                }
                if commands.is_empty() {
                    warnings.push(format!("`{event}[{gi}]` 没有可执行的 command —— 该组已忽略"));
                    continue;
                }
                groups.push(Group {
                    event: event.clone(),
                    matcher,
                    commands,
                });
            }
        }

        Hooks {
            enabled,
            groups,
            warnings,
            error: None,
        }
    }
}

// ── 热重载（按 mtime）─────────────────────────────────────────────

fn config_path() -> Option<PathBuf> {
    let raw = std::env::var("LUNAC_HOOKS_FILE").ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Some(PathBuf::from(raw))
}

fn disabled() -> Arc<Hooks> {
    static D: OnceLock<Arc<Hooks>> = OnceLock::new();
    D.get_or_init(|| {
        Arc::new(Hooks {
            enabled: false,
            groups: Vec::new(),
            warnings: Vec::new(),
            error: None,
        })
    })
    .clone()
}

struct Cache {
    /// 上一次读盘时的 mtime（`None` = 文件当时不存在）
    mtime: Option<SystemTime>,
    hooks: Arc<Hooks>,
}

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| {
        Mutex::new(Cache {
            mtime: None,
            hooks: disabled(),
        })
    })
}

/// 取当前生效的 hooks 配置（每个事件调用一次；文件变了就重读）。
pub fn current() -> Arc<Hooks> {
    let Some(path) = config_path() else {
        return disabled();
    };
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let Ok(mut c) = cache().lock() else {
        return disabled();
    };
    if c.mtime == mtime {
        return Arc::clone(&c.hooks);
    }
    c.mtime = mtime;
    let next = match std::fs::read_to_string(&path) {
        Ok(text) => {
            let h = Hooks::parse(&text);
            match &h.error {
                None => {
                    log::info(format!("hooks 配置：{}", h.summary()));
                    for w in &h.warnings {
                        log::warn(format!("hooks 配置：{w}"));
                    }
                    Arc::new(h)
                }
                Some(e) => {
                    // 保留上一份有效配置 + 一行 WARN（用户在前端也能看到语法错误：
                    // 设置面板的 hooks 行会读同一个文件报错）。
                    log::warn(format!(
                        "hooks 配置无效，沿用上一份：{e}（{}）",
                        path.display()
                    ));
                    return Arc::clone(&c.hooks);
                }
            }
        }
        Err(e) => {
            log::warn(format!("读不出 hooks 配置（{}）：{e}", path.display()));
            disabled()
        }
    };
    c.hooks = Arc::clone(&next);
    next
}

// ── 执行 ─────────────────────────────────────────────────────────

/// 一条要展示/回灌的 hook 输出。`kind`：
/// `block`（拦下 / 交给模型）/ `allow`（放行）/ `info`（补充信息）/ `error`（hook 失败）
pub struct Item {
    pub kind: &'static str,
    pub text: String,
    pub command: String,
}

#[derive(Debug, PartialEq)]
pub enum Decision {
    /// 没有 hook 表态 —— 照原逻辑走
    Pass,
    /// 有 hook 明确放行（只在 PreToolUse / PermissionRequest 上有意义）
    Allow,
    /// 有 hook 明确拒绝，附原因（只在能拦的事件上出现）
    Deny(String),
}

impl Default for Decision {
    fn default() -> Self {
        Decision::Pass
    }
}

#[derive(Default)]
pub struct Run {
    pub decision: Decision,
    pub items: Vec<Item>,
}

impl Run {
    /// 这个事件有没有产出任何要告诉用户的东西
    pub fn is_quiet(&self) -> bool {
        self.items.is_empty()
    }

    /// PostToolUse 的「交给模型」文本：`info`（hook 的补充说明）与 `block`
    /// （退出码 2 的反馈）都要拼进 `tool_result` —— 这是 PostToolUse 唯一的用处。
    pub fn model_note(&self) -> Option<String> {
        let parts: Vec<String> = self
            .items
            .iter()
            .filter(|i| matches!(i.kind, "info" | "block"))
            .map(|i| i.text.clone())
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    }
}

/// 跑一个事件的全部命中 hook。
///
/// `payload` 既是**子进程的 stdin**，也是 matcher / cwd 的来源 —— 形状见 §3.5。
pub fn fire(h: &Hooks, event: &str, payload: &Value) -> Run {
    let mut run = Run::default();
    if h.is_empty() {
        return run;
    }
    let subject = payload.get("tool_name").and_then(Value::as_str);
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let blocking = can_block(event);
    for g in h.groups.iter().filter(|g| g.event == event) {
        if let Some(re) = &g.matcher {
            // 没有 tool_name 的 payload 与 matcher 组不匹配（不是「匹配一切」）
            let hit = subject.map(|s| re.is_match(s)).unwrap_or(false);
            if !hit {
                continue;
            }
        }
        for cmd in &g.commands {
            match run_one(cmd, event, payload, &cwd) {
                Ok(out) => absorb(&mut run, out, cmd, blocking),
                Err(e) => run.items.push(Item {
                    kind: "error",
                    text: format!("hook 无法启动（{e}）—— 本次不拦"),
                    command: cmd.command.clone(),
                }),
            }
        }
    }
    run
}

struct RawOut {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

fn absorb(run: &mut Run, out: RawOut, cmd: &Cmd, blocking: bool) {
    let label = cmd.command.clone();
    if out.timed_out {
        run.items.push(Item {
            kind: "error",
            text: format!(
                "hook 超时（{}s）已终止 —— 本次不拦",
                cmd.timeout.as_secs()
            ),
            command: label,
        });
        return;
    }
    if out.code == Some(2) {
        let reason = first_non_empty(&[&out.stderr, &out.stdout])
            .unwrap_or_else(|| "blocked by hook".to_string());
        verdict(run, blocking, reason, label);
        return;
    }
    if out.code != Some(0) {
        let detail = first_non_empty(&[&out.stderr, &out.stdout]).unwrap_or_default();
        run.items.push(Item {
            kind: "error",
            text: format!(
                "hook 退出码 {}（视为失败，本次不拦）{}",
                out.code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "null".into()),
                if detail.is_empty() {
                    String::new()
                } else {
                    format!("：{detail}")
                }
            ),
            command: label,
        });
        return;
    }

    let stdout = out.stdout.trim();
    if stdout.is_empty() {
        return;
    }
    if !stdout.starts_with('{') {
        // 纯文本 stdout：对 PostToolUse 而言这就是「给模型的补充信息」，
        // 其余事件里是给用户看的一行说明。
        run.items.push(Item {
            kind: "info",
            text: truncate(stdout),
            command: label,
        });
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(stdout) else {
        run.items.push(Item {
            kind: "error",
            text: "hook 的 stdout 以 { 开头但不是合法 JSON —— 本次不拦".into(),
            command: label,
        });
        return;
    };
    if !v.is_object() {
        run.items.push(Item {
            kind: "error",
            text: "hook 的 stdout 必须是 JSON 对象（如 {\"decision\":\"allow\"}）—— 本次不拦".into(),
            command: label,
        });
        return;
    }
    let mut recognized = false;
    match v.get("decision").and_then(Value::as_str) {
        Some(d) => {
            let d = d.trim().to_ascii_lowercase();
            let reason = v
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            match d.as_str() {
                "allow" => {
                    recognized = true;
                    run.items.push(Item {
                        kind: "allow",
                        text: if reason.is_empty() {
                            "hook 允许本次调用".into()
                        } else {
                            truncate(&reason)
                        },
                        command: label.clone(),
                    });
                    if run.decision == Decision::Pass {
                        run.decision = Decision::Allow;
                    }
                }
                "deny" | "block" => {
                    recognized = true;
                    let reason = if reason.is_empty() {
                        "blocked by hook".to_string()
                    } else {
                        reason
                    };
                    verdict(run, blocking, reason, label.clone());
                }
                other => {
                    recognized = true;
                    run.items.push(Item {
                        kind: "error",
                        text: format!("hook 的 decision=`{other}` 不认识（只认 allow / deny）—— 本次不拦"),
                        command: label.clone(),
                    });
                }
            }
        }
        None => {}
    }
    if let Some(ctx) = v
        .get("additionalContext")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        recognized = true;
        run.items.push(Item {
            kind: "info",
            text: truncate(ctx),
            command: label.clone(),
        });
    }
    if !recognized {
        run.items.push(Item {
            kind: "error",
            text: "hook 输出了 JSON 但没有可识别的 decision / additionalContext —— 本次不拦".into(),
            command: label,
        });
    }
}

/// 记一条裁决。能拦的事件里它就是「拒绝」；不能拦的事件里它只是「这段话说给模型/用户听」。
fn verdict(run: &mut Run, blocking: bool, reason: String, command: String) {
    let reason = truncate(&reason);
    run.items.push(Item {
        kind: "block",
        text: reason.clone(),
        command,
    });
    if blocking && !matches!(run.decision, Decision::Deny(_)) {
        run.decision = Decision::Deny(reason);
    }
}

fn first_non_empty(cands: &[&String]) -> Option<String> {
    cands
        .iter()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_ITEM_CHARS {
        return s.trim().to_string();
    }
    let head: String = s.chars().take(MAX_ITEM_CHARS).collect();
    format!("{}…（已截断）", head.trim())
}

/// 起一个 hook 子进程，喂 payload 到 stdin，收 stdout / stderr，按超时兜底。
///
/// 与 `tools.rs::run_shell` 同一套纪律：并发读干管道（不读会在管道写满时死锁）、
/// Windows 下 `CREATE_NO_WINDOW`（release 是 GUI 进程，不许弹黑框）、超时 kill。
fn run_one(cmd: &Cmd, event: &str, payload: &Value, cwd: &Path) -> Result<RawOut, String> {
    // **必须用 `raw_arg` 原样拼命令行**（2026-09-20 实测）：
    // `cmd /C` 的引号语义由它自己解释，而 `Command::arg` 会按 MSVC 规则把命令里的
    // `"` 转义成 `\"`（命令含空格时必然触发）—— cmd 不认这种转义，于是
    // `type "C:\a b\x.json"` 这类命令会整条失败。实测：同一条 `type` 命令带引号时
    // hook 拿不到任何输出（非 0 退出），去掉引号即正常；改用 raw_arg 后带引号也正常。
    #[cfg(windows)]
    let mut line = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd");
        c.raw_arg("/C").raw_arg(&cmd.command);
        c
    };
    #[cfg(not(windows))]
    let mut line = {
        let mut c = Command::new("sh");
        c.arg("-c").arg(&cmd.command);
        c
    };
    line.current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("LUNAC_HOOK_EVENT", event);
    if let Some(t) = payload.get("tool_name").and_then(Value::as_str) {
        line.env("LUNAC_TOOL_NAME", t);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        line.creation_flags(0x0800_0000);
    }
    let mut child = line.spawn().map_err(|e| e.to_string())?;

    // payload 从**独立线程**灌进 stdin：`Write` 的正文可达几百 KB，而管道写满时
    // 若子进程压根不读 stdin 就会死锁（与 tools.rs 的 drain 是同一类陷阱）。
    let body = payload.to_string();
    let mut stdin = child.stdin.take();
    let writer = std::thread::spawn(move || {
        if let Some(mut s) = stdin.take() {
            let _ = s.write_all(body.as_bytes());
        }
    });
    let out_pipe = child.stdout.take();
    let err_pipe = child.stderr.take();
    let out_reader = std::thread::spawn(move || drain(out_pipe));
    let err_reader = std::thread::spawn(move || drain(err_pipe));

    let deadline = Instant::now() + cmd.timeout;
    let mut timed_out = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code(),
            Ok(None) => {}
            Err(_) => break None,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break None;
        }
        std::thread::sleep(Duration::from_millis(POLL_MS));
    };

    let _ = writer.join();
    Ok(RawOut {
        code,
        stdout: out_reader.join().unwrap_or_default(),
        stderr: err_reader.join().unwrap_or_default(),
        timed_out,
    })
}

fn drain<R: Read>(pipe: Option<R>) -> String {
    let Some(mut r) = pipe else {
        return String::new();
    };
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match r.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if buf.len() < MAX_OUTPUT_BYTES {
                    let take = (MAX_OUTPUT_BYTES - buf.len()).min(n);
                    buf.extend_from_slice(&chunk[..take]);
                }
            }
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

// ── 测试 ─────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(tool: Option<&str>) -> Value {
        let mut v = json!({ "hook_event_name": "PreToolUse", "cwd": std::env::temp_dir() });
        if let Some(t) = tool {
            v["tool_name"] = json!(t);
        }
        v
    }

    /// 把一段文本落成临时文件，用 `type <file>` 当 hook 命令 —— 绕开在 cmd 里
    /// 转义 JSON 引号的麻烦，同时测的是**真实子进程**。
    fn echo_file(tag: &str, content: &str) -> String {
        let path = std::env::temp_dir().join(format!(
            "lunac_hook_test_{}_{}.json",
            tag,
            std::process::id()
        ));
        std::fs::write(&path, content).unwrap();
        // 带引号（用户脚本路径常含空格）—— 顺带钉住 `raw_arg` 那条修复
        format!("type \"{}\"", path.display())
    }

    fn one_group(event: &str, matcher: Option<&str>, command: &str) -> Hooks {
        let group = match matcher {
            Some(m) => json!({ "matcher": m, "hooks": [{ "command": command }] }),
            None => json!({ "hooks": [{ "command": command }] }),
        };
        let cfg = json!({ "hooks": { event: [group] } });
        Hooks::parse(&cfg.to_string())
    }

    #[test]
    fn config_is_parsed_with_matchers_and_timeouts() {
        let h = Hooks::parse(
            r#"{"enabled":true,"hooks":{"PreToolUse":[
                 {"matcher":"Bash|PowerShell","hooks":[{"type":"command","command":"echo a","timeout":5}]},
                 {"hooks":[{"command":"echo b"}]}
               ],
               "Stop":[{"hooks":[{"command":"echo c"}]}]}}"#,
        );
        assert!(h.error.is_none());
        assert!(h.warnings.is_empty(), "{:?}", h.warnings);
        assert!(!h.is_empty());
        assert_eq!(h.groups.len(), 3);
        assert_eq!(h.groups[0].commands[0].timeout, Duration::from_secs(5));
        // 默认超时
        assert_eq!(h.groups[2].commands[0].timeout, Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        // 超时上限
        let h2 = Hooks::parse(
            r#"{"hooks":{"Stop":[{"hooks":[{"command":"echo a","timeout":99999}]}]}}"#,
        );
        assert_eq!(h2.groups[0].commands[0].timeout, Duration::from_secs(MAX_TIMEOUT_SECS));
    }

    #[test]
    fn enabled_defaults_to_true_and_can_be_turned_off() {
        let on = Hooks::parse(r#"{"hooks":{"Stop":[{"hooks":[{"command":"echo a"}]}]}}"#);
        assert!(!on.is_empty());
        let off = Hooks::parse(
            r#"{"enabled":false,"hooks":{"Stop":[{"hooks":[{"command":"echo a"}]}]}}"#,
        );
        assert!(off.is_empty(), "enabled=false 时不许跑任何 hook");
    }

    #[test]
    fn broken_config_is_reported_not_silently_ignored() {
        for bad in ["[]", "{", r#"{"hooks":[]}"#, "null"] {
            let h = Hooks::parse(bad);
            assert!(h.error.is_some(), "`{bad}` 应被报为配置错误");
            assert!(h.is_empty());
        }
    }

    #[test]
    fn unknown_event_bad_regex_and_wrong_type_are_warned() {
        let h = Hooks::parse(
            r#"{"hooks":{
                 "PostToolUseX":[{"hooks":[{"command":"echo a"}]}],
                 "PreToolUse":[{"matcher":"([","hooks":[{"command":"echo b"}]}],
                 "Stop":[{"hooks":[{"type":"prompt","command":"echo c"}]},
                         {"hooks":[{"type":"command"}]}],
                 "PreCompact":[{"matcher":"Bash","hooks":[{"command":"echo d"}]}]}}"#,
        );
        assert!(h.error.is_none());
        // 未知事件 / 坏正则组 / 没有 command 的组都不该进执行表；matcher 用在非工具事件上是警告
        let events: Vec<&str> = h.groups.iter().map(|g| g.event.as_str()).collect();
        assert_eq!(events, vec!["PreCompact"]);
        // 5 处配置问题：未知事件 / 坏正则 / type 不是 command + 该组空 / 缺 command + 该组空 / matcher 用在非工具事件
        assert_eq!(h.warnings.len(), 7, "{:?}", h.warnings);
        assert!(h.groups[0].matcher.is_none(), "非工具事件的 matcher 必须被忽略");
    }

    #[test]
    fn exit_code_two_blocks_only_blocking_events() {
        let h = one_group("PreToolUse", None, "exit 2");
        let run = fire(&h, "PreToolUse", &payload(Some("Bash")));
        assert!(matches!(run.decision, Decision::Deny(_)), "{:?}", run.decision);
        assert_eq!(run.items[0].kind, "block");

        // PostToolUse 拦不住（工具已经跑完）—— 文本只回灌给模型
        let h = one_group("PostToolUse", None, "exit 2");
        let run = fire(&h, "PostToolUse", &payload(Some("Bash")));
        assert_eq!(run.decision, Decision::Pass);
        assert_eq!(run.items[0].kind, "block");
    }

    #[test]
    fn denial_reason_comes_from_stderr() {
        let h = one_group("PreToolUse", None, "echo nope-from-hook 1>&2 & exit 2");
        let run = fire(&h, "PreToolUse", &payload(Some("Bash")));
        match run.decision {
            Decision::Deny(r) => assert!(r.contains("nope-from-hook"), "{r}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn allow_decision_is_reported() {
        let cmd = echo_file("allow", r#"{"decision":"allow","reason":"白名单内"}"#);
        let h = one_group("PreToolUse", None, &cmd);
        let run = fire(&h, "PreToolUse", &payload(Some("Bash")));
        assert_eq!(run.decision, Decision::Allow);
        assert_eq!(run.items[0].kind, "allow");
    }

    #[test]
    fn deny_decision_wins_over_allow() {
        let allow = echo_file("allow2", r#"{"decision":"allow"}"#);
        let deny = echo_file("deny2", r#"{"decision":"deny","reason":"规则命中"}"#);
        let cfg = format!(
            "{{\"hooks\":{{\"PreToolUse\":[{{\"hooks\":[{{\"command\":{}}},{{\"command\":{}}}]}}]}}}}",
            serde_json::to_string(&allow).unwrap(),
            serde_json::to_string(&deny).unwrap()
        );
        let run = fire(&Hooks::parse(&cfg), "PreToolUse", &payload(Some("Bash")));
        match run.decision {
            Decision::Deny(r) => assert!(r.contains("规则命中"), "{r}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn additional_context_is_kept_for_the_model() {
        let cmd = echo_file("ctx", r#"{"additionalContext":"lint 建议：补个分号"}"#);
        let h = one_group("PostToolUse", None, &cmd);
        let run = fire(&h, "PostToolUse", &payload(Some("Bash")));
        assert_eq!(run.decision, Decision::Pass);
        assert!(run.model_note().unwrap().contains("补个分号"));
    }

    #[test]
    fn plain_stdout_is_info_and_unparsable_json_is_visible_error() {
        let h = one_group("PostToolUse", None, "echo hello");
        let run = fire(&h, "PostToolUse", &payload(Some("Bash")));
        assert_eq!(run.items[0].kind, "info");
        assert!(run.model_note().unwrap().contains("hello"));

        // 以 { 开头但不是合法 JSON ⇒ 可见的 error，且**不拦**
        let h = one_group("PreToolUse", None, "echo {not json");
        let run = fire(&h, "PreToolUse", &payload(Some("Bash")));
        assert_eq!(run.decision, Decision::Pass);
        assert_eq!(run.items[0].kind, "error");
    }

    #[test]
    fn non_zero_exit_is_visible_but_never_blocks() {
        let h = one_group("PreToolUse", None, "echo boom-err 1>&2 & exit 7");
        let run = fire(&h, "PreToolUse", &payload(Some("Bash")));
        assert_eq!(run.decision, Decision::Pass, "只认显式拒绝：非 0/2 一律不拦");
        assert_eq!(run.items[0].kind, "error");
        assert!(run.items[0].text.contains("退出码 7"), "{}", run.items[0].text);
        assert!(run.items[0].text.contains("boom-err"));
    }

    #[test]
    fn unmatched_tool_never_runs_the_hook() {
        let h = one_group("PreToolUse", Some("Bash"), "exit 2");
        let run = fire(&h, "PreToolUse", &payload(Some("Read")));
        assert!(run.is_quiet());
        assert_eq!(run.decision, Decision::Pass);
        // 没有 tool_name 的 payload 也不算命中
        let run = fire(&h, "PreToolUse", &payload(None));
        assert!(run.is_quiet());
    }

    #[test]
    fn timeout_is_visible_and_does_not_block() {
        // 睡眠命令：timeout 1s，用 ping 拖住（sleep 在 cmd 里不一定有）
        let cfg = r#"{"hooks":{"PreToolUse":[{"hooks":[{"command":"ping -n 6 127.0.0.1 > nul","timeout":1}]}]}}"#;
        let run = fire(&Hooks::parse(cfg), "PreToolUse", &payload(Some("Bash")));
        assert_eq!(run.decision, Decision::Pass);
        assert_eq!(run.items[0].kind, "error");
        assert!(run.items[0].text.contains("超时"), "{}", run.items[0].text);
    }
}
