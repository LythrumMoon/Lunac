//! 对话「更多设置」的持久化（v1 是 localStorage，v2 落 `<exe 根>/config/chat.json`）。
//!
//! **对齐口径**（v1 `main.ts` 的当前实现）：
//!   · 命令审批方式 `runMode`（`manual` / `allowlist` / `auto`）—— 决定「问不问」；
//!   · 权限 `profile`（`safe` / `project` / `full`）—— 决定「允不允许」；
//!   · 思考开关 `thinking` —— 对应 agent 的 `LUNAC_THINKING`；
//!   · 工具黑名单 `toolBlacklist` —— 保存后持久化，agent 重启时变成 `--disallowedTools`。
//!
//! 两者正交（v1 `RUN_MODE_PROFILE`）：切运行方式会同时落一个档位（自动 → full，其余 → project），
//! 而「只读」档下「自动审批」的说法不成立，所以 `profile=safe` 会把 `runMode` 收回 `manual`
//! （收紧方向，不会悄悄放松）。
//!
//! 人格 `config/persona.md` 也在这里读写：agent 经 `LUNAC_PERSONA_FILE` **启动时读一次**，
//! 所以面板保存后要重启 agent 才生效（v1 同语义）。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// 人格文本上限（字符）。与 v1 `storage::MAX_PERSONA_CHARS` / core-agent 同值。
pub const MAX_PERSONA_CHARS: usize = 8_000;

/// 命令审批方式（顺序 = 点一下循环的顺序）。
pub const RUN_MODES: [&str; 3] = ["manual", "allowlist", "auto"];
/// 权限档位（文件边界）。
pub const PROFILES: [&str; 3] = ["safe", "project", "full"];
/// 工作细节的折叠时机（§待办 3-28.17）：
///   · `reply` = 回复结束就收起（现状，缺省）；
///   · `nextMessage` = 等下一条用户消息到达才收起（长回合跑完后还能回头看过程）。
pub const FOLD_TIMINGS: [&str; 2] = ["reply", "nextMessage"];

/// 运行方式 → 权限档位（与 v1 `RUN_MODE_PROFILE` 逐条一致）。
fn run_mode_profile(mode: &str) -> &'static str {
    match mode {
        "auto" => "full",
        _ => "project",
    }
}

/// 工具黑名单候选（与 v1 `TOOL_BLACKLIST_CANDIDATES` 同名单；MCP 用户工具名运行期才知道，
/// 不在此列）。
pub const TOOL_CANDIDATES: [&str; 34] = [
    "Read",
    "Write",
    "Edit",
    "NotebookEdit",
    "Cmd",
    "PowerShell",
    "Glob",
    "Grep",
    "WebSearch",
    "WebFetch",
    "AskUserQuestion",
    "TodoWrite",
    "SessionSearch",
    "MemorySearch",
    "CronList",
    "Agent",
    "ListMcpResourcesTool",
    "ReadMcpResourceTool",
    "Remember",
    "Skill",
    "EnterPlanMode",
    "ExitPlanMode",
    "ListPeers",
    "SendMessage",
    "PetControl",
    "EnterWorktree",
    "ExitWorktree",
    "SendUserFile",
    "PushNotification",
    "RepoMap",
    "LSP",
    "WebBrowser",
    "ImageMeasure",
    "ImageAnnotate",
];

