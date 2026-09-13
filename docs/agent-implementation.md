# Lunac Agent 实现文档

> **定位**：本文描述 Lunac 自研 agent（`agent.exe`）的**实现现状与演进路线**。
> 它回答三个问题：① 现在实现了什么；② 还差什么（含 25 个未复刻具名工具、未复刻的 core 子系统）；③ 用户怎么用本地文件扩展它（`skills\` / `tools\`）。
>
> **关联**：[ai-spec.md](./ai-spec.md) §3.5（协议契约）/ §11（硬约束规则）、[agent-feature-backlog.md](./agent-feature-backlog.md)（待办清单 + 实施顺序）、[agent-ui-spec.md](./agent-ui-spec.md)（对话面板 UI）。
>
> **最后核对时间**：2026-09-13

---

## 1. 定位：这是「一个完整的 agent 类应用」，不是启动器的附属功能

Lunac 的 AI 对话**不是**「桌面启动器顺手带的一个小助手」。它的既定目标是成为一个**可以完全类比于完整 agent 类应用**的能力体：自带工具链、权限闸门、上下文管理、本地扩展（技能 / 工具）、多轮工具往返 —— 用户应当能像使用一个独立 agent 产品那样使用它。

这条定位有两个直接推论，**写在这里是为了防止后续把自己的能力边界定得过窄**：

1. **不能因为「宿主是桌面启动器」就判定某项能力「用不上」。** 早先的文档里把旧 CLI 的多代理协作、子代理、任务系统、计划模式等一律归为「与 Lunac 无关」—— 那是错的：它们是「完整 agent 类应用」的组成部分，只是**前置能力未到**（例如没有子代理框架，`TaskOutput` / `TaskStop` 就无从谈起）。正确的说法是**优先级排序**，不是**价值否定**。
2. **缺什么要写在明面上。** 「agent 已全部复刻」不成立（见 §4）。凡是尚未实现的能力，都要在本文件登记、在 backlog 里排期，不能靠"用户没提"来默认不需要。

---

## 2. 架构与数据根

| 项 | 说明 |
|---|---|
| 形态 | `agent.exe`（Rust，约 2.5MB），**独立进程**，由 `lunac.exe` 的 `start_cli` 以子进程方式拉起 |
| 通信 | stdin/stdout 上的 **stream-json**（NDJSON），契约见 [ai-spec.md](./ai-spec.md) §3.5；agent 的 stdout **只走协议**，日志一律落盘 |
| 数据根 | **便携模式**：一律 `<exe 根>`（`current_exe()` 所在目录），实现 dev/release 物理隔离与卸载彻底化 |
| 关键路径 | `skills\`（技能）、`tools\`（用户工具定义）、`ModuleData\`（用量日志等）、`temp\logs\`（落盘日志） |
| 环境注入 | 端点 / token / 模型 / 思考档位 / 安全档位 / 工作区 / `LUNAC_SKILLS_DIR` / `LUNAC_LOG_DIR` **只在 spawn 时注入**；切换这些项 = `kill_and_cleanup()` 重启 agent |
| 源码 | [core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs)（主循环）、[tools.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/tools.rs)、[skills.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/skills.rs)、[mcp.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/mcp.rs)、[log.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/log.rs) |

---

## 3. 已实现能力

| 阶段 | 能力 | 关键实现 |
|---|---|---|
| **P0** | 多轮上下文、SSE 增量打字、用量上报（input / output / cacheRead / cacheCreate）、错误回传（失败轮整体回滚历史） | `run_query` 主循环；用量口径见 ai-spec §3.5「用量与对账」 |
| **P0** | 思考档位跨模型自适应 | `MAX_THINKING_TOKENS` → `Thinking` 形态；400 沿降级链 `enabled+budget → adaptive → 不带字段` 重试一次并缓存结果 |
| **P1** | 内置工具 + `tool_use` / `tool_result` 往返循环 | 11 件内置工具，见 §4.1 |
| **P2** | 权限审批：写类工具发 `can_use_tool` → 阻塞等前端回包（超时按拒绝） | 与工作区锁是**与**关系（ai-spec §11 规则 14） |
| **P2** | 上下文预算 + 两级压缩 + 400 兜底 | 水位 0.85 / 0.95 + 滞回；只瘦身 / 丢弃 / 强制三档，见 ai-spec §3.5 与 §11 规则 23 |
| **P3** | MCP 工具桥 | 把 `<exe 根>\tools\*.json` 的用户工具以 `mcp__<名>` 接进请求体 |
| **P4** | 技能（渐进披露） | `LUNAC_SKILLS_DIR` 下 `<key>/SKILL.md`；提示词只列 `key: 描述`，模型调 `Skill` 取正文 |
| **UI** | AI 对话面板（思考省略 / 命令卡片 / 回合折叠 / 运行方式三档 / 用量面板） | 规范见 [agent-ui-spec.md](./agent-ui-spec.md) |
| 运维 | 落盘日志（两进程各写 `temp\logs\{agent,lunac}-YYYY-MM-DD.log`，含每次工具调用与耗时） | ai-spec §11 规则 20 |

---

## 4. 工具全景

### 4.1 已实现（11 件内置 + `Skill` + MCP 动态代理）

| 工具 | 说明 |
|---|---|
| `Bash` / `PowerShell` | 命令执行（PowerShell 走 `-NoProfile -NonInteractive -Command` + 双 UTF-8 兜底） |
| `Read` / `Write` / `Edit` | 文件读写与精确替换编辑 |
| `Glob` / `Grep` | 文件与内容检索 |
| `WebSearch` | 联网检索（主源 bocha/tavily/exa/firecrawl，兜底 Bing/百度抓取） |
| `WebFetch` | 抓取网页并转纯文本 |
| `TodoWrite` | 长任务进度面板（**唯一常驻免审批工具**） |
| `AskUserQuestion` | 结构化提问（答案经 `can_use_tool` 的 `updatedInput` 回传） |
| `Skill` | 取回本地技能正文（渐进披露的取回端） |
| `mcp__*` | `<exe 根>\tools\*.json` 的用户工具，动态接入 |

### 4.2 未复刻的具名工具

> 口径说明：本节从 [core/tools.ts](file:///d:/cc/claude-code-cli-master/core/tools.ts) `getAllBaseTools()`（旧 CLI 自称的「ALL tools 唯一真源」）逐个核对。**扣掉已实现的 12 件后仍有 39 个名字**；其中一部分在旧 CLI 里已被显式置空或只在特定构建/环境变量下出现（下表标「条件」）。backlog §0 的「25 个」是「默认构建下真正注册的」口径。

**A. 前置能力到位后就有价值**

| 工具 | 前置 / 说明 |
|---|---|
| `Agent`（旧名 `Task`） | **子代理框架**。这是多个工具的总前置：`TaskOutput` / `TaskStop` / `TaskCreate` 系列都依赖它做长任务并行探索 |
| `TaskOutput` / `TaskStop` | 依赖后台任务框架 |
| `TaskCreate` / `TaskGet` / `TaskUpdate` / `TaskList` | 任务 v2。与 `TodoWrite` 功能重叠，已选 `TodoWrite`（可视为**已覆盖**） |
| `EnterPlanMode` / `ExitPlanMode` / `VerifyPlanExecution` | 计划模式闭环，需前端配套计划卡片 UI |
| `ListMcpResources` / `ReadMcpResource` | MCP resources。需先给 `mcp_server.rs` 补 `resources/read`（现只有 `resources/list`） |
| `ToolSearch` | 工具过多时按需检索工具定义（条件） |

**B. 面向「完整 agent 类应用」的能力拓展**

| 工具 | 说明 |
|---|---|
| `SendMessage` / `ListPeers` / `TeamCreate` / `TeamDelete` | 多代理协作（团队、消息、成员发现）。**不再定性为「与 Lunac 无关」** —— 它是完整 agent 应用的组成部分，排在子代理之后 |
| `NotebookEdit` | Jupyter 场景编辑 |
| `EnterWorktree` / `ExitWorktree` | git worktree 工作流 |
| `WebBrowser` | 浏览器控制（条件） |
| `LSP` | 语言服务器（需常驻 IDE 上下文，条件） |
| `CronCreate` / `CronDelete` / `CronList` | 后台定时任务（Lunac 已有 Windows 计划任务做自启，能力不重叠） |
| `SendUserFile` / `PushNotification` / `Brief` | 向用户推送文件 / 通知 / 简报（远端通道） |

**C. 旧 CLI 的历史包袱 / 实验性（条件或已置空）**

`Config`（ant-only）、`REPL`（ant-only VM）、`Workflow`、`RemoteTrigger`、`Monitor`、`SubscribePR` / `SuggestBackgroundPR`、`TerminalCapture`、`Snip`、`Sleep`、`StructuredOutput`、`OverflowTest` / `CtxInspect`、`TestingPermission`。

> 这一组不否定「将来可能需要」，只是它们要么绑定旧 CLI 的特定形态（Ink TUI / ant 内部构建）、要么是调试与实验开关。随能力补齐再逐项评估。

---

## 5. 本地扩展（用户可直接引用）

发布包会在 `<exe 根>` 下预置两个目录（纯模板，**不会自动加载**）：

```
<exe 根>\
  skills\
    README.md                     ← 技能格式与生效方式说明
    _example\SKILL.md.example     ← 照抄模板（刻意不叫 SKILL.md，不会被加载）
  tools\
    README.md                     ← 工具 JSON 格式与安全说明
    example-tool.json.example     ← 照抄模板（刻意不叫 .json，不会被加载）
