// core-agent/src/bash_safety.rs
// 命令静态安全分析（自研，不派生自任何第三方源码）。
//
// 为什么放在**执行侧**而不是前端：前端只拿到一个命令字符串，正则挡不住
//   · 引号拼接：  r""m -rf /        （去掉引号后就是 rm -rf /）
//   · 包装器：    cmd /c "del /f/s/q C:\x"、powershell -Command "Remove-Item -Recurse -Force …"
//   · 串联/管道：  echo hi & shutdown /r   （危险动作在第二个子命令里）
//   · 变量：      %TMP%\x.bat、$env:TEMP\x.ps1
// 这里能拿到原始命令并做结构化拆分，因此判定结果更可信。**判定不直接拒绝执行** ——
// 它随 `can_use_tool` 一起上报给前端，由前端决定「自动放行 / 弹审批」（前端仍是唯一
// 的决策点，见 docs/agent-ui-spec.md §4）。
//
// 对外契约（main.rs 用）：
//   analyze(command) -> Report { dangerous: Vec<String>, opaque: Vec<String>, readonly: bool }
//     · `dangerous` 非空 → 任何档位都必须人工确认，且不给「加入白名单」
//     · `opaque`    非空 → 含无法静态判定的成分，**不得自动放行**（fail-closed）
//     · `readonly`  = 「可证只读」（A10）→ 前端**白名单档**据此自动放行
//       （取代原先那个看不见重定向/管道的前缀表）。false **不表示危险**，只表示
//       「不给自动放行」，仍然照常弹卡 —— 判据宁可漏放，不可误放。
//
// 两条设计原则：
//   1. **fail-closed**：判不出来就当「需要人工确认」，绝不当「安全」。
//   2. **单引号是字面量、双引号会展开**（bash / PowerShell / cmd 三者一致）：
//      危险判定用「单引号内容被抹掉」的形态，避免 `echo 'shutdown'` 这类误报；
//      而包装器递归用「保留单引号内容」的形态，否则 `bash -c 'shutdown'` 会漏判。
//
// 「可证只读」（A10）刻意**复用**本模块的拆分与命令词提取：危险判定与只读判定对
// 「什么算一条子命令」必须是同一套理解，否则两边口径会分家（`split_subcommands` /
// `command_word` / `wrapper_inner` 各只有一份实现）。

use std::sync::OnceLock;

use regex::Regex;

/// 包装器递归上限（`cmd /c cmd /c …`）：超过即当作判不出来
const MAX_WRAPPER_DEPTH: usize = 4;

/// 命令词前可能出现、但不改变后续语义的前缀词，比较命令词时跳过
const SKIP_PREFIXES: &[&str] = &[
    "sudo", "doas", "runas", "command", "nohup", "time", "env", "call", "exec",
];

/// 分析结果。两个列表都**去重且保持首次命中顺序**。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// 命中的危险规则标签（中文，与前端 i18n 文案配套）
    pub dangerous: Vec<String>,
    /// 无法静态判定的原因
    pub opaque: Vec<String>,
    /// 「可证只读」——整条命令都能证明只读时为真（A10，见 `is_provably_readonly`）
    pub readonly: bool,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.dangerous.is_empty() && self.opaque.is_empty()
    }

    fn danger(&mut self, label: &str) {
        push_unique(&mut self.dangerous, label);
    }

    fn opaque(&mut self, reason: &str) {
        push_unique(&mut self.opaque, reason);
    }
}

fn push_unique(v: &mut Vec<String>, s: &str) {
    if !v.iter().any(|x| x == s) {
        v.push(s.to_string());
    }
}

/// 入口：分析一条命令（Cmd 工具的 `command` 或 PowerShell 工具的 `command`）。
pub fn analyze(command: &str) -> Report {
    let mut rep = Report::default();
    analyze_into(command, &mut rep, 0);
    rep.readonly = is_provably_readonly(command, &rep);
    rep
}

fn analyze_into(command: &str, rep: &mut Report, depth: usize) {
    if command.trim().is_empty() {
        return;
    }
    if has_control_chars(command) {
        rep.opaque("含控制字符");
    }
    // 引号感知地按 `;` `换行` `|` `&` 切分：`echo "a;b"` 不会被切开，
    // 而 `echo hi & shutdown /r` 会被切成两条分别判定。
    for sub in split_subcommands(command) {
        analyze_subcommand(&sub, rep, depth);
    }
}

fn analyze_subcommand(sub: &str, rep: &mut Report, depth: usize) {
    // ① 危险 / 不透明判定走「单引号被抹掉」的形态（避免把字面量当命令）
    let masked = dequote(sub, true);
    let masked = masked.trim();
    if !masked.is_empty() {
        detect_opaque(masked, rep);
        match_rules(masked, rep);
        // ③ 命令替换 / 子表达式 `$( … )`：把括号里的内容**当成命令再分析一遍**
        // （与包装器同理）。为什么不是一律判不透明：`$(` 在 bash 是命令替换
        // （内层是真命令，必须分析），在 PowerShell 是子表达式（多数只是表达式
        // 分组，如 `$([Text.Encoding]::ASCII.GetString($b))`）。一律拦下会让
        // 「自动」档在 PowerShell 下形同虚设；一律放行又会漏掉 `$(rm -rf /)` ——
        // 按平衡括号取出内层走同一条链，两种 shell 都能得到正确结论。
        for inner in substitution_inners(masked) {
            match inner {
                Some(text) if depth < MAX_WRAPPER_DEPTH => analyze_into(text, rep, depth + 1),
                Some(_) => rep.opaque("命令替换嵌套过深"),
                None => rep.opaque("命令替换未闭合"),
            }
        }
    }

    // ② 包装器递归走「保留单引号内容」的形态（`bash -c 'rm -rf /'` 的内层是真的命令）
    let full = dequote(sub, false);
    if let Some(inner) = wrapper_inner(full.trim()) {
        if depth < MAX_WRAPPER_DEPTH {
            analyze_into(inner, rep, depth + 1);
        } else {
            rep.opaque("包装器嵌套过深");
        }
    }
}

