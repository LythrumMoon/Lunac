// 详细搜索的「设置 / 系统动作」目录（对齐 v1 `app/src-tauri/src/system_catalog.rs`）。
//
// 两条硬约束（与 v1 一致）：
//   ① **只放静态表，不做任何 OS 查询** —— 设置页没有可枚举的公开 API，名称是 Windows
//      自己的资源；这里给出 zh / en 两套名字，前端按系统语言取一套。
//   ② **动作必须按 id 白名单执行**，绝不接受前端传来的命令行字符串；
//      `open_setting` 只接受 `ms-settings:` 前缀。
//
// 与 v1 的差别只有一处：v1 把每条都写成一次 `setting(...)` 调用，这里收成两张元组表
// （数据完全相同），新增条目只需要加一行。

/// 一条目录：设置页（kind = "setting"）或系统动作（kind = "action"）。
#[derive(Clone)]
pub struct CatalogItem {
    pub id: &'static str,
    pub kind: &'static str,
    pub icon: &'static str,
    pub title_zh: &'static str,
    pub title_en: &'static str,
    /// setting: `ms-settings:xxx`；action: 动作 id（= id）
    pub target: &'static str,
    /// 危险动作（关机 / 重启）：前端必须二次确认
    pub danger: bool,
    /// 是否有「以管理员身份运行」形态（由 `action_spec()` 推导，不在表里手写）
    pub elevatable: bool,
    pub keywords: &'static [&'static str],
}

