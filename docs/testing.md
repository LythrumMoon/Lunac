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

# 2b. 前端协议单测（插件信道 / 纯逻辑，vitest）
npm test

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
| `npm test`（app） | 插件信道协议单测 | 全绿 |
| `npm run build`（app） | 前端编译 + **媒体代理 helper**（`app/media-proxy` release） | exit 0；`media-proxy/target/release/lunac-media-proxy.exe` 存在 |
| `cargo check` / `cargo test` | 宿主 + core-agent + 引擎 | 全绿（含守门单测） |
| `cargo clippy --all-targets` | 引擎 lint | 0 warning |
| `npm run verify` | 仓库自检（含 BOM 守卫） | 全 PASS |

- **守门单测**要随改动同步：内置工具总数、`dev_build_path_detection`、`fts5_is_compiled_in`、`merge_input_usage_*`、`budget_gate_*`、`phase_summaries_are_scoped_to_multi_round_tasks` 等——**改动对应代码必须重跑它们**。
- **改 `core-agent/` 后必须重编并重新部署 `agent.exe`**：dev 查找顺序里「core-agent 的构建」必须排最前（`target\{debug,release}\agent.exe` 只是构建 lunac 时平铺的快照，会遮蔽新构建）。日志里出现「忽略输入类型: Some(...)」= 正在跑的 agent.exe 是旧的。
- **改 `tuning-engine/` 会让宿主编译**（它是路径库依赖）⇒ 同样要先停 dev 实例。

## §测试 2 打包链（仅在用户明确说「打包」时执行）

```powershell
# v2（Flutter）安装包：编 core-agent release + flutter release + 暂存 + Inno Setup
#   （没装 Inno Setup 6.7+ 时自动退回 NSIS 脚本）
powershell -ExecutionPolicy Bypass -File scripts\build-release-v2.ps1
#   -Version 1.0.1 显式指定版本（并写回 app-flutter\pubspec.yaml）；-NoBuild 只重暂存 + 打包
# 产物：release\Lunac-<version>-Setup.exe（暂存目录 release\LunacV2\）

# v1（Tauri，遗留）：仅当明确要打旧版时才用
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1

# 一个插件：改源码 → 打 dev 与安装包两条路（顺序固定，不许跳步）
powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1 -Plugin <id>

# 全量插件
powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1

# 发 Release（自更新用）
powershell -ExecutionPolicy Bypass -File scripts\publish-release.ps1

# 发插件到市场（构建 + 拷贝/索引 + push + 线上校验，一条命令）
powershell -ExecutionPolicy Bypass -File scripts\publish-plugins.ps1 -Build
# 或构建时直接带上发布（不再单独跑 publish）
powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1 -Publish
```

- **v2 打包口径**：`build-release-v2.ps1` 每次都**重编 `core-agent` release**（`agent.exe` 随包更新），
  再 `flutter build windows --release`；CMake 的 `lunac_core` 按 `$<CONFIG>` 走
  `rust\build.ps1 -Profile Release`，DLL 取 `rust\target\release\lunac_core.dll`。
  暂存只收 Flutter 运行必需文件（exe / DLL / `data\` / `Modules\pet\engine` / `skills` / `tools`），
  明确剔除 `config\`、`ModuleData\`、`temp\`、`*.pdb` 等 dev 残留；Inno Setup 编译成功即视为打包成功
  （Inno 输出是单文件 LZMA2，7z 无法再列目录）；退回 NSIS 时仍用 7z 复核
  **必含 `agent.exe` / `lunac.exe` / `lunac_core.dll` / `flutter_windows.dll` / `icudtl.dat`、不得含 `cli.exe`**。
- **安装包前端优先用 Inno Setup 6.7+**：`scripts\lunac-v2-installer.iss` 用内置
  `WizardStyle=modern dark includetitlebar`（页面 / 按钮 / 标题栏全暗色），目录页上直接挂
  「开机自启 / 桌面快捷方式」两个复选框，完成页用 `postinstall` 给「Open Lunac now」。
  找不到 `ISCC.exe` 时退回 `scripts\lunac-v2-installer.nsi`（MUI 深色版，按钮为原生控件）。
  构建机安装 Inno Setup 后，`build-release-v2.ps1` 会自动优先使用它（也认
  `tools\InnoSetup6\ISCC.exe` 与 `%LOCALAPPDATA%\Programs\Inno Setup 6\`）。

- **插件改源码 ≠ 生效**：运行读的是盘上 `Modules\<id>\index.js`。发布顺序固定：改源码 → `build-plugins.ps1`（先铺 dev 的 `target\debug\Modules\<id>\`、再铺 `release\ext-plugins\`）→ dev 验证 → `build-release.ps1` → `publish-plugins.ps1`。**先 publish 后 build = 把上一版推上去**。
- **资源库（`resource-hub`）是外部工程插件（2026-11-01 起纳入正式插件包）**：它不在 `app/vite.plugins.config.ts` 的入口表里，产物由 `resource-hub` 自己的 `npm run build` 生成。**仍然只跑 `build-plugins.ps1`** —— 脚本会先构建它、再把 `apps\lunac-plugin\dist\` 拷进 `app\plugin-dist\resource-hub\`，之后走与其余插件完全相同的 zip / `ext-plugins` / dev `Modules` 三条路。它的**版本号唯一真相源**是 `resource-hub\apps\lunac-plugin\public\lunac-plugin.json`（发版前 +1 这一份，别在 `build-plugins.ps1` 里再抄一份）。
- **发布仍是显式步骤，不会在构建时偷偷推**：一条命令是 `publish-plugins.ps1 -Build`（或 `build-plugins.ps1 -Publish`）。它只发每个 id 的最高版本，推完用 `gh api` 读回线上 `index.json` 与每个包的 size，逐条比对 id / version / sha256 / size，不一致就非 0 退出；本地内容没变时不制造空提交。**插件包可复现**：zip 条目按排序后的相对路径写入、时间戳钉死 2020-01-01，同样内容重建不会换 sha256。
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
- **持久化防护**：`commit.ps1` 在 `git add` 前归一 BOM；`build-plugins.ps1` / `build-release.ps1` / `publish-plugins.ps1` 在写操作前自愈守卫；`verify-git.ps1` 第 ⑧ 节**检测并就地归一**（幂等；2026-10-15 由「只报 FAIL」改——全对时零改动）。改完 `.ps1` / `.nsi` 一律跑 `npm run verify`。
- **「BOM 反复变多」的取证结论（2026-10-15）**：累加发生在**编辑 / 写文件时**（不是脚本运行时）——但**本仓工具链里没有任何写入者会加 BOM**：本环境编辑工具（`SearchReplace` / `Write`）实测每次写只会**剥掉一份** BOM（1→0 / 2→1 / 3→2），**从不累加**；累加源自**外部编辑器 / 文本替换工具**（「`ReadAllBytes` → 前置 `EF BB BF` 但不先剥原有 BOM」这一类写法，与 archive `ai-spec.md` §11 规则 82 同源）。⇒ `commit.ps1` 的自愈**只覆盖走它提交**的路径，直接 `git add` / `git commit`（agent 任务常见）会绕过它（历史确有 2 份 BOM 的提交 `7ee492c`）；这正是把归一**并入 `verify` 第 ⑧ 节**的原因。
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
- **结构不变量优先（2026-11-05）**：改二进制、索引、清单这类**有结构约束**的文件前，先把约束写成失败测试（升序、hash 唯一、条目数 == 头部计数、提供者唯一），跑出 RED 再动文件，最后 GREEN。只测「函数返回成功」不算数 —— 它测不到结构被破坏。
- RED（写失败测试，一次一个行为）→ GREEN（最小实现）→ REFACTOR（消除重复）。
- 禁止「水平切片」（先写完所有测试再写实现）；卡住时先回到 Phase 1 调查。
- **频繁提交**（每个任务完成后提交）；commit message 含中文时用 `git commit -F <UTF8 文件>`。
- **提交前必过**：`npx tsc --noEmit`、`cargo check`、`cargo test`（`core-agent` 与 `src-tauri` 两侧）。首次提交前用 `git diff --cached --name-only` 确认无 `.env`、无 MB 级文件。

## §测试 10 AI 侧无头端到端实测（`scripts\agent-e2e.ps1`）

- **定位**：AI 侧的**可脚本化实测**手段——直接拉 `core-agent` 的 `agent.exe`**走真实 API**，喂 query、捕获完整 `stream-json` 事件流、产出结构化结果并做「预期效果」断言。用于「多次单测 + 模块化混测」时反复验证改动是否达到预期。
- **目标产物 = release（纪律）**：脚本**默认瞄准 release 产物** `core-agent\target\release\agent.exe`，配套的 lunac 数据根也取 release 布局（`app\src-tauri\target\release`）。理由 = **交付目标 = 用户实际安装 / 运行的版本**，端到端实测必须对着它跑，而不是对着 dev 跑完再假设 release 一样。**dev 只是显式例外**：加 `-Dev` 才指回 `core-agent\target\debug\agent.exe`。
  - **开关名 `-Dev`（非 `-Debug`）的理由**：脚本带 `[CmdletBinding()]`，它自带的公共参数已占用 `-Debug`，再声明同名参数会抛「参数名 'Debug' 被多次定义」；`-Dev` 亦更贴合「指回 dev 产物」的语义。
  - **缺失即失败、绝不静默回落**：默认（release）产物不存在时，脚本打印可执行提示「**先构建 release 产物：`cargo build --release`（cwd=`core-agent`）**」并以 **exit 2** 失败；显式传了 `-Dev` 而 dev 产物不存在时同样报「先构建 dev 产物：`cargo build`（cwd=`core-agent`）」并 exit 2。**不会**从「缺 release」偷偷改用 debug（静默回落正是本条纪律要纠正的毛病）。
  - **宿主可执行文件（仅 `-McpExe` 接 MCP 桥时用）**：release 布局 = `app\src-tauri\target\release\lunac.exe`，dev = `app\src-tauri\target\debug\lunac.exe`，**安装版 = 安装根目录**（如 `$INSTDIR\lunac.exe`，由 `build-release.ps1` / `lunac-installer.nsi` 把 `lunac.exe` 与 `agent.exe` 平铺到同一目录）。`-McpExe` **无默认值**（默认不接桥）；要接就显式传对应布局的 `lunac.exe`。判据：release 下脚本**不会**去猜 dev 的 `lunac.exe`。脚本会把该路径**转成绝对路径**再交给 agent —— agent 是以**自身 cwd**（run 的 `workspace`）为基准 spawn 的，传相对路径会 spawn 失败（`os error 3`），工具只回 `bridge-unavailable`、看起来像「桥没实现」。
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

## §测试 12 Windows Defender 误报（dev，2026-10-20）

- **现象**：dev 版 `lunac.exe` 启动约 10s 后静默消失；日志没有 `PANIC`，事件日志没有
  `Application Error`，也没有 WER dump —— 看起来像「闪退」但不是崩溃。
- **取证**：Defender Operational 1116 / 1117 给出
  `Behavior:Win32/LummaStealer.CER!MTB`（ThreatID 2147935475，**行为判定**，不是文件签名），
  `Action = 删除`；`Get-MpThreatDetection` 的 `Resources` 直接指向
  `app\src-tauri\target\debug\lunac.exe` 与对应 PID。12:15 / 12:39 / 12:40 / 12:52
  四次「闪退」全部命中同一条检测。
- **触发面**（都是产品正常行为，组合起来像 stealer）：30 万条文件索引、WebView2 profile +
  `--remote-debugging-port=9222`、剪贴板历史、开机项查询、子进程 / 系统代理操作。
- **处理（2026-10-20 已加，需管理员）**：
  ```powershell
  Add-MpPreference -ExclusionPath '<repo>\app\src-tauri\target\debug\lunac.exe'
  Add-MpPreference -ExclusionProcess 'lunac.exe'
  ```
- **纪律**：这是 dev 未签名构建的误报。release 若也被判，走微软误报提交，
  **不要直接关闭实时保护**；排查「静默消失」时先查 Defender 1116/1117，再查 AppHang / dump。
- **验证注意**：用工具命令启动 GUI 后必须**在同一个命令内**观察（见 §测试 11.5），
  命令结束沙箱会回收子进程；判断是否仍被 Defender 终止，看 `Get-MpThreatDetection` 是否新增记录。

### §测试 12.1 触发点定位（2026-10-20，A/B 实测）

用同一份 `lunac.exe` 的副本（不在排除名单）逐项删权限，CDP 调 `open_plugin_window`
打开阅读插件，观察是否新增检测：

| 权限组合 | 结果 |
|---|---|
| 完整（window.float / net.fetch / storage.plugin / media.proxy / engine.eval） | 检测 |
| 去掉 `engine.eval` | 检测 |
| 再去掉 `net.fetch` | 检测 |
| 再去掉 `media.proxy`（仅 window.float + storage.plugin） | **不检测** |
| 只把 `media.proxy` 加回 | 检测 |
| `media.proxy` 仍开、但请求不带 Referer / User-Agent | 检测 |
| 打开音乐插件窗（不涉及 media.proxy） | 不检测 |

- **结论：底线是 `media.proxy`（宿主媒体代理）本身**，不是 `engine.eval`、
  不是 `net.fetch`、也不是伪造 Referer/UA；只要宿主进程扮演「任意 URL 的 HTTP 取回代理」，
  Defender 的 LummaStealer 行为模型就会命中。
- 触发链路：插件打开 → 渲染缓存卡片 → `<img src="lunac-media://localhost/...">` →
  宿主 `plugin_media` 用 reqwest 取任意远程图片 → 行为判定。
- **已落地对策（2026-10-20，方案 2：进程隔离）**：
  - 新增独立 crate `app/media-proxy`，产物 `lunac-media-proxy.exe`；它监听
    `127.0.0.1:<随机端口>`，只接受带 token 的 `GET|HEAD /fetch`，自己完成远程取回。
  - 宿主 `plugin_media::handle` 不再直接访问远程：只校验能力门 / URL，然后转发给 helper；
    宿主进程只与 127.0.0.1 通信。helper 随宿主 Job Object 退出，掉线时宿主自动重起并重试一次。
  - 构建：`npm run build` / `npm run tauri:dev` / `npm run tauri:build`（app）都会先编
    `media-proxy` release —— `tauri-build` 会校验 `bundle.resources` 里的 helper 必须存在，
    缺文件直接失败；`scripts/tauri-build.ps1` 的清理列表也包含平铺的
    `target\release\lunac-media-proxy.exe`（与 `agent.exe` 同口径）。
  - `.gitignore` 已忽略 `app/media-proxy/target/`（与 `core-agent` / `tuning-engine` 同口径）。
  - 验证：用**不在排除名单**的 `lunac-test.exe` + CDP 打开阅读插件，25s 后宿主存活、
    `media proxy helper started` 出现在日志、`Get-MpThreatDetection` 新增 0 条
    （宿主与 helper 都没被判定）。
  - **A/B 实验后必须核对清单还原**：实验会临时删 dev 副本
    `app\src-tauri\target\debug\Modules\<id>\lunac-plugin.json` 的权限；漏还原的症状是
    插件侧 `E_DENIED: 未声明 net.fetch / engine.eval 能力`、来源抓取与详情全失败
    （2026-10-21 实测踩过）。还原方式：把构建产物
     `resource-hub\apps\lunac-plugin\dist\lunac-plugin.json`（五权限齐全）覆盖回 dev 副本，
     再关掉插件窗重开；`index.js` 用 `Get-FileHash` 与 dist 比对即可。
     若 `index.js` 与 dist 不一致，多半是残留了实验版（封面 URL 指向
     `http://127.0.0.1:8124/fetch?url=`，表现为所有封面 / 章节图 `naturalWidth=0`），
     同样用 dist 覆盖；覆盖后确认 `Select-String 8124` 为 0。

