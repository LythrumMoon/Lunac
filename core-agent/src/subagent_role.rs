// core-agent/src/subagent_role.rs
// 子代理角色（2026-10-24）：把「派子代理」从一句自由文本，收口成**可枚举的角色**。
//
// 为什么要有它：`Agent` 原先只有一个 `prompt`，工具集是「除少数几件之外的全集」，
// 于是 review / explore 类任务也能写文件、跑任意命令；模型只能从描述里猜自己该用什么工具。
// 角色把三件事绑在一起（与 `tools.rs` 的 `Agent.input_schema` 同源）：
//   ① 工具白名单（`allowed_tools`，单一真相源就是这个模块）；
//   ② 角色提示词（`prompt`，拼进子代理自己的 system，不动主对话前缀）；
//   ③ 可读的短名（`as_str`，进日志 / `task_started` 事件 / 前端框格标题）。
//
// **默认 `General` = 逐字节保持旧行为**（工具集不裁、提示词不追加），老调用方不受影响。

/// 子代理角色。`General` 是缺省值，语义 = 改造前的「无角色」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    General,
    Explore,
    Coding,
    Review,
    Research,
}

/// `Agent.input_schema.role.enum` 的取值（与 [`Role::as_str`] 必须一致）。
pub const ROLE_VALUES: [&str; 5] = ["general", "explore", "coding", "review", "research"];

/// 各角色的工具白名单。`None` = 不裁（沿用 `subagent_tool_defs()` 的结果）。
///
/// 只列**内置工具名**；桥工具（`mcp__*`）与 `Agent` 已在 `subagent_tool_defs()` 里剔除，
/// 这里不重复判。名字必须与 `tools.rs` 的 `defs()` 完全一致（PascalCase）。
const EXPLORE_TOOLS: [&str; 3] = ["Read", "Glob", "Grep"];
const CODING_TOOLS: [&str; 8] =
    ["Read", "Write", "Edit", "NotebookEdit", "Glob", "Grep", "Cmd", "PowerShell"];
/// review 需要跑测试 / 看 diff，所以保留命令类；但**不给写类**（Write / Edit / NotebookEdit）。
const REVIEW_TOOLS: [&str; 5] = ["Read", "Glob", "Grep", "Cmd", "PowerShell"];
const RESEARCH_TOOLS: [&str; 6] =
    ["Read", "Glob", "Grep", "WebSearch", "WebFetch", "WebBrowser"];

