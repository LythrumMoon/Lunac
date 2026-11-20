//! **插件存储**（v1 `plugin_kv` 的 v2 等价物，docs/todo.md §M5z-补69）。
//!
//! v1 的插件是 WebView + TS，键值对经 Tauri `invoke("plugin_kv_get"/…)` 落在宿主侧；
//! v2 不加载第三方 WebView（唯一例外是桌宠的 Live2D），插件 UI 由宿主画，所以这一层
//! 就是**宿主侧的插件存储**：一个插件一份文件，读写都走这里。
//!
//! ## 口径（单一真相源）
//!
//! · 路径 `<exe 根>/ModuleData/plugins/<id>/kv.json`，形状
//!   `{"version":1,"values":{"<key>": <任意 JSON>}}`；
//! · 写盘用**临时文件 + rename**（与 `mcp_config.rs` / `settings.rs` 同口径）——
//!   写坏不会毁掉已有数据；读坏时**不静默清空**，而是拒绝并让用户决定（见 `read_store`）；
//! · 配额**从紧**（v1 的具体数值随源码丢失，已不可考）：单插件 256 KiB、单值 64 KiB、
//!   键数 256、key ≤ 128 字符、插件 id ≤ 64 字符且只允许 `[A-Za-z0-9._-]`。
//!   超了直接拒绝并给出实际大小 —— 不截断、不静默丢键。
//!
//! ## 边界（如实记）
//!
//! · 插件 id 进路径前过白名单（挡住 `..` / 分隔符）；`kv_dir` 不解析符号链接
//!   —— 插件目录本身在 `<exe 根>` 之内，与 `plugin_market` 的安装位置同源；
//! · 这里**不是沙箱**：sidecar 插件是完整本机进程，想绕过宿主直接写自己的文件也行；
//!   这一层保证的是「宿主侧读写不互相踩、不写坏、不无限膨胀」。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

/// 单个插件的总存储上限。
const MAX_PLUGIN_BYTES: usize = 256 * 1024;
/// 单个值（序列化后）的上限。
const MAX_VALUE_BYTES: usize = 64 * 1024;
/// 单个插件最多保留的键数。
const MAX_KEYS: usize = 256;
/// key 的长度上限（字符）。
const MAX_KEY_CHARS: usize = 128;
/// 插件 id 的长度上限（字符）。
const MAX_ID_CHARS: usize = 64;

/// 存储格式版本。字段语义变化时递增；读到不认识的版本直接拒绝（不猜、不迁移）。
const VERSION: u64 = 1;

/// 读改写要串行：同进程里两个面板同时写同一个插件的存储时，后写的那次不能盖掉前一次。
/// 进程级互斥（宿主是单进程），比每个插件一把锁简单且没有泄漏。
static KV_LOCK: Mutex<()> = Mutex::new(());

/// 插件存储目录：`<exe 根>/ModuleData/plugins/<id>`。
pub fn kv_dir(root: &Path, id: &str) -> PathBuf {
    root.join("ModuleData").join("plugins").join(id)
}

fn kv_path(root: &Path, id: &str) -> PathBuf {
    kv_dir(root, id).join("kv.json")
}

/// 读一个键：`{ ok, key, found, value, error }`；没有这个键时 `found=false`、`value=null`。
pub fn get_json(root: &Path, id: &str, key: &str) -> String {
    let id = id.trim();
    let key = key.trim();
    if let Err(reason) = check_id(id).and_then(|()| check_key(key)) {
        return json!({ "ok": false, "key": key, "found": false, "value": Value::Null, "error": reason })
            .to_string();
    }
    let _guard = KV_LOCK.lock();
    let values = match read_values(root, &id) {
        Ok(values) => values,
        Err(reason) => {
            return json!({ "ok": false, "key": key, "found": false, "value": Value::Null, "error": reason })
                .to_string()
        }
    };
    match values.get(key) {
        Some(value) => json!({ "ok": true, "key": key, "found": true, "value": value, "error": "" })
            .to_string(),
        None => json!({ "ok": true, "key": key, "found": false, "value": Value::Null, "error": "" })
            .to_string(),
    }
}

