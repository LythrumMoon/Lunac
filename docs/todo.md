# 待办事项（含调研输出总结）

> **本文档是什么**：Lunac **未完成 + 未验证**工作的唯一登记处，每条给出「目标 / 已查清的结论或调研摘要 / 待办点 / 涉及文件」。**已完成、已闭环的条目见 [`archive/agent-feature-backlog-history.md`](./archive/agent-feature-backlog-history.md)（不再保留逐条过程）。**
>
> **取代了哪些旧文档的哪些章节**：`agent-feature-backlog.md` 的「优先级总表」、§1（Agent 能力缺口）、§2（界面层待办）、§3（Lunac 自身新目标）、§4 设想区（含 §4.1 长期 AI 侧规划）、§5（边界声明）、§6（环境变量与数据根）中的**未完成项**。
>
> **维护纪律**：做完就删（结论落进 §11 规则 / 代码注释 / Git 记录，不由待办承载）；每条必须能落地（写不出落点的放 §待办 4 方向区）；优先级只按本文顺序。
>
> **历史细节见 `docs/archive/agent-feature-backlog.md`。**

---

## §待办 1 用户已排期 / 高优先

### §待办 1-1 L1 Live2D 桌宠：最后一步端到端验收
- **目标**：让桌宠能从**插件市场**装一次、走完整链路。
- **已查清**：桌宠外壳、Live2D 引擎（`pixi.js@7` + `pixi-live2d-display-lipsyncpatch`）、Core 自托管副本（带版本 + sha256 + 授权文件）、`pet` 与市场索引均已发布（远端 `index.json` 含 `pet`）。除发布外全部真机验收过。
- **待办点**：从市场装一次、走完整链路；`lunac-installer.nsi` 的 `SecExtPet` 勾选段已加，未重打包验证。
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
- **待办点**：① **自动 / 定时抓屏** —— **已实现（默认关 + 频次上限），待真机验收**；② **屏幕变化触发** —— **已实现（默认关 + 频次上限），待真机验收**；③ **当前配置的模型目录禁止 AI 读取** —— **已实现（2026-10-13，只拦读；见下 §待办 1-4 补「模型目录读禁」）**；④ 桌宠轮「不调工具 / skills」 —— **已取消（2026-10-13，用户裁决）**（原拟「桌宠轮工具闸」）；⑤ 语音输入（入口已就绪）。
- **涉及文件**：`app/src/plugins/builtin/pet.ts`、`app/src-tauri/src/screenshot.rs`、`core-agent/src/main.rs`。

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

#### §待办 1-4 补 桌宠语音朗读：去系统音色兜底（静默不读）+ 参考音色导入 + 音色库缓存（2026-10-12）

- **用户裁决（本轮三条）**：① 去掉「ZipVoice 不可用 / 失败 ⇒ 回落 WinRT 系统音色」**整条**，改为**只有 ZipVoice 可用时才出声**，不可用或失败**静默不读**（只记一行 `log::warn`，不弹错、不动气泡）；② 本轮只做 **UI 入口 + 音色库缓存层**（不接新引擎、不建多引擎注册表）；③ 参考音频 / 文本**共用既有 `PetVoiceConfig`**（`config\pet-voice.json`），**不新增第二套配置**。
- **A 去兜底（`app/src-tauri/src/tts.rs`）**：`speak_blocking`（`tts.rs:151`）改为**只走 ZipVoice**：`tts_zipvoice::available()` 为假 ⇒ `log::warn` 后直接 `return`；`speak()` 失败 ⇒ `log::warn` 后吞掉（不再合成、不再播）。`tts_speak`（`tts.rs:213`）**签名不变**（前端契约不动）。**删除清单**：WinRT `synthesize_wav`（Windows 实现）、`pick_zh_voice`、非 Windows 的 `synthesize_wav` 桩，及文件头对 WinRT 引擎的整段描述；`Cargo.toml` 里**仅为它添加的 4 个 `windows` feature**（`Media_SpeechSynthesis` / `Media_Core` / `Foundation_Collections` / `Win32_System_WinRT`）一并删（`Storage` / `Storage_Streams` 保留给 `windows_ocr.rs`；**`windows` crate 本身不删**）。`parse_wav` / `mouth_envelope` / `play_wav_bytes` / `play_wav` **保留**（口型与播放还要用）。
- **B 音色库缓存 + 导入（`app/src-tauri/src/tts_zipvoice.rs`）**：缓存目录 `config\pet-voices\`（`voices_dir()`，`tts_zipvoice.rs:286`，走既有 `lunac_root_dir()`）。新命令 **`import_pet_voice_audio`**（`tts_zipvoice.rs:725`，在 `main.rs` 命令表注册）——**入参**=用户选中音频的**绝对路径**；**校验**：文件存在 → `rodio::Decoder`（内含 symphonia）解码（wav / mp3 / flac / ogg / m4a）→ 混单声道后非空 → **时长 3~30 秒**（`REF_MIN_SECS` / `REF_MAX_SECS`，`tts_zipvoice.rs:293`；**出处 = 本文件文件头，项目既有说法，仓库内无上游文档**）；**规整**成 **PCM16 WAV**（复用 `encode_wav_pcm16`）写入缓存目录，文件名 = **内容 SHA-256**（`content_name`，`tts_zipvoice.rs:619`，防覆盖 / 免路径注入），**返回缓存副本绝对路径**，前端据此 `set_pet_voice`。**失败可读**（各一句中文，原始 OS 错误不进界面）：找不到文件 / 不是可识别音频 / 没解出声音 / 时长越界。**缓存卫生**：`prune_voice_cache`（`tts_zipvoice.rs:633`）导入后**只保留新副本**（config 只持一个 `reference_audio`）——**不留无限增长的垃圾**。命令体是 **async 薄壳 + `run_blocking`**（解码 / 落盘是阻塞 IO，见 `code-standards.md` §规范 5）。
- **`get_pet_voice` / `PetVoiceStatus`**：新增 **`reason`** 字段（机器可读 key：`disabled` / `no_ref_audio` / `no_ref_text` / `ref_missing` / `assets_missing`，**空串 = 可用**），供前端本地化；`available()` 与 `reason_for`（`tts_zipvoice.rs:300`）同源（**静默不读的判据就是它非空**）。
- **C 前端入口（`app/src/plugins/builtin/pet.ts` + `app/src/i18n.ts`）**：设置窗「语音朗读」区块（`ttsBlock`，`pet.ts:547`）新增 —— **参考文本输入框**（`#pet-tts-reftext`，placeholder 要求「填你那段音频里**实际说的原话**」）+ **「添加音频文件」**（`#pet-tts-addfile`，走 `@tauri-apps/plugin-dialog` 的 `open` → `import_pet_voice_audio` → `set_pet_voice`）+ **状态行**（`#pet-tts-status`，读 `get_pet_voice` 显示可用性与**不可用原因**）+ 明示**静默语义**的说明行（`pet.tts_silent_note`）。逻辑在 `pet.ts:2443-2517`（状态回显 / 文本提交 / 导入 / 试听）。**i18n 五语言齐全**（`i18n.ts` 的 `pet.tts_*` 一组）；样式只用既有四类 CSS 变量（`pet.ts:3084-3091`，**不新造颜色**）。
- **「试听」为什么加（可选件，本轮加了）**：本轮只有「导入 + 静默不读」两条，没有试听则用户**无法判断配好没配好**（配置错时表现为完全没反应）。按钮复用既有 `tts_speak` 读一句固定示例（`pet.tts_preview_text`），是本轮改动的最短验证路径，未新增任何链路。
- **验证（2026-10-12，cwd=`app\src-tauri` unless noted）**：`cargo test` **213 passed / 0 failed / 2 ignored**（新增 5 条：`reason_covers_every_silent_branch`、`import_rejects_missing_file`、`import_rejects_non_audio`、`import_rejects_out_of_range_duration`、`import_caches_pcm16_wav_and_prunes_old_copy`）exit 0；`cargo build --release` exit 0；`npx tsc --noEmit`（cwd=`app`）exit 0；`npm run build`（cwd=`app`）exit 0；`npm run verify`（仓库根）exit 0。⚠️ **改到宿主（`tts.rs` / `tts_zipvoice.rs` / `Cargo.toml`）⇒ 需重编 `lunac.exe`**。
- **顺带发现并修复（非本轮引入）**：`npm run verify` 报 `scripts\agent-e2e.ps1` 与 `scripts\build-plugins.ps1` 开头有**2 份 BOM**（应为 1 份）——按 `testing.md` §测试 3 的既定修法**归一成恰一份**（只改前三字节，无逻辑改动），修后 verify 通过。
- **真机验收口径（多为 UI + 音频，无头端验不了声音）**：① 设置窗「语音朗读」块内应见「参考音色」标题 + 输入框 + 「添加音频文件 / 试听」两枚按钮 + 状态行；② 未配置时状态行显示「未就绪 · 还没有导入参考音色」（或「· 还没填写参考音频里实际说的那句话」）；③ 选一个 **3~30 秒**音频 → 状态变「已配置：参考音色可用」，随后**移动 / 删除原文件**再看状态仍可用（**缓存副本生效**）；④ 已导入音频但**输入框留空** ⇒ 状态回落「未就绪 · 还没填写…」，桌宠回复**不朗读**（静默、不弹错）；⑤ 未装语音模型（资产缺失）⇒ 状态「未就绪 · 语音组件未就绪」，回复静默；⑥ 「试听」在可用时以参考音色读一句示例、不可用时**毫无反应**（无报错弹窗）。**可无头验的部分已给脱敏证据**：单测 `import_caches_pcm16_wav_and_prunes_old_copy` 断言导入在**系统 temp 子目录**生成 `voice-<sha256>.wav` 的 PCM16 WAV、可被 `read_wav_mono` 读回、重复导入落同一文件、换一段后旧副本被清（目录只剩 1 份）；夹具目录均在系统 temp、用后删除，**不落仓库**。
- **Breeze TTS 2 评估结论（本轮不接入）**：已评估，**暂不接入** —— ① **授权**：其权重 / 自托管产物**仅限研究与非商用**，与本项目分发口径冲突；② **运行时**：需 **Linux + Python + CUDA（≥12GB 显存）**，本应用是 Windows 便携单体，接不进。**故本轮仍以 ZipVoice（sherpa-onnx，Windows CPU）为唯一合成通路**；将来接新引擎时**替换 / 扩充 `tts_zipvoice` 那一处即可**（**不建多引擎注册表**）。
- **用户追认（2026-10-12，三条，`docs/todo.md` 记录）**：① **前端 `PetConfig.tts`（localStorage，「要不要读」的 UI 开关）与宿主 `PetVoiceConfig.enabled`（宿主启用位）保持两套**，**不合并**（职责不同源，各自保留）；② **音色缓存清理保持现状**（导入时删掉缓存目录里其它 `*.wav`；已知边界：导入成功但 `set_pet_voice` 随后失败时会短暂指向已删文件，表现 = **静默不读**，安全）；③ **「试听」按钮保留**。
- **第二轮（2026-10-12）· ZipVoice 正式分发 + 第三方授权声明**：用户口径 =「本项目在 GitHub 公开发布、**声明个人项目且不做商用**」⇒ CC-BY-NC 权重**可随项目再分发**（须署名 + 随附许可 + 传递 NC + 不并入 MIT + 不涉商业化）。落地：① **archive** `release\deps\zipvoice\zipvoice-1.13.8.7z`（**149.14 MiB**，sha256 `FD7476381CC426E00EE5123A8DEBA51CB967094445FD511F874A90D0E8D198B1`；内含单层 `zipvoice/` + `LICENSES/` + `THIRD_PARTY_NOTICES.md`；`espeak-ng-data` **保留全语言** = 不改动上游数据）；② `scripts\build-plugins.ps1` 的 pet 块加第 2 条依赖 `type="archive"`、`dest="zipvoice"`、`local="deps\zipvoice\_stage\zipvoice"` + **6 条 `notices`**（`:221-240`），并给 `Stage-PluginDir` 加 archive 分支（`:508`）——`dest` 语义经 **`plugin_market.rs` 的 `install_archive_dependency` → `safe_join(plugin_dir, dest)`** 核对，与 `tts_zipvoice.rs` 的 `zipvoice_dir()`/`assets_ready()` **逐字对齐**；**剔除 `sherpa-onnx-cxx-api.dll`**（`objdump` 实测 C API 不 import 它）、`vocos_24khz.onnx` **从 `_dl\` 补到根**；③ 仓库新增 `licenses\zipvoice\`：**CC-BY-NC-4.0 / Apache-2.0 / ONNX-Runtime-MIT / GPL-3.0 / Vocos-MIT** 全文（**均取上游原文**）+ `THIRD_PARTY_NOTICES.md`（组件 × 授权 × 来源 × 义务 + 非商用声明 + ZipVoice 署名 + espeak-ng 源码指引）；**Vocos 查证 = MIT**（`gemelo-ai/vocos`，原话针对代码、权重未逐字声明，已如实标注）；④ `README.md`「第三方组件与许可」补 6 行 + **明确「MIT 仅覆盖本项目原创代码」**，`LICENSE` 末尾加同义声明（**未改 LICENSE 本体类型**）；`PluginManifest` **无** license/notices 字段 ⇒ 授权说明由插件目录内 `zipvoice\LICENSES\` 承载；⑤ pet 插件版本 **0.9.53 → 0.9.54**；⑥ 验证：`build-plugins.ps1` exit 0、`Modules\pet\zipvoice\` 四件必需文件全在（⇒ **`assets_ready()` 首次为真**）、`npm run verify` exit 0（BOM 自检 [8/8] 通过）、`tsc` 0、`cargo test` **213 passed**；⑦ ⚠️ **遗留（需用户操作）**：上传 Release —— 标签 **`zipvoice-1.13.8`**、文件名 **`zipvoice-1.13.8.7z`**（必须逐字一致，否则市场因 sha256 对不上整包拒装）；**上传成功前不要发布插件**（清单已声明该依赖，Release 不存在会让市场新装/更新 `pet` 因依赖 404 失败）。
- **涉及文件**：`app/src-tauri/src/tts.rs`、`app/src-tauri/src/tts_zipvoice.rs`、`app/src-tauri/Cargo.toml`、`app/src-tauri/src/main.rs`（注册）、`app/src/plugins/builtin/pet.ts`、`app/src/i18n.ts`；顺带：`scripts/agent-e2e.ps1` / `scripts/build-plugins.ps1`（**仅 BOM 归一**）。

#### §待办 1-4 补 桌宠 ZipVoice 语音资产分发 + 第三方授权声明（2026-10-12）

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

#### §待办 1-4 补 桌宠输出框「打字机」逐字动画 + 修「上下空白框」+ 语速跟随（2026-10-12）

- **用户原话（2026-10-12，按此实现）**：「桌宠侧的对话框还是可能上方和下方出现空白框。由于可能我们要添加输出键入出现动画效果 —— **每一秒出现 2 个中文字、2 个整体单词，标点符号迅速跳过**。能否**直接重新渲染输出**，使其**跳过可能出现的换行符**，并且能够**跟随最后出现的字符自动向下滑动**。上文添加的**语音语速也跟随这个动画效果的速率**。」
- **补充裁决（同日两条，已落地）**：① **删掉**上一轮遗留的临时诊断打点（`pet.ts` 里注释自述「查清后删」的 `[pet] output`，原 `pet.ts:4202-4206`）—— 已删，**dev 产物实测残留 = 0**；② **换轮时上一轮仍在打字动画中 ⇒ 保持现状**（直接停下、不补显全文；只有「中断 / 隐藏 / 收起」才走 `stopTypewriter(true)` 补显全文）。
- **A 修「上方 / 下方空白框」**：
  - **成因（文件:行号 + 证据）**：`.pet-chat-output` 用了 `white-space:pre-wrap`（`app/src/plugins/builtin/pet.ts:3019`），它把回复文本里的**换行 / 连续空行**渲染成盒子内的**真实空白块**（含首部换行 → 上方空白；中间连续空行 → 下方空白）；而改前的 `setChatText` 只做了 `text.replace(/\s+$/, "")`（**只去尾部**空白），**首部换行与内部连续空行原样落进 DOM** ⇒ 出现上方 / 下方空白框。（容器本身高度按内容收缩、无固定 max-height 撑空白；故根因是 `pre-wrap` + 未归一文本，不是容器高度。）
  - **修法（与 C 的「跳过换行」同源）**：所有写进气泡的正文一律先过 **`normalizeReplyText`**（换行 / `\r` / `\t` → 单个空格；连续空白折叠；首尾空行丢弃）—— 见 `setChatText`（`pet.ts:3713`）与 `startTypewriter`（`pet.ts:3766`）。`pre-wrap` 保留只为兜底单个空格，不再产生空白块。**没有**用 `display:none` 之类藏内容充数。
- **B 打字机式输出动画（核心）**：
  - **纯逻辑模块**：`app/src/plugins/builtin/pet-typewriter.ts`（无 DOM / 无 tauri，可 node 直接单测）。口径：**「每秒 N 个单位」单一节拍器**。
  - **单位切分（`splitUnits`）**：全角字符（中日韩 / 假名 / 谚文 / 全角形式）**每个各算 1 个单位**（用显式码点区段判定，**不用** `\p{Script=Han}` —— 它不含假名 / 谚文 / 全角标点）；连续的西文 / 数字串**按空白切成 1 个「词」单位**（不是按字母）；空白并进**前一个**单位；**标点（`\p{P}` / `\p{S}`）吸附到前一个单位**（行首标点粘到下一个单位）—— **不占自己的节拍**。不变量：对已归一文本 `splitUnits(t).join("")===t`。
  - **归一（`normalizeReplyText`）**：换行 → 单个空格、连续空白折叠、首尾空行丢弃（同 A）。
  - **「直接重新渲染输出」**：每拍**重渲染「已揭示的前缀」**——只写 `chatText.textContent`（同一个文本节点），**不追加 DOM**（`pet.ts:3791` 的 `setInterval` 回调），天然无追加式残留。
  - **跟随最后字符自动下滑**：每拍 `scrollChatToBottom()`（`chatOut.scrollTop = scrollHeight`）；但**用户手动上滑即暂停**（`chatOut` 的 scroll 监听维护 `followBottom`，`pet.ts:3239`），回到底部再恢复。
  - **收尾**：`stopTypewriter(showFull)`（`pet.ts:3744`）清定时器；**用户中断（中止按钮）/ 新一轮（`sendToAI` → `setChatText`）/ 收起 / 隐藏或最小化（`applyHidden`）/ 手动关闭**都会调它 ⇒ **立即显全文 + 清定时器，不留悬挂 interval，不在隐藏时继续跑**；自然跑完走 `finishTyping`。
  - **上限保护**：`shouldAnimateDirectly(unitCount, rate, ANIM_MAX_MS)` —— 预估总时长 `unitCount/rate > ANIM_MAX_MS`（=120 秒）或空文本 ⇒ **直接整体显示**（例：2000 字 @2/秒 ⇒ 直接显示，不跑十几分钟动画）。
  - **与既有自动收起（`PROACTIVE_BOX_MS`=6000）的关系**：`showOutputBox()` 只「弹框」；自动收起改由 **`scheduleOutputAutoHide()`** 承担，**在逐字动画跑完（或直接整体显示）后**才起算 ⇒ **动画有多长，气泡就至少多留 6 秒**，不会被中途收起。用户自己开着输入对话（`chatOpen` 真）时 `outputBoxAuto` 为假 ⇒ 不自动收（沿用旧口径）。
- **C 语音语速跟随动画速率**：
  - **唯一真相源 = 动画速率**（`PetConfig.speechRate`，单位/秒，可调范围 `1..4`，默认 `2`，见 `pet.ts:229/264/378`）。设置窗「语音朗读」区块新增一枚拉条 `#pet-tts-rate`（`pet.ts:575`，`onInput` 处理 `pet.ts:2347`）。
  - **折算公式**：`speed = clamp(rate / 2, 0.5, 2.0)`（`rateToVoiceSpeed`）。速率范围 `1..4` ⇒ 折算后**恰好覆盖宿主 clamp 区间 `0.5..=2.0`**（`app/src-tauri/src/tts_zipvoice.rs:476` 的 `cfg.speed.clamp(0.5, 2.0)`）；默认 `rate=2 ⇒ speed=1.0`。写回走**既有 `set_pet_voice`**（**共用同一份 `PetVoiceConfig`，不新增第二套配置**）；`refreshPetVoiceStatus` 读到状态后再对账一次（覆盖「改速率时语音状态还没读回」的窗口期）。
  - ⚠️ **局限（如实说明）**：ZipVoice **只给整体语速（时间尺度）参数、没有词级时间戳** ⇒ 动画与音频**只能「整体节奏一致」，无法逐字同步**（TTS 与动画同时起跑、随同一速率同向变化）。此条已写进 `pet-typewriter.ts` 的 `rateToVoiceSpeed` 注释与 i18n 的 `pet.tts_rate_note`。
