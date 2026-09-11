// auto_start.rs — Windows 开机自启管理
//
// 方案（2026-09，联网参考 + 实测权衡）：
//   1) 优先创建「用户登录时」计划任务（schtasks /sc onlogon）——用户登录触发、实测
//      启动更早(~19s，优于 HKCU Run ~79s)。
//   2) 若创建失败（非管理员下 schtasks 常见「拒绝访问」）→ 自动回退写入
//      HKCU\...\Run 键（无需管理员、与 NSI 安装器同机制）。
//   3) 关闭自启时两者都清；状态 = Run 值存在 或 计划任务存在。
//   UI/开关均不需管理员权限：写 HKCU Run 无权限门槛；创建用户计划任务被系统拒绝时
//   静默走 Run 分支，按钮依旧可用。

use std::process::Command;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use winreg::enums::*;
use winreg::RegKey;

const TASK_NAME: &str = "Lunac";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Lunac";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
    let exe_path = std::env::current_exe()
        .map_err(|e| format!("Failed to get exe path: {}", e))?;
    Ok(format!("\"{}\" --background", exe_path.display()))
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

/// 删除同名计划任务（幂等；可能因权限失败，调用方忽略）。
fn delete_logon_task() {
    let _ = schtasks_output(&["/delete", "/tn", TASK_NAME, "/f"]);
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

/// Enable auto-start：先尝试「登录时计划任务」，失败（非管理员/被拒）自动回退 HKCU Run。
pub fn enable_auto_start() -> Result<(), String> {
    match create_logon_task() {
        Ok(()) => {
            // 任务已承载自启：移除 Run 值防双启动
            delete_run_value();
            eprintln!("[lunac::auto_start] ENABLED — Task Scheduler (logon)");
            Ok(())
        }
        Err(e) => {
            eprintln!(
                "[lunac::auto_start] Task Scheduler create failed ({}); falling back to HKCU Run",
                e
            );
            write_run_value()?;
            // 后台清旧任务（防与遗留任务双启动），不阻塞按钮
            std::thread::spawn(delete_logon_task);
            eprintln!("[lunac::auto_start] ENABLED — HKCU Run (fallback)");
            Ok(())
        }
    }
}

/// Disable auto-start：删除 Run 值并尝试删除计划任务（两者都清）。
pub fn disable_auto_start() -> Result<(), String> {
    delete_run_value();
    std::thread::spawn(delete_logon_task);
    eprintln!("[lunac::auto_start] DISABLED — removed Run value + task");
    Ok(())
}

/// 自启是否开启：Run 值存在 或（Run 缺失时）登录计划任务存在。
/// Run 存在时直接返回、不 spawn schtasks（启动/UI 热路径更快）；
/// 任务查询失败（服务未就绪）按「未开启」处理。
pub fn is_auto_start_enabled() -> Result<bool, String> {
    if run_key_value().is_some() {
        eprintln!("[lunac::auto_start] CHECK — run=true");
        return Ok(true);
    }
    match schtasks_output(&["/query", "/tn", TASK_NAME]) {
        Ok(out) => {
            let enabled = out.status.success();
            eprintln!("[lunac::auto_start] CHECK — run=false, task={}", enabled);
            Ok(enabled)
        }
        Err(_) => {
            eprintln!("[lunac::auto_start] CHECK — run=false, task query failed → false");
            Ok(false)
        }
    }
}

/// Startup repair（后台线程调用，不阻塞启动）：
/// 1) Run 键已存在 → 仅用 registry 重建当前 exe 路径（修复 exe 移动/复制的场景）。
/// 2) Run 键不存在、但存在登录计划任务 → 用当前 exe 重建该任务（同样修复移动/升级）。
pub fn repair_auto_start_on_startup() {
    if run_key_value().is_some() {
        if let Err(e) = write_run_value() {
            eprintln!("[lunac::auto_start] Repair (rewrite Run) failed: {}", e);
        }
        return;
    }
    // Run 缺失：可能由登录计划任务承载 → 重建任务到当前 exe（幂等）
    match schtasks_output(&["/query", "/tn", TASK_NAME]) {
        Ok(out) if out.status.success() => {
            eprintln!("[lunac::auto_start] Repair (rewrite logon task)");
            let _ = create_logon_task();
        }
        Ok(_) => { /* 无任务，无需处理 */ }
        Err(e) => eprintln!("[lunac::auto_start] Repair probe failed (ignored): {}", e),
    }
}
