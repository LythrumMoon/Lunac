// src/system_proxy.rs
//! **Windows 系统代理：读 / 写 / 还原**（2026-10-01，代理插件的地基）。
//!
//! 系统代理只有一个真相源：注册表
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings` 的三个值
//! `ProxyEnable`(DWORD) / `ProxyServer`(SZ) / `ProxyOverride`(SZ)。本模块**只碰这三个**，
//! 不碰 WinHTTP（`netsh winhttp`）那条**另一套**设置 —— 两套混用会出现
//! 「浏览器走了、curl 没走」这种说不清的现象。
//!
//! **三件事必须一起做，少一件都对应一个具体的坏表现**：
//!
//! ① **写完注册表要发通知**（`InternetSetOptionW` 两次）。只改注册表不通知，WinINet
//!    会继续用缓存里的旧设置，表现为「改了没反应、重启才生效」。两个 option 都要发：
//!    `SETTINGS_CHANGED` 让它重读注册表，`REFRESH` 让**已经建立的**连接也按新设置走 ——
//!    只发前一个是网上最常见的写法，实测是「一部分程序生效、一部分要重启」。
//!
//! ② **原值必须先备份、且只备份一次**。这是「不留下坏状态」的底线：用户开着我们的代理，
//!    程序一崩、整台机器的网络都断，那比没这个功能更糟。存进 `config\proxy.json` 的
//!    `backup` 字段（**不是**另开一个文件：备份与配置要一起写、一起读，分两个文件迟早
//!    出现「配置在、备份丢了」）。
//!
//! ③ **备份只做一次**（第一次启用那一刻的系统原值）。每次启用都覆盖备份的话，
//!    第二次启用的备份内容就是**我们自己上次写进去的值** —— 于是「还原」会还原成一个
//!    假的原值，用户原来的代理设置就此永久丢失。判据只有一个：`backup.is_none()`。
//!
//! ⚠️ 本模块**不做**「退出时自动还原」的决定 —— 那由 `main.rs` 在退出路径上调
//! `restore_on_exit()`。理由：还原是**用户的期望**（他开着代理关掉程序，网络该照旧），
//! 但「什么时候算退出」只有宿主知道（托盘退出 / 窗口关了不算 / 崩溃不算）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
use winreg::RegKey;

/// 系统代理在注册表里的位置（`HKCU` 下，所以**不需要管理员**）。
const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

// ── WinINet 的「通知」──
// 走**裸 FFI** 而不是引 `windows` crate 的 `Win32_Networking_WinInet` feature：
// 只为两个常量调两次函数，不值得拉一个新 feature 进依赖树（同 music.rs 里
// `CreateToolhelp32Snapshot` 那份判据）。
#[link(name = "wininet")]
extern "system" {
    fn InternetSetOptionW(
        h: *mut core::ffi::c_void,
        option: u32,
        buf: *mut core::ffi::c_void,
        len: u32,
    ) -> i32;
}
/// 「设置变了，重读注册表」（`INTERNET_OPTION_SETTINGS_CHANGED`）。
const OPT_SETTINGS_CHANGED: u32 = 39;
/// 「把新设置推给已建立的连接」（`INTERNET_OPTION_REFRESH`）。
const OPT_REFRESH: u32 = 37;

/// 一份系统代理设置。`server` = `http://127.0.0.1:8080` 这类、`bypass` = 分号分隔的例外表。
///
/// ⚠️ 这里的端口只是**文档举例**，不是任何默认值 —— `SystemProxy::default()` 的 `server`
/// 是**空串**（见 `ProxyConfig::default()`：entries 为空）。曾经用 `:7890` 举例，那是别的
/// 软件的默认端口，容易被读成「本插件推荐填这个」（2026-10-02 用户要求去掉）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemProxy {
    pub enabled: bool,
    pub server: String,
    pub bypass: String,
}

/// 读当前的系统代理。**读不到就返回默认值**（键不存在 / 被锁都是「没设代理」这个语义，
/// 不是错误 —— 把「读不出来」报成错误会让「第一次打开这个插件」直接失败）。
pub fn read() -> SystemProxy {
    let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(INTERNET_SETTINGS, KEY_READ)
    else {
        return SystemProxy::default();
    };
    SystemProxy {
        // `ProxyEnable` 存的是 0/1 的 DWORD（不是 bool）
        enabled: key.get_value::<u32, _>("ProxyEnable").unwrap_or(0) != 0,
        server: key.get_value::<String, _>("ProxyServer").unwrap_or_default(),
        bypass: key.get_value::<String, _>("ProxyOverride").unwrap_or_default(),
    }
}