- **D 合规与风格**：i18n 五语言齐全（`app/src/i18n.ts` 的 `pet.tts_rate_label` / `pet.tts_rate_note`）；样式**复用既有 `.pet-range` / `.pet-val` / `.pet-row`**，未新造颜色（只用 `--text-dim` / `--border-glass` / `--shade-rgb` / `--other-overlay-rgb` 等四类派生变量）。
- **验证（2026-10-12，逐条退出码）**：
  - `powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1`（仓库根）⇒ **exit 0**；dev 产物 `app\src-tauri\target\debug\Modules\pet\index.js` 经 `Select-String` 命中新标识 `speechRate`×5 / `pet-tts-rate`×4（证据）。
  - `npx tsc --noEmit`（`app`）⇒ **exit 0**；`npm run build`（`app`）⇒ **exit 0**；`npm run verify`（仓库根）⇒ **exit 0**（`[8/8]` BOM 自检 18 份通过；**本轮未改任何 `.ps1`**）。
  - **纯逻辑单测**：`node --experimental-strip-types scripts/pet-typewriter-selftest.mjs` ⇒ **exit 0 / 15 项全过**（本仓库**没有前端单测运行器** —— package.json 只有 tsc / vite，无 vitest / jest；故按 `testing.md` §测试 5 既有惯例用可被 node 直接跑的 `.mjs` 自检脚本）。覆盖：中英混排、标点吸附、连续换行 / 首尾换行（归一）、空串 / 纯空白、超长上限、速率折算与越界夹取、join 不变量。
- **真机验收口径（视觉 / 音频无法无头验，须起 dev 实例点一遍）**：
  1. **上方 / 下方不再出现空白框**：让模型回一段**以换行开头 / 中间夹多个空行**的文本（如 `"\n\n第一句\n\n\n第二句\n"`），气泡应只有「第一句 第二句」，**上方无空行、中间无空白块、下方无残留**（对照改前会多出空白框）。
  2. **节奏可核对（2 单位/秒）**：样例① 20 个中文字（如「春眠不觉晓处处闻啼鸟夜来风雨声花落知多少」）⇒ 揭示 20 拍、**预期总时长 10 秒**；样例② 10 个英文词（`one two … ten`）⇒ 揭示 10 拍、**预期总时长 5 秒**（首字到全文可见）。
  3. **标点不等拍**：「你好，世界。」= 4 拍（`你` `好，` `世` `界。`），逗号 / 句号随前一字**立即出现**，不会各自停一拍。
  4. **自动下滑跟随**：长回复（超 75px 出滚动条）逐字出现时自动滚到底、最后揭示的字符始终在视野内；**手动上滑后暂停跟随**（继续出字但不抢滚动），回到底部恢复跟随。
  5. **中断 / 换轮 / 隐藏立刻显全文**：动画中途点「中止」、或发新一轮、或隐藏 / 最小化桌宠窗 ⇒ 正文**立刻补齐为完整文本**且计时器停止（无残留继续跑）。
  6. **改速率后语音随之变化**：设「语速 / 打字速度」拉条（如 `rate=4` ⇒ `speed=2.0` 上限；`rate=1` ⇒ `speed=0.5` 下限；默认 `2 ⇒ 1.0`）后，朗读语速与打字节奏**同向变化**（受局限：仅整体一致）。
- **待裁定 / 需知晓**：① **无法逐字同步音频**（无词级时间戳，只能整体节奏一致）—— 已在代码与 i18n 注明，未假装精确；② **本仓库无前端单测设施**（无 vitest / jest），已按既有惯例用 node 自检脚本 + 说明；③ 换轮时 `setChatText` 走 `stopTypewriter(false)`（只清定时器、不补显上一轮全文，因为气泡马上被「思考中」覆盖）；隐藏 / 收起 / 中断走 `stopTypewriter(true)`（补显全文）—— 若要求「换轮也补显上一轮全文」再议；④ 上一轮遗留的 `pet.ts` 临时诊断打点（`[pet] output`，`CHAT_REPLY_EVENT` 处理器内）**本轮未动**（顺带记一笔）。
- **涉及文件**：新增 `app/src/plugins/builtin/pet-typewriter.ts`、`scripts/pet-typewriter-selftest.mjs`；改 `app/src/plugins/builtin/pet.ts`（导入 / `PetConfig.speechRate` / ttsBlock 速率拉条 / onInput / `setChatText` + 打字机引擎 / `showOutputBox` + `scheduleOutputAutoHide` / `setChatOpen` / `applyHidden` / `CHAT_REPLY_EVENT`）、`app/src/i18n.ts`（5 语言 2 键）。**未改宿主 Rust**（`speed` 字段与 clamp 已存在）。

#### §待办 1-4 补 桌宠语音：删提示文案 + 拆「打字速度 / 语音语速」两条 + 新增「语音音量」（2026-10-12）

- **用户原话（2026-10-12，按此实现）**：「1. 用你导入的参考音色在本机朗读（离线、不联网）：没有配好音色或缺少语音组件时会静默跳过 不弹错 不打扰 配置不齐时不会朗读静默处理」……「删除这一些提示 并且是否能将**打字速度与语速分开来成两个条 并增加语音音量拉条**」。
- **A 删掉的提示文案 + i18n 键（五语言删净，无死键）**：删 `pet.ts` `ttsBlock` 里三条 `pet-note` DOM 行 + `i18n.ts` 三键 —— `pet.tts_note`（「用你导入的参考音色在本机朗读（离线、不联网）…静默跳过」）、`pet.tts_silent_note`（「配置不齐时不会朗读（静默处理…）」）、`pet.tts_rate_note`（「每秒出现的字数…标点随前一个字立刻出现…」）。**保留**状态行 `#pet-tts-status`（就绪 / 未就绪两类状态 + 机器可读 `reason` 落文案，含「语音组件未就绪」）与导入反馈行 `#pet-tts-msg`（导入成功 / 失败提示，属操作反馈非说明性小字，见「待裁定」）。
- **B 三条拉条相互独立**（`PetConfig` 与 `PetVoiceConfig` 仍是各自那一份，**不新开第二套存储**）：
  1. **打字速度** `#pet-tts-rate`（`pet.ts:600`）：只驱动输出框**逐字动画**节拍；沿用 `PetConfig.speechRate`（单位/秒，范围 `RATE_MIN..RATE_MAX` = 1–4，默认 2），`onInput` 只 `saveConfig`，**不再折算语音语速**（`pet.ts:2378`）。
  2. **语音语速** `#pet-tts-speed`（`pet.ts:605`）：只驱动宿主 `PetVoiceConfig.speed`，经既有 `set_pet_voice` 保存（`applyVoiceSpeed`，`pet.ts:2574`）。滑条范围 `0.5..=2.0`（= 宿主 clamp，映射 1:1，step 0.1），回填见 `paintPetVoiceStatus`（`pet.ts:2529`）。
  3. **语音音量** `#pet-tts-volume`（`pet.ts:610`）：只驱动宿主 `PetVoiceConfig.volume`，经 `set_pet_voice` 保存（`applyVoiceVolume`，`pet.ts:2586`）。滑条范围 `0.0..=1.0`（映射 1:1，step 0.05，显示为百分比）。**为何上限取 1 而非 2**：音量是对每条样本的乘数，>1 会削顶失真。
- **C 音量字段与生效点（宿主 Rust）**：`PetVoiceConfig` 新增 **`volume: f32`**（`tts_zipvoice.rs:222`，**`#[serde(default = "d_one")]`** ⇒ 老 `pet-voice.json` 缺字段回落 **1.0** 不解析失败）；clamp 常量 `VOLUME_MIN=0.0` / `VOLUME_MAX=1.0`（`tts_zipvoice.rs:229`）与 `SPEED_MIN=0.5` / `SPEED_MAX=2.0`，夹取方法 `clamped_volume()` / `clamped_speed()`（`tts_zipvoice.rs:233`）；`speak()` 的 `speed` 改用 `cfg.clamped_speed()`（`tts_zipvoice.rs:500`）。
  - **音量怎么真正生效**：**rodio 0.22.2 的 `Player` 确有 `set_volume(Float)`**（`rodio-0.22.2/src/player.rs:178`，`Float = f32`；`Player` 会把每条样本乘以该值，1.0 = 原始）⇒ 直接用它，**不是假装生效**。落地在 `tts.rs`：播放前在**共享收尾** `play_wav_bytes` **集中读一次** `load_config().clamped_volume()`（`tts.rs:169`），传给 `play_wav(..., volume)` 并 `player.set_volume(volume)`（`tts.rs:196`）。因所有朗读通路都收尾到 `play_wav_bytes`，音量对 `tts_zipvoice` 那条通路（当前唯一）一致生效。
- **D 死代码清理（随 B 必须做）**：打字速度与语音语速解耦后 `pet-typewriter.ts` 的 `rateToVoiceSpeed` 失去调用者 ⇒ 连同 `pet.ts` 的导入、`scripts/pet-typewriter-selftest.mjs` 的对应断言一并删除；`RATE_MAX` 注释、`PetConfig.speechRate` 注释、`MOUTH_EVENT` / `startTypewriter` / `petVoice` 声明处注释同步改成「与语音语速相互独立」。
- **验证（2026-10-12，逐条退出码）**：
  - `powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1`（仓库根）⇒ **exit 0**。dev 产物 `app\src-tauri\target\debug\Modules\pet\index.js` 经 `Select-String` 实测：删掉的文案「静默跳过 / 配置不齐时不会朗读 / 每秒出现的字数 / 语音语速随之同步 / 没有配好音色或缺少语音组件」**全部 0 命中**；新标识 `pet-tts-speed`×6 / `pet-tts-volume`×6 / `pet-tts-rate`×4 **命中**。
  - `cargo test`（`app\src-tauri`）⇒ **exit 0 / 214 passed / 0 failed / 2 ignored**（新增 1 条 `tts_zipvoice::tests::voice_volume_default_and_clamps`：`volume` 缺省与「老 json 缺字段」回落、`clamped_volume` 边界 `-1→0 / 5→1`、`clamped_speed` 边界 `0.1→0.5 / 9→2.0`）；`cargo build --release`（同 cwd）⇒ **exit 0**（warning 均为既有 `proxy_server.rs` 死代码，与本轮无关）。
  - `npx tsc --noEmit`（`app`）⇒ **exit 0**；`npm run build`（`app`）⇒ **exit 0**（构建产物 `dist\assets\i18n-*.js` 已含 `pet.tts_speed_label` / `pet.tts_volume_label`、不含被删三键文案）；`npm run verify`（仓库根）⇒ **exit 0**（`[8/8]` BOM 自检 18 份通过；**本轮未改任何 `.ps1`**）。
  - 纯逻辑自检：`node --experimental-strip-types scripts/pet-typewriter-selftest.mjs` ⇒ **exit 0 / 13 项全过**（移除 3 条折算断言、保留/改写的夹取断言通过）。