/// 设置页：id, icon, zh, en, uri, keywords。
const SETTINGS: &[(
    &str,
    &str,
    &str,
    &str,
    &str,
    &[&str],
)] = &[
    (
        "set.display",
        "🖥",
        "显示",
        "Display",
        "ms-settings:display",
        &["display", "monitor", "xianshi", "分辨率", "亮度"],
    ),
    (
        "set.nightlight",
        "🌙",
        "夜间模式",
        "Night light",
        "ms-settings:nightlight",
        &["night light", "nightlight", "yejian", "护眼"],
    ),
    (
        "set.sound",
        "🔊",
        "声音",
        "Sound",
        "ms-settings:sound",
        &["sound", "audio", "shengyin", "音量", "输出设备"],
    ),
    (
        "set.bluetooth",
        "📶",
        "蓝牙和其他设备",
        "Bluetooth & devices",
        "ms-settings:bluetooth",
        &["bluetooth", "lanya", "蓝牙", "设备"],
    ),
    (
        "set.wifi",
        "📡",
        "WLAN",
        "Wi-Fi",
        "ms-settings:network-wifi",
        &["wifi", "wlan", "wireless", "无线", "wuxian"],
    ),
    (
        "set.ethernet",
        "🔌",
        "以太网",
        "Ethernet",
        "ms-settings:network-ethernet",
        &["ethernet", "lan", "yitaiwang", "有线"],
    ),
    (
        "set.network",
        "🌐",
        "网络状态",
        "Network status",
        "ms-settings:network",
        &["network", "wangluo", "网络", "状态"],
    ),
    (
        "set.vpn",
        "🛡",
        "VPN",
        "VPN",
        "ms-settings:network-vpn",
        &["vpn", "代理"],
    ),
    (
        "set.proxy",
        "🧭",
        "代理",
        "Proxy",
        "ms-settings:network-proxy",
        &["proxy", "daili", "代理"],
    ),
    (
        "set.power",
        "🔋",
        "电源和睡眠",
        "Power & sleep",
        "ms-settings:powersleep",
        &["power", "sleep", "battery", "dianyuan", "睡眠", "电池"],
    ),
    (
        "set.apps",
        "📦",
        "已安装的应用",
        "Installed apps",
        "ms-settings:appsfeatures",
        &["apps", "uninstall", "installed", "yianzhuang", "卸载", "应用"],
    ),
    (
        "set.defaultapps",
        "🎯",
        "默认应用",
        "Default apps",
        "ms-settings:defaultapps",
        &["default apps", "moren", "默认", "关联"],
    ),
    (
        "set.startup",
        "🚀",
        "启动应用",
        "Startup apps",
        "ms-settings:startupapps",
        &["startup", "qidong", "开机启动", "自启"],
    ),
    (
        "set.printers",
        "🖨",
        "打印机和扫描仪",
        "Printers & scanners",
        "ms-settings:printers",
        &["printer", "scanner", "dayinji", "打印机", "扫描仪"],
    ),
    (
        "set.mouse",
        "🖱",
        "鼠标",
        "Mouse",
        "ms-settings:mousetouchpad",
        &["mouse", "touchpad", "shubiao", "鼠标", "触控板"],
    ),
    (
        "set.keyboard",
        "⌨",
        "键盘与输入",
        "Typing",
        "ms-settings:typing",
        &["keyboard", "typing", "ime", "jianpan", "键盘", "输入法"],
    ),
    (
        "set.personalization",
        "🎨",
        "个性化",
        "Personalization",
        "ms-settings:personalization",
        &["personalization", "gexinghua", "个性化", "主题"],
    ),
    (
        "set.background",
        "🖼",
        "背景",
        "Background",
        "ms-settings:personalization-background",
        &["background", "wallpaper", "beijing", "壁纸", "背景"],
    ),
    (
        "set.colors",
        "🌈",
        "颜色",
        "Colors",
        "ms-settings:personalization-colors",
        &["colors", "dark mode", "yanse", "颜色", "深色"],
    ),
    (
        "set.themes",
        "🧩",
        "主题",
        "Themes",
        "ms-settings:themes",
        &["themes", "zhuti", "主题"],
    ),
    (
        "set.taskbar",
        "📊",
        "任务栏",
        "Taskbar",
        "ms-settings:taskbar",
        &["taskbar", "renwulan", "任务栏"],
    ),
    (
        "set.lockscreen",
        "🔒",
        "锁屏界面",
        "Lock screen",
        "ms-settings:lock-screen",
        &["lock screen", "suoping", "锁屏"],
    ),
    (
        "set.datetime",
        "🕒",
        "日期和时间",
        "Date & time",
        "ms-settings:dateandtime",
        &[
            "date",
            "time",
            "timezone",
            "shijian",
            "日期",
            "时间",
            "时区",
        ],
    ),
    (
        "set.language",
        "🌍",
        "语言和区域",
        "Language & region",
        "ms-settings:regionlanguage",
        &["language", "region", "yuyan", "语言", "区域"],
    ),
    (
        "set.notifications",
        "🔔",
        "通知",
        "Notifications",
        "ms-settings:notifications",
        &["notifications", "tongzhi", "通知"],
    ),
    (
        "set.focus",
        "🎧",
        "专注助手",
        "Focus assist",
        "ms-settings:focusassist",
        &["focus assist", "zhuanzhu", "专注", "免打扰"],
    ),
    (
        "set.multitasking",
        "🗂",
        "多任务处理",
        "Multitasking",
        "ms-settings:multitasking",
        &["multitasking", "duorenwu", "多任务", "虚拟桌面"],
    ),
    (
        "set.clipboard",
        "📋",
        "剪贴板设置",
        "Clipboard settings",
        "ms-settings:clipboard",
        &["clipboard", "jiantieban", "剪贴板", "历史记录"],
    ),
    (
        "set.storage",
        "💾",
        "存储",
        "Storage",
        "ms-settings:storagesense",
        &["storage", "disk", "cunchu", "存储", "磁盘空间"],
    ),
    (
        "set.update",
        "⬆",
        "Windows 更新",
        "Windows Update",
        "ms-settings:windowsupdate",
        &["windows update", "gengxin", "更新", "升级"],
    ),
    (
        "set.recovery",
        "♻",
        "恢复",
        "Recovery",
        "ms-settings:recovery",
        &["recovery", "reset", "huifu", "恢复", "重置"],
    ),
    (
        "set.easeofaccess",
        "♿",
        "辅助功能",
        "Accessibility",
        "ms-settings:easeofaccess",
        &["accessibility", "ease of access", "fuzhu", "辅助功能"],
    ),
    (
        "set.privacy",
        "🛡",
        "隐私和安全性",
        "Privacy & security",
        "ms-settings:privacy",
        &["privacy", "security", "yinsi", "隐私", "安全"],
    ),
    (
        "set.camera",
        "📷",
        "摄像头隐私",
        "Camera privacy",
        "ms-settings:privacy-webcam",
        &["camera", "webcam", "shexiangtou", "摄像头"],
    ),
    (
        "set.microphone",
        "🎙",
        "麦克风隐私",
        "Microphone privacy",
        "ms-settings:privacy-microphone",
        &["microphone", "mic", "maikefeng", "麦克风"],
    ),
    (
        "set.signin",
        "🔑",
        "登录选项",
        "Sign-in options",
        "ms-settings:signinoptions",
        &["sign in", "password", "pin", "denglu", "登录", "密码"],
    ),
    (
        "set.accounts",
        "👤",
        "账户信息",
        "Your info",
        "ms-settings:yourinfo",
        &["account", "user", "zhanghu", "账户", "用户"],
    ),
    (
        "set.developers",
        "🧪",
        "开发者选项",
        "For developers",
        "ms-settings:developers",
        &["developer", "kaifazhe", "开发者", "开发人员"],
    ),
    (
        "set.about",
        "ℹ",
        "关于",
        "About",
        "ms-settings:about",
        &["about", "system info", "guanyu", "关于", "系统信息", "版本"],
    ),
    (
        "set.backup",
        "☁",
        "备份",
        "Backup",
        "ms-settings:backup",
        &["backup", "beifen", "备份"],
    ),
    (
        "set.gaming",
        "🎮",
        "游戏",
        "Gaming",
        "ms-settings:gaming-gamebar",
        &["game", "gamebar", "youxi", "游戏", "游戏栏"],
    ),
];

