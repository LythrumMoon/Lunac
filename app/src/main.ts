// ── Lunac main.ts ─────────────────────────────────────────────────
// Hotkey is handled natively in Rust (hotkey.rs) — no JS needed.

import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { listen } from "@tauri-apps/api/event";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { type Plugin, pluginRegistry } from "./plugins/registry";
import { registerBuiltinPlugins } from "./plugins/builtin/index";
import { getSearchEngine, getSearchEngineName, setSearchEngine } from "./plugins/builtin/web-search";
import { initI18n, loadSavedLanguage, t, pluginName } from "./i18n.js";

// ── App entry from Rust backend ──────────────────────────────────
interface AppEntry {
  name: string;
  path: string;
  icon?: string;
  source: string;
}

// KaTeX — loaded from CDN in index.html
declare const katex: {
  render(expr: string, el: HTMLElement, opts?: { throwOnError?: boolean; displayMode?: boolean; trust?: boolean }): void;
};

// ── Plugin-independent state ────────────────────────────────────
interface PluginState {
  id: string;
  html: string;            // saved resultsList.innerHTML
  searchQuery: string;     // search input at time of entry
  pendingInput: string;    // unsent detached input
}
const pluginStates = new Map<string, PluginState>();

// ── DOM ──────────────────────────────────────────────────────────
const el = (id: string) => document.getElementById(id)!;
const searchInput = el("search-input") as HTMLTextAreaElement;
const searchBar = el("search-bar");
const resultsContainer = el("results-container");
const resultsList = el("results-list");
const statusBar = el("status-bar"); // 状态栏（面板最底，实测高度基准）
const statusText = el("status-text");
const statusHint = el("status-hint");
const tokenDashboard = el("token-dashboard");
const settingsBtn = el("settings-btn");
const pluginBarTitle = el("plugin-bar-title");
const pluginBarExit = el("plugin-bar-exit");
const detachedHeader = el("detached-header");
const detachedTitle = el("detached-title");
const detachedBackBtn = el("detached-back-btn");
const detachedCloseBtn = el("detached-close-btn");
const detachedVscodeBtn = el("detached-vscode-btn");
const chatInputBar = el("chat-input-bar");
const chatInput = el("chat-input") as HTMLTextAreaElement;
const chatSendBtn = el("chat-send-btn");
const chatStopBtn = el("chat-stop-btn");
const chatHistoryBtn = el("chat-history-btn");
const humanizeBtn = el("humanize-btn");
const chatFileChipsContainer = el("chat-file-chips");
const chatAddFileBtn = el("chat-add-file-btn");
const fileChips = el("file-chips");
const chatDrawer = el("chat-drawer");
const chatDrawerList = el("chat-drawer-list");
const chatDrawerClose = el("chat-drawer-close");
/** Parent of chatDrawer — used to re-insert drawer after DOM removal */
const drawerParent = chatDrawer.parentElement!;
/** Sibling before which to re-insert drawer (results-list) */
const drawerSibling = resultsList;
const chatNewBtn = el("chat-new-btn");
const chatModeBtn = el("chat-mode-btn");
const chatWorkspaceBtn = el("chat-workspace-btn");
const chatWorkspaceMenu = el("chat-workspace-menu");
const chatWorkspacePath = el("chat-workspace-path");
const chatWorkspaceSelect = el("chat-workspace-select");
const chatWorkspaceReset = el("chat-workspace-reset");
const chatToolsBtn = el("chat-tools-btn");
const chatToolsMenu = el("chat-tools-menu");
const chatToolsTitle = el("chat-tools-title");
const chatToolsList = el("chat-tools-list");
const chatToolsSave = el("chat-tools-save");
const chatToolsMsg = el("chat-tools-msg");

// ── 窗口尺寸 —— 实测驱动（方案A，修复「窗口尺寸与结果区渲染区域不一致」）──
// 设计宽度 800px；窗口可拖拽缩放（tauri.conf resizable:true），宽度变化经
// WebView2(Chromium) 的 CSS `zoom` 属性等比缩放内容。
// 高度不再用 heightMap 预测，而是渲染后实测：
//   getBoundingClientRect() 返回含 CSS zoom 的视觉尺寸（viewport 坐标系），
//   在 WebView2 下数值 = 逻辑像素(DIP)，可直接喂 LogicalSize，无需再乘 zoom。
//   这是缩放场景唯一可靠测量 —— clientHeight/scrollHeight/offsetHeight 均不含
//   zoom（MDN: 需 currentCSSZoom 校正）。此前两种失败方案（自适应不显示结果区、
//   多高度适配失败）正是栽在「预测高度没计入搜索栏+状态栏」和「量错 zoom 尺度」。
const WIN_WIDTH = 800;
const ZOOM_BASE_WIDTH = WIN_WIDTH;
const ZOOM_MIN = 0.6;
const ZOOM_MAX = 2.5;
// 在应用任何 zoom 之前快照物理↔逻辑像素比 —— Chromium 中 CSS zoom 可能
// 改变 window.devicePixelRatio，启动时快照才是换算 resize 载荷的稳定基准。
const BASE_DPR = window.devicePixelRatio || 1;
let lastWindowWidth = WIN_WIDTH;   // 当前窗口逻辑宽度（DIP）
let currentZoom = 1;               // 当前 zoom 因子
let currentWindowHeight = 200;     // 当前窗口逻辑高度（DIP，onResized 实测）
let requestedHeight = -1;          // 最近一次已下发的高度（-1=未下发/需重新断言）
// setSize 串行化（方案4）：快速键入时避免 setSize IPC 风暴与 onResized 回环。
// pendingHeight = 在途期间到达的最新期望高度（latest-wins）；sizeInFlight 防重入。
let pendingHeight: number | null = null;
let sizeInFlight = false;

// ── 窗口高度「滑动」动画（正式）── 结果区随界面高度变化平滑过渡 ─────
// 非插件态下高度变化不再一次 setSize 到位，而是逐帧逼近（rail 模式：每步等
// 上一个 setSize 经 onResized 落地后才走下一步，天然兼容现有防回环守卫）。
// 调节入口（DevTools Console 实时改，无需刷新）：
//   __lunac_resize_anim.enabled = false   // 关动画 → 恢复原直接 setSize
//   __lunac_resize_anim.rigidity = 0.4    // 0.05~0.6：大=刚性/跟手，小=柔滑拖尾
//   __lunac_resize_anim.maxStep  = 24     // 单步最大像素（刚性上限；0=不限）
//   __lunac_resize_anim.stepHz   = 60     // 每秒步数上限（调低测性能开销）
//   __lunac_resize_anim.suppressMs = 800  // 唤出/复位后抑制滑动的时长（调 0 可关抑制）
// 默认参数已按实测手感定稿：rigidity 0.22 / maxStep 14 / stepHz 120。
const RESIZE_ANIM_CFG = {
  enabled: true,   // 默认开（false 即回退旧直设行为）
  rigidity: 0.22,  // 每帧逼近比例
  maxStep: 14,     // 单步最大位移 px
  stepHz: 120,     // 步频上限
  // 唤出抑制时长：窗口被热键/托盘唤出（或首帧就绪）起算，期间的高度变化一律
  // 直设不滑动 —— 否则会看到结果区被裁剪、逐帧"撑开"。每次在抑制期内发生
  // 高度变化都会顺延该期限（内容可能分几批到达），静默超过此时长后恢复滑动。
  suppressMs: 400,
};
const resizeAnimStats = { transitions: 0, steps: 0, lastSteps: 0, lastMs: 0 };
(window as any).__lunac_resize_anim = RESIZE_ANIM_CFG;
(window as any).__lunac_resize_anim_stats = resizeAnimStats;
let bootHeightSettled = false; // 首次高度先直设落位，之后高度变化才启用滑动（防启动滑屏）
let animActive = false;        // 动画循环运行中
let animTarget = 0;            // 动画目标高度（latest-wins）
let animAwaitingLand = false;  // 已有 setSize 在途，等 onResized 落地
let animLastDispatchAt = 0;
let animStartAt = 0;
let animRaf = 0;
// 抑制滑动的截止时刻（performance.now 基准）。> now 时高度变化直接落位。
// 初值即给一个抑制期，覆盖启动首屏渲染（字体/i18n 就绪后高度可能再变一次），
// 与 bootHeightSettled（只管首次落位）互补。
let animSuppressUntil = performance.now() + RESIZE_ANIM_CFG.suppressMs;

/** 进入/顺延「抑制滑动」期：唤出窗口时调用，保证面板瞬时完整展开。 */
function suppressResizeAnimBriefly() {
  animSuppressUntil = performance.now() + RESIZE_ANIM_CFG.suppressMs;
}

/** 按窗口宽度重新计算 zoom 并应用到 <html>。 */
function applyZoom() {
  currentZoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, lastWindowWidth / ZOOM_BASE_WIDTH));
  document.documentElement.style.zoom = String(currentZoom);
}

/** 实测面板所需高度（DIP）：纵向 搜索栏 / 结果区 / 状态栏 取最底者。
 *  getBoundingClientRect().bottom 为视口坐标，已含 CSS zoom 缩放；
 *  hidden（display:none）元素的 rect 为 0，跳过即可。 */
function measurePanelHeight(): number {
  let h = 0;
  if (!searchBar.classList.contains("hidden"))
    h = Math.max(h, searchBar.getBoundingClientRect().bottom);
  if (!resultsContainer.classList.contains("hidden"))
    h = Math.max(h, resultsContainer.getBoundingClientRect().bottom);
  h = Math.max(h, statusBar.getBoundingClientRect().bottom);
  return Math.ceil(h);
}

/** 高度变更防抖：连续 onResized / ResizeObserver 事件收敛到一次 applyWindowSize。 */
let sizeTimer: number | undefined;
function scheduleApplySize() {
  if (sizeTimer !== undefined) return;
  sizeTimer = window.setTimeout(() => {
    sizeTimer = undefined;
    applyWindowSize();
  }, 60);
}

/** 搜索路径懒测量（方案4）：键入重建 DOM 后不在同任务立即同步测量 + setSize，
 *  双 rAF 等布局稳定后再实测一次；连续键入时 cancel 上一帧、只保留最新一次。 */
let searchResizeRaf = 0;
function scheduleSearchResize() {
  if (searchResizeRaf) cancelAnimationFrame(searchResizeRaf);
  searchResizeRaf = requestAnimationFrame(() => {
    searchResizeRaf = requestAnimationFrame(() => {
      searchResizeRaf = 0;
      if (pluginActive) return; // 插件面板高度由各自流程负责
      applyWindowSize();
    });
  });
}

/** setSize 串行下发（latest-wins）：在途期间新期望高度只记 pendingHeight，
 *  本次完成后若与已发高度不同再补发一次 —— 杜绝快速键入时 setSize IPC 排队堆积。 */
function requestWindowHeight(h: number) {
  // 动画在途且仍处搜索态：只更新动画目标（latest-wins），由动画逐帧收敛。
  // 插件态请求不允许被动画劫持 —— 插件是离散跳变，应走下方直设路径。
  if (animActive && !pluginActive) {
    if (performance.now() < animSuppressUntil) {
      stopResizeAnim(); // 唤出抑制期内：停掉旧动画，改走直设瞬时落位
    } else {
      animTarget = h;
      return;
    }
  }
  if (h === requestedHeight) return;
  // 窗口实际高度已是内容高 → 仅对齐标记（防 setSize→onResized 回环）
  if (Math.abs(h - currentWindowHeight) < 3) { requestedHeight = h; return; }
  // 实验：搜索/空态启用「结果区随窗口大小滑动」→ 逐帧步进 setSize。
  // bootHeightSettled=false 期间（首次高度落位）直设，防启动时窗口滑屏。
  if (bootHeightSettled && RESIZE_ANIM_CFG.enabled && !pluginActive) {
    if (performance.now() < animSuppressUntil) {
      // 唤出/复位抑制期内：直设落位，并把抑制期顺延（内容可能分几批到达：
      // 剪贴板探测 → 加泡泡 → 重跑搜索 → 实测高度，每批都会触发一次）。
      animSuppressUntil = performance.now() + RESIZE_ANIM_CFG.suppressMs;
    } else {
      animateWindowHeight(h);
      return;
    }
  }
  pendingHeight = h;
  if (!sizeInFlight) void flushWindowHeight();
}

async function flushWindowHeight() {
  if (sizeInFlight || pendingHeight === null) return;
  const target = pendingHeight;
  pendingHeight = null;
  if (target === requestedHeight) return;
  requestedHeight = target;
  bootHeightSettled = true; // 第一次物理 setSize 下发后，后续高度变化才走滑动
  sizeInFlight = true;
  try {
    await getCurrentWindow().setSize(new LogicalSize(lastWindowWidth, target));
  } catch { /* 忽略 */ }
  sizeInFlight = false;
  // 在途期间又到了更新的期望高度 → 串行补发一次
  if (pendingHeight !== null && pendingHeight !== requestedHeight) {
    void flushWindowHeight();
  }
}

// ── 滑动动画实现（实验）──────────────────────────────────────────
function stopResizeAnim() {
  if (animRaf) cancelAnimationFrame(animRaf);
  animRaf = 0;
  animActive = false;
  animAwaitingLand = false;
}

/** 启动/更新窗口高度滑动动画。距目标极近则直接对齐不起帧；
 *  已有动画在跑 → 仅更新目标（latest-wins），保证键入连发不堆积。 */
function animateWindowHeight(h: number) {
  animTarget = Math.round(h);
  if (Math.abs(animTarget - currentWindowHeight) <= 1) {
    stopResizeAnim();
    requestedHeight = animTarget;
    return;
  }
  if (animActive) return; // 已在动画中，仅更新目标
  animActive = true;
  animAwaitingLand = false;
  animLastDispatchAt = 0;
  animStartAt = performance.now();
  resizeAnimStats.lastSteps = 0;
  const minStepGap = 1000 / Math.max(1, RESIZE_ANIM_CFG.stepHz);
  const tick = () => {
    if (!animActive) return;
    animRaf = requestAnimationFrame(tick);
    // 运行时关动画 → 停帧并直设一次收敛到最新目标
    if (!RESIZE_ANIM_CFG.enabled) {
      stopResizeAnim();
      pendingHeight = animTarget;
      if (!sizeInFlight) void flushWindowHeight();
      return;
    }
    // 进入插件态 → 停帧；插件高度由插件流程（applyWindowSize）直设
    if (pluginActive) { stopResizeAnim(); return; }
    const now = performance.now();
    if (animAwaitingLand) {
      // 上一 setSize 未落地（onResized 未回）→ 等下一帧；超时兜底防卡死
      if (now - animLastDispatchAt < 200) return;
      animAwaitingLand = false;
    } else if (now - animLastDispatchAt < minStepGap) {
      return; // 步频限流（性能测试参数）
    }
    const d = animTarget - currentWindowHeight;
    if (Math.abs(d) <= 1) {
      // 收尾：精确下发一次目标值后停帧
      if (requestedHeight !== animTarget) {
        requestedHeight = animTarget;
        resizeAnimStats.steps++;
        resizeAnimStats.lastSteps++;
        void win
          .setSize(new LogicalSize(lastWindowWidth, animTarget))
          .catch(() => {});
      }
      resizeAnimStats.transitions++;
      resizeAnimStats.lastMs = now - animStartAt;
      stopResizeAnim();
      return;
    }
    // 指数趋近 + 单步上限 → 决定下一高度
    let step = d * RESIZE_ANIM_CFG.rigidity;
    const cap = RESIZE_ANIM_CFG.maxStep > 0 ? RESIZE_ANIM_CFG.maxStep : Math.abs(d);
    step = Math.max(1, Math.min(cap, Math.abs(step))) * Math.sign(d);
    const next = Math.round(currentWindowHeight + step);
    if (next === currentWindowHeight) return; // 未位移 → 等 onResized 刷新实测高
    requestedHeight = next;
    animAwaitingLand = true;
    animLastDispatchAt = now;
    resizeAnimStats.steps++;
    resizeAnimStats.lastSteps++;
    void win
      .setSize(new LogicalSize(lastWindowWidth, next))
      .catch(() => { animAwaitingLand = false; /* 失败放行，下一帧重试 */ });
  };
  animRaf = requestAnimationFrame(tick);
}

/** Apply window size based on current UI state.
 *  插件模式：固定高度（detached 600 / embedded 360 / OCR detached 520），
 *    设计 px × zoom = DIPs（插件面板按设计宽度 800 的 CSS px 排版）。
 *  搜索/结果/空态：实测内容高度（measurePanelHeight，已含 zoom），
 *    彻底取代 heightMap 预测，保证窗口与结果区渲染区域严格一致。
 *  宽度保持用户当前宽度（lastWindowWidth）。
 *  setSize 统一走 requestWindowHeight 串行化；搜索路径可改用 scheduleSearchResize
 *  懒化测量，避免快速键入时“同任务强制 layout + IPC 风暴”。 */
function applyWindowSize() {
  // 插件/分离模式 #app 撑满窗口（CSS height:100%）；搜索模式内容驱动（height:auto）
  document.getElementById("app")!.classList.toggle("plugin-active", pluginActive);

  let h: number;
  if (pluginActive) {
    // OCR needs a wider two-panel layout in detached mode
    if (detached && activePluginId === "ocr") {
      h = Math.round(520 * currentZoom);
    } else {
      h = Math.round((detached ? 600 : 360) * currentZoom);
    }
  } else {
    h = measurePanelHeight();
  }
  requestWindowHeight(h);
}

// ── File chips management ──────────────────────────────────────

/** Re-render search results after chips change.
 *  Skipped while a plugin owns the results area (e.g. AI chat): dispatching a
 *  search "input" there rebuilds resultsList and wipes the chat log / plugin
 *  panel. Chips still update — renderFileChips draws into both containers. */
function refreshSearchResults() {
  if (pluginActive) return;
  searchInput.dispatchEvent(new Event("input"));
}

/** Add a file path as a chip in the search bar */
function addFileChip(path: string) {
  if (!path.trim()) return;
  const resolved = path.trim();
  if (attachedFiles.includes(resolved)) return; // no duplicates
  attachedFiles.push(resolved);
  renderFileChips();
  syncChipsEmpty();
  // Expose to quick-launch plugin
  (window as any).__lunac_attached_files = attachedFiles;
  // Persist as a custom launch entry (点12): pasted/dragged paths should
  // survive a restart. Quick-launch execute() also registers, but that only
  // runs when the user explicitly opens the launch panel — without this the
  // entry was lost on restart. Duplicate/missing-path errors are ignored.
  invoke("add_custom_app", { name: "", path: resolved }).catch(() => {});
  // Re-trigger search to show "Custom Launch" entry
  refreshSearchResults();
}

/** Image extension check — images get thumbnail previews, other files stay
 *  as path-only chips (需求1: 仅图片预览，非图片仅传路径). */
function isImageFile(p: string): boolean {
  return /\.(png|jpg|jpeg|gif|webp|bmp|svg|ico|avif|jfif)$/i.test(p);
}

/** Remove a file chip by index */
function removeFileChip(index: number) {
  attachedFiles.splice(index, 1);
  renderFileChips();
  syncChipsEmpty();
  (window as any).__lunac_attached_files = attachedFiles;
  refreshSearchResults();
}

/** 一键删除“省略泡泡”分支内的全部文件（前 FILE_CHIP_LIMIT 个保留）。
 *  与省略号删除按钮、Backspace 删除逻辑同步。 */
function clearHiddenFileChips() {
  if (attachedFiles.length <= FILE_CHIP_LIMIT) return;
  attachedFiles = attachedFiles.slice(0, FILE_CHIP_LIMIT);
  chipMoreOpen = false;
  renderFileChips();
  syncChipsEmpty();
  (window as any).__lunac_attached_files = attachedFiles;
  refreshSearchResults();
}

/** 前 N 个文件正常显示为独立泡泡；更多文件折叠进一个“省略泡泡”分支 */
const FILE_CHIP_LIMIT = 3;
let chipMoreOpen = false; // 省略泡泡是否展开（会话内状态）

/** 单个文件泡泡 HTML */
function fileChipHtml(p: string, i: number): string {
  const name = p.split(/[\\/]/).pop() || p;
  const isImage = isImageFile(p);
  const media = isImage
    ? `<img class="file-chip-preview" src="${esc(convertFileSrc(p))}" alt="" />`
    : `<span class="file-chip-icon">${/\.(exe|lnk)$/i.test(name) ? "📦" : "📎"}</span>`;
  return `<div class="file-chip" data-index="${i}" title="${esc(p)}">
    ${media}
    <span class="file-chip-name">${esc(name)}</span>
    <button class="file-chip-remove" data-index="${i}" title="${t("tooltip.remove_file")}">×</button>
  </div>`;
}

/** Render file chips as bubble-style elements.
 *  AI chat active → render only into the chat input bar; otherwise render
 *  only into the search bar. 超过 FILE_CHIP_LIMIT 个时，多余的折叠进一个
 *  “省略泡泡”（分支），点击可展开查看；展开区/省略泡泡提供“一键删除全部隐藏”。 */
function renderFileChips() {
  const inChat = pluginActive && activePluginId === "ai-agent";
  // 非 AI 聊天的其它插件界面（设置/备忘录/OCR…）：搜索栏泡泡整体隐藏，
  // 退出插件（setPluginBar(null)）后本函数再把它们恢复渲染回搜索栏。
  const inPluginBar = pluginActive && !inChat;
  if (attachedFiles.length === 0) {
    fileChips.classList.add("hidden");
    fileChips.innerHTML = "";
    chatFileChipsContainer.innerHTML = "";
    return;
  }
  const visible = attachedFiles.slice(0, FILE_CHIP_LIMIT);
  const hidden = attachedFiles.slice(FILE_CHIP_LIMIT);
  // 文件数回落 ≤ 阈值时省略分支不复存在，复位展开态，避免下次新增文件自动弹开
  if (hidden.length === 0) chipMoreOpen = false;
  let chipsHtml = visible.map((p, i) => fileChipHtml(p, i)).join("");

  if (hidden.length > 0) {
    const n = hidden.length;
    chipsHtml += `
      <div class="file-chip-more">
        <button class="file-chip-more-btn" data-more="${chipMoreOpen ? "1" : "0"}" title="${t("tooltip.more_files")}">⋯ +${n} ${chipMoreOpen ? "▾" : "▸"}</button>
        <button class="file-chip-more-clear" data-clear-more title="${t("tooltip.clear_more_files", { n: String(n) })}">✕ ${n}</button>
      </div>`;
    if (chipMoreOpen) {
      chipsHtml += `<div class="file-chip-more-list">${hidden.map((p, i) => fileChipHtml(p, FILE_CHIP_LIMIT + i)).join("")}</div>`;
    }
  }

  if (inChat) {
    fileChips.classList.add("hidden");
    fileChips.innerHTML = "";
    chatFileChipsContainer.innerHTML = chipsHtml;
    bindChipRemoveListeners(chatFileChipsContainer);
    bindChipMoreListeners(chatFileChipsContainer);
  } else if (inPluginBar) {
    // 其它插件界面：不在搜索栏渲染泡泡（避免漂浮在原位遮挡插件内容）
    fileChips.classList.add("hidden");
    fileChips.innerHTML = "";
    chatFileChipsContainer.innerHTML = "";
  } else {
    fileChips.classList.remove("hidden");
    fileChips.innerHTML = chipsHtml;
    chatFileChipsContainer.innerHTML = "";
    bindChipRemoveListeners(fileChips);
    bindChipMoreListeners(fileChips);
  }
}

/** Attach remove listeners to a single file-chip container */
function bindChipRemoveListeners(container: HTMLElement) {
  container.querySelectorAll(".file-chip-remove").forEach(btn => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const idx = parseInt((btn as HTMLElement).dataset.index || "0");
      removeFileChip(idx);
    });
  });
}

/** 省略泡泡：展开/收起 + 一键删除省略分支内的全部文件 */
function bindChipMoreListeners(container: HTMLElement) {
  container.querySelectorAll(".file-chip-more-btn").forEach(btn => {
    btn.addEventListener("click", () => {
      chipMoreOpen = !chipMoreOpen;
      renderFileChips();
    });
  });
  container.querySelectorAll(".file-chip-more-clear").forEach(btn => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      clearHiddenFileChips();
    });
  });
}

/** Clear all file chips */
function clearFileChips() {
  attachedFiles = [];
  chipMoreOpen = false;
  renderFileChips();
  syncChipsEmpty();
  (window as any).__lunac_attached_files = [];
}

/** Sync chip empty state to Rust for Esc handling */
function syncChipsEmpty() {
  invoke("set_chips_empty", { empty: attachedFiles.length === 0 }).catch(() => {});
}

/** Auto-resize textarea to fit content (grows search bar dynamically) */
function autoResizeTextarea() {
  searchInput.style.height = "auto";
  searchInput.style.height = searchInput.scrollHeight + "px";
}

/** Auto-resize chat textarea (plugin chat input only — NOT the search box).
 *  Height is clamped to [30px, 60px]; beyond that it scrolls internally. */
function autoResizeChatTextarea() {
  chatInput.style.height = "auto";
  const h = Math.min(60, Math.max(30, chatInput.scrollHeight));
  chatInput.style.height = h + "px";
}

// ── Window ───────────────────────────────────────────────────────
const win = getCurrentWindow();

// ── 监听窗口尺寸变化（用户拖拽边缘）→ 重算 CSS zoom 等比缩放 ──────
// 载荷即新尺寸(PhysicalSize)，用启动时快照的 BASE_DPR 换算回逻辑尺寸。
// setSize() 触发的事件同样经过这里，宽度未变则 zoom 不变；高度与内容一致
// 时 applyWindowSize 会跳过（乐观高度），无震荡。
win.onResized(({ payload }) => {
  const p = payload as { width: number; height: number };
  if (p.width > 0) {
      const w = Math.round(p.width / BASE_DPR);
      const widthChanged = w !== lastWindowWidth;
      lastWindowWidth = w;
      currentWindowHeight = Math.round(p.height / BASE_DPR);
      bootHeightSettled = true; // 首个真实 resize 事件 = 高度基准已建立，后续变化可用滑动
      if (animActive) animAwaitingLand = false; // 本步 setSize 已落地 → 放行下一帧
      applyZoom();
    // 宽度变化（zoom 变 → 内容缩放后高度需跟随）或高度被外部改动
    // （用户拖拽/OS 修正）→ 重置 requestedHeight，防抖后重新断言内容高。
    if (widthChanged || Math.abs(currentWindowHeight - requestedHeight) > 3) {
      requestedHeight = -1;
      scheduleApplySize();
    }
  }
}).catch(() => {});
// 首次启动即按初始宽度(800)校准 zoom（zoom=1，视觉无变化）
applyZoom();