## §测试 13 阅读插件真机验收（2026-10-21）

> 状态（2026-10-21）：番剧嗅探、漫画章节、CMS 采集源、HLS 播放、换源抽屉、手机竖屏
> 已在 Windows dev 真机验收通过；iOS 真机因环境限制暂缓（不做）。
> 另：聚合仓库是**导入时的快照**，发布新索引后必须在「来源 → 导入规则」重新导入一次，
> 已装来源不会自动多出条目；导入只是进「聚合仓库」，还需点「安装」才出现在已导入来源。

### §测试 13.1 番剧 WebView 嗅探

前置：`lunac.exe` 是包含 `plugin_sniff.rs` 最新改动的新构建；dev 副本
`Modules\resource-hub\lunac-plugin.json` 含 `"webview.sniff"`。

1. 重启 dev（旧进程仍加载旧插件 / 旧 exe），进入阅读插件，选一个番剧源搜索 →
   详情 → 点某一集。
2. 日志 `app\src-tauri\target\debug\temp\logs\lunac-<日期>.log` 应依次出现：

```text
INFO  plugin resource-hub webview sniff start https://<剧集页>
INFO  plugin resource-hub webview sniff 命中 https://<...>.m3u8
INFO  plugin resource-hub webview sniff 完成：N 条地址
```

3. 若只有 `start` 后跟 `webview sniff 超时（15000 ms）没有媒体地址`：

- 页面本身可能必须点击播放 / 过验证：先用普通浏览器打开同一剧集页确认能否出流；
- 站点若走 MSE + `blob:`，拦截到的会是分片而不是 m3u8，需另加规则；
- `webview sniff 挂载失败：…` 表示 WebView2 COM 调用失败（看 `HRESULT`）。

4. 回到界面：播放器应出现在 `player` 视图；若报 `unsupported`，说明该来源仍是
   `capabilities: ["search","chapters"]` 的旧记录且运行时没补上 `sniff`
   （见 `runtime.sourceFor` 的 `needsSniff`）。

### §测试 13.2 PC 滚轮缩放

1. 打开任意漫画章节，确认阅读模式为「左右分页」。
2. 指针停在某一角向内滚 → 以指针为锚点放大（上限 4×）；向外滚 → 缩小，
   回到 1× 后继续向外滚应恢复容器滚动（不再是缩放）。
3. 双击仍可 2× / 还原；缩放后拖动平移，边界由 `clampZoomPan` 约束。
4. 触控板 pinch（浏览器表现为 `ctrlKey + wheel`）与滚轮同路径。

### §测试 13.3 分类 / 标签 / 筛选抽屉（2026-10-21）

1. 总览点「视频」或「番剧」分类：应触发一次按分类的来源加载；
   番剧源用「空关键词搜索」作为浏览入口，能出卡片就说明兜底生效。
2. 若某源仍为空：看该源详情页在普通浏览器里空关键词搜索是否有结果；
   没有的话该站不支持浏览，只能搜索（属上游限制）。
3. 左下角悬浮按钮应显示「全部来源」；打开后是小抽屉（不再是从左侧顶满全高），
   头部显示「当前来源」，标签页按来源分组，每个标题下只列该源的标签。
4. 在抽屉里勾选一个来源 → 确定：网格只剩该源的卡片，按钮文案变成该源名；
   选一个没有标签的来源 → 标签页显示「该源暂无可筛选的标签」。
5. 搜索栏右侧不应再有来源下拉；搜索 / 输入建议都应遵守当前分类与来源筛选。
6. 日志中不应再出现 `不允许设置请求头：Connection`；若某源仍失败，
   应给出明确的 HTTP / DNS / 超时原因。

### §测试 13.4 标签 pill / 来源搜索与折叠（2026-10-21）

1. 在筛选抽屉的「标签」页选中「百合」→ 确定：左下角筛选按钮右侧出现「百合」pill，
   右侧带 ×；点 × 后 pill 消失，网格立即恢复未按该标签过滤。
2. 来源管理页顶部有搜索框：输入来源名 / id / 域名片段，列表即时过滤；
   清空后恢复全部。
3. 「聚合仓库」「已导入」两个分区默认是收起状态（标题 + 条数）；
   点标题展开，再点收起；搜索词不会因折叠而丢失。
4. uzVideo：导入发布后的索引后，聚合仓库应出现 `video.*` 条目（共 45 条，
   文件名即来源名）；安装后可在搜索 / 总览里使用。

### §测试 13.5 CMS 采集源（2026-10-21）

1. 导入发布后的 `index.json`，聚合仓库里应出现 `cms.*` 6 条（百度 / 暴风 / 虎牙 /
   量子 / 非凡 / 火狐），来源详情显示「自建声明式 CMS 规则」。
2. 安装其中任意一条，在总览「视频」分类下应能出「近期更新」卡片（走
   `?ac=detail&pg=1`）；搜索中文片名能返回结果（走 `?ac=detail&wd=`）。
3. 打开详情 → 剧集：应列出线路与集数（`vod_play_from` / `vod_play_url` 拆分），
   点击第一集：
   - 百度 / 暴风 / 虎牙：直接播放（日志无 `webview sniff`）；
   - 量子 / 非凡 / 火狐：若该集是播放页，日志出现 `webview sniff start` 后再播放。
4. 采集站关闭搜索时（如 `suoniapi.com`）：搜索应报「该采集站未开放搜索」，
   不应出现 `invalid type` / JSON 解析错误。
5. 接口失效（域名过期 / 返回 403）：搜索该源只报自身错误，不影响其它来源。

### §测试 13.6 手机竖屏 / 横滑翻页（2026-10-21）

前置：浏览器 DevTools 设备模拟选 iPhone（375×812），刷新阅读器。

1. 上下框：所有动作按钮只显示图标、同一行排布、高度 ≥44px；
   速度显示 `1×`、模式显示 `左右分页`；无横向滚动条。
2. 左滑 / 右滑（左右分页模式）：页码前进 / 后退；竖向与条漫模式下横滑不翻页。
3. 右侧 1/3 单击仍翻页；双击仍 2× 放大；放大后单指拖动为平移。
4. 横屏（812×375）：上下框保持紧凑单行，正文区域不被压扁，图片不超出可视区。
5. 断开触屏模拟（桌面 1280×800）：文字标签恢复、按钮 40px，布局与改动前一致。
6. iOS 真机（如可用）：长按正文不弹系统菜单；横竖屏切换后文案立即更新。

### §测试 13.7 HLS 播放与换源收敛（2026-10-21）

1. 任选一个 CMS 采集源（暴风最稳）→ 详情 → 第一集：
   - `<video>` 的 `currentSrc` 必须是 `blob:`（走 MSE / hls.js）；若是
     `http://lunac-media.localhost/...m3u8`，说明又走回了原生分支（见设计 §6.4）。
   - 采集站 CDN 慢时清单可能要 10–16s，30s 内应开始播放；播放中
     `readyState` 到 4、`currentTime` 递增。
2. 换源抽屉：点「换源」后应在 15s 内收敛（有结果就逐条出现，底部保留
   「正在搜索其他来源…」进度行）；到点后即使仍有来源在跑也收尾并缓存结果。
   - 收敛后点「重试」应重新开一轮（旧一轮被 abort），不会一直转圈。
3. 关闭抽屉再打开：10 分钟内走缓存，立即出结果；不应再看到全量 110 个来源的
   `source xxx search` 日志（只应出现同类型来源）。
4. 已知环境问题：`b3.bdzybf22.com`（百度采集新 CDN）对宿主代理回 403
   `The region has been denied`，该 CDN 的剧集播不了；换同源旧 CDN
   （`vod6.bdzybf11.com`）或别的采集源验证。

### §测试 13.8 搜索 / 总览真分页（2026-10-21）

前置：安装至少一个有多页结果的来源（暴风采集最稳），并在筛选里只选它。

1. 搜「爱」：首屏 20 张、底部出现「继续下滑加载更多」。
2. 点哨兵（或滚到底）：出现「正在加载下一页」→ 卡片 20 → 40；
   再点一次 → 60（第 3 页）。三页都应真正发出 `pg=2` / `pg=3` 请求。
3. 搜「庆余年」（暴风只有 10 条）：第 2 页返回空后哨兵消失，不应留着可点的
   「加载更多」，也不应重复请求同一页。
4. 总览（全部来源）：首段 30 张；反复点哨兵，先本地扩窗（每段 30 张），
   段用完后联网追加并继续增加卡片；过程中哨兵保持可见直到所有来源都没有下一页。
5. 关闭插件窗重开：总览缓存仍能直接出卡片；继续翻页时对旧缓存来源
   （无 `hasMore` 字段）会先试一次下一页，失败 / 空页后自动收敛。

### §测试 13.9 拼音检索（2026-10-22）

前置：dev 实例 + 已同步的 `Modules\resource-hub`；宿主日志里 `pinyin_encode` 不应报错。

1. 搜索框输入 `dldl`：首帧可能只有子串结果，**无需再敲键**，随后出现《斗罗大陆》
   的建议项与网格卡片（`warmPinyin` 编码回来后自动重绘）。
2. 输入 `dou luo` / `douluodalu`：同样命中；`斗罗`（汉字）仍按子串命中。
3. 排序：同时命中子串与拼音时，**子串结果排在拼音结果前面**；都不命中的条目不出现。
4. 远程来源：拼音只匹配**当前已返回 / 已缓存**的远程标题，不会因为拼音多搜一轮；
   站点首屏没有的结果不会凭空出现（属上游限制）。
5. 降级：在旧宿主（无 `__lunac_host.pinyin`）里打开，拼音输入不再命中，
   但子串搜索与其它功能一切照旧，控制台无未捕获异常。
6. 分层自检：`cargo test --bins pinyin`（9 条）与
   `npm run test -w @resource-hub/source-sdk`（含 `pinyin.test.ts` 5 条）应为绿。

### §测试 14 任务抽屉三页签 / 回退回填 / 改动文件可读（2026-10-22）

前置：dev 实例（`cd app ; npm run tauri:dev`），聊天独立窗里跑一轮「`TodoWrite` + 写两个文件」的对话。

1. **三页互斥**：抽屉左边三个页签「任务 / 文件 / 后台」，右侧显示当前页数量
   （`N 个任务` / `N 个文件已改动` / `N 个后台命令`）；任意时刻**只有一页**可见；
   头部的「待办清单」标题与 `N/M 已完成` 进度文案**不存在**。
2. **待办页**：每条左侧有状态方块（未完成空心 / 进行中虚线 / 已完成实心）；
   「确认」按钮在**待办页右下角**（不是头部）。
3. **后台页**：AI 起一条后台命令 ⇒ 「后台」页出现 `N 个后台命令` 与「取消」；
   点当前为 0 的页签**不会被自动弹回**别的页。
4. **回退 = 删历史 + 回填**：对一条用户消息点「回退」并二次确认 ⇒ 该消息及其后全部从对话消失，
   它的正文（含附件 chip）出现在输入框且光标在末尾；对助手消息回退 ⇒ 回答消失、对应的
   用户提问回到输入框。`/rewind` 同一语义。
5. **改动文件可读**：展开文件行的 `▸` 应渲染出 diff（新建文件 = 全为 `+`），**不是**
   「无法预览」；点文件名能在资源管理器里定位。宿主新命令 `get_agent_workdir` 返回非空绝对路径
   （相对 `file_path` 靠它归一）。
6. **页签再点是回默认态**：点一次当前高亮的页签 ⇒ 进入「**没有选中任何分页**」的默认态：
   抽屉**仍然显示**（左侧三个页签栏保留、右侧数量清空），三页内容全部收起、列表数据不清空；
   点任意页签立即重新展开。之后即使又来新待办 / 新改动文件，也不会自动展开
   （尊重用户的手动选择；只有抽屉从隐藏变为出现时才会自动落到有内容的页）。
7. **回退不进历史**：任一形式回退（用户消息 / 助手消息 / `/rewind`）后打开「历史记录」，
   被回退的那段**不得**出现在该会话里；若整段被回退光（一条用户消息都不剩），
   `chat.db` 里那条会话**整条消失**（不是留一条只有标题的空壳），
   它对应的 `ModuleData\history\frames\<sid>.json` 也一并被清掉。

### §测试 15 桌宠语音输入（2026-10-22）

前置：dev 实例 + `Modules\pet` 已是 0.9.65（`build-plugins.ps1 -Plugin pet`），系统麦克风可用。

1. **入口**：打开桌宠对话输入行 ⇒ 文本框右侧、中止按钮左侧出现**麦克风按钮**（30×30、
   描边 / 底色走四类变量）；默认可见、hover 有反馈。
2. **成功路径**：点一下 ⇒ 按钮进入识别中态并禁用；说一句 ⇒ 自动通过 `pet-chat-send`
   发起一轮（与打字发送同一条链路，不出现第二种消息格式）；答复按既有流程显示 / 朗读。
3. **没听清**：点完不说话 ≥ 6s ⇒ 提示「没听清，再说一次？」；按钮恢复可点，
   **不产生**空消息、不发起空轮次。
4. **麦克风不可用**：Windows 隐私设置里禁用麦克风后重试 ⇒ 提示「麦克风不可用
   （检查 Windows 隐私设置）」；不是笼统的「失败」。
5. **没语言包**：系统未安装语音识别语言包时 ⇒ 提示「系统未安装语音识别语言包」。
6. **四类全黑**：风格里四类颜色全设 `#000000` ⇒ 该按钮底色 / 描边 / 图标随之变黑（无白名单色）。
7. **不阻塞窗口**：识别期间（最长 ~20s）拖窗 / 点其它按钮仍响应（识别在宿主阻塞线程里）。

### §测试 16 任务级预算（S2 切片，2026-10-22）

口径：`LUNAC_TASK_BUDGET_TOKENS`（默认 `0` = 关闭）；累计 = 主请求的 `in + read + create + out`，
**跨用户提问**累计；撞顶后不再续命，但**不硬拒绝新提问**。单测 3 条
（`task_budget_accumulates_across_turns_and_is_off_by_default` /
`task_budget_env_parsing_falls_back_to_off` /
`task_budget_over_disables_extensions_even_when_compressed`）。

前置：`cargo build --release`（cwd=`core-agent`）；env 由**调用脚本的 shell**注入即可
（脚本不覆盖该键，子进程继承）。

1. **默认关**：不设 `LUNAC_TASK_BUDGET_TOKENS` ⇒ 任意多轮问答的 `agent-*.log` 里
   不得出现「任务级预算」字样、`stream.jsonl` 不得出现 `budget_exhausted`。
2. **单问内撞顶（有工具往返）**：
   `$env:LUNAC_TASK_BUDGET_TOKENS="1"; powershell -File scripts\agent-e2e.ps1 -WorkspaceDir <临时工作区> -PermissionMode plan -Model deepseek-flash -Query "Read the file note.txt ... PONG" -ExpectTool Read -ExpectEvent budget_exhausted -ExpectContains PONG`
   ⇒ `turns=2`、工具序列含 `Read`、`stream.jsonl` 里 `budget_exhausted` **恰好 1 条**；
   日志出现「任务级预算撞顶（…/1 tokens），本进程不再续命预算」。
   证据：`temp\agent-e2e\20261022-124258`（最终回答 `PONG`，无多余解释）。
