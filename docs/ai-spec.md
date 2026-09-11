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

## 1. 架构概述

Lunac 是一个 **uTools 风格的桌面启动器 / 搜索工具**，由 Tauri 2.x 驱动。它从原有的 Claude Code CLI 聊天界面重构而来，现分为三个层次：

```
┌──────────────────────────────────────────────────────────────────┐
│                  Lunac 前端 (app/src/)                             │
│  main.ts → 毛玻璃搜索栏 + 插件系统 + 键盘导航                       │
│  plugins/builtin/ → quick-launch / settings /                   │
│                     ai-agent / ocr 等 7 个插件           │
│  tools/*.json → Agent MCP Tools (sys_info 等)     │
└────────────────────────────┬─────────────────────────────────────┘
                             │ Tauri IPC + WebView2
┌────────────────────────────▼─────────────────────────────────────┐
│                  Lunac 后端 (app/src-tauri/)                       │
│  main.rs         → 窗口管理 / 系统托盘 / 子进程生命周期            │
│  hotkey.rs       → 原生 Win32 LL钩子 (Alt+Space 全局热键)         │
│  commands.rs     → 子进程启动 / IPC 命令 / Start Menu 扫描        │
│  proxy_server.rs → 内置协议代理 (Anthropic↔OpenAI, 已停用)         │
│  mcp_server.rs   → MCP stdio 服务器 (Agent Tools 执行引擎)        │
└────────────────────────────┬─────────────────────────────────────┘
                             │ 子进程 (spawn)
┌────────────────────────────▼─────────────────────────────────────┐
│               core-agent/ 自研 Agent 后端 (agent.exe)              │
│  agent.exe → lunac 自带，stream-json 模式，后台运行                │
└──────────────────────────────────────────────────────────────────┘
```

## 2. 当前活跃功能

### 2.1 前端 — 搜索启动器

| 组件 | 文件 | 说明 |
|------|------|------|
| 主入口 | `app/src/main.ts` | 搜索栏 UI + 插件搜索 + 键盘事件 |
| 插件注册 | `app/src/plugins/registry.ts` | 关键词模糊匹配 + 评分排序 |
| 插件列表 | `app/src/plugins/builtin/index.ts` | 注册所有 7 个内置插件 |
| 快速启动 | `builtin/quick-launch.ts` | Start Menu 应用搜索与启动 |
| 网页搜索 | `builtin/web-search.ts` | 默认浏览器 Google 搜索 |
| 剪贴板历史 | `builtin/clipboard-history.ts` | 剪贴板历史管理 — 自动保存复制内容 |
| 设置面板 | `builtin/settings.ts` | 快捷键绑定 / 模型配置 |
| 工具编辑器 | `builtin/tool-editor.ts` | MCP tool JSON 编辑管理 |
| AI 代理 | `builtin/ai-agent.ts` | Agent 对话 — 委托 `main.ts` 启动 agent.exe 子进程 + cli-output 事件渲染 |
| OCR 识别 | `builtin/ocr.ts` | 离线 OCR 图片文字识别 (PaddleOCR-json · PP-OCRv4 · 中/英/日/韩/俄) |
| 样式 | `app/src/styles.css` | 毛玻璃 Catppuccin 主题 |
| 国际化 | `app/src/i18n.ts` | 多语言翻译模块 — 跟随 Windows 系统语言 |

### 2.1.1 国际化 (i18n)

Lunac 自动检测 Windows 系统显示语言（`GetUserDefaultUILanguage`），启动时通过 Tauri IPC 获取 BCP-47 标签。所有 UI 字符串通过 `t(key, params?)` 函数动态翻译，支持 `{param}` 插值。

**支持语言**：zh-CN、zh-TW、ja、ko、en（默认回退）

**文件架构**：
| 层 | 文件 | 职责 |
|---|------|------|
| Rust 检测 | `commands.rs`:`get_system_language` | Win32 `GetUserDefaultUILanguage` → BCP-47 标签映射 |
| 翻译字典 | `app/src/i18n.ts` | 100+ 键的翻译表，覆盖搜索/状态/聊天/插件/OCR/设置 |
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

### 2.3 热键 — 双后端 + 三层兜底（2026-09 修订）

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
- **搜索/结果/空态**：`measurePanelHeight()` 用 `getBoundingClientRect().bottom` 实测 搜索栏/结果区/状态栏 最底者——该 API 返回**含 CSS zoom 的视觉尺寸**（WebView2 下=逻辑像素 DIP），直接喂 `LogicalSize`，无需再乘 zoom。`#app` 为 `height:auto`（内容驱动），`#app.plugin-active`/`.detached` 才 `height:100%`（插件固定高窗口，面板内部滚动）。
- **插件模式（固定高度 × zoom = DIPs）**：独立插件窗口（detached）600px、OCR detached 520px、内嵌插件视图 360px。
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

- **对话模式**：应用始终运行 Agent 进程（`ai_mode` 恒为 "agent"；`set_ai_mode` 拒绝其他值），但提供**对话级模式切换**：
  - **Agent 模式**（默认）：完整工具链 + skills + 权限审批
  - **简单问答模式**：复用同一 agent.exe，发送消息时注入"直接回答、勿用工具/skills"软约束提示词（`buildSimpleChatHint`），替代已删除的 chat.rs 简单模式；前端按钮 `#chat-mode-btn` 切换，localStorage `lunac-chat-mode` 持久化
- **权限审批**：agent 发 `can_use_tool` control_request → 前端卡片（`showPermissionCard` + 内置安全前缀 / 白名单 / 危险命令黑名单）→ 回包前 agent 阻塞等待（P2 已接通，超时 300s 按拒绝）
- **工作区**：`AppState.workspace` 决定 agent.exe 的 cwd 与 `--add-dir`；为空回退用户主目录（整个系统可访问，敏感操作走 ask 弹卡）

### 3.3 技术实现 (`agent.exe` 直连)

- **直连**：`agent.exe` 直接连供应商原生 Anthropic 兼容端点（`AI_AGENT_URL` 或 `{base}/anthropic`），完整支持 tool_use
- **启动**：`start_cli` / `ensure_agent_running` → `ai_credentials()` + `configure_agent_env()` 注入 `LUNAC_AGENT_BASE_URL` / `LUNAC_AGENT_TOKEN` / `LUNAC_AGENT_MODEL` → spawn `agent.exe`（内置代理 `proxy_server.rs` 已停用 — 其 Anthropic→OpenAI 翻译会**丢弃 tools 数组**，导致模型无法输出 tool_use、退化为文本式 XML 工具调用）
- **通信**：`send_message` stdin 写入，stdout stream-json SSE 流式读取（`cli-output` 事件）
- **停止**：`stop_cli` → kill agent.exe
- **工作区**：`set_workspace(path)` canonicalize 校验目录后存入 `AppState.workspace`；`start_cli_process` 以其为 cwd + `--add-dir`（空值回退用户主目录 → 整个系统可访问，敏感操作仍走 ask 审批弹卡）；前端 localStorage `lunac-agent-workspace` 持久化，启动时恢复

### 3.4 前端接入

