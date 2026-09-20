// core-agent/src/content_safety.rs
// Lunac 自研 agent 后端 —— 写入内容的静态安全分析（2026-09-20，原 backlog A6）
//
// 与 `bash_safety.rs` **对称但方向不同**：那个扫「命令」、在**执行前**判定；这个扫
// 「文件内容」、在**写入前**判定。别把两者的规则集混起来。
//
// **只做凭据 / 密钥泄漏这一类**（2026-09-20 与用户定稿）。刻意**不做**「代码注入 /
// XSS / 反序列化」那类：正则做不到语义级判定，在正常代码里必然满屏误报，最后的结果
// 是用户学会无视它 —— 那比没有更糟。
//
// 产出走**既有**的审批卡通道：`main.rs` 的 `open_approval()` 把它塞进
// `can_use_tool` 的 `analysis.secrets`，前端 `classifyRequest()` 按「必须人看、
// 不给始终允许、自动档也拦一次」处理（与危险命令同一档展示位，文案不同）。
//
// 判据（宁少勿滥）：一条规则要进来，必须满足下面之一 ——
//   ① **形状本身有信息量**：固定前缀 + 固定长度 / 固定结构（私钥块、各家 API key、
//      JWT、`Bearer <长串>`、带口令的连接串）。这些误报≈0。
//   ② **形状自由**（`api_key = "…"`）：只在**同时**满足「键名命中白名单 + 值够长 +
//      值不是占位符」三道闸时才报（见 `looks_like_placeholder`）。

use std::sync::OnceLock;

use regex::{Regex, RegexSet};
use serde_json::{json, Value};

/// 一次分析最多报几条（按行号取前 N）。命中是**罕见**事件，报太多只会把卡片刷满。
const MAX_HITS: usize = 5;
/// **采集**上限（每条规则）：只防病态输入（一个文件里几万条命中），正常远小于它。
/// 它与 `MAX_HITS` 是两回事 —— 这里限制成本，那里限制展示。
const MAX_SCAN_HITS: usize = 200;
/// 扫描上限：超长内容只扫前这么多字节（切在字符边界上）。
/// 正常源文件 / 配置文件远小于它；上限存在的意义是别让一次 Write 卡住工具循环。
const MAX_SCAN_BYTES: usize = 512 * 1024;

/// 命中项。`rule` 直接进审批卡，所以是**人能看懂的英文短名**（不翻译：都是技术名词）。
#[derive(Clone, PartialEq, Eq)]
pub struct Hit {
    pub line: usize,
    pub rule: &'static str,
}

pub struct Report {
    pub hits: Vec<Hit>,
    /// 命中的**总条数**（`hits` 按 `MAX_HITS` 截断，这里是截断前的数）
    pub total: usize,
    /// 内容超过 `MAX_SCAN_BYTES`，尾巴没扫到 —— 如实上报，不许静默
    pub truncated: bool,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.hits.is_empty()
    }

    /// 落日志 / 拼给模型看的一句话（含行号，方便用户回去定位）
    pub fn summary(&self) -> String {
        let mut s = self
            .hits
            .iter()
            .map(|h| format!("{} (line {})", h.rule, h.line))
            .collect::<Vec<_>>()
            .join("、");
        if self.total > self.hits.len() {
            s.push_str(&format!(" …（共 {} 处）", self.total));
        }
        if self.truncated {
            s.push_str("；内容超长，只扫了前 512 KB");
        }
        s
    }

    /// `can_use_tool` 的 `analysis.secrets` 载荷
    pub fn json_hits(&self) -> Value {
        Value::Array(
            self.hits
                .iter()
                .map(|h| json!({ "rule": h.rule, "line": h.line }))
                .collect(),
        )
    }
}

