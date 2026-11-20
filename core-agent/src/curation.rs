// core-agent/src/curation.rs
// 决策驱动压缩（C 臂 MVP，2026-10-24）—— 让本地小模型对「可瘦身的工具结果」再判一次「能不能丢」。
//
// 定位（`docs/archive/agent-feature-backlog.md` §4.1.6 的 C 臂）：现有压缩是**体积驱动**的
// （水位 + 成本模型），它只问「省下的够不够抵掉废掉的缓存」，**不问这段内容还有没有用**。
// C 臂在**已经决定要瘦身**的那一刻插一道本地判定：模型说「这段还能丢」才丢，说「留着」就
// 从候选里拿掉。⇒ 相对 A 臂**只会更保守**（保留更多），不会多丢。
//
// **边界（硬）**：
//   · 只在 `Compact::Elide`（0.85 水位、成本模型已通过）生效；`ElideCold` / `Drop` / `Force`
//     **完全不走本模块** —— 那三档是「不压就可能撞上限」的安全刚需，绝不能被模型否决。
//   · **fail-open = 照旧**：后端故障 / 超时 / 通道忙 / 解析失败 ⇒ [`Curation::fallback`]，
//     调用方**原样使用**体积驱动的结果（与不接线逐字节一致）。
//     ⚠️ 与其余切片的区别：别的切片 fail-open 是「不做额外动作」，这里是「按现状压」——
//     因为「不压」可能直接撞 400，那才是更危险的一侧。
//   · **模型合法地回空列表**（`drop: []`，即「一条都不该丢」）**不是故障**：这是它的判断，
//     调用方据此把全部候选从瘦身名单里拿掉（C 臂因此更保守，实验里表现为成本上升）。
//   · 只判**前 [`MAX_ITEMS`] 个**候选（按体积排序取最大的那批）；未送判的候选**照旧**按体积
//     驱动处理 —— 不引入「后面的一律保留」这种无法解释的规则。
//
// **开关**：`LUNAC_LOCAL_CURATE`（默认**关**；真值 `1/true/on/yes`，口径同 `LUNAC_DUMP_API`）。
// **端点**：复用 `LUNAC_LOCAL_MODEL_URL`；未配 ⇒ 整层透明关闭。
// ⚠️ 不复用 `LUNAC_LOCAL_DECIDE`：那个开关下全是「只记账 / 不改变行为」的切片，本模块
// **会改变压缩选择**，必须能单独关掉（实验时也要能只开 C 臂）。

use std::time::Duration;

use serde_json::Value;

use crate::decide;
use crate::log;

/// 送判的候选条数上限。**4 条**（2026-10-24 真机调参）：8 条 × 600 字时提示词约 1200 token，
/// CPU 冷 prefill 要 ~11.6s，撞 3s 上限直接 fail-open（真机日志 `决策策展后端故障 … error sending request`）。
/// 4 条 × 400 字 ≈ 400 token，冷 prefill ~4s，落在 [`LOCAL_CURATE_TIMEOUT`] 内。
pub const MAX_ITEMS: usize = 4;

/// 单条候选送判的字符预算：判「这段是编译日志还是关键结论」不需要全文。
///
/// ⚠️ **必须头 / 中 / 尾三段采样**（见 [`sample_item`]）：只取开头会漏掉藏在文件中部的
/// 约束（本仓的决策保真度夹具 T2 就是「规则在 90 行文件的第 45 行」——只截头 400 字时
/// 模型根本看不见它，判成「filler 可丢」，C 臂就白做了）。
pub const ITEM_CHARS: usize = 400;

