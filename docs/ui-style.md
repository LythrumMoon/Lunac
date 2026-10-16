# 界面与前端风格规定

> **本文档是什么**：Lunac 的**界面视觉与交互规范**——色板与 CSS 变量、**所有按钮的类型与态**、圆角 / 间距 / 边框、图标、弹层 / 菜单 / 抽屉底色、滚动条、选中 / 禁用 / hover / focus、主题色明暗派生。写界面照这里写，改样式照这里查。
>
> **取代了哪些旧文档的哪些章节**：
> - `agent-ui-spec.md` 全文（对话区规范、运行方式三档、视觉规范、复核清单）；
> - `icon-style.md` 全文（线性 SVG 参数、残影语言、emoji 禁令）；
> - `ai-spec.md` §2.1 / §2.1.2（搜索与详细搜索）、§11 规则 9 / 21 / 44 / 45 / 46 / 47 / 48 / 49 / 50 / 51（图标、滚动条、结果区高度、外观主题、主题包锁定、结果呈现、中性叠加 token、emoji、i18n 与分类、按钮主题样式）。
>
> **历史细节见 `docs/archive/agent-ui-spec.md`、`docs/archive/icon-style.md`、`docs/archive/ai-spec.md`。** 禁令速查见 [`forbidden.md`](./forbidden.md) §禁用 2。

---

## §界面 1 色板与 CSS 变量

- **只有一套固定深色主题**（`:root` 变量，无 `data-theme`、无浅色、无 `prefers-color-scheme`）。**不新增主题切换机制**。
- **基础变量**（全部只用这些，不新造颜色）：
  - 文字：`--text`（正文）/ `--text-dim`（次要，如元信息）/ `--text-muted`（极弱，如占位）；
  - 面：`--surface-glass`（卡片底）/ `--surface-glass-hover`（hover）/ `--border-glass`（描边）；
  - 语义：`--accent` + `--accent-bg` + `--accent-border`（进行中 / 可交互）、`--green`（成功 / 放行）、`--red`（失败 / 危险）、`--yellow`（警告 / 非零退出码）、`--blue`（信息 / 链接）；
  - 圆角：`--radius`（默认 14）。
- **派生 token（主题色唯一真相源）**：
  - `--accent-rgb`：凡 accent 带 alpha 必须写 `rgba(var(--accent-rgb), x)`，**不得**硬编码 `rgba(192,160,160,x)` 这类分量；
  - `--ink-rgb`：中性叠加基色 —— `rgba(255,255,255,α)` → `rgba(var(--ink-rgb), α)`。**2026-10-11 起由「蒙版色（overlay）」的色相 + 文字亮度派生**（不再是写死的白 / 黑），见 §界面 11。
  - `--shade-scale` + `--shade-rgb`：凹陷强度与基色 —— `rgba(0,0,0,α)` → `rgba(var(--shade-rgb), calc(α * var(--shade-scale)))`。**2026-10-11 起 `--shade-rgb` 由「底色（base）」的暗变体派生、`--shade-scale` 由底色明暗定**（暗底 1 / 亮底 0.35），见 §界面 11。
  - **四类自定义颜色**（`--surface-*` 底色 / `--btn-line-*` 按钮线条 / `--btn-bg-*` 按钮背景 / `--other-overlay-*` **按钮蒙版**（旧称「其他颜色」，2026-10-11 改名））：**凡按钮 / 拉条 / 文本框 / 选中态 / 文字 / 图标 / 描边 / 中性底 / 下拉框（本体 + 可染色的收合面 / 选项，见 §界面 12）的颜色一律走这四类**，不得再写死色值。详见 §界面 11。**注（2026-10-11 更正）**：滚动条**不在**此列（轨道不描边、滑块实心无 border，与四类无关，见 §界面 6）。
- **默认观感可变但必须可读**（2026-10-11 取代旧「默认逐像素不变」）：四类未自定义时**回落主题色 / 主题派生**，观感允许与改造前不同，但**文字对比度不得崩**（默认实测 ≈ 13:1）。
- **例外**（允许裸色值）：取色器的彩虹 / 黑白渐变与 `.ap-sv-cursor` 描边、语义红绿、`--glass-sheen-image` 的玻璃反光白高光。

## §界面 2 按钮类型与态（必须覆盖所有按钮）

**两类按钮，颜色归属不同：**

| 类型 | 颜色 | 代表 class |
|---|---|---|
| **动作按钮**（可点的「操作」） | 底色走 accent 系、**边框走 `--btn-line-*`**（默认配方同 accent，见 §界面 11）；**文字灰阶** | `.settings-save-btn` / `.settings-install-btn` / `.settings-hotkey` / `.settings-skill-open` / `.custom-model-ok` / `.memo-save-btn` / `#tool-editor-save` |
| **语义 / 状态按钮** | 语义色**固定、不跟随主题** | `--green`（成功 / 放行）`--red`（危险 / 拒绝）`--yellow`（警告）`--blue`（信息）：`.settings-save-msg` / `.tool-badge.valid\|invalid` / `.approval-allow\|deny` / `.agent-status.*` / `.file-chip` |

- **所有新增按钮 / 控件必须显式声明主题样式**（WebView2 里**没有自动继承主题的原生按钮**，漏写 CSS 就退回浏览器默认外观：浅色凸起方块、`border-style: outset`）。**同族按钮并入同一族规则**：
  - 动作按钮 → `--accent` 系底色 / 边框 + 灰阶文字（hover 加亮）；
  - 中性 / 危险按钮 → 无底色 + `--border-glass` + `--text-dim`，hover 变 `--red`。
- **回归口径**：`getComputedStyle` 里 `borderTopStyle` 不得是 `outset`、`backgroundColor` 不得是 `buttonface` / `rgb(239,239,239)`；并顺带读一条靠后规则的值（inline `<style>` 若被语法错误打断，其后规则会被整体丢弃）。
- **危险操作**（删除等）保留语义色 `var(--red)` 或 hover 变红提示，但仍是同风格线性图标；两段式确认态用 `[data-armed="1"]` 红色确认。
- **文字 / 图标一律走四类派生，不用 `color: var(--accent)`**（2026-10-11 收编）：`#settings-btn:hover` / `.ai-response .cursor-blink::after` / `.ap-picker-toggle:hover` 以及**全部 `caret-color`**（含 `settings.ts` / `tool-editor.ts`）此前是 `var(--accent)`，现统一改走 **`rgb(var(--other-overlay-rgb))`**（= 按钮蒙版，默认与 accent 同值、观感不变；蒙版设黑 ⇒ 随之变黑）。**AI 输入栏那排图标按钮（`#chat-add-file-btn` / `#chat-send-btn` / `#chat-stop-btn` / `#chat-new-btn` / `#chat-more-btn`）的图标色走 `rgb(var(--btn-line-rgb))`**（= 按钮线条，见 §界面 11）。实测：把主色改 `#3a7bd5` 后，**设置侧栏选中项、下拉选中项、热键按钮、聊天用户气泡正文、memo 标签、复制/编辑/保存按钮的文字**必须仍是灰阶（只有底色 / 边框 / 图标变色）。注意 `caret-color: var(--accent)` 含子串 `color: var(--accent)`，批量替换必须按 CSS 规则块切分。
- **AI 输入栏那排按钮形状一致**：`#chat-add-file-btn` / `#chat-more-btn` / `#chat-new-btn` 为 **28×28**，`#chat-send-btn` / `#chat-history-btn` / `#chat-stop-btn` 为 **30×30**，**圆角全部 6px**，底色 / 描边 / 图标取 `--btn-bg-*` / `--btn-line-*`。
- **运行方式**是「盾牌图标 + 三格点阵」，不写文字（档位名 / 提示句只在 `title` / `aria-label`）；颜色区分语义：手动 `--text-dim`、白名单 `--green`、自动 `--red`。

## §界面 3 圆角 / 间距 / 边框

- 圆角一律 `var(--radius)`（搜索 / 结果区可各取 `--radius-search` / `--radius-results`，默认都等于 `--radius`）。
- 间距按既有 **4 / 6 / 8 / 12px** 体系，**不新造尺度**。
- 文案按钮 = 图标（≤14px）+ 文字，间距 4–6px；纯图标按钮需 `title` / `aria-label`。

## §界面 4 图标（严格线性 SVG）

- **画布** `viewBox="0 0 24 24"`；**显示尺寸**按钮内 `width="12" height="12"`（小按钮）/ 行内 12–14px。
- `fill="none"`、`stroke="currentColor"`（跟随按钮文字色，默认 `--text-dim`，hover 变亮）、`stroke-width="2.2"`（残影 / 装饰可 1.6~1.8）、`stroke-linecap="round" stroke-linejoin="round"`。
- **残影 / 点彩语言**：一个同形副本偏移 `-0.6~-0.7`、`opacity 0.28`、更细笔画，或一个点彩圆点 `r≈0.9`（高光）。
- **复用优先**：`COPY_SVG` / `RETRY_SVG` / `ROLLBACK_SVG` / `pluginIconSvg()` / `THINK_SVG` 已有路径，不重复造；新图形命名 `*_SVG`。
- **功能按钮禁用 emoji**（见下）。**深色 + 毛玻璃**：图标一律 `currentColor`，不额外换彩色图标表达 hover / 选中。
- **大图标**（插件入口、结果区主图标）走 `app/src/main.ts` 的 `pluginIconSvg()`。

## §界面 5 emoji 纪律