/// 对话设置（缺省值与 v1 一致：思考开、白名单、工作区档）。
#[derive(Clone, Debug, PartialEq)]
pub struct ChatPrefs {
    pub thinking: bool,
    pub run_mode: String,
    pub profile: String,
    pub tool_blacklist: Vec<String>,
    /// `reply` / `nextMessage`（§待办 3-28.17；缺省 `reply` = 改造前的行为）。
    pub fold_timing: String,
    /// 第二模型预审（§待办 3-28.14）：对应 agent 的 `LUNAC_SECOND_REVIEW`。
    /// 缺省关 —— 只有 deny / escalate，永不 allow，但会多花一次模型调用。
    pub second_review: bool,
    /// 技能级记忆（§待办 3-28.10 ①）：对应 agent 的 `LUNAC_SKILL_MEMORY`。
    ///
    /// 缺省关 —— 关着时 fork 子代理不读技能记忆、`Remember` 也不写技能命名空间，
    /// 行为与改造前逐字节一致。开着才把 `ModuleData/memory/skills/<key>/MEMORY.md`
    /// 注入**该技能 fork 出来的子代理**系统提示词（不动主对话前缀）。
    pub skill_memory: bool,
    /// PTC / Code Mode（§待办 3-28.15）：对应 agent 的 `LUNAC_PTC`。
    /// 缺省关 —— 关着时 `RunScript` 根本不出现在工具表里（模型不会看到它）。
    pub ptc_mode: bool,
    /// 用户选定的工作区（空 = 用应用数据目录，且**不启用工作区锁**）。
    ///
    /// v1 的语义：只有显式配置了工作区才注入 `LUNAC_WORKSPACE_LOCKED=1`（越界读写直接拒），
    /// 没配置时全系统可访问、写操作走审批卡。
    pub workspace: String,
}

impl Default for ChatPrefs {
    fn default() -> Self {
        Self {
            thinking: true,
            run_mode: "allowlist".to_string(),
            profile: "project".to_string(),
            tool_blacklist: Vec::new(),
            fold_timing: "reply".to_string(),
            second_review: false,
            skill_memory: false,
            ptc_mode: false,
            workspace: String::new(),
        }
    }
}

pub fn config_path(root: &Path) -> PathBuf {
    root.join("config").join("chat.json")
}

pub fn persona_path(root: &Path) -> PathBuf {
    root.join("config").join("persona.md")
}

/// 生效中的工作区 + 「是否为用户选定」。
///
/// v1 语义：**只有显式配置过工作区才启用工作区锁**（`LUNAC_WORKSPACE_LOCKED=1`）；
/// 没配置 / 配的目录已不存在 ⇒ 回落应用数据目录（`sandbox::workdir`），且不启用锁
/// （此时越界读写走审批卡，与 v1「全系统访问」同口径）。
pub fn resolve_workspace(root: &Path, prefs: &ChatPrefs) -> (PathBuf, bool) {
    let configured = prefs.workspace.trim();
    if !configured.is_empty() {
        let path = PathBuf::from(configured);
        if path.is_dir() {
            return (path, true);
        }
    }
    (crate::sandbox::workdir(root), false)
}

fn load_value(root: &Path) -> Value {
    let path = config_path(root);
    let Ok(text) = fs::read_to_string(&path) else {
        return json!({});
    };
    serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({}))
}

/// 读配置（读取时也做一次归一化：坏值回落缺省，`safe` 档把运行方式收回手动）。
pub fn load(root: &Path) -> ChatPrefs {
    let value = load_value(root);
    normalize(&value)
}

/// 给界面的完整快照：设置 + 工具候选 + 人格原文 + 工作区路径。
pub fn snapshot_json(root: &Path) -> String {
    let prefs = load(root);
    let persona = fs::read_to_string(persona_path(root)).unwrap_or_default();
    let (workspace, workspace_configured) = resolve_workspace(root, &prefs);
    json!({
        "thinking": prefs.thinking,
        "runMode": prefs.run_mode,
        "profile": prefs.profile,
        "foldTiming": prefs.fold_timing,
        "secondReview": prefs.second_review,
        "skillMemory": prefs.skill_memory,
        "ptcMode": prefs.ptc_mode,
        "toolBlacklist": prefs.tool_blacklist,
        "toolCandidates": &TOOL_CANDIDATES[..],
        "persona": persona,
        "personaMaxChars": MAX_PERSONA_CHARS,
        // `workspace` = **生效中的**工作区（配置了就用配置的，否则回落应用数据目录）；
        // `workspaceConfigured` 区分「用户选的」与「默认的」—— 面板要靠它决定是否显示
        // 「恢复默认」，agent 侧的工作区锁也只认 `true`（v1 同语义）。
        "workspace": workspace.display().to_string(),
        "workspaceConfigured": workspace_configured,
        "defaultWorkspace": crate::sandbox::workdir(root).display().to_string(),
    })
    .to_string()
}