// ── 内容驱动高度兜底（方案A）──────────────────────────────────────
// 搜索模式 #app 为 height:auto，其渲染尺寸变化即内容高度变化（结果增删、
// 搜索栏换行、字体/图片加载完成），经防抖重算窗口高度。插件模式 #app 为
// height:100%（随窗口），配合 applyWindowSize 的乐观高度跳过逻辑，无循环。
new ResizeObserver(() => scheduleApplySize()).observe(document.getElementById("app")!);

// ── 热键配置查询：JS 兜底 handler 需要知道当前是否 Alt+Space ────
// 自定义热键后 Alt+Space 必须彻底失效，此标志控制 JS 层是否拦截。
// 同时把状态栏提示刷成“当前实际热键”（默认 Ctrl+Alt+Space，可在设置里改键）。
let currentHotkeyCombo = "Ctrl+Alt+Space";
/** 刷新状态栏热键提示（启动 / 热键变更后调用）。 */
function refreshHotkeyHint(combo?: string) {
  if (combo) currentHotkeyCombo = combo;
  statusHint.textContent = t("status.hotkey_hint", { hotkey: currentHotkeyCombo });
}
// 供 settings 插件在改键成功后同步刷新（避免状态栏残留旧热键）
(window as any).__lunac_refresh_hotkey_hint = refreshHotkeyHint;
(window as any).__lunac_hotkey_is_alt_space = true; // 默认值
invoke<string>("get_hotkey_combo").then((combo) => {
  (window as any).__lunac_hotkey_is_alt_space = combo === "Alt+Space";
  refreshHotkeyHint(combo);
}).catch(() => {});

// ── State ────────────────────────────────────────────────────────
let isVisible = false;
let currentResults: Plugin[] = [];
let currentApps: AppEntry[] = [];   // app results for Enter key handling
let attachedFiles: string[] = [];   // files dragged/pasted into search bar
let selectedIndex = 0;

// ── Unified render order ─────────────────────────────────────────
// renderMixedResults interleaves apps + plugins + fallbacks by recency
// into a single visual list. currentEntries mirrors this exact order
// so that Enter key (which uses selectedIndex) matches what's shown.
type ScoredEntry =
  | { kind: "app"; app: AppEntry }
  | { kind: "plugin"; plugin: Plugin }
  | { kind: "ws-fallback" }
  | { kind: "memo-fallback" }
  | { kind: "memo-tag-open"; id: string; tag: string }
  | { kind: "ai-fallback" };
let currentEntries: ScoredEntry[] = [];

let pluginActive = false;           // true when a plugin result panel is showing
let activePluginId: string | null = null;  // which plugin is active (for toggle)
let detached = false;               // detach mode
let streamId = 0;                   // prevent stale callback from closed stream
let isChatHistoryView = false;      // true when showing history, not active chat
let cliReady = false;               // CLI process is running and stdin is ready

// ── Recency tracking (localStorage) ────────────────────────────
// Keys: "app://<path>" for apps, "plugin://<id>" for plugins
interface RecencyStore { [key: string]: number }

function loadRecency(): RecencyStore {
  try {
    const raw = localStorage.getItem("lunac-recency");
    return raw ? JSON.parse(raw) : {};
  } catch { return {}; }
}
function saveRecency(store: RecencyStore) {
  try { localStorage.setItem("lunac-recency", JSON.stringify(store)); } catch {}
}
function recordRecency(key: string) {
  const store = loadRecency();
  store[key] = Date.now();
  saveRecency(store);
}
/** Get recency boost score: 0 = never used / oldest, 10 = most recent */
function getRecencyScoreFor(key: string, store: RecencyStore): number {
  if (!store[key]) return 0;
  const values = Object.values(store);
  if (values.length <= 1) return 0; // single entry: no ordering needed
  const max = Math.max(...values);
  const min = Math.min(...values);
  const range = max - min || 1;
  return ((store[key] - min) / range) * 10;
}

// ── AI Chat state ──────────────────────────────────────────────
let chatHistory: Array<{ role: string; content: string }> = [];
let isStreaming = false;
let pendingMessages: string[] = [];  // queued while streaming
let userScrolledUp = false;         // true when user has scrolled away from bottom
interface ChatDoneInfo {
  stop_reason: string;
  input_tokens: number;
  output_tokens: number;
  cache_read_input_tokens?: number;
  cache_creation_input_tokens?: number;
}

// ── Cumulative token tracking (reset per conversation) ───────────
let sessionTokens = { hit: 0, miss: 0, total: 0 };
let lastAgentTokens = { input: 0, output: 0, cacheRead: 0, cacheCreate: 0 }; // CLI reports cumulative totals

// ── Chat conversation mode ────────────────────────────────────────
// DeepSeek 思考模式三档（点2/7），替换旧的 simple/agent 切换：
//   "fast"  = 不思考（MAX_THINKING_TOKENS=0，直接回答）
//   "think" = 思考（8k 思考预算）
//   "deep"  = 深度思考（32k 思考预算）
// 三档共用同一 agent.exe 完整工具链，仅思考深度不同；由 Rust 端
// set_thinking_mode → start_cli_process 写入环境变量。
type ThinkingMode = "fast" | "think" | "deep";
let chatMode: ThinkingMode = "think";
try {
  const saved = localStorage.getItem("lunac-chat-mode");
  if (saved === "fast" || saved === "think" || saved === "deep") chatMode = saved;
  else if (saved === "simple") chatMode = "fast"; // 迁移旧 simple → fast
  else if (saved === "agent") chatMode = "think"; // 迁移旧 agent → think
} catch { /* keep default */ }

const THINKING_LABEL: Record<ThinkingMode, string> = {
  fast: "chat.mode_fast",
  think: "chat.mode_think",
  deep: "chat.mode_deep",
};

/** Render the chat-mode toggle button (label + active state). */
function renderChatModeBtn() {
  if (!chatModeBtn) return;
  chatModeBtn.textContent = t(THINKING_LABEL[chatMode]);
  chatModeBtn.setAttribute("title", t("chat.mode_tooltip_" + chatMode));
  chatModeBtn.classList.toggle("simple-mode", chatMode === "fast");
  chatModeBtn.classList.toggle("deep-mode", chatMode === "deep");
}

/** Persistent conversation-mode label used in the status bar. */
function currentModeLabel(): string {
  return t(THINKING_LABEL[chatMode]);
}

// ── Agent (CLI) callbacks ─────────────────────────────────────
let cliTextCallback: ((text: string) => void) | null = null;
let cliDoneCallback: ((info?: ChatDoneInfo) => void) | null = null;
// Whether the current query produced stream_event deltas — used to skip
// the whole-message `assistant` text fallback (avoids double text).
let cliSawStreamDelta = false;
// Track consecutive tool failures for 3-fix-failure warning (ai-spec §17.2)
let consecutiveFailures = 0;

// ── Smart auto-scroll ────────────────────────────────────────────
// During AI output, auto-scroll to bottom only if user is within
// 80px of the bottom. If user scrolls up to read history, pause.
const SCROLL_THRESHOLD = 80;

function autoScrollIfNearBottom() {
  if (userScrolledUp) return;
  const dist = resultsList.scrollHeight - resultsList.scrollTop - resultsList.clientHeight;
  if (dist <= SCROLL_THRESHOLD) {
    resultsList.scrollTop = resultsList.scrollHeight;
  }
}

resultsList.addEventListener("scroll", () => {
  if (!isStreaming) {
    userScrolledUp = false;
    return;
  }
  const dist = resultsList.scrollHeight - resultsList.scrollTop - resultsList.clientHeight;
  userScrolledUp = dist > SCROLL_THRESHOLD;
});

// ── AI Chat history (localStorage) ──────────────────────────────
interface ChatSession {
  id: string;
  title: string;           // first user message, truncated
  messages: Array<{ role: string; content: string }>;
  createdAt: number;       // Date.now()
}

// ── Session persistence — file-based via Rust IPC ─────────────────
// Chat sessions are stored in Lunac 数据根（exe 所在目录）\ModuleData\history\chat-history.json
// 与 WebView2 缓存解耦，清除浏览器缓存不影响会话数据。

const MAX_SESSIONS = 50;
let currentSessionId: string | null = null;  // reuse across saves to avoid duplicates

/** Serialize saveCurrentSession calls (point 4/17). The function is invoked
 *  from several places concurrently (cliDoneCallback fire-and-forget +
 *  closePluginView/newConversation await). Without a queue, two overlapping
 *  load-modify-save cycles lose one session ("history didn't save"). */
let sessionSaveChain: Promise<void> = Promise.resolve();
function queueSessionSave(fn: () => Promise<void>): Promise<void> {
  const run = sessionSaveChain.then(fn).catch(() => {});
  sessionSaveChain = run;
  return run;
}

/** Strip the injected system-prompt hint prefix (e.g. "## Output Style ... ---")
 *  from a stored user message so restored conversations show the real question
 *  instead of the prompt boilerplate. Mirrors the title extraction in
 *  saveCurrentSession: the hint is a send-time wrapper, not conversation history. */
function stripInjectedHint(text: string): string {
  const sep = text.indexOf("\n\n---\n\n");
  return sep >= 0 ? text.slice(sep + 7) : text;
}

/** Strip the [Attached files] / [User query] wrapper that startAIChat builds
 *  for file-attached queries (point 19/17). The wrapper is a send-time format
 *  for the CLI, not conversation history — restored sessions / history previews
 *  must show the real user text. */
function stripAttachHint(text: string): string {
  const marker = "\n\n[User query]\n";
  const idx = text.indexOf(marker);
  return idx >= 0 ? text.slice(idx + marker.length) : text;
}

/** Full cleanup for a stored user message: drop the injected system hint and
 *  the attached-files wrapper so history/restore always shows real content. */
function cleanUserContent(text: string): string {
  return stripAttachHint(stripInjectedHint(text));
}

async function loadSessions(): Promise<ChatSession[]> {
  try {
    return await invoke<ChatSession[]>("load_chat_sessions");
  } catch { return []; }
}

async function saveSessions(sessions: ChatSession[]) {
  try {
    await invoke("save_chat_sessions", {
      sessions: sessions.slice(0, MAX_SESSIONS),
    });
  } catch {
    // storage unavailable — silently ignore
  }
}

/** Delete one chat session: removes it from the in-memory list AND rewrites
 *  the history file (point 18/17 — the record must vanish from disk too). */
async function deleteChatSession(id: string): Promise<ChatSession[]> {
  const sessions = await loadSessions();
  const filtered = sessions.filter(s => s.id !== id);
  await saveSessions(filtered);
  // If the deleted session was the active one, force a fresh id on next save
  // so a deleted conversation can't be silently resurrected by upsert.
  if (currentSessionId === id) currentSessionId = null;
  return filtered;
}

/** Persist a conversation snapshot. Runs inside a serial queue so concurrent
 *  callers (cliDoneCallback, closePluginView, newConversation) can't lose
 *  each other's load-modify-save cycles. Takes an explicit snapshot because
 *  callers clear chatHistory right after invoking save (e.g. ensureChatLog
 *  resets the log while the queued write is still pending). */
async function persistCurrentSessionInner(snapshotChat: Array<{ role: string; content: string }>, snapshotId: string | null) {
  // Keep any conversation with at least one real user message. Previously
  // this required 1 user + 1 assistant, so conversations where the CLI
  // errored / was closed mid-stream silently never reached history.
  if (snapshotChat.filter(m => m.role === "user" && m.content.trim()).length < 1) return;
  // Strip injected system hints from stored user messages so the history
  // file never accumulates the "## Output Style ... ---" boilerplate.
  const cleaned = snapshotChat.map(m =>
    m.role === "user" ? { ...m, content: cleanUserContent(m.content) } : m
  );
  const pruned = pruneContext(cleaned);
  const firstUser = pruned.find(m => m.role === "user");
  // Strip the injected system hint header (e.g. "## Simple Q&A Mode ... ---")
  // and the attached-files wrapper from the title so sessions show the real
  // question instead of the prompt/format boilerplate.
  const rawClean = firstUser ? cleanUserContent(firstUser.content) : "";
  const title = rawClean.trim().slice(0, 50) || "Chat";

  // Reuse session ID to avoid creating duplicate sessions per conversation
  const sid = snapshotId || (Date.now().toString(36) + Math.random().toString(36).slice(2, 6));
  currentSessionId = sid;
  const updatedAt = Date.now();

  const sessions = await loadSessions();
  const idx = sessions.findIndex(s => s.id === sid);
  const session: ChatSession = {
    id: sid,
    title,
    messages: pruned,
    createdAt: idx >= 0 ? sessions[idx].createdAt : updatedAt,
  };
  if (idx >= 0) {
    sessions[idx] = session; // update in place
  } else {
    sessions.unshift(session);
  }
  await saveSessions(sessions);
}

function saveCurrentSession() {
  // Snapshot history + id at call time — the queued persist runs later and
  // callers (ensureChatLog/newConversation) clear chatHistory immediately,
  // so reading globals inside the queue would silently drop the old chat.
  const snapshot = chatHistory.map(m => ({ ...m }));
  const snapshotId = currentSessionId;
  return queueSessionSave(() => persistCurrentSessionInner(snapshot, snapshotId));
}

function restoreSession(session: ChatSession) {
  // Continue writing to the SAME session record when the user keeps typing
  // after restore — otherwise the next save generates a brand-new id and
  // the original record is never updated (duplicate history, point 4/17).
  currentSessionId = session.id;
  // Cancel any active streaming before restoring
  if (isStreaming) {
    streamId++;
    isStreaming = false;
    setStreamingUI(false);
    cliTextCallback = null;
    cliDoneCallback = null;
  }
  agentView = null;
  agentTurn = null;
  // Strip injected system-prompt hints (## Output Style / ## Simple Q&A Mode ...)
  // from stored user messages so restored content shows the real question text.
  chatHistory = session.messages.map(m =>
    m.role === "user" ? { ...m, content: cleanUserContent(m.content) } : m
  );
  isChatHistoryView = false;
  pluginActive = true;
  activePluginId = "ai-agent";
  // Point 6/17: the plugin bar shows the session's brief content (no date/time —
  // 需求：再次进入 AI 对话时标题不应带时间) so the restored conversation is
  // recognizable at a glance.
  const briefTitle = (session.title || t("chat.empty_session")).slice(0, 20);
  setPluginBar(briefTitle);
  // Ensure chat UI buttons are in the correct state
  setStreamingUI(false);
  humanizeBtn.style.display = "none";
  invoke("set_plugin_active", { active: true }).catch(() => {});

  resultsContainer.classList.remove("hidden");
  searchBar.classList.add("has-results");

  // Build static HTML from messages — same chat-log container so the
  // conversation can continue seamlessly after restore. Every message gets a
  // roll-back button (需求4).
  renderChatLogHtml();
  statusText.textContent = t("status.history_restored");
  applyWindowSize();
  // Focus chat input so user can continue conversation immediately
  requestAnimationFrame(() => chatInput.focus());
}

// ── Chat log re-render + roll-back (需求4) ──────────────────────

/** Render chatHistory as bubbles inside #chat-log, each with a roll-back
 *  button that restores the conversation up to (and including) that node. */
function renderChatLogHtml() {
  let html = '<div class="ai-response" id="chat-log">';
  chatHistory.forEach((msg, idx) => {
    const bubble = msg.role === "user" ? "chat-msg-user" : "chat-msg-assistant";
    html += `<div class="${bubble}" data-idx="${idx}">${mdImagesToHtml(msg.content)}</div>`;
  });
  html += '</div>';
  resultsList.innerHTML = html;
  const log = document.getElementById("chat-log");
  // 需求：复制按钮所有气泡都有；回退/重试按钮只出现在用户提问气泡。
  log?.querySelectorAll<HTMLElement>(".chat-msg-user").forEach(msg => {
    const idx = Number(msg.dataset.idx);
    attachMsgActions(msg, Number.isFinite(idx) ? idx : undefined);
  });
  log?.querySelectorAll<HTMLElement>(".chat-msg-assistant").forEach(msg => {
    const idx = Number(msg.dataset.idx);
    attachMsgCopy(msg, Number.isFinite(idx) ? idx : undefined);
  });
  // Render LaTeX in restored content
  setTimeout(() => {
    const container = resultsList.querySelector(".ai-response");
    if (container) renderLatex(container as HTMLElement);
  }, 20);
  applyWindowSize();
}

/** 复制气泡原文到剪贴板（含 [Attached files] 前缀时只复制用户提问原文）。 */
async function copyMsgText(msg: HTMLElement, idx: number | undefined) {
  let text = "";
  if (idx !== undefined && idx >= 0 && chatHistory[idx]) {
    text = parseAttachedQuery(chatHistory[idx].content).text;
  }
  if (!text.trim()) text = msg.textContent || "";
  const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
  writeText(text.trim()).catch(() => {});
}

/** 在【任意气泡】上附加复制按钮（幂等，印象派 COPY_SVG）——用于 AI 回答气泡。 */
function attachMsgCopy(msg: HTMLElement, idx: number | undefined) {
  if (msg.querySelector(".msg-actions")) return;
  const wrap = doc("div");
  wrap.className = "msg-actions";
  const cp = doc("button");
  cp.className = "msg-copy";
  cp.title = t("chat.copy_msg");
  cp.innerHTML = COPY_SVG;
  cp.addEventListener("click", () => copyMsgText(msg, idx));
  wrap.appendChild(cp);
  msg.appendChild(wrap);
}

/** 在【用户提问气泡】上附加复制 + 回退 + 重试三个按钮（幂等，印象派 SVG 图案）。
 *  需求：回退/重试只出现在用户提问的气泡里；复制按钮所有气泡都有。 */
function attachMsgActions(msg: HTMLElement, idx: number | undefined) {
  if (idx === undefined || idx < 0) return;
  if (msg.querySelector(".msg-actions")) return;
  const wrap = doc("div");
  wrap.className = "msg-actions";
  const cp = doc("button");
  cp.className = "msg-copy";
  cp.title = t("chat.copy_msg");
  cp.innerHTML = COPY_SVG;
  cp.addEventListener("click", () => copyMsgText(msg, idx));
  const rb = doc("button");
  rb.className = "msg-rollback";
  rb.title = t("chat.rollback");
  rb.innerHTML = ROLLBACK_SVG;
  rb.addEventListener("click", () => rollbackChat(idx));
  const rt = doc("button");
  rt.className = "msg-retry";
  rt.title = t("chat.retry");
  rt.innerHTML = RETRY_SVG;
  rt.addEventListener("click", () => retryChat(idx));
  wrap.appendChild(cp);
  wrap.appendChild(rb);
  wrap.appendChild(rt);
  msg.appendChild(wrap);
}

/** 解析 startAIChat 拼装的 finalQuery（含 [Attached files] 前缀），
 *  恢复原始提问文本与附加文件列表，供重试复用。 */
function parseAttachedQuery(q: string): { text: string; files: string[] } {
  const m = q.match(/^\[Attached files\]\n([\s\S]*?)\n\n\[User query\]\n([\s\S]*)$/);
  if (!m) return { text: q, files: [] };
  return {
    files: m[1].split("\n").map(l => l.replace(/^- /, "")).filter(Boolean),
    text: m[2],
  };
}

/** 重试：回退到该条用户消息之前（保留先前上下文），再重新发送其内容。
 *  idx=0 时回退到空对话。rollbackChat 内部负责停流/重置 CLI/重渲染。 */
async function retryChat(idx: number) {
  if (idx < 0 || !chatHistory[idx] || chatHistory[idx].role !== "user") return;
  const { text, files } = parseAttachedQuery(chatHistory[idx].content);
  await rollbackChat(idx - 1);
  await startAIChat(text, files);
}

/** 回退/重试按钮 SVG —— 印象派：扫笔残影 + 主体 + 点彩高光。
 *  回退 = 单直线向左箭头（需求：不脱离当前图案主题，保留残影+点彩）。 */
const ROLLBACK_SVG = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
  <g opacity="0.28" transform="translate(-0.6,-0.6)" stroke-width="1.8">
    <line x1="19" y1="12" x2="5" y2="12"/><polyline points="11 5 5 12 11 19"/>
  </g>
  <line x1="19" y1="12" x2="5" y2="12"/><polyline points="11 5 5 12 11 19"/>
  <circle cx="6.5" cy="6" r="0.9" fill="currentColor" stroke="none"/>
  <circle cx="17.5" cy="17.5" r="0.4" fill="currentColor" stroke="none" opacity="0.45"/>
</svg>`;
const RETRY_SVG = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
  <g opacity="0.28" transform="translate(-0.6,-0.6)" stroke-width="1.8">
    <path d="M21 12a9 9 0 1 1-3-6.7L21 8"/><polyline points="21 3 21 8 16 8"/>
  </g>
  <path d="M21 12a9 9 0 1 1-3-6.7L21 8"/><polyline points="21 3 21 8 16 8"/>
  <circle cx="7" cy="7" r="0.9" fill="currentColor" stroke="none"/>
  <circle cx="17.5" cy="17.5" r="0.4" fill="currentColor" stroke="none" opacity="0.45"/>
</svg>`;

/** 复制按钮 SVG —— 同主题（双矩形 + 扫笔残影 + 点彩高光）。 */
const COPY_SVG = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
  <g opacity="0.28" transform="translate(-0.6,-0.6)" stroke-width="1.8">
    <rect x="9" y="9" width="12" height="12" rx="2"/>
  </g>
  <rect x="9" y="9" width="12" height="12" rx="2"/>
  <path d="M5 15V5a2 2 0 0 1 2-2h10"/>
  <circle cx="7.5" cy="7.5" r="0.9" fill="currentColor" stroke="none"/>
  <circle cx="18" cy="18" r="0.4" fill="currentColor" stroke="none" opacity="0.45"/>
