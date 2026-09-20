# Lunac 待办清单（**唯一待办真相源**）

> **用途**：本项目**全部未完成工作**的唯一登记处。`ai-spec.md` / `agent-implementation.md` / `agent-ui-spec.md` / `architecture-rendering.md` **只保留规则与现状，不再各自维护待办**；任何新想法一律登记到本文。
>
> **三条维护纪律**
> 1. **做完就删** —— 条目完成后从本文**删除**，不标 ✅ 留在原地。「为什么这么做、踩了什么坑」的结论落进 `ai-spec.md` §11 规则、代码注释与 Git 提交记录，**不由待办条目承载**（本文因此不再有历史包袱，读一遍就知道还剩什么）。
> 2. **每条必须能落地** —— 写清「做什么 / 为什么值得 / 依赖 / 落点文件」；写不出落点的东西属于设想，一律放 §4 设想区，不混进待办。
> 3. **优先级只按本文顺序** —— 级别（P0–P3）是粗档，档内顺序即实施顺序。
>
> **最后核对**：2026-09-20（逐条对照 `core-agent/src/`、`app/src/`、`app/src-tauri/src/` 实测，非照抄旧文档）
>
> **口径说明**：本文的前身是「对照旧 `cli.exe` 的完整差距清单」。差距面已核对完毕（见 §5 边界声明），**已完成项全部撤下**，现只保留未完成项。Lunac 的目标仍是 **一个可以完全类比于完整 agent 类应用**的能力体（见 [agent-implementation.md](./agent-implementation.md) §1）—— 所以下面的分组只是**优先级**，不是**价值否定**。
>
> **§ 号变更对照**（代码注释与规范里仍散落旧编号 `backlog §8.x` 等，按此换算即可，不必逐处回改）：
>
> | 旧编号 | 现在在哪 |
> |---|---|
> | §0 差距总览 / §4 不是缺口 | §1.1 组 A（工具）**A14** / **§5 边界声明**（A3 / A5 / A8 / A9 已落地）；其余见 §4 设想区 |
> | §1.2 组 B / §1.3 组 C | **A13 按需重估**（+ §4 设想区） |
> | §2.1 组 A（子系统） | **已全部落地** —— A6 写文件内容级安全扫描于 2026-09-20 完成（A4 长期记忆与后台复盘、会话持久化与检索更早已完成，见 `ai-spec.md` §3.5） |
> | §2.2 组 B | **已全部落地** —— A11（会话 id 与 rewind）于 2026-09-20 完成，见 `ai-spec.md` §3.5「会话 id 与 rewind」/ §11 规则 64（A3 / A5 / A8 / A9 / A10 更早已落地） |
> | §2.3 组 C | **A13** + §4 设想区 |
> | §3 协议 / 接口层 | **已全部落地** —— A11 于 2026-09-20 补齐 `session_id` 真值与任意消息回退（其中的 A1 子代理框架更早已落地） |
> | §5 空转 UI / 失实文案 | **已全部修复**，条目已删 |
> | §6 建议实施顺序 | 本文 **「优先级总表」** |
> | §8.1 被改动文件路径追踪 | **已完成** ⇒ 结论在 `ai-spec.md` §11 规则 32 |
> | §8.2 摘要式压缩 | **已完成** ⇒ 结论在 `ai-spec.md` §11 规则 39 |
> | §8.3 任务快照 | **已完成** ⇒ 结论在 `ai-spec.md` §11 规则 37 |
> | §8.4 一次对话中的多任务并行 | **A14** |
> | §8.5 对话数据库 | 五步全落地 ⇒ **已删除**（检索侧见 `ai-spec.md` §3.5「往期会话检索」；记忆**写入**侧已由 A4 于 2026-09-20 落地，见 `ai-spec.md` §11 规则 56） |
> | §8.6 建议顺序 | 本文 **「优先级总表」** |

---

## 优先级总表

| 级别 | 条目 | 为什么在这个位置 |
|---|---|---|
| **P2** | L1 插件市场（含 Live2D 桌宠） | **用户已定方向**，卡在三条硬约束（CSP / 资产 / 常驻开销） |
| **P2** | L2 AI 人格 / 风格录入 | **用户已选「人格编辑器」**，改动集中在提示词装配 |
| **P3** | A13 记忆目录 / 斜杠命令 / 剩余工具 | 按需重估，见各项 |
| **P3** | A14 多任务并行 | 依赖**已落地**的子代理（原 A1）+ 前端分栏 UI，排在功能项之后 |
| **P3** | A15 压缩策略的成本模型 | 命中价是 miss 价的 1/50 ⇒ 就地瘦身多数净亏，水位需重估 |
| **P3** | A16 跨提问首轮的缓存接力 | 首请求是单次提问里权重最大的一次 miss，直接决定能否守住 92% |
| **P3** | U1 diff 卡 / U2 缩略导航·Fork / U3 真沙箱调研 | 界面增强与调研，排在功能之后 |
| **P3** | M1 两个待实测项 | 不阻塞任何开发，攒到复现时做 |

---

## 1. Agent 能力缺口

### P3

**A13. 按需重估的剩余项**（不做只因为优先级，不是因为没价值）

| 项 | 说明 / 何时重估 |
|---|---|
| 斜杠命令 | 旧 CLI 有 75+，多数是 CLI 会话内操作（`/theme` `/vim` `/statusline` `/login` …），桌面端另有 UI。**只挑与能力相关的子集**（`/compact`、`/rewind`、memory 类）评估，不整体照搬 |
| 记忆目录（CLAUDE.md 体系） | 跨会话记忆；Lunac 已有 `ModuleData` 体系，**先照 `SessionSearch` 那套（会话库 + 冻结快照注入）设计**，别另起一套 |
| `NotebookEdit` | Jupyter 场景，用户群不大 |
| `EnterWorktree` / `ExitWorktree` | 需要 git worktree 工作流 |
| `CronCreate` / `CronDelete` / `CronList` | 后台定时任务；Lunac 已有 Windows 计划任务做自启，能力不重叠 |
| `WebBrowser`（浏览器控制） / `LSP` | 需要常驻上下文（浏览器会话 / 语言服务器） |
| `SendUserFile` / `PushNotification` / `Brief` | 面向远端 / 移动端的推送通道 |
| 多代理协作（`TeamCreate` / `TeamDelete` / `SendMessage` / `ListPeers`） | **前置已落地**（子代理 `Agent`，2026-09-20）—— 应立刻重估，不要再按「与 Lunac 无关」处理 |
| 输出样式 / statusline | CLI 的终端样式体系，已被 WebView 取代 |
| Ink TUI / Vim / 语音 / buddy / chrome / 桥接远程控制 / OAuth 账号体系 / 自动更新 / 代理证书 mTLS / IDE 集成 | 绑定的分别是旧 CLI 的终端形态、当前交互形态没有的入口、企业网关场景；**由 NSIS 安装包与 `vscode-extension/` 各自负责的部分已完成**。逐项都要「先确认它在新宿主下还有意义」再重估 |

**A14. 一次对话中的多任务并行（多个任务各自走独立 API 请求）**

