# Lunac 代码规则 — 强制性参考

> **任何代码修改前，必须先对照本文逐项检查。忽略这些规则的修改有极高概率引入回归 bug。**

本文提炼自上轮 100+ 条 bug 修复的经验教训，覆盖 Tauri 2 + WebView2 透明窗口在 Windows 平台的所有已知陷阱。

---

## 第 1 章：预检清单（每次修改必查）

修改任何代码前，按顺序确认：

| # | 检查项 | 参考章节 |
|---|--------|---------|
| 1 | 新增 JS 调用 Tauri API 时，`capabilities/default.json` 是否已显式声明对应权限？ | §2 |
| 2 | 新增 `invoke()` 时，JS 参数名和类型是否与 Rust 函数签名完全匹配？ | §3 |
| 3 | 是否有 `await` 导致不必要的串行等待？隐藏/启动/读写操作应并发。 | §4 |
| 4 | 新 CSS 是否有 `overflow: hidden` 可能裁剪 WebView2 原生弹窗？ | §5 |
| 5 | 修改 DOM 后是否正确恢复 `hidden`/`pluginActive`/`isStreaming` 状态？ | §6 |
| 6 | 热键相关修改是否兼顾 LL 钩子 + WndProc 子类化 + JS 兜底三层？ | §7 |
| 7 | 窗口操作是否正确处理前台锁定（`ForegroundLockTimeout`）？ | §8 |
| 8 | 流式传输中途关闭组件时是否递增 `streamId` 防止回调污染？ | §9 |
| 9 | 新增 UI 字符串是否使用 `t("key")` 并在 `i18n.ts` 中添加了翻译？ | §13 |
| 10 | 新增的按钮 / 控件**是否写了 CSS**？（WebView2 没有「自动继承主题」的原生按钮 —— 漏写就退回原生外观） | ai-spec §11 规则 51 |
| 11 | 若改动了发往端点的 `history`（尤其**末尾消息的形态**），是否确认 `read` 没塌到 `system + tools` 的量级？追加内容**只能拼进已有 `tool_result` 的文本内部**，不得新增消息、也不得新增内容块。 | ai-spec §11 规则 23 |
| 12 | 新增 / 改名内置工具时，是否同步了**四处**：① `tools.rs` 的 `defs()`（含 `needs_approval` / `parallel_safe` / `gated_in_read_only` 三张表的判断，**走 MCP 桥的还要进 `BRIDGE_TOOLS`**，**条件注册的别塞进 `defs()`**）、② ai-spec §3.5 契约表与权限策略表、③ `agent-implementation.md` §4.1 与总数口径、④ 前端 `main.ts` 的工具黑名单候选名单？是否改完**读回确认**工具定义真的在文件里（编辑工具报成功但未落盘的情况出现过）并跑 `cargo test`（守门单测断言工具总数）？**另**：新工具若走**早退分支**（`Skill` / `SessionSearch` / `needs_bridge` 那一类，绕开 `tools::run()`），写类拦截（`tools::write_blocked()`）**必须自己在分支里补一次** —— 否则只读档 / 计划相位就能从这条缝里写本机。 | ai-spec §11 规则 14 / 54 / 55 / 56 / 59 |
| 13 | 若新增的是一份**每轮都要发**的提示词段落（记忆 / 索引 / 清单），是否满足：① 只在**启动时取一次**（冻结快照，进程内逐字节不变）；② 与「对应的工具是否真的在工具池里」**同源判断**（工具被禁 / 桥没接通 ⇒ 一并停注，否则提示词会指挥模型去调一个不存在的工具）？（2026-09-20 A4 的记忆块与 A2 的会话索引是同一条纪律） | ai-spec §11 规则 53 / 56 |
| 14 | 若新增一个**可被用户覆盖的 CSS 变量**（`--ctx-*` / `--btn-line-*` 这类），`styles.css` 的 `:root` 里那个默认值是否**逐项等于改造前的硬编码值**？默认态必须逐像素不变 —— 这是「用户没动过控件就不该看出区别」的唯一保证。**且覆盖要可撤销**：不需要覆盖时写成空串清掉内联值、回落 `:root`，不许逐个记旧值。改完用 `getComputedStyle` 对一次，别靠截图猜。 | ai-spec §11 规则 45 |
| 15 | 若改的是 `settings.ts` / `tool-editor.ts` 里**模板字符串内的 CSS / HTML**，注释里是否混进了**反引号**？（会提前终止模板字符串，`tsc` 报 `TS1005: ';' expected` —— 已**四次**踩到，最近一次是 2026-09-20 写 `.ap-slider` 那条 CSS 注释。**写完先扫一眼注释里有没有反引号**，写变量名直接裸写 `--btn-line-rgb`，别包起来）。另：自绘组件的 `colorPickerHtml(id)` 会占掉 `#<id>-panel` 等一串 id，**外层容器的 id 不要与它们撞名**。 | ai-spec §11 规则 45 |
| 16 | 若一个控件有**两条输入路径**（如「色板 ↔ 饱和度/明度滑块」这种同一组值的联动），是否保证：① 两条路径写**同一份状态**（不要各存一份）；② 回写对方时走**只重画不 commit** 的通道（否则 A→B→A 互相触发死循环）？ | ai-spec §11 规则 45 |
| 17 | 若引入了「某个开关一开就全部走主题」这类**失效语义**，是否把这套语义的**边界逐条写进规范**（哪些生效、哪些刻意不生效、为什么）？边界没写清 = 下次回归时会被当漏做而「修」成 bug。`tintBase`（界面文案「恢复默认主题」）当前的边界：**只有三组配色的「颜色」失效**（三个取色器 + 三对饱和/明度），`--ctx-*` 一并清空；**四个例外照常生效也照常可调** —— 底色透明度 / 按钮线条透明度 / 按钮背景透明度 / **文字明度**。**锁范围与失效范围是两张表**，改一处必须回头看另一处。 | ai-spec §11 规则 45 |
| 18 | 改了 `app/src-tauri/themes/**` 或任何 `tauri.conf.json` 里 `resources` 映射的文件时，是否知道 dev 实例读的是 **`target\debug\` 下的构建产物拷贝**（`themes_root()` = exe 根）？只改源文件而不重新构建，dev 里看不到变化。 | ai-spec §11 规则 45 |
| 19 | 若某个工具的**副作用取决于入参**（`Skill`：inline 是纯读、`context: fork` 却会派子代理），是否把「**只看名字**的判据」按**最坏模式**算（`parallel_safe`）、把「带输入的判据」单独写一份（`needs_approval_with`）？**别**为省事把整件工具塞进 `tools::needs_approval()` —— 那会让「多数情况下无害」的那种用法每次白弹卡。另：凡**要发自己的 API 请求**的工具（`Agent` / fork 技能 / 后台复盘），都必须**显式串行** + 在主循环的工具执行分支里**特判**（`dispatch_tool()` 拿不到 `cfg`），并且**不许进 `parallel_safe` 白名单**。 | ai-spec §11 规则 57 |
| 20 | 若在**审批卡**上新增一档「必须人看」的提示（如写入内容的凭据扫描），是否做到三件事：① 走**独立通道**，没有塞进 `analysis.dangerous`（那条的文案是「危险命令」，混用会让用户看不懂到底在问什么）；② 该档**任何运行方式档位都不自动放行**（含「自动」）、**不给「始终允许」**（写类工具的「始终允许」= 以后所有 `Write` 都免问）；③ 命中项**可见地**列在卡片正文，不能只塞标题 tooltip（藏在 hover 里等于没做）。另：任何**静态扫描的规则集都宁漏勿误报** —— 正则在正常代码里做不了语义判定，误报攒够就会让用户学会无视告警，那比没有更糟。 | ai-spec §11 规则 58 |
| 21 | 动**「写类能不能做」**这条判据时，是否只改 `tools::write_blocked()` **一处**？两条闸门（**用户**的只读档位 / **模型**的计划相位）的**拒因措辞必须分开** —— 用户要做的动作不同（去设置改档位 vs 去批准计划），说错一句模型就会去调一个在当前档位下永远失败的动作。另：计划相位是**进程内**状态（`Ctx.plan_phase: Arc<AtomicBool>`），**不许**为解锁写类去重启 agent、也**不许**让模型改用户的安全档位（那是用户的边界）；而**绕过 `tools::run()` 的早退分支**（`Agent` / fork 技能 / `Remember` / MCP 与走桥工具）**各自都要补一次**这条判据。 | ai-spec §11 规则 59 |
| 22 | 若往**请求体**里加新的**内容块类型**或新的 stdin 字段（如图片附件 A8 的 `{"type":"image","source":{"type":"file",…}}`），是否做到：① 在 ai-spec §3.5 的 stdin / stdout 契约表**登记字段名与语义**，并在 agent-ui-spec §9 登记前端要读的那个回报字段；② **保持既有文本不变**（旧消费者仍可读 —— 附件仍要给 `[Attached files]` 文本，不许因为有块了就删掉）；③ 接收方**按内容判定**而不是按发送方声明（图片按**魔术字节**判类型、不信扩展名），并且**失败可见**（不能静默丢：走 `system/attachment_note` 这类如实上报 + 前端可见地列出来）；④ 该不该带**大字节**过 IPC 想清楚：**只传路径、由消费端读盘转 base64**，别把几 MB 塞进管道与 WebView 内存。另：**用户显式选中的附件，读盘不走工作区锁**（它不是模型找出来的东西；剪贴板图片就落在 `%TEMP%`，套锁会让主用法失效）——但模型的 `Read` 照旧受锁约束，两者别混。 | ai-spec §3.5「图片附件」 / §11 规则 60 |
| 23 | 若给 agent 加**用户脚本介入点**（权限 hooks A9 这类「外部进程能决定放不放行」的机制），是否守住四条：① **事件只挂「所有工具调用都会经过的那一层」**（主循环与子代理循环的「执行工具」段），**不许下沉进 `tools::run()`** —— 会漏掉 `Skill` / `SessionSearch` / `needs_bridge` / `Agent` / fork 技能那几条早退分支；② **只认显式拒绝**（退出码 2 / `{"decision":"deny"}`）才拦，超时 / 崩溃 / 坏 JSON 一律**放行但可见**（`kind=error` 落日志 + 前端可见）—— **反向的 fail-closed 也不行**，脚本一崩全线卡死比没装更糟；③ hook 的 `allow` **只等于用户白名单**（跳过审批卡），**不越过两道硬闸**：静态安全分析命中（危险命令 / 写入内容里的凭据）仍强制弹卡、工作区锁与计划相位照旧，判据 `hook_allow_needs_card()`；④ **配置唯一真相 + mtime 热重载**（宿主**无条件**注入 `LUNAC_HOOKS_FILE`，连文件不存在时也给；解析失败保留上一份有效配置）。另：**Windows 下只要被解释的是 `cmd /C`，拼命令行就必须用 `raw_arg`** —— `Command::arg` 会按 MSVC 规则把命令里的 `"` 转义成 `\"`，含空格的命令整条失败（2026-09-20 已全仓清理：`hooks.rs` / `tools.rs` 的 `Bash` / `mcp_server.rs` 的用户工具通道 / `kill_port` 四处；**`PowerShell` 实测不受影响、刻意不改**）。且 `for /f ('…')` 里的重定向要写 `2^>nul`、`do` 子句末尾的 `2>nul` **不要**转义 —— 判据是「这段文本由外层 cmd 读还是子 cmd 读」。 | ai-spec §3.5「权限 hooks」 / §11 规则 61 |
| 24 | 若在**「审批频率」**上加/改判据（A10 的只读分类、A9 的 hook `allow` 都是这一类），是否做到：① **只减询问、不加闸门** —— 新判据的反面（`false` / 判不出来）**不表示危险**，不许据此拒绝执行，也不许拿它去收紧「自动」档（三档语义见 agent-ui-spec §4.2）；② **正向白名单，不是黑名单** —— 只加「证实过安全」的条目，不写「排除已知危险写法」，那正是旧前缀表翻车的方式（前缀是字符串匹配，看不见 `>` 与 `\|` ⇒ `echo hi > f` 被当「安全前缀」零询问地写文件）；③ 判据放**执行侧**（agent 拿得到原命令并能按子命令结构化拆分），因为子代理 / fork 技能 / 后台复盘那几条路径**拿不到前端的正则表**；④ 与既有安全结论**自洽**（`dangerous` / `opaque` 非空时不许再给任何「安全」结论）；⑤ **缺字段 / 判不出时按保守侧处理**（只认显式的 `true`），且**绝不回落到已被取代的旧表**。 | ai-spec §3.5「只读分类」 / §11 规则 62 |
| 25 | 若要做一块**统计 / 金额类面板**（A12 成本面板这类「拿外部会变的数据算给你看」的东西），是否做到：① **会变的外部数据不写进代码** —— 单价、汇率这类东西一律落用户可编辑的配置文件，且**不预置样例数字**（凭空填一行「看着很像」的数会被拿去对账，比留空糟得多），每项带**出处**（`source_url` / `updated_at`）；② **算钱必须按能拿到单价的最小粒度分组**（这里是**先按天、再按模型** —— 只按天合计就会把两种模型的 token 混着乘一个单价）；③ **缺数据就不算**：没单价的模型只标「未定价」并单列提示、总额前缀 `≥`，**禁止**当 0 计、也禁止拿别的模型的单价顶替；④ **写入必须「先校验后覆盖」**，校验不过**一个字都不写**（缺字段也算非法：金额会悄悄少算一块），并给用户一个**看得见的新旧对照**再让他确认；⑤ 若这份数据要由 **agent 代抓**，落点必须是**它在工作区锁下仍写得进去的目录**（A12 用 agent 的工作目录；写 `config\` 会被 `tools::guard()` 直接拒 —— 实测见 ai-spec §3.5），且**确认动作只由用户点按钮触发**，不给「自动确认」留开关。 | ai-spec §3.5「定价表与成本面板」 / §11 规则 63 |
| 26 | 写 **GitHub 上会被人读到的页面**（`README.md` / `agent-templates/**/README.md` / `docs/**` 正文）时，是否**一个装饰性 emoji 都没插**？（清单 / 表格 / 标题一律纯文字 —— 插件名就写「快速启动 Quick Launch」，不带火箭 / 齿轮这类图标前缀）。**要指认某个字符时用文字描述**（「思考那个对话气泡字符」「剪贴板图标」），不把字符本身写进页面 —— 本条与 `ai-spec` §8.3 那条自身也按此写。**这条与「控件文案不带 emoji」（§11 规则 49 / 预检 #10 同族）是一条纪律的两种范围**：界面不许有、页面也不许有。 | ai-spec §8.3「页面一律不插 emoji」 / §11 规则 49 |
| 27 | 若把**下标**烘进 DOM（聊天气泡的 `data-idx`、`.result-item` 的 `data-idx` 这类「渲染时算一次、点击时再读」的定位符），是否确认**没有任何路径会改动它所指向的那个数组**？本仓已有反例：`pruneContext()` 每回合从**队首**丢消息，而气泡上的下标从不重算 ⇒ 丢 N 条后所有老气泡都偏大 N，回退**切错消息**、复制**复制错**（A11 才修掉，落地形态 `shiftRenderedMsgIdx`）。两条出口照着做：① **数据被裁剪的那一刻，同步前移已渲染元素的下标**；② **目标已不在数据里的元素，把「会切错东西」的按钮摘掉**，只留能安全退化的那条路（复制退回按气泡文本复制）。另：写「回退 / 撤销」这类能力时**只承诺做得到的** —— 只回退对话就别在任何文案里暗示会还原文件（写进按钮 title 与执行后的状态行）；**写进 localStorage 却全仓无读取方的「备份」是死代码**，白占配额还让注释撒谎，发现即删。 | ai-spec §11 规则 64 |
| 28 | 若要让**同一个进程里的多个任务并发**（A14 的子代理批这类「每个任务自持状态、自己发请求」的活），是否守住五条：① **共享状态能不能跨线程先查清楚** —— 含 `Cell` / `RefCell` 的类型是 `!Sync`，`&T` 过不了线程边界，必须能**每线程一份克隆**（本仓形态：`cfg.detached()`；且它复制的是**当前已跑通并缓存**的形态，不是回到默认值）；② **独占借用的资源根本不要传给并发单元**（`&mut Bridge` 这类，宁可传 `None`），并顺手把它相关的工具从并发单元的工具集里剔掉（否则是「保证失败」，还可能先弹一张注定白问的审批卡）；③ **判定只做一次**，规划与执行共用同一份结论（`subagent_call()` + `subagent_kinds`），**禁止**在批循环里再写第二处 `if 工具名 == …` —— 那正是「规划说串行、执行走并行」的漂移源；④ **结果按下标回填**，并发的完成先后**不许**影响回灌 / 渲染顺序（worker panic 也要按下标写一条错误结果，不留空洞）；⑤ **交互通道要自证不会死锁** —— 并发单元等用户应答时，先问「此刻谁在收应答」（本仓：`route_control_response` 在**独立 stdin 线程**上按 `request_id` 投递，所以主线程阻塞在 `thread::scope` 里也照样收发；改成主线程轮询就会直接锁死）。另：**跨任务的聚合必须带来源标识**（前端按 `task_id` 归组；命令合并这类「并成一行」的操作**不许跨任务**，否则用户分不清自己批准的是谁的命令），**缺标识就是「不属于任何任务」而不是「随便归一个」**。**最后，「真的并发了」要拿证据说**，不要只说「上限设成了 N」—— 本仓 e2e 的判据是「第 2 个任务的 started 早于第 1 个的 done」+「两条请求同刻在飞」。 | ai-spec §11 规则 65 |
| 29 | 若在为一个问题**加埋点 / 定判据**（「跨提问的缓存接力为什么时好时坏」这类只能靠日志离线归因的事），是否先自问「**这条埋点能回答它自己提出的那个问题吗**」？本仓反例（A16）：判据写的是 `公共前缀=N/M条`，而那串逐条指纹当时是 `run_query` 的**局部量** ⇒ **每一问的首请求都只会打 `0/0条`**，可 A16 要判的恰恰是「两次提问之间有没有改写 history」—— **跨边界（提问 / 会话 / 进程）的判据，状态就必须跨边界存活**（由边界之外的持有者按 `&mut` 传进去，并把判定收口成一个**可单测的纯函数**，如 `common_prefix_len()`）。配套三条：① **日志行带上「第几次提问 / 本问第几步」**（`问#N` + `#n`），离线看日志不必靠时间戳猜边界；② **能拿线上报文当证据就别只信本侧变量** —— 假端点抓请求体、把 `messages` 数组**元素级**逐条比对（重序列化后按字符串比）比任何本侧断言都硬，而且零成本、可重复；③ 「A 的锅还是 B 的锅」这类二分，**先把本侧钉死再谈对方**（本侧纯追加 + 固定前缀哈希不变 ⇒ 责任在端点侧，别对着本侧猜），并**给「正常但看起来像故障」的形状起个名字**（A16 的 `history=1条` = 新进程签名，此时 `read` 落到 `system + tools` 属正常）。 | ai-spec §11 规则 23（埋点条目） |
| 30 | 若某个动作的**收益与代价都依赖「位置」**（改了哪一条、它后面还剩多少 —— 就地瘦身 / 重写历史 / 插一条合成消息 / 重排工具结果都属于这类），判据是否把**被改处之后要全价重发的部分**算进去了？本仓反例（A15）：旧闸门只比「可省体积 ÷ 上下文体积」（`ELIDE_MIN_SAVINGS_RATIO`，已删），看不见代价的位置，而就地瘦身**省的是 1/50 价的命中、废的是全价 miss** ⇒ 实测出现「省 176101 字 / 让该问 `read` 从 108160 塌到 2176」。正确形态：把收益与代价都写成**同一个后缀口径**的两个量（`σ` = 省下的字符、`Δ` = `history[k..]` 里没被改的剩余字符），再比 `σ × 收益次数 > Δ × 折扣`（两侧同量级时收敛成一句白话「**省下的要多于废掉的**」）；**枚举的自由度可能只有一个下标**（给定 k，把 k 之后全部改掉永远不差），别写成贪心逐条判定。配套两条：① **可选档走判据、安全刚需档不走** —— 本仓 `Drop`（0.95 水位）与 `Force`（400 兜底）是**防 400 的硬需求**，必须无条件压，判据只作用于 `Elide`（实现时踩过：初版套在「非 Force」上会让 `Drop` 也被拦下）；② **给机器读的埋点键值用 ASCII**（`sigma=… delta=… net=…`，中文散文可以并存）—— 本仓 e2e 是 PS 5.1 脚本，按 ANSI 读取，**脚本里写不了中文模式串**；且「真的没改」要拿**线上报文**当证据（原始内容字符串是否还在请求体里），不能只凭日志说自己跳过了。 | ai-spec §11 规则 23（位置成本模型） |
| 31 | 若你让**「能改变模型行为的文本」变成用户可配置**（人格 / 自定义提示词这类「用户想改 agent 怎么说、怎么做」的需求），是否守住四条：① **落点仍是固定前缀** —— 启动时读一次、进程内逐字节不变；「可配置」不等于「每轮可变」，也别因为它现在来自文件就想给它加个热重载：**需要热更新的是配置，需要固定的是提示词**，两者走两条不同的通道（本仓 hooks 是 mtime 热重载、人格是「保存后重启 agent 才生效」，面板文案必须如实写明是哪一种）；② **「没配」必须逐字节等于「没这个功能」**（空串：不加空行、不加表头、不放占位文案），判据拿**线上报文的 `system` 前缀哈希**与引入本功能前的基线比，别只看代码里那句 `if`；③ **注入面明确收窄** —— 本仓刻意**只进主提示词**，不进子代理与后台复盘（它们是内部产物，且每个并发子代理都要重发一次）；④ **有上限且两侧同值**（本仓 8000 字符：宿主保存前硬校验「拒收且一个字都不写」、消费侧读到超长截断 + warn，兜住绕过面板直接改文件的人）。**注意别把 hooks 当文本注入通道**：hooks 是控制通道（放行 / 拒绝 / 提示行），它的输出今天不进模型（只有 `PostToolUse` 能拼进**已有** `tool_result` 的文本内部），用它承载人格还会多出一条权限面「谁能写 `hooks.json` 谁就能改系统指令」。 | ai-spec §3.5「人格 / 自定义提示词」 / §11 规则 66 |

| 32 | 若你要**从网上下载并执行代码**（第三方插件 / 扩展 / 主题包这类「用户自己选的可执行包」），是否守住五条：① **传输加密**（只收 `https` —— 明文 http 的包会被解压执行）；② **三道体积闸**：压缩包大小、**解压后总量**、条目数（只拦压缩包拦不住 zip bomb）；③ **解压路径逐条校验**并把判据收成**可单测的纯函数**（拒绝绝对路径 / `..` / 空段 / 过深 / 以点或空格结尾的分段 —— 最后一条是因为 Windows 会悄悄改掉这种名字，让「校验过的路径」与「写下去的文件」不是同一个）；④ **已存在即拒绝**（不静默覆盖：无声换掉一个插件目录里的代码是最不该发生的事）+ 先解到 `.staging-*` 再改名、**失败即清理**；⑤ **「装了它就等于在本机跑它的代码」必须写在界面上**（来源 + 可执行代码两条），**禁止**把这条描述成「已沙箱化」或「只是个配置」——这是**防事故**不是防恶意。另：**坏包不能静默消失**（清单坏 / 入口丢的包要列出来并写出原因），且「装完多久生效」要如实写（本仓插件 = 立即生效；技能 = 重启 agent，两者别混）。**⑥ 若「能下什么」还依赖一份远端清单**（本仓插件市场索引 `plugins/index.json`）：那份清单是**数据不是指令** —— 「已装 / 未装 / 坏了」的判据一律取**本机事实**（registry + 目录扫描），不许拿清单自称的字段去决定；清单里每条 URL 仍要按 ① 的口径**重新筛一遍**（https + 安全 id + 有名字，收口在纯函数并有单测）；且**由宿主去拉**（前端 CSP 的 `default-src` 不含远端域，前端 `fetch()` 会被拦掉 —— 顺带也是「不把这份清单变成前端可绕过的旁路」）。坏条目**只丢自己**（warn 留痕），不丢整份清单。 | ai-spec §3.5「插件市场」 / §11 规则 67 |

| 33 | 要让前端**打开一个东西**时，是否先分清它是 **URL** 还是**本地路径**？**URL 走 `@tauri-apps/plugin-shell` 的 `open()`**（`mailto:` / `https://`…）；**本地路径一律走宿主命令 `open_path`**（本仓前端封装成 `openLocalPath()`，见 `settings.ts` 顶部注释）。**不能**把本地路径塞给 `open()` —— shell 插件会拿 `capabilities/default.json` 的 `shell:allow-open` scope 正则去校验入参，路径过不了就抛 `scoped command argument at position 0 was found but failed regex validation`；而调用点通常挂着 `.catch(() => {})`（那是给「用户点了取消」准备的静默分支）⇒ **用户看到的就是「按钮点了没反应」**（2026-09-21 的主题目录 / hooks.json / pricing.json 三处就是同一个根因）。**也禁止**为了让它能开而把本地路径写进 open scope —— 那等于给所有前端代码（含第三方插件）开了任意本地路径的口子。`open_path` 是自定义 `#[tauri::command]`，**不需要**改 `capabilities/default.json`。回归口径：点按钮时 DevTools **不得**出现 `failed regex validation`，且资源管理器真的弹出。 | agent-ui-spec §3.10 |

---

## 第 2 章：Tauri 2 权限系统

### 2.1 铁律

> **`core:default` 不含任何窗口修改类权限。前端调 `hide/show/setFocus/setSize/startDragging` 若缺权限，会被静默拒绝（Promise reject 被 `.catch(() => {})` 吞掉，零错误提示）。**

### 2.2 权限清单（`capabilities/default.json`）

```json
{
  "permissions": [
    "core:default",
    "core:window:allow-hide",          // win.hide()
    "core:window:allow-show",          // win.show()
    "core:window:allow-set-focus",     // win.setFocus()
    "core:window:allow-is-visible",    // win.isVisible()
    "core:window:allow-start-dragging",// win.startDragging()
    "core:window:allow-set-size",      // win.setSize() ← 容易漏
    "shell:allow-open",                // open() URL
    "shell:allow-execute",
    "dialog:default",
    "clipboard-manager:allow-read-text",
    "clipboard-manager:allow-write-text",
    "clipboard-manager:allow-clear"
  ]
}
```

### 2.3 新增前端 API 检查流程

1. 确认该 API 所属的 Tauri plugin 名称（如 `window`、`shell`、`clipboard-manager`）
2. 在 `capabilities/default.json` 添加 `"<plugin>:allow-<method>"` 权限
3. **不要**依赖 `"<plugin>:default"` — 它经常不含完整权限集

---

## 第 3 章：IPC 调用规范（invoke / emit / listen）

### 3.1 JS `invoke()` 参数必须与 Rust 签名完全匹配

```typescript
// ❌ 错误：包裹在对象中
await invoke("set_ai_config", { config: { provider, url, key, model } });

// ✅ 正确：平面参数对齐 Rust fn set_ai_config(provider, url, key, model, anthropic_url)
await invoke("set_ai_config", { provider, url, key, model, anthropicUrl });
```

- Rust 函数签名中每个参数在 JS `invoke` 的第二个参数中**必须是顶层的 key**
- 检查方式：搜索 Rust 端 `#[tauri::command] fn xxx(...)` 确认参数列表

### 3.2 Rust 返回值类型必须与 JS 接收类型匹配

```typescript
// ❌ 错误：Rust 返回 String，JS 解构对象
const { vk, modifiers } = await invoke("get_hotkey_combo");

// ✅ 正确：Rust 返回 String
const combo: string = await invoke<string>("get_hotkey_combo");
```

### 3.3 事件监听器生命周期

```typescript
// ❌ 每次打开面板注册新监听器，从不取消 → 内存泄漏 + 重复回调
function openPanel() {
  listen("some-event", handler);
}

// ✅ 模块级 unlisten 变量，attach 时先清理旧的
let _unlisten: (() => void) | null = null;
async function openPanel() {
  if (_unlisten) { _unlisten(); _unlisten = null; }
  _unlisten = await listen("some-event", handler);
}
```

### 3.4 异步初始化时确保监听器已就绪

```typescript
// ❌ 动态 import 异步加载 → listener 可能未绑定
const { listen } = await import("@tauri-apps/api/event");

// ✅ 静态 import 确保就绪
import { listen } from "@tauri-apps/api/event";
```

### 3.5 状态同步顺序：先 Rust 后 JS

```typescript
// ❌ 先设 JS 标志再 invoke → async invoke 未完成时有竞态窗口
window.__my_flag = true;
await invoke("set_state", { state: true });

// ✅ 先 await invoke 同步 Rust 原子变量，再设 JS 标志
await invoke("set_state", { state: true });
window.__my_flag = true;
```

---

## 第 4 章：异步/时序规则

### 4.1 隐藏和操作必须并发，禁止串行 await

```typescript
// ❌ 串行：hide 完成 → launch_app IPC 往返 → 窗口才消失（用户感知延迟）
await win.hide();
await invoke("launch_app", { path });

// ✅ 并发：两个操作同时触发，窗口瞬间消失
win.hide();
invoke("launch_app", { path }).catch(console.warn);
```

### 4.2 requestAnimationFrame 不是可靠的布局完成信号

WebView2 中 `requestAnimationFrame` 回调触发时 DOM 可能尚未完成布局（尤其透明窗口 + 小视口场景）。不应依赖它在回调中测量 `scrollHeight`/`offsetHeight` 来确定窗口尺寸。

### 4.3 setTimeout 优于 requestAnimationFrame 做延迟检测

```typescript
// ✅ 给浏览器额外时间完成渲染
setTimeout(() => { /* 测量或操作 */ }, 50);
```

### 4.4 CSS transitionend 事件可能永不触发

CSS transition 缺失或被中断时 `transitionend` 不触发。**必须加 setTimeout fallback**：

```typescript
element.addEventListener("transitionend", handler, { once: true });
setTimeout(() => { /* fallback logic */ }, 500);
```

### 4.5 mousedown 优于 focusin 用于预判 popup 可见性

`focusin` 事件在浏览器**已完成** native popup 可见性判定后才触发。需要在点击**瞬间**（popup 渲染前）修改 CSS 时，用 `mousedown` + `{ capture: true }`。

---

## 第 5 章：CSS / WebView2 渲染规则

### 5.1 `overflow: hidden` 会裁剪 WebView2 原生弹窗

这是 WebView2 透明窗口的**硬限制**。任何父元素的 `overflow: hidden` 都会裁剪 `<select>` 弹出层、自定义下拉框等。

**解决方案优先级**：
1. 改用自定义下拉组件（`.custom-select`，用 `position: absolute; z-index: 999` 脱离 overflow 上下文）
2. 或：在打开弹窗时动态添加 class 解除 overflow
3. **不要**依赖 inline style（在 WebView2 中不稳定）

```css
/* 动态解除 overflow 的 pattern */
.plugin-open {
  overflow: visible !important;
  max-height: none !important;
}
```

> **⚠️ 只对本条描述的「纯裁剪容器」生效（2026-09-19 补 —— 缺这一句就是事故）**
>
> `.plugin-open` 把 `overflow` 改成 `visible`，对**做裁剪用**的容器是对的；但加到一个**靠自身滚动**的列表上（典型：`#results-list`、`.tool-result`、`#chat-log` 的子滚动盒），会直接把它的滚动盒拆掉 —— 表现是 `scrollTop` 归零、整页跳回顶部、再也滚不动。判断口径只有一条：**这个元素是不是滚动条真正出现的那一层？** 是 → 不能用这个 pattern，改用「把弹窗挂到 `body` 下」或「换自定义下拉组件」。
>
> 依据：`docs/agent-ui-spec.md` §5.4 第 3 条「不要『藏』滚动条」——`overflow: visible` 与 `overflow: hidden` 在「把滚动盒拆掉」这件事上是同一个后果。

