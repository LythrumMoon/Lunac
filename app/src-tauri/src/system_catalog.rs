// system_catalog.rs
// 「详细搜索」里的系统设置页索引 + 系统动作白名单（见 docs/ai-spec.md §2.1.2）。
//
// 两条硬约束：
//   ① **只放静态表，不做任何 OS 查询** —— 设置页没有可枚举的公开 API，
//      名称是 Windows 自己的资源；这里给出 zh / en 两套名字，前端按系统语言取一套
//      （UI 外壳文案走 i18n 五语言，设置项名不做五种语言的官方译名 —— 拿不到）。
//   ② **动作必须按 id 白名单执行**，绝不接受前端传来的命令行字符串；
//      `open_setting` 只接受 `ms-settings:` 前缀。前端能传什么，这里就只认什么。

use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct CatalogItem {
    /// 稳定 id（前端 i18n / 日志用）
    pub id: String,
    /// "setting" = Windows 设置页；"action" = 系统动作
    pub kind: String,
    pub icon: String,
    pub title_zh: String,
    pub title_en: String,
    /// setting: `ms-settings:xxx`；action: 动作 id（= id）
    pub target: String,
    /// 危险动作（关机 / 重启）：前端必须二次确认
    pub danger: bool,
    /// 搜索关键词（zh + en + 拼音首字母，命中任一即可）
    pub keywords: Vec<String>,
}

fn setting(id: &str, icon: &str, zh: &str, en: &str, uri: &str, kw: &[&str]) -> CatalogItem {
    CatalogItem {
        id: id.into(),
        kind: "setting".into(),
        icon: icon.into(),
        title_zh: zh.into(),
        title_en: en.into(),
        target: uri.into(),
        danger: false,
        keywords: kw.iter().map(|s| s.to_string()).collect(),
    }
}

fn action(id: &str, icon: &str, zh: &str, en: &str, kw: &[&str], danger: bool) -> CatalogItem {
    CatalogItem {
        id: id.into(),
        kind: "action".into(),
        icon: icon.into(),
        title_zh: zh.into(),
        title_en: en.into(),
        target: id.into(),
        danger,
        keywords: kw.iter().map(|s| s.to_string()).collect(),
    }
}

