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
  - 语义：`--accent` + `--accent-bg` + `--accent-border`（进行中 / 可交互）、`--green`（成功 / 放行）、`--danger-rgb`（危险 / 失败 / 拒绝 / 告警红，**唯一数值来源**；`--danger` / `--red` 是它的消费名，见 §界面 16）、`--yellow`（警告 / 非零退出码）、`--blue`（信息 / 链接）；
  - 圆角：`--radius`（默认 8）。
- **默认主题色板（2026-10-18 定稿，中性石墨 · 印象化）**：`--surface-rgb` `22, 24, 27` / hover `42, 45, 49`（hover = 常态 +20/通道，由主题 `surface` token 自动派生）；`--accent-rgb` `207, 200, 187`（#cfc8bb 香槟银）；`--text` `#f2f1ee` / `--text-dim` `#c2c3c4` / `--text-muted` `#8b8e92`。**页面配色不取自参考图**（图只供 icon 笔触与动效气质），主题包 `themes/default/theme.json` 同步声明 `accent` / `surface` / 三档文字；`surface` 必须显式写 —— 否则底色会由 accent 的暖色相派生。
- **动效 token**（2026-10-18）：`--ease-out`（常规）、`--ease-collapse`（收拢）、`--motion-fast` `160ms` / `--motion-base` `280ms` / `--motion-slow` `520ms`。组件里不得再写时长 / 曲线魔法数字，见 §界面 18。
- **派生 token（主题色唯一真相源）**：
  - `--accent-rgb`：凡 accent 带 alpha 必须写 `rgba(var(--accent-rgb), x)`，**不得**硬编码 `rgba(192,160,160,x)` 这类分量；
  - `--ink-rgb`：中性叠加基色 —— `rgba(255,255,255,α)` → `rgba(var(--ink-rgb), α)`。**固定 `255,255,255`（2026-10-17 起不再随蒙版派生）**，见 §界面 11。
  - `--shade-scale` + `--shade-rgb`：凹陷强度与基色 —— `rgba(0,0,0,α)` → `rgba(var(--shade-rgb), calc(α * var(--shade-scale)))`。**2026-10-11 起 `--shade-rgb` 由「底色（base）」的暗变体派生、`--shade-scale` 由底色明暗定**（暗底 1 / 亮底 0.35），见 §界面 11。
  - **四类自定义颜色**（`--surface-*` 底色 / `--btn-line-*` 按钮线条 / `--btn-bg-*` 按钮背景 / `--other-overlay-*` **按钮蒙版**（旧称「其他颜色」，2026-10-11 改名））：**凡按钮 / 拉条 / 文本框 / 选中态 / 图标 / 描边 / 中性底 / 下拉框（本体 + 可染色的收合面 / 选项，见 §界面 12）的颜色一律走这四类**，不得再写死色值。**注**：三档文字是 `:root` 固定值，不随四类漂移（见 §界面 11）。**注（2026-10-11 更正）**：滚动条**不在**此列（轨道不描边、滑块实心无 border，与四类无关，见 §界面 6）。
- **默认观感可变但必须可读**（2026-10-11 取代旧「默认逐像素不变」）：四类未自定义时**回落主题色 / 主题派生**，观感允许与改造前不同，但**文字对比度不得崩**（默认实测 ≈ 15.9:1）。
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
- **两态开关（开 / 关）统一用 `.settings-toggle`**（2026-10-24 收编）：设置面板与输入栏「更多设置」里的所有两态开关（思考 / 阶段状态栏 / 本地模型决策）**必须共用这一套**，不得再写 `.chat-more-seg` 那种文字按钮，也不得在插件内联 `<style>` 里复制一份。
  - 样式唯一真相源 = `app/src/styles.css` 的 `.settings-toggle*`；轨道 / 描边走 `--btn-line-*`（关 0.31×alpha、开 0.44×alpha），圆点走主题色，拖动「线条透明度」时三层同步缩放。
  - 标记固定为 `<label class="settings-toggle"><input type="checkbox"><span class="settings-toggle-slider"></span></label>`；`aria-label` / `title` 写当前动作，点击要 `stopPropagation()`（菜单里的开关不能把 ⋯ 菜单关掉）。
  - 三档及以上（权限 / 运行方式）仍用点阵或循环按钮，**不要**把多态硬塞成开关。

```html
<label class="settings-toggle">
  <input type="checkbox" id="chat-mode-toggle" checked>
  <span class="settings-toggle-slider"></span>
</label>
```

## §界面 3 圆角 / 间距 / 边框

- 圆角一律 `var(--radius)`（默认 8；搜索 / 结果区可各取 `--radius-search` / `--radius-results`，默认都等于 `--radius`）。小控件（结果行 / 图标底 / 徽标）沿用既有 4 / 8px，弹层上限 12px，**不新造大圆角**。
- 间距按既有 **4 / 6 / 8 / 12px** 体系，**不新造尺度**。
- 文案按钮 = 图标（≤14px）+ 文字，间距 4–6px；纯图标按钮需 `title` / `aria-label`。
- **动作按钮默认「图标 + 文字」（2026-10-18 用户裁定）**：新增 / 改造按钮时优先组合式，**尽量少用仅文字按钮**；仅文字只保留在空间极窄的纯导航 tab、行内链接与确实无合适图形语义的场景。图标不是装饰，必须能表意（搜索 / 打开 / 返回 / 收藏 / 导入 / 删除 / 关闭）。
- **主题化**：图标颜色只允许 `currentColor`（随按钮文字 / hover 态），不得写死 `#rrggbb` / `rgb()`；图标不单独换色表达选中，选中态仍由按钮底色 / 描边承担（见 §界面 2）。

## §界面 4 图标（严格线性 SVG）

- **组合优先**：凡是动作按钮，结构统一为「线性 SVG + `<span>` 文字」；图标 `aria-hidden="true"`，文字保留可读性。仅文字按钮属于例外，需在评审时说明原因。
- **画布** `viewBox="0 0 24 24"`；**显示尺寸**按钮内 `width="12" height="12"`（小按钮）/ 行内 12–14px。
- `fill="none"`、`stroke="currentColor"`（跟随按钮文字色，默认 `--text-dim`，hover 变亮）、`stroke-width="2.2"`（残影 / 装饰可 1.6~1.8）、`stroke-linecap="round" stroke-linejoin="round"`。
- **线宽按「渲染像素」折算，不按 viewBox 单位**（2026-10-18 澄清）：图标 `viewBox="0 0 24 24"`、按钮内显示 12px ⇒ `stroke-width="2.2"` 实际约 **1.1px**，正落在新主题要求的 1.25~1.5px 细线档位附近。**不得**把 `stroke-width` 直接改成 1.25 / 1.5 —— 那会渲染成 0.6~0.75px，WebView2 下出现明显锯齿与发虚。
- **残影 / 点彩语言**：一个同形副本偏移 `-0.6~-0.7`、`opacity 0.28`、更细笔画，或一个点彩圆点 `r≈0.9`（高光）。
- **复用优先**：`COPY_SVG` / `RETRY_SVG` / `ROLLBACK_SVG` / `pluginIconSvg()` / `THINK_SVG` 已有路径，不重复造；新图形命名 `*_SVG`。
- **功能按钮禁用 emoji**（见下）。**深色 + 毛玻璃**：图标一律 `currentColor`，不额外换彩色图标表达 hover / 选中。
- **大图标**（插件入口、结果区主图标）走 `app/src/main.ts` 的 `pluginIconSvg()`。
- **未知插件 / AI 生成插件的兜底（2026-10-21）**：不再回退 emoji `🔧`，改用 `DEFAULT_PLUGIN_ICON`
  （插件包 + 星光，走同一个包装：残影 + 点彩）。设置面板的本地插件、市场索引、工具链三处图标
  也统一走 `window.__lunac_plugin_icon(id)`，清单 `icon` 为空时不再显示空白方块。
  设计说明见 [`theme-palette-icon-design.md`](./theme-palette-icon-design.md) §2.2 / §2.4。
- **印象化光晕（2026-10-18）**：结果行图标在 hover / 选中时加 `drop-shadow(0 0 2px rgba(var(--ink-rgb), 0.16))`，容器底色抬到 10% 同色低透明填充；不重绘路径、不做反白色块、不为动画加模糊滤镜。

## §界面 5 emoji 纪律

- **UI 文案里永远不得内嵌 emoji 当图标**：按钮标签、状态行 / 提示行、占位文案一律「**纯文字**」或「**线性 SVG + 文字**」。`📋 复制结果`、`📁 选择文件`、`🖼️ 暂无图片`、`⚠️ …`、`⏳ …`、`✅ 完成`、`❌ 失败` 全部不合格。
- **判据（一句话）**：这个字符是「**一条可点控件 / 一句提示的文案的一部分**」（→ 禁止），还是「**一个条目 / 一条状态的标记**」（→ 允许）？带 `U+FE0F`（变体选择符，强制彩色）的**按 emoji 算**（`⚠️` 禁止、`⚠` 允许）。
- **允许且必须保留**（不属于本条约束）：
  - **列表项 / 条目图标**：结果区行图标、目录数据自带的 `item.icon`、`plugin.icon`（未知插件已改为内联 SVG 兜底，2026-10-21）、`.clip-item-icon`、`.file-chip-icon`、web-search 引擎图标；
  - **单色文字状态符号**：`✓ ✗ ⚠ ↔ ↩ ◐ ○ ✔ ✕ × ＋`（表达状态，如 `✓ 已复制` / `✗ 失败`）。
  - **例外中的例外**：**任务抽屉 `#todo-drawer` 不出现任何 emoji / 字符图标**。2026-10-22 用户要求「每条待办左侧显示是否完成」⇒ 用**纯 CSS 方块**（空心 = 未完成、虚线 = 进行中、实心 = 已完成），它不是图标、不依赖字体。
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
  - **批量按钮（全部允许 / 全部拒绝）只在「普通多命令审批卡」上出现**（2026-11-01 用户要求）：计划卡（`ExitPlanMode`）与 AskUserQuestion 卡（AI 请求用户就任务方向 / 分歧做决定）**都不出现**；一批里只要混有一张这类卡，批量按钮**整体收起**（不单藏那几行）。原因：AskUserQuestion 的「允许」= 提交答案，批量放行会回空 `updatedInput`，agent 侧拿不到 `answers` 只能报错。
- **折叠 / 展开（2026-10-11 通用纪律）**：「行多 / 字段多」的可编辑块**允许折叠**：**折叠态只显示一行摘要**（关键字段用 ` · ` 拼接）、**过长一律省略**（`white-space:nowrap; overflow:hidden; text-overflow:ellipsis`），**展开态**显示全部字段。折叠开关用**线性 SVG 箭头**（`▸/▾` 语义、`currentColor`、展开时旋转 90°、带 `aria-expanded`），**不写字符**；**缺省展开**（不隐藏既有功能）。首次落地 = 桌宠「动作链」（`pet.ts` 的 `.pet-chain-toggle` / `.pet-chain-summary`，折叠态摘要 = 每步「文件 · 循环 · 硬停」的拼接）。

## §界面 7 选中态 / 禁用态 / hover / focus

- **hover / 选中态靠按钮自身背景色变化表达**，不额外换用彩色图标。
- **「按钮蒙版」（旧称「其他颜色」，2026-10-11 改名）只有一组控件**（整块里**恰好**一个取色器 `#ap-ooverlay` + **一条**透明度滑块 `overlayAlpha`），驱动**四处浮层**：
  1. 结果项 `.result-item:hover` / `.selected`；
  2. 设置侧栏选中项 `.settings-sidebar-item.active`；
  3. 分类标题 `.settings-group-title`；
  4. **顶栏图标按钮的 hover 浮层**（`#settings-btn` / `#plugin-bar-exit` / 大窗口 `#detached-*`）。
  - **不得**把「运行端输出文字 / 对话背景 / 设置面板背景」加回来（三项各回主题）。
  - 默认配置下 `<html>` 的 `style` 上不应出现 `--other-overlay-*`（颜色空串 ⇒ 清掉内联值）；四处浮层此时分别是 `rgba(207,200,187,0.1)` / `(…,0.14)` / `(…,0.14)` / `(…,0.2)`。
- **取色器是应用内自绘**：色号框在最左 + 色块 + 30×30 圆角方展开按钮，面板整宽、SV 高 40px；**不得**退回系统 `<input type=color>` 或色轮。
- **分组小标题**：放大 + 600 字重 + accent 竖条 + 淡底框。
- **焦点 / 输入**：`caret-color: var(--accent)`（灰阶文字的例外之一）；文本输入一律「失焦 / 回车才提交」，编辑期间周期渲染要跳过（焦点守卫）。
- **禁用态**：置灰 + 不可点；注意 **`.settings-save-msg` 默认 `opacity:0`** —— 失败文案会「有高度、看不见」，看着像空白占位，状态要直接写进那行提示。

## §界面 8 主题色明暗与主题包