- **真机验收口径（视觉 / 音频无法无头验，须起 dev 实例点一遍）**：
  1. 设置窗「语音朗读」块内**不再出现**三条说明小字；**只剩** `#pet-tts-status` 就绪状态行（就绪显示「已配置：参考音色可用」；未配齐显示「未就绪 · <原因>」）。
  2. **三条拉条各自独立可观察**：拖「打字速度」只改输出框**逐字变快/变慢**（@2 = 每秒 2 单位；@4 更快），**不影响**朗读语速与音量；拖「语音语速」只改**朗读声音快慢**（0.5x 慢 / 2.0x 快），不影响打字动画；拖「语音音量」只改**声音大小**（0% 静音 / 100% 原始），不影响两者。三处配置重启后仍在（分别落 `PetConfig`(localStorage) 与 `PetVoiceConfig`(pet-voice.json)）。
  3. 音量拖到 0 ⇒ 朗读**无声**但打字动画 / 口型时间线照常（音量只作用于 rodio 输出）；拖到 100% 与原行为一致。
- **涉及文件**：`app/src/plugins/builtin/pet.ts`、`app/src/plugins/builtin/pet-typewriter.ts`、`app/src/i18n.ts`、`app/src-tauri/src/tts.rs`、`app/src-tauri/src/tts_zipvoice.rs`、`scripts/pet-typewriter-selftest.mjs`。⚠️ **改到宿主（`tts.rs` / `tts_zipvoice.rs`）⇒ 需重编 `lunac.exe`**；改插件源码已跑 `build-plugins.ps1` 铺 dev。
- **待裁定 / 需知晓**：① `volume` 上限取 **1.0**（不取 2.0，避免削顶失真）—— 若用户要「放大」再议；② `#pet-tts-msg`（导入成功 / 失败反馈）**保留**，未算作「说明性小字」；③ 顺带发现（**未改**）：`refreshPetVoiceStatus` 里两条拉条的回填依赖 `get_pet_voice` 返回，首次渲染到回填之间有极短窗口（拉条短暂显示默认 1.0 / 100%）；④ 第 3 条涉及的 **Live2D 参数 / 预设未动**（按约束）。

#### §待办 1-4 补 桌宠模型参数：写入层夹取 + 物理驱动标注 + 档位选择器 + 预设按钮化（2026-10-12）

- **⚠️ 已按用户要求回退（2026-10-12）：本批属误解 —— 用户原意是「查证模型 `.vtube`/`.cdi3`/`.physics3` 三个文件里那些参数能否正确改模型、范围是否为 `-1..1`」，而非「加档位 / 预设按钮把参数固定到某值」。**代码已退回本批之前：写入层夹取（`clampParamValue` / `resolveParamRange` / `MountOptions.paramRangeFallback`）、档位选择器（`pet-gear` / `pet-gears` / `data-param-gear` / `data-gears` / `isGearParam` / `GEAR_MAX_SPAN` / `gearValues` / `restoreParams` 回填高亮 / 点击分支 / CSS）、物理驱动标注（`.pet-phys-note` / `pet.debug_phys_driven_note` / `Cdi3Names.physicsDriven` / `cdi3PhysicsParamIds`）均已删除；新增文件 `app/src/plugins/builtin/pet-param-logic.ts` 与 `scripts/pet-param-logic-selftest.mjs` 已整体删除；`pet-hotkeys.ts` 恢复为原私有 `physicsParams`（`physicsParamIds` 已被移除）。以下原文**保留**，仅作历史记录。

- **用户裁决（本轮四条，全部按此实现）**：① **范围口径以模型 moc3（Cubism Core 运行期）为准**，**不引入** `.vtube.json` 的 `InputRange/OutputRange`（那是 VTS 输入→输出映射，实测更窄）；② **物理驱动参数保留可控 + 加标注 + 补夹取**；③ **离散档位参数**（如发型的档位那种）**做成档位选择器**，不是裸滑条；④ **模型自带的表情预设 → 复用现有热键 / 表情机制做成可点按钮**。
- **⚠️ 法规纪律（硬，遵守裁决 5）**：只允许**运行期读用户本机模型目录里的文件**；本轮**没有**把任何具体模型的参数 Id / 名称 / 数值写进仓库代码或文档 —— 读取逻辑**只认公开 schema 的字段名**（`PhysicsSettings[].Output[].Destination`、`ParameterGroups[].Name/Parameters`），档位数 / 范围**一律运行时从 Core 或模型文件取**。
- **新增纯逻辑模块 `app/src/plugins/builtin/pet-param-logic.ts`**（无 DOM / 无 tauri，可被 node 直接单测；`pet.ts` 与 `live2d-engine.ts` 各自内联）：`clampParamValue`（`:31`）、`resolveParamRange`（`:40`）、`isGearParam`（`:51`）、`gearValues`（`:60`）、`physicsParamIds`（`:79`，替换掉 `pet-hotkeys.ts` 里旧的私有 `physicsParams`，避免第二份实现）、`cdi3PhysicsParamIds`（`:103`）。
- **A 写入层范围夹取**（最关键，堵住「程序化写入可越界」）：
  - **唯一夹取点 = 引擎 `setParameter`**：`app/src/plugins/builtin/live2d-engine.ts:674-684` —— 写入前 `clampParamValue(value, rangeOf(id))`，`heldParams` 存**夹取后的值**（每帧写回自然也在范围内），再立即写 Core。**所有程序化写入路径都汇总到它**：设置窗滑条 / 数值框（`pet.ts` 的 `emitParamThrottled` → `set-param` 广播 → `pet.ts:4705`）、`reset-params`（`pet.ts:4711`）、热键 run / toggle（`pet.ts:4403` / `:4528`）、AI `param` 动作（`pet.ts:4595`，含 `PetControl` 越界值）、以及引擎内的**叠加表情层**（`live2d-engine.ts:540`）与**口型**（`:550`）。
  - **`add` 模式夹的是叠加后的结果**：调用方先 `getParameter + value` 算好和再进 `setParameter`（`pet.ts:4403`），夹取作用在**和**上。
  - **范围优先级（与现状一致）**：运行期 Core 的 `paramRanges[id]`（moc3 自己的 min/max）→ 缺失回落 `pet.ts` 的静态 `DEBUG_PARAMS`（经 `DEBUG_PARAM_RANGES`，`pet.ts:186`，随 `mountLive2D` 的 `paramRangeFallback` 传入，`pet.ts:3511` / `live2d.ts:306`）→ 再回落 `[-100,100]`（`DEFAULT_PARAM_RANGE`）。夹取**只发生在写入边界**，不改「保持 / 每帧压过物理」的既有语义（`applyHeld` 的写回次序不变）。
- **B 物理驱动参数：判定 + 保留可控 + 标注**：
  - **判定双来源**（`pet.ts:2054`）：**首选** `enumerateTargets` 的 `physicsOutputs`（来自模型 `*.physics3.json` 的 `PhysicsSettings[].Output[].Destination`，字段 `Target`，缺时认 `Id`；文件不存在 / 结构不认识 ⇒ 静默空集）；**兜底** `.cdi3.json` 的 `ParameterGroups`（分组名含「物理」的组内的 `Parameters`，`pet-hotkeys.ts:386` 的 `cdi3.physicsDriven`）。**两条都拿不到 ⇒ 不做标注**（不影响功能），且**保留可控**（这些参数照旧可被滑条 / AI / 热键写入）。
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

#### §待办 1-4 补 桌宠输出框「上下空白」真因：模板缩进 × pre-wrap 幽灵空行（2026-10-13）

- **用户报障**：桌宠气泡里文字**上方 / 下方各多出一段空白**（不是回复文本里的换行造成的，而是固定多出来的）。
- **真因（文件:行号 + 无头实测）**：气泡模板（`app/src/plugins/builtin/pet.ts:3138-3139`）在 `.pet-chat-output` 与 `.pet-chat-text` 两行之间**夹着换行 + 缩进**（HTML 排版的空白文本节点），而 `.pet-chat-output` 带 `white-space:pre-wrap`（`pet.ts:3225`）、又是**块级**容器 ⇒ 按 CSS 规范把这段**纯空白子文本节点**当成真内容渲染成空行 ⇒ 文字顶部 / 底部各多约两行。（上一轮的 `normalizeReplyText` 只归一**回复文本本身**，管不到模板自带的这段空白。）无头 Chrome 实测：文字距盒顶 **42.84px**，而 `padding-top` 只有 6px；盒内容高 **104** > 下限 75，单行文本也平白多出一条滚动条。
- **修法（`app/src/plugins/builtin/pet.ts`）**：`.pet-chat-output` 由块级改 **flex 列**（`display:flex;flex-direction:column`，`:3226`）—— flex 容器**不渲染**纯空白子文本节点（规范按 `display:none` 处理）⇒ 与模板缩进无关地保证文字贴顶；`.pet-chat-text` 声明 `flex:0 0 auto`（`:3229` 附近）不许被压，超出走盒内滚。无正文时靠 `:has(> .pet-chat-text:empty){visibility:hidden}`（`:3230`）**不可见但仍占位** ⇒ 输出带回到下限 75px、开合对话时模型不位移。**没有**用 `display:none` 藏内容充数。
- **验证结论**：`npx tsc --noEmit`（app）exit 0；`npm run build`（app）exit 0；`npm run verify`（根）exit 0。**无头实测对照**：文字距盒顶 **42.84px → 6px**（= padding-top）、内容高 **104 → 75**（= 下限）、**单行不再出滚动条**。（模板两行 3138-3139 **未改**，只改 CSS 3225-3230。）
- **待真机验收**：① 一段**单行**中文回复 ⇒ 气泡内文字**贴顶**、上下无空行、**无滚动条**；② 对照改前（文字被顶到盒中偏下）。
- **涉及文件**：`app/src/plugins/builtin/pet.ts`（CSS 3225-3230）。

#### §待办 1-4 补 桌宠输出框随内容封顶 + 窗口可缩放（记忆 / 向上生长）+ 语音播完再隐（2026-10-13）

- **用户口径**：① 输出框**随内容变高但封顶**（超上限才盒内滚），短 / 中回复不再出滚动条；② 窗口**可拖拽缩放**、尺寸要**记住**；③ 变高时窗口**向上生长**（模型不位移）；④ 语音**播完再自动隐**。
- **A 输出框随内容封顶（纯逻辑 `app/src/plugins/builtin/pet-window-size.ts`）**：下限 `OUT_MIN_H = 75`（`pet-window-size.ts:21`）、上限 `OUT_CAP_H = 255`（`:43`）、20px 档位 `OUT_STEP = 20`（`:65`）。纯函数 `clampOutTop`（`:83`，内容→带高、夹 `[75,255]`、向上取整）、`quantizeTopH`（`:91`，逐字增长时**只在跨档才 resize**，把一次动画的 resize 次数从「每字一次」压到 ≤9 次）、`targetWindowHeight`（`:100`，= 基准高 + (带高 − 75)）、`baseHeightFromWindow`（`:125`）、`resizePlan`（`:133`）。落点在 `pet.ts` 的 `applyOutHeight`（`:3951`）与 `setChatText`（`:3979`）—— 逐字动画每拍调 `applyOutHeight(false)`（跨档节流），收尾 / 换内容调 `(true)`。
- **B 窗口可缩放 + 记忆 + 向上生长**：清单 pet `window` 段 `resizable = $true`（`scripts/build-plugins.ps1:265`）；前端「窗上缩放手柄」`.pet-resize-handle`（CSS `pet.ts:3256`、DOM `:3170`、mousedown → `startResizeDragging("SouthEast")` `:3818`）；权限 `core:window:allow-start-resize-dragging`（`app/src-tauri/capabilities/default.json`）；尺寸记忆落本插件 localStorage（`CFG_KEY = "lunac.pet.v1"` 的 `winW`/`winH`），`onResized` 去抖 250ms 记账（`pet.ts:5043`），开窗读回并 `clampWindowSize`（`:5031` 附近）；右键「恢复默认窗口大小」`data-act="win-reset"`（`:3162` / `:4611`）。**向上生长** = 宿主新命令 `plugin_window_resize_anchored`（`app/src-tauri/src/plugin_window.rs:736`，下沿锚定换算 `anchored_top` `:675`；命令注册在 `main.rs:815` 附近）：`set_size` 后把上沿按高度差反向搬一次 ⇒ 下沿（屏幕 y）不动，多出的高度全落在输出框上、模型区与输入带**不位移**。
- **C 语音播完再隐（`app/src-tauri/src/tts.rs` + `pet.ts`）**：宿主在**一次朗读结束之后**（播完 / 未就绪静默跳过 / 合成失败**都算**）定向发 `TTS_DONE_EVENT = "pet-tts-done"`（`tts.rs:33`，发送点 `tts.rs:242` 的 `tts_speak` 线程尾；前端同名常量 `pet.ts:136`）。前端 `beginTtsWait`（`:4403`）在收到最终回复且 `tts` 开时置位 + 起 **3 分钟看门狗**（`TTS_WAIT_MAX_MS = 180000`，`:4112`）；`endTtsWait`（`:4414`）由该事件或用户中断解除；`maybeScheduleAutoHide`（`:4425`）是**自动收起唯一判据**（本轮自动展开 + 可见 + 动画已停 + 语音已播完）⇒ 起算 `PROACTIVE_BOX_MS = 6000`（`:149`）。`tts` 关着时不置位 ⇒ 仍是「动画跑完即起算 ≈6s」的兜底。
- **验证结论**：`build-plugins.ps1`（根）exit 0（pet **0.9.58**）；`npx tsc --noEmit` exit 0；`npm run build` exit 0；`cargo test --bins`（app\src-tauri）exit 0（含 `anchored_top_keeps_the_bottom_edge_fixed`、`clamp_size_bounds_bad_and_out_of_range_values`）；`cargo build --release` exit 0。
- **待真机验收**：① 长回复 ⇒ 盒到 255 后**盒内滚动**、窗口随之变高（上沿上移、下沿不动、模型不位移）；② 拖边缘 / 角 / 手柄改尺寸 ⇒ 重启后**记住**；右键「恢复默认窗口大小」⇒ 回 320×510 基准；③ 开朗读时气泡**等语音播完**再隐（对照：关朗读 ⇒ 动画跑完 ≈6s 收）。
- **涉及文件**：`app/src/plugins/builtin/pet-window-size.ts`、`app/src/plugins/builtin/pet.ts`、`app/src-tauri/src/plugin_window.rs`、`app/src-tauri/src/tts.rs`、`app/src-tauri/src/main.rs`（注册新命令）、`app/src-tauri/capabilities/default.json`、`scripts/build-plugins.ps1`。

#### §待办 1-4 补 桌宠画布随窗 + 模型「不变只多空间」贴底（2026-10-13）

