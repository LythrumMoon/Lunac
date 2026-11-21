//! Lunac v2 的 Rust 侧薄层（M1：搜索主页面）。
//!
//! 边界（docs/flutter-migration.md §一 双端架构原则）：
//!   · 业务逻辑在 Rust；Dart 只做表现与平台壳；
//!   · 本 crate 承载「内置命令 + 应用 / 文件搜索 + 动作执行」，索引实现来自共享 crate
//!     `lunac-indexer`（与 v1 同一份代码）。
//!
//! ABI 约定：
//!   · 返回的字符串由 Rust 分配，调用方读完后必须调用 `lunac_free_string` 释放；
//!   · 动作返回码：0 = 已执行，1 = 未知动作，2 = 由 UI 处理，3 = 执行失败。

use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Once;

use serde_json::json;

mod agent;
mod appearance;
mod auto_start;
mod change_review;
mod chat_history;
mod chat_settings;
mod child_job;
mod clipboard;
mod clipboard_history;
mod clipboard_payload;
mod convert;
mod cron;
mod hotkey;
mod image_pattern;
mod library;
mod log;
mod mcp_bridge;
mod mcp_config;
mod media_lib;
mod memo;
mod memory;
mod mouse_hook;
mod music;
mod music_online;
mod music_spotify;
mod native_proxy;
mod ocr;
mod pet;
mod pet_voice;
mod plan_archive;
mod player;
mod plugin_market;
// 插件存储（v1 `plugin_kv` 的 v2 等价物，docs/todo.md §M5z-补69）：
// `<exe 根>/ModuleData/plugins/<id>/kv.json`，配额 + 原子写。
mod plugin_kv;
mod plugin_runtime;
// 插件开发工具链（轨道 H，docs/todo.md §M5z-补69）：授权账本 + 安装 + 构建 + 停止。
mod plugin_toolchain;
mod proxy;
mod recency;
mod resource_hub;
mod sandbox;
mod security_ledger;
// 会话轨迹导出（docs/todo.md §待办 3-28.12）：会话 → NDJSON 事件行（脱敏 + 截断）。
mod session_export;
mod settings;
mod single_instance;
mod skills_admin;
// 技能安装前的安全闸（docs/todo.md §待办 3-28.9）：注入 / 凭据 / 危险命令的启发式扫描。
mod skill_scan;
mod system_catalog;
// 通用后台任务队列（docs/todo.md §待办 3-28.16 ①，2026-11-16）：重建 v1 丢失的
// `task_queue.rs` —— worker 只读档单发 agent，契约与 core-agent 的 TaskEnqueue 一致。
mod task_queue;
mod terminal;
mod translate;
mod tuning;
mod tuning_probe;
mod usage;
mod vscode_bridge;

/// ABI 版本：Dart 侧启动时校验，避免 DLL 与 Dart 代码错配时静默读错内存。
const ABI_VERSION: u32 = 2;

/// 单次搜索最多返回的条数（界面最多显示 5 行，其余在列表里滚动）。
const MAX_RESULTS: usize = 12;
/// 简要搜索里应用占的上限（空查询只取最近打开的 5 个，v1 `search_apps` 同口径）。
const MAX_APP_HITS: usize = 6;

/// 一条内置命令 / 目录动作。
struct Entry {
    id: &'static str,
    title: &'static str,
    subtitle: &'static str,
    kind: &'static str,
    /// 额外可匹配的关键词（空格分隔），让中英文都能搜到同一条目。
    keywords: &'static str,
}

const CATALOG: &[Entry] = &[
    Entry {
        id: "web-home",
        title: "Web 搜索",
        subtitle: "在浏览器中打开搜索首页",
        kind: "web",
        keywords: "web google bing baidu search 搜索 网页 浏览器",
    },
    Entry {
        id: "ai",
        title: "AI 对话",
        subtitle: "与 Lunac 助手对话",
        kind: "command",
        keywords: "ai chat 对话 助手 assistant",
    },
    Entry {
        id: "settings",
        title: "设置",
        subtitle: "打开 Lunac 设置（M3 接入）",
        kind: "command",
        keywords: "settings 设置 配置 config",
    },
    Entry {
        id: "clipboard-history",
        title: "剪贴板历史",
        subtitle: "最近复制过的文本与文件",
        kind: "command",
        keywords: "clipboard history paste 剪贴板 剪切板 历史 粘贴 复制",
    },
    Entry {
        id: "memo",
        title: "备忘录",
        subtitle: "本地快速记录（可设检索标识）",
        kind: "command",
        keywords: "memo note notes 备忘 备忘录 笔记 记录",
    },
    Entry {
        id: "quick-launch",
        title: "快捷启动",
        subtitle: "打开自定义文件与程序",
        kind: "command",
        keywords: "launch open run start custom app file path 启动 打开 运行 自定义 快捷 启动器 快速启动 程序 文件 路径",
    },
    Entry {
        id: "translate",
        title: "翻译",
        subtitle: "翻译与查词",
        kind: "command",
        // 与 v1 `app/src/plugins/builtin/translate.ts:64-67` 的关键词一致。
        keywords: "translate translation dict dictionary meaning word 翻译 词典 字典 查词 释义 译文 中英",
    },
    Entry {
        id: "convert",
        title: "文件转换",
        subtitle: "图片 / 音频 / 视频 / PDF 格式转换",
        kind: "command",
        // 与 v1 `app/src/plugins/builtin/convert.ts:470` 的关键词一致。
        keywords: "convert conversion format transcode ffmpeg libreoffice pdf 转换 格式 转码 提取音频 图片转换 视频转换 音频转换",
    },
    Entry {
        id: "ocr",
        title: "OCR 文字识别",
        subtitle: "图片文字识别（PaddleOCR · 离线高精度）",
        kind: "command",
        // 与 v1 `app/src/plugins/builtin/ocr.ts:172-177` 的关键词一致。
        keywords: "ocr 识别 文字识别 图像识别 图片转文字 截图识别 图识字 文字提取",
    },
    Entry {
        id: "music",
        // 标题与 v1 插件清单 `app/plugin-dist/music/lunac-plugin.json` 的 `name` 一致。
        title: "音乐",
        // v1 的 Spotify 在线库在 v2 未接入；歌词已接在线（LRCLIB → 网易云兜底）。
        subtitle: "本地媒体库 · 在线歌词（LRCLIB / 网易云）",
        kind: "command",
        keywords: "music 音乐 歌词 lyrics lrc 本地 媒体库 歌曲 播放 spotify lrclib",
    },
    Entry {
        id: "tool-editor",
        title: "工具编辑器",
        subtitle: "管理 AI Agent 自定义工具（MCP 桥）",
        kind: "command",
        // 与 v1 `app/src/plugins/builtin/tool-editor.ts:264` 的关键词一致。
        keywords: "tool tools 工具 mcp agent skill 插件 扩展 工具编辑器 自定义工具",
    },
    Entry {
        id: "proxy",
        title: "代理",
        subtitle: "Windows 系统代理开关（本地服务 / 内核未接入）",
        kind: "command",
        // 与 v1 `app/src/plugins/builtin/proxy.ts:1000` 的关键词一致。
        keywords: "proxy 代理 系统代理 梯子 vpn socks5 http proxy 订阅 节点",
    },
    Entry {
        id: "pet",
        title: "桌宠",
        subtitle: "独立透明置顶的桌面形象窗（图片或 Live2D 模型）",
        kind: "command",
        // 与 v1 `<Modules>\pet\lunac-plugin.json` 的 keywords 一致。
        keywords: "pet 桌宠 宠物 桌面宠物 live2d 看板娘 吉祥物 desktop pet",
    },
    Entry {
        id: "open-exe-dir",
        title: "打开应用目录",
        subtitle: "Lunac 可执行文件所在目录",
        kind: "folder",
        keywords: "open folder exe dir 目录 应用 位置",
    },
    Entry {
        id: "open-downloads",
        title: "打开下载文件夹",
        subtitle: "用户下载目录",
        kind: "folder",
        keywords: "downloads 下载 目录 folder",
    },
    Entry {
        id: "quit",
        title: "退出 Lunac",
        subtitle: "关闭应用",
        kind: "command",
        keywords: "quit exit 退出 关闭 close",
    },
];

/// 返回 agent 状态（NUL 结尾的 UTF-8 静态串）。
///
/// 调用方只读，**不得 free**：指针指向静态内存，生命周期 = 进程。
/// M2 起真实运行态请用 `lunac_agent_running`（本函数只保留给启动日志的探针）。
#[no_mangle]
pub extern "C" fn lunac_agent_status() -> *const c_char {
    static STATUS: &[u8] = b"agent: ready (see lunac_agent_running)\0";
    STATUS.as_ptr().cast()
}

/// 当前 C ABI 版本（Dart 侧据此判断库与代码是否配套）。
#[no_mangle]
pub extern "C" fn lunac_abi_version() -> u32 {
    ABI_VERSION
}

/// 初始化宿主日志（幂等）：`<exe 根>\temp\logs\lunac-YYYY-MM-DD.log`。
///
/// 不调也会在第一次 `lunac_log_write` 时自动初始化；Dart 在 `main()` 里显式调一次，
/// 可以让「启动早期」的异常也落盘（见 `log.rs` 的开关与级别）。
#[no_mangle]
pub extern "C" fn lunac_log_init() {
    log::init("lunac");
}

/// 写一条宿主日志。`level`：1=error 2=warn 3=info 4=debug（其余按 info）。
///
/// `msg` 为 NUL 结尾的 UTF-8；**不返回**字符串，调用方无需 free。
#[no_mangle]
pub extern "C" fn lunac_log_write(level: u32, msg: *const c_char) {
    if msg.is_null() {
        return;
    }
    let text = unsafe { CStr::from_ptr(msg) }.to_string_lossy().to_string();
    let level = match level {
        1 => log::Level::Error,
        2 => log::Level::Warn,
        4 => log::Level::Debug,
        _ => log::Level::Info,
    };
    log::log(level, text);
}

/// 启动常驻 agent（M2；幂等）。返回空串 = 已启动 / 已在运行；否则是可读的失败原因。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_agent_start() -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string("无法定位可执行文件目录".to_string());
    };
    match agent::start(&root) {
        Ok(()) => to_c_string(String::new()),
        Err(reason) => to_c_string(reason),
    }
}

/// 停掉 agent（退出应用时调用；UI 关闭对话面板不调）。
#[no_mangle]
pub extern "C" fn lunac_agent_stop() {
    agent::stop();
}

/// agent 是否在运行（1 = 是）。
#[no_mangle]
pub extern "C" fn lunac_agent_running() -> i32 {
    if agent::running() {
        1
    } else {
        0
    }
}

// ── VSCode 扩展的本机 HTTP / SSE 桥（v1 `agent_server.rs`，2026-11-15 移植）────────
//
// 三个端点与 v1 逐条一致（`/health` / `/agent/start` / `/agent/message` / `/agent/stream`），
// 共用**同一个** agent 进程。桥本身只有在 `start` 成功后才占端口；失败原因是给人看的。

/// 起桥（幂等）。返回 JSON：`ok` / `addr` / `running` / `error`。
#[no_mangle]
pub extern "C" fn lunac_vscode_bridge_start() -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string(
            json!({ "ok": false, "addr": vscode_bridge::BRIDGE_ADDR, "running": false,
                    "error": "无法定位可执行文件目录" })
            .to_string(),
        );
    };
    let payload = match vscode_bridge::start(root) {
        Ok(()) => json!({ "ok": true, "addr": vscode_bridge::BRIDGE_ADDR,
                          "running": vscode_bridge::running(), "error": "" }),
        Err(reason) => json!({ "ok": false, "addr": vscode_bridge::BRIDGE_ADDR,
                               "running": vscode_bridge::running(), "error": reason }),
    };
    to_c_string(payload.to_string())
}

/// 桥的诊断状态（不阻塞）。JSON：`running` / `addr` / `subscribers`。
#[no_mangle]
pub extern "C" fn lunac_vscode_bridge_status() -> *mut c_char {
    to_c_string(
        json!({
            "running": vscode_bridge::running(),
            "addr": vscode_bridge::BRIDGE_ADDR,
            "subscribers": vscode_bridge::subscriber_count(),
        })
        .to_string(),
    )
}

/// 停桥（幂等；只关监听与 SSE，不杀 agent）。
#[no_mangle]
pub extern "C" fn lunac_vscode_bridge_stop() {
    vscode_bridge::stop();
}

/// 计划留档（§待办 3-28.3）：`<exe 根>/ModuleData/plans/plan-<时间>.json`。
///
/// 返回 JSON：`{"ok":true,"path":"…"}` 或 `{"ok":false,"error":"…"}`。
/// 三份文本都由 Dart 侧给（原始计划 / 改后计划 / 备注）；本函数只负责落盘与命名。
#[no_mangle]
pub extern "C" fn lunac_plan_archive(
    original: *const c_char,
    edited: *const c_char,
    note: *const c_char,
) -> *mut c_char {
    let Some(root) = exe_dir() else {
        return to_c_string(json!({ "ok": false, "error": "拿不到程序目录" }).to_string());
    };
    match plan_archive::archive(&root, &read_arg(original), &read_arg(edited), &read_arg(note)) {
        Ok(path) => to_c_string(
            json!({ "ok": true, "path": path.to_string_lossy() }).to_string(),
        ),
        Err(error) => to_c_string(json!({ "ok": false, "error": error }).to_string()),
    }
}

/// 往 agent 的 stdin 写一行 JSON（Rust 补换行并 flush）。
///
/// 返回 0 = 已写入，1 = agent 未启动，2 = 写失败。
#[no_mangle]
pub extern "C" fn lunac_agent_send(line: *const c_char) -> i32 {
    match agent::send(&read_arg(line)) {
        Ok(()) => 0,
        Err(reason) => {
            eprintln!("[lunac-agent] send: {reason}");
            if reason.contains("未启动") {
                1
            } else {
                2
            }
        }
    }
}

/// 取一条待处理的 stream-json 事件（没有则空串）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_agent_poll() -> *mut c_char {
    to_c_string(agent::poll().unwrap_or_default())
}

/// 读 AI 配置（设置面板「AI 模型」用）：返回 JSON 对象字符串，没有配置时为 `{}`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_ai_config() -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string("{}".to_string());
    };
    to_c_string(
        serde_json::to_string(&agent::config_value(&root)).unwrap_or_else(|_| "{}".to_string()),
    )
}

/// 保存 AI 配置（与现有文件合并，不覆盖未编辑的键）。
///
/// 返回 0 = 已保存，1 = JSON 非法，2 = 写失败。
/// 保存后需要重启 agent 才生效（对话面板的「新对话」会重新拉起）。
#[no_mangle]
pub extern "C" fn lunac_set_ai_config(json: *const c_char) -> i32 {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return 2;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&read_arg(json)) else {
        return 1;
    };
    if value.as_object().is_none() {
        return 1;
    }
    match agent::save_config(&root, &value) {
        Ok(()) => 0,
        Err(reason) => {
            eprintln!("[lunac-agent] 保存 AI 配置失败: {reason}");
            2
        }
    }
}

/// 预热应用 / 文件索引（幂等；Rust 侧起后台线程扫描，本函数立即返回）。
#[no_mangle]
pub extern "C" fn lunac_index_init() {
    ensure_indexer();
}

// ── 对话「更多设置」（v1 localStorage 的 v2 对应物）────────────────────

/// 读对话设置快照：`thinking` / `runMode` / `profile` / `toolBlacklist` /
/// `toolCandidates` / `persona` / `personaMaxChars` / `workspace`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_chat_settings() -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string("{}".to_string());
    };
    to_c_string(chat_settings::snapshot_json(&root))
}

/// 合并写入对话设置（只覆盖传入的键），返回 `{"ok":bool,"error":"…"}`。
///
/// 保存后需要重启 agent 才生效（与 v1 `set_security_profile` / `set_thinking_mode` 一致）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_chat_set_settings(json: *const c_char) -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string(
            serde_json::json!({ "ok": false, "error": "无法定位可执行文件目录" }).to_string(),
        );
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&read_arg(json)) else {
        return to_c_string(
            serde_json::json!({ "ok": false, "error": "设置格式非法" }).to_string(),
        );
    };
    match chat_settings::save_patch(&root, &value) {
        Ok(_) => to_c_string(serde_json::json!({ "ok": true, "error": "" }).to_string()),
        Err(reason) => to_c_string(serde_json::json!({ "ok": false, "error": reason }).to_string()),
    }
}

/// 保存人格（空文本 = 恢复内置人格）。返回 `{"ok":bool,"error":"…"}`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_set_persona(text: *const c_char) -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string(
            serde_json::json!({ "ok": false, "error": "无法定位可执行文件目录" }).to_string(),
        );
    };
    match chat_settings::save_persona(&root, &read_arg(text)) {
        Ok(()) => to_c_string(serde_json::json!({ "ok": true, "error": "" }).to_string()),
        Err(reason) => to_c_string(serde_json::json!({ "ok": false, "error": reason }).to_string()),
    }
}

/// 读对话历史（JSON 数组字符串；没有历史时为 `[]`）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_chat_history() -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string("[]".to_string());
    };
    to_c_string(chat_history::load_json(&root))
}

/// 写入 / 覆盖一条会话（按 `id`）。
///
/// 返回 0 = 已保存，1 = JSON 非法 / 缺 id，2 = 写失败。
#[no_mangle]
pub extern "C" fn lunac_chat_history_save(session: *const c_char) -> i32 {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return 2;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&read_arg(session)) else {
        return 1;
    };
    match chat_history::upsert(&root, &value) {
        Ok(()) => 0,
        Err(reason) => {
            eprintln!("[lunac-history] 保存失败: {reason}");
            if reason.contains("id") || reason.contains("对象") {
                1
            } else {
                2
            }
        }
    }
}

/// 删除一条会话；不存在也算成功。
///
/// 返回 0 = 已处理，1 = id 为空，2 = 写失败。
#[no_mangle]
pub extern "C" fn lunac_chat_history_delete(id: *const c_char) -> i32 {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return 2;
    };
    let id = read_arg(id);
    if id.trim().is_empty() {
        return 1;
    }
    match chat_history::delete(&root, id.trim()) {
        Ok(()) => 0,
        Err(reason) => {
            eprintln!("[lunac-history] 删除失败: {reason}");
            2
        }
    }
}

/// 历史列表元数据（JSON 数组：id / title / createdAt / updatedAt / messageCount）。
///
/// 只读，不含 `messages` / `steps`；点开某条历史时再走 `lunac_chat_history_session`。
#[no_mangle]
pub extern "C" fn lunac_chat_history_meta() -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string("[]".to_string());
    };
    to_c_string(chat_history::list_meta_json(&root))
}

