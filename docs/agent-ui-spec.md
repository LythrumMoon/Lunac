# Lunac AI 对话界面规范（参照 Trae 侧栏）

> 定位：本文是 **AI 对话区（agent 面板）内部** 的界面与交互规范，供「参照 Trae AI 侧栏改造 Lunac 对话界面」使用。
> 状态：**已批准并实施（2026-09）**。阶段 1（思考省略 / 命令卡片 / 系统提示块 / 回合折叠）与阶段 2（运行方式三档 + 安全档位入口 + 越界卡片三动作）已落地，对应约束已回写 [ai-spec.md](./ai-spec.md) §3.5、§3.7B、§11 规则 21。阶段 3（diff 卡 / Fork / 真沙箱）仍未做。
> 关联规范：[icon-style.md](./icon-style.md)（图标）、[code-rules.md](./code-rules.md)（前端硬规则）、[ai-spec.md](./ai-spec.md) §3.5（agent 协议契约）、§11 规则 4（窗口高度与搜索性能）。

---

## 0. 硬边界（不可越界）

1. **不动总体视窗**：窗口宽度（`WIN_WIDTH = 800`）、透明/无边框/毛玻璃形态、`#search-bar` → `#results-container` → `#status-bar` 的纵向结构、AI 态窗口高度策略（embedded 360 / detached 600 / OCR detached 520，×`currentZoom`，离散直设、不走滑动动画）**全部保持不变**。
   - 本规范只改 **`#results-list` 内部**（对话流的 DOM/CSS/交互），以及输入栏内部控件的增删。
   - 明确**不做**：Trae 那种「可拖拽宽度、常驻右侧的独立侧栏」。Lunac 是 800px 宽的独立唤出式窗口，没有编辑区可让位。
2. **不改 agent 协议契约**（`stream-json` 的字段与语义，见 ai-spec §3.5）。所有新增展示信息**优先从现有字段推导**；确需新字段时，必须在本文第 9 节登记并由后端同步文档。
3. **不新增前端依赖**：不引入 UI 框架/图标库/动画库；样式写在 `app/src/styles.css`，图标用内联 SVG（icon-style.md §1 参数）。
4. **所有文案走 `t()`**：新增 key 一律进 `app/src/i18n.ts` 的 5 语言 DICT（zh-CN / zh-TW / ja / ko / en），**禁止硬编码中文**（现状有历史遗留，见 §3.2）。
5. **不削弱安全语义**：审批与工作区锁是硬边界（ai-spec §11 规则 14），任何「自动放行」类 UI 都不得取消这两道闸门，只能改变**询问频率**（见 §4.3）。

---

## 1. 调研基线

### 1.1 Lunac 现状（事实，含证据）

| 面 | 现状 |
|---|---|
| 对话流容器 | `.ai-response#chat-log` → 每回合一个 `.agent-flow[data-turn-id]`（`main.ts` L4356、L4502）；滚动区是 `#results-list`（`styles.css` L900） |
| 用户消息 | `.chat-msg-user`（accent 底色气泡，`styles.css` L1755），带 `.msg-actions`（复制/回滚/重试）；**无头像、无角色标签、无时间戳** |
| 助手消息 | `.agent-text` 段落（`styles.css` L1159），无气泡 |
| 思考（thinking） | `<details class="think-block">`（`main.ts` L2497-2504），流式中 summary 为「💭 思考中… (N 字)」，结束后收起为「💭 思考过程 (N 字)」（L2622-2627）；CSS `styles.css` L1224-1256。**summary 文案部分硬编码中文**（只有初始 summary 走 `agent.thinking_toggle`） |
| 工具调用 | 单行 `.tool-row`：`🔧 <工具名> <参数摘要>`（`main.ts` L2512-2516、L2556-2575）。**不显示**退出码 / 耗时 / 输出 |
| 工具结果 | `.tool-row.tool-ok`（可折叠 `<details>`，正文截 600 字符）/ `.tool-error`（`main.ts` L2654-2680）；连续 3 次失败追加 `.tool-warn` |
| 回合汇总 | `.turn-footer` + `.turn-tool-count`（成功/失败计数，`main.ts` L4410-4421） |
| 审批卡 | 已相当完整：批量卡 `.approval-batch-card`、危险命令 `classifyRequest`（L2734）+ `CMD_BLACKLIST`（L2701）高亮并**移除「始终允许」**、白名单自动放行（`BUILTIN_SAFE_PREFIXES` L2692 + localStorage `lunac-approve-whitelist`）、连续简单命令合并（L2796/L2819/L2913）、`AskUserQuestion` 专属卡片 |
| 安全档位 | **后端已就绪、前端零入口**：`set_security_profile`（`commands.rs` L710）无人调用，默认恒为 `project`（ai-spec L241 已记为缺口） |
| 「沙箱」 | 前端不存在该概念与文案 |
| 主题 | **仅一套固定深色**：`:root` 变量（`styles.css` L4-23）`--surface-glass / --border-glass / --text / --text-dim / --text-muted / --accent / --accent-bg / --accent-border / --green / --red / --yellow / --blue / --radius`；无 `data-theme`、无浅色、无 `prefers-color-scheme` |
| 图标 | 已遵循 icon-style.md（内联线性 SVG）。emoji 仅作装饰前缀：`💭`(思考) `🔧`(工具) `📋`(待办) `💬`(历史) |
| `cli-stderr` | **无 UI**，只 `console.warn` + 存 `lastCliStderr`（`main.ts` L3365）；仅在 CLI 关闭时兜底显示一行状态 |

### 1.2 Trae 侧栏的参照点（公开文档）

