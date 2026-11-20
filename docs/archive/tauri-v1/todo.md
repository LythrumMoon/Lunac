# 待办事项（含调研输出总结）

> **本文档是什么**：Lunac **未完成 + 未验证**工作的唯一登记处，每条给出「目标 / 已查清的结论或调研摘要 / 待办点 / 涉及文件」。**已完成、已闭环的条目见 [`archive/agent-feature-backlog-history.md`](./archive/agent-feature-backlog-history.md)（不再保留逐条过程）。**
>
> **取代了哪些旧文档的哪些章节**：`agent-feature-backlog.md` 的「优先级总表」、§1（Agent 能力缺口）、§2（界面层待办）、§3（Lunac 自身新目标）、§4 设想区（含 §4.1 长期 AI 侧规划）、§5（边界声明）、§6（环境变量与数据根）中的**未完成项**。
>
> **维护纪律**：做完就删（结论落进 §11 规则 / 代码注释 / Git 记录，不由待办承载）；每条必须能落地（写不出落点的放 §待办 4 方向区）；优先级只按本文顺序。
>
> **历史细节见 `docs/archive/agent-feature-backlog.md`。**
>
> **真机验收路线图**：[`acceptance-roadmap.md`](./acceptance-roadmap.md) —— 全仓「已实现、只差真机验收」条目的批次、顺序与取证口径（本文只登记，怎么跑看它）。

---

## §待办 1 用户已排期 / 高优先

### §待办 1-0 删除一律「彻底删除」，不走回收站（2026-11-03，用户裁决，最高优先）
- **目标**：所有由 agent / 脚本发起的文件、目录删除，一律**永久删除**（不进回收站），避免「删了但空间没释放 + 额外磁盘写入」。
- **已查清（2026-11-03 实测教训）**：D 盘可用空间为 **0** 时，`cargo clean -p lunac` 明确删除 **7.3 GiB / 2857 个文件**，但 `Get-PSDrive D` 的 `Free` **仍然是 0** —— 删除被重定向进了**回收站**：空间没有释放，文件数据还要额外占用一份。随后 `cargo test` 直接以
  `failed to write query cache … 磁盘空间不足 (os error 112)` 失败。
  **结论：先前采用的删除动作（`cargo clean` 与工具删除）全部走了回收站，之后一律避免。**
- **待办点**：
  1. 清理构建产物 / 临时文件 / 日志时，改用**系统级永久删除**；不要再用会进回收站的命令。
  2. 删除入口统一收口到项目脚本（如 `scripts/clean.ps1`），要求：**先打印将删除的路径与总大小**（dry-run 默认），确认后再永久删除；不提供「回收站」模式。
  3. 当前执行环境对 `rm` / `del` / `Remove-Item` / `rd` 等命令有**硬性禁用**，且沙箱可能把删除重定向到回收站 ⇒ 若无法在允许的命令集内做到彻底删除，**如实告知用户并请求手动执行**（不要反复尝试同一命令）。
  4. 磁盘不足时优先清理构建产物（`target/`、临时 target 目录），清理前后都用 `(Get-PSDrive D).Free` 记录可用空间，作为「确实释放了」的证据。
- **涉及文件**：`scripts/`（新增清理脚本）、本文件与 [`code-standards.md`](./code-standards.md) 的执行约定。
- **验收**：清理前后可用空间数字确实增加（不是只看到「Removed N files」）；`cargo test` 不再报 `os error 112`。

### §待办 1-1 L1 Live2D 桌宠：最后一步端到端验收
- **目标**：让桌宠能从**插件市场**装一次、走完整链路。
- **已查清**：桌宠外壳、Live2D 引擎（`pixi.js@7` + `pixi-live2d-display-lipsyncpatch`）、Core 自托管副本（带版本 + sha256 + 授权文件）、`pet` 与市场索引均已发布（远端 `index.json` 含 `pet`）。除发布外全部真机验收过。
- **待办点**：从市场装一次、走完整链路；`lunac-installer.nsi` 的 `SecExtPet` 勾选段已加，未重打包验证。
- **打包侧复核（2026-10-27，只读 + 重跑暂存，未发版）**：
  - `build-plugins.ps1 -Plugin pet` exit 0 ⇒ `pet v0.9.65`，`release\plugin-packages\pet-0.9.65.zip`（373.8 KB）；
    同时刷新 `release\ext-plugins\pet\` 与 dev 的 `target\debug\Modules\pet\`。
  - 暂存清单 `release\ext-plugins\pet\lunac-plugin.json`：两条依赖（`file` → `engine/live2dcubismcore.min.js`、
    `archive` → `zipvoice`），**无 `local` 字段**（`local` 只在源表里、写清单时摘掉）；`provides` = `desktop.pet` / `tts.zipvoice`。
  - 暂存目录形状：`engine\` 四件（Core + `live2d-engine.js` + `LICENSE.md` + `RedistributableFiles.txt`）、
    `zipvoice\` **只有授权五份 + `THIRD_PARTY_NOTICES.md`**（216 MiB 运行库不在包内，符合「语音资产不内嵌」口径）。
  - NSI 侧对得上：`!cd ${__FILEDIR__}\..\release` 把 `ext-plugins\pet` 锚到 `release\ext-plugins\pet`；
    `MUI_PAGE_COMPONENTS` 在场、`Section /o "Desktop pet" SecExtPet` 为**默认不勾**；
    `!if /FileExists` 在缺暂存时整段跳过（不报错）。`build-release.ps1` 的**新鲜度守卫**会在
    `release\ext-plugins\` 比插件源码旧时当场 `throw`（不会把上一版静默打进包）。
  - **重打包验证已完成（2026-10-27）**：`build-release.ps1 -NoBump` exit 0（0.9.41，`Lunac-0.9.41-Setup.exe` 19.2 MB）。
    注意它**先被新鲜度守卫拦了一次**（`i18n.ts` / `styles.css` / `convert.ts` / `settings.ts` / `proxy.ts` / `music.ts` 比暂存旧）
    ⇒ 先跑全量 `build-plugins.ps1`（7 个插件全部重新暂存）才通过 —— 守卫按设计工作。
    makensis 报 **`7 sections (1 required)`** = 6 个插件勾选段（全部 `/o`）+ Uninstall，`SecExtPet` 确实进包；
    用 `7z l` 列包可见 `Modules\pet\` 下 `index.js` + `lunac-plugin.json` + `engine\`（Core / `live2d-engine.js` / 两份授权）
    + `zipvoice\{LICENSES\, THIRD_PARTY_NOTICES.md}`，**无 216 MiB 语音运行库**，也**无 `Modules\local-model`**
    （后者本轮未被 NSI 声明，属既有口径：它的 2.33 GiB 依赖只走市场）。
  - **唯一剩下的**：从**插件市场**装一次、走完整链路（需要你点击；另外安装包里 `SecExtPet` 默认不勾，属既定口径）。
- **涉及文件**：`app/src/plugins/builtin/pet.ts`、`live2d.ts`、`live2d-engine.ts`、`scripts/build-plugins.ps1`、`scripts/lunac-installer.nsi`。

### §待办 1-2 L6 成本归因：用修复后的账单再验一次
- **目标**：验证「没跑完的回合」这条修复在实机（≥ 2026-10-06 的某天）也补齐。
- **已查清**：三条硬结论——① 「元 / 百万 token」不能当 KPI，改用「每次提问成本」；② 节假日全天按谷价；③ 本地账必须覆盖没跑完的回合（agent 逐请求 `usage_delta` + 前端三处兜底落账）。跨源基线已跑过（差额 0.000000、exit 0）。
- **待办点**：用**修复后**某天再导一次平台 CSV 复验。
- **涉及文件**：`app/src/main.ts`（`flushTurnUsage`）、`app/src/usage-cost.ts`、`scripts/reconcile-usage.ps1`、`core-agent/src/main.rs`。

### §待办 1-3 L8 应用自更新：实机闭环
- **目标**：验证「检查更新 → 下载 → 自动重启成新版」与 `autoInstall` 免打扰路径。
- **已查清**：链路已落地（`updater.rs` + 设置→常规更新区 + 启动后 5 秒静默检查 + `publish-release.ps1`）；「旧数据自动替换」缺的三条（认旧安装目录 / 装前停进程 / 装完自启）已补进 NSI。只做 sha256、无签名（信任根是 GitHub）。
- **待办点**：**先发一个 Release、再有第二个版本**才验得了；`autoInstall` 打开时的免打扰路径。
- **涉及文件**：`app/src-tauri/src/updater.rs`、`scripts/publish-release.ps1`、`scripts/lunac-installer.nsi`。

### §待办 1-4 L10 桌宠 × AI：剩余三条
- **目标**：桌宠的屏幕获取自动化 + AI 触发工具 + 目录保护。
- **已查清**：①②③ 的主干均落地（手动抓屏 `screenshot_monitor` + 「看一眼屏幕」；对话框 + 主动反应（默认关 + 间隔 + 每日上限 + 不可见即停）；用量来源 `source="pet"` + 成本面板来源下拉）。契约 = `pet-chat-send` / `pet-chat-reply`（take 语义、复用 `startAIChat`、不新开 API 通道）。
- **待办点**：③ **当前配置的模型目录禁止 AI 读取** —— **已实现（2026-10-13，只拦读；见下 §待办 1-4 补「模型目录读禁」）**；④ 桌宠轮「不调工具 / skills」 —— **已取消（2026-10-13，用户裁决）**（原拟「桌宠轮工具闸」）；⑤ 语音输入 —— **已实现（2026-10-22；见下「§待办 1-4 补 桌宠语音输入」）**，待真机验收。
- **涉及文件**：`app/src/plugins/builtin/pet.ts`、`app/src-tauri/src/screenshot.rs`、`core-agent/src/main.rs`。

#### §待办 1-4 补 桌宠语音输入（2026-10-22，已实现，待真机验收）

- **目标**：桌宠输入行点麦克风说一句 → 识别结果走**既有** `sendToAI()`（`pet-chat-send`）发起一轮。
  **不新开对话通道、不新造消息格式**（与「主动反应 / 抓屏」同一条「桌宠发起」链路）。
- **识别来源 = Windows 内置 `SpeechRecognizer`**（离线、本地、零模型下载、零新增依赖）：
  只启用既有 `windows` crate 的 `Media_SpeechRecognition` + `Win32_System_WinRT` 两个 feature。
- **链路**：
  - **宿主** `app/src-tauri/src/voice_input.rs`：一次一句。`RoInitialize(RO_INIT_MULTITHREADED)`
    （windows 0.58 没有 `initialize_mta()`，与 `run_ocr` 同一条坑）→ `SpeechRecognizer::new()`
    → `Timeouts` 设三个上限（首静音 6s / 句末静音 1.2s / 噪声 10s）→ `CompileConstraintsAsync`
    → `RecognizeAsync`；返回 `{ text, reason }`。
  - **命令** `recognize_speech`（`commands.rs`，`run_blocking`）→ `main.rs` 注册。**绝不在主线程等**
    （识别最长 ~20s，留在主线程就是窗口未响应）。
  - **前端** `pet.ts`：输入行多一颗麦克风按钮（`#pet-chat-mic`，30×30，形状 / 颜色照 §界面 14.1
    的图标按钮：按钮背景 + 按钮线条 + 描边走四类变量）；识别中挂 `.on` + 禁用；成功即
    `sendToAI(text)`，失败按 `reason` 给不同人话（没听清 / 没麦克风 / 没语言包 / 其他）。
  - **i18n**：`pet.voice_input` / `_listening` / `_no_speech` / `_no_mic` / `_no_language` / `_failed`（五语言）。
- **失败语义（如实分成四类，不合并成一句「失败」）**：
  `no-speech`（首静音超时 / 成功但空文本）/ `microphone-unavailable`（隐私设置禁用）/
  `language-not-supported`（系统没装语音识别语言包）/ 其余 `unknown` / `network`。
- **已知边界**：可用性取决于系统装了哪些**语音语言包**（Windows 设置 → 语言）；没装就不是「功能坏了」，
  而是返回 `language-not-supported`。**不做**云端识别兜底（本地优先 + 成本）。
- **涉及文件**：`app/src-tauri/src/voice_input.rs`（新增）、`commands.rs`、`main.rs`、`Cargo.toml`、
  `pet.ts`、`i18n.ts`、`scripts/build-plugins.ps1`（pet **0.9.64 → 0.9.65**）。
- **验证**：`cargo check --bins` exit 0；`cargo test --bins voice_input` **3 passed**；
  `npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（`pet 0.9.65`）。
- **真机验收口径**：见 `testing.md` §测试 15。

#### §待办 1-4 补 桌宠模型目录「禁止 AI 读取」+ `mask_secrets` 扩展（2026-10-13，本轮）

- **②1 桌宠模型目录：只拦读的「策略级路径闸」（端到端已实现，待真机验收）**
  - **链路四段**：① **前端** `app/src/plugins/builtin/pet.ts` 新增 `reportModelDir()`（把 `PetConfig.model` 的 **dirname** 上报宿主；空串 = 清除），在插件 `attach()`（启动同步一次）与「选模型 `pickShape`」「换模型（桌宠窗 `change`）」「取消配置 `#pet-clear`」处调用；② **宿主** `app/src-tauri/src/commands.rs` 新增命令 **`set_pet_model_dir`**（存进 `AppState.pet_model_dir`，**同一份真相源**，不造第二套存储），在 `start_cli_process` 注入 `LUNAC_DENY_READ_DIRS`（多路径 `;` 分隔、绝对路径、**空不注入**；`main.rs` 注册命令）；③ **agent** `core-agent/src/tools.rs` 的 `Ctx` 新增 `deny_read_dirs`（启动时经 `parse_deny_read_dirs` 解析一次，与既有 `LUNAC_WORKSPACE_LOCKED` 读取处同风格）；④ **读类闸** `guard_read()`：**先判 deny、deny 优先于 allow、独立于 `locked`**（未配工作区也拦），只用于 `read` / `glob` / `grep`。
  - **只拦读**：`Read` / `Glob` / `Grep` 命中读禁目录即拒；**写类不动**（`Write` / `Edit` / `NotebookEdit` / `SendUserFile` 仍走 `guard`，`Cmd` / `PowerShell` 本就不过闸）。
  - ⚠️ **诚实边界（不得称沙箱，见 `forbidden.md` §禁用 1.1）**：**拦不住 `Cmd` / `PowerShell`**（`tools.rs:1272-1273` 刻意不解析 shell 命令行）⇒ **shell 仍能读到**；**符号链接 / junction 可绕过**（词法比较不解析链接）。本闸定性 = **策略级路径闸**。
  - ⚠️ **改后需重启 agent 才生效**（env 只在 spawn 时注入），沿用「重启才生效」的既有纪律。
  - **路径归一与既有 `guard` 一致**：deny 命中走 `normalize()` + 组件前缀比较，与 `guard` / `path_is_within` **同源**（**不新造第二套路径归一**，见 `forbidden.md` §禁用 62）。
  - **单测（4 条，`core-agent/src/tools.rs`）**：`guard_denies_read_in_deny_list_even_when_unlocked`（未配工作区 + deny 命中 ⇒ 拒）、`guard_does_not_deny_writes_for_read_only_deny`（deny 只影响读类）、`guard_allow_when_dir_not_denied`、`parse_deny_read_dirs_ignores_empty_and_relative`（env 解析：空 / 多路径 / 相对路径丢弃）。
- **① `mask_secrets` 扩展（`app/src-tauri/src/log.rs`）**：在原三类（键值对 / `Bearer <token>` / 裸 `sk-`）之外补四类 —— **PEM 私钥多行块**（保留 `BEGIN`/`END` 两行、**中段每行内容换成 `***`**，**换行数与缩进形状不变**，含 RSA/EC/OPENSSH/ENCRYPTED 各种词形）、**裸 JWT**（`eyJ…` 三段 base64url 点分，不依赖前置键名）、**前缀型 token**（`ghp_`/`gho_`/`ghs_`/`ghu_`、`xoxb-`/`xoxp-`/`xoxa-`、`AIza`、`AKIA`/`ASIA`）、**裸连接串** `scheme://user:password@host`（只抹口令、保留 scheme/user/host 形状）；`SECRET_KEYS` 补 `passphrase` / `secret_key` / `client_secret` / `private_key` / `access_key` / `refresh_token` / `session_token`。
  - **为什么锚定形态**：该函数被**日志 / API 报文 / 记忆落盘**多处共用，误伤会把普通文本打成码 ⇒ 新模式一律**锚定可辨认形态**（前缀 / `://…@` 结构 / `eyJ` 头 / 行首 PEM 标记），并保留前缀便于辨认；普通词 `token`（后面不是取值符）一律不动。
  - **仍是纯字符串变换**（返回 `String`、**无 `Result`**、不新增失败路径）；**老三类逐字节不变**。
  - **单测（6 条，`log.rs` tests）**：`mask_hides_pem_private_key_body_keeps_begin_end` / `mask_hides_bare_jwt` / `mask_hides_prefixed_tokens` / `mask_hides_bare_connection_string` / `mask_keeps_multiline_bullet_shape` / `mask_keeps_plain_words_untouched`（反向：普通文本不误伤）。
- **涉及文件**：`app/src/plugins/builtin/pet.ts`、`app/src-tauri/src/commands.rs`、`app/src-tauri/src/main.rs`、`app/src-tauri/src/log.rs`、`core-agent/src/tools.rs`、`core-agent/src/main.rs`、`scripts/build-plugins.ps1`（pet **0.9.63 → 0.9.64**）。

#### §待办 1-4 补 桌宠轮「关掉 AI 插件后永久不达」修复（2026-10-11）

- **用户报障**：桌宠插件与 AI 插件都开着时**关掉 AI 插件**，桌宠那侧的消息「永远无法送达」；希望桌宠侧不依赖 AI 插件也能跑通自己那一轮。
- **根因（代码路径证据，release 实测坐实）**：桌宠那一轮由**聊天窗**（`plugin-chat`）里的 `startAgentChat` 执行（`app/src/main.ts:9972`）；它只在 `cliReady` 为真时真发，否则把这一轮塞进 `pendingTurns`，把「就绪」的置位托付给宿主事件 `cli-status:stdout`（`main.ts:8877-8893` / `9983-10019` / `326-335`）。而 `cli-status:stdout` **只在 agent 进程真被拉起时发一次**（`commands.rs:584`）。关掉 AI 聊天窗（`detachedCloseBtn` → `win.close()`，`main.ts:4666-4668`）时，宿主 `on_window_event` 对 `plugin-*` 只做 `note_destroyed`、**不杀 agent**（`main.rs:473-487`）⇒ `agent.exe` 仍在、`cli_bridge::is_running()` 为真。此后主窗收到桌宠消息会新建隐藏聊天窗（`main.ts:10915-10931` → `open_plugin_window(hidden)`，`plugin_window.rs:371-487`），新窗 `cliReady=false` 再调 `start_cli`，而宿主 `start_cli` 见「已在跑」**从不重播 `stdout`**（旧逻辑只发 `starting`；`start_cli_process` 提前返回，`commands.rs:351-353`）⇒ 新窗 `cliReady` 恒假 ⇒ `pendingTurns` 永不冲刷 ⇒ 桌宠永久停在「思考中」。
- **为何「一开始就没开 AI」不表现**：agent 懒启动，首轮由聊天窗真正 spawn ⇒ 有 `stdout` ⇒ 正常。差异 = **状态残留（孤儿 agent 进程）**，非纯缺失。
- **选定修法（乙：修断点，不采纳甲）**：甲（主窗直接跑该轮）会让主窗承担整套流式渲染、违背「主窗不跑聊天」的既定决策且改动面大 ⇒ 弃。乙 = `start_cli` 在「agent 已在跑」时**补发一次 `cli-status:stdout`**（就绪信号的单一真相源；语义正确：复用路径下 stdin 本就就绪）。保留「必须聊天窗执行」的设计，不新增并行通道、不改用量归因（仍 `source="pet"`）、不影响 `cancelInflightTurn` / `pet-chat-stop`。
- **已实现（2026-10-11）**：`app/src-tauri/src/commands.rs` 的 `start_cli`（`commands.rs:829-851`）。
- **复现与验收口径（release 实测，可复核）**：夹具 `temp/pet-e2e.mjs`（CDP 驱动，`--remote-debugging-port=9222`；须在**单个命令内**启动→驱动→观测，否则沙箱会回收 GUI 进程）。四条：① 桌宠首轮（agent 未起）→ PASS；② **两窗都开 → 关聊天窗 → 桌宠再发**（本次核心）→ 修复后 PASS、**修复前 FAIL**；③ 同窗普通聊天不受影响 → PASS；④ `pet-chat-stop` 中止 → PASS。判据 = 回复里的算术答案（避开提问文本假阳性）；并取证「关窗后 `get_status` 仍回 `ready`」（状态残留实锤）。
- **涉及文件**：`app/src-tauri/src/commands.rs`（唯一改动）、`app/src/main.ts`（未改，仅路径证据）、`temp/pet-e2e.mjs`（测试夹具，gitignore）。**未改** `pet.ts`（无需插件重打包 / 版本 +1）。

#### §待办 1-4 补 附 关窗时「在飞轮次」有收尾方 —— **已存在**（2026-10-11 复核，不留悬空待办）

- **结论**：这个行为**代码里早已实现**，**不需要另开一条修**。`temp/pet-e2e.mjs` 的 CASE3 对着**安装目录 `D:\Lunac`** 实测 PASS 坐实（桌宠收到「（本轮已中断）」+ 关窗后 agent 变 `idle`）。
- **实现位置与机制**：
  - **前端关窗裁决** = `app/src/main.ts:12508-12522` 的 `win.onCloseRequested`，**注册在 `IS_CHAT_WINDOW` 块内**（只有聊天独立窗有）。判据 `if (!isStreaming) return;` —— 空闲关窗原样放行（agent 保热上下文）；**有在飞轮次**才 `event.preventDefault()` → 收尾 → `win.destroy()` 真关。
  - **收尾复用** `cancelInflightTurn()`（`main.ts:10867-10889`）：`streamId++` + 清 `cliTextCallback`/`cliDoneCallback` + `isStreaming=false` + `setStreamingUI(false)` + `flushTurnUsage()` + `markFlyingCardsStopped()` + `replyPetTurn(reason)` + `invoke("stop_cli")`（`cliReady=false`、`agentThread=null`）+ 桌宠轮时 `switchThread("chat")` + `drainPendingTurns()`。
  - **桌宠收到的那句** = `replyPetTurn`（`main.ts:10848-10852`）→ `emitPetReply`（`main.ts:10841-10843`）→ `emitTo("plugin-pet","pet-chat-reply",{text})`；文案 = `t("pet.chat_reply_interrupted")` = 「（本轮已中断）」（`app/src/i18n.ts:1904-1910`）。
  - **宿主侧**：`app/src-tauri/src/main.rs:448-465` 的 `on_window_event` 对 `plugin-*` 窗的 `CloseRequested` **不做** `prevent_close`（「关掉 = 收进托盘」只属主窗，`main.rs:460-464`）⇒ 交给 Tauri 内建的 JS `onCloseRequested` 机制裁决（有 JS 监听且 `preventDefault` 即拦下）；真正落地靠 `win.destroy()`。
- **覆盖边界（如实记）**：
  - **只有聊天独立窗**（`plugin-chat`）注册了这条收尾。**主窗**关窗 = 收进托盘（不是收尾）；**桌宠窗 / 其他插件窗**关窗走 `Destroyed` 清理（`main.rs:467-488` 的 `mouse_hook`/`note_destroyed`/`music`/`plugin_sidecar`），与「在飞轮次收尾」无关。
  - **桌宠轮**：桌宠收到「（本轮已中断）」+ agent 停（`get_status` 变 `idle`）—— CASE3 两条均 PASS。
  - **普通聊天轮**：同样做完整前端清理 + `stop_cli`（`cancelInflightTurn` 是两条线程的共用出口）；但 `replyPetTurn` 仅在 `currentTurnFromPet` 为真时才回桌宠 ⇒ 普通轮**不向桌宠发消息**（符合预期，非缺口）。
  - **空闲关窗**：`isStreaming=false` ⇒ 直接放行、不杀 agent（CASE2 实测：窗口关掉、agent 仍 `ready`、桌宠无误收尾）。
- **涉及文件（仅路径证据，未改）**：`app/src/main.ts`（`onCloseRequested` / `cancelInflightTurn` / `replyPetTurn` / `emitPetReply`）、`app/src-tauri/src/main.rs`（`on_window_event` 分流）、`app/src/i18n.ts`（`pet.chat_reply_interrupted`）、`temp/pet-e2e.mjs`（CASE3 夹具，gitignore）。


#### §待办 1-4 补 桌宠 ZipVoice 语音资产分发 + 第三方授权声明（2026-10-12）

> **2026-10-23 复核订正（以 `scripts/build-plugins.ps1` 清单为准）**：下面 A / C 两条里写的 `sha256 = FD7476…`、体积 `156,380,798 B` 与 `local = deps\zipvoice\_stage\zipvoice` 都是 **2026-10-12 当时的档案 / 清单状态**，现已过时 —— ① 现行清单与远端档案为 **sha256 = `DE25D3192B9C6C34DF642F1FB58852ED4991E604AFA95754499627D8C03FFEE2`**、**156,392,382 B**（解压后仍是 369 文件 / 226,689,463 B，未变）；② `local` 字段已于 **2026-10-13 用户要求移除**（见 `build-plugins.ps1` 的「本依赖没有 local」注释），本机 dev 资产只手工铺 `Modules\pet\zipvoice\`。**2026-10-23 真机资产已按新档案重下并复验**（`assets_ready()` 同款头字节判据两处目录均通过）。

- **用户裁决（本轮前提）**：本项目是**个人项目、明确非商用**，在 GitHub 公开分发 ⇒ ZipVoice 权重为 CC-BY-NC **可接受**，但**必须把授权与 NC 限制讲清楚**（避免法律 / 商用纠纷）。
- **A 组装可分发 archive（产物在 `release\`，gitignore 不入库）**：暂存目录 `release\deps\zipvoice\_stage\zipvoice\` = 运行期布局（4 个 dll + `model\`）+ **补入 `vocos_24khz.onnx`**（原在 `_dl\`，运行期要求它在根）+ `LICENSES\` + `THIRD_PARTY_NOTICES.md`。**剔除 `sherpa-onnx-cxx-api.dll`**：`objdump -p` 实测 `sherpa-onnx-c-api.dll` 只 import `onnxruntime.dll`（不依赖 cxx-api），而 `sherpa-onnx-cxx-api.dll` 反向 import c-api ⇒ 本通路（只用 C API，见 `tts_zipvoice.rs`）用不到它。**`espeak-ng-data` 保留全语言**（不裁剪：裁剪 = 修改上游数据，会引入额外义务）。`C:\Program Files\7-Zip\7z.exe a -t7z -mx=9` 打出 `release\deps\zipvoice\zipvoice-1.13.8.7z`（内含**单层 `zipvoice\`**，安装器按 `flatten_single_top_dir` 摊平到 `dest`）。**体积 156,380,798 B（149.14 MiB）/ 369 文件 / 40 目录**；**sha256 = `FD7476381CC426E00EE5123A8DEBA51CB967094445FD511F874A90D0E8D198B1`**。解压后总量 226,689,463 B（216.2 MiB）< `MAX_DEP_EXTRACT_BYTES`（1 GiB）。
- **B 授权文件（仓库 `licenses\zipvoice\`，纯文本、取上游原文）**：`CC-BY-NC-4.0.txt`（creativecommons.org 官方 legalcode → **ZipVoice 权重**）、`Apache-2.0.txt`（apache.org → **sherpa-onnx**）、`ONNX-Runtime-MIT.txt`（microsoft/onnxruntime → **ONNX Runtime**）、`GPL-3.0.txt`（gnu.org → **espeak-ng-data**）、`Vocos-MIT.txt`（gemelo-ai/vocos → **vocos_24khz.onnx**）；`THIRD_PARTY_NOTICES.md`（组件 × 授权 × 来源 × 义务表 + 非商用声明 + ZipVoice 署名 + espeak-ng 源码获取指引）。**ZipVoice 权重授权原话**（HF `k2-fsa/ZipVoice` 模型卡）：「Our code is released under the Apache 2.0 License. The pre-trained model is licensed under the CC-BY-NC due to constraints from its training data (Emilia).」**Vocos 查证结论**：代码库 `gemelo-ai/vocos` LICENSE = MIT（Charactr Inc.），HF 模型卡 `charactr/vocos-mel-24khz` front-matter `license: mit` ⇒ **按 MIT 处理**（模型卡原话针对「仓库中的代码」；严格商用仍建议向权利人复核 —— 已在 notices 如实标注）。
- **C `scripts/build-plugins.ps1`**：给 `pet` 的 `dependencies` 加第二条 `archive` 依赖（`build-plugins.ps1:221-240`：`type="archive"` / `url=…/releases/download/zipvoice-1.13.8/zipvoice-1.13.8.7z` / `dest="zipvoice"` / `sha256=…FD7476…` / `local="deps\zipvoice\_stage\zipvoice"` / 6 条 `notices` 到 `zipvoice/LICENSES/*` + `zipvoice/THIRD_PARTY_NOTICES.md`）。**`dest` 语义依据**：`plugin_market.rs` 的 `install_archive_dependency` 用 `safe_join(plugin_dir, d.dest)`，而 `plugin_dir = plugins_dir()/<id> = Modules\pet` ⇒ `dest="zipvoice"` 解到 `Modules\pet\zipvoice\`，与 `tts_zipvoice.rs:271-282` 的 `zipvoice_dir()` / `assets_ready()` **逐字对齐**。`Stage-PluginDir` 增 `archive` 分支（`build-plugins.ps1:508`）：archive 的 sha256 校验的是**压缩包本身**（市场安装器负责），本机暂存只按 `local`（**已解开的目录**）整树拷到 `dest`，让 dev / 安装包目录与「市场装完」形状一致。**注释一致性**：`tts_zipvoice.rs:20` 的「由桌宠清单的 `dependencies`（`type:"archive"`）装入」在本轮**加了声明后才成立**，现与事实一致、**无需改动**（全文再无与资产落点不符的注释）。
- **D 声明（避免商用纠纷）**：① 仓库根 `README.md` 的「第三方组件与许可」章节**补齐（非重复）**：新增 sherpa-onnx(Apache-2.0) / ONNX Runtime(MIT) / ZipVoice(**CC BY-NC，非商用**) / eSpeak NG 数据(GPL-3.0) / Vocos(MIT) / Live2D Cubism Core(专有) 六行 + 「本项目为个人项目、明确非商用」段落 + 指向 `licenses\zipvoice\`、`licenses\live2d-cubism-core\` 的两条路径。② **插件侧**：`plugin_market.rs` 的 `PluginManifest`（`plugin_market.rs:189-243`）**无** `license` / `notices` 字段 ⇒ **schema 不支持**，故授权说明改由**插件目录内的 `zipvoice\LICENSES\` + `zipvoice\THIRD_PARTY_NOTICES.md`** 承载（由 `notices` 机制拷入，已实测落地于 dev 与 `release\ext-plugins`）。
- **验证（2026-10-12）**：`powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1`（仓库根）**exit 0**；实测 `app\src-tauri\target\debug\Modules\pet\zipvoice\` 下 **四个必需文件全在**（`sherpa-onnx-c-api.dll` / `model\encoder.int8.onnx` / `model\decoder.int8.onnx` / `vocos_24khz.onnx` ⇒ `assets_ready()` 为真），`LICENSES\`（5 份）+ `THIRD_PARTY_NOTICES.md` 亦到位。`npm run verify`（根）exit 0（BOM 自检 18 份通过）；`npx tsc --noEmit`（`app`）exit 0；`cargo test`（`app\src-tauri`）**213 passed / 0 failed / 2 ignored**（宿主 Rust 未改逻辑）。
- **待用户上传（本轮唯一阻断项）**：archive **必须由用户上传**到公开插件仓库 Release，**标签与文件名要逐字一致**，否则市场安装因 sha256/URL 对不上**整包拒装** —— `gh release create zipvoice-1.13.8 release\deps\zipvoice\zipvoice-1.13.8.7z -R LythrumMoon/lunac-plugins -t 'ZipVoice 语音资产 1.13.8'`。上传后 `pet` 清单的 `url`（`scripts/build-plugins.ps1:224`）即生效；若要市场播发，另跑 `publish-plugins.ps1`。
- **待裁定 / 需知晓**：① **LICENSE 冲突**：仓库根 `LICENSE` 为 **MIT**，但随插件分发 **CC-BY-NC** 权重（ZipVoice）与 **GPL-3.0** 数据（espeak-ng-data）—— 二者**不被 MIT 覆盖**；本轮**未擅改 LICENSE**，建议在 `LICENSE` / README 显式声明「随分发的第三方组件各自适用其自身许可，MIT 仅覆盖本项目原创代码」。② **Vocos 授权**：按上游模型卡 MIT 处理，但模型卡原话针对「代码」，严格商用建议复核（已标注）。③ **`espeak-ng-data` 体积 17,991,651 B（约 17 MB）**：本轮**保留全语言**；若需裁剪须另行评估「修改版」义务。④ 本轮**未 +1** `pet` 的 `version`（`build-plugins.ps1:168`，仍 0.9.53）：加依赖属插件变更，若要市场判为「有更新」需用户拍板是否 +1 并 `publish-plugins.ps1`。
- **涉及文件**：`scripts/build-plugins.ps1`、`README.md`、`licenses\zipvoice\`（新增 7 个纯文本）；产物（gitignore、不入库）：`release\deps\zipvoice\_stage\zipvoice\`、`release\deps\zipvoice\zipvoice-1.13.8.7z`。

#### §待办 1-4 补 中文 TTS 授权口径再标一笔：ZipVoice 权重 = CC-BY-NC-4.0 ⇒ **仅限非商用**（2026-10-13）

- **当前中文 TTS 通路（已定）**：**继续用 ZipVoice，不换引擎**（supertonic 已下架；`audio.cpp` / CosyVoice 仅作将来备选）。**授权口径**：**ZipVoice 预训练权重** —— 即上游包 `zipvoice-distill-int8-zh-en-emilia`、对应随 `licenses\zipvoice\` 分发的 `model\encoder.int8.onnx` / `model\decoder.int8.onnx` 等 —— = **CC-BY-NC-4.0（署名—非商业性使用）⇒ 仅限非商用**；**`vocos_24khz.onnx`（声码器）= MIT（可商用）**；同目录 **DLL / 运行时各按自身许可**（`sherpa-onnx-c-api.dll` = Apache-2.0、`onnxruntime.dll` = MIT、`espeak-ng-data` = GPL-3.0）。逐组件见 `licenses\zipvoice\THIRD_PARTY_NOTICES.md`（仓库已如实声明，本条只是**再标一笔**防忘）。
- **⚠️ 商用前必须换引擎**：ZipVoice 权重**不得用于任何商业目的**（售卖 / SaaS / 商业产品内嵌均属）；本项目当前口径 = **个人项目、明确非商用、仅在 GitHub 公开分发**（据此再分发该权重）。**Lunac 的 MIT 许可不覆盖该权重**。一旦涉及商用 ⇒ **必须先换掉中文 TTS 引擎 / 权重**（不能沿用当前 ZipVoice 权重）。
- **已评估的商用友好备选（将来要捡起来时从这条入手）**：**`audio.cpp`**（授权 = **Apache-2.0**，可商用）—— 含 **Qwen3-TTS Base / VoxCPM2**，支持**中文 + 参考音频克隆**，**RTF 0.39 @ RTX 5090**、**空载实测 ~10 MB**（轻量）。接入点 = **替换 / 扩充 `app/src-tauri/src/tts_zipvoice.rs` 那一处**（**不建多引擎注册表**）。另：CosyVoice 属同方向备选，仅作将来考虑。
- **依据来源（与日期）**：HF 模型卡 `k2-fsa/ZipVoice` 的 License 节原话「Our code is released under the Apache 2.0 License. The pre-trained model is licensed under the CC-BY-NC due to constraints from its training data (Emilia).」—— 即**权重 NC 的成因 = 训练数据 Emilia 的 CC-BY-NC 口径**；`licenses\zipvoice\THIRD_PARTY_NOTICES.md` §二 已逐字记录。本条日期 **2026-10-13**。
- **用途**：**避免以后忘记** —— 将来若要做商用或改变分发口径，**先回到本条**，核对「当前中文 TTS 权重是否已换成可商用引擎」。

#### §待办 1-4 补 桌宠 TTS 并存切换：Supertonic 通路 —— **已下架（2026-10-13，用户裁决）**

- **裁决 / 来源**：**用户 2026-10-13 亲耳确认**后裁决 —— **干净下架**上一阶段「阶段 1」的 Supertonic 通路**全部改动**（资产依赖 / 后端 `engine` 分支 / 前端引擎与音色控件 / i18n key / `licenses\supertonic\`）。
- **⚠️ 下架原因（保留结论）**：**Supertonic 官方 31 语言表不含中文（`zh`）** —— `lang="zh"` 会被 sherpa-onnx 直接拒绝（打印 `Invalid language: zh` 并返回空音频）；汉字文本虽能在 `lang="en"`（不传 `lang` 时内部缺省 `en`）下**出声**，但模型 `tts.json` 的 `n_langs=0` / `lang_emb_dim=0`（**无语言条件**）、文本被 `<en>…</en>` 包裹后按**裸字符 id** 送入 ⇒ **汉字走兜底发音，中文听感「按日文读」**（用户已亲耳确认）⇒ **中文场景直接弃用**。（原实测的「时长随字数近似线性、常用汉字均被 `unicode_indexer.bin` 索引、非静音」等数据仍为真，但**听感不达标是决定性的**。）
- **保留（与引擎无关、且已被实证必要）**：① **确定性 DLL 加载** —— `LoadLibraryExW(..., LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)`；理由：ZipVoice 同样依赖 `onnxruntime.dll`，走标准搜索序会命中 System32 的 1.17.1（低于 sherpa 1.13.8 要求的 1.26）⇒ `0xC0000005`。② 常驻诊断日志（`zipvoice: onnxruntime path=… version=…`）。
- **保留（外部）**：公开插件仓库的 Release 资产 **`supertonic-3-tts-int8-2026-05-11` 未删除**（无害，便于将来复活）。
- **ZipVoice 未受影响**：参考音频链路与用户新增的 **4 个参考音频单测**（`import_accepts_at_max_rejects_just_over` / `import_resamples_to_target_rate_preserving_duration` / `scale_guard_flags_oversized_reference` / `old_oversized_cache_is_blocked_before_engine`）**逐字保留**；参考音频**时长上限保持现状**（`REF_MAX_SECS = 10.0`，未改）。
- **版本处理**：`scripts/build-plugins.ps1` 的 pet 版本**保持 0.9.63**（0.9.63 从未发布，不回退以免与可能已构建的产物混淆）。
- **本轮回退涉及文件**：`app/src-tauri/src/tts_zipvoice.rs`、`app/src-tauri/src/main.rs`（命令注册）、`app/src/plugins/builtin/pet.ts`、`app/src/i18n.ts`、`scripts/build-plugins.ps1`（pet 依赖表 + 发布提示）；**删除** `licenses\supertonic\`（3 个纯文本）。
- **历史存档（原「阶段 1」要点，均已移除）**：Supertonic 3 **合成期不吃参考音频**（预置音色 `sid` 0–4=F1..F5 / 5–9=M1..M5 + 自建音色 `custom_voice`），语言经 `GenerationConfig.extra={"lang":…}` 传入 —— 本可**整类消除** ZipVoice 那条「192 kHz × 29.2 s 旧缓存触发 `0xc0000005`」的崩溃类；另含「音色 JSON → `voice.bin`」纯函数打包器 `pack_voice_bin` / 落盘 `write_supertonic_voice` 与新命令 `import_supertonic_voice`；`PetVoiceConfig` 的新字段 `engine` / `sid` / `lang` / `custom_voice` 亦一并移除（回到阶段 1 之前的结构；serde 默认忽略未知字段，老 `pet-voice.json` 仍可解析、无兼容问题）。
- **注**：`tts_zipvoice.rs` 里**仍保留** FFI 结构 `OfflineTtsSupertonicModelConfig` 及 `OfflineTtsModelConfig.supertonic` 字段（置零）—— 那是 sherpa-onnx 1.13.8 的 **ABI 布局要求**（少字段 ⇒ 结构偏移错位、`SherpaOnnxCreateOfflineTts` 崩溃），**非功能残留**。


#### §待办 1-4 补 桌宠「试听」闪退根因 + 打包「下载污染」闸门（2026-10-12）

- **用户报障**：桌宠设置里点「试听」→ 应用直接闪退。
- **① 根因（已复现、有运行时证据）**：`vocos_24khz.onnx` 被 gh-proxy 之类 GitHub 加速镜像的 **HTML 落地页**污染 —— 文件前 **4699 字节是 `<!DOCTYPE html>`**，真 ONNX 载荷从偏移 4699 开始；**总尺寸与上游完全相同（54,157,409 B）**，所以「只比大小」的校验把它漏过去了。ONNX Runtime 解析失败抛 `Protobuf parsing failed`（MSVC C++ 异常）→ sherpa-onnx C API 未接管 → **`0xe06d7363` 未被捕获** → 宿主被杀。
- **② 修复内容（2026-10-12 已完成）**：合法 `vocos_24khz.onnx`（sha256 `BCB3B970E384161C4D634F0BB9E999FF1C471B34C9BC0B1049A5014065ED3CC0`）已替换到 `release\deps\zipvoice\_dl`、`_stage\zipvoice`、`release\ext-plugins\pet\zipvoice`、dev `app\src-tauri\target\debug\Modules\pet\zipvoice`；`.7z` 已重打（新 sha256 `DE25D3192B9C6C34DF642F1FB58852ED4991E604AFA95754499627D8C03FFEE2`，size 156,392,382）；`scripts\build-plugins.ps1` 的 pet 依赖 `sha256` 已同步为新值。
- **③ 新增闸门（`scripts\build-plugins.ps1`，在把依赖 `local` 拷进插件目录**之前**跑）**：入口 `Assert-DepInputClean`（`:514`，调用点 `:580`，唯一拷贝入口 `Stage-PluginDir` 内）——
  - **拦什么·通用 HTML 污染**：对依赖 `local`（`file`=单文件、`archive`=目录**递归**）只读每个文件**前 512 字节**（`Read-FileHeadText` `:492`），命中 `^\s*<!DOCTYPE` / `^\s*<html`（大小写不敏感、容忍 BOM/空白）即 `throw`；信息含**文件路径 + 该文件首 40 字节 + 一句诊断**（「疑似被 GitHub 加速镜像/gh-proxy 的 HTML 落地页污染：文件总尺寸可能与上游相同但内容不是原文件；请核对 sha256 或更换下载通道」）。
  - **拦什么·ZipVoice 必需文件**：`sherpa-onnx-c-api.dll` / `model\encoder.int8.onnx` / `model\decoder.int8.onnx` / `vocos_24khz.onnx` 存在且 `Length > 0`（**仅** pet 的 archive 依赖，直白断言、不做通用框架）。
  - **拦什么·vocos 钉死 sha256**（`BCB3B970…`，`:546`）—— ⚠️ **换模型/换版本时必须同步更新此常量**。
- **④ 验证**：阴性（**仓库之外**造污染副本、改写头几字节为 `<!DOCTYPE html>` 且**总尺寸不变**，把闸门指向它）⇒ 必 `throw`（贴首 40 字节 `<!DOCTYPE html>..<html lang="en">..`）；阳性（真实 `_stage\zipvoice`）⇒ 通过；完整 `powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1`（仓库根）**exit 0**；`npm run verify` **exit 0**（`[8/8]` BOM 自检 18 份通过）。
- **⚠️ BOM 陷阱（记一笔）**：编辑 `.ps1` 的工具可能**剥掉 UTF-8 BOM**，PS 5.1 下无 BOM 的 `.ps1` 会按 GBK 解码中文而**解析失败**（连引号一起吃掉）。改完必须确认该文件**恰好一份** `EF BB BF`（第 4 字节不是 `EF`），并跑 `npm run verify`（其 `[8/8]` 就是 BOM 自检）——见 `testing.md` §测试 3。
- **清理（2026-10-12）**：删 `release\deps\zipvoice\zipvoice\`（**无任何脚本引用的遗留副本**：缺 `vocos_24khz.onnx` 与 `LICENSES\`，多一个 `sherpa-onnx-cxx-api.dll`）**164.7 MB**；清 `%TEMP%` 调试残留 `zipvoice-repro`（**96 MB**；`zipvoice-redl` / `voctest` / `good` / `out-veh*.txt` 经查本就不存在）；自测临时件 `zipvoice-gate-neg`（51.6 MB）与 harness 一并清除。**释放合计约 312 MB**（仓库 164.7 + temp 147.6）。**保留** `_dl`（163.4 MB）/ `_stage`（216.2 MB）/ `_work`（166 MB）/ `zipvoice-1.13.8.7z`（149.1 MB）。`release\` 本就 gitignore，`git status` **无任何 `release/` 条目**（大件未入库）。
- **⑤ ⚠️ 遗留（需用户操作）**：**必须重新上传**新的 `zipvoice-1.13.8.7z` —— 标签 **`zipvoice-1.13.8`**、文件名 **`zipvoice-1.13.8.7z`** **不变，但内容与 sha256 已变**；传旧包会因 sha256 不符**整包拒装**。命令：`gh release create zipvoice-1.13.8 release\deps\zipvoice\zipvoice-1.13.8.7z -R LythrumMoon/lunac-plugins -t 'ZipVoice 语音资产 1.13.8'`（若该 tag/Release 已存在，须先删旧 tag 与 Release 再传，否则 `create` 失败）。
- **涉及文件**：`scripts/build-plugins.ps1`（新增 `Read-FileHeadText` / `Get-FileHeadPreview` / `Assert-DepInputClean` + 调用点）；产物（gitignore、不入库）：`release\deps\zipvoice\`。


#### §待办 1-4 补 桌宠模型参数：写入层夹取 + 物理驱动标注 + 档位选择器 + 预设按钮化（2026-10-12）

- **⚠️ 已按用户要求回退（2026-10-12）：本批属误解 —— 用户原意是「查证模型 `.vtube`/`.cdi3`/`.physics3` 三个文件里那些参数能否正确改模型、范围是否为 `-1..1`」，而非「加档位 / 预设按钮把参数固定到某值」。**代码已退回本批之前：写入层夹取（`clampParamValue` / `resolveParamRange` / `MountOptions.paramRangeFallback`）、档位选择器（`pet-gear` / `pet-gears` / `data-param-gear` / `data-gears` / `isGearParam` / `GEAR_MAX_SPAN` / `gearValues` / `restoreParams` 回填高亮 / 点击分支 / CSS）、物理驱动标注（`.pet-phys-note` / `pet.debug_phys_driven_note` / `Cdi3Names.physicsDriven` / `cdi3PhysicsParamIds`）均已删除；新增文件 `app/src/plugins/builtin/pet-param-logic.ts` 与 `scripts/pet-param-logic-selftest.mjs` 已整体删除；`pet-hotkeys.ts` 恢复为原私有 `physicsParams`（`physicsParamIds` 已被移除）。以下原文**保留**，仅作历史记录。**现状（2026-10-17 核对）**：仓库里**不存在** `clampParamValue` / `resolveParamRange` / `pet-param-logic.ts` / `scripts/pet-param-logic-selftest.mjs`，下面 A–D 均为**历史 / 计划**，不是当前实现。

- **历史 / 计划（现状未实现，见上一段的核对）** —— **当时用户裁决（当时按此实现，后整体回退）**：① **范围口径以模型 moc3（Cubism Core 运行期）为准**，**不引入** `.vtube.json` 的 `InputRange/OutputRange`（那是 VTS 输入→输出映射，实测更窄）；② **物理驱动参数保留可控 + 加标注 + 补夹取**；③ **离散档位参数**（如发型的档位那种）**做成档位选择器**，不是裸滑条；④ **模型自带的表情预设 → 复用现有热键 / 表情机制做成可点按钮**。
- **⚠️ 法规纪律（硬，遵守裁决 5）**：只允许**运行期读用户本机模型目录里的文件**；本轮**没有**把任何具体模型的参数 Id / 名称 / 数值写进仓库代码或文档 —— 读取逻辑**只认公开 schema 的字段名**（`PhysicsSettings[].Output[].Destination`、`ParameterGroups[].Name/Parameters`），档位数 / 范围**一律运行时从 Core 或模型文件取**。
- **新增纯逻辑模块 `app/src/plugins/builtin/pet-param-logic.ts`**（无 DOM / 无 tauri，可被 node 直接单测；`pet.ts` 与 `live2d-engine.ts` 各自内联）：`clampParamValue`（`:31`）、`resolveParamRange`（`:40`）、`isGearParam`（`:51`）、`gearValues`（`:60`）、`physicsParamIds`（`:79`，替换掉 `pet-hotkeys.ts` 里旧的私有 `physicsParams`，避免第二份实现）、`cdi3PhysicsParamIds`（`:103`）。
- **A 写入层范围夹取**（最关键，堵住「程序化写入可越界」）：
  - **唯一夹取点 = 引擎 `setParameter`**：`app/src/plugins/builtin/live2d-engine.ts:674-684` —— 写入前 `clampParamValue(value, rangeOf(id))`，`heldParams` 存**夹取后的值**（每帧写回自然也在范围内），再立即写 Core。**所有程序化写入路径都汇总到它**：设置窗滑条 / 数值框（`pet.ts` 的 `emitParamThrottled` → `set-param` 广播 → `pet.ts:4705`）、`reset-params`（`pet.ts:4711`）、热键 run / toggle（`pet.ts:4403` / `:4528`）、AI `param` 动作（`pet.ts:4595`，含 `PetControl` 越界值）、以及引擎内的**叠加表情层**（`live2d-engine.ts:540`）与**口型**（`:550`）。
  - **`add` 模式夹的是叠加后的结果**：调用方先 `getParameter + value` 算好和再进 `setParameter`（`pet.ts:4403`），夹取作用在**和**上。
  - **范围优先级（与现状一致）**：运行期 Core 的 `paramRanges[id]`（moc3 自己的 min/max）→ 缺失回落 `pet.ts` 的静态 `DEBUG_PARAMS`（经 `DEBUG_PARAM_RANGES`，`pet.ts:186`，随 `mountLive2D` 的 `paramRangeFallback` 传入，`pet.ts:3511` / `live2d.ts:306`）→ 再回落 `[-100,100]`（`DEFAULT_PARAM_RANGE`）。夹取**只发生在写入边界**，不改「保持 / 每帧压过物理」的既有语义（`applyHeld` 的写回次序不变）。
- **B 物理驱动参数：判定 + 保留可控 + 标注**：
  - **判定（2026-10-17 更正为与代码一致）**：物理输入 / 输出参数的**唯一来源** = 模型 `*.physics3.json` 的 `PhysicsSettings[].Input[].Source.Id` / `[].Output[].Destination.Id`，统一由 `pet-hotkeys.ts` 的 `readPhysicsParams()` 解析（**`Id` 优先且只认 `Id`**；`Source.Target` / `Destination.Target` 只是**类型标记**，恒为 `"Parameter"`，**不得当作参数 id**；去重、保持出现顺序）。文件不存在 / 结构不认识 ⇒ 空集合 + 一条 warn，**不阻断挂载**。**现状没有** `.cdi3.json` 的 `ParameterGroups` 兜底 —— 上一版写的「兜底 cdi3、字段缺时认 `Target`」是**未实现的历史方案**，已作废。**这些参数仍可被滑条 / AI / 热键临时写入（钉住）**，但自 2026-10-17 起**不再持久化**（代码：`pet.ts` 的 `persistParam` 与挂载重放都跳过物理输出）。
  - **标注文案（i18n 五语言）**：新增 `pet.debug_phys_driven_note` =「物理驱动：拉动会暂时钉住摆动，松手恢复」，渲染在调试行参数名**下方**一行（`.pet-phys-note`，`pet.ts:1988-1990` / CSS `:3248`）。既有「物理输入 / 物理输出」小标签（`pet.debug_phys_in` / `pet.debug_phys_out`）**保留**。
  - **物理开关**：既有 `setPhysicsEnabled` 已经**有入口**（热键动作类型 `physics`，见 `pet-hotkeys.ts` 的 `ACTION_TYPES` / `pet.ts:4524`）⇒ 按「已有入口就别重复加」**未新增**按钮 / 开关（不扩范围做新框架）。
- **C 离散档位参数 → 档位选择器**：
  - **判定**（`pet-param-logic.ts:51`）：该参数在 **Core 里的范围**满足「`max-min` 为**小跨度整数**（`1 <= 跨度 <= GEAR_MAX_SPAN = 8`）且 min/max 均为整数」⇒ 视为档位参数；**开关形态优先**（先判 `isSwitch`）。
  - **渲染**（`pet.ts:1996-2018`）：把滑条换成 **N 个档位按钮**（值 = `min..max` 的整数，当前值高亮，`.pet-gear.on`）；点击分支 `pet.ts:2298-2314` → 高亮该档 + 同步隐藏 range（值容器）+ 走既有节流广播 `emitParamThrottled`；`restoreParams` 回填高亮（`pet.ts:1717-1725`）。
  - **⚠️ 档位数只能运行时从 Core 读**（`.cdi3.json` v3 不带范围）⇒ `isGear` 只看 `core.parameterRanges[id]`（`pet.ts:2099`）；**取不到范围 ⇒ 照旧渲染滑条**（不瞎猜档位数）。
- **D 预设（表情）按钮化 —— 结论：已存在，无需新增**：
  - **依据**：设置窗「热键」页**已经把导入的表情类热键列成可点按钮**（`pet.ts:1405` 的行尾状态按钮 `data-hk-run`，开 / 关文案 + `aria-pressed` 可见状态）→ 点击 `pet.ts:2743` 广播 `hotkey-toggle` → 桌宠窗 `toggleHotkey`（`pet.ts:4506-4513`）调**叠加表情层** `setExpressionLayer`（多条可同时生效、可单独取消，对齐 VTS 的 Expression on/off）。VTS 作者热键导入（`importHotkeysFromFolder` → `hotkeyFromVts`，`action.type="expression"`）本就把它们喂进这张表。⇒ 完全命中需求，**复用既有机制、未新造预设数据格式 / 未新增存储**。另：模型自带表情另有既有入口（调试区 `.pet-exprs` 的按钮，`renderExpressions`）。
- **验证（2026-10-12，逐条退出码）**：
  - `powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1`（仓库根）⇒ **exit 0**（首跑因 dev `lunac.exe` 锁着 `Modules\pet\zipvoice\onnxruntime.dll` 报 IOException ⇒ 按 `testing.md` §测试 1 先停 dev 实例后重跑通过）。dev 产物 `app\src-tauri\target\debug\Modules\pet\index.js` 经 `Select-String -SimpleMatch` 实测含新标识：`pet-gear`×6 / `data-param-gear`×5 / `data-gears`×3 / `pet-phys-note`×2 / `debug_phys_driven_note`×1 / `paramRangeFallback`×1；引擎产物 `Modules\pet\engine\live2d-engine.js` 含 `paramRangeFallback`（`clampParamValue` / `resolveParamRange` 被压缩改名，故按名字查不到 —— 已由纯逻辑单测 + 源码核对坐实）。**产物中不含任何来自具体模型的 Id / 名称**（新增代码只认公开 schema 字段，未写入任何模型数据）。
  - `npx tsc --noEmit`（`app`）⇒ **exit 0**；`npm run build`（`app`）⇒ **exit 0**；`npm run verify`（仓库根）⇒ **exit 0**（`[8/8]` BOM 自检 18 份通过；**本轮未改任何 `.ps1`**）。
  - **纯逻辑单测**：`node --experimental-strip-types scripts/pet-param-logic-selftest.mjs` ⇒ **exit 0 / 18 项全过**（本仓库**没有前端单测运行器** ⇒ 按 `testing.md` §测试 5 既有惯例用 node 自检脚本）。覆盖：① 夹取（范围内 / 下越界 / 上越界 / 范围缺失回落 `[-100,100]` / `add` 叠加后结果夹取 / `NaN`→0 与 `±Infinity`→边界）+ 范围优先级（运行期 > 兜底 > 默认）；② 档位判定（小跨度整数=档位 / 大跨度或非整数=不是 / 缺范围或零跨度=不是）+ 档位值含两端；③ 物理判定（physics3 输出集 / `Destination` 只有 `Id` 也认 / cdi3 分组名含「物理」兜底 / 结构不认识=空 / 双来源优先口径）。
- **真机验收口径（视觉 / 交互无法无头验，须起 dev 实例点一遍）**：
  1. **档位参数显示为按钮且点击生效**：调试区里档位参数（小跨度整数）呈一排档位按钮、当前档高亮，点另一档模型立刻变到该档。**档位数 = 运行期 Core 给出的 `min..max`**（不同模型不同）。
  2. **物理参数旁有标注且仍可控**：有物理输出的参数行标出「物理驱动…」；拉动该行滑条**形态立刻改变**（值被「保持」钉住，物理摆动暂被压过），**松手后不残留**（动作 / 物理从自己的基准恢复摆动）。
  3. **表情预设按钮开 / 关生效**：热键页里表情类热键的状态按钮点一下叠加、再点一下取消，模型表现对应变化、按钮开 / 关态可见。
  4. **任何时候都不越界**：让 AI 用 `PetControl` / 参数动作给一个**远越界**的值（如给 `[-1,1]` 的参数 `+999`）⇒ 形态停在该参数的量程端点（被夹住），**不会异常跳变 / 拉爆**。
- **涉及文件**：新增 `app/src/plugins/builtin/pet-param-logic.ts`、`scripts/pet-param-logic-selftest.mjs`；改 `app/src/plugins/builtin/live2d-engine.ts`（写入夹取 + `rangeOf`）、`app/src/plugins/builtin/live2d.ts`（`MountOptions.paramRangeFallback` + `setParameter` 契约注释）、`app/src/plugins/builtin/pet-hotkeys.ts`（`Cdi3Names.physicsDriven` + 改用 `physicsParamIds`）、`app/src/plugins/builtin/pet.ts`（`DEBUG_PARAM_RANGES` / `paramRowHtml` 选项化 + 档位行 + 物理标注 / `refreshDebug` / `restoreParams` / 点击委托 / CSS）、`app/src/i18n.ts`（`pet.debug_phys_driven_note` 五语言）。**未改宿主 Rust**（纯前端 + 插件）。
- **待裁定 / 需知晓**：
  - **必须运行时才能确定的量**：**档位数与档位值**（= 运行期 Core 的 min/max）、**哪些参数是物理驱动**（= 模型 `*.physics3.json` 输出集，缺失时回落 cdi3 分组）、**参数范围**（Core 优先，静态 `DEBUG_PARAMS` 仅兜底）—— 三者都**不写死**，随模型不同而不同；`.cdi3.json` v3 不带范围，故档位判定**拿不到范围就不判定**。
  - **未做（按用户裁决 / 约束）**：`.vtube.json` 的 `InputRange/OutputRange` **未引入**；cdi3 分组标题 **未做**；**未新增**第二套参数 / 预设存储；**未碰**语音三条拉条与 `tts*.rs`；物理开关**未新增**按钮（已有热键入口）。
  - **顺带发现（只报告，未改）**：`readCdi3Names` 现在多返回一个 `physicsDriven` 字段（原只收 Id→Name）；引擎产物经压缩后**标识符被改名**，故「按函数名在产物里查证据」不适用于引擎那一份（改用纯逻辑单测 + 源码核对）。


#### §待办 1-4 补 顺带：`publish-plugins.ps1` 开头两份 BOM 归一成一份（2026-10-13）

- **改动点**：`scripts/publish-plugins.ps1` 开头由 `EF BB BF EF BB BF`（两份 BOM）归一为 `EF BB BF`（**恰一份**），**只改前 3 字节、无任何逻辑改动**（现首六字节 = `EF BB BF 23 20 E6`）。
- **为什么**：多一份 BOM 时首行变成命令、`param` 不再绑定参数（PowerShell 5.1 报「无法将 `#` 项识别为 cmdlet」），且**会让 `npm run verify` 的 `[8/8]` 脚本编码自检 FAIL**（判据：前 3 字节 = `EF BB BF` 且第 4 字节 ≠ `EF`，见 [`testing.md`](./testing.md) §测试 3）。
- **验证结论**：`npm run verify`（根）**exit 0**（`[8/8]` BOM 自检 18 份通过）。
- **待真机验收**：无（编码卫生，不影响运行）。

### §待办 1-5 L5 自研调音 / 音乐插件：剩余与未发布
- **目标**：调音「全局生效」与在线音乐发布收尾。
- **已查清**：引擎（`tuning-engine/`，手写 FFT）+ 宿主接线 + 音乐插件调音页（可编辑链 / 预设 / 声道槽 / 卷积 / 延迟 / A-B 盲测）均已落地并实测；在线音乐已并入同一条链（`librespot` `-B pipe -f F32` + `live_audio.rs`）。
- **待办点**：① **P6 全局生效（S3）**：默认输出切虚拟声卡 → 采集 → DSP → 回真实设备 + 「Lunac 音乐 / 全机托管」双模式开关；② **S4 AI 调参**（确定性拟合在引擎里，AI 输出必须过本地关口）；③ **实时频谱**经用户 2026-10-06 决定「暂时不放了」⇒ 暂缓（不是取消）；④ 音乐插件 M2-21/M2-22 的改动**未起 dev 实例真机点击验收、未发布市场**。
- **涉及文件**：`tuning-engine/`、`app/src-tauri/src/player.rs`、`live_audio.rs`、`app/src/plugins/builtin/music.ts`。

### §待办 1-6 长期记忆：触发门槛与自动抽取（**由 §待办 4 提升而来，2026-10-11**）
- **目标**：让「长期记忆」真正被用起来 —— 先解决「写入侧从未触发过」这个不可达，再把 §待办 4 的 **S0「记忆自动抽取（云侧先行）」** 落成可实施方案。（用户 2026-10-11 定为 **AI 侧第一项**，故放在本节 = 已排期 / 高优先。）
- **已查清（2026-10-11 运行时证据，逐条可复核）**：
  - 写入侧只有两条路，**都没触发过**：① 模型主动调 `Remember`（`tools.rs` 条件注册，桥接通才进工具池，`main.rs:3157`）；② 每 N 次**用户提问**的后台复盘 fork（`main.rs:3464`）→ 复盘调模型、经 `lunac/memory_write` 写 `<exe 根>\ModuleData\memory\MEMORY.md`。
  - 门槛计数 `questions` 是 **agent 进程内的局部变量**（`main.rs:3346` 声明 =0、`main.rs:3460` 每次提问 +1），**进程重启即归零**；阈值 `LUNAC_NUDGE_INTERVAL` 默认 **10**（`main.rs:300`），宿主从不注入 ⇒ 恒默认 10。
  - 实测 dev 期（`temp/logs/agent-*.log` 的 `=== agent start` 计数）：2026-10-10 共 **16** 次、2026-10-11 共 **10** 次 agent 进程启动，**单进程最多只处理到 4 次提问**就被重启（换模型 / 换档位 / 新对话 / 中断 / MCP 改动 / App 重启都会 `stop_cli`）⇒ **阈值 10 够不着**。
  - 全新安装（记忆为空）时系统提示词**完全不含** `Remember` 引导：`memory_block` 在 `fetch_memory` 为空时直接返回空串（`main.rs:1728-1730`），只有「已有记忆」时才追加那句提示 ⇒ 冷启动只能靠复盘 fork 播种，而复盘又没触发 ⇒ **死锁**。
  - 成本：复盘**真调模型**（`run_review_fork` → `run_subagent`，单次预算 12 万 token，`main.rs:305`）；用户成本敏感 ⇒ 任何「会持续调模型」的触发方案**必须写明频次上限**。
  - 历史证据（**找不到 = 证据**）：`ModuleData\memory\`（`MEMORY.md`）**不存在**；`temp\logs\*.log` 全文无 `复盘 / Remember / memory_write / nudge` 任何痕迹。
- **已实现（2026-10-11，用户拍板 = 方案 D + 方案 B）**：
  - **D（零模型成本）**：修掉冷启动死锁 —— 记忆为空时**也注入 Remember 引导**（`memory_block_text`：表头 + 引导构成**固定前缀**，记忆正文永远拼在其后；仅「空记忆 + 写侧不可用」才返回空串）。缓存纪律（引导逐字节稳定、正文不混进前缀）已写进代码注释。
  - **B（按会话 + 时间，带上限）**：触发条件 = ① 本轮确实完成一次用户提问（判定点就在 user 分支、`run_query` 返回后）；② 距上次复盘 ≥ **30 分钟**（`REVIEW_MIN_GAP_SECS`）；③ **本会话（= 本 agent 进程）未复盘过**（每会话 ≤ 1 次）。三条全满足才 spawn 一次后台 fork（单次只 spawn 一个线程、无循环重试 ⇒ 单次 ≤ 1 次云调用）。
  - **「上次复盘时间」落点**：agent 自己的 `<exe 根>\temp\review-last`（单行 epoch 秒；`review_state_path` 从 `log_dir` 的父目录派生，同 `images`）。**不写进 MEMORY.md**（会污染模型可见记忆 + 动摇缓存前缀），**不新增宿主通道**（memory 桥只有 read/write，宿主注入还要求宿主知道复盘何时跑过）。跨进程重启有效。
  - **`LUNAC_NUDGE_INTERVAL` / `DEFAULT_NUDGE_INTERVAL` 已删除**（连 `nudge_interval()` 一并删）：宿主从不注入它、新模型不再有「间隔」概念，留着即死代码。原「`0` = 关闭」语义由 `remember_on`（没桥 / 记忆被关 / `Remember` 被黑名单裁掉 ⇒ `ReviewSetup.enabled=false`）承接。
  - **`read_only` / plan_phase 的短路语义原样保留**（那两档下 `write_blocked` 会拒写，派出去只白烧一次 API）。
- **待真机验收口径（2026-10-11 起）**：
  1. **D**：全新（或删掉 `ModuleData\memory\MEMORY.md` 后）启动 agent，agent 日志应出现 `长期记忆为空，已注入 Remember 引导（冷启动）`；模型在一轮里**主动调 `Remember`** 时，`MEMORY.md` 应被创建且含该条 bullet。
  2. **B / ①A**：同一个 agent 进程里连问多次，agent 日志里 `后台复盘 {} 启动` **每 scope 至多 1 次**（global / pet 各 ≤ 1，合计至多 2；2026-10-13 折中）；关掉 App 重开（重启 agent）后，距上次复盘 < 30 分钟应**不再**触发（时间闸两侧共享）、≥ 30 分钟才触发；`<exe 根>\temp\review-last` 应记着上次 epoch 秒。
  3. 复盘真跑成时，`MEMORY.md` 应多出条目（或回 `NOTHING_TO_REMEMBER` 不写）；复盘的模型调用经 `run_subagent` 的 `usage_delta` 通道上报（与子代理同形，`main.rs` 的 `UsageAcc::record`），故成本面板 / `usage-*.jsonl` 应新增对应调用记录。
  - **S0 自动抽取（2026-10-11，方案甲）**：已把复盘 fork 提示词升级为**结构化抽取**（四类该抽 + 显式不抽 + **禁写凭据**），并加**落盘前 `mask_secrets()` 兜底**；触发 / 频次 / 开关一行未改。详见下方「S0 记忆自动抽取」小节的「已实现」。
- **涉及文件**：`core-agent/src/main.rs`（`memory_block` / `memory_block_text` / `should_review` / `review_state_path` / `ReviewSetup` / `run_review_fork`）、`core-agent/src/tools.rs`（`remember_tool` / `BRIDGE_TOOLS` / `write_blocked`）、`app/src-tauri/src/mcp_server.rs`（`lunac/memory_read` / `_write`）、`app/src-tauri/src/commands.rs`（`tool_blacklist_args`、spawn env）。
- **已实现（2026-10-13，P1 折中 + B2）**：
  - **①A 复盘配额折中** —— **选型 (a)**：保留**单一** `temp\review-last` 承载**共享时间闸**，把「每会话 ≤ 1 次」的 `reviewed_this_session` 由单个 bool 拆成**按 scope 的 `[bool; 2]`**（`tools::MemoryScope::index` 取下标，global / pet 各一份）。⇒ `Global` 与 `Pet` 在同一 agent 进程内**各能触发一次**；`last_review_ts` 仍**两侧共享**（任一作用域刚复盘过，另一侧 30 分钟内也不跑，防总成本翻倍）。落点：`should_review` 改签名收 `fired_per_scope: [bool; 2]` + `scope`（**仍纯函数**）；调用点按本轮 `turn_scope` 置对应下标（`main.rs` 提问循环）；`ReviewSetup::spawn` 经 `tctx` 克隆已随本轮作用域，**无需改签名**。触发语义其余不动（每会话每 scope 上限、30 分钟、输入 6000 字、输出 ≤ 3 条、预算 12 万 token、`enabled` 开关链）。
  - **①B 复盘来源 / 时间标注** —— B2（块头一行，2026-10-13）已做：`memory_block_text` 的引导段加**一行固定来源 / 时间说明**（英文，属固定前缀、逐字节稳定、按 scope 措辞）。**B1（逐条日期 / 来源）已实现（2026-10-17）**，见下一条；去重语义已同步改。
  - **①B-1 逐条日期 / 来源 + 去重新语义（已实现，2026-10-17）**：
    - **格式**：写侧唯一落盘口 `mcp_server.rs` 的 `write_memory_at` 给每条 bullet 加行首前缀 `- [YYYY-MM-DD · <scope>] <原内容>`。日期由**宿主**用 Win32 `GetLocalTime` 取**本地日期**（不交给模型；UTC 会跨日），scope 用稳定英文 id（`global` / `pet`，不随界面语言漂移）；多行 bullet 的续行缩进原样保留，前缀只加在首行 `- ` 之后。`MEMORY.md` 仍是扁平 bullet，不分节。成长笔记 / 人格建议复用同一落盘助手但**不盖前缀**（`provenance: None`）。
    - **去重新语义**：按「去掉行首 `[日期 · scope]` 前缀后的正文」归一比较（折叠所有空白 + 大小写不敏感，与逐条删共用 `normalize_entry`）。命中同正文 ⇒ **不追加**，并把该条日期**刷新为本次写入日期**（语义 = **最后确认时间**，不是初次得知）；正文与 scope 不变。旧的无前缀 bullet 在同正文再次写入时**原地补前缀**（自然迁移，**不批量回填**历史日期 —— 没有可信的旧日期）。
    - **闸与脱敏**：单条 ≤2000（按最终落盘内容，含 `- ` 与来源前缀）；`mask_secrets` 仍在落盘前执行；`replace: true` 仍整体重写，且所有 bullet 的日期 / scope 由宿主重盖（模型自带的假日期一律覆盖）。
    - **上限已改动态（2026-10-17）**：正文额度 6000 不占前缀；总长硬顶 9000。正文额度只统计每条去掉 `- ` 与 `[日期 · scope] ` 前缀后的正文（含续行），前缀再多也不挤占正文档位；总长硬顶（含前缀 / 换行 / 续行缩进）防「条目多 + 前缀长」把每轮注入体积顶爆。校验顺序 = 脱敏 → 单条 2000 → 正文合计 6000 → 总长 9000；三种拒绝语义分别为「单条记忆过长」「正文额度已满」「总长度硬顶」，均报错不静默截断，`replace: true` 同样受两口径约束。注入侧 `MEMORY_INJECT_CHARS` 同步取 9000（服务端允许落盘的文件整份注入，不在读侧二次截断）；冻结快照语义不变。
    - **注入提示**：`memory_block_text` 的 B2 说明行改为描述新格式（`[YYYY-MM-DD · global|pet]`，日期 = 最后确认；无前缀 = 更早的、未记录日期的背景），仍是**逐字节稳定**的固定前缀。
    - **本轮新增单测（宿主 +5）**：`memory_entries_are_tagged_with_host_date_and_scope`（前缀格式 + 宿主本地日期 + 两 scope 隔离）、`memory_same_body_on_a_new_date_is_refreshed_not_duplicated`（同正文不重复 + 日期刷新）、`memory_legacy_untagged_entry_is_migrated_in_place_on_rewrite`（旧条目原地补前缀）、`memory_multiline_bullets_keep_continuation_indent`（续行缩进）、`memory_caps_still_apply_after_tagging`（单条 2000 含前缀 + 正文额度 6000 / 总长 9000，2026-10-17 已按动态口径更新）；另更新 `memory_masks_secrets_before_persisting` 钉住「前缀不绕过脱敏」。动态上限轮新增：`memory_body_budget_excludes_provenance_prefix`、`memory_body_budget_overflow_is_rejected_with_body_semantics`、`memory_total_hard_cap_rejects_prefix_heavy_file`、`memory_replace_obeys_both_budgets`、`memory_budgets_are_judged_after_masking`（宿主 +5）。
  - **2026-10-13 轮新增单测**：`review_gate_splits_session_quota_by_scope_and_shares_time_gate`（per-scope 配额 + 共享时间闸）、`memory_block_provenance_line_is_byte_stable_and_precedes_body`（说明行逐字节稳定 + 与正文分离；2026-10-17 断言同步改为新文案）。

### S0 记忆自动抽取（云侧先行）· 可实施方案（2026-10-11 设计）

> **状态：已实现（2026-10-11）：甲已落地 + 凭据兜底已加。** 本节不推翻上面「已实现 D+B」的任何结论。用户 2026-10-11 拍板：选**甲**、**不加**独立的「自动抽取」开关（沿用「关掉 `Remember` 即连带关掉复盘 / 抽取」）。

**已查清（现有复盘 fork 的真实行为，逐条可复核）**
- **它让模型做什么**：提示词 = `REVIEW_SYSTEM`（角色段）+ 环境块 + 技能清单（`build_review_system`，`main.rs:1583-1585`）+ 用户消息（当前记忆 + 对话快照 + 指令，`review_prompt`，`main.rs:4483-4498`）。内容 = 「判断这段对话里有没有值得长期保留的事实，有就用 `Remember` 写」——**不是**自由复盘；无事实即回 `NOTHING_TO_REMEMBER`（`main.rs:352`、`4496`），单次最多写 `REVIEW_MAX_ITEMS=3` 条（`main.rs:320`）。
- **输入**：`review_transcript(history, REVIEW_TRANSCRIPT_CHARS)`（`main.rs:4428-4477`）——只取 text / tool_use / tool_result 三类块，工具调用压成一行摘要、工具结果只留前 300 字（`4456`），**从最早处**丢弃到 **6000 字符**内（`main.rs:318`、`4467`）；thinking 等不进。**不是完整 history**。
- **能调哪些工具**：`REVIEW_TOOL_WHITELIST = [Read, Glob, Grep, Write, Edit, Skill, Remember]`（`main.rs:331-332`），从**本轮工具池**筛出（`main.rs:3248-3256`）⇒ 天然继承 `--disallowedTools` 与条件注册；无 `Agent`（防递归）。写记忆**只走一条**：`Remember` → 桥 `lunac/memory_write`（`main.rs:4759-4778` → `mcp_server.rs:536-593`）。
- **`.md` 格式**：**扁平 bullet 列表**，每条 `- ...`、多行缩进续行（`mcp_server.rs:562`）；单条 ≤2000 字符、整文件 ≤6000 字符（`509-511`）；**去重 = 整条 bullet 完全相等**（`563-564`），**无语义去重、无冲突消解、无分节/分类**。
- **用户看得到吗**：**看不到产物写的是什么**。结论只进 agent 日志两行（`后台复盘 {id} 启动（快照 N 字）`，`main.rs:4608`；`后台复盘完成：{首行}`，`join_review` `4615-4627`）；用量经 `UsageAcc` 上报（`main.rs:1951-1975`）⇒ 成本面板 / `usage-*.jsonl` **有记录**。**没有**任何 UI 展示「写了什么」，`MEMORY.md` 也无查看入口。⇒ 对用户**不透明**。
- **`NOTHING_TO_REMEMBER` 从哪来、怎么处理**：它是 `review_prompt` 里给模型的**出口文案**（`main.rs:4496`）；模型「不写」= **不调 `Remember`**（不是靠识别这句话）——桥/`write_memory_at` 对这句话**无任何特殊处理**。
- **凭据缺口（2026-10-11 已修）**：原 `REVIEW_SYSTEM`（`main.rs:336-361`）**没有**任何「禁止写凭据/密钥」的措辞（只有 `no paths or IDs`），桥侧 `write_memory_at` 也不做 `mask_secrets()`。⇒ 对话里一旦出现 API key / 私钥，复盘**可能原样写进 `MEMORY.md` 并注入以后每次对话**（与 A17 / M2-8 的凭据告警先例同类）。**现已双管齐下**：① 提示词写明**禁写凭据**；② `write_memory_at` 落盘前过 `mask_secrets()` 兜底（见下「已实现」）。

**「自动抽取」的增量判定（用证据确认初判）**
- **初判成立**：现有复盘 fork **已经是「会话级、粗粒度的自动抽取」**（自动触发 + 自动判断 + 自动写长期记忆）。
- 于是「自动抽取」相对现有 fork 的增量**只剩两条**：**(a) 口径结构化**（明确 taxonomy + 禁写清单，含凭据）；**(b) 粒度变细**（每 N 轮 / 每轮，而非每会话一次）。「会话结束时一次性收口」这条在现有实现下**不可达**（见乙）。

**触发时机逐个取舍（含调用频次）**

| 触发时机 | 能抓到什么 | 漏掉什么 | 云侧调用频次 |
|---|---|---|---|
| 每轮提问后 | 当下所有事实 | 几乎不漏 | 每轮 1 次 —— **撞成本红线，S0 不可选** |
| 每 N 轮 | 会话中期冒出的偏好/约定 | 末尾 <N 轮的事实；计数需额外落盘才不重蹈「重启归零」 | `ceil(提问数/N)`，**仍持续调云** |
| 会话结束（进程退出前） | 整段会话收口 | 宿主对 agent 是**硬杀**（`taskkill /F /T` + `child.kill()`，`cli_bridge.rs:126-131`；agent 主循环仅在 stdin 断开时 break，`main.rs:3407`）⇒ **没有优雅退出信号可挂** | 每会话 ≤1，但与现有复盘**同粒度重叠** |
| 与现有复盘合并 | 复用已实现的频次闸 / 桥 / 工具集 | 粒度仍是会话级 | 每会话 ≤1（现状） |

**选定方案（推荐 · 甲）：把现有复盘 fork 的提示词升级为「结构化抽取」，触发与频次一行不改**
- **做法**：把 `REVIEW_SYSTEM`（`main.rs:336-361`）与 `review_prompt`（`4483-4498`）从「泛化复盘」改成**结构化抽取**——写明抽取 taxonomy（用户偏好/工作风格 · 项目约定 · 环境事实 · 已达成的决策及理由）与**禁写清单**（一次性过程信息 · 进度/待办 · 可从仓库读到的 · **任何凭据/密钥/口令/私钥/连接串**）；触发点、频次、工具集、写回通道**全部不动**。
- **频次上限（写死）**：每会话 ≤1 次、距上次 ≥30 分钟（`REVIEW_MIN_GAP_SECS`，`main.rs:310`）；单次输入截断 `REVIEW_TRANSCRIPT_CHARS=6000` 字符；单次输出 ≤`REVIEW_MAX_ITEMS=3` 条；单次 token 硬上限 `REVIEW_BUDGET_TOKENS=120_000`（`main.rs:314-315`）**不变**。**成本量级与现状同阶**（提示词加几十～几百 token、不新增调用）。
- **与已存在记忆的关系**：沿用整条 bullet 精确去重；矛盾时**不自动删旧条**，改为在提示词里要求「发现与已有记忆冲突时用 `Remember` 的 `replace` 合并重写」（`replace` 通道已存在，`tools.rs:696-699`）；格式仍为扁平 bullet，**暂不分节**。
- **写回通道**：**复用 `lunac/memory_write`**，不新增桥方法。
- **可观测性**：① agent 日志两行（启动/完成）；② 用量面板 / `usage-*.jsonl`（成本可对账）；③ 产物即 `ModuleData\memory\MEMORY.md` 本身（用户可直接读）——**不加 UI**（另议）。
- **开关**：沿用现有两处——`LUNAC_MEMORY`（读侧总闸，默认开，`main.rs:1686-1697`）与 `Remember` 工具黑名单（关掉 ⇒ `remember_on` 假 ⇒ 复盘 `enabled=false`，`main.rs:3141-3143`/`3267`，**连带关掉自动抽取**）。是否要**独立开关**见文末裁定。
- **不影响固定前缀缓存**：抽取跑在**自己那段系统提示词**（`review_system`）上，与主请求前缀无关；写入后本进程系统提示词**逐字节不变**（冻结快照，`main.rs:1682-1684`）⇒ 不产生「每写一次记忆就打一次 miss」。
- **实施点（改哪些行）**：`core-agent/src/main.rs:336-361`（`REVIEW_SYSTEM` 文案）、`4483-4498`（`review_prompt` 文案）；`app/src-tauri/src/mcp_server.rs:536-593`（**仅当**决定做冲突合并/凭据过滤时才动）。**触发与频次代码一行不改。**

**已实现（2026-10-11，方案甲已落地 + 凭据兜底已加）**
- **A（方案甲：提示词结构化）**：`core-agent/src/main.rs` 的 `REVIEW_SYSTEM`（const 现 `main.rs:342`）重写为**结构化抽取**——新增「**只抽四类**」（用户偏好/工作风格 · 项目约定与规范 · 环境与工具链事实 · 长期任务的背景与决策）与「**不抽**」（一次性过程信息 / 本轮的 play-by-play · 可从仓库或文件现读到的内容 · 进度与待办 · 已在当前记忆里出现过的 · 闲聊与猜测），并**单列一段 `NEVER write credentials`**：对话里出现 API key / token / 密码 / 私钥 / 连接串等**一律不写、连片段都不写**；`review_prompt`（现 `main.rs:4507`）的指令段同步升级（禁写凭据复述一遍、只抽分类内事实）。**出口 `NOTHING_TO_REMEMBER` 与 `REVIEW_MAX_ITEMS=3` 两条上限语义不变**。
- **B（凭据兜底：落盘前过 mask）**：`app/src-tauri/src/mcp_server.rs` 的 `write_memory_at`（现 `537-` 起）在**落盘前**对内容过既有的 `crate::log::mask_secrets`（复用 `app/src-tauri/src/log.rs:336` 的实现，**未另造一套**）—— 裸 `sk-...`、`Bearer <token>`、`api_key/token/password/secret/...` 键值对一律打码成 `***` / `sk-***`。脱敏是**纯字符串变换（返回 `String`、无 `Result`）**，只把凭据值打码、其余字符（含换行）逐个保留 ⇒ 既**不弄坏多行 bullet 的续行缩进**，也**不新增写失败路径**（失败路径与改造前逐字节相同，最坏只是内容里多两处码）。单测 `mcp_server::tests::memory_masks_secrets_before_persisting` 钉住这两点。
- 触发条件与频次**一行未改**：每会话 ≤1 次、距上次 ≥30 分钟、单次 ≤1 次云调用、输入截断 6000 字、输出 ≤3 条、单次预算 12 万 token —— **全部保持**。
- 开关**未新增**：沿用「关掉 `Remember`（工具黑名单）⇒ `remember_on` 假 ⇒ 复盘 `enabled=false`」这一条路。

**为什么不是乙 / 丙**
- **乙（会话结束收口）**：与现有复盘**同粒度、同频次**（每会话 ≤1），不产生新信息；且宿主对 agent 是**硬杀**（`cli_bridge.rs:126-131`），没有「退出前」这个时机，要先加**宿主优雅握手**（跨层改动）；与复盘**叠加**还会二次计费。⇒ 不做。
- **丙（每 N 轮细粒度）**：这正是 §4.1.6 给 **S1 本地决策旁路**排的位（「每 K 轮 / 压缩事件，不做每轮」，`archive/agent-feature-backlog.md:1316`）——在**云侧先行**阶段每 N 轮调云模型，频次随会话长度线性增长，直接撞用户成本红线。⇒ **推迟到 S1**（换本地小模型 ⇒ 零 API 成本）再做。

**验收口径（方案已落地，待真机验收）**
1. **触发**：同一 agent 进程内 `后台复盘 … 启动` **每 scope 至多 1 次**（global / pet 各 ≤ 1；2026-10-13 折中，时间闸两侧共享）；`temp\review-last` 记 epoch 秒（沿用 D+B 口径）。
2. **口径（甲）**：构造含「用户偏好 / 项目约定 / 一次性任务细节 / 一条 API key」的对话，抽取后 `MEMORY.md` **只应含前两类**、**不含**任务细节与凭据。
3. **凭据兜底（B，与提示词独立）**：即便是提示词漏掉 / 模型不听那一轮，凡经 `write_memory_at` 落盘的内容都要**在文件里被脱敏**——`MEMORY.md` 里不应出现完整 `sk-...` 明文，且**多行 bullet 的缩进形状不被破坏**（`- ` 起头、续行缩进仍在）。
4. **去重 / 冲突**：重复事实不再追加第二条；与已有记忆冲突时走 `replace` 合并（或至少不产生自相矛盾的两条并存）。
5. **成本**：`usage-*.jsonl` 新增记录的单次 token 与现状同阶（≤ 数万，远低于 12 万上限）。

**已裁定（2026-10-11）**
1. **选甲** —— 零新增调用、频次与成本不增、直接补上口径与凭据缺口。
2. **不加**独立的「自动抽取」开关 —— 沿用「关掉 `Remember` 工具即连带关掉复盘与自动抽取」。

### §待办 1-7 调研：对话流里的「重要判断」必须给用户一句短结论（2026-11-03，用户报障）
- **目标**：模型在思考里做过**影响后续动作的关键判断**（是否要删除某文件 / 是否要改某配置 / 走哪条分支）时，Lunac 要在对话里输出**一句面向用户的短结论**，让用户不必翻「思考过程」才知道它判了什么、为什么。
- **用户报障（2026-11-03，release 实测）**：上一轮要求「先判断模组文件是否需要删除；**不需要删除就直接继续任务**，需要删除才回答」。思考过程里**明确判出了「不需要删除」**，但 Lunac 最终**没有任何一句总结**，用户看不到结论，只能去展开思考块才明白它为什么不删。
- **已查清（代码路径，待复核清单）**：
  - 思考（`thinking`）与正文（`text`）在 core-agent 里是两条独立通道；前端 `agentView.textSegs` 只把 `text` 段当「最终答案」（`agentFinalText`），`thinking` 不进 `chatHistory`、也不进最终气泡 ⇒ **判断只存在于思考里时，界面上就只剩过程、没有结论**。
    - 相关落点：`core-agent/src/main.rs` 的 `PERSONA_AND_STYLE` / `SYSTEM_PROMPT`、`app/src/main.ts` 的 `agentNewBlock("thinking")` 与 `agentFinalText`。
  - 2026-11-03 刚加进 `PERSONA_AND_STYLE` 的「一口气做完、不许让用户发继续」硬规则**只约束「不要停」**，没有要求「做了分支判断要报结论」⇒ 模型可以合法地「判完就闷头继续」。
- **待办点（调研 + 落地口径）**：
  1. **判定「什么是重要判断」**：给出可执行的判据（分支闸门类：删 / 不删、改 / 不改、装 / 不装、走 A 还是 B；以及「本可以问用户但选择自行决定」的那类决定）。**不要求**每步都汇报（否则变成噪音、也违背「一口气做完」）。
  2. **落点纪律**（不得违反既有缓存与收口纪律）：短结论必须拼进**普通 `text` 段**（走 `agentAppend("text", …)` 那条），**不许**新增 `system` 消息 / 新增 content 块 / 拼进固定前缀；也不得因此中断当前任务等用户确认（与 §待办里「一轮做完」的硬规则叠加验证）。
  3. **文案形态**：一句话，带判断 + 理由（例：「已确认模组文件不需要删除（……），继续执行后续任务。」），**不复述**思考原文、不重复已输出过的正文。
  4. **验证方式**：构造同一类提问（「若 X 则答我，否则直接做」）跑 release agent，检查对话流里是否稳定出现一句判断结论，且思考块之外可读。
- **涉及文件**：`core-agent/src/main.rs`（`PERSONA_AND_STYLE` / 收口与 `final_text` 组装处）、`app/src/main.ts`（`agentAppend` / `agentFinalText` / 过程折叠）、`docs/ai-spec.md`（对话流契约）。
- **验收**：同一条「判断题」请求，界面上**不展开思考块也能读到结论**；且该结论只出现一次（不因流式 / 整包两条渲染路径重复）；一整轮仍然一口气跑完、不出现「请发继续」。
- **已实现（2026-11-03，AI 侧先行）**：`core-agent/src/main.rs` 的 `PERSONA_AND_STYLE` 新增
  `## Visible decisions (hard rule, 2026-11-03)` —— 分支判断（删/不删、改/不改、装/不装、
  走 A/走 B、问你/自己定）必须写进**可见正文**，一句话给结论 + 理由，不许只留在 thinking；
  不许事后复述、不许变成计划。单测
  `visible_decision_and_pre_question_summary_rules_are_present`（`cargo test` **464 passed /
  15 ignored / 0 failed**）。
- **仍未做**：真机验证（要构造一次「若 X 则答我，否则直接做」的请求）；前端零改动。

### §待办 1-8 调研：思考过程里显示「本次改了哪些文件」（2026-11-03，用户提出）
- **目标**：过程块里除了「已执行 N 条命令」，再给一行文件维度 —— 单个文件写**文件名**，
  多个文件省略成「**已修改 N 个文件**」，样式与 `agent.cmd_group_cmds` 那条一致。
- **现状（代码位置）**：
  - 分组头由 `refreshCmdGroup` 从组内 DOM 现推（`app/src/main.ts:7517-7535`），只统计
    `Cmd` / `PowerShell` 条数与失败数；
  - 工具卡本身带名字与参数摘要（`app/src/main.ts:2885-2894` 历史、实时同族）；
  - 过程快照的类型 `SessionStep`（`app/src/main.ts:2197`）**没有文件字段** ⇒ 即使实时算出来，
    切回历史会话也会丢。
- **可行边界（不能糊弄，按 `forbidden.md` §禁用 1）**：
  - `Write` / `Edit` / `NotebookEdit` 的参数里有 `file_path` / `notebook_path` ⇒ **可以精确**。
  - `Cmd` / `PowerShell` **无法**从命令文本推断改了哪些文件（`cargo fmt`、脚本、`git` 都可能改）。
    要判定只能前后对比工作区（`git status` 或文件快照），代价高且对未纳入 git 的文件会漏。
    ⇒ 第一版**只统计文件类工具**，命令类不冒充；确实由命令改的文件不显示。
- **落点**：`agentAppend("tool", …)` 处解析参数写入 `SessionStep.files?: string[]`；
  `refreshCmdGroup` 追加一段文件文案；`historyStepHtml` 与落盘快照同步该字段。
- **验收**：单文件编辑后过程块出现该文件名；连续改 3 个文件出现「已修改 3 个文件」；
  只跑命令时**不出现**文件行；切换会话再回来仍可见。

### §待办 1-9 调研：思考过程里展示前端样式改动的「前后对比图」（2026-11-03，用户提出）
- **目标**：改前端样式时，过程块里直接给出 before / after 两张图，不必让用户自己开界面比对。
- **现状**：
  - 对话区**已经**能内联本地图片（`convertFileSrc`，见 `app/src/main.ts:852` 与
    `app/src/main.ts:14264-14293` 的全窗查看），CSP 的 `img-src` 也已包含 asset 协议；
  - **agent 侧没有任何截图能力**：`core-agent/src/webbrowser.rs:15` 明确写着
    「只打开 + 等待 + 取 DOM，没有 click / type / **截图**」。
- **可行路径（含代价）**：
  1. 新增 `Screenshot` 工具：WebView2 / CDP 的 `Page.captureScreenshot`（要求前端以可调试
     端口起在 dev 模式），或 Windows `PrintWindow` / GDI+（对 WebView2 常拿到黑帧，不推荐）。
  2. 采集时机：改样式**前**抓一张、热更新**后**再抓一张，成对落到临时目录并回传路径。
  3. 展示：思考块里内联 `<img>`，折叠态不加载（避免大图常驻内存）。
  4. **可信度风险**：截图是像素级证据，受主题 / DPI / 窗口尺寸影响 —— 必须固定 viewport、
     固定主题、固定缩放，否则「前后对比」是假的。
- **结论**：技术可行，但要「截图工具 + 固定夹具 + 思考块图片渲染」三件套，属独立课题；
  建议**先做 1-8**（纯前端、无新依赖），1-9 单独立项再评估。
- **验收（如果做）**：一次样式改动后过程块出现两张图且标注顺序正确（before → after），
  折叠态不加载图片，展开才请求。

### §待办 1-10 调研：弹「问用户」卡片前必须先输出一段可见总结（2026-11-03，用户提出）
- **目标**：模型一旦要问用户（`AskUserQuestion` / 任何会弹审批卡的动作），必须在卡片**上方**
  先给一段**普通正文**总结 —— 说明「之前在思考什么、目的是什么、进度到哪、现在要你定什么」。
  这段**不是 thinking**（思考块默认收起，用户看不到）。最终回答直接在这段总结上**续写**，
  不再另起一段总结，省掉一次重复输出。
- **现状**：
  - `thinking` 不进 `chatHistory`、也不进最终气泡（§待办 1-7 已查清）⇒ 卡片出现时用户往往
    只看到一个孤立的提问框，不知道上下文。
  - 卡片走 `can_use_tool` → 前端 `classifyRequest()` 插入对话流；正文走 `agentAppend("text")`。
- **待办点（契约 + 落点）**：
  1. **触发条件**：调用 `AskUserQuestion`（以及任何会被拦成审批卡的工具）之前，模型必须已输出
     一个 text 段；提示词里写成硬规则，并给一句模板（目的 / 已完成 / 待定）。
  2. **落点纪律**（不得违反缓存与收口纪律）：走普通 `agentAppend("text", …)`，**不许**新增
     system 消息 / 新增 content 块 / 拼进固定前缀，也不许因此停下等确认。
  3. **续写而非重复**：卡片后的最终回答接在同一段之后，不允许把总结再复述一遍
     （参考 §待办 1-7 的「不复述」口径）。
  4. **验证**：构造一个必须提问的任务，检查 ① 卡片上方有总结且含目的/进度；② 不展开思考块
     也能读懂；③ 最终回答是续写、全文只出现一次同类总结；④ token 用量与现状同阶（不新增调用）。
- **涉及文件**：`core-agent/src/main.rs`（`PERSONA_AND_STYLE`、`AskUserQuestion` 工具描述、
  `final_text` 组装）、`app/src/main.ts`（`agentAppend` 与审批卡插入顺序）、`docs/ai-spec.md`。
- **验收**：审批卡上方必有一段可见总结；卡片在它下方；同一问题全程只有一段总结。
- **已实现（2026-11-03，AI 侧先行）**：
  - `core-agent/src/main.rs` 的 `PERSONA_AND_STYLE` 新增 `## Asking the user (hard rule)`
    —— 调 `AskUserQuestion` 之前必须先写一段普通正文（在做什么 / 现在到哪 / 要你定什么），
    工具返回后**从这段继续**，不许复述、不许再补一段收尾总结；
  - `core-agent/src/tools.rs` 的 `AskUserQuestion` 描述追加同一要求
    （工具描述是模型唯一能看到的调用纪律）；单测
    `ask_user_question_requires_a_visible_summary_first`。
- **仍未做（属 UI 侧，换 Flutter 后一起做）**：卡片与总结的 DOM 顺序、最终回答「续写而不是
  新气泡」的渲染验证 —— 归 §待办 1-14 的 B / C 类。

### §待办 1-11 前端：回合信息条按工具类别统计 + 「思考过程」式折叠明细（2026-11-03，用户提出）
- **用户口径**：现在信息条只有「耗时 X 秒 · N 次失败」，要在同一行补上分类计数：
  **创建几个文件 / 编辑几个文件 / 读取几个文件 / 搜索几个文件 / 抓取几个网页 / 执行几个命令**；
  每一类都能像「思考过程 (N 字)」那样**折叠展开**，展开后给出具体条目：
  - 读取类：`已读取 <文件名>`；
  - 搜索类：`找到 x 行` + `文件:行号`（Grep 的命中逐条列出）；
  - 网页类：`抓取 x 个相关网页` + URL 列表；
  - 写/编辑类：文件路径列表（与 §待办里「已改文件」列表同源，不另起一份状态）。
- **现状（代码位置）**：
  - 信息条文字只拼两项：`turnMetaText`（`app/src/main.ts:2933-2940`）；
  - 信息条本身是 `buildTurnMeta`（`main.ts:2945-`），折叠由 `wireTurnMeta` 接到过程容器；
  - 过程快照 `SessionStep`（`main.ts:2197`）有 `name` / `detail` / `result` / `path`，
    **没有**「工具类别」与「命中行」这类结构化字段 ⇒ 历史回放时无法重算分类计数。
- **落点（不新增第二套状态）**：
  1. `agentToolInput()` 处按工具名归类（Write/Edit/NotebookEdit/Read/Glob/Grep/WebSearch/
     WebFetch/Cmd/PowerShell…），把 `kind` 写进 `SessionStep`（沿用 `path` 的做法）；
  2. Grep 的结果在 `fillToolCard` 时已拿到原文，解析出 `file:line` 对（解析失败就只给行数，
     不编造字段名）；
  3. `turnMetaText` 改成渲染「耗时 · 失败 · 创建 N · 编辑 N · 读取 N · 搜索 N 行 · 网页 N ·
     命令 N」，**0 项不显示**（避免一条全是 0 的长条）；
  4. 每类做成 `<details class="think-block">` 同族的折叠块，summary 用同一套图标 + 暗字 +
     字数/条数，展开体复用 `.think-content` 的底色与行高。
- **纪律**：全部走前端过程区，**不进** `chatHistory`、不进回灌 payload、不改固定前缀；
  计数从已有工具调用现推，不额外调用模型。
- **待定**：信息条放不下时是否换行 / 只显示前 4 类 + 「更多」；单张工具卡是否也显示自己的耗时
  （现在 `AgentToolCard.startAt` 只用于内部，未渲染）。
- **验收**：一次「读 2 个文件 + 改 1 个文件 + grep 命中 12 行 + 抓 1 个网页 + 跑 3 条命令」的
  回合，信息条六类数字正确；每类展开后条目与工具卡一致；0 值不出现；切历史会话后数字不变。

### §待办 1-12 缺陷：PowerShell / Cmd 卡片在历史记录里「展不开」（2026-11-03，根因已定位）
- **用户报障**：历史记录里的 PowerShell 工具卡点了没反应，看不到输出。
- **根因（三处串起来的，不是 CSS 问题）**：
  1. 命令类卡片把输出写进 **xterm 终端** `.tool-term`（`app/src/main.ts:7686`），
     而 xterm 的 `textContent` 是空的；
  2. 采集快照时 `stepFromEl` 用 `ownToolOut(el)?.textContent` 取结果（`main.ts:2784`），
     而 `ownToolOut` 只认 `:scope > .tool-body > .tool-out`（`main.ts:7242-7244`）——
     命令卡只有 `.tool-term` ⇒ 取到空串；
  3. 历史渲染 `historyStepHtml` **只在 `s.result` 非空时**才输出 `.tool-body`
     （`main.ts:2892`）⇒ 命令卡在历史里**根本没有可展开的正文**，`<details>` 开合当然没反应。
- **修复方向（两处都要改）**：
  - 快照要保存终端缓冲而不是 DOM 文本：`appendTermBuf` 已经维护 `card.termBuf`
    （`main.ts:7561-7565`），把它同步到一个可读位置（如 `termHost.dataset.buf` 或
    `SessionStep.result` 直接取 `card.termBuf` 的截断副本，上限沿用 `TERM_BUF_MAX` 的口径）；
  - 历史命令卡按 shell 形态渲染 `<pre class="tool-out">`（而不是空的 `.tool-body`），
    并保证**即使输出为空也输出 body**，让展开至少能看到完整命令（`.tool-cmd-full`）。
- **验收**：跑一条有输出的 PowerShell 与一条无输出的 PowerShell，进历史会话后都能展开：
  前者显示输出、后者至少显示完整命令；`Cmd` 同样；普通工具卡不受影响。

### §待办 1-13 调研：默认工具集是否有可合并 / 可优化的（2026-11-03，仅调研）
- **现状工具清单**（`core-agent/src/tools.rs`，按出现顺序）：Read / Write / Edit /
  NotebookEdit / Cmd / PowerShell / Glob / Grep / WebSearch / WebFetch / WebBrowser /
  AskUserQuestion / TodoWrite / PetControl / SessionSearch / MemorySearch / Agent /
  EnterPlanMode / ExitPlanMode / ListPeers / SendMessage / EnterWorktree / ExitWorktree /
  SendUserFile / PushNotification / RepoMap / LSP / CronCreate / CronDelete / CronList /
  TaskEnqueue / TaskList / TaskCancel / ImageMeasure / ImageAnnotate；另有桥接工具
  （`BRIDGE_TOOLS` 11 个）、Remember / PetGrowth / PetPersonaSuggest、MCP 资源与提示词 4 个。
- **合并候选（收益 / 风险）**：
  | 候选 | 收益 | 风险 |
  |---|---|---|
  | `CronCreate/Delete/List` → 一个 `Cron{action}` | 少 2 个 schema | 前端与权限按名字特判的地方要同步 |
  | `TaskEnqueue/List/Cancel` → 一个 `Task{action}` | 少 2 个 | 同上 |
  | `EnterWorktree/ExitWorktree` → `Worktree{action}` | 少 1 个 | `enter_worktree` 有独立单测与门控 |
  | `EnterPlanMode/ExitPlanMode` → `PlanMode{action}` | 少 1 个 | 模式切换是硬状态，合并后误用代价高 |
  | `ListMcpResourcesTool/ReadMcpResourceTool`、`ListMcpPromptsTool/GetMcpPromptTool` | 4 → 2 | MCP 协议侧名字可能被外部约定 |
  | `WebFetch` + `WebBrowser` → `Web{render:"static"/"browser"}` | 少 1 个 | 两者权限档位不同（一个纯 HTTP、一个开浏览器） |
  | `Glob` + `Grep` → `Search{pattern,paths}` | 少 1 个 | Grep 还承担「按内容找」的安全边界，合并后门控要重写 |
  | `ImageMeasure` + `ImageAnnotate` → `Image{action}` | 少 1 个 | 两者输出形态差异大（JSON vs 新文件） |
- **不建议合并**：`Cmd` 与 `PowerShell` 虽然能力同构，但**前端与 agent 都按名字特判**
  （终端渲染 `main.ts:7673`、实时控制 `main.ts:7717`、权限与 `run_one_tool` 分支），
  合并会同时动 UI / 权限 / 安全校验三层，收益（1 个 schema）远小于风险；
  `Read` 与 `Write` 更不该合（权限相反）。
- **共同代价**：所有工具 schema 都在**固定前缀**里，改一次就整体 cache miss 一次；
  另外提示词里有多处按工具名写的规则（`PERSONA_AND_STYLE` / 审批文案），必须同批改完，
  不能分两次上线。
- **结论**：优先做「纯动作枚举」的三组（Cron / Task / Worktree），其次 MCP 两组；
  `Cmd`+`PowerShell`、`Read`+`Write`、`Glob`+`Grep` **保持现状**。实施前先量一次
  固定前缀的 token 占比，收益 < 1% 就不做。

### §待办 1-14 Linux / Flutter 迁移：先做「AI 侧」的拆分结论（2026-11-03，用户裁决）
- **用户口径**：计划迁到 Linux，并**先接入 Flutter**；因此**先做 AI 侧（core-agent / 工具 /
  记忆 / 引擎）的改动**，纯前端的改动等 UI 重做时再做，避免白做。
- **前提事实（回答「只换前端行不行」）**：
  - Tauri 能上 Linux，但渲染引擎从 WebView2(Chromium) 换成 **WebKitGTK**；前端代码几乎不用改，
    **但宿主 Rust 有 58 处 `cfg(target_os = "windows")`**（自启 / 系统代理 / 透明窗 / GDI 抓屏 /
    WASAPI / SpeechRecognizer / CoreWebView2 拦截），`tauri.conf.json` 的 `bundle.targets`
    目前只有 `nsis`。⇒ **不是"只换前端"，是"前端不动 + 补 Linux 后端"**。
  - 换 Flutter 则相反：**后端 Rust 基本全留**（可经 FFI/IPC 复用），**前端与 TS 插件全部重写**。
- **分类（按「换框架后是否仍然有效」）**：
  | 类别 | 待办 | 说明 |
  |---|---|---|
  | **A 纯 AI 侧（先做）** | 1-7 重要判断给短结论、1-10 提问前先总结（提示词部分）、1-13 工具合并、3-28.1 中途三态（agent 侧已实现）、3-28.4 错误三要素的**分类与文案**、3-28 其它 tools 补全、1-6 记忆触发与抽取、1-2 成本归因、3-29 引擎缺口 G7–G15、真机 4 类互操作（SS legacy / SS-2022 / VMess / REALITY） | 全在 `core-agent/` 或 `app/src-tauri/native_proxy/`，与 UI 框架无关 |
  | **B 纯 UI 侧（暂缓）** | 1-8 过程块显示文件名、1-9 前后对比图（展示部分）、1-11 信息条分类统计、1-12 PowerShell 历史展不开、3-27 代理面板三段式、3-28.2 活动面板分离、3-28.3 计划步骤可编辑、3-19 / 3-20 的界面验收、§待办 2 全部、1-1/1-3/1-4/1-5 的真机点击部分 | 换 Flutter 会整体重写，先做等于白做 |
  | **C 混合（后端先做，UI 后补）** | 1-10 卡片与总结的顺序、3-28.4 错误渲染、3-28.1 三态按钮、1-13 合并后前端按名字特判的同步 | 后端契约稳定后 UI 再接，接口不随框架变 |
- **建议顺序**：① 提示词两项（1-7 + 1-10）——零前端改动、直接提升对话可读性；
  ② 真机 4 类互操作；③ 1-13 工具合并（动固定前缀，需单独一批）；④ 其余 AI 侧。
- **不在本轮**：Flutter 工程落地、Linux 平台模块（自启 / 系统代理 / 抓屏 / 语音 / OCR）、
  插件层的脚本化设计 —— 待 UI 侧正式开工时另立条目。

## §待办 2 已实现、只差一次真机验收

> **2026-10-23 用户裁定：本节所有验收暂时不做**（真机 / 浏览器实测类一律暂停）。本节仅作台账保留，恢复时机由用户指定。

| # | 项 | 差哪一步 |
|---|---|---|
| **M2-1** | 卸载时随 NSIS 清理插件目录 | `nsis-hooks.nsh` 已加 `RMDir /r "$INSTDIR\Modules"`，**没重打包验证**；验完把 `Modules` 补进 §11 规则 12 的卸载清单 |
| **M2-2** | 开机自启守卫真机闭环 | 现有「dev 构建拒注册 + 能查旧注册项 + 一次修复入口」；**要安装版在登录时触发才算数** |
| **M2-3** | 任务抽屉与语义色按钮的视觉 / 手感 | 抽屉真实对话显隐、待办与已改文件两列表**同时展开**、语义色按钮没被盖掉——未做浏览器实测 |
| **M2-6** | 中断后「飞行中」的命令卡停在「执行中」 | 代码已完成（三处调 `markFlyingCardsStopped`）；**待真机确认**三条路径卡片都变黄「已停止」 |
| **M2-7** | 2026-10-01 批（内置技能 / AGENTS.md / MCP 远端 / 快照落盘） | ① ② ③ ⑤ 已端到端验收；**④ 快照落盘**（多会话 UI 回退）仍需真机 UI |
| **M2-8** | A17 结果卡上的凭据告警 | `cargo test` / `tsc` 已过，**无真机闭环**：让模型 Write 含私钥内容 → 结果卡应多一块红色告警 + 规则 / 行号、且自动摊开；Edit 删掉凭据时那块不应出现 |
| **M2-9** | 四类自定义颜色覆盖验收 | **已实现（2026-10-11）**：文字 / 图标 / 描边（`--border-glass` / `--ctx-border-glass`）/ 中性底（`--shade-rgb`）均已并入四类（`main.ts` 的 `applyFourClassNeutrals`），`tsc` 已过。**只差真机验收**：把「风格 → 底色 / 按钮线条 / 按钮背景 / 按钮蒙版（旧称「其他颜色」，2026-10-11 改名）」四类**全设 `#000000`** + 文字明度拉到极端，逐界面（搜索栏 / 结果区 / AI 输入栏 / 设置各页 / 各插件面板）扫，不应再出现非黑的按钮 / 拉条 / 文本框 / 文字 / 图标 / 描边颜色；**白名单**（语义色 / 音乐在播 / 内容色 / 取色器 / 反光白）见 `ui-style.md` §界面 11。**下拉框（2026-10-11 新增 §界面 12）**：设置里在用的自绘 `.custom-select` 已收编（本体走底色 + 按钮线条，选项悬停 / 选中走按钮蒙版 `-nav-alpha`）；全局滚动条：**2026-10-11 第 5 轮撤销「轨道描边」**（用户裁定此前要求是误判）⇒ 回归「**滑块实心无 border、轨道不描边**」（§界面 6 / §界面 12 已改，滚动条不再关联四类颜色；range 滑块本就无 border，其轨道亦不描边）；现存原生 `<select>` 仅剩 translate / pet（弹出层不可样式化，属豁免；**用量成本三处已于 2026-10-23 改自绘 `.custom-select`**，见 `ui-style.md` §12.6），pet 的三处（热键行 / 动作行 / 动作链步骤）收合面已复核**未被压掉**并按 §界面 12 对齐（弹出层仍不可控）；**数字输入框（2026-10-11 新增 §界面 13）**：原生 spinner 不可美化 ⇒ 全局隐藏 + 自绘 `.num-stepper` 规范 —— 待真机确认上述观感。**音乐插件三处收编（2026-10-11 第 4 轮）**：播放 / 扫描进度条填充改走按钮线条、轨道改走底色派生；播放模式 / 随机按钮线条归按钮线条、激活态归按钮蒙版；选中歌曲选中框（`.music-tr.sel` / `.music-mt.sel`）改走按钮蒙版（正在播放仍靠文字强调区分）；music `version 0.9.35→0.9.36` 已重打包 —— 同样待真机按「四类全设黑」复扫 |

## §待办 3 按需重估（不做只因优先级，不因没价值）

### §待办 3-1 A13 Agent 能力缺口剩余项
- **已查清**：MCP 的 Streamable HTTP / OAuth 2.1 / prompts / roots / elicitation / `.mcp.json`（项目级）+ 信任门 + 配置改动检测自动重启**均已落地**；内置技能 / 工具模板、`AGENTS.md` 项目记忆**多层 / 向上递归已落地（2026-10-11，见下 3-1a）**。
- **状态（2026-10-23）：本节全部完成** —— 最后两项 `sampling`（见 3-1f，2026-10-11 已实现并解除休眠）与 `WebBrowser`（见 3-1i，2026-10-23）均已落地。逐项对照：**记忆目录子目录级**（3-1a）/ **斜杠命令三件**（3-1b）/ **`NotebookEdit`**（3-1c）/ **`CronCreate/Delete/List` 阶段 0+1**（3-1d；**写类 cron 用户裁定不做**）/ **`EnterWorktree` / `ExitWorktree`**（3-1e）/ **MCP `sampling`**（3-1f）/ **`SendUserFile` / `PushNotification`**（3-1g）/ **`LSP`**（3-1h）/ **`WebBrowser`**（3-1i）。**已废弃的 HTTP+SSE 双端点传输**仍不做。
- **涉及文件**：`core-agent/src/tools.rs`（`NotebookEdit` / `EnterWorktree` / `ExitWorktree` / `LSP` / `WebBrowser` 的工具定义与分派）、`core-agent/src/lsp.rs`、`core-agent/src/webbrowser.rs`、`core-agent/src/main.rs`（`mod lsp` / `mod webbrowser`）、`app/src/main.ts`（工具黑名单候选）、`core-agent/src/mcp.rs`、`skills.rs`。

#### §待办 3-1a 记忆目录：多层 / 向上递归（固定前缀层**已实现**）+ 子目录级（**已实现（2026-10-12）**，注入层单测**已补（2026-10-13）**）

- **目标**：把项目记忆从「只读工作目录下那一份 `AGENTS.md`」扩成「多层 / 向上递归」。
- **已查清（现状，2026-10-11，逐条可复核）**：
  - **「项目级那层」原来怎么落地**：只读**工作目录**（cwd = agent 进程 cwd；宿主 `spawn_child` 的 `current_dir`，`app/src-tauri/src/commands.rs:261` / 有效工作目录 `:372`）下的 `AGENTS.md`，**启动读一次**（`core-agent/src/main.rs:1542` `read_agents_md` → `:1639` `project_block`），**拼进 `env_section`**（`:3488-3492`）⇒ 进**主提示词 / 子代理 / 复盘三份系统提示词的固定前缀**；**mtime 热重载无**（改完要重启 agent，与 persona 同纪律）；上限 8000 字符、超长截断、空文件零字节。
  - **各注入通道的层归属（本设计的地基）**：① `config\persona.md`（`LUNAC_PERSONA_FILE`）与 `env_block`（工作目录 / 宿主 / 技能目录 / 插件目录）**都在固定前缀**（persona 只进主提示词；env_block 进三份）；② 长期记忆 `ModuleData\memory\MEMORY.md` 走 `context_block_message`（`:1695`）作为**一条 `user` 消息**（**可变层**）；③ `LUNAC_WORKSPACE_LOCKED` 是**策略闸**（`tools::guard`），根本不是提示词。
  - **cwd 怎么定、可不可变**：cwd = 宿主 `effective_workdir()`（配了工作区就用它，否则 `<exe 根>\temp\transStorage`），spawn 时 `current_dir` 定死，**进程内不可变**；`--add-dir` 只加「可访问目录」（工作区边界），不改 cwd。**agent 没有「当前正在操作哪个目录 / 文件」的状态**（工具只收到显式路径）⇒ 「子目录级指令」只能靠**每次工具调用去扫路径所属目录**才发现。
  - 全仓 grep `AGENTS.md` / `agent-instructions`：**只有这一处单层实现，无多层半成品**（`.mcp.json` 同为「cwd 下单个项目级文件」）。
- **「哪一级算数」的裁定（本设计的核心交付）**：
  - **扫描范围**：从 cwd **逐级向上到盘根**，每一级只要有**非空 `AGENTS.md`** 就收（`collect_agents_md`，`:1558`；层级上限 12 防御）；「项目级 / 父级」= cwd 之上的那些级，「本级」= cwd 自己那一份。
  - **优先级 / 覆盖**：**都注入、后来者优先** —— 顺序**浅→深**（父级在前、cwd 那一份在最后），越深越具体 ⇒ 冲突时**深层赢**；多份时每份前加一行来源路径便于判读。
  - **加载时机**：**启动时扫一次**（`project_block` 在启动装配处调一次，`:3491`）⇒ 进**固定前缀**；cwd 恒定 + 文件集合读一次即定 ⇒ **进程内逐字节稳定、不废缓存**。**子目录级**那层只能**懒加载**（进消息、绝不动固定前缀）——**已实现（2026-10-12）**（见下；注入层单测**已补（2026-10-13）**）。
  - **体量与截断**：单份 ≤ 8000 字符（截断 + 日志 warn）；**总上限 16000 字符**（`MAX_AGENTS_TOTAL_CHARS`，`:1537`），超时**丢最浅的**（保留更贴近工作目录的）、日志 warn。
  - **可观测**：启动一条 INFO 日志列出**这次用了哪几层**（份数 + 逐份绝对路径，`project_block`）；报文级证据用 `-DumpApi` 看请求体 `system`。
- **已实现（2026-10-11，只做能稳进固定前缀的确定集合）**：`core-agent/src/main.rs` 的 `collect_agents_md`（浅→深收集）/ `project_block_from_chain`（单份时输出与单层**逐字节相同**；多份带来源标签；总量超限丢最浅）/ `project_block`（读盘 + 拼装 + 记日志）；**沿用既有 `AGENTS.md` → `env_section` 通道，未新造第二套记忆 / 指令机制**。单测 5 条（多层顺序、前缀稳定、单份截断、空文件零字节、总量丢最浅）。
- **已实现（2026-10-12）· 子目录级「按需加载」（注入层单测**已补（2026-10-13）**）**：需要 ① agent 侧在**每个读写类工具入口**新增「工具路径 → 所属目录」判定点（`tools.rs` 的 `path_arg_kind` / `path_arg_key` / `touched_dir` / `eligible_subdir`，2026-10-12 落地）；② 注入到**可变层**（拼进已有 `tool_result` 文本内部，或一条 `user` 消息），**绝不动固定前缀**；③ 去重（每个目录的 `AGENTS.md` 每会话只注一次）与体量预算。代价 = 跨工具入口的钩子 + 与 ai-spec 那条「动 history **尾部**块结构会打乱缓存单元」的实测红线对齐，属**独立设计**。
- **注入层单测（2026-10-13 补，`core-agent/src/main.rs` 的 tests，临时目录夹具、未改生产代码）**：`subdir_memory_hit_is_injected_into_its_tool_result`（单一命中 ⇒ 拼进对应 `tool_result`、带 `SUBDIR_MEMORY_HEADER` 与 `### <绝对路径>`、原文本保留在前、未新增消息 / 块）；`subdir_memory_is_injected_once_per_session_and_restored_after_set_history`（同目录每会话只注一次 + `set_history` 回退后能补回）；`subdir_memory_stops_at_session_dir_cap`（达 `MAX_SUBDIR_MEMORY_DIRS=16` 停注；`warn` 是日志副作用、未在单测断言）；`subdir_memory_skips_ineligible_dirs`（超深度 / 在 `SKIP_DIRS` / 等于 cwd 均不注）；`subdir_memory_and_background_result_share_one_tool_result`（子目录注入与后台结果回流共存于同一块，顺序子目录先、后台结果后——后台落点是内联在 `run_query` 里的、非纯函数，故复刻其最小追加逻辑验证）。
- **脚本与真机验收口径**：
  - **脚本（已跑，exit 0）**：`scripts\agent-e2e.ps1`（本次新增 `-WorkspaceDir` 夹具开关，见 `docs/testing.md` §测试 10；**脚本默认瞄准 release 产物**，2026-10-11 已在 release 上重跑通过）——夹具 `F\AGENTS.md` + `F\sub\AGENTS.md`、cwd = `F\sub`；`-DumpApi` 后 `api-dump.jsonl` 的请求体：`system` 含**两份标记且浅在深前**、`messages` **不含**（⇒ 进固定前缀不进消息）；同一进程两次请求的 `system` **sha256 逐字节相同**（前缀不抖）；默认工作目录（无 `AGENTS.md`）`system` **不含**该段（与「没这个功能」等价）。
  - **真机**：`<exe 根>\temp\logs\agent-*.log` 应出现 `项目记忆已注入系统提示词固定段：N 份、… [路径…]`；改 `AGENTS.md` 后需**重启 agent** 才生效（与 persona 同一条纪律）。
- **涉及文件**：`core-agent/src/main.rs`（`AGENTS_MD_FILE` / `AGENTS_MD_HEADER` / `MAX_AGENTS_TOTAL_CHARS` / `collect_agents_md` / `project_block_from_chain` / `project_block` + 5 条单测）、`scripts/agent-e2e.ps1`（`-WorkspaceDir`）。

#### §待办 3-1b 斜杠命令（会话内操作子集：`/rewind` + memory 类 + `/compact`，均已实现）

- **目标**：把「与能力相关的一小撮」会话内操作做成斜杠命令（`/rewind`、`/memory`、`/memory clear`、`/help`），**命令不进模型**（不当普通文本送出去）。
- **已查清（2026-10-11，逐条可复核）**：
  - **输入到 agent 的链路**：聊天输入框提交 = `sendChatMessage()`（`app/src/main.ts:3935`）→ `startAIChat()`（:10288）→ `startAgentChat()`（:9813）→ 拼 `{"type":"user",…}` 并 `invoke("send_message")`（:10200）→ 宿主 `send_message`（`app/src-tauri/src/commands.rs:854`）→ `cli_bridge::write_to_cli` → agent stdin。**全链路上原本没有任何 `/` 前缀处理**（前端 grep `startsWith("/")` / `slash` 均无命中）。
  - **`/compact` 的外部触发入口（2026-10-11 补齐）**：agent 侧压缩 `compact_history` 原先只在**预算闸 / 400 兜底**自动触发（`core-agent/src/main.rs` 三处 `fire_plain_hook("PreCompact", …)` 之前）；stdin 主循环原先只认 **`user` / `set_history` / `control_response`** 三种输入类型。**2026-10-11 新增第四种 `{"type":"compact"}`**，把「立刻压一次」做成外部入口（复用 `Force` 档，见「已实现」）。`core-agent/src/decide.rs` 里那处「`/` 开头 ⇒ 当斜杠命令」仍只是 **S1 本地决策桩**，与本改动无关。
  - **`/rewind` 的既有能力**：前端已有 `rollbackChat(idx)`（`app/src/main.ts:2320`，闭区间语义：保留 idx、丢弃其后；含**文件快照还原** + 会话落盘 + 历史回灌 agent + 整块重绘），按钮路径由 `onRollbackClick()`（:2427）做两段式确认、`retryMessage()` 用 `rollbackChat(idx-1)`。**可直接复用**。
  - **memory 类既有能力**：真相源 = `<exe 根>\ModuleData\memory\MEMORY.md`，读写只有 `app/src-tauri/src/mcp_server.rs` 的 `read_memory` / `write_memory_at`（桥方法 `lunac/memory_read` / `_write`，**agent ↔ 宿主内部桥，前端直接调不到**）；**前端原本没有查看 / 清除记忆的入口**；宿主也没有把记忆 pub 成 `#[tauri::command]`。
- **已实现（2026-10-11）**：
  - **拦截点** = 聊天输入框提交这一处（`sendChatMessage` 开头）；命令**不发模型、不写 `chatHistory`**（旁注走独立 `.sys-note` 节点，同「上下文压缩提示」的纪律）。
  - **命令集（本轮实现）**：`/rewind`（撤销最近一轮用户提问，复用 `rollbackChat`）、`/memory`（查看当前记忆）、`/memory clear`（清空记忆）、`/compact`（立刻压缩一次上下文，见下）、`/help`（命令清单）。
  - **`/compact`（2026-10-11 落地，此前只「识别 + 提示 + 不发模型」）**：
    - **agent 侧新增一种 stdin 输入类型 `{"type":"compact"}`（最小改法）**：主循环 match 加一臂（判定抽成 `stdin_kind()`，`core-agent/src/main.rs`），命中即调 `manual_compact()` —— **复用既有 `compact_history(history, Compact::Force, measured)` 路径**（它只是 `Force` 档的第三个调用点，前两个 = 预算续命 / 400 兜底），**未复制任何压缩逻辑**；随后 `pin_summary_of_dropped()` 与自动路径同款。**无 Force 变体问题**：`Compact::Force` 本就是「立刻压、不留情」的档，直接复用。
    - **回执事件**：沿用既有 `system/context_compacted` 形状（字段 `elided` / `dropped`，前端**早已**能渲染该事件与「压缩上下文」阶段浮标）——**未新造字段**。注：既有形状只有这两个**计数**字段，没有 token 量字段；按「不新造字段」约束，回执只带这两个计数（`compact_history` 内部有 `elided_chars`，但它不进事件、只进日志）。
    - **用户已裁定（2026-10-11，两条，按现状定稿、无需改代码）**：① **回执不带 token 量** —— 现有事件形状里没有该字段，按「不新造字段」约束保守处理；若日后要带，需放宽该约束并同步前端渲染。② **`plan` / `read_only` 档下允许 `/compact`** —— 与自动压缩路径语义一致（只改内存 history、不写盘、不新增落点）。另两条现状亦确认不变：**并发取「排队」**（不打断流式/审批）、**单发 `--print` 下 `compact` 天然不可达**（无常驻 stdin）。
    - **护栏**：① **空历史**：不调 `compact_history`（`Force` 会给空历史插一条 `TRIMMED_MARKER` 合成消息，属无中生有）⇒ 直接回 `elided=0/dropped=0` 的回执，不崩、不污染；② **压不动**（`elided+dropped==0`）：`compact_history` 只在真压动时才发事件，故由 `manual_compact` **补发一条 0/0 回执**，保证前端「已请求压缩」占位一定落地（每命令恰好一条回执，不重复）；③ **`read_only` / plan 档**：**允许**压缩 —— 它只改内存里的 `history`、不写盘、不新增落点，自动路径（预算闸 / 400 兜底）同样不看 `read_only` ⇒ 保持同一语义，`PreCompact` hook 照发（`trigger=manual`）；④ **并发**：`manual_compact` 在**主循环**里执行，跑回合时主循环正阻塞在 `run_query` ⇒ 压缩期间到达的 `compact` 只在 stdin 队列里**排队**、**等本回合收尾后才执行**（不打断流式 / 审批）——**选最安全的排队**，不拒绝、不抢占。
    - **前端**：`runSlashCommand` 的 `compact` 分支改为 `runSlashCompact()` —— 走既有 `invoke("send_message")` 把 `{"type":"compact"}` 写进 agent stdin（宿主 `send_message` 已能写任意一行 JSON，**未改宿主**）；先旁注「已请求压缩，稍候…」，发送失败旁注失败提示。命令**不进模型、不写 `chatHistory`**。`/help` 清单里 `/compact` 改为「立即压缩上下文」。
    - **不破坏既有**：不带 `compact` 时 stdin 行为逐字节不变（`stdin_kind` 与原 `match msg.get("type")…` 判定逐字一致）；未知类型仍落 `Unknown` 打「忽略输入类型」；自动压缩路径（预算闸 / 400 兜底）与其 `PreCompact` hook 一行未动。
  - **解析规则**：① 首字符必须是 `/`（trim 之后）；② 命令名 = 到第一个空白为止、**大小写不敏感**；③ **命令名命中已知集合才拦截**，未识别的 `/xxx` **一律当普通文本发给模型**（理由 = 宁可交给模型，也不吞用户内容；正常文本里的 `/` 不命中集合，故不会被误判）；④ 参数按空白切分、忽略多余空白；⑤ `/memory` 空参或 `show` = 查看，`clear` = 清空，其它子命令 → 用法提示。
  - **破坏性命令二次确认**：`/rewind`、`/memory clear` 都走**就地二次确认**（`slashConfirm()`，复用 `.run-mode-confirm` 形态 + 语义红确认按钮，与「切自动档」同款；措辞写明「不可撤销」）。
  - **宿主命令（新增两条，均为既有逻辑的 pub 包装，未新增落点 / 未改格式与上限）**：`get_memory`（只读，走 `mcp_server::memory_read_text` → 既有 `read_memory`）、`clear_memory`（走 `mcp_server::clear_memory_text` → 新增 `clear_memory_at`，把 `MEMORY.md` **截断为空**、文件不存在为幂等空操作）。**单测** `mcp_server::tests::memory_clear_truncates_and_is_idempotent`（临时目录，不碰真实 ModuleData）。
  - **UI / 文案**：旁注 = `.sys-note sys-note-line`；记忆查看 = `<details class="sys-note">` + `pre.sys-note-body`（可展开，避免 6000 字铺屏）；确认条 = `.run-mode-confirm`；**颜色全走四类变量 / 语义色白名单**（未新增任何字面量颜色、未新增滚动条）；文案全走 `t()`（`chat.slash_*`，五语言）。
  - **可观测**：`console.log("[slash] …")`（devtools）+ 界面旁注（执行了哪个命令、结果如何）。
- **做不到 / 需后续（如实记，本轮不做）**：
  - **命令候选菜单 / 自动补全**：未做（项目无「输入 `/` 弹候选」先例）。最小发现方式 = 解析执行 + `/help` + 文档；若日后要做候选浮层，需按 `ui-style.md` §界面 14 形态、颜色走四类变量。
- **验证（2026-10-11，以 release 为目标）**：
  - `npx tsc --noEmit`（cwd=`app`）exit 0；`npm run build`（cwd=`app`）exit 0；`npm run verify`（仓库根）exit 0。
  - `cargo check --bins` / `cargo test --bins`（cwd=`app\src-tauri`）exit 0（**208 passed**，含新单测 `memory_clear_truncates_and_is_idempotent`）。
  - `cargo build --release`（cwd=`app\src-tauri`）exit 0（Finished release profile）。**未改 `core-agent` ⇒ 未重编 agent、未跑 `agent-e2e.ps1`**（本功能是宿主 + 前端，脚本无前端、测不到）。⚠️ **宿主 release 需单独在 `app\src-tauri` 编一次**（不会随 core-agent 的 release 构建更新）。
  - **本轮（`/compact`，2026-10-11）**：`cargo test`（cwd=`core-agent`）exit 0（**170 passed**，含新增 3 条：`stdin_input_types_are_classified` / `manual_compact_on_empty_history_receipts_zero_without_pollution` / `manual_compact_receipts_zero_when_nothing_to_compress`）；`cargo build --release`（cwd=`core-agent`）exit 0（Finished release profile；2 条既有 `dead_code` 告警非本轮引入）。`npx tsc --noEmit` / `npm run build` / `npm run verify` 全 exit 0。
  - **协议级实跑取证（本轮重点，已跑，exit 0）**：`scripts\agent-e2e.ps1 -RawStdin <2 行文件> -ExpectEvent context_compacted -Model deepseek-flash` —— 文件首行一条 `user`（PONG），第二行 `{"type":"compact"}`。实测：`stream.jsonl` 出现 `{"dropped":0,"elided":0,"subtype":"context_compacted","type":"system"}`；`summary.json` 的 `system_events=["init","context_compacted"]`、`failures=[]`、`exit_code=0`；`stderr.log` **无**「忽略输入类型」；agent 日志 `INFO 手动压缩：瘦身 0 个 tool_result，丢弃 0 条旧消息`（本轮是「短历史 ⇒ 无可压缩」，正好命中「补发 0/0 回执」那条护栏；该次 `-PermissionMode plan` 只读档也一并证明 read_only 下允许压缩）。脚本新增 `-RawStdin`（原样写 stdin）与 `-ExpectEvent`（断言 system 子类型），见 `docs/testing.md` §测试 10。
- **真机验收口径（待做，release 目标）**：起 dev 实例（`cd app ; npm run tauri:dev`）→ 开聊天独立窗，逐条：
  1. `/help` 回车 → 对话流出现命令清单旁注；**未向模型发问**（状态栏不进入「思考中」、无 token 消耗）。
  2. `/compact` → 先旁注「已请求压缩，稍候…」，随后 agent 回 `context_compacted` 的「上下文已压缩」旁注（短历史 / 空历史时为 0/0）；**未向模型发问**（无「思考中」、无 token 消耗）。压缩期间正有回合在跑 ⇒ 该命令**排队**，等回合收尾后再压。
  3. 先问一句拿到回答，再 `/rewind` → 出现语义红确认条；点「确认」→ 最近一轮被移除（与点回退按钮同效、含文件还原）；点「取消」→ 无变化。
  4. `/memory` → `MEMORY.md` 有内容时出现可展开的「长期记忆（当前内容）」；为空时旁注「长期记忆为空」。
  5. `/memory clear` → 出现确认条；确认后 `MEMORY.md` 被截断为空、旁注「已清空长期记忆」；取消则不动。
  6. 未识别（如 `/foo`）→ **当普通文本发给模型**（不拦截、正常得到回答）。
  7. 大小写：`/REWIND`、`/Memory Clear` 同样识别；`/memory   clear`（多空白）同样识别。
- **涉及文件**：`app/src/main.ts`（`SLASH_COMMANDS` / `parseSlashCommand` / `runSlashCommand` / `runSlashCompact` / `runSlashRewind` / `runSlashMemory` / `appendSlashNote` / `appendSlashDetails` / `slashConfirm` + `sendChatMessage` 拦截）、`app/src/i18n.ts`（`chat.slash_*` 共 13 键 × 5 语言）、`app/src-tauri/src/mcp_server.rs`（`clear_memory_at` / `memory_read_text` / `clear_memory_text` + 单测）、`app/src-tauri/src/commands.rs`（`get_memory` / `clear_memory`）、`app/src-tauri/src/main.rs`（导入 + 注册两条命令）、**`core-agent/src/main.rs`（`StdinKind` / `stdin_kind` / `manual_compact` + 3 条单测；主循环 match 新增 `compact` 一臂）、`scripts/agent-e2e.ps1`（`-RawStdin` / `-ExpectEvent`）**。

#### §待办 3-1c `NotebookEdit`（按 cell 编辑 `.ipynb`，**已实现**，2026-10-11）

- **目标**：按 Claude Code 口径按 cell 编辑 Jupyter notebook（`.ipynb`），**不让模型拿 `Write` / `Edit` 去改结构化 JSON**。
- **标准语义（已照此实装）**：`notebook_path`（必填）+ `cell_id` / `cell_number`（0 基，二选一定位；`cell_id` 优先）+ `new_source`（必填，replace / insert 用；delete 忽略）+ `cell_type`（`code` / `markdown`，insert 必填）+ `edit_mode`（`replace` 默认 / `insert` / `delete`）。insert 用 `cell_id` 时插到该 cell **之后**，用 `cell_number` 时插到该下标（= cell 数 ⇒ 追加）；replace 传 `cell_type` 时顺带切类型。
- **已实现**：
  - `core-agent/src/tools.rs`：`defs()` 新增 `NotebookEdit` schema（`Read/Write/Edit` 之后）；`notebook_edit()` 实现三档；`needs_approval` **要问**（写类）；不进 `parallel_safe`（写盘）、不进 `gated_in_read_only`（只读档直接拒，拒因指向「去设置里改档位」）。辅助：`locate_cell` / `source_value` / `build_notebook_cell` / `random_cell_id` / `detect_indent`；`NotebookMode` 枚举。
  - `core-agent/src/main.rs`：`written_payload` 增加 `NotebookEdit → new_source`（delete 无该字段 ⇒ 不扫）；`security_warnings` / `open_approval` 的写类扫描 / `hook_allow_needs_card` **三处一并覆盖** `NotebookEdit`（与 `Write` / `Edit` 同源，凭据扫描不漏这扇门）。
  - `app/src/main.ts`：`TOOL_BLACKLIST_CANDIDATES` 加 `NotebookEdit`（用户可关）；`WRITE_TOOLS` 加 `NotebookEdit → "notebook_path"`（工具卡可点定位 + 回退可采快照）。
  - **工具计数守门单测** `agent_tool_is_registered_and_filterable`：`18 → 19`（**有意更新数字**，不是删断言）。
- **结构与风格（关键边界，已钉死）**：解析成 `serde_json::Value` → 只改目标 cell → 序列化。key 顺序天然字典序 —— 这正是 Jupyter `nbformat` 的落盘口径（`json.dumps(..., indent=1, sort_keys=True)`）；缩进由 `detect_indent` **沿用原文件**、末尾换行照原样保留 ⇒ **其它 cell / `nbformat` / `metadata` 逐字节不变**。insert 造的新 cell 字段顺序与 Jupyter 一致；用 id 的 notebook（`nbformat_minor ≥ 5` 或已有 cell 带 `id`）才给新 cell 生成 id。
- **错误可读（不崩）**：缺文件 / 非合法 JSON / 顶层不是对象 / 没有 `cells` 数组 / `cell_id` 找不到 / `cell_number` 越界 / `edit_mode` 非法 / insert 缺 `cell_type` / replace·insert 缺 `new_source` / 只读档 —— 均返回清晰 `Err`（成为 `is_error` 的 tool_result）。
- **未做 / 边界（如实记）**：① **不编辑 output / execution_count**（只动 `source`，可顺带切 `cell_type`）；② cell 定位靠 notebook 里的 `id`（nbformat 4.5+），老格式只能用 `cell_number`；③ 不做 nbformat 结构校验（只保 JSON 可解析 + `cells` 存在）；④ 单发 `--print` 下它属写类，默认被拒（需 `--dangerously-skip-permissions`）。
- **验证（以 release 为目标，2026-10-11）**：
  - `cargo test`（cwd=`core-agent`）**exit 0 / 174 passed**（含新增 4 条：`notebook_edit_tool_is_registered_and_gated` / `notebook_edit_replaces_only_the_target_cell` / `notebook_edit_inserts_and_deletes_cells` / `notebook_edit_reports_clear_errors`，以及改判后的 `security_warnings_only_cover_written_payloads`）。
  - `cargo build --release`（cwd=`core-agent`）**exit 0**（Finished release profile；2 条既有 `unused_mut` / `dead_code` 告警**非本轮引入**）。
  - `npx tsc --noEmit`（cwd=`app`）**exit 0**。
  - **端到端实测（`scripts\agent-e2e.ps1`，release，exit 0 / PASS）**：夹具 `.ipynb`（放 `temp\agent-e2e\nbedit-fixture\`，用后已清）2 个 cell；命令 `-WorkspaceDir <夹具> -PermissionMode acceptEdits -Model deepseek-flash -Query "用 NotebookEdit 把 cell edit-me 的 source 改成 print(2)" -ExpectTool NotebookEdit -ExpectContains DONE`。实测：工具池 **19**；工具序列 `[Read, NotebookEdit]`；夹具中 `edit-me` 的 source 变为 `"print(2)"`、`keep-me` 的 `"print(1)\n"` 及其它字段/缩进/结构**未动**。
    - **所选权限档与理由**：用 `-PermissionMode acceptEdits`（**非** `plan`）—— plan 档下写类工具被 `write_blocked` 直接拒；而**常驻 stdio 路径不传 `-StdioApproval`（即无 `--permission-prompt-tool stdio`）** ⇒ agent 对写类工具**不弹卡、直接执行**（脚本无审批回包器）。故**无需也不便**用 `--dangerously-skip-permissions`（该脚本未暴露此开关）。
- **涉及文件**：`core-agent/src/tools.rs`、`core-agent/src/main.rs`、`app/src/main.ts`。

#### §待办 3-1d `CronCreate/Delete/List`（阶段 0 + 阶段 1 **已实现（2026-10-17）**；阶段 2B「无人值守可写」**已实现（2026-10-27）**）

- **结论（2026-10-27 阶段 2B 落地后更新）**：**只读定时任务已自动执行** —— 宿主 30s tick 调度器到点拉起一次 `agent.exe --print --permission-mode plan`（零新增依赖）。阶段 0（只读列表 + 手动触发）、阶段 1（宿主调度 + `CronCreate/Delete` + 每日上限 + 自动停用 + 通知 + 忙态）均已实现；**阶段 2B 已实现（2026-10-27）**：用户可在任务列表里**逐 job** 打开「无人值守可写」⇒ 该 job 的单发换成 `acceptEdits` 档 + `--unattended-allow Write,Edit,NotebookEdit`，从裁剪名单里只移出写类三件；命令类 / 外部出口 / 派生任务 / MCP 一律照旧被裁。**2A（`--dangerously-skip-permissions`）仍然不做**（见下方阶段 2）。

##### 一、已核实的事实（逐条带行号；文末标出与旧描述的差异）

1. **`agent.exe` 是宿主 spawn 的常驻 stdio 进程，且会被宿主主动杀掉。** 宿主以 `--print --input-format stream-json --output-format stream-json` 拉起它（`app/src-tauri/src/commands.rs:381-396`），agent 侧按行读 stdin（`core-agent/src/main.rs:5009-5019`）。`stop_cli` 走 `cli_bridge::kill_and_cleanup()`（`app/src-tauri/src/commands.rs:873-879`），内部先 `taskkill /F /T` 收整棵进程树、再 `child.kill()` 兜底（`app/src-tauri/src/cli_bridge.rs:81-117`、`:126-155`）。实际触发点包括：
   - 新对话：`app/src/main.ts:3176-3185`；
   - 用户停止 / 中断：`app/src/main.ts:5030-5054`、`:12130-12131`；
   - 退出 AI：`app/src/main.ts:5662`；
   - 换工作区：`app/src/main.ts:3437-3439`；
   - 回退时正在流式：`app/src/main.ts:2520-2527`；
   - MCP 配置改动（回合结束或空闲时重启）：`app/src/main.ts:1029-1034`、`:9709-9710`、`:10019-10022`；
   - 技能 / 工具黑名单 / 人格类「重启 AI」：`app/src/main.ts:12205-12210`、`:12236-12239`；
   - 换安全档位 / 换思考档：`app/src-tauri/src/commands.rs:1021-1025`、`:2256-2259`；
   - App 退出：`app/src-tauri/src/main.rs:476-518`。
   - **与旧描述的差异（1）**：**换模型不会自动 `stop_cli`**。设置面板保存只调 `set_ai_config`（`app/src/plugins/builtin/settings.ts:3243-3267`、`:3500-3511`），宿主只落盘并注入进程 env（`app/src-tauri/src/commands.rs:1302-1356`）；agent 的模型 / 端点 / key 只在 spawn 时读一次（`core-agent/src/main.rs:2997-3027`）。所以新模型要等下一次 spawn（新对话 / 中断 / 改档 / 重启应用等）才生效。
   - **与旧描述的差异（2）**：「agent 只在有对话时存活」太绝对。agent 是懒启动，但聊天窗关掉后它仍可在跑（`app/src-tauri/src/commands.rs:845-854` 的就绪信号复用路径即为此场景）；它确实会在上面列出的动作和 App 退出时死掉，所以**不能把定时器放在它里面当唯一调度**。
2. **单发模式已存在。** `--print` / `-p` 的判定 = 带 `--print` 且**未**显式声明 `--input-format stream-json`（`core-agent/src/main.rs:4508-4515`）；query 取 `--print` 后紧跟的非 flag 值，否则从 stdin 读一次（`:4533-4546`）。输出形态 `OutputMode` = `text` / `json` / `stream-json`（`:3062-3072`、`:4551-4560`），`json` 只落一条 `result`、`stream-json` 落完整事件流（`:3138-3155`）。**单发没有审批回包通道**：需审批的工具默认直接拒绝（`ApprovalPolicy::UnattendedDeny`，`:5539-5556`、`:8581-8590`），除非 `--dangerously-skip-permissions`（`:4565-4569`）；只要有工具因该策略被拒，退出码非 0（`:5319-5323`）。契约与实测口径见 `docs/testing.md:160-165`。
   - **单发确实支持** `--add-dir`、`--disallowedTools`、`--mcp-server`、`--permission-mode`：解析在 `core-agent/src/main.rs:4432-4506`，应用到 `Ctx` / 工具池在 `:4575-4614`、`:4636-4642`。注意 `--disallowedTools` 是 variadic（吃到下一个 `--flag` 为止，`:4489-4497`），cron 拼参数时要么放最后、要么后面紧跟另一个 flag。
   - **`--mcp-server` 在单发下可用，但不等于 MCP 工具可用**：`mcp__*` 在 plan 档根本不进工具池（`core-agent/src/main.rs:4639-4642`），且全部 MCP 工具一律需要审批（`core-agent/src/main.rs:5530-5536`）⇒ 单发默认拒绝。它真正的用途是接通宿主内部桥方法（`SessionSearch` / `MemorySearch` 等，`core-agent/src/tools.rs:866-884`）。
3. **cron 已接上检查点 / `--resume`（2026-10-22）。** 单发 runner 固定加 `--checkpoint`；job 有上一次的 `lastSessionId` 且检查点还在时追加 `--resume <id>` ⇒ **不再是每次全新会话**。会话 id 存在 `cron-state.json` 的 `lastSessionId`；带 `--resume` 仍失败时清掉它（下一轮开新会话），检查点被份数上限清理时记一条 info 并开新会话。写类 cron 见阶段 2B（2026-10-27 已实现，逐 job 由用户授权）。
4. **`PushNotification` / `SendUserFile` 已存在。** schema 在 `core-agent/src/tools.rs:785-805`，实现在 `:3411-3520`；两者都**不需要审批**（`:1132-1140`）。但单发 `--output-format json` 会把非 `result` 事件全部挡掉（`core-agent/src/main.rs:3138-3155`）⇒ 若想拿到 `system/push_notification` / `system/user_file`，cron 必须用 `--output-format stream-json`。另外 `PushNotification` 是**应用内浮层**，不是 OS toast（`app/src/main.ts:9251-9256`）；窗口不可见时用户当场看不到（【推论】，未实测）。
5. **宿主侧没有通用调度器（2026-10-17 时点）。** 阶段 1 后已补：`app/src-tauri/src/cron.rs` 起 30s tick 线程（见本节结论）。 `app/src-tauri/Cargo.toml:10-55` 没有任何 cron / schedule / chrono 依赖；宿主里的 `thread::spawn` 都是索引、播放、窗口、sidecar 等一次性或短周期设施（如 `app/src-tauri/src/file_indexer.rs:210-231`、`app/src-tauri/src/app_indexer.rs:106`），没有「到点执行任务」的循环。agent 侧唯一的定时 tick 是 MCP 配置监听的 `MCP_WATCH_TICK = 1s`（`core-agent/src/main.rs:3179`、`:5076`），不是宿主调度器。
   - **与旧描述的差异（3）**：没有 `tauri-plugin-autostart`。开机自启是自研 `app/src-tauri/src/auto_start.rs`：优先用 `schtasks /sc onlogon` 建一个名为 `Lunac` 的任务，失败回退 HKCU Run（`auto_start.rs:1-16`、`:25`、`:82-150`），且计划任务创建 / 删除需要管理员（`:178-187`）。它只做「开机拉起 Lunac」，**不是 cron 调度**。
   - **未查到**：任何宿主侧的 `WM_POWERBROADCAST` / 睡眠唤醒处理；调度只能靠墙钟比较（见 C）。
6. **数据根与 `ModuleData\tasks\`。** 数据根 = `<exe 根>`：`config\` 是应用配置（ai / hotkey / hooks / pricing / local-model 等），`ModuleData\` 是业务数据（`app/src-tauri/src/storage.rs:3-21`、`:40-43`；`docs/code-standards.md:21-22`）。**当前 `ModuleData\tasks\` 并不存在**：对 `app/src-tauri/target/{debug,release}/ModuleData` 的实际目录列表没有 `tasks\`，Rust 源码里也没有 `tasks` 路径；S2 只是计划「任务队列 + 检查点落 `ModuleData\tasks\`」（`docs/todo.md:1563`），尚未实现。
   - 时间：项目**不引 `chrono`**（`app/src-tauri/src/log.rs:15`）；宿主已有 Win32 `GetLocalTime` 取本地日期（`app/src-tauri/src/mcp_server.rs:689-693`），`windows` crate 的对应 feature 已在依赖里（`app/src-tauri/Cargo.toml:51-54`）。这决定了下面的「简化 schedule」方案。

##### 二、A. 三条实现路线对比与推荐

| 路线 | 做法 | 优点 | 代价 / 风险 | 与既有约束的冲突 | 新依赖 | 结论 |
|---|---|---|---|---|---|---|
| A1 agent 进程内定时器 | 复用主循环 tick（`core-agent/src/main.rs:3179`、`:5076`）+ 进程内 `static Mutex<Vec<CronJob>>` | 改动最小；无需新进程；无落盘 | 新对话 / 中断 / 换档 / MCP 改动 / App 退出都会杀 agent（见事实 1）⇒ 定时器丢；宿主不启动时不存在；跨进程不持久 | 与「换档 / 新对话必须重启 agent」的既定纪律直接冲突；无法兑现「定时」承诺 | 无 | **不成立**（最多做会话内演示，不建议） |
| A2 宿主调度 + `--print` 单发 | Rust 常驻线程到点 spawn 一次 `agent.exe --print`（独立子进程） | 宿主生命周期长；配置 / 工作区 / 日志 / 用量都在宿主手里；单发审批 fail-closed（默认拒绝）；子进程可超时杀掉；零新依赖；可先做手动触发验证 | 只在宿主运行时有效；每次是新会话（无上下文）；需审批的工具默认被拒；需新增调度线程 / 存储 / UI；工具表变更要让 agent 重启一次 | 必须自己实现并发闸、频次上限、超时、日志脱敏；不能把任务表写进 system（§禁用 45）；不能进 `chat.db`（§禁用 61） | 无 | **推荐** |
| A3 Windows 计划任务（schtasks） | 每个 job 建一条计划任务，到点直接跑 `agent.exe --print` | 宿主不启动也能跑 | 创建 / 删除需管理员（`auto_start.rs:178-187`）；任务在系统里可见；卸载残留（NSI 无 schtasks 清理，仓库搜索零命中）；每 job 一条任务难维护；无审批 / 无 UI | 撞「卸载 = 完全不存在」的精神（`docs/forbidden.md:72`，§禁用 51）；**致命**：agent 只读 env 凭据（`core-agent/src/main.rs:2997-3010`），计划任务的 `/tr` 里要么带 key（写进任务 XML，违反 §禁用 58 / 53）要么另造注入器 | 无（schtasks 是系统自带） | **不做** |

- **推荐 A2**。理由：它把「定时」放在宿主这个本来就长期存活的进程里，同时把「执行」隔离到一次性子进程；单发没有审批回话通道，天然适合「默认拒绝需审批工具」的无人值守语义（`docs/testing.md:164`、`core-agent/src/main.rs:5539-5556`）。代价是必须诚实承认：**宿主没启动就不会跑；每次都是新会话**。
- **一个容易忽略的实现前提**：cron 子进程必须拿到与常驻 agent 同一套 env（端点 / key / 模型 / 技能目录 / Modules / hooks / persona / MCP / 日志 / 工作区锁）。当前这套拼装在 `start_cli_process` 内部（`app/src-tauri/src/commands.rs:429-529`），需要抽成**一处**共享构造函数供两条 spawn 路径复用，避免出现第二份 env 逻辑（§禁用 62）。

##### 三、B. 数据模型与落点

- **落点结论：`ModuleData\tasks\cron.json`（job 定义）+ `ModuleData\tasks\cron-state.json`（运行态）。**
  - 理由 1：job 的 `prompt` 是用户内容、`runCount` / `lastStatus` 是运行产生的业务数据；按 `storage.rs:19-21` 的划分，「业务数据才进 ModuleData」，`config\` 只放应用配置。放进 `config\` 会把「用户内容 + 运行计数」混进配置区。
  - 理由 2：便携式数据根 = exe 根，卸载即随安装目录清除，天然不留外部残留（与 §禁用 51 的「卸载 = 完全不存在」同一精神）。
  - 理由 3：**不进 `chat.db`**（§禁用 61）。cron 不是会话消息，也不该被会话清理 / 检索 / 回灌逻辑波及。
- **为什么要拆成两个文件（单写者原则）**：
  - `cron.json` 只由 **agent 侧** 的 `CronCreate` / `CronDelete` 写（原子写：同目录 `*.tmp` 后 rename）；
  - `cron-state.json` 只由 **宿主调度器** 写（last/next/计数/心跳/自动停用）；
  - 宿主每 tick 重读 `cron.json`（按 mtime 或直接读），**从不写它**；agent 只读 state，**从不写它** ⇒ 避免两个进程对同一 JSON 的读改写丢失。
  - 路径的**唯一真相源**在宿主 `storage.rs` 的 `cron_dir()`（`module_data_dir().join("tasks")`），并作为 `LUNAC_CRON_DIR` 注入 agent；agent 不硬编码路径（同 `LUNAC_MODULES_DIR` 的既有做法，`core-agent/src/main.rs:4599-4608`）。agent 侧还要把该目录放进 `add_dirs`，否则工作区锁会拒写。
- **`CronJob` 字段草案**（定义文件 + 运行态的合并视图；存储 JSON 用 camelCase）：

| 字段 | 类型 / 默认 | 归属 | 说明 |
|---|---|---|---|
| `id` | string，必填 | 定义 | `cron_<epoch_ms>_<pid>_<seq>`；不引 UUID 依赖 |
| `schedule` | object，必填 | 定义 | **二选一：简化形式，不用 cron 表达式**。`{"kind":"every","everyMinutes":N}` 或 `{"kind":"daily","at":"HH:MM"}`。理由：零新增依赖、无需解析器；cron 表达式的字段序 / DST / 步进边角多，收益不足以抵消 |
| `prompt` | string，1–8000 字符 | 定义 | 每次运行喂给单发 agent 的 query；写入前过 `content_safety::analyze`（`core-agent/src/content_safety.rs:167`），命中凭据即拒 |
| `enabled` | bool，默认 true | 定义 | 用户开关；与自动停用分开（见 `autoDisabled`） |
| `createdAt` | epoch ms | 定义 | agent 侧 `SystemTime` |
| `scope` | `global` / `pet`，默认 `global` | 定义 | **阶段 1 只接受 `global`**；`pet` 需单发消息带 `source:"pet"` 才切记忆作用域（`core-agent/src/main.rs:2754-2759`、`:5002-5007`），留到阶段 2 |
| `workspace` | string，默认 `""` | 定义 | 空 = 运行时用宿主当前有效工作区（`commands.rs:1553-1561`）；**阶段 1 只接受空串或与当前 cwd 相同值**，且要求已配置工作区（见 C） |
| `notify` | `none` / `toast` / `sound` / `toast_sound`，默认 `toast` | 定义 | 结果投递方式；`sound` 复用 `notify_sound`（`commands.rs:1134-1145`） |
| `maxRunsPerDay` | int，默认 12，硬上限 48 | 定义 | 按**本地日历日**计数；`daily` 类自然只能 1 |
| `lastRunAt` | epoch ms / null | 运行态 | 最近一次真正开始的时刻 |
| `nextRunAt` | epoch ms / null | 运行态 | 下一次应运行的时刻；宿主算 |
| `nextRunAtLocal` | string / null | 运行态 | 给 `CronList` / UI 显示（`YYYY-MM-DD HH:MM`，本地时间） |
| `lastStatus` | enum | 运行态 | `never` / `ok` / `error` / `denied_tool` / `timeout` / `skipped_busy` / `skipped_missed` / `skipped_daily_cap` / `skipped_global_cap` / `disabled_after_failures` |
| `lastSummary` | string | 运行态 | 最终文本前 500 字符（脱敏后），只供列表显示 |
| `lastError` | string | 运行态 | 最后一条错误（≤500 字符，脱敏） |
| `runCount` | u64 | 运行态 | 累计成功开始次数 |
| `failureCount` | u64 | 运行态 | 累计失败次数 |
| `consecutiveFailures` | u64 | 运行态 | 连续失败次数；成功清零；达 3 置 `autoDisabled` |
| `autoDisabled` | bool | 运行态 | 自动停用标记；`enabled` 仍保持用户原值，便于 UI 解释与恢复 |
| `todayDate` / `todayRuns` | string / u64 | 运行态 | 本地日与当日已跑次数 |
| `heartbeatAt` | epoch ms | 运行态 | 调度器心跳；`CronList` 据此判断宿主调度是否在跑 |

- **执行历史不落 JSON。** 只留计数 / 最后状态 / 最后摘要；每次运行的完整输出、stderr、usage 写 `temp\logs\cron-<id>-<YYYYMMDD-HHMMSS>.log`（`.log` 扩展名可被既有 7 天清理覆盖，`app/src-tauri/src/log.rs:12`、`:204-226`），避免 `ModuleData\tasks\` 无限增长。日志落盘前过 `mask_secrets`（`app/src-tauri/src/log.rs` 的既有脱敏入口；`docs/forbidden.md:85`）。
- **成本归因**：宿主从 `result.usage` 取 token（不在这里算钱），追加 `ModuleData\usage\usage-YYYY-MM-DD.jsonl`，`source = "cron"`（`UsageRecord.source` 是自由字符串，`app/src-tauri/src/storage.rs:1135-1140`；追加函数 `:1221`）。注意前端成本面板目前只分 `pet` / `chat` 两档（`app/src/usage-cost.ts:150-157`、`:593-602`）⇒ 不改的话 cron 用量会落进「普通聊天」；实现时要同步把 `cron` 加成第三档（含 i18n）。

##### 四、C. 调度语义（逐条定）

1. **时区 / DST**：一律用**本地墙钟**（`GetLocalTime` 已有，`app/src-tauri/src/mcp_server.rs:689-693`；项目不引 chrono，`log.rs:15`）。
   - `daily`：每个 tick 比较本地 `YYYY-MM-DD` + `HH:MM`，并用「本地日期 + `lastRunEpoch` 单调」去重。
   - **春季 DST 跳过的本地时刻**（如 02:30 不存在）：当天**跳过并记 `skipped_missed`**，不顺延到 03:00（避免「用户以为 02:30，实际 03:00」）。
   - **秋季 DST 重复的本地时刻**：同一天只跑第一次，第二次因「本地日期已跑」被去重。
   - `every N`：按 **UTC epoch 的真实间隔**计算，不受 DST 影响（墙钟跳变不会让它多跑/少跑）。中国无 DST，以上主要对海外时区用户生效。
2. **错过的运行**（宿主没开 / 电脑睡眠 / 时钟跳变）：**只补最近一次，且仅当延迟 ≤ 10 分钟；否则跳过并记 `skipped_missed`**，然后把 `nextRunAt` 推到下一个未来时刻。
   - 理由：补跑全部会形成「醒来后一串 API 调用」；补跑很久以前的任务，prompt 往往已经过期（例如「今天早上 9 点的日报」在下午 3 点跑没有意义）。10 分钟宽限覆盖「睡眠刚醒 / 短暂卡顿」。
   - `every N` 同理：醒来时若已过多个周期，只跑一次（若有宽限内到期），随后从「现在 + N」重新起算，不补一串。
3. **并发**：**全局同一时刻只允许一个 cron 运行**；若用户正在对话（`isStreaming` 为真）或已有 cron 在跑 ⇒ **`skipped_busy`，不排队**。
   - 理由：单发子进程虽然技术上可与常驻 agent 并存，但它们共享同一个工作区、同一份配置 / 限流 / 磁盘；排队会导致「用户刚聊完，后台突然连跑三条」且 prompt 过期。跳过并如实记录更可预测。
   - 建议的忙判据：前端在 `startAgentChat` 置真、收尾置假（`isStreaming` 现有生命周期：`app/src/main.ts:1089`、`:11241`、`:11433`），通过一条新命令 `set_agent_busy` 同步给宿主；宿主再用「最近一次 `send_message` / `result`」做兜底。
4. **成本上限**：
   - 最小间隔 **15 分钟**（`everyMinutes >= 15`）；
   - 每 job `maxRunsPerDay` 默认 **12**、硬上限 **48**；
   - 全局每日上限 **48 次**（防止 20 个 job 各跑满）；
   - 连续失败 **3 次** ⇒ `autoDisabled`，不再自动跑，需用户恢复；
   - 全局 job 数上限 **20**，prompt ≤ 8000 字符，定义文件 ≤ 256 KB；
   - **单次没有硬性费用上限**：agent 的 `LUNAC_TURN_BUDGET_TOKENS` 默认 500k 且是可续命的软闸（`core-agent/src/main.rs:196-206`、`:663-669`），不构成硬上限；HTTP 请求超时是 1800s（`:108`）。宿主必须自己加**单次运行墙钟 deadline（建议 10 分钟）**并在超时后杀进程树，否则一次卡住会占住整个调度。
5. **审批（必须诚实）**：单发默认拒绝所有需审批工具（`core-agent/src/main.rs:5539-5556`、`:8581-8590`），包括 `Write` / `Edit` / `NotebookEdit` / `Cmd` / `PowerShell` / `WebSearch` / `WebFetch` / `AskUserQuestion` / `Remember` / `PetGrowth` / `Agent` / `ExitPlanMode` / `ImageGen` / `EnterWorktree` / `ExitWorktree`（`core-agent/src/tools.rs:1081-1130`、`core-agent/src/main.rs:5530-5536`），以及全部 `mcp__*`。
   - **阶段 1 能给用户的承诺只有**：`Read` / `Glob` / `Grep` / `SessionSearch` / `MemorySearch` / `TodoWrite` / `RepoMap` / `SendUserFile` / `PushNotification` 这类**无需审批**的工具可用；**WebSearch / WebFetch 也会被拒**（它们在 `needs_approval` 里）。
   - **不要把它说成「无人值守安全」**：它只是「默认拒绝 + 只读白名单」，不是沙箱（§禁用 1.1）。
   - **`--dangerously-skip-permissions` 不是写类任务的可行解**：① 它在单发下把审批策略改成直接放行（`core-agent/src/main.rs:4565-4569`），`Cmd` / `PowerShell` 的危险命令静态分析只挂在审批请求上（`:3637-3657`），没有人看卡就等于绕过了「危险命令任何档位都要人工确认」（`docs/forbidden.md:14`，§禁用 1.2）；② 它同时把工作区锁关掉（`core-agent/src/main.rs:4613-4614`）。所以「无人值守 + 全放行」既没有边界，也违反硬禁令，**不作为阶段 2 的直接做法**（见 E 的 2A / 2B）。
6. **结果投递**：**宿主自己的「定时任务」列表 + 应用内浮层 + 可选提示音**为主；agent 的 `PushNotification` / `SendUserFile` 作为可选的**次级**通道。
   - 单发用 `--output-format stream-json`，宿主解析 `result`（最终文本 / usage）以及可选的 `system/push_notification` / `system/user_file`；若用 `json`，后两种事件会被吞掉（`core-agent/src/main.rs:3138-3155`）。
   - **不把 cron 结果用 `send_message` 注回常驻 agent**，也不写进主对话 `chatHistory`（否则就是「用户没打过的字」自动进对话，且会把上下文撑大；见 §禁用 45 / §禁用 61）。
   - **没有 OS 级 toast**：项目当前没有桌面通知设施，`PushNotification` 只是应用内浮层（`app/src/main.ts:9251-9256`）。若应用收在托盘、窗口不可见，用户当场可能看不到（【推论】，未实测）；`notify_sound`（`commands.rs:1134-1145`）是唯一能立刻提醒的既有能力。

##### 五、D. 工具契约

- **`CronCreate`**
  - 入参（统一 camelCase，与 B 的字段草案同源）：
    ```
    schedule: { kind: "every", everyMinutes: 15..1440 }
            | { kind: "daily", at: "HH:MM" }          // 必填
    prompt: string (1..8000)                          // 必填
    enabled?: boolean = true
    scope?: "global" | "pet" = "global"               // 阶段 1 只接受 global
    workspace?: string = ""                           // 阶段 1 只接受空或当前 cwd
    notify?: "none" | "toast" | "sound" | "toast_sound" = "toast"
    maxRunsPerDay?: integer 1..48 = 12
    ```
  - 出参：`{ ok, job: { ...合并视图... }, warning: string }`。`warning` 明确写「宿主未运行时不会执行；每次运行是新会话、无上下文；需审批的工具会被拒绝」。
  - **必须审批**：它持续产生 API 调用（成本副作用），且会写持久化文件。除 `tools.rs` 的 `needs_approval` 返回真外，还要在前端 `classifyRequest` 里做**硬挡**（即使自动档也弹卡），位置在 `app/src/main.ts:8241-8256` 的 `AskUserQuestion` / `ExitPlanMode` 同款分支——审批的唯一决策点在前端（§禁用 1.4）。
  - 只读档（plan）下由 `write_blocked` 直接拒（`core-agent/src/tools.rs:324-338`），不是「问了再做」。
- **`CronDelete`**
  - 入参：`{ id: string }`；出参：`{ ok, deleted: boolean, alreadyGone?: boolean }`（**幂等**：不存在也返回成功，避免模型重试）。
  - **免审批**（与 `TodoWrite` 同级），但实现里要调 `write_blocked`：只读档下删定义也算写，直接拒。
- **`CronList`**
  - 入参：`{ includeDisabled?: boolean = true }`；出参：`{ schedulerAlive: boolean, jobs: [...] }`，每个 job 带定义 + 运行态合并视图。
  - **免审批**；`prompt` 只回前 200 字符 + `promptChars`（避免一次 `CronList` 把上下文撑爆；遵守工具结果预算，`docs/code-standards.md:81`、`docs/forbidden.md:95`）。
  - 若 `LUNAC_CRON_DIR` 未注入（例如 standalone 跑 `agent.exe`），返回明确错误「cron store 未配置」，**不猜路径**。
- **注册与裁剪**：
  - 三件都进 `tools::defs()`（`core-agent/src/tools.rs:344` 起），**无条件注册**。当前工具总数 **25**（守门单测 `agent_tool_is_registered_and_filterable`，`core-agent/src/tools.rs:4390-4393`）⇒ 阶段 0 加 `CronList` 为 **26**，阶段 1 加 `CronCreate` / `CronDelete` 为 **28**。必须同步改这个数字与断言，不是删断言。
  - **是否进固定前缀：是。** 工具表是请求体固定前缀的一部分（`docs/code-standards.md:80`；宿主注释也明确「工具表是每次请求都要发的固定前缀」`app/src-tauri/src/commands.rs:522-525`）⇒ 加工具会让前缀变一次，**必须重启 agent 才生效**；升级后第一次对话 / 手动重启即可。
  - **`--disallowedTools` 必须能裁**：过滤在 `tools::defs(&cli.disallowed)`（`core-agent/src/tools.rs:342-344`），守门测试已钉住 `Agent` / `MemorySearch` 的裁法（`:4395-4407`）；三件都加同样的断言。前端候选名单加进 `app/src/main.ts:3471-3514` 的 `TOOL_BLACKLIST_CANDIDATES`。
  - **子代理不接**：三件都加进 `subagent_tool_defs` 的排除列表（`core-agent/src/main.rs:7560-7585`），并同步测试（`:10187-10267`）。理由：子代理是「同批并发、跑完即散」，让它建 / 删定时任务会绕过用户对「持续成本」的确认。
  - **cron 子进程自己再裁一次**：调度器拉起的单发进程应带 `--disallowedTools CronCreate CronDelete CronList Agent`，防止「定时任务自己再建定时任务」或派子代理扩大成本。
- **落盘与安全**：
  - `CronCreate` / `CronDelete` 写 `cron.json` 前调用 `write_blocked`；`CronCreate` 还要对 `prompt` 跑 `content_safety::analyze`（`core-agent/src/content_safety.rs:167`；与写类工具同一套凭据扫描，`core-agent/src/main.rs:3575-3598`），命中即拒。
  - 原子写：同目录 `cron.json.tmp` → rename；宿主读取失败时保留上一份有效定义并 `warn`，**不删文件、不静默清空**。
  - `LUNAC_CRON_DIR` 由宿主注入（`app/src-tauri/src/commands.rs:429-529` 的 envs 段），agent 侧加入 `Ctx.add_dirs`（照 `LUNAC_MODULES_DIR` 的写法，`core-agent/src/main.rs:4599-4608`）；只放行 `ModuleData\tasks` 这一个目录，不放宽整个 `ModuleData` / `config`。
- **协议**：**不新增 stream-json 字段**。`CronCreate/Delete/List` 只是工具；job 元数据不注入 system、不进 history 前缀。将来做 `scope=pet` 时复用已有的 user 消息 `source:"pet"` 字段（`app/src/main.ts:11582-11585`、`core-agent/src/main.rs:2754-2759`），仍不需要新字段。

##### 六、E. 分阶段建议（不排期；每阶段都给改动点 / 验证 / 风险）

**阶段 0：只做 `CronList` + 手动触发（零自动执行）** —— **已实现（2026-10-17）**

> **实现备注（2026-10-17，阶段 0）**：
> - **落点**：`<exe 根>\ModuleData\tasks\cron.json`（job 定义；阶段 0 **只读、无写者**，手工可写）+ `cron-state.json`（运行态，**只由宿主写**）。路径唯一真相源 = `app/src-tauri/src/storage.rs` 的 `cron_dir()` / `cron_defs_path()` / `cron_state_path()`；启动时 `ensure_cron_dir()` 建目录，`LUNAC_CRON_DIR` 由宿主注入 agent，agent 侧并进 `Ctx.add_dirs`（`core-agent/src/main.rs`）。
> - **数据契约**：`cron.json` = `{ "version": 1, "jobs": [ { id, schedule:{kind:"every",everyMinutes} | {kind:"daily",at:"HH:MM"}, prompt, enabled, createdAt, scope, workspace, notify, maxRunsPerDay } ] }`（也接受裸数组）；`cron-state.json` = `{ "version": 1, "jobs": { "<id>": { lastRunAt, lastStatus, lastSummary, lastError, runCount, failureCount } } }`。
> - **`CronList`**（`core-agent/src/tools.rs`）：**只读、免审批、只读档放行、可被 `--disallowedTools` 裁、子代理排除**；输出恒为 JSON 文本；`prompt` 只回前 200 字符 + `promptChars`；文件缺失 / 损坏返回 `reason:"missing"` / `"invalid"`（未注入 `LUNAC_CRON_DIR` 返回 `reason:"not_configured"`），不 panic。工具数 **25 → 26**（守门单测已同步）。
> - **宿主命令**（`app/src-tauri/src/commands.rs`）：`cron_list`（定义 + 运行态合并给 UI，`schedulerAlive` 恒 false）与 `cron_run_now(id)`（手动单发）。单发参数逐字为：`agent.exe --print --output-format stream-json --permission-mode plan --add-dir <workdir> --disallowedTools Write Edit NotebookEdit Cmd PowerShell ImageGen EnterWorktree ExitWorktree Remember PetGrowth PetPersonaSuggest WebSearch WebFetch AskUserQuestion Agent CronCreate CronDelete CronList`（**variadic 放最后**），query 走 **stdin**；env 与常驻 agent **共用 `agent_envs()`**（单一拼装点，含 `LUNAC_CRON_DIR`）；**10 分钟墙钟 deadline**，超时用 `cli_bridge::kill_tree()` 杀整棵进程树；输出写 `temp\logs\cron-<id>-<YYYYMMDD-HHMMSS>.log`（落盘前 `mask_secrets`），并更新 `cron-state.json` 的 `lastRunAt` / `lastStatus` / `runCount` / `failureCount` / `lastSummary` / `lastError`；`result.usage` 以 **`source:"cron"`** 追加 `ModuleData\usage\usage-YYYY-MM-DD.jsonl`。
> - **前端**：设置面板「通用」页「定时任务（实验）」区块 —— 只读列表（id / schedule / enabled / lastRunAt / lastStatus / runCount / failureCount）、逐条「立即运行」（运行中禁用并显示进行中）、「刷新」、空态与缺失 / 损坏提示；**不提供**新建 / 编辑 / 删除。5 语言文案已补齐。
> - **成本面板**：`app/src/usage-cost.ts` 的来源档新增 `cron`（`parseUsageSourceValue` / `sourcePartition` / 下拉选项 + i18n），cron 用量不再混进「普通聊天」。
> - **零自动执行**：阶段 0 **没有**调度线程 / 定时器 / 每日上限 / 失败自动停用 / 通知 / `CronCreate` / `CronDelete` / `schtasks`；全仓唯一运行入口是前端按钮触发的 `cron_run_now`。
> - **验证方式**：`cargo test`（core-agent，基线 281）、`cargo test --bins`（app/src-tauri，基线 255）、两侧 `cargo build --release`、`npx tsc --noEmit`、`npm run build`、`npm run verify`；手工夹具：在 `<exe 根>\ModuleData\tasks\cron.json` 写入一条 job，`CronList` / 设置面板可列出，点「立即运行」后检查 `temp\logs\cron-*.log`、`cron-state.json`、`ModuleData\usage\usage-*.jsonl` 的 `"source":"cron"`。

- 改动点：
  - `core-agent/src/tools.rs`：`defs()` 加 `CronList`，实现读 `LUNAC_CRON_DIR/cron.json` + `cron-state.json` 并合并；工具数 25 → 26（`:4390-4393`）；子代理排除（`core-agent/src/main.rs:7560-7585`）。
  - `core-agent/src/main.rs`：把 `LUNAC_CRON_DIR` 放进 `add_dirs`（`:4593-4608` 旁）。
  - `app/src-tauri/src/storage.rs`：新增 `cron_dir()` / `cron_defs_path()` / `cron_state_path()`（建立在 `module_data_dir()` 上，`storage.rs:40-43`）。
  - `app/src-tauri/src/commands.rs`：在 env 拼装段注入 `LUNAC_CRON_DIR`（`:429-529`）；新增 `cron_list` / `cron_run_now` 两个命令；`cron_run_now` 用 async 薄壳 + `run_blocking`（§规范 5，`commands.rs:22`）。
  - `app/src-tauri/src/main.rs`：注册命令（`:548` 起）；不需要启动调度线程。
  - 前端：设置面板新增「定时任务」分区（`app/src/plugins/builtin/settings.ts` 现有分区形态），列表 + 「立即运行」；i18n 进 `app/src/i18n.ts` 的 DICT。阶段 0 的 job 来源可以是手写 `cron.json` 夹具 / UI 表单，不引入 `CronCreate`。
- 验证：
  - 单测：`CronList` 能读定义 + state、state 缺失时给 `null`、非法 JSON 报错不崩；`--disallowedTools CronList` 能裁掉它；工具总数 26。
  - 端到端：在临时数据根手写一条 job，`cron_run_now` 走一次 `agent.exe --print --permission-mode plan`；断言产生 `temp\logs\cron-*.log`、`ModuleData\usage\*.jsonl` 一条 `source:"cron"`、state 更新；断言不产生任何自动后续运行。
  - 用 `deepseek-flash` 做真实端点验证（`docs/testing.md` §测试 6）。
- 风险：新增工具让固定前缀变一次（必须重启 agent）；`LUNAC_CRON_DIR` 若忘记加进 `add_dirs`，写 / 读会被工作区锁拒；手写夹具的 job 字段与正式 schema 漂移（阶段 0 就要把 schema 定死）。

**阶段 1：宿主调度 + `--print` + 只读白名单 + 每日上限 + 通知 —— 已实现（2026-10-17）**

> **实现备注（2026-10-17，阶段 1；用户已裁定「按推荐默认值继续」）**：
> - **采纳的默认值（即 §七 G 的推荐项）**：只读任务（写类 cron 不做）；`CronCreate` **所有运行模式都强制弹卡**；强制要求已配置工作区；最小间隔 **15 分钟**；每 job 每日默认 **12** / 硬上限 **48**；全局每日 **48**；连续失败 **3** 次自动停用；job 上限 **20**；`prompt` ≤ **8000** 字符；`scope` 只支持 **global**（`pet` 明确拒绝并给原因）；`notify` 默认 **toast**（应用内浮层事件）；错过运行**只补最近一次且延迟 ≤10 分钟**，否则 `skipped_missed`；**忙则跳过不排队**（`skipped_busy`）；单次 **10 分钟**墙钟 deadline，超时杀整棵进程树；usage 记 `source="cron"`。
> - **工具（总数 26 → 28）**：`CronCreate` / `CronDelete` 进 `tools::defs()`；`CronList` 补 `enabled` 过滤与 `nextRunAt` / `nextRunAtLocal` / `consecutiveFailures` / `autoDisabled` / `todayRuns`。`CronCreate` 在 `needs_approval`（要问）、`CronDelete` 免审批（幂等）；两者只读档 / 计划相位由 `write_blocked` 直接拒；**子代理排除**、**可被 `--disallowedTools` 裁**。守门单测同步 **28**。
> - **`CronCreate` 校验与写盘**：`everyMinutes` 15..1440；`at` 严格 `HH:MM`；`prompt` 1..8000 且过 `content_safety::analyze`（命中凭据即拒）；`scope` 只 `global`（`pet` 给原因拒绝）；`workspace` 只接受空串或当前 cwd，且要求 `LUNAC_WORKSPACE_LOCKED=1`（已配置工作区）；`maxRunsPerDay` 1..48（默认 12）；job 数 ≤20；**拒绝覆盖损坏的 `cron.json`**。写盘 = `cron.lock` 内读改写 + `cron.json.tmp` → rename（Windows rename 不覆盖时先删旧再改名，仍在锁内）。
> - **审批硬闸（即使 skip-permissions 也不放行）**：除 `needs_approval` 外，`main.rs::dispatch_tool` 在工具执行前显式检查 `approval_policy()`：`CronCreate` 在 `UnattendedDeny` / `UnattendedAllow`（= `--dangerously-skip-permissions`）下**直接拒绝**并说明「必须走交互审批卡」；`hook_allow_needs_card` 对 `CronCreate` 恒为 true（hook allow 也翻不过去）。
> - **调度器（新文件 `app/src-tauri/src/cron.rs`）**：`thread::spawn` 30s tick（`start()` 在 `app.manage(AppState)` 后启动；`stop()` 由窗口 `Destroyed` 退出路径调用，tick 以 500ms 粒度检查停止标志）。纯函数 `is_due` / `missed_policy` / `caps_allow` / `latest_due_at` / `next_run` / `initial_next_run_at` 全部注入 `LocalNow`，可单测；`LocalNow` 用既有 Win32 `GetLocalTime`（零新增依赖）。
> - **调度语义（与 §四 C 对齐）**：`every N` 按 epoch 真实间隔，醒来只取最近周期点；`daily` 按本地墙钟 + `state.nextRunAt` 去重（同一天只跑第一次，秋季 DST 重复小时天然去重）；春季 DST 不存在的时刻因延迟 >10 分钟被 `skipped_missed`（不顺延）；每次运行 / 跳过都把 `nextRunAt` 推到下一个未来时刻，**不补一串**。
> - **单飞 / 忙态**：`cron.rs` 全局 `RUNNING` CAS 保证同时只有 1 个 cron；新增命令 **`cron_set_busy(bool)`**（前端在流式开始 / 结束时调用，本任务只加命令，前端由另一任务接）；`BUSY || RUNNING` ⇒ 到期任务记 `skipped_busy`，不排队。
> - **执行（唯一 runner）**：手动 `cron_run_now` 与调度器共用 `cron.rs::execute_job`；参数固定 `--print --output-format stream-json --permission-mode plan --add-dir <workspace> --disallowedTools <写类+外部+子代理+Cron*>`（variadic 放最后），query 走 stdin；env 复用 `commands::agent_envs()`；超时用 `cli_bridge::kill_tree()` 杀整棵进程树；日志写 `temp\logs\cron-<id>-<ts>.log`（落盘前 `mask_secrets`）；usage `source="cron"`。
> - **通知**：运行完成 / 失败时 `emit("cron-finished", {id, status, summary})`（`notify="none"` 不发）；**不注入主对话**、不写 `chat.db`。
> - **锁协议（跨 crate 约定）**：`cron.json` 由 agent 的 Create/Delete 写，另有**一个**用户动作例外（`cron_set_write_access` 只改 `writeAccess` 一个键，2026-10-27 阶段 2B）；`cron-state.json` 仅宿主写；两者共用 `cron.lock`（`create_new` 获取 + 5s 超时重试 + 释放即删；崩溃遗留 >30s 的陈旧锁可接管）。单写者与锁不冲突：锁保护「读 → 改 → 原子替换」临界区。路径唯一真相源 = `storage.rs`（`cron_dir()` / `cron_defs_path()` / `cron_state_path()` / `cron_lock_path_in()`）。
> - **stateless 重启安全**：`cron-state.json` 原子写（tmp+rename）并含 `heartbeatAt`；调度器每 tick 更新心跳（agent 的 `CronList` 以 90s 窗口报 `schedulerAlive`）。
> - **验证（2026-10-17）**：`cargo test`（core-agent）exit 0 / **287 passed**（基线 282，净 +5）；`cargo test --bins`（app/src-tauri）exit 0 / **270 passed**（基线 259，净 +11，含 4 条从 `commands.rs` 迁入 `cron.rs` 的既有用例）；两侧 `cargo build --release` exit 0；`npx tsc --noEmit` / `npm run build` / `npm run verify` exit 0（前端未改）。
> - **写类任务（2026-10-27 更新）**：默认仍是「单发固定 plan 档 + 裁掉写类 / 外部 / 子代理 / Cron* 工具」；唯一例外 = 用户在任务列表里逐 job 打开的 `writeAccess`（阶段 2B：换 `acceptEdits` + `--unattended-allow Write,Edit,NotebookEdit`，命令类与外部工具照旧被裁）。`--dangerously-skip-permissions` **仍然**不作为 cron 的运行方式（会关工作区锁并绕过危险命令人工确认，§禁用 1.2）。「无人值守安全」仍不成立 —— 它只是策略级的「默认拒绝 + 白名单」。
- 改动点：
  - 新模块 `app/src-tauri/src/cron.rs`：调度线程（建议 30s tick）、定义 / 状态读写、到期判断（纯函数，注入 `now` 便于单测）、单发 runner、超时杀进程树、日志 / usage / 通知。
  - `app/src-tauri/src/main.rs`：`mod cron;`；`app.manage(cron::CronState::default())` 或 AppState 增字段（`:126-137`、`:445-455`）；启动调度线程；注册 `cron_list` / `cron_run_now` / `cron_set_enabled` / `cron_delete`。
  - `app/src-tauri/src/commands.rs`：把 `start_cli_process` 的 env 拼装抽成共享函数（`:429-529`），resident 与 cron 两条 spawn 都调用它；cron runner 复用 `spawn_child`（`:250-288`，已带 `child_job::assign`，`:269-272`）；把 `kill_process_tree` 提为 `pub(crate)`（`cli_bridge.rs:93-117`）供超时使用。
  - `core-agent/src/tools.rs`：加 `CronCreate` / `CronDelete`，`needs_approval` 加 `CronCreate`（`:1081-1130`），工具数 28；`write_blocked` 与 `content_safety` 落地。
  - `app/src-tauri/src/storage.rs`：`append_usage_log` 的调用方（`:1221`）；`source:"cron"`。
  - `app/src/usage-cost.ts`：来源分区加 `cron`（`:150-157`、`:593-602`）+ i18n；否则 cron 成本会算进普通聊天。
  - 前端：`classifyRequest` 对 `CronCreate` 硬挡（`app/src/main.ts:8241-8256`）；忙标记 `set_agent_busy`（在 `startAgentChat` / 收尾处，`app/src/main.ts:11176`、`:11241`、`:11433`）；「定时任务」列表 + 结果浮层 + 可选提示音。
  - 单发参数（只读档）：`--print --output-format stream-json --permission-mode plan --mcp-server stdio:<lunac.exe> --add-dir <workspace> --disallowedTools <写类 + 外部工具 + 子代理 + Cron*> --disallowedTools` 放最后（variadic 规则，`core-agent/src/main.rs:4489-4497`）。query 用 stdin 写入（`:4533-4546`），避免命令行长度与引号问题。
- 验证：
  - 单测：到期计算（daily / every / 错过 / DST 去重）、每日上限、连续失败自动停用、忙跳过、state 损坏回退；用注入的 `now` / 本地日期做纯函数测试，不依赖真实时钟。
  - 集成：临时数据根 + 一条 1 分钟后到期的 job，起 release 宿主；断言只剩一次运行、日志 / usage / 通知 / state 正确；第二次触发命中每日上限或正常下一次；用一个必失败的配置连跑 3 次，断言 `autoDisabled`。
  - 审批：写一个会调用 `Write` / `WebSearch` 的 prompt，断言单发拒绝、退出非 0、state `lastStatus=denied_tool`（不是卡死）。
  - 并发：用户对话进行中命中到期，断言 `skipped_busy` 且不排队。
  - 目标必须是 release / 安装版（`docs/testing.md` §测试 11）。
- 风险：共享 env 构造函数改动可能影响常驻 agent（必须回归 `start_cli` / 换档 / 换工作区）；调度线程与 App 退出 / 睡眠的竞态；日志里的输出可能含敏感内容（必须 `mask_secrets`）；单发拒绝外部工具会让任务失败并累计失败（prompt 要明确「只用只读工具」）。

**阶段 2：写类任务（2A 不做；2B 已于 2026-10-27 实现）**
- **2A（不推荐，且与硬禁令冲突）**：`--dangerously-skip-permissions` + 去掉 `--permission-mode plan`。它会让 `Cmd` / `PowerShell` 绕过危险命令的人工确认（§禁用 1.2），并关闭工作区锁（`core-agent/src/main.rs:4613-4614`）。即使再用 `--disallowedTools` 移除 shell，写仍可落到 `config\` / 任意盘符，**不能叫「有边界」**。
- **2B（已实现，2026-10-27）**：在 agent 侧新增「无人值守工具白名单」开关（例如 `--unattended-allow Write,Edit,NotebookEdit`），在保持 `read_only=false` 与工作区锁的前提下，只对白名单工具跳过审批；危险命令 / 凭据扫描仍照旧（复用 `bash_safety` / `content_safety` 的既有硬闸）。这是**新代码**，不是零成本。
  - 改动点：`core-agent/src/main.rs` 的 `ApprovalPolicy` / `needs_approval_with`（`:4551-4569`、`:7588-7608`）；`core-agent/src/tools.rs` 的 `write_blocked` 不拦白名单（但要保留 `read_only` 语义）；`app/src-tauri/src/cron.rs` 的 job 模式字段与红色确认 UI。
  - 验证：临时工作区写一个 canary 文件；断言 `Cmd` 仍被拒、工作区外写入被拒、白名单外写工具被拒；真机确认红色告警与二次确认。
- **2B 落地（2026-10-27）**：
  - **agent**：`--unattended-allow <names>`（逗号分隔 / 可重复）⇒ `effective_unattended_allow()` 收成有效白名单，**只认 `UNATTENDED_WRITE_TOOLS = [Write, Edit, NotebookEdit]`**，其余名字丢弃并 `warn`（命令类 / 外部出口 / `Agent` / `Cron*` / `Task*` / `mcp__*` 永不进）。两处决策点（主循环 + 子代理循环）在 `ApprovalPolicy::UnattendedDeny` 下多一条带守卫的分支：`unattended_allows(name) && !hook_allow_needs_card(name, input)` 才放行 —— **凭据扫描命中照样拒**。常驻模式（`Stdio`）带了参数也只 `warn`、不放宽任何东西。
  - **宿主**：`cron.json` 的 job 新增 `writeAccess`（缺省 false）。`run_args(.., write_access)` 开时把 `--permission-mode` 从 `plan` 换 `acceptEdits`、追加 `--unattended-allow Write,Edit,NotebookEdit`、并从 `READ_ONLY_DENY` 里**只**移出这三件（`Cmd` / `PowerShell` / `WebFetch` / `Agent` / `Cron*` / `Task*` 仍在）。调度与「立即运行」都读 job 的字段（`job_write_access`，只认 `true`）。
  - **授权只能用户给**：新命令 `cron_set_write_access(id, allow)` + 任务列表里每行的红字二次确认开关；`CronCreate` **恒写 `writeAccess: false`**，模型没有任何通道自己打开。宿主这条路径是**唯一**写 `cron.json` 的地方（与 agent 共用 `cron.lock`，只改这一个键）。
  - **单测**：agent `unattended_allow_parses_and_only_covers_write_tools`（解析 / 去重 / 非法名丢弃 / 大小写不放过）；宿主 `write_access_args_only_swap_the_mode_and_the_three_write_tools`（换档 + 白名单位置在 variadic 之前 + 其余裁剪不变 + 关掉逐字节回到只读）、`job_write_access_defaults_to_off`。
  - **验证（2026-10-27）**：`core-agent` `cargo test --bins` **422 passed / 0 failed / 15 ignored**；宿主 `cargo test --bins` **336 passed / 0 failed / 2 ignored**；`npx tsc --noEmit` exit 0；`npx vitest run` **17 passed**；仓库根 `npm run verify` **8/8（0 提醒）**；`core-agent` 的 `cargo build --release` exit 0（**agent.exe 已重编** —— dev 布局优先取 `core-agent\target\release\agent.exe`，不重编会静默跑旧解析）。
  - **仍未做**：真机跑一条写类 job（canary 文件 + 红色告警点击）与「工作区外写入被拒」的实测，属验收类（用户已裁定验收暂缓）。宿主 `set_write_access` 本身**没有单测**（它直接读写数据根 `ModuleData\tasks\cron.json`，与 `cron.rs` 其余 I/O 函数同一条口径：只测纯函数 + 参数拼装），验证靠上面的真机点击。
- 风险：无人值守的写类任务会把「模型判断」直接变成磁盘副作用；prompt 注入（若允许 `WebFetch`）风险更高；必须默认关闭、逐 job 显式开启、并在 UI 上用语义红写明「无人值守 + 可写」。

##### 七、F. 明确「不做 / 不能做」

- **不做**：
  - 不引入任何调度库 / cron 解析库 / `chrono`（零新增依赖；时间用既有 `GetLocalTime`）。
  - 不写 `schtasks`（理由见 A3；包括不建开机任务之外的任何计划任务）。
  - 不复用 `chat.db` 存 job / 结果（§禁用 61）。
  - 不把 cron job 列表 / 结果自动注入 system 固定前缀或主对话 history（§禁用 45；结果只进宿主列表 / `temp\logs`）。
  - 不把 `CronCreate` 做成「自动档静默放行」；不默认开 `--dangerously-skip-permissions`。
  - 不做「醒后补跑所有错过任务」的突发行为；不做无上限的每 job 历史文件。
  - 不把进程内定时器当作真正的调度实现（只可能是会话内演示）。
  - 不让 cron / 子代理再创建 cron（单发进程裁掉三个工具 + `Agent`）。
  - 不新增 stream-json 字段、不改 `tool_result.content` 语义。
- **不能做（如实写）**：
  - **宿主不启动就无法运行**（除非走 A3 的 schtasks，本方案明确不做）。
  - **每次 cron 都是新会话**：`--resume` 已实现（2026-10-22），但 cron runner 未接（无 job 级会话 id / 未传 `--checkpoint` / `--resume`）⇒ 当前仍没有对话上下文；job 的 `prompt` 必须自带全部前提。
  - **单发没有审批回包通道**：需审批的工具只能被拒；不能「先跑，遇到审批再问用户」。
  - **没有硬性单次费用上限**：`LUNAC_TURN_BUDGET_TOKENS` 是软闸且可续命（`core-agent/src/main.rs:196-206`）；只能靠每日次数上限与超时兜底。
  - **写类 cron 的边界（2026-10-27 更新）**：2B 已落地 ⇒ 用户逐 job 打开后，模型可以在**工作区内**用 `Write` / `Edit` / `NotebookEdit` 直接改文件；**命令类仍然被拒**（危险命令的人工确认没有回包通道，`Cmd` / `PowerShell` 不在白名单）。**仍不能承诺「无人值守安全」**：没有审批卡 = 没有「写到一半被拦」，回滚只能靠用户自己的版本控制；它不是沙箱（§禁用 1.1）。
  - **没有 OS 级通知**：只有应用内浮层（`app/src/main.ts:9251-9256`）与提示音；托盘 / 无窗口时不能保证用户当场看见。
  - **不能保证精确到秒**：调度 tick + 睡眠 / 唤醒只保证「到期后一个 tick 内尝试」，错过的按 C2 跳过或宽限补一次。
  - **不能承诺「无人值守安全」**：它是策略级（默认拒绝 + 白名单），不是沙箱（§禁用 1.1）。

##### 八、G. 未查到 / 需拍板

- **未查到**：
  - 宿主侧通用调度器 / 定时线程（`Cargo.toml:10-55` 无相关依赖；现有 `thread::spawn` 均非 cron）。
  - `tauri-plugin-autostart`（不存在；开机自启是自研 `auto_start.rs`）。
  - `ModuleData\tasks\`（目录不存在，无代码路径；S2 只是计划）。
  - 「换模型会自动 `stop_cli`」（不存在；`set_ai_config` 只落盘 + 注入 env）。
  - 卸载器对计划任务的清理（`scripts\lunac-installer.nsi` 搜索 `schtasks` 零命中）。
  - `PushNotification` 在「无可见窗口」时的实际可见性（未实测；代码是 DOM 浮层）。
- **需拍板**：
  1. 阶段 1 是否要求「必须先配置工作区」才允许创建 cron（建议：是；否则无人值守读可越出默认 `transStorage`）。
  2. 最小间隔 / 每 job 每日上限 / 全局每日上限 / 连续失败阈值（本节建议 15 分钟 / 12 / 48 / 3）。
  3. `CronCreate` 是否在所有运行模式下都强制弹卡（建议：是，即使自动档）。
  4. 写类 cron 是否永远不做，或只接受 2B 的新 agent 开关 —— **已裁定（2026-10-27）：做 2B**；默认关，逐 job 由用户在任务列表里显式打开（红字二次确认）。
  5. `scope:"pet"` 何时支持（阶段 1 只 `global`；阶段 2 复用 `source:"pet"`）。
  6. 手动触发入口放设置面板还是 AI 对话的「更多设置」（建议设置面板，避免与聊天流混）。
  7. `notify` 默认值（建议 `toast`；托盘场景是否默认加 `sound`）。
  8. 成本面板是否加 `cron` 来源档（建议加，否则 cron 用量混入普通聊天）。
- **实现备注（不再改本节以外文档的前提下）**：若日后落地，需同步更新 `docs/todo.md` §待办 5 的 `LUNAC_*` 清单（新增 `LUNAC_CRON_DIR`）与 `docs/testing.md` §测试 10（若给 `agent-e2e.ps1` 加 `-CronDir` 夹具开关）。

#### §待办 3-1e `EnterWorktree` / `ExitWorktree`（git worktree，**已实现**，2026-10-11）

- **目标**：`EnterWorktree` 为当前仓库建/进入一个 git worktree（新分支），把 agent 的**有效工作目录**切过去；`ExitWorktree` 离开、回到原工作目录。两者幂等 / 可重入、绝不破坏用户工作区、可观测。
- **可行性判定（两条，含证据）**：
  - **① 有效工作目录能否中途改变 —— 能，且用「独立覆盖层」避免改动面过大**：现状 `cwd` 是**不可变单值** —— `tools.rs:120` `Ctx { cwd: PathBuf, .. }`；`main.rs:3425-3426` 从 `std::env::current_dir()` 取一次；`resolve`（约 `tools.rs:1009`）拼它、`guard`（`tools.rs:1018`）用它当边界、`run_shell`（`tools.rs:1535`）设 `current_dir`、`glob`·`grep` 默认根也用它；`env_block`（`main.rs:1820` / `1874-1891`）把 cwd 烘进系统提示词**固定前缀**（规则 18 前缀不变量）。⇒ **不把 `cwd` 变可变**，而是新增**独立的一层覆盖** `Ctx.worktree: Arc<Mutex<Option<PathBuf>>>` + `effective_cwd()`：**只让三类寻址用途读覆盖**（`resolve` / `run_shell` / `glob`·`grep` 默认根）；**`guard` 边界恒为 `cwd`+`add_dirs`**（落点 2026-10-11 改到**数据根**、不再位于 `cwd` 之内 ⇒ 把该根目录并入 `add_dirs` 放行，见下；原工作区照旧可达）；**系统提示词不重建**（规则 18；新目录由工具返回文本告知模型）。改动面 = tools.rs 约 5 处读取点 + 1 字段 + 2 工具实现 + 三张表 + 分派；main.rs 2 处 `Ctx{}` + `subagent_tool_defs` 排除两件。
  - **② `git worktree` 既有用法 / 工作目录是不是 git 仓库**：全仓 `core-agent/src` grep `worktree` **零命中**（无既有实现 / 约定）；工作目录默认 = `<exe 根>\temp\transStorage`（**通常不是** git 仓库），只有用户把工作区设成某个仓库时才是 ⇒ **非 git 仓库是常态**，工具必须清晰报错、状态不变。
- **已实现**：
  - `core-agent/src/tools.rs`：`Ctx.worktree` 字段 + `effective_cwd()`；`resolve` / `run_shell` / `glob`·`grep` 默认根改用它；`defs()` 加两件 schema（**无条件一等工具**）；`needs_approval` 两件**都真**（按写类：改仓库状态）、`parallel_safe` 都不进（改共享有效 cwd，必须串行）、`gated_in_read_only` 都不进（只读档该**直接拒**，不是「问了再做」）；`run()` 分派用 `write_blocked` 闸；`enter_worktree` / `exit_worktree` / `run_git` / `valid_worktree_name`；落点根 `worktree_root()` / `prepare_worktree_dir()` 与目录名 `worktree_dir_name()`（见下）。
  - **语义（保守口径）**：落点 = `<exe 根>\temp\worktrees\<仓库目录名>-<name>`（**数据根、在用户仓库之外**，见下「落点与推导」）；分支 = `git worktree add -b <name> <dir>` 新建；**已在 worktree 里再进 ⇒ 报错**、**未进就退 ⇒ 报错**（幂等 / 可重入）；目标目录 / 分支已存在 ⇒ 转达 git 原文、**不 `--force`**；`ExitWorktree` **只切回、不删** worktree 与分支（留给用户 `git worktree remove`）。
  - **落点与推导（2026-10-11 改到数据根）**：`tools::worktree_root()` = `crate::log::log_dir().parent().join("worktrees")` —— **复用既有「从 exe 根推导 temp」的那一套**，与 `tools::output_dir()`（`tool-outputs`）、`image::image_dir()`（`images`）、`main.rs::review_state_path()`（`review-last`）同源：`LUNAC_LOG_DIR` 优先（宿主注入），否则回退 `<agent.exe 目录>\temp\logs`，取父目录即 `<exe 根>\temp`。**为什么不建在 `<cwd>\.lunac-worktrees`**：那会在用户仓库里显出一个未跟踪目录；改到应用自己的数据根后**用户仓库保持干净**（本次核心诉求）。`prepare_worktree_dir()` 在启动时建目录。
  - **命名防撞**：worktree 目录名 = **`<仓库目录名>-<name>`**（`worktree_dir_name(&toplevel, name)`；`toplevel` = `git rev-parse --show-toplevel` 取末段，取不到回退 `repo`）。理由：现在所有仓库共用一个 `<temp>\worktrees\`，若目录名只用 `<name>`，**两个不同仓库同名就会撞**（第二次 `git worktree add` 因目标目录已存在被拒）。**分支名仍是不带前缀的 `<name>`**（分支是每仓库独立的 ref，不会跨仓库撞名）。**同名再进入**：同仓库同名 ⇒ 目录已存在 ⇒ 报错（转达 git 原文 / 自定义「已存在」），**不 `--force`**（延续既有口径）。
  - **`guard` 白名单与 workspace-locked 判断**：新落点在 `cwd` / `--add-dir` 之外 ⇒ main.rs 把 `tools::prepare_worktree_dir()`（=`<exe 根>\temp\worktrees`）推进 `Ctx.add_dirs`（与 `output_dir` / `image_dir` **同一种机制**），**只加这一个目录**，不放宽 `<exe 根>` 整体、不碰 `config\` / `ModuleData\`。**`LUNAC_WORKSPACE_LOCKED=1` 下也放行该目录（判断）**：`add_dirs` 本就是工作区锁边界定义的一部分（`guard` 判据 = `cwd` + `add_dirs`），该目录属**应用自有**（与 `tool-outputs` / `images` / `skills` / `Modules` 同类），不是用户工作区；且**只放行、不代替审批**（`Write` / `Edit` 仍走审批，`EnterWorktree` 本身按写类要审批）。若不放行，则「配置了工作区」（locked=1，常态）时 worktree 功能整体不可用 —— 故按**一致性与可用性**选「放行」。
  - **可观测**：两件各写一条 INFO 日志（当前 worktree / 分支 / 仓库根）；并把新 cwd 放进**工具返回文本**（系统提示词不重建），返回文本里点明 worktree 的**实际落点（数据根）**与「自行 `git worktree remove <path>`」。
  - `core-agent/src/main.rs`：`tools_ctx` / `test_ctx` 补 `worktree`；启动时把 `tools::prepare_worktree_dir()` 推进 `Ctx.add_dirs`（放行数据根里的 worktree，见上「guard 白名单」）；`subagent_tool_defs` **排除**两件（有效工作目录是主循环状态，同 `EnterPlanMode` 判据）+ 单测断言。
  - `app/src/main.ts`：`TOOL_BLACKLIST_CANDIDATES` 加 `EnterWorktree` / `ExitWorktree`（用户可关）。**未改 `WRITE_TOOLS`** —— 这两件没有「文件路径」入参，不是「改动过的文件」。
- **工具数 19 → 21**：守门单测 `agent_tool_is_registered_and_filterable` 由 `19` **有意改成 `21`**（并加两件在表内的断言）；`defs()` 文档注释「十九个」→「二十一个」。全仓非 archive grep 无别处硬编码 `19/21`（唯一命中 `docs/todo.md:263` 的 Cron 前瞻「19 → 22」已同步改成「21 → 24」；archive 为历史、按纪律**不回写**，其 `19` 属存量）。
- **验证（以 release 为目标，2026-10-11）**：
  - `cargo build --release`（cwd=`core-agent`）**exit 0**（Finished release profile；2 条既有 `unused_mut` / `dead_code` 告警**非本轮引入**）；`cargo test`（cwd=`core-agent`）**exit 0 / 176 passed / 0 failed / 3 ignored**（含新增 `worktree_tools_are_registered_and_gated` / `worktree_dir_name_prefixes_the_repo`，以及改判后的 `agent_tool_is_registered_and_filterable` / `subagent_tool_defs_drops_agent_and_bridge_only_tools`）。
  - `npx tsc --noEmit`（cwd=`app`）**exit 0**。
  - **端到端实测（`scripts\agent-e2e.ps1`，release，均 exit 0 / PASS）**：
    - **git 仓库场景**（夹具 = **系统 temp 的独立 git 仓库、在仓库之外**；用后已删）：`-WorkspaceDir <夹具> -OutDir <系统 temp 的 run 根> -PermissionMode acceptEdits -Model deepseek-flash -Query "…EnterWorktree wt2 → Write probe.txt → ExitWorktree → Write after.txt → DONE" -ExpectTool EnterWorktree,Write,ExitWorktree -ExpectContains DONE -ExpectNoError`（**以 `LUNAC_WORKSPACE_LOCKED=1` 运行**，见下）⇒ exit 0 / PASS、工具池 **21**、工具序列 `[EnterWorktree, Write, ExitWorktree, Write]`、最终文本 `DONE`。**脱敏落盘证据**：`git -C <夹具> worktree list` 第一列 = `…\lunac-wt-run-<id>\<stamp>\worktrees\lunac-wt-fix-<id>-wt2`（**= 数据根 `<temp>\worktrees\…`，非用户仓库**）、分支 `[wt2]`；`<数据根>\worktrees\<夹具目录名>-wt2\probe.txt` = `hello-in-worktree`；退出后 `<夹具>\after.txt` = `back-in-main`（**证明有效 cwd 已切回主检出**）；`<夹具>\probe.txt` **不存在**、`<夹具>\.lunac-worktrees` **不存在**（用户仓库干净）。日志实证推导：`worktree 落盘根: <runDir>\worktrees` 与 `EnterWorktree：有效工作目录切到 <runDir>\worktrees\<夹具>-wt2（新分支 wt2，仓库根 <夹具>）`。**该场景设 `LUNAC_WORKSPACE_LOCKED=1`**（模拟「宿主配了工作区」）= 直接验证 guard 白名单 —— worktree 内的 `Write` 通过（若未并入 `add_dirs` 会被工作区锁判 `Access denied`）。worktree 与分支 `wt2` **保留在磁盘**（保守），用完 `git worktree remove --force` + `prune` 清理。
    - **非 git 场景**（夹具 = 系统 temp 的普通目录，**在仓库之外**；用后已清）：`… -ExpectTool EnterWorktree -ExpectContains "Not a git repository"` ⇒ 工具序列 `[EnterWorktree]`，最终文本 = `Error: Not a git repository (or git is unavailable) at <dir>: fatal: not a git repository (or any of the parent directories): .git`（随后 `DONE`）；该目录运行后**内容为空**（未生成 `.lunac-worktrees`，状态不变），数据根 `worktrees\` 也为空（未建任何 worktree）。
    - **权限档与理由**：用 `-PermissionMode acceptEdits`（**非** `plan`）。这两件按写类实现，`plan` 档会被 `write_blocked` 直接拒；而**常驻 stdio 路径不传 `-StdioApproval`** ⇒ 写类工具不弹卡、直接执行（脚本无审批回包器），故无需 `--dangerously-skip-permissions`（该脚本也未暴露此开关）。**另在外层设 `LUNAC_WORKSPACE_LOCKED=1`**（脚本里没有这个开关，故由外层进程环境传入）—— 这样才能真正触发工作区锁、直接验证 A3 的 `guard` 白名单；`-OutDir` 指到**仓库之外**的 run 根，使派生的数据根 `worktrees\` 与夹具都在仓库外。**如实记**：`--print` 单发路径下写类工具默认被拒，故本实测走 **stdio 路径**；`-ExpectTool` 为数组参数，经 `powershell -File` 传会退化成单串 ⇒ 用 `-Command "& …"` 传（首次 `-File` 跑的断言因参数拼接失真而误报，功能本身已成功）。
  - **夹具纪律**：**本轮已按此纪律执行（2026-10-11 改落点）**：两个夹具都建在**系统 temp、仓库之外**（git 场景为独立 `git init` 仓库），`-OutDir` 也指到系统 temp ⇒ 操作全程不触及主仓库；跑完 `git worktree remove --force` + `prune` + 删夹具 / run 目录，并**独立核实**：`git -C d:\cc\claude-code-cli-master worktree list` 仅 `main`、`branch --list` 无 `wt*` 残留、主仓无 `.lunac-worktrees`、系统 temp 无 `lunac-wt-*` 残留。**历史教训（上一轮，如实记一次污染）**：最初把「非 git」夹具放进 `temp\agent-e2e\`（**在本仓库内部**）⇒ `git rev-parse` 向上找到本仓库、把它当仓库，在主仓库登记了一个 linked worktree；**已清理**（`git worktree prune` + 删测试分支 `wt1`），主仓库 `git worktree list` 恢复为仅 `main`。**非 git 场景必须用仓库之外的目录**才是有效反例。
  - **用户已裁定（2026-10-11，两条，按现状定稿、无需改代码）**：① **接受「`LUNAC_WORKSPACE_LOCKED=1` 下也放行数据根的 `temp\worktrees\`」** —— `add_dirs` 本就是工作区锁边界定义的一部分，该目录属**应用自有**（同 `tool-outputs` / `images` / `skills` / `Modules` 那几类例外），且**只放行、不代替审批**（`Write`/`Edit` 仍走审批、`EnterWorktree` 本身按写类要审批）；不放行则「配了工作区」（locked=1，常态）时 worktree 功能整体不可用。② **worktree 的 `name` 参数保持现状不收紧**（仍沿用既有 `valid_worktree_name`、允许含 `/`）；带仓前缀后可能多建一层子目录，同仓同名仍靠「目录已存在 ⇒ 报错、不 `--force`」兜住。
- **涉及文件**：`core-agent/src/tools.rs`、`core-agent/src/main.rs`、`app/src/main.ts`。
- **边界（如实记，未做）**：① **不改系统提示词里的「工作目录」行**（规则 18 前缀不变量）—— 新目录由工具返回文本传递；② worktree 落在**数据根** `<exe 根>\temp\worktrees\`（2026-10-11 从「工作区根内 `.lunac-worktrees\`」改过来）—— **用户仓库不再显出未跟踪目录**；代价是该根目录须并入 `add_dirs`（见上「guard 白名单」），且 `LUNAC_WORKSPACE_LOCKED=1` 下也对其放行（判断与理由见上）；③ `ExitWorktree` 不删 worktree / 分支（保守）；④ 只支持新建分支（`-b <name>`），不支持检出「指定已有分支」—— 分支已存在则转达 git 报错。

#### §待办 3-1f MCP `sampling`（服务端反向调用模型，**已实现 · 已扩外部 · 默认开 · 高底线审批卡 · 本会话记住 · HTTP 反向通道（POST 响应流 + GET 独立流）**，2026-10-11 第 4 轮）

- **结论**：已实现 MCP `sampling/createMessage` 的服务端→客户端反向调用，**复用既有模型调用入口**（`Cfg` 的 `detached()` 副本 + 非流式 Messages 请求，与摘要压缩同写法），**未新开 HTTP 通道、未新增内置工具（工具表仍 21，不含条件注册的 MCP/resources 四件）、未动 `tools.rs`**。
- **用户裁决（2026-10-11 第 2 轮，推翻上一轮的「不扩外部」）**：① **允许 `sampling` 扩到外部服务器** —— 但**必须加闸**；② **去掉开关**（`LUNAC_MCP_SAMPLING` 不再存在）：**默认开启**（默认声明该能力）；③ 授权**像审批一样弹卡**，且这条审批属**「高底线审批」—— 与删除文件 / 移动文件同级，不可被自动放行跳过**。
- **能力声明扩到所有 MCP 服务器（本轮改动）**：`handshake` 里 `sampling` **默认声明**，覆盖**每一台**连上的服务器 —— ① 宿主 stdio 桥（`lunac.exe --mcp-server`，原有）；② `.mcp.json` 的外部 stdio 服务器；③ 远端 Streamable HTTP 服务器。钩子由 `sampling_handler_for`（`mcp::SamplingFactoryFn`）**每台现场构造**，闭包捕获服务器名（审批卡据此显示「是谁在借你的模型」；宿主桥固定名 `Lunac`）。
  - **传输层能力边界（2026-10-11 第 4 轮更新：GET 独立流已补全）**：`sampling` 在**两条传输上都可达** —— ① **stdio**（宿主桥 + 外部 stdio）：服务端请求插在 stdout 行流里，`Bridge::request` 的就地应答循环接住；② **远端 Streamable HTTP**：规范的**两条反向通道都已做** —— **(a) POST 响应 SSE 流**（一次 POST 的响应被服务器当成 `text/event-stream` 返回、在流里夹带服务端请求）、**(b) 客户端 GET 独立 SSE 流**（`initialize` 之后另开一条 `Accept: text/event-stream` 的 GET 长连接，服务器在流上主动推送；服务器不支持时按规范回 405/404 ⇒ 只记一行、不重连）。**两条通道收到的服务端请求都只被「转发」给一个独立线程处理**（见下「180s 边界解法」），主线程（请求-响应路径）自身不等用户。
- **去开关（不留死代码）**：`MCP_SAMPLING_ENV` 常量、`mcp_sampling_enabled()`、`env_truthy()` 与「默认关」分支**已全部删除**；默认即「声明 + 可被请求」。
- **高底线审批卡（本任务题眼）**：
  - **首次**收到某服务器的 `sampling/createMessage` ⇒ **弹一张 `control_request`（`subtype = "mcp_sampling"`）**，用户**明确同意**才执行；卡上只给「允许（本会话）/ 拒绝」，**无「始终允许」**，前端**不**把它交给 `classifyRequest()`（那条路只管 `can_use_tool`）。
  - **本会话记住（2026-10-11 第 3 轮）**：用户点过「允许」后，把**该服务器标识**（键 = 传输 + 服务器名，`sampling_approval_key`；**带上传输**，避免同名 stdio 服务器已同意就静默放行同名 HTTP 服务器）记入**会话内**已同意集合 `MCP_SAMPLING_APPROVED`（进程级 `Mutex<Vec>`）。此后该服务器在本会话内再请求 sampling **直接放行、不再弹卡**；**拒绝不写入**（下一次仍弹卡）；**进程重启即清空**（跨进程不记住）。**stdio 与 HTTP 反向两条路都适用**。⚠️ 卡片**必须明说**「点允许后本会话内该服务器不再询问（重启后需重新确认）」——不能悄悄变成长期静默：卡上有 `agent.sampling_session` 这一行提示，按钮文案也是「允许（本会话）」，i18n 五语言齐全。⚠️ **记住只跳过「审批卡」这一道**，①（无审批通道）③（成本护栏）两道照旧。
  - 走的**是**与 `can_use_tool` / `elicitation` / `mcp_trust` **同一条审批通道**（`control_request` / `control_response`；`open_sampling_approval` 与 `open_elicitation` 同型）。
  - **四条自动放行各自为何仍被挡住**：① `--dangerously-skip-permissions`（单发 `UnattendedAllow`）**不能**跳过 —— 钩子体在弹卡前先判 `approval_policy() != Stdio` 即**直接拒**（`UnattendedAllow` 与 `UnattendedDeny` 同等对待）；常驻模式下该 flag 本就只改工作区锁、不碰审批。② hook 的 `allow` **不能**免卡 —— hooks 只挂在「工具调用」路径（`PreToolUse` / `PermissionRequest`），sampling 是**服务端反向请求**、不经过 hooks。③ 白名单 / always-allow **不能**自动放行 —— 本卡无「始终允许」按钮，也不进白名单（「本会话记住」是**用户此前显式点过允许**的结果，不是自动放行）。④ 「自动」运行方式不适用 —— 不经过 `classifyRequest()`（记住集只由用户显式允许写入）。
  - **单发 `--print` / 无审批通道**：**明确拒绝**（回一条带说明的 JSON-RPC error `-32000 "no user-approval channel (single-shot / --print mode)…"`，绝不静默挂起），与既有「无审批通道就拒」一致。**注意**：`--dangerously-skip-permissions` 在单发下会**放行需审批的工具**（让夹具跑起来、才到得了 sampling），但 sampling 本身**仍被拒** —— 这正是「不可被自动放行」的实证。
- **HTTP 反向通道（2026-10-11 第 3 轮，**做了**）**：远端 Streamable HTTP 服务器此前只是「声明了能力、实际不可达」。本轮在 `mcp.rs` 给 HTTP 传输补上规范允许的**「POST 响应 SSE 流」反向通道**：`request_http` 不再「把整段 body 读完再取首条 id 消息」，而是按 content-type 分流 —— `text/event-stream` 走新的 `read_http_sse`（用 `BufReader` 逐行读、空行切事件、`flush_sse` 拼多行 `data:`）**增量**读；途中遇到**带 `method` + `id` 的服务端请求**即由 `dispatch_http_event` 调 `answer_server_request` 就地应答（另发一次 POST 回 JSON-RPC response 到同一端点），继续读直到命中自己的 id。握手期之外的普通 JSON 回包仍走原缓冲路径。配套把带 401 刷新重试的发送逻辑抽成 `http_start`（`request_http` 与 `http_post` 共用）。`SamplingFactoryFn` 加第二个入参 `via_http`（stdio=false / HTTP=true），供审批卡区分常规卡 / 反向警告卡。（**第 4 轮起**：这条路径改为**把服务端请求转发**给反向处理线程，不再就地等待 —— 见下「第 4 轮」。）
  - **边界（第 3 轮如实记，**第 4 轮已消解**）**：当时的「**未做** GET 独立 SSE 流 / 不做断线重连 / 反向请求在卡上等用户的时间计入该次 `tools/call` 180s 预算」三条，**第 4 轮全部处理**（GET 独立流已补全、含心跳 / 退避重连 / 401 重建；反向请求改到独立线程处理，等用户的时间不再计入任何 `tools/call`）。
- **反向请求的警告卡（2026-10-11 第 3 轮）**：经 **HTTP 反向通道**到达的 `sampling` 请求，`open_sampling_approval` 在 `control_request.request` 上带 `viaHttp: true`；前端 `showSamplingCard(..., viaHttp)` 据此渲染**警告样式**：卡片多名一个 class `.mcp-sampling-warn`（左缘 2px `var(--yellow)`，§界面 11 白名单语义色，**不新造颜色**），并在正文顶部加一行 `agent.sampling_http_banner` 文案**明确「这条请求由服务器经 HTTP 主动推来（反向请求），不是你发起的调用」**，再列出服务器名 / 消息数 / 最大 token。与 stdio 路径的常规卡区分开。**同样适用「本会话记住」**（键含传输，见上）。
- **成本护栏（保留为第二层兜底）**：单次 token 预算 `LUNAC_MCP_SAMPLING_MAX_TOKENS`（默认 4096，含输入侧 ≈4 字符/token 粗算）；会话内次数上限 `LUNAC_MCP_SAMPLING_MAX_CALLS`（默认 8）；**超限即拒并说明**。用户本轮**只去开关、未去护栏**。
- **不得递归**：sampling 触发的请求体**不带 `tools`** ⇒ 模型不可能产生 `tool_use`；服务端 `modelPreferences` 一律忽略（模型只由本机配置决定）。
- **用量归因**：走既有 `usage` 通道（`SubagentUsage` ⇒ `usage_delta` + `result.usage.subagent`，并并入 `requests[]`），日志标明「来源=MCP 服务端反向调用」。**如实记**：前端 `usage_delta` 处理器只读 `in/read/create/out`，未加 `source` 字段（加了前端也不读）；若要在成本面板单列「MCP sampling」需前端侧配合。
- **休眠已解除（2026-10-11 第 2 轮）**：上一轮记录的「已实现但未启用（dormant）」**不再成立** —— `sampling` 现在**默认声明**、外部服务器也接、且每次请求都走**高底线审批卡**。逐条对照旧的休眠理由：① 「只给宿主桥、不扩外部」→ **已推翻**（本轮扩到外部，且加了审批卡 + 成本护栏两道闸）；② 「对规范只覆盖子集」→ **两条反向通道都已补全**：stdio 就地应答 + 远端 HTTP 的「POST 响应 SSE 流」反向通道（第 3 轮）与「GET 独立 SSE 流」通道（第 4 轮，含心跳 / 重连 / 401 重建）；③ 「没有真实使用方」→ 仍成立（宿主桥是工具提供方、插件进程内不走 MCP），但用户已明确要求放开机制并加闸，故不再按休眠处理。
- **放宽信任门语义的后果（保留记录、仍适用）**：外部服务器是第三方代码 / 远端设施，允许它们 request sampling = 把「提供工具」的信任门抬高为「可用我的模型凭据、决定什么内容进我的模型通道、拿模型输出」，并有成本外流 + **经模型通道外传数据**（服务端可把它读到的本机内容塞进 `messages`，流量与我们自己打的同源、网络层看不出异常）+ 归因塌陷等风险。本轮**按用户裁决**放开，**对冲即上述三道闸**（**逐请求高底线审批卡** + 不带 `tools` + 成本护栏）；**不夹带**我们的 system 提示词 / 记忆（请求体由服务端 `params` 决定，我们只翻成 Messages 形状，不注入本机上下文）。**如实记（未做）**：本轮**未**加「逐服务器配额」，只有全局单次 token 预算与会话次数上限。
- **错误码取舍**：未装钩子（理论上不发生）＝`-32601`（语义＝不支持）；已识别但被拒（用户拒绝 / 无审批通道 / 超预算 / 超次数 / 调用失败）＝`-32000`（附可读原因，区别于「不支持」）。
- **验收口径（release 目标，2026-10-11 第 2 轮实测）**：`cargo build --release` exit 0、`cargo test` **176 passed / 0 failed / 3 ignored**；`npx tsc --noEmit`（cwd=app）、`npm run build`（cwd=app）、`npm run verify`（仓库根）均 exit 0。`scripts\agent-e2e.ps1`（本轮**新增** `-AutoApprove` 交互式审批回包器 / `-DenySampling` / `-DangerouslySkipPermissions` / `-ExpectStderr`）配 **stdio sampling 夹具**（发两条 `sampling/createMessage` 的外部 stdio MCP 服务器；**建在系统 temp、仓库之外，用后已删**）五例全 exit 0：
    - ① **未同意 ⇒ 拒**：`-McpExe <夹具> -StdioApproval -AutoApprove -DenySampling -PermissionMode full` ⇒ 夹具 stderr 得 `-32000 "sampling refused: the user denied this request"`。
    - ② **同意 ⇒ 成功**：`… -AutoApprove` ⇒ 工具序列含 `mcp__probe`、`-ExpectSubagentUsage` 过（`result.usage.subagent.requests` 非空）、夹具 stderr 得 `sampling#9001 ok=true`。
    - ③ **单发无审批通道 ⇒ 拒** / ④ **`--dangerously-skip-permissions` 仍不自动放行**：`-Print -DangerouslySkipPermissions -McpExe <夹具>` ⇒ 夹具 stderr 得 `-32000 "no user-approval channel (single-shot / --print mode)…"`（工具被 skip-permissions 放行、sampling 仍被拒）。另测 `-Print`（不带 skip）⇒ 工具本身即以「无审批通道」被拒（stderr）。
    - ⑤ **超次数上限 ⇒ 拒**：`LUNAC_MCP_SAMPLING_MAX_CALLS=1` + 同意 ⇒ 第 1 条 `ok=true`、第 2 条 `-32000 "session call limit reached (1, LUNAC_MCP_SAMPLING_MAX_CALLS)"`。
    - `-DumpApi` 可证 sampling 请求体无 `tools`（沿用上一轮口径）。
- **验收口径（release 目标，2026-10-11 第 3 轮实测）**：`cargo build --release` exit 0（仅既有 `BgCmd` dead_code 告警）、`cargo test` **176 passed / 0 failed / 3 ignored**；`npx tsc --noEmit`、`npm run build`（cwd=app）、`npm run verify`（仓库根）均 exit 0。`scripts\agent-e2e.ps1` 本轮**新增** `-McpConfigFile`（注入 `LUNAC_MCP_FILE`；常驻 + `-Print` 两条路都注入）/ `-ExpectSamplingCards <n>`（统计 `mcp_sampling` 授权卡张数）/ 卡片明细打印（`sampling card #n: server=… viaHttp=…`），**并修掉交互式回包器的一个真 bug**（Windows + PS5.1 往 stdin 管道写的**第一行会被吞掉** ⇒ 需先垫一个空行；此前 `-AutoApprove` 路径在本机完全不可用）。夹具（**建在系统 temp、仓库之外，用后已删**）：① **external stdio MCP 服务器**（走工作目录 `.mcp.json`，发两条 `sampling/createMessage`）；② **远端 Streamable HTTP MCP 服务器**（Node，在 `tools/call` 的响应 SSE 流里反向推 `sampling/createMessage`）。全 exit 0：
    - ① **stdio「本会话记住」**：同一会话内夹具连发两条 sampling ⇒ `-ExpectSamplingCards 1`（**只弹 1 张卡**）、两条都成功（`SAMPLING_ALL_OK=true`）、`-ExpectSubagentUsage` 过 ⇒ 第二次是**记住后直接放行**。
    - ② **拒绝 ⇒ 不记住**：`-AutoApprove -DenySampling` ⇒ `-ExpectSamplingCards 2`（两次都要卡）、`SAMPLING_ALL_OK=false`（`-32000 "the user denied this request"`）。
    - ③ **HTTP 反向通道 + 警告卡 + 记住**：`-McpConfigFile <远程 HTTP 配置>` + `-AutoApprove` ⇒ 卡明细打印 `sampling card #1: server=http-fixture viaHttp=True`（**警告卡**）、`-ExpectSamplingCards 1`（服务器连发两条、只弹一张 ⇒ HTTP 路径也记住）、工具结果含 `HTTP_SAMPLING_REPLY_1000/1001` 的 `result`（反向往返成功）、`-ExpectSubagentUsage` 过。
    - ④ **回归：单发 `--print` + `--dangerously-skip-permissions` 仍不自动放行**：`-Print -PermissionMode full -DangerouslySkipPermissions -McpConfigFile <远程 HTTP>` ⇒ 工具被 skip-permissions 放行、服务器推来两条 sampling **都被拒** `-32000 "no user-approval channel (single-shot / --print mode)…"`。
    - ⑤ **回归：超 `MAX_CALLS` ⇒ 拒**：`LUNAC_MCP_SAMPLING_MAX_CALLS=1` + 记住场景 ⇒ 第 1 条 `ok=true`、第 2 条 `-32000 "session call limit reached (1, LUNAC_MCP_SAMPLING_MAX_CALLS)"`（此时已记住 ⇒ 无卡，只被护栏拦住）。
- **前端配套 + 真机验收口径**：`control_request` 新增 `subtype = "mcp_sampling"`（并带 `viaHttp`）→ `showSamplingCard(requestId, server, messageCount, maxTokens, viaHttp)`（`.approval-card.mcp-sampling-card`；`viaHttp` 时再加 `.mcp-sampling-warn`）；`control_cancel_request` 一并收卡；i18n `agent.sampling_*`（title / body / **session** / **http_banner** / messages / max_tokens / allow / deny = 8 键）× 5 语言。**真机验收（需在 release / 安装版点一遍）**：
  - **常规卡（stdio 路径）长这样**：标题「MCP 服务器请求调用模型」+ 服务器名 + 「消息数 / 最大 token」两行 + 费用提示 + 「点允许后本会话内该服务器不再询问（重启后需重新确认）」；按钮**只有「允许（本会话）」「拒绝」**（**无「始终允许」**）。
  - **警告卡（HTTP 反向路径）与之的差别**：卡片**左缘多一条 `--yellow` 竖线**，正文顶部多一行**「这条请求由服务器经 HTTP 主动推来（反向请求），不是你发起的调用」**（其余同常规卡）。
  - **允许文案**：两卡按钮一致 =「允许（本会话）」；点后服务端拿到模型回复、用量计入 `result.usage.subagent`。
  - **记住后的表现**：**同一服务器**在**同一会话内**再次请求 ⇒ **不再弹卡**、直接放行；**拒绝后**再次请求 ⇒ 仍弹卡；**重启应用（= 新 agent 进程）后** ⇒ 重新弹卡（跨进程不记住）。
  - 把运行方式切到「自动」（= `--dangerously-skip-permissions`）后再触发 ⇒ **仍弹卡**、不自动放行。
- **涉及文件**：`core-agent/src/mcp.rs`（`SamplingFactoryFn` 加 `via_http` 入参、`connect_stdio_server` / `connect_http` / `McpSet::connect` / `add_project` 各装钩子、`handshake` 默认声明、`http_start` / `request_http` / `read_http_sse` / `dispatch_http_event` / `flush_sse`（HTTP 反向通道）、`answer_server_request` 文案）、`core-agent/src/main.rs`（`sampling_handler_for(server, via_http)`、`mcp_sampling_create(server, params, via_http)` 的高底线闸 + `open_sampling_approval`（含 `MCP_SAMPLING_APPROVED` 记住集 + `sampling_approval_key` + `viaHttp` 载荷）、装配与 `detached()` 注入、删开关）、`app/src/main.ts`（`showSamplingCard(..., viaHttp)` 警告样式 + 事件分支 + 事件类型字段 `viaHttp`）、`app/src/i18n.ts`（+2 键）、`app/src/styles.css`（`.mcp-sampling-warn` / `.mcp-sampling-warn-line`）、`scripts/agent-e2e.ps1`（`-McpConfigFile` / `-ExpectSamplingCards` / 卡片明细打印 / 交互式回包器垫空行修复）。
- **用户裁决已落地（2026-10-11 第 3 轮）**：上一轮遗留的「要不要支持一次同意后本会话内记住该服务器」**已裁定为「支持」**并实现（含卡片必须告知「允许 = 本会话内不再询问」）；同时补上“HTTP 反向通道”（做了，见上）与“反向警告卡”。
- **用户裁决已落地（2026-10-11 第 4 轮，两条）**：① **接受「服务器 gate 住 `tools/call` 响应」这条残留边界** —— 那是**服务器自己的行为**（它扣住响应直到收到我们的反向响应），**客户端消不掉**；GET 独立流通道**不受影响**。要彻底免疫需把 POST 响应流也改成「独立线程读 + 主线程按调用预算等」，属另开一轮的更大改动，本轮不做。② **确认回环 URL 一律 `.no_proxy()`** —— 符合既有规范，且是「本机代理开启时本地 HTTP MCP 能通」的必需项（原 HTTP client 从未设 `.no_proxy()`，与 `code-standards.md` 的相关要求不符，本轮为解 502 才补，**仅回环**，远端仍走系统代理）。**「本会话记住」的键确认为「传输 + 服务器名」**（不用名字，防同名跨传输被静默放行）。

- **第 4 轮（2026-10-11）：把 HTTP 反向通道做完整** —— 用户裁决：① 补「客户端 GET 独立 SSE 流」那条通道；② 一并要心跳容忍 / 断线重连（含退避）/ 401 刷新后重建流；③ **解掉「反向请求在卡上等用户的时间算进该次 `tools/call` 的 180s 预算」**；④ 全部闸门、本会话记住（键 = 传输 + 服务器名）、反向警告卡、`不带 tools`、忽略 `modelPreferences`、用量归因、`MAX_TOKENS`/`MAX_CALLS` 第二层兜底**保持不变**。
  - **实现（`core-agent/src/mcp.rs`，单文件；`main.rs` 仅类型 `+Send`，零逻辑改动）**：
    - **抽出可跨线程共享的 `HttpChannel`**（`client`/`url`/`headers`/`session`/`oauth`）—— GET 独立流要一条**常驻线程**（长连接），它拿不到主线程那条 `&mut Bridge`（同 `run_review_fork` 的约束），只能共享 `Arc<HttpChannel>`；`session`/`oauth` 包 `Mutex`（两条线程都会写）。另配一个**不设总超时**的 client 专供长连接（用 180s 的 client 会让流每 180s 被掐断）。
    - **`start_reverse_channel`**：每台 HTTP 服务器在 `connect_http` 的 `handshake` **之后**起两条常驻线程 —— ① **流线程**（`stream_loop`）常驻读客户端 GET 独立 SSE 流；② **处理线程**（串行）接服务端请求 → `answer_server_request`（含采样钩子：高底线审批卡 + 护栏）→ **另发一次 POST** 回 JSON-RPC response。`initialize` 后开流 = 规范要求。
    - **心跳容忍**：抽 `SseDecoder`（`read_http_sse` / `read_stream` / `parse_sse` 三处共用）—— SSE 注释行（`:` keep-alive）/ `event:` / `id:` / `retry:` / 空行都**不算事件、也不会让读取循环退出**；只有「空行收尾 + 有 `data:`」才产出 JSON。
    - **断线重连（含退避）**：`stream_retry_delay(attempt) = 1s × 2^attempt，封顶 30s`（纯函数，可单测）；流结束（服务器关闭 / 网络断）即按退避重连；`sleep_interruptible` 每 200ms 看一次 stop。
    - **401 刷新后重建**：`HttpChannel::open_stream` 在 OAuth 模式遇 401 时刷新令牌后重发一次（与 `post_start` 同一口径）；服务器不支持 GET 时按规范回 **405/404 ⇒ 只记一行、不重连**（避免对不支持的服务器无限重连）。
    - **优雅收尾**：`Bridge::Drop` 对 HTTP 置 `stop`（流线程不再重连、退避 sleep 尽快醒；处理线程因 `tx` 随结构体 drop 而退出）。**正在阻塞的 GET 读**由**进程退出**收尾（reqwest blocking 无「按读超时」可设）—— 进程退出即终止全部线程，**不留孤儿**。
    - **180s 边界解法（明确机制）**：服务端请求**一律只被「转发」**给上文的**处理线程**（`forward_reverse`），主线程的请求-响应路径（`read_http_sse` / GET 流线程）**自己不等待**。「审批卡在等用户」这件事只发生在处理线程，**不进任何一次 `tools/call` 的超时预算** ⇒ 反向请求**不会**把某次工具调用拖超时。**用户长时间不点时**：处理线程按既有审批超时（`APPROVAL_TIMEOUT_SECS`，见 `await_approval`）到点按**拒绝**处理、回服务端 `-32000 ... the user denied this request` **并撤卡**（`control_cancel_request`）—— **绝不静默挂起**；主线程那次工具调用早已正常返回。**如实记残留**：若服务器把某次 `tools/call` 的响应**扣住**直到收到我们的反向响应（gate），该次调用仍受**服务器**决定（网络读受 `CALL_TIMEOUT=180s` 约束）—— 但这已与「我们等用户」解耦，且 GET 独立流那条通道完全不受影响。
    - **顺带修（本次必需）**：`connect_http` 的 HTTP client 对**回环** URL 加 `.no_proxy()` —— reqwest 在 Windows 上读系统代理却**不认绕过表**（本机 `ProxyEnable=1` + `127.*` 在 `ProxyOverride` 里也没用），实测本地 HTTP MCP server 会被系统代理接管返回 **502** ⇒ 本地 HTTP MCP 完全不可靠。远端服务器仍走系统代理。
  - **如实记的边界**：① 同一台服务器的多个反向请求由**一个处理线程串行**处理（不会同时弹两张卡）；② 每台 HTTP 服务器**一条** GET 流；③ 长连接 **HTTP/1.1 连接数**：每台服务器多占 1 条 GET 长连接（+1 个 client 连接池），服务器端若按连接数限流需自行留意；④ 服务器不支持 GET 通道时该台只走 POST 响应流（能力降级、非失败）。
  - **验收口径（release 目标，2026-10-11 第 4 轮实测）**：`cargo build --release` exit 0（仅既有 `BgCmd` dead_code 告警）、`cargo test` **179 passed / 0 failed / 3 ignored**（本轮新增 3 个纯函数单测：`SseDecoder` 心跳容忍、退避序列、回环判定）。**未改 `app/`**；`scripts\agent-e2e.ps1`（**未改脚本**，复用 `-McpConfigFile` / `-AutoApprove` / `-DenySampling` / `-ExpectSamplingCards`）配**新建的 Node HTTP MCP 夹具**（**建在系统 temp、仓库之外，用后已删**；三种模式 `gate` / `reconnect` / `nogate`；POST 响应**不含**任何服务端请求，**只经 GET 流推送**）逐场景 exit 0：
    - ① **只经 GET 流推送 + 警告卡 + 记住 + 拿到回复**：`-AutoApprove` ⇒ 夹具日志 `get_stream_open`（GET 流建立）、2 条 `sampling_response`（`text_len=15`，即回复回来了）；脚本侧 `sampling card #1: server=http-get-fixture viaHttp=True`（**警告卡**）、`-ExpectSamplingCards 1`（连推 2 条只弹 1 张 ⇒ **记住**）、`-ExpectTool mcp__probe`、`-ExpectNoError`，工具结果含 `GET_SAMPLING_REPLY_1000=GETSAMPLING1000; …_1001=GETSAMPLING1001`。
    - ② **心跳容忍**：夹具每 300ms 发 SSE 注释行（`: keep-alive …`），① / ③ 里 sampling 仍正常到达 ⇒ 注释行未被打成事件、也没断流。
    - ③ **断线重连**：`reconnect` 模式 —— 夹具 GET#1 推完 `1000` 后**关闭连接**，代理重连 → 夹具日志 `get_stream_open n=2` → 在新流上推 `1001`；两条回复都收到（`1001` **只能**经重连后的流到达）⇒ 重连成功、新流继续收请求。
    - ④ **180s 边界已解**：`nogate` 模式（tools/call 立即返回、采样异步推）+ **不点卡**（无 `-AutoApprove`）⇒ 夹具日志有 `push_sampling` 但**没有** `sampling_response`（卡一直没人点），而该次 `tools/call` **1.4s 就返回**、脚本 exit 0（run 目录 `stream.jsonl` 里确有未应答的 `control_request`（`subtype=mcp_sampling`、`viaHttp=true`））⇒ 未点的卡**没有**把这次工具调用拖超时。
    - ⑤ **回归**：单发 `--print` + `--dangerously-skip-permissions` ⇒ 工具被放行、两条 sampling 都被拒 `-32000 "no user-approval channel (single-shot / --print mode)…"`（**直接调 `agent.exe` 得退出码 1**；夹具日志为证）。`-DenySampling` ⇒ **弹 2 张卡**（拒绝不记住）、两条都 `-32000 "the user denied this request"`。`LUNAC_MCP_SAMPLING_MAX_CALLS=1` + 同意 ⇒ 1 张卡、第 1 条 ok、第 2 条 `-32000 "session call limit reached (1, …)"`。常驻 + `--dangerously-skip-permissions` ⇒ **仍弹卡**（不自动放行）。
  - **涉及文件**：`core-agent/src/mcp.rs`（`HttpChannel`（`build` / `remember_session` / `post_json` / `post_start` / `refresh_oauth` / `open_stream`）、`HttpTransport{channel,reverse}`、`ReverseJob` / `ReverseChannel` / `start_reverse_channel` / `stream_loop` / `read_stream`、`stream_retry_delay` / `sleep_interruptible` / `SseDecoder`、`request_http` / `read_http_sse` / `dispatch_http_event` / `forward_reverse` / `http_post` / `Bridge::Drop`、`connect_http`（握手后起反向线程 + 回环 `.no_proxy()`）、`url_host` / `is_loopback_url`、`SamplingHandler` 加 `+ Send`、文件头能力边界注释、3 个新单测）。**未改** `core-agent/src/main.rs`（采样钩子已是 `Box<dyn FnMut>`，仅类型别名加 `Send` 在 `mcp.rs` 侧），**未改** `app/`，**未改** `scripts/agent-e2e.ps1`。

#### §待办 3-1g `SendUserFile` / `PushNotification`（agent → 用户呈递，**已实现**，2026-10-11）

- **既定语义（引自本节待办清单与 `docs/archive/agent-feature-backlog.md` §1 A13）**：两项列在「**面向远端 / 移动端的推送通道**」一栏；§待办 3-1 只**点名**、未展开细则（全文只有那一句）。故按**桌面端最小形态**落地并在「待裁定」里写明取舍：**agent 把一个本地文件呈递给用户**（对话流出现一张文件卡）/ **agent 发一条桌面通知**（前端弹出）。
- **已实现**：
  - `core-agent/src/tools.rs`：`defs()` **追加**两件（**未重排**既有 21 件）⇒ 工具数 **23**；`send_user_file(ctx, input)`（返回 `(回执, 载荷)`；载荷 = `path` / `name` / `size`（字节）/ `kind`，`preview` 仅在显式要求且为文本类时才有）/ `push_notification(input)`（载荷 = `title` / `body`）/ `classify_media_kind` / `previewable_kind` / `push_notification_wait_ms`（纯函数节流判据）+ 4 条单测；两件在 `run()` 里各有一条直通臂（`tools.rs` 独立可调）。
  - `core-agent/src/main.rs`：`dispatch_tool` 在两件上把载荷发成**既有 `system` 通道**的两条新子类型 —— `system/user_file`（`file:{path,name,size,kind,preview?}`）与 `system/push_notification`（`notification:{title,body}`）—— 再回一句回执给模型（`tools.rs` 拿不到 `emit`，与 `PetControl` 同型）；`subagent_tool_defs` **剔除**两件（子代理的 `tool_use` 不进前端助手流，在里面调只会冒出一张无上下文的卡）+ 守门单测断言。
  - `app/src/main.ts`：`TOOL_BLACKLIST_CANDIDATES` 加两件（用户可关）；两条 `system` 事件分支 ⇒ `showUserFileCard(data.file)`（`.file-handoff-card`）与 `showPushNotification(title, body)`（`#push-toast-host` 浮层）；两件的 `tool_use` **不建命令卡**（同 `PetControl` 的拦截），其 `tool_result` 也**不**回填到别的卡上；辅助 `formatFileSize` / `userFileKindLabel`；新增线性图标 `FILE_SVG`。
  - `app/src/styles.css`：`.file-handoff-*` 与 `.push-toast-*`（颜色全走**四类变量 / 语义色白名单**，未新造字面量；滚动条走 §界面 6 全局规则）。
  - `app/src/i18n.ts`：`agent.user_file_*` / `agent.notify_dismiss` 共 **15 键 × 5 语言**。
  - `scripts/agent-e2e.ps1`：**新增** `-ExpectEventContains "<subtype>:<needle>"`（断言某条 `system` 事件含某子串 = 载荷字段取证）。
- **审批判定（与既有约定一致 —— 两件都免审批）**：`SendUserFile` 只是**读**一个本地文件的元信息（`resolve` + `guard` + `stat`，与 `Read` **逐行同源**；可选预览也只在**本地**界面显示），既有约定里 **`Read` 不需要审批** ⇒ 与它同级，本工具同样不需要 —— **不**是绕过 `Read` 审批的旁路（它既不读全文也不把内容带出本机）。`PushNotification` 只发一条 `system` 事件、前端弹一条**本机**通知，无本机副作用、无外部出口，与 `TodoWrite` / `PetControl` 同级。两件在**只读档 / 计划相位**下也都**放行**，**不进** `gated_in_read_only`；**不进** `parallel_safe`（需串行以保证 `emit` 顺序与节流时间戳），理由各写在 `tools.rs` 对应注释里。
- **路径与安全**：不存在的文件返回**可读错误** `<path>: <os error>`（与 `Read` 同一措辞）、**绝不静默成功**；目录被明确拒绝；工作区锁下越界由 `guard` 拒。
- **大文件 / 二进制**：只给元信息；`preview=true` 时仅对文本 / 代码类、且 **≤ 128 KB** 的文件附 **≤ 1200 字符**预览 —— 大文件与二进制**永不**整份进事件。
- **节流值**：`PushNotification` 进程级最短间隔 = **3000 ms**（`PUSH_NOTIFICATION_MIN_INTERVAL_MS`）—— 间隔内第二次**直接拒**并如实告知还需等多久；标题截断 120 字符、正文 500 字符。
- **验收口径（release 目标，2026-10-11 实测）**：
  - `cargo test`（cwd=`core-agent`）**exit 0 / 183 passed / 0 failed / 3 ignored**（含新增 4 条：`send_user_file_reports_metadata_and_errors` / `send_user_file_preview_is_bounded_and_binary_excluded` / `push_notification_wait_ms_is_a_pure_gate` / `push_notification_throttles_and_truncates`，以及改判后的 `agent_tool_is_registered_and_filterable` **21→23**）。
  - `cargo build --release`（cwd=`core-agent`）**exit 0**（Finished release profile；1 条既有 `BgCmd` `dead_code` 告警，**非本轮引入**）。`npx tsc --noEmit` / `npm run build`（cwd=`app`）、`npm run verify`（仓库根）**均 exit 0**。
  - **端到端（`scripts\agent-e2e.ps1`，release，全 exit 0 / PASS；夹具建在系统 temp、仓库之外，用后已清）**：
    - ① **成功呈递文件**：`-PermissionMode plan -Model deepseek-flash -Query "Call the SendUserFile tool … file_path = <temp>\report.txt … DONE" -ExpectTool SendUserFile -ExpectEvent user_file -ExpectEventContains user_file:report.txt -ExpectContains DONE` ⇒ 工具池 **23**、工具序列 `[SendUserFile]`、`stream.jsonl` 得 `{"file":{"kind":"text","name":"report.txt","path":"…\\report.txt","preview":"…","size":24},"subtype":"user_file","type":"system"}`。
    - ② **失败路径（不存在的文件）**：同法把 `file_path` 指向 `<temp>\missing.txt` ⇒ 工具序列 `[SendUserFile]`、**无** `system/user_file` 事件（计数 0）、`tool_result` = `{"content":"Error: …\\missing.txt: 系统找不到指定的文件。 (os error 2)","is_error":true,…}` ⇒ **可读错误、不静默成功**。
    - ③ **桌面通知**：`-Query "Call the PushNotification tool … title = BuildDone and body = Finished … DONE" -ExpectTool PushNotification -ExpectEvent push_notification -ExpectEventContains push_notification:BuildDone -ExpectContains DONE` ⇒ `{"notification":{"body":"Finished","title":"BuildDone"},"subtype":"push_notification","type":"system"}`。
- **真机验收口径（待做，release 目标）**：
  - 起 dev 实例（`cd app ; npm run tauri:dev`）→ 开聊天独立窗，让模型调 `SendUserFile`：对话流出现**文件卡**（标题「向你发送了一个文件」+ 折角文件图标；文件名加粗、其下一条完整路径、再下一行「大小 · 类型」，如「24 B · 文本」），两颗按钮 **「打开」（宿主 `open_path`，按关联程序打开）/「在资源管理器中显示」（宿主 `reveal_in_explorer`，定位该文件）**；`preview=true` 时卡上多一个可展开的「预览」块（等宽小字、限高可滚）。文件被移走后点按钮，状态行如实提示「无法打开该文件…」「无法定位该文件…」。
  - 让模型调 `PushNotification`：窗口**右下角**弹出一条通知浮层（标题加粗 + 正文 + 关闭按钮），**6 秒**后自动收起；点关闭按钮立即收起。**同一会话 3 秒内第二次调用** ⇒ 被节流：**不弹第二条**，模型收到「rate-limited… wait N ms」并如实回话。
- **涉及文件**：`core-agent/src/tools.rs`（schema ×2、`send_user_file` / `push_notification` / `classify_media_kind` / `previewable_kind` / `push_notification_wait_ms` / `PUSH_NOTIFICATION_MIN_INTERVAL_MS` / `LAST_PUSH_NOTIFICATION`、`run()` 两条直通臂、4 单测、工具数守门 21→23）；`core-agent/src/main.rs`（`dispatch_tool` 两条事件分支、`subagent_tool_defs` 剔除 + 单测）；`app/src/main.ts`（`TOOL_BLACKLIST_CANDIDATES`、两条 `system` 分支、`showUserFileCard` / `showPushNotification` / `formatFileSize` / `userFileKindLabel` / `FILE_SVG`、`tool_use` 拦截与 `tool_result` 排除）；`app/src/styles.css`；`app/src/i18n.ts`；`scripts/agent-e2e.ps1`（`-ExpectEventContains`）。
- **新登记的事件字段（沿用既有 `system` 通道，不新造顶层类型）**：`system/user_file` 的 `file{path,name,size,kind,preview?}`；`system/push_notification` 的 `notification{title,body}`。前端**兼容缺字段**（无 `file` / `notification` 即不渲染）。
- **待裁定 / 取舍（如实记）**：① 文档对两项只给了「面向远端 / 移动端的推送通道」一句、桌面端**无对应设施** ⇒ 本轮落成**应用内**文件卡 + **应用内**桌面通知浮层；**未**引入 OS 级 toast（需 `tauri-plugin-notification` 或 WinRT `UI_Notifications`，超出「只做这两个工具 + 必要前端与 i18n 配套」的范围）。若要求真·系统通知，请裁定是否允许加该依赖。② 两件都**免审批**（判据见上）；若日后要收严，最小改法是收进 `needs_approval` —— 但那会与「`Read` 免审批」不一致，需先裁定。
- **顺带修复（如实记，非本次功能引入）**：`scripts/agent-e2e.ps1` 与 `scripts/build-plugins.ps1` 原有**双 BOM**（`EF BB BF EF BB BF`），令仓库根 `npm run verify` 的 §8 报 FAIL；本次按 `docs/testing.md` §测试 3 的口径把两者归一为**恰好一份** BOM（内容未改）。

#### §待办 3-1h `LSP`（语言服务器诊断，**已实现**，2026-10-23）

- **目标**：让模型在 `Write` / `Edit` 之后自己**看诊断**（等价于编辑器里飘红的那些），而不是靠猜「大概能编译」。
- **形态**：`LSP` 工具，入参 `file_path`（必填）/ `timeout`（毫秒，默认 20000、1000–60000）/ `limit`（默认 50、1–200）；返回**恒为 JSON**：`{file, language, server, diagnostics[{line,column,severity,message,source?,code?}], total, reason, error, servers}`。
- **实现**：`core-agent/src/lsp.rs`（新模块，~700 行含注释与单测）——
  - **服务器探测**：按扩展名选 `languageId` + 候选可执行名（`.rs`→rust-analyzer；`.ts/.tsx/.js/.jsx/.mjs/.cjs`→typescript-language-server `--stdio`；`.py/.pyi`→pyright-langserver `--stdio`，退 pylsp；`.go`→gopls；`.c/.h/.cc/.cpp/.cxx/.hpp/.hxx/.hh`→clangd；`.lua`→lua-language-server）。**只认 PATH**（Windows 额外试 `.exe/.cmd/.bat/.com`；`.cmd/.bat` 经 `cmd /C` 起）。
  - **协议**：手写 LSP over stdio（`Content-Length` 帧），`initialize → initialized → textDocument/didOpen → publishDiagnostics`；收到**非空**诊断即返回，只有空诊断时等到预算耗尽（rust-analyzer 会先推空列表、分析完再推真实结果）。期间**自动应答**服务端请求（`workspace/configuration` 回等长 `null` 数组，其余回 `null`），否则 clangd / rust-analyzer 会卡在等回执。
  - **超时与进程**：stdout / stderr 各一个读线程 + channel（主线程带 deadline 等）；结束（含所有失败路径）一律 `kill` + `wait`，**不留孤儿进程**；stderr 只留尾部 600 字符，失败时并进 `error`（说明「为什么起不来」）。
  - **失败语义（fail-open，绝不抛硬错误）**：`unsupported-extension` / `no-server`（附候选名）/ `spawn-failed` / `initialize-failed` / `server-exited` / `timeout` / `read-failed`。**只有缺参数才 `Err`**。
  - **接入**：`tools::defs()` 新增一件（**工具数 31 → 32**，守门单测已改）；`tools::run()` 一条直通臂；`tools.rs` 新增 `resolve_read_path` / `lsp_root` 两个小助手（**路径归一 / 读类闸复用 `Read` 那一套**，不新造第二套）；`main.rs` 加 `mod lsp;`；`app/src/main.ts` 的 `TOOL_BLACKLIST_CANDIDATES` 加 `LSP`（用户可 `--disallowedTools LSP` 关掉）。
- **边界（如实记）**：① **只认 PATH** —— nvm / venv / VS Code 扩展里自带的服务器探测不到，回 `no-server`；`rustup` 的 `rust-analyzer.exe` 若所在工具链**没装该组件**，表现为 `server-exited` 且 `error` 里有原文。② **每次调用新起一个服务器**（不缓存）⇒ 冷启动慢，给了 `timeout`；③ **只查一个文件**，不是全项目体检；④ **只读档 / 计划相位放行**（与 `RepoMap` 同级：不写用户文件、不发网络；语言服务器自己的索引缓存不算工作区写入）；⑤ **免审批**（与 `Read` 同级）；⑥ **不进 `parallel_safe`**（起进程 + 重，串行）。
- **验证（2026-10-23）**：`cargo test --bins`（`core-agent`）**323 passed / 0 failed / 6 ignored**（新增 9 条：语言表 / `file://` 编码 / 帧往返 / 诊断转换 / 长消息截断 / 无服务器 JSON / 会话握手+自动应答 / 服务器中途退出 / initialize error / 超时；纯函数 + **脚本化假对端**，不依赖本机装没装服务器）。`cargo check --bins` 仅 1 条既有 `BgCmd` dead_code 告警；`npx tsc --noEmit`（`app`）exit 0。
- **真机验收口径（尚未执行，用户已裁定验收类暂缓）**：① 本机装好任意一个服务器（如 `rust-analyzer`）→ 让模型对**有类型错误**的文件调 `LSP` ⇒ `reason=ok`、`total≥1`、`severity=error`、行列是 1 基且与编辑器一致；② 干净文件 ⇒ `reason=ok`、`diagnostics=[]`；③ 删掉 / 改名服务器 ⇒ `no-server` 且 `servers` 列出候选；④ 未知扩展名（如 `.txt`）⇒ `unsupported-extension`；⑤ 把 `timeout` 设 1000 跑一个大项目 ⇒ `timeout`（不是崩溃）；⑥ 全程 `tasklist` 里不残留语言服务器进程。
- **已知未覆盖**：本机 `rustup` 的 `rust-analyzer.exe` 是**空 shim**（`rustup component add rust-analyzer` 在 `x86_64-pc-windows-gnu` 上 404）⇒ 真机冒烟测试 `lsp_real_server_smoke`（`#[ignore]`）在本机**自动跳过**；换一台装了可用 RA / clangd 的机器再跑：`cargo test --bins lsp_real_server_smoke -- --ignored --nocapture`。

#### §待办 3-1i `WebBrowser`（无头浏览器渲染，**已实现**，2026-10-23）

- **目标**：读**跑完 JS 之后**的页面（SPA / 脚本才出内容的站点）——`WebFetch` 只做静态 GET，拿不到这类页面。
- **方案（零新依赖、零新下载）**：调用**本机已装的** Edge / Chrome，`--headless=new --dump-dom` 取渲染后的 DOM；不引 Playwright / Puppeteer（不下载内核、不加 Node 运行时）。
- **形态**：入参 `url`（必填）/ `wait_ms`（虚拟时间预算，默认 5000、上限 20000）/ `format`（`text` 默认 / `html`）；返回 `URL / Title / Rendered: headless <浏览器名>` + 正文（`text` 走**同一个** `html_to_text`；`html` 回原始 DOM，60k 字符封顶；正文 40k 字符封顶，**只在真截断时**才加提示）。
- **实现**：`core-agent/src/webbrowser.rs`（新模块）——
  - **浏览器探测**：`LUNAC_BROWSER`（显式路径）> PATH 上的 `msedge` / `chrome` / `chromium` > 平台常见安装路径（Windows：Edge x86 / x64、Chrome x64 / x86；macOS / Linux 各一条列表）。都找不到 ⇒ 可读错误「no Edge / Chrome found…」。
  - **参数**（纯函数、可单测）：`--headless=new --dump-dom --disable-gpu --hide-scrollbars --no-first-run --no-default-browser-check --disable-extensions --mute-audio --user-data-dir=<临时 profile>` + `--virtual-time-budget=<wait_ms>` + **URL 放最后**。
  - **进程**：stdout / stderr 各一个读线程（边跑边读，DOM 超过管道缓冲不会把双方卡死）；总时限 = `wait_ms + 20 s`，到点 `kill + wait`；**每次独立临时 profile，用完即删**（不碰用户日常配置 / cookie）；`CREATE_NO_WINDOW` 不闪黑框。
  - **复用**：URL 校验用 `tools::normalize_url`（http→https 升级、长度封顶）、DOM→文本用 `tools::html_to_text`（**只许一份实现**，不新造第二套）。
  - **接入**：`tools::defs()` 新增一件（**工具数 32 → 33**，守门单测已改）；`run()` 一条直通臂；**`needs_approval` + `gated_in_read_only` 都加它**（外部网络出口，与 `WebFetch` 同判据：自动档静默、只读档仍要问）；**不进 `parallel_safe`**（起进程、重）；`app/src/main.ts` 的 `TOOL_BLACKLIST_CANDIDATES` 加它（可 `--disallowedTools WebBrowser` 关掉）。
- **边界（如实记）**：① **不是沙箱** —— 浏览器以当前用户身份运行；用独立临时 profile 只是不碰用户数据，不等于隔离（`forbidden.md` §禁用 1）；② **只能打开 + 等待 + 取 DOM**，没有 `click` / `type` / 截图（那要 CDP 常驻会话，archive backlog 原描述就是「需要常驻上下文」，本轮不做）；③ 每次调用**起一次浏览器**（约 1–5 s 起 + `wait_ms`），纯文档页优先用 `WebFetch`；④ 页面若需交互才出内容，本工具拿到的仍是初始 DOM。
- **验证（2026-10-23）**：`cargo test --bins`（`core-agent`）**328 passed / 0 failed / 7 ignored**（新增 5 条纯函数单测：参数拼装 / `wait_ms` 上限 / Edge+Chrome 候选覆盖 / 标题提取与实体解码 / 读管道封顶）；`cargo check --bins` 仅 1 条既有 `BgCmd` dead_code 告警；`npx tsc --noEmit`（`app`）exit 0。
- **真机冒烟（已跑，本机 Edge）**：`cargo test --bins webbrowser_real_browser_smoke -- --ignored --nocapture` ⇒ **1 passed**（1.45 s），输出 `Title: T1` / `Rendered: headless msedge (waited 2000 ms)` / 正文 `Hello` —— 证明参数、进程、读管道、标题与正文转换**真的走通**（用 `data:` URL，不联网）。
- **真机验收口径（尚未执行，用户已裁定验收类暂缓）**：① 对**静态页**（如 example.com）调 `WebBrowser` ⇒ 有标题与正文；② 对**必须跑 JS 才出内容**的页面 ⇒ 正文非空（`WebFetch` 同页应为空 / 极少），比较两者差异；③ `format="html"` ⇒ 回原始 DOM（含标签）；④ 断网 / 非法 URL ⇒ 可读错误；⑤ 审批：自动档静默放行、只读档仍需确认；⑥ 调用结束后 `tasklist` 无残留 `msedge` / `chrome`（且 `%TEMP%\lunac-webbrowser-*` 已删）。

#### §待办 3-1b 补：斜杠命令候选菜单（**已实现**，2026-10-11）

- **目标**：输入 `/` 时弹候选清单、随输入过滤、↑↓ 选择、Enter/Tab 采纳、Esc 关闭、鼠标可点选；不破坏既有 Enter=发送 与 IME。
- **已实现**：
  - 触发 = 光标前文本匹配 `^\s*\/(\S*)$`（命令位置）；过滤按 `SLASH_MENU` 的命令名前缀（大小写不敏感）。**只认「整条消息的首个 token」**（与 `parseSlashCommand` 对齐；「hello /rew」不弹）。
  - 键盘：↑↓ 循环、Enter(非 Shift)/Tab 采纳、Esc 关闭；**菜单打开时 Enter = 采纳（不是发送）**，采纳后关菜单、再按一次 Enter 才发送。Shift+Enter 仍换行；`isComposing` 组合输入期不弹、不采纳（`compositionstart/end` + `e.isComposing` 双守卫）。
  - 鼠标：hover 高亮、点击采纳（`mousedown` 阻止默认以保住输入框焦点）。
  - 采纳 = 把输入框内容整体替换成该命令（`/memory ` 带尾空格便于补 `clear`/`show`），光标置尾，**不直接执行**（破坏性命令 `/rewind`、`/memory clear` 仍走既有二次确认）。
  - 未识别 `/xxx` 仍当普通文本发（解析语义**未动**）。
  - 命令清单单一真相源 = `SLASH_MENU`（`app/src/main.ts`），`SLASH_COMMANDS` 由它派生（不再另抄一份）。
  - UI：`#chat-slash-menu` 复用「更多」菜单的贴底向上定位（`bottom:calc(100%+6px)`）与 `.custom-select-option` 的按钮蒙版口径；颜色全走四类变量，滚动条走全局唯一那条，无新字面量颜色；文案走 `t()`（`chat.slash_menu_*` 5 键 × 5 语言）。
- **验收口径（release 目标）**：`npx tsc --noEmit` / `npm run build`（cwd=app）/ `npm run verify`（仓库根）全 exit 0；真机逐条：① 输 `/` 弹候选、首条高亮；② 输 `/m` 过滤、输 `/x` 菜单消失；③ ↑↓ 循环、hover 跟随；④ Enter/Tab 采纳且**不发送**；⑤ Esc 关菜单后 Enter 恢复发送；⑥ 点条目采纳且焦点仍在输入框；⑦ `/memory ` 带空格自动关；⑧ IME 组合期不弹/不采纳；⑨ `/foo` 回车当普通文本发；⑩ `a/b` 里的 `/` 不弹；⑪ 菜单不覆盖输入框、布局尺寸不变。
- **顺带发现（仅报告）**：`main.ts` 既有 Enter=发送分支本身**没有** `isComposing` 守卫（非本轮引入），本轮只保证候选菜单不因 IME 误触发，未改既有发送行为。
- **涉及文件**：`app/src/index.html`（`#chat-slash-menu`）、`app/src/styles.css`（`#chat-slash-menu` / `.chat-slash-item`）、`app/src/main.ts`（`SLASH_MENU` + 菜单逻辑 + keydown 接线）、`app/src/i18n.ts`（`chat.slash_menu_*`）。

### §待办 3-2 U3 真沙箱调研
- **目标**：Windows 侧隔离手段（Job Object 资源限制 / 低完整性级别令牌 / AppContainer）调研。
- **约束**：**结论需单独立文档**，不得与 [`ui-style.md`](./ui-style.md) / archive `agent-ui-spec.md` §4 的「运行方式」三档混称——那三档是**策略级**的。
- **结论（2026-11-01，已完成）**：见 [`sandbox-research.md`](./sandbox-research.md)。五条要点：
  ① 不承诺「黑箱沙箱」，对外只写「策略边界 + 可见审计」；② 「白箱」= 可解释 / 可预演 / 可审计 /
  可撤销 / 可关闭；③ 近期先做 **W0 决策账本 + 原因码 + 预演 + 导出**（零新依赖、不改行为）；
  ④ 中期用 **MXC ProcessContainer** 只包 `Cmd` / `PowerShell` 子进程，**从 `learningModeLogging`
  （拒绝并记录）起步**，`denials.json` 进同一本账；⑤ 不做：AppContainer 铺全量插件、WDAC、
  Windows Sandbox、自研驱动。
- **状态**：调研完成；**W0 已施工（2026-11-01）** —— 决策账本 / 原因码 / 预演 / 导出已落地，入口在
  聊天窗「更多设置 → 安全」（在「其他」之前），见 [`testing.md`](./testing.md) §测试 28。
  W1（MXC `learningModeLogging`）需 Windows 11 24H2 `26100.9278+` 实测，**待用户拍板**。

### §待办 3-3 L3 上下文感知提示词注入
- **已查清**：工作目录 / 宿主 / 技能目录 / 插件目录已进系统提示词（`env_block`，进程内常量，`main.rs:1664`）；前端已有按 query 关键词的 `buildSystemPromptHint()`（**发送期拼进那条 user 消息开头**，`app/src/main.ts:9598`）；附件路径走 `[Attached files]` 文本；长期记忆走 `context_block_message`（`CONTEXT_BLOCK_HEADER`，`main.rs:1600`）作为**一条 user 消息**幂等注在 history 开头（`main.rs:5551`）。
- **「当前打开的插件」单一真相源 = 宿主**：AI 聊天是**独立 WebView**（label `plugin-chat`，`main.ts` 顶部窗口分工表）；它拿不到主窗 `activePluginId`，前端也无窗口枚举手段 ⇒ 只有宿主知道哪些插件窗开着。
- **已实现（2026-10-11）「当前打开的插件」（可变上下文，进消息、不进系统提示词）**：
  - **新增只读宿主命令 `open_plugin_ids()`**（`app/src-tauri/src/plugin_window.rs`）：`app.webview_windows()` 取 `plugin-` 前缀、去前缀、去重排序、**排除 `plugin-chat` 自己**；在 `app/src-tauri/src/main.rs` 注册。前端聊天窗发请求前问一次。
  - **注入形态**：`startAgentChat` 里拼成 `## Open plugin (this turn only)\n- <名称> (<id>)`（id 用 `pluginRegistry` 最长前缀匹配，附属窗如 `music-curve` → `music`），并入既有的发送期 `prefix` 数组（**与 `buildSystemPromptHint` 同一出口**，只进那条 user 消息；`chatHistory` 仍存干净 query，不污染历史）。
  - **幂等**：只在**相对上一次发生变化**时才带 —— `lastOpenPluginsSent` 是本地快照；`queueAgentHistory()`（恢复 / 回退 / 切线程会整份替换 agent history）与 `newConversation()` 处清为 `null`，下一轮重估。状态没变就不重复堆叠；**由非空变空**会补一条 `(none)` 纠偏；**首次**（快照为 `null` 且当前无插件）不带（否则每次新对话首问都多一句）。
  - **体积上限**：最多 **8** 个插件、整段 ≤ **300** 字符；只放标识（名称 + id），不塞内容。
  - **可观测**：注入内容就在**发出去的那条 user 消息**里 —— `LUNAC_DUMP_API=1` 时见 run 目录 `logs\api-dump.jsonl` 的请求体 `messages`（`log_frontend` 是错误/警告专用通道，**未**拿它打这种 info 行）。
  - **为什么不破坏缓存**：它随「哪些插件窗开着」高频变，放系统提示词会让端点前缀整段 miss；现只拼进那条 user 消息（可变层），系统提示词逐字节不变。
- **「当前选中的文件」：不存在这个独立概念，未做**（如实记，**未造新概念**）。逐个核实：① `attachedFiles`（文件 chip / 附件）**已有**且经 `[Attached files]` 文本交给模型（再注 = 重复）；② 搜索结果选中项（`selectedIndex`）/ 详情视图选中项（`detailSel`）是**瞬时高亮**，非稳定「选中文件」；③ 任务抽屉「已改文件」（`todoDrawerFiles`）是 **AI 改过的文件**（展示用），不是用户选中的文件；④ 无编辑器视图。⇒ 只做「当前打开的插件」那一半。**若要「选中的文件」需先裁定接哪一个**（见文末待裁定）。
- **待真机验收（2026-10-11 起）**：起 dev 实例（`cd app ; npm run tauri:dev`），先开一个插件窗（如音乐 / 桌宠）再开 AI 聊天问一句 —— 开 `LUNAC_DUMP_API=1` 后，run 目录 `logs\api-dump.jsonl` 的请求体 `messages` 应含 `## Open plugin (this turn only)` + `- 音乐 (music)`，而 `system` 不含；连续两轮状态不变时**只在第一轮出现**；关掉插件后再问一句应补一条 `(none)`。
- **脚本侧证据（已跑）**：`scripts\agent-e2e.ps1 -Query "## Open plugin (this turn only) - Music (music). …" -ExpectContains PONG -DumpApi` exit 0；解析 `api-dump.jsonl` 得 `messages_has_header = True` / `system_has_header = False` —— 证明「拼进 user 消息的上下文**进 `messages`、不进 `system` 固定前缀**」。**如实记**：脚本直连 `agent.exe`、无宿主前端 ⇒ 它**天然测不到前端那段拼装**，故脚本里是**手工喂**一条带该 header 的 user 消息，只验证 agent 侧不会把上下文挪进前缀。
- **涉及文件**：`app/src-tauri/src/plugin_window.rs`（`open_plugin_ids`）、`app/src-tauri/src/main.rs`（命令注册）、`app/src/main.ts`（`describeOpenPlugins` / `openPluginsCtx` / `lastOpenPluginsSent`）。
- **已裁定（2026-10-11，三条，按现状定稿、无需改代码）**：① **接受**为「当前打开的插件」新增的宿主只读命令 `open_plugin_ids()`（现架构下聊天窗拿不到主窗状态、也不能枚举窗口，没有「纯前端零宿主改动」的做法）。② **「当前选中的文件」不接** —— 该概念不存在，最接近的 `attachedFiles` 已有 `[Attached files]` 通道、再接即重复。③ **主窗内嵌插件面板不覆盖** —— 内嵌态下搜索栏被禁用、开不了 AI，判为不可达。

### §待办 3-4 L4 调试阶段状态栏
- **目标**：一个**开关控制**的阶段状态栏，展示 agent 当前处于哪个阶段。
- **已实现（2026-10-11）· 纯前端**：阶段**全部由既有 `stream-json` 事件派生**，**未改 `core-agent`**（故 `tools.rs` / `main.rs` 一行未动）。
  - **开关**：`localStorage` 键 `lunac-debug-phase`（值 `"1"` = 开；沿用既有 `lunac-*` 命名）。**UI 入口** = 聊天独立窗「更多设置 ⋯」菜单**最底部**新增一行「阶段状态栏（调试）」（两档分段，复用既有 `.chat-more-seg`，与「思考」开关同构）。
  - **浮标**：`#debug-phase-bar`（`position:fixed` + `pointer-events:none`，由 JS 在开关打开时挂到 `document.body`；颜色全走四类自定义变量 `--surface-glass` / `--border-glass` / `--text`；单行 `nowrap + ellipsis`，**无滚动条**）。只在**聊天独立窗**且开关打开时创建节点（`setDebugPhase` 首行即 `if (!debugPhaseOn || !IS_CHAT_WINDOW) return`）。
  - **阶段集合（11）**：`启动中 / 空闲 / 请求模型 / 思考中 / 生成回复 / 执行工具（带工具名）/ 等待审批 / 压缩上下文 / 子代理运行中 / 完成 / 出错`。派生源：`cli-status`（starting / stdout / closed）· `system`（`init` / `api_retry` / `context_compacted` / `budget_extended` / `task_started|progress|done`）· `stream_event`（`content_block_start` 的 thinking / tool_use / text、`message_stop`）· `control_request(can_use_tool)` · `control_cancel_request` · `assistant(tool_use)` · `user(tool_result)` · `result`（success⇒完成 / 非 success⇒出错）· 回合开始（`startAgentChat`）· 用户停止（`stopAIChat`）。
  - **涉及文件**：`app/src/main.ts`（`setDebugPhase` / `renderDebugPhaseSeg` / 各事件打点）、`app/src/index.html`（开关行）、`app/src/styles.css`（`#debug-phase-bar`）、`app/src/i18n.ts`（`chat.more_debug_phase` + `debugphase.*`，5 语言）。
- **未做 / 取不到的阶段（如实记，不假装有）**：
  - **后台复盘**：agent **刻意不上报**该阶段（`core-agent/src/main.rs` 的 `ForkSpec.emit_progress=false`，注释写明「发进度会让状态栏停在错的文案上」；且它在提问之间单独跑）⇒ 现有事件里**取不到**，**按纪律省略**。若日后必须显示，需 agent 侧新增一条**默认关**的事件（同 `LUNAC_DUMP_API` 纪律：默认关、关闭零开销、不动常驻 stdio 协议）。
  - **已裁定（2026-10-11，三条）**：① **「后台复盘」阶段保持省略**（它在提问之间跑，展示出来反而易误读）。② **调试开关保持在聊天窗「更多设置 ⋯」菜单**（不移到设置页、也不藏成纯 localStorage）。③ **已清掉死配置**：`main.ts` 里 `agentAutoFold()` 读的 `lunac-agent-autofold` 全仓只读无写（设置里已无对应开关）⇒ 删除该函数与那次读取，折叠门槛**保持原来的默认行为**（工具调用 ≥2 或过程块 ≥3）。
  - **`tools.rs` 无可复用的「当前在执行哪个工具」全局态** —— 工具名本就随 `stream_event` / `assistant` 的 `tool_use` **逐次**到达前端（`main.ts` 的 `cb.name` / `block.name`），**无需 agent 改动**。
  - **不新增第二套状态显示**：浮标与常显状态行（`statusText`）**由同一批事件驱动**，故不会「打架」；与 `.sys-note` 职责不重叠（`.sys-note` 是**对话流内**的历史记录、会随对话滚走；浮标是**当前态**的独立浮标、且默认关）。
- **验收（脚本侧，已跑）**：`npx tsc --noEmit` + `npm run build`（cwd `app`）exit 0；`npm run verify`（仓库根）exit 0。**未改 `core-agent` ⇒ 未跑 `agent-e2e.ps1`**（该脚本无宿主前端，本就测不到 UI）。
- **真机验收口径（待做）**：
  1. 起 dev 实例开聊天独立窗 → 「更多设置 ⋯」**最底部**出现「阶段状态栏（调试）」一行 + 开/关两档；**默认关**。
  2. **关闭态零副作用**：devtools 里 `document.getElementById("debug-phase-bar")` 应为 `null`。
  3. 打到「开」：右上（标题栏下方）出现浮标。发一句提问，浮标应依次走过 `请求模型 → 思考中/生成回复 → 执行工具：<工具名> → 请求模型 … → 完成`；出现审批卡时显「等待审批」，压缩时显「压缩上下文」，子代理/技能 fork 时显「子代理运行中」，agent 崩溃/异常收尾显「出错」。
  4. **不干扰**：点浮标区域应**穿透**（`pointer-events:none`）；不挡输入框 / 结果区交互、不抢焦点、窗口尺寸不变。
  5. **四类颜色**：把四类全设 `#000000`，浮标底色/描边/文字应随之变黑（走四类变量，无白名单语义色）。
  6. 关掉再重开聊天窗，开关状态保持（`localStorage`）。
- **其余关联文件**：`app/src/main.ts`、`core-agent/src/tools.rs`（经查**无需改动**）。

### §待办 3-5 L7 AI 聊天独立界面：实机回归
- **已查清**：AI 聊天已搬进独立无边框窗（`pluginId="chat"`，1280×720、最小 720×420）；主窗两态（360 内嵌 / 600 detached）已去掉；VSCode 按钮进独立窗标题栏。
- **待办点**：真机回归——搜索命中 AI 开独立窗且主窗无空面板；窗内聊天 / 流式 / 审批卡 / 历史 / 仪表盘；`×` 关窗后能再从搜索开；**同时开音乐窗与聊天窗互不串扰**（`emit` 是广播，最该盯）。已知代价：聊天窗会多跑一次与全局状态无关的初始化。
- **涉及文件**：`app/src-tauri/src/plugin_window.rs`、`app/src/main.ts`。

### §待办 3-6 L11 本地「只参与决策」模型：结论存档
- **已查清**（结论，落点已实现为 headless 插件 `local-model`）：本地决策模型已是独立品类（TypeSafe Jev + firelex Jeff、Ollama `/v1/systemone` 的 `nimble`/`tev1`、Cactus Needle 2、distil labs 的 `defer_to_larger_model`）。**可靠线**：7B 以下通用模型会吐畸形工具调用，要 7B 以下必须选**专训**的（decision model / Needle）。
- **待办点**：agent 侧「重要节点审查 / 策展」功能本身**尚未实现**（阶段 0/1 都没做）；本地引擎已换 **Qwen3-4B-Instruct-2507-Q4_K_M**，需先实测 CPU 单次判定延迟再定节点频率。
- **本轮进展（2026-10-15）：判定输出加 GBNF 约束（S1 的真正前置）**。选型 = **请求体 `grammar` 字段**（GBNF 文本），实测 `llama-server` build **b11524**（win-cpu-x64）**支持且生效**；`response_format={"type":"json_schema",…}` **也支持**（回 `{"send":false}`）但因输出更长（16 token）未选；**未用启动参数** `--grammar-file` / `--grammar` ⇒ **插件与 `build-plugins.ps1` 均未改、插件版本不动**。落点 = `core-agent/src/decide.rs` 的 `VERDICT_GRAMMAR` 常量 + 判定请求体的 `"grammar"` 字段（内联字符串、零新依赖）。
  - **约束确被服务端应用（非静默忽略）的对照证据**：给「你是诗人，只能写诗、绝不给单个词」的系统提示，**不带**约束时回整段诗（`finish_reason:"length"`、出满 max_tokens）；**带上**约束后被**强制**回枚举（`SKIP`）。
  - **实测数字**（纯 CPU，`-c 8192 -np 1`，3 类输入 = 真需求 / 闲聊 / 明确触发词 × 各 20+ 次）：
    - **合法枚举率：改造前 75/75、改造后 75/75** —— ⚠️ 本机基线**未复现**任务描述的「对真需求开始作答（出满 8 token 散文）」漂移，模型稳定输出 `SEND`（1 token）；约束的**咬合力**以对照实验为准（见上）。
    - **延迟：p50 158→146ms、p95 187→165ms**（约束下仍 1 token 枚举，无「作答 8 token」的尾部）。
  - **本轮进展（2026-10-15，S1 首个切片已接线）**：`decide.rs` 的语义由**输入门**整体换成 agent 侧的**只读预筛**（`RUN`/`SKIP`：这一轮值不值得派一次后台复盘 fork），模块级 `#![allow(dead_code)]` 已移除，头注释里的「调用点 = L10 桌宠常驻输入」已删（用户已裁定落 core-agent 侧）。**消费点只接一处** = `main.rs` 主循环「是否派后台复盘 fork」：`SKIP` ⇒ 该轮不派（省一次云调用），但仍**吃掉本 scope 会话配额**（幂等，跳过不会变成「下次还能再派」）；`RUN` / 任何故障 ⇒ 照旧派。**摊销口径** = **每 K 轮**（`LUNAC_LOCAL_DECIDE_K`，默认 5）+ **压缩事件**（四个触发点各只读判定并记账，**首切片只记账、绝不改压缩行为**）。**开关** `LUNAC_LOCAL_DECIDE` **默认关**；未配 `LUNAC_LOCAL_MODEL_URL` ⇒ 整层透明关闭。**决策账本**独立落盘 `<exe 根>\temp\decisions\<scope>.jsonl`（不进 `chat.db`，见 forbidden §禁用 61），落盘前过 `mask_secrets`、只存 `log::hash64` 指纹、写失败只 warn。**fail-open**：超时 / 端点不可用 / 模型没起 / 解析失败 ⇒ 退化为「不决策」= 走现状路径 ⇒ 默认关时与不接线**逐字节一致**。**五类已全部接线（2026-10-24）** —— 复盘预筛 / 记忆候选抽取 / 检索查询改写 / 安全预筛已全部实现，能力路由以「只记账、无消费者」口径落地（见下方「本轮进展」）。
  - **本轮进展（2026-10-15，S1 冷启动预热已接线）**：真机暴露 —— `llama-server` 冷启后**第一次**判定耗时 **>1500ms**（撞 `LOCAL_DECIDE_TIMEOUT`）⇒ 每次 agent 重启后首次预筛静默 fail-open（行为安全，但等于没生效）；热机后 ~394ms 正常。**加预热**：`decide.rs` 新增 `warmup_with` / `warmup_config`，`main.rs` 在进主循环前 `thread::spawn`（`spawn_local_decide_warmup`）对**本地端点**发**一次**极短请求（同一 `LocalHttp` 通道、同样带 GBNF、`max_tokens` 极小）**丢弃结果** —— 把模型加载 / 首次 KV / 连接吃进缓存。硬约束：**绝不阻塞启动 / 主循环**（后台线程）、**只在开关开 + 端点已配**时预热（默认关 ⇒ 零预热零请求）、**只打本地端点**、**不进账本**、**不动固定前缀**、失败静默（只 `info`）。⚠️ 预热用**专用更宽上限** `WARMUP_TIMEOUT=30s`（判定的 1500ms **一个字不改**）：实测冷启第一次推理本就更慢，沿用 1500ms 会把预热自身提前掐掉、留下未热身的服务端。
- **本轮进展（2026-10-15）：修复「安装版本地模型从未真正可用」（方案 B）**。**根因**：`local-model` 的两个依赖（`bin/llama-server.exe` + `models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf`，实测 **2.33 GiB**）**唯一来源 = 运行期从插件市场安装时按 `dependencies[]` 下载**（`plugin_market.rs:887` → `install_dependencies_with_progress` → `install_archive_dependency`(bin) / `install_file_dependency`(GGUF)）。**安装器不内嵌**（NSIS 无该段，见 `forbidden.md` §禁用 5 第 56 条），设置里的开关也**不下载依赖** —— 安装机上只铺了「不含依赖的 1.4KB 插件包」，依赖下载从未触发；而「已装可用」判据只查「清单可解析 + 入口存在」，于是那句「未安装…请到插件下载」永远不显示，用户一开就撞 `sidecar 可执行文件不存在：…\bin\llama-server.exe`。
- **方案 B 三件事（已实现）**：① **开启前先补依赖** —— 开启开关时若依赖未就位，**先**调**既有** `install_plugin_dependencies`（与 `ocr-engine.ts` 同一条路，**未造第二套下载器**）按清单补齐，带 `plugin-install-progress` 进度 / 可取消（宿主 `cancel_plugin_dependency_install` 置中止标志）/ 失败可重试；② **判据扩展到「依赖就位」** —— `plugin_market::list_installed` 新增 `ready`（`sidecar_command_ready`：声明了 `sidecar` 的插件要求其 `sidecar.command` 文件真的存在；**无 sidecar 的插件恒就绪，不误伤**），前端据此显示「未就绪 / 待下载」而不是放行后报底层错误；③ **失败文案可操作**（人话 + 「重试」入口）。
- **仍**未内嵌依赖、也**不做**开机自动下载（2.33 GiB 体积太大，违背既有口径）⇒ **首次开启需下 ~2.33 GiB**（视网速数分钟到数十分钟，界面已给时间预期提示）。**未动** `build-plugins.ps1`（local-model 的 468MB/0.5B 旧注释口径订正不在此范围）。
- **涉及文件**：`core-agent/src/decide.rs`、`app/src/plugins/builtin/local-model.ts`、`app/src-tauri/src/plugin_sidecar.rs`（本轮**另加**：`app/src-tauri/src/plugin_market.rs`、`app/src-tauri/src/commands.rs`、`app/src-tauri/src/main.rs`、`app/src/plugins/market.ts`、`app/src/plugins/builtin/settings.ts`、`app/src/i18n.ts`）。
- **本轮进展（2026-10-24，S1 第三切片：检索查询改写已接线）**：新增 `core-agent/src/query_rewrite.rs`（专属提示词 + **GBNF** 锁死 `{"terms":[...]}` + 本地后端 + 通道锁 + fail-open），**消费点 = `MemorySearch`**（唯一的检索工具）。语义定为「**只做增量召回**」：原查询先检索、**命中不足 `limit` 时**才问本地模型要扩展词；扩展命中**只追加**、同 scope 同文本去重、**原命中一条不减少且顺序不变**（`main.rs` 的 `merge_expansion_hits`）⇒ 本地模型改写得再离谱，最坏也只是「没补到东西」。**开关**复用 `LUNAC_LOCAL_DECIDE`（默认关）、端点复用 `LUNAC_LOCAL_MODEL_URL`；默认关 / 未配端点 ⇒ **零请求、零账本行**。**账本**新增 `kind=query-rewrite`（`verdict=ok|empty|fallback`、`candidate_count` = 扩展词条数、`candidates` = 每词的 hash + 长度，**绝不落查询 / 扩展词原文**；`turn=0` 因为 `MemorySearch` 是回合中间的工具调用）。**通道**：与判定 / 抽取 / 预热**共用** `decide::try_acquire_local_channel()`，忙 ⇒ **立即 busy-skip、不排队**。**摊销**：本切片不叠「每 K 轮」—— 调用频次由模型是否调用 `MemorySearch` 决定，天然低频。**明确不做**：网页摘要、多查询改写、回答生成、改任何注入。
  - **单测（`query_rewrite` 9 条 + `main` 6 条，另 1 条 `#[ignore]` 真机）**：fail-open / 空查询 / schema 四类非法 / 归一（剔空 · 大小写去重 · 剔除原查询已有的词 · 截断 · 5 条封顶）/ grammar 钉住 `terms` / 超时须宽于判定 / 通道忙立即返回 / 端点不可达 fail-open / **原命中不丢不乱序** / limit 封顶 / 账本恰好一行且无原文 / 故障也记账 / 记账失败不影响返回 / 默认关整层不介入。`cargo test --bins` **343 passed / 0 failed / 8 ignored**。
  - **真机验收口径（尚未执行，按用户裁定验收类暂缓）**：`LUNAC_LOCAL_DECIDE=1` + `LUNAC_LOCAL_MODEL_URL=<本地端点>` ⇒ 让模型调一次 `MemorySearch`（原查询故意命中不足 `limit`）：日志应出现 `检索查询改写：N 条扩展词`、账本 `temp\decisions\global.jsonl` 多一行 `kind=query-rewrite`，结果里原命中在前、扩展命中在后。关掉开关 ⇒ 日志与账本都没有该行，检索结果与改造前逐字节一致。

- **本轮进展（2026-10-24，S1 第四切片：安全预筛已接线，只记账）**：新增 `core-agent/src/safety_prescreen.rs`（提示词 + **GBNF** 锁死 `LOW`/`MEDIUM`/`HIGH` 三档 + 本地后端 + 共用通道锁 + fail-open），**消费点 = `run_tool` 的入口**（所有工具都过这里）。**用户 2026-10-24 裁定 = 只记账**：**不改审批 / 不放行 / 不拦截 / 不改工具结果** —— 审批的唯一决策点仍在前端 `classifyRequest()`，`CMD_BLACKLIST` / 静态安全分析命中时任何档位照旧强制人工确认（`forbidden.md` §禁用 1 第 2/4 条）。**判据**复用 `needs_approval()`（需人工确认的那批：写类四件 + 外部出口 + `Remember` / `PetGrowth` / `Agent` / 全部 MCP；**排除 `AskUserQuestion`** 这个纯交互 UI）。**采样** = 每 K 次需审批的工具调用判一次（K 复用 `decide::amortize_k()`，默认 5）—— 本切片是**采样留痕**、不是全覆盖审计。**不阻塞**：生产入口 `maybe_safety_prescreen` 一律 `thread::spawn`（fire-and-forget），**绝不在工具路径上等待**；为此 `decisions::append` 加了**进程级写锁**（后台线程与主循环并发写时，行内容 + 换行不交错）。**账本**新增 `kind=safety-prescreen`：`verdict=low|medium|high|unknown`、新增字段 `tool`（只记工具名，`skip_serializing_if` 保证其余 kind 逐字节不变）、`turn=0`；**命令 / 文件内容只在当次判定输入里用（已过 `mask_secrets` + 400 字截断），绝不落盘**。**fail-open** ⇒ `unknown` + `fallback=true`，**绝不当成 `low`**（否则统计会系统性低估风险）。
  - **单测（`safety_prescreen` 8 条 + `main` 3 条 + `decisions` 2 条，另 1 条 `#[ignore]` 真机）**：fail-open 记 unknown / 空输入 / 三档原样传回 / 解析（JSON + 裸词 + 脏输出必须 Err）/ grammar 三档齐全 / 通道忙立即 busy-skip / 端点不可达 fail-open / 输入带工具名且脱敏 / 账本一行且带 `tool`、命令原文不落盘 / 故障记 unknown / 老账本行仍可读 / **并发 100 条写入全部可解析**。`cargo test --bins` **355 passed / 0 failed / 9 ignored**。
  - **真机验收口径（尚未执行，按用户裁定验收类暂缓）**：`LUNAC_LOCAL_DECIDE=1` + `LUNAC_LOCAL_MODEL_URL=<本地端点>` ⇒ 让模型跑一次写类 / 命令类工具：日志应出现 `安全预筛：high`（或 low/medium），账本 `temp\decisions\global.jsonl` 多一行 `kind=safety-prescreen` + `"tool":"Cmd"`；**审批卡行为与关掉开关时完全一致**（该弹的仍弹、自动档该放的仍放）。关掉开关 ⇒ 无日志、无账本行、无后台线程。

- **本轮进展（2026-10-24，S1 第五切片：能力路由已接线，只记账 / 无消费者）**：新增 `core-agent/src/capability_route.rs`（提示词 + **GBNF** 锁死 `CHAT` / `WORK` / `DELEGATE` 三档 + 本地后端 + 共用通道锁 + fail-open）。**如实记：本切片没有任何下游消费者** —— 现有架构里「改工具数组」= 改请求固定前缀（违反 `code-standards.md` §规范 6 的缓存不变量），「往消息里塞路由提示」= 动主请求前缀（违反 S1「不动主请求前缀」硬约束）⇒ 本轮只把判定落进决策账本，作为「决策保真度」实验（§4.1.6 阶段 1）的**评测数据**；**开关开与关，可见行为逐字节一致**。真要消费，接线点必须从「回合收尾」移到「回合开始」，且需先重新裁定上面两条约束。**时机** = 回合收尾（用户已在看回复）、**同步执行不 spawn**；**与记忆抽取错开一轮**（`prescreen_due(question_seq + 1, k)`）⇒ 不在同一轮连续打两次本地端点。**输入** = 用户提问头部 `ROUTE_INPUT_CHARS=800` 字（只用于当次判定，账本只落 hash）。**账本** `kind=capability-route`：`verdict=chat|work|delegate|unknown`、`turn` 记真实提问号、`tool=None`。**fail-open** ⇒ `unknown` + `fallback=true`（语义 = 按现状，模型自己决定）。
  - **单测（`capability_route` 8 条 + `main` 2 条，另 1 条 `#[ignore]` 真机）**：fail-open 记 unknown / 空输入 / 三档原样传回 / 解析（JSON + 裸词 + 脏输出必须 Err）/ grammar 三档齐全 / 通道忙立即 busy-skip / 端点不可达 fail-open / 账本一行且提问原文不落盘 / 超长提问按 800 字截断后再算哈希。`cargo test --bins` **364 passed / 0 failed / 10 ignored**。
  - **真机验收口径（尚未执行，按用户裁定验收类暂缓）**：`LUNAC_LOCAL_DECIDE=1` + `LUNAC_LOCAL_MODEL_URL=<本地端点>` ⇒ 连问几轮，日志按 K 出现 `能力路由：work`（或 chat / delegate）、账本多 `kind=capability-route` 行；**模型请求、工具选择、回复内容全部不受影响**。关掉开关 ⇒ 无日志、无账本行、无本地请求。

#### §待办 3-6 补 llama.cpp 升级到 b11803 + 冷 prefill 超时校准（2026-10-24）

- **变更**：`local-model` 的 `llama-server` 从 **b11524**（0.6.0-dev, commit 86a283532）升到 **b11803**（0.7.0-dev, commit a37cc4610）。
- **校验（不是「拉下来就用」）**：zip `llama-b11803-bin-win-cpu-x64.zip` 体积 **24,069,022 B**、**sha256 `68A11F1F…6D27954` 与 GitHub release API 的 `assets[].digest` 逐字一致**（下载走 `gh.nxnow.top` 镜像；直连 GitHub 仍不可用）。GGUF（2.33 GiB，`3605803B…`）**未变、未重下**。
- **部署**：替换 dev 布局 `app\src-tauri\target\debug\Modules\local-model\bin\`（52 个文件，旧 bin 备份在 `temp\llama-b11524-bin`）；`--version` 回 `0.7.0-dev (build 11803)`。
- **兼容性实测**：GBNF 在新 build 上**仍生效** —— 带 `grammar` 的判定请求回 `RUN`、`finish_reason=stop`（若约束被忽略会出散文 / `length`）。
- **升级暴露的真实缺陷（与版本无关）**：CPU 上 4B Q4 的**冷 prefill 实测 ~103 tok/s**（187 token 的 system ≈ **1.81s**），**b11524 与 b11803 完全一致** ⇒ 不是新 build 的性能回归。但 `WARMUP_TIMEOUT` 只预热**判定**那一份 system，其余四类切片（抽取 / 改写 / 安全预筛 / 能力路由）的 system 各自不同、**首次都是冷 prefill** ⇒ 在 1500ms 上限下**必然 fail-open**（`cargo test --ignored` 复现：decide 通过、其余三类 `fail-open-backend-error`）。
  - **修复**：`LOCAL_DECIDE_TIMEOUT` **1500ms → 3000ms**（单一真相源）；`LOCAL_REWRITE_TIMEOUT` 不再自成一套，直接取 `decide::LOCAL_DECIDE_TIMEOUT`。热机后同一 system 命中前缀缓存，实测仍在 **~150ms**（b11524：run1 1912ms / run2 134ms）。
  - **未做**：没有把预热扩到五类 system —— `-np 1` 单槽的 KV 缓存会被后一个 system 覆盖，预热多个等于只热最后一个；真正有效的仍是「放宽冷启动那一次的上限」。
- **验证**：`cargo test --bins` **364 passed / 0 failed / 10 ignored**；5 条 `#[ignore]` 真机测试在 b11803 上**全过**（判定 / 改写 / 安全预筛 / 能力路由 / 记忆抽取，`--test-threads=1` 串行 + `LUNAC_LOCAL_MODEL_URL`）。
- **分发链同步**：`scripts/build-plugins.ps1` 的 URL 与 sha256 已换；`local-model` **0.11.0 → 0.11.1** 并重打包（`-Plugin local-model`）；dev 已安装清单同步为 0.11.1。⚠️ 本轮编辑 `.ps1` 时**丢过一次 BOM**（PowerShell 5 把中文按 ANSI 读 ⇒ 解析报错），已恢复并 `npm run verify` **8/8、0 提醒**。

#### §待办 3-6 补 决策保真度实验阶段 1：A vs B（2026-10-24 已跑）

- **目的**（archive `agent-feature-backlog.md` §4.1.6 阶段 1）：只跑 **A（体积驱动压缩）vs B（append-only 大上下文）**，先回答一个问题 —— **「决策保真度」指标能不能区分两种策略**？不能区分就先修指标，不急着做 C 臂。
- **口径（如实记，含两处偏离设计的取舍）**：
  - **A 臂 = `LUNAC_MAX_CONTEXT_TOKENS=16000`**（≈ 默认窗口的 1/8），**不是生产默认 128k**；理由是让压缩在可负担的会话长度内**必然触发**（实测固定前缀约 **7.5k token**，在 16k 预算里占 47%）。**B 臂 = 默认 128k**（12 次里 0 次压缩）。⇒ 本实验是**压缩放大**口径，**不得**外推成生产环境的压缩频率。
  - 任务 = **T2**（早期约束：规则藏在 90 行文件的第 45 行，被 elide 即丢失）+ **T5**（早期大工具结果回读：码藏在 90 行 ledger 的第 45 行）；**这两个各跑 3 次/臂**。其余四个（T1/T3/T4/T6）用 v2 夹具补跑 **1 次/臂**（夹具需要「串行读 + 文件数加倍」才够越过水位，第一版 5 个文件时 A 臂根本没压缩 —— 于是测到的是「无压缩 vs 无压缩」）。
  - 每臂每任务 **3 次**；`deepseek-flash`；`--permission-mode plan`；固定系统提示词 / 工具池（33）/ 工作区；驱动 = `scripts\agent-e2e.ps1`（release 产物）。
  - 任务 prompt 显式写「每个文件只读一次、不许用其他工具、不许重读」⇒ **「重读自救」计入约束违反**（否则 A 臂会被模型自己重新检索掩盖，第一版夹具就撞过这个坑）。
- **结果（每格 = 3 次里答对几次）**：

| 任务 | A 臂（16k，压缩） | B 臂（128k，不压缩） | 压缩事件（A / B） |
|---|---|---|---|
| **T2** 早期约束 | **0 / 3**（3 次都如实报告「规则被 elide」，0 次编造） | **3 / 3** | 3/3 · 0/3 |
| **T5** 早期大结果回读 | **1 / 3**（2 次丢失；其中 1 次**先编造 `ORBITAL-FERRET-9` 后撤回**） | **3 / 3** | 3/3 · 0/3 |

- **补充覆盖（v2 夹具，T1/T3/T4/T6 各 1 次/臂；判据见下）**：

| 任务 | 关键信息位置 | A 臂 | B 臂 | A 臂是否真压缩 |
|---|---|---|---|---|
| **T1** 跨文件修改（不得改 c） | 用户 prompt（首条 head） | 通过（a/b=DONE、c 未动） | 通过 | **否**（14.4k，未到水位） |
| **T3** 库约束（禁用 axios） | `a.ts` 第 2 行注释 | 通过（AXIOS=forbidden、FETCH_COUNT=30） | 通过 | 是（27.4k） |
| **T4** 未决问题回收（Q7） | `notes/1.md` 末尾 | **失败**（明确报告 1.md/2.md 被 elide） | 通过（原文答出） | 是（19.8k） |
| **T6** 目标漂移（守住 spec） | `spec.md` 首段 | 通过（锚定 parse 流式化） | 通过 | 是（13.6k） |

  - 补充覆盖合计 `in` 123,317 / `out` 4,575 token（8 次）。**阶段 1 全部 20 次合计**：`in` ≈ 312k、`out` ≈ 11k。
  - **为什么不区分（按任务形态）**：T1 的约束在**首条用户文本**里（`compact_history` 对 head 永不丢）且未触发压缩；T3 的答案（禁用 axios）**靠常识也能对**，计数也不依赖那条注释；T6 的 spec 目标在压缩后仍能靠前段/摘要守住。⇒ **只有「关键信息藏在会被 elide 的早期大工具结果里、且最终问题必须回读它」的任务（T2 / T4 / T5）才灵敏**。

- **成本（T2/T5 共 12 次合计）**：`in` ≈ **189k**（189,316）、`out` ≈ **6.4k**（6,408）、`cache_read` ≈ **712k**（712,059）token。⚠️ 反直觉但可解释：**A 臂总 `in` 高于 B 臂**（T2 59.7k vs 40.3k；T5 58.2k vs 31.2k）—— 压缩后模型多轮往返（A 7 turns，B 2–6 turns），**每轮都重发 7.5k 固定前缀**；在「固定前缀占预算 47%」的极端比例下，压缩省下的体积抵不过多出来的往返。**不得**据此断言「压缩不省 token」。
- **结论（阶段 1 的问题有答案）**：
  1. **指标能区分 —— 但只在特定任务形态下**（T2 0/3 vs 3/3、T4 0/1 vs 1/1、T5 1/3 vs 3/3；T3/T6/T1 两边都过）⇒ 指标本身可用、**不必先修指标**；但任务套件必须按「**关键信息藏在会被 elide 的早期大工具结果里、最终问题必须回读它**」这条口径筛，否则测到的是「无压缩 vs 无压缩」或「常识也能答对」。
  2. 判定口径要按 **「最终答案 + 中途是否编造」两条一起看** —— T5 有一次「最终答错但中途编造过」、一次「最终答对但自己承认前面是猜的」，只看最终文本会把后者记成成功。
  3. 任务 prompt 必须**禁止重读**，否则模型会重新检索自救，把 A 臂的缺陷掩盖掉（第一版夹具的 T2 就是这样「答对了」的）。
- **证据**：`temp\fidelity\runs\{t2-a,t2-b,t5-a,t5-b}\<时间戳>\summary.json`（每 run 的 `results[0].final_text` / `tools` / `usage` / `system_events`，压缩与否看 `system_events` 是否含 `context_compacted`）；v2 补充在 `temp\fidelity\runs\{t1-a,t1-b,t3-a2,t3-b2,t4-a2,t4-b2,t6-a2,t6-b2}\`；汇总 `temp\fidelity\results.json`（T2/T5）与 `results-2.json`（v2）；夹具 `temp\fidelity\t{1,2,3,4,5,6}\workspace` + `t{1,2,3,4,5,6}.query.txt`。**全部在 `temp\`（中间产物，不入库）**；需要留存时再决定是否移进仓库。

#### §待办 3-6 补 阶段 2：C 臂「决策驱动瘦身」MVP（2026-10-24 已实现）

- **做了什么**：新增 `core-agent/src/curation.rs`；消费点 = `compact_history` 的 **`Compact::Elide` 分支**，且只在**成本模型已判定「值得瘦身」之后**插一道本地判定 —— 把候选工具结果（头/中/尾三段采样、单条 400 字、最多 4 条）发给本地模型，只让它在 `{"drop":[编号...]}` 里挑「确认可以丢」的。**模型说留的，从瘦身名单里拿掉** ⇒ C 臂相对 A 臂**只会更保守**。
- **边界（硬）**：
  - **只作用于 `Elide`（0.85 水位）**；`ElideCold` / `Drop` / `Force` **完全不经过**本模块 —— 那三档是「不压就可能撞 400」的安全刚需，绝不能被模型否决。
  - **fail-open = 照旧按体积压**（与其余切片的「fail-open = 不做事」相反）：这里「不压」才是更危险的一侧，可能直接撞上限。所以后端故障 / 超时 / 通道忙 / 解析失败 ⇒ 原样使用体积驱动的结果。
  - **模型合法回空列表 ≠ 故障**：`drop: []` 表示「一条都不该丢」⇒ 瘦身名单清空、这一轮不瘦身（C 臂因此可能更费 token，这是实验要测的 trade-off，不是 bug）。
  - **只判前 4 条**（按成本模型给出的顺序）；未送判的尾部照旧按体积驱动处理 —— 不引入「后面一律保留」这种无法解释的规则。
  - **送判文本必须头/中/尾三段采样**：只截头部会漏掉藏在文件中部的约束（T2 夹具就是「规则在第 45 行」），那样 C 臂会把它判成 filler 直接丢掉、白做。
- **开关**：`LUNAC_LOCAL_CURATE`（**默认关**），**不复用** `LUNAC_LOCAL_DECIDE` —— 那个开关下全是「只记账 / 不改行为」的切片，本模块**会改变压缩选择**，必须能单独关掉（实验时也要能只开 C 臂）。端点复用 `LUNAC_LOCAL_MODEL_URL`；未配 ⇒ 整层透明关闭。
- **单测（`curation` 10 条 + `main` 5 条）**：fail-open 原样 / 空输入不算故障 / 模型回空 = 全保留 / schema・越界・非整数・非 JSON 全 Err / 去重排序 / 名单过滤保留未送判尾部 / 「全可丢」不改名单 / 「全不能丢」清空名单 / grammar 锁 1–4 / **过滤后 net 变负 ⇒ 一条都不瘦** / 通道忙立即跳过 / 端点不可达 fail-open / 头中尾采样。`cargo test --bins` **379 passed / 0 failed / 11 ignored**；C 臂 `#[ignore]` 端到端在 b11803 上通过。
- **真机验证与两处调参（2026-10-24，都有日志依据）**：
  1. **首跑静默失效**：8 条 × 600 字 ≈ 1200 token，CPU 冷 prefill **~11.6s**，撞当时复用的 3s 上限 ⇒ 日志 `决策策展后端故障 … error sending request` ⇒ fail-open（C 臂等于没做）。**改为 4 条 × 400 字（≈400 token，冷 prefill ~4s）+ 独立 10s 上限**（不再复用判定的 3s —— 输入大一个量级，套用必然超时）。
  2. **过滤后没重跑成本判据**：首跑成功那次日志写 `瘦身决策 … net=-194800` 却照样压了 4 条 —— C 臂把「省下的体积」改小了，必须**重算 `sigma`/`net`，`net <= 0` 就一条都不瘦**。已补上并加单测。
  - **最终实测**（夹具 `t2b`：10 个文件、规则藏第 47 行、`budget=30000`）：日志 `决策策展：送判 4 条，模型认为可丢 3 条` → `瘦身名单 5 → 0 条` → `跳过瘦身 … net=-194650`（保住了含规则的 00 文件）；下一次压缩 `名单 6 → 5 条、net=309300` 正常动手。最终答对 `INDENT=4 SPACES`（A 臂在 30k 预算同口径下答不出）。
- **未做**：C 臂只覆盖 **elide 选择**，没有改 `Drop` 层（「append-only 全量」那一半仍未实现）。

#### §待办 3-6 补 阶段 2 三臂对照：A vs B vs C（2026-10-24 已跑）

- **三臂口径**：A = 体积驱动压缩（`LUNAC_MAX_CONTEXT_TOKENS` = **30000**（t2b）/ **32000**（t5b），约为 7.9k 固定前缀的 4 倍）；B = 大上下文不压（**120000**，实测 0 次压缩）；C = **与 A 完全同预算** + `LUNAC_LOCAL_CURATE=1` + `LUNAC_LOCAL_MODEL_URL=http://127.0.0.1:8090`（Qwen3-4B-Q4_K_M / llama.cpp b11803）。每格 **3 次**，`deepseek-flash`、`--permission-mode plan`、同一工作区与工具池（33 个）。
- **夹具**：`t2b`（10 个文件，约束 `PROJECT_RULE_ALPHA: Indentation must be 4 spaces` 在 `notes/00-notes.md` **第 47 行**）与 `t5b`（`data/ledger.txt` 共 7,860 字符，`code word: ORBITAL-FERRET-9` 在**第 47 行** / 字符偏移 **3,884**）。两者都是「串行读 10 个文件、禁止换工具、禁止重读」，先读的 6 个结果正好是压缩候选 ⇒ 关键信息**必然**落在会被 elide 的块里。
- **结果（每格 = 3 次里答对几次）**：

| 任务（夹具） | A 臂（压缩） | B 臂（120k 不压） | C 臂（A + 本地策展） |
|---|---|---|---|
| **T2** 早期约束（`t2b`，budget 30k） | **0 / 3** | **3 / 3** | **3 / 3** |
| **T5** 早期大结果回读（`t5b`，budget 32k） | **0 / 3** | **3 / 3** | **0 / 3** |

  - **判定口径（与阶段 1 一致，必须按此读表）**：「答对」= 最终答案给出具体值**且未自我标注为猜测**。A 臂 `t2b` run1 写 `INDENT=4 SPACES — but flag it … this number is a guess` ⇒ 记**失败**；A 臂 `t5b` run1 先给出 `ORBITAL-FERRET-9`、随后自己撤回「那是猜的」⇒ 记**失败**（它在前几轮确实读到过，但压缩后已无法自证，最终选择不采信）。
- **成本（18 次合计 `in` 313,491 / `out` 12,513 / `cache_read` 3,283,319）**：

| 夹具 · 臂 | `in` | `out` | `cache_read` | 单次耗时 |
|---|---|---|---|---|
| `t2b` · A | 59,230 | 2,608 | 512,128 | 10.5 / 11.0 / 11.9 s |
| `t2b` · B | 35,048 | 1,344 | 600,573 | 8.0 / 8.6 / 9.0 s |
| `t2b` · C | 49,896 | 2,085 | 553,466 | 16.7 / 12.6 / 12.2 s |
| `t5b` · A | 60,303 | 2,210 | 521,472 | 12.1 / 10.6 / 12.0 s |
| `t5b` · B | 48,671 | 1,601 | 572,800 | 9.8 / 8.7 / 9.6 s |
| `t5b` · C | 60,343 | 2,665 | 522,880 | 17.2 / 12.9 / 12.3 s |

  ⇒ **C 的云侧成本 ≈ A**（`t2b` **0.84×**，因为第一次压缩被否决 ⇒ 少一次历史改写；`t5b` **1.00×**）；**额外代价全在本地**：每次真压动多一次本地判定，实测 **~7s**（07:38:38 → 07:38:45，在 10s 上限内），端到端比 A 多 2–5s。
- **C 臂机制证据（日志逐字）**：
  - `t2b` 三次完全一致：`决策策展：送判 4 条，模型认为可丢 3 条` → `瘦身名单 5 → 0 条` → `跳过瘦身 … net=-194650`（含规则的那条被保住）→ 第二次压缩 `6 → 5 条`、`net=309300` 正常动手 ⇒ 最终答对。
  - `t5b` 三次也完全一致：`决策策展：送判 4 条，模型认为可丢 4 条` → 名单原样 6 条、`sigma=52673`（**与 A 臂逐字同值**）⇒ ledger 照旧被压掉。
- **⚠️ 负结果与归因（必须如实记）**：**C 臂在 T5 上完全无效（0/3），失效方式与 A 臂毫无区别**。
  - **不是采样漏掉**：`Read` 实际送判文本 `n = 8,128`（含行号前缀），`sample_item` 中段窗口 = 中点（4,064）前后各 66 字 ⇒ 覆盖字符 **3,998–4,131**，而码在 **≈4,013** ⇒ **码确实在送判文本里**；送判按历史顺序取最老的 4 个，ledger 是第 1 条 ⇒ 它确实被判过。
  - **是判定错，但主因不在「同质流水」本身**：90 行 audit trail 把中间那行 `code word: …` 淹掉了一层，**还有一层更关键** —— `Read` 返回带 `"{行号}\t"` 前缀，4B 把它当机器噪声。离线探针（t5b 真实样本、`n=8,128`）：**带前缀**判 `drop:[1,2,3,4]`，**剥掉前缀后同一内容**判 `drop:[]`。根因与修法见下一条「修正批」。
  - **有效边界**：修正前 C 臂只在「**约束是一句自然语言、出现在文件正文中间**」（`t2b`）时有效；「**目标信息是一行孤立数据、与上下文同质**」（`t5b`）时无效。
- **结论（按 archive §4.1.6 的判据）**：判据 =「C 相对 A 约束遵守率 **+≥15pp** 且成本 **≤ A×1.2**」。T2 满足（**+100pp**、成本 0.84×）；T5 **不满足**（+0pp、成本 1.00×）⇒ **不能宣布 C 臂成功**，只能宣布「机制在 T2 形态上成立」。
- **下一步（本批未做 → 已由「修正批」完成）**：改 `CURATE_SYSTEM` 措辞 + 定位 T5 的根因；`Drop` 层（append-only 全量）仍未动。
- **证据**：`temp\fidelity\runs\{t2b,t5b}-{a,b,c}-r\<时间戳>\summary.json`（`results[0].final_text` / `usage` / `system_events`）；本地策展日志在**工作区内**同一相对路径 `temp\fidelity\t{2,5}b\workspace\temp\fidelity\runs\<同名 run>\logs\agent-2026-10-24.log`（看 `决策策展` / `瘦身决策` / `跳过瘦身` 三行）；夹具 `temp\fidelity\t2b\workspace` / `t5b\workspace` + `t2b.query.txt` / `t5b.query.txt`。**全在 `temp\`（中间产物，不入库）**。

#### §待办 3-6 补 阶段 2 三臂对照·修正批：C 臂 0/3 的根因已定位并修掉（2026-10-24）

- **根因（两级，逐级实测，不是猜）**：
  1. **提示词没点名「同质流水里的单行事实」**：旧 system 只说 keep「a fact the final answer may need to quote」⇒ 4B 把 90 行 audit trail 整块判成噪声。
  2. **主因 = `Read` 的行号前缀**（上一批漏掉的一层）：`Read` 返回 `"{行号}\t{行}"`，**带前缀时**同一段流水判 `drop:[1,2,3,4]`，**剥掉前缀后同一内容**判 `drop:[]`（离线探针，t5b 真实样本，`n=8,128`）。行号是本项目展示层加的，不属于文件内容。
- **修法（两处，都在单一落点）**：`curation::strip_line_numbers()` + `sample_item()` 内先剥行号（所有调用方自动受益，不存在「谁忘了剥」的第二种写法）；`CURATE_SYSTEM` 增两句 —— **逐条独立判**，且「单行具名事实（口令 / token / ID / 编号 / 路径 / 阈值 / 规则）也必须整条保留」。
- **修正批结果（同口径、各 3 次，只重跑 C 臂）**：

| 任务 | C 臂（修正后） | 关键日志 |
|---|---|---|
| T2（`t2b`） | **3 / 3** | `送判 4 条，模型认为可丢 0 条` → `名单 5 → 0 条` → `跳过瘦身` |
| T5（`t5b`） | **3 / 3**（原 **0/3**） | 同上（`0 条` ⇒ 名单 `6 → 0` ⇒ 不压） |

- **⚠️ 必须同时记的代价（不粉饰）**：修正后本地 4B **在这两个夹具上一条都不批准丢** ⇒ C 臂退化成「不压缩」，行为与 B 臂等价。**保真度 3/3 是靠「不做任何瘦身」换来的，不是靠「聪明地保留关键块」** ⇒ 指标过了、**压缩收益为零**。
  - 这不是 bug，是当前提示词的**过度保守**：离线探针里「1 条关键 + 3 条明显噪声」会正确回 `drop:[2,3,4]`，但「4 条都像文件正文」时回 `drop:[]`。真实流量（大量文件正文）下 C 会频繁整段不压。
  - 成本：`t5b`-C 三次 `in` = 20,626 / 21,972 / 22,154（≈ B 臂 21k 量级；A 臂 60k 是压缩后多轮改写推高的）；`t2b`-C 三次 `in` = 2,154 / 2,154 / 22,588。本地每次真压动仍多 **~7s** 判定。
- **单测**：`cargo test --bins` **381 passed / 0 failed / 13 ignored**。新增 `line_numbers_are_stripped_before_judging`、`sampling_reaches_the_code_word_through_read_line_numbers`，以及两条 `#[ignore]` 本地探针（单条流水 / 四条同质，**均带 `Read` 行号前缀**，与真机同形）。
- **下一步（未做）**：① 让 C 恢复「敢丢纯噪声」—— 再调 `CURATE_SYSTEM`（现在过度保守），并补一个「纯噪声块」夹具证明它还能丢；② 在此之前 `LUNAC_LOCAL_CURATE` **保持默认关**；③ `Drop` 层（append-only 全量）仍未动。
- **证据**：`temp\fidelity\runs\{t5b,t2b}-c4-r\<时间戳>\summary.json` + 工作区内 `…\logs\agent-2026-10-24.log`（`决策策展`/`瘦身决策`/`跳过瘦身` 三行）；根因对照 = 离线探针直接调 `curate()`（带 / 不带行号各一次）。

#### §待办 3-6 补 阶段 2 确定性护栏：压缩安全不再依赖本地模型（2026-10-24）

- **做了什么**：新增 `curation::protected_reason()`（纯函数）与 `main::guard_elision_plan()`，**在 C 臂之前**执行：命中即把整条候选从瘦身名单里拿掉，**不送本地模型**。只作用于 `Compact::Elide`；`ElideCold` / `Drop` / `Force` 仍是「不压就可能撞 400」的安全刚需，**一条不改**。护栏无开关（恒定生效），与 `LUNAC_LOCAL_CURATE` 无关。
- **命中规则（两条，窄口径）**：
  1. **`named-fact-line`**：一行 `标签: 值` / `标签=值`，标签 ≤32 字且（a）属于白名单 `code word / secret / token / password / rule / constraint / requirement / instruction`，或（b）**全大写**（≥2 个大写、无小写，如 `PROJECT_RULE_ALPHA`）。
  2. **`secret-marker`**：行内含 `code word` / `codeword`（自然语言形态）。
  - 显式排除日志级别与文档提示词（`INFO/WARN/ERROR/DEBUG/NOTE/TODO/FIXME/EXAMPLE/USAGE…`），否则任何带 `NOTE:` 的正文都会被整条保护、瘦身直接失效。
  - 剥 `Read` 行号前缀复用同一实现（`strip_line_numbers`），所以**带不带行号都能命中**。
- **真机验证（A 臂，`LUNAC_LOCAL_CURATE` 关，同口径各 3 次）**：

| 任务 | 修正前 A 臂 | 加护栏后 A 臂 | 护栏日志 |
|---|---|---|---|
| T2（`t2b`，budget 30k） | 0 / 3 | **3 / 3** | `保护 1 条（named-fact-line），瘦身名单 5 → 0 条` → `跳过瘦身 net=-194800`（第二次 `6 → 5 条`、`net=309150` 正常动手） |
| T5（`t5b`，budget 32k） | 0 / 3 | **3 / 3** | `保护 1 条（secret-marker），瘦身名单 6 → 5 条` → `瘦身决策 sigma=44545 net=228150` **照样压 5 条** |

  ⇒ 关键差别：**护栏只拦下那一条，其余照压**（t5b 仍省 44,545 字），不像 C 臂修正后「一条都不压」。成本：`t2b`-A 三次 `in` 29,633 / 29,746 / 29,555，`t5b`-A 三次 28,949 / 27,617 / 11,291。
- **单测**：`cargo test --bins` **387 passed / 0 failed / 13 ignored**（+6：护栏两条真机形态、普通日志不误伤、小写关键标签、护栏重算 `sigma`、净亏清空、未命中逐字段原样）。
- **已知盲区（如实记，未做语义判断）**：只保护「有标签」的行。像 `Indentation must be 4 spaces` 这种**没有 `标签:` 前缀的叙述型约束**不会被保护；也没有实现「上一轮引用过的文件不瘦」。两者都留作后续（前者要靠关键词/句法，后者要在 `guard_elision_plan` 里回看最近消息里的路径）。
- **与 C 臂的关系**：护栏先跑、C 臂只看剩余候选 ⇒ 即使将来 C 臂恢复「敢丢」，事实行仍有确定性兜底。⇒ **`LUNAC_LOCAL_CURATE` 可以继续默认关**，压缩安全性已经不再依赖那个 4B。

#### §待办 3-6 补 本地决策优化清单（2026-10-24 登记；逐项实现）

> 依据：三臂对照 + 修正批 + 确定性护栏三轮实测（见上三条）。**P0 已落地**，其余按序实现。

| # | 项 | 为什么现在做 | 判据（可证伪） |
|---|---|---|---|
| **P0** | 确定性护栏（具名事实 / 口令行） | ✅ **已完成**：A 臂 T2/T5 由 0/3 → 3/3，且仍压 5 条 | 见上一条「确定性护栏」 |
| **P1** | 护栏盲区 A：**无标签的叙述型约束**（如独立一行 `Indentation must be 4 spaces`） | ✅ **已完成** | `narrative-constraint`：≤3 行、≤200 字、非注释/非代码符号；多了就按「到处都有 must」不保护 |
| **P2** | 护栏盲区 B：**最近一轮引用过的文件不瘦**（用户/模型刚提过的路径） | ✅ **已完成** | `recently-referenced`：末 8 条消息的正文/路径命中候选文件名即保护；⚠️ 曾因把整段 JSON 当文本而让单字符路径假命中，已改为只取正文 |
| **P3** | C 臂恢复「敢丢纯噪声」+ 纯噪声夹具 | ✅ **已完成** | 提示词补「只有计数器变化 = filler，举例」；纯噪声探针 `drop:[1,2,3,4]`，两条含事实的探针仍保留第 1 条 |
| **P4** | `query_rewrite` 端到端 A/B 评估 | ✅ **已完成（并修掉一处真短板）** | 见下方「P4 结果」：改前 on/off 都 **0 命中**，改后 on **3/3 命中** |
| **P5** | GPU offload（可选 CUDA 形态，CPU 保底） | ✅ **已完成（改用 Vulkan，默认分发已切换）** | GPU 1,658.8 tok/s / CPU 兜底 98.2 tok/s / GBNF 生效 / 全部 15 条 `#[ignore]` 通过 |
| **P6** | `Drop` 层「append-only 全量」 | ✅ **已完成**（小补丁 + 全量形态，全量**默认关**） | 小补丁见下；全量 = `LUNAC_COMPACT_APPEND_ONLY=1` 时 0.95 水位只瘦不丢，折算后仍超预算才回落 `Drop` |

**P3 真机回归（护栏 + C 臂一起，各 3 次）**：`t5b` **3/3**、`t2b` **3/3**。日志链条（t5b 三次一致）：
`确定性护栏：保护 1 条（secret-marker），瘦身名单 6 → 5 条` → `决策策展：送判 4 条，模型认为可丢 4 条` → `瘦身决策 sigma=44545 net=228000 —— 动手`。
⇒ 护栏保住事实行、C 臂继续丢 filler，**压缩收益不再是零**（对比修正批的 `drop:[]`）。

**P4 结果（`t7` 夹具：记忆 8,431 字、事实在 6,000 字快照切点之后；查询与条目无字面重合）**：
- 关改写：`MemorySearch 命中 0 条（reason=no-match）` **3/3**；但最终文本仍 2/3 答 `INDENT=4` —— **是猜的**，所以判据必须看 `命中` 行，不能看最终答案。
- 开改写（改前）：`4 条扩展词` ⇒ 仍 **0 命中**（2 次；另 1 次撞「本地通道忙」被跳过）。
- **根因**：检索是**纯子串匹配**，而 4B 输出的是「代码规范 / 风格要求」这类带修饰的短语，与条目里的「排版规范」互不为子串 ⇒ 一条也召不回。
- **修法**：① 提示词明确「中文 1–4 字短词、英文单词，不要带修饰的短语」；② 新增**确定性** `add_cjk_bigrams()`：给 CJK 长词补滑窗 2 字片段（已在原查询里的跳过，总数上限 8）。
- **改后**：`8 条扩展词` ⇒ `MemorySearch 命中 1 条（reason=ok）` **3/3**；探针 `local_rewrite_bridges_a_query_with_no_literal_overlap` 通过。
- ✅ 抢槽那半条已在同日晚些时候修掉（`-np 1` → `-np 2`，见下方「P5b 多槽」）。

**P5 实测结果（2026-10-24，结论：走 Vulkan，不走 CUDA）**：
- **CUDA 路线放弃**：引擎包 `llama-b11803-bin-win-cuda-12.4-x64.zip`（259.6 MiB）能起服务，但同目录缺 `cudart64_12.dll` / `cublas64_12.dll`（补包 373.3 MB，镜像只有 0.12–0.22 MB/s，两次都在 13–16 MB 处断开）；且实测 **98.5 tok/s ≈ CPU 95.3** ⇒ 不作为分发形态。
- **Vulkan 路线成立**：`llama-b11803-bin-win-vulkan-x64.zip` **36.9 MB**（比 CPU 包只大 12.8 MB），`--version` = `0.7.0-dev (build 11803, commit a37cc4610)`，与 CPU 包同源。
  - 同一 981 token 冷 prefill：**GPU（RTX 4060 Laptop，`-ngl 99`）= 1,658.8 tok/s / 590.8 ms**；CPU 包 = **95.3 tok/s / 10,241 ms** ⇒ **17.4×**；生成 47.0 vs 15.4 tok/s（3.1×）。
  - **GBNF 仍生效**：`grammar` 锁 `RUN|SKIP` 回 `SKIP`。
  - **CPU 兜底已实测**：把 `ggml-vulkan.dll` 改名（模拟没有 Vulkan 的机器）⇒ 同一份包用 CPU 跑 **98.2 tok/s**，与 CPU 包持平。⚠️ **不能写 `--device Vulkan0`**：没有 Vulkan 设备时它会直接报 `invalid device` 并退出；只写 `-ngl 99`，让它自己挑设备。
- **分发已切换（2026-10-24）**：`scripts/build-plugins.ps1` 的 `local-model` → **0.12.0**（同日晚些时候又升 **0.13.0**，只因 `-np 2`，见 P5b），依赖换成 Vulkan 包，`sidecar.args` 增 `-ngl 99`；sha256 与 GitHub release 页 `expanded_assets` 清单**逐字一致**（该 tag 的 REST API 只列 cudart，`expanded_assets` 才带主包 digest）。
- **可复现的坑（必须记）**：Vulkan 下**模型加载后的第一次请求**包含 shader 编译，实测超过 `LOCAL_DECIDE_TIMEOUT`(3s) ⇒ 单跑 `cargo test --ignored` 的 `decide` 那条会**假红**（`fail-open-backend-error`），**重跑即过**（0.25s）。生产侧由 `WARMUP_TIMEOUT`(30s) 的启动预热覆盖，不需要改超时；但**测试里首次冷跑失败不要当成回归**。
- **超时未重标**：GPU 下 3s / 10s 有巨大余量，但 CPU-only 用户仍需要 ⇒ 保持现值，不做「按后端自适应」。

**P5b 多槽（2026-10-24，`-np 1` → `-np 2`；解决 `MemorySearch` 与预热 / 判定抢槽）**：
- **现象 / 根因**：agent 侧所有本地调用共用**一把二值锁**（`decide::try_acquire_local_channel`），而服务端只有 1 个解码槽 —— 两者都假设「同时只能有一个请求」。于是回合早期的改写撞上预热 / 判定时只能 `busy-skip`（t7 实测 **1/3 次**）。这不是超时值的问题，是**并发度**的问题。
- **改法（两处，缺一不可）**：
  1. **服务端**：`scripts/build-plugins.ps1` 的 `local-model` → **0.13.0**，`sidecar.args` 的 `-np 1` 改 **`-np 2`**。
  2. **客户端**：`decide.rs` 把二值锁换成**按槽计数**（`LOCAL_CHANNEL_IN_FLIGHT` + `channel_slots()`）；槽数由宿主从**插件清单的 `-np`** 读出来、经 `LUNAC_LOCAL_MODEL_SLOTS` 注入（`commands.rs` 的 `local_slots()` / `arg_value()`）。
     - 与 `--port` 同一条纪律：**清单是唯一真相源**，宿主不另写常量（§禁用 62）。
     - 上限 `MAX_LOCAL_SLOTS = 4` **只在 agent 侧**卡（宿主原样透传），避免同一阈值两处实现。
     - **默认 1 槽**：老插件 / 未配 / 非法值 / 超上限 ⇒ 与改造前逐字节一致；宿主也只在 `>1` 时注入变量。
- **实测（本机 RTX 4060 Laptop + Vulkan，`-c 8192 -np 2`）**：
  - 启动日志 `n_slots = 2, n_ctx_slot = 4096` ⇒ `-c` 被两槽均分（不是每槽 8192）。
  - 进程工作集 **2.6 GB**（`-np 1` 时约 1.4 GB），仍远低于 sidecar 的 8 GiB 上限。
  - **真并行**：单个 200-token 请求 **4.3s**；两个并发 **4.8s**（若排队应 ≈ 8.6s）。
  - 每槽 4096 token 对现有五类调用足够（最大的送判提示词 = 4 条 × 400 字 + 短 system）。
- **单测**：agent 侧 `channel_allows_two_in_flight_when_the_endpoint_has_two_slots`、`channel_slots_fall_back_to_one_on_bad_input`（+ 既有两条显式钉成 1 槽，去掉执行顺序依赖）；宿主侧 `slots_come_from_the_manifest_args` / `slots_fall_back_to_one` / `parallel_spellings_are_accepted` / `host_does_not_clamp_the_upper_bound`。
- **验证**：`cargo test --bins`（core-agent）**399 passed / 0 failed / 15 ignored**；`cargo test --bin lunac`（宿主）**326 passed / 0 failed / 2 ignored**；`npm run verify` 8/8；插件已重打包 `release\plugin-packages\local-model-0.13.0.zip`，dev `Modules\local-model` 同步为 `-np 2`。
- ⚠️ **未做**：真机「busy-skip 率」复测（要起 dev 实例 + 真实回合，属验收类，用户已裁定暂缓）；**发布**到插件市场（`publish-plugins.ps1` 需人工执行）。

**P6 结果（2026-10-24，**已实现「让路式护栏」小补丁**）**：
- **做了什么**：`Compact::Drop` 在 `layer_aware_drop_cut` 之后再过一道 `guarded_drop_cut()`（`core-agent/src/main.rs`）——
  把丢弃边界收缩到**第一条受保护消息之前**（受保护 = `curation::protected_reason` 命中：`secret-marker` /
  `named-fact-line` / `narrative-constraint`，与 Elide 档同一套判据，不新增第二套口径）。
  - **为什么是「收缩边界」而不是「跳过中间几条」**：`drop` 的对象是**一段连续区间** `[head, cut)`，
    `dropped_msgs` / 摘要 / 回滚锚点三处语义都建立在这个前提上（`layer_aware_drop_cut` 约束②）；
    非连续抽取属于「append-only 全量」那一档，不在本例。
  - **让路规则**：只有受保护消息就在可丢区间开头（收缩后一条也丢不掉）时才放弃收缩 ——
    丢零条只会白废前缀缓存。此后按**今天的口径**丢干净 ⇒ **最坏情况与不接线逐字节一致**。
  - **不碰的三处**：`CompactOutcome.dropped` 语义、回滚锚点 `base`、`Force` 档（预算 / 400 兜底
    仍无条件压，被丢的事实照样进摘要 / 结论索引）。
  - **日志**：`Drop 档护栏：保护 N 条（reason），丢弃边界 X → Y（A 条 → B 条）` /
    `Drop 档护栏被迫让路：…仍按原边界丢 N 条`。
- **单测（4 条，`main.rs` tests）**：`drop_guard_keeps_protected_fact_line`（事实行保留 + 配对 `tool_use`
  一起留）、`drop_guard_keeps_bare_narrative_constraint`（无标签叙述型约束）、
  `drop_guard_yields_when_the_wall_is_at_head`（让路 = 原口径）、`drop_guard_is_a_noop_on_ordinary_noise`（无命中零变化）。
- **验证**：`cargo test --bins` **397 passed / 0 failed / 15 ignored**（本补丁落地时；同日晚 P5b 又加了两条 `decide` 单测 ⇒ 现为 **399**）；仓库根 `npm run verify` 通过（8/8）。
**P6 全量形态（2026-10-24 同日晚，**已实现 · 默认关**）**：
- **开关**：`LUNAC_COMPACT_APPEND_ONLY`（`1/true/on/yes`；**默认关** ⇒ 该档不可达、行为与改造前逐字节一致）。
  关着时 0.95 水位仍走 `Compact::Drop`，开时才走新档 `Compact::AppendOnly`。
- **语义（`core-agent/src/main.rs`）**：
  1. **瘦身范围 = 全量**：`elide_keep` 从 8 降到 **2**（与 `Force` 同），即**连尾部一起瘦**；
     尾部块若不瘦，append-only 就没有腾空间的手段。
  2. **不看成本模型**（这一档是「尽量不丢消息」，压得越多越好），但**仍过确定性护栏**；
     护栏分两档（`GuardMode`）：`Elide` 保护「事实行 + 最近引用过的文件」，
     `AppendOnly` **只保护事实行**。理由：尾部块的文件路径必然出现在紧邻的 `tool_use` 里，
     若照样保护则尾部永远瘦不掉、开关等于白开；而文件**重读一次就能拿回**，事实行丢了找不回。
  3. **回落判据** `append_only_must_drop()`：`measured_tokens − elided_chars / 4 ≥ 预算` ⇒ 必须丢。
     折算用 **4 字符 ≈ 1 token**（刻意低估省下的量：只会更早回落，不会「以为够、结果撞 400」）；
     `measured_tokens == 0`（体积未知）⇒ 直接回落。回落走的是**与 `Drop` 完全相同**的一套
     （层感知 + 让路式护栏），不是第三套丢弃策略。
  4. **不动的三处**：`CompactOutcome.dropped` 语义、回滚锚点 `base`、`Force` 档。
     回落时 `elided + dropped` 同一次调用内完成 ⇒ `context_compacted` 事件**只发一次**。
  5. **日志**一行：`append-only：瘦身 N 块（省 C 字 ≈ T token），折算后 ~X / 预算 ⇒ 不丢消息 / 回落 Drop 丢 M 条`。
- **单测（5 条，`main.rs` tests）**：`append_only_keeps_every_message_when_elision_is_enough`（条数不变、
  不丢消息）、`append_only_falls_back_to_drop_when_the_estimate_still_overflows`（折算仍超 ⇒ 回落丢）、
  `append_only_elides_inside_the_normal_keep_tail`（与 `Elide` 对照：同一条尾部大块，`Elide` 不碰、
  append-only 瘦掉）、`append_only_still_respects_the_fact_guard`（事实行不换掉，且**不因 `net ≤ 0` 清空名单**）、
  `append_only_must_drop_is_conservative`（体积未知必回落 / 4 字≈1 token 的边界）。
- **验证**：`cargo test --bins` **404 passed / 0 failed / 15 ignored**；仓库根 `npm run verify` 通过。
- ⚠️ **仍未做（如实记）**：① 生产 128k 下「Drop 真实触发频率」仍无实测数据 ⇒ **不建议默认开**；
  ② 未做「elide 块折叠成指针 + 结论索引」之外的额外折叠（现有 `conclusions::extract_from_elided`
  已写指针条目，重注入仍只在真丢消息时发生）；③ 无真机夹具（默认关 + 单测覆盖，真机验收待排期）。

#### §待办 3-6 补 子代理角色 / 预算口径 / 本地探活 + 开关迁移（2026-10-24，已实现）

> 依据：`docs\cache-hit-experiment.md` A/B/C 数据复盘 —— 平台用量 CSV 只有密钥维度（TEST A/B/C
> 合计 4 次请求），A/B/C 对话实际走了软件自身密钥；C 组 `LUNAC_LOCAL_DECIDE` 未设、本地模型
> 加载后零推理。**命中率结论作废**，改为先修下面三项可观测问题（不依赖重跑实验）。

- **P0-1 子代理角色**：`Agent.input_schema` 新增可选 `role`（`general|explore|coding|review|research`），
  新增 `core-agent/src/subagent_role.rs` 作为「工具白名单 + 角色提示词 + 短名」的**单一真相源**；
  `subagent_role_defs()` 在 `subagent_tool_defs()`（安全底线）之后按角色收窄；`general`/缺省/未知值
  **逐字节保持旧行为**。角色短名进 `task_started.role` 与启动日志。单测：`subagent_role::tests::*`
  3 条 + `subagent_role_defs_narrow_tool_pool_per_role`。
- **P0-2 预算按价计 + 收口**：`SUBAGENT_BUDGET_TOKENS` 改为**加权单位**
  （`in×1 + out×4 + cache_read÷50 + cache_create×1.25`，与 `pricing.json` 同源）；
  80% 注入收口提示（与轮数倒数第二取先到者）；撞预算不再直接交 134–345 字残片 ——
  给未执行的 `tool_use` 补 `tool_result` 后进入**纯报告轮**（请求不带 tools），`partial` 仍为真。
  单测：`subagent_budget_weights_cache_read_and_output`、`budget_wrap_fires_at_eighty_percent`。
- **P0-3 本地模型真实探活**：宿主新增 `local_model_probe` 命令与 `probe_local_model_startup()`
  （`llama-server` 起后 30s 内重试 `GET /health`，成功落 `本地模型就绪：…`）；前端把
  「已加载 / 已就绪 / 失败原因」显示出来。`plugin_sidecar::start` 对 `local-model` 起独立探活线程。
- **开关迁移**：本地模型决策开关从「设置 → AI」迁到输入栏「更多设置 → 其他」
  （`app/src/local-model.ts`；`settings.ts` 不再渲染 / 绑定该块），原依赖下载与确认警告一并保留。
- **验证**：core-agent `cargo test --bins` **410 passed / 0 failed / 15 ignored**；
  宿主 `cargo check --bins` 通过；`app` 侧 `npx tsc --noEmit` 与 `npm test`（12 passed）通过。
- ⚠️ **未做**：真机点一遍新面板（需 dev 实例）与「角色 + 加权预算」的端到端对比实验；
  `LOCAL_MODEL_DECIDE` 的 C 组前置门禁仍写在实验文档，未做成自动脚本。

#### §待办 3-6 补 本地判定 / 抽取「撞车」修法 + 记忆读写路径查证（2026-10-16，本轮）

- **修 1（本地模型通道进程级非阻塞互斥）**。**根因（已查实）**：`llama-server` 以 `-np 1`（**单并发槽**）运行；开 `LUNAC_LOCAL_DECIDE=1` 时**同一个回合**里「预筛（`LOCAL_DECIDE_TIMEOUT=1500ms`）」与「记忆候选抽取（`LOCAL_EXTRACT_TIMEOUT=5000ms`）」会**几乎同时**发往同一本地端点 ⇒ **互相排队** ⇒ 两条都等到各自超时 ⇒ 双双 fail-open（真机 `app/src-tauri/target/debug/temp/logs/agent-2026-10-16.log`：抽取 `latency 5009ms`、判定 `1515ms` 都在上限处被掐断，而**单独**那次预筛仅 `1133ms`）。**修法（最小改动，首选方案）** = 判定 / 抽取 / **预热** 三者**共用一把进程级非阻塞锁**（`core-agent/src/decide.rs` 的 `try_acquire_local_channel` + `ChannelGated`，`memory_candidates.rs` 的 `ChannelGated`）—— **拿不到锁就立即跳过本次**（判定 / 抽取折成 `fallback=true` + `reason="busy-skip"`、记一条账本行；预热只记一行 info），**绝不排队等待**（这些是「摊销的尽力而为」调用，跳过无损，排队只会把两个请求都拖死）。**未动**：超时值、`-np`、「每轮多请求」（其中 `-np` 已于 2026-10-24 改为 **2**，见 P5b；超时值与「每轮多请求」仍未动）；**默认关 ⇒ 逐字节不变**（开关关 / 未配端点时根本不进判断层，零请求零锁）。**单测**：`decide::tests::gated_request_busy_skips_immediately_without_waiting`（持锁时第二个「本地请求」**立即** busy-skip、`< 1s` 不等待）、`channel_lock_is_exclusive_and_non_blocking`、`memory_candidates::tests::gated_extract_busy_skips_without_waiting`。
  - ⚠️ **卡 1 / 卡 2（预筛 × 抽取撞车）的根因 = 上面这条**（`-np 1` 单并发槽 + 同回合并发的两个本地请求）；**已由本轮修 1 消除**。
  - ⚠️ **卡 8 需按「先开一个非聊天插件窗」重验**（本轮**未**验完；起 dev 后先开一个非聊天插件窗再看该现象）。
- **修 5 查证结论：记忆「写了读不到」= 非 bug**。**读**（core-agent 注入用的 `memory_block`）：`fetch_memory` → 桥 `lunac/memory_read`（`core-agent/src/main.rs:2531-2533`）→ 宿主 `mcp_server::read_memory` → `memory_path_for(Global)`（`app/src-tauri/src/mcp_server.rs:549-553`）= `<exe 根>\ModuleData\memory\MEMORY.md`。**写**（`Remember`）：桥 `lunac/memory_write` → `write_memory_entry`（`mcp_server.rs:726-728`）→ **同一个** `memory_path_for` = **同一文件**。⇒ **读写同源、单一真相源，不是代码缺陷**（`mcp_server::tests::memory_scopes_are_isolated` 已钉住 `memory/MEMORY.md` 与 `memory/pet/MEMORY.md`）。观察到的 `ModuleData\MEMORY.md`（**root**，非 `memory\` 子目录）**无任何代码路径会写**（`git grep` + 全历史核对：自 A4 长期记忆引入起一直是 `memory\MEMORY.md`，从未写过 root；`module_data_dir()` 也从未与 `MEMORY_FILE` 直接拼接）；其 mtime `09:26:29Z`（= 最后一次 `Remember` 写入时刻 `agent-2026-10-16.log:168-170`）**早于**空目录 `ModuleData\memory\` 的 mtime `09:53:26Z` ⇒ 形态正是「文件被**移出** `memory\`」（外部移动保留文件 mtime、更新来源目录 mtime）。⇒ **非 bug**，root 那份是**孤儿文件**；**未改任何代码、未新增第二条路径推导**（`/memory` 与设置页读的仍是 `memory\MEMORY.md`）。

### §待办 3-7 M1 两个待实测项
| # | 项 | 待办点 |
|---|---|---|
| **M1-1** | 前端 CDN 与 WebView2 冷启动在开机场景的真实占比 | 现象侧已定论（开机自启慢根因 = OS 触发时机 + 计划任务权限）；只差实测确认「CDN / 冷启动无关」后写死结论 |
| **M1-2** | 缓存命中率遗留疑点：复现 `in=2287 / read=0` 那一轮 | 「落盘慢」已证伪；剩余两种可能：① 前缀本身变了（技能清单 / 工具黑名单 / cwd）；② 供应商侧缓存被清。工具已就绪（`result.usage.requests[]` + 启动时落盘的 `固定前缀 …` 行前后对照） |

### §待办 3-8 其他已查清、待决的点
- **长期记忆触发门槛**：**已并入 §待办 1-6 并已实现（2026-10-11）** —— 根因 = 复盘门槛计数（原 `LUNAC_NUDGE_INTERVAL` 默认 10 次用户提问）是 **agent 进程内变量、频繁重启归零**；现改为「按会话 + 按时间」触发并修掉空记忆冷启动死锁，详见 §待办 1-6，此处不再单列。
- **`chat.db` 历史遗留大库**：那份 298 MB 的库已于 2026-10-02 去重重写（→ 0.81 MB），备份留在 `ModuleData\history\chat.db.bak-2026-10-02`（确认无误后可删）。

### §待办 3-9 记忆读侧三件套：快照兜底 + 只读检索工具 + 压缩边界刷新（2026-10-15 登记；**A 不动 / B 已验收 / C 2026-10-23 已实现**）

- **现状定性（一句话）**：**读 = prompt**（`ModuleData\memory\MEMORY.md` / 桌宠私有一份，一行一条 bullet；**启动读一次的冻结快照**注在 `history` 开头（2026-10-23 起：压缩边界会重读一次并原地替换，见 C），**无读工具**、**整段注入、零检索召回**）／**写 = loop**（`Remember` 工具 + 每会话每 scope 至多一次的后台复盘 fork + 人工 `/memory clear`），合起来是**混合**形态。⚠️ **`docs/` 与 archive 从未使用「prompt 架构 / loop 架构」这两个词** —— 这是我方**按定义**给的分类（读侧靠 prompt 注入 = prompt 架构；写侧靠 agent 循环里调工具 = loop 架构），**不是文档既有术语**，采信前先认这个定义。
- **待解决的墙**：`MEMORY.md` **整文件 6000 字符封顶**（`mcp_server.rs` 硬闸）⇒ 写满后**唯一出口是 `replace:true` 整体重写**（`docs/archive/ai-spec.md` §11 规则 56）⇒ **被迫丢信息**（想加新条就得先删旧条）；且**会话中新写的记忆要重启 agent 才可见**（冻结快照，规则 56 明写「会话中新写的记忆只落盘」；**2026-10-23 由 C 在压缩边界修掉**）。
- **三件套（逐项：做什么 / 代价 / 为什么这样切）**：
  - **A 保留快照兜底**（不动）：冻结快照**每轮一定在场**（注在 `history` 开头，规则 56）。**做什么**：什么都不做，仅作为 B / C 的兜底。**代价**：零（现状即它）。**为什么这样切**：稳定、每轮必达、不依赖模型「想起去查」；B 再强也不能假设模型一定会调工具。
  - **B 加 `MemorySearch`（只读工具）**：**已实现（日期 2026-10-17）**；**端到端已验证（2026-10-22，`agent-e2e.ps1` + 宿主桥 + 真实模型）**。新增一个**只读、免审批、可被 `--disallowedTools` 裁**的检索工具（按查询召回记忆条目），**只能模型主动调、绝不自动注入**。**代价**：多一个工具进工具表（固定前缀变一次，需重启 agent 才生效）；每次调用多一次往返；检索质量是**新增问题**。**为什么这样切**：解决**扩展性**（6000 字符封顶的根因）；**范式先例** = `SessionSearch` 就是「只能模型主动调、不得自动注入」（`docs/forbidden.md` §禁用 4 第 46 条 / 规则 53）—— B 完全照它走，**不新造范式**。
    - **实现落点**：`core-agent/src/tools.rs` 的 `defs()`（无条件注册，内置工具 24 → 25）与 `BRIDGE_TOOLS`（走桥 ⇒ 子代理剔除、串行）；`core-agent/src/main.rs` 的 `memory_search()`（读取**复用** `fetch_memory` → 既有 `lunac/memory_read` 桥，不新造读取路径）与 `memory_search_log_args()`；检索内核 `core-agent/src/memory_search.rs`（纯函数 `split_bullets` / `search` / `rank_cmp`）；前端黑名单候选 `app/src/main.ts` 的 `TOOL_BLACKLIST_CANDIDATES`。
    - **入参 / 输出**：`query`（必填）、`scope`（`global` / `pet` / `all`，缺省 `all` = 两份都查）、`limit`（默认 10、**上限 30**）；返回 `{"results":[{"scope","text","score":{"matched_terms","query_terms","occurrences"},"why"}],"reason"}`。
    - **检索 / 排序规则**：按空白切词、**任一**命中即候选；中文按**子串**匹配（含整句），英文 / 数字**大小写不敏感**；排序 = 命中词数降序 → 出现次数降序 → 条目字符数升序（跨 scope 复用同一 `rank_cmp`）。**上限 30 的理由**：记忆文件本身 6000 字符封顶、条目数天然有限，30 是防「用户手工把文件改大」时一次检索吃掉上下文的硬闸（与 `SessionSearch` 同口径）。
    - **验收口径如何满足**：①「记忆超过 6000 字符后仍能取回旧条」—— 单测 `memory_search::tests::entry_beyond_the_6000_char_snapshot_is_still_retrievable` 直接构造「快照截断点之后的旧条」并检索取回；②「同一问题在快照缺失时靠检索仍答对」—— **端到端已验证（2026-10-22）**：release 布局 `ModuleData\memory\MEMORY.md` 造 ~7.9 KB（快照 6000 字截断点之后放 `ORBITAL-FERRET-9`），`agent-e2e.ps1 -McpExe <绝对路径 lunac.exe>` + 真实模型 ⇒ 日志 `MemorySearch 命中 1 条（scope=global，reason=ok）`、最终回答 `ORBITAL-FERRET-9`（证据目录 `temp\agent-e2e\20261022-120548`；临时记忆文件已删除）；③「压缩后写的新记忆无需重启即可见」= **C 项，2026-10-23 已实现**（见下）。另：纯函数单测覆盖中文子串 / 英文大小写 / 多关键词任一命中 / 排序三级 / limit / 空结果；**未触碰注入**（`memory_block` / `cap_inject` / 冻结快照注入一行未改，无任何自动调用点）；失败一律回**空结果 + reason**（`bridge-unavailable` / `memory-disabled` / `empty-or-missing` / `no-match` / `invalid-scope` / `missing-query`），不 panic、不抛硬错误。
    - **可观测**：日志一行 `MemorySearch 命中 N 条（scope=…，reason=…，query_len=…，query_hash=…）`；**不记查询全文** —— 查询词可能含用户敏感信息，长度 + FNV 哈希足以归因且不可还原原文。
  - **C 压缩边界刷新快照**：**已实现（2026-10-23）**。**做什么**：在**压缩事件后**重读一次记忆，把快照**原地替换**进 `history` 开头。**落地**：`core-agent/src/main.rs` 新增 `refresh_memory_snapshot()`（重读 + `apply_refreshed_snapshot()` 落盘）——只在 `compact_history` **真压动过**（`elided + dropped > 0`）时调用，覆盖全部 6 个压缩路径（Drop / ElideCold / Elide / 预算 Force / 400 兜底 Force / 手动 `/compact`）；**原地替换**消息条数不变 ⇒ 回滚锚点 `base` 与刷新前的 `dropped` 修正互不干扰；刷新结果**写回主循环跨提问持有的 `ctx_block`**（`run_query` 的参数由 `Option<&Value>` 改为 `&mut Option<Value>`），否则下一轮会重新注入旧块、记忆来回闪。**边界（如实记）**：① 本来就没有块（记忆被关 / 桥不在）⇒ 不刷新、也不凭空造块；② 刷新读到空（记忆被 `/memory clear` 清空 / 桥临时失败）⇒ **保留旧块**；③ 内容与旧块逐字节相同 ⇒ 什么都不做；④ 仍**禁止**每轮重读、禁止 `Remember` 写成功后立即刷新（本项不触碰这条）。**为什么这样切**：**修掉「重启才可见」**（规则 56 的副作用），不是新开通道，也不改动注入位置（仍注在 `history` 开头、不进 `system`）。**代价**：每次真压动多一次本地记忆读（桥调用，非云调用）；会消费掉「压缩后本来就会作废」的那段前缀缓存（无额外损失）。
    - **单测（4 条，`main.rs` tests）**：`snapshot_refresh_replaces_head_block_in_place`（替换后条数不变、快照写回）、`snapshot_refresh_keeps_old_block_when_memory_is_empty`（读空 ⇒ 保留旧块）、`snapshot_refresh_is_a_noop_when_content_is_unchanged`（同内容不制造前缀变更）、`snapshot_refresh_does_nothing_without_an_existing_block`（无块不造块）。`manual_compact` 的返回值同时改为 `(回执, 是否真压动)`，两条既有单测补断言「空历史 / 压不动 ⇒ 不触发刷新」。
    - **验证（2026-10-23）**：`cargo test --bins`（`core-agent`）**313 passed / 0 failed / 5 ignored**；`cargo check --bins` exit 0（仅 2 条既有 `BgCmd` dead-code 警告）；仓库根 `npm run verify` exit 0。
    - **真机验收口径（尚未执行，用户已裁定验收类暂缓）**：起 dev 实例 → 会话中途让模型 `Remember` 一条新事实（或不重启、直接在 `MEMORY.md` 里加一条带 `[YYYY-MM-DD · global]` 的 bullet）→ 触发一次压缩（手动 `/compact` 最直接；也可造大工具结果触发水位压缩）→ **同一进程内**再问一与该事实相关的问题：模型应能答出，且 agent 日志出现 `压缩边界：长期记忆快照已刷新（… → … 字，可变层原地替换）`；若压缩没压动（`elided + dropped = 0`）则**不应**出现该行。
- **代价与冲突（必须写清）**：
  - ⚠️ **规则 56 的口径已在「压缩边界」这一处放宽（2026-10-23，用户拍板做 C）**：archive 的 `ai-spec.md` §11 规则 56 明写「注入必须是冻结快照……**禁止**『写完记忆就刷新提示词』『每轮重读一次记忆文件』」，并解释「会话中新写的记忆只落盘，下次重启才可见」。**这两条禁令仍然有效**（本项不碰它们），新增的**唯一例外**是「`compact_history` 真压动过的那一次可以重读并原地替换」——理由是该轮前缀本来就已经作废。**按 `docs/README.md` §八「新规则一律写进 6 份现行文档、不回写 archive」的纪律，规范落点 = [`code-standards.md`](./code-standards.md) §规范 6**，archive 原文不改。
  - **延迟**：读侧一旦从「整段注入」变成「模型按需调工具」，**每次读到记忆都至少多一次往返**（本地检索是本地调用，但若走云则多一次云往返）。
  - **不可复现 / 归因变难**：模型「这一轮到底查没查记忆、查到了什么」不可预知 ⇒ 同一问题两次可能一次答对一次答错（取决于模型是否调用工具）。
  - **失败模式**：「**该用记忆时模型忘了查**」—— 这是 loop 读的**固有失败模式**；A（快照兜底）在场时危害小，**若为省 token 去掉 A 则显著放大**。
  - **检索质量是新增问题**：现在**完全没有记忆召回**（零检索）⇒ 一旦加 B，「召回得准不准」成为**新的一组待调问题**。⚠️ `conclusions::retrieve` 是**压缩恢复**（把被压掉的结论找回上下文），**不是**记忆召回，**别混**。
- **验收口径（可证伪）**：① **「记忆超过 6000 字符后仍能取回旧条」** —— 造 >6000 字符的记忆（或 `replace` 丢过信息后），用 B 检索应取回**快照里已不在**的旧条；② **「同一问题在快照缺失时靠检索仍答对」** —— 造一个只存在于检索条目、不在快照里的事实，模型应能靠 `MemorySearch` 答对；③ **「压缩后写的新记忆无需重启即可见」** —— 会话中途写一条新记忆 + 触发一次压缩 ⇒ 该条**无需重启 agent** 即应能被模型看到（C）。
- **分期建议**：**A 不动**（兜底，删了会放大「忘了查」）；**B 已实现并端到端验收（2026-10-17 / 2026-10-22）**；**C 已实现（2026-10-23）**（只在压缩边界重读，不改注入位置、不碰「禁止每轮重读」）。
- **依据**：`docs/archive/agent-feature-backlog.md` §4.1.1 / §4.1.7、`docs/archive/ai-spec.md` §11 规则 56 与 §3.5、`docs/forbidden.md` §禁用 45 / §禁用 61、现状代码 `app/src-tauri/src/mcp_server.rs`（6000 上限）/ `core-agent/src/main.rs`（冻结快照注入）。

### §待办 3-10 插件清单「引脚」与「主控芯片 ↔ 插件芯片」单信道方案（2026-10-17 设计修订；2026-10-18 部分施工）

> **修订说明（2026-10-17 第二轮）**：本条目原为「引脚最小施工」调研。用户新要求是——**即使全库没有 dev 环境**，AI 也能通过装插件、并在**用户允许**下装 SDK / 开发环境，进而制造**前端 + 后端 + 架构 + 依赖**；**默认遵守 `ui-style.md`**，用户需要独立前端外观时**可以显式绕开**。Lunac 视为主控芯片，插件系统是插件芯片，两者只经**一条信道**通信；用户自主开发可把插件封装为**独立芯片**交付，过程含**传输、接收、边界隔离**。追加裁定：**可以调用 dev 环境以及 SDK**。**与前一轮结论冲突处，以本段为准。**
>
> **一句话结论**：方案主轴改为**双轨制**——**轨道 L**（两个纯文本文件，无工具链）保留为 AI 自建插件的立身之本；**轨道 H**（用户显式授权的 SDK / 开发工具链）新增，用来产出前端 + 后端 + 架构 + 依赖。前端默认 `inherit` 遵守 `ui-style.md`，声明 `custom` 才绕开；宿主与插件只走 `postMessage` 单信道；iframe 作为 **opt-in**，且**只是 DOM/CSS/脚本作用域边界，不是沙箱**（`forbidden.md` §禁用 1）。
>
> **状态（2026-10-18 收尾）**：E.0 / E.1 / E.2（含两个常驻锚点）/ E.3 / E.4 / E.5 / E.6 已施工，
> `net.fetch` / `media.proxy`（`lunac-media` 协议）/ `storage.plugin`（宿主 KV）/
> `engine.eval`（QuickJS 沙箱）已落地；`provides` 冲突 warn 与解析器已全量接入窗口 /
> sidecar / 依赖安装；工具链授权与每 sidecar 独立 Job Object 限额已接线。
> **剩余 = dev 真机验证**：iframe / 两个常驻锚点 / 工具链构建。**探针插件已提供（2026-10-27）** ——
> `scripts\probe-plugin\`（iframe + `chat-sidebar` + `settings-section`），
> `scripts\install-probe-plugin.ps1` 一键铺进 dev 的 `Modules\probe`；**工具链那一条仍缺一个真实资产**。
>
> **⚠️ 2026-10-27 真机报障（2026-10-29 定位到真根因、已修）**：dev 的 `Modules\probe\` 一度被判成
> **空目录**，表现为 `E_TIMEOUT：iframe 未在 3s 内完成握手 —— 文档没有触发 load
> （lunac-plugin:// 未解析到 ui/index.html）`。**当时的处置（重跑 `install-probe-plugin.ps1`）不对症** ——
> 三件套一直在盘上；真根因是 iframe 的 `src` 写成了 `lunac-plugin://localhost/...`：
> Windows / Android 的 WebView **不能直接加载自定义 scheme**，Tauri 只把
> `http://<scheme>.localhost` 映射进处理器（同 `convertFileSrc` / `http://tauri.localhost`）。
> 未知 scheme 的导航**既不触发 `load` 也不触发 `error`** ⇒ 只能看到那句 3s 超时。
> 修法：`pluginFrameUrl()`（`app/src/plugins/channel/host.ts`）按平台给两种形态，三处 iframe
> （主窗内嵌 / 插件窗 / 两个常驻锚点）统一走它。
> **运维口径**仍有效：`Modules\` 被覆盖后要重铺探针，验收前先确认
> `Modules\probe\{lunac-plugin.json, index.js, ui\index.html}` 三件都在。
>
> **施工进度（2026-10-18）**：
> - ✅ **E.0 测试 runner**：`app` 新增 `vitest` devDependency 与 `test` / `test:watch`；
>   `app/src/plugins/channel/protocol.test.ts` 12 条通过。
> - ✅ **E.1/E.3 清单地基**：`PluginManifest` 新增 `provides` 与 `render.engine/appearance`，
>   校验形状 / 条数 / 枚举；`InstalledPlugin` 下发；`resolve_plugin_dir()`（精确 id 优先、
>   再按 `provides` 取 id 升序第一个）与冲突 warn；前端 `Plugin` / `MarketPluginInfo` 透传；
>   `build-plugins.ps1` 与 `agent-templates/modules/README.md` 同步。
> - ✅ **C.4 `net.fetch`（light-DOM 桥先行）**：新增 `plugin_net.rs`（能力校验、http/https、
>   私网拒绝、超时 / 体积上限、重定向复检）；插件悬浮窗桥升到 `apiVersion 3`，
>   同时提供 `storage` / `media` / `engine` 占位。`resource-hub` 清单已声明 `net.fetch`。
> - ✅ **C.4 补 `media.proxy`（A2，2026-10-18）**：新增 `lunac-media` 自定义协议
>   （`plugin_media.rs`）；`__lunac_host.media.proxy(url, {referer, userAgent})` 返回可给
>   `<img>` / `<video>` 的 URL；转发 `Range`、拒绝私网、重定向逐跳复检、单次 ≤32 MiB。
>   能力门 = 清单 `permissions: ["media.proxy"]`（`resource-hub` 已声明）。
> - ✅ **C.4 补 `storage.plugin`（A3，2026-10-18）**：新增 `plugin_kv.rs`
>   （`<exe 根>\ModuleData\plugin-data\<id>\kv.json`）；按插件 id 隔离、原子写、坏文件隔离；
>   配额 = 键 ≤128 字符 / 单值 ≤256 KiB / 总量 ≤2 MiB / ≤500 键；能力门 =
>   清单 `permissions: ["storage.plugin"]`；`uninstall_plugin` 一并清除数据。
>   `__lunac_host.storage` 已从 localStorage 切到宿主命令，旧数据一次性迁移。
> - ✅ **C.4 补 `engine.eval`（A4，2026-10-18）**：新增 `plugin_engine.rs`（`rquickjs` 0.14）。
>   每次调用新建 QuickJS 运行时，不注入文件 / 网络 / 进程绑定；默认 3s、上限 10s，
>   内存 32 MiB、栈 1 MiB、源码 / 结果各 ≤1 MiB；能力门 =
>   清单 `permissions: ["engine.eval"]`。**仅服务 Windows 插件**；APK / IPA 独立应用
>   各自内嵌引擎，只共享 `EngineApi` 契约。边界 = 脚本执行边界，不是 OS 沙箱。
> - ✅ **E.2 信道 + iframe opt-in（悬浮窗 + 主窗内嵌 + 两个常驻锚点）**：新增 `channel/host.ts` / `channel/client.ts`
>   （握手、mount/execute、请求-响应、超时、`ui.resize` 合并、来源/能力校验）；
>   新增自定义协议 `lunac-plugin://localhost/<id>/ui/index.html`（`plugin_scheme.rs`，
>   只暴露 `ui/` 与 `assets/`，带 iframe CSP）；`plugin-window.ts` 与 `main.ts` 均按
>   `render.engine` 分支；`theme.ts` 汇总 token 快照，`client.ts` 自动写入 documentElement；
>   `tauri.conf.json` 增加 `frame-src`。**待 dev 真机验证。**
> - ✅ **常驻锚点（2026-10-18）**：清单 `render.slots`（`chat-sidebar` / `settings-section`，
>   枚举 + 只允许 iframe 引擎）；`anchor-slots.ts` 统一挂载 / 主题转发 / dispose；
>   聊天侧栏与设置分组都做到「无插件不占位、卸载不残留、面板关闭即摘监听」。
> - ✅ **E.4 工具链授权（轨道 H，2026-10-18）**：清单 `dev` 段（`toolchains[]` / `build`）解析与
>   静态校验（https、sha256 必填、`dest`/`binDir` 必须在 `.toolchain\` 内、`command` 只允许包内
>   路径或构建器白名单）；`plugin_toolchain.rs` 落地授权账本 `config\plugin-toolchain.json`
>   （按 `dev` 段指纹，任一字段变化即失效）、工具链安装、撤销（杀构建 + 可选删文件）、
>   构建执行（独立 Job Object、输出 / 完成事件、**永不自动运行**）；`commands.rs` 6 条命令；
>   设置页「开发工具链」分区（授权 / 撤销 / 安装 / 构建 / 输出）。裸命令只从工具链 `binDir`
>   解析，**不回落全局 PATH**。
> - ✅ **E.6 Job Object 限额（2026-10-18）**：`child_job.rs` 新增 `Job` / `JobLimits`
>   （kill-on-close + 内存 + 进程数）；`plugin_sidecar.rs` 为每个 sidecar 建**独立 job**
>   （失败回落全局 job 并 warn），清单新增 `sidecar.memoryLimitMiB`（0 或 32–32768）与
>   `processLimit`（0–64）。不污染包含 agent 的全局 job。
> - ✅ **真机已验证（2026-10-29）**：iframe 引擎 + 两个常驻锚点（探针插件）——
>   首次跑红是 `iframe.src` 写成 `lunac-plugin://`（WebView2 不认），修为 `pluginFrameUrl()`
>   后按 §测试 21.1 的 ①–⑥ 判据通过；工具链的**拒绝路径 + 授权账本 + 指纹失效 + 撤销**同日通过
>   （§测试 21.2）。
> - ⬜ **仍缺**：工具链**安装 / 构建 / 停止**的端到端——需要一个真实 https 工具链资产
>   （便携 node 即可），换成真 `url` / `sha256` / `dest` 后再点。
>
> - 🧪 **探针插件（2026-10-27 已提供，不入市场 / 不进安装包）**：`scripts\probe-plugin\` =
>   `lunac-plugin.json`（`render.engine = "iframe"`、`appearance = "inherit"`、
>   `slots = ["chat-sidebar", "settings-section"]`、`entry = index.js` 为占位）+ `index.js`（空入口）
>   + `ui/index.html`（**自包含**：内联脚本实现最小信道客户端 —— `hello → ready`、`theme.changed`
>   把 token 写到 documentElement、`mount` 回 `{ok, slot}`、`execute` 计数、`host.resize`、
>   `lifecycle: dispose`，并主动上报 `ui.resize`；每件事都渲染成一行文字，供截图取证）。
>   **装机**：`powershell -ExecutionPolicy Bypass -File scripts\install-probe-plugin.ps1`
>   （默认 `app\src-tauri\target\debug`；`-Root D:\Lunac` 可指安装根）。
>   **为什么不在 `$Plugins` 表里**：它是开发自检件，进市场 / 安装包会让终端用户看到一个没用的插件。
>   **夹具不过时会先红**：`plugin_market::tests::probe_plugin_fixture_matches_the_manifest_rules`
>   钉住 id / 引擎 / 两个槽位 / `ui/index.html` 必须存在。
>   **真机判据**：① 重新扫描后出现「信道探针」；② 打开面板见 `hello` + `→ ready` 两行；
>   ③ 聊天侧栏与设置分组各出一块（`mount：slot=chat-sidebar` / `slot=settings-section`）；
>   ④ 换主题 ⇒ `theme.changed` 且 `--text` 有值；⑤ 关面板 ⇒ `lifecycle：dispose`；
>   ⑥ 卸载 / 重扫后两处锚点不残留。
>
> - 🔎 **握手失败自述（2026-10-27，真机首次暴露「3s 未握手」后改）**：`ChannelHost` 的超时不再是
>   一句空话，而是分三档说清卡在哪一步 —— **文档没触发 load** / **收到消息但 origin 被拒**
>   （并把**实际 origin** 打出来）/ **文档已 load 但零消息**（脚本没跑 / 被 CSP 拦）。
>   同时 origin 白名单**接受 `http://` 与 `https://` 两种 `lunac-plugin.localhost`**（Windows 上由
>   WebView2 决定映射成哪一个）：原先只认 `http://`，映射一变 `postMessage(targetOrigin)` 就会被
>   浏览器**静默丢弃**（不报错、不触发任何事件），表现就是「永远 3s 未握手」。
>   iframe 的 `error` 事件也直接失败（不再干等 3 秒）。

#### A. 双轨制：轨道 L（低门槛）与轨道 H（用户授权的开发环境）

##### A.1 轨道 L：两个纯文本文件即可（保留，不可丢）

- **最小产物** = `Modules\<id>\lunac-plugin.json` + `index.js`：清单文件名 `plugin_market.rs:47`，字段与默认入口 `:186-244`、`:246-248`，入口必须是包内相对 `.js` / `.mjs`（`:636-649`）。
- **不需要** Node、包管理器、构建器、类型包、SDK；AI 直接写两个文本文件即可被扫描、注册、执行（`plugin_market.rs:1517-1585`、`market.ts:155-193`）。
- **这是 AI 自建插件的默认路径**。轨道 H 只能**增量**提供能力，不得把轨道 L 变成「必须先有工具链」。

##### A.2 轨道 H：用户显式授权后装 SDK / 开发工具链

- **能产出什么**：前端（单入口内联，或 `ui/` 多文件后打包）、后端（sidecar 二进制 / 本机服务）、架构（目录与模块拆分、构建脚本）、依赖（`dependencies[]` + 锁文件）。产物仍须落回 `Modules\<id>\`，仍受清单校验。
- **授权模型（设计，待施工）**：
  - **谁能装 = 只有用户**。AI 只能**提议**（写 `dev` 声明 / 触发「申请开发工具链」命令），**不能自批**；批准入口是一张宿主信任卡，与 sidecar 信任卡同构（`plugin_sidecar.rs:18`「模型只能提议，不能自己把门推开」、`:250-263` 的 `start()` 信任门）。
  - **授予单位** = `(plugin_id, profile, version, sha256)` 指纹；任一变化即旧授权失效、重新弹卡（沿用 `fingerprint()` 的思路，`plugin_sidecar.rs:156-164`）。
  - **落点** = **推荐 `<插件目录>\.toolchain\<profile>\`**（比较见 A.3）。
  - **撤销** = 删授权记录 → 杀该插件的构建进程（Job Object）→ 可选删 `.toolchain\`；撤销后未重新授权不得再执行任何构建命令。
  - **审计** = `<exe 根>\logs\` 落盘，记录 grant / revoke / 构建命令（过 `mask_secrets()`）/ 退出码 / 时长；**不记凭据**（`logging.md` 口径）。
  - **单一真相源** = `config\plugin-toolchain.json` 只存**授权记录**（与 `plugin-trusted.json` 同处，`plugin_sidecar.rs:51-52`）；插件能力声明仍只在 `lunac-plugin.json`（`forbidden.md` §禁用 62）。**不得**把工具链能力再抄一份到前端常量表。
- **SDK 的定义**：全仓**未查到**现成的 `@lunac/plugin-sdk` 包。设计口径 = 宿主随包提供**本地类型声明 + 信道客户端胶水**（`app/src/plugins/host.ts` 的 `.d.ts` 镜像 + C 节协议类型），Track H 可复制进插件 `sdk\` 供 TS 编译；**不发布、不新增运行时 npm 包**，因此不破坏「零新增运行时依赖」。
- **安装方式**：优先**单文件二进制 profile**（如独立构建器可执行文件），其次 Node + 包管理器；npm 安装工具链依赖时默认 `--ignore-scripts`，需要生命周期脚本的包必须单独在信任卡上列出并由用户批准【推论：具体哪些包需要脚本，需按 profile 清单实测】。
- **与 `dependencies[]` 三分闸的关系**：
  - **复用**同一套下载 / 校验 / 落盘管线：https only、`safe_join`、`sha256`、单条 4 GiB（`plugin_market.rs:60`）、解压 1 GiB（`:66`）、≤32 条（`:62`）、staging + 原子替换（`:895-916`、`:1009-1058`）。
  - **不新增** `dependencies[].type="toolchain"`，改用独立顶层 `dev` 段。理由：① `dependencies[]` 是**运行时资产**，装插件时自动拉取；工具链是**构建期**能力，必须**逐次授权**，不能被「装个插件」顺带执行；② 混在一起会让「生产运行需要什么」与「开发需要什么」失去边界；③ `npm` 类型已覆盖运行时 JS 包（`plugin_market.rs:259-260`、`commands.rs:2956-2961`），再叠一个会变成第二套包管理器。
  - `dev` 段**不随插件安装自动执行**；`build.command` **永不自动运行**，每次执行都要用户确认（可「本次允许 / 始终允许该指纹」）。
- **与「卸载 = 完全不存在」的关系**：推荐落点在插件目录内 ⇒ 卸载删目录即回收工具链，满足 `forbidden.md` §禁用 51。**本轮不做**宿主共享的可写工具链缓存（它会让卸载后仍有残留）；若将来要做，必须带引用计数与显式 GC，并明确它是宿主资产而非插件资产。
- **诚实边界（必须原样写进 UI / 文档）**：装了工具链 = 本机可执行代码 + 构建期脚本执行 ⇒ **不是沙箱**（`forbidden.md` §禁用 1）。文案只能写「**用户授权的开发环境**」，不得写「安全沙箱」「隔离环境」「受控执行」。审批是**策略级**边界，不提供 OS 级隔离；构建脚本仍可能直接起进程、写文件、联网。

##### A.3 安装落点比较（推荐：插件目录内）

| 方案 | 落点 | 优点 | 问题 | 结论 |
|---|---|---|---|---|
| **插件目录内** | `<exe 根>\Modules\<id>\.toolchain\` | 卸载=删目录即回收（禁用 51）；与 `dependencies[]` 落点同源；无管理员权限；可移植；版本互不污染 | 每个插件各存一份，占磁盘；Windows 路径长度要注意 | **推荐** |
| 宿主共享（可写） | `<exe 根>\ModuleData\toolchain\` | 多插件复用、省磁盘 | 卸载后仍残留 ⇒ 破坏「卸载=不存在」；需要引用计数 / GC；升级互相影响 | 本轮不做 |
| 用户目录 | `%LOCALAPPDATA%\...` | 不占 exe 根 | 违反「数据根 = exe 根」的便携式约定（`code-standards.md:21`）；dev / release 各一份 | **拒绝** |
| 系统全局 | 全局 `PATH` / `npm -g` / `Program Files` | 一次装处处用 | 需管理员；卸载不干净；与用户已有环境冲突；AV 误报 | **拒绝** |

#### B. 前端外观：默认 `inherit` 遵守 `ui-style.md`，声明 `custom` 才绕开

##### B.1 两级规范

| 级别 | 清单字段 | 行为 | 宿主提示 |
|---|---|---|---|
| **inherit（默认）** | `render.appearance: "inherit"` | 继承宿主 token / 间距 / 圆角 / 按钮与态 / 滚动条 / 焦点规则；只消费 CSS 变量，**不复制 token 表** | 无额外提示 |
| **custom（显式）** | `render.appearance: "custom"` | 允许独立外观；仍须满足 B.2 最低要求 | 插件市场详情 / 设置页插件条目显示「独立外观」标签；安装或首次打开时给一次可见提示 |

- 旧的 `render.styleScope` 方案**撤销**（从未施工，无迁移成本）；外观只有 `inherit` / `custom` 两级，避免第二真相源。
- `inherit` 的规范引用：`ui-style.md` §界面 1（色板与变量）、§界面 2（按钮类型与态）、§界面 3（圆角 / 间距 / 边框）、§界面 7（选中 / 禁用 / hover / focus）、§界面 8（主题色明暗与主题包）、§界面 9（视窗与布局硬边界）。**本文不复制任何 token 值**。
- 插件自带 UI 框架仍允许（`agent-templates/modules/README.md:399-424`），但 `inherit` 时框架主题必须接到宿主变量；`custom` 时才可完全自绘。

##### B.2 `custom` 的最低要求（绕开外观 ≠ 绕开宿主边界）

| # | 要求 | 说明 |
|---|---|---|
| 1 | **Esc 归宿主** | 插件不得吞掉 Esc；Esc 先关插件内弹层，再交由宿主关闭面板。 |
| 2 | **键盘可达 + 焦点可见** | Tab 顺序合理；不得有键盘陷阱（模态内可循环，但 Esc 必须能退出）；焦点样式不得去掉（`ui-style.md` §界面 7）。 |
| 3 | **可访问性** | 语义化元素 / label / `aria-live` 状态；对比度可读；不得只把文字画在 canvas 上；尊重 `prefers-reduced-motion`。 |
| 4 | **主题不崩** | 必须响应 `theme.changed` 的明暗模式；固定配色须在清单声明 `theme: "fixed"` 并由宿主提示；不得在深色主题下闪白。 |
| 5 | **布局边界** | 不产生横向滚动；单层滚动；高度必须上报（B.3）；不得覆盖宿主标题栏 / 状态栏。 |
| 6 | **作用域** | 不得读写宿主 DOM（iframe 下天然做不到）；不得依赖宿主 class 名；CSS 必须带插件前缀（旧 light-DOM 兼容期）。 |
| 7 | **无全局副作用** | 不得改宿主 CSS 变量、`document.body`、全局快捷键；不得静默常驻定时器（`detach` 时清理）。 |

> `custom` 只改变**视觉语言**，不改变 C 节的能力审批、D 节的卸载语义与 E 节的测试要求。

##### B.3 iframe 下的 token 注入与尺寸自适应（设计）

- **token 真相源不变**：仍是主窗口算好的内联变量，经 `broadcastThemeVars()` 广播（`main.ts:13247-13252`）；插件窗已有同构接收（`plugin-window.ts:57-70`、`:194-195`）。iframe 不能读宿主 CSSOM，改为**握手后由宿主发快照**。
- **注入顺序**：`hello`（含 `themeMode`）→ `theme.changed`（`inherit` 带 token 快照，`custom` 只带 `mode`，除非声明 `themeTokens: true`）→ `mount`。先注入再挂载，避免首帧白闪。
- **iframe 内落地**：插件把收到的变量写到**自己**的 `documentElement.style`；`inherit` 必须使用 `var(--...)`，不得硬编码色值（`forbidden.md` §禁用 17 / 19）。
- **尺寸上报**：插件用 `ResizeObserver` 观察根节点，发 `ui.resize {height}`；宿主 clamp 到 100–4000（与 `plugin_market.rs:151-152` 同口径）。合并上报用 16–50ms 定时器，**不得用 `requestAnimationFrame` 断言布局完成**（`code-standards.md` §规范 7）。
- **初始高度**：`results` 槽位由宿主给初值，插件首帧后覆盖；`document` 槽位固定为视口高，不随内容伸缩。

#### C. 主控芯片 ↔ 插件芯片：一条信道（传输 / 接收 / 边界隔离）

##### C.1 信道形态与硬禁止

- **唯一通道** = iframe ↔ 宿主的 `postMessage`。宿主侧唯一入口 `app/src/plugins/channel/host.ts`【待建】，插件侧唯一入口 channel client【待建】；两端都只依赖同一份协议类型文件。
- **禁止**：
  - 插件直接 `fetch()` 宿主内部接口或 `127.0.0.1`（现有 CSP 与侧车口径：`tauri.conf.json:35`、`forbidden.md` §禁用 10 / §禁用 3 第 31 条）。
  - 插件访问 `parent.document` / 宿主 `globalThis` / `__lunac_host`。旧 light-DOM 插件仍走 `host.ts:64-69` 的全局桥；**iframe 插件不得使用**，否则边界失效。
  - 宿主放宽 CSP、给 iframe 加 `allow-same-origin` 后又不做来源校验；`targetOrigin` 不得写 `*`。
- **来源校验**：宿主必须校验 `event.source === iframe.contentWindow` 与预期 origin；插件必须校验 `event.source === window.parent`。消息只接受结构化克隆数据，不 `eval`、不 `Function`、不传 DOM 节点。

##### C.2 消息信封与版本协商

```jsonc
{
  "v": 1,                                  // 信道协议版本，独立于 apiVersion
  "id": "3f2a…",                           // 请求/响应关联；事件也带，便于去重与排障
  "type": "req | res | err | event",
  "method": "ui.resize",                   // res/err 回填原请求 method
  "payload": {},                           // 只允许结构化克隆数据
  "ts": 0                                  // 毫秒时间戳，仅排障
}
```

- **版本分离**：`v` = 信道协议版本，从 `1` 开始；`apiVersion` = 宿主桥能力版本，现状为 `2`（`host.ts:59`、`main.ts:2812`、`plugin-window.ts:178`），信道落地时升到 `3`。两者**不得合并**，否则旧插件无法判断「有没有信道」。
- **协商**：宿主 `hello` 里给 `channelVersions: [1]` 与 `apiVersion`；插件回 `ready` 选最高共同版本；无交集 ⇒ `E_VERSION` 并终止挂载。未知 `method`：请求回 `E_UNSUPPORTED`，事件忽略并 warn。**只增不减**；删字段/改语义必须升大版本，并按「至少兼容两个 minor」执行。
- **旧插件**：不握手 ⇒ 宿主继续走 light-DOM 旧路径；新宿主不得因缺少信道而拒绝旧插件。iframe 插件 3s 内不 `ready` ⇒ 显示错误占位，不得空白。

##### C.3 请求-响应、事件、超时与错误码

| 类别 | 语义 | 关联 | 超时 |
|---|---|---|---|
| `req` / `res` / `err` | 一问一答；同一 `id` 只能有一个终态 | `id` | 控制类 10s；`net.fetch` 30s；sidecar 启动用 `sidecar.readyTimeoutSec`（缺省 10s，见 `agent-templates/modules/README.md:270-272`） |
| `event` | 单向通知，不回复 | `id` 仅去重 | 无 |
| 取消 | `type:"req", method:"cancel"` 或复用 `id` 的 `cancel` | `id` | 立即 |

| 错误码 | 含义 | 典型场景 |
|---|---|---|
| `E_TIMEOUT` | 超时 | 宿主/插件未在时限内回包 |
| `E_DENIED` | 能力未授权或用户拒绝 | 未声明 `net.fetch`、信任卡拒绝 |
| `E_UNSUPPORTED` | 方法/能力不认识 | 新插件在旧宿主 |
| `E_VERSION` | 无共同协议版本 | 握手失败 |
| `E_INVALID` | 载荷不合法 | 越界尺寸、路径穿越 |
| `E_NOT_READY` | sidecar / 资源未就绪 | 依赖未下载 |
| `E_LIMIT` | 超过并发/体积/频率上限 | 请求队列满 |
| `E_CANCELLED` | 被取消 | 面板关闭 / 用户取消 |
| `E_HOST` | 宿主内部失败 | 下载失败、IO 错误；**不回传原始凭据/URL**（`forbidden.md` §禁用 59） |

##### C.4 能力表（caps）：清单声明 → 信道可校验

> 原则：**能力只从 `lunac-plugin.json` 派生**，信道不另存一份；宿主对每个请求按能力表校验，未声明的能力一律 `E_DENIED`。

| 能力（信道名） | 清单来源（现状） | 必须宿主代执行 | iframe 可用 | 备注 |
|---|---|---|---|---|
| `ui.mount` | 隐含（所有插件） | 挂载 / 卸载 | ✅ | 基础能力 |
| `ui.layout.takeover` | `permissions: ["layout.takeover"]`（`plugin_market.rs:76-81`） | `document` 槽位铺满 | ✅（槽位内） | 不再直接把 `document.body` 交给插件（`main.ts:10789-10804`） |
| `ui.window.float` | `permissions: ["window.float"]` | 开独立窗 | ✅ | 窗口仍由宿主建 |
| `ui.window.resize` | `permissions: ["window.resize"]` | 改宿主窗口尺寸 | ✅ | 与 `ui.resize`（内容高度）区分 |
| `ui.reuse` | `reuse: [...]`（`plugin_market.rs:231`） | 挂内置面板监听 | ❌ | 需要与宿主同 DOM；iframe 插件**不支持 `reuse`** |
| `process.spawn` | `permissions: ["process.spawn"]` + `sidecar`（`plugin_market.rs:238-243`） | 起/停/请求本机进程 | ✅ | 仍需独立信任卡（`plugin_sidecar.rs:250-263`） |
| `net.sidecar.http` | `sidecar.allowLocalPorts` / `allowLocalAny` | 代请求本机端口 | ✅ | 现存口径 `plugin_sidecar.rs:607-653` |
| `net.fetch` | 新增（待设计） | 代请求远端；域名白名单 | ✅ | **未查到**现成的通用 `net.fetch` 能力；不得让插件直连 |
| `fs.pick` | 新增（待设计） | 弹宿主文件选择 | ✅ | 复用 `@tauri-apps/plugin-dialog` |
| `fs.read` / `fs.write` | 新增（待设计） | 只限用户刚选中的路径 / 插件目录 | ✅ | 需逐次确认或会话授权 |
| `clipboard.read` / `clipboard.write` | 新增能力名（现有命令） | 读写剪贴板 | ✅ | 现有命令 `commands.rs:3197-3220` 等；剪贴板读取必须在主线程（`code-standards.md` §规范 8） |
| `screenshot.capture` | 新增能力名（现有命令） | 截图 overlay | ✅ | 现有入口 `screenshot.rs:84 / :111 / :128`；隐私口径见 `screenshot.rs:382-385` |
| `shell.openUrl` | 新增能力名（现有 `plugin-shell`） | 只开 URL | ✅ | 本地路径必须走宿主 `open_path`，见 `code-standards.md:65` |
| `assets.installed` | `dependencies[]` | 提供只读路径 | ✅ | 文件已按 `safe_join` 落在插件目录 |
| `notify` | 新增（待设计） | 宿主通知 | ✅ | **未查到**现成的插件通知能力；不得用 DOM 伪造宿主提示 |

> 表中「新增（待设计）」= 当前没有对应的插件能力名 / 通用入口，只有宿主内部实现或完全没有；施工前必须先立项，**不能假定已存在**。

##### C.4.1 挂载锚点（**2026-10-17 用户已拍板：新增两个**）

现有 3 个（`index.html:223`、`plugin.html:54`、`plugin-window.ts:40`）：`results`（`#results-list`）、`document`（`document.body`）、`window`（插件窗 `#results-list`）。

**本轮定稿 = 再新增 2 个**（用户选择「新增锚点」）：

| 槽位 id | 宿主位置 | 用途 | 施工注意 |
|---|---|---|---|
| `chat-sidebar` | 聊天区侧栏（主对话 / 独立聊天窗共用） | 常驻小面板（状态、快捷操作） | 必须与 `#results-list` 同源重绘机制解耦；窄窗需可折叠，否则挤占对话宽度 |
| `settings-section` | 设置面板内一个分组容器 | 插件自己的配置界面 | 分页机制沿用 `settings.ts` 的 `data-cat` ↔ `data-pane`（`:2659-2681`），**不要**再引第三套分页；分组需可被插件声明隐藏 |

- ⚠️ **只加这两个**；再加锚点需**重新拍板**（每加一个都扩大宿主 DOM 契约与回归面）。
- ⚠️ 两个新锚点都必须满足：**卸载 = 完全不存在**（删目录后宿主不得残留占位/空白分组）；无插件时**不占位、不留空容器**。
- ⚠️ 新的槽位名要进 `mount` 的 `slot` 枚举（见 C.5）与清单校验，**不是**自由字符串。

##### C.5 消息清单（传输与接收）

**宿主 → 插件（传输）**

| method | 类型 | payload | 说明 |
|---|---|---|---|
| `hello` | event | `{channelVersions, apiVersion, pluginId, caps, themeMode, locale}` | 握手起点 |
| `mount` | req | `{input, slot, size}` | 挂载；`slot` ∈ `results` / `document` / `window` / **`chat-sidebar`** / **`settings-section`**（后两个为 2026-10-17 新增，见 C.4.1） |
| `execute` | req | `{input}` | 重新执行（等价现有 `execute(input)`） |
| `theme.changed` | event | `{mode, tokens?}` | `inherit` 带 token；`custom` 默认只带 `mode` |
| `locale.changed` | event | `{locale}` | 语言切换 |
| `host.resize` | event | `{width, height}` | 宿主视口变化 |
| `visibility.changed` | event | `{visible}` | 面板显示 / 隐藏 |
| `lifecycle` | event | `{phase: "suspend"\|"resume"\|"dispose"}` | 生命周期；`dispose` 后不得再发请求 |
| `ping` | req | `{}` | 健康检查 / 超时探测 |

**插件 → 宿主（接收）**

| method | 类型 | payload | 说明 |
|---|---|---|---|
| `ready` | event | `{channelVersion, capabilitiesUsed}` | 握手应答；宿主据此决定是否放行 |
| `ui.resize` | event | `{height, width?}` | 内容尺寸上报；宿主 clamp |
| `ui.status` | event | `{text}` | 状态行文本（走 `t()`，不得内嵌 emoji） |
| `ui.notify` | req | `{level, message}` | 宿主通知；需 `notify` 能力 |
| `net.fetch` | req | `{url, method, headers?, body?, timeoutMs?}` | 宿主代请求；域名白名单；响应体限长 |
| `fs.pick` | req | `{mode: "open"\|"save", filters?}` | 用户选择路径 |
| `fs.read` / `fs.write` | req | `{path, ...}` | 只限授权范围 |
| `clipboard.read` / `clipboard.write` | req | `{text?}` | 剪贴板 |
| `screenshot.capture` | req | `{mode}` | 截图 |
| `shell.openUrl` | req | `{url}` | 只开 URL |
| `process.start` / `stop` | req | `{}` | sidecar 生命周期（映射 `sidecar.ts:168-208`） |
| `process.request` | req | `{method, params}` | stdio NDJSON 请求 |
| `process.http` | req | `{method, path, body?, port?}` | 宿主代请求本机端口（`sidecar.ts:211-219`） |
| `log` | event | `{level, message}` | 插件日志；宿主脱敏并限频 |
| `bye` | event | `{}` | 主动退出；宿主清理 |

##### C.6 传输与接收语义（背压 / 限额 / 顺序）

- **顺序**：同一方向的事件按发送顺序处理；请求可交错，响应按 `id` 关联，不保证全局顺序。
- **并发**：宿主 → 插件最多 8 个未决请求；插件 → 宿主最多 16 个；超限排队，队列上限 32，再超 `E_LIMIT`。`ui.resize` / `log` 不排队，直接合并 / 丢弃并计数。
- **体积**：控制类 payload ≤ 1 MiB；`net.fetch` 响应默认 ≤ 8 MiB（按能力配置）；`fs.read` 走分块或返回临时文件句柄【推论：分块协议需在施工时定稿】。
- **频率**：`ui.resize` 16–50ms 合并；`log` ≤ 20 条/秒；`ui.status` ≤ 5 条/秒。
- **背压**：队列满时宿主回 `E_LIMIT`，插件必须降级（丢弃低优先级事件），不得无限重试。
- **关闭**：面板关闭 → 宿主发 `lifecycle:dispose` → 等待 200ms → 摘 iframe；期间到达的响应按 `E_CANCELLED` 处理。

##### C.7 边界隔离的诚实结论（必须写进实现与文案）

- **能做到**（仅当 iframe 为**不透明源**且协议正确）：DOM / CSS / 脚本作用域隔离；插件拿不到宿主 DOM、变量、`__lunac_host`；宿主按 caps 逐请求审批。
- **做不到**：本机权限隔离。插件代码一旦被安装并运行，仍是**本机可执行代码**；它能拿到的能力由用户授权决定，审批是策略级，不是 OS 级（`forbidden.md` §禁用 1）。
- **同源 iframe 不算边界**：`allow-same-origin` 下插件可访问 `parent.document`，只能算「样式/布局隔离」。**不得**把这种形态宣传成「隔离」或「沙箱」。
- **postMessage 本身不是安全边界**：必须做 `source` / `origin` 校验与 caps 校验；不得把「用了 postMessage」当作安全证明。
- **一句话**：信道解决「主控芯片与插件芯片怎么说话、说什么、能要什么」；**不解决「插件是不是恶意代码」**。装谁 = 信任谁，这条没有变。

#### D. 芯片封装：把插件打成独立可交付单元

##### D.1 封装产物（目录 / 清单 / 入口 / 依赖 / 资源 / 校验）

```text
Modules\<id>\
  lunac-plugin.json        # 唯一清单 / 唯一真相源（现有，plugin_market.rs:47, :190-244）
  index.js                 # DOM 引擎入口；iframe 插件也保留一份「旧宿主降级占位」入口
  ui\index.html            # iframe 引擎入口（可选；render.engine = "iframe" 时必需）
  ui\...                   # 前端资源（打包产物）
  server\...               # 后端 sidecar（可选；由 sidecar.command 声明）
  assets\...               # 其它资源
  .toolchain\              # 轨道 H 的开发工具链（安装期生成，**不进发布包**）
  node_modules\            # 现有 npm 依赖落点（可选，不进发布包）
```

- **清单新增字段（设计，待施工）**：`render.engine: "dom" | "iframe"`（默认 `dom`）、`render.appearance: "inherit" | "custom"`（默认 `inherit`）、`dev`（工具链声明）。`provides` / `consumes` 仍按上一轮设计，但**本轮不施工**。
- **入口规则**：`entry` 仍是 ESM（`plugin_market.rs:636-649`）；`render.engine="iframe"` 时额外要求 `ui/index.html` 存在。**`index.js` 不得删除**——它是旧宿主上的降级占位，避免「装了新插件在旧宿主上一片空白」。
- **依赖**：运行时资产继续走 `dependencies[]`，复用 4 GiB / 1 GiB / 32 条 / 32 MiB / 192 MiB / 4000 条闸（`plugin_market.rs:49-53`、`:60-66`）。
- **校验**：
  - 依赖与 sidecar 已有 `sha256` 口径（`plugin_market.rs:283-286`、`:344`、`:1066-1094`、`:1123-1146`）。
  - **插件包本体目前没有 sha256**：市场索引 `PluginIndexEntry` 无该字段（`plugin_market.rs:717-735`），安装路径只校验 https 与体积（`commands.rs:2894-2944`）。
    **✅ 2026-10-17 用户已拍板：强制** —— 索引新增 `sha256` 字段 + 安装前校验；**旧包（无哈希）迁移期只 warn、不拒装**，迁移期结束后转强制。芯片封装以此为前提。
  - 构建元数据（buildId、文件哈希）放**包外**的 `SHA256SUMS` / 索引条目，**不放进插件目录当运行时真相**，避免第二份清单（禁用 62）。

##### D.2 「独立芯片」的含义边界

- **可以**：独立交付（zip / 目录）、独立版本号、干净卸载（删 `Modules\<id>\` 即不存在）、独立依赖与资源。
- **不可以脱离宿主独立运行前端**：
  - 加载依赖 `convertFileSrc` + 动态 `import()`（`market.ts:222-231`）；
  - CSP 只放行 `'self'` 与 asset 协议（`tauri.conf.json:35`）；
  - 宿主桥 `__lunac_host` 由宿主注入（`host.ts:64-69`、`main.ts:2812`、`plugin-window.ts:178`）；
  - 即使改成 iframe + 自定义 scheme，iframe 仍由宿主创建、注入 token、代执行能力。
- **后端 sidecar 也不能原样脱离宿主**：`spawn_proc` 依赖 `AppHandle`（`plugin_sidecar.rs:317-323`），事件走 `emit_to`（`:523`）。它可以作为独立进程被别的程序启动，但那就不是「Lunac 插件」语义了。
- **结论**：独立芯片 = **可独立交付 / 可独立卸载**，不是**可独立运行**。文案必须写清，避免用户以为能脱离 Lunac。

##### D.3 迁移路径与兼容矩阵

- **迁移是 opt-in**：`render.engine` 默认 `dom`，现有 light-DOM 插件零变化；只有显式声明 `iframe` 的插件走新路径。
- **旧插件 → iframe**：不是自动迁移；由插件作者（或 AI 在轨道 H 下）重写入口为 `ui/index.html` + 信道客户端，同时保留 `index.js` 占位。

| 产物 \ 宿主 | 旧宿主（无信道 / `apiVersion < 3`） | 新宿主（有信道） |
|---|---|---|
| 旧 DOM 插件（无 `render`） | 正常 | 正常，走旧路径，零变化 |
| 旧 DOM 插件（有 `reuse`） | 正常 | 正常；`reuse` 仍只在 DOM 引擎可用 |
| iframe 插件（带 `index.js` 占位） | 显示「需要更新宿主」占位，不崩 | 走 iframe + 信道 |
| iframe 插件（不带占位） | **可能空白 / 加载失败** | 正常 |
| `custom` 外观 + 旧宿主 | 字段被忽略，light-DOM 下样式可能外泄 | iframe 内天然有样式边界 |
| 带 `dev` 段 + 旧宿主 | 字段被忽略，无影响 | 可见「需要授权开发工具链」，不自动安装 |

- **兼容纪律**：新宿主必须同时支持 `dom` 与 `iframe`；旧宿主不认识的新字段由 serde 忽略（全仓无 `deny_unknown_fields`，见 G 表 1.3），但**不能保证 iframe 插件可用**——所以必须带占位入口。

#### E. 施工顺序（2026-10-18 部分施工；E.0 / E.1 / E.2 悬浮窗已完成）

> 顺序按用户已拍板：`provides`（身份）→ 信道协议 + iframe opt-in（**先落 vitest**）→ 外观双级 → 工具链授权（轨道 H）→ 芯片封装 → Job Object 限额。`provides` 已于 2026-10-18 施工。

##### E.0 先落 vitest（2026-10-18 已完成）

- **破例范围**：只新增 `vitest` 到 `app/package.json` 的 `devDependencies`（现状 `:25-31`），并加 `test` / `test:watch` 脚本（现状 `:6-13`）。**不改** `dependencies`（`:14-24`）、**不加**运行时依赖、**不引** UI 框架 / 图标库 / 动画库；`code-standards.md:55` 的「不新增前端依赖」按「不新增运行时依赖」解释，本次破例仅限 devDependency。
- **测试范围**：先测**纯协议逻辑**（信封校验、版本协商、caps 校验、超时/错误码、resize 合并），不测真实 iframe DOM；DOM 级测试等信道落地后再评估是否需要额外 devDependency。
- **为什么必须破例**：前端现状无 test 脚本、无 vitest（`app/package.json:6-13`、`:25-31`），`app/src` 无自有测试；不改宿主前端逻辑则无法做到「先红后绿」。

##### E.1 阶段 1：`provides` 身份（2026-10-18 已施工）

- **施工状态（2026-10-18）**：清单字段、校验、`provides` 冲突 warn 与
  `resolve_plugin_dir()` 已落地；窗口形态（`plugin_window::declared_shape`）、
  sidecar（`plugin_sidecar::spec_for`）、依赖安装（`install_plugin_dependencies`）与
  `local_model_env()` 已全部切到 resolver（不再按目录名硬编码）。前端仍按 id 展示 ——
  那是**身份**，不是目录定位。
- **全量声明（2026-10-18）**：官方磁盘插件（music / clipboard-history / ocr / convert /
  proxy / pet / local-model）、7 个基础（内置）插件与 `resource-hub` 均已声明 `provides`；
  `local-model` 另补 `sidecar.memoryLimitMiB` / `processLimit`。声明只是身份，不改运行行为。
- **已拍板规则**：多个插件声明同一 `provides` ⇒ **warn + 按 id 升序取第一个**，不拒装。
- **设计要点**：`provides: string[]`，默认 `[]`，上限 8，元素沿用 `[a-z0-9._-]`、≤32（`plugin_market.rs:505-519` 的口径）；新增唯一解析器 `resolve_plugin_dir(root, key)`（精确 id/目录优先，再按 `provides` 扫）；`id = 目录名` 仍是硬契约（`plugin_market.rs:192`），`provides` **不用于改目录名**。
- **施工时改动点（本轮不动）**：`plugin_market.rs:190-244`、`:525-566`、`:652-711`、`:1517-1585`；`commands.rs:639-660`、`:2956-2961`；`plugin_sidecar.rs:166-181`；`plugin_window.rs:280-286`；`market.ts:232-235`；`settings.ts:2931`；`kinds.ts:72-74`；`scripts/build-plugins.ps1:294`、`:423-434`；`agent-templates/modules/README.md:93-107`。
- **兼容**：旧插件无 `provides` ⇒ 回落到目录名，仅 warn 一次；新插件在旧宿主 ⇒ serde 忽略，目录名=id 时行为不变。
- **测试（施工时）**：`manifest_parses_provides_identity`、`resolve_plugin_dir_prefers_declared_provides`、`resolve_plugin_dir_falls_back_to_directory_for_legacy`、`resolve_plugin_dir_ignores_invalid_manifest`、`provides_conflict_warns_and_picks_lowest_id`。
- **风险**：把 `provides` 误当「可任意重命名目录」⇒ 窗口 label / 会话 key / 构建白名单全部错位；文档必须写明只用于**按能力定位**。
- **越界**：不新增运行时依赖；不引入注册表；不按插件 id 写 bundle 分支（`forbidden.md` §禁用 51）。

##### E.2 阶段 2：信道协议 + iframe opt-in（悬浮窗、主窗与两个常驻锚点均已施工）

> **源模型（2026-10-18 已定）**：iframe 不从 `asset.localhost` 加载。asset 协议的
> `Access-Control-Allow-Origin` 绑定窗口 origin，不透明源 / asset 源的 ESM 跨源加载不可靠；
> 改用自定义 scheme：**Windows / Android 上 iframe 的 `src` 必须写 `http://lunac-plugin.localhost/<id>/ui/index.html`**
> （WebView 不能直接加载自定义 scheme；写 `lunac-plugin://` 会「load 不触发」，2026-10-29 真机定案）；
> macOS / Linux 仍是 `lunac-plugin://localhost/<id>/ui/index.html`，两态由
> `app/src/plugins/channel/host.ts` 的 `pluginFrameUrl()` 统一给出。iframe 同源加载自己的 `ui/` 资源，
> 与宿主仍跨源；`plugin_scheme.rs` 只暴露 `ui/` 与 `assets/` 并附加 iframe CSP。

- **改动点（施工时）**：
  - 新增 `app/src/plugins/channel/protocol.ts`（纯类型 + 校验 + 版本协商 + 错误码）与 `host.ts` / `client.ts`【待建】；测试同目录 `*.test.ts`。
  - `plugin_market.rs:190-244` 加 `render.engine` / `render.appearance` 的解析与校验；`InstalledPlugin`（`:525-566`）下发。
  - `main.ts:10783-10805` 按 `render.engine` 分支：`dom` 走现状；`iframe` 建 iframe、发 `mount`；`plugin-window.ts:74-104` 同步。
  - `tauri.conf.json:35` 的 CSP 需增加 `frame-src`（当前没有）；是否新增自定义 scheme 取决于上面的**前置实验**（H.4）。
  - **新增两个锚点**（H.3 已拍板，见 C.4.1）：`chat-sidebar`（聊天区侧栏）与 `settings-section`（设置面板分组）——两者都要在 `mount` 的 `slot` 枚举与清单校验里登记，并满足「无插件不占位 / 卸载不残留」。
    **施工口径（2026-10-18）**：清单用 `render.slots: ["chat-sidebar" | "settings-section"]`
    声明，**只允许 iframe 引擎**；宿主 `anchor-slots.ts` 统一挂载，`market.ts` 每次重扫后
    广播 `lunac-market-refreshed` 触发对账；设置分组随面板关闭 `detachSettingsHost()` 回收。
  - 主题注入复用 `main.ts:13247-13252` / `plugin-window.ts:57-70`。
- **兼容**：默认 `dom`；旧插件零变化；iframe 插件必须带 `index.js` 占位（D.3）。
- **测试**：vitest：`channel_handshake_negotiates_lowest_common_version`、`channel_rejects_unknown_version_with_E_VERSION`、`channel_unknown_method_returns_E_UNSUPPORTED`、`channel_request_times_out_with_E_TIMEOUT`、`channel_requires_cap_for_net_fetch`、`channel_resize_events_are_coalesced`、`channel_rejects_cross_origin_source`。Rust：`manifest_accepts_iframe_engine_with_entry_html`、`manifest_rejects_iframe_engine_without_entry_html`、`manifest_engine_defaults_to_dom`。
- **风险**：iframe 加载 asset 的跨源 / CSP（H.4 未决）；`reuse` 在 iframe 不可用；旧插件样式外泄不会因新字段自动修好。
- **越界**：不放宽 CSP 的 `connect-src` / `script-src`；不让插件直连端口（`forbidden.md` §禁用 10）；不称「沙箱」。

##### E.3 阶段 3：外观双级（`inherit` / `custom`）—— 2026-10-18 已施工

- **施工状态**：清单字段 + `market.ts` / `registry.ts` 透传 + 设置页「独立外观 / iframe 引擎」
  标签已落地；`custom` 首次打开时用应用内浮层提示一次（`lunac-appearance-noticed`，
  按插件 id 记忆，不占结果区）；`inherit` 的 token 快照经 `channel/theme.ts` 注入 iframe。
- **改动点**：`plugin_market.rs` 的 `RenderSpec` 增加 `appearance` 枚举；`market.ts` / `registry.ts:15-43` 透传；`settings.ts` 插件条目显示「独立外观」；iframe 侧 token 注入（B.3）。
- **兼容**：默认 `inherit`；`custom` 只在显式声明时生效；旧宿主忽略字段，light-DOM 下仍可能外泄 ⇒ 文档要求插件 CSS 加前缀。
- **测试**：`manifest_appearance_defaults_to_inherit`、`manifest_rejects_unknown_appearance`、vitest `theme_snapshot_is_sent_only_for_inherit`、`theme_mode_is_sent_to_custom`、`resize_payload_is_clamped`。
- **风险**：custom 绕开导致视觉割裂 / 可访问性回退；宿主必须保留 B.2 的最低要求并可见提示。
- **越界**：不新增主题机制；不复制 `ui-style.md` token；不硬编码色值（`forbidden.md` §禁用 17-20）。

##### E.4 阶段 4：工具链授权（轨道 H）—— 2026-10-18 已施工

- **施工状态（2026-10-18）**：`plugin_market.rs` 新增 `PluginDevSpec` / `ToolchainProfile` /
  `PluginBuildSpec` 与 `dev_fingerprint()`、`validate_dev()`；`plugin_toolchain.rs` 落地
  `status` / `grant` / `revoke` / `install` / `run_build` / `stop_build`；`commands.rs` 6 条薄壳
  （`run_blocking`）；`main.rs` 注册；设置页「开发工具链」分区（授权 / 撤销 / 安装 / 构建 /
  输出日志）+ 两段式撤销确认。测试：`manifest_validates_dev_toolchain_and_build`、
  `toolchain_grant_requires_exact_fingerprint`、`toolchain_revoke_removes_grant`、
  `toolchain_build_command_stays_inside_plugin_dir`。
- **与设计差异**：`build.command` 采用「包内相对路径 + 构建器白名单」双轨；白名单裸命令
  **只从工具链 `binDir` 解析**、不回落全局 `PATH`（避免「装了工具链却用系统程序」）。
- **改动点（施工时）**：
  - 新增 `app/src-tauri/src/plugin_toolchain.rs`【待建】：授权记录读写（`config\plugin-toolchain.json`）、`install_toolchain` / `revoke_toolchain` / `run_build` 命令；所有命令 `async` 薄壳 + `run_blocking`（`code-standards.md` §规范 5）。
  - 复用 `plugin_market.rs` 的 `safe_join` / `sha256` / 体积闸 / staging（`:895-916`、`:1066-1146`）。
  - `plugin_sidecar.rs:51-52` 的信任文件模式作为授权账本先例；构建进程 spawn 后立刻 `child_job::assign`（`:367`）。
  - `app/src/plugins/...` 的申请入口 + 设置页授权 / 撤销 UI；`settings.ts` 加「开发工具链」分区。
  - `agent-templates/modules/README.md` 增加 `dev` 段说明（`:93-107`、`:426-484` 附近）。
- **兼容**：无 `dev` 段 = 无变化；旧宿主忽略；工具链安装不影响插件运行。
- **测试**：`toolchain_grant_requires_user_approval`、`toolchain_fingerprint_changes_invalidate_grant`、`toolchain_install_stays_inside_plugin_dir`、`toolchain_revoke_removes_grant_and_kills_build`、`toolchain_archive_respects_dep_size_gates`；Windows 手工：撤销后无法再跑构建、删除插件目录后 `.toolchain\` 不残留。
- **风险**：任意代码执行 / 供应链；`npm` 生命周期脚本；磁盘占用；AV 误报；构建脚本绕过宿主直连网络。
- **越界**：不是沙箱（禁用 1）；不写全局 `PATH` / 全局 npm；不把工具链塞进 `dependencies[]` 自动安装；不新增运行时依赖。

##### E.5 阶段 5：芯片封装（构建 / 发布 / 校验）—— 2026-10-18 已施工（sha256）

- **施工状态**：`PluginIndexEntry.sha256` + `parse_index` 格式校验 + `verify_package_sha256()`
  已落地；`install_plugin_from_url(url, sha256)` 在进入安装管线前逐字节校验（不匹配即拒、
  旧版本不动；缺省 = 迁移期 warn 后安装）；`settings.ts` 从索引透传到命令；
  `publish-plugins.ps1` 自动写 `sha256`。测试：`index_sha256_is_validated_and_packages_are_verified`。
- **改动点**：`scripts/build-plugins.ps1`（清单白名单 `:423-434`、`local-model` 元数据 `:294`）与 `scripts/publish-plugins.ps1:63 / :71` 支持 `render.engine` / `appearance` / 可选 `dev`；生成包外 `SHA256SUMS`；`plugin_market.rs:717-735` 的索引条目加 `sha256`，`commands.rs:2894-2944` 安装前校验；打包时**排除** `.toolchain\`、`node_modules\`、`sdk\`。
- **兼容**：旧包无 `sha256` ⇒ 首次仍可装但 warn（或按发布策略拒装，需拍板）；旧宿主忽略新字段。
- **测试**：Rust `index_sha256_is_required_for_chip_packages`、`install_rejects_zip_hash_mismatch`；脚本级 `npm run verify`（BOM）+ 构建产物大小 / 条目数断言。
- **风险**：构建产物混入开发文件 / 模型；zip 体积超 32 MiB 需改走 `dependencies[]`；哈希发布流程要防「索引与包不同步」。
- **越界**：不新增第二份运行时清单；不手工改 `release\Lunac\Modules\README.md`（构建产物）。

##### E.6 阶段 6：Job Object 限额（资源闸，不是隔离）—— 2026-10-18 已施工

- **施工状态（2026-10-18）**：`child_job.rs` 新增 `JobLimits` / `Job`（`KILL_ON_JOB_CLOSE` +
  `JOB_OBJECT_LIMIT_PROCESS_MEMORY` + `JOB_OBJECT_LIMIT_ACTIVE_PROCESS`，`Drop` 关闭句柄）；
  `plugin_sidecar.rs` 的每个 `Session` / `Proc` 持有自己的 `Job`，起进程时按清单建限；
  分配失败回落全局 `child_job::assign`（保证崩溃回收）并 warn「限额未生效」。
  清单新增 `sidecar.memoryLimitMiB`（`0` 或 `32..=32768`）与 `processLimit`（`0..=64`）。
  测试：`sidecar_job_limits_are_positive`。**Windows 触顶行为与退出回收仍需真机确认。**
- **现状**：随宿主退出回收已有（`child_job.rs:88-105`、`plugin_sidecar.rs:367`）；`ExtendedLimits` 的 `process_memory_limit` / `job_memory_limit` 仍为 0（`child_job.rs:70-73`）。
- **改动点**：为每个 sidecar 建独立 job（不能直接在包含 agent 的全局 job 上加内存上限）；新增 `sidecar.memoryLimitMiB` / `processLimit`（或宿主缺省），`plugin_sidecar.rs:317-367` 使用；保留 kill-on-close。
- **测试**：`sidecar_job_limits_are_positive`、`sidecar_job_limits_do_not_apply_to_global_agent_job`；Windows 真机用任务管理器确认触顶行为与退出回收。
- **风险**：限额过小会杀大模型 sidecar（如 4B 模型）；必须可配置并给缺省建议值。
- **越界**：资源闸 ≠ 隔离，不得据此自称沙箱；不改变全局 agent job 的语义。

##### E.7 不做清单（本轮刻意边界）

1. **不重设计骨架**：目录即真相源、清单声明式、additive 三条保留；不引入注册表 / 数据库。
2. **不把插件系统说成沙箱**：没有 OS 级隔离（`forbidden.md` §禁用 1）。
3. **不做全局包管理器**：不建全局 `PATH` / `npm -g` / 共享可写工具链缓存；工具链只在插件目录内。
4. **不自动执行构建**：`dev.build.command` 永不随安装自动跑。
5. **不把 iframe 设为默认**：默认 `dom`，避免一次全炸。
6. **不在 iframe 里支持 `reuse`**：它依赖与宿主同 DOM；新引擎不承诺。
7. **不做 AppContainer 底座**：会拦设备 / 网络（CUDA / Vulkan 走不通为【推论】，仓库无实现可实测）。
8. **不做目录名与 id 脱钩**：`id = 目录名` 仍是硬契约。
9. **不手工改 `release\Lunac\Modules\README.md`**：构建产物由脚本重新生成（当前副本已写 `2`）。

#### F. 明确「推翻 / 保留 / 新增」

| # | 上一版结论 | 本次修订后 | 依据 / 说明 |
|---|---|---|---|
| 1 | 不做 SDK | **可选 SDK（需用户授权）**；轨道 L 最低门槛保留 | A.1 / A.2；SDK = 本地类型 + 信道胶水，不发 npm 包 |
| 2 | 不扩依赖 / 不要 node_modules | **保留 `dependencies[].type="npm"`**（已支持），**新增工具链授权**（不入运行时依赖） | `plugin_market.rs:259-260`、`:1288-1352`；A.2 |
| 3 | 不做 iframe，shadow root 作 opt-in | **iframe 作 opt-in**；shadow root **放弃** | 用户已拍板；B.3 / D.3 |
| 4 | `apiVersion` 文档要修 | **已不成立，撤掉** | 源 `agent-templates/modules/README.md:370` 与 release `:354` 都是 `2` |
| 5 | `provides` 本轮施工 | **本轮只出设计，不施工** | 用户已拍板；E.1 |
| 6 | 同 `provides` 冲突待拍板 | **warn + 按 id 升序取第一个**，不拒装 | 用户已拍板；E.1 |
| 7 | 前端无 runner，零新增依赖 | **引入 vitest**，破例仅限 devDependency | 用户已拍板；E.0；`app/package.json:6-31` |
| 8 | 包管理器 / node_modules 不进本轮 | **不建全局包管理器**；工具链 profile 只在插件目录内，需授权 | A.3 / E.4 |
| 9 | 「沙箱 / 隔离」措辞 | **不是沙箱**；iframe 只是 DOM/CSS/脚本作用域边界 + 能力边界 | `forbidden.md` §禁用 1；C.7 |
| 10 | `render.styleScope: plugin/global` | **改为 `render.appearance: inherit/custom`** + `render.engine: dom/iframe` | B.1；旧字段从未施工，无迁移 |
| 11 | `consumes` 声明（只 warn） | **保留为可选设计**，不进入本轮施工 | 上一版 C 节；能力定位仍以 `provides` 为主 |
| 12 | Job Object 随宿主退出已有 | **保留**；只补内存 / 进程数上限 | `child_job.rs:88-105`、`plugin_sidecar.rs:367` |
| 13 | 目录即真相源、卸载=删目录、additive、`id = 目录名` | **全部保留** | G 表 1.1-1.3、2.1；`plugin_market.rs:192` |

- **仍成立的三条骨架**：① 目录即真相源（扫描 `plugin_market.rs:1517-1585`，卸载 `:1588-1605`）；② 清单声明式扩展点（`permissions` / `reuse` / `window` / `sidecar`，`:220-243`）；③ additive 演进（未知 permission 只 warn `:682-688`，未知 reuse 只 warn `attach.ts:69-73`，serde 忽略未知字段）。
- **仍成立的硬契约**：`id = 目录名`（`plugin_market.rs:192`）；`provides` 只解决「按能力定位」，**不支持任意改目录名**。

#### G. 核实底账（沿用 2026-10-17 只读调研，只记事实）

> 本表是上一轮的核实结果，**只作为事实底账**；方案与顺序以 A–F 为准。表中旧建议（尤其 3.3「不要 node_modules」、3.6「建议顺序」）**已被本轮修订取代**。

##### G.1 逐条核对面（2026-10-17 只读调研）

| # | 对话结论 | 判定 | 证据 / 现状 |
|---|---|---|---|
| 1.1 | 目录即真相源：`Modules\<id>\` 存在即插件存在，无注册表，卸载=删目录 | ✅ 仍成立 | 扫描只读目录：`plugin_market.rs:1517-1585`；卸载只删 `<plugins_root>\<id>`：`:1588-1605`；前端 `market.ts:155-192` 先摘后挂、不持久化任何注册表；清单注释明写「id 同时是目录名」：`plugin_market.rs:192`。 |
| 1.2 | 清单声明式扩展点：`permissions` / `reuse` / `window` / `sidecar` | ✅ 仍成立 | `PluginManifest` 字段：`permissions` `plugin_market.rs:220`、`reuse` `:231`、`window` `:237`、`sidecar` `:243`；已知能力仅 4 个：`KNOWN_PERMISSIONS` `:76-81`（`layout.takeover` / `process.spawn` / `window.resize` / `window.float`）。 |
| 1.3 | additive 演进：不认识 permission / reuse 只 warn，未知字段不报错 | ✅ 仍成立 | 未知 permission 只 warn：`plugin_market.rs:682-688`；未知 reuse 由前端 warn、不拒装：`attach.ts:69-73`；Rust 侧无 `deny_unknown_fields`（全文件 grep 零命中）⇒ serde 默认忽略未知字段。 |
| 2.1 | 债 1：`local_model_env()` 直接按目录名 `local-model` 读清单 | ✅ 仍成立 | `commands.rs:639-660`（`:644-646` 拼 `plugins_dir().join("local-model").join("lunac-plugin.json")`）；调用点 `:460-468`。 |
| 2.2 | 债 2：`reuse` 只认内置面板 id，白名单硬编码；`MAX_REUSE = 4` | ✅ 仍成立 | 硬编码 switch：`attach.ts:27-57`，实际可复用 **5 个**：`settings` / `tool-editor` / `quick-launch` / `memo` / `translate`；`MAX_REUSE = 4`：`plugin_market.rs:72`（Rust 只校验形状与条数 `:693-705`，不认识的面板 id 由前端 warn `attach.ts:72`）。 |
| 2.3 | 债 3：`agent-templates\modules\README.md:344` 写「桥的协议版本，当前 1」，与实际 2 矛盾 | ❌ **已不成立（文档已修）** | 源 README `:370` = 版本当前 **`2`**；`:257` = `apiVersion >= 2` 才有 sidecar；发布副本 `release\Lunac\Modules\README.md:354` = **`2`**、`:241` = `>= 2`；宿主 `host.ts:59` = 当前 2；实际注入 `main.ts:2812` / `plugin-window.ts:178` 均为 `apiVersion: 2`。全仓 grep「当前 1 / 协议版本 1」零命中 ⇒ **当前无矛盾**。对话引用的 `:344` 现为 `attach/detach` 代码示例行，不是版本声明。 |
| 2.4 | 债 4：单条 ≤ 4 GiB、解压 ≤ 1 GiB、依赖 ≤ 32 条；是刻意边界 | ⚠️ **数值全对，适用对象需写清** | `MAX_DEP_BYTES = 4 GiB`：`plugin_market.rs:60`；`MAX_DEP_ENTRIES = 32`：`:62`；`MAX_DEP_EXTRACT_BYTES = 1 GiB`：`:66`。三者都是清单 **`dependencies[]`** 的闸；`sidecar` 段本身**没有**独立体积闸（其 `command` 由依赖下载闸约束）。插件包另有 32 MiB / 192 MiB / 4000 条：`:49 / :51 / :53`。 |
| 3.1 | 引脚提案：清单加 `provides` / `consumes` / `render` | ✅ 确认为「新增」 | 现有清单字段只有 `id/name/description/keywords/icon/version/entry/homepage/dependencies/permissions/reuse/window/sidecar`（`plugin_market.rs:190-244`）；全仓（排除 `target/`）grep `provides` / `consumes` / `render` 只命中 `tuning-engine/src/main.rs:73` 的无关命令 ⇒ **三字段均不存在**。 |
| 3.2 | 当前插件与宿主同一 document，全仓无 iframe / shadowRoot；id 冲突 / 全局 CSS / `innerHTML` 注入是真实洞 | ✅ 仍成立 | `app/src` grep `iframe` / `shadowRoot` / `attachShadow` **零命中**；非 takeover 直接 `resultsList.innerHTML`：`main.ts:10794-10797`，takeover 直接铺满：`:10789-10792`；`attach` 的 root 在 takeover 时是 `document.body`：`:10804`；插件窗同样 `root.innerHTML`：`plugin-window.ts:40 / :100-104`、`plugin.html:54`。`type:"html"` 走 `innerHTML` 的说明见 `README.md:313`、`market.ts:20-22`。 |
| 3.3 | 依赖：CSP 只许本地加载、禁 http import ⇒ 第三方库打进单一入口；不要 node_modules / 包管理器 | ⚠️ **CSP 部分成立；「不要 node_modules」与现状不符** | CSP 仅放行 `'self'` + asset：`tauri.conf.json:35`；加载确实走 `convertFileSrc` + 动态 `import()`：`market.ts:227-228`。但清单**已支持** `dependencies[].type="npm"`（`plugin_market.rs:259-260`、`:469-486`、`:1292+`），装进插件目录 `node_modules\`；模块 README 也把它列为可选路径（`:409`、`:459-461`），只是推荐「打进入口」（`:408`、`:418`）。⇒ **只能「不扩、降级为最后手段」，不能直接删**。 |
| 3.4 | 真 OS 沙箱：Job Object 便宜有用（内存/进程数上限 + 随宿主退出回收），但只是资源闸；AppContainer 能真隔离但设备访问报废 | ⚠️ **部分已实现** | 随宿主退出回收**已有**：`plugin_sidecar.rs:367` 每次 spawn 后 `child_job::assign`；`child_job.rs:88-105` 只设 `KILL_ON_JOB_CLOSE`，**未设**内存 / 进程数上限（`ExtendedLimits` 的 `process_memory_limit` / `job_memory_limit` `:70-73` 保持 0）。AppContainer 全仓无实现（仅 `todo.md:731` 等调研条目）⇒「设备访问报废」为【推论】，未在代码/实测中验证。 |
| 3.5 | TDD 现实：Rust 可严格 TDD；前端无 runner；CDP 夹具是事后取证 | ⚠️ **Rust 成立；CDP 一项部分过时** | Rust 先例：`plugin_market.rs:1608-2064` 共 19 条单测，含 `manifest_validates_permissions` `:1953`、`manifest_validates_reuse` `:1972`。前端：`app/package.json:6-13` **无 test 脚本**、`:25-31` **无 vitest**（全仓 JSON grep `vitest|jest|mocha` 零命中），`app/src` 无自有 `*.test.*`（唯一命中 `target/debug/temp/mcp-fixture.test.mjs` 是构建产物）。`scripts/agent-e2e.ps1` 存在；`temp/pet-e2e.mjs` **当前检出未查到**（gitignore）。 |
| 3.6 | 建议顺序：引脚 → 外观隔离（先落 vitest）→ sidecar Job Object 资源上限 | ⚠️ **方向成立，需修正两点** | ① `apiVersion` 文档**无需再修**（已正确）；② Job Object 的「随宿主退出回收」已存在，只剩内存/进程数上限是新增量。建议顺序改为：`provides` → `render`/`consumes`（声明+校验）→ 外观隔离（先定前端 runner）→ Job Object 限额。 |
| 4 | 不能脱离 Tauri + WebView2 | ✅ 仍成立 | 前端死绑：`market.ts:227-228`（`convertFileSrc` + 动态 `import()`，只有 asset 协议可加载）；`tauri.conf.json:35`（`script-src` 无 http/https，只有 `'self'` + asset）；`host.ts:64-69` 的 `globalThis.__lunac_host` 桥 + `apiVersion: 2`；插件碰 `127.0.0.1` 必须宿主代请求：`sidecar.ts:211-219`、`README.md:268-269`、`forbidden.md:22`。后端 sidecar 只是**进程形态**可独立：当前 `spawn_proc` 依赖 `AppHandle`（`plugin_sidecar.rs:317-323`）、事件 `emit_to`（`:523`），不能原样脱离宿主。 |
| A8 | 「宿主一条引用路径」仍成立 | ✅（有 2 条旁路） | 启动：`main.ts:2816` / `plugin-window.ts:182`；手动重扫：`settings.ts:1938`；AI 写 `Modules\` 后：`main.ts:1041-1050` 标记 → `:9974-9979` 广播 → `:12256-12257` 各窗重扫，另有唤出兜底 `:13684`。统一入口 = `market.ts:155-158` → `commands.rs:2751-2753` → `plugin_market.rs:1517-1585`。**旁路**（不经扫描、直接 `join(id)` 读清单）：`commands.rs:644-650`、`plugin_sidecar.rs:175-177`、`plugin_window.rs:280-286`。 |

**债 1 的宿主 id 硬编码全量清单（本轮 grep 结果）**

- **Rust 运行时**：`commands.rs:644-646`（按目录名找清单）、`:2958`（`install_plugin_dependencies` 直接 `join(id)`）；`plugin_sidecar.rs:171`（`plugins_dir().join(id)`）；`plugin_window.rs:281-283`（按 id 读窗口形态）。
- **前端运行时**：`main.ts:2825-2827`（`hasDiskPlugin` / `pluginSidecarRunning` / `plugin_sidecar_start` 均传 `"local-model"`）；`settings.ts:1302-1303`、`:2931`、`:2952`、`:2983-2991`；`kinds.ts:72-74`（`HEADLESS_PLUGIN_IDS`）。
- **构建期**：`vite.plugins.config.ts:47`、`scripts/build-plugins.ps1:294`（`local-model` 元数据）与 `:423-434`（清单字段白名单）、`scripts/publish-plugins.ps1:63 / :71`。
- **文档**：`README.md:57`。
- 说明：`storage.rs:121-155` 的 `config\local_model.json` 是**功能开关配置**，不是插件查找点，不纳入本债。
- 额外发现（不在债 1 范围，但同属「按 id 特判」）：`main.ts:10702`（`ai-agent`）、`:10706`（`music` 过渡兜底）、`:10714`（`pet`）。这些是 `forbidden.md:71` 已禁止的形态，建议后续用 `provides`/能力声明逐个收口。

#### H. 仍未决 / 需我拍板

| # | 事项 | 我的建议 | 为什么必须拍板 |
|---|---|---|---|
| 1 | ~~工具链装在哪~~ **✅ 已拍板（2026-10-17）** | **`<插件目录>\.toolchain\<profile>\`**；不做宿主共享 / 用户目录 / 全局 | 已定稿：卸载删目录即回收，守住「卸载 = 完全不存在」 |
| 2 | **iframe 内如何注入主题 token** | 握手后由宿主 `postMessage` 发快照；`inherit` 全量、`custom` 只发 `mode`（可选 token） | iframe 读不到宿主 CSSOM；发多了会把宿主 token 变成插件契约，发少了 `inherit` 失效 |
| 3 | ~~是否新增挂载锚点~~ **✅ 已拍板（2026-10-17）** | **新增 2 个**：`chat-sidebar`、`settings-section`（定义见 **C.4.1**）；**只加这两个**，再加需重新拍板 | 用户已选择「新增锚点」；缺口 = 宿主 DOM 契约与回归面变大，已在 C.4.1 写明施工注意（窄窗可折叠 / 无插件不占位 / 卸载不残留） |
| 4 | ~~iframe 的源模型~~ **✅ 已定（2026-10-18）** | 用**自定义 scheme** `lunac-plugin://localhost/<id>/ui/index.html`（Windows 实际 origin = `http://lunac-plugin.localhost`）。原因：asset 协议的 `Access-Control-Allow-Origin` 绑定窗口 origin，不透明源 / asset 源的 ESM 跨源加载不可靠；自定义 scheme 让 iframe 同源加载自己的 `ui/` 资源，同时与宿主跨源 | 决定 iframe 的 URL 形态、CSP `frame-src`、targetOrigin 与来源校验口径 |
| 5 | **工具链 profile / 版本白名单** | 先只做「单文件构建器」与「Node LTS + 包管理器」两档；版本与哈希写死 | 决定攻击面、磁盘占用与可复现性；无白名单等于允许任意 npm 包 |
| 6 | **工具链联网策略** | 构建期联网走宿主代理 + registry 白名单；不允许插件脚本自行直连 | 与 `forbidden.md` §禁用 10 的「HTTP 一律宿主代请求」对齐；否则工具链成为绕过口 |
| 7 | **市场插件是否允许 `dev` 段** | 允许声明，但**默认永不授权**；只对本地 / 用户自建插件提供「申请授权」 | 防止「装个市场插件顺带索要开发环境」；供应链风险显著高于本地自建 |
| 8 | **`reuse` 在 iframe 不可用** | 接受；`reuse` 只保留给 DOM 引擎，文档写明 | 它依赖与宿主同 DOM；强行支持等于给 iframe 开宿主 DOM 口子 |
| 9 | **`custom` 的可见提示放哪** | 插件市场详情 + 设置页插件条目；首次打开给一次轻提示；不放进结果区 | 放结果区会占布局 / 破坏视觉一致性；只放设置又可能不够可见 |
| 10 | **Job Object 缺省限额** | 内存与进程数都可配，缺省值需按 4B 模型 sidecar 实测后定 | 定小了会杀本地模型；定大了等于没闸 |
| 11 | ~~插件包 `sha256` 是否强制~~ **✅ 已拍板（2026-10-17）** | **强制**（索引新增字段 + 安装前校验）；**旧包迁移期只 warn、不拒装**，迁移期结束后转强制 | 现在插件包本体无哈希（`PluginIndexEntry` `:717-735`、`commands.rs:2894-2944`），无法证明交付物未被替换 |
| 12 | **CDP 夹具现状** | `temp/pet-e2e.mjs` 当前检出**未查到**（gitignore）；`scripts/agent-e2e.ps1` 存在 | 若后续要用 CDP 做端到端验收，需要先确认夹具是否仍在 |

### §待办 3-11 插件面板「图片化 / 可视化设计 ↔ 代码」可行性（2026-10-17 只读调研，未施工）

> 来源：用户提问「是否可以**直接通过前端的图片化可视化设计来与代码本身交互**」。本文只给结论与路线，**不构成施工任务**；与 **§待办 3-10**（引脚 / 单信道 / 芯片封装）配套阅读 —— 本文的 B 方案正是「IR 走那条信道」的载荷。

**一句话结论**
- **单向（图片 → 代码）：能，但只是「高概率可用的初稿」**，不是编译器。静态布局 / 文字 / 配色可用；**像素级间距、响应式、交互态**是明确短板。
- **双向（可视化 ↔ 任意代码）：不能「直接」做到。** 工程上只有两条路：**IR 为单一真相源**，或**代码为真相源 + 插桩 + 限制编辑范围**。前者适合 Lunac，后者不适合（详见 C）。

**A. 单向「图片 → 代码」的能力边界（有据）**

| 事实 | 出处 |
|---|---|
| Design2Code（截图→HTML/CSS）人工评估：GPT-4V 产物 **49%** 可替代原页面；细粒度上主要落后在「召回视觉元素」和「生成正确布局」，文字/配色可微调改善 | [项目页](https://salt-nlp.github.io/Design2Code/) · [arXiv 2403.03163](https://arxiv.org/abs/2403.03163) |
| **交互生成明显弱于整页生成**；十类失败模式；视觉上不明显的交互表现差（127 网页 / 374 交互 / 31 类交互） | [Interaction2Code, ASE 2025](https://arxiv.org/abs/2411.03292) |
| **「保真度—响应式悖论」**：视觉还原越高的模型越容易抄绝对坐标、产出非响应式代码（metadata trap） | [FigmaBench（OpenReview）](https://openreview.net/attachment?id=VCyvem1v0R&name=pdf) |
| 从零生成可交互站点：最好组合（Bolt.diy + DeepSeek-R1）**27.8%** 用例通过 | [WebGen-Bench, NeurIPS 2025](https://papers.nips.cc/paper_files/paper/2025/hash/6841eed8bb6a2ec49e49235c8115efee-Abstract-Datasets_and_Benchmarks_Track.html) |
| 成熟度分三档（可靠性递增，原因是**输入信息更多**不是模型更聪明）：截图 → 设计文件（变量/组件/布局）→ 设计系统（Code Connect） | [Figma MCP](https://help.figma.com/hc/en-us/articles/32132100833559-Guide-to-the-Dev-Mode-MCP-Server) · [Builder.io](https://www.builder.io/figma-to-code) |

⚠️ **未查到**：2026 年当代模型在同一口径（Design2Code / Interaction2Code）上的最新权威分数；现有数字基于 2024–2025 模型，**不能直接外推**。

**B. 双向为什么难（七类丢信息点）**：节点身份、格式/结构、样式语义、token 语义、动态行为、响应式、**真相源漂移**。
- **CSSOM 不可靠**：浏览器会规范化 shorthand、把颜色转 `rgba()`、重排值顺序；CSS 变量在 shorthand 里可能序列化成空串（[GrapesJS 官方警告](https://grapesjs.com/docs/guides/Custom-CSS-parser.html)）。
- **最直白的反例**：**Webflow 导出不能导回**（[官方](https://help.webflow.com/hc/en-us/articles/33961386739347-Code-export) · [社区](https://discourse.webflow.com/t/exporting-and-importing-website/38734/1)）；DevLink Export 也只承诺「Webflow → 代码」单向（[官方](https://help.webflow.com/hc/en-us/articles/44746282129427-Export-components-with-DevLink-Export)）。

**两条工程化路线**

| 路线 | 真相源 | 代表 | 对 Lunac 的适配 |
|---|---|---|---|
| **IR 为单一真相源** | 结构化 JSON（节点树） | Puck（`Data.content/root/zones`）、Retool（App JSON 可导出/导入）、Node-RED / n8n（Flow JSON） | ✅ **适配**：符合 §禁用 62；token 可强制；无需 `innerHTML` |
| **代码为真相源 + 插桩** | 源码 + AST | Onlook（宣称 code is source of truth，构建期注入标记；**只支持 React 仓库**） | ❌ **不适配**：Lunac 插件是单文件 ESM、无统一构建步骤、`type:"html"` 是字符串子语言、无 source map |

**C. Lunac 内的三个递进方案（推荐 A → B；C 不做）**

| | A 截图反馈（现在） | B IR 单一真相源（iframe 后） | C 代码双向（不做） |
|---|---|---|---|
| 真相源 | 代码（`index.js`） | **IR（`panel.json`）** | 代码 + AST |
| 双向 | 否（是「重新生成 + 人工复核」） | 是（IR ↔ 可视化/AI） | 是 |
| 表达力 | 任意 HTML/CSS | 节点白名单（v1 建议 10–12 个） | 任意代码 |
| 主题合规 | 靠提示 + lint | **渲染器强制**（IR 只存 token 名） | 难保证 |
| 新增运行时依赖 | 0 | 0 | 需 parser（与零依赖冲突） |
| 成本 | 1–2 人日 | 3–5 周 MVP（不做编辑器则 1.5–2 周） | 1–3 人月起，长期维护重 |

**A 的失败模式与缓解**（按概率）：抄绝对坐标 → 提示强制 flex/grid + 4/6/8/12、禁绝对定位；响应式没做 → 至少给 2–3 个宽度截图；交互态缺失 → 明确列出 hover/focus/disabled/空/错误态；**硬编码颜色** → 加 lint 禁 `#rrggbb`/`rgb()` 只许 `var(--…)`；引网络 import → 提示「单文件 ESM、零网络 import」；**跨域 iframe 是黑盒** → 用宿主级 OS 截图，不要试图读 iframe DOM（tldraw Make Real 同遇此问题，[原文](https://tldraw.substack.com/p/make-real-the-story-so-far)）。

**B 的 IR 骨架（要点）**
- 节点：`panel/row/column/grid/scroll/divider/spacer` + `text/heading/icon/image` + `button/input/textarea/toggle/slider/select/progress/list`。
- **颜色只允许 token 名**（`text` / `text-dim` / `text-muted` / `surface-glass` / `border-glass` / `accent` / `green` / `red` / `yellow` / `blue`）；间距只允许 `4/6/8/12`；按钮 `variant` 只允许 `action/neutral/danger`；**IR 里出现 hex/rgb 即构建失败**。
- 事件只写「事件 → 动作名」；`actions` 只允许 `set/toggle/increment/call/emit`；`call` 指向 `index.js` 的 handler，**IR 不嵌入可执行代码**；插值只允许 `{{state.x}}`。
- **单一真相源**：`panel.json` 管结构与样式；`index.js` 只留行为；`renderIR()` 是唯一 IR→DOM 实现；`styles.css` 仍是 token 值的唯一来源，**加一条测试：IR 用到的每个 token 必须在 `:root` 存在**。
- 与信道：`ir:render`（全量快照，MVP 够用）/ `state:patch` / `theme` / `resize` / `reload` + 插件侧 `action` / `bind` / `ready` / `error`；**IR 过信道而不是 HTML 字符串**（渲染用 `textContent`/DOM API，不 `innerHTML`）。
- 兼容：保留 `type:"html"`，新插件才走 `type:"ir"`，**不强制迁移**。

**D. 对 §待办 3-10 的两条直接影响**
1. **iframe 不只隔离 CSS/DOM，也能隔离脚本执行** —— 参照 Retool 把 JS 放**不同域独立 iframe**（[官方](https://docs.retool.com/3.300/queries/concepts/javascript)）；这为「把 `type:"html"` 从 `innerHTML` 迁移到 iframe」提供了产品级先例。
2. **B 方案的保存路径缺一个窄命令**：仓库**未查到**现成的 `write_file` / `write_text` 宿主命令 ⇒ 可视化编辑器「保存 `panel.json`」需要新增**窄范围**写入命令（路径白名单 + 原子写），或走 agent 写盘。

**E. 建议顺序**
1. **现在**：A（加「截图并问 AI」动作，提示词固定带 `Modules\README.md` + `docs/ui-style.md` + 当前 `index.js`；加生成后 lint）。
2. **iframe 信道落地后**：B（先 `panel.json` + 渲染器 + `type:"ir"`，**不先做可视化编辑器**）。
3. **再做**：可视化编辑器（树 + 属性面板 + 预览 + 保存）。
4. **C 不做**（除非将来插件改成 React/TS + 构建链，且宿主愿意维护 parser / 插桩 / source map）。

### §待办 3-12 三环境对照实验 v2：本地决策价值（2026-10-24 登记并执行；结果已回填）

> **执行手册 = [`cache-hit-experiment.md`](./cache-hit-experiment.md)**（任务书母版 / 前置自检 / 取证模板 / 回填表都在那里）。
> 本节只做**规范登记**；两处若不一致，**以执行手册为准**。

> **v1 结论作废（2026-10-24）**：平台 CSV 只有密钥维度、A 走 Trae 通道、B/C 共用 key，`TEST A/B/C`
> 合计只有 4 次探活请求；C 组 `LUNAC_LOCAL_DECIDE` 未设、本地模型零推理。⇒ 命中率不可归因、C 组未发生。
> v2 改用**本组测试密钥跑正文** + **本地模型真实探活门禁**，主指标换成未命中 token / 成本 / 完成度，
> 命中率降为辅助；任务书正文保持不变。

> **性质 = 实验协议，不是功能开发**。实验期间**不改本仓库任何文件**：`d:\cc\claude-code-cli-master` 只**读**（尤其 `docs\`），所有写操作只落在 `D:\test\<A|B|C>\`。下面的「任务书」可**原样粘贴**给三个环境 —— 这是公平性的前提。

**目标**：同一任务书（**人格提示词由执行者自配、三组一致，不写进任务书**）、各环境**新开一次对话**，比较三套环境的**每轮未命中输入 token / 估算成本 / 任务完成度**，并观察**一次上下文压缩**对任务连续性的影响。

**三环境（唯一允许的差异）**

| 组 | 环境 | 目录 | 正文密钥 | 关键设置 |
|---|---|---|---|---|
| **A** | Trae | `D:\test\A` | `TEST A`（通道允许时） | 平台默认；人格在 Trae 内置配置里设；通道不可替换则只作定性对照 |
| **B** | Lunac（本地决策**关**） | `D:\test\B` | `TEST B` | `LUNAC_LOCAL_DECIDE` **不设或 0**；`LUNAC_LOCAL_CURATE` 不设；`LUNAC_COMPACT_APPEND_ONLY` 不设 |
| **C** | Lunac（本地决策**开**） | `D:\test\C` | `TEST C` | `LUNAC_LOCAL_DECIDE=1`；其余与 B **逐字节相同**（含 `LUNAC_MAX_CONTEXT_TOKENS`） |

> **模型 / API**：三组统一 `deepseek-flash`，由执行者自配，**不写进本待办与执行手册**；端点 / 计费通道差异只作结论里的「已知偏差」。
> **三组目录各需一份 `TEST API.txt`**（执行者放入，agent 运行时读），任务书要求：按其说明调用、**不得把内容（尤其凭据）写进代码 / 日志 / report**，取证只记「是否调用 + 结果摘要」到 `report\test-api.txt`。

**前置自检（任一不满足 ⇒ 该组实验无效，重跑）**
- **正文密钥**：B/C 必须把 AI 配置换成 `TEST B` / `TEST C`，并在 `agent 凭据` 行核对 `key_tail=`；A 通道不可替换时如实记「不可替换」，只作定性对照。
- **B**：`LUNAC_LOCAL_DECIDE` 不设或 `0`，且 `agent-*.log` 里**没有** `检索查询改写` / `记忆候选抽取` / `安全预筛` / `能力路由` 这类本地判定行。
- **C**：`LUNAC_LOCAL_DECIDE=1`；宿主日志有 `本地模型就绪：…（model=…）`（或前端显示「已就绪」）；且确实出现上面那些判定行。
- 三组 `taskbook.md`（**不含人格**）**逐字节相同**：sha256 记进 `report\manifest.txt`。
- 三组都**新开一个对话**（空历史），不在已有会话里续跑。

**选定任务 —— 本仓库未完成项：P6 余项「elided 指针回注」**
> 为什么选它：真实未做（`docs/todo.md` §待办 3-6 补 P6 已如实记「重注入仍只在真丢消息时发生」）、**纯函数**、与上下文压缩强相关、不依赖宿主环境；同时逼模型先读规范再动手。

任务书（三组完全一致；**只许 std，不引外部 crate**，保证离线可 `cargo test`）：
1. 在 `D:\test\<组>\ptrblock\` 新建**独立 Rust crate**（不依赖本仓库任何 crate，也不放进本仓库）。
2. 实现：

```rust
pub struct Elided { pub tool: String, pub input: Vec<(String, String)> }

pub fn render_pointers(
    elided: &[Elided],
    recent_requests: &[String],
    budget_chars: usize,
) -> String
```

规格：
   - 只列**可重跑**工具（`Read` / `Grep` / `Glob` / `Cmd` / `PowerShell` / `WebFetch` / `WebSearch`），其余忽略；
   - 每条渲染成 `- Read src/main.rs (re-runnable)` / `- Grep "protected_reason" in src/ (re-runnable)`；
   - **去重**：同 `tool` + 同一关键入参只留一条；**稳定排序**：先按「最近诉求里出现该路径 / 关键词」降序，再按渲染文本字典序；
   - 首行固定 `[elided pointers]`；超 `budget_chars` 时按上述顺序截断，末行 `... (+N more)`；
   - **纯函数**：零网络、零磁盘、零全局状态；空输入 ⇒ 空串（不是 `[elided pointers]`）；
   - 渲染前先做**凭据打码**（`sk-…` / `Bearer …` / `key=value` 的值 → `***`），打码后的文本才参与长度计算。
3. 测试：`cargo test` **≥ 10 条**，必须覆盖去重、排序（近期命中优先）、预算截断与 `(+N more)`、空输入、非可重跑工具被忽略、CJK 路径 / 关键词、凭据打码、单条超长截断。
4. 交付：`cargo test` 全绿；`cargo clippy` 无 **error**（允许 warn）；`README.md` 写清接口与全部规则。
5. **禁止**：把规则写进任何 `system` 提示词；引入网络依赖；向 `d:\cc\claude-code-cli-master` 写任何东西（只读）。

**高强度上下文（同时是压缩触发器）**
任务书必须包含这一段：**先只读**通读下列 6 份，各写一条「本文档对本次交付的约束」到 `report\doc-checklist.md`，然后才允许写代码：
`d:\cc\claude-code-cli-master\docs\` 下的 `README.md`、`code-standards.md`、`forbidden.md`、`logging.md`、`testing.md`、`todo.md`（§待办 3-6 补 P6 的两节）。
> 实测体量（2026-10-24）：这 6 份合计 **3,139 行 / ~497 KB**，其中 `todo.md` 单份 **1,914 行 / ~380 KB**。⇒ 仅「读规范」就会吃掉大量 input token，是**触发压缩的主要手段**（不要靠只调小预算去凑）；若工具输出被截断，就分段 `Read` —— 分段本身也会增加轮次与 token，属预期。

**并行子 agent（硬要求）**
- 至少 **3 个并行子任务，且必须真正并发发起**（不是串行做完说成并行）：① 输入归一 + 去重；② 排序 + 预算截断 + 渲染；③ 测试与夹具。
- 记到 `report\subagents.txt`：子任务名、发起时间、返回摘要、各自改了哪些文件。失败也如实记，**不准编造「看起来并行」**。

**上下文压缩（硬要求：≥ 1 次）**
- 判定：Lunac 侧看 `agent-*.log` 的 `上下文压缩：…` 或前端 `context_compacted`；Trae 侧看会话内的压缩 / 截断提示，取不到日志就写「无法取证」。
- 若自然未触发：**B、C 各补跑一次**，在 `report\compaction.txt` 标为**「强制触发档」**，两组同设 `LUNAC_MAX_CONTEXT_TOKENS=48000`；A 无法设置 ⇒ 如实记「未强制」。**强制档与自然档分开统计，不混算。**
- 必须记：压缩发生在第几轮、压缩前后各一轮的 `in` / `cache_read`、压缩后模型是否还记得任务书规格（让它「复述『二、任务 第 3 条 a–i』」验证）。

**取证（每组 `report\` 下七件 + 一张截图，缺一不可）**
- `conversation.jsonl`：每轮 `{turn, role, summary, tools[], usage{in,out,cache_read,cache_creation}}`（拿不到就写 `null`，**不要编**）。
- `usage.csv`：表头 `turn,in,out,cache_read,cache_creation`，**B/C 以本地账为主口径**（`temp\usage\usage-*.jsonl` / 日志）；平台 CSV 只作交叉核对；A 取平台面板。
- `preflight.txt`：`key_tail` / `channel_replaced` / `decide` / `local_model_probe` / `decision_lines` / `max_context_tokens`（模板见执行手册 §2.3）。
- `compaction.txt`、`subagents.txt`（v2 增加 `role=` / `cap=` / `report_chars=`）、`manifest.txt`（`taskbook.md` + 全部产物的 sha256）、`test-api.txt`（是否调用 + 结果摘要，不含凭据）。
- `dashboard.png`（平台用量 / 成本读数，截图存证）。

**判据**
- **主指标** = ① 每轮未命中输入 token（`Σ cache_miss / 轮数`）；② 估算成本（miss / read / out 按平台单价）；③ 任务完成度（测试 ≥10 且全绿 / clippy 无 error / 规格 a–i 逐条满足）。
- 辅指标：缓存命中率（只在同环境内比较）、压缩前后 `cache_read` / `miss` 落差、子代理完成/触顶/partial/角色分布、`recall_check`。
- 结果回填到本节：

| 组 | 轮数 | 请求数 | Σ miss | Σ read | Σ out | 估算成本 | 命中率（辅） | 压缩次数 | 子 agent（完成/触顶） | 测试 |
|---|---|---|---|---|---|---|---|---|---|---|
| A | 1 | 48 | 224,156 | 1,488,640 | 66,180 | 0.519 元 | 86.9% | 自然 1（无法取证） | 3 / 0 | 15 全绿 |
| B | 1 | 81 | 127,593 | 2,078,464 | 96,184 | 0.554 元 | 94.2% | 自然 3 | 3 / 1 | 13 全绿 |
| C | 1 | 117 | 236,050 | 2,859,264 | 173,878 | 0.989 元 | 92.4% | 自然 3 | 无法取证 | 32 全绿 |

> 数字由 `temp\logs\agent-2026-10-24.log` 的 `请求用量` / `子代理/复盘用量并入` 行重算（B 窗口 `11:47:38Z–11:58:21Z`、C 窗口 `12:05:29Z–12:15:08Z`）；单价取 `config\pricing.json`（CNY，元/百万 token，周六基础价 `input=1 / cache_read=0.02 / output=4`）。A 通道不可替换 ⇒ 四列 `null`，仅定性对照。完整结论、取证口径与数据质量问题见执行手册 §6.1–§6.3。

**回填结论（2026-10-24）**

1. **完成度**：三组 `cargo test` 全绿（A 15 / B 13 / C 32）、`clippy` 无 error；C 缺 `README.md` 与产物 sha ⇒ `partial`。
2. **成本：A 0.519 / B 0.554 / C 0.989 元**；每请求 miss A 4,670 / B 1,575 / C 2,018；输出占成本 51% / 69.5% / 70.3% ⇒ 优化优先级 = **输出 > 请求数 > 前缀**。`n=1`，不作因果归因。
3. **本地决策确实在跑**（C 第二次启动后）：15 条安全预筛 + 4 条记忆候选抽取；B 窗口判定行 0。
4. **阻断点已定位**：C 第一次启动时「模型 ready 但 `decide=0`」——开关只拉起 `llama-server` 并下发端点，不下发 `LUNAC_LOCAL_DECIDE`，且该开关是 `OnceLock` 进程内缓存，运行中翻不动 ⇒ 只能手工设环境变量 + 重启。修复项见 **§待办 3-15**。

**已知偏差（写结论时必须带上）**
1. 各组端点的缓存实现 / 计费 / 命中口径不同（前缀 vs 块、是否含 system 等）⇒ 命中率**不能当严格 A/B 指标**。
2. A（Trae）拿不到 `usage-*.jsonl` / `api-dump.jsonl`，只有面板 ⇒ 取证粒度天然弱于 B/C；通道不可替换时只作定性对照。
3. B/C 的本地决策**不产生云调用**，但会改变压缩 / 检索等**上下文组织** ⇒ 未命中 token 差异不能全部归因于「本地决策」。
4. **人格提示词由产品配置注入**：三组只保证内容一致，实际拼装位置（前 / 后、单独 system / 合并）由各产品决定 ⇒ 固定前缀的起点可能不同。
5. **v1 教训**：`TEST A/B/C` 只被用于探活（1 / 2 / 1 次请求）、C 组 `LUNAC_LOCAL_DECIDE` 未设 ⇒ v2 的 §0.7 / §0.8 门禁必须逐条过。
6. 子代理预算已改为按价加权 + 角色，v1 的「触顶轮数 / partial 字数」不能与 v2 直接对比。

**边界（不做）**：不改本仓库；不把 `D:\test` 产物回灌仓库；不为「让压缩发生」而只调低 B/C 的预算（要调就 B/C 同值并单独标记）；A 侧取不到的指标不编造。

### §待办 3-21 S2 跨回合审批上下文：续跑时「最严者胜」（2026-10-31 已实现）

> **来源**：§待办 4 S2 一直挂着「跨回合**工具与审批**上下文」这条未做项。原实现只把审批口径
> 写进检查点（`checkpoint::ApprovalContext`）并在 `--resume` 时**对比告警**，理由是
> 「静默沿用 `--dangerously-skip-permissions` / `acceptEdits` 等于替用户放宽权限」。
> 但反方向一直是个洞：`--continue` / `--resume` 只要**没重传** `--permission-mode plan`，
> 续跑就会悄悄回到可写档 —— 用户上一轮明确按只读跑，这一轮凭一句「继续」就把写权限放开了。

- **规则（单向合并，只收紧、绝不放宽）**——唯一实现在 `checkpoint::ApprovalContext::tightest()`：
  - `permission_mode`：任一侧是 `plan` ⇒ `plan`（否则保留本次命令行给的值）；
  - `skip-permissions`：**两侧都为真**才生效（检查点那次没开，这次也不替你开）；
  - 审批卡 `ask_permission`：任一侧要求就要求；
  - `disallowedTools`：取**并集**（禁得更多 = 更严），且**只在真有新增时**才改写（不制造看不见的重排）；
  - `--add-dir`：取**交集**（少一个目录 = 更严）；**旧检查点没记目录（空）时不做交集** ——
    否则老会话续跑会突然「工具全都 Access denied」；
  - `cwd` 不参与合并（进程 cwd 由启动方决定）。
- **生效点只有一个**：`main()` 里恢复检查点之后调 `apply_approval_context(&mut cli, …)` 写回
  `CliArgs` —— `read_only` / 审批策略 / 工具池 / 工作区边界全部从这几个字段派生，写回即全链路生效，
  **不存在「某处忘了看新上下文」的第二种写法**。收紧项逐条写进 stderr + `agent-*.log`
  （`--resume 收紧口径：…`），不让它变成看不见的行为差异。
- **验证**：
  - 单测 3 条（`checkpoint.rs`）：`tightest_adopts_stricter_checkpoint_fields`（五类收紧 + 说明齐全）、
    `tightest_never_loosens_a_stricter_current_run`（本次更严 ⇒ 原样保留、零说明）、
    `tightest_keeps_current_dirs_when_the_checkpoint_recorded_none`（旧检查点不吃掉本次目录）。
  - `cargo test` **437 passed / 0 failed / 15 ignored**。
  - 命令行实跑（无 API key，只为取证前两行）：手写一份 `permission_mode=plan` 的检查点，再以
    `--print "hi" --permission-mode acceptEdits --disallowedTools Edit --resume sess_tight` 启动 ⇒
    `session=sess_tight`（确实续上）、stderr 打出
    `--resume 已按「最严者胜」收紧本次口径：permission-mode 沿用检查点的 plan（只读）；disallowedTools 并入 [Write]`，
    且启动日志的工具池里 `Write` / `Edit` 都已消失。
- **涉及文件**：`core-agent/src/checkpoint.rs`（`tightest` + 3 条单测）、`core-agent/src/main.rs`
  （`apply_approval_context` + 恢复段改写）。
- **未做**：GUI 上「续跑提示」的展示（现在只在 agent 日志 / stderr）；`--mcp-server` 等其余
  跨回合上下文仍未随检查点恢复。

### §待办 3-22 S3 子 agent 团队：常驻角色 + 任务 DAG（2026-10-31 由 §待办 4 S3 提升为正式条目；分片 1–5 已实现）

> **提升理由**：用户 2026-10-31 裁定「全部都升级成正式条目，然后开始任务」。§待办 4 的 S3 原文只有
> 一句话（「常驻角色 planner / executor / reviewer + 任务 DAG（需重开架构）」），这里拆成可独立验收的分片。
> **与 §待办 4 的关系**：方向登记仍在 §待办 4，**实施只认本节**。

- **总目标**：长任务能「拆成有依赖的步骤 → 分派给不同角色 → 各自汇报 → 汇总」，而不是现在这样
  「一个子代理跑完即散」。
- **本轮边界（不做）**：不引入常驻进程 / 后台守护；不做跨会话 DAG 持久化；`Agent` 的
  「同批并发」不变（`SUBAGENT_PARALLELISM = 3`）。**分片 4 起，具名角色的子代理会话可跨派发复用**
  —— 它不是常驻进程，只是把同一角色的对话接在下一轮前面（见分片 4）。
- **分片**
  1. ✅ **DAG 数据基座（2026-10-31 已实现）**：`TodoWrite` 条目新增 `id` / `blocked_by`；
     `todo_write` 做**四类确定性校验** —— 重复 id、`blocked_by` 指向不存在的 id、自依赖、依赖环
     （DFS 三色），**全部报错让模型自己改，绝不静默丢边**；随后按「前置全部 `completed`」
     算出 `Ready` / `Blocked` 两组回给模型。落点 `core-agent/src/tools.rs`（`todo_write` + schema 的
     `id` / `blocked_by` 两个可选字段）；单测 3 条；**不用依赖的老清单一字不改**（回执逐字节相同）。
  2. ✅ **就绪态进前端面板（2026-10-31 已实现）**：待办抽屉的每行行尾多一个小标签 ——
     `等前置`（还有前置未完成，开工会被后端拒；hover 列出缺哪几个 id）/ `可开始`（前置已完成、还是
     pending）。判定抽成纯函数 `app/src/todo-deps.ts` 的 `decorateTodos()`（可单测，前端不维护第二份
     状态、不新增 IPC），抽屉只负责画。**可开始**那条的左侧方块走主题强调色（`--accent-rgb`），
     不引新颜色、不引图标；文案 `agent.todo_blocked` / `agent.todo_ready` 五语言齐发。
  3. ✅ **就绪态成为硬约束（2026-10-31 已实现）**：`TodoWrite` 里若把某条标成 `in_progress`
     而它的 `blocked_by` 还没全部 `completed` ⇒ **直接报错**（列出缺哪几个前置），
     不再靠提示词自觉。**只拦 `in_progress`、不拦 `completed`**（已完成的那一步确实发生过；
     且「把前置改回 pending 重新规划」是正常操作，拦 completed 会把人锁死）。
     Ready / Blocked 回执也只在**未开工**的条目里列 —— `in_progress` 的已经在做，混进去就读不出
     「接下来该做哪条」。
     **与原计划的差异（如实记）**：原本写的是「`Agent` 入参加 `todo_id`」，实现时改成收在
     `TodoWrite` 自己的校验里 —— 因为 `dispatch_tool` 拿不到 `history`，要接就得把「最近一条
     TodoWrite」抄成一份进程内状态，那就成了第二真相源；而清单的唯一真相明确是**模型最近一条
     `tool_use`**。同一个目标（不许在阻塞条目上动手）用这条更短的路达到了。
     `Agent.todo_id` 若将来要做，前提是「把 history 快照显式传进 dispatch」这一个架构决定先定。
  4. ✅ **角色常驻（最小版，2026-10-31 已实现）**：同一具名角色（explore / coding / review /
     research）**复用上一段子代理对话** —— 新回合的 history = 旧会话 + 本轮 prompt。
     落点 `core-agent/src/subagent_session.rs`（新模块）：
     - 存储：`<检查点目录>\roles\<role>.json`，**与检查点同一个开关**（`--checkpoint` /
       `--resume` / `--continue`）；不开检查点的进程既不读也不写，行为与改造前逐字节一致；
     - 裁剪：按**回合**（一次派发 = 一个回合）整段丢弃最旧的，最多 `MAX_TURNS = 4` 个；
       单文件 `MAX_BYTES = 512 KiB`，连一个回合都放不下 ⇒ 放弃写（不截断、不写半份）；
     - 收尾：撞预算 / 撞轮数时 history 可能停在 `tool_result` 上 ⇒ 用本轮报告补一条
       `assistant` 再落盘；补不上就**拒绝落盘**（宁可下次不常驻，也不留接不上的文件）；
     - 失败处理：读不到 / 损坏 / 版本不符 ⇒ 记 warn 后按**新会话**继续（常驻是增强，不该让
       这次派发整个失败）；损坏文件保留为 `.bad` 再重开。
     `general` **不常驻**（它的语义就是「改造前的无角色」，临时任务互相串台只会污染）；
     fork 技能与后台复盘都不接常驻会话（原样传空切片）。
     修改：`run_subagent` 多一个 `resumed` 形参、`SubagentOutcome` 多带 `history`；
     `subagent_role::Role::is_resident()` 是「谁常驻」的单一真相源。
     **与原计划的差异（如实记）**：没有做成「`--resume` 到该子代理的检查点文件」那种复刻主会话
     的形态 —— 主检查点存的是 `history + 提问序号 + 预算`，子代理没有提问序号与预算续跑语义，
     硬套只会多一套没有消费者的字段。这里只保留**真正要复用的那一件**：对话本身。
     **边界**：常驻会话是**旧快照**（文件可能已被改过，工具结果里的代码是当时的版本），
     且可能提到当前已被 `--disallowedTools` 禁掉的工具 —— 它只影响模型「记得什么」，
     不会绕过任何闸：每次工具调用仍走同一套 `run_one_tool` 判定。
     **代价**：每次派发都会把旧会话一起发出去（4 回合 / 512 KiB 的上限就是给这条成本兜底；
     前缀未变的部分可命中 prompt cache）。
  5. ✅ **复核角色闭环（2026-10-31 已实现）**：reviewer 只读这条**本来就靠工具池**保证
     （`REVIEW_TOOLS` 没有 Write / Edit / NotebookEdit，单测 `subagent_role_defs_narrow_tool_pool_per_role`
     钉着）；缺的是**闭环**——报告交回主线程后没人知道「证据够不够、哪些没验」。
     本轮补的是**确定性回执**（`core-agent/src/subagent_role.rs` 的 `review_digest()` /
     `review_receipt()`）：
     - 报告格式写进 review 角色的提示词：必须给出 `file:line` 证据，并以 `## unverified`
       小节收尾（没有就写 `- none`），且**不许提内联补丁**（它改不了文件）；
     - `run_agent_tool` 在 `role == review` 时把回执放在报告**前面**：这一步没改任何文件 /
       有几行 `file:line` 证据 / 未验证条目（最多列 8 条，超出报总数）/ 最后一句明确
       「要不要返工由你决定（返工就派 `role=coding` 并带上这份 file:line 清单）」；
     - 报告里没有 `## unverified` 小节 ⇒ 回执写 `NOT DECLARED`，并提示按「全部未验证」处理；
       一行 `file:line` 都没有 ⇒ 同样显式警告。**不做语义判断**，也不自动返工。
     - 边界：回执是**格式判断不是内容判断**（`src/a.rs:12` 这种 token 就算证据，不校验真假）；
       回执本身约 300 字符，加在角色报告上限之外（review 的 2k 仍是报告本身的线）。
     **本节到此收口**：分片 1–5 全部实现；「多角色并行 DAG 调度」若要做，另开条目（前提是先定
     「谁维护清单的写权」——目前清单真相是模型最近一条 `TodoWrite`，多线程同时写会打架）。
- **验收判据**：① 一条三层依赖的清单，模型若在 `Blocked` 条目上开工会被确定性拒（已由单测覆盖）；
  ② 同一子代理第二次分派能接上上一次上下文（**已由分片 4 覆盖**：`subagent_session` 的回合
  读回 / 超限裁剪 / 残缺跳过 / 报告补收尾 6 条单测 + `subagent_seed_history` 顺序 1 条）；
  ③ 复核报告带确定性回执、主线程能看到「证据数 / 未验证项 / 由你决定」；
  ④ 所有失败路径都有可读原因，不静默。
- **验证（分片 1–5）**：`cargo test todo` **5 passed**（新增 2 条：前置未完成时标 in_progress 被拒 +
  前置完成即放行且 in_progress 不进 Ready/Blocked；已完成条目不被「前置被重开」牵连）；
  `cargo test subagent` **25 passed**（其中 `subagent_session` 7 条、`Role::is_resident` 1 条、
  `subagent_seed_history` 1 条）；`cargo test subagent_role` **11 passed**（新增复核回执 5 条：
  证据与未验证计数 / 显式 none / 缺小节 / 截断到 8 条 / 遇下一个标题即止）；
  全量 `cargo test` **456 passed / 0 failed / 15 ignored**、`cargo build --release` exit 0；
  前端 `npx tsc --noEmit` exit 0、`npm test` **21 passed**（新增 `todo-deps.test.ts` 4 条：
  ready/blocked 拆分、completed / in_progress 不算 ready、未知前置按未完成算、脏输入不崩）。

### §待办 3-23 S4 持续自治 + IDE 附着（2026-10-31 由 §待办 4 S4 提升为正式条目；分片 1–3 已实现）

> **提升理由**：同上。方向登记仍在 §待办 4，**实施只认本节**。

- **现状（改造前）**：`vscode-extension/` 已是薄客户端（`chatView.ts` 走宿主 HTTP / SSE，不另开协议）；
  宿主是 cron 30s tick + 任务队列 worker 两个常转循环。缺的是「**没有任务时休眠、有任务即刻唤醒**」。
- **分片**
  1. ✅ **IDE 最小闭环（2026-10-31 已实现）**：编辑器 / 资源管理器右键 →「Lunac: Ask About
     This File」（`vscode-extension/package.json` 的 `lunac.askAboutFile` + 两个 `menus` 入口）。
     命令只拼**路径 + 行范围**（`vscode.workspace.asRelativePath`；有选区就带 `第 X–Y 行`），
     **不贴文件内容** —— agent 自己有 `Read`，贴一遍等于同一段代码付两次钱，而且贴的是编辑器
     里的旧文本。发送走**现有通道**：`chatView.ts` 的 `askInChat()` → 与输入框同一条
     `{type:"user", message:{…}}`；没启动就先走面板「启动」同一条 `start()`；回显用 webview
     早已处理的入站 `{type:"user"}`。**没有新增任何消息类型**。
  2. ✅ **后台休眠 / 唤醒判据（2026-10-31 已实现）**：新增宿主模块
     `app/src-tauri/src/wake.rs`，`next_wait_ms(now, next_due_at, max_wait)` 是「下一觉睡多久」的
     **唯一**算法（`Due` → 0 / `Wait` → 差值封顶 / `Idle` → 睡满上限）。cron 调度线程不再每 30s
     醒一次：`tick()` 返回「这一轮真的发起几次执行」，随后按最近一个启用 job 的 `nextRunAt`
     睡到那一刻（`next_due_at()` 只读，不修 state）；上限 `MAX_WAIT_MS = 60s` **由
     `scheduler_alive()` 的 90s 心跳窗口决定**（心跳只在醒来时写，超了会被前端误判成死）。
     tick 之后若仍「已到期」⇒ 退 `MIN_RETRY_MS = 1s`，绝不空转；分块睡 `SLEEP_CHUNK_MS = 5s`
     只为 `stop()` 响应，块内零 I/O。唤醒判据变化时落一行
     `cron 唤醒判据：reason=due|wait|idle 等待=…ms`。
     **任务队列的 3s 轮询保持不变**（如实记原因）：`queue.json` 是跨进程通道（agent 侧
     `TaskEnqueue` 直接写盘，宿主收不到通知），且 `testing.md` §测试 20 的验收口径就是
     「3s 内 pending→running」；这里改成「空转时零副作用」（不写状态 / 不写日志 / 不登记）。
  3. ✅ **唤醒源登记表（2026-10-31 已实现）**：`wake::WakeSource` = `Timer` / `QueueFile` /
     `IdeRequest` 三类，**只有这三个**（加第四类必须改枚举，不许在调用点拼字符串）；
     `wake::note(source, detail)` 写一行结构化日志 `wake_source=… detail=…` 并计数。
     **只在真的干活时登记**：cron 真的发起执行（`Timer`）、worker 拿到 pending 条目
     （`QueueFile`）、宿主 HTTP 桥收到 `/agent/start` 或 `/agent/message`（`IdeRequest`）。
     空转到点不登记 —— 否则日志本身就成了新的常转源。前端**不参与**这件事。
- **边界（不做）**：不做云端常驻；不新增守护进程（与「便携式、双击即用」冲突）；
  不把 IDE 扩展做成第二套 agent（它只能是同一引擎的薄客户端）；不为唤醒源新增 IPC ——
  登记表目前只落宿主日志与进程内计数（`wake::counts()`），前端读不到也不读。
- **前提**：§待办 3-22 分片 1–3 完成后再动 —— 否则「唤醒谁、干活的对象是谁」没有定义（已达成）。
- **验证**：宿主 `cargo test` **351 passed / 0 failed / 2 ignored**（新增 `wake` 5 条：到期立即醒 /
  未来封顶 / 无事睡满 / 三类来源名稳定 / 计数不串）；`cargo check` 无新增警告；
  `vscode-extension` `npm run compile` exit 0。真机步骤见 `testing.md` §测试 26。

### §待办 3-24 记忆层 wiki 化（B1，2026-10-31 由 §待办 4 补 B1 提升为正式条目；前置裁定已定，四个分片全部实现）

> **提升理由**：同上。方向登记仍在 §待办 4 补 B1，**实施只认本节**。

- **前置裁定（已定，动手前不再讨论）**
  1. **真相源**：`ModuleData\memory\MEMORY.md` 仍是**唯一**记忆正文；wiki 化 = **同一份存储的多页形态**，
     不是第二套知识库 ⇒ **不引第三方 llmwiki 实现**（PyPI / npm 那些都不接）。
  2. **层归属**（沿用 §待办 3-1a）：`AGENTS.md` = **指令**，进固定前缀；记忆（含未来的页）= **事实**，
     进可变层（`context_block_message`）。两者**不混**。
  3. **冲突时指令优先**：页里的事实与 `AGENTS.md` 的规则冲突，以 `AGENTS.md` 为准；**不自动改写指令**。
  4. **缓存纪律不变**：记忆快照仍**只在压缩边界刷新**（`forbidden.md` §禁用 45）；页的增减不改这条。
- **分片**
  1. ✅ **页维度（写入侧，2026-10-31 已实现）**：`Remember` 增加可选 `page`
     （`[a-z0-9][a-z0-9_-]{0,31}` 且不能以 `-` / `_` 开头），落
     `<记忆目录>\pages\<page>.md`；缺省 = `MEMORY.md`（**字节级兼容**）。页名的形状**只在宿主
     `parse_memory_page` 校验一次**（带建议 slug，非法不静默改写），agent 侧只透传；
     带页写入的回执追加「（页：xxx）」，模型下一轮知道这条不在默认页。
  2. ✅ **页维度（读取侧，2026-10-31 已实现）**：`memory_read` 把默认页 + 各页按名字序拼成
     一段（每页前一行 `## <page>`，空页跳过）；**没有任何页时逐字节等于旧文件内容**（兼容性由单测钉死）。
     **拼接有总长硬顶**（与写侧同一个 9000）：超出的页不拼，末尾留一行
     `[N more memory page(s) omitted …]` —— 页一多也不会把注入体积顶爆，且**不静默丢**。
     「清空长期记忆」连页一起清（默认页截断 + 页文件删除，幂等）。
     **边界如实记**：设置窗的「成长笔记」列表 / 逐条删仍只作用于默认页（页是模型侧的整理维度，
     等分片 3–4 落地后再决定是否按页分组展示）。
  3. ✅ **互链（2026-10-31 已实现）**：页内写 `[[other-page]]`，注入时在该页正文后追加一行
     `→ refs: a, b`（**只列真实存在的页**、去重保序、最多 8 个；指向不存在页的 `[[x]]` 原样留在
     正文里，当作「这一页还没写」的记号）。**不递归展开** —— 递归会让注入体积随引用图膨胀，
     而这个功能只需要回答「去别的哪一页找」。
  4. ✅ **自愈 / 去矛盾（2026-10-31 已实现）**：`REVIEW_SYSTEM`（复盘 fork 自己的系统提示词，
     与主对话固定前缀无关）补两条硬要求 —— ① 事实属于某页时就带 `page` 写进那一页；
     ② **发现与已有记忆冲突时不许两条并存**，用**一次** `Remember` + `replace: true`（同页）
     重写成唯一结论。落盘前的确定性去重（同正文 + 同 scope）此前已有（`write_memory_at` 的
     `normalize_entry`），本轮未改。
- **验证（分片 1–4）**：宿主 `cargo test --bins memory` **23 passed** + `cargo test --bins page`
  **2 passed**（合计 25 条记忆页相关用例；新增 7 条：页名校验 + 建议值、写页不动默认页、
  读取按名字序拼接且无页时逐字节等于旧文件、拼接到 9000 硬顶就停并显式说明、清空连页一起清 + 幂等、
  互链索引且不递归、引用去重 / 上限 8 / 非法名忽略）；`cargo check --bins` exit 0；
  core-agent `cargo test` **440 passed / 0 failed / 15 ignored** 与 `cargo build --release` exit 0
  （`Remember` schema 多了 `page`，桥参数 `lunac/memory_write` 多传一个可选字段）。
- **验收判据**：① 不指定 `page` 时 `MEMORY.md` 的读写与今天**逐字节一致**；② 两页互链可读；
  ③ 总长超 9000 仍**报错不静默截断**；④ 注入快照的刷新时机不变（只在压缩边界）。

### §待办 3-25 自研代理引擎（SS / SS-2022 / Trojan / VLESS / VMess / AnyTLS / Hysteria2 + 规则，2026-10-31；用户裁决「C」，八轮落地）

> 来源：用户 2026-10-31 提问「代理的引擎是否可以自己写」，在 A（只补分享链接）/ B（抽
> `ProxyEngine` trait）/ C（自研最小内核）中选了 **C**。动机不是功能，而是**许可与自足**：
> mihomo 是 GPL-3.0，只能作为插件依赖下载，首次启用必须联网装内核；自研编进宿主就没有这两条。

**已实现（`app/src-tauri/src/native_proxy/`，21 个文件）——逐项证据见 [`acceptance-map.md`](./acceptance-map.md)**

| 能力 | 实现 | 状态 |
|---|---|---|
| 入站 SOCKS5（CONNECT + UDP ASSOCIATE）/ HTTP（CONNECT + 绝对 URI） | `server.rs`、`udp.rs` | ✅ |
| SS AEAD（aes-128/256-gcm、chacha20）+ SS-2022（三种 cipher，含多用户 `iPSK:uPSK`） | `ss.rs` | ✅ |
| SS 的 SIP003 插件：simple-obfs（`http` / `tls`）+ v2ray-plugin（`websocket`） | `ss_plugin.rs` | ✅ |
| Trojan（TLS 走 SChannel） | `trojan.rs` | ✅ |
| VLESS（Vision 未做；REALITY 走 `reality.rs`） | `vless.rs` | ✅ |
| VMess AEAD（alterId=0，AES-128-GCM + ChunkStream） | `vmess.rs` | ✅ |
| AnyTLS（单 Stream、无填充策略） | `anytls.rs` | ✅ |
| Hysteria2（QUIC + HTTP/3；`salamander` / `gecko` 混淆；TCP + UDP） | `hysteria2.rs`、`hysteria2_udp.rs` | ✅ |
| 传输 TCP / TLS / WebSocket / **REALITY**（自带 TLS 1.3） | `transport.rs`、`reality.rs` | ✅ |
| 规则 `DOMAIN*` / `IP-CIDR*` / `MATCH` + 本机私网直连 | `rules.rs` | ✅ |
| 订阅：Clash YAML / base64 / 分享链接（含 `vmess://JSON`） | `clash.rs` | ✅ |
| 生命周期 start / stop / status / select / delay + 端口占用回写 | `mod.rs` | ✅ |
| 与 mihomo 的 `engine` 分派（native 可切换） | `mihomo.rs` | ✅（**缺省已为 native**，见 §待办 3-26 K1/K2） |

**验证**：`cargo check --bin lunac` exit 0；`cargo test --bin lunac native_proxy` **111 passed / 2 ignored**（2026-11-03）。
真机验收清单见 `docs/testing.md` §测试 27（第 1–16 条）。

**第九轮（2026-11-02）：VLESS + REALITY 落地**

- 新增 `reality.rs`：自写最小 TLS 1.3 客户端（X25519 key_share + AES-128/256-GCM /
  ChaCha20-Poly1305 记录层、HKDF-Expand-Label、Finished 校验），并在 ClientHello 的
  `legacy_session_id` 里做 REALITY 标签：
  `AuthKey = HKDF-SHA256(X25519(eph, server_pk), salt=Random[:20], info="REALITY")`，
  `SessionId = AES-256-GCM(AuthKey, nonce=Random[20:32], AAD=SessionId 全零的 ClientHello,
  plaintext=版本[4]+时间戳[4]+shortId[8])`；服务端证书必须满足
  `HMAC-SHA512(AuthKey, ed25519_pk) == signature`，否则断开且**不发送 VLESS 头**。
- **X25519 自带实现**：ring 的 `agree_ephemeral` 会消耗私钥，而 REALITY 要用同一个临时私钥
  做两次协商（AuthKey + TLS key_share），故按 TweetNaCl 写了一份 radix-2^16 的标量乘法
  （RFC 7748 向量已单测），没有新增第三方依赖。
- `clash.rs`：YAML `reality-opts.public-key` / `short-id` 与链接 `pbk` / `sid` 都能解析；
  REALITY 只接受 TCP（`ws + reality` 跳过）。
- **Vision（同日补齐）**：`flow: xtls-rprx-vision` 会写进 VLESS 头的 protobuf addons
  （`0A 10 "xtls-rprx-vision"`）。上行首段包成
  `[uuid:16][cmd=End][clen][plen][content][padding]` 后转裸流；下行按同一格式解填充
  （`cmd=0` 续帧，`1/2` 结束并转裸流）。这既解决「Xray 服务端在 `account.flow=vision`
  时拒绝空 flow 请求」（`inbound.go`：`is not able to use ""`），也满足服务端对
  首包必须是 Vision 帧的期望。
- **VLESS 修复（同日）**：`connect` 改为「请求头与首段业务数据一起发、响应头推迟到
  首次读」—— Vision 服务端要拿到首段数据才回响应头，提前阻塞会死等；地址类型改用
  Xray 的 `protocol.AddressType`（域名=2，不是 SOCKS5 的 3）；对端 `close_notify`
  按 EOF 处理。
- 面板 `transport_label()` 对 REALITY 节点显示 `reality`。
- 单测：`vless_reality_link_reads_pbk_and_sid`、`vless_vision_is_sent_as_plain_vless`、
  `clash_yaml_reality_reads_public_key_and_short_id`、`x25519_matches_rfc7748_vector`、
  `x25519_base_point_matches_rfc7748_public_key`、`reality_seal_uses_zeroed_session_id_as_aad`、
  `rfc8448_handshake_secrets_match`；Vision：`vision_header_carries_flow_addons`、
  `vision_reader_unpads_frames_then_passes_raw`、`vision_reader_passes_stream_without_uuid_prefix`。
- **真机已验证（2026-11-02）**：`live_node_fetches_generate_204` 对
  `us1.oh-node.com:443`（VLESS + REALITY + `flow: xtls-rprx-vision`）经节点取到
  HTTP 204；与官方 Xray 26.10.31 客户端同时对照通过。

**第十轮（2026-11-02）：UDP 上游（VLESS / Trojan / VMess / AnyTLS）**

- 新增 `vless_udp.rs`：Vision 节点不能直接发 `cmd=UDP`（服务端
  `inbound.go`：`flow doesn't support UDP`），要按 Xray 客户端的行为改写成
  **Mux**（`v1.mux.cool`）并用 **XUDP** 封装，一条连接承载所有目标。
- XUDP 帧：`[meta_len:2][session:2][status:1][opt:1][network:1][port:2][addr][global:8?][data_len:2][data]`；
  首包 `status=New(1)` 带 8 字节 GlobalID，之后 `status=Keep(2)`；`status=4` 是保活包。
- 两个真 bug：①`cmd=Mux` 的 VLESS 头**不带端口和地址**（对齐 `EncodeRequestHeader`），
  多写会被服务端当业务数据；②XUDP 的地址是 **端口在前**（`protocol.PortThenAddress`），
  不是 SOCKS5 的「地址在前」。
- 新增 `trojan_udp.rs`：`cmd=0x03` 建关联，每包 `[ATYP][addr][port][len:2][CRLF][payload]`
  （包自带目标地址，一条连接可承载多个目标）。
- 新增 `vmess_udp.rs`：`cmd=0x02`，chunk 流里每包 `[len:2][payload]`；每连接一个目标，
  按目标懒建。
- 新增 `anytls_udp.rs`：按 sing-box **udp-over-tcp v2** —— Stream 目标为
  `sp.v2.udp-over-tcp.arpa`，首包 `isConnect(1) + SOCKS5 地址`，之后每包 `[len:2][payload]`，
  按目标懒建。
- `udp.rs` 接入：`NodeKind` 的 Vless / Trojan / Vmess / Anytls 懒建上游，回包按目标地址
  回填 SOCKS5 数据报；Hysteria2 的 UDP 归入 G6（需 QUIC datagram + UDPMessage 分片）。
- 单测：`xudp_first_frame_matches_xray_layout`、`xudp_reader_splits_back_to_back_frames`、
  `xudp_reader_skips_discard_and_short_frames`、`mux_header_omits_port_and_address`、
  `udp_frame_matches_trojan_spec`、`udp_reader_splits_back_to_back_frames`、
   `udp_reader_skips_junk_and_bad_crlf`、`udp_head_uses_command_three`、
   `decoder_splits_back_to_back_frames`、`decoder_waits_for_partial_frame`、
   `uot_v2_request_uses_connect_and_socks_address`；
   真机：`live_node_answers_dns_over_udp`（经节点查 `1.1.1.1:53` 收到同事务 ID 的回包）。

**第十一轮（2026-11-03）：传输层 `http` / `h2` / `grpc`（G3 第一段）**

- 新增 `http_obfs.rs`：`network: http` 的混淆握手 —— `PUT <path> HTTP/1.1` + `Host` +
  自定义头，读到 `200` 后就是裸流（没有分帧，和 WS 不是一回事）。
- 新增 `h2tunnel.rs`：`network: h2` 与 `network: grpc` 共用 HTTP/2 连接。
  * `h2`：`POST <scheme>://<authority><path>`，请求体 / 响应体就是裸流；
  * `grpc`：路径 `/<serviceName>/Tun`（空 = `GunService`，`/` 开头按完整路径取 `|` 前段），
    每条消息 = `[0][len u32 BE][0x0A][varint dataLen][data]`（gRPC 头 + `Hunk{bytes data=1}`）；
  * 阻塞中继 ↔ 异步 h2 的桥沿用 Hysteria2 的线程 + 双通道做法；
  * **不阻塞等响应头**：gRPC 服务端要等第一次写才发头，先等会互相等死。
- 两个实现细节（都是容易踩的坑）：①h2 的 `poll_capacity` 只在**容量增加**时就绪，
  初始窗口要先看 `capacity()` 再用；②测试里的 h2 服务端必须把每个请求丢进独立任务，
  `accept()` 停轮询会让连接自己把自己锁死。
- `clash.rs` 放开 `network` 白名单（YAML `http-opts` / `h2-opts` / `grpc-opts`；链接
  `type=http|h2|grpc`、`serviceName`、`authority`；vmess JSON 的 `net` / `path` / `host`）。
  `xhttp` / `kcp` 仍在解析阶段跳过（`xhttp` 需要 SplitHTTP 会话层，`kcp` 需要完整可靠传输）。
- 单测：`http_obfs_handshake_and_echo`、`http_obfs_rejects_non_200`、
  `h2_transport_echoes_through_local_server`、`grpc_transport_wraps_hunk_and_echoes`、
  `grpc_path_follows_xray_service_name_rules`、`grpc_frame_matches_hunk_layout`、
  `grpc_decoder_waits_for_partial_frames`、`yaml_parses_http_h2_and_grpc_transports`、
  `links_parse_h2_http_and_grpc_transports`。
  真实节点尚未验收（手上没有 h2 / grpc 节点），状态保持 🟡。

**第十一轮续（2026-11-03）：XHTTP（SplitHTTP）`stream-one` / `stream-up`**

- 按 Xray 真实客户端（`splithttp/dialer.go`、`client.go`、`config.go`、`xpadding.go`）
  对齐线上格式：
  * `stream-one`：`POST <path>/`，请求体上传、响应体下载；
  * `stream-up`：`POST <path>/<session>` 上传 + `GET <path>/<session>` 下载（同一 h2 连接）；
  * session 是带连字符的 UUID v4；路径规范成以 `/` 结尾；
  * **每个请求都要带 padding**：默认 `Referer: <base>?x_padding=XXX…`，长度 100–1000，
    服务端 `IsPaddingValid` 会校验，不发直接 `400`；
  * `Content-Type: application/grpc`（Xray 客户端的默认行为）。
- `packet-up`（每个 seq 一条 POST）**未实现**：订阅里写了 `packet-up` 时按 `stream-one` 发，
  服务端默认配置（`mode` 为空）同时接受这三种模式，失败会如实报 HTTP 状态码。
- 一个实现坑：连接 future 必须与收发一起被轮询；用 `tokio::spawn` 驱动时出现过
  「HEADERS 发了、DATA 一直不刷」。现在把 `Connection` 放进 `select!` 里一起推进。
- 单测：`xhttp_stream_one_echoes_with_padding`、`xhttp_stream_up_splits_upload_and_download`
  （本地 h2 服务端模拟 Xray：校验 padding、路径带 session、上传先回 200 再读流）、
  `xhttp_parts_normalizes_trailing_slash_and_query`、
  `xhttp_padding_referer_carries_100_to_1000_marks`。

**第十二轮（2026-11-03）：Hysteria2 `gecko` 混淆 + UDP 转发（G6）**

- 线上格式逐条对齐 `apernet/hysteria` 的 `extras/obfs/gecko*.go` 与 `PROTOCOL.md`：
  * **Salamander** 仍是底座：每个 QUIC 数据报都套 `[salt 8B][payload XOR BLAKE2b-256(key||salt)]`；
  * **Gecko** 在其上，只切 **QUIC 长头（握手）包**（首字节 `& 0x80`），切成 2–8 片，
    每片补齐到 `[min, max]`（缺省 512 / 1200）；Salamander 解出的帧格式为
    `[0x80][msgID:1][chunkIdx:4|totalChunks:4][padLen:2 BE][pad…][chunk…]`，
    短头（数据）包原样通过；
  * **UDP** 走 QUIC **不可靠数据报**（RFC 9221），`UDPMessage` 全部大端：
    `[session u32][packet u16][fragId u8][fragCount u8][varint addrLen][addr "host:port"][payload]`；
    `fragCount=1` = 未分片；丢任一片整包丢弃（重组有 TTL 与上限）。
  分片数受 `fragCount: u8` 限制：载荷过大时放大每片，保证不超过 255 片。
- 认证响应里的 `Hysteria-UDP: false` ⇒ **直接报错**，不假装能转发；`Hysteria-CC-RX` 仍恒发 `0`。
- 抽出可复用入口：`hysteria2::open_connection()` / `authenticate()`（TCP 与 UDP 共用同一套
  TLS / 混淆 socket / h3 认证），并把 UDP 数据报收发缓冲开到 1 MiB。
- `clash.rs`：YAML `obfs: salamander|gecko` + `obfs-password`，gecko 另读
  `obfs-min-packet-size` / `obfs-max-packet-size`（缺省 512 / 1200；`min > max` 或
  `max > 2048` 视为非法）；链接 `?obfs=…&obfs-password=…` 同规则。
  **obfs 写了却解析不出来 ⇒ 整个节点跳过**，绝不悄悄退回「无混淆」。
- `udp.rs`：新增 Hysteria2 上游（懒建、回包按目标地址回填 SOCKS5 数据报），
  原先「Hysteria2 不支持 UDP」的兜底提示删除。
- `Brutal` 拥塞控制**不做**：quinn 不开放可插拔 CC；`Hysteria-CC-RX` 恒发 0，
  服务端按自己的 CC 自适应，客户端发送速率交给 quinn 默认 CC。
- 单测：`gecko_frame_round_trips_and_pads_to_packet_bounds`、`gecko_rejects_malformed_frames`、
  `obfs_spec_maps_defaults_and_keys`、`message_layout_matches_hysteria_spec`、
  `large_payload_is_fragmented_and_reassembled`、`duplicate_fragment_is_dropped`、
  `authority_parses_ipv4_ipv6_and_domain`；`clash.rs` 的 gecko / salamander / 非法 obfs
  解析断言同步放宽。**真机 gecko / UDP 待补**（手上无对应节点），状态保持 🟡。

**第十三轮（2026-11-03）：SS 的 SIP003 插件（G4）**

- 新增 `ss_plugin.rs`，支持三种现实里最常见的形态（格式逐条对齐 `shadowsocks/simple-obfs`
  的 `src/obfs_http.c` / `src/obfs_tls.c`）：
  * **simple-obfs `obfs=http`**：首包前写一段 WebSocket 升级请求
    （`GET / HTTP/1.1` + `Host` + `Upgrade: websocket` + `Sec-WebSocket-Key` +
    `Content-Length: <首包长度>`），**之后就是裸流**；读方向先剥掉响应头（扫到 `\r\n\r\n`）。
  * **simple-obfs `obfs=tls`**：首包是一个假 ClientHello（字段偏移与 packed 结构体逐字节一致），
    **首个载荷藏在 `session_ticket` 扩展里**；之后每包 `[0x17 0x03 0x03][len u16][payload]`；
    读方向先消费 107 字节服务端头（96 ServerHello + 6 CCS + 5 encrypted handshake），
    其中 `msg_len` 就是首个载荷长度。
  * **v2ray-plugin `mode=websocket`**：标准 WebSocket + 可选 TLS，直接复用 `transport.rs`。
- `clash.rs`：YAML `plugin` + `plugin-opts`（`mode` / `host` / `path` / `tls`）与 SS 链接的
  `?plugin=obfs-local;obfs=http;obfs-host=…` 都能解析；**不认识的插件 / 模式在解析阶段跳过**
  （v2ray-plugin 的 `quic` 明确不做），不会悄悄退回裸 SS。
- `ss.rs`：`SsTunnel` 的底层流从 `TcpStream` 改成 `Box<dyn Tunnel>`，这样插件层可以插在
  「TCP」与「SS 的 salt / 地址头」之间；`connect` 多一个 `plugin` 参数。
- `udp.rs`：SS 插件只承载 TCP，UDP 直接记一行「不转发」而不是硬套插件。
- 单测：`parse_obfs_and_v2ray_plugin_specs`、`obfs_http_tunnel_prepends_upgrade_request`、
  `obfs_tls_client_hello_matches_simple_obfs_layout`、
  `obfs_tls_tunnel_round_trips_through_mock_server`、`tls_app_frame_carries_five_byte_header`，
  以及 `clash::tests::ss_link_and_yaml_parse_plugins`。
  **真机 204 待补**（手上没有带插件的 SS 节点），状态保持 🟡。

**第十四轮（2026-11-03）：SS-2022 多用户 PSK（G10）+ 经节点真实测速（G14）**

- **G10（SIP023 identity header）**：密码写成 `iPSK:uPSK`（多级继续用 `:` 串）。
  * salt 之后、第一个 AEAD 块之前，按顺序插入每层 **16 字节** identity header：
    `identity_subkey = blake3::derive_key("shadowsocks 2022 identity subkey", iPSK || salt)`，
    明文 = `blake3::hash(next_psk)[..16]`，用 AES-128 / AES-256（随 PSK 长度）做单块加密；
  * 会话子密钥仍由**最后一个键（uPSK）**派生，请求与响应方向一致；
  * 顺手把 salt + identity + 固定头 + 变长头改成**一次 write**（SIP022 §3.1.4 的硬要求）。
- **G14（真实测速）**：`delay()` 从「到节点服务器的 TCP 建连耗时」改为**经节点**发
  `HEAD http://www.gstatic.com/generate_204`，量到第一个响应字节；失败如实报错，
  **不回退 TCP 建连**（那会让数字偏乐观）。`server::dial()` 抽出 `dial_node()` 供两者复用。
- 单测：`psk_decoding_handles_single_and_multi_user`、
  `multi_user_psk_sends_identity_header_and_uses_last_key`（mock 服务端用 AES **解密**
  identity header，核对它等于 uPSK 的 hash 前 16 字节，再用 uPSK 走完整 2022 握手）。
- **真机待补**：多用户节点 204；面板延迟与 mihomo 对照（手上无多用户节点，测速也依赖真实网络）。

**第十五轮（2026-11-03）：对话不再因预算中断 + 「继续」不再复述旧文字**

- **用户报障**：release 里长任务会「突然中断」，要用户一步步发「继续」；而继续时上一段
  助手文字又会被重播回对话框。
- **release 日志证据**（`D:\Lunac\temp\logs\agent-2026-11-03.log`，可复核）：
  · `成本预算撞顶（500000 tokens）⇒ 续命 第 1/2 次` **11 次**、`第 2/2 次` **5 次**；
  · `成本预算用尽（续命已用满 2/2），要求模型收口作答` **3 次** —— 这就是「突然中断」；
  · 同一份日志 **`TodoWrite` 出现 0 次** ⇒ 模型从没建过任务清单，旧闸门只能按
    「无计划 = 2 次续命」硬砍。
- **修复（`core-agent/src/main.rs`）**：
  1. **计划即闸门的例外**：`budget_extension_allowed(…, plan_unfinished)` —— 最近一份
     `TodoWrite` 清单里还有 `pending` / `in_progress` 时，**不数续命次数、忽略任务级预算
     否决**，只要求上下文还能安全继续（压动了 / 仍宽裕）；没有计划时保持旧行为（2 次）。
     删掉 `MAX_BUDGET_EXTENSIONS_HARD`（不再需要第二档上限）。
  2. **续命优先于硬收**：原来「给过一次收口提示」的下一轮直接 `break`，即使这次还压得动 /
     还宽裕。现在先算 `can_extend`，能续就续（并重置 `hint_sent`，新一段预算重新给一次
     收口机会），续不动才硬收。
  3. **断点说明**：三处硬收尾（任务级 / 回合级 / 轮次）都把 `CUT_OFF_NOTE` 拼进**最后
     一条 `tool_result` 文本内部**（尾部新内容，不动前缀缓存），告诉模型「上次被截断、
     继续时从断点接着写、不要复述」。
  4. **系统提示词**：`PERSONA_AND_STYLE` 新增 `## Task completion (hard rule, 2026-11-03)`
     —— 多步请求先写 Todo 计划，然后**在同一轮里把整份计划做完**，不许交半成品、
     不许让用户发「继续」；`CONTINUE_PROMPT` 增加「不要重复对话里已有的文字」。
- **前端（`app/src/main.ts`）**：`chatHistory` 的助手消息新增可选 `truncated`（只内存、
  只影响回灌 payload）——`finalizeInterruptedTurn` 与被 `budget/rounds_hard_stop` 截断的
  正常收尾都置位；`sendAgentHistory` 回灌时追加 `TRUNCATED_HISTORY_MARK`。界面与磁盘
  仍只认 `role` / `content`（Rust `ChatMessage` 无 `deny_unknown_fields`，多字段被忽略）。
- **i18n**：`agent.budget_hard_stop` / `agent.rounds_hard_stop` 五语言去掉「要继续请直接
  发『继续』」，改成「已停止、改动保留」。
- **验证**：`cargo test --bins` **462 passed / 15 ignored**；`npx tsc --noEmit` exit 0。
- **新增调研任务**：`§待办 1-7`（重要判断必须给用户一句短结论）。

**VPS 实验环境（2026-11-03，已起服务；自研引擎端到端仍待 UI 导入）**

- **机器**：`43.103.55.150`，Ubuntu 24.04.2 LTS，2 vCPU / 894 MB，Xray **26.10.31**。
- **阿里云轻量安全组实测**：仅 **TCP 22 / 80 / 443** 对外可达；8080 / 8443 / 2053
  从公网连不上（不是服务没起，是安全组没放行）。⇒ 后续实验一律复用 80 / 443。
- **已起两条入站**：
  - TCP **443**：VLESS + **REALITY**（`www.microsoft.com:443`，`flow=xtls-rprx-vision`）；
  - TCP **80**：Shadowsocks **2022-blake3-aes-128-gcm 多用户**（服务端 PSK + 一个用户 PSK，
    对应 SIP023 的 `iPSK:uPSK`）。
- **服务端自检（官方 Xray 客户端在同一台机器上回连，经公网 IP）**：两条都
  `https://www.gstatic.com/generate_204` → **204**；服务端日志分别出现 `[direct]` 与
  `email: lunac-user`。⇒ 配置与凭据本身是通的。
- **仍未做**：用**自研引擎**连这两条（`native_proxy` 没有 CLI 入口，只能在 Lunac 的代理
  面板导入后点「测试连通性」+ 浏览器走代理验收）。按 `acceptance-map.md` §六，证据不足
  之前对应行保持 🟡。凭据不入库（只在上一条实验时临时打印，已随会话结束失效前请勿外传）。

**第十六轮（2026-11-03）：引擎/服务拆开（未开服务也能测延迟）+ 状态栏代理泡泡框**

- **用户口径**：
  1. 打开代理插件就**静默拉起引擎**，不必先开启服务就能测节点延迟；
  2. 测延迟用**直连可达的网站**做探针；
  3. 退出插件的收尾：服务开着 ⇒ 引擎留后台；服务没开 ⇒ 关引擎；
  4. 搜索区状态栏（`#status-text` 右侧）加一枚「代理:开/关」泡泡框。
- **根因（代码证据）**：旧实现把「引擎」和「服务（系统代理）」混成一个 `running` ——
  `mihomo_start` 同时开引擎 + 设系统代理，`mihomo_stop` 同时关两者；`delay()` 又要求
  引擎在跑，所以不开启服务就测不了任何节点。
- **后端（`mihomo.rs` / `native_proxy/mod.rs`）**：
  - `MihomoStatus` 新增 `enabled`（服务开关，来自 `config\proxy.json` 的 `mihomo.enabled`），
    `running` 语义收窄为「引擎在跑」。
  - 新增 `mihomo_engine_start` / `mihomo_engine_stop`：只动引擎，不碰系统代理、不改
    `enabled`（后者假定调用方已确认服务关着）。
  - `mihomo_stop` 改为**只关服务**（还原系统代理 + `enabled=false`），**保留引擎**。
  - `mihomo_start` 容忍「引擎已在跑」（否则面板静默拉起后点开启服务必报
    「引擎已经在运行」）。
  - `mihomo_refresh_subs`：引擎在跑时临时停→拉订阅→再拉起（服务开着则如实报错，
    不偷偷断网）。
  - 测速探针：`www.gstatic.com/generate_204` → **`www.baidu.com/`**（直连可达；明文 80，
    只量首字节）。
- **前端（`proxy.ts` / `main.ts` / `index.html` / `styles.css` / `i18n.ts`）**：
  - `attach()` 末尾 `void ensureEngine()`：静默启动引擎并重拉状态（不挡首屏、失败只
    `console.warn`）；`doTestDelays()` 在引擎还没就绪时补一次启动。
  - `detach()`：`running && !enabled` ⇒ `mihomo_engine_stop`；服务开着则留后台。
  - 大字按钮 / 状态行改看 `enabled`（新增 `proxy.px_state_ready`：引擎就绪、服务未开启）；
    `doMode` 在「服务关、引擎跑」时静默停→改→拉起。
  - 状态栏 `#status-text` 右侧新增 `#proxy-state-pill`（`#status-left` 包一层，保持
    `space-between` 三段布局）；文案 `proxy.px_pill_on/off`（五语言），开 = 语义绿。
  - 插件每次刷新 / 开关服务后 `emit("proxy-service-state", { enabled })`，主窗据此更新
    泡泡框；主窗启动时也读一次 `mihomo_status`。
- **验证**：`npx tsc --noEmit` exit 0；`cargo check --bins` 通过；`cargo test --bins`
  **467 passed / 4 ignored**。（当时 Defender 误报导致 sidecar 复制失败，根因与修复见
  **第十七轮**；现已用干净的 release `agent.exe` 重新验证通过。）

**第十七轮（2026-11-03）：Defender 误报 `Trojan:Win32/Bearfoos.A!ml` 的根因 = release 的 `strip`**

- **现象**：`D:\Lunac\agent.exe` 与仓库新构建的 `core-agent\target\release\agent.exe`
  被 Windows Defender 判为 `Trojan:Win32/Bearfoos.A!ml`（严重），构建完成即被隔离；
  `tauri-build` 复制 sidecar 时报 `os error 225`。`D:\Lunac` 里那份的 `Process Name`
  是 `explorer.exe`，说明是**文件写入 / 复制时**被静态判毒，不是运行时行为。
- **证据链（可复核）**：
  - 事件日志：`Microsoft-Windows-Windows Defender/Operational` 的 1116/1117，
    `Detection Type: 快速路径`、`ThreatName: Trojan:Win32/Bearfoos.A!ml`。
  - 病毒库 `AntivirusSignatureVersion 1.463.101.0` 的更新时间 = **2026-11-03 06:38**；
    11-01 构建的旧二进制一直没动，今天才被判毒 ⇒ 是**新病毒库的 ML 判定**，不是本轮代码。
  - 二进制字符串核对（对同源码的 debug 产物逐块扫描）：`certutil` / `mshta` / `rundll32` /
    `regsvr32` / `wscript` / `cscript` / `EncodedCommand` / `FromBase64String` /
    `Invoke-Expression` / `schtasks` / `taskkill` / `Start-Process` / `headless` /
    `user-data-dir` / `AKIA` / `PRIVATE KEY` 都在（来自 `bash_safety.rs` 的恶意命令词表、
    `content_safety.rs` 的凭据正则、`webbrowser.rs` 的无头浏览器参数）。
  - **A/B 对照**（同一份源码、同一病毒库）：
    · 默认 `strip = true` → 4.68 MB，构建后**立即被隔离**；
    · `strip = "none"` → 8.40 MB，`MpCmdRun -Scan -ScanType 3` 明确返回
      **`found no threats`**（exit 0）。
- **结论**：**不是某一行 AI 逻辑 / 某次改动的问题**，而是「`strip = true` + LTO +
  `opt-level="z"` 的 4.7 MB 无符号单文件」这一组合触发了 Bearfoos 的文件级 ML 误报；
  词表 / 凭据正则 / 无头浏览器只是把分数推高的特征，去掉符号即可降下来。
- **修复（已落地）**：`core-agent/Cargo.toml` 的 `[profile.release]` 把 `strip = true`
  改为 **`strip = "none"`**（并写明原因）。代价 = 安装包多约 3.7 MB；换来了构建 / 运行
  不再被隔离。
- **验证**：`cargo build --release --bin agent` → 8,399,232 字节；`MpCmdRun` 扫描
  **no threats**；`app/src-tauri` 的 `cargo check --bins` 通过（sidecar 复制不再报 225）。
- **完整重建（2026-11-03 20:2x）**：先 `scripts\build-plugins.ps1`（`build-release.ps1`
  的新鲜度守卫拦下了 `proxy.ts / i18n.ts / styles.css` 的过期暂存），再
  `scripts\build-release.ps1 -NoBump`：
  `release\Lunac-0.9.45-Setup.exe` **22.1 MB**；`release\Lunac\agent.exe` 8 MB /
  `lunac.exe` 61.2 MB。三个产物 `MpCmdRun -Scan` 全部 **no threats**。
- **安装版现状**：`D:\Lunac\agent.exe` 已用干净产物恢复（先前被隔离）；
  但 `D:\Lunac\lunac.exe` 仍是 19:15 的旧构建，**本轮代理 / 状态栏 / 对话修复要装新的
  `release\Lunac-0.9.45-Setup.exe` 才会生效**。若仍希望保留 strip 后的体积，则需
  **代码签名 + 向微软提交误报**（https://www.microsoft.com/en-us/wdsi/filesubmission）。
- **长期建议**：发版前把 `MpCmdRun -Scan` 作为构建后置检查（产物被隔离时构建脚本应
  当场失败，而不是静默留下一个缺失的 sidecar）。

**第十八轮（2026-11-03）：`skip-cert-verify` 打通到所有 TLS 传输（全格式真机遍历的前置）**

- **用户口径**：想用一个订阅装下所有支持的代理格式、靠面板「测延迟」遍历实机测试。
  调研结论 = 可行，但唯一硬缺口是**自签证书**：此前只有 Hysteria2 认 `skip-cert-verify`，
  vless / trojan / vmess / anytls / h2 / grpc / xhttp 一旦遇到自签就 TLS 握手失败。
- **根因（代码位置）**：
  - [`transport.rs`](../app/src-tauri/src/native_proxy/transport.rs) 的 `TlsConnector::builder()`
    没有 `danger_accept_invalid_certs`（Windows 上走 SChannel，用系统信任库）；
  - [`h2tunnel.rs`](../app/src-tauri/src/native_proxy/h2tunnel.rs) 的 `tls_config()` 固定用
    `WebPkiServerVerifier` + `webpki-roots`；
  - `Transport` 结构里**根本没有**「跳过校验」这个字段，解析出来的订阅值无处可放。
- **改动**：
  - `Transport` 新增 `insecure`（[`mod.rs`](../app/src-tauri/src/native_proxy/mod.rs)），
    注释写清它只作用于传输层 TLS，REALITY 与 Hysteria2 各自有自己的通道。
  - `clash.rs`：
    - 新增 `yaml_insecure()`（认布尔 / `"true"` / `"1"` / `1`），YAML 的 trojan / vless /
      vmess / anytls / hysteria2 全部读它；
    - `link_transport()` 认 `allowInsecure` / `insecure` / `skip-cert-verify` 三个键；
    - vmess 的 base64 JSON 读 `allowInsecure`（布尔或字符串）；
    - anytls / hysteria2 的 `Transport` 字面量同步填上。
  - `transport.rs`：`danger_accept_invalid_certs(insecure)` +
    `danger_accept_invalid_hostnames(insecure)`。
  - `h2tunnel.rs`：`tls_config(insecure)` + 新增 `AcceptAnyCert`（与 `hysteria2.rs` 的
    同名结构各自独立），`connect()` / `run_engine()` 多带一个 `insecure` 参数；
    h2 / grpc / xhttp 三条路一起生效。
  - `ss_plugin.rs` / `ss.rs`：v2ray-plugin 的 `tls` 模式也会走 `transport::connect`，
    `SsTunnel::connect` 与 `ss_plugin::connect` 各加一个 `insecure` 参数（其余插件是明文，
    传了也不生效）；调用点由 [`server.rs`](../app/src-tauri/src/native_proxy/server.rs)
    的 `dial_node` 从 `node.transport.insecure` 传入。
- **测试**（证据）：
  - 新增 `clash::tests::skip_cert_verify_reaches_transport_for_all_tls_shapes`
    （YAML 的 trojan / vless+h2、链接的 vless+grpc / trojan、vmess 的 `allowInsecure`，
    以及「没写就不能默认跳过校验」）；
  - 新增 `h2tunnel::tests::tls_config_honors_insecure_switch`；
  - 既有 `anytls_link_reads_password_and_sni` / `hysteria2_link_reads_insecure_and_obfs`
    补上 `transport.insecure` 断言；
  - `cargo check --bins` 通过；`cargo test --bins native_proxy` **113 passed / 2 ignored
    / 0 failed**；全量 `cargo test --bins` **469 passed / 4 ignored / 0 failed**。
- **仍未做**：真机端到端（要等 VPS 端口放行 + 面板导入），`acceptance-map.md` 对应行
  继续 🟡；`forbidden.md` §禁用 1 依然成立 —— 本轮只是把「用户显式要求跳过校验」这件事
  如实实现，**没有**默认放宽任何证书校验。

**全格式真机测试的端口规划（2026-11-03，等用户在阿里云安全组放行）**

前提：Xray **不提供** AnyTLS / Hysteria2 入站，这两个要额外起 sing-box（或 mihomo）。
所有 TLS 入站共用**一张自签证书**（本轮补丁之后引擎能接受），不需要域名与 ACME。

| 端口 | 协议 / 传输 | 覆盖的引擎路径 |
|---|---|---|
| TCP 80 | SS-2022 多用户（现有） | `ss` SIP022 多用户 |
| TCP 443 | VLESS + REALITY + Vision（现有） | `reality` + `vision` |
| TCP 8081 | SS 裸（aes-256-gcm） | `ss` 基础 |
| TCP 8082 | SS + simple-obfs(http) | `ss_plugin` obfs |
| TCP 8083 | SS + v2ray-plugin(ws) | `ss_plugin` ws |
| TCP 8084 | VLESS 裸 TCP（无 TLS） | `transport.tls=false` |
| TCP 8443 | Trojan + TLS | `trojan` |
| TCP 8444 | VLESS + TLS + Vision | `vision` |
| TCP 8445 | VLESS + TLS + WS | `ws` |
| TCP 8446 | VLESS + TLS + H2 | `h2` |
| TCP 8447 | VLESS + TLS + gRPC | `grpc` |
| TCP 8448 | VLESS + TLS + XHTTP | `xhttp` |
| TCP 8449 | VLESS + TLS + HTTP 伪装 | `http` |
| TCP 8450 | VMess(AEAD, aid=0) + TLS + WS | `vmess` |
| TCP 8451 | AnyTLS（sing-box） | `anytls` |
| UDP 8443 | Hysteria2 + salamander（sing-box） | `hysteria2` |
| UDP 8444 | Hysteria2 + gecko（sing-box） | `hysteria2` gecko |

要点：TCP 与 UDP 同号不冲突（8443 的 Trojan 与 Hysteria2 可并存）；阿里云要同时放行
**安全组**与系统防火墙；端口都开齐后，把上面每行做成一条分享链接粘进面板「粘贴导入」，
或打成一份 base64 / Clash YAML 订阅用本地 HTTP 静态服务挂出去，再用「测延迟」一次遍历。

**外网实测（2026-11-03，用户称已放行；结果与预期不符）**：本机 TCP 探测
`43.103.55.150` ⇒ **80 / 443 通**；**8081-8084、8443-8451 全部超时（filtered）**。
超时（而不是 RST）说明包在安全组 / 云防火墙那一层就被丢了 ⇒ 规则**尚未生效**：
要么加在了别的实例 / 地域，要么加的是 UDP，要么轻量控制台的「防火墙」页未保存。
服务器上有没有进程监听，用 `ss -lntp` 一眼可见，与外网 filtered 是两回事。

**端口复测（2026-11-03 晚，用户重设规则后）**：TCP 8081/8083/8084/8444/8446/8447/8448/
8449/8451 均能连上（RST = 端口已放行、只是没服务）；偶发超时是链路抖动，不是规则问题。
**结论：安全组已放行**。

**真机部署（2026-11-03，实际已跑起来）**

- 服务器上 **Xray 26.10.31 原本没有在跑**（只有 22/53 在听）。已用
  `/usr/local/etc/xray/config.test.json` 启动（`setsid nohup`，日志 `/var/log/xray-test.log`），
  原有 `config.json` 未改动，443 REALITY / 80 SS-2022 两条入站照旧。
- 新增自签证书：`/usr/local/etc/xray/certs/{cert,key}.pem`（CN=lunac.test，
  SAN=`DNS:lunac.test,DNS:localhost,IP:43.103.55.150`，10 年）。
- 新增 Xray 入站：8081 SS(aes-256-gcm)、8084 VLESS 裸 TCP、8443 Trojan+TLS、
  8444 VLESS+TLS+Vision、8445 VLESS+ws+TLS、8447 VLESS+gRPC+TLS、8448 VLESS+XHTTP+TLS、
  8450 VMess(ws+TLS)。
- **Xray 26 已移除的传输**：`network: http`（HTTP 伪装）与 `network: h2` —— 加进去
  `xray -test` 直接报「has been removed and migrated to XHTTP」⇒ 这两类**无法在本版本
  服务端做真机验收**，只能保留本地回环单测。
- `apt install shadowsocks-libev simple-obfs`：8082 / 8083 两套 SIP003
  （`obfs=http` / `obfs=tls`）已起；`v2ray-plugin` 不在 apt 里，未部署。
- `sing-box 1.14.4`：8451 AnyTLS、UDP 8443 Hysteria2+salamander、UDP 8444 Hysteria2+gecko
  （gecko 被 sing-box 接受，已监听）。
- 新增验收夹具：`app/src-tauri/src/native_proxy/mod.rs` 的 **`live_probe_every_node`**
  （`#[ignore]`，读 `LUNAC_LIVE_LINKS` 文本文件，逐条 `dial_node` 发 `HEAD`，打印 OK/FAIL）。
  链接清单在 `temp\live-links.txt`（被 gitignore，**凭据不入库**）。

**真机遍历结果（2026-11-03，9/15 通）**

| 节点 | 结果 |
|---|---|
| vless-reality-vision (443) | ❌「证书签名不是 64 字节（非 REALITY 临时证书）」—— 我们的 REALITY ClientHello 没被服务端认可，拿回的是 www.microsoft.com 的真实证书 |
| ss2022-multi (80) | ❌ 连接被服务端 RST（SIP023 identity/密钥对不上） |
| ss-bare (8081) / ss-obfs-http (8082) / ss-obfs-tls (8083) | ❌ 三条都是「SS 解密失败」⇒ 客户端与 libev / Xray 的 legacy AEAD 互操作有问题（自己的单测里两端共用同一实现，掩盖了它） |
| vless-tcp-plain (8084) | ✅ 167ms |
| **trojan-tls (8443)** | ✅ 1465ms（自签 + `allowInsecure=1`，本轮补丁生效） |
| **vless-tls-vision (8444)** | ✅ 247ms |
| **vless-ws-tls (8445)** | ✅ 996ms |
| **vless-grpc-tls (8447)** | ✅ 327ms |
| **vless-xhttp-tls (8448)** | ✅ 914ms |
| vmess-ws-tls (8450) | ❌「对端直接关闭」——VMess 请求体与 Xray 期望不一致 |
| **anytls (8451)** | ✅ 319ms |
| **hy2-salamander (udp 8443)** | ✅ 587ms |
| **hy2-gecko (udp 8444)** | ✅ 928ms |

**结论**：`skip-cert-verify` 这一轮补丁在真机上**全部命中**（trojan / vision / ws / grpc /
xhttp / anytls / hy2 六类自签节点全通）；没通的三组是**协议实现本身**的互操作缺陷
（SS legacy、SS-2022、VMess、REALITY），与证书无关，已如实记为待办（不可宣称「已支持真机」）。


**KCP：明确不做（G3b）**

- Xray 的 mKCP 不是「UDP 上跑 KCP」一句话：要完整实现 KCP ARQ（重传 / 快速重传 /
  拥塞窗口 / 探活）+ Xray 的 24 字节段头 + `header` 混淆（`srtp` / `utp` /
  `wechat-video` / `dtls` / `wireguard`）+ `seed` 的老式加密 + 可选 FEC。
- 这几块没有真机节点可验收，且 2026 年机场几乎不再提供 KCP；按 `forbidden.md` §禁用 1
  （不假装能力），**不写一个看着像、实际连不上的实现**。订阅里的 `network: kcp`
  继续在解析阶段跳过。

**当前功能缺口（2026-11-03 盘点，会一直存在，别误以为已支持）**

| 类别 | 缺口 | 影响 |
|---|---|---|
| **SS（legacy AEAD）** | 2026-11-03 真机：对 Xray 与 shadowsocks-libev 均「SS 解密失败」；自测两端共用同一实现，掩盖了互操作问题 | SS 节点**真机不可用**（单测全绿不代表能连） |
| **SS-2022** | 真机：多用户节点被服务端直接 RST（SIP023 identity / 子密钥对不上） | SS-2022 多用户**真机不可用** |
| **VMess** | 真机：ws+TLS 节点「对端直接关闭」 | VMess 节点**真机不可用** |
| **REALITY** | 真机：`证书签名不是 64 字节` ⇒ ClientHello 未被认可，拿回目标站真实证书 | REALITY 节点**真机不可用** |
| 传输 | `kcp` 未做（`http / h2 / grpc / xhttp` 已支持，见第十一轮）；XHTTP 的 `packet-up` 未做；v2ray-plugin 的 `quic` 模式、simple-obfs 之外的 SS 插件（kcptun / cloak / restls …）未做 | 这批节点在订阅解析阶段被跳过 |
| REALITY | 无 uTLS 浏览器指纹伪装（JA3/JA4 ≠ Chrome）、不支持 HelloRetryRequest / X25519MLKEM768 | 能连标准 X25519 的 REALITY 服务端；抗主动探测弱于 Xray |
| VLESS | `xtls-rprx-vision` 已实现功能链路；未做 TLS-in-TLS 的 long padding / 直通指纹伪装 | 正常浏览可用；抗主动探测弱于 Xray |
| VMess | `alterId != 0`、chunk masking、global padding 未做 | 老式 VMess 节点跳过 |
| SS-2022 | UDP 未做（SIP022 的数据报封装） | SS-2022 节点下 UDP 应用不转发 |
| AnyTLS | 填充策略 / 多流复用未做 | 仅单个 Stream |
| Hysteria2 | Brutal 拥塞控制未做（quinn 不开放可插拔 CC；`Hysteria-CC-RX` 恒发 0） | TCP + `salamander` / `gecko` + UDP 都可用；发送速率交给 quinn 默认 CC |
| TUIC | v5 整体未做 | QUIC 系另一套节点跳过 |
| UDP 上游 | 直连、SS（SIP004）、VLESS（Vision → Mux + XUDP）、Trojan、VMess、AnyTLS（UoT v2）、Hysteria2（QUIC 数据报 + `UDPMessage` 分片） | 六类节点的 UDP 应用（DNS / QUIC）都可转发 |
| 规则 | `GEOIP` / `RULE-SET` / `PROCESS-NAME` 跳过（需要外部数据库） | 「国内直连」退化为「只本机 / 私网直连」 |
| 测速 | `delay()` 现在**经节点**发 HTTP 探针（2026-11-03 起目标 = 直连可达的 `www.baidu.com/`），失败如实报错、不回退 TCP 建连；打开面板会静默拉起引擎，**不必开启服务**即可测 | 节点连不上 baidu 时测速会失败（这是真相，不是 bug） |
| 其它 | TUN（需管理员 + wintun）、Clash 控制口 / 策略组、Mux 多路复用、自定义 CA / `client-fingerprint` 生效 | 无系统级接管、无连接复用 |

**协议覆盖调研（2026-10-31，用户要求「遍历机场节点类型，能全包括就全包括」）**

数据源：某机场评测站对 **191 篇机场评测**的 `protocols` 字段统计（2026-09 核实）。

| 协议 / 方案 | 在用机场数 | 概率 | 本引擎现状 |
|---|---|---|---|
| Shadowsocks（AEAD） | 49 | 最高 | ✅ 已支持（三种 AEAD） |
| SS-2022（`2022-blake3-*`） | 并入 SS 统计 | 高（新机场默认） | ✅ 已支持（aes-128/256-gcm、chacha20；含多用户 `iPSK:uPSK`） |
| V2Ray 系（VMess / VLESS） | 47 | 很高 | ✅ 已完成：VLESS（TLS / WS / **REALITY** / **Vision 功能兼容**）+ VMess（AEAD 头 / alterId=0） |
| Trojan | 31 | 高 | ✅ 已支持 |
| AnyTLS | 26 | 中（2026 增长最快） | ✅ 已支持（TLS + 会话层；不含填充策略 / 多流复用） |
| Hysteria2 | 24 | 中 | ✅ 已支持 TCP + UDP（QUIC + HTTP/3）+ `salamander` / `gecko` 混淆 |
| TUIC v5 | 未单列 | 中低 | ❌ 未做（QUIC） |
| SSR | 2 | 极低（已淘汰） | ❌ 不做 |
| REALITY | 个位数 | 低（机场部署成本高） | ✅ 已支持（X25519 REALITY；无 uTLS 指纹模仿） |

> 关键结论：**REALITY 在机场里罕见**（个位数），它主要活在自建 VPS；真正决定覆盖的是
> **SS-2022 + VMess/VLESS + AnyTLS**，其次才是 QUIC 系。2026-11-02 已补上 REALITY ——
> 实际起因是用户订阅的 29 个节点全是 `vless + reality`，不补就一个都拉不到。

**分层现实**：机场节点 = 代理协议（SS / VMess / VLESS / Trojan / AnyTLS / Hysteria2）
+ 传输安全（TLS / REALITY / AnyTLS 的填充）
+ 传输方式（TCP / WS / gRPC / HTTP2 / XHTTP / QUIC）。
只补协议不补传输，仍然会有一批节点连不上。

**建议分片（按覆盖率 / 成本排序）**

1. **S1（性价比最高）**：~~VLESS + **WS 传输**~~ ✅ **已完成（2026-10-31 第二轮）**；
   ~~SS-2022~~ ✅ **已完成（2026-10-31 第三轮）**；
   ~~VMess AEAD~~ ✅ **已完成（2026-10-31 第四轮）** —— **S1 全部收口**。
2. **S1.5**：~~AnyTLS（TLS + padding 方案，实现量小、增长最快）~~ ✅ **已完成（2026-10-31 第五轮）**；
3. **S2**：~~Hysteria2~~ ✅ **TCP + salamander 已完成（2026-10-31 第六 / 七轮，QUIC + HTTP/3）**；
   ~~Hysteria2 的 gecko / UDP~~ ✅ **已完成（2026-11-03 第十二轮）**；
   ~~SOCKS5 UDP ASSOCIATE（直连 + SS UDP）~~ ✅ **已完成（2026-10-31 第八轮）**；
   剩余 = TUIC v5、SS-2022 与其余协议的 UDP。
4. **S3**：~~REALITY~~ ✅ **已完成（2026-11-02 第九轮；无 uTLS 指纹伪装）**；
   ~~gRPC / HTTP2 / HTTP / XHTTP 传输~~ ✅ **已完成（2026-11-03 第十一轮，引入已在依赖树里的 `h2`）**；
   剩余 = uTLS 指纹、KCP（G3b，明确不做）、Snell / SSH / WireGuard、SSR（不做）。

**当前不做、但用户可见的诚实提示**：订阅里被跳过的协议类型目前只记一行日志；
S1 时应在面板上显示「跳过 N 个节点（hysteria2 / reality …）」，避免用户以为节点「丢了」。


### §待办 3-29 自研引擎能力缺口清单（2026-11-02，用户要求「全部计入待办」）

> §3-25 的「当前功能缺口」表是**现状描述**；本节把每一项转成**可验收的待办**。
> 顺序按「对当前订阅 / 日常浏览的影响」排序，P0 做完再动 P1。

| # | 优先级 | 缺口 | 影响 | 验收 |
|---|---|---|---|---|
| G1 | **P0** | VLESS Vision：✅ **已完成**（2026-11-02）—— flow addons、上行首帧 padding（`cmd=End` 后转裸流）、下行解填充、`close_notify` 当 EOF | 真机 `us1.oh-node.com` 经节点取到 HTTP 204 | `live_node_fetches_generate_204` 通过；帧解析 / 分片有单测。抗 TLS-in-TLS 指纹伪装仍缺 → G8 |
| G2 | **P0** | REALITY 真机握手：✅ **已完成**（2026-11-02）—— X25519 / SessionId / 临时证书绑定全部与真实 Xray 服务端互通 | 同 G1 的真机 204 即为验收 | 认证失败报「收到的是真实站点证书」，网络错误原样透出，两者可区分 |
| G3 | P1 | 传输：✅ **`http` / `h2` / `grpc` / `xhttp`（stream-one + stream-up）已完成**（2026-11-03 第十一轮）；`kcp` 明确不做（→ G3b）；XHTTP `packet-up` 未做 | 四类节点不再被跳过；`kcp` 仍跳过 | 帧级 + 本地 h2 回环 + XHTTP padding / 路径 / 双流单测；**真机 204 待补**（手上无对应节点） |
| G3b | P3 | mKCP：完整 KCP ARQ + 24 字节段头 + `header` 混淆（srtp/utp/wechat-video/dtls/wireguard）+ `seed` 加密 + 可选 FEC | 机场几乎不再提供 KCP | **明确不做**：没有真机节点可验收，按 `forbidden.md` §禁用 1 不写「看着像、连不上」的实现 |
| G4 | P1 | SS 的 SIP003 插件：✅ **simple-obfs `http` / `tls` + v2ray-plugin `websocket` 已完成**（2026-11-03 第十三轮）；v2ray-plugin 的 `quic` 模式与其它插件（kcptun / cloak / restls）明确不做 | 带这类插件的 SS 节点不再被跳过 | 解析 + 帧级单测（假 ClientHello 偏移 / 首包藏 `session_ticket` / 响应头剥离 / WS 复用）；**真机 204 待补**（手上无对应节点） |
| G5 | P1 | UDP 上游：✅ **VLESS / Trojan / VMess / AnyTLS / Hysteria2 已完成**（2026-11-02 ~ 11-03，见 G6） | 这六类协议的节点下 DNS / QUIC 可走 UDP | 真机 `live_node_answers_dns_over_udp`（VLESS）经节点查 `1.1.1.1:53` 收到同事务 ID 回包；其余有帧级单测，待对应节点真机验收 |
| G6 | P1 | Hysteria2：✅ **`gecko` 混淆 + UDP 转发已完成**（2026-11-03 第十二轮）；Brutal 明确不做（quinn 无插拔 CC） | TCP + `salamander` / `gecko` + UDP 全部可用 | 帧级单测（gecko 切片 / 重组、`UDPMessage` 分片 / 重组）；**gecko / UDP 真机待补**（手上无对应节点） |
| G7 | P1 | TUN 未实现（需管理员 + wintun） | 无系统级接管，只能靠系统代理 | 非管理员弹闸门；管理员启动后全局流量走引擎 |
| G8 | P2 | REALITY 无 uTLS 指纹伪装（JA3/JA4 ≠ Chrome）、不支持 HelloRetryRequest / X25519MLKEM768 | 抗主动探测弱于 Xray | 抓包比对 JA3/JA4；HRR 服务端可连 |
| G9 | P2 | VMess `alterId != 0`、chunk masking、global padding 未做 | 老式 VMess 节点跳过 | 解析 + 真机 204 |
| G10 | P2 | SS-2022 多用户 PSK：✅ **已完成**（2026-11-03 第十四轮，SIP023 identity header） | 多用户机场节点不再被跳过 | 帧级单测（identity header 由 AES 解密回 uPSK 的 hash）；**真机 204 待补** |
| G11 | P2 | AnyTLS 填充策略 / 多流复用未做 | 仅单个 Stream | 与真实服务端多流复用 |
| G12 | P2 | TUIC v5 整体未做 | QUIC 系另一套节点跳过 | 真机 204 |
| G13 | P2 | 规则缺 `GEOIP` / `RULE-SET` / `PROCESS-NAME`（无外部数据库） | 「国内直连」退化为「只本机 / 私网直连」 | 内置 GeoIP 数据后按国家分流 |
| G14 | P2 | 测速：✅ **`delay()` 已改为经节点发 HTTP 探针**（2026-11-03 第十四轮；第十六轮目标改为直连可达的 `www.baidu.com/`，并支持未开服务直接测） | 面板延迟是「握手 + 转发 + 一次 RTT」，失败如实报错 | 单测覆盖不到真实网络；**真机对照 mihomo 延迟待补** |
| G15 | P2 | 缺 Mux 多路复用、自定义 CA / `client-fingerprint` 生效 | 无连接复用；自签 CA 节点不可用 | 多请求复用一条连接；自定义 CA 节点 204 |
| G16 | P3 | SSR（已淘汰）、Snell / SSH / WireGuard | 覆盖率极低 | 不做，除非用户明确要求 |

**执行顺序**：~~G1（阻塞当前订阅）→ G2（真机验收）→ G5 → G3（KCP 除外，见 G3b）→ G6 → G4（SS `plugin`）→ G10（SS-2022 多用户）→ G14（真实测速）~~（均已闭环）→ G7（TUN）→ 其余（G8 / G9 / G11 / G12 / G13 / G15）。
每完成一项，回填本节状态、[`acceptance-map.md`](./acceptance-map.md) 与
[`archive/agent-feature-backlog-history.md`](./archive/agent-feature-backlog-history.md)。


### §待办 3-26 内核收口：默认自研、不再依赖 mihomo.exe（2026-10-31，用户裁决）

> 来源：用户 2026-10-31「内核的替换方案 转向我们自己的内核 不采用 mihomo.exe」。
> 与 §待办 3-25 的关系：3-25 只做**能力补齐**（已完成，见 [`acceptance-map.md`](./acceptance-map.md)）；
> 本节是**产品化收口** —— 把 native 变成默认与唯一内核，mihomo 只保留一版只读兼容。

**现状（2026-11-01 更新）**

- ✅ **K1 已落地**：`MihomoConfig::default().engine = native`；`engine_name` 把「空串 / 旧配置缺字段」也解释成 native，只有**显式**写 `mihomo` 的旧配置还留在第三方内核。单测 `default_engine_is_native` / `blank_engine_means_native_but_explicit_mihomo_survives`。
- ✅ **K2 已落地**：代理插件按 `status.engine === "native"` 分支 —— 不再出现「下载内核」，主按钮只可能是启动 / 停止；`mihomo_set_engine("mihomo")` 在本机没有 `Modules\proxy\bin\` 可执行文件时**直接拒绝**（不再制造「切换成功、启动才报错」的死配置）。插件版本 `0.9.4 → 0.9.6`（K3 又改了一次源码）。
- ✅ **K3 已落地（2026-11-01）**：`mihomo.rs` 从 1650 行收缩到约 570 行 —— 删掉子进程托管、`config.yaml` 生成、Clash REST 控制口、内核可执行文件定位 / 下载入口；`is_running()` 只看 `native_proxy`，`proxies / providers / delay / select / set_mode` 全部直连自研引擎。`build-plugins.ps1` 的 proxy 段 `dependencies` 清空（不再下载 GPL 内核）。
- ✅ **K4 已落地（2026-11-01）**：显式 `engine: "mihomo"` 时 `start` / `set_engine` / `config_set` 都返回同一句可操作错误（**不静默降级**）；面板对该配置只显示提示、不给启动按钮。单测 `legacy_mihomo_engine_is_rejected_with_actionable_message`。
- 面板仍**不提供内核切换控件**：native 是唯一路径，`mihomo` 只为旧配置给出「怎么改回来」的提示。
- native 已覆盖 6 种协议（SS / Trojan / VLESS / VMess / AnyTLS / Hysteria2，均含 UDP 上游；SS 另含 SIP003 插件与 SS-2022 多用户）+ TLS / WS / HTTP / H2 / gRPC / XHTTP / REALITY 传输 + `salamander` / `gecko` 混淆，`cargo test --bin lunac native_proxy` 111 passed / 2 ignored（2026-11-03）。

**目标**

1. 新装 / 升级后的**默认内核 = native**，不再自动下载 mihomo。
2. 移除 mihomo 的下载与子进程托管路径；老配置显式写了 `mihomo` 时**如实提示**，不静默降级。
3. 面板显示当前内核（native），不再出现「下载内核」这类对 native 无意义的入口。

**分片（按顺序做，每片可独立验收）**

| 片 | 内容 | 验收 |
|---|---|---|
| K1 ✅ | `engine` 缺省值改 `native`；显式 `mihomo` 仍可跑（兼容一版）—— 2026-11-01 | 删掉 `config\proxy.json` 后启动 ⇒ 状态行是 native |
| K2 ✅ | 面板：状态行显示 `native`；隐藏「下载内核」；`mihomo_set_engine` 在无 mihomo.exe 时如实报错 —— 2026-11-01 | dev 实例点一遍；无 exe 的干净目录可启动 |
| K3 ✅ | 去依赖：删除下载 / 解压 / 子进程托管分支；`is_running()` 只看 native；`proxies / providers / delay / select` 只走 native —— 2026-11-01 | `cargo test`；全仓 grep 不再有下载调用 |
| K4 ✅ | 兼容与回滚：老 `engine: mihomo` ⇒ 启动报「当前版本已不再内置 mihomo，请改用 native」，不静默 —— 2026-11-01 | 手工造旧配置验收（单测已钉文案） |
| K5 | 收口验收：断网冷启动、系统代理启停还原、§测试 27 第 1–16 条全绿 | 按下表取证 |

**边界（必须如实记）**

- 去掉 mihomo 后，**`GEOIP` / `RULE-SET` 规则、策略组、真延时测速、Clash 控制口**这四项能力
  确实消失。K5 之前要在插件界面写清「当前内核的能力边界」，不能让用户以为功能回退了。
- 保留 `mihomo` 枚举值（K4 的只读兼容），但不再有任何下载 / 拉起行为。
- 不为了「去 mihomo」而假装支持 native 没有的协议（TUIC / KCP / SS `plugin` / 未知 `obfs` … 在解析阶段跳过）。

### §待办 3-27 代理插件三段式面板（2026-11-01，用户口径）

> 来源：用户 2026-11-01「代理插件的前端需要进行以下大改」—— 独立面板 1280×720、竖直三段
> （工具区 / 抽屉 + 大字开关 / 可拖拽日志）、全面改用自研引擎、日志要含目标域名。

**已落地（2026-11-01）**

- ✅ 独立窗定尺：proxy 清单 `window = 1280×720`（min 960×600），版本 `0.9.6 → 0.10.0`；
  安装器里 proxy 改为**默认勾选**（`Section /o` → `Section`）。
- ✅ 工具区：服务器导入（自定义服务器 / 剪贴板导入）、服务器订阅更新、设置 —— 下拉菜单 +
  界面内独立面板；自定义服务器含 名称 / 地址 / 端口 / 协议 / 密码或 UUID（ss 另有加密方式），
  由 `mihomo_node_add_custom` 拼成分享链接并走同一套 `parse_link` 校验。
- ✅ 中间区：左抽屉（类型 / 名称 / 地址 / 端口 / 传输 / 来源订阅）、展开铺满、右缘拉回
  （60px 竖向 `<` 拉手，2026-11-02 第二轮）；
  隐藏态中央 168px 圆形「开启 / 关闭服务」+ 上传 / 下载速率 + 出口地区（经引擎请求 ipinfo）+
  模式（规则 / 全局 / 直连）+ TUN 开关。
- ✅ 日志区：`mihomo_requests` 环形缓冲（最近 500 条），界面每 2s 取 200 条，逐行显示
  「目标域名:端口 → 来源订阅 / 节点 / 直连 / 拒绝」；顶部拖拽调高、**始终展开**（第二轮
  删掉展开/收起按钮）、高度偏好存 localStorage。
- ✅ 宿主侧：`ServerView`（含 `source` / `transport`）、`mihomo_servers` / `mihomo_refresh_subs` /
  `mihomo_requests` / `mihomo_exit_info` / `mihomo_node_add_custom` / `mihomo_set_tun`；
  `clash::build` 改为接收 `(名称, URL)` 并在 `Plan.sources` 里保留来源。

**修复（2026-11-02）：无节点时中间区塌陷 / 日志拖不动**

- 根因：`.proxy-panel` 的父级高度按内容算（auto），而 `.px-stage` 的唯一子项 `.px-center`
  是绝对定位 ⇒ stage 内容高度 0，只剩 `.px-log` 的固定 190px；拖拽上限按正在缩小的
  `stage.clientHeight` 计算，越拖越小。
- 修复：`#results-list:has(.proxy-panel)` 改为纵向 flex，`.proxy-panel` 作为 flex 子项吃满
  剩余高度；`.px-center` 改为在流内（`position:relative; flex:1; min-height:0`）；
  `.px-log-handle` 命中区 10 → 12px + `touch-action:none` + `pointercancel` 收尾；
  拖拽上限改按面板总高算（至少给中间区留 320px）。
- 验证：headless Chromium（730×601）panel 596 / stage 320 / center 318 / log 190；
  `npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin proxy` 通过（`proxy-0.10.0.zip`）。
- 未做：1280×720 真机拖拽手感。

**第二轮（2026-11-02）：日志常开 / 拉回在右 / 抽屉瘦身 / 设置收敛 + 域名编辑（proxy 0.10.3）**

- 日志区：删「收起」按钮与 `logCollapsed`；日志始终展开，仅高度可拖。底部贴窗（panel
  padding-bottom 归 0、空 `#px-msg` 不占位），headless 实测 `gapBelowLog = 0`。
- 拉回拉手：移到抽屉**右端**（`right:0`、圆角 `8px 0 0 8px`），高 60、宽 26、内容为 `<` SVG。
- 抽屉：删右上服务器数量（`#px-drawer-count`）与底部「刷新服务器」（只保留工具区的
  「服务器订阅更新」）。
- 设置：删「系统代理」「本机播放也走代理」两个开关（系统代理由中间大按钮一并启停；
  librespot 由规则接管）；新增**直连域名 / 代理域名**编辑（一行一个）→ 存
  `mihomo.direct_domains` / `proxy_domains` → `rule` 模式下展开成 `DOMAIN-SUFFIX` 规则，
  且排在订阅规则之前。改完需重启引擎。
- 拖动：pointer + mouse 双通道（WebView2 pointer 流可能整段丢失），命中区 16px。
- 验证：headless 加载真实构建产物：无 toggle/count/refresh/sys/link，域名框存在；
  拉回拉手 `h=60 / right≈stage.right`；日志 190→250 可拖。`cargo check`（target-check）exit 0；
  `cargo test --bin lunac native_proxy::` 56 passed（含新增域名规则用例）；
  `build-plugins.ps1 -Plugin proxy` ⇒ `0.10.3`。
- 未做：真机手感；域名规则的实际分流（需连节点）；购买/订阅后的刷新归属。

**第三轮（2026-11-02）：规则配置文件 / 模式两档 / TUN 管理员闸门 / 订阅即刷新（proxy 0.10.4）**

> 用户口径：上一轮的「直连域名 / 代理域名」两个框理解错了。规则模式要在订阅规则之外
> **再由插件过滤一层**；这一层在设置里是「一份域名清单 + 黑/白名单滑轨」（黑名单 = 名单内
> 不让代理走，白名单 = 名单内走代理），并给一份常见国内默认清单。模式只留规则 / 全局；
> TUN 要先弹独立警告框申请管理员权限、同意后提权重启才允许开启；服务器订阅更新就是
> 刷新服务器，不要独立面板。

- 规则分层：`rule` 模式顺序 = `lan_rules()` → **插件名单层** → 订阅规则 → `fallback=Proxy`；
  名单层必须排在订阅规则之前（`decide()` 先匹配先赢）。名单统一展开成
  `DOMAIN-SUFFIX,<domain>,DIRECT|PROXY`（`clash.rs::domain_rules`）。
- 配置：`direct_domains` / `proxy_domains` 下线，换成 `rule_mode`（`blacklist`|`whitelist`）+
  `rule_domains: Vec<String>`；都有 serde 默认值 ⇒ 旧配置读回即拿到约 90 条国内默认清单
  （`cn` / `baidu.com` / `qq.com` / `bilibili.com` / `taobao.com` / `jd.com` / `163.com` /
  `douyin.com` / `zhihu.com` / `12306.cn` …），默认语义 `blacklist`。
- 设置页：「规则配置文件」一行 = 标签 + 黑/白文案 + 滑轨（`#px-rule-mode`），下方单个域名
  清单（`#px-rule-domains`，一行一个，`#` / `//` 行忽略）；点「保存」才写入
  `mihomo_config_set`（同时保留端口编辑）。
- 模式两档：菜单删 `direct`，只剩规则模式 / 全局代理；旧 `direct` 配置仍由引擎执行，
  界面按规则模式展示。
- TUN 闸门：新增 `mihomo_is_elevated`（`IsUserAnAdmin`）与 `mihomo_relaunch_elevated`
  （隐藏 PowerShell `Wait-Process` 后 `Start-Process -Verb RunAs`，宿主 `app.exit(0)`）。
  未提权时开关回弹 + 独立 `.px-modal` 警告框；同意后才提权重启，重启后再开 TUN。
  **dev 不支持自动提权重启（2026-11-02 最终口径）**：dev 的 debug 构建是 console 子系统，
  前端又是 `devUrl` 的 vite 子进程（旧进程退出时一起被收掉），直接重启 exe 会弹命令行且
  5173 不可达；再从提权进程里另起 `npm run tauri:dev` 实测也拉不起来。因此 dev 只返回
  可操作提示（用管理员终端重启 `npm run tauri:dev`），**不退出进程**；TUN 警告框在 dev
  下隐藏「同意并重启」、改为「知道了」，由新命令 `mihomo_is_dev_build` 判定。
  release 仍重启自身 exe（`-WindowStyle Hidden`）。
  **TUN 引擎仍未实现**，开启仍会如实报错并回弹。
- 订阅更新：删除 `data-panel="subs"` 面板与 `renderSubs` / `doSubAdd` / `doSubDel` /
  `#px-subs*` 绑定；工具区按钮 = `data-action="refresh-subs"`，直接 `mihomo_refresh_subs`。
- 日志拖拽（第三次）：只挂 mouse 事件。旧版 pointer + mouse 双通道会被 WebView2 的
  `pointercancel` 提前拆掉（拖动刚开始就结束），已在第三轮改为 mousedown/mousemove/
  mouseup + blur，`mousemove` 用 `ev.buttons` 兜底窗口外松开。
- 验证：`npx tsc --noEmit` exit 0；`cargo check --bin lunac`（target-check）exit 0；
  `cargo test --bin lunac native_proxy::` 56 passed（`rule_list_switches_between_blacklist_and_whitelist`）；
  `build-plugins.ps1 -Plugin proxy` ⇒ `0.10.5`。
- 未做：真机验证名单分流；UAC 重启链路（需非管理员启动一次）；TUN 引擎本体。

**未做 / 明确边界**

- ❌ **TUN 不是「已支持」**：开关已接到宿主，但 `mihomo_set_tun(true)` 会**如实拒绝**并回弹
  （需要 wintun 驱动 + 用户态 TCP/IP 栈）。真做 TUN = 独立里程碑：引 `wintun` + `smoltcp`、
  管理员提权、改路由 / DNS、失败回滚；在此之前不得在文案里写「已支持 TUN」。
- ⬜ 真机：1280×720 观感、抽屉 / 日志拖拽手感、订阅更新后的节点归属、日志域名与实际访问一致。
- ⬜ `mihomo_refresh_subs` 只在引擎停止时可用（运行中提示先停用）；若要热更新，需要先做
  「重建 Shared 而不中断监听」的设计。

### §待办 3-28 Agent 对话流 / tools / skills 对标补全（2026-11-02 两轮调研，仅登记未施工）

> 来源：用户 2026-11-02「调研当前市面上优秀的 agent 对话流 tools skills，进行两轮轮询，对当前
> lunac 上的对话流进行调优以及功能上的补全，输出后仅计入待办」。
> **本条只登记结论与实施口径，不改代码。** 落地时按 P0 → P1 → P2 提升为独立实施条目。

**调研口径（两轮）**

- **第一轮（横向扫描，2026-11-02）**：Claude Code（subagents / agent view / fork / skills /
  plan mode / checkpointing）、Codex CLI（v0.164 交互面 / 审批档 / instant interrupt / skills /
  hooks / plan mode）、Cursor 与 Cline（编辑器内 agent 的 diff 审阅与权限）、Manus（三栏 +
  Manus's Computer 实时面板）、OpenHands（Agent Canvas + Task List）、Agentic UX 前端模式研究、
  Agent Skills 生态报告。
- **第二轮（定点核验）**：Claude Code 官方文档的子代理 / agent view / `/fork` 章节；Codex v0.164
  版本说明（`instant_interrupt`、排队提示等待审批、Code Mode 中断）；Agent Skills 开放标准
  （2025-12-18 发布，agentskills.io）；Snyk ToxicSkills 审计（2026-02-05，3,984 个技能：
  **36.82% 至少一个安全问题、13.4% 含 critical、76 个确认恶意载荷**）；SkillsBench 质量结论
  （47,150 个公开技能均分 **6.2/12**，精选技能把通过率平均抬高 **16.2 个百分点**）。
- **两条不能照抄的边界**：① 不引入 AG-UI / A2UI 协议栈 —— Lunac 的 stream-json 契约与「前缀
  逐字节稳定」是现有根基，换协议等于重做宿主与 agent 两侧；② 不做「始终允许」—— 与
  `forbidden.md` §禁用 1.2 直接冲突。下文只取**交互模式**，不取它的协议与权限模型。

**Lunac 已有（对标时不要重复造）**

- 对话流：流式 thinking / 文本 / 工具卡、命令组折叠、子代理框格、回合耗时与失败数、`sys-note`、
  待发送队列（生成中入队 / 立即发送 / 移除）、停止、回退、重试、`/compact`、`/rewind`、
  `/memory`、`/help`、计划卡（`ExitPlanMode`）、审批卡停靠、改写文件列表、用量 / 成本表盘。
- tools：Read / Glob / Grep / Write / Edit / Cmd / PowerShell / WebSearch / WebFetch /
  WebBrowser / ImageMeasure / ImageAnnotate / Agent / Skill / TodoWrite / SessionSearch /
  Remember / MCP 桥 / 后台命令。
- skills：`SKILL.md` 渐进披露（清单只带 250 字描述 + 8,000 字预算，正文按需 `Skill` 加载）、
  `context: fork` 子代理执行、`allowed-tools` 白名单、技能自带资源清单。
- 长任务：检查点 / `--resume` / `--continue`、角色会话持久化（`subagent_session.rs`）、
  任务队列、cron 会话延续。

**P0 对话流调优（先改这四件，收益最大）**

- [ ] **3-28.1 中途三态控制：排队 / 立即插入 / 暂停**
  - 现状：生成中按发送只能**入队**（本轮结束后才发），要改变方向只能「停止」——整轮工作作废。
  - 对标：Codex 的 `instant_interrupt`（新输入抢占当前回复）与 Code Mode 中断、AG-UI 的
    mid-stream steering、Claude Code 的 interrupt-and-continue。
  - 口径：生成中发送键拆成三态 —— **排队（现状默认）** / **立即插入**（只在「上一批工具结束、
    下一次请求未发」的轮首注入一条 user 消息，已完成工具不重跑）/ **暂停**（在下一个安全点
    停住并落检查点，可继续）。注入点与压缩的时机不变量**同一处**（`run_query` 轮首），
    绝不在工具批中途插消息（会拆坏 `tool_use` / `tool_result` 配对）。
  - 验收：插入的消息出现在下一次请求的 `history` 里且工具配对完整；暂停后 `--resume` 能续跑；
    日志各有 `steer=` / `pause=` 一行；真机点一遍三态。
  - **落地（2026-11-02，待真机）**：
    - core-agent：`StdinKind` 新增 `pause`；`drain_steering()` / `apply_steering()` /
      `push_steering_message()` 三件套只在**轮首安全点**分流 —— 轮循环顶部、以及
      「最终回答没有工具调用」的收口点（轮末）。带 `steer:"insert"` 的 user 注入 history；
      其余消息原样进 `VecDeque` backlog 交给主循环（不丢、不乱序）。
    - 暂停：轮首/轮末停下，**强制**落一份检查点（不依赖 `--checkpoint`），发
      `system/paused` + `result.subtype="paused"`；不触发 Stop hook。
    - 入口：聊天「更多设置 → 其他 → 生成中发送」循环三档（`lunac-chat-steer`，默认排队）；
      生成中输入 + 插入 ⇒ 不中断当前回合；队列条的「立即插入」也不再 stop 重发。
    - 兜底：回合已结束才到的插入（预算/轮次硬收口）由主循环丢弃并发 `system/steer_dropped`，
      前端撤回那条 `chatHistory` 并提示重发 —— 不在无回合状态下偷偷跑。
    - 验证：`cargo test` **461 passed / 15 ignored**（新增
      `steering_splits_inserts_pause_and_backlog_in_arrival_order`）；`npx tsc --noEmit` exit 0；
      `cargo build --release` exit 0。
    - **待真机**：三态实际点一遍；插入后下一次请求的 `api-dump.jsonl` 里确认 history 含插入
      且 tool 配对完整；暂停后 `agent.exe --resume latest` 续跑。

- [ ] **3-28.2 活动面板与对话流分离**
  - 现状：过程块全部内联在对话流里（可折叠），任务抽屉只有待办 / 改动文件 / 后台任务三页。
  - 对标：Manus 的三栏 + Manus's Computer（文件树 / 浏览器 / 终端实时），Claude Code 的
    finished subagents panel 与 task list，Agentic UX 研究的「activity panel 必须独立于
    conversation thread」。
  - 口径：右侧（或抽屉）增加**活动面板**：当前工具、子代理列表（状态 / 耗时 / token）、
    后台任务、待审批、文件改动；对话流只保留「阶段摘要一行 + 面向用户的结果」。面板**只读
    同一份事件流**，不新建状态源（沿用 `UI_MODE` 与现有事件），不新增对话区滚动容器。
  - 验收：面板与对话流同一事件驱动、无第二套状态；折叠面板不中断更新；长任务滚动对话流
    不再被过程块淹没；`#chat-log` 仍只有一层滚动。

- [ ] **3-28.3 计划步骤可编辑（plan-and-execute preview）**
  - 现状：计划卡只能「批准 / 拒绝」，不能删掉其中一步或改一句。
  - 对标：Agentic UX 的 plan-and-execute previews（批准前可改单步 / 移除步骤）。
  - 口径：把计划解析成步骤列表（编号标题 / 复选框），允许**逐条开关** + 一行备注；批准时把
    编辑后的计划作为 `ExitPlanMode` 的放行依据回传，并把原始计划与编辑记录一起留档到
    `ModuleData\plans\`。
  - 验收：删除某步后 agent 的后续动作不再执行该步；留档含 diff；键盘可达；拒绝仍是默认焦点。

- [ ] **3-28.4 错误三要素统一（what / why / next）**
  - 现状：工具失败只显示结果首行；系统旁注多为一句结论。
  - 对标：Agentic UX 的 structured error surface，一致优于「something went wrong」。
  - 口径：把 `tool FAILED` 卡与相关 `sys-note` 统一成三段：**哪一步没成 / 最可能的原因 /
    用户可以做什么**。原始状态码 / URL / JSON 仍只进日志（`forbidden.md` §禁用 6），界面只
    出现 i18n 文案；常见类（工作区锁 / 审批被拒 / 超时 / 429 / 上下文超限）各一条 key。
  - 验收：每类失败都渲染三段；界面无原始错误体；日志仍保留原文。

**P1 tools 补全**

- [ ] **3-28.5 视觉验证工具链：`Screenshot` + 固定镜头对比**
  - 现状：没有截图工具；需要看界面时只能让模型拼 Edge headless + 脚本截图（2026-11-01
    手柄图因此出了十几版，见 `docs/todo.md` §待办 3-16 的 2026-11-02 取证）。
  - 口径：新增宿主工具 `Screenshot`（窗口 / 区域 / 全屏，返回图片；**必须走审批**，与截图
    插件同级）；配套确定性能力：固定 viewport / DPR 的渲染脚本、`ImageDiff`（逐像素差异 +
    差异百分比 + 差异包围盒）。文档写死「视觉回归 = 固定镜头 + golden 图 + diff」，不靠肉眼。
  - 验收：agent 能截 HUD 窗口并与 golden 比对输出差异百分比；截图有审批与脱敏；不新增
    后台常驻进程。

- [ ] **3-28.6 会话分叉（fork）**
  - 现状：只有回退（丢后续消息）与重试，没有「保留原会话、另开一条分支继续」。
  - 对标：Claude Code `/fork`（复制到后台会话，原会话继续）与 agent view。
  - 口径：在任一助手消息上提供「从此分叉」：复制到该点的 `messages` + 人格 / 用量到新
    session id，原会话逐字节不变；两条会话在历史里并列；切换时走现有 `set_history`。
  - 验收：分叉生成新 id、原会话未被修改；两条可各自续跑；标题带「分支」后缀且不重复。

- [ ] **3-28.7 后台子代理 + 完成面板**
  - 现状：`Agent` 子代理在**本轮内**跑完即散（角色会话可持久化，但不能跨回合继续跑）；
    后台只有命令与只读任务队列。
  - 对标：Claude Code 子代理默认后台运行、finished subagents panel、可观察 / 可 steer 的
    running forks。
  - 口径：允许 `Agent` 以「后台」模式跨回合运行：状态进 3-28.2 的活动面板，完成后补一条
    `sys-note`（可选桌面通知）；结果由用户「取回」或自动并入下一轮。**必须**受任务级预算 +
    运行方式 + 审批约束，提供一键终止，不能在无人值守时无限烧 token。
  - 验收：主回合结束后子代理仍显示 running → done；取回结果不丢引用；终止立刻生效；日志可
    还原「谁在跑 / 花了多少」。
  - dsh 的边界可直接借：主对话不转发（prompt 必须自带文件与目标）、任务一次性（无追问 /
    续跑 / 实时流，只回最终结果）、取消**不撤销**外部 agent 已写的文件 —— 若照此实现，UI
    必须把「不可追问 / 取消不还原文件」写清楚。

- [ ] **3-28.8 上下文水位条与压缩预告**
  - 现状：表盘有用量 / 命中率，但没有「当前上下文占预算多少」的常显；压缩只在发生后补旁注。
  - 口径：用 `请求前缀` 与 `usage` 的实测值显示水位（60% / 85% / 95% 三档与常量同源），
    临近阈值时提示「下一次可能压缩，会改写前缀缓存」；不新增请求、不估算代替实测。
  - 验收：水位与 `budget_points` / `max_context_tokens` 同源；压缩前后数值可见；无额外 API 调用。

**P2 skills 补全**

- [ ] **3-28.9 Skill 市场 + 质量 / 安全闸**
  - 现状：技能是随包 / 本地目录的 `SKILL.md`，没有市场、没有安装前扫描、没有版本与权限 diff。
  - 数据：Snyk 2026-02-05 审计 3,984 个技能，36.82% 至少一个安全问题、13.4% critical、
    76 个确认恶意载荷；SkillsBench 均分 6.2/12，精选技能平均 +16.2pp ⇒ **分发已解决，质量与
    安全没有**。
  - 口径：市场清单至少含 `name / description / version / sha256 / allowed-tools / files`；
    安装前三件事：哈希校验、`SKILL.md` + 脚本的注入 / 凭据 / 危险命令扫描、**权限 diff 必须
    用户确认**；升级若扩大权限必须重新确认；默认关闭未验证来源。精选列表优先于「数量」。
  - 验收：critical 命中即拒装；权限扩大弹确认；哈希不符拒装；安装 / 升级 / 禁用都有审计行。

- [ ] **3-28.10 子代理预加载技能 + 技能级边界**
  - 对标：Claude Code 子代理 frontmatter 的 `skills:`（启动即注入技能正文）、`tools` 限制、
    `memory` 持久记忆。
  - 口径：给 Lunac 的子代理定义增加 `skills` / `allowed-tools` / 可选记忆命名空间；预加载只
    影响子代理自己的上下文，**不动主对话前缀**；技能级记忆必须显式开启且可清空。
  - 验收：子代理首轮即带指定技能正文；主前缀哈希不变；记忆隔离可导出 / 清除。

- [ ] **3-28.11 技能清单预算告警**
  - 现状：清单超出 `LISTING_BUDGET_CHARS = 8_000` 时静默 `break` 并写「还有 N 个未列出」，
    启动日志没有告警。
  - 口径：启动时若发生截断，落一行 warn（总数 / 已列 / 未列 + 建议缩短描述），设置页技能页
    给一次可见提示；不改变清单内容与逐字节稳定性。
  - 验收：超预算时日志与界面都能看到；未超时零噪音。

**补充（2026-11-02）：DeepSeek Harness 最新对话流可参考项**

> 版本口径：dsh `v0.2.1` 线（2026-10-17 的 rc.2 适配 DeepSeek-V4.1-Flash-1017 与
> Search / Fetch API；2026-10-23 上游再加左侧栏 / 会话视图 / 右侧插件栏，并修掉「切换会话
> 丢失未发送草稿」）。Web UI 默认 `http://127.0.0.1:3080`，官方定位是「模型 + Harness」，
> 与 Lunac 同层，可直接对照。
>
> **2026-10-30 的 rc.5 不是大改**：官方 release notes 只有一条
> 「优化对话中思考内容的展示效果 / Improve how thinking content is displayed in conversations」；
> 10-28 的 rc.4 也只有一条 ACP stdout 修复。对话流变化的大头在 **10-17 的 rc.1**
> （Developer Tools 原始日志 + 对话分组双向导航、「回复结束 / 下一条消息」时才折叠工作细节、
> 工具调用准备中显示命令描述 / 路径 / 进度、`/compact`）与 **10-28 的 0.2.2-alpha.2**
> （`subagent_oar` 委派本地 Codex / Claude、PR 视图、按轮统计插件、后台任务结果进完成通知）。
> 因此只取**交互细节**，不因 rc.5 重做对话流。

- [ ] **3-28.12 Chat / Trajectory 双视图 + 会话 JSONL 导出**
  - dsh 的做法：同一份**仅追加**会话日志支撑 Chat 与 Trajectory 两个页签；Trajectory 里
    按来源展开 SYSTEM / CONTEXT / USER / ASSISTANT / TOOL，右侧看 Payload / Result / Schema /
    Timing，顶部是 Input / Model / Tools 的时间线；恢复、分叉、检索、回放共用这一份事件流。
  - 与 Lunac 的差别：Lunac 已有「查看过程」（渲染后的步骤），原始「模型到底看到了什么」只
    散在 `LUNAC_DUMP_API` 的 `api-dump.jsonl`，两者没有合成一个可导出的视图。
  - 口径：在 3-28.2 的活动面板里增加「轨迹」页：复用已有 stream-json 事件与请求前缀哈希，
    不新造采集；提供**当前会话导出 JSONL**（过 `mask_secrets`，不含凭据）。不做浏览器 UI，
    只做桌面内视图。
  - 验收：任意会话可导出并复原每轮 system / context / user / tool；导出无明文凭据；导出不
    触发任何新请求。

- [ ] **3-28.13 Artifact-first 的回合收尾（产出物卡片 + 内联 diff）**
  - dsh 的做法：回合末尾用「Produced `sort.py`」这类徽章列出产出物，点开直接看本次改动的
    diff（含 `+n −m` 与 Open），不是让用户翻工具卡。
  - 与 Lunac 的差别：Lunac 有「改写文件列表」与写类工具卡，但产出物与 diff 分散在过程里。
  - 口径：每个回合收尾生成一张「产出物」汇总（沿用 `rebuildChangedFilesFromSteps` 的数据），
    每项带 diff 统计、打开、还原入口；与现有 3-28.2 活动面板共用同一份文件事件。
  - 验收：一个回合改了 3 个文件 ⇒ 一张卡列全；diff 与快照一致；还原走既有回退链路。

- [ ] **3-28.14 第二模型预审：只许降级 / 升级，不许放行**
  - dsh 的做法：`dsh-auto-review` 把只读第二模型放在审批链上，对每个工具请求返回结构化
    allow / deny + 理由，**失败即拒**（fail-closed），并把拒绝理由回喂给主模型。
  - 与 Lunac 的差别：Lunac 已有本地决策的「安全预筛」，但**只记账**；审批仍完全靠人工卡。
  - 口径：允许第二模型做**预审**，但输出只有两态：`deny`（附理由，直接拒）或 `escalate`
    （升级到人工卡）；**永不 `allow`** —— 与 `forbidden.md` §禁用 1.2「危险命令永远人工确认、
    永不始终允许」一致。超时 / 崩溃 / schema 不符一律走人工卡，不静默放行。
  - 验收：预审只能 deny 或 escalate；崩溃时仍出人工卡；拒绝理由进对话流与日志；有开关。

- [ ] **3-28.15 PTC（Code Mode）作为可选模式，先调研**
  - dsh 的做法：PTC 模式让模型写**一段 TypeScript 程序**，在一次执行里组合多步工具调用，
    把「每调一次工具就往返一次」压成一次；同一套工具也可切回 native 逐调。
  - 与 Lunac 的差别：Lunac 全部是逐调用；手柄图那次 23 分钟里 26 次 Edit + 43 次 PowerShell，
    正是 PTC 想消掉的形态。
  - 口径：只作为**可选运行模式**调研，不设默认。前置：安全解释器 / 工具绑定 / 审批粒度
    （程序内每次工具调用仍要过审批）/ 前缀稳定性 / 失败可回滚。先做 1 个只读工具的 PoC，
    再决定是否接线。
  - 验收：PoC 能把 ≥10 次只读调用压成 1 次模型往返；每一步仍可在轨迹里还原；不绕过审批。

- [ ] **3-28.16 定时 / 后台任务进活动面板**
  - dsh 的做法：定时任务与后台任务在 UI 里可见（创建、下次执行时间、状态），不是只活在
    配置里。
  - 与 Lunac 的差别：`cron` 与 `task_queue` 都已实现，但**前端没有入口**（见 §待办 4 的
    「任务队列首版」边界）。
  - 口径：把 cron / 队列 / 后台子代理合并进 3-28.2 的活动面板，支持查看、取消（pending）、
    看下次运行时刻；写类任务仍按既有权限口径。
  - 验收：面板能看到任务状态与下次执行时间；取消 pending 生效；running 的取消仍按拒绝处理。

- [ ] **3-28.17 工作细节的折叠时机 + 工具准备中的进度**
  - dsh 的做法（rc.1）：可设置在「回复结束」或「下一条常规消息到达」时才折叠工作细节；
    `tool_use` 还没发完时，卡片先展示命令描述 / 文件路径 / 内容生成进度。
  - 与 Lunac 的差别：过程块在回合收尾统一折叠，只有「完成」与「运行中」两态；工具卡要等
    `input_json_delta` 收完才显示参数，准备阶段是一张空卡。
  - 口径：给「折叠时机」一个二选一设置（默认保持现状 = 回复结束）；工具卡在 `content_block_start`
    就显示工具名 + 从**已收到的 partial JSON** 里安全解析的描述 / 路径 / 进度，解析不出就不猜。
  - 验收：折叠时机切换后立即生效并持久化；准备阶段卡片有可读状态；不新增请求、不改前缀。

**不做 / 边界**

- 不引入 AG-UI / A2UI / 生成式 UI：与现有 stream-json 契约、前缀缓存不变量冲突；要评估另开条目。
- 不做「始终允许」、不做危险命令自动放行、不让活动面板成为第二套状态源。
- 不做常驻 agent 团队 / 跨会话消息总线：现有 S3 的「同批并发、跑完即散」口径不变，真要做得
  先重开架构（见 §待办 3-22）。
- 不为了对标而引入前端依赖或第二套滚动容器。
- 不照抄 DeepSeek Harness 的四处：本地 HTTP Web UI（扩大攻击面，Lunac 是桌面端）、
  「万物皆插件」内核重写（与前缀稳定 / 现有契约冲突）、`Workspace Write` 那种「只限写、
  不限读」的沙箱语义、以及能**放行**的自动审批。

**验收总口径**

- 每条落地时补：① 自动化测试（纯函数 / 单测优先）；② 真机步骤与证据落 `testing.md`；③
  只在有证据时把本条勾掉，未做真机的写「待真机」，不得以「代码像对的」结案。

## §待办 4 长期方向（方向性、不排期；进入实施前先提升为 §待办 1–3 正文）

> 来源：用户 2026-10-09 立项的**长期 AI 侧规划**（分阶段能力成熟度路线图 S0–S4）。主题 = 本地模型只参与决策 / agent 整体框架图 / skills + tools 完善与长任务托管 / 到达 AGI 的要求。**本节不构成待办**，只登记方向与已对齐的调研结论。

- **S0 单模型增强**：搜索溯源（每结论挂 URL）· CLI 无头面（`--print` / `--resume` / `--output-format stream-json` / `agent mcp serve`）· **压缩语义化（已实现、已验收 2026-10-22，见 `archive/agent-feature-backlog-history.md`）** · 记忆自动抽取（云侧先行）**→ 已提升为 §待办 1-6（2026-10-11）**。
  - **无头面验收口径 = 瞄准 release（2026-10-11）**：`scripts\agent-e2e.ps1` 已改为**默认拉 release 产物**（`core-agent\target\release\agent.exe`），dev 需显式加 `-Dev`；release 缺失即 **exit 2** 并提示「先 `cargo build --release`（cwd=`core-agent`）」，**不静默回落**。理由 = 交付目标即用户实际安装 / 运行的版本。见 `docs/testing.md` §测试 10。
  - **release 二进制已重建（2026-10-11，按「以 release 为目标」这条口径补做）**：`agent.exe`（`cargo build --release`，cwd=`core-agent`，13:44）与 **宿主 `lunac.exe`**（`cargo build --release`，cwd=`app\src-tauri`，13:50，Finished in 2m52s）。⚠️ **宿主 release 不会随 core-agent 的 release 构建一起更新** —— 必须单独在 `app\src-tauri` 编一次；本次借此把本会话对 `src-tauri` 的三处改动**带进了 release 宿主**：`mcp_server.rs`（记忆落盘前过 `mask_secrets`）、`plugin_window.rs`（`open_plugin_ids` 当前打开的插件）、`main.rs`（该命令注册）。已用 `scripts\agent-e2e.ps1 -McpExe app\src-tauri\target\release\lunac.exe` 实跑 **exit 0 / PASS**（工具池 18 ⇒ MCP 桥在 release 上连通）。
  - **CLI 无头面现状（2026-10-11 摸清，逐条可复核）**：
    - **已有什么**：`agent.exe` 本就是**常驻 stdio 循环** —— `stdin` 每行一条 `{"type":"user",...}`、`stdout` 每行一条 JSON 事件（`system` / `stream_event` / `assistant`（含 `tool_use`）/ `user`（含 `tool_result`）/ `usage_delta` / `result`；契约见 `core-agent/src/main.rs:8-34`）。**已实装**：`--add-dir <dir>`（可重复）、`--permission-mode plan|acceptEdits`、`--dangerously-skip-permissions`、`--permission-prompt-tool stdio`（写类工具发 `can_use_tool` 审批）、`--disallowedTools <name…>`、`--mcp-server stdio:<exe>`（agent 拉起该 exe 的 MCP server —— 即 `agent mcp serve` 的等价物）。`--verbose` 仍**接受并忽略**；`--include-partial-messages` **已实装（2026-10-14）**（见下条「单发模式」）；**`--checkpoint` / `--resume` / `--continue` 已实装（2026-10-22，见下条 `--resume`）**。
    - **已实现（2026-10-11）：`--print` / `-p` 单发模式**。一次性跑**恰好一轮**提问后退出；**触发判据 = 带 `--print` 且未声明 `--input-format stream-json`**（后者是上游常驻 stdio 流式输入模式，宿主 `start_cli_process` 一直这么传 ⇒ **常驻模式不受影响、行为逐字节不变**）。query 来源 = `--print` 紧跟的那个非 flag 值，没给则从 stdin 读**一次**（整段一条）。`--output-format`（默认 `text`）：`text` 只打最终助手文本、`json` 打**一个** `result` JSON（含 usage / is_error）、`stream-json` 沿用事件流并在 `result` 后退出。**审批策略（关键）**：单发没有 `control_response` 回包通道 ⇒ 需审批的工具默认**直接拒绝**（绝不静默挂起，不会干等 300s 超时），除非显式 `--dangerously-skip-permissions`；项目 `.mcp.json` 信任门跳过、elicitation 直接 decline。退出码：成功 0，模型 / 网络失败或审批被拒 ⇒ 非 0。`--include-partial-messages`（2026-10-14 实装）：单发 `stream-json` 下**额外**逐条输出 `stream_event` 增量行（每行一条 JSON、当场 flush），默认**关**；常驻模式不受影响（`stream_event` 恒发）。实现落点：`core-agent/src/main.rs` 的 `CliArgs::parse` / `CliArgs::single_shot` / `OutputMode` / `ApprovalPolicy` / `emit` / `unattended_deny_reason`；用法与实测见 `docs/testing.md` §测试 10。
    - **`--resume`（跨进程会话续跑）：已实现（2026-10-22，作为 S2 检查点的第一个消费者）**。原评估「暂不做、不为此单独造会话存储」仍然成立 —— 本轮**没有另造一套会话子系统**，而是与 §待办 4 S2「检查点落盘」共用一份存储：`core-agent/src/checkpoint.rs` 把 `history` + 会话 id + 提问序号 + 任务级预算累计写成 `ModuleData\tasks\sessions\<13 位毫秒>-<session>.json`（原子写 / 单份 ≤32 MiB / **每会话**留 20 份、全局 200 份）；`--checkpoint` 开启写入，`--resume <session-id|latest>` 启动时恢复并**沿用原会话 id**（`SESSION_ID` 由 `OnceLock` 承载，`adopt_session_id` 在首次取用前写入）。默认不写 ⇒ 宿主常驻 stdio 路径逐字节不变。**边界如实记**：检查点含 history / 序号 / 预算 / **审批与工作区口径**（`approval`；恢复时只对比告警、不自动沿用）；**回合末尾**与**每批工具结果回灌之后**各落一份 ⇒ 中断后续跑不会重跑已完成的工具；工作区外的 `--add-dir` / `--permission-mode` 等仍需调用方显式重传。`set_history` 回退也会覆盖检查点（否则续跑会把已回退内容带回来）。实测与命令见 `docs/testing.md` §测试 17。
    - **仍缺什么**：~~通用任务队列（把 `scripts\agent-task-loop.ps1` 的前台循环搬进宿主）~~ —— 已由下一条落地（2026-10-22）；跨回合后台续跑已由 §待办 3-1d 的 cron 会话延续覆盖（2026-10-22）。
    - **通用任务队列首版（2026-10-22，已实现）**：宿主 `app/src-tauri/src/task_queue.rs` 起 3s tick worker，按 FIFO 跑 `ModuleData\tasks\queue.json` 里的 pending 条目；agent 侧新增 `TaskEnqueue`（**要审批卡**，单发 / skip-permissions 直接拒）/ `TaskList`（只读）/ `TaskCancel`（只取消 pending）。任务固定**只读档**（plan + 裁写类工具），单任务最多 `maxRounds`（默认 5、上限 20）轮：第 1 轮发正文，之后 `--continue --resume <session>`，退 3 就再给一轮；每轮 10 分钟墙钟。与 cron 共用 `RunGuard` 单飞闸，宿主退出时仍在跑的条目记 `failed`（不悄悄重跑）。用量记 `cron` 分区。**边界如实记**：没有前端 UI（由模型在交互会话里入队 / 查状态），写类任务不支持，运行中的任务不能取消，宿主不在时不跑。**真机验收通过（2026-10-23）**：交互入队审批卡 → 3s 内 `pending→running→done`、严格 FIFO、cron 到点 `skipped_busy`、`TaskCancel` 对 pending 成功 / 对 running 拒绝、`maxRounds` 封顶记 `failed`、杀宿主后重开记 `failed`；同轮修掉宿主 `effective_workdir()` verbatim 路径（`\\?\D:\…`）与 agent 侧 `cwd` 判不等的缺陷（`normalize_path()` + 单测）。取证见 `docs/testing.md` §测试 20。
    - **用户已裁定（2026-10-11，两条，按现状定稿、无需改代码）**：① **单发判据保持现状** —— 单发 = 「带 `--print`/`-p` **且未**声明 `--input-format stream-json`」；这是与字面「`--print` 即单发」的**有意偏离**，换取「宿主两个 spawn 路径零改动、常驻模式逐字节不变」（宿主那两处本就传 `--print` + `--input-format stream-json`）。② **「工具被拒但模型仍作答」时退出码保持非 0** —— 只要有工具因单发审批策略被拒就非 0，脚本断言才能灵敏发现「这次结果靠不住」。
    - **已实现（2026-10-11）：API 报文调试转储**。此前「看不到请求体 / 响应体原文」的缺口（现有可观测性只有 usage 级：`result.usage.requests[]` + `usage_delta` + `agent-*.log`）已补：agent 侧 `LUNAC_DUMP_API`（**默认关**，真值 `1` / `true` / `on` / `yes`）开启后，把每次 API 调用（主循环流式 + 子代理 + 摘要）的 request / response 报文各写一行到 `<log_dir>\api-dump.jsonl`；`body` 落盘前过 `mask_secrets()`，**请求头一个都不记**（连头名也不记），单条 body 截断 64K 字符、超限记 `truncated` + 实际长度，写失败只 warn。落点与目录解析沿用 `core-agent/src/log.rs` 的 `log_dir()`；测试脚本用法（`-DumpApi`、`summary.json` 字段、记什么/不记什么）见 `docs/testing.md` §测试 10。
    - **本脚本用到哪些**：`scripts\agent-e2e.ps1`（见 `docs/testing.md` §测试 10；**默认瞄准 release 产物**，dev 需显式 `-Dev`）有**两条路径**：① 常驻 stdio（默认，与前一致）—— 复用**全部已实装**参数（`stream-json` 输入输出 + `--permission-mode` + `--add-dir` + 可选 `--mcp-server`），靠「写 stdin 行 → 关 stdin → 等退出」跑单轮 / 多轮，用 `stream.jsonl` + `summary.json` 取最终文本 / 工具序列 / 逐请求 usage；② **单发 `-Print`（2026-10-11 新增）** —— 走刚实装的 `--print --output-format json`，一次一条 query、取一个 `result` JSON（最终文本 / usage / 成败），更简单但无工具序列（故不支持 `-ExpectTool`）。两条路径都**未依赖任何未实现参数**。
- **S1 本地决策旁路**：`local-engine`（llama.cpp，CPU 优先）+ 小 GGUF；**只做五类结构化决策**（能力路由 / 安全预筛 / 记忆候选抽取 / 检索查询改写 / 计划续跑判据），**不产生可见文本、不动主请求前缀**（不废缓存），**不承担 `tool_use` 往返**。**前置**：模型许可允许再分发、内存 / CPU 下限与降级路径（不满足则整个决策层透明关闭）、性能压测。**落地状态（2026-10-24）**：五类已全部接线 —— 复盘预筛（`decide.rs`）/ 记忆候选抽取（`memory_candidates.rs`）/ 检索查询改写（`query_rewrite.rs`）/ 安全预筛（`safety_prescreen.rs`，**只记账**）/ 能力路由（`capability_route.rs`，**只记账、无消费者**）；计划续跑判据由 `TodoWrite` 清单的**确定性**实现顶替（`budget_extension_allowed`），不再走本地模型。**仍缺**：**决策保真度实验阶段 1（A vs B）已于 2026-10-24 跑完**（2 任务 × 2 臂 × 3 次，结论 = 指标能区分两种策略，见 §待办 3-6 补）；**阶段 2 的「C 臂决策驱动瘦身」已于 2026-10-24 实现并实跑**（`core-agent/src/curation.rs`，见 §待办 3-6 补「阶段 2」），~~只剩「append-only 全量」那一半未做~~ —— **也已实现（2026-10-24，`LUNAC_COMPACT_APPEND_ONLY`，默认关；见 §待办 3-6 补 P6）**；另剩断网 / 性能压测（验收类，按用户裁定暂缓）；**6 个任务均已覆盖**（T2/T5 各 3 次/臂，T1/T3/T4/T6 各 1 次/臂抽样，其中只有 T4 区分）。
- **S2 长任务托管**：任务队列 + 检查点落盘 + 跨回合续跑（预算不再终止计划）；落 `ModuleData\tasks\`。**最小档已落地**（预算撞顶后按「`TodoWrite` 清单是否未完成」续命 2→6）；**任务级预算已落地（2026-10-22，最小切片）** —— `LUNAC_TASK_BUDGET_TOKENS`（默认 `0` = 关）按**主请求 `in+read+create+out` 跨提问累计**；撞顶后**不再续命**（当前问先收口一次，后续每问最多一个回合预算），但**不硬拒绝新提问**（守住「预算不终止计划」）；上报复用既有 `system/budget_exhausted`（每进程一条）、提示语带「来自 harness、不是工具输出」的来源限定。落点 `core-agent/src/main.rs`（`TaskBudget` / `parse_task_budget` / `budget_extension_allowed`）；`cargo test` 304 passed / 5 ignored（+7 条）、`cargo build --release` exit 0、agent-e2e 证据 `temp\agent-e2e\20261022-124258` / `-124451`，口径见 `testing.md` §测试 16。**检查点 + `--resume` 已落地（2026-10-22，S2 的公共前置）** —— 新增 `core-agent/src/checkpoint.rs`：每个用户回合（以及 `set_history` 回退）后把 `history` + 提问序号 + 任务级预算累计写成 `ModuleData\tasks\sessions\<13 位毫秒>-<session>.json`（原子写、单份 ≤32 MiB、目录只留最近 20 份）；`--checkpoint` 开启写入、`--resume <session-id|latest>` 启动时恢复并沿用原会话 id，任务级预算的累计值**跨进程接上**（limit=0 时不让旧文件偷偷开闸）。默认不写 ⇒ 常驻 stdio 行为逐字节不变。实测（`temp\agent-e2e\20261022-checkpoint`）：A 进程 `--checkpoint` 落 q1/history 2/预算 6866；B 进程 `--resume latest` 恢复同一 session、q2 从 1 续编、预算 6866/1000 立即撞顶但仍答出 `BANANA-42`，随后写入 q2/history 4/预算 13758。口径见 `testing.md` §测试 17。**跨回合续跑已落地 CLI 原语（2026-10-22）**：`--continue`（单发）= `--resume latest` + `--checkpoint` + 内置「接着做」提问（不写 query 时不读 stdin）；调度循环留给外部 cron / CI，agent 自身不自动循环（防无人在场时无限烧钱）。实测 `temp\agent-e2e\20261022-continue`：A 建 `CONTINUE-MARKER-77` 待办 → B 只跑 `--print --continue` 即接上文并复述该待办，用户无需重述。**调度层最小版已落地（2026-10-22）**：单发退出码新增 **3 = 正常结束但清单未完成**（仅带检查点时出现，默认路径仍是 0/1），配 `scripts\agent-task-loop.ps1` 自动循环 `--print --checkpoint` → `--print --continue`（`-MaxRounds` 封顶）。实测 `temp\agent-e2e\20261022-task-loop`：第 1 轮 exit 3，第 2 轮读出 `FIRST-LINE-MARK-52` 并 exit 0（「完成（2 轮）」）；只建 pending 不做的反例会在 `-MaxRounds` 停下并保留检查点。口径见 `testing.md` §测试 18。**仍未做**：~~常驻调度服务 / 任务队列~~（已由 §待办 3-1d 阶段 2B 之后的通用任务队列覆盖）、~~跨回合工具与审批上下文~~（审批 / 工作区口径已按「最严者胜」随检查点恢复，见 **§待办 3-21**）；剩下的是 `--mcp-server` 这类**其余**跨回合上下文。
- **S3 子 agent 团队**：常驻角色（planner / executor / reviewer）+ 任务 DAG（**需重开架构**）。**→ 已提升为 §待办 3-22（2026-10-31），实施只认那一节；分片 1–5 已实现（DAG 基座 / 面板就绪态 / 硬约束 / 具名角色常驻 / 复核回执）。**
- **S4 持续自治 + IDE 附着**：`vscode-extension/` 变成同一引擎的薄客户端（复用工具 / 审批 / 记忆 / 技能），后台常驻按需唤醒。**→ 已提升为 §待办 3-23（2026-10-31），实施只认那一节；分片 1–3 已实现（IDE 选文件提问 / 按需唤醒判据 / 唤醒源登记表）。**
- **已对齐的验收先行件「决策保真度」实验**：三臂对照（A 现状体积驱动压缩 / B 纯大上下文 / C 本地策展）+ 6 个埋约束的长任务套件 + 三层指标（决策保真度为主）。**阶段 1（可立即做）= 只跑 A vs B，先校准指标本身。**
- **本地审查 / 策展可行性结论**：小模型只宜做**受约束三态判定**（`supported`/`violated`/`unsure` + 必引 span）；**确定性优先**（引用存在性 / 声称改动 vs 实际 diff / 测试覆盖缺口先用工具判）；**不确定就 abstain、默认不拦**；拦截只对「高置信 `violated` 且不可逆 / 高危」同步拦，其余只提示 / 记日志。**落盘独立「决策账本」，不进 `chat.db`**。
- **`TeamCreate` / `TeamDelete`**：随 A13 重估后暂不做（子代理是「同批并发、跑完即散」，无常驻团队；真要做需先重开架构）。

### §待办 3-13 会话人格署名落 chat.db（2026-10-24，已实现）

> 现象：改人格预设名后，**之前对话**里每段回答上方的人格署名会跟着变 —— 历史回放用的是
> 「当前应用的人格名」（`currentPersonaName()`），而不是这段对话生成时的名字。

- **存储**：`sessions` 表新增 `persona TEXT NOT NULL DEFAULT ''`（`chat_db.rs` 建表 + `ALTER`
  迁移，老库读出来是空串）；`ChatSession.persona`（`storage.rs`，`#[serde(default)]`）。
- **写入**：`persistCurrentSessionInner` 在**首次落盘时冻结**（`saveCurrentSession` 当场取
  `sessionPersonaName || currentPersonaName()`）；**已有记录优先保留**原值，旧记录为空时
  本次保存补写。普通 / 桌宠两条线程的署名随 `switchThread()` 一起 swap（`ChatThreadPark.persona`
  / `petPersona`）。
- **展示**：新增 `displayPersonaName()` = 会话署名 || 当前人格名；`personaLineEl()` 与
  `renderChatLogHtml()` 全部改走它。新对话 / 首轮通过 `ensureTurnPersona()` 冻结，
  中途改预设名**不再影响已开始的对话**。
- **边界**：旧记录没有署名（无法追溯）⇒ 回落当前名，并在下一次保存时补写；本改动不改
  agent 系统提示词 / 固定前缀，只动展示与 `chat.db` 的一列。
- **验证**：`chat_db.rs` 新增 `persona_survives_roundtrip_and_overwrite`，并给老库迁移测试补
  `persona == ""` 断言；宿主 `cargo test --bins` **327 passed / 0 failed / 2 ignored**；
  前端 `npx tsc --noEmit` 通过。

### §待办 3-14 「更多设置」两态开关统一为 `.settings-toggle`（2026-10-24，已实现）

- **问题**：同一排「更多设置」里同时存在文字分段（`.chat-more-seg` 开 / 关）与设置面板的滑轨开关，形态不一致。
- **改法**：把 `.settings-toggle` 从 `settings.ts` 的内联 `<style>` **收编到 `styles.css`**（唯一真相源）；
  思考 / 阶段状态栏 / 本地模型决策三处两态控件改成同一套 `<label class="settings-toggle">`
  （`#chat-mode-toggle` / `#chat-debugphase-toggle` / `#chat-local-model-toggle`），
  旧的 `.chat-more-seg` 规则删除；三档及以上的权限 / 运行方式仍用点阵 / 循环按钮。
- **规范**：规则写进 [`ui-style.md`](./ui-style.md) §界面 2（含固定 markup 与 `stopPropagation` 要求）。
- **验证**：`npx tsc --noEmit` 通过、`npm test` 12 passed；未起 dev 实例做真机点击。

### §待办 3-15 本地决策开关：一个开关管到底 + 状态可见（2026-10-24 登记并实现 P0-1~P0-4）

> 来源：§待办 3-12 实验 C 组第一次启动失败。现场 = 前端「更多设置 → 其他 → 本地模型决策」已开、宿主日志 `本地模型就绪`、前端显示「已就绪」，但 `decide=0`、零判定行，整组等价于 B；用户只能手工 `set LUNAC_LOCAL_DECIDE=1` 重启宿主，再去 grep `agent-*.log` 才能确认。

**现象（可复核）**

- `lunac-2026-10-24.log:359-360`（`12:05:29Z`）：`agent 凭据 … key_tail=5581` + `本地决策模型端点 → agent: http://127.0.0.1:8090（2 槽）`；
- 同次启动前（`11:59Z`）也有一模一样的端点注入 + `本地模型探活：ready=true`，但 `agent-2026-10-24.log` 判定行为 0；
- 只有 `12:05:30Z` 出现 `本地判定预热完成` 之后，才有 `安全预筛` / `记忆候选抽取` 行。

**根因（三处，缺一不可）**

1. **开关只下发端点，不下发决策开关**：`app\src-tauri\src\commands.rs` 的 `local_model_env()` 只按 `config\local_model.json` 的 `enabled` 返回 `(url, slots)`，`push` 的只有 `LUNAC_LOCAL_MODEL_URL` / `LUNAC_LOCAL_MODEL_SLOTS`（`:424-433`），**从不注入 `LUNAC_LOCAL_DECIDE`**。
2. **决策开关只在宿主进程环境里读、且进程内缓存**：`core-agent/src/decide.rs:135-140` 用 `std::env::var("LUNAC_LOCAL_DECIDE")` + `OnceLock` ⇒ 运行中翻不动，只能「启动宿主前设好」。
3. **没有决策层状态出口**：宿主只有 `local_model_probe()`（`:2976-3013`，探 `/health` 与 `/v1/models`），没有「决策开没开 / 最近判定在哪一轮 / 各类判定多少条」的查询命令 ⇒ 用户只能读日志。

**改法（按优先级）**

- **P0-1 一个开关管到底**：`local_model_env()` 在 `enabled` 时同时下发 `LUNAC_LOCAL_DECIDE=1`；`LocalModelConfig` 增加 `decide: bool`（`#[serde(default = "default_true")]`），保留「只探活、不决策」档。**单一真相源**仍是 `config\local_model.json`，不要另造第二份开关。
- **P0-2 切开关立即生效**：保存 `local_model.json` 后重启 agent 子进程（沿用 §待办 5「切换任何 `LUNAC_*` = 重启 agent」的 `kill_and_cleanup()` 路径），让 `OnceLock` 在**新进程**里重新取值；不允许要求用户手工设环境变量。
- **P0-3 状态可见**：新增 `local_decision_status` 命令，返回 `{enabled, endpoint, slots, probe_ready, last_decision_at, counts{安全预筛/记忆候选抽取/检索查询改写/能力路由}, ledger_lines}`，在「更多设置 → 其他」同一面板显示「决策：开 / 关」「模型：就绪 / 未就绪」，替代 grep 日志。
- **P0-4 启动自检告警**：`LUNAC_LOCAL_MODEL_URL` 已注入但 `decide` 未开 ⇒ 落一行 `warn`（含补救动作），避免「看着已就绪、实际零判定」。
- **P1-5 文档同步**：§待办 5 的 `LUNAC_*` 清单补上 `LUNAC_LOCAL_MODEL_URL` / `_SLOTS`，并注明 `LUNAC_LOCAL_DECIDE` 由开关派生（P0-1 落地后不再是「需用户手设」）。
- **P1-6 实验协议**：`cache-hit-experiment.md` 的 `preflight.txt` 已要求 `decide` 从运行中进程取证（见该文件 §2.3 与 §6.2 第 5 条）；本项落地后可直接用 `local_decision_status` 填写。

**验收口径**

- 关开关 ⇒ `local_decision_status.enabled=false`、`agent-*.log` 判定行 0、无 `LUNAC_LOCAL_DECIDE`（等价 B 组）。
- 开开关（**不重启宿主、只切 UI**）⇒ agent 重启，`enabled=true`，一轮提问内出现 `本地判定预热完成` 与至少一类判定行；面板同时显示「决策：开」。
- 关闭后立刻再开，行为可重复（`OnceLock` 不残留旧值）。
- 单测：`LocalModelConfig` 的 `decide` 缺省值、`local_model_env()` 注入项（含/不含 DECIDE 两态）、`local_decision_status` 的计数解析。**不动系统提示词、不改压缩策略。**

**实现（2026-10-24）**

- **P0-1**：`storage::LocalModelConfig` 新增 `decide: bool`（`#[serde(default = "default_decide")]`，缺省 `true`）；`commands.rs` 的 spawn env 在 `local_model_env()` 命中且 `decide` 为真时追加 `LUNAC_LOCAL_DECIDE=1`。`local_model_config_set(enabled, decide: Option<bool>)` —— 不传 `decide` 时保留原值，兼容旧前端。
- **P0-2**：前端 `local-model.ts` 原本就在开关切换后调 `__lunac_reload_agent`（重启 agent 子进程），本轮未改这条链路；`OnceLock` 在新进程里重新取值。
- **P0-3**：新增命令 `local_decision_status()` → `LocalDecisionStatus { enabled, configEnabled, configDecide, endpoint, slots, counts{prewarm,safety,memory,rewrite,route}, lastLine }`；计数读**最近一份** `agent-*.log`，并排除 `INFO  tool …` 回显（与实验口径一致）。面板在「模型探活」下方多一行「决策：开 · 最近日志 N 条判定 / 尚无判定行」。
- **P0-4**：`decide=false`（仅排障档）时端点仍注入，但落一行 WARN 说明「决策层未开启（仅探活）」。
- **验证**：`cargo check --bins` exit 0；`cargo test --bins local_decision` → `counts_only_real_decision_lines` passed；`npx tsc --noEmit` exit 0。**未做真机点击验证**（开关 → 首轮对话出现判定行）。

### §待办 3-16 对话成本结构优化：先压输出与请求数（2026-10-24 登记，2026-10-27 已实现 P0-1~P0-3 + P1-4~P1-7 + P2-8~P2-9）

> 来源：§待办 3-12 三组数据。A / B / C 成本分别为 0.519 / 0.554 / 0.989 元，**输出占 51% / 69.5% / 70.3%**，缓存读只占 5.7%–7.5%；每请求 miss 为 4,670 / 1,575 / 2,018。⇒ 命中率已到 86.9%–94.2%，继续追命中率收益很小，**钱在输出与请求数上**。完整分析见 [`cache-hit-experiment.md`](./cache-hit-experiment.md) §6.4。

**P0 — 输出与请求数（预期收益最大）**

1. **子代理报告分级收口**：`SUBAGENT_REPORT_CHARS`（`core-agent/src/main.rs:446`，现 8,000 字符）按角色分级（explore ≈1.5k / coding ≈3k / review ≈2k），收口提示强制「结论 + 文件:行号 + 未验证项」，不复述过程。依据：C 的子代理平均 2,104 out/请求，是主线程 687 的 3 倍。
2. **子代理数量与任务规模挂钩**：小任务默认 ≤2 个子代理；「收尾类」工作（README / manifest / 自检结论）固定由主线程负责。依据：C 用 66 个子代理请求（138,848 out）仍缺 README ⇒ `partial`。
3. **复盘 fork 空转收口**：无新增结论时只写计数行，不生成正文（现状已有「无新增（跳过重复 N 条）」，需确认不会同时生成大段复盘文本）。

**P1 — 前缀与压缩**

4. **前缀稳定性门禁**：`公共前缀=N/M条`（`main.rs:9453`）纳入自检，相邻请求 < 90% 即 warn。
5. **序列化确定性**：工具 schema / 结构化输出用固定字段顺序的结构体，禁止 `HashMap` 直出 JSON（键序漂移会使整段前缀失效）。
6. **压缩时机**：把压缩放到「工具批次结束、下一次请求之前」，避免回合中途作废缓存；保持「先瘦身可重跑 tool_result、再丢消息」的顺序。依据：C 紧邻压缩的请求出现 miss 尖峰（9,858 / 8,497）。
7. **接 ptrblock**：压缩后被删的可重跑调用渲染成 `[elided pointers]` 回注（本次实验交付的 `render_pointers` 即此环，见 §待办 3-6 补 P6）。

**P2 — 计量口径**

8. `子代理/复盘用量并入` 拆成 `subagent` / `review` 两行；面板并列「miss/请求」「out/请求」「成本占比」。
9. 本地决策复测前固定子代理规模，否则测到的是子代理数量差异（B 30 vs C 66 请求），不是决策层。

**实现（2026-10-27，P0-1~P0-3）**

- **P0-1 报告分级**：`subagent_role::Role::report_chars()` 成为唯一真相源 ——
  general 8,000（**逐字节保持旧行为**）/ explore 1,500 / review 2,000 / coding 3,000 /
  research 3,000。`main.rs` 的 `Agent` 分支改为 `role.report_chars()` 收口，超限时多落一行
  `子代理 <id> 报告超角色上限（role=…，N 字），已裁剪`（原来静默裁剪）。
  fork 技能没有角色，仍用 `SUBAGENT_REPORT_CHARS = 8_000`。
  收口语改成三段式硬结构（Conclusion / Evidence `file:line` 或 `command -> result` /
  Not verified），并明令「不复述步骤、不贴原始工具输出」：`SUBAGENT_SYSTEM` 与
  `SUBAGENT_WRAP_HINT` 两处同口径（后者是预算 80% / 倒数第二轮注入的那句）。
- **P0-2 数量策略**：写进 `tools.rs` 的 `Agent.description`（主模型唯一的学习入口）——
  默认 1 个；单次搜索 / 单文件 / 单问题**不要派**；只有「互相独立且各自要大量工具调用」才同批
  2–3 个，一轮不超过 3（与 `SUBAGENT_PARALLELISM = 3` 一致）；README / manifest / 最终汇总 /
  自检结论这类**收尾工作留在主线程**（子代理看不到兄弟报告，交给它必然 partial）。
- **P0-3 复盘空转**：`REVIEW_SYSTEM` 与 `review_prompt` 都加「空转 = 计数行」口径
  （原文只有 `NOTHING_TO_REMEMBER`，没禁止解释）；`join_review` 命中该标记时只落
  `后台复盘完成：无新增（计数行 · 未写记忆）`，不再把模型正文抄进日志首行。
- **验证**：`cargo check`（core-agent）exit 0；`cargo test --bin agent` **411 passed / 0 failed**
  （含新增的 `report_caps_are_graded_but_general_keeps_the_old_line` 与 `review_prompt` 的
  3 条 P0-3 断言）；宿主 `cargo check --bins` exit 0。改动 `SUBAGENT_WRAP_HINT` 措辞会让
  `wrap_hint_fires_once_on_penultimate_round_only` 里写死的 `"complete report"` 失配 ——
  已改成按 `SUBAGENT_WRAP_HINT` **常量原文**计数（改措辞不再连带改测试）。
  **P2（计量口径拆分）未动**。

**实现（2026-10-27，P1-4~P1-7）**

- **P1-4 前缀稳定性门禁**：`main.rs` 新增纯函数 `prefix_stability_break(cur_common, prev_len)`
  —— 上轮 history ≥ 8 条且公共前缀跌破 **90%** 时返回百分比。`请求前缀` 埋点旁边接一条：
  没压缩却跌破 ⇒ `warn`（含问号 / 轮号 / `N/M 条`），`compacted_this_round ||
  compacted_for_retry` ⇒ 只记 `info`（压缩是正当下跌，不该当故障刷警告）。
- **P1-5 序列化确定性**：`serde_json` **不开** `preserve_order` ⇒ Map 是 BTreeMap ⇒ 键恒按字典序，
  与构造路径无关。把这条前提写进 `core-agent/Cargo.toml` 的依赖注释，并加测试
  `tool_schema_json_is_byte_stable_under_btreemap_ordering`：① `tools::defs()` 连续两次序列化
  逐字节相同；② `json!({"b":1,"a":2})` 必须序列化成 `{"a":2,"b":1}`（谁打开 `preserve_order`
  这条立刻红）。另核对过两处 `HashMap`（`conclusions.rs` 的 id 索引、`tools.rs` 的符号计数）
  都只用于**查找**，输出顺序来自 Vec / 显式全序排序。
- **P1-6 压缩时机**：核对结论 = **本来就对**，这次把它写成不变量：压缩只在轮循环顶部
  （上一批工具结束、下一请求未发）执行；`compact_history` 内部是「① 先瘦可重取的
  tool_result → ② `can_drop = (elided == 0)` 才丢整条」。补测试
  `drop_mode_elides_first_and_only_drops_when_nothing_was_elided` 钉住这个顺序。
- **P1-7 ptrblock**：新增 `core-agent/src/ptrblock.rs`（移植 `D:\test\A\ptrblock`，打码改用
  `log::mask_secrets`）。`restore_after_compaction` 增加**第 ④ 件产物**：把被丢掉的可重跑调用
  （Read / Grep / Glob / Cmd / PowerShell / WebFetch / WebSearch）渲染成 `[elided pointers]`，
  按「与最近诉求相关」优先、全序排序、1,200 字符预算，插在 `[摘要][结论索引]` 之后；
  只对 `dropped` 做（瘦身档参数还在原位）。顺序固定为 `[摘要][结论][指针][任务快照]`。
- **验证**：`cargo test --bin agent` **420 passed / 0 failed**（新增 9 条：前缀门禁 1 +
  键序 1 + 压缩顺序 1 + ptrblock 6）；宿主 `cargo check --bins` exit 0。

**实现（2026-10-27，P2-8 / P2-9）**

- **P2-8 归因拆行**：`Cfg` 拆成两本账（`sub` = 子代理 / fork 技能、`review` = 后台复盘），
  `ForkSpec` 新增 `usage: UsageKind` 由三个调用点各自声明；`detached()` 两个 `Arc` 都 clone
  （否则并行批记进副本、主循环取不到）。提问收尾时 `take()` 两本、落两行日志
  （`子代理用量并入本次提问…` / `复盘用量并入本次提问…`），`result.usage` 拆成
  `subagent` / `review` 两段（都**是总量的一部分**，消费者不得再加一次）。
  宿主 `UsageRecord` 同步加 `review`（旧记录读成 `None`，不写回），`cron.rs` 的两段归因
  改成同一段闭包解析；前端 `ChatDoneUsage.review` → 落盘 → 成本面板。
- **面板三列**：`usage-cost.ts` 的 `COL_KEYS` 追加 `未命中/请求` / `输出/请求` / `成本占比`
  （i18n 五语言）。派生列的口径：请求数为 0 或没定价时给 `—`（不编 0%）；合计行的占比恒
  100%，两个均值用总量重算。
- **P2-9 复测协议**：写进 [`cache-hit-experiment.md`](./cache-hit-experiment.md) §6.4 第 10 条
  —— 任务书写死「子代理调用数上限 3、角色固定 explore ×1 + coding ×1」，`preflight.txt`
  记实际 `task_started` 条数，条数不符则该轮作废。
- **验证**：`cargo test --bin agent` **421 passed / 0 failed**（新增
  `subagent_and_review_ledgers_stay_separate`）；宿主 `cargo test --bins usage` 9 passed；
  `npx tsc --noEmit` exit 0。**未做真机看面板**（三列的实际排版）。
- **补记（2026-10-27，顺手清掉一处重复）**：`tools.rs` 里 `Agent.input_schema.role.enum` 原先把五个角色名
  **硬编码一份**，而 `subagent_role::ROLE_VALUES` 同值仅被单测用到 ⇒ 编译报 `ROLE_VALUES is never used`。
  改成 schema 直接引用 `crate::subagent_role::ROLE_VALUES`（**序列化结果逐字节不变**，仍是那五个、
  同一顺序），常量与注释「schema 枚举的取值」就此成立，也不会再漂移。`cargo check --bins` / `cargo test --bin agent` 复跑通过。

**修复（2026-11-02）：预算 / 轮次硬收尾不再静默停住**

- **取证**：release 日志 `D:\Lunac\temp\logs\agent-2026-11-01.log` 有多次
  `成本预算用尽且收口提示无效，硬收尾（累计 532129 tokens）`（07:30 / 08:21 / 09:25 …），
  每次紧跟在一次只有工具调用的响应之后 ⇒ `final_text` 为空，`result.result` 也是空，
  前端没有任何可显示的内容；用户看到的就是「AI 无缘无故不说了」。
- **修复**：`core-agent/src/main.rs` 的三处硬收尾（任务级预算 / 提问预算 / 工具轮次）
  在 `break` 前调用 `append_hard_stop()` 把原因写进 `final_text`，并 emit
  `system/budget_hard_stop` 或 `system/rounds_hard_stop`；前端（`main.ts`）收到后
  在对话流里补一条 `sys-note`，明确「已停止 + 改动保留 + 发『继续』可续」。
  另外，`agent.exe` 在本轮中途意外退出（`cli-status: closed` 且未收到 `result`）时，
  前端也补一条 `agent.stream_closed` 旁注，不再只退回 idle。
- **验证**：`cargo test --bin agent` **457 passed / 0 failed**（新增 `hard_stop_reason_is_never_lost`）；
  `npx tsc --noEmit` exit 0。

**修复（2026-11-02）：压缩频次过高 + 思考记录错位 / 空记录**

- **取证**（`D:\Lunac\temp\logs\agent-2026-11-01.log` + `ModuleData\history\chat.db` 会话 `mvx8tgwdj33h`）：
  - 3 小时内 `上下文压缩` **58 次**（07:12:21Z–09:55:37Z，约 2.8 分钟/次），其中 **57 次**与
    `成本预算撞顶` 相邻；压缩时上下文只占 128k 的 **20.7%–52.1%**（中位 33%，最高 66,743
    tokens）⇒ **水位压缩 0 次**，全是 50 万预算续命的副作用。
  - 49 次摘要成功（50,176 输出 tokens、286.9 秒纯等待）+ **8 次空摘要**；合计丢弃 **2,235 条**
    旧消息（平均 38.5 条/次）。
  - 全天 1,177 次请求共 37.29M tokens，其中 **34.98M（93.8%）是 `cache_read`**，却按面值计入预算。
  - 过程快照 42 组只有 **11 个不同 `turn`**（`turn 8/9/10` 分别重复 10/14/8 次）；793 个思考块里
    **225 个（28.4%）** 被记成「思考过程 (0 字)」。
- **A 续命与压缩解耦**：新增纯函数 `should_budget_compact()` 与
  `BUDGET_EXTEND_ROOMY_RATIO = 0.6` —— 上下文 < 60% 时预算撞顶**只续命、不压缩**；
  真压缩才走 `Compact::Force`，并受 `MAX_BUDGET_COMPACTS_PER_TURN = 1` +
  `BUDGET_COMPACT_MIN_ROUNDS = 6` 节流。`budget_extension_allowed()` 增加 `roomy` 参数
  （不压缩也能续命；任务级撞顶仍一票否决）。
- **B 计点口径**：新增 `budget_points(in, read, create)` 作为唯一实现 —— `in + create` 全额、
  `read` 按 `CACHE_READ_BUDGET_WEIGHT_PCT = 10` 折算；`turn_tokens` / `task_budget` 的三个
  累加点全部改走它。**账面用量（in/read/create/out）与前端 usage 对账口径不变。**
- **C 摘要空结果**：摘要请求显式 `thinking=disabled`（不关时 1024 输出额度会被思考吃光）；
  端点 400 不认该字段 ⇒ 去掉字段重试一次；仍返回空摘要 ⇒ `max_tokens` 翻倍重试一次
  （最多两次尝试）。
- **D 过程快照滑窗**：`pruneContext` 裁掉 N 条助手消息后，`sessionSteps` 同步左移 N
  （`turn < N` 的组丢弃）—— 旧快照不会再按重复的 `turn` 挂到新问题上。
- **E 空思考**：`agentCloseBlock` 在 `thinkText` 为空时移除思考块；`stepFromEl` 不再回落到
  `textContent`（否则把「思考过程 (0 字)」当正文存进历史）。
- **验证**：`cargo test --bin agent` **460 passed / 0 failed**（新增
  `roomy_context_allows_extension_without_compaction` /
  `budget_compaction_is_throttled_and_skipped_when_roomy` /
  `cache_read_tokens_are_discounted_in_budget_points`）；`cargo check` exit 0；
  `npx tsc --noEmit` exit 0；`npm run verify` 自检通过（0 项提醒）。
  **未做真机**：需要把新 `agent.exe` 放进 `D:\Lunac` 才能观察实际压缩频次。

### §待办 3-17 前端三件：抽屉页签计数 / 设置「命令添加」归位 / 文件转换插件重构（2026-10-24 登记，2026-10-27 已实现）

> 来源：用户 2026-10-27 一条消息里的三项界面改动。同一条消息前的「没有出现判定行」**不是测试步骤问题，
> 是看错了实例**：
> - **dev 实例**（`app\src-tauri\target\debug\`）：`config\local_model.json` = `{"enabled": true, "decide": true}`，
>   `agent-2026-10-27.log` 里判定行齐全 —— `11:16:17Z` / `11:16:26Z` / `11:17:25Z` 三条
>   `安全预筛：medium`、`11:16:33Z` 的 `记忆候选抽取：4 条`、`11:17:33Z` 的 `记忆候选抽取：3 条`、
>   `11:17:19Z` 的 `本地判定预热完成（结果丢弃：skip）`，另有两行
>   `decide: action=skip reason=model-skip fallback=false`；
> - **release 实例**（`D:\Lunac\`）：`config\` 下**根本没有 `local_model.json`**（缺省 = `enabled: false`），
>   `lunac-2026-10-27.log` 与 `agent-2026-10-27.log` 里也就一行本地模型 / 判定相关的记录都没有。
> ⇒ 「没有判定行」= 当时看的是 release 那份；打开「更多设置 → 其他 → 本地模型决策」后即可复现 dev 的行为。
> 唯一值得记的异常是 dev 侧 `11:15:57Z` 的 `本地判定预热未成功 … HTTP 503`（模型当时还没起好）——
> 它只影响预热，判定路径随后照常出结果。

**① 任务抽屉页签上的数量（语义红 `(n)`）**

- `main.ts::renderTodoDrawer()` 的页签按钮改为「文本节点 + `<span class="todo-tab-count">(n)</span>`」，
  数量为 0 时不插（与 `empty` 态一致）；样式 `#todo-drawer-tabs .todo-tab-count` 取 `var(--danger)` +
  `tabular-nums`，不新造色值。

**② 设置面板「命令添加」与「测试联通 / 保存」同一行**

- `settings.ts` 里把它从独立 `settings-row` 挪进 `justify-content: flex-end` 那一行，样式仍用
  `.settings-install-btn`（与「测试联通」同款）；`i18n` 的 `settings.curl_btn` 文案改为「命令添加」。

**③ 文件转换插件重构（独立窗 / 四类转换 / 多文件队列）**

- **清单与窗口**：`build-plugins.ps1` 的 convert 升到 `0.9.9`、`permissions = @("window.float")`、
  补 `window` 定尺（820×470，下限 780×430）；`plugin_window.rs` 的 `default_size` / `min_size`
  同步加 `convert` 分支（清单声明优先，这里只是建窗初值）。
- **入口**：`main.ts::openPluginWindow(pluginId, presetInput?)` —— 附件进来的那条链接
  （`__lunac_convert_file`）改为随 `open_plugin_window` 的 `input` 下发；`__lunac_*` 变量跨不过窗口边界。
- **前端** `plugins/builtin/convert.ts` 重写：最左四类页签（PDF / 视频 / 视频音频提取 / 音频）、
  输入框 300×250、中间图标（转换中换成条形进度 + 上方百分比）、输出框 300×250；
  拖放走 `getCurrentWebview().onDragDropEvent()` 且**按当前页签白名单过滤扩展名**；
  队列串行执行，条目自带入队时的 `tab` / `target`（切页签不会把 PDF 条目送进 ffmpeg）；
  两侧泡泡框可点开对应文件（`open_path`），输入侧泡泡的 `×` 先 `convert_cancel` 再移除；
  输入框右上角一键清空（点两次确认）；顶栏右侧显示保存目录 + 设置按钮（`localStorage` 记住）。
- **宿主** `src-tauri/src/convert.rs`：`PDF_TARGETS` + `kind_of` 的 `pdf` 分支 + `resolve_soffice()`
  （PATH → 两个默认安装位置）；`convert_run(..., output_dir: Option<String>)` 支持保存目录；
  PDF 走 LibreOffice 无头模式（临时目录 + `-env:UserInstallation=` 隔离 profile 再改名落盘，
  **只导出 PDF 第一页**）；新增 `convert_cancel()`（`taskkill /PID <pid> /T /F` + 删半成品），
  已在 `main.rs` 的 `generate_handler!` 注册。
- **i18n**：补 `convert.tab_*` / `drop_hint` / `pdf_missing_hint` / `out_dir_default` /
  `set_out_dir` / `clear` / `clear_confirm` / `remove` / `err_no_pdf_engine` / `err_out_dir` /
  `err_type`（五语言）。输出框**不写提示文本**（用户口径）。

**④ PDF 引擎随插件装（2026-10-27 用户追加口径）**

- **宿主查找顺序**（`convert.rs::resolve_soffice`）：① `<Modules>\convert\libreoffice\program\soffice.exe`
  （依赖解到插件目录的那份，也认便携版的 `libreoffice\LibreOffice\program\soffice.exe`）
  → ② PATH / exe 根 → ③ `Program Files` 两个默认安装位置。用户自己装过的仍能用。
- **清单**：`build-plugins.ps1` 在 `release\deps\libreoffice\libreoffice-portable.7z` **存在时**
  才声明 `archive` 依赖（`dest = "libreoffice"`，sha256 由脚本现算，`url` 指向公开 Release 的
  `libreoffice-$LoVersion`）；文件不存在则只打一行提示、**不声明依赖**（否则整包装不上）。
  资产准备步骤（删 help/gallery/pdb → 7z → 上传）写在该段注释里。
- **硬约束**：解压后必须 < 1 GiB（`plugin_market::MAX_DEP_EXTRACT_BYTES`）；依赖**不加 `local`**，
  免得几百 MB 被拷进安装包与 dev 的 `Modules\`。
- **验证**：`-NoBuild` 重打包一次，日志出现「未找到 … 本次不声明 PDF 引擎依赖」，
  `lunac-plugin.json` 的 `dependencies` 仍为空数组（本机没有该资产）—— 分支正确。

**验证**

- `npx tsc --noEmit` exit 0；`cargo check --bins` exit 0；`cargo test --bins convert` 5 passed；
  `scripts\build-plugins.ps1 -Plugin convert` 产出 `release\plugin-packages\convert-0.9.9.zip`（16.4 KB）。
- **未做真机点击验证**（拖放 / 泡泡取消 / 保存目录）；本机 `ffmpeg` 在 PATH，但 `soffice` 缺失 ⇒
  PDF 分页会如实显示「未找到 LibreOffice」。
- **补记（2026-10-27）**：重构后 `convert.rs` 的 `unique_output()`（默认「源文件同目录」那版）只剩单测在用，
  生产走 `unique_output_in()` ⇒ release 编译报 `unique_output is never used`。已给它加 `#[cfg(test)]`
  （**测试仍覆盖它**，`cargo test --bins convert` 5 passed；宿主编译告警 26 → 25）。

### §待办 3-18 代理内核改插件依赖 + 插件下载可暂停 / 断点续传（2026-10-27 已实现）

> ⚠️ **2026-11-01 部分被取代**：①「mihomo 作为代理插件依赖」已由 §待办 3-26 K3 删除 ——
> 内核改为自研、编进宿主，`Modules\proxy\bin\` 不再需要任何可执行文件。本节①保留为历史记录；
> ②「下载可暂停 / 断点续传」仍然有效（它服务的是**其它**插件的依赖）。

> 来源：用户 2026-10-27 两条口径 —— ① 「代理插件的 core mihomo 应该作为代理插件的依赖放在
> `Modules\proxy\bin\`，不应该在 `lunac\cores\` 里」；② 「下载插件时退出设置面板 / 切到其他分页要
> **后台继续下**，并加**暂停按钮**；**强制退出 Lunac** 后能从退出时的进度**继续下载**」。

**① mihomo 只认插件依赖（`Modules\proxy\bin\`）**

- **删掉宿主自己下载内核的那条路**：`mihomo.rs` 的 `install_core` / `fetch_checksum` / `http_get` /
  `release_base` / `asset_name` / `legacy_core_exe` / `DEFAULT_RELEASE_BASE`，以及
  `MihomoConfig.release_base` 字段（旧 `config\proxy.json` 里的多余键由 serde 忽略，不需要迁移）。
- **命令保留、语义改掉**：`mihomo_install_core` 仍叫这个名字（前端不必改），实现改为
  `install_plugin_dependencies("proxy")` —— 校验 sha256、失败回滚、进度事件全部复用**既有**那条链路，
  不再维护第二份版本常量或第二个下载器。
- `bundled_core_exe()` 是**唯一**来源：`Modules\proxy\bin\` 下按 `mihomo.exe` →
  `mihomo-windows-amd64.exe` → `mihomo-windows-amd64-<版本>.exe` → 目录内第一个 `.exe` 兜底；
  `is_installed()` / `start()` 都只走它。**卸载插件即随目录清掉**。
- **运行期工作目录迁出 `cores\`**：`core_dir()` → `work_dir()` = `<exe 根>\ModuleData\proxy`
  （`config.yaml`（每次启动重写）/ `data\`（geoip、provider 缓存）/ `mihomo.log`）。
  旧 `<exe 根>\cores\mihomo\` **不再读、也不再写**；老用户那份可以手删（订阅 / 节点在
  `config\proxy.json`，不受影响；geo 数据会重新下载）。
- **清单侧**（`build-plugins.ps1` proxy 段）：`archive` 依赖 `dest = "bin"` 是内核唯一来源，注释同步；
  升版本要同时改 `mihomo.rs` 的 `MIHOMO_VERSION` / `ASSET`。`proxy` **0.9.3 → 0.9.4**（源码变了）。

**② 下载可暂停 + 断点续传（`temp\downloads\`）**

- **背景**：宿主原本就是「阻塞线程里逐个文件下载」，**关面板 / 切页签并不会中断**；丢的是
  ① 界面（那一行重绘后进度没了）② 半截数据（`Vec<u8>` 或 `dest.part`，应用一退就白下）。
- **宿主（`plugin_market.rs`）新增 `fetch_resumable`**：所有下载统一落到
  `<exe 根>\temp\downloads\<sha256(url + 清单哈希) 前 16 位>.part` + 同名 `.json`
  （`url` / `id` / `etag` / `total` / `offset` / `sha256`）。
  - 续传 = 按 `.part` 的**实际长度**发 `Range: bytes=<offset>-`，有 ETag 就带 `If-Range`：
    `206` 追加写；`200`（服务端不认 Range 或文件换了）**丢断点从头来**；`416` 同样丢掉重来。
  - **暂停** = 断开当前连接、原地等（不读流、不占 socket），继续时重新发 Range 接上；
    **取消** = 删 `.part` + `.json`（与旧语义一致）。
  - sha256 在**下完之后整份重算**（增量哈希跨不过进程），代价是一次本地顺序读。
  - 断点名字里带**清单哈希**：作者换版本时旧断点自动作废，绝不会把两版拼在一起。
- **接入点**：`install_file_dependency`（GGUF 等大文件）、`install_archive_dependency`（zip/7z）、
  插件包本体（`commands::install_plugin_from_url_blocking` 改走 `fetch_package_bytes`）。
- **新命令**（`commands.rs` + `main.rs` 注册）：
  `plugin_install_pause(paused)` / `plugin_install_state()`（`active` / `kind` / `id` / `url` /
  `downloaded` / `total` / `paused` / `resumable`）/ `plugin_pending_downloads()`（重启后还剩哪些断点）。
- **前端（`settings.ts`）**：进度改成**模块级快照 + 全局单监听**，重绘后由 `mountMarketInstall()`
  把环 / 阶段 / 速度 / **暂停按钮**挂回对应的行；行上「暂停 ⇄ 继续」调 `plugin_install_pause`。
  磁盘上有断点时，该行按钮文案变「**继续下载 (37%)**」，点一下即从断点续。
  新 i18n（五语言）：`plugins_market_pause` / `_resume` / `_continue` / `_paused`。
- **边界（如实记）**：① 同一时刻只支持**一次**安装（状态是单例，与「前端按插件串行发起」的既有约定一致）；
  ② 「断点续传」依赖服务端支持 Range —— GitHub Releases 支持；不支持时退化成从头下（不会坏包）；
  ③ `install_plugin_dependencies`（local-model / ocr / proxy 的「补依赖」）同样享受暂停与续传，
  但那几处面板**暂未加暂停按钮**（入口在各自的插件面板，不在设置的市场行）。

**③ 用户复测两条（2026-10-27 修复，未真机验证）**

> 用户原话：「1. 更新关闭后继续下载，进度条会出现**两种进度**，一个稍快一个稍慢，速度不正常，稍快的是真实进度；
> 2. 有时候会**秒更新完毕**，但其实没更新完毕。」

- **两条同一个根因：并发写同一个 `.part`**。暂停的实现是「宿主线程原地等」，所以暂停后再点一次
  「下载 / 继续」会**再起一个**下载线程（前端 `download()` 当时不查宿主状态、宿主也没有入口互斥），
  两个线程各自按 `.part` 长度算 offset 追加写 ⇒ 界面出现一快一慢两条进度；谁先读到 EOF 谁就把
  **半截文件**改名成正式文件 ⇒ 「秒更新完毕，其实没下完」。
- **宿主加安装闸（`INSTALL_BUSY` + `InstallBusyGuard` + `try_begin_install`）**：粒度 = 一次安装
  （插件包本体 + 它的依赖算一次；补依赖 / 开发工具链各自一次）。占闸的三个入口：
  `install_plugin_from_url_blocking`（"插件包"）、`install_plugin_dependencies`（"插件依赖"）、
  `plugin_toolchain::install`（"开发工具链"）。**暂停中的那次仍然占着闸** —— 恢复必须走
  `plugin_install_pause(false)`，不能再发一次安装；被拒时如实回「已暂停：点『继续』接着下」。
- **`fetch_resumable` 收尾判据收紧**：原来 `offset >= total` 就当成功（`total` 估计偏小 / 半截文件
  都会被判成完整）⇒ 改为 `offset == total` 才算完；`offset > total` 丢弃断点重来；
  连续 `short_reads > 5` 次仍不齐 ⇒ **如实报错**（「只拿到 x / y 字节」），不再静默成功。
- **前端（`settings.ts`）`download()` 先查宿主**：进入时 `invoke("plugin_install_state")`，若已有活动任务
  且 `url`/`id` 命中本行 ⇒ **只做「继续 + 重挂进度」，不发安装**；用对象身份 `mine` 判归属，
  连点时后来者不会把先启动那条的进度清掉（原先 `finally` 里的无脑 `liveInstall = null` 已删）。
- **未坐实**：若用户的「秒更新完毕」还来自别处（例如市场索引里 sha256 为空 ⇒ 校验跳过、或极小包
  立即命中旧断点），需真机日志再定位，本轮**不宣称已彻底修完**。

**验证（2026-10-27）**

- 宿主：`cargo check --bins` exit 0；`cargo test --bins plugin_market` **30 passed**
  （新增 `part_path_is_stable_per_url_and_manifest_hash`、`part_meta_roundtrips_and_tolerates_old_files`、
  `install_gate_is_exclusive_and_released_on_drop`）。
- 前端：`npx tsc --noEmit` exit 0；`npx vitest run src/download-progress.test.ts` **5 passed**；
  `npm test` **17 passed**；`scripts\build-plugins.ps1` 全量 exit 0（`proxy v0.9.4`、`ocr v0.9.11`）。
- **未做真机点击**：暂停 / 继续、切页签 / 关面板后进度还在、强杀后「继续下载 (n%)」、
  以及本轮修的「两种进度 / 秒完成」四件都要真机验。

### §待办 3-19 图片标注：把「测量 + 绘制」从模型手里拿走（`ImageMeasure` / `ImageAnnotate`）（2026-10-29 登记；2026-10-30 两步都已实现，待真机验收）

> **进度（2026-10-30）**：两步都已落地。
>
> ① `ImageMeasure`（第一步）—— `core-agent/src/image_measure.rs` +
> `image` crate（只开解码器）+ 工具 schema / dispatch / 并行白名单 / 前端工具开关。
> 输入 `file_path`（可选 `threshold` / `region` / `scan_rows` / `scan_cols` / `ascii_columns`），
> 输出紧凑 JSON：尺寸、前景 bbox（含 `right` / `bottom` 闭区间端点）、像素数与占比、
> 指定行 / 列上暗区的首末像素、可选 ASCII 缩略图。只读 / 本地 / 免审批 / 只读档放行。
>
> ② `ImageAnnotate`（第二步）—— `core-agent/src/image_annotate.rs`，输入**规格 JSON**
> （`line` / `dimension` / `arrow` / `text` 四种图元），用**系统 GDI+**（`windows` crate 的
> `Win32_Graphics_GdiPlus`）绘制，字体取系统已装的「Microsoft YaHei」（失败依次退
> SimHei / SimSun / Arial，**不随包分发字体**），输出 PNG（`image` 编码）并回传每个图元的
> 实际坐标 / 箭头三角 / 文字 bbox。语义要点：图元按数组顺序绘制（后画的在上）；
> `arrow_dir` 的 `out` = 尖在端点、底边朝内（制图标准），`in` = 三角画在外侧；
> 标签缺省落在中点沿法线的 -14 px（水平线即上方），给了 `at` 就用它；**坐标越界直接拒**
> 并报出图的真实尺寸；源图永不覆盖。写类 ⇒ 进 `needs_approval`、只读档 / 计划相位由
> `write_blocked` 拒、不进并行白名单。规格解析对**未知键 / 未知枚举值**一律报错
> （拼错 `"dimenison"` 或 `"colour"` 不会被静默忽略）。
>
> 验证：`cargo test` **434 passed / 0 failed / 15 ignored**（其中 `ImageMeasure` 4 条、
> `ImageAnnotate` 7 条：尺寸线落点 + 用 `ImageMeasure` 复算 bbox、绘制顺序（后画覆盖先画）、
> 中文出墨与 bbox、PNG 尺寸 / 魔数、越界拒、规格严格性、颜色 CSS 顺序），前端 `tsc` 绿。
> **未做**：真机验收（步骤见 `docs/testing.md` §测试 23）。

> **来源**：2026-10-27 / 10-29 的真机会话 `mvqmx4a24j0w`（四杯照片标尺寸）。24 条消息里 12 轮
> 都在修同一张图，用户最后「算了」。取证结论：**不是模型没收到图**（`vision: true`，08:19 起
> 每轮都随请求发出 1–2 张、约 267 KB base64），而是三条结构性缺口。

- **现象（逐条有日志/对话为证）**
  1. **VLM 报绝对像素坐标不可靠**：1540×920 的图里 logo 只有约 60 px 宽，模型先写
     `draw-cup4-dims.ps1` + 像素采样脚本，再按返回值填坐标；用户连续纠正「logo 顶/底整体低
     60–70 px」「右端落到了绳子上」「界线离杯体太远」「两个箭头同向」。
  2. **没有可复用的标注工具**：每轮现写一份 PowerShell + `System.Drawing` 脚本，坐标常量埋在
     脚本里，改一处错一处；自检只 `Write-Host` 打印像素值，**没有断言**（例如「界线距轮廓 ≤4 px」
     「数值不压图案」）。
  3. **规范 / 澄清缺位**：先画「单箭头 + 引线」（不合机械制图），要用户教「两条尺寸界线 + 反向
     箭头」；单位是 cm 还是 logo 宽、引线是否出杯也没先问；`draw-cup4-dims.ps1` 存成 UTF-8
     **无 BOM** ⇒ PowerShell 5.1 按 GB2312 读，图上中文变乱码（`docs/testing.md` §测试 3 早有 BOM 纪律）。
- **不做**：微调模型权重；把标注质量押在「换个更强的 VL 模型」上（用户 2026-10-29 裁定：暂不切
  独立视觉模型，先用确定性工具补）。
- **目标形态（两个内建工具，宿主侧 Rust）**
  - `ImageMeasure`：输入 `path` + 若干「探针」描述（整图 / 指定矩形 / 指定行或列 / 指定色阈），
    输出**确定性**像素统计：前景 bbox、按行/列的暗区边界（首末像素）、指定区间的最宽位置、
    ASCII 缩略图。全部是整数坐标 + 采样口径，可复算。
  - `ImageAnnotate`：输入 `path` + **标注规格 JSON**（尺寸线两端点、界线起点/终点、箭头方向
    `in/out`、文本、字号、颜色、层叠顺序、输出路径），输出 PNG 并返回实际绘制坐标。
    模型只产出「规格」，**不再手写绘图脚本**。
  - 落点：`core-agent/src/tools.rs`（工具注册 + 描述）+ 新模块 `core-agent/src/image_annotate.rs`；
    只读文件系统语义与 `Read` 一致（受工作区锁约束），写只允许落到用户给定路径。
- **验收判据**
  1. 同一张杯子照片、同一份自然语言规格：从「12 轮 + 用户纠正 6 次」降到 **≤3 轮**、无需人工指坐标。
  2. 工具输出里每条坐标都能被 `ImageMeasure` 复算验证（断言，不是打印）。
  3. 中文文本渲染不乱码（工具内部写 UTF-8 → 直接调 GDI+，不再经过 PowerShell 脚本文件）。
  4. 不改变现有 `Read` / `SendUserFile` 行为；无图片任务时工具不被选中（描述写清适用面）。
- **排序理由**：`SendUserFile` / 图片附件 / `vision` 开关都已具备，唯一缺口是「谁来做像素测量」；
  这一层补上后，同类任务（图纸标注 / 截图对比 / UI 像素走查）都直接受益。

### §待办 3-20 加密图源的封面（`onThumbnailLoad`）仍会坏图（2026-10-29 登记；2026-10-30 已实现，待真机验证）

> **来源**：2026-10-29 修「章节图片加载不出来」时只接了 `comic.onImageLoad`（章节页），
> 封面走的是另一条分支 `comic.onThumbnailLoad` —— 6 条加密图源（漫蛙吧 / 漫王 /
> mojoin_v2 / manga51 / gfmhapp / ccc）的**列表封面、书架封面仍然是坏图**。
> 章节侧的取证与修法见 `docs/resource-hub-design.md` §13.5「加密图源：章节图片加载不出来」。

- **落地形态（2026-10-30）**
  1. 沙箱 `image` 方法本来就按「有无 chapterId」分流到 `onImageLoad` / `onThumbnailLoad`，
     本轮只补了两头：`ResourceSource.thumbnailData(url)`（SDK）+ 插件调用点。
  2. 渲染封面时带上 `data-thumb-source` / `data-thumb-url`（原地址，**不过代理**）；
     网格封面、远程详情页封面都写，历史记录从 `remoteWorks` 取原地址。
  3. `error` 事件不冒泡 ⇒ 在 root 上**捕获阶段**听：`<img data-thumb-url>` 一失败就调
     `thumbnailData` 解出明文，换成 `blob:` URL（`destroy()` 统一 `revokeObjectURL`）。
     解不出来就**保持坏图**，不弹提示（与现状观感一致）。
  4. `imageData` / `transform` 只留在漫画兼容层（`createJsSource` 那条通用契约没有任何
     图片钩子，写进去就是死代码，已移除）。
- **验证**：SDK 测试 98 条全过（同一条用例里补了「无 chapterId ⇒ 走 onThumbnailLoad」断言）；
  SDK + 插件 `typecheck` 绿；插件产物 1,284,354 B 已覆盖 dev `Modules\resource-hub`。
- **未做**：真机看一次漫蛙吧搜索结果 / 书架封面（需重开插件窗）。
- **边界如实**：一屏 20 张封面 = 20 次 `engine.eval`，只在**加密来源**且**首次失败**时发生；
  仍受沙箱「结果 ≤1 MiB」限制，超限封面退回坏图。

### §待办 4 补 记忆层演进 + 其他 LLM 类工具（按价值排序）

> **来源**：用户 2026-10-11 两条诉求 —— ① 「记忆层演进（LLM-Wiki 范式）」登记为一条方向；② 把调研到的其他 LLM 类工具/实现**按价值**整理进来。
>
> **纪律**：**本小节仍守 §待办 4 顶部的规矩 —— 不构成待办、不排期；只登记方向与「值不值得做 / 该提升到哪个 §」的判断。**进入实施前先**提升为 §待办 1–3 正文**，此处不写「待实施的任务」。

**B1. 记忆层演进：单文件 `MEMORY.md` → 互链 wiki 目录（LLM-Wiki 范式）**

> **→ 已提升为 §待办 3-24（2026-10-31）**：前置裁定已定（真相源仍是 `MEMORY.md`、指令优先于事实、
> 注入快照只在压缩边界刷新）；实施只认 §待办 3-24，本节保留方向与调研原文。

- **范式来源**：Andrej Karpathy 提出的 **LLM Knowledge Base / LLM Wiki** 模式 —— 把 RAG 的「**查询时检索**（query-time retrieval）」换成「**写入时编译**（compile-time）」：原始资料（论文 / 仓库 / 邮件…）进 → LLM agent 把它**编译成互相链接的 markdown wiki 页面** → 之后的长期查询与增补都在这个 wiki 上做。**不引入向量库、不建 embedding 管线**（技术栈就是 markdown + LLM + 一个 UI）。
- **现成实现形态（都是 CLI / 本地服务，与「本地优先」同向）**：PyPI 的 `llmwiki`；npm 的 `@tymolu/llmwiki`；以及一个 FastAPI 写的 `llm-wiki` —— 带**四层记忆 + 双时态事实 + 自动矛盾消解 + 定时维护**。
- **与我们现状的同源性**：我们的 `ModuleData\memory\MEMORY.md`（**复盘 fork 编译出的 bullet 列表**）**就是这套范式的「最小版」** —— 写入时由 LLM 编译、读时整份注入。⇒ 演进方向 = **单文件 → 互链页面目录**，并补上这套范式标配的**自愈 / 去矛盾 / 定时维护**。
- **落地落点（都已存在，不必重开架构）**：注入通道 `context_block_message`（见 **§待办 3-3**）、读写桥 `lunac/memory_read` / `_write`（见 **§待办 1-6**）、复盘 fork 抽取器（**§待办 1-6**）、多层 `AGENTS.md` 目录（**§待办 3-1a**）。**不新造第二套记忆通道。**
- ⚠️ **必须先想清（前置裁定）**：它与 `AGENTS.md`（**指令**，进固定前缀）和 `MEMORY.md`（**记忆**，进可变层）**概念会重叠** —— **哪一份是真相源**要先定死，否则会演化成两套互相打架的知识库。层归属直接引 §待办 3-1a 已定的那份（指令 = 固定前缀、记忆 = 可变消息）。
- **建议**：**不急着接第三方实现**，先把「真相源 + 分层」定下来，再借 LLM-Wiki 范式改我们**自己**的记忆层（单文件→目录、补自愈 / 去矛盾 / 定时维护）。

**B2. 其他 LLM 类工具/实现（按对我们这套 agent 的价值排序）**

> 排序基准 = 「本地优先、成本敏感、已有 traits（单一 `MEMORY.md`、复盘抽取、`SessionSearch`、`Grep/Glob`、S1 本地决策计划）」下的 **缺口大小 × 落地代价**。

1. **【高价值·理论缺口】时间性事实 / 事实有效期** —— `Zep`（其引擎 `Graphiti`，论文 arXiv 2501.13956）是**时序知识图谱**：记录「**每个事实何时为真、何时不再为真**」；llm-wiki 也主张**双时态事实**（valid time + transaction time）。
   - **对我们的用处**：我们 `MEMORY.md` 是**扁平 bullet、没有有效期概念**，去重只做「**整条 bullet 完全相同**」（现状见 **§待办 1-6**）⇒ **过期事实会无限积累、且永不失效**。这是**最明显、代价最小的缺口**。
   - **要不要做 / 放哪**：**值得做**，最小步 = bullet 上加 `valid_from/valid_to`，或**在复盘时做冲突消解**（与 B2-4 Mem0 的更新语义合流）。归 **§待办 1-6**（复盘抽取）与 **§待办 3-1a**（记忆目录）范围，**不单开 §**。
2. **【高价值·低成本】Repo Map（tree-sitter AST + PageRank + 按 token 预算裁剪）** —— `aider` 的成熟做法：解析 AST 抽符号 → PageRank 排序 → **二分**适配 token 预算，得到「整库结构骨架」。
   - **对我们的用处**：我们**已有 `Grep/Glob`，但没有「代码库结构感知」**（不知道谁是核心、谁被引用最多）。它**直接服务 §待办 4 S0 的「压缩语义化」**，且 **tree-sitter 有 Rust 绑定、不依赖任何第三方服务**（契合本地优先）。
   - **要不要做 / 放哪**：**值得做**，作为 **§待办 4 S0「压缩语义化」**的实现路径之一（提升时并入 S0 正文）；**不单开 §**。
   - **已实现（2026-10-14，先验证：零依赖轻量版）**：用户裁决「先做零依赖正则版、不引 tree-sitter」⇒ 实装内置工具 `RepoMap`（`core-agent/src/tools.rs`，工具池 23 → 24）—— 纯正则抽符号 + **引用计数近似排序**（**不是 PageRank**）+ 字符预算裁剪，输出「文件路径 → 顶层符号」骨架。**按需调用的只读工具**，结果进 `tool_result`（可变层），**不进 system 固定前缀**（骨架随文件变，进固定前缀会每轮 cache miss，见 `forbidden.md` §禁用 45）。扫描**跳过 `deny_read_dirs`**（安全项：不得成为读禁目录后门；**策略级过滤、非沙箱**）。
   - **精度边界（如实记，尚未达到文档口径）**：① 抽取是**正则近似** —— 行首锚定 + 跳注释行可抑制多数误捞，但**块注释内独立成行 / 多行字符串里的声明样式文本仍可能误捞**，异形（宏生成 / 跨行）声明会漏；`#[test]` 等嵌套 `fn` 也会计入（不区分顶层 / 嵌套）。② 排序是**引用计数**，会给 `new` / `run` / `name` 这类高频通用名虚高分（真 PageRank 会按符号引用图加权）。③ 大仓库按**体积上限早退**（600 文件 / 单文件前缀 1 MB / 合计 4 MB），且按**路径序**扫描 ⇒ 路径靠后的子树可能被前段吃满预算而整段落空（例：本仓库根扫描被 gitignore 的上游 `_reference/` 占满，`core-agent` 未进榜；指向子目录 `core-agent/src` 即得干净骨架）。④ **tree-sitter AST + PageRank 版仍未做**（本轮只验证「值不值得」）。
3. **【中价值·同源】记忆分层与「谁策展」** —— `Letta`（MemGPT 论文 arXiv 2310.08560）的**自管上下文分页 / 分层记忆**；llm-wiki 的 **four tiers**（working / episodic / semantic / procedural）。
   - **对我们的用处**：与我们 **§待办 3-6 L11** 的「**重要节点审查 / 策展**」**同源**（都在问「谁决定哪条记忆该留、该提升」）。
   - **要不要做 / 放哪**：**建议两条互相引用、不各写一份** —— 分层设计并进 **§待办 3-6 L11** 的策展方案讨论，术语可借 llm-wiki 四层。
4. **【中价值】冲突消解 / 自愈** —— `Mem0` 的 extract-and-retrieve（对已有记忆做**更新 / 失效**操作）；llm-wiki 的**自动矛盾消解**。
   - **对我们的用处**：我们现在只有「**整条相同即去重**」⇒ 借它的**更新语义（新增 / 替换 / 失效三态）**即可，`replace` 通道**已存在**（见 **§待办 1-6**）。
   - **要不要做 / 放哪**：**值得做**，与 B2-1（时间性事实）**同批落地**，归 **§待办 1-6**。
5. **【中价值·参考基准】记忆评测口径** —— 2026 年横评基准 = **LoCoMo / LongMemEval / BEAM**；公认最难的两个开放问题 = **cross-session identity** 与 **temporal reasoning**。
   - **对我们的用处**：**temporal reasoning 正好压在 B2-1**（事实有效期）；**cross-session identity 正好对应我们「每进程一个会话」的现状** —— 跨会话身份 / 连续性是我们的**结构性缺口**（相关落点见 **§待办 4 S2**「跨回合续跑」与 §待办 4 内对 `--resume` 的评估）。
   - **要不要做 / 放哪**：**只作评测参照**（我们无云端评测台，能用的地方有限）—— 登记为「衡量记忆层进展的标尺」，**不单独排期**。
6. **【低优先·与我们取向冲突】语义代码检索类** —— `claude-context`（向量库 + 混合检索，宣称省 token ~39%）、`jCodeMunch`（建符号索引、暴露 11 个工具）、`Sourcegraph`（跨仓库检索）。
   - **对我们的用处**：我们**刻意不引入向量库**，且已有 `Grep/Glob/SessionSearch` ⇒ 与取向**直接冲突**。
   - **要不要做 / 放哪**：**不做**（**除非**将来跨仓库 / 大型代码库检索成为瓶颈）—— 登记为「**已知替代路线、暂不采纳**」。
7. **【顺带记录·接第三方路线】** `Supermemory` 等**记忆 MCP 服务**（自带 MCP 集成、面向编码 agent）。属「**接第三方**」路线：与我们本地优先取向**不冲突但需评估**（数据出本机、成本 / 依赖、与自有记忆层的关系）。**只登记名字与取向，不展开**。

## §待办 5 环境变量与数据根（核对基线）

- **数据根 = `<exe 根>`（便携式，无独立 HOME）**：`skills\` / `tools\` / `ModuleData\`（`history\chat.db`、`memory\MEMORY.md`、`usage\usage-*.jsonl`、`tasks\cron.json` / `tasks\cron-state.json` / `tasks\cron.lock` / `tasks\queue.json`（**通用任务队列**，2026-10-22）/ `tasks\queue.lock` / `tasks\sessions\`（**检查点**，2026-10-22））/ `temp\`（`logs`、`tool-outputs`、`transStorage`、`webview-data`、`review-last`）/ `config\`。
- **`core-agent` 读取的 20 个 `LUNAC_*`**（均**只在 spawn 时注入**；切换其中任何一项 = `kill_and_cleanup()` 重启 agent）：端点 / 凭据 / 模型、`LUNAC_MAX_CONTEXT_TOKENS`、`LUNAC_SUMMARY_COMPACT`、`LUNAC_MEMORY`、`LUNAC_SKILLS_DIR`、`LUNAC_CRON_DIR`（**定时任务目录**，2026-10-17 阶段 0 新增；阶段 1 未新增任何 `LUNAC_*`）、`LUNAC_THINKING`、`LUNAC_WORKSPACE_LOCKED`、`LUNAC_DENY_READ_DIRS`（**读禁目录**，2026-10-13 新增，见 §待办 1-4 补「模型目录读禁」）、`LUNAC_HOOKS_FILE`（按 mtime 热重载）、`LUNAC_PERSONA_FILE`（启动读一次）、`LUNAC_MCP_FILE`、`LUNAC_SEARCH_PROVIDER` / `_KEY`、`LUNAC_LOG_DIR` / `LUNAC_LOG` / `LUNAC_LOG_LEVEL`。**（`LUNAC_NUDGE_INTERVAL` 已于 2026-10-11 随复盘触发改造删除，见 §待办 1-6。）**
- 另有 7 个由 agent 自己解析 / 兜底（`LUNAC_MODULES_DIR`、`LUNAC_CACHE_TTL_SECS`、`LUNAC_TURN_BUDGET_TOKENS`、`LUNAC_TASK_BUDGET_TOKENS`（**任务级预算**，2026-10-22 新增，默认 `0` = 关，见 §待办 4 S2）、`LUNAC_CHECKPOINT_DIR`（**检查点目录**，2026-10-22 新增，默认 `<exe 根>\ModuleData\tasks\sessions`，见 §测试 17）、`LUNAC_TASKS_DIR`（**通用任务队列目录**，2026-10-22 新增，默认 `<exe 根>\ModuleData\tasks`，见 §测试 20）、`LUNAC_COMPACT_APPEND_ONLY`（**append-only 全量压缩**，2026-10-24 新增，默认 `0` = 关，见 §待办 3-6 补 P6））与 2 个只传给 hook 子进程（`LUNAC_HOOK_EVENT`、`LUNAC_TOOL_NAME`）。
- **本地决策（S1）一组**（2026-10-24 补记，默认**全关** ⇒ 与不接线的行为逐字节一致）：`LUNAC_LOCAL_MODEL_URL`（**只在装了 `local-model` 插件且设置里打开时才注入**；未注入 = agent 整层透明关闭）、`LUNAC_LOCAL_MODEL_NAME`（缺省 `local`）、`LUNAC_LOCAL_MODEL_SLOTS`（**2026-10-24 新增**：= 插件清单 `sidecar.args` 的 `-np`，由宿主解析后注入，缺省 / 非法 / 超 4 ⇒ 1）、`LUNAC_LOCAL_DECIDE`（**2026-10-24 起由宿主派生**：`config\local_model.json` 的 `enabled && decide` 为真就注入 `1`，见 §待办 3-15，不再需要用户手工设环境变量）、`LUNAC_LOCAL_DECIDE_K`、`LUNAC_LOCAL_CURATE`（后者独立于前两者，默认关）。
- **「什么时候生效」要如实说**：工具与注入块是「重启 agent 才生效」；只有 `hooks.json` 是 mtime 热重载；技能 = 重启 agent，插件 = 立即。

---

> **历史细节见 `docs/archive/agent-feature-backlog.md`（含 §0 已闭环记录、§4.1 长期规划全文、§5 边界声明）。**
