// ── 插件悬浮窗入口 ────────────────────────────────────────────────
// 2026-09-27。`plugin.html` 的脚本；由宿主 `plugin_window::open_plugin_window`
// 建窗时加载。
//
// **这个入口必须保持「惰性」**：宿主有一批**全局单值**状态 —— `hotkey::UI_MODE`、
// `DETACHED`、`QUERY_EMPTY`、`MAIN_HWND`，以及 `main.rs` 里 `Destroyed` 的全局清理。
// 它们全都只描述**主窗口**。所以这里：
//   · 绝不调 `set_ui_mode` / `set_detached` / `set_query_state` / `hide_lunac`；
//   · 绝不注册剪贴板、热键、对话流、搜索那几条链路（那些是主窗口的职责）。
// 违反任何一条，第二个窗口就会和主窗口抢同一份全局状态。
//
// **外观不自己算**：主题色 / 玻璃参数 / 圆角 / 文字色那一整套派生逻辑在
// main.ts 里（`applyAppearance` + 派生调色），复刻一份必然漂移。改成
// 「主窗口把**它算好的**内联 CSS 变量广播过来」（`lunac-theme-vars`），
// 本窗口原样套用 —— 单一真相源，且主窗口改主题时两边同步。

import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { initI18n, loadSavedLanguage, pluginName, t } from "./i18n.js";
import { pluginRegistry } from "./plugins/registry";
import { registerBuiltinPlugins } from "./plugins/builtin/index";
import { attachPluginListeners, detachPluginListeners } from "./plugins/attach";

/** 主题变量事件：主窗口 → 所有窗口（见 main.ts 的 broadcastThemeVars）。 */
const THEME_VARS_EVENT = "lunac-theme-vars";
const THEME_REQ_EVENT = "lunac-theme-request";

interface PluginWindowInit {
  plugin_id: string;
  input: string;
}

const root = document.getElementById("results-list") as HTMLElement;
const titleEl = document.getElementById("plugin-title") as HTMLElement;

let currentPluginId = "";
let renderGen = 0;

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

// ── 外观同步 ──────────────────────────────────────────────────────

/** 上一次套用的变量名。**不能靠遍历 `style` 反查** —— 那要赌
 *  CSSStyleDeclaration 的迭代包含自定义属性；自己记账才是确定的。
 *  换主题时先按这张表清空，再套新的一份（否则被删掉的变量会留旧值）。 */
let appliedThemeKeys: string[] = [];

function applyThemeVars(vars: Record<string, string>) {
  const s = document.documentElement.style;
  for (const k of appliedThemeKeys) s.removeProperty(k);
  appliedThemeKeys = [];
  for (const [k, v] of Object.entries(vars)) {
    if (!k.startsWith("--")) continue;
    // 空串 = 主窗口那边已经清掉了这个变量（切回默认主题），这里也必须删掉，
    // 否则会留着上一个主题的值。CSSOM 里 setProperty(k, "") 等价于 removeProperty，
    // 但那是个容易忘的约定，这里写明。
    if (v === "") s.removeProperty(k);
    else s.setProperty(k, v);
    appliedThemeKeys.push(k);
  }
}

// ── 渲染 ──────────────────────────────────────────────────────────

async function render(pluginId: string, input: string) {
  const myGen = ++renderGen;
  // 换插件前先把上一个插件的定时器/监听收掉（同一个窗口可以被复用去装另一个插件）
  if (currentPluginId && currentPluginId !== pluginId) detachPluginListeners(currentPluginId);
  currentPluginId = pluginId;

  const plugin = pluginRegistry.getAll().find(p => p.id === pluginId);
  titleEl.textContent = plugin ? pluginName(plugin.id) : pluginId;

  if (!plugin) {
    // 坏状态要可见：宿主传了一个本窗口认不出的插件 id（版本错位 / 外部包被删）
    root.innerHTML = `<div class="plugin-result"><div class="plugin-result-content">${esc(t("plugin.float_unknown", { id: pluginId }))}</div></div>`;
    return;
  }

  try {
    const result = await plugin.execute(input);
    if (myGen !== renderGen) return; // 期间又换了一次插件，本次结果作废
    root.innerHTML =
      result.type === "html"
        ? `<div class="plugin-result">${result.content}</div>`
        : `<div class="plugin-result"><div class="plugin-result-content">${esc(result.content)}</div></div>`;
    await attachPluginListeners(plugin, root);
  } catch (e) {
    if (myGen !== renderGen) return;
    const msg = e instanceof Error ? e.message : String(e);
    root.innerHTML = `<div class="plugin-result"><div class="plugin-result-content">${esc(msg)}</div></div>`;
    console.error("[lunac plugin-window] execute failed:", e);
  }
}

// ── 标题栏三按钮 ──────────────────────────────────────────────────

function wireTitlebar() {
  const btn = (id: string) => document.getElementById(id) as HTMLButtonElement | null;

  btn("plugin-pin")?.addEventListener("click", async () => {
    const el = btn("plugin-pin")!;
    const pinned = el.dataset.pinned !== "1";
    try {
      await invoke("plugin_window_set_pin", { pinned });
      el.dataset.pinned = pinned ? "1" : "0";
      el.classList.toggle("active", pinned);
    } catch (e) {
      console.error("[lunac plugin-window] set pin failed:", e);
    }
  });

  btn("plugin-min")?.addEventListener("click", () => {
    void invoke("plugin_window_minimize").catch(() => {});
  });

  btn("plugin-close")?.addEventListener("click", () => {
    detachPluginListeners(currentPluginId);
    void invoke("plugin_window_close").catch(() => {});
  });

  // 置顶初始态**必须问宿主**：建窗时就是 always_on_top(true)，
  // 前端自己猜会出现「按钮显示未置顶、实际已置顶」的错位。
  void invoke<boolean>("plugin_window_pin_state")
    .then(pinned => {
      const el = btn("plugin-pin");
      if (!el) return;
      el.dataset.pinned = pinned ? "1" : "0";
      el.classList.toggle("active", pinned);
    })
    .catch(() => {});
}

// ── 启动 ──────────────────────────────────────────────────────────

(async () => {
  await initI18n();
  loadSavedLanguage();
  registerBuiltinPlugins();
  wireTitlebar();

  // 外观：先要一次，之后主窗口每次改主题都会广播（main.ts 的 applyAppearance 收尾）
  void listen<Record<string, string>>(THEME_VARS_EVENT, ev => {
    if (ev.payload && typeof ev.payload === "object") applyThemeVars(ev.payload);
  });
  void emit(THEME_REQ_EVENT);

  // 已开着同一个插件时再次「打开」⇒ 宿主不新建窗口，改推一条新入参过来
  void listen<string>("plugin-window-input", ev => {
    if (typeof ev.payload === "string" && currentPluginId) void render(currentPluginId, ev.payload);
  });

  // 宿主在**建窗之前**就把载荷写好了（前端可能比命令返回更快），所以这里直接取
  try {
    const init = await invoke<PluginWindowInit>("plugin_window_init");
    await render(init.plugin_id, init.input);
  } catch (e) {
    console.error("[lunac plugin-window] init failed:", e);
    root.innerHTML = `<div class="plugin-result"><div class="plugin-result-content">${esc(t("plugin.float_init_failed"))}</div></div>`;
  }
})();