/// 去掉 `Read` 工具自己加的 `"{行号}\t"` 前缀（**只去这种形态**，其余内容一字不动）。
///
/// ⚠️ 2026-10-24 真机依据：同一段 90 行流水，**带行号**时本地 4B 判 `drop:[1,2,3,4]`
/// （把整块当机器噪声），**去掉行号后同一内容**判 `drop:[]`。行号是本项目展示层的前缀、
/// 不属于文件内容 ⇒ 送判前必须剥掉，否则 C 臂会把「夹着一条关键事实的流水」整条丢掉
/// （T5 0/3 的直接原因）。
pub fn strip_line_numbers(text: &str) -> String {
    text.lines()
        .map(|l| match l.split_once('\t') {
            Some((head, rest)) if !head.is_empty() && head.bytes().all(|b| b.is_ascii_digit()) => rest,
            _ => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 判定「一行是不是具名事实」的标签白名单（小写比较）。
const FACT_LABELS: &[&str] = &[
    "code word", "codeword", "secret", "token", "password", "api key",
    "rule", "constraint", "requirement", "instruction",
];

/// 「全大写 + 冒号」但不属于事实的常见标签（日志级别 / 文档提示词）——
/// 不排掉它们的话，任何带 `NOTE:` / `ERROR:` 的正文都会被整条保护，瘦身直接失效。
const NON_FACT_LABELS: &[&str] = &[
    "INFO", "WARN", "WARNING", "ERROR", "DEBUG", "TRACE", "VERBOSE", "NOTICE", "FATAL",
    "NOTE", "TODO", "FIXME", "HACK", "EXAMPLE", "USAGE", "OUTPUT", "INPUT",
];

/// 叙述型约束（**没有** `标签:` 前缀）的关键词（小写比较，含中文）。
const CONSTRAINT_PHRASES: &[&str] = &[
    " must ", " must be", " must not", " never ", " always ", " do not ", " don't ",
    " required ", " shall ", "禁止", "必须", "不得", "不要",
];

/// 叙述型约束最多认这么多行：超过就当作「代码 / 文档里到处都有 must」，**不保护**
/// （否则随便读一个源码文件就能让所有瘦身失效）。
const MAX_NARRATIVE_LINES: usize = 3;

/// 一行算不算「叙述型约束」。刻意排除注释前缀与代码符号，避免把 `// must be > 0` 这类
/// 源码注释当成规则行（那样的文件会被整条保护）。
fn is_narrative_constraint(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() || t.chars().count() > 200 {
        return false;
    }
    for p in ["//", "/*", "#", "*"] {
        if t.starts_with(p) {
            return false;
        }
    }
    if t.contains('{') || t.contains('}') || t.contains("=>") || t.contains(';') {
        return false;
    }
    let low = t.to_ascii_lowercase();
    CONSTRAINT_PHRASES.iter().any(|p| low.contains(p))
}

/// **确定性护栏**：命中即「这条候选绝不允许被瘦身」，**不经过本地模型**。
///
/// 依据（2026-10-24 实测出的失败形态，C 臂在 T5 上 0/3）：
///   · `PROJECT_RULE_ALPHA: Indentation must be 4 spaces`（规则行藏在 90 行 filler 中间）
///   · `code word: ORBITAL-FERRET-9`（具名事实行藏在 90 行审计流水中间）
///   · 无标签的叙述型约束（如单独一行 `Indentation must be 4 spaces`）—— 但**只认少量**
///     （见 [`MAX_NARRATIVE_LINES`]），否则源码文件会被整条保护、瘦身失效。
/// 共同形态 = **少量决定性行，周围全是同质噪声**。4B 在带行号 / 同类条目较多时会把整块
/// 判成噪声 ⇒ 这一层不再交给模型判，命中即整条保留（宁多花 token，不丢事实）。
pub fn protected_reason(text: &str) -> Option<&'static str> {
    let mut narrative = 0usize;
    for line in strip_line_numbers(text).lines() {
        let low = line.to_ascii_lowercase();
        if low.contains("code word") || low.contains("codeword") {
            return Some("secret-marker");
        }
        if let Some((label, value)) = split_label(line) {
            if !value.trim().is_empty() && is_fact_label(label) {
                return Some("named-fact-line");
            }
        }
        if is_narrative_constraint(line) {
            narrative += 1;
        }
    }
    if (1..=MAX_NARRATIVE_LINES).contains(&narrative) {
        return Some("narrative-constraint");
    }
    None
}

/// 剥掉行首的项目符号 / 空白后按**第一个** `:` 或 `=` 切成 `(标签, 值)`。
fn split_label(line: &str) -> Option<(&str, &str)> {
    let t = line.trim_start_matches(|c: char| {
        c.is_whitespace() || c == '-' || c == '*' || c == '•'
    });
    let (label, value) = t.split_once([':', '='])?;
    let label = label.trim_end();
    if label.is_empty() || label.chars().count() > 32 {
        return None;
    }
    if label.chars().any(|c| c.is_ascii_control()) {
        return None;
    }
    Some((label, value))
}

fn is_fact_label(label: &str) -> bool {
    let low = label.to_ascii_lowercase();
    if FACT_LABELS.contains(&low.as_str()) {
        return true;
    }
    if NON_FACT_LABELS.iter().any(|l| l.eq_ignore_ascii_case(label)) {
        return false;
    }
    // 全大写（无小写字母、至少两个大写字母）的短标签，如 `PROJECT_RULE_ALPHA`
    let upper = label.chars().filter(|c| c.is_ascii_uppercase()).count();
    let lower = label.chars().filter(|c| c.is_ascii_lowercase()).count();
    upper >= 2 && lower == 0
}

/// 把一条候选压成「头 / 中 / 尾」三段摘要，总长不超过 `cap`（另加两处 `[…]` 标记）。
/// 采样前先剥掉行号前缀（见 [`strip_line_numbers`]）。
pub fn sample_item(text: &str, cap: usize) -> String {
    let text = strip_line_numbers(text);
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= cap {
        return text;
    }
    let part = (cap / 3).max(1);
    let mid = chars.len() / 2;
    let mid_start = mid.saturating_sub(part / 2);
    let mut out: String = chars[..part].iter().collect();
    out.push_str("\n[...]\n");
    out.extend(chars[mid_start..(mid_start + part).min(chars.len())].iter());
    out.push_str("\n[...]\n");
    out.extend(chars[(chars.len() - part)..].iter());
    out
}

/// 单次判定的硬上限。**独立于判定 / 改写**（10s），理由是真机实测：
/// 送判提示词 = 4 条 × 400 字 ≈ 400 token，CPU 冷 prefill 约 **4s**（103 tok/s），
/// 再加几条编号的输出；判定（3s）与改写（3s）的输入量级小得多，套用它们会必然超时
/// （2026-10-24 真机：8 条 × 600 字 + 3s ⇒ `error sending request` ⇒ C 臂静默失效）。
/// 超时即 fail-open（照旧按体积压）。热机后同一 system 命中前缀缓存，实际只需 ~200ms。
pub const LOCAL_CURATE_TIMEOUT: Duration = Duration::from_millis(10_000);

/// 判定提示词：只回答「这些工具结果里，哪些**已经不再需要**」。
///
/// ⚠️ 第 3 / 6 句是 2026-10-24 真机负结果补的：t5b 夹具里 90 行同质「审计流水」中间夹一行
/// `code word: …`，且**四条候选全是同质文件正文** ⇒ 旧措辞让 4B 把四条整体判成噪声
/// （`drop:[1,2,3,4]`，C 臂在 T5 上 0/3）。现措辞要求「逐条独立判」+「出现单行命名实体 /
/// 口令 / 编号也必须整条保留」；两条都要，只加后者时本地探针仍会全丢。
const CURATE_SYSTEM: &str = "You decide which old tool outputs are safe to drop from an agent's context to save space.\n\
Output STRICT JSON only: {\"drop\":[<index>,...]} where indexes are 1-based numbers from the list.\n\
Judge every item on its own text; how many other items look similar must not change the verdict for any one of them.\n\
Drop an item only if nothing in it is still needed (build logs, directory listings, repeated file dumps, progress noise).\n\
Keep anything that states a decision, a constraint, a requirement, an unresolved question, or a fact the final answer may need to quote.\n\
Also keep an item if a single line in it names something specific (a code word, token, secret, ID, number, path, threshold or rule), even when every other line is repetitive.\n\
An item whose lines all repeat one sentence pattern with only counters changing is filler: drop it (e.g. 90 lines of `- note 3 item 41 : review comments about X`).\n\
When unsure, keep it. If nothing is safe to drop, output {\"drop\":[]}.";

/// 输出约束（GBNF）：`{"drop":[1..8,...]}`，编号只允许 1–8。
const DROP_GRAMMAR: &str = r#"root ::= "{" ws "\"drop\"" ws ":" ws arr ws "}"
arr ::= "[" ws ( num ( ws "," ws num )* )? ws "]"
num ::= "1" | "2" | "3" | "4"
ws ::= [ \t\r\n]*"#;

/// 一次策展的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Curation {
    /// 模型明确说「可以丢」的候选编号（**1 基**，已去重、已排序）。
    pub drop: Vec<usize>,
    /// 是否走了后备路径（后端故障 / 超时 / 通道忙 / 解析失败）⇒ 调用方**原样**用体积驱动的结果。
    pub fallback: bool,
    /// 机器可读的原因码。
    pub reason: &'static str,
}

impl Curation {
    fn judged(drop: Vec<usize>) -> Self {
        let reason = if drop.is_empty() { "keep-all" } else { "ok" };
        Curation { drop, fallback: false, reason }
    }
    /// **fail-open 的唯一出口**：调用方据此**不改变**体积驱动的选择。
    fn fail_open(reason: &'static str) -> Self {
        Curation { drop: Vec::new(), fallback: true, reason }
    }
    /// **busy-skip 的唯一出口**：本地通道忙 ⇒ 立即跳过（不排队）⇒ 照旧。
    pub fn busy_skip() -> Self {
        Curation { drop: Vec::new(), fallback: true, reason: "busy-skip" }
    }
}

/// 判定后端（实现者只回 `Result`）：`Ok` = 可丢的 1 基编号（可为空）；`Err` = 我这层坏了。
pub trait Curator {
    fn judge(&self, items: &[String]) -> Result<Vec<usize>, String>;
}

/// 本地模型后端（形态同 `decide::LocalHttp`：OpenAI 兼容端点、回环、显式禁代理）。
pub struct LocalCurator {
    cfg: decide::LocalHttpConfig,
}

impl LocalCurator {
    pub fn new(cfg: decide::LocalHttpConfig) -> Self {
        LocalCurator {
            cfg: decide::LocalHttpConfig { timeout: LOCAL_CURATE_TIMEOUT, ..cfg },
        }
    }
}

impl Curator for LocalCurator {
    fn judge(&self, items: &[String]) -> Result<Vec<usize>, String> {
        // 与其余本地调用共用通道锁（`llama-server` 是 -np 1）；忙 ⇒ 立即 Err（⇒ 照旧）。
        let _guard = decide::try_acquire_local_channel()
            .ok_or_else(|| decide::BUSY_SKIP.to_string())?;
        let client = reqwest::blocking::Client::builder()
            .timeout(self.cfg.timeout)
            .no_proxy()
            .build()
            .map_err(|e| format!("建 HTTP client 失败：{e}"))?;
        let url = format!("{}/v1/chat/completions", self.cfg.base_url.trim_end_matches('/'));
        let mut listing = String::new();
        for (i, item) in items.iter().enumerate() {
            listing.push_str(&format!("\n[{}] {}\n", i + 1, item));
        }
        let body = serde_json::json!({
            "model": self.cfg.model,
            "messages": [
                { "role": "system", "content": CURATE_SYSTEM },
                { "role": "user", "content": listing },
            ],
            // 最多 8 个编号 ⇒ 48 token 足够。
            "max_tokens": 48,
            "temperature": 0.0,
            "grammar": DROP_GRAMMAR,
        });
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| format!("连不上本地模型（{url}）：{e}"))?;
        if !resp.status().is_success() {
            return Err(format!("本地模型 HTTP {}", resp.status()));
        }
        let v: Value = resp.json().map_err(|e| format!("本地模型响应不是 JSON：{e}"))?;
        let content = v
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .ok_or_else(|| "响应里没有 choices[0].message.content".to_string())?;
        parse_drop(content, items.len())
    }
}