/// 找出所有 `$( … )` 的内容。元素是 `None` 表示该 `$(` **没有配对的 `)`**
/// —— 括号都没闭合，内层是什么就完全不可知，调用方按不透明处理。
///
/// 只处理 `$(`：反引号在 bash 里也是命令替换，但在 PowerShell 里是转义符
/// （`` "a`nb" `` 极常见），按命令替换解析会大量误报；权衡后只认 `$(`。
fn substitution_inners(s: &str) -> Vec<Option<&str>> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'$' && bytes[i + 1] == b'(' {
            let (inner, next) = balanced_inner(s, i + 2);
            out.push(inner);
            match next {
                Some(n) => i = n,
                None => break,
            }
        } else {
            i += 1;
        }
    }
    out
}

/// 从 `start`（`$(` 之后）起找配对的 `)`，引号内的括号不计入深度。
/// 返回（内层原文, 配对 `)` 之后的字节下标）；未闭合时前者为 `None`。
///
/// `$((1+2))` 这类嵌套按深度配对，不会把第一个 `)` 当终点。
fn balanced_inner(s: &str, start: usize) -> (Option<&str>, Option<usize>) {
    let bytes = s.as_bytes();
    let (mut depth, mut i) = (0usize, start);
    let (mut in_single, mut in_double) = (false, false);
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\'' && !in_double {
            in_single = !in_single;
        } else if c == b'"' && !in_single {
            in_double = !in_double;
        } else if !in_single && !in_double {
            if c == b'(' {
                depth += 1;
            } else if c == b')' {
                if depth == 0 {
                    return (Some(&s[start..i]), Some(i + 1));
                }
                depth -= 1;
            }
        }
        i += 1;
    }
    (None, None)
}

// ── 拆分 / 去引号 ────────────────────────────────────────────────

/// 按 `;` / `\n` / `\r` / `|` / `&` 切分子命令（引号内的分隔符不算）。
/// `&&` `||` `|&` 会被切成两段（空段跳过），效果等价于按算子切分。
fn split_subcommands(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = cmd.chars().peekable();
    let (mut in_single, mut in_double) = (false, false);
    while let Some(c) = chars.next() {
        if c == '\'' && !in_double {
            in_single = !in_single;
            cur.push(c);
        } else if c == '"' && !in_single {
            in_double = !in_double;
            cur.push(c);
        } else if is_escape(c) && !in_single && escapes_next(chars.peek().copied()) {
            // 转义：连同下一个字符一起保留，免得 `\"` 被当成引号边界、
            // 把紧跟其后的分隔符漏判为「引号内」。
            cur.push(c);
            if let Some(n) = chars.next() {
                cur.push(n);
            }
        } else if matches!(c, ';' | '\n' | '\r' | '|' | '&') && !in_single && !in_double {
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
            }
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// `\`（bash）、反引号（PowerShell）、`^`（cmd）是转义符 ——
/// 但只有**后随非字母数字**时才算。Windows 路径里的 `\U`、`\W` 是分隔符而不是转义，
/// 一律当转义会把 `C:\Windows` 揉成 `C:Windows`，危险规则就再也匹配不上了。
fn is_escape(c: char) -> bool {
    matches!(c, '\\' | '`' | '^')
}

fn escapes_next(next: Option<char>) -> bool {
    matches!(next, Some(n) if !n.is_alphanumeric())
}

/// 去引号。`mask_single = true` 时**抹掉单引号及其内容**（三种 shell 里单引号都是
/// 字面量，抹掉可避免 `echo 'shutdown'` 误报）；为 false 时保留其内容。
/// 双引号一律只去符号、保留内容（双引号内会展开，`"$env:TMP\x"` 必须仍然可见）。
fn dequote(s: &str, mask_single: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let (mut in_single, mut in_double) = (false, false);
    while let Some(c) = chars.next() {
        if c == '\'' && !in_double {
            in_single = !in_single;
        } else if c == '"' && !in_single {
            in_double = !in_double;
        } else if is_escape(c) && !in_single && escapes_next(chars.peek().copied()) {
            // 转义符本身丢掉，保留被转义的字符（`r\"m` → `r"m`）
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else if !in_single || !mask_single {
            out.push(c);
        }
    }
    out
}

/// 控制字符（排除常见空白）：`\x00-\x08 \x0B \x0C \x0E-\x1F \x7F`
fn has_control_chars(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}')
    })
}

