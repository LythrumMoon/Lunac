//! 工具编辑器（v1 `tool-editor` 插件的原生等价物）：`<exe 根>/tools/*.json`。
//!
//! v1 的 `tool-editor.ts` 调的是 `list_tool_files` / `read_tool_file` /
//! `save_tool_file` / `delete_tool_file`（`app/src-tauri/src/commands.rs:2603-2688`），
//! 宿主侧真正落盘的位置是 `storage::lunac_root_dir().join("tools")` —— 也就是
//! `<exe 根>\tools\`，**一份文件 = 一个工具定义**，格式与 `mcp_server::ToolDef` 一致：
//! `{ name, description, inputSchema?, handler: { type: "shell"|"http"|"builtin", ... } }`。
//!
//! 它**不是** core-agent 的 MCP **服务器**配置：`config/mcp.json`（远端 HTTP 服务器）与
//! 项目 `.mcp.json`（stdio / http，带信任门）都由 agent 侧读取（`core-agent/src/mcp.rs`，
//! v1 设置面板也只提供「打开文件」，刻意不做图形编辑器）。本模块不碰这两份文件。
//!
//! 写盘用临时文件 + rename（与 `memo.rs` 同口径），写坏不会毁掉已有工具文件；
//! 坏 JSON / 缺字段只把那一份标成「无效」，不影响其它条目，也绝不 panic。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// 工具定义目录（与 v1 `commands.rs::mcp_tools_dir` 同名：`<exe 根>/tools`）。
pub fn tools_dir(root: &Path) -> PathBuf {
    root.join("tools")
}

/// 读取全部工具定义（JSON：`{ "entries": [...], "error": "…" }`）。
///
/// `entries[]` 字段：`filename` / `name` / `description` / `command` / `kind` /
/// `params[]` / `valid`。目录读不了时 `entries` 为空、`error` 是一句人话。
pub fn load_json(root: &Path) -> String {
    let (entries, error) = read_entries(root);
    json!({ "entries": entries, "error": error }).to_string()
}

/// 写入请求（JSON：`{ ok: bool, error: string }` 由调用方拼，这里只返回 Result）。
///
/// 支持的 `action`：
///   · `save`   —— `{ action, filename?, content }`：`content` 必须是合法工具 JSON；
///   · `delete` —— `{ action, filename }`：不存在也算成功（幂等）。
pub fn save_json(root: &Path, request: &Value) -> Result<(), String> {
    let action = request
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("save");
    match action {
        "save" => {
            let content = request
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| "缺少 content（工具 JSON 文本）".to_string())?;
            let value: Value = serde_json::from_str(content).map_err(|e| {
                format!("工具 JSON 不是合法 JSON：{e}；请检查引号 / 逗号后重试")
            })?;
            validate_tool(&value)?;
            let filename = filename_of(request, &value)?;
            write_atomic(&tools_dir(root), &filename, content)
        }
        "delete" => {
            let filename = request
                .get("filename")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "缺少要删除的 filename".to_string())?;
            check_filename(filename)?;
            let path = tools_dir(root).join(filename);
            if !path.exists() {
                return Ok(());
            }
            std::fs::remove_file(&path)
                .map_err(|e| format!("删不掉 {filename}：{e}；请确认文件没被其它程序占用"))
        }
        other => Err(format!("未知操作 {other}（只支持 save / delete）")),
    }
}

/// 读取一份工具的原始 JSON 文本（v1 `read_tool_file`）。
///
/// 返回 JSON：`{ ok, content, error }`；文件名不合规 / 读不到时 `ok=false` 且 `error` 是人话。
pub fn read_file_json(root: &Path, filename: &str) -> String {
    let filename = filename.trim();
    if let Err(reason) = check_filename(filename) {
        return json!({ "ok": false, "content": "", "error": reason }).to_string();
    }
    let path = tools_dir(root).join(filename);
    match std::fs::read_to_string(&path) {
        Ok(content) => json!({ "ok": true, "content": content, "error": "" }).to_string(),
        Err(e) => json!({
            "ok": false,
            "content": "",
            "error": format!("读不了 {filename}：{e}"),
        })
        .to_string(),
    }
}

