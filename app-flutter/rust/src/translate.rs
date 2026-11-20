//! 翻译插件的宿主侧（移植 v1 `app/src-tauri/src/translate.rs`）。
//!
//! 三层结构与 v1 **一一对应**（`app/src-tauri/src/translate.rs:8-20`）：
//!   ① 本地缓存：`<exe 根>/ModuleData/translate/cache.db` 的 `translations` 表，
//!      同一 (源语言, 目标语言, 原文) 只查一次外部接口；命中时 `cached=true`；
//!   ② 词典底座（不花钱）：MyMemory 给主译文、有道 jsonapi 给音标 / 释义 / 例句，
//!      两层都没结果时返回 `source="none"`，由面板提示走 AI；
//!   ③ 模型补漏（花钱）：只在用户点按钮时发起，直连 `{agent_endpoint}/v1/messages`
//!      —— 凭据与端点复用 `agent::load_config`（与 AI 对话同一份 `config/ai.json`）。
//!
//! 纪律（与 v1 逐条对齐）：
//!   · 非官方接口**一律降级不报错**：抓不到就 `source="none"`，不把网络错误当成
//!     「查无此词」，也绝不 panic（`lookup` 只对空输入 / 超长返回 Err）；
//!   · HTTP 200 但正文极短 = 被反爬（v1 预检 #41），按无结果处理；
//!   · 失败文案只说「哪一步没成 + 用户能做什么」，状态码 / 响应体只进 stderr 日志。
//!
//! 依赖选型：与 v1 同款 `reqwest::blocking` + `rusqlite(bundled)`。代价是 debug
//! cdylib 明显变大（hyper / tokio / native-tls / SQLite），换来缓存与请求形状的
//! 口径完全一致；测试只测纯解析与临时目录缓存，**不发真实网络请求**。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── 常量（逐项对齐 v1 `translate.rs:51-79`）────────────────────────

/// MyMemory —— 免费、无需 key 的翻译记忆库；匿名额度按天计，超额 `responseStatus: 403`。
const MYMEMORY: &str = "https://api.mymemory.translated.net/get";
/// 有道词典 jsonapi —— **非官方只读接口**（能用就用）。
const YOUDAO: &str = "https://dict.youdao.com/jsonapi";
const YOUDAO_REFERER: &str = "https://dict.youdao.com/";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

/// 两家公开接口的单次超时：答不出来就会很快答不出来，10s 足够。
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// 送进词典的文本上限（MyMemory 匿名额度按字符数算）。
const DICT_MAX_CHARS: usize = 1000;
/// 送进模型的文本上限。
const AI_MAX_CHARS: usize = 20000;
/// 模型输出上限。
const AI_MAX_TOKENS: u32 = 2048;
/// 例句条数上限（面板一列内容，多了没人看）。
const MAX_EXAMPLES: usize = 3;
/// 释义行上限。
const MAX_EXPLAINS: usize = 6;

// ── HTTP client（进程级复用，v1 `translate.rs:81-97`）───────────────

static HTTP_CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();

fn http() -> Result<reqwest::blocking::Client, String> {
    HTTP_CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .map_err(|e| {
                    eprintln!("[lunac-translate] 初始化 HTTP 客户端失败: {e}");
                    "网络组件初始化失败，请稍后再试".to_string()
                })
        })
        .clone()
}