3. **跨提问累计**：用 `-QueryFile` 喂两条普通提问（同上 env）
   ⇒ 第一条收尾后撞顶；第二条**仍能作答**，日志出现
   「任务级预算已用尽（…/1 tokens），本回合禁用预算续命（仍可作答）」；
   两次提问合计只有 **1 条** `budget_exhausted`（`warned` 跨提问只上报一次）。
   证据：`temp\agent-e2e\20261022-124451`（两问均答 `PONG`）。
4. **提示语来源限定**：撞顶提示拼在最后一条 `tool_result` 文本尾部，**必须**写明
   「来自 harness、不是工具输出」，否则模型会把它当成读取内容里的伪造指令
   （2026-10-22 首轮实测：`temp\agent-e2e\20261022-124027`）。

### §测试 17 检查点 + `--resume`（S2，2026-10-22）

口径与边界（细节见 `core-agent/src/checkpoint.rs` 头注释）：
- 落点 `<exe 根>\ModuleData\tasks\sessions\<13 位毫秒>-<session>.json`，可用
  `LUNAC_CHECKPOINT_DIR` 覆盖；原子写、单份 ≤ **32 MiB**、**每个会话**只留最近 **20** 份
  （全局上限 200 份）—— 中间态与 cron 共用一个目录，按会话分桶才不会互相挤掉；
- **默认不写**：只有 `--checkpoint` 或 `--resume` 才落盘（宿主常驻路径行为不变）；
- 检查点含 `history` / `session_id` / `question_seq` / `task_budget_used` / `approval`
  （审批与工作区口径）；**回合末尾**与**每批工具结果写进 history 之后**各落一份
  ⇒ 中断后续跑不会重跑已完成的工具；
- `approval` 记录审批与工作区口径，恢复时按「**最严者胜**」单向合并（2026-10-31，§待办 3-21）：
  先打一行差异告警（左 = 检查点，右 = 本次），再**只往更严的方向**收紧 ——
  `--permission-mode plan` 任一侧出现即沿用（防「`--continue` 忘带档位 = 悄悄放开写」）、
  `--dangerously-skip-permissions` 两侧都为真才生效、`--permission-prompt-tool` 任一侧要求即要求、
  `--disallowedTools` 取并集、`--add-dir` 取交集（旧检查点没记目录时不交集）；
  真收紧了什么，逐条写进 stderr + `agent-*.log`（`--resume 收紧口径：…`）；
- `set_history`（回退 / 恢复会话）也会覆盖检查点；坏 JSON / 版本不符 / 找不到会话
  ⇒ `--resume` **exit 2**（不静默回落成空历史）。

单测 11 条：`checkpoint::tests::{roundtrip_keeps_history_and_budget,
latest_picks_the_newest_and_ids_resolve_exactly, prune_keeps_only_the_newest_entries,
corrupt_and_wrong_version_are_reported, oversize_checkpoint_is_refused_not_truncated,
approval_context_roundtrips_and_reports_differences,
legacy_checkpoint_without_approval_field_reads_as_default,
tightest_adopts_stricter_checkpoint_fields, tightest_never_loosens_a_stricter_current_run,
tightest_keeps_current_dirs_when_the_checkpoint_recorded_none}` +
`cli_checkpoint_and_resume_parse`。

端到端（真实模型，release；凭据沿用 `app\src-tauri\.env` 的 `AI_API_URL` / `AI_API_KEY`）：

1. 设 `LUNAC_CHECKPOINT_DIR=<临时目录>\sessions`、`LUNAC_LOG_DIR=<临时目录>\logs`、
   `LUNAC_AGENT_BASE_URL=<AI_API_URL 去掉 /v1 后>/anthropic`、`LUNAC_AGENT_TOKEN=<AI_API_KEY>`、
   `LUNAC_AGENT_MODEL=<AI_MODEL 或 deepseek-flash>`、`LUNAC_TASK_BUDGET_TOKENS=1000`。
2. **A 进程（写入）**：
   `agent.exe --print "Remember the exact token BANANA-42. Reply with just OK." --checkpoint --output-format text`
   ⇒ 退出码 0、输出 `OK`；日志出现
   「检查点写入：…（第 1 问 / history 2 条 / 任务预算累计 6866）」。
3. **B 进程（恢复）**：
   `agent.exe --print "What exact token did I ask you to remember? Reply with just the token." --resume latest --output-format text`
   ⇒ 退出码 0、输出 **`BANANA-42`**（证明 history 真的接上了）；日志出现
   「从检查点恢复：…（会话 sess_… / 第 1 问 / history 2 条）」、启动横幅的会话 id 与 A 相同、
   「任务级预算撞顶（6866/1000 tokens）」（证明预算累计跨进程接上）、
   收尾再写一份「第 2 问 / history 4 条 / 任务预算累计 13758」。
4. **失败路径**：`--resume sess_missing` ⇒ exit 2，日志写「找不到会话 … 的检查点」。
5. **默认不写**：不带 `--checkpoint` / `--resume` 的常驻 / 单发跑一轮 ⇒ 检查点目录为空。
6. **`--continue`（跨回合续跑的最小闭环）**：单发下隐含 `--resume latest` + `--checkpoint`；
   **不写 query** 时用内置 `CONTINUE_PROMPT`（「接着做、别重复已完成步骤」）作为本轮提问，
   **不读 stdin**；常驻模式传它只记一条 warn、不生效。
   实测（`temp\agent-e2e\20261022-continue`）：
   - A：`--print "用 TodoWrite 建一条 content=CONTINUE-MARKER-77 / status=pending 的任务 … 只回 SET" --checkpoint`
     ⇒ 输出 `SET`，检查点 q1/history 4；
   - B：`--print --continue`（**完全不写任务**）⇒ 输出
     「…the list holds exactly one task — `CONTINUE-MARKER-77`, status pending…」，
     证明 history 跨进程接上且用户无需重述；收尾写 q2/history 6。
   - 没有检查点时 `--continue` 与 `--resume latest` 一样 exit 2。
7. **中间态（2026-10-22，工具结果后立即落盘）**：夹具目录里放 `note.txt`（内容
   `MID-CPK-91`），A 进程：
   `agent.exe --print "Read note.txt with the Read tool and reply with only the code inside it."
   --checkpoint --permission-mode acceptEdits --add-dir <ws>` ⇒ 输出 `MID-CPK-91`、exit 0；
   检查点目录出现**两份**同 `question_seq` 的文件：
   - 前一份 `history 3`、最后一条是 `user(tool_result)`、`tool_use` / `tool_result` 配对完整
     （= 一批工具刚回灌就落盘）；
   - 后一份 `history 4`、最后一条是 `assistant(text)`（= 回合收尾）。
   B 进程（**不传** `--permission-mode` / `--add-dir`）：
   `agent.exe --print --continue --output-format text` ⇒ exit 0，回答
   「Already done — … `MID-CPK-91` … Nothing left to run」，且新检查点里
   **`tool_use` 仍是 1 条**（没有重跑已完成的 Read）；stderr 同时出现
   「审批 / 工作区口径与检查点不同（左 = 检查点，右 = 本次；只收紧不放宽）：
   permission-mode acceptEdits → (default)；add-dir […] → []」——本次与检查点都属可写档，
   目录交集 = 本次的空集 ⇒ **不触发收紧**（行为与改造前一致）。
8. **「最严者胜」收紧（2026-10-31，不需要真实模型）**：手写一份检查点
   （`version=1`、`approval.permission_mode="plan"`、`approval.disallowed=["Write"]`，
   **UTF-8 无 BOM** —— PowerShell 5.1 的 `Set-Content -Encoding utf8` 会加 BOM，
   `serde_json` 直接判「解析检查点失败」），再跑
   `agent.exe --print "hi" --permission-mode acceptEdits --disallowedTools Edit --resume <id>`
   ⇒ ① 启动横幅的会话 id 与检查点一致（确实续上）；
   ② stderr 两行：差异告警（`permission-mode plan → acceptEdits`）+ `--resume 已按「最严者胜」
   收紧本次口径：permission-mode 沿用检查点的 plan（只读）；disallowedTools 并入 [Write]`；
   ③ 同一行的工具池里 `Write` / `Edit` **都消失**（只读档 + 禁用并集）。
   没有 API 凭据时进程随后以 `LUNAC_AGENT_BASE_URL is not set` 退出 1 —— 这两行在它之前，取证成立。

证据：`temp\agent-e2e\20261022-checkpoint`（checkpoint/resume）、
`temp\agent-e2e\20261022-continue`（`--continue`，含两问日志）与
`temp\agent-e2e\20261022-midcp`（中间态：两份同问检查点 + 恢复不重跑工具）。

### §测试 18 长任务循环（`scripts\agent-task-loop.ps1`，2026-10-22）

**退出码契约**（`core-agent/src/main.rs` 的 `single_shot_exit_code`，单测同名）：

| 码 | 含义 |
|---|---|
| 0 | 正常结束，且 `TodoWrite` 清单没有 `pending` / `in_progress` |
| 1 | 本轮失败（`is_error` / 审批被拒 / 网络等） |
| 3 | **正常结束，但清单未完成**；**只有带检查点时才会出现**（不带检查点的单发仍是 0/1，既有调用方不变） |

脚本行为：第 1 轮 `--print "<任务>" --checkpoint`，之后每轮 `--print --continue`；
退出码为 3 且未到 `-MaxRounds` 就继续，0 / 其它码 / 到上限即停。
`-WorkspaceDir` 同时作为 agent 的 cwd 与 `--add-dir`；默认 `-PermissionMode plan`（只读），
写类任务需显式 `-SkipPermissions`（单发没有审批回包通道，风险见 `forbidden.md` §禁用 1.2）。

验收（真实模型，release）：

1. **退出码 3**：`agent.exe --print "用 TodoWrite 建一条 pending 任务 PENDING-MARK-91 … 只回 SET" --checkpoint --output-format text`
   ⇒ 输出 `SET`、**exit 3**，日志「单发退出码 3：TodoWrite 清单未完成（可用 --continue 续跑）」。
   证据：`temp\agent-e2e\20261022-exit3`。
2. **循环跑到完成**：
   `powershell -File scripts\agent-task-loop.ps1 -Query "…建一条 pending 任务：读取 note.txt 并报告第一行…不要现在读，只回 SET" -WorkspaceDir <含 note.txt 的目录> -MaxRounds 3 -Model deepseek-flash`
   ⇒ 第 1 轮 `exit=3`；第 2 轮 `--continue` 读出 `FIRST-LINE-MARK-52` 并 **exit=0**；
   脚本输出「完成（2 轮）」。
   证据：`temp\agent-e2e\20261022-task-loop`（`logs\` + `sessions\` + `ws\note.txt`）。
3. **未完成时停在上限**：把任务写成「只建 pending、不做」⇒ 每轮 exit 3，到 `-MaxRounds`
   后脚本黄字提示「仍未完成；检查点保留在 …，可再跑 --continue」，退出码仍为 3。
4. **脚本卫生**：`scripts\agent-task-loop.ps1` 必须**恰一份 UTF-8 BOM**（`npm run verify` §8 会查）。

### §测试 19 定时任务的会话延续（2026-10-22）

口径（`app/src-tauri/src/cron.rs`）：
- cron 单发**固定带 `--checkpoint`**；job 运行态里有 `lastSessionId` 且对应检查点仍在时，
  追加 `--resume <id>` ⇒ 每次 cron 不再从零开始；
- 检查点目录与 agent 同一推导（`LUNAC_CHECKPOINT_DIR` 优先，否则
  `<exe 根>\ModuleData\tasks\sessions`）；文件被份数上限清理时记一条 info 并开新会话；
- 带 `--resume` 仍失败时清掉 `lastSessionId`，避免坏检查点让 job 永久失败；
- 只读边界不变：仍固定 `--permission-mode plan` + 写类 / 外部 / 子代理 / Cron* 的
  `--disallowedTools`；`--resume` 必须排在 variadic 之前。

单测（已跑）：`cargo test --bins cron` **17 passed**，其中新增
`resume_args_carry_the_session_before_the_variadic_flag` /
`resume_target_ignores_empty_or_missing_state`，并给
`run_args_pin_the_read_only_single_shot_contract` 加了「必须带 `--checkpoint`、首跑不带 `--resume`」断言。

真机步骤（**待 dev 实例**；cron 最小间隔 15 分钟，可连点两次「立即运行」代替等待）：

1. 建一条 job：prompt「读取 note.txt 并只回复第一行」（工作区放 `note.txt`，内容 `CRON-MARK-31`）。
2. 第一次运行后 `ModuleData\tasks\cron-state.json` 的该 job 出现非空 `lastSessionId`；
   `temp\logs\cron-<id>-<ts>.log` 的 `cron run` 参数行含 `--checkpoint`、**不含** `--resume`。
3. 第二次运行（或点「立即运行」）：参数行含 `--resume <同一个 id>`；回答能引用上一轮读到的内容；
   检查点 history 条数比第一轮多（证明接上了前文）。
4. 删掉该会话的检查点文件后再运行：日志出现「检查点已不在…本轮开新会话」，
   参数行回到不带 `--resume`，运行仍 exit 0。
5. 把检查点改成坏 JSON 再运行：本轮失败；再运行一次应自动开新会话（`lastSessionId` 已被清掉）。
6. 全程确认写类工具仍被裁掉（日志 / stdout 里不出现 `Write` / `Cmd` 的成功调用）。

> 状态（2026-10-23 真机验收通过）：1–6 全部在 dev 实例跑过（`cargo run` 的
> `target\debug\lunac.exe`，job `job_mk31`，工作区 `temp\transStorage`）：
> ① 首跑 `lastSessionId=sess_88384_1792728409649`，参数行含 `--checkpoint`、不含 `--resume`，
> 摘要 `CRON-MARK-31`；
> ② 把 note.txt 改成 `CRON-MARK-99` 后再跑，参数行出现 `--resume sess_88384_1792728409649`
> （在 `--disallowedTools` 之前），摘要仍是 `CRON-MARK-31` ⇒ 接上了前文；检查点 1235 → 1456 字节；
> ③ 删掉该会话的检查点后重跑：日志「检查点已不在（被清理 / 换了目录），本轮开新会话」，
> 参数行回到无 `--resume`，新会话 id `sess_86024_…`，仍 ok；
> ④ 写入坏 JSON 检查点后重跑：本轮失败（`解析检查点失败 … key must be a string`），
> `lastSessionId` 被清掉；再跑一次自动开新会话 `sess_71340_…`、ok；
> ⑤ 全程 stderr 的工具清单只有只读工具（无 `Write` / `Cmd`）。
> 已知小瑕疵（非本轮引入）：`lastError` 取的是 stderr 最后一行，成功轮也会记下
> `[agent] P1–P4 就绪…` 横幅（不影响 `lastStatus`）。

### §测试 20 通用任务队列（2026-10-22）

口径（`app/src-tauri/src/task_queue.rs` + `core-agent/src/tools.rs` 的 Task* 段）：
- 数据：`<exe 根>\ModuleData\tasks\queue.json`（`{version, items}`）与 `queue.lock`；
  agent 写「入队 / 取消」，宿主 worker 写 `running / done / failed / round / sessionId`；
- 工具：`TaskEnqueue`（**要审批卡**；单发 / `--dangerously-skip-permissions` 直接拒绝）、
  `TaskList`（只读）、`TaskCancel`（只取消 `pending`，幂等）；
- 执行：宿主 3s tick、FIFO、固定只读档（`--permission-mode plan` + `cron::READ_ONLY_DENY`）、
  与 cron 共用 `RunGuard`；单任务最多 `maxRounds`（默认 5 / 上限 20）轮：
  第 1 轮把 prompt 写进 stdin，之后 `--continue --resume <sessionId>`；
  退出码 3（TodoWrite 未完成）且还有轮次就继续，每轮 10 分钟墙钟；
- 宿主退出时仍在 `running` 的条目在下次启动记 `failed`（不悄悄重跑）。

单测（已跑）：
- core-agent：`task_ids_and_list_view_keep_the_queue_contract`（id 前缀 / 预览截断 /
  完成态过滤 / 审批档位）、`task_queue_read_normalizes_and_refuses_corruption`（缺失 / 损坏 / 裸数组）；
- host：`pick_pending_takes_the_first_pending_in_order`、
  `round_action_continues_only_on_exit_three_with_rounds_left`、
  `round_args_are_read_only_and_resume_after_checkpoint`。

端到端（真实模型，release，`LUNAC_TASKS_DIR=<临时目录>`）：
1. **TaskList 读侧**：`queue.json` 放 1 条 pending + 1 条 done，
   `agent.exe --print "Call the TaskList tool with includeFinished=true. Then reply with exactly
   one line: COUNT=<count> PENDING=<pending>." --output-format text --permission-mode plan`
   ⇒ 输出 `COUNT=2 PENDING=1`、exit 0。证据：`temp\agent-e2e\20261022-taskqueue`。