- **不做浅色主题**（见 §界面 1），但 **token 必须能跟随底色明暗**（2026-10-17 重写）：`--shade-rgb` / `--shade-scale` 由**底色**派生（暗底 `1`、亮底 `0.35`）；**三档文字与 `--ink-rgb` 是 `:root` 固定值**，不随主题色 / 蒙版 / 底色漂移（主题包显式 text token 可覆盖，见 §界面 11）。
- **主题色只能有一个真相源 `--accent-rgb`**；改主色为 `#3a7bd5` 后，相关元素的 `border-color` / `background-color` / `color` 三处都要跟着变。
- **主题包锁定态**：`themeId !== "default"` 时 `#ap-color-group` 必须带 `.locked`（灰掉 + 不可点）、`#ap-bg-pick` 必须 `disabled`、`#ap-lock-note` 必须可见；切换主题时**实时同步**；「自定义」那五个**玻璃质感拉条在任何主题下都必须还能拖**。
- **主题列表是「泡泡框」**：每个主题文件夹一个 `.ap-theme` 胶囊泡泡 = 主色圆点 `.ap-theme-dot`（取 `manifest.tokens.accent`）+ 名称 + 内置徽标，`title` 是文件夹名；取数必须仍是 `themes()`（Rust `list_themes()` 扫文件夹）。
- **主题图标只走 `pluginIconSvg()` 一个出口**；设置面板只调 `window.__lunac_appearance`，不自己写 localStorage。

## §界面 9 视窗与布局硬边界

- **窗口宽度 `WIN_WIDTH = 800`**；透明 / 无边框 / 毛玻璃形态、`#search-bar → #results-container → #status-bar` 纵向结构**不变**。
- **搜索栏不设装饰层（2026-10-18 定）**：曾试验的蝴蝶饰带 `#search-bar-filigree` 已整体删除（`styles.css` 规则同步移除，`#search-input-area` 的 `z-index` 恢复默认）。搜索栏内只允许输入区与功能按钮，**不得**再叠加装饰性 SVG / 固定层。
- **AI 态 / 插件态高度**：embedded 360 / detached 600 / OCR detached 520（× `currentZoom`），**离散直设、不走滑动动画**；详细搜索大界面是并列固定档 `640 × zoom`。**不得**新增 `setSize`、不得把插件态拉进高度滑动动画。
- **详细搜索大界面的打开是硬切（2026-10-28 用户口径：双击搜索栏的打开动作不要动画）**：`enterDetail()` 进入前先 `stopResizeAnim()` 掐掉在途的搜索态滑动，`requestWindowHeight()` 在 `pluginActive || detailOpen` 时也只走直设 —— 只靠动画 tick 里的 `stopResizeAnim()` 要等下一帧，会看到「先滑一段再跳」。
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
- **简洁搜索结果区的行只留「图标 + 标题」，不渲染 `.result-item-desc`**（2026-11-02 用户定；同日**回改**其中三行）：
  键入文本后，应用 / 插件 / OCR / 文件转换 / 自定义启动 / 标识直达等行只有标题；
  **例外——AI 助手 / 备忘录 / 网页搜索** 三行保留副标题，用来显示**当前键入的检测内容**：
  AI 助手与备忘录显示原内容，网页搜索固定为 **`搜索引擎:内容`**（如 `Google: 财报`）。
  行高仍是 **56px**（36px 图标 + 20px 上下 padding）。
  **另一处例外**：详细搜索面板的行标签（`#detail-results`）与 `quick-launch` 面板的路径副标题不属于本区，保留。
- **网页搜索的右键引擎菜单只显示引擎名文字**（2026-11-02 用户定）：不得再带 emoji / 图标
  （`SEARCH_ENGINES` 已去掉 `icon` 字段）。

---

## §界面 11 自定义颜色体系（四类）

- **一件事**：用户在「设置 → 风格」里能改**四类颜色**（底色 / 按钮线条 / 按钮背景 / 按钮蒙版），经 `main.ts` 的 `applyAppearance()` → `applyFourClassNeutrals()` 合成到 `<html>` 行内 CSS 变量上。**这四类是整个 UI 颜色的唯一来源**：四类之外，除白名单语义色与**固定三档文字**外，界面上不得再有任何颜色（含**图标 / 描边 / 中性底**）。
- **硬验收判据（2026-10-17）**：把四类**全设 `#000000`**（四个取色器都写成真 hex），逐界面扫（搜索栏 / 结果区 / AI 输入栏 / 设置各页 / 各插件面板）**按钮 / 拉条 / 文本框 / 图标 / 描边**不应再出现非黑颜色，只剩白名单语义色；**三档文字仍为固定值**（`#f2f1ee` / `#c2c3c4` / `#8b8e92`），不随蒙版变黑 / 变亮且保持可读。
- **字段 → CSS 变量 → 默认值（唯一真相源）**：

| 类别 | 字段 | CSS 变量 | 默认值（未自定义时） | 负责范围（2026-10-11 定稿） |
|---|---|---|---|---|
| 底色 | `baseColor`（空串 = 跟随主题派生） | `--surface-rgb` / `--surface-rgb-hover`（+ 派生 `--ctx-*`、`--shade-rgb`） | `22, 24, 27` / `42, 45, 49` | 插件 / 面板底；**并派生中性黑遮罩**（`--shade-rgb` = 底色的暗变体，供 `rgba(var(--shade-rgb), α)`）；**拉条轨道**（音乐播放 / 扫描进度条轨道，见 §界面 11 白名单注） |
| 按钮线条 | `btnLineColor`（空串 = 跟随主题色）+ `btnLineAlpha` | `--btn-line-rgb` + `--btn-line-alpha` | `var(--accent-rgb)` + `1` | 未选中**文本框 / 按钮的 border**、按钮内 **icon 线条**、**拉条圆形滑块**（含音乐进度圆点）/ **拉条填充**（音乐播放 / 扫描进度条填充）、切换开关轨道、**音乐播放模式按钮的描边**；**并接管所有面板 / 文本框描边**（`--border-glass` / `--ctx-border-glass`，α 固定 `0.14`）。**注（2026-10-11 更正）**：滚动条**已不使用**按钮线条 —— 滚动条轨道不描边、滑块实心无 border（见 §界面 6），故本节不再列「滚动条轨道描边」 |
| 按钮背景 | `btnBgColor`（空串 = 跟随主题色）+ `btnBgAlpha` | `--btn-bg-rgb` + `--btn-bg-alpha` | `var(--accent-rgb)` + `0.09` | 未选中背景（全局 `#app button:where(...)`、输入栏那排按钮及其 hover 抬档） |
| 按钮蒙版（旧称「其他颜色」，2026-10-11 改名） | `overlayColor`（空串 = 跟随主题色）+ `overlayAlpha` | `--other-overlay-rgb` + `--other-overlay-alpha`（派生 `-nav-alpha` = ×1.4、`-btn-alpha` = ×2） | `var(--accent-rgb)` + `0.1` | 选中态 border + 蒙版 + icon；**并接管图标颜色**（见下）；含**下拉框选项的悬停 / 选中**（见 §界面 12）、**音乐选中框**（`.music-tr.sel` / `.music-mt.sel` 的整圈色框 + 淡填充）与**音乐播放模式按钮的激活态文字** |

- **透明度默认值（2026-10-18 用户裁定，记录其手工调整值）**：底色 `surfaceAlpha = 1`、按钮线条 `btnLineAlpha = 1`、按钮背景 `btnBgAlpha = 0.09`、按钮蒙版 `overlayAlpha = 0.1`。
- **「恢复默认主题」管辖范围（2026-10-18 二次调整）**：开关开着时，**三组颜色回到默认值并锁定**；**四种透明度不是不可调** —— 打开开关时设置面板把它们一次性写回默认值、刷新滑块读数，但滑块**不锁定**，之后仍可继续拖动并生效。`applyAppearance()` 恒取用户当前值（此前的 `alpha()` 兜底与 `.locked` 已删除）。

- **固定值（单一真相源 = `styles.css :root`）**：三档文字与中性叠加基色**不派生**，也不随主题色 / 按钮蒙版 / 底色改变明暗（2026-10-17 删除旧派生逻辑）：
  - `--text` = `#f2f1ee`（正文，一级最亮）/ `--text-dim` = `#c2c3c4`（次要）/ `--text-muted` = `#8b8e92`（极弱）；与 `themes/default/theme.json` 的 `text` / `text_dim` / `text_muted` 同值；
  - 在默认底色 `#16181b` 上的对比度分别 ≈ **15.9:1 / 10.2:1 / 5.4:1**（正文远高于 WCAG AA 的 4.5:1；极弱档只用于占位 / 元信息，不承载正文）；
  - `--ink-rgb` = `255, 255, 255`（`--ctx-rgb` / `--ctx-ink-rgb` 同源），固定中性叠加基色；
  - **覆盖优先级**：`:root` 固定默认 **<** 主题包显式 token（`tokens.text` / `text_dim` / `text_muted`，由 `applyAppearance()` 第 ④ 步写入）；主题不提供时用 `:root` 固定默认。
  - **文字明暗度（`textLight`，±100，默认 0）**：设置 → 风格 ·「文字自定义」的滑块，**只做 HSL 明度偏移**（三档一起挪，色相 / 饱和度不变）。0 时不写内联变量 ⇒ 逐像素等于上面的固定值；非 0 时以「主题 token 或 `:root` 固定值」为基准偏移。**不受「恢复默认主题」管辖**；非默认主题包下随配色组一起被主题锁。
- **两条派生链（`applyFourClassNeutrals`，JS 算、CSS 只消费；规则：颜色字段空串 ⇒ 用主题色 `--accent`）**：
  1. **描边 ← 按钮线条（btnLine）**：`--border-glass` = `--ctx-border-glass` = `rgba(var(--btn-line-rgb), 0.14)`（α 固定、不随 `btnLineAlpha`）。默认 = `rgba(207,200,187,0.14)`（2026-10-18 由 0.10 提到 0.14）。
  2. **中性黑遮罩 ← 底色（base）**：`--shade-rgb` = 底色各通道 ×0.25（暗变体、近黑；黑底 ⇒ 全黑）；`--shade-scale` = 暗底 `1` / 亮底 `0.35`。默认 = `8, 6, 6`（在默认底色 `31,25,25` 上，与原纯黑 `0,0,0` 近乎不可分）。消费方一律写 `rgba(var(--shade-rgb), calc(α * var(--shade-scale)))`。反差四件套 `--ctx-*` 同源：凹陷基色取 shade、描边取 line；叠加基色固定取 `:root` 的 ink。
- **派生规则**：颜色字段是**空串哨兵** —— 空串 = 用户没动过 ⇒ 回落 `:root`（= `var(--accent-rgb)`，跟随主题色）；用户一动取色器就写成真 hex，从此以用户值优先。
- **`--border-glass` / `--ctx-border-glass` 归「按钮线条」**（2026-10-11 翻牌，取代此前标记的「待拍板」）：`main.ts` 无条件按 `--btn-line-rgb` 写出这两条内联值（插件悬浮窗经 `broadcastThemeVars` 拿到同一份），文本类输入框 / 面板描边由此统一受控。
- **强制要求**：
  - **今后设计任何按钮 / 拉条 / 文本框 / 选中态 / 图标 / 描边 / 中性底（含新插件）时，颜色一律走上面四类变量，不得写死 `#rrggbb` / `rgb()` / `rgba()` / `hsl()` 字面量。三档文字固定用 `var(--text)` / `var(--text-dim)` / `var(--text-muted)`。**
  - 透明度类颜色走对应 `*-alpha` 变量（或由它 `calc()` 派生），**不写死 α**。
  - 插件是**独立窗口**：CSS 与主窗共用同一份 `styles.css`，但变量靠 `broadcastThemeVars()`（`lunac-theme-vars` 事件）转发 —— 插件里**直接用 `var(--btn-*)` / `var(--other-overlay-*)` / `var(--ink-rgb)` / `var(--shade-rgb)` 即可**（主窗已把内联值转发过来），**不要假设 `:root` 之外会自动继承**，也不要自己再算一份派生。
  - 遇到「看起来不受按钮颜色控制」的反馈：先对齐**形状**（尺寸 / 圆角 / 边框）与**状态规则**，确认同源后再查颜色来源。
- **白名单（故意不并入四类，保持固定；把四类全设黑后仍允许存在的颜色与理由）**：
  - **语义色**（固定、不随主题）：`--danger-rgb`（危险 / 失败 / 拒绝 / 告警红，**唯一数值来源**）与 `--green` / `--yellow` / `--blue` 及其字面量 —— `rgba(var(--danger-rgb),*)`（危险，2026-10-16 起由 `--danger-rgb` 单一来源供给，见 §界面 16）/ `rgba(157,180,172,*)`（成功）/ `rgba(201,184,150,*)`·`rgba(198,168,106,*)`·`rgba(214,187,121,*)`（警告）/ `rgba(148,144,180,*)`（信息）。**理由**：它们表达状态（成功 / 失败 / 警告 / 信息），若跟随四类会丢失语义。
  - **音乐「正在播放」文字高亮**（`.music-*.playing`：正在播放那一行 / 歌单只提亮文字）与**左栏编辑态勾中行**（`.music-pl.sel` 的 `--accent` 底 + 左缘）。**理由**：与「正在播放」/ 编辑勾选这一**插件内运行态**绑定。（注：`.music-*.playing` 的文字强调**实际已走 `rgb(var(--other-overlay-rgb))`**，见 §界面 2；但本轮未点名收编它，按用户口径**保留**。）**2026-10-11 第 4 轮：播放 / 扫描进度条与选中框已收编** —— 进度条填充（`.music-bp-fill` / `.music-progress-fill` / `.music-scan-fill`）改走**按钮线条**、其轨道改走**底色**派生的中性黑；选中框（`.music-tr.sel` / `.music-mt.sel`）改走**按钮蒙版**；**正在播放文字高亮仍保留**。
  - **封面占位渐变**（`.music-pl-liked-cover` 紫色渐变、`.music-pl-trend-cover`）、**截屏暗幕**（`.shot-body{background:#000}` 与 `#shot-size` / `#shot-hint` 的 `#fff`）、**终端底 / 前景**（`.tool-card .tool-term{background:#0b0b0c}` 与 `main.ts` 的 xterm theme）、**取色器自身彩虹 / 黑白渐变与 `.ap-sv-cursor` 描边**。**理由**：这些是**内容本身**（图片像素 / 屏幕像素 / 终端 ANSI），不是界面配色。
  - **`--glass-sheen-image` 反光白**。**理由**：镜面高光物理上就是白的。