/// 全部条目（设置页 + 动作）。数据量很小（几十条），前端一次拉走、本地过滤。
pub fn all() -> Vec<CatalogItem> {
    vec![
        // ── 常用设置页 ──────────────────────────────────────────────
        setting("set.display", "🖥", "显示", "Display", "ms-settings:display",
            &["display", "monitor", "xianshi", "分辨率", "亮度"]),
        setting("set.nightlight", "🌙", "夜间模式", "Night light", "ms-settings:nightlight",
            &["night light", "nightlight", "yejian", "护眼"]),
        setting("set.sound", "🔊", "声音", "Sound", "ms-settings:sound",
            &["sound", "audio", "shengyin", "音量", "输出设备"]),
        setting("set.bluetooth", "📶", "蓝牙和其他设备", "Bluetooth & devices", "ms-settings:bluetooth",
            &["bluetooth", "lanya", "蓝牙", "设备"]),
        setting("set.wifi", "📡", "WLAN", "Wi-Fi", "ms-settings:network-wifi",
            &["wifi", "wlan", "wireless", "无线", "wuxian"]),
        setting("set.ethernet", "🔌", "以太网", "Ethernet", "ms-settings:network-ethernet",
            &["ethernet", "lan", "yitaiwang", "有线"]),
        setting("set.network", "🌐", "网络状态", "Network status", "ms-settings:network",
            &["network", "wangluo", "网络", "状态"]),
        setting("set.vpn", "🛡", "VPN", "VPN", "ms-settings:network-vpn",
            &["vpn", "代理"]),
        setting("set.proxy", "🧭", "代理", "Proxy", "ms-settings:network-proxy",
            &["proxy", "daili", "代理"]),
        setting("set.power", "🔋", "电源和睡眠", "Power & sleep", "ms-settings:powersleep",
            &["power", "sleep", "battery", "dianyuan", "睡眠", "电池"]),
        setting("set.apps", "📦", "已安装的应用", "Installed apps", "ms-settings:appsfeatures",
            &["apps", "uninstall", "installed", "yianzhuang", "卸载", "应用"]),
        setting("set.defaultapps", "🎯", "默认应用", "Default apps", "ms-settings:defaultapps",
            &["default apps", "moren", "默认", "关联"]),
        setting("set.startup", "🚀", "启动应用", "Startup apps", "ms-settings:startupapps",
            &["startup", "qidong", "开机启动", "自启"]),
        setting("set.printers", "🖨", "打印机和扫描仪", "Printers & scanners", "ms-settings:printers",
            &["printer", "scanner", "dayinji", "打印机", "扫描仪"]),
        setting("set.mouse", "🖱", "鼠标", "Mouse", "ms-settings:mousetouchpad",
            &["mouse", "touchpad", "shubiao", "鼠标", "触控板"]),
        setting("set.keyboard", "⌨", "键盘与输入", "Typing", "ms-settings:typing",
            &["keyboard", "typing", "ime", "jianpan", "键盘", "输入法"]),
        setting("set.personalization", "🎨", "个性化", "Personalization", "ms-settings:personalization",
            &["personalization", "geXinghua", "个性化", "主题"]),
        setting("set.background", "🖼", "背景", "Background", "ms-settings:personalization-background",
            &["background", "wallpaper", "beijing", "壁纸", "背景"]),
        setting("set.colors", "🌈", "颜色", "Colors", "ms-settings:personalization-colors",
            &["colors", "dark mode", "yanse", "颜色", "深色"]),
        setting("set.themes", "🧩", "主题", "Themes", "ms-settings:themes",
            &["themes", "zhuti", "主题"]),
        setting("set.taskbar", "📊", "任务栏", "Taskbar", "ms-settings:taskbar",
            &["taskbar", "renwulan", "任务栏"]),
        setting("set.lockscreen", "🔒", "锁屏界面", "Lock screen", "ms-settings:lock-screen",
            &["lock screen", "suoping", "锁屏"]),
        setting("set.datetime", "🕒", "日期和时间", "Date & time", "ms-settings:dateandtime",
            &["date", "time", "timezone", "shijian", "日期", "时间", "时区"]),
        setting("set.language", "🌍", "语言和区域", "Language & region", "ms-settings:regionlanguage",
            &["language", "region", "yuyan", "语言", "区域"]),
        setting("set.notifications", "🔔", "通知", "Notifications", "ms-settings:notifications",
            &["notifications", "tongzhi", "通知"]),
        setting("set.focus", "🎧", "专注助手", "Focus assist", "ms-settings:focusassist",
            &["focus assist", "zhuanzhu", "专注", "免打扰"]),
        setting("set.multitasking", "🗂", "多任务处理", "Multitasking", "ms-settings:multitasking",
            &["multitasking", "duorenwu", "多任务", "虚拟桌面"]),
        setting("set.clipboard", "📋", "剪贴板设置", "Clipboard settings", "ms-settings:clipboard",
            &["clipboard", "jiantieban", "剪贴板", "历史记录"]),
        setting("set.storage", "💾", "存储", "Storage", "ms-settings:storagesense",
            &["storage", "disk", "cunchu", "存储", "磁盘空间"]),
        setting("set.update", "⬆", "Windows 更新", "Windows Update", "ms-settings:windowsupdate",
            &["windows update", "gengxin", "更新", "升级"]),
        setting("set.recovery", "♻", "恢复", "Recovery", "ms-settings:recovery",
            &["recovery", "reset", "huifu", "恢复", "重置"]),
        setting("set.easeofaccess", "♿", "辅助功能", "Accessibility", "ms-settings:easeofaccess",
            &["accessibility", "ease of access", "fuzhu", "辅助功能"]),
        setting("set.privacy", "🛡", "隐私和安全性", "Privacy & security", "ms-settings:privacy",
            &["privacy", "security", "yinsi", "隐私", "安全"]),
        setting("set.camera", "📷", "摄像头隐私", "Camera privacy", "ms-settings:privacy-webcam",
            &["camera", "webcam", "shexiangtou", "摄像头"]),
        setting("set.microphone", "🎙", "麦克风隐私", "Microphone privacy", "ms-settings:privacy-microphone",
            &["microphone", "mic", "maikefeng", "麦克风"]),
        setting("set.signin", "🔑", "登录选项", "Sign-in options", "ms-settings:signinoptions",
            &["sign in", "password", "pin", "denglu", "登录", "密码"]),
        setting("set.accounts", "👤", "账户信息", "Your info", "ms-settings:yourinfo",
            &["account", "user", "zhanghu", "账户", "用户"]),
        setting("set.developers", "🧪", "开发者选项", "For developers", "ms-settings:developers",
            &["developer", "kaifazhe", "开发者", "开发人员"]),
        setting("set.about", "ℹ", "关于", "About", "ms-settings:about",
            &["about", "system info", "guanyu", "关于", "系统信息", "版本"]),
        setting("set.backup", "☁", "备份", "Backup", "ms-settings:backup",
            &["backup", "beifen", "备份"]),
        setting("set.gaming", "🎮", "游戏", "Gaming", "ms-settings:gaming-gamebar",
            &["game", "gamebar", "youxi", "游戏", "游戏栏"]),

        // ── 系统动作（id 白名单，见 run_action）────────────────────
        action("act.lock", "🔐", "锁定计算机", "Lock", &["lock", "suoding", "锁定", "锁屏"], false),
        action("act.sleep", "😴", "睡眠", "Sleep", &["sleep", "shuimian", "睡眠"], false),
        action("act.taskmgr", "📈", "任务管理器", "Task Manager",
            &["task manager", "taskmgr", "renwu guanliqi", "任务管理器", "进程"], false),
        action("act.control", "🎛", "控制面板", "Control Panel",
            &["control panel", "kongzhi mianban", "控制面板"], false),
        action("act.devmgmt", "🧰", "设备管理器", "Device Manager",
            &["device manager", "shebei guanliqi", "设备管理器", "驱动"], false),
        action("act.resmon", "📉", "资源监视器", "Resource Monitor",
            &["resource monitor", "ziyuan jianshiqi", "资源监视器", "性能"], false),
        action("act.sysinfo", "🗃", "系统信息", "System Information",
            &["system information", "msinfo32", "xitong xinxi", "系统信息", "硬件"], false),
        action("act.recyclebin", "🗑", "打开回收站", "Open Recycle Bin",
            &["recycle bin", "huishouzhan", "回收站"], false),
        action("act.shutdown", "⏻", "关机", "Shut down",
            &["shutdown", "power off", "guanji", "关机"], true),
        action("act.restart", "🔄", "重启", "Restart",
            &["restart", "reboot", "chongqi", "重启"], true),
    ]
}

