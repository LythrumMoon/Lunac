//! 代理插件的宿主侧（移植 v1 `app/src-tauri/src/system_proxy.rs` 的读写纪律）。
//!
//! 本版只做「不依赖外置内核」的闭环：
//!   · **Windows 系统代理**的读 / 写 / 备份还原（注册表三值 + `InternetSetOptionW` 通知）；
//!   · 状态 JSON：系统代理 / 本地服务 / mihomo 内核可用性 + 搜索路径。
//!
//! **未移植**（如实降级，面板会显示原因）：
//!   · v1 的自研代理内核 `native_proxy`（20+ 文件的协议实现：ss / trojan / vless / vmess /
//!     reality / hysteria2 …）与 mihomo 内核管理（v1 已于 2026-11-01 删除）；
//!   · `proxy_server.rs` 也不移植：那是宿主内置的 Anthropic→OpenAI 协议桥（AI 对话链路），
//!     不是代理插件的「本地代理服务」。
//!
//! 纪律（逐条照 v1 `system_proxy.rs` 模块头，少一条就对应一个具体坏表现）：
//!   ① 写完注册表必须发两次 `InternetSetOptionW`：`SETTINGS_CHANGED` 让它重读注册表、
//!      `REFRESH` 让**已建立**的连接也按新设置走；只发前一个是「一部分程序生效」；
//!   ② 系统原值只在**第一次启用**时备份，判据只有 `system_backup.is_none()`；
//!   ③ 关闭时优先还原备份；**没有备份就什么都不做** —— 那种情况下「关掉」等于抹掉
//!      用户自己配的代理，是纯粹的数据损失。
//!
//! 可测性：注册表值解析 / 地址规范化 / ProxyOverride 拼接都是纯函数；
//! 单测**不写系统代理**，真实写入只发生在运行时。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 系统代理在注册表里的位置（`HKCU` 下，所以**不需要管理员**）。
pub const INTERNET_SETTINGS: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

/// 系统代理设置。`server` = `http://127.0.0.1:8080` 这类，`bypass` = 分号分隔的例外表。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemProxy {
    pub enabled: bool,
    pub server: String,
    pub bypass: String,
}

/// 内置默认绕过表（v1 `system_proxy.rs:184` 原样）。
///
/// **必须包含 `<local>`**：不加它访问 `127.0.0.1` 也会被送去代理，而 Lunac 自己就靠
/// 本机的几个服务活着，那会自己把自己绕死。
pub const DEFAULT_BYPASS: &str = "<local>;localhost;127.*;10.*;172.16.*;172.17.*;172.18.*;172.19.*;172.20.*;172.21.*;172.22.*;172.23.*;172.24.*;172.25.*;172.26.*;172.27.*;172.28.*;172.29.*;172.30.*;172.31.*;192.168.*";

/// 本地代理服务未启动时的原因（面板如实显示）。
pub const LOCAL_SERVICE_REASON: &str =
    "自研代理引擎未启动：在代理面板里粘贴订阅 / 分享链接后点「启动」";

/// 内核未安装的原因。
pub const KERNEL_MISSING_REASON: &str =
    "未找到 mihomo 内核；本版也未移植内核管理，无法启动内核。可继续使用系统代理，或把 mihomo.exe 放到 <exe 根>/Modules/proxy/mihomo";

/// 找到了 mihomo.exe 但本版不能启动它的原因。
pub const KERNEL_UNMANAGED_REASON: &str =
    "找到了 mihomo.exe，但本版未移植内核管理，无法启动；请继续使用 Windows 系统代理";

fn config_path(root: &Path) -> PathBuf {
    root.join("config").join("proxy.json")
}