// ── 结果形状（与 v1 `TranslateResult` 字段逐一对应）─────────────────

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct TranslateExample {
    pub src: String,
    pub dst: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TranslateResult {
    /// 原文（已 trim、已截断；面板拿它对账）。
    pub text: String,
    /// 实际使用的源语言（`auto` 已解析成具体值）。
    pub from: String,
    pub to: String,
    /// `dict` = 词典底座；`ai` = 模型补漏；`none` = 两层都没有结果。
    pub source: String,
    /// 是否直接来自本地缓存（这次没碰网络）。
    pub cached: bool,
    /// 主译文；`source="none"` 时为空串。
    pub translation: String,
    /// 音标（有道的英美两个音标拼在一起，空 = 没有）。
    pub phonetic: String,
    /// 释义 / 译文候选行。
    pub explains: Vec<String>,
    /// 双语例句。
    pub examples: Vec<TranslateExample>,
}

impl TranslateResult {
    fn none(text: &str, from: &str, to: &str) -> Self {
        Self {
            text: text.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            source: "none".into(),
            cached: false,
            translation: String::new(),
            phonetic: String::new(),
            explains: Vec::new(),
            examples: Vec::new(),
        }
    }

    /// 这一份有没有可展示的内容（全空 = 词典查不到，面板据此提示走 AI）。
    fn is_empty(&self) -> bool {
        self.translation.trim().is_empty() && self.explains.is_empty() && self.examples.is_empty()
    }
}

// ── 译文缓存（SQLite，与 v1 同表结构）──────────────────────────────

/// `<exe 根>/ModuleData/translate/cache.db`（v1 `translate.rs:158-161`）。
pub fn cache_path(root: &Path) -> PathBuf {
    root.join("ModuleData").join("translate").join("cache.db")
}

fn open_cache(root: &Path) -> Result<Connection, String> {
    let path = cache_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建译文缓存目录失败: {e}"))?;
    }
    let conn = Connection::open(&path).map_err(|e| format!("打开译文缓存失败: {e}"))?;
    // WAL 让读写不互斥；`execute_batch` 才能吞下返回行的 PRAGMA（v1 同口径）。
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;");
    let _ = conn.busy_timeout(Duration::from_millis(3000));
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS translations (
            key        TEXT PRIMARY KEY,
            src        TEXT NOT NULL,
            dst        TEXT NOT NULL,
            source     TEXT NOT NULL,
            payload    TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );",
    )
    .map_err(|e| format!("建译文缓存表失败: {e}"))?;
    Ok(conn)
}

/// 缓存键 = 源语言 + 目标语言 + 原文；**不做大小写归一**（与 v1 同口径）。
fn cache_key(from: &str, to: &str, text: &str) -> String {
    format!("{from}\u{1}{to}\u{1}{text}")
}

fn cache_get(root: &Path, from: &str, to: &str, text: &str) -> Option<TranslateResult> {
    let conn = open_cache(root).ok()?;
    let mut stmt = conn
        .prepare("SELECT payload FROM translations WHERE key = ?1")
        .ok()?;
    let payload: String = stmt
        .query_row([cache_key(from, to, text)], |r| r.get(0))
        .ok()?;
    let mut hit: TranslateResult = serde_json::from_str(&payload).ok()?;
    hit.cached = true;
    Some(hit)
}

/// 只缓存**有内容**的结果：查不到不写，避免一个还没收录的词被永久钉死。
fn cache_put(root: &Path, res: &TranslateResult) {
    if res.is_empty() {
        return;
    }
    let Ok(conn) = open_cache(root) else { return };
    let Ok(payload) = serde_json::to_string(res) else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let _ = conn.execute(
        "INSERT OR REPLACE INTO translations (key, src, dst, source, payload, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            cache_key(&res.from, &res.to, &res.text),
            res.from,
            res.to,
            res.source,
            payload,
            now,
        ],
    );
}

// ── 语言判定（v1 `translate.rs:236-270`）───────────────────────────

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF     // 日文假名
        | 0x3400..=0x4DBF   // CJK 扩展 A
        | 0x4E00..=0x9FFF   // CJK 基本区
        | 0xAC00..=0xD7AF   // 韩文音节
        | 0xF900..=0xFAFF   // CJK 兼容表意
        | 0x20000..=0x2FA1F // CJK 扩展 B+
    )
}

/// `auto` → 具体语言：含 CJK 字符算中文，否则算英语（与 v1 完全一致）。
fn detect_lang(text: &str) -> &'static str {
    if text.chars().any(is_cjk) {
        "zh-CN"
    } else {
        "en"
    }
}

