# Lunac v2（Flutter）迁移文档

> **状态**：2026-11-03 立项；2026-11-05 用户裁决**目标改为 Windows + Linux 双端共用**，
> 开发顺序 = **Windows 先跑通，再补 Linux 验收**。**M0 已在 Windows 闭环**；
> **M1（搜索主页面）Windows 主链路已跑通**（Rust 搜索 C ABI + Dart 搜索页 + 无边框置顶窗 + 托盘），
> 应用 / 文件数据源与 Linux 补验未完成。Linux 侧待补验。
> **适用范围**：本仓库（更名后为 `lunac-master`）下的第二代前端。
> **配套文档**：v1（Tauri + WebView2）的文档已**冻结归档**到
> [`archive/tauri-v1/`](./archive/tauri-v1/README.md)，只读、不再更新；v1 的运行规则仍以
> [`forbidden.md`](./forbidden.md) / [`code-standards.md`](./code-standards.md) 为准，直到 v2 另立规则。

---

## 一、为什么换、换什么

- **动机**：目标是一个 **Windows 与 Linux 共用的桌面应用**（2026-11-05 用户明确），
  同时不再背 WebView2/WebKitGTK 的渲染差异（视频 MSE、透明窗、CSS 兼容）。
- **用户口径（2026-11-05）**：**先 Windows 跑通，再迁 Linux**；不是 Linux 单端替换。
- **换的只有表现层**：`app/`（Tauri 宿主 + `app/src` 前端 + `app/src/plugins` 的 TS 插件）
  换成 `app-flutter/`（Flutter 桌面壳 + Dart UI）。
- **不换的**：`core-agent/`（agent 全部逻辑）、`agent` 的 **stream-json 协议**、
  记忆 / 成本 / 检查点 / 任务队列的数据格式。
  **例外（2026-11-15 更正）**：v1 `app/src-tauri/src/native_proxy/`（21 个文件）已随目录误删
  **永久丢失**（快照与全盘搜索均无），自研代理引擎改在 `app-flutter/rust/src/native_proxy/`
  **按能力账本重建** —— 逐项状态只看 [`acceptance-map.md`](./acceptance-map.md) §二bis，
  不要再引用 §三 的 v1 路径当可运行代码。

### 双端架构原则（2026-11-05 裁决）

1. **业务逻辑只写一份**：agent、代理引擎、文件索引、成本算法等都在 Rust；Flutter 只做表现与平台壳。
2. **平台差异必须收口**：Windows / Linux 的差异只允许出现在 `app-flutter/rust/`（平台能力）
   与 `lib/platform/`（Dart 侧接口）两个薄层；禁止在业务代码里散落 `Platform.isWindows`。
3. **先 Windows 验收、Linux 补验**：M0–M2 以 Windows 为开发端；每个里程碑在 Linux 上补跑
   一次同一份代码，缺口登进本文件而不是"以后再说"。
4. **数据目录可迁移**：便携式数据根（exe 同级）在 Windows 保留；Linux 用同一套相对布局，
   不把路径硬编码成盘符。

```
lunac-master/
├── core-agent/          # 不变：agent（Rust）
├── app/                 # v1：Tauri + WebView2，冻结保留（可编译、不再加功能）
├── app-flutter/         # v2：Flutter 桌面（新增）
│   ├── lib/             # Dart UI
│   ├── rust/            # 复用宿主能力的 Rust 静态库（cdylib，flutter_rust_bridge 或 C ABI）
│   └── linux/ windows/  # 平台壳
├── docs/                # 现行文档；v1 快照在 docs/archive/tauri-v1/
└── scripts/             # 构建脚本（v1 的保留，新增 v2 的）
```

## 二、明确不做的事

1. **不新建仓库、不新建工程**：v2 与 v1 同仓库，共享 `core-agent` 与协议；避免两套 agent 分叉。
2. **不改 agent 协议**（`stream-json` 事件与 `control_response` 审批）：这是 v1/v2 共用的地基，
   换 UI 不得动它（见 `archive/tauri-v1/` 里的协议说明）。
3. **不在 v2 里另立第二套代理引擎语义**：v1 源码丢失后已按能力账本在
   `app-flutter/rust/src/native_proxy/` 重建（现含 SS AEAD / **SS-2022（SIP022）** / Trojan /
   VLESS / VMess / AnyTLS + 规则 + TCP/TLS/WS 传输）；未重建的协议一律**如实报错**，
   不静默降级，状态以 [`acceptance-map.md`](./acceptance-map.md) §二bis 为准。