/// 按 id 取完整会话（JSON；不存在 = `null`）。
#[no_mangle]
pub extern "C" fn lunac_chat_history_session(id: *const c_char) -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string("null".to_string());
    };
    to_c_string(chat_history::session_json(&root, read_arg(id).trim()))
}

/// 检索库体量快照（JSON：`{ok, entries, sessions, tasks, dbBytes, error}`）。
///
/// 只读；设置页 / 日志用它判断索引是否膨胀、是否需要重建。
#[no_mangle]
pub extern "C" fn lunac_library_stats() -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string(serde_json::json!({ "ok": false, "error": "无法定位数据目录" }).to_string());
    };
    match library::stats(&root) {
        Ok(stats) => {
            let mut value = stats.to_json();
            value["ok"] = serde_json::json!(true);
            to_c_string(value.to_string())
        }
        Err(reason) => {
            to_c_string(serde_json::json!({ "ok": false, "error": reason }).to_string())
        }
    }
}

/// 系统强调色与深 / 浅色偏好（JSON：`accent` / `dark` / `source`）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_system_theme() -> *mut c_char {
    to_c_string(appearance::system_theme_json())
}

/// 已安装的主题包（JSON 数组；`<exe 根>/themes` 不存在 = `[]`，本函数不创建目录）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_themes() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(appearance::themes_json(&root))
}

/// 主题目录（**不存在则创建**；「打开主题目录」入口用）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_themes_dir() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(appearance::themes_dir(&root))
}

/// 当前主题 id（`config/appearance.json` 的 `themeId`；缺文件 = `default`）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_appearance_theme() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(appearance::theme_id(&root))
}

/// 写主题 id（未知 id 收回到 `default`），返回 `{"ok":bool,"themeId":"…","error":"…"}`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_set_appearance_theme(id: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match appearance::set_theme_id(&root, &read_arg(id)) {
        Ok(actual) => to_c_string(
            serde_json::json!({ "ok": true, "themeId": actual, "error": "" }).to_string(),
        ),
        Err(reason) => to_c_string(
            serde_json::json!({ "ok": false, "themeId": "", "error": reason }).to_string(),
        ),
    }
}

/// 外观配置快照（`config/appearance.json` 的合并结果，JSON 对象）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_appearance_config() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(appearance::config_json(&root))
}

/// 合并写入外观配置（只覆盖传入的键），返回 `{"ok":bool,"error":"…"}`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_set_appearance_config(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&read_arg(json)) else {
        return to_c_string(
            serde_json::json!({ "ok": false, "error": "配置格式非法" }).to_string(),
        );
    };
    match appearance::save_config_patch(&root, &value) {
        Ok(_) => to_c_string(serde_json::json!({ "ok": true, "error": "" }).to_string()),
        Err(reason) => to_c_string(serde_json::json!({ "ok": false, "error": reason }).to_string()),
    }
}

/// 弹出系统文件对话框选背景图，返回选中的绝对路径（取消 / 失败 = 空串）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_appearance_pick_bg() -> *mut c_char {
    to_c_string(appearance::pick_bg_image())
}

/// 开机自启状态（JSON：`enabled` / `mechanism` / `stale` / `dev` / `supported`）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_auto_start_info() -> *mut c_char {
    to_c_string(auto_start::info_json())
}

/// 开启 / 关闭开机自启（`enabled` 非 0 = 开启）。
///
/// 返回空串 = 已生效；否则是**一句人话原因**（开发构建 / 权限不足 / 用户取消等），
/// 调用方据此把开关弹回原状态并显示原因。
#[no_mangle]
pub extern "C" fn lunac_auto_start_set(enabled: i32) -> *mut c_char {
    match auto_start::set_enabled(enabled != 0) {
        Ok(()) => to_c_string(String::new()),
        Err(reason) => {
            eprintln!("[lunac-autostart] 失败: {reason}");
            to_c_string(reason)
        }
    }
}

/// 提权子进程入口（Dart `main()` 在建窗之前调用，随后以返回值作为退出码）。
#[no_mangle]
pub extern "C" fn lunac_auto_start_elevated(action: *const c_char) -> i32 {
    auto_start::handle_elevated(&read_arg(action))
}

/// 读剪贴板历史（JSON 数组；字段与 v1 相同：`clip_type` / `text` / `file_paths` / `time`）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_clipboard_history() -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string("[]".to_string());
    };
    to_c_string(clipboard_history::load_json(&root))
}

/// 记一条剪贴板历史（去重 → 置顶 → 最多 30 条；空串 / 超长静默忽略）。
///
/// 返回 0 = 已处理，2 = 写失败。
#[no_mangle]
pub extern "C" fn lunac_clipboard_history_append(text: *const c_char) -> i32 {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return 2;
    };
    match clipboard_history::append(&root, &read_arg(text)) {
        Ok(()) => 0,
        Err(reason) => {
            eprintln!("[lunac-clip] 记录失败: {reason}");
            2
        }
    }
}

/// 把一条已存在的记录提到最前（面板点击复制后调用）。
///
/// 返回 0 = 已处理，2 = 写失败。
#[no_mangle]
pub extern "C" fn lunac_clipboard_history_promote(text: *const c_char) -> i32 {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return 2;
    };
    match clipboard_history::promote(&root, &read_arg(text)) {
        Ok(()) => 0,
        Err(reason) => {
            eprintln!("[lunac-clip] 置顶失败: {reason}");
            2
        }
    }
}

/// 清空剪贴板历史（调用方负责二次确认）。
///
/// 返回 0 = 已清空，2 = 写失败。
#[no_mangle]
pub extern "C" fn lunac_clipboard_history_clear() -> i32 {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return 2;
    };
    match clipboard_history::clear(&root) {
        Ok(()) => 0,
        Err(reason) => {
            eprintln!("[lunac-clip] 清空失败: {reason}");
            2
        }
    }
}

/// 读全部备忘录（JSON 数组；字段同 v1：`id` / `text` / `tag` / `images` / `ts`）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_memo_list() -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string("[]".to_string());
    };
    to_c_string(memo::load_json(&root))
}

/// 整体保存备忘录（清空全部 / 批量写入）。
///
/// 返回 0 = 已保存，1 = JSON 不是数组，2 = 写失败。
#[no_mangle]
pub extern "C" fn lunac_memo_save(entries: *const c_char) -> i32 {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return 2;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&read_arg(entries)) else {
        return 1;
    };
    match memo::save_json(&root, &value) {
        Ok(()) => 0,
        Err(reason) => {
            eprintln!("[lunac-memo] 保存失败: {reason}");
            if reason.contains("数组") {
                1
            } else {
                2
            }
        }
    }
}

/// 新增 / 覆盖一条备忘录（按 `id`；新条目插到最前）。
///
/// 返回 0 = 已保存，1 = 缺 id / text，2 = 写失败。
#[no_mangle]
pub extern "C" fn lunac_memo_upsert(entry: *const c_char) -> i32 {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return 2;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&read_arg(entry)) else {
        return 1;
    };
    match memo::upsert(&root, &value) {
        Ok(()) => 0,
        Err(reason) => {
            eprintln!("[lunac-memo] 写入失败: {reason}");
            if reason.contains("缺") {
                1
            } else {
                2
            }
        }
    }
}

/// 删除一条备忘录；不存在也算成功。返回 0 = 已处理，2 = 写失败。
#[no_mangle]
pub extern "C" fn lunac_memo_delete(id: *const c_char) -> i32 {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return 2;
    };
    match memo::delete(&root, &read_arg(id)) {
        Ok(()) => 0,
        Err(reason) => {
            eprintln!("[lunac-memo] 删除失败: {reason}");
            2
        }
    }
}

/// 清空全部备忘录（调用方负责二次确认）。返回 0 = 已清空，2 = 写失败。
#[no_mangle]
pub extern "C" fn lunac_memo_clear() -> i32 {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return 2;
    };
    match memo::clear(&root) {
        Ok(()) => 0,
        Err(reason) => {
            eprintln!("[lunac-memo] 清空失败: {reason}");
            2
        }
    }
}

/// 把剪贴板里的图片存进某条备忘录的图片目录（返回它的绝对路径）。
///
/// 入参 = 备忘录 id；返回 JSON：`{ "ok": bool, "path": "…", "error": "…" }`。
/// 图片统一落成 24 位 BMP（避免 32 位 DIB 的 alpha 让 Flutter 当成全透明）。
/// 面板负责再把 `path` 追加进该条目的 `images` 后写盘。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_memo_add_clipboard_image(id: *const c_char) -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string(
            json!({ "ok": false, "path": "", "error": "找不到数据根目录" }).to_string(),
        );
    };
    match memo::save_clipboard_image(&root, &read_arg(id)) {
        Ok(path) => to_c_string(json!({ "ok": true, "path": path, "error": "" }).to_string()),
        Err(reason) => {
            eprintln!("[lunac-memo] 粘贴图片失败: {reason}");
            to_c_string(json!({ "ok": false, "path": "", "error": reason }).to_string())
        }
    }
}

/// 读全部自定义工具定义（`<exe 根>/tools/*.json`，v1 `list_tool_files` 的同口径）。
///
/// 返回 JSON：`{ "entries": [...], "error": "…" }`；坏 JSON / 缺字段只把那一份标成
/// `valid: false`，目录读不了时 `entries` 为空并给出原因，**不 panic**。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_mcp_config() -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(mcp_config::load_json(&root))
}

/// 写入一条工具定义 / 删除一条（v1 `save_tool_file` / `delete_tool_file` 的等价物）。
///
/// 入参 JSON：`{ "action": "save", "filename"?, "content" }` 或
/// `{ "action": "delete", "filename" }`。返回 JSON：`{ "ok": bool, "error": "…" }`。
/// 写盘用临时文件 + rename；文件名只允许 `tools` 目录下的裸 `.json`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_mcp_config_save(request: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let result = match serde_json::from_str::<serde_json::Value>(&read_arg(request)) {
        Ok(value) => mcp_config::save_json(&root, &value),
        Err(e) => Err(format!("请求不是合法 JSON：{e}")),
    };
    match result {
        Ok(()) => to_c_string(json!({ "ok": true, "error": "" }).to_string()),
        Err(reason) => {
            eprintln!("[lunac-tools] 保存失败: {reason}");
            to_c_string(json!({ "ok": false, "error": reason }).to_string())
        }
    }
}

/// 远端 MCP 组入口（§待办 3-31 ①）：返回 `<exe 根>/config/mcp.json` 的绝对路径，
/// 文件不存在就先落一份空模板 `{"servers": []}`（已存在则一个字节都不动）。
///
/// 返回 `{ ok, path, error }`；界面拿到路径后交给系统默认编辑器打开。
#[no_mangle]
pub extern "C" fn lunac_mcp_config_file() -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match mcp_config::ensure_remote_file(&root) {
        Ok(path) => to_c_string(
            json!({ "ok": true, "path": path.to_string_lossy(), "error": "" }).to_string(),
        ),
        Err(reason) => {
            eprintln!("[lunac-tools] 准备 mcp.json 失败: {reason}");
            to_c_string(json!({ "ok": false, "path": "", "error": reason }).to_string())
        }
    }
}

/// 列出 `<exe 根>/skills` 下的技能（v1「设置 · AI · 技能」组的读半边）。
///
/// 返回 JSON：`{ "entries": [...], "error": "…" }`；`entries[]` 字段
/// `key` / `name` / `description` / `fork` / `bytes` / `valid`。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_skills() -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(skills_admin::list_json(&root))
}

/// 读一份技能的 `SKILL.md` 原文：入参是 key，返回 `{ ok, key, content, error }`。
#[no_mangle]
pub extern "C" fn lunac_skills_read(key: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(skills_admin::read_json(&root, &read_arg(key)))
}

/// 写入 / 覆盖一份技能：入参 `{ "key": "…", "content": "…" }`，返回 `{ ok, key, error }`。
///
/// `key` 只允许 `[A-Za-z0-9_-]`；写入走临时文件 + rename。
#[no_mangle]
pub extern "C" fn lunac_skills_save(request: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match serde_json::from_str::<serde_json::Value>(&read_arg(request)) {
        Ok(value) => to_c_string(skills_admin::save_json(&root, &value)),
        Err(error) => to_c_string(
            json!({ "ok": false, "key": "", "error": format!("请求不是合法 JSON：{error}") })
                .to_string(),
        ),
    }
}

/// 扫一份技能正文（安装前的安全闸，§待办 3-28.9）：入参是 `SKILL.md` 原文，
///
/// 返回 `{ risk: "safe"|"warn"|"danger", bytes, findings:[{code,severity,line,excerpt,why}] }`。
/// 判定只有这一处（安装流程与界面上的「检查」按钮共用）。
#[no_mangle]
pub extern "C" fn lunac_skill_scan(content: *const c_char) -> *mut c_char {
    to_c_string(skill_scan::scan_json(&read_arg(content)))
}

/// 删除一份技能（整个 `skills/<key>` 目录）：入参是 key，返回 `{ ok, key, error }`。
#[no_mangle]
pub extern "C" fn lunac_skills_delete(key: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(skills_admin::delete_json(&root, &read_arg(key)))
}

/// 「精选」技能索引 + 与已装技能的比对（§待办 3-28.9 ①）。
///
/// 返回 `{ ok, version, updated, entries[], error }`；每条 `status` 是
/// `installed` / `outdated` / `missing`。清单来自随包分发的 `skills/curated.json`。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_skills_curated() -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(skills_admin::curated_json(&root))
}

/// 从「精选」安装 / 修复一份技能：入参 `{ key, overwrite, acceptScan, acceptPermissions }`。
///
/// 与 URL 安装走**同一条**安装闸（哈希 / 危险扫描 / 权限 diff / 审计），只是来源换成
/// 随包只读镜像（不联网）。回执形状与 `lunac_skills_install` 完全一致。
#[no_mangle]
pub extern "C" fn lunac_skills_install_curated(request: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match serde_json::from_str::<serde_json::Value>(&read_arg(request)) {
        Ok(value) => to_c_string(skills_admin::install_curated_json(&root, &value)),
        Err(error) => to_c_string(
            json!({ "ok": false, "key": "", "error": format!("请求不是合法 JSON：{error}") })
                .to_string(),
        ),
    }
}

/// 从 raw URL 安装一份技能：入参 `{ "key": "…", "url": "https://…", "overwrite": bool }`。
///
/// 返回 `{ ok, key, bytes, name, exists, scan, scanBlocked, error }`；`exists=true` 表示同名
/// 技能已存在、等用户确认覆盖后再带 `overwrite=true` 重发。只收 `https://`，下载时就地卡
/// 128 KiB。**安装前过安全扫描**（`skill_scan`）：命中 `danger` 时回 `scanBlocked=true`
/// 且不落盘，用户看过检查结果后带 `acceptScan=true` 重发才装。
#[no_mangle]
pub extern "C" fn lunac_skills_install(request: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match serde_json::from_str::<serde_json::Value>(&read_arg(request)) {
        Ok(value) => to_c_string(skills_admin::install_json(&root, &value)),
        Err(error) => to_c_string(
            json!({
                "ok": false,
                "key": "",
                "bytes": 0,
                "name": "",
                "exists": false,
                "error": format!("请求不是合法 JSON：{error}"),
            })
            .to_string(),
        ),
    }
}

/// 读取一份工具定义的原始 JSON 文本（v1 `read_tool_file`）。
///
/// 返回 JSON：`{ "ok": bool, "content": "…", "error": "…" }`。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_tool_read(filename: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(mcp_config::read_file_json(&root, &read_arg(filename)))
}

/// 翻译词典底座（缓存 → MyMemory + 有道）。
///
/// 返回 JSON 字符串：成功时字段与 v1 `TranslateResult` 一致
/// （`text` / `from` / `to` / `source` / `cached` / `translation` / `phonetic` /
/// `explains` / `examples[]`）；失败时是 `{"error": "…"}`（空输入 / 超长 / 网络组件不可用）。
/// 联网失败按 v1 口径降级为 `source: "none"`，**不会 panic**。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_translate_lookup(
    text: *const c_char,
    from: *const c_char,
    to: *const c_char,
) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match translate::lookup(&root, &read_arg(text), &read_arg(from), &read_arg(to)) {
        Ok(result) => to_c_string(serde_json::to_string(&result).unwrap_or_else(|_| {
            r#"{"error":"译文序列化失败"}"#.to_string()
        })),
        Err(reason) => {
            eprintln!("[lunac-translate] 词典查询失败: {reason}");
            to_c_string(translate_error_json(&reason))
        }
    }
}

/// 翻译模型补漏（**会花钱**，只在用户点按钮时调用；复用 `config/ai.json`）。
///
/// 返回形状与 [`lunac_translate_lookup`] 相同；失败时 `{"error": "…"}`，
/// 文案只说「哪一步没成 + 用户能做什么」。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_translate_ai(
    text: *const c_char,
    from: *const c_char,
    to: *const c_char,
) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match translate::ai(&root, &read_arg(text), &read_arg(from), &read_arg(to)) {
        Ok(result) => to_c_string(serde_json::to_string(&result).unwrap_or_else(|_| {
            r#"{"error":"译文序列化失败"}"#.to_string()
        })),
        Err(reason) => {
            eprintln!("[lunac-translate] AI 补漏失败: {reason}");
            to_c_string(translate_error_json(&reason))
        }
    }
}

/// 文件转换计划：`kind` / `targets[]` / `error` / `engine` / `engineFound`（JSON 对象）。
///
/// `kind` = image / audio / video / pdf / unknown；`targets` 已去掉源扩展名本身；
/// 后缀不支持时 `targets` 为空且 `error` 是一句人话；找不到引擎不算不能转，
/// 只把 `engineFound` 置 false（面板据此禁用按钮并显示装法）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_convert_plan(path: *const c_char) -> *mut c_char {
    let plan = convert::plan(&read_arg(path));
    to_c_string(serde_json::to_string(&plan).unwrap_or_else(|_| {
        r#"{"kind":"unknown","targets":[],"error":"转换计划序列化失败","engine":"","engineFound":false}"#
            .to_string()
    }))
}

