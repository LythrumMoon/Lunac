// src/resource_hub_tools.rs
//! **资源库来源的 AI 工具**（A5，2026-10-18）。
//!
//! 四件：`ResourceHubSearch` / `SourceValidate` / `SourceInstall` / `SourceDiagnose`。
//!
//! ## 单一真相源（硬约束）
//!
//! RSP 的**语义**校验在 `resource-hub/packages/rh-source-sdk`（TS）—— 这里只做
//! **写入前的形状 / 边界检查**（id 安全、必备字段、规则段形状），不复制 TS 的归一化逻辑。
//! 插件加载来源时仍会用 TS SDK 再做一次完整校验；坏条目会被插件丢弃。
//!
//! ## 与宿主的分工
//!
//! · `ResourceHubSearch` / `SourceInstall` **走桥**：来源目录在宿主
//!   （`<exe 根>\ModuleData\resource-hub\sources`），agent 不自己拼路径；
//! · `SourceValidate` 只读一个本地 JSON 文件，不联网、无副作用；
//! · `SourceDiagnose` 会向来源站点发一次 GET/POST（与 `WebFetch` 同一条外部数据出口判据，
//!   故在 `tools::needs_approval` 里**要问**）。
//!
//! ## 边界
//!
//! · `SourceInstall` 是写类：只读档 / 计划相位由 `tools::write_blocked` 拒；
//! · 安装**不执行**规则；删来源只在插件 UI 里（AI 没有删除通道）。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

/// 单个来源文件上限（与宿主 `resource_hub.rs` 同口径）。
const MAX_SOURCE_BYTES: u64 = 1024 * 1024;
/// 诊断时最多读响应体多少字节（只看头部特征，不把整站拖回来）。
const MAX_DIAGNOSE_BYTES: usize = 256 * 1024;
/// 诊断超时。
const DIAGNOSE_TIMEOUT: Duration = Duration::from_secs(12);
/// 搜索默认 / 上限条数。
const SEARCH_DEFAULT_LIMIT: usize = 10;
const SEARCH_MAX_LIMIT: usize = 30;

fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

fn is_http_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// 规则段（与 `DeclarativeRule` 的键一一对应）。
const SECTIONS: [&str; 5] = ["search", "chapters", "pages", "content", "play"];

/// 形状校验的返回（给模型看的 JSON）。
fn report(
    ok: bool,
    errors: Vec<String>,
    warnings: Vec<String>,
    manifest: Option<&Value>,
    sections: &[String],
) -> Value {
    let id = manifest.and_then(|m| m.get("id")).and_then(Value::as_str).unwrap_or("");
    let name = manifest.and_then(|m| m.get("name")).and_then(Value::as_str).unwrap_or("");
    let kind = string_list(manifest.and_then(|m| m.get("kind")));
    let capabilities = string_list(manifest.and_then(|m| m.get("capabilities")));
    json!({
        "ok": ok,
        "id": id,
        "name": name,
        "kind": kind,
        "capabilities": capabilities,
        "sections": sections,
        "errors": errors,
        "warnings": warnings,
    })
}

