# 测试与验收规则

> **本文档是什么**：Lunac 的**构建 / 检查 / 验收链**——跑哪些命令、预检清单、BOM 纪律、真机验收要求，以及调试、代码审查、TDD 三条流程规范。
>
> **取代了哪些旧文档的哪些章节**：
> - `code-rules.md` 第 1 章（预检清单 #1–#77）、第 12 章（PR 前检查清单）、第 13 章（i18n 与提示精简）；
> - `ai-spec.md` §7（开发命令）、§8.2（打包流程与守卫）、§11 规则 16 / 35 / 43 / 68、§14（系统化调试）、§15（代码审查工作流）、§16（TDD）。
>
> **历史细节见 `docs/archive/code-rules.md`、`docs/archive/ai-spec.md`。** 被引用的预检编号（如「预检 #57」）仍指向 archive 的原文件。

---

## §测试 1 构建 / 检查 / 验收链

**改任何代码后，至少跑齐这几条（按顺序）：**

```powershell
# 1. 先停掉 dev 实例！运行中的 lunac.exe 锁着 target\debug，cargo 会报 os error 32（不是代码错）
Get-Process lunac -EA SilentlyContinue | Stop-Process

# 2. 前端类型检查（必过，exit 0）
cd app ; npx tsc --noEmit

# 3. 前端编译验证
npm run build

# 4. Rust 检查与测试（两侧 + 引擎）
cd ..\app\src-tauri ; cargo check --bins
cargo test --bins
cd ..\..\core-agent ; cargo test
cd ..\tuning-engine ; cargo test ; cargo clippy --all-targets   # 引擎须保持 0 warning

# 5. 仓库自检（新克隆自检 8 项；第 ⑧ 节扫 .ps1/.nsi 的 BOM）
cd ..\.. ; npm run verify
```

| 命令 | 作用 | 判据 |
|---|---|---|
| `npx tsc --noEmit` | 前端类型 | exit 0 |
| `npm run build` | 前端编译 | exit 0 |
| `cargo check` / `cargo test` | 宿主 + core-agent + 引擎 | 全绿（含守门单测） |
| `cargo clippy --all-targets` | 引擎 lint | 0 warning |
| `npm run verify` | 仓库自检（含 BOM 守卫） | 全 PASS |

- **守门单测**要随改动同步：内置工具总数、`dev_build_path_detection`、`fts5_is_compiled_in`、`merge_input_usage_*`、`budget_gate_*`、`phase_summaries_are_scoped_to_multi_round_tasks`、`librespot_args_pin_every_flag` 等——**改动对应代码必须重跑它们**。
- **改 `core-agent/` 后必须重编并重新部署 `agent.exe`**：dev 查找顺序里「core-agent 的构建」必须排最前（`target\{debug,release}\agent.exe` 只是构建 lunac 时平铺的快照，会遮蔽新构建）。日志里出现「忽略输入类型: Some(...)」= 正在跑的 agent.exe 是旧的。
- **改 `tuning-engine/` 会让宿主编译**（它是路径库依赖）⇒ 同样要先停 dev 实例。

## §测试 2 打包链（仅在用户明确说「打包」时执行）

```powershell
# 一个插件：改源码 → 打 dev 与安装包两条路（顺序固定，不许跳步）
powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1 -Plugin <id>

# 全量插件
powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1

# 安装包（自动 patch +1；-NoBump 保持版本重打，-Version 显式指定）
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1

# 发 Release（自更新用）/ 发插件到市场
powershell -ExecutionPolicy Bypass -File scripts\publish-release.ps1
powershell -ExecutionPolicy Bypass -File scripts\publish-plugins.ps1
```

