# Lunac 待办清单（**唯一待办真相源**）

> **用途**：本项目**全部未完成工作**的唯一登记处。`ai-spec.md` / `agent-implementation.md` / `agent-ui-spec.md` / `architecture-rendering.md` **只保留规则与现状，不再各自维护待办**；任何新想法一律登记到本文。
>
> **三条维护纪律**
> 1. **做完就删** —— 条目完成后从本文**删除**，不标 ✅ 留在原地。「为什么这么做、踩了什么坑」的结论落进 `ai-spec.md` §11 规则、代码注释与 Git 提交记录，**不由待办条目承载**（本文因此不再有历史包袱，读一遍就知道还剩什么）。
> 2. **每条必须能落地** —— 写清「做什么 / 为什么值得 / 依赖 / 落点文件」；写不出落点的东西属于设想，一律放 §4 设想区，不混进待办。
> 3. **优先级只按本文顺序** —— 级别（P0–P3）是粗档，档内顺序即实施顺序。
>
> **最后核对**：2026-09-30（M2-4 四条新功能真机回归通过后**已撤下**，同时登记回归中发现的 M2-6；**L1 的 Live2D 引擎已落地并真机验收** —— 栈定案为 `pixi.js@7` + `pixi-live2d-display-lipsyncpatch`，桌宠窗里真的渲染出模型（18215 个非透明像素），**只剩「发出去」**（传 Core Release 资产 + 附授权文件 + 推市场）；**L5 的 S0 引擎内核也已落地并验收**（`tuning-engine/` 独立 crate，54 条单测，端到端 `gen`→`render`→`measure` 实测 = 理论 ±0.1dB 口径，下一刀是 S1）；新增 L9 / L10 与 M2-5。L1–L8 其余内容仍是 2026-09-29/30 逐条对照 `core-agent/src/`、`app/src/`、`app/src-tauri/src/` 实测的结论，非照抄旧文档）。**2026-10-09 追加**：§4 增加 **§4.1「长期 AI 侧规划」**（分阶段能力成熟度路线图 S0–S4，用户立项，主题 = 本地模型只参与决策 / agent 整体框架图 / skills+tools 完善与长任务托管 / AGI 达成要求）—— 该节为**方向性、不排期**，条目在进入实施前先提升为 §1–§3 正文。同日再追加 **§4.1.6「决策保真度」实验设计**（方向已定 = 本地策展 + 大上下文、摊销参与；该实验是它的验收先行件）。**2026-10-09 晚间三件已落地**：① **长任务最小档**（预算撞顶不中止，见 §4.1.3 / `ai-spec.md` 规则 90）；② **桌宠控制台「导入模型后按钮全失效」改为事件委托**（绑在结果区稳定容器上，面板重建不再丢监听）+ 插件窗 / 挂载失败补 `log_frontend` 上报（此前插件窗异常不进宿主日志）；③ **`E:\Live2dbase\御姐2` 依赖适配**：13 个 `*.exp3.json` 接进 `FileReferences.Expressions`（**触发入口留给 AI**，本次不做 UI）；**纹理降采样已回退原图** —— 随后定位到真因不是纹理而是 pixi 的 CSP `unsafe-eval` 报错（见 ④）；④ **桌宠 release 加载失败的**真因 = 严格 CSP**：引擎补 `import "@pixi/unsafe-eval"`（dev 不套 CSP 故只在 release 复现），规范见 `ai-spec.md` 规则 91`。**桌宠 UI 重构（用户 2026-10-09 定，第 2/3 条第一批，pet 0.9.13）**：① **搜索「桌宠」直接开那扇设置独立窗**（`plugin-pet-settings`）；**旧的内嵌控制台面板已删**，其功能（状态切换 / 更换形象 / 取消配置 / 穿透 / 大小）整体迁到设置窗，主窗那格只留一句提示 + 「再次打开」；②「显示桌宠 / 收起来」合成**单一状态按钮**（用 `pet-control` 的 **ping/pong** 判桌宠窗死没死 —— 零宿主改动）；③「选择模型 / 选择图片」合并为**统一「更换形象」**（一个对话框，按扩展名分流 json/图片）；④「不用模型」→「取消配置」；⑤ 底部整段说明改为「该插件只提供 Live2D 模型入口」；⑥ 桌宠窗右键菜单加「桌宠设置…」（**同一份界面另起一窗**，label `plugin-pet-settings`）、去掉「大一点/小一点」。**仍未做（第 2/3 条剩余）**：模型调试区（横向拉条 + 表情状态按钮）—— 需 设置↔桌宠 的「取参数 / 下发参数」协议 + 引擎 `setParameter` / `setExpression`；**交互第 1 条**（Alt+拖动模型 / 滚轮按指针缩放 / 穿透时 Alt+右键唤出 —— 用户已选**不吞键的全局 LL 鼠标钩子**）；**第 4 条**（**当前配置的模型目录**禁止 AI 读取，需新增「宿主→agent」路径黑名单通道）。**并已落地 L10 ① 的「手动」一半（2026-10-09）**：宿主新增 `screenshot_monitor`（抓**当前显示器**整屏 —— **宿主首次有显示器枚举**）+ 桌宠右键「**看一眼屏幕**」→ 抓前先亮可见标识 → 图作为附件走**现有对话链路** `startAIChat(query,[path])`（**不新开 API 通道**；聊天窗已开 / 未开都覆盖）；`vision` 关时明确告知；`pet 0.9.14`；规范见 `ai-spec.md` §4.5。**桌宠入口形态再改（2026-10-09 用户要求，`pet 0.9.15`）**：与 AI 助手 / 音乐同形态 —— `main.ts` 的 `executePlugin` 按 id 特判，搜索「桌宠」**直接开那扇设置独立窗**（带 key 的附属窗），**主窗不再有任何内嵌页面**（原 `launcherHtml` / `attachLauncher` 已删）；规范见 `ai-spec.md` §4.8。**并按顺序做「模型调试区」（2026-10-09，`pet 0.9.16`）**：引擎 `Live2DHandle` 新增 `setParameter` / `setExpression` / `resetExpressions` —— 参数**每帧保持**（在 `internalModel.on("beforeModelUpdate")` 里把 pin 住的值写回；依据是库的 update 帧序，见代码注释）；设置窗新增「模型调试」区块 = **9 条横向拉条**（头部左右/上下/倾斜、左右眼开、嘴张开、身体左右/倾斜、呼吸）+ 按 model3.json `FileReferences.Expressions` 生成的**表情按钮** + 「还原」；仅模型模式可用，桌宠未开时提示先显示；跨窗走 `pet-control` 的 `set-param` / `set-expression`。**并按顺序做「交互第 1 条」（2026-10-09，`pet 0.9.17`）**：宿主新增 **`WH_MOUSE_LL` 全局鼠标钩子**（`mouse_hook.rs` —— **不吞键**（一律 `CallNextHookEx`）+ **懒启停**（`pet_alt_rclick_watch(on)`，进穿透才装）；事件 `pet-alt-rclick{x,y}` **定向**发 `plugin-pet`；`plugin-pet` 窗 Destroyed 与应用退出都会卸钩子）；引擎加 `setOffset(dx,dy)` 与 `zoomAtPointer(factor,px,py)`（**指针锚点缩放**；`fit()` 保留「用户视图」，`ResizeObserver` 不再冲掉用户调好的位置/缩放）；桌宠窗 **Alt+左键拖动形象**、**滚轮按指针缩放**（并把新 scale 同步回设置窗拉条）、**穿透态 Alt+右键唤出菜单**（唤出时**临时关穿透**、菜单关闭后恢复）；`PetConfig` 增 `offsetX/offsetY`。**桌宠模型调试区再修（2026-10-09，`pet 0.9.18`）**：① 拉条的**数值文本改成可键入文本框**（照音乐调音插件的 `music-tune-num`：`type="text"` + `inputmode="decimal"`，Enter/失焦提交并夹范围，与拉条**双向同步**）；② **表情按钮改成开关态**（生效=高亮、再点=还原，同时只有一个生效）；③ **修掉「图片输入」被误判为关**：dev **没有 `config\ai.json`** ⇒ `AI_VISION` 从未被设置 ⇒ `get_ai_config().vision` 返回 `false` ⇒ 桌宠「看一眼屏幕」被判「模型未开启图片输入」。现改为 **`vision` 默认开**（`storage.rs::default_vision` + `get_ai_config`/`set_ai_config` 的 `unwrap_or(true)`）—— 面板早已撤掉该开关且恒写 `true`；见 `ai-spec.md` 规则 60。**桌宠再修（2026-10-09，`pet 0.9.19`）**：① 形象大小上限 **200% → 400%**（`PET_SCALE_MAX` 2→4、设置窗拉条 `max=400`）；② **表情状态持久化且按模型区分**（`PetConfig.expressions: {模型路径 → 表情 id}` —— 设置窗按当前模型恢复高亮、桌宠窗装载模型后自动应用，重开窗/重启应用都还在；之前 `activeExpr` 只活在设置窗闭包里，重开就丢）；③ 表情列表改为**从模型目录里实际存在的 `*.exp3.json` 读取**（新增宿主命令 **`list_expression_files(model_path)`** —— **只扫该模型同层目录的 exp3、不递归、刻意不做通用列目录**；引擎把「目录里有但 `model3.json` 未声明」的 exp3 追加进 `FileReferences.Expressions` 再交给 Live2D，`setExpression` 才真的切得动）。**本地决策模型（S1）与桌宠 AI 对话框（2026-10-09 用户新提出）**：**`local-model` 插件骨架已落地（2026-10-09）**：`app/src/plugins/builtin/local-model.ts` + `vite.plugins.config.ts` 的 `pluginEntries` + `build-plugins.ps1` 的 `$Plugins`（v0.9.0）；并**给清单生成器补上 `sidecar` 透传** —— 此前它**从没输出过** `sidecar`，磁盘插件的 sidecar 一直是断的（已重建 `pet` 验证不回归；BOM 守住）。`local-model-0.9.0.zip` 已构建，清单里 `sidecar` 与 `dependencies`(llama-server archive + sha256) 已确认写出。**未做**：只剩**市场发布**（`publish-plugins.ps1`，待 dev 验证后跑）。**2026-10-09 收尾已办**：① 模型的**自动下载入口** —— 用户定**强制随装下载**（`dependencies` 加一条 `type:"file"` + 本地实算 sha256；468MB 落在 `MAX_DEP_BYTES` 512MB 闸内）；② `LUNAC_LOCAL_MODEL_URL` 已由宿主下发 agent（`commands.rs` 的 `local_model_env()` 从插件清单读 sidecar `--port`）；**并修掉一个阻塞市场发布的硬伤**：清单有 `sidecar` 却 `permissions` 为空 ⇒ `plugin_market::validate_sidecar` fail-closed **整包拒装**（此前手工铺进 `Modules\` 才没暴露），已补 `process.spawn`。**2026-10-09 形态改造（用户定，插件 v0.10.0）**：`local-model` 由「可搜索 / 可打开的工具插件」改为 **headless 基础设施插件**（新判据 `kinds.ts::isHeadlessPlugin`：搜索里没有它、市场行不给「打开」，但**仍留在市场**可下载 / 卸载；**不复用** `MERGED_INTO`）；面板删除、开关搬到「**设置 → AI → 本地模型决策**」（状态落 `config\local_model.json`，宿主命令 `local_model_config_get/_set`；`local_model_env()` 受其门控 = 关则 agent 完全不介入）；sidecar 由**设置开关托管**（owner = 主窗；`sidecar.ts` 新增 `startPluginSidecar()/stopPluginSidecar()`；**设置路径改用 `startPluginSidecarTrusted()` —— 信任卡渲染在 `#results-list` 里会被设置布局吞掉，改为面板内「警告 + 确认」后直接落盘信任**；启动时前端静默 ensure）。**agent 侧一度误接成「输入门」并已撤销**：曾把 `decide()` 接到 `core-agent/src/main.rs` 的用户回合分支（`Skip` 就不发云模型）—— 用户 2026-10-09 **否决**：那是**桌宠侧**的形态，agent 侧要的是**重要节点上的审查 / 策展**（tools 输出是否跑偏 / 引用落点是否与真实代码与已改文件一致 / 方向是否偏离目标，**拦截并确认**），**不是拦消息**。core-agent 已回滚到「未接线」基线（`cargo test` **161 passed**），并已完成**可行性调研**（结论见 §4.1.7）。规范见 `ai-spec.md` 规则 93 / 94。**另加「测试连通」按钮（2026-10-09 用户要求）**：设置 → AI →「AI 模型」用**表单当前值**直打最小请求（宿主命令 `test_ai_connection`，不落盘）；该命令**不套** `validate_public_https_url`（AI 端点可为本机/局域网），见规则 94。**S1 第一刀已落地（2026-10-09）**：`core-agent/src/decide.rs` —— `Action`(Send/Skip) + `Decision{action,reason,fallback}` + `Decider` trait + **统一入口 `decide()`（fail-open 的唯一出口：后端 `Err` ⇒ 放行并记 `fail-open-backend-error`）** + 超时第二道闸 `within_deadline` + 桩后端 `Heuristic`（斜杠命令 / 无字母数字才 Skip，其余一律发）；**9 条单测**（后端故障必放行 / 拿不准也放行 / 空输入不发 / 桩后端不产生 fallback / 超时判定）。**当前无人调用是刻意的**（调用点 = L10 桌宠常驻输入），故模块级 `#![allow(dead_code)]` 并注明**接线后删**。① S1 **后端形态已定 = 形态 B**（子进程 `llama-server` + GGUF，**模型由用户自备**；本环境无法下载/运行，模型侧**不做实测**）；**交付形态 = 插件，进「额外插件市场」并留下载 / 卸载入口** —— 复用插件 `dependencies`（`type:"file"` 下载 llama-server.exe 与 GGUF）+ 清单 `sidecar` 段由宿主托管进程 + 「卸载 = 删目录」；**第一刀仍先把 `decide()` 边界 + fail-open + 日志/用量埋点** 立起来（后端可插拔：先桩后端 → 再接 llama-server）；② **用户问「一定要内置 llama.cpp 吗」→ 不必**：四档形态里**静态链接 llama.cpp 进 agent.exe（形态 C）最不划算**，真要上本地模型**优先「子进程 `llama-server` + 一个 GGUF」（形态 B，复用 `child_job`/sidecar/依赖下载）**，最轻是外部服务（D，但不满足开箱即用）；③ **阻塞**：B/D 都要下载并跑起 0.5–0.6B GGUF（约 300–450MB），当前环境**无法下载/运行** ⇒ S1 的模型侧**无法实测**；④ **桌宠右键菜单加「AI 对话框」**：桌宠窗内换出聊天界面（**底部方形文本框 + 上方气泡只显示最后一条输出**、`thinking` 调**低**），**语音输入等其它输入后续复用同一接口**；⑤ 该「发送决策」守的是**桌宠常驻输入**（依赖先做 **L10 桌宠×AI**）
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
- **落地**：新增 `core-agent/src/peers.rs`（`PeerBus` 登记处 + 线程本地「我是谁」+ 五条单测）；`tools.rs` 增 `ListPeers` / `SendMessage` 两件内置工具（总数 15 → **17** → 19：后续 2026-10-10 + `PetControl`、2026-10-11 + `NotebookEdit`）、进 `parallel_safe` 的只有 `ListPeers`；`main.rs` 的 `run_agent_tool` 用 **RAII guard** 注册 / 注销 peer，`run_subagent` **每轮开始** drain 收件箱追加成 `user` 消息；前端黑名单候选名单补两条。**不违反规则 54** —— 消息只进收件方子代理自己的 history。
- **契约与六条纪律**：`ai-spec.md` §3.5「多代理通信」+ §11 **规则 85**；并发类的运维纪律补进 `code-rules.md` 预检 **#28**。
- **验证**：`core-agent` `cargo test --bins` **140 passed / 0 failed / 2 ignored**（+6 条：`peers` 模块五条 + `tools::peer_tools_are_registered_and_ungated`）；`npx tsc --noEmit` exit 0。
- **真机端到端已验收（2026-10-05）**：真端点（`deepseek/deepseek-v4.1-flash`）+ **从仓库新构建的 agent 二进制**，一轮里让模型并发派两个 `Agent`。日志铁证：`固定前缀 tools=17个/10764字`（两件新工具已进工具池；此为 2026-10-05 日志，后续 + `PetControl`（2026-10-10）、+ `NotebookEdit`（2026-10-11）⇒ 现为 19 件）、`子代理并行批 2 条（并发上限 3）: Agent, Agent`、`tool ListPeers ok` / `tool SendMessage ok`、**`子代理 task-1 收到 task-2 的消息（27 字）`**。语义侧：A 的 `ListPeers` 看到 `- task-1 — peer-B` 与 `- task-2 (you) — peer-A`（自己标 `(you)`），`SendMessage` 到 `task-1` 回「Message delivered to task-1 (27 chars)」；B（跑完 `ping` 后）在下一轮读到并**逐字引用** `PING-FROM-A-42 hello from A`。B 的第二次 `ListPeers` 只剩自己 —— 与「已结束的 peer 退出列表」的设计一致。
- **两条运维事实（排查时别踩）**：① **装着的 `D:\Lunac\agent.exe` 是旧构建**（2026-10-04 01:50），不含本批两件工具；要用得先 `cargo build` 再重打包。验收时的判据是 `init.tools` 里有没有 `ListPeers` / `SendMessage`（旧二进制是 15 件、新的是 17 件；此为 2026-10-05 验收时的数字，后续 + `PetControl`（2026-10-10）、+ `NotebookEdit`（2026-10-11）⇒ 现为 19 件）。② 中转端点（`api.moonobscura.com`）对**非流式**请求（子代理/复盘走的是 `stream:false`）偶发长时间挂死后失败（实测一次 `子代理请求发送失败` 挂到 **158s**），模型重试即成功 —— 是端点侧可靠性，不是本仓缺陷。

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

**M2-17. 新建对话与上个对话的「模型级隔离」——移除「往期会话索引」自动注入**（2026-10-07，用户报「思考过程中会出现上个对话的相关记录」）。

- **用户口径**：「如果新建对话时，这个对话与上个对话模型应该有隔离」。
- **诊断**：新建对话本身没问题（`newConversation()` 会 `stop_cli` 重启 agent，agent 内存里的 `history` 确实清空）。**真凶是「往期会话索引」自动注入**（A2，`LUNAC_HISTORY_INDEX` 默认开）：agent 每次启动都会从 `chat.db` 取一份「最近 20 条会话的标题 + 首句」窄表注进 history 开头；而 agent 每次新建对话都会重启 ⇒ 重启后取到的快照**必然包含刚结束的那个会话**，模型思考里就带上了个对话的痕迹。实证：release 日志 `D:\Lunac\temp\logs\lunac-2026-10-07.log` 每次启动都打 `往期会话索引已注入（640~848 字）`；`chat.db` 里最新会话正是刚聊过的几条。
- **改法（用户选「彻底关掉索引注入」）**：`core-agent/src/main.rs` 删除 `history_index_block` / `history_index_enabled` / `HISTORY_INDEX_ENV`；`context_block_message` 改为只承载**长期记忆**；宿主侧 `lunac/history_index` 自定义方法、`chat_db::recent_sessions` / `render_digest`（含单测）一并删除（**彻底**，不留死代码）；`SessionSearch` 按需检索保留（`lunac/history_search`）。单测改名 `memory_lives_in_the_context_block_not_the_system_prompt`。
- **顺带修一个竞态**：`newConversation()` / `ensureChatLog()` / `startAIChat()` 的 `!pluginActive` 分支此前只清 `chatHistory`、**没清 `pendingAgentHistory`** —— 若恢复会话时 CLI 未就绪，上一段会话会挂起、等新对话的 agent 就绪时被灌回去。三处新会话入口现在一律 `pendingAgentHistory = null`（`app/src/main.ts`）。
- **规则**：见 ai-spec §11 规则 53（新增「禁止恢复任何形式的自动注入往期会话内容」）/ 56（记忆块不再与索引同块）。
- **验证**：`cargo check --tests`（core-agent，无新增 warning）、`cargo test`（148 passed）；`cargo check --tests` + `cargo test --bins chat_db`（12 passed）；`npx tsc --noEmit` 通过。

**观察：长期记忆功能没有缺失，只是从未写入过**（2026-10-07，用户问「长期记忆我记得已实现，是否功能缺失」）：`Remember` 工具在工具池里、`lunac/memory_read` / `memory_write` 通道通、复盘 fork 已接线（release 今日日志的工具清单含 `Remember`）。但 release 的 `<exe 根>\ModuleData\memory\MEMORY.md` **不存在**、日志里也**没有任何 `后台复盘 … 启动`** ⇒ 两个写入侧都没触发过：`Remember` 需模型主动调用；复盘按 `LUNAC_NUDGE_INTERVAL`（默认 **10** 次**用户提问**）触发，而**计数在 agent 进程内、每次新建对话 / 换模型 / 换思考档都重启清零** ⇒ 单次会话不到 10 问就永远不触发。**结论：非缺失，是触发门槛在「频繁重启」下几乎够不着**（是否调低门槛待用户决定）。