- **取代旧约束**：旧文「可覆盖变量的默认值必须逐项等于改造前硬编码值 / 默认逐像素不变」**本次（2026-10-11 用户裁定）明确作废**。新约束 = **默认观感可变，但必须可读、可用**（文字对比度不得崩，见上面「固定值」的对比度数据）。
- **自查清单**（写控件前逐条过）：
  1. 我写的这个颜色，**用户把四类全设 `#000000` 时会不会变黑？**（图标 / 描边 / 中性底要能变黑；三档文字是固定值例外，不随四类变）
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
  全局 `#app button:where(...)` 亦给同值）；描边走**按钮线条** α0.14（`var(--ctx-border-glass)` = `rgba(var(--btn-line-rgb), 0.14)`）；
  文字 = `var(--text)`；**箭头**是内联 SVG、`stroke="currentColor"` ⇒ 随本体文字色（`var(--text)`）。
  hover 时描边抬到 `rgba(var(--btn-line-rgb), var(--btn-line-alpha))`。
- **下拉面板（dropdown）**：底 = **底色**（`var(--surface-glass)`）；描边 = **按钮线条**（`var(--border-glass)`）；
  阴影用底色派生的中性黑 `rgba(var(--shade-rgb), calc(0.4 * var(--shade-scale)))`。
- **选项（option）**：未选中 = 透明底 + 文字 `var(--text-dim)`（固定值，不随按钮蒙版漂移）；
  **悬停 / 选中 = 按钮蒙版** `rgba(var(--other-overlay-rgb), var(--other-overlay-nav-alpha))`（档位同设置侧栏选中项 / 分类标题）。
- **滚动条：与四类无关**（2026-10-11 更正）：本组件列表用 §界面 6 那条**唯一**全局 `::-webkit-scrollbar`
  —— **滑块实心无 border、轨道不描边**，不按容器另写。

### §12.5 明确的反面（硬事实）

- **原生 `<select>` 无法对齐本组件**：其弹出层（选项行 + 右侧滚动条）由 OS / 浏览器绘制，`<option>` **不可样式化**
  ⇒ 连「选项颜色」「弹出层滚动条」都染不了，收合面也**只在收合态**可染。
  **凡要求「与设置里的下拉一致」的场合，必须换成 `.custom-select`** —— 只调收合面永远对不上。

### §12.6 现存原生 `<select>`（历史遗留，豁免）

- `plugins/builtin/translate.ts` 的 `.xl-select`、
  `plugins/builtin/pet.ts` 的**热键行 `group`/`id`/`mode`/`enabled`** 与**动作行** `.pet-mo-row select`。
  它们的**弹出层不受四类控制**；收合面按基准对齐（高 26 / 圆角 6 / 内边距 `5px 8px` / `font-size: .73rem` /
  底 `rgba(var(--shade-rgb), calc(0.25 * var(--shade-scale)))` / 描边 `var(--border-glass)` / 文字 `var(--text)`）。
- **2026-10-11 已改自绘（收编）**：`pet.ts` 的**热键行「类型」「表情对象 target」**与**动作链步骤「动作文件」**
  三处已从原生 `<select>` 换成 `.custom-select`（`customSelectHtml` + `attachCustomSelect`），与设置里的下拉**同源**。
- **2026-10-23 已改自绘（收编）**：`app/src/usage-cost.ts`（右下角表盘展开的「用量与成本」面板）的
  **日期 / 来源 / 模型**三个下拉由原生 `<select class="cost-range-select">` 换成 `.custom-select`
  （id 不变：`cost-range-select` / `cost-source-select` / `cost-model-select`）；面板 `overflow:auto`
  会在展开时临时放开（`onOpenChange`），关面板时 `closeAllCustomSelects()` + 还原，避免弹层被裁。
  原 `.cost-range-select` 的 CSS 已删。
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
.approval-deny  { border-color: rgba(var(--danger-rgb),0.35); color: var(--red); }
.approval-deny:hover  { background: rgba(var(--danger-rgb),0.15); }
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

## §界面 15 对话区栅格（仅用户气泡右移）

> 2026-10-16（用户裁定）：**只有「我们问出去的问题」那个对话框向右移、左侧留约 1/6 空档**；
> **AI 的回答仍旧铺满屏幕**。
>
> **同日订正**：本节初版（同日）曾把**整列** `#chat-log` 左缩进（`#chat-log { margin-left: var(--chat-gutter) }`）
> —— 那样连 **AI 的回答与所有过程块**一起被挤窄了，与用户口径相反。现改为**只右移用户气泡**
> （`.chat-msg-user`）；`#chat-log` / `.chat-msg-assistant` / `.agent-flow` 一律**全宽**。

- **作用范围 = 用户气泡 `.chat-msg-user`**（聊天独立窗 `plugin-chat` 与主窗共用同一份
  `main.ts` / `index.html`）。渲染路径两条都吃同一套类名 ⇒ 实时（`appendUserMsg`）与历史回放
  （`renderChatLogHtml`）**一致右移**。**不受影响**：AI 回答 / 过程块（`.chat-msg-assistant` /
  `.agent-flow`）**全宽**；桌宠输出框（`.pet-chat-output`，另一套 DOM）不动；插件面板不动。
- **实现**：`:root` 的 `--chat-gutter: 16.6667%`（唯一布局 token，**唯一消费方 = `.chat-msg-user`**）
  + `.chat-msg-user { margin-left: var(--chat-gutter) }`。`#chat-log` **不设** `margin-left`，
  也**不设** `max-width`（那会变成固定列宽，与「留 1/6」是两个口径）。
- **响应式判据**：用百分比而非硬编码 px ⇒ 把聊天窗拖到最窄，空档同比缩小、气泡仍占剩余 5/6
  且可读；窗口拉宽，空档与气泡同步放大。
- **一段对话 = 一个 `.chat-turn` 板块**（2026-10-17，用户要求「小对话及其回答以及人格名称加上
  展开过程独立成一个板块」）：板块内部顺序**固定**为
  `用户气泡 → 人格名 → 回合信息条 → 过程（thinking / 工具卡 / 中间输出，可折叠）→ 最终答案 → 回答操作（重试 / 复制，仅 icon）`。
  板块之间只用既有 **12px** 间距区分（不新造卡片底色 / 边框 / 阴影，见 §界面 3）；
  用户气泡仍单独吃 `--chat-gutter` 右移，AI 回答 / 过程块仍是**全宽**。
- **轮次边界刚性**：实时路径由 `appendUserMsg` 新建板块、`startAgentChat` 写回同一板块；
  正常完成 / 停止 / 中断 / 空输出 / 切会话 / 新建对话都会显式 `closeLiveTurn`，**下一轮一定新建
  板块** ⇒ 被暂停的空轮，其过程块不会续写到下一轮。
- **人格名 = `.chat-answer-persona`**（2026-10-16 引入，2026-10-17 随板块固定到顶部）：每轮
  **一行、紧贴用户气泡下方**（过程之前），次要文字档 `--text-dim`（不高亮、不描边）。
  它是**独立 DOM 节点**，不进 `textSegs` / `chatHistory` / 主请求 / `persona.md` ⇒
  **复制正文拿不到它**（复制取 `chatHistory` 原文，不读 DOM）。名字 =
  `lunac-persona-active` 预设名 || 内置名 `Lunac`（唯一真相源 `main.ts` 的
  `currentPersonaName()`）。**只在主对话 / 聊天窗渲染**，桌宠窗（`.pet-chat-output`，
  `pet.ts`）不含此行。
- **实时 / 历史同构**：历史回放 `renderChatLogHtml` 也按 `.chat-turn` 逐轮包裹；
  `renderHistoryProcess` 把过程快照插回**该轮自己的**板块（有正式回答 ⇒ 插在回答之前；
  暂停 / 空输出 ⇒ 追加在板块末尾），实时与历史回放结构一致（§禁用 62）。
- **最终回答只出现一次（2026-10-23，用户报「两段最终输出」）**：过程快照里**最后一条 text
  步骤**就是本轮正式回答（`agentStartTextSeg` 只在 `tool_result` 处起新段、只有最后一段没有
  `data-mid`），历史重绘时它又由助手气泡渲染 ⇒ 同一段出现两次。现在 `renderHistoryProcess`
  用 `stripAnswerStep()` 把它从过程块里摘掉；`hasMsg === false`（无正文的截断轮）不摘。
- **回合信息条 `.turn-meta`（2026-10-23，替代 `turn-footer` / `turn-fold` / `turn-rollback`）**：
  人格名下方**一行可点摘要**，形态**照「思考过程」那一行**（`.think-block summary` 同族：
  左侧 2px `--border-glass` 细线 + `padding-left 8px` + 11px 暗字 + 线性时钟图标），
  内容 = `耗时 {dur} · 失败 {n} 次`（历史旧记录没有 `SessionProcess.durationMs` ⇒ 只显示失败数）；
  右侧 `chevron` 提示可展开，点整条 = 切换过程折叠（展开时箭头转 90°、`aria-expanded=true`）。
  没有过程（纯问答）时无箭头、`is-static` 不可点。**无底色**（用户 2026-10-23 二次口径：
  不要把它和输出正文隔开）。**不再有**「展开过程」按钮与「回退到本轮」按钮；助手气泡悬停仍保留「回退」。
  间距：紧跟其后的 `.agent-flow` 上内边距 2px；历史折叠态的过程块
  `.agent-flow.history-process.flow-folded` **整块 `display:none`**（不占位），
  其后的回答气泡上内边距归零 ⇒ 信息条与正文之间没有空隙。
- **回答操作 `.flow-actions`（2026-10-23）**：最终回答下方一行**仅图标**按钮 —— 重试（`RETRY_SVG`）
  + 复制（`COPY_SVG`），复制**只取最终回答正文**（不含过程 / 信息条 / 括号里的汇总文本）。
  实时收尾与历史气泡共用 `buildAnswerActions()`（§禁用 62）；重试下标存 `data-retry-idx`，
  随上下文裁剪由 `shiftRenderedMsgIdx` 前移（被裁掉则摘掉按钮）。
  助手气泡悬停动作条不再挂复制（避免与下方图标重复），只留回退。

## §界面 16 危险色（`--danger-*`：固定、不随主题漂移）

> 2026-10-16（用户裁定）：把所有「危险 / 失败 / 拒绝 / 告警红」统一到**唯一数值来源** `--danger-rgb`，并调得更显眼。

- **定义处**：`styles.css` 的 `:root` —— `--danger-rgb: 222, 115, 115;`（hsl(0,59%,64%)）、
  `--danger: rgb(var(--danger-rgb));`、`--red: var(--danger);`（历史别名，沿用约 120 处既有调用点）。
  整族**只有一个数值**（§禁用 62）；带 alpha 一律写 `rgba(var(--danger-rgb), α)`，不得再出现
  `rgba(192,138,138,α)` 这类硬编码分量（§禁用 17）。
- **为什么它不受主题色影响**：`--danger-rgb` 是 `:root` 上的**固定常量**，既不由 `--accent` 派生，
  也不经 `applyAppearance()` / `applyFourClassNeutrals()` 重写 —— 改主题色 / 四类自定义色都不会动它。
  旧值 `#c08a8a` 是低饱和「尘玫瑰」，与**默认主题色** `#c0a0a0` 几乎同色；用户把主色改成红系时危险红
  会被日常 UI 淹没（这就是「警告红被主题色影响」的真实来历）。新值提高饱和度、拉开与任何主题色的距离，
  暗底 / 亮底都可辨识。
- **什么场景用**：拒绝 / 失败 / 危险 / 删除 / 两段式确认 —— `approval-deny`、`.run-mode-auto` 警示、
  `.tool-card.failed`、`.tool-secret-warn`、`.sys-note-error`、`[data-armed="1"]`、各处删除按钮…
- **不改哪些**：品牌色 / 装饰 / 链接的红**不在此列** —— 主色「尘玫瑰」是 `--accent`（跟随主题，§界面 11）；
  链接 / 信息用 `--blue`；「要人看一眼但不确认危险」的告警（无法静态判定、疑似凭据）用 `--yellow`。
  终端底 / 前景、截屏暗幕、封面渐变等**内容色**同样不动（§界面 11 白名单）。