/// `(上报名称, 正则, 是否要过占位符闸)`。第三项只对形状自由的两条为 `true`。
const RULES: &[(&str, &str, bool)] = &[
    // ── ① 形状唯一、零误报 ────────────────────────────────────────
    (
        "private key block",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
        false,
    ),
    // ── ② 各家固定前缀的 API key（前缀 + 长度就是判据）─────────────
    ("AWS access key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", false),
    ("GitHub token", r"\bgh[pousr]_[A-Za-z0-9]{36,}\b", false),
    ("OpenAI key", r"\bsk-[A-Za-z0-9]{32,}\b", false),
    ("Anthropic key", r"\bsk-ant-[A-Za-z0-9_\-]{20,}\b", false),
    ("Google API key", r"\bAIza[0-9A-Za-z_\-]{35}\b", false),
    ("Slack token", r"\bxox[baprs]-[0-9A-Za-z\-]{10,}\b", false),
    ("Stripe key", r"\b(?:sk|rk)_live_[0-9A-Za-z]{24,}\b", false),
    // ── ③ 自带结构的凭据 ──────────────────────────────────────────
    (
        "JSON Web Token",
        r"\beyJ[A-Za-z0-9_\-]{10,}\.eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
        false,
    ),
    (
        "Authorization header",
        r"(?i)\b(?:bearer|basic)\s+[A-Za-z0-9\-._~+/]{20,}=*",
        false,
    ),
    // 带口令的连接串：`postgres://user:secret@host/db`。要求口令至少 6 位，
    // 否则 `http://a:b@c` 这种正常的用户名密码占位会把误报拉高。
    (
        "connection string password",
        r"://[^/\s:@]{1,64}:[^/\s:@]{6,64}@",
        false,
    ),
    // ── ④ 形状自由的两条：三道闸一起才报 ──────────────────────────
    //
    // **闸 1 · 键名**：必须含 key / secret / token / passwd / password / credential 之一。
    // 没有它，`path = "/usr/local/lib"` 这类正常赋值会被当成凭据。兼容 `- key: v`
    // （YAML 列表项）与 `export KEY=v`（shell）。
    // 名字用 `[A-Za-z0-9_\-]*` 打头而不是 `[A-Za-z_][A-Za-z0-9_\-]*` —— 后者会把首字母
    // 吃掉，于是**以凭据词开头**的名字（`keyId` / `tokenStore` / `secret1`）反而匹配不上。
    // **闸 2 · 值必须到行尾**：少了它，`let token = self.refresh_token().await?;` 会被
    // 当成 `token = "self.refresh_token"` —— `.` 在值的字符集里，匹配会在 `(` 前停住。
    // 锚定行尾之后，带调用 / 分号的行一律不成立（守门测试 `code_expression_is_not_a_literal`）。
    // **闸 3 · 值不能是占位符**：见 `looks_like_placeholder`。
    (
        "hardcoded credential",
        r#"(?im)^\s*(?:-\s+)?(?:export\s+)?["']?([A-Za-z0-9_\-]*(?:key|secret|token|passwd|password|credential)[A-Za-z0-9_\-]*)["']?\s*[:=]\s*["']([^"'\n]{12,})["']\s*[,;]?\s*$"#,
        true,
    ),
    // `.env` 风格：值不带引号，同样要求到行尾（正因为没有引号，这道闸更关键）
    (
        "hardcoded credential (env)",
        r#"(?im)^\s*(?:-\s+)?(?:export\s+)?["']?([A-Za-z0-9_\-]*(?:key|secret|token|passwd|password|credential)[A-Za-z0-9_\-]*)["']?=([A-Za-z0-9\-_./+=]{12,})\s*$"#,
        true,
    ),
];

struct Rules {
    /// 先跑一次 `RegexSet`：绝大多数待写内容一个 pattern 都不命中，**零额外成本**返回。
    set: RegexSet,
    patterns: Vec<(&'static str, Regex, bool)>,
}

fn rules() -> &'static Rules {
    static RULES_ONCE: OnceLock<Rules> = OnceLock::new();
    RULES_ONCE.get_or_init(|| {
        let mut patterns = Vec::with_capacity(RULES.len());
        for (name, pat, needs_value_gate) in RULES {
            // 规则表是**常量**，编译失败只可能是写错了正则 —— 那是开发期错误，
            // 所以直接 panic（静默跳过一条规则 = 悄悄少一层保护，比崩更危险）。
            let re = Regex::new(pat).unwrap_or_else(|e| panic!("content_safety 规则 {name} 非法: {e}"));
            patterns.push((*name, re, *needs_value_gate));
        }
        let set = RegexSet::new(RULES.iter().map(|(_, p, _)| *p))
            .expect("content_safety 规则集编译失败");
        Rules { set, patterns }
    })
}

/// 扫一段**将被写入**的文本。`text` 由调用方从 `Write.content` / `Edit.new_string` 取。
///
/// **每一处命中都要收**（不是「每条规则只报第一处」）：用户需要知道一共有几个。
/// 展示条数由 `MAX_HITS` 截断，但 `total` 是真实的 —— 截断只在**渲染**这一层发生。
pub fn analyze(text: &str) -> Report {
    let truncated = text.len() > MAX_SCAN_BYTES;
    let head = head_of(text, MAX_SCAN_BYTES);

    let mut all: Vec<Hit> = Vec::new();
    if !head.is_empty() && rules().set.is_match(head) {
        for (name, re, needs_value_gate) in &rules().patterns {
            // 采集上限只防病态输入（一个文件里几万条），正常命中远小于它
            let mut found = 0usize;
            if *needs_value_gate {
                for c in re.captures_iter(head) {
                    let Some(value) = c.get(2) else { continue };
                    if looks_like_placeholder(value.as_str()) {
                        continue;
                    }
                    let Some(m) = c.get(0) else { continue };
                    all.push(Hit { line: line_of(head, m.start()), rule: name });
                    found += 1;
                    if found >= MAX_SCAN_HITS {
                        break;
                    }
                }
            } else {
                for m in re.find_iter(head) {
                    all.push(Hit { line: line_of(head, m.start()), rule: name });
                    found += 1;
                    if found >= MAX_SCAN_HITS {
                        break;
                    }
                }
            }
        }
    }
    all.sort_by(|a, b| (a.line, a.rule).cmp(&(b.line, b.rule)));
    all.dedup();
    let total = all.len();
    all.truncate(MAX_HITS);
    Report {
        hits: all,
        total,
        truncated,
    }
}