impl Role {
    /// 解析模型给的字符串。**大小写不敏感、未知值回落 `General`**（fail-open：
    /// 角色写错不该让整次派发失败；`General` 与改造前一致）。
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "explore" => Role::Explore,
            "coding" => Role::Coding,
            "review" => Role::Review,
            "research" => Role::Research,
            _ => Role::General,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::General => "general",
            Role::Explore => "explore",
            Role::Coding => "coding",
            Role::Review => "review",
            Role::Research => "research",
        }
    }

    /// 该角色的工具白名单；`None` = 不裁剪（只有 `General`）。
    pub fn allowed_tools(self) -> Option<&'static [&'static str]> {
        match self {
            Role::General => None,
            Role::Explore => Some(&EXPLORE_TOOLS),
            Role::Coding => Some(&CODING_TOOLS),
            Role::Review => Some(&REVIEW_TOOLS),
            Role::Research => Some(&RESEARCH_TOOLS),
        }
    }

    /// 回灌给主对话的**报告字符上限**（2026-10-27，`docs/todo.md` §待办 3-16 P0-1）。
    ///
    /// **为什么按角色分级**：`SUBAGENT_REPORT_CHARS` 原来是 8,000 一刀切 —— 对「查一个接口
    /// 在哪」这种 explore 任务宽得离谱，子代理会把过程也写进去。三环境实验（`cache-hit-experiment.md`
    /// §6.4）实测 C 组子代理平均 **2,104 out/请求**，是主线程（687）的 3 倍，而输出单价是
    /// 未命中输入的 4 倍 ⇒ 收口线本身就是成本闸门。
    ///
    /// `General` 保持 8,000 = **逐字节旧行为**（老调用方不受影响）。
    pub const fn report_chars(self) -> usize {
        match self {
            Role::General => 8_000,
            // 只找位置 / 读片段：结论 + 路径就够，写多了必然是过程复述。
            Role::Explore => 1_500,
            // 改动 + 跑了什么 + 结果，三段式本身就占地方。
            Role::Coding => 3_000,
            // 按严重度列 findings + file:line。
            Role::Review => 2_000,
            // 结论 + 出处（URL / 路径），与 coding 同档。
            Role::Research => 3_000,
        }
    }

    /// 该角色是否走「角色常驻」（§待办 3-22 分片 4）。
    ///
    /// **只有具名角色常驻，`General` 不常驻**：`General` 的语义就是「对齐改造前的无角色」，
    /// 它承载的是互不相关的临时任务；把它们的对话串在一条会话里只会互相污染。
    /// 常驻的开关本身在 `main.rs`（`SUBAGENT_RESIDENT`，随 `--checkpoint` 打开）。
    pub fn is_resident(self) -> bool {
        !matches!(self, Role::General)
    }

    /// 工具是否在该角色的白名单里。`General` 一律放行（= 旧行为）。
    pub fn allows(self, tool: &str) -> bool {
        match self.allowed_tools() {
            None => true,
            Some(list) => list.iter().any(|n| *n == tool),
        }
    }

    /// 角色段（拼在通用 `SUBAGENT_SYSTEM` 之后）。英文同 `SUBAGENT_SYSTEM`：给模型看的元信息。
    pub fn prompt(self) -> &'static str {
        match self {
            Role::General => "",
            Role::Explore => {
                "\nRole: explore. You are a read-only exploration subagent. Use Read / Glob / Grep \
to locate code and collect facts. Do NOT modify files and do NOT run commands. Your report must \
list the exact paths (and line ranges when useful), what you found, and what you could not verify."
            }
            Role::Coding => {
                "\nRole: coding. You are the coding subagent. Implement the requested change, run the \
relevant build/tests, and fix what you broke. You may use Write / Edit / Cmd / PowerShell. Your \
report must contain: files changed, commands run with their results, tests passed/failed, and any \
remaining risk or unverified behaviour."
            }
            Role::Review => {
                "\nRole: review. You are the review subagent. You may read code and run non-mutating \
commands (build, test, lint, git diff); you must NOT edit files — you cannot fix anything, so do \
not propose inline patches. Look for correctness bugs, contract violations and missing tests. Your \
report must list findings by severity with exact file:line evidence, and must END with a section \
titled `## unverified` listing every claim you could not check as `- <file:line> — <why>` \
(write `- none` if you checked everything). The main agent reads that section to decide whether \
to rework."
            }
            Role::Research => {
                "\nRole: research. You are the research subagent. Use WebSearch / WebFetch / \
WebBrowser for external facts and Read / Glob / Grep for local context. Do NOT modify files. \
Your report must answer the question, cite the URLs or file paths you relied on, and separate \
verified facts from guesses."
            }
        }
    }
}

// ── 复核回执（§待办 3-22 分片 5，2026-10-31）──────────────────────
//
// 复核角色是**只读**的（工具池里没有 Write / Edit / NotebookEdit，见 `REVIEW_TOOLS`）：
// 它只能「说」，不能「改」。因此它的报告必须能被主线程**直接决策** —— 接受，或者返工。
// 这一段把报告里真正要决策的两件事抽成**确定性**摘要：有多少条 `file:line` 证据、
// 哪几条被报告自己标成了「未验证」。不做语义判断，也不替主线程拍板。

/// 回执里最多列出的未验证条目（上下文预算；超出只报总数）。
pub const MAX_UNVERIFIED_SHOWN: usize = 8;

/// 复核报告的确定性摘要。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReviewDigest {
    /// 含 `file:line` 形态证据的行数。
    pub evidence_lines: usize,
    /// `## unverified` 小节里的前 [`MAX_UNVERIFIED_SHOWN`] 条（原文照录）。
    pub unverified: Vec<String>,
    /// 该小节实际写了多少条（含未显示的部分）。
    pub unverified_total: usize,
    /// 报告里是否存在 `## unverified` 小节。
    pub has_unverified_section: bool,
}

fn is_unverified_heading(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('#') && t.to_ascii_lowercase().contains("unverified")
}

/// 一行里是否有 `path:line` 形态的引用（`src/a.rs:12`、`docs/x.md:7`）。
///
/// 只按「冒号后面全是数字」判定，不校验路径是否存在 —— 回执只报「模型有没有给证据」，
/// 证据本身对不对是 review 的活，不是这段代码的活。
fn cites_file_line(line: &str) -> bool {
    line.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '(' | ')' | '[' | ']' | '{' | '}' | '`' | ',' | ';' | '"' | '\''
            )
    })
    .any(|tok| {
        let Some((path, no)) = tok.rsplit_once(':') else {
            return false;
        };
        !path.is_empty() && !no.is_empty() && no.chars().all(|c| c.is_ascii_digit())
    })
}

