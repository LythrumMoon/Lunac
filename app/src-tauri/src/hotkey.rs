// src-tauri/src/hotkey.rs
// 全局热键 — 双模式：RegisterHotKey 优先，LL 钩子兜底（默认 Ctrl+Alt+Space）
//
// 后端选择（2026-09 重构，见 install_hook_thread）：
//   A. RegisterHotKey（默认路径）：内核级注册，**不发钩子、不注入输入** ——
//      AV 误报特征几乎归零；且由系统会话直接投递 WM_HOTKEY，**与前台窗口
//      是谁、什么权限无关**，因此管理员运行/独占全屏的游戏也能唤出。
//   B. WH_KEYBOARD_LL 钩子（兜底）：仅当 A 失败时启用 —— 即热键为 Alt+Space
//      （被 Windows 内核保留、无法注册）或组合已被其它程序占用。
//      AV 会把钩子 + SendInput 认作键盘记录器/注入特征，故尽量不启用。
//   代价：全局 Alt+Space 只能走 B（内核保留）；如需彻底避免钩子，改用
//   Ctrl+Alt+Space 等可注册组合（设置面板可录制改键）。
//
// LL 钩子分支的关键原则（参考 PowerToys / uTools / AutoHotkey 实现）：
// 1. LL 钩子回调必须微秒级返回，否则超过 LowLevelHooksTimeout (~300ms) 时
//    Windows 会放行按键（弹出系统菜单）并静默移除钩子。
//    因此回调内只做原子操作 + PostThreadMessageW（异步投递），
//    所有窗口操作（show/hide/focus）都转移到独立线程执行。
// 2. 修饰键检测只用无状态查询：LLKHF_ALTDOWN flag + GetAsyncKeyState（物理层权威）。
//    禁止手动 down/up 跟踪 — 修饰键 up 被漏掉（UAC 等场景）会状态卡死。
// 3. 吞掉热键后前台只剩"孤立 Alt down→up"序列，Chromium/WebView2 会因此弹系统菜单，
//    需 SendInput 注入 dummy key 打断（AutoHotkey MenuMaskKey 方案）。
//    RegisterHotKey 模式由系统消费按键、无孤立 Alt，故无需注入。
// 4. 自身窗口焦点时按键绕过 LL 钩子 → SetWindowSubclass 拦截 WM_SYSCOMMAND/SC_KEYMENU。
// 5. 热键可通过前端录制实时配置，组合格式 "Mod+Mod+Key"（如 Ctrl+Shift+K），
//    持久化到 <exe 根>\config\hotkey.json。

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use tauri::{AppHandle, Emitter, Manager};

// ── Windows 常量 ─────────────────────────────────────────────────
const WH_KEYBOARD_LL: i32 = 13;
const WM_KEYDOWN: usize = 0x0100;
const WM_KEYUP: usize = 0x0101;
const WM_SYSKEYDOWN: usize = 0x0104;
const WM_SYSKEYUP: usize = 0x0105;
const VK_SPACE: u32 = 0x20;
const VK_MENU: u32 = 0x12;   // 通用 Alt（GetAsyncKeyState 用）
const VK_ESCAPE: u32 = 0x1B;
const LLKHF_ALTDOWN: u32 = 0x0020;
const PM_NOREMOVE: u32 = 0x0000;
const INPUT_KEYBOARD: u32 = 1;
const KEYEVENTF_KEYUP: u32 = 0x0002;
const VK_DUMMY: u16 = 0xFF; // 保留键值，仅用于打断 Alt 单按序列
const WM_SYSCOMMAND: u32 = 0x0112;
const SC_KEYMENU: usize = 0xF100; // Alt+Space / Alt 激活系统菜单
// 双击标题栏（HTCAPTION 命中）→ 禁用铺满全屏（最大化）
const WM_NCLBUTTONDBLCLK: u32 = 0x00A3;
const HTCAPTION: usize = 2; // 命中测试：标题栏区域

// 自定义线程消息：钩子回调 → 消息循环（posted 消息会唤醒 GetMessageW）
const WM_APP_TOGGLE: u32 = 0x8001;
const WM_APP_ESC: u32 = 0x8002; // 自己前台时按下 Esc
const WM_APP_WATCHDOG_PING: u32 = 0x8003; // 看门狗心跳（验证消息泵存活）

const WM_HOTKEY: u32 = 0x0312; // RegisterHotKey 产生的窗口消息
const MOD_NOREPEAT: u32 = 0x4000; // RegisterHotKey: 不重复触发

// ── 可配置热键修饰键位掩码 ───────────────────────────────────────
const MOD_ALT: u32 = 0x01;
const MOD_CONTROL: u32 = 0x02;
const MOD_SHIFT: u32 = 0x04;
const MOD_META: u32 = 0x08;

const DEFAULT_HOTKEY: &str = "Ctrl+Alt+Space";

// ── 全局状态 ─────────────────────────────────────────────────────
static APP: OnceLock<AppHandle> = OnceLock::new();
static HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static LAST_TOGGLE_TICK: AtomicU32 = AtomicU32::new(0);
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static LAST_ESC_TICK: AtomicU32 = AtomicU32::new(0);
/// 当前热键的组合描述字符串（如 "Alt+Space"），供前端查询
static HOTKEY_COMBO: Mutex<String> = Mutex::new(String::new());
/// 当前热键的修饰键位掩码（初值须与 DEFAULT_HOTKEY 一致，防止“显示与实际不符”）
static HOTKEY_MODIFIERS: AtomicU32 = AtomicU32::new(MOD_CONTROL | MOD_ALT);
/// 当前热键的主键 VK 码
static HOTKEY_VK: AtomicU32 = AtomicU32::new(VK_SPACE);
/// 前端输入框是否空白（由 set_query_state 命令实时同步），
/// 让 Rust 端可独立完成"空白+Esc→隐藏"，不依赖 emit/listen 链路。
pub static QUERY_EMPTY: AtomicBool = AtomicBool::new(true);
/// 设置面板正在录制快捷键（Esc 应取消录制而非隐藏窗口）
pub static RECORDING: AtomicBool = AtomicBool::new(false);
/// 当前「界面层」。**Esc 的隐藏判据看层，不看内容**（2026-09-15 修正）。
///
/// 旧实现用「query 空 + chips 空 + 非插件态 ⇒ 隐藏」。详细搜索大界面把查询词放在
/// `#detail-input`、简洁搜索栏是空的 ⇒ Esc 在详情态直接把整个窗口隐藏（用户报的 bug）。
/// 根因是拿「有没有内容」代理「有没有层」，而这个代理在「内容在别处」的界面上必然失效。
///
/// 为什么用**单一枚举**而不是「每个界面一个 AtomicBool」：后者每加一个界面都要
/// 改这里的 Esc 分支，且前端变量与 Rust 原子量是两个真相源、漏同步一处行为就漂。
/// 现在新增界面 = 加一个常量 + 前端加一支处理，Rust 分支数不变。
pub const UI_MODE_MAIN: u8 = 0;   // 简洁搜索（唯一的「按内容判空」层）
pub const UI_MODE_PLUGIN: u8 = 1; // 插件 / AI 对话大界面
pub const UI_MODE_DETAIL: u8 = 2; // 详细搜索大界面（双击搜索栏进入）
pub static UI_MODE: AtomicU8 = AtomicU8::new(UI_MODE_MAIN);
/// 窗口处于独立界面模式（前台守卫不应自动隐藏）
pub static DETACHED: AtomicBool = AtomicBool::new(false);
/// 搜索栏文件泡泡框是否为空（由 set_chips_empty 命令同步），
/// Esc 空白+无泡泡时才隐藏窗口，有泡泡时仅移除一个泡泡。
pub static CHIPS_EMPTY: AtomicBool = AtomicBool::new(true);