**M2-18. 「AI 自建东西该放哪」的四分法**（2026-10-07，用户报「做 PS 插件时模型把整个项目直接建在了 `skills\` 下」）。

- **用户口径（四类各有各的家）**：① Lunac 插件 → `Modules\`；② AI 技能 → `skills\`；③ MCP 工具 → `tools\`；④ **其它一切**（独立工具 / 脚本 / 工程，例如 PS 的 CEP 面板）→ **工作目录 `transStorage`**。
- **根因**：`env_block()` 只告诉模型「skills 在哪、Modules 在哪」，**没写清四类目录各自的用途**，也没有「不属于这三类的往工作目录放」这条 —— 模型看到一个「能写的应用目录」就顺手用了。
- **改法**：`core-agent/src/main.rs::env_block()` 新增 `placement_line`（四类逐条列路径 + 判据 + 「不要自创第五个目录」；`tools\` 从 `LUNAC_SKILLS_DIR` 的父目录推出，**不新增环境变量**）；三份规范 README（`agent-templates\{skills,modules,tools}\README.md`）各加一节「⚠️ 先分清该放哪」。
- **部署**：已拷到 **dev 与 release 两份实际目录** —— `app\src-tauri\target\debug\{skills,Modules,tools}\README.md` 与 `D:\Lunac\{skills,Modules,tools}\README.md`（这两处的 README 平时由 `build-release.ps1` 从 `agent-templates\` 铺，dev 侧要手工拷）。
- **规则**：见 ai-spec §20.1「AI 自建东西该放哪的四分法」。
- **验证**：`cargo test`（core-agent **148 passed**）。
- **待办**：`D:\Lunac\skills\live2d-split`（那个 PS CEP 面板）正是本次误放的实例 —— 按四分法应挪去工作目录。**用户 2026-10-07 决定：暂不搬，等插件实机验证完再搬**（免得搬完又得重装 CEP 副本）。搬的时候要一起处理：`config.json` 里的 `engine` 绝对路径、已装到 `%APPDATA%\Adobe\CEP\extensions\` 的那份副本（重跑 `install.ps1` 即可）；`tests\common.py` 用相对路径，不受影响。

**M2-19. 桌宠 × AI：独立界面 + AI 对话框 + 主动反应 + 用量来源归因**（2026-10-09，实施 L10 ②③；承接 ①「看一眼屏幕」）。

- **入口形态对齐音乐 / AI 助手**（用户口径）：进搜索打开桌宠 = **直接开独立界面窗**（`executePlugin` 里对 `id === "pet"` 直接 `open_plugin_window({key:"settings"})`）——**主窗里不再有任何桌宠页面**。
- **调试区（设置窗）**：① **数值全部改成可键入文本框**（照音乐调音插件：`change` 才提交 + 焦点还原）；② **表情按钮做成开关态**（当前选中项高亮 `.on`、关闭态恢复普通样式）；③ **形象大小上限 2 → 400%**（`PET_SCALE_MAX = 4`）；④ **`activeExpr` 持久化 + 按模型分开**（`PetConfig.expressions: Record<模型路径, 表情id>`，重开设置窗时按当前模型恢复高亮）；⑤ **表情列表从实际目录读**（新增宿主命令 `list_expression_files(model_path)` 扫模型**同层**目录的 `*.exp3.json`，与 `model3.json` 声明项合并 —— 未声明的 exp3 由引擎追加进 `FileReferences.Expressions`）。
- **AI 对话框**（用户选「对话框 + 主动反应」）：桌宠右键菜单新增「AI 对话框」→ **桌宠窗内** `#pet-chat`（底部方形输入框 + 上方**只显示最后一条**回复的气泡，thinking 调低）；统一发送入口 `sendToAI(query, files?)` → `emit("pet-chat-send", {query, files?})`。链路契约见 `ai-spec.md` §4.5 的 **`pet-chat-send` / `pet-chat-reply`** 段：**take 语义**保证「只发一次」、复用**现有对话链路**（`startAIChat`）、**不新开 API 通道**。
- **主动反应**（`PetConfig.proactive`）：默认**关**；间隔（15/30/60/120 分，**自绘下拉**）+ 每日上限（1–200）；按**自然日**计数、触顶自停、跨日自动恢复；`plugin-window-visibility` 不可见即暂停（**看不见时绝不烧 token**）；单轮 `proactiveBusy`（`pet-chat-reply` 结束，另有 90s 看门狗）。
- **模型调试区「参数」持久化**：`PetConfig.params: Record<模型路径, Record<参数id, number>>` —— 设置窗恢复 / 写回、桌宠窗装载后应用（此前只活在设置窗 UI 侧，重开即丢）。
- **用量来源归因**（承接 L10 ③）：`UsageRecord.source` / `UsageModelTotals.source`（`storage.rs`，`#[serde(default)]`）；桌宠那一轮在 `startAgentChat` 开头由 `petTurnActive` 认领成 `currentTurnFromPet` ⇒ `turnUsageSource = "pet"`，本轮记录全带 `"pet"`。成本面板加「**来源**」下拉（`全部 / 桌宠 / 普通聊天`，`usage-cost.ts` 的 `sourcePartition`）—— **与「日期」下拉同一形态 / 同一容器**（`.cost-range-select` / `.cost-range`）；金额仍走既有 `dayCost`，**不另写一套算法**。（§11 规则 5 禁的**原生 `<select>`** 限定**透明窗口**，本面板在设置窗里，与既有「日期 / 模型」两个下拉同源，故沿用。）
- **插件版本 / 构建**：`pet 0.9.14 → 0.9.20`；`build-plugins.ps1 -Plugin pet` 跑通（zip 310.5 KB、dev 的 `target\debug\Modules\pet\` 已同步）。**注意 `build-plugins.ps1` 必须 UTF-8 with BOM**（本轮又被编辑器抹掉过一次，已用 `[IO.File]::ReadAllBytes` 补回）。
- **验证**：`npx tsc --noEmit` exit 0；`cargo check` exit 0（仅既有 18 条 dead-code 警告）。
- **仍未做**：**第 4 条（当前模型目录禁止 AI 读取）**、**自动 / 定时抓屏**（需「默认关 + 频次上限」，属 L10 ② 前置）；主动反应的已知限制 = 聊天窗未开时会**前置聊天窗**、一轮只按 `pet-chat-reply` 归零。

**M2-20. 桌宠：穿透开关对齐 + 桌宠专属独立人格**（2026-10-09，用户两条要求）。

- **穿透开关「对齐」**（用户报「在模型界面切换穿透只有打开没有关闭」）：根因 = 桌宠右键菜单那条是**单向动作**（`data-act="through"` 恒定 `clickThrough=true`、文案写死「开启鼠标穿透」），且两扇窗之间**没有回传**（设置窗改了桌宠窗不知道、桌宠窗改了设置窗也不知道）。改法：① 设置窗的复选框 + 桌宠菜单的单向按钮，**统一改成切换按钮**「鼠标穿透：开 / 关」（两处共用 `throughLabel()`）；② **双向同步** —— 设置窗改 ⇒ `emit(set-click-through)`；桌宠菜单改 ⇒ `emit(click-through-changed)`，桌宠窗开窗时再广播一次当前态 ⇒ 两处状态永远一致（用户口径：桌宠设置与模型之间的所有参数都要对齐）。
- **桌宠专属独立人格（方案 A：前端按轮注入）**：新增 `config\pet-persona.md` + 宿主命令 `get_pet_persona` / `set_pet_persona`（复用 `storage::validate_persona_text` 的校验）。前端在 `startAgentChat` 里、当 `currentTurnFromPet` 为真时把这段文本拼进**该轮的用户消息**。⇒ 只作用桌宠发起的轮次（AI 对话框 + 「看一眼屏幕」）、**即时生效、无需重启 agent**、不影响普通聊天。**为什么不用固定前缀**：系统提示词是启动时冻结的（改它要重启、还会波及所有对话），而桌宠轮在前端已被 `currentTurnFromPet` 唯一标识 ⇒ 拼进消息天然「按轮」。契约见 ai-spec §4.5。
- **人格编辑区**：桌宠设置窗**最下方**（`#pet-persona`）—— textarea + 保存 / 清空 / 重启 AI + 状态行；与 AI 侧人格窗口**独立**（不同文件、不同生效方式）。
- **桌宠侧「重启 AI」通道**：桌宠设置窗无 `__lunac_reload_agent` 桥 ⇒ `emit("pet-agent-restart")`，**主窗**（`IS_CHAT_WINDOW` 挡一道）接住跑同一套「存会话 → stop_cli → start_cli」，回 `pet-agent-restart-done`。**如实记**：方案 A 本不需要它（保存即时生效），这条只是与 AI 侧**功能对齐**。
- **插件版本**：`pet 0.9.20 → 0.9.21`；`build-plugins.ps1 -Plugin pet` 跑通（zip 311.2 KB、dev 的 `target\debug\Modules\pet\` 已同步）。
- **验证**：`npx tsc --noEmit` exit 0；`cargo check` exit 0（仅既有 18 条 dead-code 警告）。

**M2-21. 音乐插件：队列 / 自定义队列、播放位置恢复、提示与拖动视觉、AI 供应商 curl 一键导入**（2026-10-09，用户七条）。

- **① 本地队列按钮（音量右侧）**：本地底部控制栏 `#music-l-queue` 与本地小窗 `#music-lp-queue` 各加一枚队列按钮；点击切到本地页并打开**占满内容区的「队列页」**（同本地歌单那种独立页面，非下拉浮层）。
- **② 自定义队列**：**状态按钮 `#music-local-qmode` 在顶部工具条**（2026-10-09 **二次修订**：用户口径「**搜索栏右边、重试按钮左边**」—— 原先塞在本地页那排图标里，已挪走；与搜索栏同隐显，`localPage` 时一起隐藏）。开启时高亮（`.on` + `aria-pressed`）；开启时 **「设置」齿轮 `#music-setup-toggle` → 「添加队列」**（本地页时 `placeGear` 会把齿轮挪进本地那排，文案收敛在 `applyGearQueueState`）、**本地页 `⟳` → 「退出这个状态」**（三处视觉收敛在 `applyQueueModeUi`）。点「添加队列」→ **页面中心浮层** `#music-queue-dlg`（自定义队列名 + 多次「添加文件…」跨文件夹多选、按路径去重），确定后落成当前队列并复用既有本地播放（`startLocalPlay` / `advanceLocal`，**未新造播放器**）。
- **③ 播放位置**：新增 `scrollLocalListToCurrent`（**不用 `scrollIntoView`** —— 那会把整个面板一起滚，改按容器 `scrollTop` 算）—— 起播 / 换曲滚到当前项；重开插件按 `readLocalLast` 恢复的当前曲目滚到「上次那首」。
- **④ 底部警告区不占位**：那块就是 `#music-msg`（原来 `min-height` 占一行的静态块）⇒ 改 `absolute` 浮层、`pointer-events:none`、渲染在进度条附近；`setMsg()` 统一入口 + **5s 自动消失**（`MSG_TTL_MS 8000 → 5000`，换字重置计时）。
- **⑤ 拖动视觉条高度减半**：`.music-bp.music-dragging` 展示条 `18px → 9px`、`.music-progress.music-dragging` `21px → 10.5px`（居中，其它视觉不变）。
- **⑥ 设置里「测试连通」与「保存」等高**：新增 `#settings-test-ai-btn` 的 padding / 字号 / 圆角覆盖，**以保存按钮盒模型为准**（**不改**被市场安装等处复用的 `.settings-install-btn` 本体）。
- **⑦ AI 供应商 curl 一键导入**：`parseCurlConfig()` 解析 URL / `Authorization: Bearer` / `--data` 里的 `model`；「一键添加配置」入口 + 粘贴框 ⇒ 原地写入供应商下拉 / baseurl / key / 模型 + `set_ai_config` 落盘 + 记入 `lunac-custom-providers`。**取舍**：`base_url` 只取 URL 的 **origin**（与既有「预设不带 /v1 与业务路径」同语义），被丢弃的路径用 `settings.curl_path` 明确提示；解析不到 `model` 时**不自动保存**、明确提示（`set_ai_config` 要求 url/model 非空）。
- **插件版本**：`music 0.9.27 → 0.9.29`（`0.9.28` 打完后按用户「状态按钮挪到工具条」的口径又改了一次，故再 +1）；`build-plugins.ps1 -Plugin music` 跑通（zip 56.7 KB、dev 已同步）。
- **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin music` exit 0。
- **仍未做 / 待验**：**未起 dev 实例做真机点击验收**（浮层居中、本地图标 12px 归一、滚动位置、⑤ 的视觉、⑦ 的解析都要真机点一遍）；**未发布市场**（`publish-plugins.ps1` 未跑，市场里 music 仍是旧版）。

**M2-22. 桌宠 × AI 第二轮：独立线程 + 历史分页 + 语音朗读 + 自动弹窗 + 间隔自定义 + 对话改渲染**（2026-10-09，用户六条）。

- **① 对话独立成线程 + 历史分页 + 单独存储**：`ChatSession` 加 `source`（空串 / `"pet"`；chat.db 的 `sessions` 加列，照 `todos` 那样 `ALTER TABLE` 迁移）。聊天窗维护**两套平行线程状态**（普通 / 桌宠：history、session id、用量、过程、快照、待办），轮次开始时 `switchThread` 并把该线程 history 经既有 `set_history` 灌给 agent（`agentThread` 记当前装载的线程，agent 重启即置空、强制下轮重灌）；落库带 `source`，读写基于库里全量列表 ⇒ 两类会话互不冲掉。AI 历史（左侧抽屉 `renderDrawerHistory` 与 `showChatHistory`**两处**）各加「对话 / 桌宠」分段，按 `source` 过滤。契约见 `ai-spec.md` §4.5。
- **② 主动反应间隔可自定义（更小）**：原来只有 15/30/60/120 四档自绘下拉 ⇒ 改成**可键入数字框**（`PROACTIVE_INTERVAL_MIN/MAX` = **1..1440 分钟**），下拉与其 CSS 一并删除。
- **③ 桌宠默认人格**：`storage::DEFAULT_PET_PERSONA`（女性 / 简洁 / 温柔 / 口语化），首次读 `get_pet_persona` 时落盘成 `config\pet-persona.md`（**只在文件不存在时写**，不覆盖用户清空过的）。人格仍**按轮注入** ⇒ 线程分开后天然只作用桌宠线程。
- **④ 桌宠发消息自动弹出 AI 窗（前台修复）**：`open_plugin_window` 对 `chat` 窗在「复用」与「新建」两条路上都调 `hotkey::force_foreground_hwnd`（`AttachThreadInput` + `SetForegroundWindow` 借前台线程输入优先级）—— Tauri 的 `set_focus()` 对**后台进程**会被 Windows 前台锁拦下，表现为「窗开了却躲在别的窗口后面」。
- **⑤ 对话改渲染**：`#pet-chat` 不再是居中的卡片 ⇒ 整层铺满窗口（`pointer-events:none`），**输出贴上沿、输入贴下沿**，不再挡模型。
- **⑥ 语音朗读（TTS）**：新增宿主 `tts_speak`（`app/src-tauri/src/tts.rs`）—— WinRT `SpeechSynthesizer`（离线、系统自带）合成 WAV（优先中文语音），`DataReader` 读字节 → 临时文件 → **rodio** 播放（与音乐播放各开各的流）；整段在后台线程（`RoInitialize` + `.get()`）。Cargo 只加**同一个 `windows` crate 的 feature**（`Media_SpeechSynthesis` / `Media_Core` / `Foundation_Collections` / `Win32_System_WinRT`），**无任何新下载**。桌宠窗收到**最终回复**后朗读（`PetConfig.tts` 默认 **true**，设置窗可关）。
- **⑦ 桌宠轮「不弹窗、只在桌宠窗显示」（2026-10-09 用户追问后补）**：`open_plugin_window` 加**可选 `hidden`**（缺省 false ⇒ 既有调用方逐字不变）——桌宠发起的那一轮（`handlePetChatSend` 主窗分支 + 「看一眼屏幕」）传 `hidden:true`：窗口**已存在**就只 `emit("plugin-window-input")`、不 show/focus/强制前台；**不存在**就 `.visible(false)` 建窗、不聚焦（WebView 照常加载执行）。配套：`main.ts` 加**唯一判定** `suppressChatRender()`（`currentTurnFromPet || petTurnActive`）——桌宠轮把会话流写进**隐藏容器**（`chatFlowHost()`）、用户气泡 `appendUserMsg` 早退 ⇒ **聊天窗可见区一个节点都不新增**，对话只在桌宠窗显示；普通轮路径一字未变。
- **⑧ 桌宠窗放大、模型定尺（2026-10-09 用户追问后补）**：清单 `window` `300×400` ⇒ **`320×560`**，但 `.pet-stage` 改成**定尺 `300×400` 居中**（**模型大小不变**）；多出来的上下各 80px 给**输出（贴上沿）/ 输入（贴下沿）** —— 对话不再遮挡模型。
- **⑨ 桌宠轮一律关 thinking（按轮覆盖，2026-10-09 用户要求）**：新增**按轮思考覆盖** —— 用户消息可带 `"thinking":"off"`（agent 侧 `thinking_override_from()`）⇒ 该轮强制 `Disabled`，**与全局开关 `LUNAC_THINKING` 无关**；覆盖时那条 400 降级**只在本轮内**前进、**不写回** `cfg.thinking`（免得桌宠轮把普通聊天已跑通的形态改掉）。前端在 `currentTurnFromPet` 时带上它 ⇒ **桌宠闲聊不烧推理 token**；普通轮不带 ⇒ 用全局档。单测 `per_turn_thinking_override_only_turns_things_off` 钉住「只认 off」。**如实记**：thinking 只改生成参数（`thinking` 形态 + `max_tokens`）、**不触发工具调用** —— 用户问的「桌宠别调工具 / skills」是另一件事（**未做**，需要收窄工具集或人格约束）。**部署**：改的是 core-agent ⇒ 已跑 `build-core.ps1` 重编 `agent.exe`（3.1 MB，`cargo test` 162 passed）。
- **⑩ 桌宠主动对话「刷新模型 / 丢失回复」修复（2026-10-10 用户报障；两轮才修透）**：根因是 `plugin-window-input` **两端都要定向**。
  · 第一轮（只改宿主，不够）：`plugin_window.rs` 复用路径用 `existing.emit("plugin-window-input", …)` —— tauri 的 `Emitter::emit` 默认是 `manager().emit()` = **广播**；改成 `app.emit_to(label, …)`。
  · 第二轮（真正生效的一环）：**消费端 `plugin-window.ts` 用的是裸 `listen`**（`@tauri-apps/api/event`），其 target = `EventTarget::Any`；而 tauri `match_any_or_filter` 对 `Any` **恒匹配** ⇒ 宿主的 label 过滤对它完全失效。改用 `getCurrentWindow().listen(...)`（target = 本窗 label）后，聊天窗被复用前置时那条入参不再打进桌宠窗。
  · 症状对应：桌宠主动反应 → 桌宠窗被**整页重挂**（重载模型 = 用户说的「刷新模型」；新闭包把 `proactiveBusy` 清零 ⇒ 回复到达被判非主动轮 ⇒ 气泡不弹 = 「没输出」）。`main.ts` 聊天窗那条早已是 `WebviewWindow.listen`，所以只有 `plugin-window.ts` 漏了。
  · ⚠️ **纪律（进 ai-spec §4.5）**：给插件窗传参 = 宿主 `emit_to(label)` + 消费端 `getCurrentWindow().listen`；不用无 label 的 `emit`、不用裸 `listen` 订阅定向事件。
- **⑪ 桌宠对话布局定稿 + 三种情况不显示输入框（2026-10-10 用户口径）**：窗口 **`320×510`** = 输出带 75 + 模型 300×400 + 输入带 35（**紧贴、无空隙**，整体变大但模型不变）。`.pet-root` 改 `flex-direction:column`；输出框 **宽 320 × 高 75**（贴上沿）、输入行 **高 35**（贴下沿）；两条带**始终占位** ⇒ 模型不跳。显隐靠 `chat-on` / `chat-input-on` 两个类（`setChatOpen(on, focusInput, showInput)`）。**语音输入 / 主动询问 / 抓取屏幕图像**三种「桌宠发起」的情况**不显示输入框** —— 只弹输出、`PROACTIVE_BOX_MS`(6s) 后自动收起（`flashOutputOnly()`；`pet-chat-reply` 一律调它，用户已开着输入对话时是空操作）。语音输入本身未实现，入口已就绪。
- **⑫ 桌宠热键「无法按键生效」诊断 + 触发链路（2026-10-10 用户报障）**：用户报「无法按键生效」，查下来是**整条触发链路缺失**，不是某个配置值写错。
  - **根因（三条叠加）**：① **没有任何「按下按键 → 执行动作」的派发器** —— `hkList` 只用于渲染 / 编辑 / 导入；唯一的 keydown 监听是**录键模式**（`hkCapturing` 才工作），只往 `keys` 字段写字、不执行动作；② 数据层 `lunac.hotkeys.json` 里 31 条 `keys` **全是空串**（早期导入只建条目、不绑键）；③ `HotkeyAction`（motion/expression/param/artmesh/part/physics）**没有落到模型上的执行端**（`live2d-engine.ts` 早有 `playMotion`/`setDrawableOpacity`/`setPartOpacity`/`setPhysicsEnabled`，但无调用方）。
  - **触发方式（用户 2026-10-10 裁定）**：**仅 AI 触发**（本地不做按键监听）；**导入时自动分配默认键**；并**先给一个点击「测试」按钮**用于预览。按键自此只作 AI 侧标识，不再是用户按的快捷键。
  - **实现**：① **执行端** `pet.ts` 的 `attachPetWindow` 加 `runHotkey(action)`，六类动作落到 `l2dHandle`（`expression` 的 `clear` 走 `resetExpressions`；`param` 的 `add` 走新加的 `getParameter`）；② **触发通道** 沿用 `CTL_EVENT` 广播 `{type:"hotkey-run", action}` —— 设置窗（无实例）→ 桌宠窗（有实例）；③ **测试按钮**：`renderHotkeys` 每条加「测试」（`data-hk-run`），点击前先 `pingPet()`，桌宠没开时给明确提示而非「点了没反应」；④ **默认键**：`pet-hotkeys.ts` 新增 `defaultKeysForIndex(i)`（小键盘 1..9 × 7 层修饰键 = 63 个不重复）；导入时与 `refreshHotkeys` 加载**空键**时各补一次（`assignDefaultKeys`，只填空、不覆盖已录的）。⑤ `live2d.ts`/`live2d-engine.ts` 新增 `getParameter(id)`（读运行时 Core 的 `parameters.values`，退回 `heldParams`）。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（zip 316.3 KB、dev 已同步，v0.9.27）。
  - **未做**：AI 触发工具（宿主命令 + `core-agent` 工具，需重编 `agent.exe`）。
- **⑬ 热键页「模型选择无法识别模型」修复 + 描述精简（2026-10-10 用户报障）**：
  - **根因**：换模型后热键页**不刷新** —— `pickShape()`（更换形象）与「取消配置」都只 `emit(CTL_EVENT,"reload")`（那是发给**桌宠窗**的），设置窗自己的 `refreshHotkeys()` 没重跑 ⇒ 目标下拉还列着**上一个模型**的项或空（`hkTargets` 只在 attach 时算一次）。
  - **修法**：`pickShape()` 末尾与 `#pet-clear` 分支各补 `void refreshHotkeys()`；`hkOptList` 把**当前值并进选项**（枚举不全时已存目标仍照常显示、不被清空），占位文案由「（空 —— 先在桌宠里选模型）」改为「（无可用项）」（原话会误导：模型其实已选）。
  - **描述精简（`code-rules.md` §13.5 ⑤）**：删掉热键区下方那段多句 `.pet-note`（>1 句 ⇒ 先删），把「存哪 / 导入范围 / 默认键 / 按键仅 AI 触发」压进**导入按钮的 `title`**。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（zip 316.3 KB、dev 已同步，v0.9.28）。
- **⑭ 导入表情真正「应用到当前模型」（2026-10-10 用户纠正：「表情完全没有应用到当前模型」）**：⑫ 里只做了「列得出 + 可触发」，但**引擎根本不认识**那些导入的表情 ⇒ `setExpression(name)` 是空操作。
  - **根因**：库的 `model.expression(id)` 按 `settings.FileReferences.Expressions[].Name` 查表；引擎只会登记 ① `model3.json` 声明的 ② **模型文件同层**、`list_expression_files` 扫到的 `.exp3.json`。而热键导入走的是 `list_exp3_files`（**递归**、用户自选文件夹）⇒ 表情在**子目录 / 别处**时引擎扫不到、不登记，触发自然不生效。
  - **修法**：`MountOptions` 加 `extraExpressions: {name,file}[]`；桌宠窗 `paint()` 挂载前用 `collectHotkeyExpressions(modelPath)`（读 `lunac.hotkeys.json` 里带 `file` 的 `expression` 动作，按绝对路径去重）交给引擎；`live2d-engine.ts` 把两个来源（同层 extra + `extraExpressions`）**按 asset URL 去重**后一起追加进 `FileReferences.Expressions`（无名 / 无文件的跳过）。导入成功后补一条 `emit(CTL_EVENT,"reload")` ⇒ 桌宠窗重挂、新表情立刻可用。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（zip 316.5 KB、dev 已同步，v0.9.29）。
- **⑮ 桌宠设置窗分页 + 调试区列全部参数 + 控件对齐主题（2026-10-10 用户三条）**：
  - **① 热键 / 模型调试 分成两个独立页签**：照 `.music-tab`（`.chat-hist-tab` 同源）的胶囊芯片语言，新增 `.pet-tabs` / `.pet-tab` / `.pet-tabpage`；设置窗里「模型调试」（默认）与「热键」两块各进一个 `[data-pet-page]`，点页签由 `attachSettings` 的委托切换（互斥显示、`on` 选中态）。页内的块不再各画上分隔线（由那排页签承担）。
  - **② 调试区列出该模型全部参数 + 物理输入/输出**：原来是写死的 9 个常用参数 ⇒ 改成**运行时 id 表优先**（`readCoreIds` 读桌宠窗写进 localStorage 的那份，= 模型全部参数；已知项按静态表顺序排前，其余原序）；拿不到运行时表时退回静态 9 个（保证「没显示过桌宠」也能调）。物理参数从 `.physics3.json` 的 `Input/Output` 标出「物理输入 / 物理输出」小标签（与热键页同一份 `enumerateTargets` 枚举）。
  - **③ 参数取值范围**：`Live2DHandle.listCoreIds()` 增 `parameterRanges`（Cubism Core `parameters` 的 `minimumValues`/`maximumValues`/`defaultValues`，与 `ids` 同序）—— 拉条按真区间定 min/max，`step` 按跨度自适应（≤2 用 0.01、≤20 用 0.1、否则 1）；数值框夹取保留 3 位小数（旧实现 `Math.round` 会把 -1..1 的参数全压成整数）。范围清单改由模块级 `debugMeta` 承接（`restoreParams` / `commitParamNum` 不再依赖静态表）。
  - **④ 拉条 / 按钮对齐主题**：`#pet-scale` 原先是**裸原生 range** ⇒ 挂上 `.pet-range`（与调试拉条同一套 themed 样式）；热键行的「点这里按键 / 测试 / 删除」补 `.pet-btn`（原来只有颜色、退化成原生按钮），删除按钮 hover 的红色单独钉住（否则被 `.pet-btn:hover` 的 accent 盖掉）。新样式只用主题变量（`--accent` / `--accent-bg` / `--accent-border` / `--border-glass` / `--text-dim`），无裸 `rgba(...)`。
  - **i18n**：`pet.hotkey_title` / `pet.debug_phys_in` / `pet.debug_phys_out`（5 语言）。
  - **踩坑如实记**：CSS 注释里写了反引号 ⇒ 打断 `PET_CSS` 模板字符串、连报 4 条假错（含两处「Cannot find name」）—— 预检 #15 / #39⑩ 又一次命中，已改为不带反引号。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（zip 317.5 KB、dev 已同步，v0.9.30）。
