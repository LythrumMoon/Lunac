// src-tauri/src/mouse_hook.rs
// 全局低级鼠标钩子（WH_MOUSE_LL）—— 桌宠「鼠标穿透」态下，Alt+右键 仍能唤出菜单
//
// ## 为什么需要它
// 桌宠窗（label = `plugin-pet`）开鼠标穿透后会被加上 `WS_EX_TRANSPARENT | WS_EX_LAYERED`
// （见 `plugin_window::plugin_window_set_click_through`），于是**它收不到任何鼠标消息** ——
// 前端自己的 contextmenu 监听在穿透态形同不存在。要在该限制下仍识别「Alt+右键」，
// 只能装一个**全局**低级鼠标钩子，从系统输入链路上旁听。
//
// ## 为什么不吞键（本模块最关键的纪律）
// 低级鼠标钩子处于**全系统**输入链路上，它的返回值会被系统解释成「是否吞掉这次点击」。
// 我们只想「额外观察一次 Alt+右键」，**绝不是**要拦截右键本身 —— 用户的右键菜单、拖拽、
// 绘图软件、游戏里的右键操作全在这条链路的下游。一旦命中时返回 1（吞掉），副作用是
// **全系统**级别的（最典型：别的软件右键失灵）。所以本钩子**一律** `CallNextHookEx`
// 原样放行：命中与否只决定「要不要多发一条事件」，绝不改变原始输入本身。
//
// ## 为什么懒启停（不常驻装钩子）
// LL 钩子意味着全系统每一次鼠标事件都要经过我们的回调：既有 AV 把「全局钩子」认作可疑
// 特征的代价（与 hotkey.rs 头注释同一口径），也有回调超时（`LowLevelHooksTimeout`，
// 默认 ~300ms）导致系统静默摘钩的风险。它只在**桌宠处于鼠标穿透态**这段窗口期才有意义，
// 因此**按需开关**：前端进穿透时 `pet_alt_rclick_watch(true)`，退穿透 / 关窗时
// `pet_alt_rclick_watch(false)`。装 / 卸都幂等（重复调用是安全的空操作）。
//
// ## 为什么钩子回调里只判断 + 投递，不直接 emit
// 回调必须微秒级返回（同 hotkey.rs 的 LL 钩子纪律）。命中时只做一次
// `GetAsyncKeyState` 判断 + 一次 `PostThreadMessageW`（把坐标打包投递给消息泵），
// 真正的 `emit` 放在**消息泵线程**里做 —— emit 要序列化 + 入队，属于「重活」，
// 不该在钩子回调里做。
//
// ## 清理：进程内不得留悬挂钩子
// 钩子回调是**本进程内的静态函数**，卸载模块时若仍挂着钩子，系统回调会打到已失效的
// 代码页 → 进程崩。所以 `plugin-pet` 窗 Destroyed、以及应用退出时，main.rs 都会调
// `mouse_hook::stop()`（幂等；且会 join 等钩子线程真正退出后才返回）。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;
use std::thread;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

// 复用 hotkey.rs 的 Win32 消息结构：`GetMessageW` / `PeekMessageW` 的参数是 `*mut Msg`，
// 而「同一个 Win32 符号在一个 crate 里只能有一种声明」—— 各自定义一份 Msg 会触发
// `clashing_extern_declarations`（见 main.rs / win_proc.rs 的同类备注）。
use crate::hotkey::Msg;

// ── Windows 常量 ─────────────────────────────────────────────────
const WH_MOUSE_LL: i32 = 14;
const WM_RBUTTONDOWN: usize = 0x0204;
const HC_ACTION: i32 = 0;
const VK_MENU: u32 = 0x12; // 通用 Alt（与 hotkey.rs 的修饰键检测同一口径）
const PM_NOREMOVE: u32 = 0x0000;
const WM_QUIT: u32 = 0x0012;
/// 自定义线程消息：钩子回调 → 消息泵（posted 消息会唤醒阻塞中的 GetMessageW）。
const WM_APP_ALT_RCLICK: u32 = 0x8001;

/// 命中时下发给前端的事件名。
pub const EVENT_NAME: &str = "pet-alt-rclick";
/// 桌宠窗口 label（= `plugin_window::label_for("pet")`）。
const PET_LABEL: &str = "plugin-pet";

// ── 事件载荷 ─────────────────────────────────────────────────────

