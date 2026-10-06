// auto_start.rs — Windows 开机自启管理
//
// 方案（2026-09，联网参考 + 实测权衡）：
//   1) 优先创建「用户登录时」计划任务（schtasks /sc onlogon）——用户登录触发、实测
//      启动更早(~19s，优于 HKCU Run ~79s)。
//   2) 若创建失败（非管理员下 schtasks 必报「拒绝访问」）→ 自动回退写入
//      HKCU\...\Run 键（无需管理员、与 NSI 安装器同机制）。
//   3) 关闭自启时两者都清；状态 = Run 值存在 或 计划任务存在。
//   4) 「拨开开关」这一个动作就要点到位：先写 Run 键（无条件成功、开关不空转），
//      紧接着自动提权补建计划任务（弹一次 UAC）。成功 → 清 Run、改由任务承载；
//      取消 → 保留 Run，仍是「已开启，只是慢」。**不在 UI 暴露机制名** ——
//      「Run 键 / 计划任务」是纯实现术语，用户不会看也看不懂；机制只进落盘日志
//      （设置面板打开时会记一行「探测 run=? task=? ⇒ 机制=?」）。
//   5) 计划任务在根目录，**创建与删除都要管理员**，两处都必须能提权 ——
//      否则会出现「关不掉的任务」。删除失败会如实报错（开关弹回去），
//      不能像早期那样「发个后台线程就不管了」。

use std::process::Command;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use serde::Serialize;
use winreg::enums::*;
use winreg::RegKey;

const TASK_NAME: &str = "Lunac";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Lunac";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 自启的**实际生效机制**（给 UI 显示用）。
///
/// 为什么必须让用户看见：计划任务（`onlogon`）与 HKCU Run 的触发时间**实测差约 60 秒**
/// （见文件头注释的 ~19s vs ~79s），而非管理员下 `schtasks` 常被系统拒绝、
/// 会**静默**回落到慢的那条 —— 用户只看到一个 `true/false`，根本无从判断
/// 「开机后要等很久」是自己机器的正常表现还是出了问题。
#[derive(Debug, Serialize, Clone)]
pub struct AutoStartInfo {
    pub enabled: bool,
    /// `"task"`（登录时计划任务）/ `"run"`（HKCU Run 键）/ `"both"`（两条都在，可能被拉起两次）/ `"none"`
    pub mechanism: String,
    /// **已注册的开机项指向的不是当前这份 exe**（见 `registered_entry_is_foreign`）。
    /// 面板据此提示一次「修复」：它通常意味着开机拉起的是旧路径 / 别的构建（实测那会
    /// 开机弹 cmd、界面还是旧的），而修复需要管理员权限，只能在用户点一下时提权做。
    pub stale: bool,
}

/// 当前进程是不是**开发构建**（`cargo build` 产物，落在 `target\{debug,release}\`）。
///
/// 为什么要专门判它（2026-09-21 实测取证，不得删）：
///   · 开发构建是**控制台子系统**（`#![cfg_attr(not(debug_assertions), windows_subsystem)]`
///     只在 release 下生效），被开机项拉起时会**弹一个黑窗口**；
///   · 它带着旧 `dist`（或 devUrl），出来的界面可能是旧的。
/// 实测日志：计划任务曾被一次 dev 调试注册成 `…\target\debug\lunac.exe --background`，
/// 之后每次登录都由它拉起 —— 用户看到的正是「开机弹 cmd + 呼出来的界面不能用」。
/// 所以开发构建**一律不碰用户的开机项**：既不许注册，也不去「修复」。
///
/// `pub(crate)`：`updater.rs` 也要同一份判断（开发构建不许自更新，理由同类）——
/// 判据必须只有一份，别各抄一遍。
pub(crate) fn is_dev_build() -> bool {
    if cfg!(debug_assertions) {
        return true;
    }
    std::env::current_exe()
        .map(|p| looks_like_dev_build_path(&p.to_string_lossy()))
        .unwrap_or(false)
}