2. **TaskEnqueue 硬闸**：同环境单发调用 `TaskEnqueue`
   ⇒ 工具返回「requires an interactive approval card…」，模型回 `DENIED`，
   `queue.json` 条目数不变（2）。证据同上。

真机步骤（**待 dev 实例**）：
1. 交互会话里让模型 `TaskEnqueue`（prompt 自包含）⇒ 出现审批卡；允许后 `ModuleData\tasks\queue.json`
   出现 `pending` 条目，`TaskList` 能看到 `queuePosition`。
2. worker 在 3s 内把它置 `running`；`TaskList` 显示 `round` / `sessionId` / `summary` 递增；
   完成后 `done`（或 `failed` + `error`）；事件 `task-queue-changed` 同步到前端（如有监听）。
3. 入队两条：严格 FIFO，一条完成后再开始下一条；期间 cron 到点会被 `skipped_busy`（共用单飞闸）。
4. 对 `pending` 条目 `TaskCancel` ⇒ `cancelled`；对 `running` 条目应报「cannot be cancelled」。
5. 任务运行中直接关宿主：重开后该条目为 `failed`，`error` 写明 host stopped。

> 状态（2026-10-23 真机验收通过）：
> ① **交互入队**：Lunac 窗口里对模型说「用 TaskEnqueue 入队…」⇒ 审批卡出现、允许后
> `queue.json` 新增 `pending` 条目（`id=task_c9fe3fb2`、`cwd=D:\新建文件夹 (2)`、
> `maxRounds=3`），`TaskList` 能看到队位；
> ② worker 3s 内置 `running` → `done`，`summary` = `QUEUE-UI-1`，`sessionId` 落盘；
> ③ FIFO：两条 pending 严格按入队顺序（前一条 `finishedAt` 后 2ms 才置下一条 `startedAt`）；
> 同在跑时 cron 到点记 `skipped_busy`（共用单飞闸）；
> ④ `TaskCancel`：pending ⇒ `{"cancelled":true,"status":"cancelled"}`；running ⇒
> `task … is running; it cannot be cancelled from here`；任务跑到 `maxRounds` ⇒ `failed` +
> `max rounds reached (20) with unfinished todos`（20 轮共用同一 `sessionId`）；
> ⑤ 跑一半直接杀宿主：重开后该条目 `failed` + `error = host stopped while running; re-enqueue if needed`，
> 日志「任务队列：1 条上次未跑完的任务已标记 failed」。
> **真机发现并修掉的缺陷**：宿主 `effective_workdir()` 给的是 verbatim 路径
> （`\\?\D:\新建文件夹 (2)`），agent 侧 `TaskEnqueue` 记的是 `D:\新建文件夹 (2)` ⇒
> `same_path()` 判成「工作区已变」、任务一入队就 failed。修复 = `normalize_path()`
> 去掉 `\\?\` / `\\?\UNC\` 前缀、统一分隔符与大小写，并加单测
> `same_path_ignores_verbatim_prefix_and_separators`（host 321 → 322 passed）。

---

## §测试 21 资源库一次性验收地图（2026-10-29）

> **用途**：`docs/resource-hub-design.md` §13.5 里剩下的「未做 / 真机验收项」已全部收敛到这一条
> 路线，按 21.0 → 21.13 一次走完即可。**全部在 Windows dev 实例上跑**，release / 安装版留到
> 出包后按需复验（§测试 11.1：release 仍是最终裁判）。
> 每条都给「操作 → 期望 → 证据」；不合格就把那一行结论回填 §13.5，不要口头记。
> 判定口径：**UI 现象 + 日志行** 两者对齐才算过；只有 UI 看着对但日志没有对应行，按未过处理。

### §测试 21.0 前置（只做一次）

1. **关掉正在跑的 dev 实例**（`Get-Process lunac`）——否则 `cargo` 拷 `lunac.exe` 会撞
   `os error 32`（另一个程序正在使用此文件）。
2. `cd app; npm run tauri:dev`。这条会先 `npm run dev:media-proxy`
   （`cargo build --release` 出 `lunac-media-proxy.exe`）再起 dev。
   核对 `app\media-proxy\target\release\lunac-media-proxy.exe` 的时间戳是本次构建。
3. **声明数据根**（§测试 11.4 硬纪律）：本轮数据根 = `app\src-tauri\target\debug\`。
   插件目录 `Modules\resource-hub\`、`ModuleData\`、`temp\logs\` 都在它下面。
4. 插件产物核对：`Modules\resource-hub\index.js` 为 **1,284,354 B**（2026-10-30，含加密图源封面修复；
   这个数字随每次 `build-plugins.ps1` 变，**以盘上产物为准**，2026-10-29 那轮是 1,273,066 B）；
   `lunac-plugin.json` 的 `permissions` 为
   `window.float / net.fetch / storage.plugin / media.proxy / engine.eval / webview.sniff`。
5. 日志窗口常开：`app\src-tauri\target\debug\temp\logs\lunac-<日期>.log`。

### §测试 21.1 插件框架：iframe 引擎 + 两个常驻锚点

> **状态（2026-10-29，用户真机确认通过）**：修掉 `iframe.src` 的 `lunac-plugin://` 写法
> （见本节末尾）后重跑，①–⑥ 全部通过。

前置：`powershell -ExecutionPolicy Bypass -File scripts\install-probe-plugin.ps1`
（默认装到 `app\src-tauri\target\debug`；`-Root D:\Lunac` 指安装根）。

| # | 操作 | 期望（判据） |
|---|---|---|
| ① | 设置 → 插件 → 重新扫描 | 列表出现「信道探针」 |
| ② | 打开它的面板 | 窗口里出现 `hello` 与 `→ ready` 两行 |
| ③ | 打开聊天区 / 设置面板 | 聊天侧栏一块显示 `mount：slot=chat-sidebar`；设置里一块显示 `slot=settings-section` |
| ④ | 主窗切换主题 | 探针面板出现 `theme.changed`，且 `--text` 有值（不是空串） |
| ⑤ | 关闭面板 | 出现 `lifecycle：dispose` |
| ⑥ | 卸载 / 重扫后 | 聊天侧栏与设置分组**不残留**空位（无插件不占位） |

失败时读握手自述：`ChannelHost` 会分三档报「文档没触发 load / origin 被拒（含实际 origin）/
文档已 load 但零消息（脚本没跑或被 CSP 拦）」，日志与面板都会写。

> **2026-10-29 首次真机跑这条就红了，已修**：三份 iframe（主窗内嵌 / 插件窗 / 两个锚点）把
> `src` 写成了 `lunac-plugin://localhost/...`，而 Windows 的 WebView2 **不能直接加载自定义
> scheme**（Tauri 只映射 `http://<scheme>.localhost`）⇒ 导航静默失败：`load` 与 `error` 都不触发，
> 只剩「3s 未握手：文档没有触发 load」。修法 = `pluginFrameUrl()`（`app/src/plugins/channel/host.ts`）
> 按平台给两种形态。**复跑前必须重启 dev**（前端是 Vite 现拉的，旧页面里的 `src` 不会自己变）。

### §测试 21.2 工具链授权（需要一份真实工具链资产）

> **状态（2026-10-29，用户真机确认通过）**：拒绝路径、授权账本、指纹失效（`dev` 段）、撤销
> 全部通过。**仍未验**：真实工具链资产的「安装 → 构建 → 停止」（没有资产）。
> 踩过的坑：改**顶层** `version` 不会失效（设计如此，指纹只认 `dev` 段）—— 见下文「指纹失效」。

**入口是 GUI，不用跟 AI 对话**：设置 →「插件」→ 下方「开发工具链」分区
（`#settings-toolchain-block`）。**它只列清单里带 `dev` 段的插件** —— 现有插件都没声明，
所以这块默认整块隐藏，属于正常；要做这一条得先造一份带 `dev` 的测试清单。

没有资产时只验**拒绝路径**（也必须过）：

