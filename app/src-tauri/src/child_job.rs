// src/child_job.rs
//! **子进程严格绑定 lunac.exe 的生命周期**（2026-10-02 从 `commands.rs` 提取成共用模块）。
//!
//! 规则（预检 #57，高优先级）：**插件起的任何子进程，都必须随该插件的退出而终结**。
//! 判据是「界面上再没有任何地方能控制它」—— 音乐插件的 librespot 就是活例子：窗口一关
//! 音量 / 暂停 / 切歌全都没了，进程却还在出声、还占着 Spotify 里那台叫 `Lunac` 的设备。
//!
//! 两层机制，缺一不可：
//!
//! ① **显式杀（正常路径）**：句柄还在我们手里时，宿主主动 `kill()`。
//!    librespot 由 `music::on_plugin_window_destroyed`（关窗）与 `music::kill_librespot`
//!    （退程序）负责 —— 这一层**只对长期驻留的进程有意义**，一次性的（PaddleOCR 单图、
//!    ffmpeg 单次转换）跑完自己就退了。
//!
//! ② **Job Object 兜底（异常路径）**：见下。这一层是「进程被崩溃 / 强杀」时唯一的保险 ——
//!    此时 ① 根本没机会执行（我们自己的代码已经不在跑了），而子进程由 OS 记录父子关系，
//!    父进程一死它就成孤儿。**只有内核能在这个时刻替我们动手。**
//!
//! ## 为什么是 Job Object
//!
//! spawn 的子进程由 OS 记录父子关系（任务管理器"进程"页展开 Lunac 分组就能看到；
//! `CREATE_NO_WINDOW` 只是不弹控制台，进程本身可见）。但**父进程崩溃时子进程会变孤儿**。
//! Job Object + `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 保证 lunac.exe 以任何方式退出
//! （含崩溃 / `taskkill /F`）时，Windows 内核自动终止 job 内的全部子进程。
//!
//! **用法：每次 `spawn()` 之后立刻 `assign(&child)`**，不要等到「用完」——
//! 从 spawn 到 assign 之间那个窗口里崩溃，子进程照样是孤儿。
//!
//! ⚠️ 落点是**同一个全局 job**（`OnceLock`）：所有插件子进程同生共死，这正合语义 ——
//! 它们都是 lunac.exe 的孩子。job 句柄**永不关闭**，进程退出时由内核关闭并触发 KILL。

#[cfg(target_os = "windows")]
mod imp {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attrs: *mut std::ffi::c_void, name: *const u16) -> isize;
        fn SetInformationJobObject(
            job: isize,
            class: u32,
            info: *const std::ffi::c_void,
            len: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: isize, process: isize) -> i32;
    }

    static JOB: OnceLock<isize> = OnceLock::new();

    fn handle() -> isize {
        *JOB.get_or_init(|| unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job != 0 {
                let mut info = ExtendedLimits::default();
                info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    job,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    &info as *const _ as *const std::ffi::c_void,
                    std::mem::size_of::<ExtendedLimits>() as u32,
                );
            }
            job // handle 永不关闭 — 进程退出时由内核关闭并触发 KILL
        })
    }

    /// 将子进程加入 job。失败不致命（极老系统不支持嵌套 job），仅记录日志。
    pub fn assign(child: &std::process::Child) {
        let job = handle();
        if job == 0 {
            return;
        }
        unsafe {
            if AssignProcessToJobObject(job, child.as_raw_handle() as isize) == 0 {
                crate::log::warn(format!(
                    "child_job：AssignProcessToJobObject 失败（pid {}）—— 该子进程在宿主崩溃时会成为孤儿",
                    child.id()
                ));
            }
        }
    }
}

/// 把子进程绑到 lunac.exe 的生命周期上（**非 Windows 上是空操作**）。
///
/// 调用点是「每一次 `spawn()` 之后」，不是「用完时」—— 见模块头注释。
pub fn assign(child: &std::process::Child) {
    #[cfg(target_os = "windows")]
    imp::assign(child);
    #[cfg(not(target_os = "windows"))]
    let _ = child;
}