// ── 钩子健康监控 ─────────────────────────────────────────────────
/// 钩子线程最近一次处理消息的时间戳（GetTickCount）。
/// 看门狗线程每 2 秒检查一次；若超过 5 秒未更新则认为钩子已死亡。
static LAST_HOOK_PING: AtomicU32 = AtomicU32::new(0);
/// 看门狗是否正在尝试重装钩子（防止并发重装）
static RESTARTING_HOOK: AtomicBool = AtomicBool::new(false);
/// 当前是否装配了 WH_KEYBOARD_LL 钩子。
/// false = 内核级 RegisterHotKey 模式（无钩子：AV 误报最低、不受前台程序权限影响）；
/// true  = 注册失败（Alt+Space 系统保留 / 组合被占用）时的 LL 钩子兜底模式。
static HOOK_MODE: AtomicBool = AtomicBool::new(false);
/// RegisterHotKey 注册的热键 ID
const HOTKEY_ID: usize = 1;

// ── 统计（诊断用） ───────────────────────────────────────────────
/// 钩子线程重启总次数
pub static HOOK_RESTART_COUNT: AtomicU32 = AtomicU32::new(0);
/// LL 钩子触发的 toggle 次数
pub static TOGGLE_COUNT_LL: AtomicU32 = AtomicU32::new(0);
/// RegisterHotKey 触发的 toggle 次数
pub static TOGGLE_COUNT_RHK: AtomicU32 = AtomicU32::new(0);