- 目标：一次对话里互不依赖的多个任务并发跑，而不是严格的一问一答。
- 现状（别与「只读工具并行」混淆）：**只读工具并行已经做了** —— 一轮里**连续的**只读调用合成一批并发（上限 4），写类 / 命令 / MCP 串行，结果按下标回填 ⇒ 回灌顺序恒等于 `tool_use` 原顺序。那是「同一次 API 响应里的多个工具调用」；**模型请求本身仍然串行**。
- 硬约束：① 每个任务须有**独立的消息数组与独立工具环境**；② 结果回灌必须**能归因到任务**（子代理的短计数 `task-N` 已落地；跨运行的归因由 `session_id` 提供 —— A11 已把它改成真值，见 `ai-spec.md` §11 规则 64）；③ 并发 = 花钱 ⇒ 并发上限 + 预算封顶；④ 前端要能把多任务的流式输出**分栏 / 分组**，否则用户看到的是交织成一团乱的流。
- 依赖：子代理框架（**已落地**，见 `ai-spec.md` §3.5「子代理」）+ 前端分栏 UI。区别：本项要的是**多个子代理同时跑**（`Agent` 现在是**串行**的，且 `parallel_safe` 为 `false`），所以不是「有了 `Agent` 就自动有了 A14」。

**A15. 压缩策略的成本模型重估（就地瘦身多数情况净亏）**

- 做什么：按 DeepSeek 的**真实价差**重估 `compact_history()` 三档的水位与「值不值得」闸门，减少「省小废大」。
- 为什么值得（2026-09-20 量化）：`deepseek-v4-flash` 的命中价是 miss 价的 **1/50**（0.02 vs 1 元/M）。就地瘦身**省下的是廉价 `read`、废掉的是全价 miss**：
  - 收益 ≈ 省下的 token × **剩余轮数** × 0.02；代价 ≈ 被作废的后缀（≈ 当前总输入）× 1.0
  - 实测（`usage-2026-09-18.jsonl` 第 2 条）：一次 `elide` 瘦身 23 个 `tool_result`（省 176101 字 ≈ 91k token），当轮 `read` 从 108160 塌到 2176；按上式剩余 9 轮 ⇒ 收益 ≈ 16k、代价 ≈ 23.5k，**净亏**。
- 候选手段（需实测择一，不要一次全上）：① 抬高 `ELIDE_RATIO`（减少次数，但单次损失更大，拐点要实测）；② 瘦身**从靠近尾部开始**（失效点靠后 ⇒ 作废的后缀更小）；③ 把 `elide` 降级为「防止 drop 的缓冲」，只在逼近 `DROP_RATIO` 时才允许。
- 硬约束：`Drop` / `Force` 两档是**防 400 的安全刚需**，不在重估范围内。
- 前置：需要一个**可重复的大上下文回归**（先把上下文养到 0.85 水位以上、再打满轮次），否则改完无法验证。可复用 A16 的探针。
- 落点：`core-agent/src/main.rs` 的 `compact_history()` / `ELIDE_RATIO` / `ELIDE_MIN_SAVINGS_RATIO`；结论回写 `ai-spec.md` §11 规则 23。

**A16. 跨提问首轮的缓存接力不稳定（待查）**

- 现象（2026-09-20 实测）：同一个 agent 进程内连续提问两次，第二次的**首个请求**有时能吃到上一次留下的缓存（`usage-2026-09-18.jsonl` 第 2 条的首请求 `read=81792`），有时只命中到 `system + tools`（探针 `-Runs 2` 的第二次提问首请求 `read=2560`，而第一次提问末轮的 `total=13745`）。
- 为什么值得查：首请求是单次提问里**权重最大**的一次 miss（整段历史重发），它直接决定 92% 这条线守不守得住。
- 判据已就绪：`请求前缀` 行的 `公共前缀=N/M条` + `请求用量` 行的 `read`（见 `ai-spec.md` §11 规则 23 的埋点条目）—— 先判定是不是本侧在两次提问之间动了 `history`。
- 落点：`core-agent/src/main.rs` 的提问入口与 `run_query()` 边界。

---

## 2. 界面层待办（`agent-ui-spec.md` 阶段 3）

> `agent-ui-spec.md` §10 的阶段 1 / 阶段 2 **已全部落地**，清单已撤下；阶段 3 的三项挪到此处统一排序。**界面规范本身（§0–§9 与 §8 复核清单）继续有效，不回退。**

| # | 项 | 说明 |
|---|---|---|
| **U1** | 代码变更 diff 卡 | `Write` / `Edit` 的变更预览 + 单条 / 单文件 / 全部接受与拒绝，外加 DiffView 汇总受影响文件数与变更行数。需要后端回传 diff 或前端重算，改动面大于阶段 1/2 |
| **U2** | 多轮缩略导航 / 会话 Fork | 与现有历史抽屉职责重叠，先做抽屉增强；Fork 收益低（本地单机工具，无分享链路） |
| **U3** | 真沙箱调研 | Windows 侧隔离手段（Job Object 资源限制 / 低完整性级别令牌 / AppContainer）。**结论需单独立文档**，不得与 `agent-ui-spec.md` §4 的「运行方式」混称 —— 那三档是**策略级**的，`agent-ui-spec.md` §4.1 的诚实原则继续有效 |

---

## 3. Lunac 自身新目标

> 本节不是「旧 cli.exe 有而我们没有」，而是用户直接提出的新方向。
> 调研参照物 = **Hermes Agent**（Nous Research，MIT，`github.com/NousResearch/hermes-agent`）；其「三层记忆」骨架（`MEMORY.md` 2200 字符 + `USER.md` 1375 字符 + `state.db` FTS5 会话检索）与省 token 机制（解析见 `ai-spec.md` §9.1 难点 1）仍是主要参考。

**L1. 插件市场（阶段 2，用户已定方向）**

- 目标：设置 ·「插件」分区列出全部插件，可从用户 GitHub 仓库下载 / 卸载；下载前提示需要一并下载的前置与依赖。**Live2D 桌宠做成插件**而非内置依赖。本地（dev）全部内置。
- 现成先例：`download_tool_from_url` / `install_skill_from_url`（工具与技能的 URL 安装链路已经通了）。
- **三条硬约束必须先解决**（否则一动手就撞墙）：
  1. **CSP 是 `script-src 'self'`**（见 `ai-spec.md` §3.7 的 KaTeX 缺陷）⇒ 运行时依赖必须随插件**本地打包**，**不能走 CDN**；
  2. Live2D 需要 pixi + Cubism Core + 数 MB 模型资产（**Cubism 有商用授权条款**）⇒ 走「插件自带依赖 + 下载时提示」；
  3. 常驻画布有持续 GPU / rAF 开销，**窗口隐藏时必须暂停** —— 与国际化的「轻量」目标冲突，**接入前先做最小原型验证**。
- 相关：`ai-spec.md` §20 的「路径 2」是本节的前身。**注意 `core/` 的含义**：它是**本机参考用的旧 CLI 源码**（被 `.gitignore` 排除、**不在仓库里**），所以 `core/plugins/...` 一类路径只表示「参考它的设计、在新宿主重建」，**照它去找一定找不到**；真要做时**先重写落地路径**。

**L2. AI 人格 / 风格录入（阶段 2，用户已选「人格编辑器」）**

