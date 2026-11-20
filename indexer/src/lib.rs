//! 应用 / 文件索引：v1（Tauri 宿主）与 v2（Flutter 薄层）共用的唯一实现。
//!
//! 两个业务模块原本在 `app/src-tauri/src/` 下，直接引用 `crate::storage` /
//! `crate::log` / `crate::win_proc`。收进本 crate 后仍保留同名内部模块（shim），
//! 所以业务代码零改动：
//!   · storage：根目录由宿主用 `init()` 注入（v1 / v2 都传可执行文件所在目录）；
//!   · log：默认写 stderr，宿主可用 `set_logger()` 接管（v1 接自己的落盘日志）；
//!   · win_proc：后台降级（Windows 下调当前线程的 CPU / IO 优先级）。
use std::path::PathBuf;

pub mod app_indexer;
pub mod file_indexer;
pub mod icon;
mod log;
pub mod pinyin;
mod storage;
mod win_proc;

/// 注入数据根目录（= 宿主可执行文件所在目录）。进程内只需调用一次；重复调用被忽略。
pub fn init(root_dir: PathBuf) {
    storage::init(root_dir);
}

/// 接管日志：level 为 "info" 或 "warn"。不设置时写 stderr。
pub fn set_logger(logger: fn(&str, &str)) {
    log::set_logger(logger);
}
