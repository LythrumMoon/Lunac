//! 单实例保护 + 「再启动一次就唤出窗口」。
//!
//! 实现与 v1 `app/src-tauri/src/single_instance.rs` 同思路：命名内核对象（零依赖手写 FFI），
//! 不落锁文件（进程退出 / 被杀由内核自动回收，无陈旧锁）。
//!
//! **命名故意带 `V2`**（`Local\LunacV2SingleInstance`）：迁移期 v1 与 v2 可同时运行，
//! 方便对照；否则开着 v1 时 v2 会静默退出，看起来像「v2 起不来」。M6 替换 v1 时
//! 把这两个名字改回 v1 的 `Local\LunacSingleInstance` / `Local\LunacActivate` 即可。
//!
//! 第二个实例的行为：`acquire()` 返回 false → 调用方 `signal_existing()` 后退出。
//! 首实例的监听线程收到信号后只累加唤出计数（`hotkey::request_wake`），
//! 由 Dart 的轮询统一处理显示 / 隐藏 —— 与热键走同一条路，避免两套唤出逻辑。

#[cfg(target_os = "windows")]
mod win {
    pub const MUTEX_NAME: &str = "Local\\LunacV2SingleInstance";
    pub const EVENT_NAME: &str = "Local\\LunacV2Activate";
    pub const ERROR_ALREADY_EXISTS: u32 = 183;
    pub const EVENT_MODIFY_STATE: u32 = 0x0002;
    pub const INFINITE: u32 = 0xFFFF_FFFF;
    pub const WAIT_OBJECT_0: u32 = 0;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn CreateMutexW(attrs: isize, initial_owner: i32, name: *const u16) -> isize;
        pub fn CreateEventW(attrs: isize, manual_reset: i32, initial_state: i32, name: *const u16)
            -> isize;
        pub fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> isize;
        pub fn SetEvent(handle: isize) -> i32;
        pub fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
        pub fn CloseHandle(handle: isize) -> i32;
        pub fn GetLastError() -> u32;
    }
}

#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicIsize, Ordering};

/// 首实例持有的互斥体句柄。**进程存活期间不得关闭**：句柄一关命名对象就被回收，
/// 后续实例会误判成「没有别的实例在跑」。
#[cfg(target_os = "windows")]
static MUTEX: AtomicIsize = AtomicIsize::new(0);

/// 唤出事件句柄（仅首实例持有；0 = 未拿到）。
#[cfg(target_os = "windows")]
static ACTIVATE_EVENT: AtomicIsize = AtomicIsize::new(0);

#[cfg(target_os = "windows")]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 尝试成为唯一实例。`true` = 首个实例；`false` = 已有实例在跑（调用方 signal 后退出）。
///
/// 创建失败（极端情况，如安全策略拦下）时**放行**：单实例是体验保护，
/// 不该因为它自己出问题就让用户完全打不开应用。
#[cfg(target_os = "windows")]
pub fn acquire() -> bool {
    use win::*;

    let name = wide(MUTEX_NAME);
    let handle = unsafe { CreateMutexW(0, 0, name.as_ptr()) };
    if handle == 0 {
        eprintln!("[lunac-single] CreateMutexW 失败 → 放行（本次不做单实例限制）");
        return true;
    }
    // GetLastError 必须在任何别的 Win32 调用之前读，否则会被覆盖。
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already {
        unsafe { CloseHandle(handle) };
        return false;
    }
    MUTEX.store(handle, Ordering::SeqCst);

    // 唤出事件随首实例一起建：auto-reset（manual_reset=0），被 Wait 消费后系统自动复位。
    let event_name = wide(EVENT_NAME);
    let event = unsafe { CreateEventW(0, 0, 0, event_name.as_ptr()) };
    if event == 0 {
        eprintln!("[lunac-single] CreateEventW 失败 → 再次启动不会唤出窗口");
    }
    ACTIVATE_EVENT.store(event, Ordering::SeqCst);
    if event != 0 {
        start_activate_listener();
    }
    true
}

#[cfg(not(target_os = "windows"))]
pub fn acquire() -> bool {
    // Linux 的单实例（抽象 socket / 锁）留到 Linux 补验时一起做。
    true
}

/// 首实例侧：常驻等待唤出信号，收到后累加唤出计数。
#[cfg(target_os = "windows")]
fn start_activate_listener() {
    use win::*;

    let event = ACTIVATE_EVENT.load(Ordering::SeqCst);
    if event == 0 {
        return;
    }
    std::thread::spawn(move || loop {
        if unsafe { WaitForSingleObject(event, INFINITE) } != WAIT_OBJECT_0 {
            eprintln!("[lunac-single] WaitForSingleObject 失败 → 停止监听唤出信号");
            return;
        }
        eprintln!("[lunac-single] 收到唤出信号（用户又启动了一次）");
        crate::hotkey::request_activate();
    });
}

/// 告知已有实例「又有人启动了一次」。返回是否送达（失败也必须照常退出，
/// 否则用户会看到两个窗口）。
#[cfg(target_os = "windows")]
pub fn signal_existing() -> bool {
    use win::*;

    let name = wide(EVENT_NAME);
    let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    if event == 0 {
        return false;
    }
    let ok = unsafe { SetEvent(event) } != 0;
    unsafe { CloseHandle(event) };
    ok
}

#[cfg(not(target_os = "windows"))]
pub fn signal_existing() -> bool {
    false
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn wide_is_null_terminated_utf16() {
        assert_eq!(
            wide("Lunac"),
            vec![b'L' as u16, b'u' as u16, b'n' as u16, b'a' as u16, b'c' as u16, 0]
        );
    }

    #[test]
    fn names_are_session_local_and_v2_scoped() {
        // Local\ = 同会话内唯一；带 V2 是为了迁移期不与 v1 互斥（见文件头注释）。
        assert!(win::MUTEX_NAME.starts_with("Local\\LunacV2"));
        assert!(win::EVENT_NAME.starts_with("Local\\LunacV2"));
    }
}