// ── Windows 结构体 ───────────────────────────────────────────────
#[repr(C)]
struct Kbdllhookstruct {
    vk_code: u32,
    scan_code: u32,
    flags: u32,
    time: u32,
    dw_extra_info: usize,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct Point { x: i32, y: i32 }

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct Msg {
    hwnd: isize,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    pt: Point,
}

// INPUT (x64: 40 bytes) — type + union{ MOUSEINPUT(32) / KEYBDINPUT(24) / ... }
#[repr(C)]
struct KeybdInput {
    vk: u16,
    scan: u16,
    flags: u32,
    time: u32,
    extra: usize,
}

#[repr(C)]
struct Input {
    r#type: u32,
    ki: KeybdInput,
    _pad: [u8; 8], // 补齐 union 至 MOUSEINPUT 大小
}

type HookProc = unsafe extern "system" fn(i32, usize, isize) -> isize;

#[link(name = "user32")]
extern "system" {
    fn SetWindowsHookExW(id_hook: i32, lpfn: HookProc, hmod: isize, thread_id: u32) -> isize;
    fn UnhookWindowsHookEx(hhk: isize) -> i32;
    fn CallNextHookEx(hhk: isize, n_code: i32, w_param: usize, l_param: isize) -> isize;
    fn GetMessageW(msg: *mut Msg, hwnd: isize, filter_min: u32, filter_max: u32) -> i32;
    fn PeekMessageW(msg: *mut Msg, hwnd: isize, filter_min: u32, filter_max: u32, remove: u32) -> i32;
    fn PostThreadMessageW(thread_id: u32, msg: u32, w_param: usize, l_param: isize) -> i32;
    fn GetAsyncKeyState(vk: i32) -> i16;
    fn SendInput(n: u32, inputs: *const Input, size: i32) -> u32;
    fn GetForegroundWindow() -> isize;
    fn SetForegroundWindow(hwnd: isize) -> i32;
    fn IsWindowVisible(hwnd: isize) -> i32;
    fn IsIconic(hwnd: isize) -> i32;
    fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
    fn AttachThreadInput(id_attach: u32, id_attach_to: u32, attach: i32) -> i32;
    fn OpenClipboard(hwnd: isize) -> i32;
    fn CloseClipboard() -> i32;
    fn GetClipboardData(format: u32) -> isize;
    fn IsClipboardFormatAvailable(format: u32) -> i32;
    fn RegisterHotKey(hwnd: isize, id: i32, modifiers: u32, vk: u32) -> i32;
    fn UnregisterHotKey(hwnd: isize, id: i32) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLastError() -> u32;
    fn GetCurrentThreadId() -> u32;
    fn GetTickCount() -> u32;
    fn GlobalLock(h_mem: isize) -> isize;
    fn GlobalUnlock(h_mem: isize) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    fn DragQueryFileW(h_drop: isize, i_file: u32, buf: *mut u16, buf_size: u32) -> u32;
}

#[link(name = "comctl32")]
extern "system" {
    fn SetWindowSubclass(hwnd: isize, pfn: SubclassProc, id: usize, data: usize) -> i32;
    fn DefSubclassProc(hwnd: isize, msg: u32, w_param: usize, l_param: isize) -> isize;
}

type SubclassProc = unsafe extern "system" fn(isize, u32, usize, isize, usize, usize) -> isize;

// ── 主窗口子类化：拦截 WM_SYSCOMMAND / SC_KEYMENU / 双击标题栏 ───
// 无边框窗口内置了对 Alt+Space 的系统菜单响应（即使 decorations: false，
// Chromium 在 WM_SYSKEYDOWN 后通过 WM_SYSCOMMAND 激活）。
// 子类化在消息到达 Tauri/Chromium 默认 WndProc 之前拦截并转为 hide。
// 同时拦截 WM_NCLBUTTONDBLCLK(HTCAPTION)：双击拖拽区会触发 Windows 默认
// 最大化（铺满全屏），与内容实测驱动高度冲突，一并吞掉。

const SUBCLASS_ID: usize = 1;

// Clipboard formats
const CF_HDROP: u32 = 15;

unsafe extern "system" fn main_window_subclass(
    hwnd: isize,
    msg: u32,
    w_param: usize,
    l_param: isize,
    _id: usize,
    _ref_data: usize,
) -> isize {
    if msg == WM_HOTKEY {
        // RegisterHotKey 触发 — 与 LL 钩子使用相同的已验证路径。
        // 注意：本分支仅在 RegisterHotKey 模式（无 LL 钩子）下到达，按键由系统
        // 直接消费、不存在"孤立 Alt"序列，故**不做 SendInput 注入**（去掉注入
        // 特征，降低 AV 误报）。
        TOGGLE_COUNT_RHK.fetch_add(1, Ordering::Relaxed);
        // 防抖：250ms 内不重复（RegisterHotKey 自带 MOD_NOREPEAT 但仅防硬件重复）
        let now = GetTickCount();
        let last = LAST_TOGGLE_TICK.load(Ordering::SeqCst);
        if now.wrapping_sub(last) >= 250 {
            LAST_TOGGLE_TICK.store(now, Ordering::SeqCst);
            thread::spawn(toggle_window);
        }
        return 0;
    }
    if msg == WM_NCLBUTTONDBLCLK && w_param == HTCAPTION {
        // 双击标题栏区域 → Windows 默认触发最大化（铺满全屏）。无边框窗口经
        // drag-region（startDragging → WM_NCLBUTTONDOWN/HTCAPTION）拖动，双击
        // 标题栏即落入此分支。吞掉消息阻止 DefWindowProc 发 SC_MAXIMIZE，
        // 保证窗口始终由 JS 内容实测驱动高度、不铺满全屏。
        return 0;
    }
    if msg == WM_SYSCOMMAND && (w_param & 0xFFF0) == SC_KEYMENU {
        // Block system menu activation.
        if l_param == 0x20 {
            // Only treat as toggle if Alt+Space IS the configured hotkey
            // AND we're not currently recording (JS hotkey capture in progress).
            //
            // 必须**同时**比对主键与修饰键：默认热键已改为 Ctrl+Alt+Space，
            // 其主键同为 VK_SPACE —— 若只比对 VK，Alt+Space 会被误判为当前热键
            // （表现为“无论怎么改键，Alt+Space 都能呼出”）。
            let hvk = HOTKEY_VK.load(Ordering::SeqCst);
            let hmods = HOTKEY_MODIFIERS.load(Ordering::SeqCst);
            let recording = RECORDING.load(Ordering::SeqCst);
            if hmods == MOD_ALT && hvk == VK_SPACE as u32 && !recording {
                send_dummy_key();
                thread::spawn(toggle_window);
            } else if recording {
                // Recording mode + SC_KEYMENU lParam=0x20 = Alt+Space pressed.
                // Chromium swallows Space in its internal menu mode, so JS keydown
                // never fires for Alt+Space. Capture directly from WndProc.
                RECORDING.store(false, Ordering::SeqCst);
                let _ = APP.get().map(|a| a.emit("lunac-hotkey-recorded", serde_json::json!({
                    "vk": VK_SPACE,
                    "modifiers": MOD_ALT,
                    "combo": "Alt+Space",
                })));
            }
        }
        return 0; // Always swallow SC_KEYMENU to prevent system menu
    }
    DefSubclassProc(hwnd, msg, w_param, l_param)
}

// ── 辅助函数（均为非阻塞、微秒级，回调内安全） ───────────────────

/// 检查当前按下的修饰键是否满足热键要求。
/// required_mods 是 MOD_ALT|MOD_CONTROL|MOD_SHIFT|MOD_META 的组合。
unsafe fn modifiers_match(kb: &Kbdllhookstruct, required_mods: u32) -> bool {
    if required_mods & MOD_ALT != 0 {
        if (kb.flags & LLKHF_ALTDOWN) == 0
            && (GetAsyncKeyState(VK_MENU as i32) as u16 & 0x8000) == 0
        {
            return false;
        }
    }
    if required_mods & MOD_CONTROL != 0 {
        if (GetAsyncKeyState(0x11) as u16 & 0x8000) == 0 {
            return false;
        }
    }
    if required_mods & MOD_SHIFT != 0 {
        if (GetAsyncKeyState(0x10) as u16 & 0x8000) == 0 {
            return false;
        }
    }
    if required_mods & MOD_META != 0 {
        // Check both left (0x5B) and right (0x5C) Windows keys
        if (GetAsyncKeyState(0x5B) as u16 & 0x8000) == 0
            && (GetAsyncKeyState(0x5C) as u16 & 0x8000) == 0
        {
            return false;
        }
    }
    true
}

/// 将按键名称（如 "Space", "K", "F1", "Enter"）转换为 VK 码。
fn key_name_to_vk(name: &str) -> Option<u32> {
    match name.to_uppercase().as_str() {
        "SPACE" => Some(VK_SPACE),
        "ENTER" | "RETURN" => Some(0x0D),
        "TAB" => Some(0x09),
        "ESCAPE" | "ESC" => Some(VK_ESCAPE),
        "BACKSPACE" | "BACK" => Some(0x08),
        "DELETE" | "DEL" => Some(0x2E),
        "INSERT" | "INS" => Some(0x2D),
        "HOME" => Some(0x24),
        "END" => Some(0x23),
        "PAGEUP" | "PGUP" => Some(0x21),
        "PAGEDOWN" | "PGDN" => Some(0x22),
        "UP" | "ARROWUP" => Some(0x26),
        "DOWN" | "ARROWDOWN" => Some(0x28),
        "LEFT" | "ARROWLEFT" => Some(0x25),
        "RIGHT" | "ARROWRIGHT" => Some(0x27),
        "PRINTSCREEN" => Some(0x2C),
        "CAPSLOCK" => Some(0x14),
        "NUMLOCK" => Some(0x90),
        "SCROLLLOCK" => Some(0x91),
        "APPS" | "CONTEXTMENU" => Some(0x5D),
        _ => {
            // Single character keys: A-Z, 0-9
            if name.len() == 1 {
                let ch = name.chars().next().unwrap();
                if ch.is_ascii_alphanumeric() {
                    Some(ch.to_ascii_uppercase() as u32)
                } else {
                    match ch {
                        '.' => Some(0xBE),
                        ',' => Some(0xBC),
                        '/' => Some(0xBF),
                        '\\' => Some(0xDC),
                        '-' => Some(0xBD),
                        '=' => Some(0xBB),
                        '[' => Some(0xDB),
                        ']' => Some(0xDD),
                        ';' => Some(0xBA),
                        '\'' => Some(0xDE),
                        '`' => Some(0xC0),
                        _ => None,
                    }
                }
            } else {
                // F1-F12
                if name.starts_with('F') && name.len() >= 2 {
                    name[1..].parse::<u32>().ok()
                        .filter(|&n| (1..=12).contains(&n))
                        .map(|n| 0x6F + n)
                } else {
                    None
                }
            }
        }
    }
}

/// 解析热键组合字符串 "Ctrl+Shift+K" → (modifiers_mask, vk_code)
fn parse_hotkey_combo(combo: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = combo.split('+').map(|s| s.trim()).collect();
    let mut mods = 0u32;
    let mut key_vk: Option<u32> = None;

    for part in &parts {
        if part.is_empty() {
            continue;
        }
        match part.to_uppercase().as_str() {
            "ALT" => mods |= MOD_ALT,
            "CTRL" | "CONTROL" => mods |= MOD_CONTROL,
            "SHIFT" => mods |= MOD_SHIFT,
            "META" | "WIN" | "WINDOWS" | "COMMAND" | "CMD" => mods |= MOD_META,
            key_str => {
                if key_vk.is_some() {
                    return None; // multiple non-modifier keys
                }
                key_vk = key_name_to_vk(key_str);
            }
        }
    }

    let vk = key_vk?;
    if mods == 0 {
        return None; // must have at least one modifier
    }
    Some((mods, vk))
}

/// 持久化热键配置路径（<exe_dir>\config\hotkey.json）
fn hotkey_config_path() -> std::path::PathBuf {
    crate::storage::lunac_root_dir()
        .join("config")
        .join("hotkey.json")
}

/// 保存热键配置到文件
fn save_hotkey_config(combo: &str) {
    let path = hotkey_config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = format!(r#"{{"combo":"{}"}}"#, combo);
    if let Err(e) = std::fs::write(&path, json) {
        eprintln!("[lunac::hotkey] Failed to save hotkey config: {}", e);
        let _ = io::stderr().flush();
    }
}

/// 从文件加载热键配置，失败返回 None
fn load_hotkey_config() -> Option<String> {
    let path = hotkey_config_path();
    let content = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    v.get("combo")?.as_str().map(|s| s.to_string())
}

/// 应用热键配置：解析组合字符串 → 更新原子变量 → 持久化 → 重装热键后端
/// （优先 RegisterHotKey，失败才装 LL 钩子 —— 因此支持任意改键组合）。
pub fn parse_and_set_hotkey(combo: &str) -> Result<(), String> {
    let (mods, vk) = parse_hotkey_combo(combo)
        .ok_or_else(|| format!("Invalid hotkey combo: {}", combo))?;
    HOTKEY_MODIFIERS.store(mods, Ordering::SeqCst);
    HOTKEY_VK.store(vk, Ordering::SeqCst);
    if let Ok(mut s) = HOTKEY_COMBO.lock() {
        *s = combo.to_string();
    }
    // 持久化：写入 <exe 根>\config\hotkey.json
    save_hotkey_config(combo);
    // 重装后端。启动早期（主窗口句柄未就绪）只设原子变量不重装 ——
    // 由 start_hotkey 末尾统一调用 install_hook_thread 完成初次装配。
    if MAIN_HWND.load(Ordering::SeqCst) != 0 {
        install_hook_thread();
    }
    Ok(())
}

/// 获取当前热键组合字符串（供前端查询）
pub fn get_current_hotkey_string() -> String {
    HOTKEY_COMBO.lock().map(|s| s.clone()).unwrap_or_else(|_| DEFAULT_HOTKEY.to_string())
}

/// 注入 dummy key (0xFF down+up)，打断前台窗口眼中的"孤立 Alt"序列，
/// 防止 Chromium/WebView2 及部分应用在 Alt up 时弹出系统菜单/聚焦菜单栏。
unsafe fn send_dummy_key() {
    let inputs = [
        Input {
            r#type: INPUT_KEYBOARD,
            ki: KeybdInput { vk: VK_DUMMY, scan: 0, flags: 0, time: 0, extra: 0 },
            _pad: [0; 8],
        },
        Input {
            r#type: INPUT_KEYBOARD,
            ki: KeybdInput { vk: VK_DUMMY, scan: 0, flags: KEYEVENTF_KEYUP, time: 0, extra: 0 },
            _pad: [0; 8],
        },
    ];
    SendInput(2, inputs.as_ptr(), std::mem::size_of::<Input>() as i32);
}

// ── 钩子回调（必须极快，禁止任何阻塞/窗口操作） ─────────────────

unsafe extern "system" fn keyboard_hook(n_code: i32, w_param: usize, l_param: isize) -> isize {
    if n_code >= 0 {
        let kb = &*(l_param as *const Kbdllhookstruct);
        let is_keydown = w_param == WM_KEYDOWN || w_param == WM_SYSKEYDOWN;
        let is_keyup = w_param == WM_KEYUP || w_param == WM_SYSKEYUP;

        // ── Configurable hotkey check ───────────────────────────
        let hotkey_vk = HOTKEY_VK.load(Ordering::SeqCst);
        let hotkey_mods = HOTKEY_MODIFIERS.load(Ordering::SeqCst);

        if kb.vk_code == hotkey_vk && modifiers_match(kb, hotkey_mods) {
            if is_keydown && !RECORDING.load(Ordering::SeqCst) {
                // 按住不放产生 key repeat → 防抖：250ms 内只 toggle 一次（但仍吞键）
                let now = GetTickCount();
                let last = LAST_TOGGLE_TICK.load(Ordering::SeqCst);
                if now.wrapping_sub(last) >= 250 {
                    LAST_TOGGLE_TICK.store(now, Ordering::SeqCst);
                    TOGGLE_COUNT_LL.fetch_add(1, Ordering::Relaxed);
                    send_dummy_key();
                    PostThreadMessageW(
                        HOOK_THREAD_ID.load(Ordering::SeqCst),
                        WM_APP_TOGGLE,
                        0,
                        0,
                    );
                }
                return 1; // 吞掉按键，抑制系统菜单
            }
            if is_keyup {
                // 吞掉配对 keyup，避免前台收到孤立事件
                return 1;
            }
        }

        // ── Esc：仅当 Lunac 窗口可见时拦截 ─────────────────
        if kb.vk_code == VK_ESCAPE && is_keydown {
            let main = MAIN_HWND.load(Ordering::SeqCst);
            let visible = main != 0 && IsWindowVisible(main) != 0;
            if visible {
                let now = GetTickCount();
                let last = LAST_ESC_TICK.load(Ordering::SeqCst);
                if now.wrapping_sub(last) >= 300
                    && LAST_ESC_TICK
                        .compare_exchange(last, now, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    PostThreadMessageW(HOOK_THREAD_ID.load(Ordering::SeqCst), WM_APP_ESC, 1, 0);
                }
                return 1; // 吞掉，由 Rust/前端处理
            }
        }
        if kb.vk_code == VK_ESCAPE && is_keyup {
            let main = MAIN_HWND.load(Ordering::SeqCst);
            if main != 0 && IsWindowVisible(main) != 0 {
                return 1; // 吞掉配对 keyup
            }
        }
    }
    CallNextHookEx(0, n_code, w_param, l_param)
}

// ── 窗口切换（纯 Win32，绕开 Tauri API 跨线程黑箱） ──────────────
// 实测 Tauri `w.hide()` 从后台线程调用在自身前台时静默失败（调用返回
// 但窗口不动）。ShowWindow 直接操作窗口状态，微秒级、立即生效。

const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
const SW_RESTORE: i32 = 9; // restore minimized window

pub fn hide_window() {
    let hwnd = MAIN_HWND.load(Ordering::SeqCst);
    if hwnd != 0 {
        unsafe { ShowWindow(hwnd, SW_HIDE) };
    }
}

/// 绕过 Windows 前台锁定：使用 AttachThreadInput 借用前台线程的前台权限，
/// 然后 SetForegroundWindow。AttachThreadInput 曾被禁用（因为它阻塞钩子回调），
/// 但现在 toggle_window 在独立 spawned 线程执行，钩子回调只做 PostThreadMessageW
/// （微秒级返回），不会受影响。
///
/// 游戏场景下 SetForegroundWindow 可能因前台锁定竞争而失败（游戏持续抢占前台），
/// 加入最多 3 次重试（间隔 16ms ≈ 1 帧）。
unsafe fn force_foreground(hwnd: isize) {
    let fg = GetForegroundWindow();
    if fg == hwnd {
        return; // already foreground
    }

    let fg_thread = GetWindowThreadProcessId(fg, std::ptr::null_mut());
    let our_thread = GetCurrentThreadId();

    // Attach to foreground thread → borrow its foreground activation rights
    let attached = if fg_thread != 0 && fg_thread != our_thread {
        AttachThreadInput(our_thread, fg_thread, 1) != 0
    } else {
        false
    };

    for attempt in 0..3 {
        if GetForegroundWindow() == hwnd {
            break;
        }
        if attempt > 0 {
            thread::sleep(std::time::Duration::from_millis(16));
        }
        // Don't use send_alt_unlock when AttachThreadInput is active —
        // the input injection could interfere with the foreground transition.
        SetForegroundWindow(hwnd);
    }

    if attached {
        AttachThreadInput(our_thread, fg_thread, 0);
    }

    if let Some(app) = APP.get() {
        let _ = app.run_on_main_thread(|| {
            if let Some(w) = APP.get().and_then(|a| a.get_webview_window("main")) {
                let _ = w.set_focus();
            }
        });
    }
}

/// Read file paths from clipboard (CF_HDROP format).
/// Returns an empty Vec if no files are on the clipboard.
unsafe fn read_clipboard_files() -> Vec<String> {
    let mut files = Vec::new();
    if OpenClipboard(0) == 0 {
        return files;
    }
    if IsClipboardFormatAvailable(CF_HDROP) == 0 {
        CloseClipboard();
        return files;
    }
    let h_data = GetClipboardData(CF_HDROP);
    if h_data != 0 {
        let h_global = GlobalLock(h_data);
        if h_global != 0 {
            let count = DragQueryFileW(h_global, 0xFFFFFFFF, std::ptr::null_mut(), 0);
            for i in 0..count {
                let mut buf = vec![0u16; 260]; // MAX_PATH
                let len = DragQueryFileW(h_global, i, buf.as_mut_ptr(), buf.len() as u32);
                if len > 0 {
                    buf.truncate(len as usize);
                    if let Ok(s) = String::from_utf16(&buf) {
                        files.push(s);
                    }
                }
            }
            GlobalUnlock(h_global);
        }
    }
    CloseClipboard();
    files
}

/// 通知前端「窗口已被唤出」，并在主线程读一次系统剪贴板。
///
/// **热键与托盘 / 菜单 / 单实例必须是同一条路（2026-09-19 修，不得只改一边）**：
/// 前端把 `lunac-window-shown` 当作唤出的**唯一**信号 —— 监听器里会退出详细搜索、
/// 重报界面层、抑制高度滑动并主动重跑当前查询。此前只有 `toggle_window()` 发这个事件，
/// `show_and_focus()`（托盘 / 菜单 / 单实例）完全不发 ⇒ 从那三条路唤出时前端停在旧状态：
/// 卡在上次搜索结果、大界面不退出、静态帧不刷新（用户报的「残影 / 卡在上次搜索页」）。
/// 顺序纪律见 docs/ai-spec.md §11 规则 31。
///
/// 剪贴板必须在主线程读（`run_on_main_thread`），否则 WebView2 会弹权限框；
/// 这里只排队、不入队等待，所以不会拖慢后面的 `force_foreground()`。
fn notify_window_shown() {
    let Some(app) = APP.get() else { return };
    let _ = app.emit("lunac-window-shown", ());

    // Read system clipboard on main thread and pass text directly to frontend.
    // This avoids the WebView2 permission prompt and thread-safety issues
    // that occur when reading clipboard from a spawned thread.
    let app_for_clip = app.clone();
    let _ = app.run_on_main_thread(move || {
        let mut payload = String::new();

        // Try reading text from clipboard
        match arboard::Clipboard::new() {
            Ok(mut cb) => {
                if let Ok(text) = cb.get_text() {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() && trimmed.len() < 2000 {
                        payload = trimmed.to_string();
                    }
                }
            }
            Err(_) => { /* clipboard inaccessible */ }
        }

        // Also check for files in clipboard (CF_HDROP)
        // `read_clipboard_files` 是 unsafe（裸 Win32 FFI，调用方保证已在主线程）——
        // 本闭包就是 `run_on_main_thread` 投递的，满足这个前提。
        let files = unsafe { read_clipboard_files() };
        for path in &files {
            if !payload.is_empty() { payload.push('\n'); }
            payload.push_str(path);
        }

        if !payload.is_empty() {
            let _ = app_for_clip.emit("lunac-clipboard", payload);
        }
    });
}

fn toggle_window() {
    let hwnd = MAIN_HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    unsafe {
        let is_minimized = IsIconic(hwnd) != 0;
        if IsWindowVisible(hwnd) != 0 && !is_minimized {
            ShowWindow(hwnd, SW_HIDE);
        } else {
            if is_minimized {
                ShowWindow(hwnd, SW_RESTORE);
            } else {
                ShowWindow(hwnd, SW_SHOW);
            }
            // 顺序（2026-09-17 重排，**不得再改回「force_foreground 在最前」**）：
            //   ① Notify frontend（本 if 块） → ② refresh_if_stale → ③ force_foreground
            // 为什么激活要挪到最后：`AttachThreadInput` 的等待时间不可控（它要接前台
            // 线程的输入队列），原来排在 emit 之前 ⇒ 前端必须等激活做完才收到
            // `lunac-window-shown`。而前端对这个事件的反应正是「主动重跑当前查询」的
            // 唯一入口（main.ts 的同名监听器），高负载下这段等待被放大，用户看到的就是
            // 「唤出后停在上次搜索结果」。emit 只是投递、不入队等待；下面排队到主线程的
            // 剪贴板读取也能与激活路径里的 sleep 并行执行。
            notify_window_shown();

            // ② 唤出时若应用列表文件比刷新间隔更旧 → 后台重扫（非阻塞）。
            // 搜索路径只读文件、永不扫描，所以这一步只影响「列表有多新」，
            // 不会让搜索等待。
            crate::app_indexer::refresh_if_stale();

            // ③ 窗口激活 —— 放最后，理由见上面那段顺序注释。
            force_foreground(hwnd);

            // Update toggle tick AFTER force_foreground completes, so the
            // foreground guard cooldown doesn't start until the window is
            // actually visible and in front. Previously this was set before
            // ShowWindow, causing the guard to auto-hide the window if
            // force_foreground took >500ms (e.g., AttachThreadInput +
            // Tauri set_focus IPC roundtrip).
            LAST_TOGGLE_TICK.store(GetTickCount(), Ordering::SeqCst);
        }
    }
}

// ── 公共 API ─────────────────────────────────────────────────────

/// Show the window and force it to foreground (for tray / menu / single-instance
/// "activate" signal).
///
/// **与 `toggle_window()` 的唤出路径完全同构（2026-09-19 修，不得回退成「只 ShowWindow」）**：
/// 顺序 = ① 先写一次 `LAST_TOGGLE_TICK`（挡住前台守卫在转场期间自动隐藏）→
/// ② `ShowWindow` → ③ `notify_window_shown()`（emit + 剪贴板）→ ④ `refresh_if_stale()`
/// → ⑤ `force_foreground()` → ⑥ 再写一次 `LAST_TOGGLE_TICK`（激活完成才算冷却起点，
/// 同规则 31）。此前本函数缺 ③，导致托盘 / 菜单 / 单实例唤出时前端收不到
/// `lunac-window-shown`：详细搜索不退出、结果不重跑、静态帧停在旧画面。
///
/// 两次写 `LAST_TOGGLE_TICK` 是刻意的：这里走的是 `SW_SHOW`（窗口此前不可见），
/// 若只在最后写，`force_foreground()` 期间守卫会读到上一次的旧 tick
/// （`now - last >= 2000`）而把刚显示的窗口又隐藏掉。
pub fn show_and_focus() {
    let hwnd = MAIN_HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    unsafe {
        // ① Prevent foreground guard from auto-hiding us during the transition
        LAST_TOGGLE_TICK.store(GetTickCount(), Ordering::SeqCst);

        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        } else {
            ShowWindow(hwnd, SW_SHOW);
        }
        // ③ 通知前端（与热键同一条路）
        notify_window_shown();
        // ④ 显示时若应用列表文件比刷新间隔更旧 → 后台重扫（非阻塞）
        crate::app_indexer::refresh_if_stale();
        // ⑤ 窗口激活 —— 放最后，理由见 toggle_window 的顺序注释
        force_foreground(hwnd);
        // ⑥ 激活完成才算冷却起点（规则 31）
        LAST_TOGGLE_TICK.store(GetTickCount(), Ordering::SeqCst);
    }
}

// ── RegisterHotKey 互补机制 ─────────────────────────────────────
// WH_KEYBOARD_LL 在以下场景会失效：
//   - 高负载下回调超时 → Windows 静默移除钩子
//   - 前台应用以管理员运行 (UIPI 隔离)
//   - 游戏独占全屏 (某些引擎绕过 LL 钩子)
// RegisterHotKey 是内核级热键注册，不受 UIPI 影响，但 Alt+Space
// 被 Windows 内核保留（无法注册），对自定义热键（Ctrl+Shift+K等）
// 可提供更可靠的兜底。

/// 尝试用 RegisterHotKey 注册当前热键（内核级、无钩子）。
/// 返回 true = 注册成功，**无需 LL 钩子**；false = 无法注册（Alt+Space 系统保留 /
/// 组合被其它程序占用 / 窗口句柄未就绪）→ 调用方回退 LL 钩子。
fn register_system_hotkey() -> bool {
    let hwnd = MAIN_HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return false;
    }
    let mods = HOTKEY_MODIFIERS.load(Ordering::SeqCst);
    let vk = HOTKEY_VK.load(Ordering::SeqCst);