/// 执行一次文件转换：`ok` / `outputPath` / `error`（JSON 对象）。
///
/// 阻塞到子进程结束（分钟级），Dart 侧必须放进后台 isolate；
/// 失败不 panic，`error` 里带引擎 stderr 摘要（最多 500 字符）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_convert_run(
    src: *const c_char,
    target: *const c_char,
    out_dir: *const c_char,
) -> *mut c_char {
    match convert::run(&read_arg(src), &read_arg(target), &read_arg(out_dir)) {
        Ok(output) => to_c_string(
            serde_json::json!({ "ok": true, "outputPath": output, "error": "" }).to_string(),
        ),
        Err(reason) => {
            eprintln!("[lunac-convert] 转换失败: {reason}");
            to_c_string(
                serde_json::json!({ "ok": false, "outputPath": "", "error": reason }).to_string(),
            )
        }
    }
}

/// 当前转换进度：`running` / `percent`（`-1` = 不知道总时长）/ `totalMs` / `cancelled`。
///
/// Dart 侧每 300ms 轮询一次（C ABI 没有事件总线）；只读内存状态，不阻塞。
#[no_mangle]
pub extern "C" fn lunac_convert_progress() -> *mut c_char {
    to_c_string(convert::progress_json())
}

/// 取消当前转换（按 pid 杀 ffmpeg 进程树 + 删半成品）。没有在跑时是 no-op。
#[no_mangle]
pub extern "C" fn lunac_convert_cancel() -> *mut c_char {
    match convert::cancel() {
        Ok(()) => to_c_string(serde_json::json!({ "ok": true, "error": "" }).to_string()),
        Err(reason) => to_c_string(serde_json::json!({ "ok": false, "error": reason }).to_string()),
    }
}

/// 按页签弹多选文件框（v1 转换窗口的「点击输入框选文件」）。
///
/// 入参是页签 kind（`pdf` / `video` / `extract` / `audio`）；返回 JSON 数组。
/// 取消 = 空数组。返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_convert_pick_files(kind: *const c_char) -> *mut c_char {
    let files = convert::pick_inputs(&read_arg(kind));
    to_c_string(serde_json::to_string(&files).unwrap_or_else(|_| "[]".to_string()))
}

/// 弹系统的「选择文件夹」对话框（转换保存目录）。返回路径字符串；取消 = 空串。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_convert_pick_out_dir() -> *mut c_char {
    to_c_string(convert::pick_out_dir())
}

/// OCR 引擎状态：`installed` / `enginePath` / `searchPaths`（JSON 对象字符串）。
///
/// 面板据此决定「开始识别」是否可用，并在未安装时列出搜索过的路径。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_ocr_status() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(ocr::status_json(&root))
}

/// OCR 单图识别：起 PaddleOCR-json 子进程，返回 `text` / `lines[]` / `error`。
///
/// `text` = 按行合并的结果；`error` 为空 = 成功（可能图里没有文字）。
/// 失败不 panic，只把「哪一步没成 + 用户能做什么」放进 `error`。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_ocr_image(path: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match ocr::recognize_image(&root, read_arg(path).trim(), "chs") {
        Ok(lines) => to_c_string(
            serde_json::json!({
                "text": lines.join("\n"),
                "lines": lines,
                "error": serde_json::Value::Null,
            })
            .to_string(),
        ),
        Err(reason) => {
            eprintln!("[lunac-ocr] 识别失败: {reason}");
            to_c_string(
                serde_json::json!({
                    "text": "",
                    "lines": Vec::<String>::new(),
                    "error": reason,
                })
                .to_string(),
            )
        }
    }
}

/// OCR「选择文件」：系统单选框（图片过滤）。返回路径；取消 = 空串。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_ocr_pick_image() -> *mut c_char {
    to_c_string(ocr::pick_image())
}

/// OCR「截屏识别」：拉起系统截图浮层并等剪贴板出现新图（阻塞，最长 `timeout_ms`）。
///
/// 返回 JSON：`{"path":"...","error":""}`。要等用户框选，Dart 侧必须放进后台 isolate。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_ocr_capture_screen(timeout_ms: u64) -> *mut c_char {
    let timeout = std::time::Duration::from_millis(timeout_ms.clamp(5_000, 120_000));
    to_c_string(ocr::capture_screen(timeout))
}

/// 本地音乐：添加 / 移除曲库目录 + 选择目录。
///
/// v1 的「本地音乐」用 `media_lib.rs` 的 `roots` 表管理曲库（`<exe 根>/ModuleData/music/media.db`）；
/// v2 的读写实现已在 `music.rs`，这里只是把它交给面板（此前没有入口）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_music_add_root(path: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match music::add_root(&root, &read_arg(path)) {
        Ok(normalized) => {
            to_c_string(json!({ "ok": true, "path": normalized, "error": "" }).to_string())
        }
        Err(reason) => to_c_string(json!({ "ok": false, "path": "", "error": reason }).to_string()),
    }
}

/// 移除曲库目录（幂等）。返回 JSON：`{ ok, error }`。
#[no_mangle]
pub extern "C" fn lunac_music_remove_root(path: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match music::remove_root(&root, &read_arg(path)) {
        Ok(()) => to_c_string(json!({ "ok": true, "error": "" }).to_string()),
        Err(reason) => to_c_string(json!({ "ok": false, "error": reason }).to_string()),
    }
}

/// 弹系统的「选择文件夹」（曲库目录）。取消 = 空串。
#[no_mangle]
pub extern "C" fn lunac_music_pick_root() -> *mut c_char {
    to_c_string(convert::pick_dir_with_title("选择音乐曲库目录"))
}

/// 弹系统的「选择文件夹」（AI 工作区）。取消 = 空串。
///
/// 与曲库目录共用 `convert::pick_dir_with_title`（同一个 shell32 手写 ABI）——
/// 不为了选目录再抄一份对话框代码（`forbidden.md` §禁用 62）。
#[no_mangle]
pub extern "C" fn lunac_pick_workspace() -> *mut c_char {
    to_c_string(convert::pick_dir_with_title("选择工作区目录"))
}

/// 工作区指纹（命令类产出识别，§待办 3-28.13 ②）。返回
/// `{ok, truncated, entries:{相对路径:"大小:改动ms"}}`；抓帧只看元数据、不读内容。
#[no_mangle]
pub extern "C" fn lunac_workspace_fingerprint() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(sandbox::workspace_fingerprint_json(&root))
}

/// 本地音乐媒体库（JSON：`tracks[]` / `roots[]` / `error`）。
///
/// 扫描 v1 用户添加过的曲库目录（`<exe 根>\ModuleData\music\media.db` 的 `roots` 表）
/// 与系统音乐目录；曲目含标题 / 艺术家 / 专辑 / 时长 / 路径 / 缓存封面绝对路径。
/// 可能耗秒级（读标签），Dart 侧必须放进后台 isolate。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_music_library() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(music::library_json(&root))
}

/// 同名 `.lrc` 歌词（JSON：`lines[]`（`timeMs` / `text`）/ `error`）。
///
/// 入参是**音频文件路径**；同目录同名 `.lrc` 存在时解析，不存在时 `lines` 为空
/// 且 `error` 为 null（面板显示「未找到歌词」）。编码容错见 `music::decode_lyrics_bytes`。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_music_lyrics(path: *const c_char) -> *mut c_char {
    to_c_string(music::lyrics_json(&read_arg(path)))
}

/// 在线歌词（LRCLIB → LRCLIB 搜索 → 网易云兜底，与 v1 三级口径一致）。
///
/// 入参 JSON：`{"title":"…","artist":"…","album":"…","duration":238.0}`（专辑 / 时长可省）。
/// 返回 JSON：`found / instrumental / source / track / artist / album / duration /
/// lines[]（timeMs, text，直接复用本地 LRC 的时间轴口径）/ plain / error`。
///
/// 会**发网络请求**（超时 8s）：Dart 侧在 worker isolate 里调，不冻 UI。
/// 三级全落空是 `found:false` + `error:null` —— 那是「没找到歌词」，不是错误。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_music_lyrics_online(json: *const c_char) -> *mut c_char {
    to_c_string(music_online::online_lyrics_json(&read_arg(json)))
}

/// Spotify 在线库状态：是否已连接 / 账号名 / 429 硬闸 / 回调地址。
#[no_mangle]
pub extern "C" fn lunac_music_spotify_state() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(music_spotify::state_json(&root))
}

/// 开始 Spotify 授权（PKCE）：起环回回调 + 打开浏览器，立即返回授权链接。
///
/// 结果由面板轮询 `lunac_music_spotify_state` 的 `connected` / `pending` 得到。
#[no_mangle]
pub extern "C" fn lunac_music_spotify_connect() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(music_spotify::connect_json(&root))
}

/// 断开 Spotify（清空本地令牌；`config/music.json` 里的其它键保留）。
#[no_mangle]
pub extern "C" fn lunac_music_spotify_disconnect() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(music_spotify::disconnect_json(&root))
}

/// Spotify Web API 通用调用：`{action, …}` → `{ok, error, data}`。
///
/// `action`：`status` / `playlists` / `playlist_tracks` / `queue` / `liked` / `devices` /
/// `search` / `control` / `play` / `transfer` / `resume`。
/// **会发网络请求**（超时 15s）：Dart 侧在 worker isolate 里调。
#[no_mangle]
pub extern "C" fn lunac_music_spotify_call(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(music_spotify::call_json(&root, &read_arg(json)))
}

/// 保存「自己的 Client ID / 回调端口」：入参 `{clientId?, port?}` → `{ok,error}`。
///
/// 两个字段都是可选的（缺省 = 别动）；换 Client ID 或端口会清掉旧令牌。
#[no_mangle]
pub extern "C" fn lunac_music_spotify_config(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&read_arg(json)) else {
        return to_c_string(
            serde_json::json!({ "ok": false, "error": "参数不是合法 JSON" }).to_string(),
        );
    };
    let client_id = value.get("clientId").and_then(|v| v.as_str());
    let port = match value.get("port") {
        None | Some(serde_json::Value::Null) => None,
        Some(item) => match item.as_u64().and_then(|n| u16::try_from(n).ok()) {
            Some(n) => Some(n),
            None => {
                return to_c_string(
                    serde_json::json!({ "ok": false, "error": "端口不合法（要 1024–65535）" })
                        .to_string(),
                )
            }
        },
    };
    match music_spotify::save_settings(&root, client_id, port) {
        Ok(()) => to_c_string(serde_json::json!({ "ok": true, "error": "" }).to_string()),
        Err(reason) => to_c_string(
            serde_json::json!({ "ok": false, "error": reason }).to_string(),
        ),
    }
}

/// 自定义队列（`config/music-queues.json`）：`queues[]` / `error`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_music_queues() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(music::queues_json(&root))
}

/// 覆盖写全部自定义队列（入参 `{"queues":[{id,name,paths}]}`）。
///
/// 返回 `{ok,error,queues[]}`（收敛后的队列回给面板直接采用）。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_music_save_queues(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match music::save_queues(&root, &read_arg(json)) {
        Ok(queues) => to_c_string(
            serde_json::json!({ "ok": true, "error": "", "queues": queues }).to_string(),
        ),
        Err(reason) => to_c_string(
            serde_json::json!({ "ok": false, "error": reason, "queues": [] }).to_string(),
        ),
    }
}

/// 导入 `.m3u/.m3u8/.pls`：返回 `{ok,canceled,name,paths[],error}`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_music_import_playlist() -> *mut c_char {
    to_c_string(music::import_playlist_json())
}

/// 弹出多选框选音频文件（新建队列）：返回 `{ok,paths[],error}`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_music_pick_files() -> *mut c_char {
    to_c_string(music::pick_audio_files_json())
}

// ── 本地媒体库的 v1 形态（SQLite + 后台扫描线程，`media_lib.rs`）──
//
// 面板打开时只查库（不重新解析几千个文件的标签）；扫描是后台线程 + 进度轮询；
// 删曲目连索引与封面缓存一起删。库文件与表结构**与 v1 同一份**。

/// 曲库目录列表（含每个目录的曲目数与「目录还在不在」）：`{ok,error,roots[]}`。
#[no_mangle]
pub extern "C" fn lunac_media_roots() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(media_lib::roots_json(&root))
}

/// 添加曲库目录（**不立刻扫描**）：`{ok,error,roots[]}`。
#[no_mangle]
pub extern "C" fn lunac_media_add_root(path: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(media_lib::add_root_json(&root, &read_arg(path)))
}

/// 移除曲库目录（连同它的曲目）；扫描进行中拒绝（`ERR_SCAN_RUNNING`）。
#[no_mangle]
pub extern "C" fn lunac_media_remove_root(path: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(media_lib::remove_root_json(&root, &read_arg(path)))
}

/// 开始扫描（后台线程，立刻返回）。入参空串 = 扫全部。
#[no_mangle]
pub extern "C" fn lunac_media_scan_start(root_path: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(media_lib::scan_start_json(&root, &read_arg(root_path)))
}

/// 扫描进度（界面 500ms 轮询一次；不扫时拿到上一次的结果）。
#[no_mangle]
pub extern "C" fn lunac_media_scan_status() -> *mut c_char {
    to_c_string(media_lib::scan_status_json())
}

/// 查曲目：`{ok,error,total,items[]}`（`total` 不受 `limit` 影响）。
#[no_mangle]
pub extern "C" fn lunac_media_tracks(
    root_path: *const c_char,
    query: *const c_char,
    limit: i32,
) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let limit = if limit <= 0 { 1000 } else { limit as u32 };
    to_c_string(media_lib::tracks_json(
        &root,
        &read_arg(root_path),
        &read_arg(query),
        limit,
    ))
}

/// 从库里删除单个文件（磁盘文件 + 索引 + 封面缓存）：`{ok,error}`。
#[no_mangle]
pub extern "C" fn lunac_media_delete_file(path: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(media_lib::delete_file_json(&root, &read_arg(path)))
}

/// 弹「打开」选一份播放列表（`.m3u/.m3u8/.pls`）：`{ok,path}`（空串 = 取消）。
#[no_mangle]
pub extern "C" fn lunac_media_pick_playlist() -> *mut c_char {
    to_c_string(media_lib::pick_playlist_json())
}

/// 读一份播放列表文件的曲目（**不入库**）：`{ok,error,name,total,items[]}`。
#[no_mangle]
pub extern "C" fn lunac_media_playlist(path: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(media_lib::playlist_json(&root, &read_arg(path)))
}

// ── 调音（v1 `player.rs` 的控制面；DSP 内核直接用 `tuning-engine`）──────────
//
// 全部返回 `{ok, error, data}`；`data` 是同一份 `TuningDto`（含频响曲线）。
// `data` 里字段为 camelCase（`preampDb` / `bassDb` / `filter.freq_hz` 除外 ——
// 滤波器段与磁盘 / 引擎同形，保持 snake_case，避免三层各翻一次）。

/// 读调音状态（配置 + 用户预设 + 频响曲线）。
#[no_mangle]
pub extern "C" fn lunac_tuning_get() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(tuning::get_json(&root))
}

/// 只改总开关（不重置链）。`enabled != 0` = 开。
#[no_mangle]
pub extern "C" fn lunac_tuning_set_enabled(enabled: i32) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(tuning::set_enabled_json(&root, enabled != 0))
}

/// 整条链重置成某条预设，并设置开关（v1 `player_tuning_set`）。
#[no_mangle]
pub extern "C" fn lunac_tuning_set_preset(enabled: i32, preset: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(tuning::set_preset_json(&root, enabled != 0, &read_arg(preset)))
}

/// 把一条预设整条展开进来（开关不动）。
#[no_mangle]
pub extern "C" fn lunac_tuning_apply_preset(preset: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(tuning::apply_preset_json(&root, &read_arg(preset)))
}

/// 覆盖整条链（入参 camelCase JSON，`preset` 变 `custom`；合法性由引擎校验）。
#[no_mangle]
pub extern "C" fn lunac_tuning_set_chain(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(tuning::set_chain_json(&root, &read_arg(json)))
}

/// 把当前链存成用户预设（`overwrite != 0` 才允许覆盖同名）。
#[no_mangle]
pub extern "C" fn lunac_tuning_preset_save(name: *const c_char, overwrite: i32) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(tuning::preset_save_json(&root, &read_arg(name), overwrite != 0))
}

#[no_mangle]
pub extern "C" fn lunac_tuning_preset_delete(name: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(tuning::preset_delete_json(&root, &read_arg(name)))
}

#[no_mangle]
pub extern "C" fn lunac_tuning_preset_rename(from: *const c_char, to: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(tuning::preset_rename_json(&root, &read_arg(from), &read_arg(to)))
}

/// 导入预设文件（**导入即应用**）。
#[no_mangle]
pub extern "C" fn lunac_tuning_preset_import(path: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(tuning::preset_import_json(&root, &read_arg(path)))
}

/// 导出当前链到预设文件。
#[no_mangle]
pub extern "C" fn lunac_tuning_preset_export(path: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(tuning::preset_export_json(&root, &read_arg(path)))
}

/// A / B 两槽各自有没有快照。
#[no_mangle]
pub extern "C" fn lunac_tuning_ab_state() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(tuning::ab_state_json(&root))
}

#[no_mangle]
pub extern "C" fn lunac_tuning_ab_save(slot: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(tuning::ab_save_json(&root, &read_arg(slot)))
}

#[no_mangle]
pub extern "C" fn lunac_tuning_ab_apply(slot: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(tuning::ab_apply_json(&root, &read_arg(slot)))
}

/// 弹「打开」选一个预设文件：`{ok,canceled,path,error}`（取消不是错误）。
#[no_mangle]
pub extern "C" fn lunac_tuning_preset_pick() -> *mut c_char {
    to_c_string(tuning::pick_preset_json())
}

/// 弹「另存为」选导出位置：`{ok,canceled,path,error}`。
#[no_mangle]
pub extern "C" fn lunac_tuning_preset_pick_save() -> *mut c_char {
    to_c_string(tuning::pick_export_json())
}

/// 弹「打开」选卷积脉冲响应（WAV）：`{ok,canceled,path,error}`。
#[no_mangle]
pub extern "C" fn lunac_tuning_pick_ir() -> *mut c_char {
    to_c_string(tuning::pick_ir_json())
}

/// **实时测量**一次「最终送到硬件的频响」（WASAPI 回环，约 6 秒，**会出声**）。
///
/// 结果 `data`：`{sampleRate,freqs,db,chainDb,refDb,coverage,elapsedMs}`
/// （`chainDb` = 仅自身链，**这条**才该跟合成曲线对齐）。
/// 这是阻塞调用（两趟各 ~2 秒）⇒ Dart 侧必须放 `Isolate.run`。
#[no_mangle]
pub extern "C" fn lunac_tuning_measure() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match tuning_probe::measure(&root) {
        Ok(data) => to_c_string(
            serde_json::json!({ "ok": true, "error": "", "data": data }).to_string(),
        ),
        Err(error) => to_c_string(
            serde_json::json!({ "ok": false, "error": error, "data": null }).to_string(),
        ),
    }
}

