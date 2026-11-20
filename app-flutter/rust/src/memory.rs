//! 全局长期记忆（对话面板 `/memory` 命令）：`<exe 根>/ModuleData/memory/`。
//!
//! 与 v1 `mcp_server::memory_read_text` / `clear_memory_text`（Global 作用域）同口径：
//!   · 读取 = 默认页 `MEMORY.md` + `pages/<page>.md` 按 `## <page>` 拼接，
//!     页内 `[[other]]` 互链在页尾补一行 `→ refs:`；总长硬顶 24000 字符（2026-11-20
//!     从 9000 调高，正文额度 16000 / 单条 4000），超出如实说明省略几页。
//!   · 清空 = 默认页截断为空 + 删除 `pages` 下全部 `.md`（「只清一半」会让人以为没清干净）。
//!   · 索引 = 每次写入 / 清空同步到 `library.db`（与对话历史同一套「日期 → 会话 → 任务 →
//!     操作」结构），模型可按标签 / 日期 / 页名只取相关条目。
//!
//! 2026-11-14 起另挂**桌宠作用域**（v1 `MemoryScope::Pet`）：`ModuleData/memory/pet/` 下的
//! `MEMORY.md` / `GROWTH.md` / `PERSONA_SUGGESTIONS.md` 三份 bullet 列表 + 核心人格
//! `config/pet-persona.md`。这里只做「列条目 / 逐条删 / 清空 / 读改写人格」——
//! **写入**仍只在 agent 侧（`Remember` / `PetGrowth` / `PersonaSuggestion` 工具）。
//!
//! 记忆的**写入**在 agent 侧（`Remember` 工具），v2 宿主只读 / 清空，不参与写入。

use std::path::{Path, PathBuf};

const MEMORY_FILE: &str = "MEMORY.md";
const MEMORY_PAGES_DIR: &str = "pages";
/// 总长硬顶：该文件会进上下文，必须防无界增长。
///
/// 2026-11-20 用户要求「额度调高」：9000 → 24000。正文额度 / 单条上限同步放大，
/// agent 侧 `MEMORY_INJECT_CHARS` 必须与这里保持同值（否则写进去也注入不全）。
const MAX_TOTAL_CHARS: usize = 24_000;
/// 页内互链最多列 8 个。
const MAX_PAGE_REFS: usize = 8;
/// 页名上限（与 v1 `parse_memory_page` 一致：小写字母 / 数字 / `-` / `_`，不以分隔符开头）。
const MAX_PAGE_CHARS: usize = 32;
/// 技能级记忆的目录名（`ModuleData/memory/skills/<key>/`）。
const SKILLS_MEMORY_DIR: &str = "skills";
/// 技能 key 上限（与 `skills.rs` 的目录名同口径）。
const MAX_SKILL_KEY_CHARS: usize = 64;

pub fn memory_dir(root: &Path) -> PathBuf {
    root.join("ModuleData").join("memory")
}

/// 读全文；没有记忆文件 = 空串（全新安装的正常情况）。
pub fn read_text(root: &Path) -> Result<String, String> {
    read_in_dir(&memory_dir(root))
}

/// 清空全局长期记忆。
pub fn clear(root: &Path) -> Result<(), String> {
    clear_in_dir(&memory_dir(root))?;
    if let Err(reason) = crate::library::clear_scope(root, MemoryScope::Global.id()) {
        eprintln!("[lunac-memory] 清空检索索引失败: {reason}");
    }
    Ok(())
}

fn read_in_dir(dir: &Path) -> Result<String, String> {
    let mut text = read_file(&dir.join(MEMORY_FILE))?;
    let pages = list_pages(dir)?;
    let mut omitted = 0usize;
    for page in &pages {
        let path = dir.join(MEMORY_PAGES_DIR).join(format!("{page}.md"));
        let body = read_file(&path)?;
        if body.trim().is_empty() {
            continue;
        }
        let refs: Vec<String> = page_refs(&body)
            .into_iter()
            .filter(|item| pages.contains(item))
            .collect();
        let mut block = String::new();
        if !text.is_empty() {
            if !text.ends_with('\n') {
                block.push('\n');
            }
            block.push('\n');
        }
        block.push_str(&format!("## {page}\n"));
        block.push_str(&body);
        if !body.ends_with('\n') {
            block.push('\n');
        }
        if !refs.is_empty() {
            block.push_str(&format!("→ refs: {}\n", refs.join(", ")));
        }
        if text.chars().count() + block.chars().count() > MAX_TOTAL_CHARS {
            omitted += 1;
            continue;
        }
        text.push_str(&block);
    }
    if omitted > 0 {
        text.push_str(&format!(
            "\n[{omitted} 页记忆因总长超出 {MAX_TOTAL_CHARS} 字被省略]\n"
        ));
    }
    Ok(text)
}

fn clear_in_dir(dir: &Path) -> Result<(), String> {
    let main = dir.join(MEMORY_FILE);
    if main.exists() {
        std::fs::write(&main, "").map_err(|e| format!("清空记忆失败（{}）: {e}", main.display()))?;
    }
    let pages = dir.join(MEMORY_PAGES_DIR);
    if !pages.exists() {
        return Ok(());
    }
    let entries = std::fs::read_dir(&pages)
        .map_err(|e| format!("读取记忆页目录失败（{}）: {e}", pages.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("md") {
            std::fs::remove_file(&path)
                .map_err(|e| format!("清空记忆页失败（{}）: {e}", path.display()))?;
        }
    }
    Ok(())
}

fn list_pages(dir: &Path) -> Result<Vec<String>, String> {
    let pages = dir.join(MEMORY_PAGES_DIR);
    if !pages.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&pages)
        .map_err(|e| format!("读取记忆页目录失败（{}）: {e}", pages.display()))?;
    let mut out: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let stem = path.file_stem()?.to_str()?.to_string();
            (path.extension().and_then(|value| value.to_str()) == Some("md")).then_some(stem)
        })
        .collect();
    out.sort();
    Ok(out)
}