/// 路径判据单独成函数，为的是可单测 —— `is_dev_build()` 在测试进程里恒为 `true`
/// （测试就是 debug 构建），路径分支只能靠这个纯函数钉住。
fn looks_like_dev_build_path(p: &str) -> bool {
    let p = p.to_lowercase();
    p.contains(r"\target\debug\") || p.contains(r"\target\release\")
}

fn current_exe_str() -> Result<String, String> {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .map_err(|e| format!("Failed to get exe path: {}", e))
}

fn schtasks_output(args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = Command::new("schtasks");
    cmd.args(args);
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output()
        .map_err(|e| format!("Failed to run schtasks: {}", e))
}

/// 当前启动命令行：`"<exe>" --background`（静默后台加载，首次热键即用）。
fn startup_command_line() -> Result<String, String> {
    let exe_path = current_exe_str()?;
    Ok(format!("\"{}\" --background", exe_path))
}

/// Read the HKCU Run\Lunac value, if present.
fn run_key_value() -> Option<String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let run_key = hkcu.open_subkey_with_flags(RUN_KEY, KEY_READ).ok()?;
    run_key.get_value::<String, _>(VALUE_NAME).ok()
}

/// 删除 HKCU Run\Lunac 值（无管理员权限即可，幂等）。
fn delete_run_value() {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(run_key) = hkcu.open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE | KEY_QUERY_VALUE) {
        let _ = run_key.delete_value(VALUE_NAME);
    }
}

