//! 主页面设置（默认搜索引擎 + 自动更新开关）。
//!
//! v1 把搜索引擎存在 `localStorage["lunac-search-engine"]`，右键 Web 搜索行可以切换
//! Google / Bing / 百度。v2 没有 WebView，localStorage 也不在 Rust 侧，所以改成
//! `<exe 根>/config/search.json`（`{"engine":"google"}`），与热键配置同目录。
//!
//! 只影响「Web 搜索兜底行」的标题与打开的 URL；搜索其它数据源不受影响。
//!
//! 自动更新沿用 v1 `updater.rs` 的 `config/update.json`（字段名与文件格式一字不改：
//! `checkOnStartup` / `autoInstall`）。设置面板只有一个开关，读写时两字段同真同假
//! （v1 2026-10-05 的合并语义），默认**开**。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

/// 支持的引擎（顺序 = 右键菜单顺序）。
pub const ENGINES: [&str; 3] = ["google", "bing", "baidu"];

static ROOT: OnceLock<PathBuf> = OnceLock::new();
static ENGINE: AtomicU8 = AtomicU8::new(0);

/// 注入数据根目录并载入配置（幂等；读坏时回落 google）。
pub fn init(root: PathBuf) {
    let engine = load(&root);
    ENGINE.store(index_of(&engine), Ordering::SeqCst);
    let _ = ROOT.set(root);
}

/// 当前引擎名（`google` / `bing` / `baidu`）。
pub fn current() -> &'static str {
    ENGINES[ENGINE.load(Ordering::SeqCst) as usize % ENGINES.len()]
}

/// 当前引擎的展示名（搜索结果行标题用）。
pub fn current_display() -> &'static str {
    display_name(current())
}

pub fn display_name(engine: &str) -> &'static str {
    match engine {
        "bing" => "Bing",
        "baidu" => "百度",
        _ => "Google",
    }
}

/// 切换引擎并持久化；名字非法返回 false（不改当前值）。
pub fn set(engine: &str) -> bool {
    let Some(index) = ENGINES.iter().position(|e| e.eq_ignore_ascii_case(engine)) else {
        return false;
    };
    ENGINE.store(index as u8, Ordering::SeqCst);
    if let Some(root) = ROOT.get() {
        save(root, ENGINES[index]);
    }
    true
}

/// 带查询词的搜索 URL（UTF-8 百分号编码）。
pub fn search_url(query: &str) -> String {
    let query = crate::url_encode(query);
    match current() {
        "bing" => format!("https://www.bing.com/search?q={query}"),
        "baidu" => format!("https://www.baidu.com/s?wd={query}"),
        _ => format!("https://www.google.com/search?q={query}"),
    }
}

/// 空查询时「Web 搜索」行打开的首页。
pub fn home_url() -> &'static str {
    match current() {
        "bing" => "https://www.bing.com",
        "baidu" => "https://www.baidu.com",
        _ => "https://www.google.com",
    }
}

fn config_path(root: &Path) -> PathBuf {
    root.join("config").join("search.json")
}

/// 自动更新配置（v1 `updater::UpdateConfig` 的两个字段，snake_case 只在本侧用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateConfig {
    /// 启动时静默检查一次（默认开）。
    pub check_on_startup: bool,
    /// 发现新版就自动下载并安装（默认开）。
    pub auto_install: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            check_on_startup: true,
            auto_install: true,
        }
    }
}

impl UpdateConfig {
    /// 面板上的单个开关 = v1 的「两个字段都真」。
    #[cfg(test)]
    pub fn enabled(&self) -> bool {
        self.check_on_startup && self.auto_install
    }
}

fn update_path(root: &Path) -> PathBuf {
    root.join("config").join("update.json")
}

/// 读自动更新配置；文件不存在 / 被改坏一律回落默认（**开**），与 v1 同一条判据。
pub fn update_config(root: &Path) -> UpdateConfig {
    let Ok(text) = std::fs::read_to_string(update_path(root)) else {
        return UpdateConfig::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return UpdateConfig::default();
    };
    let default = UpdateConfig::default();
    UpdateConfig {
        check_on_startup: value
            .get("checkOnStartup")
            .and_then(|v| v.as_bool())
            .unwrap_or(default.check_on_startup),
        auto_install: value
            .get("autoInstall")
            .and_then(|v| v.as_bool())
            .unwrap_or(default.auto_install),
    }
}