- **插件改源码 ≠ 生效**：运行读的是盘上 `Modules\<id>\index.js`。发布顺序固定：改源码 → `build-plugins.ps1`（先铺 dev 的 `target\debug\Modules\<id>\`、再铺 `release\ext-plugins\`）→ dev 验证 → `build-release.ps1` → `publish-plugins.ps1`。**先 publish 后 build = 把上一版推上去**。
- **发布是独立的人工步骤，绝不自动**；摘插件时同一批必须把市场索引也发出去（否则插件既不在 bundle 也不在市场）。
- **`build-release.ps1` 守卫**：第 ⓪ 步读回确认六处版本载体；`release\ext-plugins\` 用**新鲜度**判据（插件源码最新 mtime ≤ 暂存清单最旧 mtime，否则 `throw`）；打包前后双校验**必须含 `agent.exe`、不得含 `cli.exe`**。
- **插件版本号与应用版本解耦**：包内容变了才 +1 那个插件自己的 `version`，别按内容哈希自动推版本。
- **日常修改不打包**，做编译验证即可。

## §测试 3 BOM 纪律（含中文的 `.ps1` / `.nsi` 必须恰一份 BOM）

- Windows PowerShell 5.1 / makensis 对无 BOM 的这类文件按 ANSI(GBK) 解码，中文会吃掉引号导致**解析失败**（报错行号还很靠前，与真因不符）。
- **两个方向都会坏**：编辑器可能剥掉 BOM，也可能**多加一个**（首六字节 `EF BB BF EF BB BF`）。多加时首行变成命令、`param` 不再绑定参数——报的却是「无法将 `#` 项识别为 cmdlet」。
- **判据**（改完 `.ps1` / `.nsi` 后跑一次）：前 3 字节 = `EF BB BF` **且第 4 字节不是 `EF`**。

  ```powershell
  $b=[IO.File]::ReadAllBytes("<file>.ps1"); ($b[0..5] | % { $_.ToString('X2') }) -join ' '
  # 期望 EF BB BF 23 ...（恰好一个 BOM + 首字符）
  ```
- **修法**（归一成恰好一份）：读出全部字节 → 跳过开头所有 `EF BB BF` → 用 `UTF8Encoding($true)` 重写。**不要用编辑器加/去 BOM。**
- **持久化防护**：`commit.ps1` 在 `git add` 前归一 BOM；`build-plugins.ps1` / `build-release.ps1` 在写操作前自愈守卫。改完 `.ps1` / `.nsi` 一律跑 `npm run verify`。
- 其余文件（`.ts` / `.rs` / `.css` / `.md`）**一律不带 BOM**。

## §测试 4 预检清单（每次修改必查）

改动前照这几条过一遍（**完整 70+ 条见 archive 的 `code-rules.md` 第 1 章**，本文只留高频项）：

- [ ] 新增 JS 调 Tauri API，`capabilities/default.json` 是否已声明权限？
- [ ] 新增 `invoke()` 参数名/类型是否与 Rust 签名完全匹配？
- [ ] 是否有不必要的串行 `await`（隐藏 / 启动 / 读写应并发）？
- [ ] 新 CSS 是否有 `overflow: hidden` 会裁剪 WebView2 弹窗？
- [ ] 新增 UI 字符串是否走 `t()` 并在 `i18n.ts` 补翻译（至少 zh-CN + en）？
- [ ] 新增按钮 / 控件是否**显式写了 CSS**？（WebView2 没有自动继承主题的原生按钮；`border-style` 不得是 `outset`，背景不得是 `buttonface`）
- [ ] 改动了发往端点的 `history`（尤其末尾消息形态）时，`read` 是否没塌到 `system + tools` 量级？（只允许拼进已有 `tool_result` 内部）
- [ ] 新增 / 改名内置工具，是否同步了四处：`tools.rs` 的 `defs()` + 三张判断表、契约表、`agent-implementation` 工具总数、前端黑名单候选名单？跑工具总数守门单测了吗？
- [ ] 绕过 `tools::run()` 的早退分支（`Skill` / `SessionSearch` / `needs_bridge` / `Agent` / fork 技能）是否自己补了写类拦截？
- [ ] 新增「每轮都要发」的提示词段落，是否启动时取一次、进程内恒定、且与工具池同源判断？
- [ ] 每一条会进 `history` 的路径（工具结果、后台命令回流、附件、注入块）是否都过预算 / 有硬上限？
- [ ] 新增 `chat.db` 表列，是否同步了 `ALTER TABLE … ADD COLUMN` 与 `read_all` 的 `SELECT`？
- [ ] 改 `.ps1` / `.nsi` 后 BOM 恰一份、`npm run verify` 过？

**三条高优先级预检**（archive `code-rules.md` 有完整版）：

- **#40 用户可见提示必须精简**：默认不说过程；一个动作最多一条提示；失败只说「哪一步没成 + 用户能做什么」，原始错误进日志；开发者向数据不进主界面。判据见 [`ui-style.md`](./ui-style.md) §界面 8。
- **#57 插件起的子进程必须随插件退出而终结**：`spawn()` 之后立刻 `child_job::assign`；长期驻留的注册「插件退出 ⇒ 杀死」。判据：界面上再没有地方能控制它，它就不该继续跑。
- **#65 插件改动的发布流程有序**：改源码 ≠ 生效，必须按 §测试 2 的顺序走完。