/// 计划任务是否存在。用 `/query` 只看退出码 —— 不解析任何文本，
/// 因此不受系统语言影响（`/fo LIST` 的字段名会本地化，见 `probe_logon_task`）。
fn task_exists() -> bool {
    schtasks_output(&["/query", "/tn", TASK_NAME])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 删除同名计划任务（幂等）。非管理员下会因「拒绝访问」失败 ——
/// `Result` 交回调用方决定要不要走提权（见 `disable_auto_start`）。
fn delete_logon_task() -> Result<(), String> {
    let out = schtasks_output(&["/delete", "/tn", TASK_NAME, "/f"])?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "schtasks delete failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// 创建「用户登录时」计划任务（最快路径；非管理员下常被拒绝则回退调用方处理）。
fn create_logon_task() -> Result<(), String> {
    let cmd = startup_command_line()?;
    let out = schtasks_output(&[
        "/create",
        "/tn", TASK_NAME,
        "/tr", &cmd,
        "/sc", "onlogon",
        "/f",
    ])?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "schtasks create failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// 写入当前 exe 的 HKCU Run 值（无管理员权限即可）。
fn write_run_value() -> Result<(), String> {
    let value = startup_command_line()?;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run_key, _) = hkcu
        .create_subkey(RUN_KEY)
        .map_err(|e| format!("Failed to open Run key: {}", e))?;
    run_key
        .set_value(VALUE_NAME, &value)
        .map_err(|e| format!("Failed to write Run value: {}", e))?;
    Ok(())
}

/// 落盘日志。**不要用 `eprintln!`** —— release 是 GUI 子系统、没有控制台，
/// `eprintln!` 写进空气；宿主的落盘日志在 `<exe 根>\temp\logs\lunac-YYYY-MM-DD.log`
/// （见 log.rs）。自启这一类「用户看不见、又决定开机体验」的决策必须留痕。
fn note(msg: impl AsRef<str>) {
    crate::log::info(format!("auto_start: {}", msg.as_ref()));
}

// ── 提权（UAC）：补建 / 删除计划任务 ─────────────────────────────
//
// 为什么必须提权：计划任务落在 `C:\Windows\System32\Tasks`（任务计划根目录），
// 非管理员进程连**创建**和**删除**都会被拒 —— 实测日志原文
// `schtasks create failed: 错误: 拒绝访问。`。于是「计划任务比 Run 键早约 60 秒
// 触发」这条最快的路对普通用户默认走不通，只能由用户在设置面板显式点一下、
// 过一次 UAC（见 ai-spec §9.1 难点 2）。
//
// 为什么提权目标是**我们自己的 exe**，而不是直接提权跑 `schtasks`：`/tr` 的值里
// 带引号路径与 `--background`，经 cmd / PowerShell / ShellExecute 逐层转义极易出错
// （早期探针就因此被安全策略拦下）。把自己作为提权目标后，命令行只是一个不含引号、
// 不含空格的开关，真正的 `schtasks` 参数仍由本文件里已验证的 Rust 代码拼装。

/// 提权子进程的入口参数：`lunac.exe --lunac-auto-start-task=create|delete`。
/// 带 `=` 前缀是为了让 `main()` 能用一次 `strip_prefix` 判定，无需再写解析器。
const ELEVATED_FLAG: &str = "--lunac-auto-start-task=";
const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;
const SW_HIDE: i32 = 0;
const INFINITE: u32 = 0xFFFF_FFFF;
const ERROR_CANCELLED: u32 = 1223;

/// `SHELLEXECUTEINFOW`（x64 下 `cbSize` 必须正好 112；字段顺序不可改动）。
///
/// 这里手写而不是用 `windows` crate：项目既有 FFI 约定就是手写
/// `#[link] extern "system"`（见 `hotkey.rs`），且无需为此新增依赖特性。
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
    lp_id_list: *mut std::ffi::c_void,
    lp_class: *const u16,
    hkey_class: isize,
    dw_hot_key: u32,
    /// union { hIcon, hMonitor } —— 占位，本用途不读
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

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 本进程是否是被 UAC 提权拉起、专门来干这一件事的（见 `ELEVATED_FLAG`）。
/// 返回要执行的动作名（`create` / `delete`）。
pub fn elevated_action_from_args() -> Option<String> {
    action_from_arg(std::env::args())
}

/// 从命令行里挑出提权动作。独立成一个函数是为了可单测 ——
/// `std::env::args()` 在测试进程里是测试框架自己的参数，不可控。
fn action_from_arg<I, S>(args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .find_map(|a| a.as_ref().strip_prefix(ELEVATED_FLAG).map(str::to_string))
}

/// 提权子进程侧：执行一次动作，返回**进程退出码**（0 = 成功）。
/// `main()` 直接把返回值交给 `std::process::exit` —— 父进程靠它判断成败。
pub fn handle_elevated_action(action: &str) -> i32 {
    note(format!("[elevated] 收到动作 {action}（管理员令牌）"));
    let result = match action {
        "create" => create_logon_task(),
        "delete" => delete_logon_task(),
        other => Err(format!("未知动作 {other}")),
    };
    match result {
        Ok(()) => {
            note(format!("[elevated] {action} 成功"));
            0
        }
        Err(e) => {
            note(format!("[elevated] {action} 失败：{e}"));
            1
        }
    }
}

/// 提权跑一次动作（弹 UAC），**等它结束**并检查退出码。
///
/// 必须等：调用方紧接着要复查任务是否真的建成/删掉，不等就会误判。
/// 用户点了「否」时 `ShellExecuteExW` 直接失败（`ERROR_CANCELLED`），
/// 此时什么都不该做 —— 尤其**不能**顺手去清 Run 值，否则会把用户原有自启弄没。
fn run_elevated_task_action(action: &str) -> Result<(), String> {
    let exe = current_exe_str()?;
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
            "用户取消了管理员确认".to_string()
        } else {
            format!("提权启动失败 err={err}")
        });
    }
    if info.h_process == 0 {
        // 理论上不会发生（带 SEE_MASK_NOCLOSEPROCESS 必回句柄），但不猜
        return Err("提权进程句柄缺失".to_string());
    }
    unsafe {
        WaitForSingleObject(info.h_process, INFINITE);
        let mut code: u32 = 1;
        GetExitCodeProcess(info.h_process, &mut code);
        CloseHandle(info.h_process);
        if code != 0 {
            return Err(format!("提权进程退出码 {code}（原因见 temp\\logs）"));
        }
    }
    Ok(())
}

