// src-tauri/src/translate.rs
// 翻译插件的宿主命令 —— 2026-09-29
//
// **为什么这件事必须由宿主做**：前端插件跑在 WebView 里，`tauri.conf.json` 的 CSP 是
// `default-src 'self' https://asset.localhost` —— 不含任何远端域，插件里的 `fetch()`
// 会被直接拦掉（与音乐歌词 / 插件市场索引同一条理由，见 ai-spec §11 规则 67 的第 ⑥ 条）。
//
// ── 三层结构（用户 2026-09-29 定：「词典做底座 + 模型补漏 + 译文存数据库」）────────
//
//   ① **缓存**：`<exe 根>\ModuleData\translate\cache.db` 的 `translations` 表。
//      同一 (源语言, 目标语言, 原文) 只查一次外部接口 —— 用户的原话是「避免二次翻译」。
//      缓存命中的结果里 `cached=true`，前端要如实标出来（它可能是很久以前的译文）。
//   ② **词典底座**：两家**免 key** 的公开接口，都不需要用户申请凭据：
//        · MyMemory（`api.mymemory.translated.net`）—— 给**主译文**，整句/短语/单词都行；
//        · 有道词典 jsonapi（`dict.youdao.com/jsonapi`）—— 给**词条详情**：音标、释义、
//          双语例句；实测**整句**查询时它的 `ec.word[0].trs` 也是译文候选，故兼作兜底。
//   ③ **模型补漏**：上面两层都给不出结果时，**由用户点按钮**才发起（花钱的动作不由系统
//      自作主张）。直连 `{agent_endpoint}/v1/messages`（与 core-agent 同一套形状：
//      `authorization: Bearer` + `anthropic-version`，见 ai-spec §3.5 的 stdin/stdout 契约）。
//
// ── 冷启动实测（2026-09-29，本机**直连**、不走系统代理）──────────────────────────
//
//   | 接口 | 结果 |
//   |---|---|
//   | `translate.googleapis.com/translate_a/single`（免 key 的 Google） | **不通**（curl 000） |
//   | `api.mymemory.translated.net/get` | 200；`hello`→你好、`你好，今天天气不错`→英文、139 字整段无截断 |
//   | `dict.youdao.com/jsonapi` | 200（`hello` 54 KB / `give up` 21 KB 词条详情） |
//
//   ⇒ Google 那条**故意不实现**：本机实测连不上，写成「首选 + 失败降级」只会让每次查询
//   先白等一个超时。另：MyMemory 的 `langpair=Autodetect|zh-CN` 实测**不可靠**
//   （`hello world` 被原样返回），所以源语言一律由本地判据（见 `detect_lang`）决定。
//
// ── 两条纪律 ─────────────────────────────────────────────────────
//
//   · **非官方接口一律降级不报错**：这两家都没有承诺、也没有 SLA。抓不到就返回
//     `source="none"`（前端据此显示「用 AI 翻译」），绝不把网络错误当成「查无此词」，
//     也绝不用异常打断面板。HTTP 200 + 正文极短 = 被反爬（预检 #41 的判据），同样按降级处理。
//   · **阻塞纪律（code-rules 预检 #35）**：命令体里有 `reqwest::blocking` ⇒ 一律
//     `async` 薄壳 + `run_blocking`，绝不留在主线程上冻窗口。

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::run_blocking;

// ── 常量 ──────────────────────────────────────────────────────────

/// MyMemory —— 免费、无需 key 的翻译记忆库。匿名额度按天计（几千字符量级），
/// 超额会返回 `responseStatus: 403`（那一条按降级处理，不弹错）。
const MYMEMORY: &str = "https://api.mymemory.translated.net/get";
/// 有道词典 jsonapi —— **非官方只读接口**（同网易云歌词的地位：能用就用）。
const YOUDAO: &str = "https://dict.youdao.com/jsonapi";
/// 有道认这个头（预检 #41：结果页类接口要把 `User-Agent` / `Accept` / `Accept-Language`
/// 凑齐，必要时再补 `Referer`；只补一个往往仍是降级空壳）。
const YOUDAO_REFERER: &str = "https://dict.youdao.com/";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

