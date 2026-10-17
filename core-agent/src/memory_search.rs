// core-agent/src/memory_search.rs
// 长期记忆的只读检索内核（§待办 3-9 B 项，2026-10-17）—— **纯函数，不碰桥 / 不落盘 / 不注入**。
//
// 定位：只读工具 `MemorySearch` 的检索实现。输入是 `MEMORY.md` 切出来的 bullet 列表，
// 输出按「命中词数 → 出现次数 → 条目更短」排序的命中列表。**不新造读取路径**：正文由
// main.rs 的 `fetch_memory`（既有 `lunac/memory_read` 桥）读入后交给这里；本模块不读文件、
// 不写文件、不调 `Remember`、不产生任何自动调用点（只能由模型主动调用工具触发）。
//
// 检索口径（刻意最小实现，不引向量库 / 分词器）：
//   · 查询按空白切成多个词；**任一**命中即候选（查询无空白时 = 整句子串匹配）。
//   · 两边都 `to_lowercase()` 后再比 ⇒ 英文 / 数字大小写不敏感；中文经它不改字，
//     仍是**子串**匹配（含整句）。
//   · 排序键固定三级：命中词数降序 → 命中总次数降序 → 条目字符数升序；
//     `sort_by` 是稳定排序，完全并列时保持原文顺序。
//   · 不做同义词 / 拼音 / 模糊匹配 —— 那是另一组待调问题，先保持可解释、可单测。

use std::cmp::Ordering;

/// 一条命中。字段就是排序键与回给模型的「分数」，不含任何写入能力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// 命中的 bullet 原文（已去掉 `- ` 前缀；续行以换行保留）。
    pub text: String,
    /// 查询词里命中了几个。
    pub matched_terms: usize,
    /// 查询一共切出几个词（用于给模型解释「命中 2/3 个词」）。
    pub query_terms: usize,
    /// 所有查询词在该条目里出现的总次数（不重叠计数）。
    pub occurrences: usize,
}

/// 排序判据（**唯一实现**）：命中词数降序 → 出现次数降序 → 条目字符数升序。
///
/// 抽成公开函数是为了让 main.rs 合并多个 scope 的结果时复用同一把尺子，
/// 避免各写一份排序键导致「单 scope 与跨 scope 顺序不一致」。
pub fn rank_cmp(a: &Hit, b: &Hit) -> Ordering {
    b.matched_terms
        .cmp(&a.matched_terms)
        .then_with(|| b.occurrences.cmp(&a.occurrences))
        .then_with(|| a.text.chars().count().cmp(&b.text.chars().count()))
}

/// 把记忆原文切成 bullet 列表。
///
/// 形态见 `MEMORY.md`：`- ` 起头是新条目、缩进续行并入上一条；其余行（标题 / 空行 /
/// 引导语）一律忽略。只做展示层面的归一（去前缀、续行 trim），不改条目文字内容。
pub fn split_bullets(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("- ") {
            out.push(rest.trim().to_string());
            continue;
        }
        // 续行：缩进行且已有条目 ⇒ 并入上一条（保留换行，回显时仍是完整条目）。
        if (line.starts_with(' ') || line.starts_with('\t')) && !line.trim().is_empty() {
            if let Some(last) = out.last_mut() {
                last.push('\n');
                last.push_str(line.trim());
            }
        }
    }
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