/// 写系统代理并发通知。空值一律**删键**而不是写空串 —— 写空串等于
/// 「代理开着、但服务器是空的」，那会让 WinINet 直接把所有请求发失败。
pub fn write(s: &SystemProxy) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    // `create_subkey`：键不存在时新建（正常情况下它一定在，但全新用户/被清理过时不能崩）
    let (key, _) = hkcu
        .create_subkey(INTERNET_SETTINGS)
        .map_err(|e| format!("打开 Internet Settings 失败：{e}"))?;

    key.set_value("ProxyEnable", &u32::from(s.enabled))
        .map_err(|e| format!("写 ProxyEnable 失败：{e}"))?;
    put_or_delete(&key, "ProxyServer", &s.server)?;
    put_or_delete(&key, "ProxyOverride", &s.bypass)?;

    notify();
    Ok(())
}

fn put_or_delete(key: &RegKey, name: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        // 删不掉（本来就没有）不算失败
        let _ = key.delete_value(name);
        return Ok(());
    }
    key.set_value(name, &value.to_string())
        .map_err(|e| format!("写 {name} 失败：{e}"))
}

/// 告诉 WinINet「设置变了」（见模块头 ①）。**失败不报错**：通知本身不影响我们这边的
/// 状态，而且它没有返回值可判 —— 顶多是「要重启相关程序才生效」，不该因此把整步判失败。
fn notify() {
    unsafe {
        InternetSetOptionW(std::ptr::null_mut(), OPT_SETTINGS_CHANGED, std::ptr::null_mut(), 0);
        InternetSetOptionW(std::ptr::null_mut(), OPT_REFRESH, std::ptr::null_mut(), 0);
    }
}

// ── 配置（用户维护的代理列表）──────────────────────────────────────

/// 用户加的一条代理。`server` 会**原样**进 `ProxyServer`，所以形态就是 WinINet 认的那几种：
/// `http://host:port` / `https://host:port` / `socks5://host:port`（也可以不带 scheme，
/// 那就按 `host:port` 当 HTTP 代理 —— WinINet 的老写法）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProxyEntry {
    /// 稳定 id（前端生成，用于「当前启用的是哪条」与列表增删）。
    pub id: String,
    /// 显示名（用户自己起，比如「家里的梯子」）。
    #[serde(default)]
    pub label: String,
    pub server: String,
    /// 不走代理的地址（分号分隔）。留空 = 用内置默认（绕过本机与局域网）。
    #[serde(default)]
    pub bypass: String,
}

/// `config\proxy.json` 的形状。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub entries: Vec<ProxyEntry>,
    /// 当前启用的那条的 `id`。空串 = 没启用（**不是**「没配」—— 列表可能有一堆，
    /// 只是没开）。
    #[serde(default)]
    pub active: String,
    /// 见模块头 ②③：**只在第一次启用时写一次**。
    #[serde(default)]
    pub backup: Option<SystemProxy>,
    /// **一键联动**（2026-10-02，用户口径）：启用代理时把它同步给**本机播放的 librespot**
    /// （`music.json` 的 `librespot_proxy`），关闭时代它回直连。
    ///
    /// 为什么这个开关存在宿主而不是插件的 localStorage 里：它是一条**跨模块的配置**
    /// （代理 ↔ librespot），与 `entries`/`active` 是同一份「代理怎么用」的真相。
    /// 分成两处存，就会出现「换台机器 / 清缓存后，代理列表还在、联动却悄悄没了」。
    ///
    /// ⚠️ 语义边界（界面文案要如实写）：开着它时，**关闭代理会把 librespot 也掰回直连** ——
    /// 哪怕那个值是用户在音乐插件里手填的。那是这个开关的字面意思，不是 bug。
    #[serde(default)]
    pub link_librespot: bool,
    /// **mihomo 内核托管**（2026-10-03）：订阅 / 端口 / 模式 / 内核安装状态。
    ///
    /// 为什么放在这一份里而不是另开 `mihomo.json`：它俩是同一件事的两半 ——
    /// 「用哪个代理」与「系统代理指向哪」。分两个文件后，「内核在跑、但系统代理指向
    /// 的端口是旧的」这种不一致没有任何单一判据能发现（同模块头 ② 那条备份与配置
    /// 必须同文件的理由）。
    #[serde(default)]
    pub mihomo: crate::mihomo::MihomoConfig,
}

/// 内置的默认绕过表。**必须包含 `<local>`**：不加它的话，访问 `127.0.0.1`
/// 也会被送去代理 —— 而 Lunac 自己就靠 127.0.0.1 上的几个服务活着
/// （`proxy_server.rs` 的 8788、agent 的桥 8789），那会**自己把自己绕死**。
pub const DEFAULT_BYPASS: &str = "<local>;localhost;127.*;10.*;172.16.*;172.17.*;172.18.*;172.19.*;172.20.*;172.21.*;172.22.*;172.23.*;172.24.*;172.25.*;172.26.*;172.27.*;172.28.*;172.29.*;172.30.*;172.31.*;192.168.*";