/// 词典/翻译接口的单次超时。这两家都是「答不出来就会很快答不出来」，给 10s 足够；
/// 再长只会让面板看着像卡死。
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// 送进**词典**的文本上限。MyMemory 匿名额度按字符数算，长文既贵又译不好 ——
/// 超过就如实让用户走「用 AI 翻译」（那条上限另算，见 `AI_MAX_CHARS`）。
const DICT_MAX_CHARS: usize = 1000;
/// 送进**模型**的文本上限。与摘要压缩的输入闸同一个量级，兜住「用户往输入框里灌一本书」。
const AI_MAX_CHARS: usize = 20000;
/// 模型输出的上限。译文不该比原文长太多，1024 够一段话；超了就是模型跑偏。
const AI_MAX_TOKENS: u32 = 2048;
/// 返回给前端的例句条数上限（面板是一列内容，多了没人看）。
const MAX_EXAMPLES: usize = 3;
/// 返回给前端的释义行上限。
const MAX_EXPLAINS: usize = 6;

// ── HTTP client（进程级复用）──────────────────────────────────────
//
// `Client::builder().build()` 每次都是**全新的连接池**：翻译面板每查一个词都要新建一次，
// 手一快就是几十个连接池。与 music.rs 同一条理由（ai-spec §4.6「轮询要快」）。

static HTTP_CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();

fn http() -> Result<reqwest::blocking::Client, String> {
    HTTP_CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .map_err(|e| format!("HTTP client: {e}"))
        })
        .clone()
}

// ── 结果形状（前端直接渲染这一份）─────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct TranslateExample {
    pub src: String,
    pub dst: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TranslateResult {
    /// 原文（**归一化之后**的：已 trim、已截断；前端拿它对账/复制）
    pub text: String,
    /// 实际使用的源语言（`auto` 已解析成具体值）—— 面板要显示「识别为英语」
    pub from: String,
    pub to: String,
    /// `dict` = 词典底座给出的；`ai` = 模型补漏给出的；`none` = 两层都没有结果
    pub source: String,
    /// 是否直接来自本地缓存（**没碰网络**）—— 面板要如实标出来
    pub cached: bool,
    /// 主译文。`source="none"` 时为空串（不是 `—`：空串表示「没有」，占位符由前端决定）
    pub translation: String,
    /// 音标（只有词条才有；有道的英美两个音标拼在一起，空 = 没有）
    pub phonetic: String,
    /// 释义 / 译文候选行
    pub explains: Vec<String>,
    /// 双语例句
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

    /// 这一份有没有可展示的内容（全空 = 词典查不到，前端据此提示走 AI）
    fn is_empty(&self) -> bool {
        self.translation.trim().is_empty() && self.explains.is_empty() && self.examples.is_empty()
    }
}

// ── 译文缓存（SQLite）─────────────────────────────────────────────
//
// 落点 `<ModuleData>\translate\cache.db`：`ModuleData` 是业务数据的家（会话库在
// `ModuleData\history\chat.db`），**不塞进 chat.db** —— 那是「对话历史」，把词条缓存混进去
// 会让「history 里到底存了什么」变得说不清。
//
// 建表全部 `IF NOT EXISTS` ⇒ 幂等，每次打开都跑一遍也不怕，与 `chat_db.rs` 同一套做法
// （省掉 `PRAGMA user_version` 版本簿记，现在只有 v1，簿记是纯负担）。

fn cache_path() -> PathBuf {
    crate::storage::module_data_dir().join("translate").join("cache.db")
}

fn open_cache() -> Result<Connection, String> {
    let path = cache_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建译文缓存目录失败: {e}"))?;
    }
    let conn = Connection::open(&path).map_err(|e| format!("打开译文缓存失败: {e}"))?;
    // 与 chat_db.rs 同款：WAL 让读写不互斥；pragma_update 遇返回行的语句会报
    // ExecuteReturnedResults，故用 execute_batch（否则 WAL 会被静默地设不上）。
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

/// 缓存键 = 「源语言 + 目标语言 + 原文」。**不做大小写 / 全半角归一**：
/// 归一会让 `Hello` 与 `hello` 共用一份译文，而它们的词典释义常有差别（有道自己就分条），
/// 省下的那点空间不值一次「查的词和返回的词不是一个」的困惑。
fn cache_key(from: &str, to: &str, text: &str) -> String {
    format!("{from}\u{1}{to}\u{1}{text}")
}

