# Lunac 待办清单（**唯一待办真相源**）

> **用途**：本项目**全部未完成工作**的唯一登记处。`ai-spec.md` / `agent-implementation.md` / `agent-ui-spec.md` / `architecture-rendering.md` **只保留规则与现状，不再各自维护待办**；任何新想法一律登记到本文。
>
> **三条维护纪律**
> 1. **做完就删** —— 条目完成后从本文**删除**，不标 ✅ 留在原地。「为什么这么做、踩了什么坑」的结论落进 `ai-spec.md` §11 规则、代码注释与 Git 提交记录，**不由待办条目承载**（本文因此不再有历史包袱，读一遍就知道还剩什么）。
> 2. **每条必须能落地** —— 写清「做什么 / 为什么值得 / 依赖 / 落点文件」；写不出落点的东西属于设想，一律放 §4 设想区，不混进待办。
> 3. **优先级只按本文顺序** —— 级别（P0–P3）是粗档，档内顺序即实施顺序。
>
> **最后核对**：2026-09-21（逐条对照 `core-agent/src/`、`app/src/`、`app/src-tauri/src/` 实测，非照抄旧文档）
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
| **P2** | L1 Live2D 桌宠 | **用户已定方向**；桌宠三条**已逐条裁决**（2026-09-21，见 L1-A / L1-B）：桌宠 = **插件** + **独立透明置顶窗** + **模型由用户自备**，安装包零第三方模型资产。插件市场那一层、最小原型（2026-09-29，见 `architecture-rendering.md` §6.2）、宿主侧的**鼠标穿透**与**「可见吗」下发**（`ai-spec.md` §4.8）、以及**桌宠外壳本体**（2026-09-29：清单声明窗口形态 + `Modules\pet\` 的「控制台 + 桌宠窗」两窗 + 形象导入，实机验收见 `ai-spec.md` 文末实测表）**都已就绪**；剩下的是 **Live2D 引擎那一层**，动手前先解 L1 里那两条硬障碍 |
| **P2** | L5 自研调音插件（DSP 引擎 + 测量 + AI 调参） | **用户 2026-09-29 定方向**：自研 DSP、脱离 EqualizerAPO / Peace（只作学习参考）。体量远大于 L1，但 S0（引擎内核，离线可单测）/ S1（自身播放闭环）/ S2（loopback 测量）**都不碰系统音频、不要驱动、不要 UAC**，可独立验收。**排在 L1 之后**（档内顺序即实施顺序） |
| **P2** | **L6 成本归因与降本** | **用户 2026-09-29 提出**（「Lunac 的 API 金额是 Trae 的 10 倍」）。归因已结案（`ai-spec.md` §9.1 难点 3：11.2 倍 = 命中率差 3.09× × token 结构差 3.63×，**不是计价贵**；那两档价是官方的**峰谷定价**）。**已完成六件**：子代理 / 复盘用量并入 `result.usage`（§3.5）、思考档 A/B 实测（非主因）、**价格表支持时段价 + 预置官方峰谷价**（§3.5 + 规则 63，面板金额已与账单逐分对上）、**输出瘦身**（输出纪律进 `PERSONA_AND_STYLE`，稳态每次提问成本 **-41%**，§9.1 结论 7）、**压请求次数已结案**（`TOOL_PARALLELISM` = 「同响应多 `tool_use`」，提示词层引导**实测未生效** ⇒ 请求数是模型自己的往返节奏，勿动并行实现）、**对账基线工具**（`scripts\reconcile-usage.ps1`）。**未做只剩一步**：拿平台导出的 CSV 跑一次跨源基线 |
| **P3** | A13 记忆目录 / 斜杠命令 / 剩余工具 | 按需重估，见各项 |
| **P3** | A17 `tool_result` 侧的安全告警色块 | 扫描器与审批卡通道都已落地，只差工具卡上那块渲染 |
| **P3** | U1 diff 卡 / U2 缩略导航·Fork / U3 真沙箱调研 | 界面增强与调研，排在功能之后 |
| **P3** | M1 两个待实测项 / M2 三条待验收 | 不阻塞任何开发，攒到复现 / 验收时做 |

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
| 多代理协作（`TeamCreate` / `TeamDelete` / `SendMessage` / `ListPeers`） | **前置已落地**（子代理 `Agent`，2026-09-20；同日 A14 起同一轮里还能**并发**派多位，上限 3）—— 应立刻重估，不要再按「与 Lunac 无关」处理。注意 `SendMessage` / `ListPeers` 这类**代理间通信**与 A14 不是一回事：A14 的子代理之间**没有任何通道**，只各自回报告 |
| 输出样式 / statusline | CLI 的终端样式体系，已被 WebView 取代 |
| Ink TUI / Vim / 语音 / buddy / chrome / 桥接远程控制 / OAuth 账号体系 / 自动更新 / 代理证书 mTLS / IDE 集成 | 绑定的分别是旧 CLI 的终端形态、当前交互形态没有的入口、企业网关场景；**由 NSIS 安装包与 `vscode-extension/` 各自负责的部分已完成**。逐项都要「先确认它在新宿主下还有意义」再重估 |
| **MCP 远程传输与其余能力**（`ai-spec.md` §20.1 的「仍未做的」） | 远程传输（sse / http / ws）、prompts / roots / elicitation / OAuth、`.mcp.json` —— 现在只有 **stdio + tools / resources** 两条。**别因为 §20.1 标题写着「已落地」就当 MCP 全做完了** |

**A17. `tool_result` 侧的安全告警渲染（`securityWarnings` 色块）**

- 现状：`content_safety.rs` 的扫描结果**只接在审批卡**这一条通道上（`analysis.secrets` → 卡片正文明细行）；工具返回文本里只有一句给**模型**看的纯文本提示（`tools.rs` 的 `secret_note()`）。前端**没有** `securityWarnings` 组件 ⇒ 工具卡上不会把命中项显式标出来。
- 要做：工具返回里带结构化告警 → 工具卡渲染成色块。口径与边界见 `ai-spec.md` §13.1 / §19.2 与 §11 规则 58（「必须人看」那一档的判据别放松）。
- 2026-09-29 通读文档时补登记的：它是当时全仓**唯一**「既未做、又没进本文」的功能缺口。

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

**L1. Live2D 桌宠**

- **还剩什么（2026-09-29 更新）**：**只剩 Live2D 引擎那一层**。外壳已经做完并实机验收：桌宠插件本体（`Modules\pet\`，源码 `app/src/plugins/builtin/pet.ts`）能开出一个**独立透明置顶、不进任务栏、禁手动缩放、无标题栏**的窗，形象由用户在**控制台**里导入（`dialog.open()` 选图片 → 存 `localStorage` → `convertFileSrc` 载入），控制台负责穿透开关 / 形象大小 / 显示·隐藏（**分工不能反过来**：穿透开着时桌宠窗收不到鼠标事件，开关只能放在另一个窗里）。宿主那两条能力也已在桌宠上跑通（穿透开/关 `0x40118 ⇄ 0xC0138`；最小化后动画停、拉回来立刻恢复）。窗口形态这件事由**插件清单的 `window` 段**声明（宿主不按 id 写死），契约与两条实测坑见 `ai-spec.md` §4.8「窗口形态由插件清单声明」，本轮全部实机数据见 `ai-spec.md` 文末实测表。
- **Live2D 引擎为什么单列**（动手前先解这两条，别直接怼 pixi）：① **`.model3.json` 里的纹理 / 动作是相对引用**，而 asset 协议的 URL 形态是把**整个绝对路径** percent-encode 进最后一段（`http://asset.localhost/D%3A%5Ccc%5C…%5Cicon.png`，本机实测），`new URL("tex.png", 模型URL)` 必然落到错的地方 ⇒ 要么引资源改写（自己解析 model3.json 把每个引用换成绝对路径再 `convertFileSrc`），要么确认 pixi-live2d-display 的 loader 中间件能拦 URL —— **两条都要先做最小实验**；② **Cubism Core 是 Live2D 的专有文件**（可再分发但带义务，见 L1-B ①），而且官方 URL **不带版本号** ⇒ 写成 `dependencies[file].sha256` 会在上游某次更新后**把安装整个卡死**（不写 sha256 只 warn 留痕）。另：`pixi-live2d-display` 最新只有 **0.4.0**（2021，绑 pixi v6），而 pixi 已到 8.x —— 用老栈还是改用 Cubism Web Framework 直连，也要先定。
- **最小原型已量完（2026-09-29）**：数据与取证方式见 **`architecture-rendering.md` §6.2**。三条结论 —— ① 第二个窗口的边际成本 = **+1 个 `renderer` 进程**（不是再起一个实例）/ 工作集 **+109 MB**、私有 **+54 MB**，关窗即回收；② **最小化不会让 rAF 停**（仍 ~170 fps、~5% 单核，`document.visibilityState` 全程是 `visible`）；③ 透明置顶窗的**透明度已经实测成立** —— 宿主侧那两个前置**已落地并实机验证**（`ai-spec.md` §4.8 与文末实测表）。
- **插件市场这一层不必再设计**：落盘 `<exe 根>\Modules\<id>\`、清单 `lunac-plugin.json` + 已编译 ESM、https zip 安装 / 卸载，契约见 `ai-spec.md` §3.5「插件市场」；**插件带代码不带模型**。
- **发布这一步还没做**（2026-09-29，与 Live2D 引擎无关，是「让桌宠能到用户机器上」的动作）：① `scripts\build-plugins.ps1` 的 `-Plugin pet` 已能产出 `release\plugin-packages\pet-0.9.6.zip`（14.5 KB）并在 `release\ext-plugins\pet\` 暂存，但**没推市场**（`publish-plugins.ps1` = 传 Release + 改 `index.json`，是人工一步，见预检 #43）；② `scripts\lunac-installer.nsi` 的「拓展插件」段目前只列了 clipboard-history / ocr / convert / music **四段**，要进安装包的可勾选项**得再补一段**（照抄那几段即可，`!if /FileExists` 已能保证没暂存时不报错）。

**L1-A. 桌宠的三条已裁决（2026-09-21 用户拍板，实施时不得自行改回）**

| 项 | 裁决 | 代价 / 约束 |
|---|---|---|
| **形态** | **独立透明置顶窗**（真桌宠，另开一个 Tauri 窗口 = **第二份 WebView2**） | 用户已知情并选择接受（此前为省 11MB 的 `--disable-gpu` 都专门权衡过，这一条要写进实施记录，别事后当成「忘了优化」）。**代价口径已按 2026-09-29 实测修正**：第二个窗口**只多 1 个 renderer 进程**（不是再起一个实例），边际 = 工作集 **+109 MB** / 私有 **+54 MB**，详见 `architecture-rendering.md` §6.2 |
| **形象来源** | **用户自己导入模型**；Lunac 只提供引擎 + 导入通道 | 安装包**零第三方模型资产**。义务因此落在终端用户身上（他下载时自己接受 Live2D 的协议） |
| **实施顺序** | **先做插件市场（L1 本体），桌宠作为插件接入** | 桌宠不做成内置依赖；模型目录 / 引擎路径都由插件自己声明 |

**L1-B. 版权四条线（2026-09-21 核实官方条款，改资产策略前必须重读）**

**① 引擎侧（Cubism Core）—— 可以随包分发，但有义务**：`live2dcubismcore.min.js` 属 **Live2D Proprietary Software**（不是开源），但它在官方 `RedistributableFiles.txt` 清单内 ⇒ 协议 **5.1** 明确允许复制与转发。条件：**保留随附的 LICENSE / RedistributableFiles**、要求下游接受同等条款（**5.2.2**）、第三方因此产生的费用与诉讼自行承担（**5.2.3**）。另有 **Cubism SDK Release License（出版许可）**：门槛 = 最近一个会计年度销售额 **≥ 1000 万日元**（约 6.7 万美元 / 约 48 万人民币）的「事业者」必须签 ⇒ 本项目当前不需要，**越线要补签**。

**② 模型数据侧 —— 「Distribute ≠ Redistribute」，这条最容易踩**：官方样例模型（Hiyori / Haru / Mao / Miara…）属 **Free Material License + Sample Data 利用条件**：一般用户 / 小规模事业者**可商用**，也**可以**把「含有该角色的自己的作品」交给第三方（Distribute）；但 **4.1.1 No Redistribution 明确禁止把素材本身或其复制品再分发** ⇒ **把官方 `.moc3` + 纹理打进 Lunac 安装包 = 禁止**（把它当默认桌宠随包发正好踩这条）。附带义务：必须写官方指定的**版权声明**（长 / 短两版，视载体而定）；**Miara / Hiyori 的设计不得做任何改动**、Shizuku 不得改名改设定、Mark-kun 不得画成写实帅哥、Nito 保持头身比；个别样例对「直播使用」另有额外限制。**协作角色（名執尽 / 春日笠つみき）更紧：非商用 only、不得改动、不得分发** ⇒ 桌宠里绝不能用。**外部授权角色**（初音ミク / ずんだもん / ユニティちゃん）要遵守第三方各自的条款。

**③ 角色 IP 侧（最容易被忽略）**：即便模型是自己建的，只要画的是**别人的角色**（VTuber / 游戏 / 动画），就是**角色著作权与商标**问题，与 Live2D 授权无关；**同人模型不能随公开发行的软件分发**。判据与既有纪律同源（`core/` 不入库、上游 `cli.exe` 不进包）：**公开仓库 ≠ 什么都能放**。

**④ 我们自己的代码与插件市场**：`pixi-live2d-display`（MIT）+ pixi.js（MIT）可用，但 **MIT 只覆盖包装代码**，Core 仍受 ① 约束，许可证文件要随包保留。L1 会**放大**这条：从用户 GitHub 仓库下载的插件若自带模型数据，责任落在「谁分发」⇒ **插件只带引擎与逻辑、不带模型**，模型一律由用户导入。

- 相关：`ai-spec.md` §20 的「路径 2」是本节的前身。**注意 `core/` 的含义**：它是**本机参考用的旧 CLI 源码**（被 `.gitignore` 排除、**不在仓库里**），所以 `core/plugins/...` 一类路径只表示「参考它的设计、在新宿主重建」，**照它去找一定找不到**；真要做时**先重写落地路径**。

**L3. 上下文感知提示词注入的剩余子集**

- 现状（**部分已落地，别当从零做**）：① 工作目录 / 宿主 / 技能目录已由 `core-agent` 的 `env_block()` 拼进系统提示词；② 前端已有按 query 关键词（debug / TDD / review）的 `buildSystemPromptHint()`，拼进**消息**；③ 附件路径走 `[Attached files]` 文本。
- **缺的只有**：「当前打开的插件 / 当前选中的文件」进上下文。补的时候注意区分「进系统提示词」（必须固定，否则破坏缓存）与「进消息」（可变）。

**L4. 调试阶段状态栏**

- 原 `ai-spec.md` 的 P2 遗留项（现 §19.1）。做一个只由开关控制的阶段状态栏，展示 agent 当前处于哪个阶段。现状只有**常显**的状态机与运行时提示（`.sys-note`，展示 agent stderr），没有分阶段的调试视图。

**L5. 自研调音插件（DSP 引擎 + 测量 + AI 调参，**不依赖 EqualizerAPO / Peace**）**

- **方向（用户 2026-09-29 定）**：**自研 DSP 引擎**；EqualizerAPO / Peace 只作**学习参考**（学它的模型与交互），最终**脱离它们**、作为 Lunac 的一个拓展插件存在。要的能力面：**测量频率曲线**（含 **WASAPI loopback 电气闭环**）、**用 AI 调各频段 dB**、**多个效果器**、**接管输出 / 输入设备**。
- **平台硬约束（先记住，别绕）**：Windows 上「让**系统级**音频经过自己的处理」只有两条路 —— ① **APO**（COM in-proc，跑在 `audiodg.exe` 里，崩溃会带走整个音频服务；EqualizerAPO 做了十几年）② **虚拟声卡**（第三方内核驱动）。**本项目明确不做 ①**，所以「全局生效」落在 **S3**：默认输出切到虚拟声卡 → 我们采集 → DSP → 渲染回真实设备。
- **形态**：引擎 = **插件自带的独立 exe**（Rust，走 `dependencies[type=file]` 装进 `Modules\<id>\bin\`，与 `music` 的 librespot **同一条路** ⇒ 卸载即完全不存在、宿主不必为它长大）；面板 = 纯 UI（曲线图 / 滤波器表 / A-B / 测量）；两者之间 **照抄 `agent.exe` 的 stream-json 范式**（stdin/stdout NDJSON + 日志落盘），**不发明新协议**。**配置与测量的落盘归引擎** ⇒ 「插件要写盘」这件事在本架构下**不需要**给宿主加通用文件命令。
- **配置格式自立为 JSON**（不再用 EAPO 的文本语法）。**从 Peace / EAPO 学的四样**：滤波器类型表（PK / LS / HS 各阶、LP / HP / BP / NO / AP）、效果器链的组织方式、作用域（设备 / 通道 / 前后级）、快照与一键 A-B。
- **分期**：**S0** 引擎内核（**完全离线可单测**：WAV→DSP→WAV，用「某滤波器在某频点的增益 = 理论值 ±0.1dB」钉住；FFT / 反卷积 / 配置 JSON）→ **S1** 自身播放链路闭环（先只管 Lunac 自己放的音乐，零驱动零 UAC）→ **S2** WASAPI **loopback** 闭环测量（本机「立体声混音」是 ACTIVE 的）→ **S3** 全局接管（虚拟声卡 + 按设备切配置 + 输入侧）→ **S4** AI（意图解析 / 选目标曲线 / 复核微调 / 解释；**确定性拟合在引擎里，AI 的输出必须过本地关口**：参数合法 + 拟合残差不变差 + 增益结构不削波）。
- **两个必须提前知道的坑**：① **EqualizerAPO 目前挂在本机 13 个端点上**（9 Render + 4 Capture，APO CLSID `{EACD2258-FCAC-4FF4-B36D-419E924A6D79}` / `{EC1CC9CE-FAED-4822-828A-82A81A6F018F}`）—— 引擎上马时两边会**叠加**，且测量曲线里会混着 EAPO 的 EQ；「迁移口径」（何时卸、卸 APO 要 UAC + `Configurator.exe`）要单列一条，否则「测不准」会被误判成引擎的 bug。② S3 的虚拟声卡是**第三方内核驱动**（VB-Cable / Voicemeeter，各有许可条款），自研内核驱动不现实 ⇒ **S3 开工前再选**。
- **附加功能候选**（有自研引擎后不再受 EAPO 能力限制）：自动 Preamp 防削波 / 等响度补偿 / 交叉馈电 / 卷积与房间校正 / A-B 盲测 / 听力自测补偿（必须标「非医疗器械」）/ 把 PEQ 导出给耳机 App 与 DAP / 实时频谱 / 多声道低音管理。
- **落点**：`<exe 根>\Modules\<id>\`（id 待定：`apo` / `eq` / `tuning`）；引擎源码进本仓（与 `core-agent/` 并列新 crate），产物挂公开 Release 资产；面板按 `agent-templates\modules\README.md` 的插件规范写。
- **诚实提醒**：本条的体量**远大于** L1 桌宠 —— S0/S1/S2 是能独立验收的三小步，S3 才碰系统音频。

**L6. 成本归因与降本（2026-09-29 提出；用户口径「Lunac 的 API 金额是 Trae 的 10 倍」）**

- **归因已结案**（数据与推导见 `ai-spec.md` §9.1 难点 3）：**不是计价贵** —— 两个 key 同账号，那两档价是 DeepSeek **2026-08-17 起生效的官方峰谷定价**（工作日 09:00–12:00 + 14:00–18:00 峰、其余含周末谷、谷 = 峰 ÷ 2）；11.2 倍 = **命中率差 3.09× × token 结构差 3.63×**（输入命中 98.30% vs 89.14%；输出占 token 的 0.50% vs 12.4%）。**「元 / 百万 token」不能当 KPI**（它奖励「堆缓存命中」，会把「上下文很长但几乎全命中」判成先进），改用「每次提问成本」。
- **已完成，不得回退**：① **子代理 / 后台复盘的用量并入 `result.usage`** —— 此前它**完全没进本地账**（平台同一 key 24 次请求 vs 本地 `usage-*.jsonl` 17 次），契约与守门单测见 `ai-spec.md` §3.5「用量与对账」；② **思考档 A/B 实测**（结论：**不是主因**，是 1.0~1.3× 的乘数；探针 `core-agent\target\hooktest\e2e-ab-thinking.ps1`，数据见 `ai-spec.md` §9.1 难点 3 结论 6）；③ **价格表支持时段价 + 预置官方峰谷价**（`time_windows` 契约见 `ai-spec.md` §3.5 与规则 63，实测见文末）—— 修好前面板对谷时用量**高估 45%**（dev 环境 2026-09-29：0.198505 → 0.136858 元），与账单已能**逐分对上**。
- **已完成，不得回退**（续上一条的 ①②③，本轮 2026-09-29 做完 ④⑤⑥）：
  1. **输出瘦身** —— 输出纪律进了 `PERSONA_AND_STYLE` 的 Output Style 段（无开场白 / 不预告工具调用 / 不复述刚读到的内容 / 只答被问的 / 不贴回用户已能看到的正文）。固定任务三次采样：答案字符 **1156 → 701 / 474**，稳态**每次提问成本 -41%**（4854 → 2843 miss 等价）。探针 `core-agent\target\hooktest\e2e-l6-cost.ps1`，数据见 `ai-spec.md` §9.1 难点 3 结论 7。**改这两块常量会打掉一次端点侧缓存**（每台机器每套前缀各一次，不是每次提问都付）—— 别为此把新句子挪进用户消息。
  2. **压请求次数：已结案（不是本侧的锅）** —— 查清 `TOOL_PARALLELISM` 的语义是「**同一个响应里 N 个 `tool_use`、本地并发执行**」，**不是**并发发 N 份请求（`ai-spec.md` §9.1 结论 8）；`SYSTEM_PROMPT` 里加了「互不依赖的读 / 搜放进同一次响应」的引导，但**实测三次请求数全是 4、工具序列逐字相同 ⇒ 未生效**。结论：「一次提问 7~10 次请求」是**模型自己的往返节奏**，**不要再动并行实现**；真要压只能从「让模型少问几轮」入手（任务熟悉度 / 更强的工具描述）。
  3. **对账基线工具就绪** —— [reconcile-usage.ps1](file:///d:/cc/claude-code-cli-master/scripts/reconcile-usage.ps1)：本地 `usage-*.jsonl` 按**本地小时**聚合四类 token + 用 `pricing.json` **逐桶**峰谷计价（14:00 桶 = 0.07521104 元，与手工核算逐位一致）；`-Csv <平台导出>` 再做逐小时对照（表头模糊识别，认不出 `exit 2`；金额差 > 0.0001 元标 `DIFF` / `exit 1`）。契约见 `ai-spec.md` §3.5「定价表与成本面板」。
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
| **M2-3** | 任务抽屉与语义色按钮的视觉 / 手感 | 抽屉在真实对话里的显隐、两列表互斥展开、语义色按钮没被盖掉 —— **未做浏览器实测**（回归口径见 `ai-spec.md` 规则 45 与 `agent-ui-spec.md` §8） |

> 同一段历史里还有一条**已经关闭**的：插件 ESM 走 asset 协议的真机链路 —— `ai-spec.md` §3.5「插件市场」的 2026-09-29 实测行已跑通（音乐窗资源列表里只有 `asset.localhost/…/Modules/music/index.js`），不再列为待办。

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
| `LUNAC_PERSONA_FILE` | `main.rs` | **用户人格 / 自定义提示词的来源文件**（`config\persona.md`，无条件注入；**启动时读一次**进系统提示词的固定段 ⇒ 改完要重启 agent 才生效，与上一行的语义相反，见 §11 规则 66） |
| `LUNAC_SEARCH_PROVIDER` / `LUNAC_SEARCH_KEY` | `tools.rs` | 联网检索主源 |
| `LUNAC_LOG_DIR` / `LUNAC_LOG` / `LUNAC_LOG_LEVEL` | `log.rs` | 落盘日志 |

**数据根 = `<exe 根>`（便携式，无独立 HOME）**：`skills\`（技能）、`tools\`（用户工具定义）、`ModuleData\`（`history\chat.db`、`memory\MEMORY.md`、`usage\usage-*.jsonl`）、`temp\`（`logs\`、`tool-outputs\`、`transStorage`、`webview-data`）、`config\`。用户资产的扩展格式见 [agent-implementation.md](./agent-implementation.md) §5。

---

*一次性的「追加」记录（A1–A16 各批的完成说明、B 类缓存崩塌的复现数据、八套 e2e 的串跑基线等）已于 2026-09-29 **全部删除** —— 本清单只留未完成项。结论与实测数据都在 `ai-spec.md` §3.5 / §11 规则与 Git 记录里；其中**仍未验收**的三条已搬进上面的 **M2** 表格。*