/// 占位符闸：`api_key = "your-key-here"` / `"xxxx"` / `"<token>"` / `"${TOKEN}"` 一律放过。
///
/// 判据分三类：**包含已知占位词**、**整串同一个字符**、**带模板语法**。宁可漏报也不能
/// 误报 —— 一份到处写着 `your_api_key_here` 的 README 模板被标成「疑似凭据」，
/// 用户下一次就会直接无视这个标记。
fn looks_like_placeholder(value: &str) -> bool {
    let low = value.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "xxxx", "your_", "your-", "yourkey", "change_me", "changeme", "placeholder",
        "example", "dummy", "sample", "redacted", "secret_here", "key_here", "token_here",
    ];
    if MARKERS.iter().any(|m| low.contains(m)) {
        return true;
    }
    // 模板语法：`<TOKEN>` / `${API_KEY}` / `{{token}}` / `***`
    if low.contains('<') || low.contains("${") || low.contains("{{") || low.contains("***") {
        return true;
    }
    // 整串同一个字符（`aaaaaaaaaaaa` / `000000000000`）
    let mut chars = value.chars();
    if let Some(first) = chars.next() {
        if chars.all(|c| c == first) {
            return true;
        }
    }
    // 值就是键名本身时也算占位：`password = "password"` / `token = "token"`
    const SELF: &[&str] = &["password", "passwd", "secret", "token", "credential", "apikey"];
    SELF.contains(&low.as_str())
}