/// 远端 MCP 服务器配置：`<exe 根>\config\mcp.json`（`{ "servers": [{name,url,headers,auth}] }`）。
///
/// 这份文件由 agent 在启动时读一次（`core-agent/src/mcp.rs::parse_config`），**宿主刻意
/// 不做图形编辑器**（v1 设置面板也只给「打开文件」）—— 写错一个字段就是整份配置失效，
/// 与其做半吊子表单，不如让用户直接改 JSON。
pub fn remote_file(root: &Path) -> PathBuf {
    root.join("config").join("mcp.json")
}

/// 打开前的准备：文件不存在就先落一份**空模板** `{"servers": []}`，返回绝对路径。
///
/// 模板刻意只有空数组：写一条示例服务器会让用户以为「已经配好了一台」。
/// 已存在则**一个字节都不动**（用户手写的注释格式 / 字段顺序不能被我们重排）。
pub fn ensure_remote_file(root: &Path) -> Result<PathBuf, String> {
    let path = remote_file(root);
    if path.exists() {
        return Ok(path);
    }
    let Some(parent) = path.parent() else {
        return Err("配置路径不合法".to_string());
    };
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("建不了配置目录（{}）：{e}", parent.display()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, "{\n  \"servers\": []\n}\n")
        .map_err(|e| format!("写不进 {}：{e}；请确认目录可写", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("替换 {} 失败：{e}", path.display()))?;
    Ok(path)
}

fn read_entries(root: &Path) -> (Vec<Value>, String) {
    let dir = tools_dir(root);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return (
            Vec::new(),
            format!("建不了工具目录（{}）：{e}", dir.display()),
        );
    }
    let mut paths = Vec::new();
    match std::fs::read_dir(&dir) {
        Ok(items) => {
            for item in items.flatten() {
                let path = item.path();
                if path.extension().map_or(false, |ext| ext == "json") {
                    paths.push(path);
                }
            }
        }
        Err(e) => {
            return (
                Vec::new(),
                format!("读不了工具目录（{}）：{e}", dir.display()),
            )
        }
    }
    paths.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    let mut entries = Vec::new();
    let mut first_error = String::new();
    for path in paths {
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        match std::fs::read_to_string(&path) {
            Ok(text) => entries.push(entry_json(&filename, &text)),
            Err(e) => {
                if first_error.is_empty() {
                    first_error = format!("读不了 {filename}：{e}");
                }
                entries.push(invalid_entry(&filename));
            }
        }
    }
    (entries, first_error)
}

fn invalid_entry(filename: &str) -> Value {
    json!({
        "filename": filename,
        "name": "",
        "description": "",
        "command": "",
        "kind": "",
        "params": [],
        "valid": false,
    })
}

/// 一份文件 → 面板要的一条（坏 JSON / 缺字段 = `valid: false`，仍列出来）。
fn entry_json(filename: &str, text: &str) -> Value {
    let parsed = serde_json::from_str::<Value>(text).ok();
    let valid = parsed
        .as_ref()
        .map(|value| validate_tool(value).is_ok())
        .unwrap_or(false);
    let Some(value) = parsed else {
        return invalid_entry(filename);
    };
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let (kind, command) = handler_summary(&value);
    let params = schema_params(&value);
    json!({
        "filename": filename,
        "name": name,
        "description": description,
        "command": command,
        "kind": kind,
        "params": params,
        "valid": valid,
    })
}

/// `handler` → `(type, 一行命令摘要)`。
fn handler_summary(value: &Value) -> (String, String) {
    let Some(handler) = value.get("handler").and_then(Value::as_object) else {
        return (String::new(), String::new());
    };
    let kind = handler
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let command = match kind.as_str() {
        "shell" => handler
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "http" => {
            let method = handler
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("");
            let url = handler.get("url").and_then(Value::as_str).unwrap_or("");
            format!("{method} {url}").trim().to_string()
        }
        "builtin" => handler
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    };
    (kind, command)
}

