# Lunac 项目规范

> **回归保护 — 最优先原则**
>
> 修改任何已有功能的代码前，**必须确保之前的功能不受影响**。每次改动后需验证以下核心行为没有被破坏：
> - 热键唤出/隐藏（默认 Ctrl+Alt+Space）
> - ESC 逐级清除与自动隐藏
> - 唤出应用后 Lunac 自动隐藏
> - AI 对话发送/中止/流式输出/模式切换
> - 搜索匹配、文件拖放、插件执行
>
> 若改动引入回归 bug，必须立即修复，不得累积。
>
> **强制阅读：[code-rules.md](./code-rules.md)** — 代码规则 & 反模式速查表。任何代码修改前必须先对照该文档逐项检查，忽略其规则的修改有极高概率引入回归 bug。
>
> **架构方向（最高优先级，2026-09-17 立项 / 2026-09-18 定案）**：渲染层**维持全局 WebView2**，**不脱离 WebView2 / Tauri**（原「原生搜索主层 + WebView 按需创建」方向已于 2026-09-18 废弃）。理由、实测基线（5 个 `msedgewebview2.exe` / 50.4 MB）、被否的 B/C 方案对比与六条 HWND 接缝记录见 **[architecture-rendering.md](./architecture-rendering.md)** —— 它**优先于** [agent-feature-backlog.md](./agent-feature-backlog.md) 的全部条目。**该决策不放松本文任何既有规则**（尤其 §11 规则 31 唤出顺序、规则 33 `pre-wrap` 幽灵空行、规则 38 环境变量合并语义）。

## 1. 架构概述

Lunac 是一个 **uTools 风格的桌面启动器 / 搜索工具**，由 Tauri 2.x 驱动。它从原有的 Claude Code CLI 聊天界面重构而来，现分为三个层次：

```
┌──────────────────────────────────────────────────────────────────┐
│                  Lunac 前端 (app/src/)                             │
│  main.ts → 毛玻璃搜索栏 + 插件系统 + 键盘导航                                  │
│  plugins/builtin/ → quick-launch / web-search /                  │
│                     settings / clipboard-history / tool-editor / ai-agent /│
│                     ocr / memo 等 8 个插件                           │
│  tools/*.json → Agent MCP Tools (sys_info 等)                     │
└────────────────────────────┬─────────────────────────────────────┘
                             │ Tauri IPC + WebView2
┌────────────────────────────▼─────────────────────────────────────┐
│                  Lunac 后端 (app/src-tauri/)                       │
│  main.rs         → 窗口管理 / 系统托盘 / 子进程生命周期                         │
│  hotkey.rs       → 原生 Win32 热键 (RegisterHotKey 优先 / LL 钩子兜底)     │
│  commands.rs     → 子进程启动 / IPC 命令 / Start Menu 扫描                │
│  proxy_server.rs → 内置协议代理 (Anthropic↔OpenAI, 已停用)                │
│  mcp_server.rs   → MCP stdio 服务器 (Agent Tools 执行引擎)              │
└────────────────────────────┬─────────────────────────────────────┘
                             │ 子进程 (spawn)
┌────────────────────────────▼─────────────────────────────────────┐
│               core-agent/ 自研 Agent 后端 (agent.exe)                │
│  agent.exe → lunac 自带，stream-json 模式，后台运行                        │
└──────────────────────────────────────────────────────────────────┘
```

## 2. 当前活跃功能

### 2.1 前端 — 搜索启动器

| 组件 | 文件 | 说明 |
|------|------|------|
| 主入口 | `app/src/main.ts` | 搜索栏 UI + 插件搜索 + 键盘事件 |
| 插件注册 | `app/src/plugins/registry.ts` | 关键词模糊匹配 + 评分排序 |
| 插件列表 | `app/src/plugins/builtin/index.ts` | 注册所有 8 个内置插件 |
| 快速启动 | `builtin/quick-launch.ts` | Start Menu 应用搜索与启动 |
| 网页搜索 | `builtin/web-search.ts` | 默认浏览器打开搜索页（Google / Bing / Baidu，见 §11 规则 6） |
| 剪贴板历史 | `builtin/clipboard-history.ts` | 剪贴板历史管理 — 自动保存复制内容 |
| 设置面板 | `builtin/settings.ts` | 快捷键绑定 / 模型配置 |
| 工具编辑器 | `builtin/tool-editor.ts` | MCP tool JSON 编辑管理 |
| AI 代理 | `builtin/ai-agent.ts` | Agent 对话 — 委托 `main.ts` 启动 agent.exe 子进程 + cli-output 事件渲染 |
| OCR 识别 | `builtin/ocr.ts` | 离线 OCR 图片文字识别 (PaddleOCR-json · PP-OCRv4 · 中/英/日/韩/俄) |
| 备忘录 | `builtin/memo.ts` | 本地自动保存备忘录 — 检索标识直达 / 图片粘贴（见 §11 规则 8） |
| 样式 | `app/src/styles.css` | 毛玻璃 Catppuccin 主题 |
| 国际化 | `app/src/i18n.ts` | 多语言翻译模块 — 跟随 Windows 系统语言 |

### 2.1.1 国际化 (i18n)

Lunac 自动检测 Windows 系统显示语言（`GetUserDefaultUILanguage`），启动时通过 Tauri IPC 获取 BCP-47 标签。所有 UI 字符串通过 `t(key, params?)` 函数动态翻译，支持 `{param}` 插值。

**支持语言**：zh-CN、zh-TW、ja、ko、en（默认回退）

**文件架构**：
| 层 | 文件 | 职责 |
|---|------|------|
| Rust 检测 | `commands.rs`:`get_system_language` | Win32 `GetUserDefaultUILanguage` → BCP-47 标签映射 |
| 翻译字典 | `app/src/i18n.ts` | 435 个顶层键（2026-09-18 实测），覆盖搜索/状态/聊天/插件/OCR/设置 |
| 语言函数 | `app/src/i18n.ts`:`t()` / `pluginName()` | 运行时翻译，支持参数插值 + en 回退 |

**翻译覆盖范围**：
- 搜索栏 placeholder / 无结果提示
- 状态栏（就绪/AI 模式/Agent 状态/插件运行/错误）
- AI 对话（输入框/流式生成/新对话/历史）
- OCR 插件（按钮/状态/结果/复制提示）
- 系统托盘 / 独立窗口标题
- 插件名称（8 个内置插件）
- 设置面板（语言选择器 / 保存提示）
- 剪贴板检测提示

**语言优先级**：用户手动选择（localStorage `lunac_lang`）→ Windows 系统语言 → en 默认

**热更新**：`setLanguage(tag)` 切换运行时语言 + 持久化到 localStorage，无需重启。

### 2.1.2 详细搜索大界面（2026-09-15，双击搜索栏进入）

简洁搜索旁边多一层**可选的**大界面：**在搜索栏上双击**即进入，Esc / 返回按钮退出。定位参照 Windows `Win+S`：一个大面板、结果**分组**、能搜文件与系统设置。**简洁搜索的一切行为不变** —— 进大界面只是叠一层视图，退出时把查询词带回搜索栏并重跑一次简洁搜索。

| 面 | 约定 |
|---|---|
| 进入 / 退出 | 双击 `#search-bar`（`pluginActive` / `detached` / 已在大界面时忽略，插件锁定态的双击仍走既有的「分离窗口」）；退出口 = 返回按钮 **或 Esc**（见下一行的「界面层」）。隐藏窗口后重新唤出会自动落回简洁搜索（`lunac-window-shown` 里 `exitDetailSilently()`） |
| **Esc 与「界面层」** | Rust 侧 `hotkey.rs` 有唯一一个**界面层枚举** `UI_MODE`（`main` / `plugin` / `detail`，由前端 `set_ui_mode` 同步）。Esc 的判据是**层**，不是内容：`RECORDING` → 取消录制；`UI_MODE != main` → **无条件 emit `lunac-esc-clear`**（层还没退完，交给前端）；`main` 且 query/chips 空 → 隐藏窗口；否则清内容。**禁止改回「query 空 + chips 空 + 非插件态 ⇒ 隐藏」** —— 详情态的查询词在自己的输入框里、简洁搜索栏本来就是空的，按内容判空会让一下 Esc 直接隐藏整个窗口（2026-09-15 修掉的 bug）。前端 `handleEscClear()` 的分支顺序：抽屉 → **详情大界面**（`exitDetail()`，把词带回搜索栏）→ 插件 → 泡泡 → 文本 |
| **界面层的同步纪律** | 层的真相源在 WebView（`pluginActive` / `detailOpen`），判据在 Rust 进程，**两边必须每次都对齐**。唯一出口是 `main.ts` 的 `syncUiMode()`（由那两个状态位推导，不写字面量），它挂在 **`applyWindowSize()` 末尾**（该函数本来就按「插件 / 详情 / 简洁」分支，是层的天然汇聚点，漏同步的转场会在下一帧自愈），并在**模块初始化**与**每次 `lunac-window-shown`** 各无条件重报一次。为什么这两处必要：WebView 一旦重载（DevTools / 前端刷新），WebView 侧状态归零而 Rust 侧留着旧值 ⇒ 表现为「**简洁界面里按 Esc 不隐藏窗口**，控制台一直打 `Esc(poll): ui_mode=2 (non-main)`」。**禁止给 `syncUiMode()` 加「值没变就不发」的去重缓存** —— 那要求所有发送点共用一份缓存，别处只要还留着裸 `invoke("set_ui_mode")` 就会脱节并反向压住一次必要的同步 |
| 窗口尺寸 | 固定档 **640 × zoom**（`DETAIL_HEIGHT`，与插件态 600/520/360 同一套「设计 px × zoom = DIPs」算法），且**不进高度滑动动画**（离散切换，滑动只属于搜索/结果态） |
| 分类 | 六个 Tab（全部 / 应用 / 文件 / 设置 / 系统动作 / 命令），**Tab 键循环切换**；切分类只筛已有结果、不重新查询。结果按 应用 → 文件 → 设置 → 系统动作 → 命令 → 网络搜索 分组显示（带组标题） |
| 文件类型 | 顶部一排类型胶囊（全部类型 / 文件夹 / 文档 / 图片 / 视频 / 音频 / 压缩包 / 程序 / 其他）。**这是唯一走后端重查的筛选**（`search_files(kind)`），值必须与 `file_indexer::Kind` 一一对应 |
| 数据源 | 应用 = `search_apps`；文件 = `search_files`（文件索引）；设置页 + 系统动作 = `system_catalog`（几十条，**前端本地匹配**）；命令 = `pluginRegistry.search`；网络搜索 = `web-search` 插件（回车兜底） |
| 键盘 | ↑↓ 选择（跨分组连续）、Enter 打开、Tab / Shift+Tab 切分类。Enter 落在空分类时 → 直接用输入内容走网页搜索 |
| 危险动作 | 关机 / 重启（目录里唯一 `danger: true` 的两条）**二次确认**：第一次 Enter 只把标题换成「再按一次 Enter 确认：…」并标红，3 秒内再按一次才执行 |
| 图标 | 应用 / 文件用系统图标（`get_app_icon`，异步回填 + token 作废旧回包）；插件用 `pluginIconSvg`；设置 / 动作条目用目录数据里的 emoji（列表项图标，非功能按钮 —— 见 [icon-style.md](./icon-style.md) §4） |

**文件索引**（`app/src-tauri/src/file_indexer.rs`）：与「应用列表」同一套纪律 —— 唯一存储是 `<exe 根>\temp\file-index-cache.json`，**搜索路径只读内存索引、永不扫盘**；扫盘只在后台线程（启动 600ms 后 / UI 上的「重建索引」按钮），写回原子（临时文件 + rename）。

| 项 | 值 / 理由 |
|---|---|
| 扫描范围 | 系统盘（`%SystemDrive%`）→ **只扫 `%USERPROFILE%`**；其它固定盘（`GetDriveTypeW == DRIVE_FIXED`，手写 FFI）→ 全盘扫。Windows / Program Files 里没有用户要找的文件，却是扫盘耗时的大头 |
| 存储形态 | **不存整条路径**：存「目录表下标 + 文件名 + 分类枚举 + 修改时间」。实测本机用户目录就有 19.6 万条，路径版缓存 20 万条 = 47.8MB，紧凑版 30 万条 = 30.8MB。全路径只在**返回命中的那几条**时拼回（`FileHit::path`） |
| 匹配 | 全部**大小写无关、零分配**（`eq_ci` / `starts_with_ci` / `find_ci`）：精确 1000 > 前缀 700 > 包含 500 > 多词全中 400，同分按修改时间倒序。**空查询 = 最近修改的文件**（Win+S 的空态语义） |
| 遍历顺序 | **广度优先**。截断是常态，BFS 保证浅层文件先入索引；深度优先会把预算全喂给第一个子目录，用户看到的是「随机缺文件」 |
| 上限 | 条数 `MAX_ENTRIES = 300_000`（到顶即停并标记 `truncated`，UI 如实显示「索引不完整」）、深度 12、跳过 `SKIP_DIRS`（Windows / AppData / node_modules / target / .git / Temp…） |
| 刷新节流 | 缓存 **24 小时**才算过期。实测扫一遍十几秒，绝不能每次启动都重扫（开机自启场景尤其不能）；想立刻纳新文件点「重建索引」 |
| 就绪前 | 索引没载入 / 正在扫 → `search_files` 返回空数组，界面显示「正在建立文件索引…」（每 2 秒轮询状态，就绪后自动补一次查询）。**绝不在搜索线程里同步扫盘** |

**系统设置与系统动作**（`app/src-tauri/src/system_catalog.rs`）：静态表 41 个常用 `ms-settings:` 页面 + 10 个系统动作，两条安全约束：

- **`open_setting` 只接受 `ms-settings:` 前缀**，`run_system_action` **只认白名单 id**（绝不接受前端传来的命令行字符串）—— 前端能传什么，后端就只认什么，不给「任意 URI / 任意程序执行」开口子。
- 条目名给 **zh / en 两套**（前端按系统语言取一套）。**不给五种语言**：Windows 设置页的名字是 OS 自己的资源，我们拿不到官方译名；搜索关键词里同时放中英与拼音首字母，所以任何语言下都搜得到。UI 外壳文案（Tab / 类型 / 提示）照旧走 i18n 五语言。

### 2.2 后端 — Rust 原生功能

| 功能 | 文件 | 实现方式 |
|------|------|---------|
| 全局热键 | `hotkey.rs` | **双模式（2026-09）**：优先 `RegisterHotKey`（内核级、无键盘钩子 → AV 误报最低，且不受前台程序权限影响，管理员/全屏游戏可用）；仅注册失败（Alt+Space 系统保留 / 组合被占用）才回退 `SetWindowsHookEx (WH_KEYBOARD_LL)`。默认热键 `Ctrl+Alt+Space` |
| 系统托盘 | `main.rs` | Tauri tray-icon，左键切换、右键菜单 (Show/Hide/Quit) |
| 关闭行为 | `main.rs` | `CloseRequested` 拦截 → hide 到托盘而非退出 |
| 窗口拖拽 | `main.ts` | JS `win.startDragging()` on search-bar mousedown |
| 幽灵点击 | `styles.css` | `#app { pointer-events: none }` + 子元素逐一手动 `pointer-events: auto` |
| AI 后台预加载 | `main.rs` + `commands.rs` | 启动时后台 `thread::spawn` 启动 proxy_server 内置代理，就绪后 `AI_READY` 置位 + emit `ai-ready`；退出时 `WindowEvent::Destroyed` 统一 kill |
| 子进程清理 | `main.rs` | `Destroyed` 事件 kill CLI + proxy 两个进程 + `taskkill` 清端口 5173 |
| OCR 引擎按需部署 | `paddle_ocr.rs` + `commands.rs` | PaddleOCR-json（`.7z` 约 88MB / 解压约 300MB）**不入库**；`ocr_engine_status` 查询、`ocr_engine_install` 后台下载 GitHub Release → `sevenz-rust` 解压到 staging → 校验 exe+config → 原子替换到 `<exe 根>\paddle-ocr`，进度经 `ocr-engine-progress`/`ready`/`error` 事件回传 |
| 文件索引（详细搜索） | `file_indexer.rs` | `temp\file-index-cache.json` 唯一真相；搜索只读内存索引、永不扫盘；后台扫盘（启动 600ms 后 / 手动重建）+ 原子写；见 §2.1.2 |
| 系统设置与动作目录 | `system_catalog.rs` | 静态表（41 个 `ms-settings:` 页 + 10 个动作）；`open_setting` 只认 `ms-settings:` 前缀、`run_system_action` 只认白名单 id；见 §2.1.2 |
| 被改动文件定位 | `commands.rs` + `main.ts` | `reveal_in_explorer(path)`（`#[tauri::command(async)]`）→ `explorer.exe` 的**单参数** `/select,<path>`；只收**绝对路径 + 存在性**；路径来源 = `tool_use` 入参的 `file_path`；见 §11 规则 32 |

### 2.3 热键 — 双后端（RegisterHotKey / LL 钩子）+ 三层兜底（2026-09 修订）

**后端选择**（`install_hook_thread`）：先试 `RegisterHotKey`，成功即**完全不装键盘钩子**；失败才装 LL 钩子。判定依据 `HOOK_MODE` 原子变量。

| 后端 | 触发条件 | 特点 |
|------|---------|------|
| **RegisterHotKey**（默认） | 组合可注册（如默认 `Ctrl+Alt+Space`） | 内核级注册、不发钩子不注入输入 → AV 误报最低；`WM_HOTKEY` 由系统投递给主窗口 → **不受前台程序权限/全屏独占影响**（管理员游戏可唤出） |
| **WH_KEYBOARD_LL 钩子**（兜底） | 注册失败（`Alt+Space` 被内核保留 / 组合被占用） | 兼容全局 `Alt+Space`；代价是 AV 会视作键盘记录器特征，故仅失败时启用 |

| 层级 | 覆盖场景 | 实现 |
|------|---------|------|
| **RegisterHotKey** | 其他应用为焦点（默认路径） | 内核级注册 → `WM_HOTKEY` → 窗口子类化 → 独立线程 toggle（**无 SendInput 注入**） |
| **LL 钩子** | 注册失败时的兜底 | 钩子回调纯原子检测 → `PostThreadMessageW` 异步转发 → 独立线程 toggle；`SendInput` dummy key 打断孤立 Alt 序列；250ms 防抖 |
| **窗口子类化** | 自身窗口为焦点 | `SetWindowSubclass` 拦截 `WM_SYSCOMMAND/SC_KEYMENU`（系统菜单必经消息），`lParam=0x20` 时 toggle，一律 `return 0` 抑制菜单 |
| **JS 兜底** | 漏网 keydown | document capture 层 `preventDefault()` + hide（仅当热键 = Alt+Space 时生效） |

**消息泵线程**恒常运行（承担 Esc 处理 `WM_APP_ESC` 与看门狗心跳）；无钩子模式下仅阻塞在 `GetMessageW`。**看门狗**健康检查仅在 `HOOK_MODE=true` 时进行，避免无钩子模式误判重装。改键（设置面板录制）后 `parse_and_set_hotkey` 重装后端 —— 因此任意组合都支持，且始终优先走无钩子路径。

### 2.4 窗口配置 — 实测驱动（Measure-driven）

```json
{
  "width": 800,  "height": 200,     // 初始尺寸；启动后 JS 实测内容重设
  "decorations": false,            // 无标题栏
  "transparent": true,             // WebView2 透明背景
  "alwaysOnTop": true,             // 始终置顶
  "resizable": true,               // 用户可拖拽缩放宽度；高度由 JS 内容驱动
  "minWidth": 480, "minHeight": 56,
  "center": true,
```

窗口高度由 JS `applyWindowSize()` 决定（2026-08-16 起改为**实测驱动**，取代 item-count 预测）：
- **搜索/结果/空态**：`measurePanelHeight()` 用 `getBoundingClientRect().bottom` 实测 搜索栏/结果区/状态栏 最底者——该 API 返回**含 CSS zoom 的视觉尺寸**（WebView2 下=逻辑像素 DIP），直接喂 `LogicalSize`，无需再乘 zoom。`#app` 为 `height:auto`（内容驱动），`#app.plugin-active`/`.detached`/`.detail-mode` 才 `height:100%`（固定高窗口，面板内部滚动）。
- **插件模式（固定高度 × zoom = DIPs）**：独立插件窗口（detached）600px、OCR detached 520px、内嵌插件视图 360px。
- **详细搜索大界面（2026-09-15）**：固定 **640px × zoom**（`DETAIL_HEIGHT`）。与插件模式同性质 = 离散跳变，**不进高度滑动动画**（`requestWindowHeight` / `animateWindowHeight` 里与 `pluginActive` 同条件排除）。
- **防抖与防循环**：`scheduleApplySize()` 60ms 防抖；`requestedHeight` + `currentWindowHeight` 双守卫跳过重复 setSize；onResized 检测宽度变化/外部改高后重置重断言；`ResizeObserver` 观察 `#app` 兜底内容变化。
- 宽度固定设计 800px（`WIN_WIDTH`），窗口宽度变化经 CSS `zoom`（clamp 0.6~2.5）等比缩放内容；`onResized` 载荷为 PhysicalSize，用启动快照 `BASE_DPR` 换算回逻辑宽度。

## 3. AI 对话系统 — Agent 单模式架构

Lunac AI 采用 **Agent 单模式** 设计（简单模式已于 2026-08-04 移除）：所有 AI 对话统一走后端子进程 `agent.exe`（自研 [core-agent](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs)，见 §3.5），具备完整工具链（工具调用、文件读写、Shell、权限审批）。

```
用户输入
  │
  └── Agent 模式 (agent.exe 直连)
       └─ spawn agent.exe + 直连供应商 Anthropic 兼容端点
       └─ 完整工具链 + 技能 + 提示词 + 权限审批 (control_request)
       └─ 可选工作区 (workspace) — 限定 Agent 文件操作目录
```

### 3.1 架构要点

| 维度 | Agent 模式 |
|------|:---:|
| **后端** | `agent.exe`（自研 core-agent）直连供应商（内置代理已停用） |
| **API 调用** | N 次（工具循环） |
| **Token 消耗** | ~4000+/轮 |
| **权限** | `can_use_tool` 审批走前端卡片（批量整合 + 对话暂停） |
| **工作区** | `set_workspace` 设置 cwd + `--add-dir` 作用域 |

### 3.2 模式说明

- **对话模式**：应用始终运行 Agent 进程（`ai_mode` 恒为 "agent"；`set_ai_mode` 拒绝其他值）。唯一的对话级开关是**思考开关**（开 / 关）：
  - **思考开关**：`on` / `off` 两档，UI 在输入栏 ⋯ 菜单里（`#chat-mode-seg`），localStorage `lunac-chat-mode` 持久化，切换即重启 agent。端点为什么只配两档见 §3.5「思考开关跨模型自适应」。旧的 simple / agent 模式切换（`#chat-mode-btn` / `buildSimpleChatHint`）已废弃。
- **权限审批**：agent 发 `can_use_tool` control_request → 前端卡片（`showPermissionCard` + 内置安全前缀 / 白名单 / 危险命令黑名单）→ 回包前 agent 阻塞等待（P2 已接通，超时 300s 按拒绝）
- **工作区**：`AppState.workspace` 决定 agent.exe 的 cwd 与 `--add-dir`；为空时用**默认工作目录** `<exe 根>\temp\transStorage`（2026-09-17 起，见 §11 规则 34）—— 整个系统仍可访问，敏感操作走 ask 弹卡

### 3.3 技术实现 (`agent.exe` 直连)

- **直连**：`agent.exe` 直接连供应商原生 Anthropic 兼容端点（`AI_AGENT_URL` 或 `{base}/anthropic`），完整支持 tool_use
- **启动**：`start_cli` / `ensure_agent_running` → `ai_credentials()` + `configure_agent_env()` 注入 `LUNAC_AGENT_BASE_URL` / `LUNAC_AGENT_TOKEN` / `LUNAC_AGENT_MODEL` → spawn `agent.exe`（内置代理 `proxy_server.rs` 已停用 — 其 Anthropic→OpenAI 翻译会**丢弃 tools 数组**，导致模型无法输出 tool_use、退化为文本式 XML 工具调用）
- **通信**：`send_message` stdin 写入，stdout stream-json SSE 流式读取（`cli-output` 事件）
- **停止**：`stop_cli` → kill agent.exe
- **工作区**：`set_workspace(path)` canonicalize 校验目录后存入 `AppState.workspace`；`start_cli_process` 以其为 cwd + `--add-dir`（为空时用 `default_work_dir()` = `<exe 根>\temp\transStorage`，**启动时自动创建**，见 §11 规则 34 —— 不再回退用户主目录；整个系统仍可访问，敏感操作走 ask 审批弹卡）；前端 localStorage `lunac-agent-workspace` 持久化，启动时恢复

### 3.4 前端接入

| 功能 | 文件 | 说明 |
|------|------|------|
| Agent 对话 | `main.ts` → `startAgentChat` → `start_cli` → `send_message` | cli-output 事件流式渲染 |
| 权限审批 | `main.ts` `showPermissionCard` | 多请求合并为单批处理卡片，pending 期间状态栏显示"对话已暂停"；白名单/安全前缀自动放行，危险命令只给手动确认（P2 已接通） |
| 工具卡片 | `main.ts` `agentNewBlock("tool")` / `agentToolArgsDelta` / `agentToolResult` | `content_block_start(tool_use)` + `input_json_delta` 流式展开参数，`tool_result` 内联成功/失败（P1 起真正生效） |
| 工作区设置 | `main.ts` AI 对话输入栏 `#chat-workspace-btn`（唯一入口） | 选择目录/重置 → `invoke("set_workspace")` + 重启 CLI；默认=`<exe 根>\temp\transStorage`（整个系统可访问，见 §11 规则 34） |
| 思考开关 | `main.ts` `#chat-mode-seg`（输入栏 ⋯ 菜单内）+ `set_thinking_mode` | `on` / `off` 两档 → `LUNAC_THINKING` → agent 侧 `Thinking`（见 §3.5「思考开关跨模型自适应」）；切换重启 agent |
| Token 仪表盘 | `main.ts` `addUsageToTotals` / `updateTokenDashboard` / `appendUsageLog` | 计费口径：Hit=缓存读取，Miss=普通输入+缓存写入，Total=四类 token 之和。**数值 = 本地用量日志的「今日累计」**（每次提问落一行 JSONL，见 §3.5「用量与对账」），可与供应商平台按天对账 |

### 3.5 自研 agent 核心 `core-agent/`（2026-09，已接线）

后端二进制 `agent.exe` 由本仓库自研（[core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs)），遵守下列 stream-json 契约。src-tauri 的 `start_cli` / `ensure_agent_running` / `start_agent_http` 统一经 `commands.rs` 的 `core_dir()` 定位二进制，按优先级：