// ── 本地播放（v1 `player.rs` 的出声层；DSP 链挂在解码后、出声前）──
//
// 全部返回 `{ok, error, data}`，`data` 是同一份 `PlayerDto`
// （`active/playing/path/positionMs/durationMs/volume/video`）。
// 之前 v2 用 Dart 的 `just_audio` 出声 —— 那是**架构级缺口**：
// 调音链必须改样本流，样本在 Rust 侧才拿得到（见 player.rs 文件头）。

/// 播放一个本地文件（会先停掉上一首；调音链按当前配置自动挂上）。
///
/// `autoplay != 0` = 装好就开声；`0` = 装好但停在首帧（面板自己再 `resume`）。
#[no_mangle]
pub extern "C" fn lunac_player_play(path: *const c_char, autoplay: i32) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(player::play_json(&root, &read_arg(path), autoplay != 0))
}

#[no_mangle]
pub extern "C" fn lunac_player_pause() -> *mut c_char {
    to_c_string(player::pause_json())
}

#[no_mangle]
pub extern "C" fn lunac_player_resume() -> *mut c_char {
    to_c_string(player::resume_json())
}

#[no_mangle]
pub extern "C" fn lunac_player_stop() -> *mut c_char {
    to_c_string(player::stop_json())
}

/// 拖动进度（毫秒；越界由 `rodio` 饱和到 `[0, duration]`）。
///
/// seek 会顺带**复位调音链的滤波器状态**（否则旧样本的尾音会拖进新位置）。
#[no_mangle]
pub extern "C" fn lunac_player_seek(position_ms: u64) -> *mut c_char {
    to_c_string(player::seek_json(position_ms))
}

/// 音量（0..1，越界夹紧并记住，下一首沿用）。
#[no_mangle]
pub extern "C" fn lunac_player_volume(value: f64) -> *mut c_char {
    to_c_string(player::volume_json(value as f32))
}

/// 当前播放状态（面板轮询进度用；没有会话时也返回 `data`，只是 `active=false`）。
#[no_mangle]
pub extern "C" fn lunac_player_status() -> *mut c_char {
    to_c_string(player::status_json())
}

/// 代理状态：系统代理（是否开启 / 地址）/ 本地服务（未接入 / 端口）/ mihomo 内核
/// （是否可用 / 搜索路径）。JSON 对象字符串。
///
/// 本函数只**读**系统代理，不改任何设置。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_proxy_status() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(proxy::status_json(&root))
}

/// 开关 Windows 系统代理：`enabled != 0` = 开启（`server` 只在开启时使用）。
///
/// 返回 JSON：`ok` / `error` / `system`（失败时 `system` 仍是实际状态）。
/// 首次开启会备份系统原值，关闭时还原；没有备份时关闭是**空操作**并给出原因。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_proxy_system_set(enabled: i32, server: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(proxy::set_system_json(&root, enabled != 0, &read_arg(server)))
}

/// 启动自研代理引擎。入参 JSON：
/// `{"links":["ss://…","trojan://…"], "port":7890, "active":0, "mode":"rule",
///   "ruleMode":"blacklist", "ruleDomains":["baidu.com"], "rules":["MATCH,PROXY"]}`
/// （`links` 也可换成 `subscription` = 整段 base64 订阅）。
#[no_mangle]
pub extern "C" fn lunac_proxy_local_start(json: *const c_char) -> *mut c_char {
    ensure_data_root();
    to_c_string(native_proxy::start_json(&read_arg(json)))
}

/// 停止自研代理引擎。
#[no_mangle]
pub extern "C" fn lunac_proxy_local_stop() -> *mut c_char {
    ensure_data_root();
    to_c_string(native_proxy::stop_json())
}

/// 自研代理引擎状态（运行 / 端口 / 节点 / 跳过原因）。
#[no_mangle]
pub extern "C" fn lunac_proxy_local_status() -> *mut c_char {
    ensure_data_root();
    to_c_string(native_proxy::status_json())
}

/// 切换节点（运行中即刻生效）。
#[no_mangle]
pub extern "C" fn lunac_proxy_local_select(index: u32) -> *mut c_char {
    ensure_data_root();
    to_c_string(native_proxy::select_json(index as usize))
}

/// 节点时延（重建期为 TCP 连通耗时；`scope:"tcp"` 如实标注）。
#[no_mangle]
pub extern "C" fn lunac_proxy_local_delay(index: u32) -> *mut c_char {
    ensure_data_root();
    to_c_string(native_proxy::delay_json(index as usize))
}

/// 节点清单（面板展示用）。
#[no_mangle]
pub extern "C" fn lunac_proxy_nodes() -> *mut c_char {
    ensure_data_root();
    to_c_string(native_proxy::proxies_json())
}

/// 最近 `limit` 条请求日志（JSON 数组，按时间正序；面板每 2s 取 200 条）。
///
/// 每条：`{"host":"…","port":443,"route":"node|direct|rejected","detail":"订阅 · 香港 01"}`。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_proxy_requests(limit: u32) -> *mut c_char {
    ensure_data_root();
    let limit = if limit == 0 { 200 } else { limit as usize };
    to_c_string(native_proxy::requests_json(limit))
}

/// 切换模式（`rule` 规则模式 / `global` 全局代理）。运行中即刻生效。
#[no_mangle]
pub extern "C" fn lunac_proxy_local_mode(mode: *const c_char) -> *mut c_char {
    ensure_data_root();
    to_c_string(native_proxy::set_mode_json(&read_arg(mode)))
}

/// 出口地区（经当前节点取 `ipinfo.io/country`；失败如实报错）。
/// JSON：`{"ok":true,"country":"JP","error":""}`。
#[no_mangle]
pub extern "C" fn lunac_proxy_exit_info() -> *mut c_char {
    ensure_data_root();
    to_c_string(native_proxy::exit_json())
}

/// 自定义服务器字段 → 分享链接（面板的「服务器导入 → 自定义服务器」用）。
/// 入参 JSON：`{"kind":"ss|trojan|vless","name":…,"server":…,"port":…,
/// "password":…,"method":…,"uuid":…}`；返回 `{"ok","link","name","kind","error"}`。
#[no_mangle]
pub extern "C" fn lunac_proxy_custom_link(json: *const c_char) -> *mut c_char {
    ensure_data_root();
    to_c_string(native_proxy::build_custom_link_json(&read_arg(json)))
}

/// 已装插件清单（扫 `<exe 根>/Modules`）。JSON：`entries[]` / `dir` / `error`。
///
/// 坏包也会列出来（`valid: false` + `error` 原样带回）—— 否则用户只会看到插件
/// 「莫名其妙消失」。返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_market_installed() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = plugin_market::plugins_dir(&root);
    let entries = plugin_market::list_installed(&dir);
    to_c_string(
        json!({
            "entries": entries,
            "dir": dir.to_string_lossy(),
            "error": "",
        })
        .to_string(),
    )
}

/// 远端插件索引（https，只读）。JSON：`entries[]` / `error`。
///
/// 走网络，Dart 侧必须放进后台 isolate。返回字符串由 Rust 分配，
/// **调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_market_index() -> *mut c_char {
    let payload = match plugin_market::fetch_index() {
        Ok(entries) => json!({ "entries": entries, "error": "" }),
        Err(reason) => json!({ "entries": [], "error": reason }),
    };
    to_c_string(payload.to_string())
}

/// 从 https zip 安装插件（同 id = 重装 / 升级，失败回滚）。
///
/// 入参：`url`（必须公网 https）、`sha256`（空 = 不校验）。返回 JSON：
/// `ok` / `id` / `error`。走网络 + 解压，Dart 侧必须放进后台 isolate。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_market_install(url: *const c_char, sha256: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = plugin_market::plugins_dir(&root);
    let payload = match plugin_market::install_from_url(
        &read_arg(url),
        &read_arg(sha256),
        &dir,
    ) {
        Ok(id) => json!({ "ok": true, "id": id, "error": "" }),
        Err(reason) => json!({ "ok": false, "id": "", "error": reason }),
    };
    to_c_string(payload.to_string())
}

/// 卸载插件（递归删 `<exe 根>/Modules/<id>`）。返回 JSON：`ok` / `id` / `error`。
#[no_mangle]
pub extern "C" fn lunac_market_uninstall(id: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = plugin_market::plugins_dir(&root);
    let id_text = read_arg(id);
    let payload = match plugin_market::uninstall(&id_text, &dir) {
        Ok(()) => json!({ "ok": true, "id": id_text, "error": "" }),
        Err(reason) => json!({ "ok": false, "id": id_text, "error": reason }),
    };
    to_c_string(payload.to_string())
}

/// 当前这次安装的进度快照（`InstallState`，字段与 v1 同名同义）。
///
/// 不阻塞（只读一个内存里的快照）。Dart 侧在安装期间 500ms 轮询一次 ——
/// v2 没有 v1 的插件事件总线，轮询是同一份语义的实现方式。
#[no_mangle]
pub extern "C" fn lunac_market_install_state() -> *mut c_char {
    to_c_string(
        json!({
            "state": plugin_market::install_state(),
            "busy": plugin_market::install_busy(),
        })
        .to_string(),
    )
}

/// 磁盘上还剩哪些可续传的断点（`temp/downloads/*.part`）。
///
/// 不阻塞（只扫一个目录）。重启后界面靠它把「继续下载 (37%)」挂回对应的行。
#[no_mangle]
pub extern "C" fn lunac_market_pending_downloads() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = plugin_market::plugins_dir(&root);
    to_c_string(
        json!({ "entries": plugin_market::pending_downloads(&dir) }).to_string(),
    )
}

/// 暂停 / 继续当前这次下载。`paused != 0` = 暂停。幂等（没有活动下载时只置标志）。
#[no_mangle]
pub extern "C" fn lunac_market_install_pause(paused: i32) -> *mut c_char {
    plugin_market::set_install_paused(paused != 0);
    to_c_string(json!({ "ok": true, "paused": paused != 0 }).to_string())
}

/// 取消当前这次依赖下载（删 `.part`，整次安装由调用方回滚）。幂等。
#[no_mangle]
pub extern "C" fn lunac_market_cancel_dependency() -> *mut c_char {
    plugin_market::cancel_dependency_install();
    to_c_string(json!({ "ok": true }).to_string())
}

/// 给已装插件补装清单里的依赖（走网络 → Dart 侧必须放进后台 isolate）。
///
/// 返回 JSON：`ok` / `id` / `installed`（装了几条）/ `error`。
#[no_mangle]
pub extern "C" fn lunac_market_install_deps(id: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = plugin_market::plugins_dir(&root);
    let id_text = read_arg(id);
    let payload = match plugin_market::install_dependencies_for(&id_text, &dir) {
        Ok(count) => json!({ "ok": true, "id": id_text, "installed": count, "error": "" }),
        Err(reason) => json!({ "ok": false, "id": id_text, "installed": 0, "error": reason }),
    };
    to_c_string(payload.to_string())
}

// ── 插件运行时（sidecar）：让 AI 写的插件可以用任何语言 / 框架 ──────────────
//
// 契约见 `plugin_runtime.rs` 模块头；与 v1 `plugin_sidecar.rs` 同口径。
// 所有函数返回 JSON（Rust 分配，调用方 `lunac_free_string`）；
// `start` / `request` / `http` 会阻塞（起进程 / 等回包），Dart 侧必须放进后台 isolate。

/// sidecar 状态：清单 / 信任 / 运行 / 退出原因 / 事件游标。
#[no_mangle]
pub extern "C" fn lunac_plugin_runtime_status(id: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(plugin_runtime::status_json(&root, &read_arg(id)))
}

/// 起 sidecar。`owner` = 面板 / 窗口标识（关窗时按它收进程）。
///
/// 未信任时返回 `status:"needs_trust"` + 信任卡内容（命令 / 参数 / 校验和 / 授权端口）。
#[no_mangle]
pub extern "C" fn lunac_plugin_runtime_start(
    id: *const c_char,
    owner: *const c_char,
) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(plugin_runtime::start_json(
        &root,
        &read_arg(owner),
        &read_arg(id),
    ))
}

/// stdio 请求：`{"id":N,"method":…,"params":…}`，等回包（30s 超时）。
#[no_mangle]
pub extern "C" fn lunac_plugin_runtime_request(
    id: *const c_char,
    method: *const c_char,
    params_json: *const c_char,
) -> *mut c_char {
    to_c_string(plugin_runtime::request_json(
        &read_arg(id),
        &read_arg(method),
        &read_arg(params_json),
    ))
}

/// 宿主代请求 `127.0.0.1:<port>`（`port = 0` 用插件自己的端口）。
#[no_mangle]
pub extern "C" fn lunac_plugin_runtime_http(
    id: *const c_char,
    method: *const c_char,
    path: *const c_char,
    body: *const c_char,
    port: u16,
) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(plugin_runtime::http_json(
        &root,
        &read_arg(id),
        &read_arg(method),
        &read_arg(path),
        &read_arg(body),
        port,
    ))
}

/// 停止某个插件的 sidecar。
#[no_mangle]
pub extern "C" fn lunac_plugin_runtime_stop(id: *const c_char) -> *mut c_char {
    to_c_string(plugin_runtime::stop_json(&read_arg(id)))
}

/// 按 owner 收掉它起的全部 sidecar（面板 / 窗口关闭时调）。
#[no_mangle]
pub extern "C" fn lunac_plugin_runtime_stop_owner(owner: *const c_char) -> *mut c_char {
    to_c_string(plugin_runtime::stop_owner_json(&read_arg(owner)))
}

/// 信任一次（`remember != 0` 落盘，否则仅本会话）。
#[no_mangle]
pub extern "C" fn lunac_plugin_runtime_trust(
    id: *const c_char,
    remember: u8,
) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(plugin_runtime::trust_json(
        &root,
        &read_arg(id),
        remember != 0,
    ))
}

/// 拒绝（本会话内不再询问）。
#[no_mangle]
pub extern "C" fn lunac_plugin_runtime_deny(id: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(plugin_runtime::deny_json(&root, &read_arg(id)))
}

/// 拉取 `since` 之后的事件（面板每秒轮询）。
#[no_mangle]
pub extern "C" fn lunac_plugin_runtime_events(
    id: *const c_char,
    since: u64,
) -> *mut c_char {
    to_c_string(plugin_runtime::events_json(&read_arg(id), since))
}

/// 收掉全部 sidecar（应用退出前调）。
#[no_mangle]
pub extern "C" fn lunac_plugin_runtime_kill_all() -> *mut c_char {
    to_c_string(plugin_runtime::kill_all_json())
}

/// 读一个插件存储键：入参 id + key，返回 `{ ok, key, found, value, error }`。
///
/// 存储路径 `<exe 根>/ModuleData/plugins/<id>/kv.json`（v1 `plugin_kv` 的等价物）。
#[no_mangle]
pub extern "C" fn lunac_plugin_kv_get(
    id: *const c_char,
    key: *const c_char,
) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(plugin_kv::get_json(&root, &read_arg(id), &read_arg(key)))
}

/// 写一个插件存储键：入参 `{ "id": "…", "key": "…", "value": … }`。
///
/// 回执 `{ ok, key, bytes, error }`（`bytes` = 写完后的整份存储大小）。
/// 配额：单插件 256 KiB / 单值 64 KiB / 键数 256；超了拒绝且不动已有数据。
#[no_mangle]
pub extern "C" fn lunac_plugin_kv_set(request: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match serde_json::from_str::<serde_json::Value>(&read_arg(request)) {
        Ok(value) => to_c_string(plugin_kv::set_json(&root, &value)),
        Err(error) => to_c_string(
            json!({ "ok": false, "key": "", "bytes": 0, "error": format!("请求不是合法 JSON：{error}") })
                .to_string(),
        ),
    }
}

/// 删一个插件存储键：入参 id + key，返回 `{ ok, key, removed, bytes, error }`。
#[no_mangle]
pub extern "C" fn lunac_plugin_kv_delete(
    id: *const c_char,
    key: *const c_char,
) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(plugin_kv::delete_json(&root, &read_arg(id), &read_arg(key)))
}

/// 列出一个插件存储的全部键：`{ ok, entries:[{key,bytes}], bytes, error }`。
#[no_mangle]
pub extern "C" fn lunac_plugin_kv_list(id: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(plugin_kv::list_json(&root, &read_arg(id)))
}

/// 清空一个插件的存储（删掉整份 `kv.json`）：`{ ok, bytes, error }`。
#[no_mangle]
pub extern "C" fn lunac_plugin_kv_clear(id: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(plugin_kv::clear_json(&root, &read_arg(id)))
}

/// 插件开发工具链状态（轨道 H）：入参 `{ "plugin": "…" }`。
///
/// 回执 `{ ok, plugin, name, fingerprint, profiles[], build, running, granted, error }`；
/// `profiles[]` 每项带 `name / install / binDir / granted / installed / dir`。
/// 回执里**没有任何授权判定以外的语义**：`granted` 只表示「账本里有当前指纹的记录」。
#[no_mangle]
pub extern "C" fn lunac_plugin_toolchain_status(request: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    with_json(request, "plugin", |value| {
        plugin_toolchain::status_json(&root, value)
    })
}

/// 授权一个工具链 profile：`{ "plugin": "…", "profile": "node" }`。
///
/// 只有用户点「授权」才会调到这里（AI 只能改清单声明，不能自批）。回执 `{ ok, error }`。
#[no_mangle]
pub extern "C" fn lunac_plugin_toolchain_grant(request: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    with_json(request, "plugin", |value| {
        plugin_toolchain::grant_json(&root, value)
    })
}

/// 撤销授权并**立刻停掉**在跑的安装 / 构建：`{ "plugin": "…", "profile": "…"? }`。
///
/// `profile` 为空 = 撤销该插件全部。回执 `{ ok, removed, stopped, error }`。
#[no_mangle]
pub extern "C" fn lunac_plugin_toolchain_revoke(request: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    with_json(request, "plugin", |value| {
        plugin_toolchain::revoke_json(&root, value)
    })
}

/// 跑工具链的安装命令（需已授权）：`{ "plugin": "…", "profile": "…" }`。
///
/// 回执 `{ ok, bytes, steps:[{command,code,ok,timedOut,output}], error }`。
/// **同步执行**（npm 冷装较慢）：前端走 `Isolate.run`，别卡面板。
#[no_mangle]
pub extern "C" fn lunac_plugin_toolchain_install(request: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    with_json(request, "plugin", |value| {
        plugin_toolchain::install_json(&root, value)
    })
}