1. **命令卡片**：AI 给出的命令以卡片呈现，卡片上可「执行 / 跳过」；执行后自动读取并分析输出，失败时给原因与后续方案；成功后可点卡片右上角在终端查看执行日志。
2. **思考与对话流自动折叠**：完成的节点自动折叠并**生成摘要**，可展开看细节（设置里可开关）。
3. **沙箱 / 命令运行方式**：三选一 —— **沙箱运行（支持白名单，默认）** / **手动运行** / **自动运行**；沙箱内越权时给三选项：**跳过 / 在沙箱外运行 / 加入白名单**；另有 Shell 拦截（`rm` 类高危命令）兜底。
4. **代码变更**：接受 / 拒绝单条、单文件、全部；DiffView 汇总受影响文件数与变更行数。
5. **会话辅助**：对话缩略导航（跳到任一 Query）、创建会话副本（Fork）、分享（图片/链接）、恢复到 N 回合前。
6. **上下文输入**：文件 / 文件夹 / 图片、语音输入、「优化输入内容」。

---

## 2. 目标形态：采用 / 适配 / 不做

| Trae 特征 | Lunac 决策 | 说明 |
|---|---|---|
| 思考过程折叠 | **采用**（已有基础，规范化） | §3.2：折叠 + 首行预览 + 字数；**不做模型摘要**（无额外模型调用） |
| 命令执行卡片 | **采用**（重点） | §3.3：状态 / 退出码 / 耗时 / 可折叠输出 / 复制 |
| 对话流自动折叠 | **采用** | §3.5：回合完成后折叠为摘要行，设置可关 |
| 沙箱 / 运行方式三档 | **适配**（重点，见 §4） | Lunac 无 OS 级沙箱，用「运行方式 + 工作区锁 + 危险命令拦截」表达同等意图，**且不得自称沙箱** |
| 白名单（命令前缀） | **采用** | 已有机制，补 UI 与说明 |
| 高危命令拦截 | **采用**（已有 `CMD_BLACKLIST`） | 补「拦截原因」在卡片上的可读展示 |
| 代码变更接受/拒绝 + DiffView | **暂不做**（P3 评估） | 需要后端回传 diff 或前端重算，改动面大于本轮目标。先在 §10 记录路线 |
| 会话 Fork / 分享 | **不做** | Lunac 是本地单机工具，分享链路与产品定位不符；Fork 收益低 |
| 恢复到 N 回合前 | **已有**（`.msg-rollback`） | 保持现状，不扩 |
| 多轮缩略导航 | **P2** | 与现有历史抽屉职责重叠，先做抽屉增强 |
| 语音输入 / 优化输入内容 | **不做** | 需要云端能力或额外模型调用 |
| 文件/图片上下文 | **已有** | 搜索栏气泡 + 聊天输入栏复用，保持现状 |

---

## 3. 对话流规范

### 3.1 消息单元

对话流由「回合（turn）」组成，一个回合 = 用户消息 + 助手的一次完整应答（可能含多轮工具往返）。

- 用户消息：保持 `.chat-msg-user`（accent 底色气泡）+ `.msg-actions`。**本次不改其视觉**。
- 助手应答：仍无气泡，由若干「块」按时间顺序纵向排列：
  - `.think-block`（思考）
  - `.agent-text`（正文，流式增量）
  - `.tool-row`（工具调用与结果，见 §3.3）
  - `.turn-footer`（回合汇总，回合结束时出现）
- **新增**：回合计时与状态徽标（`.turn-badge`，可选显示）—— 回合进行中在 `.turn-footer` 位置显示「执行中 · 已用 N 秒」，结束后替换为「完成 · N 步工具调用」。

规则：
- 块级元素一律左对齐、同一左边距基准（与 `#results-list` 内边距对齐），不引入横向滚动。
- 所有新增块必须能被 `#results-list` 的既有滚动条样式覆盖（`styles.css` L1701 那组选择器要同步加类名）。
- 长内容 **必须** 在自己的块内折叠，**禁止**让单个块无限拉长（否则窗口固定高度下用户只能一路滚）。

### 3.2 思考省略（thinking collapse）

目标：思考过程默认「隐身」，只留一行可展开的痕迹 —— 与 Trae 的节点折叠同义。

- **流式中**：summary 显示 `思考中… (N 字)`，右侧带呼吸光标（复用 `.cursor-blink`），**并自动展开**（让用户看到正在进行）。
- **结束后**：自动收起，summary 变 `思考过程 (N 字)`；用户点击可展开/收起。
- **省略策略**（"省略"的具体含义）：
  - 展开时完整展示，但若超过 **4000 字**，只渲染首 2000 字 + 中间省略提示 + 末 500 字（避免长思考拖垮渲染）。
  - 折叠状态下不渲染正文 DOM 之外的内容；**禁止**在折叠时保留完整正文节点（用 `textContent` 惰性填充，展开时才写入）。
- **不做模型摘要**：Trae 的「折叠后生成摘要」需要额外一次模型调用；Lunac 的成本与延迟都不划算。改为「首行预览」：summary 末尾附正文首行前 40 字（脱敏后）。
- **文案全部走 i18n**：`agent.thinking_running` / `agent.thinking_done`，`{n}` 为字数插值。**必须修掉现状的硬编码中文**（`main.ts` L2535、L2625）。
- **图标**：把 `💭` 换成线性 SVG（气泡 + 残影，参数按 icon-style.md §1）。emoji 仅可用于纯装饰，不再承担「状态指示」语义。

### 3.3 命令执行卡片（`.tool-row` 升级）

现状是「一行文字」，升级为**可折叠卡片**，但仍是**行内卡片**（不弹独立窗口、不占满宽度）。

卡片结构（自上而下）：