4. **不并行维护两套前端功能**：v2 达到 M3 之前，v1 只修阻断性缺陷，不加新功能。

## 三、能力映射（v1 → v2）

| 能力 | v1（Tauri/WebView2） | v2（Flutter/Linux） | 备注 |
|---|---|---|---|
| 窗口 / 透明 / 置顶 | Tauri 窗口 + layered window | `window_manager` + 平台通道 | Linux 按 X11 / Wayland 分别验证 |
| 托盘 | `tauri` tray-icon | `tray_manager` 0.8（Linux 走 StatusNotifierItem / D-Bus） | 不需要 `libayatana-appindicator3`；GNOME 需 AppIndicator 扩展（Ubuntu 自带） |
| 开机自启 | 注册表 `Run` | `~/.config/autostart/*.desktop` | 由 Rust 侧按平台实现 |
| 系统代理 | WinINet / 注册表 | 环境变量 + `gsettings`（GNOME） | 各桌面环境不一致，需如实提示 |
| 抓屏 | GDI BitBlt | `grim`（Wayland）/ X11 `xwd` 或 `libpipewire` | 桌宠「看一眼屏幕」 |
| 语音输入 | Windows `SpeechRecognizer` | 暂无等价内置 → 先标记不可用 | 不假装支持 |
| OCR | Windows OCR / PaddleOCR-json | PaddleOCR-json（Linux 版） | 依赖分发方式待定 |
| 视频播放 | `<video>` + MSE | **libmpv 旁路**（已有 `plugin_mpv_*` 思路） | WebKitGTK 无 MSE，不能走 hls.js |
| Live2D | pixi.js + Cubism Core(WASM) | Flutter 的 Live2D 封装或 `texture` + 原生 GL | 透明窗与 GPU 是主要风险 |
| 插件 | TS 包 + WebView 面板 | **先做原生内置页**（M5a 起；剪贴板历史已落地）；**插件市场已接**（M5n：索引 / 下载 / sha256 / 安装 / 卸载，落点与 v1 共用 `Modules\<id>`）；**第三方插件运行已接**（M5z-补13：v1 的 sidecar 契约 —— 任意语言的插件进程 + 信任门 + Job Object，界面走 Flutter 控制台，不开第三方 WebView） | 已验证「任意语言」这条路；插件自带 UI 仍不渲染 |
| 主题 | CSS 变量 + 四类颜色 | Flutter ThemeData + 同一套色值 | 观感对齐需要独立验收 |

## 四、里程碑

> **顺序调整（2026-11-05 用户裁决）**：搜索主页面是产品核心与日常入口，先做；
> 对话主链路随后。窗口形态（无边框 / 置顶 / 尺寸）与平台薄层因此在 M1 就定下来。

| # | 内容 | 出口条件 |
|---|---|---|
| M0 | 环境与骨架 | **Windows 先跑通**：`flutter run -d windows` 出空白窗口；Rust 侧能通过 C ABI 返回 `agent` 状态。随后同一份代码在 Linux 上补跑一次 |
| M1 | **搜索主页面** | 无边框 / 置顶 / 680×400 窗口；搜索框 + 结果列表 + 键盘上下/回车/Esc；数据源先接内置命令与系统动作，再抽 `app_indexer` / `file_indexer` 接应用 / 文件；结果动作可执行（Windows 验收，Linux 补验） |
| M2 | 对话主链路 | 输入 → `agent.exe` → stream-json → Dart 渲染文本 / 思考 / 工具卡；审批卡可放行（复用 M1 同一窗口的 AI 态） |
| M3 | 核心面板 | 设置、历史会话、用量、待办抽屉；代理面板只做「节点列表 + 开关」 |
| M4 | 平台能力 | 托盘、自启、系统代理、透明置顶窗在 **Windows 与 Linux 各一条真机验收**；差异只进 `rust/` 与 `lib/platform/` |
| M5 | 媒体与桌宠 | 视频走 libmpv；Live2D 上屏；插件层方案定稿 |
| M6 | 打包与替换 | Windows 安装包 + Linux `.deb` / AppImage；v1 → v2 数据目录兼容读取 |

