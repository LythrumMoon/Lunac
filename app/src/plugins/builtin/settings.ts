// Keep track of the last active settings category across open/close cycles
let activeSettingsCategory = "general";
// 四组「自定义」拉条的展开状态：跨「关掉设置再打开」保留（否则每次进来都要再点一次
// 「自定义」才看得到滑块，用户会以为设置在跳）。
let bgSlidersOpen = false;
/** 底色 / 按钮 / 文字（2026-09-20 新增的后两组，形态与背景那组一致）。 */
let basePanelOpen = false;
let btnPanelOpen = false;
let textPanelOpen = false;

// Unlisten functions for hotkey recording events — cleaned up on re-attach to avoid memory leaks
let _clickOutsideHandler: ((e: Event) => void) | null = null;
let _onRecordingCaptured: ((e: Event) => void) | null = null;
let _onRecordingCancelled: (() => void) | null = null;
let _unlistenHotkeyRecorded: (() => void) | null = null; // Tauri event (for Alt+Space via WndProc)

import type { Plugin } from "../registry";
import { pluginRegistry } from "../registry";
import { refreshMarketPlugins, type MarketPluginInfo } from "../market";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-shell";
import { t, setLanguage, resetToSystemLanguage, pluginName, pluginDesc } from "../../i18n.js";
import { installOcrEngine } from "./ocr.js";


function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function formatKey(vk: number): string {
  const map: Record<number, string> = {
    0x0d: "Enter", 0x1b: "Esc", 0x20: "Space", 0x21: "PgUp", 0x22: "PgDn",
    0x23: "End", 0x24: "Home", 0x25: "←", 0x26: "↑", 0x27: "→", 0x28: "↓",
    0x2d: "Ins", 0x2e: "Del",
    0x70: "F1", 0x71: "F2", 0x72: "F3", 0x73: "F4", 0x74: "F5",
    0x75: "F6", 0x76: "F7", 0x77: "F8", 0x78: "F9",
    0x79: "F10", 0x7a: "F11", 0x7b: "F12",
    0xa0: "LShift", 0xa1: "RShift", 0xa2: "LCtrl", 0xa3: "RCtrl",
    0xa4: "LAlt", 0xa5: "RAlt",
    0x5b: "LWin", 0x5c: "RWin",
    0xbc: ",", 0xbe: ".", 0xbf: "/", 0xba: ";", 0xde: "'",
    0xdb: "[", 0xdd: "]", 0xdc: "\\", 0xc0: "`",
    0xbd: "-", 0xbb: "=",
  };
  if (map[vk]) return map[vk];
  if (vk >= 0x30 && vk <= 0x39) return String.fromCharCode(vk);
  if (vk >= 0x41 && vk <= 0x5a) return String.fromCharCode(vk);
  return String.fromCharCode(vk);
}

// ── Content pane builders (one per category) ────────────────────

/** Language options shown in the General pane. Names are native so they're
 *  recognizable in any UI language; only the "Follow System" label is
 *  translated via t(). */
const LANG_OPTIONS: { value: string; labelKey?: string; label?: string }[] = [
  { value: "system", labelKey: "settings.lang_follow_system" },
  { value: "zh-CN", label: "简体中文" },
  { value: "zh-TW", label: "繁體中文" },
  { value: "ja", label: "日本語" },
  { value: "ko", label: "한국어" },
  { value: "en", label: "English" },
];

function currentLangSelection(): string {
  try {
    return localStorage.getItem("lunac_lang") || "system";
  } catch { return "system"; }
}

function buildGeneralPane(hotkey: string, autoStart: boolean): string {
  const langOptions = LANG_OPTIONS.map(o => ({
    value: o.value,
    label: o.label ?? t(o.labelKey!),
    selected: o.value === currentLangSelection(),
  }));
  const langSelectHtml = renderCustomSelectInline("settings-language", langOptions);

  return `
    <div class="settings-pane" data-pane="general" id="sp-general">
      <div class="settings-pane-title">${t("settings.general")}</div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.hotkey")}</span>
        <button id="settings-hotkey-btn" class="settings-hotkey" title="${t("settings.hotkey_record")}">
          ${esc(hotkey)}
        </button>
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.auto_start")}</span>
        <label class="settings-toggle">
          <input type="checkbox" id="settings-autostart" ${autoStart ? "checked" : ""}>
          <span class="settings-toggle-slider"></span>
        </label>
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.language")}</span>
        ${langSelectHtml}
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.ocr_engine")}</span>
        <div class="settings-bg-actions">
          <span id="settings-ocr-status" style="font-size:0.72rem;color:var(--text-dim);margin-right:8px;"></span>
          <button id="settings-ocr-install" class="settings-btn">${t("ocr.engine_download")}</button>
        </div>
      </div>
      <!-- 自定义背景已于 2026-09-19 移到「风格」分区（含三项滑块与主题色）。 -->
    </div>`;
}

/** Local variant of renderCustomSelect (general pane) — plain labels, no icons. */
function renderCustomSelectInline(id: string, options: { value: string; label: string; selected: boolean }[]): string {
  const optsHtml = options.map(o =>
    `<div class="custom-select-option${o.selected ? ' selected' : ''}" data-value="${esc(o.value)}">${esc(o.label)}</div>`
  ).join("");
  const selectedLabel = options.find(o => o.selected)?.label || options[0]?.label || "";
  return `
    <div class="custom-select" id="${id}">
      <button class="custom-select-trigger" type="button">
        <span class="custom-select-label">${esc(selectedLabel)}</span>
        <svg class="custom-select-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="6 9 12 15 18 9"/></svg>
      </button>
      <div class="custom-select-dropdown">${optsHtml}</div>
    </div>`;
}

// ── 外观 / 主题分区（「风格」，2026-09-19 新增）──────────────────
//
// **配置与落地全在 main.ts**（`window.__lunac_appearance` 这座桥），本分区只做
// 界面与事件绑定：不自己写 localStorage、不自己拼 CSS 变量。理由：改造前的背景图
// 就是「设置写 localStorage + main.ts 读」两份实现，一旦再分出去（滑块/主题色），
// 两处必然漂移（改了一处忘了另一处 = 用户看到「拖了没反应」）。

/** 外观配置（与 main.ts 的 `Appearance` 同形；这里只声明界面用得到的部分）。 */
interface AppearanceConfig {
  bgImage: string | null;
  bgBlur: number;
  bgSaturate: number;
  bgOpacity: number;
  sheen: number;
  /** 界面上的「底色透明度」（原「界面玻璃透明度」）。2026-09-20 从背景区迁进「底色自定义」。 */
  surfaceAlpha: number;
  /** **「恢复默认主题」**。**默认 false**（开 = 底色 / 按钮线条 / 按钮背景的颜色
   *  全部不生效，回到默认主题配色；三个透明度与文字明度仍可调且生效）。字段名沿用 `tintBase`。 */
  tintBase: boolean;
  /** 空串 = 还没动过底色 ⇒ 仍按主题自动派生（取色器显示派生出来的那个色）。 */
  baseColor: string;
  /** 空串 = 跟随主题色（默认）。 */
  btnLineColor: string;
  btnLineAlpha: number;
  /** 按钮背景色。空串 = 跟随主题色（默认）。2026-09-20 新增，不再跟底色。 */
  btnBgColor: string;
  btnBgAlpha: number;
  /** 文字明度偏移 ±100（0 = 派生原值）。 */
  textLight: number;
  themeId: string;
}
interface AppearanceThemeInfo {
  manifest: { id: string; name: string; version: string; author: string };
  builtin: boolean;
}
interface AppearanceBridge {
  get(): AppearanceConfig;
  set(patch: Partial<AppearanceConfig>): void;
  themes(force?: boolean): Promise<AppearanceThemeInfo[]>;
  /** 「跟随态」下三个取色器该显示什么色（底色 = 主题包 surface 或派生的表面色，
   *  按钮线条 / 按钮背景 = 主题色）。只用于取色器的**初始显示**，不落盘 ——
   *  面板不自己复制一份派生逻辑（否则必然漂移）。 */
  resolvedSwatches(): { base: string; btnLine: string; btnBg: string };
  themesDir(): Promise<string>;
  pickBgImage(): Promise<boolean>;
}

/** 取 main.ts 的外观桥。拿不到时（WebView 刚重载、热重载竞态）返回 null，
 *  界面照常画出来但所有控件早退 —— 比抛异常把整个设置面板打空要好。 */
function appearanceBridge(): AppearanceBridge | null {
  return ((window as any).__lunac_appearance as AppearanceBridge | undefined) ?? null;
}

// ── 应用内取色器（2026-09-19）────────────────────────────────────
//
// 为什么不用 `<input type="color">` 直接了事：它打开的是 **Windows 系统取色对话框**，
// 与 Lunac 的暗色玻璃界面完全脱节（用户要求「取色器的界面跟 lunac 界面主题对齐」），
// 而且它在 WebView2 里是原生窗口、样式一个像素都改不了。
// 为什么不再留「色轮 + 饱和度/明度滑块」：那三件和取色器表达的是同一个自由度，
// 并存只会互相打架（用户明确指出「功能发生冲突，只采用取色器」）。
// 现在的形态 = 一个色块按钮 → 展开**一个**面板：色相条 + 饱和度/明度方块 + hex 输入
// + 预设色板。全部用项目自己的 token 画（--border-glass / --text-dim / --accent）。

/** 取色器的 HTML。`id` 是前缀，便于在同一页挂多个实例（底色 / 按钮线条 / 按钮背景）。 */
function colorPickerHtml(id: string, hex: string): string {
  const presets = ["#c0a0a0", "#3a7bd5", "#5b9a68", "#c9a227", "#c05555", "#8e6fc0", "#3f9a9a", "#d07aa0"];
  return `
    <div class="ap-picker" id="${id}">
      <div class="ap-picker-head">
        <!-- 色号输入放在最左：用户要求「具体色号的整个区域向左移动、与左侧 label 对齐」，
             放在色块之后永远差一个色块+间距的宽度。 -->
        <input type="text" class="ap-hex" id="${id}-hex" value="${esc(hex)}" spellcheck="false" maxlength="7">
        <button type="button" class="ap-swatch-btn" id="${id}-swatch"></button>
        <!-- 「从电脑中取色」（2026-09-20 用户要求）：走 Chromium 的 EyeDropper API，
             可以在**屏幕任意位置**吸一个像素。不支持该 API 时由 JS 把它隐藏（WebView2
             基于 Chromium，正常可用；老内核才没有）。图标是内联 SVG，走 currentColor。 -->
        <button type="button" class="ap-picker-toggle ap-eyedropper" id="${id}-pick" title="${esc(t("settings.appearance_eyedropper"))}">
          <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m2 22 1-1h3l9-9"/><path d="M3 21v-3l9-9"/><path d="m15 6 3.4-3.4a2.1 2.1 0 1 1 3 3L18 9l.8.8a2 2 0 0 1 0 2.8l-1.4 1.4a2 2 0 0 1-2.8 0l-6-6a2 2 0 0 1 0-2.8l1.4-1.4a2 2 0 0 1 2.8 0Z"/></svg>
        </button>
        <button type="button" class="ap-picker-toggle" id="${id}-toggle">▾</button>
      </div>
      <div class="ap-pick-panel hidden" id="${id}-panel">
        <div class="ap-sv" id="${id}-sv"><div class="ap-sv-cursor" id="${id}-sv-cursor"></div></div>
        <div class="ap-hue" id="${id}-hue"><div class="ap-hue-cursor" id="${id}-hue-cursor"></div></div>
        <div class="ap-presets" id="${id}-presets">
          ${presets.map(p => `<button type="button" class="ap-preset" data-color="${p}" style="background:${p}"></button>`).join("")}
        </div>
      </div>
    </div>`;
}

