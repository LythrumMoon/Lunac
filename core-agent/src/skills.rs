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
// **两种模式**（2026-09-20 A5，抄旧 CLI `core/tools/SkillTool/SkillTool.ts`）：
//   · **inline**（默认）—— 把正文交回主循环，模型自己在**当前对话**里照着做。
//     只读能力（不改文件、不发网络）⇒ 不需要审批，`plan` 档也允许，与 `Read` 同级。
//   · **fork**（frontmatter 写 `context: fork`）—— 派生一个**独立上下文的子代理**
//     去执行，主对话只收它的报告。它可能写文件、跑命令 ⇒ **要审批**，且 `plan` 档拒绝；
//     执行入口在 main.rs 的 `run_forked_skill()`，**不在本文件**（这里只有
//     `run()` 那条 inline 路，遇到 fork 技能会明确报错）。
//   两条路共用 `find()` / `instruction()`，只有「谁来照做」不同。
//
// **自带脚本 / 资源**（2026-09-20 A5 剩余项）：技能目录里除 `SKILL.md` 之外的文件在
// 扫描时登记（`collect_resources()`），**调用 `Skill` 时**附在返回里（inline 附在正文后、
// fork 附进子代理的任务说明，见 `resources_note()`）。**刻意不进系统提示词** —— 它会随
// 用户往目录里丢文件而变化，进了提示词就等于让整段前缀缓存跟着文件系统抖动（规则 18）。

use std::path::PathBuf;

use serde_json::{json, Value};

/// 单条描述上限：列表只用于「发现」，正文由 Skill 工具按需加载，
/// 描述写太长只会白占每轮前缀（旧 CLI 同样截到 250 字符）。
const MAX_DESC_CHARS: usize = 250;
/// 技能清单在系统提示词里的总字符预算，超出部分只报数量
const LISTING_BUDGET_CHARS: usize = 8_000;
/// 一个技能最多登记多少个**自带资源文件**（2026-09-20 A5 剩余项）。
/// 边界刻意**保守**：漏掉一个深层文件只是少一条提示，把 `node_modules` 里几千条路径
/// 灌进上下文则是灾难。到顶时如实上报「还有没列出的」，**不静默截断**。
const MAX_RESOURCE_FILES: usize = 40;
/// 资源目录的递归深度上限：`scripts/run.py` 是 2 层，`a/b/c.txt` 是 3 层，再深不跟。
const MAX_RESOURCE_DEPTH: usize = 3;
/// 递归时跳过的目录名（忽略大小写）—— 它们是依赖 / 构建产物，不是技能资源
const SKIP_RESOURCE_DIRS: &[&str] = &["node_modules", "target"];

/// `Clone` 是为了后台复盘 fork：它在另一条线程上跑，需要自己那份技能清单
/// （见 main.rs 的 `run_review_fork`）。
#[derive(Clone)]
pub struct Skill {
    /// 目录名 —— 模型调用 Skill 时用它
    pub key: String,
    /// frontmatter.name（缺省回落为 key）
    pub name: String,
    pub description: String,
    /// `context: fork`（2026-09-20 A5）—— **在子代理里执行**而不是把正文注入主对话。
    /// 判据抄旧 CLI：`frontmatter.context === 'fork'`（`core/skills/loadSkillsDir.ts`），
    /// 执行路径对应 `core/tools/SkillTool/SkillTool.ts` 的 `executeForkedSkill()`。
    pub fork: bool,
    /// `allowed-tools:` 的工具白名单（逗号 / 空格分隔）。**空 = 不限制**（用子代理的默认工具集）。
    /// 只对 fork 技能有意义：inline 技能不执行任何工具，它只是把指令交给主循环。
    pub allowed_tools: Vec<String>,
    /// 技能目录下**自带的资源文件**（相对技能目录、`/` 分隔、已排序）。见 `collect_resources()`。
    ///
    /// **刻意不进系统提示词**：它随用户往目录里丢文件而变，进了提示词就等于让整段前缀缓存
    /// 跟着文件系统抖动（ai-spec §11 规则 18）。只在 `Skill` 被调用时附在返回里。
    resources: Vec<String>,
    /// 资源文件达到 `MAX_RESOURCE_FILES` 上限（还有没列出的）—— 上报用，不静默截断
    resources_capped: bool,
    /// SKILL.md 全文（含 frontmatter；正文见 `body()`）
    content: String,
}