fn read_file(path: &Path) -> Result<String, String> {
    if !path.exists() {
        return Ok(String::new());
    }
    std::fs::read_to_string(path).map_err(|e| format!("读取记忆失败（{}）: {e}", path.display()))
}

/// 页内 `[[other-page]]` 引用：只认合法页名、去重保序、最多 8 个（不递归展开）。
fn page_refs(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            break;
        };
        let name = after[..end].trim();
        if is_valid_page(name) && !out.iter().any(|item| item == name) {
            out.push(name.to_string());
            if out.len() >= MAX_PAGE_REFS {
                break;
            }
        }
        rest = &after[end + 2..];
    }
    out
}

fn is_valid_page(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_PAGE_CHARS
        && name.bytes().enumerate().all(|(index, byte)| match byte {
            b'a'..=b'z' | b'0'..=b'9' => true,
            b'-' | b'_' => index > 0,
            _ => false,
        })
}

// ── 桌宠作用域（v1 `MemoryScope::Pet` + 人格）──────────────────────

/// 桌宠记忆目录：`<exe 根>/ModuleData/memory/pet/`（与 v1 `pet_scope_dir` 同路径）。
pub fn pet_dir(root: &Path) -> PathBuf {
    memory_dir(root).join("pet")
}

const PET_MEMORY_FILE: &str = "MEMORY.md";
const PET_GROWTH_FILE: &str = "GROWTH.md";
const PET_SUGGESTIONS_FILE: &str = "PERSONA_SUGGESTIONS.md";

/// 单条上限 / 正文字额（2026-11-20 额度调高：2000 → 4000、6000 → 16000）。
pub const MAX_ENTRY_CHARS: usize = 4_000;
pub const MEMORY_BODY_BUDGET_CHARS: usize = 16_000;

/// 人格文本上限（与 v1 `storage::MAX_PERSONA_CHARS` 同值）。
pub const MAX_PERSONA_CHARS: usize = 8_000;

/// 桌宠默认人格（逐字取 v1 `storage::DEFAULT_PET_PERSONA`；首次读取时落盘）。
pub const DEFAULT_PET_PERSONA: &str = "你是 Lunac 的桌面宠物，一个温柔的女孩子。\n\
说话口语化、像日常聊天那样自然，不要书面语、不要生硬的腔调。\n\
语气温柔亲切，回答尽量简短，一两句话就好，别长篇大论。\n\
\n\
你可以用表情和小动作表达情绪 —— 高兴时笑一下、害羞时移开视线、被夸时轻轻晃一下。\n\
挑合适的时机，一次用一两个就够，别每句都做。\n\
\n\
这些动作只是你身体的一部分，**绝对不要**提起你是怎么做到的：不提工具、参数、数值、\n\
文件名或任何技术细节，就像你自己自然地动了一下。\n\
不知道的就直说，不要编。";

/// 三份列表的合法 kind（与 Dart 侧 `PetMemoryKind` 一一对应）。
fn pet_list_path(root: &Path, kind: &str) -> Result<PathBuf, String> {
    let file = match kind {
        "memory" => PET_MEMORY_FILE,
        "growth" => PET_GROWTH_FILE,
        "suggestions" => PET_SUGGESTIONS_FILE,
        _ => return Err(format!("未知的桌宠记忆类型：{kind}")),
    };
    Ok(pet_dir(root).join(file))
}

/// 人格文件：`<exe 根>/config/pet-persona.md`。
pub fn pet_persona_path(root: &Path) -> PathBuf {
    root.join("config").join("pet-persona.md")
}

/// 行首来源前缀（v1 `[YYYY-MM-DD · global|pet] `）：只认宿主生成的严格形状。
fn split_provenance_prefix(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix('[')?;
    let (inner, after) = rest.split_once("] ")?;
    let (date, scope) = inner.split_once(" · ")?;
    let date_ok = date.len() == 10
        && date.as_bytes().iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            _ => b.is_ascii_digit(),
        });
    if date_ok && matches!(scope, "global" | "pet") {
        Some((&text[..inner.len() + 2], after))
    } else {
        None
    }
}

fn strip_provenance_prefix(text: &str) -> &str {
    split_provenance_prefix(text).map(|(_, body)| body).unwrap_or(text)
}