/// 走 UAC 补建登录计划任务。成功 = `true`；用户取消 / 提权失败 = `false`。
///
/// **不把「用户取消 UAC」当失败上报**：那会把「拨开关」变成一次可能报错的提权操作。
/// 调用方（`enable_auto_start`）已经把 Run 键写好了，取消 UAC 只是回到慢路径，
/// 功能仍然可用 —— 这条判断留在调用方，此函数只回答「任务建成没有」。
fn try_elevated_logon_task() -> bool {
    if let Err(e) = run_elevated_task_action("create") {
        note(format!("提权补建计划任务未完成（{e}）→ 继续用 Run 键"));
        return false;
    }
    // 提权子进程可能「成功返回但任务没建成」（参数被拒等）→ 复查，不轻信退出码。
    if !task_exists() {
        note("提权进程已结束，但任务仍不存在 → 继续用 Run 键（原因见上一行 elevated 日志）");
        return false;
    }
    delete_run_value();
    note("UPGRADED — 已改由登录计划任务承载，Run 值已清");
    true
}

/// Enable auto-start：**一次点击做完两件事** —— 先写 Run 键，再自动提权补建计划任务。
///
/// 为什么不把「写 Run」与「建任务」拆成两个用户动作（早期做法）：
/// ① 设置面板只能显示「开/关」，机制名（Run 键 / 计划任务）是纯实现术语，
///    用户既不会看也看不懂，多出来的入口等于不会有人点；
/// ② 拨开关是用户唯一的心智动作，就该一次点到位。
///
/// 顺序不能反：Run 键是**无条件成功**的那条，先写它 ⇒ 后续 UAC 被取消也只是
/// 「仍然开着、只是慢」，绝不会出现「弹了窗、点了否、结果什么都没开」。
/// 若本进程已是管理员，第一步 `create_logon_task()` 就直接成功，不会弹 UAC。
///
/// 返回值只表达「开关有没有真的开起来」：落到 Run 键也算成功（见 ai-spec §9.1 难点 2）。
pub fn enable_auto_start() -> Result<(), String> {
    // ⓪ 开发构建一律不注册（见 `is_dev_build`）：它会被控制台窗口与旧界面拉起来。
    //    这里**必须返回错误**而不是静默跳过 —— 开关要如实弹回去，别让用户以为开上了。
    if is_dev_build() {
        note("REFUSED — 开发构建不注册开机自启（会弹 cmd 窗口、界面也可能是旧的）");
        return Err("当前是开发构建，不注册开机自启；请用安装版开启".into());
    }

    // ① 无需提权就能建任务（本进程已带管理员令牌）→ 直接用任务，不留 Run 值
    if create_logon_task().is_ok() {
        delete_run_value();
        note("ENABLED — 实际机制=task（本进程已具备权限，未弹 UAC）");
        return Ok(());
    }

    // ② 权限不足（非管理员必报「拒绝访问」）→ 先落 Run 键保底，开关不会空转
    write_run_value()?;
    note("Run 键已写入（保底路径）；接着尝试提权改用计划任务（会弹一次管理员确认）");

    // ③ 弹一次 UAC 补建计划任务；成败都不影响「已开启」这个事实
    let upgraded = try_elevated_logon_task();
    note(format!(
        "ENABLED — 实际机制={}",
        if upgraded { "task" } else { "run" }
    ));
    Ok(())
}

/// Disable auto-start：Run 值与计划任务都清。
///
/// 计划任务若存在，**删除同样需要管理员权限**（与创建同理），所以先试无提权删除
/// （管理员运行本程序时能直接成功），删不掉再走一次 UAC。不能像早期那样「发个后台
/// 线程就不管了」：那样用户关掉开关后任务还在、开机照样被拉起 —— 比「慢」更糟。
pub fn disable_auto_start() -> Result<(), String> {
    delete_run_value();
    if task_exists() {
        let _ = delete_logon_task();
        if task_exists() {
            note("计划任务无管理员权限删不掉 → 请求提权删除");
            run_elevated_task_action("delete")?;
        }
    }
    if task_exists() {
        return Err("计划任务仍然存在（原因见 temp\\logs）".to_string());
    }
    note("DISABLED — 已清 Run 值与计划任务");
    Ok(())
}