- **UI 文案里永远不得内嵌 emoji 当图标**：按钮标签、状态行 / 提示行、占位文案一律「**纯文字**」或「**线性 SVG + 文字**」。`📋 复制结果`、`📁 选择文件`、`🖼️ 暂无图片`、`⚠️ …`、`⏳ …`、`✅ 完成`、`❌ 失败` 全部不合格。
- **判据（一句话）**：这个字符是「**一条可点控件 / 一句提示的文案的一部分**」（→ 禁止），还是「**一个条目 / 一条状态的标记**」（→ 允许）？带 `U+FE0F`（变体选择符，强制彩色）的**按 emoji 算**（`⚠️` 禁止、`⚠` 允许）。
- **允许且必须保留**（不属于本条约束）：
  - **列表项 / 条目图标**：结果区行图标、目录数据自带的 `item.icon`、`plugin.icon`（未知插件回退）、`.clip-item-icon`、`.file-chip-icon`、web-search 引擎图标；
  - **单色文字状态符号**：`✓ ✗ ⚠ ↔ ↩ ◐ ○ ✔ ✕ × ＋`（表达状态，如 `✓ 已复制` / `✗ 失败`）。
  - **例外中的例外**：**任务抽屉 `#todo-drawer` 整个抽屉一个图标都不许有**（连单色状态符号也不用，状态只靠文字颜色）。
- **仓库页面（README / `docs/**` / `agent-templates/**/README.md`）同样不插装饰性 emoji**（清单、表格、标题一律纯文字）；要指认某个字符时用文字描述。

## §界面 6 弹层 / 菜单 / 抽屉 / 下拉

- **统一细滚动条（优先级高，不得回退）**：项目内**所有**可滚动容器共用 `styles.css` 里**唯一**的全局 `::-webkit-scrollbar`（宽 / 高 4px、滑块 `rgba(var(--ink-rgb), 0.18)`、`border-radius: 2px`、hover `0.32`）。**2026-10-11 更正（用户裁定：先前要求「轨道描边」是误判，已撤销）**：滚动条恢复为「**滑块实心无 border、轨道也不加 border**」—— 滑块 `::-webkit-scrollbar-thumb` 实心 `rgba(var(--ink-rgb), 0.18)`、`border: none`；轨道 `::-webkit-scrollbar-track` **不描边**（`border: none`），底色可为**底色派生的中性底**（现为 `transparent`，即不加底色）。这是全局那一条规则上写的，所以对**所有**滚动条生效（含下拉框右侧的滚动条，见 §界面 12）。
  - **统一口径：拉条（滚动条）= 滑块（圆点 / 拇指）实心、无 border；轨道不描边**。range 滑块同理**滑块实心无 border**、**其轨道也不描边**，原因有二：① **原生 range**（调音竖拉条 `.music-tune-vslider`、外观页 `.ap-slider input`）靠 `accent-color` 上色、**刻意不重画**（保留键盘与无障碍语义，见 §界面 8），其轨道 / 滑块由 UA 绘制、CSS 无处落 border；② **自绘 range**（音乐音量 `.music-vol`、桌宠 `.pet-range`）的轨道只有 3–4px 高，1px border 会把填充整个吃掉、视觉退化 ⇒ 不加。自绘滑块一律实心 `rgb(var(--btn-line-rgb))` + `border: none`。
  - **唯一例外**：`#chat-node-rail` 用 `::-webkit-scrollbar { display: none }` 藏起节点导轨的滚动条（历史实现，**不得据此给别的容器单独声明**）。
  - **禁止写 `scrollbar-width` / `scrollbar-color`**（任何地方）——Chromium 见到非 `auto` 会整段忽略 `::-webkit-scrollbar`；
  - **禁止按容器单独声明**滚动条样式（新容器必漏）；新容器写 `overflow-y: auto` 即自动获得统一外观；
  - **不要藏滚动条**；需要可滚动就保留 `overflow-y: auto`。
- **禁用原生 `<select>`**（透明窗口不渲染弹层）→ 用既有 `.custom-select` 或胶囊 / 分段控件。**原生 `<select>` 的弹出层（选项行 + 其右侧滚动条）由 OS / 浏览器绘制、不可样式化** ⇒ 四类颜色无从接管；下拉框的颜色归属（本体 / 选项 / 滚动条描边）见 §界面 12。
- **弹层底色不得硬编码**：`#chat-more-menu` / `#chat-drawer` / `.custom-select-dropdown` 走 `var(--surface-glass)`；全工程不得再出现 `rgba(28,26,32…` / `rgba(24,24,37…` / `#1c1a20` 这类常量底色。
- **顶层插件面板不自加压暗**：OCR 的 `.ocr-image-panel` / `.ocr-text-panel` 必须 `background: transparent`（与 memo 的 `.plugin-result` 一致），左右分栏靠 `border-right`；`rgba(var(--shade-rgb), calc(α * var(--shade-scale)))` 只允许出现在**嵌套**的次级块（输入框 / 工具卡 / 弹层 / 遮罩）。
- **`overflow` 行为**：`overflow: hidden` 会裁剪 WebView2 原生弹窗（换自定义下拉或动态解除）；但**不得**把 `overflow:visible` 加到滚动容器上（会拆滚动盒）。抽屉与 `#context-menu` 的 `overflow` 行为不得被新的 `overflow:hidden` 破坏。
- **审批卡排版**：`.approval-item` **不设固定高度**；命令区**不许第二层滚动容器**（只由 `.approval-batch-body` 承担）；`└ ` 前缀行挂 `.with-sep`（`padding-left: 2ch; text-indent: -2ch`）。⚠️ **不要给命令区加复制按钮**。
- **折叠 / 展开（2026-10-11 通用纪律）**：「行多 / 字段多」的可编辑块**允许折叠**：**折叠态只显示一行摘要**（关键字段用 ` · ` 拼接）、**过长一律省略**（`white-space:nowrap; overflow:hidden; text-overflow:ellipsis`），**展开态**显示全部字段。折叠开关用**线性 SVG 箭头**（`▸/▾` 语义、`currentColor`、展开时旋转 90°、带 `aria-expanded`），**不写字符**；**缺省展开**（不隐藏既有功能）。首次落地 = 桌宠「动作链」（`pet.ts` 的 `.pet-chain-toggle` / `.pet-chain-summary`，折叠态摘要 = 每步「文件 · 循环 · 硬停」的拼接）。

## §界面 7 选中态 / 禁用态 / hover / focus

- **hover / 选中态靠按钮自身背景色变化表达**，不额外换用彩色图标。
- **「按钮蒙版」（旧称「其他颜色」，2026-10-11 改名）只有一组控件**（整块里**恰好**一个取色器 `#ap-ooverlay` + **一条**透明度滑块 `overlayAlpha`），驱动**四处浮层**：
  1. 结果项 `.result-item:hover` / `.selected`；
  2. 设置侧栏选中项 `.settings-sidebar-item.active`；
  3. 分类标题 `.settings-group-title`；
  4. **顶栏图标按钮的 hover 浮层**（`#settings-btn` / `#plugin-bar-exit` / 大窗口 `#detached-*`）。
  - **不得**把「运行端输出文字 / 对话背景 / 设置面板背景」加回来（三项各回主题）。
  - 默认配置下 `<html>` 的 `style` 上不应出现 `--other-overlay-*`（颜色空串 ⇒ 清掉内联值）；四处浮层此时分别是 `rgba(192,160,160,0.1)` / `(…,0.14)` / `(…,0.14)` / `(…,0.2)`。
- **取色器是应用内自绘**：色号框在最左 + 色块 + 30×30 圆角方展开按钮，面板整宽、SV 高 40px；**不得**退回系统 `<input type=color>` 或色轮。
- **分组小标题**：放大 + 600 字重 + accent 竖条 + 淡底框。
- **焦点 / 输入**：`caret-color: var(--accent)`（灰阶文字的例外之一）；文本输入一律「失焦 / 回车才提交」，编辑期间周期渲染要跳过（焦点守卫）。
- **禁用态**：置灰 + 不可点；注意 **`.settings-save-msg` 默认 `opacity:0`** —— 失败文案会「有高度、看不见」，看着像空白占位，状态要直接写进那行提示。

## §界面 8 主题色明暗与主题包

- **不做浅色主题**（见 §界面 1），但 **token 必须能跟随底色 / 蒙版色明暗**（2026-10-11 重写）：`--shade-rgb` / `--shade-scale` 由**底色**派生（暗底 `1`、亮底 `0.35`）；`--ink-rgb` 与三档文字由**蒙版色（overlay）**色相 + 亮度派生，底色为浅色时文字亮度**反向**（浅底深字，见 §界面 11）。
- **主题色只能有一个真相源 `--accent-rgb`**；改主色为 `#3a7bd5` 后，相关元素的 `border-color` / `background-color` / `color` 三处都要跟着变。
- **主题包锁定态**：`themeId !== "default"` 时 `#ap-color-group` 必须带 `.locked`（灰掉 + 不可点）、`#ap-bg-pick` 必须 `disabled`、`#ap-lock-note` 必须可见；切换主题时**实时同步**；「自定义」那五个**玻璃质感拉条在任何主题下都必须还能拖**。
- **主题列表是「泡泡框」**：每个主题文件夹一个 `.ap-theme` 胶囊泡泡 = 主色圆点 `.ap-theme-dot`（取 `manifest.tokens.accent`）+ 名称 + 内置徽标，`title` 是文件夹名；取数必须仍是 `themes()`（Rust `list_themes()` 扫文件夹）。
- **主题图标只走 `pluginIconSvg()` 一个出口**；设置面板只调 `window.__lunac_appearance`，不自己写 localStorage。

## §界面 9 视窗与布局硬边界