/// 跑插件的 `dev.build.command`（需已授权；**永不自动触发**）：`{ "plugin": "…" }`。
///
/// 回执 `{ ok, code, durationMs, timedOut, stopped, output, error }`。
#[no_mangle]
pub extern "C" fn lunac_plugin_toolchain_build(request: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    with_json(request, "plugin", |value| {
        plugin_toolchain::build_json(&root, value)
    })
}

/// 停止在跑的安装 / 构建（整棵进程树）：`{ "plugin": "…" }` → `{ ok, stopped, error }`。
#[no_mangle]
pub extern "C" fn lunac_plugin_toolchain_stop(request: *const c_char) -> *mut c_char {
    with_json(request, "plugin", |value| plugin_toolchain::stop_json(value))
}

/// 「入参是 JSON 对象」的薄壳：解析失败时回一句人话（不回显原始报文，
/// `docs/forbidden.md` §禁用 59）。
fn with_json(
    request: *const c_char,
    fallback_key: &str,
    run: impl FnOnce(&serde_json::Value) -> String,
) -> *mut c_char {
    match serde_json::from_str::<serde_json::Value>(&read_arg(request)) {
        Ok(value) => to_c_string(run(&value)),
        Err(error) => to_c_string(
            json!({
                "ok": false,
                (fallback_key): "",
                "error": format!("请求不是合法 JSON：{error}"),
            })
            .to_string(),
        ),
    }
}

/// 沙箱盘状态（`temp/transStorage` 的用量 / 固定额度 / 条目数）。
///
/// **它不是 OS 级隔离**：边界来自 agent 的启动参数（cwd + `--add-dir`），
/// 本函数只读地统计（见 `docs/sandbox-research.md`）。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_sandbox_status() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(sandbox::status_json(&root))
}

/// 沙箱盘清单（最多 4 层 / 500 条；目录在前）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_sandbox_list() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(sandbox::list_json(&root))
}

/// 只读预览沙箱盘里的一个文件（入参是相对路径；越界一律拒绝）。
///
/// 文本最多回 256 KiB（`truncated` 标明截断）；图片只回绝对路径交给界面渲染。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_sandbox_read(path: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(sandbox::read_json(&root, &read_arg(path)))
}

/// 清空沙箱盘（删全部子项、保留根目录；调用方负责二次确认）。
///
/// 返回 JSON：`ok` / `removed` / `failed` / `error`。
#[no_mangle]
pub extern "C" fn lunac_sandbox_clear() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(sandbox::clear_json(&root))
}

/// 设置沙箱盘的固定额度（64 MiB–8 GiB，夹取后写 `config/sandbox.json`）。
///
/// 返回 JSON：`ok` / `quotaBytes` / `error`。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_sandbox_set_quota(quota_bytes: u64) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(sandbox::set_quota_json(&root, quota_bytes))
}

/// 预览区文件操作（「小 VSCode」）：新建文本文件。
///
/// 入参 `{"path":"相对沙箱根的路径","text":"内容"}`；同名已存在一律拒绝（不覆盖）。
/// 返回 `{"ok":bool,"path":"…","error":"…"}`，由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_sandbox_write(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let value: serde_json::Value =
        serde_json::from_str(&read_arg(json)).unwrap_or(serde_json::Value::Null);
    let path = value
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let text = value.get("text").and_then(|v| v.as_str()).unwrap_or_default();
    to_c_string(sandbox::write_json(&root, path, text))
}

/// 预览区文件操作：新建目录（上级目录必须已存在；同名已存在就拒绝）。
#[no_mangle]
pub extern "C" fn lunac_sandbox_mkdir(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let value: serde_json::Value =
        serde_json::from_str(&read_arg(json)).unwrap_or(serde_json::Value::Null);
    let path = value
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    to_c_string(sandbox::mkdir_json(&root, path))
}

/// 预览区文件操作：重命名 / 移动（同一沙箱内；目标不存在）。
#[no_mangle]
pub extern "C" fn lunac_sandbox_rename(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let value: serde_json::Value =
        serde_json::from_str(&read_arg(json)).unwrap_or(serde_json::Value::Null);
    let from = value
        .get("from")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let to = value.get("to").and_then(|v| v.as_str()).unwrap_or_default();
    to_c_string(sandbox::rename_json(&root, from, to))
}

/// 预览区文件操作：删除文件 / 空目录（沙箱根不可删）。
#[no_mangle]
pub extern "C" fn lunac_sandbox_delete(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let value: serde_json::Value =
        serde_json::from_str(&read_arg(json)).unwrap_or(serde_json::Value::Null);
    let path = value
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    to_c_string(sandbox::delete_json(&root, path))
}

/// 「添加到对话」入队（预览窗 → 聊天面板 / 主窗，跨 FlutterEngine 用）。
///
/// 入参 `{"paths":["C:\\…"]}`；去重（大小写不敏感），上限 32 条（超出丢最早的）。
#[no_mangle]
pub extern "C" fn lunac_chat_attach_push(json: *const c_char) -> *mut c_char {
    to_c_string(sandbox::chat_attach_push(&read_arg(json)))
}

/// 取走全部待加入对话的文件（排空）：返回 `{"paths":[…]}`。
#[no_mangle]
pub extern "C" fn lunac_chat_attach_take() -> *mut c_char {
    to_c_string(sandbox::chat_attach_take())
}

/// 预览区终端：起一个伪终端（已经活着就复用）。
///
/// 入参 `{"cols":120,"rows":30}`；cwd 固定是沙箱盘（与 agent 同一个工作目录）。
/// 返回 `{"ok":bool,"reused":bool,"error":"…"}`。
#[no_mangle]
pub extern "C" fn lunac_term_start(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(terminal::start_json(&read_arg(json), &root))
}

/// 预览区终端：写入按键 / 粘贴的字符（`{"data":"…"}`）。
#[no_mangle]
pub extern "C" fn lunac_term_write(json: *const c_char) -> *mut c_char {
    to_c_string(terminal::write_json(&read_arg(json)))
}

/// 预览区终端：改尺寸（`{"cols":120,"rows":30}`）。
#[no_mangle]
pub extern "C" fn lunac_term_resize(json: *const c_char) -> *mut c_char {
    to_c_string(terminal::resize_json(&read_arg(json)))
}

/// 预览区终端：取走待显示的输出（排空）。返回 `{"ok":true,"data":"…","alive":bool}`。
#[no_mangle]
pub extern "C" fn lunac_term_read() -> *mut c_char {
    to_c_string(terminal::read_json())
}

/// 预览区终端：关掉当前终端（关标签 / 重新开始时调）。
#[no_mangle]
pub extern "C" fn lunac_term_stop() -> *mut c_char {
    to_c_string(terminal::stop_json())
}

/// 预览窗标签请求入队（AI 工具 → 聊天面板 → 预览窗，跨 FlutterEngine 用）。
///
/// 入参 `{"title":"…","kind":"app|file|url|workspace","target":"…"}`；
/// 返回 `{"ok":bool,"error":"…"}`，由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_preview_push(json: *const c_char) -> *mut c_char {
    to_c_string(sandbox::preview_push(&read_arg(json)))
}

/// 取走全部待开的预览标签（排空）。返回 `{"tabs":[…]}`。
#[no_mangle]
pub extern "C" fn lunac_preview_take() -> *mut c_char {
    to_c_string(sandbox::preview_take())
}

/// 播一次系统提示音（与 v1 `notify_sound` 同一个 MessageBox「星号」音）。
///
/// 为什么走 `MessageBeep` 而不是自带 wav / just_audio：
///   · 不新增资源与解码链路；
///   · 用户把系统提示音关掉时这里自然静音（不绕过系统意愿）；
///   · 静音 / 无音频设备时返回 0 都属正常，调用方不该据此报错。
#[no_mangle]
pub extern "C" fn lunac_notify_sound() -> i32 {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        extern "system" {
            fn MessageBeep(u_type: u32) -> i32;
        }
        // MB_ICONASTERISK = 0x0000_0040（与 v1 commands.rs 同一个音）
        unsafe { MessageBeep(0x0000_0040) }
    }
    #[cfg(not(windows))]
    {
        0
    }
}

// ── 会话内「AI 改过的文件」审阅（2026-11-14）────────────────────
// 快照落在 `<exe 根>/temp/change-review/<session>/`（未传会话 = 根目录）；
// 每个会话一份，回退不会误还原别的会话改过的文件。细节见 change_review.rs。

/// 采一份「改动前」快照（**不带回合号**，等同第 0 回合；保留给旧调用方）。
#[no_mangle]
pub extern "C" fn lunac_change_snapshot(
    session: *const c_char,
    path: *const c_char,
) -> *mut c_char {
    to_c_string(change_review::snapshot(&read_arg(session), &read_arg(path), 0))
}

/// 按回合采快照：入参 `{"session":"…","path":"C:\\…","turn":N}`（同一回合同一文件只采一次）。
#[no_mangle]
pub extern "C" fn lunac_change_snapshot_at(json: *const c_char) -> *mut c_char {
    let value: serde_json::Value =
        serde_json::from_str(&read_arg(json)).unwrap_or(serde_json::Value::Null);
    let session = value
        .get("session")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let path = value
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let turn = value.get("turn").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    to_c_string(change_review::snapshot(session, path, turn))
}

/// 快照 vs 当前磁盘内容的逐行差异。返回 `{ok, existed, added, removed, lines, …}`。
#[no_mangle]
pub extern "C" fn lunac_change_diff(
    session: *const c_char,
    path: *const c_char,
) -> *mut c_char {
    to_c_string(change_review::diff(&read_arg(session), &read_arg(path)))
}

/// 确认保留改动（删快照，不动文件）。路径为空 = 全部确认（会话为空时 = 所有会话）。
#[no_mangle]
pub extern "C" fn lunac_change_accept(
    session: *const c_char,
    path: *const c_char,
) -> *mut c_char {
    to_c_string(change_review::accept(&read_arg(session), &read_arg(path)))
}

/// 撤销改动（写回原文 / 删除 AI 新建的文件）。路径为空 = 全部撤销。
#[no_mangle]
pub extern "C" fn lunac_change_revert(
    session: *const c_char,
    path: *const c_char,
) -> *mut c_char {
    to_c_string(change_review::revert(&read_arg(session), &read_arg(path)))
}

/// 回退到第 N 回合：只还原 `turn >= N` 的快照（v1 回退按钮的文件还原）。
/// 返回 `{ok, restored, deleted, failed, paths}`；`paths` 供前端从「改动文件」列表摘掉。
#[no_mangle]
pub extern "C" fn lunac_change_revert_since(session: *const c_char, turn: i32) -> *mut c_char {
    to_c_string(change_review::revert_since(
        &read_arg(session),
        turn.max(0) as u32,
    ))
}

/// 这个文件是否还有未确认的改动快照。返回 `{pending}`。
#[no_mangle]
pub extern "C" fn lunac_change_pending(
    session: *const c_char,
    path: *const c_char,
) -> *mut c_char {
    to_c_string(change_review::pending(&read_arg(session), &read_arg(path)))
}

/// 清掉已删会话的快照目录（v1 `prune_frames`）：入参是**保留的会话 id 数组**。
#[no_mangle]
pub extern "C" fn lunac_change_prune(json: *const c_char) -> *mut c_char {
    let value: serde_json::Value =
        serde_json::from_str(&read_arg(json)).unwrap_or(serde_json::Value::Null);
    let keep: Vec<String> = value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|text| text.to_string()))
                .collect()
        })
        .unwrap_or_default();
    to_c_string(change_review::prune(&keep))
}

/// 追加一条用量日志（入参 `{"date":"YYYY-MM-DD","record":{…}}`，与 v1 同格式）。
///
/// 返回 `{"ok":bool,"error":"…"}`，由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_usage_append(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&read_arg(json)) else {
        return to_c_string(
            serde_json::json!({ "ok": false, "error": "参数不是合法 JSON" }).to_string(),
        );
    };
    let date = value
        .get("date")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let Some(record) = value.get("record") else {
        return to_c_string(
            serde_json::json!({ "ok": false, "error": "缺少用量记录" }).to_string(),
        );
    };
    match usage::append(&root, date, record) {
        Ok(()) => to_c_string(serde_json::json!({ "ok": true, "error": "" }).to_string()),
        Err(reason) => to_c_string(serde_json::json!({ "ok": false, "error": reason }).to_string()),
    }
}

/// 读多天用量并汇总（入参 `{"dates":[…],"utcOffsetMinutes":480}`）。
///
/// 返回 JSON 数组（天 → 模型 → 小时），由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_usage_range(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let value = serde_json::from_str::<serde_json::Value>(&read_arg(json))
        .unwrap_or_else(|_| serde_json::json!({}));
    let dates: Vec<String> = value
        .get("dates")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let offset = value
        .get("utcOffsetMinutes")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .clamp(-24 * 60, 24 * 60) as i32;
    to_c_string(serde_json::to_string(&usage::read_range(&root, &dates, offset)).unwrap_or_default())
}

/// 定价表状态：`{path,text,pendingPath,pendingText}`（读不出就是空串）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_pricing_state() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(usage::pricing_state_json(&root))
}

/// 确保 `config/pricing.json` 存在（缺文件时落预置骨架），返回 `{ok,path,error}`。
#[no_mangle]
pub extern "C" fn lunac_pricing_ensure() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match usage::ensure_pricing_file(&root) {
        Ok(path) => to_c_string(
            serde_json::json!({ "ok": true, "path": path.display().to_string(), "error": "" })
                .to_string(),
        ),
        Err(reason) => to_c_string(
            serde_json::json!({ "ok": false, "path": "", "error": reason }).to_string(),
        ),
    }
}

/// 确认候选价格（`today` = 本地 `YYYY-MM-DD`；先校验后覆盖），返回 `{ok,path,error}`。
#[no_mangle]
pub extern "C" fn lunac_pricing_commit(today: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match usage::commit_pricing_pending(&root, &read_arg(today)) {
        Ok(path) => to_c_string(
            serde_json::json!({ "ok": true, "path": path.display().to_string(), "error": "" })
                .to_string(),
        ),
        Err(reason) => to_c_string(
            serde_json::json!({ "ok": false, "path": "", "error": reason }).to_string(),
        ),
    }
}

/// 放弃候选价格（删掉候选文件）。
#[no_mangle]
pub extern "C" fn lunac_pricing_discard() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match usage::clear_pricing_pending(&root) {
        Ok(()) => to_c_string(serde_json::json!({ "ok": true, "error": "" }).to_string()),
        Err(reason) => to_c_string(serde_json::json!({ "ok": false, "error": reason }).to_string()),
    }
}

/// 追加一条边界判定（白箱账本 W0）。入参 `{"date":"YYYY-MM-DD","entry":{…}}`。
///
/// `ts` / `v` 由宿主生成，前端传入的一律忽略；返回 `{"ok":bool,"error":"…"}`。
/// 失败由前端静默忽略 —— 账本是旁路，不能因为写盘失败打断对话。
#[no_mangle]
pub extern "C" fn lunac_security_ledger_append(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&read_arg(json)) else {
        return to_c_string(
            serde_json::json!({ "ok": false, "error": "参数不是合法 JSON" }).to_string(),
        );
    };
    let date = value
        .get("date")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let Some(entry) = value.get("entry") else {
        return to_c_string(
            serde_json::json!({ "ok": false, "error": "缺少边界判定" }).to_string(),
        );
    };
    match security_ledger::append(&root, date, entry) {
        Ok(_) => to_c_string(serde_json::json!({ "ok": true, "error": "" }).to_string()),
        Err(reason) => to_c_string(serde_json::json!({ "ok": false, "error": reason }).to_string()),
    }
}

/// 读最近的边界判定（新 → 旧）。入参 `{"limit":200}`，返回 JSON 数组。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_security_ledger_recent(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let value = serde_json::from_str::<serde_json::Value>(&read_arg(json))
        .unwrap_or_else(|_| serde_json::json!({}));
    let limit = value
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(200)
        .clamp(1, 10_000) as usize;
    to_c_string(
        serde_json::to_string(&security_ledger::recent(&root, limit)).unwrap_or_default(),
    )
}

/// 导出自包含的复盘包（账本 + policy + 版本 / OS build，不含 Key / Cookie）。
///
/// 入参 `{"policy":{…},"appVersion":"0.9.45"}`，返回 `{ok,path,error}`。
#[no_mangle]
pub extern "C" fn lunac_security_ledger_export(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let value = serde_json::from_str::<serde_json::Value>(&read_arg(json))
        .unwrap_or_else(|_| serde_json::json!({}));
    let policy = value
        .get("policy")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let app_version = value
        .get("appVersion")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    match security_ledger::export(&root, &policy, app_version) {
        Ok(path) => to_c_string(
            serde_json::json!({ "ok": true, "path": path.display().to_string(), "error": "" })
                .to_string(),
        ),
        Err(reason) => to_c_string(
            serde_json::json!({ "ok": false, "path": "", "error": reason }).to_string(),
        ),
    }
}

/// 判定预演：把一条命令交给 `agent.exe --analyze`（只分析、不执行），
/// 返回 `{ok,analysis,error}`；`analysis` 与审批请求里的同形（危险 / 不可判定 / 可证只读）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_security_analyze(text: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match security_ledger::analyze_command(&root, &read_arg(text)) {
        Ok(analysis) => to_c_string(
            serde_json::json!({ "ok": true, "analysis": analysis, "error": "" }).to_string(),
        ),
        Err(reason) => to_c_string(
            serde_json::json!({ "ok": false, "analysis": null, "error": reason }).to_string(),
        ),
    }
}

/// 会话「轨迹」预览（§待办 3-28.12）：把一条会话（`chat_history` 的形状）拍平成
/// NDJSON 事件行，**不落盘**。入参就是会话对象本身。
///
/// 返回 `{ok,id,lines,count,bytes,truncated,error}`；每行都过了 `mask_secrets()`。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_session_trace(session: *const c_char) -> *mut c_char {
    match serde_json::from_str::<serde_json::Value>(&read_arg(session)) {
        Ok(value) => to_c_string(session_export::trace_json(&value)),
        Err(error) => to_c_string(
            json!({
                "ok": false,
                "id": "",
                "lines": [],
                "count": 0,
                "bytes": 0,
                "truncated": false,
                "error": format!("会话不是合法 JSON：{error}"),
            })
            .to_string(),
        ),
    }
}

