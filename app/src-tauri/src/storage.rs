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
    /// **出图模型名**（A13，2026-10-03）。空串 = 未配置 ⇒ agent 不注册 `ImageGen` 工具。
    ///
    /// 为什么与文本模型分开一个字段：Qwen-Image 系**不活在 OpenAI 兼容 chat 端点**上
    /// （`compatible-mode` 只服务文本），它单独走 DashScope 的多点编辑端点；把两个模型
    /// 塞进同一个 `model` 字段会让「对话用 qwen3-max、出图用 qwen-image-3.0」没法同时表达。
    #[serde(default)]
    pub image_model: String,
    /// **出图端点**。空串 = 用 DashScope 默认（见 agent 侧 `image.rs` 的 `DEFAULT_ENDPOINT`）。
    /// 单列字段是为了「换一家出图服务」不必改代码 —— 与文本侧的 `url` 同口径。
    #[serde(default)]
    pub image_url: String,
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

// ── 远端 MCP 服务器（`config\mcp.json`，2026-10-01）──────────────────
//
// 用户在 `<exe 根>\config\mcp.json` 里声明远端 MCP 服务器；**由 agent 侧读取**
// （core-agent 经 `LUNAC_MCP_FILE` 拿到路径，见 core-agent/src/mcp.rs）。
// 宿主这里只做三件事：定位路径、缺文件时落一份骨架、给设置面板读状态。
//
// **格式**（`headers` 可选，用来放 `Authorization: Bearer …` 这类静态凭据）：
//   { "servers": [ { "name": "notion", "url": "https://…", "headers": { "Authorization": "Bearer …" } } ] }
//
// **两道判据在 agent 侧**（这里不重复实现，免得两处漂移）：url 只接受 https
// （明文 http 仅限本机）、坏条目只丢自己。宿主这边**不解析内容**，只看「能不能读成对象」。

pub fn mcp_config_path() -> PathBuf {
    lunac_root_dir().join("config").join("mcp.json")
}

/// 读 mcp.json 的原文；文件不存在返回 `Ok(None)`。
pub fn load_mcp_text() -> Result<Option<String>, String> {
    let path = mcp_config_path();
    match fs::read_to_string(&path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读不出 {}：{e}", path.display())),
    }
}

/// 缺文件时落一份**最小骨架**（空的 servers 数组）—— 用户点「打开配置」时才有东西可编辑。
///
/// 刻意**不预置示例服务器**：一条真 URL 会被真的去连（握手失败要等 15 秒），
/// 而用户只是打开文件看了一眼。
pub fn ensure_mcp_file() -> Result<PathBuf, String> {
    let path = mcp_config_path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&path, "{\n  \"servers\": []\n}\n")
            .map_err(|e| format!("写不进 {}：{e}", path.display()))?;
    }
    Ok(path)
}

// ── 人格 / 自定义提示词（`config\persona.md`，L2）────────────────────
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

/// 缺文件时落的**预置定价表**（2026-09-29 用户批准打破规则 63 的「不预置任何价格数字」）。
///
/// **为什么现在可以预置**：这些数字不是从官方页抄来的「看着很像」的值 —— 它们是**两处
/// 独立证据逐项对上的**：① 官方定价页的峰谷表（美元/百万，峰 = 谷 × 2）；② 用户
/// 2026-09-29 的真实账单 CSV 反算出的单价（元/百万）。两者的结构完全一致
/// （命中 : 未命中 : 输出 = 0.02 : 1 : 4，峰档整体 ×2），折算汇率也自洽
/// （0.15 USD → 1 CNY、0.6 USD → 4 CNY，同为 6.67）。
///
/// **时段规则同样有出处**（官方价目表脚注）：高峰 = **周一至周五（不含中国法定节假日）**
/// 的北京时间 **09:00–12:00** 与 **14:00–18:00**，其余全部（含整个周末）为谷时，
/// 谷价 = 峰价一半。
///
/// **`holidays` 这条不能省（2026-10-06 补，实测）**：官方脚注原文是「周一至周五**（不含中国
/// 法定节假日）**……其余时段，包括周末及**中国法定节假日全天**均为空闲时段」。
/// 缺了它，节假日会被按峰价高估 —— 2026-10-02（国庆假期、周五）实测多算 **0.979 元**
/// （当天平台 7 个小时的金额全部等于谷价公式，逐分吻合）。表里落的是 2026 年国务院
/// 放假安排（国办发明电〔2025〕7 号）。
///
/// **只写一次**：`ensure_pricing_file` 仅在文件不存在时落这份；用户改过的一律不动
/// （**唯一例外**是「老文件缺 `holidays` 时补一次」，见该函数的注释）。
/// 预置的模型名是 **`deepseek-v4-flash`** —— 用户 `.env` 与实际用量日志里都是它（官方页
/// 写明旧名 `deepseek-v4-flash` 与 `deepseek-flash` 同模型同价）。**没有实测依据的模型不预置**
/// （如 pro：官方页只有美元价，折算汇率是本机假设 —— 宁可让面板显示「未定价」）。
///
/// 顶层 `updated_at` 这次填的是**这份价的核对日期**（不是文件写入时刻）—— 缺了它面板会显示
/// 「—」，而预置价表明明有据可查。任何一次「确认候选价」都会把它盖回落盘日期，语义复原。
const DEFAULT_PRICING_JSON: &str = r#"{
  "updated_at": "2026-09-29",
  "holidays": [
    "2026-01-01", "2026-01-02", "2026-01-03",
    "2026-02-15", "2026-02-16", "2026-02-17", "2026-02-18", "2026-02-19",
    "2026-02-20", "2026-02-21", "2026-02-22", "2026-02-23",
    "2026-04-04", "2026-04-05", "2026-04-06",
    "2026-05-01", "2026-05-02", "2026-05-03", "2026-05-04", "2026-05-05",
    "2026-06-19", "2026-06-20", "2026-06-21",
    "2026-09-25", "2026-09-26", "2026-09-27",
    "2026-10-01", "2026-10-02", "2026-10-03", "2026-10-04",
    "2026-10-05", "2026-10-06", "2026-10-07"
  ],
  "models": {
    "deepseek-v4-flash": {
      "input": 1,
      "cache_read": 0.02,
      "cache_write": 0,
      "output": 4,
      "source_url": "https://api-docs.deepseek.com/quick_start/pricing",
      "updated_at": "2026-09-29",
      "time_windows": [
        {
          "days": [1, 2, 3, 4, 5],
          "from": "09:00",
          "to": "12:00",
          "input": 2,
          "cache_read": 0.04,
          "cache_write": 0,
          "output": 8
        },
        {
          "days": [1, 2, 3, 4, 5],
          "from": "14:00",
          "to": "18:00",
          "input": 2,
          "cache_read": 0.04,
          "cache_write": 0,
          "output": 8
        }
      ]
    }
  }
}
"#;