## §测试 5 真机验收要求

- **前端没有自动化 UI 测试**（只有 `tsc` + `vite`）⇒ 凡改到界面 / 交互，**必须起 dev 实例真机点一遍**，别只看代码像对的。
- 起法：`cd app ; npm run tauri:dev`（占用 5173，会拉起 vite + `cargo run`）。
- **用 CDP / 探针取证**（只读），别凭截图猜；改渲染后要**真的点一次**（CDP 派发 `click` 也算）。**探针开头先清脏状态**：上一探针没关插件会让 `#search-bar` 带 `plugin-locked`、`runSearchNow()` 直接 return；遗留附件会让结果区只走文件分支——先断言 `getComputedStyle(#search-input).display !== "none"` 且 `__lunac_attached_files.length === 0`（HMR 重跑会叠加监听，必要时先 `location.reload()`）。
- **agent 侧可端到端驱动**：`agent.exe` 是纯 stdin/stdout，用 `Start-Process -RedirectStandardInput/-Output/-Error` 单发一行 JSON 再关 stdin 即可跑完（**不要**交互式读循环，会变常驻僵尸）。**已封装成可复用脚本** `scripts\agent-e2e.ps1`（走真实 API、捕获完整 stream-json、带断言）——见 §测试 10。
- **验收口径 = 真实数据对账**：如本地用量账要拿平台 CSV 跑 `reconcile-usage.ps1` 且 `总差额 = 0.000000`、exit 0；别用「大概差不多」交差。

## §测试 6 测试用模型

- 任何**真实端点测试**（工具往返、权限审批、上下文压缩、MCP 桥等）把 `AI_MODEL` / `LUNAC_AGENT_MODEL` 指向 **`deepseek-flash`**，**不要用 `deepseek-v4-pro`**——测试只验证链路、契约与结构，flash 足够且更快更省；只有问题与回答质量本身相关、或需复现线上行为时才用 pro。

## §测试 7 系统化调试规范

> 铁律：**不找到根因，不写修复代码。** 对症状打补丁 = 失败。

- **Phase 1 根因调查**（必做）：仔细读错误信息 → 构建紧反馈循环（一条命令触发症状）→ 检查近期变更（`git log` / `git diff`）→ 在组件边界采集证据 → 追溯数据流。
  - 完成清单：□ 错误已理解 □ 紧循环可运行且红 □ 近期变更已审查 □ 证据已采集 □ 问题隔离到具体组件。
- **Phase 2 模式分析**：最小化复现 → 找正常示例 → 对比差异 → 理解依赖。
- **Phase 3 假设与测试**：列 3–5 个可证伪假设按概率排序；**一次只测一个变量**；不加补丁叠补丁。
- **Phase 4 实现修复**：先写复现测试；只修根因、一处改动；**≥3 次修复失败就停下来质疑架构**，别试第 4 次。
- **红旗（立即回到 Phase 1）**：「先快速修一下」「试改改 X」「同时改多处」「大概就是 X」「再来一次」（已试过 ≥2 次）、每次修复都在别处暴露新问题。

## §测试 8 代码审查工作流

> 核心原则：**不要让写代码的 agent 自己审自己**——独立上下文才能发现盲区。

8 步预提交管道：① 取 `git diff --cached`（>15K 字符按文件拆）② 静态安全扫描（仅新增行：硬编码密钥 / shell 注入 / eval / 反序列化 / SQL 注入）③ 基线测试与 lint（只拦**新引入**的失败）④ 自查清单（无密钥 / 输入校验 / 参数化 SQL / 路径无穿越 / 外部调用有错误处理 / 无残留 debug / 无注释掉的代码 / 新代码有测试）⑤ 独立审查（派生子 agent，只给 diff + 扫描结果，fail-closed）⑥ 评估 ⑦ 自动修复循环（最多 2 次，不改代码 / 不重构 / 不加功能）⑧ 提交（`[verified]` 前缀）。
- 注：本仓的静态安全扫描等价物是 `bash_safety.rs`（扫**命令**、执行前）与 `content_safety.rs`（扫**文件内容**、写入前）——**没有**「落盘后再扫、注入下一轮上下文」的环节。