- 目标：设置里可编辑人格与输出风格，内容落到 exe 根 config（如 `config\persona.md`），**agent 启动时拼进系统提示词的固定段**。
- 硬约束：**绝不按消息拼接**（那会让提示词每轮都变、把整个固定前缀缓存打掉，`ai-spec.md` §11 规则 23）。现状 `PERSONA_AND_STYLE` 是 `core-agent/src/main.rs` 的 **Rust 常量**（2026-09-17 从「前端每条消息拼接」搬到 agent 侧常量化，就是为了缓存命中），所以本项要动的是「常量 → 启动时可配置的固定段」。

**L3. 上下文感知提示词注入的剩余子集**

- 现状（**部分已落地，别当从零做**）：① 工作目录 / 宿主 / 技能目录已由 `core-agent` 的 `env_block()` 拼进系统提示词；② 前端已有按 query 关键词（debug / TDD / review）的 `buildSystemPromptHint()`，拼进**消息**；③ 附件路径走 `[Attached files]` 文本。
- **缺的只有**：「当前打开的插件 / 当前选中的文件」进上下文。补的时候注意区分「进系统提示词」（必须固定，否则破坏缓存）与「进消息」（可变）。

**L4. 调试阶段状态栏**

- 原 `ai-spec.md` §19.6 的 P2 遗留项。做一个只由开关控制的阶段状态栏，展示 agent 当前处于哪个阶段。现状只有**常显**的状态机与运行时提示（`.sys-note`，展示 agent stderr），没有分阶段的调试视图。

**M1. 两个待实测项（不阻塞开发）**

| # | 项 | 说明 |
|---|---|---|
| **M1-1** | 前端 CDN 与 WebView2 冷启动在**开机场景**的真实占比 | 现象侧已定论（开机自启慢的根因是 OS 触发时机 + 计划任务权限，见 `ai-spec.md` §9.1 难点 2）；此项只差「实测确认 CDN/冷启动无关」后写死结论，不再靠猜 |
| **M1-2** | 缓存命中率唯一遗留疑点：复现 `in=2287 / read=0` 那一轮 | 「落盘慢」已被 0ms vs 6s 对照实验**证伪**（两组逐字节相同、均 97.9%）。剩余两种可能：① 前缀本身变了（`skills::listing()` / 工具黑名单 / `cwd` 任一变化）；② 供应商侧缓存被清。**工具已就绪** —— 用 `result.usage.requests[]` 与启动时落盘的 `固定前缀 …` 行前后对照即可定位 |
| **M1-3** | 两个 `--disable-features` 是否逗号合并（`architecture-rendering.md` §6 末尾） | WebView2 自带的 `msWebOOUI,msPdfOOUI,msSmartScreenProtection` 与我们的 `PermissionPrompt,ClipboardContentRead` **是否同时在场**。现按「Chromium 逗号合并重复 switch」实现；若实测发现我们的串**挤掉了** WebView2 自带那份，合并策略要改成「并入同一个 switch 的值」。ai-spec §11 规则 38 已登记 |

---

## 4. 设想区（**没有落点，勿当成待办**）

以下是不再有实现意图、或只有一句话想法的东西。**保留只为防止重复讨论**，真要做时先重写落地路径再挪进正文。