1. 造一个清单（可复制 `scripts\probe-plugin\` 再改），`dev.toolchains[].sha256` 留空 /
   `dev.toolchains[].dest` 越出 `.toolchain\` / `command` 既不是包内路径也不在构建器白名单里
   → 铺进 `Modules\<id>\` 后「重新扫描」，该插件应判**坏包**（列表里带错误原因），不入库。
2. 改成合法形状（https + sha256 齐全、`dest` 在 `.toolchain\` 内）→ 重新扫描通过，
   设置里出现「开发工具链」区块；未授权时「安装 / 构建」不可用。

有资产时：

3. 授权 → `config\plugin-toolchain.json` 出现一条指纹记录。
4. 改清单 `dev` 段任一字段 → 原授权失效（重新要求授权）。
5. 撤销 → 构建进程被杀、可选删文件；**构建永不自动运行**（只能手动点）。

**做法（可照抄）**：新建 `app\src-tauri\target\debug\Modules\probe-tc\`，放两个文件 ——
`index.js` 写 `export default {};`，`lunac-plugin.json` 写：

```json
{
  "id": "probe-tc",
  "name": "工具链探针",
  "version": "0.1.0",
  "entry": "index.js",
  "dev": {
    "toolchains": [
      {
        "id": "demo",
        "url": "https://example.com/tool.zip",
        "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "dest": ".toolchain/demo",
        "binDir": ".toolchain/demo/bin",
        "kind": "archive",
        "version": "1.0.0"
      }
    ],
    "build": { "command": "node", "args": ["build.js"], "cwd": "" }
  }
}
```

**拒绝路径逐级试**（每次只改一处 → 设置 → 插件 →「重新扫描」→ 该插件应带错误不入库）：

| 改动 | 期望错误关键词 |
|---|---|
| `url` 改 `http://…` | `url 必须是 https` |
| `sha256` 改空串 | `sha256 必须是 64 位十六进制` |
| `dest` 改 `bin/demo`（不在 `.toolchain\` 下） | `dest 必须在 .toolchain\ 下` |
| `build.command` 改 `curl` | `只允许包内相对路径或常见构建器名` |

改回上面的合法形状 → 重新扫描通过 → 设置里出现「开发工具链」行；点「授权」→
`<exe 根>\config\plugin-toolchain.json` 出现 `{"probe-tc":{"fingerprint":"…","grantedAt":…}}`。

**指纹失效**：把 **`dev.toolchains[0].version`** 从 `1.0.0` 改成 `1.0.1`（**不是**顶层 `version`
—— 顶层的插件版本号刻意不参与指纹，改它不该让用户重新授权）→「重新扫描」→ 该行变成
「清单已变化，需重新授权」，按钮从「撤销授权」退回「授权」，账本里那条记录的 `fingerprint`
仍是旧值；再点一次「授权」→ 账本 `fingerprint` 换成新值。
（`dev` 段里别的字段同理：`url` / `sha256` / `dest` / `binDir` / `kind` / `build.args` 任一变化都会失效。）
点「撤销授权」要连点两次（就地二次确认），账本条目消失。
`example.com` 那条 `url` 不要真点「安装工具链」（会下载失败，属预期，不是验收判据）。

### §测试 21.3 Cookie 注入媒体代理（2026-10-29 新增，A/B 对照）

> **状态（2026-10-29，用户确认通过）**：步骤 0（httpbin 回显）+ A/B 对照均过。
> 附带结论：`cdn1.comicknew.pictures` / `img.guazicdn.com` 的 403 是目标站
> **Cloudflare `Attention Required!`**（与 Cookie 无关，按合规边界记，不做绕过）；
> `sy.mangafunb.fun` 的 502 属 helper 瞬时重置，重放为 200。

前置：一个封面 / 图片需要登录 Cookie 的来源（或任意防盗链图床）。

**0. 先跑确定性验证（不依赖任何站点登录，最硬的证据）**：媒体代理对 `httpbin.org/cookies`
取一次，看它回显的 JSON 里有没有我们塞进去的 Cookie。

```js
// 在 dev 实例里用 WebView2 的 CDP（`--remote-debugging-port=9222`）连到 resource-hub 插件窗，
// 或主窗（两侧 CSP 的 connect-src 都已放行 lunac-media）控制台执行：
const target = "https://httpbin.org/cookies";
const q = (s) => encodeURIComponent(s);
const withCookie = `http://lunac-media.localhost/resource-hub/${q(target)}?c=${q("sid=abc; theme=dark")}`;
const without   = `http://lunac-media.localhost/resource-hub/${q(target)}`;
console.log(await (await fetch(withCookie)).text());   // {"cookies":{"sid":"abc","theme":"dark"}}
console.log(await (await fetch(without)).text());      // {"cookies":{}}
```
两条 JSON 对得上 ⇒ 插件 → 宿主 `parse_options` → helper `Cookie` 头整条链是通的；
对不上再按下面第 4 步分段定位。Cookie 值的来源：**浏览器 DevTools → Network → 任一请求 →
Request Headers → `Cookie:` 整串**（`document.cookie` 拿不到 HttpOnly，登录态通常正是 HttpOnly）。

1. 来源 → 设置 → 填入 `Cookie`（与 `Referer`），保存（写进 `manifest.headers`）。
2. **A 组**：把 Cookie 清空 → 回总览刷新 → 封面应裂图 / 403。
3. **B 组**：填回 Cookie → 重新打开该来源 → 封面出现。
4. 链路核对（任一环节断了就是没过）：
   `app.ts` 的 `proxyMediaUrl()` 读 `headers.Cookie` → `media.proxy(url,{referer,userAgent,cookie})`
   → `plugin-window.ts` 拼 `c=` → 宿主 `plugin_media::parse_options()` → `media_proxy::fetch()`
   → helper 落成 `Cookie` 请求头。
   dev 下 helper 是 `app\media-proxy\target\{debug,release}\lunac-media-proxy.exe`。
5. 仍 403 时先**分开定性**：用普通浏览器带同样 `Cookie` + `Referer` 直接开同一图地址。
   浏览器也 403 ⇒ 站点反爬（合规边界，按 `blocked` 记，不再追）；浏览器能开 ⇒ 代理没带上，属缺陷。
6. 回归：不填 Cookie 的来源封面仍正常（缺省不发送 `Cookie`，不能变成空头 `Cookie:`）。

**判读：封面失败时先分清是谁拒的**（2026-10-29 dev 实测的三类）

| 现象 | 是谁 | 怎么办 |
|---|---|---|
| `403` + `text/html` + `<title>Attention Required! \| Cloudflare` | **目标站的 Cloudflare WAF**（换 UA / 带 Referer 都 403，TLS 指纹与挑战 cookie 都对不上） | 合规边界，**不算验收失败**，也不做 WebView 绕过；换源或放弃 |
| `403` + `text/plain` + `target rejected: …` / `no media.proxy permission` | **我们自己的宿主**（私网 / DNS 失败 / 清单没声明 `media.proxy`） | 看 `temp\logs\lunac-<日期>.log` 的原文；DNS 失败属本机网络 |
| `502` + 日志里连续两条 `media proxy helper started` | helper 被重启过（瞬时连接失败 → 重置 → 重试仍失败） | 刷新面板重试；**持续** 502 才查 helper（`media-proxy` 进程是否在跑） |
| `200` + `binary/octet-stream` | 正常（部分 CDN 不给图片 MIME） | `<img>` 按魔术字节解码，能显示即通过 |

> 实测样本（2026-10-29）：`cdn1.comicknew.pictures` 与 `img.guazicdn.com` 都是 Cloudflare
> `Attention Required!`（前者 4865 B / 后者 4573 B 的 HTML）—— 与 Cookie 注入无关；
> `sy.mangafunb.fun` 同一地址重放是 `200`、50 KB JPEG，当时的 502 属瞬时失败。

### §测试 21.4 播放器：三条退出路径 + 最小化

用任意 HLS 源（CMS 采集源即可），播放中逐条做：

| # | 操作 | 期望 |
|---|---|---|
| ① | 播放器里点「返回剧集」 | 声音立刻停；日志无新增分片请求 |
| ② | 主窗内嵌 iframe 插件（用探针插件）收起面板 | `iframe.plugin-iframe` 从 DOM **移除**（不是只 hidden）；这条与 resource-hub 无关，验的是宿主 `unmountIframePlugin()` |
| ③ | 播放中关闭插件窗 | 同上（`pagehide` → `destroy()`）；resource-hub 声明 `window.float`，内嵌面板不适用 |
| ④ | 播放中最小化窗口 → 恢复 | 最小化即停流；恢复后可点播继续（不要求自动续播） |

> **若「退出后还能听到声音」**：插件窗里执行
> `document.querySelectorAll("video").length` —— 若为 `0` 却仍有声，说明是**脱离 DOM 的播放器**
> （浏览器不回收仍在播的媒体元素）。2026-10-29 已用模块级 `liveVideos` 登记表 + `stopAllPlayers()`
> 在 `paintNow()` / `destroy()` 两处兜底；复测仍响时用 CDP `Media` 域看还有几个 player 在跑。

### §测试 21.5 播放器：HEVC 兜底 + 复制直链

1. 找一条 **HEVC** 线路（HLS 内 HEVC，或 H.265 的 mp4）播放。
2. 期望：提示「该线路编码不受支持（MKV / HEVC），请换线路或换源」，并出现「复制直链」；
   点「复制直链」后剪贴板内容 = 当前播放地址（被权限拦时退回重选文本框）。
3. 反例：能播的 H.264 MKV / mp4 **不得**出现该提示（`<video>` 试播成功就不该提示）。
4. 触发条件只认 `error.code` 3（DECODE）/ 4（SRC_NOT_SUPPORTED）、hls.js fatal、
   `Hls.isSupported()` 为假；网络错误（code 2）不在此列。
5. **「有声音、没画面」**（2026-10-29 报障）：视频轨编码不支持但音频轨能解时 `error` 不会触发
   ⇒ 播起来后最多 **8s** 应自动出现同一条「编码不受支持 + 复制直链」提示
   （判据：没报错、没暂停、`currentTime ≥ 1.2s`，但 `videoWidth === 0` 且
   `getVideoPlaybackQuality().totalVideoFrames === 0`）。超过 8s 仍只有转圈 = 未通过。
6. **「先出声、后出画面」**（2026-10-30 首报 / **2026-10-31 二次报障后加固**）：点开任意线路，
   起播阶段应**先静音**，满足「**两帧**已呈现 + `videoWidth > 0`」才恢复声音 —— 听感上必须是
   「声画同时出现」，不允许「有声、画面还在加载」的两三秒。
   - 判据从一帧改成两帧的原因：起播的第一帧常是**起始黑帧**，一帧就放声等于黑屏出声。
   - 纯音频流 / 引擎不给帧回调：最迟 **开播后** 8s 放声（不永久静音）；注意这 8s 从
     `playing` 起算，**不是**从点开线路起算 —— 慢源拉流 7s + 开播 1s 才不会提前放声。
   - 先确认跑的是新构建：设置 → 插件里「阅读」的版本应为 **0.2.1**（0.2.0 = 只有旧的一帧规则）。
7. **「同一段声音隔约 0.5s 又响一次」（2026-10-31 报障，每条线路都会）**：
   开插件 → 快速点作品 → 点集数（模拟「开插件后 0.5s 内开始播放」）⇒ 起播后应**只响一遍**。
   - 反例（旧行为）：前 0.5s 的声音再重复一次 = 未通过。
   - 要验的是「迟到的重绘不再重挂播放器」：`view=player` 时 `paintNow` 直接返回；
     同时 `mountPlayer` 入口会兜底停掉任何残留的 `<video>`。
   - 换线路也验一次（应同样只响一遍）。
   - 版本门槛：设置 → 插件里「阅读」应为 **0.2.3**（0.2.4 仍须满足本条）。
8. **「退出后音频还在后台播」（2026-10-31 报障，用户原话「不管哪个退出音频都会出现后台播放」）**：
   播放中回主窗再点一次「阅读」（宿主发现同名插件窗已开着 ⇒ 给**同一个**窗口推
   `plugin-window-input` ⇒ 插件窗 `render()` 整段替换结果区）⇒ 声音必须**立刻停**，
   界面回到卡片 / 首页。
   - 反例：界面已经回到卡片、声音还在继续 = 未通过（0.2.3 就是这个行为）。
   - 另验两条：播放中点插件窗标题栏「×」；播放中先最小化再关闭 —— 都不该留残余声音。
   - 现场判据：插件窗控制台里 `document.querySelectorAll("video").length` 应为 `0`；
     若为 0 却还有声，说明是**脱离 DOM 的播放器**（看 `player.ts` 的 `liveVideos`）。
   - 机制：`paintNow` 的早退要求 `playerHandle.isAttached()`；`timeupdate` 看门狗发现
     `!video.isConnected` 会自己 `destroy()`（最迟约 250ms）。
   - 版本门槛：设置 → 插件里「阅读」应为 **0.2.4**。

### §测试 21.6 一键安装失败提示 + uzVideo 导入即拦

1. 来源 → 导入规则 → 粘贴含 `//#BaseCode1#` 与 `//@codeID:<非空>` 的批量索引 → 一键安装。
2. 期望：失败条数为 N，提示形如「名字：原因」，**最多 3 条**；已装来源里**不出现**这些条目
   （导入阶段就拦，不是装完搜索才报 `unsupported`）。
3. 反例：`//@codeID:` **空值**（模板占位）的正常源必须能装（不能误杀）。

### §测试 21.7 uzVideo sniffer（播放页 → 真直链）

前置：一条 `getVideoPlayUrl` 返回**播放页**的来源，且清单带 `webview.sniff`
（见 §测试 13.1 的日志口径）。

1. 搜索 → 详情 → 点某一集。
2. 日志依次出现：`webview sniff start <播放页>` → `webview sniff 命中 <m3u8|mp4>` →
   `webview sniff 完成：N 条地址`；播放器起播。
3. 降级：嗅探为空 / 旧宿主无 `sniff` ⇒ 原地址原样保留（播放器报不支持），**不得抛异常**。
4. 一次 `play()` 最多嗅 3 个播放页（多线路只挑前几个）。

### §测试 21.8 A7 对话入口（一期 + 二期）

1. 对话里说「帮我找 XX 漫画」⇒ 模型调 `ResourceHubOpen` ⇒ 聊天出现工具卡
   （标题 + 搜索词 + 类型 + 「打开阅读」）。
2. 点「打开阅读」⇒ 阅读窗打开、`#rh-q` 已填好、自动出结果。
3. 该轮搜索完成后 ⇒ 卡片下方出现前 **8** 条结果（`lunac-resource-hub-results` 广播）。
4. 点其中一条 ⇒ **直接进该作品详情**（章节由详情页自取）；来源已卸载 ⇒ 退回按标题搜索，不给空白页。
5. 再点同一张卡 ⇒ 已开的窗被**前置并重新搜**，不新开窗。
6. 反例：用户自己手搜不回传；「加载更多」不回传；卡片随聊天重渲染会丢（预期，不修）。
7. **入口不带搜索词（2026-10-31 用户口径）**：在主搜索框输入「斗罗大陆」→ 点搜索结果里的
   「阅读」⇒ **阅读窗打开的是正常首页**（搜索框为空、不发起任何搜索）。
   反例（旧行为）：一打开就落在「斗罗大陆」的搜索结果里 = 未通过。
   注意第 2 步的 AI 卡「打开阅读」**仍然**要带搜索词 —— 两条入口的口径不同，别一起改掉。

### §测试 21.9 A6 技能端到端（模型照技能写规则并装成功）

前置：`app\src-tauri\target\debug\skills\resource-hub\SKILL.md` 存在（与 code-review / debug 同级）。

1. 对话里让模型「用 resource-hub 技能给 <某站> 写一条来源规则并安装」。
2. 期望链路：`SourceValidate`（必填项过）→ `SourceInstall`（写进 `ModuleData\resource-hub\sources\`）
   → 插件里出现该来源。
3. 装完立刻搜一次要能出结果。常见坑（技能正文已写明）：接口返回 JSON 必须写 `list.jsonPath`；
   翻页只靠 `@page` 占位符（`paging` 只有清单层声明，执行器不读）。

### §测试 21.10 B3 书架导入 / 导出（内嵌 + 独立窗各一次）

1. 书架页右上「导出书架」⇒ 导出页文本框**只读**、`复制` 可用；复制得到的 JSON 满足
   `{kind:"lunac-resource-hub-library", version:1, exportedAt, library, progress, bookmarks}`。
2. 改一个作品的阅读进度、加一个书签 ⇒ 粘贴**旧** JSON ⇒「合并导入」可用态随文本变化。
3. 期望：进度取 `updatedAt` 更晚者（新进度不被旧备份冲掉）；书签取**并集**；
   坏条目静默丢弃（只按 `workId` / 「章:页」去重），只有 `kind` / `version` 不对才整体报错。
4. **入口各一次**：搜索结果点开、聊天卡「打开阅读」，两处都进同一个插件窗；两次都要能看到
   「导出书架 / 导入书架」按钮且行为一致（`window.float` ⇒ 一律独立窗，主窗内嵌不适用）。

### §测试 21.11 百合会 loadEp 真机复核

1. 装百合会来源 → 搜「高嶺之花」→ 详情 → 打开该章。
2. 期望：`chapters` 1 条；`pages` **15 张**（与页面 `li.last` 的 `data-page=14` 一致）；
   详情页标签显示分组形态（`作者：透村` / `更新：…` / `标签：翻页漫画`）。
3. 若仍 `parse` 报错 ⇒ 抓日志回填（2026-10-28 的宿主两轮修复后离线已通，真机未点过）。

### §测试 21.12 加密图源：章节图片能出画（2026-10-29 新增）

> 覆盖 `comic.onImageLoad` 的两件事：**逐页 headers**（图片 CDN 的 Referer 与清单不同）
> 与 **`onResponse` 字节变换**（加密图源）。代表来源 = **漫蛙吧**（`comic.manwaba`，
> 图片是 AES-256-CBC 加密的 WebP，前 16 字节是 IV）。

1. 装 `comic.manwaba`（漫蛙吧）→ 搜任意作品 → 打开任意一章。
2. 期望：章节图片**逐张出画**（不再是一排坏图 / 空白）；控制台**没有**
   `comic image transform failed`。
3. 反证（确认坏的环节不是代理）：同一张图 `curl.exe` 直连是 `200 image/jpeg`，
   但首 4 字节是 `33 E9 0C ED`（不是 `FF D8 FF` / `RIFF`）⇒ 上游发的是**密文**，
   只能由源的 `onResponse` 解。
4. 回归：不带 `onImageLoad` 的来源（百合会、comick）章节图仍正常。
5. **封面（2026-10-30 补）**：同一来源的**搜索结果网格 / 远程详情页封面**也应逐张出画
   （封面走 `onThumbnailLoad`，加载失败时插件才去解码，控制台无 `cover transform failed`）。
6. 已知边界：受沙箱「结果 ≤1 MiB」限制，超大图/封面会退回坏图；`mkv` 之类与图片无关。

### §测试 21.13 判定表（走完逐行打勾）

| 编号 | 判定（全过才算） | 结果 |
|---|---|---|
| 21.0 | dev 起得来、`lunac-media-proxy.exe` 时间戳是本次、数据根已声明 | ✅ |
| 21.1 | 探针 ①–⑥ 全过，两处锚点不残留 | ✅ |
| 21.2 | 拒绝路径必过；有资产时授权失效 / 撤销 / 不自动构建过 | ✅（真实资产安装/构建未验） |
| 21.3 | A/B 对照成立（清空 Cookie 403、填回能出图）+ 不填 Cookie 的来源不回归 | ✅ |
| 21.4 | ①③ 停流 + iframe 面板移除 + 最小化暂停恢复 | ☐ |
| 21.5 | HEVC 提示 + 复制直链可用；H.264 不误报 | ☐ |
| 21.6 | 失败提示「名字：原因」≤3 条；空 `codeID` 不误杀 | ☐ |
| 21.7 | 嗅探三条日志齐全并起播；嗅探为空时降级不崩 | ☐ |
| 21.8 | 说 → 卡 → 开窗 → 回传 8 条 → 点条目进详情 → 再点前置 | ☐ |
| 21.9 | 模型照技能产出规则 → 校验 → 安装 → 搜得出结果 | ☐ |
| 21.10 | 导出可复制、合并导入不冲掉新进度；两个入口都在 | ☐ |
| 21.11 | 15 张 + 分组标签 | ☐ |
| 21.12 | 漫蛙吧章节图片逐张出画；无 `comic image transform failed`；其他来源不回归 | ☐ |

全过 ⇒ 资源库功能项验收完成，剩下只有**出包**
（`scripts\build-plugins.ps1` → `scripts\build-release.ps1`；版本仍 `0.9.43`）与出包后按
§测试 13 口径对安装版抽查。

### §测试 21.14 长期后台打开后的加载页与首帧（2026-10-31）

> 对应 `resource-hub-design.md` 的「长期后台打开：加载数据时不进加载页」。要验的是
> **窗口被挂起很久之后，重新触发加载时界面有没有立即给出反馈**，以及恢复时不再顿一下。

1. 开阅读插件，停在总览，**最小化或让窗口被遮挡 ≥ 10 分钟**（Chromium 会对后台窗口
   挂起 rAF、把定时器节流到 1 次/分钟）。
2. 切回窗口：第一屏若还在加载，应**立即**看到加载条（`#rh-source-loading` 或 `#rh-more`
   里的「正在加载来源…」），不允许出现「先看到旧界面 / 空白，过一会儿内容突然出现」。
