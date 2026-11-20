//! 拼音 / 首字母匹配的共享实现（v1 宿主与 v2 Flutter 薄层共用）。
//!
//! 数据来源：**复用 `pinyin` crate**（`indexer/Cargo.toml` 既有依赖 `0.11` + `plain`，
//! v1 的 `app/src-tauri/Cargo.toml` 与 `app/src-tauri/src/pinyin_bridge.rs` 用的是同一份）。
//! 不内置汉字表、不联网、不新增依赖。
//!
//! 口径与 v1 `app_indexer::generate_pinyin_tokens` 完全一致（应用 / 文件搜索的拼音命中
//! 就是靠它），也与 v1 `app/src/plugins/registry.ts` 的 `_pinyinTokens` 同语义
//! （那里用 `pinyin-pro`，都是「全拼 + 首字母」两个 token）：
//!   · 只有含汉字的文本才产生 token；
//!   · token[0] = 全拼，按字连写、无声调（「快捷启动」→ `kuaijieqidong`）；
//!   · token[1] = 首字母，每个汉字音节的第一个字母（「快捷启动」→ `kjqd`）；
//!     与全拼相同时不重复给出（单字等场景）；
//!   · ASCII / 数字 / 空白 / 标点不进入 token —— 中英混排的关键词由调用方对英文部分
//!     做直接匹配、对汉字部分做拼音匹配（v1 也是这两条路并行）。
//!
//! 大小写：token 天然是 ASCII 小写，调用方把查询词 `to_lowercase()` 后比较即可。

use ::pinyin::ToPinyin;