> **M1 进展（2026-11-05 起，Windows）**：Rust 薄层已提供 `lunac_search` / `lunac_run_action` /
> `lunac_index_init` / `lunac_clipboard_text` / `lunac_app_icon` / `lunac_reveal_path` /
> `lunac_single_instance_*` / `lunac_hotkey_start`（ABI 2），Dart 侧搜索页（上下键 / 回车 / Esc /
> hover / 右键菜单 / 动作执行）、无边框置顶 680 宽、**高度随结果行数伸缩**的窗口、托盘已实跑通过；
> 顶部隐形拖拽条已去掉，**拖拽改由搜索栏背景承担**；底部状态栏已删除，结果行副标题平时隐藏、
> 鼠标进入时动画显示；**真实应用图标**（`indexer/src/icon.rs` + 路径缓存）。
> Esc 语义与 v1 一致：有查询词先清空，空白才隐藏。**数据源已接全**（2026-11-07）：应用 / 文件索引
> 抽到共享 crate [`indexer/`](../indexer)（v1 与 v2 同一份实现，根目录与 logger 由宿主注入），
> `lunac_search` 合并内置动作 + 应用 + 文件 + Web 搜索兜底。**平台唤出链已接**（2026-11-08）：
> 全局热键 `Ctrl+Alt+Space`（`RegisterHotKey`，配置 `config/hotkey.json`）、单实例唤回
> （命名互斥体 + 事件，迁移期用 `LunacV2*` 命名避免与 v1 互斥）、**事件驱动唤出**
> （Dart 把主窗 HWND 交给 Rust，热键 / 再启动直接 `PostMessage(WM_APP+1/+2)` → runner
> `lunac/wake` 通道 → Dart，后台零轮询；旧 DLL 退回 120/250ms 轮询兜底）、
> 失焦 150ms 自动隐藏、
> 唤出时剪贴板文本带入并**重跑当前查询**、使用频次排序。主页面细节对齐 v1：空态有
> 「Web 搜索」行、右键 Web 行切换 Google / Bing / 百度（`config/search.json`）、
> 输入去抖 60ms、hover 只做视觉高亮不抢键盘选中。**附件泡泡已接**（2026-11-08）：
> runner 的 `WM_DROPFILES` → `lunac/drop` 通道 → Dart 泡泡；粘贴 / 拖放 / Backspace / Esc
> 删除、附件优先视图（打开附件 + AI 占位），窗口高度随泡泡行伸缩。**仍缺**：附件上的
> OCR / 转换 / AI 动作（M2/M4）、插件层（M4）、AI 对话（M2）与 Linux
> 补验；细节见 [`todo.md`](./todo.md) §待办 1-14。**主页面视觉（2026-11-08）**：runner 侧窗口为
> `WS_POPUP` + 透明（无边框 / 无 DWM 投影与描边，窗口边界不可见），只画「搜索框 + 结果区」，
> **零间隙拼接**、接缝 1px（`LunacColors.divider`）；圆形设置按钮内嵌在搜索栏右端（40px 中空）。
> 首次启动只显示圆钮、搜索栏从其**左缘**向左拉开，且**不聚焦**；结果区在搜索框第一次拿到
> 焦点后以 130ms 展开，**失焦不收起**（展开一次即保持，避免点结果行时面板先收起导致点击
> 落空），窗口 **680×68–350**。
> **附件抽屉是独立渲染的窗口**（第二个 `FlutterEngine`，runner `attachment_window.cpp` +
> `lib/attachment_drawer_main.dart`），吸附在主窗口搜索栏左端：一整块纯色底板 + 文件
> **纵向罗列**（宽度 150–240、高度封顶 334，超过在面板内滚动，不越过主面板底边）。
> 出现时底板向左拉开、内容右锚定（动画只揭开面板，不滑动文件）；窗口尺寸只在开始 / 结束时
> 各下发一次，180ms 的揭开完全在 Flutter 内完成（逐帧改窗口会重建 swapchain 掉帧）；
> 空列表时隐藏。
> 抽屉右缘是直角、贴在主窗 `left + 12`，因此**有附件时搜索栏左上角改为直角**（无附件时仍是
> 12 圆角），抽屉顶到面板底边时左下角同样改直角，由抽屉自己的左端圆角收口，接缝严密贴合，
> 1px 分界线由搜索栏描边提供。第二个 engine 在主窗首帧后 600ms 预热（宽度 0 时隐藏），
> 首次粘贴不再现建引擎；隐藏 / 退出时两个窗口在同一帧一起消失。两块窗按「同一进程在
> 前台」判定失焦隐藏（抽屉点击不激活、`WM_MOUSEACTIVATE → MA_NOACTIVATE`），点抽屉时
> 主窗不会藏、搜索框也不丢键盘焦点，视觉与行为上都当成一整块。
> 搜索栏左上角的猫**已取消绘制**（2026-11-08 用户裁决），相关 `cat_window` / `cat_overlay`
> 产物已全部删除，顶部不为它留白。**详细搜索大界面已接**（2026-11-09）：双击搜索栏进入，
> 固定 640 高，7 个分类 Tab（全部 / 应用 / 文件 / 设置 / 系统动作 / 命令 / 网络搜索）+
> 文件类型胶囊，↑↓ / Enter / Esc / Tab；数据走 `lunac_search_apps` / `lunac_search_files` /
> `lunac_system_catalog`（41 个设置页 + 26 个白名单动作，关机 / 重启二次确认，
> 可提权动作右键「以管理员身份运行」）；底栏显示文件索引状态（未载入 / 扫盘中只给转圈，
> 就绪显示「已索引 N 项」，2 秒轮询，就绪后补跑一次查询），头部有当前分类计数与
> 「重建文件索引」按钮（`lunac_index_status` / `lunac_index_refresh`）；
> Shift+Enter = 提权运行（应用 / 文件 / 白名单动作），分类无结果时回车 = 网页搜索兜底。
> 选中应用 / 文件行时右侧出现 232px 预览区：图片真缩略图（`Image.file` + `cacheWidth`）、
> 其余用类型图标，附「类型 / 大小 / 修改时间 / 路径」与「打开 / 定位 / 复制路径」
> （`platform/file_preview.dart`）。结果已按「应用 / 文件 / Windows 设置 / 系统动作 / 命令 /
> 网络搜索」分组（2026-11-09，M1z；键盘选中滚动按可变高列表项累加偏移）。
> **设置面板第一版已接**（2026-11-09，M3 切片）：搜索栏圆钮 / 「设置」结果项打开，
> 与详细搜索同高（640 固定档）；分区 = 搜索（默认引擎切换，立即生效并写入
> `config/search.json`）、AI 模型（9 个供应商预设 + 模型 / Base URL / API Key，保存到
> `config/ai.json`，与现有文件合并、未编辑键保留；`lunac_ai_config` / `lunac_set_ai_config`）、
> 文件索引（状态 + 重建，2 秒轮询）、关于（数据目录）；
> Esc / 返回关闭，窗口再唤出时回到简洁搜索。**「通用」分区已接热键改键与开机自启**
> （2026-11-09，M3c / M3d）：热键录制走 `lunac_hotkey_combo` / `lunac_hotkey_set`（Rust 在
> 热键线程上换注册，失败回滚旧键，成功才写 `config/hotkey.json`）；开机自启移植 v1 的
> 「登录时计划任务 + HKCU Run 双机制」（`lunac_auto_start_info` / `_set` / `_elevated`，
> 提权子进程是本 exe 的 `--lunac-auto-start-task=` 开关，`main()` 在建窗前执行并退出；
> `--background` 静默进托盘）；开发构建禁用开关并写明原因。**检查更新已接「只检查」**
> （2026-11-10，M3e）：`platform/updater.dart` 用 `dart:io` 拉 GitHub 的 `latest.json`
> （固定入口、20s 超时、64KB 上限），校验口径与 v1 一致（版本可解析 / https + GitHub 域 /
> sha256 64 位十六进制）；**M5v 起这一行并回「常规」分区**（v1 的位置：状态文字 = 当前版本
> vX / 有新版本 vX / 已是最新版本，按钮 = 检查更新 + 下载并安装，不再有「前往下载」外链）。
> 安装包下载 / sha256 实算 / 静默安装 / 启动自动检查随 M6 安装器接入。
> **外观只保留主题包展示**（2026-11-10 M3f → 2026-11-14 M5v 调整）：`rust/src/appearance.rs`
> 移植 v1 —— 系统强调色 / 深浅色与 `<exe 根>/themes` 扫描（tokens / assets 解析 + 三层
> 路径穿越防护，内置 default 以 `include_str!` 为准）；FFI `lunac_system_theme` /
> `lunac_themes` / `lunac_themes_dir` 都在，但 **v1 前端已无系统主题调用方**，所以设置
> 「风格」分区不再显示系统色块，只按 v1 画「主题目录 + 打开目录 + 主题包泡泡框」
> （主色圆点 / 名称 / 内置徽标，默认主题带选中描边）。tokens / 背景 / 图标的**颜色与背景
> 编辑**（需要运行时色板，`LunacColors` 目前是 const）属前端改版，**M6 打包时要把
> `app/src-tauri/themes` 拷到 `<exe 根>/themes`**。
> 外观与插件分区随各自里程碑接入。
> **插件层先走「原生内置页」路线**（2026-11-10，M5a）：Flutter 没有 DOM，v1 的 TS + WebView
> 插件无法直接搬；`clipboard-history` 已作为第一个原生内置页接入 —— Rust 侧
> `clipboard_history.rs` 自管 `<exe 根>/ModuleData/history/clipboard-history.json`
> （文件名与字段与 v1 一致，去重 / 置顶 / 30 条 / 2000 码元上限逐条对齐），
> 唤出时由 `main.dart` 顺手记录；搜索「剪贴板」进入 640 档原生面板（点击复制并置顶、
> 清空二次确认）。**备忘录**（M5b）是第二个原生内置页：`memo.rs` 自管
> `<exe 根>/ModuleData/memo/memo.json`（字段与 v1 一致），新条目置顶、编辑保位；
> 标识（`tag`）做精确 / 前缀匹配并**插入 `lunac_search` 结果最前**（`memo:<id>`），
> 面板为「编辑器 + 历史（复制 / 删除 / 清空）」；图片与跨重启草稿尚未接。
> 其余内置 / 市场插件（翻译 / 快捷启动 / OCR / 文件转换 / 音乐）
> 已逐个接入原生内置页；**插件市场本身已接**（2026-11-13，M5n）：
> `rust/src/plugin_market.rs` 照 v1 的判据拉索引 / 下载 https zip / sha256 /
> 解压到 `<exe 根>/Modules/<id>` / 卸载（依赖只支持 `file` 形态），设置「插件」栏
> 就是这张表。**第三方插件代码已可运行**（2026-11-15，M5z-补13）：走 v1 的
> **sidecar** 契约 —— 插件在清单里声明自己的可执行文件（Python / Node / Go / Rust
> 都行），宿主起进程 + NDJSON / 本机端口通信 + 收尾，信任门与 Job Object 同 v1；
> **v2 仍不开第三方 WebView**（桌宠 Live2D 是唯一例外），插件的界面由 Flutter 的
> 「插件运行时」控制台承载。
> **AI 沙箱盘预览窗已接**（2026-11-14，M5o）：`rust/src/sandbox.rs` 把 agent 的受控工作目录
> `<exe 根>/temp/transStorage` 当成固定额度（512 MiB，`config/sandbox.json` 可改）的小盘，
> 提供只读的用量 / 清单 / 预览与清空；预览窗复用插件窗口基础设施（id `sandbox`，860×560，
> 不占「最多两个」额度，贴在调用窗口旁），AI 对话标题栏的「沙箱盘」按钮打开它。
> **它不是 OS 级隔离、不是虚拟机**（见 [`sandbox-research.md`](./sandbox-research.md)），
> 文案统一写「受控工作目录 / 沙箱盘」。桌宠的 Live2D 渲染已在 `rust/src/pet.rs` 用 WebView2
> 落地（透明窗 + 控制台），本轮未改。
> **插件版式开始逐个对齐 v1 当前形态**（2026-11-14，M5p）：文件转换窗口已按 v1 2026-10-24
> 重构版排布（顶部引擎 + 保存目录 / 左四页签 / 输入→中间→输出 / 底部消息），并补了
> 系统多选文件框与「选择保存目录」（Rust 手写 shell32 ABI，避免 `windows` crate 的
> `Win32_UI_Shell_Common` feature 在本机触发 dlltool 缺失）。其余插件按同一方法逐个比对。
> **对话主链路第一版已接**（2026-11-09，M2 切片）：`rust/src/agent.rs` 照搬 v1 的 spawn
> 参数（stream-json 双向、`--permission-prompt-tool stdio`、`--permission-mode acceptEdits`）与
> 凭据注入（`config/ai.json` → `LUNAC_AGENT_BASE_URL/TOKEN/MODEL`），stdout 逐行入队、
> stdin 常驻；FFI `lunac_agent_start/stop/running/send/poll`；Dart 40ms 轮询 +
> `chat_model.dart` 折叠（增量与整包去重、工具卡、审批卡、`result` 收尾）+ 640 高对话面板
> （「AI 对话」/「AI 分析附件」，关闭面板不杀常驻 agent，退出时显式 stop）。
> **四类审批已全部接入**（2026-11-09，M2d）：`can_use_tool`（允许 / 拒绝）、
> `elicitation`（按 `requestedSchema` 铺表单，提交值走 `updatedInput`）、
> `mcp_trust`（仅本次 / 始终信任〔`{remember:true}`〕/ 拒绝）、
> `mcp_sampling`（允许本会话 / 拒绝，HTTP 反向请求用警告色，不提供始终允许）。
> 未接：`--mcp-server`（等 v2 的 MCP 服务端，M4）。
> **事件面已接**（2026-11-09，M2f）：`system/plan_mode` → 单例计划模式横幅（进 / 出只更新
> 同一条，可带 `reason`）；`system/hook_note` → 一条旁注（逐条 `[kind] text — command`）；
> `system/user_file` → 文件呈递卡片（名称 / 路径 / 大小·类型 / 预览 + 打开〔`file:<path>`〕/
> 定位）；`system/push_notification` → 应用内旁注；`system/mcp_config_changed` → 如实提示
> 「重启 agent 后生效」。v1 没有 @ 文件交互，面板交互只差 M4 的 MCP 相关项。
> **待办抽屉已接**（2026-11-09，M2g）：`TodoWrite` 的清单整体替换成 `TodoItem`，
> 输入框上方常驻「待办 N/M + 当前项」可展开（最多 140 高）；该工具不占命令卡、结果也
> 不进对话流。「改动文件 / 后台任务」两页依赖文件快照与任务队列，随 M4 / M6 一起接。
> **斜杠命令已接**（2026-11-09，M2e）：`/help`、`/compact`（发 `{"type":"compact"}`，
> 占位旁注在 `system/context_compacted` 到达时原地更新）、`/rewind`（删最近一轮并
> `set_history` 回灌；二次确认；v2 无文件快照，不还原磁盘文件）、`/memory [clear]`
> （`rust/src/memory.rs` 读 `ModuleData/memory/MEMORY.md` + `pages/*.md`，清空 = 截断 +
> 删页；FFI `lunac_memory_read` / `lunac_memory_clear`）。候选菜单 ↑↓ / Enter·Tab 采纳
> （只填不执行）/ Esc 收起；`budget_extended` / `budget_exhausted` / `rounds_exhausted`
> 也已落成状态旁注。
> **附件图片已随消息发送**（M2b）：vision 开时图片附件追加 `image` 块（只带路径，
> agent 自己读字节），vision 缺省 = 开；图片路径仍保留在提示词文本里。
> **对话历史已接**（2026-11-09，M2c）：`rust/src/chat_history.rs` 自管
> `<exe 根>/ModuleData/history/chat-history.v2.json`（会话形状同 v1 `ChatSession`，
> upsert 按 id 覆盖、按 `updatedAt` 降序、上限 100 条 / 200 消息 / 单条 20000 字，
> 临时文件 + rename）；FFI `lunac_chat_history` / `lunac_chat_history_save` /
> `lunac_chat_history_delete`；面板标题栏「历史」按钮展开列表（标题 / 相对时间 / 条数 /
> 删除），点一条 = `restore()` 重建界面 + `set_history` 回灌 agent（只换上下文、
> 不触发模型调用）；落盘时机 = 一轮结束 / 关面板 / 新对话 / 恢复前。v1 的 `chat.db`
> 在 M6 统一迁移。
> **图书馆式检索库**（2026-11-20，用户口径）：`rust/src/library.rs` 在
> `<exe 根>/ModuleData/library/library.db` 建 SQLite 索引，四层固定为
> **日期 → 对话 → 任务 → 操作**；任务标题取该轮用户提问的首行概括（不再用「任务 N」），
> 每条操作带 `tags` 与 `priority`。`chat_history` 保存 / 删除时增量重建索引，
> `SessionSearch` 支持 `query + tags / date / session / task` 收窄，只返回命中的操作，
> 不再把整段历史塞进上下文。长期记忆写入后同步同一套结构（scope = `global` /
> `pet` / `skill:<key>`），额度从 9000/6000/2000 调高到 24000/16000/4000
> （总长 / 正文 / 单条），agent 侧 `MEMORY_INJECT_CHARS` 同值。
> 参考图待用户提供，后续只调视觉参数。