3. 在窗口里点「刷新总览」（或在 20+ 来源时冷启动）：点击后**同一帧**就出现加载条，
   然后才读到宿主 KV 缓存、再联网 —— 不允许「点了没反应」。
4. 数据到达后加载条消失、卡片出现；滚动不出现 >200ms 的停顿。
5. 关掉插件再打开（后台 ≥ 10 分钟后再打开）重复第 2 步，结论相同。
6. 版本门槛：设置 → 插件里「阅读」应为 **0.2.5**。

## §测试 22 音乐插件：本地 WebM（2026-10-30）

> **分工没有变**：画面仍由插件窗的 `<video>`（WebView2）出，声音仍走宿主 `player.rs` 的
> rodio。变的是 **WebM 的音轨要先抽** —— rodio 没有 Matroska 解复用器，宿主用本机
> ffmpeg 把音轨转成缓存的 m4a（`<exe 根>\temp\webm-audio\<16 位>.m4a`，最多留 20 份）。

1. 准备：本机 `ffmpeg` 可用（exe 根自带，或 `where ffmpeg` 能查到）。
2. 本地音乐 → 扫描一个含 `.webm` 的目录 → 该文件应**出现在列表里**（标题取文件名）。
3. 点播：底部先出现「首次播放 WebM 要先抽取音轨，可能要十几秒…」；
   抽完后**画面出、声音出**，进度条与画面同步（宿主是唯一时钟）。
4. 再点一次同一集：应**秒开**（命中缓存，不再转码）；`temp\webm-audio` 里文件数 ≤ 20。
5. 反例（缺 ffmpeg）：把 ffmpeg 从 PATH / exe 根移走后点播 ⇒ 提示
   「播放 .webm 需要本机 ffmpeg…」并**拒播**，不能出现「有画面没声音」。
6. 回归：`mp4 / m4v / mov` 不走抽轨，行为与之前一致；`mkv / avi / wmv / flv`
   **仍不入库**（Chromium 不解这些容器）。

---

## §测试 23 图片标注工具（`ImageMeasure` / `ImageAnnotate`，2026-10-30）

> **这一节验的是「模型不再自己猜坐标、不再现写绘图脚本」**（`todo.md` §待办 3-19）。
> 历史现场：2026-10-27 / 29 的 `mvqmx4a24j0w` 会话，一张四杯照片标尺寸用了 12 轮、
> 用户纠正 6 次，最后放弃。

1. 准备：把那张 1540×920 的四杯照片放进工作区（任意目录）。开一个**新**对话。
2. 量：让 agent「先量出四个杯子的外轮廓，再报数」。预期看到 `ImageMeasure` 调用
   （**不弹审批卡**），返回值里的 `width` / `height` 与图片一致，`bbox` 落在杯体上。
   打开 `ascii_columns: 60` 时，ASCII 缩略图的形状应当**能认出四个杯子**。
3. 画：让 agent「按机械制图画出每个杯子的高度：两条尺寸界线 + 反向箭头 + 中文标签」。
   预期：`ImageAnnotate` **弹一次审批卡**（写类），批准后输出一张新 PNG
   （缺省 `<原名>.annotated.png`，源图不动）。
4. 看结果：agent 应当 `Read` 那张 PNG 并自查。人工核对四件：
   - 中文标签**不是乱码**、不是方框（字体走系统 GDI+，与 `.ps1` 编码无关）；
   - 箭头尖端**落在尺寸界线上**（`arrow_dir: "out"` 的标准画法）；
   - 数值**不压**图案（默认落在线的法线 -14 px 一侧，即水平线上方）；
   - 同一条尺寸线两端的箭头**方向相反**（尖端相背）。
5. 复算：让 agent 用 `ImageMeasure` 再量一次输出图，报出的线段 bbox 应与规格里的端点
   一致（±2 px 的抗锯齿误差）。这一步就是「坐标可被复算」的验收点。
6. 反例（越界）：故意给一个超出图宽的 `to.x` ⇒ 工具**直接拒**，错误里带出
   `"outside the 1540x920 image"`；模型下一轮据此改对，而不是拿到一张被静默裁剪的图。
7. 反例（只读档 / 计划相位）：切到只读档后调用 `ImageAnnotate` ⇒ **不问审批、直接拒**
   （`write_blocked` 的文案指向「去设置里改成项目档位」）；`ImageMeasure` 仍然放行。
8. 反例（拼错规格）：把 `"type": "line"` 写成 `"dimenison"`，或把 `color` 写成 `colour`
   ⇒ 报错并列出允许值，不会画出一张「少了一条线」的图。
9. 计数：同一张照片从「12 轮」降到 **≤3 轮**（量 → 画 → 自查一次微调），
   且**全程不需要用户口述坐标**。

## §测试 24 子代理角色常驻（§待办 3-22 分片 4，2026-10-31）

> **这一节验的是「同一角色第二次派发能接上上一次的上下文」**。默认关（跟检查点同一个开关），
> 所以第 1 步必须先打开开关，否则后面全是旧行为 —— 那本身也是一条要验的边界。

1. 关着开关（对照）：`lunac --print "<让 agent 用 role=explore 查一个符号>"`（**不带**
   `--checkpoint`）⇒ `ModuleData\tasks\sessions\roles\` 目录**不出现**新文件；
   `Agent` 的行为与改造前一致（每次新起）。
2. 打开开关：`lunac --print --checkpoint "<同一句话>"` ⇒ 子代理跑完后出现
   `roles\explore.json`，内容里 `turns` 有 1 个回合（`user` 提问开头、`assistant` 报告结尾）。
3. 接上一段：再跑一次同角色的派发（不同问题）⇒ 启动日志出现
   `子代理角色 explore 接上上一段会话（N 条消息）`；`roles\explore.json` 的 `turns` 变成 2 个。
   模型这次**没有重新查**上一轮已经报过的位置（这就是「接上」的可观察证据）。
4. 不串台：换成 `role=review` ⇒ 读的是 `roles\review.json`（全新），
   不会带上 explore 那一轮的内容；`general`（不写 role）**永不常驻**，不产生任何 `roles\general.json`。
5. 裁剪：连续派 5 次同角色 ⇒ 文件里只留**最近 4 个回合**（`MAX_TURNS`），
   最旧的那个回合**整段消失**（不会出现「开头是 tool_result」的半截对话）。
6. 失败不致命：手工把 `roles\explore.json` 改成 `not json` 再派发 ⇒ 日志
   `读取角色 explore 的常驻会话失败，按新会话继续：…`，本次派发照常完成；
   落盘时旧文件被改名成 `explore.json.bad`，新文件是完整的一份。
7. 反例（残缺不写）：把 `roles\coding.json` 的最后一个 `assistant` 删掉（只剩 `user`）⇒
   该回合在读取时被**整段跳过**并 warn；新写入的回合仍然以 `assistant` 结尾。
8. 不接常驻的两条路径：fork 技能（`context: fork`）与后台复盘 fork 都**不读写**
   `roles\` —— 它们不是「同一角色的重复派发」（单测 `subagent_seed_history` 覆盖了传空切片）。

## §测试 25 复核角色闭环（§待办 3-22 分片 5，2026-10-31）

> **这一节验的是「reviewer 只说不改，且主线程能一眼看出该不该返工」**。回执是**格式判断**，
> 不判断证据真假 —— 别拿它当「代码肯定没问题」的凭据。

1. 只读（工具池）：让主 agent「派一个 `role=review` 的子代理看 `core-agent/src/tools.rs` 的
   `todo_write`」。展开子代理的内部过程，确认它**只**调用了 `Read` / `Glob` / `Grep` / 命令类；
   让它「顺手把那个 unwrap 改掉」⇒ 子代理**做不到**（工具集里没有 `Write` / `Edit`）。
2. 回执在前：子代理报告回到主对话时，`tool_result` 的**第一段**应是
   `[review receipt] read-only reviewer — it cannot edit files, so nothing was changed by this step.`
   后面依次是 `file:line evidence: N line(s).` 与 `unverified (...)`，最后一句是
   `DECISION: you decide whether to rework …`。
3. 证据计数：报告里每有一行带 `src/x.rs:12` 这种引用，`N` 就 +1；让 review 只给「看起来没问题」
   的结论（不带任何 `file:line`）⇒ 回执必须写 `file:line evidence: none — treat every finding
   below as unverified.`
4. 未验证小节：报告以 `## unverified` 收尾时，回执逐条列出（最多 8 条）；超过 8 条末尾出现
   `…(+K more, see the report)`，括号里的总数仍是**真实总数**。
5. 缺小节：把提示词里的要求绕过（例如让它「用一段话总结」）⇒ 报告没有 `## unverified`，
   回执必须写 `unverified: NOT DECLARED … treat every claim below as unverified.`
   ——**不能**静默当作「全部已验证」。
6. 显式 none：报告写 `## unverified` + `- none` ⇒ 回执写 `unverified: none declared.`
   （这是唯一一种「没有未验证项」的合法形态）。
7. 决策留痕：主线程拿到回执后，应明确写出「接受」或「返工」。返工时派 `role=coding`，
   并把回执里那份 `file:line` 清单原样带进 prompt；**不要**指望 reviewer 自己修。
8. 回归：`cargo test subagent_role` 11 passed（回执 5 条 + 角色其余 6 条）；
   `cargo test subagent_role` 里 `subagent_role_defs_narrow_tool_pool_per_role` 仍钉着
   「review 的工具池没有写类」。

## §测试 26 持续自治 + IDE 附着（§待办 3-23 分片 1–3，2026-10-31）

> 三条各自独立验收：IDE 右键提问、后台按需唤醒、唤醒源登记。第 2–3 条看的是**宿主日志**
> （`ModuleData\logs\`），不是界面。

**A. IDE 选文件提问（分片 1）**

1. 在 VS Code 里打开本仓库，`F5` 启动扩展宿主（或装 dev VSIX）。
2. 资源管理器里右键任意 `.rs` 文件 →「Lunac: Ask About This File」⇒ 侧边栏 AI Chat 自动
   切到前台；输入区上方出现一条**用户消息**，内容是「请分析 `core-agent/src/tools.rs`
   （整个文件）…」，**不含文件正文**。
3. 编辑器里选中 12–30 行再右键同一命令 ⇒ 消息变成「…（第 13–31 行）…」（0 基行号 +1）。
4. 没启动过会话时点这个命令 ⇒ 它会自己启动 `agent.exe`（等同面板里的「启动」），并正常发出；
   启动失败（找不到 exe / provider 没配）⇒ 弹错误提示，**不会静默什么都不发生**。
5. 协议回归：整个过程中 `chatView.ts` 发的仍是 `{type:"user", message:{role,content}}`；
   宿主→webview 只多用了已有的 `{type:"user"}` 回显 —— **没有新增消息类型**。

**B. 后台按需唤醒（分片 2）**

6. 没有启用任何 cron job 时启动宿主 ⇒ 日志出现一次
   `cron 唤醒判据：reason=idle 等待=60000ms`，之后**不再每 30s 刷** `cron` 行；
   `schedulerAlive` 仍是 `true`（心跳 ≤60s < 90s 窗口）。
7. 建一条 `every 15` 分钟的 job（`nextRunAt` 在 10 分钟后）⇒ 日志出现
   `reason=wait 等待=600000…` 之类的封顶值（`60000`），到点前不会再频繁醒；
   到点后确实执行，日志出现 `wake_source=timer detail=1 cron job(s) ran`。
8. 把系统时间 / job 的 `nextRunAt` 改成「已经过期」再等一轮 ⇒ 出现 `reason=due`，
   且执行完立刻回到 `wait`（**不会**因为仍看到过期而空转：tick 之后退 1s 再看）。
9. 关掉所有 job ⇒ 回到 `reason=idle`；此时把 `cron.json` 手动改回启用，最多 60s 内被看到
   （这就是上限存在的意义：外部改动不能等下一个 `nextRunAt` 才被发现）。

**C. 唤醒源登记（分片 3）**

10. `POST /agent/start` 或扩展里发一句话 ⇒ 宿主日志出现
    `wake_source=ide_request detail=POST /agent/message`。
11. 用 agent 侧的 `TaskEnqueue` 入队一条 ⇒ 3s 内日志出现
    `wake_source=queue_file detail=pending task <id>`，紧接着 `running`。
12. 空转时**不该**出现任何 `wake_source=` 行（它是「真的干活」的登记，不是心跳）。
13. 三类来源的名字是固定枚举（`timer` / `queue_file` / `ide_request`），
    单测 `the_three_wake_sources_have_stable_names` 钉着 —— 加第四类必须改 `wake.rs`。

## §测试 27 自研代理引擎（SS / SS-2022 / Trojan / VLESS / VMess / AnyTLS / Hysteria2 + 规则，2026-10-31）

> 对应 `docs/todo.md` §待办 3-25。**自动化部分已经全绿**（`cargo test --bin lunac native_proxy`
> ⇒ 53 条：规则 / 分享链接 / 订阅解析 / SS 两代（SIP004 + SIP022）的回环 / **SS UDP 回环** /
> Trojan 握手形状 / VLESS 头与 UUID / **VMess（KDF 已知答案 + authID + 头与分块回环）** /
> **AnyTLS（认证 + Settings/SYN/PSH + 心跳回环）** /
> **Hysteria2（varint 已知答案 + 地址串 + 认证请求形状 + salamander 混淆）** /
> **SOCKS5 UDP 数据报编解码** / WS 握手与分帧 /
> SOCKS5 与 HTTP CONNECT 两条端到端）；
> 下面第 1–16 条是**真机接真实节点**的那一半。

**A. 先确认跑的是自研引擎**

1. **缺省就是 native（2026-11-01，§待办 3-26 K1）**：删掉 `<exe 根>\config\proxy.json`
   （或让它不含 `engine` 字段）后启动 ⇒ 状态行应显示 native。**不再需要手改配置**。
   反证（K4）：手工写 `"engine": "mihomo"` ⇒ 启动**直接报**「当前版本已不再内置 mihomo …」，
   **不会**静默跑 native。
2. 代理插件 → 状态行应显示服务 / 引擎状态（启用之后显示 `服务已开启 · 127.0.0.1:<port>`）；
   按钮里**没有**「下载内核」这个入口（K3 已把下载 / 安装整条路删掉）。
   旧配置 `engine: mihomo` 时：状态行显示同一句提示，**整排按钮不出现**（不给一个点了必失败的启动）。
2b. **引擎 / 服务拆开（2026-11-03）**：只打开代理插件、**不开服务** ⇒
   `mihomo_status` 应为 `running=true / enabled=false`（引擎被静默拉起）；状态行显示
   `引擎就绪（服务未开启）· 127.0.0.1:<port>`；此时直接点「测试连通性」应能出延迟
   （探针 = 直连可达的 `www.baidu.com/`），搜索区状态栏泡泡框显示 `代理:关`。
   点「开启服务」后泡泡框变 `代理:开`；关闭服务后**引擎仍在**（还能继续测）。
   关闭插件时：服务开着 ⇒ 引擎留后台（`running=true`）；服务没开 ⇒ 引擎被停掉
   （`running=false`）。
3. 没有 ss / trojan / vless / vmess / anytls / hysteria2 节点的订阅 ⇒ 点「启动」直接报
   「自研引擎没有可用节点：只支持 ss / trojan / vless / vmess / anytls / hysteria2 …」，
   **不会**假装启动成功。
3b. `RULE-SET`（2026-11-18）：订阅里写 `rule-providers:`（`type: inline` 最省事）+ 
   `rules: - RULE-SET,<name>,DIRECT` ⇒ 启动后访问该规则集里的域名，请求日志显示「直连」，
   且状态里的 `skipped` **不再**出现「RULE-SET,<name>」；把 `type` 写成 `http` 且 URL 不可达
   ⇒ 首次启动会同步拉一次（最多 5s）后如实跳过并记原因，**不假装规则生效**。
3c. `mux.cool`（2026-11-18）：分享链接带 `mux=1`（VLESS / Trojan）或 vmess JSON 带
   `"mux":"1"` ⇒ 节点照常打开网页；Xray 服务端日志应出现
   `received request for tcp:v1.mux.cool:9527`（之后的多条业务连接都从这条会话里出）。
   对照：不开 mux 时每条 TCP 各拨一次；把 Clash 的 `smux: enabled: true` 写进订阅
   **不会**被当成 mux.cool（两套协议不兼容），节点仍可用但不会复用。

**B. 浏览器（系统代理）**

4. 启停一次：关闭后 `设置 → 网络 → 代理` 应回到原来的状态（`system_proxy.rs` 的备份 / 还原
   在新旧两条内核路上**始终是同一份**）；打开后 `curl -x http://127.0.0.1:<port> https://www.gstatic.com/generate_204 -I`
   应返回 `HTTP/1.1 204`。