| 区域 | 内容 | 数据来源 |
|---|---|---|
| 头部 `summary` | 状态图标 + 工具名 + **命令/参数摘要**（单行，超长省略号） | 现有 `tool_use` 入参 |
| 元信息行 | 状态标签（执行中 / 成功 / 失败 / 已跳过）+ **退出码** + **耗时** | 退出码见下；耗时前端自算 |
| 输出区 | 折叠的 stdout / stderr 文本（默认收起，最多渲染 600 字符 + 「查看全部」提示） | 现有 `tool_result.content` |
| 操作区 | 复制命令、复制输出 | 纯前端 |

**数据获取原则（本轮不改协议）**：

- **退出码**：`run_shell` 已把 `exit code: N` 写进 `tool_result` 文本（`core-agent/src/tools.rs`），前端按 `/(?:^|\n)exit code: (\d+)/` 与 `(no output)` 解析即可 —— 解析失败时不显示，不报错。
- **耗时**：前端在收到 `tool_use` 时记 `performance.now()`，收到对应 `tool_result` 时相减（按 `tool_use_id` 配对）。不依赖后端字段。
- **超时**：`run_shell` 会写 `(timed out after N ms — process killed)`，识别后状态显示为「超时」而非「失败」。
- **成功/失败判定**：优先看 `tool_result.is_error`；`is_error=false` 但退出码 ≠ 0 时显示为「完成（退出码非零）」，仍**不**标红 —— 与后端语义一致（工具本身没失败，是命令返回非零）。
- 若上述解析在真实数据上覆盖率不足，再走「后端新增结构化字段」路线（见 §9 的登记流程），**不得**先私自加字段。

**命令类工具**：`Bash` / `PowerShell` 用等宽字体显示命令原文（保留换行，最多 3 行 + 省略）；`Read` / `Write` / `Edit` / `Grep` / `Glob` 显示 `k=v` 摘要（现状逻辑保留）。`TodoWrite` 仍走 `.todo-panel`（不画卡片）。

> **注意区分两个「复制」**：上面的操作区属于**执行卡片**（`.tool-row`，命令已经跑完）。**审批卡**（`.approval-batch-card`）的命令区**不放复制按钮**（2026-09 用户要求删除）—— 按钮固定在右上角，短命令与它之间会空出一大片（还得给 `.approval-cmd-list` 预留 56px 右边距），而命令文本本身已可选中复制（`.approval-cmd-box` 有 `user-select: text`），够了。**不得**为「方便复制」把按钮加回去。

### 3.4 结果与错误

- 成功：`.tool-ok` 折叠体，summary 为「成功」+ 首行摘要。
- 命令非零退出：状态「完成（退出码 N）」，用 `--yellow`；**不做** `--red`。
- 工具真失败（`is_error=true`）：`--red` + 失败原因（`tool_result` 文本首行）。
- 连续失败 ≥3：回合内追加一条 `.tool-warn` 提示（保留现状），并在回合汇总里计数。
- **`cli-stderr` 落 UI**（现状缺失）：收到 `cli-stderr` 时，在对话流末尾插入一条**折叠的系统提示块** `.sys-note.sys-note-error`（默认收起，summary 为「运行端输出 (N 行)」）。这是「UI 显示报错、但事后无日志」问题的正面修复；与 §11 规则 20 的落盘互补。

### 3.5 回合自动折叠

- 回合完成后（收到 `result`），若回合内块数 ≥ 3 或工具调用 ≥ 2，**默认折叠**该回合的所有中间块（思考/工具/结果），仅保留：用户消息、助手最终正文、`.turn-footer`（含「展开过程」按钮）。
- 折叠态在 `.turn-footer` 显示摘要行：`N 步工具调用 · M 次失败 · 耗时 Xs`。
- 用户手动展开后，该回合在本次会话内保持展开（不因后续回合而回弹）。
- **开关**：`localStorage` 键 `lunac-agent-autofold`，默认 `1`（开）；入口放设置面板「AI」分区。关闭时行为与现状一致（全部展开）。

### 3.6 权限卡（审批卡）

一张批量卡（`.approval-batch-card`）承载**一次权限运行**里的所有请求，卡头有数量与「全部允许 / 全部拒绝」，卡体**一行一条**请求（`.approval-item`）。规范：