## 五、环境准备

### Windows（开发端，先跑通）

**已完成（2026-11-05）**：Flutter 3.47.9 stable（`D:\flutter\flutter`）+ VS 2022 Build Tools
（Desktop development with C++ / VCTools），`flutter doctor` 的 Visual Studio 行已是 [√]；
`flutter build windows --debug` 成功，`lunac.exe` 能起窗。

若换机器或重装，需要：

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools -e
# 安装器里勾选 "Desktop development with C++"；装完 flutter doctor 该行应变 [√]
```

### Rust 侧薄层（M0 已接通，Windows）

- `app-flutter/rust/` 是 **cdylib（debug）**，导出 `lunac_abi_version` / `lunac_agent_status` /
  `lunac_search` / `lunac_search_apps` / `lunac_search_files` / `lunac_system_catalog` /
  `lunac_index_status` / `lunac_index_refresh` / `lunac_data_dir` / `lunac_run_action` /
  `lunac_index_init` / `lunac_agent_start` / `lunac_agent_stop` / `lunac_agent_running` /
  `lunac_agent_send` / `lunac_agent_poll` / `lunac_ai_config` / `lunac_set_ai_config` /
  `lunac_chat_history` / `lunac_chat_history_save` / `lunac_chat_history_delete` /
  `lunac_memory_read` / `lunac_memory_clear` / `lunac_hotkey_combo` / `lunac_hotkey_set` /
  `lunac_auto_start_info` / `lunac_auto_start_set` / `lunac_auto_start_elevated` /
  `lunac_system_theme` / `lunac_themes` / `lunac_themes_dir` / `lunac_open_url` /
  `lunac_clipboard_history` / `lunac_clipboard_history_append` / `lunac_clipboard_history_promote` /
  `lunac_clipboard_history_clear` / `lunac_memo_list` / `lunac_memo_save` / `lunac_memo_upsert` /
  `lunac_memo_delete` / `lunac_memo_clear` / `lunac_custom_apps` / `lunac_add_custom_app` /
  `lunac_remove_custom_app` / `lunac_clipboard_payload` / `lunac_free_string`；
  Dart 侧唯一库加载入口是 `lib/platform/native_bridge.dart`（ABI 校验也在这里）。
- **应用 / 文件索引不在薄层里**：实现在仓库根 [`indexer/`](../indexer)（`lunac-indexer` crate），
  v1（`app/src-tauri`）与 v2（`app-flutter/rust`）都依赖它；数据根目录用 `init(root)` 注入、
  日志用 `set_logger()` 接管，两个宿主各自传自己的可执行文件目录。
- `windows/runner/CMakeLists.txt` 在 `flutter build windows` 时调用 `rust/build.ps1`（只跑
  `cargo build`，**不跑 release**）并把 `lunac_core.dll` 拷到可执行文件同目录。
- 验证口径：`flutter run -d windows` 的日志出现
  `Lunac M0 agent status: agent: not started (M0 stub)`。
- **Linux 的等价构建 / 拷贝规则待补**（M0 的 Linux 补验时一起做，不在 Windows 阶段写未验证的 CMake）。

### 常用命令（便携，任一目录可执行）

```powershell
# 只做静态检查 + 单测（最快）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\v2-dev.ps1 -Check -Sandbox
# 构建 debug（内部自动构建 Rust 薄层并拷 DLL）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\v2-dev.ps1 -Build -Sandbox
# 启动窗口（默认动作；日志里能看到 agent 状态）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\v2-dev.ps1 -Run -Sandbox
# 直接启动已构建的 debug 产物（不编译；首次需先 -Build）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\v2-dev.ps1 -Launch -Sandbox
```

- **不要反复重跑 `-Run`**：它每次都会走一遍 Flutter 的增量构建（即使没有改动，也要跑 assemble + 链接，实测约 8–15s）。
  交互式开发请让它**常驻**，在它的终端里按 `r` 热重载、`R` 热重启、`q` 退出。
- **`-Launch`** 不调用 Flutter 工具链，直接运行已有的
  `build\windows\x64\runner\Debug\lunac.exe`（Linux 为 `build/linux/x64/debug/bundle/lunac`），
  起窗是秒级；应用会继承当前控制台，调试日志与 Dart VM Service URL 仍可见。代价是没有热重载，
  只适合快速看界面或验收（需要热重载时仍用 `-Run` 常驻 + `r`/`R`）。

- 脚本从自身位置推导仓库根，**无绝对路径**；不加参数时默认 `-Run`。
- `-Sandbox` 只在 Trae 沙箱里需要（它禁止写 `%LOCALAPPDATA%\.dartServer`，脚本会把
  `LOCALAPPDATA` 指到 `<repo>\temp\dart-home\LocalAppData`）；普通 PowerShell 不加。
- 非 Windows 上同一脚本自动改用 `-d linux`（需 PowerShell 7+）。
- 三条命令都只产出 **debug**，符合 §六 的「改造期禁止 release」。

> **插件软链坑（2026-11-05 实测）**：项目里同时存在 `windows/` 与 `linux/` 时，`flutter build windows`
> 会为**两个平台**建插件软链；若 `linux/flutter/ephemeral/.plugin_symlinks` 不存在，会报
> `Flutter failed to create a link … .plugin_symlinks/…`（errno 2，实际是父目录缺失）。
> 先建出该目录再构建即可，与业务代码无关。

### Linux（第二验收端）

```bash
# 1. 基础
sudo apt update
sudo apt install -y build-essential pkg-config libgtk-3-dev libx11-dev libxi-dev \
  libmpv-dev libssl-dev clang cmake ninja-build