| 功能 | 文件 | 说明 |
|------|------|------|
| Agent 对话 | `main.ts` → `startAgentChat` → `start_cli` → `send_message` | cli-output 事件流式渲染 |
| 权限审批 | `main.ts` `showPermissionCard` | 多请求合并为单批处理卡片，pending 期间状态栏显示"对话已暂停"；白名单/安全前缀自动放行，危险命令只给手动确认（P2 已接通） |
| 工具卡片 | `main.ts` `agentNewBlock("tool")` / `agentToolArgsDelta` / `agentToolResult` | `content_block_start(tool_use)` + `input_json_delta` 流式展开参数，`tool_result` 内联成功/失败（P1 起真正生效） |
| 工作区设置 | `main.ts` AI 对话输入栏 `#chat-workspace-btn`（唯一入口） | 选择目录/重置 → `invoke("set_workspace")` + 重启 CLI；默认=用户主目录（整个系统可访问） |
| 简单问答切换 | `main.ts` `#chat-mode-btn` + `buildSimpleChatHint` | 软约束"勿用工具/skills"，状态栏显示当前模式 |
| Token 仪表盘 | `main.ts` `updateTokenDashboard` | 完整计费口径：Hit=缓存读取，Miss=普通输入+缓存写入，Total=四类 token 之和 |

### 3.5 自研 agent 核心 `core-agent/`（2026-09，已接线）

后端二进制 `agent.exe` 由本仓库自研（[core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs)），遵守下列 stream-json 契约。src-tauri 的 `start_cli` / `ensure_agent_running` / `start_agent_http` 统一经 `commands.rs` 的 `core_dir()` 定位二进制，按优先级：

1. `<exe_dir>\resources\agent.exe`（Tauri 打包资源，`bundle.resources` 用 map 形式平铺）
2. `<exe_dir>\agent.exe`（便携版 / NSIS 安装根，与 lunac.exe 同级）
3. dev：`<repo>\core-agent\target\{release,debug}\agent.exe`（cargo 产物）
4. 兜底 `<repo>\core`（历史目录）

**命名纪律**：自研侧不得再出现 `ANTHROPIC_*` / `CLAUDE_CODE_*` 环境变量；仅保留协议必需的 `anthropic-version` 请求头与供应商侧的 `/anthropic` 路由（外部协议名，改了就不通）。IPC 名 `start_cli` / `stop_cli` / `cli-output` / `cli-status` / `cli_bridge` **保持历史命名**（前端与本文档的既有契约，与二进制文件名无关）。

**契约（前端既有约定，不得改动）**

| 面 | 内容 |
|---|---|
| env | `LUNAC_AGENT_BASE_URL`（已是完整端点，请求拼 `/v1/messages`）、`LUNAC_AGENT_TOKEN`（**必须走 `authorization: Bearer`**；用 `x-api-key` 会被兼容端点判 401）、`LUNAC_AGENT_MODEL`；另有 `MAX_THINKING_TOKENS`（思考档位）、`LUNAC_MAX_CONTEXT_TOKENS`（上下文预算，默认 128000、低于 8000 的取值视为无效）、`LUNAC_SKILLS_DIR`、`LUNAC_WORKSPACE_LOCKED` |
| 启动参数 | `--add-dir <dir>`（可重复，工作区外追加可访问目录）/ `--permission-mode plan`（只读）/ `--dangerously-skip-permissions`（忽略工作区锁）/ `--permission-prompt-tool stdio`（写类工具先审批）/ `--disallowedTools <name…>`（这些工具不进请求体）/ `--mcp-server stdio:<exe 路径>`（拉起该 exe 的 MCP server 并接入其工具，P3）；其余（`--print` / `--verbose` / `--input-format stream-json` / `--include-partial-messages` …）一律接受并忽略 |
| stdin | 每行一条 JSON：`{"type":"user","session_id":"","message":{"role":"user","content":[{"type":"text","text":"…"}]},"parent_tool_use_id":null}`；`{"type":"control_response","response":{"subtype":"success","request_id":"…","response":{"behavior":"allow"\|"deny",…}}}` 为审批回包（P2，由 stdin 线程按 request_id 直接投递给等待中的工具调用） |
| stdout | 每行一条 JSON：`system/init`（含 `tools` 名单）→ `system/context_compacted`（`elided` / `dropped` 计数，压缩发生时补发）→ `stream_event`（`content_block_start` / `content_block_delta`(`text_delta`\|`thinking_delta`\|`input_json_delta`) / `content_block_stop` / `message_stop`）→ `assistant`（整包，含 `tool_use`，仅无增量时前端兜底）→ `control_request`（`can_use_tool`，写类工具执行前）→ `user`（整包，含 `tool_result`）→ `result`（`subtype` / `is_error` / `usage`，用量为整轮累计） |

**P0 已完成**：多轮上下文（进程内 history）、SSE 增量打字、用量上报（input/output/cache_read/cache_creation）、错误回传（失败轮按 `history.truncate(base)` 整体回滚，不污染后续对话）、stdin 读取线程与查询线程经 mpsc 解耦（为 P2 的 `control_response` 预留通路）。

**P1 已完成（2026-09，内置工具循环）**：六件工具实现在 [core-agent/src/tools.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/tools.rs)，主循环按「请求 → 流式收块 → 有 `tool_use` 就执行并以 `tool_result` 回灌 → 再请求」往返，直到模型不再调工具（上限 `MAX_TOOL_ROUNDS=16`，到顶后再给一次「只用文本收口」的机会）。

| 工具 | 入参 | 行为 |
|---|---|---|
| `Read` | `file_path` / `offset` / `limit` | 带 1-based 行号输出；单次 ≤2000 行、文件 ≤2MB |
| `Write` | `file_path` / `content` | 自动建父目录，整文件覆盖 |
| `Edit` | `file_path` / `old_string` / `new_string` / `replace_all` | 精确串替换；找不到、或多处匹配且未开 `replace_all` 时按错误返回 |
| `Bash` | `command` / `timeout` | `cmd /C` 执行（带 `CREATE_NO_WINDOW`，GUI 宿主下不闪黑框）；默认 120s、上限 600s，超时 kill；stdout+stderr 合并回传 |
| `Glob` | `pattern` / `path` | `**` 递归、`*` 不跨目录；≤200 条 |
| `Grep` | `pattern` / `path` / `glob` / `ignore_case` | Rust 正则逐行匹配，输出 `路径:行号:内容`；跳过 `.git`/`node_modules`/`target` 等重目录与二进制文件；≤200 条 |

**工具权限策略**