## §界面 17 对话区层叠（z-index 单一尺度）

> 2026-10-17（用户裁定）：**历史记录抽屉必须压住待办清单 / 消息 / 过程块 / 工具卡 / 审批卡 /
> 菜单**；抽屉遮罩在抽屉之下、在其余对话内容之上。

- **真相源**：`app/src/styles.css` 的 `:root` —— `--z-chat-*`（见文件顶部注释）。
  消费点**只写** `var(--z-chat-*)`，不得再散写 `z-index: 9999` 这类魔法数字（§禁用 62）。
- 这些值都位于 `#results-container` 这一个 stacking context 内（该上下文由
  `backdrop-filter` 创建），因此只需在**上下文内部**比较大小：

| 档 | 变量 | 值 | 消费方 |
|---|---|---|---|
| 提问节点 | `--z-chat-node` | 18 | `#chat-node-panel` / `#chat-node-rail` |
| 底部停靠 | `--z-chat-todo` / `--z-chat-input` | 20 | `#todo-drawer` / `#chat-input-bar` |
| 审批停靠 | `--z-chat-approval` | 21 | `#approval-dock` |
| 对话弹层 | `--z-chat-popover` | 60 | `#chat-more-menu` / 斜杠候选菜单 |
| 抽屉遮罩 | `--z-chat-drawer-backdrop` | 9998 | `#chat-drawer-backdrop` |
| 历史抽屉 | `--z-chat-drawer` | 9999 | `#chat-drawer` |

- **新增浮层的选档规则**：只能从上表里选一档（与消费方语义对齐）；确需新档时，
  先改本节 + `styles.css` 的 `:root`，再改消费点 —— 不许在局部另写数字。
- **交互不变**：遮罩仍可点击关闭、`Esc` 仍可关闭；遮罩 z 必须**低于**抽屉 z。
- **范围外**：`#image-viewer`（固定层）、`#push-toast-host`、调试浮标等不属于对话区
  内容层，不纳入本尺度。

## §界面 18 动效（2026-10-18）

- **曲线与时长**统一取 `:root` 的 `--ease-out` / `--ease-collapse` / `--motion-fast|base|slow`；组件里不得再写 `0.08s` 这类魔法数字。原则：克制飘逸，常规 240~320ms、大面积 480~600ms，**禁弹跳 / 过冲**。
- **减少动态效果**：`prefers-reduced-motion: reduce` 时退场动画与新增过渡一律不播（`styles.css` 有 `@media` 兜底），退化为即时隐藏。
- **搜索栏保持 `transition: none`**（打字性能，2026-09-19 定）：退场动画只允许用 `animation` + `html.is-exiting` 类驱动，**不得**为了动画给 `#search-bar` 加 transition。
- **执行搜索结果的退场动画**（2026-10-18 三次定稿：画卷式拉缩；总长 900ms，`main.ts` 的 `EXIT_TOTAL_MS` 必须与 CSS 一致）：

| 时间 | 对象 | 效果 |
|---|---|---|
| 0~495ms（0~55%） | 整个页面 `#app` | `clip-path: inset()` 上下裁切，从整页收拢到被点结果所在的 **60px 横带**；**宽度不变、内容不缩放（不用 `scaleY`）、不做淡出** |
| 495~900ms（55~100%） | `#app` | 保持 60px 横带（keyframes 的 100% 与 55% 相同） |
| 0~900ms | `#exit-focus-card` | 被点结果克隆成的 **60px 高固定卡**，不缩放、不淡出，停留到窗口隐藏 |

- **实现方式**：`clip-path` 只裁显示区域、不改内容尺寸，所以文字与图标不会被压扁；被点行加 `.is-exit-source` 并 `opacity: 0`（由固定卡接管，避免重影）。JS 在动画前把 60px 横带的上下裁切量写入 `#app` 的 `--exit-clip-top` / `--exit-clip-bottom`（布局像素，rect ÷ `currentZoom`）；`#exit-focus-card` 克隆 `innerHTML`，`position: fixed`、`height: 60px`，坐标同样 ÷ `currentZoom`。固定卡挂在 `body` 下、不在 `#app` 内，因此不被裁切。`resetResultExit()` 在窗口隐藏时清除 `--exit-clip-*`、`.is-exit-source` 与固定卡。
- **无蝴蝶饰带**：退场不再有 `#exit-ornament` 固定层，搜索栏也不再渲染饰带。
- **唤出入场**：`lunac-window-shown` 后给 `html` 挂 `is-booting` 620ms，`#results-list` 容器播放 `result-boot`（淡入 + 上浮 4px，600ms）；不做 blur（整列表 blur 在 WebView2 上掉帧）。作用在容器而非逐行，避免唤出后立刻打字时反复重播。
- **选中行图标**：`.result-item.selected .result-item-icon` 用 10% 同色低透明填充表达「水彩晕染微膨胀」，不做反白色块。
- **挂钩点**：`launchApp()`（本地应用）与 `web-search` 插件的 `execute()`（隐藏主窗前调 `window.__lunac_play_exit()`）。聊天窗 / detached / 详细搜索大界面**不播**（没有结果列表形态）；`web-search` 调用时 `pluginActive` 已为真，走 `allowInPlugin` 例外。
- **窗口纪律不变**：动画期间**不得** `setSize`、不得增删 `#results-list` 条目、不得调 `applyWindowSize()`；真正隐藏放在动画末尾，唤出时 `lunac-window-shown` 先 `resetResultExit()` 再 `forceHeightReassert()`。
- **防抢跑**：动画期间 `_exitAnimating = true`，失焦自动隐藏（`win.onFocusChanged` 的 150ms 定时器）必须让位；动画结束后在 `launchApp` 的 `finish` 里清标志（插件路径另有 1.3s 兜底清除）。

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

---

## 本轮改动记录（2026-10-16 · 对话区栅格订正 + 人格名改为「每段正式回答开头」+ 人格预设）

> 用户口径（2026-10-16）：「把我们问出去的问题的对话框向右移动，AI 模型的回答依旧铺满屏幕」；
> 「对话中的人格名称应该放置在每一段回答的开头」；「预设列表直接放在人格分页处，在人格上方放置预设按钮」。
> 代码 = `app/src/{styles.css,index.html,main.ts,i18n.ts}`、`docs/ui-style.md`（§界面 15）。

- **§界面 15 订正（从「整列左缩进」改为「仅用户气泡右移」）**：撤掉 `#chat-log { margin-left: var(--chat-gutter) }`（它把 AI 回答与过程块一起挤窄了），改由**用户气泡 `.chat-msg-user` 单独**吃 `--chat-gutter`（`margin-left`）。`--chat-gutter` **保留**，唯一消费方改为 `.chat-msg-user`（不留「看着在用其实没效果」的死变量）。
- **人格名改呈现方式（删「左上常驻一行」，改为「每段正式回答开头一行」）**：删除 `#chat-persona-name` 及其 CSS / 逻辑（含同一批的「行内人格名输入行」与 localStorage `lunac-persona-name`）；新增 `.chat-answer-persona`——在每段**最终答案块之前**另起一行显示当前人格名（`--text-dim`）。独立 DOM 节点 ⇒ **不进 `textSegs` / `history` / 复制正文**；实时与历史回放一致。
- **人格预设（`lunac-persona-presets` / `lunac-persona-active`）**：入口「预设」按钮 + 新建按钮内联在**人格分页**（不另起页）；命名弹框自绘（标题=人格名称、一行文本框、保存/取消），复用玻璃面板 + 动作按钮族、**绝不用 `window.prompt`**；点列表某条 = 把该预设文本写进人格框并走既有 `savePersona()`；高亮用「名 + 文本双判」。
- **「⋯ 更多设置」宽度 288 → 320px**（`#chat-more-menu`），并核实各页控件不再换行。
- **「⋯ 更多设置」宽度 320 → 420px（2026-11-01）**：新增「安全」分页（白箱边界账本，W0）后，六枚页签与账本行在 320px 下会挤成两行；页序固定为 **权限 / 工作区 / 工具 / 人格 / 安全 / 其他**（「安全」在「其他」之前，用户指定）。

---

## 本轮改动记录（2026-10-18 · 默认主题重构 + 动效体系）

> 用户口径（2026-10-18）：按参考图 `HJMomEnbUAAeaH-.jpg` 的人物做印象化设计，但**页面配色不取自图片** —— 页面要简洁、易读、大气；图片只供 icon 与动效气质。**不添加背景图片**。退场动画三次定稿为「画卷式上下拉缩到被点结果的 60px 横带 → 保持」，**不压扁内容、不淡出**；蝴蝶饰带设计已整体去除。透明度以用户手工调整值为新默认；打开「恢复默认主题」时自动写回这些值，但**透明度仍可调**；「文字明暗度」滑块同日恢复。
> 代码 = `app/src/{styles.css,main.ts}`、`app/src/plugins/builtin/{settings.ts,web-search.ts}`、`app/src/i18n.ts`、`app/src-tauri/themes/default/theme.json`、`docs/ui-style.md`、`docs/theme-impression-design.md`。

- **前置删除（已核实，本会话未重复改）**：三档文字与 `--ink-rgb` 的自动明暗派生已于 2026-10-17 删除；`styles.css :root` 固定值、`main.ts` 的 `applyFourClassNeutrals` 只保留「描边 ← 按钮线条」「中性底 ← 底色」两条派生链（**自动派生不恢复**，只恢复用户可调的明度偏移，见下）。
- **§界面 1 / 3 色板与圆角（2026-10-18 定稿）**：`--surface-rgb` `22,24,27`（hover `42,45,49`）；`--accent-rgb` `207,200,187`（#cfc8bb 香槟银）；三档文字 `#f2f1ee` / `#c2c3c4` / `#8b8e92`（在 #16181b 上 ≈15.9:1 / 10.2:1 / 5.4:1）；`--radius` 14 → 8。页面配色不取自参考图（图只供 icon / 动效气质，见 `docs/theme-impression-design.md`）。
- **透明度（2026-10-18 用户裁定）**：把用户手工调整值记为默认 —— `surfaceAlpha 1` / `btnLineAlpha 1` / `btnBgAlpha 0.09` / `overlayAlpha 0.1`；`styles.css :root`、`main.ts APPEARANCE_DEFAULTS`、`settings.ts` 兜底三处同步。**「恢复默认主题」的边界**：打开开关时把四种透明度一次性写回默认值并刷新滑块读数，但**滑块不锁、渲染层不归零**（删除此前的 `alpha()` 兜底与 `.locked` 类），之后仍可调。
- **主题包（`themes/default/theme.json`）**：`accent` / `text` / `text_dim` / `text_muted` / `radius_search|results` 同步；**新增 `surface: "#16181b"`** —— 否则底色由 `#cfc8bb` 的暖色相派生，目标中性石墨底无法成立。`border_glass` token 删除：`applyFourClassNeutrals` 会无条件覆写它，留着等于「看着可配、实际无效」。
- **icon（印象化）**：不改 `stroke-width`（2.2 在 12px 显示下≈1.1px，对应描述里的「屏幕约 1px」）；在 §界面 4 补充「按渲染像素折算」的口径，禁止直接写 1.25 / 1.5。选中行图标容器改为 10% 同色低透明填充，hover / 选中加 2px 柔光。
- **§界面 18（新增）动效体系**：新增 `--ease-out`（水感 `cubic-bezier(0.25,0.1,0.25,1)`）/ `--ease-collapse` / `--motion-*` token；`.result-item` hover 过渡 0.08s → `--motion-fast`；新增 `page-exit-scroll` / `result-boot` 两组 keyframes 与 `html.is-exiting` / `html.is-booting` 门控；`launchApp()` 增加可选 `clicked` 参数，页面 900ms 用 `clip-path: inset()` **画卷式上下裁切**到被点结果的 60px 横带（宽度不变、内容不缩放、不淡出），同时把被点结果克隆成 60px 固定卡 `#exit-focus-card`（坐标除以 `currentZoom`，不淡出、随窗口隐藏消失）；应用启动在 405ms 发出、隐藏放到动画末尾；`web-search` 插件在 `win.hide()` 前调用 `window.__lunac_play_exit()`；`_exitAnimating` 阻止失焦自动隐藏抢跑。
- **移除蝴蝶饰带（2026-10-18）**：`index.html` 的 `#search-bar-filigree`、`styles.css` 的对应规则、`#exit-ornament` 固定层与 `buildExitOrnament()` 全部删除；`#search-input-area` 的 `z-index: 3` 一并回退。
- **文字明暗度恢复（2026-10-18）**：`Appearance.textLight` 字段（±100，默认 0）与设置 → 风格 ·「文字自定义」面板回归；`main.ts` 新增 `shiftHexLightness()`，在「主题 token 或 `:root` 固定值」上做 HSL 明度偏移，0 时不写内联变量（默认逐像素不变）；`loadAppearance()` 不再丢弃老配置里的 `textLight`。**自动派生不恢复** —— 文字颜色仍不随主题色 / 蒙版漂移。
- **不做**：不添加背景图片、不新增 `setSize`、不改语义色白名单（`--danger-rgb` 等保持原值）、不做浅色主题。

---

