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
>
> **文档分工（2026-09-19 整理）**——每类信息**只有一个落点**，不重复搬运：
>
> | 想问什么 | 去哪看 | 本文有没有 |
> |---|---|---|
> | **还没做什么、按什么顺序做** | [agent-feature-backlog.md](./agent-feature-backlog.md)（**唯一待办真相源**） | ❌ **正文不再保留任何「待办 / 路线 / 待实现」小节**（§10 只留一句指针） |
> | agent 已实现能力的全景 | [agent-implementation.md](./agent-implementation.md) | 只留协议契约（§3.5） |
> | 对话面板的界面与交互 | [agent-ui-spec.md](./agent-ui-spec.md) | — |
> | 代码硬规则 / 反模式 | [code-rules.md](./code-rules.md) | — |
> | 图标与 UI 文案的图标规则 | [icon-style.md](./icon-style.md) | — |
> | **本规范正文** | 本文 | §1–§8 架构与现状、§11–§17 **规则**（只写「是什么 / 为什么 / 不得怎么做」）、§12 计划模式、§14 调试、§15 审查、§16 TDD |

## 1. 架构概述

Lunac 是一个 **uTools 风格的桌面启动器 / 搜索工具**，由 Tauri 2.x 驱动。它从原有的 Claude Code CLI 聊天界面重构而来，现分为三个层次：

```
┌──────────────────────────────────────────────────────────────────┐
│                  Lunac 前端 (app/src/)                             │
│  main.ts → 毛玻璃搜索栏 + 插件系统 + 键盘导航                                  │
│  plugins/builtin/ → quick-launch / web-search /                  │
│                     settings / clipboard-history / tool-editor / ai-agent /│
│                     ocr / memo 等 9 个插件                           │
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
| 插件列表 | `app/src/plugins/builtin/index.ts` | 注册所有 10 个内置插件 |
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
| 图标 | 结果行：应用 / 文件用系统图标（`get_app_icon`，异步回填 + token 作废旧回包）；插件用 `pluginIconSvg`；设置 / 动作条目用目录数据里的 emoji（列表项图标，非功能按钮 —— 见 [icon-style.md](./icon-style.md) §4）。详细搜索右侧预览区另走 `get_file_thumbnail`（图片真解码出缩略图、其余回落系统图标 —— 见 §11 规则 47） |

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
| AI 后台桥 | `main.rs` + `agent_server.rs` | 启动时 `agent_server::start()` 起 **127.0.0.1:8789** 的 HTTP/SSE 桥（给 VSCode 扩展）；无「预加载代理 + `AI_READY` / `ai-ready` 事件」这回事（那是旧的内置代理方案，已停用） |
| 子进程清理 | `main.rs` | `Destroyed` 事件 kill agent 进程 + `taskkill` 清端口 5173 |
| OCR 引擎按需部署 | `paddle_ocr.rs` + `commands.rs` | PaddleOCR-json（`.7z` 约 88MB / 解压约 300MB）**不入库**；`ocr_engine_status` 查询、`ocr_engine_install` 后台下载 GitHub Release → `sevenz-rust` 解压到 staging → 校验 exe+config → 原子替换到 `<exe 根>\paddle-ocr`，进度经 `ocr-engine-progress`/`ready`/`error` 事件回传 |
| 文件索引（详细搜索） | `file_indexer.rs` | `temp\file-index-cache.json` 唯一真相；搜索只读内存索引、永不扫盘；后台扫盘（启动 600ms 后 / 手动重建）+ 原子写；见 §2.1.2 与 §11 规则 42 |
| 系统设置与动作目录 | `system_catalog.rs` | 静态表（41 个 `ms-settings:` 页 + **26 个动作**，含 2026-09-19 补齐的 cmd / PowerShell / mmc 管理单元 / applet / regedit）；`open_setting` 只认 `ms-settings:` 前缀、`run_system_action` 只认白名单 id、`run_system_action_elevated` 另要求该动作有提权形态；见 §2.1.2 与 §11 规则 41 |
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
| 工作区设置 | `main.ts` AI 对话输入栏 ⋯ 菜单（`#chat-more-menu`）内的「工作区」行 | 行内显示当前路径 + 「选择目录 / 清除」→ `invoke("set_workspace")` + 重启 CLI；默认=`<exe 根>\temp\transStorage`（整个系统可访问，见 §11 规则 34）。**独立的 `#chat-workspace-btn` 已不存在**（2026-09 并入 ⋯ 菜单，见 agent-ui-spec §5.3） |
| 思考开关 | `main.ts` `#chat-mode-seg`（输入栏 ⋯ 菜单内）+ `set_thinking_mode` | `on` / `off` 两档 → `LUNAC_THINKING` → agent 侧 `Thinking`（见 §3.5「思考开关跨模型自适应」）；切换重启 agent |
| Token 仪表盘 | `main.ts` `addUsageToTotals` / `updateTokenDashboard` / `appendUsageLog`；**点表盘展开面板**见 [usage-cost.ts](file:///d:/cc/claude-code-cli-master/app/src/usage-cost.ts) | 计费口径：Hit=缓存读取，Miss=普通输入+缓存写入，Total=四类 token 之和。**数值 = 「当前对话」的累计**（每次提问落一行 JSONL，见 §3.5「用量与对账」），可与供应商平台按天对账。**点表盘（`#token-dashboard`，仅 `.visible` 时）向上弹出 `#token-usage-panel`**：里面是近 30 天逐日用量 + 金额（**含总计行**）与价格表状态 —— **只放数据，不放说明文字**；对话进行中每次落用量都顺带刷新（关着时不刷） |

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
| env | `LUNAC_AGENT_BASE_URL`（已是完整端点，请求拼 `/v1/messages`）、`LUNAC_AGENT_TOKEN`（**必须走 `authorization: Bearer`**；用 `x-api-key` 会被兼容端点判 401）、`LUNAC_AGENT_MODEL`；另有 `LUNAC_THINKING`（思考开关：`off` = 关，其余 = 开；见 §3.5「思考开关跨模型自适应」）、`LUNAC_MAX_CONTEXT_TOKENS`（上下文预算，默认 128000、低于 8000 的取值视为无效）、`LUNAC_SUMMARY_COMPACT`（摘要式压缩开关：`0`/`false`/`off`/`no` = 关，其余含未设置 = **开**；见 §11 规则 39）、`LUNAC_HISTORY_INDEX`（往期会话索引注入开关：同上写法，默认**开**；见 §11 规则 53）、`LUNAC_MEMORY`（长期记忆读取/注入开关：同上写法，默认**开**；见 §11 规则 56）、`LUNAC_NUDGE_INTERVAL`（后台复盘的**轮次门槛**：每 N 次用户提问跑一次，默认 10，`0` = 关；非数字回落默认；见 §11 规则 56）、`LUNAC_SKILLS_DIR`、`LUNAC_WORKSPACE_LOCKED`、`LUNAC_SEARCH_PROVIDER` + `LUNAC_SEARCH_KEY`（WebSearch 主源的服务商与密钥，服务商可选 bocha / tavily / exa / firecrawl；缺任一项则只用无 key 的 Bing / 百度兜底源）、`LUNAC_LOG_DIR`（宿主注入的日志目录 = `<exe 根>\temp\logs`）、`LUNAC_LOG`（`off` = 关闭落盘日志）、`LUNAC_LOG_LEVEL`（`error\|warn\|info\|debug`，默认 `info`；见 §11 规则 20） |
| 启动参数 | `--add-dir <dir>`（可重复，工作区外追加可访问目录）/ `--permission-mode plan`（只读）/ `--dangerously-skip-permissions`（忽略工作区锁）/ `--permission-prompt-tool stdio`（写类工具先审批）/ `--disallowedTools <name…>`（这些工具不进请求体）/ `--mcp-server stdio:<exe 路径>`（拉起该 exe 的 MCP server 并接入其工具，P3）；其余（`--print` / `--verbose` / `--input-format stream-json` / `--include-partial-messages` …）一律接受并忽略 |
| stdin | 每行一条 JSON：`{"type":"user","session_id":"","message":{"role":"user","content":[{"type":"text","text":"…"}]},"parent_tool_use_id":null}`（此处 `session_id` 是**上行占位**，agent 不读取它；带**真实**会话 id 的是下行消息，见 §3.5「会话 id 与 rewind」。A8：`content` 里可再加图片块 `{"type":"image","source":{"type":"file","path":"…"}}`，见本节「图片附件」）；`{"type":"control_response","response":{"subtype":"success","request_id":"…","response":{"behavior":"allow"\|"deny",…}}}` 为审批回包（P2，由 stdin 线程按 request_id 直接投递给等待中的工具调用）；`{"type":"set_history","messages":[{"role":"user"\|"assistant","content":"纯文本"}]}` 为**会话历史整体替换**（2026-09-17，回退 / 恢复历史时回灌上文的唯一通道，见 §11 规则 30）。`set_history` **不触发模型调用**（不是提问），只替换 agent 内的 `history` 并回一个 `system/history_set`（含 `messages` 条数）|
| stdout | 每行一条 JSON：`system/init`（含 `session_id`（真实值，A11）/ `model` / `tools` 名单）→ `system/context_compacted`（`elided` / `dropped` 计数，压缩发生时补发）→ `system/api_retry`（`attempt` / `max_retries` / `error_status` / `delay_ms`，瞬时失败退避重试时补发，前端解析分支早已就绪）→ `stream_event`（`content_block_start` / `content_block_delta`(`text_delta`\|`thinking_delta`\|`input_json_delta`) / `content_block_stop` / `message_stop`）→ `assistant`（整包，含 `tool_use`，仅无增量时前端兜底）→ `control_request`（`can_use_tool`，写类工具执行前）→ `user`（整包，含 `tool_result`）→ `result`（`subtype` / `is_error` / `usage` / `session_id`，用量为整轮累计） |

**P0 已完成**：多轮上下文（进程内 history）、SSE 增量打字、用量上报（input/output/cache_read/cache_creation）、错误回传（失败轮按 `history.truncate(base)` 整体回滚，不污染后续对话）、stdin 读取线程与查询线程经 mpsc 解耦（为 P2 的 `control_response` 预留通路）。
#cbcbcb#cbcbcb
**P1 已完成（2026-09，内置工具循环）**：**十五件**工具实现在 [core-agent/src/tools.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/tools.rs)（第 12 件 `SessionSearch` 于 2026-09-19 补入，见本节「往期会话检索」；第 13 件 `Agent` 于 2026-09-20 补入，见本节「子代理」；第 14 / 15 件 `EnterPlanMode` / `ExitPlanMode` 亦于 2026-09-20 补入，见本节「计划模式闭环（A7）」），主循环按「请求 → 流式收块 → 有 `tool_use` 就执行并以 `tool_result` 回灌 → 再请求」往返，直到模型不再调工具（上限 `MAX_TOOL_ROUNDS=16`，到顶后再给一次「只用文本收口」的机会）。

**条件注册的四件不要混进来**：`Skill`（装了技能才注册）、`ListMcpResourcesTool` / `ReadMcpResourceTool`（桥接上了用户工具才注册）、`Remember`（桥接通才注册）—— 它们**不在 `defs()` 的十五个里**，各自只在满足条件时追加进请求体。理由见各条目（§11 规则 18 ⑤ 的「空转项」）。

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
| `Agent` | `description`（3–5 词任务标签）/ `prompt`（**自包含**的任务说明，子代理看不到本对话） | 派生一个**独立上下文**的子代理跑一件自包含任务，只把最终报告回灌主对话（中间工具输出不进主上下文）。报告形态 `[task-<N>] subagent report:\n\n…`（≤8000 字符）。**要审批**（派生的是能写文件、能跑命令的子代理）、**只读档直接拒绝**、**子代理工具集不含 `Agent`**（防无限递归）。**并发**：同一轮里与它相邻的 `Agent` / fork 技能合成一批并发（上限 `SUBAGENT_PARALLELISM = 3`，A14）—— 它**不进只读并行白名单**，并发走的是另一条腿。见下「子代理」 |
| `EnterPlanMode` | `reason`（可选：给用户看的一句话） | 进入**计划相位**：只把 `Ctx.plan_phase` 置真 + 广播 `system/plan_mode`（`state:"on"`），**没有任何本机副作用** ⇒ **免审批**、串行。此后写类工具（内置四件 + `Agent` + fork 技能 + `Remember` + MCP 工具）一律以 `is_error=true` 拒绝。见下「计划模式闭环」 |
| `ExitPlanMode` | `plan`（**整份计划正文**，markdown） | 计划相位的**唯一出口**：走 `can_use_tool` 审批卡（正文 = `plan` 原样铺开），批准 ⇒ 广播 `state:"off"` 并解除相位（写类恢复）；拒绝 ⇒ 相位**保持为真**（前端回一句「你仍在计划模式」的专用拒因）。**只读档直接拒绝**（批准了也执行不了）。见下「计划模式闭环」 |
| `Remember`（**条件注册**） | `content`（一条自包含的事实）/ `replace`（可选：整体替换） | 写**跨会话长期记忆**（`<exe 根>\ModuleData\memory\MEMORY.md`，走桥的 `lunac/memory_write`）。**只在桥接通时注册**（没桥写不进去）、**要审批**、**必须串行**、只读档 / 计划相位以 `is_error=true` 拒绝。见下「长期记忆与后台复盘」 |

**工具权限策略**

| 条件 | 效果 |
|---|---|
| `--permission-mode plan`（前端「安全」档） | 只读：`Write`/`Edit`/`Bash`/`PowerShell` 一律以 `is_error=true` 拒绝（不弹审批）；`Read`/`Glob`/`Grep`/`WebSearch`/`WebFetch` 可用，`AskUserQuestion` 也可用，`TodoWrite` 照常可用（它只改前端面板） |
| `--permission-prompt-tool stdio` 且非 plan 档 | `Write`/`Edit`/`Bash`/`PowerShell`/`WebSearch`/`WebFetch`/`AskUserQuestion` 执行前先发 `can_use_tool` 请前端审批；前端自行判定「内置安全前缀 / 白名单自动放行」还是「弹卡片」（危险命令永远只给手动确认）。没有这个开关就不问，避免对着无人应答的通道干等。`TodoWrite` **永远不在此列**（不碰本机，问了纯属打扰） |
| `WebSearch` / `WebFetch` / `AskUserQuestion` 在 plan（只读）档 | **照常审批**（`tools::gated_in_read_only`）—— 只读档对写类工具的「不必问」豁免不适用于它们：写类工具在只读档会被直接拒绝（问了白问），而这三件在只读档是放行的。`WebSearch` 会把查询词发往外部搜索源；`WebFetch` 能把 `Read` 到的文件内容拼进 URL 带出本机；`AskUserQuestion` 的答案只能从卡片上取（不问就拿不到答案） |
| `Agent`（子代理） | 非 plan 档**先发 `can_use_tool`**（`needs_approval` 含它）—— 「派一个能写文件、能跑命令的子代理出去」这个决定本身要用户确认；**但子代理内部的每次写操作仍各自再走一次审批**，不是「一次批准、后面全放行」。plan（只读）档**直接以 `is_error=true` 拒绝**（`Agent is disabled in read-only (plan) mode`）—— 它派生的是动手能力，与写类工具同理（问了白问）。**不在 `gated_in_read_only` 里是达意的**：该表管「只读档仍要问谁」，而 Agent 在只读档根本不放行 |
| `LUNAC_WORKSPACE_LOCKED=1`（配置了工作区时 src-tauri 注入） | 文件类工具路径先做词法规范化（消 `..`），越出工作区（cwd / `--add-dir`）即拒绝 —— 含 `Read` 的越界读取。**审批通过也不放行**（这是硬边界） |
| MCP 工具（P3，见下） | 非 plan 档下**一律先发 `can_use_tool`**（handler 能跑 shell / 发 HTTP，且定义来自用户 JSON，agent 侧无权替用户判断）；plan 档压根不接入，模型看不到这些工具 |
| `Remember`（长期记忆写入侧） | 非 plan 档**先发 `can_use_tool`** —— 记忆会在用户没看见的时候被注入以后的每一次对话，副作用比「改一个文件」更持久。**plan（只读）档以 `is_error=true` 拒绝**（`Remember is disabled in plan mode (read-only)`）。**注意**：它走 `dispatch_tool` 的 `needs_bridge` 早退分支，**绕过了 `tools::run` 的只读拦截**，所以这条拒绝是**分支内自己判的**，别以为写类工具的统一拦截覆盖了它 |
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

**写入内容的凭据扫描（`Write` / `Edit`，2026-09-20，原 backlog A6）**：同一个可选字段 `analysis` 下多一个 `secrets`（实现见 [core-agent/src/content_safety.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/content_safety.rs)，落地形态见 §13.1，规则见 §11 规则 58）：

```json
{"type":"control_request","request_id":"req_…","request":{
  "subtype":"can_use_tool","tool_name":"Write","tool_use_id":"tu_2",
  "input":{"file_path":"…","content":"…"},
  "analysis":{"secrets":[{"rule":"AWS access key","line":3}]}}}
```

| 项 | 约定 |
|---|---|
| 扫什么 | `Write` 的 `content` / `Edit` 的 `new_string`。**不扫 `old_string`** —— 那是要被删掉的内容，扫它会把「正在清理凭据」的操作标成可疑，正好反了 |
| 只做一类 | **只做凭据 / 密钥泄漏**，不做代码注入 / XSS / 反序列化（正则在正常代码里做不了语义判定，必然满屏误报 ⇒ 用户学会无视它） |
| 判定不代替决策 | agent 只上报 `[{rule, line}]`，**不拒绝执行** —— 与命令分析同一条纪律（规则 21） |
| 前端强度 | 与 `dangerous` 同级但**走独立通道**：不自动放行（含「自动」档）、**不给「始终允许」**、命中项要**可见地**列在卡片正文（`agent.static_secrets_body`），不能只塞标题 tooltip |
| 模型侧 | 工具返回文本里附一句 `secret_note()`（`tools.rs`），否则模型不知道用户为什么被多问了一次 |
| 缺字段 | 旧 agent / 非写入类工具不带该字段 ⇒ 按「无命中」处理 |
| 落盘 | 命中写一条 `warn`（**只记规则名与行号**，绝不记内容原文，仍过 `mask_secrets`） |

**只读分类 / 可证只读（A10，2026-09-20）**：同一个 `analysis` 下再多一个 `readonly: bool`，供**「白名单」运行档**自动放行只读命令（实现见 [core-agent/src/bash_safety.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/bash_safety.rs) 的 `is_provably_readonly`，纪律见 §11 规则 62）：

```json
{"type":"control_request","request_id":"req_…","request":{
  "subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tu_3",
  "input":{"command":"git status"},
  "analysis":{"dangerous":[],"opaque":[],"readonly":true}}}
```

| 项 | 约定 |
|---|---|
| 谁用它 | **只有「白名单」档**。手动档连只读命令也照问；自动档本来就不问（三档语义见 [agent-ui-spec.md](file:///d:/cc/claude-code-cli-master/docs/agent-ui-spec.md) §4.2，**刻意不改**） |
| 取代了什么 | 原先前端的 `BUILTIN_SAFE_PREFIXES` **前缀表（已删除）**。前缀是**字符串匹配**，看不见重定向与管道 ⇒ `echo hi > important.txt`、`cat a.txt > b.txt` 会因为「echo / cat 是安全前缀」被自动放行，等于零询问地写文件 |
| 判据（四条全过才 true） | ① **单条**命令（`;` / `&` / `\|` / 换行一律不算 —— 保守档不逐段判定）；② **无输出重定向**（`>` / `>>` / `>& file`；`2>&1` 这类 fd 复制先摘掉再判）；③ 无包装器（`cmd /c …` / `powershell -Command …`）与命令替换（`$( … )`）；④ 命令词（+ 子命令词）落在**正向白名单**里 |
| 白名单是正向的 | `ls` / `cat` / `dir` / `echo` / `Get-ChildItem` 这类**整条即只读**的命令词；`git` / `npm` / `pip` / `cargo` 只认列出的**子命令**（`git status` 放行，`git add` / `npm run build` 不放行）；`python` / `node` 只放行 `--version` 这类。刻意**不含** `find`（`-delete` / `-exec`）/ `sort -o` / `uniq IN OUT` / `sed -i` / `awk` / `tee` / `xargs` |
| 保守优先 | **证不出来就 false**，而 false **不表示危险**，只表示「不给自动放行」—— 仍然照常弹卡。`dangerous` / `opaque` 非空的命令**一律**拿不到 `readonly:true`（`is_provably_readonly` 第一件事就是 `rep.is_clean()`） |
| 缺字段 | 旧 agent 不带该字段 ⇒ 前端按「不放行」处理（**只认显式的 `true`**，不回落到任何本地前缀表）。用户白名单（`canWhitelistCmd` + 用户列表）照旧独立生效 |
| 实测（2026-09-20） | `cargo test` core-agent **95 passed / 0 failed / 2 ignored**（新增 `readonly_classification_is_conservative`，逐条钉住放行与不放行）；真机（假端点 + 真 `agent.exe --permission-prompt-tool stdio`）：`git status` ⇒ `analysis.readonly=true`、`echo hi > out.txt` ⇒ `analysis.readonly=false` |

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

**往期会话检索（SessionSearch + 往期会话索引，2026-09-19，原 backlog A2）**：把**早就建好却一直没有调用方**的会话库 FTS5 索引接上 —— agent 既能检索用户以前聊过什么，也能在系统提示词里拿到一份「往期会话索引」窄表。

| 项 | 约定 |
|---|---|
| 工具 | 一等公民内置工具 `SessionSearch{query, limit?}`（默认 10 条、上限 30）。**免审批** —— 它只读本机自己的会话库，与 `Read` / `Grep` 同级；`plan`（只读）档同样可用 |
| 通道 | 走 MCP 桥的**两个自定义方法** `lunac/history_index` / `lunac/history_search`，**不列进 `tools/list`** |
| 为什么不用用户工具 | ① `<exe 根>\tools\*.json` 是**用户**的目录，内置能力混进去会被 Tool Editor 改坏 / 删掉；② MCP 工具在 core-agent 里**一律弹审批卡**（`needs_approval` 对 `mcp__*` 恒真）⇒「每问一句历史就打扰一次」不可接受 |
| 检索实现 | [chat_db.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/chat_db.rs) 的 `search()`：先走 FTS（`messages_trgm` 管 CJK 子串 + `messages_fts` 管英文 / 代码词），**两者都没命中再回落 `LIKE`** |
| 为什么必须留 LIKE 兜底 | FTS5 的 **trigram 分词器要求查询词 ≥3 字符**，而中文常用词大量是 2 字（「缓存」「命中」「热键」）⇒ 只走 trigram 会**静默返回 0 条**（不报错），属最难排查的那类失效。LIKE 路径的 `%` / `_` / `\` 必须转义（否则「搜 `100%`」会变成「匹配任意结尾」） |
| 索引注入 | 启动时取**一次**（`lunac/history_index`），拼进系统提示词固定段：最多 20 条会话 / 总预算 2000 字符；每行只有「相对天数 + 标题 + 消息数 + 首句」，并明确告诉模型**它并不知道内容、要去调 `SessionSearch`**；空会话（只有标题没有消息）不入索引 |
| 冻结快照（硬） | 这段**只在 agent 启动时构建一次**，进程内逐字节不变；本次会话新存的会话**只落盘**，下次重启 agent 才可见。理由：每轮重取 = 每轮 cache-miss（§11 规则 18 / 23）。日期用**相对天数** —— Rust 标准库没有时区表（项目不为此引 chrono），绝对日期只能是 UTC、会出现「本地今天 / UTC 昨天」的错位 |
| 开关 | `LUNAC_HISTORY_INDEX`（`0`/`false`/`off`/`no` 关，**默认开**）；工具也可被用户从工具黑名单禁用 —— 工具被禁用时**索引一并停注**（否则等于让模型去调一个不存在的工具，同「技能清单遇 `Skill` 被禁就不列」的纪律） |
| 桥的角色变化 | 只读（plan）档**现在也建 MCP 桥**（此前完全不建），但**不把 `mcp__*` 工具放进工具池** —— 桥要用来跑上面两个自定义方法。这个改动让「只读档查自己的历史」变得可用，而不违反「只读档不给动手工具」 |
| 实测（2026-09-19） | `固定前缀 system=2542字 tools=12个/6697字 ≈2310 tokens（1.8%）`；禁用 `SessionSearch` 后 `system=2168字 tools=11个/5860字 ≈2007 tokens（1.6%）` ⇒ 净成本 **+374 字 / +1 工具 / +303 tokens（占预算 0.24%）**。真实库端到端：6 字中文（走 trigram）与 **2 字中文（走 LIKE 兜底）** 都命中同一条会话；查不到时给「No past message matched …」正常回执而非报错；空 `query` 与未知方法都走 `result.isError`（不是 JSON-RPC error） |

**子代理（`Agent` 工具，2026-09-20，原 backlog A1）**：让模型派生一个**独立上下文**的子代理去跑一件自包含任务，主对话只收它的最终报告 —— 子代理的中间工具输出（几十条 `Read` / `Grep` 结果）**永远不进主上下文**，这是它最大的价值（省上下文 = 省 miss token）。实现在 [core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs) 的 `run_subagent()` / `run_agent_tool()`，工具定义在 [tools.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/tools.rs)。

| 项 | 约定 |
|---|---|
| 独立上下文 | 每次调用新建一份 `history`（首条 = `user(prompt)`），与主对话**零共享** —— 主对话的 `history` 一个字都不进去，子代理的中间过程也不回灌。所以 `prompt` **必须自包含**（子代理系统提示词里明确要求「缺信息就写清假设、不要提问」） |
| 非流式 | 子代理请求固定 `stream: false`，一次取回整包。理由：主对话只要结果、中间过程不进 UI，非流式省掉一整套 SSE 解析分支。代价是子代理内部无逐字流式 —— **刻意如此**。**生成参数与主循环同源**：`max_tokens = max_tokens_for(cfg.thinking.get())` 且发同一个 `thinking` 字段（2026-09-20 复查修。原先固定 `max_tokens: 4096` 且完全不发 `thinking`，两处都不对：**不发该字段 ≠ 关思考**，端点默认就是开着，而思考文本算在 `max_tokens` 里 ⇒ 报告容易被挤成空；`LUNAC_THINKING=off` 对子代理也完全失效）。复用 `cfg.thinking` 还省掉了在子代理里重走一遍 400 降级链 —— 能派子代理，说明主请求至少成功过一次 |
| 系统提示词 | `SUBAGENT_SYSTEM`（角色段）+ `env_block(cwd)` + `skills::listing()`，**在启动时拼一次**（2026-09-20 复查补）。后两块是必需的：子代理**看不到主对话的环境块**，不给就不知道自己的工作目录绝对路径（只能指望调用方在 `prompt` 里手抄 cwd）；`Skill` 在它的工具集里，而**技能清单原本只写在主提示词里** —— 不给清单等于给一串不知道有哪些钥匙的钥匙串 |
| 工具集 = 剔掉 `Agent` + `mcp__*` + `SessionSearch` | `subagent_tool_defs()` 在主循环外算一次 ⇒ 所有子代理的 `tools` 数组**逐字节一致**（多子代理之间因此能共享端点侧 prompt cache）。剔除三类各有理由：`Agent` = **防无限递归**（不能再派孙代理）；`mcp__*` 与 `SessionSearch` = **它们在子代理里必然失败**（子代理不接桥 ⇒ `MCP bridge is not connected` / 「往期会话检索不可用」，而 `mcp__*` 还会**先弹一张注定白问的审批卡**）。**`--disallowedTools` 把工具全裁掉时子代理工具集为空** —— 正确行为（用户已经禁了这些能力），不要"修" |
| 并发闸门 | ① **并发上限 `SUBAGENT_PARALLELISM = 3`**（A14，2026-09-20）：同一轮里**连续的**子代理调用合成一个 `Subagent` 批并发跑，每线程一份 `cfg.detached()`；② 轮次上限 `MAX_SUBAGENT_ROUNDS = 8`；③ token 预算 `SUBAGENT_BUDGET_TOKENS = 300_000`（用量**含 `cache_read`** —— 命中缓存的也算真实吞吐，只是单价低）。到顶/超预算**不报错**，而是回一份「已尽力」的部分报告。上限为什么是 3 而不是只读批的 4：每个子代理的预算是 30 万 token，3 个同时跑最坏就是 90 万一起烧，且它与只读批的「不同值」本身就是一道刹车。**`parallel_safe("Agent")` 仍恒假** —— 那张白名单只收「不写盘、不发请求」的纯读工具，子代理两样都干，并发走的是另一条腿（见「子代理并行批」行） |
| 防递归的实现位置 | 不在 `run_subagent` 里判空，而在 `subagent_tool_defs()` 里就把 `Agent` 裁掉 ⇒ 子代理**根本看不到**这个工具（提示词里也没有），比「看见了再拒」更干净。守门单测 `subagent_tool_defs_drops_agent_and_bridge_only_tools` 钉住「剔哪三类、留哪些、**顺序不变**」 |
| 参数校验 | `prompt` 为空 ⇒ `is_error=true` + 一段中文说明（**不静默走空任务**）；`description` 缺失回落 `subtask` |
| 不接 MCP 桥 | `run_one_tool(tctx, None, …)` 传 `None` —— 桥是**单线程 stdio 通道**，主循环还持有 `&mut Bridge`；且它一接就是全量用户工具，会给子代理一个不受控的副作用面。**工具集里因此也剔掉了依赖桥的两件**（`mcp__*` / `SessionSearch`，见上）；子代理能用的是内置工具 + 技能 |
| 审批 | `Agent` 本身在 `needs_approval` 里；子代理**内部**每次写操作**各自再走一次** `await_approval`（`ask_permission` 透传），所以是「批一次派代理」+「每次动手再批」，不是一次批准全放行。plan（只读）档在 `run_subagent` 开头直接 `Err`，`task_done` 带 `ok=false`（**2026-09-20 实测**：主对话如实回报 `[task-1] subagent failed: Agent is disabled in read-only (plan) mode`）。**A14 起审批带归属**：子代理内部发出的 `control_request` 在 `request.task_id` 上写明属于哪个子任务（主循环自己发起的调用**不写该键**）—— 否则 3 个并行子任务同时弹卡时，用户点「全部允许」分不清自己放行了谁；前端的命令合并也**不许跨子任务**（见 §11 规则 65） |
| 子代理并行批（A14，2026-09-20） | 一轮里**连续**的「要 `cfg` 的调用」合成一个 `BatchKind::Subagent` 批，按 `SUBAGENT_PARALLELISM = 3` 分块 + `thread::scope` 跑，**结果按下标回填** ⇒ 回灌顺序恒等于 `tool_use` 原顺序（与只读批同一条纪律）。三个实现要点：① **每线程一份 `cfg.detached()`**（`Cfg` 含 `Cell` ⇒ `!Sync`，`&Cfg` 过不了线程边界；`detached()` 复制的是当前**已跑通并缓存**的思考形态）；② `subagent_call(name, input, skills)` **只判定一次**，规划与执行共用（`Agent` 与 fork 技能是同一族，inline 技能不算）；③ `cfg.detached()` 建不出来时（极罕见）**退回串行**用主 `cfg` 跑完，不许把调用静默丢成空结果。**不会死锁**：审批由**独立的 stdin 线程**按 `request_id` 投递（`route_control_response`），主线程就算阻塞在 `thread::scope` 里也照样收发 |
| 单元素段不并行 | 只读批与子代理批共用同一条不变量：**批内只有一个调用就不标并行**（省一次线程 spawn，行为与串行逐字节一致）。因此「一轮里只派一个子代理」与 A14 之前完全相同 |
| 报告收口 | 回灌文本 = `[task-<N>] subagent report:\n\n<报告>`，报告按 `SUBAGENT_REPORT_CHARS = 8000` 字符裁剪（`log::truncate_chars`）；整条链路失败为 `[task-<N>] subagent failed: <原因>`（`is_error=true`）。子代理跑完却不写报告时给 `(subagent <id> finished without writing a report)` |
| task id | 只能来自**进程内存态**计数器 `TASK_SEQ: AtomicU64` → `task-1` / `task-2` …。理由：它会出现在**给模型看的**回灌文本里，短才好读；计数器每个 agent 进程从 1 起（重启即重置，属已知取舍 —— 它是「本次运行的第几个子任务」，不是全局工单号）。跨运行的归因由 `session_id` 承担（A11 2026-09-20 起已是真值，见 §11 规则 64）—— 但**不要**把 session 塞进 task id。**A14 起 id 在子代理线程内分配**（`fetch_add` 仍是原子的，唯一性不变），所以它与 `tool_use` 的先后**不保证一致** —— 「第几个报告回给谁」的真相在**回灌下标**，不在 id 上，别拿 id 当顺序用 |
| 事件流（前端据此画「子任务」分组面板） | `system/task_started`（`task_id` / `description`）→ 每次工具往返前一条 `system/task_progress`（`task_id` / `round` / `tool`）→ `system/task_done`（`task_id` / `ok` / `ms`）。前端按 `task_id` **归组**渲染成一块「子任务」面板（每个子任务一行，就地更新），状态行在并行时显示并行数、`task_done` 后**还有别的子任务在跑就继续报并行数**，全跑完才回「工作中」（2026-09-20 A5 补：此前只进不出，状态栏会停在「子任务执行中」直到模型下一轮开口；A14 补：不能有一个 `task_done` 就宣布「工作中」，那会让用户以为剩下的也结束了）。**后台复盘 fork 刻意不发这三个事件**（无人值守，发了既扰民又没有恢复时机） |
| 与主循环的衔接 | `dispatch_tool()` **拿不到 `cfg`**（子代理要发自己的 API 请求），所以在主循环的工具执行分支里对 `Agent` **特判**（`run_agent_tool(cfg, …)`），与 `run_one_tool` 并列。**`Skill` 的 fork 模式同样在这里特判**（2026-09-20 A5，`run_forked_skill(cfg, …)`）—— inline 技能**不走这条路**（它只是把 md 正文交回主循环，与 `Read` 同级）。A14 把这条特判收口成 `subagent_call()` + `run_subagent_call()` 一处判定、一处执行，串行批与并行批共用。**特判的副作用（排查时别踩）**：`Agent` 不走 `run_tool`，所以日志里**没有** `tool Agent ok (…ms, args=…)` 那一行 —— 它的调用记录是 `子代理 task-N 启动：<description>` 与 `子代理 task-N 完成（Nms，报告 N 字）`/`失败（Nms）` 这对（含耗时与报告体积）。按「子代理」搜，别按 `tool Agent` 搜；fork 技能同理按「fork 技能」搜。并行批另有自己的一行 `子代理并行批 N 条（并发上限 3）: Agent, Agent` |
| 三个调用方共用一台引擎 | `run_subagent()` + `ForkSpec` 现在有**三个**调用方：① `Agent` 工具（`emit_progress=true`、`bridge=None`、工具集 = `subagent_tool_defs()`）；② A4 后台复盘 fork（`emit_progress=false`、自带一条桥、工具集 = 读写记忆白名单）；③ **A5 fork 技能**（`emit_progress=true`、`bridge=None`、工具集 = `allowed-tools` 与 `subagent_tool_defs()` 的**交集**）。把差异收进结构体是这三个能共用的前提 —— **新增调用方时只加字段，不要在 `run_subagent` 里加 `if 调用方 == X`** |
| 构建期校验 | `cargo test` 四条守门单测：`agent_tool_is_registered_and_filterable`（在内置表里 / 能被 `--disallowedTools` 裁掉 / **总数 = 15**）、`agent_is_gated_and_serial`（`needs_approval` 真、`parallel_safe` 假、`gated_in_read_only` 假）、`plan_mode_tools_are_registered_and_gated`（A7：两件计划工具的表口径）、`subagent_tool_defs_drops_agent_and_bridge_only_tools`（剔哪几类 / 留哪些 / 顺序不变）。**A14 追加五条批切分单测**：`batches_never_span_a_writing_call`、`single_read_only_call_is_not_marked_parallel`、`consecutive_subagents_form_one_parallel_batch`（两个 `Agent` / fork 技能相邻成一批；**inline 技能不算**）、`single_subagent_is_not_marked_parallel`、`subagent_batches_break_on_either_side`（子代理批与只读批**都不许被对方跨过**，谁也不许越界重排） |
| 实测（2026-09-20，deepseek-flash，release 产物） | **首次**：一次提问 → 模型发 `tool_use(Agent)` → `task_started(task-1)` → `task_progress`（round1 `Glob` / round1 `Bash` / round2 `PowerShell` / round3 `Read`×3）→ `task_done(ms=3938, ok=true)`；主对话 FINAL 恰为 `[task-1] subagent report: … Total lines: 21`（**答案正确**，3 个各 7 行的文件）；**无孙代理**。**复查复跑**（改完上面三处之后）：让子代理自报工具名与工作目录 → 它列出 `Read / Write / Edit / Bash / PowerShell / Glob / Grep / WebSearch / WebFetch / AskUserQuestion / TodoWrite`（**11 件 = 15 件内置 − `Agent` − `SessionSearch` − 计划相位两件**；A7 之前是 13 − 2，件数不变、只是式子变了）、报出正确的绝对 cwd、正确数出 21 行；`task_done(ms=5825, ok=true)`，4 次工具调用全部 `ok`（含带 `thinking` 的非流式请求被端点接受）。plan 档拒绝亦已实测 |
| 实测（A14 并发，2026-09-20，debug 产物 + 假 Anthropic 端点） | 一轮里 `tool_use(Agent)` ×2（alpha / beta）→ `task_started` ×2（两个不同 task id）→ **第 2 个 `task_started` 早于第 1 个 `task_done`**（串行实现做不出来）→ `task_done` ×2 且 `ok=true`。端点侧另有一条**同刻在飞**的证据：两个非流式子代理请求在**同一瞬间**都被收下（`PARALLEL-OK n=2`），若串行，第 2 条永远等不到第 1 条回包。回灌保序：`REPORT-ALPHA` 在 `REPORT-BETA` 之前（= `toolu_a1` → `toolu_b1` 的 `tool_use` 原序）。审批归属：顶层 `Agent` 的 `control_request` **无** `task_id`，alpha 子代理内部 `Write` 那条**带** `task_id` 且该 id 正是它自己的 `task_started` id。脚本 `core-agent/target/hooktest/e2e-a14.ps1`，**14 条断言全过** |

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
| 抓取请求头 | 主源一律用 `Lunac/<版本>`；**抓取类兜底源必须凑齐浏览器那一套头：`User-Agent` + `Accept-Language: zh-CN` + `Accept`，百度再额外带 `Referer: https://www.baidu.com/`** —— Bing / 百度对非浏览器请求只返回降级空壳（实测数据即用浏览器 UA 取得），这是抓取结果页的必要条件，不代表身份伪装。**2026-09-29 实测：百度认的是「组合」** —— 只补 `Accept` 或只补 `Referer`，它照样回一页 **1488 字节**的 `百度安全验证`（mkdjump 跳转页，正文只有「网络不给力，请稍后重试」，**HTTP 200**，`status` 检查拦不住、也没有任何结果容器）⇒ 解析器报「疑似改版或被反爬」，看着像百度改版，其实是少带了头。两种一起带才稳定出结果（连跑 3 次各 5 条）。**判据：正文 ≈ 1488 字节 = 被反爬，去补请求头，别去改解析正则** |
| 审批 | 在 `needs_approval` 与 `gated_in_read_only` 里（查询词是外部出口），**plan 档同样弹审批**；前端工具黑名单候选名单同步补 `WebSearch` |
| 不新增依赖 | 解析全部用既有 `regex` + `serde_json`（`head_chars()` 按 UTF-8 边界截断，避免中文页面切片 panic） |
| 验证口径（2026-09-17 用户定） | **四家付费主源的「成功」路径不作为验收项** —— 预算原因拿不到可用 key，只保「请求形状 + 错误透传 + 回落」正确（已验）。**真正要守的是兜底链**（它才是「未配 key 也能搜」这句话的支撑）：`cd core-agent && cargo test fallback_scrapers -- --ignored --nocapture` —— 全仓**唯一联网**用例，故意标 `#[ignore]`（依赖外网与对方页面结构，进常规 `cargo test` 会让离线/CI 随机挂），但必须保持可一键重跑。它断言两件事：① 三级至少一级可用（等价于 `scraped_search()` 能出结果）；② **Bing RSS 必须单独活着** —— 它是链的首选，不单独钉的话「它挂了但百度还在」会被 ① 掩盖成静默降级。**2026-09-17 实测：三级全部 OK 各 5 条**（2.84s，含两次 1.1s 节流）。**对方改版后必须重跑这一条**。**2026-09-29 复跑（用户报「三家全挂」）：只有百度是结构性失效**（1488 字节验证页，见上「抓取请求头」一行），Bing 两家只是**偶发**（连续跑三次里有一两次某一家因限流/连接失败，属正常抖动）；补齐请求头后三级各 5 条、可复现 |

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
| 白名单 | `tools::parallel_safe(name)`：`Read` / `Glob` / `Grep` / `WebSearch` / `WebFetch` / `TodoWrite`。**写类（`Write`/`Edit`）与 `Bash`/`PowerShell` 一律不并行** —— 它们有副作用，且顺序本身就是语义。**`Skill` 已从白名单移出**（2026-09-20 A5）：它两种模式一读一写（inline 只读、fork 会派子代理发 API），而白名单**只看名字**，按最坏的那种算 |
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
| 瘦身 | 实测 > 预算 × 0.85 **且**距上次压缩已再长 ≥ 预算 × 0.15（滞回） | 把较旧轮次里超 2000 字符的 `tool_result.content` 就地换成 `[elided: N chars dropped to save context]`（文件内容/Grep 结果是体积大头，价值递减），尾部 8 条不动。**只瘦身、永不丢整条消息** —— 在这个水位上丢消息等于白废一次缓存。过水位只是「可以看一眼」，**动不动手还要过位置成本模型**（省下的要多于被作废的后缀才压，见 §11 规则 23） |
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
| **子代理 / 复盘的用量必须并入（2026-09-29）** | `run_subagent`（`Agent` 工具 / fork 技能 / 后台复盘）的每轮用量**并入** `result.usage` 的四类总量，其请求明细追加进 `requests[]` 末尾。此前它只累进自己的 `spent`（预算熔断用）而**从未上报** ⇒ **平台照收钱、本地账看不见**（实测：平台同一 key 24 次请求 vs 本地 `usage-*.jsonl` 17 次）。实现是 `Cfg.sub: Arc<SubagentUsage>`：`detached()` **共享同一本账**（并行子代理批跑的是副本，各建一份就等于只在串行路径生效 —— 有守门单测 `detached_shares_the_subagent_usage_ledger`），`run_query` 成功收尾时 `take()` 取走并归零。推入 `requests[]` 的先后由线程调度决定（并发完成），平台对账按「条数 + 合计」对齐，**不依赖顺序**。**错误路径（`finish_error`）不归并** ⇒ 被放弃那一问的子代理用量会落进下一问（量级受一问的子代理预算封顶） |
| **四类总量里来自子代理的部分** | `result.usage.subagent = {input_tokens, output_tokens, cache_read_input_tokens, cache_creation_input_tokens, requests}` —— 它是**上面四类的子集**（已含在里面），只为**归因**（「这一问的钱有多少是子代理烧的」）。消费者**不得**把它再加一次；`usage-cost.ts` 的金额算式**只看四类总量**，一个字都没为此改。落盘 `UsageRecord.subagent`（`Option`：旧记录读成 `None`，`None` 不写回日志） |
| 落盘 | 每次提问追加一行到 `<exe 根>\ModuleData\usage\usage-YYYY-MM-DD.jsonl`（只追加不重写、按天分片），字段 `{ts, model, sessionId?, input, output, cacheRead, cacheCreate, elided, dropped, requests?, subagent?}`；`ts` 为本地时钟 epoch 毫秒，`model` 取自 `system/init`。`sessionId` = 产生这一行的 **agent 运行**（A11，见 §3.5「会话 id 与 rewind」）；**空值不写该键**，旧记录读成空串（`serde(default)`）—— 对账口径仍是 `ts` + `model`，它只做归因 |
| 读写命令 | [storage.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/storage.rs) `append_usage_log(date, record)` / `read_usage_log(date)` / `read_usage_range(dates)`（后者一次读多天并汇总成「按天 + 按模型」，成本面板用）；`date` 只接受严格 `YYYY-MM-DD`（文件名来自前端，必须挡路径拼串）；读取时单行损坏只跳过该行 |
| 表盘数值 | token 仪表盘 = **当前这次对话**（新建会话 / 切到别的会话即归零），提问结束实时累加；按天用量与金额在**表盘展开面板**里（点 `#token-dashboard`，A12，读 `read_usage_range`，见 §3.5「定价表与成本面板」）。发生过压缩时，命中率 tooltip 会追加「本对话压缩 N 次瘦身 / M 条丢弃」—— **这是解释命中率的归因口径**：压缩是「断裂型」失效，其余偏低才是「自然未命中」 |
| 计算口径 | Hit = `cacheRead`；Miss = `input + cacheCreate`（Anthropic 的 `input_tokens` **不含**缓存两项，故不能拿它减 `cache_read`）；Total = Miss + Hit + `output` |

**定价表与成本面板（A12，2026-09-20）**：把「这些提问花了多少钱」从本地用量日志算出来。**价格不写进代码** —— 各家单价差十倍以上、官方还会调价，写死一个数字等于把错误金额当事实展示。实现在 [storage.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/storage.rs)（定价表 / 候选文件 / 按天汇总）+ [usage-cost.ts](file:///d:/cc/claude-code-cli-master/app/src/usage-cost.ts)（**数据层 + 渲染层，表盘面板与设置面板共用一份**）+ [settings.ts](file:///d:/cc/claude-code-cli-master/app/src/plugins/builtin/settings.ts)（只剩按钮与候选确认）。要点：

| 项 | 约定 |
|---|---|
| **界面落点（2026-09-29 定）** | **表格与价格表数据整体在表盘展开面板里**（主界面点 `#token-dashboard` → 面板在表盘上方展开）；**设置里那一节只剩**「打开价格表 / 更新价格」两个按钮与候选价格的确认 / 放弃。理由有二：① 设置面板打开着的时候看不见对话，而这些数字恰恰要在对话**进行中**看；② 面板里的东西是**数据**（价格表时间 / 未定价模型 / 候选文件路径 + 逐日表格），**不是说明文字** —— 用户原话「只是移入数据，并不是移入说明」，别把解释性文案再塞回去 |
| **算法只许一份** | 金额、合计、格式化的唯一实现在 `usage-cost.ts`（`dayCost` / `sumUsageCost` / `fmtMoney` / `renderUsageCostTable`）。**两处宿主都 import 它，不许各写一份** —— 同预检 #39 ⑩：同一个值只写一处，否则两边会慢慢漂移成两个数 |
| 价格表 | `<exe 根>\config\pricing.json`，**用户可编辑**（与 `ai.json` / `hooks.json` 同级）。单位**元 / 百万 token**，四类分别计价：`input`（未命中缓存的输入）/ `cache_read`（命中缓存）/ `cache_write`（缓存写入）/ `output`（输出）—— 字段名与用量日志的四类 token **一一对应** |
| **时段价（分时价，2026-09-29）** | 模型条目可带 `time_windows`：数组，每条 `{days?, from, to, input, cache_read, cache_write, output}`。语义 = **基础四类价是缺省价（谷价），命中的那一条覆盖它**；`days` 省略 = 每天，给了就是 **ISO 周几**（1=周一 … 7=周日）；`from`/`to` 是**本地** `HH:MM`，区间 `[from, to)`，**`from` 必须早于 `to`**（跨午夜拆两条）。**列表里第一条命中的生效**（顺序即优先级）。写法与星期都必须严格：`9:00` / `days: []` / `days: [1,1]` / `from >= to` 一律判非法（这张表是拿来对账的，宁可他当场看到报错）。**官方 DeepSeek 就用这套表达**：基础价 = 谷价，`time_windows` = 周一至周五 09:00–12:00 与 14:00–18:00 的峰价（峰 = 谷 × 2） |
| 出处必须可核 | 每个模型可带 `source_url` / `updated_at`；顶层 `updated_at` 由**宿主**在落盘那一刻盖上（表示「这份文件什么时候写进去的」，**不是**「官方什么时候调的价」）。**2026-09-29 起改为「预置但有据可核」**（用户批准，见规则 63 第一条）：`ensure_pricing_file` 在**文件不存在**时落一份 `DEFAULT_PRICING_JSON`（`deepseek-v4-flash` 的官方峰谷价，带 `source_url`），**只在文件不存在时写**，用户改过的一个字都不覆盖；没有实测依据的模型（如 pro）**不预置** —— 宁可让面板显示「未定价」 |
| 校验 | `validate_pricing_text`：顶层是对象 / `models` 是对象 / 每个模型的四类价格**齐全且为非负数字** / `time_windows`（可选）是数组且每条同样四类齐全、时段与星期写法合法。**缺字段也算非法**：「少一个字段」在面板上的表现是金额悄悄少算一块，比当场报错难查得多。未知字段忽略（允许用户自己加注释字段） |
| 「更新价格」 | 面板自己**不抓**（它既没有网络也没有模型），把任务交给 agent：注入一条提示词让它用 `WebSearch` / `WebFetch` 查官方定价页，再用 `Write` 落**候选文件**；抓不到就如实报错并保留原值。安全档位为「只读」时按钮直接说明原因（`write_blocked` 会拦住 Write）。**提示词里已写明**：官方页有峰谷 / 分时价时照现有 `config\pricing.json` 的 `time_windows` 写法补上（基础四类价填谷价） |
| 候选文件 | = **agent 工作目录**下的 `lunac-pricing.pending.json`（`effective_workdir()`，与 `start_cli_process` 同一套判据）。**刻意不放 `config\`**：配了工作区时 agent 的文件工具被硬锁在工作区内（`tools::guard()`，越界直接拒、连审批卡都没有），写 `config\` 必然失败。面板列出「旧值 → 新值」（只列变化的 + 新增 + 「确认后失去价格」+ **「时段价已变更（旧条数 → 新条数）」**）——**确认前一个字都不动正式价格**。时段价那一行是必需的：只改时段价的候选若不显示，预览会变成「无变化」而用户以为点确认没影响 |
| 确认 / 放弃 | `commit_pricing_pending(workdir, today)`：**先校验后覆盖**，校验不过**一个字都不写**；成功后盖顶层 `updated_at` 并删候选文件。「放弃」只删候选文件。写文件本身由用户点按钮触发，不经 agent |
| 汇总粒度 | `read_usage_range(dates, utc_offset_minutes)` 一次读多天，返回**按天 + 按模型**（`UsageDay{date,turns,input,output,cacheRead,cacheCreate,models[]}`），每个模型再带一份**按本地小时**的同一批量（`UsageModelTotals.hours[] = {hour,turns,input,output,cacheRead,cacheCreate}`，`hour` 是 0–23，只含非零桶、升序）。**必须分模型**：一天里换过模型的话，只按天合计就把两模型的量混在一起了（单价差十倍），算出来的钱没有意义。**分时价还要求分小时**：桶是**同一批 token 再切一刀**（逐桶之和恒等于总量），不做插值 —— 一次提问整条记在它 `ts` 所属的那个小时里。`utc_offset_minutes` = 本地时区偏移（东八区 **480**，前端传 `-new Date().getTimezoneOffset()`）：`ts` 是 UTC epoch，**Rust 侧没有时区库**，本地墙钟只能由调用方给。没有任何记录的天不返回 |
| 金额计算 | 在**前端**算（价格表是用户随时会改的，改完即时重算，不必再跑一趟 IPC）。**有 `hours` 桶就逐桶算**：对每个桶取 `priceAt(价格表, 模型, 该天的星期, 桶的小时)`（基础价 + 第一条命中的时段价），再 `(input*p.input + cacheRead*p.cache_read + cacheCreate*p.cache_write + output*p.output) / 1e6` 累加；`hours` 缺失（旧宿主）才退回「总量 × 基础价」。**桶的粒度是整点**：用该小时的起点去比时段，边界都在整点时无损，写成半点最多让一小时的量按基础价算（偏保守）。星期由 `weekdayOf(day.date)` 从日期串算，与桶同属本地时区 |
| **总计行** | 逐日表格的 `<tfoot>` 里给一行 `settings.cost_total`（问数 / 输入 / 命中 / 写入 / 输出 / 金额），与逐日行**同一套列**、同一套 `fmtTokenCount` / `amountText` 格式化。它由 `sumUsageCost()` 一次算出（**不是**在渲染时把已渲染的字符串再加一遍）。新的天数在上（先看最近几天） |
| 未定价 | 没价格的模型**只标「未定价」并单列，绝不当 0 计**；此时金额前缀 `≥` —— 面板显示的必须是「已知部分的合计」，不是假装准确的总数。历史记录里模型名为空（见下条）同样落进这一档，但**列名字面必须与「值缺失」区分开**：空模型名显示成 `settings.cost_model_unknown`（「未记录模型名」），**不许**走 `modelLabel` 而渲染成 `—` —— 那会写成「未定价：—」，读起来正好是反的 |
| 区间 | 默认近 **30** 天。日期列表由前端按**本地日期**算好传给宿主（`YYYY-MM-DD`，与 `append_usage_log` 同一套：Rust 侧没有 chrono） |
| `model` 字段 | 用量日志的 `model` 取自 `system/init`（见上表）。**2026-09-20 之前它一直是空串**：前端声明了 `agentModel` 却没在 init 分支赋值 ⇒ 历史记录没有模型名（面板显示「未记录模型名」并计入未定价），本次一并修好 —— 分模型计价是这个面板成立的前提 |
| 实测（2026-09-20） | 宿主侧：`cargo test`（`usage_range_groups_by_day_and_model` / `pricing_candidate_is_validated_before_commit`）+ 拿**真实日志**跑一次 `read_usage_range`（2026-09-17 / 09-18：按天分片、四类 token 汇总与分组均正确）。agent 侧真机（`core-agent\target\hooktest\e2e-a12.ps1`，5 条断言全过）：工作区锁开着时，写**工作目录内**的候选文件成功（按 JSON 读回，`input=2`），写**工作目录外**被拒（`Access denied: … outside the workspace`）—— 候选文件放工作目录的理由由实测坐实 |
| 实测（2026-09-29，表盘展开面板） | `npx tsc --noEmit` = 0；dev 实例（9222）CDP 探针在**干净 reload 后**实测主窗：点 `#token-dashboard` → `#token-usage-panel` 的 `hidden` 摘掉、`#token-dashboard` 加 `expanded`、`panel.style.bottom = 34px`（状态栏实测高 28 + 6），面板里 `table.cost-table` + `tr.cost-total-row` 都在、5 个逐日行，总计行文案 `合计 73 2.4k 2.39M 0 80.4k ≥ ¥0.35`；再点一次收起（`hidden` 回来、`expanded` 摘掉）。**探针必须在 reload 之后跑**：HMR 重跑 `main.ts` 会叠加 `#token-dashboard` 的 click 监听器，一次点击被 toggle 两次 ⇒ 面板看着「没打开」（dev-only 假象，不是代码问题） |
| **对账基线（2026-09-29）** | 工具 = [reconcile-usage.ps1](file:///d:/cc/claude-code-cli-master/scripts/reconcile-usage.ps1)（唯一落点，别再往别处抄一份对账算法）。它把本地 `usage-*.jsonl` 按**本地小时**（`ts` + `-UtcOffsetMinutes`，默认 **480**）聚合四类 token，再用 `pricing.json` **逐桶**计价（`priceAt` 口径：基础价 + 第一条命中的时段价），打印逐小时表 + 合计；给 `-Csv <平台导出>` 时做逐小时对照 —— 表头**模糊识别**（时间列与金额列**必需**，四类 token 列能认多少认多少），**认不出就打印实际表头并 `exit 2`**（不许静默当空数据），金额差绝对值 > `0.0001` 元标 `DIFF`（`exit 1`）。**本地账的精度上限**：一行只带**开始时刻** `ts`，跨小时边界的提问会被整条算进开始时那一小时（≤1 小时归属偏移）；脚本会把这句话印在表下面，不要把它当成分钟级证据。**已实测**：`2026-09-29` 的 14:00 桶 = **0.07521104 元**，与手工核算（`8402*2 + 66176*0.04 + 6970*8`）逐位一致；合成 CSV 对照 0 差异（`exit 0`）、改掉一行 0.01 元 ⇒ 一行 `DIFF`（`exit 1`）、乱写表头 ⇒ `exit 2`。**跨源基线（本地 vs 平台）待用户提供平台导出的 CSV** —— 本仓不存供应商账单 |

**会话 id 与 rewind（A11，2026-09-20）**：`session_id` 从恒为 `""` 改成**真值**，回退点从「用户轮」扩到**任意消息**。规则清单见 §11 规则 64，这里是形态与实测。

| 项 | 约定 |
|---|---|
| 生成 | agent 进程内 `session_id()`（[main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs)）：`OnceLock` 惰性生成一次，形态 `sess_<pid>_<启动时刻 epoch 毫秒>`。**不引 chrono / uuid**（前者为一行时间戳不值一个依赖，后者要的语义是「这次运行」，不是全局唯一标识） |
| 语义 | **一次 agent 运行**，不是一段对话（换模型 / 换思考档 / 换工作区 / 回退取消流式都会重启进程 ⇒ 新 id）。它**不接管**上下文：历史仍由前端 `chatHistory` 经 `set_history` 灌（规则 30） |
| 覆盖位置 | **7 处 JSON 字段**：`system/init`、成功 `result`、`finish_error` 的 `result`、启动期错误的 `result`、`hook_blocked` 的 `result`、`hook_tool_payload`、`fire_plain_hook`；**另有 1 行启动日志** (`[agent] P1–P4 就绪 session=…`)。**任务事件（`task_started` 等）不带 session** —— 那边是短计数 `task-N`（规则 54 约束②） |
| 前端用途 | `main.ts` 在 `system/init` 分支连同 `model` 一起记下，随每次提问的用量写进 `usage-*.jsonl` 的 `sessionId`（见 §3.5「用量与对账」）。**界面刻意不显示它** —— 它是排查用的归因标签，不是给用户看的状态 |
| 回退点 | 任意消息：用户气泡、助手气泡、实时回合页脚（`.turn-rollback`）都挂回退按钮；**重试**仍只对用户提问成立 |
| 回退的边界 | **只回退对话与 agent 上下文**（`set_history`），磁盘上 agent 已改动的文件**不还原**（没有、也不做文件内容历史快照）。**不可撤销**：裁剪后的会话立刻全删全插写回 `chat.db` |
| 下标的换算 | `data-idx` 是渲染那一刻的下标，而 `pruneContext()` 每回合从队首丢消息 ⇒ 必须用 `shiftRenderedMsgIdx(dropped)` 把已渲染气泡的下标一起前移，并把被丢掉的气泡（新下标 < 0）的回退 / 重试按钮摘掉。**此前没有这层换算**：长对话里回退与复制都会指错消息（`copyMsgText` 取 `chatHistory[idx]` 的原文） |
| 实测（2026-09-20） | agent 侧真机 `core-agent\target\hooktest\e2e-a11.ps1`（**6 条断言全过**）：`system/init` 里的 id 形态合法、**pid 段 = 本次进程 pid**、毫秒段是可信时间戳、`result` 行同一 id（一次运行一个值）、stderr 启动行同一 id、stdout 里**没有** `"session_id":""`。宿主侧单测 `usage_record_session_id_is_backward_compatible`（旧行读成空串 / 空值不写回） |

**P3 已完成（2026-09，MCP 工具桥）**：实现在 [core-agent/src/mcp.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/mcp.rs)。src-tauri 在 spawn 时把 lunac.exe 自己的路径交过来（`--mcp-server stdio:<路径>`），agent **作 client** 把该 exe 以 `--mcp-server` 拉起 —— 那个进程会拦截该参数、进 stdio MCP server 模式（实现在 [mcp_server.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/mcp_server.rs)），读 `<exe 根>\tools\*.json` 的用户自定义工具（handler 有 `shell` / `http` / `builtin` 三种）。

握手：`initialize` → `notifications/initialized` → `tools/list`（超时 15s）→ 模型调用时 `tools/call`（超时 180s）。要点：

| 项 | 约定 |
|---|---|
| 命名 | 一律 `mcp__<原名>`（前端审批卡的「始终允许」按完整名字记入 localStorage 白名单，**前缀必须稳定**）；原名含非字母数字/`-`/`_` 或超 64 字符时替换/截断为 `_`，重名追加 `_2` |
| 接入范围 | 非 plan 档启动时接一次；`--disallowedTools` 里出现原名或带前缀名即不接入；plan（只读）档**不连接**（接进来只会每次被拒，还让工具清单随档位漂移） |
| 审批 | MCP 工具一律先发 `can_use_tool`（见上表）；deny → `is_error=true` 的 `tool_result` |
| 顺序 | `tools/list` 的返回顺序不保证稳定，接入时**按工具名排序后再入请求体** —— tools 数组属于请求前缀，顺序一变端点侧的前缀缓存整段失效 |
| 错误 | 上游 `isError=true`、进程退出、超时都转成 `is_error=true` 的 `tool_result`，不中断整轮；结果与内置工具走**同一预算出口**（超 12000 字符落盘、只内联头尾，见 §3.5「单条工具输出预算」） |
| 容错 | spawn/握手失败只往 stderr 记一行并继续 —— 十五件内置工具必须照常可用；MCP server 的 stderr 直接并入 agent stderr（上游会转发到前端/终端），stdout 独占给 JSON-RPC |
| 生命周期 | agent.exe 退出时 kill 子进程（`Drop for Bridge`）；新增/改动 `tools\*.json` 后需重启 agent.exe 才生效（与工具黑名单同一套重启流程） |

**MCP resources 读侧（A3，2026-09-20）**：`resources/list` 把 `<exe 根>\tools\*.json` 报成 resource，`resources/read` 把其中一个**原样读回来**。为什么要读侧：模型能从 `mcp__*` 的 schema 知道用户工具的名字与入参，但**看不到 `handler`**（到底跑哪条命令 / 打哪个 HTTP 端点），而 `tools\` 通常在**工作区之外** ⇒ 内置 `Read` 会被工作区锁直接拒掉。要点：

| 项 | 约定 |
|---|---|
| 两件工具 | `ListMcpResourcesTool`（无入参）/ `ReadMcpResourceTool{uri}`。**条件注册** —— 只在桥真的接上了用户工具（`!bridge.defs().is_empty()`）时才追加进 `tool_defs`；`--disallowedTools` 仍可逐件裁掉。见 §11 规则 55 |
| 为什么要条件化 | 出厂时 `<exe 根>\tools\` 只有 README 与 `*.example`，**一个可加载的工具都没有** ⇒ `resources/list` 恒为空表。无条件注册 = 在每一次请求的**固定前缀**里放两件永远查不到东西的占位工具（§11 规则 18 ⑤ 的「空转项」）。判据用 `b.defs()` 而不另发一次 `resources/list`：两者同源，且 `defs()` 握手时就拿在手里（少一次启动 RPC） |
| 回包形状 | 服务端严格按规范：`resources/list` → `{resources:[{uri,name,mimeType}]}`，`resources/read` → `{contents:[{uri,mimeType,text}]}`。**与 `tools/call` 的 `content` 不同** ⇒ 客户端不走 `result_text`，由 `Bridge::list_resources()` / `read_resource()` 各自解析并渲染成紧凑文本（uri 原样给出，模型要拿它当参数） |
| 错误通道 | `resources/*` 是**标准 MCP 方法**（不是工具调用）⇒ 出错走 **JSON-RPC `error`**（`-32002`），由 `Bridge::request()` 转成 `Err`，最终仍是 `is_error=true` 的 `tool_result`。工具调用那套 `result + isError` 不适用于它 |
| **安全边界（硬）** | `uri` 是**模型**给的，而模型会被读到的文件内容提示注入 ⇒ 只允许读 **`tools\` 目录之内的 `.json` 文件**，其余一律拒。判据是 `canonicalize()` 之后比前缀（`..` 与符号链接都会被展开），**不是字符串检查**；单文件上限 512 KB。守门单测 `resources_read_is_confined_to_the_tools_dir` 覆盖越界 / `..` / 非 .json / 空串 / 非 `file://` |
| 接受三种写法 | `file:///C:/…/tools/foo.json`（我们给出的原始形状）、`foo.json`、`foo`（后两种在 `tools\` 下解析）。裸名字含路径分隔符即拒 —— 这是为了不把「相对路径拼接」重新变回一条绕过边界检查的缝 |
| 权限三口径 | **不审批**（只读本机自己的文件，与 `Read`/`Grep` 同级）、**plan（只读）档放行**（同 `SessionSearch`）、**必须串行**（走单线程 stdio 桥，**判据是「要不要走桥」而非「是不是只读」**）。三者都在 `tools.rs` 的 `BRIDGE_TOOLS` / `needs_bridge()` 一处定义 |
| 子代理里不可用 | 两件都在 `tools::BRIDGE_TOOLS` 里 ⇒ `subagent_tool_defs()` 会把它们剔掉（子代理不接桥，留着就是保证失败），与 `SessionSearch` 同一处理 |
| 实测（2026-09-20） | **服务端直驱**（无模型，向 `lunac.exe --mcp-server` 连打 6 条 JSON-RPC）：`resources/list` 列出 `tools\deploy.json`；`file:///C:/Windows/win.ini` → `-32002` 拒绝；`../deploy` → 拒绝；裸名 `deploy` → 读回 JSON 正文；缺 `uri` → 拒绝。**模型侧**（release `agent.exe` + 真桥，`tools\` 里放一个 `deploy.json`）：`system/init` 的 tools 里出现 `ListMcpResourcesTool` / `ReadMcpResourceTool`，模型依次调用两者（`ok (1ms, out 193 chars)` / `ok (22ms, out 589 chars)`）并**逐字报出 handler 的 `echo deploying {version}`**（3 次请求，read=9344）。**条件注册反证**：把该文件改名后重跑，`system/init` 里两件工具**都不在**（也不再有 `mcp__*`） |

**长期记忆与后台复盘 fork（A4，2026-09-20）**：给「记忆写入」一个**触发点**。取回侧（`SessionSearch` + 往期会话索引）2026-09-19 就通了，但一直**只有取回、没有写回** —— 用户每次都要重新交代一遍相同的偏好与约定。要点：

| 项 | 约定 |
|---|---|
| 落点与形态 | `<exe 根>\ModuleData\memory\MEMORY.md`（与 `history\chat.db` 同在 ModuleData 下），一行一条 bullet。**与会话库不是一类东西**：`chat.db` 是**原始流水**（全量、按会话、FTS5 检索，靠 `SessionSearch` 现查），`MEMORY.md` 是从流水里**提炼出来的少量结论**（用户偏好 / 项目约定 / 踩过的坑）。流水里没有「哪句值得留」这个判断 —— 那正是复盘 fork 的职责。选文本而非再开一张 DB 表的现实理由：它要进**系统提示词的固定前缀**（必须是一段稳定纯文本）、用户要能直接读改（Hermes 同款分层）、写入只是「追加 + 上限」的小文件 |
| 通道 | 桥上的两个**自定义方法** `lunac/memory_read` / `lunac/memory_write`，**不列进 `tools/list`**（同 `lunac/history_*`：不占用户工具面、不吃审批卡、用户改不坏）。错误走 `result + isError`（「这次没写成」），**不是** JSON-RPC error |
| 硬闸（服务端） | 单条 ≤2000 字符、整文件 ≤6000 字符（≈1500 token，占 128k 预算 1.2%）。**按整条条目去重**（复盘每 N 轮跑一次，极易重复写同一条事实）；同一条重写回 `(already remembered — nothing changed)`。**超限报错、不静默截断**（静默截断会让模型以为记住了）。`replace: true` 是**整体替换**，用于「记忆已满时整理合并」——没有它，记忆满了就永远写不进去了 |
| 注入（冻结快照） | 启动时读**一次**，拼进系统提示词固定段。与往期会话索引同纪律（规则 18 / 53）：会话中新写的记忆**只落盘**，本次进程的提示词逐字节不变，**下次重启 agent 才可见**。禁止「保存记忆后就刷新提示词」——那会让每轮都 cache-miss |
| 停注条件（硬） | 无桥 / `LUNAC_MEMORY=0` / `Remember` 被 `--disallowedTools` 裁掉 ⇒ **不注入**，且注入文案里那句「用 `Remember` 追加」也随之换成「本会话不可写」。提示词让模型去调一个不在工具表里的工具，是自相矛盾的组合 |
| 写入侧工具 | `Remember{content, replace?}`：**条件注册**（只在桥接通时进请求体）、`needs_approval` 为真、`parallel_safe` 为假（走单线程 stdio 桥）、plan 档拒绝、进 `tools::BRIDGE_TOOLS`（于是子代理天然拿不到它） |
| 触发点（轮次门槛） | 每完成 `LUNAC_NUDGE_INTERVAL` 次**用户提问**（默认 10，`0` = 关）派一次后台复盘。**按提问数而不是工具轮次**：一次提问内部可以有 16 轮工具往返，按那个计数会在一次长提问中途触发，而那时复盘看到的还是半截对话。**不引定时器**（backlog 硬约束）：定时器会在用户什么都没干的时候空转，每次都是真花钱，而「跑不跑」与「这段时间有没有值得留的东西」无关 |
| 「后台」= 提问之间 | `run_query` 返回（答案已交付前端）之后才 `thread::spawn`，**处理下一条消息之前收掉已经跑完的那次**（还在跑就让它继续，见下一条）。所以它**不占任何一次回答的时延** —— 用户读答案 / 打字的那几秒正好跑完。**不是**随时并发：两条线程同时 `emit` 会交错（agent 的 stdout 是逐行 JSON 契约） |
| 不阻塞前台（硬） | 收复盘只收**已跑完**的（`JoinHandle::is_finished()`）：`REQUEST_TIMEOUT_SECS` 是 30 分钟，无条件 `join` 会让一次卡住的复盘把用户的**下一次提问**一起卡住 —— 复盘是后台事务，不该决定前台时延。它也不与主对话共享状态（`ReviewJob` 全是克隆出来的），所以放着它自己跑完是安全的。退出时同样只收已完成的（不阻塞退出）。**不会**出现两条复盘并发：`pending` 非空就不再派。 |
| 复盘的工具集（白名单） | **从本轮工具池里挑子集** ⇒ 天然继承 `--disallowedTools` 与两处条件注册：`Read` / `Glob` / `Grep`（查证）+ `Write` / `Edit`（改技能）+ `Skill`（读技能正文）+ `Remember`（写记忆）。**`Bash` / `PowerShell` / `WebFetch` / `WebSearch` / `Agent` / MCP 工具一律不在**——这是一个**无人值守**的进程，工具面必须最小；`Agent` 不在名单里也顺带防了递归。上限 `MAX_REVIEW_ROUNDS=4` / `REVIEW_BUDGET_TOKENS=12 万` |
| 复盘的提示词 | 系统提示词 = `REVIEW_SYSTEM` + `env_block` + 技能清单（**跨次逐字节相同**）；**当前记忆与对话快照进用户消息**（记忆每次都不同，放系统提示词会废掉前缀一致性）。快照 = `角色: 文本` 若干行（工具调用只留一行摘要、结果只留 300 字），超预算**从最早处**丢。提示词里给了明确的出口：没有值得记的就回 `NOTHING_TO_REMEMBER` 且不调任何工具 |
| 审批随前端 | 复盘的写操作走**同一条** `can_use_tool` 通道 ⇒ 运行方式自动档 = 静默放行（前端 `classifyRequest` 对非命令类工具按档位判定），手动 / 白名单档 = 弹卡。**不给后台进程另开一条审批旁路** |
| 桥的归属（重要） | 复盘在**另一条线程**上跑，而主循环那条 `&mut Bridge` 借不出去 ⇒ 复盘**自己连一条桥**（`Bridge::connect(spec, &[])`，只为 `lunac/memory_write` 一个方法）。同样原因 `Cfg` 也取独立副本（`Cfg::detached()`：同端点 / 凭据 / 模型 + **当前已跑通并缓存的 thinking 形态**，两个 `Cell` 归零） |
| 不打扰主对话 | 复盘**不发** `task_started` / `task_progress` / `task_done`（前端收到 `task_progress` 只会把状态栏改成「子任务运行中」且没有恢复时机），结论**只进 `log::info`** —— 它的产物是**记忆文件本身**。前端在 `idle` 下收到 `control_request` 时**不推状态机**（`idle → approval` 本就是非法迁移），卡片照常显示 |
| 技能目录进 `add_dirs` | 白名单里那两件写工具要能改**技能**（`<exe 根>\skills`），而技能目录在**工作区之外** —— 不进可访问范围，工作区锁会直接拒（不是弹审批，是拒），「改技能」就永远不会发生。与 `output_dir` 同理：进的是**应用自己的目录**，不是放宽用户的工作区边界 |
| 无桥就不跑 | `remember_on` 为假（没桥 / `LUNAC_MEMORY=0` / `Remember` 被裁）时把复盘间隔当 0 ⇒ **不派复盘**。它的唯一产品是记忆条目，写不进去还每 N 轮花一次 API 调用是纯浪费 |
| 实测（2026-09-20） | **服务端直驱**（向 `lunac.exe --mcp-server` 连打 9 条 JSON-RPC）：追加（55 字符）→ 同条重写回 `(already remembered — nothing changed)` → 追加第二条（98 字符）→ 空 `content` 拒绝 → `memory_read` 读回两条 → 未知方法拒绝 → `replace` 后文件只剩新内容（34 字节）。**模型侧端到端**（release `agent.exe` + 真桥 + `LUNAC_NUDGE_INTERVAL=2`）：`system/init` 出现 `Remember`（14 件；A7 之后同口径是 16 件 —— 那次实测在 A7 之前，件数是当时的真实数字）；第 1 问**逐字抄回**预置的记忆文件（注入可用）；第 2 问跨过门槛后日志出现 `后台复盘 review-1 启动（快照 9xx 字）` → 9 秒后两条 `tool Remember ok`（`args={"content":"The user works only in Rust."}` / `…cargo test before committing…`），记忆文件随即多出这两条（预置那条原样保留）。**全过程没有 `task_progress` 行**（`emit_progress=false` 生效）。**反证**：不传 `--mcp-server` 时启动行 `工具=[…]` 里**没有** `Remember`（13 件），且**没有**注入行 —— 记忆文件就在磁盘上，仅凭「没有桥」两件事都不发生 |

**P4 已完成（2026-09，技能 SKILL.md）；两种执行模式 2026-09-20（原 backlog A5 的 fork 一半）**：实现在 [core-agent/src/skills.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/skills.rs)。agent.exe 启动时扫 `LUNAC_SKILLS_DIR`（= `<exe 根>\skills`，见 §11 规则 3）下的 `<key>/SKILL.md`，采用**渐进披露**：系统提示词里只列 `key: 描述`（描述 ≤250 字符、清单总预算 8000 字符），模型需要时调内置 `Skill` 工具取回正文（`$ARGUMENTS` 已按调用参数替换）。要点：

| 项 | 约定 |
|---|---|
| 清单顺序 | 技能按 `key` 排序后再拼进系统提示词 —— 与 MCP 工具同理，顺序抖动等于废掉整段前缀缓存 |
| 解析 | frontmatter 与正文分离；读不出的 SKILL.md 静默跳过，不影响其余技能。**只认四个字段**：`name` / `description` / `context` / `allowed-tools`（旧 CLI 的 `model` / `effort` / `paths` / `hooks` 在 Lunac 无落点，见 §11 规则 57）。**注意区分**：这里的 `hooks` 指的是**技能 frontmatter 自带的 hooks 字段**（仍不解析）；**用户级权限 hooks 已于 A9 落地**（`config\hooks.json`，见 §3.5「权限 hooks」）—— 两者不是一回事 |
| 匹配 | 调用参数先匹配 `key`（精确）→ frontmatter.name（忽略大小写）→ `key`（忽略大小写）；三条都不过就是 `Unknown skill "X". Available: …`。**inline 与 fork 两路共用同一个 `find()`** —— 各写一份必然在「谁优先」上漂移 |
| **模式一：inline**（默认） | `Skill` 把**正文**交回主循环，模型自己在当前对话里照做。纯读（与 `Read` 同级）⇒ **不审批**、**plan 档放行**；并行上**与 fork 一起算串行**（白名单只看名字，见下行） |
| **模式二：fork**（`context: fork`） | `Skill` **不返回指令，而是派生一个子代理去执行**，主对话只收报告。判据抄旧 CLI：`frontmatter.context === 'fork'`（精确小写整词，不发明第三态） |
| 生效 | 面板安装 / 保存 / 删除后调用 `__lunac_reload_agent` 重启 agent.exe（技能目录在启动时扫描一次） |
| fork 的三种后果 | ① **要审批** —— 与 `Agent` 同理，「派一个代理出去干活」这个决定本身值得确认（判据带**入参**：`needs_approval_with()` 查 `skill` 指向的那个技能是不是 fork；**别**把 `Skill` 整件塞进 `tools::needs_approval()`，那会让 inline 技能每次都白弹卡）；② **plan 档拒绝**（`fork skills are disabled in read-only (plan) mode`）；③ **必须串行** —— `parallel_safe("Skill")` 已改 `false`（白名单只看名字，判不出模式，按最坏的那种算） |
| fork 的工具面 | frontmatter `allowed-tools:`（兼容 `[a, b]` / `a, b` / `a b`）**只在 `subagent_tool_defs()` 的结果里挑** ⇒ 技能无法借白名单把 `Agent` / 走桥的 / `mcp__*` 弄进来，判据仍只有一处。空 = 不限制；非空但一件都没匹配上 = **一件也不给**（fail-closed，写错白名单不该悄悄放开成全集），并落一行 warn 日志 |
| fork 与子代理共用同一台引擎 | 走 `run_subagent()` + `ForkSpec`（与 `Agent` 工具、A4 后台复盘**第三个调用方**共用）：任务 = 技能正文、工具集 = `allowed-tools` 交集、`emit_progress = true`（它是用户/模型主动发起的，进度必须回前端）、轮次与预算同 `MAX_SUBAGENT_ROUNDS` / `SUBAGENT_BUDGET_TOKENS`。**回灌措辞必须与 inline 区分**：inline 是「照着做的指令」，fork 是「已经做完了，这是结果」 |
| 清单里的 `[subagent]` 标记 | fork 技能在系统提示词与 `Skill` 工具描述里都标出来。标记是技能固有属性 ⇒ 逐字节稳定，不破坏前缀缓存。**漏标等于让模型把「子代理的报告」当成「要自己再执行一遍的指令」** |
| 递归 | 子代理的工具集里**有** `Skill`（技能组合很自然），但 fork 技能无法被再 fork —— `skills::run()` 对 fork 技能一律回 `cannot be loaded inline`。这是**防路由漏洞的保险**，正常路由不会走到 |
| **自带脚本 / 资源**（2026-09-20，A5 剩余项收尾） | 启动扫描时登记技能目录里**除 `SKILL.md` 之外的文件**（`collect_resources()`），**调用 `Skill` 时**附在返回里（inline 附在正文之后、fork 附进子代理的任务说明）。清单是**相对路径**（`/` 分隔、已排序），抬头写明「相对于 `<skills dir>/<key>/`」—— 模型据此用 `Read` / `Glob` 自取；执行仍走 `Bash` / `PowerShell`，照常审批。**刻意不进系统提示词**：它随用户往目录里丢文件而变，进了提示词就等于让整段前缀缓存跟着文件系统抖动（规则 18）。边界保守：深度 ≤ 3、条数 ≤ 40（超了如实写一行「还有没列出的」）、跳过隐藏项与 `node_modules` / `target`、**不跟随符号链接**（「报出去的路径一定落在技能目录内」这条保证就来自这里，不需要逐条 `canonicalize()`） |
| 未做 | **remote**（远端拉取）**结论：不移植** —— 旧 CLI 的 `remoteSkillLoader` / `remoteSkillState` 在 `feature('EXPERIMENTAL_SKILL_SEARCH')` 开关之后、**磁盘上文件已不存在**，且依赖 Lunac 没有的 `akiBackend` 服务。A5 至此**全部完成**（fork + 自带资源），条目已从 backlog 撤下 |

**计划模式闭环（A7，2026-09-20）**：把 §12「Agent Plan 模式规范」从**一段写给模型看的提示词**变成**进程内的硬状态**。旧 `cli.exe` 的 `EnterPlanMode` / `ExitPlanMode` 在 Lunac **没法照抄** —— 那边的 `plan` 是 CLI 启动参数、切档要重启 agent，而「先出计划、用户点头后再动手」是一轮对话内部的事。所以这里做的是**计划相位**（`plan_phase`）：与用户的**安全档位**（`read_only`）是两回事。要点：

| 项 | 约定 |
|---|---|
| 计划相位 vs 安全档位（**别混**） | `read_only` = **用户**在设置里选的「只读」（`--permission-mode plan`，**启动时定死**，改它要重启 agent，只读档下写类工具**永远**被拒）；`plan_phase` = **模型自己**的临时承诺（`EnterPlanMode` 置真、`ExitPlanMode` 被批准后置假，**进程内即时生效、不重启**）。两者的**拒**共用同一个出口 `tools::write_blocked()`，但**措辞必须分开** —— 用户该做的动作不同（一个去设置里改档位，一个去批准计划） |
| 状态存在哪 | `tools::Ctx.plan_phase: Arc<AtomicBool>`（**不是** `main.rs` 的局部 bool）。三条理由：① 它要和 `read_only` **在同一个地方被同一个判据读到**，否则「哪些工具算写类」这份知识会散到 `main.rs` 的五六处早退分支上；② `Ctx` 是 `Clone` 且要跨线程（只读工具并行批用 `&Ctx`、后台复盘 fork 拿克隆）⇒ 值语义的 `bool` 会各持一份、`Cell` 会破 `Sync`；③ 派生出去的子代理 / fork 技能 / 后台复盘**自动继承**计划相位 —— 正是想要的 |
| 两件工具 | `EnterPlanMode{reason?}`：**免审批**（只改一个进程内标志 + 发一条状态事件，比 `TodoWrite` 还轻）、`parallel_safe=false`；`ExitPlanMode{plan}`：**要审批**（那张卡就是它的产品）、必须串行（等人裁决，并发弹两张卡会让「批准了哪一份」无法回答）。两件都**无条件注册**进 `defs()`（计划相位是运行期翻转的，而工具表是请求体里的固定前缀，事后没法增删 ⇒ 只能靠执行侧硬拒），也都能被 `--disallowedTools` 裁掉 |
| 硬拒（不靠模型自觉） | 写类工具在计划相位里一律返回 Err：内置四件走 `tools::write_blocked()`（`tools.rs` 的 `write` / `edit` / `bash` / `powershell`）；**四个早退分支必须各自补一次**，它们绕过 `tools::run()`：`Agent`（`run_subagent`）、fork 技能（`fork_skill_allows`）、`Remember`、`mcp__*`/走桥工具（`dispatch_tool`） |
| 只读档下不许「退出计划模式」 | 只读档 + `ExitPlanMode` ⇒ **拒绝**，并说明「批准了计划也执行不了，请改用正文交代计划；要执行先去设置里把档位改成 project」。说不清这一句，模型会把用户引到一个必然失败的动作上 |
| 为什么不改档位、不重启 | 计划相位是**一次对话内部**的状态；为了让写类解锁而重启 agent 会丢掉整段上下文，代价远超收益。用户的安全档位也不许被模型改 —— 那是**用户**的边界 |
| 前端的三处落点（不新增协议字段） | ① `ExitPlanMode` 复用**已有的** `can_use_tool` 审批卡 + `updatedInput` 通道（同 `AskUserQuestion`：卡片正文 = 入参 `plan`，原样铺开、不用 markdown 渲染器、用 `textContent` 防注入）；② `EnterPlanMode` 只改 agent 状态 ⇒ 前端靠一条 `system/plan_mode`（`state: on/off` + 可选 `reason`）**镜像**它，自己**不推断**（推断要在「模型调过哪些工具」与「用户批准了没有」之间做二次判断，很容易和 agent 里的真值脱节）；③ 批准后把计划留档（见下一行） |
| 计划落盘 | `<exe 根>\ModuleData\plans\<本地时间戳>.md`（`storage::save_plan_md`，前端在批准那一刻调）。文件名时间戳由**前端**给（`YYYY-MM-DD_HHMMSS`，本地时区）+ Rust 侧严格校验形状（前端给的文件名一律不可信）—— 同 `append_usage_log`，Rust 侧没有 chrono（见 `log.rs`）。**落盘不是执行的前置**：失败只提示，绝不反过来拦住执行 |
| 拒绝路径 | 被拒时 `run_one_tool` 的 `denied` 早退**根本不会走到** `dispatch_tool` ⇒ 相位保持为真。前端随拒绝回一句**专用**拒因（i18n `agent.plan_deny_msg`）：必须说清「**你仍在计划模式**」，否则模型会以为可以接着动手，然后每个写类调用都撞一次 `write_blocked`、白烧一轮往返 |
| 不做 `VerifyPlanExecution` | 验证环节交给已有的 `TodoWrite`（见 §12 的分期结论）：为「逐条核对计划是否执行」再造一件工具，等于把 `TodoWrite` 的职责抄第二遍 |
| 子代理看不到这两件 | `subagent_tool_defs()` 把 `EnterPlanMode` / `ExitPlanMode` 一并剔掉：计划相位是**主循环**的状态，而子代理**问不了用户**（它的提示词就是这么写的）⇒ 在里面 `ExitPlanMode` 只会弹一张无人能负责的卡 |
| 实测（2026-09-20，release + deepseek-flash） | 脚本扮演 lunac.exe 直驱 stdio（`--permission-prompt-tool stdio`），11 条断言全过：`EnterPlanMode` 广播 `plan_mode state=on`（带模型自述的理由）→ `Read` 通过 → `ExitPlanMode` 走审批通道（计划正文 1197 字符）→ **拒绝** → 拒因原话进入 `tool_result` → 第 2 问要求直接动手时 `Write` 被硬拒（`Write is disabled while a plan is pending approval. Present the plan with ExitPlanMode …`）→ 再交计划并**批准** → 广播 `state=off` → 同一个 `Write` 落盘且内容正确 |

**图片附件（A8，2026-09-20）**：让模型**真的看到图**。此前附件（粘贴 / 拖拽 / 文件对话框）一律退化成 `[Attached files]` 文本里的**路径**，图片也只能靠模型自己去 `Read` —— 而那是二进制，读不出画面。要点：

| 项 | 约定 |
|---|---|
| 契约（**只传路径、不传字节**） | stdin 的 user 消息 `content` 里可再加 `{"type":"image","source":{"type":"file","path":"C:\\…\\a.png"}}`；`media_type` **不用给**（由接收方按魔术字节判定）。字节由 core-agent 按路径读出来，转成端点要的 `{"type":"image","source":{"type":"base64","media_type":…,"data":…}}`。为什么不让前端传 base64：三种来源本来就已落成路径（剪贴板图片也是先落盘再变成 chip），几 MB 的 base64 既不必过 IPC 管道，也不必在 WebView 里再存一份 |
| 文本块**照旧** | `[Attached files]` 文本仍然带上（§9 的字段登记要求「保持文本不变，旧消费者仍可读」——历史 / 标题 / 复制都只认它）。图片块是**追加**而不是替换：文本还承载「哪个路径对应哪张图」的对应关系 |
| 开关在**前端**、默认关 | 设置面板「模型支持图片输入」⇒ `config\ai.json` 的 `vision`（走 `set_ai_config` / `get_ai_config`）。**为什么默认关**：发给不支持视觉的端点（DeepSeek 官方端点）会直接 400，agent 侧无从预判 ⇒ 只能由用户显式断言。它只决定「发不发块」，**不需要重启 agent**（不改任何启动参数） |
| 类型只认魔术字节 | 只放行 PNG / JPEG / GIF / WebP（端点支持的就这四种），**不信扩展名**（改过名的文件、剪贴板落盘的 `.png` 都可能是别的东西）。BMP / TIFF / ICO 等仍走老路（路径文本交给模型） |
| 上限 | 单图原始字节 ≤ **3.5 MB**（端点的 5 MB 通常按 base64 后算，而 base64 膨胀约 4/3 ⇒ 原始字节卡在 3.5 MB 才不越线）；一条消息 ≤ **10 张** |
| 失败**如实上报** | agent 发 `system/attachment_note`（`skipped:[{path,reason}]`，**只在真有失败项时才发**），前端渲染成一条黄色 `.sys-note-warn`（i18n `agent.attachment_skipped`），正文逐条列「路径 — 原因」。没有它，用户只会看到「模型说它看不到图」而毫无线索 |
| **不检查工作区锁**（刻意的） | 附件路径来自**用户显式选中**，不是模型自己找出来的 ⇒ 读它不走工作区锁；模型的 `Read` 照旧受锁约束。理由：剪贴板图片本来就落在 `%TEMP%`（在工作区之外），套锁会让**最主要的那条用法**直接失效 |
| 历史与压缩 | 图片块**留在 `history` 里**（Anthropic 官方做法）：后续每次提问都会重发这些字节，这是视觉能力的固有代价，压缩会随消息自然衰减（瘦身档只动 `tool_result`，丢弃档整条消息一起丢）。**`set_history` 只认文本** ⇒ 回退 / 恢复会话后图片块不在（附件路径仍在文本里，模型可自行读该文件）—— 已知且刻意 |
| 只做图片，不做 PDF | 原生 `document` 块只有 Claude 系端点支持；本地提文本要引 PDF 解析库、且对扫描件无效 ⇒ PDF 仍走「把路径交给模型」 |
| 实测（2026-09-20） | `cargo test` core-agent **80 passed / 0 failed / 2 ignored**（新增 `image_blocks_are_resolved_by_path_and_limited`）、src-tauri **58 passed / 0 failed / 1 ignored**、`tsc --noEmit` 通过；**假端点实测**（本地 TcpListener 直接抓 `/v1/messages` 请求体，13 条断言全过）：真 PNG ⇒ 请求体里有 `"type":"image"` + `"media_type":"image/png"` + **与文件逐字节一致的 base64**；读不出来的那张 ⇒ 请求体里**没有**它、stdout 有且仅有一条 `attachment_note`（含路径与原因）；第 2 轮请求体里**仍带着第 1 轮那张图**（历史保留的证据），且**任何 `"type":"file"` 都不会真的发到端点** |

**与旧 cli.exe 的完整差距清单、价值评级与实施顺序见 [agent-feature-backlog.md](file:///d:/cc/claude-code-cli-master/docs/agent-feature-backlog.md)。**

**权限 hooks（A9，2026-09-20）**：让**用户自己的脚本**在 agent 的关键节点上介入（拦下一次工具调用、给模型补一句上下文、拦下整轮提问）。实现见 [core-agent/src/hooks.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/hooks.rs)（配置解析 + 事件执行，单测 13 条）、[main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs) 的 `hook_tool_gate()` / `fire_plain_hook()` / `report_hook_run()` 与 [commands.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/commands.rs) 的三个设置面板命令。要点：

| 项 | 约定 |
|---|---|
| 事件面（**只做真有落点的 8 个**） | `SessionStart`（cfg 就绪、开始收 stdin 之前）/ `UserPromptSubmit`（进 `run_query` 之前，**可拦整轮提问**）/ `PreToolUse`（**每一次**工具调用，含只读工具与子代理内部）/ `PermissionRequest`（**只在「本来就要弹审批卡」的那一刻** —— hook 可代答）/ `PostToolUse`（`run_one_tool()` 内、工具跑完之后）/ `PreCompact`（`compact_history()` 的三处调用点之前，带 `trigger: drop\|elide\|force`）/ `Stop`（`run_query` **成功**收尾、`result` 发出之后；出错收尾不发）/ `SessionEnd`（stdin 关闭、退出之前）。Claude Code 的 19 类里 `Notification` / `SubagentStop` / `TeammateIdle` 等在 Lunac **没有对应节点** ⇒ 一份都不空跑 |
| 配置（唯一真相源） | `<exe 根>\config\hooks.json`：`{"enabled":true,"hooks":{"PreToolUse":[{"matcher":"Bash\|PowerShell","hooks":[{"type":"command","command":"…","timeout":30}]}]}}`。`enabled` 缺省 **true**（文件存在本身就表示用户配了）；文件不存在 = 没配 = 关。`matcher` 是**正则**（只对三个工具类事件有意义，用在别的事件上会被当配置警告报出来并按全匹配处理）；`timeout` 秒，缺省 60、上限 600。设置面板的开关写的**就是这个字段**（不另存一份前端状态） |
| 谁读、何时读 | **agent 侧**读（宿主只在 spawn 时无条件注入 `LUNAC_HOOKS_FILE`）。**按文件 mtime 热重载** ⇒ 改完**即时生效、不必重启 agent**；解析失败**保留上一份有效配置**并落 WARN（配置写坏不该让工具链停摆），语法错在设置面板那一行同时报出来。UTF-8 BOM 两侧都容忍（编辑器常写 BOM，而 serde_json 见 BOM 直接判非法） |
| hook 的**输入**（与 Claude Code 同形，便于搬脚本） | stdin 一行 JSON：`{session_id,cwd,hook_event_name,tool_name?,tool_input?,tool_use_id?,prompt?,trigger?,tool_response?}`；`session_id` = **本次 agent 运行**的真值（A11 起不再是空串，见 §11 规则 64）。另给环境变量 `LUNAC_HOOK_EVENT`（与 `LUNAC_TOOL_NAME`）。子进程 **cwd = 工作区**，Windows 下 `CREATE_NO_WINDOW`，并发读干 stdout/stderr，超时 kill |
| hook 的**输出**（只有一套形状） | 退出码 **0** = 看 stdout：整行 JSON 对象 `{"decision":"allow"\|"deny","reason":…,"additionalContext":…}`；纯文本 stdout = 一段说明（`PostToolUse` 时**交给模型**，其余事件里只是给用户看的提示）。退出码 **2** = 拒绝（`stderr` 当拒因；不能拦的事件里它只是「把这段话交给模型/用户」）。**其余非 0 / 超时 / 输出看不懂 = 失败**：一律**不拦**，但做一条**可见**的 `error` 提示 |
| **只认显式拒绝** | 只有退出码 2 或 `{"decision":"deny"}` 才拦。超时 / 崩溃 / 坏 JSON **放行但可见** —— 「以为装了保护、其实没跑」是最危险的状态，所以宁可放过也不能静默 |
| `allow` 的边界（**等价用户白名单，不是绕过闸门**） | hook 的 `allow` 只等于**跳过审批卡**：**静态安全分析命中（危险命令 / 写入内容里的凭据）仍强制弹卡**，工作区锁与计划相位也照旧生效（判据 `hook_allow_needs_card()`，见 §11 规则 14 / 59）。`opaque`（判不定）**不**强制弹卡 —— 与前端「自动」档口径一致 |
| 多命中的次序 | 一个事件的全部命中 hook **按配置顺序全部执行**；`deny` 优先于 `allow`；**不并发**（用户脚本按顺序执行才可预期） |
| 结果去哪 | 工具类事件：`deny` ⇒ 工具不执行，拒因以 `is_error` 的 `tool_result` 回给模型；`PostToolUse` 的 `info` / 退出码 2 文本 ⇒ **拼进 `tool_result` 的文本内部**（content 数组形状与块数不变，见规则 23）。用户可见面：`system/hook_note`（见 [agent-ui-spec.md](./agent-ui-spec.md) §9） |
| 上限 | 每条提示文本 2000 字符（截断标记），单个 hook 的 stdout/stderr 各读 512 KB（读满即丢，与 `tools.rs` 的管道纪律同源） |
| 实测（2026-09-20） | `cargo test` core-agent **93 passed / 0 failed / 2 ignored**（新增 `hooks` **13 条**）、src-tauri **58 passed / 0 failed / 1 ignored**、`tsc --noEmit` exit 0；**真机端到端 18 条断言全过**（假 Anthropic 端点抓请求体 + 真 hook 子进程 + 真 Bash 工具）：拦下 ⇒ 工具未执行 / 无审批卡 / 拒因进 `tool_result`；放行 ⇒ 免卡且 `tool_result` 有 `exit code: 0`；**放行 + 危险命令 ⇒ 仍弹卡且 `analysis.dangerous` 非空**；`PostToolUse` 的 `additionalContext` ⇒ 第 2 次请求体里出现 `[PostToolUse hook]`；`UserPromptSubmit` 拦下 ⇒ 端点**零请求** + `result.subtype=hook_blocked`；退出码 7 ⇒ **不拦**（照常弹卡）且 `kind=error` 可见；`enabled:false` ⇒ 一条 hook 都不跑 |

**人格 / 自定义提示词（L2，2026-09-21）**：让用户在设置里写一段**自己的**人格 / 输出风格，与内置人格段一起进系统提示词的**固定前缀**。实现：落盘 `config\persona.md`（纯文本）+ 宿主注入 `LUNAC_PERSONA_FILE` + [core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs) 的 `read_user_persona()` / `persona_block()` / `build_system_prompt()`；前端 `get_persona` / `set_persona`。要点：

| 项 | 约定 |
|---|---|
| 落点（**唯一正确的位置**） | 系统提示词的**固定段**，位置 = **内置人格段之后、`Environment:` 之前**：`SYSTEM_PROMPT + PERSONA_AND_STYLE + [用户人格段] + env_block + skills::listing() + 索引 + 记忆`（`build_system_prompt()`）。理由：人格是**固定块**（每次都要生效、内容与请求无关）⇒ 只有进固定前缀才永远命中缓存（§11 规则 18/23）。**绝不按消息拼接**（旧形态，已废），也**绝不用 hooks 承载** —— hooks 是控制通道，它的输出今天不进模型（只有 `PostToolUse` 能拼进已有 `tool_result` 文本内部），且那样会多出一条权限面「谁能写 `hooks.json` 谁就能改系统指令」 |
| 谁读、何时读 | **agent 侧**读，**启动时读一次**（`read_user_persona()` 只在 `main()` 装配处调一次，之后进程内逐字节不变）。宿主只在 spawn 时**无条件**注入路径（与 `LUNAC_HOOKS_FILE` 同形，不判断文件在不在）。⇒ **改完必须重启 agent 才生效**，面板如实写明并给「立即重启 AI」按钮 —— 这与 hooks 的 mtime 热重载**语义相反**，两处不要混 |
| 「没配」的三种情况都等价 | 环境变量没给 / 文件不存在 / 读不出来 ⇒ 空串 ⇒ `persona_block()` 返回**空串**，提示词里**一个字节都不加**（只有第三种落一行 warn）。空文本是合法状态，面板「恢复内置」写的就是空文本（**不是**切开关、也**不是**删文件） |
| 为什么在**内置段之后** | 内置 `PERSONA_AND_STYLE` 保住产品底线约束（文风 / 禁 emoji 这类不该被用户改掉的），用户段是**追加**而非替换 |
| 长度上限 **8000 字符** | `core-agent` 的 `MAX_PERSONA_CHARS` 与宿主 `storage::MAX_PERSONA_CHARS` **两处同值**（改一处要改两处）。分工：宿主在**保存前硬校验**（超长 / 含 NUL ⇒ 拒收、一个字都不写），agent 侧读到超长**截断 + warn**（防御绕过面板直接改文件的人）。上限的理由：这段进的是**每次请求都要发的固定前缀** |
| **刻意不进**子代理与后台复盘 | `build_subagent_system()` / `build_review_system()` 只有「角色段 + 环境块 + 技能清单」—— 两者都是内部产物（子代理报告 / 复盘写记忆），用户的文风口吻在那里没有意义，而多一份文本就是**每个并发子代理**都要重发一次的固定成本。守门单测 `user_persona_reaches_only_the_main_prompt` |
| 文件位置 | `<exe 根>\config\persona.md`，与 `ai.json` / `hotkey.json` / `hooks.json` / `pricing.json` 同级（「应用配置」；业务数据才进 `ModuleData`） |
| 实测（2026-09-21，`core-agent\target\hooktest\e2e-l2.ps1`，真 `agent.exe` + 假端点，**16 条断言全过**） | 两轮对照：**Run A** 把 `LUNAC_PERSONA_FILE` 指向含 `PERSONA-MARKER-42` 的文件 ⇒ 线上 `system` 字符串里 `## Personality` < `## User-defined persona` < `Environment:` 三个下标严格递增、marker 在场，且该问两次请求的 `system` 前缀哈希**同值**（逐字节不变）；**Run B** 指向空文件 ⇒ marker / 表头都不在，且前缀哈希与**未引入 L2 时的基线完全相同** ⇒「没配人格时不加任何字节」由实测坐实（不是靠代码里那句 `if` 说服自己） |

**插件市场（L1，2026-09-21；2026-09-28 迁目录 + 加依赖）**：`<exe 根>\Modules\<id>\` 下的第三方插件**在启动时被注册进同一个 `pluginRegistry`**，于是结果区渲染、拼音匹配、`pluginIconSvg()`、i18n 的 `plugin.<id>` 全部零改动。宿主侧实现：[app/src-tauri/src/plugin_market.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/plugin_market.rs)（扫描 / 解压 / 校验 / 卸载 / **索引解析** / **依赖安装**，12 条单测）+ [commands.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/commands.rs) 的五个命令；前端适配层 [app/src/plugins/market.ts](file:///d:/cc/claude-code-cli-master/app/src/plugins/market.ts)；可下载清单由本仓 [plugins/index.json](file:///d:/cc/claude-code-cli-master/plugins/index.json) 提供。要点：

| 项 | 约定 |
|---|---|
| 包形状 | **https 的 zip**，包内（根目录或唯一一层子目录 —— GitHub 的 zip 会给一个顶层目录）必须有 `lunac-plugin.json` + **已编译好的 ESM 入口**（`entry`，默认 `index.js`）。清单字段：`id`（**同时是目录名**）/ `name` / `description` / `keywords` / `icon` / `version` / `entry` / `homepage` / **`dependencies`**（见下一条） |
| 加载通道 | `convertFileSrc(entry)` → `import(/* @vite-ignore */ url)`。**CSP 必须含 `https://asset.localhost`（script-src）**—— 2026-09-21 已在 `tauri.conf.json` 加上。两条前提经 Tauri 源码核实（不是猜的）：`.js` / `.mjs` 在 asset 协议下被标成 **`text/javascript`**（`tauri-utils/src/mime_type.rs`），asset 响应一律带 **`Access-Control-Allow-Origin: <窗口 origin>`**（`tauri/src/protocol/asset.rs`）⇒ 跨 origin 的模块加载成立。**插件代码永远不能走 CDN**（同 §3.7 的 KaTeX 缺陷是同一条约束） |
| 插件契约 | 入口**默认导出** `{ execute(input) => string \| { type, content } }`；也接受「默认导出就是函数」或「具名导出 execute」。入参是搜索栏原始文本（与内置插件一致）；`type: 'html'` 的结果会被 `innerHTML` 渲染 —— 与内置插件同路，所以**装谁 = 信任谁的代码**，界面上必须把「这是可执行代码 + 来源」写出来。**会被独立打包的内置插件，源码本身必须满足这份契约**（2026-09-28 实测踩到）：`music.ts` 原先只导出具名 `musicPlugin`，独立打包出来的包在市场里会报「没有可调用的 `execute`」—— 内置构建**看不见这个洞**（那边是 `import { musicPlugin }`，走具名符号）。判据：`vite build` 出的 `index.js` 末尾那条 `export { … }` 里必须有 `as default`（现在它是 `export default musicPlugin;`）。 |
| **磁盘插件的挂载约定**（2026-09-28） | 内置插件的 attach 是 `app/src/plugins/attach.ts` 里一张**硬编码 switch**（要 import 各家模块、传各自参数）。磁盘插件不能改宿主源码，于是改成**插件自己声明**：入口里具名导出（或默认导出对象上的）`attach(root)` / `detach()`，`attach.ts` 的 `default` 分支转交 `market.ts::externalAttach()`。挂上的钩子存在 `market.ts` 的 `hooks` 表里，面板关闭时 `externalDetach()` 收掉。**没有 `attach` 就是「不需要挂载」**（与内置表未命中同义），不是错误。**磁盘插件优先于硬编码表**：磁盘上有**可用**的一份时（`hasDiskPlugin()`，坏包不算），`attachPluginListeners` / `detachPluginListeners` 都**先走插件自己的钩子、不再回落内置表** —— 否则「市场里点了下载、跑的还是 bundle 里的旧代码」，面板显示新版本号而用户无从察觉（内置双轨期的音乐 / OCR 正是这种同一个 id 两边都在的情况）。**不回落**这一条是刻意的：用户装的那份没导出 `attach`，语义是「它不需要挂载」，不是「请用内置那份」。 |
| **插件拿宿主能力走桥**（2026-09-28） | 插件是**独立打包的 ESM**，`import` 到的宿主模块（i18n 等）是**另一份未初始化的副本** ⇒ 插件自带 i18n 会让界面全是 `music.play` 这类 key。所以宿主把自己的 `t` 挂到 `globalThis.__lunac_host`（`app/src/plugins/host.ts` 的 `installHostBridge()`，main.ts 与 plugin-window.ts 启动时各装一次），插件 `import { t } from ".../host.js"` —— 同一份源码主 bundle 里是宿主实现、插件 bundle 里读全局。**桥只加不减**（`apiVersion` 当前 1），插件侧对 `undefined` 必须有兜底。 |
| **依赖随插件装**（2026-09-28） | 清单 `dependencies[]` 由 `plugin_market::install_dependencies()` 在**插件落盘之后、同一次安装里**逐条拉好；**任一条失败 = 整次安装失败**（新版本删掉、旧版本搬回来）。两种形态：`file`（`url` 必须 https + `dest` 必须留在插件目录内 + 可选 `sha256`，**写了就必须对上**；先写 `.part` 再改名）与 `npm`（`npm install --prefix <插件目录> <包>@<版本>`，需要用户机器有 Node.js，**找不到就如实报错**）。单文件上限 512 MB、最多 32 条。**这是「release 缺 librespot / 缺 PaddleOCR」这类问题的根治办法**：依赖跟着用到它的插件走，不再由构建脚本塞进安装包（作者漏拷一次就只能等下一个版本）。**第一条真依赖（2026-09-28）**：`music` → librespot 0.8.0（`type=file`、`dest=bin/librespot.exe`、`sha256` 写死）。**上游不发 Windows 二进制**（v0.8.0 / v0.7.x 三个 release 的 `assets` 全是空数组），所以那份 exe 是我们用 `cargo install librespot --version 0.8.0 --locked` 自己构建、挂到**公开插件仓库的 Release 资产**上（`LythrumMoon/lunac-plugins` 的 `librespot-0.8.0`）—— 终端用户免鉴权直链，`release\deps\` 是本机的中转目录（不进 git）。宿主认这条落点：`music.rs::find_librespot()` 加了一条 `Modules\music\bin\librespot.exe`，用户不必自己去填路径。依赖二进制**换版本时 sha256 必须与上传的那一份逐字节一致**，对不上宿主会整包拒绝安装。 |
| **Lunac 自己写插件**（2026-09-28） | `<exe 根>\Modules\README.md`（源文件 `agent-templates\modules\README.md`，由 `build-release.ps1` 第 6 步复制、NSI 安装，**升级安装不清空该目录**）既是给用户的文档，也是给 agent 的规范。宿主把 `LUNAC_MODULES_DIR` 交给 agent.exe（commands.rs）：core-agent ① 把它推进 `Ctx.add_dirs`（工作区锁生效时也写得进去）、② 在 `env_block()` 里写明「插件放这儿、规范是同目录的 README.md、写完后让用户去 设置→插件→重新扫描」。于是**自建插件 = 写两个纯文本文件**（`lunac-plugin.json` + `index.js`），不需要 dev 版、不需要前端构建环境。 |
| 校验（比 tools / skills 先例更严，因为解压的是可执行代码） | ① **只收 https**（明文 http 的 zip 会被解压执行）；② 压缩包 ≤ 32MB、**解压后总量 ≤ 192MB**、条目 ≤ 4000（zip bomb）；③ 逐条拒绝绝对路径 / `..` / 空段 / 深度 > 16 / 以点或空格结尾的分段（路径穿越，判据收口在纯函数 `safe_join()`，有单测）；④ `id` 只允许 `[a-z0-9._-]` 且不以 `.` 开头（它直接当目录名）；⑤ `entry` 必须是相对路径且以 `.js` / `.mjs` 结尾；⑥ **同 id = 重装 / 升级**（2026-09-28 改，原为「已存在即拒绝」）：旧目录先改名成 `.old-*`，新包或依赖任一步失败就搬回来、成功才删备份 —— 一键升级与「不静默换代码」两条都要，靠回滚而不是靠拒绝；⑦ 先解到 `.staging-*` 再改名进正式目录，**失败即清理**（不留半成品） |
| 目录改名（2026-09-28） | 插件根从 `<exe 根>\plugins\` 改为 **`<exe 根>\Modules\`**（用户指定；与 `ModuleData\` 是两回事 —— 那边是业务数据）。旧目录由 `plugin_market::migrate_legacy_plugins_dir()` 在 `main()` 里**一次性搬运**（幂等；同名冲突保留 `Modules\` 那份并 warn；搬不动就留着旧目录，绝不删用户文件）。 |
| 安全边界要诚实 | 这里做的是**防事故**（写坏路径、把包塞爆、装重了），**不是防恶意** —— 插件是用户自己选择安装的可执行代码，装上即等同一份本机权限（与 `tools\` 的 shell handler 同族）。别把这条写成「已沙箱化」。**界面文案的现状（2026-09-28 按用户要求改）**：标题下那句提示改成「插件是 Lunac 专用的拓展功能模块。」——**「装它等于在本机运行它 / 只装信任来源」那句话撤下了**（同一天还删掉了表格下方那行 `settings.plugins_hint` 整行，它原本讲「已安装的直接「打开」」）。撤下的是**文案**、不是判据：来源仍必须挂在每一行的 `title` 上（`MarketRow.source` = `homepage`，没有就退回 zip 地址），声明的宿主能力（`permissions`）仍原样列出 —— 「装谁 = 信任谁的代码」这条调查路径不能断。 |
| 生效语义 | 装完 / 卸完**立即生效，不必重启 AI 也不必刷前端**：`refreshMarketPlugins()` 把旧 id `unregister` 掉再 `register` 新对象（registry 不去重，直接二次 register 会让结果区出现两行）。**升级会按 `version` 清模块缓存**（2026-09-28）：升级装的是同一个目录、同一个 `entry` 路径，不清缓存会把用户按回旧代码；卸载掉 / 已不存在的 id 也一并清（连 `attach` 钩子一起）。**手工把文件夹写进 `Modules\` 的情况**（Lunac 自建插件、作者本机调试）走设置面板的「重新扫描」按钮 —— 语义比“每次打开设置都悄悄重注册”更清楚，也不会在用户没动作时换掉正开着的插件。这与「技能改完要重启 agent」是两回事（技能是 agent 的能力、插件是前端的界面件） |
| 坏包必须可见 | 清单坏了、入口丢了的包**不进 registry**（用不了），但**必须列在面板上并写出原因** —— 否则用户只会看到插件莫名消失、手上没有任何线索。`list_installed_plugins` 因此返回 `valid` + `error` 两个字段 |
| **索引 = 「去哪下」的清单**（2026-09-21 二次改版加的；2026-09-28 换到独立公开仓库） | 面板要能列出「**没装但可以装**」的插件，而本地扫描只看得见「已经装了的」。索引与插件包都在**公开仓库 `LythrumMoon/lunac-plugins`**：`main/index.json`（顶层是**数组**，一条 = `{ id, name, description, version, url, keywords?, icon?, homepage? }`；`url` 是 https 的插件 zip 直链，形如 `https://raw.githubusercontent.com/LythrumMoon/lunac-plugins/main/packages/<id>-<ver>.zip`）。地址是 **Rust 常量** `plugin_market::INDEX_URL`，**不由前端传**。**为什么必须公开**：主仓库 `LythrumMoon/Lunac` 是私有的，而 raw/Release 对私有仓库要鉴权 ⇒ 终端用户**永远拉不到**，索引与包都是死的（2026-09-28 查出来的既有缺陷）。发布链路：`scripts\build-plugins.ps1`（vite 库模式出单文件 ESM + 生成清单 + 打 zip）→ `scripts\publish-plugins.ps1`（拷进插件仓库的 `packages\`、生成 `index.json`、commit + push）。**主仓库里的 `plugins/index.json` 已删除** —— 索引只有一份，别留第二个真相源 |
| 索引**由宿主去拉**，且**逐条再筛一遍** | 两条理由：① 前端 CSP 的 `default-src` 不含 github 域（没写 `connect-src` ⇒ 回落 default-src），前端 `fetch()` 会被直接拦掉；② 索引是**远端可改的文本**，谁写索引谁就影响了「前端能下什么」⇒ 必须按插件包的口径重新校验：`id` 过 `is_safe_id()`（它将来是目录名）、`url` 必须 `https://`、`name` 不能空。判据收口在纯函数 **`parse_index()`**（两条单测），坏条目**只丢自己**（`warn` 留痕）而不是丢整份索引 —— 这与「坏包必须可见」是两条不同的处置：那边是用户**已经装在盘上**的东西，消失了他找不到 |
| 索引的传输闸 | 与插件包同一套：**只 https**、体积上限（`MAX_INDEX_BYTES` = 1 MB，Content-Length 先拦 + 按真实读到的字节再拦）、条目数上限（`MAX_INDEX_ENTRIES` = 500）；拉不到就**只提示一行**，市场退回「只有本机插件」的形态（等于这个功能不存在时的样子），**不静默变成空表**。**⚠️ 缺陷与加固（2026-09-29，已按用户决定修）**：这条原先用 reqwest **30s** 超时，而插件包那条是 120s。同一台机器同一 URL 的实测结论是**抖动极大**：同一轮会话里索引**三次全失败**（19.2s / 19.4s / 30.0s），换个时间点再测**961ms 就成功**（拿到 1 条 `music 0.9.6`）⇒ 慢的是**建连、不是正文**（索引只有几百字节），30s 对首字节太紧，而这条一旦失败用户看到的是「市场列表只剩一行错误」—— 已**把超时提到与包体同档的 120s**（`commands.rs::fetch_plugin_index_blocking`）。**没做**的是「加直链兜底（`cdn.jsdelivr.net`）」—— 该选项被否（不值得为此多挂一条外部 CDN 依赖） |
| 界面形态（2026-09-21 二次改版；2026-09-28 加「更新 / 重新扫描 / 依赖条数」） | 插件面板**只有一段**「插件市场」，一张表一行一个插件，按钮由**本机事实**（`pluginRegistry` + 插件目录扫描）决定，**不是索引自称的**：已装且能用 ⇒ 「打开」（第三方多一个两段式确认的「卸载」；内置编译进 bundle，没有目录、没得卸）；**已装而索引里版本不同 ⇒ 多一个「更新」**（同 id 重装 = 升级，走的就是那条安装命令 —— 版本号只做「不同即视为有新版」，**不猜大小**：索引版本是作者写的自由文本）；装了但包坏了 ⇒ 原因 + 「卸载」；没装而索引里有 ⇒ 「下载」。行内还会标出**依赖条数**（「安装时会一并下载 N 项依赖」）。表尾有**「重新扫描」**按钮（给「文件已经写进 `Modules\`」用，见生效语义那条）。合并顺序**固定**：已装（registry 顺序照旧）→ 坏包 → 索引里还没装的。**下载与手工安装是同一条**宿主命令 `install_plugin_from_url` —— 索引只提供 URL，别在前端另开一条 |
| 面板取数**不在构建期** | 表格字符串（`buildPluginsPane`）是**整块设置面板**的一部分，而索引要走网络（差网络下能拖到超时）⇒ 那里一旦 `await`，**打开设置**就跟着卡住。所以列表在**挂载后**由 `renderMarket()` 填，且**先本机、后索引**两段画：本机扫描是毫秒级的，不该被一份可有可无的推荐清单拖住 |
| 事件绑定用**委托**（只在挂载时绑一次） | 这三个按钮过去是「重绘完再逐个 `addEventListener`」，而挂载时没人调那次 render ⇒ **初始渲染出来的按钮全是死的**（2026-09-21 用户报的「卸载点了没反应」就是这个根因）。改成在列表容器上委托后，重绘只改 `innerHTML`、监听永不丢 —— 这一类 bug 从此不存在。**新加的按钮一律并入这份委托，不要在重绘路径里重新绑**。「重新扫描」按钮在**列表外面**（列表整块重绘，按钮不该跟着被换掉）⇒ 它的委托挂在**设置容器**上（2026-09-28）。 |
| 模型资产（Live2D 等） | **安装包零第三方模型资产**：Lunac 只提供引擎与导入通道，模型由终端用户自备（他说下载时自己接受 Live2D 的协议）。版权四条线见 backlog **L1-B** —— 尤其：官方样例模型属 **No Redistribution**，**不得**随包分发 |
| 实测（2026-09-21） | `cargo test --bins` src-tauri **73 passed / 0 failed / 1 ignored**（其中 `plugin_market` **9 条**：越界路径被拒 / zip bomb 被拦 / 正常往返 + 同 id 拒绝 + 入口缺失拒绝且不留 staging 残渣 / 接受 GitHub 的单层顶层目录 / 坏包如实上报 / 清单与 entry 校验 / BOM 容错 / **索引逐条筛（坏 id、明文 http、空名字、重复 id 各丢自己）/ 索引非 JSON 报错且接受 BOM 与空表**；`appearance` 增 **1 条**：**随包发货的主题包逐项校验**（清单能解析 + 声明的背景 / 花纹 / 图标真在盘上；2026-09-21 删掉魅魔包后只断言 `default`，`succubus` 那两条断言一并删除））、`tsc --noEmit` exit 0、`npm run build` exit 0 |
| 实测（2026-09-28，目录改名 + 依赖 + 自建插件这一批） | `cargo test --bins plugin_market` **12 passed / 0 failed**（原 9 条 + 新增 3 条：**依赖字段逐条校验**（明文 http / dest 越界 / 缺 dest / npm 缺包名 / npm 包名带 `--` 注入 / 未知 type 各拒）/ **旧 `plugins\` 一次性搬到 `Modules\`**（同名冲突保留新的、幂等）/ **依赖 dest 越界在写盘前就被拦**）；原「同 id 拒绝」那条改为「**升级覆盖 + 入口缺失时旧版原样保留且无残渣**」；`cargo check --bins` exit 0、`npx tsc --noEmit` exit 0。界面侧的「更新 / 重新扫描 / 依赖条数」与磁盘插件的 `attach` 约定**尚未做实机回归**（本轮只做了编译与单测）。 |
| 实测（2026-09-28 晚，依赖落地的第一批：librespot） | librespot 0.8.0 用 `cargo install librespot --version 0.8.0 --locked --root release\deps\librespot` 构建成功（3m16s；`librespot.exe` 36637180 字节，`--version` 自报 `librespot 0.8.0`）；产物挂成公开仓库 `LythrumMoon/lunac-plugins` 的 Release 资产（tag `librespot-0.8.0`）—— **免鉴权** HEAD 200 + `application/octet-stream` + 36637180 字节，下载回来的 sha256 与本地 `Get-FileHash`、与 GitHub 自报的 `digest` **三者一致**。重打的 `music-0.9.6.zip`（16811 字节，sha256 `06abd4b2…38d6`）与线上下载的字节**完全相同**，`index.json` HTTP 200。同时修掉一个静默洞：**独立打包的 music 包没有可调用的 `execute`**（源码只导具名 `musicPlugin`，内置构建看不出来）⇒ 补 `export default musicPlugin;`。`npx tsc --noEmit` exit 0、`cargo check --bins` exit 0（19 条既有 warning）。**「市场里装音乐 + 磁盘插件 `attach` 生效 + 依赖真的落进 `Modules\music\bin\`」仍未做实机回归。** |
| 实测（2026-09-29，dev 实例 + CDP 探针把上面那条补上了） | 上一轮列的「仍未做实机回归」**这次全跑通了**（探针：`node` 直连 9222 的 CDP，表达式写成文件、`Runtime.evaluate` 求值；`%TEMP%` 下两个一次性文件，不进仓库）。逐条：① **依赖真的落盘** —— `install_plugin_from_url` 端到端成功（`Modules\music\` 里有 `index.js` / `lunac-plugin.json` / **`bin\librespot.exe` 36637180 字节**，sha256 `7509c74b…4c1a`，与公开 Release 资产**逐字节一致**）；② **`find_librespot()` 新增的那条兜底生效** —— 把配置里指的 exe 临时改名后，`librespot_status` 回报的 path 变成 `…\target\debug\Modules\music\bin\librespot.exe`（验完原样还原）；③ **磁盘插件优先是真的** —— 音乐窗（`plugin.html`）的资源列表里只有 `asset.localhost/…/Modules/music/index.js`，那个模块导出 `attach` / `detach` / `default.execute` 齐全；④ **面板**：11 个内置插件只有「打开」，从市场装进来的 music 那一行多出**「卸载」**（判据就是「盘上有没有 `Modules\<id>\`」—— 这也解释了「内置插件为什么没有卸载按钮」，不是 bug），行内还标出「安装时会一并下载 1 项依赖」。**新发现的缺陷（同日已修）**：`fetch_plugin_index` 那条链路**抖动极大** —— 同一台机器同一个 URL，同一轮里**三次全失败**（19.2s / 19.4s / 30.0s），而走 `install_plugin_from_url`（120s 超时）**21.3s 就成功**、包体耗时 **235 秒**；但**当天稍后复测只用 961ms 就成功**（1 条 `music 0.9.6`）⇒ 慢的是**建连、不是正文**，30s 对首字节太紧。现网表现是「市场列表偶发拉不出来（只剩一行错误），但手工下载能用」。**按用户决定已修**：索引超时从 30s 提到**与包体同档的 120s**（`commands.rs::fetch_plugin_index_blocking`）；「加 `cdn.jsdelivr.net` 直链兜底」这一选项被否（不值得多挂一条外部 CDN）。**同日第二次实机回归（用户重启 dev 实例后）**：面板三类改动逐条对上 —— ① 提示区第一行是「插件是 Lunac 专用的拓展功能模块。」，正文里搜不到「已安装的直接」（整行已删）；② 11 行内置只有「打开」，`音乐歌词 · v0.9.6` 那行带**「卸载」**；③ 音乐窗资源仍是 `asset.localhost/…/Modules/music/index.js`，导出 `attach` / `detach` / `default.execute` 齐全（磁盘优先复现）；盘上 `bin\librespot.exe` 36637180 字节、sha256 与清单声明**逐字节一致**（13:51:19 落包、13:51:30 落依赖）。**另一件事故**：一次批量重存把这 4 个脚本（`build-plugins.ps1` / `lunac-installer.nsi` / `publish-plugins.ps1` / `build-release.ps1`）各写成 **4 份 BOM**，而自检旧判据只认「第 4 字节是不是 EF」⇒ 报的是「有两份」（数字不对）—— 已按 1 份归一化（`git checkout` 恢复后确认字节数与 `Parser::ParseFile` 均正常），并把自检改成**数全部 BOM + 写明份数**（造一个 4 份 BOM 的文件做反向验证，确认报「**4 份**」并阻断）。`npx tsc --noEmit` exit 0、`npm run verify` 8/8（唯一提醒是 5173 被 dev 实例占用）。 |
| **事故与实测（2026-09-29，「OCR 插件凭空消失」）** | **症状**：用户报「好像丢失了 OCR 插件」。**根因不在代码，在发布链路漏了一步**：当天把 4 个拓展插件（`ocr` / `clipboard-history` / `convert` / `music`）从 bundle 摘出（`builtin/index.ts` 只注册 `BASE_PLUGIN_IDS`），但**市场索引还停在 2026-09-28 那一份**（只有 1 条 `music`）⇒ OCR **既不在 bundle（已摘出）、也不在市场（索引里没有）**，在真机上等于凭空消失。**证据链**（逐条可核）：线上 `index.json` 只有 `music` 一条；`gh api …/contents/packages` 只有 `music-0.9.6.zip`（另三个包从没推上去过）；本机 `app\plugin-dist\ocr\` 与 `release\plugin-packages\ocr-0.9.6.zip` **一直是好的**（18:43 就建成了）—— 所以「丢」的不是产物，是**分发**；dev 实例 `target\debug\Modules\` 里同样只有 `clipboard-history` / `convert` / `music`（那三个先前装过）。**处置**：重跑 `scripts\build-plugins.ps1`（4 个包 + 清单 + zip + `release\ext-plugins\` 暂存）→ `scripts\publish-plugins.ps1`（拷包、生成 **4 条**索引、commit `5785685`、push 到 `LythrumMoon/lunac-plugins`）。**实机回归（走真实入口，非手工解压）**：设置 → 插件 → 市场里 `ocr` 那行是「**下载**」⇒ 点它 ⇒ 状态行「**插件 ocr 已安装，立即可用**」⇒ 该行按钮变「**卸载**」⇒ `Modules\ocr\` 落盘 `index.js`（10470 字节）+ `lunac-plugin.json`（737 字节）⇒ `__lunac_open_plugin("ocr")` 打开接管型面板（`searchBar` 带 `plugin-locked`；`ocr-image-preview` / `ocr-clipboard-btn` / `ocr-file-btn` / `ocr-status-line` / `ocr-copy-btn` / `ocr-result` 六个节点齐）⇒ **`layout.takeover` 那条「插件自己声明能力、宿主不按 id 写死」的通道成立**。**要记住的两条**：① `build-plugins.ps1`（产 zip）与 `publish-plugins.ps1`（发市场）是**两步、后者不自动** —— 摘插件的那次改动必须**同一批把索引发出去**，否则用户看到的是「插件凭空消失」而不是「还没发」（已写成预检 #43）；② 用户端 `raw.githubusercontent.com` 有 CDN 缓存，索引 push 完**不是立刻可见**（本机实测约 1 分钟，其间 `index.json` 仍只有 1 条）—— 验收前先 `Invoke-WebRequest` 确认能看到新条目。另外，探针第一次跑时 `fetch_plugin_index` 报了 `Download failed: error sending request`（见上一行的抖动记录）——**重试即成功**，与系统代理 `127.0.0.1:7892` 无关（`curl -x` 走它 0.7s 通、直连 5/5 通）。 |

| **实测（2026-09-29，宿主侧两个前置：鼠标穿透 + 「可见吗」下发）** | **背景**：给 backlog **L1 桌宠**铺路。桌宠的两条实测见 [architecture-rendering.md](./architecture-rendering.md) §6.2，本轮只补**宿主侧**缺的那两件（用户 2026-09-29 选的走法）——契约与实现理由写进 §4.8。① **`plugin_window_set_click_through(ignore)`**（新命令）：**收成宿主命令而不是放开 `core:window:allow-set-ignore-cursor-events`** —— 后者按窗口授，而 `capabilities` 的 `windows` 含 `main`，主窗口一旦被穿**就再也点不回来**（只能杀进程）。② **`plugin-window-visibility` `{visible}`**（新事件）：`WindowEvent::Resized`（tao 把 `WM_SIZE` 含 `SIZE_MINIMIZED` 统一发成它）+ `open()` 复用路径（`show()` 不产生 `WM_SIZE`）两处触发，`LAST_VISIBLE` 记账、**只在翻转时发**。**验证**：`cargo check --bins` exit 0（19 条既有 warning，**无新增**）；`cargo test --bins` **103 passed / 0 failed / 1 ignored**（含新增 `plugin_window::tests::visibility_event_fires_only_on_flip`）。**实机**（dev 0.9.6，CDP 探针 + `user32` 取证）：穿透前 `exStyle=0x40118`、`WindowFromPoint` 命中插件窗自己（root 的 class = `Tauri Window`、title = `memo`）；调命令回 `true` 后 `exStyle=0xC0138`（`WS_EX_TRANSPARENT\|WS_EX_LAYERED`）、**同一点命中它下面的 Chrome 窗口**；关掉后 exStyle 与命中点**全部复原**；主窗口调该命令回 `ERR_NOT_PLUGIN_WINDOW`；穿透开/关两次窗口截图 mean \|dRGB\| = **3.64**（渲染未被穿透破坏）。可见性事件：插件窗订阅后跑两轮「最小化 → 还原」，恰好收到 `false,true,false,true` **四条**（中间那些 `Resized` 没被转发）。⚠️ 跑 `cargo test` 前**必须先停掉 dev 实例** —— 运行中的 `lunac.exe` 锁着 `target\debug`，`tauri-build` 会以 `os error 32` 失败（不是代码错）。 |

| **实测（2026-09-29，桌宠 L1 前半程：清单声明窗口形态 + 桌宠插件本体）** | **背景**：上一行把宿主侧两个前置做完了，这一轮做**桌宠插件本体**。**新增**：清单 `window` 段（见 §4.8「窗口形态由插件清单声明」）、`plugin-window-visibility` 载荷补 `label`、`plugin_window_init` 回 `chrome`、`styles.css` 的 `html.no-chrome`、插件 `Modules\pet\`（源码 `app/src/plugins/builtin/pet.ts`，进 `vite.plugins.config.ts` 的 `pluginEntries` 与 `build-plugins.ps1` 的元数据表，并加进 `main.ts` 的 `FLOATABLE_PLUGINS`）。**编译与单测**：`cargo check --bins` exit 0（19 条既有 warning，无新增）；`cargo test --bins` **107 passed / 0 failed / 1 ignored**（上一轮 103 ⇒ +4：`manifest_validates_window_shape` / `declared_window_shape_overrides_defaults` / `partial_window_shape_keeps_the_other_side_at_default` / `min_size_never_exceeds_default_size`）；`npx tsc --noEmit` exit 0；`npm run verify` **8/8（0 项提醒）**。**实机**（dev 0.9.6，CDP 探针 + `user32` + UI Automation）：清单形态**逐项生效** —— 窗实测 **300×400**（清单值）、`#plugin-titlebar` 的 `display:none`（`chrome:false`）、`html` 的 class = `no-chrome`、`#results-container::before` 的 `display:none`、`body` 背景 `rgba(0,0,0,0)`；`GWL_STYLE` 无 `WS_THICKFRAME`（**禁缩放生效**，主窗口同项有）；`GWL_EXSTYLE` 含 `WS_EX_TOPMOST`（置顶）；**任务栏对照**：只开桌宠窗时任务栏无 Lunac 条目，开一个没声明 `window` 段的窗（音乐）立刻出现 `Lunac - 1 个运行窗口`（⚠️ **不能看 `WS_EX_APPWINDOW`**，那条永远在，见 §4.8）；**透明性**（同矩形前后对照）内容整块藏起时 `504 种色 / 最大单色 92.4%`、最小化后同矩形 `486 种 / 92.7%`。**导入通道**：`localStorage` 里写一个绝对路径 → 图片经 `convertFileSrc` 载入成功（`naturalWidth/Height = 256`）。**可见性停摆**：可见时 `transform` 两次采样不同（呼吸动画在跑），**最小化后两次采样完全相同**（rAF 已停），再从主窗口 `open_plugin_window` 拉回来**立刻恢复**。**控制台 → 桌宠窗**：控制台勾选「鼠标穿透」→ 桌宠窗 `exStyle` `0x40118 → 0xC0138`，取消勾选复原。**顺带修一处**：`executePlugin` 的插件栏标题原来写 `pluginName(plugin.id)`（无兜底）⇒ 宿主词典里没有的插件**把原始 id 当标题显示**（桌宠显示成 `pet`），改成 `pluginName(plugin.id, plugin.name)` 后实测显示 `桌宠`。**踩到的坑**：文本替换改 `scripts\build-plugins.ps1` 会**把 BOM 抹掉**，PS 5.1 立刻按 GBK 解码、在首个中文行报解析错（`npm run verify` 第 ⑧ 节能查出来，已补进 §6 的编码纪律）。**仍未做**：Live2D 引擎（见 backlog L1 的两条硬障碍）。 |
**构建**：`powershell -ExecutionPolicy Bypass -File scripts\build-core.ps1`（等价 `cd core-agent; cargo build --release`）→ `core-agent\target\release\agent.exe`，约 2.5MB（P1 引入 glob/regex 后从 1.5MB 增长）。打包链路（**实际生效的那条**）：`build-release.ps1` **[6/9]** 步把 `lunac.exe` + `agent.exe` + `WebView2Loader.dll` 拷进暂存目录 `release\Lunac\`，再由 `release\lunac-installer.nsi` 的 `File` 指令打进安装包。注意两点：①脚本走的是 `cargo build --release` + 手写 NSI，**不跑 `tauri build`**，所以 `tauri.conf.json` 的 `bundle.resources` 在本流程里并不生效（它只在 Tauri 自带打包器下起作用，别把它当打包依据）；②**[4/9]** 步必须在 Rust 构建之前跑，因为同一步的产物 `agent.exe` 是 **[6/9]** 步要拷的文件。

| **实测（2026-09-29，成本归因第一刀：子代理 / 复盘用量并入 `result.usage`）** | **背景**：backlog **L6** 的第一刀，归因见 §9.1 难点 3。**改动**：`core-agent` 新增 `SubagentUsage`（原子量 + 明细 `Mutex`）并挂在 `Cfg.sub: Arc<…>` 上，`run_subagent` 每轮 `record()`；`detached()` **克隆同一个 `Arc`**（并行子代理批跑的是副本，各建一份就只在串行路径生效）；`run_query` 成功收尾时 `take()` 取走并归零，并入四类总量 + `requests[]`，另**额外上报** `usage.subagent`（**四类总量的子集**，只作归因）；宿主 `UsageRecord.subagent: Option<UsageSubagent>`；前端 `ChatDoneInfo.subagent` + `parseSubagentUsage()`；**`usage-cost.ts` 的金额算式一个字没动**（避免重复计费）。**编译与单测**：`cargo test`（core-agent）**107 passed / 0 failed / 2 ignored**（+2：`subagent_usage_accumulates_and_take_resets` / `detached_shares_the_subagent_usage_ledger`）；`cargo test --bins`（宿主）**108 passed / 0 failed / 1 ignored**（+1：`usage_record_subagent_attribution_is_optional`；同时核过 `usage_record_json_shape` 的**逐字节期望串没被新字段破坏** —— `None` 不写键）；`npx tsc --noEmit` exit 0。**真机**（`core-agent\target\hooktest\e2e-ab-thinking.ps1`，真实端点 flash）：① **子代理链路端到端通过**（`Agent` 工具真的派了一次子代理）—— `turns=2` 而 **`requests=4`**（主 2 + 子代理 2），`subagent: requests=2 in=2448 read=2048 out=179`，答案 `4` 正确；**修好前这一问只会报 2 次请求**；② 顺带量出思考档 A/B（三组，数据见 §9.1 难点 3 结论 6）。**踩到的坑**：用 PS 5.1 驱动 `agent.exe` 时**不要走 .NET 的 `Process.StandardInput`** —— 本机 `Console.InputEncoding` 是带 BOM 的 UTF-8，那 3 字节前导会落在子进程 stdin 头部，而 agent 的读取端只做 `line.trim()`（BOM 不是 Rust 的空白字符）⇒ 每轮报 `忽略非法 JSON 输入行: expected value at line 1 column 1`；`ProcessStartInfo.StandardInputEncoding` 在 .NET Framework 上**不存在**，直接写 `BaseStream`（哪怕用 `Encoding.ASCII`）**也照样带 BOM**（`_probe-stdin.ps1` 三种写法实测均以 `efbbbf` 开头）⇒ 正解 = `cmd /c … < q.json` 喂**无 BOM 文件**且 `RedirectStandardInput = $false`。**仍未做**：面板金额与平台账单的逐小时对账（要等价格表支持分时价，见 L6 第 1 条）。 |

| **实测（2026-09-29，L6① 价格表支持时段价 + 预置官方峰谷价）** | **背景**：backlog **L6** 第 ① 条。归因见 §9.1 难点 3 结论 1 —— 那两档价是 DeepSeek 的**官方峰谷定价**（工作日 09:00–12:00 + 14:00–18:00 峰、其余含周末谷、谷 = 峰 ÷ 2），不是「临时调价」。**改动**：① `pricing.json` 支持 `time_windows`（每条 `{days?, from, to, 四类价}`；**基础四类价 = 缺省（谷）价**，第一条命中的时段覆盖它）；② `validate_pricing_text` 加**严格**时段/星期校验（`9:00` / `from >= to` / `days: []` / `days: [1,1]` / `days: [0]` / `days: [8]` / 时段缺字段 全部判非法）；③ `ensure_pricing_file` 从「空骨架」改成**预置** `DEFAULT_PRICING_JSON`（`deepseek-v4-flash` 官方峰谷价，**只在文件不存在时写**，用户改过的一个字都不覆盖 —— 规则 63 第一条当天按用户批准由「不预置」修订为「预置但有据可核」）；④ `read_usage_range(dates, utc_offset_minutes)` 新增**按本地小时**的桶（`models[].hours[]` = 同一批 token 再切一刀，逐桶之和恒等于总量）；⑤ 前端 `priceAt()` / `weekdayOf()` / `dayCost()` 逐桶计价（`hours` 缺失才退回总量法），`usage-cost.ts` 仍是**唯一**的算法实现（规则 63「算法只许一份」不变）；⑥ 候选预览加「时段价已变更（旧条数 → 新条数）」—— 只改时段价的候选否则会显示成「无变化」；⑦「更新价格」提示词（5 语言）补上「官方页有峰谷价时照 `time_windows` 写法补」。**编译与单测**：`cargo test --bins` **110 passed / 0 failed / 1 ignored**（+2：`usage_range_buckets_by_local_hour` —— 同一份记录用偏移 0 与 480 各跑一次，钉住「偏移真的参与换算」且逐桶之和 == 总量；`default_pricing_json_is_valid` —— 预置表自身合法 + 谷价在基础位、峰价在窗口位、峰 = 谷 × 2、`days` 是 5 天）；`pricing_candidate_is_validated_before_commit` 非法样例 +9 条、新增一条合法时段价样例；`npx tsc --noEmit` = 0。**前端计价用真源码验证**（本仓没有前端测试框架 ⇒ 用 esbuild 把 `app/src/usage-cost.ts` 打成 ESM、把 tauri/i18n 两个 import 换成 stub，再跑断言 —— **17 条全过**）：周二 10/14 点命中峰价、13/18/22 点与周六/周日走谷价、未定价仍 `null`、`weekdayOf("2026-09-29") == 2`；**与账单逐分对账** —— 本地记录 1（`ts=1790662817758` = 北京周二 14:20 峰时，`in=8402 / read=66176 / out=6970`）算出 **0.07521104 元**，与平台 CSV 该窗口的金额**完全相同**；同一批 token 放谷时窗口**只有一半**（旧单档口径因此高估 2 倍）。**dev 环境实测**：2026-09-29 那天，分时口径 **0.136858 元**（峰 0.075211 + 谷 0.061647）vs 旧单档口径 **0.198505 元** ⇒ **面板原本高估 45%**；`target\debug\config\pricing.json` 已按新格式更新为峰谷价。**未做**：重启 dev 后看面板实际渲染的数字（`read_usage_range` 多了一个参数，**旧宿主会忽略它**、面板安静退回单档口径而**不报错** —— 所以不重启也看不出坏，只是数字还是旧的）。 |

### 3.6 数学公式渲染

Agent 回复支持 KaTeX 实时渲染 LaTeX 数学公式：

| 组件 | 说明 |
|------|------|
| **引擎** | KaTeX 0.16.11，**从 CDN 加载**（`index.html` 的 `<script defer src="https://cdn.jsdelivr.net/...">`），手动 `katex.render()` 而非 auto-render，避免 `renderMathInElement` 对动态 DOM 的兼容问题 |
| **定界符** | `$$...$$` 块级公式，`$...$` 行内公式，正则逐一匹配 → `katex.render()` 逐个渲染 |
| **渲染时机** | 流式响应结束后 `setTimeout(renderLatex, 10)` 等待 DOM 稳定后手动 TreeWalker 扫描文本节点 |
| **样式** | 使用 KaTeX 自带 CSS（CDN），还原了之前的暗色主题覆盖，避免与内置样式冲突 |
| **System Prompt** | 指示 AI 所有数学/信号处理公式使用 LaTeX 格式输出 |
| **错误处理** | `throwOnError: false` — 语法错误时保留原始文本 |

> **⚠️ 已知缺陷（2026-09-19 记，未修）**：`tauri.conf.json` 的 CSP 是
> `script-src 'self' 'unsafe-inline'` / `style-src 'self' 'unsafe-inline'` —— **不含 `cdn.jsdelivr.net`**，
> 所以 `index.html` 里那两条 CDN `<script>` / `<link>` 会被 CSP 直接拦掉，`katex` 全局变量**根本不会存在**；
> `renderLatex()` 首行的 `if (typeof katex === "undefined") return;` 会让**公式静默不渲染**（不报错、不留痕）。
> 两条出路（**需要用户决策，别私自选**）：① 把 `https://cdn.jsdelivr.net` 加进 CSP 的 `script-src` / `style-src`
> （恢复联网依赖，牺牲离线可用性、扩大远程代码面）；② `npm i katex` 走本地打包（离线可用、CSP 不动，
> 代价是包体 +约 300KB 含字体）。当前**两条都没做**。

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
- 状态栏展示：就绪/运行中/AI·模式/热键提示/token 仪表盘（恒为真实计费口径：Hit=缓存读、Miss=输入+缓存写、Total=四类之和；数值为**当前这次对话**的累计，与 §3.5「用量与对账」同一口径）。**点表盘**（`#token-dashboard`，有对话时才可点）在其上方展开 `#token-usage-panel`：近 30 天逐日用量与金额（含总计行）+ 价格表状态（见 §3.5「定价表与成本面板」）。
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
- **运行时路径**：`<exe 根>\tools\`（`mcp_server::tools_dir()` = `storage::lunac_root_dir()/tools`）。**不是 `%LOCALAPPDATA%\Lunac\tools\`** —— 整个应用是**便携式**的，所有业务数据都在 exe 所在目录下（`ModuleData/` / `tools/` / `skills/` / `temp/`），卸载时随目录一起清掉。用户可自定义增删。
- **调用链路**：用户输入 → agent.exe (Agent) → MCP Bridge → `lunac.exe --mcp-server` → 执行 shell/http/builtin handler
- **管理方式**：tool-editor 前端插件提供 UI 管理

```
用户输入 (Agent 模式)
  │
  └─ agent.exe 解析意图
       └─ 匹配 MCP Tool
            └─ MCP Bridge (lunac.exe --mcp-server)
                 └─ 执行 Shell / HTTP / builtin
                      └─ 返回结果 → agent.exe → 前端渲染
```

**内置 Agent Tools（`app/src-tauri/tools/`，当前 3 个）：**

| Tool ID | 类型 | 说明 |
|---------|------|------|
| `system_info` | shell | 系统硬件信息：CPU/RAM/GPU/Disk 查询 |
| `weather` | shell | wttr.in 天气查询 |
| `image_pattern_analysis` | builtin | 纯像素统计的图片风格特征提取（亮度/色调/笔触方向/纹理密度/对称性），全离线，用于参考图 → UI 设计 |

> 新增内置 tool 只需往 `app/src-tauri/tools/` 放一个 JSON；`handler.type = "builtin"` 的要在 `mcp_server.rs` 里有对应实现，`shell` / `http` 由 JSON 自带命令。

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
| **历史持久化** | **SQLite**（不是 `localStorage`、也不再是单文件 JSON）：`<exe根>\ModuleData\history\chat.db`，由 Rust 侧 `load_chat_sessions` / `save_chat_sessions`（[chat_db.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/chat_db.rs)）读写，**不再有「最多 50 条」这个上限**（旧 JSON 实现才有，2026-09-17 随 SQLite 化去掉）。`ChatSession = { id, title, messages[], createdAt, usage?, steps? }` —— `usage` 是**表盘口径**的 token 用量快照（hit / miss / total / elided / dropped），`steps` 是**按回合分组的过程快照**（thinking / tool / text，超长截断，见 `recordTurnSteps`）。**回顾历史时把两者读回来**：表盘数值还原到 token 仪表盘，过程渲染成可折叠的「过程 · N 步」块（`renderHistoryProcess`）。ai-agent 仅输入关键词时展示历史列表；新对话自动保存上一会话。详见 §11 规则 30 |

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

**分类是硬约束**（2026-09-29 用户定）：插件分**基础**与**拓展**两类，判据的唯一真相源是
[app/src/plugins/kinds.ts](file:///d:/cc/claude-code-cli-master/app/src/plugins/kinds.ts) 的
`BASE_PLUGIN_IDS` —— 前端任何地方判「是不是基础插件」都必须调它，不许各自再抄一份名单。

| 分类 | 名单 | 随安装包 | 能装 / 能卸 | 代码在哪 |
|---|---|---|---|---|
| **基础** | quick-launch / settings / web-search / ai-agent / memo / **translate** | ✅ 一定装 | ❌ 都不行（市场上只有「打开」） | 编译进 bundle（`builtin/index.ts` 静态 import） |
| **拓展** | clipboard-history / ocr / music / convert（**今后新增的默认都按拓展处理**） | ❌ 默认不装（安装包里有勾选项，见下） | ✅ 市场下载 / 卸载 | 独立打包成 ESM，落在 `<exe 根>\Modules\<id>\` |

`translate` 是 2026-09-29 用户**点名**加进来的基础插件（原话：「词典做底座 + 模型补漏 +
译文存数据库（避免二次翻译），作为基础插件」）—— 它不推翻「今后新增的一律按拓展处理」，
因为那是「默认」，而这是一个显式指定。它排在清单最后：用户没给位置，
插在中间会打乱他定的那 5 个展示顺序（顺序就是面板顺序，见 `kinds.ts`）。

`tool-editor` 归在「AI 助手」名下（`kinds.ts` 的 `MERGED_INTO`）：仍是一个独立、可被搜到的
插件，但市场上不单独占一行。

**拓展插件必须真的「在 bundle 之外」** —— 这是「卸载 = 完全不存在于本应用」的前提：

- 主 bundle 里**不许**留任何对拓展插件模块的 import / 动态 import。`attach.ts` 的硬编码表、
  `main.ts` 里的 `id === "xxx"` 分支都算耦合。宿主需要的能力改由两条路提供：
  **宿主命令**（剪贴板历史的写入 = `append_clipboard_entry`）或
  **声明式权限**（OCR 的接管布局 = `permissions: ["layout.takeover"]`，宿主判据
  `main.ts::isTakeoverPlugin()`；悬浮窗 = `window.float`）。
- 卸载要一次收干净：detach 监听 → 关它的悬浮窗（宿主命令 `close_plugin_window`）→
  关主窗口里它的面板 → `refreshMarketPlugins()` 重扫。**重扫必须顺手摘掉「上次注册过、
  这次盘上没有」的 id**（`market.ts` 的 `marketRegistered`）—— 只遍历当次扫描结果去
  `unregister` 永远摘不掉它，表现为「卸完了搜索里还能搜到」（2026-09-29 实测踩到）。
- 市场列表的两次绘制（先本地、后索引）有并发：`renderMarket` 用代次号 `marketRenderGen`
  保证**迟到的旧快照不许覆盖新结果**，否则会看到「卸完了那行还挂着卸载按钮」。

**插件包必须单文件自包含**：`app/vite.plugins.config.ts` **一次只打一个入口**（由
`scripts/build-plugins.ps1` 逐入口各调一次，用环境变量 `LUNAC_PLUGIN=<id>` 指定打谁）。
多入口同批构建时 Rollup 会把共享模块提到 `plugin-dist/<chunk 名>/chunk-*.js`，而插件包只搬走
自己那个目录 ⇒ 那份 chunk 丢失、`import` 404、插件打开即失败（2026-09-29 实测踩到）。
（Vite 的 CLI **不支持**一个配置文件导出多份配置，别往那个方向改。）

安装包侧的勾选项在 `scripts/lunac-installer.nsi`：核心段 `SectionIn RO`（组件页上取消不掉），
四个拓展插件各一段 `/o`（**默认不勾**），包体来自 `build-plugins.ps1` 暂存的
`release\ext-plugins\<id>\`。音乐那段额外带上 `deps\librespot\bin\librespot.exe` ——
走市场安装时这一步由清单的 `dependencies[]` 自动下载，走安装包安装没有那一步。

下表是各插件的图标与触发关键词（与分类无关）：

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
| music | 🎵 | 音乐, 歌词, 歌曲, 正在播放, music, lyrics, spotify, 播放控制, 暂停, 下一首 |
| convert | 🔄 | 转换, 格式转换, 转格式, convert, 格式, 转码, 提取音频, 图片转换, 视频转换, 音频转换 |
| translate | 🔤 | translate, translation, dict, dictionary, meaning, word, 翻译, 词典, 字典, 查词, 释义, 译文, 中英 |

> `translate` 的行内图标是**语言符号**（「文」与「A」相对，`PLUGIN_ICON_PATHS`），
> **刻意不用地球** —— 那个图形已经是 `web-search` 的，两行图标一样会让用户认错插件。

### 4.6 音乐插件（歌词 + Spotify 播放控制，2026-09-27）

用户要求把「歌词抓取」与「Spotify 播放控制」做成**一个**插件。前端只画界面 + 轮询，
**一切联网都在宿主** —— 理由与「插件市场索引必须由宿主去拉」完全相同（§11 规则 67 第 ⑥ 条）：
`tauri.conf.json` 的 CSP 是 `default-src 'self' https://asset.localhost`，**不含远端域**，
插件里 `fetch()` 会被直接拦掉。

| 宿主命令（[music.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/music.rs)） | 作用 |
|---|---|
| `music_config_get` / `music_config_set` | 读写 `config\music.json`（Client ID / 回调端口 / 令牌）。**不回传令牌**给前端 |
| `spotify_connect` | 起环回服务器并返回授权 URL（前端交给 `open()`）。**唯一不用 `run_blocking` 的命令**：它只 bind 端口就返回，accept + 换令牌在独立线程 |
| `spotify_disconnect` | 清空本地令牌（等于「忘记账号」） |
| `spotify_status` | `GET /me/player`；204 = 无活跃设备（不是错误）。另回传 `shuffle` / `repeat` / `context_uri`（控制条三态与歌单高亮都靠它，**状态一律回读、不自记**） |
| `spotify_control` | play / pause / next / previous / seek / volume |
| `spotify_playlists` | `GET /me/playlists?limit=50` —— 歌单列（**不开分页**）。曲目数读 `items.total`（`tracks.total` 作兜底） |
| `spotify_playlist_tracks` | `GET /playlists/{id}/items?limit=100` —— 展开某个歌单时的曲目（前端按歌单 id **缓存**，收起/展开不再打 API）。**必须是 `/items`，不是 `/tracks`**，见下面「端点改名」 |
| `spotify_queue` | `GET /me/player/queue` —— 播放队列（当前曲 + 最多 20 条待播） |
| `spotify_play_context` | `PUT /me/player/play` 带 `context_uri`（+ 可选 `offset.uri`）。点歌单里的某一首走它 ⇒ 队列就是整个歌单 |
| `spotify_play_uri` | `PUT /me/player/play` 带 `uris:[uri]` —— 队列项「从这首开始播」（见下面「没有删除队列项的接口」） |
| `spotify_set_play_mode` | 随机三态：`off` / `shuffle` / `repeat_one`（落到 `shuffle` + `repeat` 两个端点）。**没有「智能随机」这一档** |
| `spotify_liked` | `GET /me/tracks?limit=50` —— **「我喜欢的歌曲」（收藏夹）**。见下面「收藏夹是独立资源」 |
| `spotify_play_uris` | `PUT /me/player/play` 带 `uris:[…]`（最多 100 条）—— 收藏夹**没有可播的 `context_uri`**，只能走它 |
| `spotify_search` | `GET /search?type=track,playlist,album,artist,show&limit=10` —— 顶部工具条那个搜索框（**下拉预览 + 主区详细页共用这一次请求**）。`q` 必须 `qenc` 转义 |
| `spotify_artists` | `GET /me/following?type=artist&limit=50` —— 左栏「歌手」。**必须有 `user-follow-read`**，缺了就是 `403`（面板会如实把那句话显示成这一栏的内容） |
| `spotify_albums` | `GET /me/albums?limit=50` —— 左栏「专辑」（走 `user-library-read`，不需要新 scope） |
| `spotify_shows` | `GET /me/shows?limit=50` —— 左栏「电台」（同上） |
| `spotify_item_tracks` | `kind + id` → 该条目的曲目：`playlist` `/playlists/{id}/items`、`album` `/albums/{id}/tracks`、`artist` `/artists/{id}/top-tracks?market=from_token`、`show` `/shows/{id}/episodes`。**歌手那条的数组键是 `tracks` 而不是 `items`**，专辑那条是 Simplified Track Object（**没有 `album` 字段**，行内封面为空是正常的） |
| `spotify_devices` | `GET /me/player/devices` —— 设备弹层。**无活跃设备时它是空数组，不是错误** |
| `spotify_transfer` | `PUT /me/player`（`device_ids` + `play`）—— 把播放转到某台设备 |
| `librespot_status` / `librespot_start` / `librespot_stop` | 本机播放（librespot 子进程）的状态 / 起 / 停。见下面「本机播放」 |
| `lyrics_get` | 歌词三源：LRCLIB 精确 → LRCLIB 搜索 → **网易云兜底**（见下） |

> 另有一条**宿主命令**不在 `music.rs` 里但由本插件用：`plugin_window_set_resizable`（[plugin_window.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/plugin_window.rs)）—— 播放态关掉手动缩放，见下面「播放态禁用缩放」。

**`SCOPES` 2026-09-28 扩到六项**：`user-read-playback-state` / `user-modify-playback-state` /
`user-read-currently-playing`（播放控制与回读）、`playlist-read-private` / `playlist-read-collaborative`
（歌单列要读私有与协作歌单）、**`user-library-read`**（收藏夹 + 专辑 + 电台）、
**`user-follow-read`**（左栏「歌手」，2026-09-28 加）。
**scope 变了 ⇒ 已授权的令牌不会自动带上新权限** —— 老用户必须**重新登录一次 Spotify**，否则受影响的
那一栏会如实报 `403 Insufficient client scope`（实测就是这个措辞）。这不是 bug，是 OAuth 的规则。

**收藏夹是独立资源，不在 `/me/playlists` 里**（2026-09-27 排查确认，`music.rs`）

「我喜欢的歌曲」这一项**不是**某个歌单：实测 `/me/playlists` 只有 `total=8` 且**没有它**，
它走 `GET /me/tracks`，**且必须有 `user-library-read`** —— 缺这一项直接 `403`
（所以「歌单列里找不到收藏夹」的答案不是「Spotify 把它藏了」，而是**我们没申请那条权限**）。
两条连带后果，都必须照做：

1. **收藏夹那一行是前端合成的**（`likedHtml()`），不从歌单数组里筛 —— 它是 `likedHtml()` 拼在**最前面**的，
   且**不随歌单请求的成败消失**（歌单拉失败时用户仍该看得见自己的收藏）。它的副标题是
   「账号名 · 总数」，缩图是渐变 + 白心（Spotify 那块拼图的观感）。
2. **收藏夹没有可播的 `context_uri`**（`spotify:collection` 不在合法上下文里）⇒ 播放只能走
   `spotify_play_uris`。代价：`player.context_uri` 永远匹配不到它，于是**「正在播的就是这个歌单」那个高亮对它无效**
   —— 不接受也得接受，这是 API 的形状。

> **垃圾入参一律先拒**：`playlist_id` / `context_uri` / `track uri` 全走白名单函数（`is_safe_spotify_id` /
> `is_safe_context_uri` / `is_safe_track_uri`）—— 它们会被拼进 URL 路径与请求体，是唯一的外部输入面。
> 三条判据都有单测（`spotify_id_and_uri_whitelists_are_narrow`）。

**凭据：内置 Client ID + 保留自填**（2026-09-27 加）——**两条路必须并存**。

`music.rs` 顶部的 `pub const BUILTIN_CLIENT_ID: &str` 是给仓库主人填**自己那个应用**的
Client ID 的（2026-09-27 **已填入**仓库主人的应用 ID；改成空串 = 没有内置值，行为与加这个
常量之前完全一致）。填上之后：

- 普通用户**只需点「连接 Spotify」并在浏览器里登自己的账号**，不必去 developer.spotify.com
  建应用 —— 这是用户 2026-09-27 提的第 4 条（「这个步骤只是针对了开发者」）。
- `MusicConfigDto.builtin` 告诉前端当前用的是不是内置值，面板据此把凭据字段**默认收起来**
  （`music.ts` 的 `renderSetup` + `#music-fields-toggle`）。自填入口**不得删**：内置值只有
  **5 个授权名额**（见下），超过就得用自己的应用。
- **所有者那一侧有前置**：内置值的那个应用里必须已经登记面板上显示的那个回调地址
  （默认 `http://127.0.0.1:8899/callback`；端口改过就要与新的逐字符一致），否则用户点
  「连接」会在浏览器里撞上 `INVALID_CLIENT: Invalid redirect URI`。Spotify **不接受 `localhost`**。

**自动弹出**（2026-09-27 加，`spawn_spotify_watcher`，由 `main.rs` 的 `setup` 起线程）：
Spotify 桌面端**从「没在跑」变成「在跑」**的那一刻自动开 `plugin-music` 悬浮窗
（用户选定的触发条件是「只要 Spotify 在运行就弹」）。三条约束：

1. **判据是进程存在，不是「正在播放」** —— 后者要先有 OAuth 令牌，未登录时永远判不出来。
   用 raw FFI 的 `CreateToolhelp32Snapshot` 读进程表（`music.rs` 的 `win_proc` 模块），
   **不引 `sysinfo`、也不 spawn `tasklist`**（后者每 3 秒起一个进程，且输出是本地化文本）。
2. **只在边沿触发**（`SPOTIFY_WAS_RUNNING` 原子量 + `!running || was` ⇒ 跳过）。
   写成「只要在跑就确保窗口存在」的话，用户刚关掉窗口就会被 3 秒后下一轮重新拎出来 ——
   **关掉等于关不掉**。边沿语义下「关掉」有效：要它再弹，得让 Spotify 退出再启动。
3. **已经开着就不动**（`plugin_window::is_open`）—— `open()` 的复用路径会 `set_focus()`，
   从后台线程定时抢焦点是最不该发生的事。另：开机自启（`--background`）时把「上一轮」
   预置成 `true`，于是「开机时 Spotify 已经在跑」不构成边沿，静默启动不会弹窗。

**界面：两态 + 定尺**（2026-09-27 重构，2026-09-28 默认态重做成两栏）

两态**都在 DOM 里**（靠 `.hidden` 切），理由不只是省事：播放态的窗口高度要按**当前那一态**的内容实测下发，
`display:none` 的节点不参与布局、量出来是 0，切错了量到的就是另一态的高度。

| 态 | 内容 | 窗口大小 |
|---|---|---|
| **默认态** | **工具条**（头像 / 账号 / 搜索 **+ 下拉预览** / 状态 / 设置）+ 连接设置（未登录时）+ **左栏 Library（四类页签，宽度可拖）+ 分隔条 + 主区（曲目列表 / 卡片网格）** + **底部控制栏**（正在播放 / 播放键组 / 设备 / 音量 / 队列 / 展开播放面板，见下） | **`1280×720`**（16:9，用户 2026-09-28 定）—— **定尺**，两栏各自内部滚动 |
| **播放态** | **长条本身**（见下） | **`550×130`**；鼠标进窗时关闭操作栏浮出 ⇒ **`550×170`** |

**两态怎么切：只由用户点**（2026-09-28 用户改口径，**取消了 2026-09-27 那套「在播就自动进播放界面、暂停就自动退回歌单列」**）。
进：底部控制栏那块「正在播放」、或它右端的展开按钮（`#music-open-player`）。出：长条上的返回（只退一层、不停播）。
停播 + 退回是标题栏那个 ← 的事。**别再把它改回自动** —— 那套要在换曲那一瞬防抖（`STOP_CONFIRM = 2`），
且一旦写成「有曲目就进」+「没在播就退」就会每 tick 拉一次、以几秒为周期反复横跳。

**进入音乐界面的唯一入口是独立窗**（2026-09-28 用户定：「从 Lunac 中进入音乐界面时自动展开成独立界面，
禁止进入到插件版本中的音乐界面」）：宿主在 `main.ts` 的 `executePlugin()` **开头**拦掉 `id === "music"`
（二十来个调用方——搜索命中 / 设置里的插件总览 / 右键菜单 / 详情项 / 快速启动——逐个判必漏一处，
漏掉那处就是「内嵌版」的后门），改成调 `open_plugin_window`。独立窗自己的渲染走 `plugin-window.ts` 的
`plugin.execute()`，**不经过** `executePlugin`，所以不会自打转。

**播放态：窗口就等于长条**（2026-09-28 用户要求「把那个 562×200 的渲染窗口去掉」）。两处来源都得治，
**只治一处等于没治**：

1. **宽度** —— 默认态窗口 1280 是给两栏用的；播放态的垫料被 CSS 清零
   （`styles.css` 的 `body:has(.music-root.music-player-on)` 那一组：`#app` padding、容器边框、
   `#results-list` padding、`.plugin-result` padding 全部归零），所以**窗口就是 550**。
   前端因此有**两个宽度常量**（`MUSIC_W = 1280` / `MUSIC_W_BAR = 550`），`applyResize` 按态选，
   并把宽度也写进 `lastResize` 的键（只比高度会漏掉「高度巧合相同」的那次）。
   ⚠️ **默认态不量内容**：它是定尺 1280×720（两栏各自内部滚动），只有播放态才实测下发。
2. **高度** —— 宿主 `plugin_window_resize` 原来用**通用的 `MIN_H = 200`** 夹，而播放态实测只要
   **159**（130 长条 + 29 外层）⇒ 窗口被顶到 200，用户看到的就是「长条下面多出一截」。
   现在按插件取最小值（`plugin_window.rs` 的 `min_size()`：音乐插件有 `MUSIC_MIN_H = 120`），
   **`min_size()` 与 `min_inner_size()` 必须同时用** —— 后者是系统级硬约束（Windows 走
   `WM_GETMINMAXINFO`），只放开 clamp 不改它，`set_size` 照样被系统夹回去。
   清零垫料后 `chrome` 恰好等于标题栏高度，实测 `130`（收起）/ `170`（hover），**不再有半像素偏差**。
3. **顺带两个坑**：容器上那条 `margin-top: -1px`（为叠搜索栏边框）在垫料归零后会变成
   「列表比窗口高 1px」⇒ `chrome` 量出 **-1**、下发高度少 1px 且可能诱出滚动条，所以也必须归零；
   容器圆角要写成 **12px** 与 `.music-pv-card` 对齐（默认的 `--radius` 是 14，差 2px 时玻璃层的圆角会露出来）。


**默认态 = Spotify 的复刻版**（2026-09-27 起两轮改造：先重做行样式 + 工具条，2026-09-28 再拆成两栏）

布局（用户 2026-09-28 的要求逐条对应）：

```
┌─ 1280×720 ────────────────────────────────────────────────────────────────┐
│ 头像 账号 [搜索框（下拉预览挂它下面）] 状态 ⚙设置                          │
│ ┌─ 左栏 300（可拖 180–520）─┐ ┃ ┌─ 主区（吃掉剩下全部）───────────────────┐ │
│ │ [歌单][专辑][歌手][电台]  │ ┃ │ 详情头：封面 120 + 类型/名字/副标题/播放全部│ │
│ │ ─────────────────────────│ ┃ │ ─────────────────────────────────────────│ │
│ │ ♥ 我喜欢的歌曲           │ ┃ │ 曲目列表（编号 / 歌名 / 歌手 / 时长）      │ │
│ │ 歌单 / 专辑 / 歌手 / 电台 │ ┃ │                                        │ │
│ │ （内部滚动）             │ ┃ │ （内部滚动）                            │ │
│ └──────────────────────────┘ ┃ └────────────────────────────────────────┘ │
│ ══════════════════ 细进度线 2px（可拖）═══════════════════════════════════ │
│ ┌封面 歌名/歌手┐      ⤨ ◀◀ ▶ ▶▶ ↻       🔊── 🔊音量  ☰队列 ⤢展开 ⚙设备   │
└──────────────────────────────────────────────────────────────────────────┘
```

**底部控制栏**（用户 2026-09-28：「在默认面板下方添加 Spotify 下方的播放控制，并把一些按钮也移动到下方」）：
三列 `grid-template-columns: minmax(0,1fr) auto minmax(0,1fr)` —— 左 = 正在播放（整块是一个按钮，点它进播放面板）、
中 = 随机 / 上一首 / 播放暂停 / 下一首、右 = 设备 / 音量 / 队列 / 展开。**中间那组必须用 grid 的 `justify-self` 才真居中**
（flex 会因为左右两簇宽度不等把中间整体推偏，而这是「对齐 Spotify」唯一的硬判据；实测两组中点都在 640）。
栏顶那条 **2px 细进度线**绝对定位（不占高），与播放长条里那条**共用同一套拖动绑定**。
两个弹层（设备 / 队列）都**向上弹**（`.music-dd` 默认是向下的，这两条把 `top` 清掉改用 `bottom` —— 栏贴着窗口下沿，
向下弹会跑出窗口）；队列浮层与长条下面那块队列面板**同一份渲染**（`renderQueue` 写两处，同一批 `play-uri` 行）。
两条栏上的按钮是同一件事的两个入口 ⇒ **只写一份处理函数、按 id 各挂一次**（`on([...])`），
状态也**只由 `renderPlayer` 一处写两处**（图标 / 模式 / 音量 / `--seek-pct`；两处各记一份必然漂移）。

- **左栏那四类只有一个渲染入口**（`renderSide` + `libRowHtml`）：`PlaylistDto` 用
  `libItemOfPlaylist()` 归一成 `LibraryItemDto`，其余三类宿主直接回这个形状。**收藏夹是前端合成的**
  （见上面「收藏夹是独立资源」），固定拼在「歌单」栏第一行。每类各自的列表**缓存**（`sideCache`），
  切回来不重打 API；**失败不写空数组** —— 那会被渲染成「这一栏是空的」，把真因（`403` / 超时）藏掉。
- **左栏宽度可拖**（用户：「歌单列作为单独的一列可拉动」）：`#music-split` 是 6px 的分隔条，
  线画在它的 `::before` 上（真画 1px 的线，鼠标几乎压不中）。**宽度只有一处定义者** ——
  JS 往 `.music-root` 写 `--music-side-w`，CSS 用 `flex: 0 0 var(--music-side-w, 300px)`；
  拖动期间给 `body` 加 `.music-resizing`（锁 `col-resize` 光标 + 禁选中）。
  用 `pointerdown` + window 上的 move/up（`setPointerCapture` 对合成事件会抛 `NotFoundError`）。
- **主区三种内容互斥**：**搜索详细页 > 选中项详情 > 引导文案**。详情头 = 封面 120（歌手是圆头像）+
  类型 / 名字 1.35rem / 副标题 / 「播放全部」；下面就是曲目列表（`.music-list`，主区与搜索页共用一套行）。
- **搜索分两层**（用户 2026-09-28 要求）：工具条那个下拉是**速览预览**（歌曲 3 + 歌手/专辑/歌单各 2 +
  底部「查看全部」），**回车或点「查看全部」⇒ 主区进详细页**（5 个页签：歌曲 / 歌手 / 专辑 / 歌单 / 电台，
  非歌曲的用卡片网格）。预览与详细页**不互斥**：详细页开着时继续敲字，预览照样更新。
  预览**绝对定位挂在 `.music-search-wrap` 下**（对齐输入框、不占布局 —— 占了布局，定尺窗口的高会跟着结果条数抖）。
- **设备按钮在设置左边**（用户：「如果可以拉取这个功能就尽量作为控制设备的按钮放置在设置的左边」）：
  它取代了旧版那条只能看、不能点的「无活跃设备」文本行。信息不丢 —— 挂在该按钮的 `title` 上，
  且**没有活跃设备时给按钮加 `.warn`**（那正是用户最需要知道的时候）。
- **条目行的选中态与「正在播」是两件事**：`.music-pl.on` = 主区正在看的那一项（前端记的），
  `.music-pl.playing` / `.music-tr.playing` = `player.context_uri` / 当前曲目命中（每秒那轮 tick 重算，
  只改 class —— 整块重建会把选中态和滚动位置打掉）。
- **工具条**：头像（26px 圆，`/me` 的 `images[0]`，授权那一次抓的，见 `MusicConfigDto.avatar`）
  + 账号名（`max-width:130px` 截断）+ **搜索框**（胶囊，占满余下宽度）+ 状态文字 + 设置。
  **设备按钮与「展开播放面板」都在底部控制栏那一端**（2026-09-28 挪下去 —— Spotify 的顶栏本来就没有这两样）。
- **头像 URL 会晚一步**：老配置文件里没有这个字段（`#[serde(default)]` ⇒ 空串），
  **要等用户重新授权一次才填上** —— 空串时头像就是个素色圆，不是 bug。
- **条目行照 Spotify 的样子**：缩图 **40px**、行 hover 加一层 `rgba(var(--ink-rgb),0.05)` 淡底、名字 0.78rem。
  **行尾那枚「播放这一项」圆钮已删**（用户 2026-09-28：「删除左栏跟随歌单的按钮，以及搜索栏中跟随歌单的按钮，
  功能挪到双击这一项」）⇒ 现在**双击左栏某一行 = 播它**（详细页头那个「播放全部」保留，所以「先看曲目再决定」这条路没断）。
  为了让双击真的能触发，单次点击**不许重建左栏**（`syncSideSelection` 只改 class）—— 否则第二次 click 落在新节点上、
  浏览器按 UI Events 的规矩**不会发 `dblclick`**（顺带也修好了「点第 20 行被弹回第一行」）。
- **左栏行不着重画框**（用户 2026-09-28：「取消外框，改成不明显的分界线」）：`.music-pl` 没有 border / radius，
  相邻行之间一条 `rgba(var(--ink-rgb),0.05)` 的细线（最后一行不画）。连带三处改成不靠 border 表达：
  「正在播」只提亮名字、选中态用更淡的一层底、收藏夹读失败把**副标题**染红。

- **搜索必须防抖**（`SEARCH_DEBOUNCE_MS = 450`）：每敲一个字打一次 Spotify 搜索既浪费又容易撞限流。
  回调里要带「用户又改了词 ⇒ 这次结果作废」的守卫（`runSearch` 开头比对当前 `searchQuery`）。
- **空搜索词直接清结果**，不发请求（`/search` 的空 `q` 是 400）。
- **主区那两个滚动条各滚各的**：默认态整条链路是「撑满窗口」（`styles.css` 里
  `body:has(.music-root):not(:has(.music-player-on))` 那一组把 `#results-list` 变 flex 容器、
  `.plugin-result` 撑满、`.music-body` 吃余高）。**这组选择器必须排除播放态** —— 那一态要靠
  `.plugin-result` 的**实测高**反推窗口尺寸，给它 `height: 100%` 就成了「量高度 ← 窗口高 ← 量高度」的死循环。

播放态的长条是用户 2026-09-27 给的定尺，**每个数都一一对上**（dev 实例实测，见下）：

```
┌─ 长条 550×130（圆角矩形）────────────────────────────────┐
│ ┌────────┐ ┌─ 右侧列 450×130 ────────────────────────┐ │
│ │ 封面   │ │ 歌名模块  450×30                        │ │
│ │ 100×100│ ├────────────────────────────────────────┤ │
│ │        │ │ 歌词模块  450×65（两行，行高 32）        │ │
│ │        │ ├────────────────────────────────────────┤ │
│ │        │ │ 控制面板  450×35                        │ │
│ │        │ │  ├ 进度条 450（**叠在顶部，不占高度**）  │ │
│ │        │ │  ├ 时间 0:12/3:45（**最左端、条下方**）  │ │
│ │        │ │  └ [随机][上一首][播放][下一首] 居中     │ │
│ │        │ │     音量条 80 ─ 播放列表按钮（右端）     │ │
│ └────────┘ └────────────────────────────────────────┘ │
└────────────────────────────────────────────────────────┘
```

- **内容 550 是用户定的基准**，播放态靠「窗口 550 + 外层 0」凑齐它（见上面「播放态：窗口就等于长条」）。
  **默认态不再需要凑 550** —— 它是 1280×720 的两栏，外层那 12px 玻璃垫料照旧留着（`#app.detached` padding 4 +
  `#results-container` 边框 2 + `#app.detached #results-list .plugin-result:has(.music-root)` 的 3×2）。
- **长条的边框必须用 `box-shadow: inset 0 0 0 1px`，不能用 `border`**：`border` 会吃掉内容盒
  （实测 550→548），右侧列被压成 448、`height: 130` 还会比内容盒高 2px 而溢出。inset 阴影不参与布局。
- **右侧列里不许留左右 padding**（`450` 是用户定的，留 10px 就变成 438，实测过）。文案的呼吸感
  放在三块**各自的内部 padding** 里，块宽仍是 450。
- **时间（当前 / 总）在控制块的最左端、进度条下方**（用户 2026-09-27 定）：`.music-bar > .music-time`
  **绝对定位** `left:0`，于是**不参与四钮的居中计算**（写成普通 flex 子元素的话，四钮会被它挤得偏右）。
  实测 `left` 与 `.music-bar` 的左边界**差 0**。
- **关闭操作栏：高 40、宽 550，且只在鼠标进入窗口时显示**（用户 2026-09-27 定；见 [styles.css](file:///d:/cc/claude-code-cli-master/app/src/styles.css) 的 `#plugin-titlebar`）。
  三条实现约束，改一条就会破：
  1. **收起时高度归 0**，不是 `visibility:hidden` —— 窗口高度是按内容实测下发的，归 0 才能让窗口真的缩回
     「长条 130」那一档；`overflow:hidden` 保证收起时按钮既不显示也点不到。
  2. **高度不做过渡** —— 它一变就要重算窗口尺寸，补间期间量到的是中间值。
  3. 尺寸联动手柄：`music.ts` 在 `document.body` 上听 `mouseenter` / `mouseleave` **立刻** `scheduleResize`
     （不能等下一秒那轮 tick，否则那 40px 会先被裁掉一下才长回来）。多出来的高度由 `chrome` 现算，
     **不需要**在这里加常量。
- **播放态那个「×」其实是个「← 回退」**（用户 2026-09-28 定：**「将 × 改成向左的回退按钮 用来引导」**）。
  为什么换：播放态是定尺长条、**缩放已经关掉**，那个位置上的 × 会**直接关掉整个悬浮窗** —— 用户的本意
  往往只是「退出播放界面」，结果面板整个没了（要回主窗口重开）。同一位置两种语义就此分开：
  **默认态的 × 真的关窗，播放态的 ← 只退一层（顺带停播）**。三条实现约束：
  1. 图标在 **JS 里换**（`syncTitlebarClose()`）：那个 × 是 `plugin.html` 里写死的静态 SVG，
     用 CSS 藏一半再画一半要赌 `:has()` 与伪元素尺寸。原样记在 `closeBtnOriginal` 里，换回来**逐字还原**
     （`stopMusicPolling()` 里也还原一次 —— 悬浮窗会被复用去装别的插件）。
  2. **点击在捕获阶段拦**（`document` 上的 `click` + `capture: true`）：那个按钮的关窗监听器在
     `plugin-window.ts`（所有插件共用的入口，不能为一个插件改它）。捕获阶段先用 `stopPropagation()`
     把事件掐掉，共用入口一行都不用动。
  3. 退回去要做两件事：`spotify_control{action:"pause"}`（**Spotify Web API 根本没有 stop**）+ 回默认面板。
     **不必再记「退掉的是哪一首」** —— 2026-09-28 起没有任何自动切态的逻辑（见上「两态怎么切」），
     退回去它就一直停在默认面板上。
- **播放态禁用窗口缩放**（用户 2026-09-28 定）：定尺长条手动拉一下只会被下一轮 tick 贴回去
  （用户看到的就是「拉了没反应」，像坏了）。前端在 `renderMode` 里按态调一次
  `plugin_window_set_resizable`（**只在态变化时发一次 IPC**，别放进每秒那轮 tick —— `set_resizable`
  会打一次窗口消息）。默认态仍是可缩放的（用户拖大后不会被贴回去：`lastResize` 相同时 `applyResize`
  直接返回，不去打架）。
- **进度条叠在控制面板顶部**（绝对定位 `top: -6px` + `padding: 6px 0` + `background-clip: content-box`）：
  画出来只有 3px，但可点可拖的范围上下各 6px，**不占长条高度**。所以 `.music-bar` 要 `padding-top: 4px`
  把按钮下移，否则居中的播放键会压在那条线上（按钮 26px / 播放键 30px 就是按这个余量定的）。
  位置**只写一个 CSS 变量 `--seek-pct`**（填充 / 圆点 / 时间气泡都由 CSS 从它取值）——
  三处各自写 JS 迟早漂移。拖动中给 `.music-progress` 加 `.music-dragging`：填充关掉补间（要逐帧跟手）、
  圆点与气泡浮出。**松手才真 seek**（拖动期间只画不发请求，否则一次拖动会打出几十个 seek）。
- **队列面板挂在长条下方**，**不再与歌词互斥**（旧版那套「谁占封面下面那一格」是 450 宽旧布局的产物）：
  长条是定尺 130，塞不进队列。开着时窗口按内容长高。
- **高度由前端实测后经 `plugin_window_resize` 下发**（`WebviewWindow::set_size`）：`chrome`（标题栏 + 外层 padding +
  边框）用「窗口内高 − `#results-list` 可视高」现算，所以外层 CSS 改了不用跟着改常量。**`applyResize` 里必须
  `closest()` 往上找 `.plugin-result` / `#results-list`** —— 传进来的 `root` 是 `.music-root`，用 `querySelector`
  往下找永远是 null（2026-09-27 就这么写错过一次，表现是「尺寸一次都没下发」，而且因为 catch 掉错误、页面毫无反应）。
- **只在插件悬浮窗里下发尺寸**：判据是 `#plugin-titlebar` 存在（`plugin.html` 独有）。同一份 `music.ts` 也会
  **内嵌在主窗口**的 `#results-list` 里跑，那时调这条命令会去改**主窗口**的尺寸。
- **「没生效」由宿主回读尺寸判定，不看 `is_minimized()`**（2026-09-28 改）：
  窗口最小化时 `set_size` 会返回 `Ok` 但可见几何一点不变（`inner_size()` 回的是最小化窗口那个退化的
  `160×28`）—— 这种「命令成功、窗口没动」最难查。所以 `plugin_window_resize` 下发之后**回读实际尺寸**，
  对不上就回 `ERR_SIZE_STUCK(...)`；前端**只有真的贴合了才记 `lastResize`**，并且每秒那轮 tick 会再
  `scheduleResize` 一次 ⇒ 用户把窗口恢复出来时（≤1s）自动贴合。
  ⚠️ 早先那条「`is_minimized()` 为真就直接拒」在 2026-09-28 的验收里表现成**静默失效**（窗口卡在默认态尺寸、
  每轮都在重试但不留任何痕迹）；回读判定既覆盖那个场景、又给出数字（`160x28≠550x130`）。
  容差 **2px**：`LogicalSize → PhysicalSize` 那一趟按 DPI 比例取整（1.25 缩放下 550 → 688 → 550.4）。
- **歌词区正好两行**：行高 32px + 盒高 65px ⇒ 当前行贴顶、下面那行就是「接下来要唱的」；
  行内 `nowrap + ellipsis`（定尺窗口里换行会打乱这条几何关系）。
- **歌词着色：已过 + 正在 = 亮，未到 = 暗**（用户 2026-09-27 定，取代旧版「当前行 / 下一行」两档）。
  「亮」是**色相不变、只提亮** —— 向 `--ink-rgb`（深色主题=白 / 浅色主题=黑）混合，两种主题下都朝
  「更醒目」的方向走；**不能**用 `filter: brightness()`（浅色主题下会把文字越调越淡）。
  着色只改**状态变化的那一段**的 class（正常播放就是 1 行）：整段重扫在几百行的长歌词里每秒跑一次纯属浪费，
  多写 DOM 还会打断正在跑的滚动动画。
- **滚动用 rAF 自己缓动（`glideTo`），`LYRIC_SCROLL_MS = 800` + `easeOutQuint`**（用户嫌原来的太快、
  要求「阻尼调高」）。**因此 `.music-lyr` 上不能再写 `scroll-behavior: smooth`** —— CSS 平滑会接管
  每一帧 `scrollTop` 的赋值，与 rAF 叠成两段动画（走起来一顿一顿的）。
- **点某一行歌词 = 跳到那一句**（用户 2026-09-27 加）。**拖滚动条松手同样会发 `click`** ⇒ 用「指针位移
  是否超过 4px」把两者分开，否则每拖一次都会跳进度。
- **用户可以自己滚**（滚轮 / 拖滚动条）：只认 `wheel` 与 `pointerdown` 来挂起自动跟随，**不听 `scroll`** ——
  程序化滚动同样会发 `scroll` 事件，听了就会自己把自己挂起。挂起后 4 秒自动回到当前行。
  **挂起只影响滚动、不影响着色** —— 否则用户滚一下歌词，全篇颜色会跟着冻 4 秒。
- **两态切换只由用户点**（2026-09-28 用户改口径，**取消了 2026-09-27 那套自动切态**）：
  进 = 底部控制栏那块「正在播放」或它右端的展开按钮；出 = 长条上的返回（只退一层、不停播）。
  历史留档：那套自动切态（`playing === true` 就进、`!playing` 连续两轮就退）在换曲那一刻必须防抖
  （`STOP_CONFIRM = 2`），而且**两条判据必须互斥** —— 「有曲目就进」+「没在播就退」会每 tick 拉一次、
  以几秒为周期反复横跳（歌单列一闪一闪）。用户嫌它吵，直接取消了；**要恢复的话这两条纪律一条都不能少**。
- **状态行要区分「还没问过」与「问了、没设备」**（2026-09-27 修）：`player === null`（刚开窗、第一轮
  轮询还没回来）说「无活跃设备」是冤枉。顺序是：未配置 → 未连接 → **读取播放状态…** → 无活跃设备 →
  已暂停 → 正在播放。
- **`#music-msg` 是一次性提示，必须自己回收**（2026-09-27 修）：它有七八处写入方（控制失败、歌单拉取失败、
  歌词失败、连接成功…），原先**写进去就再没人清** —— 用户某次控制失败留下的「没有可控制的播放设备」
  会一直挂在面板底部，于是「正在播放」与「没有设备」同屏出现（用户报的正是这个）。
  做法是**看门狗 `sweepMsg()`**：在 `tick` 里统计「同一段文字挂了多久」（`MSG_TTL_MS = 8000`，
  **按真实时间判、不按 tick 计数** —— tick 的真实周期是「轮询耗时 + 1000ms」，数 tick 会变成十几秒），
  换字即归零。放在 tick 里而不是每个写入处加定时器：逐处加必然漏一处，漏掉的就是又一条永久假状态。
- **播放态那行提示是绝对定位浮层 ⇒ 没字时必须不着色**（2026-09-28 用户报的「播放态底下有个
  透明黑色蒙版」）：它在播放态被改成 `position:absolute` + `pointer-events:none` 才不占那 130 的高度，
  但背景色挂在**元素**上、元素又自带 `min-height` + `padding` ⇒ 一个字都没有时照样铺出一条
  `rgba(0,0,0,.62)` 的横条（`sweepMsg` 清的是文本，不是这个元素）。修法是
  `.music-root.music-player-on .music-msg:empty { background:none; padding:0; min-height:0 }`
  —— 清空走的是 `textContent = ""`，正好命中 `:empty`。
- **轮询要快：宿主三处缓存 + 前端本地插值**（2026-09-29，用户报「每次动作到功能延迟长、进度条一跳一跳」）。
  口径是**先只提速、不删 Web API**（librespot 本地没有控制接口，删不掉它）。三条缺一不可：
  1. `music.rs` 的 `http()` **必须复用同一个进程级 client**（`static HTTP_CLIENT: OnceLock<Result<…>>`）。
     每次 `Client::builder().build()` 都是**一个全新的连接池** ⇒ 每个动作、每秒那轮轮询都要重做一次
     TCP + TLS 握手。这是「每次动作到功能之间那段延迟」里最大的一块。
  2. `load_config()` **必须走内存缓存**（`static CONFIG_CACHE: Mutex<Option<MusicConfig>>`）：
     它在 `spotify_status` 那条每秒路径上被反复调到，原先是「读盘 + 反序列化」× 30 处。
     **`save_config()` 写盘成功后必须同步这份缓存** —— 否则同一进程内立刻读到旧值
     （刚登录完 token 还是空的，等于把「登录状态丢了」这个最难查的 bug 请回来）。
  3. 进度条**必须本地插值**：那轮轮询的真实周期是「Web API 往返 + 1000ms」（实测被网络拉到 ~1.7s），
     只靠它写 `--seek-pct`，用户看到的就是「跳一格、僵一两秒、再跳一格」。`music.ts` 用一个
     `PROGRESS_TICK_MS = 200` 的 `setInterval` 调 `paintProgressSmooth()`，按**真实流逝时间**
     （`progressBaseMs + (Date.now() - progressBaseAt)`，夹到总时长）在两次轮询之间外推；
     `renderPlayer()` 每轮回来重新对齐基准。**纯本地计算，不增加任何 Web API 调用** —— 提速不提负载。
     只在「正在播 + 有曲目 + 没在拖（`seekRatio < 0`）」时推（暂停 / 拖动时位置本就该定住），
     定时器与 `pollTimer` 同生共死（`stopPolling()` 里一起 `clearInterval`）。


**歌词：三个来源，缺一不可**（`lyrics_get`）

1. `LRCLIB /api/get`（要 `track_name` + `artist_name`，`album_name` / `duration` 有就给）—— 精确命中最好；
2. `LRCLIB /api/search` —— 精确拿不到时的回落（Spotify 报的时长与库里录音版本常有 1–3 秒差 ⇒ 直接 404）。
   **LRCLIB 整条链路挂了（非 404 的错误 / 搜索请求失败）也要继续走第 3 级** —— 兜底的意义正在于此；
3. **网易云**（用户 2026-09-27 选定）—— 非官方只读接口，搜索 `/api/search/get/web` 拿 song id，再
   `/api/song/lyric?id=…&lv=-1&kv=-1&tv=-1` 取 `lrc.lyric`（`[mm:ss.xx]` 逐行）。
   **两个请求都必须带 `Referer: https://music.163.com/`** —— 缺了直接 `{"msg":"参数错误","code":400}`
   （2026-09-27 在目标机器实测，且与 UA 无关）。挑选顺序：歌名+歌手都对 → 只歌名对 → 第一条。
   全程**失败即 `None`**，绝不把「主源没有、兜底也挂了」升级成一次报错。

**前端不做手搜歌词**（2026-09-27 用户要求删掉搜索框与候选区）：歌词**只在换歌时自动抓一次**
（`lyricsFor` 记曲目 id）。原来那条「用户手选后不再被自动抓覆盖」的路径一并删除。

**端点改名（2026-09-27 实测，别再照记忆写）**

| 老 | 新 | 不改的症状 |
|---|---|---|
| `GET /playlists/{id}/tracks` | **`GET /playlists/{id}/items`** | 老路径直接 `403 Forbidden`（**与 scope 无关**，重新授权也一样） |
| 条目里 `{…, track:{…}}` | `{…, item:{…}}` | 取不到歌 ⇒ 展开后一首都没有 |
| 歌单对象 `tracks.total` | `items.total` | 面板上每个歌单的曲目数恒为 0 |
| `GET /me` 的 `product` / `country` | 已不返回 | **判不出账号是不是 Premium**，只能从错误里的 `Premium required` 反推 |

判据一句话：**「歌单能列出来、点开一首都没有」= 端点改名了**，不是权限问题，别去折腾 scope。
宿主机两个名字**都兜着读**（`.items` 优先、`.tracks` 回落），灰度期不会整块失效。

**被 Spotify 拒了要落盘 + 说人话**（2026-09-27 加）

- 宿主 `log_api_reject()` 在 `api_get_json` / `api_send` / `spotify_control` 三处统一记
  `spotify: {动作} 被拒（{状态码}）{端点路径} → {响应体前 200 字}`（**不记令牌**：我们调的 URL 本来就不带）。
  为什么必须落盘 —— 面板上那行小字会被下一轮轮询/重绘覆盖，用户报「操作被拒绝」时日志里**一个字都没有**，
  只能靠猜（这正是 2026-09-27 那次排查的开局：日志只记了「已授权成功」）。
- 前端 `errText()` 把 Spotify 的 `message` / `reason` 翻成**说清该做什么**的话（按**子串**判，不解析 JSON ——
  那条消息被截断成 200 字且带格式化换行，正则解析只会变成新的失败点）：
  `Insufficient client scope` → 去重新登录；`No active device` → 先在某台设备上播一次；
  `Premium required` → 控制播放要 Premium（看歌词 / 封面不受影响）；
  `Restriction violated` → 免费账号不能点播单曲 / 跳进度。
- 展开歌单失败**不许渲染成「这个歌单是空的」**（那会把真因藏掉）：原因贴在那一块里（`.music-empty-err`），
  下次点它会重试。本机音乐（`is_local`）没有 `spotify:` uri ⇒ 宿主机直接滤掉，面板不给「点了播不了」的死条目。

**四条不得回退的约束**：

1. **只走官方 OAuth（Authorization Code + PKCE + 环回地址）**。Spotify **没有**「账号密码异地登入」
   这类接口；桌面端正确姿势是 RFC 8252 的环回重定向。播放控制另需 **Premium** +
   `user-modify-playback-state`；读状态需 `user-read-playback-state` / `user-read-currently-playing`。
2. **Redirect URI 必须是 `http://127.0.0.1:<port>/callback`**：Spotify 自 2025-11 起对新应用
   **拒绝 `localhost`**（只接受 HTTPS 或 127.0.0.1/[::1] 字面量），且要求**逐字符精确匹配**
   （唯一豁免是环回 IP 的端口可动态分配）。端口固定在 `music.rs` 的 `DEFAULT_PORT`（8899，
   刻意避开 8788/8789 的本地服务与 9222 的调试口），面板上把完整回调地址显示出来供用户照抄。
3. **令牌过期要能自动救回来**：`ensure_access_token()` 在 `expires_at` 前 60s 主动刷新；
   **刷新被拒就清空本地令牌并报 `ERR_NOT_CONNECTED`**，让面板退回「未登录」而不是反复 401。
   刷新响应里**可能不带新的 refresh_token**，此时必须保留旧的（清掉等于把用户踢下线）。
4. **Client ID 的默认值走 `serde(default = "default_client_id")` 而不是 `#[serde(default)]`** ——
   两者差别就在「老配置文件里没有这个字段」时：前者补内置值，后者补空串，于是老用户
   明明有内置值可用却仍然被判成「未配置」。面板不允许保存空 Client ID（前端会拒），
   所以「文件里是空」只可能来自老版本，补内置值正是想要的。

**边界要诚实**（这一节直接决定用户会不会白折腾）：

- 这个插件控制的是「当前活跃的 Spotify Connect 设备」（本机 Spotify 客户端即可）。
  **在 Lunac 里出声**有两条路，我们都**不走 Web Playback SDK**：那条要 Premium + EME/Widevine DRM，
  而 WebView2 里能不能过 DRM 一直不确定；桌面端控制的正解是 **Web API**（`/me/player*` 那组端点）。
  我们选的是**另一条**：**librespot 独立进程**（见下）—— 它自己就是一台 Connect 接收器，
  于是「登入一次后不必开 Spotify 桌面端也能出声」，而播放控制仍然全部走 Web API。
- **librespot（非官方 Connect 接收器）= 「登入一次后脱离桌面端出声」的唯一现实路径；已实测能出声
  （2026-09-28）**。要点（每条都是实测踩出来的，别照直觉写）：
  1. **它必须走代理**。直连时日志刷 `Audio key response timeout` → `continuing without decryption`
     → 一堆 `invalid mpeg audio header`（拿到的是**没解密**的字节）；**音频密钥走的是另一条到 AP 的 socket**，
     那条路被挡就是这个现象。`-x/--proxy URL` 实测**连 AP 与密钥两条 socket 一起走**
     （日志里 `librespot_core::socket] Using proxy "…"` 出现**两次**），挂上本机代理后
     `Audio key response timeout` **0 次**、音频被正确解成 Ogg Vorbis、Web API 回读 `playing=true`
     且进度按实时推进。⇒ **「密钥超时」是网络路径问题，不是账号或代码问题。**
  2. 命令（凭据落盘后可复用，不必重新登录）：
     `librespot --name "<名字>" -c <cache目录> -x http://127.0.0.1:7892 --bitrate 320`。
     `-c` 指到上次那个缓存目录，它自己会 `credentials.json` 续用。
  3. **构建**：0.8.0 在 Windows 上**不需要 OpenSSL / Bonjour / protoc**（默认特性 = `native-tls`(SChannel)
     + `rodio`(WASAPI) + 纯 Rust mDNS）；`cargo install` **必须加 `--locked`**，不加会撞上游 `vergen`
     双版本冲突（`error[E0277]: … vergen_lib::entries::Add`）。
     **产物怎么到用户手里（2026-09-28 落地）**：上游**一个二进制都不发**（v0.8.0 / v0.7.x 三个
     release 的 `assets` 全是空数组，别再去上游翻），所以我们自己构建的那份挂到**公开插件仓库的
     Release 资产**（`LythrumMoon/lunac-plugins` 的 `librespot-0.8.0`，资产名 `librespot.exe`），
     由 music 插件的 `dependencies[]` 拉到 `<exe 根>\Modules\music\bin\librespot.exe` ——
     `find_librespot()` 认这条路径。实测那份 **36637180 字节**、
     `sha256=7509c74b1be2bdcd8debcf6575e557f31db80ea0512f157872429b11e92a4c1a`
     （本地 `Get-FileHash`、GitHub 自己报的 `digest`、免鉴权下载回来的字节三者一致；
     构建耗时 3m16s）。本机中转目录是 `release\deps\`（`release\` 整体 gitignore）。
  4. **已接进 Lunac**（2026-09-28，用户选定「现在集成（脱离桌面端出声）」）：宿主 `music.rs` 管子进程
     （`librespot_status` / `librespot_start` / `librespot_stop` + `kill_librespot()`），前端把它做成
     设备弹层里的**第一行开关**（「本机播放」）。四条实现纪律：
     ① **路径与代理都是配置项**（`librespot_path` / `librespot_proxy` 两个字段，留空 = 自动探测 / 直连）——
     实测**直连有时也行**（取决于网络路径），所以「必须代理」不能写死；
     ② **它挂了 / 应用退出必须立刻摘设备**：`librespot_stop` **先 pause 再 kill**，`main.rs` 的
     `Destroyed` 分支调 `kill_librespot()` —— 半死状态仍占着会话，会诱发 `NO_ACTIVE_DEVICE`（2026-09-27 踩过），
     而下次开机它还会以一台永远连不上的「Lunac」出现在设备列表里；
     ③ **它是独立进程**，WebView 里起不了进程，只能由宿主起停；起进程要带 `CREATE_NO_WINDOW`（否则闪一个黑框），
     并且 **stdout / stderr 两个管道都要排空**（塞满即双向死锁，与 `convert.rs` 预检 #36 ① 同一条纪律），
     排空时顺手转进本仓日志 —— `Audio key response timeout` 是**唯一**能诊断「在跑但没声音」的线索；
     ④ **凭据缓存是那条真正的门槛**（2026-09-28 实测）：`-c <dir>` 是**音频**缓存，`credentials.json`
     跟着它落盘（`--system-cache` 未指定时默认取 `-c` 的值）。**一份空缓存 = librespot 静默连不上**：
     进程在跑（`running=true`）、音频后端初始化完成、日志里一行错误都没有，但 `/me/player/devices`
     **永远是空数组**。所以「本机播放」第一次用必须有一次**交互式登录**（`librespot -j/--enable-oauth`，
     浏览器里授权一次，凭据落盘后就不必再来）。宿主现在的形态是「用已有凭据的缓存直接起设备」，
     **首次登录那条流程还没做**（见下「已知缺口」）。
     另：`-k/--access-token` 也能起，但 access token 一小时就过期 ⇒ 不适合当长驻设备。
- **已知缺口（2026-09-28 验收时确认）**：
  1. **librespot 的首次登录没有入口**：宿主不会带 `-j/--enable-oauth` 起它、也不会把那个授权 URL 交给用户，
     所以在一台**从没登录过 librespot** 的机器上点「本机播放」只会得到一个「在跑但设备列表里没有它」的状态。
     现在靠的是「把一份已有 `credentials.json` 放进配置里的缓存目录」—— 缓存目录默认是
     `<exe 目录>\config\librespot-cache`。
  2. **加了 `user-follow-read` ⇒ 必须重新登录一次 Spotify**，否则左栏「歌手」如实报
     `403 Insufficient client scope`（面板会把这句翻成「请点『连接 Spotify』重新登录一次」）。
- **控制播放（play / pause / next / previous / seek / volume）必须有 Premium**，
  这是 Spotify 的规则，与 Lunac 无关；**只看歌词 / 封面 / 正在播放则不需要 Premium**。
  文案里必须把这两件事分开说（`music.setup_builtin` / `music.setup_hint` 就是这么写的）。
- **两条 API 硬限制（做不出来，也不假装能做）** —— 2026-09-27 查证 + 实测确认：
  1. **没有「智能随机」（Smart Shuffle）接口**：`GET /me/player` 只给 `shuffle_state`(bool) +
     `repeat_state`(off/context/track)，**读都读不到** —— 它是手机/桌面客户端的本地功能。
     所以控制条那个三态按钮只能是 **关 / 随机（shuffle）/ 单曲循环（repeat=track）**（用户 2026-09-27 选定）。
  2. **没有「删除队列项」接口**：队列只有 `GET /me/player/queue`（读）与 `POST /me/player/queue`（加到下一首）。
     于是「播放列表里移除某一首」用 **「点它 = 从它开始播」** 代替（`spotify_play_uri`，等价于丢掉它之前的队列项）。
     **文案必须写「从这首开始播」，不许写「移除」**（`music.play_from_here` 里带括号如实写明原因）。
- **Development Mode 的硬限制**（2026-02-11 起，老应用宽限到 2026-03-09）：
  应用所有者**必须持有 Premium**（掉订阅就整个应用停摆）；**每个应用最多 5 个授权用户**
  （含所有者自己，要在仪表盘的 User Management 里逐个加白）；**搜索类端点每次最多 10 条**；
  一部分端点被关掉。**解除限制要申请 Extended Quota Mode**，而那条路自 2025-03 起只发给
  「**合法注册的组织** + **月活 ≥ 25 万**」的申请者 —— 个人拿不到，**不要**在任何文案里
  承诺「以后能支持更多用户」。2026-07 起一个开发者可建的 Client ID 数从 1 提到 25，但
  **名额（5 用户/应用）没变**，多建几个 Client ID 只是绕开「1 个 ID」的限制。
- **refresh token 会在用户首次授权后 6 个月硬过期**（刷新 access token 不会延长它），
  所以正式发出去的版本要预期「约每半年重新登录一次」，面板的报错要能被用户看懂。
- 公开只读的歌词源是 LRCLIB（免费、无需 key，要求带 `User-Agent` 并遵守 429 的 `Retry-After`），
  与 Spotify 的授权完全无关 —— 没登 Spotify 也能用歌词。**兜底源网易云是非官方接口**
  （必须带 `Referer`，见上）⇒ 它随时可能失效，所以口径是「**拿不到就当没有歌词**」，
  绝不让兜底的失败影响主流程（第 2 级那两条 LRCLIB 失败路径都写成 `unwrap_or_default()`）。

### 4.7 文件转换插件（图片 / 音频 / 视频，2026-09-27）

用户选定范围：图片格式互转、音频格式互转、视频格式互转。引擎 = **本机 ffmpeg**，
前端插件只画界面，**转换必须由宿主做** —— 插件跑在 WebView 里，没有执行外部进程的能力
（与 §4.6「联网必须走宿主」是同一条纪律，code-rules 预检 #36）。

| 宿主命令（[convert.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/convert.rs)） | 作用 |
|---|---|
| `convert_engine_status` | 找 ffmpeg / ffprobe（**exe 根自带 → PATH**，只找不下载）并回报版本 |
| `convert_probe` | 按后缀给出 `kind` / 体积 / 时长 / **可选目标格式** |
| `convert_run` | 跑一次转换；进度经 `convert-progress` 事件回传 |

**一个引擎覆盖三类**：ffmpeg 同时能读写图片（png/jpg/webp/bmp/tiff/gif）、音频
（mp3/wav/flac/m4a/aac/ogg/opus）、视频（mp4/mkv/webm/avi/mov/gif），所以不引第二套实现。
曾考虑用 `image` crate 单做图片，但那样就有两条路径、两套错误语义，且 `image` 的 webp
**只有解码**。目标格式表刻意只列 ffmpeg 稳的（不加 avif / heic / ico：前者看构建里的编码器，
后者 ffmpeg 写出来容易是坏的）。

**四条不得回退的约束**：

1. **ffmpeg 不随包分发**（体积 + 许可）。找不到就在面板上如实说「未找到 ffmpeg」并给出装法，
   同时禁掉选择按钮 —— 与 OCR 引擎缺失、插件市场拉不到索引是同一条「坏状态要可见」的纪律。
2. **输出路径由宿主算**，前端只传目标扩展名。三条硬约束：① 绝不等同源文件（否则就是覆盖
   用户的原始素材）；② 已存在就加 ` (1)` / ` (2)` 递增，静默覆盖是最不该发生的事；
   ③ 失败要把半成品删掉，别在用户目录里留一个「转坏了的文件」。
3. **子进程的 stderr 必须在独立线程里排空**：stdout（`-progress pipe:1`）与 stderr 两个管道
   都塞满时（ffmpeg 报错刷屏），主线程只读 stdout ⇒ 子进程写阻塞 ⇒ **双向死锁**。
4. **进度诚实**：`-progress` 的 `out_time_us` 实测是**微秒**（ffmpeg 8 里同行的
   `out_time_ms` 同样是微秒 —— 只认 `out_time_us`）；`ffprobe` 探不到时长（图片就是没有
   时长这个概念）时 `percent = -1`，面板切成**不确定态**进度条，**不编一个假百分比**。

**无音轨的视频不给音频目标**：录屏、静音素材抽不出音轨。`convert_probe` 用**一次** ffprobe
同时拿时长与 `codec_type`，没有音轨的视频就不列音频目标（`targets_for_input()`，有单测），
`convert_run` 用同一判据拒绝 —— 否则用户会看到 `Output file does not contain any stream`
这种看不懂的报错。

**探测失败一律按「有音轨 + 时长 0」处理**：拦不住就交给 ffmpeg 如实报错，
好过因为探测本身失败而凭空砍掉候选。

**附件入口**：附件的后缀属图片 / 音频 / 视频时，搜索结果里多一条「文件转换」，
点它即把该文件预置为源文件（`window.__lunac_convert_file`）。前端只做**粗筛**
（一张正则），后缀不认识时插件会如实说「不支持」，所以不必与 `convert.rs` 的格式表逐项对齐。

### 4.8 插件悬浮窗（多窗口基础设施，2026-09-27）

**为什么加**：在此之前整个项目**只有 `main` 一个窗口**（`tauri.conf.json` 的
`app.windows` 只有一项，全仓没有任何 `WebviewWindowBuilder`）。插件面板是**内嵌**在
主窗口 `#results-list` 里的，于是 `runSearchNow()` 开头那句 `if (pluginActive) return;`
会把「插件开着时用户敲进搜索栏的每一个字」**整段丢弃** —— 想看插件就用不了搜索。
用户要求「插件窗口与搜索窗同时存在」，所以这里加一套真正的多窗口。

| 宿主命令（[plugin_window.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/plugin_window.rs)） | 作用 |
|---|---|
| `open_plugin_window(plugin_id, input)` | 建窗或**复用**（同 id 不建第二个）；复用路径只 `unminimize + show + set_focus` 并推一条 `plugin-window-input` |
| `plugin_window_init` | 窗口自己来取启动载荷（`{plugin_id, input}`） |
| `plugin_window_close` / `plugin_window_minimize` | 关 / 最小化自己 |
| `plugin_window_set_pin` / `plugin_window_pin_state` | 置顶开关 / 读当前置顶态（前端**不许猜**初始值） |
| `plugin_window_set_click_through(ignore)` | 切**鼠标穿透**（2026-09-29）。**只认 `plugin-` 前缀的窗口** —— 理由见下 |
| 事件 `plugin-window-visibility` `{label, visible}` | 宿主 → 插件窗：这个窗**现在可见吗**（2026-09-29）。**只在翻转时发**，且**必须带 `label`** —— `Emitter::emit` 是广播，同时开两个插件窗时不给 label 会让另一个窗把别人的事当成自己的 |

**为桌宠补的两个前置（2026-09-29，实测数据与取证方式见 [architecture-rendering.md](./architecture-rendering.md) §6.2）**：

1. **鼠标穿透必须走宿主命令，不能直接放开 core 权限。** `core:window:allow-set-ignore-cursor-events` 是**按窗口**授的，而 `capabilities/default.json` 的 `windows` 里含 `main` —— 一旦放开，任何一处前端 bug 都能把**主窗口**变成穿透的，而穿透后的窗口**收不到鼠标**，用户没有任何办法点回来（只能去杀进程）。所以收成一条只认 `LABEL_PREFIX` 的命令。回值 = **本次下发的值**，不是回读（tao 没有 `is_ignore_cursor_events()`）。
   实测（`WindowFromPoint` + `GWL_EXSTYLE`）：开之前 `exStyle=0x40118`、命中点落在插件窗自己；开之后 `0x40138`（`WS_EX_TRANSPARENT|WS_EX_LAYERED`）、**同一个点落在它下面的另一个应用的窗口**；关掉即全部复原；主窗口调用回 `ERR_NOT_PLUGIN_WINDOW`。开/关两次的窗口截图 mean |dRGB| = 3.64（**渲染没有被穿透破坏**，差异只是毛玻璃实时抖动）。
2. **「现在可见吗」WebView2 报不出来，只能由宿主显式下发。** Win32 的 `IsWindowVisible()` 对**最小化**窗口返回**真** ⇒ `document.visibilityState` 永远是 `visible`、`visibilitychange` **永不触发**、rAF 在最小化后仍按刷新率满速跑（实测 ~170fps / **~5% 单核**）。落点是 `plugin_window::announce_visibility()`，**两个触发点缺一不可**：① `main.rs` 的 `WindowEvent::Resized`（tao 把 `WM_SIZE`（含 `SIZE_MINIMIZED`）统一发成它 —— `WindowEvent` 里**没有** `Minimized` 这一项）；② `open()` 的**复用路径**（`show()` 不产生 `WM_SIZE`，所以「插件自己 hide ⇒ 宿主再打开」这一声必须由主动方经 `announce_visibility_now()` **无条件**喊，否则插件恢复不了动画）。
   **只在翻转时发**：拖拽缩放时 `Resized` 每移动一像素来一条，记账在 `LAST_VISIBLE`（`note_destroyed()` 里清）。实测两轮「最小化 → 还原」恰好收到 `false,true,false,true` 四条。

**窗口形态**：`label = plugin-<id>`、默认 420×560（最小 300×200）、`decorations:false` +
`transparent` + `always_on_top`、可缩放、进任务栏，**这几项可被插件清单的 `window` 段覆盖**
（见下）。前端入口是**独立的 `plugin.html` + `plugin-window.ts`**（vite 多页入口），
不是把 `main.ts` 跑两遍。

#### 窗口形态由插件清单声明（2026-09-29，桌宠 L1）

**要解决的问题**：`open()` 原先对所有插件用同一套建窗参数（420×560 / 可缩放 / 进任务栏 /
带标题栏）。桌宠要的是「定尺 + 禁缩放 + **不进任务栏** + **没有标题栏**」，而宿主**不允许按 id 写死**
（拓展插件要能独立打包，宿主在真机上不认识它们）。

| 落点 | 内容 |
|---|---|
| 清单字段 | `PluginManifest.window: Option<PluginWindowShape>`（`plugin_market.rs`）—— `width/height/minWidth/minHeight/resizable/skipTaskbar/alwaysOnTop/chrome`，**全部有缺省值，缺省值逐项等于加这个字段之前的行为** |
| 校验 | `validate_window_shape()`：尺寸必须是 `0`（= 用宿主缺省）或 100~4000；`min > 初始` 直接**拒整包**（那种清单在建窗时会被系统夹一次，表现是「命令成功、窗口没动」） |
| 建窗 | `plugin_window::declared_shape()` 读 `<exe 根>\Modules\<id>\lunac-plugin.json`（**读不到 = 没声明**，不报错：编译进主程序的插件盘上本来没有目录）；`default_size()` / `min_size()` 收 `Option<&PluginWindowShape>`，只写一半时另一半仍走宿主缺省 |
| 标题栏 | 形状里的 `chrome: false` 由 `plugin_window_init` 回给前端（新增 `chrome` 字段），`plugin-window.ts` 在 `<html>` 上挂 `no-chrome`；`styles.css` 的 `html.no-chrome` 那组规则**收起标题栏 + 去掉结果区玻璃底/描边/毛玻璃 + 垫料归零** —— 那层 `#results-container::before` 是整个窗唯一的色块，去掉它才是真透明 |

**⚠️ 两条实测出来的坑，别按直觉写**：

1. **`skip_taskbar` 不能靠 `GWL_EXSTYLE` 判**。tao 的 `with_skip_taskbar(true)` 落在
   `ITaskbarList::DeleteTab(hwnd)`（tao `window.rs:1334` → `set_skip_taskbar`），
   而 `WS_EX_APPWINDOW` 是它给**无父窗口**无条件加上的（`window.rs:1164` 的
   `WindowFlags::ON_TASKBAR`）—— 两者互不影响，只看 exStyle 会得出「没生效」的错误结论。
   **正确判据**是查任务栏本身（本仓用 UI Automation 列 `Shell_TrayWnd` 下的按钮名，
   实测：只开桌宠窗时任务栏**没有** Lunac 条目；开一个没声明 `window` 段的窗（音乐）
   立刻出现 `Lunac - 1 个运行窗口` —— 对照成立）。
2. **透明性要用「同一矩形的前后对照」证，不能用别处的桌面色当对照**——桌面上不同位置的
   颜色分布差得非常远（实测同一屏内一块 300×400 是 `14950` 种色 / 最大单色 29.7%，
   另一块是 `486` 种 / 92.7%）。做法：记下窗口矩形 → **把它最小化**（或先别开）→ 截同一矩形
   拿到「它背后真正是什么」→ 恢复后再截一次 → 两组统计一致才算透明。实测（内容整块藏起时）
   `504 种色 / 最大单色 92.4%`，最小化后同矩形 `486 种 / 92.7%` ⇒ 窗内像素几乎逐点等于
   「没有这个窗」时的画面。

**第一个消费者：桌宠插件（`Modules\pet\`，2026-09-29）**。它的形态是
`{width:300, height:400, minWidth:160, minHeight:200, resizable:false, skipTaskbar:true, chrome:false}`，
并且**刻意不声明 `window.float`** —— 搜索打开的是它的**控制台**面板（导入形象 / 穿透开关 /
形象大小），桌宠窗由控制台里的「显示桌宠」开。**这不是设计偏好**：穿透开着时桌宠窗
**收不到任何鼠标事件**（那正是它的用途），开关若只放在桌宠窗自己的右键菜单里，
用户一按下去就再也关不掉（只能去杀进程）。⇒ 分工固定为「**桌宠窗负责做**（所有窗口操作
只能由它自己发，命令拿的是调用方那个窗口）、**控制台负责说**（写配置 + 广播一条
`pet-control`）」。回归口径：控制台勾选「鼠标穿透」→ 桌宠窗 `GWL_EXSTYLE` 从 `0x40118`
变 `0xC0138`（`WS_EX_TRANSPARENT|WS_EX_LAYERED`）；取消勾选即复原。


**五条不得回退的约束**：

1. **插件窗口必须是「惰性」的。** 宿主有一批**全局单值**状态 —— `hotkey::UI_MODE` /
   `DETACHED` / `QUERY_EMPTY`、`MAIN_HWND`，以及 `main.rs` `on_window_event` 里
   `Destroyed` 的全局清理（`cli_bridge::kill_and_cleanup()` + `kill_port(5173)` +
   `agent_server::stop()`）。这些**全都只描述主窗口**。所以：
   `on_window_event` 必须按 `window.label()` 分流；插件窗口的前端**绝不**调
   `set_ui_mode` / `set_detached` / `set_query_state` / `hide_lunac`；
   热键与 Esc 逻辑一律只认 `MAIN_HWND`（原本就如此，不要改）。
   违反任何一条就是「关掉一个音乐小窗把整个 agent 后端清掉」这类灾难。
2. **失焦守卫必须放行。** `hotkey.rs` 的轮询里有「可见但不在前台 ⇒ 2s 后自动隐藏」。
   用户点插件窗口时主窗口必然不在前台 —— 不放行的话，「两窗同时存在」这条需求
   会在 2 秒后被守卫自己推翻。落点是 `plugin_window::OPEN_WINDOWS` 这个**计数**原子量
   （用计数不是 bool：同时开两个小窗时，关掉一个不该让守卫重新生效）。
3. **`capabilities/default.json` 的 `windows` 必须含 `plugin-*`** —— 它是 host 命令的
   授权清单，label 不在里面连 `set_always_on_top` 都会被拒。窗口 label 的前缀常量
   （`plugin_window::LABEL_PREFIX`）与这个通配必须**逐字对齐**（有单测钉住）。
4. **参数传递不走 URL query。** `WebviewUrl::App` 的路径经 url join 处理，把
   `?id=…&input=…` 塞进 `PathBuf` 是在赌它的拼接实现。改成「宿主先把载荷存进
   `PENDING`、前端启动后自己 `plugin_window_init` 来取」—— 没有时序问题也不用转义
   （页面可能比命令返回更快，所以载荷必须在建窗**之前**写好）。
5. **外观不在悬浮窗里自己算。** `applyAppearance` 那一整套（主题包 tokens → 底色 /
   按钮色 / 文字明度 / 统一浮层的派生）有近百行，复刻一份必然漂移。落点是
   **主窗口广播它算好的内联 CSS 变量**（`lunac-theme-vars`），悬浮窗原样套用；
   悬浮窗启动时发一次 `lunac-theme-request`（主窗口那份一次性的 applyAppearance
   不会为迟到的窗口重跑）。名字表**只增不减**，清掉的变量要广播成空串，
   否则悬浮窗会留着上一个主题的值。

**可悬浮的插件是一份显式清单**（`main.ts` 的 `FLOATABLE_PLUGINS`）：音乐 / 转换 /
备忘录 / 剪贴板历史 / 快速启动 / 工具编辑 / 网页搜索 / 翻译。**不含** `settings`
（按 800px 宽 + 左侧栏分类设计，小窗里会散架）、`ocr`（detached 双栏，同 settings 的
理由），以及 `ai-agent` —— 它走**专用通道**：不进 `FLOATABLE_PLUGINS`、也不走
`plugin.execute` 的渲染，而是由 `openChatWindow()` 直接开**聊天独立窗**（见下一条）。

**聊天独立窗（2026-09-29，用户定：「ai 插件也需要独立界面状态……并去除小窗口和大窗口，
独立界面尺寸设置成与音乐插件相同的」）**：AI 聊天的界面**整体搬进一个独立的无边框窗口**，
主窗里的内嵌小面板（360）与 detached 600 大窗**两态都去掉**。

| 项 | 约定 |
|---|---|
| 窗口身份 | `pluginId = "chat"`（`plugin_window::CHAT_WINDOW_ID`）⇒ label **`plugin-chat`**。**刻意落在 `plugin-*` 通配里**：`capabilities` 授权、`on_window_event` 按 label 分流、`OPEN_WINDOWS` 失焦放行、`Destroyed` 不跑全局清理 —— 插件窗那一整套基础设施**全部自动复用**，一条都不用重写 |
| 尺寸 | **1280×720**（与音乐默认态同档，用户原话「与音乐插件相同的」），最小 **720×420**（这条界面有输入栏 + 历史抽屉 + 工具卡，缩到通用那档 300 宽会散架）。落在 `default_size()` / `min_size()` 的一档特例里 |
| 加载哪个页面 | **`index.html`**（不是 `plugin.html`）—— **这是本条最关键的选择**：聊天的界面（`#results-list` 的对话流、输入栏、历史抽屉、任务抽屉、审批卡）与逻辑（流式渲染、会话、审批回传）就是主界面那一份，**一行都不用搬进 `plugin-window.ts`**。`open()` 里按 id 选页面，别的插件仍是 `plugin.html` |
| 前端怎么知道自己是谁 | `getCurrentWindow().label === "plugin-chat"`（**同步**）。**不许改用 URL query** —— 规则 4 明写了 `WebviewUrl::App` 的路径要经 url join，塞 `?view=chat` 是在赌它的拼接实现 |
| **分工（硬约束）** | 两个窗口跑**同一份 `main.ts`**，按钮由 `IS_CHAT_WINDOW` 判定切分：**主窗** = 搜索 / 插件面板 / 设置 / 详情 / 托盘 / 热键 / 剪贴板 / `set_ui_mode` / `set_detached` / `set_query_state` / `hide_lunac`；**聊天窗** = `cli-output` / `cli-status` / `cli-stderr`（对话流）与聊天界面。**`emit` 是广播**：`cli-*` 只能一个窗口消费，否则双渲染、双写会话 |
| 收口点（四处，都是「漏一处就出错」的地方） | ① **`invoke` 包装**（`MAIN_WINDOW_ONLY_CMDS` = `set_ui_mode` / `set_detached` / `set_query_state` / `hide_lunac` / `set_chips_empty`）—— 那些调用散在十来处，逐个加 `if` 必漏；② `applyWindowSize()` / `syncUiMode()` 开头早退（尺寸驱动与界面层都是主窗的）；③ `onResized` 早退（聊天窗定尺，不做 zoom 与内容驱动高度）；④ 7 条事件归属：`cli-*` 三条只聊天窗收，`lunac-clipboard` / `lunac-window-shown` / `lunac-esc-clear` / `lunac-esc-cancel-rec` 四条只主窗收 |
| 入口（主窗侧） | ① `executePlugin()` 里 `plugin.id === "ai-agent"` **在调 `plugin.execute` 之前**转 `openChatWindow()`（否则会先在主窗长出一个空面板）；② `startAIChat()` 开头分流 —— **主窗调用一律转开窗**。这一处覆盖全部入口（搜索命中 / 右键「问 AI」/ 历史抽屉 / 去痕迹…），调用点有十几处，收口在这里才不漏 |
| 传参 | 开窗时宿主把「那句话」写进 `PENDING`，聊天窗启动后 `plugin_window_init` 取走（take 语义）；**窗口已开着**时宿主不建新窗、改推 `plugin-window-input`，聊天窗监听它并 `startAIChat(q)` |
| 外观 | 聊天窗启动时 `setDetached(true)` —— **借主窗「分离态」那套外观**（`#app.detached` = padding 2px + `#results-container` 圆角玻璃底 + 撑满 + `#detached-header` 当窗口标题栏）。`set_detached` 那条命令由 `invoke` 包装拦掉，所以不会污染 Rust 的 `DETACHED`。**VSCode 按钮**（用户要求「装入独立界面状态」）就是 `#detached-header` 里的 `#detached-vscode-btn`，`setDetached` 里按 `activePluginId === "ai-agent"` 显示 —— 不必新写一个按钮 |
| 关闭语义 | 窗口标题栏的 × = **关掉这个窗口**（主窗那套是「退出插件、回到搜索栏」，独立窗没有搜索栏可回）。再进来：搜索命中 AI 重新开一个 |

**不得回退的四条**：① `cli-*` 只由聊天窗消费（广播禁令）；② 聊天窗**绝不**发 `set_ui_mode` /
`set_detached` / `set_query_state` / `hide_lunac`（全靠 `invoke` 包装那一道闸）；③ 聊天窗**绝不**注册
剪贴板 / 热键 / 搜索链路；④ 主窗**绝不再长回**内嵌聊天（所有「进入 AI」的入口必须汇到
`openChatWindow()`）。

**已知代价（第一版刻意保留）**：聊天窗跑的是同一份 `main.ts`，因此主窗那些**与全局单值状态
无关**的初始化（注册内置插件、扫插件目录、拉市场索引）也会跟着跑一遍 —— 功能上无害，只是多一次
开销。**清理它们属于后续优化**：第一版优先保证「聊天行为与它在主窗里逐字节一致」，多跑几步初始化
换「零搬迁」是划算的；要紧的是**没有**多注册剪贴板 / 热键 / 对话流那几条（那几条才是会互相抢的）。

**实测（2026-09-29，第一版）**：`npx tsc --noEmit` **exit 0**；宿主 `cargo check --bins` exit 0、
`cargo test --bins` **112 passed / 0 failed / 1 ignored**（新增两条守门单测：`chat` 那一档的
尺寸与「最小 ≤ 默认」、以及**「只有聊天窗加载 `index.html`」**）。**实机回归未做** —— 逐条验收项见
[backlog](./agent-feature-backlog.md) **L7**（开窗 / 窗内聊天与审批 / `×` 关窗后重开 /
**与音乐窗并存互不串扰**）。

**挂载只有一处**（[plugins/attach.ts](file:///d:/cc/claude-code-cli-master/app/src/plugins/attach.ts)）：
主窗口的内嵌面板与悬浮窗共用同一份 `attachPluginListeners(plugin, root)` 映射。
分两份写的话，加了插件只改一边就会出现「内嵌能用、悬浮窗是死的」。
好消息是**内置插件模块都不依赖 `main.ts`**（只依赖 registry / i18n / Tauri API），
独立入口因此不需要把 main.ts 拆开。

### 4.9 翻译插件（词典 + 模型补漏 + 译文缓存，2026-09-29）

用户 2026-09-29 的第三条需求：「**词典做底座 + 模型补漏 + 译文存数据库（避免二次翻译）**，
作为基础插件」。实现在
[app/src-tauri/src/translate.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/translate.rs)（宿主：联网 + 缓存 + 模型）
与 [app/src/plugins/builtin/translate.ts](file:///d:/cc/claude-code-cli-master/app/src/plugins/builtin/translate.ts)（面板）。

**为什么联网必须在宿主**：前端插件在 WebView 里，CSP 是 `default-src 'self'
https://asset.localhost`，插件里的 `fetch()` 会被直接拦掉（与音乐歌词 / 插件市场索引同一条，
见 §11 规则 67 的第 ⑥ 条）。

| 层 | 端点 / 落点 | 给什么 | 花钱 |
|---|---|---|---|
| **① 缓存** | `<exe 根>\ModuleData\translate\cache.db` 的 `translations` 表 | 同一 (源语言, 目标语言, 原文) 只查一次外部接口 | 否 |
| **② 词典底座：译文** | `api.mymemory.translated.net/get`（免 key） | 主译文（整句 / 短语 / 单词都行） | 否 |
| **② 词典底座：词条** | `dict.youdao.com/jsonapi`（免 key，**非官方只读**） | 音标（英 / 美）、释义行、双语例句；**整句**查询时它的 `ec.word[0].trs` 也是译文候选 | 否 |
| **③ 模型补漏** | `{agent_endpoint}/v1/messages`（与 core-agent 同一套形状） | 上面两层都没有结果时，**由用户点按钮**才发起 | **是** |

**免 key 源是实测选出来的，不是照记忆写的**（2026-09-29，本机**直连**、不走系统代理）：

| 候选 | 结果 |
|---|---|
| `translate.googleapis.com/translate_a/single`（免 key 的 Google） | **不通**（curl 000，直连与经代理都一样）⇒ **不实现**：写成「首选 + 失败降级」只会让每次查询先白等一个超时 |
| `api.mymemory.translated.net/get` | 200；`hello`→你好、`你好，今天天气不错`→英文、139 字整段无截断 |
| `dict.youdao.com/jsonapi` | 200（`hello` 54 KB / `give up` 21 KB 词条详情） |

**语言判定在本地做，不用对方的能力**：MyMemory 的 `langpair=Autodetect|…` 实测**不可靠**
（`hello world` 被原样返回，等于没翻译）⇒ `detect_lang()` 只判「含不含 CJK 字符」
（中日韩 vs 其余），够用且完全确定、不引依赖。用户显式选了源语言就**不猜**（`resolve_from`）。

**译文缓存表**（`IF NOT EXISTS` 幂等，与 `chat_db.rs` 同一套做法，无 `user_version` 簿记）：

```sql
CREATE TABLE IF NOT EXISTS translations (
    key        TEXT PRIMARY KEY,   -- "源语言\u{1}目标语言\u{1}原文"
    src        TEXT NOT NULL,
    dst        TEXT NOT NULL,
    source     TEXT NOT NULL,      -- dict | ai
    payload    TEXT NOT NULL,      -- TranslateResult 的 JSON
    created_at INTEGER NOT NULL
);
```

- **独立库、不塞进 `chat.db`**：`chat.db` 是「对话历史」，把词条缓存混进去会让
  「history 里到底存了什么」变得说不清。业务数据都在 `ModuleData\` 下，这里另开一个目录。
- **缓存键不做大小写 / 全半角归一**：归一会让 `Hello` 与 `hello` 共用一份译文，
  而它们的词典释义常有差别（有道自己就分条），省下的那点空间不值一次「查的词和返回的词
  不是一个」的困惑。
- **只缓存有内容的结果**：把「查不到」也写进去的话，一个还没收录的词会被永久钉死成查不到
  —— 而那正是下次可能查到的那个词。

**两条命令**（`Result<_, String>`，均 `async` + `run_blocking`，见 §11 规则 68）：

| 命令 | 何时调 | 失败语义 |
|---|---|---|
| `translate_lookup(text, from, to)` | 用户点「翻译」 | **除空输入 / 超长外永不 Err**：网络故障、被反爬、额度用尽都只是「这次没查到」⇒ 返回 `source: "none"`，由面板提示走模型。**不许**把它变成一行红色错误（那会把「对方今天不通」说成「你这个词有问题」） |
| `translate_ai(text, from, to)` | **只在用户点「用 AI 翻译 / 重译」时** | 真实报错（用户主动点的动作必须可见），但**只给「用户能做什么」**：`401/403`→「凭据无效，请到设置里检查」、`404`→「端点或模型名不对」、`429`→「请求太频繁」、`5xx`→「暂时不可用」，**状态码与响应体只进日志**（预检 #40 ③） |

返回形状 `TranslateResult{ text, from, to, source, cached, translation, phonetic, explains[], examples[] }`：

- `source` = `dict` / `ai` / `none`（**三层真实来源**，`cached` 另用一个布尔标「这次没碰网络」）。
- 同一句问第二次走缓存 ⇒ `cached: true`（实测 6~7 ms 返回，不再出网）。
- 命中缓存时**不跳过**用户主动点的「用 AI 重译」：缓存里已是 AI 结果才直接复用
  （拿旧的词典结果把他挡回去等于按钮没反应）。

**界面**（用户 2026-09-29 选的是「**可悬浮 + 主窗面板**」，所以它进了 `FLOATABLE_PLUGINS`）：

- 主窗内嵌面板 = 输入框 + 源语言 / 目标语言 + 译文（大字）+ 音标 + 释义列表 + 例句；
  Enter 翻译、Shift+Enter 换行。
- 悬浮窗（420×560）走 `plugin-window.ts`，与内嵌版共用 `attach.ts` 那一份挂载表。
- **花钱的动作只在结果出来之后出现**：词典查不到 → 「用 AI 翻译」；词典查到了 →
  「用 AI 重新翻译」。绝不自动替用户出网去问模型。
- 来源只用一行小字自明（来自词典 / 来自 AI / 来自本地缓存），**不弹提示**（预检 #40）。

**搜索栏带过来的待译内容（预填）**：`execute(input)` 把搜索栏那串文字当待译内容预填进输入框，
但**先剥掉开头的调用词**（`stripInvocation`：`^(翻译|translate|词典|字典|查词|dict)(\s+|$)`）——
用户敲「翻译」是在**调起插件**，不是要把「翻译」两个字译出来。规则只有两条：

- 整个查询就是调用词（`翻译` / `translate`）⇒ 预填**空**；
- 前缀不是调用词（`translator`）⇒ **原样保留**（正则结尾的 `\s+|$` 就是为它写的：
  否则 `translator` 会被切成 `or`）。

> **多词查询今天搜不到这个插件**（实测：`翻译 hello` 不出现 translate 那一行）。
> 那是 `registry.ts` 的通用匹配规则（关键词是整词比对 / 子序列模糊，带空格外加字母的查询
> 匹配不上 `翻译`），**不是翻译插件的缺陷**，也**不要**为一个插件去改全局匹配 ——
> 上面那条 `\s+` 分支留作防御：宿主把「调用词 + 待译内容」整串传进来时照样正确。

**重开面板不恢复缓存 HTML**（`main.ts` 的 `skipRestore` 里加了 `translate`）：翻译面板带着
**来自搜索栏的待译内容**，若按别家的做法把上一次的 HTML 抬回来，这次的查询就被悄悄丢掉 ——
第一次点开有预填、第二次没有，同一个动作两种结果。

**实测（2026-09-29，dev 实例 + CDP 探针，逐字）**：

- `translate_lookup("hello")` → 2688 ms，`你好` + `英 həˈləʊ  美 həˈloʊ` + 3 条释义 + 3 条例句；
  再查一次 → **7 ms、`cached: true`**。
- `translate_lookup("how are you doing today")` → 658 ms，`你今天过得怎么样`（有道的 `trs` 兼作译文）。
- `translate_lookup("你好，今天天气不错", to:"en")` → 786 ms，`Hello, it's a nice day today`。
- `translate_lookup("zzzzqqqqxyznotaword")` → 671 ms，`source: "none"`、不报错。
- `translate_ai("The early bird catches the worm.")` → 1293 ms，`早起的鸟儿有虫吃。`；
  再点一次 → **6 ms、`cached: true`**（不重复花钱）。
- 面板侧：语言下拉五语言正确、查询后 `dictTrans=你好`、`explains=3`、`examples=3`、
  小字「来自本地缓存」；点「用 AI 重新翻译」→ 小字变「来自 AI」；
  点悬浮按钮 → 主窗回到搜索态、悬浮窗（420×560，标题「翻译」）里 `give up` → `放弃`。
- 市场面板：基础插件组里 `翻译` 只有「打开」（与备忘录 / AI 助手 / 网页搜索 / 设置 /
  快速启动 并列），拓展插件组里的三个仍带「卸载」。
- 预填（走真实入口：搜索栏派发 `input` → 点结果区那一行 → 读 `#xl-input`）：
  `翻译` ⇒ `""`、`translate` ⇒ `""`、`translator` ⇒ `"translator"`（**没被切成 `or`**）、
  `翻译 hello` ⇒ 不出现 translate 那一行（多词查询匹配不上关键词，见上）。
  三个查询**连续开合**都拿到各自的值 ⇒ `skipRestore` 那一处修对了。

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
│   │   ├── icon_extractor.rs        # 系统图标 + 缩略图（SHGetFileInfoW / image crate → base64 PNG，结果列与详细搜索预览区用）
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
├── src/skills.rs                    # P4 技能（LUNAC_SKILLS_DIR 的 <key>/SKILL.md + Skill 工具；inline / fork 两模式）
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
>
> **2026-09-21 补一种更隐蔽的坏法：两份 BOM（实测坏掉的就是 `build-release.ps1`）。** 一次批量文本编辑把文件按「BOM + 原文」（原文自己已带 BOM）重存 ⇒ 首六字节 `EF BB BF EF BB BF`。PS 5.1 只吃掉**第一份** BOM，剩下的 `U+FEFF` 让**首行**（`# Lunac Release Build Script`）变成一条命令 ⇒ 第 18 行的 `param()` 不再是「首语句」⇒ **脚本参数全部不绑定**（`$Version` / `$NoBump` 全成 `$null`），而报错是「无法将 `?#` 项识别为 cmdlet」+「无法将 `param` 项识别为 cmdlet」这种与真实原因**毫不相干**的东西，最后停在「版本号必须形如 x.y.z，收到： False」（`$Version` 在消息里显示成字符串 `False`）—— 只看报错完全猜不到根因。**判据**：首三字节 `EF BB BF` **且第四字节不是 `EF`**（两份 BOM 时 PS 报不出「BOM」这个词，所以必须主动查字节）。**修法**是「以 UTF-8 with BOM **重存**」（等价于去掉多余 BOM），不是「再加一份 BOM」。**自动守卫**：`npm run verify` 第 ⑧ 节扫全仓 `.ps1` / `.nsi`，两种坏法都报 FAIL —— ① 开头有两份 BOM；② 含中文却没有 BOM（这一条同时把 `docs\parse-minidump.ps1` 的旧违规修掉）。两个分支都做过反向验证（临时造坏文件，确认真的报 FAIL 并让 `verify` 以 1 退出）。**2026-09-29 又踩到一次同类**：用文本替换改 `scripts\build-plugins.ps1`（加一个插件条目）时，工具把 BOM **整个抹掉**（首字节变成 `23 20` = `# `），PS 5.1 立刻按 GBK 解码，报的是 `Missing argument in parameter list` + 一串 `Unexpected token '鏋勫缓鎻掍欢鍖咃紙vite'` 这种**与真实原因毫不相干**的乱码报错。**判据与修法同上**（首三字节必须是 `EF BB BF`）：`[IO.File]::ReadAllBytes()` 读出来前面补 `EF BB BF` 再写回即可。⇒ **改 `.ps1` / `.nsi` 之后一律跑一次 `npm run verify`**（第 ⑧ 节就是为这一类坏法设的守卫）。

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

采用 `x.y.z` 三段格式。**当前 0.9.2**（`build-release.ps1` 在**打包时自动把 patch +1** 并同步下面「六处载体」+ 读回确认，见 §8.2）：

| 阶段 | 版本号 | 触发条件 |
|------|--------|---------|
| 日常开发 | 不增版 | 每次修改完成后仅执行 `npm run build` / `cargo test` / `cargo check` 编译验证 |
| 打包 | **patch 自动 +1** | 用户**明确要求打包**时执行 `build-release.ps1`：读 `package.json` 的版本 → patch +1 → 同步**六处载体** → **读回确认**（**必须在 `cargo build` 之前**，否则 exe 内嵌版本与安装包名不一致） |
| 大版本跳档 | 显式传 `-Version x.y.z` | e.g. `.\build-release.ps1 -Version 0.10.0`（不递增，直接同步到指定值）；`-NoBump` 则保持当前版本重打包（**仍会读回确认**） |

**版本号载体清单（2026-09-21 收口）** —— 属于 **app 版本线**的共 **8 处**（表中前 8 行；第 9 行是**独立版本线**，列出来只为提醒别把它算进来）。有 N 处写版本就有 N-1 处会漂移，「安装包 0.9.x、随包的 VSCode 扩展 0.9.y」这类对不上就是这么来的：

| 载体 | 值随谁变 | 说明 |
|---|---|---|
| `app/package.json` | **脚本** | 唯一真相源（`Get-DeclaredVersion` 读它来 +1） |
| `app/src-tauri/tauri.conf.json` | **脚本** | 被 Tauri 嵌进 `lunac.exe` 的属性 |
| `app/src-tauri/Cargo.toml` | **脚本** | 参与编译 ⇒ **必须早于 `cargo build`** 改 |
| `vscode-extension/package.json` | **脚本（2026-09-21 才纳入）** | `vsce package` 用它命名 `.vsix`；不在链里时第 ⑦ 步挑不到匹配版本 ⇒ `WARN` 后把**旧版扩展**塞进新包（实测踩过：该文件曾比 app 三处**超前**一个小版本） |
| `app/package-lock.json` | **脚本（2026-09-21 才纳入）** | npm 只在 `install` 时改写 ⇒ 平时一直躺着旧号（实测停在 0.1.0） |
| `vscode-extension/package-lock.json` | **脚本（2026-09-21 才纳入）** | 同上（实测停在 0.6.0） |
| `app/src-tauri/Cargo.lock` | **cargo**（不必管） | 构建时自己跟着 `Cargo.toml` 走 |
| `scripts/lunac-installer.nsi` | **脚本第 ⑨ 步** | `PRODUCT_VERSION` —— 安装包名与「卸载」里的 `DisplayVersion` 都由它展开 |
| `core-agent/Cargo.toml` | **独立版本线** | agent.exe 自己的版本（0.1.0），不跟 app 走；注意它**没有** VERSIONINFO，右键看不到版本号 |

### 8.2 打包流程

仅在用户明确说"打包"或"生成安装包"时执行。一条命令搞定：

```
powershell -ExecutionPolicy Bypass -File build-release.ps1                   # 自动 patch +1（推荐）
powershell -ExecutionPolicy Bypass -File build-release.ps1 -Version 0.10.0   # 显式指定，不递增
powershell -ExecutionPolicy Bypass -File build-release.ps1 -NoBump           # 保持当前版本重打包（调试用）
```

脚本九步：⓪ **版本号**：读 `app/package.json` → patch +1 → 六处载体全部写回 → **读回确认**（任一处读到别的值就当场 `throw`，**必须在第 ⑤ 步之前**）① 预检 cargo / makensis ② kill 运行中的 lunac.exe / agent.exe ③ `npm run build`（前端）④ `cargo build --release`（core-agent → agent.exe，**必须早于第 5 步**）⑤ `cargo build --release`（src-tauri → lunac.exe）⑥ **清空并重建暂存目录** `release\Lunac\` + 拷 `lunac.exe` / `agent.exe` / `WebView2Loader.dll` + **拷 `agent-templates\{skills,tools}` → 暂存目录同名子目录**（README + `*.example` 模板，装完用户可照抄，见 §11 规则 24）⑦ 打包 VSCode 扩展 → `lunac.vsix`（**按第 ⓪ 步同步后的扩展版本现场 `vsce package`**；挑不到匹配版本的 `.vsix` 只 `WARN` 后回退最新的那个 —— 版本对不上时这就是**静默**的那一步，所以第 ⓪ 步的读回确认把扩展也纳入了）⑧ 预置 PaddleOCR-json（本地 `paddle-ocr/` 优先，缺失则从 GitHub 下载 .7z）⑨ 改写 NSI 版本号 → **先删同名旧产物**（makensis 覆盖已存在文件时只会含糊地报 `Can't open output file`，实测于旧包刚生成、杀软仍在扫描它时）→ makensis → `release\Lunac-<版本>-Setup.exe`。

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

**页面一律不插 emoji（2026-09-20，用户明确要求）**：**GitHub 上会被人读到的页面** —— `README.md`、`agent-templates/**/README.md`、`docs/**` 正文 —— 一律**不出现装饰性 emoji**（火箭、齿轮、剪贴板、地球、扳手、放大镜、备忘录、机器人这类图标字符，以及勾叉、星标、火花等）。清单、表格、标题一律**纯文字**（插件名就写「快速启动 Quick Launch」，不带图标前缀）。三条理由：① 与既有的「控件文案不带 emoji」（§11 规则 49 + `icon-style.md` §4.1）是**同一条纪律**，只是范围从界面扩到仓库页面；② 这些字符在 GitHub / 终端 / 编辑器里的**字形与宽度各不相同**，对齐会散；③ 一个图标前缀会让整份文档看起来像营销页，与「规范文档」的定位不符。**本规范与 `code-rules.md` 预检 #26 自身也按此写** —— 需要指认某个字符时用**文字描述**（「思考那个对话气泡字符」「剪贴板图标」），不把字符本身写进页面。新增页面 / 段落时按此写，**不要**事后清理。

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

**当前无未解决的已知问题。** 本节原先维护的「已关闭问题」逐条存档（窗口拖拽、Esc 逐级退出、幽灵框穿透、热键双后端、各工具的真机烟测记录…）已于 2026-09-19 撤下 —— 这些事实的落点在别处，不重复搬运：

| 想知道什么 | 去哪看 |
|---|---|
| 搜索栏 / 结果区 / 详细搜索大界面的现状 | §2.1 / §2.1.1 / §2.1.2 |
| 热键双后端 + 四层兜底 | §2.3 |
| 窗口高度实测驱动机制 | §2.4 |
| agent 已实现能力的全景（工具 / 压缩 / MCP / 技能 / 会话持久化） | [agent-implementation.md](./agent-implementation.md) §3 |
| **还没做的能力**（唯一待办真相源，按优先级排序） | [agent-feature-backlog.md](./agent-feature-backlog.md) |
| 前端 / Tauri / WebView2 的已知陷阱与反模式 | [code-rules.md](./code-rules.md) |

> **纪律**：问题修完**只留规则与代码注释**，不再往本节追加「✅ 已完成」的流水账 —— 已完成项堆积会把真正的待办淹掉。新增的未解决问题写 §9.1（疑难点）或 backlog。

### 9.1 疑难点（三个都已定位，只剩两项待实测）

> 本节曾登记两个「没有定论」的硬骨头。**两个都已定位完毕**，剩下的只是复现与实测，已挪进 [agent-feature-backlog.md](./agent-feature-backlog.md) §3 的 **M1**。这里只留结论与证据，避免下次重新调研。

#### 难点 1：AI 缓存命中率「偏低」—— **结论：不是代码 bug**

**现象**：长会话命中率低于社区自述的 97–99% 区间，且与供应商平台用量页对不上。

**结论（2026-09-15 结案）**：

1. **本侧请求形状已是理想形状** —— `system`（`SYSTEM_PROMPT + env_block(cwd) + skills::listing() + 往期会话索引`）在进程内**只构建一次、逐字节不变**；`tools` 全量注入且 MCP 按名排序；`history` 严格 append-only。⇒ 差距只可能在「压缩改写前缀」「供应商落盘规则」「统计口径」三处。
2. **本地命中率已在自然上限附近**：例 `in=4612 / read=15744` → 77.3%，恰好等于「上一轮上下文 ÷（上一轮 + 本轮新增）」这一上限公式。**差距来自会话太短**（这些记录上下文只有 15–20K）；社区那个 97–99% 是几十轮以后新尾占比变小才达到的。
3. **「落盘慢」已被证伪** —— 同一前缀、纯追加的 A/B 对照（0ms 间隔 vs 6s 间隔）结果**逐字节相同、均 97.9%**。所以「供应商来不及落盘」不是低命中的解释，别再拿它当理由。注意第 1 次请求必然 0%（全新前缀没有已落盘单元），这是**规则性的**、不是缺陷。
4. **固定前缀体积远小于直觉** —— 实测 `system=1058 字 + tools=11 个 / 5860 字` ≈ **1730 tokens，只占 128k 预算的 1.4%**。⇒ 因此**明确不做**「工具 schema 按需检索」：省下的上限只有几百 token，却要引入检索桥 + 一次前缀变动，是净亏。
5. **主动断裂源只有压缩这一类** —— `compact_history` 的 elide / drop / 插 `TRIMMED_MARKER` / 钉回任务快照 / 钉回摘要；且后三项**只在「本轮真的 `dropped > 0`」时**发生，不额外制造压缩时机。另给瘦身档加了「值不值得」闸门，**判据按位置算**（2026-09-21 A15：`σ × 50 > Δ × 50`，即「省下的要多于废掉的」，`σ` = 省下的字符、`Δ` = 被改写那条之后没被瘦身的剩余字符）：旧口径「可省体积 ÷ 上下文体积 ≥ 5%」（已删的 `ELIDE_MIN_SAVINGS_RATIO`）看不见代价的位置，于是频繁出现「省 2% 体积、废掉 60% 前缀」—— 推导与实测见 §11 规则 23。
6. **埋点已补齐**：agent 在每次 `message_stop` 记一条 `{in, read, create, out}`，随 `result.usage.requests` 上报，前端写进 `usage-*.jsonl` 的 `requests` 数组 ⇒ 从此能与平台用量页**逐行**对账。

**唯一遗留疑点**（→ backlog M1-2）：dev 日志里那一轮 `in=2287 / read=0`（且 `elided/dropped` 都是 0）**不能**再用「来不及落盘」解释。剩余两种可能：① **前缀本身变了**（`skills::listing()` / 工具黑名单 / `cwd` 任一变化都会改 `system` 或 `tools`）；② 供应商侧缓存被清。下一轮立刻回到 89.7% 说明前缀随后又对上了。**定位工具已就绪** —— 用 `requests[]` 与启动时落盘的 `固定前缀 …` 行前后对照即可。

**验收口径**：同一会话连续 20 轮工具往返，逐轮命中率不低于「上一轮上下文长度 ÷ 本轮总输入」这一自然下限；且压缩次数与未命中增量可分离统计（两者都有落盘字段：`requests[]` 与 `elided`/`dropped`）。

**复现脚本**（原样保留，换 key 后可直接重跑）：

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

判读口径：两组 `read` 相同 ⇒ **落盘不是瓶颈**，低命中只能由「新尾占比大」或「前缀本身变了」解释；若 B 组明显大于 A 组，才说明「落盘要时间」是短间隔连续请求丢命中的主因（属规则性，改代码无用）。

**可借鉴的三份资料**（调研记录，支撑 §11 规则 18 / 23）：

| 资料 | 对本项目的启示 |
|---|---|
| dsh（DeepSeek Harness）对话链路 | 会话是**事件溯源 append-only** 日志；原文结论「前缀缓存稳定性是架构的**推论**，而不是被管理的目标」⇒ 任何「重排 / 回填 / 就地编辑历史」都是命中率杀手 |
| ponytail 的 skills 实现 | **固定前缀必须短**：能按需加载的能力（技能正文、工具 schema）都不要塞进每轮都发的前缀 |
| Hermes 的省 token 机制 | ① system prompt 一 session 只构建一次、逐字节回放；② 时间戳降精度到日且放最末；③ 插件上下文 / 记忆 recall **一律注入 user 消息**，绝不动 system；④ 传输层用**内容寻址 cache key**（`instructions` + 按名排序的 tools 取 hash）而非 session_id；⑤ **压缩是唯一被允许的前缀失效时机**；⑥ 工具 schema 按需检索（**本侧已决定不做**，见上第 4 点）；⑦ 单条工具输出设预算（**已等价做到**） |

#### 难点 2：开机自启后 Lunac 很久才可用 —— **根因已定案：不是 Lunac 冷启动慢**

**现象**：手动双击是**秒启**；由 Windows 开机自启拉起时要等很久才响应热键。

**根因（两条叠加，2026-09-15 定案）**：

1. **机制差异完全来自 OS 触发时机** —— 计划任务（`schtasks /sc onlogon`）**~19s** 触发，HKCU Run 键 **~79s**。这 60 秒差距与进程内代码无关。
2. **非管理员必然落到慢的那一条** —— `enable_auto_start()` 先建计划任务、失败才回退 Run；而计划任务在任务计划**根目录**，非管理员创建**必被拒**（实测日志 `schtasks create failed: 错误: 拒绝访问。`）。NSIS 是 `currentUser` 安装模式 ⇒ 对普通用户这是**必然**而非偶发。

**已落地的修复（要点，不得回退）**：

- **自启项改成「按需重建」**：先比对 Run 值与当前 `startup_command_line()`，一致就什么都不做；不一致再读计划任务 `<Command>` 比对。消掉了此前**每次开机**的两次 `schtasks.exe` 与一次注册表写入。
- **决策全部落盘**：`auto_start.rs` 的决策日志统一走 `crate::log::info`（release 是 GUI 子系统，`eprintln!` 等于写进空气），前缀 `auto_start:`，可 `grep` 查证。**新增可诊断代码时不要用 `eprintln!`。**
- **提权链路跑自己的 exe**：`ShellExecuteExW("runas")` 提权跑 `lunac.exe --lunac-auto-start-task=create|delete`，而不是提权跑 `schtasks`（后者 `/tr` 里的引号路径 + `--background` 经多层转义极易出错）；父进程 `WaitForSingleObject` 等子进程结束后**复查任务是否真的存在**（不轻信退出码）。**取消 UAC 不算失败**（仍是「已开启、只是慢」），且此时**绝不能顺手清 Run 键**，否则会把用户原有自启弄没。**删除也必须能提权** —— 否则会出现「关不掉的自启」，比「慢」更糟。
- **单实例保护**（`single_instance.rs`，手写 FFI，未新增依赖）：会话级命名互斥体 + 唤出事件。名字里不含版本 / exe 路径 ⇒ dev 与 release 互相排斥。没有它，自启实例已在后台时再双击会让 `RegisterHotKey` 失败并退化装全局钩子（同时也是杀软误报 `Prowloc` 的成因）。
- **机制探测只进日志**：`auto_start_info()` 返回 `{enabled, mechanism}`；前端只用 `enabled` 拨开关，`mechanism` 每次打开设置面板记一行。**一度在面板上显示过机制小字**（「Run 键 / 计划任务」+「改用计划任务」按钮），**已按用户要求撤掉** —— 纯实现术语，终端用户既不会注意也看不懂。

**两个硬事实（踩过，别再犯）**：

- **`schtasks` 的文本输出字段名会随系统语言本地化** —— 中文 Windows 上 `schtasks /query /fo LIST` 连 `TaskName:` 都匹配不到。所以任务内容必须走 **`/xml`**（`<Command>` 是固定标签，与语言无关）。
- **`/xml` 的输出编码无法在本机实测**（沙箱不允许枚举系统计划任务）⇒ `decode_task_xml()` **UTF-8 与 UTF-16LE 两种都试**（判据是解出来能否找到 `<Command>`），并把「读不出内容」与「没有任务」**分成两种结果**：后者什么都不做，前者**按旧行为重建** —— 解析失败绝不能变成「以后不再自愈」的静默回归。该方法有单测覆盖两种编码。

**待定**（→ backlog M1-1）：实测确认前端 CDN 与 WebView2 冷启动在开机场景的真实占比；若确认无关，就把结论写死，不再靠猜。

**验收口径**：实际生效机制可在落盘日志里查证（`grep auto_start`）；开机后「登录完成 → 热键首次可响应」的时长与手动启动的差值不超过 OS 触发时机本身的差异；自启实例在运行时再双击**不产生第二个进程**，且能把已有窗口唤出（2026-09-15 已实测）。

#### 难点 3：API 消费金额归因（2026-09-29 结案）—— 「是 Trae 的 10 倍」不是计价贵

**现象**：用户口径「Lunac 的 API 消费金额基本是 Trae 的 10 倍」。

**数据**（`D:\Downs\usage_data_2026-09-29_2026-09-29.zip` → 解出 `amount-*.csv` / `cost-*.csv`）：

- 两个 key 同属**一个账号**（`user_id` 相同）：`flash -trae`（`sk-209a8…`）与 `test  for lunac gith`（`sk-9842b…` = Lunac `.env` 的 key）。
- 全日：trae 12.908 元 / 1,372 请求；Lunac 0.181 元 / 24 请求。

**结论 1：不是计价档位差，是官方的峰谷定价。** 同一天**同一个 key** 也出现两档价（miss `1e-6↔2e-6`、hit `2e-8↔4e-8`、output `4e-6↔8e-6`，整体 2 倍），落在 14:00 与 17:00 两个窗口。

**2026-09-29 已查明（当天晚些时候补做）**：这不是「临时调价」也不是「后端版本混用」，而是 **DeepSeek 2026-08-17 起生效的峰谷定价** —— 高峰 = **周一至周五**的北京时间 **09:00–12:00** 与 **14:00–18:00**，其余全部（含整个周末与中国法定节假日）为谷时，**谷价 = 峰价的一半**。逐小时核对完全吻合：CSV 里 12/13 点谷、**14 点峰**、**17 点峰**、18 点起谷。单价也与官方价目表对得上（命中 : 未命中 : 输出 = `0.02 : 1 : 4`，折算汇率自洽）。⇒ **首轮拿 14:00 那一行去比 12:00 那一行得出的「两个 key 差 2 倍」是误读，已纠正**；同时这也解释了「面板金额比账单高约 2 倍」—— `pricing.json` 当时只有单档（峰）价。**处置**：价格表已支持时段价并预置官方峰谷价，详见 §3.5 与规则 63。

**结论 2：11.2 倍 = 命中率差 × token 结构差，两个独立因子。**

| | flash -trae | test for lunac gith |
|---|---|---|
| 请求数 | 1,372 | 24 |
| 输入 hit | 202,709,504（**98.30%**） | 205,440（**89.14%**） |
| 输入 miss | 3,526,785 | 25,023 |
| 输出 | 1,029,619 | 28,514 |
| 输出 / 输入 | **0.50%** | **12.4%** |
| 元 / 百万 token | 0.0623 | **0.698（11.2×）** |

拆开算：只用 Lunac 自己的命中率、换上 trae 的 token 结构 ⇒ **3.09×**；再叠上 token 结构差 ⇒ **3.63×**；3.09 × 3.63 ≈ 11.2×。

**结论 3：「元 / 百万 token」不能当 KPI。** 它奖励的是「堆缓存命中」，会把「上下文很长但几乎全命中」判成先进（trae 那 2 亿输入里绝大部分正是每请求重发的 ~150k 静息上下文，单价只有 miss 的 1/50）。**该用「每次提问成本」或「每个有效产出的成本」。** 换成「元 / 请求」，Lunac（0.00753）其实**低于** trae（0.00941）。

**结论 4：除命中率外的影响因子分五层**（可供复查的清单）：

| 层 | 因子 | 实测 / 依据 |
|---|---|---|
| 0 单价 | 模型档位；**峰谷时段价**（官方规则，实测 2×）；面板价格表口径 | 已查明是官方峰谷定价（见结论 1）⇒ 价格表**已支持时段价并预置官方峰谷价**（§3.5 + 规则 63）。修复前 `pricing.json` 只有单档（峰）价，面板对谷时用量**高估约 2 倍**：dev 环境 2026-09-29 那天，分时口径 **0.136858 元** vs 旧单档口径 **0.198505 元** |
| 1 每请求 token 构成 | **输出**（单价 = miss × 4 = hit × 200）；**思考 token 按输出计价**；miss 输入；hit 输入 | Lunac 当天 **78.5% 的成本是输出**（0.1419 / 0.1808）。**2026-09-29 已动手**：输出纪律进 `PERSONA_AND_STYLE` ⇒ 稳态每次提问成本 **-41%**（4854 → 2843 miss 等价，结论 7） |
| 2 请求次数 | **agent 工具循环**（一次提问实测 7~10 次请求，上限 `MAX_TOOL_ROUNDS` = 16）；**子代理**（`MAX_SUBAGENT_ROUNDS` = 8）与**后台复盘 fork**（`MAX_REVIEW_ROUNDS` = 4）各是完整对话；Drop / Force 档额外一次摘要请求 | `usage-2026-09-29.jsonl` 两条记录分别 10 / 7 次请求。**2026-09-29 结案**：这是**模型自己的往返节奏**，不是本侧并行没做 —— `TOOL_PARALLELISM` = 「同响应多 `tool_use`」（结论 8），提示词层引导**未能**改变它（结论 7）。子代理的账已并入（结论 5） |
| 3 前缀重建 | agent 冷启动（system 块全 miss；**往期会话索引含相对时间标签**，跨天 / 跨会话必变）；压缩 / 摘要 / 任务快照 / 相位注记插回 history；切模型 / 切思考档 / 换工作区 ⇒ 重启 | 同规则 23 的断裂源清单 |
| 4 计量口径 | `result.usage` = 本次提问绝对值；**子代理 / 复盘此前完全没进账**（见结论 5）；DeepSeek 无 cache write 费（`cacheCreate` 恒 0 **不是 bug**） | 平台同一 key **24 次**请求 vs 本地 `usage-*.jsonl` **17 次** |

**结论 5：真因之一是本地账漏计。** 「平台 24 次 vs 本地 17 次」这条差额的主因是 `run_subagent` 的用量**从来没进过 `result.usage`**（只累进它自己的预算熔断 `spent`）。**已修**，契约见 §3.5「用量与对账」里那两条 2026-09-29 行。

真机端到端（2026-09-29，`e2e-ab-thinking.ps1` 的 subagent 模式，真实端点）：一次提问 `turns=2`、**`requests=4`**（主循环 2 + 子代理 2），子代理吃掉 `in 2448 / 2917 = 84%` 的未命中输入、`out 179 / 350 = 51%` 的输出 —— **修好之前这一问的账面上只有 2 次请求**。⇒ 在修好之前，任何「改哪儿能省钱」的判断都建立在偏低的基数上。

**结论 6：思考档不是主因（A/B 实测，2026-09-29）。** 探针 `core-agent\target\hooktest\e2e-ab-thinking.ps1`（同一任务、各自全新进程 ⇒ 两次都是冷启动），真实端点 flash：

| 任务 | thinking=on | thinking=off | 输出比 | 价格加权成本（miss 等价） |
|---|---|---|---|---|
| 单文件摘要（2 请求） | out=127 | out=103 | 1.23× | 3,756 vs 3,728 = **1.01×** |
| 三文件读取 + 计数（2 请求，第 1 次） | out=185 | out=122 | 1.52× | 1,268 vs 965 = **1.31×** |
| 三文件读取 + 计数（2 请求，第 2 次） | out=122 | out=121 | 1.01× | 1,014 vs 960 = **1.06×** |

三次的**最终答案逐字相同** ⇒ 差额就是**思考 token**（原始流里确有 `thinking_delta` 块，且按 output 计价）。三次都落在 1.0~1.3× ⇒ 思考档是一个 **1.0~1.3× 的乘数**，随任务难度浮动，**不是 10 倍级的主因**；真正的乘数是「输出总量 × 请求数」。

**结论 7：能动的两条提示词，一条生效一条不生效（2026-09-29，探针 `core-agent\target\hooktest\e2e-l6-cost.ps1`）。** 固定任务（读工作目录里全部 `.md` → 报最多 / 最少行数 + 2~3 句用途）、固定模型（flash）、固定思考档（默认 on）、每次**全新进程**：

| 轮次 | 请求数 | `out` | 答案字符 | 思考字符 | 首请求 `in / read` | miss 等价 |
|---|---|---|---|---|---|---|
| 改前基线 | 4 | 936 | 1156 | 887 | 841 / 2944 | 4854 |
| 改后第 1 次（前缀刚变 ⇒ 端点侧无此单元） | 4 | 788 | 701 | 811 | 4002 / 0 | 7369 |
| 改后第 2 次（前缀已入缓存） | 4 | **488** | **474** | 476 | 612 / 3198 | **2843** |

- **输出纪律生效**：`PERSONA_AND_STYLE` 的 Output Style 段新增「只把输出 token 花在答案上」—— 无开场白、不预告「接下来我要做什么」、不复述计划、不复述刚读到的内容、只答被问的、不把用户已经能看到的正文贴回来、结尾只留一行「改了什么」。两次改后采样的答案字符（701 / 474）都低于基线 1156，`out`（788 / 488）都低于 936；第 2 次采样里那三段过程旁白（`I'll list…` / `Read all four…`）**彻底消失**。两边都处于「前缀已缓存」的稳态时：**4854 → 2843，每次提问成本 -41%**。
- **改后第 1 次的 7369 不是回退**，别误读：那是「固定前缀刚被改写 ⇒ 端点侧还没有对应单元」的一次性代价（`in=4002 / read=0`），第 2 次就回到 612 / 3198。⇒ **改固定前缀这件事，成本是「每台机器每套前缀各一次」**，不是每次提问都付。
- **批量化没生效（如实记，别再往这个方向使劲）**：`SYSTEM_PROMPT` 新增「互不依赖的读 / 搜放进**同一次响应**」之后，三次采样的 `requests` **全是 4**，工具序列也逐字相同（`Glob, Read×4, Bash`）。⇒ 在这个任务上模型本来就按「一轮一个调用」走，**一句提示词没有改变它的往返节奏**。「一次提问 7~10 次请求」是**模型自己的节奏**，不是本侧把并行做丢了 —— `plan_tool_batches()` 那条路的语义已经查清（见结论 8）。要压这个数只能从「让模型少问几轮」入手，**不要**去改并行实现。
- **对照口径（本探针的坑）**：改前那次首请求 `read=2944` 起步，是因为端点侧**早就存着同一份前缀**（此前跑过同类探针）；改后第 1 次是冷前缀。⇒ `miss 等价` 只在**双方都处于已缓存稳态**时才可比（对照「改前基线」与「改后第 2 次」两行），别把冷前缀那一行读成回退。

**结论 8：`TOOL_PARALLELISM` 的语义已核实（回答 backlog L6 的那个疑问）。** 它是**同一个响应里带 N 个 `tool_use` 块、由本侧并发执行**（`plan_tool_batches()` 切批 → 只读批内 `TOOL_PARALLELISM = 4` 条并发），**不是**「并发发 N 份请求」。请求数恒等于「模型轮数」，与并行度无关；探针抓到的线上请求数（4）与工具数（6）正好说明这一点。

**遗留**（→ backlog §3 的 **L6**）：~~① 价格表要能表达**分时价**~~ **（2026-09-29 已完成：`time_windows` + 逐桶计价 + 预置官方峰谷价，契约见 §3.5 与规则 63，实测见文末）**；~~② 输出瘦身~~ **（2026-09-29 已完成：输出纪律进 `PERSONA_AND_STYLE`，稳态每次提问成本 -41%，见结论 7）**；~~③ 压请求次数~~ **（2026-09-29 已结案：确认 `TOOL_PARALLELISM` 是「同响应多 `tool_use`」（结论 8）；提示词层的批量化引导实测**未改变**请求数，如实记为「已排除本侧实现嫌疑」而非「已降本」）**；④ 建立「每次提问 × 真实峰谷单价」的对账基线 —— **本地侧口径与工具已就绪**（`scripts\reconcile-usage.ps1`，见 §3.5），**待平台导出 CSV 才能出跨源基线**。


**踩过的坑（实测，别再犯）**：用 PowerShell 5.1 驱动 `agent.exe` 时**不要走 .NET 的 `Process.StandardInput`** —— 本机 `Console.InputEncoding` 是带 BOM 的 UTF-8，那 3 字节前导会落在子进程 stdin 头部；而 agent 的读取端只做 `line.trim()`（BOM **不是** Rust 的空白字符）再 `serde_json::from_str`，于是每轮都报 `忽略非法 JSON 输入行: expected value at line 1 column 1`。`ProcessStartInfo.StandardInputEncoding` 在 .NET Framework 上**不存在**；把字节直接写进 `BaseStream`（哪怕用 `Encoding.ASCII`）**也照样带 BOM**（`_probe-stdin.ps1` 三种写法实测均以 `efbbbf` 开头）。正解 = 让 `cmd` 用 `< q.json` 重定向喂**无 BOM 的文件**，并保持 `RedirectStandardInput = $false`，即**根本不让 .NET 建 stdin writer**。

---

## 10. 待办的落点（仅留指针）

**唯一待办真相源 = [agent-feature-backlog.md](./agent-feature-backlog.md)**。规范正文只写「是什么 / 为什么这么做 / 不得怎么做」—— **新增待办一律登记到 backlog**，不要再往正文里插「待办 / 路线 / 待实现」小节。

> 为什么只留一处（2026-09-19 整理的实测教训）：待办原先散在四处（本节、§13 参考设计、§19.6 待实现清单、§20 路径 2，外加 backlog 自己），同一件事写两三遍、**完成状态还不同步** —— 整理时发现 Humanizer 按钮与安全警告块其实早已落地，文档却一直标着「待实现」。

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
   - **开发构建绝不注册 / 不修复开机项（2026-09-21 实测取证，不得回退）**：`is_dev_build()`（`cfg!(debug_assertions)`，或 exe 路径含 `\target\debug\` / `\target\release\`）为真时，`enable_auto_start()` **直接返回错误**（开关如实弹回去，并把这句原因显示在面板上），`repair_auto_start_on_startup()` **什么都不做**。真发生过：一次 dev 调试把登录计划任务写成了 `…\target\debug\lunac.exe --background`，此后**每次登录都由那份控制台子系统的旧构建拉起** —— 用户看到的就是「开机弹 cmd 窗口 + 呼出来的界面是旧的/不能用」，而当时日志里只有一句「重建计划任务」，看不出原因。判据有单测 `dev_build_path_detection` 钉住。
   - **开机项指向「别的程序」必须查得出、且给一次修复入口（2026-09-21）**：`auto_start_info()` 多返回一个 `stale`（Run 值与计划任务 `<Command>` **两边都查**；只对读得到的内容下结论，不存在 / 读不出时一律 `false`），设置面板据此显示一行提示 + 「修复」按钮（复用 `set_auto_start(enabled:true)`，会提权重建任务）。同时 `repair_auto_start_on_startup()` 的重建失败**必须如实记账**：旧实现 `let _ = create_logon_task()` 把「非管理员下必然被拒」吞掉了，于是开机项一直指错、日志里也看不出原因 —— 这种「静默修不好」与规则 1 的其余各条是同一类毛病。
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
14. **Agent 内置工具与审批（P1/P2，2026-09）**：十五件工具全部在 `core-agent/src/tools.rs`，工具名必须保持 **PascalCase**（前端 `main.ts` 对 `"Bash"` / `"PowerShell"` 有专门的命令展示与危险命令分类分支），新增/改名要同步 §3.5 的契约表。写类四件（`Write`/`Edit`/`Bash`/`PowerShell`）**必须先发 `can_use_tool` 等前端回包**，agent 侧不做二次判断（白名单与危险命令分类归前端 `classifyRequest()`）；`plan` 档直接拒绝、`LUNAC_WORKSPACE_LOCKED=1` 拦越界 —— 这两道闸门与审批是**与**关系，任何一道都不得为了「少点一次同意」而放宽。**`WebSearch`、`WebFetch` 与 `AskUserQuestion` 同样必须先审批，且是「`plan` 档不解禁」的例外**（`tools::gated_in_read_only`）：只读档对写类工具免于询问，是因为那些工具反正会被拒（问了白问）；这三件在只读档**放行** —— `WebSearch` 会把查询词发往外部搜索源，`WebFetch` 能把 `Read` 到的文件内容拼进 URL 带出本机，`AskUserQuestion` 的答案只能从卡片上取。**`Agent`（子代理）也必须先审批，且 `plan` 档直接拒绝** —— 它派生的是一个能写文件、能跑命令的子代理；但这个批准**只覆盖「派代理」本身，子代理内部每次写操作仍各自再走一次审批**（不是一次批准、后面全放行）。**常驻免审批的只有 `TodoWrite`（只改前端面板）、只读的 `SessionSearch`（只读本机自己的会话库）与 `EnterPlanMode`（只改 agent 进程内的计划相位标志，见规则 59）**，它们的免审批不构成先例：判断新工具是否免问，看的是「执行会不会改变本机或把数据带出」。**`ExitPlanMode` 必须先审批**（那张卡就是它的产品，见规则 59）。工具报错必须以 `is_error=true` 的 `tool_result` 回给模型（不中断整轮），只有 HTTP/流错误才回滚 history。
15. **Agent 上下文压缩不变量（2026-09）**：历史一律以 **user 文本消息**开头（不是 `tool_result`），`tool_use` 与对应 `tool_result` 不得被拆散（丢弃点要跳过 `tool_result` 开头的位置）。任何改动 `compact_history()` 的代码都必须同步修正调用方的回滚锚点 `base`（`base -= dropped`），并在压缩后往 `system/context_compacted` 事件里报出计数 —— 这三条是「压缩后仍能继续对话」的充分条件，改动后请用 `LUNAC_MAX_CONTEXT_TOKENS=8000` 的真实端点烟测复验。
16. **测试一律用 flash 模型（2026-09）**：任何真实端点测试（工具往返、权限审批、上下文压缩、MCP 桥等）把 `AI_MODEL` / `LUNAC_AGENT_MODEL` 指向 **`deepseek-flash`**，**不要用 `deepseek-v4-pro`** —— 测试只验证链路、契约与结构，flash 足够且更快更省；只有当问题与回答质量本身相关、或需要复现线上行为时才用 pro。
17. **MCP 工具命名与审批（P3，2026-09）**：接进请求体的用户工具名一律 `mcp__<原名>`，**前缀与清洗规则（非法字符换 `_`、超长截断、重名加 `_2`）不得随意改动** —— 前端审批卡的「始终允许」按完整工具名记 localStorage 白名单，改名等于让用户的白名单失效。MCP 工具**必须**先发 `can_use_tool`（handler 能跑 shell / 发 HTTP），且 plan（只读）档不接入；桥的失败（spawn/握手/超时）只记 stderr，**绝不允许影响十五件内置工具的可用性**。
18. **前缀缓存不变量（2026-09）**：DeepSeek 等端点的自动前缀缓存按「最长公共前缀」命中，**请求体里任何靠前内容逐字节抖动都会让整段缓存失效**。已定稿的稳定化措施，改动时不得回退：① `history` 一律以 user 文本消息开头；② 压缩丢弃点左移 `base` 锚点而不是改历史首条；③ 系统提示词固定、技能清单按 `key` 排序；④ 内置工具名 PascalCase 稳定、MCP 工具数组**按名排序**后再入请求体；⑤ 工具黑名单只裁剪真实存在的工具名（`core-agent` 的内置十五件 + `Skill`），`src-tauri` 侧**不再内置旧 CLI 时代的默认名单** —— 那批名字对自研 agent 全是空转项，且按名精确比较会误伤同名 MCP 工具。判断「改了会不会掉缓存」的方法：把两次请求体开头做 diff，出现任何顺序变化即为回归。
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
22. **系统提示词环境块（2026-09）**：agent 的系统提示词 = `SYSTEM_PROMPT`（身份 / 工具使用） + `PERSONA_AND_STYLE`（内置的固定「人格 + 文风」，2026-09-17 方案 B 从用户消息搬来） + **用户人格段**（L2，2026-09-21：`config\persona.md` 有内容时紧接在内置人格段之后追加，空则**一个字节都不加**） + `env_block(cwd)` + `skills::listing()`（+ 往期会话索引 / 记忆块，各自按开关），按序拼接且**只构建一次**，见 [core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs) 的 `env_block()` / `build_system_prompt()`。环境块必须写明：宿主是 **Lunac**（不是任何其它 agent 框架的一部分）、**工作目录的绝对路径**、**Lunac 自己的技能目录**（`LUNAC_SKILLS_DIR`），并**显式禁止用磁盘上的文件反推宿主**。
    - **为什么必须写**（真实案例）：默认工作区是用户主目录，而用户主目录里可能躺着**别的 agent 框架**的目录（实测：`~/.hermes/skills`）。模型回答「我自己的 skills 在哪」时只能从文件系统反推 —— Glob 到那些目录后，它把宿主认成了那个框架，整个思考过程都锁死在那里（只是「你是 Lunac 的助手」这一句并不够）。
    - **不得回退**：这段话是身份纠偏的唯一来源，删掉就会退回「模型自己猜宿主」。内容在一次会话内必须**逐字节不变**（cwd 与技能目录在 agent 进程生命周期内都是常量），否则违反规则 18 的前缀缓存不变量。
23. **缓存命中率的解释口径（2026-09）**：命中率**有自然下限**，不能拿 100% 当目标 —— 每轮新增的 user 提问 / assistant 输出 / `tool_result` 都是新内容，天然不被上一轮缓存覆盖，命中上限 ≈ 上一轮长度 ÷ 本轮长度；工具往返多、`tool_result` 大时必然偏低。**因此必须把「自然未命中」与「断裂失效」分开统计**，只有后者才是回归。
    - **全链路审计结论**：`system`（含 `env_block` 与 2026-09-19 起加入的**往期会话索引**）、`tools`（按名排序）、`history` 的四个追加点都是 append-only 且进程内恒定，**不是**命中率低的来源。
    - **断裂源按影响排序**（都在 `compact_history()` 及其调用点，[core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs)）：① `drop`（>95% 水位才丢中段）> ② `elide`（>85% 水位 + 滞回）> ③ 首条插入 `TRIMMED_MARKER` > ④ 失败回滚 `history.truncate(base)` 与压缩叠加 > ⑤ 任务快照钉回（仅当本轮真的 `dropped > 0`，见规则 37 —— 属于 ① 的附带项，不单独增加损失）> ⑥ 摘要压缩钉回（仅当本轮真的 `dropped > 0` 且过了 `SUMMARY_MIN_INPUT_CHARS`，见规则 39 —— 同属 ① 的附带项）。任何改动这几处的代码都要意识到「这是在主动放弃整段前缀缓存」。
    - **已实施的减损措施（不得回退）**：水位从 0.70/0.90 抬到 **0.85/0.95**；瘦身档加**滞回**（`Cfg.last_compact`：一次压缩后要再长 ≥ 预算 ×0.15 才允许动第二次）；`Compact` 三档化，**瘦身档永不丢整条消息**（原先「无可瘦身内容就直接丢」会让 0.85 水位也丢整段，等于白废一次缓存）。丢弃档不受滞回约束 —— 到 0.95 不压就可能 400，安全优先。
    - **瘦身档的「值不值得」闸门 = 位置成本模型（2026-09-21 A15 重写，不得回退）**：瘦身是**就地改写较早的消息**，端点侧从被改的那条起再也匹配不上已落盘的缓存前缀单元 ⇒ 收益与代价**都必须按位置算**。旧闸门（已删的 `ELIDE_MIN_SAVINGS_RATIO = 0.05`）只看「可省体积 ÷ 上下文体积」，**看不见代价的位置** —— 于是「省 2% 体积、废掉 60% 前缀」也能过闸。实测代价（`usage-2026-09-18.jsonl` 第 2 条）：一次 `elide` 瘦身 23 个 `tool_result`（省 176101 字，日志行 `瘦身 23 个 tool_result（省 176101 字）`），该问第 8 次请求的 `read` 从 108160 **塌到 2176**、第 16 次 48000 → 8448 —— **省下的是廉价命中、废掉的是全价重发**。
      - **判据**（[core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs) 的 `worthwhile_elisions()`，纯函数、有守门单测 `worthwhile_elisions_decides_by_positions`）：对「最靠前被瘦身的那个块」k 算
        `σ = 省下的字符数`（**永久收益**：这条消息此后一直短着，会话活着就一直在省）与
        `Δ = history[k..] 里没被瘦身的剩余字符数`（改完要与端点侧缓存对齐不上、必须**全价 miss 重发**的那部分），
        只有 `σ × ELIDE_PAYBACK_REQUESTS(50) > Δ × CACHE_HIT_DISCOUNT(50)` 才动手。两个常数同量级（命中价 = miss 价 ÷ 50，供应商公开价差），于是判据收敛成一句好记的话：**省下的要多于废掉的**（相等也不动 —— 估算本身有误差，白改一次不值）。难度在**位置**不在**总量**：`Δ` 覆盖 k 之后的**全部**消息，不只是候选块。
      - **自由度只有一个 k**：给定 k，把 k 之后**所有**候选都瘦掉永远不比只瘦一部分差（σ 更大、Δ 更小）⇒ 枚举 k 取净收益最大者即可。backlog 候选手段②「瘦身**从靠近尾部开始**」**是这个模型的自然结果**，不必另加规则（实测：两条大块中间夹 5 万字非候选时，模型只动靠后那条，靠前那条**逐字节保留**）。
      - **`Drop` / `Force` 不适用**：它们是防 400 的安全刚需（不压就可能撞上限），照旧**无条件**压，Force 还连尾部也瘦（`elide_keep = 2`）。成本模型只作用于**可选**的瘦身档。
      - **另两条候选手段经实测判定不做**（backlog 要求「实测择一、不要一次全上」）：① **抬高 `ELIDE_RATIO`** 不解决问题 —— 水位只管「什么时候看一眼」，「动不动手」已交给位置模型；抬高它反而让上下文更胖（Δ 更大 ⇒ 更保守），代价是离 0.95 的丢弃档更近。② **把 `elide` 降级为「防止 drop 的缓冲」**（只在丢弃档瘦身）等于连本来划算的那部分也一并放弃 —— 位置模型在净亏时已经自动做到「一个字都不动」（实测①），同时仍保留净赚的那部分（实测②）。
      - **埋点**：两条决策日志都带 `sigma=/delta=/net=/ctx=` 的 `key=value`（中文散文给人看、键值给机器取 —— PS 5.1 的 e2e 脚本按 ANSI 读，脚本里写不了中文模式串）：不划算时 `跳过瘦身 sigma=… delta=… net=… ctx=… tokens —— 判为省小废大（省下的不比废掉的多），一个字都不动`；动手时 `瘦身决策 sigma=… delta=… net=… ctx=… tokens —— 省大废小，动手（命中折扣 50×）`。
      - **实测（2026-09-21，`core-agent\target\hooktest\e2e-a15.ps1`，真 `agent.exe` + 假端点，17 条断言全过）**：假端点报 `cache_read` 把水位顶到 **0.9**（进瘦身档、避开 0.95 的丢弃档），两问各造一种形状 —— ① **省小废大**：小块 2098 字 + 其后 10 轮胖工具往返 ⇒ `sigma=2098 delta=14293 net=-609750` ⇒ 判为不划算：**一个字都没动**（触发那一次请求 `elided=0`，且该问 12 个请求在线上**逐条纯追加**、小块内容逐字节不变）；② **省大废小**：大块 7993 字 + 其后 5 轮瘦工具往返 ⇒ `sigma=7993 delta=2800 net=259650` ⇒ 动手，且**只动那一条**（靠前那条不划算的小块在线上仍逐字节不变，`common` 从 26 断到 35 ⇒ 改写点正好落在大块位置）。这两条就是本模型的行为契约：**不划算时零改写、划算时只改该改的**。
      - 配套：`compact_history()` 返回 `CompactOutcome{elided, dropped, dropped_msgs, pinned}`，「扫了一圈但决定不动」时**不推进 `last_compact` 滞回时钟**（否则会白等一个 15% 增长窗口，且压缩计数被污染）。
    - **判断「命中率是否真的偏低」要先对账**：按 §9.1 难点 1 的做法，用 `requests[]` 与平台逐行对齐，并拿单轮数据去比「上一轮上下文 ÷（上一轮 + 本轮新增）」这个上限公式。2026-09-15 实测：dev 实例真实记录合计命中率 **77.4%**，单轮恰好贴着上限（如 `in=4612 / read=15744` → 77.3%）⇒ **低位来自会话短，不是缺陷**；不要为了「向 dsh 的 97–99% 看齐」去改结构。
    - **思考开关不直接进前缀**：thinking 只进 `display`、**不进 history**（回灌会 400）；其 400 降级只改 `thinking` 形态与 `max_tokens` 两个生成参数（`Thinking` / `max_tokens_for`），是否掉缓存取决于端点侧 hash 口径（本仓库无法自证）。但**切思考开关会重启 agent ⇒ history 清空 ⇒ 缓存必然重建**，这是「切换后命中率骤降」的合理解释，属预期行为。
    - **思考只有开 / 关两态（2026-09-15，不得回退）**：端点**没有**思考力度旋钮（`budget_tokens` 不被 enforce、`effort` 字段被静默忽略，实测见 §3.5），因此**禁止**再把档位做成「快速 / 思考 / 深度」这类深度分级、也禁止把 `budget_tokens` 暴露给用户 —— 那是在承诺端点做不到的事。开关值只有 `on` / `off`（`LUNAC_THINKING`），`budget_tokens` 退化为单一常量 `THINKING_BUDGET`。
    - **注入提示的「固定 / 条件」位置纪律（2026-09-17，方案 B，不得回退）**：给模型的行为约束按「是否随请求变化」分成两类，**落点不同**：
      - **固定块**（人格 / 文风 —— 每次都要生效、内容与请求无关）**必须放进 agent 的系统提示词**（`core-agent/src/main.rs` 的 `PERSONA_AND_STYLE`，与 `SYSTEM_PROMPT` 拼接）。它是固定前缀的一部分 ⇒ 永远命中缓存。**2026-09-21 起这一段是两截**：内置常量 `PERSONA_AND_STYLE` + 用户在设置里写的那段（`config\persona.md`，**启动时读一次**，见 §3.5「人格 / 自定义提示词」/ §11 规则 66）—— 「可配置」与「进固定前缀」这两件事必须同时成立，别为了让用户能改就把它挪进消息侧。**2026-09-29（L6 降本）在这两块各加了一条**：`PERSONA_AND_STYLE` 加「输出纪律」（只把输出 token 花在答案上：无开场白 / 不预告工具调用 / 不复述刚读到的内容 / 只答被问的 / 不贴回用户已能看到的正文），`SYSTEM_PROMPT` 加「批量化」（互不依赖的读 / 搜放进**同一次响应**）。两条都是**与请求无关的常量** ⇒ 位置纪律不变、仍进固定前缀；实测见 §9.1 结论 7（**输出纪律 -41%，批量化未生效**）。改这两块会**打掉一次端点侧缓存**（每台机器每套前缀各一次，不是每次提问都付）—— 别因为怕掉缓存就把新句子挪进用户消息。
      - **条件块**（`## Debugging Methodology` / `## TDD Requirement` / `## Code Review Pipeline` —— 按 query 关键词命中）**只能留在用户消息里**，因为随 query 变；搬进系统提示词会让提示词每轮都变、把整个固定前缀打掉（比不搬更糟）。
      - **为什么这条是硬约束**：原本两块固定文案（1153 字符 ≈ 288 token）由前端 `buildSystemPromptHint()` 拼在**每条用户消息最前面**，位置决定了它**每次提问都必然未命中**（新用户消息天生不在上一轮缓存前缀里）。实测纯问答类提问的首请求未命中量 `in = 236 / 289 / 313` token 与它几乎相等 ⇒ **首请求未命中的约 90% 就是这两块**。搬进系统提示词后它们进入固定前缀，从此零未命中。
      - 守门测试：`core-agent` 的 `system_prompt_is_stable_and_carries_persona`（同一 cwd 下逐字节可复现 + 两块文案确实在提示词里）。**前端 `buildSystemPromptHint()` 返回空串是合法状态**（不含关键词的提问就是空），`wrappedQuery` 与 `cleanUserContent()` 都按「没有 `\n\n---\n\n` 分隔符」处理。
    - **前缀指纹埋点（2026-09-17 新增，不得精简掉）**：agent 在**每次** API 请求发送之前落一行 `请求前缀 #n system=<hash>/N字 tools=<hash>/N字 history=<hash>/N条/N字`（`log::hash64` = FNV-1a；**只记哈希不记原文**，前缀里可能含用户文件内容，哈希天然满足脱敏硬要求）。**为什么必须有**：同一个会话内跨提问时 `read` 会莫名回落（实测 `usage-2026-09-16.jsonl`：记录 6 末次 `read=3712` → 记录 7 首次 `read=2304`，丢 1408；记录 8 → 9 丢 3584），而**静态读用量日志无法区分**「本侧前缀被改写」与「端点侧淘汰了已落盘单元」—— 两条曲线的形状完全一样，再怎么对着 `requests[]` 看也判不出来。判据：三块指纹与上一轮逐字节相同而 `read` 掉了 ⇒ 端点侧淘汰，本侧无责；某一块的指纹变了 ⇒ 本侧改的，直接去那块找原因（`system` = 身份/环境块/技能清单，`tools` = 工具 schema，`history` = 历史）。
    - **端点侧「缓存前缀单元」机制（2026-09-20 实测，规则 23 最重要的一条补充）**：DeepSeek 的命中**不是**朴素的「最长前缀匹配」—— 官方注 1/注 2 只说了「64 tokens 为一个存储单元」「尽力而为，不保证 100% 命中」，实测行为更接近「**独立的完整单元**」：① 每次请求的「用户输入结束位置」与「模型输出结束位置」各落盘一个单元；② 系统还会把多次请求的**公共前缀**单独落盘成一个单元；③ 后续请求要命中某个单元，必须**完整匹配**它。
      - **推论（最反直觉的一条）**：`history` **末尾的消息形态**就能决定整段前缀能否命中 —— 不只是「改了中段才失效」。单变量对照（全部打满 `MAX_TOOL_ROUNDS`(16) 后的第 17 个请求，`公共前缀` 一列均为 `31/31条`，即本侧逐字节零改写）：

        | 第 17 轮 `history` 末条形态 | 条数 | `read` | 第 17 轮命中率 | 整次提问汇总 |
        |---|---|---|---|---|
        | `user(tool_result)`，无收口指令（对照） | 33 | 11776 | 92.7% | 91.7% |
        | `user(tool_result)` **+ 新 `user(收口指令)`**（旧实现） | 34 | 2560 | 23.3% | 87.2% |
        | `user(tool_result, text)`（多追加一个 content 块） | 33 | 2560 | 23.3% | 85.9% |
        | `user(tool_result 文本内含收口指令)`（**现实现**） | 33 | 10880 | 91.3% | **95.9%** |

      - 崩塌时 `read` 恰好落到 `system + tools` 的大小（≈2560 tokens）—— 那正是「公共前缀检测」落盘的那个单元，说明**没有任何「请求结束位置」单元被匹配上**。
      - **因此结论**：收口指令这类追加内容，**只能拼进已有 `tool_result` 的文本内部**（`TOOL_BUDGET_HINT`，[core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs)）；**不得**新增消息，**也不得**在 content 数组里新增内容块。
      - **这不是「连续两条 `user` 就失效」**：纯文本 20 轮探针（无 `tool_use`/`tool_result`、无 `tools` 参数、`total` 一路涨到 61k）里追加同样的双 `user` 消息，`read` 全程精确接力（`read` ≈ 上一轮 `total`，20/20 轮全部正常）。所以它是**工具消息形态**特有的交互。
    - **逐条指纹 + 逐请求命中率埋点（2026-09-20 新增，2026-09-21 扩到「跨提问」，不得精简掉）**：`请求前缀 问#<N> #<n> system=…/…字 tools=…/…字 history=…/…条/…字 公共前缀=N/M条`（`公共前缀` = 与本轮 `history` **逐字节相同**的条数 / **上一次请求**的总条数；`N < M` 时自动追加「← 本侧就地改写了历史」），`message_stop` 后多一行 `请求用量 问#<N> #<n> in=… read=… create=… out=… 命中率=…% 公共前缀=…条`（`问#<N>` = 第几次用户提问、`#<n>` = 本问内第几次请求，两者都只进日志：离线看日志时不用再靠时间戳猜边界）。
      - **为什么必须有**：整体的 `history` 哈希对「是否就地改写」**没有分辨力**（只追加也会让它变），而 B 类崩塌正是被这一列钉死的 —— 第一轮改完就打出 `公共前缀=31/31条`，直接证明本侧零改写、责任在端点侧，省掉了整轮瞎猜。
      - **指纹必须跨提问存活（2026-09-21，A16）**：`prev_hist_hashes` 由 `main()` 持有、按 `&mut` 传进 `run_query`（判定收口在 `common_prefix_len()`，有守门单测 `common_prefix_counts_only_byte_identical_messages`）。它早先是 `run_query` 的局部量 ⇒ **每一问的首请求都只会打 `0/0条`**，「两次提问之间本侧有没有改写 history」在日志里**根本没有证据** —— 埋点必须能回答它自己提出的那个问题，否则等于没有。
      - **跨提问判据（A16，2026-09-21）**：看**本问首请求**那一行（`问#N #1`）的 `公共前缀` —— 等于**上一问末请求**的 `history` 条数（`M`）⇒ 纯追加，本侧无责，`read` 掉了就是端点侧的事；小于 M ⇒ 本侧在第 N 条改写了历史。另有一个独立签名：`history=1条` 说明这是**新进程的首请求**（新进程没有上文），此时 `read` 落到 `system + tools` 是**正常现象而不是缺陷** —— 「跨提问接力」这个词只对**同一进程**成立。
      - **实测结论（2026-09-21，`core-agent\target\hooktest\e2e-a16.ps1`，14 条断言全过）**：假端点抓**线上报文**逐条比对 `messages` 数组（元素级、重序列化后按字符串比），四问跑在**同一进程**里、其中一问跑满 16 轮工具往返（即 B 类崩塌那条路径 + `TOOL_BUDGET_HINT` 收口）：**20 个请求逐条都是纯追加**（`common == prevmsgs`）、`system` / `tools` 哈希全程逐字节不变 ⇒ 跨提问的首请求就是上一次请求的**严格延长**，端点侧具备最大复用条件。与两组真实数据点一致（同一进程的第二问首请求 `read=81792`，`usage-2026-09-18.jsonl`；`read=3840` / 94.3%，09-20 plan e2e）⇒ **A16 的「只命中 system + tools」不是本侧改写造成的**：要么那一次的第二问其实是**另一个进程**（`history=1条` 签名），要么是端点侧淘汰。
      - **真实端点收官实测（2026-09-21，`core-agent\target\hooktest\e2e-a16-live.ps1`，flash）**：同一进程两问、中间隔 2 秒 —— `问#1 #1 in=3001 read=128`（新进程首请求，固定前缀与上一轮不同 ⇒ 几乎全 miss）、`问#2 #1 公共前缀=1/1条 in=204 read=2944 命中率=93.5%` ⇒ **接力在真实端点上成立**，`read≈system+tools` 那个形状今天在**同一进程**里复现不出来。注意 `问#1 #1` 只有 `read=128` 这件事本身也是一条判据：**新进程的首请求不一定命中到 `system + tools` 全额**（固定前缀是否逐字节相同取决于 cwd / 技能清单 / 工具表，见规则 18）—— 所以「第二问首请求 read 很小」既可能是新进程、也可能是固定前缀自己变了，两者要用 `请求前缀` 行的三个哈希与 `history=` 条数分开。
      - 配套：`上下文压缩` 日志由 `eprintln!` 改为 `log::info`（原先是靠宿主把 stderr 以 warn 级镜像进 lunac 日志才看得到 —— A 类崩塌的唯一物证 `瘦身 23 个 tool_result（省 176101 字）` 就藏在那里）。
      - **归因套路（下次再遇 `read` 骤降照这个走）**：先看 `公共前缀` 是否等于上轮条数 ⇒ 定性本侧/端点侧；本侧则顺着 `上下文压缩` 行找是哪一档压的；端点侧则检查**本次请求与前一次的末条形态差异**（B 类就是这么找到的）。
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
    - **白名单只有 6 个**（`tools::parallel_safe`）：`Read` / `Glob` / `Grep` / `WebSearch` / `WebFetch` / `TodoWrite`。**新增工具默认串行** —— 要进白名单必须先自证「只读、不落盘、无全局状态」。**`Skill` 在 2026-09-20 被移出**（A5）：判据是**只看名字**的，而 `Skill` 分 inline（纯读）/ fork（派子代理发 API、能写文件）两种模式，名字判不出来 ⇒ 按最坏的那种算。守门单测 `only_read_only_tools_may_run_in_parallel` 把 `Skill` 钉在「不该并行」一侧，改这段必跑。
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
    - **历史存储 = SQLite（`<exe 根>\ModuleData\history\chat.db`），不再是 `chat-history.json`**（原 backlog §8.5 的第一步，已落地；剩余部分见 backlog **A2 / A4**）：单文件 JSON 的全量重写随历史变长而变差，且做不了检索。**库文件存在即唯一真相源**，旧 JSON 只在库不存在时被导入一次（迁移后**保留**旧文件不删，否则用户删掉的会话会「复活」）。实现在 [chat_db.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/chat_db.rs)。
    - **FTS5 索引随写入用触发器维护**，不许改成「先写数据、以后再补索引」（回填极易漏）：两张表 —— 默认 unicode61（英文 / 代码词）+ `tokenize='trigram'`（**CJK 子串检索唯一可行的一条**，默认分词器对中文不切词，`MATCH` 永远命中 0）。`rusqlite` 用 `bundled` 且必须**实测** FTS5 可用（`chat_db::tests::fts5_is_compiled_in` —— 构建配置问题不实测就只能在线上炸）。
    - **`sessions.pos` / `messages.idx` 两列不许省**：旧 JSON 是数组、顺序即语义，而 SQLite 没有隐式顺序，靠插入序或主键序会**静默改变历史列表排列**。
31. **唤出（热键）路径的顺序不变量（2026-09-17）**：`toggle_window()` 里 **`force_foreground()` 必须是最后一步**，顺序固定为 ① `emit("lunac-window-shown")` + 剪贴板读取排队 → ② `refresh_if_stale()` → ③ `force_foreground()` → ④ `LAST_TOGGLE_TICK`。
    - **理由**：`AttachThreadInput` 的等待时间不可控（它要接前台线程的输入队列）。排在 emit 之前时，前端要等激活做完才收到 `lunac-window-shown`，而该事件正是「唤出后主动重跑当前查询」的唯一入口 —— 高负载下用户看到的就是「呼出后卡在上次搜索结果」。`LAST_TOGGLE_TICK` 仍必须在 `force_foreground` **之后**写（写早了会被前台守卫当成「冷却已过」而自动隐藏）。
    - **前端必须在 `lunac-window-shown` 里主动重跑当前查询**（`refreshSearchResults()`，限非插件 / 非抽屉态）：Rust 的唤出路径**不会**重跑搜索 —— `hide_window()` 只有一行 `ShowWindow(SW_HIDE)`（不清结果、不发事件），唯一的重算链是「剪贴板事件 → 合成 input → 60ms 去抖」，而剪贴板没变化时整段不执行 ⇒ 静态帧一直停在上次结果。复用 `refreshSearchResults()` 而不是直接调 `runSearchNow()`：走同一套去抖 + 序号校验，不会与正在输入的字抢渲染。
    - **剪贴板读取命令一律 `#[tauri::command(async)]`**（`read_clipboard_files` / `read_clipboard_backup_image`）：同步命令跑在 Tauri 主线程上，而它们恰好在唤出的那一刻被调用 —— 一张 4K 截图展开成 BMP 就足以拖住主线程。两者都是 `OpenClipboard(0)` 开头的纯 Win32 FFI + 文件 IO，**无线程亲和性**，放工作线程安全。
    - **`show_and_focus()`（托盘 / 菜单 / 单实例）已一并覆盖（2026-09-19）**：这三条唤出路径此前**不发** `lunac-window-shown` 也不读剪贴板，导致「从托盘唤出时静态帧残留、且不回简洁搜索」。现在两条路径共用 `notify_window_shown()`，行为与热键完全一致 —— 详见规则 40（含 `show_and_focus()` 必须写两次 `LAST_TOGGLE_TICK` 的理由）。
32. **被改动文件的路径追踪（2026-09-17，原 backlog §8.1 —— 已完成）**：会话里被 `Write` / `Edit` 动过的文件必须**可点击定位**，并在对话流末尾给出「本次会话改动过的文件」列表。
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
    - **Chromium 侧的依据 + 已实测复核（2026-09-29）**：解析 argv 时重复的 `--disable-features` 逐项逗号合并（union），所以追加同名 switch **不会**挤掉 WebView2 自带的那份。**实测取证**（dev 实例 `lunac.exe` → WebView2 浏览器进程，读 `Win32_Process.CommandLine`）：浏览器进程拿到的 `--disable-features` 是 **`msWebOOUI,msPdfOOUI,msSmartScreenProtection,PermissionPrompt,ClipboardContentRead`** —— 五条**全在**（子进程里顺序会重排成 `ClipboardContentRead,PermissionPrompt,msPdfOOUI,msSmartScreenProtection,msWebOOUI`，同一个集合），同一命令行上 `--js-flags=--scavenger_max_new_space_capacity_mb=8` 也在。⇒ **合并语义成立，不必改成「并入同一个 switch 的值」**（原登记的待复核项已据此关闭）。复跑判据：`Get-CimInstance Win32_Process | ? { $_.Name -like '*webview2*' }` 看 `CommandLine` 里的 `--disable-features`。
    - **顺带的能力**：合并语义使 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 成为一个**免编译的 A/B 入口** —— 在普通 shell 里设成 `--disable-gpu` 再启动即可试旗标，不必重新构建。**注意 Trae 沙箱会拦掉 `D:\Lunac\temp\*` 的写入**，`WebView2` 环境创建直接失败（表现为「进程数为 0」），所以 A/B 必须在**普通 PowerShell** 里跑。
    - **任何时候都要留痕**：合并后 `log::info` 记最终值（对照规则 36 的埋点纪律）。
39. **摘要式压缩：只在丢弃档触发、失败必须降级（2026-09-18，原 backlog §8.2 —— 已完成）**：机械压缩（瘦身 / 丢弃）**不额外调模型**，是默认且无条件执行的那条路；摘要压缩是它的**可选补强** —— 当丢弃档真的扔掉一大段历史时，花**一次** API 调用把它压成摘要钉回历史开头，而不是只留一句 `TRIMMED_MARKER`。实现：`render_dropped_for_summary()` / `summarize_dropped()` / `pin_summary_of_dropped()`，提示词常量 `SUMMARY_PROMPT`。
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

40. **唤出路径必须同构：热键 / 托盘 / 菜单 / 单实例走同一条通知链（2026-09-19 修，不得只改一边）**：`hotkey.rs` 把「通知前端」抽成 `notify_window_shown()`（= `emit("lunac-window-shown")` + 在主线程读一次剪贴板），`toggle_window()` 与 `show_and_focus()` **都必须调它**。
    - **症状（用户报的「有时候呼出时卡在上次搜索页 / 显示上次搜索的残影」）**：`show_and_focus()`（托盘菜单 「Show / Hide」、托盘左键、单实例「又启动了一次」）此前**只做 `ShowWindow` + `force_foreground`**，既不发 `lunac-window-shown` 也不读剪贴板。而前端把该事件当作唤出的**唯一**信号 —— 监听器里要做四件事：`suppressResizeAnimBriefly()`（抑制高度滑动）、`exitDetailSilently()`（大界面不跨隐藏存活）、`syncUiMode()`（重报界面层）、`refreshSearchResults()`（主动重跑当前查询）。缺了这一发 ⇒ 前端停在上次的状态不动 ⇒ 用户看到的是**静态旧帧**，要等下一次交互才恢复。这正好解释「**有时候**」—— 热键那条路一直是好的，只有另外三条路坏。
    - **`show_and_focus()` 要写两次 `LAST_TOGGLE_TICK`**（`ShowWindow` 前一次、`force_foreground` 后一次），**这不是冗余**：它走的是 `SW_SHOW`（窗口此前不可见），只写最后一次的话，`force_foreground()` 期间前台守卫会读到上一次的旧 tick（`now - last >= 2000`）而把刚显示的窗口又藏掉。规则 31 说的「必须在 `force_foreground` 之后」针对的是 `toggle_window()` 那条已经在可见窗口上切换的路径 —— 两条路径的约束不冲突，实现里两次都写是同时满足两边的最省心做法。
    - **顺序仍是规则 31 那套**：`LAST_TOGGLE_TICK`(挡守卫) → `ShowWindow` → `notify_window_shown()` → `refresh_if_stale()` → `force_foreground()` → `LAST_TOGGLE_TICK`(冷却起点)。**`force_foreground()` 依旧最后**，理由同规则 31（`AttachThreadInput` 等待时间不可控，前端不该等它）。
    - **`read_clipboard_files()` 是 `unsafe`**：`notify_window_shown()` 本体不是 `unsafe fn`，所以那一行要显式 `unsafe { }` 包起来，并在注释里写清前提（闭包由 `run_on_main_thread` 投递，满足「必须在主线程」的约束）。
    - **验收口径**：托盘菜单唤出 → 日志里应能看到 `lunac-window-shown` 的连带效果（例如 `set_ui_mode` 的重报、简洁搜索的重跑）；从**详细搜索**里托盘唤出 → 必须落回简洁搜索，而不是停在大界面。
41. **系统动作只有一张表：`action_spec()`（2026-09-19，覆盖 §2.1.2 的目录清单）**：`system_catalog.rs` 里「动作怎么被拉起来」的唯一真相源是 `action_spec(id) -> Option<ActionSpec>`；`run_action()` / `run_action_elevated()` / `CatalogItem::elevatable` **三者都从它推导**。
    - **为什么必须收成一张表**：旧实现是 `run_action()` 里一个大 `match`，而「有没有提权形态」要靠界面另外声明 —— 两处必然漂移，漂移的表现是「界面上画了盾牌、点下去说『该动作没有提权形态』」。现在新增一个动作 = `all()` 加一条 + `action_spec()` 加一个分支，两道闸门由单测 `every_action_id_has_an_execution_spec` 把住（漏了第二个分支就编译过、测试红）。
    - **两种启动形态**（`enum Launch`）：`Argv(&[&str])` 用 `Command::spawn` 起（**不走 shell**，无拼接面）；`Shell(&str)` 交给 `ShellExecuteW("open")` —— `.msc` / `.cpl` / `shell:` 这类目标 `Command::spawn` 根本起不来，必须由 Windows 解析。
    - **提权形态一律 `ShellExecuteW` 的 `runas` verb**（`app_indexer::launch_elevated(file, params)`，也是全项目**唯一**一处主动请求 UAC 的用户界面入口；另一处是 `auto_start.rs` 创建登录计划任务，只在自启修复时用）。**本进程自身不是提升的**（`asInvoker`，见规则 29），所以 UAC 一定会弹，且提权后的目标与我们**不在同一个权限上下文**。
    - **用户取消 UAC 必须让用户看见**：`ShellExecuteW` 返回 `SE_ERR_ACCESSDENIED (5)` 时返回中文错误而不是静默失败；前端 `flashDetailHint()` 把原因显示在按键提示行上（详情页没有 toast 设施）。`launch_elevated` 成功时落一行 `log::info`。
    - **提权入口只对「有提权形态的东西」开放**：设置页（`ms-settings:`）永远 `elevatable: false`；动作看各自的 `action_spec().elevate`；应用/文件看扩展名白名单（`DETAIL_ELEVATABLE_EXTS` = exe/lnk/msc/cpl/bat/cmd/com，目录一律排除）。**前端判据与 Rust 同源**，Rust 侧还会再校验一次（多传一个 id 只会拿到 `Err`，不会变成「任意命令的提权执行入口」）。
    - **单测锁死约定**：`elevatable_set_is_exactly_the_reviewed_list` 把当前 21 个可提权 id 写死 —— 多一个（偷偷开了提权）少一个（界面漏了盾牌）都会红。改这张表必须同时改那条断言。
    - **2026-09-19 补齐的 16 条系统命令/工具**（此前「Win+S 搜得到、Lunac 搜不到」）：`act.cmd` / `act.powershell` / `act.regedit` / `act.gpedit` / `act.services` / `act.diskmgmt` / `act.compmgmt` / `act.eventvwr` / `act.perfmon` / `act.taskschd` / `act.appwiz` / `act.ncpa` / `act.sysdm` / `act.optionalfeatures` / `act.cleanmgr` / `act.mstsc`。关键词里**显式带上 `.msc` / `.cpl` 与 `appwiz` / `ncpa` 这类文件名** —— 用户就是照 Windows 搜索框里的写法敲的。
    - **已知取舍**：`gpedit.msc` 只在专业版/企业版存在，`optionalfeatures.exe` 在精简版可能缺失，`mstsc.exe` 在家庭版基础版缺 —— 这三条**照常列出**，起不来时由 `ShellExecuteW` 的错误码回给前端（统一给「启动失败」提示），不做「先探测存在性」——那要求搜索路径做 OS 查询，违反本文件头部的第 ① 条硬约束。
42. **详细搜索的检索能力与上限（2026-09-19，替代原先的三层钳制）**：文件结果上限 `DETAIL_FILE_LIMIT` 40 → **200**（= `file_indexer::MAX_RESULTS` 的硬上限，前端不再自我设限）；应用 6 → **20**；插件命令 6 → **12**；设置页/系统动作 8 → **20**。
    - **文件名的兜底匹配（`file_indexer::rank` 第二轮）**：在第一轮（精确 1000 / 前缀 700 / 包含 500 / 多词 400，语义未变）**没凑够 limit** 时才跑，补两档 ——
      - **模糊（子序列）**：查询字符按顺序出现在文件名里即命中（`rjl` → `recent-journal-list`），分数落在 **200 一档**；跨度越大、名字越长扣分越多。
      - **拼音**：仅对含汉字的文件名，全拼与首字母各试一遍，**首字母整体比全拼低一档**（340 / 300）——「weixin」命中「微信截图」比「wx」命中更确定，而两字母首字母几乎能匹配一大片中文名，不降档就会把全拼命中盖掉。
    - **两档都低于「包含」档（500）**：兜底命中不得把精确命中挤下去。单测 `subsequence_matching_is_a_fallback_tier` / `fuzzy_only_fills_the_gap_left_by_exact_passes` 把这条锁住。
    - **门控不是优化而是必需**：单字母查询在第一轮就填满 200 条，再为它扫一遍全表纯属浪费；空查询（「最近文件」）**绝不能**进第二轮，否则「按修改时间倒序」会被模糊命中打乱。
    - **拼音必须封顶**：`PINYIN_SCAN_CAP = 30_000` 个文件名/次。`pinyin` crate 的转换要分配字符串，而索引上限是 30 万条 —— 不封顶就是每次击键白烧几十毫秒（`search_files` 是 `#[tauri::command(async)]`，跑在工作线程，但也不能无限烧）。查询长度另受 `FUZZY_MAX_QUERY_LEN = 12` 约束，且只有**单 token + 纯 ASCII** 才走拼音。
    - **复用而非重写**：拼音口径（`has_chinese` / `generate_pinyin_tokens`）直接复用 `app_indexer`，两者已改为 `pub(crate)` —— 各写一份必然漂移。
    - **已知边界**：文件索引**不扫** `C:\Windows` / `Program Files` / `AppData`（见 `SKIP_DIRS`，26 项），所以 `cmd.exe` / `powershell.exe` / `regedit.exe` **搜文件搜不到** —— 它们由规则 41 的 `act.*` 系统动作提供（在「动作」分类里命中）。这两条路是互补的，不是重复。
43. **日志分级与降噪（2026-09-19）—— 只降噪，不删证据**：中等级别的精简，落到四条改动上。
    - **降级**：`core-agent` 的 `等待审批 {tool}` 由 `eprintln` 改为 `log::debug`（默认级别 info 下不输出，`LUNAC_LOG_LEVEL=debug` 可开）。理由：**每个需要审批的工具调用**都会走它，而 `eprintln` 会被宿主 `commands.rs:584` 以 **warn 级**镜像落盘 ⇒ 一行普通进度被记成了警告，实测占 dev 日志行数的三分之一。审批**结果**（超时 / 用户拒绝）仍是 warn / info 级，不受影响。
    - **删除纯重复**：`core-agent` 的 `eprintln!("[agent] 执行工具 {name}")` 直接删 —— `run_tool()` 的 `tool {name} ok (…ms, out … chars) args=…` 已经记全了同一件事（连耗时和参数摘要都有），前者只是把它再镜像一遍。
    - **合并同文本双写**：`set_history: N 条` 与 `stdin closed, exiting` 各有一对 `eprintln` + `log::info`，各删 eprintln 只留 `log::info`（agent 日志）—— 同一件事不再在两个日志文件里各占一行。
    - **仍然保留在 stderr 的**（**不得顺手删**）：`忽略输入类型`（规则 35 的判据）、`忽略非法 JSON 输入行`、`MCP 桥未接通`、`用户输入过长`、`端点不接受 thinking`、`端点回报上下文超限`、`工具轮次达上限`、`用户要求中断本轮`、`P0/P1 启动横幅` —— 这些都是**低频且定位问题必需**的。
    - **规范明文要求保留、任何时候都不得降级的日志**（散落在规则 20/23/25/26/27/28/35/36/38/39 以及 §3.5 里）：`请求前缀 #n`（含 system / tools / history 三个前缀哈希与字数）、`摘要压缩：…`、`上下文压缩：…`、`api retry`、`tool X ok` / `tool X FAILED`、`输出超预算（… 全文已落盘）`、`只读工具并行批 N 条`、WebView2 profile 与 browser args 两条、`AI 配置来源` / `agent 凭据`（只留 key_tail）、`auto_start`、`[frontend]`、`shell[...] exit=`、`文件索引已重建`、`launch_elevated`。
    - **口径**：日志精简的目标是**单位行数的信息密度**，不是行数本身 —— 所以「删掉能证明结论的那一行」永远是错的，要删的是**同一事实的第二、第三遍转录**。

44. **结果区高度完全由行数决定（2026-09-19 改，不得回退成固定夹取）**：`#results-container` **不再有** `min-height: 120px` / `max-height: 380px`；高度 = 内容高（12px padding + N × 56px，`.result-item` 实测 56px/行）。唯一上限是 `max-height: var(--results-max-h, none)`，值由 JS 写。
    - **用户报的症状与实测对账**：「呼出时高度被钉在 200 左右」。拆开是两件事：① `min-height: 120px` 让**2 行**的结果区被撑到 120px（搜索栏 ~52 + 120 + 状态栏 ~30 ≈ **202px**）—— 这就是那个 200；② `max-height: 380px` 让第 7 行起只能内部滚动。用户要求「一条记录一格、每格 ~60px」，故两段都去掉。
    - **上限必须由 JS 按屏幕算，不能用 CSS `100vh`**：本窗口是**内容驱动**高度（`#app` 为 `height:auto`，量完再 `setSize`）。用 `vh` 就构成「窗口高 ← 内容高 ← vh ← 窗口高」的循环，实测会抖。`resultsMaxHeight()` 取 `window.screen.availHeight − SEARCH_CHROME_PX(120)`，只依赖显示器，是干净的常量；超上限时 `#results-list` 的 `overflow-y: auto` 接管（本来就有的内部滚动）。
    - **`--results-max-h` 用行内 style 写是刻意例外**：值每块屏都不一样，写进 class 或样式表做不到；且它只是喂给 CSS 的一个数字，不与任何 class 规则争优先级。code-rules §5.6「class 优于 inline style」针对的是**会被 class 覆盖的样式属性**（如 `overflow`），不是这种动态数值。
    - **插件态不受影响**：`#app.plugin-active #results-container { max-height: none }`（以及 `#app.detached ...`）特异性高于单 id 规则，AI 对话 / 分离窗口照旧撑满窗口。
    - **`applyResultsMaxHeight()` 必须在 `applyWindowSize()` 里、测量之前调用** —— 否则量到的是「没封顶」的高度，量完再改就又触发一轮 resize。
    - **顺带修掉「唤出后高度不对、过一会才自愈」**：`requestWindowHeight()` 有两条会静默跳过 `setSize` 的早退（`h === requestedHeight` 缓存命中、`|h − currentWindowHeight| < 3`）。这两条在**键入**时是对的（防 IPC 风暴与 setSize→onResized 回环），但**唤出**时是错的：窗口在隐藏期间的真实高度可能已被改动，而 `requestedHeight` 还留着旧值 ⇒ 测量与缓存一致 ⇒ 一声不响不下发。现在 `lunac-window-shown` 的第一件事是 `forceHeightReassert()`（`requestedHeight = -1` + 双 rAF 重测），保证这一轮**必定**落地。
    - **往 `#results-list` 增删条目 = 必须重新断言高度，且必须是同步测量（2026-09-19 修，不得只靠 ResizeObserver / rAF）**：用户报「唤出时第 3 项被截掉一半，直到搜索变动才恢复」。根因是空查询常驻项**分两步**渲染：`renderAIEntry()` 只渲染 2 项（Web 搜索 + AI）并**当场** `applyWindowSize()` 下发 2 项高（实测 199），随后 `renderClipboardOCREntry()`（剪贴板有图时）把 OCR 项插成第 3 项（需 255）**却没有补测** ⇒ 窗口停在 2 项高度，第 3 项只露出 46 − 33 ≈ 23px。`renderAIEntry` / `renderMixedResults` 收尾都同步调了 `applyWindowSize()`，只有这条插入路径漏了 —— 现在 `renderClipboardOCREntry()` 末尾同样同步调用它。**为什么必须是同步而不是等 RO / 双 rAF**：唤出瞬间 WebView 常仍被判定为「未渲染」，ResizeObserver 回调与 `requestAnimationFrame` 都会被推迟（隐藏页面里 rAF 干脆不跑），靠它们兜底就等于把这个错误高度一直留在屏幕上；`getBoundingClientRect()` 会强制一次布局，当场就能量到真值。浏览器实测：修复后「插入第 3 项」与「`setSize(254)` 下发」发生在**同一 tick（差 0ms）**，修复前要等 RO 的 60ms 防抖 + 定时器约 135ms。**纪律：任何改动 `#results-list` 条目的函数，收尾都要量一次高度。**
    - **已知取舍**：窗口变高时左上角不动、向下生长（`tauri.conf.json` 的 `center: true` 只管创建那一刻）。结果很多时窗口会一直向下长到屏幕底边附近 —— 被 `--results-max-h` 截住后转为内部滚动。**没有**做「贴底时自动向上生长」的重定位。

45. **外观 / 主题：配置在 localStorage、主题包在文件、落地只走 CSS 变量（2026-09-19 新增；2026-09-20 三次重构「主题颜色」）**：设置 → **风格** 分区，现在五块能力 = 背景（三按钮 + **4** 滑块）+ 主题颜色（**恢复默认主题**开关置顶 + **底色自定义** + **按钮自定义** + **文字自定义** + **其他颜色**）+ 整套配色派生 + 主题包。
    - **主题色取色器已删除（2026-09-20 第二次重构，用户要求「去除主题色取色」，并选「连字段一起删」）**：设置里的那一行没了，`Appearance.customAccent` 字段也没了，`buildAppearancePane` 不再挂 `ap-accent`。**主题色现在只有一个来源：主题包的 `tokens.accent`**（`main.ts` 的 `themeAccent()`；主题读不到 / 损坏时回落 `DEFAULT_ACCENT = "#c0a0a0"`）。
      - **副作用（必须知道，不是 bug）**：老配置里存过的自定义主题色会**失去载体**，渲染回落到当前主题声明的 accent（默认主题 = `#c0a0a0`）。选主题时也不再「顺手把主色写进配置」——那段代码已删。
      - 已删的键：`settings.appearance_color_picker`（i18n）。别在 `#ap-color-group` 里再挂 `colorPickerHtml("ap-accent", …)`。
    - **「跟随 Windows 强调色」已整体下线（2026-09-20，用户指定）**：「取色方式」（自定义 / 跟随 Windows）分段按钮、`Appearance.colorMode` 字段、`systemTheme` 缓存、`refreshSystemTheme()`、启动时那次无条件取色、`lunac-window-shown` 时的取色、**15s 轮询**、设置里的「当前系统色 / 刷新」那一行，**全部删除**；`i18n` 里对应的 5 个键（`appearance_color_mode` / `appearance_color_custom` / `appearance_color_system` / `..._current` / `..._refresh` / `..._unavailable`）一并删掉。**影子值**：老配置里的 `colorMode` 变成未知键，读进来被忽略、下次落盘自动消失。
      - **Rust 的 `get_system_theme` 命令保留**（`appearance.rs`）：那里钉着 AccentColor `0xAABBGGRR` / ColorizationColor `0xAARRGGBB` 的**本机取证与单测**，删掉等于丢证据。但前端已无调用方 —— 回归时不要把「前端没有入口」当成 bug。
    - **三层优先级：用户配置 > 主题包 > `styles.css` 的 `:root` 默认值**。`main.ts` 的 `applyAppearance()` 是**唯一**合成点，结果写在 `<html>` 行内 style 上（同 `--results-max-h` 的例外论据：值来自用户配置，写不进样式表）。配置存 `localStorage.lunac-appearance`（本机偏好）；主题包存 `<exe 根>\themes\<名>\theme.json`（含图片资产，必须落文件才能导入导出/分享 —— 与 skill / tool 同一套 portable 约束，见规则 24）。旧键 `lunac-bg-image` **一次性迁移**进新配置后立即删除（留着就是第二个真相源）。
    - **整套配色由主色派生（2026-09-19 二次定稿；2026-09-20 那个开关先改名「主题色代替底色」并反转默认值、同日再改名「恢复默认主题」并扩大失效范围）**：`derivePalette()` 按主色的**相对亮度**（WCAG，非 HSL 的 L —— 纯黄 `#ffe066` 的 HSL-L 只有 70% 看着「中等」，相对亮度却 0.75，人眼就是觉得亮）派生六项：
      - **文字一律纯灰阶、不带色相**（用户明确要求「不采用红绿蓝色相调整，只在黑白之间渐变」）：`--text`/`--text-dim`/`--text-muted` = `hsl(0, 0%, 89→9% / 68→10% / 50→8%)`；
      - **文字明暗与主色明暗反向**：主色很亮 → 文字黑；很暗 → 文字白；
      - **底色明度与文字同步反向**（`--surface-rgb` 明度 11% → 83%）—— 否则「亮主色 + 黑字」会落在深底上，等于看不清；
      - `--border-glass` 取「文字那一侧」的对比色（深色主题白、浅色主题黑），否则浅底上看不见边框；
      - 转折点 = `DEFAULT_ACCENT`（`#c0a0a0`）的相对亮度 ⇒ `bright=0` 时逐项等于原配色（**默认外观几乎不变**）；中段用 `t=(bright−0.5)×2` 做**过渡带**，亮度落在 0~0.5 区间的颜色一律按深色主题处理（不做过渡带会得到「中灰底 + 中灰字」）。
    - **优先级顺序 = 主题包先写、派生后写、但主题包显式声明的底色不让位（不许调换）**：`applyAppearance()` 里 ④ 主题包 token、⑤ 派生。用户报的「#1C1A20 底色不跟随、有一层颜色蒙版」根因就是早期顺序相反 + 内置默认主题钉了 `surface: #1c1a20`（那层「蒙版」是 `#results-container::before` 的玻璃底色）。
      - **「主题全面代替底色」的落点**：⑤ 的跳过条件是 `(baseOv || surface)` —— **主题包声明了 `tokens.surface` 就以它为准**（此前是 ⑤ 无条件覆盖，等于主题包的 `surface` 完全是死代码）。默认主题已于 2026-09-20 **删掉这个 token**（用户要求「去除默认主题对底色的影响」），所以默认主题下这条恒为空、底色完全由派生 / 用户自定义决定 ⇒ 默认渲染不变。
      - **`themes/default/theme.json` 改完要同步 `app/src-tauri/target/debug/themes/default/theme.json`**：`themes_root()` = `<exe 根>\themes`（`storage::lunac_root_dir()`），dev 下 exe 在 `target\debug`，读的是那份**构建产物拷贝**（由 `tauri.conf.json` 的 `resources: { "themes": "themes" }` 复制）。只改源文件不重新构建的话，dev 实例仍按旧的 `surface` 渲染。
    - **「恢复默认主题」开关 + 三组自定义的失效语义（2026-09-20 三次定稿，用户逐条指定）**：
      - **开关本身**：文案「**恢复默认主题**」（`settings.appearance_restore_theme`），**摆在「主题颜色」整块的最顶上**（用户指定），不再夹在「底色自定义」面板里。字段名仍叫 `Appearance.tintBase`（`false` = 默认）—— **改名要动 localStorage 迁移，收益只有可读性**，所以只在注释里写明沿革。沿革：`底色跟随主题色` → `主题色代替底色` → **`恢复默认主题`**。
      - **开关关着（默认）** → 底色 / 按钮线条 / 按钮背景 / 文字明度**全部可生效**：没动过底色（`baseColor` 空串）就仍按主题派生，动过就用用户的值；主题包声明了 `surface` 时以它为准。**默认渲染与 2026-09-19 逐像素一致**。
      - **开关开着** → **回到一开始保存的那套默认主题配色**：`baseOv = null`（底色走主题包 surface 或 `derivePalette()`）、`colorsOn = false`（⑤c/⑤d 清空两个颜色内联值）。文字明度**不归零**（见下一条）。同时**锁住三组配色**（见下一条）。
      - **锁定范围（用户原话：「开恢复默认主题时除了透明度…以外的选项都不可调」，随后又明确「文字明度不锁定」）**：
        | | 项 | 开关开着时 |
        |---|---|---|
        | **锁** | 四个取色器（`#ap-base-picker` / `#ap-btn-line-picker` / `#ap-btn-bg-picker` / 其他颜色浮层那个 `#ap-ooverlay` 的外层 `div[data-tint-lock]`） | 不可调 |
        | **锁** | 三对「饱和度 / 明度」滑块（`#ap-base-axes` / `#ap-btn-line-axes` / `#ap-btn-bg-axes`） | 不可调 |
        | **不锁** | 底色透明度 `surfaceAlpha` | **可调，且值仍然生效** |
        | **不锁** | 按钮线条透明度 `btnLineAlpha` | **可调，且值仍然生效** |
        | **不锁** | 按钮背景透明度 `btnBgAlpha` | **可调，且值仍然生效** |
        | **不锁** | 其他颜色浮层透明度 `overlayAlpha` | **可调，且值仍然生效**（2026-09-21 加，与上面三个 α 同一口径） |
        | **不锁** | 文字明度 `textLight` | **可调，且值仍然生效**（`#ap-text-controls` 这个容器已删，别再给它加 `data-tint-lock`） |
        - **锚点统一是 `data-tint-lock` 属性**（`buildAppearancePane` 里的 `tl()` 输出），`syncTintLock()` 一句 `querySelectorAll("[data-tint-lock]")` 覆盖全部 —— **加新项只要标一下属性，不会漏**。初始 `locked` class 写死在 HTML 里（免得 attach 之前闪一帧「可编辑」）。
        - **四个例外必须「留在锁外」且「无条件写」**：这是这块最容易做错的地方 —— 上一版的实现是 `setVar("--btn-line-alpha", buttonsOn ? … : "")`，开关一开就把 α 清掉、回落到 `:root`。现在两个 α **任何状态下都写**（默认 0.32 / 0.14 == `:root` ⇒ 默认态零变化）；`--surface-alpha` 本来就在 ② 步无条件写；`textLight` 在 ⑤ 里**不**被 `tintBase` 归零。
        - **锁定用 `.locked`（遮点击 + 降透明度）而不是 `disabled`**：自绘取色器不认 `disabled`（规则 46 已登记过这条教训）。
      - **开关开着时「四个例外可调」与「颜色不可调」并不矛盾**：三个 α 是玻璃质感 / 叠加强度、`textLight` 是明度偏移，都与**色相**无关；开着时颜色来自默认主题套餐，用户仍可决定这套颜色**多透、文字多亮**。
      - **为什么把默认值改成 false（上一轮定案，仍然有效）**：默认开着时那几个组一进去就是灰的、点不动，而解锁开关在**另一组**里 ⇒ 从「按钮自定义」进去的用户既看不出原因、也没有就地解锁的入口，实测被报成「按钮自定义颜色无法生效」。改成默认关着之后，进来就是可编辑的。**本轮把开关提到整块顶部之后这个坑也顺带消除了**（任何一组进去都能一眼看到它）。
      - **配套的一次性迁移不能省**：磁盘上存着的 `tintBase: true` 是**上一版默认值自己写下去的**，留着它新默认值对老配置完全失效。`loadAppearance()` 里用独立键 `lunac-appearance-migrated` 做**只清一次**（放配置对象内部不行 —— 每次 `persistAppearance()` 整份重写会把它冲掉）；清掉后用户以后真的打开这个开关，重启不会被再次清掉。
      - **开关与取色器必须看得见地联动**：切开关只重画 `.locked` + 那行 note，**不**动 `baseColor` / `btnLineColor` / `btnBgColor` / `textLight` —— 用户的值留着，开关再关回去立刻恢复，不需要重选、也不需要备份。
      - **失效范围只剩一条边界**：**`--ctx-*` 反差四件套在开关开着时清空** ⇒ 回落 `:root`，等于跟随主题。（早先版本把 `--surface-alpha` / `textLight` 也算进失效范围，**两条均已作废**：前者升级成「锁外且生效」，后者改成「不锁也不失效」。）
    - **底色的「饱和度 / 明度」滑块与色板是**同一组值**（2026-09-20 用户改定，推翻了早先的「微调偏移」）**：滑块写的就是色板的两轴（HSV 的 S / V），拖色板滑块跟着动、拖滑块色板跟着动。**「微调偏移」这套语义已彻底删除**（`baseSatOffset` / `baseLightOffset` / `btnLineSatOffset` / `btnLineLightOffset` 四个字段与 `resolveTunedColor()` 一并删掉；老配置里的这四个键变成未知键、读进来被忽略）。
      - **实现**：`mountColorPicker()` 返回 `ColorPickerHandle`（只有 `apply(hex)`，**只重画不 commit**）。面板侧把三组颜色放进一张 `colors` 表（`base` / `btnline` / `btnbg`），每条路径都写同一份 `hex`：色板 commit → `syncAxes()` 回写滑块的 value 与读数；滑块 input → `hsvToHex()` 算出新色 → `handles[slot].apply()` 回写色板。
      - **`apply()` 不 commit 是防回环的关键**（否则滑块 ↔ 色板互相触发）。这也是 2026-09-19 删掉「色轮 + 饱和度/明度滑块」三件套的理由 —— 现在之所以又能共存，是因为它们已经是同一组值，而不是两份各自解释的量。
      - **色相从当前色现取**（`hexToHsv(c.hex).h`），只替换被拖动的那一轴。全灰（s = 0）时色相为 0，拖饱和度会往红走 —— 与色板左边缘的行为一致，已知且可接受。
      - 滑块用 `data-axis` + `data-color` 两个属性（**不是** `data-ap`）：它的值不直接落配置，而是经所属取色器换算成 hex 再落 `<slot>Color`。加新取色器就用 `colorAxisRow()`。
    - **「反差四件套」（2026-09-20 新增，`--ctx-*`）**：开关**关掉且设过底色**时，下面四处要「底色暗 ⇒ 比底色亮、底色亮 ⇒ 比底色暗」，**其余地方仍参考当前主题**：
      | # | 位置 | 选择器 | 用到的 token |
      |---|---|---|---|
      | ① | 风格里的分类区域 | `.settings-group-title`（`settings.ts`） | `--ctx-rgb` |
      | ② | 文本框 | `.settings-input` / `.custom-select-trigger` / `.ap-hex`（`settings.ts`）、`#chat-input`（`styles.css`） | `--ctx-shade-rgb` + `--ctx-shade-scale` / `--ctx-ink-rgb` / `--ctx-border-glass` |
      | ③ | 取色器的框格 | `.ap-swatch-btn` / `.ap-pick-panel` / `.ap-sv` / `.ap-hue` / `.ap-preset` / `.ap-picker-toggle` | `--ctx-border-glass`（面板底走 `--ctx-ink-rgb`） |
      | ④ | 结果区当前选中项 | `.result-item:hover, .result-item.selected` | `--ctx-rgb` |
      - **取值方式：把「底色」当成主色，跑一遍 `derivePalette()`**（`contrastFor()`），取它的 ink（白 / 黑）+ shade + border。**不要另写一套阈值** —— 两套的「亮暗分界线」一旦不同，就会出现「底色偏亮时文字是白、反差却是黑」这种自相矛盾的结果。
      - **`--ctx-shade-scale` 固定 0.35**，不取 `derivePalette` 给的 1：白洗叠在暗底上比黑洗叠在暗底上抢眼得多，用 1 会过冲（25% 白 ≈ 一块灰斑）。
      - **`--ctx-shade-rgb` 是必须的第五个 token**：凹陷层的原式是 `rgba(0, 0, 0, α)`，而**黑洗没法反向变成白洗**，所以基色本身要可换（默认 `0, 0, 0` ⇒ 开着时等于原式）。
      - **`--ctx-*` 的默认值就是「跟随主色」的那几项**（`styles.css` 的 `:root` 里写成 `var(--accent-rgb)` / `var(--ink-rgb)` / `var(--shade-scale)` / `var(--border-glass)`，加 `--ctx-shade-rgb: 0, 0, 0`）。**不需要自定义底色时，`main.ts` 把这五个写成空串清掉内联值即可** —— 逐像素回到改造前，不必逐个记值。**回归口径**：默认配置下 `<html>` 的 `style` 上不应出现任何 `--ctx-*`。
      - **饱和度-明度框的渐变本身（`linear-gradient` 的黑 / 白与色相彩虹）一个字不许动** —— 那是取色器的真实颜色，不是主题洗色，见规则 48。
    - **「按钮自定义」（2026-09-20 新增，按钮线条 + 按钮背景）**：两组变量 —— 线条 `--btn-line-*`、背景 `--btn-bg-*`。
      - **作用对象是同一批按钮 + 所有切换开关**：**新建对话 / 更多设置 / 历史记录 / 发送 / 停止 / 添加文件**六个（`#chat-new-btn` / `#chat-more-btn` / `#chat-history-btn` / `#chat-send-btn` / `#chat-stop-btn` / `#chat-add-file-btn`）+ **项目内所有切换开关**（`.settings-toggle-slider` 的轨道底 + 开启态的描边）。**2026-09-21 起「背景」不再是这一批**：它扩到了所有按钮，见下面「按钮背景」那条；这条范围现在只描述**线条**。
        - 线条 → 这六个按钮的 `border` + **这六个按钮的图标（内联 SVG 的 `currentColor`）** + 开关的轨道 / 描边 + **所有滑块（拉条）的 `accent-color`**；背景 → **所有按钮的 `background`**（2026-09-21 由「这六个」扩到全体，见下一条）。
          - **图标跟线条，且用全不透明的 `rgb(var(--btn-line-rgb))`**（2026-09-20 用户要求「历史记录等等的 icon 颜色应该跟随按钮线条」）：图标只有形状没有 α 语义，带 α 会被洗淡；而默认下 `rgb(var(--btn-line-rgb))` == `var(--accent)` ⇒ **默认逐像素不变**。详见下面「图标分两档」那一条。
          - **滑块也在「线条」的范围内（2026-09-20 用户要求「按钮也应该包括拉条的颜色」）**：`.ap-slider input[type="range"] { accent-color: rgb(var(--btn-line-rgb)) }`。**必须是全不透明的 `rgb(...)`**：① `accent-color` 带 α 会把轨道与圆点洗淡；② 默认下 `rgb(var(--btn-line-rgb))` == `rgb(var(--accent-rgb))` == 改造前的 `var(--accent)`，**逐像素不变**（写成 `rgba(..., var(--btn-line-alpha))` 就不成立）。
          - **全项目只有这一处滑块样式**（`appearanceSliderRow()` 与 `colorAxisRow()` 都输出 `.ap-slider`）—— 所以「拉条跟按钮线条」改这一条规则就够了，`styles.css` 里没有第二处 `input[type=range]`。新增滑块时**沿用 `.ap-slider`**，别另写 `accent-color`。
        - `#chat-stop-btn` 必须跟：它占的就是发送按钮那一格，不跟会出现「一开始生成就跳色」。`#chat-add-file-btn` 2026-09-20 补齐（它的边框原先是 `--accent-border`）—— **默认值下两者逐像素相同**（`--btn-line-rgb`/`α` 的默认就是 accent/0.32）。
      - **按钮背景不再跟底色**（用户要求「按钮背景底色应该也可以自定义调节而不是跟随上方的底色」）：它读 `--btn-bg-rgb` / `--btn-bg-alpha`，**与 `--surface-rgb`（底色）没有任何关系**。默认值是原 `--accent-bg` 的配方（accent + 0.14）⇒ 不动控件时逐像素不变。
        - **2026-09-21：背景的作用对象扩到「所有按钮」**（用户报「按钮背景应该包含所有按钮 是否未大批量应用」—— 上面那条 2026-09-20 的边界只覆盖输入栏六个，其余按钮各写各的、多数纯图标按钮压根是 `background: none`，所以拖「按钮背景」时它们一动不动）。实现是 `styles.css` 里一条全局规则 `#app button:where(...)`，**线条仍然只管那六个按钮 + 开关**（用户这次只点名背景）。
          - **可用的机制只有「特异性盖过」**：`#app button`(1,0,1) 恰好高过各按钮自己的类选择器(0,1,0) ⇒ 能盖住它们的 `background: none`；`#app button:where(...)` 的 `:where()` 贡献 0 特异性 ⇒ 盖不掉带 id 的状态规则（`#chat-more-btn.open` (1,1,0) 照旧生效）。**排除项必须写在 `:where()` 里面** —— 写到外面会把特异性抬到 (1,1,1) 连 id 状态规则一起盖掉。
          - ⚠ **那条 hover 规则是例外，它盖得掉 id 状态规则**（2026-09-21 浏览器实测）：`#app button:where(...):not(:disabled):hover` 的特异性是 **(1,2,1)** 而不是 (1,0,1) —— `:not()` 本身贡献 0，但**它参数里的 `:disabled` 是伪类、照样记一档**，再加 `:hover`。后果：`#settings-btn:hover` / `#plugin-bar-exit:hover` / `#detached-*:hover` (1,1,0) 这三条 **hover 底色一直没生效**（实测悬停齿轮得到 `rgba(192,160,160,0.2)`，即全局规则的 `--btn-bg-alpha + 0.06`，而不是它们自己写的 `--ink-rgb 0.05` / `--accent-bg`）。**修法是「点名放行」**：把这五个 id 加进 hover 那条的 `:where()`（`:where()` 内的 `:not(#id)` 仍贡献 0 特异性），它们的 id 规则就重新生效。**新增「想把某按钮的 hover 底色握在自己手里」时，必须同时加进这份放行名单**（这是「按钮底色全覆盖」那张名单下唯一需要双写的地方）。
          - **两类排除，各有硬理由**：① **语义色按钮**（`.tool-refusal-btn` / `.approval-btn` / `.memo-save-btn` / `.file-chip-more-clear` / `.file-chip-remove` / `.result-item-remove` / `.run-mode-confirm-ok` / `.ocr-copy-btn` / `.settings-save-btn`）—— 红 / 绿 / 黄表达状态或危险，按规则 45/48 与「语义状态色固定」那条**不得随主题漂移**；② **状态类 `.active` / `[data-armed="1"]`** —— 它们自带底色（accent-bg / 待确认红）而特异性只有 (0,2,0)，不排除就会被盖掉，**症状是段选看着像没选中、两段式确认看着像没进入待确认**。
          - **已知代价（2026-09-21 用户明确选此方案）**：纯类选择器的 hover 底色（`.chat-more-actions button:hover` / `.memo-item-actions button:hover` / `.history-item-delete:hover` …）会被全局 hover 规则统一成同一套主题底色。**hover 时的文字色 / 边框色变化照旧保留**（那是别的属性），所以「危险 / 成功」的可辨识度没丢，丢的只是底色。另配一条 `#app button:where(...):not(:disabled):hover`（抬 α 0.06，与那六个按钮同配方）—— 没有它的话，**没有自己 hover 规则的按钮一悬停底色就整个消失**。
          - **回归口径**：① 拖「按钮背景」颜色 / 透明度 ⇒ `#plugin-bar-exit` / `#settings-btn` / `#chat-history-btn` / `.ap-picker-toggle` / `.tool-action` 等**非输入栏**按钮的计算背景必须跟着变（`getComputedStyle().backgroundColor`）；② `.chat-more-seg button.active` 与 `.settings-skill-del-installed[data-armed="1"]` 的底色**必须仍是 accent-bg / 红**（没被全局规则盖掉）；③ `.approval-deny` / `.run-mode-confirm-ok` 仍是红、`.file-chip-remove` 仍是红。
        - hover / 打开态按**同一基色抬 α**：`.hover = calc(var(--btn-bg-alpha) + 0.06)`、`#chat-more-btn.open = calc(... + 0.14)`。默认 0.14 ⇒ 0.20 / 0.28，正是改造前的硬编码值。
      - **按钮的图标仍跟随主题色** → **已作废（2026-09-20）**：见上面「线条」里那条 —— 六个按钮的图标现在跟按钮线条。**唯一例外是语义状态覆盖**：`#chat-more-btn.run-mode-auto { color: var(--red) }`（自动档警示）优先级更高，它**必须保持 `--red`**，别被「图标跟线条」这条规则顺手改掉。
      - **开关的圆点也不跟**：它是「填充」不是「线条」。
      - **轨道底的 alpha 取线条 alpha 的固定比例**（关 0.31 / 开 0.44）—— 这样拖「线条透明度」时三层同步缩放，不会只剩描边动。默认 0.32 ⇒ 0.0992 / 0.1408，与改造前的 0.1 / 0.14 肉眼无差。**唯一已知的像素级偏差**：关闭态轨道底的**色相**从「中性白黑」变成「按钮线条色」（明度不变）。
      - **两个颜色字段空串 = 跟随主题色**（默认）⇒ 清掉内联值、回落 `:root` 的 `--btn-line-rgb: var(--accent-rgb)` / `--btn-bg-rgb: var(--accent-rgb)`。**两个 α 任何状态下都写**（默认 0.32 / 0.14 == 原 `--accent-border` / `--accent-bg` 的 α）—— **「恢复默认主题」开关开着时也照写**（用户要求那三项透明度仍可调，见上面「失效范围」表）。取色器在「跟随态」显示主题色 —— 由桥上的 `resolvedSwatches()` 给，面板不复制派生逻辑。
      - **`resolvedSwatches()` 现在返回四项**（`base` / `btnLine` / `btnBg` / `overlay`）：底色取「主题包 surface 优先、否则派生」，另外三项都是当前主题色。
      - **还有 20 余处 `var(--accent-border)` 有意没收进来**（输入框 focus、卡片左边线、`.detail-chip.active`、各类文字按钮…）：用户点名的是「按钮的线条」，那些不是按钮。**这是边界不是漏做**；要扩大范围先问。
    - **「文字自定义」（2026-09-20 新增）—— 只调三档文字的明度**：一个 ±100 的滑块（`Appearance.textLight`，`data-ap="textLight"`），**偏移量**语义（0 = 主题派生原值 ⇒ 逐像素不变），叠在 `derivePalette()` 产出的 `--text` / `--text-dim` / `--text-muted` 上，三档**一起挪**。
      - **实现在 ⑤ 的循环里**，用 `shiftHslLightness(v, delta)`：只认 `derivePalette()` 那种 `hsl(0, 0%, N%)` 形式（文字一律纯灰阶，见上面），不匹配就原样返回。**不许改 `derivePalette()` 本身**（它还被 `contrastFor()` / `resolvedSwatches()` 复用，加了偏移会让「反差四件套」的亮暗判定跟着漂）。
      - **`delta !== 0` 才走替换**，等于 0 时逐字节写原值。
      - **它不受「恢复默认主题」管辖**（用户 2026-09-20 明确「文字明度不锁定」：它是明度偏移、不是配色本身 ⇒ ⑤ 里 `textDelta` 照写），但**会被主题包锁定**（同规则 46）。
    - **「其他颜色」（2026-09-21 新增 → 同日定稿为一组统一控制）—— 一处颜色管四处浮层**：一对字段 `Appearance.overlayColor` / `overlayAlpha`（`#ap-ooverlay` 取色器 + `overlayAlpha` 滑块，都在 `#ap-other-sliders` 面板里，「自定义」按钮 `#ap-other-custom`）。
      - **沿革（初版六项 → 用户要求合并，2026-09-21 同日内两次改定）**：初版把这六处拆成六个独立「取色器 + 透明度」——结果项浮层 / 设置选中项 / **运行端输出文字** / **AI 对话背景** / **设置面板背景** / 分类标题背景。用户随后判定「分得太细」，原话：「**对话背景取色透明度和面板背景取色和透明度都需要删除分类**」+「**其他颜色的其他调色不应该分出来几类而是由一个调色统一控制**」+「**控制台输出文本的颜色退出其他颜色的控制**」，于是：
        - **整项删除三项**（连字段、连 i18n key、连 CSS 变量一起删）：`runtimeTextColor`（⇒ `.sys-note-body` 退回 `var(--text-dim)`）、`chatBgColor/chatBgAlpha`（⇒ 删掉 `#results-container.ai-chat::before` 那条规则，回归 `--surface-glass`）、`panelBgColor/panelBgAlpha`（⇒ 删掉 `.plugin-result > .settings-layout` 那条规则，设置面板本来就没有自己的底色）。
        - **余下三处 + 顶栏图标按钮的 hover 浮层合成一组**，共用一个取色器 + 一条透明度。
      - **驱动的四处（`--other-overlay-rgb` / `--other-overlay-alpha`）**：① 结果项浮层 `.result-item:hover, .result-item.selected`；② 设置侧栏选中项 `.settings-sidebar-item.active`（底色 + 左侧竖条）；③ 设置分类标题 `.settings-group-title`；④ **顶栏图标按钮的 hover 浮层** —— `#settings-btn`（进入设置）、`#plugin-bar-exit`（退出设置 / 退出插件，也就是 Esc 那一下的可视入口）、`#detached-back-btn` / `#detached-vscode-btn` / `#detached-close-btn`（大窗口顶栏同族）。用户原话：「**鼠标移至上方的浮层，相同的还有进入设置的按钮、退出插件的按钮**」。
        - **④ 只统一下它的 hover 底色**：图标色仍是各按钮的语义色（退出 `--red` / 设置 `--accent` / 大窗口关闭 `--red`），`opacity` 变化照旧。
        - **「退出设置按钮」原本就存在**（用户 2026-09-21 确认）—— 就是 Esc 控制的那一下，可视入口是 `#plugin-bar-exit`；本轮**不新增按钮**，只把它的 hover 浮层并进这一组。
      - **一条 α 还原三种历史 α**：四处消费位改造前各自不同 —— 结果项 `0.1`、选中项 / 分类标题 `0.14`、顶栏按钮 hover `0.2`（**不是源码里那个 `0.05`**：`#settings-btn:hover` 那几条规则一直被全局 hover 规则压住，见下一条），合并成一条 α 后由 `styles.css` 的 `:root` 两条派生变量按比例还原：`--other-overlay-nav-alpha: calc(var(--other-overlay-alpha) * 1.4)`、`--other-overlay-btn-alpha: calc(var(--other-overlay-alpha) * 2)`。**统一 α 默认 0.1 ⇒ 四处分别回到 0.1 / 0.14 / 0.14 / 0.2，逐像素不变**。改这两条比例等于改「四处之间的相对浓淡」，不是随手可调的旋钮。
      - **约定与 `--btn-*` 完全一致**：颜色字段**空串 = 跟随**（`setVar(name, "")` ⇒ `removeProperty` ⇒ 回落 `:root` 的 `--other-overlay-rgb: var(--accent-rgb)`），α 字段**恒写**；颜色受「恢复默认主题」管辖（`data-tint-lock`），α 滑块留在锁外（同上面「失效范围」表）。
      - **范围只到「浮层」**：文字颜色（如运行端输出、按钮标签）**不在**这一组里 —— 用户明确要求它退出，文字一律回落主题派生的 `--text*`。
    - **「更多设置」`#chat-more-btn` 与同排按钮完全同源（2026-09-21 定稿，用户报「好像不受按钮颜色控制」）**：底色 `rgba(var(--btn-bg-rgb), var(--btn-bg-alpha))`、描边 `rgba(var(--btn-line-rgb), var(--btn-line-alpha))`、图标 `rgb(var(--btn-line-rgb))`，与 `#chat-add-file-btn` / `#chat-send-btn` / `#chat-history-btn` 逐条同源；hover `calc(var(--btn-bg-alpha) + 0.06)`、`.open` `calc(... + 0.14)`，形状 28×28 / 圆角 6px，与「添加文件」完全一致。
      - **排查纪律（本轮踩到的教训）**：报「不受颜色控制」时，先对齐**三处形状**（尺寸 / 圆角 / 边框）再查颜色 —— 颜色同源时，观感差异一定来自形状或**状态规则**（`.open` / `.run-mode-auto`）。历史上这处 `border-radius` 曾是 8px（同排只有它是 8px）。
      - **唯一的非 token 例外保持不动**：`#chat-more-btn.run-mode-auto { color: var(--red); border-color: rgba(192, 138, 138, 0.35) }` —— 自动档警示是**语义状态色**（规则 45/48），必须固定。
    - **CSS 变量契约**（`styles.css` 侧每一个都带默认值，即「不做任何覆盖时渲染不变」）：`--bg-opacity` / `--bg-blur` / `--bg-saturate`（背景图层 `#app-bg-image`）；`--glass-sheen-alpha`（反光，`0`）经 `--glass-sheen-image`（`linear-gradient(160deg, rgba(255,255,255,α) 0%, transparent 46%)`）落到**三个主面板的 `background-image`** 上 —— **刻意不用伪元素**：`#results-container` 的 `::before` 已是玻璃底色层，再叠 `::after` 会落在内容之上（伪元素是「最后子元素」），而 background-image 天然在背景色之上、内容之下；`--surface-alpha` 经 `--surface-rgb` / `--surface-rgb-hover` 合成 `--surface-glass*`（hover 用 `calc(var(--surface-alpha) - 0.08)` 保持「比常态再透一点」）；`--radius-search`（搜索栏形状）与 `--radius-results`（结果区 + 状态栏**底部**形状）**各自独立** —— 这就是「搜索栏与结果区不同图形」的实现方式，`#results-container` 自身仍是直角（它紧贴状态栏，给圆角会在接缝露出透明像素）；`--search-pattern-image` / `--pattern-opacity`（搜索栏花纹，走 `#search-bar::before`，默认 `none`；`border-radius: inherit` 跟随形状被裁剪）；`--accent-rgb` / `--accent` / `--accent-bg` / `--accent-border`（主题色：**`--accent-rgb` 是唯一真相源**的三元组 `r, g, b`，另外三项由它在 `:root` 派生 —— `--accent` 是 hex，半透明档 `--accent-bg`(0.14) / `--accent-border`(0.32)）；`--ink-rgb` / `--shade-scale`（中性叠加基色 / 凹陷压暗缩放，2026-09-19 批 6 收口，见规则 48）；**`--ctx-rgb` / `--ctx-ink-rgb` / `--ctx-shade-rgb` / `--ctx-shade-scale` / `--ctx-border-glass`** 与 **`--btn-line-rgb` / `--btn-line-alpha`** / **`--btn-bg-rgb` / `--btn-bg-alpha`**（2026-09-20 新增；默认值即「跟随主色」⇒ 不写内联值就渲染不变，见上面「反差四件套」与「按钮自定义」两条）；**`--other-overlay-rgb` / `--other-overlay-alpha`** 与两条派生比例变量 **`--other-overlay-nav-alpha`** / **`--other-overlay-btn-alpha`**（2026-09-21，「其他颜色」的统一浮层，见上面那条；默认 `var(--accent-rgb)` / 0.1 ⇒ 不写内联值时四处浮层逐像素回到改造前）。
    - **按钮配色纪律：动作按钮跟随主题色、语义状态色固定、选中态文字不带色相（2026-09-19 新增，批 4 任务 1/2）**：
      - **只有 `--accent-rgb` 一个真相源**：样式表里凡「accent 带别的 alpha」一律写 `rgba(var(--accent-rgb), x)`（`x` 视场景取 0.06~0.6）。**禁止**再出现 `rgba(192, 160, 160, x)` 这类硬编码 rgb 分量 —— 用户报的「设置 → 搜索分类的 save 按钮没跟随主题颜色」正是由此而来（`styles.css` 曾散落 20 余处、`settings.ts`/`tool-editor.ts` 各数处）。同理，`applyAccent()` **只写 `--accent` 与 `--accent-rgb` 两行**，半透明档交给 `:root` 派生，不许各自 `setVar`。
      - **动作按钮 → `--accent` 系（底色 / 边框 / 图标，不含文字）**：`background: rgba(var(--accent-rgb), 0.1~0.18)`、`border: 1px solid var(--accent-border)`；`color: var(--accent)` **只留给「按钮内是纯图标」的情况**（内联 SVG 走 `currentColor`）与 `caret-color`，**按钮上的文字标签一律 `color: var(--text)`**。hover 抬到 0.2~0.25、active 到 0.35。已按此改造：`.settings-save-btn` / `.settings-install-btn` / `.settings-hotkey` / `.settings-tool-btn` / `.settings-skill-open` / `.custom-model-ok` / `#tool-editor-save` / `#tool-new-btn`（文字全部转灰阶，底色边框仍跟随主题色）。
      - **语义状态色保持固定，不跟随主题**：`--green`(成功/有效/放行) / `--red`(危险/失败/拒绝) / `--yellow`(警告/录制中/待确认) / `--blue`(信息)。判据是「它表达的是**状态**还是**可点的动作**」—— 例如 `.settings-save-msg`（「已保存」提示）保留 `var(--green)`，而旁边的 `.settings-save-btn` 改走 accent；`.approval-allow`/`.approval-deny`、`.tool-badge.valid/invalid`、`.agent-status.*`、`.file-chip`、`.memo-item-actions .memo-copy` 同理保持。**不要把这两个家族混为一谈**（把语义色也塞进 accent 会让「危险/成功」失去可辨识度）。
        - **2026-09-20 补：`styles.css` 里 28 处硬编码 `rgba(157, 180, 172, x)`（`--green`）按同一条判据清了一遍**。判据仍是「这个绿表达的是**可点的动作**还是**状态**」：
          - **改成 `rgba(var(--accent-rgb), x)`（α 原样保留）**：`.file-chip` / `.file-chip:hover` / `.file-chip-more-btn` / `.file-chip-more-btn:hover` / `.file-chip-list` 的虚线 / `#search-bar.drag-over` + `#chat-input-bar.drag-over` / `#chat-add-file-btn` / `.ql-add-btn`（文字同时由 `--green` 改为 `--text`）。
          - **一个都没动**：`.tool-row.auto-approved` / `.tool-row.tool-ok` / `.tool-refusal-btn.tool-refusal-allow` / `.approval-allow` / `.approval-batch-card .approval-batch-allow-all` / `.agent-status.ready` / `.memo-item-actions .memo-copy:hover` / `.memo-item-actions .memo-save-edit` —— 它们旁边都还写着 `color: var(--green)`，那是「成功 / 放行 / 有效」。
          - **唯一的例外：`.result-item-badge.app-badge`**（`quick-launch.ts` 的「常驻 / 本次」标记）**保持绿**。它的绿不是语义状态也不是动作，而是「与别的 badge 区隔」的分类色（注释原文 *distinct color to separate from plugin badges*）—— 改成 accent 就与 `.result-item-badge` 的底色 `--accent-bg` 重合，区分作用直接消失。**不要以「消除硬编码」为名把它一起改掉。**
      - **「文字只跟明暗、不带色相」的边界（2026-09-19 批 6 定稿，用户答复「文字全灰阶，图标留 accent」）**：判据只有一条 —— **看这个控件渲染出来的是文字还是图标**。
        - **文字**（含按钮标签、侧栏 / 下拉选中项、聊天区用户气泡正文、标签胶囊、状态词、占位与提示文字）一律走 `--text` / `--text-dim` / `--text-muted`（它们已由 `derivePalette()` 做成纯灰阶）；**严禁** `color: var(--accent)`。
        - **图标**（内联 SVG 走 `currentColor`）里分两档：
          - **六个聊天动作按钮的图标跟「按钮线条」**（2026-09-20 用户要求「类似于 ai 对话里面 历史记录等等的 icon 颜色应该跟随按钮线条」）：`color: rgb(var(--btn-line-rgb))` —— 发送 / 停止 / 历史记录 / 新建对话 / 更多设置 / 添加文件。**必须是全不透明的 `rgb(...)`**：默认下它 == `rgb(var(--accent-rgb))` == 改造前的 `var(--accent)`，**逐像素不变**（写成 `rgba(..., var(--btn-line-alpha))` 会把图标洗淡、也不满足默认不变）。
          - **其余图标（如 `#settings-btn` 的齿轮、`#humanize-btn:hover` 的「去 AI 痕迹」、`.result-item-elevate:hover` 的盾牌）与「输出光标」`.ai-response .cursor-blink::after` 仍保留 `var(--accent)`** —— 它们不在「按钮自定义」的对象里。
        - **`caret-color`**（输入光标）**一律保留 `var(--accent)`**（4 处）：它是文字输入点位，不属于任何按钮。
        - **混合控件（图标 + 文字标签，如「复制」「编辑」「保存」按钮）按文字算** ⇒ `var(--text)`。图标跟着变灰可以接受，文字上色不可接受。
        - **批量改法**：`styles.css` 全表 + `settings.ts` / `tool-editor.ts` 各一条，**逐块判定**（按 `选择器 { 声明 }` 切块，而不是整文件字符串替换）—— 用整文替换必然会误伤 `caret-color`（`caret-color: var(--accent)` 里含子串 `color: var(--accent)`）与紧跟在注释后的选择器。
        - **本轮清完的清单（回归时按此对账）**：`styles.css` 的 `.context-menu-item:hover` / `.context-menu-item-active` / `.clip-item-copy` / `.ocr-primary-btn` / `.ocr-copy-btn.ocr-copied` / `.result-item-badge` / `.tool-card.running .tool-state` / `.chat-msg-user` / `.msg-actions button:hover` / `.flow-copy:hover` / `.chat-more-seg button.active` / `.memo-tag-chip` / `.memo-tag-ok` / `.memo-item-actions .memo-edit:hover` / `.detail-preview-actions button`；`settings.ts` 的 `.settings-sidebar-item.active` / `.settings-hotkey` / `.settings-tool-btn` / `.settings-install-btn` / `.settings-save-btn` / `.settings-skill-open` / `.custom-select-option.selected`；`tool-editor.ts` 的 `#tool-editor-save` / `#tool-new-btn`。
        - **保留 `--accent` 的白名单（只有这些，别再多）**：`#settings-btn:hover`、`#humanize-btn:hover`、`.result-item-elevate:hover`（盾牌 SVG，2026-09-19 批 8 前是 emoji 🛡）、`.ai-response .cursor-blink::after`（输出光标）、`.ap-picker-toggle:hover`（▾ 箭头），以及全部 `caret-color`。选中与否由**底色 / 边框**表达即可。
          - **2026-09-20 从这份白名单里移出的**：`#chat-send-btn` / `#chat-stop-btn` / `#chat-new-btn` / `#chat-more-btn` / `#chat-history-btn` / `#chat-add-file-btn` —— 它们的图标改跟**按钮线条**（见上面「图标」那一条）。回归时别把这六个又「修」回 `var(--accent)`。
    - **背景区 = 三按钮 + 一组可折叠拉条（2026-09-19 二次定稿）**：`选择图片`（唯一背景来源，优先级：主题自带背景 > 用户图片）/ `自定义`（**展开或收起**那**四个**拉条：毛玻璃化 / 饱和度 / 背景透明度 / 反光 —— 收起状态跨「关设置再打开」保留在 `bgSlidersOpen`）/ `清除`。**不要再把「自定义」理解成「另一种背景来源」**（曾按纯色背景实现，被用户纠正）。
      - **原第五个「界面玻璃透明度」已于 2026-09-20 迁进「主题颜色 → 底色自定义」并改名「底色透明度」**：它就是 `surfaceAlpha`（玻璃底色的 alpha），属于**配色**而不是背景图 —— 与「恢复默认主题」那个开关管的是同一批东西，放在背景区是错的分组。字段名不变（`surfaceAlpha`），只是控件换了位置；且它是**「恢复默认主题」开启后仍可调的三个透明度之一**。
      - **三种「自定义」= 三个独立的展开状态**（`bgSlidersOpen` / `basePanelOpen` / `btnPanelOpen` / `textPanelOpen`），形态统一为「按钮 + ▾ → 一块 `.ap-sliders-panel`」，绑定统一走 `bindPanel()`。**新增一组时照抄这套，不要新造形态**（2026-09-20 起共四组：背景 / 底色 / 按钮 / 文字）。
    - **主题图标只有一个出口**：`pluginIconSvg()` 先查 `themeIconUrls`（主题包 `assets.icons.<插件 id>`），命中返回 `<img class="result-item-icon-img">`，否则回退内联 SVG。**禁止在各个渲染点各自判断主题** —— 图标汇聚点只有这一个（`themeIconUrls` 的声明必须放在 `pluginIconSvg` 之前：`const` 在声明前是 TDZ，放文件末尾就是必然的白屏）。
    - **`theme.json` 资产路径必须做穿越防护**（`appearance.rs`：拒绝 `..`、绝对路径、resolve 后逃出主题目录）；单个主题解析失败只 `warn` 并跳过，**不能让一个坏主题打空整个列表**。
    - **内置主题包：现在只有 `themes\default`**：`tauri.conf.json` 的 `resources` 把仓库 `app/src-tauri/themes` 映射到 exe 根，目录会被 `list_themes()` 扫到；**目录里有名为 `builtin` 的标记文件**的会被打上「内置」徽标（`ThemeInfo.builtin`）。**新增内置包时不能只放素材** —— 必须同时把它的 id 加进 `appearance.rs` 的 `shipped_theme_packs_parse_and_their_assets_exist`（那条 `for` 循环只覆盖「已经扫到的包」，包整个漏掉时一次都不跑，等于没有守护）。
      - **`themes\succubus`（「魅魔 · 灰玫瑰」）已于 2026-09-21 整包删除**：用户看过实际效果后判定不满意（原话「删除这个主题吧 我不是很满意」）。删掉的不只是素材目录 —— 一并清了 `appearance.rs` 里那两条断言（`contains("succubus")` 与「主色与 default 同值」）。**素材生成脚本留在 `D:\ui\_build` 不动**（它是通用的「SVG → resvg PNG」主题包生成器，不属于某一个主题）。
      - **配套的「主题被删」闸（`loadThemes()`）**：磁盘上选过的主题包被删掉后，配置里还留着那个 id ⇒ **回落 `"default"` 并落盘**。不回落的话「风格 → 主题颜色」整块会被主题锁灰掉且点不动（锁定判据是 `themeId !== "default"`，见规则 46），而设置里的泡泡框一个都不是选中态 —— 用户看到的是「配色突然改不动了，而且没有任何原因」，无从下手。**删任何内置主题包时都靠这道闸兜住老配置。**
    - **主题列表 = 泡泡框（2026-09-21 定稿，用户要求「用分割出来的泡泡框去识别文件夹，然后点击切换」）**：每个**主题文件夹**渲染成一个 `.ap-theme` 泡泡（胶囊圆角 `border-radius: 999px`，与会分行铺开的 `.settings-group-title` 分割块区分开），内容是 `[主色圆点][名称][内置徽标]`；列表本身就是 `list_themes()` 扫 `<themes 根>\*\theme.json` 的结果 —— **取数来源不许改**（不要改成硬编码清单）。点击 = `ap.set({ themeId })` + `.active` 换位 + `syncThemeLock()`。
      - **主色圆点**：`<span class="ap-theme-dot" style="background:...">`，颜色取 `manifest.tokens.accent`，由 `themeAccentColor()` 归一化 —— **两种写法都认**（`#RRGGBB` 与 `r, g, b`，与 `tokens.surface` 同一宽容度），取不到时回落中性灰而不是让泡泡炸掉。圆点带一圈 `inset` 淡描边（主色接近底色时不至于「消失」）。加它就是为了「不用先点一下才知道那个主题什么颜色」。
      - **`title` 用文件夹名（= 主题 id）**，方便用户对照资源管理器里的 `<themes 根>`。
      - **调试提示**：dev 实例读的是 **`app\src-tauri\target\debug\themes\`**（exe 根），不是仓库源码目录 —— 仓库里新增主题后**不重新构建就看不到**（`bundle.resources` 在构建期才拷贝）。症状是「改了主题但设置里没有」；最快的验证手段是直接把仓库 `themes\*` 拷到 `target\debug\themes\`，或跑一次 `cargo build` / `npm run tauri:dev`。
    - **~~跟随 Windows = 跟随强调色~~ —— 已整体下线（2026-09-20）**：原先的实现读 `HKCU\Software\Microsoft\Windows\DWM\AccentColor`（**实测为 `0xAABBGGRR` 字节序**），刷新时机三处（启动无条件一次 / `lunac-window-shown` / 系统模式下每 15s 轮询）。**三段代码现在全部删除**（含那个 `setInterval`）。下面这条只作为**存档**保留，改回时按它还原：
      - 若要恢复：语义是「强调色只是主题色的一个来源」，`dark` 字段读到了但**从未使用**（本 UI 只有深色）；切到跟随模式时必须**顺带刷一次并重画该行**，否则 `--accent` 停在上一个自定义色（浏览器实测：表现为「切了没反应，得再点一次刷新」）；**不做** `WM_DWMCOLORIZATIONCOLORCHANGED` 消息驱动（要动 `hotkey.rs` 的 WndProc 子类化，收益仅「变色后 15s 内察觉」）。
    - **取色器是应用内自绘的，且是唯一取色入口**（2026-09-19 定稿）：`<input type="color">` 弹的是 Windows 原生对话框（WebView2 里样式改不了一个像素，与暗色玻璃界面脱节），**已移除**；「色轮 + 饱和度/明度滑块」与取色器表达同一组自由度、并存互相打架，**也已移除**。现形态 = 色号输入框 + 色块按钮 + **屏幕取色按钮** → 展开一个面板：色相条 + 饱和度/明度面板 + 预设色板。
      - **同一份工厂现挂 4 个实例（2026-09-21 起）**：`ap-base`（底色）/ `ap-btnline`（按钮线条）/ `ap-btnbg`（按钮背景）/ `ap-ooverlay`（其他颜色 · 统一浮层）。**`ap-accent` 已删除**（主题色取色器下线）。**加新取色器就复用 `colorPickerHtml()` / `mountColorPicker()` / `colorAxisRow()`，不要另写一套。**
      - **「从电脑中取色」（2026-09-20 新增，用户要求）**：`mountColorPicker()` 里绑 `#<id>-pick`，走 Chromium 的 **`EyeDropper` API**（屏幕任意位置吸一个像素，回传 `sRGBHex`）。三条纪律：① **不支持就 `classList.add("hidden")` 整条隐藏**（不留一个点了没反应的按钮）；② 用户按 Esc 取消时 `open()` 会 reject —— **静默**，不是错误；③ 回来的值先 `toLowerCase()` 再用 `/^#[0-9a-f]{6}$/` 校验，通过才 `paint(true, hex)`（与 hex 输入同一条路径，**原样落盘**）。图标是内联 SVG（走 `currentColor`，见上面「图标留 accent」那条），复用 `.ap-picker-toggle` 的方形尺寸。
      - **`colorPickerHtml(id)` 会占掉 `#<id>` / `-hex` / `-swatch` / `-pick` / `-toggle` / `-panel` / `-sv` / `-sv-cursor` / `-hue` / `-hue-cursor` / `-presets`** —— 外层容器的 id **不要叫 `#<id>-panel`**。实测踩过：底色那组的外层拉条面板原本叫 `#ap-base-panel`，与取色器自己的展开面板同名，`querySelector` 取到错的那个，于是「展开取色器」会把整组拉条一起收起来。现名为 `#ap-base-sliders` / `#ap-btn-sliders` / `#ap-text-sliders` / `#ap-other-sliders`。
      - 两条硬纪律：① hex 输入与预设给的**原始值必须原样落盘**，只有拖面板/色相条才允许走 hex→HSV→hex 往返（会丢 1/255 —— 实测把 `#3a7bd5` 存成 `#397ad5` 才加的这条）；② 初始化只重画界面、**不 commit**（否则每打开一次设置就把舍入误差存一次）。
      - **「跟随态」显示什么色由桥给**（`resolvedSwatches()`）：底色 = 主题包 `surface` 或 `derivePalette(主题色)` 派生的表面色、按钮线条 / 按钮背景 / 其他颜色浮层 = 主题色。**面板不许自己复制那份派生逻辑** —— 复制必然与 `applyAppearance()` 漂移。
    - **取色器尺寸与对齐（2026-09-19 二次定稿；`.ap-sv` 高度 2026-09-20 改过）**：所在行用 `.settings-row.ap-row-block`（整宽上下列，label 在上、取色器在下并占满内容宽，实测 355px @ 549 视口 / 约 410px @ 800 窗口）；**`.ap-sv` 高 80px**（2026-09-20 用户要求「色板的高度调高至 80px」，原为 40px —— 现在它与同组的「饱和度 / 明度」滑块是同一组值，高度直接决定竖直方向的拖拽精度；**三个取色器共用这条 class，尺寸天然一致**，改高度只有这一个地方）；展开按钮 **30×30 圆角正方形**（`border-radius: 8px`）；色块 34×26；**色号输入框排在头部最左**（`[hex][色块][取色][▾]`）—— 放色块之后永远差「色块+间距」而无法与左侧 label 左端对齐（实测差 40px）。
      - **教训**：写这批样式时残留了一条 `#ap-accent { width: 46px; height: 24px; ... }`（本意是给已成历史的原生 `<input type=color>` 用），**ID 选择器优先级压过了 `.ap-picker { width: 100% }`**，于是取色器被压成 46px 宽的窄条（实测 `sv` 只有 30px 宽）。改这类「同名元素从原生控件换成自绘组件」的地方，**必须先把旧控件的 ID 规则删干净**。
    - **原配色基线（2026-09-19 存档，改外观前请先对这张表）**：下面这套就是「改造前的外观」，也就是 **「主题色代替底色」关掉、但底色取色器没动过（`baseColor` 空串）+ 选中默认主题** 时应有的取值（`baseColor` 一旦有值，`--surface-rgb` 与「反差四件套」就由它派生，见上面两条）。`themes/default/theme.json` 存的是 schema 能表达的那部分 —— **2026-09-20 起不再含 `surface`**（用户要求「去除默认主题对底色的影响」；那张表里的 `28, 26, 32` 是**原始 `:root` 值**，而 `derivePalette(#c0a0a0)` 实际给出的是 `31, 25, 25`，与改造前真正渲染的值一致，所以删掉 `surface` 才是「零变化」的选择）；`green`/`red`/`yellow`/`blue` 与 `--radius` 主题 schema **未收录**，只在 `:root` 里，改主题系统时别把它们弄丢。

      | token | 原值 | 备注 |
      |---|---|---|
      | `--accent-rgb` / `--accent` / `--accent-bg` / `--accent-border` | `192, 160, 160` / `#c0a0a0` / `rgba(192,160,160,0.14)` / `rgba(192,160,160,0.32)` | 尘玫瑰；`--accent-rgb` 是真相源，后三项由它派生（2026-09-19 批 4 收口） |
      | `--surface-rgb` / `--surface-rgb-hover` | `28, 26, 32` / `48, 44, 54` | 面板玻璃底（冷灰，色相 260°） |
      | `--surface-alpha` | `0.88` | 玻璃不透明度 |
      | `--text` / `--text-dim` / `--text-muted` | `#eae2da` / `#b2a9a3` / `#837a78` | |
      | `--border-glass` | `rgba(232, 216, 202, 0.10)` | |
      | `--green` / `--red` / `--yellow` / `--blue` | `#9db4ac` / `#c08a8a` / `#c9b896` / `#9490b4` | 语义色（schema 未收录） |
      | `--radius` | `14px` | 搜索栏四角 + 状态栏底部两角；`#results-container` 自身仍是 `0` |
      | 背景图 / `--bg-blur` / `--bg-saturate` / `--bg-opacity` | 无 / `4px` / `0.92` / `0.5` | |
      | `--glass-sheen-alpha` | `0` | 反光默认关 |
      | `--search-pattern-image` / `--pattern-opacity` | `none` / `0.18` | |
| `--ink-rgb` / `--shade-scale` | `255, 255, 255` / `1` | 中性叠加基色 / 凹陷压暗缩放（2026-09-19 批 6，见规则 48）；深色主题的默认值 == 改造前 |
    - 设置侧**只调桥**（`window.__lunac_appearance`），不自己写 localStorage、不自己拼 CSS 变量 —— 改造前的背景图就是「设置写 + main 读」两份实现，再加滑块必然漂移。**注意：往 `style.textContent` 的模板字符串里写 CSS 注释时禁止使用反引号**（会提前终止模板字符串，已**三次**踩到：`tsc` 报 `TS1005: ';' expected`）。
46. **主题包锁定「主题颜色」与「背景图片」，玻璃质感拉条除外（2026-09-19 批 5 任务 1）**：`themeId !== "default"` 时，设置 → 风格的**主题颜色整块**与**「选择图片」按钮**必须不可用。
    - **判据是「主题包 = 一整套定好的外观」**：放开配色与背景，用户一改就不像那个主题了；而「自定义」里那**四个**拉条（毛玻璃化 / 饱和度 / 背景透明度 / 反光）是**窗口玻璃质感**、与配色无关，任何主题下都必须**保持可用**。（原第五个「界面玻璃透明度」2026-09-20 已迁进「底色自定义」，因此它**跟着「主题颜色」一起被锁** —— 它本来就是配色。）
    - **禁用而不是隐藏**（用户明确选的）：`#ap-color-group` 加 `.locked`（`opacity: 0.42` + `pointer-events: none`）、`#ap-bg-pick` 加 `disabled`、另起一行 `#ap-lock-note` 用 `--yellow`（语义警告色）说明原因。隐藏会让用户以为功能消失了。
      - **`#ap-color-group` 里现在有 4 个取色器 + 二十余个控件**（底色 / 按钮线条 / 按钮背景 / 其他颜色浮层 + 各自的滑块、开关、与四组「自定义」按钮）—— 这也是「整块锁」而不是逐个加 `disabled` 的第二个理由。**新增的配色控件只要放进 `#ap-color-group` 就自动被主题包锁住**，不要另写一套。**「文字自定义」组也在里面** ⇒ 它同样被主题包锁（用户 2026-09-20 选「保持现状」）。
    - **为什么整块 `pointer-events: none` 而不是给每个控件加 `disabled`**：控件太多逐个加必然漏；且自绘取色器面板不认 `disabled`。
    - **同一个 `.locked` class 有两个用途，别混**：① 主题包锁定（整块 `#ap-color-group`，由 `syncThemeLock()` 实时同步）；② **「恢复默认主题」开着时禁用三组配色控件**（由 `syncTintLock()` 同步，锚点是 `data-tint-lock` 属性 —— 现在有 7 个块：4 个取色器 + 3 对饱和/明度）。判据相同（「这块现在不该被改」），所以刻意复用同一条 CSS，**不要新造 `disabled` 态**。**四组「透明度」滑块（底色 / 按钮线条 / 按钮背景 / 其他颜色浮层）仍在锁外**，见上面「失效范围」表。
      - **②的初始态是「不锁」**：`tintBase` 默认 false（2026-09-20 用户要求），所以**进设置就能直接改底色与按钮颜色**。②只在用户主动打开那个开关后才生效。
      - **②的锁定范围与①不同（2026-09-20 四次定稿）**：②**不锁**底色透明度 / 按钮线条透明度 / 按钮背景透明度三行，也**不锁**文字明度。改 `syncTintLock()` 时注意别把这几项加进 `data-tint-lock` —— **锁了它们就等于让用户调不动**，那是用户明确要保留的能力。
    - 切主题时**必须实时同步**（`syncThemeLock(id)`）—— 只在 `buildAppearancePane()` 里算一次的话，切换主题要重开设置才生效。
47. **搜索结果的呈现与检索容错（2026-09-19 批 5 任务 2/3，对齐 Win11 新版搜索 KB5120998）**：
    - **简洁搜索的结果区不再有类型标签**（用户明确要求，已全量移除）：`.result-item-badge`（快捷方式 / 文件夹 / 扩展名 / 工具 / AI / 网页 / OCR / memo）**一个都不留**，结果行只有「图标 + 标题 + 副行」。行类型由**图标**表达即可 —— 标签既占宽又和图标重复。**例外两处不要跟着删**：① 详细搜索面板的行（那是「来源标签」，见下条）；② `quicklaunch` 插件面板里的 `常驻 / 本次` 标记（它是状态、不是类型）。
    - **详细搜索面板必须有右侧预览区**（`#detail-preview`，宽 **210 设计 px**）：选中一条结果就显示 ① 缩略图 ② 来源 ③ 完整路径 ④ 修改时间 ⑤ 「打开」「复制路径」。布局是 `#detail-main { display:flex }` + `#detail-results { flex:1; min-width:0 }` + 预览定宽 —— **`min-width: 0` 不能漏**，否则长文件名会把列表撑爆、把预览挤出面板。选中行变化时重建（`renderDetailPreview()`），`renderDetail()` 与 `markDetailSelection()` 都要调。
      - 两条防抖纪律（鼠标划过每一行都会触发重建）：`detailPreviewKey` 相同直接早退（不重建 DOM、不重发 IPC）；`detailThumbCache` 按路径缓存缩略图，**「取过但拿不到」也要缓存**（用空串表示），否则来回扫同一批文件会反复请求。
      - 缩略图走 `get_file_thumbnail(path, max)`（Rust `icon_extractor::extract_thumbnail_base64`）：**图片文件真解码**（`image` crate，只开 png/jpeg feature ⇒ 白名单就是 png/jpg/jpeg/jfif，其余**不要假装能缩略**）、**其余一切情况回落系统类型图标**（`SHGetFileInfoW`）。三条硬约束：文件超过 24MB 不解码、`(async)` 不许占主线程、调用方只面对「有图 / 没图」两种结果。
    - **「设置」类结果必须直达对应设置页**，不能停在「找到一条叫这个名字的记录」：`settings` 分类的行点击走 `open_setting(ms-settings:xxx)`（`actions` 走 `run_system_action`）—— 这条已实现，回归时别改回只选中。
    - **检索容错（typo / 子串）**：`file_indexer::rank` 与 `app_indexer::search_apps` 已有「精确 → 前缀 → 包含 → 子序列 → 拼音」五档，**别在别处重造**；前端 `matchDetailCatalog`（系统设置页 + 系统动作的本地匹配）本轮补上了**第二轮子序列兜底**，因为原来只有「包含」，拼错一个字（`instaled`）就再也命中不了「已安装的应用」—— 那正是 Win11 新版搜索最被称道的一条。
      - 两条门控照抄 Rust 侧：**模糊兜底只在第一轮没凑够上限时跑**（单字符查询第一轮就已命中该命中的，不该再扫全表）；**子序列最低 2 字符**（`DETAIL_FUZZY_MIN_CHARS`，对齐 Win11 的「2 字符起」）。
      - 模糊命中的分数必须**压在「包含」档之下**（20 一档 vs 30/60/100）：兜底永远不能把精确命中挤下去。
    - **已知未做（记录在案，不是遗漏）**：详细搜索**空输入**时不显示「最近打开」（用户明确不采纳该项）；此时 `file_indexer::rank` 对空查询给每一条 `score = 0`，于是结果区列出**索引顺序的前 N 条文件**。若哪天真要改成「最近打开」，改这里。

48. **中性叠加 / 凹陷强度两个 token：`--ink-rgb` 与 `--shade-scale`（2026-09-19 批 6 新增）**：样式表里原本散落着 `rgba(255, 255, 255, α)`（44 处）与 `rgba(0, 0, 0, α)`（18 处）—— 它们**与主题无关**，主色一换成浅色的「浅底 + 黑字」，白叠加就等于没画（hover / 选中反馈整片消失）、黑凹陷又脏得刺眼。收口成两个 token，由 `derivePalette()` 随主色明暗写出。
    - **`--ink-rgb`（中性叠加基色）**：hover 底 / 次级面板 / 分隔线 / 淡高亮。深色主题 = `255, 255, 255`（提亮一档）、浅色主题 = `0, 0, 0`（压暗一档）—— **必须翻转**。用法：`rgba(var(--ink-rgb), α)`，α 保持原值不动。
    - **`--shade-scale`（凹陷压暗缩放）**：输入框 / 卡片 / 次级块的「凹进去」底色（原 α 0.12~0.65，含 `.custom-select-trigger` / `#chat-input-bar` / `.tool-card` / `.memo-tag-dialog` 与各级 `box-shadow`）。深色主题 = `1`（**精确等于改造前**）、浅色主题 = `0.35`（亮底上同样的黑脏得多）。用法：`rgba(0, 0, 0, calc(α * var(--shade-scale)))`。
    - **判据**：这个半透明层是**为了「提亮一档 / 压暗一档」**（→ 走 token），还是**在表达一个真实颜色 / 真实光影**（→ 保持硬编码）。属于后者、**绝对不要换算**的：取色器的饱和度-明度面板 `linear-gradient(to top, #000, …)` 与 `linear-gradient(to right, #fff, …)`、`background-color: var(--ap-hue-color)`、色相条 `hsl(...)` 彩虹渐变、`.ap-sv-cursor` 的白边 + 黑描边环、`.tool-btn-del.armed` 的 `#fff`（红底白字是危险态的固定搭配）、`.tool-badge.valid/invalid` 与 `.cs-del` 的语义红绿。另有一处**故意保留白色**：`--glass-sheen-image` 的 `linear-gradient(160deg, rgba(255, 255, 255, var(--glass-sheen-alpha)) 0%, transparent 46%)` —— 那是**玻璃反光（镜面高光）**，物理上就该是白的；换成 `--ink-rgb` 后浅色主题会得到一道黑边，语义从「反光」变成「阴影」。
    - **默认值 == 改造前**（`styles.css` 的 `:root` 里 `--ink-rgb: 255, 255, 255` / `--shade-scale: 1`）⇒ `tintBase` 关掉或主色仍是默认的尘玫瑰时，渲染逐像素不变。
    - **插件底色必须走 token，不许硬编码**（用户点名的是 AI 助手，实际是普适规则）：`#chat-more-menu` 曾硬编码 `rgba(28, 26, 32, 0.94)`、`#chat-drawer` 曾硬编码 `rgba(0, 0, 0, 0.92)`、`settings.ts` 的 `.custom-select-dropdown` 曾硬编码 `rgba(24,24,37,0.97)` —— 三处换主色纹丝不动（用户报的「更多设置菜单 / 历史记录抽屉 / AI模型下拉没跟着 lunac 主题」）。现在一律**`var(--surface-glass)`**。**新写任何面板 / 菜单 / 抽屉 / 下拉的底色，只能用 `--surface-glass` / `--surface-glass-hover` / `rgba(var(--ink-rgb), α)` / `rgba(0, 0, 0, calc(α * var(--shade-scale)))` 这四个出口**，出现 `rgba(28, 26, 32` / `rgba(24,24,37` / `#1c1a20` 这类常量即为回归。
    - **顶层插件面板不得自加压暗（2026-09-19 批 7，用户报「OCR 插件没对齐默认主题」）**：全部插件的顶层容器都是**直接坐在 `#results-container` 的玻璃底上、自己不加任何底色**（`memo` 的 `.plugin-result` 实测 `transparent`）。OCR 是唯一的例外 —— `.ocr-image-panel` / `.ocr-text-panel` 各自叠了一层 `rgba(0, 0, 0, calc(0.12 * var(--shade-scale)))`，于是它的两个半屏比别的插件**暗一档**、且那层黑与主题色相无关。**已改为 `background: transparent`**，左右分栏改由 `.ocr-image-panel` 的 `border-right` 表达。**纪律**：`rgba(0, 0, 0, calc(α * var(--shade-scale)))` 只允许用在**嵌套在面板内部**的次级块（输入框 / 工具卡 / 弹层 / 遮罩），**顶层插件面板一律透明**。
    - **回归自查一句话**：样式表里 `rgba(255, 255, 255, α)` 与 `rgba(0, 0, 0, α)` 的**剩余出现次数应为 0**（`grep -c` 只允许命中规则文本与本条列出的取色器例外）—— 用户报的「设置里常规 / 风格 / ai模型 / 搜索 / 技能拓展 / 插件大类、热键设置、AI 助手四个按钮、各插件底色都漏改」就是靠这一条一次性兜住的。
49. **UI 文案里永远不得内嵌 emoji 当图标（2026-09-19 批 8，用户明确要求）**：按钮标签、状态行 / 提示行、占位文案一律「**纯文字**」或「**§1 的线性 SVG + 文字**」，禁止 `📋 复制结果` / `📁 选择文件` / `🖼️ 暂无图片` / `⚠️ 剪贴板中暂无图片，请复制图片后重试` / `⏳ 识别中` / `✅ 完成` / `❌ 失败` 这类写法。
    - **判据**：这个 emoji 是「一条可点控件 / 一句提示的**文案的一部分**」（→ 禁止），还是「一个**条目 / 一条状态的标记**」（→ 允许）。
    - **允许且必须保留**：① 列表项 / 条目图标（结果区行图标、目录数据自带的 `item.icon`、`plugin.icon` 与未知插件的 🔧 兜底、`.clip-item-icon` 📁/📋、`.file-chip-icon` 📦/📎、todo 面板标题 📋、tool-editor 🔧、web-search 引擎图标 🔍/🌐/🐻）；② 单色状态符号 `✓ ✗ ⚠ ↔ ↩ ◐ ○ ✔ ✕ × ＋`（如 `clipboard.copied` = `"✓ 已复制"`、`clipboard.copy_failed` = `"✗ 失败"`、todo 的 `✔/◐/○`、工具黑名单的 `×/✓`）。带 `U+FE0F` 变体选择符的按 emoji 算（`⚠️` 禁止、`⚠` 允许）。
    - **对话记录条目上的 💬 已删除（2026-09-20，用户明确要求）**：`renderHistoryList()` / 历史抽屉两处的 `<span class="history-item-icon">💬</span>` 与其 CSS 规则一并移除，条目只剩「标题 + 条数 + 删除按钮」。**这是上一条「① 条目图标允许」的一个例外** —— 判据不是「它是不是条目图标」，而是用户要不要它；**不要**拿 ① 把它加回来。`.history-item-icon` 这个类名不得再出现在 HTML 或样式表里。
    - **完整条款与逐项清单在 [icon-style.md](./icon-style.md) §4.1 / §4.2**（那里是图标类规定的唯一真相源，本文件只做指针）。
    - **本轮已清理**：`ocr.ts` 全量（三个按钮 + 图片占位 + 引擎状态 + 全部状态行）、`clipboard-history.ts` 的复制按钮（原 `📎 复制` / `📋 复制`）、`i18n.ts` 的 `clipboard.copy`（原 `📋 复制`，5 语言）。**回归口径**：新增/修改按钮或状态行后，`grep` 该处文案不应命中任何 emoji 码位。

50. **插件名 / 描述必须走 i18n；设置分类结构（2026-09-19 批 9）**：
    - **插件显示名与描述一律从 i18n 取**：`pluginName(id, fallback)` / `pluginDesc(id, fallback)`（`i18n.ts`），`fallback` 传插件清单里的 `name` / `description`（用户自装插件没有 i18n 键时用）。**禁止再直接渲染 `plugin.name` / `plugin.description`** —— 清单里那些是英文常量（`"Custom Launch"` / `"Search the web with your default browser"`…），中文界面下会露英文，用户报的「中文时结果区有些插件名称与描述没翻译」就是这么来的。已修四处：结果区插件行、右键菜单「运行 X」、详细搜索 commands 行、插件总览；`pluginName` 同时补了 fallback（此前缺键会返回 `plugin.xxx` 字面量）。
      - **新增插件 = 三件事一起做**：清单里 `name`/`description`（英文兜底）+ i18n 的 `plugin.<id>`（名字）+ `plugin.<id>.desc`（描述），**五语言都要**。缺名字键 → 中文界面露英文；缺描述键 → 描述为空。
    - **设置侧栏固定五项：常规 / 风格 / AI / 搜索 / 插件**（不再有独立的「技能扩展」项）。
    - **「AI」分类 = 三个分块**：**AI 模型**（供应商 / 模型 / Base URL / API Key / 搜索源 / 安全档位 / 回合折叠 + 保存）、**技能 (Skill Store)**、**工具 (MCP)**（从 URL 安装工具 / 已安装工具 / 社区工具 / 打开工具编辑器）。三个分块标题**必须用与「风格」相同的 `.settings-group-title`**（用户要求「各个分块采用跟风格里的分块一样」），块内次级标题仍用 `.settings-marketplace-title` 以区分层级。
    - **「插件」分类 = 插件市场总览**（只读）：列出 `pluginRegistry.getAll()` 的全部插件（图标走 `window.__lunac_plugin_icon`，即与结果区同源的 `pluginIconSvg`；名称/描述走上面两个函数），每行一个「打开」按钮走 `window.__lunac_open_plugin(id)`。**它必须与 AI 下的 tools 严格区分** —— tools 是「AI Agent 能调用的自定义工具（MCP 桥）」，这里的行是 Lunac 自己的插件（结果区里能搜到、点开的那些）。此前两者混在同一个分类里、分类名还叫「插件 (MCP 工具)」，正是用户要拆开的对象。搜索关键词放每行的 `title`（悬停可见），不铺在界面上。
    - **设置面板不自己 `executePlugin`**：切插件要动 `#results-container` / `#search-bar` 状态，属主界面职责 —— 一律走 `window.__lunac_*` 桥（`__lunac_open_plugin` / `__lunac_execute_tool_editor` / `__lunac_refresh_plugin`）。

51. **新增按钮 / 控件必须显式声明主题样式（2026-09-19）**：**WebView2 里没有「继承到主题」的原生按钮** —— 只要某个 class 漏写 CSS，它就会退回浏览器默认外观（浅色凸起方块、`border-style: outset`），在一片深色毛玻璃里像贴上去的异物。
    - **成因与实例**：设置 ·「AI → 技能 → 已安装技能」列表的三个按钮里，`.settings-skill-open` 有规则，而 `.settings-skill-edit` 与 `.settings-skill-del-installed` **两个类在整份样式表里一个字都没有**（HTML 里写了 class，CSS 里忘了加），于是「编辑 / 移除」长期是原生按钮。修复 = 把它们**并入同族的既有规则**（动作按钮并进 `.settings-skill-open`、中性/危险按钮并进 `.settings-skill-del`），不新写重复规则。
    - **判据（动作 vs 状态，沿用规则 45）**：动作按钮（打开 / 编辑）走 `--accent` 系（底色 `--accent-bg` + 边框 `--accent-border`），**文字一律灰阶 `--text`**；中性 / 危险按钮（移除）无底色、边框走 `--border-glass`、文字 `--text-dim`、hover 转 `--red`。
    - **两段式确认（`data-armed="1"`）必须有自己的可见态**：只换文字的话，用户看不出「再点一次就真删」。规范写法 `.xxx[data-armed="1"] { color: var(--red); background: rgba(192,138,138,0.14); }`。
    - **回归口径（浏览器实测 `getComputedStyle`，别看截图猜）**：① 同族按钮的计算样式**逐项相同**（`backgroundColor` / `borderTopColor` / `color` / `padding` / `borderRadius`）；② **`borderTopStyle` 不得是 `outset`**、`backgroundColor` 不得是 `buttonface` / `rgb(239,239,239)` 这类默认值；③ 新规则生效**且**它后面的规则也生效（承载它的 inline `<style>` 若被语法错误打断，**该规则之后的所有规则会被整体丢弃** —— 所以必须顺带读一条靠后规则的值作旁证）。2026-09-19 实测（主色 `#c0a0a0`）：open 与 edit 的 `backgroundColor` 均为 `rgba(192,160,160,0.14)`、`borderTopColor` 均为 `rgba(192,160,160,0.32)`、`color` 均为 `rgb(227,227,227)`；del-installed 为 `rgba(0,0,0,0)` + `1px solid rgba(255,255,255,0.1)` + `rgb(173,173,173)`；armed 态为 `rgb(192,138,138)` / `rgba(192,138,138,0.14)`（约 0.1s 过渡后才到位，**要等 300ms 再读**）；同 `<style>` 内 127 条规则无截断。
    - **纪律**：往设置面板或任何插件里**加按钮时，class 与 CSS 必须同一次改完**；发现「某个 class 在样式表里 grep 不到」就是 bug，不是「有默认样式兜着」。

52. **文档结构纪律：待办只有一个落点（2026-09-19 整理）**：
    - **唯一待办真相源 = [agent-feature-backlog.md](./agent-feature-backlog.md)**。`ai-spec.md` / `agent-implementation.md` / `agent-ui-spec.md` / `architecture-rendering.md` **正文一律不得再出现「待办 / 路线 / 待实现 / 下一步」小节**（本节 §10 只留一句指针）。
    - **教训**：待办原先散在四处（§10 待办路线、§13 参考设计、§19.6 待实现清单、§20 路径 2，外加 backlog 自己），同一件事写两三遍、完成状态还不同步 —— 整理时实测发现 **Humanizer 按钮与安全警告块其实早已落地**、而文档一直标着「待实现」。
    - **做完就删，不标 ✅ 留原地**：「为什么这么做、踩了什么坑」写进 §11 规则 / 代码注释 / Git 记录，**不由待办条目承载**。反例：§9 曾有一张 17 行的「已关闭问题」存档表，纯属噪声。
    - **规范正文只写「是什么 / 为什么 / 不得怎么做」**；现状（已实现能力）归 `agent-implementation.md`，界面规范归 `agent-ui-spec.md`。
    - **§ 号变更靠对照表换算**：backlog 重排后，代码注释与规范里残留的旧编号（如 `backlog §8.3`）**不必逐处回改** —— backlog 头部有「§ 号变更对照」表，按表换算即可。
    - **`core/` 的含义**：它是**被 `.gitignore` 排除的旧 CLI 参考源码，不在仓库里**（`git clone` 下来不会有）。文档里凡以 `core/...` 为落点的路径，一律理解为「参考它的设计、在新宿主重建」，**别照它去找代码**。

53. **往期会话检索与「往期会话索引」注入（2026-09-19，原 backlog A2）**：会话库（`ModuleData\history\chat.db`）的 FTS5 索引**从建库起一直没有调用方**，本轮接上 —— 内置工具 `SessionSearch` + 启动时注入一段索引。契约、通道与实测数据见 §3.5「往期会话检索」。**六条不得回退**：
    - **注入必须是冻结快照**：只在 agent 启动时取一次、进程内逐字节不变（规则 18 / 23）。**禁止**改成「每轮重建」或「保存会话后就刷新提示词」—— 那会让每一轮都 cache-miss，代价远大于这点信息量。
    - **FTS 之外必须留 `LIKE` 兜底**：trigram 分词器要求查询词 ≥3 字符，2 字中文（「缓存」）在纯 FTS 下**静默 0 条**。这是「功能看起来在、其实没生效」的典型 —— 改检索时必须保留兜底，并跑 `chat_db` 的单测（`search_falls_back_to_like_for_short_cjk_query` 就是专门钉它的）。
    - **走自定义 MCP 方法，不进 `tools/list`、不弹审批卡**：理由见 §3.5 的表。**禁止**把它做成 `tools/*.json` 的用户工具（会被用户改坏，且每次调用都弹卡）。
    - **工具被禁用 ⇒ 索引一并停注**：`--disallowedTools` 里出现 `SessionSearch` 时不得再注入索引段，否则提示词会指挥模型去调一个不存在的工具（同「技能清单遇 `Skill` 被禁就不列」）。
    - **不要给绝对日期**：Rust 侧没有时区表（项目不为此引 chrono，见 `log.rs`），绝对日期只能是 UTC，会出现「本地已是今天、UTC 还是昨天」的错位 —— 用相对天数。
    - **只读（plan）档也建 MCP 桥**：桥现在承担「索引 + 检索」两个自定义方法，所以只读档不再跳过建桥；但 `mcp__*` 工具**仍然不进工具池**（只读档不给动手工具这条不变）。改这里时别把「不建桥」写回去，否则只读档的 `SessionSearch` 会静默失效。

54. **子代理框架（`Agent` 工具，2026-09-20，原 backlog A1）**：契约与实测见 §3.5「子代理」。**五条硬约束不得放宽**：
    - **独立上下文**：子代理自带一份全新 `history`（首条 = `user(prompt)`），与主对话**零共享**。**禁止**把主对话历史拼进子代理请求（那等于没省上下文，还会让子代理按主对话的既有结论行事）；也**禁止**把子代理的中间过程回灌主对话（只回最终报告）。因此 `prompt` 必须自包含，子代理系统提示词里明确「缺信息就写清假设、不要提问」。
    - **结果可归因**：回灌文本固定 `[task-<N>] subagent report:` / `[task-<N>] subagent failed:` 形态，`task_id` 来自**进程内存态**计数器 `TASK_SEQ`（它要进模型可见的回灌文本，短才好读；**不许**塞 session 前缀）。事件 `task_started` / `task_progress` / `task_done` 都带 `task_id`。**不要**把 task id 落盘或做成跨进程工单号 —— 跨运行的归因由每条消息上的 `session_id` 负责（A11，见规则 64）。
    - **并发 = 花钱 ⇒ 三重闸门**：`parallel_safe("Agent")` 为 `false`（**不进只读并行白名单**，那张表只收纯读工具）+ `SUBAGENT_PARALLELISM = 3`（A14 起：同一轮里连续的 `Agent` / fork 技能可并发，**3 是刻意与只读批的 4 取不同值** —— 每个子代理预算 30 万 token）+ `MAX_SUBAGENT_ROUNDS = 8` + `SUBAGENT_BUDGET_TOKENS = 300_000`（用量**含 `cache_read`**）。**到顶不报错**，回一份「已尽力」的部分报告（`hit the N-token budget at round R` / `hit its N-round ceiling`）——报错会把「已经花掉的钱」变成零产出。
    - **工具集在循环外算一次，剔除三类**：`subagent_tool_defs(tool_defs)` = 本轮工具池 **剔掉 `Agent` / `SessionSearch` / `mcp__*`**（2026-09-20 复查补全）。① `Agent` ⇒ **防递归**：子代理**根本看不到**这个工具（比「看见了再拒」干净）；② 后两件**必须有 MCP 桥才能跑**，而子代理不接桥 ⇒ 留着就是**保证失败**（`MCP bridge is not connected` / 「往期会话检索不可用」），`mcp__*` 还会**先弹一张注定白问的审批卡**。剔完还要保证 `tools` 数组**逐字节一致**（多子代理之间能共享端点侧 prompt cache，规则 18）。**`--disallowedTools` 把工具全裁掉时子代理工具集为空，这是正确行为，不要"修"。** 守门单测 `subagent_tool_defs_drops_agent_and_bridge_only_tools`。
    - **生成参数与主循环同源**：`max_tokens = max_tokens_for(cfg.thinking.get())`，并附同一个 `thinking` 字段。**不许**给子代理另定一个小的 `max_tokens` —— **不发 `thinking` 字段 ≠ 关思考**（端点默认开着），思考文本算在 `max_tokens` 里，小上限会把报告挤空；用户 `LUNAC_THINKING=off` 时子代理也必须跟着关。复用 `cfg.thinking` 还顺便省掉了在子代理里重走 400 降级链（能派子代理说明主请求已成功过一次）。
    - **系统提示词 = 角色段 + 环境块 + 技能清单**（启动时拼一次）：子代理看不到主对话的环境块，不给 `env_block(cwd)` 它就不知道自己的工作目录；`Skill` 在它的工具集里，而技能清单原本只写在主提示词里 —— 不给清单等于给一串不知道有哪些钥匙的钥匙串。
    - **非流式 + 不接 MCP 桥 + 审批逐次做**：子代理请求固定 `stream: false`（主对话只要结果，省掉整套 SSE 解析分支；代价是无逐字流式，**刻意如此**）；`run_one_tool(tctx, None, …)` 传 `None` 不接桥（桥是单线程 stdio 通道，主循环还持有 `&mut Bridge`，且一接就是全量用户工具 = 不受控的副作用面）；`Agent` 本身要审批（`needs_approval`）、**plan（只读）档直接拒绝**（`Agent is disabled in read-only (plan) mode`，`task_done.ok=false`），而子代理**内部每次写操作仍各自再走一次审批** —— 「批一次派代理」≠「预先批准它接下来做的每件事」。
    - **实现位置**：`run_subagent()` / `run_agent_tool()` / `subagent_tool_defs()`（[core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs)）；`dispatch_tool()` 拿不到 `cfg`，所以在主循环工具执行分支里对 `Agent` **特判**。**副作用**：`Agent` 不走 `run_tool` ⇒ 日志里没有 `tool Agent ok (…)` 那行，它的记录是 `子代理 task-N 启动/完成/失败`（含耗时与报告体积），排查时按「子代理」搜。守门单测四条（工具总数 **15**）。**收口指令的落点教训**（打满工具轮次时追加内容只能拼进 `tool_result` 文本，不得新增消息/内容块）见规则 23，改这两处时一并遵守。

55. **MCP resources 读侧（`ListMcpResourcesTool` / `ReadMcpResourceTool`，2026-09-20，原 backlog A3）**：契约与实测见 §3.5「MCP resources 读侧」。**六条不得回退**：
    - **工具必须条件注册**：只在桥真的接上了用户工具（`!bridge.defs().is_empty()`）时才追加进 `tool_defs`。出厂时 `tools\` 只有 README 与 `*.example` ⇒ `resources/list` 恒为空表，无条件注册就是在**每一次请求的固定前缀**里放两件永远查不到东西的占位工具（规则 18 ⑤）。**别改成无条件注册**，也别为此新增一次启动 RPC。
    - **安全边界是硬要求**：`uri` 来自**模型**，而模型会被读到的文件内容提示注入 ⇒ 服务端只允许读 `tools\` 目录内的 `.json`，判据是 **`canonicalize()` 之后比前缀**（`..` 与符号链接都会被展开），不是字符串黑名单；裸名字（`foo` / `foo.json`）在 `tools\` 下解析，**含路径分隔符即拒**。单文件 512 KB 上限。守门单测 `resources_read_is_confined_to_the_tools_dir`，**改这段必跑**。
    - **服务端守规范形状、客户端做渲染**：`resources/list` → `{resources:[…]}`、`resources/read` → `{contents:[…]}`（**都不是 `tools/call` 的 `content`**）。渲染成文本是客户端的事（`Bridge::list_resources()` / `read_resource()`），服务端不掺私货 —— 它是真 MCP server，形状是互操作契约。
    - **错误走 JSON-RPC `error`**：`resources/*` 是标准方法，不是工具调用，所以不用 `result + isError` 那套。`Bridge::request()` 会把 `error` 转成 `Err`，最终仍落成 `is_error=true` 的 `tool_result`，模型照旧能读到原因并改方案。
    - **桥工具的名单只有一个真相源**：`tools::BRIDGE_TOOLS` / `needs_bridge()`。三处都从它推导 —— `dispatch_tool` 的无桥早退（**plan 档放行**，与 `SessionSearch` 同理）、`subagent_tool_defs()` 的剔除、启动时的条件注册。**新增走桥的工具只改这个数组**；`dispatch_tool` 里的 `match` 必须保留显式兜底分支（不许写成 `_ =>` 落到某个具体实现上）。
    - **权限三口径**：**不审批**（只读本机自己的工具定义，与 `Read`/`Grep` 同级）、**plan 档放行**、**必须串行**（走单线程 stdio 桥 —— 判据是「要不要走桥」，不是「是不是只读」）。三者都有守门单测 `mcp_resource_tools_are_bridge_only_and_conditional`。

56. **长期记忆与后台复盘 fork（`Remember` 工具 + 每 N 轮一次的后台复盘，2026-09-20，原 backlog A4）**：契约与实测见 §3.5「长期记忆与后台复盘 fork」。**八条不得回退**：
    - **注入必须是冻结快照**：启动时读**一次**，进程内逐字节不变（规则 18 / 53）。**禁止**「写完记忆就刷新提示词」「每轮重读一次记忆文件」—— 那会让每一轮都 cache-miss，代价远大于这点信息量。会话中新写的记忆**只落盘**，下次重启 agent 才可见。
    - **停注与注册必须同源**：`remember_on = memory_enabled() && 桥接通 && !disallowed(Remember)` 这**一个**判据同时管三件事 —— 是否注入记忆段、段里那句「用 `Remember` 追加」怎么写、`Remember` 是否进工具表。三者必须一致：提示词让模型调一个不在工具表里的工具，就是规则 14 那类自相矛盾的组合。（与规则 53「工具被禁 ⇒ 索引一并停注」是同一条纪律。）
    - **计划模式必须显式拒**：`Remember` 走 `dispatch_tool` 的 `needs_bridge` **早退分支**，**绕过了 `tools::run` 里的写类只读拦截** ⇒ 只读档的拒绝只能在那个分支里自己写。**别**假设「写类工具在 plan 档统一被拒」覆盖了它。
    - **上限是硬闸、且不许静默截断**：单条 ≤2000 字符、整文件 ≤6000 字符、按整条去重。超限**报错**（模型看得见、可自行整理），静默截断会让模型以为已经记住了。`replace: true`（整体替换）是「记忆满了怎么办」的**唯一出口**，删掉它记忆就会永久卡死在上限上。
    - **触发按提问数、不引定时器**：`LUNAC_NUDGE_INTERVAL`（默认 10，`0` = 关）数的是**完成的用户提问数**。**不要**改成工具轮次（一次提问内部有 16 轮，按那个计数会在半截对话上复盘）或定时器（用户什么都没干时空转，每次都是真花钱）。
    - **后台 = 提问之间，不是随时并发**：`run_query` 返回后 `spawn`，**处理下一条消息之前收掉已跑完的那次**（`is_finished()` 才 `join`）。**不要**改成无条件 `join`：`REQUEST_TIMEOUT_SECS` 是 30 分钟，一次卡住的复盘会把用户的下一次提问一起卡住（后台事务不该决定前台时延）；也**不要**改成随时并发（两条线程同时 `emit` 会交错 stdout）。复盘**不发** `task_*` 事件（前端收到 `task_progress` 只会改状态栏且没有恢复时机），结论只进 `log::info`。
    - **复盘的写权限靠白名单收窄，不靠提示词**：工具集 = 从本轮工具池里挑 `Read`/`Glob`/`Grep`/`Write`/`Edit`/`Skill`/`Remember` 的子集（于是天然继承 `--disallowedTools` 与条件注册）。**`Bash` / `PowerShell` / `WebFetch` / `Agent` / MCP 工具一律不给** —— 它是无人值守进程。提示词里那句「只在用户明确说过时才改技能」是软约束，白名单才是硬约束，**别把工具面放宽成「反正提示词说了别乱用」**。
    - **审批走同一条通道、桥要自己一条、`Cfg` 要独立副本**：复盘的写操作经 `can_use_tool` 交前端（运行方式自动档 ⇒ 静默放行），**不给后台进程开审批旁路**；主循环那条 `&mut Bridge` 跨不了线程 ⇒ 复盘自己 `Bridge::connect`；`Cfg` 含 `Cell` 不是 `Sync` ⇒ `Cfg::detached()`（同端点/凭据/模型 + 当前已缓存的 thinking 形态）。**技能目录必须进 `add_dirs`**，否则白名单里的 `Write`/`Edit` 会被工作区锁直接拒，「改技能」永远不会发生。

57. **技能（fork 模式 + 自带脚本 / 资源，2026-09-20，原 backlog A5）**：契约与逐项对照见 §3.5「P4 已完成」表。**九条不得回退**：
    - **frontmatter 只认四个字段**：`name` / `description` / `context` / `allowed-tools`。旧 CLI 还有 `model:` / `effort:` / `paths:` / `hooks:` / `agent:` / `shell:` / `argument-hint:` —— 它们在 Lunac **没有落点**（端点只有一个模型、思考只有开/关两档、没有条件激活与技能钩子），**解析了不用就是死代码**，还会让人误以为改了会生效。要加新字段，先有落点再解析（守门单测 `unknown_frontmatter_keys_are_ignored` 钉住）。
    - **`context` 只认 `fork` 整词**（去引号后忽略大小写比较），其余一律按 inline。抄旧 CLI 的 `context === 'fork' ? 'fork' : undefined`，**不要发明第三态**，也不要支持 `context: subagent` 之类的别名。
    - **两路共用 `find()` / `instruction()`**：inline 与 fork 的差别**只有「谁来照做」**，查找顺序（精确 key → name 忽略大小写 → key 忽略大小写）与正文生成（剥 frontmatter + 替换 `$ARGUMENTS`）必须同源。各写一份的后果是「模型看到的可用列表」与「实际命中的技能」在边界情况下不一致。
    - **副作用按最坏模式算，判据只有一处**：`parallel_safe("Skill")` = `false`（白名单只看名字，判不出模式）；`needs_approval_with(name, input, skills)` 才是带输入的判据（fork 要审批、inline 不要），且**必须复用本文件的 `needs_approval()`**（内含「全部 MCP 工具一律问」），别直接调 `tools::needs_approval()`。**plan（只读）档一律拒 fork** —— 技能正文是用户装的、里面可以写任意 `Bash`，放它出去等于把「只读」这个承诺交给第三方的 md 文件去守。
    - **工具面：白名单是交集，不是并集**：`allowed-tools` **只在 `subagent_tool_defs()` 的结果里挑** ⇒ 技能无法借白名单把 `Agent` / 走桥的 / `mcp__*` 捞回来。空 = 不限制；非空但一件都没匹配上 = **一件也不给**（fail-closed + 一行 warn 日志）—— 写错白名单时宁可让子代理只靠推理，也不能悄悄放开成全集。
    - **回灌措辞必须与 inline 明确区分**：fork 回的是「`Skill "X" ran in sub-agent [skill-N] — the work is DONE; this is its report:`」，不是「照着做的指令」。含糊的后果是模型把子代理的报告**再执行一遍**（同一件事做两次，还可能重复写文件）。同理，清单里 fork 技能必须带 `[subagent]` 标记 —— 那是模型唯一的提示。
    - **递归**：子代理工具集里**保留** `Skill`（技能组合很自然），fork 技能的再 fork 由 `skills::run()` 的守卫拒绝（回 `context: fork … cannot be loaded inline`）。这是**防路由漏洞的保险**，正常路由不会走到（主循环与 `run_subagent` 都优先走 `run_forked_skill`）；**别**改成「在 `subagent_tool_defs()` 里把 `Skill` 也剔掉」—— 那会连带砍掉 inline 技能的子代理可用性。
    - **自带资源只走「调用时附上」这一条通道，绝不进系统提示词**（2026-09-20 与用户定稿）：清单随用户往目录里丢文件而变，进了提示词就等于让整段前缀缓存跟着文件系统抖动（规则 18）。两个模式都要给到（inline 附在正文之后、fork 附进子代理的任务说明 —— 子代理看不到主对话与技能清单，不给它就只能猜路径）。路径给**相对形式**并写明相对谁（`<skills dir>/<key>/`），别给绝对路径：短、可移植、不把本机用户名写进会话历史，而绝对根 env_block 本来就给了。边界**保守**（深度 ≤ 3 / 条数 ≤ 40 / 跳过隐藏项与 `node_modules`·`target` / **不跟随符号链接**）：漏一个深层文件只是少一条提示，把几千条路径灌进上下文是灾难；截断时**如实上报**，不许静默。**别**为此新增 frontmatter 字段或新工具 —— 那是多一份会与磁盘漂移的状态、多一次往返。
    - **remote 不移植（已定论，别再开工）**：旧 CLI 的 `remoteSkillLoader` / `remoteSkillState` 在 `feature('EXPERIMENTAL_SKILL_SEARCH')` 之后、**磁盘上文件已不存在**，且依赖 Lunac 没有的 `akiBackend` 服务。**A5 至此全部完成**（fork + 自带资源），条目已从 backlog 撤下。

58. **写入内容的凭据扫描（2026-09-20，原 backlog A6）**：落地形态与规则清单见 §13.1（唯一真相源 `core-agent/src/content_safety.rs`）。**六条不得回退**：
    - **只做「凭据 / 密钥泄漏」一类**（2026-09-20 与用户定稿）：**不做**「代码注入 / XSS / 反序列化 / 加密缺陷」。正则在正常代码里做不了语义级判定，那几类必然满屏误报，最后的结局是用户学会无视告警 —— 比没有更糟。§13.1 里旧设想的「25 条规则」是**抄不到的参照物**（`core/security/scanContent()` 整个目录不在仓库），**别照那个数字去补齐**。
    - **扫的是「将要写进磁盘的文本」**：`Write` 取 `content`、`Edit` 取 `new_string`；**绝不扫 `old_string`** —— 那是要被删掉的内容，扫它会把「正在清理凭据」的操作反而标成可疑。
    - **时机在写入前（`can_use_tool` 审批时），不是落盘后**：落盘后只能事后告知，而用户的决策点就在审批卡上。
    - **判定不代替决策**：agent 只往 `analysis.secrets` 上报 `[{rule, line}]`，**不拒绝执行**。前端 `classifyRequest()` 把它当「必须人看」那一档：① **任何**运行方式档位（含「自动」）都不自动放行；② **不给「始终允许」**（写类工具的「始终允许」= 以后所有 `Write` 都免问，正是这条扫描想防的）；③ 命中项要**可见地列在卡片正文**（`agent.static_secrets_body`），不能只塞标题 tooltip —— 藏在 hover 里等于没做。工具侧另给**模型**一句提示（`tools.rs` 的 `secret_note()`），否则模型不知道用户为什么被多问了一次。
    - **不并进 `dangerous` 通道**：那条的文案是「危险命令」，与「正常代码里混进了一把 key」是两回事，共用一个字段会让用户看不懂到底在问什么。
    - **闸门是为了「宁漏勿误报」，只许收紧不许放松**：`RegexSet` 先跑一遍（零命中即返回，正常写入零额外成本）；通用赋值规则要**同时**过「键名含 key/secret/token/passwd/password/credential + 赋值到行尾 + 值不是占位符」三道闸；展示 5 条 / 采集 200 条 / 512 KB 上限都是**成本与噪声**的约束。动规则集必须同步 `content_safety.rs` 的 10 条单测并跑 `cargo test`；动前端展示要跑 `npx tsc --noEmit`。

59. **计划模式闭环（计划相位，2026-09-20，原 backlog A7）**：契约与实测见 §3.5「计划模式闭环」；**计划文档本身的写法**仍以 §12 为准（那节是给模型看的规范，本节是给实现看的）。**七条不得回退**：
    - **计划相位 ≠ 安全档位**：`Ctx.read_only` 是**用户**的档位（启动时定死、改它要重启 agent）；`plan_phase` 是**模型自己**的临时承诺（进程内即时生效、`ExitPlanMode` 被批准后立刻解除）。实现上必须让两者**共用一个判据出口**（`tools::write_blocked()`），但**拒绝措辞必须分开**。
    - **约束是硬的，不靠模型自觉**：写类工具在计划相位里**执行侧直接拒**（不靠提示词劝），且**四个绕开 `tools::run` 的早退分支各自也要补一次**（`Agent` / fork 技能 / `Remember` / MCP 与走桥工具）—— 少一处就等于给「计划相位里不许写」开了个后门。将来新增任何早退分支（新工具、新的 `needs_bridge` 通道）**必须同时**补这一判据。
    - **工具无条件注册，靠执行侧拦**：`EnterPlanMode` / `ExitPlanMode` 永远在 `defs()` 里 —— 工具表是请求体里的**固定前缀**（规则 18），而计划相位是运行期翻转的，事后没法增删 ⇒ 只能硬拒。别想着「进计划模式时把写类工具从工具表里删掉」。
    - **`EnterPlanMode` 免审批、`ExitPlanMode` 要审批**：前者只改一个进程内标志（比 `TodoWrite` 还轻）；后者**那张卡就是它的产品** —— 用户必须在卡上读到整份计划再裁决。两件都**不能**进「始终允许」（把 `ExitPlanMode` 白名单化 = 以后每份计划都自动批准，等于把整个计划模式关掉）。
    - **只读档下 `ExitPlanMode` 也要拒**：写类被用户档位永久拒绝时，批准计划也执行不了 —— 必须让模型改用正文交代计划、并说明「要执行得去设置里改档位」。说成「批准后就能写」是把模型引到一个必然失败的动作上。
    - **前端状态只镜像、不推断**：`system/plan_mode`（`state: on/off` + 可选 `reason`）是 agent 里那个标志的广播；前端**不许**拿「模型调过哪些工具」自己推 —— 那要在「调了工具」与「用户批准了没有」之间做二次判断，极易与真值脱节。**agent 进程重启要把横幅清掉**（`plan_phase` 是进程内状态，随进程消失）。
    - **计划落盘只在批准那一刻、且不挡执行**：`ModuleData\plans\<本地时间戳>.md`。时间戳由前端给（本地时区）+ Rust 侧严格校验（同 `append_usage_log`）；落盘失败只提示，**绝不**反过来拦住已经批准的执行。拒绝时给模型的回话必须**明说「你仍在计划模式」**（i18n `agent.plan_deny_msg`），否则它会接着调写类工具、白烧一轮往返。

60. **图片附件（A8，2026-09-20）**：契约与实测见 §3.5「图片附件」。**五条不得回退**：
    - **只传路径、不传字节**：stdin 里给的是 `{"type":"file","path":…}`，base64 由 core-agent 读出来再转 —— 前端三种附件来源本来就已经是路径，别为了「省一次读盘」把几 MB 的 base64 塞进 IPC 管道与 WebView 内存。
    - **`[Attached files]` 文本必须保留**：新增的图片块是**追加**，不许替换那段文本（它是历史 / 标题 / 复制三条旧路径的唯一依据，也是「哪个路径对应哪张图」的唯一说明）。任何「有图片块了就把路径文本去掉」的改动都是回退。
      - **2026-09-21 补：这条契约在「落盘 / 恢复」两处被违反过，已修**（用户报「历史记录进入时文件类型消息会消失」）。根因：`persistCurrentSessionInner()` 与 `restoreSession()` 都用 `cleanUserContent()` 洗用户消息，而它**同时剥掉**「发送期提示词」和「`[Attached files]` 块」⇒ 附件清单**只活在内存里**，磁盘上只剩提问正文，一进历史气泡里的 `📎 文件名` 就没了（连带「复制正文 / 重试带附件」两条路径一起断）。现在：
        - **落盘 / 恢复只调 `stripInjectedHint()`**（只剥提示词，空转、纯防御 —— `startAgentChat()` 本来就把提示词拼进 `wrappedQuery` 发给 agent，`chatHistory` 里从来没有它）；该函数**必须先判 `text.startsWith("## ")` 再切 `\n\n---\n\n`**（2026-09-21 同批补的闸）：注入块一律以 `## ` 开头，而普通提问正文里完全可能出现 `\n\n---\n\n`（markdown 分隔线），不判前缀就切会把「上面\n\n---\n\n下面」削成「下面」，**落盘即丢前半段**。同日实测：加闸前该断言为红，加闸后全绿。
        - **`cleanUserContent()` 只允许用在纯展示的一次性场景**（会话标题、抽屉预览）；
        - **气泡文本由 `userBubbleText()` 从存储形态现推**（`parseAttachedQuery()` → `正文 + \n📎 文件名1, 文件名2`），与实时路径 `appendUserMsg()` 的形态逐字对齐 —— **两个形态必须在这一处对齐，别在渲染点各写一遍**。
        - **回归口径**：发一条带附件的提问 → 打开历史记录点回来 ⇒ 气泡里的 `📎 文件名` 必须还在；「复制」只得到正文；「重试」能把原附件重新带上；再发一条正文里含 `---` 分隔线的提问 ⇒ 落盘后首段仍在。（`userBubbleText()` 的输出只用于渲染、**不得回写 `chatHistory`** —— 展示形态里没有路径，回写一次「复制 / 重试」就再也拿不回附件。）
    - **开关默认关，且只在前端**：`config\ai.json` 的 `vision`（默认 `false`）。发给不支持视觉的端点必 400，而 agent 侧**无法预判**模型能力 ⇒ 只能由用户显式断言。**不许**改成「按模型名自动推断」或「先发再 400 回落」——前者会猜错，后者每轮白烧一次请求并打断前缀缓存。
    - **类型只认魔术字节、失败必须可见**：放行 PNG / JPEG / GIF / WebP 四种（不信扩展名）；读不出来 / 超限 / 超张数的一律进 `system/attachment_note`（`skipped:[{path,reason}]`）并在前端可见地列出来。**静默丢弃是最坏的一种**——用户只会看到「模型说它看不到图」。
    - **附件读盘不走工作区锁，这一点不许「顺手补上」**：路径来自用户显式选中（不是模型自己找到的），而剪贴板图片就落在 `%TEMP%` —— 套锁会让最主要的那条用法直接失效。模型的 `Read` 仍然照旧受锁约束，两者不要混为一谈。

61. **权限 hooks（A9，2026-09-20）**：契约与实测见 §3.5「权限 hooks」。**七条不得回退**：
    - **只做真有落点的事件，一个都不铺空跑**：`SessionStart` / `UserPromptSubmit` / `PreToolUse` / `PermissionRequest` / `PostToolUse` / `PreCompact` / `Stop` / `SessionEnd` 这 8 个，每一个都在 `main.rs` 里有确切调用点（表见 §3.5）。**别**为了「和 Claude Code 对齐」把 `Notification` / `SubagentStop` 之类也做成事件 —— 配了永不触发的项比没有更糟（用户以为装了保护）。
    - **只认显式拒绝：失败一律「放行但可见」**：只有退出码 2 或 `{"decision":"deny"}` 才拦；**超时 / 崩溃 / 输出看不懂 = 不拦 + `kind:"error"` 的 `system/hook_note` + 一行 WARN 落 agent 日志**。也别反过来做成 fail-closed：用户脚本一崩就全线卡死，比没装 hook 更糟。这条与 A8 的「失败必须可见」是同一条纪律的两个落点。
    - **`allow` 只等于用户白名单，不得越过安全闸门**：hook 放行 ⇒ 只跳过**审批卡**；**静态安全分析命中（危险命令 / 写入内容里的凭据）仍强制弹卡**（判据 `hook_allow_needs_card()`），工作区锁与计划相位的写类拦截也照旧 —— 规则 14 的「任何一道闸门都不得为了少点一次同意而放宽」在这里是同一句话。**别**把 `opaque`（判不定）也算进强制弹卡：它的口径要与前端「自动」档一致。
    - **配置只有一份真相，且必须热重载**：`config\hooks.json`（`enabled` 缺省 true；文件不存在 = 没配）。**agent 按 mtime 重读**（改完即时生效，与技能的「扫描一次」不同 —— 这里没有前缀缓存的问题），解析失败**保留上一份有效配置**。宿主因此**无条件注入** `LUNAC_HOOKS_FILE`（连文件不存在时也给）：若改成「文件存在才注入」，运行中新建设置的用户就得等下次 spawn 才生效 —— 这类「开了没反应」的坑不要在别处重演。
    - **事件必须挂在「所有工具调用都会经过的那一层」，不许下沉进 `tools::run`**：`PreToolUse` / `PermissionRequest` 在主循环与子代理循环各自的「执行工具」段统一过一遍（覆盖**只读工具**与**子代理内部**的调用），`PostToolUse` 在 `run_one_tool()` 里。下沉进 `tools::run` 会漏掉 `Skill` / `SessionSearch` / `needs_bridge` / `Agent` / fork 技能那几条**早退分支**（它们根本不进 `tools::run`）—— 与规则 59 里「早退分支各自补 `write_blocked`」是同一类陷阱的镜像。
    - **`PostToolUse` 的文本只拼进 `tool_result` 的文本内部**：它是**追加**到既有 `content` 字符串里，**不得**新增消息、也不得新增内容块（规则 23 —— 末尾消息形态一变，缓存 `read` 就塌到 `system + tools` 的量级）。
    - **Windows 下起 `cmd /C` 子进程必须用 `raw_arg` 拼命令行**：`cmd /C` 的引号语义由 cmd 自己解释，而 `Command::arg` 会按 MSVC 规则把命令里的 `"` 转义成 `\"`（命令含空格时必然触发）⇒ 形如 `python "C:\my hooks\check.py"` 的命令**整条失败**（实测：同一条 `type "<file>"` 带引号时 hook 拿不到任何输出，去掉引号即正常）。**判据是「被解释的是 `cmd`，不是 MSVC」**，所以同一坑在**每个把整条命令字符串塞给 `cmd /C` 的地方**都存在，与它属于哪个模块无关 —— 2026-09-20 已按此判据全仓清理：`hooks.rs` / `tools.rs` 的 `Bash` / `host` 的 `mcp_server.rs`（用户 `tools\*.json` 的执行通道）/ `host` 的 `kill_port` 四处。**`PowerShell` 是例外**：它自己的解析器认得 `\"`，实测 `Write-Output "a b"` 输出正确 ⇒ **刻意不改**（改了反而要自己重拼一遍命令行）。断言钉在 `tools.rs` 的单测 `quoted_shell_arguments_survive_the_command_line`。
    - **`cmd` 的两条「解释权」陷阱**（同一天在 `kill_port` 上一并实测）：① `for /f ('…')` 里那句命令是**交给另一层 cmd 执行**的，所以写在里面的重定向要写成 `2^>nul` —— 裸 `2>nul` 会被外层 cmd 抢先解释，直接报「此时不应有 2>」，整条循环一次都不跑；② 但 `do` 子句末尾的 `2>nul` **不要**转义 —— `do @echo x 2^>nul` 会把 `2>nul` 当普通文本打进输出（实测输出 `FOUND 8896 2>nul`）。**判据：这段文本是「外层 cmd 读」还是「子 cmd 读」，只有子 cmd 读的才转义。**
    - **UI 开关已移除：hooks 是开发者选项（2026-09-21，用户明确要求）**：设置面板不再有「权限 hooks」行、描述与「打开 hooks.json」按钮 —— 它**常驻可用**，是否生效只由 `config\hooks.json` 的 `enabled` 字段决定（缺省 `true`；文件不存在 = 没配 = 什么都不跑）。宿主侧三个命令（`get_hooks_config` / `set_hooks_enabled` / `hooks_file_path`）**保留**（开发者路径与 e2e 的落点），但**前端不得再接线**。别再以「用户看不到就不知道有这个功能」为由把开关加回来：用户明确说这个不需要展示给用户。

62. **只读分类（A10，2026-09-20）**：契约与实测见 §3.5「只读分类 / 可证只读」。**六条不得回退**：
    - **`readonly` 只是「放行建议」，不是安全判定**：它的反面（`false`）**不表示危险**，只表示「证不出来」。任何把 false 当危险处理、或据此拒绝执行的改法都是错的 —— 与 `dangerous` / `opaque` 的语义方向不同，别混用。
    - **只对「白名单」档生效**：手动档照问、自动档照放（三档语义见 `agent-ui-spec` §4.2）。**不许**拿它去收紧自动档，也**不许**拿它当第二道安全闸。
    - **它是正向白名单，不是黑名单**：判据是「命中受信命令表」，没列到的一律 false。加规则时**只加证实过只读的**，不要用「排除已知危险写法」的反向写法 —— 那正是旧前缀表翻车的方式。
    - **四条结构判据缺一不可**：单条命令（不拆段）、无输出重定向（`2>&1` 这类 fd 复制先摘掉）、无包装器/命令替换、命令词命中白名单。**任何一条放宽都要重新走一遍 §3.5 那张表**：`echo hi > f` 被放行一次，就等于零询问地写了文件。
    - **必须与 `dangerous` / `opaque` 自洽**：命中危险或判不出来的命令**一律**拿不到 `readonly:true`（`is_provably_readonly` 第一件事就是 `rep.is_clean()`）。这条是纵深防御 —— 前端另有优先级，但结论之间不许互相矛盾。
    - **前端缺字段按不放行处理**：只认显式的 `true`（旧 agent 不带该字段）。**不许**退回「缺字段就用本地前缀表兜底」—— 那会把已经修掉的误放行原样还回来。

63. **定价表与成本面板（A12，2026-09-20）**：契约与实测见 §3.5「定价表与成本面板」。**九条不得回退**：
    - **价格不得写进代码**：单位、币种、四类分档、每个模型一条 —— 全部落在用户可编辑的 `config\pricing.json` 里。**2026-09-29 修订（用户批准）**：原先「**不预置任何价格数字**」的理由是「本仓没逐项核对过官方定价页，凭空填一行看着很像的数会被用户当事实拿去对账」—— **这个前提已经消失**：`DEFAULT_PRICING_JSON` 里的 `deepseek-v4-flash` 峰谷价是**两处独立证据逐项对上的**（官方定价页的峰谷表 × 用户 2026-09-29 真实账单 CSV 反算的单价，结构与汇率都自洽）。所以改成「**预置但有据可核**」，且三条边界不得放松：① **只在文件不存在时写**（`ensure_pricing_file`），用户改过的一个字都不覆盖；② 每个预置模型必须带 `source_url`，并在常量注释里写明依据；③ **没有实测依据的模型不预置**（如 `deepseek-v4-pro`：官方页只有美元价，折算汇率是本机假设）—— 宁可让面板显示「未定价」。预置内容本身也要过 `validate_pricing_text`（守门单测 `default_pricing_json_is_valid`）：预置一份不合法的价表比不预置更糟。
    - **时段价（分时价，2026-09-29）**：模型条目可带 `time_windows`（数组，每条 `{days?, from, to, 四类价}`），语义是「**基础四类价 = 缺省（谷）价，第一条命中的时段覆盖它**」。**三条不得回退**：① **`from`/`to`/`days` 一律严格校验**（`HH:MM` 两位、`from < to`、`days` 是 1–7 且不重复不空）—— 宽松解析会把用户的意思读成另一个时刻，而这张表是拿来对账的；② **计价必须逐桶**：`read_usage_range` 返回的 `models[].hours[]` 是「同一批 token 按本地小时再切一刀」（逐桶之和恒等于总量），前端的 `priceAt()` 逐桶取价，**不许**拿总量乘某一个价（那等于把分时价抹平）；③ **本地时刻靠调用方给的 `utc_offset_minutes`**（东八区 480）—— Rust 侧没有时区库，**不许**为此引入 chrono，也不许把 `ts` 当本地时间直接用。
    - **金额必须逐模型算**：`read_usage_range` 的返回带 `models[]`，绝不能只按天合计（一天里换过模型 = 两个单价被混算）。任何「反正只有几厘，按天算就行」的简化都是错的。
    - **没价格就不算，不许当 0**：未定价的模型只标「未定价」并单列提示、金额前缀 `≥`。**禁止**用当前模型的单价代替缺失模型的单价，也禁止把缺失当 0 —— 两种都会让面板显示一个看起来很确定的假数字。**空模型名（历史记录）要显示成「未记录模型名」**，不许复用 `modelLabel` 的 `—`：那与「值缺失」的占位符同形，`未定价：—` 读起来正好是反的。
    - **候选与正式必须分成两个文件**：agent 只能写候选（`lunac-pricing.pending.json`），确认动作**只由用户在面板上点**。**不许**让 agent 直接改 `config\pricing.json`（提示词里也明写了这条），也不许给「自动确认」留开关。
    - **候选文件的落点是 agent 的工作目录**，不是 `config\`：配了工作区时写 `config\` 会被 `tools::guard()` 直接拒（实测见 §3.5）。改这个落点前必须先确认「agent 在工作区锁下仍写得进去」。
    - **校验不过一个字都不写**：`commit_pricing_pending` 先 `validate_pricing_text` 再覆盖；「缺字段」也算非法（金额会悄悄少算一块）。候选文件缺失时**报错**，不许静默当成「没有变化」。
    - **表格与数据只在表盘展开面板里，设置里只剩按钮**（2026-09-29 定）：逐日表格（含总计行）+ 价格表时间 / 未定价 / 候选路径全部落在 `#token-usage-panel`（点主界面 `#token-dashboard` 向上弹出）；设置面板那一节只留「打开价格表 / 更新价格」与候选价格确认。**别把这些数字搬回设置**：设置打开时看不见对话，而这些数字要在对话进行中刷新（`updateTokenDashboard` → `refreshTokenUsagePanel`，单飞 + 追最新）。面板里**只放数据不放说明文字**（用户原话「只是移入数据，并不是移入说明」）。
    - **算法只许一份，总计行由它算出来**：金额 / 合计 / 格式化只有 [usage-cost.ts](file:///d:/cc/claude-code-cli-master/app/src/usage-cost.ts) 一份实现，两个宿主都 import 它；逐日表格的 `<tfoot>` 总计行取 `sumUsageCost()`（**禁止**在渲染时把已渲染的行再加一遍 —— 那是同一个值的第二份实现，迟早会与逐日行对不上）。

64. **会话 id 与 rewind（A11，2026-09-20）**：契约与实测见 §3.5「会话 id 与 rewind」。`session_id` 从**恒为 `""`** 改成**真值**，回退点从「用户轮」扩到**任意消息**。**六条不得回退**：
    - **`session_id` 的语义是「一次 agent 运行」，不是「一段对话」**：形态 `sess_<pid>_<启动时刻 epoch 毫秒>`，`OnceLock` 进程内恒定（不引 chrono / uuid）。宿主每次重启 agent（换模型 / 换思考档 / 换工作区 / 回退时取消流式）就是**新会话**。**禁止**把它当会话持久化的键：对话仍由前端 `chatHistory` + `chat.db` 决定，上下文仍由 `set_history` 灌（规则 30）—— 这个 id **只负责归因**。
    - **带 session 归因的位置固定在 7 处 JSON 字段 + 1 行日志**（改动这些 emit 时不要退回 `""`）：`system/init`、成功 `result`、`finish_error` 的 `result`、启动期错误的 `result`、`hook_blocked` 的 `result`、`hook_tool_payload`、`fire_plain_hook`；另加启动日志行 `[agent] P1–P4 就绪 session=…`。前端在 `system/init` 分支捕获它（与 `model` 同一处），随用量记录落进 `usage-YYYY-MM-DD.jsonl` 的 `sessionId` 字段（**`serde(default)`，旧记录读成空串，空值不写回**）。
    - **`task_id` 仍是短计数**（`task-1` / `skill-1`）：它进模型可见的回灌文本，短才好读，**不许**塞 session 前缀（规则 54 约束②）。
    - **回退点 = 任意消息**：用户与助手气泡上都挂回退按钮（「保留到这里、丢掉其后」），实时对话的助手回复在**回合页脚**（`.turn-rollback`）上给入口。**重试仍只对用户提问成立**（要重发的是用户那句话）。
    - **回退只动对话与上下文，不动磁盘**：**没有**文件内容历史快照，也不打算有 —— 界面文案（`.msg-rollback` 的 title 与回退后的状态行）必须如实写明「磁盘上已改动的文件不会还原」。回退**不可撤销**：裁剪后的会话立刻全删全插写回 `chat.db`，被丢掉的那段没有第二份（旧的 `lunac-rollback-snapshots` localStorage 备份**全仓无读取方**，A11 已删 —— 不要以「可恢复」为由把它加回来）。
    - **`data-idx` 必须跟着 `pruneContext` 前移**（A11 修的既有 bug）：气泡下标是「渲染那一刻的下标」，而 `pruneContext` 每回合从**队首**丢消息 ⇒ 丢 N 条后所有老气泡的下标都偏大 N，回退会**切错消息**、复制会**复制错**。落地形态 = `shiftRenderedMsgIdx(dropped)`，并摘掉已被丢掉气泡（新下标 < 0）的回退 / 重试按钮（消息已不在会话里，回退过去只会切错位置）。**新增任何「把下标烘进 DOM」的渲染点，都要同时接上这个换算。**

65. **一轮多子任务并行（A14，2026-09-20，原 backlog §8.4）**：契约与实测见 §3.5「子代理」的「子代理并行批」与「实测（A14 并发）」两行。**六条不得回退**：
    - **并发只发生在连续段内，且只对子代理族**：`plan_tool_batches()` 把一轮调用切成 `Serial` / `ReadOnly` / `Subagent` 三类批，连续的只读调用成 `ReadOnly`（上限 `TOOL_PARALLELISM = 4`）、**连续的** `Agent` / fork 技能成 `Subagent`（上限 `SUBAGENT_PARALLELISM = 3`），其余各自成批。子代理**可能写文件**（`Write` / `Bash`）⇒ 与前后调用之间存在真实的先后依赖，**两种并行批都不许被对方或写类调用跨过**（跨过去就是「写 A → 读 A」被重排成「读 A（旧）→ 写 A」这类无声错误）。单元素段一律 `Serial` —— 省一次 spawn，行为与串行逐字节一致。
    - **判定只做一次**：`subagent_call(name, input, skills)` 是唯一判定点（`Agent`；`Skill` 仅在入参指向 `context: fork` 的技能时算，inline 技能与 `Read` 同级），结果存进 `subagent_kinds` 供规划与执行**共用**。**禁止**在批循环里再写第二处 `if name == "Agent"` —— 那正是「规划说串行、执行走并行」的漂移源。执行侧同理收口在 `run_subagent_call()`。
    - **每线程一份 `cfg.detached()`**：`Cfg` 含 `Cell`（思考形态 / 实测体积缓存）⇒ **`!Sync`**，`&Cfg` 过不了线程边界。必须在 **spawn 之前、主线程上**建好每份 `Cfg`（建 client 要 `&Cfg`）；`detached()` 复制的是**当前已跑通并缓存**的思考形态，子代理因此不必重走 400 降级链。建不出来（极罕见）时**整批退回串行**用主 `cfg` 跑完 —— **宁可慢，不许把调用静默丢成空结果**。
    - **结果按下标回填 ⇒ 回灌顺序 = `tool_use` 原序**：`slots[i]` 回填 + 批区间无缝覆盖（规则 23 的同一条纪律），并发的完成先后**不得**影响回灌顺序；worker panic 也按下标写一条 `is_error` 结果，不留空洞。
    - **审批带归属；合并不许跨任务；审批通道不许改成主线程轮询**：子代理内部发出的 `control_request` 在 `request.task_id` 写明属于哪个子任务，主循环自己发起的调用**不写该键**（前端据此标注归属）。前端的命令合并（`findLastCmdGroup(taskId)`）必须比对 task id —— 否则两个子任务的命令并进同一行，「允许」一次放行两个任务、而标注只能显示其中一个。**死锁红线**：审批由**独立的 stdin 线程**按 `request_id` 投递（`route_control_response` + `pending_approvals`），所以主线程阻塞在 `thread::scope` 里也照样收发；**不要**把审批改成在主线程上轮询，那会在多子代理同时等审批时直接锁死。
    - **前端按 `task_id` 归组，且不动总体视窗**：三个 `task_*` 事件按 `task_id` 渲染成一块「子任务」面板（每个子任务一行、就地更新），**不做左右分栏** —— 分栏要改总体视窗宽度、还会在列内引入第二层滚动条，两条都是明令禁止的（agent-ui-spec §0 规则 1、规则 44）。面板**不给** `max-height` / `overflow`（高度跟内容长，滚动交给外层）。状态行在 `task_done` 后**还有别的子任务在跑就继续报并行数**，全跑完才回「工作中」——一个 `task_done` 就说「工作中」会让用户以为剩下的也结束了。

66. **用户人格 / 自定义提示词（L2，2026-09-21，原 backlog L2）**：契约与实测见 §3.5「人格 / 自定义提示词」。一句话形态：**把「人格」从写死的 Rust 常量变成用户在设置里可编辑的一段文本，但它仍然落在系统提示词的固定前缀里**。**六条不得回退**：
    - **落点只能是系统提示词的固定段**（内置人格段之后、`Environment:` 之前），**绝不按消息拼接**（那会让提示词每轮都变、把整个固定前缀打掉 —— 规则 23 的实测：那两块固定文案占首请求未命中的约 90%），**也绝不用 hooks 承载**（hooks 是**控制通道**：放行 / 拒绝 / 提示行，它的输出今天不进模型，只有 `PostToolUse` 能拼进**已有** `tool_result` 的文本内部；拿它当文本注入通道等于给同一机制加第二个职责，还会多出一条权限面「谁能写 `hooks.json` 谁就能改系统指令」）。
    - **启动时读一次**（`read_user_persona()` 在 `main()` 装配处调一次），进程内**逐字节不变** —— 这是规则 18 的前缀缓存不变量，「可配置」不等于「每轮可变」。代价是**改完必须重启 agent 才生效**：面板必须**如实写明**并给「立即重启 AI」按钮（复用 `window.__lunac_reload_agent`，不要另造重启通道）。**不要**以「hooks 是热重载的」为由把这段也做成热重载 —— 提示词不是配置，它是请求体的一部分。
    - **「没配」必须等于「一个字节都不加」**：环境变量没给 / 文件不存在 / 读不出来 ⇒ 空串 ⇒ `persona_block()` 返回空串。**别加空行、别加表头、别用占位文案** —— e2e 的判据就是「空文件时的 `system` 前缀哈希与未引入本功能前**完全相同**」。空文本是合法状态（面板的「恢复内置」写的就是空文本，不是删文件、也不是切开关）。
    - **只进主提示词，刻意不进子代理与后台复盘**：`build_subagent_system()` / `build_review_system()` 保持「角色段 + 环境块 + 技能清单」。理由是那两者是内部产物（报告 / 写记忆），用户文风在那里没有意义，而多出的文本是**每个并发子代理**都要重发一次的固定成本。守门单测 `user_persona_reaches_only_the_main_prompt` 同时钉住「逐字在场 / 位置正确 / 花括号按字面量走（不进 `format!` 求值） / 只进主提示词」四件事。
    - **长度上限 8000 字符，两侧同值**（`core-agent` 的 `MAX_PERSONA_CHARS` 与宿主 `storage::MAX_PERSONA_CHARS`）：宿主**保存前**硬校验（超长 / 含 NUL ⇒ 拒收且一个字都不写），agent 侧读到超长只**截断 + warn**。为什么要上限：这段是**每次请求都要发**的固定前缀，塞长文等于给每一轮都加一笔固定成本 —— 它不是「用户想写多少就写多少」的地方（用户绕过面板直接编辑文件是这条截断兜的场景）。
    - **落盘是 `config\persona.md` 纯文本**（与 `ai.json` / `hotkey.json` / `hooks.json` / `pricing.json` 同级）：后台配置走 `config\`、业务数据走 `ModuleData\`，这条分工不许破例；宿主只在 spawn 时**无条件**注入 `LUNAC_PERSONA_FILE`（同 `LUNAC_HOOKS_FILE` 的先例：「文件不在 = 没配」由一处判定，宿主不替它判断）。
    - **UI 落点是输入栏「更多设置」的 ⋯ 菜单，不在设置面板（2026-09-21，用户明确要求）**：它是**进阶 / 一次性**配置，占设置面板一整块不值当 —— 设置面板那边整块撤掉，textarea 与保存 / 恢复 / 重启三个按钮搬进 ⋯ 菜单（`#chat-persona-*`，惰性装载：展开时才 `get_persona`，进程内只装一次以免冲掉没保存的草稿），提示压成**一句**（上限 + 重启生效 + 不进子代理）。**契约一个字都没变**（落盘路径 / 启动读一次 / 8000 上限 / 不进子代理），变的只是入口。

67. **第三方插件市场（L1，2026-09-21，原 backlog L1 本体）**：契约与实测见 §3.5「插件市场」。形态：**`<exe 根>\plugins\<id>\` 下的包在启动时被注册进同一个 `pluginRegistry`**（于是结果区 / 拼音 / 图标 / i18n 零改动），安装走 **https 的 zip**（`lunac-plugin.json` + 已编译的 ESM 入口）；面板上**没装的条目**来自本仓 `plugins/index.json` 索引（宿主去拉并逐条再筛）。**八条不得回退**：
    - **加载通道只能是 asset 协议**：`convertFileSrc(entry)` → `import(/* @vite-ignore */ url)`。CSP 的 `script-src` **必须含 `https://asset.localhost`**（2026-09-21 加）；**永远不能走 CDN**（同 §3.7 的 KaTeX 缺陷）。这条链路的两个前提是**核实过 Tauri 源码**的，不是猜的：`.js` / `.mjs` 在 asset 协议下是 `text/javascript`（`tauri-utils` 的 `mime_type.rs`），asset 响应一律带 `Access-Control-Allow-Origin: <窗口 origin>`（`tauri/src/protocol/asset.rs`）。
    - **校验比 tools / skills 先例更严，因为解压的是可执行代码**：只收 https；压缩包与**解压后总量**都有上限 + 条目数上限（zip bomb）；路径穿越的判据收口在纯函数 `safe_join()`（拒绝对路径 / `..` / 空段 / 深路径 / 以点或空格结尾的分段）并有单测；`id` 只允许 `[a-z0-9._-]`；`entry` 必须相对且 `.js` / `.mjs`；**同 id 已存在 ⇒ 拒绝**（不静默覆盖）；先解到 `.staging-*` 再改名，失败即清理。
    - **安全边界写实话**：这是**防事故**，不是防恶意 —— 插件是用户自己装的代码，装上即等同一份本机权限。**禁止**把这条描述成「已沙箱化」（同 §4.1 的诚实原则）。
    - **坏包必须可见**：清单坏 / 入口丢的包用不了（不进 registry），但**要列出来并写出原因** —— 否则用户只看到插件莫名消失。
    - **生效语义 = 立即生效**：`refreshMarketPlugins()` 先 `unregister` 再 `register`（registry 不去重，二次 register 会让结果区出现两行）；**不要**去抄「技能改完要重启 agent」那条 —— 技能是 agent 的能力，插件是前端的界面件。
    - **安装包零第三方模型资产**：Lunac 只给引擎与导入通道，模型由终端用户自备。官方样例模型属 **No Redistribution**（见 backlog L1-B 的四条线），**不得**随包分发。
    - **设置面板里必须写清两件事**：这是**可执行代码**、装它等于在本机运行它（只装信任来源）；以及插件目录在哪。别让用户以为插件只是个配置项。
    - **索引是「数据」，不是「指令」**（2026-09-21 二次改版加）：`plugins/index.json` 只提供 `url` 与展示用的文字，**判据一律取本机事实** —— 「已装 / 未装 / 坏了」由 `pluginRegistry` + 插件目录扫描说话，**不许**用索引自称的字段去决定。索引**由宿主拉**（前端 CSP 拦得住，见 §3.5）且**逐条再筛**（`id` 过 `is_safe_id` / `url` 只收 https / `name` 不能空，收口在纯函数 `parse_index()` 并有单测），坏条目**只丢自己**（`warn` 留痕）；索引地址是 **Rust 常量**，不由前端传。界面上的按钮**一律走事件委托**（在列表容器上绑一次，重绘只改 `innerHTML`）—— 2026-09-21 用户报的「卸载点了没反应」根因就是「监听只在重绘里绑、挂载时没人调那次重绘」，那条写法不得回退。

68. **宿主命令里不许有阻塞 IO（2026-09-22，「release / dev 频繁应用未响应」的根因）**：Tauri 的**非 `async` 命令跑在主线程上**（窗口消息泵就在那儿），命令体里任何等待都等于冻住整个窗口 —— 用户看到的就是 Windows 那句「应用程序未响应」。**六条不得回退**：
    - **判据是「命令体里有没有等待」，不是「平时快不快」**：`reqwest::blocking`、`Command::output()` / `schtasks`、图标提取、OCR、大文件读写一律算。落地形态统一为「`#[tauri::command] pub async fn foo(...)` 薄壳 + `run_blocking(move || foo_blocking(...)).await`」，`run_blocking` 定义在 `commands.rs` 顶部（`tauri::async_runtime::spawn_blocking` + 错误包装）。**不要**改用 `#[tauri::command(async)]` 顶替 —— 那只把阻塞体丢到 tokio 的 worker 线程（默认数量 = 核数），一个挂 20s 的 `reqwest::blocking` 会长期占住一个 worker，其余异步命令跟着排队。
    - **实测（2026-09-22，dev 实例 + CDP 探针，同一台机）**：`raw.githubusercontent.com` 不可达时 `fetch_plugin_index` 单次阻塞 **19783ms**，同一时刻页面心跳（每 150ms 一次最轻命令 `plugins_dir_path`）延迟 **19675ms** —— **命令的阻塞时长 = 窗口的阻塞时长**（另两次同口径采样：fetch 770ms ⇒ 心跳卡 606ms；fetch 360ms ⇒ 心跳卡 360ms）。而设置面板**每次打开**都会拉一次市场索引（`settings.ts` 的 `wirePluginMarket()` → `renderMarket()`）⇒「开设置 = 卡 20s」，这就是「频繁未响应」的来源。改后同一场景：fetch 3752ms 期间心跳**无一次 > 120ms**。
    - **本次一并搬走的同类命令**：`install_plugin_from_url`（120s 超时）、`install_skill_from_url`（20s）、`download_tool_from_url`（15s）、`get_auto_start_info`（spawn `schtasks`，实测 112–261ms，设置面板一开就调两次）、`get_app_icon`（实测 26–272ms，**结果区每行都调一次**）、`run_paddle_ocr`（子进程 + 等待）。`get_file_thumbnail` / `search_files` / `read_clipboard_files` 等原先已经是 `#[tauri::command(async)]` 的保持不动。
    - **搬出主线程后要让「共享资源」自己排队**：主线程天然串行的东西，搬走后会变成真并发，于是暴露出原本被串行掩盖的竞态。本仓实测（2026-09-22）：`get_app_icon` 搬走当天，**新进程里第一批并发图标请求只回来 1/6**（同一批之后再跑 4 次都是 6/6），而搬走前的同步版本新进程第一批就是 6/6 ⇒ 是并发踩的，不是路径的问题。修法**不是退回主线程**，而是在共享资源的**唯一入口**加进程内串行闸（`icon_extractor::ICON_LOCK`，同时管住 `get_app_icon` 与 `get_file_thumbnail` 的图标回落）—— 请求在阻塞池里排队，主线程照样不被占。同类资源（GDI / 剪贴板 / 单实例句柄）搬之前先问一句「这东西并发调用安全吗」。
    - **唯一的例外要写明理由**：`run_ocr` 是 WinRT（`Windows.Media.Ocr`）调用，WinRT 的激活与 `IAsyncOperation::get()` 要求调用线程**先初始化过套间**，而 windows 0.58 没提供 `initialize_mta()` ⇒ 它留在主线程（且前端当前不调它，图片识别走 `run_paddle_ocr`）。**新增例外必须在命令头注释里写清「为什么不能搬」，否则按本条处理。**
    - **回归口径**：同一场景下用探针看**页面心跳无一次 > 120ms**（探针口径见 §9.1 与本条实测行）。只跑 `cargo test` 绿**不算**通过 —— 单测跑在测试线程里，天然看不见「主线程被冻住」。

69. **翻译插件（2026-09-29）**：契约与实测见 §4.9。用户要的是「**词典做底座 + 模型补漏 + 译文存数据库（避免二次翻译）**」。**八条不得回退**：
    - **免 key 源必须实测选，不许照记忆写**：Google 的免 key 端点在本机**不通**（curl 000）⇒ 不实现，写成「首选 + 失败降级」只会让每次查询先白等一个超时。当前实现是 MyMemory（译文）+ 有道 jsonapi（词条详情）。**加新源之前先跑一次真机 `curl`**，并把结果写回 §4.9 那张表。
    - **源语言判定在本地做**，不用对方的 `Autodetect`（实测不可靠：`hello world` 被原样返回）。`detect_lang()` 只判「含不含 CJK」，用户显式选了源语言就不猜。
    - **`translate_lookup` 除空输入 / 超长外永不 Err**：网络故障、被反爬、额度用尽都只是「这次没查到」⇒ 返回 `source: "none"`。**禁止**把它变成一行红色错误 —— 那会把「对方今天不通」说成「你这个词有问题」。HTTP 200 + 正文极短的判据同预检 #41（**别去改解析正则**）。
    - **花钱的动作只能由用户点**：`translate_ai` 只挂在结果出来后那两个按钮上（查不到→「用 AI 翻译」，查到了→「用 AI 重新翻译」），**不许**做成「词典没有就自动问模型」。
    - **模型报错只给「用户能做什么」**：`401/403`→凭据、`404`→端点或模型名、`429`→太频繁、`5xx`→稍后再试；**状态码与响应体只进日志**（预检 #40 ③）。
    - **缓存键含语言对、且只缓存有内容的结果**：`"源语言\u{1}目标语言\u{1}原文"` —— 只按原文做键会把 `hello` 的英译中结果当成它的中译英原文查回来。查不到的**不写缓存**：写进去等于把一个还没收录的词永久钉死成查不到。
    - **模型补漏用的是 agent 的凭据与端点**（`commands::ai_credentials()` / `commands::agent_endpoint()`，两处都是 `pub(crate)` 共用）—— **禁止**在 translate.rs 里自己再读一遍 `config\ai.json` 或自己拼 URL，那会造出第二份「AI 配置是什么」的真相（2026-09-15 那次 401 的成因）。
    - **每次打开都按搜索栏那串查询重建面板**（`main.ts` 的 `skipRestore` 里必须留着 `translate`）：面板带着「待译内容」，若随别家一起恢复缓存 HTML，这次的查询会被悄悄丢掉 —— 第一次点开有预填、第二次没有。预填写进输入框前要剥掉开头的调用词（整串就是调用词 ⇒ 空；`translator` 这类前缀**不是**调用词 ⇒ 原样留）。

## 12. Agent Plan 模式规范

*来源：Hermes Agent 的 `plan` SKILL.md（MIT 协议，obra/superpowers 贡献），经适配整合。*

> **2026-09-20 落地（原 backlog A7）**：本节是**写给模型看的计划规范**（计划长什么样、任务切多细）；**怎么让它成为硬约束**是 A7 —— 见 §3.5「计划模式闭环」与 §11 规则 59。两处分工：本节管**文档写法**，那两处管**相位状态机、执行侧拦截与前端计划卡**。曾列在同一 backlog 条目里的 `VerifyPlanExecution` **不做**（验证交给 `TodoWrite`）。

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

## 13. 参考设计（原样设想；**13.1 已按自己的形态落地**）

> **2026-09-19 压缩说明**：本节原先用「模块清单 + API 表」的写法描述 Hermes 整合来的两个模块，读起来像已经实现 —— 但那些路径都指向 `core/`，而 **`core/` 是被 `.gitignore` 排除的旧 CLI 参考源码，不在仓库里**（`git clone` 下来不会有；仓库里只有 `app/`（前端 + src-tauri）、`core-agent/`（自研 agent 后端）、`vscode-extension/`、`scripts/`、`docs/`、`agent-templates/`）。原表里的每个路径都是「设想中的落点」，**照它去找一定找不到**。
>
> **2026-09-20 更正**：13.1 已经落地（2026-09-20，原 backlog A6），但**落点是自选的** —— 名字、路径、规则集都与旧设想不同，唯一真相源是 `core-agent/src/content_safety.rs`。13.2 仍然是「未落地也不打算做」。

### 13.1 文件内容级静态安全扫描（**已落地** 2026-09-20）

来源：Anthropic `claude-plugins-official`（Apache 2.0）经 Hermes 中继。原设想是「FileWrite/FileEdit 落盘后 `scanContent(content, path)` → 命中的 warnings 注入下一轮上下文」，规则集覆盖代码注入 / XSS / 反序列化 / 加密缺陷 / CI 注入等 25 条。**参照物与规则集都不可得** —— 那个 `scanContent()` 在 `core/` 里，而 `core/` 不在仓库（见本节开头的压缩说明），所以规则集是自定的。

**落地的形态（唯一真相源：`core-agent/src/content_safety.rs`）**：

- **时机是「写入前」，不是「落盘后」**。落盘后再报只能事后告知，而用户的决策点就在审批卡上；因此它挂在 `can_use_tool` 的 `open_approval()` 里，扫的是**将要写进磁盘的文本**（`Write` 取 `content`、`Edit` 取 `new_string` —— **不取 `old_string`**：那是要被删掉的内容，扫它会把「正在清理凭据」的操作也标成可疑，正好反了）。
- **只做「凭据 / 密钥泄漏」这一类**（2026-09-20 与用户定稿）。刻意**不做**「代码注入 / XSS / 反序列化」：正则做不到语义级判定，在正常代码里必然满屏误报，最后的结果是用户学会无视它 —— 那比没有更糟。
- **规则集 12 条，分三类**：① 形状唯一、零误报（`-----BEGIN … PRIVATE KEY-----`）；② 固定前缀的 API key（AWS / GitHub / OpenAI / Anthropic / Google / Slack / Stripe）；③ 自带结构的凭据（JWT、`Bearer`/`Basic` 头、带口令的连接串、以及两条「键名 + 赋值 + 非占位符值」的通用赋值规则）。
- **成本与误报的闸门**：`RegexSet` 先跑一遍（绝大多数内容一个 pattern 都不命中 ⇒ 零额外成本返回）；通用赋值规则要**同时**满足「键名含 key/secret/token/passwd/password/credential + 赋值到行尾 + 值不是占位符」三道闸；单次最多**报** 5 条、每条规则最多**采集** 200 条、超 512 KB 只扫前段并在文案里**如实上报**「只扫了前 512 KB」。
- **展示通道**：审批卡的 `analysis.secrets`（`[{rule, line}]`，与危险命令的 `analysis.dangerous` 同一字段家族，见 §11 规则 26 与规则 58）。前端 `classifyRequest()` 把它当**「必须人看」**这一档：不自动放行、不给「始终允许」、任何档位（含「自动」）都弹卡，并在卡片正文里**可见地**列出命中项（不是只放 tooltip）。
- **两个方向别混**：`bash_safety.rs` 扫**命令**、在**执行前**判定；`content_safety.rs` 扫**文件内容**、在**写入前**判定。规则集互不通用。
- **仍未做的部分**：`tool_result` 侧的警告渲染层（`securityWarnings` 色块）—— 现在只有审批卡这一条通道，工具返回文本里只有一句给**模型**看的提示（`tools.rs` 的 `secret_note()`，见 §19.2）。

### 13.2 会话文件清理服务（设想：`track` / `quick` / `cleanupSession`）

来源：Hermes 的 `disk-cleanup` 插件（MIT）。设想是按 `test`/`temp`/`session`/`download` 四类生命周期策略自动清理会话产生的临时文件。

- **本项目实际的做法**：临时产物集中在 `<exe 根>\temp\`（`transStorage` / `tool-outputs` / `logs` / `*-index-cache.json` / `webview-data`），**卸载时由 NSIS 的 `NSIS_HOOK_POSTUNINSTALL` 整目录删除**（见 §11 规则 12）；`logs` 另有一条 7 天保留的清理（`log::purge_old`，`KEEP_DAYS=7`）。**没有**按分类的自动清理服务，也不需要 —— 便携式布局下「整个 temp 目录」就是清理单位。
- **文中的 `LUNAC_HOME` 不存在**（本项目是 exe 目录即根，没有独立 HOME 概念）。

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

> **2026-09-19 更正**：原文写「Step 2 的静态扫描已由 `core/security/index.ts` 的 `scanContent()` 自动执行」—— **该文件不存在**（见 §13 的说明）。

**实际存在的那部分对应关系**：Static 扫描在本项目里有**两个**等价物 —— ① 扫**命令**的 `core-agent/src/bash_safety.rs`（时机在**执行前**，产物是审批卡上的 dangerous / opaque 标记，见 §11 规则 26）；② 扫**文件内容**的 `core-agent/src/content_safety.rs`（时机在**写入前**，产物是审批卡的 `analysis.secrets`，见 §13.1 与规则 58）。**没有**任何「落盘后扫、再把 warnings 注入下一轮上下文」的环节。本工作流的其余环节（测试、构建、审查清单）照旧有效。

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

## 18. 插件发现架构（已落地，正文不在此）

Lunac 的插件加载**不是** Hermes 那种「双目录 + `register(ctx)` + `sys.modules` 手法」。真实形态：目录扫描（`<exe 根>\Modules\<id>\` + `lunac-plugin.json`）→ 已编译的 ESM 入口经 **asset 协议** `import()` → 插件自己导出 `attach` / `detach`。**契约见 §3.5「插件市场」，纪律见 §11 规则 67。**

---

## 19. Hermes 方法论的前端接入（**只剩两项没做完**）

> 本节原是一份「前端集成规范」，其中**已落地的三项**（上下文感知提示词注入 / Humanizer 按钮 / 会话清理）**已从正文删除** —— 规范正文只写「未做完什么」（§10）。它们的正文与纪律在 §11 规则 18 / 23（提示词注入与缓存）、§17（humanize 写作文法）、§13.2（临时产物清理）。

### 19.1 ❌ 调试阶段状态栏（**未做**）

原设计：按 `tool_result` 失败推进 `debugPhase`（根因 → 模式 → 假设 → 实现），连续 3 次修复失败后注入一条「停下来质疑架构」的消息。

- **「3 次修复失败警告」部分已落地**（2026-07-21，见 §11 规则 15 系列）。
- **分阶段调试状态栏未做**（全仓无 `debugPhase` / 阶段栏代码）⇒ [backlog](./agent-feature-backlog.md) **L4**。

### 19.2 ⚠️ `tool_result` 侧的安全告警渲染（**未做**，唯一缺口）

**扫描器与审批卡通道都已落地**：`core-agent/src/content_safety.rs` 扫写入内容，命中项经 `analysis.secrets` 进审批卡正文（明细行 + 卡片标题上的命中标记），见 §13.1 与 §11 规则 58。

**缺的是 `tool_result` 侧那块色块**：工具返回文本里现在只有一句给**模型**看的纯文本提示（`tools.rs` 的 `secret_note()`，也照常显示在工具卡上），前端**没有** `securityWarnings` 组件 ⇒ 工具卡上不会把命中项显式标出来。

---

## 20. 用户自定义 Agent 工具 / 技能

> **本节原为「双路径执行计划」，2026-09-19 收敛**：**两条路径都已落地** —— 路径 1 = MCP 桥接层（见 20.1，其中「仍未做的」见下），路径 2 = 前端插件市场（见 §3.5「插件市场」/ §11 规则 67，本节不再复述）。
> 原文里两类路径**一律作废**：① `core/...` 一类 —— `core/` 是**本机参考用的旧 CLI 源码**（被 `.gitignore` 排除、**不在仓库里**，`git clone` 下来不会有），所以凡以 `core/plugins/...` 为落点的写法都只表示「参考它的设计、在新宿主重建」，**不是可直接引用的仓库文件**；仓库里真正存在的是 `app/`（前端 + src-tauri）、`core-agent/`（自研 agent 后端）、`vscode-extension/`、`scripts/`、`docs/`、`agent-templates/`；② `%LOCALAPPDATA%\Lunac\...` —— 本项目是**便携式**的，用户资产根 = **exe 所在目录**。若在别处见到 `%LOCALAPPDATA%\Lunac`，它只出现在「删除旧版本遗留目录」的卸载清理里（§11 规则 12），**不是运行时路径**。

### 20.1 ✅ 路径 1：声明式用户自定义工具（MCP 桥接层）—— **已落地**

真实落点（**唯一真相源，照这个找代码**）：

| 环节 | 落点 |
|---|---|
| 工具定义 | `<exe 根>\tools\*.json`（一份文件一个工具：`name` / `description` / `inputSchema` / `handler`） |
| 执行引擎 | `app/src-tauri/src/mcp_server.rs`（`lunac.exe --mcp-server`，stdio） |
| agent 侧 client | `core-agent/src/mcp.rs`（`initialize` / `tools/list` / `tools/call` / `resources/list` / `resources/read`） |
| 前端管理界面 | **tool-editor 插件**（增删改 + 保存后自动重启 agent）；设置 ·「AI → 工具 (MCP)」列已安装项与安装入口 |
| 接入规则 | 工具名一律 `mcp__<原名>`；**按名排序**后进请求体（§11 规则 18）；**任何档位下调用都先弹审批卡**；只读档（`plan`）**不接入** |
| resources 读侧 | `ListMcpResourcesTool` / `ReadMcpResourceTool`（2026-09-20 落地，**条件注册**）—— 让模型能看到用户工具的 `handler`。契约见 §3.5「MCP resources 读侧」、纪律见 §11 规则 55 |

**仍未做的**：远程传输（sse / http / ws）、prompts / roots / elicitation / OAuth、`.mcp.json` ⇒ [backlog](./agent-feature-backlog.md) **A13**。

**技能（`skills\`）**：inline 模式已落地（`LUNAC_SKILLS_DIR` 下 `<key>/SKILL.md`，渐进披露 + `$ARGUMENTS` 替换）；**fork 模式 2026-09-20 已落地**（frontmatter `context: fork` + `allowed-tools:`，走 `run_subagent()`，见 §3.5 与 §11 规则 57）；**remote 已定论不移植**（旧 CLI 源码在磁盘上不存在 + 依赖 `akiBackend`）；**技能自带脚本 / 资源已落地**（调用 `Skill` 时随返回值一并附上）。格式与生效方式见 [agent-implementation.md](./agent-implementation.md) §5。

### 20.2 ✅ 路径 2：插件市场（**已落地**）

前端插件市场（`Modules\` 扫盘 + `lunac-plugin.json` + asset 协议 `import()` + 市场索引与 zip 安装）：**正文见 §3.5「插件市场」、纪律见 §11 规则 67**，本节不再复述。三条硬约束（CSP `script-src 'self'` / Cubism 资产授权 / 常驻画布开销）在讨论**桌宠**时仍然适用 —— 那一条还挂在 [backlog](./agent-feature-backlog.md) **L1**。

**另一条社区扩展形态**（一直可用、与插件市场并存）：20.1 的 MCP 桥 + `agent-templates/`（`skills/` 与 `tools/` 各一份 `.example` 模板），用户手工放进 `<exe 根>\skills\` / `tools\`。