/// 读 `config/proxy.json` 的 `system_backup`（文件不存在 / 解析失败都按「没有备份」）。
fn load_backup(root: &Path) -> Option<SystemProxy> {
    let text = std::fs::read_to_string(config_path(root)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    serde_json::from_value(value.get("system_backup")?.clone()).ok()
}

/// 只改 `system_backup` 这一个键，**保留文件里的其它字段**。
///
/// v1 的 `config/proxy.json` 还有 `entries` / `active` / `mihomo` 等段；整份覆盖会把
/// 它们抹掉（v1 自己专门防过这条数据损失路径）。本版用 `Value` 就地增删。
fn store_backup(root: &Path, backup: Option<SystemProxy>) -> Result<(), String> {
    let path = config_path(root);
    let mut value = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<serde_json::Value>(&text)
            .unwrap_or_else(|_| serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    };
    let Some(object) = value.as_object_mut() else {
        return Err("config/proxy.json 不是 JSON 对象，未改动".to_string());
    };
    match backup {
        Some(backup) => {
            let item = serde_json::to_value(backup)
                .map_err(|e| format!("序列化系统代理备份失败：{e}"))?;
            object.insert("system_backup".to_string(), item);
        }
        None => {
            object.remove("system_backup");
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建 config 目录失败：{e}"))?;
    }
    let text = serde_json::to_string_pretty(&value)
        .map_err(|e| format!("序列化代理配置失败：{e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写 config/proxy.json 失败：{e}"))
}

// ── 纯函数（单测覆盖，不碰系统）─────────────────────────────────────

/// `ProxyEnable` 存的是 0/1 的 DWORD（不是 bool）；**非 0 一律算开启**（v1 同口径）。
fn dword_to_enabled(value: u32) -> bool {
    value != 0
}

/// 代理地址规范化：trim → 补 scheme → 校验 scheme / 主机 / 端口 → 去掉尾斜杠。
///
/// WinINet 认 `http://host:port` / `https://host:port` / `socks5://host:port`，
/// 也认不带 scheme 的老写法 `host:port`（按 HTTP 代理）。其余 scheme 一律拒绝：
/// 写进 `ProxyServer` 也不会生效，不如在这里给一句人话。
fn normalize_server(server: &str) -> Result<String, String> {
    let raw = server.trim();
    if raw.is_empty() {
        return Err("代理地址不能为空".into());
    }
    if raw.chars().any(|c| c.is_whitespace()) {
        return Err("代理地址里不能有空格或换行".into());
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let (scheme, rest) = with_scheme
        .split_once("://")
        .ok_or_else(|| "代理地址格式不对，形如 http://127.0.0.1:8080".to_string())?;
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "socks5") {
        return Err("只支持 http / https / socks5 代理".into());
    }
    let rest = rest.trim_end_matches('/');
    let (host, port) = rest
        .rsplit_once(':')
        .ok_or_else(|| "代理地址要带端口，形如 http://127.0.0.1:8080".to_string())?;
    if host.trim().is_empty() {
        return Err("代理地址缺少主机名，形如 http://127.0.0.1:8080".into());
    }
    let port: u16 = port
        .parse()
        .map_err(|_| "代理端口要填数字（1–65535）".to_string())?;
    if port == 0 {
        return Err("代理端口要填数字（1–65535）".into());
    }
    Ok(format!("{scheme}://{host}:{port}"))
}

/// 把多个例外项拼成 WinINet 认的分号串：去空白、去空项、去重、保持原顺序。
fn join_bypass(items: &[&str]) -> String {
    let mut out: Vec<&str> = Vec::new();
    for item in items {
        let value = item.trim();
        if value.is_empty() || out.contains(&value) {
            continue;
        }
        out.push(value);
    }
    out.join(";")
}

/// ProxyOverride 规范化：空串用内置默认表，否则按分号拆开再拼（去空 / 去重）。
fn normalize_bypass(raw: &str) -> String {
    if raw.trim().is_empty() {
        return DEFAULT_BYPASS.to_string();
    }
    join_bypass(&raw.split(';').collect::<Vec<_>>())
}

// ── 系统代理：读 / 写（平台分派）───────────────────────────────────

fn read_system_proxy() -> SystemProxy {
    #[cfg(target_os = "windows")]
    {
        imp::read()
    }
    #[cfg(not(target_os = "windows"))]
    {
        SystemProxy::default()
    }
}

fn write_system_proxy(proxy: &SystemProxy) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        imp::write(proxy)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = proxy;
        Err("系统代理目前只支持 Windows".to_string())
    }
}