</svg>`;

/** Roll back the conversation to message `idx` (inclusive): strips all later
 *  messages, persists the trimmed session, resets the CLI context and
 *  re-renders. A full snapshot is backed up to localStorage first so the
 *  operation is reversible (no data loss). idx=-1 表示回退到空对话（重试首条消息用）。 */
async function rollbackChat(idx: number) {
  if (idx < -1 || (idx >= 0 && !chatHistory[idx])) return;
  if (isStreaming) {
    streamId++;
    isStreaming = false;
    setStreamingUI(false);
    cliTextCallback = null;
    cliDoneCallback = null;
    agentView = null;
    agentTurn = null;
  }
  // 1) Backup full snapshot (无数据丢失)
  try {
    const snaps: { ts: number; sessionId: string | null; messages: { role: string; content: string }[] }[] =
      JSON.parse(localStorage.getItem("lunac-rollback-snapshots") || "[]");
    snaps.unshift({ ts: Date.now(), sessionId: currentSessionId, messages: chatHistory.map(m => ({ ...m })) });
    localStorage.setItem("lunac-rollback-snapshots", JSON.stringify(snaps.slice(0, 50)));
  } catch {}
  // 2) Trim to the roll-back node (inclusive)
  chatHistory = chatHistory.slice(0, idx + 1);
  // 3) Persist the trimmed session
  try { await saveCurrentSession(); } catch {}
  // 4) Reset CLI context so the next turn doesn't see the removed messages
  try { await invoke("stop_cli"); } catch {}
  cliReady = false;
  try { await invoke("start_cli"); } catch {}
  // 5) Re-render
  renderChatLogHtml();
  statusText.textContent = t("chat.rolled_back");
  requestAnimationFrame(() => chatInput.focus());
}

async function showChatHistory() {
  // Cancel any active streaming before showing history
  if (isStreaming) {
    streamId++;
    isStreaming = false;
    setStreamingUI(false);
  }
  isChatHistoryView = true;
  pluginActive = true;
  activePluginId = "ai-agent";
  setPluginBar(t("chat.ai_entry"));
  searchInput.disabled = true;
  invoke("set_plugin_active", { active: true }).catch(() => {});
  const sessions = await loadSessions();
  resultsContainer.classList.remove("hidden");
  searchBar.classList.add("has-results");

  if (sessions.length === 0) {
    resultsList.innerHTML = `
      <div class="plugin-result">
        <div class="plugin-result-content" style="color:var(--text-dim);padding:16px 12px;">
          ${t("chat.no_history")}</div>
      </div>`;
    statusText.textContent = t("chat.session_count", { count: "0" });
    applyWindowSize();
    return;
  }

  let html = `<div class="plugin-result">`;


  for (const s of sessions) {
    const preview = s.messages.find(m => m.role === "assistant")?.content.slice(0, 150) || "";
    // Point 19/17: titles saved before the fix may still carry the
    // [Attached files] / [User query] wrapper — clean on display too.
    // 需求：标题严格居中、不显示时间，meta 仅保留消息条数。
    const cleanTitle = cleanUserContent(s.title);
    html += `
      <div class="history-item" data-sid="${s.id}">
        <div class="history-item-header">
          <span class="history-item-icon">💬</span>
          <div class="history-item-info">
            <div class="history-item-title">${esc(cleanTitle)}</div>
            <div class="history-item-meta">${t("chat.messages", { count: String(s.messages.length) })}</div>
          </div>
          <button class="history-item-delete" title="${esc(t("chat.delete_session"))}">×</button>
        </div>
        <div class="history-item-preview">${esc(preview || t("chat.assistant_response"))}</div>
      </div>`;
  }
  html += '</div>';
  resultsList.innerHTML = html;
  statusText.textContent = t("chat.session_count", { count: String(sessions.length) });

  // Bind click: first click opens drawer, second click restores
  resultsList.querySelectorAll(".history-item").forEach(el => {
    const header = el.querySelector(".history-item-header") as HTMLElement;
    header.addEventListener("click", () => {
      const sid = (el as HTMLElement).dataset.sid;
      const session = sessions.find(s => s.id === sid);
      if (!session) return;
      if (el.classList.contains("open")) {
        restoreSession(session);
        resultsList.querySelectorAll(".history-item.open").forEach(e => e.classList.remove("open"));
      } else {
        resultsList.querySelectorAll(".history-item.open").forEach(e => e.classList.remove("open"));
        el.classList.add("open");
      }
    });
    // Delete button: remove record + persist to history file, then re-render
    const delBtn = el.querySelector(".history-item-delete") as HTMLElement;
    delBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      const sid = (el as HTMLElement).dataset.sid;
      if (!sid) return;
      deleteChatSession(sid).then(() => {
        showChatHistory();
      }).catch(() => {});
    });
  });
  applyWindowSize();
}

// ── AI Chat state ───────────────────────────────────────────────
// Simple mode removed (2026-08-04): the app always runs Agent (agent.exe).

function forceResetPluginUI() {
  // Exit any detached mode
  if (detached) {
    setDetached(false);
  }
  // Reset plugin bar
  setPluginBar(null);
  // Reset plugin state flags
  pluginActive = false;
  activePluginId = null;
  isChatHistoryView = false;
  // Close drawer
  closeDrawer();
  // Reset agent state machine (Pi)
  agentReset();
  // Clear streaming state
  if (isStreaming) {
    streamId++;
    isStreaming = false;
    setStreamingUI(false);
  }
  cliTextCallback = null;
  cliDoneCallback = null;
  pendingMessages = [];
  // Ensure both inputs are unlocked
  searchInput.disabled = false;
  chatInput.disabled = false;
  // Hide chat input bar
  chatInputBar.classList.add("hidden");
  resultsContainer.classList.remove("ai-chat");
  showTokenDashboard(false);
}

// ── Init i18n + plugins ──────────────────────────────────────────
(async () => {
  await initI18n();
  loadSavedLanguage();
  applyI18nToStaticUI();
  registerBuiltinPlugins();
  statusText.textContent = t("status.plugin_count", { count: String(pluginRegistry.getAll().length) });

  // 预热备忘录标识检索索引（供搜索栏“标识直达编辑”使用）
  import("./plugins/builtin/memo").then(m => m.refreshMemoIndex()).catch(() => {});

  // Restore user AI provider config (settings plugin) — overrides .env defaults
  try {
    const raw = localStorage.getItem("lunac-ai-config");
    if (!raw) return;
    const cfg = JSON.parse(raw);
    if (cfg?.url && cfg?.model) {
      invoke("set_ai_config", {
        provider: cfg.provider || "deepseek",
        url: cfg.url,
        key: cfg.key || "",
        model: cfg.model,
        agent_url: cfg.agent_url || null,
      }).catch(() => {});
    }
  } catch { /* keep .env defaults */ }

  // Restore AI workspace — scopes the agent to a folder; empty = whole system
  try {
    const ws = localStorage.getItem("lunac-agent-workspace");
    if (ws) {
      invoke("set_workspace", { path: ws }).then(() => {
        currentWorkspace = ws;
        refreshWorkspaceUI();
      }).catch(() => {});
    }
  } catch { /* no workspace configured */ }

  // Restore custom tool blacklist (第19点) — Rust merges it with defaults
  // on the next agent.exe start.
  try {
    const bl = localStorage.getItem("lunac-tool-blacklist");
    if (bl) {
      const arr = JSON.parse(bl);
      if (Array.isArray(arr)) {
        invoke("set_tool_blacklist", { blacklist: arr.filter((x: unknown) => typeof x === "string") }).catch(() => {});
      }
    }
  } catch { /* no blacklist saved */ }

  // Sync DeepSeek thinking mode (fast/think/deep) to Rust on startup.
  // restart=false → 只存值，不拉起 agent.exe（保持懒启动）。
  invoke("set_thinking_mode", { mode: chatMode, restart: false }).catch(() => {});
})();

// ── Drag ─────────────────────────────────────────────────────────
// uTools-style: all non-interactive areas are drag handles.
// Input area uses a movement threshold to distinguish click from drag.

let dragging = false; // true during any drag operation — prevents auto-hide

// Drag handler factory — generic mouse-move threshold drag
function makeDragHandle(el: HTMLElement) {
  el.addEventListener("mousedown", (e) => {
    const target = e.target as HTMLElement;
    // Don't drag on buttons or interactive elements
    if (target.tagName === "BUTTON" || target.closest("button") || target.closest("input")) {
      return;
    }
    const startX = e.clientX;
    const startY = e.clientY;
    let dragged = false;

    const onMove = (ev: MouseEvent) => {
      if (!dragged && (Math.abs(ev.clientX - startX) > 3 || Math.abs(ev.clientY - startY) > 3)) {
        dragged = true;
        dragging = true;
        win.startDragging();
      }
    };
    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      if (dragged) {
        // Keep dragging=true for a short grace period to prevent
        // onFocusChanged auto-hide from firing during drag cleanup
        setTimeout(() => { dragging = false; }, 200);
      }
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  });
}

// Search bar (non-input area)
makeDragHandle(searchBar);

// Window drag is intentionally LIMITED to the title-bar areas only
// (search bar / plugin title bar / detached header — all carry
// `data-tauri-drag-region` + makeDragHandle). The results/chat body must
// NOT start drags: a mousedown handler here calls preventDefault() which
// kills text selection initiation, and a 3px move hijacks the whole window
// — that's why AI chat text couldn't be selected (需求3). Body text stays
// fully selectable; move the window from the title bar instead.

// Search input — uTools-style: disable pointer events during drag
searchInput.addEventListener("mousedown", (e) => {
  const startX = e.clientX;
  const startY = e.clientY;
  let dragged = false;

  const onMove = (ev: MouseEvent) => {
    if (!dragged && (Math.abs(ev.clientX - startX) > 3 || Math.abs(ev.clientY - startY) > 3)) {
      dragged = true;
      dragging = true;
      // Disable input interaction during drag (uTools-style cursor switch)
      searchInput.style.pointerEvents = "none";
      win.startDragging();
    }
  };
  const onUp = () => {
    document.removeEventListener("mousemove", onMove);
    document.removeEventListener("mouseup", onUp);
    if (dragged) {
      // Restore after drag ends
      setTimeout(() => {
        searchInput.style.pointerEvents = "";
        dragging = false;
        // Re-focus if needed
        if (isVisible && !pluginActive) searchInput.focus();
      }, 200);
    }
  };
  document.addEventListener("mousemove", onMove);
  document.addEventListener("mouseup", onUp);
});

// ── Plugin bar lock — search bar transforms to plugin title + exit ─
function setPluginBar(title: string | null) {
  if (title) {
    searchBar.classList.add("plugin-locked");
    pluginBarTitle.textContent = title;
    searchInput.disabled = true;
    // Show chat input bar for AI chat
    if (activePluginId === "ai-agent") {
      resultsContainer.classList.add("ai-chat");
      const st = pluginStates.get("ai-agent");
      chatInput.value = st?.pendingInput || "";
      showTokenDashboard(true);
      statusHint.classList.add("hidden");
    }
  } else {
    searchBar.classList.remove("plugin-locked");
    searchInput.disabled = false;
    resultsContainer.classList.remove("ai-chat");
    showTokenDashboard(false);
    statusHint.classList.remove("hidden");
  }
  // Re-render file chips so they live in the chat bar while AI is active
  // and return to the search bar otherwise (point 5/17).
  renderFileChips();
}

// Refocus chat input when clicking status bar / token dashboard in AI chat —
// prevents "locked dialog" after user clicks the status area
function refocusChatIfActive() {
  if (pluginActive && activePluginId === "ai-agent" && !isStreaming) {
    requestAnimationFrame(() => chatInput.focus());
  }
}

// ── Plugin bar exit button → close plugin view ─────────────────
pluginBarExit.addEventListener("click", async () => {
  if (pluginActive) await closePluginView();
});

// Clicking status bar / token dashboard in AI chat refocuses chat input
document.getElementById("status-bar")?.addEventListener("click", (e) => {
  // Don't intercept clicks on interactive elements (buttons, etc.)
  if ((e.target as HTMLElement).closest("button")) return;
  refocusChatIfActive();
});

// ── Chat input bar handlers ──────────────────────────────────

// Agent CLI status tracking
let agentStatus: "idle" | "loading" | "ready" | "error" = "idle";
let agentStatusEl: HTMLElement | null = null;

function updateAgentStatus(status: "idle" | "loading" | "ready" | "error", msg?: string) {
  agentStatus = status;
  // Re-create if detached from DOM (e.g. statusText.textContent nuked children)
  if (!agentStatusEl || !agentStatusEl.parentNode) {
    agentStatusEl = doc("span");
    agentStatusEl.className = "agent-status";
    statusText.appendChild(agentStatusEl);
  }
  agentStatusEl.className = `agent-status ${status}`;
  if (status === "loading") agentStatusEl.textContent = t("agent.loading");
  else if (status === "ready") agentStatusEl.textContent = t("agent.ready");
  else if (status === "error") agentStatusEl.textContent = msg ? t("agent.error") + ": " + msg.slice(0, 40) : t("agent.error");
  else { agentStatusEl.textContent = ""; agentStatusEl.className = "agent-status"; }
}

// New conversation: clear state + show fresh chat UI.
// When skipCliRestart is true, the CLI is left untouched (already fresh);
// when false (called from the "new conversation" button), kill and restart
// the CLI to clear accumulated conversation context.
async function newConversation(skipCliRestart = false) {
  // Save current session if exists
  await saveCurrentSession().catch(() => {});
  currentSessionId = null;  // start a fresh session
  chatHistory = [];
  if (isStreaming) {
    streamId++;
    isStreaming = false;
    setStreamingUI(false);
  }
  cliTextCallback = null;
  cliDoneCallback = null;
  agentView = null;
  agentTurn = null;
  pendingMessages = [];

  // Restart CLI to clear accumulated conversation context.
  // Without this, the CLI retains all previous messages in its
  // internal state — new messages are processed with full history,
  // so the LLM generates responses based on prior context rather
  // than treating it as a fresh conversation.
  if (!skipCliRestart) {
    await invoke("stop_cli").catch(() => {});
    cliReady = false;
    updateAgentStatus("ready");
  }

  humanizeBtn.style.display = "none";
  closeDrawer();

  // Show fresh AI chat UI
  pluginActive = true;
  activePluginId = "ai-agent";
  isChatHistoryView = false;
  setPluginBar(t("chat.ai_entry"));
  invoke("set_plugin_active", { active: true }).catch(() => {});
  resultsContainer.classList.remove("hidden");
  searchBar.classList.add("has-results");
  resultsList.innerHTML = `<div class="ai-response" id="chat-log"><div class="chat-msg-system">${t("chat.new_conversation_msg", { mode: currentModeLabel() })}</div></div>`;
  statusText.textContent = t("status.ai", { mode: currentModeLabel() });
  chatInput.value = "";
  autoResizeChatTextarea();
  chatInput.focus();
  applyWindowSize();
}
chatNewBtn.addEventListener("click", () => newConversation());

// Toggle DeepSeek thinking mode: fast → think → deep → fast.
chatModeBtn.addEventListener("click", () => {
  chatMode = chatMode === "fast" ? "think" : chatMode === "think" ? "deep" : "fast";
  try { localStorage.setItem("lunac-chat-mode", chatMode); } catch {}
  renderChatModeBtn();
  invoke("set_thinking_mode", { mode: chatMode, restart: true }).catch((e) => console.warn("set_thinking_mode", e));
  if (statusText) statusText.textContent = t("status.ai", { mode: currentModeLabel() });
});

// ── AI workspace (chat window entry — the ONLY workspace UI) ─────
// Default (empty) workspace = user home dir → whole system reachable,
// sensitive edits outside it still go through ask approval cards.
let currentWorkspace = "";
function refreshWorkspaceUI() {
  if (!chatWorkspacePath) return;
  chatWorkspacePath.textContent = currentWorkspace || t("settings.workspace_default");
  chatWorkspacePath.setAttribute("title", currentWorkspace || "");
  if (chatWorkspaceBtn) {
    chatWorkspaceBtn.classList.toggle("has-workspace", !!currentWorkspace);
    chatWorkspaceBtn.setAttribute("title", currentWorkspace || t("settings.workspace_default"));
  }
}
async function applyWorkspace(path: string): Promise<boolean> {
  try {
    await invoke("set_workspace", { path });
    try {
      if (path) {
        localStorage.setItem("lunac-agent-workspace", path);
      } else {
        localStorage.removeItem("lunac-agent-workspace");
      }
    } catch {}
    currentWorkspace = path;
    refreshWorkspaceUI();
    // Restart CLI so the new scope takes effect on next conversation.
    await invoke("stop_cli").catch(() => {});
    await invoke("start_cli").catch(() => {});
    return true;
  } catch {
    return false;
  }
}
chatWorkspaceBtn?.addEventListener("click", (e) => {
  e.stopPropagation();
  // Toggle the workspace picker menu (select folder / reset)
  chatWorkspaceMenu?.classList.toggle("hidden");
});
chatWorkspaceSelect?.addEventListener("click", async () => {
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true, multiple: false, title: t("settings.workspace_select") });
    if (typeof picked === "string" && picked) {
      const ok = await applyWorkspace(picked);
      if (!ok) statusText.textContent = t("settings.workspace_fail");
    }
  } catch {}
  closeWorkspaceMenu();
});
chatWorkspaceReset?.addEventListener("click", async () => {
  const ok = await applyWorkspace("");
  if (!ok) statusText.textContent = t("settings.workspace_fail");
  closeWorkspaceMenu();
});
function closeWorkspaceMenu() {
  chatWorkspaceMenu?.classList.add("hidden");
}
document.addEventListener("click", (e) => {
  const wrap = chatWorkspaceBtn?.parentElement;
  if (wrap && !wrap.contains(e.target as Node)) {
    closeWorkspaceMenu();
  }
});
// Keep the menu's button labels in sync with the current language
function renderWorkspaceMenuLabels() {
  if (chatWorkspaceSelect) chatWorkspaceSelect.textContent = t("settings.workspace_select");
  if (chatWorkspaceReset) chatWorkspaceReset.textContent = t("settings.workspace_reset");
}

// ── AI tool blacklist (第19点缓存优化 — 入口在 AI 对话界面 🛠) ──
// 勾选 = 从请求体 tools schema 中剔除 → tools 数组更短，且前缀里不再有
// 用不上的工具定义（DeepSeek 自动前缀缓存按最长公共前缀命中）。
// 名单 = agent.exe 的真实内置工具 + Skill（MCP 用户工具名在运行期才知道，
// 不在此列出；要禁用它们请从 `--disallowedTools` 侧或删掉 tools\*.json）。
interface BlacklistTool { name: string; locked?: boolean; }
const TOOL_BLACKLIST_CANDIDATES: BlacklistTool[] = [
  { name: "Read" },
  { name: "Write" },
  { name: "Edit" },
  { name: "Bash" },
  { name: "PowerShell" },
  { name: "Glob" },
  { name: "Grep" },
  { name: "WebFetch" },
  { name: "Skill" },
];

function loadCustomBlacklist(): string[] {
  try {
    const raw = localStorage.getItem("lunac-tool-blacklist");
    if (raw) {
      const arr = JSON.parse(raw);
      if (Array.isArray(arr)) return arr.filter((x: unknown) => typeof x === "string");
    }
  } catch {}
  return [];
}

function renderToolsBlacklist() {
  if (!chatToolsList || !chatToolsTitle) return;
  chatToolsTitle.textContent = t("chat.tools_title");
  const custom = new Set(loadCustomBlacklist());
  chatToolsList.innerHTML = TOOL_BLACKLIST_CANDIDATES.map(tool => `
    <label class="chat-tools-item${tool.locked ? " locked" : ""}" title="${esc(tool.name)}">
      <input type="checkbox" data-tool="${esc(tool.name)}" ${tool.locked ? "checked disabled" : custom.has(tool.name) ? "checked" : ""}>
      <span>${esc(tool.name)}</span>
      ${tool.locked ? `<span class="chat-tools-desc">${esc(t("chat.tools_locked"))}</span>` : ""}
    </label>
  `).join("");
}
chatToolsBtn?.addEventListener("click", (e) => {
  e.stopPropagation();
  const willOpen = chatToolsMenu?.classList.contains("hidden") ?? false;
  chatToolsMenu?.classList.toggle("hidden");
  if (willOpen) renderToolsBlacklist();
});
document.addEventListener("click", (e) => {
  const wrap = chatToolsBtn?.parentElement;
  if (wrap && !wrap.contains(e.target as Node)) {
    chatToolsMenu?.classList.add("hidden");
  }
});
// 保存流程（用户需求顺序）：点击保存 → 按钮原位切换提醒 → 保存历史 →
// 退出 agent.exe → 重启 agent.exe 应用新黑名单（具体在 __lunac_save_tool_blacklist）
chatToolsSave?.addEventListener("click", async () => {
  if (!chatToolsSave) return;
  const custom: string[] = [];
  chatToolsList?.querySelectorAll<HTMLInputElement>("input[type=checkbox]:checked:not(:disabled)").forEach(cb => {
    const name = cb.getAttribute("data-tool");
    if (name) custom.push(name);
  });
  (chatToolsSave as HTMLButtonElement).disabled = true;
  chatToolsSave.textContent = t("chat.tools_restarting");
  try {
    const result = await (window as any).__lunac_save_tool_blacklist?.(custom);
    if (chatToolsMsg) {
      chatToolsMsg.textContent = result === "ok" ? t("chat.tools_saved") : t("chat.tools_failed");
    }
  } catch {
    if (chatToolsMsg) chatToolsMsg.textContent = t("chat.tools_failed");
  }
  setTimeout(() => { if (chatToolsMsg) chatToolsMsg.textContent = ""; }, 3000);
  (chatToolsSave as HTMLButtonElement).disabled = false;
  chatToolsSave.textContent = t("chat.tools_save");
});
function renderToolsBlacklistLabels() {
  if (chatToolsSave) chatToolsSave.textContent = t("chat.tools_save");
  if (chatToolsBtn) chatToolsBtn.setAttribute("title", t("chat.tools_btn"));
}

// ── Drawer: open / close with slide animation ───────────────────
let drawerVisible = false;
let drawerBackdrop: HTMLElement | null = null;

function openDrawer() {
  if (drawerVisible) return;
  drawerVisible = true;
  // Re-insert drawer into DOM (physically removed when closed)
  drawerParent.insertBefore(chatDrawer, drawerSibling);
  chatDrawer.classList.remove("hidden");
  void chatDrawer.offsetWidth; // force reflow so transition fires
  chatDrawer.classList.add("visible");

  if (!drawerBackdrop) {
    drawerBackdrop = doc("div");
    drawerBackdrop.id = "chat-drawer-backdrop";
    resultsContainer.appendChild(drawerBackdrop);
    drawerBackdrop.addEventListener("click", () => closeDrawer());
  }
  drawerBackdrop.classList.add("visible");
  loadSessions().then(s => renderDrawerHistory(s)).catch(() => {});
}

function closeDrawer() {
  if (!drawerVisible) return;
  drawerVisible = false;
  chatDrawer.classList.remove("visible");
  if (chatDrawer.parentElement) chatDrawer.remove();
  chatDrawer.classList.add("hidden"); // clean class state for re-insert

  if (drawerBackdrop) {
    drawerBackdrop.classList.remove("visible");
    drawerBackdrop.remove();
    drawerBackdrop = null;
  }

  // Refocus chat input — drawer removal may leave no focused element
  if (pluginActive && activePluginId === "ai-agent") {
    requestAnimationFrame(() => chatInput.focus());
  }
}

function toggleDrawer(show?: boolean) {
  const target = show !== undefined ? show : !drawerVisible;
  if (target) openDrawer(); else closeDrawer();
}

function renderDrawerHistory(sessions: ChatSession[]) {
  chatDrawerList.innerHTML = "";
  if (sessions.length === 0) {
    chatDrawerList.innerHTML = `<div style="padding:16px;text-align:center;color:var(--text-dim);font-size:0.78rem;">${t("chat.no_history_short")}</div>`;
    return;
  }
  sessions.forEach((s, i) => {
    const item = doc("div");
    item.className = "history-item";
    const firstMsg = cleanUserContent(s.messages[0]?.content || t("chat.empty_session"));
    const preview = firstMsg.length > 60 ? firstMsg.slice(0, 60) + "…" : firstMsg;
    const msgCount = s.messages.length;
    // 需求：标题严格居中、不显示时间，meta 仅保留消息条数。
    item.innerHTML = `
      <div class="history-item-header">
        <span class="history-item-icon">💬</span>
        <div class="history-item-info">
          <div class="history-item-title">${esc(preview)}</div>
          <div class="history-item-meta">${t("chat.msgs", { count: String(msgCount) })}</div>
        </div>
        <button class="history-item-delete" title="${esc(t("chat.delete_session"))}">×</button>
      </div>`;
    item.querySelector(".history-item-header")!.addEventListener("click", async () => {
      toggleDrawer(false);
      await restoreSession(s);
    });
    // Delete button: persist to history file + refresh drawer list
    item.querySelector(".history-item-delete")!.addEventListener("click", async (e) => {
      e.stopPropagation();
      await deleteChatSession(s.id);
      const remaining = await loadSessions();
      renderDrawerHistory(remaining);
    });
    chatDrawerList.appendChild(item);
  });
}

chatHistoryBtn.addEventListener("click", () => toggleDrawer());
chatDrawerClose.addEventListener("click", () => toggleDrawer(false));

function sendChatMessage() {
  const text = chatInput.value.trim();
  if (!text && attachedFiles.length === 0) return;
  // Queue messages while streaming — don't silently drop them
  if (isStreaming) {
    pendingMessages.push(text || "(files only)");
    statusText.textContent = t("status.queued", { count: String(pendingMessages.length) });
    chatInput.value = "";
    autoResizeChatTextarea();
    return;
  }
  chatInput.value = "";
  autoResizeChatTextarea();
  startAIChat(text);
}

// ── Stop / Queue helpers ────────────────────────────────────────

function setStreamingUI(streaming: boolean) {
  if (streaming) {
    chatSendBtn.style.display = "none";
    chatStopBtn.style.display = "flex";
    chatInput.placeholder = t("chat.generating");
  } else {
    chatSendBtn.style.display = "flex";
    chatStopBtn.style.display = "none";
    chatInput.placeholder = t("chat.placeholder");
  }
}

// ── Token dashboard ─────────────────────────────────────────────

/** Format token count: 1,234 → "1.2k" or "987" */
function fmtTokens(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (n >= 1_000) return (n / 1_000).toFixed(1) + "k";
  return String(n);
}

/** Update the token dashboard with cumulative session stats.
 *  Billing-accurate accounting (provider-billed tokens, per turn):
 *  Hit   = cache_read_input_tokens    (cached context, billed at ~10%)
 *  Miss  = input_tokens + cache_creation_input_tokens  (new input + cache writes)
 *  Total = input + cache_creation + cache_read + output (all billed tokens)
 *  NOTE: Anthropic's usage.input_tokens does NOT include cache tokens, so
 *  "miss" must add cache_creation instead of subtracting cache_read. */
function updateTokenDashboard(info?: ChatDoneInfo) {
  if (info) {
    const cacheHit = info.cache_read_input_tokens || 0;
    const cacheCreate = info.cache_creation_input_tokens || 0;
    const cacheMiss = info.input_tokens + cacheCreate;
    const total = cacheMiss + cacheHit + info.output_tokens;
    sessionTokens.hit += cacheHit;
    sessionTokens.miss += cacheMiss;
    sessionTokens.total += total;
  }

  const { hit, miss, total } = sessionTokens;
  const inputTokens = hit + miss;
  const hitPct = inputTokens > 0 ? Math.round((hit / inputTokens) * 100) : 0;

  tokenDashboard.innerHTML =
    `<span class="tk-bar" title="${t("token.cache_tooltip", { hit: fmtTokens(hit), miss: fmtTokens(miss), total: fmtTokens(total) })}">` +
      `<span class="tk-bar-fill tk-bar-hit" style="width:${hitPct}%"></span>` +
      `<span class="tk-bar-fill tk-bar-miss" style="width:${100 - hitPct}%"></span>` +
    `</span>` +
    `<span class="tk-pct">${hitPct}%</span>` +
    `<span class="tk-total" title="${t("token.detail_tooltip", { hit: fmtTokens(hit), miss: fmtTokens(miss), out: fmtTokens(total - inputTokens), total: fmtTokens(total) })}">${fmtTokens(total)}</span>`;
}

function showTokenDashboard(show: boolean) {
  if (show) {
    sessionTokens = { hit: 0, miss: 0, total: 0 };
    lastAgentTokens = { input: 0, output: 0, cacheRead: 0, cacheCreate: 0 };
    tokenDashboard.classList.remove("hidden");
    tokenDashboard.classList.add("visible");
    updateTokenDashboard(); // render "0% · 0"
  } else {
    tokenDashboard.classList.remove("visible");
    tokenDashboard.classList.add("hidden");
    sessionTokens = { hit: 0, miss: 0, total: 0 };
    lastAgentTokens = { input: 0, output: 0, cacheRead: 0, cacheCreate: 0 };
  }
}

async function stopAIChat() {
  if (!isStreaming) return;
  // Cancel streaming by advancing streamId — old callbacks will no-op
  streamId++;
  cliTextCallback = null;
  cliDoneCallback = null;
  isStreaming = false;
  setStreamingUI(false);

  // Stop the CLI (agent mode) — kills agent.exe and clears its context.
  await invoke("stop_cli").catch(() => {});
  cliReady = false;
  agentView = null;
  agentTurn = null;
  clearPermissionCards();

  // Remove loading cursor from last message
  const loading = resultsList.querySelector(".cursor-blink");
  if (loading) loading.remove();

  if (pendingMessages.length > 0) {
    statusText.textContent = t("status.stopped_queued", { count: String(pendingMessages.length) });
  } else {
    statusText.textContent = t("status.stopped");
  }

  // Process queued messages
  processQueue();
}

async function processQueue() {
  while (pendingMessages.length > 0 && !isStreaming) {
    const msg = pendingMessages.shift()!;
    statusText.textContent = t("status.sending_queued", { count: String(pendingMessages.length) });
    await startAIChat(msg);
    // Brief pause to let the caller settle
    await new Promise(r => setTimeout(r, 100));
  }
}

chatStopBtn.addEventListener("click", stopAIChat);

chatSendBtn.addEventListener("click", sendChatMessage);

chatInput.addEventListener("keydown", (e) => {
  // Backspace on empty → remove last chip（与搜索栏一致：省略泡泡折叠态下
  // 视为删除该省略分支，即同步到省略号内“一键删除”按钮，仅作用于省略号内容）
  if (e.key === "Backspace" && !chatInput.value && attachedFiles.length > 0) {
    e.preventDefault();
    if (!chipMoreOpen && attachedFiles.length > FILE_CHIP_LIMIT) {
      clearHiddenFileChips();
    } else {
      removeFileChip(attachedFiles.length - 1);
    }
    return;
  }
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    sendChatMessage();
  }
});

chatInput.addEventListener("input", () => {
  autoResizeChatTextarea();
});

// "+" button: native file dialog (returns full paths, unlike HTML file input)
chatAddFileBtn.addEventListener("click", async () => {
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const selected = await open({ multiple: true, title: t("dialog.attach_files") });
    if (Array.isArray(selected)) {
      selected.forEach(p => addFileChip(p));
    } else if (selected) {
      addFileChip(selected);
    }
  } catch (err) {
    statusText.textContent = t("status.dialog_error", { err: String(err) });
  }
});

// Humanize button — append humanize request for last assistant response
humanizeBtn.addEventListener("click", () => {
  const lastAssistant = chatHistory.filter(m => m.role === "assistant").pop();
  if (!lastAssistant || isStreaming) return;
  const humanizeQuery = "Rewrite the following to remove AI writing patterns. Follow the humanizer methodology: remove significance inflation, promotional language, AI vocabulary words, copula avoidance, em dashes, boldface headers, emoji decorations, collaborative artifacts, knowledge-cutoff disclaimers, filler phrases, and generic conclusions. Add a human voice with varied rhythm, opinions, and specific details. Output ONLY the rewritten text:\n\n" + lastAssistant.content;
  chatInput.value = humanizeQuery;
  autoResizeChatTextarea();
  sendChatMessage();
});

// ── Detach mode — hide search bar, expand results panel ────────
function setDetached(on: boolean) {
  detached = on;
  // Sync with Rust foreground guard (prevents auto-hide on focus loss)
  invoke("set_detached", { detached: on }).catch(() => {});
  // OCR is detached-only (点10/17): no "<" back-to-embedded button, keep "X" only
  detachedBackBtn.classList.toggle("hidden", on && activePluginId === "ocr");
  if (on) {
    document.getElementById("app")!.classList.add("detached");
    statusHint.textContent = t("detached.click_restore");

    // Set detached header title
    const plugin = pluginRegistry.getAll().find(p => p.id === activePluginId);
    detachedTitle.textContent = plugin ? pluginName(plugin.id) : t("detached.title_fallback");

    // Show VSCode button only for AI Agent plugin
    detachedVscodeBtn.classList.toggle("hidden", activePluginId !== "ai-agent");
    if (activePluginId === "ai-agent") {
      detachedVscodeBtn.title = t("detached.vscode");
    }

    // Focus chat input in detached mode
    if (activePluginId === "ai-agent" && !isChatHistoryView) {
      setTimeout(() => chatInput.focus(), 100);
    }
  } else {
    document.getElementById("app")!.classList.remove("detached");
    statusHint.textContent = t("hint.esc_close");
    // Save unsent input to plugin state before hiding
    if (activePluginId) {
      const st = pluginStates.get(activePluginId);
      if (st) st.pendingInput = chatInput.value;
    }
  }
  applyWindowSize();
}

// Double-click search bar (plugin-locked) → enter detached mode
searchBar.addEventListener("dblclick", () => {
  if (!pluginActive || !searchInput.disabled) return;
  if (resultsList.children.length > 0) {
    setDetached(true);
  }
});

// Detached header: back button → restore search bar (exit detached mode).
// OCR is detached-only (点10/17): back goes straight back to the search bar
// instead of degrading to the embedded small pane (which left attached-file
// chips lingering above the title bar).
detachedBackBtn.addEventListener("click", () => {
  if (!detached) return;
  if (activePluginId === "ocr") {
    closePluginView();
  } else {
    setDetached(false);
  }
});

// Detached header: close button → exit plugin entirely
detachedCloseBtn.addEventListener("click", async () => {
  if (detached) setDetached(false);
  if (pluginActive) await closePluginView();
});

// Detached header: VSCode button → open/attach to VSCode
detachedVscodeBtn.addEventListener("click", () => {
  invoke("open_in_vscode").catch((e) => console.error("VSCode launch failed:", e));
});

// ── 印象派插件 icon（结果区）────────────────────────────────────
// 图案语言与按钮 icon 一致：扫笔残影（左上偏移淡副本）+ 主体 +
// 点彩高光（左上密大 / 右下疏淡，不对称），stroke 用 currentColor 保语义色。
// 替代原先各插件的 emoji icon，视觉风格与参考肖像图（水平+左斜笔触、高光点）统一。
const PLUGIN_ICON_PATHS: Record<string, string> = {
  "settings": `<path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/><circle cx="12" cy="12" r="3"/>`,
  "web-search": `<circle cx="12" cy="12" r="10"/><line x1="2" y1="12" x2="22" y2="12"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/>`,
  "ai-agent": `<path d="M12 8V4H8"/><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M15 13v2"/><path d="M9 13v2"/>`,
  "memo": `<path d="M15.5 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V8.5z"/><polyline points="15 3 15 9 21 9"/>`,
  "ocr": `<circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>`,
  "quick-launch": `<line x1="12" y1="17" x2="12" y2="22"/><path d="M5 17h14l-1.5-2h-11z"/><path d="M12 2a5 5 0 0 0-5 5c0 2.5 2 4 3.5 5.5L12 14l1.5-1.5C15 11 17 9.5 17 7a5 5 0 0 0-5-5z"/>`,
  "tool-editor": `<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>`,
  "clipboard-history": `<rect x="8" y="2" width="8" height="4" rx="1"/><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/>`,
};