- **旧 CLI 的 `Config`（ant-only）/ `REPL`（ant 专用 VM）/ `Workflow` / `RemoteTrigger` / `Monitor` / `SubscribePR`·`SuggestBackgroundPR` / `TerminalCapture` / `Snip` / `Sleep` / `StructuredOutput` / `OverflowTest`·`CtxInspect` / `TestingPermission`** —— 要么绑定旧 CLI 的特定形态（Ink TUI / ant 内部构建），要么是调试与实验开关；在 `getAllBaseTools()` 里大多已被显式置为 `null` 停用，属旧 CLI 自己的历史包袱。
- **按分类的会话临时文件清理服务**（Hermes 的 `disk-cleanup` 插件思路）—— 本项目的临时产物集中在 `<exe 根>\temp\`，**卸载时由 NSIS 整目录删除**，`logs` 另有 7 天保留清理（`log::purge_old`，`KEEP_DAYS=7`）。便携式布局下「整个 temp 目录」就是清理单位，**不需要**按 test / temp / session / download 分类。详见 `ai-spec.md` §13.2。

---

## 5. 边界声明（本清单覆盖什么、不覆盖什么）

**覆盖**：用户可以观察到的能力面 —— **工具名**、**子系统**、**协议消息类型**、**界面分期**、**自身新目标**。

- **工具面已核对完毕**：旧 CLI 的 `getAllBaseTools()` 里的每一个名字都已归位（实现 / 待办 / 设想）；Lunac 当前实际注册的是 **15 件内置工具**（`Read` / `Write` / `Edit` / `Bash` / `PowerShell` / `Glob` / `Grep` / `WebSearch` / `WebFetch` / `AskUserQuestion` / `TodoWrite` / `SessionSearch` / `Agent` / `EnterPlanMode` / `ExitPlanMode`）+ **条件注册**的四件：`Skill`（技能目录非空且未被黑名单裁掉时）+ `ListMcpResourcesTool` / `ReadMcpResourceTool`（**桥接上了用户工具时**，2026-09-20）+ `Remember`（**桥接通时**，2026-09-20，长期记忆写入侧）+ `mcp__*`（来自 `<exe 根>\tools\*.json`），可用 `--disallowedTools` 裁剪。
- **不覆盖一**：`core-agent/src/` 里的实现细节级能力（如各工具的解析细节、日志格式），按子系统归并。
- **不覆盖二**：旧 CLI 终端渲染组件（`core/tools/**/UI.tsx`）随 Ink TUI 一并排除。
- **不覆盖三**：Lunac 与旧 CLI **都有**的能力不再列出（例如前端依赖的 stdout 契约 `system/init` / `stream_event` / `assistant` / `user/tool_result` / `control_request` / `result` **全部已提供**；`--disallowedTools` 链路已通；思考开关跨模型自适应是**超集**——只有开 / 关两档，不要按「多档更深」扩）。
- **口径提醒**：`session_id` 自 A11（2026-09-20）起是**真值**并已接上消费者（前端随用量落盘做归因，见 `ai-spec.md` §11 规则 64）；`total_cost_usd` / `num_turns` / `duration_ms` 仍**前端零引用**、`stop_reason` 未提供 —— 这几项**当前无影响**，不列为待办，改动前先确认有消费者。

---

## 6. 环境变量与数据根（核对基线）

`core-agent` 当前读取的 `LUNAC_*` 环境变量共 **17 个**（均**只在 spawn 时注入**；切换其中任何一项 = `kill_and_cleanup()` 重启 agent）：

| 变量 | 落点 | 用途 |
|---|---|---|
| `LUNAC_AGENT_BASE_URL` / `LUNAC_AGENT_TOKEN` / `LUNAC_AGENT_MODEL` | `main.rs` | 端点 / 凭据 / 模型 |
| `LUNAC_MAX_CONTEXT_TOKENS` | `main.rs` | 上下文预算（未设走默认） |
| `LUNAC_SUMMARY_COMPACT` | `main.rs` | 摘要式压缩开关（`0`/`false`/`off`/`no` 关，默认开） |
| `LUNAC_HISTORY_INDEX` | `main.rs` | **往期会话索引**注入开关（`0`/`false`/`off`/`no` 关，默认开）；工具侧另可用 `--disallowedTools SessionSearch` 连索引一起停 |
| `LUNAC_MEMORY` | `main.rs` | **长期记忆**注入开关（写法同上，默认开）；`--disallowedTools Remember` 会连注入一起停（两者同源，见 ai-spec §11 规则 56） |
| `LUNAC_NUDGE_INTERVAL` | `main.rs` | **后台复盘的轮次门槛**（每 N 次用户提问跑一次，默认 10，`0` = 关；非数字回落默认） |
| `LUNAC_SKILLS_DIR` | `main.rs` / `skills.rs` | 技能目录 |
| `LUNAC_THINKING` | `main.rs` | 思考开关（`off` / 其余=开） |
| `LUNAC_WORKSPACE_LOCKED` | `main.rs` | 工作区锁（硬边界，见 `ai-spec.md` §11 规则 14） |
| `LUNAC_HOOKS_FILE` | `hooks.rs` | **权限 hooks 配置文件的路径**（`config\hooks.json`，无条件注入；「文件不在 = 没配」由 agent 一处判定，按 mtime 热重载 ⇒ 改配置不必重启 agent，见 §11 规则 61） |
| `LUNAC_SEARCH_PROVIDER` / `LUNAC_SEARCH_KEY` | `tools.rs` | 联网检索主源 |
| `LUNAC_LOG_DIR` / `LUNAC_LOG` / `LUNAC_LOG_LEVEL` | `log.rs` | 落盘日志 |

**数据根 = `<exe 根>`（便携式，无独立 HOME）**：`skills\`（技能）、`tools\`（用户工具定义）、`ModuleData\`（`history\chat.db`、`memory\MEMORY.md`、`usage\usage-*.jsonl`）、`temp\`（`logs\`、`tool-outputs\`、`transStorage`、`webview-data`）、`config\`。用户资产的扩展格式见 [agent-implementation.md](./agent-implementation.md) §5。

---

*本轮整理说明（2026-09-19）：本文由「旧 CLI 差距清单 + 五项用户新目标 + 界面阶段 3 + 散落在 `ai-spec.md` §10/§13/§19.6/§20 的待办」合并而成，**删除全部已完成条目**（其结论已落进 `ai-spec.md` §3.5 / §11 规则与 Git 记录），**按优先级重排**，并补入逐条实测的现状。对照面的审计结论保留在 §5 边界声明。*

*2026-09-19 追加：**A2（会话历史检索 + 记忆注入）已完成并已从本文删除** —— 契约与实测数据在 `ai-spec.md` §3.5「往期会话检索」、纪律在 §11 规则 53。*

*2026-09-20 追加：**B 类缓存崩塌已修复**（打满 `MAX_TOOL_ROUNDS` 时追加的收口消息改变了 `history` 末尾的块结构 ⇒ 端点侧前缀缓存单元整体失配：第 17 轮 `read` 2560 / 命中率 23.3%；修复后 10880 / 91.3%，整次提问汇总 **87.2% → 95.9%**）。三种形态的单变量对照表与归因套路在 `ai-spec.md` §11 规则 23。本次新登记 **A15 / A16** 两条待办。*

*2026-09-20 追加（同日第二批）：**A1 子代理框架已完成并从本文删除** —— 内置工具 `Agent`（第 13 件）落地，五条硬约束全部满足并经端到端实测（`cargo test` 45 passed；deepseek-flash 真机：`tool_use(Agent)` → `task_started` → `task_progress`×6 → `task_done(ok=true, ms=3938)`，报告 `Total lines: 21` 正确；plan 档拒绝亦已实测）。契约与实测数据在 `ai-spec.md` §3.5「子代理」、纪律在 §11 规则 54；工具总数口径 13（`agent-implementation.md` §4.1）。**A3 / A4 / A5 / A14 的前置据此全部满足**（A14 仍需前端分栏 UI，且 `Agent` 目前是串行的）。当前未落地 **14 项**（A3–A16）。*

*2026-09-20 追加（同日第三批，**A1 复查**）：对 A1 逐行复查后修掉三处 —— ① **子代理工具集只剔了 `Agent`**，把 `mcp__*` 与 `SessionSearch` 一起发了过去（而子代理不接桥 ⇒ 调用必然失败，`mcp__*` 还会先弹一张白问的审批卡）⇒ 提取 `subagent_tool_defs()` 剔三类 + 守门单测；② **子代理的系统提示词只有角色段**，不知道自己的工作目录、也不知道有哪些技能（`Skill` 在它的工具集里却没有清单）⇒ 补 `env_block(cwd)` + `skills::listing()`；③ **生成参数与主循环不一致**（固定 `max_tokens: 4096` 且不发 `thinking` ⇒ 端点默认开着思考、思考文本会把报告挤空，且 `LUNAC_THINKING=off` 对子代理失效）⇒ 与主循环同源复用 `cfg.thinking`。复查后 `cargo test` **46 passed**，release 真机复跑：子代理自报工具 11 件（13 − `Agent` − `SessionSearch`，无 `mcp__*`）、报出正确 cwd、数出 21 行，`task_done(ok=true, ms=5825)`。*

*2026-09-20 追加（同日第四批）：**A3（MCP resources 读侧）已完成并从本文删除** —— 服务端补 `resources/read`（**硬边界**：只允许读 `tools\` 目录内的 `.json`，`canonicalize()` 后比前缀，挡 `..` 与符号链接），客户端加 `Bridge::list_resources()` / `read_resource()`，两件工具 `ListMcpResourcesTool` / `ReadMcpResourceTool` **条件注册**（只在桥真的接上了用户工具时才进请求体，与 `Skill` 同理 —— 出厂时 `tools\` 只有模板，无条件注册就是在固定前缀里放两件空转工具）。契约与实测在 `ai-spec.md` §3.5「MCP resources 读侧」、纪律在 §11 规则 55。桥工具的名单收口到 `tools::BRIDGE_TOOLS` 一处（`dispatch_tool` 早退 / `subagent_tool_defs` 剔除 / 条件注册三处共用）。`cargo test`：core-agent **47 passed**、src-tauri **56 passed**。当前未落地 **13 项**（A4–A16）。*

*2026-09-20 追加（同日第五批）：**A4（每轮后台复盘 fork + 长期记忆）已完成并从本文删除** —— 落点 `<exe 根>\ModuleData\memory\MEMORY.md`（桥上的 `lunac/memory_read` / `lunac/memory_write` 两个自定义方法，不进 `tools/list`），启动时**冻结快照**注入系统提示词（与会话索引同纪律），写入侧新增**条件注册**的 `Remember` 工具；每 `LUNAC_NUDGE_INTERVAL`（默认 10）次**用户提问**在**提问之间**派一次后台复盘 fork（白名单工具 `Read`/`Glob`/`Grep`/`Write`/`Edit`/`Skill`/`Remember`、自己连一条桥、独立 `Cfg` 副本、不发 `task_*` 事件、审批走同一条 `can_use_tool` 通道随前端运行方式，**不引定时器**）。顺带修掉前端一处**与后端语义不一致**：`classifyRequest()` 的 `opaque`（判不出来的命令）在**自动档**也弹卡 ⇒ 现改为按档位判定（自动档放行，手动 / 白名单仍弹卡），危险命令的优先级不变 —— 见 agent-ui-spec §4.2。契约与实测在 `ai-spec.md` §3.5「长期记忆与后台复盘 fork」、纪律在 §11 规则 56。`cargo test`：core-agent **52 passed**、src-tauri **57 passed**。当前未落地 **12 项**（A5–A16）。*

*2026-09-20 追加（同日第六批）：**A5 全部完成 + A6 已完成，两条均已从本文删除**。① **A5 前半（fork 模式）**：frontmatter `context: fork` 的技能改由**子代理执行**、主对话只收报告，工具面由 `allowed-tools` 收窄；`Skill` 因「副作用随入参而变」**移出 `parallel_safe` 白名单**（只看名字的判据按最坏模式算），带输入的判据另写 `needs_approval_with()`；fork 复用子代理引擎（`Agent` / A4 复盘 / fork 技能 = 三个调用方共用一个 `run_subagent()` + `ForkSpec`）。② **A5 后半（技能自带脚本 / 资源）**：技能目录里除 `SKILL.md` 之外的文件在扫描时登记（深度 ≤ 3 / ≤ 40 条 / 跳过隐藏项与 `node_modules`·`target` / **不跟随符号链接** ⇒ 「报出去的路径一定落在技能目录内」不需要逐条 `canonicalize()`），**调用 `Skill` 时**附在返回里（inline 附正文之后、fork 附进子代理任务说明），**刻意不进系统提示词**（否则前缀缓存跟着文件系统抖动）；路径给**相对形式**并写明相对谁；超限**如实上报**「还有没列出的」。**remote 已定论不移植**。③ **A6（写入内容的凭据扫描）**：`core-agent/src/content_safety.rs` —— 12 条规则分三类（私钥块 / 七家固定前缀 API key / JWT·`Bearer`·连接串口令·两条三道闸的通用赋值），`RegexSet` 先跑一遍（零命中即返回）、`Write` 取 `content`·`Edit` 取 `new_string`（**不扫 `old_string`**）、512 KB 上限如实上报；**只做凭据一类**（代码注入 / XSS 那类正则在正常代码里必然满屏误报 ⇒ 用户学会无视告警，比没有更糟）；只上报 `analysis.secrets`**不代替决策**，前端走**独立通道**（不自动放行含「自动」档、不给「始终允许」、命中项可见地列在卡片正文而不是只塞 tooltip）；规则集刻意与 `bash_safety.rs`（扫**命令**、**执行前**）分开。**实测**：`cargo test` core-agent **75 passed / 0 failed**（含新增 `content_safety` 10 条、`skills` 资源类 5 条）；deepseek-flash 真机跑 `Skill`（inline）确认自带资源清单进得了 `tool_result` 且模型能原样复述（`assets/tpl.md` / `scripts/run.py`）。契约在 `ai-spec.md` §3.5「命令静态安全分析 / 写入内容的凭据扫描」「P4 已完成」、§13.1，纪律在 §11 规则 57 / 58，预检在 `code-rules.md` #20。同期还落了外观项「恢复默认主题」的边界修订（`tintBase`：只有三组配色的**颜色**失效，三个透明度与**文字明度**照常生效 —— 见 `ai-spec.md` §11 规则 45 与 `code-rules.md` 预检 #17）。当前未落地 **10 项**（A7–A16）。*

*2026-09-20 追加（同日第七批）：**A7（计划模式闭环）已完成并从本文删除**。形态刻意**不复用「只读档位」**，而是拆成两个互不替代的概念：**用户的只读档**（`--permission-mode plan`，设置里选、启动时定死、改它要重启 agent）与**模型的计划相位**（`plan_phase`，进程内即时生效、批准后立刻解除）。① **两件内置工具**：`EnterPlanMode`（只把相位标志置真 + 广播 `system/plan_mode state=on` 带模型自述理由，**免审批** —— 问它等于让用户批准「我要开始思考了」）、`ExitPlanMode`（**必须走审批** —— 这张卡就是它的产品，用户要在卡上读到整份计划再裁决），工具总数 13 → **15**；守门单测同步。② **判据收口到 `tools::write_blocked(ctx, what)` 一处**，两档的**拒因措辞分开**（用户该做的动作不同：只读档指向「去设置改档位」、计划相位指向「去批准计划」），把「哪些工具算写类」这份知识也收口到 `tools.rs`。③ 相位用 `Arc<AtomicBool>` 而非 `bool` / `Cell`：`Ctx` 是 `Clone` 且跨线程（并行只读批拿 `&Ctx`、后台复盘 fork 拿克隆）⇒ 值语义会各持一份、`Cell` 破 `Sync`；`Arc` 让**派生子代理 / fork 技能 / 后台复盘自动继承**相位。④ **四个早退分支全部补判**：`Skill` / `SessionSearch` / `needs_bridge` / MCP 工具都早于 `tools::run()` 返回、会绕过那里的拦截 ⇒ `Remember` / `Agent` / fork 技能 / MCP 四处各补一次 `write_blocked`（否则只读档与计划相位能从这条缝里写本机）。⑤ **只读档下不许** `ExitPlanMode`（否则模型能靠它把用户选的档位绕开），报错指向设置；相位**不被清**。⑥ **前端零协议新增字段**：计划正文本来就在 `request.input.plan` 里，`renderPlanCard()` 直读 ⇒ `open_approval()` **零改动**、计划卡 = 同一条审批通道的第三种行；`classifyRequest()` 对 `ExitPlanMode` 免疫（白名单也不放行）、不给「始终允许」；批准时 `save_plan_md` 落 `<exe 根>\ModuleData\plans\<本地时间戳>.md`（Rust 侧没有 chrono ⇒ 时间戳由**前端**给 + 后端只做严格形状校验 `YYYY-MM-DD_HHMMSS`，同 `append_usage_log` 先例）；`EnterPlanMode` 另挂顶部横幅，agent 重启时清掉。⑦ **刻意不做** `VerifyPlanExecution`（验证这一环交给 `TodoWrite`）；子代理看不到这两件工具，后台复盘门槛加 `&& !plan_phase`。**实测**：`cargo test` core-agent **79 passed / 0 failed / 2 ignored**、src-tauri **58 passed / 0 failed / 1 ignored**，`tsc --noEmit` exit 0，`cargo build --release` 通过；release 真机 E2E **11 条断言全过**（`EnterPlanMode` 广播 → `Read` 通过 → `ExitPlanMode` 走审批、计划正文 1197 字符 → **拒绝**后拒因原话进 `tool_result` → 下一问要求直接动手时 `Write` 被硬拒 → 再交计划并**批准** → 广播 `state=off` → 同一个 `Write` 落盘且内容正确）。契约在 `ai-spec.md` §3.5「计划模式闭环（A7）」、纪律在 §11 规则 59，UI 在 `agent-ui-spec.md` §3.6 / §4.4 / §9，预检在 `code-rules.md` #12（扩写：早退分支必须自己补 `write_blocked`）与 #21（新增：写类判据只改一处、两档措辞分开）。当前未落地 **9 项**（A8–A16）。*

*2026-09-20 追加（同日第八批）：**A8（多模态输入·图片一半）已完成并从本文删除**。① **形态**：stdin 的 user 消息 `content` 里可再加 `{"type":"image","source":{"type":"file","path":"…"}}` —— **只传路径、不传字节**（前端三种附件来源本来就已落成路径；几 MB 的 base64 不必过 IPC 管道、也不必在 WebView 里再存一份），字节由 core-agent 读出来转成端点要的 `{"type":"base64","media_type":…,"data":…}` 块；`[Attached files]` **文本照旧保留**（历史 / 标题 / 复制三条旧路径只认它，且它承载「哪个路径对应哪张图」）。② **开关在前端、默认关**：设置面板「模型支持图片输入」→ `config\ai.json` 的 `vision`（`set_ai_config` / `get_ai_config`），**不改启动参数、不重启 agent**。理由是发给不支持视觉的端点（DeepSeek 官方端点）必 400，而 agent 侧**无法预判模型能力** ⇒ 只能由用户显式断言；刻意**不**做「按模型名推断」与「先发再 400 回落」。③ **接收方按内容判定**：类型只认**魔术字节**（PNG / JPEG / GIF / WebP 四种，不信扩展名），单图原始字节 ≤ 3.5 MB（端点 5 MB 按 base64 算 ⇒ 3.5 MB 才不越线）、每条 ≤ 10 张。④ **失败可见**：读不出来 / 超限 / 超张数的一律走 `system/attachment_note`（`skipped:[{path,reason}]`，只在真有失败项时才发），前端渲染成一条黄色 `.sys-note-warn` 明细行 —— 静默丢弃会让用户只看到「模型说它看不到图」。⑤ **附件读盘刻意不走工作区锁**：路径来自用户显式选中（不是模型找出来的），而剪贴板图片就落在 `%TEMP%`（在工作区之外）—— 套锁会让最主要的那条用法直接失效；模型的 `Read` 照旧受锁约束。⑥ **顺手修掉一个真 bug**：剪贴板图片落盘用的是 `lunac_ocr_<pid>.<ext>` / `lunac_clip_<pid>.<...>` **固定名**，同一进程第二次粘贴会覆盖第一张（而旧 chip 还指着同一路径）⇒ 改为 `temp_image_path()` 生成唯一名（四处现场全改）。**实测**：`cargo test` core-agent **80 passed / 0 failed / 2 ignored**（新增 `image_blocks_are_resolved_by_path_and_limited`）、src-tauri **58 passed / 0 failed / 1 ignored**、`tsc --noEmit` exit 0；**假端点实测**（本地 TcpListener 直抓 `/v1/messages` 请求体，13 条断言全过）：真 PNG ⇒ 请求体含 `"type":"image"` + `"media_type":"image/png"` + 与文件**逐字节一致**的 base64；读不出来的那张 ⇒ 请求体里没有它、stdout 有且仅有一条 `attachment_note`；第 2 轮请求体里仍带第 1 轮那张图（历史保留），且**任何 `"type":"file"` 都没真的发到端点**。契约在 `ai-spec.md` §3.5「图片附件」、纪律在 §11 规则 60，前端字段登记在 `agent-ui-spec.md` §9，预检在 `code-rules.md` #22。**PDF 刻意不做**（原生 `document` 块只有 Claude 系支持、本地提文本要引解析库且对扫描件无效）⇒ 仍走路径文本。当前未落地 **8 项**（A9–A16）。*

*2026-09-20 追加（同日第九批）：**A9（权限 hooks）已完成并从本文删除**。① **事件面砍到 8 个**：只做在 core-agent 里**真有落点**的 `SessionStart`（cfg 就绪后）/ `UserPromptSubmit`（进 `run_query` 前，**可拦整轮**）/ `PreToolUse`（**每一次**工具调用，含只读工具与子代理内部）/ `PermissionRequest`（只在「本来要弹卡」的那一刻，hook 可代答）/ `PostToolUse`（`run_one_tool()` 内、工具跑完之后）/ `PreCompact`（`compact_history()` 的三处调用点之前）/ `Stop`（成功收尾后）/ `SessionEnd`（stdin 关闭、退出前）。Claude Code 那套 19 类里的 `Notification` / `SubagentStop` / `TeammateIdle` 之类在 Lunac **没有对应节点**，一份都不空跑 —— 配了永不触发的事件比没有更糟。② **裁决权只到「等价白名单」为止**：hook 的 `allow` 只等于跳过审批卡，**静态安全分析命中（危险命令 / 写入内容里的凭据）仍强制弹卡**，工作区锁也照旧生效 —— 判据是 `hook_allow_needs_card()`，这是 §11 规则 14「任何一道闸门都不得为了少点一次同意而放宽」在 hooks 上的落点；`opaque`（判不定）刻意**不**强制弹卡，与前端「自动」档口径一致。③ **失败语义只认显式拒绝**：只有退出码 2 或 `{"decision":"deny"}` 才拦；**超时 / 崩溃 / 输出看不懂一律放行但可见**（`kind=error` 的 `system/hook_note` 到前端 + 一行 WARN 落到 agent 日志）—— 「以为装了保护、其实没跑」是最危险的状态，所以宁可放过也不能静默。④ **配置只有一份真相**：`config\hooks.json`（`enabled` 缺省为真；文件不存在 = 没配；设置面板的开关写的就是这个字段），**agent 侧按 mtime 热重载** ⇒ 改完**即时生效、不必重启 agent**（宿主因此**无条件**注入 `LUNAC_HOOKS_FILE`，连文件还不存在时也给 —— 否则用户在运行中新建配置文件就得等下次 spawn 才生效）；解析失败**保留上一份有效配置** + 落 WARN，语法错同时在设置面板那一行报出来。⑤ **顺手修掉三处真问题**：**(a)** `cmd /C` 前面用 `Command::arg` 会被 MSVC 引号规则把命令里的 `"` 转义成 `\"`，`cmd` 不认 ⇒ 形如 `type "C:\a b\x.json"` 的 hook 命令**整条失败**（实测：带引号时拿不到任何输出、去掉引号即正常）⇒ hooks 侧改用 `raw_arg` 原样拼命令行，**同一处陷阱在 `tools.rs` 的 `Bash` / `PowerShell` 上同样存在**（未在本批改动，已于同日第十批清理，见文末）；**(b)** 设置面板的「模型支持图片输入」开关**从未回读** `ai.json`（A8 的编辑没落盘）⇒ 面板每次都显示「关」，用户一按保存就把开着的功能静默关掉，本批补回；**(c)** `i18n.ts` 里 `agent.attachment_skipped` 有落盘 ⇒ 提示标题渲染成裸 key，本批补回。**实测**：`cargo test` core-agent **93 passed / 0 failed / 2 ignored**（其中 `hooks` 13 条，含退出码 2 / 非 0 / 超时 / 坏 JSON / matcher 不匹配 / deny 优先等语义）、src-tauri **58 passed / 0 failed / 1 ignored**、`tsc --noEmit` exit 0。**真机端到端 18 条断言全过**（假 Anthropic 端点抓请求体 + 真 hook 子进程 + 真 `Bash` 工具）：拦下 ⇒ 工具未执行 / 无审批卡 / 拒因以 `is_error` 回灌；放行 ⇒ 免卡且 `tool_result` 里出现 `exit code: 0`；**放行 + 危险命令 ⇒ 仍弹卡且 `analysis.dangerous` 非空**；`PostToolUse` 的 `additionalContext` ⇒ 第 2 次请求体出现 `[PostToolUse hook]`；`UserPromptSubmit` 拦下 ⇒ 端点**零请求** + `result.subtype=hook_blocked`；退出码 7 ⇒ 不拦（照常弹卡）且 `kind=error` 可见；`enabled:false` ⇒ 一条 hook 都不跑。另有一轮更省的复跑（`target\hooktest\e2e-mini.ps1`，**不联网**、不落 marker、纯 ASCII 断言，覆盖 `SessionStart` / `UserPromptSubmit` / `SessionEnd` / `enabled` 开关）**13 条断言全过**，可作为 `PreToolUse` 那几条缺 harness 时的最低成本回归。契约在 `ai-spec.md` §3.5「权限 hooks」、纪律在 §11 规则 61，前端字段登记在 `agent-ui-spec.md` §9，预检在 `code-rules.md` #23。**顺带记一笔**：`tools.rs` 的 `Bash` / `PowerShell` 仍用 `Command::arg` 拼 `cmd /C` 命令行（同一个引号陷阱），本批**未改** —— 留待单独一笔，避免与 hooks 混在一起回滚。当前未落地 **7 项**（A10–A16）。*

