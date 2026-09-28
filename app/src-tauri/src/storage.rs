// src-tauri/src/storage.rs
// File-based persistent storage for chat history, clipboard history and memo.
// 数据目录（2026-09 修订）：全部缓存与业务/插件数据统一放「exe 安装根目录」，
// 即可执行文件所在目录，使应用数据结构清晰、随卸载一并清除：
//   - <exe_dir>\ModuleData\history\chat.db（会话历史，SQLite。**2026-09-17 起**；同目录的
//     chat-history.json 只是迁移前的旧文件，库建好后即不再是真相源，见 chat_db.rs）
//   - <exe_dir>\ModuleData\history\clipboard-history.json
//   - <exe_dir>\ModuleData\memo\memo.json（备忘录，含图片 images\<id>\）
//   - <exe_dir>\ModuleData\custom\app_registry.json（自定义启动项）
//   - <exe_dir>\temp\webview-data（WebView2 用户数据/缓存，见 main.rs）
//   - <exe_dir>\temp\app-index-cache.json（应用扫描缓存，见 app_indexer.rs）
//   - <exe_dir>\temp\transStorage（**agent 的默认工作目录**：未配置工作区时 agent 的 cwd，
//     模型写的临时/草稿文件落在这里 —— 以前回退用户主目录，会堆到 C:\Users\<名> 根下，
//     见 commands.rs 的 default_work_dir() 与 ai-spec §11 规则 34）
//   - <exe_dir>\temp\logs、<exe_dir>\temp\tool-outputs（落盘日志与超长工具输出，见 log.rs）
//   - <exe_dir>\skills、<exe_dir>\tools、<exe_dir>\Modules（**插件**，2026-09-28 由 plugins\
//     改名；每个插件一个子目录，内含 lunac-plugin.json + 已编译的 ESM 入口 + 它自己的依赖）、
//     <exe_dir>\paddle-ocr
//   - <exe_dir>\config（ai.json 凭据 / hotkey.json 热键 / hooks.json 权限 hooks /
//     pricing.json 定价表 —— 都是「应用配置」，业务数据才进 ModuleData）
// 旧版本数据曾放在 %LOCALAPPDATA%\Lunac(-dev)，首次启动由
// migrate_legacy_localappdata() 整体搬移后删除。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// 应用数据根目录 = 可执行文件所在目录（exe 安装根）。
///   release → 安装目录；dev → target\debug。dev/release 数据因此天然隔离。
/// 所有数据落盘模块都应调用本函数，禁止各自硬编码路径。
pub fn lunac_root_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 业务数据根目录：<exe_dir>\ModuleData
pub fn module_data_dir() -> PathBuf {
    lunac_root_dir().join("ModuleData")
}

// ── AI 配置（<exe_dir>\config\ai.json）───────────────────────────
//
// **AI 凭据的唯一真相源**（2026-09-15 改）。设置面板保存的供应商/地址/key/模型落在这里，
// 启动时由 `commands::apply_saved_ai_config()` 读回并注入环境变量。
//
// 为什么不继续用 localStorage：它曾与 `.env` **争话语权** —— 启动时前端把 localStorage
// 里的旧值回灌进 env，**覆盖**了用户刚改过的 `.env`，表现为「key 改了不生效、一直 401」，
// 而且用户完全无从判断生效的是哪一份；排查时还要去翻 WebView2 的 leveldb 才能看见。
// 另外它不在 exe 根目录（违反便携约束），dev 与 release 还各存一份、互不相同。
//
// 放在 `config\` 与 hotkey.json 同级：都是「应用配置」，业务数据才进 ModuleData。
// 想回到 `.env` 的默认值：删掉本文件即可（启动日志会写明当前生效来源）。

/// 设置面板保存过的 AI 配置。字段级 `#[serde(default)]`：旧文件或手工编辑缺字段也能读。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct AiConfig {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub agent_url: String,
    #[serde(default)]
    pub search_provider: String,
    #[serde(default)]
    pub search_key: String,
    /// 当前模型是否支持图片输入（A8，2026-09-20）。**默认关** —— 发给不支持视觉的
    /// 端点（如 DeepSeek 官方端点）会 400，所以由用户显式打开；开着时前端才会把
    /// 图片附件作为 `image` 块发出去（见 ai-spec §3.5「图片附件」/ §11 规则 60）。
    #[serde(default)]
    pub vision: bool,
}

fn ai_config_path() -> PathBuf {
    lunac_root_dir().join("config").join("ai.json")
}

/// 读取 AI 配置；文件不存在或损坏时返回 `None`（调用方据此回落到 `.env`）。
pub fn load_ai_config() -> Option<AiConfig> {
    let text = fs::read_to_string(ai_config_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_ai_config(cfg: &AiConfig) -> Result<(), String> {
    let path = ai_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())
}

// ── 权限 hooks（A9，2026-09-20）───────────────────────────────────
//
// 用户脚本的配置文件放在 `<exe 根>\config\hooks.json`，由 **agent 侧**读取
// （core-agent 经 `LUNAC_HOOKS_FILE` 拿到路径，见 ai-spec §3.5「权限 hooks」）。
// 宿主这里只做三件事：定位路径、给设置面板读状态（存在 / 启用 / 语法是否合法）、
// 缺文件时落一份骨架并在编辑器里打开。
//
// **开关口径**：`enabled` 字段缺省为 `true`（文件存在本身就表示用户配了 hooks，
// 与 Claude Code 一致）；设置面板的开关写的就是这个字段。文件不存在 = 没配 = 关。

pub fn hooks_config_path() -> PathBuf {
    lunac_root_dir().join("config").join("hooks.json")
}

/// 读 hooks.json 的原文；文件不存在返回 `Ok(None)`。
pub fn load_hooks_text() -> Result<Option<String>, String> {
    let path = hooks_config_path();
    match fs::read_to_string(&path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读不出 {}：{e}", path.display())),
    }
}

pub fn save_hooks_text(text: &str) -> Result<(), String> {
    let path = hooks_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&path, text).map_err(|e| format!("写不进 {}：{e}", path.display()))
}