### 5.2 原生 `<select>` 在 WebView2 透明窗口中不可用

`tauri.conf.json` 中 `"transparent": true` 时，WebView2 不会渲染原生 `<select>` 弹出层。**必须用自定义下拉组件替代。**

### 5.3 `pointer-events: none` 的区域无法点击

```css
#app { pointer-events: none; }  /* 整个窗口穿透 */
#search-bar { pointer-events: auto; }  /* 逐一恢复 */
#results-container { pointer-events: auto; }
#status-bar { pointer-events: auto; }
```

- 所有交互子元素必须显式 `pointer-events: auto`
- 窗口边缘 padding 会制造不可交互死区 — 用子元素 margin 替代

### 5.4 自定义滚动条：**统一细滚动条，不得按容器声明、不得隐藏**

> **本节 2026-09-19 重写。** 旧版本教的是下面这段，**它是被明令禁止的写法，照抄会直接制造事故**：
>
> ```css
> /* ❌ 禁止。历史事故就是这一段被复制到新容器上 */
> .custom-select-dropdown { scrollbar-width: none; }
> .custom-select-dropdown::-webkit-scrollbar { display: none; }
> ```
>
> 两个错误：① `scrollbar-width: none` 一出现，Chromium 就**忽略全部 `::-webkit-scrollbar`**（滚动条退回系统默认外观）；② 「藏滚动条」与项目要求相反。

