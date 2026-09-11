// ── 备忘录插件 ────────────────────────────────────────────────
// 数据落盘：<exe 根>\ModuleData\memo\memo.json（Rust memo_save/load_entries）
// 草稿：localStorage lunac-memo-draft（仅文本草稿，非业务数据）
// 交互（2026-09）：
//   - 主界面 = 编辑界面：输入内容 → 保存 → 弹出“标识”对话框 → 完整保存
//   - 保存栏左侧（紧邻“清空全部”左侧）有“历史记录”入口
//   - 历史记录为独立子界面：各条完整预览 + 复制/编辑(删除·保存)，保存逻辑与主界面一致
//   - 用户可为每条备忘录自定义检索标识；搜索栏精确/前缀匹配该标识 → “编辑备忘录·标识”
//     结果 → 点击直达该备忘录编辑界面
// 图标：遵循 docs/icon-style.md —— 线性 SVG（stroke currentColor），不再用 emoji。

import type { Plugin, PluginResult } from "../registry";
import { t } from "../../i18n.js";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";

export interface MemoEntry {
  id: string;
  text: string;
  tag: string; // 用户自定义检索标识（可空）
  images?: string[]; // 随备忘录保存的图片绝对路径
  ts: number;
}

// ── 线性 SVG 图标（icon-style.md）────────────────────────────
const I = {
  save: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>`,
  copy: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/></svg>`,
  edit: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.8 2.8 0 0 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/></svg>`,
  del: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6M14 11v6M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>`,
  cancel: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`,
  history: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="8" y1="6" x2="21" y2="6"/><line x1="8" y1="12" x2="21" y2="12"/><line x1="8" y1="18" x2="21" y2="18"/><line x1="3" y1="6" x2="3.01" y2="6"/><line x1="3" y1="12" x2="3.01" y2="12"/><line x1="3" y1="18" x2="3.01" y2="18"/></svg>`,
  tag: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M20.6 13.4 13.4 20.6a2 2 0 0 1-2.8 0L2 12V2h10l8.6 8.6a2 2 0 0 1 0 2.8Z"/><circle cx="7" cy="7" r="1"/></svg>`,
};

// ── 缓存 + 检索索引 ───────────────────────────────────────────
let entriesCache: MemoEntry[] = [];
let moduleEditingId: string | null = null; // 历史编辑载入主界面时的目标 id
let savedImages: string[] = [];   // 正在编辑条目的已有图片路径
let pendingImages: string[] = []; // 本次粘贴的图片 dataURL，随保存写入 ModuleData