- **窗口宽度 `WIN_WIDTH = 800`**；透明 / 无边框 / 毛玻璃形态、`#search-bar → #results-container → #status-bar` 纵向结构**不变**。
- **AI 态 / 插件态高度**：embedded 360 / detached 600 / OCR detached 520（× `currentZoom`），**离散直设、不走滑动动画**；详细搜索大界面是并列固定档 `640 × zoom`。**不得**新增 `setSize`、不得把插件态拉进高度滑动动画。
- **搜索结果区高度 = 行数 × 56px**：`#results-container` **无** `min-height` / `max-height` 定值，唯一上限是 JS 写的 `--results-max-h`。**不得**退回固定 `min-height:120px` / `max-height:380px`，也**不得**用 `100vh` 当上限。
- **任何往 `#results-list` 增删条目的函数，收尾都要同步调一次 `applyWindowSize()`**；唤出路径（`lunac-window-shown`）必须调 `forceHeightReassert()`。
- **简洁搜索结果区不再有 `.result-item-badge`**（两处例外必须保留：详细搜索面板的行标签、`quicklaunch` 面板的「常驻 / 本次」状态标记）。
- **详细搜索预览区**：`#detail-main` 横向 flex，`#detail-preview` 宽 210；`#detail-results` 的 `min-width: 0` 不能漏。
- **任务抽屉与子任务面板**：子任务面板是 `agent-flow` 内部的**分组面板**（每子任务一行），**不得**改左右分栏、不得加 `max-height` / `overflow-y`；任务抽屉有内容的唯一判据 = 待办 / 改动文件 / 后台命令三者非空。
- **设置侧栏恰好五项**：常规 / 风格 / AI / 搜索 / 插件（无独立「技能扩展」）；`#sp-ai` 内 `.settings-group-title` 恰好 4 个（AI 模型 / 技能 / 工具 (MCP) / 用量与成本）。
- **运行方式三档不明文称「沙箱」**（见 [`forbidden.md`](./forbidden.md) §禁用 1）。

## §界面 10 文案 / i18n / 提示精简

- **所有文案走 `t()`**，新增 key 进 `app/src/i18n.ts` 的 DICT（`agent.*` / `settings.*` / `plugin.<id>` …），**至少 zh-CN + en**（现状 5 语言：zh-CN / zh-TW / ja / ko / en），**禁止硬编码中文**。插件名 / 描述走 `pluginName()` / `pluginDesc()`。
- **面向用户的提示必须精简（高优先级）**：① 默认不说过程；② 一个动作最多一条提示（能靠界面状态自明的就不弹）；③ **失败只说「哪一步没成 + 用户能做什么」，原始错误进日志**；④ 开发者向数据（路径 / 版本号 / 字节数 / 哈希 / 端口 / 进程名）一律不进主界面（要留就放进「详情 / 开发者选项」）；⑤ 超过一句话的文案先问「这句能不能删」而不是「怎么排版好看」；⑥ **唯一例外：安全与不可逆操作的确认文案允许长**。
- **判据**：把文案贴给一个没用过本软件的人——他若问「这是什么意思」或「我该做什么」= 没写完（信息不足）；他若说「我知道该怎么做」= 已经多余（该删）。

---

## §界面 11 自定义颜色体系（四类）

- **一件事**：用户在「设置 → 风格」里能改**四类颜色**（底色 / 按钮线条 / 按钮背景 / 按钮蒙版），经 `main.ts` 的 `applyAppearance()` → `applyFourClassNeutrals()` 合成到 `<html>` 行内 CSS 变量上。**这四类是整个 UI 颜色的唯一来源**：四类之外，除白名单语义色外，界面上不得再有任何颜色（含**文字 / 图标 / 描边 / 中性底**）。
- **硬验收判据（2026-10-11）**：把四类**全设 `#000000`**（四个取色器都写成真 hex）+ 文字明度拉到极端，逐界面扫（搜索栏 / 结果区 / AI 输入栏 / 设置各页 / 各插件面板）**不应再出现非黑的「按钮 / 拉条 / 文本框 / 文字 / 图标 / 描边」颜色**，只剩白名单语义色。
- **字段 → CSS 变量 → 默认值（唯一真相源）**：

| 类别 | 字段 | CSS 变量 | 默认值（未自定义时） | 负责范围（2026-10-11 定稿） |
|---|---|---|---|---|
| 底色 | `baseColor`（空串 = 跟随主题派生） | `--surface-rgb` / `--surface-rgb-hover`（+ 派生 `--ctx-*`、`--shade-rgb`） | `28, 26, 32` / `48, 44, 54` | 插件 / 面板底；**并派生中性黑遮罩**（`--shade-rgb` = 底色的暗变体，供 `rgba(var(--shade-rgb), α)`）；**拉条轨道**（音乐播放 / 扫描进度条轨道，见 §界面 11 白名单注） |
| 按钮线条 | `btnLineColor`（空串 = 跟随主题色）+ `btnLineAlpha` | `--btn-line-rgb` + `--btn-line-alpha` | `var(--accent-rgb)` + `0.32` | 未选中**文本框 / 按钮的 border**、按钮内 **icon 线条**、**拉条圆形滑块**（含音乐进度圆点）/ **拉条填充**（音乐播放 / 扫描进度条填充）、切换开关轨道、**音乐播放模式按钮的描边**；**并接管所有面板 / 文本框描边**（`--border-glass` / `--ctx-border-glass`，α 固定 `0.10`）。**注（2026-10-11 更正）**：滚动条**已不使用**按钮线条 —— 滚动条轨道不描边、滑块实心无 border（见 §界面 6），故本节不再列「滚动条轨道描边」 |
| 按钮背景 | `btnBgColor`（空串 = 跟随主题色）+ `btnBgAlpha` | `--btn-bg-rgb` + `--btn-bg-alpha` | `var(--accent-rgb)` + `0.14` | 未选中背景（全局 `#app button:where(...)`、输入栏那排按钮及其 hover 抬档） |
| 按钮蒙版（旧称「其他颜色」，2026-10-11 改名） | `overlayColor`（空串 = 跟随主题色）+ `overlayAlpha` | `--other-overlay-rgb` + `--other-overlay-alpha`（派生 `-nav-alpha` = ×1.4、`-btn-alpha` = ×2） | `var(--accent-rgb)` + `0.1` | 选中态 border + 蒙版 + icon + 文字；**并接管文字色与图标色的色相**（见下）；含**下拉框选项的悬停 / 选中**（见 §界面 12）、**音乐选中框**（`.music-tr.sel` / `.music-mt.sel` 的整圈色框 + 淡填充）与**音乐播放模式按钮的激活态文字** |

- **三条派生链（`applyFourClassNeutrals`，JS 算、CSS 只消费；规则：颜色字段空串 ⇒ 用主题色 `--accent`）**：
  1. **文字 / 图标 ← 蒙版色（overlay）**：`--text` / `--text-dim` / `--text-muted` 的**色相与饱和取自 overlay**（饱和收敛到 ≤30），**亮度** = 默认可读档（89 / 68 / 50）× `overlayHSL_L / 69`，再加上 **`textLight`（文字明度滑块）偏移**。偏移在缩放**前**加 ⇒ overlay 全黑时三档文字**恒为 0（黑）**，滑块拉到极端也不越界。`--ink-rgb`（半透明叠加基色）取同一份「overlay 色相 + 文字亮度」。
     - **默认实际取值**（overlay = `--accent` #c0a0a0，hsl(0,20%,69%) ⇒ 缩放系数 1）：`--text` = `hsl(0, 20%, 89%)` = `rgb(233,221,221)`；`--text-dim` = `hsl(0,17%,68%)` = `rgb(187,160,160)`；`--text-muted` = `hsl(0,14%,50%)` = `rgb(145,110,110)`；`--ink-rgb` = `233, 221, 221`。**可读性**：正文 `rgb(233,221,221)` 在默认底色 `rgb(31,25,25)` 上对比度 ≈ **13:1**（远高于 WCAG AA 的 4.5:1），三档均清晰。
  2. **描边 ← 按钮线条（btnLine）**：`--border-glass` = `--ctx-border-glass` = `rgba(var(--btn-line-rgb), 0.10)`（α 固定、不随 `btnLineAlpha`）。默认 = `rgba(192,160,160,0.10)`，与改造前 `rgba(232,216,202,0.10)` 观感接近（同为 10% 淡描边）。
  3. **中性黑遮罩 ← 底色（base）**：`--shade-rgb` = 底色各通道 ×0.25（暗变体、近黑；黑底 ⇒ 全黑）；`--shade-scale` = 暗底 `1` / 亮底 `0.35`。默认 = `8, 6, 6`（在默认底色 `31,25,25` 上，与原纯黑 `0,0,0` 近乎不可分）。消费方一律写 `rgba(var(--shade-rgb), calc(α * var(--shade-scale)))`。反差四件套 `--ctx-*` 同源：叠加基色取 ink、凹陷基色取 shade、描边取 line。
- **派生规则**：颜色字段是**空串哨兵** —— 空串 = 用户没动过 ⇒ 回落 `:root`（= `var(--accent-rgb)`，跟随主题色）；用户一动取色器就写成真 hex，从此以用户值优先。
- **`--border-glass` / `--ctx-border-glass` 归「按钮线条」**（2026-10-11 翻牌，取代此前标记的「待拍板」）：`main.ts` 无条件按 `--btn-line-rgb` 写出这两条内联值（插件悬浮窗经 `broadcastThemeVars` 拿到同一份），文本类输入框 / 面板描边由此统一受控。
- **强制要求**：
  - **今后设计任何按钮 / 拉条 / 文本框 / 选中态 / 文字 / 图标 / 描边 / 中性底（含新插件）时，颜色一律走上面四类变量，不得写死 `#rrggbb` / `rgb()` / `rgba()` / `hsl()` 字面量。**
  - 透明度类颜色走对应 `*-alpha` 变量（或由它 `calc()` 派生），**不写死 α**。
  - 插件是**独立窗口**：CSS 与主窗共用同一份 `styles.css`，但变量靠 `broadcastThemeVars()`（`lunac-theme-vars` 事件）转发 —— 插件里**直接用 `var(--btn-*)` / `var(--other-overlay-*)` / `var(--ink-rgb)` / `var(--shade-rgb)` 即可**（主窗已把内联值转发过来），**不要假设 `:root` 之外会自动继承**，也不要自己再算一份派生。
  - 遇到「看起来不受按钮颜色控制」的反馈：先对齐**形状**（尺寸 / 圆角 / 边框）与**状态规则**，确认同源后再查颜色来源。