0. **dev 布局**（exe 位于 `…\src-tauri\target\{debug,release}\`）：`<repo>\core-agent\target\{release,debug}\agent.exe` —— **必须排最前**（2026-09-17 修，见 §11 规则 35）。理由：`target\{debug,release}\agent.exe` 是 Tauri 构建时按 `bundle.resources` 平铺的**快照**，只在构建 lunac 时刷新，会遮蔽 core-agent 的新构建。
1. `<exe_dir>\resources\agent.exe`（Tauri 打包资源，`bundle.resources` 用 map 形式平铺）
2. `<exe_dir>\agent.exe`（便携版 / NSIS 安装根，与 lunac.exe 同级）
3. dev：`<repo>\core-agent\target\{release,debug}\agent.exe`（cargo 产物）
4. 兜底 `<repo>\core`（历史目录）

**命名纪律**：自研侧不得再出现 `ANTHROPIC_*` / `CLAUDE_CODE_*` 环境变量；仅保留协议必需的 `anthropic-version` 请求头与供应商侧的 `/anthropic` 路由（外部协议名，改了就不通）。IPC 名 `start_cli` / `stop_cli` / `cli-output` / `cli-status` / `cli_bridge` **保持历史命名**（前端与本文档的既有契约，与二进制文件名无关）。

**契约（前端既有约定，不得改动）**

| 面 | 内容 |
|---|---|
| env | `LUNAC_AGENT_BASE_URL`（已是完整端点，请求拼 `/v1/messages`）、`LUNAC_AGENT_TOKEN`（**必须走 `authorization: Bearer`**；用 `x-api-key` 会被兼容端点判 401）、`LUNAC_AGENT_MODEL`；另有 `LUNAC_THINKING`（思考开关：`off` = 关，其余 = 开；见 §3.5「思考开关跨模型自适应」）、`LUNAC_MAX_CONTEXT_TOKENS`（上下文预算，默认 128000、低于 8000 的取值视为无效）、`LUNAC_SUMMARY_COMPACT`（摘要式压缩开关：`0`/`false`/`off`/`no` = 关，其余含未设置 = **开**；见 §11 规则 39）、`LUNAC_SKILLS_DIR`、`LUNAC_WORKSPACE_LOCKED`、`LUNAC_SEARCH_PROVIDER` + `LUNAC_SEARCH_KEY`（WebSearch 主源的服务商与密钥，服务商可选 bocha / tavily / exa / firecrawl；缺任一项则只用无 key 的 Bing / 百度兜底源）、`LUNAC_LOG_DIR`（宿主注入的日志目录 = `<exe 根>\temp\logs`）、`LUNAC_LOG`（`off` = 关闭落盘日志）、`LUNAC_LOG_LEVEL`（`error\|warn\|info\|debug`，默认 `info`；见 §11 规则 20） |
| 启动参数 | `--add-dir <dir>`（可重复，工作区外追加可访问目录）/ `--permission-mode plan`（只读）/ `--dangerously-skip-permissions`（忽略工作区锁）/ `--permission-prompt-tool stdio`（写类工具先审批）/ `--disallowedTools <name…>`（这些工具不进请求体）/ `--mcp-server stdio:<exe 路径>`（拉起该 exe 的 MCP server 并接入其工具，P3）；其余（`--print` / `--verbose` / `--input-format stream-json` / `--include-partial-messages` …）一律接受并忽略 |
| stdin | 每行一条 JSON：`{"type":"user","session_id":"","message":{"role":"user","content":[{"type":"text","text":"…"}]},"parent_tool_use_id":null}`；`{"type":"control_response","response":{"subtype":"success","request_id":"…","response":{"behavior":"allow"\|"deny",…}}}` 为审批回包（P2，由 stdin 线程按 request_id 直接投递给等待中的工具调用）；`{"type":"set_history","messages":[{"role":"user"\|"assistant","content":"纯文本"}]}` 为**会话历史整体替换**（2026-09-17，回退 / 恢复历史时回灌上文的唯一通道，见 §11 规则 30）。`set_history` **不触发模型调用**（不是提问），只替换 agent 内的 `history` 并回一个 `system/history_set`（含 `messages` 条数）|
| stdout | 每行一条 JSON：`system/init`（含 `tools` 名单）→ `system/context_compacted`（`elided` / `dropped` 计数，压缩发生时补发）→ `system/api_retry`（`attempt` / `max_retries` / `error_status` / `delay_ms`，瞬时失败退避重试时补发，前端解析分支早已就绪）→ `stream_event`（`content_block_start` / `content_block_delta`(`text_delta`\|`thinking_delta`\|`input_json_delta`) / `content_block_stop` / `message_stop`）→ `assistant`（整包，含 `tool_use`，仅无增量时前端兜底）→ `control_request`（`can_use_tool`，写类工具执行前）→ `user`（整包，含 `tool_result`）→ `result`（`subtype` / `is_error` / `usage`，用量为整轮累计） |

**P0 已完成**：多轮上下文（进程内 history）、SSE 增量打字、用量上报（input/output/cache_read/cache_creation）、错误回传（失败轮按 `history.truncate(base)` 整体回滚，不污染后续对话）、stdin 读取线程与查询线程经 mpsc 解耦（为 P2 的 `control_response` 预留通路）。

**P1 已完成（2026-09，内置工具循环）**：十一件工具实现在 [core-agent/src/tools.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/tools.rs)，主循环按「请求 → 流式收块 → 有 `tool_use` 就执行并以 `tool_result` 回灌 → 再请求」往返，直到模型不再调工具（上限 `MAX_TOOL_ROUNDS=16`，到顶后再给一次「只用文本收口」的机会）。

| 工具 | 入参 | 行为 |
|---|---|---|
| `Read` | `file_path` / `offset` / `limit` | 带 1-based 行号输出；单次 ≤2000 行、文件 ≤2MB |
| `Write` | `file_path` / `content` | 自动建父目录，整文件覆盖 |
| `Edit` | `file_path` / `old_string` / `new_string` / `replace_all` | 精确串替换；找不到、或多处匹配且未开 `replace_all` 时按错误返回 |
| `Bash` | `command` / `timeout` | `cmd /C` 执行（带 `CREATE_NO_WINDOW`，GUI 宿主下不闪黑框）；默认 120s、上限 600s，超时 kill；stdout+stderr 合并回传 |
| `PowerShell` | `command` / `timeout` | `-NoProfile -NonInteractive -Command` 执行（无用户 profile、不卡交互式输入），超时与结果口径同 `Bash`（共用 `run_shell`）；命令前置 `$OutputEncoding` / `[Console]::OutputEncoding` 双 UTF-8 兜底 —— PS 5.1 重定向到管道时按控制台 ANSI 码页（中文 Windows = GBK）输出，不切 UTF-8 会把中文变成替换字符 |
| `Glob` | `pattern` / `path` | `**` 递归、`*` 不跨目录；≤200 条 |
| `Grep` | `pattern` / `path` / `glob` / `ignore_case` | Rust 正则逐行匹配，输出 `路径:行号:内容`；跳过 `.git`/`node_modules`/`target` 等重目录与二进制文件；≤200 条 |
| `WebSearch` | `query` / `count`（1–10，默认 5） | 联网检索。**主源 = 可配置的搜索 API**（`LUNAC_SEARCH_PROVIDER` 选 bocha / tavily / exa / firecrawl，配 `LUNAC_SEARCH_KEY`），**兜底 = Bing RSS → Bing HTML → 百度 HTML 抓取**（无 key；两次抓取间强制 ≥1.1s，202/429 视为限流）。主源失败/0 结果/未配齐服务商与 key 时自动回落，回落结果尾部附 `[fallback] <原因>`；三级兜底全失败则整条报错（原因写进 `is_error=true` 的 `tool_result`，模型可自行改方案）。返回 `Query / Source / 编号列表（标题 + URL + 摘要）`；主源用 UA `Lunac/<版本>`，抓取类兜底源用浏览器 UA（Bing / 百度对非浏览器 UA 只给降级空壳）。为什么不用 DuckDuckGo：本机实测 `html.duckduckgo.com` 与 lite 版均 15s 超时（国内不可达），「不配 key 也能搜」会变成空话；为什么不是 Bing Search API：微软已于 2025-08-11 退役全部 Bing Search API（老 key 410 Gone、不再接受新注册），官方替代品是绑定 Azure 的 AI 平台产品而非 SERP API |
| `WebFetch` | `url` / `prompt`（提示性） | 抓取 URL 并把 HTML 转成纯文本（去 script/style/注释、块级标签当换行、剥标签、解高频实体，无 DOM 依赖）；`reqwest` 60s 超时、≤10 次重定向、≤10MB 响应、UA 标识为 `Lunac/<版本>`；返回 `URL / Status / 正文`。**不做二次模型摘要**（正文直接回给主模型，省一次往返、不绑死供应商小模型，故 `prompt` 只作提示）；**不做域名预检**（旧 CLI 依赖 `api.anthropic.com/api/web/domain_info`，我们没有该服务，安全性交审批与前端白名单）。`Read` 到的文件内容可拼进 URL，故它会把数据发往外部 |
| `AskUserQuestion` | `questions`（1–4 题，每题 2–4 选项）/ `answers`（**由前端填**） | 结构化提问：选项给用户点选。**工具自身只做格式化** —— 答案由前端经审批卡的 `updatedInput` 回传（`{...input, answers}`），工具把它排成 `User has answered your questions: "题" = "答"`。收不到 `answers` 就**报错而非编答案**（模型会改用文本提问）—— 见下「结构化提问」一节 |
| `TodoWrite` | `todos`（数组，项含 `content` / `status`(`pending`\|`in_progress`\|`completed`) / `activeForm`，三项必填） | 待办清单：模型**每次都发完整清单**（整体替换语义）。工具**不维护状态、不落盘、不碰本机** —— 清单的唯一真相是模型最近一条 `tool_use` 入参，进程重启 / 多会话并行都不会串味；`todo_write()` 只回一段确认 + 清单快照（状态回显成规范名，防止模型用别的词造成漂移）。**免审批**（`needs_approval` 不含它）。前端拿流式入参画面板（见下「待办面板」） |

**工具权限策略**

| 条件 | 效果 |
|---|---|
| `--permission-mode plan`（前端「安全」档） | 只读：`Write`/`Edit`/`Bash`/`PowerShell` 一律以 `is_error=true` 拒绝（不弹审批）；`Read`/`Glob`/`Grep`/`WebSearch`/`WebFetch` 可用，`AskUserQuestion` 也可用，`TodoWrite` 照常可用（它只改前端面板） |
| `--permission-prompt-tool stdio` 且非 plan 档 | `Write`/`Edit`/`Bash`/`PowerShell`/`WebSearch`/`WebFetch`/`AskUserQuestion` 执行前先发 `can_use_tool` 请前端审批；前端自行判定「内置安全前缀 / 白名单自动放行」还是「弹卡片」（危险命令永远只给手动确认）。没有这个开关就不问，避免对着无人应答的通道干等。`TodoWrite` **永远不在此列**（不碰本机，问了纯属打扰） |
| `WebSearch` / `WebFetch` / `AskUserQuestion` 在 plan（只读）档 | **照常审批**（`tools::gated_in_read_only`）—— 只读档对写类工具的「不必问」豁免不适用于它们：写类工具在只读档会被直接拒绝（问了白问），而这三件在只读档是放行的。`WebSearch` 会把查询词发往外部搜索源；`WebFetch` 能把 `Read` 到的文件内容拼进 URL 带出本机；`AskUserQuestion` 的答案只能从卡片上取（不问就拿不到答案） |
| `LUNAC_WORKSPACE_LOCKED=1`（配置了工作区时 src-tauri 注入） | 文件类工具路径先做词法规范化（消 `..`），越出工作区（cwd / `--add-dir`）即拒绝 —— 含 `Read` 的越界读取。**审批通过也不放行**（这是硬边界） |
| MCP 工具（P3，见下） | 非 plan 档下**一律先发 `can_use_tool`**（handler 能跑 shell / 发 HTTP，且定义来自用户 JSON，agent 侧无权替用户判断）；plan 档压根不接入，模型看不到这些工具 |
| `--dangerously-skip-permissions`（前端「完全」档） | 忽略工作区锁 |

**P2 审批实现要点**：一批工具**先全部发请求、再逐个等回包**（前端才能把连续 Bash 合并成一行一次性决定，见 `findLastBashGroup`）；`updatedInput` 为非空对象时覆盖原参数，空对象表示按原参数执行；`behavior=deny` 转成 `is_error=true` 的 `tool_result` 交回模型（模型可改方案），回包带 `interrupt=true` 则本轮就此结束；等待上限 300s，超时按拒绝处理并回一条 `control_cancel_request` 让前端撤掉卡片。回包由 stdin 线程按 `request_id` 直接投递给等待中的调用，不进主消息队列。

**命令静态安全分析（Bash / PowerShell，2026-09）**：`can_use_tool` 请求体里多了一个**可选字段** `analysis`（实现见 [core-agent/src/bash_safety.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/bash_safety.rs)）：

```json
{"type":"control_request","request_id":"req_…","request":{
  "subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tu_1",
  "input":{"command":"del /f/s/q %TMP%"},
  "analysis":{"dangerous":["强制删除"],"opaque":["变量展开"]}}}
```

| 项 | 约定 |
|---|---|
| 为什么在 agent 侧 | 前端只拿到命令字符串，正则挡不住 ① 引号拼接（`r""m -rf /`）② 包装器（`cmd /c "del /f/s/q …"`、`powershell -Command "…"`、`bash -c '…'`）③ 变量（`%TMP%\x.bat`、`$env:TEMP\x`）④ 串联/管道的**后半段**（`echo hi & shutdown /r`）。agent 能拿到原命令并按子命令结构化拆分 |
| 判定不代替决策 | agent **只上报判定**，不拒绝执行 —— 「自动放行 / 弹审批」仍是前端的唯一决策点（规则 21）。这样「用户明确点允许」依然能放行 |
| `dangerous` | 命中的危险规则标签（中文）。**非空 ⇒ 任何运行方式档位都必须人工确认，且不提供「始终允许」**（与 `CMD_BLACKLIST` 同等强度，但不可被引号/变量/管道绕过） |
| `opaque` | 含**无法静态判定**的成分。**判据 2026-09 收窄** —— 只拦「不知道要跑**哪个程序**」：① **命令词本身**是变量（`$cmd …` / `!CMD! …` / `%CMD% …`）；② 命令替换 `$(…)`（内层命令会被执行，而我们没分析它）；③ cmd 的 `%VAR%`（cmd 在解析阶段展开，值里的 `&` `|` 会变成新命令）；④ 编码执行（`-EncodedCommand`、`FromBase64String`）；⑤ 间接执行器（`certutil`/`wscript`/`cscript`/`mshta`/`rundll32`/`regsvr32`）；⑥ 动态启动进程（`Start-Process`）；⑦ 包装器嵌套过深、控制字符。**参数里的变量不再算**（`Get-ChildItem $HOME`、`$f = "…"; Get-Item $f` 是普通命令，`$HOME`/`$env:TEMP` 的展开结果不会被重新解析成**命令**）—— 旧口径把参数变量一并拦下，导致「自动」档位下每条带 `$` 的命令都弹卡。**非空 ⇒ 不得自动放行**（fail-closed：判不出来就当「要人看」） |
| 规则口径 | 覆盖 Windows 两类 shell：递归/强制删除、格式化与分区、覆写物理磁盘、注册表、引导配置、系统还原点/备份、关机重启、强制结束进程、权限修改、账户/服务/计划任务、动态执行，以及 git 强制推送 / `reset --hard` / `clean -f` / `branch -D`。**单引号是字面量、双引号会展开**（三种 shell 一致）：`echo 'rm -rf /'` 不误报，`bash -c 'rm -rf /'` 仍会被递归解析出内层 |
| 前端兼容 | **缺字段时回落到前端自己的正则**（`CMD_BLACKLIST`），旧 agent 照常工作；字段存在时**以 agent 判定为准**，正则降为二道网。`tool_result` 文本不变 |
| 白名单化限制 | 解释器/启动器前缀（`cmd` / `powershell` / `bash` / `python` / `node` / `npx` / `iex` / `env` / `schtasks`…）**永不进白名单** —— 白名单是前缀匹配，放进去等于把「以后任何 `powershell …`」全自动放行；`opaque` 命令同样不给「始终允许」 |
| 落盘 | 命中时写一条 `warn`（只记标签，不记命令原文，避免把命令里的凭据抄进日志；仍过 `mask_secrets`） |

✅ **档位已可切换（2026-09）**：两个入口共用 `set_security_profile`（`restart=true` 才重启 agent）——① 设置 · AI 面板的**安全档位**下拉（只读 / 项目 / 完全，边界 = 允不允许）；② AI 输入栏的**运行方式**胶囊（手动 / 白名单 / 自动，频率 = 问不问，「自动」档映射 `full`）。两者关系、自动档二次确认与常驻警示、越界卡片的三个动作见 [agent-ui-spec.md](file:///d:/cc/claude-code-cli-master/docs/agent-ui-spec.md) §4，规则见 §11 规则 21。

**结构化提问（AskUserQuestion，2026-09）**：模型发 `AskUserQuestion{questions:[…]}` → agent 发 `can_use_tool` → 前端把选项渲染成按钮 → 用户点选后**经 `updatedInput` 回答案**，agent 用它覆盖原参数并执行工具（答案不带 `interrupt`，一轮照常继续）。

| 项 | 约定 |
|---|---|
| 答案通道 | 复用 `can_use_tool` 的 `updatedInput`（`{...input, answers}`）——**不新增 stdout 消息类型、不新增 control 子协议**。所以 `answers` 必须挂在非空对象里（agent 只认「非空 object 才覆盖」，空 `{}` = 用原参数） |
| 题面 | `question`（完整题）+ `header`（≤12 字符标签）+ `options`（2–4 项 `{label, description}`）+ `multiSelect`；答案的**键是 `question` 原文** |
| 前端 UI | `app/src/main.ts` `renderAskQuestions()` + `styles.css` `.approval-ask-*`；「允许」按钮在该行改写为「提交」，且**不给「始终允许」**（白名单化 = 以后自动回空 `updatedInput` = 模型永远拿不到答案） |
| 不可自动放行 | `classifyRequest()` 对 `AskUserQuestion` 恒定返回 `auto:false`，即使工具名已进白名单 |
| 缺答案 | 工具收到不到 `answers`（未传 `--permission-prompt-tool stdio`，或用户没选就点了全部允许）**必须报错**、不得编造答案；模型会改用文本提问 |
| plan 档 | 可用，且**照常审批**（`tools::gated_in_read_only`）—— 提问本来就不改本机，而答案只能从卡片取 |

**待办面板（TodoWrite，2026-09）**：模型发 `TodoWrite{todos:[…]}` → agent 直接执行（**不发审批**）→ 前端把 `tool_use` 入参画成一块面板。全程**不新增协议**。

| 项 | 约定 |
|---|---|
| 数据方向 | **单向**：后端只回确认文本，面板完全由前端渲染 `tool_use` 入参 —— 工具不持有状态，不存在「前后端两份清单对不上」 |
| 整体替换 | 模型每次必须发**完整**清单；前端在同一轮次里**就地重绘**同一块 `.todo-panel`，所以多次 `TodoWrite` 只留最后一份状态，不会堆成一摞 |
| 流式解析 | 入参是 `input_json_delta` 的合法前缀，分片残缺时 `JSON.parse` 必失败 → 跳过；末片必定完整，故最终状态一定画得出来（`parseTodoArgs` / `todosFromInput`） |
| 不做审批 | `needs_approval("TodoWrite") == false`；它不碰本机任何东西，问了纯属打扰 |
| 结果不重复渲染 | 成功的 `tool_result`（"Todos have been modified successfully…" 会把整份清单原样重复）**不贴**「✓ 完成」行；失败照常显示 |
| 前端 UI | `app/src/main.ts` `renderTodoPanel()` + `styles.css` `.todo-panel` / `.todo-item`；工具黑名单候选名单同步补 `TodoWrite` |

**工具错误不中断整轮**：工具返回 Err 时转成 `is_error=true` 的 `tool_result` 交回模型自行纠正；只有 HTTP / 流错误才终止本轮并回滚 history。写回上下文的 assistant 消息会**剔除 thinking 块**（端点要求 thinking 带 `signature`，回灌会 400），发给前端的整包仍保留 thinking。

**联网检索（WebSearch，2026-09）**：主源是**可配置的搜索 API**（设置 · AI · 搜索服务商 + 搜索 API 密钥），兜底是**免 key 的结果页抓取**（Bing RSS → Bing HTML → 百度 HTML）。

| 项 | 约定 |
|---|---|
| 选型原因 | 兜底源必须**免 key 且国内可达**：原选的 DuckDuckGo 实测在本机（国内）15s 超时（`html.duckduckgo.com` 与 lite 版都连不上），等于「不配 key 也能搜」是假的；Bing（`www` / `cn` 均 200，<600ms）与百度（200，1.5s）均通。抓取之所以是「结果页」而不是 API：**Bing Search API 已于 2025-08-11 全部退役**（老 key 返 410 Gone、不再接受新注册），官方替代「Grounding with Bing Search」是绑定 Azure 项目的 AI 平台产品、不是 SERP API；DuckDuckGo / 百度同样没有公开免费的 SERP API |
| 主源服务商 | `bocha`（博查，`api.bochaai.com/v1/web-search`，国内直连、中文结果最好，注册只需微信扫码）/ `tavily`（`api.tavily.com/search`，`Authorization: Bearer`）/ `exa`（`api.exa.ai/search`，`x-api-key`）/ `firecrawl`（`api.firecrawl.dev/v2/search`）。四家只差 endpoint / 鉴权头 / 响应字段名，共用一个 `post_json()` + `json_hits()`。**不做「猜服务商」**：服务商没选或名字不认识就退回兜底源并说明原因 —— 猜错等于把密钥发给无关的第三方服务器 |
| key 存放 | **唯一真相源 = `<exe 根>\config\ai.json`**（`storage::load_ai_config` / `save_ai_config`，`set_ai_config` 写入并注入 env、`get_ai_config` 回读），启动时 `commands::apply_saved_ai_config()` 把它注入 `AI_SEARCH_PROVIDER` / `AI_SEARCH_KEY`，再经 `configure_agent_env` → agent.exe 的 `LUNAC_SEARCH_PROVIDER` / `LUNAC_SEARCH_KEY`（`configure_agent_env` + `start_cli_process` 两条路径都要给）；**空串 = 删除**，前端每次都回传输入框当前值。**不再用 localStorage**（它会覆盖 `.env`，见 §11 规则 2） |
| 回落语义 | 未选服务商 / 未配 key / 服务商名未知 / 主源报错 / 主源 0 结果 → 走抓取兜底，结果尾部附 `[fallback] <原因>`；兜底三家按 **Bing RSS → Bing HTML → 百度** 顺序试，全失败则整条 `Err`（进 `is_error=true` 的 `tool_result`，原因含三家各自的报错），**不编造结果** |
| 抓取限流 | 进程内 `OnceLock<Mutex<Instant>>` 强制两次抓取间隔 ≥1.1s；HTTP 202 / 429 视为限流并显式报错（区分「被限流」与「没结果」）；响应里连结果容器（`<item>` / `b_algo` / `result c-container`）都没有时**报错**而不是返回空列表 —— 不把「改版/被反爬」说成「没搜到」 |
| 解析要点 | Bing RSS 是首选（干净 XML、`<link>` 就是真实 URL、无跳转壳），先切 `<item>` 块再在块内取字段（通道级同名标签会串）；Bing HTML 取 `<h2><a href>` + 就近 2KB 内的 `b_caption` 摘要；百度取 `class="result c-container"` 容器的 `mu="真实URL"`（**不必跟 `baidu.com/link?url=` 的 302**）+ 块内 `<h3>` 标题，摘要字段不稳定故留空 |
| 抓取 UA | 主源一律用 `Lunac/<版本>`；**抓取类兜底源用浏览器 UA + `Accept-Language: zh-CN`** —— Bing / 百度对非浏览器 UA 只返回降级空壳（实测数据即用浏览器 UA 取得），这是抓取结果页的必要条件，不代表身份伪装 |
| 审批 | 在 `needs_approval` 与 `gated_in_read_only` 里（查询词是外部出口），**plan 档同样弹审批**；前端工具黑名单候选名单同步补 `WebSearch` |
| 不新增依赖 | 解析全部用既有 `regex` + `serde_json`（`head_chars()` 按 UTF-8 边界截断，避免中文页面切片 panic） |
| 验证口径（2026-09-17 用户定） | **四家付费主源的「成功」路径不作为验收项** —— 预算原因拿不到可用 key，只保「请求形状 + 错误透传 + 回落」正确（已验）。**真正要守的是兜底链**（它才是「未配 key 也能搜」这句话的支撑）：`cd core-agent && cargo test fallback_scrapers -- --ignored --nocapture` —— 全仓**唯一联网**用例，故意标 `#[ignore]`（依赖外网与对方页面结构，进常规 `cargo test` 会让离线/CI 随机挂），但必须保持可一键重跑。它断言两件事：① 三级至少一级可用（等价于 `scraped_search()` 能出结果）；② **Bing RSS 必须单独活着** —— 它是链的首选，不单独钉的话「它挂了但百度还在」会被 ① 掩盖成静默降级。**2026-09-17 实测：三级全部 OK 各 5 条**（2.84s，含两次 1.1s 节流）。**对方改版后必须重跑这一条** |

**思考开关跨模型自适应**（2026-09；2026-09-15 由「fast/think/deep 三档」收敛为**开 / 关**）：开关由 src-tauri 的 `LUNAC_THINKING` 在 spawn 时传入（`off` = 关，其余含未设置 = 开）。

**为什么不做「思考力度 / 深度」档位**（实测，2026-09-15）：本端点没有这个旋钮 ——

| 探测 | 结果 | 含义 |
|---|---|---|
| `budget_tokens` = 1 / 1024 / 32768 | 思考量**完全一样** | 预算**不被 enforce** |
| `output_config.effort` / `reasoning_effort` | 200 但行为无变化 | 被当**未知字段静默忽略** |
| 顶层未知字段 `foo=bar` | 200 | 端点不校验未知字段 |
| `thinking.type = nonsense` / `budget_tokens = "abc"` | **400** | `thinking` 对象本身会校验 |
| 不发 `thinking` 字段 | 仍在思考（`[thinking, text]`） | 默认即开，只有显式 `disabled` 才真关 |

即三档在端点上本就退化成两态 ⇒ UI 只做两档，**不得再对外声称有思考深度档位**（那是端点做不到的承诺）。

各供应商的 Anthropic 兼容端点对 `thinking` 字段接受度不同（DeepSeek 只认 `enabled`/`disabled`、原生 Messages 端点的新模型要 `adaptive`、Kimi 等兼容层可能完全不支持），故**不硬编码模型名单**，而是：

| 输入 | 首选形态 | `max_tokens` |
|---|---|---|
| `LUNAC_THINKING=off` | `{"type":"disabled"}` | 8192 基线 |
| 其余（`on` / 未设置 / 值不认识） | `{"type":"enabled","budget_tokens":8192}` | `max(8192+4096, 8192)` = 12288 —— 端点要求 `budget_tokens < max_tokens` |

预算值来自**单一常量** `THINKING_BUDGET`（[core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs)）：端点不 enforce 时它无意义，真 enforce 的端点只要一个合法值。**不得**再把它做成用户可见的档位旋钮。

**400 降级链**（仅当错误正文含 `thinking`/`adaptive`/`budget_tokens` 才触发，避免把「模型名不存在」这类无关 400 也白重试）：开档为 `enabled+budget → adaptive → 不带字段`；关档为 `disabled → 不带字段`（**不退到 adaptive**，否则等于反过来把思考打开）。降级结果缓存在进程内，后续轮次不再试错，并往 stderr 打一行说明。

**单条工具输出预算**（2026-09-15）：单条工具结果超过 **12000 字符**时，把**全文**落到 `temp\tool-outputs\{毫秒}-{工具名}.txt`，上下文里只内联「头 8000 + 尾 2000 + 行数 + 路径」，并告诉模型可用 `Read`（带 `offset`/`limit`）或 `Grep` 取回。

| 面 | 约定 |
|---|---|
| 唯一出口 | `tools::apply_budget(name, body)`，只在 [core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs) 的 `run_tool` 调用 —— 只有那一层同时拿到**工具名**与**未经裁剪的完整输出**。各工具内部一律不再自行截断（Skill / MCP 的结果也走同一出口） |
| 为什么不做硬截断 | 旧实现超 30000 字符直接丢，模型侧**永久看不到**（只有 `LUNAC_LOG_LEVEL=debug` 能在日志里翻到），长构建日志/整页抓取经常表现成「像是什么都没输出」 |
| 尾部必留 | 报错结论通常压在输出末尾，只留头部等于把最值钱的部分丢掉 |
| 落盘体积 | 正文按 **1.5MB 字节**封顶（切在字符边界）—— 必须低于 Read / Grep 的 `MAX_TEXT_BYTES`(2MB) 文件门槛，否则模型读不回自己落盘的文件；被砍时预览里如实写「only the first N chars」 |
| 可读性 | 落盘目录由 `tools::prepare_output_dir()` 在启动时建好，并**并入 `Ctx.add_dirs`** —— 默认 project 档的工作区锁会拦工作区外路径，不并入等于模型读不到自己的输出 |
| 保留 | 启动时清理 7 天前的 `*.txt`（与落盘日志同口径）。**不新增环境变量**：目录取 `log::log_dir()` 的父目录（宿主已注入 `LUNAC_LOG_DIR`，agent 独立运行也有回退） |
| 落盘失败 | 磁盘满 / 无权限 → 仍按内联预算收口，末尾说明改成「全文落盘失败，超出部分已丢弃」，**不**把十几万字符塞回上下文 |

**真机烟测（2026-09-15，deepseek-flash，工作区锁开启）**：让 agent 跑出 4000 行 / **170906 字符**的 PowerShell 输出 → 落盘 `1789477750484-PowerShell.txt`（170906 字节，与全文一致）、内联 **10229 字符**、日志记一行「输出超预算（170906 字符 / 4001 行，全文已落盘）」。**落盘目录刻意放在工作区之外**，模型仍成功 `Read(file_path=…, offset=3900, limit=1)` 取回该行（`add_dirs` 生效），全程 **0 次 `Access denied`**。

**只读工具并行**（2026-09-15）：模型一轮里可以同时发多条工具调用（真机实测 DeepSeek 一次发 4 条）。**连续的只读调用**合成一批并行执行，其余各自串行。

| 面 | 约定 |
|---|---|
| 白名单 | `tools::parallel_safe(name)`：`Read` / `Glob` / `Grep` / `WebSearch` / `WebFetch` / `Skill` / `TodoWrite`。**写类（`Write`/`Edit`）与 `Bash`/`PowerShell` 一律不并行** —— 它们有副作用，且顺序本身就是语义 |
| **批必须连续** | 只读段**绝不允许跨越写类调用**：否则「写 A → 读 A」会被重排成「读 A（旧内容）→ 写 A」，错得无声无息。切分见 `plan_tool_batches`，单元素的只读段**不标并行**（省一次线程 spawn，行为与串行完全一致） |
| 并发上限 | `TOOL_PARALLELISM = 4`，靠「分块 + 块内 join」实现（不引信号量），见 [core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs) |
| 回灌顺序 | **恒等于 `tool_use` 的原顺序** —— 结果按下标回填 `slots`，再统一用 `tool_result_block()` 组装。并行只改变执行时机，不改变任何可观测顺序 |
| 审批仍在并行之前 | 先把整批 `pendings` 按原顺序逐个 `await_approval`，再执行。审批顺序乱了会打乱前端按「未应答行」合并同一批命令的结果 |
| MCP 不并行 | 只读白名单里只有内置工具 ⇒ 并行批不需要 `mcp_bridge`，顺带绕开了 `&mut Bridge` 无法跨线程共享的问题。MCP 调用永远走串行路径 |
| 为什么只在只读上做 | dev 实测本地读取是毫秒级（`Read 3ms` / `Glob 0ms`），并行对它们收益≈0；**真正有收益的是 `WebFetch`（网络）**，而长命令（构建/测试）带副作用与全局限流，不能并行 |
| 可观测 | 每个并行批往落盘日志记一行「只读工具并行批 N 条（并发上限 4）: Read, Read, Glob」，否则线上无法判断并行到底有没有生效 |

**真机烟测（2026-09-15，deepseek-flash）**：一次提问要求「同一条回复里发起 4 个工具调用」→ 模型发出 `Read ×3 + Glob ×1`，日志记「只读工具并行批 4 条（并发上限 4）: Read, Read, Read, Glob」，四条 `tool_result` 回灌顺序与 `tool_use` 完全一致。

**上下文预算与压缩**（2026-09）：端点的上下文窗口是硬限制，超了就是 400，而失败轮会整体回滚 history —— 不管理体积的话对话会「越用越死」。预算取 `LUNAC_MAX_CONTEXT_TOKENS`（默认 128000，低于 8000 视为无效），**水位以端点实测值判断**（`message_start` 的 `input_tokens + cache_read + cache_creation`，跨轮保留在 `Cfg.last_input`），比按字符估算准。**前两级不额外调用模型**，第三级（摘要）才调，且只挂在丢弃档上：

| 档 | 触发 | 动作 |
|---|---|---|
| 瘦身 | 实测 > 预算 × 0.85 **且**距上次压缩已再长 ≥ 预算 × 0.15（滞回） | 把较旧轮次里超 2000 字符的 `tool_result.content` 就地换成 `[elided: N chars dropped to save context]`（文件内容/Grep 结果是体积大头，价值递减），尾部 8 条不动。**只瘦身、永不丢整条消息** —— 在这个水位上丢消息等于白废一次缓存 |
| 丢弃 | 实测 > 预算 × 0.95 | 从最老处整条丢弃、只留尾部 8 条（**仅当没有可瘦身的大块**时；体积在对话本身才丢）；**始终保留开头那条用户提问**（任务目标），且丢弃后首条不得是 `tool_result`（必须紧跟对应 `tool_use`），否则补一条 `TRIMMED_MARKER` 文本消息 |
| 400 兜底 | 端点回报的 400 正文含 `context`/`too long`/`input length` | 强制压缩一次后重试（每轮至多一次），兜住估算误差 |
| **摘要（2026-09-18，§8.2）** | **只挂在上面「丢弃」与「400 兜底」两档**（`dropped > 0` 且送进模型的原文 ≥ 4000 字） | 把被丢的那段调一次模型压成摘要、钉回历史第 1 条之后。**0.85 的瘦身档绝不触发**；三道成本闸 + `LUNAC_SUMMARY_COMPACT` 开关 + 失败一律降级，见 §11 规则 39 |

**水位为什么定得高 + 为什么要滞回（2026-09）**：每次压缩都会改写请求前缀，端点侧 KV 缓存随之整段作废。原先 0.70 水位 + 无滞回，会让**每轮都有一两条旧消息跨过保留尾部被瘦身** → 前缀每轮都变、缓存每轮归零，实测是缓存命中率偏低的最大来源。现在：水位抬到 0.85/0.95、瘦身档加滞回（一次压缩后要再长 15% 预算才允许动第二次），并把「只瘦身档也能直接丢消息」这条去掉。丢弃档不受滞回约束 —— 到了 0.95 不压就可能 400，安全优先。见 §11 规则 23。**摘要压缩也遵守同一条纪律**：它只挂在本来就必然要 drain 的那两档上，**不额外制造压缩时机**。

压缩发生时往 stderr 与 stdout 各报一次（stdout 为 `system/context_compacted`，含 `elided` / `dropped` 计数）。**压缩会左右移动 `history`，调用方的失败回滚锚点 `base` 必须同步修正为 `base - dropped + pinned`**（`pinned` = 压缩插回的合成消息条数，即 `TRIMMED_MARKER` + 任务快照；摘要钉回时再 `+1`），否则回滚（`history.truncate(base)`）会误删保留段或真实历史。单条用户输入超 100000 字符先截断（防一次粘贴顶爆窗口）。