```

> 模板的源文件在仓库 [agent-templates/](file:///d:/cc/claude-code-cli-master/agent-templates)，由 [build-release.ps1](file:///d:/cc/claude-code-cli-master/build-release.ps1) 拷进暂存目录、由 [lunac-installer.nsi](file:///d:/cc/claude-code-cli-master/scripts/lunac-installer.nsi) 打进安装包。

### 5.1 技能（`skills\`）

- 布局：`skills\<技能名>\SKILL.md`，frontmatter 含 `name` / `description`，正文为 Markdown。
- **渐进披露**：系统提示词里**只列 `key: 描述`**（描述 ≤ 250 字符、清单总预算 8000 字符），模型需要时才调 `Skill` 工具取回正文，可用 `$ARGUMENTS` 接收调用参数。
- 清单**按 `key` 排序**后拼进提示词 —— 顺序抖动等于废掉整段前缀缓存（ai-spec §11 规则 18）。
- 生效：设置 ·「技能扩展」面板增删改后自动重启 agent；手工改目录需重启 Lunac。
- 未做：技能的 **fork**（在子代理里跑）与 **remote**（远端拉取）两种模式。

### 5.2 工具（`tools\`）

- 布局：`tools\*.json`，一份文件一个工具；字段 `name` / `description` / `inputSchema` / `handler`。
- `handler.type` 三选一：`shell`（执行命令）/ `http`（发请求）/ `builtin`（Lunac 进程内实现）。
- 参数占位符 `{{参数名}}`（兼容 `{{ 参数名 }}`）。
- 接入后工具名一律 `mcp__<原名>`；**任何档位下调用都先弹审批卡**；只读档（`plan`）**不接入**。
- 数组**按工具名排序**后才进请求体（同上，前缀缓存不变量）。
- 生效：新增 / 修改后需重启 agent（工具编辑器面板保存后会自动重启）。

---

## 6. 尚未复刻的 core 子系统

| 优先级 | 子系统 | 现状与差距 |
|---|---|---|
| **高** | 模型输出重试 | 只有「thinking 参数 400 降级」；网络抖动 / 5xx / 429 一律直接失败。前端已有 `system/api_retry` 解析分支，agent 侧未发该事件 |
| **高** | Bash 静态安全分析 | 危险命令判定**全在前端正则黑名单**，可被引号 / 变量 / 管道绕过；旧 CLI 在 agent 侧做 AST 级判定（`bashParser` / `bashSecurity` / `sedValidation` / `readOnlyValidation`） |
| **中高** | 写文件前安全扫描 | 旧 CLI 写入前扫凭据与危险模式（`core/security/scanContent()`），现在没有 |
| **中高** | 多代理 / 子代理框架 | 见 §4.2 组 A、B —— 这是「完整 agent 类应用」的核心缺口 |
| **中高** | MCP 全栈 | 已通：stdio + tools。缺：远程传输（sse / http / ws）、resources / prompts / roots / elicitation / OAuth、`.mcp.json` |
| **中** | 权限 hooks（19 类事件） | PreToolUse / PostToolUse / SessionStart / PreCompact / PermissionRequest … 供用户脚本介入 |
| **中** | 自动权限分类器 | 自动判定「这条命令能不能不问」；现在只有前端白名单前缀 + `CMD_BLACKLIST` |
| **中** | 多模态输入 | 前端只把附件**路径**拼进文本让 agent 自己读；真正的图片 / PDF 内容块未实现 |
| **中** | 计划模式闭环 | 需前端配套计划卡片 UI（目前前端零相关代码） |
| **低** | 会话持久化 / resume / rewind | 会话历史由前端 localStorage 自持，价值低 |
| **低** | 记忆目录（CLAUDE.md 体系） | 跨会话记忆；Lunac 已有 `ModuleData` 体系，可另设计 |
| **低** | 插件市场 / 插件命令 | 目前只有只读列表 + 安装入口 |
| **低** | 输出样式 / statusline | CLI 终端样式体系，已被 WebView 取代 |

### 6.1 协议 / 接口层

| 项 | 现状 | 影响 |
|---|---|---|
| `system/api_retry` | 未发（前端有解析分支） | 无重试可上报；与「模型输出重试」绑定 |
| `system/task_started` / `task_progress` | 未发（前端有解析与文案） | 子代理进度文案永不出现；与子代理框架绑定 |
| 命令行开关 | 只解析 6 个（`--add-dir` / `--permission-mode` / `--permission-prompt-tool` / `--dangerously-skip-permissions` / `--disallowedTools` / `--mcp-server`） | 其余 ~116 个被忽略；多数绑定 CLI 形态，按需再评估 |
| 斜杠命令 | 0 / 75+ | 多数是 CLI 会话内操作（`/theme` `/vim` `/statusline` …），桌面端另有 UI —— **按需逐项评估**，不整体照搬 |

---

## 7. 实施顺序（当前判断）

1. **模型输出重试** —— 唯一「网络一抖就整轮失败」的可用性缺口，且前端解析分支已就绪。
2. **Bash 静态安全分析** —— 现有正则黑名单是安全边界上的已知短板。
3. **子代理框架（`Agent`）** —— 解锁 `TaskOutput` / `TaskStop` / 多代理协作一整组能力，是「完整 agent 类应用」的关键一步。
4. MCP `resources` 两件（需先扩 `mcp_server.rs`）、写文件前安全扫描、权限 hooks、多模态输入。
5. 其余按 §4.2 / §6 逐项评估。

> 具体排期与勾选状态以 [agent-feature-backlog.md](./agent-feature-backlog.md) 为准；本节的顺序是在 2026-09-13 的判断。
