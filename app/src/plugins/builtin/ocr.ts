// ── OCR 识别插件 ────────────────────────────────────────────────
// 使用 PaddleOCR-json (PP-OCRv4 模型)，中文精度远超 Windows OCR。
// PaddleOCR-json 是 Umi-OCR (42k+ GitHub stars) 内使用的 C++ OCR 引擎。
// 离线运行、零 CDN 依赖、支持中/英/日/韩/俄等多语言。
//
// 界面：进入 OCR 插件自动 detach 为独立窗口
//   ┌────────────────────┬──────────────────────┐
//   │  图片预览（左半）   │  文字编辑区（右半）    │
//   │  [识别剪贴板]      │  [复制结果]           │
//   └────────────────────┴──────────────────────┘
// 按钮/状态行/占位提示**一律纯文字，不带 emoji 图标**（2026-09-19 批 8，
// 用户明确要求；判据与例外见 docs/icon-style.md §4）。
//
// 工作流程：
//   文件路径 → invoke("run_paddle_ocr") → PaddleOCR-json 子进程 → JSON → 文本
//   剪贴板  → invoke("save_temp_image") 写临时 → invoke("run_paddle_ocr")

import type { Plugin, PluginResult } from "../registry";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { t } from "../../i18n.js";

// ── 引擎部署（按需下载）───────────────────────────────────────────
// PaddleOCR-json 引擎体积大（解压后约 300MB），不随发行包分发（见 .gitignore）。
// 运行时若缺失，由前端触发从 GitHub Release 下载到 `<exe 根>\paddle-ocr`。

/** 触发引擎下载安装。进度经 `ocr-engine-progress`/`ready`/`error` 事件回传。
 *  返回 Promise<boolean>：true = 安装成功。供 OCR 面板与设置面板共用。 */
export function installOcrEngine(
  onProgress?: (info: { percent: number; mb: number }) => void,
): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    const unlisteners: Array<() => void> = [];
    const cleanup = () => {
      for (const fn of unlisteners) { try { fn(); } catch { /* ignore */ } }
    };
    void (async () => {
      unlisteners.push(await listen("ocr-engine-progress", (ev) => {
        const { downloaded, total } = ev.payload as { downloaded: number; total: number };
        onProgress?.({
          percent: total > 0 ? Math.round((downloaded / total) * 100) : 0,
          mb: downloaded / 1048576,
        });
      }));
      unlisteners.push(await listen("ocr-engine-ready", () => { cleanup(); resolve(true); }));
      unlisteners.push(await listen("ocr-engine-error", () => { cleanup(); resolve(false); }));
      try {
        await invoke("ocr_engine_install");
      } catch {
        cleanup();
        resolve(false);
      }
    })();
  });
}

/** 引擎缺失时在状态行内联「下载并安装」按钮。 */
function renderEngineInstallPrompt(statusEl: HTMLElement | null) {
  if (!statusEl) return;
  statusEl.innerHTML = `${t("ocr.engine_missing")} `;
  const btn = document.createElement("button");
  btn.className = "ocr-action-btn ocr-primary-btn";
  btn.style.marginLeft = "6px";
  btn.textContent = t("ocr.engine_download");
  btn.addEventListener("click", async () => {
    btn.disabled = true;
    btn.textContent = t("ocr.engine_downloading").replace("{percent}", "0");
    const ok = await installOcrEngine(({ percent, mb }) => {
      btn.textContent = percent > 0
        ? t("ocr.engine_downloading").replace("{percent}", String(percent))
        : t("ocr.engine_downloading_unknown").replace("{mb}", mb.toFixed(1));
    });
    statusEl.textContent = ok
      ? `${t("ocr.engine_ready")}`
      : `${t("ocr.engine_failed")}`;
  });
  statusEl.appendChild(btn);
}

/** 执行 OCR 前确认引擎就绪；缺失则渲染安装提示并返回 false。 */
async function ensureEngineReady(statusEl: HTMLElement | null): Promise<boolean> {
  try {
    if (await invoke<boolean>("ocr_engine_status")) return true;
  } catch {
    return true; // 状态查询异常时不拦截，交由 run_paddle_ocr 报真实错误
  }
  renderEngineInstallPrompt(statusEl);
  return false;
}

// ── 剪贴板图片读取 ────────────────────────────────────────────────
// 使用原生 Win32 FFI（CF_HDROP + CF_DIB/CF_DIBV5），避免 arboard 插件在
// BI_BITFIELDS 压缩 DIB 上触发 STATUS_HEAP_CORRUPTION 崩溃。
// ShareX 等截图工具默认输出 BI_BITFIELDS 格式 DIB。

async function getClipboardImage(): Promise<string | null> {
  // Stage 1: CF_HDROP file paths
  try {
    const files: string[] = await invoke("read_clipboard_files");
    if (files.length > 0) {
      return `__file__${files[0]}`;
    }
  } catch { /* no files on clipboard */ }

  // Stage 2: Native DIB → BMP
  // Returns "path|fingerprint" — extract path portion only
  try {
    const result: string = await invoke("read_clipboard_backup_image");
    if (result) {
      const sepIdx = result.lastIndexOf("|");
      const tempPath = sepIdx > 0 ? result.substring(0, sepIdx) : result;
      return `__file__${tempPath}`;
    }
  } catch { /* no DIB on clipboard */ }

  return null;
}

function blobToDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = () => reject(new Error("Failed to read blob"));
    reader.readAsDataURL(blob);
  });
}

// ── OCR via PaddleOCR-json ────────────────────────────────────────

interface OcrResult {
  text: string;
}

/** OCR from a file path — directly passed to PaddleOCR-json */
async function ocrFromFile(filePath: string): Promise<OcrResult> {
  const text: string = await invoke("run_paddle_ocr", { path: filePath });
  return { text: text.trim() };
}

/** OCR from clipboard data URL: save temp image → PaddleOCR-json */
async function ocrFromDataUrl(dataUrl: string): Promise<OcrResult> {
  const tempPath: string = await invoke("save_temp_image", { dataUrl });
  const text: string = await invoke("run_paddle_ocr", { path: tempPath });
  invoke("delete_temp_image", { path: tempPath }).catch(() => {});
  return { text: text.trim() };
}

// ── File path → data URL (for image preview) ─────────────────────
// WebView2's CSP `img-src` has no `file:` scheme, so a plain `file:///`
// URL is silently blocked. `convertFileSrc` maps the local path to the
// Tauri asset protocol (`https://asset.localhost/...`, whitelisted), which
// is how local images render reliably (点16/17).

async function fileToDataUrl(filePath: string): Promise<string> {
  return convertFileSrc(filePath);
}

// ── Plugin 定义 ───────────────────────────────────────────────────

export const ocrPlugin: Plugin = {
  id: "ocr",
  name: "OCR 文字识别",
  keywords: [
    "ocr", "识别", "文字识别", "图像识别",
    "图片转文字", "截图识别", "图识字",
    "文字提取",
  ],
  description: "OCR 图片文字识别 (PaddleOCR · 离线高精度)",
  icon: "\uD83D\uDD0D",
  badge: "AI",

  async execute(input: string): Promise<PluginResult> {
    // Always return the detached two-panel HTML skeleton.
    // Actual OCR work is done in the post-execute callback in main.ts
    // via autoStartClipboardOcr() or ocrImageFile().
    return { type: "html", content: buildDetachedPanelHtml() };
  },
};

// ── Detached two-panel HTML ───────────────────────────────────────

function buildDetachedPanelHtml(): string {
  return `
    <div class="ocr-detached-layout">
      <!-- 左半：图片预览 -->
      <div class="ocr-image-panel">
        <div id="ocr-image-preview" class="ocr-image-preview">
          <span class="ocr-image-placeholder">${t("ocr.no_image")}</span>
        </div>
        <div class="ocr-image-actions">
          <button id="ocr-clipboard-btn" class="ocr-action-btn ocr-primary-btn">
            ${t("ocr.clipboard_btn")}
          </button>
          <button id="ocr-file-btn" class="ocr-action-btn">
            ${t("ocr.file_btn")}
          </button>
        </div>
      </div>

      <!-- 右半：文字编辑区 -->
      <div class="ocr-text-panel">
        <div id="ocr-status-line" class="ocr-status-line">
          ${t("ocr.default_status")}
        </div>
        <button id="ocr-copy-btn" class="ocr-copy-btn">${t("ocr.copy_btn")}</button>
        <textarea
          id="ocr-result" class="ocr-textarea"
          spellcheck="false"
          placeholder="${t("ocr.placeholder")}"
        ></textarea>
      </div>
    </div>`;
}

// ── Auto-start & image file entry (called by main.ts) ────────────

export async function autoStartClipboardOcr() {
  const statusEl = document.getElementById("ocr-status-line");
  const resultEl = document.getElementById("ocr-result") as HTMLTextAreaElement | null;
  const previewEl = document.getElementById("ocr-image-preview");

  if (statusEl) statusEl.textContent = t("ocr.reading");
  const img = await getClipboardImage();
  if (!img) {
    if (statusEl) statusEl.textContent = t("ocr.no_clipboard");
    return;
  }

  // Handle file path marker (CF_HDROP from Explorer)
  if (img.startsWith("__file__")) {
    const filePath = img.slice(8);
    await ocrImageFile(filePath);
    return;
  }

  // Show clipboard image in preview
  if (previewEl) {
    previewEl.innerHTML = `<img src="${img}" class="ocr-preview-img" alt="Clipboard image" />`;
  }

  if (!(await ensureEngineReady(statusEl))) return;
  if (statusEl) statusEl.textContent = t("ocr.recognizing");
  try {
    const result = await ocrFromDataUrl(img);
    if (statusEl) statusEl.textContent = t("ocr.done");
    if (resultEl) {
      resultEl.value = result.text || t("ocr.no_text");
      resultEl.style.height = "auto";
      resultEl.style.height = Math.max(resultEl.scrollHeight, 120) + "px";
    }
  } catch (e) {
    if (statusEl) statusEl.textContent = `${t("ocr.failed")}${e}`;
  }
}