/// 校验一份 `{ manifest, rule }`。**只判定能静态判定的事实**，不替来源补默认值。
pub fn validate_source(source: &Value) -> Value {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    let manifest = match source.get("manifest").filter(|v| v.is_object()) {
        Some(m) => m,
        None => {
            errors.push("缺少 manifest 对象".into());
            return report(false, errors, warnings, None, &[]);
        }
    };

    let id = manifest.get("id").and_then(Value::as_str).unwrap_or("");
    if !is_safe_id(id) {
        errors.push("manifest.id 只允许 [a-z0-9._-]，≤48 字符，且不能以 . 开头".into());
    }
    if manifest.get("name").and_then(Value::as_str).unwrap_or("").trim().is_empty() {
        errors.push("manifest.name 不能为空".into());
    }
    if manifest.get("version").and_then(Value::as_str).unwrap_or("").trim().is_empty() {
        errors.push("manifest.version 不能为空".into());
    }

    let kind = string_list(manifest.get("kind"));
    if kind.is_empty() {
        errors.push("manifest.kind 至少一项（comic / novel / anime / tv）".into());
    }
    for k in &kind {
        if !matches!(k.as_str(), "comic" | "novel" | "anime" | "tv") {
            errors.push(format!("manifest.kind 不支持：{k}"));
        }
    }

    match manifest.get("engine").and_then(Value::as_str).unwrap_or("") {
        "declarative" => {}
        "" => errors.push("manifest.engine 必填（当前只支持 declarative）".into()),
        other => errors.push(format!(
            "manifest.engine = {other}；当前资源库插件只支持 declarative"
        )),
    }

    let entry = manifest.get("entry").and_then(Value::as_str).unwrap_or("");
    if entry.is_empty() {
        errors.push("manifest.entry 必填（声明式规则通常写 rule.json）".into());
    } else if entry.contains("..")
        || entry.contains('\\')
        || entry.starts_with('/')
        || !entry.to_ascii_lowercase().ends_with(".json")
    {
        errors.push("manifest.entry 必须是来源包内的相对 .json 路径".into());
    }

    let capabilities = string_list(manifest.get("capabilities"));
    if capabilities.is_empty() {
        errors.push("manifest.capabilities 至少一项".into());
    }
    for cap in &capabilities {
        if !matches!(
            cap.as_str(),
            "search" | "explore" | "detail" | "chapters" | "pages" | "content" | "play" | "auth"
        ) {
            errors.push(format!("manifest.capabilities 不支持：{cap}"));
        }
    }

    if let Some(base) = manifest.get("baseUrl") {
        let base = base.as_str().unwrap_or("");
        if !base.is_empty() && !is_http_url(base) {
            errors.push("manifest.baseUrl 必须是 http(s) 地址".into());
        }
    }

    let rule = match source.get("rule").filter(|v| v.is_object()) {
        Some(r) => r,
        None => {
            errors.push("缺少 rule 对象（当前只接受声明式规则）".into());
            return report(false, errors, warnings, Some(manifest), &[]);
        }
    };

    let mut present: Vec<String> = Vec::new();
    for section in SECTIONS {
        let Some(value) = rule.get(section) else {
            continue;
        };
        present.push(section.to_string());
        let Some(obj) = value.as_object() else {
            errors.push(format!("rule.{section} 必须是对象"));
            continue;
        };
        let Some(request) = obj.get("request").filter(|v| v.is_object()) else {
            errors.push(format!("rule.{section}.request 必须是对象"));
            continue;
        };
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        if !matches!(method, "GET" | "POST") {
            errors.push(format!("rule.{section}.request.method 只支持 GET / POST"));
        }
        let url = request.get("url").and_then(Value::as_str).unwrap_or("").trim();
        if url.is_empty() {
            errors.push(format!("rule.{section}.request.url 不能为空"));
        } else if !is_http_url(url) {
            let has_base = manifest
                .get("baseUrl")
                .and_then(Value::as_str)
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !has_base {
                warnings.push(format!(
                    "rule.{section}.request.url 是相对地址，但 manifest.baseUrl 为空"
                ));
            }
        }
        if !obj.get("list").map_or(false, Value::is_object) {
            errors.push(format!("rule.{section}.list 必须是选择器对象"));
        }
        let Some(fields) = obj.get("fields").filter(|v| v.is_object()) else {
            errors.push(format!("rule.{section}.fields 必须是非空对象"));
            continue;
        };
        if fields.as_object().map_or(true, |f| f.is_empty()) {
            errors.push(format!("rule.{section}.fields 不能为空"));
        }
        for (field, spec) in fields.as_object().into_iter().flatten() {
            let selector = spec.get("selector").filter(|v| v.is_object());
            if selector.is_none() {
                errors.push(format!("rule.{section}.fields.{field} 缺少 selector 对象"));
                continue;
            }
            let selector = selector.unwrap();
            let has_one = ["css", "xpath", "jsonPath"]
                .iter()
                .any(|key| selector.get(*key).and_then(Value::as_str).map_or(false, |s| !s.trim().is_empty()));
            if !has_one {
                errors.push(format!(
                    "rule.{section}.fields.{field}.selector 至少要写 css / xpath / jsonPath 之一"
                ));
            }
        }
    }
    if present.is_empty() {
        errors.push("rule 至少要有一段（search / chapters / pages / content / play）".into());
    }

    // 能力声明与规则段的对应关系：只 warn（有些来源用 detail / explore / auth，规则段不表达）。
    for cap in &capabilities {
        if SECTIONS.contains(&cap.as_str()) && !present.iter().any(|s| s == cap) {
            warnings.push(format!("capabilities 声明了 {cap}，但 rule 里没有对应段"));
        }
    }
    for section in &present {
        if !capabilities.iter().any(|c| c == section) {
            warnings.push(format!("rule 有 {section} 段，但 capabilities 未声明 {section}"));
        }
    }

    report(errors.is_empty(), errors, warnings, Some(manifest), &present)
}