## §测试 9 TDD 与提交

- **铁律：没有先失败的测试，不写产品代码。** 测试没写就写代码 ⇒ 删掉重来，没有例外。
- RED（写失败测试，一次一个行为）→ GREEN（最小实现）→ REFACTOR（消除重复）。
- 禁止「水平切片」（先写完所有测试再写实现）；卡住时先回到 Phase 1 调查。
- **频繁提交**（每个任务完成后提交）；commit message 含中文时用 `git commit -F <UTF8 文件>`。
- **提交前必过**：`npx tsc --noEmit`、`cargo check`、`cargo test`（`core-agent` 与 `src-tauri` 两侧）。首次提交前用 `git diff --cached --name-only` 确认无 `.env`、无 MB 级文件。

## §测试 10 AI 侧无头端到端实测（`scripts\agent-e2e.ps1`）

- **定位**：AI 侧的**可脚本化实测**手段——直接拉 `core-agent` 的 `agent.exe`**走真实 API**，喂 query、捕获完整 `stream-json` 事件流、产出结构化结果并做「预期效果」断言。用于「多次单测 + 模块化混测」时反复验证改动是否达到预期。
- **目标产物 = release（纪律）**：脚本**默认瞄准 release 产物** `core-agent\target\release\agent.exe`，配套的 lunac 数据根也取 release 布局（`app\src-tauri\target\release`）。理由 = **交付目标 = 用户实际安装 / 运行的版本**，端到端实测必须对着它跑，而不是对着 dev 跑完再假设 release 一样。**dev 只是显式例外**：加 `-Dev` 才指回 `core-agent\target\debug\agent.exe`。
  - **开关名 `-Dev`（非 `-Debug`）的理由**：脚本带 `[CmdletBinding()]`，它自带的公共参数已占用 `-Debug`，再声明同名参数会抛「参数名 'Debug' 被多次定义」；`-Dev` 亦更贴合「指回 dev 产物」的语义。
  - **缺失即失败、绝不静默回落**：默认（release）产物不存在时，脚本打印可执行提示「**先构建 release 产物：`cargo build --release`（cwd=`core-agent`）**」并以 **exit 2** 失败；显式传了 `-Dev` 而 dev 产物不存在时同样报「先构建 dev 产物：`cargo build`（cwd=`core-agent`）」并 exit 2。**不会**从「缺 release」偷偷改用 debug（静默回落正是本条纪律要纠正的毛病）。
  - **宿主可执行文件（仅 `-McpExe` 接 MCP 桥时用）**：release 布局 = `app\src-tauri\target\release\lunac.exe`，dev = `app\src-tauri\target\debug\lunac.exe`，**安装版 = 安装根目录**（如 `$INSTDIR\lunac.exe`，由 `build-release.ps1` / `lunac-installer.nsi` 把 `lunac.exe` 与 `agent.exe` 平铺到同一目录）。`-McpExe` **无默认值**（默认不接桥）；要接就显式传对应布局的 `lunac.exe`。判据：release 下脚本**不会**去猜 dev 的 `lunac.exe`。
  - **`-ConfigRoot` / `.env` 的 dev 例外**：`-ConfigRoot` 随目标布局（默认 release → `app\src-tauri\target\release`，`-Dev` → `...\target\debug`），用于读该布局的 `config\ai.json`；默认 `.env`（`app\src-tauri\.env`）是**dev 侧凭据来源**（仓库源树文件，装不进安装包）——安装版凭据只在 `config\ai.json`。二者都可用参数显式覆盖。
- **两条路径（并存，按场景选）**：
  - **常驻 stdio（默认）**：`agent.exe` 是**常驻 stdio 循环**——`stdin` 每行一条 JSON（`{"type":"user",...}`），`stdout` 每行一条 JSON 事件；脚本把 user 行写进 stdin、关 stdin、等进程退出。能给**完整事件流 + 工具序列 + 逐请求 usage**（`stream.jsonl` + `summary.json`），适合多问 / 断工具往返 / 模块化混测。
  - **单发 `--print`（加 `-Print`）**：走 `agent.exe --print --output-format json`，一次性跑**恰好一轮**提问后退出；query 由 stdin 原文喂入，stdout 恰好一行 `result` JSON。更简单，只给**最终文本 + usage + 成败**（json 输出不含工具序列事件 ⇒ 单发不支持 `-ExpectTool`）。两条路径的凭据解析 / 落盘目录 / 脱敏完全一致。