/// `EVENT_NAME` 的载荷：光标**屏幕坐标**（`MSLLHOOKSTRUCT.pt` 的同一套绝对坐标）。
/// 做成结构体而不是裸数组：将来加字段（如按键来源）时接收方不会静默拿到 `undefined`。
#[derive(Clone, Debug, Serialize)]
pub struct AltRclickPayload {
    pub x: i32,
    pub y: i32,
}

// ── Windows 结构体 ───────────────────────────────────────────────

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct MousePoint {
    x: i32,
    y: i32,
}

/// `MSLLHOOKSTRUCT`（WH_MOUSE_LL 的 l_param 指向它）。
#[repr(C)]
struct Msllhookstruct {
    pt: MousePoint,
    mouse_data: u32,
    flags: u32,
    time: u32,
    dw_extra_info: usize,
}

// ── FFI（与 hotkey.rs 同签名；参数/返回值类型逐字一致，避免 clashing_extern_declarations）
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
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentThreadId() -> u32;
    fn GetLastError() -> u32;
}

// ── 全局状态 ─────────────────────────────────────────────────────

/// 当前消息泵线程 id（0 = 没有）。**只给钩子回调读**：回调里绝不能去 lock 下面那把
/// 生命周期锁（`stop()` 会持锁 join 等回调返回，回调再取同一把锁就构成死锁），
/// 所以这里用无锁原子量单独给回调访问。
static PUMP_TID: AtomicU32 = AtomicU32::new(0);

/// 钩子线程的生命周期记账：Some = 正在监听。
///
/// 用「线程句柄 + 线程 id」在**一把锁**下串行化启停，而不是像 hotkey.rs 那样只靠
/// 原子量 + `PostThreadMessage`：这里要装 / 卸可反复发生，必须保证「旧泵彻底停了，
/// 新泵才起」—— 否则快速 on→off→on 会留下两个泵、每次右键发**两条**事件。
/// 持锁期间 spawn + 收握手 +（卸时）join，天然满足这一互斥。
static WATCH: Mutex<Option<WatchThread>> = Mutex::new(None);

struct WatchThread {
    tid: u32,
    handle: thread::JoinHandle<()>,
}

// ── 钩子回调（必须极快：禁止任何阻塞 / 窗口操作 / emit）──────────
unsafe extern "system" fn mouse_hook(n_code: i32, w_param: usize, l_param: isize) -> isize {
    if n_code == HC_ACTION && w_param == WM_RBUTTONDOWN {
        let ms = &*(l_param as *const Msllhookstruct);
        // Alt 是否按下：无状态查询（物理层权威，与 hotkey.rs 的修饰键检测同一口径）。
        if (GetAsyncKeyState(VK_MENU as i32) as u16 & 0x8000) != 0 {
            let tid = PUMP_TID.load(Ordering::SeqCst);
            if tid != 0 {
                // 把屏幕坐标打包进 l_param（x 低 32 位 / y 高 32 位），避免在回调里
                // 加锁或用全局 Option。x/y 是 i32，原样占满各自 32 位。
                let packed = ((ms.pt.y as u32 as i64) << 32) | (ms.pt.x as u32 as i64);
                PostThreadMessageW(tid, WM_APP_ALT_RCLICK, 0, packed as isize);
            }
        }
    }
    // **不吞键**：无论是否命中，一律原样放行（见模块头「为什么不吞键」）。
    CallNextHookEx(0, n_code, w_param, l_param)
}

// ── 消息泵线程 ───────────────────────────────────────────────────