fn resolve_path(cwd: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// 读一个来源 JSON 文件；只接受 `{ manifest, rule }` 形状。
pub fn read_source_file(ctx: &crate::tools::Ctx, raw_path: &str) -> Result<Value, String> {
    let path = resolve_path(&ctx.cwd, raw_path);
    crate::tools::guard_read(ctx, &path)?;
    let meta = std::fs::metadata(&path)
        .map_err(|e| format!("读不到来源文件（{}）：{e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("不是文件：{}", path.display()));
    }
    if meta.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "来源文件过大（{} 字节，上限 {MAX_SOURCE_BYTES}）",
            meta.len()
        ));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取来源文件失败（{}）：{e}", path.display()))?;
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|e| format!("来源文件不是合法 JSON（{}）：{e}", path.display()))?;
    if parsed.get("manifest").is_some() && parsed.get("rule").is_some() {
        return Ok(parsed);
    }
    if parsed.get("rsp").is_some() && parsed.get("entries").is_some() {
        return Err(
            "这是聚合仓库索引（registry bundle），不是可安装来源；请让它指向具体的规则文件".into(),
        );
    }
    if parsed.get("rsp").is_some() {
        return Err(
            "这是 RSP 清单，但缺少 rule；请写成 { \"manifest\": {...}, \"rule\": {...} }".into(),
        );
    }
    Err("无法识别：需要 { manifest, rule }；Kazumi 规则请先在资源库插件里导入转换".into())
}

/// `SourceValidate` 的后端。
pub fn validate_file(ctx: &crate::tools::Ctx, raw_path: &str) -> Result<Value, String> {
    let source = read_source_file(ctx, raw_path)?;
    Ok(validate_source(&source))
}

