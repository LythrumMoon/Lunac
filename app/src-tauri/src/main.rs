// src-tauri/src/main.rs
// Tauri native backend — manages subprocess lifecycle, tray, and IPC

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(rust_2024_compatibility)]

mod hotkey;
mod commands;
mod app_indexer;
mod file_indexer;
mod system_catalog;
mod icon_extractor;
mod proxy_server;
mod storage;
mod chat_db;
mod auto_start;
mod mcp_server;
mod windows_ocr;
mod paddle_ocr;
mod cli_bridge;
mod agent_server;
mod log;
mod single_instance;

use commands::{
    start_cli, stop_cli, send_message, get_status,
    set_security_profile, list_start_menu_apps,
    set_query_state, set_recording_state, set_ui_mode, set_chips_empty,
    set_ai_mode, set_thinking_mode, set_detached, get_ai_config, set_ai_config,
    set_workspace, get_workspace, set_tool_blacklist,
    search_apps, launch_app, add_custom_app, remove_custom_app, list_custom_apps,
    get_app_icon,
    search_files, file_index_status, refresh_file_index,
    system_catalog, open_setting, run_system_action, reveal_in_explorer,
    set_hotkey_combo, get_hotkey_combo,
    set_auto_start, get_auto_start_info,
    check_file_exists,
    list_tool_files, read_tool_file, save_tool_file, delete_tool_file,
    download_tool_from_url,
    list_installed_skills, read_skill_file, save_skill_file, delete_skill,
    import_skill_content, install_skill_from_url,
    run_ocr,
    save_temp_image,
    delete_temp_image,
    read_clipboard_files,
    read_clipboard_file_paths,
    read_clipboard_backup_image,
    open_in_vscode,
    open_file_in_vscode,
    get_system_language,
    run_paddle_ocr,
    ocr_engine_status,
    ocr_engine_install,
    hide_lunac,
};
use std::process::Command as StdCommand;
use std::sync::Mutex;
use tauri::{
    Manager, WindowEvent,
    tray::{TrayIconBuilder, TrayIconEvent, MouseButton},
    menu::{MenuBuilder, MenuItemBuilder},
};

pub struct AppState {
    pub cli_stdin: Mutex<Option<std::process::ChildStdin>>,
    pub cli_process: Mutex<Option<std::process::Child>>,
    pub security_profile: Mutex<String>, // "safe" | "project" | "full"
    pub ai_mode: Mutex<String>, // "agent" (simple mode removed 2026-08-04)
    pub thinking_mode: Mutex<String>, // 思考开关: "on" | "off"
    pub workspace: Mutex<String>, // AI agent workspace dir; empty = user home dir (whole system)
    pub tool_blacklist: Mutex<Vec<String>>, // user-custom tool blacklist; merged with defaults on agent.exe start
}

fn kill_port(port: u16) {
    #[cfg(target_os = "windows")]
    let cmd = {
        use std::os::windows::process::CommandExt;
        StdCommand::new("cmd")
            .args(&[
                "/c",
                &format!(
                    "for /f \"tokens=5\" %a in ('netstat -ano 2>nul ^| findstr \":{}\" ^| findstr LISTENING') do taskkill /F /PID %a 2>nul",
                    port
                ),
            ])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output()
    };
    #[cfg(not(target_os = "windows"))]
    let cmd = StdCommand::new("sh")
        .args(&["-c", &format!("fuser -k {}/tcp 2>/dev/null || true", port)])
        .output();

    let output = cmd;
    if let Ok(o) = &output {
        if !o.status.success() {
            let stderr = String::from_utf8_lossy(&o.stderr);
            if !stderr.trim().is_empty() {
                eprintln!("[cleanup] Failed to kill port {}: {}", port, stderr.trim());
            }
        }
    }
}