- **命令置顶、行高自适应（2026-09 重构）**：`.approval-item` **不得设固定高度**。旧实现给 `height: 144px` 且 `.approval-body { flex: 1 }`，于是短命令后面留出一大片空档、命令文本看起来「浮在中间」而不是贴顶。现在行高由内容决定，命令区 `.approval-cmd-box` 高度自适应内容（`.approval-input.compact` 仍保留自己的 `max-height`）。
- **命令区不许有第二层滚动容器（2026-09-15 修）**：`.approval-cmd-box` 曾自带 `max-height: 72px; overflow-y: auto`，与外层 `.approval-batch-body`（`max-height: 380px; overflow-y: auto`）叠成**嵌套双滚动条**。实测（两条稍长的命令各折 3 行 / 2 行）：`scrollHeight = 106` vs `clientHeight = 72`，内层把 34px 内容裁在框外 —— 「已合并 N 条命令」整行看不到、末尾折行只剩半行，用户描述为「空白区域」并怀疑是模型输出带了空行。**滚动只由 `.approval-batch-body` 承担。**
- **`└ ` 前缀行要有悬挂缩进（2026-09-15 修）**：`.approval-cmd-sep`（`└ `）是 inline 前缀，第 2 条命令起必须给行挂 `.with-sep`（`padding-left: 2ch; text-indent: -2ch`）。不挂的话折行续行回到框左边缘、比正文左移 **2ch**（实测 12.11px），看起来就是「缩进对不齐」。用 `ch` 不写像素 —— 跟等宽字号走。
- **命令区出现大片空白 / 内容不贴顶 = `pre-wrap` 继承把模板缩进渲染成了空行（2026-09-17 定位，真因）**：用户三次反馈「命令缩进 + 空白」，前两次改的固定高度（`height:144px`）与嵌套双滚动**都是真问题、也都该修**，但**症状仍在**。真因是：`#chat-log`（`.ai-response`）自己声明了 `white-space: pre-wrap`，而 `.approval-body` / `.approval-cmd-box` / `.approval-cmd-list` 这些**中间容器没声明** ⇒ 继承 `pre-wrap` ⇒ `main.ts` 里三处 `innerHTML = \`…\`` 模板（`card.innerHTML` / `item.innerHTML` / `renderCmdGroupBody()`）的**缩进换行被当成真实空行渲染**。实测：一处空白节点 ≈ **45px**，命令区 471px 里 **315px（67%）是空行**，命令文本只占 129px。修法是 `.approval-card { white-space: normal; }`（声明在卡片根，一次覆盖三层模板），修后 471 → **156px（−315px）**、文本仍 129px 不变、外层滚动条消失。**通用纪律**：往 `#chat-log` / `.agent-flow` 里拼 `innerHTML` 的模板**不得带缩进换行**（写成一行，或给结构容器声明 `white-space: normal`）。见 ai-spec §11 规则 33。
- **排查心法**：用户说「命令有缩进 / 有空行」时，**先按这个顺序查，别凭截图猜** —— ① **把实际命令字符串取出来看**（`ModuleData\history\chat.db` 里该会话的 `steps` 快照；注意 `detail` 是**截断到 200 字符**的显示文本，不是全文，落盘日志也只记「等待审批 PowerShell」不记命令原文）；② 查 `white-space` 的**计算值**（`getComputedStyle(el).whiteSpace`）与容器高度能否**逐项对账**（文本高度 + padding + 附属行，有无余数）；③ 用 `Range.getClientRects()` 数容器内**空白文本节点**是否产生了行盒（`rectCount > 0` 即幽灵空行）。实测多次命令原文都是**干净单行、`normalizeCmdForDisplay()` 也没改坏**，别去改 agent 侧输出或那个归一化函数。
- **命令区不放复制按钮**（2026-09）：按钮固定右上角会给短命令留一大段空白（`.approval-cmd-list` 还得预留 56px 右边距）；命令文本本身可选中复制。注意这与执行卡片 `.tool-row` 的「复制命令」是**两个不同组件**，不要混。
- **同一次权限运行内合并成一行**（2026-09 放宽）：同一条未应答的命令组行是**唯一合并目标**，`Bash` 与 `PowerShell` **视作同一族**（用户要求「短时间内不同类型的命令也合并进同一个权限运行」）。合并**只影响审批展示** —— 每条命令仍由 agent 各自执行，`&&` 短路、退出码、输出都不受影响，所以含 `&&` / `|` / 重定向 / 换行的**复杂命令同样入组**（旧实现按算子排除，正是「复杂任务里同类请求一行一条堆满卡片」的原因）。上限 `MAX_CMD_GROUP = 20` 条 / 单条 2000 字符。**跨轮永远合并不了**：下一轮的命令要等上一轮的执行结果才由模型产生。
- **合并后标题要重画**：工具名可能不止一个（显示 `Bash + PowerShell`），⛔ / ⚠ 标记也可能来自后来合并进来的那条命令 —— `renderGroupTitle()` 在每次合并时重画标题；`renderCmdGroupBody()` 顺带兜掉「始终允许」（组内只要有一条危险 / 不透明，整组都不给）。行的 `danger` 类同步补上。
- **「始终允许」写白名单要写**组内**每一条**命令的命令词（旧实现只记第一条 → 组里第二条以后的同类命令下次还要再问一遍，即用户反馈的「允许过还要再问」）。解释器 / 启动器前缀与危险 / 不透明命令照旧排除（§4.3）。
- **自动档不该弹卡**：`opaque` 判据 2026-09 已收窄到「不知道要跑哪个程序」（参数里的 `$HOME` / `$env:TEMP` 不算，见 ai-spec §3.5）—— 旧口径会让自动档下每条带 `$` 的命令都弹卡。

### 3.7 历史回顾（过程与表盘）

历史会话（**2026-09-17 起存放于 `ModuleData\history\chat.db`（SQLite + FTS5），旧位置是 `chat-history.json`**；Rust 侧 `load_chat_sessions` / `save_chat_sessions` 命令名与入参形状都没变）不只存气泡，还存：

- **`steps`（过程快照）**：回合结束时由 `recordTurnSteps()` 采集一次（thinking / tool / text，超长截断），恢复历史时用 `renderHistoryProcess()` 渲染成插在该回合助手消息之后的**可折叠「过程 · N 步」块**，默认折叠。渲染复用对话流里同样的类名（`.think-block` / `.agent-text` / `.tool-card`），保证视觉一致。
- **`usage`（表盘快照）**：恢复历史时写回 token 仪表盘（`usageTotals` + `updateTokenDashboard()`）—— 表盘口径是「当前这次对话」，所以此时显示的就是**那次对话**的数值。
- 缺字段（旧记录）时两者都按空处理，不得报错。

### 3.8 被改动文件的路径追踪（2026-09-17）

被 `Write` / `Edit` 动过的文件在两个地方出现，且**两处共用 `.file-link` 这一个类名与同一套委托点击**：