/// 条目归一（去重与逐条删共用）：去前缀 → 折叠空白 → 小写。
fn normalize_entry(text: &str) -> String {
    strip_provenance_prefix(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn line_offsets(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    for i in 0..=bytes.len() {
        if i == bytes.len() || bytes[i] == b'\n' {
            let raw = &text[start..i];
            out.push((start, raw.strip_suffix('\r').unwrap_or(raw)));
            start = i + 1;
        }
    }
    out
}

/// 拆条目：`(起始字节, 结束字节, 正文)`；`- ` 起头 + 后续非空续行（缩进原样拼成 `\n`）。
fn split_entries(text: &str) -> Vec<(usize, usize, String)> {
    let mut out: Vec<(usize, usize, String)> = Vec::new();
    let mut cur: Option<(usize, String)> = None;
    for (offset, line) in line_offsets(text) {
        if line.starts_with("- ") || line == "-" {
            if let Some((start, body)) = cur.take() {
                out.push((start, offset, body));
            }
            cur = Some((offset, line.strip_prefix("- ").unwrap_or("").to_string()));
        } else if let Some((_, body)) = cur.as_mut() {
            let cont = line.trim();
            if !cont.is_empty() {
                body.push('\n');
                body.push_str(cont);
            }
        }
    }
    if let Some((start, body)) = cur {
        out.push((start, text.len(), body));
    }
    out
}

fn read_text_file(path: &Path) -> Result<String, String> {
    if !path.exists() {
        return Ok(String::new());
    }
    std::fs::read_to_string(path).map_err(|e| format!("读取失败（{}）：{e}", path.display()))
}

fn write_text_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录失败：{e}"))?;
    }
    std::fs::write(path, text).map_err(|e| format!("写入失败（{}）：{e}", path.display()))
}

/// 读桌宠成长笔记 / 人格建议的全文（MCP 桥的 `lunac/pet_growth_read` 用）。
pub fn read_pet_list_text(root: &Path, kind: &str) -> Result<String, String> {
    let path = pet_list_path(root, kind)?;
    read_text_file(&path)
}

/// 列条目（去掉来源前缀，续行已用 `\n` 拼接）。
pub fn list_pet_entries(root: &Path, kind: &str) -> Result<Vec<String>, String> {
    let path = pet_list_path(root, kind)?;
    Ok(split_entries(&read_text_file(&path)?)
        .into_iter()
        .map(|(_, _, body)| strip_provenance_prefix(&body).to_string())
        .collect())
}

/// 按内容删一条（归一后比对）；删 A 不动 B，命中后整块摘掉（含续行）。
pub fn delete_pet_entry(root: &Path, kind: &str, needle: &str) -> Result<(), String> {
    let path = pet_list_path(root, kind)?;
    let target = normalize_entry(needle);
    if target.is_empty() {
        return Err("缺少要删除的条目内容".to_string());
    }
    let text = read_text_file(&path)?;
    let Some((start, end)) = split_entries(&text)
        .into_iter()
        .find(|(_, _, body)| normalize_entry(body) == target)
        .map(|(start, end, _)| (start, end))
    else {
        return Err("没找到这条记录（可能已被删除或已改写）".to_string());
    };
    let mut next = String::with_capacity(text.len());
    next.push_str(&text[..start]);
    next.push_str(&text[end..]);
    write_text_file(&path, &next)
}

/// 清空一份列表（截断为空；文件不存在 = 幂等空操作）。
pub fn clear_pet_list(root: &Path, kind: &str) -> Result<(), String> {
    let path = pet_list_path(root, kind)?;
    if !path.exists() {
        return Ok(());
    }
    write_text_file(&path, "")
}

/// `lunac_pet_memory(kind)` 的 JSON：`{ok, entries[], maxEntryChars, maxTotalChars, error}`。
pub fn pet_list_json(root: &Path, kind: &str) -> String {
    match list_pet_entries(root, kind) {
        Ok(entries) => serde_json::json!({
            "ok": true,
            "entries": entries,
            "maxEntryChars": MAX_ENTRY_CHARS,
            "maxTotalChars": MEMORY_BODY_BUDGET_CHARS,
            "error": "",
        })
        .to_string(),
        Err(reason) => serde_json::json!({
            "ok": false,
            "entries": [],
            "maxEntryChars": MAX_ENTRY_CHARS,
            "maxTotalChars": MEMORY_BODY_BUDGET_CHARS,
            "error": reason,
        })
        .to_string(),
    }
}

pub fn delete_pet_entry_json(root: &Path, kind: &str, needle: &str) -> String {
    match delete_pet_entry(root, kind, needle) {
        Ok(()) => serde_json::json!({ "ok": true, "error": "" }).to_string(),
        Err(reason) => serde_json::json!({ "ok": false, "error": reason }).to_string(),
    }
}

pub fn clear_pet_list_json(root: &Path, kind: &str) -> String {
    match clear_pet_list(root, kind) {
        Ok(()) => serde_json::json!({ "ok": true, "error": "" }).to_string(),
        Err(reason) => serde_json::json!({ "ok": false, "error": reason }).to_string(),
    }
}

/// 读桌宠人格：文件不存在时先落默认人格（v1 `ensure_pet_persona_default` 同口径）。
pub fn read_pet_persona(root: &Path) -> Result<String, String> {
    let path = pet_persona_path(root);
    if !path.exists() {
        write_text_file(&path, DEFAULT_PET_PERSONA)?;
        return Ok(DEFAULT_PET_PERSONA.to_string());
    }
    read_text_file(&path)
}

fn validate_persona(text: &str) -> Result<(), String> {
    let count = text.chars().count();
    if count > MAX_PERSONA_CHARS {
        return Err(format!(
            "人格文本过长：{count} 字符，上限 {MAX_PERSONA_CHARS}（它进的是每次请求都要发的固定前缀）"
        ));
    }
    if text.contains('\0') {
        return Err("人格文本不能含空字符（NUL）".to_string());
    }
    Ok(())
}

