# 待办事项（2026-11-20 重整版）

> **本文档是什么**：Lunac 当前**真正还要做**的事项。只保留可执行条目；旧版 793 KB 的
> 全过程记录、调研原文与已完成项已整体归档到
> [`archive/todo-2026-11-20-full.md`](./archive/todo-2026-11-20-full.md)，
> 需要取证时按旧编号（如 `§待办 3-25`）在归档文件里搜索。
>
> **重整原则**：
> 1. 已完成、已闭环、已验收的条目不再留在本文；细节在归档与 git 记录里。
> 2. 一条只写「现在要做什么 + 验收口径 + 落点」，不重复历史推理。
> 3. 优先级只按本文顺序；P0 不做完不排 P1。
> 4. 新规则仍然写入 `code-standards.md` / `forbidden.md` / `testing.md` / `logging.md` /
>    `ui-style.md`，不要写回归档。

---

## 0. 当前基线（2026-11-20）

- **主线**：v2（`app-flutter/`，Flutter + Rust cdylib）为当前开发主线；v1（`app/`，Tauri）
  冻结保留，只修阻断性缺陷。
- **共享内核**：`core-agent/`（agent）、`indexer/`、`tuning-engine/`、`app-flutter/rust/`。
- **最近完成的检查（2026-11-21）**：
  - `flutter analyze`：0 issue；`flutter test`：**320 passed**
  - `app-flutter/rust`：`cargo check --release` **0 warning**；`cargo test --lib`
    **605 passed / 0 failed / 7 ignored**
  - `core-agent`：`cargo test --offline` **509 passed / 0 failed / 16 ignored**，0 warning
  - `scripts\build-release-v2.ps1`：**exit 0、日志 0 warning**，产出
    `release\Lunac-1.0.0-Setup.exe`（约 21.6 MB），包内含 `agent.exe`、无 `cli.exe`
- **本轮已完成（2026-11-20）**：
  - 设置界面标题栏可拖动；
  - 历史回写保留用户提问、任务抽屉改用内容概括命名；
  - 新增图书馆式检索库（日期 → 对话 → 任务 → 操作，标签 + 优先级）；
  - 长期记忆接同一套索引，额度提高到 24000 / 16000 / 4000；
  - 清理全部 Rust 构建 warning；修复 `core-agent/src/main.rs` 的 U+FFFD 乱码；
  - `app-flutter/`、`indexer/`、`core-agent` 新模块、v2 脚本与文档已提交
    （commit `ac60e32`），不再只存在于工作副本；
  - 稳定两个环境依赖测试：ruleset 回环请求改 `.no_proxy()`，ConPTY 用例转
    `#[ignore]`（真机用 `--ignored` 跑）；
  - 新增 `scripts/verify-v2.ps1` 一键验收入口，并已实跑通过。
- **2026-11-21 优化收口**（详见 §6）：
  - 构建脚本加入 **0 warning 守门**（`-AllowWarnings` 可放行），core-agent 测试告警清零；
  - `chat_history` 改 SQLite（`ModuleData/history/chat.db`），历史列表改轻量元数据 +
    点开再读完整会话；
  - `library.db` 加 FTS5 trigram、体量统计、孤儿清理与超 32 MiB 的 `VACUUM`；
  - 记忆索引改稳定 id 差量 upsert，不再整 scope 重写；
  - 记忆注入加 `LUNAC_MEMORY_MODE=full|index` A/B 开关（默认 `full`），index 模式只注入
    2000 字目录并提示模型用 `MemorySearch` 取正文。

---

## 1. P0 阻断 / 项目卫生（先做）

### P0-1 安装包在干净机 / 卸载重装场景复验
- **现状**：`build-release-v2.ps1` 已能稳定产出安装包；上一轮 `M5z-补81` 的 release
  闪退只做过符号化定位，仍需真机连续开关插件窗 / 长跑复测。
- **要做**：干净 Windows 虚拟机装一次 → 启动 → 开关设置 / 聊天 / 音乐 / 桌宠插件窗 →
  卸载 → 确认 `ModuleData`、`config`、托盘、自启动项清理干净。
- **验收**：安装、运行、卸载三份记录；无残留进程、无启动项、无崩溃 dump。