- **白名单（故意不并入四类，保持固定；把四类全设黑后仍允许存在的颜色与理由）**：
  - **语义色**（固定、不随主题）：`--red` / `--green` / `--yellow` / `--blue` 及其字面量 —— `rgba(192,138,138,*)`（危险）/ `rgba(157,180,172,*)`（成功）/ `rgba(201,184,150,*)`·`rgba(198,168,106,*)`·`rgba(214,187,121,*)`（警告）/ `rgba(148,144,180,*)`（信息）。**理由**：它们表达状态（成功 / 失败 / 警告 / 信息），若跟随四类会丢失语义。
  - **音乐「正在播放」文字高亮**（`.music-*.playing`：正在播放那一行 / 歌单只提亮文字）与**左栏编辑态勾中行**（`.music-pl.sel` 的 `--accent` 底 + 左缘）。**理由**：与「正在播放」/ 编辑勾选这一**插件内运行态**绑定。（注：`.music-*.playing` 的文字强调**实际已走 `rgb(var(--other-overlay-rgb))`**，见 §界面 2；但本轮未点名收编它，按用户口径**保留**。）**2026-10-11 第 4 轮：播放 / 扫描进度条与选中框已收编** —— 进度条填充（`.music-bp-fill` / `.music-progress-fill` / `.music-scan-fill`）改走**按钮线条**、其轨道改走**底色**派生的中性黑；选中框（`.music-tr.sel` / `.music-mt.sel`）改走**按钮蒙版**；**正在播放文字高亮仍保留**。
  - **封面占位渐变**（`.music-pl-liked-cover` 紫色渐变、`.music-pl-trend-cover`）、**截屏暗幕**（`.shot-body{background:#000}` 与 `#shot-size` / `#shot-hint` 的 `#fff`）、**终端底 / 前景**（`.tool-card .tool-term{background:#0b0b0c}` 与 `main.ts` 的 xterm theme）、**取色器自身彩虹 / 黑白渐变与 `.ap-sv-cursor` 描边**。**理由**：这些是**内容本身**（图片像素 / 屏幕像素 / 终端 ANSI），不是界面配色。
  - **`--glass-sheen-image` 反光白**。**理由**：镜面高光物理上就是白的。
- **取代旧约束**：旧文「可覆盖变量的默认值必须逐项等于改造前硬编码值 / 默认逐像素不变」**本次（2026-10-11 用户裁定）明确作废**。新约束 = **默认观感可变，但必须可读、可用**（文字对比度不得崩，见上面「默认实际取值」的对比度数据）。
- **自查清单**（写控件前逐条过）：
  1. 我写的这个颜色，**用户把四类全设 `#000000` 时会不会变黑？**（文字 / 图标 / 描边 / 中性底也要能变黑）
  2. 它是**白名单**里的语义色 / 内容色吗？是 ⇒ 固定；不是 ⇒ 必须走四类或其派生变量。
  3. 我用了**字面量**吗（`#rrggbb` / `rgb()` / `rgba()` / `hsl()`）？只有白名单允许。
  4. 透明度走 `--*-alpha` 还是写死的？写死 ⇒ 改。
  5. 这是**新插件**吗？独立窗口里 `var(--btn-*)` 来自转发事件，别再算一份派生。
  6. 这是**下拉框 / 列表项**吗？本体走底色 + 按钮线条，选项的悬停 / 选中走按钮蒙版（滚动条与四类无关：轨道不描边、滑块实心无 border，见 §界面 6 / §界面 12）。

## §界面 12 下拉框（唯一标准组件 = `.custom-select`）

> **2026-10-11 重写（用户裁定）**：桌宠下拉「参照设置 → AI → 提供商」要求了三次都没做成，根因有二 ——
> ① 规范只说「以设置里的下拉为基准」却**没把参照组件本身写进来**，执行者只能猜；
> ② 桌宠那几处是**原生 `<select>`**（弹出层由 OS 绘制）⇒ **不可能**与参照组件长得一样，只调收合面永远对不上。
> 本节把参照组件逐字写死，并明确「原生 `<select>` 不可对齐 ⇒ 必须换成 `.custom-select`」。

### §12.1 组件名与定位

- **`.custom-select` 是本项目唯一的「标准下拉框」组件**（自绘）。**新插件 / 新界面的下拉一律用它**。
- 实现分两处（单一真相源，见 [`code-standards.md`](./code-standards.md) §规范 2）：
  - **markup 与行为** → `app/src/plugins/builtin/custom-select.ts`：
    `customSelectHtml(id, options, extraAttrs?)` 出 markup；`attachCustomSelect(dd, hooks)` 接线
    （开合 / 选择 / 外点关闭 / 长列表滚轮 / 边界翻转限高）；`closeAllCustomSelects(scope)` 关全部。
  - **样式** → `app/src/styles.css` 的「`.custom-select`」一节，**共享**：插件悬浮窗（`plugin.html`）
    链接的就是同一份 `styles.css`，所以**任何插件渲染本组件都自动拿到同一副外观**，不必各写一份；
    四类变量由主窗经 `broadcastThemeVars()` 转发。
- **原生 `<select>` 仅作「非受控场景」的例外**，且必须在代码注释里说明为什么不能自绘（现有豁免见 §12.6）。

### §12.2 完整 markup（逐行照抄）

```html
<div class="custom-select" id="{id}">
  <button class="custom-select-trigger" type="button">
    <span class="custom-select-label">{当前显示文本}</span>
    <svg class="custom-select-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="6 9 12 15 18 9"/></svg>
  </button>
  <div class="custom-select-dropdown">
    <div class="custom-select-option selected" data-value="{值}">{标签}</div>
    <div class="custom-select-option" data-value="{值}">{标签}</div>
  </div>
</div>
```

- `id` 可空：需要 `#id` 定位（设置面板）时传；动态列表用 `data-*` 标识即可。
- 选项：`data-value` = 回调值；选中项加 `.selected`；可选 `data-tag`（如 `builtin` / `custom`，供删除逻辑区分）与
  尾部 `.cs-del`（`×` 删除钮，由 `deletable` 控制）。
- `customSelectHtml` 的第 3 参 `extraAttrs` 可往根 div 追加属性（如 `data-hk-field="target"`）。

### §12.3 行为契约（以 `custom-select.ts` **现有实现**为准，不编）

1. **点 trigger 开合**：加 / 去 `.custom-select.open`（`.open` 同时控制箭头旋转 180°、面板 `display:block`）。
2. **点选项即选**：给该项加 `.selected`（并清掉同级其它项的 `.selected`）、把 `.custom-select-label`
   **回填成该项文本**、关闭面板、回调 `onPick(value)`。
3. **点外部关闭**：document 上的点击若落在 dd 之外 ⇒ 关掉；每个 dd 一个**自清理**监听（节点被重渲染摘掉后自动注销）。
4. **同一时刻只开一个**：打开前先关掉同 `scope` 内的其它 `.custom-select.open`。
5. **键盘：未实现（如实标注）** —— **当前只有鼠标操作**：没有 `↑`/`↓` 移动、没有 `Enter` 选择、也**没有 `Esc` 关闭**
   （`custom-select.ts` 里没有 keydown 处理；`Esc` 只在「自定义项内联 `<input>`」那个输入行上生效）。
   要加键盘支持属**新功能**，须同步改 `attachCustomSelect` 与本节。
6. **长列表**：面板 `max-height` 限高 + 内部滚动；项超出可视区后滚轮手动滚自己（避免被外层滚动容器吞掉）。
   **边界翻转**：下方放不下时翻到上方（`position:absolute`，可用空间按最近滚动容器 `#results-list` 的可视区算）。

### §12.4 四类颜色归属（见 §界面 11）

- **本体（trigger）**：底走**按钮背景** `rgba(var(--btn-bg-rgb), var(--btn-bg-alpha))`（trigger 是 `<button>`，
  全局 `#app button:where(...)` 亦给同值）；描边走**按钮线条** α0.10（`var(--ctx-border-glass)` = `rgba(var(--btn-line-rgb), 0.10)`）；
  文字 = `var(--text)`；**箭头**是内联 SVG、`stroke="currentColor"` ⇒ 随本体文字色（`var(--text)`）。
  hover 时描边抬到 `rgba(var(--btn-line-rgb), var(--btn-line-alpha))`。
- **下拉面板（dropdown）**：底 = **底色**（`var(--surface-glass)`）；描边 = **按钮线条**（`var(--border-glass)`）；
  阴影用底色派生的中性黑 `rgba(var(--shade-rgb), calc(0.4 * var(--shade-scale)))`。
- **选项（option）**：未选中 = 透明底 + 文字 `var(--text-dim)`（文字色相由**按钮蒙版**派生）；
  **悬停 / 选中 = 按钮蒙版** `rgba(var(--other-overlay-rgb), var(--other-overlay-nav-alpha))`（档位同设置侧栏选中项 / 分类标题）。
