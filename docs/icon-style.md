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

- [ ] 不使用 emoji 作为**功能按钮**图标（emoji 仅允许用于装饰性/占位显示或图标缺失兜底）。
- [ ] 使用线性 SVG（`fill:none; stroke:currentColor`），参数符合 §1。
- [ ] hover/选中态通过按钮自身背景色变化表达，不额外换用彩色图标。
- [ ] 文案按钮 = 图标(≤14px) + 文字，间距 4–6px；纯图标按钮需 `title`/`aria-label`。
- [ ] 危险操作（删除等）保留语义色 `var(--red)` 或 hover 变红提示，但仍是同风格线性图标。
- [ ] 复用：已存在于 `pluginIconSvg()`/SVG 常量的图形，不要重复造路径。