- **`--print` 单发模式的语义（agent 侧，2026-10-11 实装）**：
  - **触发**：带 `--print` / `-p` 且**未**显式声明 `--input-format stream-json` 时生效（后者 = 上游常驻 stdio 流式输入模式，宿主 `start_cli_process` 一直这么传）⇒ **常驻模式不受影响**。
  - **query 来源**：`--print` 紧跟的那个**非 flag** 值；没给就从 stdin 读**一次**（整段当一条 query）。
  - **`--output-format`**（默认 `text`）：`text` = 只把最终助手文本原样打到 stdout（无任何 JSON 包装）；`json` = **一个** `result` JSON（最终文本 + usage + 是否 error，形状沿用既有 `result` 事件）；`stream-json` = 沿用完整事件流、收到 `result` 后退出。`--include-partial-messages`（2026-10-14 实装）：单发 `stream-json` 下**额外**逐条输出 `stream_event` 增量行（`content_block_start` / `content_block_delta` / `content_block_stop` / `message_stop`，每行一条 JSON、当场 flush），默认**关**（不开只出最终完整消息）；**常驻模式不受它影响**（宿主 / 前端本就需要增量做打字机效果，`stream_event` 恒发 ⇒ 宿主协议逐字节不变）。
  - **⚠️ 工具审批策略（单发无回包通道）**：单发**没有**前端回 `control_response` 的通道 ⇒ 默认**直接拒绝**需审批的工具（`Write` / `Edit` / `Cmd` / `PowerShell` / `WebFetch` / `WebSearch` / `AskUserQuestion` / 全部 MCP），**绝不静默挂起**（不会干等 300s 超时）；被拒会写 stderr 告警并置失败标志。要放行需显式 `--dangerously-skip-permissions`；`--permission-mode plan` 下只读工具本就免审批。项目 `.mcp.json` 信任门在单发下**跳过**、`elicitation` **直接 decline**。
  - **退出码**：成功 `0`；模型 / 网络失败、或**有任何工具因单发策略被拒** ⇒ 非 0（即使模型仍给出了最终回答）。`text` / `json` 下错误信息也在 stdout（错误文本 / `result.is_error`），原始错误同时进 stderr。