## 本轮改动记录（2026-10-21 · 同类插件图标兜底 + 阅读插件图书图标）

> 用户口径（2026-10-21）：「根据 `theme-impression-design.md` 出一个主题颜色配色 + icon 设计；
> AI 自生成的默认插件图标需要更改；阅读插件图标改成图书样式」。
> 代码 = `app/src/main.ts`、`app/src/plugins/builtin/settings.ts`；
> 设计依据 = [`theme-palette-icon-design.md`](./theme-palette-icon-design.md)。

- **默认插件图标（AI 生成兜底）**：`pluginIconSvg()` 的未知 id 不再返回 emoji `🔧`，改用
  `DEFAULT_PLUGIN_ICON`（圆角插件包 + 四角星光，18px 可读），并继续走同一个包装
  （左上残影 `translate(-0.7,-0.7)` / `opacity .28` + 左上点彩 `r=1` + 右下淡点 `r=.5`）。
- **阅读插件（`resource-hub`）**：`PLUGIN_ICON_PATHS` 新增「合着的书 + 书签丝带」；
  不用「打开的书」——18px 下两页斜线会糊。
- **设置面板图标来源统一**：本地插件列表、市场索引条目、工具链行三处此前直接渲染清单 `icon`
  （AI 生成插件通常为空 ⇒ 空白方块），现在先取 `window.__lunac_plugin_icon(id)`，再回退清单值。
- **配色**：只出提案（`theme-palette-icon-design.md` §1.2「雾银 · 冷石墨」：底 `#14171a`、
  正文 `#eff1f2` 15.88:1、accent `#c6ced2`），**本轮不改 token**；切换步骤见该文档 §1.4。
- **验证**：`npx tsc --noEmit`、`npm run build`、`npm run verify`。

## 本轮改动记录（2026-10-22 · 桌宠：主动反应文案 / 循环热更新 / 人格清空确认）

> 用户口径（2026-10-22，A 批验收后四条）：① 删主动反应页两段解释文案；② 多个「循环」动作打开时
> 模型会不可见；③ 人格「清空」要二次确认；④ 「循环播放」按钮文案缩成「循环」。
> 代码 = `app/src/plugins/builtin/pet.ts`、`live2d-engine.ts`、`live2d.ts`、`app/src/i18n.ts`。

- **主动反应页**：删掉「先按抓取频次检测变化…60 秒合并窗口…」与「开启后，桌宠会在可见时偶尔
  查看屏幕…隐藏或最小化时立即停止」两段 `pet-note`；对应 i18n 键 `pet.proactive_gap_hint` /
  `pet.proactive_shot_hint` 一并删除（死键清理）。**如实记**：删掉第二段后，界面上不再有
  「会看屏幕 / 隐藏即停」的文字告知（原注释里的隐私提示随之取消，用户明确要求删）。
- **循环按钮文案**：「循环播放：开 / 关」→「循环：开 / 关」（`renderMotions`）；`title` 保持完整解释。
  「整链循环」按钮文案未动（它是另一层语义）。
- **循环动作导致模型不可见（根因 + 修法）**：勾一个循环动作原先走 `CTL_EVENT reload` ⇒
  **整模型重挂**（`dropModel` + 重新加载纹理）。连点几个动作时多次重挂并发，CDP 实测同一时刻
  `#pet-l2d` 里叠到 **5 张画布**、并出现 0 张画布的空窗 ⇒ 用户看到的就是「模型不见了」。
  - 引擎新增 `Live2DHandle.setIdleMotions(files)`（`live2d.ts` 接口 + `live2d-engine.ts` 实现）：
    按挂载时的「文件路径 → 注入组下标」映射就地换列表，**不重挂模型**。
  - idle 驱动器改为**常驻**（此前「挂载时没有循环动作」就根本不装 ticker 回调，
    后来勾第一个只能靠重挂生效）；列表为空时每拍直接返回。
  - 设置窗勾 / 取消循环改发 `CTL_EVENT idle-set`；桌宠窗收到后重扫动作文件、算出现存项交给引擎。
- **人格「清空」二次确认**：`#pet-persona-clear` 接入既有 `armedRun`（与三张列表清空同款），
  首点只换文案「再点一次清空（不可恢复）」并 4 秒后自动复位，再点才清空并落盘。
- **验证（dev 实例 + CDP）**：动作行文案 = 「循环：关」；主动反应页三句残留文案均为 `false`；
  人格正文非空时首点 `armed=1` 且正文长度不变；勾选循环 + 连点 5 个，`#pet-l2d` 画布**始终 1 张**、
  错误条为空。`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin pet` exit 0（`pet 0.9.64`）。

## 本轮改动记录（2026-10-22 · 任务抽屉三页签 + 回退回填 + 文件路径归一）

> 用户口径（2026-10-22，B 批验收后四条）：① 回退要删掉历史里的对话、并把被回退的消息放回输入框；
> ② dev 下打开不了改动文件、也看不到内容；③ 后台命令要进抽屉；④ 待办 / 文件 / 后台**三页互斥**，
> 左侧分页按钮、右侧数量，删「待办清单」标题与进度文案，「确认」移到待办页右下角，每条待办左侧显示完成状态。

- **任务抽屉结构（`index.html` / `main.ts` / `styles.css`）**：`#todo-drawer-head` = 左侧三个页签
  （任务 / 文件 / 后台）+ 右侧当前页数量（`N 个任务` / `N 个文件已改动` / `N 个后台命令`）；
  下方三页 `.todo-page` **只显示一页**。抽屉**刚出现**时优先落「任务」页；当前页空了且用户没亲手点过
  才自动切页；亲手点过的选择（含空页）一律尊重。
- **待办页**：删除「待办清单」标题与 `N/M 已完成` 进度文案；`#todo-drawer-ok-btn`（确认）从头部移到
  待办页右下角（`#todo-drawer-tasks-foot`，`justify-content:flex-end`）；每条左侧 `.todo-state`
  是**纯 CSS 方块**（未完成空心 / 进行中虚线 / 已完成实心），不引 emoji 或字符图标。
- **后台命令**：从「始终展开的一坨」改成独立页签（`N 个后台命令`），每行仍是命令 + PID + 取消。
- **回退语义（`rollbackChat`）**：由**闭区间**改为**开区间** —— 点用户消息 = 删掉它及其后；
  点助手回复 = 往前找对应提问，一起删；随后把该提问（含附件）**回填输入框**并聚焦。
  `/rewind` 同步改成传最后一条用户消息；`retry` 走 `fillInput=false`（马上重发，不预填）。
- **回退段不进历史（2026-10-22 追加）**：回退后落盘的会话按裁剪后的历史重写；若**一条用户消息都不剩**，
  `persistCurrentSessionInner` 的「至少一条用户消息」闸会让它空转 ⇒ 盘上仍是回退前的整段。
  现在这种情况**整条会话记录删掉**（`deleteChatSession`，同一落盘队列内执行），
  `save_chat_sessions` 会顺带 prune 它的帧文件 ⇒ 历史列表里不再出现被回退的对话。
- **页签再点 = 回默认态（不是整块隐藏）**：再点一次**当前高亮**的页签 ⇒ `drawerTab = null`，
  即「没有选中任何分页」：抽屉**仍然显示**（页签栏在、右侧数量清空），三页内容全部收起，
  列表数据**不清空**；点任意页签立刻重新展开。此后新内容不会自动展开（尊重手动选择），
  只有抽屉从隐藏变出现时才自动落到有内容的页。
- **文件路径归一（`get_agent_workdir`）**：模型给的 `file_path` 常是相对 cwd，而
  `reveal_in_explorer` / `open_path` / `snapshot_read_text` **只接受绝对路径** ⇒ 新增宿主只读命令
  返回 `effective_workdir()`，前端在登记改动 / 采快照 / 读现状 / 链接前统一解析；老会话里存的相对
  路径在读回快照与重建文件列表时也一并归一。修复「点文件打不开、diff 显示无法预览」。
- **验证（dev 实例 + CDP）**：三个页签互斥、数量文本正确；待办状态方块完成=实心 / 未完成=空心；
  「确认」贴待办页右下角（gap 0）；文件页展开能读出 diff（`+M2-3-V2`，不再是「无法预览」）；
  点「后台」空页不会被弹回。`npx tsc --noEmit` / `cargo check --bins` exit 0。

## 本轮改动记录（2026-10-22 · 桌宠语音输入）

> 口径（§待办 1-4 ⑤）：桌宠输入行加一颗麦克风按钮，识别结果走既有 `sendToAI()`，不新开通道。

- **形态**：`#pet-chat-mic` 与中止按钮同形 —— 30×30、圆角 6px、`padding:0`；底色 =
  `rgba(var(--btn-bg-rgb), var(--btn-bg-alpha))`、描边 = `--btn-line-rgb/-alpha`、图标色 =
  `rgb(var(--btn-line-rgb))` ⇒ **四类颜色全设黑时随之变黑**（无白名单色）。
- **状态**：识别中挂 `.on`（`rgba(var(--btn-line-rgb), .18)` 淡底）+ `disabled`（`opacity:.55`、
  `cursor:default`）；默认常显（只在输入行出现时可见，随 `.pet-chat-inputrow`）。
- **图标**：线性 SVG 麦克风（`width/height 14`、`stroke-width 2.2`、残影 `g` + 点彩圆点），
  与中止 / 缩放按钮同一套 §界面 4 语言；**不用 emoji / 字符图标**。
- **未知数保持可见**：识别失败按宿主 `reason` 分四类文案（没听清 / 没麦克风 / 没语言包 / 其他），
  文案全走 `t()`。

## 本轮改动记录（2026-10-23 · 修复最终输出重复 + 回合信息条 + 回答操作）

> 用户口径：①「两段最终输出，一个是正常的，一个带有问题对话框的回退按钮」⇒ 删掉重复；
> ②「最终回答的下方放置 重试 / 复制（仅最后结果）的按钮（仅 icon）；人格处下方放置任务进行时间 /
> 失败次数，做成小的隐藏背景、暗文本的下拉框，右侧提示可展开的 icon；去除回退到本轮的按钮；
> 展开过程按钮逻辑交由上述的下拉框」。

- **重复根因**：`recordTurnSteps` 把最终答案那段 `.agent-text` 也收进了过程快照，历史重绘时
  `historyStepHtml` 又画一遍；助手气泡再画一遍 ⇒ 两段。修复点只在**渲染侧**
  （`stripAnswerStep()`），旧会话记录不需要迁移。
- **DOM 顺序**（实时 / 历史同一套）：`用户气泡 → 人格名 → .turn-meta → .agent-flow → 最终答案 → .flow-actions`。
- **信息条样式（2026-10-23 三次改定）**：**无底色**，形态与 `.think-block summary` 同族 ——
  左侧 2px `--border-glass` 细线、`padding-left 8px`、11px 暗字、左侧线性时钟图标
  （`CLOCK_SVG`，`stroke-width 2.2`），右侧箭头展开转 90°，过渡走 `--motion-fast` / `--ease-out`。
  与正文之间不留空隙：后续 `.agent-flow` 上内边距 2px、历史折叠过程块 `display:none`、
  紧随其后的回答气泡上内边距 0。
- **删除**：`.turn-footer` / `.turn-tool-count` / `.turn-fold` / `.turn-rollback` 的 DOM 与 CSS，
  以及 i18n `agent.turn_summary` / `agent.turn_expand` / `agent.turn_collapse` /
  `agent.process_steps` / `chat.rollback_turn`（顺带修掉主对话里引用了不存在键的
  `agent.turn_elapsed`）。
- **持久化**：`SessionProcess.durationMs`（可选，`skip_serializing_if`）随会话落盘；
  Rust `storage::SessionProcess` 增加 `duration_ms`（`rename = "durationMs"`）。
- **验证**：`npx tsc --noEmit` exit 0；`npm test -- --run` 12 passed；宿主 `cargo test --bins`
  **322 passed / 2 ignored**；`npm run verify` 8/8、0 提醒。dev 实例 + CDP 实测（2026-10-23）：
  历史回放板块顺序 = `chat-msg-user → chat-answer-persona → turn-meta → agent-flow →
  chat-msg-assistant → flow-actions`；过程块内 `.agent-text` 数为 0（重复消失，正文只在气泡里）；
  信息条文案「耗时 849ms · 失败 0 次」，点一下 `flow-folded` 由 true→false 且
  `is-open` / `aria-expanded` 同步；重载页面后信息条仍显示「耗时 849ms」（`durationMs` 已随
  会话落盘往返）；全页 `.turn-rollback` 数为 0；回答下方 `flow-retry`（`data-retry-idx`）+
  `flow-copy` 均在位。信息条三次改定后 CDP 复核：`background = rgba(0, 0, 0, 0)`（全局
  `#app button` 底色规则已用 `:not(.turn-meta)` 放行）、`border-left 2px`、`font-size 11px`、
  左右图标均在位、与正文间距 `2px`、点击后 `flow-folded` 由 true→false 且 `is-open` 同步。

## 本轮改动记录（2026-10-23 · 价格表面板下拉收编）

> 用户口径：「右下角的价格表中的所有下拉框都不符合主题规范」。