**唯一真相源是 [agent-ui-spec.md](./agent-ui-spec.md) §5.4**（三条硬约束：禁止写 `scrollbar-width` / `scrollbar-color`；禁止按容器单独声明；不要「藏」滚动条）。实现只有一处 —— `styles.css` 的全局 `::-webkit-scrollbar` 规则；新容器**只要写 `overflow-y: auto` 就自动获得统一外观，不需要任何额外 CSS**。

对应 [ai-spec.md](./ai-spec.md) §11 规则 21 的「不得回退」项。

### 5.5 隐藏元素用 class 控制，避免 display 冲突

```css
/* ❌ 两个规则冲突 */
.hidden { display: none !important; }
#chat-drawer.visible { display: block; }

/* ✅ 用 pointer-events + transitionend 延迟 hidden */
#chat-drawer { pointer-events: none; opacity: 0; transition: opacity 0.3s; }
#chat-drawer.visible { pointer-events: auto; opacity: 1; }
/* transitionend 后再加 .hidden */
```

### 5.6 HTML 元素 class 操作优于 inline style

```typescript
// ❌ inline style 可能被 CSS 覆盖
resultsContainer.style.overflow = "visible";

// ✅ 用 class + !important
resultsContainer.classList.add("plugin-open");
```

---

## 第 6 章：DOM / 状态管理规则

