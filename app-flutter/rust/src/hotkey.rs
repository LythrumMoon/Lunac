//! 全局热键（v2）：Windows 走 `RegisterHotKey`，按下后累加计数并**向主窗投递消息**。
//!
//! 为什么这样分层（与 v1 `app/src-tauri/src/hotkey.rs` 的取舍一致，但只取当前需要的部分）：
//!   · `RegisterHotKey` 是内核级注册，**不装键盘钩子、不注入输入** —— AV 误报特征最低，
//!     且与前台是谁、什么权限无关（管理员程序 / 独占全屏游戏也能唤出）；
//!   · 热键线程只做一件事：收到 `WM_HOTKEY` 就 `request_wake()`（计数 +1 + `PostMessage`）；
//!   · **事件驱动**：Dart 启动时把主窗句柄交给本模块（`set_host_window`），唤出请求会以
//!     `WM_APP+1` / `WM_APP+2` 投递到主窗，runner 再通过 `lunac/wake` 通道转给 Dart。
//!     这样后台不需要轮询（用户 2026-11-09 裁决）；
//!   · 计数仍然保留：旧 DLL / 通道缺失时 Dart 会退回轮询（`take_wake` / `take_activate`），
//!     事件路径处理完也会把计数清空，两条路不会重复触发。
//!
//! **暂未实现 v1 的兜底路径**：v1 在 `RegisterHotKey` 失败（Alt+Space 被系统保留 /
//! 组合被别的程序占用）时会装 `WH_KEYBOARD_LL` 钩子。v2 默认组合是
//! `Ctrl+Alt+Space`（可注册），先不做钩子；失败只留日志。
//!
//! 改键（2026-11-09）：设置面板「通用 → 唤醒热键」调用 `set_combo` —— 请求投递给
//! 热键线程（`PostThreadMessageW`），由它 `UnregisterHotKey` + `RegisterHotKey` 换键；
//! 新键注册失败会**回滚旧键**（否则一次误改就把唤出弄没了），成功才写 `config/hotkey.json`。
//!
//! 配置：`<exe 根>/config/hotkey.json`（与 v1 同路径同格式 `{"combo":"Ctrl+Alt+Space"}`），
//! 迁移期与 v1 共用同一份配置。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(target_os = "windows")]
use std::sync::atomic::AtomicIsize;
#[cfg(target_os = "windows")]
use std::sync::{mpsc, Mutex, Once};
#[cfg(target_os = "windows")]
use std::time::Duration;

/// 默认热键：与 v1 一致（`Ctrl+Alt+Space` 可被 `RegisterHotKey` 注册）。
const DEFAULT_HOTKEY: &str = "Ctrl+Alt+Space";

/// 投递给主窗的唤醒消息（`WM_APP + 1`）；runner 与 Dart 侧共用这个约定。
pub const WAKE_MESSAGE: u32 = 0x8001;
/// 投递给主窗的「又启动了一次」消息（`WM_APP + 2`）。
pub const ACTIVATE_MESSAGE: u32 = 0x8002;
/// 投递给**热键线程**的「换键」消息（线程消息，不带窗口）。
#[cfg(target_os = "windows")]
const CONTROL_MESSAGE: u32 = 0x8003;

/// 主窗句柄（Dart 启动时通过 `lunac_set_host_window` 传入；0 = 还没绑定）。
#[cfg(target_os = "windows")]
static HOST_WINDOW: AtomicIsize = AtomicIsize::new(0);

/// 待处理的热键唤出次数（Dart 读一次即清零）。
static HOTKEY_TICKS: AtomicU32 = AtomicU32::new(0);

/// 待处理的「又启动了一次」次数。与热键分开：热键是**切换**显示 / 隐藏，
/// 再启动一次是**显示并聚焦**（v1 同语义）。
static ACTIVATE_TICKS: AtomicU32 = AtomicU32::new(0);

/// 热键线程的 id（换键请求用 `PostThreadMessageW` 叫醒它）。
#[cfg(target_os = "windows")]
static THREAD_ID: AtomicU32 = AtomicU32::new(0);