/// 缺文件时落一份**最小骨架**（`enabled: true` + 空的 hooks 表）。
///
/// 刻意**不预置示例脚本**：示例若是真命令，用户一开开关就会每次工具调用都跑一个
/// 注定失败的进程（前端还满屏 error 提示）。格式说明交给设置面板的提示行与
/// ai-spec §3.5 —— 那里有完整契约与可直接粘贴的样例。
pub fn ensure_hooks_file() -> Result<PathBuf, String> {
    let path = hooks_config_path();
    if !path.exists() {
        save_hooks_text("{\n  \"enabled\": true,\n  \"hooks\": {}\n}\n")?;
    }
    Ok(path)
}

// ── 人格 / 自定义提示词（`config\persona.md`，L2）────────────────────
//
// 一段**纯文本**（Markdown 写法即可），用户自己在设置面板里编辑。宿主只做三件事：
// 定位路径、存取原文、长度校验 —— **不解析、不加工**。
//
// 为什么是「文件 + 启动时读一次」而不是 hooks 或热更新（决策与实测见 ai-spec §3.5
// 「人格 / 自定义提示词」）：这段文本进的是**系统提示词的固定前缀**（每次请求都要发），
// 所以它必须与请求内容无关、进程内逐字节不变 —— 热读会让前缀每轮都变、把端点侧缓存整段
// 打掉（§11 规则 18/23）。代价是「保存后要重启 agent 才生效」，这一点在面板上如实写明。

/// 人格文本上限（字符）。**与 core-agent 的 `MAX_PERSONA_CHARS` 同值**，改一处要改两处。
///
/// 为什么要有上限：这段进的是**每次请求都要发的固定前缀**，塞一篇长文等于给每一轮都加一笔
/// 固定成本，而它并不随任务变化。宿主这里是**硬拒绝**（用户当场知道），agent 侧另有截断兜底
/// （防用户绕过面板直接改文件）。
pub const MAX_PERSONA_CHARS: usize = 8_000;

pub fn persona_config_path() -> PathBuf {
    lunac_root_dir().join("config").join("persona.md")
}

/// 读原文；文件不存在返回 `Ok(None)`（= 用户没配过，agent 用内置人格）。
pub fn load_persona_text() -> Result<Option<String>, String> {
    let path = persona_config_path();
    match fs::read_to_string(&path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读不出 {}：{e}", path.display())),
    }
}

/// **先校验后写**：不合格时一个字节都不落盘（与 A12 定价表同一条纪律）。
pub fn save_persona_text(text: &str) -> Result<(), String> {
    validate_persona_text(text)?;
    let path = persona_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&path, text).map_err(|e| format!("写不进 {}：{e}", path.display()))
}

/// 校验只挡两件会真出事的事：① 超长（固定前缀每轮都发）；② NUL（会破坏文本工具链）。
///
/// 刻意**不做内容审查**：这是**用户自己的**提示词，写什么由用户负责 —— 与 `hooks.json`
/// 同口径（那里也是用户脚本，宿主不看内容）。空文本是**合法**输入：语义是「恢复内置人格」。
pub fn validate_persona_text(text: &str) -> Result<(), String> {
    let n = text.chars().count();
    if n > MAX_PERSONA_CHARS {
        return Err(format!(
            "人格文本过长：{n} 字符，上限 {MAX_PERSONA_CHARS}（它进的是每次请求都要发的固定前缀）"
        ));
    }
    if text.contains('\0') {
        return Err("人格文本不能含空字符（NUL）".into());
    }
    Ok(())
}

// ── 定价表（`config\pricing.json`，A12）────────────────────────────
//
// 成本面板要显示「花了多少钱」，但**价格不能写进代码**：各家单价差十倍以上、官方
// 还会调价，写死一个数字等于把错误金额当事实展示（用户拿它对账时会得出错误结论）。
// 所以价格是**用户可编辑的配置文件**；面板上的「更新价格」按钮由 agent 去抓官方
// 定价页来填 —— 但结果**只在面板上预览（旧值 → 新值），用户点确认才落盘**。
//
// **单位：元 / 百万 token**，四类分别计价，字段名与 usage 日志的四类 token 一一对应：
//   `input`       未命中缓存的输入（miss）
//   `cache_read`  命中缓存的输入（hit）
//   `cache_write` 缓存写入
//   `output`      输出
// 每条另带 `source_url` / `updated_at`，面板上显示「这个数字是从哪来的、什么时候取的」
// —— 价格是会过期的数据，必须能回答「它是哪来的」。

pub fn pricing_config_path() -> PathBuf {
    lunac_root_dir().join("config").join("pricing.json")
}

/// 读 pricing.json 的原文；文件不存在返回 `Ok(None)`。
pub fn load_pricing_text() -> Result<Option<String>, String> {
    let path = pricing_config_path();
    match fs::read_to_string(&path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读不出 {}：{e}", path.display())),
    }
}

pub fn save_pricing_text(text: &str) -> Result<(), String> {
    let path = pricing_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&path, text).map_err(|e| format!("写不进 {}：{e}", path.display()))
}

/// 缺文件时落一份**空骨架**（`models` 为空表）。
///
/// 刻意**不预置任何价格数字**：本仓没有逐项核对过各家官方定价页，凭空填一行
/// 「看起来很像」的数会被用户当成事实拿去对账 —— 那比留空更糟。空表时面板显示
/// 「未设置价格」并引导点「更新价格」；填过价格的模型才参与金额计算。
pub fn ensure_pricing_file() -> Result<PathBuf, String> {
    let path = pricing_config_path();
    if !path.exists() {
        save_pricing_text("{\n  \"updated_at\": \"\",\n  \"models\": {}\n}\n")?;
    }
    Ok(path)
}