fn config_path() -> PathBuf {
    crate::storage::lunac_root_dir().join("config").join("proxy.json")
}

pub fn load_config() -> ProxyConfig {
    let p = config_path();
    let Ok(text) = std::fs::read_to_string(&p) else {
        return ProxyConfig::default();
    };
    // 解析失败**保留文件、返回默认值**并如实记一行：直接覆盖会把用户手写的列表抹掉
    match serde_json::from_str::<ProxyConfig>(&text) {
        Ok(c) => c,
        Err(e) => {
            crate::log::warn(format!("config\\proxy.json 解析失败（按默认值继续）：{e}"));
            ProxyConfig::default()
        }
    }
}

pub fn save_config(cfg: &ProxyConfig) -> Result<(), String> {
    let p = config_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建 config 目录失败：{e}"))?;
    }
    let text = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化失败：{e}"))?;
    std::fs::write(&p, text).map_err(|e| format!("写 config\\proxy.json 失败：{e}"))
}

/// 一条代理的 `server` 能不能真的写进系统代理。
///
/// 三条判据都来自 WinINet 的行为，不是「看着像不像」：
/// ① 非空；② **不含空白**（`ProxyServer` 里出现空格/换行会让整串被当成一个非法条目，
///    WinINet 直接不认，表现为「开了代理但完全没走」）；③ 必须含 `:`（要端口）。
pub fn validate_server(server: &str) -> Result<(), String> {
    let s = server.trim();
    if s.is_empty() {
        return Err("代理地址不能为空".into());
    }
    if s.chars().any(|c| c.is_whitespace()) {
        return Err("代理地址里不能有空格或换行".into());
    }
    if !s.contains(':') {
        return Err("代理地址要带端口，形如 http://127.0.0.1:8080".into());
    }
    Ok(())
}

/// 按 `id` 取一条。找不到返回 `None`（调用方据此报「这条不存在了」而不是静默）。
pub fn find_entry<'a>(cfg: &'a ProxyConfig, id: &str) -> Option<&'a ProxyEntry> {
    cfg.entries.iter().find(|e| e.id == id)
}

// ── 复用出口：把「写系统代理」这件事给别的模块用（2026-10-03，mihomo 内核托管）──
//
// **备份 / 还原的纪律只实现这一处**。mihomo 起来之后也要把系统代理指向
// `127.0.0.1:<mixed-port>`，如果那边自己再写一遍注册表逻辑，就会出现两份「只备份一次」
// 的实现 —— 而这条纪律错一次就是「用户原来的代理设置永久丢失」（见模块头 ③）。

/// 启用系统代理（**备份只做一次**，判据 `backup.is_none()`）。返回回读到的真实现状。
///
/// **不碰 `active`** —— 那个字段的语义是「用户列表里选中的是哪条」，由调用方决定。
pub fn enable_system_proxy(
    cfg: &mut ProxyConfig,
    server: &str,
    bypass: &str,
) -> Result<SystemProxy, String> {
    validate_server(server)?;
    if cfg.backup.is_none() {
        let original = read();
        crate::log::info(format!(
            "代理：备份系统原值 enabled={} server_len={}",
            original.enabled,
            original.server.len()
        ));
        cfg.backup = Some(original);
    }
    let bypass = if bypass.trim().is_empty() {
        DEFAULT_BYPASS.to_string()
    } else {
        bypass.trim().to_string()
    };
    let want = SystemProxy {
        enabled: true,
        server: server.trim().to_string(),
        bypass,
    };
    write(&want)?;
    // 回读一次真值再交给前端：注册表写成功但被组策略压回去是可能的，
    // 界面要显示的是**实际生效的**那条（同 Spotify 那条「状态一律回读」）。
    Ok(read())
}

/// 还原系统代理。**没有备份时什么都不做**（那种情况下「关掉」等于把用户自己配的
/// 代理抹掉，是纯粹的数据损失）。
pub fn restore_system_proxy(cfg: &mut ProxyConfig) -> Result<SystemProxy, String> {
    if let Some(original) = cfg.backup.take() {
        write(&original)?;
        crate::log::info("代理：已关闭并还原系统原值");
    } else {
        crate::log::info("代理：已关闭（没有备份，系统设置未改动）");
    }
    Ok(read())
}

// ── tauri 命令 ──────────────────────────────────────────────────────

/// 当前系统代理的**真实现状**（不从我们的配置推断 —— 用户可能刚在别处改过）。
#[tauri::command]
pub async fn proxy_system_get() -> Result<SystemProxy, String> {
    Ok(read())
}