- **改自绘**：`app/src/usage-cost.ts` 的**日期 / 来源 / 模型**三个下拉由原生 `<select
  class="cost-range-select">` 换成 `customSelectHtml()` + `attachCustomSelect()`
  （全项目唯一标准下拉组件），id 保持 `cost-range-select` / `cost-source-select` /
  `cost-model-select`。日期组不再用 `<optgroup>`（组件无分组语义），范围三项之后直接列日期。
- **面板溢出**：`#token-usage-panel` 本身 `overflow:auto`（长表格要滚）⇒ 下拉展开时经
  `onOpenChange` 临时置 `overflow:visible`，收起 / 关面板时还原（`closeAllCustomSelects()` 兜底），
  否则弹层被面板裁掉。
- **样式**：删 `#token-usage-panel .cost-range-select`（原生 select 专用），改为面板内
  `.custom-select` 尺寸微调（高 22 / `min-width 76` / `max-width 160` / 字号 `.7rem`），
  颜色 / 描边 / 悬停全部继承组件（不另写色值）。
- **清理**：删除已无引用的 i18n 键 `settings.cost_range_dates`（日期组标签）。
- **验证**：`npx tsc --noEmit` exit 0；dev + CDP 实测：面板内 `.custom-select` 3 个、
  原生 `select` 0 个，标签「总计 / 全部 / 今天」，trigger 底色 `rgba(207,200,187,.09)`、
  描边 `rgba(207,200,187,.14)`、字号 11.2px（全走四类变量）；展开后 `overflow` 由 auto→visible、
  弹层 `display:block`；选「近 7 天」后标签更新、弹层关闭、`overflow` 还原 auto、表格重算 4 行。

## 本轮改动记录（2026-10-27 · 任务抽屉页签计数 + 文件转换插件独立窗）

> 用户口径：① 待办抽屉三个页签按钮上要显示该页数量（语义红）；② 设置面板「一键添加配置」按钮
> 与「测试联通 / 保存」同款同样高、排一行、文案改「命令添加」；③ 文件转换插件重构为独立窗口。

- **页签计数**：`main.ts::renderTodoDrawer()` 的页签按钮改成「文本节点 + `<span class="todo-tab-count">(n)</span>`」，
  `n = 0` 不插（与 `.empty` 态同义）。样式 `#todo-drawer-tabs .todo-tab-count` 只用
  `var(--danger)` + `tabular-nums`，**不新造色值**。
- **设置面板按钮归位**：`settings.ts` 里 `#settings-add-curl-btn` 从独立 `settings-row` 移进
  `justify-content: flex-end` 那一行（「命令添加 | 测试联通 | 保存」），三个按钮统一
  `.settings-install-btn` / `.settings-save-btn`，不另写尺寸。
- **文件转换插件（独立窗）**：`convert.ts` 重写为「最左四类页签（88）+ 输入框（300×250）+
  中间图标/条形进度（48）+ 输出框（300×250）」，列间距 10 ⇒ 内容 766，窗口 820×470（下限 780×430，
  清单 `window` 段与 `plugin_window.rs` 的 `CONVERT_W/H` 同口径）。中间是**图标 ↔ 进度条互斥**：
  转换中隐藏图标、显示 40×6 条形条 + 上方百分比；时长未知时走 `.indeterminate` 来回扫，
  **不编假百分比**（沿用旧 `.convert-progress-fill.indeterminate` 的口径）。
  多文件用泡泡框（`.convert-bubble`，queued / running / done / error 四态只改描边与字色，
  输入侧泡泡带 `.convert-bubble-x`），**不显示缩略图**；输出框**不写提示文本**。
- **样式纪律**：新类全部落在 `styles.css`（唯一真相源），旧 `.convert-root` / `.convert-btn` /
  `.convert-progress*` 等一次性删除；按钮仍**不写底色**（全局 `#app button:where(...)` 已统一）。
- **验证**：`npx tsc --noEmit` exit 0；`cargo check --bins` exit 0；`convert` 单测 5 passed；
  插件包 `convert-0.9.9.zip` 构建通过。**未做真机拖放 / 点击验证**。

## 本轮改动记录（2026-10-27 · 用量成本面板三列派生）

> 依据：`cache-hit-experiment.md` §6.4 第 9 条（光看总量判断不了钱花在哪）。

- **三列**：`usage-cost.ts` 的 `COL_KEYS` 在「金额」后追加 `未命中/请求` / `输出/请求` /
  `成本占比`（i18n 五语言，键 `settings.cost_col_miss_per_req` / `_out_per_req` / `_share`）。
  前两列用 `fmtTokenCount` 缩写（与主界面表盘同风格）；占比一位小数。
- **空值口径**：请求数为 0 或该范围没有定价时显示 `—`，**不编 0% / 0**；合计行占比恒 100%，
  两个均值用总量重算（不是各行简单平均）。
- **不改样式**：表格仍是 `width:100%` + `white-space:nowrap`，列变多时由
  `#token-usage-panel` 的 `overflow:auto` 横向滚动，不新造类名。

## 本轮改动记录（2026-10-27 · 插件下载进度改为「整次安装」总进度）

> 用户报障：**桌宠下载时进度环不断抖动**。根因不是网络抖动，而是**进度口径按文件**。

- **根因（三层都在，最后一层是主因）**：宿主 `plugin_market::install_dependencies_with_progress`
  是**逐个文件**下载的（插件包本体 → 依赖 1 → 依赖 2 …），每条 `DependencyProgress` 的
  `downloaded/total` 只针对**当前这个文件**；而前端原先把它当整次安装的百分比画在环上 ⇒
  每个文件结尾都会 100%，下一个文件又从 0% 起。桌宠正是最坏的形状（Live2D Core 几十 KB，
  紧接着 ZipVoice 149 MB），一次安装里要来回 3 次；再叠加「解压时切回不确定态」，
  看上去就像进度条在抖。**宿主一行未改**（它只报事实，本来就是对的）。
- **修法**：`download-progress.ts` 新增 `InstallOverall`（与既有 `DownloadMeter` 同文件、仍零依赖）：
  - 把整次安装折成**等权的 N 步**（插件包算一步、每条依赖算一步），
    总进度 =（已完成步数 + 当前步内比例）/ 总步数 ⇒ **只增不减**。
  - 步数还不知道的那一段（插件包本体，宿主给 `count = 0`）回 **`-1`**（环转圈、不写数字）——
    宁可这一段不给数字，也不先画 100% 再掉回 0%。
  - **解压阶段不再退回不确定态**：停在这条依赖「已完成」的刻度上（`percent` 取该步末尾），
    文案仍是「解压中」—— 环不再 100 → 转圈 → 100。
  - **速度不按文件清零**：换文件时把上一个文件的字节并入基线（`offset`），否则每次换文件
    都会先显示 `—`，看着像断了；「共下多少」的文案也随之跨文件累加。
- **三处消费者全部切过去**（口径必须一致，否则同一屏两套数字）：`settings.ts` 的市场安装行、
  `local-model.ts` 的依赖补齐提示、`ocr-engine.ts`（经 `ocr` 插件那个独立 bundle）。
- **版本**：`build-plugins.ps1` 的 `ocr` **0.9.10 → 0.9.11**（`download-progress` 会被打进 ocr 的
  独立 bundle ⇒ 源码变了就得让市场判为「有更新」）；`settings.ts` / `local-model.ts` 属主 bundle，
  不涉及插件版本。
- **验证**：`npx tsc --noEmit` exit 0；`npm test`（`app`）**17 passed**（新增 `download-progress.test.ts`
  5 条：本体段不给数字 / 桌宠形状只增不减 / 只补依赖按条数算步 / 解压不退回 0 / 已下字节跨文件累加）；
  `build-plugins.ps1` 全量 exit 0（`ocr v0.9.11`）。
- **未做真机点击**：真机验收口径 = 从市场装一次桌宠，环应**单调爬升**（本体段转圈 → Live2D Core
  推到 ~33% → ZipVoice 一路到 100%），换文件那一刻**不回退**。

## 本轮改动记录（2026-10-27 · 插件下载：暂停 / 继续 / 断点续传）

> 主机侧与全部边界见 [`todo.md` §待办 3-18](./todo.md)。这里只记**界面**侧。

- **行内进度常驻**：设置 → 插件市场那一行在安装期间显示「环 + 阶段 + 速度 + **暂停 / 继续**」
  （新增一条尺寸覆盖 `.settings-market-progress .settings-install-btn`，比行内按钮小一档）。
  进度状态改成**模块级快照 + 全局单监听** ⇒ 切页签 / 关面板 / 重开面板后重绘出来的行
  会由 `mountMarketInstall()` 重新挂上，不再「一重绘进度就没了」。
- **暂停态**：阶段文字「已暂停」、速度显示 `—`；按钮文案在「暂停 ⇄ 继续」之间切换
  （调宿主 `plugin_install_pause`，宿主那边是断开连接原地等，点继续后按 `Range` 接上）。
- **断点**：磁盘上还有半截文件时，该行按钮文案是「**继续下载 (37%)**」（无总长时显示已下 MB），
  点一下即从断点续 —— **强制退出 Lunac 后重开**也是这个形态。
- **一次只跑一个安装（2026-10-27 复测修复）**：点「下载 / 继续」时**先查宿主** `plugin_install_state` ——
  已有活动任务且 `url`/`id` 命中本行，就**只做「继续 + 重挂进度」**，不再发安装。
  原因：暂停是宿主线程原地等，重复点会起**第二个**下载线程写同一个 `.part` ⇒ 界面出现一快一慢两条进度，
  且可能把半截文件当成品（「秒更新完毕、其实没下完」）。宿主侧另有安装闸兜底（`try_begin_install`）。
- **不新造样式体系**：复用既有 `.dl-inline` / `dl-ring` / `.settings-install-btn` 与四类颜色变量。

## 本轮改动记录（2026-10-27 · 定时任务「无人值守可写」开关）

> 主机侧与全部边界见 [`todo.md` §待办 3-1d 阶段 2B](./todo.md)。这里只记**界面**侧。

- **位置**：聊天窗「更多设置 ⋯」→「其他」分页的定时任务列表，**每行**在「立即运行」右侧多一颗
  `允许写文件` 按钮（`.chat-cron-write`）。不新增面板、不新增第二套列表。
- **危险方向二次确认**：**开**要点两次 —— 第一次就地变成红字「再点一次确认」（`data-armed="1"`，
  4 秒后自动回弹，与卸载 / 自动档同一套「就地二次确认」形态，不用原生 `confirm`）；
  **关**一次即可（收回权限不需要再问一遍）。点第二次才 `invoke("cron_set_write_access", …)`。
- **开着时的可见性**：该行的元信息里多一条语义红的 `无人值守 + 可写` 徽标（`.chat-cron-write-badge`），
  按钮同时转红（`.chat-cron-write.on`）——这是本应用里唯一「没人看着也会改磁盘」的状态，
  不能只靠按钮文字暗示。关着时按钮走 `--text-dim`，不抢注意力。
- **颜色**：只用 `var(--red)` 与 `rgba(var(--danger-rgb), 0.35)`，与「自动运行」那颗红点同源；
  不硬编码 rgb、不新增主题变量。
- **文案**（五语言，`settings.cron_write_*`）：`enable` / `confirm` / `disable` / `badge` / `hint`
  （`hint` 挂在按钮 `title` 上，如实写明「只放开 Write / Edit / NotebookEdit，命令类与联网工具仍被拒绝」）。

## 本轮改动记录（2026-10-29 · 「在文件管理器中打开」只打开文档目录）

> 现象：AI 插件输出结果卡 / 工具卡里点文件链接或「在文件管理器中打开」，资源管理器打开的是
> **「文档」目录**，既不定位也不选中目标文件。用户报障（dev 实例）。

