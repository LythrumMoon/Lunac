# 主题配色与图标设计（2026-10-21）

> **本文档是什么**：依据 [`theme-impression-design.md`](./theme-impression-design.md) 的气质转译结论，
> 给出**一套可落地的主题配色 token**与**插件图标设计**；并记录 2026-10-21 实际落地的两处图标改动。
>
> **不改变既有硬规则**：[`ui-style.md`](./ui-style.md) §界面 1「只有一套固定深色主题、不新增主题切换机制」、
> §界面 4「严格线性 SVG」、§界面 5「emoji 纪律」、§界面 11「四类颜色是 UI 颜色的唯一来源」全部继续有效。
> 本文的配色是**token 值的替换提案**，不是新增主题包 / 切换机制。
>
> **取用边界**（沿用 2026-10-18 裁定）：页面配色**不取自参考图**。参考图只提供人物气质、笔触肌理与光影节奏，
> 落到 icon 的残影 / 点彩与动效的呼吸感上。

---

## 一、主题配色

### 1.1 方案对照

| | 方案 A（现行默认，2026-10-18 定稿） | 方案 B（本文提案，2026-10-21） |
|---|---|---|
| 命名 | 石墨 · 香槟银 | 雾银 · 冷石墨 |
| 底色 | `#16181b`（中性，略偏暖） | `#14171a`（中性，略偏冷） |
| 高亮 | `#cfc8bb` 香槟银（暖） | `#c6ced2` 雾银（冷、更低饱和） |
| 气质 | 室内暖光下的石墨台面 | 水泽旁的冷银雾面 |
| 取舍 | 暖灰与暖银互相托底，长时间阅读柔和 | 冷灰让白色文字更「亮」，贴近参考图的水泽冷光 |

两套都满足「中性底 + 暖白正文 + 单一低饱和高亮（面积 ≤5%）」，差别只在色相 5° 与高亮冷暖。
**默认仍是方案 A**（2026-10-22 用户裁定：保持方案 A，不替换 token）；方案 B 仅作为备选留档，
切到方案 B 只需替换下表 token（见 1.4），不动任何组件 CSS。

### 1.2 方案 B token（含实测对比度）

底色 `#14171a`。对比度按 WCAG 2.1 相对亮度公式实测：

| token | 值 | 对比度（vs 底色） | 用途 |
|---|---|---|---|
| `--surface-rgb` / `theme.json surface` | `20, 23, 26`（`#14171a`） | — | 底色（不透明，保证易读） |
| `--surface-rgb-hover` | `40, 43, 46`（`#282b2e`，常态 +20/通道） | — | 悬浮底 |
| `--text` | `#eff1f2` | **15.88:1** | 正文 |
| `--text-dim` | `#c0c5c8` | **10.33:1** | 次要 / 元信息 |
| `--text-muted` | `#8a9095` | **5.57:1** | 极弱 / 占位 |
| `--accent-rgb` / `--accent` | `198, 206, 210` / `#c6ced2` | 11.27:1（仅作描边 / 高亮，不作正文） | 香槟银 → 雾银 |
| `--accent-bg` | `rgba(var(--accent-rgb), 0.08)` | — | 按钮底 |
| `--accent-border` | `rgba(var(--accent-rgb), 0.24)` | — | 按钮描边 |
| `--border-glass` | `rgba(var(--btn-line-rgb), 0.14)` | — | 面板 / 文本框描边 |
| `--radius` | `8px` | — | 控件与卡片统一 8，弹层上限 12 |
| 语义色 | 不变 | — | `--danger-rgb` / `--green` / `--yellow` / `--blue` 仍按 `ui-style.md` §界面 16 |

三档文字全部高于 WCAG AA（4.5:1），`text-muted` 仍留在 5.5:1 以上，与方案 A 的
15.9 / 10.2 / 5.4 基本持平 —— 换色不换取可读性底线。

### 1.3 使用纪律（两套配色通用）

- 页面底色只有 `--surface-rgb` 一处真相源；hover / 描边 / 蒙版全部由它派生，**不新造颜色**。
- accent 面积 ≤5%，只出现在焦点、选中、进行中、可交互描边；**不写 `color: var(--accent)`**（走按钮蒙版 / 按钮线条，见 §界面 2 / 11）。
- 文字三档是 `:root` 固定值，不随 accent 或蒙版漂移。
- 不引入背景图、水渍纹理、厚毛玻璃、高饱和渐变（`theme-impression-design.md` §2.3）。

### 1.4 落地清单（切到方案 B 时）

1. `app/src/styles.css` 的 `:root`：`--surface-rgb` / `--surface-rgb-hover` / `--text` / `--text-dim` / `--text-muted` / `--accent-rgb`。
2. `app/src-tauri/themes/default/theme.json`：`surface` / `accent` / `text` / `text_dim` / `text_muted` 同步（`surface` 必须显式写）。
3. `app/src/main.ts` 的 `DEFAULT_ACCENT` 与派生默认值同步（accent 变了，四类未自定义时的回落值随之变）。
4. `app/src/plugins/builtin/settings.ts` 的兜底色板同步。
5. 验证：`npx tsc --noEmit`、`npm run build`、`npm run verify`；再按 §界面 11 的「四类全设黑」自检验收一次。

