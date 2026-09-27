// ── 文件转换插件（图片 / 音频 / 视频）────────────────────────────
// 2026-09-27。用户选定范围：图片格式互转 + 音频格式互转 + 视频格式互转。
//
// 引擎 = 本机 ffmpeg（`src-tauri/src/convert.rs`）。**转换必须由宿主做**：前端插件
// 没有执行外部进程的能力（与「联网必须走宿主」同一条纪律，code-rules 预检 #36）。
//
// 界面（内嵌插件视图 360px 高）：
//   [引擎状态：ffmpeg 8.1.1]                    [选择文件]
//   [源文件：名.mp4 · 视频 · 3.0s · 17.0 KB]
//   转换为  [mp4][mkv][webm][avi][mov][gif][mp3][wav]…
//   [开始转换]  [━━━━━━━━──────] 62%
//   完成：out.webm · 19.0 KB    [打开文件] [打开所在目录]
//
// 纪律：
//   · 进度走 `convert-progress` 事件（宿主每 ~0.5s 推一次）；`percent < 0` = 时长
//     未知（图片没有时长）⇒ 进度条切成不确定态，**不显示假百分比**。
//   · 事件监听模块级去重（重开面板不能叠监听器，code-rules §3.3）；
//     `main.ts` 的 closePluginView 会调 `window.__lunac_convert_stop` 收尾。

import type { Plugin, PluginResult } from "../registry";
import { t } from "../../i18n.js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