- **⑯ 表情改「可叠加开关」+ 目标自动解析 + 调试区折叠表情参数（2026-10-10 用户四点）**：
  - **① 表情必须可叠加（用户口径 + VTS 实证）**：原先走库的 `model.expression(id)`，**互斥** —— 切下一个会把上一个冲掉（用户报「点击其他动作，之前的全消失」）。查证 [live2d.wiki.fc2.com](https://live2d.wiki.fc2.com/m/wiki/VTubeStudio%E7%94%A8%E3%83%A2%E3%83%87%E3%83%AB)：VTS 的 exp3 是「**变化的值会保留**、同一个键再按一次 = 关」。**落点**：`Live2DHandle.setExpressionLayer(key, params|null)` —— 引擎里加 `exprLayers: Map<key, Parameters[]>`，在 `beforeModelUpdate`（物理后、渲染前）**逐层施加**（读当前值 → 按 `Blend`：Add 加 / Multiply 乘 / 否则覆盖），排在用户「保持」参数**之下**。多条共存、各自可关。
  - **② `.exp3.json` 的 `Type` 恒为 `Live2D Expression` ⇒ 归到 `expression` 是对的（用户 2026-10-10 追问）**：VTS/ Cubism 的 exp3 顶层 `Type` **没有区分度**；能区分的是 `Parameters`（改了哪些参数 / 什么 Blend）。VTS 侧对应表情的动作类型只有 “Expression on/off” ⇒ 所有 `.exp3.json` 一律归 `expression`（不是识别不准）。动作类型标签 `切表情 → 表情开关` 以对齐语义。
  - **③ 「测试」改状态按钮**：每行按钮 = **开 / 关**（`pet-btn-toggle` 的 on 态），点一下开、再点一下关。真正的施加 / 撤销在**桌宠窗**（`hotkey-toggle` → `toggleHotkey`：`expression` 走叠加层 `setExpressionLayer`；`artmesh`/`part` 开=设该值、关=回到 1；`physics` 开=按其 `enabled`、关=取反；`param` 开=设值、关=回 0；`motion` 开=播、关=无）。状态只在桌宠窗维护，改完广播 `hotkey-states`，设置窗据此对齐按钮（**只改按钮不整表重画**，免得名字框掉焦点）；桌宠窗收到 `ping` 时也回播一次。
  - **④ 表情行的 `target` 不吃文件名**：新增 `resolveHotkeyTargets` —— 按 `.exp3.json` 的**绝对路径**与模型表情表（`listExpressions`）对齐，`target` 取**模型那一侧的 id**（模型的 `Name` 常与文件名不同）；导入时与 `refreshHotkeys` 加载时各解析一次（有改动才落盘）。
  - **⑤ 调试区把「被表情占用的参数」折叠**：`collectExpressionParamIds` 读热键里各 exp3 的 `Parameters`（有缓存），这批参数收进 `<details>`（默认收起，标题「已被表情占用（N）」），其余照常铺开 —— 用户口径：那一大批与热键的表情重合，不必特意罗列。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（zip 318.8 KB、dev 已同步，v0.9.31）。
- **⑰ 「模型全貌」导入：vtube.json 作者热键 + cdi3.json 可读名 + 换模型改选文件夹（2026-10-10 用户要求先调研再讨论）**：
  - **调研结论（公开资料，未读用户模型文件）**：`.vtube.json` = VTS 自动生成的模型配置，**官方无 schema**、纯人类可读 JSON、就放在模型旁边（VTS wiki「VTS Model File」）。它含**作者预设热键表**（`Hotkeys[]`：`Name` / `Action` / `File` / `Folder`）与**参数绑定**（`ParameterSettings[].OutputLive2D`）。VTS 公开 API 的 `HotkeyType` 共 22 种，**与模型本体相关的只有 5 种**（Trigger Animation / Change Idle Animation / Toggle Expression / Remove All Expressions / Art Mesh Color Preset），其余（换背景 / 截图 / 移动模型 / 物品 / Twitch …）是应用层的 ⇒ 我们只认前两种，`.exp3.json` 归 `ToggleExpression`（**这也解释了「为什么导入的全是表情」**）。
  - **用户提供样本 `E:\狐仙.vtube.json` 后确认字段**：`Hotkeys[]` 每条 = `{ HotkeyID, Name, Action: "ToggleExpression", File: "去水印.exp3.json", Folder }`（31 条全为 ToggleExpression，**无键位字段**——键位由我们自动分配）；`ParameterSettings[].OutputLive2D` 含 `Param139/Param157/Param161…` 这类**扩展参数**（运行时表里可能有）。
  - **落点**：① 宿主新增 `scan_model_folder(dir)`（`commands.rs`）—— 列 `*.model3.json` / `*.vtube.json` / `*.cdi3.json`（不递归，VTS 约定就在根上）。② `pet-hotkeys.ts` 新增 `readVtsHotkeys` / `readVtsParamIds` / `readCdi3Names` / `actionTypeFromVts` / `hotkeyFromVts`（全部**容错**：字段对不上当没有，VTS 换版本不崩）。③ `pet.ts` 的 **更换形象改选文件夹** → `scan_model_folder` → 取 `model3.json` 校验 → **一并读** vtube（作者热键）/ cdi3（可读名）/ 表情，落 `lunac.hotkeys.json`。④ 导入按钮改为「重新导入（模型文件夹）」：优先 vtube 的作者热键，没有 / 空表才回落递归扫 `*.exp3.json`。⑤ 调试区参数改**三方合并**（运行时 id 表 + `vtube` 的 `OutputLive2D` + cdi3 只给名字），再**移除已绑定到热键的参数**。
  - **验证**：`npx tsc --noEmit` exit 0；`cargo check` exit 0（仅 `proxy_server.rs` 的既有 warning）；`build-plugins.ps1 -Plugin pet` exit 0（zip 319.4 KB、dev 已同步，v0.9.32）。**⚠️ 改到了宿主 ⇒ 需重编 lunac.exe**（`scripts/dev.ps1` 会随 tauri dev 自动重编）。
  - **待用户确认**：「去除 png 图片的显示规格」按「更换形象只认文件夹 ⇒ 图片形象模式已无入口」处理；**未删**图片渲染代码（`PetConfig.image` 仍在），要不要彻底删由用户定。
- **⑱ 布尔/0-1 参数判定 + 预设动作页 + 调试区再精简（2026-10-10 用户两点）**：
  - **① 布尔参数调研与判定（用户问「有些参数应该是布尔型 / 只有 0 和 1」）**：**Cubism 参数本身不带类型**（`cdi3.json` / `.model3.json` 都不记类型），能用的判据只有两条 —— **区间恰为 `0..1`** 且 **动作曲线（`.motion3.json` 的 `Curves[].Segments`）实际只取过 `0` / `1`**。Live2D 官方参数表约定「`スイッチON/OFF は 0 ～ 1 基本0`」佐证。**落点**：`collectSwitchEvidence` 读该模型全部 motion 的取值集合 → `isSwitch(id)` = 区间 0..1 **且** 取值全落在 {0,1}（**读不到曲线时按区间判为开关**，与官方约定一致）。开关行渲染成 `pet-param-on` 状态键（关/开）而非拉条，拉条退为隐藏值容器；形态可**手动覆盖**（`PetConfig.paramKinds[model][id] = "switch" | "range"`，行尾 `pet-param-kind` 按钮切换，逐条校验后落盘）。
  - **② 调试区再精简（用户口径：vtube.json 很多是动作时的参数配置）**：`ParameterSettings[].OutputLive2D` 是 **VTS 追踪绑定的参数**（摄像头驱动 / 自定义追踪），与「模型调试」无关 ⇒ 与**已绑定热键的参数**（`collectExpressionParamIds`）一起，从调试区移除（`merged` 减去 `boundHotkey` + `boundVts`）。
  - **③ 预设动作页（用户：查看 VTS 标准动作文件 + 交由 AI 控制/自定义）**：**VTS 动作文件 = `.motion3.json`**（`Version=3` / `Meta{Duration,Fps,Loop,FadeInTime,FadeOutTime}` / `Curves[]{Target,Id,Segments}` / `UserData[]`），由 `.model3.json` 的 `FileReferences.Motions` 按**组**引用 —— 即「一个动作 = 一组里的一条曲线集合」，**没有单独的「预设动作」文件**。**落点**：设置窗加第三个页签「预设动作」（`motionBlock`）—— 列模型的 motion **组**（`listMotionGroups` 读 `FileReferences.Motions`）与**已存预设**（`MotionPreset{id,name,group,index}`，`loadMotionPresets`/`saveMotionPresets` 读写模型旁 **`lunac.motions.json`**），可改名 / 删 / 落盘 / **预览播放**（广播 `motion-play` → 桌宠窗 `internalModel.motion(group,index)`）。**只用模型声明的动作**（不臆造），预设只是「给某组某条起的名字」，为后续 **AI 触发/自定义**留稳定 id。
  - **落点补充**：宿主 `commands.rs` 新增 `read_pet_motions` / `write_pet_motions`（抽出 `pet_side_file_path(model_path, name)`）；`pet-hotkeys.ts` 新增 `MotionPreset` / `makeMotionPreset` / `loadMotionPresets` / `saveMotionPresets` / `listMotionGroups` / `collectMotionParamValues`（Segments 扁平数组解析：`type 0/2/3` 各带 `(x,y)`、`type 1` 贝塞尔带 6 个）。
  - **验证**：`npx tsc --noEmit` exit 0；`cargo check` exit 0（仅 `proxy_server.rs` 既有 warning）；`build-plugins.ps1 -Plugin pet` exit 0（zip 321.4 KB、dev 已同步，v0.9.34）。
  - **待办（遗留）**：把「AI 触发热键 / 触发预设动作」做成工具（宿主命令 + `core-agent` 工具，需重编 `agent.exe`）—— 用户明确要「交由 ai 控制」。
- **⑲ AI 驱动桌宠：`PetControl` 工具 + 默认人格重写（2026-10-10 用户要求，本轮闭环 ⑱ 的遗留）**：
  - **① 为什么不做成 MCP builtin**：`tools/call` 在 `lunac.exe --mcp-server` 的**另一个进程**里跑，拿不到主进程里那扇桌宠窗（无法 `emit`）。**改走 `TodoWrite` 那条已成型的老路**：工具在 `core-agent` 里声明、**效果全在前端** —— 前端（`main.ts`）读到该 `tool_use` 就**广播** `pet-control` 给桌宠窗，由**持有 Live2D 实例**的桌宠窗落到模型上（设置窗无实例，这条分工与热键「测试」一致）。
  - **② `core-agent`（`tools.rs`）**：新增内置工具 `PetControl`（`defs()` 里，现为**十九个**：本件为第 18 件，2026-10-11 再 + `NotebookEdit` 至 19 个）—— 入参 `{ action: "hotkey" | "motion" | "param" | "expression", name, value? }`。`run()` 只校验入参、回一句回执（**不碰本机、不落盘**）⇒ 与 `TodoWrite` 同型：**免审批**（`needs_approval` 不加）、**收进 `parallel_safe`**（agent 侧无副作用，前端按到达顺序执行）。`subagent_tool_defs()` 把它**剔掉**（子代理的 `tool_use` 不进前端助手流 ⇒ 在里面调是**保证的空操作**，与走桥那类同理）。
  - **③ 前端（`main.ts`）**：`handlePetControl()` 截获 `tool_use` → `emit("pet-control", {type:"ai-action", action, name, value})`（按 `tool_use_id` 去重；不合法入参静默）。**不建命令卡** —— 流式路径在 `content_block_start` 就跳过建块（卡上那行 `action=… name=…` 正是不该给用户看的技术细节），整包路径在 `tool_use` 分支截获后 `continue`；`tool_result` 也**一律丢弃**（否则 `agentToolResult` 会按「最近一张未完成的卡」兜底，把回执填到上一张卡上）。
  - **④ 名字词表注入（只在桌宠轮）**：`buildPetActionHint()` 从**本机现成的东西**拼清单 —— `read_pet_hotkeys`（键串 + 动作类型）、`read_pet_motions`（预设名）、`list_expression_files`（同层表情名）、localStorage 的运行时参数 id 表（截断 120）。拼进**桌宠轮**的用户消息前缀（`currentTurnFromPet` 分支，与 `pet-persona` 同一处）；**普通轮不注入**（不与当前任务无关的 token）。localStorage 键 `lunac.pet.v1` / `lunac.pet.coreids.v1.` 与 `pet.ts` **必须同值**（同源共享）。
  - **⑤ 桌宠窗（`pet.ts`）**：`offCtl` 加 `ai-action` 分支 → `runAiAction(action,name,value)` —— `hotkey` 先按键串、退而按热键名匹配后 `toggleHotkey`；`motion` 按预设名 `playMotion`；`param` 走 `setParameter`；`expression` 走 `setExpression` / `resetExpressions`。解析不到 / 没挂模型一律**静默**（名字来自 AI，可能过时；不去打扰用户）。
  - **⑥ 默认人格全面重写（`storage.rs` 的 `DEFAULT_PET_PERSONA`）**：保留原基调（温柔女孩子 / 口语化 / 简洁），补「可用表情与小动作表达情绪，一次一两个」，并**钉死两条硬边界** —— ① 输出里**不许出现它自己调过的动作/参数**（不提工具名、参数名、数值、文件名）；② **不影响 AI 本体的人格**（这段只落在 `config\pet-persona.md`、只拼进桌宠轮，与本体 `config\persona.md` 各走各的）。
  - **⑦ 可关闭**：`PetControl` 进 `TOOL_BLACKLIST_CANDIDATES`（设置里给用户一个「别让模型自己驱动桌宠」的开关）。
  - **验证**：`npx tsc --noEmit` exit 0；`cargo check`（src-tauri）exit 0；`cargo check`（core-agent）exit 0；`build-core.ps1` exit 0（**agent.exe 3.1 MB**，需重编 —— 内置工具变了）；`build-plugins.ps1 -Plugin pet` exit 0（zip 321.6 KB、dev 已同步，v0.9.35）。
  - **待验**：真机验收（桌宠轮里模型是否按注入的键串/名字触发、不暴露技术细节、不刷屏）；`main.ts`（主窗前端）改动需 `dev.ps1` 重跑才生效。
- **⑳ 预设动作改扫 `*.motion3.json` + 形态按钮图标 + 三页「恢复默认」+ 打开插件自动拉起模型（2026-10-10 用户六条）**：
  - **① 预设动作来源 = 扫文件夹，不看 `model3.json`（用户口径）**：宿主新增 `list_motion_files(dir)`（`commands.rs`，**递归**扫 `*.motion3.json`，深度 ≤ 5 / 条目 ≤ 3000，`name` 取文件名词干，**按路径排序**）；`pet-hotkeys.ts` 的 `listMotionGroups` **删除**，改为 `listMotionFiles(modelPath)`（+ `MotionFileRef`）。**播放要靠库的 `motion(组,序号)`**，所以桌宠窗挂载时把扫到的文件**注入成一个合成组**（`MOTION_EXTRA_GROUP = "Lunac"`，唯一定义在 `live2d.ts`）—— `MountOptions.extraMotions`（与 `extraExpressions` 同一条路子），**组内顺序 = 数组顺序 = `list_motion_files` 的路径序**，前端按同一份清单的下标编号即可对上。预设行因此由「组下拉 + 序号框」改成**文件下拉**（值 = 下标），`group` 恒记为合成组。
  - **② 形态按钮改图标（用户口径：不分状态，就一个上箭头 + 下箭头并存的图标）**：`pet-param-kind` 的文字「开关 / 连续」→ 内联 SVG（上三角 + 下三角）；形态判定改读 `data-kind`（原来读 `textContent`，改图标后会失效）。
  - **③ 参数持久化 + 对齐（用户口径）**：**已有**（`emitParamThrottled` 尾发时 `persistParam`；`restoreParams` 回填控件；桌宠窗 `paint()` 装载时应用 `savedParams`）—— 本轮把「打开插件自动拉起模型」补上，于是**重开插件 = 参数自动维持**（以前不拉模型，看起来像没记住）。
  - **④ 三个分页各一颗「恢复默认」（用户要求）**：调试页 = 清该模型的参数覆盖（落盘删键 + **一条**批量广播 `reset-params` 把面板里全部参数设回 0 + `restoreParams` 回填）；热键页 = 清空热键表（落盘为空，可再「重新导入」）；预设动作页 = 清空预设（落盘为空）。按钮带 `title` 说明各自会清什么。
  - **⑤ 每次打开插件自动拉起模型（用户要求）**：`attachSettings` 末尾 —— 配了 Live2D 模型且桌宠窗没开（`pingPet()` 无应答）就 `showPet()` 直接开起来。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（zip 322.3 KB、dev 已同步，v0.9.36）。⚠️ **宿主 `cargo check` 本轮未能跑**（用户在跑 `tauri dev`，其 cargo 进程一直持有 `target\debug` 的 `librsource.a` ⇒ `os error 32`），改用**独立 target 目录**复核；改动本身是 `list_exp3_files` 的同构拷贝 + 一处注册。**改到了宿主 ⇒ 需重编 lunac.exe**。
  - **待验**：真机上确认合成组能播（`motion("Lunac", i)`）、三个「恢复默认」的实际观感、自动拉起模型是否会打扰。
- **㉑ 桌宠窗「更换模型」失效修复 + 右键菜单加「恢复默认大小 / 位置」（2026-10-10 用户两条）**：
  - **① 修复根因**：换模型改成「选文件夹」后，**设置窗**的 `pickShape` 走的是新链路（`scan_model_folder` + 校验 + 一并导入作者热键），但**桌宠窗右键菜单的「更换形象…」没跟着改** —— 它还是旧的「选单个 `.json` / 图片」（`directory:false` + 扩展名过滤），于是选不到文件夹、也就读不到模型。**修法**：菜单那条改用**与设置窗同一条路径**（选文件夹 → `scan_model_folder` → `validateModelFile` → `importHotkeysFromFolder` → `paint()`）；图片入口一并撤掉（用户此前口径「去除 png 图片的显示规格」）。
  - **② 菜单加「恢复默认大小 / 位置」**：`data-act="reset"` —— `applyScale(1)` + `applyOffset(0,0)` + 落盘 + 广播 `scale-changed`（设置窗拉条对齐）+ 一句 toast。即「注视点（偏移）与模型大小」回默认。菜单多一条 ⇒ `openMenu` 的纵向夹取从 `innerHeight-190` 放到 `-240`。
  - **③ 文案对齐**：两处「更换形象…」改成「更换模型…」（设置窗按钮 + 桌宠菜单），控制台空态提示同步（不再提图片）。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（zip 322.4 KB、dev 已同步，v0.9.37）。**纯前端改动，不涉及宿主**。
- **㉒ 主动反应并入「自动抓屏 / 屏幕变化触发」+ 动作页改「行 = 文件」+ 可见性同步（2026-10-10 用户八条）**：
  - **① 两个抓屏子开关（用户口径：「自动抓屏」与「屏幕变化触发」归入主动反应）**：`ProactiveCfg` 加 `autoShot` / `autoShotMin`(1..240 分) / `screenWatch` / `screenWatchSec`(5..600 秒) / `screenDiff`(1..120)，**均默认关的显式 opt-in**（隐私红线：只由用户显式开启触发；桌宠不可见即停；计入**同一个**每日上限；抓屏前弹可见 toast）。「自动抓屏」按 `autoShotMin` 周期 `screenshot_monitor` 抓整屏 + 一句 prompt 交给 AI；「屏幕变化触发」按 `screenWatchSec` 周期 `screenshot_signature` 抓 16×9 指纹（**不落盘**）比上一次、`meanAbsDiff ≥ screenDiff` 就当画面变了 → 抓整屏主动说一句（触发后冷却 = 主链路 `intervalMin`）。**调度全在桌宠窗**（`scheduleAutoShot` / `fireAutoShot` / `scheduleScreenWatch` / `fireWatchTick` / `fireScreenChange`），与主链路**共享** `proactiveBusy` + 90s 看门狗；`scheduleMidnightRecheck` 跨日时一并重排三个调度器。设置窗只写配置 + 广播 `set-proactive`（现带五个新字段）。
  - **② 开关文案改「关 / 开」+ 删掉「主动反应注意」整段（用户口径）**：主动反应按钮原来写「让桌宠主动跟我说话：关」⇒ 名字移到左侧 `pet-label`，按钮只留 `onOffText()` 的「关 / 开」；`pet.proactive_note` 那条整段说明**删除**，只保留一句隐私提示（`pet.proactive_shot_hint`）。新增 i18n：`pet.proactive_autoshot` / `_screenwatch` / `_second` / `_sensitivity` / `_shot_hint` / `pet.autoshot_prompt` / `pet.watch_prompt`（5 语言）。
  - **③ 恢复默认 / 导入移到页顶（用户口径）**：三页的 `.pet-reset-row` 与热键页的「重新导入」都在页首（此前在页尾）。
  - **④ 动作页改「行 = 文件」+「新建动作」生成全参数 motion3 模板（用户口径）**：删掉 `MotionPreset` / `lunac.motions.json` 整套（含宿主 `read_pet_motions` / `write_pet_motions`）—— 动作页每行 = 扫到的一个 `*.motion3.json`（`listMotionFiles`），可改名（`rename_motion_file`）/ 删除（`delete_motion_file`）/ 播放（`motion-play` → 合成组下标）。「新建动作」= 宿主 `write_motion_file`：按当前模型**全部参数**（运行时 `parameterRanges`）生成标准 `*.motion3.json`（`Version=3` / 1s / 30fps / `Loop` / 每参常数曲线），文件名前缀 `lunac_`，交给 AI 去编曲线。
  - **⑤ 修「更换模型时动作 motion3 未被读取」**：桌宠窗右键菜单的「更换模型」仍是旧的「选单个 `.json` / 图片」picker（与设置窗的「模型 = 文件夹」链路脱节）⇒ 菜单改用 `scan_model_folder` + `validateModelFile` + `importHotkeysFromFolder`；挂载动作统一走 `listMotionFiles(modelPath)`。
  - **⑥ 修「关闭模型后设置窗卡在收起来」（可见性同步）**：根因 = 桌宠窗被**销毁**（宿主用 `destroy()`）时**不会**补发 `plugin-window-visibility` ⇒ 设置窗按钮永远停在「收起来」。修法两处 —— 桌宠窗的 `pong` **补带 `visible`**（隐藏 ≠ 关闭：隐藏的窗照样应答），设置窗开着时**每约 3s 重 ping**（`togglePoll` → `refreshToggle`），桌宠窗关掉后无 `pong` ⇒ 自动回落成「显示桌宠」。
  - **⑦ 开头四颗按钮同一行 + 显示/隐藏保持上次状态（用户口径）**：`.pet-row-main`（`flex-wrap:nowrap` + 收窄内距）把「显示桌宠 / 更换模型 / 取消配置 / 鼠标穿透」压在一行；参数 / 表情 / 大小 / 偏移一律按模型路径落 `localStorage`，桌宠窗每次挂载 `paint()` 应用 `savedParams` ⇒ 显示 / 隐藏 / 重开都维持上次状态。
  - **验证**：`npx tsc --noEmit` exit 0；宿主 `cargo check --target-dir target-check` exit 0（`screenshot_signature` + 三个 motion 命令注册）；`build-plugins.ps1 -Plugin pet` exit 0（v0.9.38）。
  - **待验**：真机确认两个抓屏子开关的节奏 / 隐私 toast / 每日上限口径，以及动作页「新建动作」模板能否被 AI 直接编曲线。
- **㉓ 设置窗「隐藏模型」（不再关插件）+ 每个动作一个「待机循环」开关（2026-10-10 用户两条）**：
  - **① 「收起来」不再关闭整个插件（用户报障：「点击收起来这个按钮会自动关闭整个插件，取消这个功能」）**：原来 `#pet-toggle` 在可见态调 `close_plugin_window`（宿主走 `destroy()`，整扇桌宠窗被销毁）。改为**广播** `pet-control` 的 `{type:"hide"}` → 桌宠窗自己 `getCurrentWindow().hide()`（`core:window:allow-hide` 早已在 capabilities 里）；按钮文案「收起来」→「**隐藏模型**」。**为什么桌宠窗要自己再走一遍收尾**：它自己 `hide()` 不产生 `plugin-window-visibility`（宿主只在 `Resized` 翻转时下发）⇒ 收到 `hide` 后主动调 `applyHidden()`（停 ticker / 关菜单 / 暂停主动反应与两个抓屏子开关）。重新显示走 `open_plugin_window` 的复用路径（宿主 `unminimize()` + `show()` + `announce_visibility_now()`）⇒ 桌宠窗收 `visible=true` 调 `applyShown()`。原来散在 `offVis` 里的两段收尾/恢复抽成 `applyHidden` / `applyShown` 一对，隐藏、最小化、恢复三处共用。
  - **② 「有些动作需要待机时循环播放」（用户原话）**：动作页每行加一枚「**待机循环：开 / 关**」状态按钮（`data-mo-idle`，可多选），状态存 `PetConfig.idleMotions`（模型路径 → 文件名词干数组，与 `expressions` / `params` 同一种按模型分键的 localStorage 存法）。实现：`live2d.ts` 加 `MOTION_IDLE_GROUP = "LunacIdle"` + `MountOptions.idleMotions`；`live2d-engine.ts` 把勾中的文件注入该组，并在 `Live2DModel.from()` 之后把库的 idle 组指过去（`model.internalModel.motionManager.groups.idle = MOTION_IDLE_GROUP` —— 类型里 `readonly`、运行时普通字段）。库在空闲 / 动作播完时 `startRandomMotion(groups.idle, IDLE)` ⇒ **随机轮流**播；循环与否由各文件自己的 `Meta.Loop` 决定（`buildMotionTemplate` 生成的模板恒为 `Loop: true`）。改开关后广播 `reload` 让桌宠窗**重挂**（idle 组只在挂载时注入）；删 / 改名 / 重扫后 `pruneIdleMotions()` 摘掉失效项。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（v0.9.39，zip 325.5 KB、dev 已同步）。**纯前端 + 插件引擎改动，不涉及宿主**（`hide` 走已有权限与已有命令）。
  - **待验**：真机确认隐藏 / 重新显示的动画恢复、以及多个「待机循环」动作的轮流观感（库是按 idle 组随机取，不是严格顺序轮播）。
- **㉔ 运行时状态「跨隐藏 / 显示 / 重开插件」保持（2026-10-10 用户报障 + 口径）**：用户报「重新显示模型时热键的状态被重置了」，并给出总口径 —— **热键 / 参数 / 动作三类状态不许被「隐藏 / 显示模型」或「关闭 / 重开插件」影响，只有换模型才重置**。
  - **① 根因之一 = 「显示」把桌宠窗整页重挂了**：设置窗的「显示桌宠」原来无条件 `invoke("open_plugin_window", {pluginId:"pet"})`；宿主的**复用路径**（`plugin_window.rs`）会推一条 `plugin-window-input`，而 `plugin-window.ts` 的 `getCurrentWindow().listen("plugin-window-input", …)` 收到就 `render()` **整页重挂**（重载模型 ⇒ 引擎里的运行时状态全没）。**修法**：设置窗先 ping —— `st.alive && !st.visible`（窗还在、只是被隐藏）时**不走** `open_plugin_window`，改广播 `pet-control` 的 `{type:"show"}`，由桌宠窗自己 `unminimize()` + `show()` + `applyShown()`；只有窗不在时才建窗（新建路径不推 `plugin-window-input`，无重挂）。
  - **② 热键开关状态持久化**：新增 `PetConfig.hotkeyStates`（模型路径 → 热键 id 数组）。`toggleHotkey` 改完调 `saveHotkeyStates()` 落盘（**`motion` 类不记** —— 它是「播一次」不是状态，记了会每次挂载重播）；挂载完成（`l2dHandle = h` + 应用 `savedParams` 之后）调 `restoreHotkeyStates(modelPath)` 逐个重新施加，并把 `hotkey-states` 广播给设置窗对齐按钮。**「换模型才重置」天然成立**：换模型会重新导入热键、id 全变 ⇒ 存档旧 id 一条都对不上。参数（`params`）与表情（`expressions`）本就是「按模型存 + 挂载时应用」，动作侧是 `idleMotions` —— 三类状态自此同一口径。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（v0.9.40，zip 325.9 KB、dev 已同步）。**纯前端改动，不涉及宿主**。
  - **待验**：真机确认「隐藏 → 显示」后热键/参数/动作都在、关闭插件再开也都在、换模型后确实重置。
- **㉕ 「待机循环开着时一播动作就永久卡死」修复 + 口型同步（2026-10-10 用户报障 + 按外部差距分析继续做）**：
  - **① 卡死根因（查证到源码级）**：**pixi 的 ticker 在 `update()` 抛异常时不再申请下一帧** —— `@pixi/ticker/lib/Ticker.js` 的 `_tick` 是 `this._requestId = null; this.started && (this.update(time), …this._requestId === null && requestAnimationFrame(this._tick))`，异常一抛，那句 `requestAnimationFrame` 永远执行不到 ⇒ **整条渲染链永久停住**（正是用户说的「模型完全卡死」）。触发面：库 `MotionManager.update` 在「idle 组自动预约」与用户显式 `startMotion` 之间的优先级/预约状态互相顶掉，`state.start` 返回 false 而 `reservedIdleGroup`/`reservedGroup` 已被占住。**修法两道**：① **渲染兜底** —— 引擎把 `model.internalModel.update` 包一层 `try/catch`（出错帧吞掉 + 最多 5 条 `log_frontend`），**任何同类异常都不会再冻住界面**；② **不再借库的 idle 组** —— 待机循环改由引擎自己在 `app.ticker` 上驱动（顺带从「随机取一条」变成**按顺序轮播**，并用各文件 `Meta.Duration` 做超时兜底），从源头绕开那条夹击路径。`MOTION_IDLE_GROUP` 这个合成组随之删除。
  - **② 口型同步（差距分析结论的第 1 优先项）**：原来只调 `tts_speak` 播音频、**嘴完全不动**。现在宿主在**开始播之前**把这条 WAV 的「张嘴幅度包络」定向喂给桌宠窗：`tts.rs` 新增极简 RIFF 解析（PCM16 / float32）+ `mouth_envelope()`（40 fps 逐格峰值 → 按整段峰值归一化成 0..255，静音段 < 6% 压 0、其余取 √），`emit_to("plugin-pet", "pet-tts-mouth", {fps, env})`；WinRT 与 ZipVoice 两条通路共用 `play_wav_bytes`，所以 `speak_blocking` / `tts_speak` / `tts_zipvoice::speak` 一并带上 `AppHandle`。前端 `driveMouth()` 按 fps 逐格 `setMouthOpen(env[i]/255)`，播完 `null` 放手。引擎新增 `Live2DHandle.setMouthOpen(v|null)`（写在 `beforeModelUpdate`、表情层同级），参数优先模型声明的 LipSync 参数、退回 `ParamMouthOpenY`，按 `parameterRanges` 满量程换算。
  - **验证**：`npx tsc --noEmit` exit 0；宿主 `cargo check --target-dir target-check` exit 0（仅 `proxy_server.rs` 既有 warning）；单测 **206 passed / 0 failed / 2 ignored**（新增 `tts::tests::mouth_envelope_tracks_amplitude` 与 `mouth_envelope_bails_out_on_garbage` 两条）；`build-plugins.ps1 -Plugin pet` exit 0（v0.9.41，zip 326.7 KB、dev 已同步）。⚠️ **改到宿主（`tts.rs` / `tts_zipvoice.rs`）⇒ 需重编 lunac.exe**。
  - **待验**：真机确认①循环开着播动作不再卡死；②朗读时嘴跟着动、说完复位；③认不出的 WAV 只静默不驱动。
  - **下一步（差距分析剩余项，按性价比）**：~~无 tool-calling 的「情绪标签 → 表情动作」回退通道~~（**已做，见 ㉗**）→ ~~语音输入（ASR / 免提 / 打断）~~（**用户 2026-10-10 裁定暂不做**；[agent-ui-spec.md](./agent-ui-spec.md) §2 的「不做」结论维持）→ 控制面补齐（**ArtMesh 颜色 tint ✔ + 模型旋转 ✔，见 ㉘；道具 / 背景 仍未做**）→ 长期记忆。
- **㉖ 修「清除桌宠侧历史后，桌宠发的消息永远得不到输出」（2026-10-10 用户报障）**：
  - **根因（轮次错位）**：`showChatHistory()`（AI 插件的历史视图；**打开一次、以及每删一条会话都会重绘一次**）里那段「Cancel any active streaming」只做了 `streamId++ / isStreaming=false / setStreamingUI(false)` —— **既没清 `cliTextCallback`/`cliDoneCallback`，也没停 `agent.exe`**。于是 agent 仍在生成那条被丢弃的 `result`，而用户从桌宠发的下一轮 `startAgentChat` 已把**新的** `cliDoneCallback` 装上 ⇒ 旧 `result` 命中的是**新回调**（`streamId` 恰好对得上）、消费掉后自行置空 ⇒ 紧接着到达的**真正的回复没有回调接住**，桌宠永远停在「思考中」。删一条会话 = 一次重绘 = 一次错位机会，所以「清除桌宠侧历史」是必现场景。
  - **修法**：新增 `cancelInflightTurn(reason)`（`app/src/main.ts`），把「掐掉在飞的那一轮」收成**一件事、三件套**：① 前端清干净（`streamId++` + 两个回调置空 + `isStreaming=false` + `setStreamingUI(false)`）；② **真正停 agent**（`flushTurnUsage()` + `markFlyingCardsStopped(agentView)` + `invoke("stop_cli")` + `cliReady=false` + `agentThread=null`；下一轮由 `startAgentChat` 的 `!cliReady` 分支自动重启）；③ **桌宠轮回一句收尾**（`replyPetTurn`；`wasPet` 必须在它之前取）。`showChatHistory()` 与 `restoreSession()` 改用它（后者**必须排在 `switchThread` 之前** —— 那样半截正文才落进它自己的线程）。`newConversation` / `stopAIChat` / `rollbackChat` 保持原样（各有自己的完整流程）。
  - **验证**：`npx tsc --noEmit` exit 0。**纯前端改动（应用主界面 / 聊天窗那一份 `main.ts`），不涉及插件包与宿主 Rust** —— dev 下重载窗口即可，release 需重编前端。
  - **待验**：真机按原步骤复现：清除桌宠侧历史 → 从桌宠对话框发一句 → 应正常收到回复（且中途那一轮被就地收尾、桌宠不会停在「思考中」）。
- **㉗ 无 tool-calling 的「情绪标签 → 表情/动作」回退通道（2026-10-10，差距分析结论第 1 项，用户「继续做下去」）**：
  - **要解决的问题**：⑲ 的 `PetControl` 只在模型**会调工具**时才有用。本地 / 弱模型不产出 `tool_use` ⇒ 桌宠在这些模型下「一动不动」，AI 托管形同虚设。回退 = 让模型**直接在正文里写 `[名字]`**（名字取自同一份动作词表），前端抽出来落成动作、并把标签从正文去掉。
  - **① 词表同源（硬纪律）**：`main.ts` 把 `buildPetActionHint()` 拆成 **`collectPetActionVocab()`（一处读盘：`read_pet_hotkeys` 键串 / `list_motion_files` 动作名 / `list_expression_files` 表情名 / localStorage 参数 id）+ `buildPetActionHint()`（拼文本 + 返回名字集合 `PetTurnActions.names`）**。两条通道必须同源，否则「注入了却认不出」。
  - **② 注入**：同一段 hint 里加一句 failover 说明（没有工具通道时可用方括号写词表里的名字，会被应用且对用户隐藏）。
  - **③ 抽取 / 去标签（`consumePetTags()`）**：`cliDoneCallback`（正常收尾）与 `finalizeInterruptedTurn`（被中断）**两处共用**；正则 `/\[([^\[\]\n]{1,40})\]/g`，**只有名字真在那份集合里才认** —— 认不出的方括号原样保留（markdown 链接 `[文字](url)`、`[1]` 引用不会被误删）。命中的名字按到达顺序广播 `pet-control` 的 `{type:"ai-action", action:"auto", name}`；正文去标签后才进历史 / 复制 / 气泡 / 朗读。
  - **④ 类型由桌宠窗解析**：标签只带名字、不带 `action` ⇒ 新增 `action:"auto"` —— `runAiAction` 按「热键键串 → 动作文件名 → 表情名」**依次尝试、命中即止**（都不中 = 静默空操作）。
  - **⑤ 不重复施加**：本轮若已走过 `PetControl`（新标志 `petControlUsedThisTurn`，`handlePetControl` 置位）就**不再吃标签** —— 表情 / 热键是**开关**语义，连按两次等于取消。标志与名字集合都在 `startAgentChat` 开头按「是否桌宠轮」重设。
  - **⑥ 用户开关复用工具黑名单**：`PetControl` 被拉进 `--disallowedTools`（`loadCustomBlacklist()`）时**两条通道一起关**（hint 不注入 + 名字集合留空）。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（v0.9.42，zip 326.8 KB、dev 已同步）。**纯前端改动（`main.ts` + 桌宠插件 `pet.ts`），不涉及宿主 Rust** ⇒ 不必重编 `lunac.exe`；release 需重编前端。
  - **待验**：真机用**不调工具的模型**跑一桌宠轮 —— 回复里若带 `[名字]`，模型应动一下、气泡与朗读里**看不到方括号**；普通聊天轮里出现 `[xxx]` 不受影响。
- **㉘ 控制面补齐第一批：ArtMesh 颜色 tint + 模型旋转（2026-10-10，按外部差距分析继续做）**：差距分析把「控制面」列为四项（ArtMesh 颜色 / 模型旋转 / 道具 / 背景），本轮做前两项。
  - **① ArtMesh 乘法色（tint）**：**写 Cubism Core 的 `drawables.multiplyColors`** —— 框架的 `getDrawableMultiplyColor(index)` 按 `index*4` 读它，且**每帧渲染时都调**（`pixi-live2d-display-lipsyncpatch/dist/cubism4.es.js` 的 `drawMesh` 路径，已读源码核实；`multiplyColors` / `screenColors` 在 Cubism Core 5.1.0 里确实存在，已在本机 `live2dcubismcore.min.js` 里核实）。做法照 `opacities`：引擎持 `heldDrawableColors: Map<id,{r,g,b}>`，在 `beforeModelUpdate` 每帧写回（RGBA 四个 float，**A 恒写 1** —— tint 不改透明度，那归 `setDrawableOpacity`）；默认 `(1,1,1,1)`。三个入口：**热键**新动作类型 `artmeshColor`（`{id, color:"#rrggbb"}`，行内 `<input type="color">`，开 = 染、关 = 还原；新增 `normalizeHex` / `hexToRgb` 两个工具函数）；**AI** `PetControl` 的 `action:"artmeshColor"` + `value` = **`0xRRGGBB`**（`value` 只有 number 一种形态 ⇒ 颜色打包成整数；`main.ts` 的 `collectPetActionVocab` 顺带读 localStorage 里的 `drawables` 注入词表）；**引擎 API** `Live2DHandle.setDrawableColor(id, {r,g,b}|null)`。
  - **② 模型旋转**：`MountOptions.rotation` + `Live2DHandle.setRotation(deg)`（`fit()` 里 `model.rotation = deg·π/180`，绕 `anchor` = 底边中心；与 `setScale` / `setOffset` 各改各的）。配置 `PetConfig.rotation`（−180..180，与 `scale` / `offsetX/Y` 同级同口径），设置窗新增「旋转角度」拉条 ↔ 桌宠窗 `set-rotation` 广播；右键菜单那条改为「**恢复默认大小 / 位置 / 角度**」，三者一起归零并广播 `rotation-changed` 回填拉条。**图片模式没有这一维**（`applyRotation` 对图片静默）。
  - **验证**：`npx tsc --noEmit` exit 0；`core-agent` `cargo check` exit 0（仅既有 2 条 warning）；`build-core.ps1` exit 0（**agent.exe 3.1 MB ⇒ 需重编** —— 内置工具入参变了）；`build-plugins.ps1 -Plugin pet` exit 0（v0.9.43、zip 327.7 KB、dev 已同步且已核对产物含 `artmeshColor` / `multiplyColors`）。**不涉及宿主 `src-tauri` Rust** ⇒ 不必重编 `lunac.exe`；release 需重编前端。
  - **待验**：真机确认①热键染色开/关（`<input type=color>` 选色 → 模型对应 ArtMesh 变色）；②设置窗旋转拉条与右键「恢复默认大小 / 位置 / 角度」双向一致、重开插件后仍保持；③AI 用 `PetControl` 传 `0xRRGGBB` 能染色。
  - **遗留（未做）**：**道具 / 背景** —— VTS 侧属应用层，要做需要新的资产管理 + 图层 UI（背景图 / 前景道具），本轮未动；VTS `ArtMeshColorPreset` 的**导入映射**也未做（用户给的样本里 31 条全是 `ToggleExpression`，无可测颜色样本）。
- **㉙ 右键菜单「恢复」文案 + 抓屏子开关不再挂在总开关下 + 引擎 `destroy()` 幂等（2026-10-10 用户三条）**：
  - **① 右键菜单那枚按钮文案**：由「恢复默认大小 / 位置 / 角度」**改成只有「恢复」**（用户口径：严格的「恢复」两个字，不要后缀）。行为不变（`scale=1` + `offset=(0,0)` + `rotation=0`，并广播 `scale-changed` / `rotation-changed` 回填设置窗）。
  - **②「自动抓屏 / 屏幕变化触发 并没有发送给 AI」的根因**：这四个函数（`scheduleAutoShot` / `fireAutoShot` / `scheduleScreenWatch` / `fireWatchTick`）的第一道闸都是 `if (!pr.enabled || ...) return;` —— 即两个抓屏子开关**被挂在「主动反应」总开关下面**。只开子开关、没开总开关时，两条链路**什么都不做、也不给任何提示**（UI 也没标这层依赖）。**修法**：删掉 `pr.enabled` 这道闸 —— 两个子开关各自就是一次**显式 opt-in**（默认关、有间隔 / 秒数 + 每日上限 + 桌宠不可见即停 + 抓屏前可见 toast），不该再被总开关挟持；`proactiveBusy` / 每日上限 / 看门狗仍与主链路共享。**另**：触顶时原来是裸 `return`（一旦触顶就再也不排）⇒ 改为用**自己的**定时器等到下一个自然日零点重排（新增 `msUntilNextMidnight()`，`scheduleMidnightRecheck` 一并改用它），不去抢主链路的 `proactiveTimer`。
  - **③ 静默失败改成可查**：`sendScreenToAI` 里 `vision` 关着时原来**直接 `return false`**（既无日志也无提示）⇒ 现在落一条 `log_frontend` 的 warn（整窗只记一条，不刷屏），并写明「要真的发出去需开启图片输入 / 换视觉模型」。
  - **④ 引擎 `destroy()` 幂等 —— 「查看某次对话后模型卡死」的调查结论**：从 `lunac-<date>.log` 拿到证据 —— 桌宠窗反复抛两条 `unhandledrejection`：`Cannot read properties of null (reading 'next')` 与 `this.cancelResize is not a function`（时间点与「看对话 → 聊天窗前后置」高度相关：`12:27:34` 看对话 → `12:30:50` 抛错）。**两条都是 pixi `Application.destroy()` 被调了两次**的特征：`Ticker.destroy()` 二次调用时 `this._head` 已是 `null`（读 `.next` 炸）、`ResizePlugin.destroy()` 二次调用时 `this.cancelResize` 已是 `null`。**修法**：`mountLive2D` 的句柄加 `destroyed` 标志，`destroy()` 幂等（第二次直接 return）—— 重画竞态（`paint()` 的「这次作废」分支与 `dropModel()` 撞在一起）下同一实例被销毁两次不再抛。**诚实边界**：这条能确证的是「重复销毁」这个缺陷本身；「卡死」的可感症状若仍复现，下一步该查桌宠窗是否只是被 WebView2 的**遮挡挂起 rAF**（前台被聊天窗盖住时 rAF 会停，露出后应自恢复）。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（v0.9.44、zip 327.9 KB、dev 已同步）。**纯前端 + 插件引擎改动，不涉及宿主 Rust** ⇒ 不必重编 `lunac.exe`；release 需重编前端。
  - **待验**：① 只开「自动抓屏」/「屏幕变化触发」（**不开**总开关）也应到点抓屏并交给 AI；② 抓屏前能看到 toast；③ 日志里不再出现那两条 `unhandledrejection`；④「恢复」按钮文案与行为。
- **㉚ 修「换出桌宠时两侧对话冲突，桌宠侧永远『思考中』+ 插件侧永远『agent 正在加载』」（2026-10-10 用户报障）**：
  - **根因（单槽 + 丢身份）**：`startAgentChat` 在 `!cliReady` 时走「挂起 + 早退」：把 query 塞进**唯一的** `window.__agent_pending_query` 字符串槽，等 `cli-status:stdout`（`cliReady=true`）再放行。而「agent 正在启动」是一段**真实窗口期**，桌宠轮与聊天轮会先后撞进来：后来者**覆盖**先来者 ⇒ 先到的那一问永远发不出去。更糟的是那个槽**只存 query** —— 「这一轮是谁发起的」没存，放行时只能读 `petTurnActive` 的**当前值**：若这期间已有别的轮次认领过它，放行出来的那一轮就被**误判来源**（`currentTurnFromPet=true`）⇒ 流被写进桌宠隐藏容器（聊天侧什么都看不到、那句「agent 正在加载」永远留着），而真正该收尾的那一轮没有 `replyPetTurn`（桌宠侧永远停在「思考中」）。图片附件同样在那个槽里丢掉。
  - **修法**：把单槽换成**带身份的 FIFO**（`pendingTurns: { query, imagePaths, fromPet }[]`）：① `!cliReady` 分支改为 `push`（**不再覆盖**）并把 `petTurnActive` **移交队列**（避免后续轮次误认领）；「agent 正在加载」那条提示按 `fromPet` 明确落进 `hiddenHost` / `chatFlowHost`（不看 `petTurnActive` 的时序）。② 新增 `drainPendingTurns()` —— **一次只起一轮**（`startAgentChat` 不防重入、`isStreaming` 又是全局的），起前把 `petTurnActive` 还原成该轮的 `fromPet`；被 `isStreaming` / 冲刷中挡住就直接返回，等下一次触发。③ 触发点三处：`cli-status:stdout`（主）、**本轮收尾**（`cliDoneCallback`，补上一次被挡下的）、**取消在飞轮**（`cancelInflightTurn` —— 这条路径不走 `cliDoneCallback`，不补就干等下一次 stdout）。④ 新增 `dropPendingTurns(reason)`：agent 起不来时**整队列丢弃并给桌宠发起的各回一句**（`pet.chat_reply_failed`），桌宠不再永远「思考中」。
  - **验证**：`npx tsc --noEmit` exit 0；`window.__agent_pending_query` 已无任何代码引用（只剩注释里的历史说明）。**纯前端改动（`app/src/main.ts`）** ⇒ 不涉及插件包与宿主 Rust；dev 重载窗口即可，release 需重编前端。
  - **待验**：真机复现原步骤 —— agent 正在启动（刚 `stop_cli` / 首次懒启动）时**先后**从桌宠与聊天侧各发一句：两侧都应拿到各自那一问的回复，`agent 正在加载` 会随该轮真正开跑而消失，桌宠不再停在「思考中」。**已知边界（未处理）**：挂起队列活在**聊天窗**的 JS 上下文里，若在等待期间把聊天窗关掉，队列随窗销毁（桌宠那一侧会挂住）—— 与「桌宠轮的审批卡在隐藏窗里点不到」同属既有交互边界。
- **㉛ 修「动作创作时一压缩上下文就回到上一次人格修改」（2026-10-10 用户报障；根因在 core-agent 压缩档）**：
  - **现场（从会话库 + agent 日志取到的硬证据）**：`ModuleData\history\chat.db` 的会话 `mv2dywf6h893`（**聊天线程**、8 条、标题「为桌宠的当前模型设置一个合适的人格」）里，同一句「根据当前的模型参数绑定 制作一个5种不同的待机动作 并制作成一个动作链 循环播放」被问了 **3 次**（第 2/3/7 条，其中两条**没有助手回复** = 那几轮被掐掉），而助手在第 6 条**又把人格重写了一遍**（御姐 → 小狐仙）。agent 日志（`temp\logs\agent-2026-10-10.log`）当天 **7 次压缩里有 5 次是 `摘要压缩失败：模型返回空摘要`**，其中 `12:44:09` 那次正好夹在这段会话中间（丢弃 32 条旧消息）。
  - **根因（丢一半的动作）**：`compact_history` 是**先 drain 掉旧消息、再由 `pin_summary_of_dropped` 让模型写一段摘要钉回去**。模型摘要失败（或输入过短被跳过、或该档被关）时那个函数直接 `return false` —— **被丢的几十条永久没有顶替物**。此时模型手里只剩 ① 被刻意钉住的「**会话第一条用户提问**」（`compact_history` 里 `head`/`cut` 那条规则：「始终保留开头那条用户提问 —— 它是任务目标」）＋ ② 尾部最近几轮。而这段会话的第一条恰好是「**设置人格**」⇒ 模型把**最早的那个诉求**当成当前任务，于是回头重谈/重写人格。用户的原话「只要压缩上下文直接回到了上一次人格修改」与这条链完全对上。
  - **修法（core-agent `main.rs`）**：新增 `fallback_digest_of_dropped()` —— **不调模型、不会失败**的确定性兜底：把被丢区间里的**用户文本消息**按原序摘成清单（每条截 140 字、最多 40 条，超出标 `…(+N … omitted)`），末尾补一行工具活动计数，并在表头**显式声明**「本会话**最新**的那条用户消息才是当前任务，最早那条只是背景」。`pin_summary_of_dropped` 改为：模型摘要可用就用它，**不可用一律退到这份兜底**（连「摘要档被关」也退 —— 丢弃已经发生，空手而归本身就是 bug）。兜底表头以 `[` 开头，与 `TRIMMED_MARKER` / 摘要头 / 任务快照同族，会被下一次压缩过滤掉、不会自我堆叠。
  - **验证**：core-agent `cargo test` → **162 passed / 0 failed / 3 ignored**（两条既有断言仍成立：`dropped` 为空必返回 false、低于 `SUMMARY_MIN_INPUT_CHARS` 不调模型）；`build-core.ps1` exit 0（**agent.exe 3.2 MB ⇒ 需重编**）。**只动 core-agent，不涉及前端与插件包**（`pet` 版本不变，仍 0.9.44）。
  - **待验**：真机跑一段**多诉求**长会话（第一条与当前任务不同，例如先让它「设置人格」再让它「做动作」），刻意撑到压缩：即使摘要失败，助手也应继续推进**最新**那个诉求、不再回头重做人格；`agent-<date>.log` 里应能看到新的 warn「摘要压缩不可用…退到确定性兜底清单」。
  - **教训（如实记）**：我第一次只用一条 grep 扫整个 `logs` 目录就断言「今天没有任何压缩记录」，实际那条记录**就在 `agent-2026-10-10.log` 里**（用户当场指出）。**诊断「有没有发生」这类事实时，必须直接读目标文件/逐文件核对，不能凭一次跨目录 grep 的零命中下结论。**
- **㉜ 修「打开价格表后整页点不动、鼠标穿过去」（2026-10-10 用户报障）**：`#token-usage-panel`（表盘的展开面板 = 用量与成本数据 + 价格表数据）漏了 `pointer-events: auto`。`#app` 是 `pointer-events: none`（透明窗：空白处要把点击放行给桌面），因此**每个 `#app` 的直接子元素都得自己打开它** —— `#results-container` / `#status-bar` / `#context-menu` 都声明了，本面板当初没写；`pointer-events` 是可继承属性，于是它跟着 `#app` 变成 `none` ⇒ 面板画得出来、整块鼠标穿透。修法：给 `#token-usage-panel` 补 `pointer-events: auto`（`styles.css`）。**纯 CSS**，dev 重载窗口即可；release 需重编前端。
- **㉝ 新增需求（2026-10-10 用户提出 + 已裁定）**：②③④⑤ 均已落地（见 ㉞/㊵/㊲/㉟）；**① 暂缓**（见下）。
  - **① Lunac 插件索引工具（用户口径：索引 + 定位 + 重建）** —— ⏸️ **暂缓（2026-10-10 用户裁定「暂时取消这个工具的实现」）**，设计与已查清的结论一并留在下面，将来重启时不必重查。
    - **原设计**：内置工具给出 `list`（已装插件 → `id`/版本/**源码入口绝对路径**/产物路径）/ `features`（功能地图：UI 区块 / 事件契约 / 宿主命令 → 源码位置「文件 + 符号 + 行号」）/ `rebuild`；**改文件交给现成 Read/Edit**（不重造编辑能力）。⚠️ **用户特别叮嘱**：该工具面向 **release 版 / 会换环境**，**不具备 dev 的全部环境与文件**（没有 `scripts\build-plugins.ps1`、没有 node/vite、可能没有 `.ts` 源码）⇒ **rebuild 必须优雅降级**：检测不到 dev 工具链就**如实报告「此环境无法重建」**并说明缺什么，**绝不假装重建**。
    - **设计结论（2026-10-10 查清，用户追问「仅 release 用是否局限性很强」后得出）**：**不该按「release 只有产物」这个最弱环境设计** —— 那样会把 **dev（用户实际最常用它改自己插件的地方）** 削成废的。正确口径是**环境自适应 / 能力分级**：`list` 两边都能用（dev 给源码入口绝对路径 + 产物路径；release 给产物路径并**明说没有源码**）；`features` 两边都要能给有用的东西（release 靠随包 features 或搜索产物 `index.js` 的锚点）；**只有 `rebuild` 是 dev-only**（release 如实拒绝）。dev 侧 I/O 是真实源码、改完能立刻重建验证，价值最大。
    - **已查清的技术前提**：宿主无条件注入 **`LUNAC_MODULES_DIR`**（core-agent 系统提示词已用它，见 `core-agent/src/main.rs` 约 1596-1614 的 `modules_line`），且 `<exe 根>\Modules` **本就在 agent 的可访问范围内**（同文件约 2967-2968）⇒ 工具不必新开环境变量；app 根从 `log::log_dir()` 的父目录推（core-agent 既有惯例）。**未决岔路**：`features` 靠**预生成地图 JSON**（准但需人工维护 + 行号会过期）还是靠**当场搜索**（零维护、两环境都用，但依赖关键词准）——用户未选（此问随「暂缓」一并搁置）。
  - **② 动作页新增「动作链」**：把多个动作排在**一条时间线**上，并能给**单个动作设循环播放时长**与**停止时间**。 ✅ **已做，见 ㊲**。
  - **③ 模型文件夹里新增 lunac 缓存文件**（调试参数 / 热键参数 / 动作参数各一份）：一举解决两个问题 —— ①进入插件**读取之前**就能拿到上次的数据（现在是「先显示过一次桌宠」才有 localStorage 运行时表）；②**AI 可直接改这个缓存文件**，再**反向同步进设置面板**。 ✅ **「调试参数」那份已做，见 ㊵**（`lunac.params.json`；热键 / 动作链那两份早前已各自落地）。
  - **④ 「待机循环」改名 + 修卡死** ✅ **已做，见 ㉞**（用户更正语义：它只是让模型循环播放某个动作，与「待机」无关）。
  - **⑤ 脚本生成的文件也要进「已改动文件」**：现在 `captureFileSnapshot` 只在**写类工具卡带 `file` 参数**时采快照 ⇒ 用 `Cmd`/`PowerShell` 跑脚本生成的文件永远不出现。**裁定：进列表但标「无改前内容」、不参与回退**（绝不误删用户原有文件）。 ✅ **已做，见 ㉟**。
- **㉞ 「循环播放」改名 + 修「一开就卡死」+ 价格表点击穿透（2026-10-10，㉝ 的 ④ 与 ㉜ 同批）**：
  - **① 语义更正与改名**：用户更正 —— 这个开关**只是让模型把被勾选的动作循环播放**，与「待机」无关（当初是按待机动作设计的）。对外文案统一改为「**循环播放**」（动作页按钮的 label 与 title 都改），`live2d.ts` / `live2d-engine.ts` / `PetConfig.idleMotions` 的注释一并更正；**存储键 `idleMotions` 与内部 `idle*` 标识保留**（改名会让已存配置全部失效），注释里标明属历史遗留。顺带修掉 title 里过时的「随机轮流播放」（㉕ 起已是**顺序轮播**）。
  - **② 卡死根因（与 ㉕ 同一条链，但漏在我们自己的回调上）**：`@pixi/ticker` 的 `_tick` 在 `update()` 抛异常后**不再申请下一帧** ⇒ 渲染永久停住。㉕ 只给 `internalModel.update` 包了 try/catch，**够不着我们 `app.ticker.add()` 注册的循环驱动器** —— 而库的 `model.motion()` 在「组 / 序号不存在」「优先级与预约状态互相顶掉」时会**同步抛**，那一下正好把 rAF 链掐断。**修法**：① 驱动器**整段** try/catch（吞掉 + 最多 5 条 `log_frontend` + 复位「卡住」判据、下一拍重试）；② 加 **400ms 冷却**，掐掉「10Hz 反复抢状态机」这条把预约状态顶乱的来源。
  - **③ 价格表点击穿透**（㉜）：`#token-usage-panel` 补 `pointer-events: auto`（`#app` 是 `none`，直接子元素必须自己打开）。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（**v0.9.45**、zip 327.9 KB、dev 已同步；产物里已核对含 `motion-cycle driver threw` 守卫与「循环播放」文案）。**纯前端 + 插件引擎改动，不涉及宿主 Rust**。
  - **待验**：① 打开「循环播放」后模型不再卡死（若仍卡，看日志里驱动守卫有无触发）；② 动作页文案已是「循环播放」；③ 价格表面板能正常点击、不再穿透。
- **㉟ 脚本（Cmd / PowerShell）生成 / 改写的文件进「已改动文件」（2026-10-10，㉝ 的 ⑤，用户裁定「进列表但不可回退」）**：
  - **要解决的**：AI 用 `Cmd` / `PowerShell` 跑脚本**生成 / 改写**的文件不走 `Write` / `Edit`，而既有链路只在**工具卡带 `file` 参数**时采改前原件（`captureFileSnapshot`）⇒ 这一类文件永远不出现在「已改动文件」列表里（用户原话「生成大的脚本所生成的文件也应该进入已改动的文件」）。
  - **宿主只做事实探测**：新增 `#[tauri::command] scan_agent_writes(state, since_ms, limit)`（`commands.rs`，注册进 `main.rs` 的 `invoke_handler`）—— 递归扫 `effective_workdir()`（即 agent 的 cwd：配了工作区就是它，否则 `temp\transStorage`），回 `mtime >= since_ms` 的文件路径；深度 ≤ 6、条目预算 20000、跳过 `node_modules` / `.git` / `target`，默认 200 / 上限 2000。**只回路径、不读内容**。
  - **没有改前内容 ⇒ 一律不写盘（判据写死）**：前端按 `FileSnapshot.unknown = true` 登记（`existed: true, content: null`），**不参与任何回退**。**三道闸**：① 对话回退 `pendingFileReverts()` 过滤 `!unknown`（「N 个文件将会回退」的确认文案也不算它）；② 变更审阅的「拒绝 / 全部拒绝」`applyChangeAction()` 过滤 `!unknown`；③ `restoreSnapshots()` 循环里再挡一道（写盘路径多重一层保险）。**为什么这样定**：这些文件不是某个工具的 `file_path`，我们不知道改前是什么，硬回退只可能误删用户原有文件 —— 宁可少回退，也不给假承诺（与 `snapshots.rs` 既有的「读不出来就不还原」同一条纪律）。
  - **探测开关与时机**：`AgentTurn.scriptRan` 只在 `cb.name === "Cmd" || "PowerShell"` 时置位；收尾（`cliDoneCallback`）里 `if (turn.scriptRan) registerScriptArtifacts(...)`。**`sinceMs` 必须用墙钟** `turnStartWall = Date.now()`（宿主按 epoch 毫秒比 mtime），**不能**用 `turnStartAt`（`performance.now()` 进程内相对时间）；回合下标 `turnIdxAtStart` 与 `captureFileSnapshot` 同口径。
  - **不得串线程**：桌宠轮收尾紧接 `switchThread("chat")` ⇒ `registerScriptArtifacts()` **当场捕获** `sessionFileSnapshots` / `sessionChangedFiles` 引用与 `activeSid()` 再异步写回，否则登记会落进另一条线程 / 另一个会话。已采到真快照的路径不会被降级成 `unknown`。
  - **UI / i18n**：审阅列表里 `unknown` 条目以斜体「脚本生成 · 无改前内容」（`agent.cr_script_note`，title 用 `agent.cr_script_hint`，五语言）替掉接受 / 拒绝按钮；统计列 `—`（`changeSides()` 对 `unknown` 基条返回 `null`）。
  - **验证**：`npx tsc --noEmit` exit 0；`cargo check --target-dir target-check` exit 0（仅既有 warning）。**涉及宿主 Rust** ⇒ **dev 需重编 `lunac.exe`**；`FrameDto.unknown` 带 `#[serde(default)]`，加字段前的旧帧文件照常解析。
  - **待验**：真机让 AI 跑一段会写出文件的脚本（如生成一个 `.txt` / `.ps1` 产物）⇒ 收尾后该文件应出现在任务抽屉的「已改动文件」列表里、标着「脚本生成 · 无改前内容」、**没有**拒绝按钮；此时点对话回退，「N 个文件将会回退」**不该**把这些脚本产物算进去、也不该把它们从磁盘上删掉。
- **㊱ 修「点开启循环播放依旧卡死」——销毁异常中断重挂链（2026-10-10 用户第二次报障，㉞② 的续修）**：
  - **现场证据**：宿主日志 `lunac-2026-10-10.log` 在每次桌宠窗口重挂后都记录 `[plugin-window pet] unhandledrejection: Cannot read properties of null (reading 'next')`（13:28 / 13:47 等多次），即 pixi `Application.destroy()` 抛出的那条 —— 说明 ㉔ 的 `destroyed` 幂等**只挡住了一半**（同一句柄被销毁两次），**没挡住 pixi 内部对同一个 ticker 的二次销毁**。
  - **根因（为什么表现成「卡死」而不是「报错」）**：`pet.ts` 的 `paint()` 次序是**先 `dropModel()`（拆旧）→ 再 `await mountLive2D(...)`（挂新）**，而 `dropModel()` 当初写在 `try` 块**之外**。`dropModel()` → `l2dHandle.destroy()` → `app.destroy()` 一旦抛出，`paint()` 就在**挂新模型之前**中断：旧模型已拆、新模型没挂 ⇒ 用户看到的就是「一开循环播放就卡死」（其实是重挂被中断、画面空了）。启用循环播放正好会 `saveConfig` + 广播 `reload` ⇒ `paint()`，所以症状与「开启循环」强相关。
  - **修法（两道）**：① `live2d-engine.ts` 的 `destroy()` 把 `app.destroy(...)` 包 try/catch（**吞掉 + 一条 `log_frontend` warn**）—— 销毁失败不该阻断调用方继续挂新模型；`Live2DModel.from` 失败分支里那次销毁同样包裹（别让销毁异常盖掉「载入失败」这个更重要原因）。② `pet.ts` 的 `dropModel()` 再兜一层 `try/catch`。**纪律**：任何销毁异常都不许中断「拆旧 → 挂新」这条链。
  - **③ 画布必须自己摘（2026-10-10 复测后补：用户报「开循环播放后下方多出一个模型、原位置那个停住」）**：pixi `Application.destroy(removeView=true)` 的内部次序是「**先 `ticker.destroy()`、再 removeView**」—— 抛的正是 `ticker.destroy()` 那一步，异常一抛 removeView **永远执行不到** ⇒ 旧画布留在 DOM、新画布 append 到它下面（旧的那个因 ticker 已停而「暂停」）。修法：引擎 `destroy()` 在 try/catch **之后**无条件 `canvas.remove()`；桌宠窗 `paint()` 在挂新模型前再清一次宿主机里的残留 `canvas`（自愈已处于两模型状态的窗口）。
  - **验证**：`build-plugins.ps1 -Plugin pet` exit 0（**v0.9.46**、zip 332.1 KB、dev 已同步；产物里已核对含 `app.destroy threw` 守卫与 `canvas.remove()`）。**纯前端 + 插件引擎改动，不涉及宿主 Rust**。
  - **待验**：① 打开「循环播放」后模型不再卡死、也**不会**出现第二个模型；② 宿主日志里那两条 `unhandledrejection` 变成 `[pet engine] app.destroy threw (已吞掉…)` 的 warn（说明异常被吞、重挂得以继续）；③ 若是别的路径仍卡，再查 WebView2 遮挡挂起。
- **㊲ 动作链（多个动作排一条时间线）+ 动作进度条（2026-10-10，㉝②，用户裁定「时长 + 硬停秒 / 独立动作可热键·AI 触发 / 跟模型走」）**：
  - **时间语义**：每步 = {动作文件, `loopSec` 循环播放时长, `stopSec` 硬停秒上限}。`loopSec` 到点切下一步（动作提前结束且未到点 ⇒ 引擎重播它）；`stopSec`（`0` = 不限）是本步硬停上限，到点无论播到哪都强制切 ⇒ 本步实际时长 = `min(loopSec, stopSec || loopSec)`（`pet-chains.ts` 的 `stepSeconds`）。
  - **存储**：`<模型目录>\lunac.chains.json`（沿用 `lunac.hotkeys.json` 的最小口子纪律）。新增宿主命令 `read_pet_chains` / `write_pet_chains`（`commands.rs`，经 `pet_side_file_path` 只认模型同层 + 固定文件名）并注册进 `invoke_handler`；前端数据模型 `app/src/plugins/builtin/pet-chains.ts`（`MotionChain` / `ChainStep` / `loadChains` / `saveChains` / `stepSeconds` / `chainSeconds`）。
  - **引擎**（`live2d-engine.ts` / `live2d.ts`）：`Live2DHandle.playChain(steps)` / `stopChain()`（`steps = {index, ms, hardMs}`），用一条 `setInterval(80ms)` 时间线逐步播；**链跑时循环播放驱动器让位**（`chainActive`）。另外把「每个注入动作的 `Meta.Duration`」一次性读齐（原来只读 idle 那几个），供进度与兜底超时共用。
  - **进度条**（用户附加要求「注意每个动画都有持续时间 + 每个动作下方一个进度条」）：引擎约 12Hz 回调 `MountOptions.onMotionProgress` → 桌宠窗广播 `motion-progress {chainId, p}` → 设置窗在**动作页每行**与**动作链每步**下方画 `<div class="pet-chain-bar"><i><span>`；`elapsedMs/totalMs` 按该动作 `Meta.Duration`，循环播放取模；无在播 / 读不到时长 ⇒ `null`（清零）。
  - **UI**：动作页**最上面**（与「恢复默认」同位置口径）新增「动作链」区：新建链 / 链名 / 总时长 / 播放 / 删除；每条链内可加 / 删步骤（下拉选动作文件 + 循环秒数 + 硬停秒数 + 删除），每步下方进度条。热键页新增动作类型「动作链」（下拉选链 id；**一次播放**、开关态不持久化，与 `motion` 同口径）。AI：链名进「动作词表」⇒ `PetControl action:"auto"/"chain"` 与正文 `[链名]` 都能整条播（**未改 core-agent 的 `PetControl` schema** ⇒ 不重编 `agent.exe`）。
  - **验证**：`npx tsc --noEmit` exit 0；`cargo check --target-dir target-check` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（**v0.9.46**、zip 332 KB、dev 已同步；产物含 `lunac.chains.json` / `chain-play` / `motion-progress` / `chain player threw` 标记）。**新增宿主命令 ⇒ dev 需重编 `lunac.exe`**。
  - **待验**：① 建两段链、各设循环 / 硬停秒，播放应逐步切；② 每步进度条按该动作时长走、播完自动清零；③ 绑热键后「测试」整条播；④ 模型文件夹里出现 `lunac.chains.json`；⑤ AI 用 `[链名]` 能触发。
- **插件版本**：`pet 0.9.21 → 0.9.46`（…`0.9.39` 「收起来」改「隐藏模型」+ 每个动作一个「待机循环」开关，`0.9.40` 「显示桌宠」不再走 `open_plugin_window` + 热键开关状态持久化，`0.9.41` 修「循环开着播动作卡死」（渲染兜底 + 引擎自驱 idle）**并落地口型同步**，`0.9.42` 新增「情绪标签 → 表情/动作」回退通道（`action:"auto"`），`0.9.43` 新增 ArtMesh 染色（`artmeshColor`）+ 模型旋转，`0.9.44` 抓屏子开关脱离总开关 + 触顶跨日自愈 + 引擎 `destroy()` 幂等，`0.9.46` 吞掉 `app.destroy` 异常（修「点开启循环播放依旧卡死」））；`build-plugins.ps1 -Plugin pet` 跑通（dev 已同步）。
- **验证**：`npx tsc --noEmit`（含 `plugin-window.ts` 的 `getCurrentWindow().listen` 改法）exit 0；`cargo check` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（清单 width 320 / height 510 / v0.9.25）。前序 `cargo test --bins` **204 passed**、core-agent `cargo test` **162 passed**。

**上一轮未做 / 待验**：① **未真机验收**（隐藏窗里桌宠轮能否真跑完、TTS 是否出声 / 中文语音是否命中、线程切换、历史分页、窗口观感）；② **审批边界**：桌宠轮若触发需审批的工具、而聊天窗是**隐藏**的 ⇒ 用户点不到确认卡（既有交互边界，本轮未处理）；③ 未发布市场。

- **㊳ 音乐插件「本地音乐」分页四处修正：扫描只留一枚 + 自定义队列开关归位 + 齿轮恒「设置」+ 队列入口（2026-10-10 用户四条口径 + 一条报障）**：
  - **① 扫描按钮重复**（用户口径「只留一枚」）：左栏「本地音乐」标题行那枚 `#music-local-scan`（⟳）**删除**；保留主区筛选标题行 `.music-local-head-row` 里那枚（`data-act="media-scan"`），并给它稳定 `id="music-scan-btn"`。
  - **② 保留的那枚在「自定义队列」态变「退出」**：`applyQueueModeUi` 里原来改 `#music-local-scan` 的那段改到新 id 上 —— 开启时 `title=music.local_queue_exit` + 图标 `${SVG.x}`，**点击 = 退出该态**（`queueEditOn=false`），关闭态点击才是扫描。它在**主区动态骨架**里，点击走 `onListClick` 的 `media-scan` 分派（原来那枚绑在已删除的左栏节点上，随之删除）。
  - **③ 根因：`#music-local-qmode` 被 `syncHead` 与在线页专用元素（搜索框 / 头像 / 账号 / 状态）一起按 `!localPage` 隐藏** —— 与它的语义正好相反（本地页才该看见它）。修法：把 `#music-local-qmode` 移入本地页**筛选标题行 `.music-local-head-row` 的最右侧**（长在 `#music-main` 骨架里 ⇒ 天然只在本地页出现），并从 `syncHead` 那个 `!localPage` 列表里删掉；它的点击改走 `data-act="qmode"` 委托（骨架会重建，逐个绑会变孤儿）。**开启即弹「添加队列」小窗**（该入口原由齿轮承担，见 ④）。
  - **④ 根因：`applyGearQueueState()` 在队列编辑态把「设置」齿轮换成 `${SVG.list}`「添加队列」** ⇒ 用户说的「设置按钮消失」。修法：**整体删除** `applyGearQueueState()` 及其调用点；齿轮点击去掉 `if (queueEditOn) openQueueDialog(view)` 分支，**恒 `openSettings()`**。自定义队列入口改由 ③ 的 `#music-local-qmode` 承担。
  - **顺带修的真 bug（同一条入口链）**：`placeGear()` 在本地页把齿轮搬进 `.music-local-head-row`（`#music-main` 内），而 `renderLocalMain` 会整块重写 `#music-main` ⇒ **打开一次队列页就把共享的 `#music-setup-toggle` 连同旧骨架一起销毁**（它按 id 绑了监听 ⇒ 永久失效）。修法：`placeGear` 判到 `queuePageOpen` 时把 home 收回工具条 —— `syncHead` 排在 `renderMain` 之前，换页那一刀会先把齿轮救出来。
  - **⑤ 本地队列按钮 `#music-l-queue` / `#music-lp-queue` → `openQueuePage()`**：逐条核了给定排查方向 —— ① 它确实切本地页、置 `queuePageOpen=true` 并 `renderMode` 落到 `renderLocalMain` 的队列分支；② 该分支在 `renderLocalMain` 里**排在最前**、无早退挡住（`mediaEditMode` 不 gate 它）；③ 两枚按钮都是 shell **静态节点**、每次 attach 后逐个绑仍有效；④ 在线 `queueOpen` 的外点关闭（document `pointerdown`）只碰在线浮层，不吞本地按钮。⇒ **静态链路上找不到「打不开」的成因**（如实记）。本轮把入口收敛得更稳：`openQueuePage` 先清 `queueEditOn`（与队列页互斥，免得回到媒体库后扫描钮停在「退出」）再翻标志。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin music` exit 0（`music 0.9.29`、zip 57.1 KB、dev 已同步）。
  - **待验**：**未起 dev 实例做真机点击验收** —— ① 本地页只剩一枚扫描钮；② 开自定义队列后它变「退出」、点击退出；③ qmode 在筛选行最右、只在本地页出现、开启高亮且弹「添加队列」小窗；④ 齿轮在本地页 / 队列页 / 编辑态都恒为「设置」且能打开；⑤ 本地条 / 小窗的队列按钮能稳定进队列页。**若 ⑤ 真机仍复现**，下一步带 CDP 探针抓点击链（给定的四条静态方向已排除）。
- **㊴ `AskUserQuestion` 卡新增「其他（自己输入）」+ 卡底「补充说明」大文本框（2026-10-10 用户要求）**：
  - **要解决的**：模型给的固定选项之外，用户必须能自己写 —— 否则只能被迫选一个都不贴切的选项（用户原话：「也需要可以让用户有其他这个选项并下方键入可以自定义文本框然后发送给 ai」）。
  - **修法**（`main.ts` 的 `renderAskQuestions`，参照计划卡「末页补充要求」的形态）：
    - **每题多一枚「其他（自己输入）」**（`.approval-ask-other`，标签斜体）：选中就地展开 `.approval-ask-other-input`（`maxlength=1000`）；其文本**取代**该题被选中的 label（多选时作为该题答案数组里的一项）。模型若自己就给了一条叫「其他」的选项则不重复追加。抽了 `makeOpt()` / `repaint()` 两个小闭包，避免与原来的循环复制两份选中逻辑。
    - **卡底「补充说明」大文本框**（`.approval-ask-extra-input`，`maxlength=1000`、`height:200px`、`max-height:40vh`）：非空时以 **`agent.ask_extra_key` 为键**加进 `answers`。
    - **为什么不用改 agent**：`ask_user_question`（core-agent `tools.rs` 2499-2532）先按题目顺序输出对上的答案，再把**没对上题的键**补在后面（`"键" = "值"`）⇒ 一个合成键就能把补充说明递到模型手里。
    - **空文本不计入**（选了「其他」却没写 = 该题不产生答案）；`answers` 最终为空时保持原行为（agent 报「没收到答案」，不编假答案）。新增 i18n：`agent.ask_other` / `ask_other_ph` / `ask_extra_title` / `ask_extra_ph` / `ask_extra_key`（五语言）。
  - **验证**：`npx tsc --noEmit` exit 0。**纯前端（`main.ts` + `styles.css` + `i18n.ts`）**，不涉及插件包与宿主 Rust；release 需重编前端。
  - **待验**：真机触发一次 `AskUserQuestion` —— ① 每题末有「其他（自己输入）」，选中展开输入框；② 输入后提交，模型收到的答案是该文本而非「其他」；③ 卡底「补充说明」填字后提交，模型侧 tool_result 里多出一行 `"补充说明" = "…"`；④ 只填补充说明、不选任何选项时，补充说明仍能送达。

- **㊵ 模型文件夹新增第三份 lunac 缓存 `lunac.params.json`（调试参数 + 运行时 id 表，2026-10-10，㉝③）**：
  - **要解决的两件事**：① 进插件（设置窗）**读取之前**就能拿到上次的数据 —— 「运行时 id 表」原先只在 `pet.ts` 的 `CORE_IDS_KEY`（localStorage）里，而它**只在桌宠窗挂载过一次模型之后才有**；没开过桌宠时设置窗调试区是空的。② **AI 可直接改这个文件**，用户重开设置窗时**反向同步**进面板。热键（`lunac.hotkeys.json`）/ 动作链（`lunac.chains.json`）已有各自同目录文件，本任务只做「调试参数」这一份，**不动那两份**。
  - **文件格式**（`<模型目录>\lunac.params.json`，AI 会直接读写）：`{ version:1, model:"<模型文件名>", coreIds:{ parameters:[], drawables:[], parts:[], parameterRanges:{ "<id>":{min,max,def} } }, params:{ "<参数id>": 数值 } }`。`coreIds` = 原先 localStorage 那份 `CoreIdTable`；`params` = 原先 `PetConfig.params[模型路径]` 那份。**缺字段 / 坏 JSON / 字段错型一律当空容忍，绝不抛错**。
  - **宿主命令**（`commands.rs`，紧挨 `read_pet_chains` / `write_pet_chains`）：`pet_params_path()` 经 `pet_side_file_path(model_path, "lunac.params.json")`（**只认模型同层 + 固定文件名**）；`#[tauri::command] read_pet_params(model_path) -> Option<String>`（不存在 ⇒ `Ok(None)`）/ `write_pet_params(model_path, json) -> String`（返回落点绝对路径）。两者都在 `main.rs` 的 `use commands::{…}` 与 `invoke_handler` 各注册一行。
  - **前端数据模型** `app/src/plugins/builtin/pet-params.ts`：`PetParamCoreIds` / `PetParamCache` / `emptyPetParamCache(modelPath)` / `loadPetParams(modelPath) -> PetParamCache | null`（invoke `read_pet_params`，解析失败 / 缺文件 ⇒ `null`）/ `savePetParams(modelPath, cache) -> Promise<string>`。文件头写清「为什么存在 / 存哪 / 两半各管什么、谁读谁写」。
  - **`pet.ts` 接线（设置窗）**：① `readCoreIds` → `readCoreIdsAsync`（旧函数体保留为 `readCoreIdsLocal`）—— **优先读文件的 `coreIds`**，文件没有 / coreIds 为空再回退 localStorage，**回退时把 localStorage 那份播种进文件**（首次迁移）；3 个调用点（`buildMotionTemplate` 改 async、`refreshHotkeys`、`refreshDebug`）全部 `await`，其中 `buildMotionTemplate` 的调用点（约 2499 行，在 async IIFE 内）改成 `await`。② `savedParams` → async，**优先读文件的 `params`**，空则回退 `loadConfig().params[modelPath]`；`restoreParams` 随之 async，两个调用点（`refreshDebug` 内 `await`、`resetDebugParams` 内 `await`）改到。③ `persistParam` 除写 localStorage 外**同时更新文件**：`scheduleParamFile` 用 **400ms `setTimeout` 防抖**（拖动时 `emitParamThrottled` 每 50ms 一次，别每 50ms 写盘），flush 前先 `await loadPetParams` 拿**最新文件内容**（保住 AI 的其它改动与 coreIds）、只覆盖 `params` 再写；写失败只 `console.warn`，不打断拖动。④ 迁移 = `readCoreIdsAsync` 回退分支里的播种（写一次）。⑤ 「恢复默认」（`resetDebugParams` 改 async）**同步清掉文件的 `params`**（文件是单一入口，否则 `restoreParams` 又把旧值读回来）。
  - **`pet.ts` 接线（桌宠窗，`attachPetWindow`，`h.listCoreIds()` 之后）**：`const cache = (await loadPetParams(modelPath)) ?? emptyPetParamCache(modelPath)`；用引擎刚给的 `ids` 覆盖 `cache.coreIds`（localStorage 那份仍写，老路径继续可用）；`cache.params` 为空而 `loadConfig().params[modelPath]` 非空 ⇒ 用它**播种**（只播一次，不覆盖文件里已有值 —— AI 可能刚改过）；`await savePetParams`（失败只 warn）。随后**先应用参数**（优先文件的 `params`、空则回退 config），**再** `restoreHotkeyStates`（保持既有次序）。
  - **验证**：`npx tsc --noEmit` exit 0；`cargo check --target-dir target-check` exit 0（仅既有 warning）；`build-plugins.ps1 -Plugin pet` exit 0（**v0.9.47**、zip 332.8 KB、dev 已同步）；`build-plugins.ps1` 首 3 字节 **`EF BB BF`（UTF-8 with BOM，仅一个）**；dev 产物 `app\src-tauri\target\debug\Modules\pet\index.js` 含 `lunac.params.json` 与 `read_pet_params` / `write_pet_params` 字样。**新增宿主命令 ⇒ dev 需重编 `lunac.exe`**。
  - **待验**：① 从没开过桌宠、直接开设置窗：调试区 / 热键页应能列出参数 / ArtMesh / Part（上次文件里的 `coreIds`）；② 拖动参数后模型文件夹里出现 `lunac.params.json`（防抖，不会每 50ms 写一次）；③ AI 手改文件里的 `params` 值 → 重开设置窗，拉条反向同步成新值；AI 手改 `params` → 重开桌宠窗，模型按新值生效；④ 坏 JSON / 字段错型时设置窗与桌宠窗均不崩（当空）；⑤ 换模型后文件按新模型路径生成、旧模型的记录互不影响。

- **㊶ 音乐插件「本地音乐」第二轮四处：筛选行常驻 + 队列模式改「列表勾选」+ 齿轮编辑态变「添加到队列」+ 队列按钮失效（2026-10-11 用户四条口径 + 一条报障）**：
  - **① 筛选行常驻（sticky）**：本地页那一行 `.music-local-head-row`（搜索框 + 扫描 `#music-scan-btn` + 自定义队列 `#music-local-qmode` + 齿轮 `#music-setup-toggle`）在曲目列表向下滚时**钉在滚动区顶部**。修法：`position: sticky; top: 0; z-index: 3; background: var(--surface-glass)`（`styles.css`）。滚动容器就是既有的 `.music-main`（`overflow-y:auto`）—— **不给面板加第二层滚动容器**（本仓禁止嵌套双滚动条）。它长在 `#music-main` 动态骨架里，骨架只在缺了时建一次、sticky 是纯 CSS 所以重画不丢；`padding-bottom:8px` + `margin-bottom:-8px` 把 `.music-local-page` 的 8px 行间距一起涂上，免得列表从缝里透出。
  - **② 自定义队列模式 = 直接在本地列表勾选（核心改动）**：原先只能靠小窗逐个「选文件」；现在 `#music-local-qmode` 开启即进**列表勾选编辑态**。新增模块级 `queuePickSel: Set<string>`（存 `MediaTrackDto.path`，与 `localQueue` 每项的路径同一个稳定标识）；`mediaTrackRowHtml` 在 `queueEditOn && act === "local-track"` 时把行首序号换成勾选框（`SVG.check` / `.music-mt-check`），勾中的行加 `.music-mt.sel`（强调色左缘 + 更实的底，与左栏 `.music-pl.sel` 同一套语言，且与 `.music-mt.playing` 用不同手段区分）；`onListClick` 的 `local-track` 分支在编辑态下**点行 = 勾选 / 取消**（不播放）。**退出该态（qmode 再点 / 扫描钮变「退出」）与落地后都 `queuePickSel.clear()`**；`attach` 重挂也重置。
  - **③ 齿轮编辑态变「添加到队列」**：恢复 `applyGearQueueState(root)`（常态 = `SVG.gear` + `t("music.settings")`；`queueEditOn` 时 = `SVG.plus` + `t("music.local_queue_add_picked")`），由 `applyQueueModeUi` 与 `syncHead` **各调一次**（前者覆盖本地页骨架重建，后者每 tick 覆盖「离开本地页齿轮回工具条」那条路）。齿轮点击在 attach 里分支：编辑态 → `addPickedToQueue(view)`，常态 → `openSettings()`。**两个状态下同一枚节点都真实可见、可点** —— 上一轮㊳删掉换语义是因为 `placeGear` 会把它搬进 `#music-main` 后销毁，本轮 `placeGear` 的 `!queuePageOpen` 守卫（㊳ 已加）已避免那个销毁。`addPickedToQueue`：取 `mediaTracks.items ∩ queuePickSel`（按列表顺序）**追加**进 `localQueue`（按路径小写去重），退出编辑态并打开队列页；**没勾中任何一首时退回小窗**（`openQueueDialog`，旧文件选择路径因此不失去入口）。选择「退出编辑态 + 进队列页」是因为与既有的 `submitQueueDialog` 收尾一致、用户当场看得到结果。
  - **④ 队列按钮失效（`#music-b-queue` / `#music-queue-btn` / `#music-l-queue` / `#music-lp-queue`）**：**诊断结论 = 未能确证「容器被整块 innerHTML 重写」**。逐函数读了 `renderMode` / `renderMain` / `renderPlayer` / `syncPlayerBody` / `renderLocalNowbar` / `renderLocalMini`：这四枚按钮全部长在 **`shellHtml()` 静态外壳**里（`execute()` 返回、宿主 `root.innerHTML` 写一次、`attach` 在其后调用），上述渲染函数**只对子节点 `setHtml`/`setText`**（如 `#music-bnow-cover`、`#music-play`），**从不整块重写** `#music-nowbar` / `#music-v-player` / `#music-lnowbar` / `#music-v-lplayer`。所以在静态链路上找不到「一次性绑定被重画冲掉」的证据（同 ㊳ ⑤ 的结论）。**修法（照任务首选）**：把这四枚收敛成**挂在稳定的 `view`（`.music-root`）上的事件委托**，`closest("#music-l-queue, #music-lp-queue")` → `openQueuePage()`，`closest("#music-queue-btn, #music-b-queue")` → `toggleQueue()`；同时删掉 6717-6718 的逐个绑定与 `on(["music-queue-btn","music-b-queue"], toggleQueue)`。理由：委托对「子节点/容器被任何重绘波及」与「`?.` 静默吞 null」两种成因都免疫，是覆盖该症状最稳的改法（即便真因是过期 dev 产物，重编同步后也就对了）。`openQueuePage`（`setLocalPage(true)` + `queueEditOn=false` + `queuePageOpen=true` + `renderMode`）与 `toggleQueue`（翻 `queueOpen` 重画）本身逻辑未变、未被早退挡住。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin music` exit 0（**v0.9.30**、zip 57.6 KB、dev 已同步）；`scripts/build-plugins.ps1` 首 3 字节 **`EF BB BF`（UTF-8 with BOM，仅一个；本轮改 version 后 BOM 被编辑工具抹掉，已用 `[IO.File]::WriteAllBytes` 补回并复测）**；dev 产物 `app\src-tauri\target\debug\Modules\music\index.js` 含 `music.local_queue_add_picked` / `music.local_queue_added` / `music-mt-check` / `#music-l-queue, #music-lp-queue` 字样。
  - **待验（未起 dev 真机点）**：① 本地页向下滚，筛选行常驻顶部、列表不从它下面透出；② 开 qmode 后列表每行出现勾选框、点行切换勾选（不播放）、勾中行高亮；③ 齿轮在编辑态显示「添加到队列」、点它把勾中的歌追加进队列并进队列页；常态仍是「设置」且能打开；④ 编辑态不勾任何歌点齿轮 → 弹「添加队列」小窗；⑤ 四枚队列按钮（在线栏 / 播放长条 / 本地条 / 本地小窗）点击都能开队列页。**若 ⑤ 真机仍复现**，下一步加 `log_frontend` 打点抓点击链（静态四条方向已排除）。

- **㊷ 音乐插件「本地音乐」第三轮：自定义队列升成**多条** + 列在左栏 + 本地队列按钮改下拉浮层（2026-10-11 用户两条裁定）**：
  - **① 数据模型（多条）**：原来的单条 `localQueue` + `queueName` 拆成 —— `localQueue`（仍是**这一轮在播的那批**，来自点播快照，续播用它）**与** `localQueues: { id; name; items }[]`（用户存下来的**多条**自定义队列，`id` 是时间戳 base36 + 随机串，照 `pet-chains.ts` 的 `newId` 风格）。当前选中的那条用 `activeQueueId: string | null`。两者是两件事：**选中一条队列**（`activeQueueId`）= 主区显示它，不等于开始播。
  - **② 持久化 + 一次性迁移**：新键 `lunac-music-local-queues`（localStorage，沿用 `lunac-music-local-last` 的前缀 / try-catch 容错风格）存 `{ queues, activeId }`；坏数据 / 缺字段一律当空。**迁移**：读代码确认旧版**没有独立的队列键** —— 单条 `localQueue` 只跟着「上次播放」记录落盘（`LOCAL_LAST_KEY` = `lunac-music-local-last` 里的 `queue`）；于是新键不存在时，从那条记录里取一次、包成一条队列（名字缺省回落「自定义队列」，见 `music.local_queue_edit`），**立刻写下新键**（迁移只跑一次）。⚠️ **`LOCAL_LAST_KEY` 不删**：它同时是**续播快照**（队列 + 下标 + 位置），删了「记住上次那首」会坏；两份数据的语义不同（一份续播快照、一份队列定义），不构成「两份真相」。
  - **③ 左栏列表**：`renderLocalSide` 在目录列表之后追加一块 `.music-lqs`（标题 = `music.local_queue_edit`）—— 一条队列一行（复用 `.music-pl` 那套：名字 + 曲目数），**当前选中的加 `.on`**；空列表给 `music.local_queues_empty`。行尾给一枚**小 ×**（`data-act="local-queue-del"`，复用 `.music-pl-x`）作删除入口 —— **选「小 ×」不选长按 / 右键**：本仓左栏移除入口本来就是这枚（悬停显形、与目录 / 歌单同一套），复用它用户不必学第二套手势，键盘也能靠 `:focus-visible` 够到。
  - **④ 主区由「左栏选中」驱动**：**删掉 `queuePageOpen`**，队列详情页改由 `activeQueueId !== null` 驱动（`renderLocalMain` 首支）；`#music-queue-page` 的骨架 / 列表数据源换成 `activeQueue()`（标题 = 队列名）；`data-act="queue-back"` 改成**清空 `activeQueueId`**（返回媒体库）；`#music-scan-btn` 那条「离开队列页」的 `placeGear` 守卫同步改成 `activeQueueId === null`。点左栏一条 = 选中 + `persistLocalQueues()` + 主区切详情。
  - **⑤ 创建队列 = 居中创建窗口**（本任务第 3 条）：齿轮编辑态「添加到队列」的 `addPickedToQueue` **不再直接追加**，改为 `openQueueDialog(root, 勾中歌的 paths)` 打开居中窗口把勾中的歌**暂存**为待创建 items；`submitQueueDialog` 确定后**新建一条队列**（`name = uniqueQueueName(输入框)`，items = 草稿文件按路径包成曲目）、选中它并让主区显示它。**未勾任何歌也打开同一个窗口**（窗口里可用「选文件」补）。**重名 = 追加序号**（`名字 (2)`、`(3)`…），**不弹重名提示** —— 用户在左栏当场看得到结果，多一步打断反而更烦。
  - **⑥ 本地队列按钮改下拉浮层**（本任务第 2 条）：`#music-lnowbar` 的 `.music-bend` 里新增 `#music-lq-dd`（`class="music-dd hidden"`，位置与 `#music-q-dd` **对齐**：`bottom: calc(100% + 8px); right: 0; width: 360px`）。数据 = 当前选中那条队列的 items（空 / 无队列给 `music.local_queue_page_empty`）；行沿用本地那套 `queue-track`（点 = 从这一首起播，并把该队列设为这一轮的播放队列）。显隐由新状态 `localQueueOpen` 管、`show()` + 两枚按钮 `.on`（`renderLocalQueueDD`，挂在 `renderLocalNowbar` 这条必经之路上）；点外部关闭**并入现有 document `pointerdown`**（排除 `#music-lq-dd` / `#music-l-queue` / `#music-lp-queue`）。**队列详情页保留**（仍由左栏选中驱动），但**不再由播放条按钮进入**。
  - **小窗那枚 `#music-lp-queue` 的选法 = 方案 ②（切到默认面板并打开浮层）**：小窗里没有内容区、也没有浮层的可锚位置。方案 ①（把浮层挂 `.music-root` 级、固定定位）要为小窗另造一套定位与挂点，与「#music-lq-dd 与 #music-q-dd 同形同锚」的口径相悖；方案 ② 复用同一条栏 / 同一枚浮层，`toggleLocalQueue` 里「若不在默认态先切回默认面板再开」一行解决，改动最小、锚点稳定。
  - **验证**：`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin music` exit 0（**music 0.9.30 → 0.9.31**、zip 58.8 KB、dev 已同步；）`scripts/build-plugins.ps1` 首 3 字节 **`EF BB BF`（UTF-8 with BOM，仅一个；本轮改 version 后 BOM 被编辑工具抹掉，已用 `[IO.File]::WriteAllBytes` 补回并复测）**；dev 产物 `app\src-tauri\target\debug\Modules\music\index.js` 含 `activeQueueId` / `toggleLocalQueue` / `music-lq-dd` / `local-queue-del` / `lunac-music-local-queues` / `music.local_queues_empty` / `music.local_queue_created` / `music.local_queue_delete` 字样。**i18n 表不在插件包内**（由宿主前端 `app/src/i18n.ts` 提供）⇒ 新增键在 `i18n.ts`，需重编前端才生效。
  - **待验（未起 dev 真机点）**：① 左栏出现「自定义队列」一段、建两条后各占一行、当前选中的高亮；② 点一条 = 主区显示它的曲目、`queue-back` 回媒体库；③ 行尾小 × 能删（删当前选中那条后回媒体库）；④ 编辑态勾几首点齿轮「添加到队列」→ 居中小窗居中、预填了那几首、可改名字、确定后左栏多一条并选中；⑤ 不勾任何歌点齿轮 → 仍弹同一窗口；⑥ 重名时名字自动变 `名字 (2)`；⑦ 本地条 / 本地小窗两枚队列按钮 → 弹出与在线同形同位的 `#music-lq-dd`（空队列给空态）；点外部能关；⑧ **重开插件后左栏队列还在、上次选中的那条仍选中**（持久化）；⑨ 首次升级时旧的「上次播放队列」被迁成一条。**⑩ 未发布市场**（`publish-plugins.ps1` 未跑）。
  - **保留**：㊶ 加的那段临时诊断打点（`[music] queue-btn hit` / `AFTER …`）**未删**，字段已随新模型更新（`queuePageOpen` 已删 ⇒ 改打 `activeQueueId` / `localQueueOpen` / `ddExists` / `ddHidden`）—— 用户仍在追「点了面板没出现」。

- **㊸ 音乐插件「本地音乐」第四轮：队列按钮=播放队列（纠正 ㊷）+ 自定义队列编辑改同「修改列表」套 + 跨文件夹收歌核实 + 齿轮被挤/被销毁根因修复（2026-10-11 用户四条澄清）**：
  - **① 右下角队列按钮 = 「播放队列」（纠正 ㊷）**：`#music-l-queue` / `#music-lp-queue` 开的 `#music-lq-dd` 数据源从 `activeQueue()`（上一轮错接的「当前选中的自定义队列」）**改回 `localQueue`**（这一轮在播 / 接下来要播的那批）；行改走**新 act `queue-play`**（点 = 把游标落到 `localQueueIndex` 再 `startLocalPlay`，**不动** `localQueue` 内容）；空态用新键 `music.local_play_queue_empty`；两枚本地队列按钮标题从 `music.queue`（「播放列表」，Spotify 语义）改成本地专用 `music.local_play_queue`（「播放队列」）。与在线 `#music-q-dd` 的**同形同锚不变**。左栏选中某条自定义队列 → 主区显示它，那是**另一个**入口（队列详情页），与本浮层无关。
  - **② 自定义队列（= 歌单）的编辑 = 与左栏「修改列表」同一套；去掉行内小 ×**：删除 `localQueueRowHtml` 行尾那枚 `.music-pl-x`（`data-act="local-queue-del"`）及其分派分支、i18n 键 `music.local_queue_delete`（删后无其它引用）。队列详情页（`activeQueueId !== null`）**复用媒体库那套编辑态**：左栏 ✎「修改列表」→ `enterMediaEdit`（此时也 `renderLocalMain` 重画队列页）→ 点队列行 = 勾选（`mediaTrackRowHtml` 在 `mediaEditMode && activeQueueId !== null && act==="queue-track"` 时把行首序号换成勾选框，勾选集用 `mediaSel`）→ 🗑 `stageMediaDelete` 暂存进**新集合 `queueItemDel`** → × `saveMediaEdit` 从该队列 items 里滤除 + `persistLocalQueues()`。`queue-back` / 换选另一条队列都清 `mediaEditMode` + 两个集合（未保存的暂存作废）。
    - **⚠️ 关键区分（防误删用户文件）**：`queueItemDel` 与 `mediaPendingDel` **必须分开** —— 后者存的是**磁盘目录路径**、× 会调宿主 `media_remove_root` **落盘**；前者存的是「某条自定义队列里的曲目路径」，只改内存 / localStorage 里的 `activeQueue().items`，**绝不动磁盘文件**。两者靠 `activeQueueId` 分流（`stageMediaDelete` / `saveMediaEdit` 首行分支）。
    - **取舍**：这是**真复用**（同一 `mediaEditMode` / `mediaSel` / `#music-local-del`(🗑) / `#music-local-editdone`(×) / 同一套「进编辑态→勾选→删除」交互），未走「另造等价入口」的退路 —— 代价是只多一个暂存集合 + 三处分流。
  - **③ 跨文件夹收歌 = 现有路径已满足（未改代码）**：`pickQueueFiles` 走宿主 `openDialog`（`multiple: true` + `QUEUE_EXTS` 初筛），**只认文件对话框、不认媒体库** ⇒ 天然可跨任意文件夹、可反复点、按路径小写去重；入口链 = 齿轮「添加到队列」`addPickedToQueue` → `openQueueDialog` 暂存勾中的歌 → 窗口里「选文件」补 / 改。故**无需新增入口**，仅此说明。
  - **④ 齿轮被挤到最右 / 离开后不还原 —— 根因与修法**：`placeGear()` 原先用 `root.querySelector(".music-local-head-row")`（**按类名查**）—— 媒体库页与队列详情页**各有一行同名标题行**（分别在 `#music-local-page` / `#music-queue-page` 里），`querySelector` 返回**文档序第一个** ⇒ 齿轮会被塞进**当前没在显示、或即将被 `renderMain` 整块重写掉**的那一行；再叠加 `syncHead` 排在 `renderMain` **之前**（换页那一刻 `#music-main` 里还是**上一页**骨架），齿轮被 append 进旧行后随 `innerHTML` 重写一起**销毁**（按 id 绑的监听永久失效）=「离开后不还原」。修法：① **按当前页 scoped 查**（`activeQueueId` 决定查 `#music-local-page` / `#music-queue-page` 哪一个容器），拿不到就把齿轮收回工具条；② 落点 = **标题行左起第二**（媒体库页 = 左侧第一枚功能钮 `#music-scan-btn` 右边；队列页 = 返回钮 `[data-act="queue-back"]` 右边），用 `insertAdjacentElement("afterend")` + `previousElementSibling` 判幂等 —— 不再 `appendChild` 到行尾被 flex 顶到最右。四种切换（进 / 出队列详情页、进 / 出编辑态）后齿轮都在**同一位置且可见**。
  - **保留**：㊶ 那段临时诊断打点（`[music] queue-btn hit` / `AFTER …`）**未删**，`AFTER toggleLocalQueue` 一行**新增 `localQueueLen`**（按新模型同步字段）。**不改**在线歌单那侧语义。
  - **验证**：`npx tsc --noEmit`（cwd = `d:\cc\claude-code-cli-master\app`）exit 0；`build-plugins.ps1 -Plugin music` exit 0（**music 0.9.31 → 0.9.32**、zip 59.2 KB、dev 已同步）；`scripts/build-plugins.ps1` 首 3 字节 **`EF BB BF`（UTF-8 with BOM，仅一个；本轮改 version 后 BOM 被编辑工具抹掉，已用 `[IO.File]::WriteAllText(..., New-Object Text.UTF8Encoding($true))` 补回并复测）**；dev 产物 `app\src-tauri\target\debug\Modules\music\index.js` 含 `queue-play` / `music.local_play_queue` / `music.local_play_queue_empty`，且**不含** `local-queue-del`。
  - **待验（未起 dev 真机点）**：① 本地条 / 本地小窗那枚队列按钮 → 浮层列的是**播放队列**（`localQueue`）、空态为新文案；点一行从该首起播；② 左栏自定义队列行**没有小 ×**；点进去 → 左栏 ✎ → 每行勾选框 → 🗑 → × 后队列 items 变少、**磁盘文件不受影响**、`lunac-music-local-queues` 已更新；③ 齿轮在媒体库页 / 队列详情页 / 编辑态开合四种切换后都在标题行左起第二个且可见、点击恒能开设置；④ 跨文件夹收歌（齿轮「添加到队列」→ 选文件）仍可跨盘多选。
  - **⑩ 未发布市场**（`publish-plugins.ps1` 未跑）。

- **㊹ 音乐插件「本地音乐」第五轮：统一行选中（单击选中/双击播放/右键菜单）+ 队列页筛选行 `[返回][搜索框][设置]` + 队列编辑可选中 + 去掉自定义队列空提示（2026-10-11 用户五条）**：
  - **① 队列详情页的歌曲选不中（修；口径见第 1 条）**：静态复读**未能在队列页编辑链路里定位到一个确定的早退 / 错行**（`renderQueuePageList` 参数、`mediaTrackRowHtml` 的 `queueEditing` 条件、`onListClick` 的 `queue-track` 分支逐行核过都对）。最可能的成因：上一轮把队列页的勾选**绑在 `mediaEditMode`（必须先点左栏 ✎）**，而行的默认单击仍是「按钮态 → 播这一首」；一旦编辑态没真的进去（或 `enterMediaEdit` 之后主区那次重画没落地），行就还是「点一下即播」⇒ 表现正是用户说的「没勾选框 / 点了没反应」。**修法 = 让选中与编辑态解耦**：行的**单击恒为选中**（常驻 `trackSel`），编辑态只负责「把 🗑 亮出来」。于是队列页无需先进编辑态即可选中，🗑 再走 `queueItemDel`（只从队列移除、**不动磁盘**）。
  - **② 去除自定义队列段下方提示（第 2 条）**：`localQueuesHtml` 不再画 `.music-lqs-empty` 空态行（空列表时那一段只有标题、不留空占位）；`styles.css` 里那条 `.music-lqs-empty` 规则一并删除。
  - **③ 队列页筛选行 = `[返回] [搜索框] [设置]`（第 3 条）**：`queuePageSkeletonHtml` 去掉标题文字、`[返回]` 后放只过滤**本队列**的搜索框 `#music-lq-q`（新增 `queueQuery` + 140ms 防抖，`renderQueuePageList` 客户端模糊匹配 title/artist/album）；`placeGear` 在队列页改为把齿轮**追加到行末（最右）**（媒体库页仍落在扫描钮右边）。扫描 / qmode 本就不在队列页。
  - **④ 歌曲行右键上下文菜单（第 4 条）**：新增单例节点 `#music-ctx`（长在 shell、鼠标位置弹出、`position:absolute` 相对 `.music-root`、点外部 / Esc 关闭；样式与项样式**逐条对齐桌宠窗 `.pet-menu`**）。菜单项按上下文：本地库的歌 → 「添加到队列…（二级：已有队列 + 新建队列…）」/「在资源管理器中显示」；自定义队列的歌 → 「从队列中移除（危险色，只改队列、不动磁盘）」/「在资源管理器中显示」；在线行 → 「播放」。**⚠️ 未做「删除本地文件」**：本仓**没有**按文件删除的宿主命令（只有删「目录根」的 `media_remove_root`，拿文件路径去调它既删不掉文件、又会误伤索引），而本轮改动被限定在 `music.ts` / `styles.css` / `i18n.ts` / `build-plugins.ps1`，加宿主命令还要重编 `lunac.exe`（超出给定验证链）⇒ **改用现有 `reveal_in_explorer`「在资源管理器中显示」作替代**（用户在资源管理器里自行删除）。若日后要真删，需新增 `media_delete_file(path)`：删文件 + 删该行索引 + 清封面缓存，再配二次确认。
  - **⑤ 取消歌曲行「按钮态」（第 5 条，选定的选中模型）**：新增**统一选中集合 `trackSel`**（`Set<string>`，本地存 `path`、在线存 `uri`），**并入原 `queuePickSel`**；单击 = 加入 / 移出（高亮 `.music-mt.sel` / `.music-tr.sel`），双击 = 播该曲，右键 = 菜单。**选中模型：不完全并入 `mediaSel`** —— `mediaSel` 是左栏「目录」编辑态的勾选集，🗑 会把它喂给 `media_remove_root` **落盘删目录**；把文件路径混进去会有「把某个文件当目录根删」的风险（安全边界）。故行选中一律走 `trackSel`，`mediaSel` 只留目录 —— 概念上仍是同一个「选中集合」，只是按**可执行动作**分了两张表（编辑态 = 选中集合 + 亮出 🗑）。**Shift / Ctrl 不做特殊处理**：普通单击即 toggle，天然支持「点几行选几行」的多选。为让 dblclick 能触发，单击**只就地改 class / 勾选框、不重建列表**（重建会换掉被点节点，浏览器就不再发 dblclick）；双击处理器挂在稳定的 `view` 上按 `data-act` 分派。队列页有筛选时按 **path** 定位起播（`data-i` 只是过滤后下标，直接索引 `q.items` 会指错歌）。**不改**在线歌单的数据来源与播放命令语义（只改行的交互）。
  - **验证**：`npx tsc --noEmit`（cwd = `d:\cc\claude-code-cli-master\app`）exit 0；`build-plugins.ps1 -Plugin music`（cwd = 仓库根）exit 0（**music 0.9.32 → 0.9.33**、zip 61.1 KB、dev 已同步）；`scripts/build-plugins.ps1` 首 3 字节 **`EF BB BF`（UTF-8 with BOM，仅一个；本轮改 version 后 BOM 被编辑工具抹掉，已用 `[IO.File]::ReadAllText/WriteAllText(..., Text.UTF8Encoding($true))` 补回并复测）**；dev 产物 `app\src-tauri\target\debug\Modules\music\index.js` 含 `music-ctx` / `music.ctx_add_queue` / `music.ctx_remove_queue` / `music-lq-q` / `reveal_in_explorer` / `trackSel`，且**不含** `local-queue-del`；诊断打点 `queue-btn hit` 仍在。
  - **待验（未起 dev 真机点）**：① 队列详情页：任意单击一行即高亮为选中（不必先点左栏 ✎）；左栏 ✎ → 行首勾选框出现 → 勾几首 → 🗑 → × 后该队列 items 变少、**磁盘文件不受影响**、`lunac-music-local-queues` 已更新；② 左栏「自定义队列」段在空的时候**只剩标题**、无提示行 / 无空占位；③ 队列页筛选行是 `[返回] [搜索框] [设置]`，搜索只过滤该队列、清空后恢复；④ 歌曲行 **单击 = 选中、双击 = 播**（本地媒体库 / 队列详情 / 在线歌单三处），右键在鼠标位置弹菜单、点外部 / Esc 关闭；⑤ 菜单项：「添加到队列…」二级能选已有队列或新建、「从队列中移除」只改队列、「在资源管理器中显示」能定位到文件（在线行只有「播放」）；⑥ **未做真删文件**（见 ④ 的说明）。
  - **⑩ 未发布市场**（`publish-plugins.ps1` 未跑）。

- **㊺ 音乐插件「本地音乐」第六轮：歌曲行/左栏行去掉外围 UA 边框 + 选中态改「外围显黄色框」+ 右键菜单呼不出来根因修复（2026-10-11 用户第二轮澄清三条）**：
  - **用户口径的纠正（先记下来）**：上一轮㊹把歌曲行改成「单击 = 选中 / 双击 = 播放 / 右键 = 菜单」，**交互是对的、用户认可**；这一轮处理的**不是「按钮态」，而是「视觉与边框」**——用户报三处**外围那条线**要拿掉，另加「右键菜单呼不出来」。故本轮**只改视觉与右键菜单本身，不动交互语义**。
  - **A. 去掉「外围那一条 border」**（用户第 A 条，三处）：
    - ① 在线歌单的曲目行（`trackRowHtml` → `<button class="music-tr">`）；
    - ② 本地播放的曲目行（`mediaTrackRowHtml` → `<button class="music-mt">`，媒体库列表 + 队列详情页列表都在内）；
    - ③ 两个页面**左栏的歌单行**（在线页歌单 / 本地页目录 / 本地页「自定义队列」，都复用 `<div class="music-pl">` 里的 `<button class="music-pl-btn">`）。
    - **逐类核过 `styles.css`：`.music-tr` / `.music-mt` / `.music-pl-btn` 此前都*没有自己的 border 声明*** —— 用户看到的那圈线就是**浏览器给 `<button>` 的 UA 默认外框**（全局只有 `*,::before,::after` 的 margin/padding 归零与 `#app button:where(...)` 的**底色**统一，**没有任何一条按钮 border 归零**）。所以在三处各补一句 `border: none;`（内边距 / 悬停底色 / `border-radius` 一律不动）。**没有删除任何既有 `border` 规则**（本就不存在）；`outline` 静态态本类也没有声明（`:focus-visible` 的键盘焦点框保留，不动）。悬停态也没有靠 border 变色的规则。
    - **取舍**：`.music-tr` / `.music-mt` 是**行本体**的 UA 边框，`.music-pl` 行本体是 `<div>`（无边框），其外框感来自内层 `.music-pl-btn` 这个 `<button>`；三处合起来正好覆盖用户点名的全部位置（且左栏目录行也顺带一致）。
  - **B. 选中态 = 外围显眼的色框（用户第 B 条）**：
    - `.music-tr.sel` 与 `.music-mt.sel`（`trackSel` 那套，单击即选中）原来都是「**强调色左缘**（`box-shadow: inset 2px 0 0 0 var(--accent)`）+ 更实的底」；本轮**改成四边一圈 2px 实线框**：`box-shadow: inset 0 0 0 2px var(--yellow)`。
    - **选色与避让理由**：「正在播放」那一行（`.music-tr.playing` / `.music-mt.playing`）用的正是 **`--accent`**（文字色）。若选中框也用 `--accent` 就与它撞色、同色系分不清，故**改用 `--yellow`**（本仓既有语义色，`#c9b896`，醒目且与 `--accent` 尘玫瑰拉开）。同时**把 `sel` 里原来那句 `color: var(--text)` 删掉** —— 这样「既在放、又被选中」的那一行，强调色文字仍能透出来，**两枚标记（accent 文字 + yellow 外框）可同时存在且一眼可分**。背景由 `rgba(var(--accent-rgb),0.14)` 换成同色系的淡黄 `rgba(201, 184, 150, 0.14)`（本仓既有写法：`--yellow` 没有 `-rgb` 三元组，需要带 alpha 时一律写字面 `rgba(201,184,150,α)`，见 `#music-devices-btn.warn` 那几处）。
    - **为什么用 `inset` 的 box-shadow 而不是 `border` / 真正向外的 `outline`**：① `border` 会把行内容整体推 2px（进出选中态行内会跳，与 `.music-pl.sel` 当初不用 `border-left` 同一条取舍）；② 正向外扩的框（`outline` / 非 inset box-shadow）在 `.music-mt` 这种 `width:100%` 的行上会被 `.music-main` 的 `overflow-x` 裁掉（`overflow-y:auto` 会把另一轴也变成 `auto`），而 `inset` 画在盒内、不吃布局、不会被裁。视觉上仍是「一圈框住整行」。
    - **只动 `trackSel` 那两张行**（`.music-tr.sel` / `.music-mt.sel`）；左栏 `.music-pl.sel`（目录编辑态）**不在本轮范围、不改**。
  - **C. 右键菜单呼不出来 —— 根因已确证（用户第 C 条）**：
    - **根因**：`#music-ctx` 这个单例菜单节点被**误放进了 `curveShellHtml()`（频响曲线窗的外壳）**，而不是主插件窗的 `shellHtml()`。（`execute` 里 `isCurveWindow ? curveShellHtml() : shellHtml()`，两个窗各自一份外壳。）而 contextmenu 监听挂在 **主窗**的 `view` 上（`attachMusicListeners` 开头 `if (isCurveWindow) return attachCurveWindow(root)` **早退**）⇒ **主窗 DOM 里根本没有 `#music-ctx`**，`openCtxMenu()` 里 `root.querySelector("#music-ctx")` 恒为 `null`、函数首行 `if (!box) return;` **直接返回**，菜单永远不出；curve 窗虽有节点却从不挂监听。**这是静态可确证的根因**（用 dev 产物核对：主窗 shell 里没有 `#music-ctx`、curve shell 里有）。
    - **修法**：把该节点（连同其注释）从 `curveShellHtml()` **移到 `shellHtml()`**（主窗 `.music-root` 的子节点，与 `#music-msg` 相邻）。**未加临时诊断打点**（根因已确证，不需要）。
    - **顺带覆盖的其余四个疑点**（逐一核过，均非本次病根，但按用户清单确认）：① `preventDefault()` **已调**（`ev.preventDefault()` + `stopPropagation`，原生菜单不会并存）；② 定位：`position: absolute` 相对 `position: relative` 的 `.music-root`，越界时翻转，不在窗外；③ 显隐：`.music-ctx.open { display: flex }`（`classList.add("open")`），不是 `hidden`；④ 指针事件：既然菜单节点在 `#app { pointer-events: none }` 的继承链上（见 ㉜），**给 `.music-ctx` 补了 `pointer-events: auto`**（与 `#token-usage-panel` / `#context-menu` 同一条纪律，属防御性加固）；⑤ 事件被当场关掉：`document` 的 `pointerdown` 关闭逻辑是 `if (ctxTarget && !closest("#music-ctx"))` —— 首次右键时 `pointerdown` 早于 `contextmenu` 且 `ctxTarget` 仍为 `null`，不会自关。
  - **纪律**：沿用既有风格；模板字符串里的注释**已确认无反引号**（首次改完 `tsc` 报 `TS1005`，正是注释里写了 `curveShellHtml` / `openCtxMenu` 两对反引号所致，已去掉）；`querySelector` 一律带 `#`；**保留**第五轮前那段临时诊断打点（`[music] queue-btn hit` / `AFTER …`）**未删**；不新增嵌套滚动容器。
  - **验证**：`npx tsc --noEmit`（cwd = `d:\cc\claude-code-cli-master\app`）exit 0；`build-plugins.ps1 -Plugin music`（cwd = 仓库根）exit 0（**music 0.9.33 → 0.9.34**、zip 61.3 KB、dev 已同步）；`scripts/build-plugins.ps1` 首 3 字节 **`EF BB BF`（UTF-8 with BOM，仅一个）**；dev 产物 `app\src-tauri\target\debug\Modules\music\index.js` 含新注释「必须长在「主插件窗」的 shell（这里）」与 `<div class="music-ctx" id="music-ctx"></div>`（在主窗 shell 段内）。
  - **待验（未起 dev 真机点）**：① 在线歌单 / 本地媒体库 / 队列详情页三处的曲目行**没有外围那圈细线**（hover 底色、圆角、内边距照旧）；② 两个页面**左栏**的歌单 / 目录 / 自定义队列行**没有外围细线**；③ 任意单击一行 → 出现**一圈显眼的黄色 2px 外框**（四边，圆角跟随行）；④「正在播放」的那一行**只用强调色文字**、若同时被选中则**黄框 + 强调色文字并存**、两者可分；⑤ **右键任意歌曲行能弹出 `#music-ctx`**（本地库 / 队列详情 / 在线歌单三处），点外部 / Esc 关闭，菜单项可点。
  - **⑩ 未发布市场**（`publish-plugins.ps1` 未跑）。

- **㊻ 音乐插件「本地音乐」第七轮：新增宿主命令 `media_delete_file` + 右键「删除本地文件」二次确认 + 本地歌曲行圆角对齐在线（2026-10-11 用户口径）**：
  - **① 新宿主命令 `media_delete_file(path: String) -> Result<(), String>`**（[media_lib.rs](file:///d:/cc/claude-code-cli-master/app/src-tauri/src/media_lib.rs) 紧接 `media_remove_root` 之后；注册在 `main.rs` 的 `generate_handler!` 里 `media_lib::media_remove_root` 之后）：删**单个文件** = 删**磁盘文件 + 库里那行 + 它的封面缓存**。用 `crate::commands::run_blocking` 包住同步逻辑（与同文件其它命令一致）。
    - **安全边界三条（缺一就有「拿任意路径删别人文件」的口子）**：① **只认库里已有的曲目路径** —— 先 `SELECT cover FROM tracks WHERE path = ?1`，**查不到直接报错、不删任何东西**（这一条同时挡住任意路径删除）；② **只删文件**（`std::fs::remove_file`，先 `is_file()` 判一次），**绝不删目录**；③ 目标文件**已不存在**（用户手动删过）时**仍然**清掉库里那行与封面缓存、按「已清理索引」处理、**不报错**。
    - **顺序与失败策略**：**先删库行（查库时顺手拿到 `cover` 文件名）→ 删磁盘文件 → 删 `covers_dir().join(cover)`**。把索引先清掉，界面立刻不再列出这首；后两步失败只记 `crate::log::warn`、**不把整体判失败** —— 反过来（先删文件后删索引）一旦索引那步失败，会留下「报错了、但歌已经没了」的半死不活状态。记一条 `crate::log::info`（形如 `media: 已删除文件 {path}`）。与扫描互斥：照 `media_remove_root`，扫描中返回 `Err("ERR_SCAN_RUNNING")`。
    - ⚠️ **封面是内容哈希命名的共享缓存**（同专辑几十首共用一张图）：`covers_dir().join(cover)` 可能被同专辑其它还在的曲目引用 ⇒ 本轮**按规范字面**无条件删这个文件（它被定义为「可删缓存」，删掉只是下次要重抽一次封面，索引不受影响）；若要避免同专辑其它行暂时破图，后续可加「无其它行引用才删」的判据。
  - **② 右键菜单补「删除本地文件」**（`music.ts`）：本地媒体库 / 队列详情页那条「在资源管理器中显示」**保留**，其后新增一枚**危险色**「删除本地文件」（`data-ctx="delete-file"`，`ctxMenuItemsHtml` 里 `row("delete-file", t("music.ctx_delete_file"), true)`）。**不再用「在资源管理器中显示」顶替**（㊹ ④ / ㊺ 的待办在此落地）。
  - **③ 二次确认（居中模态，**不用** `window.confirm`）**：点「删除本地文件」→ 先关右键菜单，再弹**居中确认卡 `#music-del-dlg`**（静态骨架，长在**主插件窗 shell**、与 `#music-queue-dlg` 相邻；复用那套 `.music-dlg-mask` / `.music-dlg` / `.music-dlg-title` / `.music-actions` 观感）。文案 `music.ctx_delete_confirm`（带 `{name}` 曲目名、写明**不可恢复**）。确认（`#music-del-ok`）→ `invoke("media_delete_file", { path })` → 成功后 `trackSel.delete(path)`、若**正在放这首**（`localPlayer?.path === path`）则走现有 `stopLocalPlayback(root)`（不复用队列跳过：删一首不该偷偷续播下一首）、`await loadMediaTracks(root)` 刷新曲目列表、`note(root, t("music.ctx_deleted"))`。失败（如 `ERR_SCAN_RUNNING`）用 `note`（= `setMsg`）显示错误、**不静默**。取消（`#music-del-cancel`）/ 点遮罩空白关闭。
  - **④ i18n（五语言）**：新增 `music.ctx_delete_file`（菜单项）/ `music.ctx_delete_title`（确认卡标题）/ `music.ctx_delete_confirm`（带 `{name}`，写明不可恢复）/ `music.ctx_delete_cancel` / `music.ctx_delete_ok` / `music.ctx_deleted`，命名沿 `music.ctx_*` 风格。
  - **⑤ 本地歌曲行圆角对齐在线（本任务第 2 条）**：读 `styles.css` —— 在线行 `.music-tr` 是 **`border-radius: 10px`**（悬停 `.music-tr:hover` / 选中 `.music-tr.sel` **都不另写圆角**，跟随基线）；本地行 `.music-mt` 此前**没有圆角声明**（= 0，方角）。**两处圆角本就不同**，故给 `.music-mt` **只补一句 `border-radius: 10px`**（`width` / `margin` / `padding` 一律不动 —— 在线行左右各 4px 的「缺口」`margin: 0 4px` + `width: calc(100% - 8px)` 是另一件事）。悬停 / 选中态（`.music-mt.sel`）自动跟随基线，无需各写。上一轮的 `border: none` 与选中框 `box-shadow: inset 0 0 0 2px var(--yellow)` 在圆角下仍正常（`inset` 阴影会**跟随 `border-radius` 弯曲**、且不吃布局）。
  - **纪律**：沿用两边既有风格；模板字符串里的注释**已确认无反引号**；`querySelector` 一律带 `#`；**保留**第五轮前那段临时诊断打点（`[music] queue-btn hit` / `AFTER …`）**未删**；不新增嵌套滚动容器。
  - **验证**：`npx tsc --noEmit`（cwd = `d:\cc\claude-code-cli-master\app`）**exit 0**；`cargo check --target-dir target-check`（cwd = `app\src-tauri`）**exit 0**；`build-plugins.ps1 -Plugin music`（cwd = 仓库根）**exit 0**（**music 0.9.34 → 0.9.35**、zip 61.8 KB、dev 已同步）；`scripts/build-plugins.ps1` 首 3 字节 **`EF BB BF`（UTF-8 with BOM，仅一个；本轮改 version 后被编辑工具抹掉，已用 `[IO.File]::ReadAllText` + `WriteAllText(..., UTF8Encoding($true))` 补回并复测）**；dev 产物 `app\src-tauri\target\debug\Modules\music\index.js` 含 `media_delete_file` / `#music-del-dlg` / `music.ctx_delete_file` / `music.ctx_delete_confirm` / `delete-file`，诊断打点仍在。
  - **待验（未起 dev 真机点）**：① 右键本地库 / 队列详情页的歌 → 菜单出现**两项**（在资源管理器中显示 / 删除本地文件，后者危险色）；② 点「删除本地文件」弹**居中确认卡**、文案带歌名 + 不可恢复说明，取消 / 点遮罩可关；③ 确定后磁盘文件消失、列表少一行、该行移出选中集合、封面缓存已清；④ **正在播放**的正是这首时删除会**停止播放**；⑤ **扫描进行中**删除报错并在底部提示（`ERR_SCAN_RUNNING`）；⑥ 本地歌曲行（`.music-mt`）四角圆角与在线行一致（10px），选中黄框随圆角。
  - **⑩ 未发布市场**（`publish-plugins.ps1` 未跑）。
  - ⚠️ **dev 需重编 `lunac.exe`**：新增了宿主命令 `media_delete_file`，前端 `invoke` 只有在 `main.rs` 注册后重编的 exe 里才存在（仅 `build-plugins.ps1` 同步的 `Modules\music\index.js` 不够）。

---

## 1. Agent 能力缺口

### P3

**A13. 按需重估的剩余项**（不做只因为优先级，不是因为没价值）

| 项 | 说明 / 何时重估 |
|---|---|
| 斜杠命令 | 旧 CLI 有 75+，多数是 CLI 会话内操作（`/theme` `/vim` `/statusline` `/login` …），桌面端另有 UI。**只挑与能力相关的子集**（`/compact`、`/rewind`、memory 类）评估，不整体照搬 |
| 记忆目录（CLAUDE.md 体系） | **项目级那一层已于 2026-10-01 落地**（工作目录下的 `AGENTS.md` ⇒ `env_section`，见 `ai-spec.md` §20.1 与预检 #54）。**仍未做**：多层 / 向上递归的记忆目录（`CLAUDE.md` 系列）、子目录级指令；要做也得先想清「哪一级算数」，别直接照搬 |
| `NotebookEdit` | **✅ 已实现（2026-10-11）**（按 cell 编辑 `.ipynb`，写类、要审批），已从本待估清单移出 —— 见 `ai-spec.md` §3.5 契约表与 [agent-implementation.md](./agent-implementation.md) §4.1。原降级理由「Jupyter 场景，用户群不大」随实现作废 |
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
  6. **输出侧三刀（2026-10-07 用户选定；数据与口径见 `ai-spec.md` §11 规则 88）** —— 由当日平台账单（`usage_data_2026-10-07`）与本地 `usage-*.jsonl` 定位：Lunac 账单里**输出占 61.7%**（同口径 Trae 工作流只占 35.7%），是 Lunac 的第一大头。**已落地三条**：① `SYSTEM_PROMPT` 加「改已存在的文件用 `Edit`，`Write` 只用于新建 / 确实要整体重写」—— 实测 8 次真实任务（含 2 文件重命名）**Write 0 次 / Edit 28 次**；② `max_tokens_for` 的思考档余量 `4096 → THINKING_HEADROOM_TOKENS = 16384`（旧值 12288 **实测把答复截断过 2 次**，见 `usage-2026-10-07.jsonl`）；③ 思考档 A/B 复测。**对 2026-09-29 那条「思考不是主因（1.0~1.3×）」的修正**：那句话在**总账**口径下仍成立（输入才是总账大头），但**在输出这一侧，思考是最大单项** —— 只切 `LUNAC_THINKING` 的 4+1 组同任务实测：思考占生成字符 **24%~66%**（硬任务 64%），输出 token **+36%**（3328 vs 2446），硬任务 **+141%**（1138 vs 472）。**未做**：把思考档从「开 / 关」做成「关 / low / high」—— DeepSeek 的 Anthropic 兼容端点是否认 `thinking.effort`（而非被静默忽略的顶层 `reasoning_effort`）**尚未实测**；料想里这是「保住推理质量的同时压掉一半思考」的唯一途径。
  7. **阶段小结（2026-10-08 用户要求，L6 输出纪律的唯一例外）** —— 用户原话「每段思考后都有一次小输出作为此段思考的总结，并且最后的输出应该总结所有的小输出」。**为什么这个例外值得开**：思考块**不回灌**（回灌 400）⇒ 一轮工具往返之后模型的推理在上下文里凭空消失，只能从 `tool_result` 重推；而 `text` 块**会**进历史（`hist.push(blk)`）⇒ 那行小结是**把这一段决策带进下一轮的唯一载体**（完整推导见 `ai-spec.md` §11 规则 88 ⑤）。落地在 `PERSONA_AND_STYLE` 的 Output Style 段（L6 那句之后），范围**刻意收窄到「多于一次工具往返」**（用户选定）：单步问答与闲聊一律不加，否则会把 2026-09-29 的 -41% 降本吃回去。守卫单测 `phase_summaries_are_scoped_to_multi_round_tasks`。**待实机验收**其真实降本 / 增本净效果。

- **未做（只剩一步）**：
  1. **跨源基线** —— 拿平台导出的 CSV 跑一次 `reconcile-usage.ps1 -Csv …`，出「本地 vs 平台」的逐小时差。本仓不存供应商账单，需要用户从平台导出一份（一次操作）。
- **验收口径**：任何降本改动按「**每次提问成本**」验收，不许拿「元 / 百万 token」当指标（理由见 `ai-spec.md` §9.1 难点 3 结论 3）。

> **L6.1（宿主后台活动的可观测性，2026-10-08）已结案并于 2026-10-09 撤下** —— 病根由 `activity:` 日志点名（游戏本体 `launcher.exe`/`yysls.exe`、代理 `hongmo.exe`、IDE `Trae.exe`；`lunac.exe` 自身从未上榜）⇒ 卡顿与 Lunac 无关；据此删除 `activity.rs` 并给索引扫盘加 Windows 后台模式。结论与纪律留在 `ai-spec.md` §11 规则 89（② 标记撤下、新增 ⑤）。

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

- **① 屏幕获取**：宿主已有 `windows_ocr.rs` / `paddle_ocr` 那条「截图 → 识别」链路可复用（至少截图那半段现成）。**要做的是**：① 定「截什么」（全屏 / 某显示器 / 某个窗口 / 只截活动窗口）；② **定频与停的条件**（桌宠最小化 / 隐藏时必须停 —— 宿主已经会下发「可见吗」，见 `ai-spec.md` §4.8，直接挂上去）；③ **隐私红线**：截屏内容**默认不上传**，只在用户显式开启「看屏幕」时按需截、且**必须在界面上有可见标识**（用户知道它在看）。**✅ 2026-10-09 已完成「手动」这一半**：宿主新增 `screenshot_monitor(label?)`（抓**当前显示器整屏** —— **宿主首次有显示器枚举**，`MonitorFromWindow`+`GetMonitorInfoW`）；桌宠右键「**看一眼屏幕**」→ **抓屏前先亮可见标识** → 把图作为**附件**送进**现有对话链路**（`startAIChat(query,[path])`，**不新开 API 通道**；「聊天窗已开 / 未开」两条路都覆盖）；`vision` 关时**明确告知不发送**。插件 `pet 0.9.14`；规范见 `ai-spec.md` §4.5。**仍未做**：「截什么」的其它档（某窗口 / 活动窗口）、**自动 / 定时**抓（那需要「默认关 + 频次上限」，属 ② 的前置）、以及 ② ③。
- **② 内置到 Lunac 的 AI agent 以做出反应**：形态建议 = 桌宠**不自己起 agent**，而是往**现有聊天窗/agent 那条链路**发一条用户可见的消息（于是工具、审批、用量记录、历史会话全部自动复用 —— **不要**给桌宠单开一条 API 通道，那会造出第二份「agent 在哪、花多少钱」的真相）。桌宠侧只负责：触发条件（定时 / 用户交互 / 屏幕变化）+ 表情动作的反应呈现。
  - **✅ 2026-10-09 已完成「对话框 + 主动反应」两半**（用户选定这个形态）：契约 = `ai-spec.md` §4.5 的 `pet-chat-send` / `pet-chat-reply`（**take 语义**保证「只发一次」、复用 `startAIChat`、**不新开 API 通道**）；落地记录见 §0 **M2-19**。**仍未做**：屏幕变化触发（依赖 ① 的自动 / 定时抓屏）。
  - **成本红线**：这条链路会**持续烧 token**。默认必须是**关**，且要有**频次上限**（例如最小间隔 + 每日上限），触顶后自动停并如实告知。**✅ 已落地**：`PetConfig.proactive` 默认关 + 间隔（15/30/60/120 分）+ 每日上限（自然日计数、触顶自停、跨日恢复）+ 不可见即暂停。
- **③ 桌宠 tokens 消费的独立表格**：`usage-*.jsonl` 已有 `sessionId` 归因（A11）与按天聚合（`usage-cost.ts` + 成本面板）。**要做的是给它一个来源维度**：桌宠那一路的请求必须带上可区分的归因（**首选**复用 `sessionId` 前缀或新增一个来源字段，**不要**靠「时间戳猜」），然后成本面板里加一块「桌宠」表格（同一份 `usage-cost.ts` 算法，**禁止**另写一套金额计算）。
  - **✅ 2026-10-09 已落地**：新增来源字段 `UsageRecord.source`（桌宠轮 = `"pet"`，经 `turnUsageSource` 在 `startAgentChat` 开头认领），成本面板加「**来源**」下拉（全部 / 桌宠 / 普通聊天）分区过滤，金额仍走既有 `dayCost`（**未另写算法**）。**形态上不是「独立一张表」**，而是给同一个面板加**来源维度** —— 与用户「跟日期一样设一个下拉框」的口径一致（见 §0 M2-19）。
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

以下是不再有实现意图、或只有一句话想法的东西。**保留只为防止重复讨论**，真要做时先重写落地路径再挪进正文。**例外是 §4.1** —— 它是与用户逐条对齐后立项的**长期方向路线图**（尚不排期），其条目在进入实施前必须**提升为 §1–§3 的正文**（写明「做什么 / 为什么值得 / 依赖 / 落点文件」，见文首「三条维护纪律」第 2 条），本节自身**不构成待办**。

### 4.1 长期 AI 侧规划（分阶段能力成熟度路线图，2026-10-09 立项）

> **来源与口径**：本条由用户 2026-10-09 提出（「创立一个长期 AI 侧的计划」，主题四块：①本地模型参与决策 ②agent 整体思路与框架图 ③skills / tools 完善与长任务托管 ④到达 AGI 的要求）。三项已对齐的决策：**并入本区**（不新建文件）· **本地模型 = 独立本地推理运行时（llama.cpp 内嵌）**· **按能力成熟度分期**。用户同日追加约束：**小模型 · 开箱即用 · 随包分发 · 体量小**。
>
> **分期依据 = 能力成熟度**（不是投入产出比）：**S0** 单模型增强 → **S1** 本地决策旁路 → **S2** 长任务托管 → **S3** 子 agent 团队 → **S4** 持续自治。每一期给出「目标 / 依赖 / 验收口径」，进入实施前先提升为正文条目。

#### 4.1.0 现状基线（2026-10-09 逐条对照 `core-agent/src/`、`app/src-tauri/src/` 实测）

| 面 | 现状（已落地） | 缺口（本规划的靶子） |
|---|---|---|
| 模型 | 单一远端 Anthropic 兼容端点（`{base}/v1/messages`，Bearer 鉴权）；`tools` 数组是第一等公民 | **无任何本地模型**；`proxy_server.rs`（Anthropic↔OpenAI 翻译）**已停用**，原因是它**丢弃 `tools` 数组** ⇒ 模型退化成文本式 XML 工具调用（[ai-spec.md](./ai-spec.md) §3.3） |
| 主循环 | `run_query`：`tool_use` / `tool_result` 往返；回合预算 50 万 token + 压缩续命（`MAX_BUDGET_EXTENSIONS = 2`）；轮次上限 64 | 预算撞满后**收口停止**（`TURN_BUDGET_HINT`）⇒ 长任务会中途断；**没有跨回合的检查点 / 自动续跑** |
| 子代理 | `Agent` 工具：同批并发（≤3）、独立上下文、只回报告；同批兄弟间 `ListPeers` / `SendMessage` | **无常驻团队 / 无角色分工 / 无任务 DAG**；主循环在子代理批期间被阻塞 ⇒ 通信只存在于同批之间 |
| 上下文 | 两级压缩（水位 0.85 / 0.95 + 滞回）+ 摘要压缩 + 工具分级瘦身 + 任务快照（`latest_todo_snapshot`）+ 冷/热缓存判据 | 压缩按**体积**而非**语义**分层；无「工作记忆 / 情景记忆 / 语义记忆」分层 |
| 记忆 | `MEMORY.md`（跨会话，`Remember` 工具）+ `AGENTS.md`（项目级，启动注入）+ `SessionSearch`（chat.db FTS5 检索） | 无**自动抽取**（现有 `Remember` 靠每 10 轮的后台复盘 fork）；无**多级 / 向上递归**目录；无**记忆淘汰 / 冲突消解** |
| 技能 | `skills\<key>\SKILL.md`；渐进披露；inline / fork 两模式；可带脚本资源 | 无版本 / 依赖 / 市场；fork 与 `Agent` 同族但**无角色语义** |
| 工具 / MCP | 17 内置 + 条件注册（`Skill` / resources / prompts / `Remember` / `ImageGen` / `mcp__*`）；MCP 支持 stdio 桥 + Streamable HTTP + OAuth 2.1 + `.mcp.json` + roots / elicitation / sampling 部分 | `sampling`（服务端反向调用模型）未做；CLI 面只有 `--disallowedTools` / `--mcp-server`，**没有可复用的「一次性 / 无头 / 引擎」子命令** |
| 联网搜索 | `WebSearch`（主源 bocha / tavily / exa / firecrawl，兜底 Bing RSS → Bing HTML → 百度）+ `WebFetch` 转纯文本 | 无引用溯源 / 无新鲜度控制 / 无结果缓存去重 / 无多源交叉验证 |
| IDE | `vscode-extension/`：Chat Participant（Path A）+ Webview 面板（Path B）+ 侧栏 ChatView（Path C）+ URI Handler | 扩展**自建一套供应商配置**（`providers.ts`）**直连端点、零工具、零审批、零记忆** ⇒ **不是 Lunac agent 的客户端**，是另一条平行链路 |
| 调度 | 无 | 无长任务队列 / 无后台常驻 / 无 IDE 侧任务续跑 |

#### 4.1.1 主题一：本地模型**只参与决策**（独立本地推理运行时）

**目标**：内嵌一个**独立本地推理运行时**（`local-engine`，llama.cpp），随包分发一个**小体量、开箱即用**的 GGUF 模型；**只把「决策」这一类调用交给它**，重推理与工具往返仍走云模型。

**为什么小模型够用**：要交给本地的不是「回答」，而是**结构化决策** —— 分类 / 路由 / 打分 / 抽取。这类任务的输出空间小（枚举 / 布尔 / 短清单），配 llama.cpp 的 **GBNF grammar** 约束后输出**确定可解析**；模型再小也能被语法约束到合法答案上。收益：**离线可用 · 零 API 成本 · 低延迟 · 决策数据不出本机**。

**决策边界（只允许本地做的五类，其余一律不做）**：

| 决策类别 | 本地小模型做什么 | 为什么可以小 | 明确不做 |
|---|---|---|---|
| 能力路由 | 判定本轮「是否需要工具 / 走哪条路」 | 分类成枚举 | 不生成工具参数 |
| 安全预筛 | 危险度打分、是否需人工确认 | 二/三分类 | 不代替**最终裁决**（`CMD_BLACKLIST` 仍强制人工，[ai-spec.md](./ai-spec.md) §11 规则 14 不放松） |
| 记忆候选抽取 | 从回合里抽「值得记住的事实」 | 结构化抽取 | 不负责总结成文 |
| 检索查询改写 | 关键词 / 多查询扩展 | 模板化改写 | 不做网页摘要 |
| 计划续跑判据 | 判定计划「已完成 / 该继续 / 该求助」 | 清单比对 + 二分类 | 不生成计划本身 |

**与主链路的关系（硬约束）**：本地决策调用**不产生用户可见文本**、**不动主请求前缀**（前缀缓存纪律 [ai-spec.md](./ai-spec.md) §11 规则 18）⇒ 新增调用不废缓存。**本地决策不承担 `tool_use` 往返** —— 这正是 `proxy_server.rs` 当年的坑（丢 `tools` 数组），本地侧只做「单次结构化问答」，绝不复刻那条翻译链路。

**分发形态（用户 2026-10-09 约束）**：`local-engine`（llama.cpp，CPU 优先，几 MB）+ 默认小 GGUF（**0.5–0.6B 级、Q4_K_M 量化，约 300–450 MB**）作为 **NSIS 可选组件**随包分发（默认勾选，开箱即用）；运行时解包到 `<exe 根>\local\models\`，随包的模型**不带任何外部下载**。用户可自行替换更大模型（1.5B–3B，落同一目录）。**体量让步要说清**：安装包将因此增大约 400 MB，这是「开箱即用」与「小体量」两条约束的折中点，若要更小只能退回「首次运行下载」（与「开箱即用」冲突，故不选）。

**风险与前置**：① 模型许可必须允许随包再分发（优先 Apache-2.0 / MIT 系权重）；② 内存 / CPU 下限与**降级路径**（不满足则整个决策层**透明关闭**、全走云，功能不残缺）；③ 性能压测（决策延迟须显著低于一次云请求，否则旁路无意义）。

**分期**：**A1** 离线分类器（安全预筛 + 能力路由，只读旁路，失败即回落云判据）→ **A2** 记忆候选抽取 + 检索改写 → **A3** 计划续跑判据（与 §4.1.3「长任务托管」的检查点联动）。

#### 4.1.2 主题二：agent 整体思路与框架图

**一句话思路**：把 `agent.exe` 从「一个会调工具的对话循环」升级为**分层 agent 引擎** —— **决策层（本地）· 规划层（云）· 执行层 · 记忆层 · 策略层 · 调度层**，宿主与 IDE 都只是它的**薄客户端**。

**现状框架图**：

```
┌──────────────── Lunac 宿主 lunac.exe (Tauri / WebView2) ───────────────┐
│ 搜索 / 插件系统 · 对话面板 main.ts · 审批卡 / 计划卡 · 用量面板          │
└───────────────────────────────┬────────────────────────────────────────┘
                                │ spawn / send_message / control_*（stream-json）
┌───────────────────────────────▼────────────────────────────────────────┐
│ agent.exe (core-agent, Rust) —— 单模式 agent 引擎                       │
│  run_query：Anthropic /v1/messages + tool_use 往返                      │
│  上下文：回合预算 / 两级压缩 / 摘要 / 任务快照                            │
│  工具：17 内置 + Skill + mcp__* / resources / prompts / Remember         │
│  子代理：同批并发(<=3) + ListPeers / SendMessage（仅同批）               │
│  记忆：MEMORY.md + AGENTS.md + SessionSearch(FTS5)                      │
│  MCP：stdio 桥 / Streamable HTTP / OAuth / .mcp.json                    │
└──────┬───────────────────────────────┬─────────────────────────────────┘
       │ MCP stdio 桥                   │ HTTPS
┌──────▼──────────┐            ┌────────▼───────────┐
│ 工具执行宿主      │            │ 云模型端点          │
│ (lunac.exe mcp)  │            │ (Anthropic 兼容)    │
└─────────────────┘            └────────────────────┘
```

**目标框架图（S4 形态）**：

```
┌──────────────────────── 客户端（薄） ──────────────────────────────────┐
│ Lunac 桌面(搜索/插件/对话面板)      VS Code 扩展(同一引擎的客户端)       │
└───────────────────────────────┬────────────────────────────────────────┘
                                │ 统一 IPC：stream-json + 控制面（审批 / 计划 / 任务）
┌───────────────────────────────▼────────────────────────────────────────┐
│ agent engine (core-agent)                                              │
│  ├ 决策层 router / classifier ───本地──> local-engine(llama.cpp + 小 GGUF)│
│  ├ 规划层 planner（任务 DAG / 检查点 / 续跑） ───云──> 云模型             │
│  ├ 执行层 executor（工具往返 / 并行批 / 子 agent）                       │
│  ├ 记忆层 memory（工作 / 情景 / 语义 / 程序）                            │
│  ├ 策略层 policy（预算 / 续跑 / 安全 / 降级 / 速率）                     │
│  └ 调度层 scheduler（长任务队列 / 常驻 / 团队角色）                       │
└──────┬─────────────┬──────────────┬──────────────┬─────────────────────┘
       │ MCP         │ 云端点        │ 本地推理       │ 持久化
   工具宿主        Anthropic 兼容    llama.cpp      chat.db / memory / plans
```

**分层职责**：

| 层 | 职责 | 落点（预期） |
|---|---|---|
| 决策层 | 五类本地决策（§4.1.1） | `core-agent/src/router.rs` + `local-engine` |
| 规划层 | 计划生成 / 任务 DAG / 检查点 | `core-agent/src/plan.rs`（现 `EnterPlanMode` / `ExitPlanMode` 扩展） |
| 执行层 | 工具往返 / 并行批 / 子 agent | `core-agent/src/tools.rs` / `main.rs` |
| 记忆层 | 四级记忆的读写与淘汰 | `core-agent/src/memory.rs` + `ModuleData\memory\` |
| 策略层 | 预算 / 续跑 / 安全 / 降级 | `core-agent/src/main.rs`（`Cfg`）拆出 `policy.rs` |
| 调度层 | 长任务队列 / 常驻任务 / 团队角色 | `core-agent/src/scheduler.rs`（新） |

#### 4.1.3 主题三：skills / tools 完善与长任务托管

| 子项 | 现状 → 目标 | 依赖 |
|---|---|---|
| **MCP 完善** | 已支持 stdio + Streamable HTTP + OAuth + `.mcp.json` + prompts/resources/roots/elicitation → 补齐 **`sampling`**（服务端反向调用模型，可复用决策层做本地采样）、**工具结果流式**、**服务器健康与超时治理** | `core-agent/src/mcp.rs` / `mcp_oauth.rs` |
| **CLI 架构** | 现只有 `--disallowedTools` / `--mcp-server` → 提供**可复用 CLI 面**：`--print`（一次问答无头）、`--resume <session>`、`--output-format stream-json`、`agent mcp serve`，让 IDE / 脚本 / CI 都能把**同一个引擎**当库用 | `core-agent/src/main.rs`（参数解析） |
| **联网搜索** | 主源 + 兜底抓取 → 增加**引用溯源（每结论挂 URL）· 新鲜度过滤 · 结果缓存去重 · 多源交叉** | `core-agent/src/tools.rs` |
| **技能完善** | inline / fork + 脚本资源 → **版本号 · 依赖声明 · 角色化子 agent 技能（planner / executor / reviewer）** | `skills.rs` + `agent-templates/` |
| **长任务托管** | 无 → **任务队列 + 检查点落盘**：长任务拆成带状态的步骤，宿主侧可**暂停 / 恢复 / 后台续跑**（不占当前对话回合）；落 `ModuleData\tasks\` | 调度层（新） |
| **子 agent 团队** | 同批并发、跑完即散 → **常驻团队 / 角色分工 / 任务 DAG**：planner 拆解 → 多 executor 并行 → reviewer 验收；这与 §4 已有的「`TeamCreate` / `TeamDelete` 需先重开架构」**同源**，届时一并挪回正文 | 调度层 + 执行层 |
| **IDE 集成** | 扩展直连端点、零工具（平行链路） → 扩展作为 **agent engine 的薄客户端**：复用同一工具链 / 审批 / 记忆 / 技能；IDE 侧只做 UI 与编辑器上下文注入（当前文件 / 选区 / 诊断） | `vscode-extension/` + `cli_bridge.rs` |
| **计划任务不随预算中止** | **最小档已落地（2026-10-09）**：撞预算后按「`TodoWrite` 清单是否未完成」分流 —— 有计划且未完成 ⇒ 续命上限 **2 → 6**（`MAX_BUDGET_EXTENSIONS_HARD`），**检查点 = 压缩本身**（最近一条清单被 `Force` 档钉回历史，续跑不丢方向），**无需用户重问**；无清单维持 2（保守）。规范见 `ai-spec.md` §11 规则 90，守卫单测 `budget_gate_opens_only_when_the_plan_is_unfinished`。**仍未做**：任务级预算（区别于回合级）/ 跨回合后台续跑 / 调度层与队列 | 策略层 + 调度层（仍未做） |
| **上下文压缩** | 按体积的两级压缩 + 摘要 → **语义分层压缩**（工作记忆保细节 / 情景记忆压成结论 / 语义记忆入长期），与决策层联动判「哪段可压」 | `main.rs` 压缩路径 |
| **记忆** | 手动 `Remember` + 备份复盘 fork → **四层记忆**（工作 / 情景 / 语义 / 程序）+ **自动抽取**（本地决策层）+ **淘汰与冲突消解** | 记忆层（新） |
| **策略** | 预算 / 安全 / 降级分散在 `main.rs` → 收拢为**显式策略层**：成本策略 · 续跑策略 · 安全策略 · 降级策略 · 速率策略，可观测、可配置 | 策略层（新） |

#### 4.1.4 主题四：到达 AGI 的要求（解释）

**先厘清一件事**：Lunac 是 **agent harness（智能体外壳）**，不是模型本身 —— 它能**放大**一个模型的能力边界，但**不能凭空造出**更强的智能。所以「到达 AGI」= **模型侧突破 × 外壳侧成熟** 的乘积，本文的四个主题只覆盖**外壳侧**那一半。

按通行口径拆成八项要求，并标注 Lunac 的位置：

| 要求 | 含义 | Lunac 现状 / 本规划对应 |
|---|---|---|
| **通用性** | 跨领域泛化，无需专门训练 | 取决于模型；外壳侧靠**工具 / 技能 / MCP** 把领域能力外挂 → §4.1.3 MCP / 技能 |
| **自主性** | 长时程自主完成目标，不靠人逐步喂指令 | **最短板**：撞预算即停、无队列、无常驻 → §4.1.3 长任务托管 + 续跑 |
| **持续学习** | 从经验中积累并复用 | 只有手动 `Remember` + 备份复盘 → §4.1.3 四层记忆 + 自动抽取 |
| **世界模型与推理** | 对环境的内部模型与因果推理 | **纯靠模型**，外壳侧只做上下文供给 |
| **工具与行动** | 在真实环境里感知与行动 | 已较强（17 工具 + MCP + 子 agent），要强化的是**反馈闭环**与**可靠执行** → §4.1.3 |
| **元认知 / 自我改进** | 知道自己会不会、能改自己的策略 | 只有「卡住检测」一个点 → §4.1.1 决策层 + §4.1.3 策略层 |
| **安全与可控** | 行为可预测、可约束、可审计 | 已较扎实（审批 / 工作区锁 / 危险命令黑名单 / 静态分析）—— **这是底线，不得为任何一期放松**（[ai-spec.md](./ai-spec.md) §11 规则 14） |
| **效率** | 单位结果的时间 / 算力 / 成本 | 本规划的**本地决策旁路**正是为此（省 token 且降延迟）→ §4.1.1 |

**结论**：Lunac 在**安全 / 工具 / 上下文**三块已接近「成熟外壳」，在**自主性 / 持续学习 / 自我改进**三块仍是明显短板，而**通用性 / 世界模型 / 真正推理**取决于模型侧、非本仓可解。因此现实路径是：**把外壳侧六项推到极限（S0–S4），让最强模型在其上发挥最大效力**；AGI 的最后一公里仍在模型本身。

#### 4.1.5 分期总表（进入实施前先提升为 §1–§3 正文）

| 期 | 主题 | 目标 | 验收口径（草案） | 关键依赖 |
|---|---|---|---|---|
| **S0** | 单模型增强 | 搜索溯源 / CLI 无头 / 压缩语义化 / 记忆自动抽取（云侧先行） | 联网结论带可点 URL；`--print` 能跑通一次工具往返；压缩后命中率不降 | 现有 `tools.rs` / `main.rs` |
| **S1** | 本地决策旁路 | `local-engine` + 小 GGUF 随包；五类决策上线（A1–A3） | 断网下安全预筛 / 路由仍可用；决策延迟 < 单次云请求；决策不产生可见文本、不改主请求前缀 | llama.cpp 内嵌 + 模型许可 + 内存下限降级 |
| **S2** | 长任务托管 | 任务队列 + 检查点 + 跨回合续跑（预算不再终止计划） | 一个超预算计划能自动断点续跑完成，无需用户重问 | 调度层 + 策略层 |
| **S3** | 子 agent 团队 | 常驻角色（planner / executor / reviewer）+ 任务 DAG | 一个目标被拆解、并行执行、交叉验收并汇总 | 需重开架构（与 §4 现有 `TeamCreate` 条目同源） |
| **S4** | 持续自治 + IDE 附着 | IDE 成为同一引擎的客户端；后台常驻、按需唤醒 | VS Code 侧调用与桌面侧工具 / 审批 / 记忆**完全一致** | 调度层 + `cli_bridge.rs` + `vscode-extension/` |

> **与既有条目的关系**：§4.1.3 的「IDE 集成」「子 agent 团队」「长任务托管」分别承接现有 backlog 的 A13「IDE 集成」、§4 设想区的 `TeamCreate` 条目、以及 L10；**不是新增平行方向**，而是把它们的**前置条件**（调度层 / 策略层 / 检查点）显式写清。本地模型（§4.1.1 / §4.1.2）为**全新方向**。

#### 4.1.6 「决策保真度」实验设计（2026-10-09 用户选定；本地策展的**验收先行件**）

> **为什么先做它**：现有指标只有「成本 / 缓存命中率」，**测不出「地方性出错」**（丢约束、改错文件、编造路径/API）。没有这个指标，只能回答「是否更便宜」，回答不了「是否真的改善」。**三臂对照 + 可机器判定的约束清单**是唯一能证伪的办法。
>
> **已对齐的三项决策（2026-10-09）**：主方向 = **本地策展 + 大上下文**（append-only 存全量，压缩从「体积驱动」升级为「决策驱动」）；本地模型参与时机 = **摊销**（每 K 轮 / 压缩事件，不做每轮）；下一步 = **先出本实验设计**。

**A. 三条对照臂**（同一任务，各跑一次）

| 臂 | 上下文策略 | 落地现状 |
|---|---|---|
| **A 现状** | 体积驱动有损压缩（水位 0.85 / 0.95 + 滞回；见 `core-agent/src/main.rs` 的 `compact_history`） | 已实现 |
| **B 纯大上下文** | append-only、尽量不改前缀；抬高 `LUNAC_MAX_CONTEXT_TOKENS` 逼近模型窗口上限 | 只需改环境变量 |
| **C 本地策展** | append-only 全量 + 本地「决策账本」+ 决策驱动压缩（§4.1.1 / §4.1.3 的目标形态） | **未实现**（本方向自己的产物） |

**B. 任务套件**（6 个长任务；关键是让**约束故意早于**压缩点出现，否则测不出丢没丢）

| # | 任务形态 | 埋的判据点 |
|---|---|---|
| T1 | 跨 3+ 文件的多步修改 | 中途给出「不得修改 X 文件」 |
| T2 | 先读 5 个文件再回答 | 答案依赖**第 1 个**文件里的一条约束（被压掉即必错） |
| T3 | 长链工具调查 | 指定「必须用 A 库 / 禁用 B」 |
| T4 | 带未决问题的任务 | 结尾必须回到中途提出的开放问题 |
| T5 | 需回读早期工具结果的推理 | 结果超 `ELIDE_TOOL_RESULT_CHARS` 会被瘦身 |
| T6 | 目标漂移陷阱 | 中途插入诱导信息，测是否守住最初目标 |

每个任务交付两件：**约束清单（可机器判定）** + **期望产物（diff / 答案要点）**。

**C. 指标（三层）**

- **主指标 · 决策保真度**：① 约束遵守率 = 遵守数 / 总数；② 局部错误数 = 改错文件 + 违反显式禁令 + 编造不存在路径/API；③ 未决问题回收率；④ 目标锚定率。**这四项必须能机器判定**（正则 / 静态检查 / 文件存在性 / diff），判不了的约束宁可删掉。
- **副指标 · 成本/缓存**：每次提问成本（`ai-spec.md` §9.1 结论 3 口径，**不用「元 / 百万 token」**）、缓存命中率、压缩次数 / 丢弃条数 / 摘要失败次数、峰值上下文 token。
- **第三层 · 体验**：首 token 延迟、总耗时 —— **专门给「本地策展的 prefill 会不会拖慢」定参**（配套定「每 K 轮的 K」）。

**D. 执行口径（可重现）**

- 模型：主跑 `deepseek-flash`（项目规则：真实端点测试一律 flash）；涉及**质量判定**的关键任务加一轮 pro 复测（规则 16 的例外口）。
- 每臂每任务 **3 次**取中位数；固定 system prompt / 工具集 / 工作区 / 温度。
- 驱动照 `ai-spec.md` §11 规则 88 ④：`Start-Process` 单发 stdin + 收 stdout，**不要**交互式读循环。
- **判据预先写死**（防事后找理由）：C 相对 A **约束遵守率 +≥15 个百分点且成本 ≤ A×1.2** ⇒ 有效；C 相对 B **成本 ≤ B×0.7 且保真度不降** ⇒ 有优势；**两条都不满足 ⇒ 判「不值得做」，退回 A**。

**E. 分期（本地策展未实现，不能一步到位）**

1. **阶段 1（可立即做）**：只跑 **A vs B**，先**校准「决策保真度」指标本身** —— 确认它能区分两种策略（若 A、B 在该指标上无差异，说明指标不灵，先修指标再谈 C）。
2. **阶段 2**：实现本地策展 MVP（本地引擎 + 决策账本 + 压缩裁判，fail-open），跑 **A vs B vs C**。
3. **阶段 3**：把胜出策略的纪律固化进 `ai-spec.md` §11（压缩判据从体积驱动改决策驱动）。

**F. 风险 / 未定**

- 任务套件的约束若**无法机器判定** ⇒ 指标不可信（宁可少而精，不凑数）。
- 本地小模型判断力弱 ⇒ 策展必须 **fail-open**（拿不准就保留原文）；实验单列 **「误压率」**（被错误压掉且事后证明有用的比例）。
- **一处须先核实**：上一条分析里「Trae 每请求约 350K token」**超过 128K 窗口、数字站不住**（更可能是 Trae 用了更大窗口的服务，或平台计数口径不同）。要做 B 臂前先确认目标模型的实际窗口；本实验**不依赖**那个数字。

#### 4.1.7 本地「审查 / 策展」可行性（2026-10-09 调研结论）

**背景（用户 2026-10-09 定）**：agent 对话流里的本地模型**不是输入门**（「这条消息过不过 API」是**桌宠侧**的形态 —— 曾按此实现，用户否决并已回滚）。agent 侧要的是**重要节点上的审查 / 策展**，且结论**拦截并确认**：
1. **tools 的输出**是否跑偏 / 空转；
2. 对话里的**引用落点**（文件 / 行 / 函数 / 结论）是否与**磁盘真实代码**、以及**本次实际改动的文件**一致；
3. 回合 / 计划检查点上，**方向是否偏离对话最初的目的**；
4. （补充）**前端改动是否改在正确位置**、**测试是否覆盖所需功能 / 哪些区域根本没测**。

用户追问：**本地小模型判得准吗？会不会「本地幻觉 × 大模型幻觉」叠加？** ⇒ 结论：**先调研再做**。

**调研结论（可复用）**
1. **任务形态决定可行性，不是参数量**：小模型做**受约束的三态判定**（`supported` / `violated` / `unsure` + **必须引用原文 span**）**勉强可用（3B–7B、Q4_K_M 及以上）**；做**开放式评审 / 自我打分**不可靠 —— 无外部证据的自我纠错会**降低**表现（实测：强模型 98.1% → 56.9%，critic「为找错而编造错误」）。
2. **0.5B 只够做「label 提取」这类确定性极强的任务**。现役的 Qwen2.5-0.5B 只能当**通路验证**，**不能当评审器**；要评审得换 **3B / 7B**。
3. **确定性优先、模型兜底**：引用存在性（文件 / 行 / 函数）、「声称改动的文件」vs `git diff --name-only`、测试覆盖缺口（覆盖率工具给未覆盖清单）—— **先用工具判，别上模型**（消除真幻觉、不引新幻觉）。模型只做**语义层**：某条断言**是否被给定片段支持**（类 HalluGuard 的 4B grounded 检测）。
4. **叠加幻觉是真实的**：judge 自身会幻觉，且有**系统性方向偏差**（同 F1 的两个 judge，假阳 / 假阴倾向可差 ~29 个百分点）；误差相关时集成**不抵消**（「朴素平均失败、要按准确率加权」）。**缓解**：给真实证据（原文 / diff）→ 固定标签 → 强制引用 span → **不确定就 abstain** → 多信号加权 → **别让单个小模型拍板**；高风险判定**上交云 / 人工**。
5. **拦截门槛**：只对「**高置信 `violated`** 且 **不可逆 / 高危**」同步拦截，其余**只提示 / 记日志** —— 否则**确认疲劳**（40.9 万次实验：人类漏掉约 1/3 威胁、某类命令过半被盲批）。
6. **未微调的现成 3B/7B judge 大概率不够**（JudgeLRM / HalluGuard / V-Droid 都靠**微调**才可用）—— **要不要投入微调数据与流程**是个待定决策。

**分期（与 §4.1.6 的「本地策展 C 臂」同一条线）**
- **阶段 0（可立即做，收益最高）**：**零模型确定性核对** —— 引用存在性、声称改动 vs 实际 diff、测试未覆盖清单；**只提示不拦**。
- **阶段 1**：本地小模型**受约束判定**（工具输出粗相关 / 断言是否有 span 支持），GBNF 锁输出、**abstain 默认不拦**。
- **阶段 2**：高风险 / 语义一致性 / 方向漂移 → **上交云或人工**。
- **阶段 3**：门槛与呈现（证据包 + 阈值校准 + 反向抽查）。

**动手前的前置实验**
- 自建 **200–500 条带真值的节点**，报告**混淆矩阵 + 两个方向的偏差**（**别只看 F1** —— 它会掩盖方向性偏差）、用校准曲线定 abstain 阈值。
- 专门测「**本地小 verifier 与大模型是否同类犯错**」—— 这决定叠加风险的大小。

**落盘：独立「决策账本」，**不进 `chat.db`**（2026-10-09 用户问）**
本地模型的判定**要留痕**，但**不写进 `chat.db` 的会话消息** —— 那是「用户↔助手」的真相源，混进本地判定会污染回放 / 摘要 / 用量统计。应单独一张表（或数据根下的 `decisions-*.jsonl`），每条记：**节点类型 / 输入（或 hash）/ 本地判定（`supported`|`violated`|`unsure`）/ 是否 abstain / 置信度 / 引用 span / 用户是否采纳或推翻 / 时间戳**。它一份三用：**审计**、本节要的**带真值评测集**、以及**将来的微调数据源**（微调要的是「输入 + 判定 + 事后真值」，不是聊天记录）。

**2026-10-09 已换模型**：本地模型由 Qwen2.5-0.5B 换成 **Qwen3-4B-Instruct-2507-Q4_K_M**（Apache-2.0、约 2.5 GiB；插件 `local-model` **v0.11.0**，`sidecar.args` 带 `-c 8192`）。配套宿主改动：`MAX_DEP_BYTES` **512 MB → 4 GiB**、`file` 依赖改**流式落盘**（避免 2.5 GiB 整包进内存）。**并修掉一个真 bug（用户报「拉起 llama-server 几秒后闪退」）**：`http` 类 sidecar 原来**只认** NDJSON `ready` 握手行（10s），而 `llama-server` 不会说这行 ⇒ 被误判成失败、**宿主把进程杀掉**；现已改为「**ready 行 或 端口可连**」二选一 + 清单可配 `sidecar.readyTimeoutSec`（`local-model` = 180s）。见 `ai-spec.md` 规则 95。**注意**：这只换了「跑哪个模型」，**审查功能本身尚未实现**（阶段 0/1 都还没做）；且 4B 在 CPU 上单次判定会明显慢于 0.5B，**得先实测延迟**再定节点频率。

**关键来源**：[JudgeBench (arXiv:2410.12784)](https://arxiv.org/html/2410.12784v2) ｜ [JudgeLRM (arXiv:2504.00050)](https://arxiv.org/pdf/2504.00050v1) ｜ [HalluGuard (arXiv:2510.00880)](https://arxiv.org/pdf/2510.00880v1) ｜ [Weak Verifiers / Weaver (arXiv:2506.18203)](https://arxiv.org/html/2506.18203v1) ｜ [The self-critique paradox (Snorkel)](https://snorkel.ai/blog/the-self-critique-paradox-why-ai-verification-fails-where-its-needed-most/) ｜ [LLM-as-a-judge Bias (arXiv:2510.12462)](https://arxiv.org/html/2510.12462v3) ｜ [HITL Approval Misses 1 in 3](https://mer.vin/news/human-in-the-loop-approval-misses-ai-agent-threats/)

---

- **旧 CLI 的 `Config`（ant-only）/ `REPL`（ant 专用 VM）/ `Workflow` / `RemoteTrigger` / `Monitor` / `SubscribePR`·`SuggestBackgroundPR` / `TerminalCapture` / `Snip` / `Sleep` / `StructuredOutput` / `OverflowTest`·`CtxInspect` / `TestingPermission`** —— 要么绑定旧 CLI 的特定形态（Ink TUI / ant 内部构建），要么是调试与实验开关；在 `getAllBaseTools()` 里大多已被显式置为 `null` 停用，属旧 CLI 自己的历史包袱。
- **按分类的会话临时文件清理服务**（Hermes 的 `disk-cleanup` 插件思路）—— 本项目的临时产物集中在 `<exe 根>\temp\`，**卸载时由 NSIS 整目录删除**，`logs` 另有 7 天保留清理（`log::purge_old`，`KEEP_DAYS=7`）。便携式布局下「整个 temp 目录」就是清理单位，**不需要**按 test / temp / session / download 分类。详见 `ai-spec.md` §13.2。
- **`TeamCreate` / `TeamDelete`（命名团队的生命周期）** —— 随 A13「多代理协作」于 2026-10-05 重估后的结论：代理间通信只做成**同批兄弟**形态（`SendMessage` / `ListPeers`，见 §3.5「多代理通信」），因为本仓子代理是「同批并发、跑完即散」的，没有**常驻团队**这个东西 —— 没有常驻成员，`TeamCreate` / `TeamDelete` 就没有可创建 / 可解散的对象。真要做「常驻后台代理 / 团队模型」得先重开架构（属 L10/B 那一档），届时再把它挪回正文。

---

## 5. 边界声明（本清单覆盖什么、不覆盖什么）

**覆盖**：用户可以观察到的能力面 —— **工具名**、**子系统**、**协议消息类型**、**界面分期**、**自身新目标**。

- **工具面已核对完毕**：旧 CLI 的 `getAllBaseTools()` 里的每一个名字都已归位（实现 / 待办 / 设想）；Lunac 当前实际注册的是 **19 件内置工具**（`Read` / `Write` / `Edit` / `NotebookEdit` / `Cmd` / `PowerShell` / `Glob` / `Grep` / `WebSearch` / `WebFetch` / `AskUserQuestion` / `TodoWrite` / `PetControl` / `SessionSearch` / `Agent` / `EnterPlanMode` / `ExitPlanMode` / `ListPeers` / `SendMessage`）+ **条件注册**的四件：`Skill`（技能目录非空且未被黑名单裁掉时）+ `ListMcpResourcesTool` / `ReadMcpResourceTool`（**桥接上了用户工具时**，2026-09-20）+ `Remember`（**桥接通时**，2026-09-20，长期记忆写入侧）+ `mcp__*`（来自 `<exe 根>\tools\*.json`），可用 `--disallowedTools` 裁剪。
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
| `LUNAC_MEMORY` | `main.rs` | **长期记忆**注入开关（`0`/`false`/`off`/`no` 关，默认开）；`--disallowedTools Remember` 会连注入一起停（两者同源，见 ai-spec §11 规则 56） |
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