/// `SourceDiagnose` 的后端：对来源做一次**真实请求**并报告可读错误。
pub fn diagnose(ctx: &crate::tools::Ctx, input: &Value) -> Result<Value, String> {
    let raw_path = input
        .get("file_path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if raw_path.is_empty() {
        return Err("缺少 file_path 参数".into());
    }
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("test");

    let source = read_source_file(ctx, raw_path)?;
    let check = validate_source(&source);
    if !check.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(json!({
            "ok": false,
            "stage": "validate",
            "report": check,
        }));
    }
    let manifest = source.get("manifest").cloned().unwrap_or_else(|| json!({}));
    let rule = source.get("rule").cloned().unwrap_or_else(|| json!({}));
    let base_url = manifest
        .get("baseUrl")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    // 选一个请求来诊断：优先 search，其次 chapters / pages；都没有就看 baseUrl。
    let section = ["search", "chapters", "pages"]
        .iter()
        .find(|s| rule.get(*s).is_some())
        .copied();
    let Some(section) = section else {
        let target = if base_url.is_empty() {
            return Ok(json!({
                "ok": false,
                "stage": "request",
                "errors": ["规则没有 search / chapters / pages 段，manifest.baseUrl 也为空，无法诊断"],
            }));
        } else {
            base_url.clone()
        };
        return Ok(fetch_probe(&target, "GET", &[], None, &json!({})));
    };

    let request = rule
        .get(section)
        .and_then(|s| s.get("request"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let method = request.get("method").and_then(Value::as_str).unwrap_or("GET");
    let template = request.get("url").and_then(Value::as_str).unwrap_or("");
    let filled = interpolate_url(template, query);
    let mut target = if is_http_url(&filled) {
        filled
    } else if base_url.is_empty() {
        return Ok(json!({
            "ok": false,
            "stage": "request",
            "errors": ["request.url 是相对地址，但 manifest.baseUrl 为空"],
        }));
    } else {
        format!("{}/{}", base_url.trim_end_matches('/'), filled.trim_start_matches('/'))
    };

    // query 参数：GET 走 URL，POST 走表单（`body` 优先）。
    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Some(map) = request.get("query").and_then(Value::as_object) {
        for (key, value) in map {
            if let Some(text) = value.as_str() {
                pairs.push((key.clone(), interpolate_url(text, query)));
            }
        }
    }
    let body = request
        .get("body")
        .and_then(Value::as_str)
        .map(|s| interpolate_url(s, query));

    if method.eq_ignore_ascii_case("GET") && !pairs.is_empty() {
        let mut joiner = if target.contains('?') { '&' } else { '?' };
        let mut suffix = String::new();
        for (key, value) in &pairs {
            suffix.push(joiner);
            suffix.push_str(&url_encode(key));
            suffix.push('=');
            suffix.push_str(&url_encode(value));
            joiner = '&';
        }
        target.push_str(&suffix);
    }

    let headers = merged_headers(&manifest, &request);
    let uses_html = rule
        .get(section)
        .and_then(|s| s.get("fields"))
        .and_then(Value::as_object)
        .map(|fields| {
            fields.values().any(|spec| {
                let selector = spec.get("selector");
                selector
                    .and_then(|s| s.get("css"))
                    .and_then(Value::as_str)
                    .map_or(false, |s| !s.trim().is_empty())
                    || selector
                        .and_then(|s| s.get("xpath"))
                        .and_then(Value::as_str)
                        .map_or(false, |s| !s.trim().is_empty())
            })
        })
        .unwrap_or(false);
    Ok(fetch_probe(
        &target,
        method,
        &headers,
        body.as_deref(),
        &json!({ "section": section, "usesHtml": uses_html }),
    ))
}

fn interpolate_url(template: &str, query: &str) -> String {
    template
        .replace("@keyword", &url_encode(query))
        .replace("@page", "1")
        .replace("@workUrl", "")
}

fn url_encode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn merged_headers(manifest: &Value, request: &Value) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for source in [
        manifest.get("headers").and_then(Value::as_object),
        request.get("headers").and_then(Value::as_object),
    ]
    .into_iter()
    .flatten()
    {
        for (key, value) in source {
            let Some(value) = value.as_str() else { continue };
            if key.contains('\r')
                || key.contains('\n')
                || value.contains('\r')
                || value.contains('\n')
            {
                continue;
            }
            out.push((key.clone(), value.to_string()));
        }
    }
    out
}