/// 候选价格文件：agent 抓完官方定价页只能写这里，**不能直接改 `pricing.json`** ——
/// 用户在面板上看完「旧值 → 新值」并点确认之后，宿主才覆盖正式文件（A12 的预览确认）。
///
/// **为什么落在 agent 的工作目录**（而不是 `config\`）：配了工作区时 agent 的文件工具
/// 被**硬锁**在工作区内（core-agent `tools::guard()`，越界直接拒绝、连审批卡都没有），
/// 写 `config\` 必然失败；放在工作目录里则无论锁不锁都写得进去。文件名固定且醒目，
/// 确认或放弃后立即删除。
pub fn pricing_pending_path(workdir: &Path) -> PathBuf {
    workdir.join("lunac-pricing.pending.json")
}

/// 读候选价格原文；文件不存在返回 `Ok(None)`。
pub fn load_pricing_pending_text(workdir: &Path) -> Result<Option<String>, String> {
    let path = pricing_pending_path(workdir);
    match fs::read_to_string(&path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读不出 {}：{e}", path.display())),
    }
}

/// 删除候选价格文件（不存在视为已删除）。
pub fn clear_pricing_pending(workdir: &Path) -> Result<(), String> {
    let path = pricing_pending_path(workdir);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删不掉 {}：{e}", path.display())),
    }
}

/// 校验一份定价表文本 —— **用户手写的文件与 agent 抓来的候选走同一处**。
///
/// 底线只有三条：顶层是对象 / `models` 是对象 / 每个模型的四类价格都存在且是**非负数字**。
/// 之所以连「缺字段」也算错：少一个字段在面板上的表现是「这个模型的金额悄悄少算一块」，
/// 比当场判非法难查得多。未知字段一律忽略（用户想加注释字段随他）。
pub fn validate_pricing_text(text: &str) -> Result<(), String> {
    let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("语法错误：{e}"))?;
    let obj = v.as_object().ok_or("顶层必须是一个对象")?;
    let models = match obj.get("models") {
        // 允许「只有 updated_at、还没有任何价格」的过渡态（骨架就是这样）
        None => return Ok(()),
        Some(m) => m.as_object().ok_or("`models` 必须是一个对象")?,
    };
    for (name, entry) in models {
        let entry = entry
            .as_object()
            .ok_or_else(|| format!("模型 `{name}` 的值必须是一个对象"))?;
        for field in ["input", "cache_read", "cache_write", "output"] {
            let num = entry
                .get(field)
                .ok_or_else(|| format!("模型 `{name}` 缺少 `{field}`"))?
                .as_f64()
                .ok_or_else(|| format!("模型 `{name}` 的 `{field}` 必须是数字"))?;
            if !num.is_finite() || num < 0.0 {
                return Err(format!("模型 `{name}` 的 `{field}` 必须是非负数字"));
            }
        }
    }
    Ok(())
}

/// 把候选价格落成正式的 `pricing.json`：**先校验、后覆盖**，成功后删掉候选文件。
///
/// `today` 由前端给（`YYYY-MM-DD`，本地日期，与用量日志同一套口径 —— Rust 侧没有
/// chrono）。顶层 `updated_at` 由宿主在落盘这一刻盖上：它表示「这份文件什么时候写进去的」，
/// **不是**「官方什么时候调的价」；后者只能靠每个模型的 `source_url` 让用户自己核。
pub fn commit_pricing_pending(workdir: &Path, today: &str) -> Result<PathBuf, String> {
    let pending = pricing_pending_path(workdir);
    let text = fs::read_to_string(&pending)
        .map_err(|e| format!("读不出待确认价格 {}：{e}", pending.display()))?;
    // 校验不过就**一个字都不写**：半坏的定价表会让面板显示错误的金额
    validate_pricing_text(&text).map_err(|e| format!("待确认价格不合法（原文件未改动）：{e}"))?;
    validate_iso_date(today)?;
    let mut v: serde_json::Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    v["updated_at"] = serde_json::json!(today);
    let out = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    save_pricing_text(&format!("{out}\n"))?;
    clear_pricing_pending(workdir)?;
    Ok(pricing_config_path())
}


// ── 旧数据整体迁移（%LOCALAPPDATA%\Lunac(-dev) → exe 根）──────────────
// 迁移后会删除旧目录（用户决策）。幂等：仅当目标 ModuleData 尚未存在时才复制；
// 若已存在则直接清理旧目录，避免每次启动重复搬移。

fn legacy_localappdata_candidates() -> Vec<PathBuf> {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    if local.is_empty() {
        return Vec::new();
    }
    // 旧 release/默认目录在前，debug 专用目录在后（历史顺序上两者都可能存在）
    vec![
        PathBuf::from(&local).join("Lunac"),
        PathBuf::from(&local).join("Lunac-dev"),
    ]
}

/// 递归复制目录内容（跳过无法读取的文件，迁移为 best-effort）。
fn copy_dir_recursive(src: &Path, dst: &Path) {
    let entries = match fs::read_dir(src) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            let _ = fs::create_dir_all(&to);
            copy_dir_recursive(&from, &to);
        } else {
            let _ = fs::copy(&from, &to);
        }
    }
}

/// 首次启动：把 %LOCALAPPDATA%\Lunac(-dev) 下旧数据整体搬入 exe 根，
/// 成功后删除旧目录（双清理的一部分，旧版本遗留由此顺带清除）。
pub fn migrate_legacy_localappdata() {
    let target = lunac_root_dir();
    const SUBS: [&str; 6] = [
        "ModuleData",
        "skills",
        "tools",
        "config",
        "paddle-ocr",
        "temp/webview-data",
    ];
    for legacy in legacy_localappdata_candidates() {
        if !legacy.exists() || legacy == target {
            continue;
        }
        // 目标尚未有数据 → 搬移；已有则视为已迁移
        if !target.join("ModuleData").exists() {
            for sub in SUBS {
                let src = legacy.join(sub);
                if src.exists() {
                    let dst = target.join(sub);
                    if let Some(parent) = dst.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    copy_dir_recursive(&src, &dst);
                }
            }
        }
        // 仅当新根数据已就位（ModuleData 已存在）或旧根已空时才删除旧目录，
        // 避免目标目录不可写（如安装在只读位置）时误删仍有效的数据。
        let copied = target.join("ModuleData").exists();
        let legacy_now_empty = fs::read_dir(&legacy)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false);
        if copied || legacy_now_empty {
            let _ = fs::remove_dir_all(&legacy);
        }
    }
}

