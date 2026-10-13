// win_proc.rs
// Win32 裸 FFI 小工具集（不引第三方依赖）：进程表枚举 + 后台线程降级。
//
// ## 为什么单独成一个模块（2026-10-08 建；2026-10-09 扩用途）
//
// 同一个 Win32 符号在**一个 crate 里只能有一种声明** —— 两处各写一份签名不同的同名
// `extern`，rustc 会报 `clashing_extern_declarations`。所以凡是有多个调用方、又不想引
// 依赖的 Win32 调用都收在这里，各自只调这里暴露的函数：
//   · `list()`：进程表快照 —— `music` 用来探测「Spotify 桌面端在不在跑」（只要映像名）；
//   · `enter_background_mode()`：把当前线程切进 Windows「后台模式」，供索引扫盘让路
//     （见该函数注释；2026-10-09 用户口径「启动扫盘降级」）。
//
// ## 为什么不引依赖 / 不 spawn tasklist
//
// 不引 `sysinfo` 这类新依赖（项目约定不新增依赖）。也不 spawn `tasklist`：它输出的是
// **本地化文本**（`tasklist /FI` 无匹配时打的那句「信息: 没有运行的任务…」随系统语言变），
// 按文本判会误判。`hotkey.rs` 里已有一批手写 `extern "system"` 的同类写法（user32 /
// kernel32 / shell32），这里是同一套。

/// 进程表里能看到的一项。**刻意不存命令行** —— 取命令行要读目标进程的内存，
/// 成本与权限面是另一回事（当前调用方只需要「有哪些映像名」）。
///
/// 2026-10-09：`pid` / `ppid` / `threads` 已随 `activity.rs` 一起撤下 —— 它们唯一的读者
/// 就是那条后台活动自检；现在只剩 `music` 需要映像名。
pub struct ProcEntry {
    /// 映像名（不含路径，如 `Spotify.exe` / `msedgewebview2.exe`）
    pub name: String,
}

/// 当前进程表的一份快照。**读不到就返回空表** —— 调用方只做「有没有」，把失败当
/// 「什么都没有」比抛错更合适（拿不到快照不该打断任何事）。
pub fn list() -> Vec<ProcEntry> {
    #[cfg(target_os = "windows")]
    {
        imp::list()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// 把**当前线程**切进 Windows「后台模式」（`THREAD_MODE_BACKGROUND_BEGIN`）。
///
/// **为什么背景扫盘要用它**（2026-10-09 用户口径「启动扫盘降级」）：它同时下调该线程的
/// **CPU 优先级与文件 I/O 优先级** —— 索引扫盘在游戏 / 前台应用面前**主动让路**，
/// 而不是靠「晚一点再扫」赌时机。索引的正确性不受影响（只是变慢），故没有「首次运行
/// 搜索空窗」那类副作用。失败一律静默：只是没降级，不该打断扫描。
pub fn enter_background_mode() {
    #[cfg(target_os = "windows")]
    imp::enter_background_mode();
}

#[cfg(target_os = "windows")]
mod imp {
    use super::ProcEntry;

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const INVALID_HANDLE_VALUE: isize = -1;
    /// `THREAD_MODE_BACKGROUND_BEGIN`：同时降 CPU 优先级与文件 I/O 优先级。
    const THREAD_MODE_BACKGROUND_BEGIN: i32 = 0x0001_0000;
    /// `MAX_PATH`。`szExeFile` 是定长数组，进程映像名（不含路径）不可能超过它。
    const MAX_PATH: usize = 260;

    #[repr(C)]
    #[allow(non_snake_case)]
    struct PROCESSENTRY32W {
        dwSize: u32,
        cntUsage: u32,
        th32ProcessID: u32,
        th32DefaultHeapID: usize,
        th32ModuleID: u32,
        cntThreads: u32,
        th32ParentProcessID: u32,
        pcPriClassBase: i32,
        dwFlags: u32,
        szExeFile: [u16; MAX_PATH],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> isize;
        fn Process32FirstW(snapshot: isize, entry: *mut PROCESSENTRY32W) -> i32;
        fn Process32NextW(snapshot: isize, entry: *mut PROCESSENTRY32W) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }

    /// 见公开包装的注释。**伪句柄** `GetCurrentThread()` 不需要 `CloseHandle`。
    pub fn enter_background_mode() {
        unsafe {
            // 返回值不查：失败只是没降级，对扫描无害。
            SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
        }
    }

    pub fn list() -> Vec<ProcEntry> {
        let mut out = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == 0 || snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            // `dwSize` 必须在第一次调用前填好，否则 Process32FirstW 直接失败
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut entry) != 0 {
                loop {
                    let len = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(MAX_PATH);
                    out.push(ProcEntry {
                        name: String::from_utf16_lossy(&entry.szExeFile[..len]),
                    });
                    if Process32NextW(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
        out
    }
}
