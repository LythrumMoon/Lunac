# 代码规范

> **本文档是什么**：Lunac 现行有效的**代码硬规则**——分层、命名、单一真相源、TS 与 Rust 约定、前后端契约、WebView2/CSS 陷阱。写代码前照这里写，改代码前照这里查。
>
> **取代了哪些旧文档的哪些章节**：
> - `ai-spec.md` §6（文件结构）、§7（开发命令）、§11 规则 2 / 4 / 7 / 14 / 15 / 18 / 19 / 22 / 26 / 27 / 28 / 41 / 45 的结构性约定；
> - `code-rules.md` 第 2–10 章（Tauri 权限、IPC、异步、CSS/WebView2、DOM/状态、热键、窗口、流式、剪贴板）与第 11 章反模式表。
>
> **历史细节见 `docs/archive/code-rules.md`、`docs/archive/ai-spec.md`。** 单项禁令速查见 [`forbidden.md`](./forbidden.md)；构建与验收见 [`testing.md`](./testing.md)。

---

## §规范 1 分层与目录

- 五类源码根，各司其职，**跨层不要互写**：
  - `app/src/`：前端（TS + `styles.css` + `index.html`）+ 内置插件 `plugins/builtin/`；
  - `app/src-tauri/src/`：宿主 Rust（窗口 / 托盘 / 子进程生命周期 / IPC / 存储）；
  - `core-agent/src/`：自研 agent 内核（`agent.exe`，独立 crate）；
  - `tuning-engine/`：DSP 引擎（独立 crate，宿主以**路径库依赖**链它）；
  - `scripts/`、`agent-templates/`：构建发布脚本与随包预置（技能 / 工具模板）。