### 6.1 `innerHTML` 序列化不保留 DOM property

```html
<!-- 用户点击 checkbox → DOM property checked=true -->
<!-- 但 innerHTML 序列化时 checked 属性不出现 -->
<input type="checkbox">  <!-- 永远是未选中 -->
```

**规则**：任何依赖用户交互状态（checkbox/select/textarea）的插件，**不能用缓存 HTML 恢复**。必须跳过缓存、重新执行 `plugin.execute()`。

### 6.2 批量 DOM 操作用 DocumentFragment

```typescript
// ❌ 逐条插入 → 多次重绘
results.forEach(item => resultsList.appendChild(item));

// ✅ 批量替换 → 一次重绘
const frag = document.createDocumentFragment();
results.forEach(item => frag.appendChild(item));
resultsList.replaceChildren(frag);
```

### 6.3 流式传输中途关闭 → 递增 streamId 防回调污染

```typescript
let streamId = 0;

function startStream() {
  const myStreamId = ++streamId;
  // 回调中检查：
  streamCallback = (data) => {
    if (myStreamId !== streamId) return; // 已关闭，忽略
    // ... 处理数据
  };
}
```

### 6.4 关闭组件时必须完整清理状态

```typescript
function closePluginView() {
  // 必须清理的全部状态：
  if (isStreaming) {
    streamId++;                    // 防回调污染
    isStreaming = false;
    setStreamingUI(false);         // 恢复发送/停止按钮
    streamCallback = null;
    doneCallback = null;
    cliTextCallback = null;
    cliDoneCallback = null;
  }
  agentView = null;               // 清 agent 渲染状态
  currentSessionId = null;        // 下次保存用新 ID
  consecutiveFailures = 0;        // 重置错误计数
  pendingMessages = [];           // 清消息队列
  humanizeBtn?.classList.add("hidden");
}
```