fn cache_get(from: &str, to: &str, text: &str) -> Option<TranslateResult> {
    let conn = open_cache().ok()?;
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

/// 只缓存**有内容**的结果：把「查不到」也写进去的话，一个还没收录的词会被永久钉死成
/// 查不到 —— 而那正是下次可能查到的那个词。
fn cache_put(res: &TranslateResult) {
    if res.is_empty() {
        return;
    }
    let Ok(conn) = open_cache() else { return };
    let Ok(payload) = serde_json::to_string(res) else { return };
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

// ── 语言判定 ──────────────────────────────────────────────────────
//
// 为什么自己判而不用 MyMemory 的 `Autodetect`：**实测不可靠**（2026-09-29：`hello world`
// 经 `langpair=Autodetect|zh-CN` 被原样返回，等于没翻译）。判据只服务最常见的一种情形
// ——「中日韩 vs 其余」，够用且完全确定（没有网络、没有额外依赖）。

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

/// `auto` → 具体语言值。含 CJK 字符就算中文（本面板的主要用法是「中文 ↔ 英文」，
/// 日/韩按中文那一侧的接口查也一样能拿到结果，比误判成 `en` 好）。
fn detect_lang(text: &str) -> &'static str {
    if text.chars().any(is_cjk) {
        "zh-CN"
    } else {
        "en"
    }
}

fn resolve_from(from: &str, text: &str) -> String {
    let f = from.trim();
    if f.is_empty() || f.eq_ignore_ascii_case("auto") {
        detect_lang(text).to_string()
    } else {
        f.to_string()
    }
}

// ── 词典底座：MyMemory（主译文）────────────────────────────────────

/// MyMemory：`{responseData: {translatedText}}`。
/// 失败 / 超额（`responseStatus != 200`）/ 结果等于原文（等于没译）一律返回 `None`。
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
        .map_err(|e| crate::log::warn(format!("[translate] MyMemory 请求失败: {e}")))
        .ok()?;
    let status = resp.status();
    let raw = resp.text().ok()?;
    if !status.is_success() {
        crate::log::warn(format!(
            "[translate] MyMemory 返回 {status}: {}",
            crate::log::truncate_chars(&raw, 200)
        ));
        return None;
    }
    let v: Value = serde_json::from_str(&raw).ok()?;
    // `responseStatus` 在额度用尽时是 403（HTTP 仍是 200）—— 单独判一下，日志里能看出区别
    let inner = v.get("responseStatus").and_then(Value::as_i64).unwrap_or(200);
    if inner != 200 {
        crate::log::warn(format!("[translate] MyMemory 拒绝：responseStatus={inner}"));
        return None;
    }
    let out = v
        .pointer("/responseData/translatedText")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    // 有些条目在翻译记忆里就是「原文≈译文」（比如专有名词），那不算结果
    if out.is_empty() || out.eq_ignore_ascii_case(text.trim()) {
        crate::log::info(format!("[translate] MyMemory 未给出有效译文（{text:.20}）"));
        return None;
    }
    Some(out)
}

// ── 词典底座：有道词条详情（音标 / 释义 / 例句）────────────────────

