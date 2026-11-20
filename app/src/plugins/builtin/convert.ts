// ── 文件转换插件（2026-10-24 重构：独立窗口 / 四类转换 / 多文件队列）─────
//
// 引擎：本机 ffmpeg（视频 / 音频 / 动图）+ 本机 LibreOffice（PDF → Word/PPT/Excel/图片）。
// 都在宿主 `src-tauri/src/convert.rs` 里跑 —— 前端没有执行外部进程的能力。
//
// 界面：左侧「转换类型」竖排 4 页签 → 输入框（300×250）→ 中间图标/进度 → 输出框（300×250）。
//   · 输入框：点击整块弹文件选择；也支持把文件拖进窗口（只收当前页签允许的类型）。
//   · 多个文件用泡泡框列出：点泡泡打开文件、× 取消/移除；输入框右上角一键清空。
//   · 转换中：中间图标换成条形进度 + 上方百分比。
//   · 右上角：保存目录 + 设置按钮；输出泡泡点击用系统默认程序打开。
//
// 纪律：进度事件 `convert-progress` 只反映**当前这一个**文件（队列串行）；
//       面板关闭时 `detach()` 撤销监听，避免叠加。

import type { Plugin, PluginResult } from "../registry";
import { t } from "../../i18n.js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";

type TabKey = "pdf" | "video" | "extract" | "audio";
type ItemStatus = "queued" | "running" | "done" | "error";

interface EngineDto {
  found: boolean;
  path: string;
  version: string;
  pdf_found: boolean;
  pdf_version: string;
}
interface MediaDto {
  path: string;
  name: string;
  kind: "image" | "audio" | "video" | "pdf" | "unknown";
  ext: string;
  size: number;
  duration: number;
  targets: string[];
}
interface ConvertResult {
  output: string;
  size: number;
  elapsed_ms: number;
}
interface Item {
  id: string;
  path: string;
  name: string;
  status: ItemStatus;
  /** 入队时所在的分页 —— 队列跑起来时**不跟着当前页签变**（否则切页签会把 PDF 条目送进 ffmpeg）。 */
  tab: TabKey;
  /** 入队时选定的目标格式。 */
  target: string;
  output?: string;
  err?: string;
}

const VIDEO_EXTS = ["mp4", "mkv", "webm", "avi", "mov", "wmv", "flv", "m4v", "mpg", "mpeg", "ts", "3gp", "rmvb"];
const AUDIO_EXTS = ["mp3", "wav", "flac", "m4a", "aac", "ogg", "opus", "wma", "aiff", "ape", "amr"];

const TABS: { key: TabKey; label: string; accept: string[]; targets: string[] }[] = [
  { key: "pdf", label: "convert.tab_pdf", accept: ["pdf"], targets: ["docx", "pptx", "xlsx", "png"] },
  { key: "video", label: "convert.tab_video", accept: VIDEO_EXTS, targets: ["mp4", "mkv", "webm", "avi", "mov", "gif"] },
  { key: "extract", label: "convert.tab_extract", accept: VIDEO_EXTS, targets: ["mp3", "wav", "flac", "m4a", "aac", "ogg"] },
  { key: "audio", label: "convert.tab_audio", accept: AUDIO_EXTS, targets: ["mp3", "wav", "flac", "m4a", "aac", "ogg"] },
];

const SVG = {
  file: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>`,
  folder: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>`,
  swap: `<svg width="34" height="34" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h13"/><polyline points="14 4 17 7 14 10"/><path d="M20 17H7"/><polyline points="10 14 7 17 10 20"/></svg>`,
  x: `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><line x1="5" y1="5" x2="19" y2="19"/><line x1="19" y1="5" x2="5" y2="19"/></svg>`,
};

// ── 状态 ──────────────────────────────────────────────────────────
let engine: EngineDto | null = null;
let tab: TabKey = "video";
let targetByTab: Record<TabKey, string> = { pdf: "docx", video: "mp4", extract: "mp3", audio: "mp3" };
let items: Item[] = [];
let outDir = "";
let running = false;
let progress = -1;