/// 已注册的开机项是否指向**别的程序**（不是当前这份 exe）。
///
/// 为什么必须查得出这条：exe 被移动 / 升级，或曾被**别的构建**注册过（实测：一次 dev
/// 调试把计划任务写成了 `…\target\debug\lunac.exe`），开机拉起的就不是这份程序 ——
/// 用户看到「开机弹 cmd / 界面是旧的」而日志里只有一句「重建计划任务」，没有结论。
/// Run 键与计划任务**两边都查**（`both` 机制下只对一半也是坏的）。
/// 只对**读得到的内容**下结论：不存在 / 读不出（编码、语言、服务未就绪）一律 `false`，不猜。
fn registered_entry_is_foreign() -> bool {
    let Ok(exe) = current_exe_str() else {
        return false;
    };
    let exe_l = exe.to_lowercase();
    let run_foreign = run_key_value()
        .map(|v| !v.to_lowercase().contains(&exe_l))
        .unwrap_or(false);
    let task_foreign = match probe_logon_task() {
        TaskProbe::Command(c) => !c.to_lowercase().contains(&exe_l),
        _ => false,
    };
    run_foreign || task_foreign
}

/// 当前自启状态 + **实际生效机制**（UI 显示用）。两条都在时如实报 `both`。
///
/// 这是自启状态的**唯一实现**：`enabled = 计划任务存在 或 Run 值存在`。
/// 旧版另有一个「Run 存在就直接返回、不 spawn schtasks」的快速分支，但它只能回答
/// 「开没开」，而「开的是哪条」恰恰是用户唯一能据以判断「开机后要等多久」的信息
/// —— 于是合并成一个函数。它只在用户打开设置面板时被调用，不在启动热路径上。
pub fn auto_start_info() -> Result<AutoStartInfo, String> {
    let run_present = run_key_value().is_some();
    let task_present = task_exists();
    let mechanism = match (task_present, run_present) {
        (true, true) => "both",
        (true, false) => "task",
        (false, true) => "run",
        (false, false) => "none",
    };
    let stale = registered_entry_is_foreign();
    note(format!(
        "探测 run={} task={} ⇒ 机制={} 指向旧程序={}",
        run_present, task_present, mechanism, stale
    ));
    Ok(AutoStartInfo {
        enabled: task_present || run_present,
        mechanism: mechanism.into(),
        stale,
    })
}

/// 计划任务的探测结果。
///
/// 为什么要把「查不到任务」与「读不懂任务」分开：前者应当什么都不做，后者必须
/// **按旧行为重建** —— 解析失败（编码/格式变化、系统语言差异）绝不能变成
/// 「以后不再自愈」的静默回归。
enum TaskProbe {
    /// 没有这个任务（或查询本身失败，如开机时 Task Scheduler 尚未就绪）
    Absent,
    /// 读到了任务，且成功取出 `<Command>`
    Command(String),
    /// 任务存在，但取不出 `<Command>`（编码/格式与预期不符）→ 走重建
    Unreadable,
}

/// 探测登录计划任务里记载的命令行。
///
/// 用 `/xml` 而不是 `/fo LIST /v`：后者的字段名（`Task To Run:`）**会随系统语言本地化**
/// （本机中文 Windows 上连 `TaskName:` 都匹配不到，实测），而 XML 里的 `<Command>`
/// 是固定标签，与语言无关。
fn probe_logon_task() -> TaskProbe {
    let Ok(out) = schtasks_output(&["/query", "/tn", TASK_NAME, "/xml"]) else {
        return TaskProbe::Absent;
    };
    if !out.status.success() {
        return TaskProbe::Absent;
    }
    let xml = decode_task_xml(&out.stdout);
    match (xml.find("<Command>"), xml.find("</Command>")) {
        (Some(s), Some(e)) if e > s => TaskProbe::Command(xml[s + 9..e].trim().to_string()),
        _ => TaskProbe::Unreadable,
    }
}