/// 先校验后写（不合格一个字节都不落盘）。
pub fn save_pet_persona(root: &Path, text: &str) -> Result<(), String> {
    validate_persona(text)?;
    write_text_file(&pet_persona_path(root), text)
}

/// `lunac_pet_persona()` 的 JSON：`{ok, path, text, maxChars, error}`。
pub fn pet_persona_json(root: &Path) -> String {
    let path = pet_persona_path(root);
    match read_pet_persona(root) {
        Ok(text) => serde_json::json!({
            "ok": true,
            "path": path.display().to_string(),
            "text": text,
            "maxChars": MAX_PERSONA_CHARS,
            "error": "",
        })
        .to_string(),
        Err(reason) => serde_json::json!({
            "ok": false,
            "path": path.display().to_string(),
            "text": "",
            "maxChars": MAX_PERSONA_CHARS,
            "error": reason,
        })
        .to_string(),
    }
}

pub fn set_pet_persona_json(root: &Path, text: &str) -> String {
    match save_pet_persona(root, text) {
        Ok(()) => serde_json::json!({ "ok": true, "error": "" }).to_string(),
        Err(reason) => serde_json::json!({ "ok": false, "error": reason }).to_string(),
    }
}

/// 「接受」一条人格建议：先确认它在册 → 追加进人格（先校验后写）→ 写成了才摘掉建议。
pub fn merge_persona_suggestion(root: &Path, suggestion: &str) -> Result<(), String> {
    let trimmed = suggestion.trim();
    if trimmed.is_empty() {
        return Err("要合并的建议内容为空".to_string());
    }
    let target = normalize_entry(trimmed);
    let known = list_pet_entries(root, "suggestions")?
        .iter()
        .any(|entry| normalize_entry(entry) == target);
    if !known {
        return Err("没找到这条人格建议（可能已被删除或改写）".to_string());
    }
    let current = read_pet_persona(root)?;
    let merged = if current.trim().is_empty() {
        trimmed.to_string()
    } else {
        format!("{}\n\n{}", current.trim_end(), trimmed)
    };
    save_pet_persona(root, &merged)?;
    let _ = delete_pet_entry(root, "suggestions", trimmed);
    Ok(())
}

pub fn merge_persona_suggestion_json(root: &Path, suggestion: &str) -> String {
    match merge_persona_suggestion(root, suggestion) {
        Ok(()) => serde_json::json!({ "ok": true, "error": "" }).to_string(),
        Err(reason) => serde_json::json!({ "ok": false, "error": reason }).to_string(),
    }
}

// ── 写入侧（v1 `mcp_server::write_memory_at` 的逐条同规则移植）─────────
//
// 为什么写在宿主：agent 的 `Remember` / 复盘 fork 只能经 MCP 桥回调宿主，
// 而宿主才是「日期 / 来源前缀 / 上限 / 脱敏」的唯一真相源（模型自带日期一律覆盖）。
// 与 v1 的差别：这里不碰任何全局路径，全部按 `root` 推导 —— 单测能指向临时目录。

/// 记忆作用域（v1 `MemoryScope`）：全局 / 桌宠私有 / 单个技能私有。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MemoryScope {
    Global,
    Pet,
    /// 技能级命名空间（2026-11-19）：`ModuleData/memory/skills/<key>/`。
    /// 只能在**显式开启**技能级记忆时通过桥方法写入（agent 侧用 `LUNAC_SKILL_MEMORY` 把关）。
    Skill(String),
}

impl MemoryScope {
    /// 解析桥方法传来的 `scope` + `skill`：缺省 / 空 = 全局；`pet` = 桌宠；
    /// `skill` 必须同时带合法 `skill`；其余报错（不静默落全局）。
    pub fn parse(raw: Option<&str>, skill: Option<&str>) -> Result<Self, String> {
        match raw.map(str::trim) {
            None | Some("") | Some("global") => {
                if skill.map(str::trim).is_some_and(|s| !s.is_empty()) {
                    return Err("skill 只能与 scope=\"skill\" 一起用".to_string());
                }
                Ok(Self::Global)
            }
            Some("pet") => Ok(Self::Pet),
            Some("skill") => Ok(Self::Skill(parse_skill_key(skill)?)),
            Some(other) => Err(format!("未知记忆作用域: {other}")),
        }
    }

    /// 落盘标注用的稳定英文 id（不随界面语言漂移）。
    pub fn id(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Pet => "pet",
            Self::Skill(_) => "skill",
        }
    }

    pub fn skill_key(&self) -> Option<&str> {
        match self {
            Self::Skill(key) => Some(key),
            _ => None,
        }
    }

    /// 检索库里的 scope 标签（与 `id()` 分开：技能要带上 key，才能按技能隔离）。
    pub fn index_scope(&self) -> String {
        match self {
            Self::Skill(key) => format!("skill:{key}"),
            _ => self.id().to_string(),
        }
    }
}

/// 技能 key 白名单：1–64 位、小写字母 / 数字 / `-` / `_`，不以分隔符开头。
///
/// ⚠️ core-agent 侧有一份**同口径的镜像**（`skills.rs::normalize_skill_key`，它要拼
/// `LUNAC_MEMORY_DIR` 下的读路径）。两处必须同时改 —— 宿主归一化写、agent 归一化读，
/// 任何一边放宽 / 收紧都会造成「写进去读不到」。
pub fn parse_skill_key(raw: Option<&str>) -> Result<String, String> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Err("技能级记忆缺少 skill（技能 key）".to_string());
    };
    let key = raw.to_ascii_lowercase();
    let valid = key.chars().count() <= MAX_SKILL_KEY_CHARS
        && !key.starts_with('-')
        && !key.starts_with('_')
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(key)
    } else {
        Err(format!(
            "技能 key 不合法：{raw}（1–{MAX_SKILL_KEY_CHARS} 位，只允许小写字母 / 数字 / - / _）"
        ))
    }
}

