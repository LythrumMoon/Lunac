//! 资源库（Resource Hub）来源目录的**宿主后端**（重建 v1 `resource_hub.rs`）。
//!
//! 唯一持久化位置：`<exe 根>\ModuleData\resource-hub\sources\<id>.json`
//! （v1 同路径、同文件名 —— 换到 v2 后已有来源可直接读到）。
//!
//! ## 谁在写、谁在读
//!
//! · **写**：agent 的 `SourceInstall` 工具 → MCP 桥 `lunac/resource_hub_install`；
//!   插件 UI 的「导入来源」走宿主侧同一条安装逻辑。
//! · **读**：agent 的 `ResourceHubSearch` 工具 → `lunac/resource_hub_sources`。
//! · **删**：**刻意不开放给 AI**（用户资产，删错就没了）—— 只在插件 UI 里删。
//!
//! ## 分工（与 v1 同口径，别在这里复制 TS 的归一化）
//!
//! 这里只做**文件层 / 形状层**校验：能静态判定的错误一律拒绝（清单对象、`id` 安全、
//! `name` / `version` / `engine` 必备、declarative 必须有 `rule` 对象、js 必须有 `script`），
//! RSP 的**语义**校验仍由插件侧的 `rh-source-sdk`（TS）在做 —— 坏条目插件会自己丢弃。
//!
//! ## 边界
//!
//! 单条来源 ≤ 1 MiB、目录最多 200 条（与 v1 `resource_hub.rs` 同口径）；
//! 写盘一律「临时文件 + rename」原子替换，断电不会留下半个 JSON。

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};

/// 单个来源文件上限（1 MiB）。
pub const MAX_SOURCE_BYTES: u64 = 1024 * 1024;
/// 来源目录最多多少条。
pub const MAX_SOURCES: usize = 200;

/// `id` 的安全规则（与 `rh-source-sdk` 的 `ID_RE` 同口径）。
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

/// 来源目录：`<root>/ModuleData/resource-hub/sources`。
pub fn sources_dir(root: &Path) -> PathBuf {
    root.join("ModuleData")
        .join("resource-hub")
        .join("sources")
}

/// 形状校验：返回 `Ok(manifest)` 或一句可读的拒绝原因。
///
/// **刻意不补默认值** —— 缺什么就报什么（与 TS SDK 同一条纪律）。
pub fn validate_source(source: &Value) -> Result<Value, String> {
    let manifest = source
        .get("manifest")
        .filter(|value| value.is_object())
        .ok_or_else(|| "缺少 manifest 对象（形状 = { manifest, rule } 或 { manifest, script }）".to_string())?;
    let id = manifest.get("id").and_then(Value::as_str).unwrap_or("");
    if !is_safe_id(id) {
        return Err("manifest.id 只允许 [a-z0-9._-]，≤48 字符，且不能以 . 开头".to_string());
    }
    if manifest
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        return Err("manifest.name 不能为空".to_string());
    }
    if manifest
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        return Err("manifest.version 不能为空".to_string());
    }
    let engine = manifest
        .get("engine")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    match engine.as_str() {
        "declarative" => {
            if !source.get("rule").map(Value::is_object).unwrap_or(false) {
                return Err("engine=declarative 时必须带 rule 对象".to_string());
            }
        }
        "js" => {
            if source
                .get("script")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .is_empty()
            {
                return Err("engine=js 时必须带非空 script".to_string());
            }
        }
        "builtin" => {}
        other => {
            return Err(format!(
                "engine 不支持：{other}（只允许 declarative / js / builtin）"
            ))
        }
    }
    Ok(manifest.clone())
}

/// 把磁盘上的一条来源读成桥上的条目（坏文件返回原因，由调用方跳过）。
fn read_source(path: &Path) -> Result<Value, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "{} 超过单条上限（{} 字节 > {MAX_SOURCE_BYTES}）",
            path.display(),
            meta.len()
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let source: Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("{} 不是合法 JSON：{e}", path.display()))?;
    let manifest = validate_source(&source)?;
    let installed_at = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 条目形状 = v1 插件 `loadHostSources()` 的入参：manifest / rule / script / warnings /
    // installedAt（秒）。`warnings` 留空数组：语义校验在插件侧，宿主不替它下结论。
    let mut item = json!({
        "manifest": manifest,
        "warnings": [],
        "installedAt": installed_at,
    });
    if let Some(rule) = source.get("rule").filter(|value| value.is_object()) {
        item["rule"] = rule.clone();
    }
    if let Some(script) = source.get("script").and_then(Value::as_str) {
        item["script"] = json!(script);
    }
    Ok(item)
}