/// 打开 Windows 设置页。**只接受 `ms-settings:` 前缀** —— 前端能传的东西必须被
/// 约束成一个设置页，不能变成「任意 URI / 任意程序」的执行入口。
pub fn open_setting(target: &str) -> Result<(), String> {
    if !is_setting_uri(target) {
        return Err(format!("not a settings page: {}", target.trim()));
    }
    crate::app_indexer::launch_app(target.trim())
}

fn is_setting_uri(target: &str) -> bool {
    target.trim().to_ascii_lowercase().starts_with("ms-settings:")
}

/// 执行系统动作。**只认白名单 id**（前端传的是 id，不是命令）。
pub fn run_action(id: &str) -> Result<(), String> {
    match id {
        "act.lock" => spawn(&["rundll32.exe", "user32.dll,LockWorkStation"]),
        "act.sleep" => spawn(&["rundll32.exe", "powrprof.dll,SetSuspendState", "0,1,0"]),
        "act.shutdown" => spawn(&["shutdown.exe", "/s", "/t", "0"]),
        "act.restart" => spawn(&["shutdown.exe", "/r", "/t", "0"]),
        "act.taskmgr" => crate::app_indexer::launch_app("taskmgr.exe"),
        "act.control" => crate::app_indexer::launch_app("control.exe"),
        "act.devmgmt" => crate::app_indexer::launch_app("devmgmt.msc"),
        "act.resmon" => crate::app_indexer::launch_app("resmon.exe"),
        "act.sysinfo" => crate::app_indexer::launch_app("msinfo32.exe"),
        "act.recyclebin" => crate::app_indexer::launch_app("shell:RecycleBinFolder"),
        other => Err(format!("unknown action: {other}")),
    }
}

/// 起个短命进程并把参数直接传进去（不走 shell，避免命令拼接面）。
#[cfg(target_os = "windows")]
fn spawn(argv: &[&str]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let (exe, args) = argv.split_first().ok_or("empty argv")?;
    std::process::Command::new(exe)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{exe} 启动失败: {e}"))
}

#[cfg(not(target_os = "windows"))]
fn spawn(argv: &[&str]) -> Result<(), String> {
    let (exe, args) = argv.split_first().ok_or("empty argv")?;
    std::process::Command::new(exe)
        .args(args)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{exe} 启动失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_targets_are_sane() {
        let items = all();
        assert!(items.len() > 40, "目录至少要有四十来项");
        let mut seen = std::collections::HashSet::new();
        for it in &items {
            assert!(seen.insert(it.id.clone()), "重复 id: {}", it.id);
            if it.kind == "setting" {
                assert!(it.target.starts_with("ms-settings:"), "{} 必须是设置页", it.id);
                assert!(!it.danger, "设置页不该标危险: {}", it.id);
            } else {
                assert_eq!(it.kind, "action");
                assert_eq!(it.target, it.id, "动作的 target 就是 id");
                assert!(!it.title_en.is_empty() && !it.title_zh.is_empty());
            }
            assert!(!it.keywords.is_empty(), "{} 必须可搜", it.id);
        }
    }

    /// 危险动作只有关机 / 重启（前端据此弹二次确认）
    #[test]
    fn only_shutdown_and_restart_are_dangerous() {
        let dangerous: Vec<String> = all()
            .into_iter()
            .filter(|i| i.danger)
            .map(|i| i.id)
            .collect();
        assert_eq!(dangerous, vec!["act.shutdown", "act.restart"]);
    }

    /// 只接受设置页 URI；动作只认白名单 id。
    /// 这里**不真去打开**任何东西（测试不能有副作用），只验校验层。
    #[test]
    fn arbitrary_targets_are_rejected() {
        assert!(is_setting_uri("ms-settings:display"));
        assert!(is_setting_uri("  MS-SETTINGS:about  "), "大小写/空白要容忍");
        for bad in [
            "http://example.com",
            "https://example.com",
            "C:\\Windows\\System32\\cmd.exe",
            "r m -rf /",
            "",
        ] {
            assert!(!is_setting_uri(bad), "{bad} 不该被当成设置页");
        }
        for bad in ["rm -rf /", "act.shutdown; rm -rf /", "../../x", ""] {
            assert!(run_action(bad).is_err(), "{bad} 不该被执行");
        }
        assert!(run_action("act.unknown").is_err());
    }
}
