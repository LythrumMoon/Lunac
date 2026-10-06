# Lunac 待办清单（**唯一待办真相源**）

> **用途**：本项目**全部未完成工作**的唯一登记处。`ai-spec.md` / `agent-implementation.md` / `agent-ui-spec.md` / `architecture-rendering.md` **只保留规则与现状，不再各自维护待办**；任何新想法一律登记到本文。
>
> **三条维护纪律**
> 1. **做完就删** —— 条目完成后从本文**删除**，不标 ✅ 留在原地。「为什么这么做、踩了什么坑」的结论落进 `ai-spec.md` §11 规则、代码注释与 Git 提交记录，**不由待办条目承载**（本文因此不再有历史包袱，读一遍就知道还剩什么）。
> 2. **每条必须能落地** —— 写清「做什么 / 为什么值得 / 依赖 / 落点文件」；写不出落点的东西属于设想，一律放 §4 设想区，不混进待办。
> 3. **优先级只按本文顺序** —— 级别（P0–P3）是粗档，档内顺序即实施顺序。
>
> **最后核对**：2026-09-30（M2-4 四条新功能真机回归通过后**已撤下**，同时登记回归中发现的 M2-6；**L1 的 Live2D 引擎已落地并真机验收** —— 栈定案为 `pixi.js@7` + `pixi-live2d-display-lipsyncpatch`，桌宠窗里真的渲染出模型（18215 个非透明像素），**只剩「发出去」**（传 Core Release 资产 + 附授权文件 + 推市场）；**L5 的 S0 引擎内核也已落地并验收**（`tuning-engine/` 独立 crate，54 条单测，端到端 `gen`→`render`→`measure` 实测 = 理论 ±0.1dB 口径，下一刀是 S1）；新增 L9 / L10 与 M2-5。L1–L8 其余内容仍是 2026-09-29/30 逐条对照 `core-agent/src/`、`app/src/`、`app/src-tauri/src/` 实测的结论，非照抄旧文档）
>
> **口径说明**：本文的前身是「对照旧 `cli.exe` 的完整差距清单」。差距面已核对完毕（见 §5 边界声明），**已完成项全部撤下**，现只保留未完成项。Lunac 的目标仍是 **一个可以完全类比于完整 agent 类应用**的能力体（见 [agent-implementation.md](./agent-implementation.md) §1）—— 所以下面的分组只是**优先级**，不是**价值否定**。
>
> **旧编号（`backlog §N`）对照**：代码注释与规范里仍可能写着旧编号。**只有下面三行对应的东西还活着**；其余旧编号（`backlog §0`–`§8.x` —— 含 `§2.1`–`§2.3` 子系统组、`§3` 协议接口层、`§5` 空转 UI / 失实文案、`§8.1`–`§8.6`）**对应的条目全部已完成并从本文删除**：顺着旧编号只能得到「已完成」这个结论，细节去 `ai-spec.md` §3.5 / §11 规则（32 / 37 / 39 / 56 / 64 / 65）与 Git 记录里找，**不必逐处回改注释**。
>
> | 旧编号 | 现在在哪 |
> |---|---|
> | §1.2 组 B / §1.3 组 C / §2.3 组 C | **A13 按需重估**（+ §4 设想区） |
> | §6 建议实施顺序 / §8.6 建议顺序 | 本文 **「优先级总表」** |
> | §0 差距总览 / §4「不是缺口」 | **§5 边界声明**；其余见 §4 设想区 |

---

## 优先级总表

| 级别 | 条目 | 为什么在这个位置 |
|---|---|---|
| **P2** | L1 Live2D 桌宠 | **用户已定方向**；桌宠三条**已逐条裁决**（2026-09-21，见 L1-A / L1-B）：桌宠 = **插件** + **独立透明置顶窗** + **模型由用户自备**，安装包零第三方模型资产。插件市场那一层、最小原型（2026-09-29，见 `architecture-rendering.md` §6.2）、宿主侧的**鼠标穿透**与**「可见吗」下发**（`ai-spec.md` §4.8）、以及**桌宠外壳本体**（2026-09-29：清单声明窗口形态 + `Modules\pet\` 的「控制台 + 桌宠窗」两窗 + 形象导入，实机验收见 `ai-spec.md` 文末实测表）以及 **Live2D 引擎那一层**（2026-09-30：栈定案 = `pixi.js@7` + `pixi-live2d-display-lipsyncpatch`，引擎拆成「轻/重」两个文件并由附加入口单打一份、Core 走 `dependencies` 自托管带版本副本，**真机验收：桌宠窗里渲染出 18215 个非透明像素 / 310 种色、缩放与 Idle 动作都在跑、最小化即冻结**）**都已就绪**；**「发出去」也已完成（2026-10-01）**：Core 已传成公开 Release 资产（`live2dcubismcore-5.1.0`，**三份**含授权文件，digest 与本地逐字节一致）、pet `0.9.9` 与市场索引已推上 `LythrumMoon/lunac-plugins`。**只剩「从市场装一次、走完整链路」的端到端验收**，详见 L1 正文 |
| **P2** | L5 自研调音插件（DSP 引擎 + 测量 + AI 调参） | **用户 2026-09-29 定方向**：自研 DSP、脱离 EqualizerAPO / Peace（只作学习参考）。体量远大于 L1，但 S0（引擎内核，离线可单测）/ S1（自身播放闭环）/ S2（loopback 测量）**都不碰系统音频、不要驱动、不要 UAC**，可独立验收。**S0 已完成（2026-09-30）**：`tuning-engine/`（独立 crate，手写 FFT，只依赖 `serde_json`）+ 七模块 + 四子命令 CLI，**54 条单测全绿**，端到端「`gen`→`render`→`measure`：实测 +5.94dB = 理论 +5.94dB」；id 定案 `tuning`，落点 `Modules\tuning\`。契约见 `ai-spec.md` §4.10 + 规则 72，实测见 §3.5 文末。**下一刀是 S1**（面板↔引擎的 NDJSON + 只放 Lunac 自己的音乐）。**排在 L1 之后**（档内顺序即实施顺序） |
| **P2** | **L6 成本归因与降本** | **用户 2026-09-29 提出**（「Lunac 的 API 金额是 Trae 的 10 倍」）。归因已结案（`ai-spec.md` §9.1 难点 3：11.2 倍 = 命中率差 3.09× × token 结构差 3.63×，**不是计价贵**；那两档价是官方的**峰谷定价**）。**已完成六件**：子代理 / 复盘用量并入 `result.usage`（§3.5）、思考档 A/B 实测（非主因）、**价格表支持时段价 + 预置官方峰谷价**（§3.5 + 规则 63，面板金额已与账单逐分对上）、**输出瘦身**（输出纪律进 `PERSONA_AND_STYLE`，稳态每次提问成本 **-41%**，§9.1 结论 7）、**压请求次数已结案**（`TOOL_PARALLELISM` = 「同响应多 `tool_use`」，提示词层引导**实测未生效** ⇒ 请求数是模型自己的往返节奏，勿动并行实现）、**对账基线工具**（`scripts\reconcile-usage.ps1`）。**✅ 2026-10-06 跨源基线已跑（用户给的 `usage_data_2026-10-02` 真实账单 CSV）**，并因此修掉两个真问题：① **法定节假日全天按谷价**（官方脚注「周一至周五**不含中国法定节假日**才算高峰」）—— 10-02 是国庆假期，原表把 14/16/17 点按峰价算、**高估 0.979 元**；现在 `pricing.json` 多了顶层 `holidays`（2026 国务院放假安排），前端 / 对账脚本 / 宿主预置表三处同改 + 老文件补键迁移。② **本地账漏记「没跑完的回合」**（原实现只在 `result` 收尾落账，被取消 / 中断 / agent 被强杀就一条都不落）—— 实测当天 agent 日志 **389** 次请求、平台 **401** 次、本地 jsonl 只有 **308** 次（一轮 36 次请求整轮没落账），本地金额只有平台的 **53%**。修法见 ai-spec §11 规则 87：agent 逐请求发 `usage_delta`、前端累计、三处兜底落账（`partial` 标记）。**验收口径**：拿平台 CSV 的四类 token 原样造成本地账跑 `reconcile-usage.ps1` ⇒ **差额 0.000000、exit 0**（已过）。**仍未做**：用**修复后**的某天（≥10-06）再导一次 CSV，验证「没跑完的回合」这条在实机上也补齐 |
| **P2** | **M2-16 缓存 miss 归因（后台输出无预算）** | **2026-10-02 用平台导出 CSV + release agent 日志定位完毕**：那 71 万 miss token 的直接来源是**后台命令的 stdout/stderr 原样进 history，一次 `apply_budget` 都没过**（实测一条失控 PowerShell 刷出 512 KB stderr ⇒ history `361,129 → 841,816 字`），再叠上「压缩无防抖（每轮都压）+ 摘要失败退化 + `ElideCold` 300s 判冷太激进 + 重启改 system 前缀」四个放大器。**✅ 2026-10-06 A–E 五条全落地**（A/B/C 见正文首段，D「两块挪出 system」见正文 2026-10-06 段，E 由 M2-12 消除；`cargo test --bins` 147 passed） |
| **P1** | **L8 应用自更新** | **用户 2026-09-30 提出**（「检查更新 + 自动更新」，并先审了「旧数据自动替换」是否存在）。**第一版已落地**：`updater.rs`（拉 `latest.json` / 版本比对 / 下载带进度 / sha256 校验 / 拉起 `Setup.exe /S` / 退出交棒）+ 设置→常规 的更新区 + 启动后 5 秒静默检查（齿轮打点）+ `scripts\publish-release.ps1`；同时把「旧数据自动替换」缺的三条补进 NSI（`InstallLocation` 定位旧安装 / 装前停进程 / 静默装完自启）。口径见 `ai-spec.md` §8.4。**未实机验证** —— 要先发一个 Release 才验得了 |
| **P2** | **L12 插件体系扩展**（2026-10-06 用户定） | **✅ 2026-10-06 两步全落地**：① 「插件可自带 UI 框架」已写进 `Modules\README.md` §4.4；② sidecar（档 1）契约定稿并**已实施** —— 宿主代请求、独立信任门（`config\plugin-trusted.json`）、面板绑定、`process.spawn` + 清单 `sidecar` 段、插件侧走 `__lunac_host.sidecar`（`apiVersion` 2）；顺手收口了三条「任意 URL」宿主命令的 loopback 旁路。见 L12 正文 |
| **P2** | **L5 自研调音剩余**（A-B 盲测 / 实时频谱） | 引擎 P3（`GraphicEQ` / `Delay`+声道复制 / `Convolution` / `If-Else`）与宿主接线均已落地。**✅ 2026-10-06 A-B 盲测已落地并实机验证通过**（两个快照槽 + 随机映射 + 揭晓；用户「盲测结束」）—— **本档结案**；**实时频谱经用户 2026-10-06 决定「暂时不放了」，暂缓**（不是取消，见 M2-11 / L5 正文） |
| **P3** | **U1 代码变更 diff 卡** / **U2 多轮缩略导航** | **✅ 两项均已落地（2026-10-06）**：U2 = 历史抽屉轮次缩略（会话 Fork 经用户裁定取消）；U1 = 事后审阅 + 用快照回滚（接受=标记已阅 / 拒绝=写回快照；三级粒度见 §2 表 U1 行）。**U3 真沙箱调研**仍未排期 |
| **P3** | **L10 桌宠 × AI** | **用户 2026-09-30 提出**（屏幕获取 / 内置进 agent 做反应 / token 消费独立成表）。**依赖 L1 的 Live2D 引擎那一层**；三条互相依赖，且会**持续烧 token** ⇒ 默认关 + 频次上限是前置条件。**🟢 用户 2026-10-06 把本项排进下一批** |
| **P3** | A13 记忆目录 / 斜杠命令 / 剩余工具 | 按需重估，见各项 |
| **P3** | M1 两个待实测项 / M2 三条待验收 | 不阻塞任何开发，攒到复现 / 验收时做 |

---

## 0. 本轮已修（2026-10-02，闭环记录，不必再排期）

**M2-9. 音乐插件：长连接断开后播放控制条条 `404 NO_ACTIVE_DEVICE`**（用户报「播放时间过长会产生无法播放」）。

- **取证**（release `D:\Lunac\temp\logs\lunac-2026-10-02.log`）：librespot 与 Spotify 的长连接**每 ~27–36 分钟断一次**（`Websocket peer does not respond` → `Connection to server closed`；重连时 `os error 10060`（超时）与 `TLS error … unexpected EOF during handshake`）。重连后 librespot 自报 `active device is <>`（空）—— 此后 `PUT /me/player/pause` 条条 `404 NO_ACTIVE_DEVICE`（14:24 / 15:00 / 16:41 / 16:43）。
- **决定性证据**：**同一时刻的「选歌」却是成功的**（14:24:14 pause 被拒 → 14:24:19 `Loading <Love Runs Out>` 成功）—— 因为选歌走 `resolve_play_device`、按设备名认到 Lunac 并带了 `device_id`。⇒ 设备**还在设备列表里**，只是不再是「活跃设备」。
- **修法（原版）**：`spotify_control` 的控制动作（pause / next / previous / seek / volume）**一律带 `device_id`**。新抽的 `resolve_control_device()` 优先级 = 活跃的 → 本机 Lunac，但**不做「列表里第一台」的兜底**（把 pause 打到用户的手机上，比明确报错更糟）；`with_device()` 按 URL 里有没有 `?` 选分隔符（`seek` / `volume` 本来就带查询串）。单测 `control_urls_append_device_id_with_the_right_separator` 钉住分隔符。
- **⚠️ 修法（2026-10-02 二改，取代上一版）**：原版给控制命令塞 `device_id`，但那条对**非活跃**设备会回 **`403 Restriction violated`**（release 日志 `13:28:58` 实测 `pause?device_id=…` → 403；Spotify 官方口径就是「给非活跃设备 id 会这样」）。现改走**官方推荐的路子**：控制命令**一律不带 `device_id`**（对准活跃设备）；一旦吃 `404 NO_ACTIVE_DEVICE`，就用 `local_device_id()`（**只按名字认 Lunac**，仍不挑「列表第一台」）先 `transfer_to(play:false)` 把它扶成活跃设备，**再重发一次**（只重试一次，转移用 `play:false` 不顺手放歌）。`resolve_control_device()` / `with_device()` 及其单测已删除。
- **根因不在 Lunac**：本机没有代理 / TUN 进程、系统代理关闭、`music.json` 的 `librespot_proxy` 为空 ⇒ librespot **直连** `ap-gae2.spotify.com:4070`，跨境长连接被周期性掐断。**要根治得让它走代理**（代理插件已于 2026-10-02 落地，见下面 **M2-13**）。
- **同批收尾：Spotify 429 的真凶 = `/me/player*` 这一族的端点级长处罚；做法 = 少打 + 按 `Retry-After` 停够 + 保留手动恢复**（2026-10-03 定案）。实测同一个 token：`/v1/me`、`/v1/search` 都 200，只有 `/me/player*` 全 429，`Retry-After ≈ 45000s`（≈12.5h，46s 采样降 46s ⇒ 倒计时到固定复位时刻），且**换 token 也不重置** ⇒ 按 app 计的端点级处罚。成因是旧版把这一族端点当 **1Hz 心跳** + 429 后仍按秒重试（`/me/player` 的失败当时不落盘，连被打爆都看不见）⇒ 被升级成长时间封禁。改法：状态轮询 **15s + 交互驱动 + 隐藏即停**；拉闸时读 `Retry-After` 并**默认停到到期**（不自动过期）；**保留** `spotify_resume` 手动出口，按钮带倒计时。逐条见 ai-spec §4.6「Spotify Web API 限流」与预检 #62。

**M2-10. 聊天窗开局把整段会话渲染成 DOM ⇒ 单个 renderer 3.87 GB**（用户报「一开启 AI 插件就大量占用内存」）。

- **取证**（CDP 探针，只读；探针留在 `%TEMP%\lunac-cdp-probe.mjs`）：一份 45,216 条消息的会话 ⇒ **1,088,429 个 DOM 节点 / `innerText` 2470 万字**，其中 `.msg-actions` / `.msg-copy` / `.msg-rollback` 各 45,216 个、`<svg>` 115,268 个；对应 renderer **工作集 3.87 GB / 私有 3.83 GB**（同一时刻主窗口 322 节点 / 88.9 MB）。JS 堆只有 196 MB ⇒ 大头在 Blink 的节点 / 布局 / 绘制对象上。
- **修法**：① 动作条**懒创建**（`installLazyMsgActions`，委派挂 `#results-list`）；② 聊天流**窗口化渲染**（`CHAT_RENDER_WINDOW = 120` + 顶部「载入更早」，更早的从内存 `chatHistory` 取）；③ 新增**「提问节点」**（每条提问一个节点、点击跳转，目标在窗口外先扩窗）。**2026-10-02 用户要求改形态**：入口搬到**右侧竖排小圆点**——常驻只有轨道、高亮那颗 = 现在读到哪条提问，鼠标移上去才展开列表窗口（轨道封顶 48 颗，超出按比例采样）。七条纪律见 `ai-spec.md` §11 规则 76。
- **顺带发现**：`chat.db` 当时 **284.71 MB**（`pruneContext` 每回合只留 12 轮，正常会话 ≤26 条）—— **已结案并修复，见下面 M2-12**。窗口化只止住了内存，没回答「为什么会长这么大」，这一条补上了。

**M2-11. 调音插件 P0：从「一档预设」扩成「整条可编辑的链」**（承接 L5 S1，用户口径「参照 EqualizerAPO 把调音做到完善」）。

- **Rust 侧**（`player.rs` + `main.rs`）：`TuningFilter`（`kind` / `freq_hz` / `gain_db?` / `q?` / `on`）、`TuningConfig` 从「一个预设键」扩成 `preamp_db + filters`（老配置由 `migrate_tuning()` 把预设展开进表）、`effective_json()`（只发 `on` 的段、**不替引擎补 `gain_db`/`q`**）、`build_chain_from()`（**唯一**编译入口，一律过 `ChainConfig::from_json`）、`TuningDto` 带 `kinds` / `preamp_db` / `filters` / `response`；新命令 `player_tuning_set_enabled`（只改开关）/ `player_tuning_apply_preset` / `player_tuning_set_chain`（覆盖整条链，`preset`→`custom`）。4 条新单测。
- **前端**（`music.ts` / `styles.css` / `i18n.ts`）：调音页 = 总开关 + 预设 + **频响曲线画布**（画宿主的 `response`，20Hz–20kHz 对数轴）+ 预增益 + **滤波器表**（逐段 ON/OFF、类型自绘下拉、频率 / 增益 / Q、删除、加一段）。类型下拉**自绘**（透明窗口里原生 `<select>` 不渲染）；数字框按 `change` 提交并**还原焦点**；草稿与宿主真值分开（失败回滚）。新文案 × 5 语言（含 10 个滤波器类型名）。
- **口径改写**：预检 #51 ③ 从「下发给宿主的只有预设键」改成「**允许下发滤波器数组，合法性一律由引擎 `ChainConfig::from_json` 逐条校验兜底**」（用户 2026-10-02 拍板）。契约见 `ai-spec.md` §4.6「调音」。
- **仍未做（2026-10-06 更新）**：~~A-B 盲测~~ **✅ 已落地**（两个快照槽 + 随机映射 + 揭晓，见下）；**实时频谱 = 用户 2026-10-06 明确「暂时不放了」⇒ 暂缓**（数据源已勘探清楚：`player.rs` 的 `TuningSource::next()` 是唯一音频回调点，落成「无锁 ring + 后台 FFT 线程 + `emit("tuning-spectrum")`」即可；引擎 `fft.rs` 的 `fft_in_place` / `hann_window` / `next_pow2` / `spectrum_db` 可直接复用）。（`GraphicEQ` / `Delay` + 声道复制 / `Convolution` / `If-Else` 条件段**引擎侧 + 宿主接线**均已于 2026-10-03 完成，见 §0 M2-11 的 P3。）

- **A-B 盲测（2026-10-06，闭环记录）**：用户口径「两个快照槽 + 随机映射 + 揭晓」。
  - **宿主**（`app/src-tauri/src/player.rs`）：新增 `TuningAbState{a,b}` + `ab_slot_path()`（只认 `a`/`b`，落 `config\tuning\ab-a.json` / `ab-b.json`）+ 三条命令 `player_tuning_ab_state` / `_save` / `_apply`。**存之前先过 `build_chain_from`**（存一条编不成链的快照 = 埋雷）；**应用走唯一写点 `apply_tuning_config`**（#51 ③：先校验 → 落盘 → 改内存 → 版本号+1）⇒ **切 A/B 不重播**，当前这首下一个样本就换链。
  - **前端**（`music.ts` 调音页左栏）：新增 `#music-tune-ab` 一块 —— 「存到 A / 存到 B」+ 两个切换键 + 「盲测」+「揭晓」+ 一行 hint。**盲测是纯前端的事**：`tuneAbSlots` 决定「左键/右键 → 哪个槽」，开盲测时随机换序，界面只显示 `1` / `2`，点「揭晓」才写出 `1 = A，2 = B`。两槽都有快照才能开盲测（只有一个可切时隐藏没意义）。**不跨挂载**（面板一关，这次盲测结束）。
  - **如实记**：**不做自动等响度**（A/B 受音量影响是用户自己的事；preamp 纪律见规则 72「削波只暴露不处理」），所以 hint 里提示用 preamp 对齐即可 —— 这一点是刻意的，别「顺手」加自动增益。
  - **i18n**：`music.tuning_ab_*` 11 条 × 5 语言；CSS `.music-tune-ab` + `.music-btn.active`。
  - **插件版本**：`music 0.9.26 → 0.9.27`（包内容变了必须 +1，预检 #43 ④）；`build-plugins.ps1 -Plugin music` 跑通（zip 54.6 KB、dev 的 `target\debug\Modules\music\` 已同步，产物里 `music-ab-pos1` 与 `player_tuning_ab_apply` 都在）。
  - **验证**：`cargo check` exit 0；`cargo test --bins` **206 passed / 0 failed / 2 ignored**；`npx tsc --noEmit` exit 0；`npm run build` exit 0。
  - **实机验收（2026-10-06，已通过）**：开 dev 面板 → 存 A / 存 B → 切 A/B → 开盲测 → 揭晓，全链路可用（用户确认「盲测结束」，本档结案）。**仍未做**：发布（`publish-plugins.ps1` 未跑，市场里的 music 仍是 0.9.26）。
  - **补记（2026-10-06）：调音页频段数字框「点进去输入被取消」= 幂等重绘判据失效（已修）**。用户报「具体频率/增益的输入框不是改成只有失焦和回车才保存吗，怎么点进去输入就没了」。**根因不在频段框的事件委托**（那部分是对的：`input` 只接 `range`、文本框直接 `return`，`change` 才提交）—— 而在 `setHtml`（`music.ts`）的判据 `el.innerHTML !== html` **恒为真**：浏览器把 void 元素序列化时会把模板里写的 `<input … />` 读成 `<input …>`，于是「读回来 ≠ 写进去」永远成立 ⇒ `renderTuningFilters` 被 1.2s 轮询拖进**每轮整块重建** ⇒ 正在聚焦的 `<input>` 被换成新节点、焦点被踢回 `<body>`（日志铁证：`focusin INPUT.music-tune-num` → 1s 后 `focusout active=BODY` → `table-rebuilt`，之后每 ~1.2s 一次）。**修法两处**：① `setHtml` 先 `document.createElement("template")` 把待写 html 归一化，两边过同一套序列化再比；② `renderTuningFilters` 叠一道焦点守卫 `if (box.contains(document.activeElement)) return;`（用户正在这张表里编辑就不重绘）。临时探针（`TUNEDBG`）已删干净。纪律固化进 `code-rules.md` 预检 **#74**。
  - **补记（2026-10-06）：A-B 日志**。曾一度给 `player_tuning_ab_apply` 补日志、给 `save` 日志加内容概要（想复盘盲测），**随后按用户口径「不用添加日志」全部撤回** —— 盲测已结束、存/切的可观测性够用，日志保持原样（只有 `save` 的「已存入 X 槽」）。纪律（#73）也回到四条。

**M2-12. `chat.db` 298 MB / 一份会话 4.5 万条 ⇒ 根因是 `pruneContext()` 的数组别名 bug**（用户报「真正提问只有几条，不该有几万条被记录」）。

- **取证**（只读探针 `%TEMP%\chatdb-probe.mjs`，用 `bun:sqlite` 只读打开）：库 **298.5 MB**（`sessions` 2 条 / `messages` **45,218** 条，其中一份会话 **45,216** 条）；**全文只有 30 种不同的 `content`** —— 同一段 26 条（13 回合）的对话**重复了 1728 次**（`45,216 ≈ 26 × 1728`）。`messages_fts` / `messages_trgm` 两张 FTS 表各再存一份全文 ⇒ **同样的内容在库里存了三份**（体积 ×3 的放大器，不是根因）。
- **根因**（`app/src/main.ts` 的 `pruneContext()`）：分组时 `turns.push(current)` **忘了 `current = []`** ⇒ `turns` 里 N 个元素**全是同一个数组的引用**（后面 `current.push(m)` 继续往那个"已收尾的回合"里加），于是 `turns.slice(-12)` 拿到的是**整段对话的 12 个引用** ⇒ 函数返回「整段对话重复 12 遍」。
- **实证**（`%TEMP%\prune-sim.mjs` 跑同一段逻辑，13 回合 / 26 条）：带 bug **26 → 312 → 3744 → 44928 → 539136**（**44928 = 26 × 12³，与库里实测的 26 × 1728 逐位吻合**）；修好后 **26 → 24 → 24 → 24**。
- **为什么没人发现**：`pruneContext` 在两处被调用（每回合的 `chatHistory`、以及落盘快照），所以是**每保存一次 ×12**——不报错、不崩、只让数字慢慢变大，而它自己叫「压缩上下文」，看起来一切正常。
- **修法**：`turns.push(current); current = [];`（一行）。纪律见 `ai-spec.md` §11 规则 77。
- **未做**：那份 298 MB 的库**没有清** —— 它是用户数据，且 release 版正在跑（库被它打开着）。要清得先停实例、并先把那份会话去重成它真正的 26 条（或整条删掉）。
- **已清理（2026-10-02，用户批准）**：不等「下次保存自动塌回」，直接手工纠正 —— 按**首次出现顺序**去重后重写该会话（45,216 → **28 条**：user 15 / assistant 13），再重建两张 FTS 索引表 + `VACUUM`。**298.54 MB → 0.81 MB**。备份留在 `<exe 根>\ModuleData\history\chat.db.bak-2026-10-02`（284.27 MB，确认无误后可删）。两条经验：① **`VACUUM` 不重建虚拟表** —— 只删数据再 VACUUM，FTS5 里那些已删除的分段会把文件卡在 98 MB（第一次实测就是这个数），要 `DELETE FROM messages_fts` + 按 `rowid` 重灌 + `INSERT INTO messages_fts(messages_fts) VALUES('optimize')`；② 全程在 release 实例**运行中**做（WAL + `busy_timeout`），库没有被锁住、实例也没受影响。

