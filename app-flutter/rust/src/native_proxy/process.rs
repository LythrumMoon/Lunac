//! 进程名 → 路由规则 `PROCESS-NAME` 的支撑（**仅 Windows**）。
//!
//! 只回答一个问题：**本地某个端口上的入站连接，属于哪个进程**。
//! 做法是系统自带的两步（不引入第三方 crate）：
//!   1. `GetExtendedTcpTable(TCP_TABLE_OWNER_PID_ALL)` 里按「本地端口」找 PID；
//!   2. `QueryFullProcessImageNameW` 取可执行文件全路径，只留文件名。
//!
//! **诚实边界**：查不到（进程已退出 / 权限不足 / 非 Windows / 端口不在表里）
//! 一律返回 `None`，调用方把 `PROCESS-NAME` 规则当成**不命中**处理 —— 绝不猜。
//! 也不做缓存：端口 → PID 的映射会随进程退出而失效，缓存等于制造幽灵规则。
//!
//! 代价：一次全表枚举约 1–3 ms。所以只有**规则表里真的写了 `PROCESS-NAME`**
//! 才会走到这里（见 `Shared::needs_process`）。

use std::net::TcpStream;

/// 入站连接的对端端口 → 进程可执行文件名（如 `chrome.exe`）。
pub fn name_for_peer(stream: &TcpStream) -> Option<String> {
    let port = stream.peer_addr().ok()?.port();
    name_for_local_port(port)
}

/// 本地 `port` 上的 TCP 连接属于哪个进程（返回可执行文件名，不做大小写折叠）。
#[cfg(windows)]
pub fn name_for_local_port(port: u16) -> Option<String> {
    // `AF_INET` / `AF_INET6`（winsock2.h）；本地回环可能是任意一族，两族都要看。
    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    for family in [AF_INET, AF_INET6] {
        if let Some(pid) = pid_for_port(family, port) {
            if let Some(name) = process_name(pid) {
                return Some(name);
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub fn name_for_local_port(_port: u16) -> Option<String> {
    None
}

#[cfg(windows)]
fn pid_for_port(family: u32, port: u16) -> Option<u32> {
    use std::mem::size_of;
    use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
    };

    // 第一次调用只问「要多大」：返回 ERROR_INSUFFICIENT_BUFFER 是正常路径。
    let mut size = 0u32;
    let probe =
        unsafe { GetExtendedTcpTable(None, &mut size, false, family, TCP_TABLE_OWNER_PID_ALL, 0) };
    if probe != ERROR_INSUFFICIENT_BUFFER.0 || size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size as usize];
    let status = unsafe {
        GetExtendedTcpTable(
            Some(buffer.as_mut_ptr().cast()),
            &mut size,
            false,
            family,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        )
    };
    if status != 0 {
        return None;
    }
    // 布局：`u32 dwNumEntries` + 紧随其后的定长行数组（IPv4 24 字节 / IPv6 56 字节）。
    let count = u32::from_ne_bytes(buffer.get(..4)?.try_into().ok()?) as usize;
    let row_size = if family == 2 {
        size_of::<MIB_TCPROW_OWNER_PID>()
    } else {
        size_of::<MIB_TCP6ROW_OWNER_PID>()
    };
    debug_assert!(buffer.len() >= 4 + count * row_size);
    for index in 0..count {
        let offset = 4 + index * row_size;
        let bytes = buffer.get(offset..offset + row_size)?;
        let (raw_port, pid) = if family == 2 {
            let row: MIB_TCPROW_OWNER_PID =
                unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast()) };
            (row.dwLocalPort, row.dwOwningPid)
        } else {
            let row: MIB_TCP6ROW_OWNER_PID =
                unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast()) };
            (row.dwLocalPort, row.dwOwningPid)
        };
        // 结构体里的端口是**网络字节序**（netstat 那个 0x5000 写法）。
        if u16::from_be(raw_port as u16) == port && pid != 0 {
            return Some(pid);
        }
    }
    None
}

#[cfg(windows)]
fn process_name(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = vec![0u16; 1024];
    let mut length = buffer.len() as u32;
    let query = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    query.ok()?;
    let path = String::from_utf16_lossy(&buffer[..length as usize]);
    Some(
        path.rsplit(['\\', '/'])
            .next()
            .unwrap_or(path.as_str())
            .to_string(),
    )
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn a_local_connection_resolves_to_this_test_binary() {
        // 自己连自己：入站的本地端口就是 `listener.peer_addr()` 对端的源端口。
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let name = name_for_peer(&server).expect("本进程的入站连接必须能查到进程名");
        let expected = std::env::current_exe()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(name.to_ascii_lowercase(), expected.to_ascii_lowercase());
        drop(client);
    }

    #[test]
    fn an_unused_port_has_no_owner() {
        // 绑定后立刻释放的端口不会出现在表里（理论上也可能被系统复用，所以只断言不 panic）。
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let _ = name_for_local_port(port);
    }
}
