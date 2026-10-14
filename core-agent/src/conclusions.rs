// core-agent/src/conclusions.rs
// 可恢复结论索引（2026-10-14）—— 解决「上下文压缩丢结论 ⇒ 模型回到初始问题重答」。
//
// 问题：`compact_history` 的 `Drop` / `Force` 档会把历史中段整条 `drain` 掉。被丢的那段
// 里往往正躺着「用户一路想要什么」「哪条路试过、为什么失败」「结论落在哪个文件」—— 丢完后
// 模型手里只剩开头那条最早的提问 + 尾部，于是把**最早那个诉求**当成当前任务重做（用户报障
// 原话：「只要压缩上下文直接回到了上一次人格修改」）。
//
// 解法（用户裁决，2026-10-14）：压缩边界把「即将被丢的内容」压成若干条**结论**写进一个
// 追加式索引；压缩完成后按当前任务检索一次、重排、把 top-N 注入到**可变层**（紧随压缩摘要
// 之后），于是模型拿回的是「结论 + 可恢复指针」，不必重做。
//
// 三条硬口径（用户裁决，不得推翻）：
//   · **失败不删**（Manus「Keep the Wrong Stuff In」）：失败尝试用 `status=failed` + `reason`
//     标注（被后续证据推翻的用 `rejected`），**绝不删除** —— 删掉失败会移除模型纠错所需的证据。
//   · **不加预算**（Chroma《Context Rot》）：本模块只做「检索 + 重排 + 有上限的注入」，
//     不碰 `MAX_CONTEXT` / 任何预算常量。
//   · **写入时机只落在压缩边界**：不随每次工具调用写（那会频繁变动前缀、打爆 prompt cache）。
//
// 落点与为什么不在 `chat.db`：`chat.db` 只存前端展示的 user/assistant 纯文本，**不含
// tool_use / tool_result** ⇒ 那里根本没有可检索的工具结论。所以索引落在 agent 自己的根下
// （`<exe 根>\temp\conclusions\<scope>.jsonl`，与 `temp\review-last` 同一套推导），
// **按 scope 分家**（global / pet 各一份，互不掺入）。

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::log;
use crate::tools::MemoryScope;

/// `status` 三态。语义见各常量注释。
pub const STATUS_VERIFIED: &str = "verified";
pub const STATUS_FAILED: &str = "failed";
pub const STATUS_REJECTED: &str = "rejected";

/// 模型结构化段的分隔标记（给模型看的元信息，英文，同 `TRIMMED_MARKER` 的理由）。
/// 模型把这段 JSON 追加在摘要末尾；我们**从中切出并剥离**，钉回历史的摘要里不留它。
pub const INDEX_BEGIN: &str = "===LUNAC-CONCLUSIONS-BEGIN===";
pub const INDEX_END: &str = "===LUNAC-CONCLUSIONS-END===";

/// 读盘时最多保留的条数（单 scope）。追加式文件随时间增长，读取时只留最近这一段 ——
/// 这**不是上下文预算**（用户裁决：不加预算），只是把「读全文件」的内存占用封顶。
const MAX_LOAD_ENTRIES: usize = 2_000;

/// 可恢复指针：`kind` ∈ file | url | query | commit | command，`value` 是该指针的值。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ref {
    pub kind: String,
    pub value: String,
}

/// 一条结论（JSONL 的一行）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conclusion {
    /// epoch 秒（检索重排的 recency 项要用）。
    pub ts: i64,
    /// 作用域（`global` / `pet`）—— 索引本身就按 scope 分文件，这里冗余一份便于人工排查。
    pub scope: String,
    /// 写入时的第几次用户提问（0 = 手动压缩等无提问号场景）。
    pub turn: u64,
    /// 一句话结论。
    pub conclusion: String,
    /// 精确标识符优先（文件名 / 函数名 / URL / 错误码 / 命令）。
    #[serde(default)]
    pub keywords: Vec<String>,
    /// 可恢复指针。
    #[serde(default)]
    pub refs: Vec<Ref>,
    /// `verified` | `failed` | `rejected`。
    pub status: String,
    /// 失败原因（`status=failed` 时填）。
    #[serde(default)]
    pub reason: String,
    /// 会话标识（复用 agent 既有的 `session_id()`）。
    pub session_id: String,
}

/// 未带元信息的一条结论（模型结构化段 / 确定性抽取的中间产物）；
/// 写入前由 [`Meta`] 补齐 `ts` / `scope` / `turn` / `session_id`。
#[derive(Clone, Debug)]
pub struct Partial {
    pub status: String,
    pub conclusion: String,
    pub keywords: Vec<String>,
    pub refs: Vec<Ref>,
    pub reason: String,
}