**M2-13. 代理插件落地：系统代理管理 + librespot 一键联动**（2026-10-02，用户口径「新建市场插件 Modules\proxy」+「要，一键联动」；它是 **M2-9 的根治手段**）。

- **宿主侧** `system_proxy.rs` **2026-10-01 就写好了**（5 条命令 `proxy_system_get` / `proxy_config_get` / `proxy_config_set` / `proxy_enable` / `proxy_disable` + `restore_on_exit()` 已挂退出路径），**但前端 0 处调用 ⇒ 一直是死代码**；本轮才接上。
- **新增** `librespot_set_proxy`（`music.rs`）：代理是 librespot 的**启动参数**（`-x`）、进程中途改不了 ⇒ 这条链路必须宿主独占（校验 → 落盘 → 值没变不重起 → 重起）；前台不许「落盘后再自己 stop + start」。
- **`-x` 收口**：它此前是在 `librespot_start_inner` 里**另追加的一份**（预检 #46 ① 的漏网之鱼）⇒ 收进 `librespot_args()`，补单测。`-x` **只认 HTTP 代理**；socks5 放过去是**静默失败**（日志有 `Using proxy`、就是连不上）。
- **`ProxyConfig` 加 `link_librespot`**：跨模块配置（代理 ↔ librespot）存**宿主**的 `proxy.json`，不存插件 localStorage —— 否则「换机器 / 清缓存后，代理列表还在、联动悄悄没了」。语义边界：开着它时**关闭代理会把 librespot 也掰回直连**（哪怕那个值是用户在音乐插件里手填的），这是字面意思、不是 bug。
- **前端** `app/src/plugins/builtin/proxy.ts`（市场插件 `Modules\proxy` **v0.9.0**，接管型 `permissions: ["layout.takeover"]`）：列表增删启用 + **真实现状回读**（读注册表，不从 `cfg.active` 推）+ **差异如实报**（`proxy.state_mismatch`，组策略压回时）+ socks5 行内提示 + 联动开关。
- **契约**：`ai-spec.md` §4.11；预检 #46 ① 已补 `-x` 那一条。
- **已发布（2026-10-02）**：`build-plugins.ps1`（全量 6 个）→ `publish-plugins.ps1` 推送成功（`0a30dc7..cfacb7b`）；远端 `index.json` 经 `gh api` 核实 **6 条**（`proxy 0.9.0` 在内）。
- **安装包已重打（2026-10-02）**：`build-release.ps1`（patch 自动 +1 → **0.9.19**）产出 `release\Lunac-0.9.19-Setup.exe`（18,169,244 B）；`lunac-installer.nsi` 新增 `Section /o "Proxy" SecExtProxy`（勾选项由 5 → 6），7z 列清单确认含 `Modules\proxy\{index.js,lunac-plugin.json}`，且含 `agent.exe`、不含 `cli.exe`。打包时**没有** `lunac.exe` 在跑（`taskkill` 那步空转），release 实例未受影响。
- **未做**：真机验收（面板 + 联动）；代理软件本身（clash / v2ray 等）仍需用户自备 —— 插件只管把系统 / librespot 指过去。

**可走国内支付的 VPS（WebSearch 核实 2026-10-02）** —— 用户问「给我些合理的服务器，可走国内支付」：

| 服务商 | 支付方式 | 注意 |
|---|---|---|
| **搬瓦工 BandwagonHost** | **续费账单支持支付宝 + 银联**；但 **Add Funds（充值）只支持 PayPal / 信用卡** | 想用支付宝只能等账单出来直接付，不能先充值 |
| **Vultr** | 支付宝 + 银联 / PayPal | 付款页地址要填**拼音姓名 + 拼音地址 + 6 位邮编**，缺一项报 `fill out all Address fields` |
| **RackNerd / HostDare / DMIT** | 含**微信支付**，$19–22/年起 | 年付促销款；机房多在美西，晚高峰延迟看运营商 |

> ⚠️ 这只是**购买建议**，不是代码依赖：插件不绑定任何服务商，填进列表的只是一个 `http://主机:端口`。

**M2-14. 插件子进程必须随插件退出而终结 ⇒ 提成全局规则（预检 #57，2026-10-02 用户要求「优先级调高」）**。

- **触发**：用户原话「librespot 可能不会随着音乐插件的退出而自动终结进程 … **所有依赖都必须跟随当前插件的退出而终结**」。取证：本机实到 **2 个孤儿 librespot**（`D:\Lunac\Modules\music\bin\librespot.exe`，父进程已不在），正是「崩溃 / 强杀那条路没人收」的产物 —— 关窗与退程序两条显式路径都存在，但它们都要求**我们自己的代码还在跑**。
- **根治（两层）**：① 把 `commands.rs` 里私有的 `mod job` 提成共用模块 **`child_job.rs`**（Job Object + `KILL_ON_JOB_CLOSE`），并在**每个插件子进程**的 `spawn()` 之后立刻 `assign`：librespot（`music.rs`）/ PaddleOCR-json（`paddle_ocr.rs`，为拿句柄从 `.output()` 拆成 `spawn()` + `wait_with_output()`）/ ffmpeg（`convert.rs`）/ agent.exe（`commands.rs`）。② 长期驻留的仍走显式杀（librespot 样板不变）。
- **顺带踩到**：`.output()` 拿不到句柄；而 `spawn()` **不**替你设管道 ⇒ `paddle_ocr.rs` 必须显式 `.stdout(Stdio::piped()).stderr(Stdio::piped())`，漏了就是「识别成功但一个字都没有」。
- **规则落点**：`code-rules.md` 预检 **#57**（已进文首「🔴 高优先级条目」清单）+ `ai-spec.md` §11 **规则 78** + §4.6「孤儿 librespot」那段补「已在代码上根治」。
- **验证**：`cargo check --bins` exit 0；`cargo test --bins` **178 passed / 0 failed**。
- **安装包已重打（2026-10-02）**：`build-release.ps1`（0.9.19 → **0.9.20**）产出 `release\Lunac-0.9.20-Setup.exe`（18,174,163 B）；7z 校验含 `agent.exe`、不含 `cli.exe`、含 `Modules\{music,proxy}\`。
- **孤儿已清**：那 2 个（PID 8484 / 18396）在动手前已自行退出，复查本机已无 `librespot` 进程；**新机制管不了「机制生效之前」生出来的进程**，所以这类清理只能靠外部手段（按 exe 路径筛）。

**M2-15. 待办清单落盘 + 审批卡停靠输入栏 + 完成提示音**（2026-10-02，用户四条同批要求）。

- **待办清单不再随回退消失**（用户原话「代办清单有可能随着历史记录的回退而消失 是否可以添加到记忆的数据库中也存储进去」）：`chat.db` 的 `sessions` 加 `todos` 列（`SessionTodo` 时间线），按**回合**归档（`turn` = 采集时已完成的助手消息条数，与 `SessionProcess.turn` / 文件快照同口径）；回退到 idx 时取 `turn < K` 的最后一条（K = `keptAssistantCountFor(idx)`），恢复会话时取最后一条 —— 统一在 `applyTodoTimeline(kept)`。旧 `rebuildChangedFilesFromSteps` 里那句 `todoItems = []`（注释写着「待办不落盘、重建不出来」）随之前提失效而删除。老库靠 `init_schema` 里新增的 `ALTER TABLE sessions ADD COLUMN todos TEXT` 补列。规则落点 = `ai-spec.md` §11 **规则 79**。
- **审批卡 / 计划卡停靠输入栏位置**（用户原话「将其位置放置到对话框中」）：卡从 `agentView.flow`（对话流，会随滚动跑掉）改挂 `#approval-dock`；`syncApprovalChrome()` 一处管三个状态 —— `body.approval-open` 藏输入栏（卡片替代它）、`body.plan-open` 藏任务抽屉（待办 + 文件更改）、**「全部允许 / 全部拒绝」只从计划卡上收**（用户明确确认过只删计划卡上的；普通多命令审批卡保留批量按钮）。计划批准后的留档提示仍落回对话流（`host` 保留）。规则落点 = `ai-spec.md` §11 **规则 80**。
- **完成提示音**（用户原话「结束时 弹出提示音用来提示用户」）：新增 `commands.rs::notify_sound` → Windows `MessageBeep(MB_ICONASTERISK)`（零资源 / 跟随系统声音方案 / 不受 WebView 自动播放策略约束），前端在回合收尾的 `cliDoneCallback` 调 `playDoneSound()`，不 await、失败不上报。规则落点 = `ai-spec.md` §11 **规则 81**。
- **验证**：`tsc --noEmit` exit 0；`cargo test --bins` **180 passed / 0 failed / 2 ignored**（新增 `todos_roundtrip_preserves_timeline` / `legacy_db_gets_todos_column_added` 两条）；`npm run build` 通过；`npm run verify` 8/8。
- **还没进安装包**：以上三项都在 0.9.20 之后 —— 要发给用户需重打（→ 0.9.21）。

**M2-16.（已实施 A/B/C，2026-10-02）缓存 miss 归因：那 71 万 token 是「后台命令输出无预算上限」打出来的**。

- **触发**：用户给出 `D:\Downs\usage_data_2026-10-02_2026-10-02.zip`（`amount-*.csv` / `cost-*.csv`），问「为何 lunac 中间有一段缓存丢失严重」。**数据侧**：逐小时命中率里 `16:00`（本地）那一档 `miss=799,436 / hit=932,224`（**miss 占比 46.2%**）是全窗口 13:00–22:00 唯一的异常段（其余小时 miss 占比 6%–18%）。
- **取证**（release `D:\Lunac\temp\logs\agent-2026-10-02.log`）：08:57–08:58 UTC（= 16:57–16:58 本地）`问#5` 里**连续 4 次请求** `in = 170,660 / 176,754 / 180,739 / 181,509`，而 `read` 只剩 `4,352–13,568`（命中率 7.4% → **2.3%**）—— 四次合计 **≈ 71 万 miss token**，正是 CSV 里那 799,436 的主体。
- **根因 ①（首要）：后台命令的输出没有任何预算裁剪**。证据 `line 514`：`shell[powershell] 后台完成 id=bg_26116_0 … stdout=519B stderr=524342B` —— 一个 `EdgeMap` 逐像素循环刷出 **512 KB stderr**。`core-agent/src/main.rs:1989-2007` 把 stdout+stderr **原样拼接**成 `text` → `BACKGROUND_DONE` → `main.rs:5357-5365` 再拼进最后一条 `tool_result`；**整条路径一次 `apply_budget` 都没过**（前台工具结果都过 `tools.rs::apply_budget`，后台这条绕开了）。于是 history 从 `361,129 字` 一步涨到 **`841,816 字`（+480,687）**。
- **根因 ②：压缩没有滞回/防抖 ⇒ 每轮都压、每轮都重写前缀**。那条 ~83 万字的 history 一直贴着 `DROP_RATIO=0.95`，`compact_history` 于是**每一次请求都触发一次**，且每次只能丢 2 条（巨大消息动不了）→ 日志里连出四条「本侧就地改写了历史（其后整段前缀缓存必然失效）」。
- **根因 ③：摘要压缩失败后退化**。`line 530 摘要压缩失败：模型返回空摘要` —— 本想一次性付「摘要」的代价，失败后回落到「丢消息」，压得更频繁。
- **根因 ④：`ElideCold` 的 300s 判据太激进**。`06:35 / 06:50 / 06:55 / 09:24 / 09:59` 等多次「冷缓存微清理」，每次把命中率砸到 **21%–63%**。判据是「距上次请求 > `LUNAC_CACHE_TTL_SECS`(300s) ⇒ 端点缓存已过期」；但实测 elide 后 `read` 仍有 **6,528**（≈ system+tools 量级）—— 说明端点那份缓存**并没有**全冷，是**我们自己**把 history 段打掉的。DeepSeek 的 context cache 实际是**小时级**寿命（本仓按 5 分钟判冷）。
- **根因 ⑤：agent 重启 ⇒ system prompt 变 ⇒ 从第 0 token 起全 miss**。一天里 system hash 变了 **4 次**（`fe5a49…` → `6729df…` → `f8897d…` → `42bc7a…`），每次重启后首问命中率只有 **11.7%–62%**。原因是 system 里含「往期会话索引 + 长期记忆」的**启动冻结快照**（§11 规则 18 的设计如此），会话一多快照就变。
- **根因 ⑥（已消）**：`10:48 / 13:20` 两条 `丢弃 38296 条旧消息` —— 是 M2-12 的次生灾害（被污染的 `chatHistory` 经 `set_history` 灌回 agent），随 bug 修复 + chat.db 手工清理已消除。
- **方案（待用户确认后实施 —— 用户本轮选「先出分析报告，我确认后再改」）**：
  - **A（必做）给后台输出加硬上限**：构造 `text` 时就过 `tools::apply_budget`（或头尾保留 + 截断），与前台同一阈值。这是那 71 万 miss 的**直接来源**。
  - **B 给压缩加滞回/防抖**：压缩后必须等 history 落到**低水位**才允许再压；同一 `提问` 内最多压一次。
  - **C 重估 `LUNAC_CACHE_TTL_SECS`**：默认调大到 1 小时以上，或**默认关掉 `ElideCold`**（它省的是那一次输入，代价是把还热着的前缀整段作废）。
  - **D（可选，收益较小）降重启频率 / 稳定 system 前缀**：把易变的「往期会话索引 / 长期记忆」从 system 移到消息尾（代价是模型把它们当用户内容看，需权衡）。
  - **E 已验证有效**：M2-12 的修复已消除根因 ⑥。
- **本轮落地（2026-10-02，用户「A B C 都进行、D 暂时不进行」）**，全在 `core-agent/src/main.rs`：
  - **A**：后台收尾线程里那句 `let text = format!("{prog} (background)…")` 之后补一行 `tools::apply_budget(...)` —— 后台输出从此与前台同一个预算出口（预检 #59 ①）。
  - **B**：丢弃档的判据由 `ratio > DROP_RATIO` 改成 `ratio > DROP_RATIO && grew_enough`（复用已有的 `COMPACT_MIN_GROWTH = 0.15` 滞回）—— 不再「每轮都压、每轮重写前缀」；真正的 400 兜底仍是下面那条 `Compact::Force`。
  - **C**：`DEFAULT_CACHE_TTL_SECS` **300 → 3600**（端点侧实际是小时级寿命，取小不是少几次免费压缩，而是**反复白废热缓存**）。
  - **验证**：`core-agent` `cargo test` **130 passed / 0 failed / 2 ignored**（改完直接过）。
  - **D 暂缓**：`system` 前缀里那份启动冻结快照（根因 ⑤）仍在，重启后首问依旧全 miss —— 用户当时明确「暂时不进行」。
- **本轮落地（2026-10-06，用户「做 D：两块挪出 system」）**，全在 `core-agent/src/main.rs`：
  - **D：把「往期会话索引 + 长期记忆」从 `system` 挪出，改注在 history 开头的一条 `user` 消息**。`build_system_prompt` 签名 5 参 → **3 参**（`persona` / `env` / `skills`），system 从此**跨重启逐字节稳定** ⇒ 端点缓存不再从第 0 个 token 起全 miss（根因 ⑤）。
  - **位置三选一体检**：留 `system` 正是要修的病灶；注**消息尾**会踩 `TOOL_BUDGET_HINT` 那条实测红线（`read` 从 11776 掉到 2560，2026-09-20 实测）；**注开头**只动一次、之后结构恒定 ⇒ 采用开头。
  - **新增**：`CONTEXT_BLOCK_HEADER` 表头 + `context_block_message()`（两块皆空 ⇒ `None`，否则拼一条 `role:"user"` 消息）+ `is_context_block_msg()`（按表头前缀判，作**幂等注入判据**）。`run_query` 签名末尾加 `ctx_block: Option<&Value>`，在 `let mut base = history.len();` **之前**注入 —— `set_history`（回退 / 恢复）整份换掉 history 后，下一问能认出并补回（否则表现为「回退之后模型突然不记得长期记忆」）；压缩把它丢掉也会在此补回。
  - **代价（如实记）**：模型把这两块当**用户说的话**看，不再有「系统级背景」的权威感；段内措辞已按「背景，不属于当前请求」写好，但仍不如 system 里硬。
  - **验证**：`core-agent` `cargo test --bins` **147 passed / 0 failed / 2 ignored**（4 处单测改 3 参 + 新增 `index_and_memory_live_in_the_context_block_not_the_system_prompt` 钉住「不在 system / 只在上下文块 / 幂等判据认得」双面不变量）。

**多代理通信（`ListPeers` / `SendMessage`，A13，2026-10-05，闭环记录）** —— 补上 A14 那句「子代理之间**没有任何通道**」。

- **形态裁定**：先判定「主代理 ↔ 正在跑的代理」在当前架构下**没有窗口**（主循环在 `BatchKind::Subagent` 整批跑完前一直阻塞于 `thread::scope`），所以只做**同批兄弟**这一种形态（用户拍板）。要「常驻后台代理 / 团队模型」得重开架构，属另一档（已把 `TeamCreate` / `TeamDelete` 移进 §4 设想区并写明理由）。
- **落地**：新增 `core-agent/src/peers.rs`（`PeerBus` 登记处 + 线程本地「我是谁」+ 五条单测）；`tools.rs` 增 `ListPeers` / `SendMessage` 两件内置工具（总数 15 → **17**）、进 `parallel_safe` 的只有 `ListPeers`；`main.rs` 的 `run_agent_tool` 用 **RAII guard** 注册 / 注销 peer，`run_subagent` **每轮开始** drain 收件箱追加成 `user` 消息；前端黑名单候选名单补两条。**不违反规则 54** —— 消息只进收件方子代理自己的 history。
- **契约与六条纪律**：`ai-spec.md` §3.5「多代理通信」+ §11 **规则 85**；并发类的运维纪律补进 `code-rules.md` 预检 **#28**。
- **验证**：`core-agent` `cargo test --bins` **140 passed / 0 failed / 2 ignored**（+6 条：`peers` 模块五条 + `tools::peer_tools_are_registered_and_ungated`）；`npx tsc --noEmit` exit 0。
- **真机端到端已验收（2026-10-05）**：真端点（`deepseek/deepseek-v4.1-flash`）+ **从仓库新构建的 agent 二进制**，一轮里让模型并发派两个 `Agent`。日志铁证：`固定前缀 tools=17个/10764字`（两件新工具已进工具池）、`子代理并行批 2 条（并发上限 3）: Agent, Agent`、`tool ListPeers ok` / `tool SendMessage ok`、**`子代理 task-1 收到 task-2 的消息（27 字）`**。语义侧：A 的 `ListPeers` 看到 `- task-1 — peer-B` 与 `- task-2 (you) — peer-A`（自己标 `(you)`），`SendMessage` 到 `task-1` 回「Message delivered to task-1 (27 chars)」；B（跑完 `ping` 后）在下一轮读到并**逐字引用** `PING-FROM-A-42 hello from A`。B 的第二次 `ListPeers` 只剩自己 —— 与「已结束的 peer 退出列表」的设计一致。
- **两条运维事实（排查时别踩）**：① **装着的 `D:\Lunac\agent.exe` 是旧构建**（2026-10-04 01:50），不含本批两件工具；要用得先 `cargo build` 再重打包。验收时的判据是 `init.tools` 里有没有 `ListPeers` / `SendMessage`（旧二进制是 15 件、新的是 17 件）。② 中转端点（`api.moonobscura.com`）对**非流式**请求（子代理/复盘走的是 `stream:false`）偶发长时间挂死后失败（实测一次 `子代理请求发送失败` 挂到 **158s**），模型重试即成功 —— 是端点侧可靠性，不是本仓缺陷。

**命令卡改 xterm 交互式终端 + `tool_control` stdin 转发（2026-10-05，闭环记录）** —— 命令类工具（`Cmd` / `PowerShell`）卡片正文从 `<pre class="tool-out">` 纯文本改成 **xterm.js 真交互式终端**。

- **形态**：前端新增 `@xterm/xterm` + `@xterm/addon-fit` 两个依赖，卡片正文用 `<div class="tool-term">` 承载终端（高 220px、深色底），其余工具仍用 `.tool-out`；`tool_output` 的 stdout/stderr 分片直接 `term.write(chunk)`，**不再**做「只留尾部 4000 字符」的截断（终端 scrollback = 3000 行）。
- **惰性创建 / 释放**：xterm 实例**只在卡片首次展开时**建（`<details>` 的 `toggle` 事件）、**收起即 `dispose()`**，内容留在 `card.termBuf`（上限 `TERM_BUF_MAX = 200_000` 字符）以便重建回放；卡片随对话被移除时由 `MutationObserver`（`watchFlowForTermRemoval(flow)`）兜底释放。
- **键入转发**：用户在终端里键入的字符经 `tool_control`（新增动作 `action:"stdin"`、新增字段 `data`）发给 agent，agent 侧 `route_tool_control` 的 `("stdin", Some(c))` 分支写进子进程 stdin（`ShellControl::write_stdin`）；**Enter 发 `\r\n`**。**本地回显**由前端自己画（无 PTY 不会自动回显）：Enter → `\r\n`、退格（`\x7f`）→ `\b \b`、Ctrl-C（`\x03`）→ 回显 `^C\r\n` 并映射到既有的「停止」动作（写 `0x03` 进管道不会真的中断 Windows 进程）。
- **行为变化（如实记）**：`run_shell` **仅在装了控制块**（有前端在看着这次调用）时才把子进程 stdin 接成管道（`Stdio::piped()`），否则保持 `Stdio::null()` ⇒ 单测 / 子代理 / 后台复盘与改造前逐字节一致；而**读 stdin 的命令现在会等用户输入直到超时**（默认 120s），不再立刻拿到 EOF（不读 stdin 的命令不受影响）。
- **边界**：**没有 ConPTY，只做行级 stdin 转发**；方向键 / Tab 补全 / 全屏 TUI（vim、htop）/ 由控制台完成的退格等行编辑**不生效**（本轮明确选定「stdin 转发」而非「真 ConPTY」）。
- **契约与纪律**：`ai-spec.md` §3.5「stdin 转发 + 终端呈现」/ §11 规则 73；`agent-ui-spec.md` §3.3「命令卡终端」；`code-rules.md` 预检 **#64**。涉及 `core-agent/src/tools.rs`（`ShellControl.stdin` / `set_stdin` / `write_stdin`、`run_shell` 的 stdin 分支）、`core-agent/src/main.rs`（`route_tool_control` 新增 `("stdin", Some(c))` 分支）、`app/src/main.ts`、`app/src/styles.css`、`app/package.json`。

**Spotify 后台探测线程删除（2026-10-05，闭环记录）** —— 用户口径「删除这个 Spotify 检测机制，让他能抓取 Spotify 正在播放的音乐并且控制就可以」。

- **背景**：`music.rs` 有一段 `spawn_spotify_watcher` —— 独立线程每 3s 走一次 `CreateToolhelp32Snapshot` 全进程枚举，Spotify **从「没在跑」变成「在跑」**时自动弹音乐插件窗。它是本仓**唯一**「无条件、永不停止」的常驻轮询。先前先做过一版**降开销**（没装插件就不扫进程 + 主窗不可见时退避到 15s），但用户随后**直接决定删掉整套检测机制**，降开销那版一并作废。
- **删除项**：`spawn_spotify_watcher` / `SPOTIFY_WAS_RUNNING` / `SPOTIFY_POLL_SECS` / `SPOTIFY_POLL_IDLE_SECS` / `main_window_visible` / `music_plugin_installed`（`music.rs`）、`plugin_window::is_open`（唯一调用点就是这条线程，随之成死代码）、`main.rs` `setup` 里那次调用。
- **保留项**：`spotify_desktop_running()`（raw FFI 读进程表，`win_proc` 模块）**不删** —— 它还被 `music_autoconfigure` 第 ③ 步用到（「桌面端进程在跑但还没注册成设备 ⇒ 别起 librespot 抢同一个会话」）；这是**一次性**判定（打开面板时才跑），不是后台轮询。抓取「正在播放」与全部控制仍走 Spotify Web API，一条都没动。
- **落点**：`app/src-tauri/src/music.rs`、`app/src-tauri/src/main.rs`、`app/src-tauri/src/plugin_window.rs`。契约同步 `ai-spec.md` §4.x「自动弹出已移除」；`cargo check`（src-tauri）通过。

**调音输入框「误存」修正（2026-10-05，闭环记录）** —— 用户报「输入一个数字就直接保存 / 有时候一个动作就会保存」。**决定不改触发时机**（仍是失焦 / 回车），只修误存，**不加按钮**。