/// 写一个键（新建 / 覆盖）：入参 `{ id, key, value }`，回执 `{ ok, key, bytes, error }`。
///
/// `bytes` 是**这个插件整份存储**写完之后的大小（前端据此画配额用量）。
pub fn set_json(root: &Path, request: &Value) -> String {
    let id = request
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let key = request
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if let Err(reason) = check_id(&id).and_then(|()| check_key(&key)) {
        return kv_error(&key, 0, &reason);
    }
    let Some(value) = request.get("value") else {
        return kv_error(&key, 0, "缺少 value 字段（删除请用 delete）");
    };
    let value_bytes = serde_json::to_string(value).map(|s| s.len()).unwrap_or(0);
    if value_bytes > MAX_VALUE_BYTES {
        return kv_error(
            &key,
            0,
            &format!(
                "这个值太大（{value_bytes} 字节）：单个值上限 {} KiB",
                MAX_VALUE_BYTES / 1024
            ),
        );
    }

    let _guard = KV_LOCK.lock();
    let mut values = match read_values(root, &id) {
        Ok(values) => values,
        Err(reason) => return kv_error(&key, 0, &reason),
    };
    if !values.contains_key(&key) && values.len() >= MAX_KEYS {
        return kv_error(
            &key,
            0,
            &format!("键太多（上限 {MAX_KEYS} 个）：请先删掉不用的键"),
        );
    }
    values.insert(key.clone(), value.clone());
    let bytes = match write_values(root, &id, &values) {
        Ok(bytes) => bytes,
        Err(reason) => return kv_error(&key, 0, &reason),
    };
    json!({ "ok": true, "key": key, "bytes": bytes, "error": "" }).to_string()
}

/// 删一个键：`{ ok, key, removed, bytes, error }`（键不存在也算成功）。
pub fn delete_json(root: &Path, id: &str, key: &str) -> String {
    let id = id.trim();
    let key = key.trim();
    if let Err(reason) = check_id(id).and_then(|()| check_key(key)) {
        return json!({ "ok": false, "key": key, "removed": false, "bytes": 0, "error": reason })
            .to_string();
    }
    let _guard = KV_LOCK.lock();
    let mut values = match read_values(root, id) {
        Ok(values) => values,
        Err(reason) => {
            return json!({ "ok": false, "key": key, "removed": false, "bytes": 0, "error": reason })
                .to_string()
        }
    };
    let removed = values.remove(key).is_some();
    let bytes = if removed {
        match write_values(root, id, &values) {
            Ok(bytes) => bytes,
            Err(reason) => {
                return json!({ "ok": false, "key": key, "removed": false, "bytes": 0, "error": reason })
                    .to_string()
            }
        }
    } else {
        size_of(root, id)
    };
    json!({ "ok": true, "key": key, "removed": removed, "bytes": bytes, "error": "" }).to_string()
}

/// 列出这个插件的全部键：`{ ok, entries:[{key,bytes}], bytes, error }`。
pub fn list_json(root: &Path, id: &str) -> String {
    let id = id.trim();
    if let Err(reason) = check_id(id) {
        return json!({ "ok": false, "entries": [], "bytes": 0, "error": reason }).to_string();
    }
    let _guard = KV_LOCK.lock();
    let values = match read_values(root, id) {
        Ok(values) => values,
        Err(reason) => {
            return json!({ "ok": false, "entries": [], "bytes": 0, "error": reason }).to_string()
        }
    };
    let mut keys: Vec<String> = values.keys().cloned().collect();
    keys.sort();
    let entries: Vec<Value> = keys
        .into_iter()
        .map(|key| {
            let bytes = values
                .get(&key)
                .and_then(|value| serde_json::to_string(value).ok())
                .map(|s| s.len())
                .unwrap_or(0);
            json!({ "key": key, "bytes": bytes })
        })
        .collect();
    json!({
        "ok": true,
        "entries": entries,
        "bytes": size_of(root, id),
        "error": "",
    })
    .to_string()
}

/// 清空这个插件的存储（删掉整份 `kv.json`）：`{ ok, error }`；文件不存在也算成功。
pub fn clear_json(root: &Path, id: &str) -> String {
    let id = id.trim();
    if let Err(reason) = check_id(id) {
        return kv_error("", 0, &reason);
    }
    let _guard = KV_LOCK.lock();
    let path = kv_path(root, id);
    if !path.exists() {
        return json!({ "ok": true, "bytes": 0, "error": "" }).to_string();
    }
    match std::fs::remove_file(&path) {
        Ok(()) => json!({ "ok": true, "bytes": 0, "error": "" }).to_string(),
        Err(error) => kv_error(
            "",
            0,
            &format!("清空失败（{}）：{error}；请确认文件没被占用", path.display()),
        ),
    }
}

fn kv_error(key: &str, bytes: usize, reason: &str) -> String {
    json!({ "ok": false, "key": key, "bytes": bytes, "error": reason }).to_string()
}