/// 是否含汉字（`\u{4e00}..=\u{9fff}`，与 v1 `app_indexer::has_chinese` 同一判据）。
pub fn has_chinese(s: &str) -> bool {
    s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

/// 生成拼音 token：全拼 + 首字母。无汉字时返回空 vec（纯英文条目零开销）。
pub fn tokens(text: &str) -> Vec<String> {
    if !has_chinese(text) {
        return Vec::new();
    }
    let syllables: Vec<&str> = text.to_pinyin().flatten().map(|p| p.plain()).collect();
    if syllables.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    // Full pinyin: "微信" → "weixin"
    let full: String = syllables.concat();
    // First letters: "微信" → "wx"
    let firsts: String = syllables.iter().filter_map(|s| s.chars().next()).collect();
    out.push(full.clone());
    if firsts != full {
        out.push(firsts);
    }
    out
}

/// 拼音**模糊**匹配（2026-11-13 用户要求「拼音模糊搜索」）。
///
/// 比 `tokens()` 的整串包含更宽：把汉字串按音节切分后，查询词可以
/// **逐音节取前缀、并跳过任意音节**。例：
///   · `kjd`   → 快捷启动（k + j + 跳过 q + d）
///   · `kuaiqd`→ 快捷启动（kuai + j… 的逐音节前缀）
///   · `wxt`   → 微信截图（w + x + 跳过「截」+ t）
/// 已有的「全拼 / 首字母整串包含」仍然命中，所以这是纯扩展，不改变旧结果。
///
/// 约束：
///   · 查询词必须全是 ASCII 字母，且长度 ≥ 2（单字母交给精确 / 前缀匹配，
///     否则几乎每个中文名都会被命中）；
///   · 只对**连续的汉字段**做匹配（英文、数字、标点不参与）；
///   · 每个音节至少吃掉 1 个字符，且必须是该音节的前缀 —— 不做纯字母乱序。
pub fn fuzzy_match(text: &str, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.len() < 2 || !q.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    // 旧的整串口径（全拼 / 首字母包含）先兜一遍，避免行为回退。
    if tokens(text).iter().any(|token| token.contains(&q)) {
        return true;
    }
    for run in chinese_runs(text) {
        let syllables: Vec<String> = run
            .as_str()
            .to_pinyin()
            .flatten()
            .map(|p| p.plain().to_string())
            .collect();
        if syllables.is_empty() {
            continue;
        }
        if syllable_sequence_matches(&syllables, q.as_bytes()) {
            return true;
        }
    }
    false
}

/// 连续汉字段（英文 / 数字 / 标点都是分隔符）。
fn chinese_runs(text: &str) -> Vec<String> {
    let mut runs = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
            current.push(ch);
        } else if !current.is_empty() {
            runs.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

/// 音节序列 + 查询词（小写 ASCII）的匹配：逐音节取前缀、允许跳过音节。
fn syllable_sequence_matches(syllables: &[String], q: &[u8]) -> bool {
    // seen[qi][si]：从「查询第 qi 个字符 + 第 si 个音节」出发是否已判定失败。
    let mut failed = vec![vec![false; syllables.len() + 1]; q.len() + 1];
    fn walk(
        syllables: &[String],
        q: &[u8],
        qi: usize,
        si: usize,
        failed: &mut Vec<Vec<bool>>,
    ) -> bool {
        if qi == q.len() {
            return true;
        }
        if si == syllables.len() {
            return false;
        }
        if failed[qi][si] {
            return false;
        }
        // 1) 跳过这个音节（模糊：允许中间少打几个字）。
        if walk(syllables, q, qi, si + 1, failed) {
            return true;
        }
        // 2) 用这个音节的前缀吃掉查询词的一段。
        let syllable = syllables[si].as_bytes();
        let max = syllable.len().min(q.len() - qi);
        for len in 1..=max {
            if &syllable[..len] != &q[qi..qi + len] {
                break;
            }
            if walk(syllables, q, qi + len, si + 1, failed) {
                return true;
            }
        }
        failed[qi][si] = true;
        false
    }
    walk(syllables, q, 0, 0, &mut failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_pinyin_is_joined_per_character() {
        assert_eq!(tokens("快捷启动")[0], "kuaijieqidong");
        assert_eq!(tokens("设置")[0], "shezhi");
        assert_eq!(tokens("备忘录")[0], "beiwanglu");
    }

    #[test]
    fn initials_are_the_second_token() {
        assert_eq!(tokens("快捷启动")[1], "kjqd");
        assert_eq!(tokens("剪贴板历史")[1], "jtbls");
        assert_eq!(tokens("设置")[1], "sz");
    }

    #[test]
    fn mixed_text_only_encodes_the_chinese_part() {
        // 「Web 搜索」：英文由调用方的直接匹配负责，拼音 token 只来自「搜索」。
        assert_eq!(tokens("Web 搜索"), vec!["sousuo", "ss"]);
        // 中英混排：拼音只取汉字音节，token 全为小写 ASCII。
        assert_eq!(tokens("打开 Downloads 文件夹")[0], "dakaiwenjianjia");
    }

    #[test]
    fn text_without_chinese_has_no_tokens() {
        assert!(tokens("open folder").is_empty());
        assert!(tokens("12345").is_empty());
        assert!(!has_chinese("lunac"));
        assert!(has_chinese("Lunac 设置"));
    }

    #[test]
    fn single_character_does_not_duplicate_the_same_token() {
        assert_eq!(tokens("阿"), vec!["a"]);
    }

    #[test]
    fn fuzzy_accepts_syllable_prefixes_in_order() {
        assert!(fuzzy_match("快捷启动", "kuai"));
        assert!(fuzzy_match("快捷启动", "kuaij"));
        assert!(fuzzy_match("快捷启动", "kuaiqid"));
        assert!(fuzzy_match("备忘录", "beiw"));
    }

    #[test]
    fn fuzzy_accepts_skipped_syllables() {
        // k + j + (跳过 启) + d
        assert!(fuzzy_match("快捷启动", "kjd"));
        // w + x + (跳过 截) + t
        assert!(fuzzy_match("微信截图", "wxt"));
        // 跳过开头：从第二个字开始打也命中
        assert!(fuzzy_match("剪贴板历史", "bls"));
    }

    #[test]
    fn fuzzy_keeps_the_old_exact_tokens() {
        assert!(fuzzy_match("快捷启动", "kjqd"));
        assert!(fuzzy_match("快捷启动", "kuaijieqidong"));
        assert!(fuzzy_match("设置", "sz"));
    }

    #[test]
    fn fuzzy_rejects_noise() {
        // 顺序不对 / 不存在的前缀都不该命中
        assert!(!fuzzy_match("快捷启动", "dk"));
        assert!(!fuzzy_match("音乐", "wx"));
        // 单字母交给精确匹配，模糊不参与
        assert!(!fuzzy_match("快捷启动", "k"));
        // 非字母查询不参与拼音模糊
        assert!(!fuzzy_match("快捷启动", "快j"));
        // 纯英文文本没有拼音
        assert!(!fuzzy_match("open folder", "op"));
    }

    #[test]
    fn fuzzy_only_uses_the_chinese_runs() {
        // 英文部分不参与拼音模糊；汉字部分照常命中。
        assert!(fuzzy_match("Web 搜索", "ss"));
        assert!(fuzzy_match("Web 搜索", "sous"));
    }
}