/** 生成插件的印象派 SVG icon；未知插件回退 🔧（原 emoji 保底）。 */
function pluginIconSvg(id: string): string {
  const inner = PLUGIN_ICON_PATHS[id];
  if (!inner) return "🔧";
  return `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <g opacity="0.28" transform="translate(-0.7,-0.7)" stroke-width="1.4">${inner}</g>
    ${inner}
    <circle cx="7" cy="7" r="1" fill="currentColor" stroke="none"/>
    <circle cx="16.8" cy="16.8" r="0.5" fill="currentColor" stroke="none" opacity="0.45"/>
  </svg>`;
}

// ── AI entry + Web Search: always-visible results when search is empty ──
function renderAIEntry(): void {
  resultsContainer.classList.remove("hidden");
  searchBar.classList.add("has-results");
  currentResults = [];
  currentApps = [];
  selectedIndex = 0;

  const frag = document.createDocumentFragment();

  // Web Search — always visible alongside AI Agent
  const wsPlugin = pluginRegistry.getAll().find(p => p.id === "web-search");
  if (wsPlugin) {
    buildWebSearchItem(wsPlugin, searchInput.value.trim(), frag);
  }

  const aiItem = doc("div");
  aiItem.className = "result-item selected";
  aiItem.innerHTML = `
    <div class="result-item-icon">${pluginIconSvg("ai-agent")}</div>
    <div class="result-item-content">
      <div class="result-item-title">${t("chat.ai_entry")}</div>
      <div class="result-item-desc">${t("chat.ask_any", { mode: currentModeLabel() })}</div>
    </div>`;
  aiItem.addEventListener("click", () => {
    startAIChat(searchInput.value.trim());
  });
  frag.appendChild(aiItem);
  resultsList.replaceChildren(frag);
  statusText.textContent = t("status.ai", { mode: currentModeLabel() });
  applyWindowSize();
}

// ── OCR clipboard entry: inserted above AI entry when clipboard has image ──
function renderClipboardOCREntry(): void {
  const ocrPlugin = pluginRegistry.getAll().find(p => p.id === "ocr");
  if (!ocrPlugin) return;

  const ocrItem = doc("div");
  ocrItem.className = "result-item";
  ocrItem.innerHTML = `
    <div class="result-item-icon">${pluginIconSvg("ocr")}</div>
    <div class="result-item-content">
      <div class="result-item-title">${t("plugin.ocr")}</div>
      <div class="result-item-desc">${t("chat.attach_ocr_clipboard")} (PP-OCRv4)</div>
    </div>
    <span class="result-item-badge">OCR</span>
  `;
  ocrItem.addEventListener("click", () => {
    executePlugin(ocrPlugin);
  });

  // Insert before the first result item (AI entry)
  const first = resultsList.querySelector(".result-item");
  if (first) {
    resultsList.insertBefore(ocrItem, first);
  } else {
    resultsList.appendChild(ocrItem);
  }
  // Select the OCR entry by default (more relevant than AI)
  const items = resultsList.querySelectorAll(".result-item");
  items.forEach(item => item.classList.remove("selected"));
  ocrItem.classList.add("selected");
  selectedIndex = 0;
}

// ── Focus lost → hide; focus gained → 聚焦输入框 ─────────────────
// Debounce: Tauri onFocusChanged(false) may fire spuriously during
// WebView2 rendering. 150ms delay filters out transient focus flips.
// Detached mode is "pinned" — stays visible even when other apps are focused.
let hideTimer: ReturnType<typeof setTimeout> | null = null;
let blockingHide = false; // prevent onFocusChanged hide during programmatic hide from Rust
let _execGuard = false;    // prevent double-trigger of launchApp/executePlugin from rapid click+Enter

win.onFocusChanged(({ payload: focused }) => {
  if (focused) {
    if (hideTimer) { clearTimeout(hideTimer); hideTimer = null; }
    isVisible = true;
    // Focus the appropriate input: chatInput when AI agent is active
    // (searchInput is disabled then, focusing it locks the dialog)
    if (pluginActive && activePluginId === "ai-agent") {
      chatInput.focus();
    } else {
      searchInput.focus();
    }
    // On re-show (drag end / hotkey toggle): always show AI entry
    // when input is empty and no plugin is active — never hide it.
    if (!pluginActive && !searchInput.value.trim()) {
      renderAIEntry();
      // Also show OCR entry if clipboard has an image
      if ((window as any).__lunac_clipboard_has_image) {
        renderClipboardOCREntry();
      }
    }
  } else if (isVisible && !blockingHide && !detached && !pluginActive && !dragging) {
    // Clean up drawer/backdrop before hiding — prevents UI lock when
    // user opens history drawer then clicks away (drawerVisible stays
    // true and backdrop intercepts clicks on next focus).
    if (drawerVisible) {
      toggleDrawer(false);
    }
    // Only auto-hide in normal (non-detached, non-plugin, non-drag) mode.
    // Detached panel / plugin panel / mid-drag all stay visible.
    if (hideTimer) clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      hideTimer = null;
      if (!isVisible || detached || pluginActive || dragging || document.hasFocus()) return;
      win.hide().catch(() => {});
      isVisible = false;
    }, 150);
  }
});

// ── 热键兜底 + 录制：焦点在自己窗口时由 JS 直接处理 ────────────
// JS keydown 仅在 WebView2 有焦点时触发 → 录制天然"仅焦点时生效"。
// Rust 钩子命中时会吞键（WebView 收不到 keydown），因此兜底 handler
// 只在钩子未拦截的场景触发，二者互补、不会重复。
document.addEventListener("keydown", (e) => {
  // ── 热键录制模式（仅 Lunac 有焦点时生效）───────────────────────
  if ((window as any).__lunac_recording_active) {
    // Esc: cancel recording
    if (e.key === "Escape") {
      e.preventDefault(); e.stopPropagation();
      (window as any).__lunac_recording_active = false;
      window.dispatchEvent(new CustomEvent("lunac-recording-cancelled"));
      return;
    }
    // Only capture Alt + single non-modifier key
    if (!e.altKey) return;
    const key = e.key;
    if (["Alt", "Control", "Shift", "Meta", "Dead", "Process", "Unidentified"].includes(key)) return;
    // Skip injected dummy key (0xFF) from Rust send_dummy_key
    if (e.keyCode === 0xFF) return;

    e.preventDefault(); e.stopPropagation();

    // Build combo string in canonical format (Alt+Ctrl+Shift+Win+Key)
    const parts: string[] = ["Alt"];
    if (e.ctrlKey) parts.push("Ctrl");
    if (e.shiftKey) parts.push("Shift");
    if (e.metaKey) parts.push("Win");
    // Map key names to canonical form: " " → "Space", single-char → uppercase
    const canonicalKey = key === " " ? "Space" : (key.length === 1 ? key.toUpperCase() : key);
    parts.push(canonicalKey);
    const combo = parts.join("+");

    console.log("[lunac] RECORDING captured:", combo, "raw key:", JSON.stringify(key), "code:", e.code, "keyCode:", e.keyCode);

    (window as any).__lunac_recording_active = false;
    window.dispatchEvent(new CustomEvent("lunac-recording-captured", { detail: { combo } }));
    return;
  }

  // ── Alt+Space 兜底（仅当当前已配置热键为 Alt+Space 时生效）──────
  // 自定义热键后 Alt+Space 彻底失效，此条件防止双热键并存。
  if ((window as any).__lunac_hotkey_is_alt_space && e.altKey && (e.code === "Space" || e.key === " ")) {
    e.preventDefault(); e.stopPropagation();
    win.hide().catch(() => {});
    isVisible = false;
  }
}, { capture: true });

// ── 禁用默认右键菜单 ────────────────────────────────────────────
// WebView2 / Rust 默认右键菜单包含"后退/前进/重新加载/检查元素"等，
// Lunac 作为桌面工具不应暴露这些浏览器行为。
document.addEventListener("contextmenu", (e) => {
  e.preventDefault();
});

// ── Esc 逐级清除逻辑（泡泡 → 文本 → Rust hide） ──────────────
// Esc 由 Rust LL 钩子统一截获 → emit lunac-esc-clear，
// 前端仅通过 listen 事件响应，不做 JS keydown 后备
// （后备会导致 JS 先清空→Rust 后判空→直接隐藏的双击问题）。

/** 统一的 Esc 逐级清除逻辑（抽屉 → 泡泡 → 文本 → 退出/隐藏） */
function handleEscClear() {
  // Drawer visible → close it first, don't touch plugin state
  if (drawerVisible) {
    toggleDrawer(false);
    return;
  }
  if (pluginActive) {
    // OCR is detached-only (点10/17): Esc exits straight back to the search
    // bar. The old step-by-step chip clearing left the panel as an embedded
    // small pane with attached files still shown above the title bar.
    if (activePluginId === "ocr") {
      closePluginView();
      return;
    }
    // AI chat mode: chips first → text → close plugin
    if (attachedFiles.length > 0) {
      removeFileChip(attachedFiles.length - 1);
    } else if (chatInput.value.trim()) {
      chatInput.value = "";
      autoResizeChatTextarea();
    } else {
      closePluginView();
    }
    return;
  }
  // Main search: chips → text → show AI entry
  if (attachedFiles.length > 0) {
    removeFileChip(attachedFiles.length - 1);
    statusText.textContent = attachedFiles.length > 0 ? t("chat.file_count", { count: String(attachedFiles.length) }) : t("status.ready");
  } else if (searchInput.value.trim()) {
    searchInput.value = "";
    autoResizeTextarea();
    invoke("set_query_state", { empty: true }).catch(() => {});
    // Dispatch input event so the AI entry renders (per "AI入口常驻" behavior)
    searchInput.dispatchEvent(new Event("input", { bubbles: true }));
  }
  // 无泡泡、无文字 → Rust 端 hide_window()
}

// ── 空白状态同步到 Rust（Esc 时 Rust 独立判断空白→隐藏） ────────
function syncEmpty() {
  invoke("set_query_state", { empty: !searchInput.value.trim() });
}

searchInput.addEventListener("input", syncEmpty);

// ── Quick Launch: remove an item from the panel ─────────────────
// Fired by quick-launch.ts remove buttons. 语义（2026-09）：
//   1. 该路径若是当前会话气泡 → 先摘除气泡
//   2. 该路径已在自定义注册表 → remove_custom_app 注销（数据从 ModuleData 删除）
//   3. 面板若正打开 → 实时刷新列表（数据不再“消失”）
window.addEventListener("lunac:remove-attached-file", (e) => {
  const { path } = ((e as CustomEvent).detail || {}) as { path?: string };
  if (!path) return;
  const idx = attachedFiles.indexOf(path);
  if (idx >= 0) removeFileChip(idx);
  invoke("remove_custom_app", { path }).catch(() => {});
  refreshQuickLaunchPanel();
});

// Fired by quick-launch.ts “添加启动项”按钮（写入注册表后）→ 刷新面板
window.addEventListener("lunac:quicklaunch-changed", () => {
  refreshQuickLaunchPanel();
});

/** 若快速启动面板正打开则重跑插件并重新绑定（数据增删后保持列表最新） */
async function refreshQuickLaunchPanel() {
  if (!(pluginActive && activePluginId === "quick-launch")) return;
  const ql = pluginRegistry.getAll().find(p => p.id === "quick-launch");
  if (!ql) return;
  try {
    const r = await ql.execute(searchInput.value.trim());
    resultsList.innerHTML = `<div class="plugin-result">${r.content}</div>`;
    const m = await import("./plugins/builtin/quick-launch");
    m.attachQuickLaunchListeners(resultsList);
  } catch {
    // re-render failed — leave the current panel as-is
  }
}

// ── Esc：全部由 Rust 端统一处理（钩子 + 轮询兜底）───────────────
// Rust 通过 emit 事件通知前端：
// 1. "lunac-esc-clear"  — 有内容 → 清空输入  /  插件面板 → 关闭面板
// 2. "lunac-esc-cancel-rec" — 录制中 → 取消录制
// 3. 空白且无插件 → Rust 直接 hide_window()，不经过前端

listen("lunac-esc-clear", async () => {
  handleEscClear();
});

async function closePluginView() {
  const pid = activePluginId;
  if (pid) {
    // Persist AI conversation before closing — previously only "new
    // conversation" / session switches saved, so closing the panel (Esc/×)
    // silently dropped the current session from history.
    if (pid === "ai-agent") {
      await saveCurrentSession().catch(() => {});
    }
    // Save plugin state before clearing
    pluginStates.set(pid, {
      id: pid,
      html: resultsList.innerHTML,
      searchQuery: searchInput.value,
      pendingInput: chatInput.value,
    });
  }
  // Cancel any active streaming (increment id so old callbacks are ignored)
  if (isStreaming) {
    streamId++;
    cliTextCallback = null;
    cliDoneCallback = null;
    isStreaming = false;
    setStreamingUI(false);
  }
  pendingMessages = [];
  consecutiveFailures = 0;
  pluginActive = false;
  activePluginId = null;
  isChatHistoryView = false;
  // Clean up agent view (session cleanup — ai-spec §17.5)
  agentView = null;
  agentTurn = null;
  currentSessionId = null;
  // Clean up drawer if still present
  closeDrawer();
  // Await Rust state sync — prevents race: next ESC sees correct PLUGIN_ACTIVE=false
  await invoke("set_plugin_active", { active: false }).catch(() => {});
  // Stop CLI (free resources on exit)
  invoke("stop_cli").catch(() => {});
  cliReady = false;
  agentView = null;
  agentTurn = null;
  clearPermissionCards();
  setPluginBar(null);
  setDetached(false);
  resultsContainer.classList.add("hidden");
  resultsContainer.classList.remove("plugin-open");
  searchBar.classList.remove("has-results");
  resultsList.innerHTML = "";
  currentResults = [];
  selectedIndex = 0;
  humanizeBtn.style.display = "none";
  // Restore search input from plugin state
  const st = pid ? pluginStates.get(pid) : undefined;
  searchInput.value = st?.searchQuery || "";
  statusText.textContent = t("status.ready");
  searchInput.focus();
  syncEmpty();
  // Re-trigger search with restored query, or show AI entry
  if (searchInput.value) {
    searchInput.dispatchEvent(new Event("input"));
  } else {
    renderAIEntry();
  }
}

listen("lunac-esc-cancel-rec", () => {
  (window as any).__lunac_cancel_recording?.();
});

// Rust fallback: Esc pressed during hotkey recording while WebView has no
// focus (Rust LL hook emits "lunac-esc-cancel-rec"). Previously this global
// was never defined, so the fallback silently did nothing.
(window as any).__lunac_cancel_recording = () => {
  if ((window as any).__lunac_recording_active) {
    (window as any).__lunac_recording_active = false;
    window.dispatchEvent(new CustomEvent("lunac-recording-cancelled"));
  }
};

// ── Agent CLI streaming listeners ──────────────────────────────
interface CliEventLine {
  type: string;
  subtype?: string;
  attempt?: number;
  max_retries?: number;
  error_status?: number;
  result?: string;
  request_id?: string;
  request?: {
    subtype?: string;
    tool_name?: string;
    input?: Record<string, unknown>;
    tool_use_id?: string;
  };
  event?: {
    type: string;
    delta?: { type: string; text?: string; thinking?: string; partial_json?: string };
    content_block?: { type: string; name?: string };
  };
  message?: { content?: Array<{ type: string; text?: string; name?: string; thinking?: string; content?: unknown; is_error?: boolean }> };
  usage?: { input_tokens: number; output_tokens: number };
}

// ── Agent state machine (Pi reference: turn lifecycle) ────────────
// States follow Pi's agent runtime model:
//   idle → starting → running ↔ approval → done → idle
// Each state drives UI visibility + status bar text.

type AgentState = "idle" | "starting" | "running" | "approval" | "done";

let agentState: AgentState = "idle";

/** Transition the agent state machine; reject invalid transitions. */
function agentTransition(next: AgentState): boolean {
  const valid: Record<AgentState, AgentState[]> = {
    idle:     ["starting"],
    starting: ["running", "idle"],
    running:  ["approval", "done", "idle"],
    approval: ["running", "done", "idle"],
    done:     ["idle"],
  };
  if (!valid[agentState].includes(next)) {
    console.warn(`[agent] invalid transition: ${agentState} → ${next}`);
    return false;
  }
  agentState = next;
  return true;
}

/** Reset agent to idle — cleanup all turn/view tracking */
function agentReset() {
  agentState = "idle";
  agentTurn = null;
  agentView = null;
  clearPermissionCards();
}

// ── Agent turn model (Pi reference: structured turn lifecycle) ────
// Each user query → result is one turn. Tool calls within a turn
// are recorded for observability and session replay.

interface AgentToolCall {
  name: string;
  status: "pending" | "running" | "success" | "error";
  result?: string;
}

interface AgentTurn {
  id: string;           // unique turn ID (timestamp-based)
  startTime: number;
  status: "running" | "done";
  toolCalls: AgentToolCall[];
  inputTokens?: number;
  outputTokens?: number;
  cacheReadTokens?: number;
}

let agentTurn: AgentTurn | null = null;

// ── Agent ordered-block rendering ────────────────────────────────
// The agent output is a SEQUENCE of blocks (thinking / text / tool use /
// approval cards) rendered in arrival order — not one big text blob with
// cards stuck at the bottom.
interface AgentView {
  turn: AgentTurn;                       // turn this view belongs to
  flow: HTMLElement;                    // ordered block container
  current: HTMLElement | null;          // current streaming content element
  currentKind: "thinking" | "text" | "tool" | null;
  curDetails: HTMLDetailsElement | null;
  curToolName: string;                  // current tool for readable arg display
  curToolArgs: string;                  // accumulated input_json_delta
  thinkChars: number;
  textAll: string;                      // concatenated text blocks (history)
}
let agentView: AgentView | null = null;

function agentScroll() {
  autoScrollIfNearBottom();
}

function agentNewBlock(kind: "thinking" | "text" | "tool", toolName?: string) {
  const v = agentView;
  if (!v) return;
  agentCloseBlock();
  v.flow.querySelector(".plugin-result-loading")?.remove();

  if (kind === "thinking") {
    const det = document.createElement("details");
    det.className = "think-block";
    det.innerHTML = `<summary>💭 ${t("agent.thinking_toggle")}</summary><div class="think-content"></div>`;
    v.flow.appendChild(det);
    v.curDetails = det;
    v.thinkChars = 0;
    v.current = det.querySelector(".think-content");
  } else if (kind === "tool") {
    const row = document.createElement("div");
    row.className = "tool-row";
    row.innerHTML = `<span class="tool-name">🔧 ${esc(toolName || t("chat.badge_tool"))}</span> <span class="tool-args"></span>`;
    v.flow.appendChild(row);
    v.current = row.querySelector(".tool-args");
    v.curToolName = toolName || "";
    v.curToolArgs = "";
  } else {
    const p = document.createElement("div");
    p.className = "agent-text";
    v.flow.appendChild(p);
    v.current = p;
  }
  v.currentKind = kind;
  agentScroll();
}

function agentAppend(kind: "thinking" | "text", s: string) {
  const v = agentView;
  if (!v || !s) return;
  if (v.currentKind !== kind || !v.current) agentNewBlock(kind);
  if (kind === "thinking") {
    v.thinkChars += s.length;
    const sum = v.curDetails?.querySelector("summary");
    if (sum) sum.textContent = `💭 思考中… (${v.thinkChars} 字)`;
  } else {
    v.textAll += s;
  }
  v.current!.textContent = (v.current!.textContent || "") + s;
  agentScroll();
}

