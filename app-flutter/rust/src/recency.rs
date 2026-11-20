//! 最近打开时间（v1 的 recency）：让「最近在 Lunac 里打开过的应用 / 插件」排前面。
//!
//! v1 存在 `localStorage["lunac-recency"]`（键 `app://<path>` / `plugin://<id>`），
//! **值是 `Date.now()`**；排序时按时间降序，没打开过的排最后。v2 的数据都落在
//! exe 根目录，所以这里存 `<exe 根>/data/usage.json`（`{ "app:<path>": 毫秒时间戳 }`），
//! 跨重启保留。
//!
//! 只记录**结果动作真的被执行**的条目（见 `lunac_run_action`）；搜索只读内存表，
//! 不做磁盘 IO。文件很小（上限 2000 条），写入用临时文件 + rename 保证不写坏。
//!
//! 兼容旧版 `usage.json`：旧值是「使用次数」（小整数），不是时间戳。读取时原样保留，
//! 但排序上它们远小于任何真实毫秒时间戳 ⇒ 视作「很久没打开」，下一次打开会被时间戳覆盖。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 数据根目录（= 宿主可执行文件所在目录），由 `init()` 注入。
static ROOT: OnceLock<PathBuf> = OnceLock::new();
/// 内存里的最近打开时间表；`init()` 之前为 None（记录与查询都退化为空）。
static USAGE: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();

/// 表上限：超过后只保留时间最新的 2000 条（防止无界增长）。
const MAX_ENTRIES: usize = 2000;

/// 真实毫秒时间戳的最小值（2001-09-09）：小于它的旧值一定是「次数」，按从未打开处理。
const MIN_TIMESTAMP_MS: u64 = 1_000_000_000_000;

fn usage_path(root: &Path) -> PathBuf {
    root.join("data").join("usage.json")
}

/// 启动时调用一次：注入根目录并载入最近打开时间表。
pub fn init(root: PathBuf) {
    let map = load(&root);
    let _ = ROOT.set(root);
    let _ = USAGE.set(Mutex::new(map));
}

fn load(root: &Path) -> HashMap<String, u64> {
    let Ok(text) = std::fs::read_to_string(usage_path(root)) else {
        return HashMap::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return HashMap::new();
    };
    let Some(object) = value.as_object() else {
        return HashMap::new();
    };
    object
        .iter()
        .filter_map(|(key, value)| {
            let stamp = value.as_u64()?;
            // 旧版存的是次数：保留键但把时间清零（排序时等于「很久没打开」）。
            Some((key.clone(), if stamp >= MIN_TIMESTAMP_MS { stamp } else { 0 }))
        })
        .collect()
}

/// 记录一次「在 Lunac 里打开」（结果动作执行成功后调用）。
pub fn record(id: &str) {
    let Some(usage) = USAGE.get() else {
        return;
    };
    let Ok(mut map) = usage.lock() else {
        return;
    };
    map.insert(id.to_string(), crate::cron::now_ms());
    if map.len() > MAX_ENTRIES {
        let mut items: Vec<(String, u64)> = map.iter().map(|(k, v)| (k.clone(), *v)).collect();
        items.sort_by_key(|(_, stamp)| std::cmp::Reverse(*stamp));
        items.truncate(MAX_ENTRIES);
        *map = items.into_iter().collect();
    }
    if let Some(root) = ROOT.get() {
        save(root, &map);
    }
}

fn save(root: &Path, map: &HashMap<String, u64>) {
    let path = usage_path(root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(text) = serde_json::to_string(map) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// 某个 id 的最近打开时间（毫秒）；未知 / 旧版次数记录 = 0（排最后）。
pub fn score(id: &str) -> u64 {
    USAGE
        .get()
        .and_then(|usage| usage.lock().ok().and_then(|map| map.get(id).copied()))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_ids_score_zero_before_init() {
        assert_eq!(score("app:C:\\nope.exe"), 0);
        // init 之前 record 是 no-op，不应 panic。
        record("app:C:\\nope.exe");
    }

    #[test]
    fn usage_file_is_under_data_dir() {
        assert!(usage_path(Path::new("D:\\Lunac")).ends_with("data/usage.json"));
    }

    #[test]
    fn legacy_counts_are_treated_as_never_opened() {
        let dir = std::env::temp_dir().join(format!(
            "lunac-recency-legacy-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(
            usage_path(&dir),
            r#"{"app:C:\\old.exe": 7, "plugin:ai": 1720000000000}"#,
        )
        .unwrap();
        let map = load(&dir);
        assert_eq!(map["app:C:\\old.exe"], 0, "旧版次数按「从未打开」处理");
        assert_eq!(map["plugin:ai"], 1_720_000_000_000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