| 位置 | 内容 | 形态 |
|---|---|---|
| 工具卡头部 `.tool-cmd` | 可点 `.file-link`（**完整路径**）+ `.tool-cmd-rest`（该次调用的其余入参，压暗、超 160 字符截断） | 行内替换原来的纯文本摘要 |
| 对话流末尾 `.changed-files-card` | 「N 个文件已改动」，`<details>` 折叠，展开是 `.changed-file` 列表：左侧 `.file-link` 显示**文件名**、右侧 `.changed-file-dir` 显示所在目录 | 只在 `sessionChangedFiles` 非空时存在；每次更新 `appendChild` 到 `#chat-log` 末尾（复用节点即自动移到末尾），新回合开始后仍贴在最新内容之后 |

- **点击 → `reveal_in_explorer(path)`**：调 `explorer.exe /select,<path>` 打开所在文件夹并选中该文件。**只定位、不打开**（不是「用默认程序打开文件」）。失败（文件已被移动 / 删除）时把状态行文案换成 `agent.reveal_failed`，**不弹窗、不清列表**。
- **路径只来自 `tool_use` 入参**（`Write` / `Edit` 的 `file_path`），不是从工具输出正文里解析 —— 详见 ai-spec §11 规则 32（含为什么不能猜）。
- **`.file-link` 一律不设固定宽度、`word-break: break-all`**（工具卡里路径可能很长）；列表里的**目录段才是被截断的那一段**（`text-overflow: ellipsis`），文件名必须完整可读。
- **列表内容与界面留下的历史一致**：`steps` 快照每条带可选 `path`（`SessionStep.path`），`restoreSession()` / `rollbackChat()` 用 `rebuildChangedFilesFromSteps()` 重建、新对话清空列表。工具卡上的路径**不写回 agent 上下文**（纯前端展示）。
- **不新增滚动容器**：面板不过度增长（每文件一行）、不设 `max-height`；将来若要设，走 §5.4 那条唯一的全局 `::-webkit-scrollbar`，**禁止**在此声明 `scrollbar-width` / `scrollbar-color`。

---

## 4. 命令运行方式（Trae「沙箱」的 Lunac 等价物）

### 4.1 诚实原则（写进 UI 与文档）

Trae 的沙箱是 **OS 级受限执行环境**（macOS `sandbox-exec` / Windows 自研沙箱 SDK：限定可写目录、越权由操作系统拦截）。
**Lunac 没有这层能力**，现有手段是**策略级**的：

| 手段 | 现有实现 | 强度 |
|---|---|---|
| 写类工具先审批 | `can_use_tool` + 前端审批卡（`--permission-prompt-tool stdio`） | 询问，非隔离 |
| 工作区锁 | `LUNAC_WORKSPACE_LOCKED=1`，路径词法规范化后越界即拒（审批通过也不放行） | 硬边界（仅文件类工具） |
| 安全档位 | `security_profile` = `safe`(plan/只读) / `project`(默认) / `full`(忽略工作区锁) | 后端已就绪、前端缺 UI |
| 危险命令拦截 | `CMD_BLACKLIST` 正则 + 强制手动确认 | 拦截，可绕过 |
| 命令白名单 | `BUILTIN_SAFE_PREFIXES` + localStorage 白名单 | 提高便利性 |

因此 UI 与文案中 **禁止使用「沙箱」一词**，统一叫 **「命令运行方式」** 与 **「文件边界」**。理由：把策略级边界包装成隔离沙箱，会让用户对 `full` 档产生「反正有沙箱兜底」的误判 —— 这是安全承诺的失真。

### 4.2 运行方式三档（对齐 Trae 的三分法）

在 AI 输入栏新增一个**运行方式**控件（位置与形态见 §5.3），三档语义：

| 档位 | 行为 | 落到的既有机制 |
|---|---|---|
| **手动**（手动运行） | 每个写类工具（Write / Edit / Bash / PowerShell）都弹审批卡 | `security_profile=project` + 审批；**手动档连白名单命中也要问**（这是它与「白名单」档的唯一差别，见 §4.2 末） |
| **白名单**（信任前缀自动运行，默认档 = 现状） | 命中内置安全前缀或用户白名单 → 自动放行；其余仍弹卡 | 现有白名单逻辑，仅**显性化**为可选档位 |
| **自动**（全部自动运行，高风险） | 不再弹卡 | `security_profile=full`（`--dangerously-skip-permissions`）。**必须**二次确认弹窗 + 顶部常驻警示标识 |

- `safe`（只读）档**不作为运行方式选项**，它属于「我不给它动手」的另一种模式，归入设置面板的安全档位里（§4.4）。
- 运行方式存 `localStorage` 键 `lunac-agent-run-mode`（`manual` / `allowlist` / `auto`），切换即调 `set_security_profile` 对应值，并重启 agent（与现有工具黑名单的「重启生效」路径一致）。
- **手动档必须"照问不误"**：若手动档也让白名单自动放行，它与「白名单」档就完全等价、三档退化成两档。前端 `classifyRequest()` 在 `manual` 下直接返回「不自动放行」（危险命令仍走 `CMD_BLACKLIST` 拦截，优先级最高）。
- **控件形态**：运行方式在输入栏的 ⋯ 菜单里是「**图标 + 三格点阵**」，不写文字（位置与画法见 §5.3）；档位名与提示句只出现在 `title` / `aria-label`。
- 切到 **自动** 时：**不做全宽警示条**，改为在实际切换点就地提示 —— ① 运行方式行右侧的 `.chat-more-hint` 换成红字常驻警示（`agent.run_mode_auto_warning`：命令不再询问、`full` 档会忽略工作区锁）；② 该行的图标按钮与输入栏的 `#chat-more-btn` 一并标红（`.run-mode-auto`），菜单收起时也能一眼看出「自动运行开着」。二次确认同样就地内联在 `.chat-more-hint` 里（`.run-mode-confirm`），不再单独占一行。