- **① 提交去抖**：数字框（预增益 / 低音 / 高音 / 延迟 / 频段柱文本框）的 `change` 提交改走 `tuneCommitDebounced`（新增 `TUNE_CHANGE_DEBOUNCE_MS = 300`）—— 一次用户动作常连触发多个 `change`（输完点别处 ⇒ 那个框失焦提交，同时被点中的控件自己也提交），合并成**一次** IPC / 落盘。拉条（`type="range"`）松手仍**立即**提交（拖动中的节流提交未必赶上最终值）。
- **② 焦点保护扩展**：`commitTuningChain` 的焦点还原此前只覆盖频段柱里的文本框；现在扩展到预增益 / 低音 / 高音 / 延迟四个框（按 id 还原）—— 少了这一句，任何一次提交都会把用户的焦点踢掉，看着像「被误存 + 被踢出输入框」。
- **落点**：`app/src/plugins/builtin/music.ts`。触发时机、`change`/`input` 的分工与「值没变不提交」的既有判据均未改动。

**Read 读到图片不再返回乱码，改交 image 块给视觉模型（2026-10-05，闭环记录）** —— 用户报「`Read` 只按 UTF-8 读文本，PNG 进去出来是乱码字节流（31 万字节的 `�PNG`）」。

- **根因**：`tools.rs::read()` 对任何文件都 `String::from_utf8_lossy` —— 图片二进制被逐字节替换成 `�`，模型收到一整片噪声。
- **做法**：`read()` 先用既有的 `crate::image_media_type(&bytes)` 按**魔术字节**判定；命中即返回 `IMAGE_SENTINEL`（`[[lunac-image]]`）+ 绝对路径 + 一行说明，**不再走 UTF-8**。`main.rs::tool_result_block` 识别哨兵后，复用 A8 的 `load_image_block` 把它换成 `[text, image]` 块（`[{"type":"text",…},{"type":"image","source":{"type":"base64",…}}]`）；读失败/越界则**退回纯文本**，绝不泄哨兵。摘要渲染 `render_one_message_for_summary` 兼容块数组（`image` 只渲染成 `[image]`，不灌 base64）。
- **哨兵只一份**：`tools::IMAGE_SENTINEL`（`pub const`），`read()` 与 `tool_result_block` 共用，杜绝两处字面量漂移。**不经过 A8 的 `vision` 开关** —— 它是模型主动 `Read` 的结果，与用户附件是两条路。
- **落点**：`core-agent/src/tools.rs`（`IMAGE_SENTINEL` + `read()` 分支）、`core-agent/src/main.rs`（`image_media_type` 改 `pub(crate)`、`tool_result_block` 哨兵分支、摘要渲染兼容块数组）。单测 `read_image_sentinel_becomes_image_block`；`cargo test --bins` **141 passed / 0 failed / 2 ignored**。契约见 `ai-spec.md` §3.5「图片附件」与工具表 `Read` 行。

**设置「AI」分类删除 Skill Store 与工具（MCP）两个分块（2026-10-05，闭环记录）** —— 用户口径「直接删除 AI 分类里面的 skill store 和工具的全部」；选「两个都删」。

- **理由（用户补充）**：这两个入口是给进阶用户的，对 AI 零基础用户只会造成「这个该怎么用」的困扰。skill / 工具应改为**由 AI 在运行时按需查找或生成，并在动手前提醒用户** —— 入口不该暴露在设置里。
- **删除项（`app/src/plugins/builtin/settings.ts`）**：`buildSkillsSection` / `buildToolsSection` 及其全部辅助（`SKILL_SITES` / `loadSkillSites` / `saveSkillSites` / `skillEntryHtml` / `SKILL_TEMPLATE` / `installedSkillRowHtml`）、`buildAIPane` 里的两处分块标题与插值；监听侧删掉「打开工具编辑器 / 打开 mcp.json / 从 URL 安装工具 / 社区工具安装 / Skill Store 全部交互」。`open`（`@tauri-apps/plugin-shell`）随之不再被本文件使用 ⇒ 连 import 一起删。
- **保留项（刻意）**：**后端宿主命令一条未删**（`list_tool_files` / `download_tool_from_url` / `save_tool_file` / `list_installed_skills` / `install_skill_from_url` / `read_skill_file` / `save_skill_file` / `import_skill_content` / `delete_skill` / `open_mcp_config` 等）—— 它们将来要被「AI 运行时按需查找/生成」那条路复用；本轮只摘掉设置里的入口。`tool-editor` 插件本身与 `__lunac_execute_tool_editor` 桥**也保留**（插件仍注册在 registry，只是设置里那个按钮没了）。
- **验证**：`npx tsc --noEmit` 通过、`npx vite build` 成功。契约同步 `ai-spec.md` 规则 50「AI 分类 = 两个分块」。

**设置界面「对 AI 零基础做减法」大重构（2026-10-05，闭环记录）** —— 用户一次提出六项精简，逐项落地。

- **① 常规：合并更新开关**：「启动时自动检查更新」+「自动下载并安装」→ 一个「自动更新」开关（`#settings-update-auto`）。语义 = 开则启动静默检查 + 发现新版自动下载安装（会拉起安装器并重启）。**默认开**（用户选「默认开：检查+自动安装」）。实现：`update.json` 仍是 `checkOnStartup` / `autoInstall` 两字段（**不动文件格式**），面板读写时两字段同真同假；`updater.rs::UpdateConfig::default()` 改成两者皆真，单测改名 `default_config_enables_auto_update`。
- **② AI：删搜索服务商 / 搜索 API 密钥**：从「AI 模型」分块移除两行。保存时 `search_provider` / `search_key` **原样透传 `get_ai_config` 里的现值**（界面没了入口，但不去删用户已配的 key）。独立「搜索」分类（搜索引擎选择）**不动**。
- **③ AI：删安全档位 → 移到「更多设置」菜单**：回答用户「是否与自动放行重叠」——**部分重叠**：安全档位（safe/project/full，文件边界，后端「允不允许」）与输入栏「运行方式」（手动/白名单/自动，前端「问不问」）本是正交的两件事，但运行方式的档位会映射安全档位（auto→full）。两个入口确实冗余，故**保留能力、只留一个入口**（用户选「保留，移到更多设置菜单」）。新控件 = `#chat-profile-btn`（点一下循环 只读→项目→完全），`index.html` 的 `#chat-more-menu` 里、运行方式那行下面；`main.ts` 的 `renderProfileUI()` 刷文字 / tooltip，切到 safe 时把运行方式收回 manual。
- **④ AI：删「回合自动折叠」选项**：**固定默认开**（`localStorage["lunac-agent-autofold"]` 读不到即为开；已设成 `0` 的旧值仍尊重，不再有 UI 入口）。
- **⑤ AI：删「模型支持图片输入」选项**：**固定 `vision = true`**（用户口径「让模型自己判断，不做应用侧设置」）。保存时恒回传 `vision: true`。
- **⑥ AI：出图模型 / 出图端点并入供应商**：删掉两个独立字段。改为**扩展 `PROVIDER_PRESETS`**（新增可选 `image_model` / `image_url`）—— 目前只有 `qwen` 带（`qwen-image-3.0-pro` + DashScope 多点编辑端点），选中它即自动启用 `ImageGen`；其它供应商为空 = 不出图。保存时由所选供应商预设推导这两个值。后端字段 / 环境变量 / 条件注册逻辑**一条未改**。

- **落点**：`app/src/plugins/builtin/settings.ts`（`buildGeneralPane` / `buildAIModelSection` / `buildAIPane` / `saveAIConfig` / loader / 事件绑定 / `PROVIDER_PRESETS`）、`app/src/index.html`（更多菜单加一行）、`app/src/main.ts`（`renderProfileUI` + 点击循环）、`app/src/styles.css`（`#chat-profile-btn`）、`app/src/i18n.ts`（`settings.update_auto` / `_hint`）、`app/src-tauri/src/updater.rs`（默认值 + 单测）。
- **验证**：`npx tsc --noEmit` 通过、`npx vite build` 成功、`cargo check --tests`（src-tauri）通过。
- **顺带回答用户「AI 零基础还该改哪些」**：见本轮回话（搜索/模型/出图等已收；下一步可考虑把「工具黑名单」「人格」「热键录制」也收进「更多设置」，设置面板只留 供应商+模型+API Key+外观+语言+更新）。

**详细搜索三处修复 + 简洁搜索选中错位（2026-10-05，闭环记录）** —— 用户报「选项菜单点了没反应 / 方向键+回车选到结果后的内容 / 结果区那个管理员按钮是多余的」。

- **① 简洁搜索「回车打开的是高亮行之后那条」**：根因 = `renderMixedResults()` 每次都把**第一行**画成 `.selected`，但**从不重置逻辑下标 `selectedIndex`**。用户「输入文本 → 看结果 → 删掉文本 → 按上下键 → 回车」时，高亮在第一行、而 `selectedIndex` 还停在上一次结果的下标 ⇒ `currentEntries[selectedIndex]` 是另一条（看起来正是「结果之后的内容」）。修复：`renderMixedResults()` 开头 `selectedIndex = 0`，与那条 `idx === 0 ? " selected"` 强制同步。
- **② 详细搜索「选项」菜单点了没反应**：根因 = `showContextMenu()` 之后，这次 `click` **冒泡到 `document`** 上那条「点任意处即关菜单」的监听器（`hideContextMenu`），菜单在同一帧被自己关掉。修复：`optsBtn` 的 click 处理器里 `ev.stopPropagation()`（右键菜单走 `contextmenu` 事件，本来就不受影响）。
- **③ 删除结果区重复的「以管理员身份运行」盾牌**：`buildDetailItem()` 里那个 `.result-item-elevate` 按钮（连同其 bind 分支与 styles.css 里的规则）按用户要求删除。提权能力**不丢**：仍走「选项」菜单里的提权项 + `Shift+Enter`（判据同源 `detailElevatable`）。
- **落点**：`app/src/main.ts`（`renderMixedResults` / `buildDetailItem` / 预览「选项」按钮）、`app/src/styles.css`（删 `.result-item-elevate` 规则）。
- **验证**：`npx tsc --noEmit` 通过、`npx vite build` 成功。

**自定义插件三处修复：复用内置监听（`reuse`）+ 写盘自动重扫 + 简洁搜索选中错位（2026-10-05，闭环记录）** —— 用户报「AI 按规范生成的插件，搜索结果要**点设置里的重新扫描**才能注册」「换出来的插件**全部按钮丢失功能、只有后端有反应**」，并裁定**「允许复用内置插件监听」**（否决了「给插件做 SDK」那条路）。

- **① 插件按钮失效（根因）**：磁盘插件的结果 HTML 是先 `innerHTML` 插入、再调它**自己**导出的 `attach(root)`；而内置插件面板的按钮靠的是**编译进主 bundle 的那套监听**（`attachXxxListeners`）。磁盘插件默认**拿不到**这套监听（「卸载 = 完全不存在」这条硬约束不许主 bundle 留对某个插件的 import / `id === "xxx"` 分支）⇒ 照抄内置面板 HTML 的自定义插件，按钮全是死的。**解法 = 清单声明 `reuse`**：磁盘插件在清单里写 `reuse: ["settings", …]`，宿主在它自己的 `attach(root)` 之外**追加**挂那套内置监听（`plugins/attach.ts::attachBaseListeners`，上限 `MAX_REUSE = 4`、id 要合法）。
- **② 写盘自动重扫**：AI 运行时直接往 `Modules\<id>\` 写插件 ⇒ registry 里没有它、搜索搜不到（用户被迫去设置里点「重新扫描」）。改成「本回合嗅到写过 `Modules\`（`notePluginDirWrite`）⇒ 回合收尾 `emit("lunac-plugins-changed")`」；**必须广播给所有窗口** —— 搜索在主窗、AI 对话在独立窗，两个 WebView 各有一份 registry，只在本窗重扫会漏掉另一窗。
- **③ 简洁搜索选中错位**：见上一条「详细搜索三处修复」的 ①（`renderMixedResults()` 未重置 `selectedIndex`）。
- **落点**：`app/src/plugins/attach.ts`（`attachBaseListeners`）、`app/src/main.ts`（`notePluginDirWrite` + 回合收尾广播）、`app/src-tauri`（清单解析 `reuse`）、`agent-templates/modules/README.md`（`reuse` 契约）。
- **纪律**：见 [code-rules.md](./code-rules.md) 预检 #65；**改插件源码 ≠ 生效**，必须按序跑 `scripts\build-plugins.ps1`（它**先**铺 dev 的 `target\debug\Modules\<id>\`）。

**音乐插件「频段柱左侧加图例」（2026-10-06，闭环记录）** —— 用户报调音页那三个输入框（频率 / 增益 / 质量）没有文字说明，并裁定提示落点 = **整个区域左侧一列图例**（否决「每柱内」/「框上方」两种方案）。

- **形态**：`music.ts` 把 `#music-tuning-filters` 包进 `music-tune-filterwrap`，左侧新增一列 `music-tune-legend`（频率 / 空 / 增益 / 质量 四格，与右侧柱子的三行**逐行对齐**）。
- **对齐靠 CSS 变量**：`--tf-freq-h` / `--tf-slider-h` / `--tf-gainnum-h` / `--tf-q-h`（`styles.css`），柱子三行与图例四格都钉这几个高度 ⇒ 任何一边加行都不会错位。
- **同批纠正保存时机**：那三个输入框只监听 `change`（失焦 / 回车）才落盘，输入过程中不校验、不保存。
- **i18n**：`music.tuning_q` 由 `Q` 改成「质量」（zh-CN）/「質量」（zh-TW）。
- **「改了没反应」的排查口径**：调音逻辑活在 `Modules\music\`，改 `app/src/plugins/builtin/music.ts` **必须跑 `scripts\build-plugins.ps1`** 才进得去（预检 #65 ①）。

**「回退到此处」三症状（2026-10-06，用户实测报，闭环记录）** —— 用户报「点回退后：① 第一段对话的思考过程丢失；② 这段对话并没有回退；③ 没有提醒是否回退」。**三个症状同一个根因，已全部修掉。**

- **根因**：`rollbackChat()` 回退后调的是 `renderChatLogHtml()`，而它**只按 `chatHistory` 画纯文本气泡**，完全不碰 `sessionSteps`（过程快照）—— 于是回退那一瞬，思考 / 工具卡 / 命令卡**整片被抹掉**。三个症状由此推出：
  - **①「思考过程丢失」**不是「第一段」丢，是**所有过程块**都被抹掉（该重绘器本是给历史视图用的）。
  - **②「并没有回退」**：用户点的是**最后一条**「回退到此处」⇒ 其后没有消息可丢 ⇒ 文本一字未变（看着像没回退），**但过程已被清掉** ⇒ 观感是「变动很大却没回退」。两个矛盾感受同时出现，正是这段代码的行为。
  - **③「没有提醒」**：`onRollbackClick` 的两段式确认**只在真有文件要还原时**才拦一下，对话回退本身从不确认。
- **修法（三处，`app/src/main.ts`）**：
  - **第 1.6 步**：`sessionSteps = sessionSteps.filter(g => g.turn < kept)` —— 按与「待办时间线」**同一把尺子**（`turn < kept`）裁掉被丢弃那几轮。**必须在 `saveCurrentSession()` 之前**，否则盘上仍留着旧过程、恢复会话时又冒出来。
  - **第 5.5 步**：重绘后补一次 `renderHistoryProcess(sessionSteps)` 把保留的过程块**插回 DOM**。为让回退路径也能调它，`renderHistoryProcess` 的入参从 `session: ChatSession` 改成 `steps: SessionProcess[]`（历史回顾那边同步改为传 `sessionSteps`，语义等价）。
  - **`onRollbackClick`**：去掉「没文件就一步到位」那条捷径 ⇒ **任何**回退都要两段式确认；无文件时的文案是新的 `chat.rollback_armed`（5 语言）「其后的对话将被移除（不可撤销）—— 再点一次确认」。
- **理由（为什么确认要扩到「无文件」）**：回退会**立刻把裁剪后的会话写回 `chat.db`、且不可撤销**（`rollbackChat` 文件头写明）—— 一个会丢数据的动作不该因为「恰好没动过文件」就静默执行。
- **顺带修掉的隐患**：`sessionSteps` 此前**不随回退裁剪**（代码注释自己标着「既有行为」）⇒ 回退后「改动过的文件」列表与对话对不上。现在它跟着裁，`rebuildChangedFilesFromSteps` 的结果才真的等于「保留下来的回合里改动过的文件」。
- **验证**：`npx tsc --noEmit` exit 0、`npx vite build` 通过。**待真机验收**：回退后过程块以「过程 · N 步」折叠行出现、点开可见全文；点回退第一次变红并给状态栏提示、4 秒内再点才执行。

**上下文压缩改为「对话流可见」（2026-10-06，闭环记录）** —— 用户要求「如果压缩了上下文，需要在思考过程中显示」。

- **现状（改之前）**：`context_compacted` 只写状态行（`statusText`）—— 一闪而过、还会被后续状态覆盖，用户根本不知道这轮模型为什么忘了前面的事。
- **改法**：除状态行外，再往**对话流**插一条可见记录（`app/src/main.ts` 的 `context_compacted` 分支 + i18n `agent.compacted_note`，5 语言）。
- **用 text 块而不是 thinking**：它是**过程告知**、不是模型的思考内容；更实际的理由是 text 块**不会被「回合自动折叠」收起**，能完整留在记录里可回看。若将来要改成 thinking，代价就是被折叠成一行。
- **数据源不变**：事件仍只带 `elided` / `dropped` 两个数（`省 N 字` 只在 agent 日志里，不进事件）。

> ⚠️ **2026-10-06 当日修正（用户实测发现，第一版写错了载体）**：上面那条「对话流可见」最初用
> `agentAppend("text", …)` 插文本 —— 而 [main.ts](../../app/src/main.ts) 的 `agentAppend` 对 `text`
> 走的是 `v.textAll += s`，收尾时 `const finalText = v.textAll.trim()` 会被
> `chatHistory.push({ role: "assistant", content: finalText })` **存进会话库**（下次还回灌给模型）。
> 用户报「写 chrome skill 那次的回复开头跟任务完全没关系」，查 `ModuleData\history\chat.db` 坐实：
> 那条 assistant 的内容开头就是「上下文已压缩：省略 12 个工具结果，丢弃 22 条早期消息 …」。
> **改法**：照 `renderHookNote` 的形态，往 `agentView.flow` 插一个**独立节点**
> （`<div class="sys-note sys-note-line">`，新增 `.sys-note-line` 只补次要文字色）—— 它不进 `textAll`、
> 不污染历史。**教训**：往对话流写「系统旁注」前先确认它走不走 `textAll` —— 走 `agentAppend("text")`
> 的都会变成「助手说过的话」。同一条路上还有 `agent.error_inline`（`result.subtype !== "success"` 时），
> 同样是「系统文案混进助手正文」，**本轮未动**（错误进历史尚可接受，但要知道它会进）。
> 已被污染的那条历史留在 `chat.db` 里（用户数据，不擅自改）——重开新会话即可绕过。

**「AI 自建技能没弹审批卡」的排查结论（2026-10-06）** —— 用户报「创建 skill 的对话并没有弹出审核卡」。**结论：不是缺陷**，是两个档位叠加：

- **① 那次是 full 安全档**：宿主 spawn 参数含 `--dangerously-skip-permissions`（日志实证）⇒ `tools::Ctx.locked = LUNAC_WORKSPACE_LOCKED=="1" && !skip_permissions` = **false** ⇒ 工作区锁关闭 ⇒ 写 `<exe 根>\skills\` 本来就不受拦，第 5 步那道「例外」压根没被触发。
- **② 运行方式是「自动」**：`classifyRequest` 对非命令类工具（`Write` / `Edit`）直接返回 `auto: true` ⇒ 静默放行、不发卡（与 agent-ui-spec §4.2「自动档不该弹卡」一致）。
- **正确的验收姿势**：配置工作区（才有 `LUNAC_WORKSPACE_LOCKED=1`）+ 安全档 = **项目**（safe 会先被 `write_blocked` 拒）+ 运行方式 = **手动** ⇒ 写 skills 目录才会弹卡（改之前是**硬拒**）。
- **用户裁决（2026-10-06）**：**不**为 skills 目录加「强制弹卡」（不无视运行方式）—— 弹不弹仍由运行方式决定，与其他写入一视同仁。

**系统提示词：回复语言跟随用户提问（2026-10-06，闭环记录）** —— 用户要求「用户用中文提问，最后的输出除必要的英文外全是中文；用别的语言就用相应语言」。

- **现状**：内置人格段里原本只有一句很弱的 `Always reply in the user's language.`（`PERSONA_AND_STYLE`），模型并不总遵守 —— 中文提问的回答里仍会夹英文句子。
- **改法**：把那条换成明确的硬规则（`core-agent/src/main.rs` 的 `PERSONA_AND_STYLE`）：**回答用用户提问的语言**；中文提问 ⇒ 整段回复中文（代码 / 命令 / 路径 / API 与工具名保持原样，但正文不许夹第二语言）；用户换语言就跟着换。
- **为什么放人格段而不是用户消息**：人格段是**固定前缀**的一部分（§11 规则 18），进程内逐字节不变 ⇒ 永远命中缓存；放进用户消息则是每问重发、必未命中。
- **守门**：`system_prompt_is_stable_and_carries_persona` 加一条断言钉住这句措辞（原先只钉段落标题）—— 防止它被删掉或又被写弱。
- ⚠️ **与用户自定义人格的关系**：用户段（`config\persona.md`）追加在内置段**之后**，模型以其为准 —— 用户若写了相反要求会覆盖这条（既有设计，不改）。

**「更多设置 → 人格」文案精简（2026-10-06，闭环记录）** —— 用户要求：标题由「人格 / 自定义提示词」改成「**人格**」；标题下方那句说明（接在内置人格之后 / 最多 8000 字符 / 不进子代理 / 保存后需重启 AI）与文本框的 placeholder 示例（「叫我老王…一律用 Rust」）**全部删除**。

- **改动**：`index.html` 删 `#chat-persona-hint`；`main.ts` 删 `chatPersonaHint` 声明与赋值、删 `chatPersonaText.placeholder`；`i18n.ts` 删 `settings.persona_hint` / `settings.persona_placeholder` 两个 key、`settings.persona_title` 简化为「人格」；`styles.css` 删 `.chat-persona-hint` 规则。
- **刻意保留**：`maxLength`（由 `get_persona` 的 `maxChars` 驱动，是**功能**不是文案）、保存 / 恢复内置 / 立即重启 AI 三个按钮、以及保存后的反馈（`persona_saved` + `persona_takes_effect`）—— 「改完要重启 AI 才生效」这句信息仍在**操作反馈**里如实给出，只是不再常驻一行说明。
- **依据**：`code-rules.md` §13.5「面向用户的提示文本必须精简」（① 默认不说过程、② 一个动作最多一条提示）。

**「更多设置」两项改名：运行方式 → 命令审批方式、安全档位 → 权限（2026-10-06，闭环记录）** —— 用户要求改名，并把配套的提示文案一并对齐。

- **为什么改**：原名与**实际职责**不符。「运行方式」管的是**所有工具调用**要不要弹审批卡（命令、写入、网络读取都在内），不只命令；「安全档位」管的是**能不能写 + 是否受工作区限制**，「档位」这种抽象说法说不清它到底干什么。
- **改法（只改 i18n 的「值」，key 与代码零改动）**：`agent.run_mode` → 「命令审批方式」；`settings.security_profile` → 「权限」。
- **配套提示同步**：
  - `agent.run_mode_{manual,allowlist,auto}_hint`：措辞由「写操作 / 命令」改成「工具调用 / 操作」（原名与实际覆盖面不符 —— 手动档连 `WebSearch` / `AskUserQuestion` 也会弹卡）。
  - `agent.run_mode_auto_warning`：「「完全」安全档位会忽略工作区限制」→「「完全」权限会忽略工作区限制」；「自动运行中」→「自动审批中」。
  - `agent.run_mode_auto_confirm` / `_confirm_ok`：「切换到自动运行」→「切换到自动审批」。
  - `settings.security_profile_hint`：**顺手改准** —— 原文「档位决定写操作是否被自动放行」是错的（决定「问不问」的是命令审批方式）；改成「决定能否写文件、是否受工作区限制；切换会重启 Agent」。
- **「项目」与「完全」的唯一差别**（用户问）：不是「能不能写」（两者都能），而是**是否受工作区限制** —— `full` 带 `--dangerously-skip-permissions` ⇒ `locked = false` ⇒ 工作区锁失效，本机任意路径可读写。
- **注释同步**：`index.html` / `main.ts` 里那两处说明「两者正交」的注释一起改了措辞（注明 UI 名于 2026-10-06 变更）。

**权限三档改名：项目 → 工作区、完全 → 系统（2026-10-06）** —— 紧接着上面那次改名，用户看了「项目 / 完全」两档的差别（**唯一差别是受不受工作区限制**）后，要求档位名也换掉：`settings.security_profile_project` → 「工作区」、`settings.security_profile_full` → 「系统」（5 语言）。`agent.run_mode_auto_warning` 里那句「「完全」权限会忽略工作区限制」同步改成「系统」。

- ⚠️ **行为刻意不变**：用户明确「**不要**改成『完全只跳过审批、仍受工作区约束』」—— 即「系统」档**仍然会关掉工作区锁**（`locked = LUNAC_WORKSPACE_LOCKED=="1" && !skip_permissions`），这是有意保留的语义，别顺手「修」掉。

**代理插件前端重构：多页 + 自制控件 + 去解释（2026-10-06，闭环记录）** —— 用户报「代理插件前端没遵守前端插件规范：端口输入框的箭头是 WebView2 默认形态、订阅/模式的下拉框不是规范形式、一大堆解释文案多余，整体像一堆简陋文本框」，并要求**参照 Clash / v2rayN 严格重做**（确认后：做**完整多页**、策略组用**分段按钮**）。