/// 源语言码规范化：空 / `auto` 交给本地判据，其余原样透传（显式选择不猜）。
fn resolve_from(from: &str, text: &str) -> String {
    let f = from.trim();
    if f.is_empty() || f.eq_ignore_ascii_case("auto") {
        detect_lang(text).to_string()
    } else {
        f.to_string()
    }
}

// ── 词典底座解析（纯函数，单测只用 fixture）─────────────────────────

/// MyMemory 响应解析：`responseStatus != 200` / 结果等于原文 / 空串一律 `None`。
fn parse_mymemory(raw: &str, text: &str) -> Option<String> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let inner = v
        .get("responseStatus")
        .and_then(Value::as_i64)
        .unwrap_or(200);
    if inner != 200 {
        eprintln!("[lunac-translate] MyMemory 拒绝：responseStatus={inner}");
        return None;
    }
    let out = v
        .pointer("/responseData/translatedText")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if out.is_empty() || out.eq_ignore_ascii_case(text.trim()) {
        eprintln!("[lunac-translate] MyMemory 未给出有效译文");
        return None;
    }
    Some(out)
}

/// 有道 `trs` 里的 `l.i` 可能是字符串，也可能是 `{"#text": …}`（中文词条）。
fn trs_line(node: &Value) -> Option<String> {
    trs_items(node.pointer("/l/i")?)
}