fn normalize(value: &Value) -> ChatPrefs {
    let mut prefs = ChatPrefs {
        thinking: value
            .get("thinking")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        run_mode: value
            .get("runMode")
            .and_then(|v| v.as_str())
            .filter(|v| RUN_MODES.contains(v))
            .unwrap_or("allowlist")
            .to_string(),
        profile: value
            .get("profile")
            .and_then(|v| v.as_str())
            .filter(|v| PROFILES.contains(v))
            .unwrap_or("project")
            .to_string(),
        tool_blacklist: Vec::new(),
        fold_timing: value
            .get("foldTiming")
            .and_then(|v| v.as_str())
            .filter(|v| FOLD_TIMINGS.contains(v))
            .unwrap_or("reply")
            .to_string(),
        second_review: value
            .get("secondReview")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        skill_memory: value
            .get("skillMemory")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        ptc_mode: value
            .get("ptcMode")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        workspace: value
            .get("workspace")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("")
            .to_string(),
    };
    if let Some(items) = value.get("toolBlacklist").and_then(|v| v.as_array()) {
        for item in items {
            let Some(name) = item.as_str() else {
                continue;
            };
            let name = name.trim();
            if !name.is_empty() && !prefs.tool_blacklist.iter().any(|n| n == name) {
                prefs.tool_blacklist.push(name.to_string());
            }
        }
    }
    // 只读档下不存在「自动审批」：收回最保守的一档（与 v1 同方向）。
    if prefs.profile == "safe" {
        prefs.run_mode = "manual".to_string();
    }
    prefs
}