### 6.5 渲染函数必须守卫 `pluginActive`

```typescript
function renderMixedResults(...) {
  if (pluginActive) return; // 插件面板打开时不覆盖其 HTML
  // ...
}
```

---

## 第 7 章：热键系统规则

### 7.1 双后端 + 三层兜底架构（2026-09 修订）

**后端由 `install_hook_thread` 决定，优先级不可颠倒**：先 `RegisterHotKey`，成功则不装钩子；失败才 `SetWindowsHookEx(WH_KEYBOARD_LL)` 兜底。

| 层级 | 覆盖场景 | 实现位置 |
|------|---------|---------|
| **RegisterHotKey**（默认，无钩子） | 其他应用/管理员游戏焦点 | `hotkey.rs` |
| **WH_KEYBOARD_LL**（仅注册失败时） | `Alt+Space` 系统保留 / 组合被占用 | `hotkey.rs` |
| **WndProc 子类化** | 自身窗口焦点（系统菜单路径） | `hotkey.rs` |
| **JS keydown 兜底** | 漏网按键（仅 Alt+Space 配置时） | `main.ts` |

**为何优先 RegisterHotKey**：LL 钩子 + `SendInput` 注入是杀软判定的键盘记录器特征（本项目曾因此被 Defender 报 `Trojan:Win32/Prowloc.A!cl` 误报）；且 LL 钩子受 UIPI 限制（游戏以管理员运行时收不到按键）。`RegisterHotKey` 无钩子、无注入，且不受前台程序权限影响。