- **多页（4 页）**：节点 / 订阅 / 日志 / 设置 —— 左侧 88px 竖排导航 + 右侧内容（`.proxy-body` / `.proxy-nav` / `.proxy-pages` / `.proxy-page`）。切页是**纯前端 `display` 切换**、不重取数据；进「日志」页顺手 `doMihomoLog()`（否则用户看到的是空页）。
  - ⚠️ 本插件是 `layout.takeover` 型 ⇒ `document` 上可能同时存在别的面板，**选择器一律从 `.proxy-panel` 往下找**，不用全局 `querySelectorAll`（否则会误伤别的面板）。
- **消灭原生 `<select>`**：模式（rule/global/direct）与**策略组切换**都改成**自制分段控件**（`.proxy-seg` / `.proxy-seg-btn`，与输入栏「思考开关」同一形态，依据 agent-ui-spec §5.3）。分组那版允许换行（`.proxy-seg-wrap`）。交互相应从 `change` 改成 `click` 委托（`#mihomo-nodes` 一个委托吃「切组 + 测速」两种动作）。
- **`<input type="number">` 去掉原生 spinner**：端口输入框右侧那对上下小箭头是 WebView2 默认形态、与本文件顶部的**全局细滚动条**视觉不搭（用户点名）⇒ `appearance: textfield` + 隐藏 `::-webkit-{outer,inner}-spin-button`。**只去控件、保留键盘上下键**。
- **删掉 3 处常驻解释**（`.proxy-hint` × 3，依据 `code-rules.md` §13.5「提示文本必须精简」）：信息挪进对应控件的 `title`（`#mihomo-state` / `.proxy-link` / 「手动代理」区块标题），界面不再有大段说明；同时删掉 `.proxy-hint` / `.proxy-select` 两条 CSS 死规则。
- **一行未改的部分**：所有 `doXxx` 动作、宿主命令、`refresh()` 取数逻辑 —— 只动渲染与事件绑定。
- **发布**：proxy 是**磁盘（拓展）插件** ⇒ `scripts\build-plugins.ps1 -Plugin proxy`（已铺 dev 的 `Modules\proxy`，zip 5.4 KB）。⚠️ **发布市场前要把 `proxy` 版本号升上去**（当前仍 `0.9.1`）—— 不升版本市场会判「无新版」（预检 #65 ①）。
- **验证**：`npx tsc --noEmit` 通过、`npx vite build` 成功、`build-plugins.ps1 -Plugin proxy` 成功。

---

## 1. Agent 能力缺口

### P3

**A13. 按需重估的剩余项**（不做只因为优先级，不是因为没价值）

| 项 | 说明 / 何时重估 |
|---|---|
| 斜杠命令 | 旧 CLI 有 75+，多数是 CLI 会话内操作（`/theme` `/vim` `/statusline` `/login` …），桌面端另有 UI。**只挑与能力相关的子集**（`/compact`、`/rewind`、memory 类）评估，不整体照搬 |
| 记忆目录（CLAUDE.md 体系） | **项目级那一层已于 2026-10-01 落地**（工作目录下的 `AGENTS.md` ⇒ `env_section`，见 `ai-spec.md` §20.1 与预检 #54）。**仍未做**：多层 / 向上递归的记忆目录（`CLAUDE.md` 系列）、子目录级指令；要做也得先想清「哪一级算数」，别直接照搬 |
| `NotebookEdit` | Jupyter 场景，用户群不大 |
| `EnterWorktree` / `ExitWorktree` | 需要 git worktree 工作流 |
| `CronCreate` / `CronDelete` / `CronList` | 后台定时任务；Lunac 已有 Windows 计划任务做自启，能力不重叠 |
| `WebBrowser`（浏览器控制） / `LSP` | 需要常驻上下文（浏览器会话 / 语言服务器） |
| `SendUserFile` / `PushNotification` / `Brief` | 面向远端 / 移动端的推送通道 |
| 输出样式 / statusline | CLI 的终端样式体系，已被 WebView 取代 |
| Ink TUI / Vim / 语音 / buddy / chrome / 桥接远程控制 / OAuth 账号体系 / 自动更新 / 代理证书 mTLS / IDE 集成 | 绑定的分别是旧 CLI 的终端形态、当前交互形态没有的入口、企业网关场景；**由 NSIS 安装包与 `vscode-extension/` 各自负责的部分已完成**。逐项都要「先确认它在新宿主下还有意义」再重估 |
| **MCP 其余能力**（`ai-spec.md` §20.1 的「仍未做的」） | **Streamable HTTP 已于 2026-10-01 落地**（`config\mcp.json` + `McpSet` 多服务器，见预检 #54）；**同日 OAuth 2.1 也落地**（`"auth": "oauth"`：动态客户端注册 + Authorization Code + PKCE，令牌落 `config\mcp-tokens.json`，见 `core-agent/src/mcp_oauth.rs` 与 §20.1）。**prompts / roots / elicitation 已于 2026-10-04 落地**（协议升 `2025-06-18`；prompts ⇒ `ListMcpPromptsTool` / `GetMcpPromptTool`，roots / elicitation ⇒ 双向请求 + 前端表单卡，见 §3.5「MCP prompts 读侧」/「MCP elicitation / roots」与 §11 规则 55 / 84）。**仍未做**：`.mcp.json`（agent 侧自读的配置形态）；已废弃的 HTTP+SSE 双端点传输**不做**。**别因为 §20.1 标题写着「已落地」就当 MCP 全做完了** |

#### A13-扩展：`.mcp.json`（项目级 MCP，全兼容 http + stdio）+ 统一审批 UI（2026-10-05 用户定；**第 1–6 步 2026-10-06 已全部落地，并已验收**）

用户口径（逐字）：「全兼容（http + stdio）」+「审批UI也一起装入」+「需要装入 skill 或自建 skill 时也需要用这个审批」+「所有的审批卡 / 计划卡如果展示给用户的话 在我测试里会出现一大段文档 让 ai 总结到最简并保留主要内容再输送给用户」。

**已完成：第 1–4 步（2026-10-06）** —— 契约见 [ai-spec.md](./ai-spec.md) §20.1「项目级 MCP」。

1. ✅ **stdio 传输泛化**（`core-agent/src/mcp.rs`）：stdin 连接内核抽成 `Bridge::connect_stdio_cmd`（宿主桥与普通服务器共用）；新增 `connect_stdio_server`（`command` + `args` + `env`，`lunac: false`）与 `StdioServerConfig`。
2. ✅ **读 `<cwd>\.mcp.json`**（`load_project_config` / `parse_project_config`）：`mcpServers` **object** 形态；有 `command` ⇒ stdio、否则有 `url` ⇒ Streamable HTTP；`type: "sse"` 跳过并 warn；URL 仍只放行 https / 本机 http；坏条目只丢自己。纯函数 + 3 条单测（二分 / sse 与未知 type 跳过 / 坏条目只丢自己）。
3. ✅ **信任门 + 推迟连接**：信任记录 `<exe 根>\config\mcp-trusted.json`（`{项目路径 → [指纹]}`，指纹 = `sha256(传输 + 名字 + 各字段)` 前 8 字节，改任一项即失效）；启动只**读**配置，**首次查询前**（stdin 已就绪）才过信任门 + 连 + append 工具池（`project_trust_done` 只做一次）。`McpSet::add_project` / `TrustStore`。
4. ✅ **前端信任卡**（`app/src/main.ts` 的 `showMcpTrustCard`）：`control_request` 的 `subtype = mcp_trust`，三答案＝仅本次 / 始终信任（`updatedInput.remember`）/ 拒绝；卡片如实列出每台的名字、传输与目标命令；`control_cancel_request` 时一并收掉。i18n 5 条（`agent.mcp_trust_*`）。

**第 6 步的落地形态（2026-10-06，改掉了原计划）** —— 见 [agent-ui-spec.md](./agent-ui-spec.md) §9「长计划默认折叠」：

- **先调查「一大段文档」到底在哪**：普通审批卡（`Write` / `Edit`）的入参预览本来就截到 **180 字符**（`renderCmdGroupBody`）—— 不是它；真凶是**计划卡**：`renderPlanCard` 把整份 `plan` 用 `textContent` **原样铺开**，而计划动辄几十上百行。
- **⚠️ 没有按原文用「模型摘要」，改成默认折叠**（`PLAN_PREVIEW_LINES = 12` + 「展开全文 / 收起」）。理由是安全的：计划卡是用户裁决「放行写类工具、让模型开始动手」的**唯一依据**，摘要可能恰好丢掉「会删除某个文件」这类关键项 —— 那不是「保留主要内容」，而是**把风险藏在摘要之后**。折叠不丢任何内容、零成本、可逆。
- 顺带修正：原 meta 行按 `plan.trim()` 数行，末尾空行会让「共 N 行」虚高 ⇒ 改成**去尾空行后**数。
- **命令卡刻意不折叠**：完整命令是安全要求（用户要看清到底执行什么）。
- 新增 i18n：`agent.plan_expand` / `agent.plan_collapse`（5 语言）。

**第 5 步的落地形态（2026-10-06，用户裁定「给 skills 目录开审批例外」）** —— 见 [ai-spec.md](./ai-spec.md) §20.1「AI 在运行时自建技能」：

- **先核实的现状**：设置里的 skill / tools 入口已于 2026-10-05 按用户要求删除 ⇒ `install_skill_from_url` / `import_skill_content` / `save_skill_file` / `read_skill_file` **前端零调用（死代码）**，按 q9 既定结论**保留后端命令、不删、也不恢复入口**。
- **所以第 5 步不是「给设置面板装审批」，而是「让 AI 运行时自建技能、动手前提醒」**：`tools::guard` 对 `<exe 根>\skills\**` 开一道**受审批**的例外（`inside_skills_dir` + 纯判据 `path_is_within`）。
  - 未锁工作区时本来就通；**锁着时原先会硬拒**（AI 连问都问不到）—— 那道硬拒与用户诉求正好相反，这才是真正的缺口。
  - **只放行、不代替审批**：`Write` / `Edit` 在 `needs_approval()` 里恒真 ⇒ 照常弹审批卡，用户点头才落盘。
  - 触发路径 = AI 自己 `WebFetch` 取内容（或直接生成）→ `Write` → 审批卡。**不加新工具、工具表一个字节不变**。
  - **`config\` / `Modules\` 不在例外里**；只读（`plan`）档不受影响（`write_blocked()` 先于 `guard`）。
  - **生效时机**：技能清单启动时读进固定前缀 ⇒ **新建后要重启 agent 才加载**（与插件「立即生效」不同）。
- **单测**：`skills_dir_is_the_only_workspace_lock_exception`（目录内放行 / `skills-evil` 不算 / `..` 绕出不算 / 空 root 不开例外）。

> **第 3 步落地时改掉的一处 backlog 原文**：原文写「把「连项目 MCP + 信任询问」推迟到第一次查询前，**或**把 `connect` 拆成两段」。实测下来**只能选「推迟到首次查询前」**，而且要连**工具表的 append** 一起推迟 —— 工具表是固定前缀（§11 规则 18），必须在第一问发出前定死。

**信任门诊断（2026-10-06 实测，验收踩坑记录）** —— 用户按验收脚本「改 `.mcp.json` 的 url 加个 `/x` → 重启 → 提问 → 应重弹卡」跑下来**没弹卡**，判为缺陷。

- **根因：不是缺陷，是 agent 没重启**（`.mcp.json` **只在 agent 进程启动时读一次**，信任门每进程只过一次 —— 与 `config\mcp.json` 同一约束）。实证：`.mcp.json` 的 mtime `17:46:41` **晚于**最后一次 `agent spawn` `17:43:13`；期间那条 `restarting agent`（17:43:08–13）是用户切安全档位触发的，**没有覆盖改文件这个时刻**。判定口径：**最后一条 `=== agent start pid=…` 必须晚于 `.mcp.json` 的 mtime**。
- **两条自查日志（本轮补）**：此前「配置里有 N 台」走 `eprintln`、「已在信任记录里」走 `log::info`，**两条信息分居两个日志文件** ⇒ 统一走 `log::info` 并**带指纹**：① 启动时 `项目 MCP 配置：N 台待过信任门 —— <名>[<传输>] <目标> fp=<指纹>`；② 首次查询时 `项目 MCP：N 台已在信任记录里，直接接通`。**既没 ②、也没有「信任询问」** ⇔ 配置压根没被读到 / 读的是旧配置。

**配置改动检测 + 自动重启（2026-10-06，用户定；两版，已落地并验收）** —— 上一条诊断暴露了一个真实的坑：改了 MCP 配置，用户得**自己记得**重启 agent，否则静默失效。用户为此要求补一条自动链路。

**第一版（回合结束后重启）**：

- **形态**：`mcp::ConfigWatch`（启动拍 mtime 快照，**每轮收到 user 消息时**惰性比对）→ 变了上报 `system/mcp_config_changed`（只报一次）→ 前端记下 → **本回合 `result` 收尾**才调 `__lunac_reload_agent()`。挂载点与「回合收尾自动重扫插件」（`pluginDirTouched`）一致。
- **为什么检测在 agent、动作在前端**：agent 是宿主的**子进程**、重启不了自己；宿主又只透传 stdin/stdout、**不解析 stream-json**、不知道回合何时结束。两者各只能做一半。
- **为什么不在「文件一变」就重启**：`kill_and_cleanup()` 无条件立即杀、**没有空闲判定**（会打断正在跑的回合）；编辑器保存常非原子（mtime 毫秒级抖动 ⇒ 无防抖会反复重启）；重启后 `.mcp.json` 指纹变、又弹信任卡，静默触发最像 bug。
- **回合中改动**：工具表是第一问前定死的（就地重建 = 拆桥 + 重弹信任卡，代价远大于「答完再重启」）⇒ 前端**插一行如实交代**（`agent.mcp_config_reload`，5 语言）。**不静默重启是硬要求**。

**第二版（2026-10-06 实测后补，用户报「回合外更改的话会产生 dev 日志里的情况」）**：

- **第一版的缺口**：检测只挂在「收到 user 消息」⇒ **回合外**（agent 空闲时）改配置，agent 根本不知道，要等用户**下次提问**才发现，于是先用旧配置**白答一整轮**再重启。日志实证：`18:06:21` 回合#1 结束 → `18:06:36` 才检测到 → `18:06:58` 才重启，中间 22 秒白答了一整轮（问#2 跑了 13 次请求）。
- **修法**：主循环 `for msg in rx` → `loop { rx.recv_timeout(MCP_WATCH_TICK = 1s) }`，**空闲轮**顺手看一眼配置 ⇒ 回合外改动 1 秒内被发现。事件带 `idle: true/false` 供前端分流：**空闲 ⇒ 立即重启**（此刻没有回合在跑，零代价）；**回合中 ⇒ 等 `result` 收尾**。两条路共用前端 `restartForMcpConfig()`。
- **为什么 `idle` 判定可信**：`run_query` **同步阻塞**主循环，期间不会超时醒来 ⇒ 空闲轮只可能在真空闲时触发，**不依赖**前端的 `isStreaming` / `agentState` 推断。
- **「只检测一次」（用户明确要求）**：`ConfigWatch` 内部 `notified`（单测守）+ 外层 `Option<ConfigWatch>` 报完置 `None` —— 报过一次后**连函数都不再调、不再碰文件系统**。新进程重拍快照，只有**又**改了才会再报。
- **验证**：单测 `config_watch_reports_once_and_covers_new_files`（用「文件从无到有」当变化源，不依赖 mtime 时钟精度）；`cargo test --bins` **146 passed**、`tsc` / `vite build` 通过、`agent.exe` 重编译 3.1 MB。
- **验收（2026-10-06，用户实测通过）**：回合外（agent 空闲）改 MCP 配置 ⇒ **无需提问即自动重启**、新配置直接生效，不再白答一轮。

---

## 2. 界面层待办（`agent-ui-spec.md` 阶段 3）

> `agent-ui-spec.md` §10 的阶段 1 / 阶段 2 **已全部落地**，清单已撤下；阶段 3 的三项挪到此处统一排序。**界面规范本身（§0–§9 与 §8 复核清单）继续有效，不回退。**

| # | 项 | 说明 |
|---|---|---|
| **U1** | 代码变更 diff 卡 | **✅ 已落地（2026-10-06）：事后审阅 + 用快照回滚**（用户当日裁定的语义 —— 文件已被 agent 写下去，所以「接受」= 标记已阅（前端内存态、不落盘），「拒绝」= 用已有快照写回）。三级粒度：**单条**（键 `turn:path`）= 还原到这条之前、「**单文件**」= 还原到最早那条快照、「**全部**」= 所有文件各按单文件走。落点：`#todo-drawer-files` 摊开后是审阅卡（`main.ts` 的 `changeReviewHtml` / `applyChangeAction`，事件走 `[data-cr]` 委托），diff 由前端 **`app/src/text-diff.ts` 的 `lineDiff()`** 重算（有界：裁前后缀 + LCS 预算 `|A|×|B| ≤ 400_000` + `MAX_LINES = 800` 截断；`added`/`removed` 是**精确值**、与截断无关），**无需后端回传 diff**。还原复用「回退到此处」的 `restoreSnapshots()`（按回合倒序写回，`existed && content===null` 一律跳过）。i18n 新增 `agent.cr_*`（15 键 × 5 语言），样式 `.cr-*` / `.diff-*`。契约见 `ai-spec.md` 规则 86 + `agent-ui-spec.md` §3.8.1。**验证**：`npx tsc --noEmit` exit 0；`npm run build` exit 0 |
| **U2** | 多轮缩略导航 | **✅ 已落地（2026-10-06）：历史抽屉增强 = 会话可摊开成「轮次缩略」**。左抽屉（`#chat-drawer-list`）每个会话行下多一个 `{n} 轮提问` 开关（`main.ts` 的 `renderDrawerHistory` + `drawerExpanded` / `drawerTurnCount` / `chatTurnLabel`），摊开后逐轮列 `#N + 提问缩略`，**点某轮 = 恢复该会话并 `jumpToChatMsg` 停到那一轮**（此前抽屉只有「整段恢复」一种粒度，要定位某一轮只能恢复后靠右栏那排圆点自己找）。数据源就是 `loadSessions()` 已带回的 messages，**没有新增宿主命令**；条数按 `DRAWER_TURNS_WINDOW`（20）封顶 + 「更早的 N 条提问」（复用 `chat.node_earlier`），与右栏 `CHAT_NODE_WINDOW` 同一条纪律。i18n 新增 `chat.drawer_turns`（5 语言），样式 `.history-turns-*`（照 `.chat-node-item`）。**「会话 Fork」经用户 2026-10-06 裁定取消**（本地单机工具、无分享链路，收益低）—— 本项只剩形态微调与实机验收。**验证**：`npx tsc --noEmit` exit 0；`npm run build` exit 0 |
| **U3** | 真沙箱调研 | Windows 侧隔离手段（Job Object 资源限制 / 低完整性级别令牌 / AppContainer）。**结论需单独立文档**，不得与 `agent-ui-spec.md` §4 的「运行方式」混称 —— 那三档是**策略级**的，`agent-ui-spec.md` §4.1 的诚实原则继续有效 |

---

## 3. Lunac 自身新目标

> 本节不是「旧 cli.exe 有而我们没有」，而是用户直接提出的新方向。
> 调研参照物 = **Hermes Agent**（Nous Research，MIT，`github.com/NousResearch/hermes-agent`）；其「三层记忆」骨架（`MEMORY.md` 2200 字符 + `USER.md` 1375 字符 + `state.db` FTS5 会话检索）与省 token 机制（解析见 `ai-spec.md` §9.1 难点 1）仍是主要参考。

**L1. Live2D 桌宠**