function genId(): string {
  return `${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
}

function pushIndex() {
  (window as any).__lunac_memo_index = entriesCache
    .filter(e => e.tag && e.tag.trim())
    .map(e => ({ id: e.id, tag: e.tag.trim() }));
}

async function loadEntries(): Promise<MemoEntry[]> {
  try {
    const list = await invoke<MemoEntry[]>("memo_load_entries");
    entriesCache = Array.isArray(list) ? list : [];
  } catch {
    entriesCache = [];
  }
  // 迁移：历史 localStorage（2026-09 前）数据首次转存到 ModuleData 文件
  if (entriesCache.length === 0) {
    try {
      const legacy: MemoEntry[] = JSON.parse(localStorage.getItem("lunac-memo-entries") || "[]");
      if (Array.isArray(legacy) && legacy.length > 0) {
        const migrated = legacy.map(e => ({ id: e.id || genId(), text: e.text, tag: e.tag || "", ts: e.ts || Date.now() }));
        await persist(migrated);
        localStorage.removeItem("lunac-memo-entries");
      }
    } catch {}
  }
  pushIndex();
  return entriesCache;
}

async function persist(list: MemoEntry[]) {
  entriesCache = list;
  pushIndex();
  try { await invoke("memo_save_entries", { entries: list }); } catch {}
}

/** 供 main.ts 启动时调用，预热标识检索索引。 */
export async function refreshMemoIndex(): Promise<void> {
  await loadEntries();
}

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function fmtTs(ts: number): string {
  const d = new Date(ts);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

// ── 草稿 ─────────────────────────────────────────────────────
function loadDraft(): string {
  try { return localStorage.getItem("lunac-memo-draft") || ""; } catch { return ""; }
}
function saveDraft(text: string) {
  try { localStorage.setItem("lunac-memo-draft", text); } catch {}
}

// ── 视图 HTML ─────────────────────────────────────────────────

/** 主界面（编辑）：textarea + [保存][历史记录][清空全部] + 标识对话框占位 */
function mainViewHtml(text: string, notice?: string): string {
  return `
    <div class="memo-root" data-view="main">
      ${notice ? `<div class="memo-saved-msg">${I.save} ${esc(notice)}</div>` : ""}
      <textarea id="memo-textarea" class="memo-textarea" placeholder="${esc(t("memo.placeholder"))}" spellcheck="false">${esc(text)}</textarea>
      <div id="memo-images" class="memo-images"></div>
      <div class="memo-actions">
        <button id="memo-save-btn" class="memo-save-btn">${I.save} ${t("memo.save")}</button>
        <span id="memo-autosave-hint" class="memo-autosave-hint">${t("memo.autosave")}</span>
        <button id="memo-clear-btn" class="memo-clear-btn">${I.del} ${t("memo.clear_all")}</button>
        <button id="memo-history-btn" class="memo-history-btn" title="${t("memo.history")}">${I.history} ${t("memo.history")}</button>
      </div>
      <div class="memo-tag-dialog" id="memo-tag-dialog" style="display:none;">
        <div class="memo-tag-dialog-title">${I.tag} ${t("memo.set_tag")}</div>
        <input type="text" id="memo-tag-input" class="memo-tag-input" placeholder="${esc(t("memo.tag_placeholder"))}" spellcheck="false" autocomplete="off">
        <div class="memo-tag-actions">
          <button id="memo-tag-ok" class="memo-tag-ok">${I.save} ${t("memo.tag_ok")}</button>
          <button id="memo-tag-skip" class="memo-tag-skip">${t("memo.tag_skip")}</button>
        </div>
      </div>
    </div>`;
}

function tagChip(e: MemoEntry): string {
  if (!e.tag || !e.tag.trim()) return "";
  return `<span class="memo-tag-chip" title="${esc(t("memo.tag_title"))}">${I.tag} ${esc(e.tag.trim())}</span>`;
}

/** 编辑器图片条：已存图片(convertFileSrc) + 待粘贴预览(dataURL) */
function imagesStripHtml(): string {
  const cells: string[] = [];
  savedImages.forEach((p, i) => {
    cells.push(`<span class="memo-img-cell"><img src="${esc(convertFileSrc(p))}" class="memo-img-thumb" alt=""><button class="memo-img-del" data-kind="saved" data-index="${i}" title="${t("memo.delete")}">${I.cancel}</button></span>`);
  });
  pendingImages.forEach((p, i) => {
    cells.push(`<span class="memo-img-cell"><img src="${esc(p)}" class="memo-img-thumb" alt=""><button class="memo-img-del" data-kind="pending" data-index="${i}" title="${t("memo.delete")}">${I.cancel}</button></span>`);
  });
  return cells.join("");
}

function entryImagesHtml(entry: MemoEntry): string {
  const imgs = entry.images || [];
  if (imgs.length === 0) return "";
  return `<div class="memo-images">${imgs.map(p => `<span class="memo-img-cell"><img src="${esc(convertFileSrc(p))}" class="memo-img-thumb" alt=""></span>`).join("")}</div>`;
}

/** 历史子界面：各条完整预览 + 复制/编辑/删除 */
function historyViewHtml(list: MemoEntry[]): string {
  const items = list.length === 0
    ? `<div class="memo-empty">${t("memo.empty")}</div>`
    : list.map(e => `
        <div class="memo-hist-item" data-id="${e.id}">
          <div class="memo-hist-head">
            <span class="memo-item-time">${fmtTs(e.ts)}</span>
            ${tagChip(e)}
          </div>
          <div class="memo-hist-text">${esc(e.text)}</div>
          ${entryImagesHtml(e)}
          <div class="memo-item-actions">
            <button class="memo-copy" data-id="${e.id}" title="${t("memo.copy")}">${I.copy} ${t("memo.copy")}</button>
            <button class="memo-edit" data-id="${e.id}" title="${t("memo.edit")}">${I.edit} ${t("memo.edit")}</button>
            <button class="memo-del" data-id="${e.id}" title="${t("memo.delete")}">${I.del} ${t("memo.delete")}</button>
          </div>
        </div>`).join("");
  return `
    <div class="memo-root" data-view="history">
      <div class="memo-hist-header">
        <button id="memo-back-btn" class="memo-back-btn">${I.cancel} ${t("memo.back")}</button>
        <span class="memo-hist-title">${I.history} ${t("memo.history")}</span>
      </div>
      <div id="memo-hist-list" class="memo-hist-list">${items}</div>
    </div>`;
}

// ── 事件绑定 ─────────────────────────────────────────────────

export function attachMemoListeners(root: HTMLElement) {
  const view = root.querySelector<HTMLElement>(".memo-root");
  if (!view) return;

  function render(html: string) {
    if (!root) return;
    root.innerHTML = html;
    attachMemoListeners(root); // 重新绑定（旧节点已随 innerHTML 替换移除）
  }

  // 历史记录子界面按钮
  const backBtn = view.querySelector<HTMLElement>("#memo-back-btn");
  backBtn?.addEventListener("click", async () => {
    const text = moduleEditingId ? (await loadEntries()).find(e => e.id === moduleEditingId)?.text || "" : loadDraft();
    moduleEditingId = null;
    savedImages = [];
    pendingImages = [];
    render(mainViewHtml(text));
  });

  // 历史子界面：复制 / 编辑 / 删除
  const histList = view.querySelector<HTMLElement>("#memo-hist-list");
  if (histList) {
    histList.querySelectorAll<HTMLElement>(".memo-copy").forEach(btn => {
      btn.addEventListener("click", async () => {
        const id = btn.dataset.id; if (!id) return;
        const entry = (await loadEntries()).find(e => e.id === id); if (!entry) return;
        writeText(entry.text).then(() => {
          const orig = btn.innerHTML;
          btn.innerHTML = `${I.save} ${t("memo.copied")}`;
          window.setTimeout(() => { btn.innerHTML = orig; }, 1200);
        }).catch(() => {});
      });
    });
    histList.querySelectorAll<HTMLElement>(".memo-edit").forEach(btn => {
      btn.addEventListener("click", async () => {
        const id = btn.dataset.id; if (!id) return;
        const entry = (await loadEntries()).find(e => e.id === id); if (!entry) return;
        moduleEditingId = id;
        savedImages = entry.images || [];
        pendingImages = [];
        render(mainViewHtml(entry.text));
      });
    });
    histList.querySelectorAll<HTMLElement>(".memo-del").forEach(btn => {
      btn.addEventListener("click", async () => {
        const id = btn.dataset.id; if (!id) return;
        const list = (await loadEntries()).filter(e => e.id !== id);
        await persist(list);
        render(historyViewHtml(list));
      });
    });
  }

  // 以下控件仅在主界面存在
  const textarea = view.querySelector<HTMLTextAreaElement>("#memo-textarea");
  const saveBtn = view.querySelector<HTMLButtonElement>("#memo-save-btn");
  const clearBtn = view.querySelector<HTMLButtonElement>("#memo-clear-btn");
  const historyBtn = view.querySelector<HTMLButtonElement>("#memo-history-btn");
  const hint = view.querySelector<HTMLElement>("#memo-autosave-hint");
  const tagDialog = view.querySelector<HTMLElement>("#memo-tag-dialog");
  const tagInput = view.querySelector<HTMLInputElement>("#memo-tag-input");
  const tagOk = view.querySelector<HTMLButtonElement>("#memo-tag-ok");
  const tagSkip = view.querySelector<HTMLButtonElement>("#memo-tag-skip");

  // 历史入口（位于“清空全部”旁）
  historyBtn?.addEventListener("click", async () => {
    const list = await loadEntries();
    render(historyViewHtml(list));
  });

  // ── 图片条：已存图片/粘贴预览渲染、粘贴图片、移除 ──
  const imagesEl = view.querySelector<HTMLElement>("#memo-images");
  const renderImages = () => {
    if (imagesEl) imagesEl.innerHTML = imagesStripHtml();
  };
  renderImages();
  imagesEl?.addEventListener("click", async (ev: Event) => {
    const btn = (ev.target as HTMLElement).closest<HTMLElement>(".memo-img-del");
    if (!btn) return;
    const kind = btn.dataset.kind;
    const idx = Number(btn.dataset.index || "-1");
    if (kind === "pending" && idx >= 0) {
      pendingImages.splice(idx, 1);
      renderImages();
    } else if (kind === "saved" && idx >= 0) {
      savedImages.splice(idx, 1);
      renderImages();
      // 同步到当前编辑条目的文件
      if (moduleEditingId) {
        const list = await loadEntries();
        const e = list.find(x => x.id === moduleEditingId);
        if (e) { e.images = savedImages; await persist(list); }
      }
    }
  });

  textarea?.addEventListener("paste", (e: ClipboardEvent) => {
    const files = Array.from(e.clipboardData?.files || []);
    const images = files.filter(f => f.type.startsWith("image/"));
    if (images.length === 0) return;
    e.preventDefault();
    let loaded = 0;
    images.forEach(file => {
      const reader = new FileReader();
      reader.onload = () => {
        if (typeof reader.result === "string") {
          pendingImages.push(reader.result);
        }
        loaded++;
        if (loaded === images.length) {
          renderImages();
          if (hint) hint.textContent = t("memo.images_pending");
        }
      };
      reader.readAsDataURL(file);
    });
  });

  // 拖放图片到编辑器（主界面）：与粘贴一致进入待保存图片条
  if (imagesEl) {
    view.addEventListener("dragover", (e: DragEvent) => { e.preventDefault(); });
    view.addEventListener("drop", (e: DragEvent) => {
      const files = Array.from(e.dataTransfer?.files || []);
      const images = files.filter(f => f.type.startsWith("image/"));
      if (images.length === 0) return;
      e.preventDefault();
      let loaded = 0;
      images.forEach(file => {
        const reader = new FileReader();
        reader.onload = () => {
          if (typeof reader.result === "string") pendingImages.push(reader.result);
          loaded++;
          if (loaded === images.length) {
            renderImages();
            if (hint) hint.textContent = t("memo.images_pending");
          }
        };
        reader.readAsDataURL(file);
      });
    });
  }

  // 草稿自动保存
  let draftTimer: number | undefined;
  textarea?.addEventListener("input", () => {
    if (hint) hint.textContent = t("memo.typing");
    window.clearTimeout(draftTimer);
    draftTimer = window.setTimeout(() => {
      saveDraft(textarea.value);
      if (hint) hint.textContent = t("memo.autosave");
    }, 500);
  });

  // 打开标识对话框（保存后）
  function openTagDialog() {
    if (!tagDialog || !tagInput) return;
    const target = moduleEditingId ? entriesCache.find(e => e.id === moduleEditingId) : undefined;
    tagInput.value = target?.tag || "";
    tagDialog.style.display = "block";
    tagInput.focus();
  }
  function closeTagDialog() {
    if (tagDialog) tagDialog.style.display = "none";
  }

  // 完整保存：更新/新增条目文字与标识，清空编辑器
  async function commitWithTag(tag: string) {
    const list = await loadEntries();
    if (moduleEditingId) {
      const idx = list.findIndex(e => e.id === moduleEditingId);
      if (idx >= 0) { list[idx].tag = tag; list[idx].ts = Date.now(); }
      moduleEditingId = null;
    } else {
      // 主界面保存（保存时 text 已写入 list，这里仅补 tag —— 见下方保存流程）
    }
    await persist(list);
  }

  // 保存按钮：先把文本存为条目 → 图片落盘 → 弹出标识对话框 → 确认后才算完整保存
  saveBtn?.addEventListener("click", async () => {
    const text = textarea?.value.trim() || "";
    if (!text) return;
    const list = await loadEntries();
    let entry: MemoEntry;
    if (moduleEditingId) {
      const idx = list.findIndex(e => e.id === moduleEditingId);
      if (idx === -1) { moduleEditingId = null; return; }
      entry = list[idx];
      entry.text = text;
      entry.images = savedImages;
    } else {
      entry = { id: genId(), text, tag: "", images: savedImages, ts: Date.now() };
      moduleEditingId = entry.id;
      list.unshift(entry);
    }
    // 将粘贴的图片写入 ModuleData\memo\images\<id>\ 并挂到条目
    if (pendingImages.length > 0) {
      const base = (entry.images || []).length;
      const paths: string[] = [];
      for (let i = 0; i < pendingImages.length; i++) {
        try {
          const p = await invoke<string>("memo_save_image", {
            id: entry.id,
            index: base + i,
            data_url: pendingImages[i],
          });
          if (p) paths.push(p);
        } catch {}
      }
      if (paths.length > 0) {
        entry.images = [...(entry.images || []), ...paths];
        savedImages = entry.images;
      }
      pendingImages = [];
      renderImages();
    }
    await persist(list);
    openTagDialog();
  });

  tagOk?.addEventListener("click", async () => {
    const tag = tagInput?.value.trim() || "";
    await commitWithTag(tag);
    closeTagDialog();
    moduleEditingId = null;
    savedImages = [];
    pendingImages = [];
    if (textarea) textarea.value = "";
    saveDraft("");
    if (hint) hint.textContent = t("memo.autosave");
    if (view.querySelector("#memo-tag-dialog")) render(mainViewHtml("", t("memo.saved")));
  });

  tagSkip?.addEventListener("click", async () => {
    closeTagDialog();
    moduleEditingId = null;
    savedImages = [];
    pendingImages = [];
    if (textarea) textarea.value = "";
    saveDraft("");
    if (hint) hint.textContent = t("memo.autosave");
    if (view.querySelector("#memo-tag-dialog")) render(mainViewHtml("", t("memo.saved")));
  });
  // Enter 提交标识，Esc 关闭
  tagInput?.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Enter") { e.preventDefault(); tagOk?.click(); }
    else if (e.key === "Escape") { e.preventDefault(); tagSkip?.click(); }
  });

  // 清空全部（两段式确认）
  let armed = false;
  let armTimer: number | undefined;
  clearBtn?.addEventListener("click", async () => {
    if (!armed) {
      armed = true;
      clearBtn.textContent = `⚠ ${t("memo.confirm_clear")}`;
      window.clearTimeout(armTimer);
      armTimer = window.setTimeout(() => {
        armed = false;
        clearBtn.innerHTML = `${I.del} ${t("memo.clear_all")}`;
      }, 3000);
      return;
    }
    armed = false;
    window.clearTimeout(armTimer);
    moduleEditingId = null;
    savedImages = [];
    pendingImages = [];
    await persist([]);
    if (hint) hint.textContent = t("memo.autosave");
    render(mainViewHtml("", t("memo.cleared")));
  });
}

// ── Plugin 定义 ─────────────────────────────────────────────────

export const memoPlugin: Plugin = {
  id: "memo",
  name: "备忘录",
  keywords: ["备忘录", "memo", "便签", "笔记", "记事本"],
  description: "本地自动保存备忘录 (Local auto-save memo)",
  icon: "📝",
  badge: "memo",

  async execute(input: string): Promise<PluginResult> {
    await loadEntries();

    // 1) 搜索直达：标识命中 → 打开对应备忘录编辑界面
    const openReq = (window as any).__lunac_memo_open as { id: string } | null | undefined;
    (window as any).__lunac_memo_open = null;
    if (openReq?.id) {
      const entry = entriesCache.find(e => e.id === openReq.id);
      if (entry) {
        moduleEditingId = entry.id;
        savedImages = entry.images || [];
        pendingImages = [];
        return { type: "html", content: mainViewHtml(entry.text) };
      }
    }

    // 2) 历史编辑恢复（非直达）
    if (moduleEditingId) {
      const entry = entriesCache.find(e => e.id === moduleEditingId);
      if (entry) {
        savedImages = entry.images || [];
        pendingImages = [];
        return { type: "html", content: mainViewHtml(entry.text) };
      }
      moduleEditingId = null;
    }

    // 3) 普通入口：把搜索文本放入草稿（不立即保存）
    savedImages = [];
    pendingImages = [];
    const trimmed = input.trim();
    if (trimmed) saveDraft(trimmed);
    return { type: "html", content: mainViewHtml(loadDraft()) };
  },
};