let view: HTMLElement | null = null;
let gen = 0;
let unlistenProgress: (() => void) | null = null;
let unDragDrop: (() => void) | null = null;
let clearArmed = false;

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}
function extOf(path: string): string {
  const m = path.match(/\.([a-zA-Z0-9]+)$/);
  return m ? m[1].toLowerCase() : "";
}
function baseName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}
function tabDef(): (typeof TABS)[number] {
  return TABS.find((x) => x.key === tab) ?? TABS[0];
}
function errText(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e);
  if (raw.includes("ERR_NO_FILE")) return t("convert.err_no_file");
  if (raw.includes("ERR_NO_ENGINE")) return t("convert.err_no_engine");
  if (raw.includes("ERR_NO_PDF_ENGINE")) return t("convert.err_no_pdf_engine");
  if (raw.includes("ERR_OUT_DIR")) return t("convert.err_out_dir");
  return raw;
}
function setMsg(s: string) {
  const el = view?.querySelector<HTMLElement>("#convert-msg");
  if (el && el.textContent !== s) el.textContent = s;
}

// ── 视图 ──────────────────────────────────────────────────────────
function shellHtml(): string {
  const tabs = TABS.map(
    (x) => `<button type="button" class="convert-tab" data-tab="${x.key}">${esc(t(x.label))}</button>`,
  ).join("");
  return `
  <div class="convert-win">
    <div class="convert-top">
      <span class="convert-engine" id="convert-engine">${t("convert.engine_checking")}</span>
      <span class="convert-outdir" id="convert-outdir"></span>
      <button type="button" class="convert-icon-btn" id="convert-outdir-btn" title="${esc(t("convert.set_out_dir"))}">${SVG.folder}</button>
    </div>
    <div class="convert-main">
      <div class="convert-tabs" id="convert-tabs">${tabs}</div>
      <div class="convert-col">
        <div class="convert-targets" id="convert-targets"></div>
        <div class="convert-drop" id="convert-drop">
          <div class="convert-drop-hint" id="convert-drop-hint"></div>
          <button type="button" class="convert-clear" id="convert-clear" title="${esc(t("convert.clear"))}">${esc(t("convert.clear"))}</button>
        </div>
        <div class="convert-bubbles" id="convert-in"></div>
      </div>
      <div class="convert-mid">
        <div class="convert-pct" id="convert-pct"></div>
        <div class="convert-swap" id="convert-swap">${SVG.swap}</div>
        <div class="convert-bar"><div class="convert-bar-fill" id="convert-bar-fill"></div></div>
      </div>
      <div class="convert-col">
        <div class="convert-targets convert-targets-ghost"></div>
        <div class="convert-drop convert-drop-out" id="convert-out-drop">
          <div class="convert-drop-hint"></div>
        </div>
        <div class="convert-bubbles" id="convert-out"></div>
      </div>
    </div>
    <div class="convert-msg" id="convert-msg"></div>
  </div>`;
}