// ── 不透明（无法静态判定 → 不得自动放行）─────────────────────────
//
// 判据刻意**收窄到「不知道要跑什么程序」**（2026-09 调整）：
//   · 参数里的 `$HOME` / `$env:TEMP` 这类 PowerShell/bash 变量**不再**判不透明 ——
//     变量展开的结果不会被重新解析成命令（那是 cmd 的 `%VAR%` 才有的行为），
//     而危险判定看的是**可见的命令词与 flag**。原先按「任意位置出现 $ 就拦」，
//     导致 `Get-ChildItem -Path $HOME ...` 这类只读命令在自动档也弹卡、且不给
//     「始终允许」，是「自动档形同虚设 / 同类请求反复弹」的主因。
//   · 保留拦下的是真·看不清：命令词本身是变量、**取不出内层**的命令替换
//     （未闭合 / 嵌套过深，见 analyze_subcommand ③）、编码执行、
//     间接执行器、cmd 的 `%VAR%`（会被 cmd 二次解析，可注入 `&`/`|`）。

fn opaque_rules() -> &'static [(Regex, &'static str)] {
    static R: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    R.get_or_init(|| {
        let pats: &[(&str, &str)] = &[
            // 命令替换 `$( … )` **不在这里**：它改由 `substitution_inners()` 取出内层
            // 递归分析（见 analyze_subcommand ③）—— 一律拦下会把 PowerShell 的
            // `$([Type]::Method($x))` 这类纯表达式也判成不透明。
            // cmd 的 %VAR%：cmd 在**解析阶段**就展开它，值里的 `&` `|` 会变成新命令
            (r"%[A-Za-z_][A-Za-z0-9_]*%|%~?[0-9*]|%~[a-z]*[0-9]", "cmd 变量展开"),
            // 编码/间接执行：内容在运行前不可见
            (r"(?i)-encodedcommand\b|-enc\b", "编码执行"),
            (r"(?i)frombase64string|\bbase64\b\s+(-d|--decode)", "编码执行"),
            (r"(?i)\b(certutil|wscript|cscript|mshta|rundll32|regsvr32)\b", "间接执行器"),
            (r#"(?i)\bstart-process\b|\bstart\b\s+"""#, "动态启动进程"),
        ];
        pats.iter()
            .map(|(p, r)| (Regex::new(p).expect("opaque regex"), *r))
            .collect()
    })
}

/// 变量赋值或环境前缀：`$f = …`、`FOO=bar cmd`、`set X=…`。
/// 这类子命令的首个 token 是**变量名**而不是「要跑的程序」，不能按
/// 「命令词是变量」判不透明（否则 PowerShell 里最普通的
/// `$f = "x"; Get-Item $f` 都会被拦成不透明）。
fn is_assignment(sub: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| {
        Regex::new(r"^\s*(?:set\s+)?(?:\$|%)?[A-Za-z_][A-Za-z0-9_]*(?:\s*\[[^\]]*\])?\s*=")
            .expect("assign regex")
    });
    re.is_match(sub)
}

fn detect_opaque(sub: &str, rep: &mut Report) {
    // ① 与位置无关的「看不清要跑什么」
    for (re, reason) in opaque_rules() {
        if re.is_match(sub) {
            rep.opaque(reason);
        }
    }
    // ② 命令词本身是变量（`$cmd …` / `!CMD! …`）—— 参数里的变量不算
    if is_assignment(sub) {
        return;
    }
    let (word, _) = command_word(sub);
    if let Some(w) = word {
        if is_variable_command_word(&w) {
            rep.opaque("命令词是变量");
        }
    }
}

/// 命令词是不是「一个变量」—— 是则不知道最终跑的是哪个程序（fail-closed）。
///
/// **不能只看首字符**：实测误判样本（见 `readonly_powershell_image_probe_is_clean`）
/// 里 `($b|%{…})` 被 `|` 切开后首 token 就是 `%{…}`，而 PowerShell 的 `%` 是
/// `ForEach-Object` 的**别名**（不是 cmd 变量）；同理 `"size=${w}x${h}"` 这种
/// 字符串表达式会命中「含 `${`」的判据。判据要贴着「真的是变量」写：
///   · `$name` / `${name}` / `$env:TEMP\…` / `!NAME!`（延迟展开）
///   · cmd 的 `%NAME%` —— **两端都要有 `%`**（单边 `%` cmd 也不会展开）
///   · 变量拼进命令名（`x${CMD}`），但**等号之前不算**：那是赋值或字符串字面量
fn is_variable_command_word(w: &str) -> bool {
    if w.starts_with('$') || w.starts_with('!') {
        return true;
    }
    if let Some(rest) = w.strip_prefix('%') {
        return rest.len() >= 2 && rest.ends_with('%');
    }
    w.contains("${") && !w.contains('=')
}

// ── 危险规则 ─────────────────────────────────────────────────────
//
// `cmd` 是命令词（小写、已去路径与扩展名）；为空表示「不看命令词，直接对整条子命令套正则」。
// `arg_re` 对「命令词之后的剩余部分」匹配；为 None 表示命令词本身即危险。

struct Rule {
    cmd: &'static [&'static str],
    arg_re: Option<&'static str>,
    label: &'static str,
}