/// 缺文件时落一份**预置骨架**（见 `DEFAULT_PRICING_JSON`）。
///
/// 只在**文件不存在**时写：用户改过的价格一个字都不覆盖。预置内容本身也要过
/// `validate_pricing_text`（守门单测 `default_pricing_json_is_valid`）—— 预置一份不合法的
/// 价表比不预置更糟：面板要么整块算不出金额，要么提示「候选价不合法」而用户没动过任何东西。
///
/// **唯一例外（2026-10-06）：老文件缺顶层 `holidays` 时补一次**。`holidays` 是后加的字段，
/// 早期装好的 `pricing.json` 里没有它 ⇒ 面板会在法定节假日按峰价高估（2026-10-02 国庆实测
/// 多算 0.979 元）。补法是**只加这一个键**（取 `DEFAULT_PRICING_JSON` 里那份，保持单一
/// 数据源），其余字段一个都不动；补完先过 `validate_pricing_text`，不合法就**不写**。
pub fn ensure_pricing_file() -> Result<PathBuf, String> {
    let path = pricing_config_path();
    if !path.exists() {
        save_pricing_text(DEFAULT_PRICING_JSON)?;
    } else {
        backfill_pricing_holidays(&path);
    }
    Ok(path)
}

/// 老定价表缺顶层 `holidays` 时补一次。**失败一律静默** —— 补不上不该让宿主起不来，
/// 面板会照旧按「没有节假日」算（旧行为）。
///
/// 判据是**键在不在**，不是「值空不空」：用户若故意写成 `"holidays": []`（明确声明无节假日），
/// 键已存在 ⇒ 不会再补。
fn backfill_pricing_holidays(path: &Path) {
    let Ok(text) = fs::read_to_string(path) else { return };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        return; // 坏 JSON 交给面板如实报错，别在这里猜
    };
    let Some(obj) = v.as_object_mut() else { return };
    if obj.contains_key("holidays") {
        return;
    }
    // 节假日清单**从预置表里取**，不在这里再抄一份（那是同一个值的第二份实现）
    let Some(holidays) = serde_json::from_str::<serde_json::Value>(DEFAULT_PRICING_JSON)
        .ok()
        .and_then(|d| d.get("holidays").cloned())
    else {
        return;
    };
    obj.insert("holidays".into(), holidays);
    let Ok(out) = serde_json::to_string_pretty(&v) else { return };
    // 写回前先校验：宁可不补，也不要留一份面板读不动的半坏表
    if validate_pricing_text(&out).is_err() {
        return;
    }
    let _ = save_pricing_text(&format!("{out}\n"));
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

/// 解析定价表里的 `HH:MM`（本地墙钟）为「零点起的分钟数」。
///
/// 只认**严格**的 `HH:MM`：写 `9:00` / `09:0` / `09:00:00` 都判非法 —— 宽松解析会悄悄
/// 把用户的意思读成另一个时刻，而这张表是用来对账的（宁可他当场看到报错）。
fn parse_hhmm(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    if b.len() != 5 || b[2] != b':' {
        return None;
    }
    let digit = |i: usize| -> Option<u32> { b[i].is_ascii_digit().then(|| (b[i] - b'0') as u32) };
    let h = digit(0)? * 10 + digit(1)?;
    let m = digit(3)? * 10 + digit(4)?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(h * 60 + m)
}

