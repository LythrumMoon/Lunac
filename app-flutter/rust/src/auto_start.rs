// 开机自启（Windows「用户登录时拉起」）—— 移植自 v1 `app/src-tauri/src/auto_start.rs`。
//
// 口径与 v1 完全一致（每条都是实测结论，不得擅自简化）：
//   1) 优先创建「用户登录时」计划任务（`schtasks /sc onlogon`）—— 实测比 HKCU Run 键
//      早约 60 秒触发；
//   2) 非管理员下 `schtasks` 必报「拒绝访问」→ 先写 HKCU\...\Run 保底（开关不会空转），
//      再弹一次 UAC 补建计划任务；用户取消 UAC 只是回到慢路径，仍算「已开启」；
//   3) 关闭时两者都清；状态 = Run 值存在 或 计划任务存在；两条都在 = `both`；
//   4) **开发构建一律不碰用户的开机项**（会弹黑窗口、界面也可能是旧的）；
//   5) 已注册项指向的不是当前 exe ⇒ `stale`，面板提示用户点一次「修复」（修复要提权）。
//
// 与 v1 的差异（只允许这两条）：
//   · 注册表读写用手写 `advapi32` FFI（v1 用 `winreg` crate）—— v2 薄层不新增依赖；
//   · 提权子进程仍是**本 exe**（`lunac.exe --lunac-auto-start-task=create|delete`），
//     由 Dart `main()` 在建窗前调用 `lunac_auto_start_elevated` 后退出。

/// 计划任务名 / HKCU Run 值名 / Run 键路径。
const TASK_NAME: &str = "Lunac";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Lunac";

/// 提权子进程的入口参数：`lunac.exe --lunac-auto-start-task=create|delete`。
pub const ELEVATED_FLAG: &str = "--lunac-auto-start-task=";

/// 自启状态（设置面板显示用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoStartInfo {
    pub enabled: bool,
    /// `"task"` / `"run"` / `"both"` / `"none"`
    pub mechanism: &'static str,
    /// 已注册项指向的不是当前 exe（exe 被移动 / 升级，或曾被别的构建注册过）。
    pub stale: bool,
    /// 当前是不是开发构建（开发构建不允许注册，面板据此禁用开关并给原因）。
    pub dev: bool,
    /// 当前平台是否支持（Linux 暂不支持，面板据此隐藏这一行）。
    pub supported: bool,
}

impl AutoStartInfo {
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "enabled": self.enabled,
            "mechanism": self.mechanism,
            "stale": self.stale,
            "dev": self.dev,
            "supported": self.supported,
        })
        .to_string()
    }
}

/// 读取自启状态（JSON 字符串）。
pub fn info_json() -> String {
    info().to_json()
}

/// 开启 / 关闭自启；返回 `Err(一句人话原因)` 表示没有生效（开关应弹回去）。
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        if enabled {
            imp::enable()
        } else {
            imp::disable()
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = enabled;
        Err("开机自启目前只支持 Windows".to_string())
    }
}

/// 提权子进程入口：执行一次计划任务动作，返回**进程退出码**（0 = 成功）。
pub fn handle_elevated(action: &str) -> i32 {
    #[cfg(target_os = "windows")]
    {
        imp::handle_elevated(action)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = action;
        1
    }
}

/// 当前进程是否为开发构建（自启与自更新共用这一份判断）。
pub fn is_dev_build() -> bool {
    if cfg!(debug_assertions) {
        return true;
    }
    std::env::current_exe()
        .map(|p| looks_like_dev_build_path(&p.to_string_lossy()))
        .unwrap_or(false)
}