/// 写单个「自动更新」开关：两字段同真同假，保留文件里其它键（写盘方式与其它配置一致）。
pub fn set_update_auto(root: &Path, on: bool) -> Result<UpdateConfig, String> {
    let path = update_path(root);
    let mut value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    let object = value.as_object_mut().expect("已过滤为对象");
    object.insert("checkOnStartup".into(), serde_json::Value::Bool(on));
    object.insert("autoInstall".into(), serde_json::Value::Bool(on));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("配置目录不可写 —— {e}"))?;
    }
    let text = serde_json::to_string_pretty(&value).map_err(|e| format!("序列化失败 —— {e}"))?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).map_err(|e| format!("写配置失败 —— {e}"))?;
    std::fs::rename(&temp, &path).map_err(|e| format!("写配置失败 —— {e}"))?;
    Ok(update_config(root))
}

fn index_of(engine: &str) -> u8 {
    ENGINES
        .iter()
        .position(|e| e.eq_ignore_ascii_case(engine))
        .unwrap_or(0) as u8
}

fn load(root: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(config_path(root)) else {
        return ENGINES[0].to_string();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return ENGINES[0].to_string();
    };
    value
        .get("engine")
        .and_then(|v| v.as_str())
        .filter(|name| ENGINES.iter().any(|e| e.eq_ignore_ascii_case(name)))
        .unwrap_or(ENGINES[0])
        .to_string()
}

fn save(root: &Path, engine: &str) {
    let path = config_path(root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = format!("{{\"engine\":\"{engine}\"}}");
    if let Err(e) = std::fs::write(&path, text) {
        eprintln!("[lunac-settings] 写配置失败: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_follow_the_selected_engine() {
        // 全局状态：测试结束恢复默认，避免影响其它测试。
        assert!(set("bing"));
        assert_eq!(
            search_url("hello world"),
            "https://www.bing.com/search?q=hello%20world"
        );
        assert!(set("baidu"));
        assert_eq!(search_url("你好"), "https://www.baidu.com/s?wd=%E4%BD%A0%E5%A5%BD");
        assert!(set("google"));
        assert_eq!(home_url(), "https://www.google.com");
        assert_eq!(current_display(), "Google");
    }

    #[test]
    fn rejects_unknown_engine_names() {
        // 不改全局引擎，避免与上面那个测试（会切引擎）在并行时互相干扰。
        assert!(!set("duckduckgo"));
        assert_eq!(display_name("baidu"), "百度");
        assert_eq!(index_of("unknown"), 0);
    }

    #[test]
    fn engine_config_round_trips_on_disk() {
        let dir = std::env::temp_dir().join(format!("lunac-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        save(&dir, "baidu");
        assert_eq!(load(&dir), "baidu");
        assert_eq!(
            std::fs::read_to_string(config_path(&dir)).expect("config file"),
            "{\"engine\":\"baidu\"}"
        );
        save(&dir, "not-an-engine");
        assert_eq!(load(&dir), "google", "非法值回落默认");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_auto_defaults_to_on_when_missing() {
        let dir = std::env::temp_dir().join(format!(
            "lunac-settings-update-default-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let config = update_config(&dir);
        assert!(config.check_on_startup);
        assert!(config.auto_install);
        assert!(config.enabled(), "缺失时默认开（v1 2026-10-05 语义）");

        // 坏文件同样回落默认，而不是当成「关」。
        std::fs::create_dir_all(dir.join("config")).expect("config dir");
        std::fs::write(update_path(&dir), "{not json").expect("write");
        assert!(update_config(&dir).enabled());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_auto_writes_both_fields_and_keeps_other_keys() {
        let dir = std::env::temp_dir().join(format!(
            "lunac-settings-update-write-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join("config")).expect("config dir");
        std::fs::write(
            update_path(&dir),
            "{\"checkOnStartup\":true,\"autoInstall\":true,\"legacy\":1}",
        )
        .expect("write");

        let written = set_update_auto(&dir, false).expect("save");
        assert!(!written.check_on_startup);
        assert!(!written.auto_install);
        assert!(!written.enabled());
        let text = std::fs::read_to_string(update_path(&dir)).expect("config file");
        assert!(text.contains("\"checkOnStartup\": false"), "{text}");
        assert!(text.contains("\"autoInstall\": false"), "{text}");
        assert!(text.contains("\"legacy\": 1"), "保留文件里的其它键：{text}");

        let back = set_update_auto(&dir, true).expect("save");
        assert!(back.enabled());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
