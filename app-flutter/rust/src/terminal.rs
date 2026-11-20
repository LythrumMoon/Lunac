//! 预览区终端（Windows）。
//!
//! 口径（用户 2026-11-15：预览区做成「小 VSCode」，里面要有终端）：
//!   · **优先真伪终端**：`CreatePseudoConsole` + 子进程（`pwsh` → `powershell.exe`），
//!     这样 `cd` / 交互式程序 / 颜色都能用；
//!   · **伪终端不可用时降级为管道模式**：`cmd.exe /Q /K` 的 stdin/stdout 走匿名管道，
//!     逐行执行、状态（cwd / 变量）在同一进程里保留。功能少一截（没有全屏交互程序），
//!     但任何 Windows 都能跑；界面上的标签会如实写「管道模式」。
//!     （本机就是这一档：`CreatePseudoConsole` 在这台机器的 Windows 11 26100/26200 上
//!     直接返回 `0x80070006`（ERROR_INVALID_HANDLE），用纯 PowerShell P/Invoke 复现，
//!     与调用方式无关 —— 见 docs/todo.md M5z-补71。）
//!   · **cwd = 沙箱盘**（与 agent 同一个受控工作目录），终端里改的文件和 AI 看到的是同一份；
//!   · **单向缓冲**：输出由后台线程读进环形缓冲，Dart 每 120 ms 轮询一次
//!     （C ABI 不能从工作线程回调进 Flutter engine，只能轮询）；
//!   · 一个进程只开一个终端；关标签 / 重开前先关掉。
//!
//! 为什么手写而不用 `portable-pty`：本机 cargo registry 没有它的缓存（离线拉不到），
//! 而 ConPTY 只需要 5 个 API，手写比引一个新依赖更可控。

use std::collections::VecDeque;

/// 未完成的 UTF-8 尾巴：整块解码前把它留到下一轮，避免半个汉字变成乱码方块。
fn incomplete_tail(bytes: &[u8]) -> usize {
    let n = bytes.len();
    for back in 1..=core::cmp::min(3, n) {
        let byte = bytes[n - back];
        if byte < 0x80 {
            // ASCII：末尾完整，不用留。
            return 0;
        }
        if (0x80..=0xBF).contains(&byte) {
            // 续字节：继续往前找开头字节。
            continue;
        }
        let need = match byte {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => return 0,
        };
        return if back < need { back } else { 0 };
    }
    0
}

/// 从缓冲区取走全部输出（排空），末尾不完整的 UTF-8 序列留到下一轮。
fn drain_utf8(buffer: &std::sync::Mutex<VecDeque<u8>>) -> String {
    let mut guard = buffer.lock().unwrap_or_else(|error| error.into_inner());
    let mut all: Vec<u8> = guard.drain(..).collect();
    let keep = incomplete_tail(&all);
    if keep > 0 {
        for byte in all.split_off(all.len() - keep) {
            guard.push_back(byte);
        }
    }
    drop(guard);
    String::from_utf8_lossy(&all).into_owned()
}

#[cfg(windows)]
pub use imp::{read_json, resize_json, start_json, stop_json, write_json};

#[cfg(windows)]
mod imp {
    use std::collections::VecDeque;
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use serde_json::{json, Value};
    use windows::core::{Error as WinError, BOOL, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::SECURITY_ATTRIBUTES;
    use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    use windows::Win32::System::Console::{
        ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
    };
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
        TerminateProcess, UpdateProcThreadAttribute, CREATE_NO_WINDOW,
        EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
        PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW,
    };

    /// 输出缓冲上限（超出丢最早的）：Dart 每 120 ms 排空一次，正常远到不了这里。
    const MAX_PENDING_BYTES: usize = 256 * 1024;

    struct Term {
        /// 伪终端句柄；`None` = 管道模式。
        pty: Option<HPCON>,
        input: HANDLE,
        output: HANDLE,
        process: HANDLE,
        buffer: Arc<Mutex<VecDeque<u8>>>,
        alive: Arc<AtomicBool>,
        reader: Option<thread::JoinHandle<()>>,
        /// `conpty` / `pipes`（界面据此显示能力边界）。
        mode: &'static str,
    }