- **滚动条：与四类无关**（2026-10-11 更正）：本组件列表用 §界面 6 那条**唯一**全局 `::-webkit-scrollbar`
  —— **滑块实心无 border、轨道不描边**，不按容器另写。

### §12.5 明确的反面（硬事实）

- **原生 `<select>` 无法对齐本组件**：其弹出层（选项行 + 右侧滚动条）由 OS / 浏览器绘制，`<option>` **不可样式化**
  ⇒ 连「选项颜色」「弹出层滚动条」都染不了，收合面也**只在收合态**可染。
  **凡要求「与设置里的下拉一致」的场合，必须换成 `.custom-select`** —— 只调收合面永远对不上。

### §12.6 现存原生 `<select>`（历史遗留，豁免）

- `app/src/usage-cost.ts` 的 `.cost-range-select`、`plugins/builtin/translate.ts` 的 `.xl-select`、
  `plugins/builtin/pet.ts` 的**热键行 `group`/`id`/`mode`/`enabled`** 与**动作行** `.pet-mo-row select`。
  它们的**弹出层不受四类控制**；收合面按基准对齐（高 26 / 圆角 6 / 内边距 `5px 8px` / `font-size: .73rem` /
  底 `rgba(var(--shade-rgb), calc(0.25 * var(--shade-scale)))` / 描边 `var(--border-glass)` / 文字 `var(--text)`）。
- **2026-10-11 已改自绘（收编）**：`pet.ts` 的**热键行「类型」「表情对象 target」**与**动作链步骤「动作文件」**
  三处已从原生 `<select>` 换成 `.custom-select`（`customSelectHtml` + `attachCustomSelect`），与设置里的下拉**同源**。
- 另注：`settings.ts` 里的 `.settings-select` 是**死 CSS**（自绘化后遗留、无元素引用），别照它抄。

### §12.7 自查清单（「我的下拉框颜色能不能被用户改成黑色？」）

1. 收合面的底 / 边 / 字是否分别走**按钮背景 / 按钮线条 / `--text`**？
2. 选项的未选中 / 悬停 / 选中是否走**文字派生 / 按钮蒙版**？
3. 滚动条是否只用**全局那一条**（§界面 6），没有按容器另写、也没写 `scrollbar-width` / `scrollbar-color`？
4. 用的是**原生 `<select>`** 吗？是 ⇒ 弹出层不可控，改 `.custom-select`（或确认属 §12.6 豁免）。

---

## §界面 13 数字输入框与上下选择按钮

> 2026-10-11 新增（用户裁定）：`input[type=number]` 右侧那对加减箭头（spinner）此前是 WebView2 默认外观、与主题完全不搭，须按主题重新设计。

- **硬事实（WebView2 / Chromium）**：`::-webkit-inner-spin-button` / `::-webkit-outer-spin-button` 是 **UA 绘制的位图箭头** —— 只能整块隐藏（`-webkit-appearance: none`）**或**保留默认，**无法**换成四类自定义色（设 `background` 只会让箭头消失、留下空壳；尺寸 / 颜色 / hover / active 都改不动）。所以纯 CSS **美化不了**原生 spinner —— 这是与「原生 `<select>` 弹出层不可样式化」（§界面 12）同级的硬约束。
- **现行策略 = 全局隐藏原生 spinner、去控件不去语义**（`styles.css` 里**唯一**一条全局规则，禁止逐个容器声明 —— 与 §界面 6 滚动条同一条纪律）：
  - `input[type="number"] { appearance: textfield; -moz-appearance: textfield; }`
  - `input[type="number"]::-webkit-outer-spin-button, input[type="number"]::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }`
  - 输入框**仍是 `type=number`** ⇒ 键盘 ↑↓ / 滚轮 / `stepUp()` / `stepDown()` 语义全在，只是不再画那对竖箭头（proxy 端口框自 2026-10-06 起就是这个做法，本轮把它提为全局、并顺带去掉了 proxy 里那份重复声明）。
- **需要「看得见、可点、且跟随主题」的加减按钮时 → 改用自绘步进器 `.num-stepper`**（颜色全走四类，造型如下表）：

  | 部件 | 颜色归属 | 造型 |
  |---|---|---|
  | 步进器外壳 `.num-stepper` | 底 = **按钮背景** `rgba(var(--btn-bg-rgb), var(--btn-bg-alpha))`；描边 = **按钮线条** `rgba(var(--btn-line-rgb), var(--btn-line-alpha))`；文字 `var(--text)` | 高 26 / 圆角 6 / 与相邻输入框同高 |
  | `+` / `−` 按钮 `.num-stepper-btn` | 符号色走 **按钮线条** `rgb(var(--btn-line-rgb))`；**hover / active 浮层走按钮蒙版** `rgba(var(--other-overlay-rgb), var(--other-overlay-btn-alpha))`（与按钮 hover 抬档一致） | 24×24 / 圆角 5 / 无独立描边（贴外壳内缘） |
  | 数字输入 `.num-stepper-input` | 无独立背景（透外壳底）；`border: none`；`caret-color: rgb(var(--other-overlay-rgb))`；`text-align: center`；`font-variant-numeric: tabular-nums` | 与外壳同高 |
  | 禁用态 `:disabled` / `.num-stepper.is-disabled` | 整体 `opacity: .45` + `cursor: default`；`+` / `−` 不可点 | — |

  - 参照骨架（实现时照写；颜色只走变量、无字面量）：
    ```html
    <div class="num-stepper">
      <button type="button" class="num-stepper-btn" data-step="-1" aria-label="−">−</button>
      <input class="num-stepper-input" type="number" value="0">
      <button type="button" class="num-stepper-btn" data-step="1" aria-label="+">+</button>
    </div>
    ```
  - **自绘的代价**（为什么本轮不擅自铺开）：要**改 DOM + 加事件接线**（`stepUp()` / `stepDown()`、长按连加、`min` / `max` 夹取、`change` 派发），会动到所属插件的交互与版本。故本轮只把规范定死；落地按需再排。
- **自查清单**：
  1. 我用的是 `input[type=number]` 且指望它长成主题样吗？⇒ 停：原生 spinner 不可美化，要么接受全局隐藏（键盘 / 滚轮步进），要么改自绘 `.num-stepper`。
  2. 我有没有按容器单独写 `::-webkit-*-spin-button`？⇒ 禁止，唯一一条全局规则已覆盖。
  3. 自绘 `+` / `−` 的颜色是否分别走了**按钮线条 / 按钮蒙版 / 按钮背景**？有没有写死色值？

---

## §界面 14 结构代码（可直接照抄 · 形态锁定）

> 用户口径（2026-10-11）：「把规范的**按钮 / 下拉框 / 界面 / 文本框的结构代码**直接写进风格规范、用来固定风格」。
> 本节四个代码块**逐字摘自真实实现**（真相源见每块抬头），**新代码照抄此块；要改形态先改本规范**。
> 摘录只保留「定形态」的那几条（尺寸 / 圆角 / 内边距 / 底色 / 描边 / 状态 / 层级），**颜色一律走四类变量**
> （见 §界面 11），语义色白名单除外；与真实实现不一致即为错。

### §14.1 按钮

**真相源**：标记 = `app/src/index.html` 的 `#chat-send-btn`（纯图标）/ `#chat-add-file-btn`（纯图标）；样式 = `app/src/styles.css` 同名规则、`app/src/plugins/builtin/settings.ts` 的 `.settings-save-btn`、`styles.css` 末尾的全局兜底 `#app button:where(...)`。

标记（照抄；只换 `svg` 路径，详见 §界面 4）：

```html
<button id="chat-send-btn" title="Send" aria-label="Send message">
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
    <g opacity="0.28" transform="translate(-0.7,-0.7)" stroke-width="1.6">
      <line x1="22" y1="2" x2="11" y2="13"/>
      <polygon points="22 2 15 22 11 13 2 9 22 2"/>
    </g>
    <line x1="22" y1="2" x2="11" y2="13"/>
    <polygon points="22 2 15 22 11 13 2 9 22 2"/>
    <circle cx="8" cy="8" r="1" fill="currentColor" stroke="none"/>
    <circle cx="16.2" cy="15.8" r="0.5" fill="currentColor" stroke="none" opacity="0.45"/>
  </svg>
</button>
```

样式（动作按钮 · 纯图标）：

```css
/* 30×30（index.html #chat-send-btn / #chat-stop-btn / #chat-history-btn） */
#chat-send-btn, #chat-stop-btn, #chat-history-btn {
  display: flex; align-items: center; justify-content: center;
  width: 30px; height: 30px;
  background: rgba(var(--btn-bg-rgb), var(--btn-bg-alpha));
  border: 1px solid rgba(var(--btn-line-rgb), var(--btn-line-alpha));
  border-radius: 6px;
  color: rgb(var(--btn-line-rgb));
  cursor: pointer;
}
#chat-send-btn:hover, #chat-stop-btn:hover, #chat-history-btn:hover {
  background: rgba(var(--btn-bg-rgb), calc(var(--btn-bg-alpha) + 0.06));
}
/* 28×28（index.html #chat-add-file-btn / #chat-more-btn / #chat-new-btn）—— 同族，只差尺寸 */
#chat-add-file-btn {
  display: flex; align-items: center; justify-content: center;
  width: 28px; height: 28px;
  background: rgba(var(--btn-bg-rgb), var(--btn-bg-alpha));
  border: 1px solid rgba(var(--btn-line-rgb), var(--btn-line-alpha));
  border-radius: 6px;
  color: rgb(var(--btn-line-rgb));
  cursor: pointer;
}
```