/// 路径判据单独成函数，为的是可单测（测试进程本身恒为 debug 构建）。
///
/// v1 只判 `\target\{debug,release}\`；v2 的构建产物在 `build\windows\x64\runner\{Debug,Release}\`，
/// 两者都必须是「开发构建」—— 这类 exe 被开机项拉起同样是「弹黑窗 + 界面是旧的」。
fn looks_like_dev_build_path(p: &str) -> bool {
    let lower = p.to_lowercase();
    lower.contains(r"\target\debug\")
        || lower.contains(r"\target\release\")
        || lower.contains(r"\runner\debug\")
        || lower.contains(r"\runner\release\")
}

/// 读当前状态（不做任何写盘 / 提权）。
fn info() -> AutoStartInfo {
    #[cfg(target_os = "windows")]
    {
        imp::info()
    }
    #[cfg(not(target_os = "windows"))]
    {
        AutoStartInfo {
            enabled: false,
            mechanism: "none",
            stale: false,
            dev: is_dev_build(),
            supported: false,
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Output};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // ── 注册表（advapi32）────────────────────────────────────────

    type Hkey = isize;
    /// HKEY_CURRENT_USER（预定义句柄，不是真实指针）。
    const HKEY_CURRENT_USER: Hkey = 0x8000_0001u32 as i32 as isize;
    const KEY_READ: u32 = 0x20019;
    const KEY_SET_VALUE: u32 = 0x0002;
    const REG_SZ: u32 = 1;
    const ERROR_SUCCESS: i32 = 0;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            hkey: Hkey,
            sub_key: *const u16,
            options: u32,
            sam: u32,
            result: *mut Hkey,
        ) -> i32;
        fn RegCreateKeyExW(
            hkey: Hkey,
            sub_key: *const u16,
            reserved: u32,
            class: *const u16,
            options: u32,
            sam: u32,
            security: *mut c_void,
            result: *mut Hkey,
            disposition: *mut u32,
        ) -> i32;
        fn RegSetValueExW(
            hkey: Hkey,
            value: *const u16,
            reserved: u32,
            data_type: u32,
            data: *const u8,
            len: u32,
        ) -> i32;
        fn RegDeleteValueW(hkey: Hkey, value: *const u16) -> i32;
        fn RegQueryValueExW(
            hkey: Hkey,
            value: *const u16,
            reserved: *mut u32,
            data_type: *mut u32,
            data: *mut u8,
            len: *mut u32,
        ) -> i32;
        fn RegCloseKey(hkey: Hkey) -> i32;
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 读 HKCU Run\Lunac 的原始值。
    fn run_key_value() -> Option<String> {
        unsafe {
            let sub_key = wide(RUN_KEY);
            let mut key: Hkey = 0;
            if RegOpenKeyExW(HKEY_CURRENT_USER, sub_key.as_ptr(), 0, KEY_READ, &mut key)
                != ERROR_SUCCESS
            {
                return None;
            }
            let name = wide(VALUE_NAME);
            let mut data_type: u32 = 0;
            let mut len: u32 = 0;
            let probe = RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null_mut(),
                &mut data_type,
                std::ptr::null_mut(),
                &mut len,
            );
            if probe != ERROR_SUCCESS || data_type != REG_SZ || len == 0 {
                RegCloseKey(key);
                return None;
            }
            let mut buf = vec![0u8; len as usize];
            let ok = RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null_mut(),
                &mut data_type,
                buf.as_mut_ptr(),
                &mut len,
            ) == ERROR_SUCCESS;
            RegCloseKey(key);
            if !ok {
                return None;
            }
            let units: Vec<u16> = buf
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            Some(
                String::from_utf16_lossy(&units)
                    .trim_end_matches('\0')
                    .to_string(),
            )
        }
    }

    /// 写 HKCU Run\Lunac（无管理员权限即可）。
    fn write_run_value() -> Result<(), String> {
        let value = startup_command_line()?;
        let bytes: Vec<u8> = value
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        unsafe {
            let sub_key = wide(RUN_KEY);
            let mut key: Hkey = 0;
            let mut disposition: u32 = 0;
            let status = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                sub_key.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_SET_VALUE,
                std::ptr::null_mut(),
                &mut key,
                &mut disposition,
            );
            if status != ERROR_SUCCESS {
                return Err("无法打开开机启动项注册表（请检查系统权限）".to_string());
            }
            let name = wide(VALUE_NAME);
            let status = RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                bytes.as_ptr(),
                bytes.len() as u32,
            );
            RegCloseKey(key);
            if status != ERROR_SUCCESS {
                return Err("写入开机启动项失败（请检查系统权限）".to_string());
            }
        }
        Ok(())
    }

    /// 删除 HKCU Run\Lunac（幂等；不存在不算错）。
    fn delete_run_value() {
        unsafe {
            let sub_key = wide(RUN_KEY);
            let mut key: Hkey = 0;
            if RegOpenKeyExW(HKEY_CURRENT_USER, sub_key.as_ptr(), 0, KEY_SET_VALUE, &mut key)
                != ERROR_SUCCESS
            {
                return;
            }
            let name = wide(VALUE_NAME);
            let _ = RegDeleteValueW(key, name.as_ptr());
            RegCloseKey(key);
        }
    }

    // ── schtasks ────────────────────────────────────────────────

    fn schtasks_output(args: &[&str]) -> Result<Output, String> {
        let mut cmd = Command::new("schtasks");
        cmd.args(args);
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.output()
            .map_err(|e| format!("无法执行 schtasks：{e}"))
    }

    /// 启动命令行：`"<exe>" --background`（静默进托盘，首次热键即用）。
    fn startup_command_line() -> Result<String, String> {
        let exe = std::env::current_exe()
            .map_err(|e| format!("取不到当前 exe 路径：{e}"))?
            .display()
            .to_string();
        Ok(format!("\"{exe}\" --background"))
    }

    fn task_exists() -> bool {
        schtasks_output(&["/query", "/tn", TASK_NAME])
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    fn create_logon_task() -> Result<(), String> {
        let cmd = startup_command_line()?;
        let out = schtasks_output(&[
            "/create", "/tn", TASK_NAME, "/tr", &cmd, "/sc", "onlogon", "/f",
        ])?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "schtasks 创建失败：{}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    fn delete_logon_task() -> Result<(), String> {
        let out = schtasks_output(&["/delete", "/tn", TASK_NAME, "/f"])?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "schtasks 删除失败：{}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    /// 计划任务探测结果（区分「没有」与「读不懂」：后者要按旧行为重建）。
    enum TaskProbe {
        Absent,
        Command(String),
        Unreadable,
    }

    /// 用 `/xml` 读任务内容：`/fo LIST` 的字段名会随系统语言本地化，XML 标签不会。
    fn probe_logon_task() -> TaskProbe {
        let Ok(out) = schtasks_output(&["/query", "/tn", TASK_NAME, "/xml"]) else {
            return TaskProbe::Absent;
        };
        if !out.status.success() {
            return TaskProbe::Absent;
        }
        let xml = decode_task_xml(&out.stdout);
        match (xml.find("<Command>"), xml.find("</Command>")) {
            (Some(start), Some(end)) if end > start => {
                TaskProbe::Command(xml[start + 9..end].trim().to_string())
            }
            _ => TaskProbe::Unreadable,
        }
    }

    /// `schtasks /xml` 可能是 UTF-16LE（带 BOM）也可能是 UTF-8，两边都试；
    /// 认 UTF-16 的判据是「解出来含 `<Command>`」，避免把 UTF-8 误当 UTF-16 解成乱码。
    pub(super) fn decode_task_xml(bytes: &[u8]) -> String {
        if bytes.len() >= 2 {
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            let text = String::from_utf16_lossy(&units);
            if text.contains("<Command>") {
                return text;
            }
        }
        String::from_utf8_lossy(bytes).to_string()
    }

    /// 已注册的开机项是否指向别的程序（Run 与任务两边都查）。
    fn registered_entry_is_foreign() -> bool {
        let Ok(exe) = std::env::current_exe().map(|p| p.display().to_string()) else {
            return false;
        };
        let exe_lower = exe.to_lowercase();
        let run_foreign = run_key_value()
            .map(|value| !value.to_lowercase().contains(&exe_lower))
            .unwrap_or(false);
        let task_foreign = match probe_logon_task() {
            TaskProbe::Command(cmd) => !cmd.to_lowercase().contains(&exe_lower),
            _ => false,
        };
        run_foreign || task_foreign
    }

    // ── 提权（UAC）─────────────────────────────────────────────

    const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;
    const SW_HIDE: i32 = 0;
    const INFINITE: u32 = 0xFFFF_FFFF;
    const ERROR_CANCELLED: u32 = 1223;

    /// x64 下 `cbSize` 必须正好 112；字段顺序不可改动。
    #[repr(C)]
    struct ShellExecuteInfoW {
        cb_size: u32,
        f_mask: u32,
        hwnd: isize,
        lp_verb: *const u16,
        lp_file: *const u16,
        lp_parameters: *const u16,
        lp_directory: *const u16,
        n_show: i32,
        h_inst_app: isize,
        lp_id_list: *mut c_void,
        lp_class: *const u16,
        hkey_class: isize,
        dw_hot_key: u32,
        h_icon: isize,
        h_process: isize,
    }

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteExW(info: *mut ShellExecuteInfoW) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
        fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
        fn GetExitCodeProcess(process: isize, code: *mut u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    /// 以管理员身份跑一次动作（弹 UAC），**等它结束**并检查退出码。
    ///
    /// 提权目标是**我们自己的 exe**（带 `--lunac-auto-start-task=...` 开关），
    /// 而不是直接提权跑 `schtasks`：`/tr` 的值里带引号路径，经 cmd / PowerShell /
    /// ShellExecute 逐层转义极易出错（v1 的早期探针正是这么栽的）。
    fn run_elevated_task_action(action: &str) -> Result<(), String> {
        let exe = std::env::current_exe()
            .map_err(|e| format!("取不到当前 exe 路径：{e}"))?
            .display()
            .to_string();
        let params = format!("{ELEVATED_FLAG}{action}");
        let verb = wide("runas");
        let file = wide(&exe);
        let param = wide(&params);

        let mut info: ShellExecuteInfoW = unsafe { std::mem::zeroed() };
        info.cb_size = std::mem::size_of::<ShellExecuteInfoW>() as u32;
        info.f_mask = SEE_MASK_NOCLOSEPROCESS;
        info.lp_verb = verb.as_ptr();
        info.lp_file = file.as_ptr();
        info.lp_parameters = param.as_ptr();
        info.n_show = SW_HIDE;

        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let err = unsafe { GetLastError() };
            return Err(if err == ERROR_CANCELLED {
                "已取消管理员确认".to_string()
            } else {
                format!("提权启动失败（错误码 {err}）")
            });
        }
        if info.h_process == 0 {
            return Err("提权进程句柄缺失".to_string());
        }
        unsafe {
            WaitForSingleObject(info.h_process, INFINITE);
            let mut code: u32 = 1;
            GetExitCodeProcess(info.h_process, &mut code);
            CloseHandle(info.h_process);
            if code != 0 {
                return Err(format!("提权进程退出码 {code}"));
            }
        }
        Ok(())
    }

    /// 走 UAC 补建登录计划任务。成功 = `true`；取消 / 失败 = `false`（继续用 Run 键）。
    fn try_elevated_logon_task() -> bool {
        if run_elevated_task_action("create").is_err() {
            return false;
        }
        // 提权进程可能「成功返回但任务没建成」→ 复查，不轻信退出码。
        if !task_exists() {
            return false;
        }
        delete_run_value();
        true
    }

    pub fn handle_elevated(action: &str) -> i32 {
        match action {
            "create" => {
                if create_logon_task().is_ok() {
                    0
                } else {
                    1
                }
            }
            "delete" => {
                if delete_logon_task().is_ok() {
                    0
                } else {
                    1
                }
            }
            _ => 1,
        }
    }

    // ── 对外动作 ────────────────────────────────────────────────

    pub fn info() -> AutoStartInfo {
        let run_present = run_key_value().is_some();
        let task_present = task_exists();
        let mechanism = match (task_present, run_present) {
            (true, true) => "both",
            (true, false) => "task",
            (false, true) => "run",
            (false, false) => "none",
        };
        AutoStartInfo {
            enabled: task_present || run_present,
            mechanism,
            stale: registered_entry_is_foreign(),
            dev: is_dev_build(),
            supported: true,
        }
    }

    /// 开启：一次点击做完两件事 —— 先写 Run 键（无条件成功），再提权补建计划任务。
    /// 顺序不能反：UAC 被取消时仍处于「已开启、只是慢」，不会出现「弹了窗、点了否、
    /// 结果什么都没开」。
    pub fn enable() -> Result<(), String> {
        if is_dev_build() {
            return Err("当前是开发构建，不注册开机自启；请用安装版开启".into());
        }
        // ① 本进程已带管理员令牌 → 直接用任务，不留 Run 值。
        if create_logon_task().is_ok() {
            delete_run_value();
            return Ok(());
        }
        // ② 权限不足 → 先写 Run 保底，开关不会空转。
        write_run_value()?;
        // ③ 弹一次 UAC 补建计划任务；成败都不影响「已开启」。
        let _ = try_elevated_logon_task();
        Ok(())
    }

    /// 关闭：Run 值与计划任务都清；任务删不掉时走一次 UAC（否则会「关不掉」）。
    pub fn disable() -> Result<(), String> {
        delete_run_value();
        if task_exists() {
            let _ = delete_logon_task();
            if task_exists() {
                run_elevated_task_action("delete")?;
            }
        }
        if task_exists() {
            return Err("计划任务仍然存在（可能需要在管理员下删除）".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `schtasks /xml` 两种编码都要能解出 `<Command>`。
    #[cfg(target_os = "windows")]
    #[test]
    fn decodes_task_xml_in_both_encodings() {
        const SAMPLE: &str = "<Task><Actions><Exec><Command>C:\\a\\lunac.exe</Command>\
<Arguments>--background</Arguments></Exec></Actions></Task>";
        assert!(imp::decode_task_xml(SAMPLE.as_bytes())
            .contains("<Command>C:\\a\\lunac.exe</Command>"));

        let utf16: Vec<u8> = [0xFFu8, 0xFE]
            .into_iter()
            .chain(SAMPLE.encode_utf16().flat_map(|unit| unit.to_le_bytes()))
            .collect();
        assert!(imp::decode_task_xml(&utf16).contains("<Command>C:\\a\\lunac.exe</Command>"));
    }

    /// 开发构建判据：v1 的 `target\{debug,release}` 与 v2 的 `runner\{Debug,Release}`
    /// 都必须判为开发构建；安装路径（`D:\Lunac\lunac.exe`）必须放过。
    #[test]
    fn dev_build_path_detection() {
        assert!(looks_like_dev_build_path(
            r"D:\cc\lunac-master\app\src-tauri\target\debug\lunac.exe"
        ));
        assert!(looks_like_dev_build_path(
            r"D:\cc\lunac-master\app-flutter\build\windows\x64\runner\Debug\lunac.exe"
        ));
        assert!(looks_like_dev_build_path(
            r"D:\cc\lunac-master\app-flutter\build\windows\x64\runner\Release\lunac.exe"
        ));
        assert!(!looks_like_dev_build_path(r"D:\Lunac\lunac.exe"));
        assert!(!looks_like_dev_build_path(
            r"C:\Program Files\Lunac\lunac.exe"
        ));
        // 测试进程恒为 debug 构建 → 必须判为 true（这条红了说明守卫失效）。
        assert!(is_dev_build());
    }

    /// 提权参数必须是不带引号、不带空格的开关（经 ShellExecute 拼进命令行）。
    #[test]
    fn elevated_flag_is_a_plain_switch() {
        assert!(!ELEVATED_FLAG.contains('"'));
        assert!(!ELEVATED_FLAG.contains(' '));
        assert!(ELEVATED_FLAG.ends_with('='));
    }

    /// 状态 JSON 字段名与 Dart 侧一一对应。
    #[test]
    fn info_json_has_the_contract_fields() {
        let text = AutoStartInfo {
            enabled: true,
            mechanism: "run",
            stale: false,
            dev: true,
            supported: true,
        }
        .to_json();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        for key in ["enabled", "mechanism", "stale", "dev", "supported"] {
            assert!(value.get(key).is_some(), "缺少 {key}：{text}");
        }
    }
}