- **还剩什么（2026-10-01 更新）**：**功能本体已做完并真机验收**（外壳 2026-09-29、Live2D 引擎 2026-09-30），**「发出去」这一步也已在 2026-10-01 落地** —— ① Core 已传成公开插件仓库的 Release 资产（tag `live2dcubismcore-5.1.0`，三份：Core + `LICENSE.md` + `RedistributableFiles.txt`；GitHub 自报的三条 digest 与本地逐字节一致）；② `publish-plugins.ps1` 已把 pet `0.9.9` 与索引推上市场（远端 `index.json` 已含 `pet 0.9.9` / `music 0.9.13`）。**只剩「从市场装一次、走完整链路」的端到端验收**（此前都是把产物直接放进 `target\debug\Modules\pet\` 验的）。**协议 5.1 的随附文件（LICENSE / RedistributableFiles）已于 2026-10-01 落地**，见下面那一段。外壳：桌宠插件本体（`Modules\pet\`，源码 `app/src/plugins/builtin/pet.ts`）能开出一个**独立透明置顶、不进任务栏、禁手动缩放、无标题栏**的窗，形象由用户在**控制台**里导入（图片 → `dialog.open()` → 存 `localStorage` → `convertFileSrc` 载入；**Live2D 模型：选 `*.model3.json`**），控制台负责穿透开关 / 形象大小 / 显示·隐藏（**分工不能反过来**：穿透开着时桌宠窗收不到鼠标事件，开关只能放在另一个窗里）。宿主那两条能力也已在桌宠上跑通（穿透开/关 `0x40118 ⇄ 0xC0138`；最小化后动画停、拉回来立刻恢复）。窗口形态这件事由**插件清单的 `window` 段**声明（宿主不按 id 写死），契约与两条实测坑见 `ai-spec.md` §4.8「窗口形态由插件清单声明」，本轮全部实机数据见 `ai-spec.md` 文末实测表。
- **两条硬障碍已解（2026-09-30 最小实验；结论不得再推翻，改动前先重读）**：
  - **① 相对引用 ⇒ 走「自己改写引用」，已验证可行**。机制先复现：`new URL("haru_greeter_t03.2048/texture_00.png", convertFileSrc(<模型绝对路径>))` 落回 `http://asset.localhost/haru_greeter_t03.2048/texture_00.png` —— 挂在 asset 协议**根**上，不是模型目录（该 URL 把整条绝对路径 percent-encode 进最后一段，相对解析无从谈起）。解法 = 解析 `model3.json` → 每个引用 `path.resolve(模型目录, 引用)` → 再过 `convertFileSrc`。**已用真实模型（Cubism 4 的 Haru `haru_greeter_t03`）验过：27 条引用（`Moc` / `Textures` / `Physics` / `Pose` / `DisplayInfo` / 2 个动作组共 20 条）往返解析 0 错，`motion/…` 子目录那条落点也正确**。要改写的字段就这 8 处：`Moc` / `Textures[]` / `Physics` / `Pose` / `DisplayInfo` / `UserData` / `Expressions[].File` / `Motions{group}[].File`。**残留一条**：`motion3.json` 里可能有 `Sound`（相对该动作文件），是**引擎自己**去拉的 ⇒ 桌宠不做口型配音，**不要给引擎挂 sound 管理器**，让它跳过。
  - **② Core 的来源 = 自己托管一份带版本的副本随包落盘，别走 CDN、更别用 npm 那份**。三条本机实测：(a) 官方 `https://cubism.live2d.com/sdk-web/cubismcore/live2dcubismcore.min.js` 这条**连接极不稳定**（同一 URL 连续两次 HTTPS timeout，第三次才 200 / 207KB）⇒ 不能当**运行时**路径（装插件时下载也可能失败）；(b) 官方 URL **不带版本号** ⇒ 写 sha256 必被上游某次更新**卡死安装**；(c) **npm 的 `live2dcubismcore@1.0.2` 是违规转载包** —— 56MB，除专有 Core 外还塞进了官方 **Haru 样例模型（moc3 + 纹理 + 动作 + PSD）** 却标 `license: ISC`，正好踩 L1-B ② 的 No Redistribution，**绝不能进依赖树**。⇒ 结论：Core 由本项目托管**带版本号 + sha256** 的副本（先照官方取一次、固定下来），用宿主已有的 `dependencies[{type:"file", url, dest, sha256}]` 落到插件目录（与 music 的 librespot、ocr 的 PaddleOCR **同一条路**），并**随附 Live2D 的 LICENSE / RedistributableFiles**（协议 5.1 允许复制转发，条件是保留授权文件 + 下游接受同等条款，见 L1-B ①）。
  - **插件怎么找到自己那份 Core（原先的第三个未决点，也已落地）**：宿主已有 **`plugins_dir_path()`** 返回 `<exe 根>\Modules`，再配 `plugin_window_init()` 给的 `plugin_id` ⇒ `convertFileSrc(<Modules>\<id>\engine\live2dcubismcore.min.js)`。**不需要新加宿主命令**，也不要把 Core 打进 `index.js`（那会把专有文件埋进 JS，授权文件反而无处安放）。
  - **栈已定案并落地（2026-09-30）**：选 **`pixi.js@7` + `pixi-live2d-display-lipsyncpatch@0.5.0-ls-8`**（MIT；pixi 7 是它的 peer；0.4.0 那条绑 pixi v6 已放弃，官方 Cubism Web Framework 直连因接入量更大没选）。**lipsync 能力保留**（用户：音频口型后续可能用得上）—— 当前**不给引擎挂 sound 管理器**，`motion3.json` 的 `Sound` 只做引用改写，引擎见没有管理器会自己跳过。
  - **引擎落地形态（本轮新增，改之前先读这段）**：
    - **拆成两个文件**：`app/src/plugins/builtin/live2d.ts`（轻：路径 / 引用改写 / Core 注入 / 控制台的快速校验，**不 import pixi**）+ `live2d-engine.ts`（重：pixi + 引擎库，对外只有 `mountLive2D`）。**必须拆**，因为引擎库在**模块求值时**就 `if (!window.Live2DCubismCore) throw`，而 `inlineDynamicImports`（插件包必须单文件）会把同包内的惰性 `import()` **提前到顶层求值** —— 实测：内联后那句 `throw` 落在产物第 34502 行（顶层），插件一加载就炸。
    - **引擎那一份是「附加入口」**：`scripts/build-plugins.ps1` 的 `pet.extraEntries` 声明 `engine/live2d-engine.js`，同一个插件**再起一次 `vite build`**（`LUNAC_PLUGIN_EXTRA_OUT` / `_SRC`）；产物是自包含 ESM（1179 KB，pixi 全内联），运行时由 `pet.ts` 在 Core 就绪后 `import(convertFileSrc(<Modules>\pet\engine\live2d-engine.js))` —— **绝对 URL**，绕开 asset 协议下相对 specifier 必然落错那条。主入口因此只有 **76 KB**（不含 pixi）。
    - **Core 走 `dependencies[{type:file}]`**（不是打进 index.js）：`dest=engine/live2dcubismcore.min.js`、自托管带版本副本、sha256 `25ae938c…c792f`（207155 字节，Core 自报 `csmGetVersion=83951616` = 05.01.0000）。另加一个**只给打包机用**的 `local` 字段（本机副本 `release\deps\live2d\`）：安装器**不会**下载依赖，所以第 ④ 步把这份按 **sha256 逐字节核对**后预放进 `release\ext-plugins\pet\engine\`。`local` **不进清单**（写清单时摘掉）。
    - **用户导入 = 选那个 `*.model3.json`**（`dialog.open` + 过滤器 `json`）。**不动宿主、也不枚举目录**：同目录的纹理 / 动作 / 表情由改写后的引用自动带上。选完**当场在控制台验一遍**（`validateModelFile`：取 json → 过 `assertCubism4` → 报「Cubism 4 · 纹理 N · 动作组 …」），坏文件在这里就报错 —— 桌宠窗小，用户还可能已经把它收起来了。
    - **两种形象互斥**（存的时候互斥）：选图片 ⇒ 清 `model`；选模型 ⇒ 清 `image`。不互斥的话模型永远压着图片，用户会看到「选了图片但什么都没变」。
- **最小原型已量完（2026-09-29）**：数据与取证方式见 **`architecture-rendering.md` §6.2**。三条结论 —— ① 第二个窗口的边际成本 = **+1 个 `renderer` 进程**（不是再起一个实例）/ 工作集 **+109 MB**、私有 **+54 MB**，关窗即回收；② **最小化不会让 rAF 停**（仍 ~170 fps、~5% 单核，`document.visibilityState` 全程是 `visible`）；③ 透明置顶窗的**透明度已经实测成立** —— 宿主侧那两个前置**已落地并实机验证**（`ai-spec.md` §4.8 与文末实测表）。
- **插件市场这一层不必再设计**：落盘 `<exe 根>\Modules\<id>\`、清单 `lunac-plugin.json` + 已编译 ESM、https zip 安装 / 卸载，契约见 `ai-spec.md` §3.5「插件市场」；**插件带代码不带模型**。
- **发布这一步（2026-10-01 已完成）**（「让桌宠能到用户机器上」的动作）：① 现在的 `-Plugin pet` 产出 **`pet-0.9.9.zip`（302.2 KB / 实测 309407 字节）** —— `index.js` 76 KB + `engine/live2d-engine.js` 1179 KB + `engine\LICENSE.md` + `engine\RedistributableFiles.txt`，另有一份 `engine/live2dcubismcore.min.js`（207155 字节）**由 `dependencies` 在安装时装**、并已预放进 `release\ext-plugins\pet\`（安装器那条路不会下载）。**Core 已传成公开插件仓库的 Release 资产**（2026-10-01：tag `live2dcubismcore-5.1.0`，URL 已写死在 `build-plugins.ps1`，sha256 已核对；**三份一起挂**（Core + `LICENSE.md` + `RedistributableFiles.txt`）—— 只挂 Core 等于没满足条件），缺这一步市场安装会**整包失败**（`dependencies` 任一条失败 = 整次安装失败，这是有意的）。② **Live2D 的 LICENSE / RedistributableFiles 已随附（2026-10-01 落地）**：原文放仓库的 `licenses\live2d-cubism-core\`（取自官方 SDK 的 `Core/`，5-r.1 与 5-r.5 逐字节相同 ⇒ 与 Core 版本无关），由 `build-plugins.ps1` 的 `notices` 复制到 Core 旁边；**zip / `release\ext-plugins\pet` / dev 的 `Modules\pet` 三处已实测带上**，缺一份脚本直接 `throw`（见 `code-rules.md` 预检 #55）。③ **已推市场（2026-10-01）**：`publish-plugins.ps1` 跑通，远端 `index.json` 现含 `pet 0.9.9` 与 `music 0.9.13`（clipboard-history / convert / ocr 维持 `0.9.8`）—— 该脚本自带「每个插件只发最高版本」过滤，否则换版本后 `plugin-packages\` 里的旧 zip 会让同一 id 在市场列表里出现两条（见预检 #43 / #55）。**两条环境注意**：`gh` 在本机**可用但不在 PATH**（`%LOCALAPPDATA%\Programs\gh\bin\gh.exe`，脚本自带的递归查找能命中）；`git push` 到 github.c回合结束后om 偶发超时（同机 TCP 可达、`gpi` 正常）⇒ 失败时在克隆目录 `D:\cc\lunac-plugins` 重跑一次 `git push` 即可，**脚本的本地提交已完成、不必重跑整个脚本**。④ ~~`lunac-installer.nsi` 的「拓展插件」段要再补一段~~*：`Section /o "Desktop pet" SecExtPet` 已加（N只在SI 从四段变五段），规则与「漏改不报错」的提醒见 `ai-spec.md` §3.5 插件市场。

****L1-A**. 桌宠的 `pendingMcpConfigRestart` 三条已裁决（2026-09-21 用户拍板，实施时不得自行改回）**

| 项 | 裁决 | 代价 / 约束 |
|---|---|---|
| **形态** | ****完全独立**透明置顶窗**（真桌宠，另开一个 Tauri 窗口 = **第二份 WebView2**） | 用户已知情并选择接受（此前为省 11MB 的 `--disable-gpu` 都专门权衡过，这一条要写进实施记录，别事后当成「忘了优化」）。**代价口径已按 2026-09-29 实测修正**：第二个窗口**只多 1 个 renderer 进程**（不是再起一个实例），边际 = 工作集 **+109 MB** / 私有 **+54 MB**，详见 `architecture-rendering.md` §6.2 |
| **形象来源**本 |用旧配置答完用户（己导入模型**；Lun；c 只提供引擎 + 导入通道 | 安装包**零第三方模型资产**。义务因此落在终端用户身上（他下载时自己接受 Live2D 的协议） |
| **实施顺序* 本重读仍随dcu；正但leF下*提问近生效」
Mao静默重启是硬般用mo纹理打进 Lunac 安装包 = 禁止**（把它当默认桌宠随包发正好踩这条）。附带义务：必须写官方指定的**版权声明**（长 / 短两版，视载体而定）；**Miara / Hiyori 的设计不得做任何改动**、Shizuku 不得改名改设定、Mark-kun 不得画成写实帅哥、Nito 保持头身比；个别样例对「直播使用」另有额外限制。**协作角色（名執尽 / 春日笠つみき）更紧：非商用 only、不得改动、不得分发** ⇒ 桌宠里绝不能用。**外部授权角色**（初音ミク / ずんだもん / ユニティちゃん）要遵守第三方各自的条款。

**③ 角色 IP 侧（最容易被忽略）**：即便模型是自己建的，只要画的是**别人的角色**（VTuber / 游戏 / 动画），就是**角色著作权与商标**问题，与 Live2D 授权无关；**同人模型不能随公开发行的软件分发**。判据与既有纪律同源（`core/` 不入库、上游 `cli.exe` 不进包）：**公开仓库 ≠ 什么都能放**。

**④ 我们自己的代码与插件市场**：`pixi-live2d-display`（MIT）+ pixi.js（MIT）可用，但 **MIT 只覆盖包装代码**，Core 仍受 ① 约束，许可证文件要随包保留。L1 会**放大**这条：从用户 GitHub 仓库下载的插件若自带模型数据，责任落在「谁分发」⇒ **插件只带引擎与逻辑、不带模型**，模型一律由用户导入。

- 相关：`ai-spec.md` §20 的「路径 2」是本节的前身。**注意 `core/` 的含义**：它是**本机参考用的旧 CLI 源码**（被 `.gitignore` 排除、**不在仓库里**），所以 `core/plugins/...` 一类路径只表示「参考它的设计、在新宿主重建」，**照它去找一定找不到**；真要做时**先重写落地路径**。

**L3. 上下文感知提示词注入的剩余子集**

- 现状（**部分已落地，别当从零做**）：① 工作目录 / 宿主 / 技能目录已由 `core-agent` 的 `env_block()` 拼进系统提示词；② 前端已有按 query 关键词（debug / TDD / review）的 `buildSystemPromptHint()`，拼进**消息**；③ 附件路径走 `[Attached files]` 文本。
- **缺的只有**：「当前打开的插件 / 当前选中的文件」进上下文。补的时候注意区分「进系统提示词」（必须固定，否则破坏缓存）与「进消息」（可变）。

**L4. 调试阶段状态栏**

- 原 `ai-spec.md` 的 P2 遗留项（现 §19.1）。做一个只由开关控制的阶段状态栏，展示 agent 当前处于哪个阶段。现状只有**常显**的状态机与运行时提示（`.sys-note`，展示 agent stderr），没有分阶段的调试视图。

**L5. 自研调音（DSP 引擎 + 测量 + AI 调参，**不依赖 EqualizerAPO / Peace**）**

- **🔴 2026-10-01 用户改口径：取消「独立调音插件」** —— 调音**内置进音乐插件**
  （与本地音乐播放同一层：都是「Lunac 自己放的那条流」）。已经落地的形态：
  `tuning-engine` 以**库依赖**进宿主（`app\src-tauri\Cargo.toml` 的 path 依赖），
  在 `player.rs` 的播放链上套一层 `Chain::process_sample`；面板 = 音乐插件的**调音页**
  （`#music-v-tuning`，入口 = 标题栏三段切换的第三段；2026-10-02 P0 起是**可编辑的链**：
  总开关 + 五档预设展开 + 频响曲线 + 预增益 + 逐段滤波器表，见下面 §0 M2-11），
  配置 `<exe 根>\config\tuning.json`（`enabled` + `preset` + `preamp_db` + `filters`），
  **默认关闭**（DSP 改声音 ⇒ opt-in）。**不再有下面那条「独立 exe + NDJSON 面板」的形态**
  —— 那条留着只作历史记录；`Modules\tuning\`、市场条目、`tuning-engine.exe` 随安装包走
  这三件事**都不做了**（`tuning-engine` 的 CLI 仍是离线手验工具）。
  契约见 `ai-spec.md` §4.6「调音」与 §4.10，纪律见 `code-rules.md` 预检 #51。
- **方向（用户 2026-09-29 定）**：**自研 DSP 引擎**；EqualizerAPO / Peace 只作**学习参考**（学它的模型与交互），最终**脱离它们**。要的能力面：**测量频率曲线**（含 **WASAPI loopback 电气闭环**）、**用 AI 调各频段 dB**、**多个效果器**、**接管输出 / 输入设备**。
- **🔴 2026-10-03 口径补充（用户改主意，避免走偏）**：中途曾考虑「**破例直接读写 EqualizerAPO 配置**」，**已否决** —— 因为 **Peace 只是 APO 配置的前端**（实测本机：`config\config.txt` 只有一行 `Include: peace.txt`；`peace.txt` 是它生成的 EAPO 文本；`*.peace` 是 Peace 私有 INI 预设，含 `[Frequencies/ Gains/ Qualities/ Filters]` + `[Frequencies1..8]` 八个声道槽 + `[General] PreAmp/Off1..8` + `[Speakers]`）。所以「读/写 EAPO 配置」等于去当下游前端的下游，与 L5 的方向相反。**最终口径**：**参照 EAPO 的「能力面」重构自研引擎、参照 Peace 的「前端交互」重构调音页**，运行时**仍不依赖** EAPO / Peace（本机那份 EAPO 只作对照，不顺带改它）。分期（2026-10-03 用户要求「全都要」，分多轮）：
  - **P1（纯 UI，零架构风险）** ✅ **已落地（2026-10-03）**：总开关关=近全黑 / 开=满 accent + 白光晕；删掉页面下方说明（死键一并删）；预设从分段按钮改**列表**（`music-preset-item`）；频响曲线放进横向滚动容器 + 加 `－ 100% ＋` 缩放（100%–300%，按 `clientWidth` 重画）。
  - **P2 数字编辑界面** ✅ **数字界面已落地（2026-10-03）**：`#music-tune-view` 按钮在「表格 / 数字」两态间切换；数字态 = 横排**竖向拉条**（每根 = 一段的 dB 增益，`writing-mode: vertical-lr`）+ 选中段明细（频率 / 增益 / Q 输入 + 类型按钮 + 开关 + 删除）+ 复用既有「加一段 / 删一段 / 选类型」。**刻意复用 `.music-tune-row[data-i]` + `data-f` / `data-act`** ⇒ 三条既有委托与 `commitTuningChain` 一行未改。**未做**：Peace 的 **8 声道槽**模型（留 P5/P6）。
  - **P3 引擎能力面（参照 EAPO）**：`GraphicEQ` ✅ **已完成（2026-10-03，`cargo test` 58 条全绿）** —— 顶层 `graphic_eq` 字段解析期展开成 peaking 段（Q 按相邻频点对数间距用 RBJ 的 `Q=1/(2·sinh(ln2/2·BW))` 折算），契约见 `ai-spec.md` §4.10。`Delay` + 声道复制 ✅ **引擎侧已完成（2026-10-03，`cargo test` 67 条全绿）** —— 新增 `ChainRuntime`（每声道滤波器状态 + 每声道环形延迟线 + 复制表，处理顺序 ①preamp ②双二阶 ③延迟 ④复制），配置加 `delay_ms` / `channel_copy`，`render()` 已切过去。`Convolution`（FFT 卷积 + 读 IR）✅ **已接进引擎（2026-10-03，`cargo test` 86 条全绿）** —— `fft::ifft_in_place` + `convolution::convolve`（overlap-add，与朴素直接卷积对拍）+ `convolution::load_ir`（**单声道 + 采样率一致**校验，不做重采样 / 多声道 IR）+ `convolution::apply_interleaved`（逐声道、**长度不变**、尾巴截断）；配置加顶层 `convolution`（IR WAV 路径），`Chain::build` 读盘并校验（缺失 / 不匹配则编链失败），`render()` 在最后一步（①preamp ②双二阶 ③延迟 ④复制 之后）做卷积。**`response()` / `response_db()` 刻意不含卷积**（任意 IR 无解析参照物，±0.1dB 判据只覆盖参数段）。**流式卷积**（`convolution::Convolver`，与接线同一批）见下。`If-Else` 条件段 ✅ **已完成（2026-10-03，`cargo test` 91 条全绿）** —— 顶层 `if_else: [{ channel, then: [...], else: [...] }]`：条件是**静态（按声道）**的，`ChainRuntime::new` 拿到声道数就定下每条声道跑哪一串（逐样本只查表）；`then` / `else` 走与 `filters` 同一套校验，**越界声道 / 两边都空一律报错**（不许静默失效）。**宿主接线也已完成（2026-10-03）**：`player.rs` 的 `TuningSource` 从逐样本 `Chain::process_sample` 换成 `ChainRuntime::process_frame`（按帧 ⇒ **攒帧**，缓冲复用不分配），去掉「段数 == 0 就跳过」那条捷径；新增引擎侧 `convolution::Convolver`（**流式** overlap-add，每声道一个，**延迟 = IR 长度 − 1**）与 `ChainRuntime::reset()`（seek 用）；`TuningConfig` / DTO / `player_tuning_set_chain` 带上全部五项（面板到 P5 才有界面，眼下**原样透传**，改 EQ 不会把 IR / 延迟冲掉）。验证：引擎 `cargo test` **95 绿**、宿主 `cargo test player::` **21 绿**。**P3 到此收口。**
  - **P4 测量最终输出** ✅ **已完成（2026-10-03）**：测量从**一趟改两趟** —— 同一段扫频先**直通**播一遍（`player::plain_buffer`）量「系统」（EAPO / 别的软件 / 设备），再**走链**播一遍（`tuned_buffer`）量「最终输出」，两趟相除得到**「仅自身链（实测）」**。面板因此画**三条线**：合成（解析式）/ 仅自身链（实测，绿虚线）/ 最终输出（含系统，蓝点线）；结果行报「仅自身链 vs 合成 平均差」+「系统平均影响」两个数。**判据只用前两条** —— 拿最终输出去比会被系统音效污染。代价：出声约 6 秒（两段扫频）。宿主测试 `two_pass_subtraction_isolates_the_chain_from_the_system` 钉住相减口径（造一个 1kHz「系统」+ 一个 3kHz「链」，要求 `chain_db` 只量到链自己）。
  - **P5 前端对齐 Peace（用户 2026-10-03：「全都要，分多轮」）**：
    - **P5-1 预设管理 + 导入导出** ✅ **已完成（2026-10-03）**：**预设 = 一整条链的快照**，落 `<exe 根>\config\tuning\presets\preset-<文件名>.json`（文件名净化过 + `preset-` 前缀躲 Windows 保留名；**显示名**存在 JSON 的 `name` 里，删 / 改按显示名扫目录找）。调音页在内置 5 档之外多一区「我的预设」：保存为预设（同名先返回 `ERR_PRESET_EXISTS`，前端把按钮换成「覆盖」再点一次才算）/ 重命名 / 删除（先过命名行确认）/ 导入（**导入即应用**）/ 导出当前链。宿主 5 条新命令 + DTO 带 `user_presets`；**文件对话框在前端**（WebView2 没有 `window.prompt` ⇒ 命名走面板内联输入行）。⚠️ **语义升级**：`player_tuning_apply_preset` 现在是**整条换**（以前只换 preamp + filters、保留 IR 等）—— 与「预设 = 一条链的快照」一致。
    - **P5-2 扬声器声道槽（8 槽）** ✅ **已完成（2026-10-03）**：仿 Peace 的声道选择 —— 调音页「滤波器」上方多一排槽（`[0]` 全局 + 每条声道一个，槽名 `L / R / C / LFE / SL / SR / BL / BR`），点一下就把下面的表切到那条声道的链。映射**一一对应**、不新增中间模型：全局槽 ↔ 引擎 `filters`，通道 `k-1` ↔ 引擎 `if_else` 里 `channel = k-1` 那条的 `then`。**槽是视图状态**（不落盘），全部槽的草稿并存、提交时由 `tuneIfElsePayload()` 整批发（`else` 分支面板没界面 ⇒ **原样保留**宿主那份）。槽数 = `1 + 宿主最近一次建链那条流的声道数`（`0` 兜底 2 声道）；**宿主按当前流裁越界声道**（`TuningConfig::for_channels` + 一行日志）—— 不让「6 声道配置 + 立体声素材」把全局 EQ 一起作废。曲线跟着槽走：宿主新增 `Chain::response_db_for_channel`（= 全局 + 该声道**逐块**命中的那一支），经 `TuningDto.cond_db`（与 `if_else` 同序）下发，前端只按当前槽选一条画。验证：引擎 `cargo test` **96 绿**（+1）、宿主 `cargo test` **191 绿**、`npx tsc --noEmit` exit 0、`npm run build` 通过。
    - **P5-3 GraphicEQ 界面** ✅ **已完成（2026-10-03）**：调音页「预增益」与「滤波器」之间多一块 `#music-tune-geq` —— 一排**竖向拉条**（复用数字界面那套 `.music-tune-vslider`，`overflow-x: auto` 横向滚动），一根 = 引擎 `graphic_eq` 里的一段；每根下面写频率名、上面写增益读数。空链时只给一颗「启用」按钮（铺 **10 段 ISO**：31.5/63/125/250/500/1k/2k/4k/8k/16k 全 0dB），非空时给「全部归零 / 关闭」。**面板不提供「换组」** —— 换组会静默清掉用户已调好的增益（「不许静默覆盖」那条纪律）；已有自定义频点原样显示、**不排序不去重**（引擎要求严格升序、Q 由相邻间距折算，`RawGraphicBand` 是 `deny_unknown_fields`）。**它是顶层（全局）字段** ⇒ 只在「全局」通道槽里显示。曲线**自动反映**（引擎解析期就把 graphic_eq 展开进 `cfg.filters`，`response_db` 与实测两趟都天然含它）。验证：`npx tsc --noEmit` exit 0、`npm run build` 通过。
    - **P5-4 卷积 / 延迟 / 复制 界面** ✅ **已完成（2026-10-03）**：调音页最下面三块，按引擎处理顺序排 —— **延迟**（数字框 + 横向拉条，一律夹到 `0..=5000` 以免顶掉整条链）→ **声道复制**（行 = 「源 → 目标 + 删除」，两侧按钮点开是当前流的声道菜单，只让在流内选）→ **卷积**（文件对话框挑 IR WAV + 显示当前路径 + 关闭）。三样都是**顶层（全局）字段** ⇒ 与图示均衡器同一条纪律：只在「全局」通道槽里显示。**只挑文件、不在面板校验 IR**（单声道 + 采样率一致只有拿到渲染流采样率才判得了）—— 校验发生在提交那一次，失败走 `commitTuningChain` 既有的「报错 + 回滚」。**顺带补掉宿主一个真缺口**：`TuningConfig::for_channels` 原来只裁 `if_else`、没裁 `channel_copy` —— 越界的复制项同样会让 `ChainRuntime::new` 报错、把**整条链**（含全局 EQ）顶掉；现在两样一起裁（`TuningCopy` 因此从「不透明 `Value`」**定成类型**），单测 `for_channels_drops_slots_and_copies_the_stream_does_not_have` 钉住。**P5 到此收口**（P5-1~P5-4 全部完成）。验证：宿主 `cargo test player::` **26 绿**（含新断言）、`npx tsc --noEmit` exit 0、`npm run build` 通过。
    - **P5-5 按 Peace 主窗对齐（三栏骨架 + 曲线独立成窗）** ✅ **已完成（2026-10-03）**：用户给了 Peace 1.6.8 主窗截图（1043×728）要求「严格对齐」。**我读不到图像**（本模型不吃图；子代理也一样 —— 最后靠 Tesseract OCR + 原图逐像素测量把结构量出来，并回答了他「能不能读到这张图」）。落地：① **调音页重排成三栏** —— 左 = 预设列表（Peace 的配置名列表）、中 = EQ 主体（两条「左标签 + 滑块 + 右数值框」的全局滑块：预增益 / 延迟；GEQ 推子组：**频率标签移到推子上方** + 补左侧 `+30 / 0 / −30 dB` 刻度；GEQ 下方是参数段表）、右 = 通道槽（**改成 Peace 那种竖排列表**）+ 全局效果器（声道复制 / 卷积）。② **频响曲线独立成窗** —— 宿主 `open_plugin_window` 加可选 `key`（label `plugin-music-curve`，仍落在 `plugin-*` 通配里，见 ai-spec §11 规则 83），音乐插件靠**读自己的窗口 label** 分岔 `execute()` / `attach()`；曲线窗与调音页**共用同一组 id**，`wireTuningCurve` 一份接两处；窗里只读（不画拖动手柄）、1.2s 轻轮询跟主窗的改动。③ **设置页加「调音…」入口**（标题栏三段切换保留）。改法上有一条硬纪律：**重排只动 HTML 位置、id 一个都不改** —— 这一页的接线函数全按 id 找节点。验证：宿主 `cargo test` **192 绿**（+1 `keyed_labels_stay_under_the_plugin_glob_and_get_their_own_size`）、`npx tsc --noEmit` exit 0、`npm run build` / `cargo build --release` 通过、插件 v0.9.23 已装。
    - **P5-6 用户第 7/8 条：中栏合并 + 文案精简 + 三段全局滑块** ✅ **已完成（2026-10-03）**：① **删掉整批说明文案**（用户点名 12 组：`tuning_hint` / `tuning_custom` / `tuning_open_hint` / `tuning_geq*` / `tuning_slot_*_hint` / `tuning_delay_hint` / `tuning_copy(_empty/_hint)` / `tuning_conv_hint` / `tuning_global_fx` / `tuning_empty` / `tuning_user_presets(_empty)` / `tuning_presets`），`i18n.ts` 里的死键一并删（code-rules §13 ⑤ 的正例）。② **中栏三条全局水平滑块** = 总增益（原「预增益」改名）/ 低音增益 / 高音增益 —— 后两条**不是引擎字段**：`TuningConfig.bass_db` / `treble_db` 由 `effective_json` 追加成 `low_shelf`(120Hz) / `high_shelf`(9kHz) 两条（**0dB 不追加**），曲线照算、频段柱不显示它们（快捷旋钮，不是用户手加的段）。③ **滤波器与均衡器合并**：频段柱（上→下 = 频率**可编辑** / 竖拉条 / **大号**增益 / Q / 类型按钮 + 开关 + 删除）改编辑**链滤波器**；`graphic_eq` 的独立界面删除、`tuneGeq` 降级成**纯透传**（读回来原样发回去，不静默清掉盘上已配好的 GraphicEQ）；表格 / 数字界面切换（`tuneView` / `tuneSel` / `#music-tune-view`）整批删除。④ **频响曲线两扇窗都能拖**（撤销 P5-5 那条「窗里只读」）+ **滚轮按鼠标位置缩放**（`wheel` 必须 `{ passive: false }`、锚点用 `clientX`、缩放后补 `scrollLeft`）。⑤ **内置 5 档预设整批删掉**，工具条（保存 / 导入 / 导出）移到左栏最上。⑥ **定宽**：左栏 180px / 右栏 220px / 数值框 60px。⚠️ 一根频段柱里 `gain_db` 有**两个**控件（竖拉条 + 大号数字框）共用 `data-f` ⇒ `commitTuningChain` 的焦点还原必须写成 `input[type="text"][data-f=…]`（只按 `data-f` 会命中排在前面的拉条）。**顺带修一个真 bug**：`music.tuning_slots` 键**从未定义过**（`t()` 缺键返回键名）⇒ 右栏与曲线窗标题一直显示字面量 `music.tuning_slots`，已按用户的叫法补「扬声器声道」。验证：`cargo test --bins` **193 绿**（+1 `bass_and_treble_shortcuts_become_shelf_filters_only_when_nonzero`）、`npx tsc --noEmit` exit 0、`npm run build` 通过、插件 v0.9.24 已装到 `D:\Lunac`。
    - **P5-7 默认档改空链 + 曲线窗反向同步** ✅ **已完成（2026-10-03）**：① 用户口径「改成空链条，并将原声作为初始值」⇒ `DEFAULT_PRESET` 由 `bass` 改成 **`flat`**（内置预设已从面板删掉，那份默认链再无可见来源 ⇒ 属于「看着没开、其实在起作用」，必须去掉）；单测改判据为「回落默认档后 `filters` 为空 + `preamp_db == 0`」。② 用户报「均衡器能影响频响曲线，但是反之不行」⇒ 曲线在**另一扇窗**里，那边有 1.2s 轮询所以正向成立；主窗原先只在**切进调音页**那一刻 `loadTuning` 一次 ⇒ 在曲线窗拖出来的段回不到中栏。修法：进调音页起一条 **1.2s 幂等轻轮询**（`startTunePagePoll` / `stopTunePagePoll`，`stopMusicPolling` 一并收）。③ 用户报的「1500Hz +9dB 但实测曲线该频率没变化」经确认是**测量时调音总开关关着**（关着 = 逐位直通，两趟相同 ⇒ `chain_db` 恒 0dB；而合成曲线照画，因为 `tuning_dto` 算 `response` 不看 `enabled`）—— 用户明确「不需要修」，结论记进 ai-spec §4.6 免得下次再查。验证：宿主 `cargo test --bins` **193 绿**、`npx tsc --noEmit` exit 0、`npm run build` / `cargo build --release` 通过、插件 v0.9.25 + 新 `lunac.exe` 已装到 `D:\Lunac`。
  - **P6 全局生效（S3）+ 双模式开关**：默认输出切**虚拟声卡** → 采集 → DSP → 回真实设备（**不碰 APO**）；届时调音页给出「Lunac 音乐 / 全机托管」两档开关（用户 2026-10-03 追加要求）。
  - **P7 在线音乐也进调音（2026-10-04）** ✅ **已完成**：过去在线歌单是 librespot 这个**独立进程**自己出声、链一点都管不到。现在给 librespot 加 `-B pipe -f F32`（**只在非登录启动时** —— 登录那次 `librespot-oauth` 会把 `Browse to:` 打到 **stdout**，混进 PCM 会让后面的 f32 整条错位），宿主新增 [app/src-tauri/src/live_audio.rs](../app/src-tauri/src/live_audio.rs)（**无锁 SPSC ring** + reader 线程 + `PipeSource`）把裸 PCM 喂进**同一个** `TuningSource` ⇒ 逐段开关 / 声道槽 / 延迟 / 复制 / 卷积 / 条件段**全部自动生效**，一条额外规则都不用加。三处收口：`librespot_start_inner` 的 stdout 从「日志」改成音频（日志只看 stderr）、`librespot_stop_inner` / `kill_librespot` 都调 `live_audio::stop()`（顺序 = 先杀进程再收链）。⚠️ 我们那条 Player 音量必须 **1.0**（librespot 已按 Connect 音量做过 softvol，见 ai-spec §4.6）。契约与实测依据见 ai-spec §4.6「在线音乐也走同一条链」。验证：宿主 `cargo test --bins` **199 绿**（+2：ring 先进先出 / 空 ring 吐静音而非结束）、`npx tsc --noEmit` exit 0。