**铁律**：
- 不要为了「少写分支」而无条件装钩子——会同时引入 AV 误报与游戏失效两个回归。
- `SendInput` dummy key **仅限 LL 钩子分支**（RegisterHotKey 由系统消费按键，无孤立 Alt 序列，注入纯属多余且增加 AV 特征）。
- 看门狗健康检查必须用 `HOOK_MODE` 守卫，否则无钩子模式会被误判为「钩子死亡」而反复重装。
- `install_hook_thread` 重装前必须向旧消息泵线程 `WM_QUIT`，否则改键会累积多个泵 → 热键与 Esc 重复响应。
- **热键判定必须「主键 + 修饰键」同时比对**：`VK_SPACE` 是 `Alt+Space` 与 `Ctrl+Alt+Space` 共用的主键，只比 VK 会让旧组合继续生效（曾出现「无论怎么改键，Alt+Space 都能呼出」）。任何新增的热键比较分支都要照此处理。
- **`HOTKEY_MODIFIERS` / `HOTKEY_VK` / `HOTKEY_COMBO` 必须经 `parse_and_set_hotkey` 一起设置**，且静态初值要与 `DEFAULT_HOTKEY` 一致。只改其中一部分 → 界面显示与实际按键不符（显示 Ctrl+Alt+Space、实际按 Alt+Space），并会连带把后端误判为「注册失败」而装钩子。