fn strip_bullet(line: &str) -> &str {
    let t = line.trim();
    for prefix in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(prefix) {
            return rest.trim();
        }
    }
    // `1. foo` / `2) foo` 这类编号列表。
    let numbered = t
        .strip_prefix(|c: char| c.is_ascii_digit())
        .and_then(|r| r.strip_prefix(". ").or_else(|| r.strip_prefix(") ")));
    numbered.unwrap_or(t).trim()
}

fn is_none_marker(item: &str) -> bool {
    let t = item.trim().trim_end_matches('.').to_ascii_lowercase();
    matches!(t.as_str(), "none" | "n/a" | "无" | "没有" | "无未验证项")
}

/// 解析复核报告。**只认结构，不认措辞**：小节标题含 `unverified` 即可。
pub fn review_digest(report: &str) -> ReviewDigest {
    let mut d = ReviewDigest {
        evidence_lines: report.lines().filter(|l| cites_file_line(l)).count(),
        ..ReviewDigest::default()
    };
    let mut in_section = false;
    for line in report.lines() {
        if is_unverified_heading(line) {
            in_section = true;
            d.has_unverified_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        // 退到下一个标题就结束（报告可能还有别的段落）。
        if line.trim_start().starts_with('#') {
            break;
        }
        let item = strip_bullet(line);
        if item.is_empty() || is_none_marker(item) {
            continue;
        }
        d.unverified_total += 1;
        if d.unverified.len() < MAX_UNVERIFIED_SHOWN {
            d.unverified.push(item.to_string());
        }
    }
    d
}

/// 把复核报告包成「主线程可直接决策」的形态：回执在前，报告原文在后。
///
/// 回执固定三件事 —— **这一步没改任何文件**、**有多少 file:line 证据**、**哪些结论未验证**，
/// 最后一句明确「要不要返工由你决定」。字数控制在一屏内（报告本身另有角色上限）。
pub fn review_receipt(report: &str) -> String {
    let d = review_digest(report);
    let mut out = String::new();
    out.push_str("[review receipt] read-only reviewer — it cannot edit files, so nothing was changed by this step.\n");
    if d.evidence_lines == 0 {
        out.push_str("file:line evidence: none — treat every finding below as unverified.\n");
    } else {
        out.push_str(&format!(
            "file:line evidence: {} line(s).\n",
            d.evidence_lines
        ));
    }
    if !d.has_unverified_section {
        out.push_str(
            "unverified: NOT DECLARED (no `## unverified` section) — treat every claim below as unverified.\n",
        );
    } else if d.unverified_total == 0 {
        out.push_str("unverified: none declared.\n");
    } else {
        out.push_str(&format!("unverified ({}):\n", d.unverified_total));
        for item in &d.unverified {
            out.push_str(&format!("- {item}\n"));
        }
        if d.unverified_total > d.unverified.len() {
            out.push_str(&format!(
                "- …(+{} more, see the report)\n",
                d.unverified_total - d.unverified.len()
            ));
        }
    }
    out.push_str(
        "DECISION: you decide whether to rework — dispatch role=coding with the exact file:line list, or accept and say so.\n\n",
    );
    out.push_str(report);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_is_case_insensitive_and_defaults_to_general() {
        assert_eq!(Role::parse("Explore"), Role::Explore);
        assert_eq!(Role::parse(" CODING "), Role::Coding);
        assert_eq!(Role::parse("review"), Role::Review);
        assert_eq!(Role::parse("research"), Role::Research);
        assert_eq!(Role::parse(""), Role::General);
        assert_eq!(Role::parse("写代码"), Role::General, "未知值 fail-open 到旧行为");
    }

    #[test]
    fn general_allows_everything_and_others_are_scoped() {
        for t in ["Read", "Write", "Cmd", "WebSearch", "McpX"] {
            assert!(Role::General.allows(t), "General 不裁工具：{t}");
        }
        assert!(Role::Explore.allows("Grep"));
        assert!(!Role::Explore.allows("Write"));
        assert!(!Role::Explore.allows("Cmd"));
        assert!(Role::Coding.allows("Edit"));
        assert!(Role::Coding.allows("Cmd"));
        assert!(!Role::Coding.allows("WebSearch"), "coding 不给联网工具");
        assert!(Role::Review.allows("Cmd"));
        assert!(!Role::Review.allows("Write"), "review 不许改文件");
        assert!(Role::Research.allows("WebFetch"));
        assert!(!Role::Research.allows("Write"));
    }

    #[test]
    fn every_role_has_a_stable_name_and_prompt() {
        for r in [Role::General, Role::Explore, Role::Coding, Role::Review, Role::Research] {
            assert!(ROLE_VALUES.contains(&r.as_str()), "名字必须进 schema 枚举：{}", r.as_str());
        }
        assert!(Role::General.prompt().is_empty(), "General 不追加提示词（旧行为）");
        for r in [Role::Explore, Role::Coding, Role::Review, Role::Research] {
            assert!(r.prompt().contains("report"), "{} 必须约定报告要求", r.as_str());
        }
    }

    #[test]
    fn only_named_roles_are_resident() {
        assert!(
            !Role::General.is_resident(),
            "general 必须保持改造前的「跑完即散」"
        );
        for r in [Role::Explore, Role::Coding, Role::Review, Role::Research] {
            assert!(r.is_resident(), "{} 应该复用上一段会话", r.as_str());
        }
    }

    #[test]
    fn review_digest_counts_file_line_evidence_and_unverified_items() {
        let report = "Findings:\n\
- HIGH: unwrap on an empty list — src/tools.rs:120\n\
- LOW: stale comment — docs/x.md:7\n\
## unverified\n\
- src/main.rs:900 — the integration test could not be run\n\
- platform behaviour on macOS\n";
        let d = review_digest(report);
        assert_eq!(d.evidence_lines, 3, "三行带 file:line（含未验证那条）");
        assert!(d.has_unverified_section);
        assert_eq!(d.unverified_total, 2);
        assert_eq!(
            d.unverified[0],
            "src/main.rs:900 — the integration test could not be run"
        );
        assert_eq!(d.unverified[1], "platform behaviour on macOS");
    }

    #[test]
    fn review_digest_treats_an_explicit_none_as_empty() {
        let d = review_digest("Findings: none.\n## Unverified\n- none\n");
        assert!(d.has_unverified_section, "标题大小写不敏感");
        assert_eq!(d.unverified_total, 0);
        assert!(d.unverified.is_empty());
    }

    #[test]
    fn review_digest_flags_a_missing_unverified_section() {
        let d = review_digest("Everything looks fine to me.");
        assert!(!d.has_unverified_section, "没有小节就必须能看出来");
        assert_eq!(d.evidence_lines, 0);
        let receipt = review_receipt("Everything looks fine to me.");
        assert!(receipt.contains("NOT DECLARED"), "{receipt}");
        assert!(receipt.contains("file:line evidence: none"), "{receipt}");
        assert!(receipt.contains("DECISION:"), "主线程要看到「你来决定」");
    }

    #[test]
    fn review_digest_shows_at_most_eight_items_but_counts_all() {
        let mut report = String::from("## unverified\n");
        for i in 1..=10 {
            report.push_str(&format!("- src/a.rs:{i} — reason {i}\n"));
        }
        let d = review_digest(&report);
        assert_eq!(d.unverified.len(), MAX_UNVERIFIED_SHOWN);
        assert_eq!(d.unverified_total, 10, "总数要如实报，不能只报显示出来的");
        let receipt = review_receipt(&report);
        assert!(receipt.contains("unverified (10)"), "{receipt}");
        assert!(
            receipt.contains("+2 more"),
            "被折叠的 2 条要有交代: {receipt}"
        );
    }

    #[test]
    fn review_digest_stops_at_the_next_heading() {
        let d = review_digest("## unverified\n- a.rs:1 — x\n## next\n- b.rs:2 — y\n");
        assert_eq!(d.unverified_total, 1, "下一个标题之后的条目不算");
    }

    #[test]
    fn report_caps_are_graded_but_general_keeps_the_old_line() {
        assert_eq!(Role::General.report_chars(), 8_000, "General 必须与改造前逐字节一致");
        assert!(
            Role::Explore.report_chars() < Role::Review.report_chars(),
            "explore 的报告上限必须比 review 更紧"
        );
        assert!(
            Role::Review.report_chars() < Role::Coding.report_chars(),
            "review 只列 findings，不该比 coding 宽"
        );
        for r in [Role::Explore, Role::Coding, Role::Review, Role::Research] {
            assert!(
                r.report_chars() <= 3_000,
                "{} 的报告上限必须收口到 3k 以内（§待办 3-16 P0-1）",
                r.as_str()
            );
        }
    }
}