5. 直连规则：`curl -x http://127.0.0.1:<port> http://127.0.0.1:8788/health` 应**不经过节点**
   （本机网段在内置 `lan_rules()` 里）—— 把节点停掉再测一次，它仍应成功。
6. 节点切换：`mihomo_select` 用的还是同一张节点表；切完再访问一次，出口 IP 应随节点变。

**C. 边界（必须如实失败，不能静默）**

7. 订阅里只有 `ssr://` / `ss://…?plugin=kcptun` / `hysteria2://…?obfs=nope` ⇒ 启动报
   「没有可用节点」，并逐条记「跳过」日志（未知协议 / 未知插件 / 未知 obfs / alterId≠0 / `network=kcp`
   都会走到这里；`grpc` / `reality` / `vision` / `gecko` / simple-obfs / v2ray-plugin / `tuic`
   现在已支持，不再走跳过；TUIC v4 的纯 token 形态仍跳过并给出「缺 uuid:password」）。
8. SOCKS5 的 **BIND** ⇒ 回 **0x07（命令不支持）**；HTTP 的 `https://` 绝对 URI
   （没有 CONNECT）⇒ 回 `400`。`UDP ASSOCIATE` 现在**支持**（见 J 段）。
9. 端口被占用时（先手动监听同一端口）⇒ 日志出现「端口 N 被占用，改用一个空闲端口」，
   启动仍成功，`mihomo_status.mixed_port` 是**实际**端口。

**D. 旧配置 mihomo 的处理（§待办 3-26 K4）**

10. 手工写 `"engine": "mihomo"` 后启动 / 点启用 ⇒ 报「当前版本已不再内置 mihomo …」，**不静默**
    改跑 native；把该字段改成 `native`（或删掉）再启动 ⇒ 正常接管。
    自研引擎在跑时调 `mihomo_set_engine("native")` ⇒ 报「先停用当前内核，再切换」；停用后切换成功。
    任何时刻调 `mihomo_set_engine("mihomo")` ⇒ 同一句「已不再内置」错误，不写进配置。

**E. VLESS / WS 传输（2026-10-31 第二轮）**

11. 订阅里同时有 `VLESS + TLS`、`VLESS + WS + TLS`、`Trojan + WS` 三类节点时：
    - 三类都应出现在节点列表里（不再是「跳过」）；
    - 逐个切过去访问 `https://www.gstatic.com/generate_204`，都应 204；
    - 带 `network: kcp` 的节点应**不在**列表里并记一行「跳过」（KCP 明确不做）；
      `flow: xtls-rprx-vision` / `security: reality` / `network: grpc` 现在都在列表里。

**F. SS-2022（2026-10-31 第三轮）**

12. 订阅里带 `cipher: 2022-blake3-aes-128-gcm`（或 `-aes-256-gcm` / `-chacha20-poly1305`）的节点：
    - 应出现在列表里并可用（`generate_204` 返回 204）；
    - 多用户 PSK（`password` 形如 `iPSK:uPSK`）的节点应能连（**204**）：客户端在 salt 后
      写 SIP023 identity header，会话仍用 uPSK；PSK 长度不对（base64 解出来不是 16/32 字节）
      同样要报出来，**不是**静默连不上。
12b. SIP003 插件（2026-11-03 第十三轮）：订阅里分别放
    `ss://…?plugin=obfs-local;obfs=http;obfs-host=…`、`…;obfs=tls`、以及
    `plugin: v2ray-plugin` + `plugin-opts.mode: websocket`（可加 `tls: true`）三条：
    - 三条都应在列表里，`generate_204` 都返回 **204**；
    - 再加一条 `mode: quic` ⇒ 应**不在列表**并记一行「跳过」（v2ray-plugin 的 quic 不做）；
    - 经插件节点做 DNS（SOCKS5 UDP ASSOCIATE）⇒ 应无回包，日志一行
      「SS 插件只承载 TCP，UDP 不转发」。

**G. VMess（2026-10-31 第四轮）**

13. 订阅里带 `type: vmess`（`alterId: 0`）或 `vmess://base64(JSON)` 的节点：
    - 应出现在列表里；逐个切过去访问 `https://www.gstatic.com/generate_204` 应 204
      （TCP 与 WS 两种 `net` 各测一条；WS 通常还叠 TLS）；
    - `alterId` 非 0 的节点应不在列表里并记一行「跳过」——老式认证已废弃，不做假装；
    - 把 `uuid` 改错一位后再连：应在第一次读到响应头时报「VMess 响应头的回显字节不匹配」或
      「VMess 解密失败」，**不是**一直挂着直到超时（响应头是惰性读，`connect` 不会死等）。

**H. AnyTLS（2026-10-31 第五轮，S1.5）**

14. 订阅里带 `type: anytls` 或 `anytls://password@host:port?sni=…` 的节点：
    - 应出现在列表里并可用（`generate_204` 返回 204）；
    - 密码写错 ⇒ 服务端会直接关连接：浏览器侧应快速失败并记一行 TLS/EOF 错误，
      **不是**挂到 12s 拨号超时；`cmdAlert` 有文本时要把它带进日志；
    - 同一节点连续访问两个不同站点（两个入站连接）都应成功 —— 本引擎是「一条入站一个
      TLS 连接一个 Stream」，不复用 Session，这条确认服务端接受这种用法。

**I. Hysteria2（2026-10-31 第六轮，S2）**

15. 订阅里带 `type: hysteria2`（`skip-cert-verify: true`）或 `hysteria2://…?insecure=1` 的节点：
    - 应出现在列表里并可用（`generate_204` 返回 204）；节点若**没有** `insecure` 且是自签证书，
      握手应在 12s 内失败并记 TLS 错误（不是静默超时）；
    - 密码写错 ⇒ 服务端返回非 233，日志应出现「Hysteria2 认证失败：HTTP <code>」；
    - 目标地址不可达 ⇒ 应出现「Hysteria2 出站被拒绝：<服务端消息>」；
    - **`obfs: salamander` + `obfs-password`** 的节点应同样可用（204）；`obfs-password` 写错
      ⇒ 握手失败（服务端看到的是乱码数据报），**不会**连上；
    - `obfs: gecko` 的节点应出现在列表里并可用（204）；`obfs-min-packet-size` /
      `obfs-max-packet-size` 非法的配置应被跳过（不让它在连接时才失败）；
    - 该节点下再做一次域名解析（SOCKS5 UDP ASSOCIATE）应收到应答 —— UDP 走 QUIC 数据报 +
      `UDPMessage`，服务端 `Hysteria-UDP: false` 时日志应如实报「未声明支持 UDP」。

**I2. 自签证书 `skip-cert-verify` 的通用校验（2026-11-03，第十八轮）**

15b. 用**同一张自签证书**起 trojan / vless+ws / vless+h2 / vless+grpc / vless+xhttp /
    vless+http 伪装 / anytls / vmess 各一条，逐条 curl：
    - 节点带 `skip-cert-verify: true`（链接写 `insecure=1` / `allowInsecure=1`；
      vmess 的 JSON 写 `allowInsecure: true` 或 `"true"`）⇒ 都应 204；
    - 同一条节点**去掉**该开关 ⇒ 应在 12s 内以 TLS 错误失败（不是静默超时），
      证明校验没有被默认放宽；
    - 明文节点（ss 裸、vless 无 `tls`）写上该开关不应报错，也不影响连接。

**J. SOCKS5 UDP ASSOCIATE（2026-10-31 第八轮）**

16. 用 `curl --socks5-hostname 127.0.0.1:<port> https://www.gstatic.com/generate_204` 走 TCP 仍 204 之后，
    再验 UDP：
    - **直连目标**（本机 / 私网，如 `127.0.0.1` 上的 DNS 或 QUIC 服务）⇒ 应通，且关掉 SOCKS5
      控制连接后转发立即停止（再发数据报没有回包）；
    - **走 SS 节点的目标**（如 `8.8.8.8:53` 的 DNS 查询）⇒ 应收到应答；把节点密码改错后
      应**没有应答**并在日志出现一行「UDP 上游不可用」（不是刷屏）；
    - 当前节点若是 SS / SS-2022 / Trojan / VLESS / VMess / AnyTLS / Hysteria2 ⇒ 都应能转发
      UDP；只有真的建不起上游时，日志才出现一行「UDP 上游不可用」，**不是**静默卡死；
    - 带 `FRAG != 0` 的数据报应被丢弃（可用 `scripts` 里的小 UDP 客户端构造）。

## §测试 28 白箱边界账本（W0，2026-11-01）

> 对应 `docs/todo.md` §待办 3-2 的 W0，设计口径见 [`sandbox-research.md`](./sandbox-research.md) §六 / §七。
> **它不是沙箱**：只记录 / 解释 / 导出「为什么放行、为什么问我」，不新增任何拦截；
> 真正的拦截仍在审批卡 / agent / 工作区锁（`forbidden.md` §禁用 1：不得自称沙箱）。
> 自动化已全绿：`cargo test --bin lunac security_ledger` ⇒ 4 条（追加即脱敏并倒序读回 /
> 小写原因码与未知 `decision` 被拒 / 超 32 MiB 自动换 `-2` 分片 / 导出包是 zip 且含三件）；
> 下面第 1–16 条是**前端 + 真机**的那一半。

**A. 入口与布局（放在 AI 插件的「更多设置」里）**

1. 打开聊天窗 → 点输入行上方的「更多设置」：页签顺序必须是
   **权限 / 工作区 / 工具 / 人格 / 安全 / 其他** —— 新增的「安全」在「人格」之后、「其他」之前
   （不是放进应用设置面板）。
2. 菜单宽度 420px（原 320px，+100px）：六个页签在**一行**内放下，不换行、不出现横向滚动；
   窗口缩到 900px 宽再开一次，仍是一行。
3. 点「安全」⇒ 账本块立刻出现（打开菜单 / 切到该页都会自动拉一次），空账本显示「还没有记录」；
   五种语言各切一次：页签与整页文案都来自 i18n，**界面上没有裸中文**。
4. 列表铺满 200 行后，**内层不出现滚动条**：滚动只由 `.chat-more-pages` 一层承担
   （`forbidden.md` §禁用 2，禁止嵌套滚动）。

**B. 账本落点与字段（NDJSON）**

5. 先制造一次「自动放行」（只读工具）和一次「弹卡确认」，再看
   `<exe 根>\ModuleData\security\decisions-<UTC 日期>.ndjson`：每行一个 JSON，字段齐
   `v / ts / session / actor / tool / target / decision / reasonCode / reasonText / policySource / scope / evidence / reversible`。
6. 字段抽查：`v=1`；`ts` 是**宿主**写的 UTC（`…Z`，前端传的会被忽略）；`actor=agent`；
   `policySource=classifyRequest`；`decision ∈ auto / ask / allow / deny`；
   `reversible` 只对 `Write / Edit / NotebookEdit` 为 `true`；`scope` 是当前工作区（无工作区时 `system`）。
7. 点「刷新」⇒ 新行在**最上面**（宿主倒序返回，界面取最近 200 条；宿主上限 500）。
8. 重启 lunac ⇒ 昨天的分片仍在。再造一个 15 天前的分片、一个 33 MiB 的分片，
   然后**触发一次新的判定**（清理挂在追加时，不是打开页面时）：15 天前的被删，
   超 32 MiB 的那条改写到 `-2` 分片（**不丢证据**；总量超过 200 MiB 时从最旧的开始删）。

**C. 原因码抽查**

9. 逐条造场景，`reasonCode` 应分别是：
   - 只读工具自动放行 ⇒ `AUTO_READONLY`；自动档下的普通调用 ⇒ `RUN_MODE_AUTO`；
   - 工具白名单 / 命令前缀放行 ⇒ `WHITELIST_TOOL` / `WHITELIST_CMD`（`evidence.detail` 是对应工具名 / 命令首词）；
   - 命中危险规则 ⇒ `DANGER_RULE`；静态判不出来 ⇒ `OPAQUE_COMMAND`；参数里带凭据 ⇒ `SECRET_IN_PAYLOAD`；
   - 普通确认 ⇒ `ASK_DEFAULT`；计划卡 ⇒ `PLAN_APPROVAL`；提问卡 ⇒ `ASK_QUESTION`；
   - 用户在卡上作答 ⇒ 再各记一条 `USER_ALLOW` / `USER_ALWAYS` / `USER_DENY`（同一 `tool`，`decision` 为 `allow` / `deny`）。
10. 契约里另有 `PLAN_MODE_BLOCK / WORKSPACE_LOCK / PROTECTED_DIR / ENGINE_TIMEOUT / ENGINE_OOM /
    MXC_* / BACKEND_UNAVAILABLE` 这几组码，W0 **还没有产生点**（接 MXC 的 W1 才有）。
    手工往 ndjson 里写一条 `MXC_FILE_DENY` 应能显示出来，文案暂时走 `security.reason.other` 兜底 ——
    这是 W0 的**已知缺口**，W1 补齐文案与产生点，验收时别把它当成新 bug 反复查。
    另外 `reasonCode` 只做字符集校验（`A-Z / 0-9 / _`）与 `decision` 枚举校验，**不做白名单比对**：
    小写或带空格的码会被拒，但拼错的大写码会照写（导出 `meta.json` 里的 `reasonCodes` 才是清单）。

**D. 导出包**

11. 点「导出」⇒ 消息行显示 zip 路径；包内恰三件：
    - `meta.json`：`schemaVersion / generatedAt / appVersion / os / osBuild`（读注册表 `CurrentBuildNumber`）
      + `ledger { parts, truncatedAt, keepDays }` + `reasonCodes`（23 个码原样）；
    - `policy.json`：`runMode / workspace / whitelistTools / whitelistCommands`（**只有数量**，不含白名单正文）；
    - `decisions.ndjson`：最多 10 000 行（被截时 `meta.ledger.truncatedAt = 10000`），与磁盘分片同构。
12. 落点固定为 `<exe 根>\ModuleData\security\exports\security-<UTC 时间戳>.zip`；导出失败时界面只给一句
    可行动提示，原始错误进 `temp\logs\lunac-*.log`（`forbidden.md` §禁用 6）。

**E. 脱敏反证（硬要求）**