/// 解析 `schtasks /xml` 的输出：可能是 UTF-16LE（带 BOM）也可能是 UTF-8，两边都试。
/// 认 UTF-16 的判据是「解出来含 `<Command>`」，避免把 UTF-8 内容误当 UTF-16 解成乱码。
fn decode_task_xml(bytes: &[u8]) -> String {
    if bytes.len() >= 2 {
        let utf16: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let as_utf16 = String::from_utf16_lossy(&utf16);
        if as_utf16.contains("<Command>") {
            return as_utf16;
        }
    }
    String::from_utf8_lossy(bytes).to_string()
}

/// Startup repair（后台线程调用，不阻塞启动）：**只在真的对不上时才重建**。
///
/// 旧实现是「无条件重建」—— Run 键存在就每开机重写一次注册表；计划任务方式下每开机
/// `schtasks /query` + `/create` **各 spawn 一次 `schtasks.exe`**。而开机瞬间正是磁盘与
/// 杀软 IO 最紧张的时候（那个 4s 延迟就是为避开它而设计的），重建真正要防的却只有
/// 「exe 被移动 / 升级导致路径失效」一种情况 —— 对得上就应当什么都不做。
pub fn repair_auto_start_on_startup() {
    // ⓪ 开发构建**什么都不做**（见 `is_dev_build`）：它的启动就是「随手跑一次」，
    //    既不该重写用户的开机项，更不该把开机项改指向 `target\debug\lunac.exe` ——
    //    那正是「开机弹 cmd + 界面是旧的」的成因。清理/修复交给安装版做。
    if is_dev_build() {
        note("修复检查跳过：开发构建不碰用户的开机项");
        return;
    }
    // ① Run 键在手：比对现值，一致就不写（注册表写入不 spawn 进程，但仍是磁盘 IO）。
    if let Some(current) = run_key_value() {
        match startup_command_line() {
            Ok(want) if want == current => {
                note("修复检查：Run 值已是最新，跳过");
            }
            Ok(_) => {
                if let Err(e) = write_run_value() {
                    note(format!("修复检查：重写 Run 值失败 {e}"));
                } else {
                    note("修复检查：重写 Run 值（exe 路径已变）");
                }
            }
            Err(e) => note(format!("修复检查跳过：{e}")),
        }
        return;
    }
    // ② Run 缺失：自启可能由登录计划任务承载 → 比对任务里的命令，一致就不重建。
    let Ok(exe) = current_exe_str() else {
        note("修复检查跳过：取不到当前 exe 路径");
        return;
    };
    match probe_logon_task() {
        TaskProbe::Absent => note("修复检查：无自启项，跳过"),
        TaskProbe::Command(cmd) if cmd.to_lowercase().contains(&exe.to_lowercase()) => {
            note("修复检查：计划任务已指向当前 exe，跳过");
        }
        TaskProbe::Command(cmd) => {
            note(format!(
                "修复检查：计划任务指向的是别的程序（{cmd}）→ 重建"
            ));
            rebuild_logon_task();
        }
        // 解析不出来时按旧行为重建：宁可多 spawn 一次，也不能让自愈能力静默消失
        TaskProbe::Unreadable => {
            note("修复检查：任务内容读不出 → 按旧行为重建（不让自愈能力静默消失）");
            rebuild_logon_task();
        }
    }
}

