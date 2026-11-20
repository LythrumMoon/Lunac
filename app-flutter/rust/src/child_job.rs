//! **子进程严格绑定 lunac.exe 的生命周期**（从 v1 `app/src-tauri/src/child_job.rs` 移植）。
//!
//! 规则（v1 预检 #57，高优先级）：**插件起的任何子进程，都必须随该插件的退出而终结**。
//! 显式 `kill()` 只覆盖正常路径；宿主崩溃 / 被强杀时，子进程会变孤儿 —— 只有内核能在
//! 那个时刻替我们动手，所以 spawn 之后立刻把子进程放进 Job Object
//! （`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`）。
//!
//! 两层：
//! ① 全局 job（`assign`）：所有插件子进程同生共死 —— 宿主退出即全部终止；
//! ② 每个 sidecar 自己的 job（`Job`）：额外带内存 / 进程数上限，`Drop` 关闭句柄即收掉
//!    它 fork 出的残留进程。
//!
//! **用法：`spawn()` 之后立刻 assign**，不要等到「用完」—— spawn 到 assign 之间崩溃，
//! 子进程照样是孤儿。

#[cfg(target_os = "windows")]
mod imp {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const JOB_OBJECT_LIMIT_ACTIVE_PROCESS: u32 = 0x0000_0008;
    const JOB_OBJECT_LIMIT_JOB_MEMORY: u32 = 0x0000_0200;
    const JOB_OBJECT_LIMIT_PROCESS_MEMORY: u32 = 0x0000_0100;
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
        fn CloseHandle(handle: isize) -> i32;
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
            job // 句柄永不关闭 —— 进程退出时由内核关闭并触发 KILL
        })
    }

    /// 将子进程加入全局 job。失败不致命（极老系统不支持嵌套 job），只打一行日志。
    pub fn assign(child: &std::process::Child) {
        let job = handle();
        if job == 0 {
            return;
        }
        unsafe {
            if AssignProcessToJobObject(job, child.as_raw_handle() as isize) == 0 {
                eprintln!(
                    "[lunac-plugin] child_job: AssignProcessToJobObject 失败（pid {}）—— 宿主崩溃时该子进程会成为孤儿",
                    child.id()
                );
            }
        }
    }

    /// **每个 sidecar 自己的 job**：带内存 / 进程数上限；`Drop` 关闭句柄即收残留。
    pub struct Job {
        handle: isize,
    }

    impl Job {
        /// 按限额建 job；`0` 表示该维度不限制。
        pub fn create(limits: super::JobLimits) -> Option<Job> {
            let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
            if job == 0 {
                eprintln!("[lunac-plugin] child_job: CreateJobObject 失败，限额未生效");
                return None;
            }
            let mut info = ExtendedLimits::default();
            info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if limits.process_memory_limit > 0 {
                info.basic.limit_flags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
                info.process_memory_limit = limits.process_memory_limit;
            }
            if limits.job_memory_limit > 0 {
                info.basic.limit_flags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
                info.job_memory_limit = limits.job_memory_limit;
            }
            if limits.active_process_limit > 0 {
                info.basic.limit_flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
                info.basic.active_process_limit = limits.active_process_limit;
            }
            let ok = unsafe {
                SetInformationJobObject(
                    job,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    &info as *const _ as *const std::ffi::c_void,
                    std::mem::size_of::<ExtendedLimits>() as u32,
                )
            };
            if ok == 0 {
                eprintln!("[lunac-plugin] child_job: SetInformationJobObject 失败，限额未生效");
                unsafe { CloseHandle(job) };
                return None;
            }
            Some(Job { handle: job })
        }

        pub fn assign(&self, child: &std::process::Child) -> bool {
            unsafe { AssignProcessToJobObject(self.handle, child.as_raw_handle() as isize) != 0 }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // 关闭句柄 ⇒ KILL_ON_JOB_CLOSE：正常 stop 时也顺带收掉残留子进程。
            unsafe { CloseHandle(self.handle) };
        }
    }
}

/// 把子进程绑到 lunac.exe 的生命周期上（**非 Windows 上是空操作**）。
pub fn assign(child: &std::process::Child) {
    #[cfg(target_os = "windows")]
    imp::assign(child);
    #[cfg(not(target_os = "windows"))]
    let _ = child;
}

/// 每个进程的**独立 job 限额**（`0` = 该维度不限制）。
#[derive(Debug, Clone, Copy, Default)]
pub struct JobLimits {
    /// 单进程内存上限（字节）
    pub process_memory_limit: usize,
    /// 整个 job 的内存上限（字节）
    pub job_memory_limit: usize,
    /// job 内最大进程数
    pub active_process_limit: u32,
}

impl JobLimits {
    pub fn is_set(&self) -> bool {
        self.process_memory_limit > 0 || self.job_memory_limit > 0 || self.active_process_limit > 0
    }
}

#[cfg(target_os = "windows")]
pub use imp::Job;

/// 非 Windows：没有 Job Object（限额是**资源闸**，不是隔离；不支持时静默降级）。
#[cfg(not(target_os = "windows"))]
pub struct Job;

#[cfg(not(target_os = "windows"))]
impl Job {
    pub fn create(_limits: JobLimits) -> Option<Job> {
        None
    }
    pub fn assign(&self, _child: &std::process::Child) -> bool {
        false
    }
}
