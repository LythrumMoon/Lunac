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
import { initI18n, loadSavedLanguage, t, pluginName, pluginDesc, lang } from "./i18n.js";

// ── 全局错误上报 ─────────────────────────────────────────────────
// release 下前端没有控制台、用户平时也不会开 DevTools，未捕获的错误必须送到
// Rust 侧落盘（<exe 根>\temp\logs\lunac-YYYY-MM-DD.log），否则「用户看到报错、
// 我们什么都查不到」。同一错误去重，避免一个每帧都抛的错误把日志刷爆。
const _reportedFrontendErrors = new Set<string>();
function reportFrontendError(level: "error" | "warn", message: string): void {
  const text = message.slice(0, 4000);
  const key = `${level}:${text}`.slice(0, 400);
  if (_reportedFrontendErrors.has(key)) return;
  _reportedFrontendErrors.add(key);
  invoke("log_frontend", { level, message: text }).catch(() => {});
}
window.addEventListener("error", (e) => {
  const where = e.filename ? ` @ ${e.filename}:${e.lineno}:${e.colno}` : "";
  reportFrontendError("error", `${e.message}${where}`);
});
window.addEventListener("unhandledrejection", (e) => {
  const reason = (e as PromiseRejectionEvent).reason;
  const detail =
    reason instanceof Error ? `${reason.name}: ${reason.message}\n${reason.stack ?? ""}` : String(reason);
  reportFrontendError("error", `unhandled rejection: ${detail}`);
});

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
const chatMoreBtn = el("chat-more-btn");
const chatMoreMenu = el("chat-more-menu");
const chatMoreThinkIc = el("chat-more-think-ic");
const chatMoreThinkLabel = el("chat-more-think-label");
const chatMoreWorkspaceLabel = el("chat-more-workspace-label");
const chatModeSeg = el("chat-mode-seg");
const chatRunmodeBtn = el("chat-runmode-btn");
const chatRunmodeHint = el("chat-runmode-hint");
const chatWorkspacePath = el("chat-workspace-path");
const chatWorkspaceSelect = el("chat-workspace-select");
const chatWorkspaceReset = el("chat-workspace-reset");
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

// 详细搜索大界面（双击搜索栏进入）—— 这两个必须声明在这里：applyWindowSize()
// 在模块初始化阶段就会被调用，声明晚于它 = TDZ 直接抛。其余详情态状态在文件末尾
// 那一整段里（见「详细搜索大界面」章节）。
const DETAIL_HEIGHT = 640;   // 固定高度档（设计 px × zoom = DIPs）
let detailOpen = false;      // 是否处于详细搜索大界面

/** 把「当前界面层」同步给 Rust（Esc 的隐藏判据，见 hotkey.rs 的 `UI_MODE`）。
 *
 *  界面的真相源在 WebView（`pluginActive` / `detailOpen`），而判据在 Rust 进程 ——
 *  两边任何一次漏同步都会让 Rust 停在非 main 层，表现为「简洁界面里按 Esc 不隐藏
 *  窗口，控制台一直打 `Esc(poll): ui_mode=2 (non-main)`」；WebView 一旦重载
 *  （DevTools / 前端刷新），WebView 侧状态归零、Rust 侧还留着旧值，也会错位。
 *
 *  所以：① 由这里**从两个状态位推导**，新增转场一律只调本函数，不要再写字面量；
 *  ② 挂在 `applyWindowSize()` 末尾 —— 那个函数本来就按「插件 / 详情 / 简洁」分支，
 *  是「层」的天然汇聚点，任何忘记同步的转场都会在下一帧自愈，模块初始化时那次
 *  调用还会把启动状态（简洁搜索 = main）无条件重报一次。
 *  ③ **刻意不做「值没变就不发」的去重**：那要求所有发送点都经过同一份缓存，
 *  一旦别处还留着裸 invoke，缓存就会与实际值脱节、反向压住一次必要的同步
 *  （例如关掉插件后缓存仍是 plugin，再进插件时这一发被吞 → Rust 停在 main →
 *  Esc 在插件里变成隐藏窗口）。一次 bool 级 IPC 而已，去重的收益不值这个风险。 */
function syncUiMode() {
  const mode = detailOpen ? "detail" : pluginActive ? "plugin" : "main";
  invoke("set_ui_mode", { mode }).catch(() => {});
}

// ── 会话历史回灌（agent 上下文 ↔ 前端 chatHistory）──────────────────
// 背景：agent 的对话上下文**完全自持在 agent 进程内**（stream-json 的 stdin/stdout），
// 前端 `chatHistory` 只用于显示。旧实现在「回退历史」时用 `stop_cli` + `start_cli`
// 清空 agent 上下文（当时的注释是 so the next turn doesn't see the removed messages），
// 代价却是**保留下来的上文也一起丢了**，用户表现为「回退后引用不到上文」；
// 「从磁盘恢复旧会话」同样只重建了 DOM，agent 侧一直是零上文。
// 这里补上那座桥：把前端这份历史整体灌给 agent（协议 `{"type":"set_history"}`，
// agent 只替换自己的 history、**不触发模型调用**，所以不花钱也不产生回答）。
//
// 为什么是 pending 队列而不是直接发：`cliReady` 由 `cli-status:stdout` 事件置位，而
// 回退 / 恢复都可能发生在 agent 还没起来（懒启动）或刚重启完的瞬间 —— 那时直接发会被
// 「CLI 未就绪」吞掉。挂起后等 `cliReady` 置位时冲刷，且**必须先灌历史再放行挂起的
// 提问**：agent 是单线程顺序吃 stdin 的，顺序反了这一问仍然是零上文。
let pendingAgentHistory: { role: string; content: string }[] | null = null;

/** 历史只保留用户 / 助手的**纯文本** —— 工具调用细节本来就不落前端，这是有意的近似。 */
async function sendAgentHistory(messages: { role: string; content: string }[]) {
  const payload = JSON.stringify({
    type: "set_history",
    messages: messages.map(m => ({
      role: m.role === "assistant" ? "assistant" : "user",
      content: cleanUserContent(m.content ?? ""),
    })),
  });
  await invoke("send_message", { message: payload });
}

/** 把历史同步给 agent；CLI 未就绪时挂起，等 `cli-status:stdout` 再发。 */
function queueAgentHistory(messages: { role: string; content: string }[]) {
  if (!cliReady) {
    pendingAgentHistory = messages.map(m => ({ ...m }));
    return;
  }
  void sendAgentHistory(messages).catch(() => {});
}

/** CLI 刚就绪时冲刷挂起的历史。返回 Promise，调用方据此排在「放行提问」之前。 */
async function flushPendingAgentHistory() {
  if (!pendingAgentHistory) return;
  const msgs = pendingAgentHistory;
  pendingAgentHistory = null;
  try { await sendAgentHistory(msgs); } catch {}
}

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

/** 唤出窗口时**强制**再断言一次高度（2026-09-19 新增）。
 *
 *  `requestWindowHeight` 有两条会「跳过 setSize」的早退：`h === requestedHeight`（缓存命中）
 *  与 `|h − currentWindowHeight| < 3`（认为窗口已经是内容高）。这两条在键入时是对的
 *  （防 IPC 风暴、防 setSize→onResized 回环），但**唤出时是错的**：
 *  窗口在隐藏期间的真实高度可能已经被 OS / 上一次会话改动过，而 `requestedHeight`
 *  还留着旧值 ⇒ 量出来的 h 与缓存一致 ⇒ 一声不响地不下发 ⇒ 窗口停在旧高度
 *  （用户看到的就是「呼出后高度不对，过一会才自己恢复」）。
 *  所以唤出路径先把 `requestedHeight` 复位成 -1，让这一轮测量必定落地。 */
function forceHeightReassert() {
  requestedHeight = -1;
  bootHeightSettled = true;  // 启动首屏已过；suppress 窗口内本来就走直设
  scheduleSearchResize();
}

/** setSize 串行下发（latest-wins）：在途期间新期望高度只记 pendingHeight，
 *  本次完成后若与已发高度不同再补发一次 —— 杜绝快速键入时 setSize IPC 排队堆积。 */