interface EngineDto {
  found: boolean;
  path: string;
  version: string;
}
interface MediaDto {
  path: string;
  name: string;
  kind: "image" | "audio" | "video" | "unknown";
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

const SVG = {
  file: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>`,
  play: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polygon points="6 4 20 12 6 20 6 4"/></svg>`,
  folder: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>`,
};

// ── 状态 ──────────────────────────────────────────────────────────
let engine: EngineDto | null = null;
let media: MediaDto | null = null;
let target = "";
let converting = false;
let result: ConvertResult | null = null;
let progress = 0;

let unlistenProgress: (() => void) | null = null;
let currentView: HTMLElement | null = null;
let gen = 0;

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function formatSize(n: number): string {
  if (!isFinite(n) || n <= 0) return "0 B";
  if (n < 1024) return `${n} B`;
  if (n < 1048576) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1073741824) return `${(n / 1048576).toFixed(1)} MB`;
  return `${(n / 1073741824).toFixed(2)} GB`;
}

function formatDuration(sec: number): string {
  if (!isFinite(sec) || sec <= 0) return "";
  const total = Math.round(sec);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return m > 0 ? `${m}:${String(s).padStart(2, "0")}` : `${s}s`;
}

function kindLabel(kind: string): string {
  switch (kind) {
    case "image": return t("convert.kind_image");
    case "audio": return t("convert.kind_audio");
    case "video": return t("convert.kind_video");
    default: return t("convert.kind_unknown");
  }
}

function errText(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e);
  if (raw.includes("ERR_NO_FILE")) return t("convert.err_no_file");
  if (raw.includes("ERR_NO_ENGINE")) return t("convert.err_no_engine");
  return raw;
}

// ── 视图 ──────────────────────────────────────────────────────────

function shellHtml(): string {
  return `
  <div class="convert-root">
    <div class="convert-head">
      <span class="convert-engine" id="convert-engine">${t("convert.engine_checking")}</span>
      <button class="convert-btn" id="convert-pick">${SVG.file} ${t("convert.pick")}</button>
    </div>

    <div class="convert-source" id="convert-source">${t("convert.pick_hint")}</div>

    <div class="convert-target-wrap hidden" id="convert-target-wrap">
      <div class="convert-label">${t("convert.target")}</div>
      <div class="convert-targets" id="convert-targets"></div>
    </div>

    <div class="convert-actions">
      <button class="convert-btn convert-primary" id="convert-start" disabled>${t("convert.start")}</button>
      <span class="convert-status" id="convert-status"></span>
    </div>

    <div class="convert-progress hidden" id="convert-progress-wrap">
      <div class="convert-progress-bar"><div class="convert-progress-fill" id="convert-progress-fill"></div></div>
      <span class="convert-percent" id="convert-percent"></span>
    </div>

    <div class="convert-done hidden" id="convert-done">
      <div class="convert-out" id="convert-out"></div>
      <div class="convert-done-actions">
        <button class="convert-btn" id="convert-open-file">${SVG.play} ${t("convert.open_file")}</button>
        <button class="convert-btn" id="convert-reveal">${SVG.folder} ${t("convert.reveal")}</button>
      </div>
    </div>
  </div>`;
}

function setText(el: HTMLElement | null, text: string) {
  if (el && el.textContent !== text) el.textContent = text;
}
function setHtml(el: HTMLElement | null, html: string) {
  if (el && el.innerHTML !== html) el.innerHTML = html;
}
function show(el: HTMLElement | null, on: boolean) {
  el?.classList.toggle("hidden", !on);
}

function renderEngine(view: HTMLElement) {
  const el = view.querySelector<HTMLElement>("#convert-engine");
  if (!engine) return;
  if (!engine.found) {
    setText(el, t("convert.engine_missing"));
    el?.classList.add("convert-warn");
    // 引擎缺失时把装法写在下面（坏状态必须可见 + 可自救，与 OCR 引擎缺失同一条纪律）
    const src = view.querySelector<HTMLElement>("#convert-source");
    if (src) src.innerHTML = `<span class="convert-warn">${t("convert.engine_missing_hint")}</span>`;
    view.querySelector<HTMLButtonElement>("#convert-pick")?.setAttribute("disabled", "true");
  } else {
    setText(el, engine.version ? `${t("convert.engine")} ${engine.version}` : t("convert.engine"));
  }
}

function renderSource(view: HTMLElement) {
  const box = view.querySelector<HTMLElement>("#convert-source");
  const wrap = view.querySelector<HTMLElement>("#convert-target-wrap");
  const start = view.querySelector<HTMLButtonElement>("#convert-start");
  if (!box || !wrap || !start) return;

  if (!media) {
    setHtml(box, t("convert.pick_hint"));
    show(wrap, false);
    start.disabled = true;
    return;
  }

  const bits = [kindLabel(media.kind), formatSize(media.size)];
  const dur = formatDuration(media.duration);
  if (dur) bits.push(dur);
  if (media.kind === "unknown" || media.targets.length === 0) {
    setHtml(
      box,
      `<div class="convert-file">${esc(media.name)}</div><div class="convert-sub convert-warn">${t("convert.unsupported")}</div>`,
    );
    show(wrap, false);
    start.disabled = true;
    return;
  }
  setHtml(
    box,
    `<div class="convert-file">${esc(media.name)}</div><div class="convert-sub">${esc(bits.join(" · "))}</div>`,
  );

  // 目标胶囊：只在该换的时候重建（重建会丢 hover/焦点）
  setHtml(
    view.querySelector("#convert-targets"),
    media.targets
      .map(tg => `<button class="convert-chip${tg === target ? " active" : ""}" data-ext="${esc(tg)}">${esc(tg)}</button>`)
      .join(""),
  );
  show(wrap, true);
  start.disabled = converting || !target;
}

function renderProgress(view: HTMLElement) {
  const wrap = view.querySelector<HTMLElement>("#convert-progress-wrap");
  const fill = view.querySelector<HTMLElement>("#convert-progress-fill");
  const pct = view.querySelector<HTMLElement>("#convert-percent");
  show(wrap, converting || progress > 0);
  if (!converting && progress <= 0) return;
  if (progress < 0) {
    // 时长未知（图片 / ffprobe 探不到）⇒ 不确定态，不编一个百分比给用户
    fill?.classList.add("indeterminate");
    if (fill) fill.style.width = "";
    setText(pct, "");
  } else {
    fill?.classList.remove("indeterminate");
    if (fill) fill.style.width = `${Math.min(100, progress)}%`;
    setText(pct, `${Math.round(progress)}%`);
  }
}

function renderDone(view: HTMLElement) {
  const box = view.querySelector<HTMLElement>("#convert-done");
  const out = view.querySelector<HTMLElement>("#convert-out");
  show(box, !!result);
  if (!result || !out) return;
  const name = result.output.split(/[\\/]/).pop() || result.output;
  setHtml(out, `${esc(name)} · ${formatSize(result.size)} · ${(result.elapsed_ms / 1000).toFixed(1)}s`);
}

// ── 交互 ──────────────────────────────────────────────────────────

async function pickFile(view: HTMLElement, path: string) {
  result = null;
  progress = 0;
  renderDone(view);
  renderProgress(view);
  setText(view.querySelector("#convert-status"), "");
  try {
    const info = await invoke<MediaDto>("convert_probe", { path });
    media = info;
    target = info.targets[0] ?? "";
    renderSource(view);
  } catch (e) {
    media = null;
    target = "";
    renderSource(view);
    setText(view.querySelector("#convert-status"), errText(e));
  }
}

export async function attachConvertListeners(root: HTMLElement) {
  const view = root.querySelector<HTMLElement>(".convert-root");
  if (!view) return;
  currentView = view;
  const myGen = ++gen;
  const q = <T extends HTMLElement>(sel: string) => view.querySelector<T>(sel);
  const status = (s: string) => setText(q("#convert-status"), s);

  // 进度监听：先撤旧的（重开面板不能叠加，code-rules §3.3）
  if (unlistenProgress) { unlistenProgress(); unlistenProgress = null; }
  unlistenProgress = await listen<{ percent: number; total_ms: number }>("convert-progress", (ev) => {
    if (myGen !== gen || !view.isConnected) return;
    progress = ev.payload.percent;
    renderProgress(view);
  });

  // 引擎状态（缺失时把选择按钮也禁掉，别让用户白点）
  try {
    engine = await invoke<EngineDto>("convert_engine_status");
  } catch {
    engine = { found: false, path: "", version: "" };
  }
  if (myGen !== gen) return;
  renderEngine(view);

  // ① 从搜索结果/附件进来的文件（main.ts 会预置这个全局）
  const preset = (window as any).__lunac_convert_file as string | undefined;
  if (preset) {
    delete (window as any).__lunac_convert_file;
    await pickFile(view, preset);
  }

  q<HTMLElement>("#convert-pick")?.addEventListener("click", async () => {
    try {
      const picked = await openDialog({
        multiple: false,
        directory: false,
        title: t("convert.pick"),
      });
      const path = Array.isArray(picked) ? picked[0] : picked;
      if (typeof path === "string" && path) await pickFile(view, path);
    } catch (e) {
      status(errText(e));
    }
  });

  // 目标格式：事件委托（胶囊每次重建都会换节点）
  q<HTMLElement>("#convert-targets")?.addEventListener("click", (ev) => {
    const chip = (ev.target as HTMLElement).closest<HTMLElement>(".convert-chip");
    if (!chip) return;
    target = chip.dataset.ext || "";
    renderSource(view);
  });

  q<HTMLElement>("#convert-start")?.addEventListener("click", async () => {
    if (!media || !target || converting) return;
    converting = true;
    result = null;
    progress = 0;
    renderDone(view);
    renderSource(view);
    status(t("convert.converting"));
    renderProgress(view);
    try {
      const r = await invoke<ConvertResult>("convert_run", { input: media.path, target });
      if (myGen !== gen) return;
      result = r;
      progress = 100;
      status(t("convert.done"));
      renderDone(view);
      renderProgress(view);
    } catch (e) {
      if (myGen !== gen) return;
      progress = 0;
      status(errText(e));
      renderProgress(view);
    } finally {
      converting = false;
      if (myGen === gen) renderSource(view);
    }
  });

  q<HTMLElement>("#convert-open-file")?.addEventListener("click", () => {
    if (result) void invoke("open_path", { path: result.output }).catch(() => {});
  });
  q<HTMLElement>("#convert-reveal")?.addEventListener("click", () => {
    if (result) void invoke("reveal_in_explorer", { path: result.output }).catch(() => {});
  });

  renderSource(view);
  renderProgress(view);
  renderDone(view);
}

/** 供 main.ts 的 closePluginView 调用；另有 `isConnected` 判断兜底。 */
export function stopConvertWatch() {
  gen++;
  if (unlistenProgress) { unlistenProgress(); unlistenProgress = null; }
}

// 懒加载钩子（与 music 同一套：没打开过插件时 main.ts 用 `?.()` 调不到，也不会报错）
(window as any).__lunac_convert_stop = stopConvertWatch;

// ── Plugin 定义 ───────────────────────────────────────────────────

export const convertPlugin: Plugin = {
  id: "convert",
  name: "文件转换",
  keywords: ["转换", "格式转换", "转格式", "convert", "格式", "转码", "提取音频", "图片转换", "视频转换", "音频转换"],
  description: "图片 / 音频 / 视频格式互转 (Image / Audio / Video converter · ffmpeg)",
  icon: "🔄",
  badge: "convert",

  async execute(input: string): Promise<PluginResult> {
    // 搜索框里直接是一个存在的绝对路径时，直接预置为源文件
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