### 7.2 LL 钩子回调铁律

- **回调内只能 `PostThreadMessageW`**（微秒级返回），不能调用 Tauri 窗口 API
- **窗口操作丢独立线程**执行
- **Alt 检测必须用无状态查询**：`LLKHF_ALTDOWN` flag + `GetAsyncKeyState`
- **禁止手动 down/up 跟踪 Alt**：UAC/管理员窗口吞 Alt up → 永久卡死
- **Alt 的 vkCode**：`VK_LMENU(0xA4)` / `VK_RMENU(0xA5)`，不是 `VK_MENU(0x12)`

### 7.3 WndProc 子类化规则

- `WM_SYSCOMMAND/SC_KEYMENU` 是系统菜单弹出的**唯一**消息路径
- `lParam=0x20` 为 Alt+Space
- 录制期间不可吞 Alt+key → 检查 `RECORDING` 原子变量
- 非录制期间 `WM_SYSCOMMAND/SC_KEYMENU` **一律 `return 0`**

### 7.4 JS 兜底规则

- 仅当 `window.__lunac_hotkey_is_alt_space === true` 时拦截 Alt+Space
- 自定义热键后该标志为 `false` → JS 完全不干预
- 录制 Alt+Space 时 Chromium 吞 Space keydown → 只能走 WndProc 子类化路径（`RECORDING=true` 时 emit `lunac-hotkey-recorded`）

### 7.5 热键持久化环节

- `lunac-hotkey-recorded` listener：更新按钮文字 **+** 调用 `invoke("set_hotkey_combo")`
- `parse_and_set_hotkey` 末尾：重装热键后端（`install_hook_thread` → 优先 RegisterHotKey，失败才装钩子）；主窗口句柄未就绪时只设原子变量，由 `start_hotkey` 末尾统一装配
- `status.hotkey_hint` 用 `{hotkey}` 插值，改键后经 `__lunac_refresh_hotkey_hint` 刷新状态栏（避免残留旧热键）
- 启动时 `repair_auto_start_on_startup` 检查注册表路径一致性

---

## 第 8 章：窗口管理规则

### 8.1 前台锁定（ForegroundLockTimeout）

Windows 禁止后台进程抢占前台。Tauri `set_focus()` 可能静默失败。

**修复链**：`AttachThreadInput` 附加前台线程 → `SetForegroundWindow` 夺焦 → 3 次重试（间隔 16ms）

### 8.2 最小化检测

```rust
// IsWindowVisible 在最小化时仍返回 true
// 必须加 IsIconic 检测
if IsIconic(hwnd) {
    // 走 SW_RESTORE 而非 toggle
}
```

### 8.3 窗口 resize 规则

- `resizable: false` + `transparent: true` + `decorations: false` → **移除 `minWidth/minHeight`**（与 Tauri 2 有交互 bug）
- `setSize` 需 `core:window:allow-set-size` 权限
- WebView2 视口不能渲染超出物理窗口尺寸的内容 → 不能用 JS 测量再 resize 的内容驱动方案

### 8.4 隐藏/显示规则

```typescript
// 搜索内容不随失焦清空
win.hide();  // 纯隐藏，不清空任何状态
// Esc 才重置搜索框内容
```

- `onFocusChanged(false)` → 150ms debounce 防 WebView2 渲染瞬发
- 程序化 hide 时设 `blockingHide = true` 防 `onFocusChanged` 重复触发

---

## 第 9 章：流式传输 / AI 对话规则

### 9.1 streamId 守卫

```typescript
let streamId = 0;

// 任何会销毁正在流式渲染的 DOM 的操作前：
streamId++;  // 旧回调全部失效
isStreaming = false;
setStreamingUI(false);
streamCallback = null;
doneCallback = null;
cliTextCallback = null;
cliDoneCallback = null;
```

**触发场景**：`restoreSession`、`newConversation`、`closePluginView`、`startAgentChat` 错误路径、`startAIChat` 错误路径

### 9.2 消息队列模式

```typescript
let pendingMessages: string[] = [];
// 流式中用户发送消息 → 入队
// doneCallback 末尾 processQueue() 依次发送
```

### 9.3 会话保存用 id 复用

```typescript
let currentSessionId: string | null = null;
// 首次保存生成 ID，后续同一对话复用同一 ID 实现 upsert
// 避免每次保存都创建新记录
```

### 9.4 抽屉 DOM 清理