样式（动作按钮 · 文字 —— `hover` / `active` 靠抬 α 表达）：

```css
/* settings.ts .settings-save-btn */
.settings-save-btn {
  padding: 6px 18px; font-size: 0.74rem; border-radius: 6px;
  border: 1px solid rgba(var(--btn-line-rgb), var(--btn-line-alpha));
  background: rgba(var(--btn-bg-rgb), var(--btn-bg-alpha));
  color: var(--text); cursor: pointer;
}
.settings-save-btn:hover  { background: rgba(var(--btn-bg-rgb), calc(var(--btn-bg-alpha) + 0.11)); }
.settings-save-btn:active { background: rgba(var(--btn-bg-rgb), calc(var(--btn-bg-alpha) + 0.21)); }
```

样式（语义 / 状态按钮 —— 语义色**固定、不随主题**）：

```css
/* styles.css .approval-allow / .approval-deny */
.approval-allow { border-color: rgba(157,180,172,0.35); color: var(--green); }
.approval-allow:hover { background: rgba(157,180,172,0.15); }
.approval-deny  { border-color: rgba(192,138,138,0.35); color: var(--red); }
.approval-deny:hover  { background: rgba(192,138,138,0.15); }
```

样式（选中态 / 禁用态）：

```css
/* 选中态（pet.ts .pet-btn-toggle.on）：底色 + 描边走「按钮蒙版」，文字保持 --text（灰阶） */
.pet-btn-toggle.on { background: rgb(var(--other-overlay-rgb)); border-color: rgb(var(--other-overlay-rgb)); color: var(--text); font-weight: 600; }
/* 禁用态（styles.css .music-btn:disabled）：整体降不透明 + 关掉 hover 反馈 */
.music-btn:disabled { opacity: 0.45; cursor: default; }
/* 全局兜底（styles.css 末尾「按钮底色全覆盖」）：所有 <button> 的常驻底色；语义色 / 状态类点名排除；
   hover 抬 α0.06 且排除 :disabled。**完整排除名单以 styles.css 那条规则为准**，此处不复制。 */
#app button:where(/* …排除名单见 styles.css… */) { background: rgba(var(--btn-bg-rgb), var(--btn-bg-alpha)); }
```

- **文字色白名单**：动作按钮**文字走 `--text` / `--text-dim`（灰阶）**、**图标走 `rgb(var(--btn-line-rgb))`**；只有**语义 / 状态按钮**的文字才用 `--green` / `--red` / `--yellow` / `--blue`。

### §14.2 下拉框（`.custom-select`）

**真相源**：标记 = `app/src/plugins/builtin/custom-select.ts` 的 `customSelectHtml()`（**逐字见 §12.2**）；样式 = `app/src/styles.css` 的 `.custom-select` 一节。

- **标记照抄 §12.2**（本节不重复）。
- **样式照抄**（`.custom-select` 一节的定形态部分；开合时的几何由 `custom-select.ts` 的 `positionDropdown()` 写行内 `left` / `width` / `max-height` / `top|bottom`）：

```css
.custom-select { position: relative; flex: 1; max-width: 220px; height: 26px; box-sizing: border-box; display: flex; align-items: center; user-select: none; }
.custom-select-trigger {
  width: 100%; height: 26px; box-sizing: border-box; display: flex; align-items: center; justify-content: space-between;
  gap: 6px; padding: 5px 8px; font-size: 0.73rem; font-family: inherit;
  background: rgba(var(--btn-bg-rgb), var(--btn-bg-alpha));
  border: 1px solid var(--ctx-border-glass); border-radius: 6px; color: var(--text);
  cursor: pointer; outline: none; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}
.custom-select-trigger:hover { border-color: rgba(var(--btn-line-rgb), var(--btn-line-alpha)); }
.custom-select-arrow { flex-shrink: 0; width: 10px; height: 10px; transition: transform 0.15s; }
.custom-select.open .custom-select-arrow { transform: rotate(180deg); }
.custom-select-dropdown {
  position: absolute; top: 100%; left: 0; right: 0; max-height: 240px; overflow-y: auto; overscroll-behavior: contain;
  background: var(--surface-glass); backdrop-filter: blur(20px); -webkit-backdrop-filter: blur(20px);
  border: 1px solid var(--border-glass); border-radius: 6px; z-index: 999; display: none;
  box-shadow: 0 4px 16px rgba(var(--shade-rgb), calc(0.4 * var(--shade-scale)));
}
.custom-select.open .custom-select-dropdown { display: block; }
.custom-select-option { padding: 6px 10px; font-size: 0.73rem; color: var(--text-dim); cursor: pointer; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.custom-select-option:hover { background: rgba(var(--other-overlay-rgb), var(--other-overlay-nav-alpha)); color: var(--text); }
.custom-select-option.selected { background: rgba(var(--other-overlay-rgb), var(--other-overlay-nav-alpha)); color: var(--text); font-weight: 600; }
```

- **滚动条**：走 §界面 6 那条**唯一**全局 `::-webkit-scrollbar`（滑块实心无 border、轨道不描边）；本组件**不**按容器另写，`custom-select.ts` 也**只**打开「长列表滚轮自滚 + 边界翻转」，不写任何滚动条样式。

### §14.3 界面（容器 / 面板 / 弹层 / 菜单）

**真相源**：容器骨架 = `app/src/plugin.html`；面板 = `app/src/plugin-window.ts` 注入的 `.plugin-result` + `app/src/styles.css` 规则；弹层 = `.custom-select-dropdown`（见 §14.2）；菜单 = `app/src/plugins/builtin/pet.ts` 的 `.pet-menu`。

标记（面板骨架 = 容器 + 标题行 + 列表 + **唯一**滚动区；照抄 `plugin.html`）：

```html
<div id="app" class="detached">
  <div id="plugin-titlebar" data-tauri-drag-region>…</div>
  <div id="results-container">          <!-- 玻璃底 + 描边；容器，本身不滚动 -->
    <div id="results-list">             <!-- **唯一**滚动区：overflow-y:auto -->
      <div class="plugin-result">       <!-- 面板内容（plugin-window.ts 注入） -->
        <div class="plugin-result-title">…</div>
        <div class="plugin-result-content">…</div>
      </div>
    </div>
  </div>
</div>
```

样式（容器 / 面板 / 菜单；底色 / 描边 / 圆角 / 内边距 / 阴影 / 层级）：

```css
#results-container {
  position: relative;
  backdrop-filter: blur(24px) saturate(180%); -webkit-backdrop-filter: blur(24px) saturate(180%);
  border: 1px solid var(--border-glass); border-top: none; border-radius: 0; overflow: hidden;
  max-height: var(--results-max-h, none); display: flex; flex-direction: column; pointer-events: auto;
}
#results-container::before { content: ""; position: absolute; inset: 0; z-index: -1; background-color: var(--surface-glass); background-image: var(--glass-sheen-image); }
#results-list { overflow-y: auto; flex: 1; padding: 6px 0; min-height: 0; }   /* 内容**只**在这一层滚 */
.plugin-result { padding: 14px 16px; font-size: 0.88rem; line-height: 1.6; }
.plugin-result-title { font-size: 0.72rem; color: var(--text-dim); font-weight: 600; text-transform: uppercase; letter-spacing: 0.8px; margin-bottom: 10px; opacity: 0.7; }

/* 菜单（pet.ts .pet-menu）：玻璃底 + 描边 + 圆角 + 阴影 + 层级 */
.pet-menu { position: absolute; display: none; flex-direction: column; min-width: 132px; padding: 4px;
  background: var(--surface-glass); border: 1px solid var(--border-glass); border-radius: var(--radius);
  backdrop-filter: blur(20px); -webkit-backdrop-filter: blur(20px); box-shadow: 0 8px 24px rgba(var(--shade-rgb),.28); }
.pet-menu.open { display: flex; }
.pet-menu button { appearance: none; background: none; border: 0; text-align: left; font: inherit; font-size: .72rem; color: var(--text); padding: 6px 10px; border-radius: 6px; cursor: pointer; white-space: nowrap; }
.pet-menu button:hover { background: var(--border-glass); }
```

- **「不许出现嵌套双滚动条」的落点**：容器 `#results-container` 与面板 `.plugin-result` **不设** `overflow-y:auto` —— **滚动只由 `#results-list` 那一层承载**；弹层 `.custom-select-dropdown` 与菜单 `.pet-menu` 是**浮层**、各带自己那一层，且**不叠在滚动区里**。硬禁见 [`forbidden.md`](./forbidden.md) §禁用 2 第 27 条（不得出现第二层滚动条）与 §界面 6。

### §14.4 文本框（`input` / `textarea` / 搜索框）

**真相源**：`app/src/index.html` 的 `#search-input`（搜索框）/ `#chat-input`（AI 输入）；`app/src/styles.css` 同名规则；桌宠 `app/src/plugins/builtin/pet.ts` 的 `.pet-chat-input` / `.pet-persona-text`。

标记：

```html
<!-- 搜索框（index.html）：无独立底 / 无描边，二者由外层 #search-bar 承载 -->
<input id="search-input" />
<!-- AI 输入（index.html）：多行、Enter 发送 -->
<textarea id="chat-input" rows="1" placeholder="Type a message..." autocomplete="off" spellcheck="false"></textarea>
<!-- 桌宠输入（pet.ts）：单行、Enter 发送、Shift+Enter 换行 -->
<textarea class="pet-chat-input" id="pet-chat-input" rows="1" spellcheck="false" autocomplete="off" placeholder="…" aria-label="…"></textarea>
```

样式（底 / 描边 / 圆角 / 内边距 / 聚焦态 / placeholder / 字数限制）：

