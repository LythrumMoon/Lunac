// ── 自绘下拉框（.custom-select）—— 全项目共享的唯一标准下拉组件 ──────────
//
// 为什么必须自绘：WebView2 透明窗**不渲染原生 `<select>` 的弹层**（见
// docs/forbidden.md §禁用 2 第 13 条 / docs/ui-style.md §界面 6、§界面 12）。
// 原生 `<select>` 的选项行与右侧滚动条由 OS / 浏览器绘制、`<option>` 不可样式化
// ⇒ 四类自定义颜色无从接管。凡「与设置里的下拉一致」的场合，**一律换成本组件**。
//
// 本模块是**唯一**的组件实现（单一真相源，见 docs/code-standards.md §规范 2）：
//   · `customSelectHtml()` —— 出 markup（trigger + label + 内联 SVG 箭头 + dropdown + option）；
//   · `attachCustomSelect()` —— 接线（开合 / 选择 / 外点关闭 / 长列表滚轮 / 边界翻转限高）；
//   · `closeAllCustomSelects()` —— 关掉某作用域内所有打开的本组件。
// 样式在 `app/src/styles.css` 的「.custom-select」一节（插件悬浮窗加载同一份
// `styles.css`，变量经 `broadcastThemeVars` 转发，见 styles.css 与 main.ts）。
//
// 颜色归属（四类，见 docs/ui-style.md §界面 11 / §界面 12）：
//   · 本体（trigger）：底 = 按钮背景（全局 `#app button:where(...)` 已给，这里显式写同值）、
//     描边 = 按钮线条 α0.10（`--ctx-border-glass`）、文字 = `--text`、箭头 = `currentColor`；
//   · 面板（dropdown）：底 = 底色（`--surface-glass`）、描边 = 按钮线条；
//   · 选项：未选中 = 透明底 + `--text-dim`；hover / 选中 = 按钮蒙版（`--other-overlay-*`）。
// 本文件只出 markup 与行为，不写任何色值（颜色全在 styles.css）。

/** 一个选项。`deletable` 为真时渲染右侧 × 删除钮（`.cs-del`）。 */
export interface CustomSelectOption {
  value: string;
  label: string;
  selected?: boolean;
  /** 写进 `data-tag`（如 builtin / custom），供删除逻辑区分；缺省不写该属性。 */
  tag?: string;
  /** 是否渲染 × 删除钮。 */
  deletable?: boolean;
}

