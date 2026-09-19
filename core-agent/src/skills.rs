// core-agent/src/skills.rs
// Lunac 自研 agent 后端 —— P4：技能（SKILL.md）
//
// 目录由 src-tauri 经 `LUNAC_SKILLS_DIR` 注入（= `<exe 根>\skills`），布局与设置
// 面板「技能扩展」一致：`<key>/SKILL.md`，frontmatter 至少含 name / description
// （key 由 Rust/前端按 frontmatter.name slug 派生，这里直接取目录名当 key）。
//
// 渐进披露：系统提示词里只列 `key: 描述`（省 token），模型判断该用哪个之后调
// `Skill` 工具取正文；正文里的 `$ARGUMENTS` 用工具入参 `args` 替换。
//
// 技能是只读能力（不改文件、不发网络），因此**不需要审批**，`plan` 档也允许 ——
// 与 `Read` 同级。

use std::path::PathBuf;

use serde_json::{json, Value};

/// 单条描述上限：列表只用于「发现」，正文由 Skill 工具按需加载，
/// 描述写太长只会白占每轮前缀（旧 CLI 同样截到 250 字符）。
const MAX_DESC_CHARS: usize = 250;
/// 技能清单在系统提示词里的总字符预算，超出部分只报数量
const LISTING_BUDGET_CHARS: usize = 8_000;

/// `Clone` 是为了后台复盘 fork：它在另一条线程上跑，需要自己那份技能清单
/// （见 main.rs 的 `run_review_fork`）。
#[derive(Clone)]
pub struct Skill {
    /// 目录名 —— 模型调用 Skill 时用它
    pub key: String,
    /// frontmatter.name（缺省回落为 key）
    pub name: String,
    pub description: String,
    /// SKILL.md 全文（含 frontmatter；正文见 `body()`）
    content: String,
}

/// 扫描 `LUNAC_SKILLS_DIR`。未设置 / 目录不存在 / 单个技能读不出来一律跳过 ——
/// 技能缺失绝不能影响六件内置工具。
pub fn load() -> Vec<Skill> {
    let Ok(dir) = std::env::var("LUNAC_SKILLS_DIR") else {
        return Vec::new();
    };
    let dir = PathBuf::from(dir.trim());
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path.join("SKILL.md")) else {
            continue;
        };
        let key = entry.file_name().to_string_lossy().to_string();
        let (name, description) = parse_frontmatter(&content);
        out.push(Skill {
            key: key.clone(),
            name: if name.is_empty() { key } else { name },
            description,
            content,
        });
    }
    // 稳定顺序 → 系统提示词不随 read_dir 抖动（前缀缓存友好）
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// 解析 `---` frontmatter 的 name / description。与 src-tauri `commands.rs`
/// 的 `parse_skill_md_str` 同一套宽松规则（逐行找 `name:` / `description:`，去引号）。
fn parse_frontmatter(content: &str) -> (String, String) {
    let mut name = String::new();
    let mut description = String::new();
    if let Some(rest) = content.trim_start().strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                let line = line.trim();
                if let Some(v) = line.strip_prefix("name:") {
                    name = v.trim().trim_matches('"').trim_matches('\'').to_string();
                } else if let Some(v) = line.strip_prefix("description:") {
                    description = v.trim().trim_matches('"').trim_matches('\'').to_string();
                }
            }
        }
    }
    (name, description)
}

/// 去掉 frontmatter 的正文（真正给模型看的指令）
fn body(content: &str) -> &str {
    if let Some(rest) = content.trim_start().strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            return rest[end + 4..].trim_start();
        }
    }
    content.trim_start()
}

/// `Skill` 工具的 schema
pub fn tool_def() -> Value {
    json!({
        "name": "Skill",
        "description": "Load a skill — an instruction set (SKILL.md) shipped with the app or \
            installed by the user — into the conversation, then follow it. Call this before \
            starting the task whenever the request matches one of the skills listed in the \
            system prompt. Loaded instructions are authoritative for how to do the task.",
        "input_schema": {
            "type": "object",
            "properties": {
                "skill": { "type": "string", "description": "Skill key, exactly as listed in the system prompt" },
                "args": { "type": "string", "description": "Optional arguments; replaces $ARGUMENTS in the skill body" }
            },
            "required": ["skill"]
        }
    })
}

/// 拼进系统提示词的技能清单。空表返回空串（调用方保持原提示词）。
pub fn listing(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n\nAvailable skills — when a request matches one, call the Skill tool with its key \
         before doing the task, then follow the loaded instructions:\n",
    );
    let mut used = 0usize;
    let mut listed = 0usize;
    for skill in skills {
        let desc: String = skill.description.chars().take(MAX_DESC_CHARS).collect();
        let line = if desc.is_empty() {
            format!("- {}\n", skill.key)
        } else {
            format!("- {}: {}\n", skill.key, desc)
        };
        let width = line.chars().count();
        if used + width > LISTING_BUDGET_CHARS {
            break;
        }
        used += width;
        out.push_str(&line);
        listed += 1;
    }
    if listed < skills.len() {
        out.push_str(&format!(
            "- ({} more skill(s) installed but not listed)\n",
            skills.len() - listed
        ));
    }
    out
}

/// 执行 `Skill` 调用：返回该技能正文（`$ARGUMENTS` 已替换）。
pub fn run(skills: &[Skill], input: &Value) -> Result<String, String> {
    let want = input
        .get("skill")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if want.is_empty() {
        return Err("missing required parameter \"skill\"".into());
    }
    let found = skills
        .iter()
        .find(|s| s.key == want)
        .or_else(|| skills.iter().find(|s| s.name.eq_ignore_ascii_case(want)))
        .or_else(|| skills.iter().find(|s| s.key.eq_ignore_ascii_case(want)));
    let Some(skill) = found else {
        let keys: Vec<&str> = skills.iter().map(|s| s.key.as_str()).collect();
        return Err(format!(
            "Unknown skill \"{want}\". Available: {}",
            keys.join(", ")
        ));
    };

    let args = input.get("args").and_then(Value::as_str).unwrap_or("");
    let body = body(&skill.content).replace("$ARGUMENTS", args);
    Ok(format!(
        "Skill \"{}\" loaded — follow these instructions:\n\n{body}",
        skill.key
    ))
}