fn data_dir() -> PathBuf {
    lunac_root_dir()
}

/// History subdirectory: <exe_dir>\ModuleData\history
fn history_dir() -> PathBuf {
    module_data_dir().join("history")
}

fn ensure_history_dir() -> std::io::Result<()> {
    fs::create_dir_all(history_dir())
}

/// 旧版目录（迁移前）：<exe_dir>\history（极早期布局，兼容用）
fn legacy_history_dir() -> PathBuf {
    data_dir().join("history")
}

/// 读取文件，若新位置不存在则尝试从旧 history 目录迁移一次。
fn read_with_legacy_migration(file: &str) -> Result<Option<String>, String> {
    let new_path = history_dir().join(file);
    if new_path.exists() {
        let json = fs::read_to_string(&new_path).map_err(|e| e.to_string())?;
        return Ok(Some(json));
    }
    let legacy_path = legacy_history_dir().join(file);
    if legacy_path.exists() {
        let json = fs::read_to_string(&legacy_path).map_err(|e| e.to_string())?;
        // 迁移：写入新位置（旧文件保留，由用户/清理策略决定）
        fs::create_dir_all(history_dir()).map_err(|e| e.to_string())?;
        fs::write(&new_path, &json).map_err(|e| e.to_string())?;
        return Ok(Some(json));
    }
    Ok(None)
}

// ── Chat History ──────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// 本次对话的 token 用量（**表盘口径**）。
/// 随会话一起落盘，历史回顾时前端读回来还原表盘（旧记录没有该字段 → 全 0）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct SessionUsage {
    #[serde(default)]
    pub hit: u64,
    #[serde(default)]
    pub miss: u64,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub elided: u64,
    #[serde(default)]
    pub dropped: u64,
}

/// 过程快照里的一步：一条思考 / 一次工具调用（含结果摘要）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct SessionStep {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(rename = "isError", default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    /// `Write` / `Edit` 改动的文件绝对路径（2026-09-17，backlog §8.1 路径追踪）。
    /// **只从 `tool_use` 的入参取**，绝不从工具输出正文里猜 —— 这是「改动过的文件」与
    /// 「只是读过的文件」唯一可靠的区分。恢复历史时据此重建「本次会话改动过的文件」列表
    /// （落盘走 `steps` 那一列的 JSON，**无需改表结构**）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// 一个回合的过程（历史回顾时按回合渲染可折叠的「过程」块）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct SessionProcess {
    #[serde(default)]
    pub turn: u64,
    #[serde(default)]
    pub items: Vec<SessionStep>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatSession {
    pub id: String,
    pub title: String,
    pub messages: Vec<ChatMessage>,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    /// token 用量（表盘口径）——历史回顾时前端读回表盘
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<SessionUsage>,
    /// 过程快照（按回合分组）——历史回顾时渲染「查看过程」
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<SessionProcess>>,
}

/// 旧会话历史文件（JSON）。**只用于一次性迁移**：库建好之后它不再是真相源。
const CHAT_FILE: &str = "chat-history.json";
/// 会话历史库（SQLite，取代上面的 JSON —— 见 chat_db.rs 的模块注释）。
const CHAT_DB: &str = "chat.db";

/// `<ModuleData>\history\chat.db`
///
/// `pub(crate)`：MCP server（`mcp_server.rs`）要以 `--mcp-server` 子进程身份读同一个库
/// 来回答 agent 的「往期会话检索」（2026-09-19，A2）—— 检索 SQL 归 `chat_db.rs` 独有，
/// 这里只交出路径，两个进程共用同一份 schema 真相。
pub(crate) fn chat_db_path() -> PathBuf {
    history_dir().join(CHAT_DB)
}

#[tauri::command]
pub fn save_chat_sessions(sessions: Vec<ChatSession>) -> Result<(), String> {
    ensure_history_dir().map_err(|e| e.to_string())?;
    crate::chat_db::save(&chat_db_path(), &sessions)
}

#[tauri::command]
pub fn load_chat_sessions() -> Result<Vec<ChatSession>, String> {
    ensure_history_dir().map_err(|e| e.to_string())?;
    let db = chat_db_path();
    if !db.exists() {
        // 一次性迁移（2026-09-17）：老 JSON 会话历史 → SQLite。
        // **只在库文件不存在时走** —— 库一旦建好就是唯一真相源，绝不能再拿旧 JSON 覆盖它
        // （否则用户删掉的会话会在下次启动时"复活"）。
        // 迁移完**保留**旧 JSON 不删：留着只是几十 KB，删了就没有退路。
        if let Ok(Some(json)) = read_with_legacy_migration(CHAT_FILE) {
            match crate::chat_db::import_legacy(&db, &json) {
                Ok(n) => eprintln!("[storage] 会话历史已迁移到 SQLite：{n} 个会话"),
                // 迁移失败不阻断启动：库会建为空库，旧 JSON 原样留着可手工抢救。
                Err(e) => eprintln!("[storage] 会话历史迁移失败（旧 JSON 保留）：{e}"),
            }
        }
    }
    crate::chat_db::load(&db)
}

// ── Clipboard History ─────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClipEntry {
    /// "text" for plain text entries, "file" for file path entries
    #[serde(default = "default_clip_type")]
    pub clip_type: String,
    /// Plain text (for text-type entries; may also hold inline text for file-type)
    #[serde(default)]
    pub text: String,
    /// File paths (for file-type entries)
    #[serde(default)]
    pub file_paths: Vec<String>,
    pub time: u64,
}