/// 系统动作：id, icon, zh, en, keywords, danger。
const ACTIONS: &[(&str, &str, &str, &str, &[&str], bool)] = &[
    (
        "act.lock",
        "🔐",
        "锁定计算机",
        "Lock",
        &["lock", "suoding", "锁定", "锁屏"],
        false,
    ),
    (
        "act.sleep",
        "😴",
        "睡眠",
        "Sleep",
        &["sleep", "shuimian", "睡眠"],
        false,
    ),
    (
        "act.taskmgr",
        "📈",
        "任务管理器",
        "Task Manager",
        &[
            "task manager",
            "taskmgr",
            "renwu guanliqi",
            "任务管理器",
            "进程",
        ],
        false,
    ),
    (
        "act.control",
        "🎛",
        "控制面板",
        "Control Panel",
        &["control panel", "kongzhi mianban", "控制面板"],
        false,
    ),
    (
        "act.devmgmt",
        "🧰",
        "设备管理器",
        "Device Manager",
        &["device manager", "shebei guanliqi", "设备管理器", "驱动"],
        false,
    ),
    (
        "act.resmon",
        "📉",
        "资源监视器",
        "Resource Monitor",
        &["resource monitor", "ziyuan jianshiqi", "资源监视器", "性能"],
        false,
    ),
    (
        "act.sysinfo",
        "🗃",
        "系统信息",
        "System Information",
        &[
            "system information",
            "msinfo32",
            "xitong xinxi",
            "系统信息",
            "硬件",
        ],
        false,
    ),
    (
        "act.recyclebin",
        "🗑",
        "打开回收站",
        "Open Recycle Bin",
        &["recycle bin", "huishouzhan", "回收站"],
        false,
    ),
    (
        "act.shutdown",
        "⏻",
        "关机",
        "Shut down",
        &["shutdown", "power off", "guanji", "关机"],
        true,
    ),
    (
        "act.restart",
        "🔄",
        "重启",
        "Restart",
        &["restart", "reboot", "chongqi", "重启"],
        true,
    ),
    (
        "act.cmd",
        "⬛",
        "命令提示符",
        "Command Prompt",
        &["cmd", "command prompt", "mingling tishi", "命令提示符", "命令行", "cmd.exe"],
        false,
    ),
    (
        "act.powershell",
        "🔷",
        "Windows PowerShell",
        "Windows PowerShell",
        &["powershell", "ps", "shell", "脚本", "命令行"],
        false,
    ),
    (
        "act.regedit",
        "📝",
        "注册表编辑器",
        "Registry Editor",
        &["regedit", "registry", "zhucebiao", "注册表"],
        false,
    ),
    (
        "act.gpedit",
        "🧭",
        "本地组策略编辑器",
        "Group Policy Editor",
        &["gpedit", "group policy", "zucelue", "组策略"],
        false,
    ),
    (
        "act.services",
        "⚙",
        "服务",
        "Services",
        &["services", "service", "fuwu", "服务", "services.msc"],
        false,
    ),
    (
        "act.diskmgmt",
        "💽",
        "磁盘管理",
        "Disk Management",
        &["disk management", "diskmgmt", "cipan guanli", "磁盘管理", "分区"],
        false,
    ),
    (
        "act.compmgmt",
        "🖥",
        "计算机管理",
        "Computer Management",
        &["computer management", "compmgmt", "jisuanji guanli", "计算机管理"],
        false,
    ),
    (
        "act.eventvwr",
        "📋",
        "事件查看器",
        "Event Viewer",
        &["event viewer", "eventvwr", "shijian chakanqi", "事件查看器", "日志"],
        false,
    ),
    (
        "act.perfmon",
        "📊",
        "性能监视器",
        "Performance Monitor",
        &["performance monitor", "perfmon", "xingneng jianshiqi", "性能监视器"],
        false,
    ),
    (
        "act.taskschd",
        "⏰",
        "任务计划程序",
        "Task Scheduler",
        &["task scheduler", "taskschd", "renwu jihua", "任务计划程序", "计划任务"],
        false,
    ),
    (
        "act.appwiz",
        "📦",
        "程序和功能",
        "Programs and Features",
        &["programs and features", "appwiz", "uninstall", "程序", "卸载", "功能"],
        false,
    ),
    (
        "act.ncpa",
        "🔗",
        "网络连接",
        "Network Connections",
        &[
            "network connections",
            "ncpa",
            "wangluo lianjie",
            "网络连接",
            "网卡",
            "适配器",
        ],
        false,
    ),
    (
        "act.sysdm",
        "🧾",
        "系统属性",
        "System Properties",
        &[
            "system properties",
            "sysdm",
            "xitong shuxing",
            "系统属性",
            "环境变量",
            "高级",
        ],
        false,
    ),
    (
        "act.optionalfeatures",
        "🧩",
        "启用或关闭 Windows 功能",
        "Windows Features",
        &["windows features", "optionalfeatures", "gongneng", "功能", "组件"],
        false,
    ),
    (
        "act.cleanmgr",
        "🧹",
        "磁盘清理",
        "Disk Cleanup",
        &["disk cleanup", "cleanmgr", "cipan qingli", "磁盘清理", "清理"],
        false,
    ),
    (
        "act.mstsc",
        "🖥",
        "远程桌面连接",
        "Remote Desktop Connection",
        &["remote desktop", "mstsc", "yuancheng zhuomian", "远程桌面", "rdp"],
        false,
    ),
];