function requestWindowHeight(h: number) {
  // 动画在途且仍处搜索态：只更新动画目标（latest-wins），由动画逐帧收敛。
  // 插件态/详细搜索态请求不允许被动画劫持 —— 它们是离散跳变，应走下方直设路径。
  if (animActive && !pluginActive && !detailOpen) {
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
  if (bootHeightSettled && RESIZE_ANIM_CFG.enabled && !pluginActive && !detailOpen) {
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
    // 进入插件态/详细搜索态 → 停帧；它们的高度由各自流程（applyWindowSize）直设
    if (pluginActive || detailOpen) { stopResizeAnim(); return; }
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

/** 结果区可用的最大高度（DIP）—— 去掉固定 `max-height: 380px` 后**唯一**的闸门。
 *
 *  为什么必须由 JS 算、且必须取自 `screen.availHeight` 而不是 CSS 的 `100vh`：
 *  本窗口的高度是**内容驱动**的（`#app` 为 `height:auto`，量完再 `setSize`）。
 *  若用 `100vh` 当上限，就构成「窗口高 ← 内容高 ← vh ← 窗口高」的循环，
 *  实测会来回抖。`screen.availHeight` 只依赖显示器，与窗口自身无关，是干净的常量。
 *
 *  余量 `SEARCH_CHROME_PX` = 搜索栏（~52）+ 状态栏（~30）+ 一点不贴屏幕底边的余量。
 *  超上限时结果区内部滚动（`#results-list` 本来就是 `overflow-y: auto`）。 */
const SEARCH_CHROME_PX = 120;
function resultsMaxHeight(): number {
  const avail = window.screen?.availHeight || 1080;
  // 下限 120：屏幕极小时也别把结果区压成 0（那时宁可内部滚动）
  return Math.max(120, Math.round(avail - SEARCH_CHROME_PX));
}

/** 把算好的上限写到 `#results-container` 上（CSS 变量，见 styles.css 那段注释）。
 *  这里用行内 style 是刻意的例外：**值每块屏都不一样**，写进 class 或样式表做不到；
 *  且它只是喂给 CSS 的一个数字，不与任何 class 规则争优先级（不像 `overflow` 那样
 *  会被 `.hidden` / `.plugin-open` 之类的规则覆盖回去）。 */
function applyResultsMaxHeight() {
  const px = resultsMaxHeight();
  if (resultsContainer.style.getPropertyValue("--results-max-h") !== `${px}px`) {
    resultsContainer.style.setProperty("--results-max-h", `${px}px`);
  }
}

/** Apply window size based on current UI state.
 *  插件模式：固定高度（detached 600 / embedded 360 / OCR detached 520），
 *    设计 px × zoom = DIPs（插件面板按设计宽度 800 的 CSS px 排版）。
 *  详细搜索大界面：固定高度 DETAIL_HEIGHT（640），同一套设计 px × zoom 算法。
 *  搜索/结果/空态：实测内容高度（measurePanelHeight，已含 zoom），
 *    彻底取代 heightMap 预测，保证窗口与结果区渲染区域严格一致。
 *  宽度保持用户当前宽度（lastWindowWidth）。
 *  setSize 统一走 requestWindowHeight 串行化；搜索路径可改用 scheduleSearchResize
 *  懒化测量，避免快速键入时“同任务强制 layout + IPC 风暴”。 */
function applyWindowSize() {
  // 插件/分离/详细搜索模式 #app 撑满窗口（CSS height:100%）；搜索模式内容驱动（height:auto）
  document.getElementById("app")!.classList.toggle("plugin-active", pluginActive);

  // 结果区的屏幕高上限必须先写进去，再测量 —— 否则量到的是「没封顶」的高度。
  applyResultsMaxHeight();

  let h: number;
  if (pluginActive) {
    // OCR needs a wider two-panel layout in detached mode
    if (detached && activePluginId === "ocr") {
      h = Math.round(520 * currentZoom);
    } else {
      h = Math.round((detached ? 600 : 360) * currentZoom);
    }
  } else if (detailOpen) {
    // 详细搜索大界面：离散高度档，不进高度滑动动画（见下方 requestWindowHeight）
    h = Math.round(DETAIL_HEIGHT * currentZoom);
  } else {
    h = measurePanelHeight();
  }
  requestWindowHeight(h);
  // 界面层在这里重报（去重后几乎零成本）：本函数本来就按「插件 / 详情 / 简洁」
  // 分支，是「层」的天然汇聚点 —— 任何忘记调 syncUiMode() 的转场都会在下一帧自愈，
  // 且模块初始化时这一次调用会把启动状态（简洁搜索 = main）无条件告诉 Rust，
  // 修掉「WebView 重载后 Rust 还停在 detail、Esc 再也不隐藏窗口」这类错位。
  syncUiMode();
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
/** 能作为 `image` 块发给端点的图片类型（A8）。
 *
 *  刻意**比 OCR 用的 `imageExtensions` 窄** —— 端点只认 PNG / JPEG / GIF / WebP，
 *  BMP / TIFF / ICO 发过去必被拒，那些类型仍走老路（路径文本交给模型）。
 *  这里只管「值不值得走新路」，类型最终由 agent 按**魔术字节**复核。
 *  是否启用见设置面板「模型支持图片输入」（存 ai.json，**默认关**）。 */
const VISION_IMAGE_RE = /\.(png|jpe?g|gif|webp)$/i;
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
/** 单次 API 请求的用量明细（对账粒度，见 ai-spec §3.5「用量与对账」）。
 *  平台上「一次带工具的提问」就是多行，本地按提问只落一行 —— 这个数组把粒度补齐，
 *  才能逐行对齐。旧 agent 不报该字段 → 空数组。 */
interface ChatDoneRequestUsage {
  in: number;
  read: number;
  create: number;
  out: number;
}
interface ChatDoneInfo {
  stop_reason: string;
  input_tokens: number;
  output_tokens: number;
  cache_read_input_tokens?: number;
  cache_creation_input_tokens?: number;
  requests?: ChatDoneRequestUsage[];
}

// ── 用量统计（口径：**当前这次对话**）────────────────────────────
// agent.exe 的 result.usage 是**每次提问的绝对值**（一次提问内所有工具往返
// 在本轮内累加），不是会话累计 —— 所以这里不做差，直接累加。
// 表盘（命中率 / 总 token）只统计当前对话，新建或切换会话即归零；
// 按天累计另有一份落盘日志，见下面 usageTotals 的注释。
/** 表盘数值 = **当前这次对话**（新建会话 / 切到别的会话即归零）。
 *  注意与落盘口径的区别：`ModuleData\usage\usage-YYYY-MM-DD.jsonl` 仍按天累加，
 *  那是留给「与供应商平台对账」的，两者不要混为一谈（ai-spec §3.5）。 */
let usageTotals = { hit: 0, miss: 0, total: 0, elided: 0, dropped: 0 };
/** 当前 agent 的模型名（来自 system/init，写进用量日志便于区分供应商/模型） */
let agentModel = "";
/** 本次提问内 agent 报告的历史压缩次数（提问结束写进用量日志）。
 *  压缩会改写请求前缀 → 端点侧缓存作废，是命中率的**断裂型**失效来源；
 *  必须与「新内容天生没被上一轮缓存覆盖」的自然未命中分开看（ai-spec §11 规则 23）。 */
let liveCompaction = { elided: 0, dropped: 0 };

// ── Chat conversation mode ────────────────────────────────────────
// 思考开关（2026-09-15：原 fast/think/deep 三档收敛为两档）：
//   "off" = 不思考（thinking.type=disabled，直接回答）
//   "on"  = 思考（先推理再回答）
// 为什么不是「思考力度」：实测该端点没有这个旋钮 —— budget_tokens 不被
// enforce（给 1 与给 32768 思考量一样）、effort 字段被静默忽略，三档在
// 端点上本就退化成两态。两档共用同一 agent.exe 完整工具链，仅思考开关
// 不同；由 Rust 端 set_thinking_mode → start_cli_process 写入环境变量。
type ThinkingMode = "on" | "off";
let chatMode: ThinkingMode = "on";
try {
  const saved = localStorage.getItem("lunac-chat-mode");
  if (saved === "on" || saved === "off") chatMode = saved;
  // 旧值迁移：fast/simple = 原「不思考」档 → off；think/deep/agent = 原「思考」档 → on
  else if (saved === "fast" || saved === "simple") chatMode = "off";
  else if (saved === "think" || saved === "deep" || saved === "agent") chatMode = "on";
} catch { /* keep default */ }

/** 分段按钮上的短标签（开 / 关）。 */
const THINKING_LABEL: Record<ThinkingMode, string> = {
  on: "chat.mode_on",
  off: "chat.mode_off",
};

/** 状态栏与系统提示里的完整说法（「思考开」），避免出现「AI · 开」这种半句话。 */
const THINKING_STATUS_LABEL: Record<ThinkingMode, string> = {
  on: "chat.status_on",
  off: "chat.status_off",
};

/** 思考开关分段（「更多」菜单里的两档：开 / 关）。 */
function renderChatModeSeg() {
  if (!chatModeSeg) return;
  chatModeSeg.querySelectorAll<HTMLButtonElement>("button[data-mode]").forEach((b) => {
    const m = b.dataset.mode as ThinkingMode;
    b.textContent = t(THINKING_LABEL[m]);
    b.title = t("chat.mode_tooltip_" + m);
    b.classList.toggle("active", m === chatMode);
  });
}

/** 「更多」菜单的静态文案与行图标（语言切换时也要重刷）。 */
function renderMoreMenuLabels() {
  if (chatMoreBtn) {
    chatMoreBtn.title = t("chat.more_btn");
    chatMoreBtn.setAttribute("aria-label", t("chat.more_btn"));
  }
  // 思考行复用思考块的 SVG 常量，不重复造路径（icon-style.md §4 检查清单）
  if (chatMoreThinkIc) chatMoreThinkIc.innerHTML = THINK_SVG;
  if (chatMoreThinkLabel) chatMoreThinkLabel.textContent = t("chat.more_thinking");
  if (chatMoreWorkspaceLabel) chatMoreWorkspaceLabel.textContent = t("settings.workspace");
  renderWorkspaceMenuLabels();
  renderToolsBlacklistLabels();
}

/** 打开菜单时把各行的动态状态刷新一遍。 */
function renderMoreMenu() {
  renderMoreMenuLabels();
  renderChatModeSeg();
  renderRunModeUI();
  refreshWorkspaceUI();
}

/** Persistent conversation-mode label used in the status bar. */
function currentModeLabel(): string {
  return t(THINKING_STATUS_LABEL[chatMode]);
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

// ── AI Chat history (file-based via Rust IPC) ───────────────────
/** 本次对话的 token 用量（**表盘口径**）。
 *  随会话一起落盘 → 历史回顾时读回来还原表盘（2026-09 起，
 *  不再「切到历史会话就归零」）。 */
interface SessionUsage {
  hit: number;
  miss: number;
  total: number;
  elided: number;
  dropped: number;
}

/** 过程快照里的一步：一条思考 / 一次工具调用（含结果）。 */
interface SessionStep {
  kind: "thinking" | "tool" | "text";
  /** 工具名（kind=tool 时） */
  name?: string;
  /** 命令原文 / 参数摘要 / 文本片段（已截断） */
  detail?: string;
  /** 工具输出（已截断，kind=tool 时） */
  result?: string;
  isError?: boolean;
  /** `Write` / `Edit` 改动的文件绝对路径（2026-09-17，backlog §8.1）。
   *  **只从 `tool_use` 入参取**（见 `WRITE_TOOLS`）—— 从工具输出正文里正则猜会把「只是读过的
   *  文件」也算成改动。恢复历史时据此重建「本次会话改动过的文件」列表。 */
  path?: string;
}

/** 一个回合的过程快照 —— 历史回顾时按回合渲染成可折叠的「过程」块。 */
interface SessionProcess {
  turn: number;
  items: SessionStep[];
}

interface ChatSession {
  id: string;
  title: string;           // first user message, truncated
  messages: Array<{ role: string; content: string }>;
  createdAt: number;       // Date.now()
  /** token 用量（表盘口径）；旧记录没有该字段 */
  usage?: SessionUsage;
  /** 过程快照（按回合分组）；旧记录没有该字段 */
  steps?: SessionProcess[];
}

// ── Session persistence — file-based via Rust IPC ─────────────────
// Chat sessions are stored in Lunac 数据根（exe 所在目录）\ModuleData\history\chat.db
// （SQLite；2026-09-17 起。同目录 chat-history.json 只是迁移前的旧文件）
// 与 WebView2 缓存解耦，清除浏览器缓存不影响会话数据。

const MAX_SESSIONS = 50;
let currentSessionId: string | null = null;  // reuse across saves to avoid duplicates

/** 本对话的**过程快照**（每回合一组：思考 / 工具调用 + 结果）。
 *  随会话一起落盘，历史回顾时渲染成可折叠的「过程」块。
 *  只在**回合结束**时采集一次（见 recordTurnSteps），不参与流式渲染。 */
let sessionSteps: SessionProcess[] = [];

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

/** Strip the injected keyword-hint prefix (e.g. "## Debugging Methodology ... ---")
 *  from a stored user message so restored conversations show the real question
 *  instead of the prompt boilerplate. Mirrors the title extraction in
 *  saveCurrentSession: the hint is a send-time wrapper, not conversation history.
 *  注：2026-09-17 起用户消息里只剩**关键词条件块**（固定的「人格 + 文风」已搬进 agent
 *  系统提示词），所以不含关键词的提问**根本没有** `\n\n---\n\n` —— 此时原样返回即可。 */
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
async function persistCurrentSessionInner(
  snapshotChat: Array<{ role: string; content: string }>,
  snapshotId: string | null,
  snapshotUsage: SessionUsage,
  snapshotSteps: SessionProcess[],
) {
  // Keep any conversation with at least one real user message. Previously
  // this required 1 user + 1 assistant, so conversations where the CLI
  // errored / was closed mid-stream silently never reached history.
  if (snapshotChat.filter(m => m.role === "user" && m.content.trim()).length < 1) return;
  // Strip injected keyword hints from stored user messages so the history
  // file never accumulates the "## Debugging Methodology ... ---" boilerplate.
  const cleaned = snapshotChat.map(m =>
    m.role === "user" ? { ...m, content: cleanUserContent(m.content) } : m
  );
  const pruned = pruneContext(cleaned);
  const firstUser = pruned.find(m => m.role === "user");
  // Strip the injected keyword-hint header (e.g. "## Debugging Methodology ... ---")
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
    // 表盘数值与过程快照一并落盘：历史回顾时读回来还原表盘 / 展示「查看过程」。
    // 旧记录没有这两个字段，读取端按可选处理（前端 `?.`，Rust 侧 `#[serde(default)]`）。
    usage: snapshotUsage,
    steps: snapshotSteps,
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
  const snapshotUsage: SessionUsage = { ...usageTotals };
  const snapshotSteps = sessionSteps.map(g => ({
    turn: g.turn,
    items: g.items.map(i => ({ ...i })),
  }));
  return queueSessionSave(() =>
    persistCurrentSessionInner(snapshot, snapshotId, snapshotUsage, snapshotSteps),
  );
}

function restoreSession(session: ChatSession) {
  // Continue writing to the SAME session record when the user keeps typing
  // after restore — otherwise the next save generates a brand-new id and
  // the original record is never updated (duplicate history, point 4/17).
  currentSessionId = session.id;
  // 表盘口径 = 当前对话，且**从会话记录里读回来**（表盘数值随会话落盘，见
  // SessionUsage）—— 历史回顾时能看到当时那次对话的命中率/总量/压缩次数。
  // 旧记录没有 usage 字段 → 归零。
  usageTotals = session.usage
    ? { ...session.usage }
    : { hit: 0, miss: 0, total: 0, elided: 0, dropped: 0 };
  liveCompaction = { elided: 0, dropped: 0 };
  updateTokenDashboard();
  // 过程快照一并恢复，供「查看过程」
  sessionSteps = (session.steps ?? []).map(g => ({
    turn: g.turn,
    items: g.items.map(i => ({ ...i })),
  }));
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
  // Strip injected keyword hints (## Debugging Methodology / ## TDD Requirement ...)
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
  invoke("set_ui_mode", { mode: "plugin" }).catch(() => {});

  resultsContainer.classList.remove("hidden");
  searchBar.classList.add("has-results");

  // Build static HTML from messages — same chat-log container so the
  // conversation can continue seamlessly after restore. Every message gets a
  // roll-back button (需求4).
  renderChatLogHtml();
  // 过程快照：每个回合的过程块插到该回合的助手气泡之后（可折叠）
  renderHistoryProcess(session);
  // 「本次会话改动过的文件」由过程快照重建（必须在 renderChatLogHtml() 之后 —— 那次调用
  // 会重建 #chat-log，面板挂在它里面）。backlog §8.1。
  rebuildChangedFilesFromSteps(sessionSteps);
  statusText.textContent = t("status.history_restored");
  // 把该会话的历史灌回 agent：恢复只是重建了 DOM，agent 侧还留着它自己上一段对话的
  // 上下文（甚至是另一个会话的）—— 不灌的话「恢复旧会话后追问」必然是零上文
  // （2026-09-17，见 queueAgentHistory 的注释）。
  queueAgentHistory(chatHistory);
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

// ── 过程快照：采集（回合结束）与回放（历史回顾）────────────────
//
// 会话记录原本只有 `{role, content}` 气泡，agent 的思考与工具调用从不落盘 →
// 历史回顾时**看不到当时的执行过程**。这里在回合结束时把过程块按 DOM 顺序抽成
// 可序列化的步骤，随会话存盘；恢复历史时再渲染成可折叠的「过程」块。
// 只存**摘要**（命令 / 参数 / 输出各截断），避免历史文件被长输出撑爆。

/** 单条步骤文本上限 */
const STEP_MAX = 2000;
/** 单回合最多保留的步骤数（超长回合只留前 N 步，防文件膨胀） */
const STEPS_PER_TURN_MAX = 60;

function clipStep(s: string, n = STEP_MAX): string {
  const v = (s || "").trim();
  return v.length > n ? v.slice(0, n) + "…" : v;
}

/** 把本轮的过程块按 DOM 顺序抽成步骤（回合结束时调用一次）。 */
function recordTurnSteps(flowEl: HTMLElement) {
  const items: SessionStep[] = [];
  flowEl
    .querySelectorAll<HTMLElement>(".think-block, .agent-text, .tool-card, .todo-panel")
    .forEach(el => {
      if (items.length >= STEPS_PER_TURN_MAX) return;
      if (el.classList.contains("think-block")) {
        const text = clipStep(el.dataset.thinkText || el.textContent || "");
        if (text) items.push({ kind: "thinking", detail: text });
      } else if (el.classList.contains("agent-text")) {
        const text = clipStep(el.textContent || "");
        if (text) items.push({ kind: "text", detail: text });
      } else if (el.classList.contains("tool-card")) {
        items.push({
          kind: "tool",
          name: el.querySelector(".tool-name")?.textContent?.trim() || "",
          detail: clipStep(el.querySelector(".tool-cmd")?.textContent || ""),
          result: clipStep(el.querySelector(".tool-out")?.textContent || ""),
          isError: el.classList.contains("failed"),
          // 写类工具卡上挂着 `data-file`（见 agentToolInput）—— 记进快照，恢复历史时据此
          // 重建「本次会话改动过的文件」列表。**不从 detail 文本里解析路径**。
          path: el.dataset.file || undefined,
        });
      } else {
        // TodoWrite 面板：把清单文本压成一行留痕
        const text = clipStep((el.textContent || "").replace(/\s+/g, " "));
        if (text) items.push({ kind: "text", detail: text });
      }
    });
  if (items.length) sessionSteps.push({ turn: sessionSteps.length + 1, items });
}

/** 历史「过程」块里的一步 → HTML（复用实时对话的类名，样式免费）。 */
function historyStepHtml(s: SessionStep): string {
  if (s.kind === "thinking") {
    const text = s.detail || "";
    return (
      `<details class="think-block"><summary><span class="think-ic">${THINK_SVG}</span>` +
      `<span class="think-label">${esc(t("agent.thinking_done", { n: String(text.length) }))}</span></summary>` +
      `<div class="think-content">${esc(text)}</div></details>`
    );
  }
  if (s.kind === "text") {
    return `<div class="agent-text">${esc(s.detail || "")}</div>`;
  }
  const firstLine = (s.result || "").split("\n")[0].replace(/\s+/g, " ").trim().slice(0, 60);
  const state = s.isError
    ? t("agent.tool_failed", { txt: firstLine || "error" })
    : t("agent.tool_ok");
  return (
    `<details class="tool-card ${s.isError ? "failed" : "ok"}">` +
    `<summary class="tool-card-head"><span class="tool-ic">${TERM_SVG}</span>` +
    `<span class="tool-name">${esc(s.name || t("chat.badge_tool"))}</span>` +
    `<span class="tool-cmd">${esc(s.detail || "")}</span>` +
    `<span class="tool-state">${esc(state)}</span></summary>` +
    (s.result ? `<div class="tool-body"><pre class="tool-out">${esc(s.result)}</pre></div>` : "") +
    `</details>`
  );
}

/** 历史回顾：把每个回合的过程快照渲染成可折叠的「过程」块，
 *  插到该回合的助手气泡之后（回合 N ↔ 第 N 条助手消息）。 */
function renderHistoryProcess(session: ChatSession) {
  const groups = (session.steps ?? []).filter(g => g.items && g.items.length > 0);
  if (!groups.length) return;
  const log = document.getElementById("chat-log");
  if (!log) return;
  const assistants = Array.from(log.querySelectorAll<HTMLElement>(".chat-msg-assistant"));
  groups.forEach((g, i) => {
    const flow = document.createElement("div");
    flow.className = "agent-flow history-process flow-folded";
    flow.innerHTML = g.items.map(historyStepHtml).join("");
    const footer = document.createElement("div");
    footer.className = "turn-footer";
    const label = document.createElement("span");
    label.className = "turn-tool-count";
    label.textContent = t("agent.process_steps", { n: String(g.items.length) });
    footer.appendChild(label);
    const btn = document.createElement("button");
    btn.className = "turn-fold";
    btn.setAttribute("type", "button");
    const setFolded = (folded: boolean) => {
      flow.classList.toggle("flow-folded", folded);
      btn.textContent = folded ? t("agent.turn_expand") : t("agent.turn_collapse");
    };
    btn.addEventListener("click", () => setFolded(!flow.classList.contains("flow-folded")));
    footer.appendChild(btn);
    flow.appendChild(footer);
    setFolded(true);
    const host = assistants[i];
    if (host) host.insertAdjacentElement("afterend", flow);
    else log.appendChild(flow);
  });
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
 *  messages, persists the trimmed session, syncs the trimmed history back to
 *  the agent's own context and re-renders. A full snapshot is backed up to
 *  localStorage first so the operation is reversible (no data loss).
 *  idx=-1 表示回退到空对话（重试首条消息用）。 */
async function rollbackChat(idx: number) {
  if (idx < -1 || (idx >= 0 && !chatHistory[idx])) return;
  // 必须在下面把 isStreaming 清零**之前**记住：第 4 步要靠它决定「要不要先取消这次运行」。
  const wasStreaming = isStreaming;
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
  // 4) 让 agent 的上下文与界面保持一致（2026-09-17 改）。
  //    旧实现是 `stop_cli` + `start_cli`：注释说为了让下一轮看不到被剪掉的消息，
  //    但它连**保留下来的上文一起清空了** —— 用户表现为「回退后引用不到上文」。
  //    现在改为把保留下来的历史整体灌回 agent（协议 `set_history`）：只丢工具调用
  //    细节（本来就不落前端），用户 / 助手的对话本身完整保留，且**不再重启进程**。
  //    仍在流式中时要先取消这一次运行：否则它跑完会把已丢弃的内容写回 agent 历史，
  //    还白烧 token。取消 = 重启进程，重启后 `queueAgentHistory` 会在 CLI 就绪时
  //    把历史补上（`cliReady` 此刻为 false，它自己会挂起）。
  if (wasStreaming) {
    try { await invoke("stop_cli"); } catch {}
    cliReady = false;
    try { await invoke("start_cli"); } catch {}
  }
  queueAgentHistory(chatHistory);
  // 5) Re-render
  renderChatLogHtml();
  // 「改动过的文件」跟着过程快照重建（`renderChatLogHtml()` 会重建 #chat-log，必须排在它之后）。
  // 注：`sessionSteps` 本身不随回退裁剪（既有行为），所以这里通常与回退前一致 ——
  // 但它保证「列表 = 记录里真实存在的改动」这一条恒成立，不依赖调用顺序。
  rebuildChangedFilesFromSteps(sessionSteps);
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
  invoke("set_ui_mode", { mode: "plugin" }).catch(() => {});
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
  // 本函数不走 applyWindowSize()，所以必须自己补一次界面层同步 —— 否则调用方
  // 回到简洁搜索后 Rust 还停在 plugin，Esc 会一直 emit clear、永远隐藏不掉窗口。
  syncUiMode();
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

  // AI 供应商配置不再由前端恢复：唯一真相源是 <exe 根>\config\ai.json，
  // 由 Rust 侧 main() 在启动时读回并注入环境变量（见 commands::apply_saved_ai_config）。
  // 以前这里会用 localStorage["lunac-ai-config"] 回灌 set_ai_config，它会在启动时
  // 把 localStorage 里的旧值**覆盖**掉用户刚改的 .env —— 表现为「key 改了不生效、
  // 一直 401」，且无从判断生效的是哪一份（2026-09-15 实测踩到）。
  //
  // 一次性迁移：老版本只有 localStorage 一份，升级后不能让它静默丢配置。
  // **只在后端完全没有 key 时才回填**（绝不覆盖 .env / ai.json），回填后立刻删掉旧键，
  // 于是它再也不可能参与后续启动。
  try {
    const raw = localStorage.getItem("lunac-ai-config");
    if (raw) {
      const old = JSON.parse(raw);
      const cur = await invoke<{ api_key?: string }>("get_ai_config");
      if (!cur?.api_key && old?.url && old?.model && old?.key) {
        await invoke("set_ai_config", {
          provider: old.provider || "deepseek",
          url: old.url,
          key: old.key,
          model: old.model,
          agent_url: old.agent_url || null,
          search_provider: old.search_provider || null,
          search_key: old.search_key || null,
        });
      }
      localStorage.removeItem("lunac-ai-config");
    }
  } catch { /* 迁移失败不影响启动：配置仍可在设置面板重填 */ }

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

  // Sync thinking switch (on/off) to Rust on startup.
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
    // 只认左键：右键是上下文菜单（搜索栏的四项菜单），中键是粘贴等，
    // 都不该启动窗口拖动 —— 否则右键后手一抖就把窗口拖走了。
    if (e.button !== 0) return;
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

// Detached（大界面）顶栏。**必须显式绑定**：Tauri 的 `data-tauri-drag-region` 是
// 「裸属性」语义 —— 只有事件目标**就是**带属性的那个元素才触发（tauri drag.js
// 的 `el === composedPath[0]`），点在子元素上无效；而顶栏中间被 `#detached-title`
// （flex:1）占满，靠属性只能拖到 12px 内边距/间隙 ⇒ 表现为「有时候拖不动」。
// JS 拖动对子元素同样生效，且 factory 内已跳过 button ⇒ 顶栏三个按钮照常可点。
// 与 `#search-bar` 一致：属性 + JS 两条通道都留，属性管「按下即拖」，JS 管「移动阈值」。
makeDragHandle(detachedHeader);

// Window drag is intentionally LIMITED to the title-bar areas only
// (search bar / plugin title bar / detached header — all carry
// `data-tauri-drag-region` + makeDragHandle). The results/chat body must
// NOT start drags: a mousedown handler here calls preventDefault() which
// kills text selection initiation, and a 3px move hijacks the whole window
// — that's why AI chat text couldn't be selected (需求3). Body text stays
// fully selectable; move the window from the title bar instead.

// 输入框拖动 —— uTools 风格：3px 阈值后才发起拖动，拖动期间关掉输入框的
// pointer-events（避免拖动过程中选中文本），结束后恢复并回焦。
// 简洁搜索框与详细搜索大界面的输入框用的是同一套手感，故抽成工厂。
function makeInputDragHandle(input: HTMLTextAreaElement | HTMLInputElement) {
  // 转成 HTMLElement 再挂监听：联合类型的 addEventListener 会退化成 Event 重载，
  // 拿不到 MouseEvent 的 clientX/clientY（编译期报错）。
  (input as HTMLElement).addEventListener("mousedown", (e) => {
    // 只认左键：右键要在输入框上弹上下文菜单（全选/复制/剪切/粘贴），
    // 若右键也能启动拖动，菜单一弹出窗口就跟着跑了。
    if (e.button !== 0) return;
    const startX = e.clientX;
    const startY = e.clientY;
    let dragged = false;

    const onMove = (ev: MouseEvent) => {
      if (!dragged && (Math.abs(ev.clientX - startX) > 3 || Math.abs(ev.clientY - startY) > 3)) {
        dragged = true;
        dragging = true;
        // Disable input interaction during drag (uTools-style cursor switch)
        input.style.pointerEvents = "none";
        win.startDragging();
      }
    };
    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      if (dragged) {
        // Restore after drag ends
        setTimeout(() => {
          input.style.pointerEvents = "";
          dragging = false;
          // Re-focus if needed
          if (isVisible && !pluginActive) input.focus();
        }, 200);
      }
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  });
}

// Search input — uTools-style: disable pointer events during drag
makeInputDragHandle(searchInput);

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
  // 表盘口径 = 当前这次对话，新对话即归零（按天日志不受影响，对账照旧）
  resetUsageTotals();
  liveCompaction = { elided: 0, dropped: 0 };
  // 新对话 → 过程快照也重新开始（旧会话的已随它自己的记录落盘）
  sessionSteps = [];
  // 「改动过的文件」跟着同一条生命周期（它本来就是从过程快照推导出来的）——
  // 不跟着清会看到上一次对话改的文件还挂在列表里。
  sessionChangedFiles = [];
  renderChangedFilesPanel();

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
  invoke("set_ui_mode", { mode: "plugin" }).catch(() => {});
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

// 思考开关：两档分段（原为循环胶囊与三档分段，2026-09 收进「更多」菜单）
function setThinkingMode(mode: ThinkingMode) {
  chatMode = mode;
  try { localStorage.setItem("lunac-chat-mode", chatMode); } catch {}
  renderChatModeSeg();
  invoke("set_thinking_mode", { mode: chatMode, restart: true }).catch((e) => console.warn("set_thinking_mode", e));
  if (statusText) statusText.textContent = t("status.ai", { mode: currentModeLabel() });
}
chatModeSeg?.querySelectorAll<HTMLButtonElement>("button[data-mode]").forEach((b) => {
  b.addEventListener("click", (e) => {
    e.stopPropagation(); // 别让「更多」菜单的 outside-click 把它关掉
    const m = b.dataset.mode as ThinkingMode;
    if (m !== chatMode) setThinkingMode(m);
  });
});

// ── 命令运行方式（Trae「沙箱」的 Lunac 等价物，见 docs/agent-ui-spec.md §4）──
// 三档**只决定「问不问」**，不动文件边界：工作区锁、安全档位、危险命令拦截都不受
// 这里影响（规范里明确不把策略级边界称作「沙箱」）。
// 默认 allowlist = 与既有行为完全一致（内置安全前缀 + 用户白名单自动放行）。
type AgentRunMode = "manual" | "allowlist" | "auto";

const RUN_MODE_KEY = "lunac-agent-run-mode";
const RUN_MODE_ORDER: AgentRunMode[] = ["manual", "allowlist", "auto"];

function getRunMode(): AgentRunMode {
  const v = localStorage.getItem(RUN_MODE_KEY);
  return v === "manual" || v === "auto" ? v : "allowlist";
}

/** 运行方式按钮：图标 + 三格档位点阵（不写档位名）+ 右侧一句功能简述。
 *  自动档是高风险状态：点阵与 ⋯ 按钮都转红，简述换成常驻警示（不再单独占一条全宽横幅）。 */
function renderRunModeUI() {
  if (!chatRunmodeBtn) return;
  const mode = getRunMode();
  const level = RUN_MODE_ORDER.indexOf(mode) + 1; // 1=手动 2=白名单 3=自动
  const label = `${t("agent.run_mode")}：${t(`agent.run_mode_${mode}`)} — ${t(`agent.run_mode_${mode}_hint`)}`;
  chatRunmodeBtn.setAttribute("title", label);
  chatRunmodeBtn.setAttribute("aria-label", label);
  chatRunmodeBtn.dataset.mode = mode;
  chatRunmodeBtn.classList.toggle("run-mode-auto", mode === "auto");
  chatRunmodeBtn.querySelectorAll<HTMLElement>(".rm-dots i").forEach((dot, i) => {
    dot.classList.toggle("on", i < level);
  });

  // 功能简述：自动档用警示文案（红字）
  if (chatRunmodeHint && !runModeConfirmEl?.isConnected) {
    chatRunmodeHint.textContent =
      mode === "auto" ? t("agent.run_mode_auto_warning") : t(`agent.run_mode_${mode}_hint`);
    chatRunmodeHint.classList.toggle("warn", mode === "auto");
  }
  // 菜单收起时也要能看出「自动运行开着」
  chatMoreBtn?.classList.toggle("run-mode-auto", mode === "auto");
}

function setRunMode(mode: AgentRunMode) {
  writeRunMode(mode);
  renderRunModeUI();
  applyRunModeProfile(mode, true);
}

/** 只写 localStorage、不碰后端（设置面板改安全档位时用它同步胶囊显示）。 */
function writeRunMode(mode: AgentRunMode) {
  try {
    localStorage.setItem(RUN_MODE_KEY, mode);
  } catch {
    /* 存不了就只在本次会话生效 */
  }
}

// 档位 → 安全档位：手动/白名单都落在 project（两者的差别只在前端「问不问」），
// 自动档落在 full（--dangerously-skip-permissions）。见 agent-ui-spec §4.2。
const RUN_MODE_PROFILE: Record<AgentRunMode, SecurityProfile> = {
  manual: "project",
  allowlist: "project",
  auto: "full",
};

/**
 * 安全档位（文件边界）—— 与运行方式正交：运行方式决定「问不问」，
 * 档位决定「允不允许」（agent-ui-spec §4.4）。两个入口（输入栏胶囊 /
 * 设置面板下拉）共用这一个下发点，避免重复 invoke。
 */
type SecurityProfile = "safe" | "project" | "full";
const PROFILE_KEY = "lunac-security-profile";

function getSavedProfile(): SecurityProfile {
  const v = localStorage.getItem(PROFILE_KEY);
  return v === "safe" || v === "full" ? v : "project";
}

/** 已经下发给后端的档位。相同就不重复下发（避免白重启一次 agent）。 */
let appliedSecurityProfile: SecurityProfile = getSavedProfile();

function setSecurityProfile(profile: SecurityProfile, restart: boolean) {
  appliedSecurityProfile = profile;
  try {
    localStorage.setItem(PROFILE_KEY, profile);
  } catch {
    /* 存不了就只在本次会话生效 */
  }
  invoke("set_security_profile", { profile, restart }).catch((e) =>
    console.warn("set_security_profile", e),
  );
}

/** 把运行方式映射出的档位下发后端。restart=false 仅用于启动同步（保持懒启动）。 */
function applyRunModeProfile(mode: AgentRunMode, restart: boolean) {
  const profile = RUN_MODE_PROFILE[mode];
  if (profile === appliedSecurityProfile) return;
  setSecurityProfile(profile, restart);
}

// 设置面板改了安全档位 → 同步胶囊显示（只读档下「自动运行」的说法不成立，
// 收回到最保守的「手动」；这是收紧方向，不会悄悄放松询问）。
window.addEventListener("lunac-security-profile-changed", (e) => {
  const profile = (e as CustomEvent).detail?.profile as SecurityProfile | undefined;
  if (profile !== "safe" && profile !== "project" && profile !== "full") return;
  setSecurityProfile(profile, true);
  if (profile === "safe") writeRunMode("manual");
  renderRunModeUI();
});

let runModeConfirmEl: HTMLElement | null = null;

/** 切到「自动」档必须二次确认：这是会显著放松询问的开关。
 *  确认条就出现在运行方式按钮旁边（原来的全宽横幅已去掉，用户要求就地提示）。 */
function askRunModeAutoConfirm() {
  if (runModeConfirmEl?.isConnected) return;
  const host = chatRunmodeHint;
  if (!host) return;
  host.textContent = "";
  host.classList.remove("warn");
  const box = document.createElement("span");
  box.className = "run-mode-confirm";
  const msg = document.createElement("span");
  msg.className = "run-mode-confirm-msg";
  msg.textContent = t("agent.run_mode_auto_confirm");
  const ok = document.createElement("button");
  ok.type = "button";
  ok.className = "run-mode-confirm-ok";
  ok.textContent = t("agent.run_mode_confirm_ok");
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "run-mode-confirm-cancel";
  cancel.textContent = t("agent.run_mode_confirm_cancel");
  const close = () => {
    box.remove();
    runModeConfirmEl = null;
    renderRunModeUI(); // 把功能简述写回来
  };
  ok.addEventListener("click", (e) => {
    e.stopPropagation();
    close();
    setRunMode("auto");
  });
  cancel.addEventListener("click", (e) => {
    e.stopPropagation();
    close();
  });
  box.append(msg, ok, cancel);
  host.appendChild(box);
  runModeConfirmEl = box;
}

// 三档循环（点一下进一档）：手动 → 白名单 → 自动 → 手动
chatRunmodeBtn.addEventListener("click", (e) => {
  e.stopPropagation(); // 别让「更多」菜单的 outside-click 把它关掉
  const next = RUN_MODE_ORDER[(RUN_MODE_ORDER.indexOf(getRunMode()) + 1) % RUN_MODE_ORDER.length];
  if (next === "auto") askRunModeAutoConfirm();
  else setRunMode(next);
});
renderRunModeUI();
// 启动同步：把上次的安全档位下发给后端（后端内存态每次启动都回到 project）。
// restart=false → 只存值、不拉起 agent，保持懒启动。
invoke("set_security_profile", { profile: appliedSecurityProfile, restart: false }).catch(() => {});

// ── AI workspace (chat window entry — the ONLY workspace UI) ─────
// Default (empty) workspace = user home dir → whole system reachable,
// sensitive edits outside it still go through ask approval cards.
let currentWorkspace = "";
function refreshWorkspaceUI() {
  if (!chatWorkspacePath) return;
  chatWorkspacePath.textContent = currentWorkspace || t("settings.workspace_default");
  chatWorkspacePath.setAttribute("title", currentWorkspace || "");
  chatWorkspacePath.classList.toggle("has-workspace", !!currentWorkspace);
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
chatWorkspaceSelect?.addEventListener("click", async () => {
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true, multiple: false, title: t("settings.workspace_select") });
    if (typeof picked === "string" && picked) {
      const ok = await applyWorkspace(picked);
      if (!ok) statusText.textContent = t("settings.workspace_fail");
    }
  } catch {}
});
chatWorkspaceReset?.addEventListener("click", async () => {
  const ok = await applyWorkspace("");
  if (!ok) statusText.textContent = t("settings.workspace_fail");
});
// Keep the row labels in sync with the current language
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
  { name: "WebSearch" },
  { name: "WebFetch" },
  { name: "AskUserQuestion" },
  { name: "TodoWrite" },
  // 往期会话检索（2026-09-19，A2）：只读本机会话库，免审批。列在这里是为了让用户
  // 能主动关掉它 —— 前缀缓存里会带一段「往期会话索引」，不想让模型看到就往这里禁。
  { name: "SessionSearch" },
  // 子代理（2026-09-20，A1）：默认可用。之所以列出来，是因为它**很花钱** ——
  // 一个子代理能跑 8 轮工具往返、并有自己的 token 预算。不想让模型自作主张派
  // 代理出去的用户，可以在设置里把它禁掉（`--disallowedTools Agent`）。
  { name: "Agent" },
  // MCP resources 读侧（2026-09-20，A3）：只在用户真的配了 `tools\*.json` 时才注册
  // （没配就压根不在请求体里）。列在这里同样是给用户一个关掉它们的入口。
  { name: "ListMcpResourcesTool" },
  { name: "ReadMcpResourceTool" },
  // 长期记忆写入侧（2026-09-20，A4）：它只在桥接通时注册（没桥就写不进去）。
  // 列出来是给用户一个「别让模型自己写长期记忆」的开关；禁掉它并不影响**读取**
  // 已存的记忆（那是 `LUNAC_MEMORY` 管的事）。
  { name: "Remember" },
  { name: "Skill" },
  // 计划相位（2026-09-20，A7）：这两件都不碰本机 —— 进入只改 agent 进程内的一个标志，
  // 退出只是把计划交出来给用户裁决。列出来是给用户一个「别让模型自作主张先出计划」的开关
  // （禁掉后模型只能直接动手，或在正文里讲计划）。
  { name: "EnterPlanMode" },
  { name: "ExitPlanMode" },
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

/** 已禁用（从请求体 tools 里剔除）的工具名。菜单打开时装载一次，之后由
 *  行内 ×/✓ 切换维护，点「保存」才写回 localStorage 并重启 agent。 */
let toolsOffSet = new Set<string>();

function loadToolsBlacklistSel() {
  toolsOffSet = new Set(loadCustomBlacklist());
}

function renderToolsBlacklist() {
  if (!chatToolsList || !chatToolsTitle) return;
  chatToolsTitle.textContent = t("chat.tools_title");
  chatToolsList.innerHTML = TOOL_BLACKLIST_CANDIDATES.map(tool => {
    const off = tool.locked || toolsOffSet.has(tool.name);
    return `
    <div class="chat-tools-item${off ? " off" : ""}${tool.locked ? " locked" : ""}">
      <button type="button" class="chat-tools-toggle" data-tool="${esc(tool.name)}"${tool.locked ? " disabled" : ""}
              title="${esc(off ? t("chat.tools_enable") : t("chat.tools_disable"))}"
              aria-label="${esc(off ? t("chat.tools_enable") : t("chat.tools_disable"))}">${off ? X_SVG : CHECK_SVG}</button>
      <span class="chat-tools-name">${esc(tool.name)}</span>
      ${tool.locked ? `<span class="chat-tools-desc">${esc(t("chat.tools_locked"))}</span>` : ""}
    </div>`;
  }).join("");
}
// 行内的 ×/✓ 切换（icon-style.md §3 的「取消 ✕」「保存 ✓」，加一点残影/高光）
const CHECK_SVG = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/><circle cx="19.6" cy="5.4" r=".9" opacity=".28"/></svg>`;
const X_SVG = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/><circle cx="19.6" cy="5.4" r=".9" opacity=".28"/></svg>`;
// 点一下切一格：× = 已禁用（红），✓ = 启用（暗）
chatToolsList?.addEventListener("click", (e) => {
  const btn = (e.target as HTMLElement).closest(".chat-tools-toggle") as HTMLButtonElement | null;
  if (!btn || btn.disabled) return;
  e.stopPropagation();
  const name = btn.getAttribute("data-tool") || "";
  if (!name) return;
  if (toolsOffSet.has(name)) toolsOffSet.delete(name);
  else toolsOffSet.add(name);
  renderToolsBlacklist();
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
}

// ── 「更多」菜单开关（思考 / 运行方式 / 工作区 / 工具黑名单四行）─────
function closeMoreMenu() {
  chatMoreMenu?.classList.add("hidden");
  chatMoreBtn?.classList.remove("open");
}
chatMoreBtn?.addEventListener("click", (e) => {
  e.stopPropagation();
  const willOpen = chatMoreMenu?.classList.contains("hidden") ?? false;
  if (willOpen) {
    renderMoreMenu();      // 打开时刷新各行状态（含工作区路径）
    loadToolsBlacklistSel();
    renderToolsBlacklist();
    chatMoreMenu?.classList.remove("hidden");
    chatMoreBtn?.classList.add("open");
  } else {
    closeMoreMenu();
  }
});
document.addEventListener("click", (e) => {
  const wrap = chatMoreBtn?.parentElement;
  if (wrap && !wrap.contains(e.target as Node)) closeMoreMenu();
});

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
function addUsageToTotals(info: ChatDoneInfo) {
  const hit = info.cache_read_input_tokens || 0;
  // DeepSeek 走自动缓存，实测 cache_creation 恒为 0；这里保留加法是为了兼容
  // 会显式建缓存的端点（Anthropic 原生）。
  const miss = info.input_tokens + (info.cache_creation_input_tokens || 0);
  usageTotals.hit += hit;
  usageTotals.miss += miss;
  usageTotals.total += miss + hit + info.output_tokens;
}

function updateTokenDashboard() {
  const { hit, miss, total, elided, dropped } = usageTotals;
  const inputTokens = hit + miss;
  const hitPct = inputTokens > 0 ? Math.round((hit / inputTokens) * 100) : 0;
  // 今天发生过压缩才追加这段说明 —— 它解释「命中率为什么掉了」。
  const compactNote = elided + dropped > 0
    ? ` · ${t("token.compact_note", { elided: String(elided), dropped: String(dropped) })}`
    : "";

  tokenDashboard.innerHTML =
    `<span class="tk-bar" title="${t("token.scope_today")} · ${t("token.cache_tooltip", { hit: fmtTokens(hit), miss: fmtTokens(miss), total: fmtTokens(total) })}${compactNote}">` +
      `<span class="tk-bar-fill tk-bar-hit" style="width:${hitPct}%"></span>` +
      `<span class="tk-bar-fill tk-bar-miss" style="width:${100 - hitPct}%"></span>` +
    `</span>` +
    `<span class="tk-pct">${hitPct}%</span>` +
    `<span class="tk-total" title="${t("token.detail_tooltip", { hit: fmtTokens(hit), miss: fmtTokens(miss), out: fmtTokens(total - inputTokens), total: fmtTokens(total) })}${compactNote}">${fmtTokens(total)}</span>`;
}

/** `YYYY-MM-DD`（本地时区）—— 用量日志的文件名分片键 */
function localDateKey(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** `YYYY-MM-DD_HHMMSS`（本地时区）—— 计划文档（A7）的文件名分片键。
 *  **必须带时刻**：只到天的话，同一天批准两份计划会互相覆盖；
 *  形状由 Rust 侧 `storage::plan_path()` 严格校验（前端给的文件名一律不可信）。 */
function localStamp(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${localDateKey(d)}_${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
}

/** 把一次提问的用量追加进本地日志（只追加，失败不影响对话） */
function appendUsageLog(info: ChatDoneInfo) {
  const now = new Date();
  void invoke("append_usage_log", {
    date: localDateKey(now),
    record: {
      ts: now.getTime(),
      model: agentModel,
      input: info.input_tokens,
      output: info.output_tokens,
      cacheRead: info.cache_read_input_tokens ?? 0,
      cacheCreate: info.cache_creation_input_tokens ?? 0,
      // 压缩次数随用量一起落盘：日后对账时可用它解释命中率的断裂
      elided: liveCompaction.elided,
      dropped: liveCompaction.dropped,
      // 每次 API 请求一行（对账粒度，与平台用量页逐行对齐）；旧 agent 缺该字段 → 不写
      requests: info.requests ?? [],
    },
  }).catch(() => {});
}

/** 表盘归零 —— 新建会话 / 切换到别的会话时调用。
 *  表盘口径是「当前这次对话」，所以换对话必须清零；按天日志不受影响（对账照旧）。 */
function resetUsageTotals() {
  usageTotals = { hit: 0, miss: 0, total: 0, elided: 0, dropped: 0 };
  updateTokenDashboard();
}

function showTokenDashboard(show: boolean) {
  if (show) {
    tokenDashboard.classList.remove("hidden");
    tokenDashboard.classList.add("visible");
    updateTokenDashboard();
  } else {
    tokenDashboard.classList.remove("visible");
    tokenDashboard.classList.add("hidden");
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

/** 当前主题包提供的插件图标（插件 id → 已 convertFileSrc 的 URL）。
 *
 *  声明在这里而不是外观模块里（外观模块在文件末尾）是**必须**的：`pluginIconSvg`
 *  可能在模块初始化阶段就被调用，而 `const` 在声明前处于 TDZ —— 放末尾就是一次
 *  必然的 ReferenceError（白屏）。所以只把「存储 + 取值」放前面，填充逻辑（拉取
 *  主题、解析资产）留在外观模块，容量极小、无副作用。 */
const themeIconUrls = new Map<string, string>();

/** 查当前主题是否为该插件提供了专属图标；没有则返回 null（调用方回退内联 SVG）。 */
function themeIconUrl(id: string): string | null {
  return themeIconUrls.get(id) ?? null;
}

/** 生成插件的印象派 SVG icon；未知插件回退 🔧（原 emoji 保底）。
 *
 *  **主题包图标的唯一覆盖点**（2026-09-19）：当前主题若为某插件提供了专属图标
 *  （`theme.json` 的 `assets.icons.<插件 id>`），这里返回 `<img>` 而不是内联 SVG。
 *  全项目的插件图标都从这个函数出（`pluginIconSvg` 是图标汇聚点），所以在这一处
 *  接主题包即可全覆盖 —— 分散到各个渲染点去判断必然漏（历史教训：同名图标曾出现
 *  三份拷贝）。复用 `.result-item-icon-img` 是刻意的：与「开始菜单应用图标」同一套
 *  CSS（尺寸/圆角/居中），不必再写第二份样式。 */
function pluginIconSvg(id: string): string {
  const themed = themeIconUrl(id);
  if (themed) return `<img src="${themed}" class="result-item-icon-img" alt="">`;
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

  // **插入第 3 项后必须重新断言高度**（2026-09-19 修，不得删）。
  // 两个调用方（`win.onFocusChanged` 的唤出分支、`runSearchNow` 的空查询分支）都是
  // 「先 `renderAIEntry()` 渲染 2 项（Web 搜索 + AI）→ 它内部 applyWindowSize() 量到
  // 2 项高 → 再调本函数插 OCR」。实测 `.result-item` 56px/行：2 项 = 199、3 项 = 255。
  // 少了这一步，窗口就停在 2 项高度 —— 用户看到的正是「唤出时第 3 项被截掉一半，
  // 直到下一次搜索变动（重渲染）才恢复」（ai-spec §11 规则 44）。
  //
  // 这里用**同步**的 `applyWindowSize()`（不是 ResizeObserver，也不是逐帧滑动）：
  // `getBoundingClientRect()` 会强制一次布局，当场就能量到 3 项高度；而唤出瞬间
  // WebView 可能仍被判定为「未渲染」，RO 回调与 rAF 都会被推迟 —— 靠它们兜底
  // 就等于把这个错误高度一直留在屏幕上。与 `renderAIEntry` / `renderMixedResults`
  // 收尾处的做法一致。
  applyWindowSize();
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

/** 统一的 Esc 逐级清除逻辑（抽屉 → 详情大界面 → 插件 → 泡泡 → 文本 → 退出/隐藏） */
function handleEscClear() {
  // Drawer visible → close it first, don't touch plugin state
  if (drawerVisible) {
    toggleDrawer(false);
    return;
  }
  // 详细搜索大界面：Esc 的第一件事是退回简洁搜索（把查询词带回去），**不是清词**、
  // 更不是隐藏窗口。Rust 侧靠 UI_MODE=detail 把这一下 Esc 交给我们（见 hotkey.rs
  // 的 Esc 分支）—— 那里已经不再按「query/chips 是否空」判断，因为详情态的
  // 查询词在自己的输入框里、简洁搜索栏本来就是空的，按内容判空会直接隐藏窗口。
  if (detailOpen) {
    exitDetail();
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
  // Await Rust state sync — prevents race: next ESC sees correct UI_MODE=main
  await invoke("set_ui_mode", { mode: "main" }).catch(() => {});
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
  /** system/context_compacted 的压缩计数（见 usageTotals / ai-spec §11 规则 23） */
  elided?: number;
  dropped?: number;
  /** system/plan_mode（计划相位，A7）：`on` = 进入 / `off` = 已批准解除；
   *  `reason` 只有进入时才有（模型给用户的一句话说明）。 */
  state?: string;
  reason?: string;
  /** system/attachment_note（图片附件，A8）：本轮**没能发出去**的附件及原因。
   *  只有真有失败项时才发这个事件（见 renderAttachmentNote）。 */
  skipped?: { path?: string; reason?: string }[];
  /** system/hook_note（权限 hooks，A9）：用户脚本的裁决 / 输出 / 失败（见 renderHookNote）。
   *  字段名是 `hook_event` 而不是 `event` —— `event` 已被 stream_event 占用（对象形状）。 */
  hook_event?: string;
  tool_name?: string;
  items?: { kind?: string; text?: string; command?: string }[];
  request?: {
    subtype?: string;
    tool_name?: string;
    input?: Record<string, unknown>;
    tool_use_id?: string;
    /** agent 附上的**执行侧**静态安全分析 —— 见 docs/ai-spec.md §3.5。
     *  缺字段时回落到本文件自己的正则（兼容旧 agent 与非命令类工具）。 */
    analysis?: AgentAnalysis;
  };
  event?: {
    type: string;
    delta?: { type: string; text?: string; thinking?: string; partial_json?: string };
    content_block?: { type: string; id?: string; name?: string; input?: unknown };
  };
  message?: {
    content?: Array<{
      type: string;
      id?: string;
      text?: string;
      name?: string;
      thinking?: string;
      input?: unknown;
      content?: unknown;
      /** tool_result 与它对应的 tool_use 配对用（core-agent 一定会带，见 main.rs） */
      tool_use_id?: string;
      is_error?: boolean;
    }>;
  };
  usage?: { input_tokens: number; output_tokens: number };
  model?: string;
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
/** 一张命令执行卡片的状态（按 tool_use_id 与 tool_result 配对）。 */
interface AgentToolCard {
  el: HTMLDetailsElement;
  name: string;
  /** 命令原文（Bash/PowerShell）或 k=v 参数摘要 */
  cmd: string;
  /** 起始时间：入参开始流式时记；经过审批的卡片会在「批准」时重置（不把用户犹豫算进耗时） */
  startAt: number;
  done: boolean;
}

interface AgentView {
  turn: AgentTurn;                       // turn this view belongs to
  flow: HTMLElement;                    // ordered block container
  current: HTMLElement | null;          // current streaming content element
  currentKind: "thinking" | "text" | "tool" | null;
  curDetails: HTMLDetailsElement | null;
  curToolName: string;                  // current tool for readable arg display
  curToolArgs: string;                  // accumulated input_json_delta
  curToolId: string;                    // tool_use id of the current card
  thinkChars: number;
  textAll: string;                      // concatenated text blocks (history)
  /** tool_use_id → 卡片；用于把 tool_result 的结果填回同一张卡 */
  toolCards: Map<string, AgentToolCard>;
  /** 最近一张未完成的卡片（端点未给 tool_use_id 时的兜底配对） */
  openCard: AgentToolCard | null;
  /** 思考正文，折叠时从 DOM 摘下来，展开时再填回（避免长思考常驻 DOM） */
  thinkText: string;
}
let agentView: AgentView | null = null;

/** 从一张卡片里取「耗时」的展示串（<1s 用 ms，否则用 s，保留一位小数）。 */
function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "—";
  return ms < 1000 ? `${Math.round(ms)}ms` : `${(ms / 1000).toFixed(1)}s`;
}

function agentScroll() {
  autoScrollIfNearBottom();
}

// ── AI 对话：思考块 + 命令卡片（2026-09，参照 Trae 侧栏，见 docs/agent-ui-spec.md）──
//
// 图标严格按 docs/icon-style.md：24 栅格、fill:none、stroke:currentColor、
// stroke-width 2.2、圆头圆角；点彩圆点 r≈0.9 + opacity .28 作为「高光」装饰。

/** 思考：气泡 + 底座 + 点彩高光 */
const THINK_SVG = `<svg class="ai-ic" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3.2a6 6 0 0 1 3.4 10.9c-.3.2-.5.6-.5 1v1.3H9.1v-1.3c0-.4-.2-.8-.5-1A6 6 0 0 1 12 3.2Z"/><path d="M10 19.6h4"/><circle cx="19.8" cy="5.2" r=".9" opacity=".28"/></svg>`;

/** 终端：>_ 形状 + 点彩高光（命令执行） */
const TERM_SVG = `<svg class="ai-ic" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 7.5 9 12l-4 4.5"/><path d="M12.5 17H19"/><circle cx="19.8" cy="5.2" r=".9" opacity=".28"/></svg>`;

/** 思考正文超过该长度时：折叠态不常驻 DOM、展开态只渲染首尾（agent-ui-spec §3.2） */
const THINK_INLINE_MAX = 4000;
const THINK_HEAD_CHARS = 2000;
const THINK_TAIL_CHARS = 500;

function setThinkLabel(det: HTMLDetailsElement, state: "running" | "done", chars: number) {
  const label = det.querySelector<HTMLElement>(".think-label");
  if (!label) return;
  const key = state === "running" ? "agent.thinking_running" : "agent.thinking_done";
  label.textContent = t(key, { n: String(chars) });
}

/** 展开时把正文填回；超长只给首尾两段（避免一次塞进上万字拖慢整个对话流）。 */
function fillThinkContent(det: HTMLDetailsElement, box: HTMLElement) {
  const text = det.dataset.thinkText || "";
  if (!text) return;
  if (text.length <= THINK_INLINE_MAX) {
    box.textContent = text;
    return;
  }
  box.textContent =
    text.slice(0, THINK_HEAD_CHARS) +
    `\n… (${text.length - THINK_HEAD_CHARS - THINK_TAIL_CHARS} chars omitted) …\n` +
    text.slice(-THINK_TAIL_CHARS);
}

/** 建一张命令执行卡片：头部（工具名 + 命令 + 状态）+ 折叠体（元信息 / 输出 / 操作）。 */
function createToolCard(v: AgentView, name: string, id: string): AgentToolCard {
  const el = document.createElement("details");
  el.className = "tool-card running";
  if (id) el.dataset.toolId = id;
  el.innerHTML =
    `<summary class="tool-card-head">` +
    `<span class="tool-ic">${TERM_SVG}</span>` +
    `<span class="tool-name">${esc(name || t("chat.badge_tool"))}</span>` +
    `<span class="tool-cmd"></span>` +
    `<span class="tool-state">${esc(t("agent.tool_running"))}</span>` +
    `</summary>` +
    `<div class="tool-body">` +
    `<div class="tool-meta"></div>` +
    `<pre class="tool-out"></pre>` +
    `<div class="tool-actions"></div>` +
    `</div>`;
  v.flow.appendChild(el);
  const card: AgentToolCard = { el, name, cmd: "", startAt: performance.now(), done: false };
  if (id) v.toolCards.set(id, card);
  return card;
}

function agentNewBlock(kind: "thinking" | "text" | "tool", toolName?: string, toolId?: string) {
  const v = agentView;
  if (!v) return;
  agentCloseBlock();
  v.flow.querySelector(".plugin-result-loading")?.remove();

  if (kind === "thinking") {
    const det = document.createElement("details");
    det.className = "think-block";
    det.open = true; // 进行中展开，让用户看得到「在思考」
    det.innerHTML =
      `<summary><span class="think-ic">${THINK_SVG}</span><span class="think-label"></span></summary>` +
      `<div class="think-content"></div>`;
    v.flow.appendChild(det);
    v.curDetails = det;
    v.thinkChars = 0;
    v.thinkText = "";
    v.current = det.querySelector(".think-content");
    setThinkLabel(det, "running", 0);
    // 结束后的折叠态不保留正文节点；展开时再按需填回（见 fillThinkContent）
    det.addEventListener("toggle", () => {
      const box = det.querySelector<HTMLElement>(".think-content");
      if (!box) return;
      if (det.open) {
        if (!box.textContent) fillThinkContent(det, box);
      } else if (det.dataset.done === "1") {
        box.textContent = "";
      }
    });
  } else if (kind === "tool") {
    v.curToolName = toolName || "";
    v.curToolArgs = "";
    v.curToolId = toolId || "";
    if (v.curToolName === "TodoWrite") {
      // 待办清单单独画成一块面板（见 renderTodoPanel），不占命令卡片
      v.current = null;
      v.openCard = null;
    } else {
      const card = createToolCard(v, v.curToolName, v.curToolId);
      v.current = card.el.querySelector(".tool-cmd");
      v.openCard = card;
    }
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
    v.thinkText += s;
    if (v.curDetails) setThinkLabel(v.curDetails, "running", v.thinkChars);
  } else {
    v.textAll += s;
  }
  v.current!.textContent = (v.current!.textContent || "") + s;
  agentScroll();
}

/** 把工具入参 JSON 变成一行可读摘要：Bash/PowerShell 给命令原文，其余给 k=v。
 *  （Trae 风格：卡片上直接看到命令，而不是一坨原始 JSON） */
function toolArgsDisplay(name: string, raw: string): string {
  try {
    const obj = JSON.parse(raw);
    if (typeof obj === "object" && obj !== null && !Array.isArray(obj)) {
      if (name === "Bash" || name === "PowerShell") {
        return typeof (obj as any).command === "string" ? ((obj as any).command as string) : raw;
      }
      return Object.entries(obj)
        .map(([k, val]) =>
          `${k}=${typeof val === "string" ? (val as string) : JSON.stringify(val)}`
        )
        .join("  ");
    }
  } catch {
    // 流式分片不是完整 JSON —— 先用原文顶上（末片一定是完整的）
  }
  return raw;
}

/** 把入参摘要写进当前卡片：流式分片路径与整包回落路径共用。 */
function agentToolInput(name: string, rawJson: string) {
  const card = agentView?.openCard;
  if (!card) return;
  const display = toolArgsDisplay(name, rawJson);
  card.cmd = display;
  const el = card.el.querySelector<HTMLElement>(".tool-cmd");
  if (!el) return;

  // 写类工具：把 `file_path` 摘出来渲染成**可点链接**（点击 → 资源管理器定位该文件），
  // 其余入参照旧跟在后面。同时把路径挂到卡片 dataset，`recordTurnSteps` 据此写进过程
  // 快照 ⇒ 恢复历史时能重建「改动过的文件」列表（走 `steps` 已有那一列，不改表结构）。
  const field = WRITE_TOOLS[name];
  const file = field ? changedFilePathFromArgs(name, rawJson) : "";
  if (file) {
    card.el.dataset.file = file;
    noteChangedFile(file);
    const rest = argsWithoutPath(rawJson, field);
    el.innerHTML =
      fileLinkHtml(file) +
      (rest
        ? ` <span class="tool-cmd-rest">${esc(rest.length > 160 ? rest.slice(0, 160) + "…" : rest)}</span>`
        : "");
    return;
  }
  el.textContent = display.length > 200 ? display.slice(0, 200) + "…" : display;
}

// ── 被改动文件的路径追踪（backlog §8.1）──────────────────────────
// 目标：工具卡里的**被改动文件**可点击 → 资源管理器定位到它；并把本次会话改动过的文件
// 汇总成一块面板（参照 Trae 的效果）。
//
// **硬约束：路径只能取自 `tool_use` 的入参**（`Write` / `Edit` 的 `file_path`），
// 禁止从工具输出正文里正则猜 —— 输出里出现的路径只说明模型**提到过**它（很可能只是刚
// 读过的文件），拿它当「改动过的文件」会让列表混进一堆只读文件。入参是模型真正要写的
// 那个文件，语义精确，而且**零协议改动**。

/** 会改动文件的工具 → 入参里那条可信的路径字段。 */
const WRITE_TOOLS: Record<string, string> = { Write: "file_path", Edit: "file_path" };

/** 本次会话改动过的文件（按首次出现顺序去重）；恢复 / 回退历史时由 `sessionSteps` 重建。 */
let sessionChangedFiles: string[] = [];

/** 从写类工具入参里取路径。流式分片不是完整 JSON 时返回空串（末片一定会再调一次）。 */
function changedFilePathFromArgs(toolName: string, rawJson: string): string {
  const field = WRITE_TOOLS[toolName];
  if (!field) return "";
  try {
    const v = (JSON.parse(rawJson) as Record<string, unknown> | null)?.[field];
    return typeof v === "string" ? v.trim() : "";
  } catch {
    return "";
  }
}

/** 摘要里去掉已单独展示的路径字段，其余参数照旧按 `k=v` 拼（保持原顺序）。 */
function argsWithoutPath(rawJson: string, field: string): string {
  try {
    const obj = JSON.parse(rawJson) as Record<string, unknown>;
    if (typeof obj !== "object" || obj === null || Array.isArray(obj)) return "";
    return Object.entries(obj)
      .filter(([k]) => k !== field)
      .map(([k, val]) => `${k}=${typeof val === "string" ? val : JSON.stringify(val)}`)
      .join("  ");
  } catch {
    return "";
  }
}

/** `C:\a\b\c.txt` → `c.txt`（只按分隔符切，不碰盘符）。 */
function pathBase(p: string): string {
  const i = Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/"));
  return i >= 0 ? p.slice(i + 1) : p;
}

/** `C:\a\b\c.txt` → `C:\a\b` */
function pathDir(p: string): string {
  const i = Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/"));
  return i > 0 ? p.slice(0, i) : "";
}

/** 记一笔改动并刷新面板（同一路径只留一次）。 */
function noteChangedFile(path: string) {
  if (!path || sessionChangedFiles.includes(path)) return;
  sessionChangedFiles.push(path);
  renderChangedFilesPanel();
}

/** 由过程快照重建列表（恢复 / 回退历史后调用）：只有带 `path` 的步骤才算真改动。 */
function rebuildChangedFilesFromSteps(steps: SessionProcess[]) {
  const out: string[] = [];
  for (const g of steps) {
    for (const it of g?.items ?? []) {
      if (it.path && !out.includes(it.path)) out.push(it.path);
    }
  }
  sessionChangedFiles = out;
  renderChangedFilesPanel();
}

/** 一次可点的文件链接。工具卡与面板共用同一套类名，点击走下面那个委托监听。 */
function fileLinkHtml(path: string): string {
  return `<a class="file-link" data-path="${esc(path)}" title="${esc(t("agent.reveal_in_explorer"))}">${esc(path)}</a>`;
}

/** 「本次会话改动过的文件」面板：只在有改动时出现，始终贴在对话流末尾。
 *  用 `appendChild` 复用**同一个**节点 —— 它会把已存在的节点移到末尾，于是新回合开始时
 *  面板自动跟在最新内容之后（不重排历史块，只移动自己这一个节点）。 */
function renderChangedFilesPanel() {
  const log = document.getElementById("chat-log");
  if (!log) return;
  let panel = log.querySelector<HTMLElement>(".changed-files-card");
  if (sessionChangedFiles.length === 0) {
    panel?.remove();
    return;
  }
  if (!panel) {
    panel = document.createElement("details");
    panel.className = "changed-files-card";
    panel.innerHTML =
      `<summary class="changed-files-head"></summary>` + `<ul class="changed-files-list"></ul>`;
  }
  const head = panel.querySelector(".changed-files-head");
  if (head) head.textContent = t("agent.changed_files", { n: String(sessionChangedFiles.length) });
  const ul = panel.querySelector(".changed-files-list");
  if (ul) {
    const hint = esc(t("agent.reveal_in_explorer"));
    ul.innerHTML = sessionChangedFiles
      .map(
        (p) =>
          `<li class="changed-file">` +
          `<a class="file-link" data-path="${esc(p)}" title="${hint}">${esc(pathBase(p))}</a>` +
          `<span class="changed-file-dir" title="${esc(p)}">${esc(pathDir(p))}</span>` +
          `</li>`
      )
      .join("");
  }
  log.appendChild(panel); // 移到末尾（新回合开始后仍贴在最新内容之后）
}

// 事件委托：`.file-link` 都是**动态重绘**的（面板每次重画、工具卡随流式增量重写），
// 逐个绑监听会在重绘后全部失效 —— 挂在 document 上一次即可。
document.addEventListener("click", (e) => {
  const link = (e.target as HTMLElement | null)?.closest<HTMLElement>(".file-link");
  const path = link?.dataset.path;
  if (!path) return;
  e.preventDefault();
  invoke("reveal_in_explorer", { path }).catch((err) => {
    // 路径可能已被移动 / 删除（Rust 侧做了存在性校验）—— 如实告诉用户，别静默失败。
    console.warn("[lunac] reveal_in_explorer failed:", err);
    if (statusText) statusText.textContent = t("agent.reveal_failed");
  });
});

function agentToolArgsDelta(s: string) {
  const v = agentView;
  if (!v || v.currentKind !== "tool" || !s) return;
  v.curToolArgs += s;
  if (v.curToolName === "TodoWrite") {
    // 入参是流式 JSON：分片不完整时解析失败，等下一个分片（末片必定完整）
    const todos = parseTodoArgs(v.curToolArgs);
    if (todos) renderTodoPanel(todos);
    return;
  }
  agentToolInput(v.curToolName, v.curToolArgs);
}

/** 从 TodoWrite 的 tool_use 入参里取 todos（非数组 = 还没拿到完整清单）。 */
function todosFromInput(input: unknown): unknown[] | null {
  const todos = (input as { todos?: unknown } | null)?.todos;
  return Array.isArray(todos) ? todos : null;
}

/** 流式入参是 JSON 的合法前缀，只有完整那一片才解析得出来。 */
function parseTodoArgs(raw: string): unknown[] | null {
  try {
    return todosFromInput(JSON.parse(raw));
  } catch {
    return null;
  }
}

/** 画 TodoWrite 的待办面板：模型每轮发**完整**清单，这里就地重绘同一块面板，
 *  所以同一轮里多次 TodoWrite 只留最后一份状态，不会堆成一摞。 */
function renderTodoPanel(todos: unknown[]) {
  const v = agentView;
  if (!v) return;
  let panel = v.flow.querySelector<HTMLElement>(".todo-panel");
  if (!panel) {
    panel = document.createElement("div");
    panel.className = "todo-panel";
    panel.innerHTML = `<div class="todo-head">📋 <span class="todo-title"></span></div><ul class="todo-list"></ul>`;
    v.flow.appendChild(panel);
  }
  const title = panel.querySelector(".todo-title");
  if (title) title.textContent = t("agent.todo_title");
  const ul = panel.querySelector(".todo-list");
  if (!ul) return;
  ul.innerHTML = todos.map((raw) => {
    const o = (raw ?? {}) as { content?: unknown; status?: unknown };
    const text = typeof o.content === "string" ? o.content : "";
    const status = o.status === "completed" || o.status === "in_progress" ? o.status : "pending";
    const mark = status === "completed" ? "✔" : status === "in_progress" ? "◐" : "○";
    return `<li class="todo-item todo-${status}"><span class="todo-mark">${mark}</span><span class="todo-text">${esc(text)}</span></li>`;
  }).join("");
  agentScroll();
}

function agentCloseBlock() {
  const v = agentView;
  if (!v) return;
  if (v.currentKind === "thinking" && v.curDetails) {
    // 收起的思考只留一行；正文转存到 dataset，展开时按需填回（agent-ui-spec §3.2）。
    // 必须先落 dataset.done 再收起：toggle 回调据此决定是否清空正文节点。
    const det = v.curDetails;
    det.dataset.done = "1";
    det.dataset.thinkText = v.thinkText;
    setThinkLabel(det, "done", v.thinkChars);
    det.open = false;
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

/** core-agent 的 run_shell 把退出码与超时写进了结果文本（协议未变），这里做轻量解析；
 *  解析不到就只显示耗时 —— 降级为不显示，绝不抛错、不打断裂渲染。 */
function parseShellOutcome(txt: string): { exitCode: number | null; timedOut: boolean } {
  const timedOut = /\(timed out after \d+ ms/i.test(txt);
  const m = txt.match(/(?:^|\n)exit code:\s*(\d+)/);
  return { exitCode: m ? Number(m[1]) : null, timedOut };
}

async function copyToClipboard(s: string) {
  try {
    const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
    await writeText(s);
  } catch {
    /* 剪贴板不可用就静默失败：不打断对话流 */
  }
}

function toolActionButton(label: string, text: string): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "tool-action";
  b.innerHTML = `${COPY_SVG}<span>${esc(label)}</span>`;
  b.addEventListener("click", (e) => {
    // 卡片体在 <details> 里：不阻止冒泡会被当成点标题而开合
    e.preventDefault();
    e.stopPropagation();
    copyToClipboard(text);
    const sp = b.querySelector("span");
    if (sp) {
      sp.textContent = t("agent.copied");
      setTimeout(() => {
        sp.textContent = label;
      }, 1200);
    }
  });
  return b;
}

/** 越界（工作区锁）拒绝文案：core-agent 的 guard() 固定输出
 *  `Access denied: <path> is outside the workspace (<cwd>)`（tools.rs）。 */
function parseBoundaryDenial(txt: string): string | null {
  const m = txt.match(/Access denied:\s*(.+?)\s+is outside the workspace/i);
  return m ? m[1] : null;
}

/** 危险命令标签 —— 与审批卡共用同一份黑名单；命中即不给「加入白名单」。 */
function dangerLabelOf(cmd: string): string | null {
  for (const b of CMD_BLACKLIST) {
    if (b.re.test(cmd)) return b.label;
  }
  return null;
}

/** 把命令首词加进用户白名单（与审批卡「始终允许」同一份数据）。 */
function allowlistCommand(cmd: string) {
  const prefix = cmd.trim().split(/\s+/)[0];
  if (!prefix) return;
  const wl = getUserWhitelist();
  if (!wl.bash.includes(prefix)) wl.bash.push(prefix);
  saveUserWhitelist(wl);
}

function refusalButton(label: string, cls: string, onClick: () => void): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.className = `tool-refusal-btn ${cls}`;
  b.textContent = label;
  b.addEventListener("click", (e) => {
    // 卡片体在 <details> 里：不阻止冒泡会被当成点标题而开合
    e.preventDefault();
    e.stopPropagation();
    onClick();
  });
  return b;
}

/** 越界 / 被拒时的用户选项（agent-ui-spec §4.3）：
 *  跳过（= 收起，默认）/ 改到工作区内重试（只回填路径，不自动放宽边界）/
 *  加入命令白名单（仅命令类，且危险命令永不提供）。 */
function appendRefusalActions(card: AgentToolCard, txt: string, isError: boolean) {
  if (!isError) return;
  const body = card.el.querySelector<HTMLElement>(".tool-body");
  if (!body) return;
  const denied = /User denied this action/i.test(txt);
  const outside = parseBoundaryDenial(txt);
  if (!denied && !outside) return;

  const isShell = card.name === "Bash" || card.name === "PowerShell";
  const danger = isShell ? dangerLabelOf(card.cmd || txt) : null;

  const box = document.createElement("div");
  box.className = "tool-refusal";
  const msg = document.createElement("div");
  msg.className = "tool-refusal-msg";
  if (danger) {
    // 危险命令被拦：给可读原因（复用黑名单标签），且不出白名单按钮
    msg.textContent = t("agent.boundary_reason", { reason: danger });
  } else if (outside) {
    msg.textContent = `${t("agent.boundary_blocked")} — ${t("agent.boundary_reason", { reason: outside })}`;
  } else {
    msg.textContent = t("agent.boundary_reason", { reason: t("agent.denied") });
  }

  const btns = document.createElement("div");
  btns.className = "tool-refusal-actions";
  btns.appendChild(refusalButton(t("agent.boundary_skip"), "tool-refusal-skip", () => {
    card.el.open = false;
  }));
  if (outside) {
    btns.appendChild(refusalButton(t("agent.boundary_retry_in_workspace"), "tool-refusal-retry", () => {
      chatInput.value = outside;
      autoResizeChatTextarea();
      chatInput.focus();
    }));
  }
  if (denied && isShell && !danger && card.cmd.trim()) {
    btns.appendChild(refusalButton(t("agent.boundary_add_allowlist"), "tool-refusal-allow", () => {
      allowlistCommand(card.cmd);
      box.remove();
    }));
  }

  box.append(msg, btns);
  body.appendChild(box);
}

/** 把一次 tool_result 填回它对应的卡片：状态 / 退出码 / 耗时 / 输出 / 复制操作。 */
function fillToolCard(card: AgentToolCard, txt: string, isError: boolean) {
  const el = card.el;
  const { exitCode, timedOut } = parseShellOutcome(txt);
  const isShell = card.name === "Bash" || card.name === "PowerShell";
  const firstLine = txt.split("\n")[0].replace(/\s+/g, " ").trim().slice(0, 80);

  el.classList.remove("running");
  const stateEl = el.querySelector<HTMLElement>(".tool-state");
  if (isError) {
    el.classList.add("failed");
    el.open = true; // 失败直接摊开，别让用户再点一次
    if (stateEl) stateEl.textContent = t("agent.tool_failed", { txt: firstLine || "unknown error" });
  } else if (timedOut) {
    el.classList.add("timeout");
    if (stateEl) stateEl.textContent = t("agent.tool_timeout");
  } else if (isShell && exitCode !== null && exitCode !== 0) {
    // 工具本身没失败，是命令返回非零 —— 用黄色，不标红
    el.classList.add("exit-nonzero");
    if (stateEl) stateEl.textContent = t("agent.tool_exit_nonzero", { code: String(exitCode) });
  } else {
    el.classList.add("ok");
    if (stateEl) stateEl.textContent = t("agent.tool_ok");
  }

  // 元信息：退出码（命令类才有）+ 耗时（审批等待已在批准那一刻剔除）
  const metaEl = el.querySelector<HTMLElement>(".tool-meta");
  if (metaEl) {
    const parts: string[] = [];
    if (isShell && exitCode !== null) {
      parts.push(t("agent.tool_exit_code", { code: String(exitCode) }));
    }
    parts.push(t("agent.tool_elapsed", { dur: formatDuration(performance.now() - card.startAt) }));
    metaEl.textContent = "";
    for (const p of parts) {
      const s = document.createElement("span");
      s.className = "tool-meta-item";
      s.textContent = p;
      metaEl.appendChild(s);
    }
  }

  const outEl = el.querySelector<HTMLElement>(".tool-out");
  if (outEl) {
    if (txt) {
      outEl.textContent = txt.length > 600 ? txt.slice(0, 600) : txt;
      outEl.dataset.full = txt;
    } else {
      outEl.remove();
    }
  }

  const actions = el.querySelector<HTMLElement>(".tool-actions");
  if (actions) {
    if (isShell && card.cmd.trim()) {
      actions.appendChild(toolActionButton(t("agent.tool_copy_cmd"), card.cmd));
    }
    if (txt) actions.appendChild(toolActionButton(t("agent.tool_copy_output"), txt));
    if (txt.length > 600) {
      const all = document.createElement("button");
      all.type = "button";
      all.className = "tool-action";
      all.textContent = t("agent.tool_show_all");
      all.addEventListener("click", (e) => {
        e.preventDefault();
        e.stopPropagation();
        const box = el.querySelector<HTMLElement>(".tool-out");
        if (box) box.textContent = box.dataset.full || box.textContent;
        all.remove();
      });
      actions.appendChild(all);
    }
    if (!actions.childElementCount) actions.remove();
  }

  // 越界 / 被拒：补一行用户可选的后续动作（§4.3）
  appendRefusalActions(card, txt, isError);
}

/** Render a tool execution outcome inline — otherwise a failed tool looks
 *  like a silent hang (only the tool row appears, then nothing). */
function agentToolResult(isError: boolean, content: unknown, toolUseId?: string) {
  const v = agentView;
  const host = v?.flow ?? resultsList;
  const txt = extractToolResultText(content).trim();

  // 优先填回它自己的卡片（tool_use_id 配对；端点未给 id 时退到最近一张未完成的卡）
  const card = (toolUseId ? v?.toolCards.get(toolUseId) : undefined) ?? v?.openCard ?? null;
  if (card && !card.done) {
    card.done = true;
    if (v?.openCard === card) v.openCard = null;
    fillToolCard(card, txt, isError);
    if (isError) {
      consecutiveFailures++;
      if (consecutiveFailures >= 3) {
        const warn = document.createElement("div");
        warn.className = "tool-row tool-warn";
        warn.innerHTML = `<strong>${t("agent.warn_tool_failures")}</strong>`;
        host.appendChild(warn);
        consecutiveFailures = 0; // Reset after warning to avoid spam
      }
    } else {
      consecutiveFailures = 0; // 修好了就复位
    }
    agentScroll();
    return;
  }

  // 兜底：没有卡片可配对（旧协议/异常顺序）时沿用一行式渲染
  if (isError) {
    const row = document.createElement("div");
    row.className = "tool-row tool-error";
    row.textContent = `${t("agent.tool_failed", { txt: txt.slice(0, 200).replace(/\s+/g, " ") || "unknown error" })}`;
    consecutiveFailures++;
    if (consecutiveFailures >= 3) {
      const warn = document.createElement("div");
      warn.className = "tool-row tool-warn";
      warn.innerHTML = `<strong>${t("agent.warn_tool_failures")}</strong>`;
      host.appendChild(warn);
      consecutiveFailures = 0;
    }
    host.appendChild(row);
  } else {
    consecutiveFailures = 0;
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

/** 不可白名单化的命令前缀（解释器 / 启动器 / 动态执行）。
 *
 *  白名单是**前缀匹配**（`cmd === p || cmd.startsWith(p + " ")`），一旦把
 *  `powershell` 放进去，以后**任何** `powershell …` 都会被自动放行 —— 等于把
 *  任意代码执行权交出去（`powershell -Command <任意脚本>` 与刚才那条毫无关系）。
 *  同理 `cmd` / `bash` / `python` / `node` / `npx` / `iex` … 因此这些前缀
 *  一律不提供「始终允许」（旧 CLI 在 auto 模式下也是直接剥离这类规则）。
 *
 *  注意：这不影响「危险命令不给白名单」那条 —— 那是按命令**内容**判定，
 *  这里按命令**词**判定，两者是「与」的关系。 */
const NON_WHITELISTABLE_PREFIXES = new Set([
  "cmd", "powershell", "pwsh", "bash", "sh", "zsh", "fish", "wsl",
  "python", "python3", "node", "npx", "npm", "yarn", "pnpm", "bun", "deno",
  "ruby", "perl", "php", "lua", "env", "sudo", "doas", "runas", "su",
  "iex", "invoke-expression", "start", "start-process",
  "wscript", "cscript", "mshta", "rundll32", "regsvr32", "certutil",
]);

/** 命令类请求的首个词（小写、去 .exe），用于判断能否白名单化 */
function cmdPrefix(bashCmd: string): string {
  return (bashCmd.trim().split(/\s+/)[0] || "").toLowerCase().replace(/\.exe$/, "");
}

function canWhitelistCmd(bashCmd: string): boolean {
  const p = cmdPrefix(bashCmd);
  return p !== "" && !NON_WHITELISTABLE_PREFIXES.has(p);
}

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

interface RequestClass {
  auto: boolean;
  danger: string | null;
  opaque: string | null;
  bashCmd: string | null;
  /** 写入内容里扫出的疑似凭据（`规则 (line N)`）—— 见下面的 ③。空 / 缺省 = 没扫到 */
  secrets?: string[];
}

/** agent 随 `can_use_tool` 附上的**执行侧**静态安全分析（见 docs/ai-spec.md §3.5）。
 *  · `dangerous` 非空 → 任何档位都必须人工确认，且不给「始终允许」（危险命令）
 *  · `opaque`    非空 → 含无法静态判定的成分（变量 / 编码执行 / 间接执行器），
 *                      **不得自动放行**（fail-closed）
 *  · `secrets`   非空 → 写入内容（`Write` 的 content / `Edit` 的 new_string）里扫出
 *                      疑似凭据，同样「必须人看」：不自动放行、也不给「始终允许」
 *  · `readonly`  = **可证只读**（A10）：**白名单档**据此自动放行。缺字段或 false
 *                      都按「不放行」处理 —— false 不表示危险，只表示「证不出来」 */
interface AgentAnalysis {
  dangerous?: string[];
  opaque?: string[];
  secrets?: Array<{ rule?: string; line?: number }>;
  readonly?: boolean;
}

/** Classify a permission request: auto-allow (safe/whitelisted), danger
 *  (blacklisted — manual only), opaque (不可静态判定 — 不自动放行), or normal. */
function classifyRequest(
  toolName: string,
  input: unknown,
  analysis?: AgentAnalysis,
): RequestClass {
  const inp = input as Record<string, unknown> | undefined;
  const bashCmd =
    (toolName === "Bash" || toolName === "PowerShell") && typeof inp?.command === "string"
      ? (inp.command as string)
      : null;

  // ① **agent 的执行侧静态分析优先于本地正则**。本地正则只看命令原文，挡不住
  //    引号拼接（`r""m`）/ 包装器（`cmd /c …`）/ 变量（`%TMP%\x.bat`）/ 串联后半段
  //    （`echo hi & shutdown /r`）；判定实现在 core-agent/src/bash_safety.rs。
  const agentDanger = (analysis?.dangerous ?? []).filter(Boolean);
  const agentOpaque = (analysis?.opaque ?? []).filter(Boolean);
  if (agentDanger.length) {
    // 危险命令任何档位都要人工确认（含「自动」档），且不给「始终允许」
    return { auto: false, danger: agentDanger.join("、"), opaque: null, bashCmd };
  }
  // ② 含无法静态判定的成分（变量 / 编码执行 / 间接执行器）→ **不自动放行**（fail-closed：
  //    判不出来就当「要人看」，绝不当「安全」）。
  //
  //    **唯一的例外是「自动」档**（2026-09-20 与后端对齐）。这一档的规范语义就是
  //    「不再弹卡」（agent-ui-spec §4.2「自动档不该弹卡」），而且这一档本来就无门槛放行
  //    `python train.py` 这类任意代码执行 —— 单独让「判不出来」的那类比它更严，结果就是
  //    用户开了「自动」却每条带 `%TMP%` / `cmd /c` 的命令都被拦住，自动档名不副实。
  //    **危险命令（①）不在此列**：它有正面的破坏性证据，任何档位都要人确认。
  if (agentOpaque.length && getRunMode() !== "auto") {
    return { auto: false, danger: null, opaque: agentOpaque.join("、"), bashCmd };
  }
  // ③ 写入内容里扫出疑似凭据 / 密钥（`Write` / `Edit`，实现在
  //    core-agent/src/content_safety.rs）→ 与 ① 同样是「必须人看」：不自动放行、
  //    任何档位都弹卡、也不给「始终允许」（写类工具的「始终允许」= 以后所有
  //    `Write` 都免问，命中过凭据的那次之后更不该开口子）。
  //    **不并进 ① 的 danger**：① 的文案是「危险命令」，与「正常代码里混进了一把
  //    key」是两回事，混用会让用户看不懂到底在问什么。
  //    **不比 ② 更严也不更松**：② 在自动档放行是因为它只是「判不出来」（无证据），
  //    这里是**看见了正面证据**，所以自动档也拦。
  const secrets = (analysis?.secrets ?? [])
    .filter((h) => h?.rule)
    .map((h) => `${h.rule} (line ${h.line ?? "?"})`);
  if (secrets.length) {
    return { auto: false, danger: null, opaque: null, bashCmd, secrets };
  }

  if (bashCmd) {
    for (const b of CMD_BLACKLIST) {
      // 危险命令任何档位都要人工确认（含「自动」档）
      if (b.re.test(bashCmd)) return { auto: false, danger: b.label, opaque: null, bashCmd };
    }
    const mode = getRunMode();
    if (mode === "auto") return { auto: true, danger: null, opaque: null, bashCmd };
    // 手动档：连只读命令也照问不误
    if (mode === "manual") return { auto: false, danger: null, opaque: null, bashCmd };
    const cmd = bashCmd.trim();
    const wl = getUserWhitelist();
    const hit = (p: string) => cmd === p || cmd.startsWith(p + " ");
    // 解释器前缀即便在用户白名单里也不放行 —— 否则「允许过一次
    // `powershell -Command A`」会变成「以后任何 `powershell …` 都自动放行」。
    const userHit = canWhitelistCmd(cmd) && wl.bash.some(hit);
    // ④ A10：白名单档的自动放行改由 **agent 侧的「可证只读」结论**决定，
    //    取代原先那张 `BUILTIN_SAFE_PREFIXES` 前缀表。前缀匹配**看不见重定向与管道**，
    //    `echo hi > important.txt`、`cat a.txt > b.txt` 会被它当「安全前缀」放行 ——
    //    等于零询问地写文件。判据在 core-agent/src/bash_safety.rs（`readonly`）：
    //    单条命令 + 无输出重定向 + 无包装器/命令替换 + 命令词在只读白名单里。
    //    **缺 `analysis` 时按不放行处理**（只认显式的 `true`）—— 与危险/凭据那两档
    //    一样，宁可多问一次。
    if (userHit || analysis?.readonly === true) {
      return { auto: true, danger: null, opaque: null, bashCmd };
    }
    return { auto: false, danger: null, opaque: null, bashCmd };
  }

  // AskUserQuestion 必须由人来选：即使工具名进了白名单也不能自动放行 ——
  // 自动放行 = 回一个空 updatedInput，模型拿不到任何答案（agent 会报
  // "No answer was collected"）。
  if (toolName === "AskUserQuestion") return { auto: false, danger: null, opaque: null, bashCmd: null };

  // ExitPlanMode（A7）**同理且更硬**：它的「允许」= 放行写类工具 + 让模型开始动手，
  // 自动放行等于「计划没人读过就开工」—— 那正是这张卡要防的事。
  // 与 AskUserQuestion 一样，**白名单也免疫**（不在下面 `wl.tools.includes` 那条路上）。
  if (toolName === "ExitPlanMode") return { auto: false, danger: null, opaque: null, bashCmd: null };

  // 非命令类工具同样受运行方式约束（写类四件走这里）
  const mode = getRunMode();
  if (mode === "auto") return { auto: true, danger: null, opaque: null, bashCmd: null };
  if (mode === "manual") return { auto: false, danger: null, opaque: null, bashCmd: null };
  const wl = getUserWhitelist();
  return { auto: wl.tools.includes(toolName), danger: null, opaque: null, bashCmd: null };
}

function respondPermission(
  requestId: string,
  allow: boolean,
  toolUseId?: string,
  updatedInput?: Record<string, unknown>,
  denyMessage?: string,
) {
  // 批准那一刻重新计时：用户在审批卡上犹豫的时间不该算进卡片的「耗时」
  if (allow && toolUseId) {
    const card = agentView?.toolCards.get(toolUseId);
    if (card) card.startAt = performance.now();
  }
  // allow with empty updatedInput = "run with the original input"
  // (explicitly supported: CLI treats {} as use-original)
  // 非空 updatedInput = 覆盖原参数 —— AskUserQuestion 靠它把用户选中的答案带回 agent
  // （agent 侧只有「非空对象才覆盖」，所以答案必须挂在对象里，不能是空 {}）。
  const inner = allow
    ? { behavior: "allow", updatedInput: updatedInput ?? {}, toolUseID: toolUseId }
    : {
        behavior: "deny",
        // 各卡片可以给一句**更贴上下文**的拒因（计划卡用它说明「仍在计划模式」）；
        // 缺省那句是通用兜底（agent 侧拿到什么就原样回灌给模型）。
        message: denyMessage ?? "User denied this action in Lunac",
        interrupt: false,
        toolUseID: toolUseId,
      };
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
const MAX_CMD_GROUP = 20;

/** 连续的命令类审批请求能否并进同一行。
 *
 *  合并**只影响审批展示**：每条命令仍由 agent 各自执行，`&&` 的短路、各自的
 *  退出码与输出都不受影响 —— 所以「含 `&&` / `|` / 重定向 / 换行 的复杂命令」
 *  同样可以合组。2026-09 起不再按算子排除：旧规则会让复杂任务里**同类请求一行
 *  一条地堆满卡片**（用户反馈的「同类型权限请求反复出现」正是这个）。
 *  只留一个宽松的长度上限，避免单条超长命令把一行撑爆。 */
function isMergeableCommand(cmd: string): boolean {
  if (!cmd) return false;
  return cmd.length <= 2000;
}

interface CmdGroupItem extends HTMLElement {
  _groupIds: string[];
  _groupCmds: string[];
  _groupToolUseIds: string[];
  /** 组内出现过的命令类工具名（Bash / PowerShell，去重） */
  _groupToolNames: string[];
  _groupDanger: boolean;
  /** 组内命中的危险标签（去重）—— 标题 tooltip 用 */
  _groupDangerLabels: string[];
  /** 组内出现过「无法静态判定」的命令（变量 / 编码执行…）→ 不留「始终允许」 */
  _groupOpaque?: boolean;
  /** 组内的不透明原因（去重）—— 标题 tooltip 用 */
  _groupOpaqueLabels: string[];
  /** 写入内容里扫出的疑似凭据（`规则 (line N)`）→ 不留「始终允许」，正文里显式列出 */
  _groupSecrets?: string[] | null;
  _groupInput?: unknown;
  _isCmdGroup: boolean;
  _finish: (allow: boolean, always?: boolean) => void;
  /** AskUserQuestion 专用：把当前选中的选项组装成 updatedInput 交给 agent */
  _buildUpdatedInput?: () => Record<string, unknown>;
  /** AskUserQuestion 专用：解决后的提示文案（普通行走 t("agent.approved") 那套） */
  _resolveNote?: (allow: boolean) => string;
}

/** Find the last open command-group row (merge target).
 *
 *  命令类工具（`Bash` / `PowerShell`）视作**同一族** —— 用户要求「短时间内不同类型的
 *  命令也合并进同一次权限运行」，所以这里不再按工具名区分：只要上一行还是**未被应答**
 *  的命令组，新命令就并进去（跨轮是并不了的：下一轮的命令要等上一轮的执行结果才产生）。 */
function findLastCmdGroup(): CmdGroupItem | null {
  let last: CmdGroupItem | null = null;
  for (const [, item] of pendingPermissionCards) {
    const it = item as CmdGroupItem;
    if (it._isCmdGroup && !it.classList.contains("answered")) {
      last = it;
    }
  }
  return last;
}

/** 行标题：合并后工具名可能不止一个（`Bash + PowerShell`），危险 / 不透明标记也可能
 *  来自后来合并进来的那条命令，所以每次合并都重画一次。 */
function renderGroupTitle(item: CmdGroupItem) {
  const el = item.querySelector(".approval-title");
  if (!el) return;
  const dangerHtml = item._groupDanger
    ? `<span class="approval-danger-inline" title="${esc(t("agent.static_danger", { labels: item._groupDangerLabels.join("、") }))}">⛔</span>`
    : "";
  // 无法静态判定 → 也给一个标记，说明「为什么这条要人看」
  const opaqueHtml = item._groupOpaque
    ? `<span class="approval-warn-inline" title="${esc(t("agent.static_opaque", { reasons: item._groupOpaqueLabels.join("、") }))}">⚠</span>`
    : "";
  // 写入内容里扫出疑似凭据 → 与 ⚠ 分开展示：那个是「判不出来」，这个是「看见了东西」
  const secretHtml = item._groupSecrets?.length
    ? `<span class="approval-warn-inline" title="${esc(t("agent.static_secrets", { hits: item._groupSecrets.join("、") }))}">🔑</span>`
    : "";
  el.innerHTML = `${dangerHtml}${opaqueHtml}${secretHtml}<b>${esc(item._groupToolNames.join(" + "))}</b>`;
}

/** AskUserQuestion 的选项界面：单选用互斥高亮、多选可叠加；选中结果写进
 *  item._buildUpdatedInput，由 finish() 经 `updatedInput` 回传给 agent
 *  （agent 侧只把 answers 排成 tool_result，见 core-agent/src/tools.rs）。 */
function renderAskQuestions(host: HTMLElement, input: unknown, item: CmdGroupItem) {
  const inp = (input ?? {}) as Record<string, unknown>;
  const questions = Array.isArray(inp.questions)
    ? (inp.questions as Array<Record<string, unknown>>)
    : [];
  // 题目原文 → 已选中的 label 列表（answers 的键就是题目原文）
  const picked = new Map<string, string[]>();

  for (const q of questions) {
    const qText = String(q.question ?? "");
    const multi = q.multiSelect === true;
    const list = Array.isArray(q.options) ? (q.options as Array<Record<string, unknown>>) : [];
    picked.set(qText, []);

    const wrap = document.createElement("div");
    wrap.className = "approval-ask-q";
    const head = document.createElement("div");
    head.className = "approval-ask-head";
    if (q.header) {
      const chip = document.createElement("span");
      chip.className = "approval-ask-chip";
      chip.textContent = String(q.header);
      head.appendChild(chip);
    }
    head.appendChild(document.createTextNode(qText));
    wrap.appendChild(head);

    const opts = document.createElement("div");
    opts.className = "approval-ask-opts";
    for (const o of list) {
      const label = String(o.label ?? "");
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "approval-ask-opt";
      const lab = document.createElement("span");
      lab.className = "approval-ask-opt-label";
      lab.textContent = label;
      btn.appendChild(lab);
      if (o.description) {
        const desc = document.createElement("span");
        desc.className = "approval-ask-opt-desc";
        desc.textContent = String(o.description);
        btn.appendChild(desc);
      }
      btn.addEventListener("click", () => {
        const cur = picked.get(qText) ?? [];
        picked.set(
          qText,
          multi
            ? cur.includes(label)
              ? cur.filter((x) => x !== label)
              : [...cur, label]
            : [label],
        );
        const now = picked.get(qText) ?? [];
        opts.querySelectorAll(".approval-ask-opt").forEach((el) => {
          const l = el.querySelector(".approval-ask-opt-label")?.textContent ?? "";
          el.classList.toggle("picked", now.includes(l));
        });
      });
      opts.appendChild(btn);
    }
    wrap.appendChild(opts);
    host.appendChild(wrap);
  }

  item._buildUpdatedInput = () => {
    const answers: Record<string, string | string[]> = {};
    for (const [qText, labels] of picked) {
      if (!labels.length) continue; // 没选的题不带答案，agent 会看到「漏答」
      const multi = questions.find((x) => String(x.question ?? "") === qText)?.multiSelect === true;
      answers[qText] = multi ? labels : labels[0];
    }
    // 必须带上 questions：agent 只认「非空对象」为覆盖，空对象 = 用原参数
    return { ...inp, answers };
  };
  item._resolveNote = (allow) => (allow ? "已提交答案" : "已拒绝提问");
}

/** 计划卡（A7）：`ExitPlanMode` 的入参 `plan` 就是整份计划，原样铺在卡里给用户读。
 *
 *  不引 markdown 渲染器 —— 全应用的对话正文都是「转义 + `pre-wrap`」（见 `esc` 与
 *  `.agent-text` 的样式）。计划里那几个 `#` / `-` 用原文反倒更可信：用户看到的就是
 *  模型交出来的那一份，没有任何中间层可能改动它。
 *
 *  用 `textContent` 而不是 `innerHTML`：计划是**模型生成的任意文本**，既不该被当成
 *  HTML 解析，也不该给它任何注入的机会。 */
function renderPlanCard(host: HTMLElement, input: unknown) {
  const inp = (input ?? {}) as Record<string, unknown>;
  const plan = typeof inp.plan === "string" ? inp.plan : "";
  const lines = plan.trim() ? plan.trim().split("\n").length : 0;

  const meta = document.createElement("div");
  meta.className = "approval-plan-meta";
  meta.textContent = t("agent.plan_meta", { lines: String(lines) });
  host.appendChild(meta);

  const box = document.createElement("div");
  box.className = "approval-plan";
  box.textContent = plan;
  host.appendChild(box);
}

/** 批准后把计划留档到 `ModuleData\plans\<本地时间戳>.md`（A7）。
 *
 *  **落盘不是执行的前置**：agent 那边已经拿到批准、开始干活了，这里只负责留一份
 *  「用户看过并批准过」的副本 —— 所以失败只提示，绝不反过来拦住执行。 */
async function persistApprovedPlan(plan: string, host: HTMLElement) {
  const row = document.createElement("div");
  row.className = "tool-row plan-saved";
  try {
    const path = await invoke<string>("save_plan_md", { stamp: localStamp(new Date()), plan });
    row.textContent = t("agent.plan_saved", { path });
  } catch (err) {
    row.textContent = t("agent.plan_save_failed", { err: String(err) });
    row.classList.add("failed");
    console.warn("[plan] save_plan_md failed", err);
  }
  host.appendChild(row);
  agentScroll();
}

/** 计划相位横幅（A7）：**单例节点**，进 / 出都复用它。
 *
 *  为什么是单例：一个会话里可能反复进出（出计划 → 用户拒绝 → 改完再出），每次都插一条
 *  会把对话流冲得乱七八糟；而且「现在到底在不在计划模式」本来就只该有一个答案。
 *  状态**只跟着 agent 的广播走**（system/plan_mode）—— 前端不拿「模型调过哪些工具」自己
 *  推断：那要在「调了工具」与「用户批准了没有」之间做二次判断，很容易和 agent 里那个
 *  真正的标志脱节。 */
let planModeNotice: HTMLElement | null = null;

function renderPlanModeNotice(state: string, reason: string) {
  const host = agentView?.flow ?? resultsList;
  if (!planModeNotice || !planModeNotice.isConnected) {
    planModeNotice = document.createElement("div");
    planModeNotice.className = "plan-mode-note";
    planModeNotice.innerHTML = `<span class="plan-mode-dot"></span><span class="plan-mode-text"></span>`;
    host.appendChild(planModeNotice);
  }
  const on = state === "on";
  planModeNotice.classList.toggle("on", on);
  const label = on ? t("agent.plan_mode_on") : t("agent.plan_mode_off");
  const textEl = planModeNotice.querySelector(".plan-mode-text");
  if (textEl) textEl.textContent = reason ? `${label} · ${reason}` : label;
  agentScroll();
}

/** agent 进程重启 = `plan_phase` 归零（它是进程内状态，不落盘），横幅必须跟着消失 ——
 *  否则界面会一直挂着一条「计划模式中」，而新进程里写类工具其实是放行的。 */
function clearPlanModeNotice() {
  planModeNotice?.remove();
  planModeNotice = null;
}

/** 附件没能随本轮发送的如实提示（A8）。
 *
 *  agent 侧「按路径读图」可能整张读不出来（文件被移走 / 扩展名与内容不符 / 超过单图上限
 *  / 一次给太多张）—— 那些块会**静默消失**在请求里，用户只会看到模型说「我看不到图」。
 *  所以这里把每条失败的**原因**摆在明面上。与 `[Attached files]` 文本互补：文本始终是
 *  路径清单，这条说的是「这一轮实际发出去几张、其余为什么没发」。 */
function renderAttachmentNote(skipped: { path?: string; reason?: string }[]) {
  if (!Array.isArray(skipped) || skipped.length === 0) return;
  const host = agentView?.flow ?? resultsList;
  const det = document.createElement("details");
  det.className = "sys-note sys-note-warn";
  det.innerHTML = `<summary></summary><pre class="sys-note-body"></pre>`;
  const sum = det.querySelector("summary");
  if (sum) sum.textContent = t("agent.attachment_skipped", { n: String(skipped.length) });
  const body = det.querySelector<HTMLElement>(".sys-note-body");
  if (body) {
    body.textContent = skipped
      .map((s) => `${s.path || "?"} — ${s.reason || "?"}`)
      .join("\n");
  }
  host.appendChild(det);
  agentScroll();
}

/** hooks（A9）的提示行：用户脚本的**裁决 / 输出 / 失败**都走这里。
 *
 *  三种 kind 的可见性口径（与 agent 侧「只认显式拒绝、失败不拦但可见」配套）：
 *   · `block` —— hook 拦下了这次工具调用 / 这轮提问（黄色 + 自动展开，必须看见）
 *   · `error` —— hook 崩了 / 超时 / 输出看不懂（**它没有拦任何东西**，但用户必须知道
 *     它没生效，否则「以为装了保护、其实没跑」是最危险的一种状态）
 *   · `info` / `allow` —— 补充信息与放行说明（中性色，默认折叠）
 *  正文每行 = `[kind] text — command`：一个事件挂多个 hook 时靠 command 才分得清是谁。 */
function renderHookNote(data: { hook_event?: string; tool_name?: string; items?: { kind?: string; text?: string; command?: string }[] }) {
  const items = Array.isArray(data.items) ? data.items : [];
  if (items.length === 0) return;
  const warn = items.some((i) => i.kind === "block" || i.kind === "error");
  const host = agentView?.flow ?? resultsList;
  const det = document.createElement("details");
  det.className = warn ? "sys-note sys-note-warn" : "sys-note";
  det.open = warn;
  det.innerHTML = `<summary></summary><pre class="sys-note-body"></pre>`;
  const sum = det.querySelector("summary");
  if (sum) {
    const tag = data.tool_name ? `${data.hook_event || "hook"} · ${data.tool_name}` : (data.hook_event || "hook");
    sum.textContent = t("agent.hook_note", { event: tag, n: String(items.length) });
  }
  const body = det.querySelector<HTMLElement>(".sys-note-body");
  if (body) {
    body.textContent = items
      .map((i) => `[${i.kind || "?"}] ${i.text || ""}${i.command ? ` — ${i.command}` : ""}`)
      .join("\n");
  }
  host.appendChild(det);
  agentScroll();
}

/** 权限卡里命令文本的**显示**归一化（纯排版，不改实际执行的命令）。
 *
 *  模型经常把命令写成「首行空白 + 后续行统一缩进」的多行串，而 `.approval-cmd`
 *  是 `white-space: pre-wrap` —— 于是卡片里第一行是空的、整段看着不贴顶，还平白带着
 *  一层缩进（用户反馈「悬浮在命令行中间、内容没置顶」，要求删掉无效空白/缩进）。
 *  做法：统一换行符 → 去掉首尾空行 → 去掉各非空行的**公共缩进**。
 *  保留换行，多行脚本仍然分行可读。 */
function normalizeCmdForDisplay(cmd: string): string {
  const lines = cmd.replace(/\r\n?/g, "\n").split("\n");
  while (lines.length && !lines[0].trim()) lines.shift();
  while (lines.length && !lines[lines.length - 1].trim()) lines.pop();
  if (!lines.length) return cmd.trim();
  const indents = lines.filter(l => l.trim()).map(l => l.match(/^[ \t]*/)![0].length);
  const common = Math.min(...indents);
  if (common <= 0) return lines.join("\n");
  return lines.map(l => (l.trim() ? l.slice(common) : "")).join("\n");
}

/** Re-render a command group's body (command list + merge count). */
function renderCmdGroupBody(item: CmdGroupItem) {
  // 危险 / 判不出来的命令不留「始终允许」—— 合并进来的**后续**命令可能才是危险的那条，
  // 而按钮是在第一条命令时就画好的，所以在这里兜一次（每次合并都会走到）。
  // 「写入内容含凭据」同理不留（写类工具的「始终允许」= 以后所有 `Write` 都免问）。
  if (item._groupDanger || item._groupOpaque || item._groupSecrets?.length) {
    item.querySelector(".approval-always")?.remove();
  }
  const bodyEl = item.querySelector(".approval-body") as HTMLElement | null;
  if (!bodyEl) return;
  if (item._isCmdGroup) {
    const cmds = item._groupCmds;
    const html = cmds
      .map((c, i) => {
        const sep = i > 0 ? `<span class="approval-cmd-sep">└ </span>` : "";
        // with-sep 只用来给「带 └ 前缀」的行加悬挂缩进（见 styles.css）：
        // 前缀是 inline，不给它挂 padding 的话折行后的续行会左移 2ch。
        const cls = i > 0 ? " with-sep" : "";
        const shown = normalizeCmdForDisplay(c);
        return `<div class="approval-cmd${cls}" title="${esc(shown)}">${sep}<span class="approval-cmd-text">${esc(shown)}</span></div>`;
      })
      .join("");
    const count = cmds.length > 1
      ? `<div class="approval-cmd-merged">${t("agent.cmd_merged", { count: String(cmds.length) })}</div>`
      : "";
    // 命令区**不带**复制按钮（2026-09）：按钮固定在右上角，短命令与它之间会留出
    // 一大片空距（.approval-cmd-list 还得为它预留 56px 右边距）。命令文本本身可选
    // 可复制（.approval-cmd-box 有 user-select: text），功能不丢。
    bodyEl.innerHTML = `
      <div class="approval-cmd-box">
        <div class="approval-cmd-list">${html}</div>
        ${count}
      </div>`;
  } else {
    let preview = "";
    try {
      const s = JSON.stringify(item._groupInput ?? {}, null, 0);
      preview = s.length > 180 ? s.slice(0, 180) + "…" : s;
    } catch {
      preview = String(item._groupInput ?? "");
    }
    // 写入内容里扫出疑似凭据 → 显式列在正文里。**不能只放标题 tooltip**：
    // 这一档的全部价值就是「用户扫一眼卡片时能看见」，藏进 hover 等于没做。
    const secretHtml = item._groupSecrets?.length
      ? `<div class="approval-secret-warn">${esc(
          t("agent.static_secrets_body", { hits: item._groupSecrets.join("、") }),
        )}</div>`
      : "";
    bodyEl.innerHTML = `${secretHtml}<div class="approval-input compact" title="${esc(preview)}">${esc(preview)}</div>`;
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
  analysis?: AgentAnalysis,
) {
  pendingPermissionCards.get(requestId)?.remove();
  const host = agentView?.flow ?? resultsList; // inline, in arrival order
  const cls = classifyRequest(toolName, input, analysis);
  // 计划卡（A7）：走的是与 AskUserQuestion 同一条通道 —— 审批卡 + `updatedInput`，
  // 不新增任何协议字段（见 agent-ui-spec §9 的字段登记原则）。
  // 声明在这里（而不是画正文那一段）：批量卡要不要放宽高度上限，取决于这一批里有没有它。
  const isPlan = toolName === "ExitPlanMode";

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
  // 计划卡要读几十行正文：把**外层**那个滚动容器的高度上限放宽（见 styles.css 的
  // `has-plan`）—— 不给计划框自己加第二层滚动条（嵌套双滚动是本项目明确禁掉的）。
  if (isPlan) permissionBatchCard.classList.add("has-plan");

  // ── Continuous-command merge: fold a follow-up command into the last open
  // Bash/PowerShell approval row instead of a new one ───────────────────
  if (cls.bashCmd && !cls.auto && isMergeableCommand(cls.bashCmd)) {
    const last = findLastCmdGroup();
    if (last && last._groupIds.length < MAX_CMD_GROUP) {
      last._groupIds.push(requestId);
      last._groupCmds.push(cls.bashCmd);
      last._groupToolUseIds.push(toolUseId ?? "");
      if (!last._groupToolNames.includes(toolName)) last._groupToolNames.push(toolName);
      last._groupDanger = last._groupDanger || !!cls.danger;
      if (cls.danger && !last._groupDangerLabels.includes(cls.danger)) {
        last._groupDangerLabels.push(cls.danger);
      }
      last._groupOpaque = last._groupOpaque || !!cls.opaque;
      if (cls.opaque && !last._groupOpaqueLabels.includes(cls.opaque)) {
        last._groupOpaqueLabels.push(cls.opaque);
      }
      if (cls.danger) last.classList.add("danger");
      renderGroupTitle(last);
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
  item._groupToolNames = [toolName];
  item._groupDanger = !!cls.danger;
  item._groupDangerLabels = cls.danger ? [cls.danger] : [];
  item._groupOpaque = !!cls.opaque;
  item._groupOpaqueLabels = cls.opaque ? [cls.opaque] : [];
  item._groupSecrets = cls.secrets ?? null;
  item._groupInput = input;
  item._isCmdGroup = cls.bashCmd !== null;

  // 危险命令永不提供「始终允许」；解释器前缀、「判不出来」的命令、以及「写入内容
  // 里扫出凭据」同样不给 —— 前两者的白名单是**前缀匹配**，放进去等于把「以后任何
  // `powershell …` / `del …`」全自动放行；后者更直接：`Write` 进了白名单 =
  // 以后所有写文件都免问，那正是这条扫描想防的。
  // 计划卡同样不给（A7）：把 `ExitPlanMode` 白名单化 = 以后**每一份计划都自动批准**，
  // 那等于把整个计划模式关掉。
  const alwaysBtn =
    cls.danger ||
    cls.opaque ||
    cls.secrets?.length ||
    isPlan ||
    (cls.bashCmd !== null && !canWhitelistCmd(cls.bashCmd))
      ? ""
      : `<button class="approval-btn approval-always">始终允许</button>`;
  // AskUserQuestion 的「允许」其实是「提交答案」，且不能白名单化
  // （白名单化 = 以后自动回空 updatedInput = 模型永远拿不到答案）
  const isAsk = toolName === "AskUserQuestion";
  item.innerHTML = `
    <div class="approval-title"></div>
    <div class="approval-body"></div>
    <div class="approval-actions">
      <button class="approval-btn approval-allow">${isAsk ? "提交" : isPlan ? t("agent.plan_approve") : "允许"}</button>
      ${isAsk || isPlan ? "" : alwaysBtn}
      <button class="approval-btn approval-deny">拒绝</button>
    </div>`;
  renderGroupTitle(item);
  // 计划卡的标题不走 `renderGroupTitle`（那个拼的是工具名 + 参数摘要）——
  // 用户要看到的是「这是一份待批准的计划」，而不是「ExitPlanMode」。
  if (isPlan) {
    const titleEl = item.querySelector(".approval-title");
    if (titleEl) titleEl.innerHTML = `<b>${esc(t("agent.plan_title"))}</b>`;
  }
  body.appendChild(item);
  if (isAsk) {
    renderAskQuestions(item.querySelector(".approval-body")!, input, item);
  } else if (isPlan) {
    renderPlanCard(item.querySelector(".approval-body")!, input);
  } else {
    renderCmdGroupBody(item);
  }
  agentScroll();

  const finish = (allow: boolean, always = false) => {
    const ids = item._groupIds;
    // Always-allow whitelists the first command's prefix or the tool name
    if (always && !item._groupDanger && !item._groupOpaque) {
      const wl = getUserWhitelist();
      if (item._isCmdGroup && item._groupCmds.length) {
        // 组内**每一条**命令的命令词都进白名单（旧实现只记第一条 → 组里第二条
        // 以后的同类命令下次还要再问一遍，就是「允许过还要再问」）。
        // 解释器/启动器前缀仍然排除（见 canWhitelistCmd）。
        for (const c of item._groupCmds) {
          const prefix = cmdPrefix(c);
          if (prefix && canWhitelistCmd(prefix) && !wl.bash.includes(prefix)) wl.bash.push(prefix);
        }
      } else if (!wl.tools.includes(toolName)) {
        wl.tools.push(toolName);
      }
      saveUserWhitelist(wl);
    }
    ids.forEach((rid, i) => {
      // AskUserQuestion：用户选中的选项要随 allow 一起带回去（覆盖原参数）
      const updated = allow ? item._buildUpdatedInput?.() : undefined;
      // 计划卡被拒 → 给模型的**专用**拒因（见 i18n 的 agent.plan_deny_msg）：
      // 通用那句「User denied」不会告诉它「仍在计划模式」，它会以为可以接着动手，
      // 然后每个写类调用都撞一次 `write_blocked`。
      const denyMsg = isPlan && !allow ? t("agent.plan_deny_msg") : undefined;
      respondPermission(rid, allow, item._groupToolUseIds[i] || undefined, updated, denyMsg);
      pendingPermissionCards.delete(rid);
    });
    // 批准的计划留档（A7）：异步、失败不影响执行，见 persistApprovedPlan。
    // 落点用 `host`（对话流）而**不是** `item.parentElement`（审批卡正文）——
    // 卡在回答后会整个移除，留在卡里的提示会跟着一起消失。
    if (allow && isPlan) {
      const approved = ((item._groupInput ?? {}) as Record<string, unknown>).plan;
      if (typeof approved === "string" && approved.trim()) {
        void persistApprovedPlan(approved, host);
      }
    }
    item.querySelector(".approval-actions")?.remove();
    const note = document.createElement("div");
    note.className = `approval-note ${allow ? "ok" : "no"}`;
    note.textContent = item._resolveNote
      ? item._resolveNote(allow)
      : allow
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
          agentNewBlock("tool", cb.name, cb.id);
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
      // 记下本轮真正生效的模型名 —— 用量日志靠它**分模型**计价（A12 成本面板：
      // 各家单价差十倍，不区分模型算出来的钱没有意义）。agent 每轮查询都会带这个字段，
      // 这里是它唯一的来源（`get_ai_config` 里的模型名只是「配置值」，可能与实际不同）。
      if (typeof data.model === "string" && data.model) agentModel = data.model;
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
    // Context compaction — agent trimmed history to fit the window.
    // 每次压缩都会改写请求前缀、作废端点侧缓存，所以既提示也计数：
    // 计数随用量落盘，用来区分「压缩断裂」与「自然未命中」（ai-spec §11 规则 23）。
    else if (data.type === "system" && data.subtype === "context_compacted") {
      const elided = Number(data.elided ?? 0) || 0;
      const dropped = Number(data.dropped ?? 0) || 0;
      liveCompaction.elided += elided;
      liveCompaction.dropped += dropped;
      statusText.textContent = t("agent.compacted", { elided: String(elided), dropped: String(dropped) });
    }
    // 计划相位（A7）：模型调 `EnterPlanMode` / 用户批准 `ExitPlanMode` 时由 agent 广播。
    // 前端**只镜像**这个状态，不自己推断（理由见 renderPlanModeNotice）。
    else if (data.type === "system" && data.subtype === "plan_mode") {
      renderPlanModeNotice(String(data.state ?? ""), String(data.reason ?? ""));
    }
    // 附件未发送的如实提示（A8）：agent 按路径读图失败 / 超限时才会来这一条
    else if (data.type === "system" && data.subtype === "attachment_note") {
      renderAttachmentNote(Array.isArray(data.skipped) ? data.skipped : []);
    }
    // 权限 hooks（A9）：用户脚本的裁决 / 输出 / 失败，全都如实摆出来
    // （hook 崩了/超时不会拦任何东西，但必须让它**可见** —— 见 renderHookNote）
    else if (data.type === "system" && data.subtype === "hook_note") {
      renderHookNote({
        hook_event: data.hook_event,
        tool_name: data.tool_name,
        items: Array.isArray(data.items) ? data.items : [],
      });
    }
    // Permission request — CLI blocks until we answer: render approval card
    else if (data.type === "control_request" && data.request?.subtype === "can_use_tool" && data.request_id) {
      // 空闲时收到的审批来自**后台复盘 fork**（A4）：它按轮次门槛在**提问之间**跑，
      // 不是任何一次提问的一部分 —— 因此不推状态机（`idle → approval` 本就是非法迁移），
      // 卡片照常显示/自动放行（自动档下它会以「✓ 自动允许」一行出现在流里）。
      if (agentState !== "idle") agentTransition("approval");
      showPermissionCard(
        data.request_id,
        data.request.tool_name || "unknown tool",
        data.request.input,
        data.request.tool_use_id,
        data.request.analysis,
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
    // 子任务收尾：`Agent` 工具与 **fork 技能**（A5）都会发，且都发在工具结果回灌之前 ——
    // 这里不回填的话状态栏会一直停在「子任务执行中」，直到模型下一轮开口才被覆盖。
    // 此刻主循环还在跑（要读子代理的报告再继续），所以落到「工作中」而不是「就绪」。
    // 后台复盘 fork 不发这一对事件（它是无人值守的），因此不会被这里误改成「工作中」。
    else if (data.type === "system" && data.subtype === "task_done") {
      statusText.textContent = t("agent.working");
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
          // TodoWrite 优先用整包入参画面板：端点可能直接在 content_block_start
          // 里给全量 input（此时不会有 input_json_delta），流式分片路径拿不到
          if (block.name === "TodoWrite") {
            const todos = todosFromInput(block.input);
            if (todos) renderTodoPanel(todos);
          }
          if (!cliSawStreamDelta) {
            agentNewBlock("tool", block.name, block.id);
            if (block.input !== undefined) {
              agentToolInput(block.name || "", JSON.stringify(block.input));
            }
            agentCloseBlock();
          }
        }
      }
    }
    // Tool execution results — surface success/failure inline
    else if (data.type === "user" && data.message?.content) {
      for (const block of data.message.content) {
        if (block.type === "tool_result") {
          // Update turn tool call status (Pi: structured tool lifecycle)
          const tc = agentTurn?.toolCalls;
          const lastTool = tc && tc[tc.length - 1];
          // TodoWrite 的成功回执就是那块面板本身 —— 再贴一条「✓ 完成」只会把
          // 整份清单原样重复一遍；失败仍照常显示。
          if (!(lastTool?.name === "TodoWrite" && !block.is_error)) {
            agentToolResult(!!block.is_error, block.content, block.tool_use_id);
          }
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
            requests: Array.isArray(u.requests) ? (u.requests as ChatDoneRequestUsage[]) : [],
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
    // 顺序：**先灌历史、再放行挂起的提问**。agent 是单线程顺序吃 stdin 的，反了这一问
    // 仍然是在零上文里发出的 —— 而「回退 / 恢复历史后立刻追问」正是这个场景
    // （2026-09-17，见 queueAgentHistory 的注释）。
    void flushPendingAgentHistory().then(() => {
      // Retry pending agent chat if CLI just became ready
      const retryQuery = (window as any).__agent_pending_query;
      if (retryQuery) {
        (window as any).__agent_pending_query = null;
        startAgentChat(retryQuery);
      }
    });
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
  appendRuntimeNote(event.payload);
});

// ── 回合折叠与运行端输出（见 docs/agent-ui-spec.md §3.4 / §3.5）──────
/** 本轮开始时间，用于回合汇总里的「耗时」。 */
let turnStartAt = 0;

const runtimeNoteBuffer: string[] = [];
let runtimeNoteEl: HTMLDetailsElement | null = null;

/** 回合完成后是否自动折叠过程（设置面板可关，默认开）。 */
function agentAutoFold(): boolean {
  return localStorage.getItem("lunac-agent-autofold") !== "0";
}

/** 回合开始时重置运行端输出的累积（每轮一条提示块）。 */
function resetRuntimeNote() {
  runtimeNoteBuffer.length = 0;
  runtimeNoteEl = null;
}

/** agent.exe 的 stderr：以前只进 console（release 没有控制台 → 用户什么都看不到）。
 *  现在折成一条系统提示块 —— 与落盘日志互补：日志是事后取证，这条是当场可见。 */
function appendRuntimeNote(line: string) {
  if (!line.trim()) return;
  const host = agentView?.flow ?? resultsList;
  runtimeNoteBuffer.push(line);
  if (!runtimeNoteEl || !runtimeNoteEl.isConnected) {
    const det = document.createElement("details");
    det.className = "sys-note sys-note-error";
    det.innerHTML = `<summary></summary><pre class="sys-note-body"></pre>`;
    host.appendChild(det);
    runtimeNoteEl = det;
  }
  const sum = runtimeNoteEl.querySelector("summary");
  if (sum) sum.textContent = t("agent.runtime_output", { n: String(runtimeNoteBuffer.length) });
  const body = runtimeNoteEl.querySelector<HTMLElement>(".sys-note-body");
  if (body) body.textContent = runtimeNoteBuffer.join("\n");
  agentScroll();
}

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
      item.innerHTML = `
        <div class="result-item-icon" data-icon-path="${esc(app.path)}">${iconText}</div>
        <div class="result-item-content">
          <div class="result-item-title">${esc(app.name)}</div>
          <div class="result-item-desc">${t("chat.launch_app")}</div>
        </div>
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
          <div class="result-item-title">${esc(pluginName(plugin.id, plugin.name))}</div>
          <div class="result-item-desc">${esc(pluginDesc(plugin.id, plugin.description))}</div>
        </div>
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
  invoke("set_ui_mode", { mode: "plugin" }).catch(() => {});

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
// ── 关键词条件式提示注入（System Prompt Injection）───────────────
// 按当前提问的关键词，往**用户消息**里追加一段方法论提示（调试 / TDD / 代码审查）。
//
// 这里**只剩条件块**（2026-09-17 方案 B）。原先还有两块**每次必带**的常量
// （`## Personality` 731 字符 + `## Output Style` 420 字符 ≈ 288 token）—— 它们拼在
// 每条用户消息最前面，位置决定了**每次提问都必然未命中**（新用户消息是全新内容，
// 天生不在上一轮的缓存前缀里）。实测闲聊类提问的首请求未命中量 `in = 236 / 289 / 313`
// token 与这 288 token 几乎相等，即首请求未命中的约 90% 就是它。
// 现已搬进 **agent 的系统提示词**（`PERSONA_AND_STYLE`，见 core-agent/src/main.rs）：
// 那是固定前缀的一部分（进程内逐字节不变，ai-spec §11 规则 18），从此永远命中。
//
// 为什么条件块**不**跟着搬：它们随 query 变，搬进系统提示词会让提示词每轮都变、
// 把整个固定前缀的缓存打掉（比原来更糟）。留在消息尾，只占自己那几十 token。
// 修完这条后 `buildSystemPromptHint()` 可能返回空串 —— 调用方已按「空则不拼分隔符」
// 处理，`cleanUserContent()` 也容忍没有 `\n\n---\n\n` 的消息。

function buildSystemPromptHint(query: string): string {
  const q = query.toLowerCase();
  const hints: string[] = [];

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

async function startAgentChat(query: string, imagePaths: string[] = []) {
  // Reset failure counter for new conversation (ai-spec §17.2)
  consecutiveFailures = 0;
  // File info is already merged into query by startAIChat.
  // (Retry path passes the stored finalQuery directly — 那时没有图片块，附件仍以
  //  `[Attached files]` 文本里的路径交给模型。)
  // A8：`imagePaths` 只装**图片附件**的路径；真正发不发由这里按
  // 「模型支持图片输入」开关（ai.json）决定，见下面拼 content 的地方。
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
    invoke("set_ui_mode", { mode: "plugin" }).catch(() => {});
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
  invoke("set_ui_mode", { mode: "plugin" }).catch(() => {});

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
  turnStartAt = performance.now();
  resetRuntimeNote();

  agentView = {
    turn,
    flow: flowEl,
    current: null,
    currentKind: null,
    curDetails: null,
    curToolName: "",
    curToolArgs: "",
    curToolId: "",
    thinkChars: 0,
    textAll: "",
    toolCards: new Map<string, AgentToolCard>(),
    openCard: null,
    thinkText: "",
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
    // 回合汇总 + 「展开过程」（参照 Trae 的对话流节点自动折叠，见 agent-ui-spec §3.5）
    {
      const errorCount = turn.toolCalls.filter((c) => c.status === "error").length;
      const dur = formatDuration(performance.now() - turnStartAt);
      const footer = doc("div");
      footer.className = "turn-footer";
      const label = doc("span");
      label.className = "turn-tool-count";
      label.textContent =
        turn.toolCalls.length > 0
          ? t("agent.turn_summary", {
              tools: String(turn.toolCalls.length),
              fails: String(errorCount),
              dur,
            })
          : t("agent.turn_elapsed", { dur });
      footer.appendChild(label);

      // 过程快照：把本轮的过程块抽成可持久化的步骤（历史回顾时展示「查看过程」）
      recordTurnSteps(flowEl);

      // 只有这一轮真的产生了「过程」才给折叠按钮
      const processEls = flowEl.querySelectorAll(
        ".think-block, .tool-card, .tool-row, .todo-panel, .sys-note",
      );
      if (processEls.length > 0) {
        const foldBtn = doc("button");
        foldBtn.className = "turn-fold";
        foldBtn.setAttribute("type", "button");
        const setFolded = (folded: boolean) => {
          flowEl.classList.toggle("flow-folded", folded);
          foldBtn.textContent = folded ? t("agent.turn_expand") : t("agent.turn_collapse");
        };
        foldBtn.addEventListener("click", () => {
          setFolded(!flowEl.classList.contains("flow-folded"));
        });
        footer.appendChild(foldBtn);
        // 默认折叠门槛（agent-ui-spec §3.5）：工具调用 ≥2 或过程块 ≥3
        setFolded(agentAutoFold() && (turn.toolCalls.length >= 2 || processEls.length >= 3));
      }
      flowEl.appendChild(footer);
    }

    isStreaming = false;
    setStreamingUI(false);
    agentTransition("done");
    statusText.textContent = t("agent.done", { count: String(turn.toolCalls.length) });
    // result.usage 是**本次提问的绝对值**（agent.exe 每次提问重置计数），不是
    // 会话累计 —— 旧 cli.exe 才是累计值。先前这里按累计做差，会把「前缀没变」
    // 的那几轮命中缓存算成 0（两轮 cache_read 相同 → 差值 0），导致本地命中率
    // 系统性低于供应商平台、无法对账。
    if (info) {
      addUsageToTotals(info);
      // 压缩计数一并计入表盘（本对话口径），面板据此解释命中率
      usageTotals.elided += liveCompaction.elided;
      usageTotals.dropped += liveCompaction.dropped;
      updateTokenDashboard();
      appendUsageLog(info);
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

  // 新提问开始：本轮压缩计数归零（一次提问内 agent 可能报多次压缩）
  liveCompaction = { elided: 0, dropped: 0 };

  try {
    // 图片附件（A8）：只有在设置里断言过「当前模型支持图片输入」时，才把图片附件作为
    // 真正的 `image` 块发出去（块里**只带路径**，字节由 agent 按路径读出来 —— 见
    // ai-spec §3.5「图片附件」）。开关关着时一切照旧。
    // **`[Attached files]` 文本无论如何都保留**：它承载「哪个路径对应哪张图」的对应
    // 关系，也是历史 / 标题 / 复制三条旧路径的唯一依据（契约要求文本不变）。
    const content: { type: string; text?: string; source?: { type: string; path: string } }[] = [
      { type: "text", text: wrappedQuery },
    ];
    if (imagePaths.length > 0) {
      let vision = false;
      try {
        vision = !!(await invoke<{ vision?: boolean }>("get_ai_config"))?.vision;
      } catch { /* 读不到就按关处理：宁可不发图片块，也不要打给不支持的端点 */ }
      if (vision) {
        for (const f of imagePaths) {
          content.push({ type: "image", source: { type: "file", path: f } });
        }
      }
    }
    // Send NDJSON message to CLI (must include session_id and parent_tool_use_id)
    const msg = JSON.stringify({
      type: "user",
      session_id: "",
      message: {
        role: "user",
        content
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
    invoke("set_ui_mode", { mode: "plugin" }).catch(() => {});
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
  // A8：图片附件另外以 `image` 块带给 agent（开关关着 / 非图片类型时不带）。
  await startAgentChat(finalQuery, hasAttach ? fileArr.filter(f => VISION_IMAGE_RE.test(f)) : []);
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
// 设置 → 插件总览：按 id 打开任意插件（2026-09-19 批 9）。
// 设置面板不自己 executePlugin —— 那要动结果区 / 搜索栏状态，是主界面的职责。
(window as any).__lunac_open_plugin = async (id: string) => {
  if (pluginActive) { await closePluginView(); await new Promise(r => setTimeout(r, 50)); }
  const p = pluginRegistry.getAll().find(pl => pl.id === id);
  if (p) { resultsContainer.classList.remove("hidden"); searchBar.classList.add("has-results"); await executePlugin(p); }
};
// 设置 → 用量与成本：「更新价格」把抓取任务交给 agent（A12）。
// 设置面板自己不能发消息（那要动结果区 / 搜索栏状态，是主界面的职责），所以走桥：
// 先关掉设置视图，再把这段提示词当一次普通提问发出去 —— 抓取过程与结果都摆在对话流里，
// 用户能看着 agent 干活（与 __lunac_open_plugin 同一套做法）。
(window as any).__lunac_agent_task = async (prompt: string) => {
  if (pluginActive) { await closePluginView(); await new Promise(r => setTimeout(r, 50)); }
  await startAIChat(prompt);
};
// 插件图标出口（主题包优先）—— 插件总览必须与结果区用同一套图标，
// 否则会出现「结果区线稿 SVG / 设置面板 emoji」两套。
(window as any).__lunac_plugin_icon = pluginIconSvg;

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
  renderMoreMenuLabels();
  renderChatModeSeg();
  renderRunModeUI();
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

/** 自定义右键菜单的一项。`disabled` 用于「无选区 → 复制/剪切置灰」这类按状态
 *  变化的项（置灰项仍渲染，只是点不动 —— 直接隐藏会让菜单忽长忽短）。 */
interface CtxMenuItem {
  label: string;
  action: () => void;
  danger?: boolean;
  disabled?: boolean;
}

function showContextMenu(x: number, y: number, items: CtxMenuItem[]) {
  // 互斥：打开结果菜单前先移除 ws 引擎菜单，避免两菜单同帧同时显示
  document.getElementById("ws-context-menu")?.remove();
  contextMenuEl.innerHTML = items.map(item =>
    `<div class="context-menu-item${item.danger ? " danger" : ""}${item.disabled ? " disabled" : ""}">${esc(item.label)}</div>`
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
      if (items[i]?.disabled) return;   // 置灰项保持菜单打开，不做任何事
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

// ── 搜索栏右键菜单：全选 / 复制 / 剪切 / 粘贴 ──────────────────────
// 全仓右键菜单已在文件顶部统一 preventDefault（WebView2 默认菜单里的
// 「后退/重新加载/检查元素」对桌面工具毫无意义），所以搜索栏原本是「右键没反应」。
// 这里补一个搜索引擎式的四项菜单。要点：
//   ① 复制 / 剪切 依赖选区，无选区时置灰（不隐藏，避免菜单忽长忽短）；
//   ② 粘贴走 Tauri 剪贴板插件（capabilities 已授 `clipboard-manager:allow-read-text`），
//      不用 `navigator.clipboard` —— WebView2 下它另需权限、且未聚焦时直接 reject；
//   ③ 任何改动值的操作都必须补发 `input` 事件：简洁搜索栏靠它同步 Rust 空白态
//      （Esc 判据）与重跑搜索，详细搜索栏靠它重跑查询，漏发 = 界面与状态脱节。

/** 简洁搜索栏是 textarea、详细搜索栏是 input，两者选区 API 相同，用联合类型接收。 */
type SearchBox = HTMLInputElement | HTMLTextAreaElement;

/** 用 text 替换输入框当前选区，光标落在插入内容之后，并补发 input 事件。 */
function replaceInputSelection(el: SearchBox, text: string) {
  const start = el.selectionStart ?? el.value.length;
  const end = el.selectionEnd ?? el.value.length;
  el.value = el.value.slice(0, start) + text + el.value.slice(end);
  const caret = start + text.length;
  el.setSelectionRange(caret, caret);
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

/** 给一个搜索输入框挂上「全选 / 复制 / 剪切 / 粘贴」右键菜单。 */
function attachSearchContextMenu(el: SearchBox) {
  // 转成 HTMLElement 再挂监听：联合类型的 addEventListener 会退化成 Event 重载，
  // 拿不到 MouseEvent 的 clientX/clientY（与 makeInputDragHandle 同一个坑）。
  (el as HTMLElement).addEventListener("contextmenu", (e) => {
    e.preventDefault();
    e.stopPropagation();   // 别冒泡到结果区/全局菜单
    el.focus();
    const start = el.selectionStart ?? 0;
    const end = el.selectionEnd ?? 0;
    const hasSelection = end > start;
    const selected = hasSelection ? el.value.slice(start, end) : "";

    showContextMenu(e.clientX, e.clientY, [
      {
        label: t("ctxmenu.select_all"),
        action: () => el.select(),
      },
      {
        label: t("ctxmenu.copy"),
        disabled: !hasSelection,
        action: () => {
          // 动态 import 与结果区菜单保持同一写法（插件不在启动路径上）
          import("@tauri-apps/plugin-clipboard-manager")
            .then(({ writeText }) => writeText(selected))
            .catch(() => {});
        },
      },
      {
        label: t("ctxmenu.cut"),
        disabled: !hasSelection,
        action: () => {
          import("@tauri-apps/plugin-clipboard-manager")
            .then(({ writeText }) => writeText(selected))
            .catch(() => {});
          replaceInputSelection(el, "");
        },
      },
      {
        label: t("ctxmenu.paste"),
        action: () => {
          import("@tauri-apps/plugin-clipboard-manager")
            .then(({ readText }) => readText())
            .then(text => { if (text) replaceInputSelection(el, text); })
            .catch(() => {});
        },
      },
    ]);
  });
}

attachSearchContextMenu(searchInput);

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

// ══════════════════════════════════════════════════════════════════
// 外观 / 主题（设置 → 风格）— 2026-09-19 新增
// ══════════════════════════════════════════════════════════════════
// 三层，优先级 **用户配置 > 主题包 > styles.css 的 :root 默认值**：
//   ① 配置：localStorage `lunac-appearance`（本机偏好，量小、随 WebView 走）
//   ② 主题包：`<exe 根>\themes\<名>\theme.json`（含图片资产，必须落文件才能
//      导入导出/分享 —— 与 skill / tool 同一套 portable 约束，见 ai-spec 规则 24）
//   ③ 应用：`applyAppearance()` 把 ①② 合成 CSS 变量写在 `<html>` 行内 style 上
//
// **本功能唯一的回归风险点**：`APPEARANCE_DEFAULTS` 必须逐项等于 styles.css
// `:root` 里的原硬编码值（accent #c0a0a0 / blur 4px / saturate 0.92 /
// opacity 0.5 / surfaceAlpha 0.88）。老用户升级时没有这个 localStorage 键，
// 走全默认值 —— 只要上面对得上，渲染结果与改造前逐像素一致。
//
// 为什么走 CSS 变量而不是 JS 逐个改元素 style：主题色被几十处引用（按钮 / 选中态 /
// 边框 / 滚动条 / 审批卡），逐个改必然漏；变量是唯一汇聚点（同 `--results-max-h` 的
// 例外论据：值来自用户配置，写不进样式表）。

interface ThemeAssets {
  background: string | null;
  search_pattern: string | null;
  icons: Record<string, string>;
}
interface ThemeTokens {
  accent: string | null;
  text: string | null;
  text_dim: string | null;
  text_muted: string | null;
  border_glass: string | null;
  surface: string | null;
  radius_search: string | null;
  radius_results: string | null;
  pattern_opacity: number | null;
}
interface ThemeManifest {
  id: string;
  name: string;
  version: string;
  author: string;
  tokens: ThemeTokens;
  assets: ThemeAssets;
}
/** Rust `list_themes()` 的返回项。`resolved` 是**绝对路径**版资产（`assets` 里是
 *  主题目录内的相对路径，前端不该自己拼路径）。 */
interface ThemeInfo {
  manifest: ThemeManifest;
  dir: string;
  builtin: boolean;
  resolved: ThemeAssets;
}

interface Appearance {
  bgImage: string | null;   // 自定义背景图（已 convertFileSrc 的 asset URL）
  bgBlur: number;           // 背景模糊 px
  bgSaturate: number;       // 背景饱和度 %
  bgOpacity: number;        // 背景不透明度 0~1
  sheen: number;            // 玻璃反光强度 0~1
  surfaceAlpha: number;     // 玻璃底色 alpha（界面上叫「底色透明度」，2026-09-20 起归入「底色自定义」）
  /** **「恢复默认主题」**（界面文案；字段名沿用历史的 `tintBase`，改名要动 localStorage
   *  迁移、收益只有可读性）。**默认 false**：
   *  false = 底色 / 按钮线条 / 按钮背景**全部采用用户自定义值**；
   *  true = **回到一开始保存的那套默认主题配色** —— 上面三项的**颜色**不生效，走
   *  `themes/default` + `DEFAULT_ACCENT` 派生的值。
   *  **四项例外（照常生效、照常可调）**：底色透明度、按钮线条透明度、按钮背景透明度
   *  （用户 2026-09-20 「除透明度外都不可调」），以及**文字明度**（同日后续明确
   *  「文字明度不锁定」）。
   *  沿革：`底色跟随主题色` → `主题色代替底色`（2026-09-20）→ **`恢复默认主题`**（同日，
   *  开关移出「底色自定义」、摆到「主题颜色」最顶上）。 */
  tintBase: boolean;
  /** 底色 #rrggbb。**空串 = 用户没动过** ⇒ 仍按主题色自动派生（「没动过就自动派生」那一档）。
   *  取色器色板与「底色饱和度 / 底色明度」两个滑块**共用这一份 HSV**：滑块写的就是色板
   *  的两轴（不再是「微调偏移」，2026-09-20 用户改定）。 */
  baseColor: string;
  /** 按钮线条 #rrggbb。**空串 = 跟随主题色**（默认）⇒ `--btn-line-*` 回落到 :root 的 accent 配方。 */
  btnLineColor: string;
  btnLineAlpha: number;     // 按钮线条透明度 0~1（只作用于「线条」，默认 0.32 == 原 --accent-border）
  /** 按钮背景 #rrggbb。**空串 = 跟随主题色**（默认）⇒ 回落 `--accent-bg` 的配方。
   *  2026-09-20 新增：按钮背景不再跟底色，自己一条。 */
  btnBgColor: string;
  btnBgAlpha: number;       // 按钮背景透明度 0~1（默认 0.14 == 原 --accent-bg 的 α）
  /** 文字明度偏移 ±100。0 = 按主题色派生的原值（逐像素不变）。
   *  **不受「恢复默认主题」管辖**（用户 2026-09-20 明确「文字明度不锁定」）——
   *  它是明度偏移、不是配色本身，开关开着也照常生效。 */
  textLight: number;
  themeId: string;          // "default" = 不套主题包
}

const APPEARANCE_KEY = "lunac-appearance";
/** 外观配置的一次性迁移标记（2026-09-20 新增，配合 `tintBase` 默认值反转）。
 *  用独立键而不是配置内的字段：配置对象每次 `persistAppearance()` 都会整份重写，
 *  放里面的标记会与「是否真的迁移过」脱钩。 */
const APPEARANCE_MIGRATED_KEY = "lunac-appearance-migrated";
/** 兜底主题色：主题包没声明（或声明了非法值）时用它 —— 也就是原 `customAccent` 的默认值。
 *  **主题色本身已没有用户入口**（2026-09-20 用户要求「去除主题色取色」，字段一并删除），
 *  唯一来源是主题包的 `tokens.accent`；这里是磁盘读不到 / 主题损坏时的最后一道。 */
const DEFAULT_ACCENT = "#c0a0a0";
const APPEARANCE_DEFAULTS: Appearance = {
  bgImage: null, bgBlur: 4, bgSaturate: 92, bgOpacity: 0.5,
  sheen: 0, surfaceAlpha: 0.88, tintBase: false, themeId: "default",
  // 三个「空串 = 跟随主题」的默认值：**这是默认外观逐像素不变的关键** ——
  // 空串时 main.ts 会把内联变量清掉，回落到 :root 里那几行 `var(--accent-*)`。
  baseColor: "", btnLineColor: "", btnLineAlpha: 0.32,
  btnBgColor: "", btnBgAlpha: 0.14, textLight: 0,
};
/** 各数值的合法区间：滑块的 min/max 只是 UI 提示，手工改 localStorage 或旧版本
 *  残留都可能给出越界值（`bgOpacity: 5` 会让背景变成纯色块直接盖住面板）。 */
const APPEARANCE_RANGE: Record<string, [number, number]> = {
  bgBlur: [0, 40], bgSaturate: [0, 200], bgOpacity: [0, 1], sheen: [0, 1], surfaceAlpha: [0.3, 1],
  btnLineAlpha: [0, 1], btnBgAlpha: [0, 1], textLight: [-100, 100],
};

let appearance: Appearance = { ...APPEARANCE_DEFAULTS };
/** 主题包缓存：id → ThemeInfo。`list_themes` 只在启动与「打开设置」时拉一次。 */
const themeInfos = new Map<string, ThemeInfo>();

function clampAppearance(a: Appearance): Appearance {
  const out = { ...a };
  for (const [k, [lo, hi]] of Object.entries(APPEARANCE_RANGE)) {
    const v = Number((out as any)[k]);
    (out as any)[k] = Number.isFinite(v) ? Math.min(hi, Math.max(lo, v)) : (APPEARANCE_DEFAULTS as any)[k];
  }
  // 三个颜色字段。它们额外允许**空串这个哨兵值**（= 跟随主题）—— 空串不是坏数据，
  // 不能当非法值回落到默认 hex，否则「没动过取色器」就会被写成 #c0a0a0，
  // 主题色一改而按钮线条不动。
  const hexOr = (v: unknown, allowEmpty: boolean): string | null => {
    if (allowEmpty && v === "") return "";
    return typeof v === "string" && /^#[0-9a-fA-F]{6}$/.test(v) ? v : null;
  };
  out.baseColor = hexOr(out.baseColor, true) ?? APPEARANCE_DEFAULTS.baseColor;
  out.btnLineColor = hexOr(out.btnLineColor, true) ?? APPEARANCE_DEFAULTS.btnLineColor;
  out.btnBgColor = hexOr(out.btnBgColor, true) ?? APPEARANCE_DEFAULTS.btnBgColor;
  if (typeof out.themeId !== "string" || !out.themeId) out.themeId = "default";
  if (out.bgImage !== null && typeof out.bgImage !== "string") out.bgImage = null;
  // tintBase 只接受真布尔
  if (typeof out.tintBase !== "boolean") out.tintBase = APPEARANCE_DEFAULTS.tintBase;
  return out;
}

function loadAppearance(): Appearance {
  let raw: unknown = null;
  try { raw = JSON.parse(localStorage.getItem(APPEARANCE_KEY) || "null"); } catch { /* 坏 JSON 按无配置 */ }
  const merged = { ...APPEARANCE_DEFAULTS, ...(raw && typeof raw === "object" ? raw : {}) } as Appearance;
  // 一次性迁移：改造前背景图单独存在 `lunac-bg-image`（当时只有这一个外观项）。
  // 迁移后立即删旧键 —— 留着它就会变成第二个真相源，以后改背景时两者打对台。
  if (!merged.bgImage) {
    try {
      const legacy = localStorage.getItem("lunac-bg-image");
      if (legacy) { merged.bgImage = legacy; localStorage.removeItem("lunac-bg-image"); }
    } catch { /* localStorage 不可用则保持无背景 */ }
  }
  // 一次性迁移（2026-09-20）：「主题色代替底色」（今称「恢复默认主题」）的默认值由
  // **true 改成 false**。此刻磁盘上存着的 `true` 是**上一版默认值自己写下去的**，
  // 几乎都不是用户的选择，留着它会让「新默认值」对老配置完全失效（表现：底色组 /
  // 按钮组一进去就是灰的）。
  // 因此清一次、让它走新默认值；标记独立存放，保证**只清一次** —— 否则用户以后真的
  // 打开这个开关，重启又会被清掉。
  try {
    if (!localStorage.getItem(APPEARANCE_MIGRATED_KEY)) {
      delete (merged as Partial<Appearance>).tintBase;
      localStorage.setItem(APPEARANCE_MIGRATED_KEY, "1");
    }
  } catch { /* localStorage 不可用则按新默认值 */ }
  return clampAppearance(merged);
}

function persistAppearance() {
  try { localStorage.setItem(APPEARANCE_KEY, JSON.stringify(appearance)); } catch { /* 配额/隐私模式 */ }
}

/** 当前生效的**主题色**。2026-09-20 起唯一来源是主题包的 `tokens.accent`
 *  （用户明确要求「去除主题色取色」，`Appearance.customAccent` 一并删除）：
 *  默认主题 = `themes/default/theme.json` 的 `#c0a0a0`，官方主题 = 它自己声明的那个。
 *  磁盘读不到主题包 / 主题损坏时回落 `DEFAULT_ACCENT`。 */
function themeAccent(): string {
  const a = themeInfos.get(appearance.themeId)?.manifest.tokens.accent;
  return typeof a === "string" && /^#[0-9a-fA-F]{6}$/.test(a) ? a : DEFAULT_ACCENT;
}

/** 当前主题包显式声明的底色（`tokens.surface`）。返回 `"r, g, b"` 或 null。
 *  **这是「主题全面代替底色」的落点**：声明了就以主题包为准（⑤ 步派生时让位），
 *  默认主题已于 2026-09-20 去掉这个 token（用户要求「去除默认主题对底色的影响」）。 */
function themeSurface(): string | null {
  const s = themeInfos.get(appearance.themeId)?.manifest.tokens.surface;
  return s ? toRgbTriplet(s) : null;
}

/** 把 `hsl(h, s%, l%)` 的 l 挪 delta 个百分点（夹到 0~100）。
 *  只认 `derivePalette()` 产出的那种纯灰阶形式；不匹配就原样返回 —— 于是
 *  delta === 0 时**必然逐字节不变**（「文字明度」滑块归零 = 完全复原）。 */
function shiftHslLightness(v: string, delta: number): string {
  const m = /^hsl\(0, 0%, (-?\d+)%\)$/.exec(v);
  if (!m) return v;
  const l = Math.min(100, Math.max(0, Number(m[1]) + delta));
  return `hsl(0, 0%, ${l}%)`;
}

// ── 颜色工具 ─────────────────────────────────────────────────────

function hexToRgb(hex: string): { r: number; g: number; b: number } | null {
  const m = /^#?([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.exec(hex.trim());
  if (!m) return null;
  let h = m[1];
  if (h.length === 3) h = h[0] + h[0] + h[1] + h[1] + h[2] + h[2];
  const n = parseInt(h, 16);
  return { r: (n >> 16) & 0xff, g: (n >> 8) & 0xff, b: n & 0xff };
}

const toHex = (r: number, g: number, b: number) =>
  "#" + [r, g, b].map(v => Math.min(255, Math.max(0, Math.round(v))).toString(16).padStart(2, "0")).join("");

/** `"#1c1a20" | "28,26,32"` → `"r, g, b"`（喂给 CSS 的 `rgb(var(--x))` 用）。 */
function toRgbTriplet(v: string): string | null {
  const hex = hexToRgb(v);
  if (hex) return `${hex.r}, ${hex.g}, ${hex.b}`;
  const parts = v.split(",").map(s => Number(s.trim()));
  if (parts.length === 3 && parts.every(n => Number.isFinite(n))) {
    return parts.map(n => Math.round(Math.min(255, Math.max(0, n)))).join(", ");
  }
  return null;
}

/** rgb → HSL（h 0~360，s/l 0~100）。派生整套配色用，见 `derivePalette`。 */
function rgbToHsl(r: number, g: number, b: number): { h: number; s: number; l: number } {
  const rn = r / 255, gn = g / 255, bn = b / 255;
  const mx = Math.max(rn, gn, bn), mn = Math.min(rn, gn, bn), d = mx - mn;
  let h = 0;
  if (d > 0) {
    if (mx === rn) h = ((gn - bn) / d) % 6;
    else if (mx === gn) h = (bn - rn) / d + 2;
    else h = (rn - gn) / d + 4;
    h *= 60;
    if (h < 0) h += 360;
  }
  const l = (mx + mn) / 2;
  const s = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
  return { h: Math.round(h), s: Math.round(s * 100), l: Math.round(l * 100) };
}

function hslToRgb(h: number, s: number, l: number): { r: number; g: number; b: number } {
  const k = (n: number) => (n + h / 30) % 12;
  const a = s * Math.min(l, 1 - l);
  const f = (n: number) => l - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
  const to = (v: number) => Math.round(Math.min(1, Math.max(0, v)) * 255);
  return { r: to(f(0)), g: to(f(8)), b: to(f(4)) };
}

/** hsl → `"r, g, b"` 三元组（喂给 CSS 的 `rgba(var(--surface-rgb), α)`）。 */
function hslTriplet(h: number, s: number, l: number): string {
  const c = hslToRgb(h, s / 100, l / 100);
  return `${c.r}, ${c.g}, ${c.b}`;
}

/** 主色的**相对亮度**（WCAG 口径，0=黑 1=白）。
 *  判「颜色亮不亮」必须用它而不是 HSL 的 L：HSL 的 L 只看 (max+min)/2，纯黄
 *  `#ffe066` 的 L 是 70%（看着「中等」），但人眼觉得它很亮（相对亮度 ≈0.75）——
 *  用户要的「颜色很亮就调成黑字」用的正是人眼口径。 */
function relativeLuminance(hex: string): number | null {
  const c = hexToRgb(hex);
  if (!c) return null;
  const ch = (v: number) => {
    const x = v / 255;
    return x <= 0.03928 ? x / 12.92 : Math.pow((x + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * ch(c.r) + 0.7152 * ch(c.g) + 0.0722 * ch(c.b);
}

/** 由**一个主色**派生整套界面配色。三条规则（2026-09-19 按用户反馈定稿）：
 *
 *  ① **色相只用在底色一处**（底色跟随主色色相）；**文字一律纯灰阶、不带色相** ——
 *     用户明确要求「不采用红绿蓝的色相调整，只在黑白之间渐变」。
 *  ② 文字明暗**只跟随主色的明暗、方向相反**：主色很亮 → 文字黑；主色很暗 → 文字白。
 *  ③ 底色明度与文字**同步反向**走，否则「亮主色 + 黑字」会落在深底上（不可读）。
 *
 *  转折点 = `DEFAULT_ACCENT`（#c0a0a0）的相对亮度：bright=0 时的
 *  取值逐项等于改造前那套原配色（底色 `28,26,32`、文字近 `#eae2da`），**默认外观因此
 *  不受影响**；主色越亮越往「浅底 + 黑字」走（最亮：底色 83% / 文字 9%）。
 *  中段用 `t = (bright−0.5)×2` 做**过渡带**：亮度落在 0~0.5 区间的颜色一律按深色主题
 *  处理 —— 否则中等亮度的主色会得到「中灰底 + 中灰字」，对比度不够等于看不清。 */
function derivePalette(accentHex: string): Record<string, string> | null {
  const c = hexToRgb(accentHex);
  const lum = relativeLuminance(accentHex);
  if (!c || lum == null) return null;
  const anchor = relativeLuminance(DEFAULT_ACCENT) ?? 0.42;
  const raw = Math.min(1, Math.max(0, (lum - anchor) / Math.max(0.05, 1 - anchor)));
  const t = Math.min(1, Math.max(0, (raw - 0.5) * 2));   // 过渡带：0~0.5 一律深色主题
  const { h, s } = rgbToHsl(c.r, c.g, c.b);
  const surfaceL = Math.round(11 + t * 72);              // 11% → 83%
  // 底色越亮，主色色相越该收敛：浅底上还挂着明显色相会变成一块彩板
  const surfaceS = Math.round(Math.min(16, Math.max(6, s * 0.55)) * (1 - t * 0.55));
  const textL = Math.round(89 - t * 80);                 // 89% → 9%
  const dimL = Math.round(68 - t * 58);
  const mutedL = Math.round(50 - t * 42);
  // 边框用「文字那一侧」的对比色：深色主题下用白、浅色主题下用黑（否则浅底上看不见）
  const inkLight = textL > 50;
  const ink = inkLight ? 255 : 0;
  return {
    "--surface-rgb": hslTriplet(h, surfaceS, surfaceL),
    "--surface-rgb-hover": hslTriplet(h, surfaceS, Math.min(96, surfaceL + 7)),
    "--border-glass": `rgba(${ink}, ${ink}, ${ink}, ${inkLight ? 0.1 : 0.14})`,
    "--text": `hsl(0, 0%, ${textL}%)`,
    "--text-dim": `hsl(0, 0%, ${dimL}%)`,
    "--text-muted": `hsl(0, 0%, ${mutedL}%)`,
    // 中性叠加基色：与文字同侧（深色主题 = 白、浅色主题 = 黑）。
    // 样式表里所有 `rgba(var(--ink-rgb), α)`（hover 底 / 分隔线 / 次级面板）
    // 靠它翻转 —— 浅底上白叠白等于没画，hover 反馈会整片消失。
    "--ink-rgb": inkLight ? "255, 255, 255" : "0, 0, 0",
    // 凹陷底（输入框 / 卡片 / 次级块）压暗强度的缩放：深色主题 1（== 改造前），
    // 浅色主题 0.35（亮底上同样的黑会脏得多）。
    "--shade-scale": inkLight ? "1" : "0.35",
  };
}

/** 用户自定义色的最终值。取色器写进来的 hex 就是最终色 ——
 *  **饱和度 / 明度滑块与色板共用同一份 HSV**（2026-09-20 用户改定，取代早先的
 *  「滑块是微调偏移」），所以这里不再做任何偏移运算，只把 hex 摊成 CSS 要的两种形状：
 *    · `triplet` 喂 `--surface-rgb` / `--btn-line-rgb` 这类 `rgba(var(--x), α)`；
 *    · `hoverTriplet` = 常态 +20/通道（与主题包 surface 的 hover 同一条口径）。 */
function resolveCustomColor(hex: string)
  : { hex: string; triplet: string; hoverTriplet: string } | null {
  const c = hexToRgb(hex);
  if (!c) return null;
  return {
    hex: toHex(c.r, c.g, c.b),
    triplet: `${c.r}, ${c.g}, ${c.b}`,
    hoverTriplet: [c.r, c.g, c.b].map(v => Math.min(255, v + 20)).join(", "),
  };
}

/** 「反差四件套」的取值（分类区域 / 文本框 / 取色器框格 / 结果区选中项）。
 *
 *  做法：**把底色当成主色，跑一遍同一条派生纪律**。为什么不另写一套阈值 —— 四处的
 *  用量都是 3%~14% 的淡洗，明暗方向必须与文字那一套一致；两套各自的「亮 / 暗」分界线
 *  一旦不同，就会出现「底色偏亮时文字是白、反差却是黑」这种自相矛盾的结果。
 *
 *  取的是 `derivePalette` 的 ink（白或黑）而**不是底色本身**：这四处要的是「与底色相反
 *  的一层」，底色已经很暗时再叠一层更暗等于没画。 */
function contrastFor(hex: string) {
  const pal = derivePalette(hex);
  if (!pal) return null;
  return {
    rgb: pal["--ink-rgb"],
    inkRgb: pal["--ink-rgb"],
    // 凹陷层强度统一 0.35，不取 derivePalette 给的 1 —— 白洗叠在暗底上比黑洗叠在暗底上
    // 抢眼得多，用 1 会过冲（25% 白 ≈ 一块灰斑）。0.35 与「浅色主题」那一档同值，两个方向手感一致。
    shadeScale: "0.35",
    borderGlass: pal["--border-glass"],
  };
}

/** 底色的最终值。`baseColor` 是空串 ⇒ 用户没动过 ⇒ null（调用方回落主题派生）。 */
function resolveBaseOverride(): { hex: string; triplet: string; hoverTriplet: string } | null {
  return appearance.baseColor ? resolveCustomColor(appearance.baseColor) : null;
}

/** 写一个 CSS 变量；值为空串时**删除**该内联变量（回落到 :root 的默认值）。
 *  CSSOM 规定 `setProperty(name, "")` 等价于 `removeProperty` —— 靠这条语义
 *  实现「切回默认主题 = 清掉主题包留下的形状/花纹」，不必逐个记变量名。 */
function setVar(name: string, value: string) {
  const s = document.documentElement.style;
  if (value === "") s.removeProperty(name);
  else s.setProperty(name, value);
}

/** 主题色 → CSS 变量。**只写两行**（2026-09-19 批 4 任务 1）：
 *  - `--accent-rgb` = `r, g, b` 三元组，是**唯一真相源**；
 *  - `--accent` = 标准 hex（滚动条 / SVG `currentColor` 等需要完整颜色的地方用）。
 *  `--accent-bg`(0.14) / `--accent-border`(0.32) 以及样式表里各处
 *  `rgba(var(--accent-rgb), x)` 全部由 :root 派生，不再逐个 setVar —— 此前那三行
 *  各写各的，导致「accent 带别的 alpha」只能硬编码 #c0a0a0 的 rgb 分量，
 *  那些按钮/选中态**不跟随主题色**（用户报的 save 按钮问题之一）。 */
function applyAccent(hex: string) {
  const c = hexToRgb(hex);
  if (!c) { setVar("--accent", ""); setVar("--accent-rgb", ""); return; }
  setVar("--accent", toHex(c.r, c.g, c.b));
  setVar("--accent-rgb", `${c.r}, ${c.g}, ${c.b}`);
}

// ── 应用 ─────────────────────────────────────────────────────────

/** 背景图层 `#app-bg-image`（在 #results-container 内部，absolute 贴合结果区 /
 *  插件界面，随面板缩放）。透明与滤镜由 styles.css 的变量控制。 */
function applyBgLayer(image: string | null) {
  const bg = document.getElementById("app-bg-image");
  if (!bg) return;
  bg.style.backgroundImage = image ? `url("${image.replace(/"/g, '\\"')}")` : "";
}

/** 把当前配置 + 当前主题包合成到 CSS 变量上。可反复调用（幂等）。
 *
 *  **顺序即优先级，不许调换**：主题包（设计稿层）先写，用户染色（`tintBase`）后写。
 *  这条顺序是踩出来的 —— 内置 `themes/default/theme.json` 曾经钉了 `surface: #1c1a20`，
 *  而当时主题排在后，于是「底色跟随主题色」永远是 #1c1a20 的冷灰（用户报的
 *  「1C1A20 没跟随、有一层颜色蒙版」就是这层玻璃底色）。 */
function applyAppearance() {
  const theme = themeInfos.get(appearance.themeId);
  const tk = theme?.manifest.tokens;
  const rs = theme?.resolved;

  // ① 背景优先级：**主题自带背景 > 用户图片**。主题要能钉死背景
  //    （「背景图片固定」），所以它排最前。
  applyBgLayer(rs?.background ? convertFileSrc(rs.background)
    : appearance.bgImage ? appearance.bgImage : null);

  // ② 背景滑块 + 玻璃透明度 + 反光：用户配置，任何主题下都生效。
  setVar("--bg-blur", `${appearance.bgBlur}px`);
  setVar("--bg-saturate", String(appearance.bgSaturate / 100));
  setVar("--bg-opacity", String(appearance.bgOpacity));
  setVar("--surface-alpha", String(appearance.surfaceAlpha));
  // 反光滑块 0~1 → 高光 alpha 0~0.22（再亮就成「一块白斑」而不是玻璃反光）
  setVar("--glass-sheen-alpha", String(Math.round(appearance.sheen * 22) / 100));

  // ③ 主题色：**唯一来源是主题包**（2026-09-20 用户要求「去除主题色取色」，设置里的
  //    取色器与 `Appearance.customAccent` 字段一并删除）。原先的「取色方式」（自定义 /
  //    跟随 Windows）连同整条跟随系统链路也已下线 —— Rust 的 `get_system_theme` 命令保留
  //    （那里带着 AccentColor 字节序的实测证据与单测），但前端不再有入口。
  const accent = themeAccent();
  applyAccent(accent);

  // ④ 主题包（设计稿层）：形状 / 花纹 / 颜色 / 玻璃底色。
  //    **每一项都要能被置空**（`setVar(v, "")` 删掉内联变量、回落 :root），
  //    否则切回「默认」主题时上一个主题的形状与颜色会残留。
  setVar("--radius-search", tk?.radius_search ?? "");
  setVar("--radius-results", tk?.radius_results ?? "");
  setVar("--pattern-opacity", tk?.pattern_opacity != null ? String(tk.pattern_opacity) : "");
  setVar("--search-pattern-image", rs?.search_pattern ? `url("${convertFileSrc(rs.search_pattern)}")` : "");
  setVar("--text", tk?.text ?? "");
  setVar("--text-dim", tk?.text_dim ?? "");
  setVar("--text-muted", tk?.text_muted ?? "");
  setVar("--border-glass", tk?.border_glass ?? "");
  // 底色：**「主题全面代替底色」的落点**。主题包声明了 surface 就以它为准，⑤ 步让位；
  // 默认主题已于 2026-09-20 去掉这个 token（用户要求「去除默认主题对底色的影响」），
  // 于是默认主题下这条恒为空、底色完全由派生 / 用户自定义决定。
  const surface = themeSurface();
  setVar("--surface-rgb", surface ?? "");
  // hover 底色 = 常态 +20/通道（原配色 28,26,32 → 48,44,54 正好是这个关系）
  setVar("--surface-rgb-hover", surface
    ? surface.split(",").map(s => String(Number(s.trim()) + 20)).join(", ")
    : "");

  // ⑤ 整套配色派生。四种情况（用户 2026-09-20 四次定稿，本轮把开关改名为
  //    「恢复默认主题」并调整失效范围）：
  //    · 「恢复默认主题」**关着（默认）** → 底色 / 按钮色 / 文字明度**全部可生效**：
  //      没动过底色（baseColor 空串）就仍按主题派生、动过就用用户的值；
  //      主题包声明了 surface 时以它为准；
  //    · 开关开着 → 回到**默认主题**那套配色：底色与按钮颜色全部不生效
  //      （⑤ 的 baseOv 与 ⑤c/⑤d 的 colorsOn 一起归零）；
  //    · **三个透明度不受管辖**（底色 / 按钮线条 / 按钮背景）：用户要求
  //      「开恢复默认主题时除透明度以外的选项都不可调」⇒ 那三项任何状态下都生效，
  //      见 ② 步的 `--surface-alpha` 与 ⑤c/⑤d 的两个 α；
  //    · **文字明度也不受管辖**（用户随后明确「文字明度不锁定」）：它是明度偏移、
  //      不是配色本身 ⇒ `textLight` 照常生效，开关开着也照常可调；
  //    · 文字那一套的**颜色**照旧按主题色派生（用户要求「文字的颜色改变方式不动」）。
  const baseOv = appearance.tintBase ? null : resolveBaseOverride();
  const textDelta = appearance.textLight;
  const pal = derivePalette(accent);
  if (pal) {
    for (const [k, v] of Object.entries(pal)) {
      // 底色已由「用户自定义」或「主题包 surface」定下 ⇒ 派生值让位
      if ((baseOv || surface) && (k === "--surface-rgb" || k === "--surface-rgb-hover")) continue;
      if (textDelta !== 0 && (k === "--text" || k === "--text-dim" || k === "--text-muted")) {
        setVar(k, shiftHslLightness(v, textDelta));
        continue;
      }
      setVar(k, v);
    }
  }
  if (baseOv) {
    setVar("--surface-rgb", baseOv.triplet);
    setVar("--surface-rgb-hover", baseOv.hoverTriplet);
  }
  // ⑤b 「反差四件套」（分类区域 / 文本框 / 取色器框格 / 结果区选中项）：底色暗 ⇒ 这四处
  //     亮、底色亮 ⇒ 这四处暗。见 styles.css `:root` 的 --ctx-* 注释与 ai-spec 规则 45。
  //     没有自定义底色（或开关开着）时**清空**内联值 ⇒ 回落 `:root`，与改造前逐像素一致。
  const ctx = baseOv ? contrastFor(appearance.baseColor) : null;
  setVar("--ctx-rgb", ctx ? ctx.rgb : "");
  setVar("--ctx-ink-rgb", ctx ? ctx.inkRgb : "");
  setVar("--ctx-shade-rgb", ctx ? ctx.inkRgb : "");
  setVar("--ctx-shade-scale", ctx ? ctx.shadeScale : "");
  setVar("--ctx-border-glass", ctx ? ctx.borderGlass : "");
  // ⑤c 按钮线条（设置 → 风格 ·「按钮自定义」）：只写这两个变量，消费方（切换开关 +
  //     若干图标按钮的描边）各自按 α 配方取值。
  //     **颜色**受「恢复默认主题」管辖：开关开着时清掉内联值、回落 `:root` 里的主题配方
  //     （默认配置下这个颜色本来就是空串，所以默认态逐像素不变）。
  //     **透明度不受管辖** —— 用户明确要求开关开着时这三项仍可调（2026-09-20 二次定稿）。
  const colorsOn = !appearance.tintBase;
  const line = colorsOn && appearance.btnLineColor ? resolveCustomColor(appearance.btnLineColor) : null;
  setVar("--btn-line-rgb", line ? line.triplet : "");
  setVar("--btn-line-alpha", String(appearance.btnLineAlpha));
  // ⑤d 按钮背景（2026-09-20 新增，用户要求「按钮背景底色也要能自定义、不再跟底色」）：
  //     作用对象 = 发送 / 停止 / 新建对话 / 更多设置 / 历史记录 / 添加文件 六个按钮的
  //     `background`。btnBgColor 空串 ⇒ 清掉 ⇒ 回落 `--accent-bg` 的配方（逐像素不变）。
  const btnBg = colorsOn && appearance.btnBgColor ? resolveCustomColor(appearance.btnBgColor) : null;
  setVar("--btn-bg-rgb", btnBg ? btnBg.triplet : "");
  setVar("--btn-bg-alpha", String(appearance.btnBgAlpha));
  // 注意 `--surface-alpha`（底色透明度）在 ② 步**无条件**写：它是玻璃质感、不随底色颜色
  // 变化，且历史上属于「背景」组、老配置里可能是非默认值 —— 归进那个开关的失效范围
  // 会把老用户的透明度静默改回 0.88。见 ai-spec 规则 45「失效范围」那一段。

  // ⑥ 主题图标表（`pluginIconSvg` 从这里取；换主题必须重建，否则残留上一个主题的图标）
  themeIconUrls.clear();
  for (const [id, abs] of Object.entries(rs?.icons ?? {})) {
    themeIconUrls.set(id, convertFileSrc(abs));
  }
}

/** 拉一次主题列表并缓存。任何失败都只是「没有主题包」——不能因此挡住外观应用。 */
async function loadThemes(force = false): Promise<ThemeInfo[]> {
  if (!force && themeInfos.size > 0) return [...themeInfos.values()];
  let list: ThemeInfo[] = [];
  try {
    list = await invoke<ThemeInfo[]>("list_themes");
  } catch (e) {
    console.warn("[lunac] list_themes failed (按无主题包处理):", e);
  }
  themeInfos.clear();
  for (const t of list) themeInfos.set(t.manifest.id, t);
  // 兜底「默认」项：磁盘上没有 default 主题时（dev 模式读不到仓库里的 themes/）
  // 也要有一个可选项，否则设置面板里会出现「选不中任何主题」的空档。
  if (!themeInfos.has("default")) {
    themeInfos.set("default", {
      manifest: { id: "default", name: "", version: "", author: "", tokens: {} as ThemeTokens, assets: { background: null, search_pattern: null, icons: {} } },
      dir: "", builtin: true,
      resolved: { background: null, search_pattern: null, icons: {} },
    });
  }
  return [...themeInfos.values()];
}

/** 设置面板的对外接口。**必须挂在 window 上**：settings 是独立插件模块，
 *  与 main.ts 是单向依赖（main.ts 引插件注册表，插件不能反向 import main.ts，
 *  否则循环）。这是既有约定（原 `__lunac_apply_bg` 同一套）。 */
(window as any).__lunac_appearance = {
  get: (): Appearance => ({ ...appearance }),
  /** 改配置：夹取 → 落盘 → 立即应用。`themeId` 变化时顺带重画结果列表，
   *  否则刚换的图标要等下次搜索才出现（插件态下 refreshSearchResults 自身会早退）。 */
  set: (patch: Partial<Appearance>) => {
    const prevTheme = appearance.themeId;
    // 选主题不再需要「顺手写主色」那一段 —— 主题色已**只**由主题包提供，
    // 没有用户侧的第二份值可写（2026-09-20 删 `customAccent`）。
    appearance = clampAppearance({ ...appearance, ...patch });
    persistAppearance();
    applyAppearance();
    // 换主题要重画结果列表（主题图标）；染色/滑块不必（不产生新 DOM）。
    if (patch.themeId !== undefined && patch.themeId !== prevTheme) refreshSearchResults();
  },
  themes: (force?: boolean): Promise<ThemeInfo[]> => loadThemes(force),
  /** 「跟随态」下三个取色器该显示什么色：底色 = 当前生效的表面色（主题包 surface 优先，
   *  否则派生），按钮线条 / 按钮背景 = 当前主题色。设置面板只拿它做**初始显示**、不落盘 ——
   *  派生逻辑只有这一份，面板复制一份必然漂移。 */
  resolvedSwatches: (): { base: string; btnLine: string; btnBg: string } => {
    const accent = themeAccent();
    let base = accent;
    const surf = themeSurface();
    if (surf) {
      const parts = surf.split(",").map(s => Number(s.trim()));
      if (parts.length === 3 && parts.every(Number.isFinite)) base = toHex(parts[0], parts[1], parts[2]);
    } else {
      const parts = (derivePalette(accent)?.["--surface-rgb"] ?? "").split(",").map(s => Number(s.trim()));
      if (parts.length === 3 && parts.every(Number.isFinite)) base = toHex(parts[0], parts[1], parts[2]);
    }
    return { base, btnLine: accent, btnBg: accent };
  },
  themesDir: (): Promise<string> => invoke<string>("themes_dir"),
  /** 选择背景图（设置面板只管选文件，落配置与应用都走这里，避免两处实现漂移）。 */
  pickBgImage: async (): Promise<boolean> => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({
        multiple: false,
        title: t("settings.appearance_bg_apply"),
        filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "bmp", "gif", "avif"] }],
      });
      if (typeof picked !== "string" || !picked) return false;
      appearance = clampAppearance({ ...appearance, bgImage: convertFileSrc(picked) });
      persistAppearance();
      applyAppearance();
      return true;
    } catch (e) {
      console.error("[lunac] pick background failed:", e);
      return false;
    }
  },
};

// 启动即应用：**先同步**应用用户配置（背景图/滑块/主题色立刻生效，避免首帧闪一下
// 默认外观），再去拉主题包并补一次（主题包读文件是异步的）。
appearance = loadAppearance();
applyAppearance();
void (async () => {
  await loadThemes(true);
  applyAppearance();
})();

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
  // 强制重新断言高度：隐藏期间窗口高可能已被改动，而 `requestedHeight` 缓存会让
  // 本轮测量被静默跳过（2026-09-19，见 forceHeightReassert 的注释）。
  forceHeightReassert();
  // 大界面不跨「隐藏 → 唤出」存活：唤出（热键 / 双击图标）一律回到简洁搜索 ——
  // 否则用户下次唤出面对的是上次留下的大界面，还占着 640 的固定高度。
  if (detailOpen) exitDetailSilently();
  // 唤出时再无条件重报一次界面层：前端一定处在已知状态，而 Rust 那份可能因为
  // 上一次「隐藏时正在换层」或 WebView 重载而残留旧值 —— Esc 是唤出后最常按的键，
  // 这里对齐一次最划算（详见 syncUiMode 的注释）。
  syncUiMode();
  // 唤出即主动重跑一次当前查询（2026-09-17 新增）。
  // Rust 侧的唤出路径**不会**重跑搜索：`hide_window()` 只有一行 `ShowWindow(SW_HIDE)`
  // （不清结果、不发事件），唯一的重算链是「剪贴板事件 → 合成 input → 60ms 去抖」，
  // 而剪贴板没变化时整段不执行 —— 高负载下用户看到的就是「呼出后卡在上次搜索结果」
  // 的静态帧。这里主动补一次：成本与按一次键相同（`search_apps` 只读
  // `app-index-cache.json`，永不扫描目录），静态帧最迟 60ms 后被新结果换掉。
  // 复用 `refreshSearchResults()`（内部就是 dispatch input）而不是直接调
  // `runSearchNow`：走同一套去抖 + seq 校验，不会与正在输入的字抢渲染。
  if (!pluginActive && !drawerVisible) refreshSearchResults();
  triggerJSClipboardRead();
});

// 启动即自报一次界面层：脚本刚跑完 = 一定是简洁搜索，而 Rust 进程可能还是上一次
// 会话残留的 plugin / detail（前端刷新但进程没重启就会错位）。这是最便宜的自愈点。
syncUiMode();

// Also try reading clipboard on initial startup
setTimeout(triggerJSClipboardRead, 800);

// ══════════════════════════════════════════════════════════════════
// 详细搜索大界面（双击搜索栏进入）
// 规范：docs/ai-spec.md §2.1.2。四条设计约束：
//   ① **简洁搜索的任何行为都不改**：进大界面只是叠一层视图；退出时把查询词带回
//      搜索栏并重跑一次简洁搜索，结果区照旧。
//   ② 数据源四个：应用索引（`search_apps`）/ 文件索引（`search_files`）/
//      Windows 设置页与系统动作（`system_catalog`）/ 插件命令（pluginRegistry）。
//   ③ 序号（seq）latest-wins，与简洁搜索同一纪律：快速键入丢弃过期回包。
//   ④ 窗口高度固定档 `DETAIL_HEIGHT × zoom`，且**不进高度滑动动画** —— 与插件态
//      同策略（离散切换），滑动只属于搜索/结果态（见 ai-spec §3.2 视窗规则）。
// ══════════════════════════════════════════════════════════════════

/** 文件索引条目（Rust `file_indexer::FileEntry`） */
interface DetailFile {
  name: string;
  path: string;
  kind: string;
  ext: string;
  /** 修改时间（毫秒时间戳；读不到为 0）。Rust `file_indexer::FileHit` 只给这一个元数据。 */
  modified: number;
}

/** 系统设置页 / 系统动作条目（Rust `system_catalog::CatalogItem`） */
interface DetailCatalogItem {
  id: string;
  kind: string;
  icon: string;
  title_zh: string;
  title_en: string;
  target: string;
  danger: boolean;
  /** 该动作有「以管理员身份运行」形态（Rust 由 `action_spec()` 推导，别在前端重算） */
  elevatable: boolean;
  keywords: string[];
}

interface DetailIndexStatus {
  count: number;
  scanning: boolean;
  saved_ms: number;
  truncated: boolean;
  roots: string[];
}

type DetailCat = "all" | "apps" | "files" | "settings" | "actions" | "commands";
type DetailRowCat = Exclude<DetailCat, "all"> | "web";

interface DetailRow {
  cat: DetailRowCat;
  app?: AppEntry;
  file?: DetailFile;
  item?: DetailCatalogItem;
  plugin?: Plugin;
  query?: string;
}

const DETAIL_CATS: DetailCat[] = ["all", "apps", "files", "settings", "actions", "commands"];
/// 文件类型筛选（"" = 全部类型；值必须与 Rust `kind_for` 的分类一一对应）
const DETAIL_KINDS = ["", "folder", "document", "image", "video", "audio", "archive", "program", "other"];
/// 文件结果条数上限（其余分类体量都很小，不需要上限）
/// 2026-09-19：40 → 200（= Rust 侧 `file_indexer::MAX_RESULTS` 的硬上限）。
/// 用户反馈「只能固定到这么多」—— 40 条对「找文件」这个主用途太窄；
/// 200 条是后端本来就允许的上限，前端不再自我设限。
const DETAIL_FILE_LIMIT = 200;
/// 应用结果上限：6 → 20（同上，开始菜单条目通常几十个，20 条足够覆盖一次检索）
const DETAIL_APP_LIMIT = 20;
/// 插件命令结果上限：6 → 12
const DETAIL_PLUGIN_LIMIT = 12;
/// 设置页 / 系统动作结果上限：8 → 20（目录一共六十来条）
const DETAIL_CATALOG_LIMIT = 20;
/// 能交给 `ShellExecuteW(runas)` 提权的扩展名。**判据必须与 Rust 侧同源**：
/// `system_catalog` 的动作看各自的 `elevatable` 字段，应用/文件看这张表。
/// 文件夹、`ms-settings:` 页、网页搜索都没有提权形态。
const DETAIL_ELEVATABLE_EXTS = ["exe", "lnk", "msc", "cpl", "bat", "cmd", "com"];
/// 结果区分组顺序（Tab「全部」时按这个顺序分组显示）
const DETAIL_GROUPS: Array<{ cat: DetailRowCat; key: string }> = [
  { cat: "apps", key: "detail.group_apps" },
  { cat: "files", key: "detail.group_files" },
  { cat: "settings", key: "detail.group_settings" },
  { cat: "actions", key: "detail.group_actions" },
  { cat: "commands", key: "detail.group_commands" },
  { cat: "web", key: "detail.group_web" },
];

const detailPanel = el("detail-panel");
const detailInput = el("detail-input") as HTMLInputElement;
const detailTabs = el("detail-tabs");
const detailKinds = el("detail-kinds");
const detailResults = el("detail-results");
const detailPreview = el("detail-preview");
const detailCount = el("detail-count");
const detailHint = el("detail-hint");
const detailIndexEl = el("detail-index");
const detailBackBtn = el("detail-back-btn");
const detailRefreshBtn = el("detail-refresh-btn");

let detailSeq = 0;            // latest-wins 序号
let detailCat: DetailCat = "all";
let detailKind = "";          // 文件类型筛选
let detailAllRows: DetailRow[] = [];  // 本次查询的全部结果（未按分类筛）
let detailRows: DetailRow[] = [];     // 当前可见（已按分类筛）—— 键盘导航就按它走
let detailSel = 0;
let detailCatalog: DetailCatalogItem[] = [];
let detailStatus: DetailIndexStatus = { count: 0, scanning: false, saved_ms: 0, truncated: false, roots: [] };
let detailEntryQuery = "";    // 进大界面时简洁搜索里的查询词（退出时若未输入则还原）
let detailArmed = "";         // 危险动作二次确认中的 id
let detailArmedTimer: ReturnType<typeof setTimeout> | null = null;
let detailStatusTimer: ReturnType<typeof setInterval> | null = null;
/** 「按键提示行临时改文案」的定时器（提权被拒等必须让用户看见的一次性反馈） */
let detailHintTimer: ReturnType<typeof setTimeout> | null = null;
let detailIconToken = 0;      // 图标异步回填的失效令牌（重渲染后旧回包作废）

/** 把按键提示行临时改成一条反馈文案，3 秒后恢复。
 *
 *  详细搜索大界面里**没有 toast 设施**，而「以管理员身份运行」失败（用户点了 UAC 的
 *  「否」、或策略禁止）是**必须被看见**的：静默失败与「点了没反应」在用户眼里一样。
 *  提示行是唯一常驻且位置合适的文本位。 */
function flashDetailHint(msg: string) {
  if (detailHintTimer) clearTimeout(detailHintTimer);
  detailHint.textContent = msg;
  detailHint.classList.add("detail-hint-error");
  detailHintTimer = setTimeout(() => {
    detailHintTimer = null;
    detailHint.classList.remove("detail-hint-error");
    if (detailOpen) detailHint.textContent = t("detail.hint_keys");
  }, 3000);
}

function clearDetailHint() {
  if (detailHintTimer) {
    clearTimeout(detailHintTimer);
    detailHintTimer = null;
  }
  detailHint.classList.remove("detail-hint-error");
}

/** 目录条目的显示名：系统语言是中文就用中文名，其余语言用英文名。
 *  为什么不给五种语言：Windows 设置页的名字是 OS 自己的资源，我们拿不到官方译名。 */
function detailCatalogTitle(it: DetailCatalogItem): string {
  return lang.startsWith("zh") ? it.title_zh : it.title_en;
}

function detailKindLabel(kind: string): string {
  return t(kind ? `detail.kind_${kind}` : "detail.kind_all");
}

/** 模糊（子序列）兜底的最低查询长度。
 *  对齐 Win11 新版搜索的「2 字符起」：1 个字符做子序列匹配等于把整张表都拉进来，
 *  那不叫容错、叫没过滤。 */
const DETAIL_FUZZY_MIN_CHARS = 2;

/** 子序列打分：查询的字符按**顺序**出现在目标里即命中，允许中间插字。
 *  语义与 Rust `file_indexer::score_subsequence` 故意保持一致 ——
 *  `utlook` 命中 Outlook、`instaled` 命中 Installed apps，这是 Win11 新版搜索
 *  最被称道的一条（拼错不再直接丢给网页搜索）。
 *  分数刻意压在「包含」档（30）之下：模糊命中是兜底，**永远不能把精确命中挤下去**。 */
function scoreSubsequence(hay: string, q: string): number {
  let i = 0;
  let first = -1;
  let last = -1;
  for (let j = 0; j < hay.length && i < q.length; j++) {
    if (hay[j] === q[i]) {
      if (first < 0) first = j;
      last = j;
      i++;
    }
  }
  if (i < q.length) return 0;                     // 还有字符没匹配上
  const gaps = last - first + 1 - q.length;       // 跨度里被跳过的字符数
  return 20 - Math.min(12, gaps);
}

/** 目录（设置页 + 系统动作）本地匹配：几十条数据，不必走后端。
 *
 *  两轮（2026-09-19 批 5 任务 3c 补第二轮）：**第一轮**精确 / 前缀 / 包含 / 多词全中；
 *  只有第一轮没凑够上限才跑**第二轮**子序列兜底 —— 与 `file_indexer::rank` 同一套门控，
 *  单字符查询在第一轮就已命中该命中的，不必再扫一遍全表。 */
function matchDetailCatalog(q: string): DetailCatalogItem[] {
  if (!q) return [];
  const tokens = q.toLowerCase().split(/\s+/).filter(Boolean);
  if (!tokens.length) return [];
  const scored: Array<{ it: DetailCatalogItem; score: number }> = [];
  const taken = new Set<string>();
  for (const it of detailCatalog) {
    const zh = it.title_zh.toLowerCase();
    const en = it.title_en.toLowerCase();
    const hay = [it.id.toLowerCase(), zh, en, ...it.keywords.map(k => k.toLowerCase())];
    let total = 0;
    let all = true;
    for (const tk of tokens) {
      const hit = hay.find(h => h.includes(tk));
      if (!hit) {
        all = false;
        break;
      }
      total += hit === zh || hit === en ? 100 : hit.startsWith(tk) ? 60 : 30;
    }
    if (all) {
      scored.push({ it, score: total });
      taken.add(it.id);
    }
  }
  if (scored.length < DETAIL_CATALOG_LIMIT && tokens.every(tk => tk.length >= DETAIL_FUZZY_MIN_CHARS)) {
    for (const it of detailCatalog) {
      if (taken.has(it.id)) continue;
      const hay = [it.title_zh.toLowerCase(), it.title_en.toLowerCase(), ...it.keywords.map(k => k.toLowerCase())];
      let total = 0;
      let all = true;
      for (const tk of tokens) {
        let best = 0;
        for (const h of hay) best = Math.max(best, scoreSubsequence(h, tk));
        if (!best) {
          all = false;
          break;
        }
        total += best;
      }
      if (all) scored.push({ it, score: total });
    }
  }
  scored.sort((a, b) => b.score - a.score);
  return scored.slice(0, DETAIL_CATALOG_LIMIT).map(s => s.it);
}

/** 该结果是否支持「以管理员身份运行」。**必须与 Rust 侧同源**（见 DETAIL_ELEVATABLE_EXTS
 *  与 `system_catalog::CatalogItem::elevatable`）：界面画了盾牌而 Rust 拒绝执行，
 *  或者反过来 —— 都会让用户觉得「这个按钮是坏的」。 */
function detailElevatable(row: DetailRow): boolean {
  switch (row.cat) {
    case "actions":
      return !!row.item?.elevatable;
    case "settings":
      return false;
    case "apps":
      // 开始菜单条目基本都是 .lnk（runas 到 .lnk 会作用到它指向的目标）；文件夹排除掉
      return detailElevatablePath(row.app!.path);
    case "files":
      // `kind === "folder"` 优先于扩展名 —— 「名为 xxx.exe 的目录」不该显示盾牌
      if (row.file!.kind === "folder") return false;
      return detailElevatablePath(row.file!.path);
    default:
      return false; // 插件命令 / 网页搜索没有提权形态
  }
}

function detailElevatablePath(path: string): boolean {
  const ext = (path.match(/\.([a-zA-Z0-9]+)$/)?.[1] || "").toLowerCase();
  return DETAIL_ELEVATABLE_EXTS.includes(ext);
}

async function ensureDetailCatalog() {
  if (detailCatalog.length) return;
  try {
    detailCatalog = await invoke<DetailCatalogItem[]>("system_catalog");
  } catch {
    detailCatalog = [];
  }
  if (detailOpen) void runDetailSearch();
}

// ── 进入 / 退出 ──────────────────────────────────────────────────

function enterDetail() {
  if (detailOpen || pluginActive) return;
  detailOpen = true;
  detailEntryQuery = searchInput.value;
  detailInput.value = detailEntryQuery;
  clearDetailHint();
  detailInput.placeholder = t("detail.placeholder");
  detailHint.textContent = t("detail.hint_keys");
  detailPanel.classList.remove("hidden");
  document.getElementById("app")!.classList.add("detail-mode");
  // 告诉 Rust「当前在详细搜索这一层」——否则它按「query/chips 空」判空，
  // 一下 Esc 就把整个窗口隐藏了（2026-09-15 修的 bug，见 hotkey.rs 的 UI_MODE）。
  invoke("set_ui_mode", { mode: "detail" }).catch(() => {});
  detailCat = "all";
  detailSel = 0;
  renderDetailFilters();
  applyWindowSize();
  startDetailStatusPolling();
  void ensureDetailCatalog();
  void refreshDetailStatus();
  void runDetailSearch();
  detailInput.focus();
  detailInput.select();
}

/** 退出大界面但**不**重跑简洁搜索 —— 用于「立刻要隐藏窗口 / 交给插件接管」的路径。 */
function exitDetailSilently() {
  if (!detailOpen) return;
  detailOpen = false;
  stopDetailStatusPolling();
  clearDetailArmed();
  clearDetailHint();
  document.getElementById("app")!.classList.remove("detail-mode");
  detailPanel.classList.add("hidden");
  applyWindowSize();
  // 同步 Rust 的界面层（Esc 的隐藏判据）。exitDetail() 也走这里，故一处即可。
  invoke("set_ui_mode", { mode: "main" }).catch(() => {});
}

/** 退出大界面并回到简洁搜索（返回按钮 / 唤出窗口时走这条）。 */
function exitDetail() {
  if (!detailOpen) return;
  const q = detailInput.value.trim() ? detailInput.value : detailEntryQuery;
  exitDetailSilently();
  searchInput.value = q;
  autoResizeTextarea();
  if (!pluginActive) {
    searchInput.focus();
    searchInput.dispatchEvent(new Event("input", { bubbles: true })); // 重跑简洁搜索
  }
}

// ── 查询 ─────────────────────────────────────────────────────────

async function runDetailSearch() {
  if (!detailOpen) return;
  const seq = ++detailSeq;
  const q = detailInput.value.trim();
  const [apps, files] = await Promise.all([
    invoke<AppEntry[]>("search_apps", { query: q, limit: DETAIL_APP_LIMIT }).catch(() => [] as AppEntry[]),
    invoke<DetailFile[]>("search_files", { query: q, kind: detailKind || null, limit: DETAIL_FILE_LIMIT })
      .catch(() => [] as DetailFile[]),
  ]);
  if (seq !== detailSeq || !detailOpen) return; // 过期回包直接丢（latest-wins）
  const rows: DetailRow[] = [];
  for (const a of apps) rows.push({ cat: "apps", app: a });
  for (const f of files) rows.push({ cat: "files", file: f });
  for (const it of matchDetailCatalog(q)) {
    rows.push({ cat: it.kind === "setting" ? "settings" : "actions", item: it });
  }
  for (const p of pluginRegistry.search(q).slice(0, DETAIL_PLUGIN_LIMIT)) rows.push({ cat: "commands", plugin: p });
  if (q) rows.push({ cat: "web", query: q });
  detailAllRows = rows;
  if (detailSel >= rows.length) detailSel = 0;
  renderDetail();
}

// ── 渲染 ─────────────────────────────────────────────────────────

function renderDetailFilters() {
  detailTabs.replaceChildren(
    ...DETAIL_CATS.map(c => {
      const chip = doc("div");
      chip.className = `detail-chip${c === detailCat ? " active" : ""}`;
      chip.textContent = t(`detail.tab_${c}`);
      chip.addEventListener("click", () => setDetailCat(c));
      return chip;
    })
  );
  detailKinds.replaceChildren(
    ...DETAIL_KINDS.map(k => {
      const chip = doc("div");
      chip.className = `detail-chip${k === detailKind ? " active" : ""}`;
      chip.textContent = detailKindLabel(k);
      chip.addEventListener("click", () => {
        if (detailKind === k) return;
        detailKind = k;
        renderDetailFilters();
        void runDetailSearch();
      });
      return chip;
    })
  );
}

/** 切分类只筛已有结果，不重新查询（数据在 detailAllRows 里，切 Tab 要瞬时）。 */
function setDetailCat(cat: DetailCat) {
  detailCat = cat;
  detailSel = 0;
  renderDetailFilters();
  renderDetail();
}

function renderDetail() {
  detailIconToken++;
  const token = detailIconToken;
  detailRows = detailCat === "all" ? detailAllRows.slice() : detailAllRows.filter(r => r.cat === detailCat);
  if (detailSel >= detailRows.length) detailSel = Math.max(0, detailRows.length - 1);

  const frag = document.createDocumentFragment();
  if (!detailRows.length) {
    const empty = doc("div");
    empty.className = "detail-group-title";
    empty.textContent = t("detail.no_results", { q: detailInput.value.trim() });
    frag.appendChild(empty);
  }
  let lastCat: DetailRowCat | null = null;
  detailRows.forEach((row, i) => {
    if (row.cat !== lastCat) {
      lastCat = row.cat;
      const group = DETAIL_GROUPS.find(g => g.cat === row.cat);
      const title = doc("div");
      title.className = "detail-group-title";
      title.textContent = group ? t(group.key) : "";
      frag.appendChild(title);
    }
    frag.appendChild(buildDetailItem(row, i, token));
  });
  detailResults.replaceChildren(frag);
  detailCount.textContent = detailRows.length ? String(detailRows.length) : "";
  renderDetailStatus();
  scrollDetailSelectionIntoView();
  renderDetailPreview();
}

/** 父目录（结果条目的副行文案；根目录时原样回显盘符）。 */
function parentDir(path: string): string {
  const cut = path.replace(/[\\/][^\\/]*$/, "");
  return cut && cut !== path ? cut : path;
}

function buildDetailItem(row: DetailRow, idx: number, token: number): HTMLElement {
  const item = doc("div");
  const armed = !!row.item?.danger && detailArmed === row.item.id;
  const canElevate = detailElevatable(row);
  item.className = `result-item${idx === detailSel ? " selected" : ""}${armed ? " danger-armed" : ""}`;
  item.dataset.idx = String(idx);

  let iconHtml = "";   // SVG 图标（插件）直接内联；文本占位走 esc()
  let iconText = "";
  let title = "";
  let desc = "";
  let badge = "";

  switch (row.cat) {
    case "apps": {
      const app = row.app!;
      const ext = (app.path.match(/\.([a-zA-Z0-9]+)$/)?.[1] || "").toLowerCase();
      const isFolder = !ext || ext === "lnk";
      iconText = isFolder ? "📁" : ext.length <= 3 ? ext : ext.slice(0, 3);
      title = app.name;
      desc = t("chat.launch_app");
      badge = isFolder ? (ext === "lnk" ? t("chat.badge_shortcut") : t("chat.badge_folder")) : ext;
      break;
    }
    case "files": {
      const f = row.file!;
      const isFolder = f.kind === "folder";
      iconText = isFolder ? "📁" : f.ext.length <= 4 ? f.ext : f.ext.slice(0, 4);
      title = f.name;
      desc = parentDir(f.path);
      badge = isFolder ? t("chat.badge_folder") : f.ext;
      break;
    }
    case "settings": {
      const it = row.item!;
      iconText = it.icon;
      title = detailCatalogTitle(it);
      desc = it.target;
      badge = t("detail.badge_setting");
      break;
    }
    case "actions": {
      const it = row.item!;
      iconText = it.icon;
      title = armed ? t("detail.confirm_danger", { title: detailCatalogTitle(it) }) : detailCatalogTitle(it);
      desc = it.keywords.slice(0, 3).join(" · ");
      badge = t("detail.badge_action");
      break;
    }
    case "commands": {
      const p = row.plugin!;
      iconHtml = pluginIconSvg(p.id);
      title = pluginName(p.id, p.name);
      desc = pluginDesc(p.id, p.description);
      badge = p.badge || t("detail.badge_command");
      break;
    }
    case "web": {
      iconHtml = pluginIconSvg("web-search");
      title = t("plugin.web-search");
      desc = `"${(row.query || "").slice(0, 40)}"`;
      badge = t("chat.badge_web");
      break;
    }
  }

  item.innerHTML = `
    <div class="result-item-icon">${iconHtml || esc(iconText)}</div>
    <div class="result-item-content">
      <div class="result-item-title${armed ? " detail-danger" : ""}">${esc(title)}</div>
      ${desc ? `<div class="result-item-desc">${esc(desc)}</div>` : ""}
    </div>
    ${canElevate ? `<button type="button" class="result-item-elevate" title="${esc(t("detail.elevate_title"))}"><svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2.5 4.5 5.6V11c0 4.7 3.2 8.8 7.5 10.5 4.3-1.7 7.5-5.8 7.5-10.5V5.6Z"/></svg></button>` : ""}
    <span class="result-item-badge">${esc(badge)}</span>
  `;
  item.addEventListener("click", () => {
    detailSel = idx;
    void activateDetailRow(row);
  });
  // 盾牌 = 「以管理员身份运行」（会弹 UAC）。**必须 stopPropagation**，
  // 否则这一次点击会先命中行上的普通「打开」监听器（用户看到的是「点了盾牌却普通启动了」）。
  if (canElevate) {
    item.querySelector<HTMLElement>(".result-item-elevate")?.addEventListener("click", (ev) => {
      ev.stopPropagation();
      ev.preventDefault();
      detailSel = idx;
      markDetailSelection();
      void activateDetailRow(row, true);
    });
  }
  item.addEventListener("mousemove", () => {
    if (detailSel !== idx) {
      detailSel = idx;
      markDetailSelection();
    }
  });
  // 应用 / 文件用系统图标替换占位文本（异步；重渲染后靠 token 作废旧回包）
  if (row.cat === "apps") applyDetailPathIcon(item, row.app!.path, token);
  else if (row.cat === "files") applyDetailPathIcon(item, row.file!.path, token);
  return item;
}

function applyDetailPathIcon(item: HTMLElement, path: string, token: number) {
  invoke<string | null>("get_app_icon", { path })
    .then(dataUrl => {
      if (!dataUrl || token !== detailIconToken) return;
      const iconEl = item.querySelector(".result-item-icon") as HTMLElement | null;
      if (iconEl) iconEl.innerHTML = `<img src="${dataUrl}" class="result-item-icon-img" alt="">`;
    })
    .catch(() => {});
}

function markDetailSelection() {
  detailResults.querySelectorAll<HTMLElement>(".result-item").forEach(it => {
    it.classList.toggle("selected", Number(it.dataset.idx) === detailSel);
  });
  scrollDetailSelectionIntoView();
  renderDetailPreview();
}

function scrollDetailSelectionIntoView() {
  detailResults
    .querySelector<HTMLElement>(`.result-item[data-idx="${detailSel}"]`)
    ?.scrollIntoView({ block: "nearest" });
}

// ── 右侧预览区（2026-09-19 批 5 任务 3a）──────────────────────────
//
// 对齐 Win11 新版搜索（KB5120998）：选中一条结果 → 右侧给出缩略图、完整路径、
// 最后修改日期，以及「打开」「复制路径」两个动作。
//
// 两条防抖纪律（这块会在鼠标划过每一行时被重建，不节流会把缩略图请求打爆）：
//   ① `detailPreviewKey` —— 同一行重复进入直接早退，不重建 DOM、不重发请求；
//   ② `detailThumbCache` —— 缩略图按路径缓存（含「失败」这个结果，用空串表示），
//      上下键来回扫同一批文件时只在第一次真的走 IPC。

/** 缩略图缓存：path → data URL（`""` = 取过但拿不到，别再问第二次） */
const detailThumbCache = new Map<string, string>();
/** 上一次已渲染的预览键（path 或 cat:title），相同则早退 */
let detailPreviewKey = "";
/** 异步缩略图回包的失效令牌（重渲染后旧回包作废，与 detailIconToken 同理） */
let detailPreviewToken = 0;

/** 把毫秒时间戳格式化成「本地时间」，读不到（0）返回空串。 */
function formatModified(ms: number): string {
  if (!ms) return "";
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return "";
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function renderDetailPreview() {
  const row = detailRows[detailSel];
  if (!row) {
    detailPreviewKey = "";
    detailPreview.classList.add("hidden");
    detailPreview.replaceChildren();
    return;
  }

  let title = "";
  let glyph = "";
  let path = "";
  let modified = 0;
  let source = "";
  let iconHtml = "";
  switch (row.cat) {
    case "apps": {
      const app = row.app!;
      title = app.name;
      path = app.path;
      source = t("detail.src_apps");
      break;
    }
    case "files": {
      const f = row.file!;
      title = f.name;
      path = f.path;
      modified = f.modified;
      source = t("detail.src_files");
      glyph = f.kind === "folder" ? "📁" : (f.ext || "📄");
      break;
    }
    case "settings": {
      const it = row.item!;
      title = detailCatalogTitle(it);
      path = it.target;
      source = t("detail.src_settings");
      glyph = it.icon;
      break;
    }
    case "actions": {
      const it = row.item!;
      title = detailCatalogTitle(it);
      path = it.target;
      source = t("detail.src_actions");
      glyph = it.icon;
      break;
    }
    case "commands": {
      const p = row.plugin!;
      title = p.name;
      source = t("detail.src_commands");
      iconHtml = pluginIconSvg(p.id);
      break;
    }
    case "web": {
      title = t("plugin.web-search");
      source = t("detail.src_web");
      glyph = "🌐";
      break;
    }
  }

  const key = path || `${row.cat}:${title}`;
  if (key === detailPreviewKey) return;
  detailPreviewKey = key;
  const token = ++detailPreviewToken;
  detailPreview.classList.remove("hidden");

  // 缩略图容器：apps/files 走真实缩略图（图片真解码、其余系统类型图标），
  // 其余分类用图标 / 字形占位。
  const thumb = doc("div");
  thumb.className = "detail-preview-thumb";
  if (iconHtml) {
    thumb.innerHTML = iconHtml;
  } else if (glyph) {
    const g = doc("span");
    g.className = "detail-preview-glyph";
    g.textContent = glyph;
    thumb.appendChild(g);
  }

  const titleEl = doc("div");
  titleEl.className = "detail-preview-title";
  titleEl.textContent = title;

  const rowsEl = doc("div");
  rowsEl.className = "detail-preview-rows";
  const addRow = (labelKey: string, value: string) => {
    if (!value) return;   // 没有这一项就不画（例如插件命令没有路径）
    const rowEl = doc("div");
    rowEl.className = "detail-preview-row";
    const k = doc("div");
    k.className = "detail-preview-key";
    k.textContent = t(labelKey);
    const v = doc("div");
    v.className = "detail-preview-val";
    v.textContent = value;
    rowEl.append(k, v);
    rowsEl.appendChild(rowEl);
  };
  addRow("detail.preview_source", source);
  addRow("detail.preview_path", path);
  addRow("detail.preview_modified", formatModified(modified));

  // 动作区：打开（与点击该行完全同一条路径，含危险动作二次确认/提权）
  const actions = doc("div");
  actions.className = "detail-preview-actions";
  const openBtn = doc("button") as HTMLButtonElement;
  openBtn.type = "button";
  openBtn.textContent = t("detail.preview_open");
  openBtn.addEventListener("click", () => void activateDetailRow(row));
  actions.appendChild(openBtn);
  if (path) {
    const copyBtn = doc("button") as HTMLButtonElement;
    copyBtn.type = "button";
    copyBtn.textContent = t("detail.preview_copy_path");
    copyBtn.addEventListener("click", async () => {
      try {
        const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
        await writeText(path);
        copyBtn.textContent = t("detail.preview_copied");
        setTimeout(() => { copyBtn.textContent = t("detail.preview_copy_path"); }, 1200);
      } catch (e) {
        console.warn("[lunac] copy path failed:", e);
      }
    });
    actions.appendChild(copyBtn);
  }

  detailPreview.replaceChildren(thumb, titleEl, rowsEl, actions);

  // 缩略图异步回填（缓存 + 令牌双重保护）
  if (row.cat === "apps" || row.cat === "files") {
    const cached = detailThumbCache.get(path);
    if (cached !== undefined) {
      if (cached) thumb.innerHTML = `<img src="${cached}" alt="">`;
    } else {
      invoke<string | null>("get_file_thumbnail", { path, max: 256 })
        .then(dataUrl => {
          detailThumbCache.set(path, dataUrl || "");
          if (token !== detailPreviewToken) return;   // 已切到别的行
          if (dataUrl) thumb.innerHTML = `<img src="${dataUrl}" alt="">`;
        })
        .catch(() => { detailThumbCache.set(path, ""); });
    }
  }
}

// ── 执行 ─────────────────────────────────────────────────────────

/** 执行一个结果行。`elevated = true` 走「以管理员身份运行」（`ShellExecuteW(runas)`，弹 UAC）。
 *
 *  提权路径与普通路径有两点刻意不同：
 *    ① **先等结果再收界面** —— UAC 被拒（`SE_ERR_ACCESSDENIED`）时必须把原因显示出来，
 *       而详情面板一收，提示就没有落点了；
 *    ② **不记使用频次、不走 launchApp** —— `launchApp` 自己会隐藏窗口并写频次，
 *       那是「普通启动」的语义，混进来会让频次统计失真。
 *  判据（哪一行有提权形态）由 `detailElevatable()` 给出，与 Rust 侧同源。 */
async function activateDetailRow(row: DetailRow, elevated = false) {
  switch (row.cat) {
    case "apps":
    case "files": {
      const path = row.app ? row.app.path : row.file!.path;
      if (elevated) {
        try {
          await invoke("launch_app_elevated", { path });
        } catch (err) {
          flashDetailHint(t("detail.elevate_failed", { err: String(err) }));
          return;
        }
        exitDetailSilently();
        return;
      }
      // launchApp 自带「隐藏窗口 + 记录使用频次」；先还原视图，下次唤出回到简洁搜索
      exitDetailSilently();
      launchApp(path);
      return;
    }
    case "settings":
      exitDetailSilently();
      invoke("open_setting", { target: row.item!.target }).catch(e => console.warn("[lunac] open_setting:", e));
      return;
    case "actions": {
      const it = row.item!;
      // 提权分支：只对 `elevatable` 的动作开放（Rust 侧还会再校验一次）
      if (elevated && it.elevatable) {
        try {
          await invoke("run_system_action_elevated", { id: it.id });
        } catch (err) {
          flashDetailHint(t("detail.elevate_failed", { err: String(err) }));
          return;
        }
        exitDetailSilently();
        return;
      }
      // 危险动作（关机 / 重启）二次确认：3 秒内再按一次才真执行
      if (it.danger && detailArmed !== it.id) {
        clearDetailArmed();
        detailArmed = it.id;
        detailArmedTimer = setTimeout(() => {
          detailArmed = "";
          detailArmedTimer = null;
          if (detailOpen) renderDetail();
        }, 3000);
        renderDetail();
        return;
      }
      clearDetailArmed();
      exitDetailSilently();
      invoke("run_system_action", { id: it.id }).catch(e => console.warn("[lunac] run_system_action:", e));
      return;
    }
    case "commands": {
      const plugin = row.plugin!;
      if (_execGuard) return;
      exitDetailSilently();
      await executePlugin(plugin);
      return;
    }
    case "web": {
      const wsPlugin = pluginRegistry.getAll().find(p => p.id === "web-search");
      exitDetailSilently();
      if (wsPlugin) void wsPlugin.execute(row.query || "");
      return;
    }
  }
}

function clearDetailArmed() {
  detailArmed = "";
  if (detailArmedTimer) {
    clearTimeout(detailArmedTimer);
    detailArmedTimer = null;
  }
}

// ── 索引状态 ─────────────────────────────────────────────────────

function renderDetailStatus() {
  if (detailStatus.scanning || !detailStatus.count) {
    detailIndexEl.textContent = t("detail.indexing");
    return;
  }
  detailIndexEl.textContent = detailStatus.truncated
    ? t("detail.index_truncated", { n: String(detailStatus.count) })
    : t("detail.indexed", { n: String(detailStatus.count) });
}

async function refreshDetailStatus() {
  let next: DetailIndexStatus;
  try {
    next = await invoke<DetailIndexStatus>("file_index_status");
  } catch {
    return;
  }
  if (!detailOpen) return;
  const wasScanning = detailStatus.scanning || !detailStatus.count;
  detailStatus = next;
  renderDetailStatus();
  // 索引刚从「扫描中/空」变成就绪 → 补一次查询（首进大界面时索引常常还没载入）。
  // 注意不要用 `!detailSeq` 之类当守卫：enterDetail() 里那次 runDetailSearch()
  // 已经把序号推进到 1，加了这个条件这条分支就变成永不触发的死代码 ——
  // 表现为「首次进大界面时文件结果区一直是空的」。过渡只发生一次，
  // 且 rerun 后 detailStatus 已更新为 scanning=false，不会自激。
  if (wasScanning && !next.scanning && next.count > 0) void runDetailSearch();
}

function startDetailStatusPolling() {
  if (detailStatusTimer) return;
  detailStatusTimer = setInterval(() => void refreshDetailStatus(), 2000);
}

function stopDetailStatusPolling() {
  if (detailStatusTimer) {
    clearInterval(detailStatusTimer);
    detailStatusTimer = null;
  }
}

// ── 事件绑定 ─────────────────────────────────────────────────────

// 双击搜索栏 → 详细搜索大界面。插件锁定态的双击另走 setDetached（见上方既有监听），
// 这里在 pluginActive / detached / 已经在详情态时直接返回，两条路径互不干扰。
searchBar.addEventListener("dblclick", (e) => {
  const target = e.target as HTMLElement;
  if (target.closest("button")) return;         // 设置按钮等
  if (pluginActive || detached || detailOpen) return;
  e.preventDefault();
  enterDetail();
});

detailInput.addEventListener("input", () => void runDetailSearch());

// 输入框也能拖窗（与简洁搜索框同一套手感）—— 顶栏大部分面积被输入框占据，
// 不给它拖动通道的话，大界面就只剩 12px 内边距能拖了（这正是「拖不动」的成因）。
makeInputDragHandle(detailInput);

// 详细搜索输入框与简洁搜索栏共用同一套右键菜单（全选/复制/剪切/粘贴）
attachSearchContextMenu(detailInput);

detailInput.addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    if (!detailRows.length) return;
    const delta = e.key === "ArrowDown" ? 1 : detailRows.length - 1;
    detailSel = (detailSel + delta) % detailRows.length;
    markDetailSelection();
    return;
  }
  if (e.key === "Tab") {
    e.preventDefault();
    const i = DETAIL_CATS.indexOf(detailCat);
    const delta = e.shiftKey ? DETAIL_CATS.length - 1 : 1;
    setDetailCat(DETAIL_CATS[(i + delta) % DETAIL_CATS.length]);
    return;
  }
  if (e.key === "Enter") {
    e.preventDefault();
    const row = detailRows[detailSel];
    if (row) {
      // Shift+Enter = 以管理员身份运行（会弹 UAC）。没有提权形态的行直接给反馈，
      // 而不是静默按普通方式打开 —— 用户按了带修饰键的 Enter 就是想提权。
      if (e.shiftKey && !detailElevatable(row)) {
        flashDetailHint(t("detail.elevate_unsupported"));
        return;
      }
      void activateDetailRow(row, e.shiftKey);
    } else if (detailInput.value.trim()) {
      // 当前分类下没有结果 → 回车直接走网页搜索（与 Win+S 的兜底一致）
      void activateDetailRow({ cat: "web", query: detailInput.value.trim() });
    }
  }
});

detailBackBtn.addEventListener("click", () => exitDetail());

detailRefreshBtn.addEventListener("click", () => {
  invoke("refresh_file_index").catch(() => {});
  detailStatus = { ...detailStatus, scanning: true };
  renderDetailStatus();
  startDetailStatusPolling();
});