// ── HSV ↔ RGB（取色器用 HSV：方块的两轴天然对应「饱和度 / 明度」）──
function rgbToHsv(r: number, g: number, b: number): { h: number; s: number; v: number } {
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
  return { h: Math.round(h), s: mx === 0 ? 0 : d / mx, v: mx };
}
function hsvToRgb(h: number, s: number, v: number): { r: number; g: number; b: number } {
  const c = v * s;
  const k = (n: number) => (n + h / 60) % 6;
  const f = (n: number) => v - c * Math.max(0, Math.min(k(n), 4 - k(n), 1));
  return { r: Math.round(f(5) * 255), g: Math.round(f(3) * 255), b: Math.round(f(1) * 255) };
}
function hsvToHex(h: number, s: number, v: number): string {
  const c = hsvToRgb(h, s, v);
  const to = (n: number) => n.toString(16).padStart(2, "0");
  return `#${to(c.r)}${to(c.g)}${to(c.b)}`;
}
function hexToHsv(hex: string): { h: number; s: number; v: number } | null {
  const m = /^#?([0-9a-fA-F]{6})$/.exec(hex.trim());
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return rgbToHsv((n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff);
}

/** 取色器实例的把手：`apply()` 只重画界面、**不 commit** —— 供外部控件（与色板
 *  同一组值的「饱和度 / 明度」滑块）把新颜色同步过来用。 */
interface ColorPickerHandle {
  apply(hex: string): void;
}

/** 挂载一个取色器实例。`onCommit` 只在用户真的动了控件时调用（初始化不调）——
 *  否则每打开一次设置就把 HSV 往返的舍入误差写一次配置。 */
function mountColorPicker(
  container: HTMLElement, id: string, initialHex: string, onCommit: (hex: string) => void,
): ColorPickerHandle {
  const pick = <T extends HTMLElement>(suffix: string) => container.querySelector(`#${id}-${suffix}`) as T | null;
  const panel = pick("panel");
  const swatch = pick("swatch");
  const hexInput = pick<HTMLInputElement>("hex");
  const sv = pick("sv");
  const svCursor = pick("sv-cursor");
  const hue = pick("hue");
  const hueCursor = pick("hue-cursor");
  let hsv = hexToHsv(initialHex) ?? { h: 0, s: 0, v: 0.75 };
  let open = false;

  const paint = (commit: boolean, exactHex?: string) => {
    // exactHex：hex 输入框 / 预设色板给的**原始值必须原样落盘**，不能走
    // hex→HSV→hex 往返（会丢 1/255，用户会觉得「我选的颜色被改了」）。
    if (exactHex) {
      const parsed = hexToHsv(exactHex);
      if (parsed) hsv = parsed;
    }
    const hex = exactHex ?? hsvToHex(hsv.h, hsv.s, hsv.v);
    if (hexInput) hexInput.value = hex;
    if (swatch) swatch.style.background = hex;
    if (sv) sv.style.setProperty("--ap-hue-color", `hsl(${hsv.h} 100% 50%)`);
    if (svCursor) { svCursor.style.left = `${hsv.s * 100}%`; svCursor.style.top = `${(1 - hsv.v) * 100}%`; }
    if (hueCursor) hueCursor.style.left = `${(hsv.h / 360) * 100}%`;
    if (commit) onCommit(hex);
  };
  paint(false);

  const setOpen = (v: boolean) => {
    open = v;
    panel?.classList.toggle("hidden", !v);
    const tgl = pick("toggle");
    if (tgl) tgl.textContent = v ? "▴" : "▾";
  };
  pick("toggle")?.addEventListener("click", () => setOpen(!open));
  swatch?.addEventListener("click", () => setOpen(!open));

  // 「从电脑中取色」：Chromium 自带的 EyeDropper，能在屏幕任意位置吸一个像素并回传
  // `sRGBHex`。**不支持就整条隐藏**（而不是留一个点了没反应的按钮）。用户按 Esc 取消时
  // `open()` 会 reject，那是正常操作、不是错误，静默即可。
  const eyeBtn = pick("pick");
  const EyeDropperCtor = (window as unknown as { EyeDropper?: new () => { open(): Promise<{ sRGBHex?: string }> } }).EyeDropper;
  if (eyeBtn) {
    if (typeof EyeDropperCtor !== "function") eyeBtn.classList.add("hidden");
    else eyeBtn.addEventListener("click", async () => {
      try {
        const res = await new EyeDropperCtor().open();
        const got = typeof res?.sRGBHex === "string" ? res.sRGBHex.trim().toLowerCase() : "";
        if (/^#[0-9a-f]{6}$/.test(got)) paint(true, got);
      } catch { /* 用户取消取色 */ }
    });
  }

  // 饱和度/明度方块：x = 饱和度，y = 明度（上亮下暗）
  const dragSv = (e: PointerEvent) => {
    if (!sv) return;
    const r = sv.getBoundingClientRect();
    const x = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    const y = Math.min(1, Math.max(0, (e.clientY - r.top) / r.height));
    hsv = { ...hsv, s: x, v: 1 - y };
    paint(true);
  };
  if (sv) {
    let dragging = false;
    sv.addEventListener("pointerdown", (e) => {
      dragging = true;
      try { sv.setPointerCapture(e.pointerId); } catch { /* 捕获失败也能拖 */ }
      dragSv(e);
    });
    sv.addEventListener("pointermove", (e) => { if (dragging) dragSv(e); });
    const stop = () => { dragging = false; };
    sv.addEventListener("pointerup", stop);
    sv.addEventListener("pointercancel", stop);
  }
  // 色相条：0° 在最左（红），与 linear-gradient 的排布一致
  const dragHue = (e: PointerEvent) => {
    if (!hue) return;
    const r = hue.getBoundingClientRect();
    const x = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    hsv = { ...hsv, h: Math.round(x * 360) % 360 };
    paint(true);
  };
  if (hue) {
    let dragging = false;
    hue.addEventListener("pointerdown", (e) => {
      dragging = true;
      try { hue.setPointerCapture(e.pointerId); } catch { /* 同上 */ }
      dragHue(e);
    });
    hue.addEventListener("pointermove", (e) => { if (dragging) dragHue(e); });
    const stop = () => { dragging = false; };
    hue.addEventListener("pointerup", stop);
    hue.addEventListener("pointercancel", stop);
  }

  const commitHex = (raw: string) => {
    const v = raw.trim().startsWith("#") ? raw.trim() : `#${raw.trim()}`;
    if (!/^#[0-9a-fA-F]{6}$/.test(v)) return; // 打字中间态不提交
    paint(true, v.toLowerCase());
  };
  hexInput?.addEventListener("input", () => commitHex(hexInput.value));
  hexInput?.addEventListener("blur", () => paint(false));  // 失焦时把非法输入回滚成合法值
  pick("presets")?.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLElement>("[data-color]");
    if (btn?.dataset.color) paint(true, btn.dataset.color);
  });

  return {
    // 外部控件把颜色同步进来：只重画（hex 会被原样写回输入框与色块），不 commit
    apply: (hex: string) => paint(false, hex),
  };
}

/** 外观滑块行。`data-ap` = 配置字段名 —— 事件绑定走**一个**委托监听，
 *  以后加滑块只写这行 HTML，不必再动事件代码（少一处必漏的地方）。 */
function appearanceSliderRow(
  labelKey: string, field: string, min: number, max: number, step: number, value: number, fmt: (v: number) => string,
): string {
  return `
    <div class="settings-row">
      <span class="settings-label">${t(labelKey)}</span>
      <div class="ap-slider">
        <input type="range" data-ap="${field}" min="${min}" max="${max}" step="${step}" value="${value}">
        <span class="ap-slider-val" data-ap-val="${field}">${esc(fmt(value))}</span>
      </div>
    </div>`;
}

/** 「与色板同一组值」的滑块行（饱和度 / 明度）。
 *
 *  它**不是** `data-ap`：值不直接落配置，而是经由所属取色器换算成 hex 再落
 *  `<slot>Color`。`data-color` 指出属于哪个取色器实例（base / btnline / btnbg），
 *  `data-axis` 指出是色板的哪一轴（sat / val）—— 这两个属性就是「双向联动」的接线点：
 *  拖色板 → `syncAxes()` 回写这里的 value 与读数；拖这里 → `hsvToHex()` 写回色板。 */
function colorAxisRow(labelKey: string, colorSlot: string, axis: "sat" | "val", value: number): string {
  const v = Math.round(value);
  return `
    <div class="settings-row">
      <span class="settings-label">${t(labelKey)}</span>
      <div class="ap-slider">
        <input type="range" data-axis="${axis}" data-color="${colorSlot}" min="0" max="100" step="1" value="${v}">
        <span class="ap-slider-val" data-axis-val="${colorSlot}-${axis}">${v}</span>
      </div>
    </div>`;
}

/** 滑块的数值文案格式（与 APPEARANCE_RANGE 的字段一一对应）。 */
function formatAppearanceValue(field: string, v: number): string {
  if (field === "bgBlur") return `${Math.round(v)}px`;
  if (field === "bgSaturate") return `${Math.round(v)}%`;
  // 文字明度是「偏移格数」：带正负号比 -12.00 好读
  if (field === "textLight") return (v > 0 ? "+" : "") + Math.round(v);
  return v.toFixed(2);
}

async function buildAppearancePane(): Promise<string> {
  const ap = appearanceBridge();
  const cfg: AppearanceConfig = ap?.get() ?? {
    bgImage: null, bgBlur: 4, bgSaturate: 92, bgOpacity: 0.5,
    sheen: 0, surfaceAlpha: 0.88, tintBase: false,
    baseColor: "", btnLineColor: "", btnLineAlpha: 0.32,
    btnBgColor: "", btnBgAlpha: 0.14, textLight: 0,
    themeId: "default",
  };
  let themes: AppearanceThemeInfo[] = [];
  try { themes = (await ap?.themes()) ?? []; } catch { /* 主题包读不到 → 只留默认项 */ }
  // 三个取色器在「跟随态」（对应字段是空串）下显示什么色，由 main.ts 算好给过来 ——
  // 面板不复制那份派生逻辑（复制必然漂移）。见 AppearanceBridge.resolvedSwatches。
  const sw = ap?.resolvedSwatches() ?? { base: "#c0a0a0", btnLine: "#c0a0a0", btnBg: "#c0a0a0" };
  const shown = {
    base: cfg.baseColor || sw.base,
    btnline: cfg.btnLineColor || sw.btnLine,
    btnbg: cfg.btnBgColor || sw.btnBg,
  };
  // 三对「饱和度 / 明度」滑块的初值 = 上面那个色的 HSV 两轴（**与色板同一组值**）。
  const axis = (slot: "base" | "btnline" | "btnbg") => hexToHsv(shown[slot]) ?? { h: 0, s: 0, v: 0 };
  // 背景来源标注：已设置图片 / 未设置
  const bgLabel = cfg.bgImage ? t("settings.appearance_bg_set") : t("settings.appearance_bg_none");
  // 只有内置的「默认」主题允许自定义主题色与背景图片（2026-09-19 批 5 任务 1）：
  // 主题包的意义就是「一整套定好的外观」，放开这两项会让它被改得不像自己。
  // 「自定义」那五个拉条是**窗口玻璃质感**、与配色无关，任何主题下都保留。
  const locked = cfg.themeId !== "default";
  /** 「恢复默认主题」开启时要锁住的块。`data-tint-lock` 是**统一切换锚点**（JS 一句
   *  `querySelectorAll` 覆盖全部，加新项不会漏）；初始 `locked` class 写死在 HTML 里，
   *  免得 attach 之前闪一帧「可编辑」。 */
  const tl = () => `class="ap-locked-group${cfg.tintBase ? " locked" : ""}" data-tint-lock="1"`;

  const themeButtons = themes.map(th => {
    const id = th.manifest.id;
    const active = id === cfg.themeId;
    // 内置默认主题的 name 可能为空（兜底项）→ 用界面语言显示「默认」
    const label = th.manifest.name || t("settings.appearance_theme_default");
    const badge = th.builtin && id !== "default"
      ? `<span class="ap-badge">${t("settings.appearance_theme_builtin")}</span>` : "";
    return `<button type="button" class="ap-theme${active ? " active" : ""}" data-theme="${esc(id)}">${esc(label)}${badge}</button>`;
  }).join("");

  return `
    <div class="settings-pane" data-pane="appearance" id="sp-appearance">
      <div class="settings-pane-title">${t("settings.appearance")}</div>

      <div class="settings-group-title">${t("settings.appearance_bg_section")}</div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.appearance_bg_title")}</span>
        <div class="settings-bg-actions">
          <span class="ap-bg-name" id="ap-bg-name">${esc(bgLabel)}</span>
          <button type="button" class="settings-btn" id="ap-bg-pick"${locked ? " disabled" : ""}>${t("settings.appearance_bg_apply")}</button>
          <button type="button" class="settings-btn ap-bg-custom" id="ap-bg-custom">${t("settings.appearance_bg_custom")}<span class="ap-toggle-caret" id="ap-bg-caret">▾</span></button>
          <button type="button" class="settings-btn" id="ap-bg-clear">${t("settings.appearance_bg_clear")}</button>
        </div>
      </div>
      <!-- 「自定义」展开的就是这一块：四个拉条（毛玻璃化 / 饱和度 / 背景透明度 / 反光）。
           原第五个「界面玻璃透明度」已于 2026-09-20 迁进「主题颜色 → 底色自定义」，
           并改名为「底色透明度」（它就是玻璃底色的 alpha，属于配色而不是背景图）。 -->
      <div id="ap-bg-sliders" class="ap-sliders-panel${bgSlidersOpen ? "" : " hidden"}">
        ${appearanceSliderRow("settings.appearance_bg_blur", "bgBlur", 0, 40, 1, cfg.bgBlur, v => formatAppearanceValue("bgBlur", v))}
        ${appearanceSliderRow("settings.appearance_bg_saturate", "bgSaturate", 0, 200, 1, cfg.bgSaturate, v => formatAppearanceValue("bgSaturate", v))}
        ${appearanceSliderRow("settings.appearance_bg_opacity", "bgOpacity", 0, 1, 0.01, cfg.bgOpacity, v => formatAppearanceValue("bgOpacity", v))}
        ${appearanceSliderRow("settings.appearance_bg_sheen", "sheen", 0, 1, 0.01, cfg.sheen, v => formatAppearanceValue("sheen", v))}
      </div>

      <div class="settings-group-title">${t("settings.appearance_color_section")}</div>
      <!-- 锁定说明 + 锁定组：非「默认」主题时主题包自己带配色，用户不允许再改它
           （2026-09-19 批 5 任务 1）。禁用而不是隐藏 —— 用户要能看见「这个能力存在、
           只是被主题锁了」，隐藏会被当成 bug。 -->
      <div id="ap-lock-note" class="ap-lock-note${locked ? "" : " hidden"}">${t("settings.appearance_theme_locked")}</div>
      <div id="ap-color-group" class="ap-locked-group${locked ? " locked" : ""}">
      <!-- 主题色取色器已删除（2026-09-20，用户要求「去除主题色取色」，字段一并删除）：
           主题色现在**只**由上面选中的主题包提供（它的 tokens.accent），没有用户侧入口。 -->

      <!-- ── ① 恢复默认主题（2026-09-20 二次定稿，用户指定摆在「主题颜色」最顶上）
           语义 = 回到**一开始保存的那套默认主题配色**（themes/default + DEFAULT_ACCENT
           派生的底色、:root 里的按钮配方）：底色 / 按钮线条 / 按钮背景的**颜色**
           全部不生效，走主题自己的值。
           例外（留在锁外、且值仍然生效）= **三个透明度**（底色 / 按钮线条 / 按钮背景）
           + **文字明度**：用户先要求「开恢复默认主题时除透明度以外的选项都不可调」，
           随后又明确「文字明度不锁定」。
           原名「主题色代替底色」，本轮改名并调整失效范围（见 ai-spec 规则 45/46）。
           注意：这段是**模板字符串内部**，注释里写不得反引号（见 code-rules 预检 #15）。 -->
      <div class="settings-row">
        <span class="settings-label" title="${esc(t("settings.appearance_restore_hint"))}">${t("settings.appearance_restore_theme")}</span>
        <label class="settings-toggle">
          <input type="checkbox" id="ap-tint-base" ${cfg.tintBase ? "checked" : ""}>
          <span class="settings-toggle-slider"></span>
        </label>
      </div>
      <div id="ap-restore-note" class="ap-lock-note${cfg.tintBase ? "" : " hidden"}">${t("settings.appearance_restore_note")}</div>

      <!-- ── ② 底色自定义（2026-09-20）──────────────────────────────────
           形态照抄背景那组的「自定义」：一个按钮 → 展开一块面板（展开态跨开关保留）。
           面板里：底色取色器 → 底色透明度 → 底色饱和度 / 底色明度。
           **饱和度 / 明度与色板同一组值（双向联动）**；「恢复默认主题」开启时取色器与
           两个轴滑块禁用，只有中间的透明度滑块留在锁外。 -->
      <div class="settings-row">
        <span class="settings-label">${t("settings.appearance_base_picker")}</span>
        <div class="settings-bg-actions">
          <button type="button" class="settings-btn ap-bg-custom" id="ap-base-custom">${t("settings.appearance_base_custom")}<span class="ap-toggle-caret" id="ap-base-caret">▾</span></button>
        </div>
      </div>
      <!-- id 用 -sliders 而不是 -panel：colorPickerHtml() 内部会生成 #ap-base-panel
           （取色器自己的展开面板），同名会让 pick() 取到错的那个。 -->
      <div id="ap-base-sliders" class="ap-sliders-panel${basePanelOpen ? "" : " hidden"}">
        <div id="ap-base-picker" ${tl()}>
          <div class="settings-row ap-row-block">
            <span class="settings-label">${t("settings.appearance_base_picker")}</span>
            ${colorPickerHtml("ap-base", shown.base)}
          </div>
        </div>
        ${appearanceSliderRow("settings.appearance_base_alpha", "surfaceAlpha", 0.3, 1, 0.01, cfg.surfaceAlpha, v => formatAppearanceValue("surfaceAlpha", v))}
        <div id="ap-base-axes" ${tl()}>
          ${colorAxisRow("settings.appearance_base_saturate", "base", "sat", axis("base").s * 100)}
          ${colorAxisRow("settings.appearance_base_light", "base", "val", axis("base").v * 100)}
        </div>
      </div>

      <!-- ── ③ 按钮自定义（2026-09-20）──────────────────────────────────
           线条 = 项目内所有切换开关 + 新建对话 / 更多设置 / 历史记录 / 发送 / 停止 /
           添加文件 六个按钮的边框；背景 = 同样这六个按钮的底色。
           **按钮背景不再跟底色**（用户要求）—— 它自己一条 --btn-bg-*，与上方底色无关。
           两组各自「取色器 + 透明度 + 饱和度/明度」，其中**两个透明度滑块留在锁外**。 -->
      <div class="settings-row">
        <span class="settings-label">${t("settings.appearance_btn_label")}</span>
        <div class="settings-bg-actions">
          <button type="button" class="settings-btn ap-bg-custom" id="ap-btn-custom">${t("settings.appearance_btn_custom")}<span class="ap-toggle-caret" id="ap-btn-caret">▾</span></button>
        </div>
      </div>
      <div id="ap-btn-sliders" class="ap-sliders-panel${btnPanelOpen ? "" : " hidden"}">
        <div id="ap-btn-line-picker" ${tl()}>
          <div class="settings-row ap-row-block">
            <span class="settings-label">${t("settings.appearance_btnline_picker")}</span>
            ${colorPickerHtml("ap-btnline", shown.btnline)}
          </div>
        </div>
        ${appearanceSliderRow("settings.appearance_btnline_alpha", "btnLineAlpha", 0, 1, 0.01, cfg.btnLineAlpha, v => formatAppearanceValue("btnLineAlpha", v))}
        <div id="ap-btn-line-axes" ${tl()}>
          ${colorAxisRow("settings.appearance_btnline_saturate", "btnline", "sat", axis("btnline").s * 100)}
          ${colorAxisRow("settings.appearance_btnline_light", "btnline", "val", axis("btnline").v * 100)}
        </div>
        <div id="ap-btn-bg-picker" ${tl()}>
          <div class="settings-row ap-row-block">
            <span class="settings-label">${t("settings.appearance_btnbg_picker")}</span>
            ${colorPickerHtml("ap-btnbg", shown.btnbg)}
          </div>
        </div>
        ${appearanceSliderRow("settings.appearance_btnbg_alpha", "btnBgAlpha", 0, 1, 0.01, cfg.btnBgAlpha, v => formatAppearanceValue("btnBgAlpha", v))}
        <div id="ap-btn-bg-axes" ${tl()}>
          ${colorAxisRow("settings.appearance_btnbg_saturate", "btnbg", "sat", axis("btnbg").s * 100)}
          ${colorAxisRow("settings.appearance_btnbg_light", "btnbg", "val", axis("btnbg").v * 100)}
        </div>
      </div>

      <!-- ── ④ 文字自定义（2026-09-20）：**只调三档文字的明度**（主 / 次 / 弱一起挪）。
           它是「偏移量」而不是绝对值 —— 0 = 主题派生原值（逐像素不变）。
           **不在「恢复默认主题」的锁定范围内**（2026-09-20 四次定稿，用户明确要求
           「文字明度不锁定」）：它是明度偏移、不是配色本身，开着开关也照常可调可生效。 -->
      <div class="settings-row">
        <span class="settings-label">${t("settings.appearance_text_label")}</span>
        <div class="settings-bg-actions">
          <button type="button" class="settings-btn ap-bg-custom" id="ap-text-custom">${t("settings.appearance_text_custom")}<span class="ap-toggle-caret" id="ap-text-caret">▾</span></button>
        </div>
      </div>
      <div id="ap-text-sliders" class="ap-sliders-panel${textPanelOpen ? "" : " hidden"}">
        ${appearanceSliderRow("settings.appearance_text_light", "textLight", -100, 100, 1, cfg.textLight, v => formatAppearanceValue("textLight", v))}
      </div>
      </div>

      <div class="settings-group-title">${t("settings.appearance_theme_section")}</div>
      <div class="ap-themes" id="ap-themes">${themeButtons}</div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.appearance_theme_hint")}</span>
        <div class="settings-bg-actions">
          <button type="button" class="settings-btn" id="ap-theme-dir">${t("settings.appearance_theme_open_dir")}</button>
        </div>
      </div>
    </div>`;
}

/** 「风格」分区的事件绑定。独立成函数（不塞进 attachSettingsListeners）是为了
 *  让外观这块的改动范围收敛在一个地方。 */
function attachAppearanceControls(container: HTMLElement, ap: AppearanceBridge): void {
  const pick = <T extends HTMLElement>(sel: string) => container.querySelector(sel) as T | null;
  const cfg = ap.get();

  // ── 滑块（三滑块 + 玻璃透明度）：一个监听覆盖所有 data-ap ──
  container.querySelectorAll<HTMLInputElement>('input[type="range"][data-ap]').forEach(el => {
    el.addEventListener("input", () => {
      const field = el.dataset.ap as keyof AppearanceConfig;
      const v = Number(el.value);
      ap.set({ [field]: v } as Partial<AppearanceConfig>);
      const out = container.querySelector(`[data-ap-val="${field}"]`);
      if (out) out.textContent = formatAppearanceValue(field, v);
    });
  });

  // ── 背景：三个按钮（选择图片 / 自定义 / 清除）───────────────
  // 「自定义」= 展开/收起**五个拉条**，不是另一种背景来源 —— 用户明确要的是
  // 「点自定义才出现 毛玻璃化/饱和度/背景透明度/反光/界面玻璃透明度」。
  const bgName = pick("#ap-bg-name");
  const bgSliders = pick("#ap-bg-sliders");
  const bgCaret = pick("#ap-bg-caret");
  const syncBgRow = () => {
    const c = ap.get();
    if (bgName) bgName.textContent = c.bgImage ? t("settings.appearance_bg_set") : t("settings.appearance_bg_none");
  };
  pick("#ap-bg-pick")?.addEventListener("click", async () => {
    if (await ap.pickBgImage()) syncBgRow();
  });
  pick("#ap-bg-custom")?.addEventListener("click", () => {
    bgSlidersOpen = !bgSlidersOpen;
    bgSliders?.classList.toggle("hidden", !bgSlidersOpen);
    if (bgCaret) bgCaret.textContent = bgSlidersOpen ? "▴" : "▾";
  });
  pick("#ap-bg-clear")?.addEventListener("click", () => {
    ap.set({ bgImage: null });
    syncBgRow();
  });

  // ── 三个取色器 + 各自的「饱和度 / 明度」滑块：**同一组值、双向联动**（2026-09-20 用户改定）
  // 每个 slot 的当前颜色只留一份（`colors[slot].hex`），两条输入路径都写它：
  //   · 取色器（色板 / 色相条 / hex / 预设 / 屏幕取色）→ commit → `syncAxes()` 回写滑块；
  //   · 滑块 → `hsvToHex()` 算出新色 → `handle.apply()` 回写色板（**不 commit**，防回环）。
  // 「跟随态」（字段是空串）下起点是 main.ts 给的派生色；用户一动就写成真 hex，
  // 从此以用户的为准（用户选的那一档：「没动过就自动派生」）。
  // **主题色取色器已删除**：主题色只由主题包提供，所以这里只剩底色 / 按钮线条 / 按钮背景三个。
  const sw = ap.resolvedSwatches();
  const colors: Record<string, { id: string; hex: string; set: (hex: string) => void }> = {
    base: { id: "ap-base", hex: cfg.baseColor || sw.base, set: (hex) => ap.set({ baseColor: hex }) },
    btnline: { id: "ap-btnline", hex: cfg.btnLineColor || sw.btnLine, set: (hex) => ap.set({ btnLineColor: hex }) },
    btnbg: { id: "ap-btnbg", hex: cfg.btnBgColor || sw.btnBg, set: (hex) => ap.set({ btnBgColor: hex }) },
  };
  const syncAxes = (slot: string) => {
    const hsv = hexToHsv(colors[slot].hex);
    if (!hsv) return;
    for (const k of ["sat", "val"] as const) {
      const v = Math.round((k === "sat" ? hsv.s : hsv.v) * 100);
      const el = pick<HTMLInputElement>(`input[data-color="${slot}"][data-axis="${k}"]`);
      if (el) el.value = String(v);
      const out = container.querySelector(`[data-axis-val="${slot}-${k}"]`);
      if (out) out.textContent = String(v);
    }
  };
  const handles: Record<string, ColorPickerHandle> = {};
  for (const slot of Object.keys(colors)) {
    const c = colors[slot];
    handles[slot] = mountColorPicker(container, c.id, c.hex, (hex) => {
      c.hex = hex;
      c.set(hex);
      syncAxes(slot);
    });
  }
  // 拖「饱和度 / 明度」滑块 = 直接改色板的**那一轴**（不是微调偏移）——
  // 所以色相从当前色现取，只替换被拖动的那一轴。写完之后滑块与色板必然一致，
  // 不存在「两组值互相打架」（那正是 2026-09-19 删掉色轮那三件套的理由）。
  container.querySelectorAll<HTMLInputElement>('input[type="range"][data-axis]').forEach(el => {
    el.addEventListener("input", () => {
      const slot = el.dataset.color ?? "";
      const c = colors[slot];
      if (!c) return;
      const hsv = hexToHsv(c.hex);
      if (!hsv) return;
      const nv = Number(el.value) / 100;
      const next = hsvToHex(hsv.h, el.dataset.axis === "sat" ? nv : hsv.s,
        el.dataset.axis === "val" ? nv : hsv.v);
      c.hex = next;
      c.set(next);
      handles[slot].apply(next);
      syncAxes(slot);
    });
  });

  /** 「恢复默认主题」开着 ⇒ **锁住三组配色，放开三个透明度 + 文字明度**（2026-09-20 四次定稿）。
   *
   *  锁住的是：三个取色器 + 三对「饱和度 / 明度」滑块；**留在锁外**的是
   *  底色透明度 / 按钮线条透明度 / 按钮背景透明度（用户要求「除透明度外都不可调」）
   *  以及**文字明度**（用户随后明确「文字明度不锁定」）。
   *  锚点统一是 `data-tint-lock`（见 `tl()`），所以加新项只要标一下属性，不会漏。
   *  用 `.locked`（遮点击 + 降透明度，同「主题锁」那套）而不是 `disabled`：
   *  自绘取色器不认 disabled（ai-spec 规则 46 已登记过这条教训）。 */
  const syncTintLock = (on: boolean) => {
    container.querySelectorAll<HTMLElement>("[data-tint-lock]").forEach(el =>
      el.classList.toggle("locked", on));
    pick("#ap-restore-note")?.classList.toggle("hidden", !on);
  };
  const tintToggle = pick<HTMLInputElement>("#ap-tint-base");
  tintToggle?.addEventListener("change", () => {
    ap.set({ tintBase: !!tintToggle.checked });
    syncTintLock(!!tintToggle.checked);
  });

  // ── 四组「自定义」的展开与收起（形态同背景那组）──
  const bindPanel = (
    btnSel: string, panelSel: string, caretSel: string,
    get: () => boolean, set: (v: boolean) => void,
  ) => {
    pick(btnSel)?.addEventListener("click", () => {
      const next = !get();
      set(next);
      pick(panelSel)?.classList.toggle("hidden", !next);
      const caret = pick(caretSel);
      if (caret) caret.textContent = next ? "▴" : "▾";
    });
  };
  bindPanel("#ap-base-custom", "#ap-base-sliders", "#ap-base-caret",
    () => basePanelOpen, v => { basePanelOpen = v; });
  bindPanel("#ap-btn-custom", "#ap-btn-sliders", "#ap-btn-caret",
    () => btnPanelOpen, v => { btnPanelOpen = v; });
  bindPanel("#ap-text-custom", "#ap-text-sliders", "#ap-text-caret",
    () => textPanelOpen, v => { textPanelOpen = v; });

  // ── 主题包：单选。`main.ts` 的 set() 会在 themeId 变化时重画结果列表（图标）──
  // 同步「主题锁」：只有默认主题允许改主题色与背景图片（见 buildAppearancePane）。
  const colorGroup = pick("#ap-color-group");
  const lockNote = pick("#ap-lock-note");
  const bgPickBtn = pick<HTMLButtonElement>("#ap-bg-pick");
  const syncThemeLock = (themeId: string) => {
    const locked = themeId !== "default";
    colorGroup?.classList.toggle("locked", locked);
    lockNote?.classList.toggle("hidden", !locked);
    if (bgPickBtn) bgPickBtn.disabled = locked;
  };
  container.querySelectorAll<HTMLElement>("[data-theme]").forEach(btn => {
    btn.addEventListener("click", () => {
      const id = btn.dataset.theme || "default";
      ap.set({ themeId: id });
      container.querySelectorAll<HTMLElement>("[data-theme]").forEach(b =>
        b.classList.toggle("active", b.dataset.theme === id));
      syncThemeLock(id);
    });
  });
  pick("#ap-theme-dir")?.addEventListener("click", async () => {
    try { await open(await ap.themesDir()); } catch (e) { console.error("[lunac] open themes dir failed:", e); }
  });
}

async function buildSearchPane(): Promise<string> {
  let engine = "google";
  try {
    const saved = localStorage.getItem("lunac-search-engine");
    // DuckDuckGo preset removed (2026-09) — migrate stale saved value back to Google
    if (saved === "google" || saved === "bing" || saved === "baidu") {
      engine = saved;
    } else if (saved) {
      engine = "google";
      localStorage.setItem("lunac-search-engine", "google");
    }
  } catch {}

  const engines = [
    { id: "google",     name: "Google",       url: "https://www.google.com/search?q={{query}}" },
    { id: "bing",       name: "Bing",         url: "https://www.bing.com/search?q={{query}}" },
    { id: "baidu",      name: "Baidu",        url: "https://www.baidu.com/s?wd={{query}}" },
  ];

  const renderCustomSelect = (id: string, options: { value: string; label: string; selected: boolean; tag?: string }[], allowDelete = false): string => {
    const optsHtml = options.map(o => {
      const del = allowDelete && o.value !== "__custom__" ? `<span class="cs-del" title="×">×</span>` : "";
      return `<div class="custom-select-option${o.selected ? ' selected' : ''}" data-value="${esc(o.value)}" data-tag="${o.tag || 'builtin'}">${esc(o.label)}${del}</div>`;
    }).join("");
    const sel = options.find(o => o.selected);
    return `
      <div class="custom-select" id="${id}">
        <button class="custom-select-trigger" type="button">
          <span class="custom-select-label">${esc(sel?.label || options[0]?.label || "")}</span>
          <svg class="custom-select-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="6 9 12 15 18 9"/></svg>
        </button>
        <div class="custom-select-dropdown">${optsHtml}</div>
      </div>`;
  };

  const engSelectHtml = renderCustomSelect("settings-search-engine", engines.map(e => ({
    value: e.id,
    label: e.name,
    selected: e.id === engine,
  })));

  return `
    <div class="settings-pane" data-pane="search" id="sp-search">
      <div class="settings-pane-title">${t("settings.web_search")}</div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.search_engine")}</span>
        ${engSelectHtml}
      </div>
      <div class="settings-row" style="justify-content: flex-end;">
        <button id="settings-save-search-btn" class="settings-save-btn">${t("settings.save")}</button>
        <span id="settings-save-search-msg" class="settings-save-msg"></span>
      </div>
      <div class="settings-hint" style="margin-top:8px;font-size:0.7rem;color:var(--text-dim);">${t("settings.search_engine_hint")}</div>
    </div>`;
}

// ── AI 供应商/模型预设（单一数据源）─────────────────────────────
// buildAIPane 与 attachSettingsListeners 共用，避免两处漂移。
// 接口地址默认不带 /v1 —— 与主流供应商文档一致；agent 端点由
// agent.exe 启动时在 base 上派生（commands.rs 已处理 /v1 双重路径）。
interface ModelPreset { name: string; default_model: string; default_url: string; }
const PROVIDER_PRESETS: Record<string, ModelPreset> = {
  "openai":       { name: "OpenAI",         default_model: "gpt-5.5",            default_url: "https://api.openai.com" },
  "deepseek":     { name: "DeepSeek",       default_model: "deepseek-flash",     default_url: "https://api.deepseek.com" },
  "anthropic":    { name: "Anthropic",      default_model: "claude-sonnet-5",    default_url: "https://api.anthropic.com" },
  "google":       { name: "Google Gemini",  default_model: "gemini-3.6-flash",   default_url: "https://generativelanguage.googleapis.com/v1beta/openai" },
  "zhipu":        { name: "Zhipu GLM",      default_model: "glm-5",              default_url: "https://open.bigmodel.cn/api/paas/v4" },
  "moonshot":     { name: "Moonshot Kimi",  default_model: "kimi-k3",            default_url: "https://api.moonshot.cn" },
  "qwen":         { name: "Qwen (Tongyi)",  default_model: "qwen3.8-max",        default_url: "https://dashscope.aliyuncs.com/compatible-mode" },
  "siliconflow":  { name: "SiliconFlow",    default_model: "Qwen/Qwen3-235B-A22B", default_url: "https://api.siliconflow.cn" },
  "custom":       { name: "Custom",         default_model: "",                   default_url: "" },
};

const MODEL_SUGGESTIONS: Record<string, string[]> = {
  "openai":     ["gpt-5.5", "gpt-5.5-pro", "gpt-5.4", "gpt-5.4-mini", "gpt-5.4-nano"],
  // DeepSeek 官方 Anthropic 兼容端点实测可用名（2026-09-15 用真实 key 逐个打到 200）：
  // "deepseek-v4-pro" / "deepseek-flash" / "deepseek-v4-flash"。
  // **三个都要列** —— 少列一个，用户存过的那个名字就会被面板当成未知模型、
  // 在切供应商时被静默改写成预设默认值（真实事故：存 deepseek-v4-flash → 变 v4-pro）。
  "deepseek":   ["deepseek-flash", "deepseek-v4-flash", "deepseek-v4-pro"],
  "anthropic":  ["claude-sonnet-5", "claude-opus-5", "claude-fable-5", "claude-haiku-4-5-20251001"],
  "google":     ["gemini-3.6-flash", "gemini-3.5-flash", "gemini-3.1-pro-preview", "gemini-3.1-flash-lite"],
  "zhipu":      ["glm-5", "glm-4.7", "glm-4.5-air", "glm-4.5-flash"],
  "moonshot":   ["kimi-k3", "kimi-k2.6", "kimi-k2.7-code"],
  "qwen":       ["qwen3.8-max", "qwen3.7-max", "qwen3.7-plus", "qwen3.7-flash"],
  "siliconflow": ["Qwen/Qwen3-235B-A22B", "deepseek-ai/DeepSeek-V3", "Qwen/Qwen3-30B-A3B", "zai-org/GLM-5"],
  "custom":     [],
};

/** 由模型名反推其所属供应商（用于未配置 provider 时的默认值兜底）。 */
function inferProviderForModel(model: string, fallback: string): string {
  if (!model) return fallback;
  for (const [k, list] of Object.entries(MODEL_SUGGESTIONS)) {
    if (k !== "custom" && list.includes(model)) return k;
  }
  for (const [k, p] of Object.entries(PROVIDER_PRESETS)) {
    if (k !== "custom" && p.default_model === model) return k;
  }
  return fallback;
}

/** 该模型名是否属于**别的**供应商（只有这种情况才允许在切供应商时回落默认值）。
 *
 * 反过来说：认不出来的名字（用户手输的自定义名、或清单里暂时漏列的名字）**必须原样保留** ——
 * 早先的实现是「不在候选清单里就回落预设默认值」，于是「存 `deepseek-v4-flash` →
 * 切一次供应商 → 变成 `deepseek-v4-pro` → 保存落盘」这条静默改写路径真实发生过。
 * 判断标准只看「是不是别的供应商的名字」，与当前供应商的清单是否完整无关。 */
function belongsToOtherProvider(model: string, provider: string): boolean {
  if (!model) return false;
  for (const [k, list] of Object.entries(MODEL_SUGGESTIONS)) {
    if (k !== "custom" && k !== provider && list.includes(model)) return true;
  }
  for (const [k, p] of Object.entries(PROVIDER_PRESETS)) {
    if (k !== "custom" && k !== provider && p.default_model === model) return true;
  }
  return false;
}

/** AI 模型这一块的**内部内容**（不含 pane 外壳与分块标题）——
 *  由 buildAIPane 组装进「AI」分类。2026-09-19 批 9 起 AI 分类下有三个分块
 *  （AI 模型 / 技能 / 工具），每个分块用与「风格」相同的 .settings-group-title。 */
function buildAIModelSection(provider: string, baseUrl: string, model: string, apiKey: string, searchProvider: string, searchKey: string, vision: boolean, hooksEnabled: boolean, hooksError: string): string {
  const masked = apiKey ? apiKey.slice(0, 4) + "\u2022\u2022\u2022\u2022" + apiKey.slice(-4) : "";

  // Filter out built-in providers the user deleted (persisted hidden-list)
  let hiddenProvs: string[] = [];
  try { hiddenProvs = JSON.parse(localStorage.getItem("lunac-hidden-providers") || "[]"); } catch {}
  const provKeys = Object.keys(PROVIDER_PRESETS).filter(p => !hiddenProvs.includes(p) || p === provider);
  // Custom dropdown HTML helper
  const renderCustomSelect = (id: string, options: { value: string; label: string; selected: boolean; tag?: string }[], allowDelete = false): string => {
    const optsHtml = options.map(o => {
      const del = allowDelete && o.value !== "__custom__" ? `<span class="cs-del" title="×">×</span>` : "";
      return `<div class="custom-select-option${o.selected ? ' selected' : ''}" data-value="${esc(o.value)}" data-tag="${o.tag || 'builtin'}">${esc(o.label)}${del}</div>`;
    }).join("");
    const selectedLabel = options.find(o => o.selected)?.label || options[0]?.label || "";
    return `
      <div class="custom-select" id="${id}">
        <button class="custom-select-trigger" type="button">
          <span class="custom-select-label">${esc(selectedLabel)}</span>
          <svg class="custom-select-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="6 9 12 15 18 9"/></svg>
        </button>
        <div class="custom-select-dropdown">${optsHtml}</div>
      </div>`;
  };
  const provOpts: { value: string; label: string; selected: boolean; tag?: string }[] = provKeys.map(p => ({
    value: p,
    label: PROVIDER_PRESETS[p].name,
    selected: p === provider
  }));
  // 已保存的自定义供应商（不在预设列表中）→ 显示为选中项
  if (provider && !provKeys.includes(provider)) {
    provOpts.push({ value: provider, label: provider, selected: true, tag: "custom" });
  }
  // 追加持久化的自定义供应商（重开设置面板仍在）
  try {
    const customProvs: string[] = JSON.parse(localStorage.getItem("lunac-custom-providers") || "[]");
    for (const p of customProvs) {
      if (p && !provKeys.includes(p) && p !== provider) {
        provOpts.push({ value: p, label: p, selected: false, tag: "custom" });
      }
    }
  } catch {}
  // 底部保留 "__custom__" 入口，用于新增自定义供应商
  provOpts.push({ value: "__custom__", label: t("settings.custom_provider"), selected: false });
  const provSelectHtml = renderCustomSelect("settings-provider", provOpts, true);

    // ── Model custom select ──────────────────────────────────
    // Filter out built-in models the user deleted (persisted hidden-list)
    let hiddenModels: string[] = [];
    try {
      const hm = JSON.parse(localStorage.getItem("lunac-hidden-models") || "{}");
      hiddenModels = (hm && hm[provider]) || [];
    } catch {}
    const modelList = (MODEL_SUGGESTIONS[provider] || []).filter(m => !hiddenModels.includes(m) || m === model);
    const hasCustomModel = model && !modelList.includes(model);
    const modelOptions: { value: string; label: string; selected: boolean; tag?: string }[] = modelList.map(m => ({
      value: m,
      label: m,
      selected: m === model
    }));
    if (hasCustomModel) {
      modelOptions.push({ value: model, label: `${model} (${t("settings.custom_model")})`, selected: true, tag: "custom" });
    }
    // 追加持久化的自定义模型（重开设置面板仍在）
    try {
      const map = JSON.parse(localStorage.getItem("lunac-custom-models") || "{}");
      const customModels: string[] = (map && map[provider]) || [];
      for (const m of customModels) {
        if (m && !modelList.includes(m) && m !== model) {
          modelOptions.push({ value: m, label: `${m} (${t("settings.custom_model")})`, selected: false, tag: "custom" });
        }
      }
    } catch {}
    modelOptions.push({ value: "__custom__", label: t("settings.custom_model"), selected: false });
    const modelSelectHtml = renderCustomSelect("settings-model", modelOptions, true);

  // WebSearch 主源服务商（值为 agent 读的 LUNAC_SEARCH_PROVIDER；空 = 只用免 key 兜底源）。
  // 品牌名不翻译；「不使用」走 i18n。
  const searchProvSelectHtml = renderCustomSelect("settings-search-provider",
    [
      { value: "", label: t("settings.search_provider_none") },
      { value: "bocha", label: "博查 Bocha" },
      { value: "tavily", label: "Tavily" },
      { value: "exa", label: "Exa" },
      { value: "firecrawl", label: "Firecrawl" },
    ].map(o => ({ ...o, selected: o.value === searchProvider })));

  // 安全档位（文件边界）：与「运行方式」（问不问）正交，这里是「允不允许」。
  // 真实下发在 main.ts（单一 invoke 点），本面板只广播选择，见 agent-ui-spec §4.4。
  let securityProfile = "project";
  try {
    const p = localStorage.getItem("lunac-security-profile");
    if (p === "safe" || p === "full") securityProfile = p;
  } catch {}
  const profileSelectHtml = renderCustomSelect("settings-security-profile",
    [
      { value: "safe", label: t("settings.security_profile_ro") },
      { value: "project", label: t("settings.security_profile_project") },
      { value: "full", label: t("settings.security_profile_full") },
    ].map(o => ({ ...o, selected: o.value === securityProfile })));

  // 回合结束自动折叠（main.ts 每次都现读，故这里只写 localStorage、不广播）
  let autofoldOn = true;
  try {
    autofoldOn = localStorage.getItem("lunac-agent-autofold") !== "0";
  } catch {}

  return `
      <div class="settings-row">
        <span class="settings-label">${t("settings.provider")}</span>
        ${provSelectHtml}
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.model")}</span>
        ${modelSelectHtml}
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.ai_vision")}</span>
        <label class="settings-toggle">
          <input type="checkbox" id="settings-vision" ${vision ? "checked" : ""}>
          <span class="settings-toggle-slider"></span>
        </label>
      </div>
      <div class="settings-hint" style="font-size:0.7rem;color:var(--text-dim);margin:2px 0 6px;">${t("settings.ai_vision_hint")}</div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.hooks")}</span>
        <label class="settings-toggle">
          <input type="checkbox" id="settings-hooks" ${hooksEnabled ? "checked" : ""}>
          <span class="settings-toggle-slider"></span>
        </label>
      </div>
      <div class="settings-hint" style="font-size:0.7rem;color:var(--text-dim);margin:2px 0 6px;">${t("settings.hooks_hint")}</div>
      <div class="settings-row">
        <button type="button" class="settings-btn" id="settings-hooks-open">${t("settings.hooks_open")}</button>
        <span class="settings-hint" id="settings-hooks-msg" style="font-size:0.7rem;color:var(--text-dim);margin-left:8px;"></span>
      </div>
      ${hooksError ? `<div class="settings-hint" style="font-size:0.7rem;color:var(--yellow);margin:2px 0 6px;">${esc(t("settings.hooks_invalid", { err: hooksError }))}</div>` : ""}
      <div class="settings-row">
        <span class="settings-label">${t("settings.base_url")}</span>
        <input type="text" id="settings-baseurl" class="settings-input" autocomplete="off" value="${esc(baseUrl)}" placeholder="${esc(PROVIDER_PRESETS[provider]?.default_url || "https://api.openai.com")}">
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.api_key")}</span>
        <input type="password" id="settings-apikey" class="settings-input" autocomplete="off" value="${esc(apiKey)}" placeholder="${masked || 'sk-...'}">
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.search_provider")}</span>
        ${searchProvSelectHtml}
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.search_key")}</span>
        <input type="password" id="settings-searchkey" class="settings-input" autocomplete="off" value="${esc(searchKey)}" placeholder="${esc(t("settings.search_key_hint"))}" title="${esc(t("settings.search_key_hint"))}">
      </div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.security_profile")}</span>
        ${profileSelectHtml}
      </div>
      <div class="settings-hint" style="font-size:0.7rem;color:var(--text-dim);margin:2px 0 6px;">${t("settings.security_profile_hint")}</div>
      <div class="settings-row">
        <span class="settings-label">${t("settings.agent_autofold")}</span>
        <label class="settings-toggle">
          <input type="checkbox" id="settings-autofold" ${autofoldOn ? "checked" : ""}>
          <span class="settings-toggle-slider"></span>
        </label>
      </div>
      <div class="settings-hint" style="font-size:0.7rem;color:var(--text-dim);margin:2px 0 6px;">${t("settings.agent_autofold_hint")}</div>
      <div class="settings-row" style="justify-content: flex-end;">
        <button id="settings-save-ai-btn" class="settings-save-btn">${t("settings.save")}</button>
        <span id="settings-save-msg" class="settings-save-msg"></span>
      </div>`;
}

// ── AI 分类（2026-09-19 批 9：三个分块合成一个分类）────────────────
/** AI 分类 = **AI 模型 + 技能（Skills）+ 工具（Tools / MCP）+ 用量与成本（A12）+ 人格（L2）**。
 *  用户要求：「将 skills 和 tools 和 ai模型 分类到 ai 分类里，各个分块采用跟
 *  风格里的分块一样」—— 所以各块都用 .settings-group-title（与「风格」的
 *  背景 / 主题颜色 / 主题包 完全同款），侧栏项只剩「AI」这一个。 */
async function buildAIPane(provider: string, baseUrl: string, model: string, apiKey: string, searchProvider: string, searchKey: string, vision: boolean, hooksEnabled: boolean, hooksError: string): Promise<string> {
  const [skillsHtml, toolsHtml, costHtml, personaHtml] = await Promise.all([buildSkillsSection(), buildToolsSection(), buildUsageCostSection(), buildPersonaSection()]);
  return `
    <div class="settings-pane" data-pane="ai" id="sp-ai">
      <div class="settings-pane-title">${t("settings.sidebar_ai")}</div>
      <div class="settings-group-title">${t("settings.ai_model")}</div>
      ${buildAIModelSection(provider, baseUrl, model, apiKey, searchProvider, searchKey, vision, hooksEnabled, hooksError)}
      <div class="settings-group-title">${t("settings.skills")}</div>
      ${skillsHtml}
      <div class="settings-group-title">${t("settings.group_tools")}</div>
      ${toolsHtml}
      <div class="settings-group-title">${t("settings.cost_title")}</div>
      ${costHtml}
      <div class="settings-group-title">${t("settings.persona_title")}</div>
      ${personaHtml}
    </div>`;
}

// ── 人格 / 自定义提示词（L2）────────────────────────────────────
//
// 形态：一段纯文本落 `config\persona.md`，agent **启动时读一次**、拼进系统提示词的固定段
// （在内置人格段之后）。所以保存后必须**重启 AI** 才生效 —— 面板如实写明这条，并给
// 「立即重启」按钮（复用 `window.__lunac_reload_agent`，与技能目录 / 工具黑名单保存后同款）。
// **刻意不做热更新**：改的正是固定前缀，热读会让系统提示词每轮都变、把端点侧缓存整段打掉
// （ai-spec §11 规则 18/23）—— 那是我们花了两批工作才摆脱的东西。
//
// 另有一条硬约束要在面板上说清：这段**不进**子代理与后台复盘（它们是内部产物）。

interface PersonaState { path: string; text: string; maxChars: number }

async function buildPersonaSection(): Promise<string> {
  return `<div id="settings-persona">${await renderPersonaBody()}</div>`;
}

async function renderPersonaBody(): Promise<string> {
  let state: PersonaState | null = null;
  try {
    state = await invoke<PersonaState>("get_persona");
  } catch {}
  const text = state?.text ?? "";
  // 上限由宿主给（storage::MAX_PERSONA_CHARS 是唯一真相源）：面板只用它做 maxlength 与计数
  const max = state?.maxChars ?? 8000;
  return `
    <textarea id="settings-persona-text" class="settings-persona-text" spellcheck="false"
      maxlength="${max}" placeholder="${esc(t("settings.persona_placeholder"))}">${esc(text)}</textarea>
    <div class="settings-hint" id="settings-persona-count"></div>
    <div class="settings-row">
      <button type="button" class="settings-btn" id="settings-persona-save">${t("settings.persona_save")}</button>
      <button type="button" class="settings-btn" id="settings-persona-reset">${t("settings.persona_reset")}</button>
      <button type="button" class="settings-btn" id="settings-persona-restart">${t("settings.persona_restart")}</button>
      <span class="settings-hint" id="settings-persona-msg" style="margin-left:8px;"></span>
    </div>
    <div class="settings-hint">${esc(t("settings.persona_hint", { max: String(max) }))}</div>
    <div class="settings-hint" title="${esc(state?.path || "")}">${esc(t("settings.persona_path", { path: state?.path || "" }))}</div>`;
}

function wirePersona(container: HTMLElement): void {
  const box = container.querySelector("#settings-persona-text") as HTMLTextAreaElement | null;
  // 提示文字每次现查节点：容器整体重渲染会把 `#settings-persona-msg` 整个换掉
  const showMsg = (text: string) => {
    const el = container.querySelector("#settings-persona-msg") as HTMLElement | null;
    if (el) el.textContent = text;
  };
  const count = () => {
    const el = container.querySelector("#settings-persona-count") as HTMLElement | null;
    if (el && box) {
      el.textContent = t("settings.persona_count", {
        n: String(box.value.length),
        max: String(Number(box.maxLength) || 8000),
      });
    }
  };
  box?.addEventListener("input", count);
  count();

  const save = async (text: string, okMsg: string) => {
    try {
      await invoke("set_persona", { text });
      // 保存 ≠ 生效：改的是系统提示词的固定前缀，得重启 agent 才读得到（如实告知）
      showMsg(okMsg + t("settings.persona_takes_effect"));
    } catch (e) {
      showMsg(t("settings.persona_failed", { err: String(e) }));
    }
  };

  container.querySelector("#settings-persona-save")?.addEventListener("click", () => {
    void save(box?.value ?? "", t("settings.persona_saved"));
  });

  container.querySelector("#settings-persona-reset")?.addEventListener("click", async () => {
    if (box) box.value = "";
    count();
    await save("", t("settings.persona_reset_done"));
  });

  container.querySelector("#settings-persona-restart")?.addEventListener("click", async () => {
    const fn = (window as any).__lunac_reload_agent;
    if (typeof fn !== "function") {
      showMsg(t("settings.persona_failed", { err: "reload hook missing" }));
      return;
    }
    try {
      await fn();
      showMsg(t("settings.persona_restarted"));
    } catch (e) {
      showMsg(t("settings.persona_failed", { err: String(e) }));
    }
  });
}

// ── 用量与成本（A12）──────────────────────────────────────────────
//
// 面板只做三件事：把**本地用量日志**按天汇总、按 `config\pricing.json` 里的单价算钱、
// 以及**确认 agent 抓回来的候选价格**。三条纪律：
//  ① 价格不写进代码。各家单价差十倍、官方还会调价，写死一个数字等于把错误金额当事实
//     展示；价格表是用户可编辑的文件，面板只读它。
//  ② 「更新价格」不由面板自己抓（它没有网络能力也没有模型），而是把任务交给 agent；
//     agent 只能写**候选文件**，面板上列出「旧值 → 新值」，用户点确认才覆盖正式价格。
//  ③ 金额一律在前端算：价格表是用户随时会改的，改完即时重算，不必再跑一趟 IPC。

/** 汇总区间（天）。日志按天分片，这里是 30 个文件、一次 IPC 读完（`read_usage_range`）。 */
const COST_RANGE_DAYS = 30;

interface UsageModelTotals {
  model: string;
  turns: number;
  input: number;
  output: number;
  cacheRead: number;
  cacheCreate: number;
}
interface UsageDay {
  date: string;
  turns: number;
  input: number;
  output: number;
  cacheRead: number;
  cacheCreate: number;
  models: UsageModelTotals[];
}
interface PricingEntry {
  input?: number;
  cache_read?: number;
  cache_write?: number;
  output?: number;
  source_url?: string;
  updated_at?: string;
}
interface PricingFile {
  updated_at?: string;
  models?: Record<string, PricingEntry>;
}
interface PricingState {
  path: string;
  text: string;
  pendingPath: string;
  pendingText: string;
}

/** 本地日期 `YYYY-MM-DD`（与 main.ts 的用量日志分片键同一套口径） */
function costDateKey(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** 近 n 天的本地日期，升序（宿主按同样的顺序读回来） */
function lastNDates(n: number): string[] {
  const today = new Date();
  const out: string[] = [];
  for (let i = n - 1; i >= 0; i--) {
    out.push(costDateKey(new Date(today.getFullYear(), today.getMonth(), today.getDate() - i)));
  }
  return out;
}

function parsePricing(text: string): PricingFile | null {
  const trimmed = (text || "").trim();
  if (!trimmed) return null;
  try {
    const v = JSON.parse(trimmed) as PricingFile;
    return v && typeof v === "object" ? v : null;
  } catch {
    return null; // 语法坏 = 没有价格（面板按「未定价」处理，不去猜）
  }
}

function priceOf(pricing: PricingFile | null, model: string): PricingEntry | null {
  const e = pricing?.models?.[model];
  return e && typeof e === "object" ? e : null;
}

/** 一天的金额。**必须逐模型算**：一天里换过模型的话，只按天合计就把两个模型的量
 *  混在一起了（单价差十倍）。`exact=false` 表示这天有模型没价格，金额只是已知部分 ——
 *  面板要如实标出来，不要假装它是个准确值。 */
function dayCost(day: UsageDay, pricing: PricingFile | null): { amount: number; exact: boolean } {
  let amount = 0;
  let exact = true;
  for (const m of day.models) {
    const p = priceOf(pricing, m.model);
    if (!p) {
      exact = false;
      continue;
    }
    amount += (
      (m.input || 0) * (p.input || 0)
      + (m.cacheRead || 0) * (p.cache_read || 0)
      + (m.cacheCreate || 0) * (p.cache_write || 0)
      + (m.output || 0) * (p.output || 0)
    ) / 1e6;
  }
  return { amount, exact };
}

/** 金额显示：单价是「元 / 百万 token」，单次提问常常只有几厘 —— 太小就多给两位小数，
 *  否则一律显示 0.00，等于把「花了很少」和「没花钱」显示成同一个样子。 */
function fmtMoney(v: number): string {
  if (!(v > 0)) return "0.00";
  return v < 0.01 ? v.toFixed(4) : v.toFixed(2);
}

/** token 数显示（表格里用 k / M 缩写，与主界面表盘同风格） */
function fmtTokenCount(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(2) + "M";
  if (n >= 1_000) return (n / 1_000).toFixed(1) + "k";
  return String(n);
}

/** 模型名显示：早期用量日志的模型名是空的（前端当时没记），显示成 `—` 而不是空单元格
 *  —— 空名字看起来像渲染坏了，而它确实会被算进「未定价」。 */
function modelLabel(model: string): string {
  return model ? model : "—";
}

/** 「旧值 → 新值」逐行列出候选价格。**只列变化的**（没变的模型不占版面），
 *  另外单列「新增」与「确认后失去价格」两类 —— 后者是整体覆盖的必然结果，
 *  不写出来用户会以为旧价格还在。 */
function pricingPreviewRows(current: PricingFile | null, next: PricingFile): string {
  const fields: { key: keyof PricingEntry; label: string }[] = [
    { key: "input", label: t("settings.cost_col_input") },
    { key: "cache_read", label: t("settings.cost_col_hit") },
    { key: "cache_write", label: t("settings.cost_col_write") },
    { key: "output", label: t("settings.cost_col_output") },
  ];
  const num = (e: PricingEntry | null, k: keyof PricingEntry): number | null =>
    e && typeof e[k] === "number" ? (e[k] as number) : null;

  const rows: string[] = [];
  const nextModels = next.models || {};
  const curModels = current?.models || {};

  for (const [model, entry] of Object.entries(nextModels)) {
    const old = curModels[model] ?? null;
    const parts: string[] = [];
    if (!old) {
      for (const f of fields) {
        const v = num(entry, f.key);
        if (v !== null) parts.push(`${f.label} ${v}`);
      }
    } else {
      for (const f of fields) {
        const a = num(old, f.key);
        const b = num(entry, f.key);
        if (a === null || b === null || a === b) continue;
        parts.push(`${f.label} ${a} → ${b}`);
      }
    }
    const badge = old ? "" : ` <span class="cost-new">${t("settings.cost_pending_added")}</span>`;
    const src = entry.source_url ? `<div class="cost-src">${esc(entry.source_url)}</div>` : "";
    const cell = parts.length
      ? esc(parts.join(" · "))
      : `<span class="cost-src">—</span>`;
    rows.push(`<tr><td>${esc(modelLabel(model))}${badge}${src}</td><td>${cell}</td></tr>`);
  }
  for (const model of Object.keys(curModels)) {
    if (!(model in nextModels)) {
      rows.push(
        `<tr><td>${esc(modelLabel(model))}</td><td><span class="cost-warn">${t("settings.cost_pending_removed")}</span></td></tr>`
      );
    }
  }
  return rows.join("");
}

/** 「用量与成本」分块（内层容器在确认 / 放弃后会整体重渲染，见 wireUsageCost） */
async function buildUsageCostSection(): Promise<string> {
  return `<div id="settings-usage-cost">${await renderUsageCostBody()}</div>`;
}

async function renderUsageCostBody(): Promise<string> {
  let state: PricingState | null = null;
  try {
    state = await invoke<PricingState>("get_pricing_state");
  } catch {}
  let days: UsageDay[] = [];
  try {
    days = await invoke<UsageDay[]>("read_usage_range", { dates: lastNDates(COST_RANGE_DAYS) });
  } catch {}

  const pricing = state ? parsePricing(state.text) : null;
  const pending = state ? parsePricing(state.pendingText) : null;
  const models = pricing?.models || {};
  const hasPrices = Object.keys(models).length > 0;

  const totals = { turns: 0, input: 0, hit: 0, write: 0, output: 0, amount: 0, exact: true };
  const unpriced = new Set<string>();
  for (const d of days) {
    totals.turns += d.turns;
    totals.input += d.input;
    totals.hit += d.cacheRead;
    totals.write += d.cacheCreate;
    totals.output += d.output;
    const c = dayCost(d, pricing);
    totals.amount += c.amount;
    if (!c.exact) totals.exact = false;
    for (const m of d.models) if (!priceOf(pricing, m.model)) unpriced.add(m.model);
  }
  const hitRate = totals.hit + totals.input > 0
    ? Math.round((totals.hit / (totals.hit + totals.input)) * 100)
    : 0;

  const summary = days.length === 0
    ? `<div class="settings-hint">${t("settings.cost_no_usage", { days: String(COST_RANGE_DAYS) })}</div>`
    : `<div class="cost-summary">${t("settings.cost_summary", {
        days: String(COST_RANGE_DAYS),
        turns: String(totals.turns),
        input: fmtTokenCount(totals.input),
        hit: fmtTokenCount(totals.hit),
        write: fmtTokenCount(totals.write),
        output: fmtTokenCount(totals.output),
        rate: String(hitRate),
        cost: hasPrices ? `${totals.exact ? "" : "≥ "}¥${fmtMoney(totals.amount)}` : "—",
      })}</div>`;

  const unpricedNote = unpriced.size > 0
    ? `<div class="settings-hint cost-warn">${esc(t("settings.cost_unpriced", { models: [...unpriced].map(modelLabel).join(", ") }))}</div>`
    : "";

  const head = `<tr>${[
    "settings.cost_col_date",
    "settings.cost_col_turns",
    "settings.cost_col_input",
    "settings.cost_col_hit",
    "settings.cost_col_write",
    "settings.cost_col_output",
    "settings.cost_col_amount",
  ].map(k => `<th>${t(k)}</th>`).join("")}</tr>`;

  // 新的在上：看用量几乎总是先看最近几天
  const body = days.slice().reverse().map(d => {
    const c = dayCost(d, pricing);
    const amount = !hasPrices ? "—" : `${c.exact ? "" : "≥ "}¥${fmtMoney(c.amount)}`;
    return `<tr><td>${d.date}</td><td>${d.turns}</td><td>${fmtTokenCount(d.input)}</td>` +
      `<td>${fmtTokenCount(d.cacheRead)}</td><td>${fmtTokenCount(d.cacheCreate)}</td>` +
      `<td>${fmtTokenCount(d.output)}</td><td>${amount}</td></tr>`;
  }).join("");

  const status = hasPrices
    ? t("settings.cost_updated_at", { date: pricing?.updated_at || "—" })
    : t("settings.cost_empty");

  const pendingBlock = pending ? `
    <div class="cost-pending">
      <div class="cost-pending-title">${t("settings.cost_pending_title")}</div>
      <table class="cost-table"><tbody>${pricingPreviewRows(pricing, pending)}</tbody></table>
      <div class="settings-row">
        <button type="button" class="settings-btn" id="settings-cost-confirm">${t("settings.cost_confirm")}</button>
        <button type="button" class="settings-btn" id="settings-cost-discard">${t("settings.cost_discard")}</button>
      </div>
      <div class="settings-hint">${esc(t("settings.cost_pending_hint", { path: state?.pendingPath || "" }))}</div>
    </div>` : "";

  return `
    <div class="settings-row">
      <button type="button" class="settings-btn" id="settings-cost-open">${t("settings.cost_open")}</button>
      <button type="button" class="settings-btn" id="settings-cost-update">${t("settings.cost_update")}</button>
      <span class="settings-hint" id="settings-cost-msg" style="margin-left:8px;"></span>
    </div>
    <div class="settings-hint" title="${esc(state?.path || "")}">${esc(status)}</div>
    <div class="settings-hint">${esc(t("settings.cost_hint"))}</div>
    ${pendingBlock}
    ${summary}
    ${unpricedNote}
    ${days.length > 0 ? `<table class="cost-table"><thead>${head}</thead><tbody>${body}</tbody></table>` : ""}`;
}

/** 绑定「用量与成本」分块里的按钮。分块内部重渲染之后**必须再调一次**
 *  （旧节点已被替换，原来绑的监听器随之失效）。 */
function wireUsageCost(container: HTMLElement): void {
  const openBtn = container.querySelector("#settings-cost-open") as HTMLButtonElement | null;
  const updateBtn = container.querySelector("#settings-cost-update") as HTMLButtonElement | null;
  // 提示文字每次现查节点：重渲染会把 `#settings-cost-msg` 整个换掉
  const showMsg = (text: string) => {
    const el = container.querySelector("#settings-cost-msg") as HTMLElement | null;
    if (el) el.textContent = text;
  };
  const refresh = async () => {
    const box = container.querySelector("#settings-usage-cost") as HTMLElement | null;
    if (!box) return;
    box.innerHTML = await renderUsageCostBody();
    wireUsageCost(container);
  };

  openBtn?.addEventListener("click", async () => {
    try {
      // 缺文件时后端先落一份空骨架再返回路径；真正的「打开」交给前端 `open()`
      await open(await invoke<string>("pricing_file_path"));
    } catch (e) {
      showMsg(t("settings.cost_failed", { err: String(e) }));
    }
  });

  updateBtn?.addEventListener("click", async () => {
    try {
      // 「只读」档位下 agent 连 Write 都过不去（见 tools::write_blocked）——
      // 与其发一条注定失败的提示词，不如在这里说清楚为什么
      if ((localStorage.getItem("lunac-security-profile") || "project") === "safe") {
        showMsg(t("settings.cost_safe_mode"));
        return;
      }
      const st = await invoke<PricingState>("get_pricing_state");
      // 要让 agent 查的模型 = 有用量记录的模型 + 当前配置的模型（还没用过就也该有价格）
      const wanted = new Set<string>();
      const days = await invoke<UsageDay[]>("read_usage_range", { dates: lastNDates(COST_RANGE_DAYS) });
      for (const d of days) for (const m of d.models) wanted.add(m.model);
      try {
        const cfg = await invoke<{ model?: string }>("get_ai_config");
        if (cfg?.model) wanted.add(cfg.model);
      } catch {}
      if (wanted.size === 0) {
        // 一个模型名都报不出来（没用量记录、也没配模型）→ 发出去只会让 agent 空转
        showMsg(t("settings.cost_no_usage", { days: String(COST_RANGE_DAYS) }));
        return;
      }
      const prompt = t("settings.cost_fetch_prompt", {
        models: [...wanted].map(m => `- ${m}`).join("\n"),
        path: st.pendingPath,
      });
      // 设置面板不能自己发消息（那要动结果区状态，是主界面的职责）—— 走 main.ts 的桥
      await (window as any).__lunac_agent_task?.(prompt);
      showMsg(t("settings.cost_sent"));
    } catch (e) {
      showMsg(t("settings.cost_failed", { err: String(e) }));
    }
  });

  container.querySelector("#settings-cost-confirm")?.addEventListener("click", async () => {
    try {
      await invoke("commit_pricing_pending", { today: costDateKey(new Date()) });
      await refresh();
      showMsg(t("settings.cost_committed"));
    } catch (e) {
      showMsg(t("settings.cost_failed", { err: String(e) }));
    }
  });

  container.querySelector("#settings-cost-discard")?.addEventListener("click", async () => {
    try {
      await invoke("discard_pricing_pending");
      await refresh();
      showMsg(t("settings.cost_discarded"));
    } catch (e) {
      showMsg(t("settings.cost_failed", { err: String(e) }));
    }
  });
}

// ── Skill Store (技能扩展) ─────────────────────────────────────

/** 推荐的国内可直连技能/智能体平台入口 */
const SKILL_SITES: { name: string; desc: string; url: string }[] = [
  { name: "Coze 扣子", desc: "字节跳动 AI 智能体 / 技能平台", url: "https://www.coze.cn/" },
  { name: "智谱清言智能体广场", desc: "智谱 AI 智能体 / 技能市场", url: "https://bigmodel.cn/marketplace/index/agent" },
  { name: "百度文心智能体平台", desc: "百度智能体创作与发布平台", url: "https://agents.baidu.com/" },
  { name: "腾讯元器", desc: "腾讯智能体创作平台", url: "https://yuanqi.tencent.com/" },
  { name: "Coze 技能页示例", desc: "xiaping.coze.com — Coze 技能页面", url: "http://xiaping.coze.com/" },
];

function loadSkillSites(): { name: string; url: string }[] {
  try {
    const raw = localStorage.getItem("lunac-skill-sites");
    const arr = raw ? JSON.parse(raw) : [];
    return Array.isArray(arr) ? arr.filter((s: any) => s && s.url) : [];
  } catch { return []; }
}

function saveSkillSites(list: { name: string; url: string }[]) {
  try { localStorage.setItem("lunac-skill-sites", JSON.stringify(list)); } catch {}
}

function skillEntryHtml(name: string, desc: string, url: string, removable: boolean): string {
  return `
    <div class="settings-marketplace-item">
      <div class="settings-marketplace-info">
        <span class="settings-marketplace-name">${esc(name)}</span>
        <span class="settings-marketplace-desc">${esc(desc)}</span>
      </div>
      <div style="display:flex;gap:6px;flex-shrink:0;">
        <button class="settings-skill-open" data-url="${esc(url)}">${t("settings.skill_open")}</button>
        ${removable ? `<button class="settings-skill-del" data-url="${esc(url)}" data-name="${esc(name)}">${t("settings.skill_remove")}</button>` : ""}
      </div>
    </div>`;
}

/** 新建技能编辑器的默认 SKILL.md 模板。 */
const SKILL_TEMPLATE = `---
name: my-skill
description: Describe what this skill does and when it should be used.
---

Write the concrete instructions/steps of this skill here.
`;

/** 已安装技能行 HTML（buildSkillsPane 初次渲染与操作后重渲染共用）。 */
function installedSkillRowHtml(s: { name: string; description: string; dir: string; key: string }): string {
  return `
    <div class="settings-marketplace-item">
      <div class="settings-marketplace-info">
        <span class="settings-marketplace-name">${esc(s.name)}</span>
        <span class="settings-marketplace-desc">${esc(s.description || s.dir)}</span>
      </div>
      <div style="display:flex;gap:6px;flex-shrink:0;">
        <button class="settings-skill-open" data-dir="${esc(s.dir)}" title="${esc(s.dir)}">${t("settings.skill_open")}</button>
        <button class="settings-skill-edit" data-key="${esc(s.key)}">${t("settings.edit")}</button>
        <button class="settings-skill-del-installed" data-key="${esc(s.key)}">${t("settings.skill_remove")}</button>
      </div>
    </div>`;
}

/** 技能（Skill Store）分块的内部内容 —— 组装进 AI 分类（见 buildAIPane）。
 *  2026-09-19 批 9：不再是独立侧栏分类，块内的次级标题仍用
 *  .settings-marketplace-title（与分块标题 .settings-group-title 区分层级）。 */
async function buildSkillsSection(): Promise<string> {
  const recommendedHtml = SKILL_SITES.map(r => skillEntryHtml(r.name, r.desc, r.url, false)).join("");
  const custom = loadSkillSites();
  const customHtml = custom.length === 0
    ? `<div class="settings-plugin-empty">${t("settings.skill_empty")}</div>`
    : custom.map(s => skillEntryHtml(s.name, s.url, s.url, true)).join("");

  // ── 已安装技能（Lunac 数据根 <exe 所在目录>\skills）—— 具体 skill 的前端体现 ──
  let installedHtml = "";
  try {
    const skills: Array<{ name: string; description: string; dir: string; key: string }> =
      await invoke("list_installed_skills");
    installedHtml = skills.length === 0
      ? `<div class="settings-plugin-empty">${t("settings.skills_none")}</div>`
      : skills.map(s => installedSkillRowHtml(s)).join("");
  } catch {
    installedHtml = `<div class="settings-plugin-empty">${t("settings.load_error")}</div>`;
  }

  return `
      <div class="settings-marketplace-section">
        <div class="settings-marketplace-title">${t("settings.skills_installed")}</div>
        <div class="settings-hint" style="font-size:0.7rem;color:var(--text-dim);margin:2px 0 6px;">${t("settings.skills_installed_hint")}</div>
        <div class="settings-skill-list" id="settings-skill-installed">${installedHtml}</div>
      </div>
      <div class="settings-marketplace-section">
        <div class="settings-marketplace-title">${t("settings.skill_install")}</div>
        <div class="settings-marketplace-row" style="flex-wrap:wrap;">
          <input type="text" id="settings-skill-install-url" class="settings-input" autocomplete="off" placeholder="${t("settings.skill_install_url_ph")}" style="flex:1 1 180px;max-width:none;">
          <button id="settings-skill-install-url-btn" class="settings-install-btn">${t("settings.skill_install_url")}</button>
          <button id="settings-skill-new-btn" class="settings-install-btn">${t("settings.skill_new")}</button>
        </div>
        <div id="settings-skill-op-msg" style="font-size:0.7rem;color:var(--text-dim);margin-top:4px;display:none;"></div>
        <div id="settings-skill-editor" style="display:none;margin-top:6px;">
          <textarea id="settings-skill-editor-text" class="settings-input" spellcheck="false" style="flex:none;width:100%;max-width:100%;height:170px;font-family:ui-monospace,Consolas,monospace;font-size:0.68rem;resize:vertical;line-height:1.4;"></textarea>
          <div style="display:flex;gap:6px;justify-content:flex-end;margin-top:6px;">
            <button id="settings-skill-editor-save" class="settings-install-btn">${t("settings.save")}</button>
            <button id="settings-skill-editor-cancel" class="settings-btn">${t("settings.cancel")}</button>
          </div>
        </div>
      </div>
      <div class="settings-marketplace-section">
        <div class="settings-marketplace-title">${t("settings.skill_add_url")}</div>
        <div class="settings-marketplace-row">
          <input type="text" id="settings-skill-url" class="settings-input" autocomplete="off" placeholder="https://xxx.coze.com/" style="max-width:100%;">
          <button id="settings-skill-add" class="settings-install-btn">${t("settings.skill_add")}</button>
        </div>
        <div id="settings-skill-msg" style="font-size:0.7rem;color:var(--text-dim);margin-top:4px;display:none;"></div>
      </div>
      <div class="settings-marketplace-section">
        <div class="settings-marketplace-title">${t("settings.skill_recommended")}</div>
        ${recommendedHtml}
      </div>
      <div class="settings-marketplace-section">
        <div class="settings-marketplace-title">${t("settings.skill_custom")}</div>
        <div class="settings-skill-list" id="settings-skill-custom">${customHtml}</div>
      </div>`;
}

/** 工具（Tools / MCP）分块的内部内容 —— 组装进 AI 分类（见 buildAIPane）。
 *  2026-09-19 批 9：此前它被挂在「插件」分类下，用户指出「这个应该是 tools」
 *  —— 它是 AI Agent 的自定义工具（MCP 桥），与 Lunac 插件是两回事。 */
async function buildToolsSection(): Promise<string> {
  let toolsHtml = "";
  let communityHtml = "";
  try {
    const tools: Array<{ filename: string; name: string; description: string; valid: boolean }> =
      await invoke("list_tool_files");
    if (tools.length === 0) {
      toolsHtml = `<div class="settings-plugin-empty">${t("settings.no_tools")}</div>`;
    } else {
      for (const t of tools) {
        const badge = t.valid
          ? '<span class="tool-badge valid">✓</span>'
          : '<span class="tool-badge invalid">✗</span>';
        toolsHtml += `
          <div class="settings-plugin-item">
            <div class="settings-plugin-info">
              <span class="settings-plugin-name">${esc(t.name || t.filename)} ${badge}</span>
              <span class="settings-plugin-desc">${esc(t.description || t.filename)}</span>
            </div>
          </div>`;
      }
    }

    // ── Community preset tools (inside try so 'tools' is in scope) ──
    const communityTools = [
      { name: "Get Weather", desc: "Query wttr.in for city weather", cmd: "curl -s \"wttr.in/{{city}}?format=3\"" },
      { name: "Web Search", desc: "Search the web via Google", cmd: "curl -s \"https://www.google.com/search?q={{query}}\"" },
      { name: "System Info", desc: "Get Windows system information", cmd: "systeminfo | findstr /B /C:\"OS Name\" /C:\"Total Physical Memory\" /C:\"System Type\"" },
      { name: "List Files", desc: "List files in a directory", cmd: "dir \"{{path}}\" /b"},
    ];

    const installedNames = new Set(tools.map(t => t.filename));
    communityHtml = `<div class="settings-marketplace-title">${t("settings.community_plugins")}</div>`;
    for (const ct of communityTools) {
      const expectedFn = ct.name.replace(/\s/g, "-").toLowerCase() + ".json";
      const installed = installedNames.has(expectedFn);
      communityHtml += `
        <div class="settings-marketplace-item">
          <div class="settings-marketplace-info">
            <span class="settings-marketplace-name">${esc(ct.name)}</span>
            <span class="settings-marketplace-desc">${esc(ct.desc)}</span>
          </div>
          <button class="settings-install-btn${installed ? ' installed' : ''}"
            data-name="${esc(ct.name.replace(/\s/g, '-').toLowerCase())}"
            data-cmd="${esc(ct.cmd)}" data-desc="${esc(ct.desc)}"
            ${installed ? 'disabled' : ''}>${installed ? t("settings.installed_btn") : t("settings.install")}</button>
        </div>`;
    }
  } catch {
    toolsHtml = `<span class="settings-plugin-empty">${t("settings.load_error")}</span>`;
    communityHtml = `<span class="settings-plugin-empty">${t("settings.marketplace_error")}</span>`;
  }

  return `
      <!-- Install from URL -->
      <div class="settings-marketplace-section">
        <div class="settings-marketplace-title">${t("settings.install_from_url")}</div>
        <div class="settings-marketplace-row">
          <input type="text" id="settings-tool-url" class="settings-input" autocomplete="off" placeholder="https://example.com/tool.json" style="max-width:100%;">
          <button id="settings-install-url" class="settings-install-btn">${t("settings.install")}</button>
        </div>
        <div id="settings-install-msg" style="font-size:0.7rem;color:var(--text-dim);margin-top:4px;display:none;"></div>
      </div>

      <!-- Installed tools -->
      <div class="settings-marketplace-section">
        <div class="settings-marketplace-title">${t("settings.installed")}</div>
        <div class="settings-plugin-list">${toolsHtml}</div>
      </div>

      <!-- Community tools -->
      <div class="settings-marketplace-section">
        ${communityHtml}
      </div>

      <div class="settings-pane-footer">
        <button id="settings-open-tools" class="settings-tool-btn">${t("settings.open_tool_editor")}</button>
      </div>`;
}

/** 「插件」分类 = **插件市场总览**（2026-09-19 批 9，用户明确要求；2026-09-21 L1 起可装可卸）。
 *
 *  **与 AI 分类下的 tools 严格区分**：tools 是「AI Agent 能调用的自定义工具
 *  （MCP 桥）」，这里列的是 **Lunac 自己的插件**（结果区里能搜到、点开的那些）。
 *  此前两者混在同一个分类里，分类名还叫「插件 (MCP 工具)」—— 用户报的正是这里。
 *
 *  总览（上半）：图标 + 本地化名称 + 本地化描述 + 「打开」；
 *  **搜索关键词进 title 属性**（悬停可见），不铺在界面上 —— 关键词数组里
 *  中英混杂且动辄十几个，平铺会把面板糊成一片。
 *
 *  第三方插件（下半，L1）：数据源是**插件目录扫描**而不是 registry —— 坏包（清单坏了、
 *  入口丢了）根本没进 registry，但**必须在这里可见**，否则用户只会看到插件莫名消失、
 *  手上没有任何线索。每行一个两段式确认的「卸载」。
 */
async function buildPluginsPane(): Promise<string> {
  const rows = pluginRegistry.getAll().map(pluginRowHtml).join("");
  const { list, dir, error } = await readPluginMarket();
  const dirRows = error
    ? `<div class="settings-plugin-empty">${esc(t("settings.plugins_market_failed", { err: error }))}</div>`
    : list.length === 0
      ? `<div class="settings-plugin-empty">${t("settings.plugins_market_empty")}</div>`
      : list.map(marketRowHtml).join("");

  return `
    <div class="settings-pane" data-pane="plugins" id="sp-plugins">
      <div class="settings-pane-title">${t("settings.plugins")}</div>
      <div class="settings-group-title">${t("settings.plugins_installed")}</div>
      <div class="settings-plugin-list" id="settings-plugin-overview">${rows}</div>
      <div class="settings-hint" style="font-size:0.7rem;color:var(--text-dim);line-height:1.5;margin:10px 0 0;">${t("settings.plugins_hint")}</div>
      <div class="settings-group-title">${t("settings.plugins_market")}</div>
      <div class="settings-market-install">
        <input type="text" id="settings-plugin-url" class="settings-input" spellcheck="false"
          placeholder="${esc(t("settings.plugins_market_url"))}" />
        <button type="button" class="settings-btn" id="settings-plugin-install">${t("settings.plugins_market_install")}</button>
      </div>
      <div class="settings-plugin-list" id="settings-plugin-dir">${dirRows}</div>
      <div class="settings-hint" id="settings-plugin-msg"></div>
      <div class="settings-hint">${esc(t("settings.plugins_market_hint"))}</div>
      <div class="settings-hint" title="${esc(dir)}">${esc(t("settings.plugins_market_dir", { path: dir }))}</div>
    </div>`;
}

/** 插件总览的一行（内置与第三方都走这里 —— 它们注册后是同一份 registry）。 */
function pluginRowHtml(p: Plugin): string {
  const icon = (window as any).__lunac_plugin_icon?.(p.id) || p.icon || "";
  const kw = p.keywords.join(" · ");
  return `
      <div class="settings-plugin-item" title="${esc(kw)}">
        <div class="settings-plugin-icon">${icon}</div>
        <div class="settings-plugin-info">
          <span class="settings-plugin-name">${esc(pluginName(p.id, p.name))}</span>
          <span class="settings-plugin-desc">${esc(pluginDesc(p.id, p.description))}</span>
        </div>
        <button class="settings-install-btn" data-open-plugin="${esc(p.id)}">${t("settings.skill_open")}</button>
      </div>`;
}

/** 插件目录里的一行：名称 + 版本 + 来源 + 卸载（坏包多一行原因）。 */
function marketRowHtml(p: MarketPluginInfo): string {
  const name = p.version ? `${p.name} · v${p.version}` : p.name;
  const src = p.homepage
    ? `<span class="settings-market-src" title="${esc(p.homepage)}">${esc(p.homepage)}</span>`
    : "";
  const broken = p.valid
    ? ""
    : `<span class="settings-market-broken">${esc(t("settings.plugins_market_broken", { err: p.error }))}</span>`;
  return `
      <div class="settings-market-row">
        <div class="settings-plugin-info">
          <span class="settings-plugin-name">${esc(name)}</span>
          ${src}
          ${broken}
        </div>
        <button class="settings-skill-del-installed" data-uninstall-plugin="${esc(p.id)}">${t("settings.plugins_market_uninstall")}</button>
      </div>`;
}

/** 读插件目录：扫描结果 + 目录绝对路径；失败时把原因**原样**带回界面（不猜、不吞）。 */
async function readPluginMarket(): Promise<{ list: MarketPluginInfo[]; dir: string; error: string }> {
  try {
    const [list, dir] = await Promise.all([
      invoke<MarketPluginInfo[]>("list_installed_plugins"),
      invoke<string>("plugins_dir_path"),
    ]);
    return { list, dir, error: "" };
  } catch (e) {
    return { list: [], dir: "", error: String(e) };
  }
}

/** 重新画「已安装插件」总览（装完 / 卸完要立刻反映出来 —— 那是用户唯一能确认「真装上了」的地方）。 */
function renderPluginOverview(container: HTMLElement) {
  const el = container.querySelector<HTMLElement>("#settings-plugin-overview");
  if (!el) return;
  el.innerHTML = pluginRegistry.getAll().map(pluginRowHtml).join("");
  bindOpenPluginButtons(el);
}

/** 总览每行的「打开」走 main.ts 的 `__lunac_open_plugin` 桥：设置面板不自己 executePlugin
 *  （那要动结果区 / 搜索栏状态，属于主界面的职责）。重绘后必须重新绑 —— 新节点没有监听。 */
function bindOpenPluginButtons(root: HTMLElement) {
  root.querySelectorAll<HTMLElement>("[data-open-plugin]").forEach(btn => {
    btn.addEventListener("click", () => {
      const id = btn.dataset.openPlugin;
      if (id) (window as any).__lunac_open_plugin?.(id);
    });
  });
}

/** 插件目录那一段的绑定（L1）：安装 / 卸载 / 重绘。
 *
 *  装完**不必重启 AI 也不必刷前端** —— 第三方插件是前端的东西（与「技能改完要重启 agent」
 *  是两回事），`refreshMarketPlugins()` 重注册一次就够了。 */
function wirePluginMarket(container: HTMLElement) {
  const listEl = container.querySelector<HTMLElement>("#settings-plugin-dir");
  const urlInput = container.querySelector<HTMLInputElement>("#settings-plugin-url");
  const installBtn = container.querySelector<HTMLButtonElement>("#settings-plugin-install");
  if (!listEl || !installBtn) return;

  // 提示行**每次现查节点**：容器整体重渲染会把 #settings-plugin-msg 整个换掉，缓存引用会写进空气里
  const showMsg = (text: string, color: string) => {
    const el = container.querySelector<HTMLElement>("#settings-plugin-msg");
    if (!el) return;
    el.textContent = text;
    el.style.color = color;
  };

  const render = async () => {
    const { list, error } = await readPluginMarket();
    listEl.innerHTML = error
      ? `<div class="settings-plugin-empty">${esc(t("settings.plugins_market_failed", { err: error }))}</div>`
      : list.length === 0
        ? `<div class="settings-plugin-empty">${t("settings.plugins_market_empty")}</div>`
        : list.map(marketRowHtml).join("");
    listEl.querySelectorAll<HTMLButtonElement>("[data-uninstall-plugin]").forEach(btn => {
      btn.addEventListener("click", async () => {
        const id = btn.dataset.uninstallPlugin || "";
        // 两段式确认（WebView2 下原生 confirm 不可靠，同技能删除的处置）
        if (btn.dataset.armed !== "1") {
          btn.dataset.armed = "1";
          btn.textContent = t("settings.plugins_market_uninstall_confirm");
          setTimeout(() => {
            btn.dataset.armed = "";
            btn.textContent = t("settings.plugins_market_uninstall");
          }, 3000);
          return;
        }
        try {
          await invoke("uninstall_plugin", { id });
          await refreshMarketPlugins();
          renderPluginOverview(container);
          await render();
          showMsg(t("settings.plugins_market_uninstalled", { id }), "var(--yellow)");
        } catch (e: any) {
          showMsg(String(e), "var(--red)");
        }
      });
    });
  };

  const install = async () => {
    const url = (urlInput?.value || "").trim();
    if (!url) {
      showMsg(t("settings.enter_url"), "var(--yellow)");
      return;
    }
    installBtn.disabled = true;
    showMsg(t("settings.plugins_market_installing"), "var(--text-dim)");
    try {
      const id = await invoke<string>("install_plugin_from_url", { url });
      if (urlInput) urlInput.value = "";
      await refreshMarketPlugins();
      renderPluginOverview(container);
      await render();
      showMsg(t("settings.plugins_market_installed_ok", { id }), "var(--green)");
    } catch (e: any) {
      showMsg(t("settings.plugins_market_install_failed", { err: String(e) }), "var(--red)");
    } finally {
      installBtn.disabled = false;
    }
  };
  installBtn.addEventListener("click", install);
  urlInput?.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      install();
    }
  });
}