*2026-09-20 追加（同日第十批）：**清理 `cmd /C` 的 `raw_arg` 陷阱**（A9 复查的副产品，不是新功能；**本条不做成待办，属于已完成的清账**）。判据一句话：**只要整条命令字符串是交给 `cmd /C` 执行的，就必须 `raw_arg`** —— `Command::arg` 按 MSVC 规则把 `"` 转义成 `\"`（命令含空格时必然触发），而 cmd 不认这个转义。按此判据全仓清了四处：① `hooks.rs`（第九批已修）；② `tools.rs` 的 `Bash` —— 修复前实测 `echo "a b"` 输出 `\"a b\"`，修复后正常，单测 `quoted_shell_arguments_survive_the_command_line` 钉住；③ 宿主 `mcp_server.rs` 的 `run_shell_with_timeout`（**用户 `tools\*.json` 的执行通道**，命令里带引号是常态）；④ 宿主 `kill_port`（dev server 5173 的端口清理）—— 这条**两处都错**：除 MSVC 转义（cmd 报「此时不应有 \"tokens=5\"」、**taskkill 一次都没跑到**）外，`for /f ('…')` 里的 `netstat -ano 2>nul` 还得写成 `2^>nul`（裸 `2>nul` 被外层 cmd 抢先解释，报「此时不应有 2>」）；**正向对照**：自建监听端口后跑修正版命令 `exit 0` 并取到该进程 PID，未转义版 `exit 1` 无输出（末尾 `do` 子句的 `2>nul` 则**不能**转义，转了会被当文本打进输出）。**`PowerShell` 刻意不改**：它的解析器认得 `\"`，实测 `Write-Output "a b"` 输出正确 —— 第九批「两边都中招」的说法按实测收敛为「只有 `Bash` 这一侧中招」，单测里一并断言防回退。**实测**：core-agent **94 passed / 0 failed / 2 ignored**、src-tauri **58 passed / 0 failed / 1 ignored**。纪律写进 `ai-spec.md` §11 规则 61 与 `code-rules.md` 预检 #23（含「外层 cmd 读 / 子 cmd 读」这条判据）。当前未落地 **6 项**（A11–A16）。*