function render() {
  if (!view) return;
  const d = tabDef();
  // 页签
  view.querySelectorAll<HTMLElement>(".convert-tab").forEach((b) => {
    const k = b.dataset.tab as TabKey;
    b.classList.toggle("on", k === tab);
    b.textContent = t(TABS.find((x) => x.key === k)!.label);
  });
  // 目标格式
  const targets = view.querySelector<HTMLElement>("#convert-targets");
  if (targets) {
    const disabled = tab === "pdf" && !engine?.pdf_found;
    targets.innerHTML = d.targets
      .map((tg) => `<button type="button" class="convert-chip${tg === targetByTab[tab] ? " on" : ""}" data-target="${tg}"${disabled ? " disabled" : ""}>${tg}</button>`)
      .join("");
  }
  // 引擎 / 保存目录
  const eng = view.querySelector<HTMLElement>("#convert-engine");
  if (eng) {
    // 顶栏只报**当前分页真正要用的那台引擎**：PDF 走 LibreOffice，其余走 ffmpeg。
    const missing = tab === "pdf" ? engine && !engine.pdf_found : engine && !engine.found;
    if (!engine) eng.textContent = t("convert.engine_checking");
    else if (tab === "pdf") {
      eng.textContent = engine.pdf_found
        ? (engine.pdf_version || "LibreOffice")
        : t("convert.err_no_pdf_engine");
    } else if (!engine.found) {
      eng.textContent = t("convert.engine_missing");
    } else {
      eng.textContent = `${t("convert.engine")} ${engine.version || ""}`.trim();
    }
    eng.classList.toggle("warn", !!missing);
  }
  const od = view.querySelector<HTMLElement>("#convert-outdir");
  if (od) od.textContent = outDir || t("convert.out_dir_default");
  // 输入提示（右侧输出框刻意留空 —— 用户明确要求不写提示文本）
  const hint = view.querySelector<HTMLElement>("#convert-drop-hint");
  if (hint) {
    if (!engine) hint.textContent = t("convert.engine_checking");
    else if (tab === "pdf" && !engine.pdf_found) hint.textContent = t("convert.pdf_missing_hint");
    else if (tab !== "pdf" && !engine.found) hint.textContent = t("convert.engine_missing_hint");
    else hint.textContent = t("convert.drop_hint");
  }
  renderBubbles();
  // 列表空时「清空」没有意义，收起来（否则一个空框右上角挂着按钮很奇怪）
  const clearBtn = view.querySelector<HTMLElement>("#convert-clear");
  if (clearBtn) clearBtn.classList.toggle("hidden", items.length === 0);
  renderProgress();
}

function bubbleHtml(it: Item, side: "in" | "out"): string {
  const name = side === "in" ? it.name : baseName(it.output || "");
  const title = it.err ? `${name}\n${it.err}` : (side === "in" ? it.path : it.output || "");
  const cls = `convert-bubble ${it.status}`;
  const del = side === "in"
    ? `<button type="button" class="convert-bubble-x" data-del="${esc(it.id)}" title="${esc(t("convert.remove"))}">${SVG.x}</button>`
    : "";
  return `<span class="${cls}" data-open="${esc(side === "in" ? it.path : it.output || "")}" title="${esc(title)}"><span class="convert-bubble-name">${esc(name)}</span>${del}</span>`;
}

function renderBubbles() {
  const inputBox = view?.querySelector<HTMLElement>("#convert-in");
  const outBox = view?.querySelector<HTMLElement>("#convert-out");
  if (inputBox) inputBox.innerHTML = items.map((it) => bubbleHtml(it, "in")).join("");
  if (outBox) {
    outBox.innerHTML = items
      .filter((it) => it.status === "done")
      .map((it) => bubbleHtml(it, "out"))
      .join("");
  }
}

function renderProgress() {
  if (!view) return;
  const swap = view.querySelector<HTMLElement>("#convert-swap");
  const bar = view.querySelector<HTMLElement>("#convert-bar-fill");
  const pct = view.querySelector<HTMLElement>("#convert-pct");
  const on = running || progress >= 0;
  swap?.classList.toggle("hidden", on);
  view.querySelector<HTMLElement>(".convert-bar")?.classList.toggle("hidden", !on);
  if (!on) {
    if (pct) pct.textContent = "";
    if (bar) bar.style.width = "0%";
    return;
  }
  const p = progress < 0 ? 0 : Math.min(100, progress);
  if (bar) {
    bar.classList.toggle("indeterminate", progress < 0);
    bar.style.width = progress < 0 ? "40%" : `${p}%`;
  }
  if (pct) pct.textContent = progress < 0 ? "" : `${Math.round(p)}%`;
}

// ── 队列 ──────────────────────────────────────────────────────────