// ── Listener attachment ──────────────────────────────────────────

export async function attachSettingsListeners(container: HTMLElement) {
  console.log("[lunac settings] attachSettingsListeners called, container:", container.id, "children:", container.children.length);
  const styleId = "settings-sidebar-styles";
  if (!document.getElementById(styleId)) {
    const style = document.createElement("style");
    style.id = styleId;
    style.textContent = `
      /* Layout: sidebar + content */
      .settings-layout {
        display: flex;
        height: 100%;
        min-height: 240px;
        overflow: hidden;
      }
      .settings-sidebar {
        width: 30%;
        min-width: 100px;
        border-right: 1px solid var(--border-glass);
        background: rgba(var(--ink-rgb), 0.02);
        display: flex;
        flex-direction: column;
        flex-shrink: 0;
      }
      .settings-sidebar-item {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 10px 12px 10px 0;
        font-size: 0.78rem;
        color: var(--text-dim);
        cursor: pointer;
        transition: background 0.1s, color 0.1s;
        border-left: 3px solid transparent;
        user-select: none;
      }
      .settings-sidebar-item:hover {
        background: rgba(var(--ink-rgb), 0.04);
        color: var(--text);
      }
      .settings-sidebar-item.active {
        color: var(--text);
        background: var(--accent-bg);
        border-left-color: var(--accent);
      }

      .settings-content {
        flex: 1;
        overflow: hidden;
        padding: 4px 0;
      }
      /* 打开下拉时放开**设置面板自身**对下拉框的裁剪。
         只放开这两个容器（它们不是滚动容器）；下拉框在滚动容器的可视区内
         翻转 + 限高（见 positionDropdown），所以不需要动滚动容器。
         **绝不要**给 #results-list / #results-container 加这个类 —— 那是滚动
         容器（styles.css 里 overflow-y:auto），改成 visible 会让它不再是滚动
         容器、scrollTop 归零，整页瞬间跳回顶部（下拉框跳顶事故的根因）。 */
      .settings-layout.sel-open {
        overflow: visible;
      }
      .settings-content.sel-open {
        overflow: visible;
      }

      /* Panes */
      .settings-pane {
        display: none;
        padding: 0 14px;
      }
      .settings-pane.active {
        display: block;
      }
      .settings-pane-title {
        font-size: 0.8rem;
        font-weight: 600;
        color: var(--text);
        margin-bottom: 10px;
        padding-bottom: 6px;
        border-bottom: 1px solid var(--border-glass);
      }
      .settings-pane-footer {
        display: flex;
        justify-content: flex-end;
        padding-top: 8px;
      }

      /* Rows */
      .settings-row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 6px 0;
        gap: 10px;
      }
      .settings-bg-actions {
        display: flex;
        gap: 6px;
        flex-shrink: 0;
      }
      /* 设置项下方的一行小字说明（如自启机制）—— 不占开关位，左对齐 */
      .settings-hint {
        padding: 0 0 6px;
        font-size: 0.68rem;
        line-height: 1.45;
        color: var(--text-dim);
        opacity: 0.85;
      }
      .settings-label {
        font-size: 0.76rem;
        color: var(--text-dim);
        flex-shrink: 0;
      }
      .settings-input {
        flex: 1;
        max-width: 220px;
        height: 26px;
        box-sizing: border-box;
        padding: 5px 8px;
        font-size: 0.73rem;
        /* ② 文本框 —— 凹陷层走 --ctx-shade-rgb / --ctx-shade-scale：
           底色暗 ⇒ 白洗（比底色亮）、底色亮 ⇒ 黑洗（比底色暗）。ON 时它就是原来的
           rgba(0, 0, 0, 0.25 * shade)。 */
        background: rgba(var(--ctx-shade-rgb), calc(0.25 * var(--ctx-shade-scale)));
        border: 1px solid var(--ctx-border-glass);
        border-radius: 6px;
        color: var(--text);
        outline: none;
        caret-color: var(--accent);
      }
      .settings-input:focus { border-color: var(--accent-border); }
      /* ── 人格 / 自定义提示词（L2）────────────────────────────────
         一块多行文本框：默认 96px 高、可纵向拉伸（内容多时用户自己拉），
         内部滚动交给全局那条 4px 滚动条（styles.css 唯一真相源，禁止按容器单写）。 */
      .settings-persona-text {
        width: 100%;
        box-sizing: border-box;
        min-height: 96px;
        max-height: 260px;
        resize: vertical;
        padding: 7px 9px;
        font: inherit;
        font-size: 0.73rem;
        line-height: 1.55;
        /* 凹陷层与 .settings-input 同源：底色暗 ⇒ 白洗、亮 ⇒ 黑洗 */
        background: rgba(var(--ctx-shade-rgb), calc(0.25 * var(--ctx-shade-scale)));
        border: 1px solid var(--ctx-border-glass);
        border-radius: 6px;
        color: var(--text);
        outline: none;
        caret-color: var(--accent);
      }
      .settings-persona-text:focus { border-color: var(--accent-border); }
      .settings-persona-text::placeholder { color: var(--text-dim); opacity: 0.6; }
      /* ── 插件市场（L1，2026-09-21）─────────────────────────────────
         目录里的一行：名称 + 来源 + 卸载（坏包多一行原因）。
         刻意与 .settings-plugin-item 分成两个类：那个是 registry 总览（行数被
         agent-ui-spec §8 的回归清单钉着），而这里可能含坏包，混进同一个计数会
         让「总览行数 == 插件数」这条断言变成假绿。 */
      .settings-market-install {
        display: flex;
        align-items: center;
        gap: 8px;
        margin: 6px 0;
      }
      .settings-market-install .settings-input { flex: 1; min-width: 0; }
      .settings-market-row {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 6px 8px;
        border-radius: 6px;
        transition: background 0.1s;
      }
      .settings-market-row:hover { background: rgba(var(--ink-rgb), 0.04); }
      .settings-market-src {
        color: var(--text-dim);
        font-size: 0.68rem;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
      }
      .settings-market-broken { color: var(--yellow); font-size: 0.68rem; }
      .settings-btn {
        flex-shrink: 0;
        padding: 5px 10px;
        font-size: 0.7rem;
        border-radius: 6px;
        border: 1px solid var(--border-glass);
        background: rgba(var(--ink-rgb), 0.06);
        color: var(--text-dim);
        cursor: pointer;
        transition: background 0.1s, color 0.1s;
      }
      .settings-btn:hover {
        background: rgba(var(--ink-rgb), 0.12);
        color: var(--text);
      }
      /* ── 用量与成本（A12）────────────────────────────────────────
         数字一律右对齐、用等宽字体：金额与 token 是拿来逐行比对的，
         比例字体下位数对不齐就看不出「哪天异常」。 */
      .cost-summary {
        padding: 2px 0 8px;
        font-size: 0.72rem;
        line-height: 1.5;
        color: var(--text);
      }
      .cost-table {
        width: 100%;
        border-collapse: collapse;
        font-size: 0.7rem;
        font-variant-numeric: tabular-nums;
      }
      .cost-table th {
        text-align: right;
        font-weight: 400;
        padding: 3px 4px;
        color: var(--text-dim);
        border-bottom: 1px solid var(--border-glass);
      }
      .cost-table td {
        text-align: right;
        padding: 3px 4px;
        color: var(--text-dim);
      }
      .cost-table th:first-child,
      .cost-table td:first-child {
        text-align: left;
        color: var(--text);
      }
      /* 待确认的候选价格：与正式价格**在视觉上分开**，避免用户以为已经写进去了 */
      .cost-pending {
        margin: 4px 0 10px;
        padding: 8px 10px;
        border: 1px solid var(--accent-border);
        border-radius: 8px;
        background: var(--accent-bg);
      }
      .cost-pending-title {
        font-size: 0.75rem;
        font-weight: 600;
        color: var(--text);
        margin-bottom: 4px;
      }
      .cost-src {
        font-size: 0.62rem;
        color: var(--text-dim);
        opacity: 0.75;
        word-break: break-all;
      }
      .cost-new { color: var(--accent); font-size: 0.62rem; }
      .cost-warn { color: var(--yellow); }
      .settings-select {
        flex: 1;
        max-width: 220px;
        height: 26px;
        box-sizing: border-box;
        padding: 5px 8px;
        font-size: 0.73rem;
        background: rgba(0, 0, 0, calc(0.25 * var(--shade-scale)));
        border: 1px solid var(--border-glass);
        border-radius: 6px;
        color: var(--text);
        outline: none;
      }
      .settings-hotkey {
        padding: 5px 12px;
        font-size: 0.73rem;
        border-radius: 6px;
        border: 1px solid var(--accent-border);
        background: var(--accent-bg);
        color: var(--text);
        cursor: pointer;
        min-width: 90px;
        text-align: center;
        transition: background 0.1s;
      }
      .settings-hotkey:hover { background: rgba(var(--accent-rgb), 0.2); }
      .settings-hotkey.recording {
        border-color: var(--yellow);
        background: rgba(201,184,150,0.15);
        color: var(--yellow);
        animation: pulse-recording 0.8s infinite alternate;
      }
      @keyframes pulse-recording {
        from { opacity: 1; } to { opacity: 0.5; }
      }
      .settings-toggle {
        position: relative;
        display: inline-block;
        width: 36px;
        height: 20px;
        flex-shrink: 0;
      }
      .settings-toggle input { display: none; }
      /* ⑤ 切换开关（项目内所有开关都吃这一条）—— 轨道与描边走「按钮自定义」。
         轨道底的 alpha 取线条 alpha 的固定比例（关 0.31 / 开 0.44）：这样拖
         「线条透明度」时开关的三层（描边 / 开启底 / 关闭底）同步缩放，不会只剩描边动。
         默认 alpha 0.32 ⇒ 关闭底 0.0992≈改造前的 0.1、开启底 0.1408≈0.14，肉眼无差。
         圆点**不动**：它是填充不是线条，仍跟随主题色（用户 2026-09-20：只管线条/描边）。 */
      .settings-toggle-slider {
        position: absolute;
        cursor: pointer;
        top: 0; left: 0; right: 0; bottom: 0;
        background: rgba(var(--btn-line-rgb), calc(var(--btn-line-alpha) * 0.31));
        border-radius: 20px;
        transition: background 0.2s;
      }
      .settings-toggle-slider::before {
        content: "";
        position: absolute;
        height: 14px; width: 14px;
        left: 3px; bottom: 3px;
        background: var(--text-dim);
        border-radius: 50%;
        transition: transform 0.2s, background 0.2s;
      }
      .settings-toggle input:checked + .settings-toggle-slider {
        background: rgba(var(--btn-line-rgb), calc(var(--btn-line-alpha) * 0.44));
        border: 1px solid rgba(var(--btn-line-rgb), var(--btn-line-alpha));
      }
      .settings-toggle input:checked + .settings-toggle-slider::before {
        transform: translateX(16px);
        background: var(--accent);
      }
      .settings-plugin-list {
        max-height: 200px;
        overflow-y: auto;
      }
      .settings-plugin-item {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 6px 8px;
        border-radius: 6px;
        transition: background 0.1s;
      }
      .settings-plugin-item:hover { background: rgba(var(--ink-rgb), 0.04); }
      .settings-plugin-icon {
        flex-shrink: 0;
        display: flex;
        align-items: center;
        justify-content: center;
        width: 20px;
        height: 20px;
        color: var(--text-dim);
      }
      .settings-plugin-icon .result-item-icon-img { width: 18px; height: 18px; }
      .settings-plugin-info { display: flex; flex-direction: column; min-width: 0; flex: 1; }
      .settings-plugin-name { font-size: 0.76rem; color: var(--text); display: flex; align-items: center; gap: 6px; }
      .settings-plugin-desc { font-size: 0.68rem; color: var(--text-dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .settings-plugin-empty { color: var(--text-dim); font-size: 0.75rem; padding: 8px 0; }
      .tool-badge { font-size: 0.58rem; padding: 1px 5px; border-radius: 4px; font-weight: 600; }
      .tool-badge.valid { background: rgba(157,180,172,0.15); color: var(--green); }
      .tool-badge.invalid { background: rgba(192,138,138,0.15); color: var(--red); }
      .settings-tool-btn {
        padding: 5px 12px;
        font-size: 0.72rem;
        border-radius: 6px;
        border: 1px solid var(--accent-border);
        background: var(--accent-bg);
        color: var(--text);
        cursor: pointer;
        transition: background 0.1s;
      }
      .settings-tool-btn:hover { background: rgba(var(--accent-rgb), 0.2); }
      .settings-marketplace-section { margin-bottom: 14px; }
      .settings-marketplace-title {
        font-size: 0.7rem;
        font-weight: 600;
        color: var(--text-dim);
        text-transform: uppercase;
        letter-spacing: 0.5px;
        margin-bottom: 6px;
      }
      .settings-marketplace-row {
        display: flex;
        gap: 6px;
        align-items: center;
      }
      .settings-marketplace-item {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 6px 8px;
        border-radius: 6px;
        transition: background 0.1s;
      }
      .settings-marketplace-item:hover { background: rgba(var(--ink-rgb), 0.04); }
      .settings-marketplace-info { display: flex; flex-direction: column; min-width: 0; }
      .settings-marketplace-name { font-size: 0.76rem; color: var(--text); }
      .settings-marketplace-desc { font-size: 0.68rem; color: var(--text-dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .settings-install-btn {
        padding: 3px 10px;
        font-size: 0.68rem;
        border-radius: 4px;
        border: 1px solid var(--accent-border);
        background: rgba(var(--accent-rgb), 0.1);
        color: var(--text);
        cursor: pointer;
        flex-shrink: 0;
        transition: background 0.1s;
      }
      .settings-install-btn:hover { background: rgba(var(--accent-rgb), 0.2); }
      .settings-install-btn.installed {
        border-color: var(--text-muted);
        background: rgba(var(--ink-rgb), 0.05);
        color: var(--text-dim);
        cursor: default;
      }
      /* 保存按钮：**动作按钮一律跟随主题色**（accent 三态 + 半透明档），
         不跟随主题色的是「语义状态色」—— --green(成功) / --red(危险) /
         --yellow(警告) 保持固定（见下方 .settings-save-msg / .tool-badge）。
         用户报的「搜索分类的 save 按钮没跟随主题颜色」根因就在这里：
         它此前用 var(--green) + rgba(157,180,172,*) 硬编码，与主题色无关。
         注意：本文件整段样式是模板字符串，注释里**不许出现反引号**。 */
      .settings-save-btn {
        padding: 6px 18px;
        font-size: 0.74rem;
        border-radius: 6px;
        border: 1px solid var(--accent-border);
        background: rgba(var(--accent-rgb), 0.12);
        color: var(--text);
        cursor: pointer;
        transition: background 0.15s, color 0.15s;
      }
      .settings-save-btn:hover { background: rgba(var(--accent-rgb), 0.25); }
      .settings-save-btn:active { background: rgba(var(--accent-rgb), 0.35); }
      /* 保存结果提示**保留语义绿**：它表达的是「操作成功」这一状态，不是动作按钮。 */
      .settings-save-msg {
        font-size: 0.72rem;
        color: var(--green);
        margin-left: 8px;
        opacity: 0;
        transition: opacity 0.2s;
      }
      .settings-save-msg.show { opacity: 1; }

      /* Custom dropdown (replaces native <select> — broken in transparent WebView2 windows) */
      .custom-select {
        position: relative;
        flex: 1;
        max-width: 220px;
        height: 26px;
        box-sizing: border-box;
        display: flex;
        align-items: center;
        user-select: none;
      }
      .custom-select-trigger {
        width: 100%;
        height: 26px;
        box-sizing: border-box;
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 6px;
        padding: 5px 8px;
        font-size: 0.73rem;
        font-family: inherit;
        /* ② 文本框（下拉的收合面）—— 与 .settings-input 同一条凹陷层口径 */
        background: rgba(var(--ctx-shade-rgb), calc(0.25 * var(--ctx-shade-scale)));
        border: 1px solid var(--ctx-border-glass);
        border-radius: 6px;
        color: var(--text);
        cursor: pointer;
        outline: none;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      .custom-select-trigger:hover { border-color: var(--accent-border); }
      .custom-select-arrow {
        flex-shrink: 0;
        width: 10px;
        height: 10px;
        transition: transform 0.15s;
      }
      .custom-select.open .custom-select-arrow { transform: rotate(180deg); }
      .custom-select-dropdown {
        position: absolute;
        top: 100%;
        left: 0;
        right: 0;
        max-height: 240px;
        overflow-y: auto;
        overscroll-behavior: contain; /* 滚轮不穿透到外层列表 */
        background: var(--surface-glass);
        backdrop-filter: blur(20px);
        -webkit-backdrop-filter: blur(20px);
        border: 1px solid var(--border-glass);
        border-radius: 6px;
        z-index: 999;
        display: none;
        margin-top: 0;
        box-shadow: 0 4px 16px rgba(0, 0, 0, calc(0.4 * var(--shade-scale)));
        /* 滚动条外观走全局统一细条（styles.css 的「统一细滚动条」一节）。
           这里**不要**再写 scrollbar-width / scrollbar-color —— 在 Chromium
           (WebView2) 里它们会让 ::-webkit-scrollbar 整段失效、退回系统默认样式。 */
      }
      .custom-select.open .custom-select-dropdown { display: block; }
      .custom-select-option {
        padding: 6px 10px;
        font-size: 0.73rem;
        color: var(--text-dim);
        cursor: pointer;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      .custom-select-option:hover { background: var(--accent-bg); color: var(--text); }
      .custom-select-option.selected { color: var(--text); font-weight: 600; }
      .custom-model-input {
        width: 100%;
        box-sizing: border-box;
        padding: 6px 10px;
        font-size: 0.73rem;
        color: var(--text);
        background: rgba(0, 0, 0, calc(0.35 * var(--shade-scale)));
        border: 1px solid var(--accent-border);
        border-radius: 4px;
        outline: none;
        caret-color: var(--accent);
      }
      .custom-model-edit {
        display: flex;
        gap: 4px;
        padding: 4px;
        align-items: center;
      }
      .custom-model-edit .custom-model-input { flex: 1; }
      .custom-model-ok {
        flex-shrink: 0;
        width: 26px;
        height: 26px;
        font-size: 0.85rem;
        line-height: 1;
        color: var(--text);
        background: rgba(var(--accent-rgb), 0.18);
        border: 1px solid var(--accent-border);
        border-radius: 4px;
        cursor: pointer;
      }
      .custom-model-ok:hover { background: rgba(var(--accent-rgb), 0.35); }
      .cs-del {
        display: inline-block;
        margin-left: 6px;
        padding: 0 3px;
        font-size: 0.7rem;
        color: var(--text-dim);
        border-radius: 3px;
        cursor: pointer;
        opacity: 0.6;
      }
      .custom-select-option:hover .cs-del { opacity: 1; color: var(--red, #c08a8a); }
      .cs-del:hover { background: rgba(192,138,138,0.2); }
      /* Detached (expanded) mode: let settings fill the whole window so no
         blank translucent area remains below the content. */
      #app.detached .plugin-result {
        height: 100%;
        box-sizing: border-box;
        padding: 8px 12px;
      }
      #app.detached .plugin-result > .settings-layout {
        height: 100%;
      }
      /* Skill store list */
      .settings-skill-list { max-height: 220px; overflow-y: auto; }
      /* 动作按钮（打开 / 编辑）：底色与边框走 --accent 系，**文字走灰阶**
         （ai-spec §11 规则 45 —— 文字只跟明暗、不带色相）。 */
      .settings-skill-open, .settings-skill-edit {
        padding: 3px 10px;
        font-size: 0.68rem;
        border-radius: 4px;
        border: 1px solid var(--accent-border);
        background: var(--accent-bg);
        color: var(--text);
        cursor: pointer;
        flex-shrink: 0;
        transition: background 0.1s;
      }
      .settings-skill-open:hover, .settings-skill-edit:hover { background: rgba(var(--accent-rgb), 0.2); }
      /* 中性/危险按钮（移除）：无底色，hover 转 --red。 */
      .settings-skill-del, .settings-skill-del-installed {
        padding: 3px 10px;
        font-size: 0.68rem;
        border-radius: 4px;
        border: 1px solid var(--border-glass);
        background: none;
        color: var(--text-dim);
        cursor: pointer;
        flex-shrink: 0;
        transition: background 0.1s, color 0.1s;
      }
      .settings-skill-del:hover, .settings-skill-del-installed:hover { color: var(--red); background: rgba(192,138,138,0.1); }
      /* 两段式确认的「待确认」态：语义色常亮，提示「再点一次才真删」。
         没有它的话 armed 态与常态只差文字，看不出是危险确认。 */
      .settings-skill-del[data-armed="1"],
      .settings-skill-del-installed[data-armed="1"] { color: var(--red); background: rgba(192,138,138,0.14); }
      /* 印象派按钮背景清理（与全局 styles.css 一致：背景全透明，无扫笔/光效） */
      .settings-tool-btn, .settings-skill-open, .settings-skill-edit, .settings-install-btn,
      .settings-save-btn, .custom-model-ok, .settings-skill-del, .settings-skill-del-installed {
        background-image: none;
        box-shadow: none;
      }
      .settings-install-btn.installed { background-image: none; box-shadow: none; }

      /* ── 「风格」分区（外观 / 主题，2026-09-19）───────────────────
         全部用既有 token（--accent / --text-dim / --border-glass），
         这样主题色与主题包一改，这块界面自己也跟着变 —— 唯一例外是色轮环
         与取色器（它们本来就是「展示颜色」的控件，必须显示真实色相）。
         不新增任何滚动条声明（走全局 ::-webkit-scrollbar，见 agent-ui-spec §5.4）。 */
      /* 分组小标题：用户要求「字号大一点、加粗、或用方框圈出来」——
         三者都用上（放大 + 600 字重 + 左侧 accent 竖条 + 淡底框），
         这样「背景 / 主题颜色 / 主题包」在长面板里一眼能找到。
         去掉了 text-transform: uppercase（中文无大小写，纯属噪声）。 */
      /* ① 风格里的分类区域 —— 走「反差四件套」（见 styles.css 的 :root 里 --ctx-* 注释）。
         开关开着时 --ctx-rgb == --accent-rgb，与改造前逐像素一致。 */
      .settings-group-title {
        margin: 18px 0 8px;
        padding: 5px 10px;
        font-size: 0.84rem;
        font-weight: 600;
        color: var(--text);
        background: rgba(var(--ctx-rgb), 0.14);
        border-left: 3px solid rgb(var(--ctx-rgb));
        border-radius: 4px;
      }
      .settings-group-title:first-of-type { margin-top: 6px; }
      /* 「自定义」展开的拉条面板 */
      .ap-sliders-panel {
        display: flex; flex-direction: column; gap: 2px;
        padding: 6px 10px; margin: 4px 0 2px;
        border: 1px solid var(--border-glass); border-radius: 8px;
        background: rgba(var(--ink-rgb), 0.03);
      }
      .ap-sliders-panel.hidden { display: none; }
      .ap-toggle-caret { margin-left: 5px; font-size: 0.62rem; opacity: 0.75; }
      /* 取色器那一行整宽上下列（label 在上、取色器在下并占满宽度）：
         原先是 .settings-row 的右侧窄列，色盘被压成 26px 宽根本没法用。
         整宽后 hex 输入区自然与同面板其它行的左端点对齐。 */
      .ap-row-block { flex-direction: column; align-items: stretch; gap: 6px; }
      .ap-slider { display: flex; align-items: center; gap: 8px; flex: 1; justify-content: flex-end; }
      /* 滑块（拉条）的颜色 = **按钮线条**（2026-09-20 用户要求「按钮也应该包括拉条的颜色」）：
         滑块轨道本身就是一条「线」，与六个按钮的边框、切换开关的轨道同源。
         用**全不透明**的 rgb(...) 而不是带 α 的 rgba(...)：① accent-color 带 α 会把
         轨道与圆点洗淡成看不清；② 默认值下 rgb(var(--btn-line-rgb)) == rgb(var(--accent-rgb))
         == 改造前的 var(--accent)，**逐像素不变**（带 0.32 的 α 就不成立了）。
         主题包锁定 / 「恢复默认主题」开着时 --btn-line-rgb 的内联值被清掉 ⇒ 回落主题色。 */
      .ap-slider input[type="range"] { width: 140px; accent-color: rgb(var(--btn-line-rgb)); cursor: pointer; }
      .ap-slider-val {
        font-size: 0.7rem; color: var(--text-dim);
        min-width: 48px; text-align: right; font-variant-numeric: tabular-nums;
      }
      .ap-bg-name { font-size: 0.7rem; color: var(--text-dim); margin-right: 8px; }
      /* 主题锁：非默认主题下灰掉「主题颜色」整块（见 buildAppearancePane 的 locked）。
         用 opacity + pointer-events 而不是给每个控件加 disabled 属性 —— 这块里
         有十几个控件（分段按钮 / 取色器 / hex 框 / 色相条 / 预设 / 开关），逐个加
         disabled 必然漏，且取色器的自绘面板不认 disabled。 */
      .ap-locked-group.locked { opacity: 0.42; pointer-events: none; }
      .ap-lock-note {
        font-size: 0.7rem; line-height: 1.45; margin: 0 0 6px;
        color: var(--yellow);
      }
      .ap-lock-note.hidden { display: none; }
      .settings-btn:disabled { opacity: 0.42; cursor: not-allowed; }
      /* ── 应用内取色器（与界面同一套 token，不用系统取色对话框）────
         为什么自己做：input[type=color] 弹出的是 Windows 原生对话框，
         在 WebView2 里样式一个像素都改不了，与暗色玻璃界面完全脱节。 */
      /* 尺寸（2026-09-19 按用户反馈放大）：整宽约 410px（= 设置内容区宽度）、
         总高约 110~120px。原来是挤在行右侧的 26px 宽窄条，色盘根本没法用。 */
      .ap-picker { display: flex; flex-direction: column; gap: 5px; width: 100%; }
      .ap-picker-head { display: flex; align-items: center; gap: 6px; }
      /* ③ 取色器的框格（色块 / 展开面板 / 饱和度-明度框 / 色相条 / 预设色块 / 收合按钮）
         —— 描边一律走 --ctx-border-glass，底色暗则偏白、底色亮则偏黑，保证框看得见。
         注意：饱和度-明度框的**渐变本身**（linear-gradient 的黑/白）不许动，见 ai-spec 规则 48。 */
      .ap-swatch-btn {
        width: 34px; height: 26px; padding: 0; cursor: pointer;
        border: 1px solid var(--ctx-border-glass); border-radius: 6px;
      }
      /* ② 文本框 —— 走「反差四件套」。底色暗 ⇒ --ctx-ink-rgb 是白（比底色亮一档），
         底色亮 ⇒ 是黑（比底色暗一档）；边框同理走 --ctx-border-glass。 */
      .ap-hex {
        width: 96px; padding: 4px 8px; font-size: 0.74rem;
        font-family: ui-monospace, Consolas, monospace;
        background: rgba(var(--ctx-ink-rgb), 0.04); color: var(--text);
        border: 1px solid var(--ctx-border-glass); border-radius: 5px;
      }
      /* 展开/收起按钮：圆角正方形（用户要求「调大调成圆框正方形」） */
      .ap-picker-toggle {
        width: 30px; height: 30px; padding: 0; cursor: pointer;
        display: inline-flex; align-items: center; justify-content: center;
        font-size: 0.8rem; line-height: 1;
        background: none; border: 1px solid var(--ctx-border-glass);
        border-radius: 8px; color: var(--text-dim);
      }
      .ap-picker-toggle:hover { border-color: var(--accent-border); color: var(--accent); }
      .ap-pick-panel {
        display: flex; flex-direction: column; gap: 4px; padding: 6px;
        border: 1px solid var(--ctx-border-glass); border-radius: 8px;
        background: rgba(var(--ctx-ink-rgb), 0.03);
      }
      .ap-pick-panel.hidden { display: none; }
      /* 饱和度/明度面板：整宽 × **80px**（2026-09-20 按用户要求由 40px 调高）。
         它与同组的「饱和度 / 明度」两个滑块是**同一组值**（x = 饱和度、y = 明度），
         所以高度直接决定拖拽精度 —— 40px 时竖直方向只有几十个可分辨位置。 */
      .ap-sv {
        position: relative; width: 100%; height: 80px; cursor: crosshair; touch-action: none;
        border-radius: 6px; border: 1px solid var(--ctx-border-glass);
        background-image:
          linear-gradient(to top, #000, rgba(0,0,0,0)),
          linear-gradient(to right, #fff, rgba(255,255,255,0));
        background-color: var(--ap-hue-color, #f00);
      }
      .ap-sv-cursor, .ap-hue-cursor {
        position: absolute; width: 12px; height: 12px; margin: -6px;
        border-radius: 50%; border: 2px solid #fff;
        box-shadow: 0 0 0 1px rgba(0,0,0,0.5); pointer-events: none;
      }
      /* 色相条：0° 在最左（红），与 TS 侧的 x/宽度 → 0~360 口径必须一致 */
      .ap-hue {
        position: relative; height: 14px; border-radius: 7px; cursor: pointer; touch-action: none;
        border: 1px solid var(--ctx-border-glass);
        background: linear-gradient(to right,
          hsl(0 100% 50%), hsl(60 100% 50%), hsl(120 100% 50%), hsl(180 100% 50%),
          hsl(240 100% 50%), hsl(300 100% 50%), hsl(360 100% 50%));
      }
      .ap-hue-cursor { top: 50%; }
      .ap-presets { display: flex; gap: 5px; flex-wrap: wrap; }
      .ap-preset {
        width: 18px; height: 18px; padding: 0; cursor: pointer;
        border-radius: 4px; border: 1px solid var(--ctx-border-glass);
      }
      .ap-themes { display: flex; flex-wrap: wrap; gap: 6px; }
      .ap-theme {
        display: inline-flex; align-items: center; gap: 6px;
        padding: 5px 10px; font-size: 0.72rem; border-radius: 6px;
        border: 1px solid var(--border-glass); background: none;
        color: var(--text-dim); cursor: pointer;
      }
      /* 选中态同 .ap-seg-btn.active：主题名也是「文字」，一律保持中性灰阶，
         不带主题色相 —— 选中与否靠底色/边框表达（用户批 4 任务 2 的要求）。 */
      .ap-theme.active { border-color: var(--accent-border); background: var(--accent-bg); color: var(--text); }
      .ap-badge { font-size: 0.62rem; opacity: 0.75; }
    `;
    document.head.appendChild(style);
  }

  // ── Sidebar category switching ───────────────────────────────
  const sidebarItems = container.querySelectorAll(".settings-sidebar-item");
  const panes = container.querySelectorAll(".settings-pane");

  sidebarItems.forEach(item => {
    item.addEventListener("click", () => {
      const cat = (item as HTMLElement).dataset.cat!;
      activeSettingsCategory = cat;
      sidebarItems.forEach(s => s.classList.remove("active"));
      item.classList.add("active");
      panes.forEach(p => p.classList.remove("active"));
      const target = container.querySelector(`[data-pane="${cat}"]`);
      if (target) target.classList.add("active");
    });
  });

  // Restore last active category (or default to "general")
  const restoreCat = activeSettingsCategory;
  const restoreItem = container.querySelector(`.settings-sidebar-item[data-cat="${restoreCat}"]`);
  const restorePane = container.querySelector(`[data-pane="${restoreCat}"]`);
  if (restoreItem) restoreItem.classList.add("active");
  else if (sidebarItems.length > 0) { sidebarItems[0].classList.add("active"); activeSettingsCategory = "general"; }
  if (restorePane) restorePane.classList.add("active");
  else if (panes.length > 0) panes[0].classList.add("active");

  // ── Hotkey recording ─────────────────────────────────────────
  const hotkeyBtn = container.querySelector("#settings-hotkey-btn") as HTMLElement | null;
  if (hotkeyBtn) {
    console.log("[lunac settings] hotkeyBtn FOUND, setting up JS recording listeners...");
    // Clean up previous listeners to avoid memory leaks from repeated open/close
    if (_onRecordingCaptured) {
      window.removeEventListener("lunac-recording-captured", _onRecordingCaptured);
      _onRecordingCaptured = null;
    }
    if (_onRecordingCancelled) {
      window.removeEventListener("lunac-recording-cancelled", _onRecordingCancelled);
      _onRecordingCancelled = null;
    }
    if (_clickOutsideHandler) {
      document.removeEventListener("click", _clickOutsideHandler);
      _clickOutsideHandler = null;
    }
    _unlistenHotkeyRecorded?.();
    _unlistenHotkeyRecorded = null;

    // Capture in local const so nested functions get narrowed type (TS18047 fix)
    const hkBtn = hotkeyBtn;

    // Shared handler: persist recorded combo + update UI
    function recordCombo(combo: string) {
      console.log("[lunac settings] RECORDING captured:", combo);
      (window as any).__lunac_recording_active = false;
      if (hkBtn.classList.contains("recording")) {
        hkBtn.textContent = combo;
        hkBtn.classList.remove("recording");
        hkBtn.setAttribute("data-original", combo);
        hkBtn.title = combo;
        invoke("set_hotkey_combo", { combo })
          .then(() => {
            console.log("[lunac settings] Hotkey saved:", combo);
            (window as any).__lunac_hotkey_is_alt_space = combo === "Alt+Space";
            // 同步刷新状态栏热键提示（Rust 已重装后端：优先 RegisterHotKey，失败才装钩子）
            (window as any).__lunac_refresh_hotkey_hint?.(combo);
          })
          .catch((err: any) => console.error("[lunac settings] FAILED to save hotkey:", err));
        invoke("set_recording_state", { recording: false }).catch(() => {});
      }
    }

    // ── JS-level recording: DOM custom event (for most keys) ─────
    _onRecordingCaptured = (e: Event) => {
      const { combo } = (e as CustomEvent).detail;
      recordCombo(combo);
    };
    window.addEventListener("lunac-recording-captured", _onRecordingCaptured);

    // ── Rust-level recording: Tauri event (for Alt+Space via WndProc) ──
    // Chromium swallows Space keydown in its internal menu mode when Alt is
    // held, so JS keydown never fires. The WndProc subclass directly emits
    // this Tauri event when RECORDING is true and SC_KEYMENU lParam=0x20 fires.
    listen<{ vk: number; modifiers: number; combo: string }>(
      "lunac-hotkey-recorded",
      (event) => { recordCombo(event.payload.combo); }
    ).then(unlisten => { _unlistenHotkeyRecorded = unlisten; });

    _onRecordingCancelled = () => {
      if (hotkeyBtn.classList.contains("recording")) {
        hotkeyBtn.classList.remove("recording");
        hotkeyBtn.textContent = hotkeyBtn.getAttribute("data-original") || "Alt+Space";
        invoke("set_recording_state", { recording: false }).catch(() => {});
      }
    };
    window.addEventListener("lunac-recording-cancelled", _onRecordingCancelled);

    hotkeyBtn.addEventListener("click", async () => {
      // Toggle recording off
      if (hotkeyBtn.classList.contains("recording")) {
        console.log("[lunac settings] Toggle recording OFF");
        (window as any).__lunac_recording_active = false;
        await invoke("set_recording_state", { recording: false }).catch(() => {});
        hotkeyBtn.classList.remove("recording");
        hotkeyBtn.textContent = hotkeyBtn.getAttribute("data-original") || "Alt+Space";
        return;
      }
      // Toggle recording on — sync Rust RECORDING FIRST to prevent LL hook from
      // swallowing the Space key before JS can capture it (race condition fix).
      console.log("[lunac settings] Toggle recording ON (syncing Rust first)...");
      const originalText = hotkeyBtn.textContent || "Alt+Space";
      hotkeyBtn.setAttribute("data-original", originalText);
      hotkeyBtn.classList.add("recording");
      hotkeyBtn.textContent = t("settings.hotkey_prompt");
      try {
        await invoke("set_recording_state", { recording: true });
        console.log("[lunac settings] set_recording_state(true) SUCCESS — LL hook will now pass keys through");
      } catch (e) {
        console.error("[lunac settings] set_recording_state(true) FAILED:", e, "— Alt+Space recording WILL NOT work!");
        hotkeyBtn.classList.remove("recording");
        hotkeyBtn.textContent = originalText;
        return;
      }
      // Only set JS flag AFTER Rust RECORDING is true — prevents race
      (window as any).__lunac_recording_active = true;
    });

    // Click outside hotkey button → cancel recording
    _clickOutsideHandler = (e: Event) => {
      if (!hotkeyBtn.classList.contains("recording")) return;
      if (!hotkeyBtn.contains(e.target as Node)) {
        hotkeyBtn.classList.remove("recording");
        hotkeyBtn.textContent = hotkeyBtn.getAttribute("data-original") || "Alt+Space";
        (window as any).__lunac_recording_active = false;
        invoke("set_recording_state", { recording: false }).catch(() => {});
      }
    };
    document.addEventListener("click", _clickOutsideHandler);
  } else {
    console.error("[lunac settings] hotkeyBtn NOT FOUND! Cannot set up recording. Searching for #settings-hotkey-btn inside:", container.id, "innerHTML length:", container.innerHTML.length);
  }

  // ── Auto-start toggle ────────────────────────────────────────
  const autoStartCheck = container.querySelector("#settings-autostart") as HTMLInputElement | null;
  if (autoStartCheck) {
    autoStartCheck.addEventListener("change", async () => {
      try {
        // 开启 = 后端一次做完：先写 Run 键（保证开关一定生效），再自动弹一次管理员
        // 确认补建「登录时计划任务」（开机后早约 60 秒可用）。取消确认不是错误 ——
        // 仍然是「已开启，只是慢」。关闭 = 计划任务若存在，同样要管理员确认才删得掉。
        // 机制细节只进落盘日志，不在 UI 暴露（见 ai-spec §11 规则 1）。
        await invoke("set_auto_start", { enabled: autoStartCheck.checked });
      } catch {
        // Revert checkbox on failure to keep UI consistent
        autoStartCheck.checked = !autoStartCheck.checked;
      }
    });
  }

  // ── 外观 / 主题（「风格」分区）─────────────────────────────────
  // 2026-09-19：原来写在这里的「自定义背景」两个按钮被这一段整体取代 —— 配置与
  // 应用统一收敛到 main.ts 的 `__lunac_appearance`（背景/滑块/主题色/主题包），
  // 设置侧只调桥，不再自己写 localStorage（两处实现必然漂移，见该分区顶部注释）。
  const apBridge = appearanceBridge();
  if (apBridge) attachAppearanceControls(container, apBridge);
  else console.warn("[lunac settings] appearance bridge unavailable — 风格分区控件不生效");

  // ── OCR 引擎（按需下载，不随发行包分发）──────────────────────
  const ocrInstallBtn = container.querySelector("#settings-ocr-install") as HTMLButtonElement | null;
  const ocrStatusEl = container.querySelector("#settings-ocr-status") as HTMLElement | null;
  if (ocrInstallBtn) {
    const syncOcrEngineState = async () => {
      let installed = false;
      try { installed = await invoke<boolean>("ocr_engine_status"); } catch { /* 查询失败按未安装显示 */ }
      ocrInstallBtn.disabled = installed;
      ocrInstallBtn.textContent = installed ? t("ocr.engine_installed") : t("ocr.engine_download");
      if (ocrStatusEl) ocrStatusEl.textContent = installed ? "" : t("ocr.engine_missing");
    };
    void syncOcrEngineState();
    ocrInstallBtn.addEventListener("click", async () => {
      if (ocrInstallBtn.disabled) return;
      ocrInstallBtn.disabled = true;
      ocrInstallBtn.textContent = t("ocr.engine_downloading").replace("{percent}", "0");
      const ok = await installOcrEngine(({ percent, mb }) => {
        ocrInstallBtn.textContent = percent > 0
          ? t("ocr.engine_downloading").replace("{percent}", String(percent))
          : t("ocr.engine_downloading_unknown").replace("{mb}", mb.toFixed(1));
      });
      if (ok) {
        ocrInstallBtn.textContent = t("ocr.engine_installed");
        if (ocrStatusEl) ocrStatusEl.textContent = t("ocr.engine_ready");
      } else {
        ocrInstallBtn.disabled = false;
        ocrInstallBtn.textContent = t("ocr.engine_retry");
        if (ocrStatusEl) ocrStatusEl.textContent = t("ocr.engine_failed");
      }
    });
  }

  // ── AI Provider ──────────────────────────────────────────────
  const providerDD = container.querySelector("#settings-provider") as HTMLElement | null;
  // let：供应商切换时 reloadModelSelector 会用 replaceWith 换成新节点并回写本引用，
  // 否则二次切换仍操作已被移除的旧节点（parentElement 为 null → 模型下拉卡死不更新）。
  let modelDD = container.querySelector("#settings-model") as HTMLElement | null;
  const searchEngineDD = container.querySelector("#settings-search-engine") as HTMLElement | null;
  const searchProviderDD = container.querySelector("#settings-search-provider") as HTMLElement | null;
  const baseUrlInput = container.querySelector("#settings-baseurl") as HTMLInputElement | null;
  const apiKeyInput = container.querySelector("#settings-apikey") as HTMLInputElement | null;

  // ── Custom dropdown setup ────────────────────────────────────
  function setupCustomDropdown(dd: HTMLElement, onChange: (value: string) => void) {
    const trigger = dd.querySelector(".custom-select-trigger") as HTMLElement | null;
    const dropdown = dd.querySelector(".custom-select-dropdown") as HTMLElement | null;
    const label = dd.querySelector(".custom-select-label") as HTMLElement | null;
    if (!trigger || !dropdown || !label) return;

    // Helper: allow dropdown to overflow parent containers
    const setOverflowVisible = (v: boolean) => setDropdownOverflow(container, v);

    /**
     * Position the dropdown absolutely within .custom-select (position:relative)
     * so it sits flush below the trigger. IMPORTANT: position:fixed is broken
     * here — #results-container has `backdrop-filter`, which per spec makes it
     * the containing block for fixed descendants, so a viewport-coordinate top
     * lands ~searchbar-height BELOW the trigger (需求2 复测: 52px gap). Absolute
     * is immune to that offset.
     *
     * 可用空间按**滚动容器（`#results-list`）的可视区**计算，不再按窗口算：
     * 裁剪下拉框的是那一层（它 `overflow-y: auto`），按窗口算会在容器底部
     * 明明放不下时仍然朝下展开 → 被裁掉半截；而放宽它的 overflow 会让整页
     * 跳回顶部（见 setDropdownOverflow 的注释），所以只能在可视区内翻转 + 限高。
     */
    function positionDropdown() {
      const dropEl = dd.querySelector(".custom-select-dropdown") as HTMLElement | null;
      const trigEl = dd.querySelector(".custom-select-trigger") as HTMLElement | null;
      if (!dropEl || !trigEl) return;
      const trigRect = trigEl.getBoundingClientRect();
      const MARGIN = 8;
      // 目标高度：约 4 行可见（紧凑，不把面板撑满窗口），
      // 更多项（如自定义模型）通过列表内部滚动条拉动查看。
      const TARGET_H = 120;
      // 边界 = 滚动容器的可视区（找不到时退回窗口）
      const scroller = dd.closest("#results-list") as HTMLElement | null;
      const box = scroller?.getBoundingClientRect();
      const boundTop = Math.max(box ? box.top : 0, 0);
      const boundBottom = Math.min(box ? box.bottom : window.innerHeight, window.innerHeight);
      const spaceBelow = boundBottom - trigRect.bottom - MARGIN;
      const spaceAbove = trigRect.top - boundTop - MARGIN;
      // 下方放得下就朝下展开；下方不够（如面板底部）则翻到上方；
      // 高度固定 TARGET_H，极小窗口下保底 60px，始终可内部滚动。
      const openUp = spaceBelow < TARGET_H && spaceAbove > spaceBelow;
      const sideSpace = openUp ? spaceAbove : spaceBelow;
      const maxH = Math.max(60, Math.min(TARGET_H, sideSpace));
      dropEl.style.overflowY = "auto";
      dropEl.style.overscrollBehavior = "contain"; // 滚轮只滚本列表，不穿透外层
      dropEl.style.position = "absolute";
      dropEl.style.left = "0";
      dropEl.style.width = "100%";
      dropEl.style.maxHeight = maxH + "px";
      if (openUp) {
        dropEl.style.top = "auto";
        dropEl.style.bottom = "calc(100% + 4px)";
        dropEl.style.marginTop = "0";
        dropEl.style.marginBottom = "0";
      } else {
        dropEl.style.top = "calc(100% + 4px)";
        dropEl.style.bottom = "auto";
        dropEl.style.marginTop = "0";
        dropEl.style.marginBottom = "0";
      }
    }

    // Toggle open/close
    trigger.addEventListener("click", (e: Event) => {
      e.preventDefault(); e.stopPropagation();
      const wasOpen = dd.classList.contains("open");
      // Close all other custom selects first
      container.querySelectorAll(".custom-select.open").forEach(el => {
        if (el !== dd) el.classList.remove("open");
      });
      if (wasOpen) {
        dd.classList.remove("open");
        setOverflowVisible(false);
      } else {
        dd.classList.add("open");
        setOverflowVisible(true);
        // position:fixed + dynamic height — never split/clipped on small windows
        positionDropdown();
        // Re-position if the window is resized while open
        window.addEventListener("resize", positionDropdown, { once: true });
      }
    });

    // Select option (and handle per-option delete buttons)
    dropdown.addEventListener("click", (e: Event) => {
      const target = e.target as HTMLElement;
      // ── Delete button (×): remove the option (custom → also persisted) ──
      const delBtn = target.closest(".cs-del") as HTMLElement | null;
      if (delBtn) {
        e.preventDefault(); e.stopPropagation();
        const opt = delBtn.closest(".custom-select-option") as HTMLElement | null;
        if (!opt) return;
        const value = opt.getAttribute("data-value") || "";
        const tag = opt.getAttribute("data-tag") || "builtin";
        const isProvider = dd.id === "settings-provider";
        // Custom entries persist in localStorage; built-ins return next time the panel opens.
        if (tag === "custom") {
          try {
            if (isProvider) {
              const arr: string[] = JSON.parse(localStorage.getItem("lunac-custom-providers") || "[]");
              const i = arr.indexOf(value);
              if (i >= 0) { arr.splice(i, 1); localStorage.setItem("lunac-custom-providers", JSON.stringify(arr)); }
            } else {
              const prov = (container.querySelector("#settings-provider .custom-select-option.selected")?.getAttribute("data-value")) || "";
              const map = JSON.parse(localStorage.getItem("lunac-custom-models") || "{}");
              const arr: string[] = (map && map[prov]) || [];
              const i = arr.indexOf(value);
              if (i >= 0) { arr.splice(i, 1); map[prov] = arr; localStorage.setItem("lunac-custom-models", JSON.stringify(map)); }
            }
          } catch {}
        } else if (tag === "builtin") {
          // Built-in entries: persist a hidden-list so the deletion survives panel reopen
          try {
            if (isProvider) {
              const arr: string[] = JSON.parse(localStorage.getItem("lunac-hidden-providers") || "[]");
              if (!arr.includes(value)) { arr.push(value); localStorage.setItem("lunac-hidden-providers", JSON.stringify(arr)); }
            } else {
              const prov = (container.querySelector("#settings-provider .custom-select-option.selected")?.getAttribute("data-value")) || "";
              const map = JSON.parse(localStorage.getItem("lunac-hidden-models") || "{}");
              const arr: string[] = (map && map[prov]) || [];
              if (!arr.includes(value)) { arr.push(value); map[prov] = arr; localStorage.setItem("lunac-hidden-models", JSON.stringify(map)); }
            }
          } catch {}
        }
        // If the removed option was selected, fall back to the first remaining one
        if (opt.classList.contains("selected")) {
          const remaining = dropdown.querySelectorAll(".custom-select-option:not([data-value=\"__custom__\"])");
          const first = remaining[0] as HTMLElement | undefined;
          if (first) {
            dropdown.querySelectorAll(".custom-select-option").forEach(o => o.classList.remove("selected"));
            first.classList.add("selected");
            label.textContent = first.textContent?.replace(/\s*×$/, "").trim() || "";
            if (isProvider) onChange(first.getAttribute("data-value") || "");
          } else {
            label.textContent = "";
          }
        }
        opt.remove();
        return;
      }
      const opt = target.closest(".custom-select-option") as HTMLElement | null;
      if (!opt) return;
      e.preventDefault(); e.stopPropagation();
      const value = opt.getAttribute("data-value") || "";
      if (value === "__custom__") {
        // WebView2 does NOT support window.prompt() — use an inline input
        promptCustomModelInput(dd, label, (val) => {
          commitCustomOption(dd, val, label);
          onChange(val);
        }, "", setOverflowVisible);
        return;
      }
      // Update visual selection
      dropdown.querySelectorAll(".custom-select-option").forEach(o => o.classList.remove("selected"));
      opt.classList.add("selected");
      label.textContent = opt.textContent?.replace(/\s*×$/, "").trim() || "";
      dd.classList.remove("open");
      setOverflowVisible(false);
      onChange(value);
    });

    // 长列表（如模型多、含自定义项时）强制内部滚动：
    // 项超出可视区后滚轮在 WebView2 下可能被外层 #results-list 吞掉，
    // 这里手动滚自己，保证永远能滚到列表底部的自定义项/新增项。
    dropdown.addEventListener("wheel", (e: WheelEvent) => {
      const canUp = dropdown.scrollTop > 0;
      const canDown = dropdown.scrollTop + dropdown.clientHeight < dropdown.scrollHeight - 1;
      if ((e.deltaY < 0 && canUp) || (e.deltaY > 0 && canDown)) {
        e.preventDefault();
        dropdown.scrollTop += e.deltaY;
      }
    }, { passive: false });
  }

  // Close dropdowns on outside click
  document.addEventListener("click", (e: Event) => {
    const target = e.target as HTMLElement;
    let anyOpen = false;
    container.querySelectorAll(".custom-select.open").forEach(dd => {
      if (dd.contains(target)) {
        anyOpen = true;
      } else {
        dd.classList.remove("open");
      }
    });
    if (!anyOpen) setDropdownOverflow(container, false);
  });

  // Provider dropdown
  if (providerDD) {
    setupCustomDropdown(providerDD, (value) => {
      const preset = PROVIDER_PRESETS[value];
      if (baseUrlInput) {
        // 内置预设填默认地址；自定义供应商（不在预设表）清空等待填写
        baseUrlInput.value = preset?.default_url ?? "";
      }
      // Rebuild model selector for this provider
      const oldModel = modelDD?.querySelector(".custom-select-label")?.textContent || "";
      reloadModelSelector(value, oldModel);
    });
  }

  // Search provider dropdown（WebSearch 主源；值由「保存」按钮统一读取持久化）
  if (searchProviderDD) {
    setupCustomDropdown(searchProviderDD, () => {});
  }

  // Model dropdown init + inline edit. 供应商切换会用新节点 replaceWith 重建，
  // 每个新节点都必须重新绑定（下拉逻辑 + 双击标签编辑自定义模型）。
  const setupModelDropdown = (dd: HTMLElement) => {
    setupCustomDropdown(dd, () => {}); // onChange is no-op, save button handles persistence
    // Double-click the selected label to type a custom model
    const trigger = dd.querySelector(".custom-select-trigger") as HTMLElement | null;
    const label = dd.querySelector(".custom-select-label") as HTMLElement | null;
    if (trigger && label) {
      trigger.addEventListener("dblclick", () => {
        const oldVal = label.textContent || "";
        // WebView2 does NOT support window.prompt() — use an inline input
        promptCustomModelInput(dd, label, (val) => {
          commitCustomOption(dd, val, label);
        }, oldVal, (v) => setDropdownOverflow(container, v));
      });
    }
  };
  if (modelDD) setupModelDropdown(modelDD);

  // ── Save button (manual, replaces auto-save on blur/change) ────
  const saveAiBtn = container.querySelector("#settings-save-ai-btn") as HTMLButtonElement | null;
  const saveMsg = container.querySelector("#settings-save-msg") as HTMLElement | null;
  if (saveAiBtn) {
    saveAiBtn.addEventListener("click", async () => {
      saveAiBtn.disabled = true;
      saveAiBtn.textContent = t("settings.saving");
      try {
        await saveAIConfig();
        if (saveMsg) {
          saveMsg.textContent = t("settings.saved_ok");
          saveMsg.classList.add("show");
          setTimeout(() => saveMsg.classList.remove("show"), 2000);
        }
      } catch {
        if (saveMsg) {
          saveMsg.textContent = t("settings.saved_fail");
          saveMsg.style.color = "var(--red)";
          saveMsg.classList.add("show");
          setTimeout(() => {
            saveMsg.classList.remove("show");
            saveMsg.style.color = "";
          }, 2000);
        }
      }
      saveAiBtn.disabled = false;
      saveAiBtn.textContent = t("settings.save");
    });
  }

  /** Rebuild the model selector when provider changes (custom dropdown).
   *  切换供应商时不把上一供应商的模型名带过来（旧模型属于别的供应商 → 回落本供应商
   *  默认模型）；但不认识的名字一律**原样保留**并标成「(自定义)」，绝不静默改写
   *  （判据见 `belongsToOtherProvider`）。 */
  function reloadModelSelector(provider: string, oldModel: string) {
    const row = modelDD?.parentElement;
    if (!row || !modelDD) return;
    const preset = PROVIDER_PRESETS[provider];
    const suggestions = MODEL_SUGGESTIONS[provider] || [];
    // 该供应商下已保存的自定义模型（切换后仍在列表中展示）
    let savedCustoms: string[] = [];
    try {
      const map = JSON.parse(localStorage.getItem("lunac-custom-models") || "{}");
      savedCustoms = ((map && map[provider]) || []).filter((m: string) => m && !suggestions.includes(m));
    } catch {}
    const selected = (oldModel && !belongsToOtherProvider(oldModel, provider))
      ? oldModel
      : (preset?.default_model || "");
    // Filter out built-in models the user deleted (persisted hidden-list)
    let hiddenModels: string[] = [];
    try {
      const hm = JSON.parse(localStorage.getItem("lunac-hidden-models") || "{}");
      hiddenModels = ((hm && hm[provider]) || []).filter((m: string) => m !== selected);
    } catch {}
    const modelList = suggestions.filter(m => !hiddenModels.includes(m));
    const hasCustom = !!selected && !modelList.includes(selected);

    const optsHtml = modelList.map(m =>
      `<div class="custom-select-option${m === selected ? ' selected' : ''}" data-value="${esc(m)}" data-tag="builtin">${esc(m)}<span class="cs-del" title="×">×</span></div>`
    ).join("");
    const savedHtml = savedCustoms.filter(m => m !== selected).map(m =>
      `<div class="custom-select-option" data-value="${esc(m)}" data-tag="custom">${esc(m)} (${t("settings.custom_model")})<span class="cs-del" title="×">×</span></div>`
    ).join("");
    const customHtml = hasCustom
      ? `<div class="custom-select-option selected" data-value="${esc(selected)}" data-tag="custom">${esc(selected)} (${t("settings.custom_model")})<span class="cs-del" title="×">×</span></div>`
      : "";
    const newHtml = `
      <div class="custom-select" id="settings-model">
        <button class="custom-select-trigger" type="button">
          <span class="custom-select-label">${esc(selected || t("settings.select_placeholder"))}</span>
          <svg class="custom-select-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="6 9 12 15 18 9"/></svg>
        </button>
        <div class="custom-select-dropdown">
          ${optsHtml}
          ${savedHtml}
          ${customHtml}
          <div class="custom-select-option" data-value="__custom__">${t("settings.custom_placeholder")}</div>
        </div>
      </div>`;
    const tmp = document.createElement("div");
    tmp.innerHTML = newHtml;
    const newDD = tmp.firstElementChild as HTMLElement;
    modelDD.replaceWith(newDD);
    // 关键：回写外层引用，否则下一次供应商切换拿到的还是被移除的旧节点，
    // reloadModelSelector 会因 parentElement==null 提前 return → 模型下拉卡死。
    modelDD = newDD;
    // Re-setup listeners on the new dropdown (下拉 + 双击编辑)
    setupModelDropdown(newDD);
  }

  async function saveAIConfig() {
    try {
      const providerOpt = container.querySelector("#settings-provider .custom-select-option.selected");
      const provider = providerOpt?.getAttribute("data-value") || "openai";
      const modelOpt = container.querySelector("#settings-model .custom-select-option.selected");
      // 只认真正选中的选项（含自定义输入提交后生成的选中项）；未选择时为空串，
      // 由后端 set_ai_config 校验拒绝，避免把下拉占位符文本当模型名保存。
      const model = modelOpt?.getAttribute("data-value") || "";
      const baseUrl = (container.querySelector("#settings-baseurl") as HTMLInputElement)?.value || "";
      const apiKey = (container.querySelector("#settings-apikey") as HTMLInputElement)?.value || "";
      // WebSearch 主源（服务商 + 密钥）。每次回传当前值，空串 = 清除
      // （后端按删除处理，agent 回落到免 key 的 Bing / 百度兜底源）。
      const searchProvider = container.querySelector("#settings-search-provider .custom-select-option.selected")?.getAttribute("data-value") || "";
      const searchKey = (container.querySelector("#settings-searchkey") as HTMLInputElement)?.value || "";
      // 图片输入开关（A8）：与模型一起存进 ai.json。**默认关** —— 发给不支持视觉的
      // 端点会 400，所以只有用户明确断言「这个模型能看图」时才打开。
      const vision = (container.querySelector("#settings-vision") as HTMLInputElement)?.checked || false;
      // Preserve existing agent_url if set (don't overwrite with empty)
      let agentUrl = "";
      try {
        const cur = await invoke<{ agent_url?: string }>("get_ai_config");
        agentUrl = cur.agent_url || "";
      } catch {}
      await invoke("set_ai_config", {
        provider,
        url: baseUrl,
        key: apiKey,
        model,
        agent_url: agentUrl,
        search_provider: searchProvider,
        search_key: searchKey,
        vision,
      });
      // 落盘由后端完成（<exe 根>\config\ai.json，唯一真相源）。
      // 这里**不要**再写 localStorage —— 旧的 localStorage 回灌会在启动时覆盖
      // .env，导致「改了 key 不生效」（2026-09-15 实测踩到，见 ai-spec §11 规则 2）。
    } catch (e) {
      console.warn("[lunac] saveAIConfig failed:", e);
    }
  }

  // ── 权限 hooks（A9）───────────────────────────────────────────
  // 开关写的是 config\hooks.json 的 `enabled` 字段（agent 侧按 mtime 热重载 ⇒
  // 改完**即时生效、不需要重启 agent**）。失败必须**把开关拨回去**并把原因显示出来 ——
  // 面板显示「已开」而实际没生效，是最难查的一类不一致。
  const hooksToggle = container.querySelector("#settings-hooks") as HTMLInputElement | null;
  const hooksMsg = container.querySelector("#settings-hooks-msg") as HTMLElement | null;
  const hooksOpenBtn = container.querySelector("#settings-hooks-open") as HTMLButtonElement | null;
  hooksToggle?.addEventListener("change", async () => {
    const want = hooksToggle.checked;
    try {
      await invoke("set_hooks_enabled", { enabled: want });
      if (hooksMsg) hooksMsg.textContent = "";
    } catch (e) {
      hooksToggle.checked = !want;
      if (hooksMsg) hooksMsg.textContent = t("settings.hooks_failed", { err: String(e) });
    }
  });
  hooksOpenBtn?.addEventListener("click", async () => {
    try {
      // 缺文件时后端先落一份骨架再返回路径；真正的「打开」交给前端 `open()`
      // （与主题目录那行同一套做法，见文件顶部 import）。
      const p = await invoke<string>("hooks_file_path");
      await open(p);
    } catch (e) {
      if (hooksMsg) hooksMsg.textContent = t("settings.hooks_failed", { err: String(e) });
    }
  });

  // ── Search engine save ──────────────────────────────────────
  const saveSearchBtn = container.querySelector("#settings-save-search-btn") as HTMLButtonElement | null;
  const saveSearchMsg = container.querySelector("#settings-save-search-msg") as HTMLElement | null;
  if (saveSearchBtn && saveSearchMsg) {
    saveSearchBtn.addEventListener("click", () => {
      try {
        const opt = container.querySelector("#settings-search-engine .custom-select-option.selected");
        const engine = opt?.getAttribute("data-value") || "google";
        localStorage.setItem("lunac-search-engine", engine);
        // Notify web-search plugin to refresh (via custom event)
        window.dispatchEvent(new CustomEvent("lunac-search-engine-changed", { detail: engine }));
        saveSearchMsg.textContent = t("settings.saved");
        saveSearchMsg.classList.add("show");
        setTimeout(() => { saveSearchMsg.textContent = ""; saveSearchMsg.classList.remove("show"); }, 1500);
      } catch (e) {
        console.warn("[lunac] saveSearchEngine failed:", e);
      }
    });
  }

  // Search engine dropdown
  if (searchEngineDD) {
    setupCustomDropdown(searchEngineDD, () => {}); // onChange is no-op, save button handles persistence
  }

  // ── 插件总览：每行的「打开」按钮（2026-09-19 批 9）── 绑定抽成函数，重绘后要再绑一次
  bindOpenPluginButtons(container);
  // ── 插件市场（L1，2026-09-21）：安装 / 卸载 / 重绘 ──────────────
  wirePluginMarket(container);

  // ── 用量与成本（A12）─────────────────────────────────────────
  // 分块自己会重渲染（确认 / 放弃候选价格之后），所以绑定的入口是个函数，
  // 重渲染完它会再调自己一次。
  wireUsageCost(container);

  // ── 人格 / 自定义提示词（L2）───────────────────────────────────
  // 绑定入口独立成函数：分块内部若重渲染，直接再调一次 `wirePersona` 即可。
  wirePersona(container);

  // ── AI 安全档位（文件边界）─────────────────────────────────────
  // 单独一个下拉，不跟 provider/model 那条保存链路混：切换立即生效（会重启 agent）。
  const profileDD = container.querySelector("#settings-security-profile") as HTMLElement | null;
  if (profileDD) {
    setupCustomDropdown(profileDD, (value) => {
      // 由 main.ts 统一 invoke set_security_profile 并同步输入栏胶囊显示；
      // 这里只广播，避免两个 invoke 点各自重启一次 agent。
      window.dispatchEvent(new CustomEvent("lunac-security-profile-changed", {
        detail: { profile: value },
      }));
    });
  }

  // ── 回合自动折叠开关 ──────────────────────────────────────────
  const autofoldCheck = container.querySelector("#settings-autofold") as HTMLInputElement | null;
  if (autofoldCheck) {
    autofoldCheck.addEventListener("change", () => {
      try {
        localStorage.setItem("lunac-agent-autofold", autofoldCheck.checked ? "1" : "0");
      } catch { /* 存不了就只在本次会话生效 */ }
    });
  }

  // ── Language selector ─────────────────────────────────────────
  const langDD = container.querySelector("#settings-language") as HTMLElement | null;
  if (langDD) {
    setupCustomDropdown(langDD, async (value) => {
      try {
        if (value === "system") {
          await resetToSystemLanguage();
        } else {
          setLanguage(value);
        }
      } catch (e) {
        console.warn("[lunac] applyLanguage failed:", e);
      }
      // Re-render the settings pane (and any open UI) in the new language.
      (window as any).__lunac_refresh_plugin?.("settings");
      (window as any).__lunac_apply_language?.();
    });
  }

  // ── Open Tool Editor ─────────────────────────────────────────
  const openToolsBtn = container.querySelector("#settings-open-tools");
  if (openToolsBtn) {
    openToolsBtn.addEventListener("click", () => {
      (window as any).__lunac_execute_tool_editor?.();
    });
  }

  // ── Install from URL ─────────────────────────────────────────
  const installUrlBtn = container.querySelector("#settings-install-url");
  const toolUrlInput = container.querySelector("#settings-tool-url") as HTMLInputElement | null;
  const installMsg = container.querySelector("#settings-install-msg") as HTMLElement | null;
  if (installUrlBtn && toolUrlInput && installMsg) {
    installUrlBtn.addEventListener("click", async () => {
      const url = toolUrlInput.value.trim();
      if (!url) { showMsg(t("settings.enter_url")); return; }
      try {
        new URL(url); // validate URL format
      } catch { showMsg(t("settings.invalid_url")); return; }

      installMsg.style.display = "block";
      installMsg.style.color = "var(--yellow)";
      installMsg.textContent = t("settings.downloading");
      try {
        const fn = await invoke<string>("download_tool_from_url", { url });
        showMsg(t("settings.installed_ok", { name: fn }), "var(--green)");
        toolUrlInput.value = "";
        // Reload settings to refresh tool list
        const evt = new CustomEvent("lunac-reload-settings");
        document.dispatchEvent(evt);
      } catch (e: any) {
        showMsg(t("settings.install_error", { err: String(e) }), "var(--red)");
      }
    });
  }
  function showMsg(text: string, color?: string) {
    if (!installMsg) return;
    installMsg.style.display = "block";
    if (color) installMsg.style.color = color;
    installMsg.textContent = text;
    setTimeout(() => { if (installMsg) installMsg.style.display = "none"; }, 4000);
  }

  // ── Community tool install buttons ───────────────────────────
  container.querySelectorAll(".settings-install-btn[data-cmd]").forEach(btn => {
    btn.addEventListener("click", async () => {
      const name = (btn as HTMLElement).dataset.name!;
      const cmd = (btn as HTMLElement).dataset.cmd!;
      const desc = (btn as HTMLElement).dataset.desc!;
      const toolJson = JSON.stringify({
        name,
        description: desc || name,
        inputSchema: { type: "object", properties: {}, required: [] },
        handler: { type: "shell", command: cmd },
      }, null, 2);

      try {
        const fn = name + ".json";
        await invoke("save_tool_file", { filename: fn, content: toolJson });
        (btn as HTMLElement).textContent = t("settings.installed_btn");
        (btn as HTMLElement).classList.add("installed");
        (btn as HTMLElement).setAttribute("disabled", "true");
        // Reload settings
        const evt = new CustomEvent("lunac-reload-settings");
        document.dispatchEvent(evt);
      } catch (e: any) {
        showMsg(t("settings.install_failed", { err: String(e) }), "var(--red)");
      }
    });
  });

  // ── Skill Store ──────────────────────────────────────────────
  const skillUrlInput = container.querySelector("#settings-skill-url") as HTMLInputElement | null;
  const skillAddBtn = container.querySelector("#settings-skill-add") as HTMLElement | null;
  const skillMsg = container.querySelector("#settings-skill-msg") as HTMLElement | null;
  if (skillMsg) {
    const showSkillMsg = (text: string, color?: string) => {
      skillMsg.style.display = "block";
      if (color) skillMsg.style.color = color;
      skillMsg.textContent = text;
      setTimeout(() => { skillMsg.style.display = "none"; }, 4000);
    };

    // (Re)render the "My Additions" list with open/remove bindings
    const renderCustomSkillList = () => {
      const listEl = container.querySelector("#settings-skill-custom");
      if (!listEl) return;
      const list = loadSkillSites();
      listEl.innerHTML = list.length === 0
        ? `<div class="settings-plugin-empty">${t("settings.skill_empty")}</div>`
        : list.map(s => skillEntryHtml(s.name, s.url, s.url, true)).join("");
      listEl.querySelectorAll(".settings-skill-open").forEach(btn => {
        btn.addEventListener("click", () => {
          const u = (btn as HTMLElement).dataset.url || "";
          if (u) open(u).catch(() => {});
        });
      });
      listEl.querySelectorAll(".settings-skill-del").forEach(btn => {
        btn.addEventListener("click", () => {
          const u = (btn as HTMLElement).dataset.url || "";
          const n = (btn as HTMLElement).dataset.name || u;
          saveSkillSites(loadSkillSites().filter(s => s.url !== u));
          renderCustomSkillList();
          showSkillMsg(t("settings.skill_removed", { name: n }), "var(--yellow)");
        });
      });
    };

    // Recommended entries: open in default browser
    container.querySelectorAll(".settings-skill-open[data-url]").forEach(btn => {
      btn.addEventListener("click", () => {
        const u = (btn as HTMLElement).dataset.url || "";
        if (u) open(u).catch(() => {});
      });
    });

    // ── 已安装技能：打开 / 编辑 / 删除 / URL 安装 / 新建 ──
    const installedListEl = container.querySelector("#settings-skill-installed") as HTMLElement | null;
    const installUrlInput = container.querySelector("#settings-skill-install-url") as HTMLInputElement | null;
    const installUrlBtn = container.querySelector("#settings-skill-install-url-btn") as HTMLButtonElement | null;
    const newSkillBtn = container.querySelector("#settings-skill-new-btn") as HTMLButtonElement | null;
    const opMsg = container.querySelector("#settings-skill-op-msg") as HTMLElement | null;
    const editorEl = container.querySelector("#settings-skill-editor") as HTMLElement | null;
    const editorText = container.querySelector("#settings-skill-editor-text") as HTMLTextAreaElement | null;
    const editorSave = container.querySelector("#settings-skill-editor-save") as HTMLButtonElement | null;
    const editorCancel = container.querySelector("#settings-skill-editor-cancel") as HTMLButtonElement | null;
    let skillEditorKey = ""; // "" = 新建（粘贴导入），否则为已有技能 key（覆盖保存）

    const showOp = (text: string, color?: string) => {
      if (!opMsg) return;
      opMsg.style.display = "block";
      if (color) opMsg.style.color = color;
      opMsg.textContent = text;
      setTimeout(() => { opMsg.style.display = "none"; opMsg.style.color = ""; }, 4000);
    };

    const openSkillEditor = (key: string, content: string) => {
      if (!editorEl || !editorText) return;
      skillEditorKey = key;
      editorText.value = content;
      editorEl.style.display = "block";
      editorText.focus();
    };
    const closeSkillEditor = () => { if (editorEl) editorEl.style.display = "none"; skillEditorKey = ""; };

    /** 技能目录只在 agent.exe 启动时扫描一次，所以技能增删改之后必须重启它才生效
     *  （与「工具黑名单」同一套流程：先存会话，再 stop_cli → start_cli）。
     *  返回是否真的重启了 —— 没接到桥（旧前端）时不谎称「已生效」。 */
    const applySkillChange = async (): Promise<boolean> => {
      const fn = (window as any).__lunac_reload_agent;
      if (typeof fn !== "function") return false;
      try { await fn(); return true; } catch { return false; }
    };

    const bindInstalledRowActions = () => {
      // 打开技能目录（Explorer）
      installedListEl?.querySelectorAll<HTMLElement>(".settings-skill-open").forEach(btn => {
        btn.addEventListener("click", () => { const d = btn.dataset.dir || ""; if (d) open(d).catch(() => {}); });
      });
      // 编辑（读取 SKILL.md 全文 → 内联编辑器）
      installedListEl?.querySelectorAll<HTMLElement>(".settings-skill-edit").forEach(btn => {
        btn.addEventListener("click", async () => {
          const key = btn.dataset.key || "";
          try {
            const content = await invoke<string>("read_skill_file", { key });
            openSkillEditor(key, content);
          } catch (e: any) { showOp(String(e), "var(--red)"); }
        });
      });
      // 删除（两段式确认：WebView2 下原生 confirm 不可靠）
      installedListEl?.querySelectorAll<HTMLButtonElement>(".settings-skill-del-installed").forEach(btn => {
        btn.addEventListener("click", async () => {
          const key = btn.dataset.key || "";
          if (btn.dataset.armed !== "1") {
            btn.dataset.armed = "1";
            btn.textContent = t("settings.skill_del_confirm");
            setTimeout(() => { btn.dataset.armed = ""; btn.textContent = t("settings.skill_remove"); }, 3000);
            return;
          }
          try {
            await invoke("delete_skill", { key });
            await renderInstalledSkills();
            showOp(t("settings.skill_removed", { name: key }), "var(--yellow)");
          } catch (e: any) { showOp(String(e), "var(--red)"); }
        });
      });
    };

    const renderInstalledSkills = async () => {
      if (!installedListEl) return;
      try {
        const list: Array<{ name: string; description: string; key: string; dir: string }> =
          await invoke("list_installed_skills");
        installedListEl.innerHTML = list.length === 0
          ? `<div class="settings-plugin-empty">${t("settings.skills_none")}</div>`
          : list.map(s => installedSkillRowHtml(s)).join("");
        bindInstalledRowActions();
      } catch {
        installedListEl.innerHTML = `<div class="settings-plugin-empty">${t("settings.load_error")}</div>`;
      }
    };

    // URL 安装 raw SKILL.md
    const installSkillFromUrl = async () => {
      const url = (installUrlInput?.value || "").trim();
      if (!url) { showOp(t("settings.enter_url")); return; }
      try {
        const key = await invoke<string>("install_skill_from_url", { url });
        if (installUrlInput) installUrlInput.value = "";
        await renderInstalledSkills();
        const restarted = await applySkillChange();
        showOp(t("settings.skill_installed_ok", { name: key }) + (restarted ? t("settings.skill_applied") : ""), "var(--green)");
      } catch (e: any) { showOp(String(e), "var(--red)"); }
    };
    if (installUrlBtn) installUrlBtn.addEventListener("click", installSkillFromUrl);
    if (installUrlInput) {
      installUrlInput.addEventListener("keydown", (e: KeyboardEvent) => {
        if (e.key === "Enter") { e.preventDefault(); installSkillFromUrl(); }
      });
    }

    // 新建（粘贴导入 SKILL.md 全文）
    if (newSkillBtn) newSkillBtn.addEventListener("click", () => openSkillEditor("", SKILL_TEMPLATE));

    // 编辑器：保存（覆盖已有 / 导入新建）、取消
    if (editorSave) {
      editorSave.addEventListener("click", async () => {
        const content = editorText?.value || "";
        if (!content.trim()) { showOp(t("settings.skill_editor_empty")); return; }
        try {
          if (skillEditorKey) {
            await invoke("save_skill_file", { key: skillEditorKey, content });
          } else {
            await invoke("import_skill_content", { content });
          }
          closeSkillEditor();
          await renderInstalledSkills();
          const restarted = await applySkillChange();
          showOp(t("settings.saved_ok") + (restarted ? t("settings.skill_applied") : ""), "var(--green)");
        } catch (e: any) { showOp(String(e), "var(--red)"); }
      });
    }
    if (editorCancel) editorCancel.addEventListener("click", closeSkillEditor);

    // buildSkillsPane 已内联渲染初始列表 → 这里绑定其行操作
    bindInstalledRowActions();

    const addSkillSite = () => {
      if (!skillUrlInput) return;
      let url = skillUrlInput.value.trim();
      if (!url) return;
      if (!/^https?:\/\//i.test(url)) url = "https://" + url;
      try { new URL(url); } catch { showSkillMsg(t("settings.skill_invalid_url"), "var(--red)"); return; }
      if (loadSkillSites().some(s => s.url === url) || SKILL_SITES.some(s => s.url === url)) {
        showSkillMsg(t("settings.skill_duplicate"), "var(--yellow)");
        return;
      }
      let name = url;
      try { name = new URL(url).hostname.replace(/^www\./, ""); } catch {}
      const list = loadSkillSites();
      list.push({ name, url });
      saveSkillSites(list);
      skillUrlInput.value = "";
      renderCustomSkillList();
      showSkillMsg(t("settings.skill_added", { name }), "var(--green)");
    };

    if (skillAddBtn) skillAddBtn.addEventListener("click", addSkillSite);
    if (skillUrlInput) {
      skillUrlInput.addEventListener("keydown", (e: KeyboardEvent) => {
        if (e.key === "Enter") { e.preventDefault(); addSkillSite(); }
      });
    }

    // Initial binding for the custom list rendered by buildSkillsPane
    renderCustomSkillList();
  }
}

// ── Plugin definition ────────────────────────────────────────────

/** 放开**设置面板自身**对下拉框的裁剪（打开下拉时用，关闭时还原）。
 *
 *  只动 `.settings-layout` / `.settings-content` 这两个纯裁剪容器 ——
 *  **绝不能碰 `#results-list` / `#results-container`**：前者是真正的滚动容器
 *  （`styles.css` 里 `overflow-y: auto`），把它改成 `visible` 会让它不再是滚动
 *  容器、`scrollTop` 被浏览器归零，整个设置页瞬间跳回顶部 —— 这正是「AI 分页
 *  里一点『搜索服务商』下拉，页面就被拉到最顶端」的根因。
 *
 *  下拉框改为在滚动容器的**可视区内**翻转 + 限高（见 positionDropdown），
 *  因此不再需要、也不允许去放宽滚动容器的裁剪。 */
function setDropdownOverflow(container: HTMLElement, v: boolean) {
  const layout = container.closest(".settings-layout");
  const content = container.closest(".settings-content");
  if (v) {
    layout?.classList.add("sel-open");
    content?.classList.add("sel-open");
  } else {
    layout?.classList.remove("sel-open");
    content?.classList.remove("sel-open");
  }
}

/**
 * Insert (or select) a custom value option in a custom dropdown.
 * The trailing "__custom__" entry is kept so users can keep adding new values.
 */
function commitCustomOption(dd: HTMLElement, val: string, label: HTMLElement) {
  const dropdown = dd.querySelector(".custom-select-dropdown");
  if (!dropdown || !val) return;
  dropdown.querySelectorAll(".custom-select-option").forEach(o => o.classList.remove("selected"));
  // Re-select an existing matching option (e.g. saved custom model) if present
  const existing = Array.from(dropdown.querySelectorAll(".custom-select-option"))
    .find(o => o.getAttribute("data-value") === val);
  if (existing) {
    existing.classList.add("selected");
  } else {
    const customOpt = document.createElement("div");
    customOpt.className = "custom-select-option selected";
    customOpt.setAttribute("data-value", val);
    customOpt.setAttribute("data-tag", "custom");
    customOpt.innerHTML = `${esc(val)}<span class="cs-del" title="×">×</span>`;
    const customEntry = Array.from(dropdown.querySelectorAll(".custom-select-option"))
      .find(o => o.getAttribute("data-value") === "__custom__");
    dropdown.insertBefore(customOpt, customEntry || null);
  }
  label.textContent = val;
  // Persist custom providers/models (点11) so they survive a panel reopen
  try {
    if (dd.id === "settings-provider") {
      const arr: string[] = JSON.parse(localStorage.getItem("lunac-custom-providers") || "[]");
      if (!arr.includes(val)) { arr.push(val); localStorage.setItem("lunac-custom-providers", JSON.stringify(arr)); }
    } else if (dd.id === "settings-model") {
      const prov = (document.querySelector("#settings-provider .custom-select-option.selected")?.getAttribute("data-value")) || "";
      const map = JSON.parse(localStorage.getItem("lunac-custom-models") || "{}");
      const arr: string[] = (map && map[prov]) || [];
      if (!arr.includes(val)) { arr.push(val); map[prov] = arr; localStorage.setItem("lunac-custom-models", JSON.stringify(map)); }
    }
  } catch {}
}

/**
 * Inline custom-model/provider input.
 * WebView2 不支持 window.prompt()，故用下拉内的内联编辑行。
 * 关键：不再用编辑器替换 dropdown 的整体 innerHTML —— 那会导致提交后
 * 原有预设选项全部丢失（「自定义后预设模型消失」）。这里把编辑行追加到
 * 列表末尾，提交/取消都只增删这一行，原有模型（分页）始终保留。
 */
function promptCustomModelInput(
  dd: HTMLElement,
  label: HTMLElement,
  onChange: (value: string) => void,
  initial: string,
  setOverflowVisible: (v: boolean) => void,
) {
  const dropdown = dd.querySelector(".custom-select-dropdown") as HTMLElement | null;
  if (!dropdown) return;
  // 清理可能遗留的旧编辑行
  dropdown.querySelectorAll(".custom-model-edit").forEach(el => el.remove());

  const editRow = document.createElement("div");
  editRow.className = "custom-model-edit";
  editRow.innerHTML = `
    <input type="text" class="custom-model-input" value="${esc(initial)}" placeholder="${t("settings.custom_model")}" autocomplete="off" />
    <button type="button" class="custom-model-ok" title="${t("settings.save")}">✓</button>
  `;
  // 追加到选项列表末尾（不动原有选项 DOM）
  dropdown.appendChild(editRow);
  dd.classList.add("open");
  setOverflowVisible(true);

  const input = editRow.querySelector("input");
  if (!input) return;
  input.focus();
  input.select();
  editRow.scrollIntoView({ block: "nearest" });

  const close = () => {
    editRow.remove(); // 只移除编辑行，保留所有模型选项
    dd.classList.remove("open");
    setOverflowVisible(false);
  };
  const commit = () => {
    const val = input.value.trim();
    if (val) {
      onChange(val); // __custom__/编辑路径在这里把自定义项插入列表并持久化
      label.textContent = val;
    }
    close();
  };
  input.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Enter") { e.preventDefault(); commit(); }
    else if (e.key === "Escape") { close(); }
  });
  input.addEventListener("blur", () => {
    // Delay so Enter/✓ commit() runs before this closes the editor.
    setTimeout(close, 100);
  });
  // ✓ button: keep focus (no blur) and commit
  const okBtn = editRow.querySelector(".custom-model-ok") as HTMLElement | null;
  if (okBtn) {
    okBtn.addEventListener("mousedown", (e: Event) => e.preventDefault());
    okBtn.addEventListener("click", (e: Event) => { e.preventDefault(); e.stopPropagation(); commit(); });
  }
}