# 2. Rust（与 v1 同版本工具链）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add x86_64-unknown-linux-gnu

# 3. Flutter（stable）
git clone https://github.com/flutter/flutter.git -b stable ~/flutter
export PATH="$PATH:$HOME/flutter/bin"
flutter doctor -v        # Linux toolchain 必须全绿
flutter config --enable-linux-desktop
```

**验收**：`flutter doctor` 的 Linux toolchain 全绿；`cargo build --release`（`core-agent`）在 Linux
上能过；`app-flutter` 空壳能起窗。

> ⚠️ **脚本编码（2026-11-03 实测）**：本机 PowerShell 5.1 会把**无 BOM 的 UTF-8 `.ps1`**
> 按 GBK 解码 —— 中文注释会变成乱码并**吃掉引号**，脚本直接语法错误（`字符串缺少终止符`）。
> 因此给用户手动执行的安装脚本一律**纯 ASCII**（或确保恰好一份 UTF-8 BOM，见
> `archive/tauri-v1/` 的 BOM 规则）。

## 六、备份、归档与 GitHub 纪律（2026-11-03 起）

- **GitHub 维护暂停**：本地继续提交，但**不再 push**、不跑 `publish-*.ps1`、不发 Release；
  恢复时机由用户指定。
- **迁移前备份**（每轮重大改动前重做一次）：
  1. `git bundle create <外部目录>\lunac-<日期>.bundle --all` —— 完整历史；
  2. `git tag backup-before-flutter-<日期>` —— 便于回滚定位；
  3. `docs/archive/tauri-v1/` —— v1 文档的**冻结快照**（只读）。
- **目录更名**：`claude-code-cli-master` → **`lunac-master`**（仅目录名，仓库内容与 `.git` 不动）。
- **禁止**：为了让 v2 跑起来而改 `core-agent` 的协议或 `native_proxy` 的行为；如确实需要，
  先在 [`todo.md`](./todo.md) 记一条，并同步 v1 的验收账本。
- **架构改造期禁止构建 release（2026-11-05 用户裁决；2026-11-19 用户明确要求打包后恢复）**：
  - 2026-11-05 ~ 11-18 只做 debug 构建与验证（`cargo build`、`flutter build windows --debug`），
    不跑 `cargo build --release` / NSIS，不覆盖 `D:\Lunac` 的已安装版本。
  - **2026-11-19 用户明确要求出 release 安装包** ⇒ 该禁令解除，v2 打包入口固定为
    `scripts\build-release-v2.ps1`（`flutter build windows --release` + `core-agent` release
    `agent.exe` + `scripts\lunac-v2-installer.nsi`）。Rust 薄层按 `$<CONFIG>` 取
    `rust\target\{debug,release}\lunac_core.dll`，不再「Release 界面 + Debug DLL」。
  - 除该脚本外，日常仍只走 debug；"顺手打个 release" 依旧不算验收证据。

## 七、风险清单（按影响排序）

1. **插件层**：v1 的 20+ 个插件是 TS + DOM；Flutter 里没有 DOM。**已定案（2026-11-15）**：
   内置插件走原生 Flutter 面板；第三方插件走 v1 的 **sidecar** 契约（任意语言的插件进程 +
   信任门 + Job Object，界面由「插件运行时」控制台承载），**不开第三方 WebView**。
   仍未做：sidecar 能力暴露成 agent 工具、插件清单里的 `autostart`。
2. **视频**：必须走 libmpv 旁路；`<video>` + MSE 的旧路在 Linux 上不可用。
3. **透明窗 / 桌宠**：Wayland 下合成器差异大，可能需要 X11 会话兜底。
4. **中文输入法**：Flutter Linux 的 IME 需要真机验证（fcitx5 / ibus）。
5. **回归成本**：v1 的 16 条真机流程全部要在 v2 重跑一遍（见
   [`acceptance-map.md`](./acceptance-map.md)），这是换框架的主要时间成本。