/// 统一入口：**fail-open 的落点**，也是唯一会写日志的地方。
pub fn curate(c: &dyn Curator, items: &[String]) -> Curation {
    if items.is_empty() {
        return Curation::judged(Vec::new());
    }
    match decide::within_deadline(LOCAL_CURATE_TIMEOUT, || c.judge(items)) {
        Ok(drop) => {
            log::info(format!(
                "决策策展：送判 {} 条，模型认为可丢 {} 条（其余保留）",
                items.len(),
                drop.len()
            ));
            Curation::judged(drop)
        }
        Err(err) if err == decide::BUSY_SKIP => {
            log::warn("决策策展：本地通道忙，立即跳过本次（按体积驱动照旧）");
            Curation::busy_skip()
        }
        Err(err) => {
            log::warn(format!("决策策展后端故障，按 fail-open 照旧瘦身（{err}）"));
            Curation::fail_open("fail-open-backend-error")
        }
    }
}

/// 解析 `{"drop":[1,3]}`。**越界 / 非整数 / 非 JSON 一律 Err**（⇒ 上层 fail-open，绝不猜）。
fn parse_drop(content: &str, item_count: usize) -> Result<Vec<usize>, String> {
    let v: Value =
        serde_json::from_str(content.trim()).map_err(|e| format!("策展输出不是合法 JSON：{e}"))?;
    let arr = v
        .get("drop")
        .and_then(Value::as_array)
        .ok_or_else(|| "策展输出不合 schema：缺 drop 数组".to_string())?;
    let mut out: Vec<usize> = Vec::new();
    for item in arr {
        let n = item
            .as_u64()
            .ok_or_else(|| "策展输出不合 schema：drop 元素不是整数".to_string())? as usize;
        if n == 0 || n > item_count {
            return Err(format!("策展输出越界：{n}（候选 {item_count} 条）"));
        }
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out.sort_unstable();
    Ok(out)
}

/// **纯函数**：从「候选」里挑出**继续走瘦身**的那些。
///
/// `drop` 里的编号（1 基）= 模型说可以丢 ⇒ 继续瘦；其余（含未送判的尾部候选）⇒ **保留**。
/// `fallback = true` 时**不改变任何东西**（返回原列表 = 与不接线逐字节一致）。
pub fn elision_candidates_after_curation<T: Clone>(candidates: &[T], curation: &Curation) -> Vec<T> {
    if curation.fallback {
        return candidates.to_vec();
    }
    candidates
        .iter()
        .enumerate()
        .filter(|(i, _)| curation.drop.contains(&(i + 1)))
        .map(|(_, c)| c.clone())
        .collect()
}

/// 生产开关：`LUNAC_LOCAL_CURATE`（默认关）。进程内读一次。
pub fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        log::is_truthy(&std::env::var("LUNAC_LOCAL_CURATE").unwrap_or_default())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 复刻 `Read` 工具的返回格式（`"{行号}\t{行}\n"`）——真机夹具就是这个形态。
    fn as_read_output(text: &str) -> String {
        let mut out = String::new();
        for (i, line) in text.lines().enumerate() {
            out.push_str(&format!("{}\t{}\n", i + 1, line));
        }
        out
    }

    /// `Read` 的行号前缀必须在送判前剥掉（否则本地 4B 把正文判成机器噪声）。
    #[test]
    fn line_numbers_are_stripped_before_judging() {
        assert_eq!(sample_item("1\thello\n22\tworld\n", 400), "hello\nworld");
        // 不是「纯数字 + Tab」的行原样保留
        assert_eq!(strip_line_numbers("not_a_prefix\tx"), "not_a_prefix\tx");
        assert_eq!(strip_line_numbers("2026-10-24\tlog line"), "2026-10-24\tlog line");
    }

    /// 行号前缀 + 90 行流水：中段采样仍必须含中间那行关键事实，且不带行号。
    #[test]
    fn sampling_reaches_the_code_word_through_read_line_numbers() {
        let mut raw = String::new();
        for i in 0..90 {
            if i == 44 {
                raw.push_str("code word: ORBITAL-FERRET-9\n");
            } else {
                raw.push_str(&format!(
                    "- ledger entry {i} : transaction reference and audit trail line for reconciliation work.\n"
                ));
            }
        }
        let sampled = sample_item(&as_read_output(&raw), ITEM_CHARS);
        assert!(sampled.contains("ORBITAL-FERRET-9"), "中段采样必须覆盖第 47 行");
        assert!(!sampled.contains("47\t"), "行号前缀不该出现在送判文本里");
    }

    /// 确定性护栏：两种真机失败形态必须命中（且不受 `Read` 行号前缀影响）。
    #[test]
    fn guard_protects_the_two_measured_failure_shapes() {
        let rule = as_read_output("filler\nPROJECT_RULE_ALPHA: Indentation must be 4 spaces\nfiller");
        assert_eq!(protected_reason(&rule), Some("named-fact-line"));
        let code = as_read_output("filler\ncode word: ORBITAL-FERRET-9\nfiller");
        assert_eq!(protected_reason(&code), Some("secret-marker"));
    }

    /// 日志级别 / 普通散文 / 小写普通标签不触发保护（否则瘦身会整体失效）。
    #[test]
    fn guard_does_not_fire_on_ordinary_text() {
        assert_eq!(protected_reason("INFO: server started\nERROR: boom"), None);
        assert_eq!(
            protected_reason("- note 0 item 1 : architecture discussion"),
            None
        );
        assert_eq!(protected_reason("time: 12:00\nlevel: info"), None);
        assert_eq!(protected_reason("https://example.com/a:b"), None);
    }

    /// 小写关键标签（自然语言形态）也要保护。
    #[test]
    fn guard_protects_lowercase_keyword_labels() {
        assert_eq!(protected_reason("secret: hunter2"), Some("named-fact-line"));
        assert_eq!(
            protected_reason("requirement: must stay offline"),
            Some("named-fact-line")
        );
    }

    /// 无标签的叙述型约束也要保护（真机盲区 A）。
    #[test]
    fn guard_protects_a_bare_narrative_constraint() {
        let t = as_read_output("filler\nIndentation must be 4 spaces, never tabs.\nfiller");
        assert_eq!(protected_reason(&t), Some("narrative-constraint"));
        assert_eq!(
            protected_reason("缩进必须为 4 个空格"),
            Some("narrative-constraint")
        );
    }

    /// 「到处都有 must」的文件不保护（按行数上限），注释与代码行也不算 —— 否则瘦身失效。
    #[test]
    fn guard_ignores_files_with_many_must_lines() {
        let mut s = String::new();
        for i in 0..5 {
            s.push_str(&format!("value {i} must be positive\n"));
        }
        assert_eq!(protected_reason(&s), None);
        assert_eq!(protected_reason("// must be > 0"), None);
        assert_eq!(protected_reason("let x = 1; // must not be zero"), None);
    }

    /// C 臂必须**还敢丢纯噪声**：4 条全是重复 filler ⇒ 至少丢 1 条。
    ///
    /// 2026-10-24 修正批的副作用：加强提示词后 4B 在 t2b/t5b 上回 `drop:[]` ⇒ C 臂退化成
    /// 「不压缩」（等价 B 臂、压缩收益为零）。本探针是那条「过度保守」的钉住。
    #[test]
    #[ignore = "需要本地 llama-server：设 LUNAC_LOCAL_MODEL_URL 后跑 cargo test -- --ignored"]
    fn local_curator_still_drops_pure_filler() {
        let Some(cfg) = decide::LocalHttpConfig::from_env() else {
            eprintln!("跳过：未设 LUNAC_LOCAL_MODEL_URL");
            return;
        };
        let notes = |k: usize| {
            let mut s = String::new();
            for i in 0..90 {
                s.push_str(&format!(
                    "- note {k} item {i} : review comments about interface contracts, retry policy, and logging levels.\n"
                ));
            }
            sample_item(&as_read_output(&s), ITEM_CHARS)
        };
        let items = vec![notes(1), notes(2), notes(3), notes(4)];
        let out = curate(&LocalCurator::new(cfg), &items);
        eprintln!("纯噪声探针 ⇒ {out:?}");
        assert!(!out.fallback, "不该走 fail-open：{out:?}");
        assert!(
            !out.drop.is_empty(),
            "纯 filler 一条都不丢 ⇒ C 臂等于没做（退化成 B）：{out:?}"
        );
    }

    struct Boom;
    impl Curator for Boom {
        fn judge(&self, _i: &[String]) -> Result<Vec<usize>, String> {
            Err("endpoint down".into())
        }
    }
    struct Fixed(Vec<usize>);
    impl Curator for Fixed {
        fn judge(&self, _i: &[String]) -> Result<Vec<usize>, String> {
            Ok(self.0.clone())
        }
    }

    /// 后端故障 ⇒ fallback（调用方据此**不改变**体积驱动的结果）。
    #[test]
    fn backend_failure_falls_back_to_status_quo() {
        let c = curate(&Boom, &["a".to_string()]);
        assert!(c.fallback);
        assert!(c.drop.is_empty());
        assert_eq!(c.reason, "fail-open-backend-error");
    }

    /// 空输入 ⇒ 空判定、不带 fallback（没有可判的内容不算故障）。
    #[test]
    fn empty_items_is_not_a_failure() {
        let c = curate(&Boom, &[]);
        assert!(!c.fallback);
        assert_eq!(c.reason, "keep-all");
    }

    /// 模型回空列表 = 「一条都不该丢」：不是 fallback，调用方要据此全部保留。
    #[test]
    fn empty_drop_list_means_keep_all() {
        let c = curate(&Fixed(Vec::new()), &["a".to_string()]);
        assert!(!c.fallback);
        assert!(c.drop.is_empty());
        assert_eq!(c.reason, "keep-all");
    }

    /// 解析：合法 JSON（含换行）可解析；越界 / 非整数 / 缺键 / 非 JSON 全 Err。
    #[test]
    fn parse_drop_enforces_schema_and_range() {
        assert_eq!(parse_drop(r#"{"drop":[1,3]}"#, 4).unwrap(), vec![1, 3]);
        assert_eq!(parse_drop("{\n \"drop\": [2]\n}", 4).unwrap(), vec![2]);
        assert!(parse_drop(r#"{"drop":[]}"#, 4).unwrap().is_empty());
        assert_eq!(parse_drop(r#"{"drop":[3,1,3]}"#, 4).unwrap(), vec![1, 3], "去重 + 排序");
        assert!(parse_drop(r#"{"drop":[5]}"#, 4).is_err(), "越界必须 Err");
        assert!(parse_drop(r#"{"drop":[0]}"#, 4).is_err(), "0 不是合法编号");
        assert!(parse_drop(r#"{"drop":["1"]}"#, 4).is_err(), "字符串不算整数");
        assert!(parse_drop(r#"{"nope":[]}"#, 4).is_err());
        assert!(parse_drop("", 4).is_err());
    }

    /// 纯函数：fallback ⇒ 原样；否则**只保留模型没说可丢的**（未送判的尾部也保留）。
    #[test]
    fn retain_keeps_everything_except_the_approved_drops() {
        let cands = vec!["a", "b", "c", "d"];
        // 模型说 1、3 可丢 ⇒ 只有 1、3 继续走瘦身。
        let c = Curation { drop: vec![1, 3], fallback: false, reason: "ok" };
        assert_eq!(elision_candidates_after_curation(&cands, &c), vec!["a", "c"]);

        // fail-open ⇒ 一条都不改（与不接线逐字节一致）。
        let f = Curation::fail_open("x");
        assert_eq!(elision_candidates_after_curation(&cands, &f), cands);

        // 模型说全不能丢 ⇒ 瘦身名单清空（C 臂更保守）。
        let k = Curation { drop: vec![], fallback: false, reason: "keep-all" };
        assert!(elision_candidates_after_curation(&cands, &k).is_empty());
    }

    /// grammar 必须把编号锁在 1–8（与 `MAX_ITEMS` 一致）。
    #[test]
    fn drop_grammar_pins_the_item_range() {
        for n in ["1", "2", "4"] {
            assert!(DROP_GRAMMAR.contains(&format!("\"{n}\"")), "grammar 必须放行 {n}");
        }
        assert!(!DROP_GRAMMAR.contains("\"5\""), "5 超出候选上限");
    }

    /// 送判文本必须**头 / 中 / 尾**都采到 —— 只截头部会漏掉文件中部的约束（T2 的教训）。
    #[test]
    fn sample_item_keeps_short_text_and_covers_head_mid_tail() {
        assert_eq!(sample_item("short", 600), "short");
        let text = format!("{}{}{}", "H".repeat(400), "MIDDLE", "T".repeat(400));
        let s = sample_item(&text, 600);
        assert!(s.starts_with('H'), "必须保留头部");
        assert!(s.ends_with('T'), "必须保留尾部");
        assert!(s.contains("MIDDLE"), "必须采到中部：{s}");
        assert!(s.chars().count() <= 600 + 16, "总长受控：{}", s.chars().count());
    }

    /// 策展上限**必须宽于**判定上限（送判提示词大一个量级，见常量注释）。
    #[test]
    fn curate_timeout_is_wider_than_the_decide_timeout() {
        assert!(
            LOCAL_CURATE_TIMEOUT > decide::LOCAL_DECIDE_TIMEOUT,
            "送判体积大一个量级，不能套用判定的上限"
        );
    }

    /// 通道忙 ⇒ 立即 busy-skip（fallback ⇒ 照旧）；释放后恢复。
    #[test]
    fn local_curator_busy_skips_without_waiting() {
        use std::time::Instant;
        let _serial = decide::TEST_CHANNEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = decide::LocalHttpConfig {
            base_url: "http://127.0.0.1:1".into(),
            model: "local".into(),
            timeout: decide::LOCAL_DECIDE_TIMEOUT,
        };
        let guard = decide::try_acquire_local_channel().expect("首个请求应拿到通道");
        let t0 = Instant::now();
        let c = curate(&LocalCurator::new(cfg), &["x".to_string()]);
        let used = t0.elapsed();
        assert!(used < Duration::from_secs(1), "持锁时必须立即返回：{used:?}");
        assert!(c.fallback);
        assert_eq!(c.reason, "busy-skip");
        drop(guard);
    }

    /// 端点连不上 ⇒ fail-open（照旧），不 panic。
    #[test]
    fn local_curator_unreachable_falls_back() {
        let _serial = decide::TEST_CHANNEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let c = LocalCurator::new(decide::LocalHttpConfig {
            base_url: "http://127.0.0.1:1".into(),
            model: "local".into(),
            timeout: Duration::from_millis(500),
        });
        let out = curate(&c, &["x".to_string()]);
        assert!(out.fallback);
    }

    /// **端到端**（默认 `#[ignore]`）：设 `LUNAC_LOCAL_MODEL_URL` 后手动跑。
    #[test]
    #[ignore = "需要本地 llama-server：设 LUNAC_LOCAL_MODEL_URL 后跑 cargo test -- --ignored"]
    fn local_model_end_to_end_when_configured() {
        let Some(cfg) = decide::LocalHttpConfig::from_env() else {
            eprintln!("跳过：未设 LUNAC_LOCAL_MODEL_URL");
            return;
        };
        let items = vec![
            "cargo build finished with 0 warnings; target directory cleaned".to_string(),
            "PROJECT_RULE_ALPHA: indentation must be 4 spaces, never tabs".to_string(),
        ];
        let out = curate(&LocalCurator::new(cfg), &items);
        assert!(!out.fallback, "不该走 fail-open：{out:?}");
        eprintln!("样例 ⇒ {out:?}");
    }

    /// **端到端回归（默认 `#[ignore]`）**：t5b 形态 —— 90 行同质「审计流水」，中间一行 `code word:`。
    /// 送判文本走真实的 `sample_item`（中段采样），要求**这条不被判成可丢**。
    ///
    /// 2026-10-24 真机负结果：旧 system 把整块判成噪声 ⇒ C 臂在 T5 上 0/3；本测试是该负结果的钉住。
    #[test]
    #[ignore = "需要本地 llama-server：设 LUNAC_LOCAL_MODEL_URL 后跑 cargo test -- --ignored"]
    fn local_curator_keeps_a_single_code_word_inside_homogeneous_log() {
        let Some(cfg) = decide::LocalHttpConfig::from_env() else {
            eprintln!("跳过：未设 LUNAC_LOCAL_MODEL_URL");
            return;
        };
        let mut ledger = String::new();
        for i in 0..90 {
            if i == 44 {
                ledger.push_str("code word: ORBITAL-FERRET-9\n");
            } else {
                ledger.push_str(&format!(
                    "- ledger entry {i} : transaction reference and audit trail line for reconciliation work.\n"
                ));
            }
        }
        let sampled = sample_item(&as_read_output(&ledger), ITEM_CHARS);
        assert!(
            sampled.contains("ORBITAL-FERRET-9"),
            "探针前提：码必须落在三段采样里"
        );
        let items = vec![
            sampled,
            "cargo build finished with 0 warnings; target directory cleaned".to_string(),
            "directory listing: src/, tests/, docs/, target/, scripts/".to_string(),
            "progress noise: 42 files scanned, nothing to report".to_string(),
        ];
        let out = curate(&LocalCurator::new(cfg), &items);
        eprintln!("单条流水探针 ⇒ {out:?}");
        assert!(!out.fallback, "不该走 fail-open：{out:?}");
        assert!(
            !out.drop.contains(&1),
            "含 code word 的流水块被误判成可丢：{out:?}"
        );
    }

    /// **端到端回归（默认 `#[ignore]`）**：t5b 的真实形态 —— **4 条候选全是同质文件正文**，
    /// 第 1 条（流水）里夹一行 `code word:`。比上一条更接近真机：模型会看到「四条都像噪声」。
    ///
    /// 2026-10-24 真机：旧 system ⇒ `drop: [1,2,3,4]`（C 臂在 T5 上 0/3）。本测试钉住这个形态。
    #[test]
    #[ignore = "需要本地 llama-server：设 LUNAC_LOCAL_MODEL_URL 后跑 cargo test -- --ignored"]
    fn local_curator_keeps_a_code_word_when_every_item_looks_like_a_dump() {
        let Some(cfg) = decide::LocalHttpConfig::from_env() else {
            eprintln!("跳过：未设 LUNAC_LOCAL_MODEL_URL");
            return;
        };
        let ledger = || {
            let mut s = String::new();
            for i in 0..90 {
                if i == 44 {
                    s.push_str("code word: ORBITAL-FERRET-9\n");
                } else {
                    s.push_str(&format!(
                        "- ledger entry {i} : transaction reference and audit trail line for reconciliation work.\n"
                    ));
                }
            }
            sample_item(&as_read_output(&s), ITEM_CHARS)
        };
        let notes = |k: usize| {
            let mut s = String::new();
            for i in 0..90 {
                s.push_str(&format!(
                    "- note {k} item {i} : architecture discussion about module boundaries, build pipeline details, and deployment notes.\n"
                ));
            }
            sample_item(&as_read_output(&s), ITEM_CHARS)
        };
        let items = vec![ledger(), notes(1), notes(2), notes(3)];
        assert!(
            items[0].contains("ORBITAL-FERRET-9"),
            "探针前提：码必须在送判文本里"
        );
        let out = curate(&LocalCurator::new(cfg), &items);
        eprintln!("四条同质探针 ⇒ {out:?}");
        assert!(!out.fallback, "不该走 fail-open：{out:?}");
        assert!(
            !out.drop.contains(&1),
            "四条都像噪声时，含 code word 的第 1 条仍被丢掉：{out:?}"
        );
    }
}