/// 检索并排序。`limit` 是硬上限（0 = 直接空结果）。
pub fn search(bullets: &[String], query: &str, limit: usize) -> Vec<Hit> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut hits: Vec<Hit> = Vec::new();
    for text in bullets {
        let lower = text.to_lowercase();
        let matched_terms = terms.iter().filter(|t| lower.contains(t.as_str())).count();
        if matched_terms == 0 {
            continue;
        }
        let occurrences: usize = terms
            .iter()
            .map(|t| lower.matches(t.as_str()).count())
            .sum();
        hits.push(Hit {
            text: text.clone(),
            matched_terms,
            query_terms: terms.len(),
            occurrences,
        });
    }
    hits.sort_by(rank_cmp);
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 切分：`- ` 起头算条目、缩进续行并入上一条、标题 / 空行忽略。
    #[test]
    fn split_bullets_joins_continuation_lines() {
        let raw = "# Long-term memory\n- prefers tabs\n  and four-space indents\n\n- uses Windows\n";
        assert_eq!(
            split_bullets(raw),
            vec!["prefers tabs\nand four-space indents", "uses Windows"]
        );
    }

    /// 中文：查询串按**子串**匹配（含整句）。
    #[test]
    fn chinese_substring_matches() {
        let bullets = vec![
            "用户偏好四空格缩进".to_string(),
            "部署环境是 Windows".to_string(),
        ];
        let hits = search(&bullets, "四空格缩进", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "用户偏好四空格缩进");
        assert_eq!(hits[0].matched_terms, 1);
    }

    /// 英文 / 数字：大小写不敏感。
    #[test]
    fn english_matching_is_case_insensitive() {
        let bullets = vec![
            "prefers Tabs".to_string(),
            "Prefers Spaces".to_string(),
        ];
        assert_eq!(search(&bullets, "tabs", 10).len(), 1);
        assert_eq!(search(&bullets, "PREFERS", 10).len(), 2);
    }

    /// 多关键词：任一命中即候选；命中词多的排前面。
    #[test]
    fn multi_keyword_any_hit_ranks_by_matched_terms() {
        let bullets = vec![
            "uses rust".to_string(),
            "writes python".to_string(),
            "uses rust and python".to_string(),
        ];
        let hits = search(&bullets, "rust python", 10);
        assert_eq!(hits.len(), 3, "任一命中即候选");
        assert_eq!(hits[0].text, "uses rust and python", "命中 2 个词的排最前");
        assert_eq!(hits[0].matched_terms, 2);
        assert_eq!(hits[0].query_terms, 2);
    }

    /// 排序第二级：命中词数相同 ⇒ 出现次数多的优先（即使条目更长）。
    #[test]
    fn more_occurrences_wins_when_term_count_ties() {
        let bullets = vec![
            "alpha one".to_string(),
            "alpha alpha beta gamma delta".to_string(),
        ];
        let hits = search(&bullets, "alpha", 10);
        assert_eq!(hits[0].text, "alpha alpha beta gamma delta");
        assert_eq!(hits[0].occurrences, 2);
        assert_eq!(hits[1].occurrences, 1);
    }

    /// 排序第三级：前两级都相同 ⇒ 条目更短的优先。
    #[test]
    fn shorter_entry_wins_when_scores_tie() {
        let bullets = vec![
            "alpha beta gamma delta".to_string(),
            "alpha beta".to_string(),
        ];
        let hits = search(&bullets, "alpha", 10);
        assert_eq!(hits[0].text, "alpha beta");
    }

    /// `limit` 是硬上限（含 0）。
    #[test]
    fn limit_is_respected() {
        let bullets = vec!["alpha".to_string(), "alpha two".to_string()];
        assert_eq!(search(&bullets, "alpha", 1).len(), 1);
        assert!(search(&bullets, "alpha", 0).is_empty());
    }

    /// 无匹配 / 空查询 ⇒ 空结果（不 panic）。
    #[test]
    fn empty_result_cases() {
        let bullets = vec!["alpha".to_string()];
        assert!(search(&bullets, "zeta", 10).is_empty());
        assert!(search(&bullets, "   ", 10).is_empty());
        assert!(search(&[], "alpha", 10).is_empty());
    }

    /// 验收口径 ①：「记忆超过 6000 字符后仍能取回旧条」。
    ///
    /// 构造一段 > 6000 字符的记忆，旧条目在**快照截断点之后**（即 `cap_inject` 注入时
    /// 已经看不到它）；检索直接读原文并切 bullet，因此仍能把它取回。
    #[test]
    fn entry_beyond_the_6000_char_snapshot_is_still_retrievable() {
        let filler = format!("- filler {}\n", "x".repeat(300)).repeat(21);
        let old = "- legacy rule: prefer 4 spaces for indentation";
        let raw = format!("{filler}{old}\n");
        assert!(raw.chars().count() > 6_000);

        let snapshot: String = raw.chars().take(6_000).collect();
        assert!(
            !snapshot.contains("legacy rule"),
            "夹具必须让旧条目落在快照之外，否则测不到验收口径"
        );

        let hits = search(&split_bullets(&raw), "legacy rule", 10);
        assert_eq!(hits.len(), 1, "快照外的旧条目必须仍能被检索返回");
        assert_eq!(hits[0].text, "legacy rule: prefer 4 spaces for indentation");
    }
}