fn fetch_probe(
    url: &str,
    method: &str,
    headers: &[(String, String)],
    body: Option<&str>,
    extra: &Value,
) -> Value {
    let client = match reqwest::blocking::Client::builder()
        .timeout(DIAGNOSE_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return json!({
                "ok": false, "stage": "client", "url": url, "errors": [format!("HTTP 客户端创建失败：{e}")],
            })
        }
    };
    let verb = if method.eq_ignore_ascii_case("POST") {
        reqwest::Method::POST
    } else {
        reqwest::Method::GET
    };
    let mut req = client.request(verb, url);
    let mut has_ua = false;
    for (key, value) in headers {
        if key.eq_ignore_ascii_case("user-agent") {
            has_ua = true;
        }
        req = req.header(key, value);
    }
    if !has_ua {
        req = req.header("User-Agent", "Lunac-ResourceHub-Diagnose/1.0");
    }
    if let Some(body) = body {
        req = req.body(body.to_string());
    }

    let response = match req.send() {
        Ok(r) => r,
        Err(e) => {
            return json!({
                "ok": false,
                "stage": "network",
                "url": url,
                "errors": [format!("请求失败：{e}")],
                "hint": "检查 baseUrl / 网络 / TLS；本机或私网地址会被宿主拒绝。",
                "extra": extra,
            })
        }
    };

    let status = response.status();
    let final_url = response.url().to_string();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let mut buf: Vec<u8> = Vec::new();
    let _ = response.take(MAX_DIAGNOSE_BYTES as u64).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let looks_html = {
        let head = text.trim_start_matches('\u{feff}').trim_start().to_ascii_lowercase();
        content_type.to_ascii_lowercase().contains("html")
            || head.starts_with("<!doctype")
            || head.starts_with("<html")
    };
    let looks_json = content_type.to_ascii_lowercase().contains("json")
        || text.trim_start().starts_with('{')
        || text.trim_start().starts_with('[');

    let mut hint = String::new();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        hint = "来源要求登录 / 拒绝访问：检查 headers / cookie / 防盗链。".into();
    } else if status.as_u16() == 404 {
        hint = "路径不存在：检查 request.url 与 baseUrl 的拼接。".into();
    } else if status.is_server_error() {
        hint = "来源服务器错误（5xx）：稍后重试或换镜像。".into();
    } else if looks_json && extra.get("section").and_then(Value::as_str) == Some("search") {
        let uses_html = extra.get("usesHtml").and_then(Value::as_bool).unwrap_or(false);
        if uses_html {
            hint = "响应是 JSON，但规则用的是 CSS / XPath 选择器：改用 jsonPath 或换 HTML 接口。".into();
        }
    } else if looks_html {
        hint = "响应是 HTML：JSONPath 规则会解析失败；请确认接口地址或改用 CSS / XPath。".into();
    }
    if hint.is_empty() {
        hint = "请求成功；完整选择器验证请在资源库插件的 harness（?fixture=1）里跑。".into();
    }

    let preview = if status.as_u16() >= 400 {
        Some(text.chars().take(200).collect::<String>())
    } else {
        None
    };
    json!({
        "ok": status.is_success(),
        "stage": "http",
        "url": final_url,
        "status": status.as_u16(),
        "contentType": content_type,
        "bytesRead": buf.len(),
        "looksHtml": looks_html,
        "looksJson": looks_json,
        "bodyPreview": preview,
        "hint": hint,
        "extra": extra,
    })
}

/// `ResourceHubSearch` 的后端：把宿主回的来源列表按关键词 / 类型过滤后返回。
pub fn filter_sources(raw: &str, input: &Value) -> String {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let kind = input
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(SEARCH_DEFAULT_LIMIT as u64)
        .clamp(1, SEARCH_MAX_LIMIT as u64) as usize;

    let parsed: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => return format!("来源列表解析失败：{e}"),
    };
    let items = parsed
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut hits: Vec<Value> = Vec::new();
    for item in items {
        let Some(manifest) = item.get("manifest") else { continue };
        let id = manifest.get("id").and_then(Value::as_str).unwrap_or("");
        let name = manifest.get("name").and_then(Value::as_str).unwrap_or("");
        let kinds = string_list(manifest.get("kind"));
        if !kind.is_empty() && !kinds.iter().any(|k| k.eq_ignore_ascii_case(&kind)) {
            continue;
        }
        let capabilities = string_list(manifest.get("capabilities"));
        let haystack = format!(
            "{} {} {} {}",
            id,
            name,
            kinds.join(" "),
            capabilities.join(" ")
        )
        .to_lowercase();
        if !query.is_empty() && !haystack.contains(&query) {
            continue;
        }
        hits.push(json!({
            "id": id,
            "name": name,
            "version": manifest.get("version").cloned().unwrap_or(Value::Null),
            "kind": kinds,
            "capabilities": capabilities,
            "installedAt": item.get("installedAt").cloned().unwrap_or(Value::Null),
        }));
        if hits.len() >= limit {
            break;
        }
    }

    json!({ "total": hits.len(), "items": hits }).to_string()
}