/// 启用系统代理（**备份只做一次**）。返回回读到的真实现状。
///
/// 顺序：先校验地址 → 再落盘备份 → 最后写注册表。备份先落盘是为了「写注册表成功、
/// 备份却没存下来」这种最坏情况（用户原来的代理就再也还原不回去了）。
fn enable_system_proxy(root: &Path, server: &str, bypass: &str) -> Result<SystemProxy, String> {
    let server = normalize_server(server)?;
    if load_backup(root).is_none() {
        store_backup(root, Some(read_system_proxy()))?;
    }
    let want = SystemProxy {
        enabled: true,
        server,
        bypass: normalize_bypass(bypass),
    };
    write_system_proxy(&want)?;
    // 回读真值：注册表写成功但被组策略压回去是可能的，界面要显示**实际生效**的那条。
    Ok(read_system_proxy())
}

/// 还原系统代理。**没有备份时什么都不做**（并如实告诉用户为什么）。
fn disable_system_proxy(root: &Path) -> Result<SystemProxy, String> {
    let Some(original) = load_backup(root) else {
        return Err(
            "系统代理不是由 Lunac 开启的（没有备份），未改动系统设置；请到 Windows「设置 → 网络和 Internet → 代理」里关闭"
                .to_string(),
        );
    };
    write_system_proxy(&original)?;
    store_backup(root, None)?;
    Ok(read_system_proxy())
}

// ── 状态 / 动作（C ABI 的 JSON 形状）───────────────────────────────

/// 内核搜索路径（按优先级：插件目录 → exe 根 → 另一常见落点）。
fn kernel_search_paths(root: &Path) -> Vec<String> {
    let exe = if cfg!(windows) { "mihomo.exe" } else { "mihomo" };
    [
        root.join("Modules").join("proxy").join("mihomo").join(exe),
        root.join("mihomo").join(exe),
        root.join("Modules").join("mihomo").join(exe),
    ]
    .iter()
    .map(|path| path.to_string_lossy().to_string())
    .collect()
}

/// 代理状态（系统代理 / 本地服务 / 内核）。只**读**系统代理，不改任何设置。
pub fn status_json(root: &Path) -> String {
    let system = read_system_proxy();
    let backup = load_backup(root).is_some();
    let search_paths = kernel_search_paths(root);
    let found = search_paths
        .iter()
        .find(|path| Path::new(path).is_file())
        .cloned();
    let engine_reason = if found.is_some() {
        KERNEL_UNMANAGED_REASON
    } else {
        KERNEL_MISSING_REASON
    };
    // 自研引擎的真实状态（`native_proxy::status_json`）：面板的「本地服务」一栏直接用它。
    let engine: serde_json::Value = serde_json::from_str(&crate::native_proxy::status_json())
        .unwrap_or_else(|_| serde_json::json!({}));
    let engine_running = engine
        .get("running")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let engine_reason_text = if engine_running {
        String::new()
    } else {
        engine
            .get("error")
            .and_then(|v| v.as_str())
            .filter(|text| !text.trim().is_empty())
            .unwrap_or(LOCAL_SERVICE_REASON)
            .to_string()
    };
    serde_json::json!({
        "supported": cfg!(target_os = "windows"),
        "system": {
            "enabled": system.enabled,
            "server": system.server,
            "bypass": system.bypass,
        },
        "backup": backup,
        "local": {
            "supported": true,
            "running": engine_running,
            "port": engine.get("port").and_then(|v| v.as_u64()).unwrap_or(0),
            "reason": engine_reason_text,
            "active": engine.get("active").and_then(|v| v.as_u64()).unwrap_or(0),
            "nodes": engine.get("nodes").cloned().unwrap_or_else(|| serde_json::json!([])),
            "skipped": engine.get("skipped").cloned().unwrap_or_else(|| serde_json::json!([])),
        },
        "kernel": {
            "available": false,
            "installed": found.is_some(),
            "path": found.unwrap_or_default(),
            "searchPaths": search_paths,
            "reason": engine_reason,
        },
    })
    .to_string()
}