/// 重建登录计划任务，并**如实记账成败**。
///
/// 为什么失败必须写清楚：非管理员下 `schtasks /create` 必报「拒绝访问」，而这条路径
/// 在启动时自动跑（不弹 UAC）—— 旧实现 `let _ = create_logon_task();` 把失败吞了，
/// 于是「开机项一直指向旧程序」既修不好、日志里也看不出原因，用户只看到开机弹 cmd。
/// 现在失败会留一行结论，设置面板也会因 `AutoStartInfo.stale` 提示用户点一次「修复」（那条会提权）。
fn rebuild_logon_task() {
    match create_logon_task() {
        Ok(()) => note("修复检查：计划任务已重建"),
        Err(e) => note(format!(
            "修复检查：重建失败（非管理员下必然如此，需要用户在设置面板点「修复」提权）{e}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "<Task><Actions><Exec><Command>C:\\a\\lunac.exe</Command>\
<Arguments>--background</Arguments></Exec></Actions></Task>";

    /// `schtasks /xml` 可能是 UTF-8，也可能是带 BOM 的 UTF-16LE（本机无法实测，
    /// 因为沙箱不允许枚举系统计划任务）—— 两种都要能解出 `<Command>`。
    #[test]
    fn decodes_task_xml_in_both_encodings() {
        assert!(decode_task_xml(SAMPLE.as_bytes()).contains("<Command>C:\\a\\lunac.exe</Command>"));

        let utf16: Vec<u8> = [0xFFu8, 0xFE]
            .into_iter()
            .chain(SAMPLE.encode_utf16().flat_map(|u| u.to_le_bytes()))
            .collect();
        assert!(decode_task_xml(&utf16).contains("<Command>C:\\a\\lunac.exe</Command>"));
    }

    /// `cbSize` 必须正好等于结构体真实大小 —— x64 下 `ShellExecuteExW` 只认 112。
    /// 字段顺序/类型写错会让它**静默失败**（返回 0、`GetLastError` = 87
    /// ERROR_INVALID_PARAMETER），表现为「点了按钮什么都没发生」。
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn shell_execute_info_size_matches_the_win32_struct() {
        assert_eq!(std::mem::size_of::<ShellExecuteInfoW>(), 112);
    }

    /// 开发构建的路径判据（2026-09-21）。**这是「开机弹 cmd + 界面是旧的」那条 bug 的
    /// 根因守卫**：`target\{debug,release}` 下的 exe 是 cargo 产物（控制台子系统 + 旧 dist），
    /// 一旦被开机项拉起就是这个现象 —— 所以这类路径必须被判为开发构建，从而既不注册自启、
    /// 也不去「修复」用户的开机项。正常安装路径（如 `D:\Lunac\lunac.exe`）必须放过。
    #[test]
    fn dev_build_path_detection() {
        assert!(looks_like_dev_build_path(
            r"D:\cc\claude-code-cli-master\app\src-tauri\target\debug\lunac.exe"
        ));
        assert!(looks_like_dev_build_path(
            r"D:\cc\app\src-tauri\TARGET\Release\deps\lunac.exe"
        ));
        assert!(!looks_like_dev_build_path(r"D:\Lunac\lunac.exe"));
        assert!(!looks_like_dev_build_path(r"C:\Program Files\Lunac\lunac.exe"));
        // 测试进程本身就是开发构建（debug 或 target\release 下的 deps）→ 必须判为 true：
        // 一旦这个断言变红，就说明守卫失效、测试机上的自启会被开发构建改写。
        assert!(is_dev_build());
    }

    /// 提权参数必须是**不带引号、不带空格**的开关：本参数会经 ShellExecute 拼进
    /// 命令行，掺进引号就要逐层转义（早期探针正是因此栽在命令行解析上）。
    #[test]
    fn elevated_flag_is_a_plain_switch() {
        assert!(!ELEVATED_FLAG.contains('"'));
        assert!(!ELEVATED_FLAG.contains(' '));
        assert!(ELEVATED_FLAG.ends_with('='));
        assert_eq!(
            action_from_arg(["--lunac-auto-start-task=create"]),
            Some("create".to_string())
        );
        assert_eq!(action_from_arg(["--background"]), None);
    }
}