- **`-RawStdin <file>`（协议级测试用，2026-10-11）**：把文件里的行**原样**写进 agent stdin（**不做** user-JSON 组装、不改写字节），用于喂**原始 JSON 行**验证 **stdin 协议处理**（例：首行一条 `user`、次行 `{"type":"compact"}`）。走**常驻 stdio 路径**（与 `-Print` 互斥），此时**不校验 result 条数**（`queries` 可为空）。配套断言用 `-ExpectEvent`；`summary.json` 增 `raw_stdin`（是否原样喂）与 `system_events`（捕获到的全量 `system` 事件子类型 = 证据来源）。
- **凭据不落明文**：脚本按 `参数 > 进程 env > <config>\ai.json > app\src-tauri\.env > 内置缺省` 解析 key/端点/模型（key 不进脚本、不进日志、不进输出，所有进屏幕/落盘的文本都过脱敏）。端点按宿主 `agent_endpoint` 同规则构造（剥末尾 `/v1` 再拼 `/anthropic`；agent 侧再拼 `/v1/messages`）。
- **捕获的「中间端点结果」**：每次运行的 `stream.jsonl` 含全部事件——`system`（`init` 带工具池 / `api_retry` / `context_compacted`）、`stream_event`（逐字增量）、`assistant`（含 `tool_use`，可提取**工具序列**）、`user`（含 `tool_result`）、`usage_delta`（**每请求**用量，流式）、`result`（`usage.requests[]` = 逐请求明细 + `usage.subagent`）。`summary.json` 汇总每条 query 的最终文本 / 工具序列 / usage / 是否报错。⇒ **每次 API 请求的用量级证据齐全**；**完整 request/response 报文**要靠 `-DumpApi`（见下一条，agent 侧 `LUNAC_DUMP_API` 开关）。
- **`-DumpApi` / `LUNAC_DUMP_API` 报文转储（默认关）**：
  - **用法**：给 `scripts\agent-e2e.ps1` 加 `-DumpApi` —— 脚本只在该开关打开时给子进程注入 `LUNAC_DUMP_API=1`（否则显式置空，避免继承宿主环境里的同名变量）；不传就是关。开关本身在 **agent 侧**读（`core-agent/src/log.rs`），值取真（`1` / `true` / `on` / `yes`，忽略大小写与空白）。
  - **落点**：`<log_dir>\api-dump.jsonl`（agent 的 `log_dir` 解析见 `core-agent/src/log.rs` 的 `log_dir()`，与 `agent-*.log` 完全同源）。脚本已把 `LUNAC_LOG_DIR` 指到 run 目录 ⇒ 文件落在 **run 目录的 `logs\api-dump.jsonl`**；`summary.json` 里 `dump_api` / `api_dump_path` / `api_dump_lines` 三个字段注明本次是否开了转储、文件路径与行数。一行一条 JSON，便于 `grep` / `jq`。
  - **记什么**：每次 API 调用（主循环流式请求、子代理非流式请求、摘要非流式请求）的 request / response 各一行，字段含 `ts`（UTC）、`kind`（`request` / `response`）、`req_id`、`model`、`status`（仅 response）、`body`（已脱敏）。`req_id` 关联规则：主循环 `q<第几次提问>-r<本问第几个请求>`（与 `result.usage.requests[]` 同序、也是本问第 N 条 `usage_delta`；重试加 `-a<n>`）、子代理 `sub-<task_id>-r<轮次>`、摘要 `summarize-<epoch_ms>`（不与 usage 明细对应）。网络错误也会记（`status` 为 `null` + `error`）。
  - **不记什么（硬约束）**：**请求头一个都不记**（连头名也不记）——`Authorization` / `x-api-key` 等承载 key 的头绝不落盘；`body` 一律先过 `mask_secrets()` 再写。**流式响应记「累积后的最终报文」**（content 块 + usage），**不记**原始 SSE 逐字行（逐字事件会把转储撑大几十倍且几乎不可读；需要逐字时序看 `stream.jsonl`）。
  - **体积上限**：单条 `body` 截断到 **64K 字符**，超出记 `truncated:true` + `body_chars`（实际字符数）。
  - **纪律**：写转储失败只 `warn`、绝不影响主流程；开关关闭时热路径零开销（不做任何 req_id 拼接 / 序列化）。