    // Alt+Space 被 Windows 内核保留，RegisterHotKey 必然失败 → 直接判为需要钩子
    if mods == MOD_ALT && vk == VK_SPACE {
        eprintln!("[lunac::hotkey] Alt+Space 为系统保留组合，无法 RegisterHotKey → LL 钩子兜底");
        let _ = io::stderr().flush();
        return false;
    }

    unsafe {
        let ok = RegisterHotKey(hwnd, HOTKEY_ID as i32, mods | MOD_NOREPEAT, vk);
        if ok != 0 {
            eprintln!("[lunac::hotkey] RegisterHotKey OK (mods={:#x}, vk={:#x}) — 无钩子模式", mods, vk);
            true
        } else {
            eprintln!(
                "[lunac::hotkey] RegisterHotKey FAILED err={} — LL 钩子兜底",
                GetLastError()
            );
            false
        }
    }
}

fn unregister_system_hotkey() {
    let hwnd = MAIN_HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    unsafe {
        UnregisterHotKey(hwnd, HOTKEY_ID as i32);
    }
}

// ── 热键后端装配（可被看门狗 / 热键设置重复调用） ─────────────────
//
// 双模式（2026-09）：优先 RegisterHotKey（内核级、无钩子 → AV 误报最低、
// 不受前台程序权限影响）；仅当注册失败才安装 WH_KEYBOARD_LL 钩子兜底。
// 消息泵线程始终启动 —— 它同时承担 Esc 处理（WM_APP_ESC）与看门狗心跳，
// 无钩子时只是阻塞在 GetMessageW 上，几乎零开销。

