# Lunac Agent 实现文档

> **定位**：本文描述 Lunac 自研 agent（`agent.exe`）的**实现现状**与**用户可直接使用的本地扩展格式**。它回答两个问题：① 现在实现了什么；② 用户怎么用本地文件扩展它（`skills\` / `tools\`）。
>
> **待办不在这里**：本文只写「已经是什么样」。凡未完成的能力（子代理、MCP resources、Skills fork/remote、多模态、权限 hooks…）一律登记在 **[agent-feature-backlog.md](./agent-feature-backlog.md)**，本文不再重复维护缺口清单。
>
> **关联**：[ai-spec.md](./ai-spec.md) §3.5（协议契约）/ §11（硬约束规则）、[agent-feature-backlog.md](./agent-feature-backlog.md)（待办唯一真相源）、[agent-ui-spec.md](./agent-ui-spec.md)（对话面板 UI）。
>
> **最后核对时间**：2026-09-19（逐条对照 `core-agent/src/`、`app/src/`、`app/src-tauri/src/` 实测）

---

## 1. 定位：这是「一个完整的 agent 类应用」，不是启动器的附属功能

Lunac 的 AI 对话**不是**「桌面启动器顺手带的一个小助手」。它的既定目标是成为一个**可以完全类比于完整 agent 类应用**的能力体：自带工具链、权限闸门、上下文管理、本地扩展（技能 / 工具）、多轮工具往返 —— 用户应当能像使用一个独立 agent 产品那样使用它。

这条定位有两个直接推论，**写在这里是为了防止后续把自己的能力边界定得过窄**：

1. **不能因为「宿主是桌面启动器」就判定某项能力「用不上」。** 早先的文档里把旧 CLI 的多代理协作、子代理、任务系统、计划模式等一律归为「与 Lunac 无关」—— 那是错的：它们是「完整 agent 类应用」的组成部分，只是**前置能力未到**（例如没有子代理框架，`TaskOutput` / `TaskStop` 就无从谈起）。正确的说法是**优先级排序**，不是**价值否定**。
2. **缺什么要写在明面上。** 「agent 已全部复刻」不成立。凡是尚未实现的能力，都要在 [agent-feature-backlog.md](./agent-feature-backlog.md) 登记，不能靠「用户没提」来默认不需要。

---

## 2. 架构与数据根

| 项 | 说明 |
|---|---|
| 形态 | `agent.exe`（Rust，约 2.5MB），**独立进程**，由 `lunac.exe` 的 `start_cli` 以子进程方式拉起 |
| 通信 | stdin/stdout 上的 **stream-json**（NDJSON），契约见 [ai-spec.md](./ai-spec.md) §3.5；agent 的 stdout **只走协议**，日志一律落盘 |
| 数据根 | **便携模式**：一律 `<exe 根>`（`current_exe()` 所在目录），实现 dev/release 物理隔离与卸载彻底化 |
| 关键路径 | `skills\`（技能）、`tools\`（用户工具定义）、`ModuleData\`（`history\chat.db`、`usage\*.jsonl`）、`temp\logs\`（落盘日志） |
| 环境注入 | 端点 / token / 模型 / 思考开关 / 安全档位 / 工作区 / `LUNAC_SKILLS_DIR` / `LUNAC_LOG_DIR` **只在 spawn 时注入**；切换这些项 = `kill_and_cleanup()` 重启 agent。完整变量表（13 个 `LUNAC_*`）见 [agent-feature-backlog.md](./agent-feature-backlog.md) §6 |
| 源码 | [core-agent/src/main.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/main.rs)（主循环 + 上下文压缩/摘要）、[tools.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/tools.rs)、[skills.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/skills.rs)、[mcp.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/mcp.rs)、[bash_safety.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/bash_safety.rs)（命令静态安全分析 → 审批卡判据）、[log.rs](file:///d:/cc/claude-code-cli-master/core-agent/src/log.rs) |

---

## 3. 已实现能力

| 阶段 | 能力 | 关键实现 |
|---|---|---|
| **P0** | 多轮上下文、SSE 增量打字、用量上报（input / output / cacheRead / cacheCreate）、错误回传（失败轮整体回滚历史） | `run_query` 主循环；用量口径见 ai-spec §3.5「用量与对账」 |
| **P0** | 思考开关跨模型自适应 | `LUNAC_THINKING`（`off` / 其余=开）→ `Thinking` 形态；400 沿降级链 `enabled+budget → adaptive → 不带字段` 重试一次并缓存结果。**只有开 / 关两档** —— 端点无思考力度旋钮（预算不被 enforce、`effort` 被静默忽略），见 ai-spec §3.5 |
| **P1** | 内置工具 + `tool_use` / `tool_result` 往返循环 | 13 件内置工具，见 §4.1 |
| **P1** | 子代理框架（`Agent` 工具）：派生独立上下文的子代理跑自包含任务，主对话只收最终报告 | `run_subagent()` / `run_agent_tool()` / `subagent_tool_defs()`；**串行** + 轮次上限 8 + token 预算 30 万；工具集剔掉 `Agent` / `SessionSearch` / `mcp__*`（无桥的必然失败项）、生成参数与主循环同源（thinking + max_tokens）、系统提示词含环境块与技能清单、非流式；事件 `task_started` / `task_progress` / `task_done`。契约见 ai-spec §3.5「子代理」与 §11 规则 54 |
| **可用性** | 瞬时失败重试：网络抖动 / 429 / 5xx（含 529）退避重试，**请求级**（不产生重复内容），并补发 `system/api_retry` | `retryable_status` / `retry_delay_ms` / `emit_api_retry`；见 ai-spec §3.5「瞬时失败重试」与 §11 规则 25 |
| **安全** | 命令静态安全分析（Bash / PowerShell）：子命令拆分 + 引号/转义归一 + 包装器递归 + Windows 危险规则集 + **fail-closed** 不透明判定，结果随 `can_use_tool` 的 `analysis` 上报 | `bash_safety::analyze`（自研，单测 8 例）；见 ai-spec §3.5「命令静态安全分析」与 §11 规则 26 |
| **P2** | 权限审批：写类工具发 `can_use_tool` → 阻塞等前端回包（超时按拒绝） | 与工作区锁是**与**关系（ai-spec §11 规则 14） |
| **P2** | 上下文预算 + 两级压缩 + 400 兜底 + **调模型的摘要式压缩** | 水位 0.85 / 0.95 + 滞回；瘦身 / 丢弃 / 强制三档；摘要在**丢弃档与 400 兜底档**触发（带三道成本闸 + `LUNAC_SUMMARY_COMPACT` 开关 + 失败降级）。见 ai-spec §3.5 与 §11 规则 23 / 39 |
| **上下文** | 单条工具输出预算：超 12000 字符落盘全文、只内联「头 8000 + 尾 2000 + 路径」，模型用 Read / Grep 取回全文 | `tools::apply_budget`（唯一出口，`run_tool` 调用）；落盘 `temp\tool-outputs`（7 天清理）并并入 `Ctx.add_dirs`；见 ai-spec §3.5「单条工具输出预算」与 §11 规则 27 |
| **上下文** | 任务快照：压缩丢弃中段时，把最近一条 `TodoWrite` 清单钉回上下文，避免「压缩后忘了在做什么」 | `latest_todo_snapshot()`；纯文本 user 消息（与 `tool_use`/`tool_result` 配对结构解耦），插在第 1 条之后；见 ai-spec §11 规则 37 |
| **执行** | 只读工具并行：一轮里**连续的**只读调用合成一批并发（上限 4），写类/命令/MCP 串行 | `tools::parallel_safe` + `plan_tool_batches`；结果按下标回填 ⇒ 回灌顺序恒等于 `tool_use` 原顺序；见 ai-spec §3.5「只读工具并行」与 §11 规则 28 |
| **P3** | MCP 工具桥 | 把 `<exe 根>\tools\*.json` 的用户工具以 `mcp__<名>` 接进请求体；**读侧**（A3）另有两件条件注册的 `resources` 工具，让模型能看到用户工具的 `handler` |
| **P4** | 技能（渐进披露，inline 模式） | `LUNAC_SKILLS_DIR` 下 `<key>/SKILL.md`；提示词只列 `key: 描述`，模型调 `Skill` 取正文 |
| **会话** | 历史持久化 / 恢复 / 回退到某个用户轮 | `ModuleData\history\chat.db`（SQLite + FTS5，含专供 CJK 的 `trigram` 索引表）；`set_history` 协议把历史灌回 agent 上下文（见 ai-spec §11 规则 30） |
| **UI** | AI 对话面板（思考省略 / 命令卡片 / 回合折叠 / 运行方式三档 / 用量面板 / 被改动文件路径追踪） | 规范见 [agent-ui-spec.md](./agent-ui-spec.md) |
| 运维 | 落盘日志（两进程各写 `temp\logs\{agent,lunac}-YYYY-MM-DD.log`，含每次工具调用与耗时） | ai-spec §11 规则 20 |

---

## 4. 工具全景

### 4.1 当前实际注册（唯一真相源）

`core-agent/src/tools.rs` 的 `defs()` 恒返回 **13 件内置工具**，再由 `main.rs` 条件追加 `Skill` 与 MCP 动态工具：

| 工具 | 说明 |
|---|---|
| `Bash` / `PowerShell` | 命令执行（PowerShell 走 `-NoProfile -NonInteractive -Command` + 双 UTF-8 兜底） |
| `Read` / `Write` / `Edit` | 文件读写与精确替换编辑 |
| `Glob` / `Grep` | 文件与内容检索 |
| `WebSearch` | 联网检索（主源 bocha / tavily / exa / firecrawl，兜底 Bing RSS → Bing HTML → 百度抓取） |
| `WebFetch` | 抓取网页并转纯文本 |
| `TodoWrite` | 长任务进度面板（**免审批**：只改前端那块面板，不碰本机） |
| `AskUserQuestion` | 结构化提问（答案经 `can_use_tool` 的 `updatedInput` 回传） |
| `SessionSearch` | **往期会话检索**（2026-09-19）：检索本机会话库（`ModuleData\history\chat.db` 的 FTS5 索引），结果按会话分组返回。**只读、免审批**，走 MCP 桥的 `lunac/history_search` 自定义方法；`plan` 档同样可用。契约见 ai-spec §3.5「往期会话检索」 |
| `Skill` | **条件注册**：技能目录非空且未被黑名单裁掉时追加。取回本地技能正文（渐进披露的取回端） |
| `mcp__*` | **动态**：`<exe 根>\tools\*.json` 的用户工具，按名排序后接入（前缀缓存不变量，ai-spec §11 规则 18） |
| `ListMcpResourcesTool` / `ReadMcpResourceTool` | **条件注册**（2026-09-20，A3）：只在桥真的接上了用户工具（`!bridge.defs().is_empty()`）时才追加 —— 出厂时 `tools\` 只有模板，无条件注册就是在每次请求的固定前缀里放两件空转工具。用来列 / 读 `tools\*.json` 这些 resource（模型借此看到用户工具的 `handler`；`tools\` 在工作区外，内置 `Read` 会被工作区锁拒掉）。**免审批、plan 档放行、必须串行**（走单线程 stdio 桥）。契约见 ai-spec §3.5「MCP resources 读侧」与 §11 规则 55 |
| `Agent` | **子代理**（2026-09-20）：派生一个独立上下文的子代理跑自包含任务，中间工具输出不进主上下文，只回最终报告（`[task-N] subagent report: …`）。**要审批、串行、plan 档拒绝**；子代理工具集剔掉 `Agent`（防递归）。契约见 ai-spec §3.5「子代理」 |
| `Remember` | **条件注册**（2026-09-20，A4）：桥接通时追加。写**跨会话长期记忆**（`ModuleData\memory\MEMORY.md`，走桥的 `lunac/memory_write`），单条 ≤2000 字符、整文件 ≤6000 字符、按条去重。**要审批、串行、plan 档拒绝**（走 `needs_bridge` 早退分支 ⇒ 只读拒绝是分支内自判的）。填充它的**主要**是每 N 轮一次的后台复盘 fork（`LUNAC_NUDGE_INTERVAL`，默认 10）。契约见 ai-spec §3.5「长期记忆与后台复盘 fork」与 §11 规则 56 |

**总数口径**：13（`defs()` 恒返回）+ `Skill`（0 或 1，条件）+ MCP resources 两件（0 或 2，条件）+ `Remember`（0 或 1，条件）+ N（`mcp__*` 用户工具）；`--disallowedTools` 可在注册后裁剪。条件注册的三类都**不进 `defs()`**，因此「13 件」这个数字在任何配置下都成立（守门单测钉住）。完整未实现工具清单见 [agent-feature-backlog.md](./agent-feature-backlog.md) §1。

### 4.2 工具名口径提醒

- 前端特判的是 `Bash` / `PowerShell`，与 agent 侧**同名同义**；其余内置工具名与旧 CLI 一致。
- 工具黑名单候选列表已是**真实工具名**；勾选即真正从请求体 `tools` 裁掉（缩短前缀、提升缓存命中）。
- 旧 CLI 的 `getAllBaseTools()` 里那些已被显式置空的工具（`Config` / `REPL` / `Workflow` / `Monitor` …）属旧 CLI 的历史包袱，登记在 backlog §4 设想区，**不要照搬**。

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
- 生效：设置 ·「AI → 技能」面板增删改后自动重启 agent；手工改目录需重启 Lunac。
- **未实现**：技能的 **fork**（在子代理里跑）与 **remote**（远端拉取）两种模式，见 backlog A5。

### 5.2 工具（`tools\`）

- 布局：`tools\*.json`，一份文件一个工具；字段 `name` / `description` / `inputSchema` / `handler`。
- `handler.type` 三选一：`shell`（执行命令）/ `http`（发请求）/ `builtin`（Lunac 进程内实现）。
- 参数占位符 `{{参数名}}`（兼容 `{{ 参数名 }}`）。
- 接入后工具名一律 `mcp__<原名>`；**任何档位下调用都先弹审批卡**；只读档（`plan`）**不接入**。
- 数组**按工具名排序**后才进请求体（同上，前缀缓存不变量）。
- 生效：新增 / 修改后需重启 agent（工具编辑器面板保存后会自动重启）。
- 面板入口：设置 ·「AI → 工具 (MCP)」；详细的编辑界面在 **tool-editor 插件**里。

---

## 6. 与其它文档的分工

| 文档 | 负责什么 |
|---|---|
| **本文** | agent 的**实现现状**与**本地扩展格式** |
| [agent-feature-backlog.md](./agent-feature-backlog.md) | **待办唯一真相源**（全部未完成项，按优先级排序）+ 环境变量表 + 数据根 |
| [ai-spec.md](./ai-spec.md) §3.5 | agent 的**协议契约**（stream-json 字段与语义、用量口径、压缩、安全分析） |
| [ai-spec.md](./ai-spec.md) §11 | **硬约束规则**（前缀缓存纪律、审批与工作区锁、日志、落盘等） |
| [agent-ui-spec.md](./agent-ui-spec.md) | 对话面板的**界面与交互规范** |
| [code-rules.md](./code-rules.md) | 前端 / Tauri / WebView2 的**代码硬规则与反模式** |

> 本文**不再列「尚未复刻的子系统」与「实施顺序」** —— 那两节已整体并入 [agent-feature-backlog.md](./agent-feature-backlog.md)（旧版的 25 个具名工具 / 16 个子系统的差距表已按「实现 / 待办 / 设想」三态归位）。

---

*最后整理：2026-09-19 —— 撤下「未复刻工具表」「未复刻子系统表」「实施顺序」三节（并入 backlog），补上本轮实测的工具注册口径与已落地的任务快照 / 摘要压缩 / 会话持久化。*

*2026-09-20 追加：**工具覆盖实测** —— 直接驱动 debug 版 `agent.exe`（带 `--mcp-server stdio:<lunac.exe>` 与 `LUNAC_SKILLS_DIR`，不经 UI）跑一次 13 步任务：`Read` / `Write` / `Edit` / `Glob` / `Grep` / `Bash` / `PowerShell` / `WebSearch` / `WebFetch` / `TodoWrite` / `SessionSearch` / `Skill` **全部真实调用成功（各 1 次 `ok`）**；`AskUserQuestion` 被调用后返回 `No answer was collected — the interactive channel is unavailable. Ask the user in plain text instead.` —— 探针没有前端，**这是刻意的优雅降级而非缺陷**（有前端时答案经 `can_use_tool` 的 `updatedInput` 回传）。该次提问 14 个请求、汇总命中率 90.3%。*

*同一次运行还确认了三条链路在真实进程里同时可用：`--mcp-server` 接通（`[agent] MCP 桥已接通，用户工具 0 个`）、技能加载 1 个（`my-skill`）、往期会话索引注入 372 字。*

*2026-09-20 追加（同日第五批，**A4 长期记忆与后台复盘 fork**）：工具面新增**条件注册**的 `Remember`（桥接通时才进请求体，`defs()` 仍是 13 件）；`ModuleData\memory\MEMORY.md` 成为**跨会话记忆**的落点（服务端两个自定义方法 `lunac/memory_read` / `lunac/memory_write`，不进 `tools/list`）；每 `LUNAC_NUDGE_INTERVAL`（默认 10）次提问在**提问之间**跑一次后台复盘 fork（白名单工具，自己连桥、独立 `Cfg` 副本、不发 `task_*` 事件、审批随前端运行方式）。实测：启动时注入可用（模型逐字抄回预置的记忆文件）、`system/init` 出现 14 件工具、复盘写出两条 `Remember`（9 秒）、无桥时两件事都不发生。契约见 ai-spec §3.5「长期记忆与后台复盘 fork」、纪律见 §11 规则 56。*
