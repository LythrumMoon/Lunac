// src-tauri/src/main.rs
// Tauri native backend — manages subprocess lifecycle, tray, and IPC

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(rust_2024_compatibility)]

mod hotkey;
mod commands;
mod app_indexer;
mod icon_extractor;
mod proxy_server;
mod storage;
mod auto_start;
mod mcp_server;
mod windows_ocr;
mod paddle_ocr;
mod cli_bridge;
mod agent_server;

use commands::{
    start_cli, stop_cli, send_message, get_status,
    set_security_profile, list_start_menu_apps,
    set_query_state, set_recording_state, set_plugin_active, set_chips_empty,
    set_ai_mode, set_thinking_mode, set_detached, get_ai_config, set_ai_config,
    set_workspace, get_workspace, set_tool_blacklist,
    search_apps, launch_app, add_custom_app, remove_custom_app, list_custom_apps,
    get_app_icon,
    set_hotkey_combo, get_hotkey_combo,
    set_auto_start, get_auto_start,
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
    pub thinking_mode: Mutex<String>, // DeepSeek thinking level: "fast" | "think" | "deep"
    pub workspace: Mutex<String>, // AI agent workspace dir; empty = user home dir (whole system)
    pub tool_blacklist: Mutex<Vec<String>>, // user-custom tool blacklist; merged with defaults on cli.exe start
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
    // Spawned by cli.exe as: lunac.exe --mcp-server
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

    // 数据根目录统一为 exe 安装根：先把旧 %LOCALAPPDATA%\Lunac(-dev) 数据整体搬入
    // 并删除（幂等，见 storage.rs）。必须在 WebView2 初始化前完成，否则旧 profile
    // 与新的 WEBVIEW2_USER_DATA_FOLDER 会割裂。
    crate::storage::migrate_legacy_localappdata();

    if std::env::var("WEBVIEW2_USER_DATA_FOLDER").is_err() {
        // 缓存统一放 <exe_dir>\temp\（2026-09 修订）：不再写入 LOCALAPPDATA / exe 目录以外
        let lunac_root = crate::storage::lunac_root_dir();
        let webview_data = lunac_root.join("temp").join("webview-data");
        let _ = std::fs::create_dir_all(&webview_data);
        // 确保业务数据根目录存在（storage.rs 也会惰性创建）
        let _ = std::fs::create_dir_all(lunac_root.join("ModuleData"));
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &webview_data);
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

            // Start Menu / 自定义启动项 扫描缓存：优先从落盘缓存预热
            // （<exe 根>\temp\app-index-cache.json，跨重启秒出，不必等首次扫描）；
            // 文件缺失/过旧才真正重扫。之后由热键唤出路径按需后台刷新。
            {
                let warmed = crate::app_indexer::warm_cache_from_disk();
                std::thread::spawn(move || {
                    // 略延迟，避免与开机瞬间的磁盘/杀软 IO 高峰抢时间
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    if warmed {
                        // 已载入落盘缓存：仅在过期时后台重建
                        crate::app_indexer::refresh_scan_cache_if_stale();
                    } else {
                        crate::app_indexer::scan_all();
                    }
                });
            }

            // Start Agent HTTP bridge for VSCode extension (127.0.0.1:8789)
            if let Err(e) = agent_server::start() {
                eprintln!("[agent_server] Failed to start: {}", e);
            }

            app.manage(AppState {
                cli_stdin: Mutex::new(None),
                cli_process: Mutex::new(None),
                security_profile: Mutex::new("project".into()),
                ai_mode: Mutex::new("agent".into()),
                thinking_mode: Mutex::new("fast".into()),
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
            set_plugin_active,
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
            storage::save_chat_sessions,
            storage::load_chat_sessions,
            storage::save_clipboard_history,
            storage::load_clipboard_history,
            storage::memo_save_entries,
            storage::memo_load_entries,
            storage::memo_save_image,
            set_hotkey_combo,
            get_hotkey_combo,
            set_auto_start,
            get_auto_start,
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
            hide_lunac,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