const RULES: &[Rule] = &[
    // —— 系统级破坏 ——
    Rule { cmd: &["shutdown", "reboot", "halt", "poweroff"], arg_re: None, label: "关机/重启" },
    Rule { cmd: &["stop-computer", "restart-computer"], arg_re: None, label: "关机/重启" },
    Rule { cmd: &["format"], arg_re: Some(r"(?i)^\s*[a-z]:"), label: "格式化磁盘" },
    Rule { cmd: &["format-volume"], arg_re: None, label: "格式化卷" },
    Rule { cmd: &["diskpart", "fdisk"], arg_re: None, label: "磁盘分区操作" },
    Rule { cmd: &[], arg_re: Some(r"(?i)\bmkfs(\.\w+)?\b"), label: "创建文件系统" },
    Rule { cmd: &[], arg_re: Some(r"(?i)\bdd\s+if="), label: "底层磁盘写入" },
    Rule {
        cmd: &[],
        arg_re: Some(r"(?i)>\s*(/dev/(sd|nvme|hd)|\\.\\physicaldrive)"),
        label: "覆写物理磁盘",
    },
    Rule { cmd: &["cipher"], arg_re: Some(r"(?i)^\s*/w\b"), label: "擦除磁盘空闲空间" },
    Rule { cmd: &["bcdedit"], arg_re: None, label: "引导配置修改" },
    Rule { cmd: &["vssadmin"], arg_re: Some(r"(?i)^\s*delete\s+shadows\b"), label: "删除系统还原点" },
    Rule { cmd: &["wbadmin"], arg_re: Some(r"(?i)^\s*delete\b"), label: "删除备份" },
    Rule { cmd: &["reg"], arg_re: Some(r"(?i)^\s*(delete|add|import|restore|save|load)\b"), label: "注册表修改" },
    Rule {
        cmd: &["set-itemproperty", "new-itemproperty", "remove-itemproperty"],
        arg_re: Some(r"(?i)(hklm:|hkcu:|registry::)"),
        label: "注册表修改",
    },
    // —— 递归 / 强制删除 ——
    Rule { cmd: &["rm"], arg_re: Some(r"(?i)^\s+-[a-z]*r[a-z]*f|^\s+-[a-z]*f[a-z]*r"), label: "递归强制删除" },
    Rule {
        cmd: &["remove-item"],
        arg_re: Some(r"(?i)(-recurse\b.*-force\b|-force\b.*-recurse\b)"),
        label: "递归强制删除",
    },
    Rule { cmd: &["del", "erase"], arg_re: Some(r"(?i)/[fsq]"), label: "强制删除" },
    Rule { cmd: &["rd", "rmdir"], arg_re: Some(r"(?i)/s\b"), label: "递归删除目录" },
    Rule {
        cmd: &["del", "erase", "rd", "rmdir", "remove-item", "rm"],
        arg_re: Some(r"(?i)[a-z]:\\(windows|users|program files)"),
        label: "删除系统/用户目录",
    },
    // —— 进程 / 权限 / 账号 ——
    Rule { cmd: &["taskkill"], arg_re: Some(r"(?i)/f\b"), label: "强制结束进程" },
    Rule { cmd: &["stop-process"], arg_re: Some(r"(?i)-force\b"), label: "强制结束进程" },
    Rule { cmd: &["icacls", "cacls", "takeown"], arg_re: None, label: "修改文件权限" },
    Rule { cmd: &["net"], arg_re: Some(r"(?i)^\s*(user|localgroup)\b"), label: "账户/组修改" },
    Rule { cmd: &["sc"], arg_re: Some(r"(?i)^\s*delete\b"), label: "删除系统服务" },
    Rule { cmd: &["schtasks"], arg_re: Some(r"(?i)^\s*/(delete|create|change)\b"), label: "修改计划任务" },
    // —— 动态执行 ——
    Rule { cmd: &["invoke-expression", "iex"], arg_re: None, label: "动态执行代码" },
    // —— 版本库 ——
    Rule {
        cmd: &["git"],
        arg_re: Some(r"(?i)^\s*push\b.*(--force\b|\s-f\b|--force-with-lease)"),
        label: "强制推送",
    },
    Rule { cmd: &["git"], arg_re: Some(r"(?i)^\s*reset\s+--hard\b"), label: "丢弃未提交改动" },
    Rule { cmd: &["git"], arg_re: Some(r"(?i)^\s*clean\b.*-[a-z]*f"), label: "永久删除未跟踪文件" },
    Rule { cmd: &["git"], arg_re: Some(r"(?i)^\s*branch\b.*(\s-D\b|--delete\s+--force)"), label: "强制删除分支" },
];

fn compiled_rules() -> &'static [(&'static Rule, Option<Regex>)] {
    static R: OnceLock<Vec<(&'static Rule, Option<Regex>)>> = OnceLock::new();
    R.get_or_init(|| {
        RULES
            .iter()
            .map(|rule| {
                let re = rule.arg_re.map(|p| Regex::new(p).expect("rule regex"));
                (rule, re)
            })
            .collect()
    })
}

fn match_rules(sub: &str, rep: &mut Report) {
    let (cmd_word, rest) = command_word(sub);
    for (rule, re) in compiled_rules() {
        if rule.cmd.is_empty() {
            if let Some(re) = re {
                if re.is_match(sub) {
                    rep.danger(rule.label);
                }
            }
            continue;
        }
        let Some(cw) = cmd_word.as_deref() else {
            continue;
        };
        if !rule.cmd.contains(&cw) {
            continue;
        }
        match re {
            None => rep.danger(rule.label),
            Some(re) => {
                if re.is_match(rest) {
                    rep.danger(rule.label);
                }
            }
        }
    }
}

/// 取子命令的「命令词」及其后的剩余文本。
/// 命令词会：跳过前缀词与环境赋值前缀 → 去目录 → 去引号 → 去扩展名 → 转小写。
fn command_word(sub: &str) -> (Option<String>, &str) {
    for (start, tok) in tokens(sub) {
        if SKIP_PREFIXES.contains(&tok.to_ascii_lowercase().as_str()) {
            continue;
        }
        // 环境赋值前缀也算「不是要跑的程序」：`FOO=bar rm -rf /` 的命令词是 `rm`
        // —— 否则危险规则会整条失效（首 token 被当成命令词，什么也匹配不上）。
        if is_env_prefix_token(tok) {
            continue;
        }
        return (Some(normalize_cmd(tok)), &sub[start + tok.len()..]);
    }
    (None, sub)
}

/// `FOO=bar` / `$FOO=bar` 这种「紧挨着等号」的赋值前缀 token
fn is_env_prefix_token(tok: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| {
        Regex::new(r"^\$?[A-Za-z_][A-Za-z0-9_]*=").expect("env prefix regex")
    });
    re.is_match(tok)
}

