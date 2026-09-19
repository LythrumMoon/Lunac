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
    /// **是否有「以管理员身份运行」形态**（`run_action_elevated` 能执行它）。
    /// 由 `action_spec()` 推导，**不在表里手写** —— 手写必然与执行表漂移。
    pub elevatable: bool,
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
        // ms-settings: 页面是 Windows 自己拉起的设置宿主，没有提权形态
        elevatable: false,
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
        elevatable: has_elevated_form(id),
        keywords: kw.iter().map(|s| s.to_string()).collect(),
    }
}

// ── 动作执行表 ────────────────────────────────────────────────────
//
// **新增一个动作 = 在 `all()` 里加一条 + 在 `action_spec()` 里加一个分支**，
// 不要再往 `run_action()` 里堆 `match` 分支 —— 两种启动形态（普通 / 提权）
// 必须读同一张表，否则「界面上写着能以管理员运行、执行时却说没有提权形态」。

/// 一个动作怎么被拉起来。
enum Launch {
    /// 命令行 argv —— 用 `Command::spawn` 起，**不走 shell**（无拼接面）
    Argv(&'static [&'static str]),
    /// 交给 `ShellExecuteW` 的 `"open"` verb —— `.msc` / `.cpl` / `shell:` 这类
    /// 由 Windows 自己解析的目标必须走这条（`Command::spawn` 起不来 .msc）
    Shell(&'static str),
}

struct ActionSpec {
    launch: Launch,
    /// 「以管理员身份运行」的形态；`None` = 该动作没有提权形态。
    /// 注意：`runas` 一律弹 UAC，所以只给「真的需要管理员」的东西开这一档。
    elevate: Option<Launch>,
}

