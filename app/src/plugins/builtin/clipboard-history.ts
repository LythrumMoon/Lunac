// ── Clipboard History plugin ──────────────────────────────────────
// Stores clipboard history as JSON files under Lunac 数据根（exe 所在目录）\ModuleData\history\
// 数据与 WebView2 缓存解耦（清 localStorage/浏览器缓存不影响剪贴板历史）。

import type { Plugin, PluginResult } from "../registry";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { invoke } from "@tauri-apps/api/core";
import { t } from "../../i18n.js";

const MAX_ITEMS = 30;

interface ClipEntry {
  clip_type?: string; // "text" | "file", default "text"
  text: string;
  file_paths?: string[];
  time: number;
}

async function loadHistory(): Promise<ClipEntry[]> {
  try {
    return await invoke<ClipEntry[]>("load_clipboard_history");
  } catch {
    return [];
  }
}

async function saveHistory(entries: ClipEntry[]) {
  try {
    await invoke("save_clipboard_history", {
      entries: entries.slice(0, MAX_ITEMS),
    });
  } catch {
    // File write failed — silently ignore (storage dir may not be writable)
  }
}

export async function addClipboardEntry(text: string) {
  const trimmed = text.trim();
  if (!trimmed || trimmed.length > 2000) return;
  const isFile = /^[A-Za-z]:[\\/]/.test(trimmed);
  const history = await loadHistory();

  if (isFile) {
    // File entry: dedupe by path, group adjacent file entries
    const idx = history.findIndex(e =>
      e.clip_type === "file" && e.text === trimmed
    );
    if (idx >= 0) history.splice(idx, 1);
    history.unshift({
      clip_type: "file",
      text: trimmed,
      file_paths: [trimmed],
      time: Date.now(),
    });
  } else {
    const idx = history.findIndex(e => e.text === trimmed);
    if (idx >= 0) history.splice(idx, 1);
    history.unshift({ clip_type: "text", text: trimmed, time: Date.now() });
  }
  await saveHistory(history);
}

export async function getClipboardPreview(): Promise<string | null> {
  const history = await loadHistory();
  if (history.length > 0) {
    const latest = history[0].text;
    return latest.length > 60 ? latest.slice(0, 60) + "…" : latest;
  }
  return null;
}

function esc(s: string) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;")
          .replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

export const clipboardHistoryPlugin: Plugin = {
  id: "clipboard-history",
  name: "Clipboard History",
  keywords: ["clipboard", "history", "paste", "剪切板", "历史", "剪贴板", "粘贴", "复制"],
  description: "Clipboard history manager — auto-saves on copy",
  icon: "📋",
  badge: "History",
  execute: async (_input: string): Promise<PluginResult> => {
    const history = await loadHistory();

    if (history.length === 0) {
      return {
        type: "html",
        content: `<div style="color:var(--text-dim);padding:16px 12px;">
          ${t("clipboard.empty")}
        </div>`,
      };
    }

    const now = Date.now();
    const items = history.map((entry, i) => {
      const timeStr = formatRelative(now - entry.time);
      const isFile = entry.clip_type === "file";
      const displayText = isFile
        ? (entry.text.split(/[\\/]/).pop() || entry.text)
        : entry.text;
      const preview = displayText.length > 80
        ? displayText.slice(0, 80).replace(/\n/g, " ") + "…"
        : displayText.replace(/\n/g, " ");
      const icon = isFile ? "📁" : "📋";
      const sizeHint = isFile ? "" : ` · ${t("clipboard.chars", { count: String(entry.text.length) })}`;

      return `
        <div class="clip-item" data-idx="${i}" title="${esc(entry.text)}">
          <span class="clip-item-icon" style="flex-shrink:0;font-size:1rem;">${icon}</span>
          <div class="clip-item-preview">${esc(preview)}</div>
          <div class="clip-item-meta">${timeStr}${sizeHint}</div>
          <button class="clip-item-copy" data-idx="${i}" title="${t("clipboard.copy_tooltip")}">${isFile ? "📎 " : "📋 "}${t("clipboard.copy_label")}</button>
        </div>`;
    }).join("");

    const content = `
      <div class="plugin-result">
        <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">
          <span style="font-size:0.72rem;color:var(--text-dim);">${t("clipboard.items", { count: String(history.length) })}</span>
          <button id="clip-clear-all" style="font-size:0.68rem;background:none;border:none;color:var(--red);cursor:pointer;opacity:0.7;">${t("clipboard.clear_all")}</button>
        </div>
        ${items}
      </div>
    `;

    setTimeout(() => {
      document.querySelectorAll(".clip-item-copy").forEach(btn => {
        btn.addEventListener("click", async (e) => {
          e.stopPropagation();
          const idx = parseInt((btn as HTMLElement).dataset.idx || "0");
          const entry = history[idx];
          if (!entry) return;
          try {
            await writeText(entry.text);
            const h = await loadHistory();
            const found = h.findIndex(e => e.text === entry.text);
            if (found >= 0) {
              const [item] = h.splice(found, 1);
              h.unshift(item);
              await saveHistory(h);
            }
            (btn as HTMLElement).textContent = t("clipboard.copied");
            setTimeout(() => {
              (btn as HTMLElement).textContent = `${entry.clip_type === "file" ? "📎 " : "📋 "}${t("clipboard.copy_label")}`;
            }, 1500);
          } catch {
            (btn as HTMLElement).textContent = t("clipboard.copy_failed");
            setTimeout(() => {
              (btn as HTMLElement).textContent = `${entry.clip_type === "file" ? "📎 " : "📋 "}${t("clipboard.copy_label")}`;
            }, 1500);
          }
        });
      });

      document.querySelectorAll(".clip-item").forEach(el => {
        el.addEventListener("click", async () => {
          const idx = parseInt((el as HTMLElement).dataset.idx || "0");
          const entry = history[idx];
          if (!entry) return;
          try {
            await writeText(entry.text);
            (el.querySelector(".clip-item-copy") as HTMLElement).textContent = t("clipboard.copied");
            setTimeout(() => {
              const btn = el.querySelector(".clip-item-copy") as HTMLElement;
              if (btn) btn.textContent = `${entry.clip_type === "file" ? "📎 " : "📋 "}${t("clipboard.copy_label")}`;
            }, 1500);
          } catch { /* ignore */ }
        });
      });

      document.getElementById("clip-clear-all")?.addEventListener("click", async () => {
        await saveHistory([]);
        const resultEl = document.querySelector(".plugin-result") as HTMLElement;
        if (resultEl) {
          resultEl.innerHTML = `<div style="color:var(--text-dim);padding:16px 12px;">${t("clipboard.cleared")}</div>`;
        }
      });
    }, 50);

    return { type: "html", content };
  },
};

function formatRelative(ms: number): string {
  if (ms < 60_000) return t("clipboard.just_now");
  const mins = Math.floor(ms / 60_000);
  if (mins < 60) return t("clipboard.m_ago", { m: String(mins) });
  const hours = Math.floor(mins / 60);
  if (hours < 24) return t("clipboard.h_ago", { h: String(hours) });
  const days = Math.floor(hours / 24);
  return t("clipboard.d_ago", { d: String(days) });
}