/// 写入元信息。
pub struct Meta<'a> {
    pub ts: i64,
    pub scope: MemoryScope,
    pub turn: u64,
    pub session_id: &'a str,
}

impl Meta<'_> {
    fn materialize(&self, p: Partial) -> Conclusion {
        // 落盘前过 `mask_secrets`（复用宿主/agent 既有的**唯一**脱敏实现，不另造一份）：
        // 结论 / 原因 / 指针的值可能含凭据（如命令里带了 token），索引不该成为新的凭据面。
        let mask = |s: &str| log::mask_secrets(s);
        // keywords 同样可能含凭据（例如命令 / token 串被抽成标识符）⇒ 一并过脱敏。
        let mut keywords: Vec<String> =
            p.keywords.into_iter().map(|k| mask(&k)).collect();
        keywords.sort();
        keywords.dedup();
        keywords.truncate(16);
        Conclusion {
            ts: self.ts,
            scope: self.scope.as_str().to_string(),
            turn: self.turn,
            conclusion: mask(&p.conclusion),
            keywords,
            refs: p
                .refs
                .into_iter()
                .map(|r| Ref { kind: r.kind, value: mask(&r.value) })
                .collect(),
            status: p.status,
            reason: mask(&p.reason),
            session_id: self.session_id.to_string(),
        }
    }
}

// ── 落点与 I/O ───────────────────────────────────────────────────

/// 索引目录：`<exe 根>\temp\conclusions`（与 `review-last` 同一套推导：`log_dir()` 的父目录）。
pub fn default_dir() -> PathBuf {
    crate::log::log_dir()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("conclusions")
}

/// 某一作用域的索引文件路径（按 scope 分家）。
pub fn path_for(dir: &Path, scope: MemoryScope) -> PathBuf {
    dir.join(format!("{}.jsonl", scope.as_str()))
}

/// 去重键：同一条结论（同状态 + 同正文）在追加式文件里只留一条，避免每次压缩重复写入。
fn dedup_key(c: &Conclusion) -> String {
    format!("{}\u{1}{}", c.status, c.conclusion)
}

/// 读取某一作用域的全部结论（坏行跳过；只留最近 `MAX_LOAD_ENTRIES` 条）。
pub fn load(dir: &Path, scope: MemoryScope) -> Vec<Conclusion> {
    let path = path_for(dir, scope);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out: Vec<Conclusion> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // 坏行（被截断 / 手改）一律跳过 —— 索引是锦上添花，读不出就当没有，绝不阻断主流程。
        if let Ok(c) = serde_json::from_str::<Conclusion>(line) {
            out.push(c);
        }
    }
    if out.len() > MAX_LOAD_ENTRIES {
        out = out.split_off(out.len() - MAX_LOAD_ENTRIES);
    }
    out
}

/// 追加写入一批结论（append-only JSONL；重复项跳过）。失败只 warn、绝不影响主流程。
///
/// 日志只记**条数**，**不记正文**（结论 / 指针可能含敏感信息）。
pub fn append(dir: &Path, scope: MemoryScope, entries: &[Conclusion]) {
    if entries.is_empty() {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(dir) {
        log::warn(format!("结论索引建目录失败（{}）：{e}", dir.display()));
        return;
    }
    let path = path_for(dir, scope);
    let mut seen: HashSet<String> =
        load(dir, scope).iter().map(dedup_key).collect();
    let mut buf = String::new();
    let mut added = 0usize;
    let mut skipped = 0usize;
    for e in entries {
        if !seen.insert(dedup_key(e)) {
            skipped += 1;
            continue;
        }
        match serde_json::to_string(e) {
            Ok(line) => {
                buf.push_str(&line);
                buf.push('\n');
                added += 1;
            }
            Err(err) => log::warn(format!("结论索引序列化失败（已跳过）：{err}")),
        }
    }
    if buf.is_empty() {
        log::info(format!(
            "结论索引写入：scope={} 无新增（跳过重复 {skipped} 条）",
            scope.as_str()
        ));
        return;
    }
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut f) => {
            if let Err(e) = f.write_all(buf.as_bytes()).and_then(|_| f.flush()) {
                log::warn(format!("结论索引写入失败（{}）：{e}", path.display()));
                return;
            }
        }
        Err(e) => {
            log::warn(format!("结论索引打开失败（{}）：{e}", path.display()));
            return;
        }
    }
    log::info(format!(
        "结论索引写入：scope={} 新增 {added} 条（跳过重复 {skipped} 条）",
        scope.as_str()
    ));
}