- **用户口径**：窗口变大时**模型不放大**、只是周围多出空间；画布（模型区）**跟随窗口**剩余空间、模型**贴底**。
- **A 「参考尺寸」语义变更**：`STAGE_H`（`pet-window-size.ts:25`）由「定尺 400」改为**参考模型区高**（缺省窗口下的 400），`STAGE_W`（`:28`）同理为参考宽 300。它们只作参考：模型基准缩放（只缩不放）与基准窗高换算。
- **B 画布跟随窗口（CSS）**：`.pet-stage` 改 `flex:1 1 auto`（`pet.ts:3191`）⇒ 吃掉「输出带 + 输入带」之外的全部高度（= 窗高 − 输出带 − 输入带），宽度 = 窗宽 − 左右留白（`--pet-stage-gap`）。`.pet-img` 改**定尺参考**（`width/height = var(--pet-stage-ref-w/h)`，缺省 300×400，`:3197`）+ `max-width/height:100%` + `object-position:bottom center` ⇒ 图片分支与模型分支行为一致（画布变大不放大、变小按需收、贴底）。参考值由 `attachPetWindow` 从常量注入 CSS 变量（`:3436-3438`）。
- **C 「只缩不放」缩放**：纯函数 `modelBaseScale`（`pet-window-size.ts:154`）= `min(参考基准, 画布基准)` ⇒ 画布比参考大 ⇒ 结果 = 参考基准（**不放大**）、比参考小 ⇒ 按画布收（**不裁切**）；由该式恒有「模型不溢出画布」。引擎侧消费见 `live2d-engine` 的 fit 与 `refStageW/refStageH` 传入（`pet.ts:3549`）。
- **验证结论**：纯逻辑单测 `scripts/pet-window-size-selftest.mjs` 覆盖「画布更大不放大 / 更小按画布收 / 纵横比不同取较小者 / 不溢出 / 宽度钳制」（本轮扩到 38 项，见下条）；`tsc` / `npm run build` / `npm run verify` exit 0。
- **待真机验收**：① 拖大窗口 ⇒ 模型**尺寸不变**、周围多出空间（且**贴底**）；② 拖小窗口 ⇒ 模型按画布收、不被裁切；③ 图片形象与 Live2D 表现一致。
- **涉及文件**：`app/src/plugins/builtin/pet.ts`、`app/src/plugins/builtin/pet-window-size.ts`、`app/src/plugins/builtin/live2d-engine.ts`（fit 参考口径）、`app/src/plugins/builtin/live2d.ts`（`refStageW/H`）。

#### §待办 1-4 补 桌宠「最小模型区」保护：动态输出上限 + 三处同源最小窗高（2026-10-13，本轮）

- **要消除的病态**：长回复（盒到 255）时把窗手动拖到最小高 ⇒ `255 + 35 > 窗高` ⇒ `.pet-stage` 被压到 **0**、模型不可见。
- **A 动态输出上限（`app/src/plugins/builtin/pet-window-size.ts`）**：新增 `outCap(winH) = clamp(winH − IN_H − STAGE_MIN_H, OUT_MIN_H, OUT_CAP_H)`（`:75-77`）；`clampOutTop` / `quantizeTopH` 增加 `cap` 参数（缺省 `OUT_CAP_H` ⇒ 旧行为逐字不变，`:83` / `:91`）；`resizePlan` 内部按 `outCap(baseH)` 取上限（`:133`）。`pet.ts` 的 `applyOutHeight` 用 `outCap(winBaseH)`（`:3955-3956`）。**为什么用「窗口基准高」而非「当前窗高」**：内容驱动生长满足 `窗高 = 基准高 + (带高 − 75)` ⇒ 模型区高 = 基准高 − 75 − 35 **与带高无关**，故以基准高推上限是**稳定**的；若用当前窗高，带越高窗越高、上限又越大，会自我放大成正反馈。窗矮 ⇒ 上限降到下限（不挤压模型区）；基准 510 ⇒ `510 − 175 = 335 ≥ 255` ⇒ 取 `OUT_CAP_H`（**正常路径完全不变**）。
- **B 抬高最小窗高（三处同源）**：新增 `STAGE_MIN_H = 140`（`pet-window-size.ts:35`），**唯一真相源**推导 `WIN_H_MIN = OUT_MIN_H + IN_H + STAGE_MIN_H = 75 + 35 + 140 = 250`（`:58`，原 200）。三处消费同一口径：① 前端 `clampWindowSize`（`:105`，用本模块常量）；② 清单 `window.minHeight = 250`（`scripts/build-plugins.ps1:264`，值 = 本式，注释声明勿写第二份）；③ 宿主 `plugin_window_resize` / `plugin_window_resize_anchored` 的 clamp（`plugin_window.rs:664` 的 `clamp_size` → `min_size` **读清单** `minHeight` ⇒ 天然同源）。⇒「窗高 < `OUT_MIN_H + IN_H + STAGE_MIN_H`」**根本不可达**。
- **C 手拖时先收窄盒高（`pet.ts` 的 `onResized`，`:5043-5051`）**：把窗拖矮 ⇒ `outCap(实际窗高)` 随之降低 ⇒ **先**把盒高收到新上限（**只收不放**：内容变短不在这里放大），再据「实际窗高 + 收窄后的盒高」反推基准高。否则盒停在旧高、`.pet-stage` 仍被压到 0。程序化下发恒满足 `盒高 ≤ outCap(窗高)`，故**不触发**、不影响既有记账（`baseHeightFromWindow` 恒等）。
- **D 尺寸记忆读回纠偏**：`loadConfig` / 开窗都过 `clampWindowSize`（`pet.ts:5031` 附近）⇒ 老用户存的过小高度（如 200）被抬到 250。
- **定值理由（`STAGE_MIN_H = 140`）**：① = 参考模型区高 `STAGE_H`(400) 的 **35%**；② = 最小窗口宽下的画布宽（`WIN_W_MIN − STAGE_GAP` = 160 − 20 = **140**）⇒ 最小合法窗口下模型区仍是一个「不低于其宽度」的竖向可辨认区域；③ 把最小窗高抬到 250（原 200），仍远低于 `WIN_BASE_H`(510) ⇒ 短回复 / 开合对话不改变窗口高。
- **验证结论**：纯逻辑单测 `node --experimental-strip-types scripts/pet-window-size-selftest.mjs` **exit 0 / 38 项全过**（本轮新增 12 项：动态上限边界「= 最小窗 ⇒ 75」「= 下限高 ⇒ 75」「大窗 ⇒ 255」「窗矮单调降且 ∈[75,255]」「非有限兜底」「clamp/quantize 按 cap 收窄」；最小窗高「推导式」「清单 minHeight/minWidth 逐字一致（读 `build-plugins.ps1` 断言）」「clamp 兜住老尺寸」；不变量「任意合法窗高下 `窗高 − 盒高 − IN_H ≥ STAGE_MIN_H`」「内容驱动路径同理」「`H = WIN_H_MIN` 时模型区恰为 `STAGE_MIN_H`」）。`npx tsc --noEmit`（app）exit 0；`npm run build`（app）exit 0；`npm run verify`（根）exit 0（`[8/8]` BOM 18 份）；`build-plugins.ps1`（根）exit 0（pet **0.9.58**、dev 产物 `Modules\pet\index.js` 含 `STAGE_MIN_H`×2）；`cargo test --bins`（app\src-tauri）exit 0（**230 passed / 0 failed / 2 ignored**）；`cargo build --release` exit 0（18 warning 均既有 `proxy_server.rs` 死代码，与本轮无关）。
- **待真机验收**：① 长回复把窗拖到**最小**（≥250）⇒ 输出框自动收到 75、**模型区仍在（≈140px）可见**、不塌成 0；② 拖回大窗 ⇒ 盒能重新长起来（内容仍长时到上限）；③ 正常窗（未手动改）下短 / 中回复行为与上一轮**完全一致**（窗口高不变）；④ 老 `winH=200` 的配置打开后窗口高 = 250。
- **涉及文件（本轮改动）**：`app/src/plugins/builtin/pet-window-size.ts`、`app/src/plugins/builtin/pet.ts`、`scripts/pet-window-size-selftest.mjs`、`scripts/build-plugins.ps1`（pet `minHeight` 250 + `version` 0.9.58）。**未改宿主 Rust**（宿主 clamp 读清单 `minHeight`，随清单同源生效）。
- **待裁定 / 需知晓**：① **清单 `minHeight` 与前端 / 宿主确为同源**：前端用 `pet-window-size.ts` 的 `WIN_H_MIN`，宿主读**同一份清单**的 `minHeight`（值由本模块推导、`build-plugins.ps1:264` 手工镜像）；单测已断言「清单 minHeight === WIN_H_MIN」，但**镜像仍需人工保持**（TS 常量无法被 ps1 直接 import）。② 动态上限取**基准高**（非当前窗高）是为避免正反馈 —— 已在 `outCap` 注释与单测钉死。

#### §待办 1-4 补 桌宠设置窗新增三页（主动反应 / 语音朗读 / 人格）+ 人格页四项接线（2026-10-13，上一轮补记）

- **用户口径**：设置窗按功能分页；主动反应、语音朗读各自独立成页；新增「人格」页，集中显示核心人格 + 事实记忆 + 成长笔记 + 人格建议四块。
- **A 新增三页（复用既有 tab 机制，未新造交互）**：页签按钮 `pet.ts:1534-1536`（`data-pet-tab` = `proactive` / `voice` / `persona`），对应页容器 `pet.ts:1561-1606`（`data-pet-page`，`hidden` 控制）；分页切换沿用既有委托 `pet.ts:2389-2398`（读 `data-pet-tab` → 切 `.pet-tab` 的 `on` + `.pet-tabpage` 的 `hidden` 互斥），**未新增第二套分页逻辑**。
- **B 主动反应 / 语音朗读控件「同文件搬迁」**：两块由内联改到独立的渲染函数 `proactiveBlock(cfg)`（`pet.ts:560`）/ `ttsBlock(cfg)`（`pet.ts:633`），分别注入各自页 —— **原处移除、控件 id 全不变**（`pet-proactive-enabled` / `pet-proactive-interval` / `pet-proactive-cap` / `pet-tts-enabled` …）、**落盘路径未动**（`PetConfig`（localStorage）与 `pet-voice.json` 照旧）。
- **C 人格页四项接线（只调既有宿主命令）**：① 核心人格 `get_pet_persona` / `set_pet_persona`（`loadPetPersona` `pet.ts:1935`、`savePetPersona` `pet.ts:1948`，点击委托 `pet.ts:2506-2513`）；② 事实记忆 / ③ 成长笔记 / ④ 人格建议三张列表走唯一映射 `PET_LIST`（`pet.ts:1961-1971`）+ `renderPetList` / `refreshPetList` / `deletePetListEntry` / `clearPetList` 一族（`pet.ts:1998-2089`），打开面板各拉一次（`pet.ts:3324-3326`）。**「接受」一条人格建议 = §8 事前二次确认后并入核心人格**（文案 `pet.suggest_accept_armed` + `armedRun` 两段式，`mergePetSuggestion` `pet.ts:2085`，委托 `pet.ts:2517-2524`）—— 与「清空」（只丢弃）语义区分。
- **为什么**：设置窗原先只有「模型调试 / 热键 / 动作」三页，主动反应与语音的设置散在页内、人格相关无统一入口；分页 + 同文件搬迁在**不改任何落盘路径与控件 id** 的前提下把功能归位。
- **验证结论（本轮复核，逐条退出码；`### §待办 1-5` 上方为同一工作树）**：`npx tsc --noEmit`（app）**exit 0**；`npm run build`（app）**exit 0**；`npm run verify`（根）**exit 0**（`[8/8]` BOM 18 份）；`cargo test --bins`（app\src-tauri）**exit 0 / 231 passed / 0 failed / 2 ignored**；`cargo build --release` **exit 0**（18 warning 均既有 `proxy_server.rs` 死代码）。开发产物 `app\src-tauri\target\debug\Modules\pet\lunac-plugin.json` 版本 **0.9.60**（本轮又 +1）。**注：本页对应上一轮，当时未单独记录**，以上为本轮同工作树的复核值。
- **待真机验收**：① 设置窗出现「主动反应 / 语音朗读 / 人格」三个页签，点切换互斥、选中态可见；② 主动反应、语音两条拉条 / 开关行为与搬迁前一致（值落盘、重启后保持）；③ 人格页四块齐全：保存 / 清空核心人格、逐条删 / 清空记忆与成长笔记均生效，「接受」建议前有二次确认、确认后并入核心人格。
- **涉及文件**：`app/src/plugins/builtin/pet.ts`（三页 + 两块搬迁 + 四项接线）、`app/src/i18n.ts`（相关文案）。

#### §待办 1-4 补 桌宠人格建议「清空」命令与接线（2026-10-13，本轮）

- **要补的缺口**：另两张列表（事实记忆 / 成长笔记）已有「清空」命令 + 前端「清空 + 二次确认」；**人格建议当时缺清空命令，故没给入口**。本轮只补这一条。
- **A 宿主命令 `clear_persona_suggestions`**：
  - `mcp_server.rs:896` 新增 `persona_suggestions_clear()` = `clear_memory_at(&persona_suggestions_path())`（截断 `ModuleData\memory\pet\PERSONA_SUGGESTIONS.md`；文件不存在 ⇒ 幂等空操作）—— **复用既有路径真相源 `persona_suggestions_path()` 与既有 `clear_memory_at`**，**未新增第二份常量 / 第二处路径拼接**（§62）。
  - `commands.rs:968` 新增 `#[tauri::command] pub fn clear_persona_suggestions()`；`main.rs` 导入（`:67`）+ `invoke_handler` 注册（`:565`），照同族命令体例。
- **B 接到人格页（与另两张表同款交互）**：建议区加「清空」按钮（`pet.ts:1606`，`class="pet-btn pet-list-clear"` + `data-pet-clear="suggest"` + `data-armed-text`），复用 `PET_LIST.suggest.clear = "clear_persona_suggestions"`（`pet.ts:1970`）与既有 `clearPetList`（`pet.ts:2077-2089`，本轮仅给它加一个**可选** `done` 轻提示参数；记忆 / 成长缺省 `""` ⇒ **行为不变**）；点击委托 `pet.ts:2548-2552` 走既有 `armedRun` **两段式确认**（4s 超时自动解除，全仓禁用原生 `confirm`）。清空成功刷新列表并给轻提示 `pet.suggest_cleared`。**「接受（合并）」= §8 事前确认后并入核心人格；「清空」= 只是丢弃建议、不写核心人格** —— 按钮与两段式确认文案均写明区别（`pet.suggest_clear_armed`）。
- **为什么**：三张列表的「清空」体验要一致；建议区补上唯一的清空入口，且必须与「接受」的写入语义明确区分。
- **验证结论（逐条退出码）**：`cargo test --bins`（app\src-tauri）**exit 0 / 231 passed**（新增 `mcp_server::tests::persona_suggestions_clear_only_truncates_its_own_file`：清空后列表为空 / 核心人格与 `MEMORY.md`、`GROWTH.md` 不动 / 文件不存在幂等——不报错、不新建）；`cargo build --release` **exit 0**（18 warning 均既有）；`npx tsc --noEmit` **exit 0**；`npm run build` **exit 0**；`npm run verify` **exit 0**（`[8/8]` BOM 18 份）；`build-plugins.ps1`（根）**exit 0**（pet **0.9.60**；dev `Modules\pet\index.js` 命中 `clear_persona_suggestions`×1 / `pet.suggest_clear_armed`×1 / `pet.suggest_cleared`×1 / `pet-suggest-clear`×1）。i18n 新 key `pet.suggest_clear_armed` / `pet.suggest_cleared` 各 **5/5 语言**。
- **待真机验收**：① 建议区有「清空」按钮，点一次变「再点一次清空建议…」、4s 内再点才清空（超时自动还原）；② 清空后建议列表变空、显示「已清空建议（核心人格未改动）」；③ 清空**不改**核心人格 / 事实记忆 / 成长笔记，与「接受」结果明显不同。
- **涉及文件**：`app/src-tauri/src/mcp_server.rs`、`app/src-tauri/src/commands.rs`、`app/src-tauri/src/main.rs`、`app/src/plugins/builtin/pet.ts`、`app/src/i18n.ts`、`scripts/build-plugins.ps1`（pet 版本 +1）。

#### §待办 1-4 补 桌宠人格页三张列表「清空」逻辑与提示归一（2026-10-13，本轮）