export const settingsPlugin: Plugin = {
  id: "settings",
  name: "Settings",
  description: "Configure Lunac — hotkey, AI provider, plugins",
  keywords: ["settings", "设置", "config", "配置", "hotkey", "热键", "api", "key", "model", "plugin", "插件"],
  icon: "⚙️",

  async execute(_input: string) {
    let hotkey = "Ctrl+Alt+Space";
    let autoStart = false;
    // 未配置 provider 时由模型名反推其所属供应商，避免“OpenAI 供应商却带
    // DeepSeek 模型”等跨供应商脏数据；baseUrl 默认填该供应商文档地址。
    let provider = "";
    let baseUrl = "";
    let model = "";
    let apiKey = "";
    let searchProvider = "";
    let searchKey = "";
    // 当前模型是否支持图片输入（A8）：跟着 ai.json 走，由用户在 AI 面板显式打开。
    let vision = false;
    // 权限 hooks（A9）：开关 + 语法错误（都取自 config\hooks.json 这一份真相）。
    let hooksEnabled = false;
    let hooksError = "";

    try {
      hotkey = await invoke<string>("get_hotkey_combo");
      // Sync JS fallback flag with loaded config
      (window as any).__lunac_hotkey_is_alt_space = hotkey === "Alt+Space";
      // 状态栏提示同步为实际热键
      (window as any).__lunac_refresh_hotkey_hint?.(hotkey);
    } catch {}

    try {
      // 只取开关状态；实际生效机制（task / run）由后端写进落盘日志，不进 UI
      const info = await invoke<{ enabled: boolean }>("get_auto_start_info");
      autoStart = info.enabled;
    } catch {}

    try {
      const aiCfg = await invoke<{ provider: string; base_url: string; model: string; api_key: string; search_provider: string; search_key: string; vision?: boolean }>("get_ai_config");
      provider = aiCfg.provider || "";
      baseUrl = aiCfg.base_url || "";
      model = aiCfg.model || "";
      apiKey = aiCfg.api_key || "";
      searchProvider = aiCfg.search_provider || "";
      searchKey = aiCfg.search_key || "";
      // 图片输入开关（A8）：**必须回读**，否则面板每次都显示「关」，
      // 而用户一按保存就把这个开着的功能静默关掉了。
      vision = !!aiCfg.vision;
    } catch {}

    // 权限 hooks（A9）：开关状态就是 config\hooks.json 的 `enabled` 字段
    // （文件不存在 = 没配 = 关）。语法错误一并取回，在面板上直接报出来。
    try {
      const hk = await invoke<{ enabled?: boolean; error?: string | null }>("get_hooks_config");
      hooksEnabled = !!hk.enabled;
      hooksError = hk.error || "";
    } catch {}

    // 未配置供应商（环境缺 AI_PROVIDER）：用模型名反推；仍无则回退 openai
    if (!provider) provider = inferProviderForModel(model, "openai");
    const preset = PROVIDER_PRESETS[provider];
    if (!baseUrl && preset) baseUrl = preset.default_url;
    if (!model && preset) model = preset.default_model;

    const generalPane = buildGeneralPane(hotkey, autoStart);
    const appearancePane = await buildAppearancePane();
    const aiPane = await buildAIPane(provider, baseUrl, model, apiKey, searchProvider, searchKey, vision, hooksEnabled, hooksError);
    const pluginsPane = await buildPluginsPane();
    const searchPane = await buildSearchPane();

    const html = `
      <div class="settings-layout">
        <div class="settings-sidebar">
          <div class="settings-sidebar-item" data-cat="general">
            <span>${t("settings.sidebar_general")}</span>
          </div>
          <div class="settings-sidebar-item" data-cat="appearance">
            <span>${t("settings.sidebar_appearance")}</span>
          </div>
          <div class="settings-sidebar-item" data-cat="ai">
            <span>${t("settings.sidebar_ai")}</span>
          </div>
          <div class="settings-sidebar-item" data-cat="search">
            <span>${t("settings.sidebar_search")}</span>
          </div>
          <div class="settings-sidebar-item" data-cat="plugins">
            <span>${t("settings.sidebar_plugins")}</span>
          </div>
        </div>
        <div class="settings-content">
          ${generalPane}
          ${appearancePane}
          ${aiPane}
          ${searchPane}
          ${pluginsPane}
        </div>
      </div>`;

    return { type: "html", content: html };
  },
};