/// `inputSchema.properties` 的键（参数摘要；最多 8 个）。
fn schema_params(value: &Value) -> Vec<String> {
    let Some(properties) = value
        .get("inputSchema")
        .and_then(|schema| schema.get("properties"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    properties.keys().take(8).cloned().collect()
}

/// 与 v1 `ToolDef`（`mcp_server.rs:19-31`）的必填项同口径：
/// `name` 非空、`description` 是字符串、`handler` 是已知类型且必填字段齐全。
fn validate_tool(value: &Value) -> Result<(), String> {
    let Some(object) = value.as_object() else {
        return Err("工具定义必须是 JSON 对象".to_string());
    };
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if name.is_empty() {
        return Err("工具缺少非空的 name".to_string());
    }
    if object.get("description").and_then(Value::as_str).is_none() {
        return Err("工具缺少 description 字符串".to_string());
    }
    let Some(handler) = object.get("handler").and_then(Value::as_object) else {
        return Err("工具缺少 handler 对象".to_string());
    };
    let kind = handler.get("type").and_then(Value::as_str).unwrap_or("");
    let required = |field: &str| {
        handler
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .is_some()
    };
    match kind {
        "shell" => {
            if !required("command") {
                return Err("shell handler 缺少非空的 command".to_string());
            }
        }
        "http" => {
            if !required("method") || !required("url") {
                return Err("http handler 缺少非空的 method / url".to_string());
            }
        }
        "builtin" => {
            if !required("name") {
                return Err("builtin handler 缺少非空的 name".to_string());
            }
        }
        other => {
            return Err(format!(
                "未知的 handler.type：{other}（只支持 shell / http / builtin）"
            ))
        }
    }
    Ok(())
}

/// 文件名：优先用调用方给的 `filename`，否则由 `name` 生成（与 v1 `tool-editor.ts:148`
/// 同一正则：非 `[A-Za-z0-9_-]` 一律换成 `-`，再补 `.json`）。
fn filename_of(request: &Value, value: &Value) -> Result<String, String> {
    if let Some(name) = request
        .get("filename")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        check_filename(name)?;
        return Ok(name.to_string());
    }
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("tool");
    Ok(format!("{}.json", sanitize_name(name)))
}

fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "tool".to_string()
    } else {
        cleaned
    }
}

/// 只允许 `tools` 目录下的裸 `.json` 文件名（防目录穿越）。
fn check_filename(filename: &str) -> Result<(), String> {
    let bad = filename.is_empty()
        || filename == "."
        || filename == ".."
        || filename.contains('/')
        || filename.contains('\\')
        || !filename.ends_with(".json")
        || filename.eq_ignore_ascii_case(".json");
    if bad {
        return Err(format!(
            "文件名不合法（{filename}）：只能是 tools 目录下的 .json 文件名"
        ));
    }
    Ok(())
}