- **用户口径**：三张列表（事实记忆 / 成长笔记 / 人格建议）的清空点击与成功提示要归一 —— 消除两套点击匹配方式；三张表都显示轻提示且文案各自区分。
- **A 点击归一（消除两套匹配）**：原先记忆 / 成长走**硬编码 `#pet-memory-clear` / `#pet-growth-clear` 分支**、建议走 `data-pet-clear` 泛化分支 —— 两种匹配方式并存。本轮**删掉**那两段硬编码分支（`pet.ts` 旧 `:2542-2547`），三张表的「清空」按钮统一带 `data-pet-clear`（`memory` / `growth` / `suggest`，`pet.ts:1585` / `:1595` / `:1606`），点击全部由**唯一一支**委托（`pet.ts:2542-2550`）读 `data-pet-clear` → `clearPetList(kind)` 驱动，复用既有 `armedRun` **两段式确认**（4s 超时自动解除，行为不变）。**未改任何命令映射**（`clear_pet_memory` / `clear_pet_growth` / `clear_persona_suggestions` 照旧）、逐条删不受影响；同时删掉仅供旧建议分支使用的 `MSG_SUGGEST_CLEARED` 常量（不留死代码）。
- **B 三表统一轻提示（文案各自区分）**：`clearPetList` 去掉缺省 `done = ""` 参数，改为从唯一映射 `PET_LIST[k].cleared` 取文案（`pet.ts:1967-1971` + `:2079-2090`）。记忆 / 成长此前「只刷新、无提示」，本轮补上各自文案；建议沿用既有那句，保留「不改核心人格」专项说明：
  - 事实记忆 → 新增 `pet.memory_cleared`（「已清空事实记忆」）
  - 成长笔记 → 新增 `pet.growth_cleared`（「已清空成长笔记」）
  - 人格建议 → **复用** `pet.suggest_cleared`（「已清空建议（核心人格未改动）」）
  - 确认态文案：记忆 / 成长沿用 `pet.list_clear_armed`（「再点一次清空（不可恢复）」），建议沿用 `pet.suggest_clear_armed`；**未给记忆 / 成长照抄「核心人格未改动」那句**（它们没有该问题，照抄是误导）。
- **为什么**：三张表同款「清空 + 两段式确认」却有两套点击识别方式（硬编码 id vs `data-pet-clear`）+ 两套提示（一张有、两张无）—— 归一后单一匹配方式、单一映射表、一致的反馈；不变量（命令映射 / 超时行为 / 逐条删）保持不变。
- **验证结论（逐条退出码）**：`npx tsc --noEmit`（app）**exit 0**；`npm run build`（app）**exit 0**；`npm run verify`（根）**exit 0**（`[8/8]`）；`build-plugins.ps1`（根）**exit 0**（pet **0.9.61**）。dev `Modules\pet\index.js`：`data-pet-clear` 命中 **3** 处（`"memory"` / `"growth"` / `"suggest"` 各 1）、硬编码 `#pet-memory-clear` / `#pet-growth-clear` 分支**已不存在**（按钮 id 各仅 1 处，只作 DOM 标识）；新 key `pet.memory_cleared` / `pet.growth_cleared` 各 **5/5 语言**，`pet.suggest_cleared` 复用。
- **待真机验收**：① 人格页三张列表各有「清空」按钮，点一次变「再点一次清空…」（建议区为专用文案）、4s 内再点才清空、超时自动还原；② 清空后三张表**都**出现各自轻提示（记忆 / 成长 / 建议文案不同），建议那句含「核心人格未改动」；③ 清空命令映射与逐条删行为与改动前一致；④ 清空人格建议**不改**核心人格 / 事实记忆 / 成长笔记。
- **涉及文件**：`app/src/plugins/builtin/pet.ts`、`app/src/i18n.ts`、`scripts/build-plugins.ps1`（pet 版本 +1）。

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
  - **①B 复盘来源 / 时间标注** —— **本轮只做 B2（块头一行）**：`memory_block_text` 的引导段加**一行固定来源 / 时间说明**（英文，属固定前缀、逐字节稳定、按 scope 措辞：全局段点明「主 AI 保存」、桌宠段点明「桌宠私有」；并交代「未标注日期者视为背景」）。**B1（逐条日期 / 来源）仍待做**，且**需同步改去重语义**（现状去重 = 整条 bullet 完全相等）。
  - **本轮新增单测**：`review_gate_splits_session_quota_by_scope_and_shares_time_gate`（per-scope 配额 + 共享时间闸）、`memory_block_provenance_line_is_byte_stable_and_precedes_body`（说明行逐字节稳定 + 与正文分离）。

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

### §待办 1-7 清理测试夹具里的「伪凭据字面量」（GitHub secret scanning 误报）（2026-10-15，本轮）

- **误报根因**：`mask_secrets` / `content_safety` 的单测为验证「能识别 / 能脱敏」，直接写了**与真实凭据格式完全一致**的合成假值。GitHub **公开**仓库 secret scanning 命中的 5 条全部如此（Google ×2 / OpenAI / Stripe / Slack），**均非真密钥**。
- **改造口径（只改 `#[cfg(test)]` 夹具，不动任何生产逻辑）**：一律**拆段拼接**（`concat!` 或拆字符串字面量），**运行时字符串逐字不变** ⇒ 规则 / 脱敏照样命中、断言不放宽；但**源码文本里不再是一个连续 token** ⇒ 文本扫描器与 push protection 不再命中。
  - ⚠️ **拆点判据**：每个片段自身都**不成形**（前缀 + 长度不足 / 无 `@` / 无 `://` / 非三段 `eyJ`），即**任一单独片段都匹配不上伙伴格式正则**（已用与 `content_safety::RULES` 同款正则全仓复扫验证）。
  - **覆盖**：`AIza`(Google) / `sk-`(OpenAI) / `sk_live_`(Stripe) / `xoxb-`(Slack) / `sk-ant-`(Anthropic) / `ghp_`(GitHub) / `AKIA`(AWS) / 三段 `eyJ` JWT / PEM 私钥头 / `scheme://user:pass@host` 连接串。
- **⚠️ 告警需在 GitHub 手动 dismiss**：本仓**不改写 git 历史**（禁 `filter-repo` / `filter-branch` / `push --force`），旧字面量仍留在历史里 ⇒ 那 5 条告警**必须在 GitHub → Security → Secret scanning 里手动 dismiss（理由：Used in tests / 合成假值）**。代码侧的 `concat!` 只负责**阻断未来新增**，清不掉历史告警。
- **为什么没加本地自检**：一条「测试夹具不得含伙伴格式凭据字面量」的检查（扫 `src/**` 按伙伴正则、命中即 FAIL）本可低成本加，但既有校验链是 `scripts/verify-git.ps1` 的固定 `[8/8]` 段（.ps1 对 BOM 敏感，改它要重排分段与编号）⇒ 依「**别造第二套 + 保 `[8/8]`**」的约束**本轮不落地**，只在此记建议：后续如需，在既有 `[8/8]` 链里补一段**只读扫描**（`core-agent/src/**` + `app/src-tauri/src/**`，按 `content_safety::RULES` 的正则扫），命中即 `Bad` 并指路。
- **涉及文件**：`core-agent/src/content_safety.rs`、`core-agent/src/main.rs`、`app/src-tauri/src/log.rs`（均**仅** `#[cfg(test)]`）、`docs/archive/agent-feature-backlog.md`（一条 PEM 示例改成带省略号以断形）。

## §待办 2 已实现、只差一次真机验收

| # | 项 | 差哪一步 |
|---|---|---|
| **M2-1** | 卸载时随 NSIS 清理插件目录 | `nsis-hooks.nsh` 已加 `RMDir /r "$INSTDIR\Modules"`，**没重打包验证**；验完把 `Modules` 补进 §11 规则 12 的卸载清单 |
| **M2-2** | 开机自启守卫真机闭环 | 现有「dev 构建拒注册 + 能查旧注册项 + 一次修复入口」；**要安装版在登录时触发才算数** |
| **M2-3** | 任务抽屉与语义色按钮的视觉 / 手感 | 抽屉真实对话显隐、待办与已改文件两列表**同时展开**、语义色按钮没被盖掉——未做浏览器实测 |
| **M2-6** | 中断后「飞行中」的命令卡停在「执行中」 | 代码已完成（三处调 `markFlyingCardsStopped`）；**待真机确认**三条路径卡片都变黄「已停止」 |
| **M2-7** | 2026-10-01 批（内置技能 / AGENTS.md / MCP 远端 / 快照落盘） | ① ② ③ ⑤ 已端到端验收；**④ 快照落盘**（多会话 UI 回退）仍需真机 UI |
| **M2-8** | A17 结果卡上的凭据告警 | `cargo test` / `tsc` 已过，**无真机闭环**：让模型 Write 含私钥内容 → 结果卡应多一块红色告警 + 规则 / 行号、且自动摊开；Edit 删掉凭据时那块不应出现 |
| **M2-9** | 四类自定义颜色覆盖验收 | **已实现（2026-10-11）**：文字 / 图标 / 描边（`--border-glass` / `--ctx-border-glass`）/ 中性底（`--shade-rgb`）均已并入四类（`main.ts` 的 `applyFourClassNeutrals`），`tsc` 已过。**只差真机验收**：把「风格 → 底色 / 按钮线条 / 按钮背景 / 按钮蒙版（旧称「其他颜色」，2026-10-11 改名）」四类**全设 `#000000`** + 文字明度拉到极端，逐界面（搜索栏 / 结果区 / AI 输入栏 / 设置各页 / 各插件面板）扫，不应再出现非黑的按钮 / 拉条 / 文本框 / 文字 / 图标 / 描边颜色；**白名单**（语义色 / 音乐在播 / 内容色 / 取色器 / 反光白）见 `ui-style.md` §界面 11。**下拉框（2026-10-11 新增 §界面 12）**：设置里在用的自绘 `.custom-select` 已收编（本体走底色 + 按钮线条，选项悬停 / 选中走按钮蒙版 `-nav-alpha`）；全局滚动条：**2026-10-11 第 5 轮撤销「轨道描边」**（用户裁定此前要求是误判）⇒ 回归「**滑块实心无 border、轨道不描边**」（§界面 6 / §界面 12 已改，滚动条不再关联四类颜色；range 滑块本就无 border，其轨道亦不描边）；现存原生 `<select>`（用量成本 / translate / pet）弹出层不可样式化，属豁免，pet 的三处（热键行 / 动作行 / 动作链步骤）收合面已复核**未被压掉**并按 §界面 12 对齐（弹出层仍不可控）；**数字输入框（2026-10-11 新增 §界面 13）**：原生 spinner 不可美化 ⇒ 全局隐藏 + 自绘 `.num-stepper` 规范 —— 待真机确认上述观感。**音乐插件三处收编（2026-10-11 第 4 轮）**：播放 / 扫描进度条填充改走按钮线条、轨道改走底色派生；播放模式 / 随机按钮线条归按钮线条、激活态归按钮蒙版；选中歌曲选中框（`.music-tr.sel` / `.music-mt.sel`）改走按钮蒙版（正在播放仍靠文字强调区分）；music `version 0.9.35→0.9.36` 已重打包 —— 同样待真机按「四类全设黑」复扫 |
| **M2-10** | 桌宠动作链 / 热键页 5 项改动（2026-10-11 第 5 轮） | **已实现**：① 动作链**折叠 / 展开**（`.pet-chain-toggle` + `.pet-chain-summary`，折叠态一行摘要、过长省略）；② 换动作 / 新增步骤时**循环秒数默认 = 该动作 `Meta.Duration`**（只在切换那一次填，之后不覆盖）；③ 动作行**四个控件同一行**（`.pet-chain-fields` 不换行 + 数字框收窄到 `width:4.2em`）；④ **整链循环**开关（`MotionChain.loop` 落盘 `lunac.chains.json`，引擎末步播完从头再来）；⑤ 热键页**删「按键显示框格」**（改成小「录制」按钮 + 只读按键文本）、**表情对象 target 改可选**（下拉源 = `enumerateTargets().expressions`）。`tsc` exit 0；`pet 0.9.50→0.9.51` 已重打包。**待真机验收**：折叠 / 展开与摘要省略；换动作后循环秒数自动填成该动作时长、且手动改过后再切别的动作才重填；一行四控件不换行、数字看得清；开启整链循环后整条播完从头再来、关闭则播完即停（每步行内循环 / 硬停仍逐行生效）；按键录制仍能绑、按键文本可见、表情对象下拉能改且 `lunac.hotkeys.json` 的 `target` / `file` 同步。**注：其中「录制」按钮 + 只读按键文本已在第 6 轮删除（见 M2-11）；「类型 / 表情对象」下拉也已改自绘（见 M2-11）** |
| **M2-11** | 下拉框组件化 + 桌宠三处收编 + 去掉录制（2026-10-11 第 6 轮） | **已实现**：① 规范 `ui-style.md` §界面 12 **重写** —— 把参照组件 `.custom-select` **本身**逐字写进规范（组件名 / 定位、完整 markup、行为契约、四类颜色归属、与滚动条关系、明确反面；键盘 `↑↓`/`Enter`/`Esc` **如实标注未实现**）；② 组件化：`.custom-select` 的 CSS 从 `settings.ts` 内联样式**抽到共享 `app/src/styles.css`**（已核实插件悬浮窗 `plugin.html` 链接同一份 ⇒ 插件窗拿得到），交互抽成共享模块 `plugins/builtin/custom-select.ts`（`customSelectHtml` / `attachCustomSelect` / `closeAllCustomSelects`），`settings.ts` 改用它（× 删除 / `__custom__` 内联输入 / 面板裁剪**行为不变**）；③ 桌宠**三处**（热键「类型」/「表情对象 target」/动作链「动作文件」）由原生 `<select>` 改 `.custom-select`，选定写回原状态并走原落盘（`scheduleSaveHotkeys` / `commitChainFile`→`persistChains`）；④ 删热键页「录制」按钮 + `.pet-hk-keytext`（DOM / CSS / `hkCapturing` / 录键 `keydown` 一并清），按键数据仍落 `lunac.hotkeys.json`（`assignDefaultKeys` 补默认；**UI 不再有改绑入口**，有意由 AI 改）。`npx tsc --noEmit` exit 0；`pet 0.9.51→0.9.52` 已重打包（`build-plugins.ps1` exit 0，BOM 校验前 3 字节 `EF BB BF` 恰一份）。**待真机验收**：设置各页下拉仍正常开合 / 选择 / × 删除 / 自定义输入、外点关闭与边界翻转一致；桌宠「类型 / 表情对象 / 动作文件」三个下拉外观与设置一致、选定后热键防抖落盘与动作链落盘生效、换动作仍自动填该动作时长；热键页无录制按钮 / 按键文本、其余手工编辑（名称 / 目标 / 删除）仍落盘 |
| **M2-12** | 桌宠「输出框」两处 bug 修复（2026-10-11 第 7 轮） | **已实现（代码 + tsc + 打包均过）**：① **问题 2（渲染形态）**——`.pet-chat-output` 由固定 `height:75px` 改成 `max-height:75px`（**高度按内容收缩**、短回复不再留大片空白）+ 显式 `text-align:left`（从顶部渲染、不垂直居中）+ `setChatText` 去掉正文末尾换行 / 空白（`white-space:pre-wrap` 会把它们渲染成真实空行 = 用户说的「文本结束后还留一大块空白」）；滚动条走 §界面 6 那条全局规则（未另写）。② **问题 1（自动弹出）**——此前只在最终回复 `pet-chat-reply` 到达那一刻弹一次，现改为**输出开始就弹**（`sendToAI` 里 `if (!chatOpen) showOutputBox(false)`，让用户立刻看到「思考中」），最终回复到达再 `showOutputBox(true)` 刷新并进入 6s 自动收起；`flashOutputOnly` 改名 `showOutputBox(autoHide)`。③ 该最终回复事件由裸 `listen` 改为 `getCurrentWindow().listen`（**定向**事件纪律，与 MOUTH_EVENT 同）。`npx tsc --noEmit` exit 0；`pet 0.9.52→0.9.53` 已重打包（`build-plugins.ps1 -Plugin pet` exit 0，BOM 校验前 3 字节 `EF BB BF` 恰一份）。**临时诊断打点**：`pet-chat-reply` 处理器里留了 `log_frontend` 的 `[pet] output reply len=… visible=… chatOpen=… auto=…`（**临时，查清后删**）。**待真机验收**：主动反应 / 抓屏 / 语音输入三类「桌宠发起」的轮次——输出框在轮次**开始**即弹出（显示「思考中」）、短回复盒子只占一行不上下留白、长回复贴顶从第一行开始、能滚到最后一行且末尾无空白、滚动条为全局细条（滑块实心 / 轨道不描边）；用户自己开着输入对话时不被抢走输入框。**如实记**：现有实现里输出框**本来就贴顶**（没有 flex 垂直居中），实际可疑声明是「固定 `height:75px` 未按内容收缩」；抓屏路径依赖「图片输入 / vision」开启，关着时**不会**发送、自然也不弹框（`sendScreenToAI` 只落一条 warn） |