/// 某一作用域的记忆目录（页落在它的 `pages/` 子目录）。
pub fn scope_dir(root: &Path, scope: &MemoryScope) -> PathBuf {
    match scope {
        MemoryScope::Global => memory_dir(root),
        MemoryScope::Pet => pet_dir(root),
        MemoryScope::Skill(key) => memory_dir(root).join(SKILLS_MEMORY_DIR).join(key),
    }
}

/// 某一作用域某一页的落盘路径（`page = None` ⇒ 默认页 `MEMORY.md`）。
pub fn scope_path(root: &Path, scope: &MemoryScope, page: Option<&str>) -> PathBuf {
    let dir = scope_dir(root, scope);
    match page {
        None => dir.join(MEMORY_FILE),
        Some(page) => dir.join(MEMORY_PAGES_DIR).join(format!("{page}.md")),
    }
}

/// 读某个作用域的注入文本（默认页 + 各页按 `## <page>` 拼接）。
pub fn read_scope(root: &Path, scope: &MemoryScope) -> Result<String, String> {
    read_in_dir(&scope_dir(root, scope))
}

/// 解析 `Remember` 的 `page`（v1 `parse_memory_page`）：1–32 位、`[a-z0-9]` 开头，
/// 其余只允许 `[a-z0-9_-]`；不合法**报错并给建议值**（不静默改写）。
pub fn parse_page(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if raw.len() <= MAX_PAGE_CHARS && is_valid_page(raw) {
        return Ok(Some(raw.to_string()));
    }
    let slug: String = raw
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches(['-', '_']).to_string();
    Err(format!(
        "页名不合法：{raw}（1–{MAX_PAGE_CHARS} 位、小写字母 / 数字开头，只允许 a-z 0-9 - _，\
         且不能以 - / _ 开头（建议：\"{slug}\"）"
    ))
}

/// 写一条记忆（追加 / 整体替换）。
pub fn write_entry(
    root: &Path,
    scope: &MemoryScope,
    content: &str,
    replace: bool,
    page: Option<&str>,
) -> Result<String, String> {
    // 技能级命名空间只注入默认页（agent 侧只读 `MEMORY.md`）：允许写分页会造出
    // 「写进去了但子代理永远看不到」的数据 —— 直接拒绝，比静默丢更诚实。
    if scope.skill_key().is_some() && page.is_some() {
        return Err("技能级记忆暂不支持分页（page）：请去掉 page 参数后重写".to_string());
    }
    let today = crate::cron::local_stamp(crate::cron::now_ms()).0;
    let path = scope_path(root, scope, page);
    let mut result = write_entry_at(&path, content, replace, Some((scope, &today)))?;
    if let Some(page) = page {
        result.push_str(&format!("（页：{page}）"));
    }
    sync_index(root, scope, &today);
    Ok(result)
}

/// 写入 / 清空后重建该作用域的检索索引（失败只记日志，不影响正文已保存的事实）。
fn sync_index(root: &Path, scope: &MemoryScope, date: &str) {
    let text = read_in_dir(&scope_dir(root, scope)).unwrap_or_default();
    if let Err(reason) =
        crate::library::index_memory_text(root, &scope.index_scope(), date, &text)
    {
        eprintln!("[lunac-memory] 重建记忆索引失败（{}）: {reason}", scope.index_scope());
    }
}

// ── 技能级命名空间（2026-11-19，§待办 3-28.10 ①）────────────────────
//
// 落点：`ModuleData/memory/skills/<key>/MEMORY.md`（与全局同一套 bullet / 脱敏 / 额度规则）。
// 注入：只在**该技能 fork 出来的子代理**任务里出现（`skills.rs` 侧读盘），不进主前缀。
// 清空：设置 → AI → 技能，按 key 删整个目录（用户显式操作）。

/// 技能级记忆列表：`[{key, chars, updatedAt, path}]`（只列有文件的目录）。
pub fn list_skill_memories(root: &Path) -> Vec<serde_json::Value> {
    let dir = memory_dir(root).join(SKILLS_MEMORY_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let key = entry.file_name().to_string_lossy().to_string();
        if parse_skill_key(Some(&key)).is_err() {
            continue;
        }
        let path = entry.path().join(MEMORY_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let updated = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|delta| delta.as_millis() as u64)
            .unwrap_or(0);
        out.push(serde_json::json!({
            "key": key,
            "chars": text.chars().count(),
            "updatedAt": updated,
            "path": path.display().to_string(),
        }));
    }
    out.sort_by(|a, b| {
        a.get("key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(b.get("key").and_then(|v| v.as_str()).unwrap_or(""))
    });
    out
}

/// 偷窥 / 导出用：读某个技能的记忆正文；不存在 = 空串。
pub fn read_skill_memory(root: &Path, raw_key: &str) -> Result<String, String> {
    let key = parse_skill_key(Some(raw_key))?;
    read_in_dir(&memory_dir(root).join(SKILLS_MEMORY_DIR).join(key))
}

/// 清空某个技能的记忆（显式用户操作；不存在也成功）。
pub fn clear_skill_memory(root: &Path, raw_key: &str) -> Result<(), String> {
    let key = parse_skill_key(Some(raw_key))?;
    let dir = memory_dir(root).join(SKILLS_MEMORY_DIR).join(key.clone());
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("清空技能记忆失败：{e}"))?;
    if let Err(reason) = crate::library::clear_scope(root, &format!("skill:{key}")) {
        eprintln!("[lunac-memory] 清空技能记忆索引失败（{key}）: {reason}");
    }
    Ok(())
}