fn main() {
    // ── MCP stdio server mode ──────────────────────────────────
    // Spawned by agent.exe as: lunac.exe --mcp-server
    // Runs the MCP protocol bridge, reading user tools from
    // <exe 根>\tools\*.json and serving them via stdio.
    if std::env::args().any(|a| a == "--mcp-server") {
        mcp_server::run_stdio();
        return;
    }

    // ── Background (auto-start) mode ───────────────────────────
    // Started by Windows auto-start (registry Run key) with `--background`.
    // The window stays hidden while WebView2 loads in the background, so the
    // first hotkey toggle is instant instead of a visible cold-start.
    // (A fixed 15s Thread::sleep() here was the #1 cause of "Lunac opens very
    // slowly at boot" — removed 2026-08-04; the flag was repurposed 2026-08-08
    // from a boot-delay sleep into the silent-launch marker, 点15/17.)
    let is_background = std::env::args().any(|a| a == "--background");

    let _ = dotenvy::dotenv(); // load .env from app/src-tauri/

    // 落盘日志：<exe 根>\temp\logs\lunac-YYYY-MM-DD.log。release 是 GUI 子系统、
    // 没有控制台，eprintln 线上拿不到；这里是宿主的启动/退出、agent 启停与
    // agent stderr 的唯一留存点。放在 dotenv 之后，好让 .env 里的 LUNAC_LOG* 生效；
    // 且**必须早于下面的提权分支与单实例判断** —— 那两条路都要留痕（"双击没反应"
    // 这类问题只能靠日志分辨是「被单实例拦下」还是「提权被取消」）。
    log::init("lunac");
    log::info(format!(
        "args={:?} background={is_background} root={}",
        std::env::args().skip(1).collect::<Vec<_>>(),
        crate::storage::lunac_root_dir().display()
    ));

    // ── 提权子进程：只做一件事（补建/删除登录计划任务）然后退出 ──────
    // 由 auto_start::run_elevated_task_action() 通过 ShellExecuteExW("runas") 拉起，
    // 命令行形如 `--lunac-auto-start-task=create`。**必须排在单实例判断之前** ——
    // 它和主实例是同一个 exe，否则会被自己拦下。退出码即成败（父进程据此判断）。
    if let Some(action) = auto_start::elevated_action_from_args() {
        std::process::exit(auto_start::handle_elevated_action(&action));
    }

    // ── 单实例保护 ───────────────────────────────────────────────
    // 自启实例在后台时用户再双击：第二实例会让 `RegisterHotKey` 失败并退化成
    // `WH_KEYBOARD_LL` 全局钩子（热键时灵时不灵 + 杀软误报的成因），
    // 所以这里直接退出。dev / release 共用同一会话命名对象，故也互相排斥。
    if !single_instance::acquire() {
        // `--background`（自启拉起）那一份**不去唤出窗口**：机制为 `both` 时
        // 计划任务与 Run 键会在开机时各拉一个，若第二个实例把窗口叫出来，
        // 用户一开机就会看到界面。只有用户主动双击的那次才唤出。
        if !is_background && !single_instance::signal_existing() {
            log::warn("single_instance: 唤出信号送达失败（旧版本实例？）→ 仅退出");
        }
        log::info("single_instance: 已有实例在运行 → 本进程退出");
        return;
    }

    // 数据根目录统一为 exe 安装根：先把旧 %LOCALAPPDATA%\Lunac(-dev) 数据整体搬入
    // 并删除（幂等，见 storage.rs）。必须在 WebView2 初始化前完成，否则旧 profile
    // 与新的 WEBVIEW2_USER_DATA_FOLDER 会割裂。也必须在单实例判断之后：
    // 两个实例并发搬同一批目录会互相踩。
    crate::storage::migrate_legacy_localappdata();

    // AI 凭据的唯一真相源：<exe 根>\config\ai.json（设置面板保存）→ 注入 env。
    // 没有该文件时才用 .env。**必须早于任何 start_cli**（也能早于 WebView2 起，
    // 反正只读一个文件）。以前这一步是前端拿 localStorage 回灌的，会在启动时
    // 把旧值覆盖进 env —— 用户改了 .env 也不生效，见 storage.rs 的注释。
    commands::apply_saved_ai_config();

    // WebView2 profile 路径：**无论是否被外部预设都要记一行**（2026-09-17 加）。
    //
    // 为什么必须记：`WEBVIEW2_USER_DATA_FOLDER` 是「一旦存在就完全接管」的变量 —— 下一段
    // 只在**未设**时才注入本应用自己的路径。实测踩过：某次调试在终端里 `$env:` 设成了
    // release 的路径、事后忘了清，之后从同一终端启动的 **dev** lunac 就一直在用 release 的
    // profile（`msedgewebview2.exe` 命令行实测 `--user-data-dir=D:\Lunac\temp\webview-data\EBWebView`），
    // dev / release 的缓存与 leveldb 互相污染 —— 而日志里**一个字都没有**，只能去任务管理器
    // 翻 WebView2 子进程的命令行才能发现。现在启动即留痕，一眼可辨。
    match std::env::var("WEBVIEW2_USER_DATA_FOLDER") {
        Ok(p) => {
            crate::log::warn(format!(
                "WebView2 profile 来自环境变量（非本应用默认目录）：{p} —— 若是 dev 实例，\
                 说明该变量被外部预设，dev 与 release 会共用同一个 profile"
            ));
        }
        Err(_) => {
            // 缓存统一放 <exe_dir>\temp\（2026-09 修订）：不再写入 LOCALAPPDATA / exe 目录以外
            let lunac_root = crate::storage::lunac_root_dir();
            let webview_data = lunac_root.join("temp").join("webview-data");
            let _ = std::fs::create_dir_all(&webview_data);
            // 确保业务数据根目录存在（storage.rs 也会惰性创建）
            let _ = std::fs::create_dir_all(lunac_root.join("ModuleData"));
            std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &webview_data);
            crate::log::info(format!("WebView2 profile: {}", webview_data.display()));
        }
    }

    // ── Suppress WebView2 permission dialogs ──────────────────────
    // Chromium prompts "Allow site to see text and images on clipboard?"
    // when navigator.clipboard.read() is called. We use the Tauri
    // clipboard plugin (native Win32) exclusively — disable the
    // browser-level API entirely to prevent accidental dialog triggers.
    // WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS is read by WebView2 at startup.
    if std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").is_err() {
        std::env::set_var(
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "--disable-features=PermissionPrompt,ClipboardContentRead",
        );
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(move |app| {
            // Show the window unless launched in silent auto-start mode
            // (--background, 点15). The window config has visible:false so a
            // boot launch loads WebView2 invisibly; the hotkey shows it.
            if !is_background {
                if let Some(w) = app.get_webview_window("main") {
                    w.show().ok();
                }
            }

            // ── System tray ──
            let show_hide = MenuItemBuilder::with_id("show_hide", "Show / Hide").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit Lunac").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&show_hide)
                .item(&quit)
                .build()?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().unwrap())
                .tooltip("Lunac")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    match event.id().as_ref() {
                        "show_hide" => {
                            if let Some(w) = app.get_webview_window("main") {
                                if w.is_visible().unwrap_or(false) {
                                    w.hide().ok();
                                } else {
                                    // Use native force_foreground (same as hotkey toggle)
                                    // instead of Tauri's set_focus() which fails under
                                    // Windows ForegroundLockTimeout for background processes.
                                    crate::hotkey::show_and_focus();
                                }
                            }
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            if w.is_visible().unwrap_or(false) {
                                w.hide().ok();
                            } else {
                                // Use native force_foreground (same as hotkey toggle)
                                // instead of Tauri's set_focus() which fails under
                                // Windows ForegroundLockTimeout for background processes.
                                crate::hotkey::show_and_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            // Start native Windows global hotkey
            // （默认 Ctrl+Alt+Space；优先 RegisterHotKey，失败才回退 LL 钩子 + 子类化）
            crate::hotkey::start_hotkey(app.handle().clone());

            // 单实例：常驻等「又有人启动了 Lunac」的信号，收到就把窗口唤出。
            // 必须在 start_hotkey 之后 —— 它负责填 MAIN_HWND / APP，而唤出走的是
            // show_and_focus()（依赖前者）。
            crate::single_instance::start_activate_listener();

            // Repair auto-start registry key on startup (handles moved/copied
            // installations). 2026-09: moved OFF the boot-critical path — the
            // repair spawns `schtasks` (query + delete), and right after a boot
            // auto-start the Task Scheduler service may still be starting, so a
            // synchronous call could block the app for a long time (Bug1: 重启后
            // Lunac 主程序迟迟才就绪). It now runs on a background thread after a
            // short delay; Lunac's own startup never waits on schtasks.
            {
                let app_handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(4));
                    crate::auto_start::repair_auto_start_on_startup();
                    drop(app_handle);
                });
            }

            // 应用列表（Start Menu + 自定义启动项）：<exe 根>\temp\app-index-cache.json
            // 是唯一真相，搜索路径只读它；这里只在启动后做一次**后台**刷新
            // （文件不存在或比刷新间隔更旧时才真扫），不阻塞启动、不阻塞搜索。
            std::thread::spawn(|| {
                // 略延迟，避免与开机瞬间的磁盘/杀软 IO 高峰抢时间
                std::thread::sleep(std::time::Duration::from_millis(600));
                crate::app_indexer::refresh_if_stale();
            });

            // 文件索引（详细搜索用）：<exe 根>\temp\file-index-cache.json。
            // 同样只在后台线程里载入/重扫 —— 搜索路径（search_files）只读内存索引，
            // 永不扫盘（见 docs/ai-spec.md §2.1.2 与 file_indexer.rs 头注释）。
            crate::file_indexer::init();

            // Start Agent HTTP bridge for VSCode extension (127.0.0.1:8789)
            if let Err(e) = agent_server::start() {
                eprintln!("[agent_server] Failed to start: {}", e);
            }

            app.manage(AppState {
                cli_stdin: Mutex::new(None),
                cli_process: Mutex::new(None),
                security_profile: Mutex::new("project".into()),
                ai_mode: Mutex::new("agent".into()),
                thinking_mode: Mutex::new("on".into()),
                workspace: Mutex::new(String::new()),
                tool_blacklist: Mutex::new(Vec::new()),
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                // Prevent window close → hide to tray instead
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    window.hide().ok();
                }
                // Real cleanup on tray "Quit" → app.exit(0)
                WindowEvent::Destroyed => {
                    log::info("window destroyed → app exit");
                    let state = window.state::<AppState>();

                    // Process/stdin live in cli_bridge since the agent HTTP
                    // bridge refactor; AppState fields are legacy no-ops.
                    if let Ok(mut stdin_guard) = state.cli_stdin.lock() {
                        *stdin_guard = None;
                    }
                    cli_bridge::kill_and_cleanup();

                    kill_port(5173);

                    agent_server::stop();
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            start_cli,
            stop_cli,
            send_message,
            get_status,
            set_security_profile,
            list_start_menu_apps,
            set_query_state,
            set_recording_state,
            set_ui_mode,
            set_chips_empty,
            set_ai_mode,
            set_thinking_mode,
            set_detached,
            get_ai_config,
            set_ai_config,
            set_workspace,
            get_workspace,
            set_tool_blacklist,
            search_apps,
            launch_app,
            add_custom_app,
            remove_custom_app,
            list_custom_apps,
            get_app_icon,
            search_files,
            file_index_status,
            refresh_file_index,
            system_catalog,
            open_setting,
            run_system_action,
            reveal_in_explorer,
            storage::save_chat_sessions,
            storage::load_chat_sessions,
            storage::save_clipboard_history,
            storage::load_clipboard_history,
            storage::append_usage_log,
            storage::read_usage_log,
            storage::memo_save_entries,
            storage::memo_load_entries,
            storage::memo_save_image,
            set_hotkey_combo,
            get_hotkey_combo,
            set_auto_start,
            get_auto_start_info,
            check_file_exists,
            list_tool_files,
            read_tool_file,
            save_tool_file,
            delete_tool_file,
            download_tool_from_url,
            list_installed_skills,
            read_skill_file,
            save_skill_file,
            delete_skill,
            import_skill_content,
            install_skill_from_url,
            run_ocr,
            save_temp_image,
            delete_temp_image,
            read_clipboard_files,
            read_clipboard_file_paths,
            read_clipboard_backup_image,
            open_in_vscode,
            open_file_in_vscode,
            get_system_language,
            run_paddle_ocr,
            ocr_engine_status,
            ocr_engine_install,
            hide_lunac,
            commands::log_frontend,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