### §待办 2 补 可恢复结论索引：压缩丢结论 ⇒ 模型回到初始问题重答（2026-10-14，本轮；已实现，待真机验收）

- **要解决什么（用户报障）**：`compact_history` 的 `Drop` / `Force` 档把历史中段整条 `drain` 掉；被丢段里的「用户一路想要什么 / 试过哪条路为什么失败 / 结论落在哪个文件」随之消失，模型只剩「开头那条最早的提问 + 尾部」⇒ 把**最早那个诉求**当当前任务重做（原话：「只要压缩上下文直接回到了上一次人格修改」）。
- **存储设计（格式 / 落点 / 按 scope 分家 / 为什么不在 `chat.db`）**：
  - 落点 = agent 自己的根下 `<exe 根>\temp\conclusions\<scope>.jsonl`（`conclusions::default_dir`，与 `temp\review-last` 同一套 `log_dir().parent()` 推导；**未新增宿主桥方法**，见 §禁用 62）；**global / pet 各一份文件**（`MemoryScope::as_str`）。**零新增依赖**（只用 `serde_json`，无 embedding 库）。
  - 格式 = **JSONL 追加式**（一行一条，append-only；写入按「状态 + 正文」去重）。
  - 字段：`ts`(epoch 秒) / `scope` / `turn` / `conclusion`(一句话) / `keywords`(精确标识符) / `refs`(`{kind:file|url|query|commit|command, value}`) / `status`(verified | failed | rejected) / `reason`(失败原因) / `session_id`。落盘前正文 / 原因 / 关键词 / 指针**一律过 `mask_secrets()`**（复用既有唯一实现）。
  - **为什么不在 `chat.db`（⚠️ 如实订正）**：`chat.db` 只存前端展示的 user / assistant **纯文本**，**不含 `tool_use` / `tool_result`** ⇒ 那里**根本没有可检索的工具结论**；**压缩丢的那段在库里本就找不到**。故索引必须另落一处（agent 根下），**不要**去库里捞。
- **写入时机（只两处；是否零新增调用）**：
  - ① **压缩边界（Drop / Force；Elide 只写指针）**：`restore_after_compaction`（`main.rs`）一次做完「摘要 + 索引写入 + 检索重注入」。结论来源**优先**用压缩摘要那**一次**模型调用产出的结构化段（`SUMMARY_PROMPT` 末尾追加 `===LUNAC-CONCLUSIONS-BEGIN/END===` 包裹的 JSON 数组，`parse_model_block` 切出并剥离）⇒ **零新增调用**；拿不到就**确定性抽取**（`extract_from_dropped`：用户诉求 / `is_error` 的失败尝试 / 写类工具文件指针）。被 **elide** 的可重现结果也写「查过哪里」的指针条目（`extract_from_elided`，不调模型）。
  - ② **里程碑（轮末）**：`run_query` 返回后 `flush_turn_conclusions` 从最近窗口**确定性**抽结论落盘（**不调模型**）—— 在结论可能被压缩丢掉之前先保住。选它是**里程碑**（一轮诉求完成），**不是**「每次工具调用 / 每轮」（那会频繁变动前缀、打爆 prompt cache，用户裁决红线）。
  - **零新增调用证据**：单测 `structured_conclusions_reuse_the_single_summary_call`（同一份 stub 响应既产出钉回历史的摘要、又产出进索引的结论；全程只一次请求）。
- **检索 + 重排（是否含向量通道 / 为什么；查询构造；打分公式与参数）**：
  - ⚠️ **未实现向量通道，原因：无可用 embedding 源**（如实标注，**不假装做了**）：已核实 `core-agent/Cargo.toml` 无嵌入 / 向量依赖；全仓无 embeddings 端点调用；供应商端点是 chat/completions 形态。⇒ **只做字面 / 标识符通道 = BM25 + 标识符重合率**。若日后接入 embedding 源，在此补「向量通道 → RRF 融合」即可，其余不动。
  - 查询串 = **最近的用户诉求（尾部最多 2 条）+ 任务快照（最近 `TodoWrite` 清单）+ 被丢弃 / 被瘦身区间的标识符与原文片段**（`build_conclusion_query`）。
  - 打分（Generative Agents 形式，各分量归一到 [0,1]）：`score = recency × importance × relevance` —— `recency = 0.99^(age_hours)`（`RECENCY_DECAY_PER_HOUR`；**用乘积而非加权和** ⇒ relevance=0 直接归零被滤掉，避免每次压缩灌无关结论）；`importance` = 启发式（**如实降级**：本仓没有给条目打 1–10 分的模型调用）：基 0.4 / `failed` +0.3 / 有指针 +0.2 / 有关键词 +0.1，上限 1.0；`relevance = 0.6 × BM25_norm + 0.4 × 标识符重合率`（BM25 `k1=1.2, b=0.75`）。注入闸门 `score ≥ MIN_RETRIEVAL_SCORE(0.05)`。
- **注入（位置 / 上限 / 失败标注 / 前缀不变量）**：
  - 位置 = **紧随压缩摘要之后**（无摘要则紧随 head 之后）的一条**独立 user 文本消息**（可变层，避开 lost-in-the-middle）；**绝不进 system / 固定前缀**（§禁用 45）。
  - 上限 = 条数 `CONCLUSIONS_MAX_ITEMS=5`、总字符 `CONCLUSIONS_INJECT_CHARS=1200`（**与 `MAX_CONTEXT` / 预算无关** —— 用户裁决**不加预算**，依据：Chroma《Context Rot》18 模型 194,480 次调用「输入越长性能越不稳」）。
  - `failed` 条目**显式标注**「曾尝试并失败，勿重复」（`render_line`）。
  - **前缀不变量证据**：单测 `compaction_and_injection_leave_the_fixed_prefix_untouched` —— `system` 提示词逐字节不变 + `history[0]`（固定上下文块）逐字节不变 + 注入块是可变层的独立 user 消息（0 号位不动）。
- **`status` 语义（三态怎么判 / 失败为何不删）**：`verified` = 成立（用户诉求 / 成功写类工具 / elided 指针）；`failed` = 模型判定的失败尝试或 `is_error` 的工具结果，**必带 `reason`**；`rejected` = 被后续证据推翻的结论。**失败不删**（Manus「Keep the Wrong Stuff In」：删掉失败会移除模型纠错所需的证据）—— 一律保留并标注状态。
- **日志**：写入 / 检索 / 注入各一条（条数 / 命中 / 字符数），**不记结论正文 / 关键词 / 指针**（可能含敏感信息）。
- **新增单测（9 条）**：`conclusions.rs`：`extract_captures_conclusion_failure_and_file_pointer` / `append_is_scoped_deduped_and_reloadable` / `retrieve_hits_by_identifier_and_marks_failure_as_dont_repeat` / `parse_model_block_splits_and_strips` / `secrets_are_masked_before_persisting` / `identifiers_keep_real_ids_and_drop_plain_words`；`main.rs`：`compaction_writes_index_and_reinjects_conclusions` / `compaction_and_injection_leave_the_fixed_prefix_untouched` / `structured_conclusions_reuse_the_single_summary_call`（并把原 `nothing_is_summarised_when_nothing_was_dropped` 改写成 `restore_does_nothing_when_nothing_was_dropped_or_elided`）。`core-agent` 侧 `cargo test` = **209 passed**（基线 200，净 +9）。
- **真机验收口径**：① 起 dev（`npm run tauri:dev`）跑一段「有诉求 + 有失败命令 + 有文件改动」的长会话，逼到 `Drop` / `Force` 压缩（也可 `/compact`）⇒ `<exe 根>\temp\conclusions\global.jsonl` 出现条目，**失败那条 `status=failed` 且带 `reason`**、指针字段正确；② 压缩后继续问「任务关键词」⇒ agent 日志见 `结论索引检索 … 命中 N 条` 与 `结论索引注入 …`，且**模型没有回到最初的诉求重做**；③ agent 日志「请求前缀」行的 `system=` / `tools=` 哈希在压缩前后**逐字节相同**，注入只体现在 history 段（可变层）。
- **涉及文件**：`core-agent/src/conclusions.rs`（新增）、`core-agent/src/main.rs`（`SUMMARY_PROMPT` 追加段 + `summarize_dropped` 返回结构化段 + `restore_after_compaction` / `build_conclusion_query` / `flush_turn_conclusions` / `CompactOutcome.elided_msgs` + 5 处调用点）、`docs/todo.md`（本条）。**未改**宿主 `app/src-tauri/**`、`chat.db`、`MAX_CONTEXT` / 预算常量、TTS / 桌宠 / `mask_secrets` / 模型目录 deny 闸。

### §待办 2 补 二 语义分层压缩（S0「压缩语义化」）（2026-10-14，本轮；已实现，待真机验收）

- **要解决什么**：现有压缩是**体积驱动**的（`ELIDE_RATIO=0.85` / `DROP_RATIO=0.95` / `COMPACT_KEEP_TAIL=8`），只看「多大」、不看「是什么」⇒ 把「用户一路的诉求 / 已试过并失败的尝试」和「一段读文件的原始输出」同等对待。本轮升级为**语义分层**（口径见 `archive/agent-feature-backlog.md:1278`「工作记忆保细节 / 情景记忆压成结论 / 语义记忆入长期」）。**两步都做**，**不新增触发时机**（四类触发点一行未改，只改触发点内部「丢谁」的决策）。
- **分层表（判层 = 确定性纯函数 `classify_message_layer(msg, tool_name)`，`core-agent/src/main.rs`）**：

  | 层 | 含义 | 判据 | 压缩处置 |
  |---|---|---|---|
  | L1 | 用户诉求 / 任务目标 | `role=user` 的非合成文本（不以 `[` / `#` 开头） | 保原文（head 永不丢；中段用户文本进摘要 / 索引） |
  | L2 | 已达成的结论与决策 | 含 `tool_use` 的 assistant | 结构化摘要 + 入结论索引 |
  | L3 | 未决问题 / 待办 | 含 `TodoWrite` 的 assistant | 摘要 + 任务快照钉回 |
  | L4 | 失败证据 | `tool_result.is_error` | **永不因体积丢弃**（保留 or 进摘要 / 索引） |
  | L5 | 可重取的原始材料 | `elidable_tool` 命中的 `tool_result` | 只留指针（可 elide；重跑即得） |
  | L6 | 纯过程性噪声 | 其余（assistant 纯文本 / 不可重取批量结果 / 合成消息 / 空白） | 优先丢弃 |

  - **判层零新增模型调用**：纯函数、不联网、不动固定前缀（**禁止让模型判层**，见 `forbidden.md` §禁用 45）。
- **第 1 步（Elide 候选限定到 L5）**：`compact_history` 的瘦身候选取「`classify_message_layer(..) == L5`」而非旧的 `elidable_tool(..)`。**多一条硬规矩**：`is_error` 的工具结果即使工具本身可重取（如 `Cmd` 报错）也判 L4 ⇒ **失败证据永不瘦身**。既有 `worthwhile_elisions` 收益 / 代价模型**原样保留**（防「省小废大」每轮改前缀）。
- **第 2 步（`Drop` 改按层丢）**：`layer_aware_drop_cut(history, tool_names, head, natural_cut)` —— 在 `head..cut` 内找**最靠前的保留层**（不在 `LAYER_DROP_ORDER=[L6,L5,L3,L2]` 里 = L1 / L4），命中就把 `cut` 收缩到它之前 ⇒ L1 / L4 及其后内容全部保留。三条硬约束：
  - ① **保配对**：保留侧不能以 `tool_result` 开头，若 `cut` 落在它上就回退到其配对的 `tool_use` 一起保留（端点硬校验 `tool_use` ↔ `tool_result`）。
  - ② **保连续**：只收缩右边界，被丢的仍是**连续一段** `head..cut`，**不把 drain 打散**成不连续区间。
  - ③ **保 L1 / L4**：`LAYER_DROP_ORDER` **只是「丢层优先序」**；L1 / L4 **不在表里 ⇒ 不可丢**。
  - **`Force`（400 兜底）不走层感知**：它是「不压就撞上限」的安全刚需，必须能压下去；其被丢的 L4 仍进摘要 / 索引（`restore_after_compaction`）⇒ 证据不消失（这正是「L4 不可被丢 = 保留 or 索引」的兜底）。**未改** `ELIDE_RATIO` / `DROP_RATIO` / `COMPACT_KEEP_TAIL`。
- **摘要 prompt**：`SUMMARY_PROMPT` 四段（`Historical Task Snapshot` / `What Was Done` / `Decisions And Constraints` / `Pending`）**已天然对应 L1–L3** ⇒ **未改内容、未改调用次数**（只在 Rust 注释里显式标注映射）。
- **新增单测（5 条，`core-agent/src/main.rs`）**：`layer_classification_covers_every_layer`（六层各一例）、`drop_prefers_noise_over_failed_evidence`（先丢 L6 噪声、保 L4）、`failed_evidence_survives_compaction`（L4 不因体积消失）、`pinned_user_request_survives`（L1 不被丢）、`drop_does_not_orphan_tool_results`（配对完整）；**扩展** `compaction_and_injection_leave_the_fixed_prefix_untouched`（加 `Drop` 档一遍：前缀不变量 + L4 存活）。`core-agent cargo test` = **216 passed**（基线 211，净 +5）；**未改动任何既有断言**（既有 `Drop` 档用例的丢弃区间里没有 L1 / L4，`Force` 档行为未变）。
- **零新增调用证据**：压缩路径仍只发**那一次**摘要请求 —— 单测 `structured_conclusions_reuse_the_single_summary_call`（stub 只服务一次即全部模型调用）；本轮**未新增任何 `cfg.client.post` 调用点**（只加纯函数判层）。
- **真机验收口径**：① 起 dev（`cd app ; npm run tauri:dev`）跑一段「有诉求 + 有失败命令 + 有文件读写 + `TodoWrite`」的长会话，逼到 `Drop` 压缩（或 `/compact`）⇒ agent 日志「上下文压缩：瘦身 … 丢弃 …」；被丢段**不含** `is_error` 的 `tool_result`、`history[0]` 那条用户诉求不变；② 压缩后继续问任务关键词 ⇒ 索引里失败条目 `status=failed` 且带 `reason`、模型没回到最初诉求重做；③ agent 日志「请求前缀」的 `system=` / `tools=` 哈希压缩前后**逐字节相同**（分层只动 history 可变层）。
- **涉及文件**：`core-agent/src/main.rs`（`Layer` / `classify_message_layer` / `message_layer` / `layer_aware_drop_cut` / `LAYER_DROP_ORDER` + 瘦身候选取 L5 + `Drop` 边界层感知 + 5 条单测）、`docs/todo.md`（本条）。**未改**：system / 固定前缀构造、`MAX_CONTEXT` / 预算常量、`conclusions.rs` 对外接口（仅调用）、TTS / 桌宠 / `mask_secrets` / 模型目录 deny 闸。