    // HPCON / HANDLE 是裸指针；句柄生命周期由本模块的全局锁串起来。
    unsafe impl Send for Term {}

    /// 读者线程需要拿走输出句柄：`HANDLE` 本身不是 `Send`，包一层显式声明。
    #[derive(Clone, Copy)]
    struct SendHandle(HANDLE);

    unsafe impl Send for SendHandle {}

    static TERM: Mutex<Option<Term>> = Mutex::new(None);

    fn error_json(message: &str) -> String {
        json!({ "ok": false, "error": message }).to_string()
    }

    /// 读者线程的主体：一直读输出端，直到管道断开。
    fn read_loop(handle: SendHandle, buffer: Arc<Mutex<VecDeque<u8>>>, alive: Arc<AtomicBool>) {
        let mut chunk = [0u8; 8192];
        loop {
            let mut read = 0u32;
            let ok = unsafe { ReadFile(handle.0, Some(&mut chunk), Some(&mut read), None) };
            if ok.is_err() || read == 0 {
                alive.store(false, Ordering::SeqCst);
                break;
            }
            let mut guard = buffer.lock().unwrap_or_else(|error| error.into_inner());
            guard.extend(&chunk[..read as usize]);
            while guard.len() > MAX_PENDING_BYTES {
                guard.pop_front();
            }
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn program_files_path(relative: &str) -> Option<PathBuf> {
        let root = std::env::var("ProgramFiles").ok()?;
        let path = PathBuf::from(root).join(relative);
        path.is_file().then_some(path)
    }

    fn system32_path(relative: &str) -> Option<PathBuf> {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let path = PathBuf::from(root).join("System32").join(relative);
        path.is_file().then_some(path)
    }

    /// 伪终端模式用的 shell：pwsh → Windows PowerShell。
    ///
    /// 带 `-NoExit`：ConPTY 下没有「交互式输入」的判定，进程必须自己赖着不退出。
    fn conpty_shell() -> Option<(PathBuf, String)> {
        let candidates = [
            program_files_path("PowerShell\\7\\pwsh.exe"),
            system32_path("WindowsPowerShell\\v1.0\\powershell.exe"),
        ];
        for exe in candidates.into_iter().flatten() {
            // 输出强制 UTF-8：中文系统的控制台默认是 OEM 代码页（GBK）。
            let command = format!(
                "\"{}\" -NoLogo -NoExit -Command \"[Console]::OutputEncoding=[Text.Encoding]::UTF8\"",
                exe.display()
            );
            return Some((exe, command));
        }
        None
    }

    /// 管道模式用的 shell：`cmd.exe /Q /K`。
    ///
    /// `/Q` 关掉命令回显、`/K` 保持进程；stdin 重定向后它会逐行读取并执行，
    /// 提示符（`D:\…>`）会自己打出来 —— 这是失败降级档里最接近终端的行为。
    fn pipe_shell() -> (PathBuf, String) {
        let exe = system32_path("cmd.exe")
            .unwrap_or_else(|| PathBuf::from("C:\\Windows\\System32\\cmd.exe"));
        let command = format!("\"{}\" /Q /K chcp 65001>nul", exe.display());
        (exe, command)
    }

    /// 起一个新的终端（已存在且还活着就复用）。
    pub fn start_json(json_text: &str, exe_root: &Path) -> String {
        let value: Value = serde_json::from_str(json_text).unwrap_or(Value::Null);
        let cols = value
            .get("cols")
            .and_then(Value::as_u64)
            .unwrap_or(120)
            .clamp(20, 400) as i16;
        let rows = value
            .get("rows")
            .and_then(Value::as_u64)
            .unwrap_or(30)
            .clamp(5, 200) as i16;

        let mut slot = TERM.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(existing) = slot.as_ref() {
            if existing.alive.load(Ordering::SeqCst) {
                return json!({
                    "ok": true, "reused": true, "mode": existing.mode, "error": ""
                })
                .to_string();
            }
        }
        if let Some(old) = slot.take() {
            // 旧进程已经退出（读到 EOF）：先把句柄还回去再重开。
            close(old);
        }

        let cwd = crate::sandbox::prepare(exe_root).unwrap_or_else(|_| exe_root.to_path_buf());
        let mut warning = String::new();
        let started = match start_conpty(cols, rows, &cwd) {
            Ok(term) => Ok(term),
            Err(reason) => {
                warning = reason;
                start_pipes(&cwd)
            }
        };
        match started {
            Ok(term) => {
                let mode = term.mode;
                *slot = Some(term);
                json!({
                    "ok": true,
                    "reused": false,
                    "mode": mode,
                    "warning": warning,
                    "error": ""
                })
                .to_string()
            }
            Err(reason) => error_json(&reason),
        }
    }

    /// 伪终端档：ConPTY + 子进程。
    fn start_conpty(cols: i16, rows: i16, cwd: &Path) -> Result<Term, String> {
        let Some((shell, command_line)) = conpty_shell() else {
            return Err("找不到 PowerShell，已改用管道模式".into());
        };
        unsafe {
            let mut input_read = HANDLE(std::ptr::null_mut());
            let mut input_write = HANDLE(std::ptr::null_mut());
            let mut output_read = HANDLE(std::ptr::null_mut());
            let mut output_write = HANDLE(std::ptr::null_mut());
            if CreatePipe(&mut input_read, &mut input_write, None, 0).is_err() {
                return Err("创建输入管道失败".into());
            }
            if CreatePipe(&mut output_read, &mut output_write, None, 0).is_err() {
                let _ = CloseHandle(input_read);
                let _ = CloseHandle(input_write);
                return Err("创建输出管道失败".into());
            }
            let pty = match CreatePseudoConsole(
                COORD { X: cols, Y: rows },
                input_read,
                output_write,
                0,
            ) {
                Ok(pty) => pty,
                Err(error) => {
                    let _ = CloseHandle(input_read);
                    let _ = CloseHandle(input_write);
                    let _ = CloseHandle(output_read);
                    let _ = CloseHandle(output_write);
                    return Err(format!("系统不支持伪终端：{error}"));
                }
            };
            // 这两端已经交给伪终端，父进程不再持有。
            let _ = CloseHandle(input_read);
            let _ = CloseHandle(output_write);

            // 属性表：先问大小，再按 usize 对齐分配（只保证 1 字节对齐的缓冲不能直接用）。
            let mut attribute_size = 0usize;
            let _ = InitializeProcThreadAttributeList(None, 1, None, &mut attribute_size);
            let unit = size_of::<usize>();
            let units = ((attribute_size + unit - 1) / unit).max(1);
            let mut attribute_storage = vec![0usize; units];
            let attributes =
                LPPROC_THREAD_ATTRIBUTE_LIST(attribute_storage.as_mut_ptr() as *mut c_void);
            if InitializeProcThreadAttributeList(Some(attributes), 1, None, &mut attribute_size)
                .is_err()
            {
                let _ = CloseHandle(input_write);
                let _ = CloseHandle(output_read);
                ClosePseudoConsole(pty);
                return Err("初始化进程属性失败".into());
            }
            let updated = UpdateProcThreadAttribute(
                attributes,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(&pty as *const HPCON as *const c_void),
                size_of::<HPCON>(),
                None,
                None,
            );

            let mut startup = STARTUPINFOEXW::default();
            startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            startup.lpAttributeList = attributes;

            let shell_wide = wide(&shell.to_string_lossy());
            let mut command_wide = wide(&command_line);
            let cwd_wide = wide(&cwd.to_string_lossy());
            let mut info: PROCESS_INFORMATION = zeroed();
            let created = if updated.is_ok() {
                CreateProcessW(
                    PCWSTR(shell_wide.as_ptr()),
                    Some(PWSTR(command_wide.as_mut_ptr())),
                    None,
                    None,
                    false,
                    EXTENDED_STARTUPINFO_PRESENT,
                    None,
                    PCWSTR(cwd_wide.as_ptr()),
                    &startup.StartupInfo,
                    &mut info,
                )
            } else {
                Err(WinError::from_win32())
            };
            DeleteProcThreadAttributeList(attributes);

            if let Err(error) = created {
                let _ = CloseHandle(input_write);
                let _ = CloseHandle(output_read);
                ClosePseudoConsole(pty);
                return Err(format!("启动 shell 失败：{error}"));
            }
            let _ = CloseHandle(info.hThread);
            Ok(build_term(
                Some(pty),
                input_write,
                output_read,
                info.hProcess,
                "conpty",
            ))
        }
    }

    /// 管道档：`cmd.exe /Q /K` 的 stdin/stdout 走匿名管道（逐行执行，状态保留）。
    fn start_pipes(cwd: &Path) -> Result<Term, String> {
        let (shell, command_line) = pipe_shell();
        // 子进程要继承这两端，其余句柄不继承。
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: BOOL(1),
        };
        unsafe {
            let mut stdin_read = HANDLE(std::ptr::null_mut());
            let mut stdin_write = HANDLE(std::ptr::null_mut());
            let mut stdout_read = HANDLE(std::ptr::null_mut());
            let mut stdout_write = HANDLE(std::ptr::null_mut());
            if CreatePipe(&mut stdin_read, &mut stdin_write, Some(&attributes), 0).is_err() {
                return Err("创建输入管道失败".into());
            }
            if CreatePipe(&mut stdout_read, &mut stdout_write, Some(&attributes), 0).is_err() {
                let _ = CloseHandle(stdin_read);
                let _ = CloseHandle(stdin_write);
                return Err("创建输出管道失败".into());
            }
            let mut startup: STARTUPINFOW = zeroed();
            startup.cb = size_of::<STARTUPINFOW>() as u32;
            startup.dwFlags = STARTF_USESTDHANDLES;
            startup.hStdInput = stdin_read;
            startup.hStdOutput = stdout_write;
            startup.hStdError = stdout_write;

            let shell_wide = wide(&shell.to_string_lossy());
            let mut command_wide = wide(&command_line);
            let cwd_wide = wide(&cwd.to_string_lossy());
            let mut info: PROCESS_INFORMATION = zeroed();
            let created = CreateProcessW(
                PCWSTR(shell_wide.as_ptr()),
                Some(PWSTR(command_wide.as_mut_ptr())),
                None,
                None,
                true,
                CREATE_NO_WINDOW,
                None,
                PCWSTR(cwd_wide.as_ptr()),
                &startup,
                &mut info,
            );
            // 子进程已经拿到这两端，父进程的副本关掉。
            let _ = CloseHandle(stdin_read);
            let _ = CloseHandle(stdout_write);

            if let Err(error) = created {
                let _ = CloseHandle(stdin_write);
                let _ = CloseHandle(stdout_read);
                return Err(format!("启动 shell 失败：{error}"));
            }
            let _ = CloseHandle(info.hThread);
            Ok(build_term(
                None,
                stdin_write,
                stdout_read,
                info.hProcess,
                "pipes",
            ))
        }
    }

    fn build_term(
        pty: Option<HPCON>,
        input: HANDLE,
        output: HANDLE,
        process: HANDLE,
        mode: &'static str,
    ) -> Term {
        let buffer = Arc::new(Mutex::new(VecDeque::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let reader = {
            let buffer = Arc::clone(&buffer);
            let alive = Arc::clone(&alive);
            let read_handle = SendHandle(output);
            thread::spawn(move || read_loop(read_handle, buffer, alive))
        };
        Term {
            pty,
            input,
            output,
            process,
            buffer,
            alive,
            reader: Some(reader),
            mode,
        }
    }

    fn close(mut term: Term) {
        unsafe {
            // 先关伪终端：它的输出端一断，读者线程的 `ReadFile` 就会返回，
            // 之后才能安全地把句柄关掉（读写与关句柄并发是未定义行为）。
            if let Some(pty) = term.pty.take() {
                ClosePseudoConsole(pty);
            }
            let _ = TerminateProcess(term.process, 0);
            let _ = CloseHandle(term.process);
            let _ = CloseHandle(term.input);
        }
        if let Some(reader) = term.reader.take() {
            let mut waited = std::time::Duration::ZERO;
            while !reader.is_finished() && waited < std::time::Duration::from_secs(2) {
                std::thread::sleep(std::time::Duration::from_millis(10));
                waited += std::time::Duration::from_millis(10);
            }
            if reader.is_finished() {
                let _ = reader.join();
                unsafe {
                    let _ = CloseHandle(term.output);
                }
            }
            // 读者线程卡住时**不关**输出句柄：让它随进程退出回收，
            // 总好过和阻塞中的 `ReadFile` 抢同一个句柄。
        }
    }

    /// 往终端写一串字符（按键 / 粘贴 / 整行命令）。
    ///
    /// 管道模式下 shell 不会回显输入（`/Q`），所以这里把写出去的字节也放进缓冲，
    /// 让界面看到的顺序和真终端一致（提示符 → 命令 → 输出）。
    pub fn write_json(json_text: &str) -> String {
        let value: Value = serde_json::from_str(json_text).unwrap_or(Value::Null);
        let data = value
            .get("data")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let (handle, echo_buffer) = {
            let slot = TERM.lock().unwrap_or_else(|error| error.into_inner());
            match slot.as_ref() {
                Some(term) if term.alive.load(Ordering::SeqCst) => (
                    Some(term.input),
                    (term.mode == "pipes").then(|| Arc::clone(&term.buffer)),
                ),
                _ => (None, None),
            }
        };
        let Some(handle) = handle else {
            return error_json("终端没有在运行");
        };
        if data.is_empty() {
            return json!({ "ok": true, "error": "" }).to_string();
        }
        let bytes = data.as_bytes();
        let mut written = 0u32;
        // 阻塞写：一次按键只有几个字节，管道几乎不会满。
        match unsafe { WriteFile(handle, Some(bytes), Some(&mut written), None) } {
            Ok(()) => {
                if let Some(buffer) = echo_buffer {
                    let mut guard = buffer.lock().unwrap_or_else(|error| error.into_inner());
                    guard.extend(bytes);
                }
                json!({ "ok": true, "error": "" }).to_string()
            }
            Err(error) => error_json(&format!("写入终端失败：{error}")),
        }
    }

    /// 改终端尺寸（面板缩放时调；管道模式没有这个概念，直接返回成功）。
    pub fn resize_json(json_text: &str) -> String {
        let value: Value = serde_json::from_str(json_text).unwrap_or(Value::Null);
        let cols = value
            .get("cols")
            .and_then(Value::as_u64)
            .unwrap_or(120)
            .clamp(20, 400) as i16;
        let rows = value
            .get("rows")
            .and_then(Value::as_u64)
            .unwrap_or(30)
            .clamp(5, 200) as i16;
        let pty = {
            let slot = TERM.lock().unwrap_or_else(|error| error.into_inner());
            match slot.as_ref() {
                Some(term) if term.alive.load(Ordering::SeqCst) => term.pty,
                _ => None,
            }
        };
        let Some(pty) = pty else {
            return json!({ "ok": true, "error": "" }).to_string();
        };
        match unsafe { ResizePseudoConsole(pty, COORD { X: cols, Y: rows }) } {
            Ok(()) => json!({ "ok": true, "error": "" }).to_string(),
            Err(error) => error_json(&format!("调整终端尺寸失败：{error}")),
        }
    }

    /// 取走待显示的输出（排空）。`alive=false` 表示 shell 已经退出。
    pub fn read_json() -> String {
        let (buffer, alive, mode) = {
            let slot = TERM.lock().unwrap_or_else(|error| error.into_inner());
            match slot.as_ref() {
                Some(term) => (
                    Some(Arc::clone(&term.buffer)),
                    term.alive.load(Ordering::SeqCst),
                    term.mode,
                ),
                None => (None, false, ""),
            }
        };
        let Some(buffer) = buffer else {
            return json!({ "ok": true, "data": "", "alive": false, "mode": "" }).to_string();
        };
        let data = super::drain_utf8(&buffer);
        json!({ "ok": true, "data": data, "alive": alive, "mode": mode }).to_string()
    }

    /// 关掉终端（关标签 / 重新开始前）。
    pub fn stop_json() -> String {
        let term = {
            let mut slot = TERM.lock().unwrap_or_else(|error| error.into_inner());
            slot.take()
        };
        if let Some(term) = term {
            close(term);
        }
        json!({ "ok": true, "error": "" }).to_string()
    }

    #[cfg(test)]
    mod tests {
        use super::{read_json, start_json, stop_json, write_json};
        use serde_json::Value;
        use std::time::{Duration, Instant};

        /// 真起一个 shell，打一条命令，确认回显能从缓冲里读回来。
        ///
        /// 伪终端不可用时走的是管道档（本机就是这样），同样要能回显 ——
        /// 所以这条测试不区分档位，只验证「链路通」。
        #[test]
        #[ignore = "requires an interactive Windows console/ConPTY; run with --ignored on a real desktop"]
        fn terminal_spawns_and_echoes_a_command() {
            let root = std::env::temp_dir().join("lunac-terminal-test");
            let _ = std::fs::create_dir_all(&root);
            let opened = start_json("{\"cols\":100,\"rows\":30}", &root);
            assert!(opened.contains("\"ok\":true"), "终端没起来：{opened}");
            assert!(
                write_json("{\"data\":\"echo lunac-pty-ok\\r\\n\"}").contains("\"ok\":true")
            );

            let deadline = Instant::now() + Duration::from_secs(20);
            let mut collected = String::new();
            while Instant::now() < deadline {
                let raw = read_json();
                if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                    if let Some(data) = value.get("data").and_then(Value::as_str) {
                        collected.push_str(data);
                    }
                }
                if collected.matches("lunac-pty-ok").count() > 1 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = stop_json();
            assert!(
                collected.contains("lunac-pty-ok"),
                "终端没有回显命令，收到：{collected:?}"
            );
        }
    }
}

#[cfg(not(windows))]
fn unsupported() -> String {
    serde_json::json!({ "ok": false, "error": "终端只在 Windows 上可用" }).to_string()
}

#[cfg(not(windows))]
pub fn start_json(_json: &str, _exe_root: &std::path::Path) -> String {
    unsupported()
}

#[cfg(not(windows))]
pub fn write_json(_json: &str) -> String {
    unsupported()
}

#[cfg(not(windows))]
pub fn resize_json(_json: &str) -> String {
    unsupported()
}

#[cfg(not(windows))]
pub fn read_json() -> String {
    serde_json::json!({ "ok": true, "data": "", "alive": false, "mode": "" }).to_string()
}

#[cfg(not(windows))]
pub fn stop_json() -> String {
    unsupported()
}

#[cfg(test)]
mod unit_tests {
    use super::incomplete_tail;

    #[test]
    fn incomplete_tail_keeps_only_a_broken_sequence() {
        assert_eq!(incomplete_tail(b""), 0);
        assert_eq!(incomplete_tail(b"abc"), 0);
        assert_eq!(incomplete_tail("中".as_bytes()), 0);
        // 「中」= E4 B8 AD：只来了前两个字节。
        assert_eq!(incomplete_tail(&[0xE4, 0xB8]), 2);
        // 只来了开头字节。
        assert_eq!(incomplete_tail(&[0xE4]), 1);
        // ASCII 收尾：完整。
        assert_eq!(incomplete_tail(b"\xE4\xB8\xADa"), 0);
    }
}