/// 导出当前会话为 `.jsonl`（`<exe 根>\ModuleData\exports\session-<id>-<UTC>.jsonl`）。
///
/// 入参同 `lunac_session_trace`；返回 `{ok,path,count,bytes,truncated,error}`。
/// **不发任何请求**：只是把已有会话写成文件（过 `mask_secrets()`，不含凭据）。
#[no_mangle]
pub extern "C" fn lunac_session_export(session: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match serde_json::from_str::<serde_json::Value>(&read_arg(session)) {
        Ok(value) => to_c_string(session_export::export_json(&root, &value)),
        Err(error) => to_c_string(
            json!({
                "ok": false,
                "path": "",
                "count": 0,
                "bytes": 0,
                "truncated": false,
                "error": format!("会话不是合法 JSON：{error}"),
            })
            .to_string(),
        ),
    }
}

/// 读全部自定义启动项（JSON 数组；每项 `name` / `path` / `source`）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_custom_apps() -> *mut c_char {
    ensure_data_root();
    let items: Vec<serde_json::Value> = lunac_indexer::app_indexer::list_custom_apps()
        .into_iter()
        .map(|app| {
            json!({
                "name": app.name,
                "path": app.path,
                "source": app.source,
            })
        })
        .collect();
    to_c_string(serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string()))
}

/// 新增一条自定义启动项（入参 JSON：`{"name": "...", "path": "..."}`）。
///
/// 返回 0 = 已注册，1 = 参数非法，2 = 注册失败（文件不存在 / 已注册 / 写失败）。
#[no_mangle]
pub extern "C" fn lunac_add_custom_app(entry: *const c_char) -> i32 {
    ensure_data_root();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&read_arg(entry)) else {
        return 1;
    };
    let Some(object) = value.as_object() else {
        return 1;
    };
    let Some(name) = object.get("name").and_then(|v| v.as_str()) else {
        return 1;
    };
    let Some(path) = object.get("path").and_then(|v| v.as_str()) else {
        return 1;
    };
    if path.trim().is_empty() {
        return 1;
    }
    match lunac_indexer::app_indexer::add_custom_app(name, path) {
        Ok(()) => 0,
        Err(reason) => {
            eprintln!("[lunac-quicklaunch] 注册失败: {reason}");
            2
        }
    }
}

/// 删除一条自定义启动项。返回 0 = 已删除，1 = path 为空，2 = 删除失败。
#[no_mangle]
pub extern "C" fn lunac_remove_custom_app(path: *const c_char) -> i32 {
    ensure_data_root();
    let path = read_arg(path);
    if path.trim().is_empty() {
        return 1;
    }
    match lunac_indexer::app_indexer::remove_custom_app(path.trim()) {
        Ok(()) => 0,
        Err(reason) => {
            eprintln!("[lunac-quicklaunch] 删除失败: {reason}");
            2
        }
    }
}

/// 读全局长期记忆全文（`/memory` 命令；没有记忆 = 空串）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_memory_read() -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string(String::new());
    };
    match memory::read_text(&root) {
        Ok(text) => to_c_string(text),
        Err(reason) => {
            eprintln!("[lunac-memory] 读取失败: {reason}");
            to_c_string(String::new())
        }
    }
}

/// 清空全局长期记忆（破坏性，调用方负责二次确认）。
///
/// 返回 0 = 已清空，2 = 写失败。
#[no_mangle]
pub extern "C" fn lunac_memory_clear() -> i32 {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return 2;
    };
    match memory::clear(&root) {
        Ok(()) => 0,
        Err(reason) => {
            eprintln!("[lunac-memory] 清空失败: {reason}");
            2
        }
    }
}

/// 技能级记忆列表（`ModuleData/memory/skills/<key>/MEMORY.md`）。
/// 返回 `{ok, items:[{key,chars,updatedAt,path}], error}`。
#[no_mangle]
pub extern "C" fn lunac_skill_memory_list() -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string(
            json!({ "ok": false, "items": [], "error": "找不到数据根目录" }).to_string(),
        );
    };
    to_c_string(
        json!({
            "ok": true,
            "items": memory::list_skill_memories(&root),
            "error": "",
        })
        .to_string(),
    )
}

/// 读某个技能的记忆正文。返回 `{ok,text,error}`。
#[no_mangle]
pub extern "C" fn lunac_skill_memory_read(key: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match memory::read_skill_memory(&root, &read_arg(key)) {
        Ok(text) => to_c_string(json!({ "ok": true, "text": text, "error": "" }).to_string()),
        Err(reason) => {
            to_c_string(json!({ "ok": false, "text": "", "error": reason }).to_string())
        }
    }
}

/// 清空某个技能的记忆（破坏性，调用方负责二次确认）。返回 `{ok,error}`。
#[no_mangle]
pub extern "C" fn lunac_skill_memory_clear(key: *const c_char) -> *mut c_char {
    ensure_data_root();
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    match memory::clear_skill_memory(&root, &read_arg(key)) {
        Ok(()) => to_c_string(json!({ "ok": true, "error": "" }).to_string()),
        Err(reason) => to_c_string(json!({ "ok": false, "error": reason }).to_string()),
    }
}

/// 桌宠人格（`config/pet-persona.md`）：`{ok,path,text,maxChars,error}`。
///
/// 文件不存在时返回并落盘 v1 的默认人格（与 v1 `ensure_pet_persona_default` 同口径）。
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_pet_persona() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(memory::pet_persona_json(&root))
}

/// 保存桌宠人格（先校验后写：≤ 8000 字符、不含 NUL）。返回 `{ok,error}`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_pet_set_persona(text: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(memory::set_pet_persona_json(&root, &read_arg(text)))
}

/// 桌宠三份列表之一（kind = `memory` / `growth` / `suggestions`）：
/// `{ok,entries[],maxEntryChars,maxTotalChars,error}`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_pet_memory(kind: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(memory::pet_list_json(&root, &read_arg(kind)))
}

/// 删掉桌宠列表里的一条（按内容归一比对）。返回 `{ok,error}`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_pet_memory_delete(kind: *const c_char, text: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(memory::delete_pet_entry_json(&root, &read_arg(kind), &read_arg(text)))
}

/// 清空桌宠列表（破坏性，调用方负责二次确认）。返回 `{ok,error}`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_pet_memory_clear(kind: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(memory::clear_pet_list_json(&root, &read_arg(kind)))
}

/// 「接受」一条人格修订建议：并进人格后从建议区摘掉。返回 `{ok,error}`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_pet_merge_suggestion(text: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(memory::merge_persona_suggestion_json(&root, &read_arg(text)))
}

/// 按「最近在 Lunac 里打开的时间」降序；时间相同保持插入顺序（v1 recency 排序同口径）。
fn sort_by_recency(items: &mut [(u64, usize, serde_json::Value)]) {
    items.sort_by_key(|(stamp, order, _)| (std::cmp::Reverse(*stamp), *order));
}

/// 简要搜索（搜索栏下方的小结果区）：应用 + 内置插件，**不返回文件**。
///
/// 排序对齐 v1 `renderMixedResults`：应用、插件按「最近在 Lunac 里打开的时间」降序；
/// 没打开过的保持原始顺序（应用按名称、插件按目录顺序）。文件只出现在详细搜索
/// （双击搜索栏）里，简要搜索不再混入文件结果。
///
/// 返回 JSON 数组字符串；**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_search(query: *const c_char) -> *mut c_char {
    let query = read_arg(query);
    let trimmed = query.trim();
    ensure_indexer();

    let plugins = search_entries(&query);
    let mut head: Vec<(u64, usize, serde_json::Value)> = Vec::new();
    let mut push_head = |stamp: u64, value: serde_json::Value| {
        let order = head.len();
        head.push((stamp, order, value));
    };

    // 应用：空查询 = 最近打开的 5 个；有查询词 = 相关性前 6 个，再按打开时间稳定重排。
    let app_limit = if trimmed.is_empty() { 5 } else { MAX_APP_HITS };
    let mut apps = if trimmed.is_empty() {
        lunac_indexer::app_indexer::apps()
    } else {
        lunac_indexer::app_indexer::search_apps(trimmed, app_limit)
    };
    apps.sort_by_key(|app| std::cmp::Reverse(recency::score(&format!("app:{}", app.path))));
    apps.truncate(app_limit);
    for app in apps {
        let key = format!("app:{}", app.path);
        push_head(
            recency::score(&key),
            json!({
                "id": key,
                "title": app.name,
                "subtitle": "应用程序",
                "kind": "app",
            }),
        );
    }

    // 插件 / 内置动作（空查询 = 全部；有查询词 = 标题 / 副标题 / 关键词 / 拼音命中）。
    for entry in &plugins {
        push_head(
            recency::score(entry.id),
            json!({
                "id": entry.id,
                "title": entry.title,
                "subtitle": entry.subtitle,
                "kind": entry.kind,
            }),
        );
    }

    // 兜底行单独一档：预留条数，保证永远可达（否则会被高 recency 的应用 / 插件挤掉）。
    let mut tail: Vec<(u64, usize, serde_json::Value)> = Vec::new();
    if !trimmed.is_empty() {
        // 备忘录「标识」直达：精确 / 前缀命中，排最前直接进入编辑（v1 同语义）。
        let memo_root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
        for (id, tag) in memo::search_tags(&memo_root, trimmed) {
            push_head(
                u64::MAX,
                json!({
                    "id": format!("memo:{id}"),
                    "title": format!("编辑备忘录 · {tag}"),
                    "subtitle": "备忘录",
                    "kind": "command",
                }),
            );
        }

        // 插件行已命中（输入 web / ai）时不再重复放兜底行（v1 `renderedPluginIds` 同款）。
        if !plugins.iter().any(|entry| entry.id == "web-home") {
            tail.push((
                recency::score("web-home"),
                0,
                json!({
                    "id": format!("web:{trimmed}"),
                    "title": format!("用 {} 搜索 “{trimmed}”", settings::current_display()),
                    "subtitle": format!("{}: {trimmed}", settings::current_display()),
                    "kind": "web",
                }),
            ));
        }
        if !plugins.iter().any(|entry| entry.id == "ai") {
            tail.push((
                recency::score("ai"),
                1,
                json!({
                    "id": format!("ai:{trimmed}"),
                    "title": format!("用 AI 提问：{trimmed}"),
                    "subtitle": "AI 助手问答",
                    "kind": "command",
                }),
            ));
        }
    }

    // 应用 + 插件按打开时间降序（时间相同保持插入顺序）；空查询不截断（目录要完整）。
    sort_by_recency(&mut head);
    if !trimmed.is_empty() {
        head.truncate(MAX_RESULTS.saturating_sub(tail.len()));
    }
    sort_by_recency(&mut tail);
    head.extend(tail);

    let items: Vec<serde_json::Value> = head.into_iter().map(|(_, _, value)| value).collect();
    let text = serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string());
    to_c_string(text)
}

/// 详细搜索：应用查询（条数由调用方给，详细搜索一屏能放更多行）。
///
/// 返回 JSON 数组字符串；**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_search_apps(query: *const c_char, limit: u32) -> *mut c_char {
    let query = read_arg(query);
    ensure_indexer();

    let limit = limit.clamp(1, 200) as usize;
    let items: Vec<serde_json::Value> =
        lunac_indexer::app_indexer::search_apps(query.trim(), limit)
            .into_iter()
            .map(|app| {
                json!({
                    "id": format!("app:{}", app.path),
                    "title": app.name,
                    "subtitle": "应用程序",
                    "kind": "app",
                })
            })
            .collect();
    to_c_string(
        serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string()),
    )
}

/// 详细搜索：按类型过滤的文件查询（`kind` 传空串 = 全部类型）。
///
/// 与 `lunac_search` 的区别：这里只查文件、条数上限由调用方给、并回传文件类型
/// （`fileKind`：folder / document / image / video / audio / archive / program / other），
/// 详细搜索大界面的类型胶囊就用它。
///
/// 返回 JSON 数组字符串；**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_search_files(
    query: *const c_char,
    kind: *const c_char,
    limit: u32,
) -> *mut c_char {
    let query = read_arg(query);
    let kind = read_arg(kind);
    ensure_indexer();

    let limit = limit.clamp(1, 200) as usize;
    let kind_filter = if kind.trim().is_empty() {
        None
    } else {
        Some(kind.trim())
    };
    let items: Vec<serde_json::Value> =
        lunac_indexer::file_indexer::search(query.trim(), kind_filter, limit)
            .into_iter()
            .map(|hit| {
                json!({
                    "id": format!("file:{}", hit.path),
                    "title": hit.name,
                    "subtitle": hit.path,
                    "kind": "file",
                    "fileKind": hit.kind,
                })
            })
            .collect();
    to_c_string(
        serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string()),
    )
}

/// 数据根目录（= 宿主可执行文件所在目录；设置面板「关于」用它）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_data_dir() -> *mut c_char {
    to_c_string(
        exe_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default(),
    )
}

/// 文件索引状态：条数 / 是否在扫盘 / 落盘时间 / 是否被上限截断 / 扫描的根目录。
///
/// 详细搜索的底栏与「重建索引」按钮用它（v1 `file_index_status` 同口径）。
/// 返回 JSON 对象字符串；**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_index_status() -> *mut c_char {
    ensure_indexer();
    let status = lunac_indexer::file_indexer::status();
    to_c_string(serde_json::to_string(&status).unwrap_or_else(|_| "{}".to_string()))
}

/// 强制重建文件索引（后台线程扫盘，立即返回）。
#[no_mangle]
pub extern "C" fn lunac_index_refresh() {
    ensure_indexer();
    lunac_indexer::file_indexer::refresh_in_background();
}

/// 详细搜索的「设置 / 系统动作」目录（按查询词过滤；空查询 = 全部）。
///
/// 数据是静态表（41 个 `ms-settings:` 页 + 26 个白名单动作），过滤在 Rust 侧做，
/// 因为关键词（中英 + 拼音首字母）只在这里维护。
///
/// 返回 JSON 数组字符串；**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_system_catalog(query: *const c_char) -> *mut c_char {
    let query = read_arg(query);
    let items: Vec<serde_json::Value> = system_catalog::search(&query)
        .into_iter()
        .map(|item| {
            json!({
                "id": item.id,
                "kind": item.kind,
                "icon": item.icon,
                "title": item.title_zh,
                "titleEn": item.title_en,
                "target": item.target,
                "danger": item.danger,
                "elevatable": item.elevatable,
            })
        })
        .collect();
    to_c_string(
        serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string()),
    )
}

/// 执行一条动作。返回码见文件头。
#[no_mangle]
pub extern "C" fn lunac_run_action(id: *const c_char) -> i32 {
    let action = read_arg(id);

    // 记录「最近在 Lunac 里打开」的时间：简要搜索结果按这个时间排序（v1 recency 同义）。
    // `web:<查询词>` / `ai:<查询词>` 归到固定插件键，避免时间被拆散到每个查询词上。
    if !action.is_empty() {
        let key = if action.starts_with("ai:") {
            "ai"
        } else if action.starts_with("web:") {
            "web-home"
        } else if action.starts_with("memo:") {
            "memo"
        } else {
            action.as_str()
        };
        recency::record(key);
    }

    // 「用 AI 提问」兜底行（`ai:<查询词>`）：由 Dart 打开对话面板并预填查询词。
    if action.starts_with("ai:") {
        return 2;
    }

    // 详细搜索的目录条目：设置页 / 系统动作 / 提权动作。
    if let Some(target) = action.strip_prefix("setting:") {
        return match system_catalog::open_setting(target) {
            Ok(()) => 0,
            Err(_) => 3,
        };
    }
    if let Some(action_id) = action.strip_prefix("action:") {
        recency::record(&action);
        return match system_catalog::run_action(action_id) {
            Ok(()) => 0,
            Err(_) => 3,
        };
    }
    // 提权形态（详细搜索 Shift+Enter / 右键「以管理员身份运行」）：
    //   `admin:action:<id>` → 白名单动作提权；`admin:app:<path>` / `admin:file:<path>`
    //   → 以管理员身份启动应用 / 打开文件（v1 `launch_app_elevated` 同口径）。
    if let Some(inner) = action.strip_prefix("admin:") {
        recency::record(inner);
        if let Some(action_id) = inner.strip_prefix("action:") {
            return match system_catalog::run_action_elevated(action_id) {
                Ok(()) => 0,
                Err(_) => 3,
            };
        }
        if let Some(path) = action_path(inner) {
            return match lunac_indexer::app_indexer::launch_elevated(path, None) {
                Ok(()) => 0,
                Err(_) => 3,
            };
        }
        // 兼容旧形态 `admin:<动作 id>`（详细搜索右键菜单在 M1w 时这么拼）。
        return match system_catalog::run_action_elevated(inner) {
            Ok(()) => 0,
            Err(_) => 3,
        };
    }

    // 应用 / 文件结果：id 里带着绝对路径（`app:` / `file:` 前缀）。
    if let Some(path) = action_path(&action) {
        recency::record(&action);
        return match lunac_indexer::app_indexer::launch_app(path) {
            Ok(()) => 0,
            Err(_) => 3,
        };
    }

    // Web 搜索兜底行：`web:<查询词>`（用当前引擎）。
    if let Some(query) = action.strip_prefix("web:") {
        recency::record(&action);
        return if open_url(&settings::search_url(query)) {
            0
        } else {
            3
        };
    }

    // 备忘录：`memo` 打开编辑器，`memo:<id>` 直达某一条（都由 Dart 面板处理）。
    if action == "memo" || action.starts_with("memo:") {
        return 2;
    }

    // 翻译：面板与联网都在 Dart / C ABI 里（`lunac_translate_lookup`），这里只交还 UI。
    if action == "translate" {
        return 2;
    }

    // 文件转换：面板在 Dart，转换在 C ABI（`lunac_convert_plan` / `lunac_convert_run`）。
    if action == "convert" {
        return 2;
    }

    // OCR：面板在 Dart（`lib/search/ocr_panel.dart`），识别走 `lunac_ocr_image`。
    if action == "ocr" {
        return 2;
    }

    // 音乐：面板在 Dart（`lib/search/music_panel.dart`），本地曲库 / 歌词走
    // `lunac_music_library` / `lunac_music_lyrics`，出声由 Flutter 的 just_audio 负责。
    if action == "music" {
        return 2;
    }

    // 代理：面板在 Dart（`lib/search/proxy_panel.dart`），系统代理读写走
    // `lunac_proxy_status` / `lunac_proxy_system_set`。
    if action == "proxy" {
        return 2;
    }

    // 工具编辑器：面板与 `<exe 根>/tools/*.json` 的读写都在 Dart / C ABI
    // （`lunac_mcp_config` / `lunac_mcp_config_save`），这里只交还 UI。
    if action == "tool-editor" {
        return 2;
    }

    // 桌宠：控制台面板在 Dart，桌宠窗在 C ABI（`lunac_pet_*`）。
    if action == "pet" {
        return 2;
    }

    match action.as_str() {
        // 空查询时的「Web 搜索」行：打开当前引擎首页。
        "web-home" => return if open_url(settings::home_url()) { 0 } else { 3 },
        "open-exe-dir" => match exe_dir() {
            Some(dir) => open_path(&dir),
            None => 3,
        },
        "open-downloads" => match downloads_dir() {
            Some(dir) => open_path(&dir),
            None => 3,
        },
        // AI 对话 / 设置 / 剪贴板历史 / 快捷启动 / 退出：由 Dart 侧处理窗口与占位提示。
        "ai" | "settings" | "clipboard-history" | "quick-launch" | "quit" => 2,
        _ => 1,
    }
}