- **根因**：路径带 **Windows verbatim 前缀** `\\?\`。`get_agent_workdir` 在含空格 / 中文的工作区
  （本例 `D:\新建文件夹 (2)`）上返回 `\\?\D:\…`，前端 `absAgentPath()` 又拿它当基准拼相对路径
  ⇒ `data-path` 一路带着前缀。该前缀能过 `Path::exists()`，但 **`explorer.exe /select,` 与
  `cmd /c start` 都不认** —— explorer 的表现就是静默退回默认文件夹（文档）。
  2026-10-27 记的「不存在的路径会退化成打开文档目录」只覆盖了**不存在**这一种，没覆盖
  **存在但形式不被 explorer 接受**。
- **修法（两道）**：
  1. 宿主侧 `commands.rs::host_visible_path()`：去 `\\?\` / `\\?\UNC\`（还原成 `\\`）、`/` → `\`、
     折叠重复分隔符（保留 UNC 开头双斜杠）。`reveal_in_explorer` 与 `open_path` 都在
     **存在性校验与传参之前**过它 —— 不依赖调用方是否已经归一。
  2. 前端 `main.ts::plainPath()`：`refreshAgentWorkdir()` 拿到工作目录就先剥前缀，
     后续 `absAgentPath()` 拼出来的路径天然是普通形式（快照 / diff / 链接同一份）。
- **边界**：只做形式归一，不做 `canonicalize()`（那会把 verbatim 前缀又加回来）；
  含引号的路径仍按原规则拒绝（`open_path` 拼 `cmd /c start` 时引号会破坏结构）。
- **验证**：`cargo test --bin lunac host_visible_path` 1 passed（盘符 verbatim / UNC verbatim /
  正斜杠 / 重复分隔符 / 已是普通形式 5 种输入）；`npx tsc --noEmit` exit 0。
- **未做**：真机点一次「工具卡文件链接 → 资源管理器选中该文件」（需重启 dev 让新 Rust 产物生效）。

## 本轮改动记录（2026-10-30 · 音乐插件本地 WebM + 阅读插件「音画同起」的文案）

> 两条都属于「用户看到的一句话 / 听到的一秒差」，视觉结构一处没动。

- **音乐插件：本地 WebM**（行为与契约见 `docs/testing.md` §测试 22、`media_lib.rs` 的
  `VIDEO_EXTS` 注释）。新增三条文案，**5 语言齐发**，措辞只讲「缺什么 + 怎么办」，
  不出现 ffmpeg 命令与英文解码器名：
  - `music.local_no_ffmpeg`：播放 `.webm` 需要本机 ffmpeg（宿主用它抽音轨）—— 放 exe 根或装到
    PATH 后重试；
  - `music.local_ffmpeg_failed`：抽音轨失败 —— 文件音轨可能损坏，或 ffmpeg 版本太旧；
  - `music.local_webm_warmup`：首次播放 WebM 要先抽取音轨，可能要十几秒…
- **文案落点**：`localErrText()` 把宿主的 `ERR_NO_FFMPEG` / `ERR_FFMPEG_RUN` 翻成人话
  （原样贴会带一串英文与路径）；warmup 那句在 `startLocalPlay()` 里**发命令之前**发，
  否则十几秒没有任何反馈 = 看起来卡死。
- **阅读插件播放器：不再「先出声、后出画面」**（细节见 `docs/resource-hub-design.md` §13.5）。
  没有新增文案：起播沿用「正在加载视频…」，**改由首帧到达才撤掉**，声音同步等到首帧。
- **验证**：`npx tsc --noEmit` exit 0；`npm test` 17 passed；`cargo check --bins` 绿；
  `cargo test --bin lunac video` 6 passed；ffmpeg 抽轨命令行实测（VP9+Opus → AAC 立体声 74 KB/3s）。
- **未做**：真机放一次 `.webm`（需重启 dev 让新 Rust 产物生效）。

## 本轮改动记录（2026-10-31 · 任务抽屉待办列表的「依赖」状态）

> 起因：`TodoWrite` 支持了 `blocked_by`（§待办 3-22），模型能写依赖图，但面板上所有待办长一个样，
> 用户看不出「这条为什么没动静」。本轮只给已有的待办行加一层状态表达，**不加新控件、不加新颜色**。

- **形态**：`.todo-item` 新增两个修饰类 —— `.blocked`（前置未完成）与 `.ready`（前置都完成、可以开工）；
  行尾追加一枚文本标签 `.todo-dep`，内容为「等前置」/「可开始」。
  - `.blocked` 的标签走 `--text-muted`，与 `pending` 的灰同源，属「说明」不属「告警」；
  - `.ready` 的标签与 `.todo-state` 圆点边框转 `rgb(var(--accent-rgb))` —— 提示「这里可以动」，
    仍弱于 `in_progress` 的实心圆点。
  - `.todo-dep` 固定 `flex: none; margin-left: auto`，标签永远贴行尾，长文案先挤文本列。
- **判定逻辑不在样式里**：`app/src/todo-deps.ts` 的 `decorateTodos()` 是纯函数（有 4 条单测），
  负责把「被谁挡住 / 是否可开始」算出来；`main.ts` 只负责拼 class 与标签。前端不维护第二份状态。
- **文案**（五语言）：`agent.todo_blocked`（等前置 / 前提待ち / 선행 대기 / blocked）、
  `agent.todo_ready`（可开始 / 着手可 / 시작 가능 / ready）。`blocked` 的 `title` 直接用缺的前置
  id 列表（`esc()` 转义后拼逗号），不额外造句子。
- **边界**：`completed` 与 `in_progress` 永远不加 `.ready`；未知前置 id 按「未完成」算，
  宁可多标一次 `.blocked`，也不要出现「看着能开工、点下去被拒」。
- **验证**：`npx tsc --noEmit` exit 0；`npm test` 21 passed（含 `todo-deps` 4 条）。
- **未做**：真机看一眼面板观感（长文案 + 标签并列时是否会挤）。

## 本轮改动记录（2026-11-01 · 代理插件三段式面板 + 详细搜索退场动画订正）

> 用户口径（2026-11-01）：代理插件的独立面板改成 **1280×720**，竖直分三段 ——
> 上「工具区」、中「服务器抽屉 + 大字开关」、下「可拖拽日志」；界面上不再出现 mihomo 字样，
> 引擎编进宿主、随安装包分发。另：详细搜索里打开应用 / 文件时**不播退场动画**。

- **窗口定尺**：`build-plugins.ps1` 的 proxy 清单新增
  `window = { width:1280, height:720, minWidth:960, minHeight:600 }`
  （`plugin_window.rs` 的缺省档是 640×560；不声明就会挤成两行）。插件版本 `0.9.6 → 0.10.0`。
- **三段式**（`styles.css` 的 `.px-*` 段，旧的 `.proxy-*` 整段删除，不留死样式）：
  - `.px-top`：品牌 + 四枚工具按钮（服务器导入 / 服务器订阅更新 / 测试连通性 / 设置），按钮点开 `.px-menu` 下拉，
    菜单项打开 `.px-layer` 里的**界面内独立面板**（不是浏览器弹窗、不是原生 `<select>`）；
    四枚按钮拼成一个 `.px-tool-group`（2026-11-03）：只有内部分隔线，外圈无边框；
  - `.px-stage`：中央 `.px-orb`（168px 圆形「开启 / 关闭服务」）+ 上传 / 下载速率 + 地区状态行；
    底部「模式（规则 / 全局 / 直连）+ TUN 开关」；左侧 `.px-rail` 是常隐藏抽屉的拉手，
    抽屉展开后铺满中间区（`.px-drawer.open`），右上「拉回」收回；
  - `.px-log`：高度由 `--px-log-h` 控制，顶部 `.px-log-handle` 拖拽调整（持久化到 localStorage），
    右侧可折叠；每行是「时间 · 目标域名:端口 · 走向（订阅 / 节点 / 直连 / 拒绝）」。
- **单一滚动面**：`.proxy-panel { overflow:hidden }`，只有抽屉列表与日志列表各自滚动 —— 面板本身
  不滚（`forbidden.md` §禁用 2）。窗口 1280×720 下三段刚好放下；窄于 960 时由窗口最小尺寸挡住。
- **TUN 的诚实边界**：开关已接线到 `mihomo_set_tun`，但当前引擎没有 TUN（需 wintun 驱动 +
  用户态 TCP/IP 栈）⇒ 宿主**返回可操作错误**、前端回弹开关，不假装开启。
- **详细搜索**：`playResultExit()` 删掉 `allowInDetail` 分支；`launchApp(..., {fromDetail:true})`
  直接 `exitMs = 0`，窗口隐藏后再收大界面。`ui-style.md` §界面 6 早先就写着「详细搜索不播」，
  本轮把代码对齐到文档（此前代码与文档相反）。
- **验证**：`cargo test --bin lunac` 411 passed；`cargo check --bin lunac` exit 0；
  `npx tsc --noEmit` exit 0；`build-plugins.ps1` 通过（`proxy-0.10.0`）。
- **未做**：真机看 1280×720 观感与拖拽手感；真正的 TUN（单独里程碑：wintun + 用户态协议栈）。

### 订正（2026-11-02 · 代理面板无节点时中间区被日志挤没）

- **现象**：没有节点时，面板父级高度按内容算（auto），而 `.px-stage` 的唯一子项 `.px-center`
  是 `position:absolute; inset:0`，不贡献高度 ⇒ stage 塌成 0，只剩固定 190px 的日志区；
  拖拽上限又按正在缩小的 stage 高度算，越拖越小，表现为「日志锁死、拖不动、中间看不见」。
- **修复**：
  - `#results-list:has(.proxy-panel)` 改为纵向 flex 容器，`.proxy-panel` 作为 flex 子项
    `flex:1 1 auto; min-height:0` 吃满剩余高度（父级定高时才分得到空间）；
  - `.px-center` 改为在流内（`position:relative; flex:1 1 auto; min-height:0`），
    父级暂时没有定高时也至少有内容高度兜底；`.px-stage` 同步改为纵向 flex；
  - `.px-log-handle` 命中区 10 → 12px，补 `touch-action:none` / `user-select:none` / `z-index`，
    并补 `pointercancel` 收尾（触摸手势/失焦时不会留下旧起点）；
  - 拖拽上限改为按**面板总高**算（至少给中间区留 320px），不再按正在缩小的 stage 算，
    避免「越拖越小」。
- **验证**：headless Chromium 复现（730×601 视口）：panel 596 / stage 320 / center 318 /
  log 190（修复前 stage 为 0）；`npx tsc --noEmit` exit 0；`build-plugins.ps1 -Plugin proxy` 通过。
- **未做**：真机 1280×720 的拖拽手感与观感。

### 订正（2026-11-02 · 代理面板：日志贴底 / 去掉 native / 拉回拉手 / 拖动失效）

- **日志区贴底**：`.proxy-panel` 底部内边距 8 → **0**；`.px-msg` 去掉 `min-height:14`，
  改为 `:empty { display:none }`（没有消息时不占 flex gap 与那一行高）。修复后日志区的
  下沿 = 面板下沿（headless 实测 `gapBelowLog = 0`；修复前 30px）。
- **左上 native 标签**：删除 `.px-brand` 里的 `<span class="px-engine">native</span>`
  与 `.px-engine` 整条 CSS（设置页的「引擎：native」是另一处信息行，保留）。
- **抽屉「拉回」**：从 `.px-drawer-head` 的普通按钮改为与左侧「服务器」拉手同形同位的
  `.px-rail.px-rail-close`：竖向、`height:60px`、内容为 `<` 线性 SVG（不再是文字），
  `z-index:7` 盖在抽屉之上；`setDrawer()` 与展开拉手互斥显隐。**第二轮（同日）把位置改到
  抽屉右端**（`right:0`、圆角 `8px 0 0 8px`），见下一条。
- **拖动失效**：`pointermove/pointerup` 改挂 `window`（不再依赖 `setPointerCapture` 与命中元素），
  命中区 12 → 16px；上限改为 `面板高 − 顶区 − 消息行 − 220 − 24`（原 `−360` 在小窗口下
  会跌到下限，表现为「拉不动」）。
- **验证**：headless Chromium 1280×720：BEFORE log 190 / `gapBelowLog 0`；向上拖 60px ⇒ 250；
  向下拖 80px ⇒ 170（修复前上限受 360 预留影响，小窗拖不动）；`npx tsc --noEmit` exit 0。
- **未做**：真机拖拽手感（需重开代理窗，加载 0.10.1）。

### 订正（2026-11-02 · 代理面板第二轮：日志常开 / 拉回在右 / 设置收敛 + 域名编辑）

> 用户口径（2026-11-02）：请求日志默认展开、不要展开按钮；拉回拉手放到抽屉右端并只用
> `<` 图标（高 60）；抽屉不再显示服务器数量、不再放刷新按钮（刷新只保留上方工具区）；
> 设置里去掉「系统代理」和「本机播放也走代理」，改成符合自研内核的域名配置。
> 另：网页搜索右键菜单的引擎图标去掉，只留文字。

- **日志区**：删除 `#px-log-toggle`（展开 / 收起）与 localStorage 的 `logCollapsed`；
  日志**永远展开**，只有高度可拖（`--px-log-h`）。
- **拉回拉手**：`.px-rail-close` 改为贴抽屉**右端**（`left:auto; right:0`、
  `border-radius:8px 0 0 8px`），`height:60px`、宽 26、内容是 `<` 线性 SVG；
  与左侧「服务器」拉手互斥显隐。
- **抽屉去杂项**：删除 `#px-drawer-count`（右上数量）与 `#px-drawer-refresh`
  （底部「刷新服务器」）；订阅刷新只保留工具区「服务器订阅更新」。
- **设置页收敛**：
  - 删除「系统代理」开关 —— 中间区大按钮 `mihomo_start` / `mihomo_stop` 已经**一并**
    启用 / 还原系统代理（见 `mihomo.rs`），设置里那只是重复入口；
  - 删除「本机播放也走代理」开关 —— 播放流量按规则走（`rule` 模式），不再单独掰 librespot；
  - 新增**直连域名 / 代理域名**两个编辑框（一行一个）：保存进 `config\proxy.json` 的
    `mihomo.direct_domains` / `proxy_domains`，引擎在 `rule` 模式下展开成
    `DOMAIN-SUFFIX,…,DIRECT` / `…,PROXY`，且**排在订阅规则之前**（用户手填优先）；
    改完需重启引擎生效。同一保存按钮里还可以改**混合端口**（`0` = 启动时自动挑空闲端口）；
    内核 / 宿主日志两行保持只读。
- **拖动（WebView2）**：同时接 `pointerdown/move/up` 与 `mousedown/move/up` 两套
  （WebView2 里 pointer capture 被系统打断时 pointermove 会整段丢失，表现是
  「指针变 ns-resize 但高度不跟」）；`dragging` 守卫保证兼容事件只开始一次。