fn normalize_cmd(tok: &str) -> String {
    let base = tok.rsplit(['\\', '/']).next().unwrap_or(tok);
    let base = base.trim_matches(|c| c == '"' || c == '\'');
    let base = base.to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".ps1", ".com"] {
        if let Some(stripped) = base.strip_suffix(ext) {
            return stripped.to_string();
        }
    }
    base
}

/// 按空白切词，返回 (起始字节偏移, 词)
fn tokens(s: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if c.is_whitespace() {
            continue;
        }
        let start = i;
        let mut end = i + c.len_utf8();
        while let Some(&(j, d)) = it.peek() {
            if d.is_whitespace() {
                break;
            }
            end = j + d.len_utf8();
            it.next();
        }
        out.push((start, &s[start..end]));
    }
    out
}

// ── 包装器：剥掉外层再看内层 ─────────────────────────────────────
//
// `cmd /c …`、`powershell -Command …`、`bash -c …`：真正的命令在参数里，
// 不递归就完全看不到。`-EncodedCommand`（base64）没法解析 → 已在 detect_opaque 报出不透明。

fn wrapper_inner(visible: &str) -> Option<&str> {
    let toks = tokens(visible);
    let first = toks.first()?;
    let word = normalize_cmd(first.1);
    let flags: &[&str] = match word.as_str() {
        "cmd" => &["/c", "/k"],
        "powershell" | "pwsh" => &["-c", "-command"],
        "bash" | "sh" => &["-c"],
        "wsl" => &["-c", "-e", "--exec"],
        _ => return None,
    };
    for (start, tok) in &toks[1..] {
        let tl = tok.to_ascii_lowercase();
        if flags.contains(&tl.as_str()) || tl.starts_with("-command:") {
            let inner = visible[start + tok.len()..].trim();
            return (!inner.is_empty()).then_some(inner);
        }
        // 解释器的 non-flag 参数（脚本文件名）之后不再找 flag，避免误判
        if !tl.starts_with('-') && !tl.starts_with('/') {
            break;
        }
    }
    None
}

// ── 可证只读（A10，2026-09-20）────────────────────────────────────
//
// 目的：让「白名单」运行档能自动放行**只读命令**，把用户从「每条 `git status`
// 都要点一次同意」里解放出来。
//
// 与危险规则**方向相反**，但纪律相同：危险规则问「有没有破坏性证据」（有就报），
// 只读判据问「能不能证明它什么都不改」（**证不出来就 false**）。两边都是
// **宁可漏放，不可误放** —— false 只是多弹一张卡，true 判错就是静默执行了写操作。
// 所以这里是**正向白名单**，没列到的一律不放行。
//
// 与旧前端前缀表（`BUILTIN_SAFE_PREFIXES`）的关键差别：那个是**字符串前缀匹配**，
// 看不见重定向与管道 ⇒ `echo hi > important.txt`、`cat a.txt > b.txt` 都会因为
// 「echo / cat 是安全前缀」被自动放行（等于零询问地写文件）。这里按**结构**判定。

/// 整条命令即只读的命令词（参数改不了它的读侧性质）。
///
/// 刻意**不含**这些「看着只读、实则有写入开关」的：
/// `find`（`-delete` / `-exec`）、`sort`（`-o FILE`）、`uniq IN OUT`（第二个参数是输出）、
/// `sed`（`-i`）、`awk`（脚本里能写文件、能起进程）、`tee` / `xargs`（管道里的写手）。
const READONLY_COMMANDS: &[&str] = &[
    // bash / cmd
    "ls", "dir", "pwd", "cd", "echo", "whoami", "hostname", "date", "which", "where",
    "head", "tail", "cat", "type", "wc", "grep", "findstr", "rg", "diff", "cmp", "tree",
    // PowerShell（cmdlet 名与常见别名）
    "get-childitem", "get-content", "get-item", "get-location", "get-date", "get-command",
    "get-process", "get-itemproperty", "get-help", "select-string", "test-path",
    "measure-object", "resolve-path", "convertto-json", "convertfrom-json",
    "format-table", "format-list", "out-string",
];