- **不污染数据**：`LUNAC_LOG_DIR` 与工作目录都指到 `temp\agent-e2e\<run>\` 下的独立子目录，agent 的 `logs` / `tool-outputs` / `images` / `review-last` 全部由此派生（`core-agent/src/log.rs` / `tools.rs` 的 `output_dir` 派生）⇒ **不写 `ModuleData`**。默认不接 MCP 桥（`-McpExe` 才接）。**`-WorkspaceDir <目录>`**（可选，目录须**已存在**）：覆盖 agent 的 `WorkingDirectory` 与 `--add-dir`（两处都用它），用于构造**多层 `AGENTS.md`** 等夹具场景（不传 = 沿用本 run 的独立 `workspace`，行为不变）。
- **用法**：

  ```powershell
  # 冒烟（常驻 stdio 路径）：默认走 release\agent.exe，断言最终文本含 PONG
  powershell -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Query "Reply with the single word: PONG" -ExpectContains PONG

  # 冒烟（单发 -Print 路径）：更简单，只取最终文本 / usage / 成败
  powershell -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Print -Query "Reply with the single word: PONG" -ExpectContains PONG

  # 显式改用 dev 构建（例外）：指回 target\debug\agent.exe
  powershell -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Dev -Query "Reply with the single word: PONG" -ExpectContains PONG

  # 工具往返：断言工具序列里有 Read（只读档 plan 下 Read/Glob/Grep 免审批；需 stdio 路径）
  powershell -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Query "Use the Read tool to open <path> and reply with its first line." -ExpectTool Read -ExpectNoError

  # 多问（文件）/ 覆盖模型 / 覆盖端点 / 接 MCP 桥（记忆与复盘需要它；宿主 exe 用 release 布局路径）
  powershell -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -QueryFile q.txt -Model deepseek-flash
  powershell -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -Query "hi" -BaseUrl https://api.deepseek.com -McpExe app\src-tauri\target\release\lunac.exe

  # 协议级：把文件里的原始 JSON 行原样喂 stdin（首行一条 user、次行 {"type":"compact"}），
  # 断言事件流里出现 system/context_compacted
  powershell -ExecutionPolicy Bypass -File scripts\agent-e2e.ps1 -RawStdin raw.jsonl -ExpectEvent context_compacted -Model deepseek-flash
  ```

- **`-Print` 断言范围**：只支持 `-ExpectContains` / `-ExpectNoError`（json 输出无工具序列事件；传 `-ExpectTool` 会以用法错误 exit 2，请改用 stdio 路径）。

- **断言开关**：`-ExpectContains`（最终输出含，任一 result 命中即算）、`-ExpectTool`（工具序列里有）、`-ExpectNoError`、`-ExpectSubagentUsage`（有子代理 / 复盘用量）、`-ExpectEvent`（事件流里出现指定的 `system` 事件子类型，如 `context_compacted`；协议级断言，配 `-RawStdin` 用）、`-ExpectSamplingCards <int>`（常驻 + `-AutoApprove` 路径上收到的 `mcp_sampling` 授权卡**张数**；用于验证「本会话记住」—— 同一会话内连发两次 sampling 只应弹 1 张卡；`-1` = 不断言）。
- **`-McpConfigFile <file>`（2026-10-11）**：注入 `LUNAC_MCP_FILE`，让 agent 连配置文件里的**远端 Streamable HTTP** 服务器（常驻 + `-Print` 两条路都注入）；用于验证 **HTTP 反向通道**（服务器在 `tools/call` 的 SSE 响应流里反向发 `sampling/createMessage`）。不传时显式置空，避免继承宿主环境里的同名变量。
- **`-AutoApprove` 的实现注意（2026-10-11 修）**：交互式回包器往 agent stdin 写的第一行会被**吞掉**（Windows + PS5.1 + .NET Framework：agent 侧收到一条空行 → 「忽略非法 JSON 输入行」）⇒ 写真正的输入前**先垫一个空行**（空行无害，agent 会跳过）。缺这一步时 `-AutoApprove` 路径在本机**完全不可用**（超时、0 张卡）。顺带：`-ExpectStderr <a>,<b>` 经 `powershell -File` 会被并成**单个**字符串（不是数组）⇒ 多子串断言请分次或用单个复合标记。
- **退出码**：`0` = 全过；`1` = 断言失败 / 超时 / 进程非零退出；`2` = 环境或用法错误（凭据缺 / exe 缺 / 文件读不到）。默认缺 release 产物时先给「`cargo build --release`（cwd=`core-agent`）」的构建提示再 exit 2（`-Dev` 缺 dev 产物同理给 `cargo build` 提示）。
- **真端点测试用 flash**：同 §测试 6，`AI_MODEL` / `-Model` 指向 `deepseek-flash`，别用 pro。带测试的脚本一律 UTF-8 with BOM（§测试 3）。

## §测试 11 用例 → 目标 矩阵（dev vs release / 安装版）

> **交付目标 = 用户实际安装 / 运行的版本**。**判定基准 = release / 安装版**。本节把「每类用例该在哪一侧跑」列成表，与 §测试 10 的「目标产物 = release（纪律）」同源。
> 本节只解决「某个用例该在哪一侧跑」；§测试 5 那条「UI 改动要真机点一遍」不受影响，只是这一遍应尽量点在 release。

### §测试 11.1 判定基准（release 是唯一裁判）

| 观察到的情况 | 判定 |
|---|---|
| release / 安装版**通过** | **算过** —— 不必再为同一个用例补一次 dev 复核 |
| **只有 dev 失败**（release 没跑 / 没复验） | **不算数** —— 必须到 release 复验；dev 这一侧的失败**不能**作为「有 bug」的结论 |
| **dev 通过、release 失败** | **一定是环境 / 时序 / 打包问题**，必须查；**不得**用「dev 通过」推翻 release 的失败 |

### §测试 11.2 三类分工（用例 → 该跑哪一侧）

| 类别 | 在哪一侧跑 | 用例举例 | 为什么 |
|---|---|---|---|
| **只在 release**（逻辑 / 协议级；两边都跑即重复） | release | 与渲染 / 打包无关的逻辑与协议级：`cargo test`、`tsc`、`agent-e2e.ps1` **直连 `agent.exe`** 的协议级断言、宿主命令级、纯逻辑断言 | 这些不依赖界面渲染与打包形态，dev / release 行为应逐字节相同；分别在两侧各跑一遍纯属重复 |
| **只在 release**（dev **不可替代**） | release | 安装布局与数据根；插件市场与**扩展插件缺失场景**；安装包与卸载；开机自启与自更新；资源 / 压缩产物；真实 `ModuleData` / `webview-data` 下的持久化断言 | 只有在「安装版布局」下才成立，dev 目录里根本不存在对应形态 |
| **必须成对（dev + release）** | dev **且** release | 时序 / 状态敏感的 UI 端到端；怀疑与构建 / 打包相关的复现；涉及 `localStorage` / 持久化状态的断言 | 结论可能因两侧状态 / 时序不同而**相反** ⇒ 两侧都看，且**以 release 为准** |

### §测试 11.3 dev 会把结论带偏的三条机制（逐条）

- **① HMR 叠加监听 ⇒ 假失败 / 假通过**：dev 下 Vite HMR 重跑会**叠加事件监听**（见 §测试 5「用 CDP / 探针取证」那条纪律）—— 断言可能命中上一轮遗留的监听器：**假失败**（旧监听抢答）或**假通过**（旧状态仍在）。E2E / 探针开头先清脏状态、必要时先 `location.reload()`。
- **② 状态不同（两套数据根）**：
  - **dev** 数据根 = `app\src-tauri\target\debug\ModuleData`；dev webview 数据 = `app\src-tauri\target\debug\temp\webview-data`。
  - **安装版** = `<exe 根>\ModuleData`；`<exe 根>\temp\webview-data`（如 `D:\Lunac\ModuleData` / `D:\Lunac\temp\webview-data`）。
  - ⇒ 外观设置、插件开关、记忆 / 历史**各存两套**。故「外观复扫」「记忆类」等断言**不可比**：在一侧成立不代表另一侧成立。
- **③ 时序差 ⇒ 时序敏感断言的结论可能相反**：同一时序敏感用例（如 `temp\pet-e2e.mjs` 的 CASE5「同窗普通聊天」）**在 release 上失败过**；**即便它在 dev 上通过，也绝不能据此判「不是 bug」** —— 差异可能来自时序 / 状态。结论只能按「目标 + 数据根」分别定位（本次即按「对 `D:\Lunac` 多次重跑」定位，**未**用 dev 放行）。

### §测试 11.4 硬纪律：测前必须声明数据根

- **每次测试前先声明用的是哪一套数据根** —— dev 的 `target\debug\ModuleData`，还是安装版 exe 根（如 `D:\Lunac\ModuleData`）。**不得拿一边的结论推断另一边**（呼应 §测试 11.3 ②）。
- **「换出来的应用 `Modules` 为空」是正常的、不是缺口**：全新 / 自建构建的 `Modules` 为空属预期 —— 项目里**基础插件随安装包分发**、**扩展插件需从插件仓库拉取**。因此「另一处换出来的应用 `Modules` 为空」不能当 bug。

### §测试 11.5 `temp\pet-e2e.mjs` 的两个目标跑法

CDP 端到端夹具（`temp\` 下、被 gitignore、**不入库**）；一次性跑完「启动 → 驱动 → 观测」（必须在**单个命令内**，否则沙箱会回收 GUI 进程）。

| 目标 | 命令 | 说明 |
|---|---|---|
| **默认**：仓库 release 产物 | `node temp\pet-e2e.mjs` | `EXE` 默认 = `app\src-tauri\target\release\lunac.exe`、`CWD` 默认 = `app\src-tauri` |
| **安装目录**（release 认定口径） | `$env:PET_E2E_EXE="D:\Lunac\lunac.exe"; $env:PET_E2E_CWD="D:\Lunac"; node temp\pet-e2e.mjs` | 用 `PET_E2E_EXE` / `PET_E2E_CWD` 指到安装根；**判定基准以这一侧为准**（见 §测试 11.1） |

> 注：CASE5 曾在 release 上**偶发 FAIL**，已定位为**夹具时序竞态**（没等上一轮收尾就在同一窗发起下一轮，撞上 `startAIChat` 开头的 `if (isStreaming) return;` 被静默丢弃），夹具已加 `waitTurnIdle` 清场 —— 这正是 §测试 11.3 ③ 说的「时序敏感」；定位靠「对 release 多次重跑」，未用 dev 放行。

---

> **历史细节见 `docs/archive/code-rules.md`（完整预检清单 #1–#77）与 `docs/archive/ai-spec.md` §14 / §15 / §16。**