/// `i` 字段的正文：字符串 / 字符串数组 / 带 `#text` 的对象数组。
fn trs_items(items: &Value) -> Option<String> {
    let mut buf = String::new();
    match items {
        Value::String(s) => buf.push_str(s),
        Value::Array(a) => {
            for it in a {
                match it {
                    Value::String(s) => buf.push_str(s),
                    Value::Object(o) => {
                        if let Some(t) = o.get("#text").and_then(Value::as_str) {
                            buf.push_str(t);
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => return None,
    }
    let s = buf.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// 有道响应解析：音标 / 释义 / 双语例句（v1 `translate.rs:355-495` 的纯解析部分）。
fn parse_youdao(raw: &str) -> (String, Vec<String>, Vec<TranslateExample>) {
    let mut phonetic = String::new();
    let mut explains: Vec<String> = Vec::new();
    let mut examples: Vec<TranslateExample> = Vec::new();

    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        eprintln!("[lunac-translate] 有道响应不是合法 JSON");
        return (phonetic, explains, examples);
    };

    // 音标：英美两条都取（只取一个有偏向）。
    if let Some(w) = v.pointer("/ec/word/0") {
        let uk = w
            .get("ukphone")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let us = w
            .get("usphone")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        phonetic = match (uk.is_empty(), us.is_empty()) {
            (false, false) if uk != us => format!("英 {uk}  美 {us}"),
            (false, _) => uk.to_string(),
            (_, false) => us.to_string(),
            _ => String::new(),
        };
        if let Some(arr) = w.get("trs").and_then(Value::as_array) {
            for item in arr {
                if let Some(line) = item
                    .get("tr")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
                    .and_then(trs_line)
                {
                    // 同一条释义会在不同词性下重复出现。
                    if !explains.contains(&line) {
                        explains.push(line);
                    }
                }
            }
        }
    }
    // 中文词条：释义在 `ce.word[0].trs`（`ec` 只对英文词条有）。
    if explains.is_empty() {
        if let Some(arr) = v.pointer("/ce/word/0/trs").and_then(Value::as_array) {
            for item in arr {
                let Some(l) = item
                    .get("tr")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
                    .and_then(|t| t.get("l"))
                else {
                    continue;
                };
                // 中文词条的 `l` 自己就是 `{i:…,#tran:…}`，直接取 `i`。
                if let Some(line) = l.get("i").and_then(trs_items) {
                    if !explains.contains(&line) {
                        explains.push(line);
                    }
                }
            }
        }
    }
    explains.truncate(MAX_EXPLAINS);

    // 双语例句：`blng_sents_part.sentence-pair[]`。
    if let Some(pairs) = v
        .pointer("/blng_sents_part/sentence-pair")
        .and_then(Value::as_array)
    {
        for p in pairs {
            let src = p
                .get("sentence")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let dst = p
                .get("sentence-translation")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if src.is_empty() || dst.is_empty() {
                continue;
            }
            examples.push(TranslateExample {
                src: src.to_string(),
                dst: dst.to_string(),
            });
            if examples.len() >= MAX_EXAMPLES {
                break;
            }
        }
    }

    (phonetic, explains, examples)
}

/// 有道 `ec.word[0].trs` 的第一条 —— MyMemory 没结果时的主译文兜底。
fn youdao_first_trs(explains: &[String]) -> Option<String> {
    explains.first().cloned()
}

/// HTTP 200 但正文极短 = 被反爬（给的是验证页），**别去改解析**（v1 预检 #41）。
fn youdao_body_looks_like_challenge(raw: &str) -> bool {
    raw.len() < 512
}

// ── 联网：两家免 key 接口 ──────────────────────────────────────────

fn mymemory_translate(
    client: &reqwest::blocking::Client,
    text: &str,
    from: &str,
    to: &str,
) -> Option<String> {
    let resp = client
        .get(MYMEMORY)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .query(&[("q", text), ("langpair", &format!("{from}|{to}"))])
        .send()
        .map_err(|e| eprintln!("[lunac-translate] MyMemory 请求失败: {e}"))
        .ok()?;
    let status = resp.status();
    let raw = resp.text().ok()?;
    if !status.is_success() {
        eprintln!(
            "[lunac-translate] MyMemory 返回 {status}: {}",
            truncate_chars(&raw, 200)
        );
        return None;
    }
    parse_mymemory(&raw, text)
}

fn youdao_detail(
    client: &reqwest::blocking::Client,
    text: &str,
) -> (String, Vec<String>, Vec<TranslateExample>) {
    let empty = (String::new(), Vec::new(), Vec::new());
    let resp = match client
        .get(YOUDAO)
        .header("User-Agent", UA)
        .header("Accept", ACCEPT)
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("Referer", YOUDAO_REFERER)
        .query(&[("q", text)])
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[lunac-translate] 有道请求失败: {e}");
            return empty;
        }
    };
    let status = resp.status();
    let raw = match resp.text() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[lunac-translate] 有道响应读取失败: {e}");
            return empty;
        }
    };
    if !status.is_success() {
        eprintln!(
            "[lunac-translate] 有道返回 {status}: {}",
            truncate_chars(&raw, 200)
        );
        return empty;
    }
    if youdao_body_looks_like_challenge(&raw) {
        eprintln!(
            "[lunac-translate] 有道正文仅 {} 字节（疑似反爬验证页），按无结果处理",
            raw.len()
        );
        return empty;
    }
    parse_youdao(&raw)
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_string()
    } else {
        text.chars().take(limit).collect::<String>() + "…"
    }
}

// ── 模型补漏（复用 AI 对话的 config/ai.json，只读）──────────────────

/// 模型端点：`LUNAC_AGENT_BASE_URL`（agent 正在跑时由宿主注入）优先，否则按
/// `ai.json` 现算，再拼 `/v1/messages` —— 与 v1 `model_endpoint` 同口径。
fn model_endpoint(root: &Path) -> Result<String, String> {
    if let Ok(base) = std::env::var("LUNAC_AGENT_BASE_URL") {
        let b = base.trim().trim_end_matches('/');
        if !b.is_empty() {
            return Ok(format!("{b}/v1/messages"));
        }
    }
    let (config, _) = crate::agent::load_config(root)?;
    let base = config.endpoint();
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("还没配置 AI，请先到设置里填好".to_string());
    }
    Ok(format!("{base}/v1/messages"))
}

fn lang_name(code: &str) -> String {
    match code {
        "zh-CN" | "zh" | "zh-TW" => "简体中文".to_string(),
        "en" => "英语".to_string(),
        "ja" => "日语".to_string(),
        "ko" => "韩语".to_string(),
        other => other.to_string(),
    }
}