/// 按字节上限切头，回退到字符边界（切在多字节字符中间会 panic）
fn head_of(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// 偏移量 → 1 起的行号
fn line_of(text: &str, at: usize) -> usize {
    text[..at].bytes().filter(|b| *b == b'\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules_hit(text: &str) -> Vec<&'static str> {
        let mut v: Vec<&'static str> = analyze(text).hits.iter().map(|h| h.rule).collect();
        v.sort_unstable();
        v
    }

    /// 各家固定前缀的密钥都要认出来（形状本身就是判据，零误报）
    ///
    /// **一条样例一次分析**：这里是逐条验证「规则本身对不对」，不是验证汇总行为。
    /// 全塞进一段文本会被 `MAX_HITS`（5）截断 —— 那样测出来的是截断逻辑，不是规则。
    /// 用各家官方文档里的**示例**值：形状对、但都不是真 key。
    #[test]
    fn prefixed_credentials_are_detected() {
        for (want, sample) in [
            ("AWS access key", "aws_access_key_id = AKIAIOSFODNN7EXAMPLE"),
            ("GitHub token", "gh = ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"),
            (
                "OpenAI key",
                "openai = sk-ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd",
            ),
            ("Anthropic key", "anthropic = sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWX"),
            ("Google API key", "google = AIzaSyA1234567890abcdefghijklmnopqrstuv"),
            ("Slack token", "slack = xoxb-123456789012-abcdefghijklmnop"),
            ("Stripe key", "stripe = sk_live_ABCDEFGHIJKLMNOPQRSTUVWX"),
        ] {
            let hit = rules_hit(sample);
            assert!(hit.contains(&want), "{want} 没被认出来：{sample:?} → {hit:?}");
        }
    }

    /// 私钥块 / JWT / Authorization / 连接串口令
    #[test]
    fn structured_credentials_are_detected() {
        let text = concat!(
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n",
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abcdefghijklmnop\n",
            "DB=postgres://appuser:s3cr3t-p4ss@db.internal:5432/app\n",
        );
        let hit = rules_hit(text);
        assert!(hit.contains(&"private key block"), "{hit:?}");
        assert!(hit.contains(&"Authorization header"), "{hit:?}");
        assert!(hit.contains(&"connection string password"), "{hit:?}");
    }

    /// 占位符 / 模板 / 示例值**一个都不能报** —— 误报比漏报更伤（用户会学会无视）
    #[test]
    fn placeholders_are_not_reported() {
        let text = concat!(
            "API_KEY=your_api_key_here\n",
            "SECRET_KEY=xxxxxxxxxxxxxxxxxxxx\n",
            "AUTH_TOKEN=<your-token>\n",
            "CLIENT_SECRET=${CLIENT_SECRET}\n",
            "PASSWORD=change_me\n",
            "apiKey: \"{{apiKey}}\"\n",
            "token = \"token\"\n",
            "password = \"password\"\n",
            "api_key = \"aaaaaaaaaaaaaaaaaa\"\n",
        );
        assert_eq!(rules_hit(text), Vec::<&str>::new(), "占位符被误报了");
    }

    /// 正常代码不该被报：键名不沾凭据词、值再长也不行
    #[test]
    fn ordinary_code_is_clean() {
        let text = concat!(
            "const path = \"/usr/local/share/lunac/themes\";\n",
            "let token = self.refresh_token().await?;\n",
            "password: string;\n",
            "// 用完记得清掉 api_key\n",
            "eval(\"console.log(1)\");\n",
            "new Function(\"return 1\");\n",
            "const url = \"http://localhost:8080/health\";\n",
            "type CredentialStore = HashMap<String, Vec<u8>>;\n",
        );
        let r = analyze(text);
        assert!(r.is_clean(), "正常代码被误报：{}", r.summary());
    }

    /// 通用赋值：三道闸（键名 / 到行尾 / 非占位符）缺一不报
    #[test]
    fn generic_assignment_needs_all_three_gates() {
        // 键名对、值够长、不是占位符 ⇒ 报（带引号与 .env 两种写法都要认）
        let r = analyze("db_password = \"hunter2-looks-real-42\"\n");
        assert_eq!(r.hits.len(), 1, "{}", r.summary());
        assert_eq!(r.hits[0].rule, "hardcoded credential");
        let r = analyze("API_KEY=abc123def456ghi789jkl\n");
        assert_eq!(r.hits.len(), 1, "{}", r.summary());
        assert_eq!(r.hits[0].rule, "hardcoded credential (env)");
        // 键名不对（`path` 不沾凭据词）⇒ 不报
        assert!(analyze("path = \"/usr/local/lib/somelongdir\"\n").is_clean());
        // 值太短 ⇒ 不报
        assert!(analyze("api_key = \"short12\"\n").is_clean());
    }

    /// 「赋值必须到行尾」那道闸的守门测试：带调用 / 分号的正常代码不许被当成字面量
    #[test]
    fn code_expression_is_not_a_literal() {
        for line in [
            "let token = self.refresh_token().await?;\n",
            "api_key = read_from_keyring()?;\n",
            "secret_key = std::env::var(\"SECRET_KEY\")?;\n",
            "password = hash(pw);\n",
        ] {
            assert!(analyze(line).is_clean(), "正常代码被误报：{line}");
        }
    }

    /// 行号要准（审批卡上用户按它回去定位）
    #[test]
    fn line_numbers_are_one_based_and_accurate() {
        let text = "line one\nline two\nGITHUB=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789\n";
        let r = analyze(text);
        assert_eq!(r.hits.len(), 1, "{}", r.summary());
        assert_eq!(r.hits[0].line, 3);
    }

    /// 报的条数有上限，但 `total` 要如实（不许静默截断）
    #[test]
    fn hits_are_capped_but_total_is_honest() {
        let text = (0..8)
            .map(|i| format!("key{i} = \"realvalue{i:04}-x\"\n"))
            .collect::<String>();
        let r = analyze(&text);
        assert_eq!(r.hits.len(), MAX_HITS, "{}", r.summary());
        assert_eq!(r.total, 8, "total 应是截断前的真实数量");
        assert!(r.summary().contains("共"));
    }

    /// 超长内容只扫前 512 KB，且要**如实标注**
    #[test]
    fn oversize_input_is_truncated_and_flagged() {
        let mut text = "x".repeat(MAX_SCAN_BYTES + 10);
        text.push_str("AKIAIOSFODNN7EXAMPLE\n");
        let r = analyze(&text);
        assert!(r.truncated);
        assert!(r.is_clean(), "尾巴上的密钥不该被扫到");
        assert!(r.summary().contains("512 KB"));
    }

    /// 多字节字符铺满时切点回退到字符边界（否则 panic）
    #[test]
    fn head_cut_lands_on_a_char_boundary() {
        let text = "中".repeat(MAX_SCAN_BYTES);
        assert!(head_of(&text, MAX_SCAN_BYTES).len() <= MAX_SCAN_BYTES);
        assert!(analyze(&text).is_clean());
    }
}