- **网页搜索右键菜单**：`SEARCH_ENGINES` 去掉 `icon` 字段，菜单只渲染引擎名文字。
- **验证**：headless 加载**真实构建产物**（`plugin-dist/proxy/index.js` + 真实 `styles.css`）：
  1280×720 下 `gapBelowLog = 0`、无 toggle / count / refresh / sys / link 元素、域名编辑框存在；
  点开抽屉后拉回拉手 `h=60 / w=26 / right≈stage.right`；日志 190 → 250 可拖。
  `npx tsc --noEmit` exit 0；`cargo check --bin lunac`（target-check）exit 0；
  `cargo test --bin lunac native_proxy::` 56 passed（含新增 `user_domain_rules_beat_subscription_rules`）；
  `build-plugins.ps1 -Plugin proxy` ⇒ `proxy 0.10.3`。
- **未做**：真机手感（勾子 / 拖拽 / 域名规则实际分流需连节点验证）。

### 订正（2026-11-02 · 代理面板第三轮：规则配置文件 / 模式两档 / TUN 管理员闸门 / 订阅即刷新）

> 用户口径（2026-11-02）：规则模式要在订阅规则之外**再由插件过滤一层**；设置里的「编辑
> 配置文件」就是这一层，形态是**一份域名清单 + 黑/白名单滑轨**（黑名单 = 不让代理走这个
> 域名，白名单 = 让代理走这个域名），并给出常见的国内默认清单；模式切换只留
> **规则模式 / 全局代理** 两档；TUN 打开前必须弹**独立警告框**申请管理员权限，同意后以
> 管理员身份重启 Lunac（锁定 UAC），**同意之后才允许开启 TUN**；「服务器订阅更新」不需要
> 独立面板，它就是把服务器列表刷新一遍。

- **规则分层**（`src-tauri/src/native_proxy/clash.rs`）：`rule` 模式的规则顺序改为
  `lan_rules()`（本机 / 私有网段直连）→ **插件名单层** → 订阅规则 → `fallback = Proxy`。
  `decide()` 先匹配先赢，所以名单层**必须排在订阅规则之前**：名单内按语义改写走向，
  名单外仍由订阅规则决定。名单统一展开成 `DOMAIN-SUFFIX,<domain>,DIRECT|PROXY`。
- **配置字段**（`src-tauri/src/mihomo.rs`）：`direct_domains` / `proxy_domains` 两个字段
  下线（上一轮的东西），换成 `rule_mode`（`blacklist` | `whitelist`）+
  `rule_domains`（`Vec<String>`）。两者都带 `#[serde(default …)]`，**旧配置读回即获得默认
  国内清单**（约 90 条：`cn`、`baidu.com`、`qq.com`、`bilibili.com`、`taobao.com`、
  `alipay.com`、`jd.com`、`163.com`、`douyin.com`、`zhihu.com`、`12306.cn` 等），
  只放「按域名能判断」的条目，不引 GEOIP 数据库。默认语义 = `blacklist`（国内站点直连）。
- **设置页**：`直连域名 / 代理域名` 两个编辑框合并为「规则配置文件」一行 ——
  标签 + 黑/白名单文案 + 滑轨开关（`#px-rule-mode`），下方一个域名清单
  （`#px-rule-domains`，一行一个，`#` / `//` 开头的行忽略）。滑轨只改文案，
  **点「保存」才落盘**（`mihomo_config_set` 带 `rule_mode` / `rule_domains`）。
- **模式两档**：中央区菜单删掉 `direct`，只剩「规则模式 / 全局代理」；旧配置里的
  `direct` 不再有入口（引擎仍按磁盘配置执行，界面按「规则模式」显示）。
- **TUN 管理员闸门**（新增命令 `mihomo_is_elevated` / `mihomo_relaunch_elevated`）：
  打开开关时先问宿主是否已提权；未提权则**开关回弹**并弹出独立 `.px-modal` 警告框
  （标题 / 正文 / 「取消」「同意并重启」）。同意后宿主机以隐藏 PowerShell 执行
  `Wait-Process -Id <pid>; Start-Process -Path <exe> -Verb RunAs`，随后 `app.exit(0)`
  —— 必须先等旧进程退出，否则撞单实例守卫；重启后再打开 TUN 才会真正下发配置。
  **dev 不支持自动提权重启（2026-11-02 最终口径）**：dev 的 debug 构建是 console 子系统，
  且前端是 `devUrl`（`http://localhost:5173`）的 vite 子进程，旧进程退出后会一起被收掉；
  另起 `npm run tauri:dev` 实测也无法可靠拉起。因此 dev 只返回可操作提示（用管理员终端重启
  `npm run tauri:dev`）、**不退出进程**，TUN 警告框隐藏「同意并重启」、只留「知道了」
  （由 `mihomo_is_dev_build` 判定）。release 仍重启自身 exe（`-WindowStyle Hidden`）。
  **边界**：TUN 引擎本身仍未实现（缺 wintun + 用户态 TCP/IP 栈），提权后关闭仍会
  如实报错并回弹，不假装成功（`docs/forbidden.md` §禁用 1）。
- **服务器订阅更新**：删除整个 `data-panel="subs"` 面板与 `renderSubs()` /
  `doSubAdd` / `doSubDel` / `#px-subs*` 绑定；工具区按钮改为
  `data-action="refresh-subs"`，点击直接 `mihomo_refresh_subs` + 行内提示（就是刷新服务器）。
- **日志拖拽（第三次修复）**：改回**只用 mouse 事件**（`mousedown/mousemove/mouseup/blur`）。
  上一版 pointer + mouse 双通道有个致命交错：WebView2 在 `pointerdown` 后可能发
  `pointercancel`，它被当作「拖动结束」⇒ 刚由 mousedown 建立的拖动被立刻拆掉，表现正是
  「指针变 ns-resize、高度不跟」。现在 `mousemove` 里再用 `ev.buttons` 兜底（在窗口外
  松开也能恢复），上限下限收敛到 96 / 80。
- **验证**：`npx tsc --noEmit` exit 0；`cargo check --bin lunac`（`target-check`）exit 0；
  `cargo test --bin lunac native_proxy::` **56 passed**（`rule_list_switches_between_blacklist_and_whitelist`
  锁定黑/白语义：黑名单命中 ⇒ Direct、白名单命中 ⇒ Proxy）；`build-plugins.ps1 -Plugin proxy`
  ⇒ `proxy 0.10.5`（dev 下 TUN 闸门只提示手动重启）。
- **未做**：真机验证名单分流与 UAC 重启链路（需连节点 + 非管理员启动一次）。

### 订正（2026-11-03 · 工具区按钮组 + 「测试连通性」）

> 用户口径：把「测试连通性（测试节点的延迟）」放在**设置按钮左边**，并把工具区四颗按钮
> 做成一个按钮组，**外圈无边框**。

- **按钮组**（`styles.css` `.px-*` 段）：`panelHtml()` 把四个 `.px-tool` 包进 `.px-tool-group`；
  组内 `.px-tool-btn` 去掉边框与圆角、背景透明，相邻项用 `box-shadow: inset 1px 0 0 …`
  画内部分隔线，只有首尾两颗按钮保留外侧圆角。悬停只变底色，不再出现四颗独立小按钮。
- **测试连通性**（`proxy.ts` `doTestDelays()`）：复用宿主 `mihomo_delay`（**经节点**取
  直连可达的 `www.baidu.com/`，见 `native_proxy::delay`），逐个节点串行测；
  测速期间按钮置灰并显示「测试中…」，结束后行内提示 `{ok}/{n}`。
- **结果展示**：抽屉的每个节点右侧新增 `.px-delay` —— ≤150ms 绿色、≥400ms 红色、
  失败显示 `×`（title 说明「经节点连直连网站失败」）；`detach()` 清空结果。
- **前置条件（2026-11-03 放宽）**：测速只需要**引擎**，不需要开启服务 —— 打开面板会
  静默拉起引擎（`mihomo_engine_start`）；引擎还没就绪时点按钮会先补一次启动，
  真起不来才提示「引擎还没就绪」。
- **验证**：`npx tsc --noEmit` exit 0；i18n 五语言新增 `proxy.px_tool_delay` /
  `px_delay_testing` / `px_delay_need_running` / `px_delay_no_nodes` / `px_delay_done` /
  `px_delay_failed` / `px_state_ready` / `px_pill_on` / `px_pill_off`。
- **未做**：真机看按钮组观感与测速结果颜色。

### 订正（2026-11-03 · 状态栏「代理:开/关」泡泡框）

> 用户口径：搜索区状态栏里、「AI 思考…」那段状态文字的**右边**放一枚代理状态泡泡框，
> 内容为 `代理:开/关`。

- **结构**：`index.html` 的 `#status-bar` 左侧新增 `#status-left` 包住 `#status-text` 与
  `#proxy-state-pill`（`space-between` 仍保持「左组 / 表盘 / Esc 提示」三段，泡泡不会
  被甩到窗口中间）。
- **取值**：只表示**服务（系统代理）**开/关（`MihomoStatus.enabled`），**不表示引擎**；
  引擎为测延迟在后台跑着、但服务没开时，仍显示「代理:关」。
- **更新**：主窗启动读一次 `mihomo_status`；代理插件每次刷新 / 开关服务后广播
  `proxy-service-state`，主窗收到即改文案与 `.on`。
- **样式**：胶囊（`border-radius:999px`）、`0.62rem`；关 = 中性描边 +
  `rgba(var(--shade-rgb), …)` 遮罩，开 = 语义绿（`--green` + 0.12 底）。
  文案走 i18n `proxy.px_pill_on` / `proxy.px_pill_off`（五语言）。

## 本轮改动记录（2026-11-02 · 简洁搜索结果区只留图标 + 标题）

> 用户口径（2026-11-02）：搜索结果区里的「提示」全部清除 —— 键入文本后，除了
> AI 助手 / 备忘录 / 网页搜索 这些入口的标题，行下方的灰色说明一个字都不留。
> **同日回改**：这三行要显示「检测内容」，网页搜索固定为 `搜索引擎:内容`。

- **改动**（`app/src/main.ts`）：
  - 空查询：`renderAIEntry` 的 AI 行、`renderClipboardOCREntry` 的 OCR 行副标题移除；
  - 附文件分支：OCR / 文件转换 / 自定义启动行副标题移除；**AI 行保留副标题**（显示键入内容）；
  - `buildWebSearchItem`：副标题改为 **`搜索引擎:内容`**（空查询不显示），右键切引擎后同步；
  - `renderMixedResults`：应用行（「启动应用」）、插件行（插件描述）、标识直达行的副标题移除；
    **AI 行 / 备忘录行保留副标题**（显示键入内容）。
- **不动的副标题**：详细搜索面板（`#detail-results`）的行标签、`quick-launch` 面板的路径 ——
  它们不属于简洁结果区（见 §界面 10 本日条目）。
- **高度**：行高仍 56px（36px 图标 + 20px 上下 padding），`applyWindowSize` 的实测口径不变。
- **验证**：`npx tsc --noEmit` exit 0。
- **未做**：真机看一眼三行副标题的截断（长查询按 40/60 字截断）。

## 本轮改动记录（2026-11-02 · 对话流中途三态控制：排队 / 立即插入 / 暂停）

> 来源：§待办 3-28.1。目标 = 生成中也能改变方向，且**不丢已完成的工作**。
> 不引入新滚动容器、不改 `#chat-log` 的单滚动面（`forbidden.md` §禁用 2）。

- **入口**：聊天「更多设置 → 其他 → 生成中发送」一行，**点一下循环三档**
  （`#chat-steer-btn`，与「运行方式 / 权限」同一交互），存 `localStorage["lunac-chat-steer"]`，
  默认 `queue`（= 旧行为）。
  - `queue`：本轮结束后按序发出（`pendingMessages`，原样）。
  - `insert`：生成中按发送 ⇒ `send_message` 写一条带 `steer:"insert"` 的 `user`；
    agent 在下一个**轮首安全点**注入 history，已完成工具不重跑。
  - `pause`：生成中按发送 ⇒ 发 `{"type":"pause"}`；**输入框文字保留**，回合在安全点收口。
- **反馈**：插入成功在当前过程块里补一条 `.sys-note.sys-note-line`（「已插入：…」），
  不新建 turn（会切断正在写的流）；暂停收口时补一条「已暂停 · 本轮工作已保留」，
  `result.subtype="paused"` **不按错误行**渲染；没赶上安全点则提示「插入未生效，请重新发送」。
- **队列条**：`chat.queue_send_now` 文案改为「立即插入（不中断当前回合）」；点击不再
  `stopAIChat` 重发，而是走插入通道（只有附件占位 `(files only)` 仍走旧的重发路径）。
- **边界**：附件消息暂不支持插入（只能排队）；暂停不是「停止」——不丢 history、不回溯工具，
  而是落检查点后收口，后续提问或 `--resume` 都可续。
- **验证**：`npx tsc --noEmit` exit 0；`cargo test`（core-agent）461 passed / 15 ignored；
  `cargo build --release` exit 0。**待真机**点一遍三态与 `--resume`。