/// 动作怎么被拉起来。两种启动形态（普通 / 提权）必须读同一张表。
enum Launch {
    /// 命令行 argv —— 用 `Command::spawn` 起，不走 shell（无拼接面）
    Argv(&'static [&'static str]),
    /// 交给 Shell 的 `"open"` —— `.msc` / `.cpl` / `shell:` 这类由 Windows 自己解析
    Shell(&'static str),
}

struct ActionSpec {
    launch: Launch,
    /// 「以管理员身份运行」的形态；None = 该动作没有提权形态
    elevate: Option<Launch>,
}

fn action_spec(id: &str) -> Option<ActionSpec> {
    let both = |f: &'static str| ActionSpec {
        launch: Launch::Shell(f),
        elevate: Some(Launch::Shell(f)),
    };
    let plain = |f: &'static str| ActionSpec {
        launch: Launch::Shell(f),
        elevate: None,
    };
    let argv = |a: &'static [&'static str]| ActionSpec {
        launch: Launch::Argv(a),
        elevate: None,
    };
    Some(match id {
        // 电源 / 会话（不提权）
        "act.lock" => argv(&["rundll32.exe", "user32.dll,LockWorkStation"]),
        "act.sleep" => argv(&["rundll32.exe", "powrprof.dll,SetSuspendState", "0,1,0"]),
        "act.shutdown" => argv(&["shutdown.exe", "/s", "/t", "0"]),
        "act.restart" => argv(&["shutdown.exe", "/r", "/t", "0"]),
        "act.recyclebin" => plain("shell:RecycleBinFolder"),

        // 系统管理工具（都可提权）
        "act.taskmgr" => both("taskmgr.exe"),
        "act.control" => both("control.exe"),
        "act.devmgmt" => both("devmgmt.msc"),
        "act.resmon" => both("resmon.exe"),
        "act.sysinfo" => both("msinfo32.exe"),
        "act.services" => both("services.msc"),
        "act.diskmgmt" => both("diskmgmt.msc"),
        "act.compmgmt" => both("compmgmt.msc"),
        "act.eventvwr" => both("eventvwr.msc"),
        "act.perfmon" => both("perfmon.msc"),
        "act.taskschd" => both("taskschd.msc"),
        "act.appwiz" => both("appwiz.cpl"),
        "act.ncpa" => both("ncpa.cpl"),
        "act.sysdm" => both("sysdm.cpl"),
        "act.optionalfeatures" => both("optionalfeatures.exe"),
        "act.cleanmgr" => both("cleanmgr.exe"),
        "act.mstsc" => both("mstsc.exe"),

        // 命令行与编辑器（提权是常见诉求）
        "act.cmd" => both("cmd.exe"),
        "act.powershell" => both("powershell.exe"),
        "act.regedit" => both("regedit.exe"),
        "act.gpedit" => both("gpedit.msc"),
        _ => return None,
    })
}