| 条件 | 效果 |
|---|---|
| `--permission-mode plan`（前端「安全」档） | 只读：`Write`/`Edit`/`Bash` 一律以 `is_error=true` 拒绝（不弹审批）；`Read`/`Glob`/`Grep` 可用 |
| `--permission-prompt-tool stdio` 且非 plan 档 | `Write`/`Edit`/`Bash` 执行前先发 `can_use_tool` 请前端审批；前端自行判定「内置安全前缀 / 白名单自动放行」还是「弹卡片」（危险命令永远只给手动确认）。没有这个开关就不问，避免对着无人应答的通道干等 |
| `LUNAC_WORKSPACE_LOCKED=1`（配置了工作区时 src-tauri 注入） | 文件类工具路径先做词法规范化（消 `..`），越出工作区（cwd / `--add-dir`）即拒绝 —— 含 `Read` 的越界读取。**审批通过也不放行**（这是硬边界） |
| MCP 工具（P3，见下） | 非 plan 档下**一律先发 `can_use_tool`**（handler 能跑 shell / 发 HTTP，且定义来自用户 JSON，agent 侧无权替用户判断）；plan 档压根不接入，模型看不到这些工具 |
| `--dangerously-skip-permissions`（前端「完全」档） | 忽略工作区锁 |

**P2 审批实现要点**：一批工具**先全部发请求、再逐个等回包**（前端才能把连续 Bash 合并成一行一次性决定，见 `findLastBashGroup`）；`updatedInput` 为非空对象时覆盖原参数，空对象表示按原参数执行；`behavior=deny` 转成 `is_error=true` 的 `tool_result` 交回模型（模型可改方案），回包带 `interrupt=true` 则本轮就此结束；等待上限 300s，超时按拒绝处理并回一条 `control_cancel_request` 让前端撤掉卡片。回包由 stdin 线程按 `request_id` 直接投递给等待中的调用，不进主消息队列。

⚠️ **档位仍不可切换**（`set_security_profile` 命令存在但前端无人调用），默认始终是「项目」档 —— 写操作「每次都要点一次同意」，但不再有静默执行。

**工具错误不中断整轮**：工具返回 Err 时转成 `is_error=true` 的 `tool_result` 交回模型自行纠正；只有 HTTP / 流错误才终止本轮并回滚 history。写回上下文的 assistant 消息会**剔除 thinking 块**（端点要求 thinking 带 `signature`，回灌会 400），发给前端的整包仍保留 thinking。

**思考档位跨模型自适应**（2026-09）：档位由 src-tauri 的 `MAX_THINKING_TOKENS` 传入（0=fast 不思考 / 8192=think / 32768=deep）。各供应商的 Anthropic 兼容端点对 `thinking` 字段接受度不同（DeepSeek 只认 `enabled`/`disabled`、原生 Messages 端点的新模型要 `adaptive`、Kimi 等兼容层可能完全不支持），故**不硬编码模型名单**，而是：

| 输入 | 首选形态 | `max_tokens` |
|---|---|---|
| `MAX_THINKING_TOKENS=0` | `{"type":"disabled"}` | 8192 基线 |
| `MAX_THINKING_TOKENS=n>0` | `{"type":"enabled","budget_tokens":n}` | `max(n+4096, 8192)` —— 端点要求 `budget_tokens < max_tokens`，deep 档 32768 配 8192 会被判非法 |
| 未设置 | 完全不发该字段 | 8192 基线 |

**400 降级链**（仅当错误正文含 `thinking`/`adaptive`/`budget_tokens` 才触发，避免把「模型名不存在」这类无关 400 也白重试）：`enabled+budget → adaptive → 不带字段`；fast 档为 `disabled → 不带字段`（**不退到 adaptive**，否则等于反过来把思考打开）。降级结果缓存在进程内，后续轮次不再试错，并往 stderr 打一行说明。

**上下文预算与压缩**（2026-09）：端点的上下文窗口是硬限制，超了就是 400，而失败轮会整体回滚 history —— 不管理体积的话对话会「越用越死」。预算取 `LUNAC_MAX_CONTEXT_TOKENS`（默认 128000，低于 8000 视为无效），**水位以端点实测值判断**（`message_start` 的 `input_tokens + cache_read + cache_creation`，跨轮保留在 `Cfg.last_input`），比按字符估算准。两级处理，都不额外调用模型：

| 档 | 触发 | 动作 |
|---|---|---|
| 瘦身 | 实测 > 预算 × 0.70 | 把较旧轮次里超 2000 字符的 `tool_result.content` 就地换成 `[elided: N chars dropped to save context]`（文件内容/Grep 结果是体积大头，价值递减），尾部 8 条不动 |
| 丢弃 | 实测 > 预算 × 0.90 | 强制模式下除瘦身外，再从最老处整条丢弃、只留尾部 8 条；**始终保留开头那条用户提问**（任务目标），且丢弃后首条不得是 `tool_result`（必须紧跟对应 `tool_use`），否则补一条 `TRIMMED_MARKER` 文本消息 |
| 400 兜底 | 端点回报的 400 正文含 `context`/`too long`/`input length` | 强制压缩一次后重试（每轮至多一次），兜住估算误差 |

压缩发生时往 stderr 与 stdout 各报一次（stdout 为 `system/context_compacted`，含 `elided` / `dropped` 计数）。**压缩会左移 `history`，调用方的失败回滚锚点 `base` 必须同步减去 `dropped`**，否则回滚会误删保留段。单条用户输入超 100000 字符先截断（防一次粘贴顶爆窗口）。

**P3 已完成（2026-09，MCP 工具桥）**：实现在 [core-agent/src/mcp.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/mcp.rs)。src-tauri 在 spawn 时把 lunac.exe 自己的路径交过来（`--mcp-server stdio:<路径>`），agent **作 client** 把该 exe 以 `--mcp-server` 拉起 —— 那个进程会拦截该参数、进 stdio MCP server 模式（实现在 [mcp_server.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/mcp_server.rs)），读 `<exe 根>\tools\*.json` 的用户自定义工具（handler 有 `shell` / `http` / `builtin` 三种）。

握手：`initialize` → `notifications/initialized` → `tools/list`（超时 15s）→ 模型调用时 `tools/call`（超时 180s）。要点：

| 项 | 约定 |
|---|---|
| 命名 | 一律 `mcp__<原名>`（前端审批卡的「始终允许」按完整名字记入 localStorage 白名单，**前缀必须稳定**）；原名含非字母数字/`-`/`_` 或超 64 字符时替换/截断为 `_`，重名追加 `_2` |
| 接入范围 | 非 plan 档启动时接一次；`--disallowedTools` 里出现原名或带前缀名即不接入；plan（只读）档**不连接**（接进来只会每次被拒，还让工具清单随档位漂移） |
| 审批 | MCP 工具一律先发 `can_use_tool`（见上表）；deny → `is_error=true` 的 `tool_result` |
| 错误 | 上游 `isError=true`、进程退出、超时都转成 `is_error=true` 的 `tool_result`，不中断整轮；结果按内置工具同一上限（30000 字符）截断 |
| 容错 | spawn/握手失败只往 stderr 记一行并继续 —— 六件内置工具必须照常可用；MCP server 的 stderr 直接并入 agent stderr（上游会转发到前端/终端），stdout 独占给 JSON-RPC |
| 生命周期 | agent.exe 退出时 kill 子进程（`Drop for Bridge`）；新增/改动 `tools\*.json` 后需重启 agent.exe 才生效（与工具黑名单同一套重启流程） |