/// 读一份存储的 `values`；文件不存在 = 空表（不是错误）。
///
/// 损坏 / 版本不符一律返回可读 `Err`，**不自动清空** —— 插件存储里可能是用户的
/// 真实数据，静默丢比报错更糟。要让用户决定：删掉那个文件，或调用 `clear`。
fn read_values(root: &Path, id: &str) -> Result<Map<String, Value>, String> {
    let path = kv_path(root, id);
    if !path.exists() {
        return Ok(Map::new());
    }
    let body = std::fs::read_to_string(&path)
        .map_err(|e| format!("读不了插件存储（{}）：{e}", path.display()))?;
    let store: Value = serde_json::from_str(&body).map_err(|e| {
        format!(
            "插件存储已损坏（{}）：{e}；请先删掉这个文件或清空存储",
            path.display()
        )
    })?;
    if store.get("version").and_then(Value::as_u64) != Some(VERSION) {
        return Err(format!(
            "插件存储版本不兼容（{}）：当前只认 v{VERSION}",
            path.display()
        ));
    }
    match store.get("values") {
        Some(Value::Object(map)) => Ok(map.clone()),
        Some(_) => Err(format!(
            "插件存储格式不对（{}）：values 不是对象",
            path.display()
        )),
        None => Ok(Map::new()),
    }
}

/// 写回一份存储，返回写入后的文件字节数。超配额时**不写**、返回可读错误。
fn write_values(root: &Path, id: &str, values: &Map<String, Value>) -> Result<usize, String> {
    let body = serde_json::to_string(&json!({ "version": VERSION, "values": values }))
        .map_err(|e| format!("序列化插件存储失败：{e}"))?;
    if body.len() > MAX_PLUGIN_BYTES {
        return Err(format!(
            "插件存储超出上限（{} 字节）：单插件上限 {} KiB；请先删掉不用的键",
            body.len(),
            MAX_PLUGIN_BYTES / 1024
        ));
    }
    let dir = kv_dir(root, id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("建不了插件存储目录（{}）：{e}", dir.display()))?;
    let path = dir.join("kv.json");
    let tmp = dir.join("kv.json.tmp");
    std::fs::write(&tmp, body.as_bytes())
        .map_err(|e| format!("写不进插件存储（{}）：{e}", tmp.display()))?;
    // Windows 的 rename 不覆盖已存在目标：先删旧再改名（与 checkpoint / 角色会话同纪律）。
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| format!("替换插件存储失败（{}）：{e}", path.display()))?;
    }
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("插件存储改名失败（{}）：{e}", path.display()))?;
    Ok(body.len())
}

/// 现有文件大小（不存在 = 0）。
fn size_of(root: &Path, id: &str) -> usize {
    std::fs::metadata(kv_path(root, id))
        .map(|m| m.len() as usize)
        .unwrap_or(0)
}

/// 插件 id：`[A-Za-z0-9._-]`、≤64 字符、不许 `.` / `..`（挡住路径穿越）。
fn check_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("插件 id 不能为空".to_string());
    }
    if id.chars().count() > MAX_ID_CHARS {
        return Err(format!("插件 id 太长（上限 {MAX_ID_CHARS} 字符）"));
    }
    if id == "." || id == ".." {
        return Err("插件 id 不合法".to_string());
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
    {
        return Err(format!(
            "插件 id 只能包含字母 / 数字 / `.` / `_` / `-`（收到 {id}）"
        ));
    }
    Ok(())
}