/// 动作 id → 启动描述。**这是动作的唯一真相源**（见上方注释）。
fn action_spec(id: &str) -> Option<ActionSpec> {
    // 可 `open` 可提权的常规目标（exe / msc / cpl）
    let both = |f: &'static str| ActionSpec {
        launch: Launch::Shell(f),
        elevate: Some(Launch::Shell(f)),
    };
    // 只能普通打开（不需要 / 不该提权，比如 shell: 命名空间、娱乐性动作）
    let plain = |f: &'static str| ActionSpec {
        launch: Launch::Shell(f),
        elevate: None,
    };
    // 自带 argv、不提权（rundll32 / shutdown 这类不该也不需要在提权环境跑）
    let argv = |a: &'static [&'static str]| ActionSpec {
        launch: Launch::Argv(a),
        elevate: None,
    };
    Some(match id {
        // ── 电源 / 会话（不提权）──
        "act.lock" => argv(&["rundll32.exe", "user32.dll,LockWorkStation"]),
        "act.sleep" => argv(&["rundll32.exe", "powrprof.dll,SetSuspendState", "0,1,0"]),
        "act.shutdown" => argv(&["shutdown.exe", "/s", "/t", "0"]),
        "act.restart" => argv(&["shutdown.exe", "/r", "/t", "0"]),
        "act.recyclebin" => plain("shell:RecycleBinFolder"),

        // ── 系统管理工具（都可提权）──
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

        // ── 命令行与编辑器（提权是常见诉求）──
        "act.cmd" => both("cmd.exe"),
        "act.powershell" => both("powershell.exe"),
        "act.regedit" => both("regedit.exe"),
        "act.gpedit" => both("gpedit.msc"),
        _ => return None,
    })
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

        // ── 系统命令与工具（2026-09-19 补齐：Win+S 能搜到而我们搜不到的）──
        // 这些是「Windows 搜索里输入 cmd / 服务 / 事件查看器就能直接打开」的那批，
        // 之前只能靠开始菜单的 .lnk 命中 `cmd.exe` 之类的快捷方式，`regedit` /
        // `services.msc` / `appwiz.cpl` 则**完全搜不到**（开始菜单里没有它们的入口）。
        action("act.cmd", "⬛", "命令提示符", "Command Prompt",
            &["cmd", "command prompt", "mingling tishi", "命令提示符", "命令行", "cmd.exe"], false),
        action("act.powershell", "🔷", "Windows PowerShell", "Windows PowerShell",
            &["powershell", "ps", "shell", "脚本", "命令行"], false),
        action("act.regedit", "📝", "注册表编辑器", "Registry Editor",
            &["regedit", "registry", "zhucebiao", "注册表"], false),
        action("act.gpedit", "🧭", "本地组策略编辑器", "Group Policy Editor",
            &["gpedit", "group policy", "zucelue", "组策略"], false),
        action("act.services", "⚙", "服务", "Services",
            &["services", "service", "fuwu", "服务", "services.msc"], false),
        action("act.diskmgmt", "💽", "磁盘管理", "Disk Management",
            &["disk management", "diskmgmt", "cipan guanli", "磁盘管理", "分区"], false),
        action("act.compmgmt", "🖥", "计算机管理", "Computer Management",
            &["computer management", "compmgmt", "jisuanji guanli", "计算机管理"], false),
        action("act.eventvwr", "📋", "事件查看器", "Event Viewer",
            &["event viewer", "eventvwr", "shijian chakanqi", "事件查看器", "日志"], false),
        action("act.perfmon", "📊", "性能监视器", "Performance Monitor",
            &["performance monitor", "perfmon", "xingneng jianshiqi", "性能监视器"], false),
        action("act.taskschd", "⏰", "任务计划程序", "Task Scheduler",
            &["task scheduler", "taskschd", "renwu jihua", "任务计划程序", "计划任务"], false),
        action("act.appwiz", "📦", "程序和功能", "Programs and Features",
            &["programs and features", "appwiz", "uninstall", "程序", "卸载", "功能"], false),
        action("act.ncpa", "🔗", "网络连接", "Network Connections",
            &["network connections", "ncpa", "wangluo lianjie", "网络连接", "网卡", "适配器"], false),
        action("act.sysdm", "🧾", "系统属性", "System Properties",
            &["system properties", "sysdm", "xitong shuxing", "系统属性", "环境变量", "高级"], false),
        action("act.optionalfeatures", "🧩", "启用或关闭 Windows 功能", "Windows Features",
            &["windows features", "optionalfeatures", "gongneng", "功能", "组件"], false),
        action("act.cleanmgr", "🧹", "磁盘清理", "Disk Cleanup",
            &["disk cleanup", "cleanmgr", "cipan qingli", "磁盘清理", "清理"], false),
        action("act.mstsc", "🖥", "远程桌面连接", "Remote Desktop Connection",
            &["remote desktop", "mstsc", "yuancheng zhuomian", "远程桌面", "rdp"], false),
    ]
}

/// 该动作是否有「以管理员身份运行」形态（`CatalogItem::elevatable` 的数据源）。
fn has_elevated_form(id: &str) -> bool {
    action_spec(id).map(|s| s.elevate.is_some()).unwrap_or(false)
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
    let spec = action_spec(id).ok_or_else(|| format!("unknown action: {id}"))?;
    start(&spec.launch)
}

/// 以管理员身份运行一个系统动作（**会弹 UAC**）。
/// 没有提权形态的动作（`elevatable == false`）在这里返回 Err，前端据此提示 ——
/// 判据与 `CatalogItem::elevatable` 同源，不会出现「界面说有、执行说没有」。
pub fn run_action_elevated(id: &str) -> Result<(), String> {
    let spec = action_spec(id).ok_or_else(|| format!("unknown action: {id}"))?;
    let el = spec
        .elevate
        .ok_or_else(|| format!("该动作没有提权形态: {id}"))?;
    start_elevated(&el)
}

fn start(l: &Launch) -> Result<(), String> {
    match l {
        Launch::Argv(argv) => spawn(argv),
        Launch::Shell(target) => crate::app_indexer::launch_app(target),
    }
}