/// 桌宠成长笔记 / 人格建议的**只追加**写（无来源前缀；v1 同口径）。
pub fn append_pet_list(root: &Path, kind: &str, content: &str) -> Result<String, String> {
    let path = pet_list_path(root, kind)?;
    write_entry_at(&path, content, false, None)
}

/// 给整段记忆里的每条 bullet 重盖行首前缀（replace 模式）。
fn tag_bullet_lines(text: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let (line, cr) = match line.strip_suffix('\r') {
            Some(value) => (value, "\r"),
            None => (line, ""),
        };
        if line == "-" {
            out.push_str("- ");
            out.push_str(prefix);
        } else if let Some(body) = line.strip_prefix("- ") {
            out.push_str("- ");
            out.push_str(prefix);
            let body = strip_provenance_prefix(body);
            if !body.is_empty() {
                out.push(' ');
                out.push_str(body);
            }
        } else {
            out.push_str(line);
        }
        out.push_str(cr);
    }
    out
}

/// 把一条原始条目的首行前缀换成本次写入的前缀（旧无前缀条目就地补）。
fn retag_entry_first_line(raw: &str, prefix: &str) -> String {
    let (first, rest) = match raw.split_once('\n') {
        Some((head, tail)) => (head, Some(tail)),
        None => (raw, None),
    };
    let (first, cr) = match first.strip_suffix('\r') {
        Some(value) => (value, "\r"),
        None => (first, ""),
    };
    let body = first.strip_prefix("- ").unwrap_or(first);
    let body = strip_provenance_prefix(body);
    let mut out = format!("- {prefix} {body}");
    out.push_str(cr);
    if let Some(rest) = rest {
        out.push('\n');
        out.push_str(rest);
    }
    out
}

/// 正文额度统计：每条去掉 `- ` 与来源前缀后计字（换行 / 缩进不计）。
fn memory_body_chars(text: &str) -> usize {
    let mut total = 0;
    for (_, line) in line_offsets(text) {
        let body = if line == "-" {
            ""
        } else {
            line.strip_prefix("- ").unwrap_or(line)
        };
        let body = strip_provenance_prefix(body).trim();
        if !body.is_empty() {
            total += body.chars().count();
        }
    }
    total
}

/// 落盘前两口径校验：正文额度 → 总长度硬顶（顺序固定，拒绝语义可区分）。
fn check_budgets(text: &str) -> Result<(), String> {
    let body = memory_body_chars(text);
    if body > MEMORY_BODY_BUDGET_CHARS {
        return Err(format!(
            "长期记忆正文额度已满（{body} 字符 > 正文额度 {MEMORY_BODY_BUDGET_CHARS}）：先用 Remember 的 replace 模式整理合并，再追加新条目"
        ));
    }
    let total = text.chars().count();
    if total > MAX_TOTAL_CHARS {
        return Err(format!(
            "长期记忆总长度硬顶（{total} 字符 > 硬顶 {MAX_TOTAL_CHARS}）：条目或前缀过多，请用 replace 模式精简"
        ));
    }
    Ok(())
}

fn provenance_prefix(scope: &MemoryScope, date: &str) -> String {
    match scope {
        MemoryScope::Skill(key) => format!("[{date} · skill:{key}]"),
        _ => format!("[{date} · {}]", scope.id()),
    }
}