/** HTML 转义（本模块自带一份：不能依赖各调用方私有的 `esc`）。 */
function escHtml(s: string): string {
  return String(s).replace(/[&<>"']/g, c =>
    c === "&" ? "&amp;" : c === "<" ? "&lt;" : c === ">" ? "&gt;" : c === '"' ? "&quot;" : "&#39;",
  );
}

/** 生成 `.custom-select` 的完整 markup。
 *  `id` 可空：需要 `#id` 定位（设置面板）时传，纯结构定位（动态列表）可不传。
 *  `extraAttrs` = 追加到根 div 的属性串（如 `data-hk-field="target"`），供调用方带语义。 */
export function customSelectHtml(id: string, options: CustomSelectOption[], extraAttrs = ""): string {
  const optsHtml = options
    .map(o => {
      const tagAttr = o.tag ? ` data-tag="${escHtml(o.tag)}"` : "";
      const del = o.deletable ? `<span class="cs-del" title="×">×</span>` : "";
      return `<div class="custom-select-option${o.selected ? " selected" : ""}" data-value="${escHtml(o.value)}"${tagAttr}>${escHtml(o.label)}${del}</div>`;
    })
    .join("");
  const selectedLabel = options.find(o => o.selected)?.label || options[0]?.label || "";
  const idAttr = id ? ` id="${escHtml(id)}"` : "";
  const extra = extraAttrs ? ` ${extraAttrs}` : "";
  return (
    `<div class="custom-select"${idAttr}${extra}>` +
    `<button class="custom-select-trigger" type="button">` +
    `<span class="custom-select-label">${escHtml(selectedLabel)}</span>` +
    `<svg class="custom-select-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="6 9 12 15 18 9"/></svg>` +
    `</button>` +
    `<div class="custom-select-dropdown">${optsHtml}</div>` +
    `</div>`
  );
}

/** 接线所需的回调。全部可选，只给需要的。 */
export interface CustomSelectHooks {
  /** 选中某个普通选项（非 × 删除、非 __custom__ 入口）。 */
  onPick: (value: string) => void;
  /** 点了 × 删除（仅当选项渲染了 `.cs-del`）。未提供 ⇒ 不处理。 */
  onDelete?: (option: HTMLElement) => void;
  /** 点了 `data-value="__custom__"` 的入口项。未提供 ⇒ 当普通选项处理。 */
  onCustomEntry?: (label: HTMLElement) => void;
  /** 开 / 合 变化（用于放宽 / 还原所属容器的 overflow）。 */
  onOpenChange?: (open: boolean) => void;
  /** 「同一时刻只开一个」的作用域：与其下其它 `.custom-select` 互斥。缺省 = document。 */
  scope?: ParentNode;
}

/** 已接线的标记：同一节点重复 attach 时跳过（重渲染会换新节点，天然重绑）。 */
const BOUND = "__lunacCustomSelectBound";

/** 把下拉框浮层定位在 trigger 正下方（放不下则翻到上方），并按**滚动容器可视区**限高。
 *
 *  为什么是 `position:absolute` 而不是 `fixed`：`#results-container` 有 `backdrop-filter`，
 *  按规范它成为 fixed 后代的包含块 ⇒ 视口坐标会偏一个搜索栏的高度。absolute 免疫。
 *  可用空间按**最近的可滚动祖先（`#results-list`）可视区**算，而不是按窗口 —— 裁剪它的
 *  是那一层；按窗口算会在容器底部明明放不下时仍朝下展开、被裁掉半截。 */
function positionDropdown(dd: HTMLElement): void {
  const dropEl = dd.querySelector<HTMLElement>(".custom-select-dropdown");
  const trigEl = dd.querySelector<HTMLElement>(".custom-select-trigger");
  if (!dropEl || !trigEl) return;
  const trigRect = trigEl.getBoundingClientRect();
  const MARGIN = 8;
  const TARGET_H = 120; // 约 4 行可见；更多项靠列表内部滚动条
  const scroller = dd.closest("#results-list") as HTMLElement | null;
  const box = scroller?.getBoundingClientRect();
  const boundTop = Math.max(box ? box.top : 0, 0);
  const boundBottom = Math.min(box ? box.bottom : window.innerHeight, window.innerHeight);
  const spaceBelow = boundBottom - trigRect.bottom - MARGIN;
  const spaceAbove = trigRect.top - boundTop - MARGIN;
  const openUp = spaceBelow < TARGET_H && spaceAbove > spaceBelow;
  const sideSpace = openUp ? spaceAbove : spaceBelow;
  const maxH = Math.max(60, Math.min(TARGET_H, sideSpace));
  dropEl.style.overflowY = "auto";
  dropEl.style.overscrollBehavior = "contain";
  dropEl.style.position = "absolute";
  dropEl.style.left = "0";
  dropEl.style.width = "100%";
  dropEl.style.maxHeight = maxH + "px";
  if (openUp) {
    dropEl.style.top = "auto";
    dropEl.style.bottom = "calc(100% + 4px)";
  } else {
    dropEl.style.top = "calc(100% + 4px)";
    dropEl.style.bottom = "auto";
  }
  dropEl.style.marginTop = "0";
  dropEl.style.marginBottom = "0";
}

/** 选项文本（去掉尾部 × 删除钮）。 */
function optionText(opt: HTMLElement): string {
  return opt.textContent?.replace(/\s*×$/, "").trim() || "";
}

/** 关掉某作用域内所有打开的本组件（缺省 = document）。返回是否关掉过。 */
export function closeAllCustomSelects(scope: ParentNode | null = document): boolean {
  const root: ParentNode = scope ?? document;
  let closed = false;
  root.querySelectorAll<HTMLElement>(".custom-select.open").forEach(dd => {
    dd.classList.remove("open");
    closed = true;
  });
  return closed;
}

/** 接线一个 `.custom-select`：开合 / 选择 / 外点关闭 / 长列表滚轮 / 边界翻转限高。 */
export function attachCustomSelect(dd: HTMLElement, hooks: CustomSelectHooks): void {
  const rec = dd as unknown as Record<string, unknown>;
  if (rec[BOUND]) return;
  rec[BOUND] = true;

  const trigger = dd.querySelector<HTMLElement>(".custom-select-trigger");
  const dropdown = dd.querySelector<HTMLElement>(".custom-select-dropdown");
  const label = dd.querySelector<HTMLElement>(".custom-select-label");
  if (!trigger || !dropdown || !label) return;

  const scope: ParentNode = hooks.scope ?? document;
  const closeOthers = () =>
    scope.querySelectorAll<HTMLElement>(".custom-select.open").forEach(el => {
      if (el !== dd) el.classList.remove("open");
    });

  // 开合
  trigger.addEventListener("click", (e: Event) => {
    e.preventDefault();
    e.stopPropagation();
    const wasOpen = dd.classList.contains("open");
    closeOthers();
    if (wasOpen) {
      dd.classList.remove("open");
      hooks.onOpenChange?.(false);
    } else {
      dd.classList.add("open");
      hooks.onOpenChange?.(true);
      positionDropdown(dd);
      window.addEventListener("resize", () => positionDropdown(dd), { once: true });
    }
  });

  // 选择 / 删除 / 自定义入口
  dropdown.addEventListener("click", (e: Event) => {
    const target = e.target as HTMLElement;
    const del = target.closest(".cs-del") as HTMLElement | null;
    if (del) {
      e.preventDefault();
      e.stopPropagation();
      const opt = del.closest(".custom-select-option") as HTMLElement | null;
      if (opt) hooks.onDelete?.(opt);
      return;
    }
    const opt = target.closest(".custom-select-option") as HTMLElement | null;
    if (!opt) return;
    e.preventDefault();
    e.stopPropagation();
    const value = opt.getAttribute("data-value") || "";
    if (value === "__custom__" && hooks.onCustomEntry) {
      hooks.onCustomEntry(label);
      return;
    }
    dropdown.querySelectorAll(".custom-select-option").forEach(o => o.classList.remove("selected"));
    opt.classList.add("selected");
    label.textContent = optionText(opt);
    dd.classList.remove("open");
    hooks.onOpenChange?.(false);
    hooks.onPick(value);
  });

  // 长列表：项超出可视区后滚轮由外层滚动容器吞掉 ⇒ 手动滚自己，保证能到列表底。
  dropdown.addEventListener(
    "wheel",
    (e: WheelEvent) => {
      const canUp = dropdown.scrollTop > 0;
      const canDown = dropdown.scrollTop + dropdown.clientHeight < dropdown.scrollHeight - 1;
      if ((e.deltaY < 0 && canUp) || (e.deltaY > 0 && canDown)) {
        e.preventDefault();
        dropdown.scrollTop += e.deltaY;
      }
    },
    { passive: false },
  );

  // 外点关闭：监听挂在 document，节点被重渲染摘掉后自行注销（防监听器堆积）。
  const onDocClick = (e: Event) => {
    if (!dd.isConnected) {
      document.removeEventListener("click", onDocClick);
      return;
    }
    const target = e.target as Node | null;
    if (target && dd.contains(target)) return;
    if (dd.classList.contains("open")) {
      dd.classList.remove("open");
      hooks.onOpenChange?.(false);
    }
  };
  document.addEventListener("click", onDocClick);
}