/// 一个「四类价格」条目（模型本体与它的每个时段）的公共校验。
///
/// 四类**缺一不可**：少一个字段在面板上的表现是「这一档的金额悄悄少算一块」，
/// 比当场报错难查得多。非负 + 有限（挡住 `NaN` / `Infinity`）。
fn validate_price_fields(
    what: &str,
    entry: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    for field in ["input", "cache_read", "cache_write", "output"] {
        let num = entry
            .get(field)
            .ok_or_else(|| format!("{what} 缺少 `{field}`"))?
            .as_f64()
            .ok_or_else(|| format!("{what} 的 `{field}` 必须是数字"))?;
        if !num.is_finite() || num < 0.0 {
            return Err(format!("{what} 的 `{field}` 必须是非负数字"));
        }
    }
    Ok(())
}

/// 校验一个 `holidays` 字段：必须是**字符串数组**，且每项都是严格的 `YYYY-MM-DD`。
///
/// 为什么要挡：节假日命中的效果是「整天按谷价」，写错一个日期**不报错、只是静默算错**；
/// 而这张表本来就是对账用的，宁可他当场看到报错。
fn validate_holidays_field(what: &str, v: &serde_json::Value) -> Result<(), String> {
    let arr = v
        .as_array()
        .ok_or_else(|| format!("{what} 的 `holidays` 必须是一个数组"))?;
    for (i, d) in arr.iter().enumerate() {
        let s = d
            .as_str()
            .ok_or_else(|| format!("{what} 的 `holidays[{i}]` 必须是 `YYYY-MM-DD` 字符串"))?;
        validate_iso_date(s)
            .map_err(|_| format!("{what} 的 `holidays[{i}]` 不是合法的 `YYYY-MM-DD`：{s}"))?;
    }
    Ok(())
}