13. 造一条带 `Bearer sk-live-1234567890`、`password=hunter2`、`ghp_aaaa…` 的命令：
    - 列表里的 `target` / `reasonText` 看不到原文；
    - ndjson 同一行已被 `mask_secrets` 替换（`Bearer ***` / `password=***` / `ghp_***`，`sk-` 只留前缀）；
    - 导出 zip 解出来的两个文件也搜不到原文（`Select-String` 0 命中）。
    脱敏在**写盘前**做：先写后掩等于没掩。

**F. 预演不执行**

14. 预演框输入 `echo hi > %TEMP%\lunac-preview-probe.txt` 回车 ⇒ 结果行是 `判定 · 原因码 · 文案`，
    但 `%TEMP%\lunac-preview-probe.txt` **不会被创建**；输入 `rm -rf /` ⇒ `DANGER_RULE`。
    预演**不写账本、不弹审批卡、不动工作区锁**（入口只调 `classifyRequest`）。
15. 预演是纯规则判定（不调模型，`analysis` 传 `undefined`）：同一命令先预演、再在对话里实跑，
    `reasonCode` 应一致；若实跑变成 `AUTO_READONLY`，说明那次走了 agent 的只读分析，属预期差异。

**G. 审批卡「为什么问我」**

16. 任何需要确认的卡，操作列表下方多一行灰字「为什么问我：<原因>」；点开卡里的命令明细，
    应与账本中同一 `tool` 的那行 `reasonCode` 对得上。批量按钮（全部允许 / 全部拒绝）只在**纯工具审批**
    的批次出现：批次里只要混入计划卡或提问卡（要求用户作答），整批的批量按钮就收起，
    提问卡本身**永远没有**批量按钮（§待办 3-26 之后的口径）。

### v2 移植（2026-11-15，`docs/todo.md` M5z-补23）

> 自动化已全绿：`cargo test --lib security_ledger` ⇒ **11 passed**；
> `cargo test --bin agent analyze_json` ⇒ 1 passed；
> `flutter test test/security_ledger_test.dart` ⇒ **7 passed**；`flutter test` ⇒ **255 passed**；
> `flutter analyze` 0 issue。下面是**口子变化**与仍需真机跑的那几条。

- **落点与字段完全同口径**：`<exe 根>\ModuleData\security\decisions-YYYYMMDD.ndjson`，
  13 个字段，`v=1`，`ts` 由宿主生成（UTC、`…Z`、前端传的忽略），`actor=agent`；
  `policySource` 在 v2 里是 `auto_approval.dart`（v1 是 `classifyRequest`）。
- **原因码只做枚举校验**：未知码 / 小写码直接拒写（v1 只查字符集）。因此 §C 第 10 条
  「手写一条 `MXC_FILE_DENY` 应能显示」在 v2 里**仍然成立**（它是合法码），但拼错的大写码
  不会再出现在账本里。
- **导出是单个自包含 JSON**（不是 zip）：`exports\lunac-security-<UTC>.json`，字段
  `entries / policy / appVersion / osBuild / exportedAt / days / totalDays`。
  §D 第 11 条按这个形状验；「不含凭据」的判据不变（`Select-String` 原文 0 命中）。
- **超 32 MiB 丢最旧行**，不再换 `-2` 分片；14 天 / 200 MiB 两条上限不变。
- **没有「关掉白箱开关」这一档**：账本只在每次工具判定时追加一行，无常驻进程 / 定时器；
  要停就删 `ModuleData\security`。
- **没有 i18n**：v2 界面全局中文直写，§A 第 3 条「五种语言」不适用。
- **没有用户白名单**：v2 不会产出 `WHITELIST_TOOL / WHITELIST_CMD / USER_ALWAYS`
  （「始终允许」按钮未移植），导出 policy 里这两个计数写 0。
- **预演**：v2 调 `agent.exe --analyze <命令>`（只分析、不执行；`CREATE_NO_WINDOW`、3 秒超时），
  再走与真实审批相同的 `describeBoundary()` ⇒ 输入 `rm -rf /` 应显示 `DANGER_RULE`，
  `git status` 在白名单档应显示 `AUTO_READONLY`。agent.exe 缺失 / 超时时只按运行方式判，
  并在结果下方写明「静态分析不可用（原因）」—— 不允许静默当成「无风险」。
  §F 第 14 条现在可以照 v1 的预期验（危险命令应给出 `DANGER_RULE`）。


## §测试 29 代理插件三段式面板（§待办 3-27，2026-11-01）

> 自动化部分：`cargo test --bin lunac native_proxy::clash` ⇒ 18 passed（含新增
> `custom_link_round_trips_for_every_supported_kind` / `custom_link_rejects_bad_input`）；
> `cargo test --bin lunac` ⇒ 411 passed；`npx tsc --noEmit` exit 0；
> `build-plugins.ps1` ⇒ `proxy-0.10.0`（清单里带 `window 1280×720 / min 960×600`）。
> 下面 1–17 是**真机 + 观感**的那一半。

**A. 窗口与三段布局**

1. 从搜索打开代理插件 ⇒ **独立窗**，初始尺寸 1280×720；手动缩到 960×600 以下应被窗口下限挡住。
   清单反证：`Modules\proxy\lunac-plugin.json` 的 `window.width/height` = 1280/720。
2. 竖直三段可见：上工具区一行（服务器导入 / 服务器订阅更新 / 设置 + 右侧状态），
   中区是玻璃面板（圆形按钮居中），下区是请求日志；**页面本身不出现滚动条**（抽屉与日志各自滚）。
3. 界面里搜 `mihomo` / `clash` / `下载内核` ⇒ 三处都搜不到（只有旧配置提示那一句会在
   `engine=mihomo` 时出现，属如实边界）。

**B. 工具区（下拉菜单 + 界面内面板）**

4. 点「服务器导入 ▾」⇒ 菜单出现两项；点「自定义服务器」⇒ 界面内弹出面板（不是新窗口），
   含 名称 / 地址 / 端口 / 协议 / 密码或 UUID；协议选 `ss` 时才出现加密方式；`vless` / `vmess`
   的凭据框是 UUID，`tuic` 同时出现 UUID 与密码两个框（可选项：`ss` / `trojan` / `vless` /
   `vmess` / `anytls` / `hysteria2` / `tuic`）。协议是 `trojan` / `vless` / `vmess` 时多一行
   「mux.cool 多路复用」开关（勾上后生成的链接带 `mux=1`；其它协议不显示，因为 mux.cool 只对
   这三类生效）。
   填一条真节点保存 ⇒ 面板关闭，`config\proxy.json` 的 `links` 多一条链接，服务器抽屉里出现该节点。
5. 点「从剪贴板导入」⇒ 面板里先点「读取剪贴板」能看到剪贴板文本；点「导入」后提示
   「已导入 N 条，跳过 M 条」；`ssr://` 这类不支持的行计入跳过，不影响其它行入库。
6. 点「服务器订阅更新」⇒ 面板列出订阅（名称 + URL + 删除）；点「立即更新订阅」后提示
   「订阅已更新：N 个节点」（引擎运行中会提示先停用，属预期）。
7. 点「设置」⇒ 面板含：系统代理开关、混合端口、引擎（native）、宿主日志路径、
   规则模式（黑名单 / 白名单）与域名清单、**自定义规则多行框**（一行一条 Clash 规则，
   `#` / `//` 与空行忽略；支持 `DOMAIN` / `DOMAIN-SUFFIX` / `DOMAIN-KEYWORD` / `IP-CIDR` /
   `IP-CIDR6` / `PROCESS-NAME` / `GEOIP` / `MATCH`；`RULE-SET` 只在**订阅自带
   `rule-providers:`** 时生效，手写框里没有规则集定义，写了会如实跳过）；
   系统代理开关在引擎没跑时会先启动引擎再指向 `127.0.0.1:<port>`，关闭后系统代理还原。
   填 `DOMAIN-SUFFIX,example.com,DIRECT` 保存 → 重开服务 → 访问该域名，日志显示「直连」。

**C. 服务器抽屉**

8. 默认隐藏，只有左边缘竖排「服务器」拉手；点击后抽屉铺满**中间区**，
   每行显示：名称、地址:端口、传输（tcp / tls / ws / ws+tls / quic）、协议、来源订阅；
   右上「拉回」收回，拉手重新出现。
9. 引擎未运行时点服务器行 ⇒ 提示「先开启服务，再切换节点」，不发切换请求；
   开启后点另一行 ⇒ 提示「已切换到 X」，该行高亮，中央地区行随之刷新。

**D. 中央区**

10. 未运行时圆形按钮是「开启服务」；点击 ⇒ 状态行变「运行中 · 127.0.0.1:<port>」，按钮变「关闭服务」，
    速率开始每秒跳动；再点 ⇒ 关闭并还原系统代理。
11. 速率：打开一个下载页，↑ / ↓ 数字应明显非零且随流量回落；关闭服务后归零。
12. 地区行：运行后显示「国家 · 地区 · 城市 · IP　来源订阅 / 节点」；断网或节点不可用时显示
    「地区未知」（`mihomo_exit_info` 的 `ok=false`，不伪造）。
13. 模式：运行中切换 ⇒ 提示「先关闭服务，再切换模式」且配置不变；停止后切「全局代理」⇒
    提示「模式已保存」；下次启动 `mode` 生效（`global` 时本机直连规则也失效，属引擎语义）。
14. TUN 开关：打开 ⇒ **宿主如实拒绝**（「TUN 模式需要 wintun 驱动与用户态 TCP/IP 栈…」），
    开关自动回弹；`config\proxy.json` 的 `tun` 仍是 `false`。**不得**出现「已开启」的假状态。

**E. 日志区**

15. 运行中访问 2–3 个站点，日志每行应是「时间 · 目标域名:端口 · 走向」；域名与浏览器地址栏一致，
    走代理时显示「来源订阅 / 节点」，命中本机 / 私网时显示「直连」，被规则拒绝时显示「拒绝」。
16. 拖动日志顶部横条 ⇒ 高度可调（80–面板高度-140），松手后重开面板仍保持；点「收起」⇒ 只剩标题行，
    再点「展开」恢复。日志里**没有 URL 路径与正文**（只记域名 / 端口 / 走向）。

**F. 详细搜索的退场动画（2026-11-01 订正）**

17. 双击搜索栏进入详细搜索 → 点一条应用 / 文件 ⇒ 窗口**直接隐藏**，**不播**画卷拉缩动画
    （`ui-style.md` §界面 6 一直这么写，此前代码相反）。反证：简洁搜索里点结果仍应播 900ms 动画。

## §测试 30 附件同气泡 / 技能记忆 / 精选索引 / PTC（2026-11-19）

> 自动化面：app-flutter `cargo test --lib` 594 passed / 6 ignored；core-agent `cargo test
> --offline` 521 passed / 16 ignored；`flutter analyze` 0 issue；`flutter test` 314 passed。
> 下面是**安装版**的人工步骤（`release\Lunac-1.0.0-Setup.exe`）。

**A. 附件与正文同气泡（3-28.18）**

1. 对话输入框里打「先看 A，」→ 从预览窗「添加到对话」拖入 `a.pdf` → 胶囊出现在光标处；
   继续打「再看 B，」→ 再拖入 `b.pdf`。
2. 发送后用户气泡里两个胶囊**仍在原句位置**；复制气泡文本，得到的是
   `先看 A，[Attached file] "…\a.pdf" 再看 B，…` 这样的可读路径。
3. agent 侧 `history` 里两段 `[Attached file]` 的先后顺序与句子一致；正文里**不残留** `[[file:`。

**B. 技能记忆（3-28.10 ①）**

4. 设置 → AI → 「对话 → 更多设置 → 技能记忆」打开（保存后会重启 agent）。
5. 让模型用 `code-review` 技能（`context: fork`）做一次审查并让它 `Remember` 一条结论。
6. 设置 → AI → 技能 → 「技能记忆」出现 `code-review · N 字符`；「查看」能看到那条结论，
   全局记忆（`/memory`）**没有**它。点「清空」→ 二次确认后该行消失。
7. 关闭「技能记忆」再跑一次：子代理不再带笔记，`Remember` 回到全局作用域（对比第 6 步）。

**C. 精选索引（3-28.9 ①）**

8. 设置 → AI → 技能 →「精选（随包、按哈希核对）」六条全为「已安装 · <hash 前 12 位>」。
9. 删掉一个技能（如 `debug`）→ 该条变「未安装」→ 点「安装」→ 恢复且哈希一致。
10. 手改该技能的 `SKILL.md` 一个字 → 该条变「与精选版本不同」→ 点「恢复精选版」→ 覆盖后
    重新一致；「更多设置 → 安全 → 最近决策」里能看到对应的 `Skills.Install` 行。

**D. PTC / Code Mode（3-28.15）**

11. 设置 → 对话 → 「脚本批处理」打开（重启 agent）；`system/init` 的工具表里出现 `RunScript`。
12. 让模型「用 RunScript 一次读这 10 个文件并只列出含 TODO 的行」→ 审批卡上能看到完整脚本
    正文；允许后**只有 1 次模型往返**，返回里有 `N 次工具调用` + 逐条 `Read ok …ms … chars`。
13. 反证：脚本里写 `__lunacCall('Write', …)` ⇒ 调用失败、原因写「只绑定了只读工具」、
    目标文件**不存在**；写 `while(true){}` ⇒ 60 秒内被中断并如实报超时。

## §测试 31 任务大抽屉 / 统计上移 / 拖放粘贴 / 插件窗窗口键（2026-11-20）

> 自动化面：`flutter analyze` 0 issue；`flutter test` 314 passed。下面是**安装版**人工步骤。

**A. 对话流归类**
1. 提一个会跑工具的问题 → 过程区出现「任务 1」（14 号大字，比正文大一号）且**默认折叠**；
   进行中标题右侧有进度点，详细思考 / 工具调用都不自动展开。
2. 点开「任务 1」→ 里面是思考块 + 命令组 + 审批卡；命令等待审批时该抽屉**自动摊开**。
3. 一段思考之后的阶段小结**没有**「复制 / 分叉」；整轮结束后只有**最后一条**总结正文右下角
   有这两颗键，按「分叉」进新会话后原会话条数不变。
4. 对话顶端常驻一行「耗时 X · N 次失败 · 创建 N 个文件」（滚动时不动）；每轮末尾不再有信息条。

**B. 输入区与附件**
5. 输入框有可见描边；点进去边框变主题色，底色与面板底色不同；上下文水位条与用量表盘在**同一行**。
6. 从资源管理器拖一个文件到对话面板 → 输入框光标处出现附件胶囊。
7. 资源管理器里 Ctrl+C 复制文件 → 对话输入框 Ctrl+V → 出现胶囊；复制一段文字再 Ctrl+V →
   文字照常插到光标处（不出现胶囊）。

**C. 插件窗标题栏**
8. 打开音乐 / 代理 / 转换 / OCR / 桌宠 / 沙箱 / 插件控制台任一独立窗，鼠标移入**不再**弹出
   额外标题栏；「置顶 / 最小化 / 关闭」常驻各自标题栏**右上角**，且三条都能用。
9. 主窗里内嵌打开同一面板时，左侧仍是「返回」，Esc 仍能退出。
10. 连续开关 AI 窗 5 次（每次关前让它跑一个工具）+ 连续开关音乐窗 5 次，进程不退出
    （对应 `M5z-补81` 的关窗时先停定时器再销毁引擎）。

---

> **历史细节见 `docs/archive/code-rules.md`（完整预检清单 #1–#77）与 `docs/archive/ai-spec.md` §14 / §15 / §16。**