/// 非流式一次往返；提示词只做一件事：**只要译文**（v1 `model_translate`）。
fn model_translate(
    root: &Path,
    client: &reqwest::blocking::Client,
    text: &str,
    from: &str,
    to: &str,
) -> Result<String, String> {
    let (config, _) = crate::agent::load_config(root).map_err(|e| {
        eprintln!("[lunac-translate] 取 AI 凭据失败: {e}");
        "还没配置 AI，请先到设置里填好".to_string()
    })?;
    if config.key.trim().is_empty() {
        return Err("还没配置 AI，请先到设置里填好".into());
    }
    let endpoint = model_endpoint(root).map_err(|e| {
        eprintln!("[lunac-translate] 解析 AI 端点失败: {e}");
        "还没配置 AI，请先到设置里填好".to_string()
    })?;
    let prompt = format!(
        "把下面这段{src}翻译成{dst}。只输出译文本身，不要解释、不要加引号、不要重复原文。\n\n{text}",
        src = lang_name(from),
        dst = lang_name(to),
    );
    let body = serde_json::json!({
        "model": config.model,
        "max_tokens": AI_MAX_TOKENS,
        "stream": false,
        "messages": [{ "role": "user", "content": prompt }],
    });
    let resp = client
        .post(&endpoint)
        .header("authorization", format!("Bearer {}", config.key))
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| {
            eprintln!("[lunac-translate] 模型请求发送失败: {e}");
            "AI 暂时连不上，稍后再试".to_string()
        })?;
    let status = resp.status();
    let raw = resp.text().unwrap_or_default();
    if !status.is_success() {
        // 原始错误只进日志，界面只留「用户能做什么」（v1 预检 #40 ③）。
        eprintln!(
            "[lunac-translate] 模型请求失败（HTTP {status}）: {}",
            truncate_chars(&raw, 400)
        );
        return Err(match status.as_u16() {
            401 | 403 => "AI 凭据无效，请到设置里检查".to_string(),
            404 => "AI 端点或模型名不对，请到设置里检查".to_string(),
            429 => "请求太频繁，稍后再试".to_string(),
            code if code >= 500 => "AI 服务暂时不可用，稍后再试".to_string(),
            _ => "AI 翻译失败，稍后再试".to_string(),
        });
    }
    let v: Value = serde_json::from_str(&raw).map_err(|e| {
        eprintln!("[lunac-translate] 模型响应不是合法 JSON: {e}");
        "AI 返回的内容看不懂，稍后再试".to_string()
    })?;
    let out = v
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    let out = out.trim().to_string();
    if out.is_empty() {
        return Err("模型没有返回译文".into());
    }
    Ok(out)
}

// ── 对外入口（C ABI 在 lib.rs 里薄薄一层）───────────────────────────

/// 词典底座查询（**不花钱**）：缓存 → 有道词条 + MyMemory 主译文。
///
/// 只对空输入 / 超长返回 Err；网络故障一律降级成 `source="none"`（v1 同口径）。
pub fn lookup(
    root: &Path,
    text: &str,
    from: &str,
    to: &str,
) -> Result<TranslateResult, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("没有要翻译的内容".into());
    }
    if text.chars().count() > DICT_MAX_CHARS {
        return Err(format!(
            "文本过长（上限 {DICT_MAX_CHARS} 字），请分段或改用 AI 翻译"
        ));
    }
    let from = resolve_from(from, &text);
    let to = to.trim().to_string();

    if let Some(hit) = cache_get(root, &from, &to, &text) {
        return Ok(hit);
    }

    let client = http()?;
    let mut res = TranslateResult::none(&text, &from, &to);

    // ① 词条详情（音标 / 释义 / 例句）
    let (phonetic, explains, examples) = youdao_detail(&client, &text);
    res.phonetic = phonetic;
    res.explains = explains;
    res.examples = examples;

    // ② 主译文；MyMemory 答不出时退到有道第一条 trs。
    res.translation = mymemory_translate(&client, &text, &from, &to)
        .or_else(|| youdao_first_trs(&res.explains))
        .unwrap_or_default();

    if res.is_empty() {
        return Ok(res); // source 仍是 "none"
    }
    res.source = "dict".into();
    cache_put(root, &res);
    Ok(res)
}