- **数据根 = `<exe 根>`（便携式，无独立 HOME）**：`config\` / `ModuleData\` / `temp\` / `skills\` / `tools\`。所有落盘模块统一走 `storage::lunac_root_dir()`，**禁止各自硬编码路径**。
- `temp\` 放缓存类（`webview-data` / `*-index-cache.json` / `logs` / `tool-outputs` / `transStorage`）；`ModuleData\` 放业务数据（`history\chat.db` / `usage-*.jsonl` / `memory` / `custom`）。
- 所有 `.ps1` 脚本必须**从自身位置推导仓库根**（`$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)`）——**禁止硬编码本机绝对路径**。
- 目录落点判据（AI 自建东西四分法）：Lunac 插件 → `Modules\`；AI 技能 → `skills\`；MCP 工具 → `tools\`；**其它一切**（独立脚本 / 工程）→ 工作目录 `transStorage`。不要自创第五个目录。

## §规范 2 单一真相源（跨层同一个值只写一处）

同一个值/判据**只能有一份定义**，别处一律引用它：

| 值 | 唯一真相源 |
|---|---|
| 供应商预设 / 模型建议 | `settings.ts` 的 `PROVIDER_PRESETS` / `MODEL_SUGGESTIONS` |
| AI 凭据 | `<exe 根>\config\ai.json`（面板保存 → `set_ai_config` 先落盘再注入 env） |
| 系统动作（设置页 / 关机…） | `system_catalog.rs` 的 `action_spec(id)`（`run_action` / `elevatable` 都从它推） |
| 应用列表 | `temp\app-index-cache.json`（Rust 侧**不持有进程内缓存**） |
| 成本算法 | `app/src/usage-cost.ts`（宿主与前端 import 同一个模块） |
| 主题色 | CSS 变量 `--accent-rgb`（`rgba(var(--accent-rgb), x)`，禁止硬编码 rgb 分量） |
| 中性叠加 / 凹陷 | `--ink-rgb` 与 `--shade-scale` |
| 滤波 / 预设 / 工具名单 | 引擎或宿主的那张表（后端回传，前端照它画，**不另抄一份**） |
| 工具名 | `tools.rs` 的 `defs()`，PascalCase 稳定（前端对 `Cmd` / `PowerShell` 有特判） |
| 插件名 / 描述 | `pluginName()` / `pluginDesc()`（禁止硬编码英文常量） |

- **判据**：改某一处后，所有消费点必须同步变；出现第二份实现即为回归。

## §规范 3 命名

- **Rust**：命令 `snake_case`；模块名与目录一致；同一 crate 里同一个 Win32 符号**只能有一种 extern 声明**（多模块要用就抽共享模块，如 `win_proc.rs`）。
- **TS / CSS**：`camelCase` 变量、`kebab-case` class 名；CSS 变量 `--xxx`；SVG 常量命名 `*_SVG`，复用已有常量（`COPY_SVG` / `pluginIconSvg()`…），不重复造路径。
- **i18n key**：沿用既有前缀——`agent.*`（对话面板）、`settings.*`（设置）、`plugin.<id>` / `plugin.<id>.desc`（插件）、`chat.*`、`music.*` 等。**新增 key 一律进 `app/src/i18n.ts` 的 DICT**。
- **工具 / 资源**：内置工具 `PascalCase`；MCP 用户工具接入名一律 `mcp__<原名>`（前缀与清洗规则不得改，白名单按完整名记）；插件目录名 = id。

## §规范 4 TypeScript / 前端

- **所有面向用户的文案走 `t("key")`**，禁止硬编码中文（含英文）——见 [`ui-style.md`](./ui-style.md) §界面 7。
- **不新增前端依赖**（不引 UI 框架 / 图标库 / 动画库）；图标用内联线性 SVG。
- **并发优先于串行 await**：隐藏窗口与启动程序等操作要并发，别让用户等 IPC 往返（`win.hide(); invoke(...).catch(...)`）。
- **状态同步顺序**：先 `await invoke(...)` 改 Rust 原子量，**再**设 JS 标志，避免竞态窗口。
- **事件监听器生命周期**：模块级保存 `unlisten`，重新 attach 前先清理旧的；用**静态 import** 而非动态 `await import`（确保监听器就绪）。
- **流式中途销毁 DOM 前递增 `streamId`**，回调里 `if (myStreamId !== streamId) return`，防旧回调污染新对话。
- **批量 DOM 用 `DocumentFragment` + `replaceChildren`**，避免逐条插入多次重绘。
- **幂等重绘**的判据不能直接比 `el.innerHTML` 与源码串（浏览器序列化会改写 void 元素）——两边过同一套序列化再比；周期性 `render*` 还要叠焦点守卫（正在编辑就不重绘）。
- **`renderMixedResults` 一类渲染函数必须守卫 `pluginActive`**，插件面板打开时不覆盖其 HTML。
- 关闭组件时**完整清理状态**（`streamId` / `isStreaming` / 各 callback / `pendingMessages` / `consecutiveFailures`）。
- **DOM 下标不能烘死**：数据被裁剪时同步前移已渲染元素的下标；目标已不在数据里的元素要摘掉会切错东西的按钮。
- **「打开本地路径」必须走宿主 `open_path`**，不得用 `plugin-shell` 的 `open()`（本地路径会被 scope 正则拒掉，且异常常被 `.catch(()=>{})` 吞掉 ⇒「点了没反应」）；**URL 仍走 `open()`**，且**不得**把本地路径加进 open scope。

## §规范 5 Rust / Tauri

- **非 `async` 的 `#[tauri::command]` 跑在主线程上**（和窗口消息泵同线程）。命令体里任何等待（`reqwest::blocking`、`Command::output()`、图标提取、OCR、大文件读写）都必须**写成 `async` 薄壳 + `run_blocking(...)`** 丢进专用阻塞池。**不要**用 `#[tauri::command(async)]` 顶替（那只丢进 tokio worker）。搬进阻塞池后要检查共享资源能否并发（必要时在唯一入口加进程内串行闸）。
- **新增前端 API 要在 `capabilities/default.json` 显式声明权限**（`core:default` 不含窗口修改类权限；缺权限会被静默拒绝）。自定义 `#[tauri::command]` **不需要**改 capabilities；插件 API / 新窗口 label 需要。
- **子进程**：`spawn()` 之后**立刻** `child_job::assign(&child)`（Job Object 兜底）；长期驻留的还要显式注册「插件退出 ⇒ 杀死」。`.output()` 拿不到句柄要拆成 `spawn()` + `wait_with_output()`，且 `spawn()` 不替你设管道，需显式 `.stdout(Stdio::piped()).stderr(Stdio::piped())`。
- **回环 HTTP 一律 `.no_proxy()`**：reqwest 在 Windows 上会读系统代理却**不认绕过表** ⇒ 到 `127.0.0.1` 的调用会静默连不上。排查「本机端点连不上」先查 `ProxyEnable`。
- **配置新增字段**必须写进对应的 `RawConfig` / struct（`deny_unknown_fields` 会拒掉没声明的字段），并想清「哪些类型用不上它」——用不上的组合**报错，不许静默忽略**。
- **校验-落盘-内存**顺序固定：先校验 → 再落盘 → 最后改内存与版本号。校验不过**一个字都不写**，界面回滚。

## §规范 6 前后端契约（IPC 与 stream-json）