function agentToolArgsDelta(s: string) {
  const v = agentView;
  if (!v || v.currentKind !== "tool" || !v.current || !s) return;
  v.curToolArgs += s;
  // Show a readable summary instead of raw JSON (Trae-style): the command
  // text for Bash/PowerShell, key=value fields for other tools.
  let display = v.curToolArgs;
  try {
    const obj = JSON.parse(v.curToolArgs);
    if (typeof obj === "object" && obj !== null && !Array.isArray(obj)) {
      if (v.curToolName === "Bash" || v.curToolName === "PowerShell") {
        display = typeof (obj as any).command === "string"
          ? ((obj as any).command as string)
          : v.curToolArgs;
      } else {
        display = Object.entries(obj)
          .map(([k, val]) =>
            `${k}=${typeof val === "string" ? (val as string) : JSON.stringify(val)}`
          )
          .join("  ");
      }
    }
  } catch {
    // partial JSON while streaming — keep raw accumulation
  }
  v.current.textContent = display.length > 200 ? display.slice(0, 200) + "…" : display;
}

function agentCloseBlock() {
  const v = agentView;
  if (!v) return;
  if (v.currentKind === "thinking" && v.curDetails) {
    // Collapse finished thinking into a small expandable header (Hermes-style)
    const sum = v.curDetails.querySelector("summary");
    if (sum) sum.textContent = `💭 思考过程 (${v.thinkChars} 字)`;
    v.curDetails.open = false;
    v.curDetails = null;
  }
  if (v.currentKind === "text") {
    v.textAll += "\n\n";
    // Render markdown images in the finished text block (点16)
    const tblock = v.current as HTMLElement;
    if (tblock) {
      tblock.innerHTML = mdImagesToHtml(tblock.textContent || "");
    }
  }
  v.current = null;
  v.currentKind = null;
}

/** Extract readable text from a tool_result content (string or block array). */
function extractToolResultText(c: unknown): string {
  if (typeof c === "string") return c;
  if (Array.isArray(c)) {
    return c
      .map((b) => (typeof b === "string" ? b : (b as { text?: string })?.text || ""))
      .join(" ");
  }
  return "";
}

/** Render a tool execution outcome inline — otherwise a failed tool looks
 *  like a silent hang (only the tool row appears, then nothing). */
function agentToolResult(isError: boolean, content: unknown) {
  const host = agentView?.flow ?? resultsList;
  const txt = extractToolResultText(content).trim();

  if (isError) {
    const row = document.createElement("div");
    row.className = "tool-row tool-error";
    row.textContent = `${t("agent.tool_failed", { txt: txt.slice(0, 200).replace(/\s+/g, " ") || "unknown error" })}`;
    // Track consecutive failures for 3-fix-failure warning
    consecutiveFailures++;
    if (consecutiveFailures >= 3) {
      const warn = document.createElement("div");
      warn.className = "tool-row tool-warn";
      warn.innerHTML = `<strong>${t("agent.warn_tool_failures")}</strong>`;
      host.appendChild(warn);
      consecutiveFailures = 0; // Reset after warning to avoid spam
    }
    host.appendChild(row);
  } else {
    // Reset counter on success — the fix chain resolved
    consecutiveFailures = 0;
    // Collapsible result (Trae-style): one-line summary, full output on click
    const det = document.createElement("details");
    det.className = "tool-row tool-ok";
    const oneLine = txt.replace(/\s+/g, " ").trim();
    det.innerHTML = `<summary>✓ 完成${oneLine ? ": " + esc(oneLine.slice(0, 120)) : ""}</summary>${txt ? `<div class="tool-result">${esc(txt.slice(0, 600))}</div>` : ""}`;
    host.appendChild(det);
  }
  agentScroll();
}

// ── Agent permission dialogs (can_use_tool control protocol) ────
// When a tool needs approval, agent.exe emits a control_request on stdout
// and BLOCKS until a control_response arrives on stdin. Without this UI
// the agent would hang forever on any gated tool (e.g. Bash commands).
const pendingPermissionCards = new Map<string, HTMLElement>();

// Built-in safe command prefixes — auto-approved (read-only, no side effects)
const BUILTIN_SAFE_PREFIXES = [
  "ls", "dir", "cat", "type", "echo", "pwd", "head", "tail", "wc",
  "git status", "git log", "git diff", "git branch", "git show",
  "which", "where", "whoami", "date", "rg", "grep", "find",
  "node -v", "npm -v", "python --version", "Get-ChildItem", "Get-Content",
];

// Hard blacklist — destructive patterns; NEVER auto-approved, no
// "always allow" offered, card shows a danger warning.
const CMD_BLACKLIST: { re: RegExp; label: string }[] = [
  { re: /\brm\s+-[a-z]*r[a-z]*f|\brm\s+-[a-z]*f[a-z]*r/i, label: "递归强制删除" },
  { re: /\bformat\s+[a-z]:/i, label: "格式化磁盘" },
  { re: /\bdel\s+\/[fsq]/i, label: "强制删除" },
  { re: /\brmdir\s+\/s/i, label: "递归删除目录" },
  { re: /\bRemove-Item\b[^\n]*-Recurse[^\n]*-Force/i, label: "递归强制删除" },
  { re: /\bdiskpart\b/i, label: "磁盘分区操作" },
  { re: /\bmkfs\b/i, label: "创建文件系统" },
  { re: /\bdd\s+if=/i, label: "底层磁盘写入" },
  { re: /\breg\s+(delete|add)\b/i, label: "注册表修改" },
  { re: /\bshutdown\b|\breboot\b/i, label: "关机/重启" },
  { re: /\bgit\s+push\b[^\n]*--force/i, label: "强制推送" },
  { re: />\s*\/dev\/(sd|nvme|hd)/i, label: "覆写块设备" },
  { re: /\btaskkill\b[^\n]*\/f/i, label: "强制结束进程" },
];

interface ApproveWhitelist { tools: string[]; bash: string[] }

function getUserWhitelist(): ApproveWhitelist {
  try {
    const parsed = JSON.parse(localStorage.getItem("lunac-approve-whitelist") || "");
    return { tools: parsed.tools ?? [], bash: parsed.bash ?? [] };
  } catch {
    return { tools: [], bash: [] };
  }
}

function saveUserWhitelist(wl: ApproveWhitelist) {
  localStorage.setItem("lunac-approve-whitelist", JSON.stringify(wl));
}

/** Classify a permission request: auto-allow (safe/whitelisted), danger
 *  (blacklisted — manual only), or normal (3-button card). */
function classifyRequest(toolName: string, input: unknown): {
  auto: boolean; danger: string | null; bashCmd: string | null;
} {
  const inp = input as Record<string, unknown> | undefined;
  const bashCmd =
    (toolName === "Bash" || toolName === "PowerShell") && typeof inp?.command === "string"
      ? (inp.command as string)
      : null;

  if (bashCmd) {
    for (const b of CMD_BLACKLIST) {
      if (b.re.test(bashCmd)) return { auto: false, danger: b.label, bashCmd };
    }
    const cmd = bashCmd.trim();
    const wl = getUserWhitelist();
    const hit = (p: string) => cmd === p || cmd.startsWith(p + " ");
    if (BUILTIN_SAFE_PREFIXES.some(hit) || wl.bash.some(hit)) {
      return { auto: true, danger: null, bashCmd };
    }
    return { auto: false, danger: null, bashCmd };
  }

  const wl = getUserWhitelist();
  return { auto: wl.tools.includes(toolName), danger: null, bashCmd: null };
}

function respondPermission(requestId: string, allow: boolean, toolUseId?: string) {
  // allow with empty updatedInput = "run with the original input"
  // (explicitly supported: CLI treats {} as use-original)
  const inner = allow
    ? { behavior: "allow", updatedInput: {}, toolUseID: toolUseId }
    : { behavior: "deny", message: "User denied this action in Lunac", interrupt: false, toolUseID: toolUseId };
  const msg = JSON.stringify({
    type: "control_response",
    response: { subtype: "success", request_id: requestId, response: inner },
  });
  invoke("send_message", { message: msg }).catch(() => {});
}

// ── Continuous-command merging (Trae-style approval) ──────────────
// Consecutive simple Bash/PowerShell commands in the same turn fold into
// ONE approval row → one permission window, one allow/deny decision for
// all of them. The merge is approval/presentation only: the CLI still
// runs each command individually, so execution semantics are never
// altered (a failed `&&` short-circuit, per-command output, etc.).
const MAX_CMD_GROUP = 8;

/** A command may join a merge group only if it is a simple single command —
 *  shell separators/pipes/redirects/backgrounding/newlines would change
 *  meaning, so those stay as their own approval row. */