### P0-2 Linux 构建与最小验收
- **现状**：v2 目标为 Windows + Linux 双端；Linux 侧只做过环境准备，未跑完整构建。
- **要做**：按 `flutter-migration.md` 的 M4/M6 在 Linux 上跑一次
  `flutter build linux` + `cargo test` + 启动搜索/聊天；记录 GTK / WebKitGTK / 托盘差异。
- **验收**：Linux 能出包并完成搜索、AI 对话、设置三项最小闭环；缺口登记回本文。

---

## 2. P1 用户已排期（按旧编号回溯）

> 详细背景在归档 `todo-2026-11-20-full.md` 的对应编号；这里只列仍要做的事。

### §1-1 Live2D 桌宠：市场安装端到端验收
- 从插件市场安装 `pet` → 导入模型 → 显示/交互 → 卸载；重打包 `lunac-installer.nsi`
  的 `SecExtPet` 段并复验。
- 旧编号：`§待办 1-1`。

### §1-2 成本归因：修复后再对一次账
- 用修复后的某天账单重新导出平台 CSV，与 `usage-*.jsonl` 对账，确认未完成回合已补齐。
- 旧编号：`§待办 1-2`。

### §1-3 应用自更新：两个版本闭环
- 先发一个 Release，再发第二个版本；验证检查 → 下载 → 校验 → 静默安装 → 自启；
  打开 `autoInstall` 时验证免打扰路径。
- 旧编号：`§待办 1-3`。

### §1-4 桌宠 × AI 剩余项
- 复核模型目录读禁、语音输入、主动反应节流在 v2 的实际接线；未接的补到 v2。
- 旧编号：`§待办 1-4`。

### §1-5 调音 / 音乐剩余
- P6「全局生效」（虚拟声卡采集 → DSP → 回真实设备 + 双模式开关）；
- S4 AI 调参（确定性拟合在前，AI 输出必须过本地关口）；
- 在线音乐与本地文件共用同一条链的剩余发布项。
- 旧编号：`§待办 1-5`。

### §1-6 长期记忆：触发门槛与自动抽取的最终复核
- 代码侧已实现会话/时间触发与空记忆冷启动修复；还需确认 v2 的写入路径真的会触发，
  并观察新 24000 额度下的注入体积与命中率。
- 旧编号：`§待办 1-6`、`§待办 3-9`。

### §1-7 至 §1-13 对话流 / 前端改进
- `1-7` 重要判断必须在可见正文给一句结论；
- `1-8` 思考过程显示本轮改了哪些文件；
- `1-9` 前端样式改动的前后对比图；
- `1-10` 弹「问用户」卡前先输出可见总结；
- `1-11` 回合信息条按工具类别统计 + 可折叠明细；
- `1-12` 历史记录里 PowerShell/Cmd 卡片展不开（根因已定位）；
- `1-13` 默认工具集合并/精简调研。
- 注意：其中一部分 v1 已实现，v2 需要逐项核对是否仍有同等能力；旧编号见归档。

### §1-14 / §1-15 Linux 迁移与迁移期纪律
- 迁移实施已在 P0-4；迁移期「暂停 GitHub 维护 + 备份 + 文档归档」继续有效。
- 恢复对外发布前先由用户确认。

### §1-16 至 §1-19 对话流架构与输出质量
- `1-16` 对话流八条纪律落成 agent 硬约束；
- `1-17` 回合控制 + 显示状态机 + 会话状态（v2 已重做一部分，仍需对照）；
- `1-18` 桌宠 TTS 商用友好候选（audio.cpp / Qwen3-TTS）可行性；
- `1-19` 正文轻量 markdown 渲染与输出可读性（v2 已有渲染器，需补差异）。
- 旧编号见归档。

---

## 3. P2 按需重估（不做只因优先级）

### Agent / 对话
- `3-1` Agent 能力缺口剩余项（多数已落地，逐项对照工具表与 MCP 读侧）。
- `3-3` 上下文感知提示词注入：工作区 / 打开插件 / 技能目录的 v2 版本。
- `3-6` 本地「只参与决策」模型：重要节点审查 / 策展（阶段 0/1）。
- `3-22` S3 多 agent 团队：常驻角色 + 任务 DAG（分片 1-3 已实现，剩余分片）。
- `3-23` S4 持续自治 + IDE 附着：无任务休眠 / 有任务唤醒。
- `3-28` Agent 对话流 / tools / skills 对标补全（P0/P1/P2 分批落地）。