/// 释放 `lunac_search` 返回的字符串。空指针是安全的 no-op。
#[no_mangle]
pub extern "C" fn lunac_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            drop(CString::from_raw(ptr));
        }
    }
}

/// 单实例：1 = 本进程是首个实例；0 = 已有实例在跑（调用方 signal 后立即退出）。
///
/// 必须在创建 / 显示窗口之前调用（Dart `main()` 第一件事）—— 首个 Flutter 帧之前
/// 退出，窗口就不会闪一下。
#[no_mangle]
pub extern "C" fn lunac_single_instance_acquire() -> i32 {
    if single_instance::acquire() {
        1
    } else {
        0
    }
}

/// 第二个实例调用：通知已有实例把窗口唤出，然后自己退出。
#[no_mangle]
pub extern "C" fn lunac_single_instance_signal() {
    let _ = single_instance::signal_existing();
}

/// 启动全局热键（幂等；Windows 为 `RegisterHotKey`，Linux 暂为 no-op）。
#[no_mangle]
pub extern "C" fn lunac_hotkey_start() {
    hotkey::start(exe_dir().unwrap_or_else(|| PathBuf::from(".")));
}

/// 绑定主窗句柄：之后热键 / 再启动一次会直接向主窗 `PostMessage`（事件驱动唤出，
/// 后台不再轮询）。句柄来自 runner 的 `lunac/wake` 通道。
#[no_mangle]
pub extern "C" fn lunac_set_host_window(hwnd: isize) {
    hotkey::set_host_window(hwnd);
}

/// 当前唤醒热键组合（`config/hotkey.json`；缺失 = 默认 `Ctrl+Alt+Space`）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_hotkey_combo() -> *mut c_char {
    let combo = match exe_dir() {
        Some(root) => hotkey::current_combo(&root),
        None => String::new(),
    };
    to_c_string(combo)
}

/// 改唤醒热键：先换注册、成功才写 `config/hotkey.json`（失败会回滚旧键）。
///
/// 返回 0 成功 / 1 组合非法 / 2 注册失败（被占用）/ 3 热键线程不可用（含 Linux 未实现）。
#[no_mangle]
pub extern "C" fn lunac_hotkey_set(combo: *const c_char) -> i32 {
    let combo = read_arg(combo);
    let Some(root) = exe_dir() else {
        return 3;
    };
    match hotkey::set_combo(&root, &combo) {
        Ok(()) => 0,
        Err((code, reason)) => {
            eprintln!("[lunac-hotkey] 改键失败: {reason}");
            code
        }
    }
}

/// 取走待处理的热键按下次数（读一次即清零）。
///
/// Dart 侧轮询到 >0 就**切换**窗口显示状态（可见→隐藏，隐藏→显示并聚焦）。
#[no_mangle]
pub extern "C" fn lunac_wake_take() -> u32 {
    hotkey::take_wake()
}

/// 取走待处理的「第二个实例启动」次数（读一次即清零）。
///
/// 与热键分开：这里只要求**显示并聚焦**，不切换（v1 同语义）。
#[no_mangle]
pub extern "C" fn lunac_activate_take() -> u32 {
    hotkey::take_activate()
}

/// 当前默认搜索引擎（`google` / `bing` / `baidu`）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_web_engine() -> *mut c_char {
    to_c_string(settings::current().to_string())
}

/// 切换默认搜索引擎（`google` / `bing` / `baidu`，大小写不敏感）。
/// 返回 1 = 成功并已持久化到 `<exe 根>/config/search.json`；0 = 名字非法。
#[no_mangle]
pub extern "C" fn lunac_set_web_engine(name: *const c_char) -> i32 {
    ensure_data_root();
    if settings::set(&read_arg(name)) {
        1
    } else {
        0
    }
}

/// 自动更新配置：`{"checkOnStartup":bool,"autoInstall":bool}`（v1 `update.json` 同一格式）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_update_config() -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string(
            serde_json::json!({ "checkOnStartup": true, "autoInstall": true }).to_string(),
        );
    };
    let config = settings::update_config(&root);
    to_c_string(
        serde_json::json!({
            "checkOnStartup": config.check_on_startup,
            "autoInstall": config.auto_install,
        })
        .to_string(),
    )
}

/// 写「自动更新」单个开关（两字段同真同假），返回 `{"ok":bool,"error":"…"}`。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
#[no_mangle]
pub extern "C" fn lunac_set_update_auto(on: i32) -> *mut c_char {
    ensure_data_root();
    let Some(root) = exe_dir() else {
        return to_c_string(
            serde_json::json!({ "ok": false, "error": "无法定位可执行文件目录" }).to_string(),
        );
    };
    match settings::set_update_auto(&root, on != 0) {
        Ok(_) => to_c_string(serde_json::json!({ "ok": true, "error": "" }).to_string()),
        Err(reason) => to_c_string(serde_json::json!({ "ok": false, "error": reason }).to_string()),
    }
}

/// 读系统剪贴板的文本（CF_UNICODETEXT；不可用 / 超长时返回空串）。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
/// 只在「窗口被唤出」时调用一次（v1 同口径），不要轮询。
#[no_mangle]
pub extern "C" fn lunac_clipboard_text() -> *mut c_char {
    to_c_string(clipboard::text())
}

/// 唤出时的剪贴板负载：文件（CF_HDROP）> 图片（CF_DIB / CF_DIBV5 → BMP）> 文本。
///
/// 返回 JSON 字符串（`kind` = `files` / `image` / `text` / `empty`），由 Rust 分配，
/// **调用方负责** `lunac_free_string`。只读系统剪贴板；任何失败降级为 `empty`。
#[no_mangle]
pub extern "C" fn lunac_clipboard_payload() -> *mut c_char {
    to_c_string(clipboard_payload::read_json())
}

/// 剪贴板里 CF_HDROP 的全部路径（JSON 字符串数组，v1 `read_clipboard_file_paths`）。
#[no_mangle]
pub extern "C" fn lunac_clipboard_file_paths() -> *mut c_char {
    to_c_string(json_array(clipboard::file_paths()))
}

/// 剪贴板里扩展名是图片的路径（JSON 数组，v1 `read_clipboard_files`）。
///
/// 唤出时若它非空，界面可插入「OCR / 图片」相关入口（附件泡泡前端改版阶段接入）。
#[no_mangle]
pub extern "C" fn lunac_clipboard_image_files() -> *mut c_char {
    to_c_string(json_array(clipboard::image_files()))
}

/// 在资源管理器里定位一个文件 / 文件夹（`explorer /select,<path>`）。
/// 返回 0 = 已发起，1 = 参数非法，3 = 失败。
#[no_mangle]
pub extern "C" fn lunac_reveal_path(path: *const c_char) -> i32 {
    let path = read_arg(path);
    if path.trim().is_empty() {
        return 1;
    }
    if reveal_in_explorer(&path) {
        0
    } else {
        3
    }
}

/// 用系统默认程序打开一个 URL（设置面板「检查更新 → 前往下载」）。
///
/// 返回 0 = 已发起，1 = 空串 / 不是 http(s)，3 = 失败。
#[no_mangle]
pub extern "C" fn lunac_open_url(url: *const c_char) -> i32 {
    let url = read_arg(url);
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return 1;
    }
    if open_url(trimmed) {
        0
    } else {
        3
    }
}

/// 取文件 / 应用的 32×32 图标（`data:image/png;base64,...`）；取不到返回空串。
///
/// 返回字符串由 Rust 分配，**调用方负责** `lunac_free_string`。
/// **不要每次击键都调**：图标抽取要走 GDI（内部串行），Dart 侧按路径缓存，
/// 只为结果行首次出现时取一次。
#[no_mangle]
pub extern "C" fn lunac_app_icon(path: *const c_char) -> *mut c_char {
    let path = read_arg(path);
    if path.trim().is_empty() {
        return to_c_string(String::new());
    }
    to_c_string(lunac_indexer::icon::extract_icon_base64(&path).unwrap_or_default())
}

// ── 内部实现 ─────────────────────────────────────────────────────

// ══════════════════════════════════════════════════════════════════
//  桌宠（v1 `pet.ts` 的窗口 / 形象那一半；见 `pet.rs`）
// ══════════════════════════════════════════════════════════════════
// 所有返回字符串由 Rust 分配，调用方负责 `lunac_free_string`。

/// 读桌宠配置（JSON；字段名与 v1 `lunac.pet.v1` 一致）。
#[no_mangle]
pub extern "C" fn lunac_pet_config() -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::config_json())
}

/// 合并保存配置（只传关心的字段），并通知已开的桌宠窗套用。
#[no_mangle]
pub extern "C" fn lunac_pet_apply(json: *const c_char) -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::apply_json(&read_arg(json)))
}

/// 合并保存「主动反应」配置（v1 `ProactiveCfg`：开关 / 间隔 / 每日上限 / 抓屏频次 /
/// 屏幕最小间隔 / 灵敏度），返回保存后的完整配置 JSON。
#[no_mangle]
pub extern "C" fn lunac_pet_set_proactive(json: *const c_char) -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::set_proactive_json(&read_arg(json)))
}

/// 桌宠语音状态：配置 + GPT-SoVITS 服务 / 参考音色体检 + 一句原因。
///
/// 合成由外部 `gsv_server` 提供；本函数只做 HTTP 连通性与参数检查。
#[no_mangle]
pub extern "C" fn lunac_pet_voice_status() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(pet_voice::status_json(&root))
}

/// 合并保存语音配置（只传关心的字段）；返回 `{ok, error, status}`。
#[no_mangle]
pub extern "C" fn lunac_pet_voice_set(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(pet_voice::set_json(&root, &read_arg(json)))
}

/// 朗读一段文本（GPT-SoVITS / gsv.cpp）；返回 `{ok, queued, error}`，播放后台进行。
#[no_mangle]
pub extern "C" fn lunac_pet_voice_speak(text: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(pet_voice::speak_json(&root, &read_arg(text)))
}

/// 选择参考音频（单选）；返回 `{ok, canceled, path}`。
#[no_mangle]
pub extern "C" fn lunac_pet_voice_pick_audio() -> *mut c_char {
    to_c_string(pet_voice::pick_audio_json())
}

/// 读桌宠旁挂文件（`kind`：`hotkeys` / `chains`）；返回 `{"ok":true,"text":...}`。
#[no_mangle]
pub extern "C" fn lunac_pet_side_read(
    model_path: *const c_char,
    kind: *const c_char,
) -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::read_side_json(
        &read_arg(model_path),
        &read_arg(kind),
    ))
}

/// 写桌宠旁挂文件；返回 `{"ok":true,"path":...}`。
#[no_mangle]
pub extern "C" fn lunac_pet_side_write(
    model_path: *const c_char,
    kind: *const c_char,
    json: *const c_char,
) -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::write_side_json(
        &read_arg(model_path),
        &read_arg(kind),
        &read_arg(json),
    ))
}

/// 列出模型目录下全部 `*.motion3.json`；返回 `{"ok":true,"files":[{path,name}]}`。
#[no_mangle]
pub extern "C" fn lunac_pet_motion_files(model_path: *const c_char) -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::motion_files_json(&read_arg(model_path)))
}

/// 新建一个动作文件（已存在则拒绝）；返回 `{"ok":true,"path":...}`。
#[no_mangle]
pub extern "C" fn lunac_pet_motion_write(
    model_path: *const c_char,
    name: *const c_char,
    contents: *const c_char,
) -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::motion_write_json(
        &read_arg(model_path),
        &read_arg(name),
        &read_arg(contents),
    ))
}

/// 删除一个动作文件。
#[no_mangle]
pub extern "C" fn lunac_pet_motion_delete(
    model_path: *const c_char,
    name: *const c_char,
) -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::motion_delete_json(
        &read_arg(model_path),
        &read_arg(name),
    ))
}

/// 重命名一个动作文件（新名留在原目录）；返回 `{"ok":true,"path":...}`。
#[no_mangle]
pub extern "C" fn lunac_pet_motion_rename(
    model_path: *const c_char,
    from: *const c_char,
    to: *const c_char,
) -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::motion_rename_json(
        &read_arg(model_path),
        &read_arg(from),
        &read_arg(to),
    ))
}

/// 递归列出模型目录下的 `*.exp3.json`（热键「重新导入」的回落数据源）。
#[no_mangle]
pub extern "C" fn lunac_pet_exp3_files(model_path: *const c_char) -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::exp3_files_json(&read_arg(model_path)))
}

/// 扫模型文件夹根（`model3` / `vtube` / `cdi3` 三类路径）。
#[no_mangle]
pub extern "C" fn lunac_pet_scan_folder(model_path: *const c_char) -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::scan_folder_json(&read_arg(model_path)))
}

/// 请求桌宠窗引擎上报「运行时参数表」（新建动作模板用；结果从 `lunac_pet_poll` 取）。
#[no_mangle]
pub extern "C" fn lunac_pet_request_core_ids() -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::request_core_ids())
}

/// 播放 `Lunac` 合成组里的第 `index` 个动作（行号 = `list_motion_files` 顺序）。
#[no_mangle]
pub extern "C" fn lunac_pet_play_motion(index: u32) -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::play_motion(index))
}

/// 读模型的物理输出参数 id（新建动作模板默认排除它们）。
#[no_mangle]
pub extern "C" fn lunac_pet_physics_outputs(model_path: *const c_char) -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::physics_outputs_json(&read_arg(model_path)))
}

/// 显示桌宠（首次调用会起窗口线程）。
#[no_mangle]
pub extern "C" fn lunac_pet_show() -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::show())
}

/// 隐藏桌宠（保留窗口与渲染器）。
#[no_mangle]
pub extern "C" fn lunac_pet_hide() -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::hide())
}

/// 退出桌宠（销毁窗口与 WebView2）。
#[no_mangle]
pub extern "C" fn lunac_pet_quit() -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::quit())
}

/// 鼠标穿透（v1 口径：只能从控制台控制）。
#[no_mangle]
pub extern "C" fn lunac_pet_set_click_through(on: bool) -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::set_click_through(on))
}

/// 置顶开关。
#[no_mangle]
pub extern "C" fn lunac_pet_set_topmost(on: bool) -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::set_topmost(on))
}

/// 重置桌宠窗位置（清掉记住的坐标，回到主屏右下）。
#[no_mangle]
pub extern "C" fn lunac_pet_reset_position() -> *mut c_char {
    ensure_data_root();
    to_c_string(pet::reset_position())
}

/// 运行时状态 JSON：`{open, error}`。
#[no_mangle]
pub extern "C" fn lunac_pet_state() -> *mut c_char {
    to_c_string(pet::state_json())
}

/// 取事件队列（JSON 数组）并清空。Dart 侧用定时器轮询
/// （右键菜单「打开控制台」、滚轮缩放、窗口关闭都从这里来）。
#[no_mangle]
pub extern "C" fn lunac_pet_poll() -> *mut c_char {
    to_c_string(pet::poll_events())
}

/// 系统的「选择形象文件」对话框。返回绝对路径；取消 / 失败返回空串。
#[no_mangle]
pub extern "C" fn lunac_pet_pick_file() -> *mut c_char {
    to_c_string(pet::pick_file())
}

/// 校验 `*.model3.json`（控制台导入前用）。返回 `pet::ModelInfo` 的 JSON。
#[no_mangle]
pub extern "C" fn lunac_pet_model_info(path: *const c_char) -> *mut c_char {
    to_c_string(pet::model_info_json(&read_arg(path)))
}

/// 「模型调试」一条指令（表情 / 参数）。返回 `{ ok, error, open }`。
#[no_mangle]
pub extern "C" fn lunac_pet_debug(json: *const c_char) -> *mut c_char {
    to_c_string(pet::debug_json(&read_arg(json)))
}

/// 起定时任务调度线程（幂等：重复调用是无操作）。App 启动时调一次。
#[no_mangle]
pub extern "C" fn lunac_cron_start() {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    cron::start_scheduler(root);
}

/// 定时任务列表（定义 + 运行态 + `schedulerAlive`）。返回 JSON 字符串。
#[no_mangle]
pub extern "C" fn lunac_cron_list() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(cron::list_json(&root))
}

/// 立即运行一条 job（阻塞到跑完；Dart 侧用 `Isolate.run` 调用）。
/// 返回 `{ ok, error, job }`。
#[no_mangle]
pub extern "C" fn lunac_cron_run_now(id: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(cron::run_now(&root, &read_arg(id)))
}

/// 前端忙态（流式对话开始 / 结束）。调度器看到忙就记 `skipped_busy`，不排队。
#[no_mangle]
pub extern "C" fn lunac_cron_set_busy(busy: i32) {
    cron::set_busy(busy != 0);
}

/// 逐 job 的「无人值守可写」授权（只有用户二次确认后才能打开）。
/// 返回 `{ ok, error, writeAccess }`。
#[no_mangle]
pub extern "C" fn lunac_cron_set_write_access(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let value: serde_json::Value =
        serde_json::from_str(&read_arg(json)).unwrap_or(serde_json::Value::Null);
    let id = value
        .get("id")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let allow = value
        .get("allow")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    to_c_string(cron::set_write_access(&root, id, allow))
}

/// 启用 / 停用一条定时任务（前端入口）。入参 `{"id":"cron_x","enabled":true}`；
/// 返回 `{ ok, error, enabled }`。重新启用会清掉「自动停用」与连续失败并重排下一次。
#[no_mangle]
pub extern "C" fn lunac_cron_set_enabled(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    let value: serde_json::Value =
        serde_json::from_str(&read_arg(json)).unwrap_or(serde_json::Value::Null);
    let id = value
        .get("id")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let enabled = value
        .get("enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    to_c_string(cron::set_enabled(&root, id, enabled))
}

/// 起任务队列 worker（幂等；App 启动时调一次）。与定时任务共用「同时只跑一个」单飞闸。
#[no_mangle]
pub extern "C" fn lunac_tasks_start() {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    task_queue::start_worker(root);
}

/// 任务队列快照（`{ok, reason, items:[…]}`）。返回 JSON 字符串。
#[no_mangle]
pub extern "C" fn lunac_tasks_list() -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(task_queue::snapshot_json(&root))
}