/// 开关系统代理，返回 `ok` / `error` / `system`（失败时 `system` 仍是实际状态）。
pub fn set_system_json(root: &Path, enabled: bool, server: &str) -> String {
    let result = if enabled {
        enable_system_proxy(root, server, "")
    } else {
        disable_system_proxy(root)
    };
    match result {
        Ok(now) => serde_json::json!({
            "ok": true,
            "error": "",
            "system": {
                "enabled": now.enabled,
                "server": now.server,
                "bypass": now.bypass,
            },
        })
        .to_string(),
        Err(reason) => {
            eprintln!("[lunac-proxy] 系统代理设置失败: {reason}");
            let now = read_system_proxy();
            serde_json::json!({
                "ok": false,
                "error": reason,
                "system": {
                    "enabled": now.enabled,
                    "server": now.server,
                    "bypass": now.bypass,
                },
            })
            .to_string()
        }
    }
}

// ── Windows：注册表 + WinINet 通知（手写 FFI，不新增 crate）──────────

#[cfg(target_os = "windows")]
mod imp {
    use super::{dword_to_enabled, SystemProxy, INTERNET_SETTINGS};
    use std::ffi::c_void;

    type Hkey = isize;
    /// HKEY_CURRENT_USER（预定义句柄，不是真实指针）。
    const HKEY_CURRENT_USER: Hkey = 0x8000_0001u32 as i32 as isize;
    const KEY_SET_VALUE: u32 = 0x0002;
    const REG_SZ: u32 = 1;
    const REG_DWORD: u32 = 4;
    const ERROR_SUCCESS: i32 = 0;

    /// 只接受对应类型：类型不符时让 `RegGetValueW` 直接返回错误。
    const RRF_RT_REG_SZ: u32 = 0x0000_0002;
    const RRF_RT_REG_DWORD: u32 = 0x0000_0010;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueW(
            hkey: Hkey,
            sub_key: *const u16,
            value: *const u16,
            flags: u32,
            data_type: *mut u32,
            data: *mut c_void,
            data_len: *mut u32,
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
        fn RegCloseKey(hkey: Hkey) -> i32;
    }