```css
/* 搜索框：无独立底、无描边（底与描边由 #search-bar 承载） */
#search-input { width: 100%; min-width: 0; background: none; border: none; color: var(--text); font-family: inherit; font-size: 1.05rem; line-height: 1.3; outline: none; caret-color: rgb(var(--other-overlay-rgb)); resize: none; }
#search-input::placeholder { color: var(--text-dim); opacity: 0.45; }

/* AI 输入（多行）：底走反差四件套、聚焦把描边换成按钮线条 α 档 */
#chat-input { flex: 1; background: rgba(var(--ctx-ink-rgb), 0.04); border: 1px solid var(--ctx-border-glass); border-radius: 8px; padding: 6px 10px; color: var(--text); font-family: inherit; font-size: 0.82rem; line-height: 1.3; outline: none; caret-color: rgb(var(--other-overlay-rgb)); resize: none; min-width: 0; min-height: 30px; max-height: 60px; overflow-y: auto; }
#chat-input:focus { border-color: rgba(var(--btn-line-rgb), var(--btn-line-alpha)); }
#chat-input::placeholder { color: var(--text-dim); opacity: 0.4; }

/* 桌宠单行输入（pet.ts .pet-chat-input）：底走底色（--surface-glass） */
.pet-chat-input { box-sizing: border-box; flex: 1 1 auto; height: 35px; resize: none; padding: 6px 10px; font: inherit; font-size: .72rem; line-height: 1.5; color: var(--text); background: var(--surface-glass); border: 1px solid var(--border-glass); border-radius: var(--radius); outline: none; caret-color: rgb(var(--other-overlay-rgb)); }
.pet-chat-input:focus { border-color: rgba(var(--btn-line-rgb), var(--btn-line-alpha)); }

/* 桌宠多行文本（pet.ts .pet-persona-text）：底走底色派生的中性黑 */
.pet-persona-text { width: 100%; box-sizing: border-box; resize: vertical; min-height: 96px; font: inherit; font-size: .72rem; line-height: 1.7; color: var(--text); background: rgba(var(--shade-rgb), calc(0.3 * var(--shade-scale))); border: 1px solid var(--border-glass); border-radius: 8px; padding: 8px 10px; outline: none; caret-color: rgb(var(--other-overlay-rgb)); }
```

- **字数限制的写法**：用 `maxlength` 属性（值来自**宿主那一份上限**的唯一真相源 —— 如 `storage::MAX_PERSONA_CHARS` / `get_pet_persona` 回的 `maxChars`），JS 侧**只做回显**（`ta.maxLength = …`），**不另立一份上限**。示例：`pet.ts` 的 `<textarea … maxlength="8000">` + `if (st?.maxChars) ta.maxLength = st.maxChars;`。
- **焦点 / 输入**：`caret-color` 一律走 `rgb(var(--other-overlay-rgb))`（见 §界面 2 / §界面 7）；文本输入遵守「失焦 / 回车才提交」。

---


> **历史细节见 `docs/archive/agent-ui-spec.md`、`docs/archive/icon-style.md`、`docs/archive/ai-spec.md` §11 规则 9 / 21 / 44–51。**

---

## 本轮改动记录（2026-10-11 · 第 3 轮）

- **§界面 6 滚动条（修正）**：~~border 由「滑块（`-webkit-scrollbar-thumb`）」移到「轨道（`-webkit-scrollbar-track`）」，颜色走**按钮线条** `rgba(var(--btn-line-rgb), var(--btn-line-alpha))`~~ —— **本条已被 2026-10-11 第 5 轮撤销**（用户裁定「轨道描边」是误判）：滚动条回归「**滑块实心无 border、轨道不描边**」，见本文件末「第 5 轮」记录。
- **range 滑块（复查）**：上一轮**未**给任何 range 滑块加 border；`.music-vol` / `.pet-range` 滑块本就 `border: none`（实心 `rgb(var(--btn-line-rgb))`）。轨道 border 不套用（原生 range 不重画 / 自绘 range 轨道仅 3–4px 高），理由见 §界面 6。
- **§界面 12 下拉框（修正 + 补录）**：~~右侧滚动条 border 归**轨道**~~ —— **该句已被 2026-10-11 第 5 轮撤销**（滚动条不再关联四类颜色）；pet 三处原生 `<select>` 收合面对齐基准（弹出层仍豁免）这一条仍成立。
- **§界面 13（新增）**：数字输入框与上下选择按钮；原生 spinner 不可美化 ⇒ 全局隐藏 + 自绘 `.num-stepper` 规范。代码 = `app/src/styles.css` 顶部新增的全局 `input[type="number"]` 规则（并移除 `.proxy-panel` 里重复的那份）。
- **pet 插件**：`pet.ts` 的 `PET_CSS` 加了下拉收合面规则；`version` `0.9.49 → 0.9.50` 并重打包（`scripts/build-plugins.ps1 -Plugin pet`）。

---

## 本轮改动记录（2026-10-11 · 第 4 轮：音乐插件三处收编）

> 用户口径（2026-10-11）：音乐插件里**进度条 / 随机播放按钮线条 / 选中歌曲的选中框**三处要跟随四类自定义颜色（此前被列进白名单保持 `--accent` / `--yellow`，现移出）。代码 = `app/src/styles.css`（进度条与按钮样式全在全局样式表；`music.ts` 只出标记）。

- **① 播放 / 扫描进度条**：填充归**按钮线条**、轨道归**底色**派生中性黑。原→新：
  - `.music-bp-fill`（在线 / 本地 / 小窗三条 seek 线的填充）、`.music-progress-fill`（本地播放条填充）：`background: var(--accent)` → **`rgb(var(--btn-line-rgb))`**。
  - `.music-scan-fill`（扫描填充）：`background: rgb(var(--other-overlay-rgb))` → **`rgb(var(--btn-line-rgb))`**。
  - `.music-bp::before`（seek 轨道）：`rgba(var(--ink-rgb), 0.14)` → **`rgba(var(--shade-rgb), calc(0.14 * var(--shade-scale)))`**；`.music-scan-bar`（扫描轨道）：`rgba(var(--ink-rgb), 0.1)` → **`rgba(var(--shade-rgb), calc(0.1 * var(--shade-scale)))`**；`.music-progress`（本地条轨道）本就是 `rgba(var(--shade-rgb), calc(0.3 * var(--shade-scale)))`（**轨道统一走底色**，理由 = 该轨道是本次「既有实现」的基准、且与 `.music-vol` 轨道同源）。`.music-progress.music-dragging` 的拖动态底色 `rgba(var(--ink-rgb), 0.14)` 一并对齐为同一条 `--shade-rgb`。
  - `.music-progress-dot`（本地条拉条圆点 / 滑块）：`background: var(--accent)` → **`rgb(var(--btn-line-rgb))`**（拉条滑块归按钮线条，与 `.music-vol` 拇指同族）。
- **② 播放模式 / 随机播放按钮**：线条归**按钮线条**、激活态归**按钮蒙版**。原→新：
  - `.music-mode-btn`（含 `#music-mode` / `#music-b-mode` / `#music-l-mode` / `#music-lp-mode`）：off 态描边此前继承 `.music-round-btn` 的 `--border-glass`（= 按钮线条 α 固定 `0.10`），现显式写 **`border-color: rgba(var(--btn-line-rgb), var(--btn-line-alpha))`**（α 走统一 token `0.32`）。
  - `.music-mode-btn[data-mode="shuffle"|"repeat_all"|"repeat_one"]`：激活态文字 `color: var(--accent)` → **`color: rgb(var(--other-overlay-rgb))`**（描边保持 `--btn-line-*`）。
  - **常驻底无需改**：由全局 `#app button:where(...)` 统一给**按钮背景** `rgba(var(--btn-bg-rgb), var(--btn-bg-alpha))`，本就是 `--btn-bg-*`。
- **③ 选中歌曲的选中框**：选中框属**选中态** ⇒ 整圈色框 + 淡填充归**按钮蒙版**。原→新（两处同套语言）：
  - `.music-tr.sel`（在线歌单 / 队列详情页选中行）、`.music-mt.sel`（本地媒体库选中行）：`background: rgba(201,184,150,0.14)` + `box-shadow: inset 0 0 0 2px var(--yellow)` → **`background: rgba(var(--other-overlay-rgb), 0.14)` + `box-shadow: inset 0 0 0 2px rgb(var(--other-overlay-rgb))`**。
  - **与「正在播放」的可区分性**：`playing` 用**文字强调**（`.music-tr.playing` / `.music-mt.playing` / `.music-pl.playing .music-pl-name` = `rgb(var(--other-overlay-rgb))` 衬在歌名上），选中用**2px 外框**（同色相、形态不同），且 `.sel` 规则**不写 `color`**，故在放那首被同时选中时文字强调照样透出 —— 两者可同时存在且一眼可分。
- **music 插件版本**：`version` `0.9.35 → 0.9.36` 并重打包（`scripts/build-plugins.ps1 -Plugin music`；脚本保持 UTF-8 with BOM）。
- **白名单**：删去「播放 / 扫描进度条（`.music-progress-fill` / `.music-scan-fill`）」整条；**保留**「正在播放文字高亮」（并注明进度条与选中框已收编）；`.music-pl.sel`（左栏编辑态勾中行，`--accent` 底 + 左缘）本轮未点名收编、仍留白名单。

---

## 本轮改动记录（2026-10-11 · 第 5 轮：撤销滚动条轨道 border + 桌宠下拉点名 + 动作链折叠）