### 记忆 / 检索
- `3-24` 记忆 wiki 化：多页、互链、自愈、去矛盾、定时维护。
- 验证「只注入记忆索引 + 模型按需 `MemorySearch`」能否在 24k 额度下继续降上下文：
  开关与步骤见 §6-3，等真实会话对比结果。

### 代理 / 网络
- `3-25` 自研代理引擎：按 `acceptance-map.md` 的缺口表继续补协议。
- `3-26` 内核收口：自研为默认唯一内核，mihomo 只保留只读兼容。
- `3-27` 代理插件三段式面板的剩余交互与日志目标域名。
- `3-29` 自研引擎能力缺口清单（P0 做完再动 P1）。

### 插件 / 界面
- `3-10` 插件芯片 / 单信道方案剩余施工。
- `3-11` 插件面板「图片化 / 可视化设计 → 代码」可行性（仅调研，不施工）。
- `1-5` 音乐插件与调音页的剩余发布项。

### 沙箱 / 安全
- `3-2` 真沙箱调研结论已出；若继续，按 `sandbox-research.md` 的 W0-W3 推进。
- 新增：把 `security_ledger` / 审批原因码纳入 v2 真机验收清单。

---

## 4. 已实现、只差真机验收

> 用户 2026-10-23 裁定：真机 / 浏览器实测类**暂不强制**；恢复时按本节逐条跑。

- `§待办 2` 原表全部条目（v1 侧）。
- `3-19` `ImageMeasure` / `ImageAnnotate`：用真实截图验测量与标注输出。
- `3-20` 加密图源封面：6 个来源的列表封面 / 书架封面实际加载。
- `M5z-补80` 对话流任务大抽屉 + 插件窗标题栏：连续开关插件窗、长对话滚动。
- `M5z-补81` release 闪退：连续开关插件窗 / 长跑复测；若复现，走「关窗不销毁引擎」方案。
- 本轮新增：设置标题栏拖动、历史回写、任务标题、检索库、记忆额度——在 v2 真机走一遍。

---

## 5. 长期方向（不排期；实施前先提升为 P1）

- `§待办 4` S0–S4：搜索溯源、无头 CLI、压缩语义化、本地决策、记忆层演进、
  多 agent 团队、持续自治 + IDE 附着。
- 记忆层演进：单文件 → 互链 wiki 目录，补自愈 / 去矛盾 / 定时维护。
- 评测标尺：LoCoMo / LongMemEval / BEAM（只作参照，不接云端评测台）。

---

## 6. 本轮优化（2026-11-21 已落地）

> 旧建议 1、2、4、5、6、7、8 已实现并有测试 / 构建取证；3 的开关已落地，剩余真实会话
> 对比（按下面步骤即可跑）。

1. **warning 守门已写进脚本**：`scripts/build-release-v2.ps1` 会收集 cargo / flutter
   输出，出现 `warning:` 直接失败；确需放行时传 `-AllowWarnings`。最近一次完整构建
   `exit 0`、日志 **0 warning**。
2. **检索库可观测性**：`library::stats` + FFI `lunac_library_stats`
   返回 `entries / sessions / tasks / dbBytes`；每次进程启动在日志里记一行
   `library: entries=… sessions=… tasks=… db=… bytes`。
3. **记忆注入 A/B（开关已落地，待真实会话对比）**：`LUNAC_MEMORY_MODE=full|index`
   （默认 `full`，行为与改造前逐字节一致）。
   - `full`：24k 整段注入。
   - `index`：只注入目录（页名 + 每条首行前 80 字，总长硬顶 2000 字），并提示模型
     在依赖记忆前调用 `MemorySearch` 取命中条目；老格式（没有 bullet）自动退回整段注入。
   - 宿主会把环境里的同名变量原样透传给 `agent.exe`；改完需重启应用 / agent。
   - 测试：`core-agent` 的 `memory_mode_switch_and_index_directory` 覆盖解析、目录压缩、
     预算截断、full/index 文案差异与固定前缀；`app-flutter/rust` 的
     `memory_mode_env_passthrough_skips_empty_values` 覆盖透传。