**瞬时失败重试（2026-09）**：这是**请求级**重试，不是「重新生成回答」—— 只在**还没读到响应体之前**退避重试，所以**永远不会产生重复内容**。它补的是「网络一抖就整轮失败」这个可用性缺口。

| 项 | 约定 |
|---|---|
| 覆盖范围 | ① 网络层：连接失败 / 连接重置 / 超时（`reqwest` 的 `is_builder()` 错误除外 —— URL 非法、TLS 配置错重试也不会成功）；② **429 限流**与 **5xx**（含部分兼容端点表示「过载」的 **529**） |
| 不在此列 | 400 + thinking 相关 → 走既有「思考降级链」换形态再试；400 + 上下文超限 → 强制压缩后再试；其余 4xx（鉴权 / 参数错）→ 重试也不会变，直接报错 |
| 次数与退避 | `MAX_API_RETRIES = 3`；首次 1s，之后 2s / 4s 指数增长，**加 0..250ms 抖动**（取系统时间亚秒位，不引 `rand`，见规则 20），`RETRY_MAX_MS = 30s` 封顶 |
| `Retry-After` | 端点给出**秒数**时优先采用（再被 30s 封顶），HTTP-date 形态忽略（不引日期解析） |
| 上报 | 每次退避发一条 `system/api_retry`（`attempt` / `max_retries` / `error_status` / `delay_ms`），前端据此显示重试状态；同时落盘一条 `warn`（过 `mask_secrets`） |
| SSE 流中途断开 | **不重试**（部分内容已经流给前端了，重来会重复）—— 只记一条 `warn`，本轮回复可能不完整 |
| 与回滚的关系 | 重试发生在 `r` 成功之前，因此不触及 `history`；只有最终仍失败才走 `finish_error` → `history.truncate(base)` |

**用量与对账（2026-09）**：`result.usage` 的四个字段是**本次提问的绝对值**（agent.exe 每次提问重置计数，提问内的工具往返在本轮内累加），**不是会话累计** —— 这是旧 cli.exe 的行为，前端一度按「累计值做差」处理，会把「前缀没变」的那几轮命中缓存算成 0（两轮 `cache_read` 相同 → 差值 0），本地命中率系统性低于供应商平台。现在前端只做**直接累加**。

| 项 | 约定 |
|---|---|
| 口径映射 | 平台「输入（命中缓存）」= `cache_read_input_tokens`；「输入（未命中缓存）」= `input_tokens`（DeepSeek 走自动缓存，实测 `cache_creation_input_tokens` 恒为 0，Anthropic 原生端点才有值）；「输出」= `output_tokens`；平台的「合计」= 三者之和 |
| 粒度差 | 平台**按每次 API 请求**记一行，本地**按每次提问**记一行 —— 一次带工具的提问在平台上就是多行（system prompt + tools 前缀每次重发）。**2026-09 起这个差已被抹平**：`result.usage.requests[]` 把每次请求的明细带上，本地日志里也逐条落盘，可直接与平台逐行对账 |
| 每次请求明细 | `result.usage.requests` = `[{in, read, create, out}]`（顺序 = 请求顺序；`in` = 该次未命中输入、`read` = 该次命中、`create` = 缓存写入、`out` = 该次输出）。agent 在每条 `message_stop` 推一条（`message_delta.output_tokens` 是**该条消息的累计值**，故用赋值而非累加）。**旧 agent 不报该字段 → 前端写空数组**；`UsageRecord.requests` 为空时**不写该键**（旧记录读时按空表） |
| 落盘 | 每次提问追加一行到 `<exe 根>\ModuleData\usage\usage-YYYY-MM-DD.jsonl`（只追加不重写、按天分片），字段 `{ts, model, input, output, cacheRead, cacheCreate, elided, dropped, requests?}`；`ts` 为本地时钟 epoch 毫秒，`model` 取自 `system/init` |
| 读写命令 | [storage.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/storage.rs) `append_usage_log(date, record)` / `read_usage_log(date)`；`date` 只接受严格 `YYYY-MM-DD`（文件名来自前端，必须挡路径拼串）；读取时单行损坏只跳过该行 |
| 表盘数值 | token 仪表盘 = **当前这次对话**（新建会话 / 切到别的会话即归零），提问结束实时累加；**界面不再读日志**（`read_usage_log` 命令保留，供外部对账脚本或将来做「按天用量」面板）。发生过压缩时，命中率 tooltip 会追加「本对话压缩 N 次瘦身 / M 条丢弃」—— **这是解释命中率的归因口径**：压缩是「断裂型」失效，其余偏低才是「自然未命中」 |
| 计算口径 | Hit = `cacheRead`；Miss = `input + cacheCreate`（Anthropic 的 `input_tokens` **不含**缓存两项，故不能拿它减 `cache_read`）；Total = Miss + Hit + `output` |

**P3 已完成（2026-09，MCP 工具桥）**：实现在 [core-agent/src/mcp.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/mcp.rs)。src-tauri 在 spawn 时把 lunac.exe 自己的路径交过来（`--mcp-server stdio:<路径>`），agent **作 client** 把该 exe 以 `--mcp-server` 拉起 —— 那个进程会拦截该参数、进 stdio MCP server 模式（实现在 [mcp_server.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/mcp_server.rs)），读 `<exe 根>\tools\*.json` 的用户自定义工具（handler 有 `shell` / `http` / `builtin` 三种）。

握手：`initialize` → `notifications/initialized` → `tools/list`（超时 15s）→ 模型调用时 `tools/call`（超时 180s）。要点：

| 项 | 约定 |
|---|---|
| 命名 | 一律 `mcp__<原名>`（前端审批卡的「始终允许」按完整名字记入 localStorage 白名单，**前缀必须稳定**）；原名含非字母数字/`-`/`_` 或超 64 字符时替换/截断为 `_`，重名追加 `_2` |
| 接入范围 | 非 plan 档启动时接一次；`--disallowedTools` 里出现原名或带前缀名即不接入；plan（只读）档**不连接**（接进来只会每次被拒，还让工具清单随档位漂移） |
| 审批 | MCP 工具一律先发 `can_use_tool`（见上表）；deny → `is_error=true` 的 `tool_result` |
| 顺序 | `tools/list` 的返回顺序不保证稳定，接入时**按工具名排序后再入请求体** —— tools 数组属于请求前缀，顺序一变端点侧的前缀缓存整段失效 |
| 错误 | 上游 `isError=true`、进程退出、超时都转成 `is_error=true` 的 `tool_result`，不中断整轮；结果与内置工具走**同一预算出口**（超 12000 字符落盘、只内联头尾，见 §3.5「单条工具输出预算」） |
| 容错 | spawn/握手失败只往 stderr 记一行并继续 —— 十一件内置工具必须照常可用；MCP server 的 stderr 直接并入 agent stderr（上游会转发到前端/终端），stdout 独占给 JSON-RPC |
| 生命周期 | agent.exe 退出时 kill 子进程（`Drop for Bridge`）；新增/改动 `tools\*.json` 后需重启 agent.exe 才生效（与工具黑名单同一套重启流程） |

**P4 已完成（2026-09，技能 SKILL.md）**：实现在 [core-agent/src/skills.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/skills.rs)。agent.exe 启动时扫 `LUNAC_SKILLS_DIR`（= `<exe 根>\skills`，见 §11 规则 3）下的 `<key>/SKILL.md`，采用**渐进披露**：系统提示词里只列 `key: 描述`（描述 ≤250 字符、清单总预算 8000 字符），模型需要时调内置 `Skill` 工具取回正文（`$ARGUMENTS` 已按调用参数替换）。要点：

| 项 | 约定 |
|---|---|
| 清单顺序 | 技能按 `key` 排序后再拼进系统提示词 —— 与 MCP 工具同理，顺序抖动等于废掉整段前缀缓存 |
| 解析 | frontmatter（`name` / `description`）与正文分离；读不出的 SKILL.md 静默跳过，不影响其余技能 |
| 匹配 | 调用参数先匹配 `key`，再退到 frontmatter.name，均大小写不敏感 |
| 生效 | 面板安装 / 保存 / 删除后调用 `__lunac_reload_agent` 重启 agent.exe（技能目录在启动时扫描一次） |
| 未做 | fork / remote 两种技能模式；MCP 的 `ListMcpResourcesTool` / `ReadMcpResourceTool`（server 侧只有 `resources/list`，没有 `resources/read`） |