**P4 待做**：skills / 系统提示词（`LUNAC_SKILLS_DIR`）；MCP 只做了动态工具代理，`ListMcpResourcesTool` / `ReadMcpResourceTool` 未做（server 侧只有 `resources/list`，没有 `resources/read`）。**与旧 cli.exe 的完整差距清单、价值评级与实施顺序见 [agent-feature-backlog.md](file:///d:/cc/claude-code-cli-master/docs/agent-feature-backlog.md)。**

**构建**：`powershell -ExecutionPolicy Bypass -File scripts\build-core.ps1`（等价 `cd core-agent; cargo build --release`）→ `core-agent\target\release\agent.exe`，约 2.5MB（P1 引入 glob/regex 后从 1.5MB 增长）。打包链路：`bundle.resources` 把它平铺成 `resources\agent.exe`，NSIS 由 `release\lunac-installer.nsi` 装到安装根；`build-release.ps1` 的 **[4/9]** 步必须在 Rust 构建之前跑，否则 resources 缺文件会打包失败。

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
- 工具参数实时显示**可读摘要**而非原始 JSON：Bash/PowerShell 显示 `command` 文本；其他工具显示 `key=value` 摘要（≤200 字符截断）。
- 工具结果成功时折叠为一行 `✓ 完成: <120字符摘要>`，点击 `<details>` 展开完整输出（≤600 字符）；失败显示 `✗` + 错误原因；⚠ 安全告警保留黄色高亮。
- 思考过程默认折叠为 `▸ 思考中…`，展开查看完整内容。
- 连续 3 次工具失败显示黄色警告条。

**C. 消息流交互细节**
- 流式文本带右侧光标闪烁指示；发送后输入框立即清空。
- 状态栏展示：就绪/运行中/AI·模式/热键提示/token 仪表盘（真实计费口径：Hit=缓存读、Miss=输入+缓存写、Total=四类之和）。
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

### 4.3 插件清单 (前端)

每个插件维护独立的状态，搜索界面作为总入口：

```
searchInput (总端口)
  │
  ├── Enter → executePlugin("calculator")
  │            └── closePluginView: 保存 calculator 状态 → pluginStates.set("calculator", ...)
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
| **历史持久化** | `localStorage` 存储 `ChatSession[]`（id, title, messages, createdAt），最多 50 条；ai-agent 仅输入关键词时展示历史列表；新对话自动保存上一会话 |

### 4.2 搜索匹配机制 (`registry.ts`)

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

### 4.3 插件清单（前端）

| ID | 图标 | 触发关键词 |
|----|------|-----------|
| quick-launch | 🚀 | open, launch, run, app, start, 打开, 启动, 运行 |
| settings | ⚙️ | settings, shortcut, hotkey, config, 设置, 快捷键, 配置 |
| clipboard-history | 📋 | clipboard, history, paste, 剪切板, 历史, 剪贴板, 粘贴 |
| web-search | 🌐 | search, google, baidu, bing, web, 搜索, 网页 |
| tool-editor | 🔧 | tool, tools, 工具, mcp, agent, skill, 插件, 扩展 |
| ai-agent | 🤖 | (被搜索过滤排除，仅作为无匹配时的回退显示) |
| hardware-ocr | 🔍 | ocr, 识别, 文字识别, 图像识别, 图片转文字, tesseract |

## 5. 关键设计决策

| 决策 | 原因 |
|------|------|
| **热键用原生 Win32 API** | `tauri-plugin-global-shortcut` 在 Windows 上不可靠；`RegisterHotKey` 是内核级 API |
| **前端不参与热键** | `hotkey.rs` 直接调用 `win.show()/hide()`，零前端依赖，避免 JS 线程延迟 |
| **`pointer-events: none` 策略** | 毛玻璃透明窗口在无结果时只占搜索栏高度，其余区域穿透点击 |
| **固定窗口 800×600** | 避免动态 resize 的竞态条件和不稳定性 |
| **搜索内容不随失焦清空** | `hideWindow()` 纯隐藏不重置，Esc 才清空 |
| **拖拽用 `startDragging()` API** | `data-tauri-drag-region` 在子元素(input/button)上不触发 |

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
│           └── ocr.ts            # OCR 文字识别 (PaddleOCR-json)
├── src-tauri/
│   ├── src/
│   │   ├── main.rs                  # Tauri 入口 — 窗口/托盘/子进程
│   │   ├── hotkey.rs                # 原生 Win32 热键
│   │   ├── commands.rs              # IPC 命令
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
├── src/tools.rs                     # P1 内置工具：Read / Write / Edit / Bash / Glob / Grep
├── src/mcp.rs                       # P3 MCP 工具桥（stdio client，连 lunac.exe --mcp-server）
├── src/skills.rs                    # P4 技能（LUNAC_SKILLS_DIR 的 <key>/SKILL.md + Skill 工具）
├── Cargo.toml
└── target/release/agent.exe         # 编译产物（cargo build --release，约 2.5MB，不入库）

scripts/
├── _env.ps1                         # 公共环境准备（把 cargo / mingw64\bin 追加进 PATH，供其它脚本 dot-source）
├── verify-git.ps1                   # 新克隆自检（npm run verify），退出码 0/1
├── commit.ps1                       # 一键提交（npm run commit）：暂存 → 敏感/超大文件检查 → 提交；-Push 才推送
├── build-core.ps1                   # 编译自研 agent 后端 core-agent → agent.exe（cargo build --release）
├── dev.ps1 / tauri-dev.ps1          # 开发启动（缺 agent.exe 时先自动构建）
├── build.ps1 / tauri-build.ps1      # 打包封装
└── download-paddle-ocr.ps1          # 预置离线 OCR 引擎
```

> 所有 ps1 脚本必须用 `$PSScriptRoot` / `Split-Path -Parent $PSScriptRoot` 推导仓库根，**禁止硬编码本机绝对路径**；统一包管理器为 `npm`。
> **ps1 含中文必须以 UTF-8 with BOM 保存** —— Windows PowerShell 5.1 对无 BOM 文件按 ANSI(GBK) 解码，中文字符会把紧随其后的引号/换行吞进双字节，导致「字符串缺少终止符」等解析错误（`download-paddle-ocr.ps1`、`build-release.ps1` 曾因此无法运行）。

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

仅在用户明确说"打包"或"生成安装包"时执行：

1. `powershell -File scripts\build-core.ps1` — 编译自研后端 `core-agent` → `agent.exe`（必须在第 2 步前，`bundle.resources` 会引用它）
2. `cd app && cargo build --release` — Rust 编译
3. `npm run build` — 前端编译
4. 复制 `lunac.exe` + `agent.exe` + `WebView2Loader.dll` → `release/Lunac/`
5. NSIS 编译 → `release/Lunac-0.x.0-Setup.exe`

**日常修改不打包** — 仅编译验证即可。

### 8.3 开源发布 / 仓库卫生（2026-09）

**当前状态：仓库为 private（`LythrumMoon/Lunac`），开发完成后才公开。** 公开前必须重跑下面的红线和首次提交验证；README 作为对外「详细页」不展示 CLI 相关实现细节。

**红线（违反会造成密钥泄露或侵权，且不可撤销）**：

1. **`core/` 绝不入库** —— `core/` 是上游 Claude Code 源码（`core/package.json` → `"name": "claude-code-cli"`），公开分发会触发 DMCA。**自研 `core-agent/` 已上线，构建与运行都不再依赖它**，该目录仅作历史参考保留在本地（已 gitignore）。
2. **`.env` 绝不入库** —— `core/.env` 与 `app/src-tauri/.env`（`AI_API_KEY` 等）含真实凭据。密钥一旦进过 commit，即使后续删除仍留在历史中，必须立即作废换新。仅提交 `.env.example` 模板。
3. **大二进制不入库** —— GitHub 单文件硬上限 100MB、仓库 >1GB 告警。以下均已 gitignore：`core/`、`core-agent/target`、`app/src-tauri/target`、`target-e2e`、`binaries`、`app/dist`、`ui/dist`、`vscode-extension/out`、`node_modules`、`mingw64`、`paddle-ocr`、`release`、`local-models`。

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
| 全局热键 Alt+Space | ✅ 已验证正常（三层防线：LL 钩子 + 子类化 + JS 兜底） |
| Esc 行为 | ✅ 有内容清空；空白时隐藏（全由 Rust 统一处理） |
| 设置按钮进入设置面板 | ✅ 支持 toggle：已打开设置时再点击关闭；ESC 关闭任意插件面板 |
| 幽灵框透明区点击穿透 | ✅ `pointer-events: none` 策略 |
| AI 对话 (Agent 模式) | ✅ Agent 单模式 — agent.exe（自研 core-agent）直连 + cli-output 流式渲染（简单模式已移除） |
| Agent 模式前端接入 | `main.ts` 监听 `cli-output` SSE 事件，流式渲染 Agent 对话 | ✅ 已完成 |
| Start Menu 实时模糊搜索 | ✅ 已集成（`main.ts` `search_apps`） |
| 计算器/编码/JSON 插件 | ❌ 已移除 — 2026-07-22 删除，功能由 AI Agent 替代 |
| OCR 文字识别插件 | ✅ 新增 `ocr.ts` — PaddleOCR-json 离线 OCR（多语言） |
| 硬件 AI Agent Tools | ✅ 新增 `tools/system_info.json` — Agent 模式 MCP Tools |
| Agent 内置工具（Read/Write/Edit/Bash/Glob/Grep） | ✅ P1 已完成 — 真实端点烟测通过（多轮工具往返、工作区越界拒绝、`plan` 档只读） |
| Agent 工具权限审批（can_use_tool） | ✅ P2 已完成 — 写类工具执行前弹卡，allow/deny/interrupt 与超时撤卡均验证通过 |
| Agent 上下文预算与压缩 | ✅ 已完成 — 按端点实测体积走瘦身/丢弃两级水位 + 400 强制压缩兜底；真实端点烟测（`LUNAC_MAX_CONTEXT_TOKENS=8000`）连跑 17 轮工具往返不中断 |
| Agent MCP 工具桥（插件面板的 tools\*.json） | ✅ P3 已完成 — agent.exe 作 client 连 `lunac.exe --mcp-server`，用户工具以 `mcp__<名>` 进请求体；真实端点烟测通过（注册、审批卡、成功/失败两条回灌路径） |
| Agent 技能（技能扩展面板的 `<exe 根>\skills\<key>\SKILL.md`） | ✅ P4 已完成 — agent.exe 读 `LUNAC_SKILLS_DIR`，提示词只列 `key: 描述`，模型调 `Skill` 拿到正文（`$ARGUMENTS` 已替换）；面板增删改后自动重启 agent 生效 |

## 10. 待办路线

1. ~~**模式选择器 UI**~~ — ✅ 已完成：状态栏模式指示 + 设置面板模式切换 + 状态同步
2. ~~**Agent 模式前端接入**~~ — ✅ 已完成：`main.ts` 监听 `cli-output` SSE 事件，流式渲染 Agent 对话
3. ~~**接入 Start Menu 实时搜索**~~ — ✅ 已完成
4. ~~**热键可配置**~~ — ✅ 已完成：settings 面板自定义 Alt+key 组合热键
5. **Agent Tools 扩展** — 继续接入更多开源的硬件 AI skill/tools (LocalAI skills, OpenJarvis skills 等)
6. ~~**P2 权限审批**~~ — ✅ 已完成：写类工具发 `can_use_tool`，前端卡片 allow/deny（含 interrupt 中断本轮），300s 超时自动撤卡；见 §3.5
7. **安全档位可切换** — `set_security_profile` 命令已存在但前端无人调用，目前永远「项目」档；需要时在设置面板加 safe/project/full 切换入口
8. **补齐 agent 后端能力** — 与旧 `cli.exe` 的差距（WebFetch/WebSearch、PowerShell、AskUserQuestion、TodoWrite、MCP resources 等）按 [agent-feature-backlog.md](file:///d:/cc/claude-code-cli-master/docs/agent-feature-backlog.md) 的分级与顺序推进
9. ~~**上下文预算与压缩**~~ — ✅ 已完成：见 §3.5「上下文预算与压缩」（backlog §2.1 第 1 项，唯一「用久了必然坏掉」的缺口）
10. ~~**P3 MCP 工具桥**~~ — ✅ 已完成：见 §3.5「P3 已完成（MCP 工具桥）」；`ListMcpResourcesTool` / `ReadMcpResourceTool` 仍未做（server 侧缺 `resources/read`）
11. ~~**P4 技能 SKILL.md**~~ — ✅ 已完成：见 §3.5「P4 已完成（技能 SKILL.md）」；fork / remote 两种模式未做

---

## 11. 关键规则与既定决策（精简规范）

> 仅保留仍然有效的架构规则与既定决策。逐条开发日志不再维护：修改以代码注释、Git 提交记录与 code-rules.md 承载。

1. **开机自启（无需管理员）**：开启时**优先创建「用户登录时」计划任务**（更早触发），创建失败（非管理员常见被拒）**自动回退 HKCU Run** 写入 `"<exe>" --background`；关闭自启时清掉 Run 值与任务两者；状态 = Run 值存在 或 计划任务存在。相关 `schtasks` 只出现在用户点击开关/启动后后台修复线程，绝不阻塞启动热路径。实现见 [auto_start.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/auto_start.rs)。
2. **设置 · AI 供应商 / 模型**：
   - 供应商预设与模型建议为单一数据源：PROVIDER_PRESETS / MODEL_SUGGESTIONS（pp/src/plugins/builtin/settings.ts），禁止两处重复维护。
   - 供应商 = 大节点：模型下拉只含**当前供应商**的预设模型 + 该供应商已保存的自定义模型；切换供应商时回落其默认模型，不把上一家模型带入。
   - 自定义模型确认 = 在**当前供应商模型列表内新增一项**，预设全部保留；内联编辑器只“追加编辑行”，禁止覆盖 dropdown 整体 innerHTML。
   - 接口地址默认**不带 /v1**；启动 agent 后端时先剥离末尾 /v1 再拼供应商的 /anthropic 路由（`AI_AGENT_URL` 可整体覆盖该端点）。
   - 模型建议名必须与供应商**实际可用名**一致：DeepSeek 的 Anthropic 兼容端点只认 `deepseek-v4-pro` / `deepseek-flash`（实测臆造名如 `deepseek-v4.1-flash` 会直接 400 并回报支持列表）—— MODEL_SUGGESTIONS 已按此修正。
3. **设置 · 技能扩展**：固定目录 `<exe 根>\skills`（布局 `<技能key>/SKILL.md`）；lunac 负责 raw SKILL.md URL 安装 / 新建粘贴 / 编辑 / 删除；agent.exe 经 `LUNAC_SKILLS_DIR` 读取该目录，与编译内置技能互不影响；key 由 frontmatter.name 安全 slug 派生。
4. **搜索性能**：Start Menu 扫描 Rust 侧带 30s TTL 缓存（增删自定义应用主动失效，并同时删除落盘文件防复活）。**三级策略（stale-while-revalidate）**：① TTL 内直接返回；② 过期则**立即返回旧数据 + 后台重建**（搜索路径永不因目录扫描阻塞）；③ 无任何缓存才同步扫一次。扫描结果**落盘到 `<exe 根>\temp\app-index-cache.json`**（含版本号 + 保存时间戳，>7 天视为不可信丢弃），**启动时优先从该文件预热**（跨重启秒出，不再等首次扫描），**热键唤出 / 托盘显示时若缓存过期则后台刷新**（方案A，热键路径非阻塞）。前端输入 60ms 去抖并丢弃过期输入，**内容检测（latest-wins）**：`search_apps` 晚回包时校验键入序号（`_searchSeq`），过期或期间已进插件态直接丢弃、不触碰 UI——快速键入只显示最终结果，杜绝旧结果覆盖/一次键入多次渲染闪烁；结果渲染不保留入场 / 开合动画。**窗口高度**：搜索路径懒测量（双 rAF 后实测），`setSize` 串行化（latest-wins，在途期间只记最新期望高度，完成后补发一次），杜绝快速键入时逐键 setSize IPC 风暴 / onResized 回环。
   - **窗口高度「滑动」动画（正式，2026-09）**：非插件/搜索态高度变化默认逐帧滑动（rail 模式——每步等上一 setSize 经 onResized 落地再走下一步），默认参数定稿 `rigidity 0.22`（每帧逼近比例，大=刚性/跟手，小=柔滑拖尾）/ `maxStep 14`（单步最大位移 px）/ `stepHz 120`（步频上限）/ `suppressMs 400`（唤出/启动抑制期）；DevTools Console `__lunac_resize_anim`（含 `enabled=false` 即回退原直设路径）可实时调节，`__lunac_resize_anim_stats` 记录步数/耗时。首次高度落位直设防启动滑屏；插件态离散跳变不走动画。**唤出抑制**：热键/托盘唤出（`lunac-window-shown`）后 `suppressMs` 内的高度变化一律直设并在期内顺延（内容分批到达：剪贴板探测 → 加泡泡 → 重跑搜索 → 实测），保证窗口**瞬时完整展开**——否则会看到结果区被物理窗口裁剪、逐帧“撑开”（WebView2 无法渲染超出窗口的内容）。
5. **通用自定义下拉框**：固定约 4 行（≈120px）可见，更多项内部滚动；滚轮强制内部滚动（`passive:false`）+ `overscroll-behavior:contain`；WebView2 透明窗口禁止原生 `<select>`。
6. **搜索引擎预设**：仅 Google / Bing / Baidu（DuckDuckGo 已移除），旧 localStorage 值自动回退 Google。
7. **数据目录**（2026-09 修订）：根 = **exe 安装根目录**（`current_exe` 所在目录；release=安装根、dev=target\debug，dev/release 数据天然隔离）。统一结构：`temp\`（缓存类：WebView2 用户数据 `temp\webview-data`、应用扫描缓存 `temp\app-index-cache.json`）、`ModuleData\`（业务数据：`history\` 聊天会话/剪贴板、`memo\` 备忘录含图片与 tag、`custom\app_registry.json` 自定义启动项）、`skills\`、`tools\`、`config\hotkey.json`、`paddle-ocr\`。**所有数据落盘模块统一走 `storage::lunac_root_dir()`，禁止各自硬编码路径**；首次启动将旧 `%LOCALAPPDATA%\Lunac(-dev)` 数据整体迁移到 exe 根并删除（`migrate_legacy_localappdata`）。
8. **备忘录插件**：主界面=编辑；保存后弹“检索标识”对话框完成完整保存；历史记录为独立子界面（预览/复制/编辑/删除，保存逻辑同主界面）；标识会同步为检索索引，搜索栏精确/前缀匹配标识可直达对应备忘录编辑界面；支持**粘贴/拖放图片**（存 `ModuleData\memo\images\<id>\`，条目 `images` 字段），编辑器与历史均缩略图显像。搜索无任何应用/插件匹配（含无关乱码）时结果区常驻 3 项：Web 搜索、AI 助手问答、备忘录录入。
9. **图标风格**：统一线性 SVG（`fill:none; stroke:currentColor`，24 栅格，按钮内 12px，stroke-width 2.2，圆头端点），功能按钮禁止用 emoji；规范见 [icon-style.md](file:///d:/cc/claude-code-cli-master/docs/icon-style.md)。
10. **文件附件省略折叠**（搜索栏 / AI 聊天输入栏共用）：前 **3** 个文件为独立泡泡，第 4 个起收进一个“省略泡泡”分支；点击 ⋯ 展开，展开项仍以泡泡框子分支显示，可单删；省略号内提供一键删除全部（仅作用于省略号内容，不影响前 3 个）；**Backspace 空输入删除同步**该按钮：折叠态下清空省略分支，展开或无分支时删最后一个泡泡。**进入任意插件界面自动隐藏**搜索栏泡泡（AI 聊天除外——其泡泡改在聊天输入栏内展示）；退出插件恢复搜索栏原样。Ctrl+V 粘贴支持文件/纯文本路径/**纯位图**（截图、ShareX 等经 Rust 原生剪贴板兜底存临时文件后成泡泡）。
11. **自定义文件启动（快速启动 / Custom Launch）**：持久注册表为 `<exe 根>\ModuleData\custom\app_registry.json`（业务数据同根统一管理；旧 exe 同目录 / LOCALAPPDATA 文件首读自动迁移）。面板打开即列出**全部已注册项**（可启动 / 逐条删除 / 「添加启动项」），重启不丢失、数据不再“消失”；删除 = 注销注册表 + 摘除对应气泡。拖入/粘贴路径进搜索栏即自动注册；入口词多语言/拼音覆盖（launch/open/启动/qidong/dakai/自定义/快速启动…，中文由 pluginRegistry 自动生成拼音索引）。
12. **卸载清理**：NSIS `installerHooks`（[nsis-hooks.nsh](file:///d:/cc/claude-code-cli-master/app/src-tauri/nsis-hooks.nsh)）`NSIS_HOOK_POSTUNINSTALL` 做**双清理**：① 递归删除 exe 安装根内的运行时数据子目录（ModuleData / temp / skills / tools / config / paddle-ocr）；② 删除旧版本遗留的 `%LOCALAPPDATA%\Lunac(-dev)`，实现干净卸载。
13. **OCR 引擎按需下载**：PaddleOCR-json 引擎不随发行包分发，落到 `<exe 根>\paddle-ocr`（与数据根一致）。前端两条入口复用 `ocr.ts` 导出的 `installOcrEngine()`（监听 `ocr-engine-progress`/`ready`/`error`）：① OCR 面板执行识别前先 `ocr_engine_status()`，缺失则在状态行内联「下载并安装」按钮；② 设置 · 常规面板常驻「OCR 引擎」行（状态 + 下载/重试）。**安装必须原子化**：下载 → 解压到 `temp\paddle-ocr-staging` → 校验 `PaddleOCR-json.exe` + `models/config_chinese.txt` → 才删除并 `rename` 到目标目录，任一环节失败清理半成品，避免 `paddle_ocr_dir()` 定位到残缺目录导致 OCR 永久失败且无从诊断。
14. **Agent 内置工具与审批（P1/P2，2026-09）**：六件工具全部在 `core-agent/src/tools.rs`，工具名必须保持 **PascalCase**（前端 `main.ts` 对 `"Bash"` 有专门的命令展示与危险命令分类分支），新增/改名要同步 §3.5 的契约表。写类三件（`Write`/`Edit`/`Bash`）**必须先发 `can_use_tool` 等前端回包**，agent 侧不做二次判断（白名单与危险命令分类归前端 `classifyRequest()`）；`plan` 档直接拒绝、`LUNAC_WORKSPACE_LOCKED=1` 拦越界 —— 这两道闸门与审批是**与**关系，任何一道都不得为了「少点一次同意」而放宽。工具报错必须以 `is_error=true` 的 `tool_result` 回给模型（不中断整轮），只有 HTTP/流错误才回滚 history。
15. **Agent 上下文压缩不变量（2026-09）**：历史一律以 **user 文本消息**开头（不是 `tool_result`），`tool_use` 与对应 `tool_result` 不得被拆散（丢弃点要跳过 `tool_result` 开头的位置）。任何改动 `compact_history()` 的代码都必须同步修正调用方的回滚锚点 `base`（`base -= dropped`），并在压缩后往 `system/context_compacted` 事件里报出计数 —— 这三条是「压缩后仍能继续对话」的充分条件，改动后请用 `LUNAC_MAX_CONTEXT_TOKENS=8000` 的真实端点烟测复验。
16. **测试一律用 flash 模型（2026-09）**：任何真实端点测试（工具往返、权限审批、上下文压缩、MCP 桥等）把 `AI_MODEL` / `LUNAC_AGENT_MODEL` 指向 **`deepseek-flash`**，**不要用 `deepseek-v4-pro`** —— 测试只验证链路、契约与结构，flash 足够且更快更省；只有当问题与回答质量本身相关、或需要复现线上行为时才用 pro。
17. **MCP 工具命名与审批（P3，2026-09）**：接进请求体的用户工具名一律 `mcp__<原名>`，**前缀与清洗规则（非法字符换 `_`、超长截断、重名加 `_2`）不得随意改动** —— 前端审批卡的「始终允许」按完整工具名记 localStorage 白名单，改名等于让用户的白名单失效。MCP 工具**必须**先发 `can_use_tool`（handler 能跑 shell / 发 HTTP），且 plan（只读）档不接入；桥的失败（spawn/握手/超时）只记 stderr，**绝不允许影响六件内置工具的可用性**。

## 12. Agent Plan 模式规范

*来源：Hermes Agent 的 `plan` SKILL.md（MIT 协议，obra/superpowers 贡献），经适配整合。*

### 10.1 核心原则

**Plan 模式下只做计划，不执行代码。**

- 不实现代码、不编辑项目文件（plan markdown 除外）
- 不运行会修改状态的终端命令（不 commit/push/外部操作）
- 可以读仓库、搜索代码、理解上下文

### 10.2 计划文档结构

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

### 10.3 Bite-Sized 任务粒度

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

### 10.4 计划编写流程

1. **理解需求** — 功能需求、验收标准、约束条件
2. **探索代码库** — 项目结构、类似功能、现有测试
3. **设计方案** — 架构模式、文件组织、依赖、测试策略
4. **编写任务** — 按序：Setup → 核心功能(TDD) → 边界 → 集成 → 清理
5. **补全细节** — 精确文件路径、完整代码示例、精确命令与预期输出
6. **审核计划** — 任务顺序合理、路径精确、代码可直接运行

### 10.5 铁律

- **DRY** — 不复制粘贴
- **YAGNI** — 只实现当前需要的
- **TDD** — 每个任务先测后写
- **频繁提交** — 每个任务完成后提交
- **精确路径** — 不是"配置文件"而是 `src/config/settings.ts`
- **完整代码** — 不是"加验证"而是完整函数代码

### 10.6 常见错误

| 错误 | 正确 |
|------|------|
| "添加认证功能" | "创建含 email 和 password_hash 字段的 User 模型" |
| "Step 1: 加验证函数" | "Step 1: 加验证函数" + 完整函数代码 |
| "Step 3: 测试它" | "Step 3: 运行 `pytest tests/test_auth.py -v`，预期: 3 passed" |
| "创建模型文件" | "创建: `src/models/user.py`" |

---

## 12. 新模块（来自 Hermes 整合）

### 11.1 安全模式引擎 (`core/security/`)

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

### 11.2 会话文件清理服务 (`core/services/cleanup.ts`)

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

## 13. 系统化调试规范

*来源：Hermes Agent 的 `systematic-debugging` SKILL.md（MIT 协议，obra/superpowers 贡献），经适配整合。*

### 12.1 铁律

```
不找到根因，不写修复代码。
```

如果你还没完成 Phase 1（根因调查），不能提出任何修复方案。对症状打补丁 = 失败。

### 12.2 四大阶段

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

### 12.3 红旗 — 立即停下来回到 Phase 1

- "先快速修一下，之后再调查"
- "试改改 X 看行不行"
- "同时改多处，一起跑测试"
- "大概就是 X，修一下吧"
- "还不太理解，但这个可能有效"
- **"再来一次"（已经试过 2 次以上）**
- **每次修复都在不同地方暴露新问题**

### 12.4 常见借口与真相

| 借口 | 真相 |
|------|------|
| "问题简单，不需要走流程" | 简单问题也有根因，流程对简单 bug 也很快 |
| "紧急，没时间走流程" | 系统化调试比猜-测-猜更快 |
| "先试一下这个，再调查" | 第一次修复定下模式，从一开始就做对 |
| "修完再写测试" | 没测试的修复不牢靠 |
| "同时改多处省时间" | 无法隔离哪个有效，还引入新 bug |

---

## 14. 代码审查工作流

*来源：Hermes Agent 的 `requesting-code-review` SKILL.md（MIT 协议，obra/superpowers + MorAlekss 贡献），经适配整合。*

### 13.1 核心原则

**不要让写代码的 agent 自己审自己。** 独立上下文才能发现盲区。

### 13.2 8 步预提交管道

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

### 13.3 与安全扫描器的关系

Step 2 的静态扫描已由 `core/security/index.ts` 的 `scanContent()` 在 FileWrite/FileEdit 工具中自动执行。此工作流提供了完整的 pre-commit 审查管道的其余环节。

---

## 15. TDD 开发规范

*来源：Hermes Agent 的 `test-driven-development` SKILL.md（MIT 协议，obra/superpowers 贡献），经适配整合。*

### 14.1 铁律

```
没有先失败的测试，不写产品代码。
```

测试还没写就写代码？删掉，重来。

**没有例外。** 不能"留作参考"、不能"边写测试边改编"、不能"看一眼"。删掉就是删掉。

### 14.2 RED-GREEN-REFACTOR 循环

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

### 14.3 禁止水平切片

错误做法（水平切片）：先写一堆想像的测试，再整体实现。产出脆弱测试。

正确做法（垂直 tracer bullet）：每发子弹走完整流程（一个 RED-GREEN 循环），每发教会你接口长什么样。

### 14.4 红旗 — 删除代码、立即重来

- 测试之前先写代码
- 实现之后补测试
- 测试第一次运行就通过
- 说不清测试为什么失败
- 测试"以后再写"
- 合理化"就这一次"
- "做了 X 小时了，删掉浪费"（沉没成本谬误）

### 14.5 卡住时

| 问题 | 方案 |
|------|------|
| 不知道怎么测 | 先写期望的 API 接口，先写断言 |
| 测试太复杂 | 设计太复杂，简化接口 |
| 必须全 mock | 代码太耦合，用依赖注入 |
| 测试准备代码太庞大 | 抽取辅助函数，还是大？简化设计 |

---

## 16. AI 文本去痕迹规范（Humanizer）

*来源：Hermes Agent 的 `humanizer` SKILL.md（MIT 协议，@blader 原创，基于 Wikipedia "Signs of AI writing"），经适配整合。*

### 15.1 何时应用

Agent 输出面向用户的文本时自动应用：发布说明、PR 描述、文档、长解释、摘要。用户要求 "humanize"/"de-AI"/"de-slop" 时显式加载。

### 15.2 29 个 AI 写作模式

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

### 15.3 加入"人味"

光去掉 AI 痕迹不够。消毒无菌的文字同样很容易识别。

**有灵魂的文字特征**：
- 句子长短不一，节奏变化
- 有观点，不是中立报道
- 承认不确定性和混合情绪
- 适当用"I"第一人称
- 有幽默、锐利、有个性
- 具体的感受，不是"this is concerning"而是"there's something unsettling about..."

### 15.4 处理流程

1. 扫描 29 个模式
2. 改写有问题的段落
3. 保持核心含义、匹配合适语气
4. 注入灵魂（观点、节奏、个性）
5. 最后反问自己"What makes this so obviously AI generated?"→ 修改剩余痕迹

---

## 17. 插件发现架构参考

*来源：Hermes Agent 的 `context_engine/__init__.py` 和 `cron_providers/__init__.py` 插件加载模式，经逆向抽象。*

### 16.1 双目录扫描

插件从两类目录加载，内置（bundled）优先：
1. **内置插件**：`bundled-plugins/<name>/` — 随应用分发
2. **用户插件**：`$APP_HOME/plugins/<name>/` — 用户安装

每个插件目录必须有 `__init__.py`（或 Lunac 等效的 `index.ts`），调用 `register(ctx)` 入口函数。

### 16.2 双重加载策略

1. 优先尝试**函数式接口**：导入 `register()` 函数，传入 `PluginContext` 收集器
2. 回退到**类实例化**：扫描模块中实现特定接口的子类并实例化

### 16.3 合成包注册

虚拟 `sys.modules` 条目使插件内的相对导入工作（如 `from .client import ...`），无需用户手动设置 Python path。

### 16.4 子模块预加载

遍历 `*.py` 文件，`importlib` 预注册所有 submodule，使插件内的交叉导入可用，避免运行时 ImportError。

### 16.5 Lunac 适配方向

Lunac 的插件系统（`app/src/plugins/registry.ts`）已实现关键词匹配 + 评分排序。未来可参考此模式扩展：
- 双目录加载（内置 + 用户）
- 入口函数注册规范
- 生命周期钩子（SessionStart/SessionEnd/PreToolUse/PostToolUse）

---

## 18. 前端集成规范（Hermes 方法论前端接入）

### 17.1 上下文感知 System Prompt 注入

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

### 17.2 调试阶段状态栏

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

### 17.3 Humanizer 按钮

在 `#chat-input-bar` 中新增 "Humanize" 按钮（仅 Agent 模式下可见）：

```html
<button id="humanize-btn" title="去除 AI 痕迹">✎</button>
```

点击后：
1. 取最后一条 agent 文本输出
2. 作为新消息发送：`"Rewrite the following to remove AI writing patterns. Follow the humanizer methodology: remove significance inflation, promotional language, AI vocabulary words, copula avoidance, em dashes, boldface headers, emoji decorations, collaborative artifacts, knowledge-cutoff disclaimers, filler phrases, and generic conclusions. Add a human voice with varied rhythm, opinions, and specific details. Output ONLY the rewritten text:\n\n" + lastOutput`
3. 不做额外解释，直接替换显示区域

### 17.4 代码审查结果行内渲染

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

### 17.5 会话清理状态

在 `/clear` 或 agent 会话结束时，调用 `cleanupSession()`：

```typescript
// In closePluginView() or on session end
if (agentChatHistory.length > 0) {
  invoke("cleanup_session").catch(() => {});
}
```

Rust 侧新增 `cleanup_session` IPC（调用 Node.js 的 `cleanupSession()` 需通过 CLI subprocess 或改为 Rust 原生实现，短期方案：前端的 `cleanupSession()` 由 Tauri 前端侧执行）。

### 17.6 待实现清单

| 优先级 | 改动 | 位置 | 工作量 |
|--------|------|------|--------|
| P0 | 上下文感知 System Prompt 注入 | `main.ts` `startAgentChat()` | ~50 行 |
| P1 | Humanizer 按钮 | `main.ts` + `styles.css` + `index.html` | ~40 行 |
| P1 | 安全警告样式块 | `main.ts` tool_result 渲染 + `styles.css` | ~30 行 |
| P2 | 调试阶段状态栏 | `main.ts` agent 流处理器 | ~60 行 |
| P2 | 3 次修复失败警告 | `main.ts` + 自动注入 message | ✅ 已完成 (2026-07-21) |
| P3 | 会话清理前端调用 | `main.ts` `closePluginView()` | ✅ 已完成 (2026-07-21) |

---

## 19. 用户自定义 Agent 工具/技能 — 双路径执行计划

> **状态：路径 1 已完成 ✅ / 路径 2 已完成 ✅，2026-07-21 执行完毕**
>
> ⚠️ **2026-09 架构变更提示**：本节所述的 MCP 客户端 / SkillTool / 技能加载等基础设施来自**上游 Claude Code 源码（`core/`）**，该目录已停用；自研 `agent.exe` 侧的对应能力（P3 MCP 工具桥、P4 skills）尚未落地，见 §3.5「P1–P4 待做」。下文凡涉及 `core/...` 路径的条目均指旧实现，接入 core-agent 时需按 §3.5 契约重做。

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