/// 四个工具的 schema（由 `main.rs` 在宿主桥可用时注册）。
pub fn tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "ResourceHubSearch",
            "description": "Search the resource-hub sources installed on this machine (id / name / \
                kind / capabilities). Read-only and local. Use it before creating a new source so \
                you do not duplicate one that already exists.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Case-insensitive substring; empty = list all" },
                    "kind": { "type": "string", "enum": ["comic", "novel", "anime", "tv"], "description": "Filter by content kind" },
                    "limit": { "type": "integer", "description": "Max results (default 10, max 30)" }
                }
            }
        }),
        json!({
            "name": "SourceValidate",
            "description": "Statically validate a resource-hub source file (`{ manifest, rule }`). \
                Checks the RSP manifest fields and the declarative rule's shape (sections, request, \
                list, fields, selectors). Read-only; it does not fetch the site or execute the rule. \
                Returns ok + errors + warnings as JSON.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute or cwd-relative path to the source JSON file" }
                },
                "required": ["file_path"]
            }
        }),
        json!({
            "name": "SourceDiagnose",
            "description": "Run one real request against the source's search/chapters/pages endpoint \
                (or its baseUrl) and report a readable diagnosis: HTTP status, content type, bytes, \
                whether the body looks like HTML or JSON, and a hint. Use `query` for the @keyword \
                placeholder. This sends data to the remote site, so it asks for approval first. It \
                does not run the selectors — full fixture validation happens in the Resource Hub \
                plugin harness.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute or cwd-relative path to the source JSON file" },
                    "query": { "type": "string", "description": "Keyword for @keyword (default \"test\")" }
                },
                "required": ["file_path"]
            }
        }),
        json!({
            "name": "SourceInstall",
            "description": "Install a validated `{ manifest, rule }` source into Resource Hub \
                (`<exe root>\\ModuleData\\resource-hub\\sources\\<id>.json`). This is a WRITE: it asks \
                for approval first. The plugin re-validates the source when it loads. Refuses to \
                overwrite an existing id unless force=true.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute or cwd-relative path to the source JSON file" },
                    "force": { "type": "boolean", "description": "Overwrite an existing source with the same id" }
                },
                "required": ["file_path"]
            }
        }),
        json!({
            "name": "ResourceHubOpen",
            "description": "Hand a concrete search keyword to the installed Read plugin (resource-hub) \
                so the user can open it and search across the sources installed on this machine. \
                This tool does NOT fetch anything and does NOT open a window by itself: it returns a \
                small JSON request and the chat shows a card; clicking that card opens the plugin, \
                which runs the search. Use it when the user asks to find / read comics, novels, \
                anime or TV shows and you can already tell the title or keyword.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search keyword or title (1..200 chars)" },
                    "kind": { "type": "string", "enum": ["comic", "novel", "anime", "tv"], "description": "Optional content kind filter" }
                },
                "required": ["query"]
            }
        }),
    ]
}