fn write_atomic(dir: &Path, filename: &str, content: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("建不了工具目录（{}）：{e}", dir.display()))?;
    let path = dir.join(filename);
    let tmp = dir.join(format!(".{filename}.tmp"));
    std::fs::write(&tmp, content)
        .map_err(|e| format!("写不进 {}：{e}；请确认目录可写", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("替换 {} 失败：{e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-tools-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    fn read(root: &Path) -> (Vec<Value>, String) {
        let raw: Value = serde_json::from_str(&load_json(root)).expect("valid json");
        let entries = raw
            .get("entries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let error = raw
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        (entries, error)
    }

    fn save_shell(root: &Path, filename: &str, name: &str, command: &str) {
        let content = json!({
            "name": name,
            "description": "desc",
            "inputSchema": {"type": "object", "properties": {"path": {}}},
            "handler": {"type": "shell", "command": command},
        })
        .to_string();
        save_json(
            root,
            &json!({"action": "save", "filename": filename, "content": content}),
        )
        .expect("save");
    }

    /// 路径口径：`<root>/tools`（v1 `commands.rs:2686-2688`）。
    #[test]
    fn tools_dir_is_root_tools() {
        let root = Path::new("D:/Lunac");
        assert_eq!(tools_dir(root), root.join("tools"));
    }

    #[test]
    fn missing_dir_reads_as_empty_without_panic() {
        let root = temp_root("missing");
        let (entries, error) = read(&root);
        assert!(entries.is_empty());
        assert!(error.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 有效 / 无效条目都列出来；命令、参数摘要与来源文件来自文件本身。
    #[test]
    fn lists_valid_and_invalid_entries_with_source_file() {
        let root = temp_root("list");
        save_shell(&root, "alpha.json", "alpha", "echo hi");
        std::fs::create_dir_all(tools_dir(&root)).unwrap();
        std::fs::write(tools_dir(&root).join("broken.json"), "{not json").unwrap();

        let (entries, error) = read(&root);
        assert!(error.is_empty());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["filename"], "alpha.json");
        assert_eq!(entries[0]["name"], "alpha");
        assert_eq!(entries[0]["command"], "echo hi");
        assert_eq!(entries[0]["kind"], "shell");
        assert_eq!(entries[0]["params"][0], "path");
        assert_eq!(entries[0]["valid"], true);
        assert_eq!(entries[1]["filename"], "broken.json");
        assert_eq!(entries[1]["valid"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 保存 → 读回 → 删除（v1 `save_tool_file` / `delete_tool_file` 的等价往返）。
    #[test]
    fn save_read_delete_round_trip() {
        let root = temp_root("roundtrip");
        save_shell(&root, "alpha.json", "alpha", "echo hi");
        let (entries, _) = read(&root);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], "alpha");

        save_json(&root, &json!({"action": "delete", "filename": "alpha.json"}))
            .expect("delete");
        assert!(read(&root).0.is_empty());
        // 删除不存在的文件也是成功（幂等）。
        save_json(&root, &json!({"action": "delete", "filename": "alpha.json"}))
            .expect("idempotent delete");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// v1 `read_tool_file`：读回原始 JSON 文本；读不到 / 文件名不合规都给人话。
    #[test]
    fn read_file_returns_raw_json_and_rejects_bad_names() {
        let root = temp_root("readfile");
        save_shell(&root, "alpha.json", "alpha", "echo hi");
        let raw: Value =
            serde_json::from_str(&read_file_json(&root, "alpha.json")).expect("json");
        assert_eq!(raw["ok"], true);
        assert!(raw["content"].as_str().unwrap().contains("\"alpha\""));
        assert_eq!(raw["error"], "");

        let missing: Value =
            serde_json::from_str(&read_file_json(&root, "nope.json")).expect("json");
        assert_eq!(missing["ok"], false);
        assert!(!missing["error"].as_str().unwrap().is_empty());

        let escape: Value =
            serde_json::from_str(&read_file_json(&root, "../config/mcp.json")).expect("json");
        assert_eq!(escape["ok"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 写前校验：缺 name / 缺 command / 未知 handler / 坏 JSON 都拒绝，且不落盘。
    #[test]
    fn save_rejects_invalid_structure() {
        let root = temp_root("invalid");
        let bad = vec![
            json!({"name": "", "description": "", "handler": {"type": "shell", "command": "x"}}),
            json!({"description": "", "handler": {"type": "shell", "command": "x"}}),
            json!({"name": "a", "description": "", "handler": {"type": "shell", "command": ""}}),
            json!({"name": "a", "description": "", "handler": {"type": "builtin"}}),
            json!({"name": "a", "description": "", "handler": {"type": "nope"}}),
        ];
        for content in bad {
            let result = save_json(
                &root,
                &json!({"action": "save", "filename": "x.json", "content": content.to_string()}),
            );
            assert!(result.is_err(), "must reject: {content}");
        }
        let result = save_json(
            &root,
            &json!({"action": "save", "filename": "x.json", "content": "{oops"}),
        );
        assert!(result.is_err());
        assert!(!tools_dir(&root).join("x.json").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 文件名只允许裸 `.json`（防目录穿越）。
    #[test]
    fn filename_guard_blocks_path_traversal() {
        let root = temp_root("guard");
        let content = json!({
            "name": "a",
            "description": "",
            "handler": {"type": "shell", "command": "x"},
        })
        .to_string();
        for filename in ["../evil.json", "sub/a.json", ".json", "a.txt"] {
            let result = save_json(
                &root,
                &json!({"action": "save", "filename": filename, "content": content}),
            );
            assert!(result.is_err(), "must reject: {filename}");
        }
        // 没给 filename 时按 name 生成。
        let result = save_json(
            &root,
            &json!({"action": "save", "content": content}),
        );
        assert!(result.is_ok());
        assert!(tools_dir(&root).join("a.json").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// http / builtin 的命令摘要。
    #[test]
    fn http_and_builtin_handlers_are_summarized() {
        let root = temp_root("handlers");
        std::fs::create_dir_all(tools_dir(&root)).unwrap();
        std::fs::write(
            tools_dir(&root).join("web.json"),
            json!({
                "name": "web",
                "description": "",
                "handler": {"type": "http", "method": "POST", "url": "https://example.com"},
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            tools_dir(&root).join("local.json"),
            json!({
                "name": "local",
                "description": "",
                "handler": {"type": "builtin", "name": "image-pattern"},
            })
            .to_string(),
        )
        .unwrap();

        let (entries, _) = read(&root);
        let by_file = |name: &str| {
            entries
                .iter()
                .find(|entry| entry["filename"] == name)
                .cloned()
                .unwrap_or(Value::Null)
        };
        let web = by_file("web.json");
        assert_eq!(web["command"], "POST https://example.com");
        assert_eq!(web["kind"], "http");
        let local = by_file("local.json");
        assert_eq!(local["command"], "image-pattern");
        assert_eq!(local["kind"], "builtin");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 坏文件不影响后续写入（临时文件 + rename 不会毁掉目录）。
    #[test]
    fn broken_file_does_not_block_later_writes() {
        let root = temp_root("broken");
        std::fs::create_dir_all(tools_dir(&root)).unwrap();
        std::fs::write(tools_dir(&root).join("broken.json"), "{not json").unwrap();
        save_shell(&root, "ok.json", "ok", "echo ok");
        let (entries, _) = read(&root);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1]["name"], "ok");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// MCP 组入口（§待办 3-31 ①）：文件不存在就先落一份空模板；已存在一个字节都不动。
    #[test]
    fn ensure_remote_file_creates_an_empty_template_only_once() {
        let root = temp_root("mcp-file");
        let path = ensure_remote_file(&root).expect("创建");
        assert!(path.starts_with(&root));
        assert!(path.ends_with("config\\mcp.json") || path.ends_with("config/mcp.json"));
        let text = std::fs::read_to_string(&path).expect("读回来");
        let parsed: Value = serde_json::from_str(&text).expect("模板必须是合法 JSON");
        assert_eq!(parsed["servers"], json!([]), "模板里不放假服务器");

        // 用户改过之后不能再被我们覆盖。
        std::fs::write(&path, "{\"servers\":[{\"name\":\"a\",\"url\":\"https://a.example/mcp\"}]}")
            .unwrap();
        let again = ensure_remote_file(&root).expect("已存在");
        assert_eq!(again, path);
        let text = std::fs::read_to_string(&path).expect("读回来");
        assert!(text.contains("a.example"), "不能重写用户手写的配置");
        let _ = std::fs::remove_dir_all(&root);
    }
}