fn default_clip_type() -> String { "text".to_string() }

const CLIP_FILE: &str = "clipboard-history.json";

#[tauri::command]
pub fn save_clipboard_history(entries: Vec<ClipEntry>) -> Result<(), String> {
    ensure_history_dir().map_err(|e| e.to_string())?;
    let path = history_dir().join(CLIP_FILE);
    let json = serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn load_clipboard_history() -> Result<Vec<ClipEntry>, String> {
    match read_with_legacy_migration(CLIP_FILE)? {
        Some(json) => serde_json::from_str(&json).map_err(|e| e.to_string()),
        None => Ok(vec![]),
    }
}

// ── 用量日志（ModuleData\usage\usage-YYYY-MM-DD.jsonl，与平台对账用）────
//
// 每次用户提问一行。**口径**：一行 = 一次提问的合计（含提问内所有工具往返），
// 而供应商平台按「每次 API 请求」记行 —— 一次带工具的提问在平台上就是多行，
// 对账时把同一时间窗的平台各行相加。
//
// 只追加不重写：文件天然按天分片、可被任何工具解析（jq/脚本），且不必担心
// 并发写坏。文件名用**本地日期**（由前端传入）：Rust 侧没有 chrono，不为一句
// 时区换算引入新依赖。

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UsageRequest {
    /// 该次请求未命中缓存的输入 token（Anthropic 的 `input_tokens`）
    #[serde(rename = "in")]
    pub input: u64,
    /// 命中缓存的输入 token（`cache_read_input_tokens`）
    #[serde(default)]
    pub read: u64,
    /// 缓存写入 token（`cache_creation_input_tokens`；DeepSeek 自动缓存下恒为 0）
    #[serde(default)]
    pub create: u64,
    /// 该次请求的输出 token
    #[serde(default)]
    pub out: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UsageRecord {
    /// 本地时钟的 epoch 毫秒（前端 Date.now()）
    pub ts: u64,
    /// 产生这条记录的模型名（来自 agent 的 system/init）
    pub model: String,
    /// 产生这条记录的 **agent 会话 id**（A11；来自 agent `system/init` 的 `session_id`）。
    /// 语义是「**一次 agent 运行**」而非「一段对话」：换模型 / 换思考档 / 换工作区 /
    /// 回退取消流式都会重启 agent 进程，也就换一个新 id。
    /// 唯一用途是**归因** —— 让这条用量记录、stdout 的消息、agent 落盘日志三者能对上
    /// 「哪些东西属于同一次运行」；对账口径本身仍按 `ts` + `model` 走。
    /// 旧记录没有该字段，读时按空串（`serde(default)`），空值也不写回日志。
    #[serde(rename = "sessionId", default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    pub input: u64,
    pub output: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    #[serde(rename = "cacheCreate")]
    pub cache_create: u64,
    /// 本次提问内 agent 报告的历史压缩次数（瘦身 tool_result / 丢弃旧消息）。
    /// 压缩会改写请求前缀 → 端点侧缓存作废，是命中率的**断裂型**失效来源，
    /// 与「新内容天生没被上一轮缓存覆盖」的自然未命中分开统计用。
    /// 旧记录没有这两个字段，读时按 0（`serde(default)`）。
    #[serde(default)]
    pub elided: u64,
    #[serde(default)]
    pub dropped: u64,
    /// **每次 API 请求**一行的用量明细（顺序 = 请求顺序）。
    /// 平台上「一次带工具的提问」就是多行、本地只落一行 → 命中率没法逐行对齐；
    /// 有了这个数组才能和 DeepSeek 平台用量页按请求对账（ai-spec §3.5「用量与对账」）。
    /// 旧记录没有该字段，读时按空表。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<UsageRequest>,
}

fn usage_dir() -> PathBuf {
    module_data_dir().join("usage")
}

/// 只接受严格的 `YYYY-MM-DD`（日期一律来自前端，必须挡住路径拼串）
fn validate_iso_date(date: &str) -> Result<(), String> {
    let b = date.as_bytes();
    let ok = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() });
    if !ok {
        return Err(format!("invalid date: {date}"));
    }
    Ok(())
}

/// 只接受严格的 `YYYY-MM-DD` —— 文件名来自前端，必须挡住路径拼串
fn usage_log_path(date: &str) -> Result<PathBuf, String> {
    validate_iso_date(date)?;
    Ok(usage_dir().join(format!("usage-{date}.jsonl")))
}

#[tauri::command]
pub fn append_usage_log(date: String, record: UsageRecord) -> Result<(), String> {
    let path = usage_log_path(&date)?;
    fs::create_dir_all(usage_dir()).map_err(|e| e.to_string())?;
    let line = serde_json::to_string(&record).map_err(|e| e.to_string())?;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(f, "{line}").map_err(|e| e.to_string())
}

/// 读取某天的全部记录；文件不存在返回空表。单行损坏只跳过该行（半个写入
/// 的文件不该让整个面板失效）。
#[tauri::command]
pub fn read_usage_log(date: String) -> Result<Vec<UsageRecord>, String> {
    let path = usage_log_path(&date)?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    Ok(text
        .lines()
        .filter_map(|l| serde_json::from_str::<UsageRecord>(l).ok())
        .collect())
}

// ── 按天用量汇总（成本面板用，A12）──────────────────────────────
//
// 金额 = 各模型四类 token × 各自单价，所以汇总**必须分模型**：一天里换过模型的话，
// 只按天合计就把两个模型的量混在一起了（单价还差十倍），算出来的钱没有意义。