/// 提权启动：一律走 `ShellExecuteW` 的 `runas` verb（`Argv` 形态则把 argv 拆成
/// file + parameters —— `runas` 只能作用在 ShellExecuteW 上，`Command::spawn` 没有提权通道）。
fn start_elevated(l: &Launch) -> Result<(), String> {
    match l {
        Launch::Argv(argv) => {
            let (file, args) = argv.split_first().ok_or("empty argv")?;
            let params = if args.is_empty() { None } else { Some(args.join(" ")) };
            crate::app_indexer::launch_elevated(file, params.as_deref())
        }
        Launch::Shell(target) => crate::app_indexer::launch_elevated(target, None),
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

    /// **每个动作条目都必须在执行表里有分支** —— 防「界面上搜得到、点了说 unknown action」。
    /// 这是把条目表和执行表绑在一起的那道闸门，新增动作时最先被它拦住。
    #[test]
    fn every_action_id_has_an_execution_spec() {
        for it in all().into_iter().filter(|i| i.kind == "action") {
            assert!(
                action_spec(&it.id).is_some(),
                "{} 在 all() 里出现了，但 action_spec() 里没有对应的启动描述",
                it.id
            );
            assert_eq!(it.target, it.id, "动作的 target 就是 id");
        }
    }

    /// `elevatable` 必须恰好等于约定好的那批 id：**多一个（偷偷开了提权）少一个（界面漏了盾牌）
    /// 都要在这里失败**。设置页永远不可提权。
    #[test]
    fn elevatable_set_is_exactly_the_reviewed_list() {
        const EXPECTED: &[&str] = &[
            "act.appwiz", "act.cleanmgr", "act.cmd", "act.compmgmt", "act.control",
            "act.devmgmt", "act.diskmgmt", "act.eventvwr", "act.gpedit", "act.mstsc",
            "act.ncpa", "act.optionalfeatures", "act.perfmon", "act.powershell",
            "act.regedit", "act.resmon", "act.services", "act.sysdm", "act.sysinfo",
            "act.taskmgr", "act.taskschd",
        ];
        let mut got: Vec<String> = all()
            .into_iter()
            .filter(|i| i.elevatable)
            .map(|i| i.id)
            .collect();
        got.sort();
        assert_eq!(got, EXPECTED);
        for it in all().into_iter().filter(|i| i.kind == "setting") {
            assert!(!it.elevatable, "设置页不该有提权形态: {}", it.id);
        }
    }

    /// 2026-09-19 补齐的那批系统命令/工具必须在表里，且能被常见关键词搜到。
    /// （缺了就是「Win+S 搜得到、Lunac 搜不到」的老问题复发。）
    #[test]
    fn shell_and_admin_tools_are_present_and_searchable() {
        let items = all();
        let has = |id: &str| items.iter().any(|i| i.id == id);
        for id in [
            "act.cmd", "act.powershell", "act.regedit", "act.gpedit", "act.services",
            "act.diskmgmt", "act.compmgmt", "act.eventvwr", "act.perfmon", "act.taskschd",
            "act.appwiz", "act.ncpa", "act.sysdm", "act.optionalfeatures", "act.cleanmgr",
            "act.mstsc",
        ] {
            assert!(has(id), "缺少系统工具 {id}");
        }
        // 关键词必须覆盖用户在 Windows 搜索框里会敲的那几个串
        let kw_of = |id: &str| -> Vec<String> {
            items
                .iter()
                .find(|i| i.id == id)
                .map(|i| i.keywords.clone())
                .unwrap_or_default()
        };
        for (id, needle) in [
            ("act.cmd", "cmd"),
            ("act.powershell", "powershell"),
            ("act.services", "services.msc"),
            ("act.appwiz", "appwiz"),
            ("act.ncpa", "ncpa"),
            ("act.regedit", "regedit"),
            ("act.eventvwr", "eventvwr"),
        ] {
            assert!(
                kw_of(id).iter().any(|k| k.eq_ignore_ascii_case(needle)),
                "{id} 的关键词里应含 {needle}"
            );
        }
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
        assert!(run_action_elevated("act.unknown").is_err());
        // 没有提权形态的动作，提权入口必须直接拒绝（**不弹 UAC**）
        assert!(run_action_elevated("act.recyclebin").is_err(), "回收站没有提权形态");
        assert!(run_action_elevated("act.lock").is_err(), "锁定会话没有提权形态");
        assert!(run_action_elevated("act.shutdown").is_err(), "关机没有提权形态");
    }
}