*2026-09-20 追加（同日第十一批）：**A10（自动权限分类器 → 只读分类）已完成并从本文删除**。形态经裁决：**判据落在 agent 侧、结论随 `analysis` 下发、只改「白名单」档、保守判定**。① **解决的问题是真洞**：原先「白名单」档的自动放行靠前端一张 `BUILTIN_SAFE_PREFIXES` **前缀表**，而前缀是**字符串匹配**，看不见重定向与管道 ⇒ `echo hi > important.txt`、`cat a.txt >> b.txt` 会被「echo / cat 是安全前缀」自动放行，等于**零询问地写文件**。② **判据**：`bash_safety::is_provably_readonly`（与危险规则**同一套** `split_subcommands` / `command_word`，口径不分家），四条全过才为 `true` —— 单条命令（`;` `&` `|` 换行一律不算，保守档不逐段判定）/ 无输出重定向（`>` `>>` `>& file`；`2>&1` 这类 fd 复制先摘掉再判）/ 无包装器与命令替换 / 命令词（+ 子命令词）命中**正向白名单**。刻意**不含** `find`（`-delete`）/ `sort -o` / `uniq IN OUT` / `sed -i` / `awk` / `tee` / `xargs` 这些「看着只读、实则有写入开关」的。③ **只减询问、不加闸门**：`false` **不表示危险**，只表示「证不出来」⇒ 照常弹卡；手动档照问、自动档照放（三档语义不变），且 `dangerous` / `opaque` 非空的命令**一律**拿不到 `readonly:true`（纵深防御）。④ **前端删表**：`classifyRequest()` 改看 `analysis.readonly`，**缺字段按不放行处理**（只认显式 `true`，不回落本地前缀表）；档位提示文案由「白名单前缀自动放行」改为「只读命令自动放行」（×5 语言）。⑤ **实测**：`cargo test` core-agent **95 passed / 0 failed / 2 ignored**（新增 `readonly_classification_is_conservative`，13 组用例逐条钉住放行与不放行）、`tsc --noEmit` exit 0；**真机**（假端点 + 真 `agent.exe --permission-prompt-tool stdio`）：`git status` ⇒ `analysis.readonly=true`、`echo hi > out.txt` ⇒ `analysis.readonly=false`（连续命令那条由单测覆盖 —— harness 第三次起 stub 端口复用会卡住，属测试脚手架问题，已记在 `e2e-a10.ps1` 旁边）。契约在 `ai-spec.md` §3.5「只读分类」、纪律在 §11 规则 62，前端在 `agent-ui-spec.md` §4.2 / §9，预检在 `code-rules.md` #24。当前未落地 **6 项**（A11–A16）。*