/// `SKILL.md` frontmatter 里我们认的字段。**只认这几个** —— 旧 CLI 还有
/// `model:` / `effort:` / `paths:` / `hooks:` 等，它们在 Lunac 没有落点（端点只有一个模型、
/// 一种思考档，也没有条件激活与技能钩子），解析了不用就是死代码，见 ai-spec §11 规则 57。
struct Frontmatter {
    name: String,
    description: String,
    fork: bool,
    allowed_tools: Vec<String>,
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
        let fm = parse_frontmatter(&content);
        let (resources, resources_capped) = collect_resources(&path);
        out.push(Skill {
            name: if fm.name.is_empty() { key.clone() } else { fm.name },
            key,
            description: fm.description,
            fork: fm.fork,
            allowed_tools: fm.allowed_tools,
            resources,
            resources_capped,
            content,
        });
    }
    // 稳定顺序 → 系统提示词不随 read_dir 抖动（前缀缓存友好）
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// 扫一个技能目录里的**自带资源文件**，返回 `(相对路径, 是否被条数上限截断)`。
///
/// 边界（保守，2026-09-20 与用户定稿）：深度 ≤ `MAX_RESOURCE_DEPTH`、条数 ≤
/// `MAX_RESOURCE_FILES`、忽略隐藏项（`.` 开头，含 `.git` / `.DS_Store`）与
/// `node_modules` / `target`。`SKILL.md` 自己不算资源。
///
/// **不跟随符号链接**（既不递归进去、也不列出）—— 「报出去的路径一定落在技能目录内」
/// 这条保证就是从这里来的：不需要逐条 `canonicalize()` 比前缀，也就不会有
/// 「链接指向哪就把哪报出去」的外逃。`DirEntry::file_type()` 本身就是 `symlink_metadata`
/// 语义，所以这个判据不额外产生系统调用。
fn collect_resources(dir: &std::path::Path) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut capped = false;
    walk_resources(dir, dir, 1, &mut out, &mut capped);
    out.sort();
    (out, capped)
}