/// 当前实际注册的组合（`None` = 还没注册成功）。
#[cfg(target_os = "windows")]
static CURRENT_COMBO: Mutex<Option<String>> = Mutex::new(None);

/// 换键请求的投递端（热键线程创建时写入）。
#[cfg(target_os = "windows")]
static CONTROL: Mutex<Option<mpsc::Sender<ComboRequest>>> = Mutex::new(None);

#[cfg(target_os = "windows")]
struct ComboRequest {
    combo: String,
    reply: mpsc::Sender<Result<(), (i32, String)>>,
}

/// 绑定主窗句柄：之后唤出请求会直接 `PostMessage` 到它（事件驱动，无需轮询）。
pub fn set_host_window(hwnd: isize) {
    #[cfg(target_os = "windows")]
    {
        HOST_WINDOW.store(hwnd, Ordering::SeqCst);
        eprintln!("[lunac-hotkey] 已绑定主窗 hwnd={hwnd}（事件驱动唤出）");
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = hwnd;
    }
}

/// 向主窗投递一条消息（未绑定时什么也不做，靠计数 + 轮询兜底）。
#[cfg(target_os = "windows")]
fn post_to_host(message: u32) {
    let hwnd = HOST_WINDOW.load(Ordering::SeqCst);
    if hwnd != 0 {
        unsafe {
            win::PostMessageW(hwnd, message, 0, 0);
        }
    }
}

/// 登记一次热键按下。
pub fn request_wake() {
    HOTKEY_TICKS.fetch_add(1, Ordering::SeqCst);
    #[cfg(target_os = "windows")]
    post_to_host(WAKE_MESSAGE);
}

/// 登记一次「第二个实例启动」的唤出请求。
pub fn request_activate() {
    ACTIVATE_TICKS.fetch_add(1, Ordering::SeqCst);
    #[cfg(target_os = "windows")]
    post_to_host(ACTIVATE_MESSAGE);
}

/// 取走并在同一次原子操作里清零热键次数（Dart 侧轮询用）。
pub fn take_wake() -> u32 {
    HOTKEY_TICKS.swap(0, Ordering::SeqCst)
}

/// 取走并在同一次原子操作里清零「再启动一次」次数。
pub fn take_activate() -> u32 {
    ACTIVATE_TICKS.swap(0, Ordering::SeqCst)
}

/// 启动全局热键线程（幂等；配置读坏时回退默认组合）。
#[cfg(target_os = "windows")]
pub fn start(data_root: PathBuf) {
    static START: Once = Once::new();
    START.call_once(move || {
        let combo = load_combo(&data_root).unwrap_or_else(|| DEFAULT_HOTKEY.to_string());
        std::thread::spawn(move || unsafe { run_loop(combo) });
    });
}

#[cfg(not(target_os = "windows"))]
pub fn start(_data_root: PathBuf) {
    eprintln!("[lunac-hotkey] 全局热键暂只在 Windows 实现（Linux 待补）");
}

