//! 数据根目录的运行时注入。v1 与 v2 的可执行文件位置不同，所以不能在 crate 里写死。
use std::path::PathBuf;
use std::sync::OnceLock;

static ROOT: OnceLock<PathBuf> = OnceLock::new();

pub fn init(root: PathBuf) {
    let _ = ROOT.set(root);
}

pub fn lunac_root_dir() -> PathBuf {
    ROOT.get().cloned().unwrap_or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
    })
}

pub fn module_data_dir() -> PathBuf {
    lunac_root_dir().join("ModuleData")
}