## §待办 3 按需重估（不做只因优先级，不因没价值）

### §待办 3-1 A13 Agent 能力缺口剩余项
- **已查清**：MCP 的 Streamable HTTP / OAuth 2.1 / prompts / roots / elicitation / `.mcp.json`（项目级）+ 信任门 + 配置改动检测自动重启**均已落地**；内置技能 / 工具模板、`AGENTS.md` 项目记忆**多层 / 向上递归已落地（2026-10-11，见下 3-1a）**。
- **待办点**（按需）：`sampling`（服务端反向调用模型）；**记忆目录的「子目录级」按需加载**（**已实现（2026-10-12）**，见下 3-1a）；斜杠命令（只挑与能力相关子集：`/compact`、`/rewind`、memory 类）—— **`/rewind` + memory 类 + `/compact` 均已实现，见下 3-1b**；**`NotebookEdit` 已实现，见下 3-1c**；**`EnterWorktree` / `ExitWorktree` 已实现，见下 3-1e**；**`CronCreate/Delete/List` 本轮未做（代价 / 可测性已评估，见下 3-1d）**；`WebBrowser` / `LSP`；**`SendUserFile` / `PushNotification` 均已实现（agent → 用户呈递：文件卡 / 桌面通知，见下 3-1g）**；**已废弃的 HTTP+SSE 双端点传输不做**。
- **涉及文件**：`core-agent/src/tools.rs`（`NotebookEdit`、`EnterWorktree` / `ExitWorktree`）、`core-agent/src/main.rs`、`app/src/main.ts`、`core-agent/src/mcp.rs`、`skills.rs`。

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

#### §待办 3-1d `CronCreate/Delete/List`（定时触发，**本轮未做**——只报告方案与代价）

- **结论**：**本轮不做**（不是没价值，而是代价 / 可测性与既有设计有冲突，见下）。**用户已裁定（2026-10-11）：接受本轮不做**（下述 5 条理由与「若日后要做的最小形态」保留）。已达成的共识：**若日后要做，最小形态 = 会话内 + 进程内定时器**，与「跨进程落盘」分开。
- **为什么不做（逐条，可复核）**：
  - **① 触发要有「活着的常驻会话 + 时间推进」，现有 E2E 驱动不了**：`--print` 单发跑完**恰好一轮**即退出；`scripts\agent-e2e.ps1` 的常驻 stdio 路径把 stdin 写成**一个文件**，agent 读完即 EOF ⇒ 主循环 `Disconnected` ⇒ 进程退出（`core-agent/src/main.rs` 主循环）。**脚本没有「stdin 保持打开 N 秒」的模式** ⇒ 无法驱动一次定时触发；要端到端验证就得先给脚本新增 keep-alive 模式。
  - **② 「触发时怎么跑」的语义未裁定**：定时触发 = 注入一轮 query 吗？那会进 `history` / 前端对话流（**用户没打过的字**），需要一个「来源标记」（= 新协议字段，触「不新造字段」约束）；且这轮里的写类工具仍要走审批（无人在场时无法完成）。
  - **③ 成本红线**：每次触发 = 一次模型调用，按项目纪律必须**写死频次上限**（最小间隔 / 每会话次数 / job 数上限）；而把最小间隔定得「能让测试跑通」就等于把 E2E 拖成长时间等待（与本任务「别把测试拖长」的要求冲突）。
  - **④ 落点与既有设计的重叠**：会话内定时器最省的写法是复用主循环的空闲 tick（`MCP_WATCH_TICK = 1s`，不新增线程）；但**跨进程持久化**就与 §待办 4 S2「检查点落盘 / 跨回合续跑」重叠 ⇒ 应**合并设计**，不为 Cron 单独造存储子系统。
  - **⑤ 无既有描述可依**：**§待办 4 里并没有对 Cron 的既有描述** —— A13 只在 §待办 3-1 的待办清单里列了名字。故「触发时怎么跑」没有现成裁定。
- **用户口径补记（2026-10-11）**：**e2e / 测试产生的用户消息直接进历史记录是可接受的**（用户是单人开发，自己删即可）—— 这条解除了上条 ②「触发的 query 会以用户消息进 history/UI 需**来源标记**」顾虑的一半（history/UI 侧不再需要来源标记；②中「无人在场时写类工具仍要走审批」那半与其余理由不变）。**但 Cron 本轮仍不做**，别因此改成「已实现」。
- **若日后要做的最小形态（留作子项）**：进程内 `static Mutex<Vec<CronJob>>` + `CronCreate` / `CronDelete` / `CronList` 三件（无条件注册 ⇒ 工具计数 21 → 24；原记「19 → 22」已随 `EnterWorktree` / `ExitWorktree` 落地作废）；主循环空闲 tick 到点把 prompt 以**带来源标记的 user 消息**注入队列；**硬边界** = 仅会话内、**进程重启即失效**；**硬上限** = 最小间隔 ≥ N 秒、每 job 最多 K 次、全局 job 数上限；端到端验证需先给 `scripts\agent-e2e.ps1` 加 keep-alive 模式。
- **涉及文件（若做）**：`core-agent/src/tools.rs`、`core-agent/src/main.rs`、`scripts/agent-e2e.ps1`。

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
  - **本轮进展（2026-10-15，S1 首个切片已接线）**：`decide.rs` 的语义由**输入门**整体换成 agent 侧的**只读预筛**（`RUN`/`SKIP`：这一轮值不值得派一次后台复盘 fork），模块级 `#![allow(dead_code)]` 已移除，头注释里的「调用点 = L10 桌宠常驻输入」已删（用户已裁定落 core-agent 侧）。**消费点只接一处** = `main.rs` 主循环「是否派后台复盘 fork」：`SKIP` ⇒ 该轮不派（省一次云调用），但仍**吃掉本 scope 会话配额**（幂等，跳过不会变成「下次还能再派」）；`RUN` / 任何故障 ⇒ 照旧派。**摊销口径** = **每 K 轮**（`LUNAC_LOCAL_DECIDE_K`，默认 5）+ **压缩事件**（四个触发点各只读判定并记账，**首切片只记账、绝不改压缩行为**）。**开关** `LUNAC_LOCAL_DECIDE` **默认关**；未配 `LUNAC_LOCAL_MODEL_URL` ⇒ 整层透明关闭。**决策账本**独立落盘 `<exe 根>\temp\decisions\<scope>.jsonl`（不进 `chat.db`，见 forbidden §禁用 61），落盘前过 `mask_secrets`、只存 `log::hash64` 指纹、写失败只 warn。**fail-open**：超时 / 端点不可用 / 模型没起 / 解析失败 ⇒ 退化为「不决策」= 走现状路径 ⇒ 默认关时与不接线**逐字节一致**。仍未做：五类决策的其余四类（能力路由 / 安全预筛 / 记忆候选抽取 / 检索查询改写）。
  - **本轮进展（2026-10-15，S1 冷启动预热已接线）**：真机暴露 —— `llama-server` 冷启后**第一次**判定耗时 **>1500ms**（撞 `LOCAL_DECIDE_TIMEOUT`）⇒ 每次 agent 重启后首次预筛静默 fail-open（行为安全，但等于没生效）；热机后 ~394ms 正常。**加预热**：`decide.rs` 新增 `warmup_with` / `warmup_config`，`main.rs` 在进主循环前 `thread::spawn`（`spawn_local_decide_warmup`）对**本地端点**发**一次**极短请求（同一 `LocalHttp` 通道、同样带 GBNF、`max_tokens` 极小）**丢弃结果** —— 把模型加载 / 首次 KV / 连接吃进缓存。硬约束：**绝不阻塞启动 / 主循环**（后台线程）、**只在开关开 + 端点已配**时预热（默认关 ⇒ 零预热零请求）、**只打本地端点**、**不进账本**、**不动固定前缀**、失败静默（只 `info`）。⚠️ 预热用**专用更宽上限** `WARMUP_TIMEOUT=30s`（判定的 1500ms **一个字不改**）：实测冷启第一次推理本就更慢，沿用 1500ms 会把预热自身提前掐掉、留下未热身的服务端。
- **本轮进展（2026-10-15）：修复「安装版本地模型从未真正可用」（方案 B）**。**根因**：`local-model` 的两个依赖（`bin/llama-server.exe` + `models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf`，实测 **2.33 GiB**）**唯一来源 = 运行期从插件市场安装时按 `dependencies[]` 下载**（`plugin_market.rs:887` → `install_dependencies_with_progress` → `install_archive_dependency`(bin) / `install_file_dependency`(GGUF)）。**安装器不内嵌**（NSIS 无该段，见 `forbidden.md` §禁用 5 第 56 条），设置里的开关也**不下载依赖** —— 安装机上只铺了「不含依赖的 1.4KB 插件包」，依赖下载从未触发；而「已装可用」判据只查「清单可解析 + 入口存在」，于是那句「未安装…请到插件下载」永远不显示，用户一开就撞 `sidecar 可执行文件不存在：…\bin\llama-server.exe`。
- **方案 B 三件事（已实现）**：① **开启前先补依赖** —— 开启开关时若依赖未就位，**先**调**既有** `install_plugin_dependencies`（与 `ocr-engine.ts` 同一条路，**未造第二套下载器**）按清单补齐，带 `plugin-install-progress` 进度 / 可取消（宿主 `cancel_plugin_dependency_install` 置中止标志）/ 失败可重试；② **判据扩展到「依赖就位」** —— `plugin_market::list_installed` 新增 `ready`（`sidecar_command_ready`：声明了 `sidecar` 的插件要求其 `sidecar.command` 文件真的存在；**无 sidecar 的插件恒就绪，不误伤**），前端据此显示「未就绪 / 待下载」而不是放行后报底层错误；③ **失败文案可操作**（人话 + 「重试」入口）。
- **仍**未内嵌依赖、也**不做**开机自动下载（2.33 GiB 体积太大，违背既有口径）⇒ **首次开启需下 ~2.33 GiB**（视网速数分钟到数十分钟，界面已给时间预期提示）。**未动** `build-plugins.ps1`（local-model 的 468MB/0.5B 旧注释口径订正不在此范围）。
- **涉及文件**：`core-agent/src/decide.rs`、`app/src/plugins/builtin/local-model.ts`、`app/src-tauri/src/plugin_sidecar.rs`（本轮**另加**：`app/src-tauri/src/plugin_market.rs`、`app/src-tauri/src/commands.rs`、`app/src-tauri/src/main.rs`、`app/src/plugins/market.ts`、`app/src/plugins/builtin/settings.ts`、`app/src/i18n.ts`）。

#### §待办 3-6 补 本地判定 / 抽取「撞车」修法 + 记忆读写路径查证（2026-10-16，本轮）