*2026-09-20 追加（同日第十二批）：**A12（本地成本面板）已完成并从本文删除**。① **价格不写进代码**是这次的定盘星：各家单价差十倍、官方还会调价，写死一个数字等于把错误金额当事实展示（用户是拿它对账的）⇒ 价格落**用户可编辑**的 `config\pricing.json`，单位**元 / 百万 token**，四类分开计价（`input` / `cache_read` / `cache_write` / `output`，字段名与用量日志一一对应），每条可带 `source_url` / `updated_at`；**刻意不预置任何价格数字**（没逐项核对过官方定价页，凭空填一行「看着很像」的数比留空糟得多）。② **算钱必须按「能拿到单价的最小粒度」分组** ⇒ 宿主新增 `read_usage_range(dates)`，一次读多天并汇总成 `UsageDay{date,turns,input,output,cacheRead,cacheCreate,models[]}`（**先按天、再按模型**；只按天合计会把两种模型的 token 混着乘一个单价）；金额在前端算（价格表用户随时会改，改完即时重算，不必再跑 IPC），**没价格的模型只标「未定价」+ 单列黄色提示、总额前缀 `≥`，绝不当 0 计**。③ **「更新价格」= agent 抓 + 面板预览确认**：面板自己没有网络也没有模型 ⇒ 注入提示词让 agent 用 `WebSearch` / `WebFetch` 查官方定价页、再用 `Write` 落**候选文件**；候选文件刻意落在 **agent 的工作目录**（`lunac-pricing.pending.json`）而**不是 `config\`** —— 配了工作区时 agent 的文件工具被硬锁在工作区内（`tools::guard()` 越界直接拒、连审批卡都没有）⇒ 写 `config\` 必然失败（**真机实测坐实**，见下）；面板列出「旧值 → 新值 / 新增 / 确认后失去价格」，用户点确认才 `commit_pricing_pending`（**先校验后覆盖**，校验不过**一个字都不写**；缺字段也算非法 —— 金额会悄悄少算一块）。④ **顺手修掉一个真 bug**：用量日志的 `model` **一直是空串** —— 前端 `let agentModel = ""` 声明了却**从未赋值**（注释还写着「来自 system/init」）⇒ 历史记录全都没有模型名。而**分模型计价正是这个面板成立的前提** ⇒ 在 `system/init` 分支补上赋值（`if (typeof data.model === "string" && data.model) agentModel = data.model`），历史空名记录在面板上显示 `—` 并计入「未定价」（如实，不猜）。⑤ **面板落位**：设置 · AI 分区的**第四块**「用量与成本」（`.settings-group-title` 3 → 4 个，回归清单已同步）：价格表状态 + 打开 / 更新两个按钮 + 候选项预览卡（`.cost-pending`）+ 汇总行 + 按天表格（新的在上），未定价单列提示；37 条 i18n key ×5 语言（含那段给 agent 的抓取提示词，**它是用户可见的一问**，同样不许硬编码中文）。⑥ **实测**：`cargo test` src-tauri **60 passed / 0 failed / 1 ignored**（新增 `usage_range_groups_by_day_and_model`（含空天跳过、驼峰字段名）与 `pricing_candidate_is_validated_before_commit`（6 组非法样本 / 拒绝时正式文件一字未改 / 确认后盖日期并删候选 / 缺候选报错））、core-agent **95 passed / 0 failed / 2 ignored**（未改动，回归通过）、`tsc --noEmit` exit 0；**真实日志**跑一次 `read_usage_range`（`usage-2026-09-17` / `-09-18` 两天：按天分片、四类 token 汇总与分模型分组均正确）；**真机端到端**（`core-agent\target\hooktest\e2e-a12.ps1`，假 Anthropic 端点 + 真 `agent.exe` + 工作区锁开：**5 条断言全过**）—— 写**工作目录内**的候选文件成功（按 JSON 读回 `input=2`），写**工作目录外**被拒（`Access denied: … outside the workspace`）⇒ 候选文件放工作目录的理由由实测坐实。契约在 `ai-spec.md` §3.5「定价表与成本面板」、纪律在 §11 规则 63，UI 在 `agent-ui-spec.md` §9（含设置面板第四块）与 §8 回归清单，预检在 `code-rules.md` #25。当前未落地 **5 项**（A11 / A13–A16）。*

*2026-09-20 追加（同日第十三批）：**A11（会话 id 与 rewind）已完成并从本文删除**。形态经用户裁决：**只补真实 `session_id` + rewind 扩到任意消息 + 不动磁盘文件**。① **`session_id` 从恒为 `""` 改成真值**：agent 侧 `session_id()`（`OnceLock` 惰性生成一次，形态 `sess_<pid>_<启动时刻 epoch 毫秒>`，**不引 chrono / uuid**），**7 处 JSON 字段**不再硬编码空串 —— `system/init`、成功 `result`、`finish_error` 的 `result`、启动期错误的 `result`、`hook_blocked` 的 `result`、`hook_tool_payload`、`fire_plain_hook`；**另有 1 行启动日志** `[agent] P1–P4 就绪 session=…`。它的语义是**一次 agent 运行**（不是一段对话）：宿主每次重启 agent（换模型 / 换思考档 / 换工作区 / 回退取消流式）就是新 id —— 上下文仍由 `set_history` 灌（规则 30），它**只做归因**（stdout、agent 落盘日志、前端用量记录三者从此对得上）。刻意**不把 session 塞进 `task_id`**（那要进模型可见的回灌文本，短才好读 —— 规则 54 约束② 原写「`session_id` 恒为 `""` 所以只能靠计数器归因」，本轮一并收敛为「跨运行归因靠 session、进程内归因靠计数器」）。② **前端接上消费者**：`system/init` 分支与 `model` 同一处记下 `agentSessionId`（`agentModel` 上一批才补过赋值，两处是同一类漏洞），随每次提问写进 `usage-*.jsonl` 的 `sessionId`（`UsageRecord` 加 `#[serde(rename="sessionId", default, skip_serializing_if="String::is_empty")]` —— 旧记录读成空串、空值不写键，**对账口径仍是 `ts` + `model`**）；**界面刻意不显示它**（它是排查用的归因标签，不是用户要看的状态）。③ **回退点从「用户轮」扩到任意消息**：`attachMsgActions` 重构为「复制所有气泡都有 / 回退只要消息还在会话里 / 重试只对用户提问成立」，助手气泡因此也挂回退按钮；实时对话里助手回复不是独立气泡（它渲染在 `agent-flow` 里），入口放在**回合页脚**（`.turn-rollback`，与旁边的「展开/收起」同族样式）。**仍不做**文件内容历史快照 —— 回退**只动对话与 agent 上下文**，按钮 title 与回退后的状态行都如实写明「磁盘上已改动的文件不会还原」。④ **顺手修掉一个既有真 bug（长对话必然踩）**：气泡上的 `data-idx` 是**渲染那一刻**的下标，而 `pruneContext()` 每回合从**队首**丢消息（12 轮上限）⇒ 丢 N 条后所有先前渲染的气泡都偏大 N，**回退会切错消息**、**复制会复制错**（`copyMsgText` 去取 `chatHistory[idx]` 的原文）。落地 `shiftRenderedMsgIdx(dropped)`：裁剪后把已渲染气泡 / 回合回退按钮的下标一起前移，并把**已被裁掉**的气泡（新下标 < 0）的回退 / 重试按钮摘掉（那消息已不在会话里，复制退化成按气泡文本复制）。⑤ **删掉一处「撒谎的死代码」**：`rollbackChat` 原先把整段历史快照写进 `localStorage` 的 `lunac-rollback-snapshots` 并注释「so the operation is reversible (no data loss)」，而**全仓没有读取方**（纯占配额）—— 删除，注释改成实话（回退**不可撤销**：裁剪后的会话立刻全删全插写回 `chat.db`）。⑥ **实测**：`cargo test` core-agent **96 passed / 0 failed / 2 ignored**（新增 `session_id_is_real_and_stable`）、src-tauri **61 passed / 0 failed / 1 ignored**（新增 `usage_record_session_id_is_backward_compatible`，并把 `usage_record_json_shape` 的逐字节断言更新为含 `sessionId`）、`tsc --noEmit` exit 0、`npm run build` 通过；**真机端到端**（`core-agent\target\hooktest\e2e-a11.ps1`，假 Anthropic 端点 + 真 `agent.exe`：**6 条断言全过**）—— `system/init` 的 id 形态合法、**pid 段 = 本次进程 pid**、毫秒段是可信时间戳、`result` 行同一 id、stderr 启动行同一 id、stdout 里**没有** `"session_id":""`（旧行为）。契约在 `ai-spec.md` §3.5「会话 id 与 rewind」、纪律在 §11 规则 64（六条不得回退），UI 在 `agent-ui-spec.md` §2 对照表 / §9（字段登记）与 §8 回归清单，预检在 `code-rules.md` #27（新增：DOM 里的下标必须跟着数据裁剪前移；回退类能力只承诺做得到的）。当前未落地 **4 项**（A13–A16）。*