- **平台硬约束（先记住，别绕）**：Windows 上「让**系统级**音频经过自己的处理」只有两条路 —— ① **APO**（COM in-proc，跑在 `audiodg.exe` 里，崩溃会带走整个音频服务；EqualizerAPO 做了十几年）② **虚拟声卡**（第三方内核驱动）。**本项目明确不做 ①**，所以「全局生效」落在 **S3**：默认输出切到虚拟声卡 → 我们采集 → DSP → 渲染回真实设备。
- **形态（⚠️ 已被 2026-10-01 那条口径取代，留作历史）**：引擎 = **插件自带的独立 exe**（Rust，走 `dependencies[type=file]` 装进 `Modules\<id>\bin\`，与 `music` 的 librespot **同一条路** ⇒ 卸载即完全不存在、宿主不必为它长大）；面板 = 纯 UI（曲线图 / 滤波器表 / A-B / 测量）；两者之间 **照抄 `agent.exe` 的 stream-json 范式**（stdin/stdout NDJSON + 日志落盘），**不发明新协议**。**配置与测量的落盘归引擎** ⇒ 「插件要写盘」这件事在本架构下**不需要**给宿主加通用文件命令。**现形态**：库依赖 + 宿主进程内的 `Chain`（不需要 exe、不需要 NDJSON、不需要 `Modules\tuning\`）。
- **配置格式自立为 JSON**（不再用 EAPO 的文本语法）。**从 Peace / EAPO 学的四样**：滤波器类型表（PK / LS / HS 各阶、LP / HP / BP / NO / AP）、效果器链的组织方式、作用域（设备 / 通道 / 前后级）、快照与一键 A-B。
- **分期**：**S0** 引擎内核（**完全离线可单测**：WAV→DSP→WAV，用「某滤波器在某频点的增益 = 理论值 ±0.1dB」钉住；FFT / 反卷积 / 配置 JSON）→ **S1** 自身播放链路闭环（先只管 Lunac 自己放的音乐，零驱动零 UAC）→ **S2** WASAPI **loopback** 闭环测量（本机「立体声混音」是 ACTIVE 的）→ **S3** 全局接管（虚拟声卡 + 按设备切配置 + 输入侧）→ **S4** AI（意图解析 / 选目标曲线 / 复核微调 / 解释；**确定性拟合在引擎里，AI 的输出必须过本地关口**：参数合法 + 拟合残差不变差 + 增益结构不削波）。
  - ✅ **S0 已完成（2026-09-30）** —— `tuning-engine/`（与 `core-agent/` 并列的独立 crate，只依赖 `serde_json`，手写 radix-2 FFT），七个模块 + 四个子命令的 CLI，**54 条单测全绿**，端到端验收「`gen`→`render`→`measure` 实测 +5.94dB vs 理论 +5.94dB」。**契约、两条测量通路的口径、CLI 与「踩过的三个坑」全部落在 [`ai-spec.md`](./ai-spec.md) §4.10 与规则 72，实测数字在 §3.5 文末那一行**。**反卷积没做**（S0 用冲激响应即可，它解析地可行），**推到 S2** —— 脉冲灌不进 loopback，那时才需要「播扫频 + 录回来 + 反卷积」。
  - ✅ **S1 已完成（2026-10-01 最小可用 → 2026-10-02 P0 可编辑的链）** —— `tuning-engine` 以**库依赖**进宿主，在 `player.rs` 的本地播放链上套一层 `Chain::process_sample`；面板 = 音乐插件的调音页（**总开关 + 预设展开 + 频响曲线 + 预增益 + 逐段滤波器表**，按下即生效即落盘）。**仍没做的**：A-B 盲测 / 实时扫频测量 / 卷积（EAPO 的 `Convolution`）/ `GraphicEQ` / 延迟与声道复制（那些留在本条的后续）。契约见 `ai-spec.md` §4.6「调音」与 §4.10，纪律见 `code-rules.md` 预检 #51，本轮改动清单见本文 §0 M2-11。
- **两个必须提前知道的坑**：① **EqualizerAPO 目前挂在本机 13 个端点上**（9 Render + 4 Capture，APO CLSID `{EACD2258-FCAC-4FF4-B36D-419E924A6D79}` / `{EC1CC9CE-FAED-4822-828A-82A81A6F018F}`）—— 引擎上马时两边会**叠加**，且测量曲线里会混着 EAPO 的 EQ；「迁移口径」（何时卸、卸 APO 要 UAC + `Configurator.exe`）要单列一条，否则「测不准」会被误判成引擎的 bug。② S3 的虚拟声卡是**第三方内核驱动**（VB-Cable / Voicemeeter，各有许可条款），自研内核驱动不现实 ⇒ **S3 开工前再选**。
- **附加功能候选**（有自研引擎后不再受 EAPO 能力限制）：自动 Preamp 防削波 / 等响度补偿 / 交叉馈电 / 卷积与房间校正 / A-B 盲测 / 听力自测补偿（必须标「非医疗器械」）/ 把 PEQ 导出给耳机 App 与 DAP / 实时频谱 / 多声道低音管理。
- **落点（⚠️ 2026-10-01 改）**：**不再有 `Modules\tuning\`**（用户取消了独立插件形态）。引擎源码仍在本仓（`tuning-engine/`，S0 已建成、S1 最小可用已接进宿主），但**不作为插件发货**：它是宿主的一条库依赖，随 `lunac.exe` 一起编译。将来的「面板」若长回音乐插件之外的形状（曲线图 / 测量），再单独定落点。
- **诚实提醒**：本条的体量**远大于** L1 桌宠 —— S0/S1/S2 是能独立验收的三小步，S3 才碰系统音频。

**L6. 成本归因与降本（2026-09-29 提出；用户口径「Lunac 的 API 金额是 Trae 的 10 倍」）**

- **归因已结案**（数据与推导见 `ai-spec.md` §9.1 难点 3）：**不是计价贵** —— 两个 key 同账号，那两档价是 DeepSeek **2026-08-17 起生效的官方峰谷定价**（工作日 09:00–12:00 + 14:00–18:00 峰、其余含周末谷、谷 = 峰 ÷ 2）；11.2 倍 = **命中率差 3.09× × token 结构差 3.63×**（输入命中 98.30% vs 89.14%；输出占 token 的 0.50% vs 12.4%）。**「元 / 百万 token」不能当 KPI**（它奖励「堆缓存命中」，会把「上下文很长但几乎全命中」判成先进），改用「每次提问成本」。
- **已完成，不得回退**：① **子代理 / 后台复盘的用量并入 `result.usage`** —— 此前它**完全没进本地账**（平台同一 key 24 次请求 vs 本地 `usage-*.jsonl` 17 次），契约与守门单测见 `ai-spec.md` §3.5「用量与对账」；② **思考档 A/B 实测**（结论：**不是主因**，是 1.0~1.3× 的乘数；探针 `core-agent\target\hooktest\e2e-ab-thinking.ps1`，数据见 `ai-spec.md` §9.1 难点 3 结论 6）；③ **价格表支持时段价 + 预置官方峰谷价**（`time_windows` 契约见 `ai-spec.md` §3.5 与规则 63，实测见文末）—— 修好前面板对谷时用量**高估 45%**（dev 环境 2026-09-29：0.198505 → 0.136858 元），与账单已能**逐分对上**。
- **已完成，不得回退**（续上一条的 ①②③，本轮 2026-09-29 做完 ④⑤⑥）：
  1. **输出瘦身** —— 输出纪律进了 `PERSONA_AND_STYLE` 的 Output Style 段（无开场白 / 不预告工具调用 / 不复述刚读到的内容 / 只答被问的 / 不贴回用户已能看到的正文）。固定任务三次采样：答案字符 **1156 → 701 / 474**，稳态**每次提问成本 -41%**（4854 → 2843 miss 等价）。探针 `core-agent\target\hooktest\e2e-l6-cost.ps1`，数据见 `ai-spec.md` §9.1 难点 3 结论 7。**改这两块常量会打掉一次端点侧缓存**（每台机器每套前缀各一次，不是每次提问都付）—— 别为此把新句子挪进用户消息。
  2. **压请求次数：已结案（不是本侧的锅）** —— 查清 `TOOL_PARALLELISM` 的语义是「**同一个响应里 N 个 `tool_use`、本地并发执行**」，**不是**并发发 N 份请求（`ai-spec.md` §9.1 结论 8）；`SYSTEM_PROMPT` 里加了「互不依赖的读 / 搜放进同一次响应」的引导，但**实测三次请求数全是 4、工具序列逐字相同 ⇒ 未生效**。结论：「一次提问 7~10 次请求」是**模型自己的往返节奏**，**不要再动并行实现**；真要压只能从「让模型少问几轮」入手（任务熟悉度 / 更强的工具描述）。
  3. **对账基线工具就绪** —— [reconcile-usage.ps1](file:///d:/cc/claude-code-cli-master/scripts/reconcile-usage.ps1)：本地 `usage-*.jsonl` 按**本地小时**聚合四类 token + 用 `pricing.json` **逐桶**峰谷计价（14:00 桶 = 0.07521104 元，与手工核算逐位一致）；`-Csv <平台导出>` 再做逐小时对照（表头模糊识别，认不出 `exit 2`；金额差 > 0.0001 元标 `DIFF` / `exit 1`）。契约见 `ai-spec.md` §3.5「定价表与成本面板」。
  4. **成本面板的表口径与时间粒度（2026-10-06 用户定）** —— ① **表格列**改成 `时间 / 请求次数 / 输入（命中缓存）/ 输入（未命中缓存）/ 缓存写入 / 输出 / 金额`（三列输入各自乘自己的单价，加起来正是金额列 ⇒ 「缓存写入」**刻意不并进「未命中」**，否则没法逐列对账）；② **「请求次数」换成真 API 请求数** —— `storage::read_usage_range` 给 `UsageDay` / `UsageModelTotals` / `UsageHourTotals` 都加 `requests`（取每条记录 `requests[]` 的条数之和；**旧记录没有该数组 ⇒ 按 1 计**，记 0 会像「这天没发过请求」）；③ **右上角新增范围下拉框**（今天 / 近 7 天 / 近 30 天 / 具体日期），**单日 ⇒ 按小时出、区间 ⇒ 按天出**（近 30 天逐小时是 720 行）；面板头（未定价）与表格裁到同一段（`scopedUsageData`），范围跨开关保留。i18n 新增 `cost_col_time` / `cost_col_requests` / `cost_col_hit_input` / `cost_col_miss_input` / `cost_range_{today,7d,30d,dates,empty}`（5 语言），删掉不再使用的 `cost_col_date` / `cost_col_turns`。契约与纪律见 `ai-spec.md` §3.5「定价表与成本面板」+ 规则 63 第十条、`code-rules.md` 预检 **#25 ②⑩**。**验证**：`cargo test --bins` **206 passed**（`usage_range_groups_by_day_and_model` 已扩到钉住 `requests` 在按天 / 按模型 / 按小时三处的累加）；`npx tsc --noEmit` exit 0；`npm run build` exit 0。**待实机验收**。
  5. **面板左上角两格（2026-10-06 二改，用户口径）** —— ① 原「价格」改名 **「最后更新价格日期」**：它的值是价格表文件的 `updated_at`，**不是任何单价**（用户就是被这个标签问住的）；② 新增 **「当前统计的模型」**：数据里出现 ≥2 个模型时是**下拉框**（`总计` + 各模型），只有一个时只显示名字。实现在 `usage-cost.ts` 的 `usageModels()` / `keepModel()` / `modelChipHtml()`，筛选时**日级合计按留下的模型重算**（`UsageDay` 的四类 token 与计数原本跨模型累加 —— 不重算就是「表里只有 A 的行、合计却是 A+B」），某天没有该模型的量则整行不显示；选项取自**未裁剪**的数据、跨开关保留。按钮那条链路**本来就是多模型抓取**（`settings.ts` 把近 30 天用量日志里出现过的模型 + 当前配置模型一起塞进 `{models}`）—— 本轮未改。**未做**：按**对话**（`sessionId`）分组的模型视图 —— 面板按时段统计，与「某个对话里用了哪些模型」不是同一维度，要做需另开后端分组。
- **未做（只剩一步）**：
  1. **跨源基线** —— 拿平台导出的 CSV 跑一次 `reconcile-usage.ps1 -Csv …`，出「本地 vs 平台」的逐小时差。本仓不存供应商账单，需要用户从平台导出一份（一次操作）。
- **验收口径**：任何降本改动按「**每次提问成本**」验收，不许拿「元 / 百万 token」当指标（理由见 `ai-spec.md` §9.1 难点 3 结论 3）。

**L7. AI 聊天独立界面（2026-09-29 用户定；第一版已落地）**

用户原话：「ai 插件也需要独立界面状态，记得将 vs 按钮装入独立界面状态，并去除小窗口和大窗口，
独立界面尺寸设置成与音乐插件相同的」。

- **已完成**：AI 聊天的界面整体搬进**独立无边框窗口**——`pluginId = "chat"`（label `plugin-chat`），
  加载 **`index.html`**，尺寸 **1280×720**、最小 720×420（与音乐默认态同档）；主窗的**内嵌小面板
  （360）与 detached 600 大窗两态都去掉**；**VSCode 按钮**随 `#detached-header` 出现在独立窗标题栏
  （就是原来那个 `#detached-vscode-btn`，`setDetached` 里按 `activePluginId === "ai-agent"` 显示）；
  标题栏 `×` = 关窗。契约、「四点收口」与「不得回退四条」见 `ai-spec.md` §4.8「聊天独立窗」。
- **关键实现选择**（省掉了整块界面的搬迁）：聊天窗与主窗跑的是**同一份 `main.ts`**，靠
  `getCurrentWindow().label` 同步判定角色（**不用 URL query** —— §4.8 规则 4 明令），
  再按角色切开事件与命令的归属：`cli-output` / `cli-status` / `cli-stderr` 归聊天窗，
  `set_ui_mode` / `set_detached` / `set_query_state` / `hide_lunac` / 剪贴板 / 热键 / 搜索归主窗。
  主窗侧所有「进入 AI」的入口收口在 `executePlugin()`（ai-agent 分支）与 `startAIChat()`（开头分流）
  两处 ⇒ 不会再长出内嵌聊天。
- **验证**：`npx tsc --noEmit` **exit 0**；宿主 `cargo check --bins` exit 0、`cargo test --bins`
  **112 passed / 0 failed / 1 ignored**（新增两条守门单测：`chat` 尺寸档、只有聊天窗加载 `index.html`）。
- **待做实机回归**（本仓前端没有自动化 UI 测试）：搜索命中 AI → 开独立窗且主窗不留空面板；
  窗内聊天 / 流式 / 审批卡 / 历史抽屉 / token 仪表盘；`×` 关窗后能从搜索再开；
  **同时开音乐窗与聊天窗**互不串扰（`emit` 是广播，这是最该盯的一条）。
- **已知代价（第一版刻意保留）**：聊天窗会跟着跑主窗那些与全局状态无关的初始化（注册插件 /
  扫盘 / 拉市场索引），功能无害、只是多一次开销；清理属后续优化。

**L8. 应用自更新（2026-09-30 用户定；第一版已落地，未实机验证）**

用户原话：「帮我看一下是否存在安装包如果在本机里如果存在之前的数据可以自动进行替换的逻辑
还有添加软件检查更新和自动更新的逻辑」。这一条把两问合在一起收口。

