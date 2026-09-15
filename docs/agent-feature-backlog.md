# agent.exe 待实现功能清单（对照旧 cli.exe）

> **用途**：把「旧 cli.exe 有、自研 agent.exe 没有」的能力集中登记为待实现项，并说明每一项对 Lunac（Windows 桌面启动器 + AI 对话）的实际价值，避免重复考古 `core/`。
>
> **状态基线**：agent.exe 目前 = P0 多轮循环/流式/用量 + P1 十一件内置工具 + P2 权限审批 + P3 MCP 工具桥 + P4 技能 + 上下文预算/压缩 + **瞬时失败重试**（2026-09），见 [ai-spec.md §3.5](file:///d:/cc/claude-code-cli-master/docs/ai-spec.md)。
>
> **核对口径**（本文的「全集」从这三个真源枚举，不是靠目录名猜的）：
> 1. 工具：[core/tools.ts](file:///d:/cc/claude-code-cli-master/core/tools.ts) `getAllBaseTools()` —— 旧 CLI 自己标注的「ALL tools 的唯一真源」
> 2. 斜杠命令：[core/commands.ts](file:///d:/cc/claude-code-cli-master/core/commands.ts) `COMMANDS` 数组
> 3. 子系统：`core/` 顶层目录结构 + 各子系统入口文件
>
> **本文只回答「还差什么、先做什么」**；已实现能力的全景、工具清单全表、以及本地 `skills\` / `tools\` 的扩展格式，见 [agent-implementation.md](./agent-implementation.md)。
>
> **定位前提（2026-09-13 修正）**：Lunac 的 AI agent 目标是**一个可以完全类比于完整 agent 类应用**的能力体，不因「宿主是桌面启动器」而降级。因此下列分组只是**优先级**，不是**价值否定** —— 旧版把多代理协作等写成「与 Lunac 无关」是错的，已改。
>
> **最后核对时间**：2026-09-11（§0 差距总览）、2026-09-13（定位修正、瞬时失败重试、命令静态安全分析落地）、2026-09-15（新增 §8「Lunac 自身新目标」五项 + Hermes 调研）

---

## 0. 差距总览（数字口径）

| 维度 | 旧 cli.exe | agent.exe 现在 | 缺口 |
|---|---|---|---|
| 工具 | 37 个具名（其中默认启用约 21 个）+ 15 个已置空的历史工具 | 11 个内置 + `Skill`（技能）+ 动态接入的 MCP 用户工具（`mcp__*`，来自 `tools\*.json`） | 25 个具名工具 |
| 斜杠命令 | 75+ | 0 | 全部 |
| 命令行开关 | 123+ 个 `--flag` | 7 个（`--add-dir` / `--permission-mode` / `--permission-prompt-tool` / `--dangerously-skip-permissions` / `--disallowedTools` / `--mcp-server` + 忽略其余） | ~116 个 |
| 顶层子系统 | ~22 | 6（Agent 循环、工具执行、权限审批、上下文预算/压缩、MCP 桥、技能） | ~16 |
| stdout 消息类型 | 8 类 | 9 类 | 无（`system/api_retry` 已于 2026-09 补发） |
| 上下文压缩 | `services/compact/` 全套 | 两级压缩 + 400 兜底（不额外调模型） | 差「调模型摘要」式变体，见 §2.1（已不再是可用性缺口） |

> **怎么读这张表**：缺口数 = **能力差距**，不是「用不上」。目标是让 Lunac 的 agent 成为一个可以完全类比于完整 agent 类应用的能力体（见 [agent-implementation.md](./agent-implementation.md) §1）；本文剩下要做的只是**排序**，不是**筛掉**。

---

## 1. 工具层缺口

### 1.1 组 A —— 对 Lunac 有真实价值（建议实现）

| 工具 | 旧 CLI 位置 | 价值 | 说明 |
|---|---|---|---|
| ~~`PowerShell`~~ | `core/tools/PowerShellTool/` | **高** | ✅ **已完成（2026-09）**：`-NoProfile -NonInteractive -Command` + 双 UTF-8 编码兜底（中文输出不再变乱码），与 `Bash` 共用 `run_shell`；`plan` 档拒绝、非 plan 档先审批，真机烟测通过 |
| ~~`WebSearch`~~ | `core/tools/WebSearchTool/` | **高** | ✅ **已完成（2026-09，同日重构）**：旧实现是 Anthropic 服务端的 `web_search_20250305` server tool（`WebSearchTool.ts:76-84`），全仓无本地搜索源，**不能照搬**。自研方案：主源 = 设置面板的「搜索服务商 + 密钥」（`AI_SEARCH_PROVIDER`/`AI_SEARCH_KEY` → `LUNAC_SEARCH_PROVIDER`/`LUNAC_SEARCH_KEY`，可选 **bocha**（博查，国内直连、微信扫码即可注册）/ tavily / exa / firecrawl）；兜底 = **Bing RSS → Bing HTML → 百度 HTML 抓取**（无 key、≥1.1s 节流、202/429 视为限流），主源失败或未配齐时自动回落并附 `[fallback] <原因>`，三级全失败就如实报错不编造。**换掉 DuckDuckGo 的原因**：本机实测国内连不上 `html.duckduckgo.com`（15s 超时），而 Bing / 百度均 200 —— 「不配 key 也能搜」必须是真的。**Bing Search API 已于 2025-08-11 退役**（老 key 410 Gone、不再接受新注册），**DuckDuckGo 无官方搜索 API**（`api.duckduckgo.com` 只返维基摘要），故不存在「bing 首选 + ddg api 兜底」这条路。关键：`WebSearch` 进 `needs_approval` 与 `gated_in_read_only`（查询词是外部出口，plan 档也弹审批）。见 [ai-spec.md §3.5](file:///d:/cc/claude-code-cli-master/docs/ai-spec.md)「联网检索」 |
| ~~`WebFetch`~~ | `core/tools/WebFetchTool/` | **高** | ✅ **已完成（2026-09）**：HTML→纯文本抓取（无 DOM 依赖）、60s 超时 / ≤10MB / UA 标识 Lunac；**未照搬两处服务端依赖** —— 域名预检 `api.anthropic.com/api/web/domain_info` 与 Haiku 二次摘要。只读档同样走审批（唯一外部数据出口），烟测见 [ai-spec.md §9](file:///d:/cc/claude-code-cli-master/docs/ai-spec.md) |
| ~~`AskUserQuestion`~~ | `core/tools/AskUserQuestionTool/` | **高** | ✅ **已完成（2026-09）**：**答案复用 `can_use_tool` 的 `updatedInput` 回传**（旧 CLI 也是这条通道，不新增协议）；前端 `renderAskQuestions()` 渲染选项、`classifyRequest()` 对它恒定 `auto:false`；收不到答案时工具报错而非编造。plan 档可用且照常审批。见 [ai-spec.md §3.5](file:///d:/cc/claude-code-cli-master/docs/ai-spec.md)「结构化提问」 |
| ~~`TodoWrite`~~ | `core/tools/TodoWriteTool/` | **中高** | ✅ **已完成（2026-09）**：长任务的进度可见性；工具**不持有状态**（清单唯一真相 = 模型最近一条 `tool_use`），前端拿流式入参就地重绘 `.todo-panel`；**免审批**、成功回执不重复渲染。见 [ai-spec.md §3.5](file:///d:/cc/claude-code-cli-master/docs/ai-spec.md)「待办面板」 |
| ~~`Skill`~~ | `core/tools/SkillTool/` | **中高** | ✅ **已完成（P4，2026-09）**：渐进披露（提示词只列 `key: 描述`，模型调 `Skill` 取正文，`$ARGUMENTS` 已替换），见 [ai-spec.md §3.5](file:///d:/cc/claude-code-cli-master/docs/ai-spec.md)；fork / remote 两种模式未做 |
| `ListMcpResourcesTool` / `ReadMcpResourceTool` / `mcp`（动态工具代理） | `core/tools/MCPTool/`、`core/services/mcp/` | **中高** | ✅ **动态工具代理已完成（P3，2026-09）**：`<exe 根>\tools\*.json` 的用户工具以 `mcp__<名>` 进请求体，见 [ai-spec.md §3.5](file:///d:/cc/claude-code-cli-master/docs/ai-spec.md)。`ListMcpResourcesTool` / `ReadMcpResourceTool` 两件仍未做（`mcp_server.rs` 只实现了 `resources/list`） |
| `Agent`（子代理，legacy 名 `Task`） | `core/tools/AgentTool/` | 中 | 长任务并行探索；代价是要配一整套子代理生命周期，收益不如上面几项直接 |
| `TaskCreate` / `TaskGet` / `TaskUpdate` / `TaskList`（任务 v2）与 `TodoWrite` 二选一 | `core/tools/Task*Tool/` | 中 | **已选 `TodoWrite`（2026-09 落地）**，故不再上任务 v2 —— 后者要配一整套任务状态存储与生命周期，收益与 `TodoWrite` 重叠 |
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

### 1.3 组 C —— 需要前置能力 / 绑定旧 CLI 形态（按完整 agent 路线图排后，**不是不做**）

`TeamCreate` / `TeamDelete` / `SendMessage` / `ListPeers`（多代理协作，`core/utils/swarm/`、`core/utils/teammate*.ts`）、`RemoteTrigger`、`Monitor`、`VerifyPlanExecution`、`Workflow`、`SubscribePR` / `SuggestBackgroundPR`、`WebBrowser`（浏览器控制）、`TerminalCapture`、`OverflowTest` / `CtxInspect` / `Snip` / `Sleep`（调试与实验）、`REPL`（ant 专用 VM 工具）、`TestingPermission`（测试用）。

> **读法**：多代理协作（`TeamCreate` / `SendMessage` / `ListPeers`）是「完整 agent 类应用」的正当能力，**前置是子代理框架（`Agent`）** —— 框架落地后应重估，不再按「无关」处理。
> 其余各项绑定的分别是：旧 CLI 的 Ink TUI 形态、ant 内部构建、IDE 常驻上下文、远端推送通道、或调试开关。它们不是「没价值」，而是**当前没有对应形态的宿主**；随能力补齐逐项重估。
> 这些在 `getAllBaseTools()` 里大多已被显式置为 `null` 停用，属旧 CLI 自己的历史包袱 —— 照搬前先确认它在新宿主下还有意义。

---

## 2. 子系统层缺口

### 2.1 ⚠️ 组 A —— 会真实影响可用性（建议优先）

| 子系统 | 旧 CLI 位置 | 说明 |
|---|---|---|
| ~~**上下文压缩 / 长度预算**~~ | `core/services/compact/`（`autoCompact` / `microCompact` / `apiMicrocompact` / `snipCompact` / `sessionMemoryCompact`）、`core/query/tokenBudget.ts` | ✅ **已完成（2026-09）**：预算 + 0.85/0.95 双水位（带滞回）+ 400 强制压缩兜底，**不额外调模型**（见 ai-spec §3.5）。与旧 CLI 的差别只是缺「调模型做摘要」那一类变体，已不再是可用性缺口 |
| ~~**模型输出重试**~~ | `core/services/api/withRetry.ts`、前端已解析的 `system/api_retry` | ✅ **已完成（2026-09）**：网络抖动 / 429 / 5xx（含 529）**请求级**退避重试（1s→2s→4s + 抖动、30s 封顶、尊重 `Retry-After`），只在读到响应体前重试故**不产生重复内容**；4xx 不重试（400 的两个专门分支保留）；SSE 流中途断开不重试（只记日志）。补发 `system/api_retry`（`attempt` / `max_retries` / `error_status` / `delay_ms`）。见 [ai-spec.md §3.5](file:///d:/cc/claude-code-cli-master/docs/ai-spec.md)「瞬时失败重试」与 §11 规则 25 |
| ~~**Bash 静态安全分析**~~ | `core/tools/BashTool/`（`bashParser.ts`、`bashSecurity.ts`、`sedValidation.ts`、`readOnlyValidation.ts`、`destructiveCommandWarning.ts`）、`core/utils/bash/` | ✅ **已完成（2026-09，自研方案）**：旧 CLI 是一整套手写 bash 语法树（面向 unix/zsh，且其自身承认 `bash -c` / `cmd /c` / `powershell -Command` 无递归解析），**不照搬**。自研 [core-agent/src/bash_safety.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/bash_safety.rs)：子命令拆分（`;` `\n` `\|` `&`，引号感知）+ 引号/转义归一（`r""m`→`rm`，且不把 `C:\Windows` 揉成 `C:Windows`）+ 包装器递归（`cmd /c` / `powershell -Command` / `bash -c`，`-EncodedCommand` 判不透明）+ **Windows 危险规则集**（递归/强制删除、格式化与分区、覆写物理磁盘、注册表、bcdedit、vssadmin/wbadmin、关机重启、taskkill、icacls、账户/服务/计划任务、`iex`、git 强制推送/`reset --hard`/`clean -f`/`branch -D`…）+ **fail-closed**：变量/子表达式/编码执行/间接执行器/嵌套过深/控制字符 ⇒ 不得自动放行。结果随 `can_use_tool.analysis` 上报（见 §3 与 ai-spec §11 规则 26），前端以它为准、原正则降为二道网；解释器前缀永不进白名单 |
| **写文件前的安全扫描** | `core/security/index.ts`（`scanContent()`，`core/security/patterns.ts`） | 写入前扫凭据/危险模式，我们现在没有 |

### 2.2 组 B —— 有价值但依赖前置项

| 子系统 | 旧 CLI 位置 | 依赖/说明 |
|---|---|---|
| MCP 全栈 | `core/services/mcp/`（stdio / sse / http / WebSocket 传输、tools、resources、prompts、roots、elicitation、OAuth、`.mcp.json`） | **stdio + tools 部分已完成（P3，2026-09）**：agent 侧作 client 连本机 `lunac.exe --mcp-server`、注册并调用其工具。仍缺：远程传输（sse/http/ws）、resources/prompts/roots/elicitation/OAuth、`.mcp.json` 配置（目前工具来源只有 `<exe 根>\tools\*.json`） |
| Skills（含 inline / fork / remote 三模式） | `core/skills/`、`core/tools/SkillTool/` | **inline 模式已完成（P4，2026-09）**：`LUNAC_SKILLS_DIR` 的 `<key>/SKILL.md` 列进提示词 + `Skill` 工具取正文（含 `$ARGUMENTS` 替换）。仍缺：fork（子代理执行技能）、remote（远端拉取）、技能自带脚本/资源 |
| 插件市场 / 插件命令 | `core/plugins/`、`core/utils/plugins/` | 桌面端的「插件」面板目前只读展示 `list_tool_files`，没有下发通道 |
| 权限 hooks（19 类事件） | `core/services/tools/toolHooks.ts`、`core/utils/hooks/`、`core/schemas/hooks.ts`、`core/hooks/useCanUseTool.tsx` | PreToolUse / PostToolUse / SessionStart / PreCompact / PermissionRequest … 供用户脚本介入 |
| 自动权限分类器 | `core/utils/permissions/`（`bashClassifier.ts`、`yoloClassifier.ts`、`classifierDecision.ts`） | 自动判定「这条命令能不能不问」；我们现在靠前端白名单前缀 |
| 会话持久化 / resume / rewind | `core/utils/sessionStorage.ts`、`sessionRestore.ts`、`fileHistory.ts`、`core/commands/rewind` | **前端不依赖**（会话历史由前端 localStorage 自持，`session_id` 恒空），故价值低 |
| 记忆目录（CLAUDE.md 体系） | `core/memdir/`、`core/utils/claudemd.ts`、`core/commands/memory` | 跨会话记忆；Lunac 已有 `ModuleData` 体系，可另设计 |
| 图片 / PDF / 附件多模态输入 | `core/utils/attachments.ts`、`imagePaste.ts`、`pdf.ts`、`FileReadTool/imageProcessor.ts`、`xlsxReader.ts` | 前端只把附件**路径**拼进文本让 agent 自己读；真正的图片内容块未实现 |
| 输出样式 / statusline | `core/outputStyles/`、`core/constants/outputStyles.ts` | CLI 的终端样式体系，Lunac 用 WebView 替代 |

### 2.3 组 C —— 绑定旧 CLI 形态（排后，按需重估）

| 子系统 | 位置 | 为什么排后 / 何时重估 |
|---|---|---|
| Ink TUI 全套 | `core/ink/`、`core/components/`、`core/screens/REPL.tsx` | CLI 自己的终端渲染器，已被 WebView 取代；若要重估，先看它的**交互模式**（如 REPL 的批量工具调用）是否有价值，而不是照搬渲染层 |
| 75+ 斜杠命令 | `core/commands/` | 多数是 CLI 会话内操作（`/theme` `/vim` `/statusline` `/login` `/upgrade` `/doctor` …），桌面端另有 UI。**逐项评估**：与能力相关的（如 `/compact`、`/rewind`、memory 类）值得单列出来看，不要因为挂在这个目录下就一并丢弃 |
| Vim 模式 / 语音 / buddy / chrome | `core/vim/`、`core/voice/`、`core/buddy/`、`core/commands/chrome` | 当前交互形态没有对应入口 |
| 桥接 / 远程控制 / teleport | `core/bridge/`、`core/utils/teleport.tsx`、`core/commands/bridge` | 面向 Claude 云端会话接管；若将来做「远端会话接管」再重估 |
| 遥测 / 成本统计 | `core/utils/telemetry/`、`cost-tracker.ts`、`core/services/analytics/` | 旧 CLI 的运营与计费上报。**成本统计值得重估**：Lunac 已有按天用量日志（ai-spec §3.5），可在此基础上做本地成本面板，不必照搬上报链路 |
| OAuth / 账号 / 订阅额度 | `core/services/oauth/`、`core/utils/auth.ts`、`commands/login`、`extraUsage` | Lunac 用自己的 API Key 直连供应商 |
| 自动更新 / 安装器 | `core/utils/autoUpdater.ts` | 由 NSIS 安装包负责 |
| 代理 / 证书 / mTLS / bedrock / aws | `core/proxy/`、`core/upstreamproxy/`、`core/utils/{proxy,mtls,caCerts,aws,bedrock}.ts` | 企业网关场景；用户提需求再评估 |
| IDE 集成（VSCode / JetBrains / Desktop） | `core/utils/ide.ts`、`jetbrains.ts`、`claudeDesktop.ts` | Lunac 已有自己的 VSCode 扩展（`vscode-extension/`） |
| 其余 utils 级实现细节 | `core/utils/`（约 200 文件：`ripgrep.ts`、`glob.ts`、`fileRead.ts`、`bashParser.ts` …） | 我们已用 11 个内置工具 + glob/regex crate 覆盖同等能力，只是实现更薄；其中 `bashParser.ts` 已单列进 §2.1（安全短板） |

---

## 3. 协议 / 接口层缺口

| 项 | 现状 | 影响 |
|---|---|---|
| `system/api_retry` | **已发**（2026-09，瞬时失败退避重试时） | 无 |
| `system/task_started` / `task_progress` | 未发（前端有解析与文案） | 子代理进度文案永不出现 |
| `control_cancel_request` | 仅审批超时时发 | 已够用 |
| `session_id` | 恒为 `""` | 前端不读，无影响 |
| `stop_reason` | 未提供（前端写死 `end_turn`） | 无影响 |
| `total_cost_usd` / `num_turns` / `duration_ms` | 已发但前端零引用 | 无影响 |
| 123+ 命令行开关 | 只解析 6 个 | 其余（`--resume` / `--settings` / `--agents` / `--mcp-config` / `--allowedTools` / `--max-turns` / `--json-schema` …）均被忽略 |

---

## 4. 已核对：以下不是缺口

- **前端依赖的 stdout 契约**：`system/init`（含 `model`，用量日志的元数据来源）、`stream_event`（4 种 delta）、`assistant`、`user/tool_result`、`control_request`、`result`（含 4 个 token 字段，**每次提问的绝对值**）—— agent.exe **全部已提供**，token 面板数据源正常；对账口径见 [ai-spec.md §3.5](file:///d:/cc/claude-code-cli-master/docs/ai-spec.md)「用量与对账」。
- **工具名硬编码**：前端特判的 `Bash`（及 `PowerShell`）命名一致；十一件内置工具名字与旧 CLI 完全同名同义。
- **`--disallowedTools` 链路**：Rust → agent.exe → 请求体过滤已通，UI 黑名单候选列表已换成真实工具名（见 §5）。
- **思考开关跨模型自适应**：旧 CLI 没有对应机制（它绑定自家模型），我们反而是超集。注意**只有开 / 关两档**（端点无思考力度旋钮，见 [ai-spec.md](./ai-spec.md) §3.5），不要按「多档更深」的方向扩。

---

## 5. 顺带要清理的「空转 UI / 失实文案」（不是新功能，但会造成误解）

| 项 | 位置 | 处理 |
|---|---|---|
| ~~工具黑名单候选列表全是旧工具名，用户**无法禁用**真实内置工具~~ | [main.ts](file:///d:/cc/claude-code-cli-master/app/src/main.ts) `TOOL_BLACKLIST_CANDIDATES` | ✅ 已修（2026-09，第 19 点缓存优化）：候选换成真实内置工具名（含 `Skill`），Rust 侧删掉对自研 agent 全为空转的 `DEFAULT_TOOL_BLACKLIST` —— 勾选即真正从请求体 `tools` 裁掉，缩短前缀、提升缓存命中 |
| ~~「技能扩展」文案声称 agent.exe 会加载该目录，实际不读~~ | [settings.ts L386-446](file:///d:/cc/claude-code-cli-master/app/src/plugins/builtin/settings.ts#L386-L446)、i18n `settings.skills_*` | ✅ 文案已属实（P4，2026-09）：agent.exe 启动时读该目录，增删改后前端自动重启 agent 生效 |
| 「插件 (MCP 工具)」面板只读展示，无下发通道 | [settings.ts L448+](file:///d:/cc/claude-code-cli-master/app/src/plugins/builtin/settings.ts#L448) | ✅ 已随 P3 落地：agent.exe 现在会读 `<exe 根>\tools\*.json`（面板的安装/编辑/删除 + 重启 agent 流程已生效）；面板自身仍是只读列表 + 安装入口，编辑在 Tool Editor 插件里 |
| ~~安全档位 `set_security_profile` 无前端入口~~ | `app/src-tauri/src/commands.rs` `set_security_profile` | ✅ 已落地（2026-09）：设置面板「AI」分区有只读/项目/完全下拉，经 `lunac-security-profile-changed` → `main.ts` `setSecurityProfile()` 单一 IPC 下发（与运行方式共用，避免重启两遍） |

---

## 6. 建议实施顺序

1. ~~**上下文预算 + 压缩**（§2.1）——唯一「用久了必然坏掉」的缺口，属防回归性质~~ ✅ **已完成（2026-09）**
2. ~~**P3 MCP 工具桥** —— 让「插件」面板与 `tools\*.json` 真正生效~~ ✅ **已完成（2026-09）**；resources 两件未做
3. ~~**P4 Skills** —— 让「技能扩展」面板生效，并修掉失实文案~~ ✅ **已完成（2026-09）**；fork / remote 未做
4. ~~**低成本高收益**~~ ✅ **已全部完成（2026-09）**：~~`PowerShell` 工具~~、~~工具黑名单候选列表刷新~~（同步提升前缀缓存命中）、~~`WebFetch`~~、~~`AskUserQuestion`~~、~~`TodoWrite`~~、~~`WebSearch`~~（主源可配置多后端 + Bing RSS / HTML、百度三级免 key 兜底，见 §1.1）。§1.1 组 A 中仅剩 `ListMcpResourcesTool` / `ReadMcpResourceTool`（MCP resources，需先扩 `mcp_server.rs`）
5. ~~**模型输出重试**~~ / ~~**Bash 静态安全分析**~~ ✅ **已完成（2026-09）** —— 见 §2.1 与 [ai-spec.md §3.5](file:///d:/cc/claude-code-cli-master/docs/ai-spec.md) 的「瞬时失败重试」/「命令静态安全分析」。余下视需要：**写文件前安全扫描**、**子代理框架（`Agent`）**、权限 hooks、自动权限分类器
6. ~~**前缀缓存命中率优化**（第 19 点）~~ ✅ **已完成（2026-09）**：MCP 工具数组按名排序、技能清单按 `key` 排序、工具黑名单不再内置空转旧名 —— 不变量见 [ai-spec.md §11 规则 18](file:///d:/cc/claude-code-cli-master/docs/ai-spec.md)

---

## 7. 完整性声明

**统计边界**：本文按「用户可见能力」三级粒度枚举 —— **工具名**（取自 `getAllBaseTools()`，共 37 具名 + 15 置空）、**斜杠命令**（取自 `COMMANDS`，75+）、**子系统**（取自 `core/` 顶层目录）。因此：

- ✅ 可以确认：**在旧 CLI 代码里能枚举出的工具/命令/子系统层面，没有遗漏项**。凡 `core/` 里存在的顶层子系统，均已在上文 §1–§3 归位（实现 / 不做 / 归并为实现细节）。
- ⚠️ 边界一：`core/utils/` 下约 200 个文件属**实现细节级**能力（如 `ripgrep.ts`、`fileHistory.ts`、`ansiToPng.ts`），本文按子系统归并，不再逐文件列；其中少数有独立价值（`fileHistory` = rewind、`attachments` = 多模态）已单列。
- ⚠️ 边界二：`core/tools/` 里带 `UI.tsx` 的文件是终端渲染组件，随 Ink TUI 一起不做，未计入缺口。
- ⚠️ 边界三：本清单只覆盖「旧 cli.exe 已有」的对照面；**Lunac 自身的新需求**（如多模态输入、本地模型、插件市场）不在本文范围，应另立路线。

---

## 8. Lunac 自身新目标（2026-09-15 用户提出，不在 §1–3 的对照面内）

> 本节的五项**不是**「旧 cli.exe 有而我们没有」，而是用户直接提出的新方向，按 §7 边界三单独立节。
> 调研参照物 = **Hermes Agent**（Nous Research，MIT，`github.com/NousResearch/hermes-agent`）。两份证据：
> ① 本机源码检出 `C:\Users\15242\AppData\Local\hermes\hermes-agent\`（`hermes_state.py` 声明 `SCHEMA_VERSION = 16`，对应 0.17.x）；
> ② 在线文档与 release notes（截至 2026-09-07 的 v0.21.2「Pantheon」）。
> **版本差说明**：本机那份落后于 GitHub 约 4 个小版本，但「三层记忆」这个骨架在 v0.21 的官方文档里**没有变化**（仍是 `MEMORY.md` 2200 字符 + `USER.md` 1375 字符 + `state.db` FTS5 会话检索），故下文的机制引用对本机版本有效，不影响结论。v0.21 新增的是外围能力（Bot Mode、`hermes peer`、cron continuity、子代理 live steering）。

### 8.1 被改动文件的路径追踪（点击 → 打开所在文件夹）

| 项 | 内容 |
|---|---|
| 目标 | 工具卡里出现的「被改动的文件路径」可点击 → 资源管理器定位到该文件；并在面板里留一份「本次会话改动过的文件」列表（参照用户提供的 Trae 截图） |
| 现状 | 工具结果里的路径是**纯文本**（`.tool-row` 是 `<details>` 折叠块），零交互；前端也没有「改动过的文件」这个概念 |
| 关键约束 | **路径必须来自可信来源，不能靠前端正则从自由文本里猜** —— 否则「输出里提到的路径」会被误当「被改动的路径」，列表里混进一堆只读过的文件 |
| 落点选择 | 推荐用 `assistant` 消息里 `tool_use` 的**入参**（`Write` / `Edit` 的 `file_path`）—— 语义精确、**零协议改动**；次选是在 `tool_result` 里加结构化字段（要走 agent-ui-spec §9 的字段登记流程） |
| 后端 | 新命令走 `explorer.exe /select,<path>`（定位并选中）。**只接受绝对路径 + 存在性校验**，绝不接受任意命令行字符串 —— 与 [system_catalog.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/system_catalog.rs) 的 `run_action` 同一条纪律 |
| 依赖 | 无（纯前端 + 一个 Rust 命令），**五项里最容易先做** |

### 8.2 自动压缩上下文（调模型的摘要式压缩）

| 项 | 内容 |
|---|---|
| 目标 | 补上「调模型生成摘要」这一类压缩变体（见 §2.1 的差异项） |
| 现状 | 已有预算 + 0.85/0.95 双水位（带滞回）+ 400 兜底，但**不额外调模型**（机械 elide/drop 历史消息） |
| Hermes 参照 | 摘要模板首段固定为 `## Historical Task Snapshot`（`agent/context_compressor.py:37`），要求**逐字捕获用户最近一条未完成输入**，并显式写明「用户刚问了一个问题也算 active task，不要写 None」；交接前缀 `SUMMARY_PREFIX`（`context_compressor.py:45-62`）定死优先级：**latest user message WINS**，历史上那些 `Historical Task` / `In-Progress` / `Pending Asks` / `Remaining Work` 章节一律视为历史 |
| Hermes 预算 | `_MIN_SUMMARY_TOKENS = 2000`、`_SUMMARY_RATIO = 0.20`、`_SUMMARY_TOKENS_CEILING = 12_000`（`context_compressor.py:142-147`）；摘要失败降级兜底 `_FALLBACK_SUMMARY_MAX_CHARS = 8000`（`166-170`） |
| Hermes 会话策略 | **原地压缩为默认**：`compression.in_place` 默认 True → 旧轮 `active=0` 软归档、**session id 不变**（`agent/conversation_compression.py:347` + `hermes_state.py:2854 archive_and_compact`）；legacy 路径才 fork 新会话并写 `parent_session_id`（`conversation_compression.py:568-667`） |
| Lunac 硬约束 | ① 摘要要**真实花钱与耗时**（一次额外 API 调用）⇒ 只在「机械压缩已不足以腾空间」时才触发，要有单轮成本上限 + 可关闭开关；② 压缩**必然改写请求前缀 ⇒ 端点侧缓存整段作废**，与 ai-spec §11 规则 23 的命中率纪律直接冲突，所以**触发频次要尽量低**，宁可压得晚也不要压得勤；③ 不引 session 分裂（Lunac 的 `session_id` 恒为 `""`，前端也不读）|
| 依赖 | **8.3 先行** —— 摘要模板的核心就是任务快照 |

### 8.3 任务总结工具（压缩时不丢「本次对话的主要任务」）

| 项 | 内容 |
|---|---|
| 目标 | 压缩前后，「当前在做什么任务」这条信息必须存活 |
| Hermes 参照 | 就是 8.2 里那段 `Historical Task Snapshot`。**Hermes 并没有一个独立的「任务总结工具」** —— 它是压缩器提示词里的一段；而且 `agent/prompt_builder.py:144-165` 的 `MEMORY_GUIDANCE` 明确**禁止**把摘要/任务进度写进 `MEMORY.md`（任务快照只活在压缩摘要里，不污染长期记忆） |
| Lunac 设计要点 | 任务快照必须是**独立于会话消息的一条 pinned 上下文**（压缩不动它）；否则「压缩 → 快照也被压掉」= 白做 |
| 推荐方案 | **不新造工具**：前端已有 `TodoWrite` 的 `.todo-panel`，而 `TodoWrite` 的语义本来就是「当前任务清单」（清单唯一真相 = 模型最近一条 `tool_use`，见 §1.1）。压缩时**把最近一条 `TodoWrite` 的清单原样 pin 住**即可，成本远低于再引一个工具 |
| 依赖 | 无（可与 8.2 合并实现） |

### 8.4 一次对话中的多任务并行（多个任务各自走独立 API 请求）

| 项 | 内容 |
|---|---|
| 目标 | 一次对话里互不依赖的多个任务并发跑，而不是严格的一问一答 |
| 现状 | 只做了**只读工具并行**（`TOOL_PARALLELISM = 4`，ai-spec §11 规则 28）—— 那是「同一次 API 响应里的多个只读工具调用」并发；**模型请求本身仍然串行** |
| Hermes 参照 | `delegate_task` 子代理（v0.21 起支持 live steering：列出运行中的子代理、中途纠偏、停止并保留部分结果、子代理可用 JSON schema 校验返回值并单独成本核算）；旧 CLI 侧的对应物是 `Agent` 工具（见 §1.1） |
| Lunac 硬约束 | ① 每个任务须有**独立的消息数组与独立的工具执行环境**，否则「A 任务的写」会污染「B 任务的读」；② 结果回灌必须能**归因到任务** —— Hermes 靠 `session_id` 天然隔离，Lunac 的 `session_id` 恒为 `""`，**得先补一个内存态 task id**；③ 并发 = 花钱，必须有并发上限 + 预算封顶；④ 前端要能把多任务的流式输出**分栏/分组**，否则用户看到的是交织成一团乱的流 |
| 依赖 | `Agent`（子代理）框架 + 前端分栏 UI。**排在 8.2 / 8.3 之后** |

### 8.5 Lunac 自主学习往期对话的数据库

| 项 | 内容 |
|---|---|
| 目标 | 历史对话变成可检索的长期记忆，跨会话复用 |
| Hermes 参照（三层记忆） | ① **Prompt memory**：`~/.hermes/memories/MEMORY.md`（agent 自己的笔记，**2200 字符** ≈ 800 tokens）+ `USER.md`（用户画像，**1375 字符** ≈ 500 tokens），以**冻结快照**注入系统提示 —— 会话中途写入只落盘、**不改系统提示**（就是为了保住前缀缓存）；条目用 `§` 分隔；上限是**字符数**不是 token；`memory` 工具只有 `add` / `replace` / `remove`，**没有 `read`**（内容本来就在提示里）。② **Skills**：`~/.hermes/skills/<name>/SKILL.md`，可自主创建/改进。③ **Session search**：`~/.hermes/state.db`（SQLite），`messages` 表存全量消息 + **FTS5 虚拟表** `messages_fts(content)`（内容由触发器拼 `content + tool_name + tool_calls`）+ 第二张 `tokenize='trigram'` 的表专供 **CJK 子串检索**；工具是 `session_search`（discovery / window / bookend 三形态） |
| Hermes 自动写入机制 | **不是定时器**，是**每轮后台复盘 fork**：`memory.nudge_interval`（默认 10 轮）触发，fork 里**只允许 `memory` 与 `skill_manage` 两个工具**（`agent/background_review.py:401`） |
| Hermes 有意不做的 | 内核**不引向量库 / embedding**（无语义检索）；语义检索只存在于可插拔的外部 provider（Honcho / mem0 等）内部，且 `MemoryManager` **同时只允许 1 个外部 provider** |
| Lunac 映射与差距 | Lunac 现有历史是 `ModuleData\history\chat-history.json` —— **单个 JSON 文件**（Rust `load_chat_sessions` / `save_chat_sessions`），与 SQLite + FTS5 差一个数量级；历史变大后**全量读改写**本身就是性能问题 |
| **第一步（不是做检索 UI）** | 把历史存储换成 **SQLite + FTS5**。Lunac 是 Rust 宿主，**不引 Python 侧任何东西**，直接用 `rusqlite`（bundled）—— 立项前先验证该构建确实带 **FTS5**（bundled 默认含，但必须实测一次 `CREATE VIRTUAL TABLE … USING fts5` 通不通）；CJK 另起一张 `tokenize='trigram'` 表（照 Hermes 的做法，中文子串检索靠它） |
| 注入纪律 | 必须遵守前缀缓存纪律（ai-spec §11 规则 23）：会话中途写入只落盘、**不改系统提示**（Hermes 的冻结快照就是这个理由）；「自主学习」的触发点照抄**每轮后台复盘 fork**（不引定时器，按轮次门槛触发，fork 里只放白名单工具） |
| 依赖 | 8.3（任务快照）+ SQLite 迁移。**五项里最大的一项，建议最后做** |

### 8.6 建议顺序

1. **8.1 路径追踪** —— 无依赖、纯前端 + 一个 Rust 命令，用户直接可见
2. **8.3 任务快照** —— 8.2 的前置，可与 8.2 合并一次做完
3. **8.2 自动压缩（调模型摘要）** —— 注意「少压」优先于「压得干净」（命中率纪律）
4. **8.5 对话数据库** —— 先把存储换成 SQLite + FTS5，再做检索与记忆注入（工作量大，但 8.2 上线后会更需要它）
5. **8.4 多任务并行** —— 依赖子代理框架 + 前端分栏，最后做
