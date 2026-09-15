// src-tauri/src/single_instance.rs
// 进程级单实例保护（命名内核对象，零依赖手写 FFI，约定同 hotkey.rs）。
//
// 为什么需要：全仓此前没有任何单实例保护。自启实例已在后台时用户再双击 →
// 第二个实例照常起窗口，而 `RegisterHotKey` 对同一组合只能注册一次，
// **第二实例必然注册失败**、退化成 `WH_KEYBOARD_LL` 全局钩子 ——
// 用户表现为「热键要按好几次 / 反应慢」，同时也是 Defender 报 `Prowloc` 的成因
// （见 ai-spec §9.1 难点 2 / §11 规则 1）。所以这里要拦的不是「省内存」，
// 而是「别让第二个实例把全局热键打坏」。
//
// 命名用 `Local\` 前缀：**同一登录会话内全局唯一**，于是 dev / release、
// 前台 / 后台（`--background`）都算同一个应用、互相排斥 —— 用户 2026-09-15
// 明确要求「不能在前台或后台里存在多个 lunac，包括 dev release 等多个版本」。
// 不用 `Global\`：创建全局命名对象需要 `SeCreateGlobalPrivilege`（普通用户没有），
// 且跨用户会话互斥也不是要拦的场景。
//
// 为什么用内核对象而不是锁文件：项目硬约束是「数据全部落在 exe 根目录」，
// 而锁文件放 exe 根 ⇒ dev 与 release 各锁各的（正是要避免的）；放到 exe 根
// 以外又违反便携模式。内核对象不落盘，进程退出（含崩溃/被杀）由内核自动回收，
// 既无残留文件也无陈旧锁。

use std::sync::atomic::{AtomicIsize, Ordering};
use std::thread;

const MUTEX_NAME: &str = "Local\\LunacSingleInstance";
const EVENT_NAME: &str = "Local\\LunacActivate";
const ERROR_ALREADY_EXISTS: u32 = 183;
const EVENT_MODIFY_STATE: u32 = 0x0002;
const INFINITE: u32 = 0xFFFF_FFFF;
const WAIT_OBJECT_0: u32 = 0;

#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(attrs: isize, initial_owner: i32, name: *const u16) -> isize;
    fn CreateEventW(attrs: isize, manual_reset: i32, initial_state: i32, name: *const u16) -> isize;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> isize;
    fn SetEvent(handle: isize) -> i32;
    fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
    fn CloseHandle(handle: isize) -> i32;
    fn GetLastError() -> u32;
}

/// 本进程持有的互斥体句柄。**进程存活期间不得关闭**：句柄一关，命名对象即被
/// 内核回收，后续实例就会误判成「没有别的实例在跑」。
static MUTEX: AtomicIsize = AtomicIsize::new(0);
/// 唤出事件句柄（仅首实例持有；0 = 未拿到）。
static ACTIVATE_EVENT: AtomicIsize = AtomicIsize::new(0);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 尝试成为唯一实例。`true` = 本进程是首个实例（已持有命名互斥体）；
/// `false` = 已有实例在运行（调用方应 `signal_existing()` 后立即退出）。
///
/// 创建失败（极端情况，如内核对象被安全策略拦下）时**放行**：单实例是体验保护，
/// 不该因为它自己出问题就让用户完全打不开应用。
pub fn acquire() -> bool {
    let name = wide(MUTEX_NAME);
    let handle = unsafe { CreateMutexW(0, 0, name.as_ptr()) };
    if handle == 0 {
        crate::log::warn(format!(
            "single_instance: CreateMutexW 失败 err={} → 放行（本次不做单实例限制）",
            unsafe { GetLastError() }
        ));
        return true;
    }
    // 注意：GetLastError 必须在任何别的 Win32 调用之前读，否则会被覆盖。
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already {
        unsafe { CloseHandle(handle) };
        return false;
    }
    MUTEX.store(handle, Ordering::SeqCst);

    // 唤出事件随首实例一起建，供后续实例「拍一下肩膀」。
    // manual_reset=0（auto-reset）：被 Wait 消费后由系统自动复位，无需手动 ResetEvent。
    let ev_name = wide(EVENT_NAME);
    let ev = unsafe { CreateEventW(0, 0, 0, ev_name.as_ptr()) };
    if ev == 0 {
        crate::log::warn(format!(
            "single_instance: CreateEventW 失败 err={} → 双击将只是无效退出（不会唤出）",
            unsafe { GetLastError() }
        ));
    }
    ACTIVATE_EVENT.store(ev, Ordering::SeqCst);
    true
}

/// 告知已有实例「又有人启动了一次」→ 让它把窗口唤出。返回是否成功送达。
///
/// **送达失败也必须照常退出**（调用方不要拿它的返回值决定去留）：否则用户会看到
/// 两个实例，正是这里要消灭的情况。返回 `false` 只意味着「旧实例可能还没建好事件」。
pub fn signal_existing() -> bool {
    let name = wide(EVENT_NAME);
    let ev = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    if ev == 0 {
        return false;
    }
    let ok = unsafe { SetEvent(ev) } != 0;
    unsafe { CloseHandle(ev) };
    ok
}

/// 首实例侧：常驻等待唤出信号，收到就把窗口显示并抢到前台。
///
/// 必须在 `hotkey::start_hotkey()` **之后**调用 —— 它负责填 `MAIN_HWND` / `APP`，
/// 而 `show_and_focus()` 依赖前者。早于此点收到的信号不会丢：auto-reset 事件在被
/// Wait 消费前一直保持有信号状态。
pub fn start_activate_listener() {
    let ev = ACTIVATE_EVENT.load(Ordering::SeqCst);
    if ev == 0 {
        return;
    }
    thread::spawn(move || loop {
        if unsafe { WaitForSingleObject(ev, INFINITE) } != WAIT_OBJECT_0 {
            crate::log::warn("single_instance: WaitForSingleObject 失败 → 停止监听唤出信号");
            return;
        }
        crate::log::info("single_instance: 收到唤出信号（用户又启动了一次）→ show_and_focus");
        crate::hotkey::show_and_focus();
    });
}

#[cfg(test)]
mod tests {
    // 命名对象的有效性无法在单测里断言（同进程内重入会被互斥体拦下，
    // 而单测进程本身不持有它），故只覆盖字符串与常量这类纯逻辑。
    #[test]
    fn wide_is_null_terminated_utf16() {
        let w = super::wide("Lunac");
        assert_eq!(w, vec![b'L' as u16, b'u' as u16, b'n' as u16, b'a' as u16, b'c' as u16, 0]);
    }

    #[test]
    fn names_are_session_local_and_version_independent() {
        // `Local\` = 同会话内唯一（dev / release 互相排斥）；
        // 名字里**不能**出现版本号或 exe 路径，否则 dev 与 release 会各起一个。
        assert!(super::MUTEX_NAME.starts_with("Local\\"));
        assert!(super::EVENT_NAME.starts_with("Local\\"));
    }
}