function isMergeableCommand(cmd: string): boolean {
  if (!cmd) return false;
  if (cmd.length > 200) return false;
  if (/[|;&<>`\n]/.test(cmd)) return false;
  return true;
}

interface CmdGroupItem extends HTMLElement {
  _groupIds: string[];
  _groupCmds: string[];
  _groupToolUseIds: string[];
  _groupToolName: string;
  _groupDanger: boolean;
  _groupInput?: unknown;
  _isCmdGroup: boolean;
  _finish: (allow: boolean, always?: boolean) => void;
}

/** Find the last open command-group row for a tool (merge target). */
function findLastBashGroup(toolName: string): CmdGroupItem | null {
  let last: CmdGroupItem | null = null;
  for (const [, item] of pendingPermissionCards) {
    const it = item as CmdGroupItem;
    if (it._isCmdGroup && it._groupToolName === toolName && !it.classList.contains("answered")) {
      last = it;
    }
  }
  return last;
}

/** Re-render a command group's body (command list + merge count). */
function renderCmdGroupBody(item: CmdGroupItem) {
  const bodyEl = item.querySelector(".approval-body") as HTMLElement | null;
  if (!bodyEl) return;
  if (item._isCmdGroup) {
    const cmds = item._groupCmds;
    const html = cmds
      .map((c, i) => {
        const sep = i > 0 ? `<span class="approval-cmd-sep">└ </span>` : "";
        return `<div class="approval-cmd" title="${esc(c)}">${sep}<span class="approval-cmd-text">${esc(c)}</span></div>`;
      })
      .join("");
    const count = cmds.length > 1
      ? `<div class="approval-cmd-merged">${t("agent.cmd_merged", { count: String(cmds.length) })}</div>`
      : "";
    bodyEl.innerHTML = `
      <div class="approval-cmd-box">
        <button class="approval-copy-btn" title="${esc(t("agent.copy_cmd"))}">${esc(t("agent.copy_cmd"))}</button>
        <div class="approval-cmd-list">${html}</div>
        ${count}
      </div>`;
    // Copy button: copies the full (merged) command text, shows feedback
    const copyBtn = bodyEl.querySelector(".approval-copy-btn");
    copyBtn?.addEventListener("click", () => {
      navigator.clipboard.writeText(cmds.join("\n")).catch(() => {});
      const label = copyBtn as HTMLElement;
      label.textContent = t("agent.copied");
      setTimeout(() => {
        label.textContent = t("agent.copy_cmd");
      }, 1200);
    });
  } else {
    let preview = "";
    try {
      const s = JSON.stringify(item._groupInput ?? {}, null, 0);
      preview = s.length > 180 ? s.slice(0, 180) + "…" : s;
    } catch {
      preview = String(item._groupInput ?? "");
    }
    bodyEl.innerHTML = `<div class="approval-input compact" title="${esc(preview)}">${esc(preview)}</div>`;
  }
}

// ── Permission approval — single consolidated card ─────────────────
// Multiple can_use_tool requests from one turn are aggregated into ONE
// card (a row per request + batch actions). The CLI blocks until every
// pending request is answered, so the conversation is paused while the
// card is open (status keeps showing 等待权限确认).
let permissionBatchCard: HTMLElement | null = null;

function isPermissionPending(): boolean {
  return pendingPermissionCards.size > 0;
}

function updatePermissionHeader() {
  if (!permissionBatchCard) return;
  const countEl = permissionBatchCard.querySelector(".approval-batch-count");
  if (countEl) {
    countEl.textContent = t("agent.permission_count", { count: String(pendingPermissionCards.size) });
  }
  statusText.textContent = t("agent.permission_wait");
}

function showPermissionCard(
  requestId: string,
  toolName: string,
  input: unknown,
  toolUseId?: string,
) {
  pendingPermissionCards.get(requestId)?.remove();
  const host = agentView?.flow ?? resultsList; // inline, in arrival order
  const cls = classifyRequest(toolName, input);

  // Whitelisted / built-in safe → auto-approve, show a one-line notice
  if (cls.auto) {
    respondPermission(requestId, true, toolUseId);
    const row = document.createElement("div");
    row.className = "tool-row auto-approved";
    row.textContent = `✓ 自动允许: ${toolName}${cls.bashCmd ? " · " + cls.bashCmd.slice(0, 80) : ""}`;
    host.appendChild(row);
    agentScroll();
    return;
  }

  // ── Build / reuse the single batch card ──
  if (!permissionBatchCard || !permissionBatchCard.isConnected) {
    const card = document.createElement("div");
    card.className = "approval-card approval-batch-card";
    card.innerHTML = `
      <div class="approval-batch-header">权限请求 <span class="approval-batch-count"></span></div>
      <div class="approval-batch-actions">
        <button class="approval-btn approval-batch-allow-all">${t("agent.allow_all")}</button>
        <button class="approval-btn approval-batch-deny-all">${t("agent.deny_all")}</button>
      </div>
      <div class="approval-batch-body"></div>`;
    host.appendChild(card);
    permissionBatchCard = card;
    card.querySelector(".approval-batch-allow-all")!.addEventListener("click", () => {
      const seen = new Set<HTMLElement>();
      for (const [, item] of [...pendingPermissionCards]) {
        if (seen.has(item)) continue; // merged group: multiple ids → one row
        seen.add(item);
        (item as any)._finish?.(true, false);
      }
    });
    card.querySelector(".approval-batch-deny-all")!.addEventListener("click", () => {
      const seen = new Set<HTMLElement>();
      for (const [, item] of [...pendingPermissionCards]) {
        if (seen.has(item)) continue;
        seen.add(item);
        (item as any)._finish?.(false, false);
      }
    });
  }
  const body = permissionBatchCard.querySelector(".approval-batch-body")!;

  // ── Continuous-command merge: fold a simple follow-up command into the
  // last open Bash/PowerShell approval row instead of a new one ─────────
  if (cls.bashCmd && !cls.auto && isMergeableCommand(cls.bashCmd)) {
    const last = findLastBashGroup(toolName);
    if (last && last._groupIds.length < MAX_CMD_GROUP) {
      last._groupIds.push(requestId);
      last._groupCmds.push(cls.bashCmd);
      last._groupToolUseIds.push(toolUseId ?? "");
      last._groupDanger = last._groupDanger || !!cls.danger;
      renderCmdGroupBody(last);
      pendingPermissionCards.set(requestId, last);
      updatePermissionHeader();
      agentScroll();
      return;
    }
  }

  // ── Build the per-item row ──
  const item = document.createElement("div") as unknown as CmdGroupItem;
  item.className = "approval-item" + (cls.danger ? " danger" : "");
  item._groupIds = [requestId];
  item._groupCmds = cls.bashCmd ? [cls.bashCmd] : [];
  item._groupToolUseIds = [toolUseId ?? ""];
  item._groupToolName = toolName;
  item._groupDanger = !!cls.danger;
  item._groupInput = input;
  item._isCmdGroup = cls.bashCmd !== null;

  const dangerHtml = cls.danger
    ? `<span class="approval-danger-inline" title="⛔ 危险操作（${esc(cls.danger)}）— 不可加入白名单，请谨慎确认">⛔</span>`
    : "";
  // Blacklisted commands never get an "always allow" button
  const alwaysBtn = cls.danger
    ? ""
    : `<button class="approval-btn approval-always">始终允许</button>`;
  item.innerHTML = `
    <div class="approval-title">${dangerHtml}<b>${esc(toolName)}</b></div>
    <div class="approval-body"></div>
    <div class="approval-actions">
      <button class="approval-btn approval-allow">允许</button>
      ${alwaysBtn}
      <button class="approval-btn approval-deny">拒绝</button>
    </div>`;
  body.appendChild(item);
  renderCmdGroupBody(item);
  agentScroll();

  const finish = (allow: boolean, always = false) => {
    const ids = item._groupIds;
    // Always-allow whitelists the first command's prefix or the tool name
    if (always && !item._groupDanger) {
      const wl = getUserWhitelist();
      if (item._isCmdGroup && item._groupCmds.length) {
        const prefix = item._groupCmds[0].trim().split(/\s+/)[0];
        if (prefix && !wl.bash.includes(prefix)) wl.bash.push(prefix);
      } else if (!wl.tools.includes(toolName)) {
        wl.tools.push(toolName);
      }
      saveUserWhitelist(wl);
    }
    ids.forEach((rid, i) => {
      respondPermission(rid, allow, item._groupToolUseIds[i] || undefined);
      pendingPermissionCards.delete(rid);
    });
    item.querySelector(".approval-actions")?.remove();
    const note = document.createElement("div");
    note.className = `approval-note ${allow ? "ok" : "no"}`;
    note.textContent = allow
      ? always
        ? t("agent.approved_whitelist")
        : ids.length > 1
          ? t("agent.cmd_allowed", { count: String(ids.length) })
          : t("agent.approved")
      : ids.length > 1
        ? t("agent.cmd_denied", { count: String(ids.length) })
        : t("agent.denied");
    item.appendChild(note);
    item.classList.add("answered");
    // Slide the resolved row out, then finalize the batch card
    setTimeout(() => {
      item.remove();
      if (pendingPermissionCards.size === 0) {
        clearPermissionCards();
      } else {
        updatePermissionHeader();
      }
    }, 350);
  };
  (item as any)._finish = finish;
  item.querySelector(".approval-allow")!.addEventListener("click", () => finish(true));
  item.querySelector(".approval-always")?.addEventListener("click", () => finish(true, true));
  item.querySelector(".approval-deny")!.addEventListener("click", () => finish(false));

  pendingPermissionCards.set(requestId, item);
  updatePermissionHeader();
}

function removePermissionCard(requestId: string) {
  const row = pendingPermissionCards.get(requestId);
  if (row) {
    row.remove();
    pendingPermissionCards.delete(requestId);
  }
  if (pendingPermissionCards.size === 0) {
    clearPermissionCards();
  } else {
    updatePermissionHeader();
  }
}

function clearPermissionCards() {
  if (permissionBatchCard) {
    permissionBatchCard.remove();
    permissionBatchCard = null;
  }
  pendingPermissionCards.clear();
}

listen<{ line: string }>("cli-output", (event) => {
  try {
    const data: CliEventLine = JSON.parse(event.payload.line);

    // Stream events (--include-partial-messages): ordered block rendering
    if (data.type === "stream_event") {
      // First content block = agent is actively working → running
      if (agentState === "starting") agentTransition("running");
      const ev = data.event;
      if (ev?.type === "content_block_start") {
        const cb = ev.content_block;
        if (cb?.type === "thinking") {
          agentNewBlock("thinking");
          statusText.textContent = t("agent.thinking");
        } else if (cb?.type === "tool_use") {
          // Track tool call in turn (Pi: structured tool lifecycle)
          agentTurn?.toolCalls.push({ name: cb.name || "unknown", status: "running" });
          agentNewBlock("tool", cb.name);
          statusText.textContent = t("agent.tool", { tool: cb.name || "..." });
        } else {
          agentNewBlock("text");
          statusText.textContent = t("agent.generating");
        }
      } else if (ev?.type === "content_block_delta") {
        if (ev.delta?.type === "text_delta" && ev.delta.text) {
          cliSawStreamDelta = true;
          agentAppend("text", ev.delta.text);
        } else if (ev.delta?.type === "thinking_delta" && ev.delta.thinking) {
          cliSawStreamDelta = true;
          agentAppend("thinking", ev.delta.thinking);
        } else if (ev.delta?.type === "input_json_delta" && ev.delta.partial_json) {
          agentToolArgsDelta(ev.delta.partial_json);
        }
      } else if (ev?.type === "content_block_stop") {
        agentCloseBlock();
      } else if (ev?.type === "message_stop") {
        // One assistant turn finished — the agent may continue with tool
        // calls. `result` is the true end-of-query marker.
        agentCloseBlock();
        statusText.textContent = t("agent.working");
      }
    }
    // System init — CLI finished loading plugins and scanning dir.
    // This arrives per-query-turn in stream-json mode. Status display
    // only; cliReady is set by cli-status("stdout") which arrives first
    // (stdin pipe is open, CLI buffers messages until fully initialized).
    else if (data.type === "system" && data.subtype === "init") {
      if (agentState === "starting") {
        agentTransition("idle");
        updateAgentStatus("ready");
        statusText.textContent = t("status.ai", { mode: currentModeLabel() });
      }
    }
    // API retry — surface silent backoff loops (e.g. auth/network failures)
    else if (data.type === "system" && data.subtype === "api_retry") {
    statusText.textContent = t("agent.retry", { attempt: String(data.attempt ?? "?"), max: String(data.max_retries ?? "?") });
    }
    // Permission request — CLI blocks until we answer: render approval card
    else if (data.type === "control_request" && data.request?.subtype === "can_use_tool" && data.request_id) {
      agentTransition("approval");
      showPermissionCard(
        data.request_id,
        data.request.tool_name || "unknown tool",
        data.request.input,
        data.request.tool_use_id,
      );
    }
    // CLI cancelled a pending request (hook decided first / query aborted)
    else if (data.type === "control_cancel_request" && data.request_id) {
      removePermissionCard(data.request_id);
      if (agentState === "approval") agentTransition("running");
    }
    // Subagent / task progress — surface instead of silent waiting
    else if (data.type === "system" && (data.subtype === "task_started" || data.subtype === "task_progress")) {
    statusText.textContent = t("agent.subtask");
    }
    // Whole assistant message: block fallback (only when no partial stream
    // events arrived, to avoid double rendering) + tool_use status
    else if (data.type === "assistant" && data.message?.content) {
      for (const block of data.message.content) {
        if (block.type === "thinking" && block.thinking && !cliSawStreamDelta) {
          agentNewBlock("thinking");
          agentAppend("thinking", block.thinking);
          agentCloseBlock();
        } else if (block.type === "text" && block.text && !cliSawStreamDelta) {
          agentNewBlock("text");
          agentAppend("text", block.text);
          agentCloseBlock();
        } else if (block.type === "tool_use") {
          statusText.textContent = `Agent tool: ${block.name || "..."}`;
          if (!cliSawStreamDelta) {
            agentNewBlock("tool", block.name);
            agentCloseBlock();
          }
        }
      }
    }
    // Tool execution results — surface success/failure inline
    else if (data.type === "user" && data.message?.content) {
      for (const block of data.message.content) {
        if (block.type === "tool_result") {
          agentToolResult(!!block.is_error, block.content);
          // Update turn tool call status (Pi: structured tool lifecycle)
          const tc = agentTurn?.toolCalls;
          const lastTool = tc && tc[tc.length - 1];
          if (lastTool && lastTool.status === "running") {
            lastTool.status = block.is_error ? "error" : "success";
            lastTool.result = extractToolResultText(block.content).slice(0, 200);
          }
        }
      }
    }
    // Result — true end of one query (success or error): finish the stream
    else if (data.type === "result") {
      const u = data.usage as Record<string, unknown> | undefined;
      // CLI reports cumulative totals (this.totalUsage); frontend computes
      // deltas against the last saved value to avoid double-counting.
      const info: ChatDoneInfo | undefined = u && typeof u.input_tokens === "number"
        ? {
            stop_reason: "end_turn",
            input_tokens: u.input_tokens as number,
            output_tokens: (u.output_tokens as number) ?? 0,
            cache_read_input_tokens: (u.cache_read_input_tokens as number) ?? 0,
            cache_creation_input_tokens: (u.cache_creation_input_tokens as number) ?? 0,
          }
        : undefined;
      // Finalize turn (Pi: turn_end)
      if (agentTurn) {
        agentTurn.status = "done";
        agentTurn.inputTokens = data.usage?.input_tokens;
        agentTurn.outputTokens = data.usage?.output_tokens;
      }
      if (data.subtype && data.subtype !== "success") {
        agentNewBlock("text");
        agentAppend("text", `[${t("agent.error_inline", { err: `${data.subtype}${data.result ? ` — ${data.result}` : ""}` })}]`);
        agentCloseBlock();
      }
      clearPermissionCards();
      agentTransition("done");
      cliSawStreamDelta = false;
      cliDoneCallback?.(info);
      cliTextCallback = null;
      cliDoneCallback = null;
    }
    // While any permission is pending the CLI blocks on our response —
    // keep the pause indicator visible regardless of other events.
    if (pendingPermissionCards.size > 0) {
      statusText.textContent = t("agent.permission_wait");
    }
  } catch { /* ignore non-JSON lines */ }
});

// Instance id of the currently-known agent.exe. Every cli-status event carries
// the instance it belongs to; a "closed" from an older instance (arriving
// after a stop+restart) is ignored so it can't tear down a fresh session.
let cliCurrentInstance: number | null = null;

listen<{ state: string; message: string; instance?: number }>("cli-status", (event) => {
  if (event.payload.state === "starting") {
    cliReady = false;
    statusText.textContent = t("status.ai", { mode: currentModeLabel() });
    updateAgentStatus("loading");
  } else if (event.payload.state === "stdout") {
    cliReady = true;
    if (event.payload.instance) cliCurrentInstance = event.payload.instance;
    statusText.textContent = t("status.ai", { mode: currentModeLabel() });
    updateAgentStatus("ready");
    // Retry pending agent chat if CLI just became ready
    const retryQuery = (window as any).__agent_pending_query;
    if (retryQuery) {
      (window as any).__agent_pending_query = null;
      startAgentChat(retryQuery);
    }
  } else if (event.payload.state === "closed") {
    // Stale close from a previous agent.exe instance (stop + fast restart) →
    // ignore, it must not null cliReady or end the new session's turn.
    if (event.payload.instance && cliCurrentInstance && event.payload.instance !== cliCurrentInstance) {
      console.warn(`[cli] ignoring stale closed event (instance ${event.payload.instance} != ${cliCurrentInstance})`);
      return;
    }
    cliReady = false;
    updateAgentStatus("idle");
    clearPermissionCards(); // CLI is gone — pending approvals can't be answered
    if (agentState !== "idle") agentTransition("idle");
    if (cliDoneCallback) {
      cliDoneCallback();
      cliTextCallback = null;
      cliDoneCallback = null;
    }
    if (lastCliStderr) {
      statusText.textContent = t("status.ai", { mode: currentModeLabel() });
      updateAgentStatus("error", lastCliStderr);
    }
  }
});

// CLI stderr — log for debugging; keep the last line for exit diagnostics
let lastCliStderr = "";
listen<string>("cli-stderr", (event) => {
  lastCliStderr = event.payload;
  console.warn("[cli-stderr]", event.payload);
});

function clearSearch(status: string) {
  searchInput.value = "";
  autoResizeTextarea();
  resultsContainer.classList.add("hidden");
  searchBar.classList.remove("has-results");
  resultsList.innerHTML = "";
  currentResults = [];
  currentApps = [];
  selectedIndex = 0;
  statusText.textContent = status;
  searchInput.focus();
  syncEmpty(); // 程序性清空不会触发 input 事件，显式同步
  // Also clear attached file chips
  clearFileChips();
}

// Sync initial visibility + render AI entry on startup
(async () => {
  try { isVisible = await win.isVisible(); } catch { isVisible = true; }
  if (isVisible) {
    // Show AI Agent entry when app first opens (no query, no plugin)
    renderAIEntry();
  }
})();

// ── Settings button ──────────────────────────────────────────────
settingsBtn.addEventListener("click", async () => {
  // Toggle: if settings is already open, close it
  if (pluginActive && activePluginId === "settings") {
    await closePluginView();
    return;
  }
  // If any other plugin is open, close it first, then open settings
  if (pluginActive) {
    await closePluginView();
    await new Promise(r => setTimeout(r, 50));
  }
  const plugin = pluginRegistry.getAll().find(p => p.id === "settings");
  if (plugin) {
    resultsContainer.classList.remove("hidden");
    searchBar.classList.add("has-results");
    await executePlugin(plugin);
  }
});

// ── Search ───────────────────────────────────────────────────────
// 快速连打字会每键触发一次：Rust search_apps（同步遍历 Start Menu）+ 整表重建 +
// 窗口高度刷新，造成顿卡/卡死。这里做 60ms 输入去抖，并丢弃过期输入。
let _searchTimer: ReturnType<typeof setTimeout> | null = null;
let _searchSeq = 0;
searchInput.addEventListener("input", () => {
  const seq = ++_searchSeq;
  if (_searchTimer) clearTimeout(_searchTimer);
  _searchTimer = setTimeout(() => {
    _searchTimer = null;
    if (seq !== _searchSeq) return; // 过期输入，丢弃
    void runSearchNow(seq);
  }, 60);
});

async function runSearchNow(searchSeq: number) {
  // Defense-in-depth: while a plugin owns resultsList (e.g. AI chat), a
  // synthetic search event must not rebuild the plugin panel / chat log.
  if (pluginActive) return;

  // Auto-resize textarea height based on content
  autoResizeTextarea();

  const q = searchInput.value.trim();

  // Always show AI Agent entry — even when search is empty.
  // Hiding the results container on empty query breaks the "always available" UX.
  if (!q && attachedFiles.length === 0) {
    renderAIEntry();

    // If clipboard has an image, also show OCR entry above AI entry
    if ((window as any).__lunac_clipboard_has_image) {
      renderClipboardOCREntry();
    }

    return;
  }

  // ── App search — inlined in the main search bar (not a plugin) ──
  let appResults: AppEntry[] = [];
  try {
    appResults = await invoke<AppEntry[]>("search_apps", { query: q, limit: 5 });
  } catch {
    // Fall through — app search is non-critical
  }
  // 内容检测（快速键入只显示最后结果）：本次派发对应的键入序号 searchSeq 若已过期
  // （await 期间输入又更新，_searchSeq 前进），说明这是旧查询的晚回包 —— 直接丢弃，
  // 禁止旧结果覆盖新结果 / 造成一次键入多次渲染闪烁。过期不触碰任何 UI 状态。
  // 期间若已进入插件态也丢弃，防止晚回包重建 resultsList 冲掉插件面板。
  if (searchSeq !== _searchSeq || pluginActive) return;

  currentApps = appResults;
  // Sort apps by recency to match visual display order (renderMixedResults uses same sort).
  // Without this, Enter key selects the raw (non-recency) order — a mismatch with what's shown.
  const recencyStore = loadRecency();
  currentApps.sort((a, b) => {
    const ra = getRecencyScoreFor("app://" + a.path, recencyStore);
    const rb = getRecencyScoreFor("app://" + b.path, recencyStore);
    return rb - ra;
  });
  currentResults = pluginRegistry.search(q);

  // OCR entry: always show when clipboard has an image (unless already matched by keyword)
  if ((window as any).__lunac_clipboard_has_image) {
    const hasOCRR = currentResults.some(p => p.id === "ocr");
    if (!hasOCRR) {
      const ocrPlugin = pluginRegistry.getAll().find(p => p.id === "ocr");
      if (ocrPlugin) {
        currentResults.unshift(ocrPlugin);
      }
    }
  }

  selectedIndex = 0;

  const hasApps = appResults.length > 0;
  const hasPlugins = currentResults.length > 0;
  const hasFiles = attachedFiles.length > 0;

  // ── File actions always at top when files are attached ─────────
  // These are independent of search matches — a file user wants AI or direct launch.
  if (hasFiles) {
    resultsContainer.classList.remove("hidden");
    searchBar.classList.add("has-results");
    resultsList.innerHTML = "";

    // AI Agent — always present as main fallback
    const aiItem = doc("div");
    aiItem.className = "result-item selected";
    aiItem.dataset.kind = "ai-fallback"; // right-click → Ask AI
    aiItem.innerHTML = `
      <div class="result-item-icon">${pluginIconSvg("ai-agent")}</div>
      <div class="result-item-content">
        <div class="result-item-title">${t("chat.ai_entry")}</div>
        <div class="result-item-desc">${hasFiles ? `"${esc(q.slice(0, 60)) || t("chat.ask_files")}"` : `"${esc(q.slice(0, 60))}"`}</div>
      </div>
      <span class="result-item-badge">AI</span>
    `;
    aiItem.addEventListener("click", () => startAIChat(q || "", attachedFiles));
    resultsList.appendChild(aiItem);

    // OCR — show when image files are attached
    // Supported by PaddleOCR-json (OpenCV imread): PNG, JPEG, BMP, TIFF, WebP, etc.
    const imageExtensions = /\.(png|jpe?g|jfif|jpe|bmp|tiff?|webp|dib|ico)$/i;
    const hasImageFiles = attachedFiles.some(f => imageExtensions.test(f));
    if (hasImageFiles) {
      const ocrPlugin = pluginRegistry.getAll().find(p => p.id === "ocr");
      if (ocrPlugin) {
        const ocrItem = doc("div");
        ocrItem.className = "result-item";
        ocrItem.dataset.kind = "ocr"; // right-click → Ask AI fallback
        ocrItem.innerHTML = `
          <div class="result-item-icon">${pluginIconSvg("ocr")}</div>
          <div class="result-item-content">
            <div class="result-item-title">${t("plugin.ocr")}</div>
            <div class="result-item-desc">${t("chat.attach_ocr_file")}</div>
          </div>
          <span class="result-item-badge">OCR</span>
        `;
        ocrItem.addEventListener("click", () => {
          const imgFile = attachedFiles.find(f => imageExtensions.test(f));
          (window as any).__lunac_ocr_image = imgFile || null;
          executePlugin(ocrPlugin);
        });
        resultsList.appendChild(ocrItem);
      }
    }

    // Custom Local Launch
    const launchItem = doc("div");
    launchItem.className = "result-item";
    launchItem.dataset.kind = "quick-launch"; // right-click → Ask AI fallback
    launchItem.innerHTML = `
      <div class="result-item-icon">${pluginIconSvg("quick-launch")}</div>
      <div class="result-item-content">
        <div class="result-item-title">${t("chat.custom_launch")}</div>
        <div class="result-item-desc">${t("chat.files_attached", { count: String(attachedFiles.length) })}</div>
      </div>
      <span class="result-item-badge app-badge">${t("chat.launch_app")}</span>
    `;
    const qlPlugin = pluginRegistry.getAll().find(p => p.id === "quick-launch");
    if (qlPlugin) {
      launchItem.addEventListener("click", () => executePlugin(qlPlugin));
    }
    resultsList.appendChild(launchItem);

    currentApps = [];
    currentResults = [];
    selectedIndex = 0;
    statusText.textContent = t("chat.ai_with_files", { count: String(attachedFiles.length) });
    scheduleSearchResize(); // 搜索路径懒测量（方案4）：键入时不逐键 setSize
    return; // File actions take priority — no mixed results below
  }

  if (!hasApps && !hasPlugins) {
    // 无应用/插件匹配（如输入无关乱码）：仍常驻显示 3 个回退项——
    // Web 搜索、AI 助手问答、备忘录录入。统一交给 renderMixedResults 的
    // ws-fallback / ai-fallback / memo-fallback 逻辑渲染（此前遗漏 memo）。
    currentApps = [];
    currentResults = [];
    selectedIndex = 0;
    resultsContainer.classList.remove("hidden");
    searchBar.classList.add("has-results");
    resultsList.innerHTML = "";
    statusText.textContent = "AI";
    renderMixedResults(appResults, currentResults);
    scheduleSearchResize(); // 搜索路径懒测量（方案4）
  } else {
    resultsContainer.classList.remove("hidden");
    searchBar.classList.add("has-results");
    const parts: string[] = [];
    if (hasApps) parts.push(t("chat.app_count", { count: String(appResults.length) }));
    if (hasPlugins) parts.push(t("chat.tool_count", { count: String(currentResults.length) }));
    if (hasFiles) parts.push(t("chat.file_count", { count: String(attachedFiles.length) }));
    statusText.textContent = parts.join(" · ");
    renderMixedResults(appResults, currentResults);
  }
}

// ── Web Search item builder (shared by all render paths) ──────────
// Creates the Web Search result item with current engine name and
// right-click context menu to switch search engines.

const SEARCH_ENGINES: Array<{ id: string; name: string; icon: string }> = [
  { id: "google",     name: "Google",     icon: "🔍" },
  { id: "bing",       name: "Bing",       icon: "🌐" },
  { id: "baidu",      name: "Baidu",      icon: "🐻" },
];

function buildWebSearchItem(wsPlugin: Plugin, query: string, appendTo: HTMLElement | DocumentFragment): HTMLElement {
  const engine = getSearchEngine();
  const engineName = SEARCH_ENGINES.find(e => e.id === engine)?.name || "Google";
  const desc = query
    ? t("chat.search_on_engine", { q: query.slice(0, 60), engine: engineName })
    : t("chat.search_web_with", { engine: engineName });

  const wsItem = doc("div");
  wsItem.className = "result-item";
  wsItem.dataset.ws = "1"; // stable marker for engine-switch updates (title text is localized now)
  wsItem.dataset.kind = "ws-fallback"; // right-click handled by own engine-switch menu below
  wsItem.innerHTML = `
    <div class="result-item-icon">${pluginIconSvg("web-search")}</div>
    <div class="result-item-content">
      <div class="result-item-title">${t("plugin.web-search")}</div>
      <div class="result-item-desc">${esc(desc)}</div>
    </div>
    <span class="result-item-badge">${t("chat.badge_web")}</span>
  `;
  wsItem.addEventListener("click", () => wsPlugin.execute(query));

  // Right-click: switch search engine context menu
  // stopPropagation: prevents bubbling to resultsContainer's contextmenu
  // handler, which would otherwise show a second (mis-indexed) menu on top.
  wsItem.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    e.stopPropagation();
    // 互斥：打开引擎菜单前先隐藏结果区菜单，避免两菜单同时显示
    hideContextMenu();
    const existing = document.getElementById("ws-context-menu");
    existing?.remove();
    const curEng = getSearchEngine();
    const menu = doc("div");
    menu.id = "ws-context-menu";
    // 与 #context-menu 相同的 zoom 补偿（CSS zoom 会放大 fixed 元素）
    const z = currentZoom || 1;
    menu.style.cssText = `position:fixed;left:${e.clientX / z}px;top:${e.clientY / z}px;z-index:1000;`;
    menu.innerHTML = SEARCH_ENGINES.map(s =>
      `<div class="context-menu-item${s.id === curEng ? ' context-menu-item-active' : ''}" data-engine="${s.id}">${s.icon} ${s.name}</div>`
    ).join("");
    menu.querySelectorAll(".context-menu-item").forEach(item => {
      item.addEventListener("click", () => {
        const id = (item as HTMLElement).dataset.engine;
        if (id && id !== curEng) {
          setSearchEngine(id);
          // Update all web search descriptions on screen (identified by data-ws
          // marker — the title is localized so text comparison no longer works)
          document.querySelectorAll(".result-item[data-ws]").forEach(item => {
            const descEl = item.querySelector(".result-item-desc");
            if (descEl) {
              const newName = SEARCH_ENGINES.find(e => e.id === id)?.name || "Google";
              const q = searchInput.value.trim();
              descEl.textContent = q
                ? t("chat.search_on_engine", { q: q.slice(0, 60), engine: newName })
                : t("chat.search_web_with", { engine: newName });
            }
          });
        }
        menu.remove();
      });
    });
    document.body.appendChild(menu);
    // 溢出视口边缘时翻转（与 #context-menu 相同：视口尺寸按 zoom 换算）
    requestAnimationFrame(() => {
      const rect = menu.getBoundingClientRect();
      const vw = window.innerWidth / z;
      const vh = window.innerHeight / z;
      if (rect.right > vw) menu.style.left = `${vw - rect.width - 8}px`;
      if (rect.bottom > vh) menu.style.top = `${vh - rect.height - 8}px`;
    });
    // Close on any outside click
    const close = (ev: MouseEvent) => {
      if (!menu.contains(ev.target as Node)) {
        menu.remove();
        document.removeEventListener("click", close);
      }
    };
    setTimeout(() => document.addEventListener("click", close), 0);
  });

  appendTo.appendChild(wsItem);
  return wsItem;
}

// ── Render mixed results: apps first, then plugins ────────────────
function renderMixedResults(apps: AppEntry[], plugins: Plugin[]) {
  // Never overwrite the results area when a plugin panel is active
  if (pluginActive) return;

  const frag = document.createDocumentFragment();
  let idx = 0;
  const q = searchInput.value.trim();
  const recencyStore = loadRecency();

  // ── Unified recency-scored list: apps + plugins + fallback entries ──
  type LocalEntry =
    | { kind: "app"; app: AppEntry; recency: number }
    | { kind: "plugin"; plugin: Plugin; recency: number }
    | { kind: "ws-fallback"; recency: number }
    | { kind: "memo-fallback"; recency: number }
    | { kind: "memo-tag-open"; id: string; tag: string; recency: number }
    | { kind: "ai-fallback"; recency: number };

  const entries: LocalEntry[] = [];

  // Apps
  for (const a of apps) {
    entries.push({ kind: "app", app: a, recency: getRecencyScoreFor("app://" + a.path, recencyStore) });
  }

  // Plugins
  const renderedPluginIds = new Set<string>();
  for (const p of plugins) {
    entries.push({ kind: "plugin", plugin: p, recency: getRecencyScoreFor("plugin://" + p.id, recencyStore) });
    renderedPluginIds.add(p.id);
  }

  // Fallback entries — only if not matched by search query
  if (!renderedPluginIds.has("web-search") && pluginRegistry.getAll().some(p => p.id === "web-search")) {
    entries.push({ kind: "ws-fallback", recency: getRecencyScoreFor("plugin://web-search", recencyStore) });
  }
  if (!renderedPluginIds.has("ai-agent")) {
    entries.push({ kind: "ai-fallback", recency: getRecencyScoreFor("plugin://ai-agent", recencyStore) });
  }
  // 标识直达：输入与某条备忘录“标识”精确/前缀匹配 → “编辑备忘录·标识”条目
  let memoTagHit: { id: string; tag: string } | null = null;
  try {
    const norm = q.toLowerCase().replace(/^#/, "").trim();
    if (norm) {
      const idx: { id: string; tag: string }[] = (window as any).__lunac_memo_index || [];
      const hit = idx.find(x => {
        const tg = x.tag.toLowerCase();
        return tg === norm || (norm.length >= 2 && tg.startsWith(norm));
      });
      if (hit) memoTagHit = hit;
    }
  } catch {}
  if (memoTagHit) {
    entries.push({ kind: "memo-tag-open", id: memoTagHit.id, tag: memoTagHit.tag, recency: 999 + Math.random() });
  }
  // 常显备忘录入口：输入任意文本时，结果区始终提供"填入备忘录新建任务"入口
  if (q && !memoTagHit && !renderedPluginIds.has("memo") && pluginRegistry.getAll().some(p => p.id === "memo")) {
    entries.push({ kind: "memo-fallback", recency: getRecencyScoreFor("plugin://memo", recencyStore) });
  }

  // Sort all entries by recency (descending)
  entries.sort((a, b) => b.recency - a.recency);

  // Save unified order for Enter key handler — must match visual rendering
  currentEntries = entries.map(e => {
    if (e.kind === "app") return { kind: "app" as const, app: e.app };
    if (e.kind === "plugin") return { kind: "plugin" as const, plugin: e.plugin };
    if (e.kind === "memo-tag-open") return { kind: "memo-tag-open" as const, id: e.id, tag: e.tag };
    return { kind: e.kind }; // ws-fallback | memo-fallback | ai-fallback
  });

  // Render in recency order
  for (const entry of entries) {
    if (entry.kind === "app") {
      const app = entry.app;
      const item = doc("div");
      item.className = `result-item${idx === 0 ? " selected" : ""}`;
      item.dataset.kind = "app"; // right-click dispatch (context menu)
      item.dataset.appPath = app.path;
      item.dataset.appName = app.name;
      const extMatch = app.path.match(/\.([a-zA-Z0-9]+)$/);
      const ext = extMatch ? extMatch[1].toLowerCase() : "";
      const isFolder = !ext || ext === "lnk";
      const iconText = isFolder ? "📁" : (ext.length <= 3 ? ext : ext.slice(0, 3));
      const badgeText = isFolder ? (ext === "lnk" ? t("chat.badge_shortcut") : t("chat.badge_folder")) : ext;
      item.innerHTML = `
        <div class="result-item-icon" data-icon-path="${esc(app.path)}">${iconText}</div>
        <div class="result-item-content">
          <div class="result-item-title">${esc(app.name)}</div>
          <div class="result-item-desc">${t("chat.launch_app")}</div>
        </div>
        <span class="result-item-badge app-badge">${esc(badgeText)}</span>
      `;
      item.addEventListener("click", () => launchApp(app.path));
      frag.appendChild(item);
      invoke<string | null>("get_app_icon", { path: app.path }).then(dataUrl => {
        if (dataUrl) {
          const iconEl = item.querySelector(".result-item-icon") as HTMLElement;
          if (iconEl) iconEl.innerHTML = `<img src="${dataUrl}" class="result-item-icon-img" alt="">`;
        }
      }).catch(() => {});
    } else if (entry.kind === "plugin") {
      const plugin = entry.plugin;
      const item = doc("div");
      item.className = `result-item${idx === 0 ? " selected" : ""}`;
      item.dataset.kind = "plugin"; // right-click dispatch (context menu)
      item.dataset.pluginId = plugin.id;
      item.innerHTML = `
        <div class="result-item-icon">${pluginIconSvg(plugin.id)}</div>
        <div class="result-item-content">
          <div class="result-item-title">${esc(plugin.name)}</div>
          <div class="result-item-desc">${esc(plugin.description)}</div>
        </div>
        <span class="result-item-badge">${esc(plugin.badge || t("chat.badge_tool"))}</span>
      `;
      item.addEventListener("click", async () => {
        if (pluginActive && activePluginId !== plugin.id) {
          await closePluginView();
          await new Promise(r => setTimeout(r, 50));
        }
        executePlugin(plugin);
      });
      frag.appendChild(item);
    } else if (entry.kind === "ws-fallback") {
      const wsPlugin = pluginRegistry.getAll().find(p => p.id === "web-search")!;
      const wsItem = buildWebSearchItem(wsPlugin, q, frag);
      if (idx === 0) wsItem.classList.add("selected");
    } else if (entry.kind === "ai-fallback") {
      const item = doc("div");
      item.className = `result-item${idx === 0 ? " selected" : ""}`;
      item.dataset.kind = "ai-fallback"; // right-click → Ask AI
      item.innerHTML = `
        <div class="result-item-icon">${pluginIconSvg("ai-agent")}</div>
        <div class="result-item-content">
          <div class="result-item-title">${t("chat.ai_entry")}</div>
          <div class="result-item-desc">"${esc(q.slice(0, 40))}"</div>
        </div>
        <span class="result-item-badge">AI</span>
      `;
      item.addEventListener("click", () => startAIChat(q));
      frag.appendChild(item);
    } else if (entry.kind === "memo-fallback") {
      const memoPlugin = pluginRegistry.getAll().find(p => p.id === "memo")!;
      const item = doc("div");
      item.className = `result-item${idx === 0 ? " selected" : ""}`;
      item.dataset.kind = "memo-fallback"; // right-click → run memo
      item.innerHTML = `
        <div class="result-item-icon">${pluginIconSvg("memo")}</div>
        <div class="result-item-content">
          <div class="result-item-title">${esc(pluginName("memo"))}</div>
          <div class="result-item-desc">"${esc(q.slice(0, 40))}"</div>
        </div>
        <span class="result-item-badge">${esc(memoPlugin.badge || "memo")}</span>
      `;
      item.addEventListener("click", () => executePlugin(memoPlugin));
      frag.appendChild(item);
    } else if (entry.kind === "memo-tag-open") {
      // 搜索标识命中 → 直接进入该备忘录编辑界面
      const memoPlugin = pluginRegistry.getAll().find(p => p.id === "memo")!;
      const item = doc("div");
      item.className = `result-item${idx === 0 ? " selected" : ""}`;
      item.dataset.kind = "memo-tag-open";
      item.innerHTML = `
        <div class="result-item-icon">${pluginIconSvg("memo")}</div>
        <div class="result-item-content">
          <div class="result-item-title">${t("chat.memo_edit_tag", { tag: esc(entry.tag) })}</div>
          <div class="result-item-desc">${esc(entry.tag)}</div>
        </div>
        <span class="result-item-badge">memo</span>
      `;
      item.addEventListener("click", () => {
        (window as any).__lunac_memo_open = { id: entry.id };
        executePlugin(memoPlugin);
      });
      frag.appendChild(item);
    }
    idx++;
  }

  resultsList.replaceChildren(frag);
  applyWindowSize();
}

// ── Launch application — via Rust ShellExecute ──────────────────
// CRITICAL: hide IPC must be queued BEFORE any sync operations
// (DOM reflow, localStorage) to avoid WebView2 delaying IPC flush.
// Double-insurance: win.hide() (Tauri API) + invoke("hide_lunac") (direct ShowWindow).
function launchApp(path: string) {
  if (_execGuard) return;
  _execGuard = true;

  // ── STEP 0: Queue hide IPCs FIRST — before any sync work ─────
  // The mere act of queueing the IPC (not awaiting it) is enough;
  // WebView2 flushes the IPC queue after the current task completes.
  invoke("hide_lunac").catch(() => {});  // Rust ShowWindow(SW_HIDE) — direct, no Tauri wrapper
  win.hide().catch(() => {});            // Tauri Window API hide — standard path

  // ── STEP 1: Guard state — must be set before onFocusChanged ───
  // Done AFTER hide IPC queue to keep hide at top, but BEFORE any
  // async gap (IPC processing), ensuring onFocusChanged sees blocking flags.
  blockingHide = true;
  isVisible = false;

  // ── STEP 2: Background work (deferred, not on critical path) ──
  setTimeout(() => { recordRecency("app://" + path); }, 0);

  // ── STEP 3: UI cleanup (sync, fast) ───────────────────────────
  searchInput.value = "";
  autoResizeTextarea();
  statusText.textContent = t("status.launched");

  // ── STEP 4: Launch app — concurrent, fire-and-forget ──────────
  invoke("set_query_state", { empty: true }).catch(() => {});
  invoke("launch_app", { path }).catch((e) => {
    console.warn("[lunac] launch_app failed:", path, e);
  });

  // ── STEP 5: Cleanup ──────────────────────────────────────────
  setTimeout(() => { _execGuard = false; }, 300);
  setTimeout(() => { blockingHide = false; }, 500);
}

// ── Execute plugin ───────────────────────────────────────────────
async function executePlugin(plugin: Plugin) {
  if (_execGuard) return;
  _execGuard = true;
  setTimeout(() => { _execGuard = false; }, 500); // longer: plugin execute can take time
  const q = searchInput.value.trim();
  searchInput.disabled = true;
  statusText.textContent = t("status.running_plugin", { name: pluginName(plugin.id) });

  // Mark plugin as active (ESC will close it, not hide the window)
  pluginActive = true;
  activePluginId = plugin.id;
  setPluginBar(pluginName(plugin.id));
  invoke("set_plugin_active", { active: true }).catch(() => {});

  // Allow native select dropdowns to overflow the container (no clipping)
  resultsContainer.classList.add("plugin-open");

  // If we have saved state for this plugin, restore it
  // Settings plugin MUST re-execute every time — its state is authoritative
  // from the backend (registry/config), not the DOM. DOM checkbox `checked`
  // attribute is NOT serialized in innerHTML, so cached HTML always shows OFF.
  // AI agent never restores cached HTML — conversations go to history,
  // the dialog should always start fresh.
  // OCR never restores — the two-panel detached UI is rebuilt fresh each time,
  // and the image path changes between sessions. Old cached HTML would show
  // a "bubble" wrapper in detached mode with mismatched element IDs.
  // Quick-launch never restores — 面板 = 注册表的实时视图（重开必须重列，
  // 否则缓存 HTML 会让“新添加的注册项消失”）。
  const skipRestore = plugin.id === "settings" || plugin.id === "ai-agent" || plugin.id === "ocr" || plugin.id === "memo" || plugin.id === "quick-launch";
  const saved = skipRestore ? undefined : pluginStates.get(plugin.id);
  if (saved?.html) {
    resultsList.innerHTML = saved.html;
    // Restore the original search input that triggered this plugin view
    if (saved.searchQuery) searchInput.value = saved.searchQuery;
    statusText.textContent = t("status.plugin_restored", { name: pluginName(plugin.id) });
    // Re-attach plugin listeners for interactive plugins
    if (plugin.id === "settings") {
      setTimeout(() =>
        import("./plugins/builtin/settings")
          .then(m => {
            console.log("[lunac main] settings module loaded (restored), calling attachSettingsListeners...");
            m.attachSettingsListeners(resultsList);
          })
          .catch(err => console.error("[lunac main] FAILED to import settings module (restored):", err)),
        50);
    }
    // quick-launch is in skipRestore — never enters this branch; panel re-executes
    if (plugin.id === "tool-editor") {
      setTimeout(() =>
        import("./plugins/builtin/tool-editor").then(m =>
          m.attachToolEditorListeners()
        ), 50);
    }
    if (plugin.id === "ocr") {
      setDetached(true);
      setTimeout(() =>
        import("./plugins/builtin/ocr").then(m => {
          m.attachOcrListeners(document);
          const imgPath = (window as any).__lunac_ocr_image as string | undefined;
          if (imgPath) {
            delete (window as any).__lunac_ocr_image;
            m.ocrImageFile(imgPath);
          } else {
            m.autoStartClipboardOcr();
          }
        }), 50);
    }
    applyWindowSize();
    return;
  }

  try {
    const result = await plugin.execute(q);
    if (result.type === "html") {
      // OCR uses its own full-height detached layout (no plugin-result wrapper)
      if (plugin.id === "ocr") {
        resultsList.innerHTML = result.content;
        setDetached(true);
        setTimeout(() =>
          import("./plugins/builtin/ocr").then(m => {
            m.attachOcrListeners(document);
            const imgPath = (window as any).__lunac_ocr_image as string | undefined;
            if (imgPath) {
              delete (window as any).__lunac_ocr_image;
              m.ocrImageFile(imgPath);
            } else {
              m.autoStartClipboardOcr();
            }
          }), 50);
      } else {
        resultsList.innerHTML = `
          <div class="plugin-result">
            ${result.content}
          </div>`;
      }
      if (plugin.id === "settings") {
        setTimeout(() =>
          import("./plugins/builtin/settings")
            .then(m => {
              console.log("[lunac main] settings module loaded, calling attachSettingsListeners...");
              m.attachSettingsListeners(resultsList);
            })
            .catch(err => console.error("[lunac main] FAILED to import settings module:", err)),
          50);
      }
      if (plugin.id === "quick-launch") {
        setTimeout(() =>
          import("./plugins/builtin/quick-launch").then(m =>
            m.attachQuickLaunchListeners(resultsList)
          ), 50);
      }
      if (plugin.id === "tool-editor") {
        setTimeout(() =>
          import("./plugins/builtin/tool-editor").then(m =>
            m.attachToolEditorListeners()
          ), 50);
      }
      if (plugin.id === "memo") {
        setTimeout(() =>
          import("./plugins/builtin/memo").then(m =>
            m.attachMemoListeners(resultsList)
          ), 50);
      }
    } else {
      resultsList.innerHTML = `
        <div class="plugin-result">
          <div class="plugin-result-content">${esc(result.content)}</div>
        </div>`;
    }
    statusText.textContent = "";
    recordRecency("plugin://" + plugin.id);
  } catch (err: any) {
    resultsList.innerHTML = `
      <div class="plugin-result">
        <div class="plugin-result-content error">${esc(err.message || String(err))}</div>
      </div>`;
    statusText.textContent = t("status.plugin_error", { err: err.message });
  }
  applyWindowSize();
}

// ── Keyboard ─────────────────────────────────────────────────────
searchInput.addEventListener("keydown", (e) => {
  // Backspace on empty input → remove last file chip
  if (e.key === "Backspace" && !searchInput.value && attachedFiles.length > 0) {
    e.preventDefault();
    // 省略泡泡折叠态下 Backspace 视为删除该省略分支（与省略号内“一键删除”同步）；
    // 已展开或没有折叠时则逐条删除最后一个泡泡。
    if (!chipMoreOpen && attachedFiles.length > FILE_CHIP_LIMIT) {
      clearHiddenFileChips();
    } else {
      removeFileChip(attachedFiles.length - 1);
    }
    return;
  }

  // Escape 由 document 级监听统一处理

  if (e.key === "ArrowDown") {
    e.preventDefault();
    // File-actions view renders DOM-only entries (AI Agent / Custom Launch)
    const total = attachedFiles.length > 0
      ? resultsList.querySelectorAll(".result-item").length
      : currentEntries.length;
    if (total > 0) {
      selectedIndex = Math.min(selectedIndex + 1, total - 1);
      updateSelection();
    }
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    selectedIndex = Math.max(selectedIndex - 1, 0);
    updateSelection();
  } else if (e.key === "Enter") {
    e.preventDefault();
    if (isStreaming || _execGuard) return;
    // File-actions view: activate selected DOM entry directly
    if (attachedFiles.length > 0) {
      const sel = resultsList.querySelector(".result-item.selected") as HTMLElement | null;
      sel?.click();
      return;
    }
    // Unified entry list (mirrors visual order from renderMixedResults)
    const entry = currentEntries[selectedIndex];
    if (entry) {
      if (entry.kind === "app") {
        launchApp(entry.app.path);
        return;
      }
      if (entry.kind === "plugin") {
        executePlugin(entry.plugin);
        return;
      }
      if (entry.kind === "ws-fallback") {
        const wsPlugin = pluginRegistry.getAll().find(p => p.id === "web-search");
        if (wsPlugin) {
          executePlugin(wsPlugin);
          return;
        }
      }
      if (entry.kind === "ai-fallback") {
        startAIChat(searchInput.value.trim());
        return;
      }
      if (entry.kind === "memo-fallback") {
        const memoPlugin = pluginRegistry.getAll().find(p => p.id === "memo");
        if (memoPlugin) {
          executePlugin(memoPlugin);
          return;
        }
      }
      if (entry.kind === "memo-tag-open") {
        const memoPlugin = pluginRegistry.getAll().find(p => p.id === "memo");
        if (memoPlugin) {
          (window as any).__lunac_memo_open = { id: entry.id };
          executePlugin(memoPlugin);
          return;
        }
      }
    }
    // Ultimate fallback: AI chat for any typed query
    if (searchInput.value.trim()) {
      startAIChat(searchInput.value.trim());
    }
  }
});

function updateSelection() {
  resultsList.querySelectorAll(".result-item").forEach((item, i) => {
    item.classList.toggle("selected", i === selectedIndex);
  });
  // Auto-scroll to keep selected item visible
  const selected = resultsList.querySelector(".result-item.selected") as HTMLElement | null;
  if (selected) {
    selected.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }
}

// ── AI Chat ─────────────────────────────────────────────────────

/** Render LaTeX formulas in a container using KaTeX.
 *  Manually scans for $$...$$ and $...$ delimiters.
 *  More reliable than renderMathInElement for dynamically inserted HTML. */
function renderLatex(container: HTMLElement) {
  if (typeof katex === "undefined") return;
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  const textNodes: Text[] = [];
  let node: Text | null;
  while ((node = walker.nextNode() as Text | null)) {
    if (node.textContent && (node.textContent.includes("$") || node.textContent.includes("\\["))) {
      textNodes.push(node);
    }
  }
  for (const tn of textNodes) {
    const text = tn.textContent || "";
    // Process $$...$$ first, then $...$
    const processed = processLatexDelimiters(text);
    if (processed !== text) {
      const span = document.createElement("span");
      span.innerHTML = processed;
      tn.parentNode?.replaceChild(span, tn);
    }
  }
}

function processLatexDelimiters(text: string): string {
  let result = text;
  // Display math: $$...$$
  result = result.replace(/\$\$([\s\S]+?)\$\$/g, (_, expr: string) => {
    try {
      const el = document.createElement("span");
      katex.render(expr.trim(), el, { throwOnError: false, displayMode: true, trust: true });
      return el.innerHTML;
    } catch { return `$$${expr}$$`; }
  });
  // Inline math: $...$ (but not $$)
  result = result.replace(/(?<!\$)\$(?!\$)([\s\S]+?)(?<!\$)\$(?!\$)/g, (_, expr: string) => {
    try {
      const el = document.createElement("span");
      katex.render(expr.trim(), el, { throwOnError: false, displayMode: false, trust: true });
      return el.innerHTML;
    } catch { return `$${expr}$`; }
  });
  return result;
}

// ── Agent Chat (CLI subprocess) ─────────────────────────────────
// ── Context-aware System Prompt Injection ──────────────────────
// Detects user intent from query keywords and prepends relevant
// methodology hints so the agent follows best practices automatically.

/** Simple Q&A hint — soft-constrains the agent to answer directly
 *  without tools/skills/file access (replaces the old built-in chat.rs).
 *  Keeps the humanizer output style but drops debug/TDD/review hints
 *  that would otherwise encourage tool use. */
function buildSimpleChatHint(query: string): string {
  return `## Simple Q&A Mode
Answer the user's question directly from your own knowledge. Do NOT call any tools (Bash, Read, Edit, Write, Glob, Grep, WebSearch, Skill, Task, etc.). Do NOT use any skills. Do NOT read files, search the codebase, or run commands — unless the user explicitly asks you to. Keep the answer concise and accurate.

## Output Style
Remove AI writing patterns: no "stands as / testament / pivotal / crucial / underscoring / delve / tapestry / landscape / fostering / moreover / furthermore / in conclusion". No emoji decorations. No "I hope this helps / let me know / great question". No boldface headers in lists. Use simple "is/are/has" instead of "serves as/stands as/represents". Vary sentence rhythm. Have opinions.`;
}

function buildSystemPromptHint(query: string): string {
  const q = query.toLowerCase();
  const hints: string[] = [];

  // ── 固定人格（点6/7）────────────────────────────────────────
  // 每次思考/回答都以同一「性格 + 思考方式 + 说话风格」呈现，
  // 避免每次回答风格漂移。
  hints.push(`## Personality (fixed — always apply)
You are "Lunac", a sharp, fast desktop AI assistant built into a launcher. Stay in character every turn.
- Thinking style: before acting, briefly structure your reasoning as Context → Analysis → Decision, then execute. Do not second-guess after deciding.
- Speaking style: calm and direct, like a senior engineer explaining to a peer. Short varied sentences, first-person "I", concrete nouns and verbs.
- No filler: never use "stands as / testament / delve / tapestry / moreover / furthermore / in conclusion / great question / I hope this helps". Have a clear opinion and recommend the single best option rather than listing everything.
- Always reply in the user's language.`);

  // Debug intent: error / bug / fix / crash / not working
  if (/bug|error|crash|fail|break|fix|wrong|not work|不工作|报错|崩溃|修复|调试/.test(q)) {
    hints.push(`## Debugging Methodology
Follow the 4-phase systematic debugging process:
1. ROOT CAUSE: Read errors, build tight feedback loop, check recent changes, trace data flow
2. PATTERN ANALYSIS: Find working examples, compare differences
3. HYPOTHESIS: Form 3-5 falsifiable hypotheses, test one variable at a time
4. IMPLEMENTATION: Create regression test first, then single fix at root cause
CRITICAL: If 3+ fix attempts fail → question the architecture, don't try a 4th fix.`);
  }

  // Code creation intent: create / write / add / implement / build / make
  if (/create|write|add|implement|build|make|写|创建|实现|添加|新建/.test(q)) {
    hints.push(`## TDD Requirement
Write failing test FIRST before any production code. RED → GREEN → REFACTOR.
No exceptions: delete any code written before its test exists.`);
  }

  // Review intent: review / check / verify / audit
  if (/review|check|verify|audit|审查|检查|验证|审计/.test(q)) {
    hints.push(`## Code Review Pipeline
8-step pre-commit verification: diff → static scan → baseline tests → self-review → independent review → evaluate → auto-fix (max 2) → commit with [verified] prefix.`);
  }

  // Text output — always apply humanizer for agent responses
  hints.push(`## Output Style
Remove AI writing patterns: no "stands as / testament / pivotal / crucial / underscoring / delve / tapestry / landscape / fostering / moreover / furthermore / in conclusion". No emoji decorations. No "I hope this helps / let me know / great question". No boldface headers in lists. Use simple "is/are/has" instead of "serves as/stands as/represents". Vary sentence rhythm. Have opinions.`);

  return hints.join('\n\n');
}

/** Context pruning pipeline (Pi reference: transformContext).
 *  Before sending to the agent CLI, trim the conversation history to
 *  fit within a reasonable context window and inject relevant context.
 *  Returns the pruned messages array ready for LLM consumption. */
function pruneContext(messages: Array<{ role: string; content: string }>, maxTurns = 12): Array<{ role: string; content: string }> {
  if (messages.length <= maxTurns * 2) return messages;

  // Keep system hints + last N turns (user-assistant pairs)
  const systemHints = messages.filter(m => m.role === "system");
  const turns: Array<{ role: string; content: string }>[] = [];
  let current: Array<{ role: string; content: string }> = [];
  for (const m of messages) {
    if (m.role === "system") continue;
    if (m.role === "user" && current.length > 0) turns.push(current);
    current.push(m);
  }
  if (current.length > 0) turns.push(current);

  // Keep last N turns; trim message content to 8K chars
  const kept = turns.slice(-maxTurns).flatMap(t => t.map(m => ({
    ...m,
    content: m.content.length > 8192 ? m.content.slice(0, 8192) + "\n[content truncated]" : m.content,
  })));

  return [...systemHints, ...kept];
}

async function startAgentChat(query: string) {
  // Reset failure counter for new conversation (ai-spec §17.2)
  consecutiveFailures = 0;
  // File info is already merged into query by startAIChat.
  // (Retry path passes the stored finalQuery directly.)
  const finalQuery = query;

  // If CLI not ready, start it and queue this query for retry
  if (!cliReady) {
    agentTransition("starting");
    (window as any).__agent_pending_query = finalQuery;
    resultsContainer.classList.remove("hidden");
    searchBar.classList.add("has-results");
    const log = ensureChatLog();
    let hint = document.getElementById("agent-loading-hint");
    if (!hint) {
      hint = doc("div");
      hint.id = "agent-loading-hint";
      log.appendChild(hint);
    }
    hint.className = "plugin-result-content plugin-result-loading";
    hint.textContent = t("agent.loading") + ", " + t("agent.please_wait");
    statusText.textContent = t("agent.loading");
    pluginActive = true;
    activePluginId = "ai-agent";
    isChatHistoryView = false;
    setPluginBar(t("chat.ai_entry"));
    invoke("set_plugin_active", { active: true }).catch(() => {});
    applyWindowSize();
    try {
      await invoke("start_cli");
    } catch (err: any) {
      hint.className = "plugin-result-content error";
      hint.textContent = t("agent.start_failed", { err: err?.toString() || "Unknown error" });
      statusText.textContent = t("agent.failed");
      (window as any).__agent_pending_query = null;
      agentTransition("idle");
    }
    return;
  }

  isStreaming = true;
  setStreamingUI(true);
  userScrolledUp = false;
  const myStreamId = ++streamId;
  searchInput.disabled = true;
  statusText.textContent = t("agent.thinking");

  // Create structured turn (Pi: turn_start)
  const turn: AgentTurn = {
    id: Date.now().toString(36) + Math.random().toString(36).slice(2, 6),
    startTime: Date.now(),
    status: "running",
    toolCalls: [],
  };
  agentTurn = turn;
  agentTransition("starting");

  // Mark as plugin-active
  pluginActive = true;
  activePluginId = "ai-agent";
  isChatHistoryView = false;
  setPluginBar(t("chat.ai_entry"));
  invoke("set_plugin_active", { active: true }).catch(() => {});

  // Show streaming UI — wrap this turn in a .agent-turn container
  resultsContainer.classList.remove("hidden");
  searchBar.classList.add("has-results");
  document.getElementById("agent-loading-hint")?.remove();
  const log = ensureChatLog();
  const flowEl = doc("div");
  flowEl.className = "agent-flow";
  flowEl.dataset.turnId = turn.id;
  flowEl.innerHTML = `<div class="plugin-result-content plugin-result-loading">${t("agent.initializing")}</div>`;
  log.appendChild(flowEl);
  resultsList.scrollTop = resultsList.scrollHeight;
  applyWindowSize();

  cliSawStreamDelta = false; // new query — reset double-render guard
  agentView = {
    turn,
    flow: flowEl,
    current: null,
    currentKind: null,
    curDetails: null,
    curToolName: "",
    curToolArgs: "",
    thinkChars: 0,
    textAll: "",
  };

  cliDoneCallback = (info?: ChatDoneInfo) => {
    if (myStreamId !== streamId) return;

    agentCloseBlock();
    const v = agentView;
    const finalText = v ? v.textAll.trim() : "";
    if (v) {
      v.flow.querySelector(".plugin-result-loading")?.remove();
      // Render LaTeX across all text blocks
      setTimeout(() => renderLatex(v.flow), 10);
    }
    agentView = null;
    agentTurn = null;

    if (finalText) {
      chatHistory.push({ role: "assistant", content: finalText });
      humanizeBtn.style.display = "flex";
      // 复制按钮（同主题）——实时对话的助手全文，挂在 agent-flow 结尾
      const copyBtn = doc("button");
      copyBtn.className = "flow-copy";
      copyBtn.title = t("chat.copy_msg");
      copyBtn.innerHTML = COPY_SVG;
      copyBtn.addEventListener("click", async () => {
        const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
        writeText(finalText).catch(() => {});
      });
      flowEl.appendChild(copyBtn);
    }

    // Prune memory: cap chatHistory to last N turns (Pi: transformContext)
    chatHistory = pruneContext(chatHistory);

    // Turn footer: tool summary (Pi: turn_end reporting)
    if (turn.toolCalls.length > 0) {
      const successCount = turn.toolCalls.filter(t => t.status === "success").length;
      const errorCount = turn.toolCalls.filter(t => t.status === "error").length;
      const footer = doc("div");
      footer.className = "turn-footer";
      footer.innerHTML = `<span class="turn-tool-count">${
        errorCount > 0
          ? t("agent.tool_summary", { count: String(turn.toolCalls.length), ok: String(successCount), failed: String(errorCount) })
          : t("agent.tool_summary_ok", { count: String(turn.toolCalls.length), ok: String(successCount) })
      }</span>`;
      flowEl.appendChild(footer);
    }

    isStreaming = false;
    setStreamingUI(false);
    agentTransition("done");
    statusText.textContent = t("agent.done", { count: String(turn.toolCalls.length) });
    // CLI reports cumulative totals → compute per-turn deltas. If the new
    // cumulative value is SMALLER than the last saved one (CLI was restarted
    // or state got out of sync), take it as absolute (fresh cumulative start).
    if (info) {
      const fresh =
        info.input_tokens < lastAgentTokens.input ||
        info.output_tokens < lastAgentTokens.output;
      const delta: ChatDoneInfo = fresh
        ? {
            stop_reason: "end_turn",
            input_tokens: info.input_tokens,
            output_tokens: info.output_tokens,
            cache_read_input_tokens: info.cache_read_input_tokens ?? 0,
            cache_creation_input_tokens: info.cache_creation_input_tokens ?? 0,
          }
        : {
            stop_reason: "end_turn",
            input_tokens: Math.max(0, info.input_tokens - lastAgentTokens.input),
            output_tokens: Math.max(0, info.output_tokens - lastAgentTokens.output),
            cache_read_input_tokens: Math.max(0, (info.cache_read_input_tokens ?? 0) - lastAgentTokens.cacheRead),
            cache_creation_input_tokens: Math.max(0, (info.cache_creation_input_tokens ?? 0) - lastAgentTokens.cacheCreate),
          };
      lastAgentTokens = {
        input: info.input_tokens,
        output: info.output_tokens,
        cacheRead: info.cache_read_input_tokens ?? 0,
        cacheCreate: info.cache_creation_input_tokens ?? 0,
      };
      updateTokenDashboard(delta);
    }
    cliTextCallback = null;
    cliDoneCallback = null;

    // Process queued messages
    processQueue();

    // Auto-save the completed turn so history survives even if the panel
    // is closed without triggering newConversation/ensureChatLog.
    saveCurrentSession().catch(() => {});

    // Auto-save AI chat state
    pluginStates.set("ai-agent", {
      id: "ai-agent",
      html: resultsList.innerHTML,
      searchQuery: searchInput.value,
      pendingInput: chatInput.value,
    });
  };

  // Inject context-aware system prompt hints based on query keywords.
  // 思考模式三档（fast/think/deep）已由 Rust 环境变量控制，前端统一走
  // 完整 agent 提示词（不再按 simple/agent 二选一）。
  const hint = buildSystemPromptHint(finalQuery);
  const wrappedQuery = hint ? `${hint}\n\n---\n\n${finalQuery}` : finalQuery;

  // Store the CLEAN query (without the injected hint prefix) in history —
  // the hint is a send-time wrapper only and must not leak into saved
  // sessions / restored conversations (ai-spec §3.4).
  chatHistory.push({ role: "user", content: finalQuery });

  try {
    // Send NDJSON message to CLI (must include session_id and parent_tool_use_id)
    const msg = JSON.stringify({
      type: "user",
      session_id: "",
      message: {
        role: "user",
        content: [{ type: "text", text: wrappedQuery }]
      },
      parent_tool_use_id: null,
    });
    await invoke("send_message", { message: msg });
  } catch (err: any) {
    if (myStreamId !== streamId) return;
    agentView = null;
    agentTurn = null;
    agentTransition("idle");
    flowEl.innerHTML = `<span class="plugin-result-content error">${t("agent.error_inline", { err: esc(err?.toString() || "CLI not running") })}</span>`;
    isStreaming = false;
    setStreamingUI(false);
    statusText.textContent = t("agent.error");
    cliTextCallback = null;
    cliDoneCallback = null;
  }
}

// ── Chat log (Q&A bubble flow) ──────────────────────────────────

/** Ensure the persistent chat log container exists.
 *  Creating a fresh log = starting a new conversation (archives the old one). */
function ensureChatLog(): HTMLElement {
  let log = document.getElementById("chat-log");
  if (!log) {
    saveCurrentSession().catch(() => {}); // fire-and-forget: archive old session
    chatHistory = [];
    resultsList.innerHTML = `<div class="ai-response" id="chat-log"></div>`;
    log = document.getElementById("chat-log")!;
  }
  return log;
}

/** Append a user bubble to the chat log. `idx` = its chatHistory index so the
 *  roll-back button (需求4) targets the right node. */
function appendUserMsg(text: string, idx?: number) {
  const log = ensureChatLog();
  const div = doc("div");
  div.className = "chat-msg-user";
  if (idx !== undefined) div.dataset.idx = String(idx);
  div.textContent = text;
  log.appendChild(div);
  attachMsgActions(div, idx);
  resultsList.scrollTop = resultsList.scrollHeight;
}

async function startAIChat(query: string, files?: string[]) {
  const fileArr = files && files.length > 0 ? files : attachedFiles;
  const hasAttach = fileArr.length > 0;
  // Allow file-only queries (no text) — files alone are a valid AI request.
  // Also allow empty query: this opens the AI chat window for the user to type.
  if (isStreaming) return;

  const userText = query.trim() || "";

  // Empty query with no files: just show the AI chat UI, don't send a request.
  // Restore the most recent session when one exists, so re-opening the AI
  // chat continues the last conversation instead of starting blank.
  if (!userText && !hasAttach) {
    const sessions = await loadSessions();
    if (sessions.length > 0) {
      restoreSession(sessions[0]);
      return;
    }
    pluginActive = true;
    activePluginId = "ai-agent";
    isChatHistoryView = false;
    setPluginBar(t("chat.ai_entry"));
    invoke("set_plugin_active", { active: true }).catch(() => {});
    resultsContainer.classList.remove("hidden");
    searchBar.classList.add("has-results");
    resultsList.innerHTML = `<div class="ai-response" id="chat-log"><div class="chat-msg-system">${t("chat.ai_chat_ready", { mode: currentModeLabel() })}</div></div>`;
    statusText.textContent = t("status.ai", { mode: currentModeLabel() });
    chatInput.focus();
    applyWindowSize();
    return;
  }
  let finalQuery = userText;
  if (hasAttach) {
    const fileList = fileArr.map(f => `- ${f}`).join("\n");
    finalQuery = `[Attached files]\n${fileList}\n\n[User query]\n${userText}`;
    clearFileChips();
  }

  // Point 6/17: entering from the SEARCH bar with content (AI not active yet)
  // starts a NEW conversation — archive the previous one first so it is
  // reliably saved. When the user continues typing inside an active AI chat
  // (chatInput → sendChatMessage → startAIChat), pluginActive is true and the
  // current session must NOT be reset.
  if (!pluginActive) {
    if (isStreaming) {
      streamId++;
      isStreaming = false;
      setStreamingUI(false);
      cliTextCallback = null;
      cliDoneCallback = null;
      agentView = null;
      agentTurn = null;
    }
    await saveCurrentSession().catch(() => {});
    currentSessionId = null;  // fresh session id on next save
    chatHistory = [];
    document.getElementById("chat-log")?.remove();
  }

  // User bubble — original text + attached file names
  appendUserMsg(hasAttach
    ? `${userText}\n📎 ${fileArr.map(f => f.split(/[\\/]/).pop()).join(", ")}`
    : userText,
    chatHistory.length);

  // Agent mode → use CLI subprocess (finalQuery already carries file info).
  // Simple mode (built-in chat.rs) was removed — see ai-spec §11 changelog.
  await startAgentChat(finalQuery);
}

// Make startAIChat accessible from plugins (ai-agent.ts)
(window as any).__lunac_start_ai_chat = startAIChat;
(window as any).__lunac_show_chat_history = showChatHistory;
// 让 agent.exe 重新读盘：技能目录等只在启动时扫描一次，
// 设置类改动（技能增删改 / 工具黑名单）之后都要走这一步。
// 先存会话再 stop → start，避免重启丢掉当前对话（前端自持历史）。
(window as any).__lunac_reload_agent = async () => {
  try { await saveCurrentSession(); } catch (e) { console.warn("[lunac] save history before restart:", e); }
  try { await invoke("stop_cli"); } catch {}
  cliReady = false;
  try { await invoke("start_cli"); return "ok"; } catch (e) { return "error: " + String(e); }
};
// 工具黑名单保存流程（第19点）：按用户要求顺序 —
// 1) localStorage 持久化用户自定义清单
// 2) 交给 Rust 合并默认黑名单（set_tool_blacklist）
// 3) 重启 agent.exe（新 --disallowedTools 生效，重启前先存会话）
(window as any).__lunac_save_tool_blacklist = async (custom: string[]) => {
  try { localStorage.setItem("lunac-tool-blacklist", JSON.stringify(custom)); } catch {}
  try { await invoke("set_tool_blacklist", { blacklist: custom }); } catch (e) { console.warn("[lunac] set_tool_blacklist:", e); }
  return (window as any).__lunac_reload_agent();
};
// Open tool editor plugin from settings panel
(window as any).__lunac_execute_tool_editor = async () => {
  if (pluginActive) { await closePluginView(); await new Promise(r => setTimeout(r, 50)); }
  const te = pluginRegistry.getAll().find(p => p.id === "tool-editor");
  if (te) { resultsContainer.classList.remove("hidden"); searchBar.classList.add("has-results"); await executePlugin(te); }
};

// ── Runtime language application ──────────────────────────────────
// Re-applies t() to static UI elements (placeholders + tooltips from
// index.html). Called on startup and whenever the language changes.
function applyI18nToStaticUI() {
  const setTitle = (id: string, key: string) => {
    const el = document.getElementById(id);
    if (el) el.setAttribute("title", t(key));
  };
  const searchInputEl = document.getElementById("search-input") as HTMLInputElement | null;
  if (searchInputEl) searchInputEl.placeholder = t("search.placeholder");
  const chatInputEl = document.getElementById("chat-input") as HTMLTextAreaElement | null;
  if (chatInputEl) chatInputEl.placeholder = t("chat.placeholder");
  setTitle("settings-btn", "tooltip.settings");
  setTitle("plugin-bar-exit", "tooltip.exit_plugin");
  setTitle("chat-add-file-btn", "tooltip.add_file");
  setTitle("chat-new-btn", "tooltip.new_chat");
  setTitle("chat-history-btn", "tooltip.history");
  setTitle("chat-send-btn", "tooltip.send");
  setTitle("chat-stop-btn", "tooltip.stop");
  setTitle("chat-drawer-close", "tooltip.close_history");
  setTitle("detached-back-btn", "tooltip.restore");
  setTitle("detached-vscode-btn", "tooltip.vscode");
  setTitle("detached-close-btn", "tooltip.close_plugin");
  renderChatModeBtn();
  renderWorkspaceMenuLabels();
  renderToolsBlacklistLabels();
  refreshWorkspaceUI();

  // Drawer header (static text in index.html)
  const drawerHeader = document.querySelector("#chat-drawer-header > span");
  if (drawerHeader) drawerHeader.textContent = t("chat.drawer_title");
}

// Re-execute a builtin plugin by id (used to re-render after language change).
(window as any).__lunac_refresh_plugin = async (id: string) => {
  const p = pluginRegistry.getAll().find(pl => pl.id === id);
  if (!p) return;
  if (!pluginActive) {
    resultsContainer.classList.remove("hidden");
    searchBar.classList.add("has-results");
  }
  await executePlugin(p);
};

// The settings plugin dispatches "lunac-reload-settings" after installing a
// tool from URL/community, but no listener existed → the tool list stayed
// stale. Re-execute the settings plugin when this fires.
document.addEventListener("lunac-reload-settings", () => {
  if (pluginActive && activePluginId === "settings") {
    const p = pluginRegistry.getAll().find(pl => pl.id === "settings");
    if (p) executePlugin(p);
  }
});

// Refresh all static UI strings in the new language.
(window as any).__lunac_apply_language = () => {
  applyI18nToStaticUI();
  if (!isStreaming) statusText.textContent = t("status.ready");
};

// ── Helpers ──────────────────────────────────────────────────────
function doc(tag: string) { return document.createElement(tag); }
function esc(s: string) {
  if (!s) return s;
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;")
          .replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** 「查看图片」按钮 SVG —— 同印象派主题（残影 + 相框 + 点彩）。 */
const IMAGE_VIEW_SVG = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
  <g opacity="0.28" transform="translate(-0.6,-0.6)" stroke-width="1.6">
    <rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/>
  </g>
  <rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/>
  <circle cx="7.5" cy="7.5" r="0.9" fill="currentColor" stroke="none"/>
  <circle cx="18" cy="18" r="0.4" fill="currentColor" stroke="none" opacity="0.45"/>
</svg>`;

/** Escape text but render markdown `![alt](path)` images as "查看图片" buttons
 *  (需求5: 不再内联渲染 <img>，改为按钮 + 独立全窗查看界面)。本地路径经
 *  convertFileSrc (asset protocol)，因 WebView2 的 CSP img-src 无 file: scheme。
 *  按钮点击由 document 级事件委托处理（openImageViewer）。 */
function mdImagesToHtml(raw: string): string {
  if (!raw) return "";
  const seg = raw.split(/!\[([^\]]*)\]\(([^)]+)\)/g);
  let html = "";
  for (let i = 0; i < seg.length; i++) {
    if (i % 3 === 1) {
      const alt = seg[i] || "";
      const src = seg[i + 1] || "";
      i++;
      const url = /^(https?:|data:)/i.test(src) ? src : convertFileSrc(src);
      const label = alt || t("chat.view_image");
      html += `<button class="agent-md-view" data-src="${esc(url)}" title="${esc(label)}">${IMAGE_VIEW_SVG}<span>${esc(label)}</span></button>`;
    } else {
      html += esc(seg[i]);
    }
  }
  return html;
}

// ── 独立图片查看界面（全窗遮罩，点击遮罩关闭）────────────────────
function openImageViewer(url: string) {
  let viewer = document.getElementById("image-viewer");
  if (!viewer) {
    viewer = doc("div");
    viewer.id = "image-viewer";
    viewer.innerHTML = `<img class="image-viewer-img" alt="">`;
    document.body.appendChild(viewer);
  }
  const img = viewer.querySelector(".image-viewer-img") as HTMLImageElement | null;
  if (img) img.src = url;
  viewer.classList.add("visible");
}

function closeImageViewer() {
  document.getElementById("image-viewer")?.classList.remove("visible");
}

// 事件委托：查看图片按钮（历史渲染/实时流式均经 innerHTML 注入）
document.addEventListener("click", (e) => {
  const tgt = e.target as HTMLElement;
  const viewBtn = tgt.closest<HTMLElement>(".agent-md-view");
  if (viewBtn?.dataset.src) {
    openImageViewer(viewBtn.dataset.src);
    return;
  }
  const viewer = document.getElementById("image-viewer");
  if (viewer?.classList.contains("visible") && tgt === viewer) closeImageViewer();
});

// ── 对话内容只读保护（需求3）────────────────────────────────────
// 用户只能选中/复制对话文本，不能粘贴、剪切或篡改 AI 生成/用户发送的
// 历史内容。输入框 (#chat-input) 等可编辑区域不在拦截范围内。
function isChatContentTarget(t: EventTarget | null): boolean {
  if (!(t instanceof HTMLElement)) return false;
  return !!t.closest(".ai-response, .agent-flow, #chat-log, #chat-drawer");
}
document.addEventListener("paste", (e) => {
  if (isChatContentTarget(e.target)) e.preventDefault();
});
document.addEventListener("cut", (e) => {
  if (isChatContentTarget(e.target)) e.preventDefault();
});

// ── 结果区自定义右键菜单 ────────────────────────────────────────────
// 全局 contextmenu 已被拦截，此处为 #results-container 提供自定义菜单，
// 根据点击目标（App 条目 / 插件条目 / AI 回退 / 选中文本）动态生成选项。

const contextMenuEl = doc("div");
contextMenuEl.id = "context-menu";
contextMenuEl.className = "hidden";
document.body.appendChild(contextMenuEl);

function showContextMenu(x: number, y: number, items: Array<{ label: string; action: () => void; danger?: boolean }>) {
  // 互斥：打开结果菜单前先移除 ws 引擎菜单，避免两菜单同帧同时显示
  document.getElementById("ws-context-menu")?.remove();
  contextMenuEl.innerHTML = items.map(item =>
    `<div class="context-menu-item${item.danger ? " danger" : ""}">${esc(item.label)}</div>`
  ).join("");

  // CSS zoom 缩放 <html> 后，position:fixed 的元素也会被等比放大，而事件
  // clientX/Y 与 getBoundingClientRect 返回的是未缩放布局坐标（Chromium
  // 特性）→ 定位需除以 currentZoom 才能在鼠标指针处正确显示。
  const z = currentZoom || 1;
  contextMenuEl.style.left = `${x / z}px`;
  contextMenuEl.style.top = `${y / z}px`;
  contextMenuEl.classList.remove("hidden");

  const menuItems = contextMenuEl.querySelectorAll(".context-menu-item");
  menuItems.forEach((el, i) => {
    el.addEventListener("click", () => {
      items[i]?.action();
      hideContextMenu();
    });
  });

  // 如果菜单溢出视口边缘则翻转位置（视口尺寸同样按 zoom 换算）
  requestAnimationFrame(() => {
    const rect = contextMenuEl.getBoundingClientRect();
    const vw = window.innerWidth / z;
    const vh = window.innerHeight / z;
    if (rect.right > vw) contextMenuEl.style.left = `${vw - rect.width - 8}px`;
    if (rect.bottom > vh) contextMenuEl.style.top = `${vh - rect.height - 8}px`;
  });
}

function hideContextMenu() {
  contextMenuEl.classList.add("hidden");
}

document.addEventListener("click", () => hideContextMenu());

resultsContainer.addEventListener("contextmenu", (e) => {
  e.preventDefault();   // 阻止 WebView2 默认右键菜单（与全局互补）
  e.stopPropagation();

  const target = e.target as HTMLElement;
  const resultItem = target.closest(".result-item") as HTMLElement | null;

  const items: Array<{ label: string; action: () => void; danger?: boolean }> = [];

  if (resultItem) {
    // 类别判定基于渲染时写入的 data-kind（与 recency 排序后的 DOM 顺序天然一致），
    // 不再依赖 currentApps/currentResults 的旧索引——排序后索引错位是右键菜单
    // 误判（ws-fallback 被当成 App、双菜单重叠）的总根因。
    const kind = resultItem.dataset.kind;

    if (kind === "app") {
      const path = resultItem.dataset.appPath;
      const name = resultItem.dataset.appName || "";
      if (path) {
        items.push({ label: t("context.open", { name }), action: () => launchApp(path) });
        items.push({
          label: t("context.open_location"),
          action: () => {
            const dir = path.replace(/[\\/][^\\/]+$/, "");
            invoke("launch_app", { path: dir }).catch(() => {});
          },
        });
      }
    } else if (kind === "plugin") {
      const id = resultItem.dataset.pluginId;
      const plugin = pluginRegistry.getAll().find(p => p.id === id);
      if (plugin) {
        items.push({ label: t("context.run", { name: plugin.name }), action: () => executePlugin(plugin) });
      }
    } else if (kind === "ai-fallback" || kind === "ocr" || kind === "quick-launch") {
      const q = searchInput.value.trim();
      if (q) {
        items.push({ label: t("context.ask_ai"), action: () => startAIChat(q) });
      }
    } else if (kind === "memo-fallback") {
      const memoPlugin = pluginRegistry.getAll().find(p => p.id === "memo");
      if (memoPlugin) {
        items.push({ label: t("context.run", { name: memoPlugin.name }), action: () => executePlugin(memoPlugin) });
      }
    }
    // kind === "ws-fallback" 由 wsItem 自身的引擎切换菜单处理（已 stopPropagation）
  }

  // 有选中文本时始终提供复制选项
  const selection = window.getSelection()?.toString().trim();
  if (selection) {
    items.push({ label: t("context.copy"), action: async () => {
      const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
      writeText(selection).catch(() => {});
    } });
  }

  if (items.length > 0) {
    showContextMenu(e.clientX, e.clientY, items);
  }
});

// ── Ready ────────────────────────────────────────────────────────
// 热键提示延迟到此刷新：确保 i18n（initI18n/loadSavedLanguage）已就绪，
// 语言与当前实际热键都取到最新值。
refreshHotkeyHint();

// ── 自定义背景（需求：透明 + 毛玻璃，无默认图）──────────────────
// 图层 #app-bg-image 已嵌入 #results-container 内部（absolute 贴合结果区/
// 插件界面，随面板大小缩放，不随窗口 fixed 导致脱离）；透明由 CSS opacity
// 控制，圆角由容器 overflow:hidden 裁剪，毛玻璃由面板 backdrop-filter 承担。
// 设置值存 localStorage("lunac-bg-image")，settings 插件写入后调用
// 全局 __lunac_apply_bg 即时生效；启动时恢复上次设置。
function applyBgImage(url: string | null) {
  const bg = document.getElementById("app-bg-image");
  if (!bg) return;
  if (url) {
    bg.style.backgroundImage = `url("${url.replace(/"/g, '\\"')}")`;
  } else {
    bg.style.backgroundImage = "";
  }
}
(window as any).__lunac_apply_bg = applyBgImage;
try {
  applyBgImage(localStorage.getItem("lunac-bg-image"));
} catch {}