/// 当前生效的组合（`config/hotkey.json`；还没注册成功时也返回配置值）。
#[cfg(target_os = "windows")]
pub fn current_combo(root: &Path) -> String {
    if let Ok(guard) = CURRENT_COMBO.lock() {
        if let Some(combo) = guard.as_ref() {
            return combo.clone();
        }
    }
    load_combo(root).unwrap_or_else(|| DEFAULT_HOTKEY.to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn current_combo(root: &Path) -> String {
    load_combo(root).unwrap_or_else(|| DEFAULT_HOTKEY.to_string())
}

/// 改全局热键。错误码：1 = 组合非法，2 = 注册失败（被占用），3 = 热键线程不可用 / 超时。
#[cfg(target_os = "windows")]
pub fn set_combo(root: &Path, combo: &str) -> Result<(), (i32, String)> {
    let combo = combo.trim();
    if combo.is_empty() {
        return Err((1, "组合键不能为空".to_string()));
    }
    if parse_combo(combo).is_none() {
        return Err((
            1,
            "组合键格式不正确：至少一个修饰键 + 一个主键（如 Ctrl+Alt+Space）".to_string(),
        ));
    }
    let sender = {
        let guard = CONTROL
            .lock()
            .map_err(|_| (3, "热键线程状态损坏".to_string()))?;
        guard
            .clone()
            .ok_or_else(|| (3, "热键线程未启动".to_string()))?
    };
    let (reply_tx, reply_rx) = mpsc::channel();
    sender
        .send(ComboRequest {
            combo: combo.to_string(),
            reply: reply_tx,
        })
        .map_err(|_| (3, "热键线程已退出".to_string()))?;
    let thread_id = THREAD_ID.load(Ordering::SeqCst);
    if thread_id != 0 {
        unsafe {
            win::PostThreadMessageW(thread_id, CONTROL_MESSAGE, 0, 0);
        }
    }
    match reply_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(result) => result?,
        Err(_) => return Err((3, "改键超时（热键线程未响应）".to_string())),
    }
    save_combo(root, combo).map_err(|reason| (2, reason))
}

#[cfg(not(target_os = "windows"))]
pub fn set_combo(_root: &Path, _combo: &str) -> Result<(), (i32, String)> {
    Err((3, "全局热键暂只在 Windows 实现（Linux 待补）".to_string()))
}

#[cfg(target_os = "windows")]
pub(crate) mod win {
    pub const WM_HOTKEY: u32 = 0x0312;
    pub const MOD_ALT: u32 = 0x0001;
    pub const MOD_CONTROL: u32 = 0x0002;
    pub const MOD_SHIFT: u32 = 0x0004;
    pub const MOD_WIN: u32 = 0x0008;
    /// 注册时带上：按住不放只触发一次（系统级去抖）。
    pub const MOD_NOREPEAT: u32 = 0x4000;
    pub const HOTKEY_ID: i32 = 1;
    pub const PM_NOREMOVE: u32 = 0x0000;

    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    pub struct Msg {
        pub hwnd: isize,
        pub message: u32,
        pub w_param: usize,
        pub l_param: isize,
        pub time: u32,
        pub pt_x: i32,
        pub pt_y: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        pub fn RegisterHotKey(hwnd: isize, id: i32, modifiers: u32, vk: u32) -> i32;
        pub fn UnregisterHotKey(hwnd: isize, id: i32) -> i32;
        pub fn GetMessageW(msg: *mut Msg, hwnd: isize, filter_min: u32, filter_max: u32) -> i32;
        pub fn PostMessageW(hwnd: isize, message: u32, w_param: usize, l_param: isize) -> i32;
        pub fn PostThreadMessageW(id: u32, message: u32, w_param: usize, l_param: isize) -> i32;
        pub fn PeekMessageW(
            msg: *mut Msg,
            hwnd: isize,
            filter_min: u32,
            filter_max: u32,
            remove: u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetLastError() -> u32;
        pub fn GetCurrentThreadId() -> u32;
    }
}

/// 热键线程主循环：注册初始键 → 收 `WM_HOTKEY` 唤出 / 收 `CONTROL_MESSAGE` 换键。
#[cfg(target_os = "windows")]
unsafe fn run_loop(initial: String) {
    use win::*;

    // 先碰一次消息队列：`RegisterHotKey(hwnd = NULL)` 把 WM_HOTKEY 投递到**本线程**队列，
    // 没有队列时注册会失败。PeekMessage(PM_NOREMOVE) 只建队列、不取消息。
    let mut msg = Msg::default();
    PeekMessageW(&mut msg, 0, 0, 0, PM_NOREMOVE);

    let (tx, rx) = mpsc::channel::<ComboRequest>();
    if let Ok(mut guard) = CONTROL.lock() {
        *guard = Some(tx);
    }
    THREAD_ID.store(GetCurrentThreadId(), Ordering::SeqCst);

    let mut current = initial.clone();
    match parse_combo(&current) {
        Some((mods, vk)) => {
            if RegisterHotKey(0, HOTKEY_ID, mods | MOD_NOREPEAT, vk) == 0 {
                eprintln!(
                    "[lunac-hotkey] RegisterHotKey 失败 err={} combo={current}（组合可能被其它程序占用）",
                    GetLastError()
                );
            } else {
                eprintln!("[lunac-hotkey] 已注册 combo={current}");
                if let Ok(mut guard) = CURRENT_COMBO.lock() {
                    *guard = Some(current.clone());
                }
            }
        }
        None => eprintln!("[lunac-hotkey] 无法解析热键 '{current}'，全局热键未启用"),
    }

    loop {
        let ret = GetMessageW(&mut msg, 0, 0, 0);
        if ret <= 0 {
            break;
        }
        if msg.message == WM_HOTKEY {
            crate::hotkey::request_wake();
        } else if msg.message == CONTROL_MESSAGE {
            while let Ok(request) = rx.try_recv() {
                apply_combo(request, &mut current);
            }
        }
    }
}

/// 换键：先注销旧键再注册新键；注册失败回滚旧键（保证「改坏了还能唤出」）。
#[cfg(target_os = "windows")]
unsafe fn apply_combo(request: ComboRequest, current: &mut String) {
    use win::*;

    let Some((mods, vk)) = parse_combo(&request.combo) else {
        let _ = request.reply.send(Err((1, "组合键格式不正确".to_string())));
        return;
    };
    UnregisterHotKey(0, HOTKEY_ID);
    if RegisterHotKey(0, HOTKEY_ID, mods | MOD_NOREPEAT, vk) == 0 {
        if let Some((old_mods, old_vk)) = parse_combo(current) {
            RegisterHotKey(0, HOTKEY_ID, old_mods | MOD_NOREPEAT, old_vk);
        }
        let _ = request.reply.send(Err((
            2,
            format!("注册失败（可能被其它程序占用，err={}）", GetLastError()),
        )));
        return;
    }
    *current = request.combo;
    eprintln!("[lunac-hotkey] 已改键 combo={current}");
    if let Ok(mut guard) = CURRENT_COMBO.lock() {
        *guard = Some(current.clone());
    }
    let _ = request.reply.send(Ok(()));
}

/// 读 `<root>/config/hotkey.json` 的 `combo`；读不到 / 格式坏都返回 None（用默认键）。
fn load_combo(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("config").join("hotkey.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("combo")?.as_str().map(|s| s.trim().to_string())
}

/// 写 `<root>/config/hotkey.json`（临时文件 + rename，只保留 `combo`）。
#[cfg(target_os = "windows")]
fn save_combo(root: &Path, combo: &str) -> Result<(), String> {
    let dir = root.join("config");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = dir.join("hotkey.json");
    let text = serde_json::json!({ "combo": combo }).to_string();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写热键配置失败: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("替换热键配置失败: {e}"))?;
    Ok(())
}

/// `"Ctrl+Alt+Space"` → (修饰键掩码, 主键 VK)。至少一个修饰键，否则返回 None。
#[cfg(target_os = "windows")]
pub fn parse_combo(combo: &str) -> Option<(u32, u32)> {
    use win::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};

    let mut mods = 0u32;
    let mut key: Option<u32> = None;
    for part in combo.split('+').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        match part.to_uppercase().as_str() {
            "ALT" => mods |= MOD_ALT,
            "CTRL" | "CONTROL" => mods |= MOD_CONTROL,
            "SHIFT" => mods |= MOD_SHIFT,
            "META" | "WIN" | "WINDOWS" | "SUPER" => mods |= MOD_WIN,
            other => {
                if key.is_some() {
                    return None; // 只允许一个主键
                }
                key = key_name_to_vk(other);
            }
        }
    }
    let vk = key?;
    if mods == 0 {
        return None;
    }
    Some((mods, vk))
}