- **`invoke()` 参数必须与 Rust `#[tauri::command] fn` 签名逐一对齐**（每个参数是顶层 key）；返回类型也要对上。
- **改 `stream-json` 协议要走登记流程**：优先从现有字段推导；确需新字段时，在契约表登记字段名与语义、**保持 `tool_result.content` 文本不变**、前端**兼容缺字段**（无字段回落文本解析）。
- **前缀缓存不变量**（DeepSeek 等端点按最长公共前缀命中）：`history` 一律以 user 文本消息开头；系统提示词固定、技能清单按 `key` 排序、工具数组按名排序；**往已有 `tool_result` 里追加内容只能拼进它的文本内部**，不得新增消息或内容块。
- **往 `history` 写内容的每条路径都要过预算/有硬上限**（工具结果、后台命令回流、附件、注入块）——漏一条就是「每轮都 miss」。
- **`result.usage` 是每次提问的绝对值**，前端禁止对它做差；用量口径与对账详见 archive 的 `ai-spec.md` §3.5。
- **「agent 就绪」信号必须能被新窗口重新获得**：前端 `cliReady` 的唯一置位源是宿主事件 `cli-status:stdout`。因此宿主里那条「确保 agent 就绪」的命令（`start_cli`）在 agent **已在跑**时也必须**重播**一次该事件 —— 否则一个**新建**的聊天窗（旧窗关掉后 `agent.exe` 仍在）将永远等不到就绪信号、其挂起的问句永不冲刷（2026-10-11 桌宠轮报障即此；见 `todo.md` §待办 1-4 补）。

## §规范 7 WebView2 与 CSS

- **禁用原生 `<select>`**（透明窗口不渲染弹层）→ 用自定义下拉组件。
- **`overflow: hidden` 会裁剪 WebView2 原生弹窗**；但**不要**把 `.plugin-open{overflow:visible}` 加到滚动容器上（会拆掉滚动盒、`scrollTop` 归零）——判据：它是不是滚动条真正出现的那一层。
- **隐藏元素用 class 控制**（`pointer-events` + opacity 过渡），避免 `display` 冲突；**overlay 关闭要 `remove()`**，不能只 `opacity:0`（仍拦点击）。
- **`pointer-events: none` 的区域无法点击**：所有交互子元素要显式 `auto`；窗口边缘 padding 会制造死区，用子元素 margin 替代。
- **统一细滚动条**见 [`ui-style.md`](./ui-style.md) §界面 6（禁止写 `scrollbar-width` / `scrollbar-color`、禁止按容器单独声明、不许藏滚动条）。
- **`requestAnimationFrame` 不是可靠的布局完成信号**（透明窗口常判未渲染）；延迟检测用 `setTimeout`。CSS `transitionend` 可能不触发，**必须加 setTimeout fallback**。
- **`pre-wrap` 继承会把 HTML 模板缩进渲染成幽灵空行**：往 `#chat-log` / `.agent-flow` 拼 `innerHTML` 的模板**不得带缩进换行**，或给结构容器声明 `white-space: normal`。

## §规范 8 热键 / 窗口 / 剪贴板

- **热键后端**：先 `RegisterHotKey`，失败才装 `WH_KEYBOARD_LL` 兜底——**不要为了少写分支无条件装钩子**（会引入 AV 误报 + 游戏失效两个回归）。
- LL 钩子回调内**只能 `PostThreadMessageW`**，窗口操作丢独立线程；Alt 检测用无状态查询（`LLKHF_ALTDOWN` + `GetAsyncKeyState`），**禁止手动 down/up 跟踪**。
- **热键判定必须「主键 + 修饰键」同时比对**；`HOTKEY_MODIFIERS` / `HOTKEY_VK` / `HOTKEY_COMBO` 必须经 `parse_and_set_hotkey` 一起设置。
- **前台锁定**：`set_focus()` 可能被 Windows 静默拦下 → `AttachThreadInput` + `SetForegroundWindow` + 重试。
- **最小化检测要加 `IsIconic`**（`IsWindowVisible` 最小化时仍返回真）。
- **剪贴板读取必须 `app.run_on_main_thread()`**，不能在线程池线程读。
- **搜索内容不随失焦清空**（`win.hide()` 纯隐藏）；`onFocusChanged(false)` 用 150ms debounce，程序化 hide 时设 `blockingHide`。

## §规范 9 文档与脚本卫生

- **幂等 / 可验证**：任何「写盘前先瘦身」的管线要加体量断言（`pruneContext` 曾因数组别名让库每轮 ×12 膨胀）。
- **`.ps1` / `.nsi` 含中文必须 UTF-8 with BOM**（恰一份），改完跑 `npm run verify`——见 [`testing.md`](./testing.md) §测试 3。
- **仓库页面（README / `docs/**` / `agent-templates/**/README.md`）一律不插装饰性 emoji**，清单表格标题用纯文字。

---

> **历史细节见 `docs/archive/code-rules.md`（预检清单 #1–#77 与 11 章反模式）与 `docs/archive/ai-spec.md` §11。** 本文只保留仍有效的现行规则。