fn install_hook_thread() {
    // 1) 卸载旧注册，并结束旧消息泵线程
    //    （避免热键变更/看门狗重装时累积多个泵 → 重复响应热键与 Esc）
    unregister_system_hotkey();
    let old_tid = HOOK_THREAD_ID.swap(0, Ordering::SeqCst);
    if old_tid != 0 {
        unsafe {
            PostThreadMessageW(old_tid, 0x0012 /* WM_QUIT */, 0, 0);
        }
        HOOK_MODE.store(false, Ordering::SeqCst);
        // 给旧泵一点时间退出（PostThreadMessageW 到已死线程安全无副作用）
        thread::sleep(std::time::Duration::from_millis(100));
    }

    // 2) 优先内核级注册；失败（Alt+Space 系统保留 / 组合被占用）才需钩子
    let registered = register_system_hotkey();
    let _ = io::stderr().flush();

    thread::spawn(move || unsafe {
        // 先创建消息队列，并记录线程 id 供回调 PostThreadMessageW 使用
        let mut msg: Msg = Msg::default();
        PeekMessageW(&mut msg, 0, 0, 0, PM_NOREMOVE);
        let my_tid = GetCurrentThreadId();
        HOOK_THREAD_ID.store(my_tid, Ordering::SeqCst);
        // Prime the watchdog with current tick so it doesn't trigger a
        // false restart immediately on startup (LAST_HOOK_PING defaults to 0).
        LAST_HOOK_PING.store(GetTickCount(), Ordering::SeqCst);

        let combo = get_current_hotkey_string();
        let restart = HOOK_RESTART_COUNT.load(Ordering::SeqCst);

        // 3) RegisterHotKey 成功 → 完全不装键盘钩子（AV 特征最小化路径）
        //    失败 → 安装 LL 钩子兜底
        let mut hook: isize = 0;
        if registered {
            HOOK_MODE.store(false, Ordering::SeqCst);
            eprintln!(
                "[lunac::hotkey] Hotkey ready via RegisterHotKey (no keyboard hook) ({})",
                combo
            );
        } else {
            // 先置 HOOK_MODE=true 再装钩子：若安装失败并提前 return，看门狗仍会
            // 因心跳停更而触发重装（恢复原有自愈能力）；装好后再打印成功日志。
            HOOK_MODE.store(true, Ordering::SeqCst);
            hook = SetWindowsHookExW(WH_KEYBOARD_LL, keyboard_hook, 0, 0);
            if hook == 0 {
                eprintln!("[lunac::hotkey] SetWindowsHookEx FAILED err={}", GetLastError());
                let _ = io::stderr().flush();
                RESTARTING_HOOK.store(false, Ordering::SeqCst);
                return;
            }
            if restart > 0 {
                eprintln!("[lunac::hotkey] Hook reinstalled (restart #{}) ({})", restart, combo);
            } else {
                eprintln!("[lunac::hotkey] Hotkey ready via LL hook fallback ({})", combo);
            }
        }
        let _ = io::stderr().flush();

        // 纯消息泵：阻塞在 GetMessageW 时系统才能调度钩子回调；
        // 收到回调 post 的自定义消息后立即转发，不做耗时操作
        loop {
            let ret = GetMessageW(&mut msg, 0, 0, 0);
            if ret == 0 || ret == -1 {
                break;
            }
            // 更新心跳时间戳 — 看门狗据此判断钩子存活
            LAST_HOOK_PING.store(GetTickCount(), Ordering::SeqCst);

            if msg.message == WM_APP_TOGGLE {
                thread::spawn(toggle_window);
            } else if msg.message == WM_APP_WATCHDOG_PING {
                // 看门狗心跳 — 空处理即可（GetMessageW 已证明存活）
            } else if msg.message == WM_APP_ESC {
                // lparam: 0=LL 钩子, 1=轮询兜底
                let src = if msg.l_param == 1 { "poll" } else { "hook" };
                if RECORDING.load(Ordering::SeqCst) {
                    let ok = APP
                        .get()
                        .map(|a| a.emit("lunac-esc-cancel-rec", ()).is_ok());
                    eprintln!("[lunac::hotkey] Esc({src}): recording -> cancel ok={:?}", ok);
                } else if UI_MODE.load(Ordering::SeqCst) != UI_MODE_MAIN {
                    // 处在插件 / 详细搜索等「大界面层」：层还没退完，一律交给前端逐级退出。
                    // **不看 query/chips 是否空**——详细搜索把内容放在自己的输入框里，
                    // 简洁搜索栏本来就是空的，按内容判空会在详情态误隐藏整个窗口。
                    let mode = UI_MODE.load(Ordering::SeqCst);
                    let ok = APP
                        .get()
                        .map(|a| a.emit("lunac-esc-clear", ()).is_ok());
                    eprintln!("[lunac::hotkey] Esc({src}): ui_mode={mode} (non-main) -> emit clear ok={:?}", ok);
                } else if QUERY_EMPTY.load(Ordering::SeqCst) && CHIPS_EMPTY.load(Ordering::SeqCst) {
                    eprintln!("[lunac::hotkey] Esc({src}): all empty -> hide");
                    hide_window();
                } else {
                    let ok = APP
                        .get()
                        .map(|a| a.emit("lunac-esc-clear", ()).is_ok());
                    eprintln!("[lunac::hotkey] Esc({src}): chips/query -> emit clear ok={:?}", ok);
                }
                let _ = io::stderr().flush();
            }
        }

        eprintln!("[lunac::hotkey] Message pump exited — unhooking");
        let _ = io::stderr().flush();
        if hook != 0 {
            UnhookWindowsHookEx(hook);
        }
        // 仅当自己仍是当前泵线程时才重置全局状态（防止旧线程退出时
        // 覆盖新线程刚设好的 HOOK_MODE / RESTARTING_HOOK）
        if HOOK_THREAD_ID.load(Ordering::SeqCst) == my_tid {
            HOOK_MODE.store(false, Ordering::SeqCst);
            RESTARTING_HOOK.store(false, Ordering::SeqCst);
        }
    });
}