// ── Drag & drop files onto search bar / chat bar ────────────────

function bindFileDrop(zone: HTMLElement) {
  let counter = 0;
  zone.addEventListener("dragenter", (e) => {
    e.preventDefault();
    counter++;
    zone.classList.add("drag-over");
  });
  zone.addEventListener("dragleave", () => {
    counter--;
    if (counter === 0) zone.classList.remove("drag-over");
  });
  zone.addEventListener("dragover", (e) => {
    e.preventDefault();
    e.dataTransfer!.dropEffect = "copy";
  });
  zone.addEventListener("drop", (e) => {
    e.preventDefault();
    counter = 0;
    zone.classList.remove("drag-over");
    const files = e.dataTransfer?.files;
    if (files && files.length > 0) {
      for (let i = 0; i < files.length; i++) {
        // Tauri drops expose the full path via a special property.
        // 只接受真实绝对路径；拿不到 path（虚拟路径/格式不支持）时跳过。
        const f = files[i] as any;
        const filePath: string = f.path || "";
        if (filePath && /^[A-Za-z]:[\\/]/.test(filePath)) addFileChip(filePath);
      }
    }
  });
}

bindFileDrop(searchBar);
bindFileDrop(chatInputBar);

// ── 全局文件拖放兜底 ──────────────────────────────────────────
// 说明：失效不是“格式不支持”——Tauri 拖放会把任意格式文件的真实路径放进
// File.path。真正问题是：(1) 上面只给 searchBar/chatInputBar 绑了处理；
// (2) 在其它区域 drop 时 WebView2 会把事件当作“打开/导航”吞掉（即“被网页拦截”），
// 即使文件被放进来也没有任何处理。
// 这里做 document 级兜底：始终 preventDefault（阻止导航/打开文件），
// 在主界面（非插件面板、非已绑区域）落盘文件时统一加入文件气泡。
function chipsFromDropFiles(files: FileList): string[] {
  const out: string[] = [];
  for (let i = 0; i < files.length; i++) {
    const f = files[i] as any;
    const p: string = f.path || "";
    // Tauri 拖放应带真实路径；仅能拿到 name（虚拟路径）时跳过，避免误加不可用项
    if (p && /^[A-Za-z]:[\\/]/.test(p)) out.push(p);
  }
  return out;
}
document.addEventListener("dragover", (e) => {
  e.preventDefault();
});
document.addEventListener("drop", (e) => {
  // 始终阻止默认（防 WebView2 打开/导航该文件）
  e.preventDefault();
  // 插件面板打开时不处理（避免破坏插件内部交互）；已绑区域自己处理
  const target = e.target as HTMLElement | null;
  if (pluginActive) return;
  if (target && target.closest("#search-bar, #chat-input-bar, #chat-input")) return;
  const files = e.dataTransfer?.files;
  if (!files || files.length === 0) return;
  const paths = chipsFromDropFiles(files);
  for (const p of paths) addFileChip(p);
});