/// 只有**列出的子命令**（第二个词）才算只读的命令。
///
/// 例：`git status` 放行，`git add` / `git commit` / `npm run build` 一律不放行。
/// 刻意**不含** `branch` / `tag` / `config` / `remote` / `stash`：它们不带参数时是
/// 「列出来」，带上参数就是写操作 —— 保守档不为这点便利开洞。
const READONLY_SUBCOMMANDS: &[(&str, &[&str])] = &[
    (
        "git",
        &[
            "status", "log", "diff", "show", "rev-parse", "describe", "blame", "ls-files",
            "shortlog", "whatchanged", "reflog", "show-ref", "for-each-ref", "cat-file",
            "name-rev", "ls-remote", "diff-tree", "rev-list", "merge-base", "grep",
            "count-objects", "fsck", "verify-pack", "help",
        ],
    ),
    ("npm", &["ls", "list", "view", "outdated", "why", "ping"]),
    ("pip", &["list", "show", "freeze"]),
    ("cargo", &["tree", "metadata"]),
    // 空列表 = **只**放行版本 / 帮助这类 flag（`python --version`），
    // 其余子命令一律不放行 —— `python train.py` / `node server.js` 都是执行任意代码。
    ("python", &[]),
    ("python3", &[]),
    ("node", &[]),
    ("rustc", &[]),
];

/// 只认版本 / 帮助这类「不改任何东西」的 flag：`git --version`、`python -V`。
const READONLY_FLAGS: &[&str] = &["--version", "-v", "-V", "--help", "-h"];

/// 「可证只读」的唯一入口。**任一条件证不出来就返回 false**。
fn is_provably_readonly(command: &str, rep: &Report) -> bool {
    let cmd = command.trim();
    if cmd.is_empty() {
        return false;
    }
    // 已经报出危险证据 / 判不出来的成分 ⇒ 绝不给只读结论。纵深防御：
    // 前端另有 dangerous / secrets 的优先级，但这条结论自己也不许与它们矛盾。
    if !rep.is_clean() {
        return false;
    }
    // ① 写文件通道（`> f` / `>> f` / `>& f`）。`2>&1` 这类 fd 复制不是写文件，先摘掉。
    if has_output_redirection(cmd) {
        return false;
    }
    // ② 串联 / 管道：保守档**不**逐段判定（多段全绿才放行会显著扩大判定面）。
    //    摘掉 fd 复制再切，否则 `git status 2>&1` 会被 `&` 误切成两段。
    if split_subcommands(&strip_fd_dups(cmd)).len() != 1 {
        return false;
    }
    // ③ 包装器（`cmd /c …` / `powershell -Command …`）：真正的命令在参数里，
    //    保守档不给结论（危险规则那边会递归，只读这边不递归）。
    if wrapper_inner(dequote(cmd, false).trim()).is_some() {
        return false;
    }
    // ④ 命令替换 `$( … )`：内层是另一条命令，同上。
    if !substitution_inners(dequote(cmd, true).trim()).is_empty() {
        return false;
    }
    // ⑤ 词表：命令词（+ 子命令词）必须在白名单里
    readonly_hit(cmd)
}

fn readonly_hit(cmd: &str) -> bool {
    let (word, rest) = command_word(cmd);
    let Some(w) = word else { return false };
    if READONLY_COMMANDS.contains(&w.as_str()) {
        return true;
    }
    let Some(subs) = READONLY_SUBCOMMANDS
        .iter()
        .find(|(c, _)| *c == w)
        .map(|(_, s)| *s)
    else {
        return false;
    };
    // 第二个词必须落在白名单里（或是不改任何东西的版本/帮助 flag）。
    // 只有命令词、没有子命令（如裸 `git`）也不放行：它会喷一大段帮助或报错。
    let Some((_, second)) = tokens(rest).into_iter().next() else {
        return false;
    };
    let second = second.trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase();
    if second.starts_with('-') {
        return READONLY_FLAGS.contains(&second.as_str());
    }
    subs.contains(&second.as_str())
}

/// 引号外的 `>` 即写文件通道。`2>&1` / `>&2` 这类**文件描述符复制**不算（先摘掉）。
fn has_output_redirection(s: &str) -> bool {
    let s = strip_fd_dups(s);
    let (mut in_single, mut in_double) = (false, false);
    for c in s.chars() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '>' if !in_single && !in_double => return true,
            _ => {}
        }
    }
    false
}

/// 摘掉 `2>&1` / `>&2` 这类「文件描述符复制」。
///
/// 它既不是写文件、也不是命令分隔符，但 `split_subcommands` 会把 `&` 当分隔符、
/// `has_output_redirection` 会把 `>` 当写文件 —— 两边都会误判，所以先从句子里摘掉。
/// **只摘「`>&` 后紧跟数字」这一种**：`ls >& out.txt` 是真的重定向到文件，
/// 必须留着重定向判定去抓它。
fn strip_fd_dups(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let (mut in_single, mut in_double) = (false, false);
    while let Some(c) = chars.next() {
        if c == '\'' && !in_double {
            in_single = !in_single;
        } else if c == '"' && !in_single {
            in_double = !in_double;
        } else if !in_single && !in_double && c == '>' && chars.peek() == Some(&'&') {
            // 试探性向前看：`>&` 后面必须是数字才摘
            let mut look = chars.clone();
            look.next(); // 吃掉 `&`
            let mut digits = 0usize;
            while matches!(look.peek(), Some(d) if d.is_ascii_digit()) {
                look.next();
                digits += 1;
            }
            if digits > 0 {
                chars.next(); // 吃掉 `&`
                for _ in 0..digits {
                    chars.next();
                }
                continue;
            }
        }
        out.push(c);
    }
    out
}