    // ── WinINet 的「通知」──
    // 只为两个常量调两次函数，不值得拉 `windows` crate 的 feature（与 v1 同判据）。
    #[link(name = "wininet")]
    extern "system" {
        fn InternetSetOptionW(
            h: *mut c_void,
            option: u32,
            buf: *mut c_void,
            len: u32,
        ) -> i32;
    }
    /// `INTERNET_OPTION_SETTINGS_CHANGED`：设置变了，重读注册表。
    const OPT_SETTINGS_CHANGED: u32 = 39;
    /// `INTERNET_OPTION_REFRESH`：把新设置推给已建立的连接。
    const OPT_REFRESH: u32 = 37;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn reg_read_dword(value: &str) -> Option<u32> {
        let sub_key = wide(INTERNET_SETTINGS);
        let name = wide(value);
        let mut data: u32 = 0;
        let mut len: u32 = std::mem::size_of::<u32>() as u32;
        let mut data_type: u32 = 0;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                sub_key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                &mut data_type,
                &mut data as *mut u32 as *mut c_void,
                &mut len,
            )
        };
        if status == ERROR_SUCCESS && len == std::mem::size_of::<u32>() as u32 {
            Some(data)
        } else {
            None
        }
    }

    fn reg_read_string(value: &str) -> Option<String> {
        let sub_key = wide(INTERNET_SETTINGS);
        let name = wide(value);
        // 先探长度（data 传空），再按实际字节数分配。
        let mut len: u32 = 0;
        let mut data_type: u32 = 0;
        let probe = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                sub_key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                &mut data_type,
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if probe != ERROR_SUCCESS || len < 2 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                sub_key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                &mut data_type,
                buf.as_mut_ptr() as *mut c_void,
                &mut len,
            )
        };
        if status != ERROR_SUCCESS {
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

    /// 读三个值；读不到一律按「没设代理」处理（键不存在不是错误）。
    pub fn read() -> SystemProxy {
        SystemProxy {
            enabled: reg_read_dword("ProxyEnable")
                .map(dword_to_enabled)
                .unwrap_or(false),
            server: reg_read_string("ProxyServer").unwrap_or_default(),
            bypass: reg_read_string("ProxyOverride").unwrap_or_default(),
        }
    }

    fn write_dword(key: Hkey, name: &str, value: u32) -> Result<(), String> {
        let name = wide(name);
        let bytes = value.to_le_bytes();
        let status = unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_DWORD,
                bytes.as_ptr(),
                bytes.len() as u32,
            )
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err("写 ProxyEnable 失败（系统代理设置不可写）".to_string())
        }
    }

    /// 空值一律**删键**而不是写空串：写空串等于「代理开着、服务器是空的」，
    /// WinINet 会把所有请求直接发失败（v1 `put_or_delete` 同口径）。
    fn put_or_delete(key: Hkey, name: &str, value: &str) -> Result<(), String> {
        let name_w = wide(name);
        if value.is_empty() {
            unsafe {
                let _ = RegDeleteValueW(key, name_w.as_ptr());
            }
            return Ok(());
        }
        let bytes: Vec<u8> = value
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        let status = unsafe {
            RegSetValueExW(
                key,
                name_w.as_ptr(),
                0,
                REG_SZ,
                bytes.as_ptr(),
                bytes.len() as u32,
            )
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("写 {name} 失败（系统代理设置不可写）"))
        }
    }

    fn notify() {
        unsafe {
            InternetSetOptionW(
                std::ptr::null_mut(),
                OPT_SETTINGS_CHANGED,
                std::ptr::null_mut(),
                0,
            );
            InternetSetOptionW(
                std::ptr::null_mut(),
                OPT_REFRESH,
                std::ptr::null_mut(),
                0,
            );
        }
    }

    /// 写三个值并发通知（`create_subkey`：键不存在时新建，全新用户 / 被清理过也不能崩）。
    pub fn write(proxy: &SystemProxy) -> Result<(), String> {
        let sub_key = wide(INTERNET_SETTINGS);
        let mut key: Hkey = 0;
        let mut disposition: u32 = 0;
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                sub_key.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_SET_VALUE,
                std::ptr::null_mut(),
                &mut key,
                &mut disposition,
            )
        };
        if status != ERROR_SUCCESS {
            return Err("打开 Internet Settings 失败（系统代理设置不可写）".to_string());
        }
        let result = (|| -> Result<(), String> {
            write_dword(key, "ProxyEnable", u32::from(proxy.enabled))?;
            put_or_delete(key, "ProxyServer", &proxy.server)?;
            put_or_delete(key, "ProxyOverride", &proxy.bypass)?;
            Ok(())
        })();
        unsafe {
            RegCloseKey(key);
        }
        result?;
        notify();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-proxy-test-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    /// ProxyEnable 是 DWORD：0 = 关，非 0 = 开（v1 `!= 0` 同口径）。
    #[test]
    fn dword_flag_maps_to_enabled() {
        assert!(!dword_to_enabled(0));
        assert!(dword_to_enabled(1));
        assert!(dword_to_enabled(2));
    }

    /// 地址规范化：补 scheme、统一小写、去尾斜杠；坏输入在**写注册表之前**拦住。
    #[test]
    fn normalize_server_adds_scheme_and_rejects_bad_input() {
        assert_eq!(
            normalize_server("127.0.0.1:8080").unwrap(),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            normalize_server("  SOCKS5://10.0.0.1:1080/ ").unwrap(),
            "socks5://10.0.0.1:1080"
        );
        assert_eq!(
            normalize_server("https://proxy.example.com:443").unwrap(),
            "https://proxy.example.com:443"
        );

        assert!(normalize_server("").is_err());
        assert!(normalize_server("   ").is_err());
        // 有空格 = WinINet 会把整串当非法条目，表现为「开了但没走」。
        assert!(normalize_server("http://127.0.0.1:8080 hello").is_err());
        assert!(normalize_server("127.0.0.1").is_err(), "没端口");
        assert!(normalize_server("ftp://127.0.0.1:21").is_err(), "不支持的 scheme");
        assert!(normalize_server("http://127.0.0.1:0").is_err(), "端口 0");
        assert!(normalize_server("http://:8080").is_err(), "缺主机名");
    }

    /// ProxyOverride 拼接：去空白 / 去空项 / 去重、保持顺序；空串回落到默认表。
    #[test]
    fn join_bypass_trims_dedupes_and_keeps_order() {
        assert_eq!(
            join_bypass(&[" <local> ", "localhost", "", "localhost", "127.*"]),
            "<local>;localhost;127.*"
        );
        assert_eq!(join_bypass(&[]), "");
        assert_eq!(normalize_bypass(""), DEFAULT_BYPASS);
        assert_eq!(normalize_bypass(" <local> ; 10.* ;; "), "<local>;10.*");
    }

    /// 默认绕过表必须包含本机 / 局域网 —— 否则 Lunac 自己会被送去代理。
    #[test]
    fn default_bypass_never_sends_loopback_to_the_proxy() {
        assert!(DEFAULT_BYPASS.contains("<local>"));
        assert!(DEFAULT_BYPASS.contains("127.*"));
        assert!(DEFAULT_BYPASS.contains("localhost"));
    }

    /// 状态 JSON 的字段名与 Dart `parseProxyStatus` 一一对应。
    /// 只读注册表，**不写**系统代理。
    #[test]
    fn status_json_has_the_contract_fields() {
        let root = temp_root("status");
        let text = status_json(&root);
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");

        assert!(value["supported"].is_boolean());
        assert!(value["system"]["enabled"].is_boolean());
        assert!(value["system"]["server"].is_string());
        assert!(value["system"]["bypass"].is_string());
        assert!(value["backup"].is_boolean());

        // 自研引擎已重建：字段仍是同一套契约，但 supported 变成 true，
        // 并多了 active / nodes / skipped（面板可列节点）。
        assert_eq!(value["local"]["supported"], true);
        assert!(value["local"]["running"].is_boolean());
        assert!(value["local"]["port"].is_u64());
        assert!(value["local"]["reason"].is_string());
        assert!(value["local"]["nodes"].is_array());
        assert!(value["local"]["skipped"].is_array());

        assert_eq!(value["kernel"]["available"], false);
        assert!(value["kernel"]["installed"].is_boolean());
        assert!(value["kernel"]["path"].is_string());
        assert_eq!(
            value["kernel"]["searchPaths"].as_array().map(Vec::len),
            Some(3)
        );
        assert!(value["kernel"]["reason"].is_string());

        let _ = fs::remove_dir_all(&root);
    }

    /// 备份进 `config/proxy.json` 能原样读回，且**保留 v1 的其它字段**；
    /// 没有备份时关闭系统代理必须是空操作（返回可读原因，不碰注册表）。
    #[test]
    fn backup_round_trips_and_disable_without_backup_is_a_noop() {
        let root = temp_root("backup");
        assert!(load_backup(&root).is_none());

        let err = disable_system_proxy(&root).expect_err("没有备份时不能改系统设置");
        assert!(err.contains("没有备份"), "原因要告诉用户为什么没动：{err}");
        assert!(!config_path(&root).exists(), "空操作不该创建配置文件");

        // v1 的配置里还有 entries / active / mihomo：本版只增删 system_backup。
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            config_path(&root),
            r#"{"entries":[{"id":"a"}],"active":"a","mihomo":{"enabled":true}}"#,
        )
        .unwrap();

        store_backup(
            &root,
            Some(SystemProxy {
                enabled: true,
                server: "http://old-proxy:8080".into(),
                bypass: "<local>".into(),
            }),
        )
        .unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(config_path(&root)).unwrap()).unwrap();
        assert_eq!(raw["active"], "a", "v1 的其它字段必须保留");
        assert!(raw["entries"].is_array());
        assert_eq!(raw["mihomo"]["enabled"], true);

        let hit = load_backup(&root).expect("backup");
        assert!(hit.enabled);
        assert_eq!(hit.server, "http://old-proxy:8080");
        assert_eq!(hit.bypass, "<local>");

        store_backup(&root, None).unwrap();
        assert!(load_backup(&root).is_none());
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(config_path(&root)).unwrap()).unwrap();
        assert_eq!(raw["active"], "a");

        let _ = fs::remove_dir_all(&root);
    }

    /// 「代理」是 UI 接管型动作（Dart 打开 `ProxyPanel`）。
    #[test]
    fn proxy_action_is_ui_owned() {
        let id = std::ffi::CString::new("proxy").unwrap();
        assert_eq!(crate::lunac_run_action(id.as_ptr()), 2);
    }
}
