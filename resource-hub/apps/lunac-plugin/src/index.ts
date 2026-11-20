import { ResourceHubApp, type OpenWork } from "./app.js";

const ZH = {
  title: "阅读",
  sub: "漫画 · 小说 · 番剧 · 电视剧",
  body: "在阅读中搜索、阅读与收藏。",
  open: "打开阅读",
};

const EN = {
  title: "Read",
  sub: "Comics · Novels · Anime · TV",
  body: "Search, read and save in Read.",
  open: "Open Read",
};

const LANG = navigator.language.toLowerCase().startsWith("zh") ? ZH : EN;

function esc(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function isPluginWindow(): boolean {
  return !!document.getElementById("plugin-titlebar");
}

/** A7 对话入口（2026-10-27）：宿主/前端传进来的开窗入参。
 *
 *  **只认显式请求**：`{"action":"search","query":…,"kind":…}`（AI 结果卡下发的「去阅读搜这个」）。
 *  其它任何字符串 —— 最典型的是宿主搜索框里的原文（用户可能只是打了「阅读」「小说」两个字
 *  来打开插件）—— **一律不当搜索词**，插件打开就是正常首页。
 *  2026-10-31 用户要求：搜索框里的内容不该直达阅读插件内部的搜索。
 *  认不出格式时返回 `null`（= 正常打开），不因为格式不对而让面板变成空的。 */
type SearchRequest = { query: string; kind?: string };

function parseInput(raw: string): SearchRequest | null {
  const text = raw.trim();
  if (!text.startsWith("{")) return null;
  try {
    const v = JSON.parse(text) as { action?: unknown; query?: unknown; kind?: unknown };
    if (v && v.action === "search" && typeof v.query === "string" && v.query.trim()) {
      return {
        query: v.query.trim(),
        kind: typeof v.kind === "string" && v.kind.trim() ? v.kind.trim() : undefined,
      };
    }
  } catch {
    /* 不是 JSON：不是显式请求 ⇒ 正常打开 */
  }
  return null;
}

function cardHtml(query: string): string {
  return `<div style="border:1px solid var(--border-glass);border-radius:8px;padding:12px 14px;background:rgba(var(--ink-rgb),.03)">
<div style="display:flex;align-items:baseline;gap:10px;margin-bottom:6px">
<span style="font-weight:600">${esc(LANG.title)}</span>
<span style="color:var(--text-muted);font-size:.78rem">${esc(LANG.sub)}</span>
</div>
<div style="color:var(--text-dim);font-size:.86rem;line-height:1.5;margin-bottom:10px">${esc(query ? `“${query}” · ${LANG.body}` : LANG.body)}</div>
<button data-rh-card-open style="border:1px solid rgba(var(--accent-rgb),.42);background:rgba(var(--accent-rgb),.12);color:var(--text);border-radius:6px;padding:6px 12px;cursor:pointer;font:inherit">${esc(LANG.open)}</button>
</div>`;
}

/** A7 二期（2026-10-28）：聊天结果卡点某一条作品 → `{"action":"open","work":{…}}`。
 *  认不出就返回 null（回落到普通搜索词路径），绝不因为格式不对把面板打开成空的。 */
function parseOpen(raw: string): OpenWork | null {
  const text = raw.trim();
  if (!text.startsWith("{")) return null;
  try {
    const v = JSON.parse(text) as { action?: unknown; work?: unknown };
    if (!v || v.action !== "open" || typeof v.work !== "object" || v.work === null) return null;
    const w = v.work as Record<string, unknown>;
    const sourceId = typeof w.sourceId === "string" ? w.sourceId.trim() : "";
    const workId = typeof w.workId === "string" ? w.workId.trim() : "";
    const title = typeof w.title === "string" ? w.title.trim() : "";
    if (!sourceId || !workId || !title) return null;
    return {
      sourceId,
      workId,
      title,
      kind: typeof w.kind === "string" && w.kind ? w.kind : "comic",
      ...(typeof w.author === "string" && w.author ? { author: w.author } : {}),
    };
  } catch {
    return null;
  }
}

let app: ResourceHubApp | null = null;
let appRoot: HTMLElement | null = null;
/** 本次 execute 带来的搜索请求：挂载后立刻用它落进搜索框（消费一次即清）。 */
let pendingSearch: SearchRequest | null = null;
/** 本次 execute 带来的「直接打开某作品」请求（聊天结果卡点条目时下发）。 */
let pendingOpen: OpenWork | null = null;

function mount(root: HTMLElement): void {
  // 同一个 root 只挂一套监听：宿主重新 execute 时会替换 innerHTML，但不会换 root。
  if (!app || appRoot !== root) {
    // 换了根节点 = 旧实例的 DOM 已被丢弃，但它手里的播放器 / 定时器还活着。
    app?.destroy();
    app = new ResourceHubApp(root);
    appRoot = root;
  }
  app.mount();
}

/** 挂载 + 消费本次入参（窗口与主窗内嵌面板共用同一条路）。
 *  `open` 优先于 `search`：点聊天结果卡里的一条作品时，直接进该作品详情，不先做一轮搜索。 */
function mountWithPending(root: HTMLElement): void {
  mount(root);
  const open = pendingOpen;
  const req = pendingSearch;
  pendingOpen = null;
  pendingSearch = null;
  if (open) {
    app?.openPublishedWork(open);
    return;
  }
  if (req) app?.applySearch(req.query, req.kind);
}

export default {
  async execute(input: string): Promise<{ type: "html"; content: string }> {
    pendingOpen = parseOpen(input);
    pendingSearch = pendingOpen ? null : parseInput(input);
    return { type: "html", content: cardHtml(pendingOpen?.title ?? pendingSearch?.query ?? "") };
  },

  attach(root: HTMLElement): void {
    // 悬浮窗（window.float）里直接进入完整界面；搜索结果区则先显示卡片，点击后再展开。
    if (isPluginWindow()) {
      mountWithPending(root);
      return;
    }
    const open = root.querySelector<HTMLElement>("[data-rh-card-open]");
    open?.addEventListener("click", () => mountWithPending(root));
  },

  detach(): void {
    // 卸载前主动收掉播放器 / 定时器：直接丢引用时 iframe 内的 `<video>` 仍可能
    // 在拉 MSE 分片，表现是「插件关了还有声音」。
    app?.destroy();
    app = null;
    appRoot = null;
    pendingSearch = null;
    pendingOpen = null;
  },
};