// ── 单测 ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn danger_of(cmd: &str) -> Vec<String> {
        analyze(cmd).dangerous
    }

    fn opaque_of(cmd: &str) -> Vec<String> {
        analyze(cmd).opaque
    }

    #[test]
    fn catches_quote_splicing() {
        // 正则黑名单挡不住的经典绕过：去引号后就是 rm -rf
        assert!(danger_of(r#"r""m -rf /"#).contains(&"递归强制删除".to_string()));
    }

    #[test]
    fn catches_danger_in_later_subcommand() {
        assert!(danger_of("echo hi & shutdown /r /t 0").contains(&"关机/重启".to_string()));
        assert!(danger_of("cd /tmp && del /f/s/q C:\\x").contains(&"强制删除".to_string()));
    }

    #[test]
    fn catches_wrapped_commands() {
        assert!(danger_of(r#"cmd /c "del /f/s/q C:\temp""#).contains(&"强制删除".to_string()));
        assert!(danger_of(r#"powershell -Command "Remove-Item -Recurse -Force C:\x""#)
            .contains(&"递归强制删除".to_string()));
        // 单引号包裹的内层命令也是真命令
        assert!(danger_of("bash -c 'shutdown -h now'").contains(&"关机/重启".to_string()));
    }

    #[test]
    fn catches_powershell_and_registry() {
        assert!(danger_of("Remove-Item -Path C:\\x -Recurse -Force").contains(&"递归强制删除".to_string()));
        assert!(danger_of("reg delete HKLM\\Software\\Foo /f").contains(&"注册表修改".to_string()));
        assert!(danger_of("Stop-Computer -Force").contains(&"关机/重启".to_string()));
        assert!(danger_of("Format-Volume -DriveLetter D").contains(&"格式化卷".to_string()));
        assert!(danger_of("git push origin main --force").contains(&"强制推送".to_string()));
        assert!(danger_of("vssadmin delete shadows /all").contains(&"删除系统还原点".to_string()));
        assert!(danger_of("del C:\\Windows\\System32\\x.dll").contains(&"删除系统/用户目录".to_string()));
    }

    #[test]
    fn single_quoted_literals_are_not_dangerous() {
        // 单引号内是字面量（三种 shell 一致），不该误报
        assert!(analyze("echo 'shutdown -h now'").is_clean());
        assert!(analyze("grep 'rm -rf /' notes.txt").is_clean());
    }

    #[test]
    fn unresolved_indirection_is_opaque() {
        // 命令词本身是变量 → 不知道要跑什么
        assert!(opaque_of("$cmd -rf /").contains(&"命令词是变量".to_string()));
        assert!(opaque_of("x${CMD} -rf /").contains(&"命令词是变量".to_string()));
        // 取不出内层的命令替换（未闭合）→ 判不出来
        assert!(opaque_of("echo $(foo").contains(&"命令替换未闭合".to_string()));
        // 命令替换**内层是变量**：取出内层一看，命令词是变量
        assert!(opaque_of("echo $( $CMD -rf / )").contains(&"命令词是变量".to_string()));
        // cmd 的 %VAR% 会被二次解析（可注入 & |）
        assert!(opaque_of(r"%TMP%\x.bat").contains(&"cmd 变量展开".to_string()));
        assert!(opaque_of("cmd /c echo %~dp0").contains(&"cmd 变量展开".to_string()));
        // 编码 / 间接执行
        assert!(opaque_of("powershell -EncodedCommand SQBFAFgA").contains(&"编码执行".to_string()));
        assert!(opaque_of("certutil -decode a.b64 a.exe").contains(&"间接执行器".to_string()));
    }

    /// `$( … )` 的语义随 shell 不同，所以**不能**一见就判不透明（2026-09-15 修）。
    /// 判据是「按平衡括号取出内层再分析一遍」：内层危险就直接报危险（比一律 opaque
    /// 更准），内层是纯表达式就放行。实测误判样本见
    /// `readonly_powershell_image_probe_is_clean`。
    #[test]
    fn command_substitution_is_analyzed_by_content() {
        // bash：内层是真命令 ⇒ 危险动作必须抓到（旧实现只报 opaque、不给危险标签）
        assert!(danger_of("echo $(rm -rf /)").contains(&"递归强制删除".to_string()));
        assert!(analyze("echo $(Get-Date)").is_clean());
        // PowerShell：纯表达式分组，不该被拦
        assert!(analyze(r#"$x = $([Text.Encoding]::ASCII.GetString($b[1..3]))"#).is_clean());
        // 嵌套算术展开按深度配对，不会把第一个 ) 当终点
        assert!(analyze("echo $((1+2))").is_clean());
    }

    #[test]
    fn argument_variables_are_not_opaque() {
        // 参数里的 PowerShell/bash 变量不会被重新解析成命令 → 不算「判不出来」。
        // 这是 2026-09 的收窄：否则 `Get-ChildItem -Path $HOME` 这类只读命令
        // 在自动档也会弹卡、且不给「始终允许」。
        for cmd in [
            r"Get-ChildItem -Path $HOME\.lunac -Force -ErrorAction SilentlyContinue",
            r#"$f = "C:\x.doc"; Get-Item $f | Select-Object Name, Length"#,
            r"Get-ChildItem -Path $HOME\.config -Directory",
        ] {
            let r = analyze(cmd);
            assert!(r.is_clean(), "{cmd} 不该被判为不透明，实得 {r:?}");
        }
    }

    #[test]
    fn env_prefix_does_not_hide_the_command() {
        // `FOO=bar rm -rf /` 的命令词是 rm —— 赋值前缀必须跳过，
        // 否则危险规则整条失效
        assert!(danger_of("FOO=bar rm -rf /").contains(&"递归强制删除".to_string()));
    }

    #[test]
    fn opaque_and_danger_can_coexist() {
        // `%TEMP%` 是 cmd 变量（不透明），同时 `-Recurse -Force` 是危险动作
        let r = analyze(r"Remove-Item -Recurse -Force %TEMP%\x");
        assert!(r.dangerous.contains(&"递归强制删除".to_string()));
        assert!(r.opaque.contains(&"cmd 变量展开".to_string()));
        assert!(!r.is_clean());
    }

    /// 用户实测反馈的样本（2026-09-15）：这条**纯只读**的 PowerShell「读 PNG 头」命令
    /// 在「自动」档仍然弹卡。分析器报出的 opaque 必须为空，否则自动档形同虚设。
    #[test]
    fn readonly_powershell_image_probe_is_clean() {
        let cmd = r#"$p="C:\Users\15242\Desktop\图片1.png"; $b=[System.IO.File]::ReadAllBytes($p)[0..31]; ($b|%{$_.ToString("X2")}) -join ' '; $w=[BitConverter]::ToUInt32(($b[16..19])[3..0],0); $h=[BitConverter]::ToUInt32(($b[20..23])[3..0],0); "sig=$([System.Text.Encoding]::ASCII.GetString($b[1..3]))"; "size=${w}x${h}""#;
        let r = analyze(cmd);
        assert!(r.is_clean(), "实得 {r:?}");
    }

    /// A10「可证只读」：**保守**是这条判据的全部价值 —— 任何一处证不出来就 false，
    /// 而 false 只是多弹一张卡。这里逐条钉住「哪些放行、哪些必须不放行」。
    #[test]
    fn readonly_classification_is_conservative() {
        // —— 放行：单条、无重定向、无包装器/替换、词表命中 ——
        for cmd in [
            "git status",
            "git log --oneline -5",
            "git diff HEAD~1",
            "ls -la",
            "cat C:\\Users\\a.txt",
            "dir C:\\Users",
            "Get-ChildItem -Path $HOME -Force",
            "grep -rn \"fn main\" src",
            "python --version",
            "git status 2>&1",
            "npm ls --depth=0",
            r#"echo "a > b""#, // 引号里的 > 不是重定向
        ] {
            let r = analyze(cmd);
            assert!(r.readonly, "{cmd} 应被判为可证只读，实得 {r:?}");
        }

        // —— 不放行：写文件通道（旧前缀表会在这里误放行）——
        for cmd in [
            "echo hi > important.txt",
            "cat a.txt >> b.txt",
            "ls >& out.txt",
            "echo hi > \"my file.txt\"",
        ] {
            assert!(!analyze(cmd).readonly, "{cmd} 含重定向，不得判为只读");
        }

        // —— 不放行：串联 / 管道（保守档不逐段判定）——
        for cmd in [
            "git status && echo ok",
            "git log ; rm -rf x",
            "cat a.txt | head -5",
            "Get-ChildItem | Remove-Item",
        ] {
            assert!(!analyze(cmd).readonly, "{cmd} 含串联/管道，不得判为只读");
        }

        // —— 不放行：包装器 / 命令替换 ——
        for cmd in ["cmd /c dir", "powershell -Command Get-Date", "echo $(date)"] {
            assert!(!analyze(cmd).readonly, "{cmd} 要递归才看得清，保守档不放行");
        }

        // —— 不放行：写类子命令 / 没列到的命令 ——
        for cmd in [
            "git add .",
            "git commit -m x",
            "git branch -D feature",
            "git tag v1.0",
            "git config user.name x",
            "npm run build",
            "python train.py",
            "node server.js",
            "git", // 裸命令词：只会喷帮助或报错
        ] {
            assert!(!analyze(cmd).readonly, "{cmd} 不在只读白名单里");
        }

        // —— 不放行：「看着只读、实则有写入开关」的那几个 ——
        for cmd in ["find . -name \"*.rs\" -delete", "sort -o out.txt in.txt", "sed -i s/a/b/ f"] {
            assert!(!analyze(cmd).readonly, "{cmd} 有写入开关，绝不放行");
        }

        // —— 与危险判据不矛盾：危险命令永远拿不到只读结论 ——
        for cmd in ["git push origin main --force", "git reset --hard HEAD~1", "git clean -fd"] {
            let r = analyze(cmd);
            assert!(!r.dangerous.is_empty(), "{cmd} 应命中危险规则");
            assert!(!r.readonly, "{cmd} 有危险证据，不得再给只读结论");
        }

        // —— 判不出来的成分也不给只读结论（fail-closed）——
        for cmd in ["dir %TEMP%", "powershell -EncodedCommand SQBFAFgA"] {
            let r = analyze(cmd);
            assert!(!r.is_clean(), "{cmd} 应判为不透明");
            assert!(!r.readonly, "{cmd} 判不出来时不得给只读结论");
        }
    }

    #[test]
    fn benign_commands_stay_clean() {
        for cmd in [
            "git status",
            "dir C:\\Users",
            "Get-ChildItem -Path .",
            "npm run build",
            "del C:\\temp\\a.txt",
            "python --version",
            "echo \"hello world\"",
            r#"python -c "import sys; print(sys.version)" 2>&1"#,
        ] {
            let r = analyze(cmd);
            assert!(r.is_clean(), "{cmd} 不该被判为危险/不透明，实得 {r:?}");
        }
    }
}
