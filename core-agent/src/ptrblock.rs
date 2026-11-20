// core-agent/src/ptrblock.rs
// 压缩指针回注（2026-10-27，`docs/todo.md` §待办 3-16 P1-7）
//
// **它补的是哪一环**：`compact_history` 丢掉旧消息、瘦掉大块 tool_result 之后，模型手里
// 只剩摘要与结论索引 —— 它知道「查过什么 / 得出了什么」，但**不知道怎么再查一遍**。
// 指针块把被压缩掉的**可重跑**调用（Read / Grep / Glob / Cmd / PowerShell / WebFetch /
// WebSearch）按原参数列出来，模型想复核时照着再发一次即可 —— 重跑命中端点缓存，
// 比它凭记忆猜路径便宜得多。
//
// **来源**：`D:\test\A\ptrblock`（§待办 3-12 三环境实验 A 组的交付物）。移植时只改了两处：
//   ① 打码改用 `crate::log::mask_secrets`（全仓唯一实现，见 `docs/logging.md` §日志 4），
//      不再自带第二套；
//   ② 入参从 `Value` 摊平的活交给调用方（本模块只吃 `(key, value)`）。
//
// **纪律**：纯函数、无 I/O、无全局状态；输出只进 `history`（可变层），**绝不进固定前缀**。

use std::collections::HashSet;

/// 一条被压缩掉的工具调用。`input` 是从 `tool_use.input` 摊平出来的键值对。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elided {
    pub tool: String,
    pub input: Vec<(String, String)>,
}

/// 归一化后的单条记录（可重跑、已打码、已去重）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    tool: String,
    args: Vec<(String, String)>,
}

/// 指针块首行 —— 模型据此认出「这是一份可重跑清单」，不是用户说的话。
pub const HEADER: &str = "[elided pointers]";
/// 单行最多字符数（超出截断并追加 `...`）。
const LINE_MAX_CHARS: usize = 200;
/// 截断标记。
const ELLIPSIS: &str = "...";

/// 只有这些工具算「可重跑」（精确匹配，大小写敏感 —— 与 `tools.rs` 的名字一致）。
///
/// `Agent` / 写类工具 / MCP 工具**一律不算**：前者重跑一次就是再花一份钱，后两者
/// 重跑一次就可能改坏东西 —— 指针块的意义是「让模型敢复核」，不是「让它重做」。
fn is_rerunnable(tool: &str) -> bool {
    matches!(
        tool,
        "Read" | "Grep" | "Glob" | "Cmd" | "PowerShell" | "WebFetch" | "WebSearch"
    )
}

/// 取第一个存在且值非空白的参数值（参数名精确匹配）。
fn arg<'a>(input: &'a [(String, String)], keys: &[&str]) -> &'a str {
    for key in keys {
        if let Some((_, value)) = input.iter().find(|(k, _)| k == key) {
            if !value.trim().is_empty() {
                return value.as_str();
            }
        }
    }
    ""
}

/// 去重用的「关键入参」：同一个工具 + 同一组关键参数 = 同一次查询，只列一次。
fn primary_key(tool: &str, input: &[(String, String)]) -> Option<String> {
    match tool {
        "Read" => {
            let p = arg(input, &["file_path", "path"]);
            (!p.is_empty()).then(|| p.to_string())
        }
        "Grep" => {
            let pattern = arg(input, &["pattern"]);
            if pattern.is_empty() {
                return None;
            }
            let path = arg(input, &["path"]);
            let path = if path.is_empty() { "." } else { path };
            Some(format!("{pattern}\u{1}{path}"))
        }
        "Glob" => {
            let p = arg(input, &["pattern"]);
            (!p.is_empty()).then(|| p.to_string())
        }
        "Cmd" | "PowerShell" => {
            let c = arg(input, &["command", "cmd"]);
            (!c.is_empty()).then(|| c.to_string())
        }
        "WebFetch" => {
            let u = arg(input, &["url"]);
            (!u.is_empty()).then(|| u.to_string())
        }
        "WebSearch" => {
            let q = arg(input, &["query"]);
            (!q.is_empty()).then(|| q.to_string())
        }
        _ => None,
    }
}