// ── 原生文件拖放（拿真实路径，WebView2 下 HTML5 File.path 常为空）────
// 桌面/资源管理器把任意文件拖入窗口时，走 Tauri 原生 onDragDropEvent，
// 直接获得真实绝对路径，避免“拖了没反应 / 加了无效假路径”。
// 允许落盘：主界面（非插件）或 AI 助手对话界面。
async function bindNativeFileDrop() {
  try {
    const wv = getCurrentWebview();
    await wv.onDragDropEvent((event) => {
      const payload = event.payload as { type: string; paths?: string[] };
      if (payload.type !== "drop" || !Array.isArray(payload.paths)) return;
      const allow = !pluginActive || activePluginId === "ai-agent";
      if (!allow) return;
      for (const p of payload.paths) {
        if (p && /^[A-Za-z]:[\\/]/.test(p)) addFileChip(p);
      }
    });
  } catch (e) {
    console.warn("[lunac] native drag&drop unavailable:", e);
  }
}
bindNativeFileDrop();

// ── Paste files / paths from clipboard ─────────────────────────

async function handleFilePaste(e: ClipboardEvent) {
  const items = e.clipboardData?.items;
  const text = e.clipboardData?.getData("text/plain") || "";
  const trimmed = text.trim();
  const isWinPath = trimmed && /^[A-Za-z]:[\\/]/.test(trimmed) && trimmed.length < 500;

  // WebView2/Chromium 里“粘贴的截图/位图/网页图片”也是 kind==="file"、type 为
  // image/* 的 DataTransferItem（点4 根因：旧代码把它们当成“资源管理器文件”走
  // CF_HDROP，而位图没有 CF_HDROP，导致什么都不加）。这里先统一收集：
  //   hasFileItem  → 任何文件/图片型项
  //   imageBlobs   → 其中 type 是 image/* 的（截图/浏览器图片）
  const imageBlobs: Blob[] = [];
  let hasFileItem = false;
  if (items) {
    for (let i = 0; i < items.length; i++) {
      const it = items[i];
      if (it.kind !== "file") continue;
      hasFileItem = true;
      const f = it.getAsFile();
      if (f && it.type.startsWith("image/")) imageBlobs.push(f);
    }
  }

  // 文件/图片/路径文本一律拦截默认粘贴（纯文本不受影响，照常插入）
  if (hasFileItem || isWinPath) e.preventDefault();

  // 纯文本粘贴（非路径、无文件项）→ 直接走系统默认，不做任何读取
  if (!hasFileItem && !isWinPath && trimmed) return;

  // ── 1. 资源管理器复制的真实文件（CF_HDROP）→ 优先用真实路径 ──
  try {
    const paths: string[] = await invoke("read_clipboard_file_paths");
    if (paths && paths.length > 0) {
      for (const p of paths) addFileChip(p);
      return;
    }
  } catch { /* 剪贴板没有 CF_HDROP */ }

  // ── 2. 事件内暴露的图片数据（截图 / Win+Shift+S / 网页复制图片）──
  if (imageBlobs.length > 0) {
    const readAsDataUrl = (blob: Blob) =>
      new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(reader.result as string);
        reader.onerror = () => reject(reader.error);
        reader.readAsDataURL(blob);
      });
    for (const blob of imageBlobs) {
      try {
        const dataUrl = await readAsDataUrl(blob);
        const tempPath = await invoke<string>("save_temp_image", { dataUrl });
        addFileChip(tempPath);
      } catch { /* 单个图片失败不影响其余 */ }
    }
    return;
  }

  // ── 3. 剪贴板文本是一个 Windows 路径（“复制为路径”）──
  if (isWinPath) {
    addFileChip(trimmed);
    return;
  }

  // ── 4. 原生兜底：纯位图（CF_DIB/截图）连 DataTransferItem 都读不到时 ──
  try {
    const result: string = await invoke("read_clipboard_backup_image");
    if (result) {
      const sepIdx = result.lastIndexOf("|");
      const backedPath = sepIdx > 0 ? result.substring(0, sepIdx) : result;
      if (backedPath) addFileChip(backedPath);
    }
  } catch { /* 剪贴板无位图 */ }
}

searchInput.addEventListener("paste", handleFilePaste);
chatInput.addEventListener("paste", handleFilePaste);

// ── Auto-detect clipboard on window show ───────────────────────
// Primary: Rust reads clipboard on main thread and emits lunac-clipboard with text.
// Fallback: JS reads via Tauri clipboard plugin when lunac-window-shown fires.
// Debounce to prevent duplicate reads.

let clipReadTimer: ReturnType<typeof setTimeout> | null = null;
let lastAutoFillClipText = ""; // dedupe identical clipboard reads (normalized)

function processClipboardText(text: string) {
  if (!text || text.length > 4000) return;
  // Skip clipboard auto-fill when AI chat is active — search bar is disabled
  // and status bar is used for token dashboard instead.
  if (pluginActive && activePluginId === "ai-agent") return;

  // Normalize: Rust side trims clipboard text before emitting; JS fallback
  // readText() does not. Without normalization the dedup check fails and
  // the same text gets auto-filled twice.
  const normalized = text.replace(/\r\n/g, '\n').trim();
  if (!normalized) return;

  // Dedupe: skip if identical to the last auto-filled clipboard content.
  if (normalized === lastAutoFillClipText) return;
  lastAutoFillClipText = normalized;

  // Split by newline — may contain both text and file paths
  const parts = normalized.split('\n').filter(p => p.trim());
  let hasText = false;
  let hasFiles = false;

  for (const part of parts) {
    const trimmed = part.trim();
    if (!trimmed) continue;
    // Windows 路径 → 添加为泡泡框 + 剪贴板历史
    if (/^[A-Za-z]:[\\/]/.test(trimmed)) {
      if (!attachedFiles.includes(trimmed)) {
        addFileChip(trimmed);
        hasFiles = true;
      }
      import("./plugins/builtin/clipboard-history").then(m => {
        m.addClipboardEntry(trimmed);
      }).catch(() => {});
    } else {
      // 普通文本 → 存入剪切板历史
      if (trimmed.length < 2000) {
        import("./plugins/builtin/clipboard-history").then(m => {
          m.addClipboardEntry(trimmed);
        }).catch(() => {});
      }
      hasText = true;
    }
  }

  // 状态栏预览
  const first = parts[0].trim();
  const preview = first.length > 50 ? first.slice(0, 50) + "…" : first;
  statusHint.textContent = `📋 ${preview.replace(/\n/g, " ")}`;

  // 触发搜索：文本填搜索栏，路径已有泡泡框
  // Skip auto-fill if the search bar already contains the same text
  if (!pluginActive) {
    if (hasText && !hasFiles) {
      const firstPart = parts[0].trim();
      if (firstPart === searchInput.value.trim()) {
        return; // duplicate — already in search bar, skip
      }
      searchInput.value = firstPart;
    }
    searchInput.dispatchEvent(new Event("input", { bubbles: true }));
    searchInput.focus();
  }
}

// Primary: Rust has already read clipboard, text arrives directly via event
win.listen<string>("lunac-clipboard", (event) => {
  if (event.payload) {
    processClipboardText(event.payload);
  }
});

// Auto-detect clipboard content when window is shown.
// Uses native Win32 FFI only (CF_HDROP + CF_DIB/CF_DIBV5) — avoids arboard
// which crashes with STATUS_HEAP_CORRUPTION on BI_BITFIELDS-compressed DIB
// placed by ShareX and other screenshot tools.
// NOTE: text clipboard detection is handled separately via lunac-clipboard event.
let lastAutoImageFingerprint = "";

function triggerJSClipboardRead() {
  if (clipReadTimer) clearTimeout(clipReadTimer);
  clipReadTimer = setTimeout(async () => {
    clipReadTimer = null;

    // Stage 1: CF_HDROP file paths (Explorer copy)
    try {
      const files: string[] = await invoke("read_clipboard_files");
      if (files.length > 0) {
        (window as any).__lunac_clipboard_has_image = true;
        if (!searchInput.value.trim() && attachedFiles.length === 0) {
          addFileChip(files[0]);
        }
        refreshSearchResults();
        return;
      }
    } catch { /* no files on clipboard */ }

    // Stage 2: Native DIB → BMP (ShareX and other screenshot tools)
    // Returns "path|fingerprint" — fingerprint is used to dedupe repeated clipboard reads
    try {
      const result: string = await invoke("read_clipboard_backup_image");
      if (result) {
        const sepIdx = result.lastIndexOf("|");
        const backedPath = sepIdx > 0 ? result.substring(0, sepIdx) : result;
        const fingerprint = sepIdx > 0 ? result.substring(sepIdx + 1) : "";

        // De-dupe: skip if same clipboard image was already auto-detected
        if (fingerprint && fingerprint === lastAutoImageFingerprint) return;
        lastAutoImageFingerprint = fingerprint;

        (window as any).__lunac_clipboard_has_image = true;
        if (!searchInput.value.trim() && attachedFiles.length === 0) {
          addFileChip(backedPath);
        }
        refreshSearchResults();
        return;
      }
    } catch { /* no image on clipboard */ }

    (window as any).__lunac_clipboard_has_image = false;
  }, 100);
}

// Fallback: use Tauri clipboard plugin when window is shown by hotkey toggle.
// NOTE: do NOT listen on tauri://focus — dragging the window triggers focus
// and would cause unwanted clipboard reads.
// 同时进入「抑制滑动」期：唤出瞬间窗口应完整展开，而不是被结果区逐帧撑开。
win.listen("lunac-window-shown", () => {
  suppressResizeAnimBriefly();
  triggerJSClipboardRead();
});

// Also try reading clipboard on initial startup
setTimeout(triggerJSClipboardRead, 800);