- **修 1（本地模型通道进程级非阻塞互斥）**。**根因（已查实）**：`llama-server` 以 `-np 1`（**单并发槽**）运行；开 `LUNAC_LOCAL_DECIDE=1` 时**同一个回合**里「预筛（`LOCAL_DECIDE_TIMEOUT=1500ms`）」与「记忆候选抽取（`LOCAL_EXTRACT_TIMEOUT=5000ms`）」会**几乎同时**发往同一本地端点 ⇒ **互相排队** ⇒ 两条都等到各自超时 ⇒ 双双 fail-open（真机 `app/src-tauri/target/debug/temp/logs/agent-2026-10-16.log`：抽取 `latency 5009ms`、判定 `1515ms` 都在上限处被掐断，而**单独**那次预筛仅 `1133ms`）。**修法（最小改动，首选方案）** = 判定 / 抽取 / **预热** 三者**共用一把进程级非阻塞锁**（`core-agent/src/decide.rs` 的 `try_acquire_local_channel` + `ChannelGated`，`memory_candidates.rs` 的 `ChannelGated`）—— **拿不到锁就立即跳过本次**（判定 / 抽取折成 `fallback=true` + `reason="busy-skip"`、记一条账本行；预热只记一行 info），**绝不排队等待**（这些是「摊销的尽力而为」调用，跳过无损，排队只会把两个请求都拖死）。**未动**：超时值、`-np`、「每轮多请求」；**默认关 ⇒ 逐字节不变**（开关关 / 未配端点时根本不进判断层，零请求零锁）。**单测**：`decide::tests::gated_request_busy_skips_immediately_without_waiting`（持锁时第二个「本地请求」**立即** busy-skip、`< 1s` 不等待）、`channel_lock_is_exclusive_and_non_blocking`、`memory_candidates::tests::gated_extract_busy_skips_without_waiting`。
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
- **`D:\Lunac\skills\live2d-split`（误放进 skills 的 PS CEP 面板）**：按四分法应挪去工作目录 `transStorage`；**用户 2026-10-07 决定暂不搬，等插件实机验证完再搬**。搬时一并处理 `config.json` 的 engine 绝对路径与 `%APPDATA%\Adobe\CEP\extensions\` 那份副本。
- **`chat.db` 历史遗留大库**：那份 298 MB 的库已于 2026-10-02 去重重写（→ 0.81 MB），备份留在 `ModuleData\history\chat.db.bak-2026-10-02`（确认无误后可删）。

### §待办 3-9 记忆读侧三件套：快照兜底 + 只读检索工具 + 压缩边界刷新（2026-10-15 登记）

- **现状定性（一句话）**：**读 = prompt**（`ModuleData\memory\MEMORY.md` / 桌宠私有一份，一行一条 bullet；**启动读一次的冻结快照**注在 `history` 开头，**无读工具**、**整段注入、零检索召回**）／**写 = loop**（`Remember` 工具 + 每会话每 scope 至多一次的后台复盘 fork + 人工 `/memory clear`），合起来是**混合**形态。⚠️ **`docs/` 与 archive 从未使用「prompt 架构 / loop 架构」这两个词** —— 这是我方**按定义**给的分类（读侧靠 prompt 注入 = prompt 架构；写侧靠 agent 循环里调工具 = loop 架构），**不是文档既有术语**，采信前先认这个定义。
- **待解决的墙**：`MEMORY.md` **整文件 6000 字符封顶**（`mcp_server.rs` 硬闸）⇒ 写满后**唯一出口是 `replace:true` 整体重写**（`docs/archive/ai-spec.md` §11 规则 56）⇒ **被迫丢信息**（想加新条就得先删旧条）；且**会话中新写的记忆要重启 agent 才可见**（冻结快照，规则 56 明写「会话中新写的记忆只落盘」）。
- **三件套（逐项：做什么 / 代价 / 为什么这样切）**：
  - **A 保留快照兜底**（不动）：冻结快照**每轮一定在场**（注在 `history` 开头，规则 56）。**做什么**：什么都不做，仅作为 B / C 的兜底。**代价**：零（现状即它）。**为什么这样切**：稳定、每轮必达、不依赖模型「想起去查」；B 再强也不能假设模型一定会调工具。
  - **B 加 `MemorySearch`（只读工具）**：**做什么**：新增一个**只读、免审批、可被 `--disallowedTools` 裁**的检索工具（按查询召回记忆条目），**只能模型主动调、绝不自动注入**。**代价**：多一个工具进工具表（固定前缀变一次，需重启 agent 才生效）；每次调用多一次往返；检索质量是**新增问题**。**为什么这样切**：解决**扩展性**（6000 字符封顶的根因）；**范式先例** = `SessionSearch` 就是「只能模型主动调、不得自动注入」（`docs/forbidden.md` §禁用 4 第 46 条 / 规则 53）—— B 完全照它走，**不新造范式**。
  - **C 压缩边界刷新快照**：**做什么**：在**压缩事件后**复用**已有**的「压缩后重注入」通道，把记忆快照**重新注入**一次。**代价**：要动注入路径（须仍满足固定前缀不变量）；会**消费**一次注入点。**为什么这样切**：**修掉「重启才可见」**（规则 56 的副作用），且**复用既有机制**（`main.rs` 那套「压缩后重注入结论索引」）、**不新开通道**。
- **代价与冲突（必须写清）**：
  - ⚠️ **要修订 `docs/archive/ai-spec.md` §11 规则 56**：它**明写**「注入必须是冻结快照……**禁止**『写完记忆就刷新提示词』『每轮重读一次记忆文件』」，并解释「会话中新写的记忆只落盘，下次重启才可见」。C 直接触碰这条 ⇒ **改的是规范不是代码**，**需先拍板**；B 不触碰它（只读工具不改注入）。
  - **延迟**：读侧一旦从「整段注入」变成「模型按需调工具」，**每次读到记忆都至少多一次往返**（本地检索是本地调用，但若走云则多一次云往返）。
  - **不可复现 / 归因变难**：模型「这一轮到底查没查记忆、查到了什么」不可预知 ⇒ 同一问题两次可能一次答对一次答错（取决于模型是否调用工具）。
  - **失败模式**：「**该用记忆时模型忘了查**」—— 这是 loop 读的**固有失败模式**；A（快照兜底）在场时危害小，**若为省 token 去掉 A 则显著放大**。
  - **检索质量是新增问题**：现在**完全没有记忆召回**（零检索）⇒ 一旦加 B，「召回得准不准」成为**新的一组待调问题**。⚠️ `conclusions::retrieve` 是**压缩恢复**（把被压掉的结论找回上下文），**不是**记忆召回，**别混**。
- **验收口径（可证伪）**：① **「记忆超过 6000 字符后仍能取回旧条」** —— 造 >6000 字符的记忆（或 `replace` 丢过信息后），用 B 检索应取回**快照里已不在**的旧条；② **「同一问题在快照缺失时靠检索仍答对」** —— 造一个只存在于检索条目、不在快照里的事实，模型应能靠 `MemorySearch` 答对；③ **「压缩后写的新记忆无需重启即可见」** —— 会话中途写一条新记忆 + 触发一次压缩 ⇒ 该条**无需重启 agent** 即应能被模型看到（C）。
- **分期建议**：**先做 B**（收益最大、不碰注入不变量、有 `SessionSearch` 先例），**再评估 C**（要动注入路径 + 改规则 56，代价最大、需拍板），**A 不动**（兜底，删了会放大「忘了查」）。
- **依据**：`docs/archive/agent-feature-backlog.md` §4.1.1 / §4.1.7、`docs/archive/ai-spec.md` §11 规则 56 与 §3.5、`docs/forbidden.md` §禁用 45 / §禁用 61、现状代码 `app/src-tauri/src/mcp_server.rs`（6000 上限）/ `core-agent/src/main.rs`（冻结快照注入）。

## §待办 4 长期方向（方向性、不排期；进入实施前先提升为 §待办 1–3 正文）

> 来源：用户 2026-10-09 立项的**长期 AI 侧规划**（分阶段能力成熟度路线图 S0–S4）。主题 = 本地模型只参与决策 / agent 整体框架图 / skills + tools 完善与长任务托管 / 到达 AGI 的要求。**本节不构成待办**，只登记方向与已对齐的调研结论。

- **S0 单模型增强**：搜索溯源（每结论挂 URL）· CLI 无头面（`--print` / `--resume` / `--output-format stream-json` / `agent mcp serve`）· **压缩语义化（已实现，2026-10-14，见 §待办 2 补 二）** · 记忆自动抽取（云侧先行）**→ 已提升为 §待办 1-6（2026-10-11）**。
  - **无头面验收口径 = 瞄准 release（2026-10-11）**：`scripts\agent-e2e.ps1` 已改为**默认拉 release 产物**（`core-agent\target\release\agent.exe`），dev 需显式加 `-Dev`；release 缺失即 **exit 2** 并提示「先 `cargo build --release`（cwd=`core-agent`）」，**不静默回落**。理由 = 交付目标即用户实际安装 / 运行的版本。见 `docs/testing.md` §测试 10。
  - **release 二进制已重建（2026-10-11，按「以 release 为目标」这条口径补做）**：`agent.exe`（`cargo build --release`，cwd=`core-agent`，13:44）与 **宿主 `lunac.exe`**（`cargo build --release`，cwd=`app\src-tauri`，13:50，Finished in 2m52s）。⚠️ **宿主 release 不会随 core-agent 的 release 构建一起更新** —— 必须单独在 `app\src-tauri` 编一次；本次借此把本会话对 `src-tauri` 的三处改动**带进了 release 宿主**：`mcp_server.rs`（记忆落盘前过 `mask_secrets`）、`plugin_window.rs`（`open_plugin_ids` 当前打开的插件）、`main.rs`（该命令注册）。已用 `scripts\agent-e2e.ps1 -McpExe app\src-tauri\target\release\lunac.exe` 实跑 **exit 0 / PASS**（工具池 18 ⇒ MCP 桥在 release 上连通）。
  - **CLI 无头面现状（2026-10-11 摸清，逐条可复核）**：
    - **已有什么**：`agent.exe` 本就是**常驻 stdio 循环** —— `stdin` 每行一条 `{"type":"user",...}`、`stdout` 每行一条 JSON 事件（`system` / `stream_event` / `assistant`（含 `tool_use`）/ `user`（含 `tool_result`）/ `usage_delta` / `result`；契约见 `core-agent/src/main.rs:8-34`）。**已实装**：`--add-dir <dir>`（可重复）、`--permission-mode plan|acceptEdits`、`--dangerously-skip-permissions`、`--permission-prompt-tool stdio`（写类工具发 `can_use_tool` 审批）、`--disallowedTools <name…>`、`--mcp-server stdio:<exe>`（agent 拉起该 exe 的 MCP server —— 即 `agent mcp serve` 的等价物）。`--verbose` 仍**接受并忽略**；`--include-partial-messages` **已实装（2026-10-14）**（见下条「单发模式」）。
    - **已实现（2026-10-11）：`--print` / `-p` 单发模式**。一次性跑**恰好一轮**提问后退出；**触发判据 = 带 `--print` 且未声明 `--input-format stream-json`**（后者是上游常驻 stdio 流式输入模式，宿主 `start_cli_process` 一直这么传 ⇒ **常驻模式不受影响、行为逐字节不变**）。query 来源 = `--print` 紧跟的那个非 flag 值，没给则从 stdin 读**一次**（整段一条）。`--output-format`（默认 `text`）：`text` 只打最终助手文本、`json` 打**一个** `result` JSON（含 usage / is_error）、`stream-json` 沿用事件流并在 `result` 后退出。**审批策略（关键）**：单发没有 `control_response` 回包通道 ⇒ 需审批的工具默认**直接拒绝**（绝不静默挂起，不会干等 300s 超时），除非显式 `--dangerously-skip-permissions`；项目 `.mcp.json` 信任门跳过、elicitation 直接 decline。退出码：成功 0，模型 / 网络失败或审批被拒 ⇒ 非 0。`--include-partial-messages`（2026-10-14 实装）：单发 `stream-json` 下**额外**逐条输出 `stream_event` 增量行（每行一条 JSON、当场 flush），默认**关**；常驻模式不受影响（`stream_event` 恒发）。实现落点：`core-agent/src/main.rs` 的 `CliArgs::parse` / `CliArgs::single_shot` / `OutputMode` / `ApprovalPolicy` / `emit` / `unattended_deny_reason`；用法与实测见 `docs/testing.md` §测试 10。
    - **`--resume`（跨进程会话续跑）：评估结论 = 暂不做、不为此单独造会话存储**。已查清：**agent 侧没有会话持久化** —— history 是**进程内** `Vec`（`main.rs` 的 `history`），会话 id 由进程内序号生成（`session_id()`，`sess_<pid>_<ts>` 形），**都不落盘**；宿主的历史在**前端**（`chat.db`，宿主持有），agent 侧拿不到。要做 `--resume` 至少需要：① 一份会话落盘（history + 会话 id + 工具/审批上下文，含预算与压缩状态）；② 一条「宿主 → agent」的恢复通道（或 agent 自己按 id 读盘）；③ 与压缩 / 上下文块幂等注入 / 记忆快照对齐。代价 = 一套新的会话存储子系统（大工程）。**建议**：与 §待办 4 S2「长任务托管 / 检查点落盘」**合并设计**（两者都要「回合状态落盘 + 跨进程续跑」，可共用一份存储与恢复语义），不为 `--resume` 先单独造。**本轮未发现「已有可用落点且改动很小」的情形** ⇒ 不动手。
    - **仍缺什么**：`--resume`（见上，暂缓）。
    - **用户已裁定（2026-10-11，两条，按现状定稿、无需改代码）**：① **单发判据保持现状** —— 单发 = 「带 `--print`/`-p` **且未**声明 `--input-format stream-json`」；这是与字面「`--print` 即单发」的**有意偏离**，换取「宿主两个 spawn 路径零改动、常驻模式逐字节不变」（宿主那两处本就传 `--print` + `--input-format stream-json`）。② **「工具被拒但模型仍作答」时退出码保持非 0** —— 只要有工具因单发审批策略被拒就非 0，脚本断言才能灵敏发现「这次结果靠不住」。
    - **已实现（2026-10-11）：API 报文调试转储**。此前「看不到请求体 / 响应体原文」的缺口（现有可观测性只有 usage 级：`result.usage.requests[]` + `usage_delta` + `agent-*.log`）已补：agent 侧 `LUNAC_DUMP_API`（**默认关**，真值 `1` / `true` / `on` / `yes`）开启后，把每次 API 调用（主循环流式 + 子代理 + 摘要）的 request / response 报文各写一行到 `<log_dir>\api-dump.jsonl`；`body` 落盘前过 `mask_secrets()`，**请求头一个都不记**（连头名也不记），单条 body 截断 64K 字符、超限记 `truncated` + 实际长度，写失败只 warn。落点与目录解析沿用 `core-agent/src/log.rs` 的 `log_dir()`；测试脚本用法（`-DumpApi`、`summary.json` 字段、记什么/不记什么）见 `docs/testing.md` §测试 10。
    - **本脚本用到哪些**：`scripts\agent-e2e.ps1`（见 `docs/testing.md` §测试 10；**默认瞄准 release 产物**，dev 需显式 `-Dev`）有**两条路径**：① 常驻 stdio（默认，与前一致）—— 复用**全部已实装**参数（`stream-json` 输入输出 + `--permission-mode` + `--add-dir` + 可选 `--mcp-server`），靠「写 stdin 行 → 关 stdin → 等退出」跑单轮 / 多轮，用 `stream.jsonl` + `summary.json` 取最终文本 / 工具序列 / 逐请求 usage；② **单发 `-Print`（2026-10-11 新增）** —— 走刚实装的 `--print --output-format json`，一次一条 query、取一个 `result` JSON（最终文本 / usage / 成败），更简单但无工具序列（故不支持 `-ExpectTool`）。两条路径都**未依赖任何未实现参数**。
- **S1 本地决策旁路**：`local-engine`（llama.cpp，CPU 优先）+ 小 GGUF；**只做五类结构化决策**（能力路由 / 安全预筛 / 记忆候选抽取 / 检索查询改写 / 计划续跑判据），**不产生可见文本、不动主请求前缀**（不废缓存），**不承担 `tool_use` 往返**。**前置**：模型许可允许再分发、内存 / CPU 下限与降级路径（不满足则整个决策层透明关闭）、性能压测。
- **S2 长任务托管**：任务队列 + 检查点落盘 + 跨回合续跑（预算不再终止计划）；落 `ModuleData\tasks\`。**最小档已落地**（预算撞顶后按「`TodoWrite` 清单是否未完成」续命 2→6）；**仍未做**：任务级预算、跨回合后台续跑、调度层与队列。
- **S3 子 agent 团队**：常驻角色（planner / executor / reviewer）+ 任务 DAG（**需重开架构**）。
- **S4 持续自治 + IDE 附着**：`vscode-extension/` 变成同一引擎的薄客户端（复用工具 / 审批 / 记忆 / 技能），后台常驻按需唤醒。
- **已对齐的验收先行件「决策保真度」实验**：三臂对照（A 现状体积驱动压缩 / B 纯大上下文 / C 本地策展）+ 6 个埋约束的长任务套件 + 三层指标（决策保真度为主）。**阶段 1（可立即做）= 只跑 A vs B，先校准指标本身。**
- **本地审查 / 策展可行性结论**：小模型只宜做**受约束三态判定**（`supported`/`violated`/`unsure` + 必引 span）；**确定性优先**（引用存在性 / 声称改动 vs 实际 diff / 测试覆盖缺口先用工具判）；**不确定就 abstain、默认不拦**；拦截只对「高置信 `violated` 且不可逆 / 高危」同步拦，其余只提示 / 记日志。**落盘独立「决策账本」，不进 `chat.db`**。
- **`TeamCreate` / `TeamDelete`**：随 A13 重估后暂不做（子代理是「同批并发、跑完即散」，无常驻团队；真要做需先重开架构）。

### §待办 4 补 记忆层演进 + 其他 LLM 类工具（按价值排序）

> **来源**：用户 2026-10-11 两条诉求 —— ① 「记忆层演进（LLM-Wiki 范式）」登记为一条方向；② 把调研到的其他 LLM 类工具/实现**按价值**整理进来。
>
> **纪律**：**本小节仍守 §待办 4 顶部的规矩 —— 不构成待办、不排期；只登记方向与「值不值得做 / 该提升到哪个 §」的判断。**进入实施前先**提升为 §待办 1–3 正文**，此处不写「待实施的任务」。

**B1. 记忆层演进：单文件 `MEMORY.md` → 互链 wiki 目录（LLM-Wiki 范式）**

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

- **数据根 = `<exe 根>`（便携式，无独立 HOME）**：`skills\` / `tools\` / `ModuleData\`（`history\chat.db`、`memory\MEMORY.md`、`usage\usage-*.jsonl`）/ `temp\`（`logs`、`tool-outputs`、`transStorage`、`webview-data`、`review-last`）/ `config\`。
- **`core-agent` 读取的 19 个 `LUNAC_*`**（均**只在 spawn 时注入**；切换其中任何一项 = `kill_and_cleanup()` 重启 agent）：端点 / 凭据 / 模型、`LUNAC_MAX_CONTEXT_TOKENS`、`LUNAC_SUMMARY_COMPACT`、`LUNAC_MEMORY`、`LUNAC_SKILLS_DIR`、`LUNAC_THINKING`、`LUNAC_WORKSPACE_LOCKED`、`LUNAC_DENY_READ_DIRS`（**读禁目录**，2026-10-13 新增，见 §待办 1-4 补「模型目录读禁」）、`LUNAC_HOOKS_FILE`（按 mtime 热重载）、`LUNAC_PERSONA_FILE`（启动读一次）、`LUNAC_MCP_FILE`、`LUNAC_SEARCH_PROVIDER` / `_KEY`、`LUNAC_LOG_DIR` / `LUNAC_LOG` / `LUNAC_LOG_LEVEL`。**（`LUNAC_NUDGE_INTERVAL` 已于 2026-10-11 随复盘触发改造删除，见 §待办 1-6。）**
- 另有 3 个由 agent 自己解析 / 兜底（`LUNAC_MODULES_DIR`、`LUNAC_CACHE_TTL_SECS`、`LUNAC_TURN_BUDGET_TOKENS`）与 2 个只传给 hook 子进程（`LUNAC_HOOK_EVENT`、`LUNAC_TOOL_NAME`）。
- **「什么时候生效」要如实说**：工具与注入块是「重启 agent 才生效」；只有 `hooks.json` 是 mtime 热重载；技能 = 重启 agent，插件 = 立即。

---

> **历史细节见 `docs/archive/agent-feature-backlog.md`（含 §0 已闭环记录、§4.1 长期规划全文、§5 边界声明）。**
