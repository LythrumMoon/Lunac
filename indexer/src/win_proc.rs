//! 扫盘线程降级：Windows 下把当前线程切进后台模式（CPU + 文件 IO 优先级同时下调）。
//! 失败一律静默：只是没降级，不影响扫描正确性。

pub fn enter_background_mode() {
    #[cfg(target_os = "windows")]
    unsafe {
        const THREAD_MODE_BACKGROUND_BEGIN: i32 = 0x0001_0000;

        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentThread() -> isize;
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
        }

        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}