/// 模型补漏（**花钱**）：只在用户点按钮时调用；已有 AI 结果才直接复用缓存。
pub fn ai(
    root: &Path,
    text: &str,
    from: &str,
    to: &str,
) -> Result<TranslateResult, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("没有要翻译的内容".into());
    }
    if text.chars().count() > AI_MAX_CHARS {
        return Err(format!("文本过长（上限 {AI_MAX_CHARS} 字），请分段翻译"));
    }
    let from = resolve_from(from, &text);
    let to = to.trim().to_string();

    if let Some(hit) = cache_get(root, &from, &to, &text) {
        if hit.source == "ai" {
            return Ok(hit);
        }
    }

    let client = http()?;
    eprintln!(
        "[lunac-translate] 模型补漏：{} 字，{} → {}",
        text.chars().count(),
        from,
        to
    );
    let translation = model_translate(root, &client, &text, &from, &to)?;
    let mut res = TranslateResult::none(&text, &from, &to);
    res.source = "ai".into();
    res.translation = translation;
    cache_put(root, &res);
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-translate-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    #[test]
    fn detect_lang_and_resolve_from_align_with_v1() {
        assert_eq!(detect_lang("hello world"), "en");
        assert_eq!(detect_lang("你好，今天天气不错"), "zh-CN");
        assert_eq!(detect_lang("こんにちは"), "zh-CN");
        assert_eq!(detect_lang("안녕하세요"), "zh-CN");
        assert_eq!(resolve_from("ja", "hello"), "ja");
        assert_eq!(resolve_from("", "hello"), "en");
        assert_eq!(resolve_from("auto", "你好"), "zh-CN");
        assert_eq!(resolve_from(" AUTO ", "hello"), "en");
        assert_eq!(lang_name("zh-TW"), "简体中文");
        assert_eq!(lang_name("fr"), "fr");
    }

    #[test]
    fn cache_key_is_scoped_by_language_pair() {
        assert_ne!(
            cache_key("en", "zh-CN", "hello"),
            cache_key("zh-CN", "en", "hello")
        );
        assert_eq!(
            cache_key("en", "zh-CN", "hi"),
            cache_key("en", "zh-CN", "hi")
        );
    }

    #[test]
    fn parse_mymemory_accepts_translation_and_rejects_echo_or_quota() {
        let ok = r#"{"responseData":{"translatedText":"你好"},"responseStatus":200}"#;
        assert_eq!(parse_mymemory(ok, "hello").as_deref(), Some("你好"));

        // 译文 ≈ 原文（专有名词）不算结果。
        let echo = r#"{"responseData":{"translatedText":"Hello"},"responseStatus":200}"#;
        assert_eq!(parse_mymemory(echo, "hello"), None);

        // 额度用尽时 HTTP 仍是 200，但 responseStatus = 403。
        let quota = r#"{"responseData":{"translatedText":"你好"},"responseStatus":403}"#;
        assert_eq!(parse_mymemory(quota, "hello"), None);

        assert_eq!(parse_mymemory("{not json", "hello"), None);
    }

    #[test]
    fn parse_youdao_extracts_phonetic_explains_and_caps_examples() {
        let raw = r#"{
            "ec": {"word": [{
                "ukphone": "həˈləʊ",
                "usphone": "həˈloʊ",
                "trs": [
                    {"tr": [{"l": {"i": ["int. 喂，你好"]}}]},
                    {"tr": [{"l": {"i": ["int. 喂，你好"]}}]}
                ]
            }]},
            "blng_sents_part": {"sentence-pair": [
                {"sentence": "Hello, world.", "sentence-translation": "你好，世界。"},
                {"sentence": "Hello there.", "sentence-translation": "你好。"},
                {"sentence": "Say hello.", "sentence-translation": "打个招呼。"},
                {"sentence": "Extra one.", "sentence-translation": "多余的一条。"}
            ]}
        }"#;
        let (phonetic, explains, examples) = parse_youdao(raw);
        assert_eq!(phonetic, "英 həˈləʊ  美 həˈloʊ");
        assert_eq!(explains, vec!["int. 喂，你好"]);
        assert_eq!(examples.len(), MAX_EXAMPLES);
        assert_eq!(examples[0].src, "Hello, world.");
        assert_eq!(examples[0].dst, "你好，世界。");
    }

    #[test]
    fn parse_youdao_handles_chinese_entries_and_bad_json() {
        let raw = r##"{
            "ce": {"word": [{"trs": [
                {"tr": [{"l": {"i": ["", {"#text": "你好"}]}}]},
                {"tr": [{"l": {"i": ["你好"]}}]}
            ]}]}
        }"##;
        let (phonetic, explains, examples) = parse_youdao(raw);
        assert!(phonetic.is_empty());
        assert_eq!(explains, vec!["你好"]);
        assert!(examples.is_empty());

        // 坏 JSON / 空对象都只返回空，不 panic。
        let (p, e, x) = parse_youdao("{not json");
        assert!(p.is_empty() && e.is_empty() && x.is_empty());
        let (p, e, x) = parse_youdao("{}");
        assert!(p.is_empty() && e.is_empty() && x.is_empty());
    }

    #[test]
    fn trs_line_accepts_both_string_and_object_items() {
        let a: Value = serde_json::json!({ "l": { "i": ["int. 喂，你好"] } });
        assert_eq!(trs_line(&a).as_deref(), Some("int. 喂，你好"));
        let b: Value = serde_json::json!({ "l": { "i": ["", { "#text": "HELLO" }, " ", { "#text": "WORLD" }] } });
        assert_eq!(trs_line(&b).as_deref(), Some("HELLO WORLD"));
        let c: Value = serde_json::json!({ "l": { "i": [] } });
        assert_eq!(trs_line(&c), None);
        assert_eq!(trs_line(&serde_json::json!({})), None);
    }

    #[test]
    fn empty_result_keeps_source_none_and_short_body_is_challenge() {
        let r = TranslateResult::none("zzzz", "en", "zh-CN");
        assert!(r.is_empty());
        assert_eq!(r.source, "none");
        let mut r2 = r.clone();
        r2.translation = "测试".into();
        assert!(!r2.is_empty());
        let mut r3 = r.clone();
        r3.explains = vec!["n. 测试".into()];
        assert!(!r3.is_empty());

        assert!(youdao_body_looks_like_challenge("<html>验证</html>"));
        assert!(!youdao_body_looks_like_challenge(&"x".repeat(512)));
    }

    #[test]
    fn cache_round_trips_in_temp_dir_and_skips_empty_results() {
        let root = temp_root("cache");
        assert!(cache_get(&root, "en", "zh-CN", "hello").is_none());

        let mut res = TranslateResult::none("hello", "en", "zh-CN");
        res.source = "dict".into();
        res.translation = "你好".into();
        res.phonetic = "həˈləʊ".into();
        cache_put(&root, &res);

        let hit = cache_get(&root, "en", "zh-CN", "hello").expect("cache hit");
        assert!(hit.cached);
        assert_eq!(hit.translation, "你好");
        assert_eq!(hit.phonetic, "həˈləʊ");
        // 不同语言对不共用同一份。
        assert!(cache_get(&root, "zh-CN", "en", "hello").is_none());

        // 空结果不落库。
        let empty = TranslateResult::none("zzzz", "en", "zh-CN");
        cache_put(&root, &empty);
        assert!(cache_get(&root, "en", "zh-CN", "zzzz").is_none());

        let _ = std::fs::remove_dir_all(&root);
    }
}