> 用户口径（2026-10-11）：① 滚动条「轨道描边」是**误判**、撤销；② 桌宠的动作 / 热键选择框**也算下拉框**、对准主题；③ 桌宠插件 5 项功能（动作链折叠 / 换动作自动填时长 / 动作行一行四控件 / 整链循环 / 热键页去框格 + 表情对象可选）。代码 = `app/src/styles.css`（滚动条）、`app/src/plugins/builtin/pet.ts` / `pet-chains.ts` / `pet-hotkeys.ts` / `live2d-engine.ts` / `live2d.ts`。

- **§界面 6 滚动条（撤销，见本轮记录顶）**：`::-webkit-scrollbar-track` 的 border **删除**（`border: none`）；滚动条回归「**滑块实心无 border、轨道不描边**」。§界面 11 的按钮线条职责表已删「滚动条轨道描边」；§界面 12 已改「滚动条不再关联四类颜色」。上方「第 3 轮」那两条相应记录已加**撤销**标注。
- **§界面 12 pet 下拉点名**：pet 的三处原生 `<select>`（热键行 `.pet-hk-row select` / 动作行 `.pet-mo-row select` / 动作链步骤 `.pet-chain-file`）**收合面按基准对齐**已复核**未被压掉**（专用规则与共享 `input, select` 同特异度、源码在后胜出）；本轮把专用规则补成**自洽**（显式写 `border`）。弹出层仍不可控（豁免、不改自绘）。
- **§界面 6 折叠 / 展开（新增通用纪律）**：见上（折叠态一行摘要 + 过长省略；线性 SVG 箭头；缺省展开）。
- **pet 版本**：`pet 0.9.50 → 0.9.51` 并重打包（`scripts/build-plugins.ps1 -Plugin pet`；脚本保持 UTF-8 with BOM，字节级校验前 3 字节 `EF BB BF` 恰一份）。

---

## 本轮改动记录（2026-10-11 · 第 6 轮：下拉框组件化 + 桌宠三处收编 + 去掉录制）

> 用户口径（2026-10-11）：桌宠下拉必须参照「设置 → AI → 提供商」的下拉（已要求三次未成；根因 = 规范没写参照组件本身 + 桌宠那几处是原生 `<select>`）。代码 = `app/src/plugins/builtin/custom-select.ts`（**新增**）、`settings.ts`、`pet.ts`、`app/src/styles.css`。

- **§界面 12 重写**：把参照组件**本身**逐字写进规范（组件名 / 定位、完整 markup、行为契约、四类颜色归属、与滚动条关系、明确反面），并如实标注「键盘 `↑↓`/`Enter`/`Esc` **未实现**、当前只有鼠标操作」。见 §12.1–§12.7。
- **组件化（任务 B）**：`.custom-select` 的 CSS 从 `settings.ts` 内联样式**抽到共享的 `app/src/styles.css`**（已核实插件悬浮窗 `plugin.html` 链接同一份 `styles.css` ⇒ 插件窗拿得到）；交互抽成可复用模块 `custom-select.ts`（`customSelectHtml` / `attachCustomSelect` / `closeAllCustomSelects`），`settings.ts` 与 `pet.ts` 共用。settings 的 × 删除、`__custom__` 内联输入、面板裁剪放宽三项**行为保持不变**（只改接线方式）。
- **桌宠三处改自绘（任务 C）**：热键行「类型」、热键行「表情对象 target」、动作链步骤「动作文件」由原生 `<select>` 换成 `.custom-select`；选定后**写回原状态并走原落盘路径**（热键 `scheduleSaveHotkeys` 防抖落盘、动作链 `commitChainFile` → `persistChains`）。热键行其余下拉（group / id / mode / enabled）与动作行仍为原生 `<select>`（豁免）。
- **去掉录制按钮 + 当前按键文本（任务 D）**：删 `.pet-hk-rec` 按钮、`.pet-hk-keytext` 文本及其 CSS，去掉 `hkCapturing`、录键 `keydown` 监听与 `[data-hk-keys]` 点击分支。**按键绑定数据照旧**（`lunac.hotkeys.json` 的 `keys` 仍由 `assignDefaultKeys` 补默认并落盘，供 AI 读写）；**UI 不再有改按键绑定的入口**（有意：数据驱动、由 AI 改）。热键行的**手工编辑落盘**（名称 / 类型 / 目标 / 删除 → `scheduleSaveHotkeys`）仍健在。
- **pet 版本**：`pet 0.9.51 → 0.9.52` 并重打包（`scripts/build-plugins.ps1 -Plugin pet`；脚本保持 UTF-8 with BOM，字节级校验前 3 字节 `EF BB BF` 恰一份）。

---

## 本轮改动记录（2026-10-11 · 第 7 轮：结构代码入规范 + 桌宠输出框两处修）

> 用户口径（2026-10-11）：① 「把规范的**按钮 / 下拉框 / 界面 / 文本框的结构代码**直接写进风格规范、用来固定风格」；② 桌宠「输出框」两处 bug（**该自动弹出却没弹**；文本**位于正中间 / 不从开头渲染**）。代码 = `docs/ui-style.md`（新增 §界面 14）、`app/src/plugins/builtin/pet.ts`。

- **§界面 14（新增）**：把四类控件的**结构代码**（HTML 标记 + CSS）逐字摘自真实实现写进规范，形态锁死 —— §14.1 按钮（动作按钮纯图标 28/30、动作按钮文字、语义按钮、选中 / 禁用、全局兜底）、§14.2 下拉框（`.custom-select`，标记照 §12.2、CSS 逐条照抄）、§14.3 界面（容器 / 面板 / 弹层 / 菜单，含「不许嵌套双滚动条」的落点）、§14.4 文本框（搜索框 / AI 输入 / 桌宠输入）。每块标注**真相源**（文件 + 类名）并写明「**新代码照抄此块；要改形态先改本规范**」。
- **pet 输出框 · 问题 2（渲染形态）**：`.pet-chat-output` 由固定 `height:75px` 改成 **`max-height:75px`**（高度按内容收缩、短回复不再上下留大片空白）+ 显式 **`text-align:left`**（从顶部渲染）；`setChatText` 去掉正文**末尾换行 / 空白**（`white-space:pre-wrap` 会把它们渲染成盒内真实空行 = 「文本结束后还留一大块空白」）；滚动条继续走 §界面 6 那条**唯一**全局规则（未按容器另写）。
- **pet 输出框 · 问题 1（自动弹出）**：此前只在最终回复（`pet-chat-reply`）到达那一刻弹一次（整轮期间什么都不显示、且该轮没产生最终回复时永不弹）⇒ 改到 **`sendToAI`**：桌宠发起且在没有开着输入对话时 **`showOutputBox(false)` 立刻弹**（显示「思考中」），最终回复到达再 **`showOutputBox(true)`** 刷新并进入 6s 自动收起；`flashOutputOnly` 改名 `showOutputBox(autoHide)`。另把该最终回复事件由**裸 `listen`** 改为 **`getCurrentWindow().listen`**（**定向**事件纪律，与 MOUTH_EVENT 同）。
- **如实记（与真实实现对照）**：现实现里输出框**本来就贴顶、没有 flex 垂直居中**（`.pet-top` 是 `align-items:flex-start`），故「居中」这一症状在代码里不成立；实际可疑声明是「固定 `height:75px` 未按内容收缩」。抓屏路径依赖「图片输入 / vision」开启，关着时**不会**发送（`sendScreenToAI` 只落一条 warn），自然也不弹框。
- **临时诊断打点**：`pet-chat-reply` 处理器里加了 `log_frontend` 的 `[pet] output reply len=… visible=… chatOpen=… auto=…`（**临时，查清后删**）。
- **纪律自查发现的既有不一致（已登记）**：① `styles.css` 的 `#chat-node-rail` 同时写了 `scrollbar-width: none`（第 4135 行）与 `::-webkit-scrollbar { display: none }` —— 而 §界面 6 把「禁止写 `scrollbar-width` / `scrollbar-color`（任何地方）」写死、唯一例外只提到后者；② 桌宠菜单项 `.pet-menu button:hover` 的 hover 底色走 **按钮线条**（`var(--border-glass)`）而**不在** §界面 7 列的「按钮蒙版四处」里（未登记）。
- **pet 版本**：`pet 0.9.52 → 0.9.53` 并重打包（`scripts/build-plugins.ps1 -Plugin pet` exit 0；脚本保持 UTF-8 with BOM，字节级校验前 3 字节 `EF BB BF` 恰一份）。`npx tsc --noEmit` exit 0。

---

## 本轮改动记录（2026-10-11 · 第 8 轮：斜杠命令的界面复用）

> 用户口径（2026-10-11）：A13「斜杠命令」子集（`/rewind` + memory 类）。代码 = `app/src/main.ts`、`app/src/i18n.ts`、`app/src-tauri/src/mcp_server.rs` / `commands.rs` / `main.rs`。**本轮不新增任何样式**，只登记「斜杠命令的界面落点复用哪几族既有形态」。

- **命令旁注** = `.sys-note sys-note-line`（一行系统旁注，与「上下文压缩提示」同族；**独立节点、不进 `textSegs`/`chatHistory`**）。
- **记忆查看** = `<details class="sys-note">` + `pre.sys-note-body`（可展开；与 hooks / 附件提示同族，避免 6000 字铺屏）。
- **破坏性命令的二次确认条** = `.run-mode-confirm`（`msg` / `ok` / `cancel`；与「切自动档」同款，`ok` 走语义红 —— 属 §界面 11 白名单语义色）。**据此确认**：这两族既有样式已够，斜杠命令**未新增任何颜色字面量、未新增滚动条、未新增容器**。
- **纪律**：命令文案全走 `t()`（`chat.slash_*`，五语言）；命令**不进模型**、不写会话历史（旁注节点纪律同「上下文压缩提示」）。