### 4.3 越界与拦截的用户选项（对齐 Trae 的「跳过 / 运行 / 加白名单」）

当工作区锁拒绝一次文件操作（工具返回拒绝文案）时，在对应 `.tool-row` 内提供：

- **跳过**：默认行为，收起该卡片。
- **改到工作区内重试**：把路径提示回填给输入框（用户可改后重发），**不**自动放宽边界。
- **加入命令白名单**：仅对**命令类**工具出现，且**仅当该命令不含危险模式**时可用（`classifyRequest` 判定为 danger 的命令**永不显示此按钮**，与现状审批卡一致）。**三种情况额外不给此按钮**（2026-09）：
  1. agent 判定 `analysis.dangerous` 非空（执行侧静态分析，见 ai-spec §11 规则 26）；
  2. agent 判定 `analysis.opaque` 非空（含变量 / 编码执行等**无法静态判定**的成分）；
  3. 命令词是**解释器 / 启动器**（`cmd` / `powershell` / `bash` / `python` / `node` / `npx` / `iex` / `env`…）—— 白名单是前缀匹配，放进去等于「以后任何 `powershell …` 都自动放行」。同一条限制在**命中用户白名单**时也要生效（历史遗留的这类条目必须拒绝自动放行）。

危险命令被拦截时，卡片显示一行可读原因（复用中文标签，如「递归强制删除」；标签可能来自 agent 的 `analysis.dangerous`，也可能是前端 `CMD_BLACKLIST`），与 Trae 的「拦截原因可见」对齐。含无法静态判定成分时，标题处显示一个 `--yellow` 的 **⚠**（`.approval-warn-inline`，tooltip 说明原因）—— 它不是危险命令，但**不会被自动放行**。

### 4.4 安全档位（补前端入口）

**已落地（2026-09）**：设置面板「AI」分区有**安全档位**下拉（只读 / 项目 / 完全）。下发点是唯一的 —— 面板只广播 `lunac-security-profile-changed`，由 `main.ts` 的 `setSecurityProfile()` 调 `set_security_profile`（见 ai-spec §11 规则 21），与运行方式共用同一条 IPC，避免一次切换重启两遍 agent。与运行方式的关系写成一句说明，避免两个控件语义打架：

- 运行方式 = **问不问**（频率）；
- 安全档位 = **允不允许**（边界）：只读档直接拒绝写类工具，完全档忽略工作区锁。

---

## 5. 视觉规范（主题与图标适配）

### 5.1 只用既有变量，不引入新色

- 文字：`--text`（正文）/ `--text-dim`（次要，如元信息）/ `--text-muted`（极弱，如占位）
- 面：`--surface-glass`（卡片底）、`--surface-glass-hover`（hover）、`--border-glass`（描边）
- 语义：`--accent` + `--accent-bg` + `--accent-border`（进行中/可交互）、`--green`（成功）、`--red`（失败/危险）、`--yellow`（警告/非零退出码）、`--blue`（信息/链接）
- 圆角：`--radius`；间距按现有 4 / 6 / 8 / 12px 体系，不新造尺度。
- **不引入主题切换**：当前只有一套深色主题，本轮不新增浅色/主题预设（要做是独立需求，会牵动全部既有样式）。

### 5.2 图标（严格按 icon-style.md）

- 画布 24、`fill:none`、`stroke:currentColor`、`stroke-width:2.2`、圆头圆角、显示 12px（行内/按钮）。
- 允许残影/点彩装饰（偏移 -0.6~-0.7、opacity 0.28）。
- **功能按钮禁用 emoji**；本次需把承担状态语义的 emoji 换成 SVG：思考 `💭`、工具 `🔧`。`📋`（待办）与 `💬`（历史）若作为纯装饰可暂留，但同一功能块内不得 SVG 与 emoji 混用。
- 复用优先级：`COPY_SVG` / `RETRY_SVG` / `ROLLBACK_SVG` / `pluginIconSvg()` 已有路径 → 新增图形（思考、终端、折叠箭头、警示）才新建命令式常量，命名 `*_SVG`，风格与既有常量一致。

### 5.3 控件落位（不改视窗，只动输入栏内部）

AI 输入栏 `#chat-input-bar` 现有：附件、新建会话、**「更多」⋯ 菜单（2026-09 合并）**、历史、发送/停止。

- **2026-09 修订（用户要求）**：思考开关、命令运行方式、工作区、工具黑名单**四个控件合并进一个 ⋯ 图标按钮**（`#chat-more-btn`），并把它排在**输入框（`#chat-input`）之后**、历史按钮之前。原先并排的 `#chat-mode-btn` / `#chat-runmode-btn` 两个文字胶囊与 `.chat-workspace-wrap` / `.chat-tools-wrap` 两个独立弹窗全部移除。
- 菜单（`#chat-more-menu`）自下向上展开、右对齐，内部四行（行间分隔线）：
  1. 思考开关 —— 两档**分段控件**（开 / 关），当前档高亮。**只有两档**：端点没有思考力度旋钮（预算不被 enforce、`effort` 字段被静默忽略），做三档等于承诺端点做不到的事，依据见 [ai-spec.md](./ai-spec.md) §3.5「思考开关跨模型自适应」。状态栏等处用完整说法（「思考开 / 思考关」）而不是单个「开 / 关」；
  2. 命令运行方式 —— **图标 + 三格档位点阵**（见下），点一下进一档；
  3. 工作区 —— 行内显示当前路径 + 「选择目录 / 清除」，**不嵌套弹窗**；
  4. 工具黑名单 —— 行内列出工具，每行左侧是 **×/✓ 图标切换按钮**（× = 已禁用、红色 + 工具名加删除线；✓ = 启用、暗色），点一下切一格，点「保存」才写回并重启 agent。
