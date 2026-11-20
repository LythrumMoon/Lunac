//! 极薄的日志门面：默认写 stderr，宿主可接管。索引模块只需要 info / warn 两级。
use std::sync::OnceLock;

type Logger = fn(&str, &str);
static LOGGER: OnceLock<Logger> = OnceLock::new();

pub fn set_logger(logger: Logger) {
    let _ = LOGGER.set(logger);
}

pub fn info(msg: impl AsRef<str>) {
    emit("info", msg.as_ref());
}

pub fn warn(msg: impl AsRef<str>) {
    emit("warn", msg.as_ref());
}

fn emit(level: &str, msg: &str) {
    if let Some(logger) = LOGGER.get() {
        logger(level, msg);
    } else {
        eprintln!("[lunac-indexer][{level}] {msg}");
    }
}