/// 某个模型在一天里的合计（决定用哪一档单价）
#[derive(Debug, Serialize, Clone, Default)]
pub struct UsageModelTotals {
    pub model: String,
    /// 提问次数（一行日志 = 一次提问，含提问内所有工具往返）
    pub turns: u64,
    pub input: u64,
    pub output: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    #[serde(rename = "cacheCreate")]
    pub cache_create: u64,
}

/// 一天的合计（面板「按天表格」的一行）
#[derive(Debug, Serialize, Clone, Default)]
pub struct UsageDay {
    pub date: String,
    pub turns: u64,
    pub input: u64,
    pub output: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    #[serde(rename = "cacheCreate")]
    pub cache_create: u64,
    /// 这一天用到的模型（按模型名升序）；金额按这里的每一项分别计价
    pub models: Vec<UsageModelTotals>,
}

/// 读**多天**用量并汇总成「按天 + 按模型」的形状（A12 成本面板）。
///
/// 日期一律由前端给（与 `append_usage_log` 同一套：Rust 侧没有 chrono），**升序**返回；
/// 没有任何记录的天不返回（面板不显示全零行）。汇总放在宿主而不是让前端逐天 IPC：
/// 30 天就是 30 次跨进程调用，这里一次读完。
#[tauri::command]
pub fn read_usage_range(dates: Vec<String>) -> Result<Vec<UsageDay>, String> {
    let mut days = Vec::new();
    for date in dates {
        let records = read_usage_log(date.clone())?;
        if records.is_empty() {
            continue;
        }
        let mut day = UsageDay {
            date,
            ..Default::default()
        };
        let mut by_model: std::collections::BTreeMap<String, UsageModelTotals> =
            std::collections::BTreeMap::new();
        for r in records {
            day.turns += 1;
            day.input += r.input;
            day.output += r.output;
            day.cache_read += r.cache_read;
            day.cache_create += r.cache_create;
            let m = by_model.entry(r.model.clone()).or_insert_with(|| UsageModelTotals {
                model: r.model.clone(),
                ..Default::default()
            });
            m.turns += 1;
            m.input += r.input;
            m.output += r.output;
            m.cache_read += r.cache_read;
            m.cache_create += r.cache_create;
        }
        day.models = by_model.into_values().collect();
        days.push(day);
    }
    Ok(days)
}

// ── 备忘录（ModuleData\memo\memo.json）──────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MemoEntry {
    pub id: String,
    pub text: String,
    /// 用户自定义检索标识（如 #项目A / travel-plan），可为空
    #[serde(default)]
    pub tag: String,
    /// 随备忘录保存的图片绝对路径（2026-09）
    #[serde(default)]
    pub images: Vec<String>,
    pub ts: u64,
}

fn memo_dir() -> PathBuf {
    module_data_dir().join("memo")
}

fn memo_images_dir(id: &str) -> PathBuf {
    memo_dir().join("images").join(id)
}

fn memo_file() -> PathBuf {
    memo_dir().join("memo.json")
}

#[tauri::command]
pub fn memo_save_entries(entries: Vec<MemoEntry>) -> Result<(), String> {
    fs::create_dir_all(memo_dir()).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?;
    fs::write(memo_file(), json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn memo_load_entries() -> Result<Vec<MemoEntry>, String> {
    let path = memo_file();
    if !path.exists() {
        return Ok(vec![]);
    }
    let json = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&json).map_err(|e| e.to_string())
}

/// 将备忘录粘贴的图片（dataURL）写入 ModuleData\memo\images\<id>\<index>.png|jpg，
/// 返回该图片的绝对路径（前端用 convertFileSrc 显像）。
#[tauri::command]
pub fn memo_save_image(id: String, index: usize, data_url: String) -> Result<String, String> {
    let mime = data_url
        .split("data:")
        .nth(1)
        .and_then(|s| s.split(';').next())
        .and_then(|s| s.split(',').next())
        .unwrap_or("image/png");
    let ext = match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/bmp" => "bmp",
        "image/webp" | "image/gif" => "png",
        "image/tiff" | "image/tif" => "tiff",
        _ => "png",
    };
    let b64 = data_url
        .split(',')
        .nth(1)
        .ok_or("Invalid data URL format")?;
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        b64,
    )
    .map_err(|e| format!("Base64 decode failed: {e}"))?;

    let dir = memo_images_dir(&id);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("{}.{}", index, ext));
    fs::write(&file, &bytes).map_err(|e| format!("Write memo image failed: {e}"))?;
    Ok(file.to_string_lossy().to_string())
}

// ── 计划文档（ModuleData\plans\<时间戳>.md，2026-09-20 A7）───────────
//
// 计划模式里**被用户批准**的那份计划正文留档。为什么落盘：
//   · 它是「用户看过并点了批准」的产物 —— 对话滚过去之后还得能回看 / 照着执行；
//   · 与 tool-outputs 那种「过程垃圾」不同，这份东西的价值不随时间衰减。
// 文件名的本地时间戳由**前端**给（同 append_usage_log）：Rust 侧没有 chrono，
// 不为一句时区换算引入新依赖（见 log.rs 的说明）。

/// 计划文档目录：<exe_dir>\ModuleData\plans
pub fn plans_dir() -> PathBuf {
    module_data_dir().join("plans")
}

/// 只接受严格的 `YYYY-MM-DD_HHMMSS` —— 文件名来自前端，必须挡住路径拼串
/// （`..` / 分隔符 / 绝对路径都进不来）。
fn plan_path(stamp: &str) -> Result<PathBuf, String> {
    let b = stamp.as_bytes();
    let shape_ok = b.len() == 17 && b[4] == b'-' && b[7] == b'-' && b[10] == b'_';
    let digits_ok = shape_ok
        && b.iter()
            .enumerate()
            .all(|(i, c)| matches!(i, 4 | 7 | 10) || c.is_ascii_digit());
    if !digits_ok {
        return Err(format!("invalid timestamp: {stamp}"));
    }
    Ok(plans_dir().join(format!("{stamp}.md")))
}