/// `ResourceHubOpen`（A7，2026-10-27）：把「在阅读里搜这个词」变成一条**纯数据**工具结果。
///
/// 为什么工具本身不开窗：单发 / 只读档 / 无人值守下都不该擅自弹窗；开窗是用户在前端点
/// 结果卡时由宿主 `open_plugin_window` 完成 —— 与 `SendUserFile` 的「呈递」同一条纪律
/// （工具只产出数据，真正影响界面的动作由用户触发）。
pub fn open_request(input: &Value) -> Result<String, String> {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if query.is_empty() {
        return Err("missing required parameter \"query\"".into());
    }
    if query.chars().count() > 200 {
        return Err("query must be at most 200 characters".into());
    }
    let kind = input
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !kind.is_empty() && !matches!(kind.as_str(), "comic" | "novel" | "anime" | "tv") {
        return Err(format!(
            "kind must be one of comic / novel / anime / tv (got {kind:?})"
        ));
    }
    let mut out = json!({
        "ok": true,
        "action": "search",
        "plugin": "resource-hub",
        "query": query,
    });
    if !kind.is_empty() {
        out["kind"] = json!(kind);
    }
    Ok(out.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        json!({
            "manifest": {
                "rsp": 1,
                "id": "comic.demo",
                "name": "示例",
                "version": "1.0.0",
                "kind": ["comic"],
                "engine": "declarative",
                "entry": "rule.json",
                "baseUrl": "https://example.com",
                "capabilities": ["search"]
            },
            "rule": {
                "search": {
                    "request": { "method": "GET", "url": "/search?q=@keyword" },
                    "list": { "css": ".item" },
                    "fields": { "id": { "selector": { "css": "a" }, "attr": "href" } }
                }
            }
        })
    }

    #[test]
    fn valid_source_passes() {
        let r = validate_source(&sample());
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["id"], "comic.demo");
        assert_eq!(r["sections"], json!(["search"]));
    }

    #[test]
    fn missing_rule_and_bad_id_are_reported() {
        let mut bad = sample();
        bad["manifest"]["id"] = json!("Bad/Id");
        bad.as_object_mut().unwrap().remove("rule");
        let r = validate_source(&bad);
        assert_eq!(r["ok"], false);
        let errors = r["errors"].as_array().unwrap();
        assert!(errors.iter().any(|e| e.as_str().unwrap().contains("manifest.id")));
        assert!(errors.iter().any(|e| e.as_str().unwrap().contains("rule")));
    }

    #[test]
    fn non_declarative_engine_is_rejected() {
        let mut bad = sample();
        bad["manifest"]["engine"] = json!("js");
        let r = validate_source(&bad);
        assert_eq!(r["ok"], false);
        assert!(r["errors"].as_array().unwrap().iter().any(|e| {
            e.as_str().unwrap().contains("只支持 declarative")
        }));
    }

    #[test]
    fn capability_and_section_mismatch_warns_but_passes() {
        let mut src = sample();
        src["manifest"]["capabilities"] = json!(["search", "pages"]);
        let r = validate_source(&src);
        assert_eq!(r["ok"], true);
        let warnings = r["warnings"].as_array().unwrap();
        assert!(warnings.iter().any(|w| w.as_str().unwrap().contains("pages")));
    }

    #[test]
    fn filter_sources_matches_query_and_kind() {
        let raw = json!({
            "total": 2,
            "items": [
                { "manifest": { "id": "comic.one", "name": "漫画一", "version": "1", "kind": ["comic"], "capabilities": ["search"] } },
                { "manifest": { "id": "novel.two", "name": "小说二", "version": "1", "kind": ["novel"], "capabilities": ["chapters"] } }
            ]
        })
        .to_string();
        let r = filter_sources(&raw, &json!({ "query": "漫画", "kind": "comic" }));
        let parsed: Value = serde_json::from_str(&r).unwrap();
        assert_eq!(parsed["total"], 1);
        assert_eq!(parsed["items"][0]["id"], "comic.one");
    }

    /// A7 对话入口：只产出一条**纯数据**请求，不做任何副作用。
    #[test]
    fn open_request_is_pure_data_and_validates_its_input() {
        let ok: Value =
            serde_json::from_str(&open_request(&json!({ "query": " 斗罗大陆 " })).unwrap()).unwrap();
        assert_eq!(ok["ok"], true);
        assert_eq!(ok["action"], "search");
        assert_eq!(ok["plugin"], "resource-hub");
        assert_eq!(ok["query"], "斗罗大陆", "两端空白要被裁掉");
        assert!(ok.get("kind").is_none(), "没给 kind 就不写这个键");

        let with_kind: Value = serde_json::from_str(
            &open_request(&json!({ "query": "x", "kind": "Comic" })).unwrap(),
        )
        .unwrap();
        assert_eq!(with_kind["kind"], "comic", "kind 归一成小写");

        assert!(open_request(&json!({})).is_err(), "缺 query 必须报错");
        assert!(open_request(&json!({ "query": "   " })).is_err(), "空白 query 必须报错");
        assert!(
            open_request(&json!({ "query": "x".repeat(201) })).is_err(),
            "超长 query 必须报错"
        );
        assert!(
            open_request(&json!({ "query": "x", "kind": "video" })).is_err(),
            "非法 kind 必须报错"
        );
    }
}