async function addPaths(paths: string[]) {
  if (!paths.length) return;
  const d = tabDef();
  const accepted: string[] = [];
  for (const p of paths) {
    const ext = extOf(p);
    if (!d.accept.includes(ext)) {
      setMsg(t("convert.err_type", { ext: ext || "?", tab: t(d.label) }));
      continue;
    }
    if (items.some((it) => it.path === p)) continue;
    accepted.push(p);
  }
  if (!accepted.length) return;
  setMsg("");
  for (const p of accepted) {
    items.push({
      id: `${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
      path: p,
      name: baseName(p),
      status: "queued",
      tab,
      target: targetByTab[tab],
    });
  }
  render();
  void runQueue();
}

async function runQueue() {
  if (running) return;
  const next = items.find((it) => it.status === "queued");
  if (!next) {
    running = false;
    progress = -1;
    renderProgress();
    return;
  }
  // 每一条按**入队时**的分页与目标格式跑：切页签不会改变已在队列里的事。
  if (next.tab === "pdf" && !engine?.pdf_found) {
    next.status = "error";
    next.err = t("convert.err_no_pdf_engine");
    render();
    void runQueue();
    return;
  }
  if (next.tab !== "pdf" && !engine?.found) {
    next.status = "error";
    next.err = t("convert.err_no_engine");
    render();
    void runQueue();
    return;
  }
  running = true;
  progress = 0;
  next.status = "running";
  render();
  try {
    const r = await invoke<ConvertResult>("convert_run", {
      input: next.path,
      target: next.target,
      outputDir: outDir || null,
    });
    next.status = "done";
    next.output = r.output;
    progress = 100;
  } catch (e) {
    next.status = "error";
    next.err = errText(e);
    progress = -1;
    setMsg(next.err);
  }
  running = false;
  render();
  void runQueue();
}

// ── 挂载 ──────────────────────────────────────────────────────────

export async function attachConvertListeners(root: HTMLElement) {
  const v = root.querySelector<HTMLElement>(".convert-win");
  if (!v) return;
  view = v;
  const myGen = ++gen;
  clearArmed = false;
  try {
    outDir = localStorage.getItem("lunac-convert-outdir") || "";
  } catch {
    outDir = "";
  }

  if (unlistenProgress) { unlistenProgress(); unlistenProgress = null; }
  unlistenProgress = await listen<{ percent: number }>("convert-progress", (ev) => {
    if (myGen !== gen || !v.isConnected) return;
    progress = Number(ev.payload?.percent ?? -1);
    renderProgress();
  });
  // 拖放：Tauri 的窗口级事件（前端拿不到 File.path）。只收当前页签允许的类型。
  if (unDragDrop) { unDragDrop(); unDragDrop = null; }
  try {
    unDragDrop = await getCurrentWebview().onDragDropEvent((ev) => {
      if (myGen !== gen || !v.isConnected) return;
      if (ev.payload.type === "drop") void addPaths(ev.payload.paths);
    });
  } catch { /* 拖放不可用不该挡住其它功能 */ }

  try {
    engine = await invoke<EngineDto>("convert_engine_status");
  } catch {
    engine = { found: false, path: "", version: "", pdf_found: false, pdf_version: "" };
  }
  if (myGen !== gen) return;
  render();

  // 预设文件（从搜索结果 / 附件进来）
  const preset = (window as any).__lunac_convert_file as string | undefined;
  if (preset) {
    delete (window as any).__lunac_convert_file;
    await addPaths([preset]);
  }

  // 页签
  v.querySelector<HTMLElement>("#convert-tabs")?.addEventListener("click", (ev) => {
    const b = (ev.target as HTMLElement).closest<HTMLElement>("[data-tab]");
    const k = b?.dataset.tab as TabKey | undefined;
    if (!k || k === tab) return;
    tab = k;
    setMsg("");
    render();
    void runQueue();
  });
  // 目标格式
  v.querySelector<HTMLElement>("#convert-targets")?.addEventListener("click", (ev) => {
    const b = (ev.target as HTMLElement).closest<HTMLElement>("[data-target]");
    const tg = b?.dataset.target;
    if (!tg) return;
    targetByTab[tab] = tg;
    // 还没开跑的同页签条目跟着改目标 —— 用户换格式的意图就是「都按这个转」。
    for (const it of items) {
      if (it.status === "queued" && it.tab === tab && it.target !== tg) it.target = tg;
    }
    render();
  });
  // 输入框：点击整块 = 选文件
  v.querySelector<HTMLElement>("#convert-drop")?.addEventListener("click", async () => {
    try {
      const picked = await openDialog({ multiple: true, directory: false, title: t("convert.pick") });
      const list = Array.isArray(picked) ? picked : picked ? [picked] : [];
      await addPaths(list as string[]);
    } catch (e) {
      setMsg(errText(e));
    }
  });
  // 一键清空（转换中的那一条先取消）
  v.querySelector<HTMLElement>("#convert-clear")?.addEventListener("click", async (ev) => {
    ev.stopPropagation();
    if (!clearArmed) {
      clearArmed = true;
      setMsg(t("convert.clear_confirm"));
      return;
    }
    clearArmed = false;
    if (items.some((it) => it.status === "running")) {
      await invoke("convert_cancel").catch(() => {});
    }
    items = [];
    running = false;
    progress = -1;
    render();
    setMsg("");
  });
  // 泡泡：点击打开；× 取消/移除
  v.querySelector<HTMLElement>(".convert-main")?.addEventListener("click", async (ev) => {
    const del = (ev.target as HTMLElement).closest<HTMLElement>("[data-del]");
    if (del) {
      ev.stopPropagation();
      const id = del.dataset.del!;
      const it = items.find((x) => x.id === id);
      if (it?.status === "running") await invoke("convert_cancel").catch(() => {});
      items = items.filter((x) => x.id !== id);
      render();
      return;
    }
    const b = (ev.target as HTMLElement).closest<HTMLElement>("[data-open]");
    const p = b?.dataset.open;
    if (p) void invoke("open_path", { path: p }).catch(() => {});
  });
  // 保存目录
  v.querySelector<HTMLElement>("#convert-outdir-btn")?.addEventListener("click", async () => {
    try {
      const picked = await openDialog({ directory: true, multiple: false, title: t("convert.set_out_dir") });
      if (typeof picked === "string" && picked) {
        outDir = picked;
        try { localStorage.setItem("lunac-convert-outdir", picked); } catch { /* ignore */ }
        render();
      }
    } catch (e) {
      setMsg(errText(e));
    }
  });
}

/** 供 main.ts 的 closePluginView 调用；另有 `isConnected` 判断兜底。 */
export function stopConvertWatch() {
  gen++;
  if (unlistenProgress) { unlistenProgress(); unlistenProgress = null; }
  if (unDragDrop) { unDragDrop(); unDragDrop = null; }
  view = null;
  running = false;
  progress = -1;
}
(window as any).__lunac_convert_stop = stopConvertWatch;

// ── Plugin 定义 ───────────────────────────────────────────────────

export const convertPlugin: Plugin = {
  id: "convert",
  name: "文件转换",
  keywords: ["转换", "格式转换", "转格式", "convert", "格式", "转码", "提取音频", "图片转换", "视频转换", "音频转换", "pdf"],
  description: "PDF / 视频 / 音频格式转换（ffmpeg + 本机 LibreOffice）",
  icon: "🔄",
  badge: "convert",
  // 独立窗口（2026-10-24 用户口径）；主窗口打开时直接开插件窗，不留内嵌面板。
  permissions: ["window.float"],

  async execute(input: string): Promise<PluginResult> {
    const trimmed = input.trim();
    if (/^[a-zA-Z]:[\\/]/.test(trimmed)) {
      try {
        if (await invoke<boolean>("check_file_exists", { path: trimmed })) {
          (window as any).__lunac_convert_file = trimmed;
        }
      } catch { /* 不存在就当普通查询处理 */ }
    }
    return { type: "html", content: shellHtml() };
  },
};

export { attachConvertListeners as attach, stopConvertWatch as detach };
export default convertPlugin;