/// 保存一份被批准的计划（A7）。返回落盘的绝对路径，前端用它给用户一句「已存到 …」。
#[tauri::command]
pub fn save_plan_md(stamp: String, plan: String) -> Result<String, String> {
    if plan.trim().is_empty() {
        return Err("plan is empty".into());
    }
    let path = plan_path(&stamp)?;
    fs::create_dir_all(plans_dir()).map_err(|e| e.to_string())?;
    fs::write(&path, &plan).map_err(|e| format!("Write error: {}", e))?;
    Ok(path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(cache_read: u64) -> UsageRecord {
        UsageRecord {
            ts: 1_700_000_000_000,
            model: "deepseek-flash".into(),
            session_id: "sess_1_1700000000000".into(),
            input: 212,
            output: 3,
            cache_read,
            cache_create: 0,
            elided: 0,
            dropped: 0,
            requests: vec![],
        }
    }

    /// 文件名来自前端 —— 必须挡住跨目录拼串与非 `YYYY-MM-DD` 形态
    #[test]
    fn usage_log_path_only_accepts_iso_date() {
        for bad in ["", "2026-9-1", "2026/09/12", "../2026-09-12", "2026-09-12x", "2026-09-1"] {
            assert!(usage_log_path(bad).is_err(), "should reject `{bad}`");
        }
        assert!(usage_log_path("2026-09-12").is_ok());
        assert!(usage_log_path("1970-01-01").is_ok());
    }

    /// 计划文件名同样来自前端（本地时间由它算）—— 必须挡住跨目录拼串、
    /// 以及「只有日期没有时刻」这种会让同一天多份计划互相覆盖的形态。
    #[test]
    fn plan_path_only_accepts_a_local_timestamp() {
        for bad in [
            "",
            "2026-09-20",              // 缺时刻 ⇒ 同一天批准两次会互相覆盖
            "2026-09-20_14301",        // 秒不足两位
            "2026-09-20_143012.md",    // 扩展名由后端拼，前端不许带
            "2026-09-20 143012",       // 空格（前端顺手用 toLocaleString 就会长这样）
            "20260920_143012",
            "../escape",
            "2026-09-20_143012/../x",
        ] {
            assert!(plan_path(bad).is_err(), "should reject `{bad}`");
        }
        let p = plan_path("2026-09-20_143012").unwrap();
        assert_eq!(p.file_name().unwrap(), "2026-09-20_143012.md");
        assert!(p.parent().unwrap().ends_with("plans"), "落点必须是 plans\\ 下");
    }

    /// 字段名是前端 `read_usage_log` 的消费契约（cacheRead/cacheCreate 驼峰；
    /// `requests` 为空时不写该键）。
    #[test]
    fn usage_record_json_shape() {
        let line = serde_json::to_string(&rec(1536)).unwrap();
        assert_eq!(
            line,
            r#"{"ts":1700000000000,"model":"deepseek-flash","sessionId":"sess_1_1700000000000","input":212,"output":3,"cacheRead":1536,"cacheCreate":0,"elided":0,"dropped":0}"#
        );
    }

    /// A11：会话 id 的**向后兼容** —— 旧日志行没有该键，读时按空串；
    /// 空值也不写回（没有会话 id 时不给日志添噪音）。
    #[test]
    fn usage_record_session_id_is_backward_compatible() {
        let old = r#"{"ts":1,"model":"m","input":1,"output":1,"cacheRead":0,"cacheCreate":0}"#;
        assert_eq!(serde_json::from_str::<UsageRecord>(old).unwrap().session_id, "");
        let mut r = rec(0);
        r.session_id = String::new();
        assert!(!serde_json::to_string(&r).unwrap().contains("sessionId"));
    }

    /// 每次 API 请求一行（对账粒度）—— `in` 是关键字，必须映射成 `in` 而不是 `input`
    #[test]
    fn usage_record_serializes_per_request_rows() {
        let mut r = rec(1536);
        r.requests = vec![
            UsageRequest { input: 700, read: 0, create: 0, out: 12 },
            UsageRequest { input: 20, read: 680, create: 0, out: 8 },
        ];
        let line = serde_json::to_string(&r).unwrap();
        assert!(line.contains(r#""requests":[{"in":700,"read":0,"create":0,"out":12}"#), "{line}");
        assert_eq!(
            serde_json::from_str::<UsageRecord>(&line).unwrap().requests[1].read,
            680
        );
    }

    #[test]
    fn usage_log_append_read_and_tolerate_broken_line() {
        let date = "1970-01-01"; // 固定的远古日期，不与真实用量混在一起
        let path = usage_log_path(date).unwrap();
        let _ = fs::remove_file(&path);

        append_usage_log(date.into(), rec(1536)).unwrap();
        append_usage_log(date.into(), rec(7000)).unwrap();
        let got = read_usage_log(date.into()).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].cache_read, 7000);

        // 半截写入不该让整个面板失效
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{{\"ts\":1,").unwrap();
        assert_eq!(read_usage_log(date.into()).unwrap().len(), 2);

        let _ = fs::remove_file(&path);
        assert!(read_usage_log("1970-01-02".into()).unwrap().is_empty());
    }

    /// 成本面板的汇总**必须分模型**：一天里换过模型的话，只按天合计就把两个模型
    /// 的量混在一起了（单价差十倍），算出来的钱没有意义（A12）。
    #[test]
    fn usage_range_groups_by_day_and_model() {
        let d1 = "1970-01-03";
        let d2 = "1970-01-04";
        for d in [d1, d2, "1970-01-05"] {
            let _ = fs::remove_file(usage_log_path(d).unwrap());
        }
        append_usage_log(d1.into(), rec(1536)).unwrap();
        let mut other = rec(0);
        other.model = "gpt-4o".into();
        other.input = 100;
        other.output = 7;
        append_usage_log(d1.into(), other).unwrap();
        append_usage_log(d2.into(), rec(7000)).unwrap();

        // 中间夹一天没记录 → 不返回（面板不显示全零行）
        let days = read_usage_range(vec![
            d1.into(),
            "1970-01-05".into(),
            d2.into(),
        ])
        .unwrap();
        assert_eq!(days.len(), 2, "空天必须被跳过：{days:?}");

        let day1 = &days[0];
        assert_eq!(day1.date, d1);
        assert_eq!(day1.turns, 2);
        assert_eq!(day1.input, 212 + 100);
        assert_eq!(day1.output, 3 + 7);
        assert_eq!(day1.cache_read, 1536);
        let models: Vec<&str> = day1.models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(models, vec!["deepseek-flash", "gpt-4o"], "按模型分组");
        assert_eq!(day1.models[1].input, 100);
        assert_eq!(day1.models[0].cache_read, 1536);

        assert_eq!(days[1].turns, 1);
        assert_eq!(days[1].cache_read, 7000);

        // 字段名是前端消费契约（驼峰）
        let json = serde_json::to_string(day1).unwrap();
        assert!(json.contains(r#""cacheRead":1536"#), "{json}");
        assert!(json.contains(r#""cacheCreate":0"#), "{json}");

        for d in [d1, d2] {
            let _ = fs::remove_file(usage_log_path(d).unwrap());
        }
    }

    /// 定价表与「候选 → 确认」的落盘纪律（A12）：
    /// ① 校验基线是「四类价格齐全且非负」；② 候选不合法时**一个字都不写**；
    /// ③ 确认时才覆盖正式文件，并盖上落盘日期、删掉候选。
    #[test]
    fn pricing_candidate_is_validated_before_commit() {
        let workdir = lunac_root_dir().join("temp").join("pricing-test");
        fs::create_dir_all(&workdir).unwrap();
        let pending = pricing_pending_path(&workdir);
        let official = pricing_config_path();

        // ① 基线：缺字段 / 非数字 / 负数 / 顶层不是对象 / 语法坏 一律判非法
        for bad in [
            "[]",
            "{",
            r#"{"models": []}"#,
            r#"{"models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0}}}"#,
            r#"{"models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": "8"}}}"#,
            r#"{"models": {"m": {"input": -1, "cache_read": 0, "cache_write": 0, "output": 8}}}"#,
        ] {
            assert!(validate_pricing_text(bad).is_err(), "should reject `{bad}`");
        }
        let good = r#"{"models": {"m": {"input": 2, "cache_read": 0.5, "cache_write": 0, "output": 8,
            "source_url": "https://example.com/pricing", "updated_at": "1970-01-01"}}}"#;
        assert!(validate_pricing_text(good).is_ok());
        // 空骨架（还没有任何价格）算合法
        assert!(validate_pricing_text("{\"updated_at\": \"\", \"models\": {}}").is_ok());

        // ② 语法坏的候选 → 拒绝，且正式文件**一字未改**
        save_pricing_text("{\n  \"updated_at\": \"1970-01-01\",\n  \"models\": {}\n}\n").unwrap();
        let before = load_pricing_text().unwrap();
        fs::write(&pending, "{ broken").unwrap();
        assert!(commit_pricing_pending(&workdir, "1970-01-01").is_err());
        assert_eq!(before, load_pricing_text().unwrap(), "拒绝时不该写正式文件");

        // ③ 合法候选 → 覆盖 + 盖日期 + 删候选
        fs::write(&pending, good).unwrap();
        let written = commit_pricing_pending(&workdir, "1970-01-02").unwrap();
        assert_eq!(written, official);
        assert!(!pending.exists(), "确认后必须删掉候选文件");
        let after = load_pricing_text().unwrap().unwrap();
        assert!(after.contains(r#""updated_at": "1970-01-02""#), "{after}");
        assert!(after.contains("https://example.com/pricing"), "{after}");
        assert!(validate_pricing_text(&after).is_ok());

        // 缺候选文件时也不该假装成功
        assert!(commit_pricing_pending(&workdir, "1970-01-02").is_err());

        let _ = fs::remove_file(&official);
        let _ = fs::remove_dir_all(&workdir);
    }

    /// 人格文本（L2）：**先校验后写**（拒绝时一个字节都不落盘）+ 往返一致 + 空文本合法。
    ///
    /// 空文本的语义是「恢复内置人格」，所以它必须能存（存成空文件），而不是被当成非法输入。
    /// 收尾会把测试前的原文件**原样还原**（这是真实用户文件，不能测完留垃圾）。
    #[test]
    fn persona_text_is_validated_before_save_and_round_trips() {
        let path = persona_config_path();
        let before = load_persona_text().unwrap();

        // ① 超长 / 含 NUL → 拒绝，且文件一字未改
        let too_long = "x".repeat(MAX_PERSONA_CHARS + 1);
        assert!(validate_persona_text(&too_long).is_err());
        assert!(save_persona_text(&too_long).is_err());
        assert!(validate_persona_text("a\0b").is_err());
        assert!(save_persona_text("a\0b").is_err());
        assert_eq!(load_persona_text().unwrap(), before, "拒绝时不该写文件");
        // 边界：刚好到上限是合法的
        assert!(validate_persona_text(&"x".repeat(MAX_PERSONA_CHARS)).is_ok());

        // ② 正常写入 → 读回逐字节一致（中文 / 花括号 / 换行都要原样保留）
        let text = "始终用中文回答。\n- 短句优先\n- 保留 {like this} 字面量";
        save_persona_text(text).unwrap();
        assert_eq!(load_persona_text().unwrap().as_deref(), Some(text));

        // ③ 空文本 = 恢复内置人格（合法，读回空串）
        save_persona_text("").unwrap();
        assert_eq!(load_persona_text().unwrap().as_deref(), Some(""));

        // 收尾：还原测试前的状态
        match before {
            Some(t) => fs::write(&path, t).unwrap(),
            None => {
                let _ = fs::remove_file(&path);
            }
        }
    }
}