/// 校验一份定价表文本 —— **用户手写的文件与 agent 抓来的候选走同一处**。
///
/// 底线：顶层是对象 / `models` 是对象 / 每个模型的四类价格齐全且非负 /
/// `time_windows`（可选）的每一条同样四类齐全，且时段写法与星期写法合法 /
/// `holidays`（可选，顶层或模型级）是合法的 `YYYY-MM-DD` 数组。
/// 未知字段一律忽略（用户想加注释字段随他）。
pub fn validate_pricing_text(text: &str) -> Result<(), String> {
    let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("语法错误：{e}"))?;
    let obj = v.as_object().ok_or("顶层必须是一个对象")?;
    if let Some(h) = obj.get("holidays") {
        validate_holidays_field("顶层", h)?;
    }
    let models = match obj.get("models") {
        // 允许「只有 updated_at、还没有任何价格」的过渡态（骨架就是这样）
        None => return Ok(()),
        Some(m) => m.as_object().ok_or("`models` 必须是一个对象")?,
    };
    for (name, entry) in models {
        let entry = entry
            .as_object()
            .ok_or_else(|| format!("模型 `{name}` 的值必须是一个对象"))?;
        validate_price_fields(&format!("模型 `{name}`"), entry)?;
        // 模型级 `holidays`（可选，覆盖顶层）—— 放在 `time_windows` 那段之前，
        // 否则下面那句 `continue`（无时段窗就跳过）会把它一起跳掉。
        if let Some(h) = entry.get("holidays") {
            validate_holidays_field(&format!("模型 `{name}`"), h)?;
        }

        // 时段价（2026-09-29）：`time_windows` 里的条目**覆盖**基础四类价。
        // 命中的判定（本地时刻落在哪一条）在前端，这里只管「写得对不对」。
        let Some(windows) = entry.get("time_windows") else {
            continue;
        };
        let arr = windows
            .as_array()
            .ok_or_else(|| format!("模型 `{name}` 的 `time_windows` 必须是一个数组"))?;
        for (i, w) in arr.iter().enumerate() {
            let what = format!("模型 `{name}` 的 `time_windows[{i}]`");
            let w = w.as_object().ok_or_else(|| format!("{what} 必须是一个对象"))?;
            validate_price_fields(&what, w)?;
            let from = w
                .get("from")
                .and_then(serde_json::Value::as_str)
                .and_then(parse_hhmm)
                .ok_or_else(|| format!("{what} 的 `from` 必须是 `HH:MM`（如 09:00）"))?;
            let to = w
                .get("to")
                .and_then(serde_json::Value::as_str)
                .and_then(parse_hhmm)
                .ok_or_else(|| format!("{what} 的 `to` 必须是 `HH:MM`（如 12:00）"))?;
            if from >= to {
                return Err(format!(
                    "{what} 必须满足 `from` 早于 `to`（跨午夜请拆成两条，如 22:00-24:00 与 00:00-06:00）"
                ));
            }
            if let Some(days) = w.get("days") {
                let days = days
                    .as_array()
                    .ok_or_else(|| format!("{what} 的 `days` 必须是一个数组"))?;
                if days.is_empty() {
                    return Err(format!("{what} 的 `days` 不能是空数组（省略它表示每天）"));
                }
                let mut seen = [false; 8];
                for d in days {
                    let n = d
                        .as_u64()
                        .ok_or_else(|| format!("{what} 的 `days` 只能是 1–7 的整数"))?;
                    if !(1..=7).contains(&n) {
                        return Err(format!("{what} 的 `days` 只能是 1–7（1=周一 … 7=周日）"));
                    }
                    if seen[n as usize] {
                        return Err(format!("{what} 的 `days` 里 `{n}` 重复了"));
                    }
                    seen[n as usize] = true;
                }
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
    /// 本回合是否**产出了助手消息**（收尾正文）。
    ///
    /// 历史渲染据此把「有消息」的过程块钉到对应助手气泡后、把「无消息」（只调工具 / 被
    /// 强制中断，没有收尾正文）的过程块挂到它自己的用户气泡之后。旧实现用「分组序号 ==
    /// 助手气泡序号」的对齐假设，一旦出现无消息的回合，后面每一组都会整体错位（用户报的
    /// 「输出挂到错误消息下」）。**旧记录没有该字段 ⇒ `None` 按「有消息」处理**（旧行为）。
    #[serde(rename = "hasMsg", default, skip_serializing_if = "Option::is_none")]
    pub has_msg: Option<bool>,
    #[serde(default)]
    pub items: Vec<SessionStep>,
}

/// 待办清单在某一回合的快照（2026-10-02）。
///
/// 为什么要有它：待办清单原本是**纯前端内存态**（`main.ts` 的 `todoItems`），回退历史时
/// 只能整块清掉（`rebuildChangedFilesFromSteps` 的旧实现），表现就是「一点回退，任务列表
/// 就没了」。现在按**回合**存一条时间线，回退到某条消息时取「回退点那一刻」的那份 ——
/// 既不会丢，也不会把回退点之后的待办显示出来。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct SessionTodo {
    /// 采集时**已完成的助手消息条数**（= 回合下标，0 起；与 `SessionProcess.turn` 同口径）
    #[serde(default)]
    pub turn: u64,
    /// 这一刻的完整待办清单（`[{content, status}]`）。
    /// **刻意不解构**：Rust 侧只负责存取，字段语义全在前端（见 `main.ts` 的
    /// `sessionTodoTimeline` / `renderTodoDrawer`）—— 这里解构一次就多一份「待办长什么样」
    /// 的真相，而渲染用的只有 `content` / `status` 两个字段，将来加字段还得改两处。
    #[serde(default)]
    pub todos: Vec<serde_json::Value>,
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
    /// 待办清单时间线 —— 回退 / 切会话后重建任务抽屉（见 `SessionTodo`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todos: Option<Vec<SessionTodo>>,
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
    crate::chat_db::save(&chat_db_path(), &sessions)?;
    // 会话表落盘成功后顺手清理**已被删掉的会话**留下的快照文件（2026-10-01）。
    // 放在这里而不是前端的删除路径：这是**唯一**知道「现在还剩哪些会话」的地方；
    // 散在前端迟早漏一处 —— 表现是「历史删了，可用户文件副本还躺在盘上」。
    // 不返回错误：清理失败不该让会话保存失败。
    crate::snapshots::prune_frames(
        &sessions.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
    );
    Ok(())
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

/// 剪贴板历史上限，与前端插件的 `MAX_ITEMS` **同值**（30，两边一起改）。
const MAX_CLIP_ITEMS: usize = 30;

/// 「粘贴进搜索框时顺手记一条」——**宿主自己实现**（2026-09-29）。
///
/// 为什么不再由插件提供：剪贴板历史已归入**拓展插件**（不随安装包默认安装），
/// 而「粘贴一下就记一条」是**主窗口**的行为 —— 它必须在插件没装时也照常工作，
/// 更不该去 `import` 一个可能不存在的插件模块（那正是 `Modules\` 化要消掉的耦合）。
///
/// 语义与插件里的 `addClipboardEntry()` **逐条对齐**（去重 → 置顶 → 截断到 30 条）；
/// 插件那份从此只读不写，两边不会各写一套。
#[tauri::command]
pub fn append_clipboard_entry(text: String) -> Result<(), String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    // 与前端 `trimmed.length > 2000` 同一口径：JS 的 `length` 数的是 **UTF-16 码元**，
    // 用 `chars().count()` 会把 emoji 算少 ⇒ 同一段文本在两边可能一个收一个不收。
    if trimmed.encode_utf16().count() > 2000 {
        return Ok(());
    }
    let is_file = is_windows_path(trimmed);
    let mut entries = load_clipboard_history()?;
    // 去重：文件条目按「类型 + 全文」找，文本条目按全文找 —— 与插件那份一致
    let dup = entries.iter().position(|e| {
        if is_file {
            e.clip_type == "file" && e.text == trimmed
        } else {
            e.text == trimmed
        }
    });
    if let Some(i) = dup {
        entries.remove(i);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    entries.insert(
        0,
        ClipEntry {
            clip_type: if is_file { "file".into() } else { "text".into() },
            text: trimmed.to_string(),
            file_paths: if is_file { vec![trimmed.to_string()] } else { vec![] },
            time: now,
        },
    );
    entries.truncate(MAX_CLIP_ITEMS);
    save_clipboard_history(entries)
}

/// `D:\...` / `C:/...` 这类「盘符 + 冒号 + 斜杠」开头的绝对路径
/// —— 与前端正则 `^[A-Za-z]:[\\/]` 同义（ASCII 盘符，别用 `is_alphabetic()` 放中文进来）。
fn is_windows_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
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
    /// 上面四类总量里**来自子代理 / 后台复盘**的部分（2026-09-29）。
    ///
    /// 与顶层同名同口径，但**是其中的一部分** —— 记账时**不得**再加一次（加了就是重复计费），
    /// 它的用途是**归因**：「这一问的量有多少是子代理烧的」。此前这些 token 根本没进
    /// `result.usage`（平台照收钱、本地账上看不见），本字段是修复后的观测面。
    /// 旧记录没有该字段，读时按 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<UsageSubagent>,
    /// 这条记录是**中断收尾的兜底值**（2026-10-06）：回合被取消 / agent 被强杀，拿不到
    /// 收尾的 `result.usage`，只能用 agent 逐请求上报的 `usage_delta` 累计值落账。
    ///
    /// 它**只覆盖已完成的上报请求** —— 最后一个飞行中的请求不在内，所以金额天然偏低一点。
    /// 标记出来是为了对账时能分辨「少是正常的」，不要当成漏记去查。正常收尾的记录该字段为
    /// `false` 且**不写盘**（旧记录没有它，读时按 `false`）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
}

/// `UsageRecord::subagent` 的形状（字段名与 agent 上报的原样一致，不做 camelCase 转换）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UsageSubagent {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    /// 子代理 / 复盘自己发的请求条数（它们的每次请求也各占平台一行）
    #[serde(default)]
    pub requests: u64,
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

/// 某个模型在一天里**某一个本地小时**的合计（分时价用，2026-09-29）。
///
/// 为什么粒度停在「小时」而不是「每次请求」：一次提问的多次 API 请求在本地日志里是**一条**
/// 记录（只有记录级的 `ts`），逐请求时刻根本没落盘。而分时价的时段边界都在整点或半点，
/// 小时桶足够；跨小时的一次提问会被整条记在**它的 `ts` 所属**的那个小时里 —— 如实、不做插值。
#[derive(Debug, Serialize, Clone, Default)]
pub struct UsageHourTotals {
    /// 本地小时 0–23（用调用方传进来的时区偏移把 `ts` 换算成墙钟时刻）
    pub hour: u32,
    pub turns: u64,
    /// **真实 API 请求数**（= 这些记录里 `requests` 数组的条数之和，2026-10-06）。
    /// 与 `turns` 的区别：一次提问里模型可能往返多次，平台上每一趟都占一行。
    /// 旧记录没有 `requests` 数组 ⇒ 按 1 计（见 `read_usage_range` 里的注释）。
    #[serde(default)]
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    #[serde(rename = "cacheCreate")]
    pub cache_create: u64,
}

/// 某个模型在一天里的合计（决定用哪一档单价）
#[derive(Debug, Serialize, Clone, Default)]
pub struct UsageModelTotals {
    pub model: String,
    /// 提问次数（一行日志 = 一次提问，含提问内所有工具往返）
    pub turns: u64,
    /// 真实 API 请求数（见 `UsageHourTotals::requests`）
    #[serde(default)]
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    #[serde(rename = "cacheCreate")]
    pub cache_create: u64,
    /// 按本地小时拆开的同一批量（**与上面四个字段是同一批 token**，只是再切一刀）。
    /// 价格表里该模型有 `time_windows` 时，前端必须用它逐桶计价；没有时段价时它与
    /// 「总量 × 基础价」等价，前端可以只看上面四个字段。旧宿主不返回该字段 → 空表。
    #[serde(default)]
    pub hours: Vec<UsageHourTotals>,
}

/// 一天的合计（面板「按天表格」的一行）
#[derive(Debug, Serialize, Clone, Default)]
pub struct UsageDay {
    pub date: String,
    pub turns: u64,
    /// 真实 API 请求数（见 `UsageHourTotals::requests`）
    #[serde(default)]
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    #[serde(rename = "cacheCreate")]
    pub cache_create: u64,
    /// 这一天用到的模型（按模型名升序）；金额按这里的每一项分别计价
    pub models: Vec<UsageModelTotals>,
}

/// 读**多天**用量并汇总成「按天 + 按模型（+ 按本地小时）」的形状（A12 成本面板）。
///
/// 日期一律由前端给（与 `append_usage_log` 同一套：Rust 侧没有 chrono），**升序**返回；
/// 没有任何记录的天不返回（面板不显示全零行）。汇总放在宿主而不是让前端逐天 IPC：
/// 30 天就是 30 次跨进程调用，这里一次读完。
///
/// `utc_offset_minutes` = 本地时区相对 UTC 的偏移（**东八区传 480**，即
/// `-new Date().getTimezoneOffset()`）。`ts` 是 UTC epoch 毫秒，`Rust` 侧没有时区数据库，
/// 所以「本地墙钟」这件事只能由调用方给一个偏移量 —— 分时价必须知道本地时刻。
/// 夏令时切换日会偏 1 小时（本机与主要目标用户都在无 DST 的时区；如实记着，不做 DST 推断）。
#[tauri::command]
pub fn read_usage_range(
    dates: Vec<String>,
    utc_offset_minutes: i32,
) -> Result<Vec<UsageDay>, String> {
    let offset_ms = (utc_offset_minutes as i64) * 60_000;
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
        let mut hours_by_model: std::collections::BTreeMap<
            String,
            std::collections::BTreeMap<u32, UsageHourTotals>,
        > = std::collections::BTreeMap::new();
        for r in records {
            // **真实 API 请求数**（2026-10-06 加，用户口径）：一次提问里模型可能往返多次，
            // 平台上每一趟各占一行 —— `requests` 数组就是逐请求落的那份明细。
            // 旧记录没有该字段（`serde(default)` ⇒ 空表）⇒ **按 1 计**：一条用量记录至少
            // 对应一次 API 请求，记 0 会让人以为「这天没发过请求」。
            let reqs = r.requests.len().max(1) as u64;
            day.turns += 1;
            day.requests += reqs;
            day.input += r.input;
            day.output += r.output;
            day.cache_read += r.cache_read;
            day.cache_create += r.cache_create;
            let m = by_model.entry(r.model.clone()).or_insert_with(|| UsageModelTotals {
                model: r.model.clone(),
                ..Default::default()
            });
            m.turns += 1;
            m.requests += reqs;
            m.input += r.input;
            m.output += r.output;
            m.cache_read += r.cache_read;
            m.cache_create += r.cache_create;

            // 本地小时 = (ts + 偏移) 的整点时刻对 24 取模（`rem_euclid` 保证非负）
            let hour = ((r.ts as i64 + offset_ms).div_euclid(3_600_000)).rem_euclid(24) as u32;
            let h = hours_by_model
                .entry(r.model.clone())
                .or_default()
                .entry(hour)
                .or_insert_with(|| UsageHourTotals {
                    hour,
                    ..Default::default()
                });
            h.turns += 1;
            h.requests += reqs;
            h.input += r.input;
            h.output += r.output;
            h.cache_read += r.cache_read;
            h.cache_create += r.cache_create;
        }
        for m in by_model.values_mut() {
            if let Some(buckets) = hours_by_model.remove(&m.model) {
                m.hours = buckets.into_values().collect();
            }
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
            subagent: None,
            partial: false,
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

    /// 子代理 / 复盘的归因字段（2026-09-29）：它是**总量的一部分**，只作归因 ——
    /// 旧记录没有该键（读成 `None`），`None` 也不写回日志（不给旧格式添噪音）。
    #[test]
    fn usage_record_subagent_attribution_is_optional() {
        let old = r#"{"ts":1,"model":"m","input":1,"output":1,"cacheRead":0,"cacheCreate":0}"#;
        assert!(serde_json::from_str::<UsageRecord>(old).unwrap().subagent.is_none());

        let mut r = rec(0);
        r.subagent = Some(UsageSubagent {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_input_tokens: 30,
            cache_creation_input_tokens: 0,
            requests: 2,
        });
        let line = serde_json::to_string(&r).unwrap();
        assert!(
            line.contains(
                r#""subagent":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":30,"cache_creation_input_tokens":0,"requests":2}"#
            ),
            "{line}"
        );
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
        // 真 API 请求数（2026-10-06）：这一条里有 3 次请求 ——
        // 其余记录没有 `requests` 数组（旧格式）⇒ 各按 1 计
        other.requests = vec![UsageRequest::default(); 3];
        append_usage_log(d1.into(), other).unwrap();
        append_usage_log(d2.into(), rec(7000)).unwrap();

        // 中间夹一天没记录 → 不返回（面板不显示全零行）
        let days = read_usage_range(
            vec![d1.into(), "1970-01-05".into(), d2.into()],
            0,
        )
        .unwrap();
        assert_eq!(days.len(), 2, "空天必须被跳过：{days:?}");

        let day1 = &days[0];
        assert_eq!(day1.date, d1);
        assert_eq!(day1.turns, 2);
        assert_eq!(day1.input, 212 + 100);
        assert_eq!(day1.output, 3 + 7);
        assert_eq!(day1.cache_read, 1536);
        // 1（旧格式按 1 计）+ 3（真的 3 次请求）
        assert_eq!(day1.requests, 4, "真 API 请求数要按 `requests` 数组累加");
        let models: Vec<&str> = day1.models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(models, vec!["deepseek-flash", "gpt-4o"], "按模型分组");
        assert_eq!(day1.models[1].input, 100);
        assert_eq!(day1.models[0].cache_read, 1536);
        assert_eq!(day1.models[0].requests, 1);
        assert_eq!(day1.models[1].requests, 3);
        // 小时桶也要带请求数（两条记录的 ts 同属一个桶）
        assert_eq!(day1.models[0].hours[0].requests, 1);
        assert_eq!(day1.models[1].hours[0].requests, 3);

        assert_eq!(days[1].turns, 1);
        assert_eq!(days[1].cache_read, 7000);
        assert_eq!(days[1].requests, 1);

        // 字段名是前端消费契约（驼峰）
        let json = serde_json::to_string(day1).unwrap();
        assert!(json.contains(r#""cacheRead":1536"#), "{json}");
        assert!(json.contains(r#""cacheCreate":0"#), "{json}");
        assert!(json.contains(r#""requests":4"#), "{json}");

        for d in [d1, d2] {
            let _ = fs::remove_file(usage_log_path(d).unwrap());
        }
    }

    /// 分时价的**时间维度**：同一模型同一天的 token 必须按**本地小时**拆开，
    /// 否则前端根本没有「这一批发生在几点」可用于查时段价（2026-09-29）。
    ///
    /// `ts` 是 UTC epoch 毫秒，本地小时 = `(ts + 偏移)` 换算 —— 这里用**东八区（480）**
    /// 与 **UTC（0）** 两组对同一份记录各跑一次，钉住「偏移真的参与了换算」。
    #[test]
    fn usage_range_buckets_by_local_hour() {
        let d = "1970-01-06";
        let _ = fs::remove_file(usage_log_path(d).unwrap());

        // 1970-01-01T00:00:00Z 起 12 小时 = 43200_000 ms → UTC 12 点，东八区 20 点
        let mut a = rec(10);
        a.ts = 43_200_000;
        a.input = 100;
        append_usage_log(d.into(), a).unwrap();
        // 再过 1 小时 → UTC 13 点，东八区 21 点
        let mut b = rec(20);
        b.ts = 46_800_000;
        b.input = 200;
        append_usage_log(d.into(), b).unwrap();

        let utc = &read_usage_range(vec![d.into()], 0).unwrap()[0];
        let hours: Vec<(u32, u64)> = utc.models[0]
            .hours
            .iter()
            .map(|h| (h.hour, h.input))
            .collect();
        assert_eq!(hours, vec![(12, 100), (13, 200)], "UTC 桶");

        let cn = &read_usage_range(vec![d.into()], 480).unwrap()[0];
        let hours: Vec<(u32, u64)> = cn.models[0]
            .hours
            .iter()
            .map(|h| (h.hour, h.input))
            .collect();
        assert_eq!(hours, vec![(20, 100), (21, 200)], "东八区桶（偏移参与换算）");

        // 桶与总量是**同一批 token 再切一刀**，不是两批
        assert_eq!(cn.models[0].input, 300);
        assert_eq!(utc.models[0].input, 300);
        let total: u64 = cn.models[0].hours.iter().map(|h| h.input).sum();
        assert_eq!(total, 300, "逐桶之和必须等于总量");

        let _ = fs::remove_file(usage_log_path(d).unwrap());
    }

    /// 预置定价表必须**自身合法**，且时段写法与官方规则一致（2026-09-29）。
    ///
    /// 预置一份不合法的价表比不预置更糟：面板要么整块算不出金额，要么弹「候选价不合法」
    /// 而用户根本没动过任何东西。顺带钉住「谷价是基础价、峰价在 time_windows 里」
    /// 这个约定 —— 反过来的话面板会把每天大部分时段算成峰价。
    #[test]
    fn default_pricing_json_is_valid() {
        validate_pricing_text(DEFAULT_PRICING_JSON).expect("预置价表必须合法");
        let v: serde_json::Value = serde_json::from_str(DEFAULT_PRICING_JSON).unwrap();
        let m = &v["models"]["deepseek-v4-flash"];
        assert_eq!(m["input"].as_f64(), Some(1.0), "基础价 = 谷价");
        assert_eq!(m["output"].as_f64(), Some(4.0), "基础价 = 谷价");
        let windows = m["time_windows"].as_array().expect("必须有峰时窗口");
        assert_eq!(windows.len(), 2, "官方峰时是两段：09:00-12:00 与 14:00-18:00");
        assert_eq!(windows[0]["from"].as_str(), Some("09:00"));
        assert_eq!(windows[0]["to"].as_str(), Some("12:00"));
        assert_eq!(windows[1]["from"].as_str(), Some("14:00"));
        assert_eq!(windows[1]["to"].as_str(), Some("18:00"));
        for w in windows {
            // 峰 = 谷 × 2（官方注：谷价是峰价的一半）
            assert_eq!(w["input"].as_f64(), Some(2.0));
            assert_eq!(w["output"].as_f64(), Some(8.0));
            assert_eq!(
                w["days"].as_array().map(|a| a.len()),
                Some(5),
                "峰时只落在周一至周五"
            );
        }
        // 法定节假日（2026-10-06）：官方脚注是「周一至周五**不含中国法定节假日**」——
        // 缺这条会把节假日按峰价高估（2026-10-02 国庆实测多算 0.979 元）。
        let holidays = v["holidays"].as_array().expect("预置表必须带法定节假日");
        for d in [
            "2026-01-01",
            "2026-02-15",
            "2026-02-23",
            "2026-04-04",
            "2026-05-01",
            "2026-06-19",
            "2026-09-25",
            "2026-10-01",
            "2026-10-02",
            "2026-10-07",
        ] {
            assert!(
                holidays.iter().any(|x| x.as_str() == Some(d)),
                "预置节假日表缺 {d}（国务院 2026 放假安排）"
            );
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
            // 时段价（2026-09-29）：容器必须数组 / 每条四类齐全 / from<to / HH:MM / days 合法
            r#"{"models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": 8, "time_windows": {}}}}"#,
            r#"{"models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": 8, "time_windows": [{"from": "09:00", "to": "12:00", "input": 2, "cache_read": 0.04, "cache_write": 0}]}}}"#,
            r#"{"models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": 8, "time_windows": [{"from": "9:00", "to": "12:00", "input": 2, "cache_read": 0.04, "cache_write": 0, "output": 8}]}}}"#,
            r#"{"models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": 8, "time_windows": [{"from": "12:00", "to": "09:00", "input": 2, "cache_read": 0.04, "cache_write": 0, "output": 8}]}}}"#,
            r#"{"models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": 8, "time_windows": [{"from": "25:00", "to": "26:00", "input": 2, "cache_read": 0.04, "cache_write": 0, "output": 8}]}}}"#,
            r#"{"models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": 8, "time_windows": [{"from": "09:00", "to": "12:00", "days": [], "input": 2, "cache_read": 0.04, "cache_write": 0, "output": 8}]}}}"#,
            r#"{"models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": 8, "time_windows": [{"from": "09:00", "to": "12:00", "days": [0], "input": 2, "cache_read": 0.04, "cache_write": 0, "output": 8}]}}}"#,
            r#"{"models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": 8, "time_windows": [{"from": "09:00", "to": "12:00", "days": [8], "input": 2, "cache_read": 0.04, "cache_write": 0, "output": 8}]}}}"#,
            r#"{"models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": 8, "time_windows": [{"from": "09:00", "to": "12:00", "days": [1, 1], "input": 2, "cache_read": 0.04, "cache_write": 0, "output": 8}]}}}"#,
            // 法定节假日（2026-10-06）：容器必须数组 / 每项必须是严格的 YYYY-MM-DD
            r#"{"holidays": {}, "models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": 8}}}"#,
            r#"{"holidays": ["2026/10/02"], "models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": 8}}}"#,
            r#"{"holidays": [20261002], "models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": 8}}}"#,
            r#"{"models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": 8, "holidays": ["2026-10-2"]}}}"#,
        ] {
            assert!(validate_pricing_text(bad).is_err(), "should reject `{bad}`");
        }
        let good = r#"{"models": {"m": {"input": 2, "cache_read": 0.5, "cache_write": 0, "output": 8,
            "source_url": "https://example.com/pricing", "updated_at": "1970-01-01"}}}"#;
        assert!(validate_pricing_text(good).is_ok());
        // 合法时段价：days 可省略（= 每天），也可是 1..=7 的不重复列表
        let good_windows = r#"{"models": {"m": {"input": 1, "cache_read": 0.02, "cache_write": 0, "output": 4,
            "time_windows": [
                {"days": [1, 2, 3, 4, 5], "from": "09:00", "to": "12:00",
                 "input": 2, "cache_read": 0.04, "cache_write": 0, "output": 8},
                {"from": "00:00", "to": "06:00",
                 "input": 0.5, "cache_read": 0.01, "cache_write": 0, "output": 2}
            ]}}}"#;
        assert!(validate_pricing_text(good_windows).is_ok());
        // 合法节假日：顶层与模型级（模型级覆盖顶层）都要放行
        let good_holidays = r#"{"holidays": ["2026-10-01", "2026-10-02"],
            "models": {"m": {"input": 1, "cache_read": 0.02, "cache_write": 0, "output": 4,
              "holidays": ["2026-01-01"]}}}"#;
        assert!(validate_pricing_text(good_holidays).is_ok());
        // 空数组 = 明确声明「没有法定节假日」，也算合法（不当作缺字段）
        assert!(validate_pricing_text(
            r#"{"holidays": [], "models": {"m": {"input": 1, "cache_read": 0, "cache_write": 0, "output": 8}}}"#
        )
        .is_ok());
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