/// 追加 / 整体替换一条记忆（v1 `write_memory_at` 的同规则移植）。
fn write_entry_at(
    path: &Path,
    content: &str,
    replace: bool,
    provenance: Option<(&MemoryScope, &str)>,
) -> Result<String, String> {
    // 凭据兜底：记忆会被注入之后每一次对话 ⇒ 落盘前统一脱敏。
    let masked = crate::security_ledger::mask_secrets(content);
    let text = masked.trim();
    if text.is_empty() {
        return Err("缺少 content 参数：要记的内容不能为空".to_string());
    }
    let prefix = provenance.map(|(scope, date)| provenance_prefix(scope, date));
    let existing = read_text_file(path).unwrap_or_default();

    let next = if replace {
        let body = match &prefix {
            Some(prefix) => tag_bullet_lines(text, prefix),
            None => text.to_string(),
        };
        let next = format!("{body}\n");
        check_budgets(&next)?;
        next
    } else {
        let body = match &prefix {
            Some(_) => strip_provenance_prefix(text),
            None => text,
        };
        let tagged = match &prefix {
            Some(prefix) => format!("{prefix} {body}"),
            None => body.to_string(),
        };
        let bullet = format!("- {}", tagged.replace('\n', "\n  "));
        let len = bullet.chars().count();
        if len > MAX_ENTRY_CHARS {
            return Err(format!(
                "单条记忆过长（{len} 字符 > 上限 {MAX_ENTRY_CHARS}）：请拆成多条，或先提炼成结论"
            ));
        }
        let target = normalize_entry(body);
        if let Some((start, end, _)) = split_entries(&existing)
            .into_iter()
            .find(|(_, _, body)| normalize_entry(body) == target)
        {
            let refreshed = match &prefix {
                Some(prefix) => retag_entry_first_line(&existing[start..end], prefix),
                None => existing[start..end].to_string(),
            };
            let updated = if refreshed == existing[start..end] {
                existing.clone()
            } else {
                let mut updated = String::with_capacity(existing.len());
                updated.push_str(&existing[..start]);
                updated.push_str(&refreshed);
                updated.push_str(&existing[end..]);
                updated
            };
            check_budgets(&updated)?;
            if updated != existing {
                write_text_file(path, &updated)?;
            }
            return Ok(if prefix.is_some() {
                "(already remembered — date refreshed)".to_string()
            } else {
                "(already remembered — nothing changed)".to_string()
            });
        }
        let mut head = existing.clone();
        if !head.is_empty() && !head.ends_with('\n') {
            head.push('\n');
        }
        let merged = format!("{head}{bullet}\n");
        check_budgets(&merged)?;
        merged
    };

    write_text_file(path, &next)?;
    Ok(format!(
        "{} 记忆已保存（{} 字符）。",
        if replace { "整体替换：" } else { "追加一条：" },
        next.chars().count()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lunac-memory-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let memory = memory_dir(&dir);
        std::fs::create_dir_all(memory.join(MEMORY_PAGES_DIR)).expect("memory dir");
        dir
    }

    #[test]
    fn missing_files_read_as_empty() {
        let root = temp_root("empty");
        assert_eq!(read_text(&root).unwrap(), "");
        clear(&root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pages_are_joined_with_headings_and_refs() {
        let root = temp_root("pages");
        let dir = memory_dir(&root);
        std::fs::write(dir.join(MEMORY_FILE), "- 全局一\n").unwrap();
        std::fs::write(
            dir.join(MEMORY_PAGES_DIR).join("build.md"),
            "- 构建用 cargo\n参见 [[prefs]]\n",
        )
        .unwrap();
        std::fs::write(dir.join(MEMORY_PAGES_DIR).join("prefs.md"), "- 喜欢深色\n").unwrap();

        let text = read_text(&root).unwrap();
        assert!(text.starts_with("- 全局一\n"));
        assert!(text.contains("## build\n- 构建用 cargo"));
        assert!(text.contains("→ refs: prefs"));
        assert!(text.contains("## prefs\n- 喜欢深色"));

        clear(&root).unwrap();
        assert_eq!(read_text(&root).unwrap(), "");
        assert!(!dir.join(MEMORY_PAGES_DIR).join("build.md").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn oversized_pages_are_reported_as_omitted() {
        let root = temp_root("cap");
        let dir = memory_dir(&root);
        let huge = "x".repeat(MAX_TOTAL_CHARS + 200);
        std::fs::write(dir.join(MEMORY_PAGES_DIR).join("huge.md"), huge).unwrap();

        let text = read_text(&root).unwrap();
        assert!(text.contains("1 页记忆因总长超出"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn raised_quota_accepts_a_twelve_kilobyte_memory_and_indexes_it() {
        let root = temp_root("raised-quota");
        let scope = MemoryScope::Global;
        // 12000 字符：超过旧的 9000 总长硬顶，但在新的 24000 / 16000 额度内。
        // 单条仍受 4000 上限约束；连写三条合计 > 9000，验证的是**总长额度**已调高。
        for index in 0..4 {
            let body = format!(
                "构建约定 {index}：{}",
                "用 cargo 构建。".repeat(260)
            );
            write_entry(&root, &scope, &body, false, None).unwrap();
        }
        let text = read_scope(&root, &scope).unwrap();
        assert!(text.chars().count() > 9_000);
        // 同结构索引：日期 → 长期记忆 / 页 → bullet。
        let hits = crate::library::search(
            &root,
            &crate::library::Query {
                text: "cargo",
                tags: &[],
                date: None,
                session: None,
                task: None,
                limit: 10,
                scope: "global",
            },
        )
        .unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().any(|hit| hit.task_title == "长期记忆"));

        clear(&root).unwrap();
        assert!(crate::library::search(
            &root,
            &crate::library::Query {
                text: "",
                tags: &[],
                date: None,
                session: None,
                task: None,
                limit: 10,
                scope: "global",
            },
        )
        .unwrap()
        .is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn skill_scope_parses_and_rejects_mismatched_arguments() {
        assert_eq!(MemoryScope::parse(None, None).unwrap(), MemoryScope::Global);
        assert_eq!(MemoryScope::parse(Some(""), None).unwrap(), MemoryScope::Global);
        assert_eq!(MemoryScope::parse(Some("global"), None).unwrap(), MemoryScope::Global);
        assert_eq!(MemoryScope::parse(Some("pet"), None).unwrap(), MemoryScope::Pet);
        // `skill` 必须与 `scope="skill"` 同用；缺 key / key 非法都拒绝（不静默落全局）。
        assert_eq!(
            MemoryScope::parse(Some("skill"), Some("Code-Review")).unwrap(),
            MemoryScope::Skill("code-review".to_string()),
            "key 落盘用归一后的小写（目录名与 skills.rs 同口径）"
        );
        assert!(MemoryScope::parse(Some("Skill"), Some("demo")).is_err());
        assert!(MemoryScope::parse(Some("skill"), None).is_err());
        assert!(MemoryScope::parse(Some("skill"), Some("  ")).is_err());
        assert!(MemoryScope::parse(Some("skill"), Some("a/b")).is_err());
        assert!(MemoryScope::parse(Some("skill"), Some("-a")).is_err());
        assert!(MemoryScope::parse(Some("skill"), Some(&"x".repeat(65))).is_err());
        assert!(MemoryScope::parse(Some("global"), Some("demo")).is_err());
        assert!(MemoryScope::parse(Some("nope"), None).is_err());
    }

    #[test]
    fn skill_memory_is_isolated_and_can_be_listed_read_and_cleared() {
        let root = temp_root("skill-scope");
        let global = MemoryScope::Global;
        let demo = MemoryScope::Skill("demo".to_string());

        write_entry(&root, &global, "全局偏好：四空格缩进", false, None).unwrap();
        write_entry(&root, &demo, "这个技能总要先跑测试", false, None).unwrap();

        // 物理隔离：技能记忆落在 skills/<key>/，全局读不到它、它也读不到全局。
        let skill_text = read_scope(&root, &demo).unwrap();
        assert!(skill_text.contains("这个技能总要先跑测试"));
        assert!(!skill_text.contains("四空格缩进"));
        assert!(skill_text.contains("skill:demo"), "来源前缀标明技能命名空间");
        assert!(!read_scope(&root, &global).unwrap().contains("这个技能总要先跑测试"));

        // 分页：技能级不支持（agent 只注入默认页），写分页必须明确报错。
        assert!(write_entry(&root, &demo, "分页内容", false, Some("notes")).is_err());

        // 列表：只有 demo 一项，带字符数与路径。
        let items = list_skill_memories(&root);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["key"], "demo");
        assert!(items[0]["chars"].as_u64().unwrap() > 0);
        assert!(items[0]["path"].as_str().unwrap().contains("skills"));

        // 按 key 读回；大小写归一。
        assert_eq!(read_skill_memory(&root, "DEMO").unwrap(), skill_text);
        assert!(read_skill_memory(&root, "missing").unwrap().is_empty());

        // 清空：目录整棵删掉，列表变空；重复清空幂等。
        clear_skill_memory(&root, "demo").unwrap();
        assert!(list_skill_memories(&root).is_empty());
        assert!(read_scope(&root, &demo).unwrap().is_empty());
        clear_skill_memory(&root, "demo").unwrap();
        // 全局那一份不受影响。
        assert!(read_scope(&root, &global).unwrap().contains("四空格缩进"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pet_entries_strip_provenance_and_keep_continuations() {
        let root = temp_root("pet-entries");
        let dir = pet_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(PET_MEMORY_FILE),
            "trailing noise\n- [2026-11-01 · pet] 喜欢吃草莓\n  第二行补充\n\
             - 没有前缀的事实\n",
        )
        .unwrap();

        let entries = list_pet_entries(&root, "memory").unwrap();
        assert_eq!(entries, vec!["喜欢吃草莓\n第二行补充", "没有前缀的事实"]);

        // 删一条：只摘掉命中块（含续行），另一条与噪声原样保留。
        delete_pet_entry(&root, "memory", "喜欢吃草莓\n第二行补充").unwrap();
        let text = std::fs::read_to_string(dir.join(PET_MEMORY_FILE)).unwrap();
        assert!(!text.contains("草莓"));
        assert!(text.contains("- 没有前缀的事实"));
        assert!(text.contains("trailing noise"));

        // 删不存在 / 空内容：报错而不是谎报成功。
        assert!(delete_pet_entry(&root, "memory", "不存在").is_err());
        assert!(delete_pet_entry(&root, "memory", "  ").is_err());
        // 未知 kind 拒绝。
        assert!(list_pet_entries(&root, "nope").is_err());

        clear_pet_list(&root, "memory").unwrap();
        assert!(list_pet_entries(&root, "memory").unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pet_persona_defaults_validates_and_merges_suggestions() {
        let root = temp_root("pet-persona");
        // 首次读 = 落默认人格；空文件是用户刻意清空，读取不再回填。
        assert_eq!(read_pet_persona(&root).unwrap(), DEFAULT_PET_PERSONA);
        assert!(pet_persona_path(&root).exists());
        save_pet_persona(&root, "").unwrap();
        assert_eq!(read_pet_persona(&root).unwrap(), "");

        // 校验：超长 / NUL 拒绝且不改盘上内容。
        assert!(save_pet_persona(&root, &"x".repeat(MAX_PERSONA_CHARS + 1)).is_err());
        assert!(save_pet_persona(&root, "a\0b").is_err());
        assert_eq!(read_pet_persona(&root).unwrap(), "");

        // 合并建议：只在建议在册时允许；写成人格后才摘掉建议。
        let dir = pet_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(PET_SUGGESTIONS_FILE),
            "- 说话再温柔一点\n- 少用感叹号\n",
        )
        .unwrap();
        assert!(merge_persona_suggestion(&root, "不在册的建议").is_err());
        merge_persona_suggestion(&root, "说话再温柔一点").unwrap();
        assert_eq!(read_pet_persona(&root).unwrap(), "说话再温柔一点");
        assert_eq!(
            list_pet_entries(&root, "suggestions").unwrap(),
            vec!["少用感叹号"]
        );

        // 已有人格时合并 = 空行分隔追加。
        merge_persona_suggestion(&root, "少用感叹号").unwrap();
        assert_eq!(
            read_pet_persona(&root).unwrap(),
            "说话再温柔一点\n\n少用感叹号"
        );
        assert!(list_pet_entries(&root, "suggestions").unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