pub fn start_hotkey(app: AppHandle) {
    // 加载持久化的热键配置；无配置则用 DEFAULT_HOTKEY。
    // 注意：必须经 parse_and_set_hotkey 统一设置 mods/vk/combo 三者 ——
    // 早期分支只改 combo 字符串，会让「界面显示 Ctrl+Alt+Space、实际仍按 Alt+Space」
    // （并因此误判为需装 LL 钩子，导致改键后 Alt+Space 仍能呼出）。
    let initial = load_hotkey_config().unwrap_or_else(|| DEFAULT_HOTKEY.to_string());
    if let Err(e) = parse_and_set_hotkey(&initial) {
        eprintln!(
            "[lunac::hotkey] Failed to apply hotkey '{}': {} — falling back to default",
            initial, e
        );
        let _ = parse_and_set_hotkey(DEFAULT_HOTKEY);
    }

    // 主线程：子类化主窗口，拦截 WM_SYSCOMMAND/SC_KEYMENU
    // （SetWindowSubclass 必须在窗口所属线程调用；setup 闭包正是主线程）
    if let Some(w) = app.get_webview_window("main") {
        if let Ok(hwnd) = w.hwnd() {
            MAIN_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
            unsafe {
                let ok = SetWindowSubclass(hwnd.0 as isize, main_window_subclass, SUBCLASS_ID, 0);
                if ok == 0 {
                    eprintln!("[lunac::hotkey] SetWindowSubclass FAILED");
                    let _ = io::stderr().flush();
                }
            }
        }
    }

    APP.set(app).expect("APP OnceLock already set");

    // ── 初始化钩子 ───────────────────────────────────────────────
    install_hook_thread();

    // ── 轮询 + 看门狗线程 ─────────────────────────────────────────
    // 40ms 间隔，同时担任三个角色：
    // 1. Esc 轮询兜底 — GetAsyncKeyState 读物理键状态，覆盖自身前台时
    //    LL 钩子收不到事件的场景
    // 2. 前台守卫 — 窗口可见但失去焦点时自动隐藏
    // 3. 钩子看门狗 — 每 2 秒检测 LL 钩子存活状态，死亡则自动重装
    thread::spawn(|| unsafe {
        let mut was_down = false;
        let mut watchdog_tick = 0u32;
        loop {
            thread::sleep(std::time::Duration::from_millis(40));
            let hwnd = MAIN_HWND.load(Ordering::SeqCst);
            if hwnd == 0 {
                continue;
            }

            // ── 看门狗：每 2 秒 (~50 次迭代) 检查钩子健康 ──────────
            // 仅 LL 钩子模式需要健康检查；RegisterHotKey 模式无钩子可死，
            // 只发心跳确认消息泵存活（否则会误判并反复重装）。
            watchdog_tick = watchdog_tick.wrapping_add(1);
            if watchdog_tick % 50 == 0 {
                if HOOK_MODE.load(Ordering::SeqCst) {
                    let ping = LAST_HOOK_PING.load(Ordering::SeqCst);
                    let now = GetTickCount();
                    // 钩子线程死亡 (ping 超过 5 秒未更新) 且未在重装中
                    if now.wrapping_sub(ping) > 5000
                        && !RESTARTING_HOOK.load(Ordering::SeqCst)
                        && RESTARTING_HOOK
                            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                            .is_ok()
                    {
                        HOOK_RESTART_COUNT.fetch_add(1, Ordering::Relaxed);
                        eprintln!(
                            "[lunac::hotkey] WATCHDOG: hook dead (last ping {}ms ago) — restarting #{}",
                            now.wrapping_sub(ping),
                            HOOK_RESTART_COUNT.load(Ordering::Relaxed)
                        );
                        let _ = io::stderr().flush();
                        // install_hook_thread 内部会向旧泵线程发 WM_QUIT 并卸载钩子，
                        // 防止钩子句柄泄漏（此处无需重复发送）。
                        install_hook_thread();
                    }
                }
                // PostThreadMessageW 让消息泵循环推进；
                // GetMessageW 本身不返回就证明活着，此 ping 作为二次确认
                let tid = HOOK_THREAD_ID.load(Ordering::SeqCst);
                if tid != 0 {
                    PostThreadMessageW(tid, WM_APP_WATCHDOG_PING, 0, 0);
                }
            }

            if IsWindowVisible(hwnd) == 0 {
                was_down = false;
                continue;
            }

            // ── Foreground guard ───────────────────────────────────────
            // 插件态可能正在跑长任务（AI 流式、OCR），不许失焦即隐藏；
            // 详细搜索是被动视图、没有在跑的东西，故照旧允许自动隐藏
            // —— 这里刻意只排除 PLUGIN，不是「非 Main 全排除」，以免顺手改了既有行为。
            if !DETACHED.load(Ordering::SeqCst) && UI_MODE.load(Ordering::SeqCst) != UI_MODE_PLUGIN {
                let fg = GetForegroundWindow();
                if fg != 0 && fg != hwnd {
                    let now = GetTickCount();
                    let last_toggle = LAST_TOGGLE_TICK.load(Ordering::SeqCst);
                    // Allow more time for force_foreground + clipboard read to finish.
            // LAST_TOGGLE_TICK is now set AFTER force_foreground completes,
            // so the window is actually ready. 2s is generous for any IPC.
            if now.wrapping_sub(last_toggle) >= 2000 {
                        eprintln!("[lunac::hotkey] auto-hide: visible but not foreground");
                        let _ = io::stderr().flush();
                        hide_window();
                    }
                    was_down = false;
                    continue;
                }
            }

            // ── Esc check (only when foreground) ────────────────────────
            let down = (GetAsyncKeyState(VK_ESCAPE as i32) as u16 & 0x8000) != 0;
            if down && !was_down {
                let now = GetTickCount();
                let last = LAST_ESC_TICK.load(Ordering::SeqCst);
                if now.wrapping_sub(last) >= 300
                    && LAST_ESC_TICK
                        .compare_exchange(last, now, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    PostThreadMessageW(HOOK_THREAD_ID.load(Ordering::SeqCst), WM_APP_ESC, 1, 1);
                }
            }
            was_down = down;
        }
    });
}