#[tauri::command]
pub async fn proxy_config_get() -> Result<ProxyConfig, String> {
    // `run_blocking` 的闭包签名是 `-> Result<T, String>`，而读配置是「读不出来就给默认值」
    // （不是错误）⇒ 这里补一个 `Ok`，不改 `load_config` 的语义。
    crate::commands::run_blocking(|| Ok(load_config())).await
}

#[tauri::command]
pub async fn proxy_config_set(config: ProxyConfig) -> Result<ProxyConfig, String> {
    crate::commands::run_blocking(move || {
        let mut incoming = config;
        let old = load_config();
        // **两个字段由宿主独占，前端回传的这份里根本没有** —— 整份落盘会把它们抹掉：
        //   · `backup`：还原所需的系统原值。界面不感知它（见文件头 ②③），所以列表里
        //     加一条 / 删一条都会把它清空 ⇒ 「用户开着我们的代理时改了下列表，之后关不掉、
        //     原来的系统代理也回不来」。这是**真实存在过的**数据损失路径。
        //   · `mihomo`：有专属命令（`mihomo_config_set`）管理，这里整体继承。
        incoming.backup = old.backup;
        incoming.mihomo = old.mihomo;
        save_config(&incoming)?;
        Ok(incoming)
    })
    .await
}

/// 启用某一条代理：**先备份原值（只一次）→ 再写系统代理**。
///
/// 顺序不能反：先写再备份的话，备份到的就是我们自己刚写进去的值（见模块头 ③）。
#[tauri::command]
pub async fn proxy_enable(id: String) -> Result<SystemProxy, String> {
    crate::commands::run_blocking(move || {
        let mut cfg = load_config();
        let entry = find_entry(&cfg, &id)
            .ok_or_else(|| format!("代理 {id} 不在列表里"))?
            .clone();

        let now = enable_system_proxy(&mut cfg, &entry.server, &entry.bypass)?;
        cfg.active = id;
        save_config(&cfg)?;
        crate::log::info(format!(
            "代理：已启用 {} -> enabled={}",
            entry.server.trim(),
            now.enabled
        ));
        Ok(now)
    })
    .await
}

/// 关闭代理并**还原系统原值**。
///
/// ⚠️ 没有备份时（从没启用过）**什么都不做** —— 那种情况下「关掉」等于把用户
/// 自己配的代理抹掉，是纯粹的数据损失。
#[tauri::command]
pub async fn proxy_disable() -> Result<SystemProxy, String> {
    crate::commands::run_blocking(move || {
        let mut cfg = load_config();
        cfg.active.clear();
        let now = restore_system_proxy(&mut cfg)?;
        save_config(&cfg)?;
        Ok(now)
    })
    .await
}

/// 退出路径上调用：**只在我们真的改动过系统代理时**才还原。
///
/// 判据是 `backup.is_some()`，不是 `active` 非空 —— 用户可能启用了之后又在别处改过、
/// 或我们崩过一次，此时以「有没有备份」为准才是可还原的那个语义。
pub fn restore_on_exit() {
    let mut cfg = load_config();
    if let Some(original) = cfg.backup.take() {
        match write(&original) {
            Ok(()) => {
                cfg.active.clear();
                let _ = save_config(&cfg);
                crate::log::info("退出：已还原系统代理");
            }
            Err(e) => crate::log::error(format!("退出：还原系统代理失败：{e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_must_be_non_empty_with_a_port_and_no_whitespace() {
        assert!(validate_server("http://127.0.0.1:8080").is_ok());
        assert!(validate_server("socks5://10.0.0.1:1080").is_ok());
        // 老写法：不带 scheme 也认
        assert!(validate_server("127.0.0.1:8080").is_ok());

        assert!(validate_server("").is_err());
        assert!(validate_server("   ").is_err());
        // 有空格 = WinINet 会把整串当非法条目，表现为「开了但没走」⇒ 必须在这里拦住
        assert!(validate_server("http://127.0.0.1:8080 hello").is_err());
        assert!(validate_server("127.0.0.1").is_err(), "没端口");
    }

    #[test]
    fn default_bypass_never_sends_loopback_to_the_proxy() {
        // Lunac 自己活在 127.0.0.1 上（8788 / 8789），这条表错了就是自己把自己绕死
        assert!(DEFAULT_BYPASS.contains("<local>"));
        assert!(DEFAULT_BYPASS.contains("127.*"));
        assert!(DEFAULT_BYPASS.contains("localhost"));
    }

    #[test]
    fn find_entry_returns_none_instead_of_a_blank_entry() {
        let cfg = ProxyConfig {
            entries: vec![ProxyEntry { id: "a".into(), server: "1.2.3.4:80".into(), ..Default::default() }],
            ..Default::default()
        };
        assert!(find_entry(&cfg, "a").is_some());
        assert!(find_entry(&cfg, "b").is_none());
    }
}