- **运行方式不写文字**：`#chat-runmode-btn` 由「盾牌图标 + 3 个圆点」构成，点阵数量 = 档位（1 手动 / 2 白名单 / 3 自动），颜色区分语义（手动 `--text-dim`、白名单 `--green`、自动 `--red`）；完整语义（档位名 + 提示句）只在 `title` / `aria-label` 里，避免输入栏被文字挤满。
- **禁止**把控件放到 `#status-bar`（那里已有 token 面板与状态提示，且高度受限）。
- 图标（⋯、盾牌、文件夹、扳手、思考灯泡）一律按 `docs/icon-style.md`：24 栅格、`fill:none`、`stroke:currentColor`、`stroke-width:2.2`、圆头端点，含「残影偏移 −0.7 / opacity 0.28 + 点彩高光」；**思考行图标复用既有的 `THINK_SVG` 常量**，不重复造路径。

### 5.4 统一细滚动条（**优先级高，不得回退**）

**项目内所有可滚动容器必须共用同一套细滚动条外观**，任何容器都不允许出现 WebView2 / 系统默认滚动条。实现**只有一处**：`styles.css` 里的全局 `::-webkit-scrollbar` 规则（宽/高 4px、轨道透明、滑块 `rgba(255,255,255,0.18)`、`border-radius: 2px`、hover `0.32`）。

三条硬约束：

1. **禁止再写 `scrollbar-width` / `scrollbar-color`**（任何地方，包括 `settings.ts` 这类动态注入的 `<style>`）。Chromium（WebView2）只要看到这两个属性取非 `auto` 值，就会**忽略 `::-webkit-scrollbar`**，滚动条立刻退回系统默认外观。历史事故：`#chat-input` / `.tool-result` 上的那两条声明把自定义样式整个架空，看起来就是「WebView2 默认滚动条」。
2. **禁止按容器单独声明滚动条样式**（一定会漏 —— 新增容器就退回默认样式）。新容器只要写 `overflow-y: auto` 就自动获得统一外观，**不需要**任何额外 CSS。
3. **不要"藏"滚动条**（`overflow: hidden` 或把滚动条伪元素设 `display:none`）；需要可滚动就保留 `overflow-y: auto`。

> 依据：用户 2026-09 明确要求「⋯ 菜单里的滚动条要和项目里所有滚动条同一样式，不要 WebView2 默认样式」，并要求把这条规范的优先级调高。对应 [ai-spec.md](./ai-spec.md) §11 规则 21 的「不得回退」项。

---

## 6. 交互状态机（agent 面板）

```
idle ──发送──▶ thinking ──工具调用──▶ tool_running ──需审批──▶ awaiting_approval
                  ▲                        │                        │
                  │                        │◀──── 允许 / 拒绝 ──────┘
                  └──── 下一次请求 ◀────────┘
                  │
                  └──完成──▶ done ──▶ （回合折叠）
                  └──出错──▶ error（正文区保留，状态栏 + .sys-note 双呈现）
```

规则：
- 状态切换时**只更新状态栏与块状态**，不重排已完成块（避免布局抖动，code-rules §4.2/§4.3）。
- 流式期间禁止整段重绘：文本仍用增量 `textContent +=`（现状），新增的卡片只做**局部**属性更新。
- 解析退出码/耗时失败的路径必须**降级为不显示**，不得抛错、不得打断渲染。
- 思考块惰性渲染（§3.2）不得影响 `streamId` 守卫与 `cliSawStreamDelta` 双渲染守卫（`main.ts` L723、L4330）。

---

## 7. i18n 规范

新增 key 一律 `agent.*`（沿用既有前缀），每个 key 需 5 语言齐备：

- 思考：`agent.thinking_running`、`agent.thinking_done`（含 `{n}` 字数插值）
- 工具卡：`agent.tool_running`、`agent.tool_ok`、`agent.tool_exit_nonzero`、`agent.tool_timeout`、`agent.tool_skipped`、`agent.tool_exit_code`、`agent.tool_elapsed`、`agent.tool_copy_cmd`、`agent.tool_copy_output`、`agent.tool_show_all`
- 回合折叠：`agent.turn_summary`、`agent.turn_expand`、`agent.turn_collapse`、`agent.turn_elapsed`
- 运行方式：`agent.run_mode`、`agent.run_mode_manual`、`agent.run_mode_allowlist`、`agent.run_mode_auto`、`agent.run_mode_auto_warning`、`agent.run_mode_auto_confirm`
- 边界/拦截：`agent.boundary_blocked`、`agent.boundary_reason`、`agent.boundary_retry_in_workspace`、`agent.boundary_add_allowlist`
- 运行端输出：`agent.runtime_output`（`.sys-note-error` 的 summary）
- 安全档位：`settings.security_profile`、`settings.security_profile_ro/project/full` + 说明句

检查项：新增 key 必须在 i18n.ts 的 DICT 中同时补 `zh-CN` / `en`（最低要求），并尽量补齐 zh-TW / ja / ko（项目 5 语言现状）。

---

## 8. 窗口与视窗约束（复核清单）

实施后必须仍满足：