export async function ocrImageFile(imagePath: string) {
  const statusEl = document.getElementById("ocr-status-line");
  const resultEl = document.getElementById("ocr-result") as HTMLTextAreaElement | null;
  const previewEl = document.getElementById("ocr-image-preview");

  if (statusEl) statusEl.textContent = t("ocr.loading_image");

  if (!(await ensureEngineReady(statusEl))) return;

  // Show image preview
  if (previewEl) {
    try {
      const dataUrl = await fileToDataUrl(imagePath);
      previewEl.innerHTML = `<img src="${dataUrl}" class="ocr-preview-img" alt="OCR image" onerror="this.parentElement!.innerHTML='<span class=\\'ocr-image-placeholder\\'>${t("ocr.load_failed").replace("'", "\\'")}</span>'" />`;
    } catch {
      previewEl.innerHTML = `<img src="${convertFileSrc(imagePath)}" class="ocr-preview-img" alt="OCR image" onerror="this.parentElement!.innerHTML='<span class=\\'ocr-image-placeholder\\'>${imagePath.split("\\").pop()}</span>'" />`;
    }
  }

  if (statusEl) statusEl.textContent = t("ocr.recognizing");
  try {
    const result = await ocrFromFile(imagePath);
    if (statusEl) statusEl.textContent = t("ocr.done");
    if (resultEl) {
      resultEl.value = result.text || t("ocr.no_text");
      resultEl.style.height = "auto";
      resultEl.style.height = Math.max(resultEl.scrollHeight, 120) + "px";
    }
  } catch (e) {
    if (statusEl) statusEl.textContent = `${t("ocr.failed")}${e}`;
  }
}

// ── Event listeners ──────────────────────────────────────────────

export function attachOcrListeners(doc: Document) {
  // "识别剪贴板" button
  doc.getElementById("ocr-clipboard-btn")?.addEventListener("click", async () => {
    const s = doc.getElementById("ocr-status-line");
    const r = doc.getElementById("ocr-result") as HTMLTextAreaElement | null;
    const previewEl = doc.getElementById("ocr-image-preview");
    if (s) s.textContent = t("ocr.reading");
    if (r) r.value = "";
    const img = await getClipboardImage();
    if (!img) {
      if (s) s.textContent = t("ocr.no_clipboard");
      return;
    }
    // Handle file path marker (CF_HDROP from Explorer)
    if (img.startsWith("__file__")) {
      await ocrImageFile(img.slice(8));
      return;
    }
    if (previewEl) {
      previewEl.innerHTML = `<img src="${img}" class="ocr-preview-img" alt="Clipboard image" />`;
    }
    if (!(await ensureEngineReady(s))) return;
    if (s) s.textContent = t("ocr.recognizing");
    try {
      const result = await ocrFromDataUrl(img);
      if (s) s.textContent = t("ocr.done");
      if (r) {
        r.value = result.text || t("ocr.no_text");
        r.style.height = "auto";
        r.style.height = Math.max(r.scrollHeight, 120) + "px";
      }
    } catch (e) { if (s) s.textContent = `${t("ocr.failed")}${e}`; }
  });

  // "选择文件" button
  doc.getElementById("ocr-file-btn")?.addEventListener("click", () => {
    const input = document.createElement("input");
    input.type = "file"; input.accept = "image/*"; input.style.display = "none";
    input.onchange = async () => {
      const file = input.files?.[0]; if (!file) return;
      const s = doc.getElementById("ocr-status-line");
      const r = doc.getElementById("ocr-result") as HTMLTextAreaElement | null;
      const previewEl = doc.getElementById("ocr-image-preview");
      if (!(await ensureEngineReady(s))) return;
      if (s) s.textContent = t("ocr.recognizing");
      if (r) r.value = "";
      try {
        const dataUrl = await blobToDataUrl(file);
        if (previewEl) {
          previewEl.innerHTML = `<img src="${dataUrl}" class="ocr-preview-img" alt="Selected image" />`;
        }
        const result = await ocrFromDataUrl(dataUrl);
        if (s) s.textContent = t("ocr.done");
        if (r) {
          r.value = result.text || t("ocr.no_text");
          r.style.height = "auto";
          r.style.height = Math.max(r.scrollHeight, 120) + "px";
        }
      } catch (e) { if (s) s.textContent = `${t("ocr.failed")}${e}`; }
    };
    document.body.appendChild(input); input.click();
  });

  // "复制结果" button
  doc.getElementById("ocr-copy-btn")?.addEventListener("click", () => {
    const el = doc.getElementById("ocr-result") as HTMLTextAreaElement | null;
    const text = el?.value || "";
    navigator.clipboard.writeText(text).then(() => {
      const btn = doc.getElementById("ocr-copy-btn");
      if (btn) {
        btn.textContent = t("ocr.copied");
        btn.classList.add("ocr-copied");
        setTimeout(() => {
          btn.textContent = t("ocr.copy_btn");
          btn.classList.remove("ocr-copied");
        }, 1500);
      }
    }).catch(() => {
      const el2 = doc.getElementById("ocr-result") as HTMLTextAreaElement | null;
      if (el2) { el2.select(); }
    });
  });
}