fn has_elevated_form(id: &str) -> bool {
    action_spec(id).map(|s| s.elevate.is_some()).unwrap_or(false)
}

/// 全部条目（设置页 + 动作）。数据量很小（几十条），前端一次拉走、本地过滤。
pub fn all() -> Vec<CatalogItem> {
    let mut items = Vec::with_capacity(SETTINGS.len() + ACTIONS.len());
    for &(id, icon, zh, en, uri, keywords) in SETTINGS {
        items.push(CatalogItem {
            id,
            kind: "setting",
            icon,
            title_zh: zh,
            title_en: en,
            target: uri,
            danger: false,
            // ms-settings: 页面是 Windows 自己拉起的设置宿主，没有提权形态
            elevatable: false,
            keywords,
        });
    }
    for &(id, icon, zh, en, keywords, danger) in ACTIONS {
        items.push(CatalogItem {
            id,
            kind: "action",
            icon,
            title_zh: zh,
            title_en: en,
            target: id,
            danger,
            elevatable: has_elevated_form(id),
            keywords,
        });
    }
    items
}

/// 按查询词过滤（空查询 = 全部）。标题（中/英）与关键词命中任一即可。
pub fn search(query: &str) -> Vec<CatalogItem> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return all();
    }
    all()
        .into_iter()
        .filter(|item| {
            item.title_zh.to_lowercase().contains(&needle)
                || item.title_en.to_lowercase().contains(&needle)
                || item
                    .keywords
                    .iter()
                    .any(|k| k.to_lowercase().contains(&needle))
        })
        .collect()
}

/// 打开 Windows 设置页。**只接受 `ms-settings:` 前缀**。
pub fn open_setting(target: &str) -> Result<(), String> {
    if !is_setting_uri(target) {
        return Err(format!("not a settings page: {}", target.trim()));
    }
    lunac_indexer::app_indexer::launch_app(target.trim())
}

fn is_setting_uri(target: &str) -> bool {
    target
        .trim()
        .to_ascii_lowercase()
        .starts_with("ms-settings:")
}

/// 执行系统动作。**只认白名单 id**（前端传的是 id，不是命令）。
pub fn run_action(id: &str) -> Result<(), String> {
    let spec = action_spec(id).ok_or_else(|| format!("unknown action: {id}"))?;
    start(&spec.launch)
}

/// 以管理员身份运行一个系统动作（**会弹 UAC**）。没有提权形态的直接 Err。
pub fn run_action_elevated(id: &str) -> Result<(), String> {
    let spec = action_spec(id).ok_or_else(|| format!("unknown action: {id}"))?;
    let elevate = spec
        .elevate
        .ok_or_else(|| format!("该动作没有提权形态: {id}"))?;
    start_elevated(&elevate)
}

fn start(launch: &Launch) -> Result<(), String> {
    match launch {
        Launch::Argv(argv) => spawn(argv),
        Launch::Shell(target) => lunac_indexer::app_indexer::launch_app(target),
    }
}