/// 合并写入：只覆盖传入的键，文件里的其它字段原样保留。
///
/// 运行方式与档位的联动与 v1 一致：传 `runMode` 会同时落对应档位；传 `profile=safe`
/// 会把运行方式收回 `manual`。
pub fn save_patch(root: &Path, patch: &Value) -> Result<ChatPrefs, String> {
    let Some(object) = patch.as_object() else {
        return Err("设置必须是 JSON 对象".to_string());
    };
    let path = config_path(root);
    let mut base = load_value(root);
    if !base.is_object() {
        base = json!({});
    }
    let map = base.as_object_mut().expect("object ensured above");

    if let Some(thinking) = object.get("thinking") {
        let Some(value) = thinking.as_bool() else {
            return Err("thinking 必须是布尔值".to_string());
        };
        map.insert("thinking".to_string(), json!(value));
    }

    // 先落档位，再落运行方式：两者同时出现时以运行方式的联动为准。
    if let Some(profile) = object.get("profile") {
        let Some(name) = profile.as_str().filter(|v| PROFILES.contains(v)) else {
            return Err("profile 必须是 safe / project / full".to_string());
        };
        map.insert("profile".to_string(), json!(name));
        if name == "safe" {
            map.insert("runMode".to_string(), json!("manual"));
        }
    }
    if let Some(mode) = object.get("runMode") {
        let Some(name) = mode.as_str().filter(|v| RUN_MODES.contains(v)) else {
            return Err("runMode 必须是 manual / allowlist / auto".to_string());
        };
        map.insert("runMode".to_string(), json!(name));
        map.insert("profile".to_string(), json!(run_mode_profile(name)));
        if map.get("profile").and_then(|v| v.as_str()) == Some("safe") {
            // 运行方式要求档位时不会留在只读档，但保留这条判断防止未来改坏。
            map.insert("runMode".to_string(), json!("manual"));
        }
    }

    // 折叠时机（§待办 3-28.17）：只接受两档白名单，默认值在此不落盘。
    if let Some(timing) = object.get("foldTiming") {
        let Some(name) = timing.as_str().filter(|v| FOLD_TIMINGS.contains(v)) else {
            return Err("foldTiming 必须是 reply / nextMessage".to_string());
        };
        map.insert("foldTiming".to_string(), json!(name));
    }

    // 第二模型预审开关（§待办 3-28.14）：布尔；开关本身不改变审批链的形状。
    if let Some(second) = object.get("secondReview") {
        let Some(value) = second.as_bool() else {
            return Err("secondReview 必须是布尔值".to_string());
        };
        map.insert("secondReview".to_string(), json!(value));
    }

    // 技能级记忆开关（§待办 3-28.10 ①）：布尔；开着才注入 / 才允许写技能命名空间。
    if let Some(skill_memory) = object.get("skillMemory") {
        let Some(value) = skill_memory.as_bool() else {
            return Err("skillMemory 必须是布尔值".to_string());
        };
        map.insert("skillMemory".to_string(), json!(value));
    }

    // PTC / Code Mode 开关（§待办 3-28.15）：布尔；关着时 `RunScript` 不出现在工具表里。
    if let Some(ptc) = object.get("ptcMode") {
        let Some(value) = ptc.as_bool() else {
            return Err("ptcMode 必须是布尔值".to_string());
        };
        map.insert("ptcMode".to_string(), json!(value));
    }

    // 工作区（v1 语义）：空串 = 恢复默认（应用数据目录，且不启用工作区锁）。
    if let Some(workspace) = object.get("workspace") {
        let Some(raw) = workspace.as_str() else {
            return Err("workspace 必须是字符串路径".to_string());
        };
        let text = raw.trim();
        if !text.is_empty() {
            let path = Path::new(text);
            if !path.is_absolute() {
                return Err(format!("工作区必须是绝对路径：{text}"));
            }
            if !path.is_dir() {
                return Err(format!("工作区目录不存在：{text}"));
            }
            if text.contains('\0') {
                return Err("工作区路径不能含空字符（NUL）".to_string());
            }
        }
        map.insert("workspace".to_string(), json!(text));
    }

    if let Some(list) = object.get("toolBlacklist") {
        let Some(items) = list.as_array() else {
            return Err("toolBlacklist 必须是字符串数组".to_string());
        };
        let mut cleaned: Vec<String> = Vec::new();
        for item in items {
            let Some(name) = item.as_str() else {
                return Err("toolBlacklist 必须是字符串数组".to_string());
            };
            let name = name.trim();
            if !name.is_empty() && !cleaned.iter().any(|n| n == name) {
                cleaned.push(name.to_string());
            }
        }
        map.insert("toolBlacklist".to_string(), json!(cleaned));
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let text = serde_json::to_string_pretty(&base).map_err(|e| format!("序列化设置失败：{e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(|e| format!("写设置失败（{}）：{e}", tmp.display()))?;
    fs::rename(&tmp, &path).map_err(|e| format!("替换设置失败（{}）：{e}", path.display()))?;
    Ok(load(root))
}

/// 读人格原文（没有文件 = 空串）。
#[cfg(test)]
pub fn persona_text(root: &Path) -> String {
    fs::read_to_string(persona_path(root)).unwrap_or_default()
}

/// 校验 + 写人格。空文本合法：语义是「恢复内置人格」。
pub fn save_persona(root: &Path, text: &str) -> Result<(), String> {
    let n = text.chars().count();
    if n > MAX_PERSONA_CHARS {
        return Err(format!(
            "人格文本过长：{n} 字符，上限 {MAX_PERSONA_CHARS}（它进的是每次请求都要发的固定前缀）"
        ));
    }
    if text.contains('\0') {
        return Err("人格文本不能含空字符（NUL）".to_string());
    }
    let path = persona_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    fs::write(&path, text).map_err(|e| format!("写不进 {}：{e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-chat-settings-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn defaults_match_v1_and_bad_values_fall_back() {
        let dir = temp_dir("defaults");
        let prefs = load(&dir);
        assert!(prefs.thinking);
        assert_eq!(prefs.run_mode, "allowlist");
        assert_eq!(prefs.profile, "project");
        assert!(prefs.tool_blacklist.is_empty());
        assert_eq!(prefs.fold_timing, "reply", "缺省 = 回复结束就收起");
        assert!(!prefs.second_review, "第二模型预审缺省关（多花一次模型调用）");
        assert!(prefs.workspace.is_empty(), "缺省没有用户工作区（不启用工作区锁）");

        fs::create_dir_all(dir.join("config")).unwrap();
        fs::write(
            config_path(&dir),
            r#"{"thinking":"yes","runMode":"bogus","profile":"root","toolBlacklist":["Write","Write",""],"foldTiming":"whenever"}"#,
        )
        .unwrap();
        let prefs = load(&dir);
        assert!(prefs.thinking, "坏值回落「开」");
        assert_eq!(prefs.run_mode, "allowlist");
        assert_eq!(prefs.profile, "project");
        assert_eq!(prefs.tool_blacklist, vec!["Write".to_string()]);
        assert_eq!(prefs.fold_timing, "reply", "坏值回落缺省");
        let _ = fs::remove_dir_all(&dir);
    }

    /// §待办 3-28.17：折叠时机两档只能走白名单；写进去后能被读回、非法值被拒。
    #[test]
    fn fold_timing_round_trips_and_rejects_unknown_values() {
        let dir = temp_dir("fold");
        let prefs = save_patch(&dir, &json!({"foldTiming": "nextMessage"})).expect("save");
        assert_eq!(prefs.fold_timing, "nextMessage");
        assert_eq!(load(&dir).fold_timing, "nextMessage", "落盘后可读回");

        let bad = save_patch(&dir, &json!({"foldTiming": "later"}));
        assert!(bad.is_err(), "未知档位必须拒绝");
        assert_eq!(
            load(&dir).fold_timing,
            "nextMessage",
            "被拒的写入不能改到已有值"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// §待办 3-28.14：第二模型预审开关落盘 / 读回 / 类型校验。
    #[test]
    fn second_review_toggle_round_trips_and_requires_a_bool() {
        let dir = temp_dir("second-review");
        let prefs = save_patch(&dir, &json!({"secondReview": true})).expect("save");
        assert!(prefs.second_review);
        assert!(load(&dir).second_review, "落盘后可读回");

        let bad = save_patch(&dir, &json!({"secondReview": "yes"}));
        assert!(bad.is_err(), "非布尔值必须拒绝");
        assert!(load(&dir).second_review, "被拒的写入不能改到已有值");

        let snapshot: Value = serde_json::from_str(&snapshot_json(&dir)).unwrap();
        assert_eq!(snapshot["secondReview"], true, "界面快照要带上");
        let _ = fs::remove_dir_all(&dir);
    }

    /// §待办 3-28.10 ① / 3-28.15：技能级记忆与 PTC 两个开关的落盘 / 读回 / 类型校验。
    #[test]
    fn skill_memory_and_ptc_toggles_round_trip_and_require_bools() {
        let dir = temp_dir("skill-memory-ptc");
        assert!(!load(&dir).skill_memory, "技能级记忆缺省关");
        assert!(!load(&dir).ptc_mode, "PTC 缺省关");

        let prefs = save_patch(&dir, &json!({"skillMemory": true, "ptcMode": true})).expect("save");
        assert!(prefs.skill_memory && prefs.ptc_mode);
        let reloaded = load(&dir);
        assert!(reloaded.skill_memory && reloaded.ptc_mode, "落盘后可读回");

        assert!(
            save_patch(&dir, &json!({"skillMemory": 1})).is_err(),
            "非布尔值必须拒绝"
        );
        assert!(
            save_patch(&dir, &json!({"ptcMode": "on"})).is_err(),
            "非布尔值必须拒绝"
        );
        assert!(load(&dir).skill_memory && load(&dir).ptc_mode, "被拒的写入不能改到已有值");

        let snapshot: Value = serde_json::from_str(&snapshot_json(&dir)).unwrap();
        assert_eq!(snapshot["skillMemory"], true, "界面快照要带上");
        assert_eq!(snapshot["ptcMode"], true, "界面快照要带上");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 工作区（v1 语义）：必须是存在的绝对目录；空串 = 恢复默认；
    /// 目录被删后回落默认且不再算「已配置」（不留下一个指向空路径的锁）。
    #[test]
    fn workspace_needs_a_real_absolute_dir_and_falls_back_when_it_disappears() {
        let dir = temp_dir("workspace");
        let target = dir.join("proj");
        fs::create_dir_all(&target).unwrap();

        let prefs = save_patch(&dir, &json!({"workspace": target.display().to_string()}))
            .expect("save");
        assert_eq!(prefs.workspace, target.display().to_string());
        let (effective, configured) = resolve_workspace(&dir, &prefs);
        assert!(configured);
        assert_eq!(effective, target);

        assert!(
            save_patch(&dir, &json!({"workspace": "relative/path"})).is_err(),
            "相对路径必须拒绝"
        );
        let missing = dir.join("does-not-exist");
        assert!(
            save_patch(&dir, &json!({"workspace": missing.display().to_string()})).is_err(),
            "不存在的目录必须拒绝"
        );
        assert_eq!(
            load(&dir).workspace,
            target.display().to_string(),
            "被拒的写入不能改到已有值"
        );

        // 目录被删掉 ⇒ 回落默认，且不再启用工作区锁。
        fs::remove_dir_all(&target).unwrap();
        let (effective, configured) = resolve_workspace(&dir, &load(&dir));
        assert!(!configured);
        assert!(
            effective.ends_with("transStorage"),
            "回落应用数据目录：{}",
            effective.display()
        );

        // 空串 = 恢复默认。
        let prefs = save_patch(&dir, &json!({"workspace": ""})).expect("reset");
        assert!(prefs.workspace.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_mode_moves_the_profile_and_safe_forces_manual() {
        let dir = temp_dir("modes");
        let prefs = save_patch(&dir, &json!({"runMode": "auto"})).expect("save");
        assert_eq!(prefs.profile, "full");
        assert_eq!(prefs.run_mode, "auto");

        let prefs = save_patch(&dir, &json!({"runMode": "allowlist"})).expect("save");
        assert_eq!(prefs.profile, "project");

        let prefs = save_patch(&dir, &json!({"profile": "safe"})).expect("save");
        assert_eq!(prefs.profile, "safe");
        assert_eq!(prefs.run_mode, "manual", "只读档下不存在自动审批");

        // 再切运行方式会把档位带回 project（v1 的联动）。
        let prefs = save_patch(&dir, &json!({"runMode": "manual"})).expect("save");
        assert_eq!(prefs.profile, "project");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_preserves_unrelated_keys_and_rejects_bad_patches() {
        let dir = temp_dir("merge");
        fs::create_dir_all(dir.join("config")).unwrap();
        fs::write(config_path(&dir), r#"{"vendorNote":"keep","thinking":true}"#).unwrap();

        save_patch(&dir, &json!({"thinking": false, "toolBlacklist": ["WebSearch"]}))
            .expect("save");
        let raw = fs::read_to_string(config_path(&dir)).unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["vendorNote"], "keep");
        assert_eq!(value["thinking"], false);
        assert_eq!(value["toolBlacklist"][0], "WebSearch");

        assert!(save_patch(&dir, &json!({"runMode": "sometimes"})).is_err());
        assert!(save_patch(&dir, &json!({"toolBlacklist": "Write"})).is_err());
        assert!(save_patch(&dir, &json!([1, 2])).is_err());
        // 非法补丁不落盘：值仍是刚才那一次的结果。
        assert_eq!(load(&dir).tool_blacklist, vec!["WebSearch".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn persona_round_trips_and_is_validated() {
        let dir = temp_dir("persona");
        assert_eq!(persona_text(&dir), "");
        save_persona(&dir, "你是 Lunac。").expect("save");
        assert_eq!(persona_text(&dir), "你是 Lunac。");

        let too_long = "x".repeat(MAX_PERSONA_CHARS + 1);
        assert!(save_persona(&dir, &too_long).is_err());
        assert!(save_persona(&dir, "a\0b").is_err());
        assert_eq!(persona_text(&dir), "你是 Lunac。", "校验失败不落盘");

        // 空文本 = 恢复内置人格（合法）。
        save_persona(&dir, "").expect("reset");
        assert_eq!(persona_text(&dir), "");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_carries_workspace_and_candidates() {
        let dir = temp_dir("snapshot");
        let value: Value = serde_json::from_str(&snapshot_json(&dir)).unwrap();
        assert_eq!(value["thinking"], true);
        assert_eq!(value["personaMaxChars"], MAX_PERSONA_CHARS);
        assert!(value["toolCandidates"].as_array().unwrap().len() >= 30);
        assert!(value["workspace"]
            .as_str()
            .unwrap()
            .replace('\\', "/")
            .ends_with("temp/transStorage"));
        assert_eq!(
            value["workspaceConfigured"], false,
            "没选过工作区 ⇒ 不启用工作区锁"
        );
        assert!(value["defaultWorkspace"].as_str().is_some());
        let _ = fs::remove_dir_all(&dir);
    }
}