```typescript
// 关闭抽屉：直接 remove backdrop，不要仅 opacity:0
drawerBackdrop.remove();
drawerBackdrop = null;
```

---

## 第 10 章：剪贴板规则

### 10.1 双路径归一化

Rust 侧 `text.trim()` + JS 侧 `readText()` 返回的文本格式不一致（尾部换行/空格）。

```typescript
// ✅ 入口统一归一化
const normalized = text.replace(/\r\n/g, '\n').trim();
```

### 10.2 搜索框去重

```typescript
// 检查当前 searchInput.value，不只是历史记录
if (firstPart === searchInput.value.trim()) {
  return; // 搜索框已存在，跳过
}
```

### 10.3 剪贴板读取线程安全

读取剪贴板**必须用 `app.run_on_main_thread()`**，不能在线程池线程中读取。

---

## 第 11 章：常见反模式速查表

| 反模式 | 后果 | 正确做法 |
|--------|------|---------|
| 新增 API 不声明权限 | 调用静默失败 | 先加 `capabilities/default.json` |
| `invoke` 参数名与 Rust 签名不一致 | 保存静默失败 | 逐参数对齐 Rust `#[tauri::command] fn` 签名 |
| 串行 `await` 隐藏+启动 | 用户感知延迟 | `win.hide()` + `invoke().catch()` 无 await 并发 |
| 依赖 `innerHTML` 恢复 checkbox 状态 | checkbox 永远未选中 | settings 插件跳过缓存 |
| `scrollHeight` 测量决定窗口尺寸 | WebView2 视口裁剪导致测量不准 | 用确定性尺寸（条目数映射/固定高度） |
| `document.createElement` + `appendChild` 逐条插入 | 多次重绘闪烁 | `DocumentFragment` + `replaceChildren` 批量替换 |
| 流式回调无 streamId 守卫 | 关闭后旧数据污染新对话 DOM | `myStreamId = ++streamId` + 回调中 `if (myStreamId !== streamId) return` |
| CSS `overflow: hidden` + `<select>` 弹窗 | 下拉框无法展开 | 自定义下拉组件 |
| inline style 解除 overflow | WebView2 中不稳定 | 用 class + `!important` |
| transition 后无 fallback timeout | transitionend 永不触发 | `setTimeout(fallback, 500)` |
| 静态 import 改动态 import | listener 未就绪 | 保留静态 import，必要时 await 确保就绪 |
| `opacity:0` 隐藏 overlay | overlay 仍占 DOM 拦截点击 | `element.remove()` |
| 不检查 `pluginActive` 渲染混合结果 | 搜索渲染覆盖插件面板 | `if (pluginActive) return` |
| 不清理 `pendingMessages` | 新对话自动发送旧消息 | `closePluginView` 中 `pendingMessages = []` |

---
## 第 12 章：检查清单（PR/MR 前必过）

- [ ] `capabilities/default.json`：所有新增 API 调用均已显式授权
- [ ] `invoke()` 参数名与 Rust `#[tauri::command] fn` 签名一致
- [ ] 无串行 `await` 导致的延迟（隐藏、启动、读写并发优先）
- [ ] 新增渲染路径有关闭清理逻辑（streamId / isStreaming / callbacks）
- [ ] 无依赖 `innerHTML` 恢复交互状态的代码
- [ ] 无 CSS `overflow: hidden` 裁剪弹窗的风险
- [ ] 无原生 `<select>` 依赖（透明窗口下不渲染）
- [ ] `renderMixedResults` 等渲染函数有 `pluginActive` 守卫
- [ ] 剪贴板文本处理有归一化步骤
- [ ] 热键修改兼顾 RegisterHotKey + LL 钩子 + WndProc + JS 四层（不可无条件装钩子）
- [ ] 新增 UI 字符串使用 `t("key")` 而非硬编码文本（包括英文）
- [ ] 新增 `t()` key 需要同时在 `i18n.ts` 的 DICT 中添加翻译（至少 zh-CN, en）
- [ ] 新增按钮 / 控件已声明主题样式（`border-style` 不得为 `outset`；同族按钮并入同一族规则 —— ai-spec §11 规则 51）
- [ ] 改动过 `history` 装配的，已按 ai-spec §11 规则 23 核过 agent 日志的 `公共前缀=N/M条`（出现 `N < M` 即本侧就地改写）与 `请求用量` 行的 `read`（是否塌到 `system+tools` 量级）
- [ ] 新增 / 改名内置工具的，已同步 `defs()` + 三张判断表 + ai-spec §3.5 契约表 + `agent-implementation.md` §4.1 总数口径 + 前端黑名单候选名单，且 `cargo test` 的工具总数守门单测通过（ai-spec §11 规则 14 / 54）
- [ ] `npx tsc --noEmit` 通过
- [ ] `cargo check` 通过
- [ ] `cargo test` 通过（`core-agent` 与 `src-tauri` 两侧）

---
## 第 13 章：国际化 (i18n) 规则

### 13.1 使用 `t()` 而非硬编码文本

```typescript
// ❌ 硬编码（只显示一种语言）
statusText.textContent = "AI thinking...";
chatInput.placeholder = "Ask anything...";

// ✅ 使用 t() 动态翻译
statusText.textContent = t("chat.ai_thinking");
chatInput.placeholder = t("chat.placeholder");
```

### 13.2 参数插值

```typescript
// ✅ t() 支持 {param} 占位符
t("status.running_plugin", { name: pluginName(plugin.id) });
t("agent.done", { count: String(turn.toolCalls.length) });
```

### 13.3 新增 key 必须同时加翻译

任何 `t("new_key")` 调用必须在 `app/src/i18n.ts` 的 `DICT` 对象中添加对应条目，至少包含 `zh-CN` 和 `en` 两种语言。

### 13.4 语言生命周期

```typescript
// 启动时：先检测系统语言，再加载用户偏好覆盖
await initI18n();       // Rust GetUserDefaultUILanguage → BCP-47
loadSavedLanguage();    // localStorage 覆盖（用户手动选择）
// 运行时切换：
setLanguage("ja");      // 切换 + 持久化
```

---
*最后更新：2026-09-19 · 来源：项目 100+ 条 bug 修复经验 + 5 天高强度迭代 · 关联文档：[ai-spec.md](./ai-spec.md)（规范正文）、[agent-ui-spec.md](./agent-ui-spec.md)（界面/滚动条的唯一真相源）*