/// 取消一条**排队中**的任务。入参 `{"id":"task_x"}`；返回 `{ ok, error }`。
#[no_mangle]
pub extern "C" fn lunac_tasks_cancel(json: *const c_char) -> *mut c_char {
    let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
    to_c_string(task_queue::cancel_json(&root, &read_arg(json)))
}

/// 宿主 MCP 桥：阻塞跑完 stdin 上的 JSON-RPC 会话（`lunac.exe --mcp-server` 专用）。
///
/// **必须在 Flutter 初始化之前调用**：agent 把本进程当 stdio MCP 服务器拉起，
/// stdout 只能有协议行。返回 0 = 正常结束。
#[no_mangle]
pub extern "C" fn lunac_mcp_serve() -> i32 {
    mcp_bridge::serve_stdio()
}

fn read_arg(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// 数据根目录只注入一次：共享索引 crate、使用频次、设置都从这里取 `<exe 根>`。
static ROOT_INIT: Once = Once::new();

fn ensure_data_root() {
    ROOT_INIT.call_once(|| {
        let root = exe_dir().unwrap_or_else(|| PathBuf::from("."));
        lunac_indexer::set_logger(|level, msg| {
            eprintln!("[lunac-indexer][{level}] {msg}");
        });
        lunac_indexer::init(root.clone());
        recency::init(root.clone());
        settings::init(root);
    });
}

/// 索引初始化只做一次：预热应用 / 文件索引。
/// 两个 refresh 都只在 Rust 侧起后台线程，本函数立即返回。
static INDEX_INIT: Once = Once::new();

fn ensure_indexer() {
    ensure_data_root();
    INDEX_INIT.call_once(|| {
        lunac_indexer::app_indexer::refresh_if_stale();
        lunac_indexer::file_indexer::init();
    });
}

/// `app:` / `file:` 前缀里取出绝对路径；不是这两类结果时返回 None。
fn action_path(action: &str) -> Option<&str> {
    action
        .strip_prefix("app:")
        .or_else(|| action.strip_prefix("file:"))
}

/// 内置命令过滤：直接命中（标题 / 副标题 / 关键词）在前，拼音（全拼 / 首字母）
/// 命中排在其后；两组内部都保持 CATALOG 的原顺序。
///
/// 排序口径对齐 v1 `app/src/plugins/registry.ts` 的意图（`matchTarget` 里直接命中
/// 优先、拼音 token 是补充命中），但 v1 的插件表是逐插件算分排序，这里按用户要求
/// 「拼音命中不得打乱既有相关性顺序」收敛成两档，不引入新的相关性顺序。
fn search_entries(query: &str) -> Vec<&'static Entry> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return CATALOG.iter().collect();
    }
    let mut direct: Vec<&'static Entry> = Vec::new();
    let mut via_pinyin: Vec<&'static Entry> = Vec::new();
    for entry in CATALOG {
        if entry_direct_match(entry, &needle) {
            direct.push(entry);
        } else if entry_pinyin_match(entry, &needle) {
            via_pinyin.push(entry);
        }
    }
    direct.extend(via_pinyin);
    direct
}

fn entry_direct_match(entry: &Entry, needle: &str) -> bool {
    entry.title.to_lowercase().contains(needle)
        || entry.subtitle.to_lowercase().contains(needle)
        || entry.keywords.to_lowercase().contains(needle)
}

/// 拼音 / 首字母命中：与 v1 `app/src/plugins/registry.ts` 的 `_pinyinTokens` 同口径
/// （对插件名 + 关键词生成全拼与首字母），这里对应标题 + 关键词；副标题是描述性文案，
/// 不参与拼音匹配（v1 对 description 只做包含 / 模糊，不做拼音）。
fn entry_pinyin_match(entry: &Entry, needle: &str) -> bool {
    pinyin_match(entry.title, needle) || pinyin_match(entry.keywords, needle)
}

/// 拼音命中：整串包含（全拼 / 首字母）+ **模糊**（逐音节前缀、允许跳音节）。
///
/// 模糊口径在共享 `lunac_indexer::pinyin::fuzzy_match`（2026-11-13 用户要求
/// 「拼音模糊搜索」）：`kjd` 也能命中「快捷启动」，`wxt` 命中「微信截图」。
fn pinyin_match(text: &str, needle: &str) -> bool {
    lunac_indexer::pinyin::tokens(text)
        .iter()
        .any(|token| token.contains(needle))
        || lunac_indexer::pinyin::fuzzy_match(text, needle)
}

/// 字符串数组 → JSON；序列化不会失败，兜底仍给空数组，绝不返回空指针。
fn json_array(items: Vec<String>) -> String {
    serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string())
}

/// 翻译失败 → `{"error": "一句人话"}`（Dart 侧先看 `error` 再看结果字段）。
fn translate_error_json(reason: &str) -> String {
    serde_json::json!({ "error": reason }).to_string()
}

fn to_c_string(text: String) -> *mut c_char {
    match CString::new(text) {
        Ok(s) => s.into_raw(),
        // 理论不可达：JSON 里不会出现 NUL；退化成空数组，绝不返回空指针。
        Err(_) => CString::new("[]").expect("literal has no NUL").into_raw(),
    }
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
}

fn downloads_dir() -> Option<PathBuf> {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    } else {
        std::env::var_os("HOME").map(PathBuf::from)
    }?;
    Some(home.join("Downloads"))
}

/// 打开一个本地路径（Windows 用资源管理器，Linux 用 xdg-open，macOS 用 open）。
/// 返回 0 = 已发起，3 = 失败。
fn open_path(path: &std::path::Path) -> i32 {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("explorer");
        c.arg(path);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        c
    };

    match cmd.spawn() {
        Ok(_) => 0,
        Err(_) => 3,
    }
}

/// URL 百分号编码：只保留 RFC 3986 的 unreserved 字符，其余按 UTF-8 字节转 `%XX`。
fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 用系统默认程序打开一个 URL。返回是否已发起。
#[cfg(windows)]
fn open_url(url: &str) -> bool {
    shell_open(url, None)
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) -> bool {
    Command::new("open").arg(url).spawn().is_ok()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_url(url: &str) -> bool {
    Command::new("xdg-open").arg(url).spawn().is_ok()
}

/// 在文件管理器里定位路径。Windows 用 `explorer /select,`；其它平台打开父目录。
#[cfg(windows)]
fn reveal_in_explorer(path: &str) -> bool {
    shell_open("explorer.exe", Some(&format!("/select,\"{path}\"")))
}

#[cfg(not(windows))]
fn reveal_in_explorer(path: &str) -> bool {
    let target = std::path::Path::new(path);
    let parent = target.parent().unwrap_or(target);
    Command::new("xdg-open").arg(parent).spawn().is_ok()
}

/// `ShellExecuteW("open", target, params)`：不弹控制台窗口，URL 与资源管理器都走这里。
#[cfg(windows)]
fn shell_open(target: &str, params: Option<&str>) -> bool {
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: isize,
            op: *const u16,
            file: *const u16,
            params: *const u16,
            dir: *const u16,
            show: i32,
        ) -> isize;
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let op = wide("open");
    let file = wide(target);
    let params = params.map(wide);
    let result = unsafe {
        ShellExecuteW(
            0,
            op.as_ptr(),
            file.as_ptr(),
            params.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            std::ptr::null(),
            1,
        )
    };
    // ShellExecute 的返回值 > 32 才算成功（<= 32 是错误码）。
    result > 32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_nul_terminated_and_readable() {
        let ptr = lunac_agent_status();
        assert!(!ptr.is_null(), "status pointer must never be null");
        let s = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .expect("status must be UTF-8");
        assert!(s.starts_with("agent:"), "unexpected status: {s}");
        assert_eq!(lunac_abi_version(), ABI_VERSION);
    }

    #[test]
    fn empty_query_returns_the_whole_catalog() {
        assert_eq!(search_entries("").len(), CATALOG.len());
        assert_eq!(search_entries("   ").len(), CATALOG.len());
    }

    #[test]
    fn filters_by_title_subtitle_and_keywords() {
        let by_title = search_entries("设置");
        assert_eq!(by_title.len(), 1);
        assert_eq!(by_title[0].id, "settings");

        let by_keyword = search_entries("download");
        assert_eq!(by_keyword.len(), 1);
        assert_eq!(by_keyword[0].id, "open-downloads");

        // 大小写不敏感（英文关键词）
        let by_upper = search_entries("EXIT");
        assert_eq!(by_upper.len(), 1);
        assert_eq!(by_upper[0].id, "quit");
    }

    /// 全拼命中：与 v1 `app/src/plugins/registry.ts` 的 `_pinyinTokens` 同口径
    /// （「快捷启动」→ kuaijieqidong，`qidong` 是它的后缀，v1 用 contains）。
    #[test]
    fn pinyin_full_match_hits_chinese_titles() {
        assert_eq!(search_entries("kuaijieqidong")[0].id, "quick-launch");
        assert_eq!(search_entries("qidong")[0].id, "quick-launch");
        assert_eq!(search_entries("jiantieban")[0].id, "clipboard-history");
        assert_eq!(search_entries("beiwanglu")[0].id, "memo");
        assert_eq!(search_entries("shezhi")[0].id, "settings");
    }

    /// 首字母命中：快捷启动 → kjqd、剪贴板历史 → jtbls。
    #[test]
    fn pinyin_initials_match_chinese_titles() {
        assert_eq!(search_entries("kjqd")[0].id, "quick-launch");
        assert_eq!(search_entries("jtbls")[0].id, "clipboard-history");
        assert_eq!(search_entries("bwl")[0].id, "memo");
    }

    /// 大小写不敏感（查询词统一小写后与 token 比较）。
    #[test]
    fn pinyin_matching_is_case_insensitive() {
        assert_eq!(search_entries("QiDong")[0].id, "quick-launch");
        assert_eq!(search_entries("KJQD")[0].id, "quick-launch");
    }

    /// 中英混排：英文部分走直接匹配，汉字部分走拼音（「Web 搜索」→ sousuo / ss）。
    #[test]
    fn mixed_chinese_english_entries_match_both_ways() {
        assert_eq!(search_entries("web")[0].id, "web-home");
        assert_eq!(search_entries("sousuo")[0].id, "web-home");
        assert!(search_entries("ss").iter().any(|e| e.id == "web-home"));
    }

    /// 排序：直接命中排在拼音命中之前（「ss」直接命中 AI 的 assistant，拼音命中 Web 搜索）。
    #[test]
    fn pinyin_matches_rank_after_direct_matches() {
        let hits = search_entries("ss");
        assert_eq!(
            hits[0].id, "ai",
            "直接命中（assistant）必须排在拼音命中之前"
        );
        let web = hits
            .iter()
            .position(|e| e.id == "web-home")
            .expect("Web 搜索应按首字母 ss 命中");
        assert!(web > 0);
    }

    /// 不误命中：「qd」只属于「快捷启动」的首字母（kjqd），不该带出其它条目。
    #[test]
    fn pinyin_does_not_match_entries_without_those_letters() {
        let ids: Vec<&str> = search_entries("qd").iter().map(|e| e.id).collect();
        assert_eq!(ids, vec!["quick-launch"]);
        assert!(search_entries("zzz").is_empty());
    }

    /// 模糊拼音（2026-11-13 用户要求）：允许逐音节前缀与跳音节。
    #[test]
    fn pinyin_fuzzy_hits_skipped_letters() {
        // k + j + 跳过 q + d
        assert!(search_entries("kjd").iter().any(|e| e.id == "quick-launch"));
        // 音节前缀混合：kuai + j + q + d
        assert!(
            search_entries("kuaijqd")
                .iter()
                .any(|e| e.id == "quick-launch")
        );
        // 中间的「贴板」被跳过
        assert!(
            search_entries("jls")
                .iter()
                .any(|e| e.id == "clipboard-history")
        );
        // 顺序不对仍然不命中（其它条目可能有自己的字序组合，这里只锁「快捷启动」）
        assert!(
            !search_entries("dkj")
                .iter()
                .any(|e| e.id == "quick-launch")
        );
    }

    #[test]
    fn json_payload_has_the_contract_fields() {
        let raw = lunac_search(std::ptr::null());
        assert!(!raw.is_null());
        let text = unsafe { CStr::from_ptr(raw) }
            .to_str()
            .expect("json must be UTF-8")
            .to_string();
        lunac_free_string(raw);

        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        let arr = parsed.as_array().expect("array");
        // 空查询 = 插件目录 + 最近打开的应用（索引未扫到应用时就是完整目录）。
        assert!(arr.len() >= CATALOG.len(), "内置动作不能被应用挤掉");
        for item in arr {
            for key in ["id", "title", "subtitle", "kind"] {
                assert!(item.get(key).and_then(|v| v.as_str()).is_some(), "missing {key}");
            }
            assert_ne!(item["kind"], "file", "简要搜索不得返回文件");
        }
    }

    #[test]
    fn brief_search_never_returns_files() {
        // 文件只属于详细搜索；简要搜索列表里不应出现 kind=file（2026-11-21 用户口径）。
        for query in ["a", "cargo", "test", "设置"] {
            let input = CString::new(query).unwrap();
            let raw = lunac_search(input.as_ptr());
            assert!(!raw.is_null());
            let text = unsafe { CStr::from_ptr(raw) }
                .to_str()
                .expect("json must be UTF-8")
                .to_string();
            lunac_free_string(raw);
            let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
            let arr = parsed.as_array().expect("array");
            assert!(
                arr.iter().all(|item| item["kind"] != "file"),
                "query={query} 不得返回文件：{text}"
            );
        }
    }

    #[test]
    fn recency_order_is_time_desc_with_stable_ties() {
        let mut items = vec![
            (10, 0, serde_json::json!({"id": "old"})),
            (30, 1, serde_json::json!({"id": "new"})),
            (30, 2, serde_json::json!({"id": "new-tie"})),
            (0, 3, serde_json::json!({"id": "never"})),
        ];
        sort_by_recency(&mut items);
        let ids: Vec<&str> = items
            .iter()
            .map(|(_, _, value)| value["id"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(ids, ["new", "new-tie", "old", "never"]);
    }

    #[test]
    fn run_action_reports_unknown_and_ui_owned_ids() {
        let unknown = CString::new("nope").unwrap();
        assert_eq!(lunac_run_action(unknown.as_ptr()), 1);

        let ui = CString::new("ai").unwrap();
        assert_eq!(lunac_run_action(ui.as_ptr()), 2);

        // 「用 AI 提问」兜底行同样交给 UI（打开对话面板并预填查询词）。
        let ask = CString::new("ai:how to test").unwrap();
        assert_eq!(lunac_run_action(ask.as_ptr()), 2);

        // 翻译由 Dart 面板接管。
        let translate = CString::new("translate").unwrap();
        assert_eq!(lunac_run_action(translate.as_ptr()), 2);

        // 工具编辑器（工具定义面板）同样由 Dart 接管。
        let tools = CString::new("tool-editor").unwrap();
        assert_eq!(lunac_run_action(tools.as_ptr()), 2);

        // C ABI：空输入返回 `{"error": …}`（不 panic、不碰网络）。
        let raw = lunac_translate_lookup(
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        );
        assert!(!raw.is_null());
        let text = unsafe { CStr::from_ptr(raw) }
            .to_str()
            .expect("error json must be UTF-8")
            .to_string();
        lunac_free_string(raw);
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(
            parsed.get("error").and_then(|v| v.as_str()),
            Some("没有要翻译的内容")
        );
    }

    #[test]
    fn quick_launch_is_ui_owned_and_searchable() {
        assert!(search_entries("启动器")
            .iter()
            .any(|entry| entry.id == "quick-launch"));
        let id = CString::new("quick-launch").unwrap();
        assert_eq!(lunac_run_action(id.as_ptr()), 2);
    }

    #[test]
    fn action_path_strips_app_and_file_prefixes() {
        assert_eq!(action_path("app:C:\\Tools\\a.exe"), Some("C:\\Tools\\a.exe"));
        assert_eq!(action_path("file:D:\\Docs\\b.pdf"), Some("D:\\Docs\\b.pdf"));
        assert_eq!(action_path("open-exe-dir"), None);
    }

    #[test]
    fn url_encode_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("你好"), "%E4%BD%A0%E5%A5%BD");
        assert_eq!(url_encode("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn ocr_action_is_ui_owned_and_status_is_json() {
        let id = CString::new("ocr").unwrap();
        assert_eq!(lunac_run_action(id.as_ptr()), 2);

        // 状态 JSON：本机开发目录一般没装引擎，但形状必须稳定（面板靠它降级）。
        let raw = lunac_ocr_status();
        assert!(!raw.is_null());
        let text = unsafe { CStr::from_ptr(raw) }
            .to_str()
            .expect("status json must be UTF-8")
            .to_string();
        lunac_free_string(raw);
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert!(parsed["installed"].is_boolean());
        assert_eq!(
            parsed["searchPaths"].as_array().map(|paths| paths.len()),
            Some(2)
        );

        // 空路径的识别请求：一句人话错误，不 panic、不真起子进程。
        let raw = lunac_ocr_image(std::ptr::null());
        assert!(!raw.is_null());
        let text = unsafe { CStr::from_ptr(raw) }
            .to_str()
            .expect("image json must be UTF-8")
            .to_string();
        lunac_free_string(raw);
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert!(parsed["error"].as_str().is_some_and(|e| !e.is_empty()));
    }

    #[test]
    fn music_action_is_ui_owned_and_lyrics_errors_are_json() {
        // 音乐由 Dart 面板接管（本地曲库 / 歌词走独立的 C ABI）。
        let id = CString::new("music").unwrap();
        assert_eq!(lunac_run_action(id.as_ptr()), 2);

        // 空路径：一句人话错误，不 panic、不碰磁盘之外的任何东西。
        let raw = lunac_music_lyrics(std::ptr::null());
        assert!(!raw.is_null());
        let text = unsafe { CStr::from_ptr(raw) }
            .to_str()
            .expect("lyrics json must be UTF-8")
            .to_string();
        lunac_free_string(raw);
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert!(parsed["lines"].as_array().is_some_and(|lines| lines.is_empty()));
        assert!(parsed["error"].as_str().is_some_and(|e| !e.is_empty()));
    }
}