- **第一问（旧数据自动替换）的结论 = 有一半。已具备**：① 同目录升级是**原地覆盖** —— NSI 核心段
  只做 `SetOutPath` + `File`，**没有** `RMDir /r $INSTDIR`（清数据只在卸载段）⇒ `ModuleData`（会话
  历史）/ `config`（凭据·热键·定价）/ `Modules`（已装插件）/ `temp` 全部保留；② `storage::migrate_legacy_localappdata()`
  把旧 `%LOCALAPPDATA%\Lunac(-dev)` 搬进 exe 根（幂等，搬完删旧目录）；③ `plugin_market::migrate_legacy_plugins_dir()`
  把 `plugins\` 搬到 `Modules\`（幂等，冲突保留新的、绝不删用户文件）。
- **缺的三条已于同日补齐**（都是「自动」二字的洞）：① **装包不认上一版装在哪个目录** —— 没有
  `InstallDirRegKey`、注册表也没写 `InstallLocation` ⇒ 上次装在 `D:\Lunac` 的用户重装会被装到
  `%LOCALAPPDATA%\Lunac`，**变成两份安装、新目录里一份旧数据都没有**；② **安装段不停进程**
  （只有卸载段 `taskkill`）⇒ `lunac.exe` 常驻托盘、被占用时 `File` 写不进去，交互式安装弹
  「Error opening file for writing」、静默安装直接失败；③ **装完不重启应用**。
- **第二问（检查更新 + 自动更新）已落地**：宿主 `app/src-tauri/src/updater.rs`（拉清单 / 版本比对 /
  下载带进度 / sha256 校验 / 拉起 `Setup.exe /S` / 退出交棒），前端「设置 → 常规」的更新区，
  以及启动后 5 秒的静默检查（有新版时在设置齿轮上打一个点）。**链路、三个前置、交棒顺序、
  清单形状与校验口径全部写在 `ai-spec.md` §8.4**，这里不重复。
- **发布侧**：`scripts\publish-release.ps1` —— 算 sha256 → 生成 `latest.json` → `gh release create/upload`
  （`-StageOnly` 只生成清单、不碰 GitHub；本机 gh 在 `%LOCALAPPDATA%\Programs\gh`）。
  **只能传 `build-release.ps1` 新产的包**：`release\` 里 0.4.0–0.9.0 那批含上游 `cli.exe`，
  公开 Release 上绝不能出现（`ai-spec.md` §8.3 红线 4）。
- **未实机验证（如实记）**：只做完静态验证（`tsc` / `cargo test` / 打包产物内含新 CSP 与 pet 勾选段）。
  真机闭环需要**先发一个 Release、再有第二个版本**才验得了：旧版点「检查更新」→ 下载 → 自动重启成新版；
  以及 `autoInstall` 打开后的免打扰路径。
- **已知代价**：① 只做 **sha256、没有签名**（清单本身走 GitHub https，信任根就是 GitHub）；
  ② ~~安装包 110 MB ⇒ 每次更新全量下载~~ **已于 2026-09-30 解决**：PaddleOCR（约 70MB 压缩）
  移出安装包，改成 `ocr` 插件清单里的 `archive` 依赖（装插件时才下到 `Modules\ocr\paddle-ocr\`，
  卸载插件即清），安装包从 **110.3 MB 降到 15.9 MB**（0.9.7 实测）—— 自更新每次的下载量
  随之降到约七分之一（见 `ai-spec.md` §8.2 / §3.5）。

**L9. 音乐插件剩余项（2026-09-30 用户提出）**

用户原话：「现在音乐插件是否只需要一次连接账户 并且在第一次进入音乐插件中 是否有给用户提示说需要连接账户
并查看本地音乐解析参考 potplayer 的 和调音插件 和 连接 qq 音乐和网易云音乐」。前两问**当场查清并如实回答**（结论写在这里，因为答案决定了后两条怎么做）：

- **账户确实只需连一次 —— 已成立，不改**。`music_config_get/set` 落盘的是 `access_token` + **`refresh_token`** + `expires_at`（`app/src-tauri/src/music.rs` 的 `MusicConfig`），`ensure_access_token()` 在快过期（`expires_at <= now + 60`）时自动用 refresh_token 换新的；换回来**可能不带新 refresh_token**，代码刻意保留旧的那份（`music.rs` 注释：「清掉就等于把用户踢下线」）。⇒ 用户只在**第一次**点「连接」走一次浏览器授权。
  - **唯一的例外（已知，不修也不瞒）**：refresh 被 Spotify 拒（用户撤销授权 / 改密码）时 `ensure_access_token` 会清空 `access_token` + `refresh_token`（`music.rs` 401 分支）⇒ 用户得**重新连一次**。这是**正确**行为，不是 bug；但 UI 上只体现成「掉线了」，见下一条。
- **首次进入的连接引导 —— 已闭环（2026-10-01 复核，本条不再有待办）**。未连接时 `#music-setup` 显示、`#music-body` 隐藏（`music.ts` 的 `renderSetup`），块里有「连接 Spotify」主按钮；**说明那一行已落地** —— `#music-setup-hint` 默认写 `music.setup_builtin`（「已使用内置的 Client ID —— 只需点『连接 Spotify』，在打开的浏览器页面里登录你自己的账号即可。控制播放需要 Premium 账号，只看歌词与封面则不需要」），用户点「使用自己的 Client ID」时才切成 `music.setup_hint`（讲 developer.spotify.com 与 Redirect URI）。**掉线路径也已指向那个按钮**：refresh 被拒会清空令牌 ⇒ `cfg.connected` 转假 ⇒ 同一张 setup 卡自动露面（`#music-body` 一起收起），错误文案 `music.err_not_connected` / `music.err_scope` 也都写明「请重新连接 / 请点『连接 Spotify』重新登录一次」。
- **串流质量可以调了，且只有一条路（2026-09-30 用户要求，已落地）**：实测 librespot 0.8 的 `-b/--bitrate {96|160|320}`（默认 160，我们默认给 320）；**官方 Web API 没有任何质量参数**（`/me/player` 连读都读不到），有 `SpPlaybackSetBitrate` 的是 Spotify 的**商业硬件 eSDK**（拿不到）。⇒ 「质量」这件事**只能从本机播放那条路调**，面板上那三档只影响本机播放、且**改动重启后才生效**。契约见 `ai-spec.md` §4.6 第 8 条。**实机回归已做（2026-09-30）**：命令行逐字带 `-c`/`-C`/`-t <cache>\tmp`/`-M 2G`/`--bitrate <所选档>`。**顺带一条部署事实**：音乐插件 09-29 起**已从 bundle 摘出**，跑的是盘上包 `<exe 根>\Modules\music\index.js` ⇒ **改前端源码对已装插件一个字都不生效**（回归第一次开窗根本没有那三档，盘上是 09-29 19:12 的旧构建）——本机要 `scripts\build-plugins.ps1 -Plugin music` + 拷进 `Modules\music\` + 重开窗，**终端用户还要跑一次 `publish-plugins.ps1`**（预检 #43）。**已发布（2026-09-30，用户批准）**：线上 `index.json` 5 条、`music-0.9.8.zip` 与本地构建逐字节一致（sha256 `A0442847…`）；本机 `github.com:443` 直连不通，**要挂代理 `127.0.0.1:7892`**。
- **缓存落点已核实 + 两个缺口已补（2026-09-30 用户要求）**：**不在 C 盘 AppData，无需迁移** —— 三处落点全在 exe 根下（音频缓存与凭据 `config\librespot-cache`、WebView2 profile `temp\webview-data`、日志 `temp\{logs,tool-outputs,transStorage}`），取证四条见 §4.6 第 8 条（`%LOCALAPPDATA%\Lunac*` 不存在等）。顺带纠正：`%LOCALAPPDATA%\Spotify` 那 **21 GB** 是**官方 Spotify 桌面端**的，不是我们的。另补两个真缺口：`-t`（边下边放的临时文件钉在自己目录下）与 `-M 2G`（音频缓存上限，不给就是无上限）；同时清掉 **2 个孤儿 librespot**（同名 `Lunac` 设备占着会话，`NO_ACTIVE_DEVICE` 的成因）。
- **本地音乐解析（参考 PotPlayer）** —— 体量大，**先定范围再动手**（2026-10-01 已按下面这个范围落下**第一刀 = 媒体库那一层**，状态见下）：PotPlayer 那一侧包含 ① 本地媒体库（目录扫描 + 标签 / 封面 / 时长解析）② 播放列表管理 ③ 编解码器与渲染（FFmpeg / madVR 那一层）④ 字幕 / 音轨切换。本项目**只用得上 ①②**（③④ 是视频播放器的事，与「音乐插件」无关）。
  - **PotPlayer 的「依赖」到底是什么（2026-09-30 查，用户问）**：它是**闭源**的（Daum/Kakao，KMPlayer 血统），**没有公开的依赖清单**；从发行形态能确证的只有三块 —— ① 主干是 **Windows 的 DirectShow**（`Module\` 里那批自家滤镜挂在它上面）；② **静态链接了一份精简过的 FFmpeg**（`avcodec`/`avformat`/`swscale`；官方另发 `OpenCodec`，落地形态是 `Module\FFmpeg\FFmpeg64.dll` 与 `FFmpegMininum64.dll`）；③ 硬解与渲染走 **DXVA2 / CUDA / QuickSync** + **EVR / EVR-HQ**（可选 madVR），较新版本字幕用 **libass**，外挂解码可挂 **LAV Filters**。
    ⇒ **这三块我们一块都不借**（它们全属下面那条 ③「编解码与渲染」，L9 明确不做）。我们缺的只有**媒体库那一层**，Rust 里的对应关系：标签 / 时长 / 封面 → **`lofty`**（纯 Rust，覆盖 12 种格式；`read_from_path()` 一次就能拿到 `properties().duration()` 与封面，**不解码**也能把列表填满 —— 这正是「不塞 FFmpeg 全家桶」能成立的原因）；目录扫描 → `walkdir`（要防符号链接环 + 有上限），索引落库用**仓库已有的 `rusqlite`（`bundled`）**，别引第二个存储层；文件类型判定 → 扩展名初筛 + **内容探测**（`lofty` 的 `Probe::guess_file_type()`），不许只看扩展名；播放列表（`.m3u`/`.m3u8`/`.pls`）→ **自己解析**（纯文本，几十行），别为它引库。真到了要解码出声那一步才需要 `symphonia`（纯 Rust；Opus / WavPack 还是 stub）或 `rodio`（内含 cpal + symphonia）—— **2026-10-01 已定案：用 `rodio`**（播放要的是一台状态机：位置口径 / 暂停语义 / seek 饱和 /「放完了」的判据），落在宿主 `player.rs`，理由见 ai-spec §4.6「本地文件播放」。
  - **状态（2026-10-01 更新：①②③ 全部落地，只剩 QQ / 网易云）**：
    **第一批 = 媒体库那一层** —— 宿主 `app/src-tauri/src/media_lib.rs`（目录扫描 + `lofty` 标签/时长/内嵌封面 + SQLite 索引 `ModuleData\music\media.db` + 封面内容寻址缓存 `temp\music-covers\` + 后台线程扫描与进度快照），音乐插件里多了「本地音乐」页（添加 / 移除目录、扫描进度、曲目表、筛选），宿主命令 `media_roots` / `media_add_root` / `media_remove_root` / `media_scan_start` / `media_scan_status` / `media_tracks`。**已实测**（dev 实例 + CDP 探针 + 真目录）：480 首的库一次轮询内入库、增量重扫 0 新增 0 更新、`LIKE` 转义成立（搜 `%` 只命中标题里真有 `%` 的那条）、内嵌封面经 asset 协议正常显示 —— 逐条见 ai-spec §4.6「本地媒体库」与预检 #47。
    **第二批 = 本地文件的播放**（用户 2026-10-01 明确纠正落点：**进音乐插件、不是进 `tuning-engine`** —— 后者是调音 DSP）—— 新模块 `app/src-tauri/src/player.rs`（`rodio` 状态机）+ 七条 `player_*` 命令 + 音乐插件「本地音乐」页底部的本机播放条（上一首 / 播放暂停 / 下一首 / 停止 / 进度拖动 / 音量），队列留在前端。生命周期与 librespot 同一条纪律（关窗带走、离开本地页也带走）。契约与五个决定见 ai-spec §4.6「本地文件播放」与预检 #49。
    **第三批 = 播放列表解析** —— 宿主 `media_playlist` 读一份 `.m3u` / `.m3u8` / `.pls`（**不入库**，只读那个文件；编码认 UTF-8 / UTF-16 BOM / **ANSI-GBK**，相对路径按列表所在目录解析并剥 `file://`，坏行只跳过自己），前端在左栏加「打开播放列表」，结果当作主区那批曲目用（点播 / 队列一行都不用改）。
    **~~仍未做：连接 QQ 音乐 / 网易云~~ —— 2026-10-06 用户决定「取消 L9」，这一条不再排期**（原卡的授权口径问题见本节末段；若要重开，先把那一段读完）。**L9 至此收口**：①②③ 三批（媒体库 / 本地播放 / 播放列表）均已落地并实测；串流质量三档、连接引导、缓存落点三个缺口也已闭环。
    **注意别把 `source = "local"` 误读成本地解析** —— 那是「用本机 librespot 当播放设备」（`music_autoconfigure`），与本地文件无关。视频那侧只有 `convert` 插件的**格式转换**（ffmpeg/ffprobe 读时长与有没有音轨），而 **ffmpeg 不随包分发**（只在 `exe根\ffmpeg\bin` 或 PATH 找）—— 那不属于「解析」。
  - **一条已排除的路（2026-09-30 实测，别重复查）**：librespot 0.8 自带 `-l/--local-file-dir`，但**不能用它替掉本地解析** —— 实测（12 秒 + `RUST_LOG=debug`）日志里对它扫到的那些文件**一个字都没提**（只有 9 行：Session / Zeroconf / softmixer / rodio S16 / WASAPI 设备名），它服务的是「**别的** Spotify 客户端来播本机文件」那条路，控制权在官方客户端手里，我们要的媒体库 UI 与它无关。
  - **落点**：`Modules\music\`（与 librespot 同层），解码沿用系统已有能力 —— **不要**把 FFmpeg 全家桶塞进插件。音频标签读取优先用已有依赖能力，读不到再退回文件名。
  - **与现有链路的关系**：本地文件播放**不进 Spotify Web API**，因此不能复用 `spotify_play_*`；但**可以**复用 librespot 那条「本机播放」的设备选择与音量链路（`music_autoconfigure` 的 `source = local`，已落地）。
  - **必须与 L5 调音插件串起来设计**：本地解析出来的音频最终要能过 DSP（L5 的 S1「自身播放闭环」正是为它准备的）——别做两套播放内核。**2026-10-01 定下的形态**：宿主**把 `tuning-engine` 当库依赖**（它是 `[lib]` + `[[bin]]`，`Chain` 就在库里）在 `player.rs` 的 `append()` 之前过一层，**不是**把播放塞进 engine、也**不是**另写一份滤波器。本轮先直通、不加链（先焊死一条链等于替调音插件那一期做了决定）—— 见 ai-spec §4.6「本地文件播放」末段与预检 #48 ⑦。
- **~~连接 QQ 音乐 / 网易云音乐~~ —— 2026-10-06 用户决定「取消 L9」，**不做**（存档，防重复讨论）**：技术上可行，但两家**都没有** Spotify 那种公开的第三方 OAuth + Web API（网易云的 `api` 类项目基本都是**非官方接口 + 抓包 Cookie**，QQ 音乐同理）⇒ 真要接等于内置一套**随时可能失效、且违反对方 ToS** 的私有接口。用户已拍板**不做**，本节只留作结论：（若将来重开，路径仍是「只做歌单 / 搜索 → 落到本地解析播放」，**不做**取流播放；且优先级排在本地解析之后。）

**L10. 桌宠 × AI（2026-09-30 用户提出）**

用户原话：「以及桌宠的屏幕获取 和桌宠内置到 lunac 的 ai agent 以做出反应 并且其桌宠的 tokens 消费作为一个独立的表格列出」。三条互相依赖，**必须一起设计**（桌宠「看到屏幕」才有得反应，有反应才有那块 token 账）。

- **① 屏幕获取**：宿主已有 `windows_ocr.rs` / `paddle_ocr` 那条「截图 → 识别」链路可复用（至少截图那半段现成）。**要做的是**：① 定「截什么」（全屏 / 某显示器 / 某个窗口 / 只截活动窗口）；② **定频与停的条件**（桌宠最小化 / 隐藏时必须停 —— 宿主已经会下发「可见吗」，见 `ai-spec.md` §4.8，直接挂上去）；③ **隐私红线**：截屏内容**默认不上传**，只在用户显式开启「看屏幕」时按需截、且**必须在界面上有可见标识**（用户知道它在看）。
- **② 内置到 Lunac 的 AI agent 以做出反应**：形态建议 = 桌宠**不自己起 agent**，而是往**现有聊天窗/agent 那条链路**发一条用户可见的消息（于是工具、审批、用量记录、历史会话全部自动复用 —— **不要**给桌宠单开一条 API 通道，那会造出第二份「agent 在哪、花多少钱」的真相）。桌宠侧只负责：触发条件（定时 / 用户交互 / 屏幕变化）+ 表情动作的反应呈现。
  - **成本红线**：这条链路会**持续烧 token**。默认必须是**关**，且要有**频次上限**（例如最小间隔 + 每日上限），触顶后自动停并如实告知。
- **③ 桌宠 tokens 消费的独立表格**：`usage-*.jsonl` 已有 `sessionId` 归因（A11）与按天聚合（`usage-cost.ts` + 成本面板）。**要做的是给它一个来源维度**：桌宠那一路的请求必须带上可区分的归因（**首选**复用 `sessionId` 前缀或新增一个来源字段，**不要**靠「时间戳猜」），然后成本面板里加一块「桌宠」表格（同一份 `usage-cost.ts` 算法，**禁止**另写一套金额计算）。
  - 依赖顺序：**先有 ②（且真的带归因）**，③ 才有数据可列。

**M1. 两个待实测项（不阻塞开发）**

| # | 项 | 说明 |
|---|---|---|
| **M1-1** | 前端 CDN 与 WebView2 冷启动在**开机场景**的真实占比 | 现象侧已定论（开机自启慢的根因是 OS 触发时机 + 计划任务权限，见 `ai-spec.md` §9.1 难点 2）；此项只差「实测确认 CDN/冷启动无关」后写死结论，不再靠猜 |
| **M1-2** | 缓存命中率唯一遗留疑点：复现 `in=2287 / read=0` 那一轮 | 「落盘慢」已被 0ms vs 6s 对照实验**证伪**（两组逐字节相同、均 97.9%）。剩余两种可能：① 前缀本身变了（`skills::listing()` / 工具黑名单 / `cwd` 任一变化）；② 供应商侧缓存被清。**工具已就绪** —— 用 `result.usage.requests[]` 与启动时落盘的 `固定前缀 …` 行前后对照即可定位 |

**M2. 已实现、但还差一次真机验收**

> 2026-09-29 从本文末尾的历史记录里搬出来的 —— 它们原先写在「未验证项（如实记）」里，随历史一起删掉就没人记得了。

| # | 项 | 差哪一步 |
|---|---|---|
| **M2-1** | 卸载时随 NSIS 清理插件目录 | `nsis-hooks.nsh` 已加 `RMDir /r "$INSTDIR\Modules"`（旧的 `$INSTDIR\plugins` 也在），**没重打包验证**。注意 `ai-spec.md` §6 的「卸载清理」清单里没列 `Modules` —— 验完顺手补上 |
| **M2-2** | 开机自启守卫的真机闭环 | 现在做到的是「开发构建拒绝注册 + 能查出旧注册项 + 给一次修复入口」；**要安装版在登录时触发才算数**（与 M1-1 的「开机 CDN / 冷启动占比」不是一件事） |
| **M2-3** | 任务抽屉与语义色按钮的视觉 / 手感 | 抽屉在真实对话里的显隐、**待办与已更改文件两个列表同时展开**（2026-09-30 起不再互斥）、语义色按钮没被盖掉 —— **未做浏览器实测**（回归口径见 `ai-spec.md` 规则 45 与 `agent-ui-spec.md` §8） |
| **M2-6** | 中断后「飞行中」的命令卡停在「执行中」 | **代码已完成（2026-10-03）**：三处收尾都调 `markFlyingCardsStopped` —— ① `stopAIChat()` 在 `stop_cli` 之后（覆盖「停止」按钮与队列「立即发送」，后者走 `sendQueuedNow` → `stopAIChat({drainQueue:false})`）；② `closed` 事件（崩溃退出）；③ 回退路径。**只改视觉、不置 `card.done`**（真相仍由 `tool_result` 说了算）。**待真机确认**这三条路径的卡片都变黄「已停止」 |
| **M2-7** | 2026-10-01 那一批（内置技能 / AGENTS.md / MCP 远端 / 快照落盘）只做了静态验证 | 四条都过了 `cargo test`（core-agent 122 / 宿主 172）与 `tsc`。**① 内置技能与 ② `AGENTS.md` 已于 2026-10-05 用 agent 侧端到端验收通过**（真端点 + `D:\Lunac\agent.exe`）：① 启动日志 `技能 4 个: [code-review,commit,debug,stuck-guard]`、`tools=16个`（15 内置 + `Skill`），模型在「审查这次改动」时真的调了 `Skill{skill:code-review}` → fork 出子代理、`task_started`/`task_progress`/`task_done` 事件齐全、`allowed-tools` 收窄到 `Read,Cmd,Glob,Grep` 生效；② 日志 `项目记忆已注入系统提示词固定段：163 字（…\proj\AGENTS.md）`，模型回答里的构建命令与 `AGENTS.md` 内容逐字一致。**③ MCP 远端与 ⑤ MCP OAuth 已于 2026-10-05 由 agent 侧端到端验收通过**（夹具已入库：`scripts/mcp-test-server.mjs` —— Streamable HTTP + RFC 9728 受保护资源元数据 + RFC 8414 授权服务器元数据 + RFC 7591 动态注册 + Authorization Code + **PKCE(S256)** + refresh_token；授权端点**自动同意**，因此整条链路可无人值守跑完，跑法见该文件头部注释）：**③** `system/init` 工具池含 `mcp__echo`，模型真调该工具、服务端回 `echo: hello-from-lunac-e2e` 并逐字回到模型；**⑤** 首次授权日志 `MCP OAuth：请在浏览器里完成授权（最多等 180s）` + 令牌落盘（`mcp-tokens.json` 含 `access_token` / `refresh_token` / `token_endpoint` / `client_id` / `resource`），**重启后 `authorize:0 / register:0`（没再开浏览器）**、服务端不认旧 access_token ⇒ `401` → 客户端**只刷新**（`refreshed:1`，全程无 `/authorize`）→ 原样重发成功、`tools/call` 计数 +1，刷新后的令牌也回写落盘。**仍未做**：④ 快照落盘（要 UI 多会话回退）—— 这条要真机 UI，不是 agent 侧能单独验完的 |
| **M2-8** | A17 结果卡上的凭据告警只做了静态验证 | `cargo test` **130 通过**（新增 `security_warnings_only_cover_written_payloads`）+ `tsc` exit 0，但**没有真机闭环**：让模型 `Write` 一段含 `-----BEGIN RSA PRIVATE KEY-----` 的内容（审批卡上会先出现 🔑），批准后**结果卡**上应多出一块红色「写入的内容里可能含凭据」+ 规则 / 行号列表、且卡片**自动摊开**；同一次 `Edit` 把凭据**删掉**时那块**不该**出现（只扫 `new_string`）。口径见 `ai-spec.md` §11 规则 74 / §13.1 |

> 同一段历史里还有一条**已经关闭**的：插件 ESM 走 asset 协议的真机链路 —— **2026-09-30 修正**：此前记的「已跑通」只在 **dev** 成立（dev 下 HTML 来自 Vite，Tauri 不注 CSP），**安装版一直是坏的**：CSP 的 `script-src` 只写了 `https://asset.localhost`，而 Windows 上 asset 协议的 origin 是 `http://asset.localhost` ⇒ 动态 import 被拦、报 `Failed to fetch dynamically imported module`。同日已修（`tauri.conf.json` 的 `default-src` / `script-src` 补 `http://asset.localhost`），口径见 `ai-spec.md` §3.5「加载通道」。**仍未在安装版实测过**，别当成已验证。

**L11. 本地「只参与决策」的模型调研（2026-10-05，用户问「市面上是否有只参与决策的本地模型」）—— 结论：有，2026 已是独立品类**

- **「决策模型」（decision models）**：不生成正文、只做**选择题 / 打分 / 分类**，输出结构化的 `choice` + 概率 + 置信度。
  - **TypeSafe 的 Jev API + firelex 的 Jeff**（MIT 代码 / Apache-2.0 权重；0.8B / 2B / Gemma4-E2B）：zero-shot 分类，单次决策约 **22–28ms**（RTX PRO 6000 / M4 Max），明确**不做多步推理**。
  - **Ollama 0.35+ 内置 `/v1/systemone` 决策端点**，三款模型：`nimble`（Bespoke Labs 9B）、`tev1`（Together 4B）/ `tev1:0.8b`；实测 **91ms/决策**（M5 Max 本地跑）。适用场景官方点名的就是 **ticket 分级 / model routing / 内容审核** —— 与「只参与决策」完全对上。
- **纯 tool-calling 小模型**：**Cactus Compute Needle 2**（45M 参数、量化后 **14MB** 单二进制、~28MB 内存、Pi 5 上 ~500 tok/s）—— 只做工具调用 / 设备控制 / 结构化抽取，**不对话**；带「置信度不够就升级给大模型」的门控。这是「决策层」最极端的形态。
- **「小模型自己知道何时 defer」**：distil labs 把 `defer_to_larger_model` 当**一个工具**训进 Qwen3-1.7B（不另建 router，96% 的路由留给小模型、4% 难的升级给大模型）。学术侧有 **xRouter**（RL 训练的 cost-aware 路由）。
- **通用小模型的工具调用可靠性（别混）**：可靠线在 **Gemma 4 27B / GLM-4.7 32B / Qwen3 32B / Qwen3-Coder 30B / Llama 3.3 70B**（Q4_K_M 是下限）；**7B 以下、且没专门训过 tool-call 的通用模型会吐畸形调用** —— 要 7B 以下就必须选**专训**的（decision model / Needle 这类）。
  - **对 Lunac 的落点（存档，不排期）**：可行形态 = 把「决策层」拆到本地小模型（跑 Ollama，走 `nimble`/`tev1` 或专训 tool-caller），只负责**路由 / 工具选择 / 要不要继续 / 何时重问**；生成仍交给云端大模型。**两个前置**：① 本仓 agent 主循环的契约是 Anthropic 形态的 `stream-json` + tool schema，接本地决策模型要一层适配；② 「决策」与「生成」分离会改变每次请求的**固定前缀**，直接影响缓存命中（见 M2-16）。所以先只作调研结论。

**L12. 插件体系的两种扩展（2026-10-06 讨论定方向，待实施）**

用户提出「插件能否拓展出其他应用类型（直接二进制进程 / 不依赖 WebView）」，真实动机是「仿成熟软件的前端时，在现有框架下要手搓很久」（当时正在做代理面板的多页重构）。

- **先纠正一个过度约束**：`agent-ui-spec.md` §0 的「不引入 UI 框架 / 图标库」是**宿主**的硬边界（它自己写明只管 `#results-list` 内部与输入栏控件的增删），**管不到插件**。`agent-templates/modules/README.md` 从未禁止插件引 UI 框架 ⇒ **插件自带框架（bundle 进 `index.js`，或走 `dependencies[{type:"npm"}]`）今天就能做**，不必改宿主。卡点不是 WebView2（Chromium 能跑任何现代前端），是「手写 CSS + 不引依赖」那套**宿主纪律被误套到插件上**了。
- **三档，代价差一个数量级**：
  - **档 0（今天可做）**：插件自带 UI 框架，仍跑在 WebView 里。代价：产物变大（React ≈ +150KB，按需加载可接受）；**自绘 UI 不跟宿主主题变量 ⇒ 视觉可能割裂**；npm 依赖要求用户机器有 Node.js。
  - **档 1（需新契约）**：插件带**二进制 sidecar**，宿主负责起进程 + stdio / 命名管道通信，UI 仍是 WebView。代价：`permissions` 那套「声明 + 告知」**降级**（能 spawn 任意二进制的插件能力无上界）⇒ 要给**独立信任门**（同 MCP 的 `.mcp.json` 那条路）；宿主多一套进程生命周期管理。
  - **档 2（建议不做）**：插件 = 独立进程 + 自己的窗口、完全脱离 WebView。代价：宿主变「通用窗口 + 进程管理器」，`Modules` 那套「一个目录、一条扫描、卸载即彻底消失」的简单性赔掉；安全上等于完全信任。真需要原生窗口观感，现成的 `window.float` + `chrome:false` 已够（桌宠就是这么做的）。
- **待实施的两步（用户 2026-10-06 确认顺序，先记待办、暂不动手）**：
  1. ~~把「**插件可自带 UI 框架**」明确写进 `agent-templates/modules/README.md`~~ **✅ 2026-10-06 已落地**：新增 §4.4「自带 UI 框架 / 第三方库（**允许**）」+ 头部那句「不需要前端构建环境」补一行例外（自带框架 / TS 才需要自己的打包步骤）。要点：宿主那条「不引 UI 框架 / 图标库」（`agent-ui-spec.md` §0 第 3 条）**只管宿主面板**（§0 开头已限定作用域 = `#results-list` 内部 + 输入栏控件），**管不到插件**；两条路 = 打进入口（推荐，零外部依赖）/ `dependencies[{type:"npm"}]`（要求用户有 Node.js）；四条注意 = 对齐宿主主题变量 / `attach(root)` 里 mount、`detach()` 里 unmount / 仍禁 CDN `import()`（CSP）/ 多插件不去重。
  2. ~~单独设计**档 1 的 sidecar 契约**（清单字段 + `permissions` + 信任门 + 宿主命令），用户确认后再动代码~~ **✅ 2026-10-06 契约定稿并已实施**（用户拍板「宿主代请求」方案）。落地清单：
     - **清单**：`plugin_market.rs` 新增 `SidecarSpec` + `PluginManifest.sidecar` + `validate_sidecar`（**必须同时有 `process.spawn`**，`command`/`cwd` 复用 `safe_join` 不得越界，枚举校验，`env` 只增不删，`sha256` 64 hex，`allowLocalPorts` 上限）+ `KNOWN_PERMISSIONS` 加 `process.spawn` + `InstalledPlugin.sidecar` 透出。
     - **新模块 `app/src-tauri/src/plugin_sidecar.rs`**：进程表（按插件 id 唯一）、NDJSON 编解码、`ready` 握手（10s）、`request`/`http`/`stop`、`restart=on-failure` 惰性重起（≤3 次）、**信任门** `config\plugin-trusted.json`（指纹 = sha256(id+command+sha256) 前 8 字节；读失败一律当未信任）、**面板绑定**（`on_plugin_window_destroyed(label)` / `kill_all()`）、`child_job::assign` 兜底。
     - **宿主代请求**：`plugin_sidecar_http` 由宿主连 `127.0.0.1:<port>`（插件 JS 从不碰端口，**CSP 不放宽**）；目标缺省只放行自己的端口，其他端口要 `allowLocalPorts`/`allowLocalAny` 且过信任卡；每次请求落审计日志。
     - **命令**：`plugin_sidecar_start/request/http/stop/running/trust/deny`（`main.rs` 注册）。
     - **前端**：`plugins/sidecar.ts`（桥实现 + 信任卡 + 活动插件绑定）、`host.ts` 加 `LunacSidecarApi`（`apiVersion` 1→**2**）、`main.ts`/`plugin-window.ts` 注入并在面板开关时起/收、`market.ts`/`registry.ts` 透出 `sidecar`（`autostart`）、i18n 五语言 9 条。
     - **安全收口**：`commands.rs` 新增 `validate_public_https_url`（https + 拒 IP 字面量/内网主机名），`install_skill_from_url`/`download_tool_from_url`/`install_plugin_from_url` 三处统一过它；`uninstall_plugin` 先收 sidecar 再删目录。
     - **文档**：`agent-templates/modules/README.md` 新增「自带本机程序：`sidecar`（进阶）」+ `process.spawn` 能力行。
     - **验证**：`cargo check` 通过；`cargo test --bins` **206 passed / 0 failed / 2 ignored**（新增 `plugin_sidecar` 2 条 + `plugin_market` 1 条）；`npx tsc --noEmit` exit 0；`npm run build` 通过。
     - **如实记（未做/边界）**：`restart=on-failure` 是**惰性重起**（下一次调用时探测并重起），不做后台守护线程；`cwd` 只允许插件目录内；不做无面板常驻（契约 ⑥）。