/// 有道 `trs` 里的 `l.i` 既可能是字符串，也可能是 `{"#text": …}`（带高亮链接的形态，
/// 中文词条就是这样）。两种都取，取不到就跳过 —— 别让一个形状差异把整条释义丢掉。
fn trs_line(node: &Value) -> Option<String> {
    let items = node.pointer("/l/i")?;
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

fn youdao_detail(client: &reqwest::blocking::Client, text: &str) -> (String, Vec<String>, Vec<TranslateExample>) {
    let mut phonetic = String::new();
    let mut explains: Vec<String> = Vec::new();
    let mut examples: Vec<TranslateExample> = Vec::new();

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
            crate::log::warn(format!("[translate] 有道请求失败: {e}"));
            return (phonetic, explains, examples);
        }
    };
    let status = resp.status();
    let raw = match resp.text() {
        Ok(t) => t,
        Err(e) => {
            crate::log::warn(format!("[translate] 有道响应读取失败: {e}"));
            return (phonetic, explains, examples);
        }
    };
    // 预检 #41：HTTP 200 但正文极短 = 被反爬（对方给的是验证页），**别去改解析**。
    // 这里是纯降级分支：返回空，由上层决定要不要走模型。
    if !status.is_success() {
        crate::log::warn(format!(
            "[translate] 有道返回 {status}: {}",
            crate::log::truncate_chars(&raw, 200)
        ));
        return (phonetic, explains, examples);
    }
    if raw.len() < 512 {
        crate::log::warn(format!(
            "[translate] 有道正文仅 {} 字节（疑似反爬验证页），按无结果处理",
            raw.len()
        ));
        return (phonetic, explains, examples);
    }

    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        crate::log::warn("[translate] 有道响应不是合法 JSON");
        return (phonetic, explains, examples);
    };

    // 音标：英 / 美两条都取（只取一个有偏向，用户看不出缺的是哪一边）
    if let Some(w) = v.pointer("/ec/word/0") {
        let uk = w.get("ukphone").and_then(Value::as_str).unwrap_or("").trim();
        let us = w.get("usphone").and_then(Value::as_str).unwrap_or("").trim();
        phonetic = match (uk.is_empty(), us.is_empty()) {
            (false, false) if uk != us => format!("英 {uk}  美 {us}"),
            (false, _) => uk.to_string(),
            (_, false) => us.to_string(),
            _ => String::new(),
        };
        if let Some(arr) = w.get("trs").and_then(Value::as_array) {
            for item in arr {
                if let Some(line) = item.get("tr").and_then(Value::as_array).and_then(|a| a.first()).and_then(trs_line) {
                    // 去重：有道同一个词会在不同词性下重复同一条释义
                    if !explains.contains(&line) {
                        explains.push(line);
                    }
                }
            }
        }
    }
    // 中文词条：释义在 `ce.word[0].trs`（`ec` 只对英文词条有）
    if explains.is_empty() {
        if let Some(arr) = v.pointer("/ce/word/0/trs").and_then(Value::as_array) {
            for item in arr {
                // 中文词条多一层：`tr[0].l` 自己就是 {i:…,#tran:…}，故先取 `l` 再进 trs_line
                let Some(l) = item.get("tr").and_then(Value::as_array).and_then(|a| a.first()).and_then(|t| t.get("l")) else {
                    continue;
                };
                if let Some(line) = trs_line(l) {
                    if !explains.contains(&line) {
                        explains.push(line);
                    }
                }
            }
        }
    }
    explains.truncate(MAX_EXPLAINS);

    // 双语例句：`blng_sents_part.sentence-pair[]`（`sentence` 原文 / `sentence-translation` 译文）
    if let Some(pairs) = v.pointer("/blng_sents_part/sentence-pair").and_then(Value::as_array) {
        for p in pairs {
            let src = p.get("sentence").and_then(Value::as_str).unwrap_or("").trim();
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

/// 有道 `ec.word[0].trs` 的第一条 —— 只当 MyMemory 没结果时的**主译文兜底**用。
/// 实测整句查询（`how are you doing today`）它的 `trs` 就是「你今天过得怎么样」这样的
/// 纯译文；词条查询时它是释义（带 `int.` 这样的词性前缀），那种情况下当译文显示并不理想，
/// 但比空着强 —— 而且前端会同时把释义列出来，用户看得出这是什么。
fn youdao_first_trs(explains: &[String]) -> Option<String> {
    explains.first().cloned()
}

// ── 模型补漏：直连 /v1/messages ───────────────────────────────────

/// 模型端点：优先用**已经算好并注入**的那一份（`start_cli` / `start_agent_http` 都写过），
/// 没有就按同一套规则现算 —— 绝不能自己拼一个 URL，否则「agent 能连上、翻译连不上」
/// 会变成两处各执一词（`agent_endpoint` 的 `/anthropic` 尾巴就是这么来的）。
fn model_endpoint() -> Result<String, String> {
    if let Ok(base) = std::env::var("LUNAC_AGENT_BASE_URL") {
        let b = base.trim().trim_end_matches('/');
        if !b.is_empty() {
            return Ok(format!("{b}/v1/messages"));
        }
    }
    let (api_url, _, _) = crate::commands::ai_credentials()?;
    let base = crate::commands::agent_endpoint(&api_url, std::env::var("AI_AGENT_URL").ok().as_deref());
    Ok(format!("{}/v1/messages", base.trim().trim_end_matches('/')))
}

/// 让模型翻译一段文本（**非流式**，一次往返）。返回译文正文。
///
/// 提示词只做一件事：**只要译文**。温度不给（端点默认即可），不传 tools、不传 thinking
/// —— 这是一个「翻译」请求，不是一次对话，任何多余的字段都只是多一种 400 的成因。
fn model_translate(
    client: &reqwest::blocking::Client,
    text: &str,
    from: &str,
    to: &str,
) -> Result<String, String> {
    // 取凭据失败 = 用户还没配过 AI ⇒ 界面只说「去设置里填」（原始原因进日志）
    let (_, token, model) = crate::commands::ai_credentials().map_err(|e| {
        crate::log::warn(format!("[translate] 取 AI 凭据失败: {e}"));
        "还没配置 AI，请先到设置里填好".to_string()
    })?;
    if token.trim().is_empty() {
        return Err("还没配置 AI，请先到设置里填好".into());
    }
    let endpoint = model_endpoint().map_err(|e| {
        crate::log::warn(format!("[translate] 解析 AI 端点失败: {e}"));
        "还没配置 AI，请先到设置里填好".to_string()
    })?;
    // 返回 `String` 而不是 `&str`：未知语言码要原样透传（用户自填的码不能丢），
    // 而那几个已知分支是字面量 —— 混在一个 `&str` 闭包里会被推断成 `&'static str`，编译不过。
    let lang_name = |c: &str| -> String {
        match c {
            "zh-CN" | "zh" | "zh-TW" => "简体中文".to_string(),
            "en" => "英语".to_string(),
            "ja" => "日语".to_string(),
            "ko" => "韩语".to_string(),
            other => other.to_string(),
        }
    };
    let prompt = format!(
        "把下面这段{src}翻译成{dst}。只输出译文本身，不要解释、不要加引号、不要重复原文。\n\n{text}",
        src = lang_name(from),
        dst = lang_name(to),
    );
    let body = serde_json::json!({
        "model": model,
        "max_tokens": AI_MAX_TOKENS,
        "stream": false,
        "messages": [{ "role": "user", "content": prompt }],
    });
    let resp = client
        .post(&endpoint)
        .header("authorization", format!("Bearer {token}"))
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| {
            crate::log::warn(format!("[translate] 模型请求发送失败: {e}"));
            "AI 暂时连不上，稍后再试".to_string()
        })?;
    let status = resp.status();
    let raw = resp.text().unwrap_or_default();
    if !status.is_success() {
        // 原始错误（状态码 / 响应体）**只进日志**，界面只留「用户能做什么」——
        // code-rules 预检 #40 ③：状态码、URL、JSON 响应体不进主界面。
        crate::log::warn(format!(
            "[translate] 模型请求失败（HTTP {status}）: {}",
            crate::log::truncate_chars(&raw, 400)
        ));
        let code = status.as_u16();
        return Err(match code {
            401 | 403 => "AI 凭据无效，请到设置里检查".to_string(),
            404 => "AI 端点或模型名不对，请到设置里检查".to_string(),
            429 => "请求太频繁，稍后再试".to_string(),
            _ if code >= 500 => "AI 服务暂时不可用，稍后再试".to_string(),
            _ => "AI 翻译失败，稍后再试".to_string(),
        });
    }
    let v: Value = serde_json::from_str(&raw).map_err(|e| {
        crate::log::warn(format!("[translate] 模型响应不是合法 JSON: {e}"));
        "AI 返回的内容看不懂，稍后再试".to_string()
    })?;
    // Anthropic 形状：`content: [{type:"text", text:"…"}]`（可能有多块，全拼上）
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

// ── 命令 ──────────────────────────────────────────────────────────

/// 词典底座查询（**不花钱**）：缓存 → MyMemory + 有道。两层都没有结果就返回
/// `source="none"`，由前端提示用户走「用 AI 翻译」。
///
/// **永不返回 Err**（除了空输入）：网络故障、被反爬、额度用尽都只是「这次没查到」，
/// 不该在面板上变成一行红色错误 —— 那会把「对方今天不通」说成「你这个词有问题」。
#[tauri::command]
pub async fn translate_lookup(text: String, from: String, to: String) -> Result<TranslateResult, String> {
    run_blocking(move || {
        let text = text.trim().to_string();
        if text.is_empty() {
            return Err("没有要翻译的内容".into());
        }
        if text.chars().count() > DICT_MAX_CHARS {
            return Err(format!("文本过长（上限 {DICT_MAX_CHARS} 字），请分段或改用 AI 翻译"));
        }
        let from = resolve_from(&from, &text);
        let to = to.trim().to_string();

        if let Some(hit) = cache_get(&from, &to, &text) {
            return Ok(hit);
        }

        let client = http()?;
        let mut res = TranslateResult::none(&text, &from, &to);

        // ① 词条详情（音标 / 释义 / 例句）
        let (phonetic, explains, examples) = youdao_detail(&client, &text);
        res.phonetic = phonetic;
        res.explains = explains;
        res.examples = examples;

        // ② 主译文；MyMemory 答不出时退到有道的第一条 trs
        res.translation = mymemory_translate(&client, &text, &from, &to)
            .or_else(|| youdao_first_trs(&res.explains))
            .unwrap_or_default();

        if res.is_empty() {
            return Ok(res); // source 仍是 "none"
        }
        res.source = "dict".into();
        cache_put(&res);
        Ok(res)
    })
    .await
}

/// 模型补漏（**花钱**）：只在用户点按钮时调用。结果同样落缓存 ——
/// 同一句问第二次不该再付一次钱（用户原话：「译文存数据库，避免二次翻译」）。
#[tauri::command]
pub async fn translate_ai(text: String, from: String, to: String) -> Result<TranslateResult, String> {
    run_blocking(move || {
        let text = text.trim().to_string();
        if text.is_empty() {
            return Err("没有要翻译的内容".into());
        }
        if text.chars().count() > AI_MAX_CHARS {
            return Err(format!("文本过长（上限 {AI_MAX_CHARS} 字），请分段翻译"));
        }
        let from = resolve_from(&from, &text);
        let to = to.trim().to_string();

        // 缓存里已经有**词典型的**结果时也照样允许走模型：用户点这个按钮就是要更好的译文，
        // 拿旧结果把他挡回去等于按钮没反应。只在已有 **AI** 结果时直接复用。
        if let Some(hit) = cache_get(&from, &to, &text) {
            if hit.source == "ai" {
                return Ok(hit);
            }
        }

        let client = http()?;
        crate::log::info(format!(
            "[translate] 模型补漏：{} 字，{} → {}",
            text.chars().count(),
            from,
            to
        ));
        let translation = model_translate(&client, &text, &from, &to)?;
        let mut res = TranslateResult::none(&text, &from, &to);
        res.source = "ai".into();
        res.translation = translation;
        cache_put(&res);
        Ok(res)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_lang_separates_cjk_from_latin() {
        assert_eq!(detect_lang("hello world"), "en");
        assert_eq!(detect_lang("你好，今天天气不错"), "zh-CN");
        assert_eq!(detect_lang("こんにちは"), "zh-CN");
        assert_eq!(detect_lang("안녕하세요"), "zh-CN");
        // 「auto」之外的值原样透传（用户显式选了目标/源语言就别再猜）
        assert_eq!(resolve_from("ja", "hello"), "ja");
        assert_eq!(resolve_from("", "hello"), "en");
        assert_eq!(resolve_from("auto", "你好"), "zh-CN");
    }

    #[test]
    fn cache_key_is_scoped_by_language_pair() {
        // 同一句「hello」在 en→zh 与 zh→en 下必须是两条（否则会把译文当原文查回来）
        assert_ne!(cache_key("en", "zh-CN", "hello"), cache_key("zh-CN", "en", "hello"));
        assert_eq!(cache_key("en", "zh-CN", "hi"), cache_key("en", "zh-CN", "hi"));
    }

    #[test]
    fn trs_line_accepts_both_string_and_object_items() {
        // 英文词条形态：`i` 里是纯字符串
        let a: Value = serde_json::json!({ "l": { "i": ["int. 喂，你好"] } });
        assert_eq!(trs_line(&a).as_deref(), Some("int. 喂，你好"));
        // 中文词条形态：`i` 里是带高亮的对象（`#text` 才是正文）
        let b: Value = serde_json::json!({ "l": { "i": ["", { "#text": "HELLO" }, " ", { "#text": "WORLD" }] } });
        assert_eq!(trs_line(&b).as_deref(), Some("HELLO WORLD"));
        // 空 / 形状不对 ⇒ None（不许返回空串，那会在面板上留下一个空行）
        let c: Value = serde_json::json!({ "l": { "i": [] } });
        assert_eq!(trs_line(&c), None);
        assert_eq!(trs_line(&serde_json::json!({})), None);
    }

    #[test]
    fn empty_result_is_not_cached_but_keeps_source_none() {
        let r = TranslateResult::none("zzzz", "en", "zh-CN");
        assert!(r.is_empty());
        assert_eq!(r.source, "none");
        // 有主译文就不算空
        let mut r2 = r.clone();
        r2.translation = "测试".into();
        assert!(!r2.is_empty());
        // 只有释义没有译文也不算空（词条场景常见）
        let mut r3 = r.clone();
        r3.explains = vec!["n. 测试".into()];
        assert!(!r3.is_empty());
    }
}