// ── 确定性抽取（不调模型，兜底路径也写索引）─────────────────────────

/// 从一组历史消息里抽取结论（确定性，不调模型）。
///
/// 三条来源：
///   · **用户文本诉求** ⇒ `verified`：结论 = 那句话（「用户一路想要什么」的最直接载体）；
///   · **失败的工具尝试** ⇒ `failed` + `reason`：结论 = 试了什么，原因 = 错误文本
///     （`is_error` 的 `tool_result`；Manus 口径：失败是模型纠错所需的证据，不得删）；
///   · **成功的写类工具**（Write / Edit / NotebookEdit）⇒ `verified`：结论 = 改了哪个文件。
///
/// 只抽这三类（而非每条工具调用）是刻意的：**结论索引不是流水账**，抽太多等于把被丢的
/// 内容按更差的形态搬回来，还挤占注入预算。
pub fn extract_from_dropped(dropped: &[Value], meta: &Meta) -> Vec<Conclusion> {
    // 先建 id → (工具名, 入参) 与 id → 错误文本 的索引（tool_result 自己只带 tool_use_id）。
    let mut calls: HashMap<String, (String, Value)> = HashMap::new();
    let mut failures: HashMap<String, String> = HashMap::new();
    for msg in dropped {
        let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
            continue;
        };
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    if let (Some(id), Some(name)) = (
                        block.get("id").and_then(Value::as_str),
                        block.get("name").and_then(Value::as_str),
                    ) {
                        let input = block.get("input").cloned().unwrap_or(Value::Null);
                        calls.insert(id.to_string(), (name.to_string(), input));
                    }
                }
                Some("tool_result") => {
                    if block.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
                        if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                            failures.insert(id.to_string(), tool_result_text(block));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let mut out: Vec<Conclusion> = Vec::new();
    for msg in dropped {
        let is_user = msg.get("role").and_then(Value::as_str) == Some("user");
        let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
            continue;
        };
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") if is_user => {
                    let Some(t) = block.get("text").and_then(Value::as_str) else {
                        continue;
                    };
                    let one = t.split_whitespace().collect::<Vec<_>>().join(" ");
                    // 合成消息（TRIMMED_MARKER / 摘要头 / 任务快照 / 本索引的注入块 / 以 `#` 开头的
                    // 上下文块）都要跳过 —— 它们不是「用户的话」。
                    if one.is_empty() || one.starts_with('[') || one.starts_with('#') {
                        continue;
                    }
                    let ask: String = one.chars().take(300).collect();
                    let keywords = extract_identifiers(&ask);
                    let refs = refs_from_text(&ask);
                    out.push(meta.materialize(Partial {
                        status: STATUS_VERIFIED.to_string(),
                        conclusion: ask,
                        keywords,
                        refs,
                        reason: String::new(),
                    }));
                }
                Some("tool_use") => {
                    let id = block.get("id").and_then(Value::as_str).unwrap_or("");
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    if let Some(reason) = failures.get(id) {
                        let args = crate::tools::summarize_args(&input);
                        let reason: String = reason.chars().take(300).collect();
                        out.push(meta.materialize(Partial {
                            status: STATUS_FAILED.to_string(),
                            conclusion: format!("Attempted {name}({args}) — did not work"),
                            keywords: extract_identifiers(&args),
                            refs: refs_from_input(&input),
                            reason,
                        }));
                    } else if matches!(name, "Write" | "Edit" | "NotebookEdit") {
                        let refs = refs_from_input(&input);
                        let target = refs
                            .iter()
                            .find(|r| r.kind == "file")
                            .map(|r| r.value.clone())
                            .unwrap_or_else(|| name.to_string());
                        out.push(meta.materialize(Partial {
                            status: STATUS_VERIFIED.to_string(),
                            conclusion: format!("{name} applied to {target}"),
                            keywords: extract_identifiers(&target),
                            refs,
                            reason: String::new(),
                        }));
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// 从被瘦身（elide）的可重现工具结果里抽「查过什么」的指针条目。
///
/// 传入的每个 `Value` 是一条**合成的 tool_use 消息**（工具名 + 入参）—— 由 `compact_history`
/// 在把结果替换成占位串前抄下来。为什么也要抽：结果文本虽可重跑拿回，但「查过哪里」这个
/// **指针**本身是有价值的（重跑时知道去哪跑）。`status=verified`（当时确实跑成了）。
pub fn extract_from_elided(elided: &[Value], meta: &Meta) -> Vec<Conclusion> {
    let mut out: Vec<Conclusion> = Vec::new();
    for msg in elided {
        let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
            let input = block.get("input").cloned().unwrap_or(Value::Null);
            let refs = refs_from_input(&input);
            let args = crate::tools::summarize_args(&input);
            out.push(meta.materialize(Partial {
                status: STATUS_VERIFIED.to_string(),
                conclusion: format!("{name} consulted earlier (result elided; re-run to recover)"),
                keywords: extract_identifiers(&args),
                refs,
                reason: String::new(),
            }));
        }
    }
    out
}

/// `tool_result` 的文本（字符串或文本块数组），截断到 400 字 —— 只给失败原因用。
fn tool_result_text(block: &Value) -> String {
    let joined = match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter(|x| x.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|x| x.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    joined.trim().chars().take(400).collect()
}

/// 从工具入参里抽可恢复指针（`file` / `url` / `query` / `command` / `commit`）。
fn refs_from_input(input: &Value) -> Vec<Ref> {
    let mut refs: Vec<Ref> = Vec::new();
    let Value::Object(map) = input else {
        return refs;
    };
    for (k, v) in map {
        let Some(s) = v.as_str() else { continue };
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        let kind = match k.as_str() {
            "file_path" | "path" | "notebook_path" | "directory" | "dir" => "file",
            "url" | "endpoint" => "url",
            "command" | "cmd" | "script" => "command",
            "pattern" | "query" | "glob" | "search" => "query",
            "commit" | "sha" => "commit",
            _ => continue,
        };
        refs.push(Ref { kind: kind.to_string(), value: s.chars().take(200).collect() });
    }
    refs
}

/// 从一段自由文本里抽指针：识别含路径分隔符 / 常见源码扩展名的 token 当作文件指针。
fn refs_from_text(text: &str) -> Vec<Ref> {
    let mut refs: Vec<Ref> = Vec::new();
    for tok in text.split_whitespace() {
        let t = tok.trim_matches(|c: char| {
            matches!(c, ',' | ';' | '.' | ':' | '(' | ')' | '"' | '\'' | '[' | ']' | '{' | '}' | '<' | '>' | '，' | '。' | '、')
        });
        if t.chars().count() < 4 {
            continue;
        }
        let looks_path = t.contains('/')
            || t.contains('\\')
            || ["rs", "ts", "js", "json", "toml", "md", "py", "ps1", "css", "html", "db", "nsi"]
                .iter()
                .any(|ext| t.to_ascii_lowercase().ends_with(&format!(".{ext}")));
        if looks_path {
            refs.push(Ref { kind: "file".to_string(), value: t.chars().take(200).collect() });
        }
    }
    refs.truncate(8);
    refs
}

/// 精确标识符抽取（BM25 之外的「字面 / 标识符」通道）：含路径分隔符 / 下划线 / 点扩展名 /
/// 内部大小写切换 / 字母数字混排的 token。**低价值 token 一律不抽**（避免噪音词进检索）。
pub fn extract_identifiers(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in s.split_whitespace() {
        let tok = raw.trim_matches(|c: char| {
            matches!(c, ',' | ';' | '.' | ':' | '(' | ')' | '"' | '\'' | '[' | ']' | '{' | '}' | '<' | '>' | '，' | '。' | '、')
        });
        if tok.chars().count() < 4 {
            continue;
        }
        let has_dot_ext = tok.contains('.');
        let has_sep = tok.contains('/') || tok.contains('\\') || tok.contains("::");
        let snake = tok.contains('_');
        let mixed = tok.chars().any(|c| c.is_ascii_alphabetic())
            && tok.chars().any(|c| c.is_ascii_digit());
        let camel = tok
            .as_bytes()
            .windows(2)
            .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase());
        if has_dot_ext || has_sep || snake || mixed || camel {
            out.push(tok.to_lowercase());
        }
    }
    out.sort();
    out.dedup();
    out
}

// ── 模型结构化段解析 ─────────────────────────────────────────────

/// 从模型摘要里切出结构化结论段：返回 `(去掉该段后的摘要, 结论)`。
///
/// 模型把 JSON 数组夹在 `INDEX_BEGIN` / `INDEX_END` 之间（见 `SUMMARY_PROMPT` 的追加段）。
/// 拿不到标记 / 解析不出 ⇒ 原样返回摘要、结论为空（调用方据此退到确定性抽取）。
pub fn parse_model_block(text: &str, meta: &Meta) -> (String, Vec<Conclusion>) {
    let Some(b) = text.find(INDEX_BEGIN) else {
        return (text.trim().to_string(), Vec::new());
    };
    let after = &text[b + INDEX_BEGIN.len()..];
    let Some(e) = after.find(INDEX_END) else {
        return (text.trim().to_string(), Vec::new());
    };
    let json_part = &after[..e];
    let summary = text[..b].trim().to_string();
    (summary, parse_entries_json(json_part, meta))
}

/// 解析结构化段里的 JSON 数组；容错：取第一个 `[` 到最后一个 `]`（忽略可能的说明文字 / 围栏）。
fn parse_entries_json(raw: &str, meta: &Meta) -> Vec<Conclusion> {
    let Some(start) = raw.find('[') else {
        return Vec::new();
    };
    let Some(end) = raw.rfind(']') else {
        return Vec::new();
    };
    if end <= start {
        return Vec::new();
    }
    let Ok(arr) = serde_json::from_str::<Value>(&raw[start..=end]) else {
        return Vec::new();
    };
    let Some(items) = arr.as_array() else {
        return Vec::new();
    };
    let mut out: Vec<Conclusion> = Vec::new();
    for item in items {
        let status = item
            .get("status")
            .and_then(Value::as_str)
            .map(normalize_status)
            .unwrap_or_else(|| STATUS_VERIFIED.to_string());
        let conclusion = item
            .get("conclusion")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if conclusion.is_empty() {
            continue;
        }
        let keywords = item
            .get("keywords")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // keywords 缺失时从正文兜底抽一份，保证检索的「标识符通道」有料。
        let keywords = if keywords.is_empty() {
            extract_identifiers(&conclusion)
        } else {
            keywords
        };
        let refs = item
            .get("refs")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|r| {
                        if let Some(s) = r.as_str() {
                            let (k, v) = s.split_once(':')?;
                            return Some(Ref { kind: k.trim().to_string(), value: v.trim().to_string() });
                        }
                        let kind = r.get("kind").and_then(Value::as_str)?.trim().to_string();
                        let value = r.get("value").and_then(Value::as_str)?.trim().to_string();
                        if value.is_empty() {
                            return None;
                        }
                        Some(Ref { kind, value })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let reason = item
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        out.push(meta.materialize(Partial { status, conclusion, keywords, refs, reason }));
    }
    out.truncate(24);
    out
}

fn normalize_status(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "failed" | "fail" | "error" => STATUS_FAILED.to_string(),
        "rejected" | "superseded" | "invalid" => STATUS_REJECTED.to_string(),
        _ => STATUS_VERIFIED.to_string(),
    }
}

// ── 检索 + 重排（Generative Agents: recency × importance × relevance）──────
//
// ⚠️ **诚实边界：未实现向量通道，原因：本仓无可用 embedding 源。**
//   已核实：`core-agent/Cargo.toml` 无任何嵌入 / 向量依赖；grep 全仓无 embeddings 端点调用；
//   供应商配置（`config\ai.json` 的端点是 chat/completions 形态）也不含 embeddings API。
//   因此本模块**只做字面 / 标识符通道**（BM25 + 标识符匹配），**不做**向量语义检索 ——
//   绝不假装做了。若日后接入 embedding 源，应在此处补「向量通道 → RRF 融合」，其余不必动。

/// BM25 参数（标准取值）。
const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

/// recency 时间衰减（Generative Agents 用的是 0.995 / 小时，但那是对「长期记忆流」的尺度；
/// 本索引以会话内为主、条目很新，故取更陡的 **0.99 / 小时** ⇒ 24 小时后约 0.79）。
const RECENCY_DECAY_PER_HOUR: f64 = 0.99;

/// 注入闸门：`recency × importance × relevance` 低于此值视为「与当前任务毫不相关」，不注入。
/// 取小值即可 —— 乘积里 relevance=0 时整体为 0，这条闸主要是滤掉「纯靠 importance/recency 凑分」
/// 的噪声（relevance ≥ 一点的条目几乎都远大于此值）。
const MIN_RETRIEVAL_SCORE: f64 = 0.05;

/// 检索 + 重排：返回按分数降序的结论。
///
/// 打分（Generative Agents 的 `recency × importance × relevance`，各分量归一到 [0,1]）：
///   `score = recency × importance × relevance`
///   · `recency   = RECENCY_DECAY_PER_HOUR ^ (age_hours)`（现在时刻传入，便于单测）
///   · `importance` = 启发式（本仓没有给条目打 1–10 分的模型调用，**如实降级**为启发式）：
///       基 0.4；`failed` +0.3（失败证据要保住）；有指针 +0.2；有关键词 +0.1；上限 1.0
///   · `relevance  = 0.6 × BM25_norm + 0.4 × 标识符重合率`
///       —— BM25 抓字面 / 词面；标识符重合率抓「精确标识符优先」（文件名 / 函数名 / 命令）。
///
/// **用乘积而非加权和**：乘积让 relevance=0 的条目直接归零被滤掉 —— 加权和会让「很新、很重要
/// 但毫不相关」的条目仅凭 recency+importance 就过闸（那等于每次压缩都灌一堆无关结论）。
pub fn retrieve(
    dir: &Path,
    scope: MemoryScope,
    query: &str,
    now: i64,
    top_n: usize,
    max_chars: usize,
) -> Vec<Conclusion> {
    let entries = load(dir, scope);
    if entries.is_empty() {
        return Vec::new();
    }
    let q_terms = tokenize(query);
    let q_ids: HashSet<String> = extract_identifiers(query).into_iter().collect();

    // BM25：语料 = 每条结论的（正文 + 关键词）分词。
    let docs: Vec<Vec<String>> = entries
        .iter()
        .map(|e| {
            let mut s = e.conclusion.clone();
            s.push(' ');
            s.push_str(&e.keywords.join(" "));
            tokenize(&s)
        })
        .collect();
    let avgdl = docs.iter().map(|d| d.len()).sum::<usize>() as f64 / docs.len() as f64;
    let mut df: HashMap<&str, usize> = HashMap::new();
    for d in &docs {
        let uniq: HashSet<&str> = d.iter().map(String::as_str).collect();
        for t in uniq {
            *df.entry(t).or_insert(0) += 1;
        }
    }
    let n = entries.len() as f64;
    let idf = |t: &str| {
        let d = *df.get(t).unwrap_or(&0) as f64;
        // BM25 的 +0.5 平滑；d 不可能为 0（term 来自某条 doc），公式对齐标准实现
        ((n - d + 0.5) / (d + 0.5) + 1.0).ln()
    };
    let raw: Vec<f64> = entries
        .iter()
        .zip(docs.iter())
        .map(|(_, doc)| {
            let dl = doc.len() as f64;
            let mut tf: HashMap<&str, usize> = HashMap::new();
            for t in doc {
                *tf.entry(t.as_str()).or_insert(0) += 1;
            }
            let mut score = 0.0;
            for qt in &q_terms {
                let f = *tf.get(qt.as_str()).unwrap_or(&0) as f64;
                if f == 0.0 {
                    continue;
                }
                let denom = f + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / avgdl.max(1.0));
                score += idf(qt) * (f * (BM25_K1 + 1.0)) / denom.max(1e-9);
            }
            score
        })
        .collect();
    let max_raw = raw.iter().cloned().fold(0.0f64, f64::max);

    let mut scored: Vec<(f64, Conclusion)> = entries
        .into_iter()
        .zip(raw.into_iter())
        .map(|(e, r)| {
            let relevance = {
                let bm25_norm = if max_raw > 0.0 { r / max_raw } else { 0.0 };
                let e_ids: HashSet<String> = e
                    .keywords
                    .iter()
                    .map(|k| k.to_lowercase())
                    .chain(extract_identifiers(&e.conclusion))
                    .collect();
                let overlap = if q_ids.is_empty() {
                    0.0
                } else {
                    q_ids.intersection(&e_ids).count() as f64 / q_ids.len() as f64
                };
                0.6 * bm25_norm + 0.4 * overlap
            };
            let age_hours = ((now - e.ts).max(0) as f64) / 3600.0;
            let recency = RECENCY_DECAY_PER_HOUR.powf(age_hours);
            let mut importance: f64 = 0.4;
            if e.status == STATUS_FAILED {
                importance += 0.3;
            }
            if !e.refs.is_empty() {
                importance += 0.2;
            }
            if !e.keywords.is_empty() {
                importance += 0.1;
            }
            let importance = importance.min(1.0);
            (recency * importance * relevance, e)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut picked: Vec<Conclusion> = Vec::new();
    let mut used = 0usize;
    for (score, e) in scored {
        if picked.len() >= top_n {
            break;
        }
        // 只收「沾边」的（乘积近 0 = 与当前任务毫不相关，注入进去只是噪音）。
        if score < MIN_RETRIEVAL_SCORE {
            continue;
        }
        let cost = render_line(&e).chars().count();
        if used + cost > max_chars && !picked.is_empty() {
            break;
        }
        used += cost;
        picked.push(e);
    }
    log::info(format!(
        "结论索引检索：scope={} 候选 {} 条 → 命中 {} 条（query {} 字）",
        scope.as_str(),
        n as usize,
        picked.len(),
        query.chars().count()
    ));
    picked
}

fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

// ── 注入渲染 ─────────────────────────────────────────────────────

/// 注入块的固定表头（**固定前缀的一部分**，逐字节稳定；正文永远拼在其后，同 `memory_block_text`
/// 的缓存纪律）。它同时是单测 / e2e 的判据锚点。
pub const INJECT_HEADER: &str = "[recoverable conclusions index — restored after context \
    compaction. These were established earlier in this session (or an earlier one); reuse them \
    and do NOT redo work that already succeeded or already failed.]";

/// 单条结论渲染成一行。`failed` 条目**显式标注「曾尝试并失败，勿重复」**（Manus 口径）。
fn render_line(e: &Conclusion) -> String {
    let refs = if e.refs.is_empty() {
        String::new()
    } else {
        format!(
            " [refs: {}]",
            e.refs
                .iter()
                .map(|r| format!("{}={}", r.kind, r.value))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    match e.status.as_str() {
        STATUS_FAILED => format!(
            "- (FAILED earlier — do NOT repeat; try a different approach) {} (reason: {}){}",
            e.conclusion, e.reason, refs
        ),
        STATUS_REJECTED => format!(
            "- (REJECTED — superseded by later evidence; do NOT rely on it) {}{}",
            e.conclusion, refs
        ),
        _ => format!("- (verified) {}{}", e.conclusion, refs),
    }
}

/// 把选中的结论渲染成一段注入文本（含固定表头）。空输入 ⇒ 空串（调用方据此不注入）。
pub fn render_injection(picked: &[Conclusion]) -> String {
    if picked.is_empty() {
        return String::new();
    }
    let mut out = String::from(INJECT_HEADER);
    for e in picked {
        out.push('\n');
        out.push_str(&render_line(e));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta(scope: MemoryScope) -> Meta<'static> {
        Meta { ts: 1_000, scope, turn: 3, session_id: "sess-1" }
    }

    /// 确定性抽取：一段「有用户结论 + 有失败尝试 + 有文件指针」的会话 ⇒ 三类条目都出来，
    /// **失败那条带 `reason`**（Manus：失败是纠错证据，不得删）。
    #[test]
    fn extract_captures_conclusion_failure_and_file_pointer() {
        let dropped = vec![
            json!({"role":"user","content":[{"type":"text","text":"帮我把 src/main.rs 里的 parse_args 改对"}]}),
            json!({"role":"assistant","content":[
                {"type":"tool_use","id":"a1","name":"Edit","input":{"file_path":"src/main.rs","old_string":"x","new_string":"y"}}
            ]}),
            json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"a1","content":"ok"}
            ]}),
            json!({"role":"assistant","content":[
                {"type":"tool_use","id":"b1","name":"Cmd","input":{"command":"cargo test --bins"}}
            ]}),
            json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"b1","is_error":true,"content":"error: test failed: parse_args_panics"}
            ]}),
        ];
        let entries = extract_from_dropped(&dropped, &meta(MemoryScope::Global));

        // 用户诉求（verified）
        let ask = entries.iter().find(|e| e.conclusion.contains("parse_args")).unwrap();
        assert_eq!(ask.status, STATUS_VERIFIED);

        // 失败尝试（failed + reason）
        let failed = entries.iter().find(|e| e.status == STATUS_FAILED).expect("必须有失败条目");
        assert!(failed.conclusion.contains("Cmd"), "失败条目要交代试了什么");
        assert!(failed.reason.contains("parse_args_panics"), "失败条目必须带 reason");

        // 文件指针（Edit 的 file_path）
        let edited = entries
            .iter()
            .find(|e| e.conclusion.contains("Edit") && e.conclusion.contains("src/main.rs"))
            .expect("成功的写类工具要留文件指针");
        assert!(edited.refs.iter().any(|r| r.kind == "file" && r.value == "src/main.rs"));
    }

    /// 追加 + 读取 + 去重：同一批写两次只留一条；按 scope 分家（global 不掺 pet）。
    #[test]
    fn append_is_scoped_deduped_and_reloadable() {
        let dir = std::env::temp_dir().join(format!("lunac-concl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let entries = extract_from_dropped(
            &[json!({"role":"user","content":[{"type":"text","text":"确认 foo.rs 里的实现"}]})],
            &meta(MemoryScope::Global),
        );
        assert!(!entries.is_empty());
        append(&dir, MemoryScope::Global, &entries);
        append(&dir, MemoryScope::Global, &entries); // 重复写入应被跳过
        let g = load(&dir, MemoryScope::Global);
        assert_eq!(g.len(), entries.len(), "重复项必须去重");
        assert!(load(&dir, MemoryScope::Pet).is_empty(), "按 scope 分家：global 不掺 pet");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 检索：压缩后用「任务关键词」查询 ⇒ 命中原结论；`failed` 条目被标为「勿重复」。
    #[test]
    fn retrieve_hits_by_identifier_and_marks_failure_as_dont_repeat() {
        let dir = std::env::temp_dir().join(format!("lunac-concl-r-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dropped = vec![
            json!({"role":"user","content":[{"type":"text","text":"把 core-agent 的 parse_args 修好"}]}),
            json!({"role":"assistant","content":[
                {"type":"tool_use","id":"b1","name":"Cmd","input":{"command":"cargo test --bins"}}
            ]}),
            json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"b1","is_error":true,"content":"error: cannot find macro"}
            ]}),
        ];
        let mut m = meta(MemoryScope::Global);
        m.ts = now_like();
        let entries = extract_from_dropped(&dropped, &m);
        append(&dir, MemoryScope::Global, &entries);

        let picked = retrieve(
            &dir,
            MemoryScope::Global,
            "继续修 parse_args，之前 cargo test 报 cannot find macro",
            m.ts,
            5,
            4_000,
        );
        assert!(!picked.is_empty(), "用任务关键词查询必须命中");
        assert!(picked.iter().any(|e| e.conclusion.contains("parse_args")), "要命中原结论");
        assert!(
            picked.iter().any(|e| e.status == STATUS_FAILED),
            "失败条目与查询沾边时也要被检索到（它正是「勿重复」的证据）"
        );
        let block = render_injection(&picked);
        assert!(block.starts_with(INJECT_HEADER), "注入块表头是固定前缀");
        assert!(block.contains("do NOT repeat"), "failed 条目必须标注「勿重复」");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 模型结构化段解析：切出 JSON 段、剥离出摘要、缺段时返回空结论（调用方退到确定性抽取）。
    #[test]
    fn parse_model_block_splits_and_strips() {
        let m = meta(MemoryScope::Global);
        let text = format!(
            "## What Was Done\n改了 foo.rs\n\n{}\n[{{\"status\":\"failed\",\"conclusion\":\"试了 A 不行\",\"reason\":\"编译错\",\"keywords\":[\"foo.rs\"],\"refs\":[{{\"kind\":\"file\",\"value\":\"foo.rs\"}}]}}]\n{}",
            INDEX_BEGIN, INDEX_END
        );
        let (summary, entries) = parse_model_block(&text, &m);
        assert!(!summary.contains(INDEX_BEGIN) && !summary.contains("试了 A 不行"));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, STATUS_FAILED);
        assert_eq!(entries[0].reason, "编译错");
        assert!(entries[0].refs.iter().any(|r| r.value == "foo.rs"));

        // 没有标记 ⇒ 原样返回、结论为空
        let (s2, e2) = parse_model_block("## What Was Done\n无结构段", &m);
        assert_eq!(s2, "## What Was Done\n无结构段");
        assert!(e2.is_empty());
    }

    /// 落盘前过脱敏：结论 / 指针里的凭据不得进索引文件。
    #[test]
    fn secrets_are_masked_before_persisting() {
        let dropped = vec![json!({"role":"user","content":[
            {"type":"text","text":"用 curl -H 'Authorization: Bearer sk-live-99887766' 试一下"}
        ]})];
        let entries = extract_from_dropped(&dropped, &meta(MemoryScope::Global));
        let blob = serde_json::to_string(&entries).unwrap();
        assert!(!blob.contains("99887766"), "凭据不得落进索引：{blob}");
    }

    /// 标识符抽取：抓标识符、放过普通词。
    #[test]
    fn identifiers_keep_real_ids_and_drop_plain_words() {
        let ids = extract_identifiers("把 src/main.rs 的 parseArgs 和 camelCase 与 foo_bar 修一下 The");
        for want in ["src/main.rs", "foo_bar", "camelcase"] {
            assert!(ids.iter().any(|i| i == want), "应抽到 {want}：{ids:?}");
        }
        assert!(!ids.iter().any(|i| i == "the"), "普通词不该被当标识符");
    }

    fn now_like() -> i64 {
        crate::now_epoch_secs()
    }
}