/// 提权启动：一律走 ShellExecuteW 的 `runas`（Argv 形态拆成 file + parameters）。
fn start_elevated(launch: &Launch) -> Result<(), String> {
    match launch {
        Launch::Argv(argv) => {
            let (file, args) = argv.split_first().ok_or("empty argv")?;
            let params = if args.is_empty() {
                None
            } else {
                Some(args.join(" "))
            };
            lunac_indexer::app_indexer::launch_elevated(file, params.as_deref())
        }
        Launch::Shell(target) => lunac_indexer::app_indexer::launch_elevated(target, None),
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
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_targets_are_sane() {
        let items = all();
        assert!(items.len() > 60, "设置页 + 动作至少六十来项");
        let mut seen = HashSet::new();
        for item in &items {
            assert!(seen.insert(item.id), "重复 id: {}", item.id);
            assert!(!item.keywords.is_empty(), "{} 必须可搜", item.id);
            if item.kind == "setting" {
                assert!(item.target.starts_with("ms-settings:"));
                assert!(!item.danger);
                assert!(!item.elevatable, "设置页不该有提权形态");
            } else {
                assert_eq!(item.kind, "action");
                assert_eq!(item.target, item.id, "动作的 target 就是 id");
                assert!(!item.title_en.is_empty() && !item.title_zh.is_empty());
            }
        }
    }

    #[test]
    fn only_shutdown_and_restart_are_dangerous() {
        let dangerous: Vec<&str> = all()
            .into_iter()
            .filter(|item| item.danger)
            .map(|item| item.id)
            .collect();
        assert_eq!(dangerous, vec!["act.shutdown", "act.restart"]);
    }

    #[test]
    fn every_action_id_has_an_execution_spec() {
        for item in all().into_iter().filter(|i| i.kind == "action") {
            assert!(
                action_spec(item.id).is_some(),
                "{} 在表里出现了，但 action_spec() 里没有对应描述",
                item.id
            );
        }
    }

    #[test]
    fn elevatable_set_is_exactly_the_reviewed_list() {
        const EXPECTED: &[&str] = &[
            "act.appwiz",
            "act.cleanmgr",
            "act.cmd",
            "act.compmgmt",
            "act.control",
            "act.devmgmt",
            "act.diskmgmt",
            "act.eventvwr",
            "act.gpedit",
            "act.mstsc",
            "act.ncpa",
            "act.optionalfeatures",
            "act.perfmon",
            "act.powershell",
            "act.regedit",
            "act.resmon",
            "act.services",
            "act.sysdm",
            "act.sysinfo",
            "act.taskmgr",
            "act.taskschd",
        ];
        let mut got: Vec<&str> = all()
            .into_iter()
            .filter(|item| item.elevatable)
            .map(|item| item.id)
            .collect();
        got.sort();
        assert_eq!(got, EXPECTED);
    }

    #[test]
    fn shell_and_admin_tools_are_present_and_searchable() {
        for (id, needle) in [
            ("act.cmd", "cmd"),
            ("act.powershell", "powershell"),
            ("act.services", "services.msc"),
            ("act.appwiz", "appwiz"),
            ("act.ncpa", "ncpa"),
            ("act.regedit", "regedit"),
            ("act.eventvwr", "eventvwr"),
        ] {
            let item = all()
                .into_iter()
                .find(|i| i.id == id)
                .unwrap_or_else(|| panic!("缺少系统工具 {id}"));
            assert!(
                item.keywords.iter().any(|k| k.eq_ignore_ascii_case(needle)),
                "{id} 的关键词里应含 {needle}"
            );
        }
    }

    #[test]
    fn search_matches_titles_and_keywords() {
        assert!(search("display").iter().any(|i| i.id == "set.display"));
        assert!(search("关机").iter().any(|i| i.id == "act.shutdown"));
        assert!(search("regedit").iter().any(|i| i.id == "act.regedit"));
        assert!(search("").len() == all().len());
        assert!(search("no-such-thing-xyz").is_empty());
    }

    #[test]
    fn arbitrary_targets_are_rejected() {
        assert!(is_setting_uri("ms-settings:display"));
        assert!(is_setting_uri("  MS-SETTINGS:about  "), "大小写/空白要容忍");
        for bad in [
            "http://example.com",
            "https://example.com",
            "C:\\Windows\\System32\\cmd.exe",
            "",
        ] {
            assert!(!is_setting_uri(bad), "{bad} 不该被当成设置页");
        }
        for bad in ["rm -rf /", "act.shutdown; rm -rf /", "../../x", ""] {
            assert!(run_action(bad).is_err(), "{bad} 不该被执行");
        }
        assert!(run_action("act.unknown").is_err());
        assert!(run_action_elevated("act.unknown").is_err());
        assert!(run_action_elevated("act.recyclebin").is_err(), "回收站没有提权形态");
        assert!(run_action_elevated("act.lock").is_err(), "锁定会话没有提权形态");
        assert!(run_action_elevated("act.shutdown").is_err(), "关机没有提权形态");
    }
}