/// 列出全部可用来源（按 id 升序，保证同一目录两次调用结果逐字节相同）。
///
/// 坏文件**只跳过它自己**并把原因写进日志，不打断其它来源。
pub fn list_sources(root: &Path) -> Result<Vec<Value>, String> {
    let dir = sources_dir(root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        // 目录还不存在 = 一条来源都没有（不是错误）。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("读取来源目录 {} 失败：{e}", dir.display())),
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("json"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();

    let mut items = Vec::new();
    for path in files {
        match read_source(&path) {
            Ok(item) => items.push(item),
            Err(reason) => eprintln!("[lunac-resource-hub] 跳过 {reason}"),
        }
    }
    items.sort_by_key(|item| {
        item["manifest"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    });
    Ok(items)
}

/// 安装（或 `force` 覆盖）一条来源，返回落盘路径。
pub fn install_source(root: &Path, source: &Value, force: bool) -> Result<String, String> {
    let manifest = validate_source(source)?;
    let id = manifest
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let dir = sources_dir(root);
    let target = dir.join(format!("{id}.json"));
    if target.exists() && !force {
        return Err(format!("来源 {id} 已存在（覆盖需要 force=true）"));
    }
    if !target.exists() && count_sources(&dir)? >= MAX_SOURCES {
        return Err(format!("来源目录已达上限 {MAX_SOURCES} 条，请先在插件里删掉不用的"));
    }
    let body = serde_json::to_vec(source).map_err(|e| format!("序列化来源失败：{e}"))?;
    if body.len() as u64 > MAX_SOURCE_BYTES {
        return Err(format!(
            "来源超过单条上限（{} 字节 > {MAX_SOURCE_BYTES}）",
            body.len()
        ));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    // 原子写：同目录临时文件 + rename（Windows 上 rename 允许覆盖已存在目标）。
    let tmp = dir.join(format!(".{id}.json.tmp-{}", std::process::id()));
    std::fs::write(&tmp, &body).map_err(|e| format!("写入 {} 失败：{e}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("落盘 {} 失败：{e}", target.display()));
    }
    Ok(target.display().to_string())
}

/// 目录里现有的 `*.json` 条数（上限判据用）。
fn count_sources(dir: &Path) -> Result<usize, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("读取来源目录 {} 失败：{e}", dir.display())),
    };
    Ok(entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("json"))
                .unwrap_or(false)
        })
        .count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-resource-hub-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(sources_dir(&dir)).unwrap();
        dir
    }

    fn declarative(id: &str) -> Value {
        json!({
            "manifest": {
                "rsp": 1,
                "id": id,
                "name": "示例来源",
                "version": "1.0.0",
                "kind": ["comic"],
                "engine": "declarative",
                "capabilities": ["search", "chapters", "pages"],
            },
            "rule": { "search": { "url": "https://example.com/search?q={query}" } },
        })
    }

    #[test]
    fn install_then_list_round_trips() {
        let root = temp_root("roundtrip");
        let path = install_source(&root, &declarative("demo.source"), false).unwrap();
        assert!(path.ends_with("demo.source.json"), "{path}");

        let items = list_sources(&root).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["manifest"]["id"], json!("demo.source"));
        assert_eq!(items[0]["manifest"]["engine"], json!("declarative"));
        assert!(items[0]["rule"].is_object(), "rule 必须原样带回");
        assert!(items[0]["installedAt"].as_u64().unwrap() > 0);
        assert_eq!(items[0]["warnings"], json!([]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_requires_force_to_overwrite() {
        let root = temp_root("force");
        install_source(&root, &declarative("demo.force"), false).unwrap();
        let err = install_source(&root, &declarative("demo.force"), false).unwrap_err();
        assert!(err.contains("已存在"), "{err}");
        install_source(&root, &declarative("demo.force"), true).unwrap();
        assert_eq!(list_sources(&root).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn invalid_sources_are_rejected_with_reasons() {
        let root = temp_root("invalid");
        // 不安全的 id
        let mut bad = declarative("Bad/Id");
        bad["manifest"]["id"] = json!("Bad/Id");
        assert!(install_source(&root, &bad, false).unwrap_err().contains("[a-z0-9._-]"));
        // 缺 name / version
        let mut bad = declarative("demo.noname");
        bad["manifest"]["name"] = json!("  ");
        assert!(install_source(&root, &bad, false).unwrap_err().contains("name"));
        let mut bad = declarative("demo.nover");
        bad["manifest"]["version"] = json!("");
        assert!(install_source(&root, &bad, false).unwrap_err().contains("version"));
        // declarative 缺 rule / js 缺 script / 未知 engine
        let mut bad = declarative("demo.norule");
        bad.as_object_mut().unwrap().remove("rule");
        assert!(install_source(&root, &bad, false).unwrap_err().contains("rule"));
        let bad = json!({
            "manifest": { "id": "demo.js", "name": "x", "version": "1", "engine": "js", "kind": ["comic"] }
        });
        assert!(install_source(&root, &bad, false).unwrap_err().contains("script"));
        let bad = json!({
            "manifest": { "id": "demo.eng", "name": "x", "version": "1", "engine": "wasm", "kind": ["comic"] }
        });
        assert!(install_source(&root, &bad, false).unwrap_err().contains("engine"));
        // 一条都不该落盘
        assert!(list_sources(&root).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_skips_broken_files_and_oversized_ones() {
        let root = temp_root("broken");
        install_source(&root, &declarative("good.source"), false).unwrap();
        let dir = sources_dir(&root);
        std::fs::write(dir.join("broken.json"), b"{ not json").unwrap();
        std::fs::write(dir.join("nomanifest.json"), b"{\"rule\":{}}").unwrap();
        // 超过 1 MiB 的文件要在「读之前」就被跳过（否则会把整块读进来）
        let mut huge = declarative("huge.source");
        huge["rule"]["padding"] = json!("x".repeat(MAX_SOURCE_BYTES as usize));
        std::fs::write(
            dir.join("huge.json"),
            serde_json::to_vec(&huge).unwrap(),
        )
        .unwrap();

        let items = list_sources(&root).unwrap();
        assert_eq!(items.len(), 1, "坏文件必须只跳过它自己");
        assert_eq!(items[0]["manifest"]["id"], json!("good.source"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_dir_lists_as_empty() {
        let root = std::env::temp_dir().join(format!(
            "lunac-resource-hub-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        assert!(list_sources(&root).unwrap().is_empty());
    }
}