#### sidecar 契约草案（2026-10-06，**待用户裁决，未动代码**）

**一、它到底解决什么问题**

今天的插件**只能活在 WebView 里**：`execute(input)` 产出 HTML、`attach(root)` 挂交互，能用的是宿主桥 + `@tauri-apps/api`。这够做「面板型」插件（番茄钟 / 翻译 / 备忘录 / 代理面板），但**做不了「背后要跑一个本机程序」的插件**。而本仓的**内置插件早就在这么干了** —— 只是那份能力编在主程序里，磁盘插件够不着：

| 内置插件 | 它跑的本机程序 | 谁起的进程 |
|---|---|---|
| 音乐（Spotify Connect） | `librespot.exe`（自己解码出声） | `music.rs`（宿主代码） |
| 文件转换 | `ffmpeg` / `ffprobe` | `convert.rs`（宿主代码） |
| OCR | PaddleOCR | `paddle_ocr.rs`（宿主代码） |
| 代理 | `mihomo` 内核 | `mihomo.rs`（宿主代码） |

**sidecar 就是把这一环开放给磁盘插件**：插件在自己的目录里带一个**二进制**，宿主负责「起进程 + 通信 + 收尾」，**不必再为它改主程序** —— 这是「插件不依赖宿主代码」那条纪律的自然延伸。

**二、有了它，插件能做什么**（WebView 里做不到 / 做不好的）

1. **跑成熟 CLI / 引擎，不重造轮子**：ffmpeg（转码）、yt-dlp / aria2（下载）、ripgrep / tree-sitter（代码索引）、7z（压缩）、ImageMagick（批处理）、llama.cpp（本地大模型）—— 插件只写 UI + 编排，重活交给现成二进制。
2. **拿到 WebView 没有的原生能力**：串口 / USB / 蓝牙 / 摄像头底层、系统托盘与全局钩子、注册表、`ReadDirectoryChangesW` 文件监视、WMI / 性能计数器。
3. **用别的语言 / 生态**：Python（numpy / pandas）、Go、Rust crate、Node 原生扩展 —— 不被 JS 束缚。
4. **把重活搬离 UI 线程**：WebView 里跑重 JS 会卡界面；sidecar 是独立进程，吃满 CPU / 多线程 / GPU 也不拖累 UI，还能做「进度 / 可取消 / 断点续传」。
5. **大文件 / 高吞吐**：流式读写、文件锁、GB 级数据处理（WebView 的 WASM 受内存与线程限制）。
6. **与本机已装软件集成**：走它们的 CLI / 本地 socket / COM，读写它们的配置。

**典型插件例子**：下载管理器（aria2）、视频转码（ffmpeg）、本地大模型对话（llama.cpp server）、串口调试台、代码语义搜索、系统监控面板、**Spotify Connect 客户端**（内置音乐插件那种，现在磁盘插件也能做）。

**三、它不是什么**（边界，先说清）

- **不替代 WebView** —— UI 仍在 WebView 里渲染（那是「档 2」，已建议不做）；sidecar 只是插件背后那台「引擎进程」。
- **不是沙箱** —— sidecar 是完整本机进程，能力 ≥ 插件代码本身；信任卡是「如实告知 + 用户裁决」，不是隔离。
- **不是无人值守后台** —— 已选「必须绑定面板」：面板一关就收进程（与预检 #57 / `child_job.rs` 的既有纪律一致）。
- **不能免信任** —— `process.spawn` 必须过信任卡（默认拒绝）。
- **不要求用户装环境** —— 二进制跟插件走（`dependencies[{type:"file"}]` 下载 + sha256 校验），不要求用户有 Python / Node。

**四、契约**

**① 清单新增字段 `sidecar`**（`lunac-plugin.json`）：

```json
{
  "permissions": ["process.spawn"],
  "sidecar": {
    "command": "bin/mytool.exe",
    "args": ["--host", "127.0.0.1"],
    "transport": "both",
    "port": 0,
    "protocol": "ndjson",
    "cwd": "",
    "env": {},
    "autostart": false,
    "restart": "on-failure",
    "sha256": "9f2c…"
  }
}
```

| 字段 | 必需 | 说明 |
|---|---|---|
| `command` | ✅ | 相对插件目录的可执行文件路径。**不许绝对路径 / 不许含 `..`**（判据与 `dependencies.dest` 同款）。可与 `dependencies[{type:"file"}]` 配合（先下载、校验后再起） |
| `args` | | 字符串数组，**直接传给 `CreateProcess`，不经 shell** —— 避免注入 |
| `transport` | | 通道：`stdio`（缺省）/ `http` / `both`，见 ③ |
| `port` | | 仅 `http` / `both` 用。`0`（缺省）= 由 sidecar 自选并经 stdio 握手行上报；非 0 = 写死该端口 |
| `protocol` | | 消息协议，缺省 `ndjson`（每行一个 JSON 对象，与 agent 的 stream-json 同风格）。首版只认这一种 |
| `cwd` | | 工作目录，缺省 = 插件目录。**限制在插件目录之内**（2026-10-06 用户裁定） |
| `env` | | 附加环境变量（在宿主环境之上**只增不删**） |
| `autostart` | | 宿主加载插件时是否自动起，缺省 `false`（插件调 API 时才起） |
| `restart` | | 崩溃重启策略：`never`（缺省）/ `on-failure`（最多 3 次、指数退避） |
| `sha256` | | 可执行文件校验，可选但**强烈建议**（与 `dependencies.sha256` 同款） |

**② 新权限 `process.spawn` + 独立信任门**（关键安全项）：

- `permissions` 里新增一条 `process.spawn`，与 `window.float` / `layout.takeover` / `window.resize` 同级。
- **与现有 `permissions` 的「声明 + 告知」模型不同**：能 spawn 任意二进制的插件，能力**无上界**，不能靠静默声明放行。带 `process.spawn` 的插件**首次启用时弹一张专用信任卡**（列出 `command` / `args` / 来源 / `sha256`），三个答案：**仅本次 / 始终信任 / 拒绝**。
- 信任记录落 `config\plugin-trusted.json`，指纹 = **插件 id + `command` + `sha256`**（文件一变 ⇒ 旧信任失效、重新弹卡）—— 与 MCP 的 `.mcp.json` 指纹门（预检 #66 ②）同款。
- **默认拒绝**：`process.spawn` **必须显式信任**才生效（不像 `window.*` 那样「声明即生效」）。

**③ 通信通道（两条，可单用可并用）**：

| `transport` | 通道 | 说明 |
|---|---|---|
| `stdio`（缺省） | 宿主 ↔ sidecar 走 stdin / stdout 的 NDJSON | 无需端口、无需握手配置；stderr 原样落盘日志 |
| `http` | sidecar 在 **`127.0.0.1:<port>`** 上监听 | `port = 0`（缺省）⇒ sidecar 自选并经 stdio 握手行 `{"method":"ready","params":{"port":12345}}` 上报；也可在清单里写死 |
| `both` | 两条都开 | 控制信令走 stdio、大流量走 http（如本地大模型的流式输出） |

- **握手**：宿主起进程后必须先收到一条 `{"method":"ready", …}`（`http` 时带 `port`）才算「起好了」；超时（默认 10s）判失败、按 `restart` 处理。
- **消息形状（借 MCP 风格）**：插件→sidecar `{"id":1,"method":"…","params":{…}}`；回包 `{"id":1,"result":{…}}` 或 `{"id":1,"error":{"message":"…"}}`；sidecar 主动推 `{"method":"event","params":{…}}`（无 `id`）。
- **⚠️ CSP 约束（决定「谁去连端口」）**：前端 CSP 的 `default-src` 是 `'self' http://asset.localhost https://asset.localhost`（[tauri.conf.json](file:///d:/cc/claude-code-cli-master/app/src-tauri/tauri.conf.json#L35)），**不含 `127.0.0.1`** ⇒ **插件 JS 不能直接 `fetch()` 这个本地端口**。所以 **http 由宿主代请求**：插件调 `host.sidecar.http({method, path, body})` → 宿主发 HTTP → 回给插件（流式经 `event` 推）。**CSP 因此不用放宽**。想让插件**直连**端口，只能在 CSP 加 `connect-src http://127.0.0.1:*` —— 那是对**全前端**放宽（任何页面都能连本机任意端口），默认**不做**，真要做需单独论证。

**④ 插件侧 API**（经宿主桥 `__lunac_host.sidecar`，与 `t()` 同一处）：

- `await host.sidecar.start()` —— 起进程（未起才起；未信任 ⇒ reject）
- `await host.sidecar.request(method, params)` —— 走 **stdio** 发一条、等回包（默认超时 30s）
- `await host.sidecar.http({ method, path, body })` —— 走 **http** 端口**由宿主代请求**（仅 `http` / `both`）；`stream: true` 时用 `on("http-chunk", …)` 收流 —— **插件 JS 自己从不碰端口**（CSP，见 ③）
- `host.sidecar.on(event, cb)` —— 订阅 sidecar 主动推（stdio 的 `event` / http 的流式分片）
- `await host.sidecar.stop()` —— 收进程

**⑤ 宿主命令（Tauri）**：`plugin_sidecar_start` / `plugin_sidecar_request` / `plugin_sidecar_stop`（按 `plugin_id` 寻址），需登记进 `capabilities/default.json`（见 code-rules §2.2）。

**⑥ 进程生命周期（硬约束，接预检 #57）**：

- **必须随插件的退出而终结**：面板关闭 / `detach()` / 卸载 / 宿主退出 ⇒ sidecar 一律被收掉（沿用 #57 那条「插件起的子进程随插件退出而终结」的既有纪律）。
- **每插件同时最多 1 个实例**（重复 `start()` 复用已在跑的那个）。
- 崩溃按 `restart` 策略处理；`never` 时把退出码如实报给插件（不静默假装成功）。
- **首版要求「有一个打开的面板」才允许起**（`window.float` 或结果区），面板一关就收 —— 避免「看不见的常驻进程」。

**⑦ 安全边界（如实记）**：**不是沙箱** —— sidecar 是完整本机进程，能力 ≥ 插件代码本身。信任门是「如实告知 + 用户裁决」，不是隔离。UI / 文档沿用既有口径，**不许自称「沙箱」**（同 ai-spec §11 规则 21）。

**⑧ 已裁决 / 仍待裁决**（2026-10-06）：

| 项 | 结论 |
|---|---|
| 能否无可见面板常驻 | ✅ **必须绑定面板**（面板一关就收进程；与预检 #57 / `child_job.rs` 一致）—— 用户裁定 |
| `cwd` 边界 | ✅ **限制在插件目录内** —— 用户裁定 |
| 通信通道 | ✅ **stdio + 本地端口**（`transport` 三档）—— 用户裁定；落法 = **端口由宿主代请求、CSP 不放宽**（见 ③） |
| 目标范围授权 | ✅ **默认只放行该插件自己的 sidecar 端口**；要串本机其他应用 ⇒ **清单声明 + 信任卡由用户批准**，模型只「提议」不「自决」（见 ⑨） |
| `env` 边界 | ✅ 默认继承宿主环境、**只允许附加**（不允许删除） |
| **契约整体定稿并进入实现** | ✅ **2026-10-06 用户拍板：按「宿主代请求」方案实施** |

**⑨ 目标范围 = 数据 + 用户授权（不是代码 + 模型自决）** —— 用户 2026-10-06 明确：

- **为什么不让模型自行拓宽**：① CSP 是**打包期配置**（[tauri.conf.json](file:///d:/cc/claude-code-cli-master/app/src-tauri/tauri.conf.json#L35)），改它要重新构建发版，不是运行时能拓宽的；② 「模型自决边界」= 取消信任门（这道门的全部意义就是**由用户裁决**）；③ 它本身就是一条**提权通道** —— 恶意插件 / 提示注入可诱导 agent「去拓宽一条」。
- **正确形态**：缺省只放行该插件**自己上报的那个 sidecar 端口**；需要串其他本机应用时，清单声明 `allowLocalPorts: [11434]` 或 `allowLocalAny: true` ⇒ **在信任卡上如实列给用户** ⇒ 用户批准后才生效、落 `config\plugin-trusted.json`。**模型能做的是「写这个声明 / 拉起这张卡」，不是自己改宿主。**
- **严格优于放宽 CSP**：按插件（不是所有插件 + 整个 UI）/ 可撤销（收回信任即可）/ 可审计（宿主记每一次请求）/ **不动 CSP**。
- **顺带收口**：现存 `install_skill_from_url` / `download_tool_from_url` / `install_plugin_from_url` 是「任意 URL、无 https 校验」的旁路（等于绕过 CSP 触达 loopback）⇒ 实施时一并改成 **https-only + 拒绝 loopback / 内网地址**，此后「连 loopback」只保留上面这一条**经授权**的通道。

**OCR 截屏识别（2026-10-06，闭环记录）** —— 用户提「OCR 插件可能需要截图功能」。**先判定了现有插件要不要 sidecar：一个都不要**（music/convert/ocr/proxy 都已经用**宿主代码**起各自的进程，宿主代码对我们自己的二进制严格更好：参数固定在代码里、不需要信任门、收尾已实测；sidecar 是给「改不了主程序的第三方 / AI 自建插件」用的）。截图同理走**宿主**，不走 sidecar。

- **新增通用宿主能力** `app/src-tauri/src/screenshot.rs`：抓整块**虚拟桌面**（多显示器合并）。选型 **GDI `BitBlt`**（无 COM/WinRT，`windows` crate 只加 `Win32_Graphics_Gdi` + `Win32_UI_WindowsAndMessaging` 两个 feature，**不新增下载**）；`CAPTUREBLT` 让分层窗口一起抓；`GetDIBits` 取 32bpp 顶朝下像素 → BGRA→RGBA（**alpha 拉满**，GDI 的 alpha 常是 0）。**如实记**：受 DRM 保护的内容会是黑的（要按窗口/按显示器抓帧得换 WinRT 那条路）。
- **⚠️ 二改（2026-10-06，用户口径「改成 ShareX 那种截屏形式」）**：一版是「宿主 `capture_screen()` 回整屏 data URL → OCR 在**插件自己的 WebView 里**铺一层 `position: fixed; inset: 0` 的 overlay 拖框 → canvas 裁剪」。**这条路走不通**：OCR 是 `layout.takeover` 型（面板内嵌在**主窗**里），overlay 只盖得住主窗 ⇒「全屏框选」实际等于「Lunac 窗内框选」，且整屏截图要先缩进窗口才显示（4K 上精度也差）。**先调研 ShareX**：它不用系统「截图」工具、也不用剪贴板，而是**自己开一扇覆盖整个虚拟桌面的无边框置顶窗体**（`RegionCaptureForm`，`TopMost` + 半透明遮罩）再 GDI 抓帧铺上去（区域截图用 `BitBlt`，屏幕录制才换 Desktop Duplication）⇒ 复刻这条路。
- **落地方案（宿主级覆盖窗）**：命令四条 —— `screenshot_overlay_begin`（抓帧 → 存**会话态** → 开 `shot-overlay` 窗）/ `screenshot_overlay_image`（覆盖页取背景图）/ `screenshot_overlay_finish(x,y,w,h)`（按**像素**矩形裁剪）/ `screenshot_overlay_cancel`；结果走**事件** `screenshot-overlay-done` / `screenshot-overlay-cancelled` 发往主窗。覆盖页 = 新的一页 `src/screenshot.html` + `src/screenshot-overlay.ts`（暗幕 + 拖框 + 尺寸读数 + Esc/右键取消，暗幕靠「框外一圈 9999px 阴影」，框内自然是原亮度）。**抓帧 / PNG 编码 / 裁剪都走 `run_blocking`**；**RGBA 留在宿主**（前端只回矩形，几 MB 的图不过 IPC）；**会话还在 = 这次框选还没结算**（`finish`/`cancel` 取走会话，Alt+F4 关窗时补发 `cancelled`，调用方不会卡在「正在截屏…」）。
- **三个非显然的坑**（都已踩过并写进 ai-spec §3.5 / 预检 #72）：① `WebviewWindowBuilder` 的 `position` / `inner_size` 收**逻辑**单位，而 `GetSystemMetrics` 给**物理**像素 ⇒ 建完（`visible(false)`）再用 `set_position(PhysicalPosition)` / `set_size(PhysicalSize)` 摆一次，**别用 `fullscreen(true)`**（Windows 上只覆盖一台显示器）；② 新窗 label 不在 `plugin-*` 里 ⇒ `capabilities/default.json` 的 `windows` 必须显式加 `"shot-overlay"`，且 `main.rs` 的 `CloseRequested`（「关掉=收进托盘」）要放行、`Destroyed` 走 `on_overlay_destroyed`；③ `vite.config.ts` 的**多页入口**要加 `screenshot`，少了它打包版开覆盖窗是白屏（dev 按需编译看不出来）。
- **OCR 插件侧只留接线**：删掉窗内 overlay（`pickRegion` 与 `.ocr-shot-*` 样式、`capture_screen` 命令一并移除），改成「点按钮 → **先挂 `listen`** → `screenshot_overlay_begin` → 收到 done 就把裁剪结果交给 `save_temp_image` + `run_paddle_ocr`」；新抽 `ocrCropped()` 复用于面板回填。i18n 沿用 `ocr.shot_hint / shot_capturing / shot_failed`（未新增键）。
- **插件版本**：`ocr 0.9.8 → 0.9.9`（一版）→ **`0.9.10`**（二版，包内容变了必须 +1，预检 #43 ④）；`build-plugins.ps1 -Plugin ocr` 跑通（`ocr-0.9.10.zip`、dev 的 `target\debug\Modules\ocr\` 已同步，产物里 `screenshot_overlay_begin` 在、`capture_screen`/`pickRegion`/`ocr-shot-overlay` 都不在）。⚠️ 文本替换改 `build-plugins.ps1` **又一次抹掉 BOM** —— 已按「先剥掉所有连续 BOM、再前置一份」修回（`ef bb bf`）。
- **验证**：`cargo check --bins` exit 0（`cargo clean -p lunac` 后完整重编，13.41s，无 error —— 前几次被运行中的 dev 实例锁住构建产物、报 `os error 32`，关掉实例后可编）；`cargo test --bins` **206 passed / 0 failed / 2 ignored**；`npx tsc --noEmit` exit 0；`npm run build` exit 0（`dist/screenshot.html` 已产出）。
- **补记（2026-10-06，实机一验踩到的坑）**：覆盖窗正常出来了、图也在，但**怎么拖都拖不出框**。根因是 CSS：暗幕层 `#shot-dim` 盖在 `#shot-img` 上面却漏了 `pointer-events: none`，把 `mousedown` 全吃掉（拖拽那时绑在 `img` 上）。**修法两处**：① `#shot-dim` 补 `pointer-events: none`；② 拖拽处理从 `img` 改绑到容器 `#shot-root`（容器里还压着框 / 读数 / 提示几层，绑在 img 上时任何一层忘了放开指针都会致命）。纪律进 `code-rules.md` 预检 **#72 ⑧**。**只改宿主前端，不涉及插件包 ⇒ 不用升 ocr 版本**。
- **实机验收（2026-10-06，已通过）**：dev 里点「截屏识别」→ 覆盖窗铺满屏幕 → 拖框 → 松手出文字，全链路可用（用户确认「已测试完毕」，本档结案）。**仍未做**：发布（`publish-plugins.ps1` 未跑，市场里的 ocr 仍是 0.9.8）。

---

## 4. 设想区（**没有落点，勿当成待办**）

以下是不再有实现意图、或只有一句话想法的东西。**保留只为防止重复讨论**，真要做时先重写落地路径再挪进正文。

- **旧 CLI 的 `Config`（ant-only）/ `REPL`（ant 专用 VM）/ `Workflow` / `RemoteTrigger` / `Monitor` / `SubscribePR`·`SuggestBackgroundPR` / `TerminalCapture` / `Snip` / `Sleep` / `StructuredOutput` / `OverflowTest`·`CtxInspect` / `TestingPermission`** —— 要么绑定旧 CLI 的特定形态（Ink TUI / ant 内部构建），要么是调试与实验开关；在 `getAllBaseTools()` 里大多已被显式置为 `null` 停用，属旧 CLI 自己的历史包袱。
- **按分类的会话临时文件清理服务**（Hermes 的 `disk-cleanup` 插件思路）—— 本项目的临时产物集中在 `<exe 根>\temp\`，**卸载时由 NSIS 整目录删除**，`logs` 另有 7 天保留清理（`log::purge_old`，`KEEP_DAYS=7`）。便携式布局下「整个 temp 目录」就是清理单位，**不需要**按 test / temp / session / download 分类。详见 `ai-spec.md` §13.2。
- **`TeamCreate` / `TeamDelete`（命名团队的生命周期）** —— 随 A13「多代理协作」于 2026-10-05 重估后的结论：代理间通信只做成**同批兄弟**形态（`SendMessage` / `ListPeers`，见 §3.5「多代理通信」），因为本仓子代理是「同批并发、跑完即散」的，没有**常驻团队**这个东西 —— 没有常驻成员，`TeamCreate` / `TeamDelete` 就没有可创建 / 可解散的对象。真要做「常驻后台代理 / 团队模型」得先重开架构（属 L10/B 那一档），届时再把它挪回正文。

---

## 5. 边界声明（本清单覆盖什么、不覆盖什么）

**覆盖**：用户可以观察到的能力面 —— **工具名**、**子系统**、**协议消息类型**、**界面分期**、**自身新目标**。

- **工具面已核对完毕**：旧 CLI 的 `getAllBaseTools()` 里的每一个名字都已归位（实现 / 待办 / 设想）；Lunac 当前实际注册的是 **17 件内置工具**（`Read` / `Write` / `Edit` / `Cmd` / `PowerShell` / `Glob` / `Grep` / `WebSearch` / `WebFetch` / `AskUserQuestion` / `TodoWrite` / `SessionSearch` / `Agent` / `EnterPlanMode` / `ExitPlanMode` / `ListPeers` / `SendMessage`）+ **条件注册**的四件：`Skill`（技能目录非空且未被黑名单裁掉时）+ `ListMcpResourcesTool` / `ReadMcpResourceTool`（**桥接上了用户工具时**，2026-09-20）+ `Remember`（**桥接通时**，2026-09-20，长期记忆写入侧）+ `mcp__*`（来自 `<exe 根>\tools\*.json`），可用 `--disallowedTools` 裁剪。
- **不覆盖一**：`core-agent/src/` 里的实现细节级能力（如各工具的解析细节、日志格式），按子系统归并。
- **不覆盖二**：旧 CLI 终端渲染组件（`core/tools/**/UI.tsx`）随 Ink TUI 一并排除。
- **不覆盖三**：Lunac 与旧 CLI **都有**的能力不再列出（例如前端依赖的 stdout 契约 `system/init` / `stream_event` / `assistant` / `user/tool_result` / `control_request` / `result` **全部已提供**；`--disallowedTools` 链路已通；思考开关跨模型自适应是**超集**——只有开 / 关两档，不要按「多档更深」扩）。
- **口径提醒**：`session_id` 自 A11（2026-09-20）起是**真值**并已接上消费者（前端随用量落盘做归因，见 `ai-spec.md` §11 规则 64）；`total_cost_usd` / `num_turns` / `duration_ms` 仍**前端零引用**、`stop_reason` 未提供 —— 这几项**当前无影响**，不列为待办，改动前先确认有消费者。

---

## 6. 环境变量与数据根（核对基线）

下表列出 `core-agent` 读取的 **19 个** `LUNAC_*`（均**只在 spawn 时注入**；切换其中任何一项 = `kill_and_cleanup()` 重启 agent）：

> 口径：这里列的是**宿主 → agent 的输入**。另有 3 个由 agent 自己解析 / 兜底
> （`LUNAC_MODULES_DIR`、`LUNAC_CACHE_TTL_SECS`、`LUNAC_TURN_BUDGET_TOKENS`）与 2 个只传给
> **hook 子进程**（`LUNAC_HOOK_EVENT`、`LUNAC_TOOL_NAME`），不在此表。

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
| `LUNAC_PERSONA_FILE` | `main.rs` | **用户人格 / 自定义提示词的来源文件**（`config\persona.md`，无条件注入；**启动时读一次**进系统提示词的固定段 ⇒ 改完要重启 agent 才生效，与上一行的语义相反，见 §11 规则 66） |
| `LUNAC_MCP_FILE` | `mcp.rs` | **远端 MCP 服务器配置**（`config\mcp.json`，无条件注入；**文件不在 = 没配远端服务器**）。只做 Streamable HTTP；授权 `auth` 两档 —— 缺省 `static`（静态 `headers`）/ `oauth`（令牌落同目录的 `mcp-tokens.json`）；改完要重启 agent 才生效（工具表进固定前缀），见 `code-rules.md` 预检 #54 |
| `LUNAC_SEARCH_PROVIDER` / `LUNAC_SEARCH_KEY` | `tools.rs` | 联网检索主源 |
| `LUNAC_LOG_DIR` / `LUNAC_LOG` / `LUNAC_LOG_LEVEL` | `log.rs` | 落盘日志 |

**数据根 = `<exe 根>`（便携式，无独立 HOME）**：`skills\`（技能）、`tools\`（用户工具定义）、`ModuleData\`（`history\chat.db`、`memory\MEMORY.md`、`usage\usage-*.jsonl`）、`temp\`（`logs\`、`tool-outputs\`、`transStorage`、`webview-data`）、`config\`。用户资产的扩展格式见 [agent-implementation.md](./agent-implementation.md) §5。

---

*一次性的「追加」记录（A1–A16 各批的完成说明、B 类缓存崩塌的复现数据、八套 e2e 的串跑基线等）已于 2026-09-29 **全部删除** —— 本清单只留未完成项。结论与实测数据都在 `ai-spec.md` §3.5 / §11 规则与 Git 记录里；其中**仍未验收**的三条已搬进上面的 **M2** 表格。*
