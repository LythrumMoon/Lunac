// Web Search plugin — opens the default browser with a configurable search engine.
//
// Supported engines (configurable via Settings → Search, or right-click context menu):
//   google     → https://www.google.com/search?q=
//   bing       → https://www.bing.com/search?q=
//   baidu      → https://www.baidu.com/s?wd=
//
// Preference is stored in localStorage under "lunac-search-engine".

import type { Plugin } from "../registry";
import { open } from "@tauri-apps/plugin-shell";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { t } from "../../i18n.js";

const ENGINES: Record<string, { name: string; icon: string; url: string }> = {
  google:     { name: "Google",     icon: "🔍", url: "https://www.google.com/search?q={{query}}" },
  bing:       { name: "Bing",       icon: "🌐", url: "https://www.bing.com/search?q={{query}}" },
  baidu:      { name: "Baidu",      icon: "🐻", url: "https://www.baidu.com/s?wd={{query}}" },
};

export function getSearchEngine(): string {
  try {
    const saved = localStorage.getItem("lunac-search-engine");
    // DuckDuckGo preset was removed (2026-09) — fall back to Google for stale saved values
    if (saved && ENGINES[saved]) return saved;
    return "google";
  } catch { return "google"; }
}

export function getSearchEngineName(): string {
  return ENGINES[getSearchEngine()]?.name || "Google";
}

export function setSearchEngine(engine: string) {
  try { localStorage.setItem("lunac-search-engine", engine); } catch {}
}

export function buildSearchUrl(query: string, engine?: string): string {
  const eng = engine || getSearchEngine();
  const tpl = ENGINES[eng]?.url || ENGINES.google.url;
  return tpl.replace("{{query}}", encodeURIComponent(query));
}

export const webSearchPlugin: Plugin = {
  id: "web-search",
  name: "Web Search",
  keywords: [
    "search", "web", "google", "browser", "bing", "internet", "url",
    "搜索", "网页", "浏览器", "上网", "百度", "谷歌",
  ],
  description: "Search the web with your default browser",
  icon: "🌐",
  badge: "Web",

  async execute(input: string) {
    const engine = getSearchEngine();
    const url = buildSearchUrl(input, engine);

    try {
      // Hide window first
      const win = getCurrentWindow();
      await win.hide();

      // Open browser with search query
      await open(url);
    } catch (e) {
      console.error("[lunac] web-search: failed to open browser:", e);
    }

    return {
      type: "text",
      content: t("websearch.searching", { engine: ENGINES[engine]?.name || "Google", query: input }),
    };
  },
};