/// 键：非空、≤128 字符、不含控制字符（键会原样进 JSON 与界面）。
fn check_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("key 不能为空".to_string());
    }
    if key.chars().count() > MAX_KEY_CHARS {
        return Err(format!("key 太长（上限 {MAX_KEY_CHARS} 字符）"));
    }
    if key.chars().any(|ch| ch.is_control()) {
        return Err("key 不能包含控制字符".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-plugin-kv-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    fn set(root: &Path, id: &str, key: &str, value: Value) -> Value {
        serde_json::from_str(&set_json(
            root,
            &json!({ "id": id, "key": key, "value": value }),
        ))
        .expect("valid json")
    }

    /// 往返：写 → 读 → 列表 → 删 → 再读回 `found=false`。
    #[test]
    fn set_get_list_delete_round_trip() {
        let root = temp_root("roundtrip");
        assert_eq!(set(&root, "pet", "theme", json!("dark"))["ok"], true);

        let got: Value =
            serde_json::from_str(&get_json(&root, "pet", "theme")).expect("valid json");
        assert_eq!(got["ok"], true);
        assert_eq!(got["found"], true);
        assert_eq!(got["value"], "dark");

        // 任意 JSON 都能存（对象 / 数组 / 数字），不只字符串。
        assert_eq!(set(&root, "pet", "window", json!({ "w": 320, "h": 480 }))["ok"], true);
        let list: Value = serde_json::from_str(&list_json(&root, "pet")).expect("valid json");
        assert_eq!(list["ok"], true);
        assert_eq!(list["entries"].as_array().unwrap().len(), 2);
        assert_eq!(list["entries"][0]["key"], "theme");
        assert!(list["bytes"].as_u64().unwrap() > 0);

        let removed: Value =
            serde_json::from_str(&delete_json(&root, "pet", "theme")).expect("valid json");
        assert_eq!(removed["ok"], true);
        assert_eq!(removed["removed"], true);
        let again: Value =
            serde_json::from_str(&get_json(&root, "pet", "theme")).expect("valid json");
        assert_eq!(again["found"], false);

        // 不同插件互不串台。
        let other: Value =
            serde_json::from_str(&get_json(&root, "other", "theme")).expect("valid json");
        assert_eq!(other["found"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 键 / id 的越界闸：`..`、分隔符、超长、控制字符一律拒，且不落盘。
    #[test]
    fn bad_ids_and_keys_are_rejected() {
        let root = temp_root("guards");
        for id in ["", "..", "a/b", "a\\b", "有中文"] {
            let result = set(&root, id, "k", json!(1));
            assert_eq!(result["ok"], false, "必须拒绝 id={id:?}");
        }
        for key in ["", "a\u{0}b", &"k".repeat(MAX_KEY_CHARS + 1)] {
            let result = set(&root, "pet", key, json!(1));
            assert_eq!(result["ok"], false, "必须拒绝 key={key:?}");
        }
        assert!(
            !kv_path(&root, "pet").exists(),
            "被拒绝的写入不能留文件"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 配额：单值超限 / 总量超限都拒绝，且**原有数据保持不变**（原子性）。
    #[test]
    fn quota_is_enforced_without_corrupting_existing_values() {
        let root = temp_root("quota");
        assert_eq!(set(&root, "big", "keep", json!("safe"))["ok"], true);

        let huge = "x".repeat(MAX_VALUE_BYTES + 1);
        let value_result = set(&root, "big", "huge", json!(huge));
        assert_eq!(value_result["ok"], false);
        assert!(
            value_result["error"].as_str().unwrap().contains("KiB"),
            "{}",
            value_result["error"]
        );

        // 总量：每个值 60 KiB，写够 5 个就超过 256 KiB。
        let block = "y".repeat(60 * 1024);
        let mut rejected = false;
        for index in 0..6 {
            let result = set(&root, "big", &format!("b{index}"), json!(block));
            if result["ok"] == false {
                rejected = true;
                assert!(
                    result["error"].as_str().unwrap().contains("单插件上限"),
                    "{}",
                    result["error"]
                );
                break;
            }
        }
        assert!(rejected, "总量超限必须被拒");

        let kept: Value =
            serde_json::from_str(&get_json(&root, "big", "keep")).expect("valid json");
        assert_eq!(kept["value"], "safe", "超限写入不能动到已有值");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 损坏的文件**不自动清空**：读回可读错误，用户自己决定删还是 clear。
    #[test]
    fn a_corrupt_store_is_reported_instead_of_silently_reset() {
        let root = temp_root("corrupt");
        let dir = kv_dir(&root, "pet");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kv.json"), b"not json").unwrap();

        let got: Value =
            serde_json::from_str(&get_json(&root, "pet", "k")).expect("valid json");
        assert_eq!(got["ok"], false);
        assert!(
            got["error"].as_str().unwrap().contains("损坏"),
            "{}",
            got["error"]
        );
        // 原文件必须原地保留（不静默清空）。
        assert_eq!(
            std::fs::read_to_string(dir.join("kv.json")).unwrap(),
            "not json"
        );

        let cleared: Value =
            serde_json::from_str(&clear_json(&root, "pet")).expect("valid json");
        assert_eq!(cleared["ok"], true);
        assert!(!dir.join("kv.json").exists(), "clear 之后可以重来");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 版本不符也拒绝（不猜、不迁移）。
    #[test]
    fn wrong_version_is_refused() {
        let root = temp_root("version");
        let dir = kv_dir(&root, "pet");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kv.json"), br#"{"version":9,"values":{}}"#).unwrap();
        let listed: Value =
            serde_json::from_str(&list_json(&root, "pet")).expect("valid json");
        assert_eq!(listed["ok"], false);
        assert!(listed["error"].as_str().unwrap().contains("版本"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 键数上限：满 256 个之后拒绝**新**键，但允许覆盖已有键。
    #[test]
    fn key_count_cap_allows_overwrites_but_not_new_keys() {
        let root = temp_root("keys");
        for index in 0..MAX_KEYS {
            let result = set(&root, "pet", &format!("k{index}"), json!(index));
            assert_eq!(result["ok"], true, "第 {index} 个键应当写得进去");
        }
        let overflow = set(&root, "pet", "one-more", json!(1));
        assert_eq!(overflow["ok"], false);
        assert!(overflow["error"].as_str().unwrap().contains("键太多"));
        let overwrite = set(&root, "pet", "k0", json!("updated"));
        assert_eq!(overwrite["ok"], true, "覆盖已有键不受键数上限影响");
        let _ = std::fs::remove_dir_all(&root);
    }
}