/// 键名 → 虚拟键码。只覆盖常用键（v1 的完整表在 `app/src-tauri/src/hotkey.rs`）。
#[cfg(target_os = "windows")]
fn key_name_to_vk(name: &str) -> Option<u32> {
    match name {
        "SPACE" => Some(0x20),
        "ENTER" | "RETURN" => Some(0x0D),
        "TAB" => Some(0x09),
        "ESC" | "ESCAPE" => Some(0x1B),
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
        "." => Some(0xBE),
        "," => Some(0xBC),
        "/" => Some(0xBF),
        "\\" => Some(0xDC),
        "-" => Some(0xBD),
        "=" => Some(0xBB),
        "[" => Some(0xDB),
        "]" => Some(0xDD),
        ";" => Some(0xBA),
        "'" => Some(0xDE),
        "`" => Some(0xC0),
        _ => {
            if name.len() == 1 {
                let ch = name.chars().next()?;
                if ch.is_ascii_alphanumeric() {
                    return Some(ch.to_ascii_uppercase() as u32);
                }
                return None;
            }
            let number = name.strip_prefix('F')?.parse::<u32>().ok()?;
            (1..=12).contains(&number).then_some(0x6F + number)
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use crate::hotkey::win::{MOD_ALT, MOD_CONTROL, MOD_SHIFT};

    #[test]
    fn parses_the_default_combo() {
        let (mods, vk) = parse_combo(DEFAULT_HOTKEY).expect("default combo must parse");
        assert_eq!(mods, MOD_CONTROL | MOD_ALT);
        assert_eq!(vk, 0x20);
    }

    #[test]
    fn parses_letters_function_keys_and_symbols() {
        assert_eq!(parse_combo("Ctrl+Shift+K"), Some((MOD_CONTROL | MOD_SHIFT, b'K' as u32)));
        assert_eq!(parse_combo("Alt+F5"), Some((MOD_ALT, 0x74)));
        assert_eq!(parse_combo("Win+/"), Some((win::MOD_WIN, 0xBF)));
    }

    #[test]
    fn rejects_combos_without_modifier_or_with_two_keys() {
        assert_eq!(parse_combo("Space"), None);
        assert_eq!(parse_combo("Ctrl+A+B"), None);
        assert_eq!(parse_combo("Ctrl+Nope"), None);
    }

    #[test]
    fn combo_config_round_trips_and_tolerates_bad_json() {
        let root = std::env::temp_dir().join(format!("lunac-hotkey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        assert_eq!(load_combo(&root), None);
        save_combo(&root, "Ctrl+Shift+K").expect("save combo");
        assert_eq!(load_combo(&root).as_deref(), Some("Ctrl+Shift+K"));

        std::fs::write(root.join("config").join("hotkey.json"), "{oops").expect("write bad json");
        assert_eq!(load_combo(&root), None);
        // 坏配置回退默认组合（不能让唤出彻底失效）。
        assert_eq!(current_combo(&root), DEFAULT_HOTKEY);
        let _ = std::fs::remove_dir_all(&root);
    }

    // 两个计数器都是进程级原子量，拆成两条用例会被 cargo 并行执行互相抢走计数；
    // 这里合并成一条，断言完全一致。
    #[test]
    fn wake_and_activate_counters_are_separate_and_consumed_once() {
        request_wake();
        request_wake();
        request_activate();
        assert!(take_wake() >= 2);
        assert_eq!(take_wake(), 0);
        assert!(take_activate() >= 1);
        assert_eq!(take_activate(), 0);
        assert_eq!(take_wake(), 0, "热键计数不能被单实例唤出污染");
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;

    // 同 Windows 段：进程级原子量不能拆成两条并行用例。
    #[test]
    fn wake_and_activate_counters_are_separate_and_consumed_once() {
        request_wake();
        request_wake();
        request_activate();
        assert!(take_wake() >= 2);
        assert_eq!(take_wake(), 0);
        assert!(take_activate() >= 1);
        assert_eq!(take_activate(), 0);
        assert_eq!(take_wake(), 0);
    }
}