- [ ] `WIN_WIDTH`、缩放逻辑（0.6–2.5）、`#app.plugin-active` 的高度分支（360 / 600 / 520）**未被改动**。详细搜索大界面是**并列的固定档** `640 × zoom`（`DETAIL_HEIGHT`，见 ai-spec §2.1.2），不参与上述分支的复用。
- [ ] AI 态高度仍是**离散直设**（`requestWindowHeight` / `animateWindowHeight` 的 `!pluginActive && !detailOpen` 条件未被放宽），未把插件态或详细搜索态拉进滑动动画。
- [ ] 滚动仍由 `#results-list` 承担；新增长内容的块都有内部折叠，不出现「整体高度被撑爆」。
- [ ] 未新增任何 `setSize` 调用；未在快速交互路径上引入逐帧测量。
- [ ] 抽屉（历史）与 `#context-menu` 的 `overflow` 行为未被新的 `overflow:hidden` 破坏（code-rules §5.1）。
- [ ] 未使用原生 `<select>`（透明窗口不渲染，code-rules §5.2）——运行方式/安全档位必须用既有 `.custom-select` 或胶囊菜单。

---

## 9. 数据与 IPC 契约

- **本轮不改 `stream-json` 协议**。前端展示所需信息按 §3.3 从前端自有时间戳与现有 `tool_result` 文本推导。
- 若后续确需结构化字段（例如后端直接给 `exit_code` / `duration_ms` / `timed_out`），必须：
  1. 在 ai-spec §3.5 的 stdout 契约表登记字段名与语义；
  2. 保持 `tool_result.content` 文本不变（旧消费者仍可读）；
  3. 前端**兼容缺字段**（无字段时回落到文本解析）。
- **已登记的结构化字段**（按上述流程走完的）：
  | 字段 | 位置 | 语义 | 前端行为 |
  |---|---|---|---|
  | `analysis.dangerous` / `analysis.opaque` | `control_request.request`（`can_use_tool`） | 命令的**执行侧**静态安全分析（ai-spec §3.5「命令静态安全分析」/ §11 规则 26） | `dangerous` 非空 ⇒ 弹危险卡且不给「始终允许」（任何档位）；`opaque` 非空 ⇒ 不自动放行（fail-closed）。**缺字段时回落到 `CMD_BLACKLIST` 正则**（兼容旧 agent） |
  | `elided` / `dropped` | `system/context_compacted` | 压缩计数 | 面板 tooltip 归因（§5.2） |
  | `attempt` / `max_retries` / `error_status` | `system/api_retry` | 瞬时失败重试 | 状态行文案（ai-spec §11 规则 25） |
  | `usage.requests[]` = `{in, read, create, out}` | `result.usage` | **每次 API 请求**的用量明细（顺序 = 请求顺序） | 前端**只落盘**（写进 `usage-*.jsonl` 的 `requests` 数组，与 DeepSeek 平台用量页逐行对账）；表盘口径不变（仍按每次提问累加）。**缺字段时写空数组**，`UsageRecord.requests` 为空则不写该键（旧记录兼容）。见 ai-spec §3.5「用量与对账」 |
- 新增前端 → 后端的调用（如 `set_security_profile` 已有、`log_frontend` 已有）必须参数名与 Rust 签名逐一对齐（code-rules §3.1），并确认是否需要 `capabilities/default.json`（自定义 `#[tauri::command]` 不需要，插件 API 需要）。

---

## 10. 分期实施与验收

### 阶段 1（低风险，纯前端，建议先做）

1. 思考块规范化：i18n 文案、省略策略、惰性渲染、SVG 图标替换 `💭`。
2. 命令卡片：状态 / 退出码 / 耗时 / 折叠输出 / 复制；`cli-stderr` 落成 `.sys-note-error`。
3. 回合自动折叠 + 开关。
4. 验收：`npx tsc --noEmit` + `npm run build` 通过；长回合（≥10 次工具调用）下窗口高度与滚动正常；无新增 `setSize` 调用。

### 阶段 2（含后端接线）

5. 运行方式三档控件 + `set_security_profile` 接线（含「自动」档二次确认与常驻警示）。
6. 安全档位设置项 + 与运行方式的关系说明。
7. 越界/拦截卡片的三个动作。
8. 验收：三档各自跑一次真实工具往返（**按 ai-spec §11 规则 16 用 `deepseek-flash`**）；确认「自动」档下危险命令仍被 `CMD_BLACKLIST` 拦下并去掉「始终允许」；工作区锁在 `project` 档仍拒绝越界（审批通过也不放行）。

### 阶段 3（评估，不在本轮承诺）

9. 代码变更 diff 卡（Write/Edit 的变更预览与接受/拒绝）、DiffView 汇总。
10. 多轮缩略导航 / 会话 Fork。
11. （若要做真沙箱）Windows 侧隔离手段调研：Job Object 资源限制、低完整性级别令牌、AppContainer；结论需单独立文档，不得与本文的「运行方式」混称。

### 全局验收

- [ ] 5 语言文案齐备，无硬编码中文。
- [ ] 所有新图标符合 icon-style.md §4 检查清单。
- [ ] 视窗与容器结构未变（§8 全项通过）。
- [ ] 更新 ai-spec：§6 文件结构（如新增前端模块）、§11 新增/修订规则（AI 面板折叠与运行方式、与规则 14 的衔接）、§11 规则 20 的日志衔接。
- [ ] 视觉自查：在 800px 宽、默认缩放、深色毛玻璃背景下，与搜索结果区/插件面板的观感一致（同一套 `--surface-glass` / `--border-glass` / `--text-dim` 语言）。

---

## 11. 与既有规范的关系

- 本文 **不覆盖** ai-spec §11 规则 14（审批与工作区锁是硬边界）与规则 4（窗口高度/搜索性能）：两者是本文 §0、§8 的上位约束。
- 本文 **不新增** 主题机制（§5.1）；`docs/icon-style.md` 是图标唯一来源。
- 与 ai-spec §11 规则 20（落盘日志）互补：日志是「事后取证」，本文 §3.4 的 `.sys-note-error` 是「当场可见」，两者都要有。