**A/B 操作步骤（真实会话）**

1. 先跑 `full`（默认，不设变量）N 个有代表性的会话，记录每次启动日志里的
   `长期记忆已注入（N 字）`。
2. `setx LUNAC_MEMORY_MODE index`（或从终端 `set LUNAC_MEMORY_MODE=index` 后启动
   `lunac.exe`），重启应用，再跑同类的 N 个会话。先确认工具编辑器里没有禁用
   `MemorySearch`（index 模式依赖它取正文）。
3. 对比三组数：
   - 上下文体积：日志里的注入字数（`记忆注入模式：index` 会额外打一行）；
   - 检索行为：`MemorySearch 命中 … 条` 的调用次数与 `reason=ok` 比例；
   - 答对率：同一批问题的人工判定（正确 / 漏答 / 编造）。
4. 结果写入本节下方表格；没有明显答对率下降就保留 `index`，否则回 `full`。

| 日期 | 模式 | 会话数 | 平均注入字数 | MemorySearch 调用 | 答对率 | 结论 |
|---|---|---|---|---|---|---|
| 2026-11-21 | full（合成 24k 语料） | — | 22817 | — | — | 单测基线 |
| 2026-11-21 | index（同上） | — | 2784（目录 2051） | — | — | 体积降到 12.2% |
| 待填 | full（真实会话） | | | | | |
| 待填 | index（真实会话） | | | | | |

> 合成语料那一行由 `core-agent` 单测
> `index_mode_shrinks_a_full_memory_block` 实测（`cargo test --offline … -- --nocapture`），
> 只证明上下文占用；答对率必须用真实会话按上面步骤补。
4. **`chat_history` 已改 SQLite**：`ModuleData/history/chat.db`，`sessions` 表按 id
   upsert，只写一条会话；旧 `chat-history.v2.json` 首次升级导入一次（有
   `legacy_imported` 标记，删掉的会话不会被导回）。
5. **检索库查询已走 FTS5**：`entries_fts` 用 `tokenize='trigram'`（CJK 可检索），
   由触发器增量维护；短查询（<3 字）继续走 `LIKE`。旧库首次打开自动 `rebuild`。
6. **记忆索引增量更新**：`replace_memory_scope` 按稳定 id（`scope:p<page>:o<op>`）
   只 upsert 当前条目、删除消失条目，单 scope 写入在一个事务里完成。
7. **`library.db` 生命周期**：`library::maintain` 清理孤儿索引 / 空会话，库超过
   32 MiB 时 `PRAGMA optimize; VACUUM;`；进程内只跑一次，失败只记日志。库文件损坏
   （不是合法 SQLite）时自动备份为 `library.db.corrupt-<时间戳>` 并重建，不让检索 /
   保存历史一直报错。
8. **Flutter 历史列表已分页/轻量化**：`lunac_chat_history_meta` 只返回
   `id / title / createdAt / updatedAt / messageCount`，点开某行才通过
   `lunac_chat_history_session` 读完整 payload；分叉只取标题也用元数据。

验收口径（最近实跑）：

- `powershell -ExecutionPolicy Bypass -File scripts\verify-v2.ps1` → 通过
- `flutter test`：**320 passed**
- `cargo test --lib`（`app-flutter/rust`）：**605 passed / 0 failed / 7 ignored**
- `cargo test --offline`（`core-agent`）：**509 passed / 0 failed / 16 ignored**，0 warning
- `scripts\build-release-v2.ps1`：**exit 0 / 0 warning**，产出
  `release\Lunac-1.0.0-Setup.exe`

---

## 7. 旧编号怎么查

- 旧版全文：[`archive/todo-2026-11-20-full.md`](./archive/todo-2026-11-20-full.md)
- 查法：在归档文件里搜索 `§待办 1-6`、`§待办 3-25` 等编号；本文只保留行动项，
  历史取证、实测数字、已废弃方案都在归档里。
