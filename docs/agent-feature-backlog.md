# agent.exe 待实现功能清单（对照旧 cli.exe）

> **用途**：把「旧 cli.exe 有、自研 agent.exe 没有」的能力集中登记为待实现项，并说明每一项对 Lunac（Windows 桌面启动器 + AI 对话）的实际价值，避免重复考古 `core/`。
>
> **状态基线**：agent.exe 目前 = P0 多轮循环/流式/用量 + P1 六件工具 + P2 权限审批，见 [ai-spec.md §3.5](file:///d:/cc/claude-code-cli-master/docs/ai-spec.md)。
>
> **核对口径**（本文的「全集」从这三个真源枚举，不是靠目录名猜的）：
> 1. 工具：[core/tools.ts](file:///d:/cc/claude-code-cli-master/core/tools.ts) `getAllBaseTools()` —— 旧 CLI 自己标注的「ALL tools 的唯一真源」
> 2. 斜杠命令：[core/commands.ts](file:///d:/cc/claude-code-cli-master/core/commands.ts) `COMMANDS` 数组
> 3. 子系统：`core/` 顶层目录结构 + 各子系统入口文件
>
> **最后核对时间**：2026-09-11

---

## 0. 差距总览（数字口径）

| 维度 | 旧 cli.exe | agent.exe 现在 | 缺口 |
|---|---|---|---|
| 工具 | 37 个具名（其中默认启用约 21 个）+ 15 个已置空的历史工具 | 6 个 | 31 个具名工具 |
| 斜杠命令 | 75+ | 0 | 全部 |
| 命令行开关 | 123+ 个 `--flag` | 6 个（`--add-dir` / `--permission-mode` / `--permission-prompt-tool` / `--dangerously-skip-permissions` / `--disallowedTools` + 忽略其余） | ~117 个 |
| 顶层子系统 | ~22 | 4（Agent 循环、工具执行、权限审批、上下文预算/压缩） | ~18 |
| stdout 消息类型 | 8 类 | 8 类 | 仅差 `system/api_retry`（无害） |
| 上下文压缩 | `services/compact/` 全套 | 两级压缩 + 400 兜底（不额外调模型） | 差「调模型摘要」式变体，见 §2.1（已不再是可用性缺口） |

---

## 1. 工具层缺口

### 1.1 组 A —— 对 Lunac 有真实价值（建议实现）

| 工具 | 旧 CLI 位置 | 价值 | 说明 |
|---|---|---|---|
| `PowerShell` | `core/tools/PowerShellTool/` | **高** | 我们是 Windows 应用却只有 `Bash`（= `cmd /C`）。前端 `classifyRequest` / `findLastBashGroup` / 危险命令黑名单**已经为 `"PowerShell"` 写好分支**，加上即生效 |
| `WebSearch` | `core/tools/WebSearchTool/` | **高** | 无联网检索；前端搜索栏已有「Web 搜索」入口，但 Agent 侧搜不了 |
| `WebFetch` | `core/tools/WebFetchTool/` | **高** | 抓取指定 URL 正文（含域名预批准 `preapproved.ts`），是「让它读文档」的前提 |
| `AskUserQuestion` | `core/tools/AskUserQuestionTool/` | **高** | 结构化提问（多选项）—— 现在只能靠模型在文本里问，用户没法点选 |
| `TodoWrite` | `core/tools/TodoWriteTool/` | **中高** | 长任务的进度可见性；旧 CLI 用它支撑多步任务，不需要任务框架也能用 |
| `Skill` | `core/tools/SkillTool/` | **中高** | P4 计划内；设置面板已有「技能扩展」且 UI 文案声称 agent.exe 会加载 —— 目前不读，属明显缺口 |
| `ListMcpResourcesTool` / `ReadMcpResourceTool` / `mcp`（动态工具代理） | `core/tools/MCPTool/`、`core/services/mcp/` | **中高** | P3 计划内；设置面板的「插件 (MCP 工具)」与 `<exe 根>\tools\*.json` 现在完全空转 |
| `Agent`（子代理，legacy 名 `Task`） | `core/tools/AgentTool/` | 中 | 长任务并行探索；代价是要配一整套子代理生命周期，收益不如上面几项直接 |
| `TaskCreate` / `TaskGet` / `TaskUpdate` / `TaskList`（任务 v2）与 `TodoWrite` 二选一 | `core/tools/Task*Tool/` | 中 | 选了 `TodoWrite` 就不必再上任务 v2 |
| `EnterPlanMode` / `ExitPlanMode` | `core/tools/Enter/ExitPlanModeTool/` | 中 | 先出计划再动手；需前端配合新增计划卡片 UI（目前前端**零相关代码**） |
| `TaskOutput` / `TaskStop` | `core/tools/TaskOutputTool/`、`TaskStopTool/` | 低 | 依赖后台任务框架，只有做了 `Agent`/后台 Bash 才有意义 |
| `NotebookEdit` | `core/tools/NotebookEditTool/` | 低 | Jupyter 场景，Lunac 用户群不大 |
| `StructuredOutput` | `core/tools/SyntheticOutputTool/` | 低 | 强制 JSON schema 输出，主要给 SDK 用 |

### 1.2 组 B —— 有条件才有意义（暂不实现）

| 工具 | 前提条件 |
|---|---|
| `LSP`（`core/tools/LSPTool/` + `core/services/lsp/`） | 需要一个常驻语言服务器；桌面启动器无 IDE 上下文 |
| `EnterWorktree` / `ExitWorktree` | 需要 git worktree 工作流 |
| `CronCreate` / `CronDelete` / `CronList`（`core/tools/ScheduleCronTool/`） | 需要后台常驻调度（`core/utils/cron*.ts`）；Lunac 已有 Windows 计划任务做自启，不必复制 |
| `Config`（ant-only） | 改 CLI 自身设置 |
| `SendUserMessage` / `Brief`、`SendUserFile`、`PushNotification` | 面向远端/移动端的推送通道 |

### 1.3 组 C —— 明确不做（旧 CLI 的协作/企业能力，与 Lunac 无关）

`TeamCreate` / `TeamDelete` / `SendMessage` / `ListPeers`（多代理团队，`core/utils/swarm/`、`core/utils/teammate*.ts`）、`RemoteTrigger`、`Monitor`、`VerifyPlanExecution`、`Workflow`、`SubscribePR` / `SuggestBackgroundPR`、`WebBrowser`（浏览器控制）、`TerminalCapture`、`OverflowTest` / `CtxInspect` / `Snip` / `Sleep`（调试与实验）、`REPL`（ant 专用 VM 工具）、`TestingPermission`（测试用）。

> 这些在 `getAllBaseTools()` 里大多已被显式置为 `null` 停用，属旧 CLI 自己的历史包袱。

---

## 2. 子系统层缺口

### 2.1 ⚠️ 组 A —— 会真实影响可用性（建议优先）

| 子系统 | 旧 CLI 位置 | 说明 |
|---|---|---|
| **上下文压缩 / 长度预算** | `core/services/compact/`（`autoCompact` / `microCompact` / `apiMicrocompact` / `snipCompact` / `sessionMemoryCompact`）、`core/query/tokenBudget.ts`、`core/utils/tokenBudget.ts` | agent.exe 的 `history` **无上限、无压缩**：会话变长后每轮都会因超上下文而失败，且失败即 `history.truncate(base)` 回滚 → **对话永久卡死，只能重开**。这是当前最该补的一项 |
| **模型输出重试** | `core/services/api/withRetry.ts`、前端已解析的 `system/api_retry`（[main.ts L2978-2980](file:///d:/cc/claude-code-cli-master/app/src/main.ts#L2978-L2980)） | 我们只有「thinking 参数 400 降级」，网络抖动/5xx/429 一律直接失败 |
| **Bash 静态安全分析** | `core/tools/BashTool/`（`bashParser.ts`、`bashSecurity.ts`、`sedValidation.ts`、`readOnlyValidation.ts`、`destructiveCommandWarning.ts`）、`core/utils/bash/` 整套 AST 解析 | 我们的危险命令判定**全在前端正则黑名单**（[main.ts L2577-2591](file:///d:/cc/claude-code-cli-master/app/src/main.ts#L2577-L2591)），能被引号/变量/管道绕过；旧 CLI 在 agent 侧做 AST 级判定 |
| **写文件前的安全扫描** | `core/security/index.ts`（`scanContent()`，`core/security/patterns.ts`） | 写入前扫凭据/危险模式，我们现在没有 |

### 2.2 组 B —— 有价值但依赖前置项

| 子系统 | 旧 CLI 位置 | 依赖/说明 |
|---|---|---|
| MCP 全栈 | `core/services/mcp/`（stdio / sse / http / WebSocket 传输、tools、resources、prompts、roots、elicitation、OAuth、`.mcp.json`） | = P3；是 §1.1 里三个 MCP 工具的前提 |
| Skills（含 inline / fork / remote 三模式） | `core/skills/`、`core/tools/SkillTool/` | = P4；`core/skills/bundled/` 里旧 CLI 自带 12+ 个内置技能 |
| 插件市场 / 插件命令 | `core/plugins/`、`core/utils/plugins/` | 桌面端的「插件」面板目前只读展示 `list_tool_files`，没有下发通道 |
| 权限 hooks（19 类事件） | `core/services/tools/toolHooks.ts`、`core/utils/hooks/`、`core/schemas/hooks.ts`、`core/hooks/useCanUseTool.tsx` | PreToolUse / PostToolUse / SessionStart / PreCompact / PermissionRequest … 供用户脚本介入 |
| 自动权限分类器 | `core/utils/permissions/`（`bashClassifier.ts`、`yoloClassifier.ts`、`classifierDecision.ts`） | 自动判定「这条命令能不能不问」；我们现在靠前端白名单前缀 |
| 会话持久化 / resume / rewind | `core/utils/sessionStorage.ts`、`sessionRestore.ts`、`fileHistory.ts`、`core/commands/rewind` | **前端不依赖**（会话历史由前端 localStorage 自持，`session_id` 恒空），故价值低 |
| 记忆目录（CLAUDE.md 体系） | `core/memdir/`、`core/utils/claudemd.ts`、`core/commands/memory` | 跨会话记忆；Lunac 已有 `ModuleData` 体系，可另设计 |
| 图片 / PDF / 附件多模态输入 | `core/utils/attachments.ts`、`imagePaste.ts`、`pdf.ts`、`FileReadTool/imageProcessor.ts`、`xlsxReader.ts` | 前端只把附件**路径**拼进文本让 agent 自己读；真正的图片内容块未实现 |
| 输出样式 / statusline | `core/outputStyles/`、`core/constants/outputStyles.ts` | CLI 的终端样式体系，Lunac 用 WebView 替代 |

### 2.3 组 C —— 明确不做（CLI 形态的产物）

| 子系统 | 位置 | 为什么不做 |
|---|---|---|
| Ink TUI 全套 | `core/ink/`、`core/components/`、`core/screens/REPL.tsx` | CLI 自己的终端渲染器，已被 WebView 取代 |
| 75+ 斜杠命令 | `core/commands/` | 多数是 CLI 会话内操作（`/theme` `/vim` `/statusline` `/login` `/upgrade` `/doctor` …），桌面端另有 UI |
| Vim 模式 / 语音 / buddy / chrome | `core/vim/`、`core/voice/`、`core/buddy/`、`core/commands/chrome` | 与 Lunac 交互形态无关 |
| 桥接 / 远程控制 / teleport | `core/bridge/`、`core/utils/teleport.tsx`、`core/commands/bridge` | 面向 Claude 云端会话接管 |
| 遥测 / 成本统计 | `core/utils/telemetry/`、`cost-tracker.ts`、`core/services/analytics/` | 旧 CLI 的运营与计费上报；前端连 `total_cost_usd` 都不读 |
| OAuth / 账号 / 订阅额度 | `core/services/oauth/`、`core/utils/auth.ts`、`commands/login`、`extraUsage` | Lunac 用自己的 API Key 直连供应商 |
| 自动更新 / 安装器 | `core/utils/autoUpdater.ts` | 由 NSIS 安装包负责 |
| 代理 / 证书 / mTLS / bedrock / aws | `core/proxy/`、`core/upstreamproxy/`、`core/utils/{proxy,mtls,caCerts,aws,bedrock}.ts` | 企业网关场景 |
| IDE 集成（VSCode / JetBrains / Desktop） | `core/utils/ide.ts`、`jetbrains.ts`、`claudeDesktop.ts` | Lunac 已有自己的 VSCode 扩展（`vscode-extension/`） |
| 其余 utils 级实现细节 | `core/utils/`（约 200 文件：`ripgrep.ts`、`glob.ts`、`fileRead.ts`、`bashParser.ts` …） | 我们已用 6 个工具 + glob/regex crate 覆盖同等能力，只是实现更薄 |

---

## 3. 协议 / 接口层缺口

| 项 | 现状 | 影响 |
|---|---|---|
| `system/api_retry` | 未发（前端有解析分支） | 无重试可上报；本身无害 |
| `system/task_started` / `task_progress` | 未发（前端有解析与文案） | 子代理进度文案永不出现 |
| `control_cancel_request` | 仅审批超时时发 | 已够用 |
| `session_id` | 恒为 `""` | 前端不读，无影响 |
| `stop_reason` | 未提供（前端写死 `end_turn`） | 无影响 |
| `total_cost_usd` / `num_turns` / `duration_ms` | 已发但前端零引用 | 无影响 |
| 123+ 命令行开关 | 只解析 6 个 | 其余（`--resume` / `--settings` / `--agents` / `--mcp-config` / `--allowedTools` / `--max-turns` / `--json-schema` …）均被忽略 |

---

## 4. 已核对：以下不是缺口

- **前端依赖的 stdout 契约**：`system/init`、`stream_event`（4 种 delta）、`assistant`、`user/tool_result`、`control_request`、`result`（含 4 个 token 字段）—— agent.exe **全部已提供**，token 面板数据源正常。
- **工具名硬编码**：前端特判的 `Bash`（及 `PowerShell`）命名一致；六件工具名字与旧 CLI 完全同名同义。
- **`--disallowedTools` 链路**：Rust → agent.exe → 请求体过滤已通，只是 UI 的黑名单候选列表全是旧工具名（见 §5 待清理）。
- **思考档位跨模型自适应**：旧 CLI 没有对应机制（它绑定自家模型），我们反而是超集。

---

## 5. 顺带要清理的「空转 UI / 失实文案」（不是新功能，但会造成误解）

| 项 | 位置 | 处理 |
|---|---|---|
| 工具黑名单候选列表全是旧工具名，用户**无法禁用**真实六件工具 | [main.ts L1525-1549](file:///d:/cc/claude-code-cli-master/app/src/main.ts#L1525-L1549)、Rust [DEFAULT_TOOL_BLACKLIST](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/commands.rs#L323-L340) | 换成真实工具名 |
| 「技能扩展」文案声称 agent.exe 会加载该目录，实际不读 | [settings.ts L386-446](file:///d:/cc/claude-code-cli-master/app/src/plugins/builtin/settings.ts#L386-L446)、i18n `settings.skills_*` | 随 P4 落地，或先改文案 |
| 「插件 (MCP 工具)」面板只读展示，无下发通道 | [settings.ts L448+](file:///d:/cc/claude-code-cli-master/app/src/plugins/builtin/settings.ts#L448) | 随 P3 落地 |
| 安全档位 `set_security_profile` 无前端入口 | `app/src-tauri/src/commands.rs` `set_security_profile` | 设置面板加 safe/project/full 切换 |

---

## 6. 建议实施顺序

1. ~~**上下文预算 + 压缩**（§2.1）——唯一「用久了必然坏掉」的缺口，属防回归性质~~ ✅ **已完成（2026-09）**
2. **P3 MCP 工具桥** —— 让「插件」面板与 `tools\*.json` 真正生效
3. **P4 Skills** —— 让「技能扩展」面板生效，并修掉失实文案
4. **低成本高收益**：`PowerShell` 工具、工具黑名单候选列表刷新、`WebFetch`/`WebSearch`、`AskUserQuestion`、`TodoWrite`
5. 视需要：权限 hooks、自动权限分类器、模型输出重试、Bash AST 安全分析

---

## 7. 完整性声明

**统计边界**：本文按「用户可见能力」三级粒度枚举 —— **工具名**（取自 `getAllBaseTools()`，共 37 具名 + 15 置空）、**斜杠命令**（取自 `COMMANDS`，75+）、**子系统**（取自 `core/` 顶层目录）。因此：

- ✅ 可以确认：**在旧 CLI 代码里能枚举出的工具/命令/子系统层面，没有遗漏项**。凡 `core/` 里存在的顶层子系统，均已在上文 §1–§3 归位（实现 / 不做 / 归并为实现细节）。
- ⚠️ 边界一：`core/utils/` 下约 200 个文件属**实现细节级**能力（如 `ripgrep.ts`、`fileHistory.ts`、`ansiToPng.ts`），本文按子系统归并，不再逐文件列；其中少数有独立价值（`fileHistory` = rewind、`attachments` = 多模态）已单列。
- ⚠️ 边界二：`core/tools/` 里带 `UI.tsx` 的文件是终端渲染组件，随 Ink TUI 一起不做，未计入缺口。
- ⚠️ 边界三：本清单只覆盖「旧 cli.exe 已有」的对照面；**Lunac 自身的新需求**（如多模态输入、本地模型、插件市场）不在本文范围，应另立路线。