---

## 二、图标设计

### 2.1 基础语言（沿用，不重写）

所有插件图标从 `main.ts` 的 `pluginIconSvg()` 一个出口出：

- 画布 `viewBox="0 0 24 24"`，结果区显示 **18px**；`stroke-width="1.8"`（18px 下约 1.35px 视觉重量）。
- `stroke-linecap / linejoin = round`，`fill="none"`，颜色只走 `currentColor`。
- 印象化三件套（自动加，图标本身不用重复画）：
  - **残影**：同形副本 `translate(-0.7,-0.7)`、`opacity 0.28`、`stroke-width 1.4`；
  - **点彩**：左上 `r=1` 实心点、右下 `r=0.5` / `opacity 0.45`（不对称，模拟高光）；
  - **线宽纪律**：不写 1.25 / 1.5（WebView2 下会发虚，见 `ui-style.md` §界面 4）。
- 主题包可通过 `theme.json` 的 `assets.icons.<插件 id>` 整体覆盖（`<img>` 分支），这是唯一覆盖点。

### 2.2 新图标一：默认插件（AI 生成兜底）

**问题**：AI 生成的插件清单里 `icon` 通常为空，未知 id 原先回退 emoji `🔧`。emoji 与其余线性图标不同源，
且在不同字体下形状不可控。

**设计**：插件包 + 星光。

```svg
<rect x="3.6" y="3.6" width="16.8" height="16.8" rx="4.4"/>
<path d="M12 7.4l1.2 3.4 3.4 1.2-3.4 1.2L12 16.6l-1.2-3.4L7.4 12l3.4-1.2L12 7.4Z"/>
```

- 圆角方块 = 插件包（通用、可识别）；
- 四角星 = AI 生成（与 AI 助手的星芒同族，但更小、居中，避免与 `ai-agent` 混淆）；
- 18px 下只有两条路径，边缘不糊；不带文字、不依赖字体。

**落地**：`main.ts` 新增 `DEFAULT_PLUGIN_ICON`，`pluginIconSvg()` 的兜底从 `"🔧"` 改为它。

### 2.3 新图标二：阅读插件（`resource-hub`）

**问题**：`resource-hub` 不在图标表里，结果区显示兜底扳手，与「阅读」语义无关。

**设计**：合着的书 + 书签丝带。

```svg
<path d="M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H20v20H6.5a2.5 2.5 0 0 1 0-5H20"/>
<path d="M10 2v8l3-3 3 3V2"/>
```

- 外轮廓是「合着的书」：左侧书脊圆弧 + 底部书页折边，是 Lucide `book` 的原始路径；
- 书签丝带（`M10 2v8l3-3 3 3V2`）补上「阅读 / 收藏」的第二层语义，与插件里的书架功能对应；
- 不用「打开的书」：18px 下左右两页的斜线会糊成一团。

**落地**：`main.ts` 的 `PLUGIN_ICON_PATHS` 增加 `"resource-hub"`。

### 2.4 设置面板的图标来源统一

设置 → 插件里两条「非 registry」列表此前直接用清单里的 `icon` 字段（AI 生成的插件通常为空 ⇒ 空白方块）：

- 本地插件列表（目录里有、registry 里没有）；
- 工具链分区（带 `dev.build` 的插件，绝大多数是 AI 生成的）。

两处改为先取 `window.__lunac_plugin_icon(id)`（= `pluginIconSvg`，含主题覆盖与默认兜底），再回退清单 `icon`。
市场索引里未安装的条目同样处理，避免出现空白图标。

---

## 三、本轮改动记录（2026-10-21）

- `app/src/main.ts`
  - 新增 `DEFAULT_PLUGIN_ICON`（插件包 + 星光）；`pluginIconSvg()` 未知 id 不再回退 emoji `🔧`。
  - `PLUGIN_ICON_PATHS` 新增 `"resource-hub"`（合着的书 + 书签丝带）。
- `app/src/plugins/builtin/settings.ts`
  - 本地插件列表、市场索引条目、工具链行的图标统一走 `__lunac_plugin_icon`，AI 生成插件不再显示空白。
- `docs/ui-style.md`
  - §界面 4 / 5 登记「未知插件回退内联 SVG」；文末追加本轮改动记录。
- 验证：`npx tsc --noEmit`、`npm run build`、`npm run verify`。
- 图标实测（2026-10-22，dev 实例 CDP 复测 `pluginIconSvg()`）：`resource-hub` 返回书本路径、
  未知 id 返回插件包 + 星光路径，两者均不再回退 emoji。
- **配色裁定（2026-10-22）**：保持方案 A「石墨 · 香槟银」，§1.2 的「雾银 · 冷石墨」不替换 token，
  仅留档备选（替换步骤见 §1.4）。