/// 归一 + 去重：过滤非可重跑工具、打码、按 (tool, 关键入参) 去重。
///
/// 去重键用**打码前**的原始入参算（打码是输出层的事，不该影响「是不是同一次查询」）。
/// 顺序 = 首次出现顺序（调用方给的是「被丢区间的原顺序」）。
fn normalize(items: &[Elided]) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for item in items {
        if !is_rerunnable(&item.tool) {
            continue;
        }
        let Some(key) = primary_key(&item.tool, &item.input) else {
            continue;
        };
        if !seen.insert((item.tool.clone(), key)) {
            continue;
        }
        out.push(Entry {
            tool: item.tool.clone(),
            args: item
                .input
                .iter()
                .map(|(k, v)| (crate::log::mask_secrets(k), crate::log::mask_secrets(v)))
                .collect(),
        });
    }
    out
}

/// 未截断的渲染行。
fn raw_line(entry: &Entry) -> String {
    match entry.tool.as_str() {
        "Read" => format!("- Read {} (re-runnable)", arg(&entry.args, &["file_path", "path"])),
        "Grep" => {
            let pattern = arg(&entry.args, &["pattern"]);
            let path = arg(&entry.args, &["path"]);
            let path = if path.is_empty() { "." } else { path };
            format!("- Grep \"{pattern}\" in {path} (re-runnable)")
        }
        "Glob" => format!("- Glob \"{}\" (re-runnable)", arg(&entry.args, &["pattern"])),
        "Cmd" => format!("- Cmd {} (re-runnable)", arg(&entry.args, &["command", "cmd"])),
        "PowerShell" => format!(
            "- PowerShell {} (re-runnable)",
            arg(&entry.args, &["command", "cmd"])
        ),
        "WebFetch" => format!("- WebFetch {} (re-runnable)", arg(&entry.args, &["url"])),
        "WebSearch" => format!("- WebSearch \"{}\" (re-runnable)", arg(&entry.args, &["query"])),
        _ => format!("- {} (re-runnable)", entry.tool),
    }
}

/// 单条渲染（按字符数截断到 [`LINE_MAX_CHARS`]）。
fn render_line(entry: &Entry) -> String {
    let line = raw_line(entry);
    if line.chars().count() > LINE_MAX_CHARS {
        let mut t: String = line.chars().take(LINE_MAX_CHARS).collect();
        t.push_str(ELLIPSIS);
        t
    } else {
        line
    }
}

/// 是否与「最近的用户诉求」相关（行或参数值互相包含即算命中）。
///
/// 命中的行排前面：**用户正在问的东西**最可能需要复核。
fn is_recent_hit(line: &str, entry: &Entry, recent: &[String]) -> bool {
    for req in recent {
        if req.is_empty() {
            continue;
        }
        if line.contains(req.as_str()) {
            return true;
        }
        for (_, value) in &entry.args {
            if value.is_empty() {
                continue;
            }
            if req.contains(value.as_str()) || value.contains(req.as_str()) {
                return true;
            }
        }
    }
    false
}

/// 排序 + 预算截断 + 组装。没有条目 ⇒ 空串（调用方据此决定不注入）。
///
/// 排序是**全序**（先按命中、再按行文本），所以同样的输入永远得到同一份输出 ——
/// 指针块进 `history` 后若每轮不一样，就成了「自己打掉自己前缀缓存」的新来源。
fn render_entries(entries: &[Entry], recent: &[String], budget_chars: usize) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let header_len = HEADER.chars().count();
    if budget_chars < header_len {
        return String::new();
    }
    let mut items: Vec<(bool, String)> = entries
        .iter()
        .map(|e| {
            let line = render_line(e);
            let hit = is_recent_hit(&line, e, recent);
            (hit, line)
        })
        .collect();
    items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let mut out = String::from(HEADER);
    let mut out_len = header_len;
    let mut accepted = 0usize;
    for (_, line) in &items {
        let add = 1 + line.chars().count();
        if out_len + add > budget_chars {
            break;
        }
        out.push('\n');
        out.push_str(line);
        out_len += add;
        accepted += 1;
    }
    let remaining = items.len() - accepted;
    if remaining > 0 {
        let tail = format!("\n... (+{remaining} more)");
        let add = tail.chars().count();
        if out_len + add <= budget_chars {
            out.push_str(&tail);
        }
    }
    out
}