**与旧 cli.exe 的完整差距清单、价值评级与实施顺序见 [agent-feature-backlog.md](file:///d:/cc/claude-code-cli-master/docs/agent-feature-backlog.md)。**

**构建**：`powershell -ExecutionPolicy Bypass -File scripts\build-core.ps1`（等价 `cd core-agent; cargo build --release`）→ `core-agent\target\release\agent.exe`，约 2.5MB（P1 引入 glob/regex 后从 1.5MB 增长）。打包链路（**实际生效的那条**）：`build-release.ps1` **[6/9]** 步把 `lunac.exe` + `agent.exe` + `WebView2Loader.dll` 拷进暂存目录 `release\Lunac\`，再由 `release\lunac-installer.nsi` 的 `File` 指令打进安装包。注意两点：①脚本走的是 `cargo build --release` + 手写 NSI，**不跑 `tauri build`**，所以 `tauri.conf.json` 的 `bundle.resources` 在本流程里并不生效（它只在 Tauri 自带打包器下起作用，别把它当打包依据）；②**[4/9]** 步必须在 Rust 构建之前跑，因为同一步的产物 `agent.exe` 是 **[6/9]** 步要拷的文件。

### 3.6 数学公式渲染

Agent 回复支持 KaTeX 实时渲染 LaTeX 数学公式：

| 组件 | 说明 |
|------|------|
| **引擎** | KaTeX 0.16.11 (CDN)，手动 `katex.render()` 而非 auto-render，避免 `renderMathInElement` 对动态 DOM 的兼容问题 |
| **定界符** | `$$...$$` 块级公式，`$...$` 行内公式，正则逐一匹配 → `katex.render()` 逐个渲染 |
| **渲染时机** | 流式响应结束后 `setTimeout(renderLatex, 10)` 等待 DOM 稳定后手动 TreeWalker 扫描文本节点 |
| **样式** | 使用 KaTeX 自带 CSS（CDN），还原了之前的暗色主题覆盖，避免与内置样式冲突 |
| **System Prompt** | 指示 AI 所有数学/信号处理公式使用 LaTeX 格式输出 |
| **错误处理** | `throwOnError: false` — 语法错误时保留原始文本 |

支持的公式类型：
- 基础数学：$E=mc^2$、$\sqrt{x^2+y^2}$、$\int_0^\infty e^{-t}dt$
- 信号处理：$F(\omega)=\int_{-\infty}^{\infty}f(t)e^{-j\omega t}dt$、$y[n]=x[n]*h[n]$、$X(z)=\sum x[n]z^{-n}$
- 矩阵与向量、希腊字母、上下标、分数、求和/积分运算符

### 3.7 Trae 参考规范（必须参考，2026-08-04 新增）

> 用户明确要求：**AI 对话窗口功能全面参考 Trae 的 AI 对话实现，但前端风格保留 Lunac 自身（毛玻璃 + 圆角 + 主题色）**。以下为必须遵守的实现规范，任何 AI 对话 / 审批交互改动都需对照执行。

**A. 权限审批（can_use_tool）**
- 连续多个简单命令（同一工具名 Bash/PowerShell、单行、不含管道/重定向/后台符/分号分隔等 shell 分隔符、≤200 字符）**自动合并为一个允许窗口**，显示"已合并 N 条命令"，一次允许/拒绝批量响应全部命令（Trae 风格）。
- 合并仅发生在审批展示层 —— CLI 仍逐条独立执行，绝不改变命令语义（`&&` 短路、每条命令独立输出）。
- 不可合并的命令保持独立行：含 shell 分隔符/管道/重定向/后台符/换行、danger 黑名单命令、非命令工具（文件读写等）。
- 批量按钮（全部允许/全部拒绝）必须对**合并组去重**（一个组多个 requestId 指向同一行）。
- 命令行渲染：等宽字体、`word-break: break-all`、`title` 悬停显示完整命令、单行不换行（多命令组内可换行显示）。
- 每行按钮紧凑（`padding: 4px 0`），danger 命令不提供"始终允许"。

**B. 工具调用过程展示（对话流内）**
> 2026-09 升级：工具行升级为**命令卡片**（状态 / 退出码 / 耗时 / 折叠输出 / 复制）、思考块规范化（省略 + 惰性渲染）、回合自动折叠，详见 [agent-ui-spec.md](file:///d:/cc/claude-code-cli-master/docs/agent-ui-spec.md) §3。以下各条仍是底线要求。
- 工具参数实时显示**可读摘要**而非原始 JSON：Bash/PowerShell 显示 `command` 文本；其他工具显示 `key=value` 摘要（≤200 字符截断）。
- 工具结果成功时折叠为一行 `✓ 完成: <120字符摘要>`，点击 `<details>` 展开完整输出（≤600 字符）；失败显示 `✗` + 错误原因；⚠ 安全告警保留黄色高亮。
- 思考过程默认折叠为 `▸ 思考中…`，展开查看完整内容。
- 连续 3 次工具失败显示黄色警告条。

**C. 消息流交互细节**
- 流式文本带右侧光标闪烁指示；发送后输入框立即清空。
- 状态栏展示：就绪/运行中/AI·模式/热键提示/token 仪表盘（恒为真实计费口径：Hit=缓存读、Miss=输入+缓存写、Total=四类之和；数值为**今日累计**，取自本地用量日志）。
- 代码块、公式（KaTeX）保留渲染；错误消息统一前缀 `⚠`。

**D. VSCode 插件 = Trae 右侧 AI 窗口形态**
- 活动栏 Lunac 图标 → 侧边栏 AI 聊天面板（`lunac.chatView`），样式参考 Trae 右侧窗口但保留自身前端风格。
- **直接驱动 agent.exe**（stream-json 协议），不依赖 Lunac 桌面应用 / HTTP bridge → **Lunac 无需打包安装包**（便携目录 release/Lunac 即可）。
- agent.exe 发现顺序：配置 `lunac.cliPath` → `release/Lunac/agent.exe` → `%USERPROFILE%/.lunac/agent.exe` → `LUNAC_AGENT_PATH`。
- provider 通过 `lunac.provider/apiKey/apiUrl/model` 配置映射为 LUNAC_AGENT_BASE_URL / LUNAC_AGENT_TOKEN / LUNAC_AGENT_MODEL 环境变量；ripgrep 随附目录加入 PATH。
- webview 内实现：流式渲染（text_delta）、思考折叠、工具卡片、审批卡片（允许/拒绝）、状态指示；进程生命周期随 webview 打开/关闭。

## 4. 插件系统 + Agent Tools 体系

### 4.1 插件独立状态架构 (前端插件)

前端插件由 `main.ts` 直接调用 `plugin.execute()`，渲染在搜索窗口内。

### 4.2 Agent Tools 体系 (MCP Bridge)

硬件 AI Tools 不走前端插件体系，而是集成到 Agent 模式：
- **定义位置**：`app/src-tauri/tools/*.json` (MCP tool JSON 定义)
- **运行时路径**：`%LOCALAPPDATA%\Lunac\tools\` (用户可自定义增删)
- **调用链路**：用户输入 → agent.exe (Agent) → MCP Bridge → `lunac.exe --mcp-server` → 执行 shell/http handler
- **管理方式**：tool-editor 前端插件提供 UI 管理

```
用户输入 (Agent 模式)
  │
  └─ agent.exe 解析意图
       └─ 匹配 MCP Tool
            └─ MCP Bridge (lunac.exe --mcp-server)
                 └─ 执行 Shell / HTTP
                      └─ 返回结果 → agent.exe → 前端渲染
```

**内置 Agent Tools：**

| Tool ID | 类型 | 说明 |
|---------|------|------|
| `system_info` | shell | 系统硬件信息：CPU/RAM/GPU/Disk 查询 |
| `get_weather` | shell | wttr.in 天气查询 |

### 4.3 插件状态保存与恢复 (前端)

每个插件维护独立的状态，搜索界面作为总入口：

```
searchInput (总端口)
  │
  ├── Enter → executePlugin("memo")
  │            └── closePluginView: 保存 memo 状态 → pluginStates.set("memo", ...)
  │            └── re-enter: 从 pluginStates 恢复 HTML + 输入内容
  │
  ├── Enter → executePlugin("settings")
  │            └── 同上：独立保存/恢复
  │
  └── No match → startAIChat → activePluginId = "ai-agent"
                 └── streaming 完成 → 自动保存到 pluginStates
```

| 概念 | 实现 |
|------|------|
| **状态存储** | `Map<string, PluginState>` — key=pluginId, value={html, searchQuery, pendingInput} |
| **保存时机** | `closePluginView()` 关闭时保存；AI streaming `doneCallback` 完成时自动保存；新对话开始前自动 `saveCurrentSession()` |
| **恢复时机** | `executePlugin()` 检测到已保存状态 → 恢复 HTML + 重新绑定监听器 |
| **streamId 去重** | `startAIChat` 分配递增 `streamId`，回调中 `myStreamId !== streamId` 时忽略，解决中途关闭后旧 streaming 回调污染新对话的问题 |
| **历史持久化** | **文件式**（不是 `localStorage`）：`<exe根>\ModuleData\history\chat-history.json`，由 Rust 侧 `load_chat_sessions` / `save_chat_sessions` 读写，最多 50 条。`ChatSession = { id, title, messages[], createdAt, usage?, steps? }` —— `usage` 是**表盘口径**的 token 用量快照（hit / miss / total / elided / dropped），`steps` 是**按回合分组的过程快照**（thinking / tool / text，超长截断，见 `recordTurnSteps`）。**回顾历史时把两者读回来**：表盘数值还原到 token 仪表盘，过程渲染成可折叠的「过程 · N 步」块（`renderHistoryProcess`）。ai-agent 仅输入关键词时展示历史列表；新对话自动保存上一会话 |

### 4.4 搜索匹配机制 (`registry.ts`)

```
用户输入 → pluginRegistry.search(query)
  ├─ Name 匹配 (exact=100 → prefix=80 → contains=60 → fuzzy×2)
  ├─ Keyword 匹配 (同上)
  ├─ Pinyin 匹配 (全拼 + 首字母，同权重)  ← 新增
  │    例: "js" → 匹配 "计算" (jisuan contains "js" + 首字母精确)
  │    例: "s"  → 匹配 "算" (suan), "数学" (shuxue), "设置" (shezhi)
  ├─ Description 匹配 (contains=30 → fuzzy)
  └─ 过滤 ai-agent → 排序 → 取前 8 个
```

### 4.5 插件清单（前端）

| ID | 图标 | 触发关键词 |
|----|------|-----------|
| quick-launch | 🚀 | open, launch, run, app, start, 打开, 启动, 运行 |
| settings | ⚙️ | settings, shortcut, hotkey, config, 设置, 快捷键, 配置 |
| clipboard-history | 📋 | clipboard, history, paste, 剪切板, 历史, 剪贴板, 粘贴 |
| web-search | 🌐 | search, google, baidu, bing, web, 搜索, 网页 |
| tool-editor | 🔧 | tool, tools, 工具, mcp, agent, skill, 插件, 扩展 |
| ai-agent | 🤖 | (被搜索过滤排除，仅作为无匹配时的回退显示) |
| ocr | 🔍 | ocr, 识别, 文字识别, 图像识别, 图片转文字, 截图识别, 图识字, 文字提取 |
| memo | 📝 | 备忘录, memo, 便签, 笔记, 记事本 |

## 5. 关键设计决策

| 决策 | 原因 |
|------|------|
| **热键用原生 Win32 API** | `tauri-plugin-global-shortcut` 在 Windows 上不可靠；`RegisterHotKey` 是内核级 API |
| **前端不参与热键** | `hotkey.rs` 直接调用 `win.show()/hide()`，零前端依赖，避免 JS 线程延迟 |
| **`pointer-events: none` 策略** | 毛玻璃透明窗口在无结果时只占搜索栏高度，其余区域穿透点击 |
| **窗口宽度固定设计值 800，高度实测驱动** | 高度改由 JS 实测内容决定（搜索档实测 `#app` 底部，插件/详情档离散固定高 600/520/360/640），取代旧的固定 800×600 —— 后者在长结果与插件态下要么裁内容、要么留大片空白，见 §2.4 |
| **搜索内容不随失焦清空** | `hideWindow()` 纯隐藏不重置，Esc 才清空 |
| **拖拽用 `startDragging()` API** | `data-tauri-drag-region` 在子元素(input/button)上不触发 |
| **只读工具并行，写类串行** | 本地读取是毫秒级、真正省时间的是网络与命令；但写类/命令的顺序本身就是语义（见 §11 规则 28） |
| **详细搜索单独一层视图，而不是第二个窗口** | 复用主窗口切大界面（`#app.detail-mode` + 640 固定档）能沿用全部窗口/热键/隐藏逻辑，避免新窗口带来的一整套焦点与单实例问题（见 §11 规则 29） |

## 6. 文件结构

```
app/
├── src/
│   ├── main.ts                      # 主入口 — 搜索栏 UI + 插件调度
│   ├── index.html                   # 单页面布局
│   ├── styles.css                   # 毛玻璃主题
│   ├── shortcut.ts                  # [闲置] 旧 JS 热键 API
│   └── plugins/
│       ├── registry.ts              # 插件注册表 + 模糊搜索
│       └── builtin/
│           ├── index.ts             # 注册所有插件
│           ├── quick-launch.ts
│           ├── web-search.ts
│           ├── clipboard-history.ts
│           ├── settings.ts
│           ├── tool-editor.ts
│           ├── ai-agent.ts
│           ├── ocr.ts            # OCR 文字识别 (PaddleOCR-json)
│           └── memo.ts           # 备忘录（自动保存 / 检索标识 / 图片）
├── src-tauri/
│   ├── src/
│   │   ├── main.rs                  # Tauri 入口 — 窗口/托盘/子进程生命周期
│   │   ├── hotkey.rs                # 原生 Win32 热键（RegisterHotKey 优先 / LL 钩子兜底，见 §2.3）
│   │   ├── single_instance.rs       # 单实例保护（命名互斥体 + 唤出事件，见 §11 规则 1）
│   │   ├── auto_start.rs            # 开机自启（计划任务优先 / HKCU Run 兜底，见 §11 规则 1）
│   │   ├── commands.rs              # IPC 命令
│   │   ├── storage.rs               # 数据根定位 lunac_root_dir() + 配置/业务数据读写（见 §11 规则 7）
│   │   ├── log.rs                   # 落盘日志（宿主侧：启动/退出、agent stderr、前端 JS 错误，见 §11 规则 20）
│   │   ├── app_indexer.rs           # 应用列表扫描（唯一存储 temp\app-index-cache.json，见 §11 规则 4）
│   │   ├── file_indexer.rs          # 文件索引（详细搜索用；见 §2.1.2 / §11 规则 29）
│   │   ├── system_catalog.rs        # Windows 设置页 + 系统动作白名单（见 §2.1.2）
│   │   ├── chat_db.rs               # 会话历史 SQLite（chat.db + FTS5，见 §11 规则 30）
│   │   ├── icon_extractor.rs        # 系统图标提取（SHGetFileInfoW → base64 PNG，应用/文件列表用）
│   │   ├── cli_bridge.rs            # agent 子进程全局状态（Tauri 命令与 HTTP bridge 共用）
│   │   ├── agent_server.rs          # HTTP bridge（127.0.0.1:8789，供 VSCode 扩展驱动 agent.exe）
│   │   ├── paddle_ocr.rs            # PaddleOCR-json 子进程 OCR（按需下载，见 §11 规则 13）
│   │   ├── windows_ocr.rs           # Windows.Media.Ocr 内置 OCR（兜底引擎）
│   │   ├── proxy_server.rs          # 内置协议代理 (Anthropic↔OpenAI, 已停用)
│   │   └── mcp_server.rs            # MCP stdio 服务器 (Agent Tools 执行引擎)
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/default.json    # Tauri 2 权限声明
│   └── tools/                       # Agent MCP Tools (内置)
│       ├── system_info.json         # 系统硬件信息 tool
│       └── weather.json             # 天气查询 tool
├── vite.config.ts
├── tsconfig.json
└── package.json

core-agent/
├── src/main.rs                      # 自研 agent 核心 —— stream-json 契约、工具循环、审批，见 §3.5
├── src/tools.rs                     # P1 内置工具：Read / Write / Edit / Bash / PowerShell / Glob / Grep / WebSearch / WebFetch / AskUserQuestion / TodoWrite
├── src/mcp.rs                       # P3 MCP 工具桥（stdio client，连 lunac.exe --mcp-server）
├── src/skills.rs                    # P4 技能（LUNAC_SKILLS_DIR 的 <key>/SKILL.md + Skill 工具）
├── src/log.rs                       # 落盘日志（agent 侧：工具调用与错误、stderr、panic，见 §11 规则 20）
├── Cargo.toml
└── target/release/agent.exe         # 编译产物（cargo build --release，约 2.5MB，不入库）

scripts/
├── _env.ps1                         # 公共环境准备（把 cargo / mingw64\bin 追加进 PATH，供其它脚本 dot-source）
├── verify-git.ps1                   # 新克隆自检（npm run verify），退出码 0/1
├── commit.ps1                       # 一键提交（npm run commit）：暂存 → 敏感/超大文件检查 → 提交；-Push 才推送
├── build-core.ps1                   # 编译自研 agent 后端 core-agent → agent.exe（cargo build --release）
├── dev.ps1 / tauri-dev.ps1          # 开发启动（缺 agent.exe 时先自动构建）
├── build.ps1 / tauri-build.ps1      # 打包封装
├── lunac-installer.nsi              # NSIS 安装脚本（入库，见 §8.2）
├── download-paddle-ocr.ps1          # 预置离线 OCR 引擎
├── make-icon.ts                     # 应用图标生成
└── _extract_colors.ps1              # 主题取色辅助脚本

agent-templates/                     # 发布包预置的用户扩展模板（README + *.example，见 §11 规则 24）
├── skills/
└── tools/

build-release.ps1                    # 一键打包（仓库根，见 §8.2）
```

> 所有 ps1 脚本必须用 `$PSScriptRoot` / `Split-Path -Parent $PSScriptRoot` 推导仓库根，**禁止硬编码本机绝对路径**；统一包管理器为 `npm`。
> **含中文的 `.ps1` 与 `.nsi` 必须以 UTF-8 with BOM 保存** —— Windows PowerShell 5.1 与 makensis 对无 BOM 文件按 ANSI(GBK) 解码，中文字符会把紧随其后的引号/换行吞进双字节：ps1 报「字符串缺少终止符」，NSI 报 `Bad text encoding: <file>:<line>`（行号指向**首个非 ASCII 行**，不是真正出问题的那一行，极易误判）。已知触发源：`download-paddle-ocr.ps1` / `build-release.ps1`（PS 侧），以及**用会丢 BOM 的编辑器/批量替换工具改 `scripts\lunac-installer.nsi`**（实测：一次文本替换就把 BOM 抹掉，makensis 立刻在第 14 行中文注释处报 `Bad text encoding`，整个打包链路直接断掉）。`build-release.ps1` 第 ⑨ 步每次都会用 `UTF8Encoding($true)` 重写 NSI，所以**从仓库新鲜克隆的 NSI 有没有 BOM 取决于最后一次提交** —— 提交前请确认首三字节是 `EF BB BF`。

## 7. 开发命令

```powershell
npm run verify               # 新克隆自检（仓库根，缺什么一次列清）
npm run commit               # 一键提交当前全部变更（自动生成提交信息）
npm run build                # 前端编译验证
# 提交脚本带参数时必须直接调用 .ps1 —— npm run commit -- -xxx 在本机 PowerShell 下不会转发参数：
powershell -ExecutionPolicy Bypass -File scripts\commit.ps1 -DryRun                       # 只看不提交
powershell -ExecutionPolicy Bypass -File scripts\commit.ps1 -Message "fix: …" -Push        # 指定信息并推送
cd app
npm install                  # 安装前端依赖
npm run tauri:dev            # 启动 Vite + Tauri 开发模式（仅占用 5173）
```

## 8. 发布与版本管理约定

### 8.1 版本号规则

采用 `0.x.0` 格式（minor 递增，patch 固定为 0）：

| 阶段 | 版本号 | 触发条件 |
|------|--------|---------|
| 日常开发 | 不增版 | 每次修改完成后仅执行 `npm run build` / `cargo check` 编译验证 |
| 重大更新 | `0.x.0` | 用户**明确要求打包**时，按用户指示的版本号迭代（如 0.2.0 → 0.3.0） |

### 8.2 打包流程

仅在用户明确说"打包"或"生成安装包"时执行。一条命令搞定：

```
powershell -ExecutionPolicy Bypass -File build-release.ps1        # 版本号取自 app/package.json
powershell -ExecutionPolicy Bypass -File build-release.ps1 0.9.1  # 或显式指定
```

脚本九步：① 预检 cargo / makensis ② kill 运行中的 lunac.exe / agent.exe ③ `npm run build`（前端）④ `cargo build --release`（core-agent → agent.exe，**必须早于第 5 步**）⑤ `cargo build --release`（src-tauri → lunac.exe）⑥ **清空并重建暂存目录** `release\Lunac\` + 拷 `lunac.exe` / `agent.exe` / `WebView2Loader.dll` + **拷 `agent-templates\{skills,tools}` → 暂存目录同名子目录**（README + `*.example` 模板，装完用户可照抄，见 §11 规则 24）⑦ 打包 VSCode 扩展 → `lunac.vsix` ⑧ 预置 PaddleOCR-json（本地 `paddle-ocr/` 优先，缺失则从 GitHub 下载 .7z）⑨ 改写 NSI 版本号 → **先删同名旧产物**（makensis 覆盖已存在文件时只会含糊地报 `Can't open output file`，实测于旧包刚生成、杀软仍在扫描它时）→ makensis → `release\Lunac-<版本>-Setup.exe`。

**NSI 脚本位置**：`scripts\lunac-installer.nsi`（**已入库**）。此前它放在 `release\` 内，而 `release\` 整体被 gitignore → 换个克隆就 `NSI script not found`，打包链路不可复现。脚本首部用 `!cd ${__FILEDIR__}\..\release` 锚定源文件目录：makensis 解析 `File` / `OutFile` 的相对路径用的是**脚本所在目录**而非调用方 CWD（实测从仓库根调用同样正确），因此 `File "Lunac\..."` 恒定解析到 `release\Lunac\`、Setup.exe 恒落在 `release\`，与 `Push-Location` 无关。

**产物不变量（两条都已在脚本里做成硬校验，违反即中止）**：

1. **必须含 `agent.exe`** —— 自研 AI 后端，`core_dir()` 只在安装根找它；少了它装完 AI 直接 `agent.exe not found`。
2. **不得含 `cli.exe`** —— 上游 Claude Code CLI 的 bun 编译产物（121MB），Anthropic 版权、**禁止再分发**（见 §8.3 红线）。**2026-09-13 之前的全部安装包（0.4.0–0.9.0）都含它**：NSI 里写的是 `File "Lunac\cli.exe"`，而暂存目录从不清理，那份僵尸文件就这么一路跟进每个包；且 2026-09-11 自研 agent 接线（`c5b6a10`）之后，新 `lunac.exe` 只认 `agent.exe`，于是脚本产出的包「既打不进 agent.exe、又删不掉 cli.exe」→ 装完 AI 不可用。

校验方式：打包前查暂存目录有无 `cli.exe`（`Test-Path`）；打包后用 7z 列包内清单（NSIS 文件表是 LZMA 压缩的，直接扫字节不可靠），命中 `cli.exe` 或缺 `agent.exe` 即 `throw`；没装 7z 则跳过并提示。

**日常修改不打包** — 仅编译验证即可。

### 8.3 开源发布 / 仓库卫生（2026-09）

**当前状态：仓库为 private（`LythrumMoon/Lunac`），开发完成后才公开。** 公开前必须重跑下面的红线和首次提交验证；README 作为对外「详细页」不展示 CLI 相关实现细节。

**红线（违反会造成密钥泄露或侵权，且不可撤销）**：

1. **`core/` 绝不入库** —— `core/` 是上游 Claude Code 源码（`core/package.json` → `"name": "claude-code-cli"`），公开分发会触发 DMCA。**自研 `core-agent/` 已上线，构建与运行都不再依赖它**，该目录仅作历史参考保留在本地（已 gitignore）。
2. **`.env` 绝不入库** —— `core/.env` 与 `app/src-tauri/.env`（`AI_API_KEY` 等）含真实凭据。密钥一旦进过 commit，即使后续删除仍留在历史中，必须立即作废换新。仅提交 `.env.example` 模板。
3. **大二进制不入库** —— GitHub 单文件硬上限 100MB、仓库 >1GB 告警。以下均已 gitignore：`core/`、`core-agent/target`、`app/src-tauri/target`、`target-e2e`、`binaries`、`app/dist`、`ui/dist`、`vscode-extension/out`、`node_modules`、`mingw64`、`paddle-ocr`、`release`、`local-models`。
4. **上游 `cli.exe` 绝不进安装包** —— `release\Lunac\cli.exe` 是上游 Claude Code CLI 的 bun 编译产物（121MB，VersionInfo：Product=Bun / Company=Oven），与第 1 条同源：**只允许留在本机作历史参考，不得随任何发行包分发**。已发布的 **0.4.0–0.9.0 安装包全都含它**（NSI 曾写 `File "Lunac\cli.exe"`），公开仓库前必须重新打包替换掉这些资产；`build-release.ps1` 已加打包前后双校验（见 §8.2）。

**README 对外页面纪律**：不出现上游 CLI / `cli.exe` 相关说明，不设「快速开始」栏目（构建与自检步骤仅在 `docs/` 与本规范内维护）。

**必备文件**：`.gitignore`、`LICENSE`（MIT，版权人 `LythrumMoon`）、`README.md`、`.env.example`。

**运行时按需下载的第三方资产**：`paddle-ocr/`（PaddleOCR-json，`hiroi-sora/PaddleOCR-json` v1.4.1，Apache-2.0 兼容）体积过大且属第三方产物，不入库也不随发行包分发。用户侧由前端触发 `ocr_engine_install` 从 GitHub Release 自动下载到 exe 根；构建侧由 [download-paddle-ocr.ps1](file:///d:/cc/claude-code-cli-master/scripts/download-paddle-ocr.ps1) 预置（打包离线版时才需要）。注意该 Release 的 **Windows 资产是 `.7z` 而非 `.zip`**，`Expand-Archive` 解不了，必须走 `sevenz-rust`（Rust 侧）或 7z.exe / bsdtar（脚本侧）。

**首次提交前必须验证**（缺一不可）：

```powershell
git add -A
git diff --cached --name-only | Select-String "\.env"      # 只应出现 .env.example
git diff --cached --name-only | ForEach-Object { Get-Item $_ -EA SilentlyContinue } |
  Sort-Object Length -Descending | Select-Object -First 10  # 不应出现 MB 级文件
```

**其他**：commit message 含中文时，用 `git commit -F <UTF8 文件>` 或先设 `[Console]::OutputEncoding = [Text.Encoding]::UTF8`，避免 PowerShell 传参转码成乱码。远端仓库需手动创建（本机未装 `gh`）。

## 9. 当前问题

| 问题 | 状态 |
|------|------|
| 搜索插件结果显示 | ✅ 已验证正常 |
| 窗口拖拽 | ✅ uTools 式三区域拖拽：搜索栏（3px 阈值）、输入框（拖拽时 `pointerEvents:none` 禁用光标 + `startDragging`）、插件标题栏（delegate） |
| 全局热键（默认 `Ctrl+Alt+Space`） | ✅ 已验证正常 — 双后端：`RegisterHotKey` 优先（内核级、无键盘钩子），注册失败才回退 LL 钩子；另有子类化与 JS 两道兜底，见 §2.3 |
| Esc 行为 | ✅ 逐级退出（全由 Rust 统一判定、前端只处理「层内」动作）：录制中 → 取消录制；`UI_MODE != main`（插件 / 详细搜索）→ 交给前端退层；`main` + 有内容 → 清空；`main` + 空白 → 隐藏窗口。判据是**界面层**而非内容，见 §2.1.2 |
| 设置按钮进入设置面板 | ✅ 支持 toggle：已打开设置时再点击关闭；ESC 关闭任意插件面板 |
| 幽灵框透明区点击穿透 | ✅ `pointer-events: none` 策略 |
| AI 对话 (Agent 模式) | ✅ Agent 单模式 — agent.exe（自研 core-agent）直连 + cli-output 流式渲染（简单模式已移除） |
| Agent 模式前端接入 | `main.ts` 监听 `cli-output` SSE 事件，流式渲染 Agent 对话 | ✅ 已完成 |
| Start Menu 实时模糊搜索 | ✅ 已集成（`main.ts` `search_apps`） |
| 计算器/编码/JSON 插件 | ❌ 已移除 — 2026-07-22 删除，功能由 AI Agent 替代 |
| OCR 文字识别插件 | ✅ 新增 `ocr.ts` — PaddleOCR-json 离线 OCR（多语言） |
| 硬件 AI Agent Tools | ✅ 新增 `tools/system_info.json` — Agent 模式 MCP Tools |
| Agent 内置工具（Read/Write/Edit/Bash/PowerShell/Glob/Grep/WebSearch/WebFetch/AskUserQuestion/TodoWrite） | ✅ P1 已完成 — 真实端点烟测通过（多轮工具往返、工作区越界拒绝、`plan` 档只读；`PowerShell` 中文输出与 `--disallowedTools` 裁剪均验证）。`WebFetch` 已完成（2026-09）：HTML→纯文本抓取，非只读档与**只读档都走审批**，真实文档页（doc.rust-lang.org）烟测通过。`AskUserQuestion` 已完成（2026-09）：选项卡片 + `updatedInput` 回答案，答题/未答/拒绝/plan 四条路径烟测通过。`TodoWrite` 已完成（2026-09）：待办面板、免审批、成功回执不重复渲染。`WebSearch` 已完成（2026-09，**同日重构**）：主源改为可配置多后端（博查 / Tavily / Exa / Firecrawl，设置面板下拉），兜底源由国内不可达的 DuckDuckGo 换成 **Bing RSS → Bing HTML → 百度**。真实端点烟测（`deepseek-flash`）两条 PASS：①未选服务商/未配 key → 直接走 Bing RSS 兜底并回真实中文结果（结果尾部含 `[fallback] 未选择搜索服务商…`）；②`provider=tavily` + 无效 key → Tavily 真返 401 并把响应正文带回 `[fallback]`，随后 Bing 兜底成功。三个解析器另用真实抓取页面离线验证：Bing RSS 10 条 / Bing HTML 10 条（带摘要）/ 百度 9 条（`mu` 取到真实 URL）。⚠️ **Exa / Firecrawl / 博查的「成功」路径尚无真实 key 复验**（本次只验到请求形状 + 错误透传 + 回落）。**该缺口已决定「不修」（2026-09-17 用户定）**：四家都是**付费**服务，预算原因拿不到可用 key ⇒ 它们的「成功」路径**不是验收项**，只保证「请求形状 + 错误透传 + 回落」正确即可。**验收口径改为「兜底链必须可用」** —— 兜底源全免费、无需 key，是「未配 key 也能搜」这句话的真正支撑。复验方式 = 跑一次**联网的** `#[ignore]` 用例（唯一依赖外网的测试，故意不进常规 `cargo test`）：<br>`cd core-agent && cargo test fallback_scrapers -- --ignored --nocapture`<br>它逐级打印结果并断言「三级至少一级可用」**且**「Bing RSS 单独必须活着」（后者是链的首选，不单独钉就会被「至少还有百度」掩盖成静默降级）。**2026-09-17 实测：三级全部 OK 各 5 条**（Bing RSS / Bing HTML / 百度，2.84s 含两次 1.1s 节流）。**对方改版后必须重跑这一条** |
| Agent 工具权限审批（can_use_tool） | ✅ P2 已完成 — 写类工具执行前弹卡，allow/deny/interrupt 与超时撤卡均验证通过 |
| Agent 上下文预算与压缩 | ✅ 已完成 — 按端点实测体积走瘦身/丢弃两级水位 + 400 强制压缩兜底；**摘要式压缩**（只挂丢弃档 / 400 兜底档，2026-09-18）见 §11 规则 39；真实端点烟测（`LUNAC_MAX_CONTEXT_TOKENS=8000`）连跑 17 轮工具往返不中断 |
| Agent MCP 工具桥（插件面板的 tools\*.json） | ✅ P3 已完成 — agent.exe 作 client 连 `lunac.exe --mcp-server`，用户工具以 `mcp__<名>` 进请求体；真实端点烟测通过（注册、审批卡、成功/失败两条回灌路径） |
| Agent 技能（技能扩展面板的 `<exe 根>\skills\<key>\SKILL.md`） | ✅ P4 已完成 — agent.exe 读 `LUNAC_SKILLS_DIR`，提示词只列 `key: 描述`，模型调 `Skill` 拿到正文（`$ARGUMENTS` 已替换）；面板增删改后自动重启 agent 生效 |

### 9.1 疑难点（待攻关）

> 这两条是**当前没有定论**的硬骨头：事实与可借鉴的实现已经查清，但**改法尚未决定**。每条记录「现象 / 已知事实（带证据）/ 可借鉴资料 / 候选方向 / 验收口径」，避免下次重新调研。

#### 难点 1：DeepSeek 侧的 AI 缓存命中率（与平台口径对不上）

**现象**：长会话的命中率明显低于 dsh（DeepSeek Harness）社区自述的 97–99% 区间；按天落盘的 `ModuleData\usage\usage-*.jsonl` 与 DeepSeek 平台用量页的「缓存命中」也对不上。

**已知事实**

1. **DS 的命中规则比「共同前缀」严格**：只认**完整匹配已落盘的缓存前缀单元**。落盘时机三条 —— ① 每次请求的**用户输入结束位置**与**模型输出结束位置**各产生一个单元；② 系统检测到多次请求存在公共前缀时，把该公共前缀**单独落盘**（所以「第二次不中、第三次才中」是正常现象）；③ 长输入/长输出按**固定 token 间隔**切单元。单元粒度 **64 token**（不足不缓存）、**尽力而为**不保证 100%、不用时数小时~数天后自动清空。官方文档说「缓存构建耗时秒级」，但**本项目的对照实验证明这秒级对本侧够快**（0ms 间隔的连续请求照样命中 97.9%，见下方 ⑤）—— 所以别再拿「来不及落盘」当低命中的解释。
2. **本侧请求形状已经是理想形状**：`system = SYSTEM_PROMPT + env_block(cwd) + skills::listing()` 在 `main()` 里**只构建一次**（[main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs#L762-L766)），进程内逐字节不变；`tools` = 内置 + `Skill` + MCP（MCP 按名排序，见 §11 规则 18）；`history` 严格 append-only。⇒ 差距**不在形状**，只可能在「压缩改写前缀」「DS 落盘规则」「统计口径」三处。
3. **本侧统计口径**：前端 `hit = cache_read_input_tokens`、`miss = input_tokens + cache_creation_input_tokens`（[main.ts](file:///d:/cc/claude-code-cli-master/app/src/main.ts#L2165-L2177)），agent 侧对**一轮内每次 API 请求累加**（[main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs#L1179-L1195)）。DS 的 `/anthropic` 兼容端点会把顶层 `system` 折成 `messages[0]`、且 **`cache_control` 断点标记基本被丢弃**（DeepSeek 不实现该协议），所以本侧**用不上 Anthropic 那种显式缓存断点**，只能靠前缀自然匹配 —— 这也意味着「抄 Anthropic 的 `system_and_3` 断点」在这里没有意义。
4. **已知的主动断裂源只有压缩这一类**：`compact_history` 的 elide / drop / 首条插 `TRIMMED_MARKER` / 钉回任务快照 / 钉回摘要。已有缓解 = 水位 0.85 / 0.95 + 瘦身档滞回（§11 规则 23）。**后三项都只在「本轮真的 `dropped > 0`」时才发生**，即都挂在 `drop` 这一条上、不额外制造压缩时机。
5. **现状没有任何「每次请求」粒度的记录**：日志里只有整轮累计值，无法与平台按请求对账。

**可借鉴的三份资料（本次实地查过）**

| 资料 | 关键做法 | 对本项目的启示 |
|---|---|---|
| **dsh（DeepSeek Harness）对话链路** | 会话是**事件溯源 append-only 日志**（历史写入即不可变）；系统提示词按模式固定；每轮只把新消息追加到尾部。原文结论：「前缀缓存稳定性是架构的**推论**，而不是被管理的目标」 | 本侧已是同一形状 ⇒ **别再为了省事去改早期消息**；任何「重排 / 回填 / 就地编辑历史」都是命中率杀手 |
| **ponytail 的 skills 实现形式**（[仓库](https://github.com/DietrichGebert/ponytail)） | 一个 **always-on 规则文件**（同一份 ~2.5 KB 内容镜像进每个 agent 的原生 rule 路径：`.agents/rules` / `.clinerules` / `.cursor/rules` / `.windsurf/rules` / `copilot-instructions.md`，blob 完全相同）+ **按需加载的 `SKILL.md`**（1.4–6 KB，含 audit / debt / gain / help / review 五个）+ 两个轻量 Node 生命周期钩子 + 三档强度 | **固定前缀必须短**：能按需加载的能力（技能正文、工具 schema）都不要塞进每轮都发的前缀。本侧 `skills::listing()` 只列 `key: 描述`、正文由 `Skill` 工具按需取 —— 方向一致，可继续沿用到工具 schema |
| **Hermes 的省 token 机制**（本机 `%LOCALAPPDATA%\hermes\hermes-agent`，逐文件查过） | ① system prompt **一 session 只构建一次、逐字节回放**；② 时间戳**降精度到日**且放 volatile 段**最末**；③ 插件上下文 / 记忆 recall **一律注入 user 消息**，绝不动 system；④ 传输层用**内容寻址 cache key**（`instructions` + **按名排序的 tools** 取 hash），而**不是** session_id（注释：用 session_id 会让 cron 每次 cache-cold）；⑤ **压缩是唯一被允许的前缀失效时机**，压缩后统一重建 system prompt，平时严格 append-only；⑥ **工具 schema 按需检索**（核心工具常驻，其余 BM25 桥接，默认在 deferrable schema 超上下文 10% 时才启用）；⑦ 单条工具输出设预算（默认 10 万字符落盘、内联仅 1500 字符预览） | ③⑤ 本侧已等价做到；①④⑦ 是**可直接抄**的；⑥ 是**最大的一块可省前缀**（本侧工具 schema 目前全量注入）；② 本侧 `env_block` 无时间戳，已天然满足 |

**实测与处置（2026-09-15，按「先对账再动手」执行完一轮）**

| # | 动作 | 结果 |
|---|---|---|
| ① | **补「每次 API 请求」粒度埋点** | 已做。agent 在 `message_stop` 记一条 `{in, read, create, out}`，随 `result.usage.requests` 上报，前端写进 `usage-*.jsonl` 的 `requests` 数组 —— 从此能与平台用量页**逐行**对齐 |
| ② | **量固定前缀体积**（我原以为工具 schema 是「最大的一块」） | **前提不成立**。实测 `system=1058 字` + `tools=11 个/5860 字` ≈ **1730 tokens，只占 128k 预算的 1.4%**（启动时落盘一行 `固定前缀 …`）。⇒ 工具 schema 按需检索（Hermes `tool_search`）**决定不做**：省下的上限也就几百 token，却要引入检索桥 + 一次前缀变动，是净亏 |
| ③ | **压缩边界收紧** | 已做。瘦身档加「值不值得」闸门：**可省体积 < 当前上下文的 5% 就不许动历史**（`ELIDE_MIN_SAVINGS_RATIO`）—— 旧实现只要过 0.85 水位就压，于是频繁出现「省 2% 体积、废掉 60% 前缀」。闸门只作用于瘦身档（可选档）；丢弃档与 400 兜底档是安全刚需，照旧无条件压。同时「决定不动」不再推进滞回时钟 |
| ④ | **用本地真实日志反推命中率** | 见下。**结论：不是代码 bug** |

**④ 的关键数据**（`app/src-tauri/target/debug/ModuleData/usage/`，dev 实例真实记录）：

| 日期 | input（未命中） | cacheRead（命中） | 命中率 |
|---|---|---|---|
| 09-13 共 6 轮 | 2830 / 1315 / 2287 / 279 / 708 / 4612 | 12544 / 15104 / **0** / 2432 / 8960 / 15744 | 81.6% / 92.0% / **0%** / 89.7% / 92.7% / 77.3% |
| 09-14 共 1 轮 | 6340 | 8192 | 56.4% |
| **合计** | 18371 | 62976 | **77.4%** |

- **本地命中率已经在自然上限附近**：例如 `in=4612 / read=15744` → 77.3%，恰好等于「上一轮上下文 ÷（上一轮 + 本轮新增）」——即 §11 规则 23 里那个上限公式。**差距来自会话太短**（这些记录上下文只有 15–20K），不是缺陷；dsh 的 97–99% 是**几十轮以后**新尾占比才变小的结果。
- `read=0` 那一轮（in=2287，且 `elided/dropped` 都是 0）**不是「来不及落盘」**（⑤ 已证伪这条），剩两种可能：**前缀本身变了**（`skills::listing()` / 工具黑名单 / `cwd` 任一变化）或 DS 侧缓存被清。下一条立刻回到 89.7% 说明前缀随后又对上了 —— 值得用新加的埋点复现一次。
- 这几轮 `elided/dropped` 全是 0 ⇒ 压缩在这些会话里**根本没触发**，所以当时的命中率与压缩无关；③ 是**预防性**收紧。

| ⑤ | **间隔对照实验**（0ms vs 6s） | 已跑完，**「落盘慢」被证伪** —— 见下 |

**⑤ 的对照实验（2026-09-15 跑完，`deepseek-v4-flash`）**

方法：同一前缀 P（40500 字 ≈ **9016 tokens**），`R1 = P+Q1`，`R2 = P+Q1+Q2`（纯追加）；A 组两问**紧挨着**发，B 组**先等 6 秒**再发第二问；用不同 nonce 隔离两组避免互相污染。

| 组 | 请求 | input（未命中） | cacheRead（命中） | 命中率 |
|---|---|---|---|---|
| A | 第 1 次（冷） | 9016 | 0 | 0% |
| A | 第 2 次（**间隔 0ms**） | 193 | 8832 | **97.9%** |
| B | 第 1 次（冷） | 9016 | 0 | 0% |
| B | 第 2 次（**间隔 6s**） | 193 | 8832 | 97.9% |

- **0ms 与 6s 的结果逐字节相同** ⇒ 对本项目这种「一轮内连续几次工具往返」的间隔，DS 的落盘**足够快**，**不会因为「还没落盘」而丢命中**。原先那条「秒级落盘是低命中主因」的假设**被证伪**。
- 第 1 次必然 0%：全新前缀没有任何已落盘单元 —— **规则性的**，不是缺陷。
- 命中量 8832 / 9016 ≈ 98%（即上一条请求的输入几乎整段被复用），与 §11 规则 23 的上限公式一致。

**因此留下的唯一疑点**：dev 日志里那条 `in=2287 / read=0` **不能**再用「来不及落盘」解释（0ms 都能中）。剩余可能只有两种 —— ① **前缀本身变了**（`skills::listing()` / 工具黑名单 / `cwd` 任一变化都会改 `system` 或 `tools`）；② DS 侧缓存被清。现在有了 `requests[]` 与启动时那行 `固定前缀 …`，下次复现时可直接对照两者的前后取值来定位。

复现脚本（原样保留，换 key 后可直接重跑）：

```powershell
# 在 app/src-tauri 下执行（.env 需有有效的 AI_API_KEY）。
# A 组：R1 与 R2 紧挨着发；B 组：先等 6 秒再发 R2。比较两组 R2 的 cache_read_input_tokens。
# R2 = R1 的原文 + 一句追加 ⇒ 纯 append-only，符合「缓存前缀单元」的匹配前提。
$kv=@{};Get-Content .\.env|?{$_ -match '^[A-Z_]+='}|%{$p=$_ -split '=',2;$kv[$p[0]]=$p[1].Trim()}
$model=$kv['AI_MODEL'];$uri='https://api.deepseek.com/anthropic/v1/messages'
$hdr=@{authorization="Bearer $($kv['AI_API_KEY'])";'anthropic-version'='2023-06-01'}
$filler=('The quick brown fox jumps over the lazy dog. '*900)
$Ask={param($t)$b=@{model=$model;max_tokens=1;stream=$false;thinking=@{type='disabled'};messages=@(@{role='user';content=@(@{type='text';text=$t})})}
  $j=$b|ConvertTo-Json -Depth 12 -Compress
  (Invoke-RestMethod -Uri $uri -Method Post -Headers $hdr -ContentType 'application/json' -TimeoutSec 180 -Body ([Text.Encoding]::UTF8.GetBytes($j))).usage}
$pa='[A-nonce]'+$filler;$u1=&$Ask ($pa+[char]10+'Reply with the single word: ok')
$u2=&$Ask ($pa+[char]10+'Reply with the single word: ok'+[char]10+'Now reply: done')
Start-Sleep -Seconds 6
$pb='[B-nonce]'+$filler;$u3=&$Ask ($pb+[char]10+'Reply with the single word: ok')
$u4=&$Ask ($pb+[char]10+'Reply with the single word: ok'+[char]10+'Now reply: done')
"A(0ms) 2nd: in=$($u2.input_tokens) read=$($u2.cache_read_input_tokens)"
"B(6s)  2nd: in=$($u4.input_tokens) read=$($u4.cache_read_input_tokens)"
```

判读口径（本次的实际读数即「两组都接近整段前缀」那种）：两组 `read` 相同 ⇒ **落盘不是瓶颈**，低命中只能由「新尾占比大」或「前缀本身变了」解释；若 B 组明显大于 A 组，才说明「落盘要时间」是短间隔连续请求丢命中的主因（属规则性，改代码无用）。

**验收口径**：同一会话连续 20 轮工具往返，逐轮命中率不低于「上一轮上下文长度 ÷ 本轮总输入」这一自然下限；且压缩次数与未命中增量可分离统计（现在两者都有落盘字段：`requests[]` 与 `elided`/`dropped`）。

#### 难点 2：Windows 开机自启 Lunac 慢（**不是** Lunac 自身冷启动慢）

**现象**（用户 2026-09-15 澄清）：**手动双击是秒启**；但由 Windows 开机自启拉起时要等很久才可用（表现为热键很久才响应）。所以问题不在 Lunac 的冷启动，而在「**开机场景 + 自启机制**」这一组合。

**已查明的事实（通读自启链路源码）**

1. **代码上，自启比手动启动几乎不多做任何事**：`is_background` 在整个 Rust 侧只出现 3 次（解析 / 日志 / 是否 `show`），**唯一的行为差异是 `main.rs` 少调一次 `w.show()`**。窗口与 WebView2 由 `tauri.conf.json` 声明（`visible: false`），两种启动方式**都会创建窗口、都会在后台加载 WebView2**（代码注释原话：`a boot launch loads WebView2 invisibly`）。热键在 `setup` 里**同步注册**（[hotkey.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/hotkey.rs) `start_hotkey`），热键可用只取决于 `setup` 返回 + 事件循环启动。⇒ **「自启慢」不是 Lunac 内部多做了一步。**
2. **热路径上没有固定 sleep**：全仓 8 处 `thread::sleep` 都不阻塞窗口/热键（4s 在 auto-start 修复线程、600ms 在索引线程、100ms 只在换键时命中、其余是轮询/重试间隔）。历史元凶 —— `--background` 分支里固定 15s 的 sleep —— 已于 2026-08-04 移除（[main.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/main.rs#L109-L115) 注释为证）。
3. **项目自己实测过两种自启机制差约 60 秒**（[auto_start.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/auto_start.rs#L1-L11) 原文）：计划任务（`schtasks /sc onlogon`）**~19s** 触发，HKCU Run 键 **~79s**。这段差距**完全来自 OS 触发时机**（onlogon 任务 vs Explorer 串行处理 Run 项），与进程内代码无关。
4. **非管理员下必然落到慢的那一条（已定案为权限问题）**：`enable_auto_start()` 先建计划任务，失败才回退写 HKCU Run。经日志实证失败原因是 `错误: 拒绝访问。` —— 计划任务在任务计划**根目录**，非管理员建不了（NSIS 安装模式是 `currentUser`，所以对普通用户这是**必然**而不是偶发）。而当时的 `is_auto_start_enabled()` **只返回 `bool`**，UI 从不告诉用户实际生效的是哪一条 ⇒ 用户感受到的延迟可能是 19s 也可能是 79s，且无从判断。**这是最能解释「开机自启慢」的一条。**
5. **每次开机都无条件重建自启项**：`repair_auto_start_on_startup()`（延迟 4s 的后台线程）在 Run 键存在时**无条件重写注册表**；计划任务方式下则 `schtasks /query` + `schtasks /create` **各 spawn 一次 `schtasks.exe`**。而它真正要防的只有「exe 被移动/升级导致路径失效」。不阻塞热键，但与开机 IO 高峰叠加 —— 代码注释亲口说 `right after a boot auto-start the Task Scheduler service may still be starting, so a synchronous call could block the app for a long time`。
6. **曾经没有单实例保护**（全仓无 `CreateMutex`、无 `tauri-plugin-single-instance`）：自启实例已在后台时用户再双击 → 第二实例的 `RegisterHotKey` 必然失败 → 退化装 `WH_KEYBOARD_LL` 全局钩子（同时也是杀软误报 `Prowloc` 的成因）。这会让热键表现为「要按好几次 / 反应慢」。**已修，见下「已做」第 7 条。**
7. 前端 `index.html` 曾从 CDN 同步拉 KaTeX 造成白屏（注释里写明 `was a white-screen startup delay`），现已改为非阻塞；且 release 的 CSP 不含 `cdn.jsdelivr.net`，远程脚本会被直接拦掉 ⇒ 本配置下大概率不构成开机延迟（待实测确认）。

**已做（2026-09-15）**

1. ✅ **自启机制判定下沉 + 只进日志**：新增 `auto_start_info()` 返回 `{enabled, mechanism}`（`mechanism ∈ task / run / both / none`），**旧的 `is_auto_start_enabled()` 已删除**（它只能回答「开没开」），`get_auto_start` 命令换成 `get_auto_start_info`。前端只用 `enabled` 拨开关；`mechanism` 每次打开设置面板记一行「探测 run=? task=? ⇒ 机制=?」到落盘日志。**一度在面板上显示过机制小字 + 「改用计划任务」按钮，已按用户要求撤掉** —— 「Run 键 / 计划任务」是纯实现术语，终端用户既不会注意也看不懂。
2. ✅ **自启项改成「按需重建」**：`repair_auto_start_on_startup()` 先比对再动手 —— Run 值等于 `startup_command_line()` 就什么都不做；否则读计划任务的 `<Command>` 与当前 exe 比对，一致就跳过。消掉了此前**每次开机**的两次 `schtasks.exe` 与一次注册表写入。
3. ✅ **自启决策全部落盘**：`auto_start.rs` 里原来用 `eprintln!` 的那些决策日志（启用走了哪条、修复检查是跳过还是重建）在 release 下**写进空气**（GUI 子系统无控制台）—— 已统一改走 `crate::log::info`，落在 `<exe 根>\temp\logs\lunac-YYYY-MM-DD.log`，前缀 `auto_start:`。于是「开机到底走了哪条机制、有没有多 spawn `schtasks`」都可事后查证（`grep auto_start`）。**新增可诊断代码时不要用 `eprintln!`。**
4. ✅ **权限根因定案**：日志实证 `schtasks create failed: 错误: 拒绝访问。`（GBK 原文，被按 UTF-8 解码成乱码，判读时别被吓到）⇒ **不是 `/tr` 引号/转义问题**，是非管理员无法在任务计划根目录建任务，所以默认**必然**落到慢约 60 秒的 Run 键。原打算用「临时探针任务」区分权限/参数，日志一出就无需再探（`cmd /c` 形式的探针也会被本机安全策略拦下）。
5. ✅ **提权链路**：`ShellExecuteExW("runas")` 提权**跑自己的 exe**（`--lunac-auto-start-task=create|delete`）而不是直接提权跑 `schtasks` —— 后者的 `/tr` 里带引号路径与 `--background`，经 ShellExecute/PS/cmd 逐层转义极易出错；跑自己则命令行只是一个开关，真正的 `schtasks` 参数仍由已验证的 Rust 代码拼装。父进程 `WaitForSingleObject` 等它结束再**复查任务是否真的存在**（不轻信退出码）；用户点「否」时 `ShellExecuteExW` 直接失败（`ERROR_CANCELLED`），此时**绝不能顺手清 Run 键**，否则会把用户原有自启弄没。**删除同样走提权**（任务在根目录，删不掉就是「关不掉的自启」）。
6. ✅ **开关改成一次点到位**：`enable_auto_start()` = ① 试建任务（本进程已是管理员则直接成功、不弹 UAC）→ ② 失败则**先写 Run 键保底** → ③ 自动弹一次 UAC 补建任务，成功就清 Run。取消 UAC 只在日志里记「继续用 Run 键」，**不算失败**（开关仍开、只是慢），避免「弹了窗、点了否、结果什么都没开」。`disable_auto_start()` 同步化：任务存在时先试无提权删除，仍存在则提权删除，最后复查，删不掉就返回 `Err` 让开关弹回去。
7. ✅ **单实例保护**（原「待定」第 1 条）：新增 `single_instance.rs` —— 会话级命名互斥体 `Local\LunacSingleInstance` + 唤出事件 `Local\LunacActivate`（手写 FFI，约定同 `hotkey.rs`，未新增依赖）。第二实例立即退出；**非 `--background`** 的那次双击额外 `SetEvent` 让已有实例 `show_and_focus()`；`--background` 不唤出（机制为 `both` 时开机有两个实例，第二个不该弹窗）。名字里不含版本/exe 路径 ⇒ dev 与 release 互相排斥（用户明确要求）。进程退出（含被杀）由内核回收句柄，无残留文件、无陈旧锁。
   - **真机验证（2026-09-15）**：`.NET OpenExisting` 能打开两个命名对象（证明 FFI 真建出来了）；新起的两个实例各记一行「已有实例在运行 → 本进程退出」并秒退，持有者仍在运行；对事件 `Set()` 后持有者记「收到唤出信号（用户又启动了一次）→ show_and_focus」且主窗口转为可见。`--lunac-auto-start-task=bogus` 正确走到提权分支、退出码 `1`。

**过程中的两个硬事实（踩过，别再犯）**

- **`schtasks` 的文本输出字段名会随系统语言本地化**：本机中文 Windows 上 `schtasks /query /fo LIST` 里连 `TaskName:` 都匹配不到（实测）。所以任务内容必须走 **`/xml`** —— `<Command>` 是固定标签，与语言无关。
- **`/xml` 的输出编码无法在本机实测**：沙箱不允许枚举系统计划任务（`Get-ScheduledTask` 与 `schtasks /query` 都被 `win32 error 5` 挡下），本机又没注册过任何 Lunac 自启项。因此 `decode_task_xml()` **UTF-8 与 UTF-16LE 两种都试**（判据是解出来能否找到 `<Command>`），并且把「读不出内容」与「没有任务」**分成两种结果**：后者什么都不做，前者**按旧行为重建** —— 解析失败绝不能变成「以后不再自愈」的静默回归。`decode_task_xml` 有单测覆盖两种编码。

**待定**

1. **实测确认**前端 CDN 与 WebView2 冷启动在开机场景的真实占比；若确认无关，就在文档里写死这个结论，不再靠猜。

**验收口径**：实际生效机制可在落盘日志里查证（`grep auto_start`）✅；开机后「登录完成 → 热键首次可响应」的时长与手动启动的差值不超过 OS 触发时机本身的差异（走任务 ~19s 量级）；自启实例在运行时再双击**不产生第二个进程**，且能把已有窗口唤出 ✅（2026-09-15 已实测）。

## 10. 待办路线

1. ~~**模式选择器 UI**~~ — ✅ 已完成：状态栏模式指示 + 设置面板模式切换 + 状态同步
2. ~~**Agent 模式前端接入**~~ — ✅ 已完成：`main.ts` 监听 `cli-output` SSE 事件，流式渲染 Agent 对话
3. ~~**接入 Start Menu 实时搜索**~~ — ✅ 已完成
4. ~~**热键可配置**~~ — ✅ 已完成：settings 面板自定义 Alt+key 组合热键
5. **Agent Tools 扩展** — 继续接入更多开源的硬件 AI skill/tools (LocalAI skills, OpenJarvis skills 等)
6. ~~**P2 权限审批**~~ — ✅ 已完成：写类工具发 `can_use_tool`，前端卡片 allow/deny（含 interrupt 中断本轮），300s 超时自动撤卡；见 §3.5
7. ~~**安全档位可切换**~~ — ✅ 已完成（2026-09）：设置 · AI 面板加「安全档位」下拉（只读 / 项目 / 完全）+ 输入栏「运行方式」胶囊（手动 / 白名单 / 自动），`set_security_profile` 接线完成；见 §3.5 与 §11 规则 21
8. **补齐 agent 后端能力** — 与旧 `cli.exe` 的差距按 [agent-feature-backlog.md](file:///d:/cc/claude-code-cli-master/docs/agent-feature-backlog.md) 的分级与顺序推进，**§6-4「低成本高收益」已全部落地**（PowerShell / 工具黑名单 / WebFetch / AskUserQuestion / TodoWrite / WebSearch）。剩余：MCP `resources` 未做（server 侧缺 `resources/read`）
9. ~~**上下文预算与压缩**~~ — ✅ 已完成：见 §3.5「上下文预算与压缩」（backlog §2.1 第 1 项，唯一「用久了必然坏掉」的缺口）
10. ~~**P3 MCP 工具桥**~~ — ✅ 已完成：见 §3.5「P3 已完成（MCP 工具桥）」；`ListMcpResourcesTool` / `ReadMcpResourceTool` 仍未做（server 侧缺 `resources/read`）
11. ~~**P4 技能 SKILL.md**~~ — ✅ 已完成：见 §3.5「P4 已完成（技能 SKILL.md）」；fork / remote 两种模式未做
12. ~~**`PowerShell` 工具 + 前缀缓存命中率优化**~~ — ✅ 已完成（2026-09）：`PowerShell` 进内置工具表（真机烟测：中文输出、`plan` 档拒绝、`--disallowedTools` 裁剪）；MCP 工具数组按名排序、工具黑名单换真实名单，不变量见 §11 规则 18
13. ~~**`WebFetch` 工具**~~ — ✅ 已完成（2026-09）：HTML→纯文本、`reqwest` 超时/重定向/体积封顶；**不做二次模型摘要与域名预检**（旧 CLI 那两处依赖 Anthropic 服务端）；只读档同样走审批（唯一外部数据出口）；烟测见 §9
14. ~~**`AskUserQuestion` 工具**~~ — ✅ 已完成（2026-09）：选项卡片 + 复用 `can_use_tool` 的 `updatedInput` 回答案（不新增协议）；工具的「始终允许」被刻意禁用、白名单也不放行它；plan 档可用且照常审批；见 §3.5「结构化提问」
15. ~~**`TodoWrite` 工具**~~ — ✅ 已完成（2026-09）：工具不持有状态（清单唯一真相 = 模型最近一条 `tool_use`），前端拿流式入参就地重绘 `.todo-panel`；免审批、成功回执不重复渲染；见 §3.5「待办面板」
16. ~~**用量口径修正 + 本地用量日志**~~ — ✅ 已完成（2026-09）：前端不再对 `result.usage` 做差（自研 agent 报的是每次提问的绝对值，做差会把未变化的前缀算成 0 命中）；每次提问落一行 `<exe根>\ModuleData\usage\usage-YYYY-MM-DD.jsonl`，token 仪表盘改为「今日累计」，可与供应商平台按天对账；见 §3.5「用量与对账」与 §11 规则 19
17. ~~**`WebSearch` 工具**~~ — ✅ 已完成（2026-09，**同日重构为多后端**）：主源 = 设置 · AI 的「搜索服务商 + 搜索 API 密钥」（`AI_SEARCH_PROVIDER`/`AI_SEARCH_KEY` → `LUNAC_SEARCH_PROVIDER`/`LUNAC_SEARCH_KEY`，可选 bocha / tavily / exa / firecrawl），兜底 = **Bing RSS → Bing HTML → 百度 HTML 抓取**（≥1.1s 节流、202/429 视为限流），三级全失败就如实报错、不编造；plan 档照常审批。换掉原 DuckDuckGo 兜底的原因：本机实测国内连不上（15s 超时），而 Bing / 百度可达 —— 「不配 key 也能搜」必须是真的；Bing Search API 已于 2025-08-11 退役、DDG 无官方 API，故兜底只能抓结果页。选型、回落语义与解析要点见 §3.5「联网检索」；烟测见 §9

---

## 11. 关键规则与既定决策（精简规范）

> 仅保留仍然有效的架构规则与既定决策。逐条开发日志不再维护：修改以代码注释、Git 提交记录与 code-rules.md 承载。

1. **开机自启**：开启时**优先创建「用户登录时」计划任务**（更早触发），创建失败（非管理员**必被拒**）**自动回退 HKCU Run** 写入 `"<exe>" --background`；关闭自启时清掉 Run 值与任务两者；状态 = Run 值存在 或 计划任务存在。相关 `schtasks` 只出现在用户点击开关/启动后后台修复线程，**绝不阻塞启动热路径**。实现见 [auto_start.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/auto_start.rs)。
   - **权限事实与提权链路（2026-09，不得回退）**：计划任务落在任务计划**根目录**（`C:\Windows\System32\Tasks`），非管理员**创建与删除都会被拒**（实测日志原文 `schtasks create failed: 错误: 拒绝访问。`）。所以两条都得能提权：提权方式是 `ShellExecuteExW("runas")` **跑自己的 exe**（`--lunac-auto-start-task=create|delete`，参数是不含引号/空格的开关，绕开 `/tr` 那种多层转义），父进程 `WaitForSingleObject` 等子进程结束后**复查任务是否真的存在**（不轻信退出码）。**删除也必须能提权**，否则会出现「关不掉的自启」——比「慢」更糟；删不掉时如实报错让开关弹回去，禁止只 spawn 个后台线程了事。
   - **开关一次点到位（2026-09，用户明确要求）**：`enable_auto_start()` 顺序固定为 ① 直接试建任务（本进程已是管理员则不弹 UAC）→ ② 失败则**先写 Run 键保底**（无条件成功，开关绝不空转）→ ③ 再自动弹一次 UAC 补建任务，成功就清 Run。**取消 UAC 不算失败**（仍是「已开启、只是慢」），绝不能出现「弹了窗、点了否、结果什么都没开」。禁止改回「先显示机制、再给一个『改用计划任务』按钮」那种两步式设计。
   - **机制名只进日志、不进 UI（2026-09，不得回退）**：`auto_start_info()` 返回 `{enabled, mechanism}`（`task`/`run`/`both`/`none`），但 `mechanism` **只用于落盘日志**（每次打开设置面板记一行「探测 run=? task=? ⇒ 机制=?」）。「Run 键 / 计划任务」是纯实现术语，终端用户既不会注意也看不懂 —— 一度在设置面板显示过机制小字与升级按钮，用户明确要求撤掉。**禁止把 `is_auto_start_enabled()` 那种只回 bool 的实现加回来**（排查「开机后要等很久」时那个字段是唯一线索，但它不必给用户看）。
   - **单实例保护（2026-09，不得回退）**：见 `single_instance.rs` —— 会话级命名互斥体 `Local\LunacSingleInstance`，第二实例**立即退出**；非 `--background` 的那次双击还要 `SetEvent(Local\LunacActivate)` 让已有实例 `show_and_focus()`。命名对象**不得含版本号或 exe 路径**（否则 dev / release 会各起一个，用户明确要求二者互相排斥）；用 `Local\` 而非 `Global\`（后者要 `SeCreateGlobalPrivilege`）。缺了它，「自启实例在后台 + 用户双击」会让第二实例 `RegisterHotKey` 失败并退化成 `WH_KEYBOARD_LL` 全局钩子（热键时灵时不灵，也是 Defender 报 `Prowloc` 的成因）。
   - **`--background` 不得唤出窗口**：机制为 `both` 时开机有两个实例同时被拉起，若第二个实例也去「拍肩膀」，用户一开机就会看到界面。
   - **自启项按需重建、不许无条件重建（2026-09）**：`repair_auto_start_on_startup()` 必须**先比对再动手**（Run 值 vs `startup_command_line()`；任务 vs 当前 exe），一致就什么都不做。理由：重建真正要防的只有「exe 被移动/升级」，而无条件重建会让**每次开机**都多付两次 `schtasks.exe` + 一次注册表写入，正好撞在开机 IO 高峰上。
   - **读计划任务必须走 `/xml`**：`schtasks /fo LIST /v` 的字段名（`Task To Run:`、连 `TaskName:`）**会随系统语言本地化**，中文 Windows 上匹配不到；`<Command>` 是固定标签。解析失败要与「没有任务」**分开处理**（前者按旧行为重建，后者什么都不做），否则解析一变就会静默丢掉自愈能力。
2. **设置 · AI 供应商 / 模型**：
   - 供应商预设与模型建议为单一数据源：PROVIDER_PRESETS / MODEL_SUGGESTIONS（pp/src/plugins/builtin/settings.ts），禁止两处重复维护。
   - 供应商 = 大节点：模型下拉只含**当前供应商**的预设模型 + 该供应商已保存的自定义模型；切换供应商时回落其默认模型，不把上一家模型带入。
   - 自定义模型确认 = 在**当前供应商模型列表内新增一项**，预设全部保留；内联编辑器只“追加编辑行”，禁止覆盖 dropdown 整体 innerHTML。
   - 接口地址默认**不带 /v1**；启动 agent 后端时先剥离末尾 /v1 再拼供应商的 /anthropic 路由（`AI_AGENT_URL` 可整体覆盖该端点）。
   - 模型建议名必须与供应商**实际可用名**一致。DeepSeek 的 Anthropic 兼容端点实测可用 `deepseek-v4-pro` / `deepseek-flash` / `deepseek-v4-flash`（2026-09-15 用真实 key 逐个打到 200），臆造名如 `deepseek-v4.1-flash` 会 400 并回报支持列表 —— MODEL_SUGGESTIONS 按此维护，**实测可用的名字一个都不能漏列**（漏列 = 用户存过的那个名字被当成未知模型，见下条）。
   - **面板不得静默改写已保存的模型名（2026-09-15 事故后定案）**：真实事故 —— dev 配置里存的是 `deepseek-v4-flash`，但它没被列进 MODEL_SUGGESTIONS，于是「切一次供应商」就按 `preset.default_model` 回落成 `deepseek-v4-pro`，面板一保存即落盘，表现为「我的模型怎么自己变回 v4-pro 了」。现在的判据只有一条：**旧模型属于别的供应商才回落本供应商默认值**（`belongsToOtherProvider()`），认不出来的名字一律**原样保留**并标成「(自定义)」。改这段逻辑必须保留这条不变量。
   - **兜底模型 = `deepseek-flash`**：DeepSeek 预设的 `default_model`（新建配置/切换供应商后的初值）与宿主 `ai_credentials()` 在 `AI_MODEL` 完全未设时的回退值都用 flash —— 与规则 16「测试一律 flash」保持一致；要更强回答由用户在面板里显式选 `deepseek-v4-pro`。
   - **AI 凭据的唯一真相源是 `<exe 根>\config\ai.json`（2026-09-15 事故后定案，不得回退）**：设置面板保存 → `set_ai_config` **先落盘**再注入进程 env；启动时 `commands::apply_saved_ai_config()` 读回注入，**没有该文件才用 `.env`**（`dotenvy` 在 `main()` 载入）。想回到 `.env` 默认值就删掉该文件。
     - 事故经过：配置曾存前端 `localStorage["lunac-ai-config"]`，启动时由 `main.ts` 回灌 `set_ai_config`。于是 localStorage 里的**旧 key 覆盖**了用户刚改的 `.env`，表现为「key 改了不生效、一直 401」，且生效的是哪一份完全无从判断（排查得去翻 WebView2 的 leveldb）。更糟的是自锁：`get_ai_config` 读的是已被覆盖的 env → 面板 key 框被预填成旧 key → 用户一点保存又把旧 key 写回 localStorage。
     - **禁止再用 localStorage 承载 AI 配置**（它不在 exe 根、违反便携约束，且 dev 与 release 各存一份、互不相同）。旧值只在「后端完全没有 key」时做**一次性迁移**回填，回填后立刻 `removeItem`，绝不覆盖已有配置。
     - **落盘失败必须报错**，不许「面板看着保存成功、重启又变回去」。
   - **凭据类日志必须写「末 4 位 + 来源」（2026-09-15）**：启动与保存时各记一行 `AI 配置来源=config\ai.json | .env … key_tail=d423`，喂给 agent 前再记一行 `agent 凭据: endpoint=… model=… key_tail=…`。三行对照，30 秒定位「现在生效的是哪一把 key」。`log::key_tail()` 是唯一实现（空 → `(empty)`、不足 5 位 → `****`）；**标签必须写成 `key_tail=`**，写成 `key=` / `api_key=` 会被 `mask_secrets` 整体打成 `***`（有单测钉住这个差别）。全程只出现末 4 位，不出现完整凭据。
3. **设置 · 技能扩展**：固定目录 `<exe 根>\skills`（布局 `<技能key>/SKILL.md`）；lunac 负责 raw SKILL.md URL 安装 / 新建粘贴 / 编辑 / 删除；agent.exe 经 `LUNAC_SKILLS_DIR` 读取该目录，与编译内置技能互不影响；key 由 frontmatter.name 安全 slug 派生。
4. **搜索性能**：应用列表（Start Menu + 自定义启动项）**只有一份存储 —— 落盘文件 `<exe 根>\temp\app-index-cache.json`**。Rust 侧**不持有任何进程内缓存**：没有 TTL、没有失效、没有 stale-while-revalidate。搜索路径（`search_apps` → `app_indexer::apps()`）**只读这个文件** —— 不扫目录、不写盘、无状态可失效，所以**任何动作（窗口唤出/隐藏、切进插件、增删自定义启动项）都不会让搜索卡住**；`search_apps` 另标 `#[tauri::command(async)]`（Tauri 同步命令跑在主线程，本命令每击键一次，放工作线程更稳）。全量扫描**只发生在后台刷新路径**（启动后 600ms、Alt+Space 唤出 / 托盘显示时若文件比 30s 更旧），扫完**原子写回**（同目录临时文件 + rename 覆盖 —— 直接 `fs::write` 会让并发读方看到半截 JSON，表现为「结果突然空了」）。**增删自定义启动项时就地重写文件**（系统项沿用文件现状 + custom 项按注册表现算，写完立刻可搜到）—— 旧实现在这里直接删文件，导致下一次搜索退化成全量**同步**扫描，正是「加完启动项后第一次搜索卡一下」的来源。合表时同名条目按来源优先级去重（系统项优先；**不能用 `dedup_by`**：排序键含来源优先级，同名条目不相邻，`dedup_by` 只处理相邻重复会漏）。自定义启动项本身属**业务数据**，存 `<exe 根>\ModuleData\custom\app_registry.json`，与列表文件分开，不会随缓存重建丢失。前端输入 60ms 去抖并丢弃过期输入，**内容检测（latest-wins）**：`search_apps` 晚回包时校验键入序号（`_searchSeq`），过期或期间已进插件态直接丢弃、不触碰 UI——快速键入只显示最终结果，杜绝旧结果覆盖/一次键入多次渲染闪烁；结果渲染不保留入场 / 开合动画。**窗口高度**：搜索路径懒测量（双 rAF 后实测），`setSize` 串行化（latest-wins，在途期间只记最新期望高度，完成后补发一次），杜绝快速键入时逐键 setSize IPC 风暴 / onResized 回环。
   - **窗口高度「滑动」动画（正式，2026-09）**：非插件/搜索态高度变化默认逐帧滑动（rail 模式——每步等上一 setSize 经 onResized 落地再走下一步），默认参数定稿 `rigidity 0.22`（每帧逼近比例，大=刚性/跟手，小=柔滑拖尾）/ `maxStep 14`（单步最大位移 px）/ `stepHz 120`（步频上限）/ `suppressMs 400`（唤出/启动抑制期）；DevTools Console `__lunac_resize_anim`（含 `enabled=false` 即回退原直设路径）可实时调节，`__lunac_resize_anim_stats` 记录步数/耗时。首次高度落位直设防启动滑屏；插件态离散跳变不走动画。**唤出抑制**：热键/托盘唤出（`lunac-window-shown`）后 `suppressMs` 内的高度变化一律直设并在期内顺延（内容分批到达：剪贴板探测 → 加泡泡 → 重跑搜索 → 实测），保证窗口**瞬时完整展开**——否则会看到结果区被物理窗口裁剪、逐帧“撑开”（WebView2 无法渲染超出窗口的内容）。
   - **文件索引（2026-09-15，详细搜索用）沿用同一纪律**：`<exe 根>\temp\file-index-cache.json` 是唯一存储，`search_files` **只读内存索引、永不扫盘**，扫盘只在后台线程 + 原子写。见 §2.1.2 与规则 29。
   - **拖拽：属性 + JS 两条通道**（2026-09-15 修「大界面有时候拖不动」）：Tauri 的 `data-tauri-drag-region` 是**裸属性语义** —— 只有事件目标**就是**带属性的那个元素才触发（`tauri/…/scripts/drag.js` 的 `el === composedPath[0]`），点在子元素上无效。所以标题/顶栏这类「中间被一个 flex:1 子元素占满」的条带**必须**同时挂 JS 拖动（`makeDragHandle(容器)`，它跳过 button），输入框则用 `makeInputDragHandle()`（3px 阈值后才 `startDragging()`，拖动期间关掉输入的 pointer-events）。“三处标题栏都能拖”这句话以前只对搜索栏成立，大界面顶栏实际只有 12px 内边距能拖。
   - **拖拽只认左键 + 搜索栏右键菜单**（2026-09-15）：两个拖动工厂都必须 `if (e.button !== 0) return` —— 右键要弹上下文菜单，若右键也能启动拖动，菜单一弹出窗口就跟着跑。全局 `contextmenu` 早已统一 `preventDefault`（WebView2 默认菜单的「后退/重新加载/检查元素」对桌面工具无意义），故搜索栏原本是「右键没反应」；现在 `attachSearchContextMenu()` 给**简洁搜索栏与详细搜索输入框**共用一套四项菜单（全选 / 复制 / 剪切 / 粘贴），无选区时复制/剪切**置灰**（`.context-menu-item.disabled`，占位不隐藏）。**必须复用 `#context-menu` 那一套 `showContextMenu()`，禁止再造第二套菜单**（多菜单同帧重叠是历史 bug）。粘贴走 Tauri 剪贴板插件（`capabilities` 已授 `clipboard-manager:allow-read-text`），**不用 `navigator.clipboard`**（WebView2 下另需权限、未聚焦时直接 reject）。任何改值操作（剪切/粘贴）都必须补发 `input` 事件：简洁搜索栏靠它同步 Rust 空白态与重跑搜索，详细搜索靠它重跑查询。
5. **通用自定义下拉框**：固定约 4 行（≈120px）可见，更多项内部滚动；滚轮强制内部滚动（`passive:false`）+ `overscroll-behavior:contain`；WebView2 透明窗口禁止原生 `<select>`。
   - **展开下拉时绝不许改滚动容器的 `overflow`（2026-09 事故）**：设置面板的下拉框曾被 `.sel-open` 放宽到 `#results-list` / `#results-container` 上（原意是「别被面板裁剪」）。`#results-list` 是 `overflow-y:auto` 的**滚动容器**，把它改成 `visible` 会让它不再是滚动容器、`scrollTop` 被浏览器归零 —— 于是「AI 分页里一打开靠下的『搜索服务商』下拉，整页瞬间跳回最顶端」。正确做法（现已实施）：`.sel-open` **只**加在 `.settings-layout` / `.settings-content` 这类纯裁剪容器上；下拉框的**可用高度与上下翻转按滚动容器的可视区**（`#results-list` 的 `getBoundingClientRect`）计算并限高（`positionDropdown()`），从而永远落在可视区内、既不被裁剪也不动滚动容器。判断新面板是否踩坑：打开下拉后 `#results-list.scrollTop` 必须保持不变。
6. **搜索引擎预设**：仅 Google / Bing / Baidu（DuckDuckGo 已移除），旧 localStorage 值自动回退 Google。
7. **数据目录**（2026-09 修订）：根 = **exe 安装根目录**（`current_exe` 所在目录；release=安装根、dev=target\debug，dev/release 数据天然隔离）。统一结构：`temp\`（缓存类：WebView2 用户数据 `temp\webview-data`、应用扫描缓存 `temp\app-index-cache.json`、文件索引缓存 `temp\file-index-cache.json`（见规则 29）、落盘日志 `temp\logs`、超长工具输出 `temp\tool-outputs`，见规则 27）、`ModuleData\`（业务数据：`history\` 聊天会话/剪贴板、`memo\` 备忘录含图片与 tag、`custom\app_registry.json` 自定义启动项、`usage\usage-YYYY-MM-DD.jsonl` 用量对账日志）、`skills\`、`tools\`、`config\hotkey.json`、`config\ai.json`（AI 凭据唯一真相源，见规则 2）、`paddle-ocr\`。**所有数据落盘模块统一走 `storage::lunac_root_dir()`，禁止各自硬编码路径**；首次启动将旧 `%LOCALAPPDATA%\Lunac(-dev)` 数据整体迁移到 exe 根并删除（`migrate_legacy_localappdata`）。
8. **备忘录插件**：主界面=编辑；保存后弹“检索标识”对话框完成完整保存；历史记录为独立子界面（预览/复制/编辑/删除，保存逻辑同主界面）；标识会同步为检索索引，搜索栏精确/前缀匹配标识可直达对应备忘录编辑界面；支持**粘贴/拖放图片**（存 `ModuleData\memo\images\<id>\`，条目 `images` 字段），编辑器与历史均缩略图显像。搜索无任何应用/插件匹配（含无关乱码）时结果区常驻 3 项：Web 搜索、AI 助手问答、备忘录录入。
9. **图标风格**：统一线性 SVG（`fill:none; stroke:currentColor`，24 栅格，按钮内 12px，stroke-width 2.2，圆头端点），功能按钮禁止用 emoji；规范见 [icon-style.md](file:///d:/cc/claude-code-cli-master/docs/icon-style.md)。
10. **文件附件省略折叠**（搜索栏 / AI 聊天输入栏共用）：前 **3** 个文件为独立泡泡，第 4 个起收进一个“省略泡泡”分支；点击 ⋯ 展开，展开项仍以泡泡框子分支显示，可单删；省略号内提供一键删除全部（仅作用于省略号内容，不影响前 3 个）；**Backspace 空输入删除同步**该按钮：折叠态下清空省略分支，展开或无分支时删最后一个泡泡。**进入任意插件界面自动隐藏**搜索栏泡泡（AI 聊天除外——其泡泡改在聊天输入栏内展示）；退出插件恢复搜索栏原样。Ctrl+V 粘贴支持文件/纯文本路径/**纯位图**（截图、ShareX 等经 Rust 原生剪贴板兜底存临时文件后成泡泡）。
11. **自定义文件启动（快速启动 / Custom Launch）**：持久注册表为 `<exe 根>\ModuleData\custom\app_registry.json`（业务数据同根统一管理；旧 exe 同目录 / LOCALAPPDATA 文件首读自动迁移）。面板打开即列出**全部已注册项**（可启动 / 逐条删除 / 「添加启动项」），重启不丢失、数据不再“消失”；删除 = 注销注册表 + 摘除对应气泡。拖入/粘贴路径进搜索栏即自动注册；入口词多语言/拼音覆盖（launch/open/启动/qidong/dakai/自定义/快速启动…，中文由 pluginRegistry 自动生成拼音索引）。
12. **卸载清理**：NSIS `installerHooks`（[nsis-hooks.nsh](file:///d:/cc/claude-code-cli-master/app/src-tauri/nsis-hooks.nsh)）`NSIS_HOOK_POSTUNINSTALL` 做**双清理**：① 递归删除 exe 安装根内的运行时数据子目录（ModuleData / temp / skills / tools / config / paddle-ocr）；② 删除旧版本遗留的 `%LOCALAPPDATA%\Lunac(-dev)`，实现干净卸载。
13. **OCR 引擎按需下载**：PaddleOCR-json 引擎不随发行包分发，落到 `<exe 根>\paddle-ocr`（与数据根一致）。前端两条入口复用 `ocr.ts` 导出的 `installOcrEngine()`（监听 `ocr-engine-progress`/`ready`/`error`）：① OCR 面板执行识别前先 `ocr_engine_status()`，缺失则在状态行内联「下载并安装」按钮；② 设置 · 常规面板常驻「OCR 引擎」行（状态 + 下载/重试）。**安装必须原子化**：下载 → 解压到 `temp\paddle-ocr-staging` → 校验 `PaddleOCR-json.exe` + `models/config_chinese.txt` → 才删除并 `rename` 到目标目录，任一环节失败清理半成品，避免 `paddle_ocr_dir()` 定位到残缺目录导致 OCR 永久失败且无从诊断。
14. **Agent 内置工具与审批（P1/P2，2026-09）**：十一件工具全部在 `core-agent/src/tools.rs`，工具名必须保持 **PascalCase**（前端 `main.ts` 对 `"Bash"` / `"PowerShell"` 有专门的命令展示与危险命令分类分支），新增/改名要同步 §3.5 的契约表。写类四件（`Write`/`Edit`/`Bash`/`PowerShell`）**必须先发 `can_use_tool` 等前端回包**，agent 侧不做二次判断（白名单与危险命令分类归前端 `classifyRequest()`）；`plan` 档直接拒绝、`LUNAC_WORKSPACE_LOCKED=1` 拦越界 —— 这两道闸门与审批是**与**关系，任何一道都不得为了「少点一次同意」而放宽。**`WebSearch`、`WebFetch` 与 `AskUserQuestion` 同样必须先审批，且是「`plan` 档不解禁」的例外**（`tools::gated_in_read_only`）：只读档对写类工具免于询问，是因为那些工具反正会被拒（问了白问）；这三件在只读档**放行** —— `WebSearch` 会把查询词发往外部搜索源，`WebFetch` 能把 `Read` 到的文件内容拼进 URL 带出本机，`AskUserQuestion` 的答案只能从卡片上取。**`TodoWrite` 是唯一的常驻免审批工具**（不碰本机、只改前端面板），它的免审批不构成先例：判断新工具是否免问，看的是「执行会不会改变本机或把数据带出」。工具报错必须以 `is_error=true` 的 `tool_result` 回给模型（不中断整轮），只有 HTTP/流错误才回滚 history。
15. **Agent 上下文压缩不变量（2026-09）**：历史一律以 **user 文本消息**开头（不是 `tool_result`），`tool_use` 与对应 `tool_result` 不得被拆散（丢弃点要跳过 `tool_result` 开头的位置）。任何改动 `compact_history()` 的代码都必须同步修正调用方的回滚锚点 `base`（`base -= dropped`），并在压缩后往 `system/context_compacted` 事件里报出计数 —— 这三条是「压缩后仍能继续对话」的充分条件，改动后请用 `LUNAC_MAX_CONTEXT_TOKENS=8000` 的真实端点烟测复验。
16. **测试一律用 flash 模型（2026-09）**：任何真实端点测试（工具往返、权限审批、上下文压缩、MCP 桥等）把 `AI_MODEL` / `LUNAC_AGENT_MODEL` 指向 **`deepseek-flash`**，**不要用 `deepseek-v4-pro`** —— 测试只验证链路、契约与结构，flash 足够且更快更省；只有当问题与回答质量本身相关、或需要复现线上行为时才用 pro。
17. **MCP 工具命名与审批（P3，2026-09）**：接进请求体的用户工具名一律 `mcp__<原名>`，**前缀与清洗规则（非法字符换 `_`、超长截断、重名加 `_2`）不得随意改动** —— 前端审批卡的「始终允许」按完整工具名记 localStorage 白名单，改名等于让用户的白名单失效。MCP 工具**必须**先发 `can_use_tool`（handler 能跑 shell / 发 HTTP），且 plan（只读）档不接入；桥的失败（spawn/握手/超时）只记 stderr，**绝不允许影响十一件内置工具的可用性**。
18. **前缀缓存不变量（2026-09）**：DeepSeek 等端点的自动前缀缓存按「最长公共前缀」命中，**请求体里任何靠前内容逐字节抖动都会让整段缓存失效**。已定稿的稳定化措施，改动时不得回退：① `history` 一律以 user 文本消息开头；② 压缩丢弃点左移 `base` 锚点而不是改历史首条；③ 系统提示词固定、技能清单按 `key` 排序；④ 内置工具名 PascalCase 稳定、MCP 工具数组**按名排序**后再入请求体；⑤ 工具黑名单只裁剪真实存在的工具名（`core-agent` 的内置十一件 + `Skill`），`src-tauri` 侧**不再内置旧 CLI 时代的默认名单** —— 那批名字对自研 agent 全是空转项，且按名精确比较会误伤同名 MCP 工具。判断「改了会不会掉缓存」的方法：把两次请求体开头做 diff，出现任何顺序变化即为回归。
19. **用量口径不变量（2026-09）**：`result.usage` 是**每次提问的绝对值**（agent.exe 每次提问把四个计数器清零再累加本轮的工具往返），**永不改成会话累计** —— 累积是前端/面板的事，agent 侧一旦改成累计，回滚（失败轮 `history.truncate(base)`）就会让计数与上下文不一致。前端**禁止对 `result.usage` 做差**（旧 cli.exe 才是累计值，这条是历史包袱）。**表盘（命中率 / 总 token）口径 = 当前这次对话** —— 新建会话、切到别的会话都归零（用户 2026-09 明确要求；此前是「今日合计」，已改）。按天合计**照旧**落盘在 `ModuleData\usage\usage-YYYY-MM-DD.jsonl`，供与供应商平台逐条对账 —— **两套口径不要混**：日志是「天」，表盘是「对话」。日志字段名 `cacheRead` / `cacheCreate`（驼峰）是日志格式契约，改名会让外部对账脚本读不到。详见 §3.5「用量与对账」。
20. **落盘日志（2026-09）**：release 是 GUI 子系统、没有控制台，`eprintln!` 线上全部丢失，前端也没有 DevTools —— 出问题原本**没有任何东西可查**。现在两个进程各自落盘到 **`<exe 根>\temp\logs\`**（`agent-YYYY-MM-DD.log` / `lunac-YYYY-MM-DD.log`，日期为 **UTC**、跨天自动换文件，启动时清理 7 天前的 `*.log`）：
    - **agent 侧**（[core-agent/src/log.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/log.rs)）：启动/退出、`ready`（工具清单 / 审批档 / 技能与 MCP 数量）、`cfg`（端点与模型，token 只记 set/empty）、**每次工具调用**（`run_tool` 是唯一入口：名称 + 参数摘要 + 成功或 `FAILED` 文案 + 耗时，覆盖内置 / Skill / MCP 三类）、`run_shell` 的**退出码 / 是否超时 / 输出规模 / stderr 原文**、panic。
    - **宿主侧**（[app/src-tauri/src/log.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/log.rs)）：启动与退出、`stop_cli`、**agent 的每一行 stderr 原样落盘**（agent 自身没机会写日志时的兜底）、以及前端经 `log_frontend` 命令上报的未捕获 JS 错误（`window.onerror` / `unhandledrejection`，前端按内容去重后上报）。
    - **开关**：`LUNAC_LOG=off` 关闭；`LUNAC_LOG_LEVEL=error|warn|info|debug`（默认 `info`，`debug` 才记工具输出全文）。`LUNAC_LOG_DIR` 由宿主注入给 agent，保证两个进程写同一目录；agent 独立运行（烟测）时回退 `<agent.exe 目录>\temp\logs`。
    - **不引依赖**：不用 `log` / `env_logger` / `chrono`，UTC 时间戳是手写换算（`civil_from_days`）—— 所以日志时间是 UTC，与本地时间差 8 小时，属已知取舍。
    - **脱敏是硬要求**：任何进日志的字符串都要过 `mask_secrets()`（`sk-` 裸 key、`Bearer <token>`、`api_key` / `search_key` / `token` 等键值对）—— 日志会被用户贴出来求助，凭据不能跟着出门。
    - **禁止写 stdout**：agent 的 stdout 是 stream-json 协议流（宿主只转发以 `{` 开头的行），日志只能走文件，否则会污染协议。
21. **AI 对话面板（参照 Trae 侧栏，2026-09）**：UI/交互的唯一规范是 [agent-ui-spec.md](file:///d:/cc/claude-code-cli-master/docs/agent-ui-spec.md)；本节只固化**不可回退的硬约束**：
    - **视窗不动**：窗口宽度、毛玻璃形态、`#search-bar → #results-container → #status-bar` 纵向结构、AI 态离散高度（360 / 600 / 520）全部不变；改造只发生在 `#results-list` 内部与输入栏内部控件，**不得**新增 `setSize` 或把插件态拉进高度滑动动画（与规则 4 一致）。
    - **不许自称「沙箱」**：Lunac 没有 OS 级隔离（无 `sandbox-exec` / 无 AppContainer），只有**策略级**边界（审批 + 工作区锁 + 危险命令黑名单 + 白名单）。UI 与文档统一叫「**命令运行方式**」（问不问）与「**安全档位 / 文件边界**」（允不允许）——把策略级边界包装成隔离沙箱会让用户在 `full` 档产生「反正有沙箱兜底」的误判。
    - **两道闸门不得因「自动」而消失**：运行方式三档（手动 / 白名单 / 自动）只改**询问频率**；`CMD_BLACKLIST` 命中的危险命令**任何档位都强制人工确认**（含自动档），且永不提供「加入白名单」（与规则 14 的「与关系」一致）。自动档必须二次确认 + 常驻警示，但**提示必须出现在切换点就地**（⋯ 菜单里运行方式按钮旁的简述换红字警示，同时给该按钮与 `#chat-more-btn` 加 `.run-mode-auto`）——**不做**输入栏顶部的全宽警示条，二次确认也内联在同一位置。
    - **单一 IPC 下发点**：运行方式与安全档位两个入口都走 `main.ts` 的 `setSecurityProfile()` → `set_security_profile`（`restart=true` 切换即重启 agent；启动同步用 `restart=false` 保持懒启动）。设置面板只广播 `lunac-security-profile-changed` 事件，禁止再造第二个 invoke 点（否则一次切换会重启两遍 agent）。
    - **展示信息从现有字段推导**：退出码 / 超时来自 `tool_result` 文本解析（`exit code: N` / `(timed out after N ms`），耗时 = `tool_use_id` 配对的前端时间戳，越界拒绝识别 `Access denied: … is outside the workspace`，用户拒绝识别 `User denied this action`。**不得**先私自给 `stream-json` 加字段（要走 §3.5 契约表登记流程）。
    - **折叠与省略**：思考块折叠态**不保留正文 DOM**（展开时惰性填充），超 4000 字只渲染首 2000 + 末 500；回合默认折叠门槛 = 工具调用 ≥2 或过程块 ≥3，开关 `lunac-agent-autofold`（默认 `1`，设置 · AI 面板）。折叠只做局部类名切换，不重排已完成块、不逐帧测量（code-rules §4.2/§4.3）。
    - **文案与图标**：新增文案一律进 `i18n.ts` 五语言 DICT（`agent.*` / `settings.*`），禁止硬编码中文；承担状态语义的图标必须按 [icon-style.md](file:///d:/cc/claude-code-cli-master/docs/icon-style.md) 用内联线性 SVG（emoji 只允许纯装饰）。
    - **一次权限运行 = 一行命令组（2026-09）**：同一次运行内 `Bash` 与 `PowerShell` 视作**同一族**，合并成**一条**未应答行（含 `&&` / `|` / 重定向 / 换行的复杂命令同样入组 —— 按算子排除会让复杂任务里「一行一条」堆满卡片）；合并**只影响审批展示**，执行语义不变。`.approval-item` **不得设固定高度**（旧 `height:144px` + `.approval-body{flex:1}` 会把命令挤在中间、上方留大片空档），命令区高度自适应内容。**命令区不许再有第二层滚动容器**（2026-09-15 修）：`.approval-cmd-box` 曾自带上限 + `overflow-y:auto`，与外层 `.approval-batch-body`（`max-height:380px` + `overflow-y:auto`）叠成嵌套双滚动条 —— 实测两条稍长的命令折行后 `scrollHeight=106 / clientHeight=72`，内层把 34px 内容裁在框外（「已合并 N 条命令」整行不见、末尾折行只剩半行），用户看到的就是「空白区域」还以为是模型输出带了空行。滚动**只由 `.approval-batch-body` 承担**。另：`└ ` 前缀行（第 2 条起）必须挂 `.with-sep`（`padding-left: 2ch; text-indent: -2ch`）—— 前缀是 inline，不挂的话折行续行会回到框左边缘、比正文左移 2ch（实测 12.11px），看起来就是「缩进对不齐」。**排版前先确认模型输出是否真有缩进**：实测拿到的命令是干净单行、`normalizeCmdForDisplay()` 也没改坏，纯粹是上面两条 CSS 造成的观感，别去改 agent 侧或 `normalizeCmdForDisplay()`。「始终允许」必须把**组内每一条**命令的命令词都写进白名单（只写第一条 = 用户反馈的「允许过还要再问」）；合并后标题要重画（工具名可能不止一个，⛔/⚠ 标记可能来自后并入的那条）。**跨轮永远合并不了**：下一轮的命令要等上一轮的执行结果才由模型产生。详见 [agent-ui-spec.md](./agent-ui-spec.md) §3.6。
    - **历史回顾不得省略过程与表盘（2026-09）**：会话记录（**2026-09-17 起存放于 `ModuleData\history\chat.db`（SQLite），旧位置是 `chat-history.json`**）除 `messages` 外还存 `usage`（表盘口径的 token 快照）与 `steps`（按回合分组的过程快照）；恢复历史时把表盘数值写回仪表盘、把过程渲染成可折叠的「过程 · N 步」块。**不是 `localStorage`**。详见 [agent-ui-spec.md](./agent-ui-spec.md) §3.7。
    - **滚动条统一（不得回退）**：项目内**所有**可滚动容器共用 `styles.css` 里**唯一**的全局 `::-webkit-scrollbar` 规则（4px / 轨道透明 / 滑块 `rgba(255,255,255,0.18)` / hover 0.32）。**禁止写 `scrollbar-width` / `scrollbar-color`** —— Chromium（WebView2）看到非 `auto` 值会让 `::-webkit-scrollbar` 整段失效、退回系统默认外观（历史事故：`#chat-input` / `.tool-result` 就是这么变成「WebView2 默认滚动条」的）；**禁止按容器单独声明**滚动条样式（新增容器必漏）。详见 [agent-ui-spec.md](./agent-ui-spec.md) §5.4。
22. **系统提示词环境块（2026-09）**：agent 的系统提示词 = `SYSTEM_PROMPT`（身份 / 工具使用） + `PERSONA_AND_STYLE`（固定的「人格 + 文风」，2026-09-17 方案 B 从用户消息搬来） + `env_block(cwd)` + `skills::listing()`，四段拼接且**只构建一次**，见 [core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs) 的 `env_block()`。环境块必须写明：宿主是 **Lunac**（不是任何其它 agent 框架的一部分）、**工作目录的绝对路径**、**Lunac 自己的技能目录**（`LUNAC_SKILLS_DIR`），并**显式禁止用磁盘上的文件反推宿主**。
    - **为什么必须写**（真实案例）：默认工作区是用户主目录，而用户主目录里可能躺着**别的 agent 框架**的目录（实测：`~/.hermes/skills`）。模型回答「我自己的 skills 在哪」时只能从文件系统反推 —— Glob 到那些目录后，它把宿主认成了那个框架，整个思考过程都锁死在那里（只是「你是 Lunac 的助手」这一句并不够）。
    - **不得回退**：这段话是身份纠偏的唯一来源，删掉就会退回「模型自己猜宿主」。内容在一次会话内必须**逐字节不变**（cwd 与技能目录在 agent 进程生命周期内都是常量），否则违反规则 18 的前缀缓存不变量。
23. **缓存命中率的解释口径（2026-09）**：命中率**有自然下限**，不能拿 100% 当目标 —— 每轮新增的 user 提问 / assistant 输出 / `tool_result` 都是新内容，天然不被上一轮缓存覆盖，命中上限 ≈ 上一轮长度 ÷ 本轮长度；工具往返多、`tool_result` 大时必然偏低。**因此必须把「自然未命中」与「断裂失效」分开统计**，只有后者才是回归。
    - **全链路审计结论**：`system`（含 `env_block`）、`tools`（按名排序）、`history` 的四个追加点都是 append-only 且进程内恒定，**不是**命中率低的来源。
    - **断裂源按影响排序**（都在 `compact_history()` 及其调用点，[core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs)）：① `drop`（>95% 水位才丢中段）> ② `elide`（>85% 水位 + 滞回）> ③ 首条插入 `TRIMMED_MARKER` > ④ 失败回滚 `history.truncate(base)` 与压缩叠加 > ⑤ 任务快照钉回（仅当本轮真的 `dropped > 0`，见规则 37 —— 属于 ① 的附带项，不单独增加损失）> ⑥ 摘要压缩钉回（仅当本轮真的 `dropped > 0` 且过了 `SUMMARY_MIN_INPUT_CHARS`，见规则 39 —— 同属 ① 的附带项）。任何改动这几处的代码都要意识到「这是在主动放弃整段前缀缓存」。
    - **已实施的减损措施（不得回退）**：水位从 0.70/0.90 抬到 **0.85/0.95**；瘦身档加**滞回**（`Cfg.last_compact`：一次压缩后要再长 ≥ 预算 ×0.15 才允许动第二次）；`Compact` 三档化，**瘦身档永不丢整条消息**（原先「无可瘦身内容就直接丢」会让 0.85 水位也丢整段，等于白废一次缓存）。丢弃档不受滞回约束 —— 到 0.95 不压就可能 400，安全优先。
    - **瘦身档的「值不值得」闸门（2026-09 新增，不得回退）**：瘦身是**就地改写较早的消息**，端点侧从被改的那条起就再也匹配不上已落盘的缓存前缀单元 ⇒ **省下的体积必须明显大于被作废的后缀**。因此只有「可省字符数 ≥ 当前上下文 token × `ELIDE_MIN_SAVINGS_RATIO`(0.05) × 4」才允许瘦身；不够就**什么都不做**（打一行 `跳过瘦身：可省 X 字 < 阈值 Y 字` 到 stderr，便于归因）。**闸门只作用于瘦身档**（可选档）；`Drop` / `Force` 是安全刚需，照旧无条件压。配套：`compact_history()` 返回 `CompactOutcome{elided, dropped, dropped_msgs, pinned}`，「扫了一圈但决定不动」时**不推进 `last_compact` 滞回时钟**（否则会白等一个 15% 增长窗口，且压缩计数被污染）。
    - **判断「命中率是否真的偏低」要先对账**：按 §9.1 难点 1 的做法，用 `requests[]` 与平台逐行对齐，并拿单轮数据去比「上一轮上下文 ÷（上一轮 + 本轮新增）」这个上限公式。2026-09-15 实测：dev 实例真实记录合计命中率 **77.4%**，单轮恰好贴着上限（如 `in=4612 / read=15744` → 77.3%）⇒ **低位来自会话短，不是缺陷**；不要为了「向 dsh 的 97–99% 看齐」去改结构。
    - **思考开关不直接进前缀**：thinking 只进 `display`、**不进 history**（回灌会 400）；其 400 降级只改 `thinking` 形态与 `max_tokens` 两个生成参数（`Thinking` / `max_tokens_for`），是否掉缓存取决于端点侧 hash 口径（本仓库无法自证）。但**切思考开关会重启 agent ⇒ history 清空 ⇒ 缓存必然重建**，这是「切换后命中率骤降」的合理解释，属预期行为。
    - **思考只有开 / 关两态（2026-09-15，不得回退）**：端点**没有**思考力度旋钮（`budget_tokens` 不被 enforce、`effort` 字段被静默忽略，实测见 §3.5），因此**禁止**再把档位做成「快速 / 思考 / 深度」这类深度分级、也禁止把 `budget_tokens` 暴露给用户 —— 那是在承诺端点做不到的事。开关值只有 `on` / `off`（`LUNAC_THINKING`），`budget_tokens` 退化为单一常量 `THINKING_BUDGET`。
    - **注入提示的「固定 / 条件」位置纪律（2026-09-17，方案 B，不得回退）**：给模型的行为约束按「是否随请求变化」分成两类，**落点不同**：
      - **固定块**（人格 / 文风 —— 每次都要生效、内容与请求无关）**必须放进 agent 的系统提示词**（`core-agent/src/main.rs` 的 `PERSONA_AND_STYLE`，与 `SYSTEM_PROMPT` 拼接）。它是固定前缀的一部分 ⇒ 永远命中缓存。
      - **条件块**（`## Debugging Methodology` / `## TDD Requirement` / `## Code Review Pipeline` —— 按 query 关键词命中）**只能留在用户消息里**，因为随 query 变；搬进系统提示词会让提示词每轮都变、把整个固定前缀打掉（比不搬更糟）。
      - **为什么这条是硬约束**：原本两块固定文案（1153 字符 ≈ 288 token）由前端 `buildSystemPromptHint()` 拼在**每条用户消息最前面**，位置决定了它**每次提问都必然未命中**（新用户消息天生不在上一轮缓存前缀里）。实测纯问答类提问的首请求未命中量 `in = 236 / 289 / 313` token 与它几乎相等 ⇒ **首请求未命中的约 90% 就是这两块**。搬进系统提示词后它们进入固定前缀，从此零未命中。
      - 守门测试：`core-agent` 的 `system_prompt_is_stable_and_carries_persona`（同一 cwd 下逐字节可复现 + 两块文案确实在提示词里）。**前端 `buildSystemPromptHint()` 返回空串是合法状态**（不含关键词的提问就是空），`wrappedQuery` 与 `cleanUserContent()` 都按「没有 `\n\n---\n\n` 分隔符」处理。
    - **前缀指纹埋点（2026-09-17 新增，不得精简掉）**：agent 在**每次** API 请求发送之前落一行 `请求前缀 #n system=<hash>/N字 tools=<hash>/N字 history=<hash>/N条/N字`（`log::hash64` = FNV-1a；**只记哈希不记原文**，前缀里可能含用户文件内容，哈希天然满足脱敏硬要求）。**为什么必须有**：同一个会话内跨提问时 `read` 会莫名回落（实测 `usage-2026-09-16.jsonl`：记录 6 末次 `read=3712` → 记录 7 首次 `read=2304`，丢 1408；记录 8 → 9 丢 3584），而**静态读用量日志无法区分**「本侧前缀被改写」与「端点侧淘汰了已落盘单元」—— 两条曲线的形状完全一样，再怎么对着 `requests[]` 看也判不出来。判据：三块指纹与上一轮逐字节相同而 `read` 掉了 ⇒ 端点侧淘汰，本侧无责；某一块的指纹变了 ⇒ 本侧改的，直接去那块找原因（`system` = 身份/环境块/技能清单，`tools` = 工具 schema，`history` = 历史）。
24. **Agent 能力定位与本地扩展目录（2026-09）**：
    - **定位**：Lunac 的 agent 目标是**一个可以完全类比于完整 agent 类应用**的能力体。**不得**以「宿主是桌面启动器」为由把某项能力判定为「用不上」—— 缺口只做**优先级排序**，不做**价值否定**（旧文档把多代理协作等写成「与 Lunac 无关」是错的，已改）。
    - **文档分工**：[agent-implementation.md](./agent-implementation.md) = 实现全景（已实现能力矩阵 / 工具清单全表 / 本地 `skills\` 与 `tools\` 扩展格式）；[agent-feature-backlog.md](./agent-feature-backlog.md) = 待办与实施顺序。两者不互相搬运重复内容。
    - **发布包必须预置 `skills\` 与 `tools\` 模板**：源文件在仓库 `agent-templates/`，由 [build-release.ps1](file:///d:/cc/claude-code-cli-master/build-release.ps1) 拷进暂存目录、由 [lunac-installer.nsi](file:///d:/cc/claude-code-cli-master/scripts/lunac-installer.nsi) 打进 `$INSTDIR\skills` / `$INSTDIR\tools`。模板**必须是不可加载的形态**（`README.md` + `*.example` 后缀）—— `skills\` 下任何含 `SKILL.md` 的子目录都会被列进系统提示词、`tools\` 下任何 `.json` 都会被当工具加载并进入请求体前缀；放真实文件会污染模型的工具清单并破坏前缀缓存不变量（规则 18）。
25. **瞬时失败重试（2026-09）**：agent 对「重试有意义」的瞬时失败做**请求级**退避重试 —— 只在**尚未读到响应体之前**重试，**永不产生重复内容**，因此**不是**「重新生成回答」。
    - **范围收窄**：只有 ① 网络层（连接失败 / 重置 / 超时，`reqwest` 的 `is_builder()` 错误除外）与 ② 429 / 5xx（含 529「过载」）才重试。**4xx 一概不重试** —— 但 400 的两个专门分支保留：thinking 相关走思考降级链、上下文超限走强制压缩，二者各有各的「换形态再试」语义，不得并入本重试。
    - **次数与退避**：`MAX_API_RETRIES = 3`，1s → 2s → 4s 指数增长 + 0..250ms 抖动（不引 `rand`，见规则 20），30s 封顶；端点给的 `Retry-After`（秒）优先。
    - **必须上报**：每次退避发 `system/api_retry`（`attempt` / `max_retries` / `error_status` / `delay_ms`）并落盘一条 `warn`（过 `mask_secrets`）—— 否则界面在退避期间毫无反馈，用户会以为卡死。
    - **SSE 流中途断开不重试**：部分内容已经流给前端，重来会重复；只记 `warn`，本轮回复可能不完整。
    - 实现在 [core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs) 的 `retryable_status()` / `retry_delay_ms()` / `emit_api_retry()` 与请求循环，纯函数有单测（`cargo test -p agent`）。
26. **命令静态安全分析（2026-09）**：危险命令判定**必须在 agent（执行侧）做**，不能只靠前端正则 —— 前端只拿到命令字符串，正则挡不住引号拼接（`r""m -rf /`）、包装器（`cmd /c "…"` / `powershell -Command "…"` / `bash -c '…'`）、变量（`%TMP%\x.bat`）与串联/管道的后半段。实现在 [core-agent/src/bash_safety.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/bash_safety.rs)，结果作为 `can_use_tool` 的 `analysis` 字段上报（契约见 §3.5「命令静态安全分析」）。
    - **只上报、不代替决策**：agent **不得**因自己判为危险就拒绝执行 —— 「自动放行 / 弹审批」永远是前端的唯一决策点（规则 21），否则用户点了「允许」也放行不了。判定是**增强**前端判定，不是替换。
    - **fail-closed 是硬要求**：判不出来（命令词是变量、命令替换 `$(…)`、cmd 的 `%VAR%`、编码执行、间接执行器、包装器嵌套过深、控制字符）⇒ `opaque` 非空 ⇒ **不得自动放行**。宁可多弹一次卡片，也不能把「看不懂的命令」当安全放过去。
    - **`opaque` 判据 2026-09 收窄到「不知道要跑哪个程序」**：**参数里的变量不算**（`Get-ChildItem $HOME`、`$f = "…"; Get-Item $f` 都是普通命令）。理由有两条 —— ① 参数展开的结果**不会被重新解析成命令**（不像 cmd 的 `%VAR%`，它的值里的 `&` / `|` 会变成新命令）；② 旧口径下 PowerShell 里几乎每条带 `$` 的命令都命中，**自动档位形同失效**（用户反馈的「自动运行开着还每条都弹卡」就是这个）。配套改动：`command_word()` 跳过环境赋值前缀（`FOO=bar rm -rf /` 的命令词是 `rm`，否则整条危险规则失效），`detect_opaque()` 对赋值子命令直接返回。
    - **危险判定不许被绕过**：`dangerous` 非空时，**任何运行方式档位**（含「自动」）都必须人工确认，且**不提供「始终允许」**；与规则 14 的两道闸门是「与」关系。
    - **白名单必须按命令词设限**：白名单是**前缀匹配**，`powershell` / `cmd` / `bash` / `python` / `node` / `npx` / `iex` / `env` 这类解释器与启动器前缀**永不入白名单**（否则「允许过一次 `powershell -Command A`」= 以后任何 `powershell …` 自动放行）；历史遗留的这类白名单条目在前端也要拒绝生效。
    - **单引号是字面量、双引号会展开**（bash / PowerShell / cmd 三者一致）：危险判定走「抹掉单引号内容」的形态（`echo 'shutdown'` 不误报），包装器递归走「保留单引号内容」的形态（`bash -c 'shutdown'` 不能漏判）。
    - **转义判定要认 Windows 路径**：`\` 只在后随**非字母数字**时才是转义（`C:\Windows` 里的 `\W` 是分隔符）—— 一律当转义会把路径揉成 `C:Windows`，危险规则再也匹配不上。
    - 改动本模块后必须跑 `cargo test`（含 `catches_quote_splicing` / `catches_danger_in_later_subcommand` / `catches_wrapped_commands` / `single_quoted_literals_are_not_dangerous` / `unresolved_indirection_is_opaque` 等），并确认 `npx tsc --noEmit` 与前端审批卡渲染正常。
27. **单条工具输出预算（2026-09-15）**：单条工具结果**超过 12000 字符就落盘全文**（`temp\tool-outputs\{毫秒}-{工具名}.txt`），上下文里只内联「头 8000 + 尾 2000 + 行数 + 落盘路径」，并提示模型用 `Read`（带 `offset`/`limit`）或 `Grep` 取回。契约见 §3.5「单条工具输出预算」。
    - **唯一出口**：只在 `run_tool` 调 `tools::apply_budget(name, body)` —— 只有那一层同时拿到工具名与未经裁剪的完整输出。**各工具内部一律不得自行截断**（旧实现 7 处 `truncate()` 已删）：那样会在预算之前就把内容丢掉，落盘也就无从谈起。
    - **不许退回硬截断**：模型看不到的内容等于不存在。尾部必须保留（报错结论压在末尾），落盘失败时也只能如实标注「已丢弃」，不许静默截断。
    - **落盘目录必须并入 `Ctx.add_dirs`**（`tools::prepare_output_dir()` 在启动时做）：默认 project 档的工作区锁会拦工作区外路径，不并入等于模型读不回自己的输出。
    - **落盘正文按 1.5MB 字节封顶**（切在字符边界）：必须低于 Read / Grep 的 `MAX_TEXT_BYTES`(2MB) 门槛，否则模型两个工具都打不开（Read 拒绝、Grep 跳过）。被砍时预览里必须如实写「only the first N chars」。
    - **保留 7 天**（启动时清 `*.txt`，与规则 20 的落盘日志同口径）；**不新增环境变量**（目录取 `log::log_dir()` 的父目录）。每次落盘往日志记一行「超预算（N 字符 / M 行）→ 路径」，否则线上无法判断「模型为什么没看到完整输出」。
28. **只读工具并行（2026-09-15）**：一轮里的多条工具调用，**连续的只读调用**合成一批并行（上限 `TOOL_PARALLELISM = 4`），其余串行。契约见 §3.5「只读工具并行」。
    - **白名单只有 7 个**（`tools::parallel_safe`）：`Read` / `Glob` / `Grep` / `WebSearch` / `WebFetch` / `Skill` / `TodoWrite`。**新增工具默认串行** —— 要进白名单必须先自证「只读、不落盘、无全局状态」。
    - **只读批绝不允许跨越写类调用**（「写 A → 读 A」被重排就是错得无声无息）。单元素只读段不标并行。
    - **回灌顺序恒等于 `tool_use` 原顺序**：结果按下标回填，禁止「谁先跑完谁先回灌」。同样地，**审批必须在并行之前**按原顺序解完（否则会打乱前端按「未应答行」合并同一批命令的结果）。
    - **MCP 与命令工具永不并行**：`&mut Bridge` 无法跨线程共享（也不该并发），命令工具带副作用。
    - 改动本模块后必须跑 `cargo test`（含 `batches_never_span_a_writing_call` / `single_read_only_call_is_not_marked_parallel`），并用真机烟测确认日志里出现「只读工具并行批」且 `tool_result` 顺序未变。
29. **详细搜索大界面（2026-09-15）**：双击搜索栏进入的大界面（类 Win+S）。契约见 §2.1.2。
    - **简洁搜索的行为一律不许丢**：进大界面只是叠一层视图（`#app.detail-mode` 上隐藏搜索栏与结果区），退出时把查询词带回搜索栏并重跑简洁搜索；**不得**在大界面里私自改搜索栏/结果区的既有语义。
    - **窗口档 = 640 × zoom 固定高，且不进滑动动画**（离散切换，与插件态同策略）。宽度不变。
    - **切分类不重查**（只筛 `detailAllRows`）；**只有文件类型筛选走后端重查**。序号 `detailSeq` latest-wins，与简洁搜索同纪律。
    - **文件索引只有一条纪律**：唯一存储 `<exe 根>\temp\file-index-cache.json`，**搜索路径只读内存索引、永不扫盘**；扫盘只在后台线程（启动 600ms 后 / 手动重建），写回原子。扫描范围/上限/存储形态的理由见 §2.1.2（拿 19.6 万条的真实数据定的口径）。
    - **存储形态不许退回「每条一份完整路径」**：紧凑形态（目录表 + 文件名）是实测定的（20 万条路径版 47.8MB vs 30 万条紧凑版 30.8MB）；匹配算法必须**零分配**（`eq_ci`/`starts_with_ci`/`find_ci`），不许在每键击里对全量条目做 `to_lowercase()`。
    - **截断必须如实告知**：达到 `MAX_ENTRIES` 时 `truncated=true` 并原样显示「索引不完整」，禁止静默丢掉一部分盘。
    - **设置页与系统动作只走白名单**：`open_setting` 只收 `ms-settings:` 前缀、`run_system_action` 只认目录里的 id（**绝不接受前端传来的命令行/任意 URI**）。新增危险动作必须在 `system_catalog` 标 `danger: true`（前端据此二次确认）。改动这两个模块后必须跑 `cargo test`（含 `arbitrary_targets_are_rejected` / `only_shutdown_and_restart_are_dangerous`）。
    - **Esc 的隐藏判据是「界面层」而不是「内容」**（2026-09-15 修）：Rust `hotkey.rs` 只用**一个** `UI_MODE` 枚举（`main`/`plugin`/`detail`，前端 `set_ui_mode` 同步），`UI_MODE != main` 时无条件 emit `lunac-esc-clear` 交给前端退层。**禁止新增「每界面一个 AtomicBool + Esc 加一条 else if」** —— 新增界面应当只加一个枚举值 + 前端 `handleEscClear()` 一支。禁止把判据改回「query 空 + chips 空」：详情态的查询词在自己的输入框里，按内容判空会一下 Esc 隐藏整个窗口（用户报的 bug）。Esc 的层级顺序 = 抽屉 → 详情大界面 → 插件 → 泡泡 → 文本 → 隐藏窗口。
    - **界面层必须有「唯一出口 + 自愈点」（2026-09-15 追加）**：前端只通过 `syncUiMode()` 上报层（由 `pluginActive` / `detailOpen` 推导），它挂在 `applyWindowSize()` 末尾做汇聚，并在**模块初始化**与**每次 `lunac-window-shown`** 无条件重报 —— 这三处保证「漏一处调用」与「WebView 重载导致前端归零、Rust 留旧值」都能自愈。真实故障：前端在简洁界面而 Rust 停在 `detail`，Esc 只 emit clear、**永远不隐藏窗口**，控制台一直打 `Esc(poll): ui_mode=2 (non-main)`。**禁止给 `syncUiMode()` 加去重缓存**（要求所有发送点共用缓存，别处的裸 `invoke("set_ui_mode")` 会让它脱节并吞掉一次必要同步）。
30. **会话历史的回灌与存储（2026-09-17）**：agent 的对话上下文**完全自持在 agent 进程内**（stream-json 的 stdin/stdout），前端 `chatHistory` 只用于显示 —— 这两份状态之间必须有一条显式的桥，否则「回退 / 恢复历史」之后 agent 眼里就是零上文。
    - **禁止再用 `stop_cli` + `start_cli` 表达「丢弃被回退的消息」**：那会把**保留下来的上文一起清空**（用户报的「回退后引用不到上文」就是这条）。回退改为 `set_history(chatHistory.slice(0, idx + 1))`，把保留下来的历史整体灌回 agent；**只在仍在流式中**才先停一次进程（否则这一轮跑完会把已丢弃的内容写回历史、还白烧 token）。
    - **必须走挂起队列**（`queueAgentHistory` / `flushPendingAgentHistory`）：`cliReady` 由 `cli-status:stdout` 置位，而回退 / 恢复都可能发生在 agent 没起来或刚重启完的瞬间 —— 直接发会被「CLI 未就绪」吞掉。冲刷必须排在**放行挂起提问之前**（agent 单线程顺序吃 stdin，反了这一问仍是零上文）。
    - **`set_history` 入参只认 `user` / `assistant` 纯文本，且必须合并连续同角色**（`normalize_history`）：工具调用细节本来就不落前端（有意近似），而 Anthropic 形态的 `messages` 要求 role 交替 —— 回退到一条用户消息后紧接着的新提问会构成连续两条 `user`，不合并就是端点 400。
    - **历史存储 = SQLite（`<exe 根>\ModuleData\history\chat.db`），不再是 `chat-history.json`**（backlog §8.5 第一步）：单文件 JSON 的全量重写随历史变长而变差，且做不了检索。**库文件存在即唯一真相源**，旧 JSON 只在库不存在时被导入一次（迁移后**保留**旧文件不删，否则用户删掉的会话会「复活」）。实现在 [chat_db.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/chat_db.rs)。
    - **FTS5 索引随写入用触发器维护**，不许改成「先写数据、以后再补索引」（回填极易漏）：两张表 —— 默认 unicode61（英文 / 代码词）+ `tokenize='trigram'`（**CJK 子串检索唯一可行的一条**，默认分词器对中文不切词，`MATCH` 永远命中 0）。`rusqlite` 用 `bundled` 且必须**实测** FTS5 可用（`chat_db::tests::fts5_is_compiled_in` —— 构建配置问题不实测就只能在线上炸）。
    - **`sessions.pos` / `messages.idx` 两列不许省**：旧 JSON 是数组、顺序即语义，而 SQLite 没有隐式顺序，靠插入序或主键序会**静默改变历史列表排列**。
31. **唤出（热键）路径的顺序不变量（2026-09-17）**：`toggle_window()` 里 **`force_foreground()` 必须是最后一步**，顺序固定为 ① `emit("lunac-window-shown")` + 剪贴板读取排队 → ② `refresh_if_stale()` → ③ `force_foreground()` → ④ `LAST_TOGGLE_TICK`。
    - **理由**：`AttachThreadInput` 的等待时间不可控（它要接前台线程的输入队列）。排在 emit 之前时，前端要等激活做完才收到 `lunac-window-shown`，而该事件正是「唤出后主动重跑当前查询」的唯一入口 —— 高负载下用户看到的就是「呼出后卡在上次搜索结果」。`LAST_TOGGLE_TICK` 仍必须在 `force_foreground` **之后**写（写早了会被前台守卫当成「冷却已过」而自动隐藏）。
    - **前端必须在 `lunac-window-shown` 里主动重跑当前查询**（`refreshSearchResults()`，限非插件 / 非抽屉态）：Rust 的唤出路径**不会**重跑搜索 —— `hide_window()` 只有一行 `ShowWindow(SW_HIDE)`（不清结果、不发事件），唯一的重算链是「剪贴板事件 → 合成 input → 60ms 去抖」，而剪贴板没变化时整段不执行 ⇒ 静态帧一直停在上次结果。复用 `refreshSearchResults()` 而不是直接调 `runSearchNow()`：走同一套去抖 + 序号校验，不会与正在输入的字抢渲染。
    - **剪贴板读取命令一律 `#[tauri::command(async)]`**（`read_clipboard_files` / `read_clipboard_backup_image`）：同步命令跑在 Tauri 主线程上，而它们恰好在唤出的那一刻被调用 —— 一张 4K 截图展开成 BMP 就足以拖住主线程。两者都是 `OpenClipboard(0)` 开头的纯 Win32 FFI + 文件 IO，**无线程亲和性**，放工作线程安全。
    - **已知未覆盖（单独决策）**：`show_and_focus()`（托盘 / 菜单 / 单实例）既**不发** `lunac-window-shown` 也不读剪贴板 ⇒ 从那条路唤出时静态帧问题依旧，且前端不会回到简洁搜索（与热键行为不一致）。改它等于顺带改行为契约，未一并处理。
32. **被改动文件的路径追踪（2026-09-17，backlog §8.1）**：会话里被 `Write` / `Edit` 动过的文件必须**可点击定位**，并在对话流末尾给出「本次会话改动过的文件」列表。
    - **路径只能取自 `tool_use` 的入参**（`Write` / `Edit` 的 `file_path`，见 `main.ts` 的 `WRITE_TOOLS` / `changedFilePathFromArgs()`），**禁止从工具输出正文里正则猜**：输出正文里的路径包含「只是读过的文件」，猜出来的列表会混进一堆没改过的项（`Read` / `Grep` 的输出里全是路径）。该做法是**零协议改动**（入参本来就在 `stream-json` 里）。
    - **`reveal_in_explorer(path)` 只收绝对路径 + 存在性校验**（[commands.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/commands.rs)）：非空 / `is_absolute()` / `exists()` 三条全过才执行，**绝不接受任意命令行字符串** —— 与规则 29（`open_setting` / `run_system_action` 白名单）同一条纪律。落点是 `explorer.exe` 的**单个参数** `/select,<path>`（打开所在文件夹并选中该文件），**全程不经 shell** ⇒ 无注入面。去掉尾部分隔符时必须保留长度 ≤ 3 的**盘符根**（`C:\` 去掉就变成 `C:`）。
    - **列表内容必须与界面留下的历史一致**：`SessionStep` 带可选 `path`（走既有的 `steps` JSON 列，**不必改表**），`restoreSession()` / `rollbackChat()` 用 `rebuildChangedFilesFromSteps()` 重建、新对话清空 —— 否则回退历史后列表会留着已经不在上下文里的文件。
    - **点击必须走 document 级事件委托**：工具卡与面板都是**动态重绘**的（面板每次重画、工具卡随流式增量重写），逐个 `addEventListener` 会在重绘后全部失效。
    - **只定位、不打开**：`/select,` 的语义是「定位并选中」，不是「用默认程序打开」；工具卡上的路径也**不写回 agent 上下文**（纯前端展示）。
33. **`pre-wrap` 继承会把 HTML 模板的缩进渲染成「幽灵空行」（2026-09-17 定位，不得回退）**：用户报「权限请求卡里 PowerShell 命令出现缩进和大片空白」，前一轮改过 `.approval-item` 固定高度与 `.approval-cmd-box` 内层滚动（那两个确实也是真问题），但**症状依旧** —— 因为根因与高度、滚动都无关。
    - **真因**：`#chat-log`（= `.ai-response`）自己声明了 `white-space: pre-wrap`；审批卡的中间容器（`.approval-body` / `.approval-cmd-box` / `.approval-cmd-list`）**都没声明**，于是**继承** `pre-wrap`。而这三处 DOM 是 `main.ts` 里 `innerHTML = \`…\`` 拼的**带缩进换行的模板**（`card.innerHTML` / `item.innerHTML` / `renderCmdGroupBody()` 的 `bodyEl.innerHTML`）—— 模板里的空白文本节点（`"\n      "`）在 `pre-wrap` 下不折叠，被当成**真实空行**渲染。
    - **实测数字（不得凭感觉调）**：一处空白节点 = 2 个 19px 行盒 ≈ **45px**；命令区深色块总高 **471px**，其中 **315px（67%）是幽灵空行**，命令文本只占 **129px**。修后 471 → **156px（正好 −315px）**，命令文本仍 129px 一字未变、`white-space` 仍为 `pre-wrap`、外层滚动条消失（内容 213px < `max-height: 380px`）。
    - **修法**：`.approval-card { white-space: normal; }` —— 声明在**卡片根**，一次覆盖三个模板所在的层（逐个声明必漏）。**不得删**，删掉立刻退回 315px 幽灵空行。
    - **通用纪律（这才是根治）**：**往 `#chat-log` / `.agent-flow`（继承 `pre-wrap`）里拼 `innerHTML` 的模板不得带缩进换行** —— 要么把模板写成一行（用 `+` 拼接，如 `.todo-panel` / `.changed-files-card` 那样），要么给该结构容器显式声明 `white-space: normal`。真正需要保留空白的都是**叶子**且**已各自声明** `pre-wrap`（`.agent-text` / `.tool-out` / `.sys-note-body` / `.think-content` / `.approval-cmd` / `.approval-input`），它们覆盖继承、不受影响。
    - **验收口径**：命令区高度必须能被逐项对账（文本高度 + padding + 附属行），**没有余数**；或直接查卡片内空白文本节点的 `Range.getClientRects().length === 0`。
    - **已知取舍**：第 2 条起命令带 `└ ` 前缀 + 悬挂缩进（`padding-left: 2ch; text-indent: -2ch`），其**正文**（含折行续行）比第 1 条整体右移 2ch —— 这是「续行与 `└ ` 后正文对齐」的有意设计，与上述幽灵空行无关。
34. **agent 的默认工作目录 = `<exe 根>\temp\transStorage`（2026-09-17 改，不得回退用户主目录）**：用户报「AI 改动的文件全落在 `C:\Users\15242` 根下」（`_tmp_dump_docx.py` / `_tmp_fmt_docx.py` / `_tmp_test_docx.py` / `_tmp_imgs.py` 四个）。根因是未配置工作区时 `start_cli` 把 agent 的 cwd 回退成**用户主目录**，而 agent 的相对路径以 cwd 为基准 ⇒ 模型写的临时脚本直接堆在主目录根下。
    - **新默认**（`commands.rs` 的 `default_work_dir()`）：`storage::lunac_root_dir()/temp/transStorage`。`lunac_root_dir()` = **exe 所在目录** ⇒ release 落安装目录（`D:\Lunac\temp\transStorage`）、dev 落 `target\debug\temp\transStorage`，与 `temp\logs` / `temp\tool-outputs` / `temp\webview-data` 同级，**卸载时随目录一起清掉**。该目录由程序在**启动 agent 前自动创建**，用户不需要手工建。
    - **必须在这里 `create_dir_all`**：`spawn_child()` 用 `Command::current_dir(cwd)`，目录不存在 ⇒ spawn 直接失败 ⇒ agent 起不来、整个 AI 面板不可用。所以不能「用到再建」。
    - **建不出来（权限等）时回退用户主目录**并记 `warn`：宁可文件仍写到主目录，也不能让 AI 起不来。
    - **权限语义不变**：workspace 为空 ⇒ **仍然不设** `LUNAC_WORKSPACE_LOCKED` ⇒ 工作目录之外的文件照旧可读可写、越界走 ask/审批卡（**不是拒绝**）。只有**用户显式选了工作区**才进锁定模式。
    - **副作用（已知、可接受）**：`env_block(cwd)` 里的工作目录随之改变 ⇒ 模型知道的 cwd 从用户主目录变成应用数据目录。它是 agent 进程生命周期内的常量，不违反规则 18 的前缀缓存不变量（但每次 agent 重启会换一次前缀）。
35. **改完 `core-agent` 必须重新部署 `agent.exe` —— 查找顺序里 dev 布局必须优先（2026-09-17 修，不得回退）**：`commands.rs` 的 `core_dir()` 现在把 **dev 布局**排在最前（见 §3.5 的优先级列表第 0 条）。
    - **为什么必须排最前**：`app\src-tauri\target\{debug,release}\agent.exe` 是 Tauri 构建时按 `tauri.conf.json` 的 `bundle.resources`（`../../core-agent/target/release/agent.exe` → `agent.exe`，map 形式）从 core-agent **平铺过来的快照**，**只在构建 lunac 时**才刷新。而它会命中「agent.exe 与 lunac.exe 同级」那一条，于是**遮蔽** core-agent 的更新构建。
    - **失效路径（实测踩到）**：改 core-agent 源码 → `cargo build --release` → **只重启 lunac.exe**（没重新构建 lunac ⇒ 资源不重新平铺）⇒ 跑起来的仍是**旧 agent.exe**，且**毫无提示**。
    - **实测代价**：`set_history` 协议 09-17 就进了 core-agent 源码，dev 实际跑的却是 **09-15** 的 agent.exe（`core-agent/target/release` 与 `src-tauri/target/debug` 两份都是 09-15 21:20）。用户报「恢复历史后追问，AI 完全不记得上文」，**唯一线索**是 agent stderr 里一句 `[agent] 忽略输入类型: Some("set_history")` —— 排查时一度怀疑数据库没建、前端回灌没发。
    - **判据**：日志里出现「忽略输入类型: Some(...)」= **正在跑的 agent.exe 是旧的**，不是协议写错、也不是前端没发。
    - **部署口径**：改 `core-agent/` 后要么 `cargo build --release` + 重新构建 lunac，要么核对二进制时间戳（源码 09-17、二进制 09-15 就是最典型的信号）。
36. **WebView2 profile 路径必须在启动日志里留痕（2026-09-17 加，不得静默）**：`main.rs` 现在无论 `WEBVIEW2_USER_DATA_FOLDER` 是否被外部预设都记一行 —— 未设时 `info` 记注入的 `<exe 根>\temp\webview-data`；已被预设时 `warn` 记该外部值，并提示「dev 与 release 会共用同一个 profile」。
    - **为什么**：这个变量是「一旦存在就完全接管」的，原实现只在**未设**时才注入自己的路径。实测踩过：某次调试在终端里把 `$env:WEBVIEW2_USER_DATA_FOLDER` 设成 release 路径后忘了清，之后从同一终端启动的 **dev** lunac 一直在用 release 的 profile（`msedgewebview2.exe` 命令行实测 `--user-data-dir=D:\Lunac\temp\webview-data\EBWebView`），dev / release 的缓存与 leveldb 互相污染 —— 而日志里**一个字都没有**。
    - **排查命令**：看 `msedgewebview2.exe` 的 `--user-data-dir`；另用 `[Environment]::GetEnvironmentVariable('WEBVIEW2_USER_DATA_FOLDER','User'/'Machine')` 确认是否被持久化（都没有 = 会话级残留，清掉即可）。
    - **易误判**：任务管理器里绝大多数 `msedgewebview2.exe` 属于 **Windows 自己的 `SearchHost.exe`**（命令行含 `--webview-exe-name=SearchHost.exe`），与本应用无关，别当成「Lunac 跑了多个实例」。Lunac 自己的 WebView2 子进程（browser / gpu / network / storage / renderer / crashpad）是 Chromium 多进程架构的**必需形态**，**无法合并进 lunac.exe 单进程**（`--single-process` 不被 WebView2 支持）。
37. **任务快照：压缩不能丢「当前在做什么」（2026-09-17，backlog §8.3 落地）**：`compact_history()` 在**丢弃**历史时，把最近一条 `TodoWrite` 的清单抄成一段纯文本、钉回历史开头（第 1 条之后），表头常量 `TASK_SNAPSHOT_HEADER`。
    - **为什么需要**：`TodoWrite` 的清单躺在历史**中段**，而它恰是「当前任务」的唯一载体 —— `drop` 一压就没了，模型随后就会跑偏（这就是 §8.3 的整个动机）。
    - **不新造工具**：清单语义本来就等于「当前任务清单」（`TodoWrite` 每次发完整列表、覆盖上一份，见 [tools.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/tools.rs) 的工具描述），直接复用即可，不引第二个工具。
    - **实现要点**：① **必须在 `drain` 之前抄**（源马上就不存在了）；② **只在「被丢的区间里真的含 `TodoWrite`」时才抄**（否则与幸存的那份重复，反而干扰模型）；③ 插回去的是**一条纯文本 user 消息**，与 `tool_use` / `tool_result` 的配对结构完全解耦 —— 端点是硬校验配对的，直接保留原工具消息会把配对拆坏；④ **插在第 1 条之后**，不抢「开头那条用户提问 = 任务目标」的位置（丢弃逻辑刻意保留 head 就是为了它）；⑤ 文案用英文，与 `TRIMMED_MARKER` / elide 占位串一致（给模型看的元信息，不进 i18n）。
    - **对命中率的影响（明账）**：它在历史靠前处插入 ⇒ 其后的前缀缓存作废。但**只在已经 `dropped > 0` 的轮次发生**，而那一轮的 `drain` 本来就把缓存废了 ⇒ **不算额外损失**。它是规则 23「断裂源」清单的新成员，评估压缩收益时要一并算入。
    - 测试：`task_snapshot_survives_a_drop`（Force 档真丢消息 + 快照存活 + 位置在第 1 条之后）、`task_snapshot_is_not_pinned_when_nothing_is_dropped`、`task_snapshot_takes_the_latest_list`（取最近一条 / 空清单 / 空历史）。core-agent `cargo test` **35 passed**。
38. **环境变量注入一律用「合并」语义，不得用「没有才设」（2026-09-18 修，不得回退）**：`main.rs` 注入 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 时，**必须读出现有值、把自己那串追加在后面**，而不是 `if var(...).is_err() { set_var(...) }`。
    - **本应用「必须存在」的旗标只有两条**（`REQUIRED_WEBVIEW_FLAGS`，2026-09-18 定案）：① `--disable-features=PermissionPrompt,ClipboardContentRead`（权限弹窗抑制 + 禁用浏览器侧剪贴板读 API）；② `--js-flags=--scavenger_max_new_space_capacity_mb=8`（压 V8 新生代堆，官方旗标表收录，**只降内存、不减进程**；代价是小 GC 更频繁）。**逐项检查、只补缺失的那些**，所以外部预设的 `--remote-debugging-port=9222`（HKCU，用户决定长期保留）会被原样留下。
    - **明确不加的旗标**：`--disable-gpu`（会去掉 gpu-process 省 ~11 MB，但代价是软件光栅化，而本 UI 有 10 处 `backdrop-filter: blur()`）、`--single-process`（WebView2 不支持）、`--in-process-gpu`（**不在** Microsoft 官方旗标表里）。三者的评估记录见 [architecture-rendering.md](./architecture-rendering.md) §4.3。
    - **为什么**：这类变量**极易被外部预设**，而 `is_err()` 守卫会让注入**完全静默地失效**。实测（2026-09-18）：`HKCU\Environment` 里存在 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = --remote-debugging-port=9222`，于是浏览器进程命令行里**只有** WebView2 自带的 `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`，我们的 `--disable-features=PermissionPrompt,ClipboardContentRead` **根本不在** ⇒ 剪贴板权限弹窗抑制、`navigator.clipboard.read()` 禁用**一直是摆设**，且日志、界面、退出码**全都没有任何异常**。
    - **与规则 36 的区别**：`WEBVIEW2_USER_DATA_FOLDER` 是「一旦被预设就整体接管、无法合并」，所以那里的正确做法是**告警留痕**；命令行参数**可以拼接**，所以必须拼接。两条规则合起来是同一句话：**外部预设 env 时，既不能静默失效，也不能假装无事发生**。
    - **Chromium 侧的依据**：解析 argv 时重复的 `--disable-features` 逐项逗号合并（union），所以追加同名 switch 不会挤掉 WebView2 自带的那份。**这一条必须靠实测复核**（见 architecture-rendering.md §6 的 `disable-feat` 字段）：若发现我们的串**挤掉了** WebView2 自带值 ⇒ 改为「并入同一个 switch 的值」。
    - **顺带的能力**：合并语义使 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 成为一个**免编译的 A/B 入口** —— 在普通 shell 里设成 `--disable-gpu` 再启动即可试旗标，不必重新构建。**注意 Trae 沙箱会拦掉 `D:\Lunac\temp\*` 的写入**，`WebView2` 环境创建直接失败（表现为「进程数为 0」），所以 A/B 必须在**普通 PowerShell** 里跑。
    - **任何时候都要留痕**：合并后 `log::info` 记最终值（对照规则 36 的埋点纪律）。
39. **摘要式压缩：只在丢弃档触发、失败必须降级（2026-09-18，backlog §8.2 落地）**：机械压缩（瘦身 / 丢弃）**不额外调模型**，是默认且无条件执行的那条路；摘要压缩是它的**可选补强** —— 当丢弃档真的扔掉一大段历史时，花**一次** API 调用把它压成摘要钉回历史开头，而不是只留一句 `TRIMMED_MARKER`。实现：`render_dropped_for_summary()` / `summarize_dropped()` / `pin_summary_of_dropped()`，提示词常量 `SUMMARY_PROMPT`。
    - **只在两个地方触发**（`run_query` 的调用点）：① >95% 的**丢弃档**；② 400「上下文超限」的 **`Force` 兜底档**。**0.85 的瘦身档绝不触发**。理由是硬约束「改写前缀 ⇒ 缓存整段作废 ⇒ 触发频次要尽量低」—— 这两个档位本来就必然要 `drain`、本来就已经把缓存废掉了，摘要属于**净赚**；挂在其它地方就是**额外制造压缩时机**（违反「宁可压得晚也不要压得勤」）。
    - **三道成本闸**（一次调用 = 真实花钱 + 真实耗时，缺一不可）：① `SUMMARY_MIN_INPUT_CHARS`(4000) —— 被丢内容太短直接跳过，不值得付费；② `SUMMARY_MAX_INPUT_CHARS`(24000) —— 送进去的原文封顶，**从最近的往老的取**（被丢区间里越靠近现在越相关），单轮输入成本由此封顶；③ `SUMMARY_MAX_OUTPUT_TOKENS`(1024) —— **摘要压得短是刻意的**，它要长期留在上下文里，比原文更贵。
    - **开关 `LUNAC_SUMMARY_COMPACT`**：`0` / `false` / `off` / `no` 关闭；**默认开**（只在丢弃档触发，本身已很稀有）。
    - **失败一律降级、绝不上抛**：请求超时、网络层错误、非 2xx、响应不是 JSON、摘要为空 —— 五种情况都 `log::warn` + 返回 `None`，调用方照旧走纯机械压缩。摘要压缩是「锦上添花」，它挂掉**不能**让整轮对话失败。**超时必须单独收窄**：共享的 `cfg.client` 超时是 `REQUEST_TIMEOUT_SECS`(1800s，给流式主请求的)，摘要用 `RequestBuilder::timeout(SUMMARY_TIMEOUT_SECS=60s)` **每请求覆盖** —— 否则摘要一卡，用户要等半小时才拿回对话。
    - **非流式**：请求体 `"stream": false`。摘要是内部产物，不往前端流，也不占用 `stream_event` 通道；因此**不新增任何 stdout 协议**（只发既有的 `system/context_compacted`，前端零改动、零解析风险）。
    - **位置与顺序**：钉在**第 1 条之后**（同规则 37 的任务快照）。两者同时存在时为 `[摘要][任务快照]` —— 摘要是「过去发生了什么」，任务快照是「现在要做什么」，读起来由远及近。
    - **提示词形态取自 Hermes 的 `context_compressor.py`**：首段固定 `## Historical Task Snapshot`，要求**逐字捕获用户最近一条未完成输入**，并显式写明「用户刚问了一个问题也算 active task，**不要写 None**」；优先级 **latest user message WINS**，历史里出现的 `Historical Task` / `In Progress` / `Pending` / `Remaining Work` 章节一律视为**历史**。另加两条本项目的要求：**用原文语言写**、**保留精确标识符**（文件路径 / 函数名 / 命令行 / 错误串）。
    - **顺带修掉的既有 off-by-one（不得回退）**：`CompactOutcome` 新增 `pinned`（压缩过程中插回的合成消息条数）。回滚锚点 `base` 必须算成 `base - dropped + pinned`，且摘要插入后再 `+1`。原实现只做了减法 ⇒ 「丢弃 + 钉任务快照」那一轮若出错，`finish_error` 的 `history.truncate(base)` 会**多切掉一条真实历史**（加上摘要就是两条）。
    - **测试**：`dropped_messages_are_handed_to_the_caller_for_summarising` / `pinned_counts_the_synthetic_messages_inserted` / `summary_input_keeps_the_newest_and_stays_under_the_cap` / `summary_input_skeletonises_tools_and_skips_empty_messages` / `nothing_is_summarised_when_nothing_was_dropped` / `tiny_dropped_regions_are_not_summarised`。core-agent `cargo test` **43 passed** / 2 ignored。
    - **有效测试途径（三条，按成本从低到高，各覆盖不同的东西）**：

      | # | 途径 | 命令 | 覆盖 / 不覆盖 | 成本 |
      |---|---|---|---|---|
      | 1 | **本地 stub 单测** | `cd core-agent; cargo test summary` | ✅ 请求形状（`stream:false` / `max_tokens` / `system` 是提示词 / **不带 tools** / 单条 user / 渲染后的原文）、响应解析（`content[].text`）、**五种失败降级**（超时 / 非 2xx / 非 JSON / 空摘要 / 连不上）、触发守卫（空 `dropped` 不发请求、太短不调模型）、渲染上限与时间顺序、`dropped_msgs` 与 `pinned`<br>❌ 证明不了**真端点接受这个形状** | 零（不联网、进常规 `cargo test`） |
      | 2 | **真端点 `#[ignore]` 用例** | `$env:LUNAC_AGENT_BASE_URL=…; $env:LUNAC_AGENT_TOKEN=…; $env:LUNAC_AGENT_MODEL=…`<br>`cd core-agent; cargo test summary_compaction_against_the_real_endpoint -- --ignored --nocapture` | ✅ 端点是否接受 `stream:false` + 无 `tools`（这是**唯一**必须真端点才能验的部分）、`system` 是否被接受、摘要内容是否符合模板（首段为 `## Historical Task Snapshot`）、**latest user message WINS**（用例内嵌 sentinel `LUNAC-SENTINEL-8421`，检查是否逐字留在摘要里）<br>⚠ sentinel 断言偶发失败 = **模型改写**而非代码 bug，此时看 `--nocapture` 的打印人工判 | 一次摘要调用：输入 ≤24000 字符 ≈ 6k token、输出 ≤1024 token |
      | 3 | **端到端（验触发条件真的会命中）** | 见下方配方 | ✅ 水位判断 → `Drop` 档 → `pin_summary_of_dropped` 整条链在**真实会话**里跑通（前两条都到不了这里：途径 1/2 是直接调用 `summarize_dropped`，绕过了 `run_query` 的水位检查） | 一次大 `tool_result` 的会话 |

      **途径 3 的配方**（关键：**不要等真跑满 128k**，把预算压到允许的最小值即可，`MIN_CONTEXT_TOKENS = 8000`）：

      1. 给 agent 注入 `LUNAC_MAX_CONTEXT_TOKENS=8000`。dev 写在 [app/src-tauri/.env](file:///d:/cc/claude-code-cli-master/app/src-tauri/.env)（`dotenvy::dotenv()` 载入宿主 env，agent.exe 继承）；release 在启动 `lunac.exe` 的 shell 里先设再启动。
      2. 让 agent 读一个 ≥ 40KB 的文件（如 `docs/ai-spec.md`）⇒ 单个 `tool_result` 就远超 `0.95 × 8000 = 7600` token 的水位；随后**再追问一句**触发下一轮水位检查。
      3. 判据（落盘日志 / 控制台 stderr）：
         - `[agent] 上下文压缩：… 丢弃 N 条旧消息` + `system/context_compacted` ⇒ 丢弃档命中；
         - `摘要压缩：N 条旧消息 / M 字 → 摘要 K 字（输出 T tokens，耗时 Ums）` ⇒ **§8.2 整条链跑通**；
         - 只看到 `跳过摘要压缩：被丢内容仅 X 字 < 阈值 4000 字` ⇒ 触发了但被成本闸拦住（**这也是有效证据**，说明触发点对、闸门对）。
      4. **对照组（必做，用来证明「是摘要压缩在起作用」而不是别的东西）**：同样流程下把 `LUNAC_SUMMARY_COMPACT=0` 再跑一遍 ⇒ 应当**只有** `上下文压缩：…` 而**没有** `摘要压缩：…` 那行。开关在/不在的差异就是判据。
      5. 注意：摘要请求是**非流式**的，前端不会看到任何流式变化 —— 它只在下一轮请求的 `history` 里多出一条 `[summary of earlier conversation …]`。

    - **端到端实测结论（2026-09-18 已跑通，A/B 闭环）**：用**直接驱动 `agent.exe`** 的方式跑的（它只吃 stdin/stdout，不需要 WebView2 与 `D:\Lunac`，因此可绕开沙箱）。同一份 16 条 / 69108 字历史 + `LUNAC_MAX_CONTEXT_TOKENS=8000`，只变 `LUNAC_SUMMARY_COMPACT`：

      | | run1（默认开） | run2（`=0`） |
      |---|---|---|
      | turn1 请求前缀 | `17条/71594字` | `17条/71589字` |
      | 压缩日志 | `上下文压缩：瘦身 0 个 tool_result（省 0 字），丢弃 10 条旧消息`<br>**`摘要压缩：10 条旧消息 / 23402 字 → 摘要 786 字（输出 728 tokens，耗时 3646ms）`** | 仅 `上下文压缩：…丢弃 10 条旧消息`（**无摘要行**） |
      | turn2 请求前缀 | `10条/5967字` | `9条/4949字` |
      | turn2 实测上下文 | **2503** tokens（in 2119 + read 384） | **2092** tokens（in 1452 + read 640） |
      | turn2 追问「把那段历史里的标记复述一遍」 | **`LUNAC-SENTINEL-8421`** | **`不知道`** |

      **为什么这条能当判据**：sentinel 只存在于**被丢弃的那段历史**里；turn1 的提问是刻意中性的（「请只回复四个字：已收到。」）且其回复不含 sentinel，而 turn1 的回复落在 `KEEP_TAIL` 被保留 —— 所以 turn2 能复述 sentinel **唯一可能的来源就是那张摘要** ⇒ 「latest user message WINS」拿到实证。run2 用同一份数据、同一个变量答「不知道」⇒ 开关与因果链同时闭合。数字也对得上：`2503 - 2092 = 411 tokens` ≈ 786 个中文字的 token 量。

      **造数据时必须避开的两个陷阱（已踩过，写下来免得重踩）**：
      1. **历史必须严格交替 `user`/`assistant`** —— `normalize_history` 会合并连续同角色消息。第一版造了 15 条、其中 3 条连续 `assistant`，被合并成 1 条 ⇒ 只剩 5 条 < `COMPACT_KEEP_TAIL`(8) ⇒ `cut == head` ⇒ **DROP 分支进了但 `dropped = 0`**，日志里什么都没有（看起来像功能没生效，其实是无区间可丢）。
      2. **`cut` 要按真实公式算** —— `run_query` 是**先 push 本轮提问、再检查水位**，所以检查时 `len = 历史 + 3`（turn1 提问 + turn1 回复 + turn2 提问）、`cut = len - 8`。少算 1 就会把 sentinel 排到 `KEEP_TAIL` 里，断言永远不成立。
      3. 另外：sentinel 要放在**被丢区间 `[1, cut)` 里下标最大的那条 user 消息**上（「latest user message WINS」取的是 transcript 里最新的用户输入，不是任意一条）。

## 12. Agent Plan 模式规范

*来源：Hermes Agent 的 `plan` SKILL.md（MIT 协议，obra/superpowers 贡献），经适配整合。*

### 12.1 核心原则

**Plan 模式下只做计划，不执行代码。**

- 不实现代码、不编辑项目文件（plan markdown 除外）
- 不运行会修改状态的终端命令（不 commit/push/外部操作）
- 可以读仓库、搜索代码、理解上下文

### 12.2 计划文档结构

```markdown
# [功能名称] 实现计划

> **For Lunac Agent:** 按 task-by-task 实现此计划。

**目标：** [一句话描述]

**架构：** [2-3 句方案说明]

**技术栈：** [关键技术/库]

---

### Task N: [描述性名称]

**目标：** 一句话说明此任务完成什么

**文件：**
- 新建: `exact/path/to/new_file.ts`
- 修改: `exact/path/to/existing.ts:45-67`
- 测试: `tests/path/to/test_file.ts`

**Step 1: 写失败测试**
[完整代码]

**Step 2: 运行测试验证失败**
命令: `...`
预期: FAIL — "function not defined"

**Step 3: 写最小实现**
[完整代码]

**Step 4: 运行测试验证通过**
命令: `...`
预期: PASS

**Step 5: 提交**
git add ... && git commit -m "feat: ..."
```

### 12.3 Bite-Sized 任务粒度

**每个任务 = 2-5 分钟专注工作。**

每步一个动作：
- "写失败测试" → 一步
- "运行确认失败" → 一步
- "写最小实现" → 一步
- "运行确认通过" → 一步
- "提交" → 一步

**太粗**（禁止）：
```markdown
### Task 1: Build authentication system
[50 lines of code across 5 files]
```

**正确粒度**：
```markdown
### Task 1: Create User model with email field
[10 lines, 1 file]
### Task 2: Add password hash field
[8 lines, 1 file]
```

### 12.4 计划编写流程

1. **理解需求** — 功能需求、验收标准、约束条件
2. **探索代码库** — 项目结构、类似功能、现有测试
3. **设计方案** — 架构模式、文件组织、依赖、测试策略
4. **编写任务** — 按序：Setup → 核心功能(TDD) → 边界 → 集成 → 清理
5. **补全细节** — 精确文件路径、完整代码示例、精确命令与预期输出
6. **审核计划** — 任务顺序合理、路径精确、代码可直接运行

### 12.5 铁律

- **DRY** — 不复制粘贴
- **YAGNI** — 只实现当前需要的
- **TDD** — 每个任务先测后写
- **频繁提交** — 每个任务完成后提交
- **精确路径** — 不是"配置文件"而是 `src/config/settings.ts`
- **完整代码** — 不是"加验证"而是完整函数代码

### 12.6 常见错误

| 错误 | 正确 |
|------|------|
| "添加认证功能" | "创建含 email 和 password_hash 字段的 User 模型" |
| "Step 1: 加验证函数" | "Step 1: 加验证函数" + 完整函数代码 |
| "Step 3: 测试它" | "Step 3: 运行 `pytest tests/test_auth.py -v`，预期: 3 passed" |
| "创建模型文件" | "创建: `src/models/user.py`" |

---

## 13. 新模块（来自 Hermes 整合）

### 13.1 安全模式引擎 (`core/security/`)

来源：Anthropic `claude-plugins-official` 仓库 (Apache 2.0)，经 Hermes Agent 中继。

| 文件 | 说明 |
|------|------|
| `core/security/patterns.ts` | 25 条安全规则定义（RuleId 枚举 + SECURITY_PATTERNS 数组） |
| `core/security/index.ts` | 扫描器：`scanContent()` / `hasSecurityIssues()` / `formatWarnings()` |

**规则覆盖**：
- 代码注入：`child_process.exec`、`eval()`、`new Function()`、`os.system()`、`subprocess shell=True`、Go `exec.Command` with shell
- XSS：`dangerouslySetInnerHTML`、`document.write`、`innerHTML`、`outerHTML`、`insertAdjacentHTML`
- 反序列化：pickle/cPickle/cloudpickle/dill/marshal/shelve/joblib/pandas/numpy
- 加密缺陷：AES ECB、Node `createCipher` (no IV)、TLS 验证禁用
- 基础设施：GitHub Actions 注入、yaml.load (非 safe)、XML XXE、script SRI 缺失、torch.load 不安全

**使用方式**：在 FileWriteTool/FileEditTool 写入文件后，调用 `scanContent(content, filePath)` 扫描是否触犯安全规则，将匹配的 warnings 注入到下一轮 assistant 上下文。

### 13.2 会话文件清理服务 (`core/services/cleanup.ts`)

来源：Hermes Agent `disk-cleanup` 插件 (MIT)，@LVT382009 贡献。

| API | 说明 |
|-----|------|
| `track(path, category)` | 注册文件到生命周期追踪 |
| `forget(path)` | 移除追踪（不删文件） |
| `quick()` | 安全确定性清理，按策略自动删除 |
| `status()` | 分类统计 + Top 10 大文件 |
| `cleanupSession()` | 会话结束时调用 quick() |
| `guessCategory(path)` | 从文件路径自动推断分类 |

**生命周期策略**：
- `test` → 会话结束立即删除
- `temp` → 7 天后删除
- `session` → 14 天后删除
- `download` / `other` → 始终保留

**安全约束**：只操作 `LUNAC_HOME` 下的文件，保护 `logs/sessions/cache/skills/plugins/` 等目录不被清理。

---

## 14. 系统化调试规范

*来源：Hermes Agent 的 `systematic-debugging` SKILL.md（MIT 协议，obra/superpowers 贡献），经适配整合。*

### 14.1 铁律

```
不找到根因，不写修复代码。
```

如果你还没完成 Phase 1（根因调查），不能提出任何修复方案。对症状打补丁 = 失败。

### 14.2 四大阶段

**Phase 1：根因调查**（必须完成才开始修复）

1. **仔细读错误信息** — 不要跳过错误/警告，它们常含精确解答
2. **构建紧反馈循环** — 一条命令就能触发用户症状 → 修复后变绿 → 快且确定性
3. **检查近期变更** — `git log --oneline -10`、`git diff`
4. **多组件系统采集证据** — 在每个组件边界记录进出数据
5. **追溯数据流** — 坏值从哪来？一路向上溯源

**Phase 1 完成清单**：□ 错误已理解 □ 紧循环可运行且红 □ 近期变更已审查 □ 证据已采集 □ 问题隔离到具体组件/代码

**Phase 2：模式分析**

- 最小化复现 → 找出正常示例 → 对比差异 → 理解依赖

**Phase 3：假设与测试**（科学方法）

- 生成 3-5 个可证伪假设，按概率排序
- 一次只测一个变量
- 不加补丁叠补丁

**Phase 4：实现修复**

- 先写复现测试（参考 TDD 规范）
- 只修复根因，一处改动
- **3 次修复失败铁律**：如果尝试 ≥3 次修复仍不奏效 → **停下来质疑架构**，不是继续试第 4 次

### 14.3 红旗 — 立即停下来回到 Phase 1

- "先快速修一下，之后再调查"
- "试改改 X 看行不行"
- "同时改多处，一起跑测试"
- "大概就是 X，修一下吧"
- "还不太理解，但这个可能有效"
- **"再来一次"（已经试过 2 次以上）**
- **每次修复都在不同地方暴露新问题**

### 14.4 常见借口与真相

| 借口 | 真相 |
|------|------|
| "问题简单，不需要走流程" | 简单问题也有根因，流程对简单 bug 也很快 |
| "紧急，没时间走流程" | 系统化调试比猜-测-猜更快 |
| "先试一下这个，再调查" | 第一次修复定下模式，从一开始就做对 |
| "修完再写测试" | 没测试的修复不牢靠 |
| "同时改多处省时间" | 无法隔离哪个有效，还引入新 bug |

---

## 15. 代码审查工作流

*来源：Hermes Agent 的 `requesting-code-review` SKILL.md（MIT 协议，obra/superpowers + MorAlekss 贡献），经适配整合。*

### 15.1 核心原则

**不要让写代码的 agent 自己审自己。** 独立上下文才能发现盲区。

### 15.2 8 步预提交管道

**Step 1 — 获取 diff**
```bash
git diff --cached
```
如果为空，试 `git diff` 再试 `git diff HEAD~1 HEAD`。diff 超过 15K 字符则按文件拆分。

**Step 2 — 静态安全扫描**（仅扫描新增行）

```bash
# 硬编码密钥
git diff --cached | grep "^+" | grep -iE "(api_key|secret|password|token)\s*=\s*['\"][^'\"]{6,}['\"]"
# Shell 注入
git diff --cached | grep "^+" | grep -E "os\.system\(|subprocess.*shell=True"
# 危险 eval/exec
git diff --cached | grep "^+" | grep -E "\beval\(|\bexec\("
# 不安全反序列化
git diff --cached | grep "^+" | grep -E "pickle\.loads?\("
# SQL 注入
git diff --cached | grep "^+" | grep -E "execute\(f\"|\.format\(.*SELECT"
```

**Step 3 — 基线测试和 Lint**：以变更前为基准，只拦截**新引入**的失败。

**Step 4 — 自查清单**
- □ 无硬编码密钥/凭据
- □ 用户输入有校验
- □ SQL 用参数化查询
- □ 文件操作验证路径（无穿越）
- □ 外部调用有错误处理
- □ 无残留 debug print
- □ 无注释掉的代码
- □ 新代码有测试

**Step 5 — 独立审查**：派生子 agent，只给 diff + 静态扫描结果，不共享上下文。Fail-closed（无法解析 = 失败）。返回 JSON：`{passed, security_concerns, logic_errors, suggestions, summary}`。

**Step 6 — 评估结果**：全部通过 → Step 8 提交；有失败 → Step 7 自动修复。

**Step 7 — 自动修复循环**：最多 2 次修复-重新验证。用第三个独立 agent 修复，不改代码、不重构、不加功能。2 次后仍失败则提交给用户，建议 `git stash` 或 `git reset`。

**Step 8 — 提交**
```bash
git add -A && git commit -m "[verified] <description>"
```
`[verified]` 前缀表示经独立审查通过。

### 15.3 与安全扫描器的关系

Step 2 的静态扫描已由 `core/security/index.ts` 的 `scanContent()` 在 FileWrite/FileEdit 工具中自动执行。此工作流提供了完整的 pre-commit 审查管道的其余环节。

---

## 16. TDD 开发规范

*来源：Hermes Agent 的 `test-driven-development` SKILL.md（MIT 协议，obra/superpowers 贡献），经适配整合。*

### 16.1 铁律

```
没有先失败的测试，不写产品代码。
```

测试还没写就写代码？删掉，重来。

**没有例外。** 不能"留作参考"、不能"边写测试边改编"、不能"看一眼"。删掉就是删掉。

### 16.2 RED-GREEN-REFACTOR 循环

**RED — 写失败测试**
- 一次一个行为
- 清晰描述性名称（名称有"and"→ 拆分）
- 真实代码，非 mock（除非不可避免）
- 一个好测试 vs 一个坏测试的区别：
  - 好：`test_retries_failed_operations_3_times()` — 清晰名称、测真行为
  - 坏：`test_retry_works()` — 模糊名称、测 mock 不测真代码

**验证 RED — 必须亲眼看着它失败**

运行测试 → 确认失败（不是拼写错误）、失败消息符合预期、因功能缺失而失败。

**GREEN — 最小代码**

只写刚够通过测试的代码。不多写一行。作弊在 GREEN 阶段是可以的：硬编码返回值、复制粘贴、跳过边缘情况。REFACTOR 阶段再修。

**验证 GREEN — 必须亲眼看着通过**

先跑当前测试 → 再跑全部测试确认无回归。

**REFACTOR — 清理**

仅通过后：消除重复、改善命名、抽取辅助函数。全程保持测试绿。测试失败 → 立即撤销、步子更小。

**重复**。下一个失败测试 → 下一个行为。一次一个循环。

### 16.3 禁止水平切片

错误做法（水平切片）：先写一堆想像的测试，再整体实现。产出脆弱测试。

正确做法（垂直 tracer bullet）：每发子弹走完整流程（一个 RED-GREEN 循环），每发教会你接口长什么样。

### 16.4 红旗 — 删除代码、立即重来

- 测试之前先写代码
- 实现之后补测试
- 测试第一次运行就通过
- 说不清测试为什么失败
- 测试"以后再写"
- 合理化"就这一次"
- "做了 X 小时了，删掉浪费"（沉没成本谬误）

### 16.5 卡住时

| 问题 | 方案 |
|------|------|
| 不知道怎么测 | 先写期望的 API 接口，先写断言 |
| 测试太复杂 | 设计太复杂，简化接口 |
| 必须全 mock | 代码太耦合，用依赖注入 |
| 测试准备代码太庞大 | 抽取辅助函数，还是大？简化设计 |

---

## 17. AI 文本去痕迹规范（Humanizer）

*来源：Hermes Agent 的 `humanizer` SKILL.md（MIT 协议，@blader 原创，基于 Wikipedia "Signs of AI writing"），经适配整合。*

### 17.1 何时应用

Agent 输出面向用户的文本时自动应用：发布说明、PR 描述、文档、长解释、摘要。用户要求 "humanize"/"de-AI"/"de-slop" 时显式加载。

### 17.2 29 个 AI 写作模式

**内容模式（6 个）**

| # | 模式 | 特征词 |
|---|------|--------|
| 1 | 夸大重要性 | stands as / testament / pivotal / vital / crucial / underscoring / marking / shaping |
| 2 | 夸大知名度 | cited in / leading expert / active social media presence |
| 3 | 伪深度 -ing 结尾 | highlighting / underscoring / emphasizing / cultivating / fostering / showcasing |
| 4 | 广告腔 | boasts / vibrant / nestled / groundbreaking / renowned / breathtaking / stunning |
| 5 | 模糊归属 | Industry reports / Observers have cited / Experts argue / Some critics argue |
| 6 | 模板化"挑战与展望"段落 | Despite its... faces several challenges... Despite these challenges... |

**语言/语法模式（7 个）**

| # | 模式 | 特征 |
|---|------|------|
| 7 | AI 高频词汇 | delve / crucial / enhancing / fostering / garner / interplay / intricate / pivotal / showcase / tapestry / testament / underscore / vibrant / landscape (抽象) |
| 8 | 系动词回避 | serves as / stands as / marks / represents / boasts / features — 替代简单的 is/are/has |
| 9 | 否定平行结构 | "It's not just about X; it's about Y" + "no guessing" 类尾随否定碎片 |
| 10 | 三法则滥用 | 凡事凑三个 |
| 11 | 同义词轮换 | protagonist → main character → central figure → hero |
| 12 | 假范围 | "from X to Y" 但 X 和 Y 不在一个量纲上 |
| 13 | 被动态/无主语碎片 | "No configuration file needed." "The results are preserved automatically." |

**风格模式（6 个）**

| # | 模式 |
|---|------|
| 14 | 破折号滥（—）用 |
| 15 | 机械式粗体强调 |
| 16 | 内联标题列表（`- **关键词:** 冒号后描述`） |
| 17 | Title Case 标题 |
| 18 | 装饰性 emoji |
| 19 | 弯引号 |

**沟通模式（3 个）**

| # | 模式 | 特征词 |
|---|------|--------|
| 20 | 聊天协作痕迹 | I hope this helps / Of course! / You're absolutely right! / let me know |
| 21 | 知识截止日期声明 | as of [date] / based on available information / While specific details are limited |
| 22 | 谄媚语气 | Great question! / That's an excellent point / 过度正面 |

**填充与模糊（7 个）**

| # | 模式 |
|---|------|
| 23 | 填充短语：In order to → To; Due to the fact that → Because; At this point in time → Now |
| 24 | 过度模糊：could potentially possibly be argued that might → may |
| 25 | 通用正面结论："The future looks bright" / "exciting times lie ahead" |
| 26 | 连字符词对：data-driven / high-quality / end-to-end → 去掉连字符 |
| 27 | 说服权威套路：The real question is / at its core / what really matters — 删除仪式感，直达内容 |
| 28 | 预告式开场：Let's dive in / Let's explore / let's break this down — 直接讲内容 |
| 29 | 碎片式标题段落：标题后跟一句重复标题意思的废句 → 删除废句 |

### 17.3 加入"人味"

光去掉 AI 痕迹不够。消毒无菌的文字同样很容易识别。

**有灵魂的文字特征**：
- 句子长短不一，节奏变化
- 有观点，不是中立报道
- 承认不确定性和混合情绪
- 适当用"I"第一人称
- 有幽默、锐利、有个性
- 具体的感受，不是"this is concerning"而是"there's something unsettling about..."

### 17.4 处理流程

1. 扫描 29 个模式
2. 改写有问题的段落
3. 保持核心含义、匹配合适语气
4. 注入灵魂（观点、节奏、个性）
5. 最后反问自己"What makes this so obviously AI generated?"→ 修改剩余痕迹

---

## 18. 插件发现架构参考

*来源：Hermes Agent 的 `context_engine/__init__.py` 和 `cron_providers/__init__.py` 插件加载模式，经逆向抽象。*

### 18.1 双目录扫描

插件从两类目录加载，内置（bundled）优先：
1. **内置插件**：`bundled-plugins/<name>/` — 随应用分发
2. **用户插件**：`$APP_HOME/plugins/<name>/` — 用户安装

每个插件目录必须有 `__init__.py`（或 Lunac 等效的 `index.ts`），调用 `register(ctx)` 入口函数。

### 18.2 双重加载策略

1. 优先尝试**函数式接口**：导入 `register()` 函数，传入 `PluginContext` 收集器
2. 回退到**类实例化**：扫描模块中实现特定接口的子类并实例化

### 18.3 合成包注册

虚拟 `sys.modules` 条目使插件内的相对导入工作（如 `from .client import ...`），无需用户手动设置 Python path。

### 18.4 子模块预加载

遍历 `*.py` 文件，`importlib` 预注册所有 submodule，使插件内的交叉导入可用，避免运行时 ImportError。

### 18.5 Lunac 适配方向

Lunac 的插件系统（`app/src/plugins/registry.ts`）已实现关键词匹配 + 评分排序。未来可参考此模式扩展：
- 双目录加载（内置 + 用户）
- 入口函数注册规范
- 生命周期钩子（SessionStart/SessionEnd/PreToolUse/PostToolUse）

---

## 19. 前端集成规范（Hermes 方法论前端接入）

### 19.1 上下文感知 System Prompt 注入

**目标**：根据用户查询意图，自动向 agent system prompt 注入对应方法论章节。

在 `main.ts` 中新增 `buildSystemPromptHint(query: string): string` 函数：

```typescript
function buildSystemPromptHint(query: string): string {
  const q = query.toLowerCase();
  const hints: string[] = [];

  // Debug intent
  if (/bug|error|crash|fail|break|fix|wrong|not work|不工作|报错|崩溃|修复|调试/.test(q)) {
    hints.push(`## Debugging Methodology
Follow the 4-phase systematic debugging process:
1. ROOT CAUSE: Read errors, build tight feedback loop, check recent changes, trace data flow
2. PATTERN ANALYSIS: Find working examples, compare differences
3. HYPOTHESIS: Form 3-5 falsifiable hypotheses, test one variable at a time
4. IMPLEMENTATION: Create regression test first, then single fix at root cause
CRITICAL: If 3+ fix attempts fail → question the architecture, don't try a 4th fix.`);
  }

  // Code creation intent
  if (/create|write|add|implement|build|make|写|创建|实现|添加|新建/.test(q)) {
    hints.push(`## TDD Requirement
Write failing test FIRST before any production code. RED → GREEN → REFACTOR.
No exceptions: delete any code written before its test exists.`);
  }

  // Review intent
  if (/review|check|verify|audit|审查|检查|验证|审计/.test(q)) {
    hints.push(`## Code Review Pipeline
8-step pre-commit verification: diff → static scan → baseline tests → self-review → independent review → evaluate → auto-fix (max 2) → commit with [verified] prefix.`);
  }

  // Text output — always apply humanizer for agent responses
  hints.push(`## Output Style
Remove AI writing patterns: no "stands as / testament / pivotal / crucial / underscoring / delve / tapestry / landscape / fostering / moreover / furthermore / in conclusion". No emoji decorations. No "I hope this helps / let me know / great question". No boldface headers in lists. Use simple "is/are/has" instead of "serves as/stands as/represents". Vary sentence rhythm. Have opinions.`);

  return hints.join('\n\n');
}
```

**注入点**：在 `startAgentChat()` 构造首条消息时，将 `buildSystemPromptHint(userQuery)` 追加到消息 content 前面。

### 19.2 调试阶段状态栏

在 `agentView` 中新增调试状态跟踪：

```typescript
let debugPhase = 0;        // 0=none, 1=root cause, 2=pattern, 3=hypothesis, 4=implement
let fixAttempts = 0;       // Track consecutive fix attempts
let lastToolError = "";    // Most recent tool error for reference
```

**触发条件**：当 `tool_result` 返回失败（`is_error: true` 或工具返回错误文本）时：
- `debugPhase = 1`，状态栏显示 "🔍 调试 Phase 1: 根因调查"
- 每次后续 `tool_use` 为 Bash/Read/Glob/Grep 时递增 phase
- `fixAttempts++` 每次 FileWrite/FileEdit 发生

**3 次修复失败警告**：当 `fixAttempts >= 3` 且仍有错误时：
- 状态栏红色显示 "⚠ 3 次修复失败 — 建议质疑架构方案"
- 注入额外的 message：`"You have attempted 3 fixes and the issue persists. Per the debugging methodology, STOP and question the architecture. Do not attempt a 4th fix. Discuss with the user about alternative approaches."`

### 19.3 Humanizer 按钮

在 `#chat-input-bar` 中新增 "Humanize" 按钮（仅 Agent 模式下可见）：

```html
<button id="humanize-btn" title="去除 AI 痕迹">✎</button>
```

点击后：
1. 取最后一条 agent 文本输出
2. 作为新消息发送：`"Rewrite the following to remove AI writing patterns. Follow the humanizer methodology: remove significance inflation, promotional language, AI vocabulary words, copula avoidance, em dashes, boldface headers, emoji decorations, collaborative artifacts, knowledge-cutoff disclaimers, filler phrases, and generic conclusions. Add a human voice with varied rhythm, opinions, and specific details. Output ONLY the rewritten text:\n\n" + lastOutput`
3. 不做额外解释，直接替换显示区域

### 19.4 代码审查结果行内渲染

扩展 `agentView` 的 `tool_result` 渲染，当 security warnings 存在时：

```
┌─────────────────────────────────────────┐
│ ⚠ Security scan: 2 warning(s)            │
│ ### child_process_exec                   │
│ ⚠ Security Warning: Using child_process... │
│ ### eval_injection                       │
│ ⚠ Security Warning: eval() executes...   │
├─────────────────────────────────────────┤
│ ✓ File updated successfully              │
└─────────────────────────────────────────┘
```

现有实现：`securityWarnings` 已注入 `tool_result` content，但以纯文本方式追加。应改为带有样式的独立警告块（黄色边框 + 半透明背景）。

### 19.5 会话清理状态

在 `/clear` 或 agent 会话结束时，调用 `cleanupSession()`：

```typescript
// In closePluginView() or on session end
if (agentChatHistory.length > 0) {
  invoke("cleanup_session").catch(() => {});
}
```

Rust 侧新增 `cleanup_session` IPC（调用 Node.js 的 `cleanupSession()` 需通过 CLI subprocess 或改为 Rust 原生实现，短期方案：前端的 `cleanupSession()` 由 Tauri 前端侧执行）。

### 19.6 待实现清单

| 优先级 | 改动 | 位置 | 工作量 |
|--------|------|------|--------|
| P0 | 上下文感知 System Prompt 注入 | `main.ts` `startAgentChat()` | ~50 行 |
| P1 | Humanizer 按钮 | `main.ts` + `styles.css` + `index.html` | ~40 行 |
| P1 | 安全警告样式块 | `main.ts` tool_result 渲染 + `styles.css` | ~30 行 |
| P2 | 调试阶段状态栏 | `main.ts` agent 流处理器 | ~60 行 |
| P2 | 3 次修复失败警告 | `main.ts` + 自动注入 message | ✅ 已完成 (2026-07-21) |
| P3 | 会话清理前端调用 | `main.ts` `closePluginView()` | ✅ 已完成 (2026-07-21) |

---

## 20. 用户自定义 Agent 工具/技能 — 双路径执行计划

> **状态：路径 1 已完成 ✅ / 路径 2 已完成 ✅，2026-07-21 执行完毕**
>
> ⚠️ **2026-09 架构变更提示**：本节所述的 MCP 客户端 / SkillTool / 技能加载等基础设施来自**上游 Claude Code 源码（`core/`）**，该目录已停用；自研 `agent.exe` 侧的对应能力（P3 MCP 工具桥、P4 技能）见 §3.5。下文凡涉及 `core/...` 路径的条目均指旧实现，修改 core-agent 时须按 §3.5 契约重做。

### 背景

当前架构存在核心缺口：用户无法为 Agent 添加自定义工具或技能。

```
现有：
  App 插件 (registry.ts)  ←→  UI 层（搜索/计算/编码）
  Core 工具 (tools.ts)    ←→  编译时硬编码，用户不可扩展
  唯一动态机制：MCP 协议（旧实现于 core/，现待 core-agent 重做）
```

旧上游 core 曾具备完善的基础设施（**均已随 `core/` 停用**）：
- **MCP 客户端** (`core/services/mcp/client.ts`) — 动态发现 MCP server 的工具列表并注入 Agent
- **插件市场 Schema** (`core/utils/plugins/schemas.ts`) — 完整 Zod schema，支持 7 种安装来源
- **SkillTool** (`core/tools/SkillTool/SkillTool.ts`) — inline/fork/remote 三种执行模式
- **文件技能加载** (`core/skills/loadSkillsDir.ts`) — 从 `.claude/skills/SKILL.md` 自动发现
- **工具搜索/延迟发现** (`core/utils/toolSearch.ts`) — LLM 可按需发现 MCP 工具

### 路径 1：MCP 桥接层 — 声明式用户自定义工具（优先级 P0）

#### 1.1 设计

```
用户自定义工具定义 (JSON/YAML 配置文件)
    ↓ 读取
Lunac 内置 MCP Server  (app/src-tauri/src/mcp_server.rs)
    ↓ stdio（agent.exe 启动时以 `--mcp-server stdio:<path>` 挂载）
agent.exe MCP Client  （旧实现 core/services/mcp/client.ts，core-agent 侧待 P3 重做）
    ↓ tools/list → tools/call
Agent 工具池自动注入 → LLM 可调用
```

#### 1.2 用户配置文件格式

存储路径：`%LOCALAPPDATA%\Lunac\tools\*.json`

```jsonc
// tools/weather.json — 示例
{
  "name": "get_weather",
  "description": "Get current weather for a city",
  "inputSchema": {
    "type": "object",
    "properties": {
      "city": { "type": "string", "description": "City name" }
    },
    "required": ["city"]
  },
  "handler": {
    "type": "shell",           // "shell" | "http" | "python" | "bun"
    "command": "curl -s 'wttr.in/{{city}}?format=3'"
  }
}
```

支持的 handler 类型：
| 类型 | 说明 | 示例 |
|------|------|------|
| `shell` | 子进程执行（支持 `{{param}}` 占位符） | `curl`, `python script.py` |
| `http` | HTTP 请求（method/url/headers/body） | REST API 调用 |
| `python` | 调用 Python 脚本（参数 stdin JSON） | 复杂数据处理 |
| `bun` | 调用 Bun 脚本（同上） | TypeScript 工具逻辑 |

#### 1.3 实现步骤

| 步骤 | 文件 | 描述 |
|------|------|------|
| 1 | `app/src-tauri/src/mcp_server.rs` | 实现 MCP stdio server：从 `%LOCALAPPDATA%\Lunac\tools\` 读取所有 JSON → 构造 `tools/list` 响应 → `tools/call` 时匹配 handler 类型并执行 |
| 2 | `app/src-tauri/Cargo.toml` | 确保 `serde_json` 已存在，无需额外依赖（MCP 协议纯 JSON over stdio） |
| 3 | `app/src-tauri/src/commands.rs` | `start_cli_process()` / `start_cli()` 已在启动 agent.exe 时追加 `--mcp-server stdio:<path>`（P0 侧仅接受并忽略）；待 core-agent 侧 P3 实现 MCP client 后即可真正消费 |
| 4 | `app/src/plugins/builtin/` | 新增 `tool-editor` 插件：JSON 编辑器 UI，搜索 "tool" / "工具" 进入，可视化创建/编辑/删除工具定义 |
| 5 | `app/src-tauri/src/mcp_server.rs` | 实现 `resources/list`（可选）：将工具定义文件列作 resource，支持 `resources/read` 读取详情 |
| 6 | `docs/` | 新建 `tools.md`：用户文档，格式说明 + 示例（天气/翻译/文件批处理） |

#### 1.4 MCP Server 路由表

```
Client → Server:
  { "jsonrpc": "2.0", "method": "tools/list",     "id": 1 }
  { "jsonrpc": "2.0", "method": "tools/call",     "id": 2, "params": { "name": "get_weather", "arguments": { "city": "Beijing" } } }
  { "jsonrpc": "2.0", "method": "resources/list", "id": 3 }

Server → Client:
  { "jsonrpc": "2.0", "id": 1, "result": { "tools": [...] } }
  { "jsonrpc": "2.0", "id": 2, "result": { "content": [{ "type": "text", "text": "..." }] } }
```

### 路径 2：激活插件市场（优先级 P1，依赖路径 1 完成 MCP 基础）

#### 2.1 设计

```
用户 / 社区
    ↓ 发布
GitHub Releases / npm / git repos  ←── plugin.json + SKILL.md
    ↓ 安装
Core 插件注册表 (core/plugins/)
    ↓ 激活 initBuiltinPlugins()
Agent 获得新工具 + 新技能 + 新钩子
```

#### 2.2 实现步骤

| 步骤 | 文件 | 描述 |
|------|------|------|
| 1 | `core/plugins/bundled/index.ts` | 实现 `initBuiltinPlugins()`：从 `%LOCALAPPDATA%\Lunac\plugins\` 扫描已安装插件目录，每个目录加载 `plugin.json` |
| 2 | `app/src/plugins/builtin/` | 新增 `plugin-manager` 插件：前端 UI — 浏览已安装插件 / 搜索社区插件 / 一键安装 / 启用/禁用 |
| 3 | `app/src-tauri/src/` | 新增 `plugin_manager.rs`：Rust 命令 — 从 GitHub/npm/git 下载插件包 → 解压到 `%LOCALAPPDATA%\Lunac\plugins\<name>\` |
| 4 | `core/plugins/` | 实现插件热加载：文件变更检测 → `refreshTools` 回调 → Agent 工具池更新 |
| 5 | `docs/` | 新建 `plugins.md`：插件开发指南（plugin.json schema / SKILL.md 格式 / 发布流程） |

#### 2.3 插件 Manifest 最小示例

```jsonc
// plugin.json
{
  "name": "weather-tools",
  "version": "1.0.0",
  "description": "Weather query tools via wttr.in",
  "author": "community",
  "tools": [
    {
      "name": "get_weather",
      "description": "Get weather for a city",
      "parameters": { "city": "string" },
      "handler": { "type": "shell", "command": "curl -s wttr.in/{{city}}?format=3" }
    }
  ],
  "skills": [
    {
      "name": "weather-guide",
      "description": "Guide for interpreting weather data",
      "file": "SKILL.md"
    }
  ]
}
```

### 架构最终目标

```
用户自定义层
├─ tools/*.json          (声明式，JSON 配置 → MCP 桥接)
├─ plugins/<name>/       (插件市场安装的社区工具/技能包)
└─ .claude/skills/       (文件技能，SKILL.md 自动发现)
        ↓                    ↓                    ↓
   MCP Server          Plugin Registry        File Scanner
        ↓                    ↓                    ↓
        └──────────────── agent.exe Agent 工具池 ─────────────┘
                                  ↓
                          LLM 可调用的完整工具集合
```

### 前置依赖关系

```
路径 1 (MCP 桥接)
  └─ 无依赖，MCP Server 侧（mcp_server.rs）已完备 ✓；agent.exe 侧 MCP 客户端待 P3 重做
  └─ 产物：mcp_server.rs + tools/*.json 约定

路径 2 (插件市场)
  └─ 依赖：路径 1 的 MCP Server 基础（插件可复用 MCP 协议注册工具）
  └─ 产物：plugin_manager.rs + plugin-manager UI + initBuiltinPlugins()
```
