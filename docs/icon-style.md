# Lunac 图标风格规范

> 目标：让按钮/操作图标与 Lunac 整体（毛玻璃 + 复古褪色 Catppuccin 主题 + 手绘细线插画风）一致。
> 2026-09 起：备忘录等内置插件的**功能按钮不再使用 emoji**，改为统一的内联线性 SVG 图标（stroke 描边风格）。
> 大图标（插件入口、结果区主图标）见 `app/src/main.ts` 的 `pluginIconSvg()`，已是一套同风格线性 SVG 路径库。

## 1. 基础参数（按钮内小图标）

| 属性 | 值 | 说明 |
|---|---|---|
| 画布 | `viewBox="0 0 24 24"` | 统一 24 栅格 |
| 显示尺寸 | 按钮内 `width="12" height="12"`（小按钮）/ 行内 12–14px | 随字号，不放大失真 |
| 填充 | `fill="none"` | 线性风格 |
| 描边 | `stroke="currentColor"` | 跟随按钮文字颜色（默认 `--text-dim`，hover 变亮） |
| 描边宽 | `stroke-width="2.2"` | 主笔画；残影/装饰元素可 `1.6~1.8` |
| 端点 | `stroke-linecap="round" stroke-linejoin="round"` | 圆角，避免生硬 |

## 2. 结构与残影语言

- 主体使用 1 个主图形（勾、笔、垃圾桶、列表…）。
- 允许“印象派残影/高光”：一个同形副本偏移 `-0.6~-0.7`、`opacity 0.28`、更细笔画，或一个点彩圆点 `r≈0.9`（高光）。
- 参考实现在 `main.ts`（`COPY_SVG`、`RETRY_SVG` 等常量）与 `pluginIconSvg()`。

## 3. 常见操作图标示例

```html
<!-- 保存 ✓ -->
<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
     stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>

<!-- 编辑 ✎ -->
<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
     stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.8 2.8 0 0 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/></svg>

<!-- 复制 -->
<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
     stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/></svg>

<!-- 删除（垃圾桶） -->
<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
     stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6M14 11v6M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>

<!-- 历史/列表 -->
<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
     stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="8" y1="6" x2="21" y2="6"/><line x1="8" y1="12" x2="21" y2="12"/><line x1="8" y1="18" x2="21" y2="18"/><line x1="3" y1="6" x2="3.01" y2="6"/><line x1="3" y1="12" x2="3.01" y2="12"/><line x1="3" y1="18" x2="3.01" y2="18"/></svg>

<!-- 取消 ✕ -->
<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
     stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>

<!-- 标识/标签 -->
<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor"
     stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M20.6 13.4 13.4 20.6a2 2 0 0 1-2.8 0L2 12V2h10l8.6 8.6a2 2 0 0 1 0 2.8Z"/><circle cx="7" cy="7" r="1"/></svg>
```

## 4. 检查清单（新增/修改图标时）

### 4.1 硬规则：UI 文案里**永远不得**内嵌 emoji 当图标（2026-09-19，用户明确要求）

- **禁止**：按钮标签、状态行 / 提示行、占位文案里拼 emoji。`📋 复制结果`、`📁 选择文件`、`🖼️ 暂无图片`、`⚠️ 剪贴板中暂无图片，请复制图片后重试`、`⏳ 识别中`、`✅ 完成`、`❌ 失败` —— 这些**全部不合格**。
  - 要图标就用 §1 的线性 SVG（`fill:none; stroke:currentColor`，随按钮文字色）；
  - 不要图标就**纯文字**。
- **判据（一句话）**：这个 emoji 是**「一条可点控件 / 一句提示的文案的一部分」**（→ 禁止），还是**「一个条目 / 一条状态的标记」**（→ 允许）？
- **允许且必须保留**（不属于本条约束）：
  - **列表项 / 条目图标**：结果区行图标、目录数据自带的 `item.icon`、`plugin.icon`（未知插件回退 🔧）、`.clip-item-icon`（📁/📋）、`.history-item-icon`（💬）、`.file-chip-icon`（📦/📎）、todo 面板标题的 📋、tool-editor 的 🔧、web-search 的引擎图标（🔍/🌐/🐻）—— 见本文件头部与 ai-spec §2.1.2。
  - **单色文字状态符号**：`✓ ✗ ⚠ ↔ ↩ ◐ ○ ✔ ✕ × ＋` 这类**单色字符**，用于表达状态（如 `clipboard.copied` = `"✓ 已复制"`、`clipboard.copy_failed` = `"✗ 失败"`、todo 的 `✔/◐/○`、工具黑名单的 `×/✓`）。它们是状态标记，不是装饰图标。
  - **细分判据**：带 `U+FE0F`（变体选择符，强制 emoji 彩色呈现）的**按 emoji 算** —— `⚠️` 禁止、`⚠` 允许。
- **本轮（2026-09-19 批 8）已按此清理**：`ocr.ts` 全量（`识别剪贴板` / `选择文件` / `复制结果` 三个按钮、图片占位、引擎状态行、以及 `⏳ 读取中 / ⏳ 识别中 / ✅ 完成 / ❌ 失败 / ⚠️ 剪贴板中暂无图片…` 等全部状态行文案）、`clipboard-history.ts` 的复制按钮（原 `📎 复制` / `📋 复制`）、`i18n.ts` 的 `clipboard.copy`（原 `📋 复制`，5 语言）与 `agent.static_danger`（原 `⛔ 危险命令…`，5 语言）、`main.ts` 详细搜索的提权按钮（原 emoji 🛡 → §1 线性 SVG 盾牌，顺带让它真正吃到 `currentColor` → `--accent`）。

### 4.2 逐项清单

- [ ] **控件文案里没有 emoji**（见 §4.1）——按钮文字 / 状态行 / 占位提示一律「纯文字」或「§1 线性 SVG + 文字」。
- [ ] 列表项图标与单色状态符号的使用方式未被误改（`.`clip-item-icon` / `.history-item-icon` / `item.icon` / `plugin.icon` 与 `✓ ✗ ⚠` 系列保留）。
- [ ] 使用线性 SVG（`fill:none; stroke:currentColor`），参数符合 §1；不使用 emoji 作为**功能按钮**图标。
- [ ] hover/选中态通过按钮自身背景色变化表达，不额外换用彩色图标。
- [ ] 文案按钮 = 图标(≤14px) + 文字，间距 4–6px；纯图标按钮需 `title`/`aria-label`。
- [ ] 危险操作（删除等）保留语义色 `var(--red)` 或 hover 变红提示，但仍是同风格线性图标。
- [ ] 复用：已存在于 `pluginIconSvg()`/SVG 常量的图形，不要重复造路径。