/// 渲染指针块：归一 + 去重 → 排序 → 单条截断 → 预算截断。
pub fn render_pointers(items: &[Elided], recent: &[String], budget_chars: usize) -> String {
    render_entries(&normalize(items), recent, budget_chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(tool: &str, pairs: &[(&str, &str)]) -> Elided {
        Elided {
            tool: tool.to_string(),
            input: pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    #[test]
    fn only_rerunnable_tools_are_listed() {
        let items = vec![
            item("Read", &[("file_path", "src/main.rs")]),
            item("Agent", &[("prompt", "do something")]),
            item("Write", &[("file_path", "x.rs")]),
            item("Grep", &[("pattern", "fn main")]),
        ];
        let out = render_pointers(&items, &[], 4_000);
        assert!(out.starts_with(HEADER), "首行必须是指针块表头");
        assert!(out.contains("Read src/main.rs"), "可重跑的 Read 要列出来：{out}");
        assert!(out.contains("Grep \"fn main\" in ."), "Grep 要带上 pattern 与默认 path");
        assert!(!out.contains("Agent"), "子代理重跑一次就是再花一份钱");
        assert!(!out.contains("Write"), "写类工具不该进复核清单");
    }

    #[test]
    fn duplicate_queries_collapse_and_order_is_deterministic() {
        let items = vec![
            item("Read", &[("file_path", "a.rs")]),
            item("Read", &[("file_path", "a.rs")]),
            item("Glob", &[("pattern", "**/*.ts")]),
        ];
        let a = render_pointers(&items, &[], 4_000);
        let b = render_pointers(&items, &[], 4_000);
        assert_eq!(a, b, "同样的输入必须得到同一份输出（否则每轮都在打掉自己的缓存）");
        assert_eq!(a.lines().count(), 3, "表头 + 两条（重复的 a.rs 只留一条）");
    }

    #[test]
    fn secrets_never_reach_the_block() {
        let items = vec![item(
            "Cmd",
            &[("command", "curl -H 'Authorization: Bearer sk-abcdef123456' x")],
        )];
        let out = render_pointers(&items, &[], 4_000);
        assert!(!out.contains("sk-abcdef123456"), "凭据必须被打码：{out}");
        assert!(!out.contains("Bearer sk-"), "Bearer 后面也不许留原值：{out}");
    }

    #[test]
    fn budget_is_respected_and_costs_a_summary_line() {
        // 用不同路径，免得被去重吃掉
        let items: Vec<Elided> = (0..50)
            .map(|i| {
                item(
                    "Read",
                    &[("file_path", &format!("src/f{i}_very_long_file_name.rs"))],
                )
            })
            .collect();
        let out = render_pointers(&items, &[], 300);
        assert!(out.chars().count() <= 300, "总长必须落在预算内：{}", out.chars().count());
        assert!(out.contains("more)"), "被截掉的条目要如实计数：{out}");
    }

    #[test]
    fn empty_input_renders_nothing() {
        assert!(render_pointers(&[], &[], 4_000).is_empty(), "没有条目就不该注入空表头");
        // 预算连表头都放不下 ⇒ 空串
        assert!(render_pointers(&[item("Read", &[("file_path", "a")])], &[], 5).is_empty());
    }

    #[test]
    fn recent_requests_rank_first() {
        let items = vec![
            item("Read", &[("file_path", "aaa.rs")]),
            item("Read", &[("file_path", "src/parser.rs")]),
        ];
        let out = render_pointers(&items, &["parser".to_string()], 4_000);
        let hit = out.find("src/parser.rs").expect("命中项要在");
        let miss = out.find("aaa.rs").expect("未命中项也在");
        assert!(hit < miss, "与最近诉求相关的行必须排在前面：{out}");
    }
}