fn walk_resources(
    root: &std::path::Path,
    dir: &std::path::Path,
    depth: usize,
    out: &mut Vec<String>,
    capped: &mut bool,
) {
    // `*capped` 兼作「就此收工」的信号：一旦到顶就没必要再往下走（它是从子目录里
    // 设上的，所以要逐层短路出去）。
    if depth > MAX_RESOURCE_DEPTH || *capped {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if *capped {
            return;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.')
            || SKIP_RESOURCE_DIRS.iter().any(|d| d.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_symlink() {
            continue;
        }
        let path = entry.path();
        if ft.is_dir() {
            walk_resources(root, &path, depth + 1, out, capped);
        } else if ft.is_file() {
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            // 统一用 `/` 拼：比 `\` 短、可读，且 `Read` / `Glob` 两种分隔符都认
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            if rel == "SKILL.md" {
                continue;
            }
            // 上限判在**真正要收录时**，不是循环开头 —— 否则「正好 40 个文件 + SKILL.md」
            // 这种恰好到顶的情况会被误报成「还有没列出的」（SKILL.md 根本不该收录）。
            if out.len() >= MAX_RESOURCE_FILES {
                *capped = true;
                return;
            }
            out.push(rel);
        }
    }
}

/// 给模型看的**自带资源清单**，附在 `Skill` 的返回里。该技能没带文件 ⇒ 空串。
///
/// 抬头必须写清**相对谁**：路径是相对的，模型手里只有 env_block 里那个技能根目录，
/// 不说清就会出现「`Read("scripts/run.py")` 相对工作目录」这种必然失败的调用。
pub fn resources_note(skill: &Skill) -> String {
    let base = std::env::var("LUNAC_SKILLS_DIR")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "<skills dir>".into());
    resources_note_with_base(skill, &base)
}

fn resources_note_with_base(skill: &Skill, base: &str) -> String {
    if skill.resources.is_empty() {
        return String::new();
    }
    // 环境变量给的是 Windows 形态（`…\skills`），拼的却是 `/` —— 不去尾就会写成
    // `…\skills/demo-bundle/`（实测 2026-09-20 看到的样子）。`Read` 认混用分隔符，
    // 但混着写只是难看且容易被模型照抄错，这里先削掉尾部分隔符。
    let base = base.trim_end_matches(['\\', '/']);
    let mut out = format!(
        "\n\nFiles bundled with this skill (paths are relative to {base}/{}/ — use \
         Read / Glob when the instructions above refer to them):\n",
        skill.key
    );
    for r in &skill.resources {
        out.push_str(&format!("- {r}\n"));
    }
    if skill.resources_capped {
        out.push_str(&format!(
            "- (more files not listed — Glob {base}/{}/ for the full list)\n",
            skill.key
        ));
    }
    out
}

/// 解析 `---` frontmatter。与 src-tauri `commands.rs` 的 `parse_skill_md_str`
/// 同一套宽松规则（逐行找键，去引号）—— 那边只取 name / description（用于安装与展示），
/// 这里多取 `context` / `allowed-tools`（只有 agent 侧执行时才需要）。
fn parse_frontmatter(content: &str) -> Frontmatter {
    let mut fm = Frontmatter {
        name: String::new(),
        description: String::new(),
        fork: false,
        allowed_tools: Vec::new(),
    };
    let Some(rest) = content.trim_start().strip_prefix("---") else {
        return fm;
    };
    let Some(end) = rest.find("\n---") else {
        return fm;
    };
    for line in rest[..end].lines() {
        let line = line.trim();
        let unquote = |v: &str| v.trim().trim_matches('"').trim_matches('\'').to_string();
        if let Some(v) = line.strip_prefix("name:") {
            fm.name = unquote(v);
        } else if let Some(v) = line.strip_prefix("description:") {
            fm.description = unquote(v);
        } else if let Some(v) = line.strip_prefix("context:") {
            // 只认 `fork` 这一个值；其余（缺省 / 拼错）一律按 inline —— 与旧 CLI 的
            // `=== 'fork' ? 'fork' : undefined` 同口径，不发明第三态。
            fm.fork = unquote(v).eq_ignore_ascii_case("fork");
        } else if let Some(v) = line.strip_prefix("allowed-tools:") {
            // 兼容 `[a, b]` / `a, b` / `a b` 三种写法：剥掉方括号后按逗号与空白切。
            fm.allowed_tools = v
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split([',', ' ', '\t'])
                .map(|s| s.trim().trim_matches('"').trim_matches('\''))
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
    }
    fm
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

/// 按 key（精确）→ name（忽略大小写）→ key（忽略大小写）找技能。
/// **inline 与 fork 两路共用** —— 各写一份必然在「谁优先」上漂移，而模型看到的是
/// `Unknown skill` 还是「按名命中」取决于这个顺序。
pub fn find<'a>(skills: &'a [Skill], want: &str) -> Option<&'a Skill> {
    let want = want.trim();
    if want.is_empty() {
        return None;
    }
    skills
        .iter()
        .find(|s| s.key == want)
        .or_else(|| skills.iter().find(|s| s.name.eq_ignore_ascii_case(want)))
        .or_else(|| skills.iter().find(|s| s.key.eq_ignore_ascii_case(want)))
}

/// 技能的**正文**，`$ARGUMENTS` 已替换成 `args`。
/// **两路共用**：inline 把它当「注入主对话的指令」，fork 把它当「子代理的任务说明」。
pub fn instruction(skill: &Skill, args: &str) -> String {
    body(&skill.content).replace("$ARGUMENTS", args)
}

/// 找不到技能时的统一报错（两路共用同一句，模型据此纠正拼写）。
fn unknown(skills: &[Skill], want: &str) -> String {
    let keys: Vec<&str> = skills.iter().map(|s| s.key.as_str()).collect();
    format!("Unknown skill \"{want}\". Available: {}", keys.join(", "))
}

/// `Skill` 工具的 schema
pub fn tool_def() -> Value {
    json!({
        "name": "Skill",
        "description": "Load a skill — an instruction set (SKILL.md) shipped with the app or \
            installed by the user. Call this before starting the task whenever the request \
            matches one of the skills listed in the system prompt. There are two kinds: \
            ordinary (inline) skills return instructions that YOU then follow in this \
            conversation; skills marked [subagent] are instead EXECUTED by a sub-agent with \
            its own context, and you get back a report of what it did — so do not redo their \
            work, and treat the report as the result.",
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
        "\n\nAvailable skills — when a request matches one, call the Skill tool with its key. \
         Ordinary skills return instructions that YOU then follow in this conversation; \
         skills marked [subagent] are EXECUTED by a sub-agent instead and you get back a \
         report of what it did:\n",
    );
    let mut used = 0usize;
    let mut listed = 0usize;
    for skill in skills {
        let desc: String = skill.description.chars().take(MAX_DESC_CHARS).collect();
        // fork 技能**必须在清单里标出来**（2026-09-20 A5）：调它拿到的是「子代理的报告」，
        // 不是「一段要自己照做的指令」。标记是每个技能固有的属性 ⇒ 逐字节稳定，不破坏前缀缓存。
        let mark = if skill.fork { " [subagent]" } else { "" };
        let line = if desc.is_empty() {
            format!("- {}{}\n", skill.key, mark)
        } else {
            format!("- {}: {}{}\n", skill.key, desc, mark)
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

/// 执行 **inline** `Skill` 调用：返回该技能正文（`$ARGUMENTS` 已替换）。
///
/// **fork 技能在这里一律拒绝**（2026-09-20 A5）：它的执行入口是 main.rs 的
/// `run_forked_skill()`（要在子代理里跑）。正常路由不会走到这里，所以这个
/// `Err` 是**防路由漏洞的保险**，同时把「为什么拒绝」讲清楚 —— 静默把 fork 技能
/// 当 inline 注入，等于让主循环去干子代理的活，用户看到的是「技能没生效」。
pub fn run(skills: &[Skill], input: &Value) -> Result<String, String> {
    let want = input
        .get("skill")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if want.is_empty() {
        return Err("missing required parameter \"skill\"".into());
    }
    let Some(skill) = find(skills, want) else {
        return Err(unknown(skills, want));
    };
    if skill.fork {
        return Err(format!(
            "Skill \"{}\" runs in a sub-agent (context: fork) and cannot be loaded inline.",
            skill.key
        ));
    }

    let args = input.get("args").and_then(Value::as_str).unwrap_or("");
    Ok(format!(
        "Skill \"{}\" loaded — follow these instructions:\n\n{}{}",
        skill.key,
        instruction(skill, args),
        resources_note(skill)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一条技能（`load()` 的解析路径 + 目录名当 key，只是省掉磁盘）
    fn skill(key: &str, content: &str) -> Skill {
        let fm = parse_frontmatter(content);
        Skill {
            name: if fm.name.is_empty() { key.to_string() } else { fm.name },
            key: key.to_string(),
            description: fm.description,
            fork: fm.fork,
            allowed_tools: fm.allowed_tools,
            resources: Vec::new(),
            resources_capped: false,
            content: content.to_string(),
        }
    }

    /// 真实临时技能目录 —— `collect_resources()` 是唯一走磁盘的部分，没法用内存糊过去。
    /// 名字带进程 id + 递增序号，避免并行测试互踩；调用方自己 `remove_dir_all` 收尾。
    fn temp_skill_dir(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lunac-skill-test-{}-{}-{tag}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(dir: &std::path::Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn inline_doc() -> Skill {
        skill(
            "code-review",
            "---\nname: code-review\ndescription: 审查改动\n---\nReview $ARGUMENTS carefully.\n",
        )
    }

    /// `context: fork` 是唯一开启 fork 的写法；其余（缺省 / 拼错 / 其它值）都是 inline
    #[test]
    fn only_context_fork_enables_fork_mode() {
        assert!(!inline_doc().fork, "没写 context 就是 inline");
        assert!(!skill("a", "---\ncontext: inline\n---\nX").fork);
        assert!(!skill("a", "---\ncontext: Forked\n---\nX").fork, "只认 fork 整词");
        assert!(skill("a", "---\ncontext: fork\n---\nX").fork);
        assert!(skill("a", "---\ncontext: \"fork\"\n---\nX").fork, "去引号");
    }

    /// `allowed-tools` 三种写法都要认；空名单 = 不限制
    #[test]
    fn allowed_tools_accepts_three_separator_styles() {
        let want = vec!["Read".to_string(), "Grep".to_string()];
        for v in ["[Read, Grep]", "Read, Grep", "Read Grep"] {
            let s = skill("a", &format!("---\nallowed-tools: {v}\n---\nX"));
            assert_eq!(s.allowed_tools, want, "{v} 应解析成两个工具");
        }
        assert!(inline_doc().allowed_tools.is_empty(), "没写就是空 = 不限制");
    }

    /// frontmatter 之外的字段不认（旧 CLI 有 model/effort/paths/hooks，Lunac 无落点）
    #[test]
    fn unknown_frontmatter_keys_are_ignored() {
        let s = skill(
            "a",
            "---\nname: A\nmodel: opus\nhooks: x\nargument-hint: [f]\n---\nX",
        );
        assert_eq!(s.name, "A");
        assert!(!s.fork);
        assert!(s.allowed_tools.is_empty());
    }

    /// 查找顺序：key 精确 → name（忽略大小写）→ key（忽略大小写）；空串一律找不到
    #[test]
    fn find_prefers_exact_key_then_name() {
        let list = vec![
            skill("alpha", "---\nname: 别名\n---\nX"),
            skill("beta", "---\nname: alpha\n---\nY"),
        ];
        assert_eq!(find(&list, "alpha").unwrap().key, "alpha", "精确 key 优先于同名 name");
        assert_eq!(find(&list, "ALPHA").unwrap().key, "beta", "name 忽略大小写先于 key 忽略大小写");
        assert_eq!(find(&list, "BETA").unwrap().key, "beta", "最后才退到忽略大小写的 key");
        assert!(find(&list, "  ").is_none(), "空串不该匹配任何技能");
        assert!(find(&list, "nope").is_none());
    }

    /// 正文 = 去掉 frontmatter 的部分，`$ARGUMENTS` 换成入参；两路共用这一份
    #[test]
    fn instruction_strips_frontmatter_and_substitutes_arguments() {
        let s = inline_doc();
        let out = instruction(&s, "src/main.rs");
        assert_eq!(out.trim_end(), "Review src/main.rs carefully.", "{out}");
        assert!(!out.starts_with("---"), "正文里不该残留 frontmatter: {out}");
        // 入参为空 ⇒ 占位符被抹掉，不留下字面量 `$ARGUMENTS`
        assert!(!instruction(&s, "").contains("$ARGUMENTS"));
    }

    /// 没有 `$ARGUMENTS` 的正文原样返回（不能因为「没占位符」就回空串）
    #[test]
    fn instruction_without_placeholder_is_unchanged() {
        let s = skill("a", "---\nname: A\n---\nDo the thing.\n");
        assert_eq!(instruction(&s, "ignored").trim_end(), "Do the thing.");
    }

    /// inline 路径**必须拒绝** fork 技能：静默当 inline 注入 = 让主循环去干子代理的活
    #[test]
    fn run_refuses_fork_skills() {
        let list = vec![
            skill("forked", "---\nname: forked\ncontext: fork\n---\nDo X\n"),
            inline_doc(),
        ];
        let err = run(&list, &json!({ "skill": "forked" })).unwrap_err();
        assert!(err.contains("sub-agent"), "错误要说清是 fork: {err}");
        // inline 技能照常返回指令，且带上自己的 key
        let ok = run(&list, &json!({ "skill": "code-review", "args": "x" })).unwrap();
        assert!(ok.contains("code-review") && ok.contains("Review x carefully"));
        // 找不到 / 缺参
        assert!(run(&list, &json!({ "skill": "ghost" })).unwrap_err().contains("Unknown skill"));
        assert!(run(&list, &json!({})).unwrap_err().contains("missing"));
    }

    /// 清单里的 `[subagent]` 标记是模型唯一的提示 —— 漏标会把「子代理已做完」当指令重做一遍
    #[test]
    fn listing_marks_fork_skills() {
        let list = vec![
            skill("forked", "---\ndescription: 干活的\ncontext: fork\n---\nX"),
            skill("plain", "---\ndescription: 读的\n---\nY"),
        ];
        let out = listing(&list);
        // 抬头必须解释 `[subagent]` 是什么意思 —— 光标记不解释，模型猜不出该期待什么
        assert!(out.contains("[subagent] are EXECUTED by a sub-agent"), "{out}");
        assert!(out.contains("- forked: 干活的 [subagent]"), "{out}");
        assert!(out.contains("- plain: 读的\n"), "{out}");
        assert!(listing(&[]).is_empty(), "空表回空串（调用方保持原提示词）");
    }

    /// 自带资源：只收真文件、**相对路径 + `/` 分隔 + 已排序**，`SKILL.md` 自己不算资源；
    /// 隐藏项与 `node_modules` / `target` 一律跳过（它们不是技能资源，量还极大）
    #[test]
    fn bundled_files_are_collected_relative_and_sorted() {
        let dir = temp_skill_dir("layout");
        put(&dir, "SKILL.md", "---\nname: a\n---\nX");
        put(&dir, "scripts/run.py", "print(1)");
        put(&dir, "assets/tpl.md", "t");
        put(&dir, ".hidden", "x");
        put(&dir, ".git/config", "x");
        put(&dir, "node_modules/pkg/index.js", "x");
        put(&dir, "target/debug/agent.exe", "x");

        let (res, capped) = collect_resources(&dir);
        assert_eq!(res, vec!["assets/tpl.md", "scripts/run.py"], "{res:?}");
        assert!(!capped, "没到上限就不该报截断");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 深度上限：`a/b/c.txt` 是 3 层（留），`a/b/c/d.txt` 是 4 层（丢）
    #[test]
    fn resources_deeper_than_the_limit_are_skipped() {
        let dir = temp_skill_dir("depth");
        put(&dir, "SKILL.md", "---\nname: a\n---\nX");
        put(&dir, "a/b/c.txt", "x");
        put(&dir, "a/b/c/d.txt", "x");

        let (res, _) = collect_resources(&dir);
        assert_eq!(res, vec!["a/b/c.txt"], "{res:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 条数上限：超了要**如实上报**（静默截断会让模型以为「文件就这些」）；
    /// 正好到上限、没有更多时**不许**报「还有没列出的」
    #[test]
    fn resource_list_is_capped_and_flagged() {
        let over = temp_skill_dir("over");
        put(&over, "SKILL.md", "---\nname: a\n---\nX");
        for i in 0..MAX_RESOURCE_FILES + 5 {
            put(&over, &format!("f{i:02}.txt"), "x");
        }
        let (res, capped) = collect_resources(&over);
        assert_eq!(res.len(), MAX_RESOURCE_FILES, "超上限时只留前 N 条");
        assert!(capped, "超出了就必须标记「还有没列出的」");
        std::fs::remove_dir_all(&over).ok();

        let exact = temp_skill_dir("exact");
        put(&exact, "SKILL.md", "---\nname: a\n---\nX");
        for i in 0..MAX_RESOURCE_FILES {
            put(&exact, &format!("f{i:02}.txt"), "x");
        }
        let (res2, capped2) = collect_resources(&exact);
        assert_eq!(res2.len(), MAX_RESOURCE_FILES);
        assert!(!capped2, "正好到上限但没有更多 ⇒ 不报截断");
        std::fs::remove_dir_all(&exact).ok();
    }

    /// 资源清单的措辞：没带文件 ⇒ 一段都不加；带了 ⇒ 必须写清**相对哪个目录**，
    /// 否则模型会把它当成相对工作目录去 `Read`（必然失败）
    #[test]
    fn resources_note_states_the_base_directory() {
        let mut s = skill("code-review", "---\nname: code-review\n---\nX");
        assert_eq!(resources_note_with_base(&s, "C:/sk"), "");

        s.resources = vec!["scripts/run.py".to_string()];
        let note = resources_note_with_base(&s, "C:/sk");
        assert!(note.contains("C:/sk/code-review/"), "要写清相对谁：{note}");
        assert!(note.contains("- scripts/run.py"), "{note}");
        assert!(!note.contains("not listed"), "没截断就不该提：{note}");

        s.resources_capped = true;
        assert!(resources_note_with_base(&s, "C:/sk").contains("not listed"));

        // 根目录带尾部分隔符（Windows 的 `…\skills\`）时不许拼成 `…\skills//x/`
        assert!(
            resources_note_with_base(&s, "C:\\sk\\").contains("C:\\sk/code-review/"),
            "尾部分隔符要先削掉"
        );
    }

    /// 两种模式都要拿到清单：inline 是 `run()` 的返回，fork 由调用方拼进任务说明
    /// （main.rs 的 `run_forked_skill()`）—— 这里是 inline 那一半。
    #[test]
    fn run_appends_bundled_files_note() {
        let mut s = skill("code-review", "---\nname: code-review\n---\nReview $ARGUMENTS.\n");
        s.resources = vec!["scripts/run.py".to_string()];
        let out = run(&[s], &json!({ "skill": "code-review", "args": "x" })).unwrap();
        assert!(out.contains("Review x."), "{out}");
        assert!(out.contains("scripts/run.py"), "{out}");

        let plain = run(&[inline_doc()], &json!({ "skill": "code-review" })).unwrap();
        assert!(!plain.contains("Files bundled"), "没带文件不该多出这段：{plain}");
    }
}