/// 装钩子 + 跑消息泵。就绪后经 `tx` 回传本线程 id（给 `start()` 记账 / 后续 stop 用）。
///
/// 只在 `SetWindowsHookExW` 失败时回传 `Err`；成功则回传 `Ok(tid)` 后进入泵循环。
/// 因此调用方的 `rx.recv()` **不会永久阻塞**：线程若在回传前 panic，`tx` 被 drop，
/// recv 会以断开错误返回。
fn pump(app: AppHandle, tx: mpsc::Sender<Result<u32, String>>) {
    unsafe {
        // 先创建本线程的消息队列（PostThreadMessageW 要求目标线程已有队列）。
        let mut msg: Msg = Msg::default();
        PeekMessageW(&mut msg, 0, 0, 0, PM_NOREMOVE);

        let hook = SetWindowsHookExW(WH_MOUSE_LL, mouse_hook, 0, 0);
        if hook == 0 {
            let e = GetLastError();
            let _ = tx.send(Err(format!("SetWindowsHookEx(WH_MOUSE_LL) err={e}")));
            return;
        }

        let tid = GetCurrentThreadId();
        PUMP_TID.store(tid, Ordering::SeqCst);
        let _ = tx.send(Ok(tid));

        loop {
            let ret = GetMessageW(&mut msg, 0, 0, 0);
            if ret == 0 || ret == -1 {
                break; // WM_QUIT（stop）或错误 → 结束泵
            }
            if msg.message == WM_APP_ALT_RCLICK {
                let packed = msg.l_param as u64;
                let x = (packed & 0xFFFF_FFFF) as u32 as i32;
                let y = ((packed >> 32) & 0xFFFF_FFFF) as u32 as i32;
                // 只在桌宠窗真的存在时投递，且用 `get_webview_window` 定向 —— 避免
                // 像 `AppHandle::emit` 那样广播、惊动其它同时开着的插件窗。
                if let Some(w) = app.get_webview_window(PET_LABEL) {
                    let _ = w.emit(EVENT_NAME, AltRclickPayload { x, y });
                }
            }
        }

        UnhookWindowsHookEx(hook);
        // 结束泵时清掉回调可见的 tid。此处无需 tid 守卫：stop() 是持锁 join 本线程的，
        // 只有本线程真的退出后 start() 才可能启动下一个泵。
        PUMP_TID.store(0, Ordering::SeqCst);
    }
}

// ── 启停（幂等）───────────────────────────────────────────────────

/// 开始监听（已在监听则空操作）。在锁内完成「spawn → 收握手」，保证调用返回时
/// 钩子已就绪、`WATCH` 已记账。
fn start(app: AppHandle) {
    let mut guard = match WATCH.lock() {
        Ok(g) => g,
        Err(_) => return, // 锁中毒：不冒险装钩子
    };
    if guard.is_some() {
        return; // 已在监听 → 幂等
    }

    let (tx, rx) = mpsc::channel::<Result<u32, String>>();
    let handle = thread::spawn(move || pump(app, tx));

    match rx.recv() {
        Ok(Ok(tid)) => {
            *guard = Some(WatchThread { tid, handle });
            crate::log::info("mouse_hook: WH_MOUSE_LL 已安装（桌宠 Alt+右键 监听开）");
        }
        Ok(Err(e)) => {
            crate::log::warn(&format!("mouse_hook: 安装失败 {e}"));
            let _ = handle.join();
        }
        Err(_) => {
            // 线程在回传前退出（panic / tx drop）—— join 只会立刻返回。
            crate::log::warn("mouse_hook: 钩子线程提前退出，未安装");
            let _ = handle.join();
        }
    }
}

/// 停止监听（没在监听则空操作）。幂等；**join 等钩子线程真正退出后才返回**，
/// 确保销毁窗口 / 退出进程时不会留下悬挂钩子。
pub fn stop() {
    let taken = match WATCH.lock() {
        Ok(mut g) => g.take(),
        Err(_) => return,
    };
    if let Some(wt) = taken {
        unsafe {
            PostThreadMessageW(wt.tid, WM_QUIT, 0, 0);
        }
        let _ = wt.handle.join();
        crate::log::info("mouse_hook: WH_MOUSE_LL 已卸载");
    }
}

/// 供 `main.rs` 的窗口 `Destroyed` 分支调用。**只认桌宠窗**：别的插件窗被销毁与
/// 鼠标钩子无关（一个窗不可能同时是 `plugin-pet`）；非桌宠窗是空操作。
pub fn on_plugin_window_destroyed(label: &str) {
    if label == PET_LABEL {
        stop();
    }
}

// ── 前端命令 ─────────────────────────────────────────────────────

/// 按需开关「桌宠穿透态 Alt+右键 唤出」的全局鼠标钩子。
///
/// 前端在桌宠**进入**鼠标穿透时发 `true`、**退出**穿透 / 关窗时发 `false`。
/// 幂等：重复 `true` / 重复 `false` 都是安全的空操作（见 `start` / `stop`）。
#[tauri::command]
pub fn pet_alt_rclick_watch(app: AppHandle, on: bool) -> Result<(), String> {
    if on {
        start(app);
    } else {
        stop();
    }
    Ok(())
}
