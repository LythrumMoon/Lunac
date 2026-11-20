import { ReaderController } from "./controller.js";
import {
  findBookmark,
  sortBookmarks,
  toggleBookmark,
  type ReaderBookmark,
} from "./bookmarks.js";
import {
  resolveTapZone,
  type ReaderChapter,
  type ReaderCropMode,
  type ReaderInitialState,
  type ReaderMode,
  type ReaderPage,
  type ReaderSnapshot,
} from "./types.js";

export interface ReaderLabels {
  exit: string;
  toc: string;
  lock: string;
  autoplay: string;
  nextChapter: string;
  crop: string;
  mode: string;
  speed: string;
  modeNames: Record<ReaderMode, string>;
  /** 裁剪模式名称；按键循环时在提示里显示。 */
  cropModes: Record<ReaderCropMode, string>;
  /** 双页拆分时的左右半页标识。 */
  splitLeft: string;
  splitRight: string;
  lockedHint: string;
  unlockedHint: string;
  firstPage: string;
  lastPage: string;
  lastChapter: string;
  tocTitle: string;
  close: string;
  progress: string;
  loadingChapter: string;
  chapterLoadFail: string;
  bookmarks: string;
  bookmarksTitle: string;
  bookmarkAdd: string;
  bookmarkRemove: string;
  bookmarkAdded: string;
  bookmarkRemoved: string;
  bookmarkEmpty: string;
}

export interface ReaderOptions {
  chapters: ReaderChapter[];
  initial?: ReaderInitialState;
  labels: ReaderLabels;
  /** 跨章按需加载：lazy 章节进入视野时调用；返回 null 表示失败。 */
  loadChapter?(chapterIndex: number): Promise<ReaderChapter | null>;
  /** 初始书签；组件内部只改副本，通过 `onBookmarksChange` 回传。 */
  bookmarks?: ReaderBookmark[];
  onExit(): void;
  onProgress?(snapshot: ReaderSnapshot): void;
  onChapterChange?(chapterIndex: number, chapter: ReaderChapter): void;
  onBookmarksChange?(bookmarks: ReaderBookmark[]): void;
}

export interface ReaderHandle {
  destroy(): void;
}

const ICONS: Record<string, string> = {
  exit: '<path d="M15 18l-6-6 6-6"/><path d="M9 12h12"/>',
  toc: '<path d="M4 6h16M4 12h16M4 18h10"/>',
  lock: '<rect x="5" y="10" width="14" height="10" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/>',
  play: '<path d="M8 5l11 7-11 7z"/>',
  speed: '<path d="M4 12a8 8 0 1 1 8 8"/><path d="M12 12l4-3"/>',
  next: '<path d="M5 5l10 7-10 7z"/><path d="M19 5v14"/>',
  crop: '<path d="M6 2v16h16"/><path d="M2 6h16v16"/>',
  mode: '<rect x="4" y="4" width="16" height="16" rx="2"/><path d="M8 4v16"/>',
  bookmark: '<path d="M6 4h12v17l-6-4-6 4z"/>',
  close: '<path d="M6 6l12 12"/><path d="M18 6L6 18"/>',
};

function icon(name: string): string {
  return `<svg class="rh-reader-icon" width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[name] ?? ""}</svg>`;
}

function esc(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** 书签列表里显示的一行摘要；图片页取说明，不把 URL 当正文。 */
function pageExcerpt(page: ReaderPage | undefined): string | undefined {
  if (!page) return undefined;
  const raw = page.kind === "image"
    ? (page.caption ?? "")
    : page.kind === "html"
      ? (page.content ?? "").replace(/<[^>]+>/g, " ")
      : (page.content ?? page.caption ?? "");
  const text = raw.replace(/\s+/g, " ").trim();
  if (!text) return undefined;
  return text.length > 48 ? `${text.slice(0, 48)}…` : text;
}

function must<T>(value: T | null, name: string): T {
  if (value === null) throw new Error(`Reader UI 初始化失败：缺少 ${name}`);
  return value;
}

// ── 图片裁白边 / 双页拆分 ───────────────────────────────────────
//
// 只读一次像素：把图片缩到最长边 ≤ 420px 后扫描非白 / 非透明区域，
// 结果按 URL 缓存。跨域图片读不到像素时退回整张图，不阻断阅读。

interface ImageBox {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

interface ImageInfo {
  img: HTMLImageElement;
  width: number;
  height: number;
  box: ImageBox;
}

/** 宽高比达到该值的图片视为跨页大图。 */
const SPREAD_RATIO = 1.2;
const WHITE_LUMINANCE = 242;
const imageInfoCache = new Map<string, Promise<ImageInfo | null>>();

function loadImageInfo(url: string): Promise<ImageInfo | null> {
  const cached = imageInfoCache.get(url);
  if (cached) return cached;
  const task = (async (): Promise<ImageInfo | null> => {
    try {
      const img = await new Promise<HTMLImageElement>((resolve, reject) => {
        const element = new Image();
        element.crossOrigin = "anonymous";
        element.onload = () => resolve(element);
        element.onerror = () => reject(new Error("image load failed"));
        element.src = url;
      });
      const width = img.naturalWidth || img.width;
      const height = img.naturalHeight || img.height;
      if (!width || !height) return null;
      return { img, width, height, box: detectContentBox(img, width, height) };
    } catch {
      return null;
    }
  })();
  imageInfoCache.set(url, task);
  return task;
}

function detectContentBox(img: HTMLImageElement, width: number, height: number): ImageBox {
  const full: ImageBox = { left: 0, top: 0, right: 1, bottom: 1 };
  const scale = Math.min(1, 420 / Math.max(width, height));
  const w = Math.max(1, Math.round(width * scale));
  const h = Math.max(1, Math.round(height * scale));
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) return full;
  try {
    ctx.drawImage(img, 0, 0, w, h);
    const data = ctx.getImageData(0, 0, w, h).data;
    let left = w;
    let top = h;
    let right = -1;
    let bottom = -1;
    for (let y = 0; y < h; y += 1) {
      for (let x = 0; x < w; x += 1) {
        const index = (y * w + x) * 4;
        if (data[index + 3] < 8) continue;
        const luminance =
          0.299 * data[index] + 0.587 * data[index + 1] + 0.114 * data[index + 2];
        if (luminance >= WHITE_LUMINANCE) continue;
        if (x < left) left = x;
        if (x > right) right = x;
        if (y < top) top = y;
        if (y > bottom) bottom = y;
      }
    }
    // 全白 / 全透明：不裁，避免把正常留白页面裁成一条线。
    if (right < left || bottom < top) return full;
    const pad = 2;
    const l = Math.max(0, left - pad);
    const t = Math.max(0, top - pad);
    const r = Math.min(w - 1, right + pad);
    const b = Math.min(h - 1, bottom + pad);
    return {
      left: l / w,
      top: t / h,
      right: (r + 1) / w,
      bottom: (b + 1) / h,
    };
  } catch {
    // 跨域画布被污染 / 解码失败：保持原图。
    return full;
  }
}

/** 把裁剪区域（或其中一半）画到 canvas；显示尺寸交给 CSS 约束。 */
function drawImageRegion(
  info: ImageInfo,
  box: ImageBox,
  half: 0 | 1 | null,
): HTMLCanvasElement | null {
  const sx = Math.round(box.left * info.width);
  const sy = Math.round(box.top * info.height);
  const sw = Math.max(1, Math.round((box.right - box.left) * info.width));
  const sh = Math.max(1, Math.round((box.bottom - box.top) * info.height));
  const partW = half === null ? sw : Math.max(1, Math.round(sw / 2));
  const sourceX = half === 1 ? sx + Math.round(sw / 2) : sx;
  // 极端长图限制画布尺寸，避免移动端 OOM；坐标按比例同步缩放。
  const maxSide = 4096;
  const ratio = Math.min(1, maxSide / Math.max(partW, sh));
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.round(partW * ratio));
  canvas.height = Math.max(1, Math.round(sh * ratio));
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  ctx.drawImage(
    info.img,
    sourceX,
    sy,
    Math.max(1, Math.round(sw / (half === null ? 1 : 2))),
    sh,
    0,
    0,
    canvas.width,
    canvas.height,
  );
  canvas.className = "rh-reader-image";
  return canvas;
}

const STYLE = `
.rh-reader{position:absolute;inset:0;display:flex;flex-direction:column;overflow:hidden;color:var(--text);background:var(--surface-glass);font:inherit}
.rh-reader-stage{position:relative;flex:1;min-height:0;overflow:hidden;touch-action:pan-y;user-select:none;-webkit-user-select:none;-webkit-tap-highlight-color:transparent;-webkit-touch-callout:none}
.rh-reader-page{position:absolute;inset:0;overflow-y:auto;overscroll-behavior:contain;padding:calc(56px + env(safe-area-inset-top)) calc(32px + env(safe-area-inset-right)) calc(118px + env(safe-area-inset-bottom)) calc(32px + env(safe-area-inset-left));box-sizing:border-box;transform-origin:0 0}
.rh-reader-page-in{animation:rh-reader-page-in .18s ease-out}
@keyframes rh-reader-page-in{from{opacity:.4;transform:translateY(4px)}to{opacity:1;transform:none}}
.rh-reader[data-mode="paged"] .rh-reader-page-in{animation-name:rh-reader-page-in-x}
@keyframes rh-reader-page-in-x{from{opacity:.4;transform:translateX(6px)}to{opacity:1;transform:none}}
/* A1 左右分页：整页适配，图片按 contain 落在可视区内，跨页大图占满宽度 */
.rh-reader[data-mode="paged"] .rh-reader-page{overflow:hidden;display:flex;align-items:center;justify-content:center}
.rh-reader[data-mode="paged"] .rh-reader-text,.rh-reader[data-mode="paged"] .rh-reader-html{width:100%;max-height:100%;overflow-y:auto;margin:auto}
.rh-reader[data-mode="paged"] .rh-reader-image{max-width:100%;max-height:100%;width:auto;height:auto;margin:0;object-fit:contain}
.rh-reader[data-mode="paged"] .rh-reader-image.is-spread{width:100%;height:auto;max-height:100%}
.rh-reader-text{max-width:760px;margin:0 auto;line-height:1.9;font-size:1rem;color:var(--text-dim)}
.rh-reader-text p{margin:0 0 1.1em;white-space:pre-wrap}
.rh-reader-html{max-width:760px;margin:0 auto;line-height:1.9;font-size:1rem;color:var(--text-dim);overflow-wrap:anywhere}
.rh-reader-html h1,.rh-reader-html h2,.rh-reader-html h3,.rh-reader-html h4,.rh-reader-html h5,.rh-reader-html h6{color:var(--text);line-height:1.45;margin:1.2em 0 .6em}
.rh-reader-html h1{font-size:1.45rem}.rh-reader-html h2{font-size:1.28rem}.rh-reader-html h3{font-size:1.14rem}
.rh-reader-html p{margin:0 0 1em;white-space:pre-wrap}
.rh-reader-html img{display:block;max-width:100%;height:auto;margin:1em auto;border-radius:6px}
.rh-reader-html blockquote{margin:1em 0;padding:.4em 1em;border-left:3px solid rgba(var(--accent-rgb),.5);color:var(--text-muted)}
.rh-reader-html ul,.rh-reader-html ol{margin:0 0 1em;padding-left:1.6em}
.rh-reader-html li{margin:.3em 0}
.rh-reader-html pre{overflow-x:auto;padding:.8em 1em;background:rgba(var(--ink-rgb),.05);border-radius:6px;white-space:pre-wrap}
.rh-reader-html code{font-family:Consolas,"Cascadia Mono",monospace;font-size:.92em}
.rh-reader-html table{width:100%;border-collapse:collapse;margin:1em 0}
.rh-reader-html td,.rh-reader-html th{border:1px solid var(--border-glass);padding:6px 8px;text-align:left}
.rh-reader-html hr{border:none;border-top:1px solid var(--border-glass);margin:1.4em 0}
.rh-reader-image{display:block;max-width:100%;margin:0 auto 12px;border-radius:6px}
/* 大屏不裁图：分页 / 上下模式让整页图片落在可视区内；条漫保持连续长图 */
.rh-reader:not([data-mode="webtoon"]) .rh-reader-image{width:auto;max-height:calc(100vh - 174px);max-height:calc(100dvh - 174px)}
/* A2 条漫：整章连续拼接 */
.rh-reader[data-mode="webtoon"] .rh-reader-page{overflow-y:auto;padding-left:0;padding-right:0}
.rh-reader[data-mode="webtoon"] .rh-reader-webtoon{display:flex;flex-direction:column;align-items:center;gap:0;max-width:900px;margin:0 auto}
.rh-reader[data-mode="webtoon"] .rh-reader-image{width:100%;max-width:100%;margin:0;border-radius:0}
.rh-reader[data-mode="webtoon"] .rh-reader-text,.rh-reader[data-mode="webtoon"] .rh-reader-html{padding:16px}
/* A3 裁剪：页边距 / 裁白边 / 双页拆分 */
.rh-reader[data-crop="margin"] .rh-reader-page,.rh-reader[data-crop="trim"] .rh-reader-page,.rh-reader[data-crop="split"] .rh-reader-page{padding-left:6px;padding-right:6px}
.rh-reader[data-crop="trim"] .rh-reader-page,.rh-reader[data-crop="split"] .rh-reader-page{padding-top:calc(46px + env(safe-area-inset-top));padding-bottom:calc(104px + env(safe-area-inset-bottom))}
.rh-reader[data-crop="trim"] .rh-reader-image,.rh-reader[data-crop="split"] .rh-reader-image{filter:contrast(1.02)}
/* A4 缩放：手势变换只作用在页面内容层 */
.rh-reader.is-zoomed .rh-reader-page{transition:none}
.rh-reader-page{transition:transform .16s ease}
.rh-reader-placeholder{max-width:760px;margin:0 auto 12px;min-height:180px;display:flex;align-items:center;justify-content:center;border:1px solid var(--border-glass);border-radius:8px;background:rgba(var(--ink-rgb),.04);color:var(--text-muted);font-size:.86rem}
.rh-reader-bar{position:absolute;left:0;right:0;z-index:5;background:rgba(var(--surface-rgb,22,24,27),.9);border-color:var(--border-glass);transition:opacity .22s ease,transform .22s ease}
.rh-reader-bar.is-hidden{opacity:0;pointer-events:none}
.rh-reader-top{top:0;display:flex;align-items:center;gap:10px;padding:calc(8px + env(safe-area-inset-top)) calc(14px + env(safe-area-inset-right)) 8px calc(14px + env(safe-area-inset-left));border-bottom:1px solid var(--border-glass)}
.rh-reader-bottom{bottom:0;display:flex;flex-direction:column;gap:6px;padding:8px calc(14px + env(safe-area-inset-right)) calc(10px + env(safe-area-inset-bottom)) calc(14px + env(safe-area-inset-left));border-top:1px solid var(--border-glass)}
.rh-reader-top.is-hidden{transform:translateY(-6px)}
.rh-reader-bottom.is-hidden{transform:translateY(6px)}
.rh-reader-title{flex:1;min-width:0;text-align:center;font-size:.88rem;color:var(--text-dim);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-reader-btn{display:inline-flex;align-items:center;gap:6px;border:1px solid transparent;background:transparent;color:var(--text-dim);border-radius:6px;padding:6px 8px;cursor:pointer;font:inherit;font-size:.82rem;white-space:nowrap}
.rh-reader-btn:hover{background:rgba(var(--accent-rgb),.08);color:var(--text)}
.rh-reader-btn.is-active{border-color:rgba(var(--accent-rgb),.42);background:rgba(var(--accent-rgb),.12);color:var(--text)}
.rh-reader-btn:disabled{opacity:.4;cursor:default}
.rh-reader-icon{flex:none}
.rh-reader-progress{display:flex;align-items:center;gap:10px;font-size:.74rem;color:var(--text-muted)}
.rh-reader-progress input[type="range"]{flex:1;min-width:0;accent-color:rgb(var(--accent-rgb))}
.rh-reader-actions{display:flex;align-items:center;gap:2px;flex-wrap:wrap}
.rh-reader-toast{position:absolute;left:50%;top:50%;transform:translate(-50%,-50%);z-index:8;background:rgba(var(--surface-rgb,22,24,27),.94);border:1px solid var(--border-glass);border-radius:8px;padding:9px 14px;color:var(--text-dim);font-size:.84rem;opacity:0;pointer-events:none;transition:opacity .18s ease;max-width:80%;text-align:center}
.rh-reader-toast.is-visible{opacity:1}
.rh-reader-toc{position:absolute;inset:0;z-index:7;display:flex;justify-content:flex-end;background:rgba(var(--shade-rgb,0,0,0),.38)}
.rh-reader-toc.is-hidden{display:none}
.rh-reader-toc-panel{width:min(360px,86%);height:100%;background:rgba(var(--surface-rgb,22,24,27),.96);border-left:1px solid var(--border-glass);display:flex;flex-direction:column;min-height:0}
.rh-reader-toc-head{display:flex;align-items:center;justify-content:space-between;gap:10px;padding:calc(10px + env(safe-area-inset-top)) 14px 10px;border-bottom:1px solid var(--border-glass);font-size:.9rem}
.rh-reader-toc-list{flex:1;min-height:0;overflow-y:auto;padding:6px}
.rh-reader-toc-item{display:block;width:100%;text-align:left;border:1px solid transparent;background:transparent;color:var(--text-dim);border-radius:6px;padding:9px 10px;cursor:pointer;font:inherit;font-size:.86rem}
.rh-reader-toc-item:hover{background:rgba(var(--accent-rgb),.08);color:var(--text)}
.rh-reader-toc-item.is-current{border-color:rgba(var(--accent-rgb),.36);background:rgba(var(--accent-rgb),.10);color:var(--text)}
.rh-reader-bookmark-head{padding:8px 10px;border-bottom:1px solid var(--border-glass)}
.rh-reader-bookmark-head .rh-reader-btn{width:100%;justify-content:center;border-color:var(--border-glass)}
.rh-reader-bookmark-sub{display:block;margin-top:2px;font-size:.74rem;color:var(--text-muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-reader-bookmark-empty{padding:18px 12px;color:var(--text-muted);font-size:.84rem;text-align:center}
/* iPad / 平板（竖屏 768+）：正文行宽收紧、字号上浮、控件留出触控尺寸 */
@media (min-width:768px){
  .rh-reader-page{padding:calc(64px + env(safe-area-inset-top)) calc(48px + env(safe-area-inset-right)) calc(132px + env(safe-area-inset-bottom)) calc(48px + env(safe-area-inset-left))}
  .rh-reader-text{max-width:min(820px,74ch);font-size:1.08rem;line-height:2}
  .rh-reader-placeholder{max-width:min(820px,74ch);min-height:220px}
  .rh-reader-top,.rh-reader-bottom{padding-left:calc(24px + env(safe-area-inset-left));padding-right:calc(24px + env(safe-area-inset-right))}
  .rh-reader-btn{font-size:.9rem;padding:9px 12px;gap:8px;min-height:40px}
  .rh-reader-title{font-size:.95rem}
  .rh-reader-actions{gap:6px}
  .rh-reader-progress input[type="range"]{height:24px}
  .rh-reader-toc-panel{width:min(420px,72%)}
  .rh-reader-toc-item{font-size:.92rem;padding:11px 12px}
  .rh-reader-toast{max-width:min(520px,72%);font-size:.92rem;padding:12px 18px}
  .rh-reader:not([data-mode="webtoon"]) .rh-reader-image{max-height:calc(100vh - 196px);max-height:calc(100dvh - 196px)}
}
/* iPad 横屏 / Stage Manager：进一步放宽侧边留白，避免正文贴边 */
@media (min-width:1024px) and (orientation:landscape){
  .rh-reader-page{padding-left:calc(72px + env(safe-area-inset-left));padding-right:calc(72px + env(safe-area-inset-right))}
  .rh-reader-text{max-width:min(900px,80ch)}
  .rh-reader-placeholder{max-width:min(900px,80ch)}
  .rh-reader-top,.rh-reader-bottom{padding-left:calc(40px + env(safe-area-inset-left));padding-right:calc(40px + env(safe-area-inset-right))}
}
/* 手机（竖屏 ≤640px / 横屏矮视口）：底部操作栏压成单行、按钮只留图标，触控目标 44px */
@media (max-width:640px), (max-height:480px) and (pointer:coarse){
  .rh-reader-page{padding:calc(52px + env(safe-area-inset-top)) calc(14px + env(safe-area-inset-right)) calc(112px + env(safe-area-inset-bottom)) calc(14px + env(safe-area-inset-left))}
  .rh-reader-text,.rh-reader-html,.rh-reader-placeholder{font-size:1.02rem;line-height:1.95}
  .rh-reader-top,.rh-reader-bottom{padding-left:calc(8px + env(safe-area-inset-left));padding-right:calc(8px + env(safe-area-inset-right))}
  .rh-reader-top{gap:6px}
  .rh-reader-btn{font-size:.76rem;padding:6px;gap:4px;min-width:44px;min-height:44px;justify-content:center}
  .rh-reader-btn-label{display:none}
  .rh-reader-value{font-size:.72rem;font-variant-numeric:tabular-nums}
  .rh-reader-actions{gap:2px;flex-wrap:nowrap;justify-content:space-between}
  .rh-reader-progress{gap:8px;font-size:.7rem}
  .rh-reader-progress input[type="range"]{height:24px}
  .rh-reader-title{font-size:.82rem;text-align:left}
  .rh-reader-toc-panel{width:min(420px,92%)}
  .rh-reader-toc-item{padding:12px 10px;font-size:.9rem}
  .rh-reader-toast{max-width:86%;font-size:.86rem;padding:10px 14px}
  .rh-reader:not([data-mode="webtoon"]) .rh-reader-image{max-height:calc(100vh - 128px);max-height:calc(100dvh - 128px)}
}
/* 触屏（含 iPad 全系）：所有动作按钮至少 44px 高，进度条加厚便于拖动 */
@media (pointer:coarse){
  .rh-reader-btn{min-height:44px}
  .rh-reader-progress input[type="range"]{height:26px}
}
`;

export function mountReader(root: HTMLElement, options: ReaderOptions): ReaderHandle {
  const controller = new ReaderController(options.chapters, options.initial);
  root.innerHTML = `${`<style>${STYLE}</style>`}
<div class="rh-reader" data-mode="vertical" data-crop="off">
  <div class="rh-reader-stage" data-role="stage">
    <div class="rh-reader-page" data-role="page"></div>
    <div class="rh-reader-toast" data-role="toast"></div>
    <div class="rh-reader-toc is-hidden" data-role="toc">
      <div class="rh-reader-toc-panel">
        <div class="rh-reader-toc-head"><span>${esc(options.labels.tocTitle)}</span>
          <button class="rh-reader-btn" data-action="close-toc">${icon("close")}<span>${esc(options.labels.close)}</span></button></div>
        <div class="rh-reader-toc-list" data-role="toc-list"></div>
      </div>
    </div>
    <div class="rh-reader-toc is-hidden" data-role="bookmarks">
      <div class="rh-reader-toc-panel">
        <div class="rh-reader-toc-head"><span>${esc(options.labels.bookmarksTitle)}</span>
          <button class="rh-reader-btn" data-action="close-bookmarks">${icon("close")}<span>${esc(options.labels.close)}</span></button></div>
        <div class="rh-reader-bookmark-head">
          <button class="rh-reader-btn" data-action="bookmark-toggle" data-role="bookmark-toggle"></button>
        </div>
        <div class="rh-reader-toc-list" data-role="bookmark-list"></div>
      </div>
    </div>
  </div>
  <header class="rh-reader-bar rh-reader-top">
    <button class="rh-reader-btn" data-action="exit" title="${esc(options.labels.exit)}" aria-label="${esc(options.labels.exit)}">${icon("exit")}<span class="rh-reader-btn-label">${esc(options.labels.exit)}</span></button>
    <div class="rh-reader-title" data-role="title"></div>
    <button class="rh-reader-btn" data-action="bookmarks" title="${esc(options.labels.bookmarks)}" aria-label="${esc(options.labels.bookmarks)}">${icon("bookmark")}<span class="rh-reader-btn-label">${esc(options.labels.bookmarks)}</span></button>
    <button class="rh-reader-btn" data-action="toc" title="${esc(options.labels.toc)}" aria-label="${esc(options.labels.toc)}">${icon("toc")}<span class="rh-reader-btn-label">${esc(options.labels.toc)}</span></button>
  </header>
  <footer class="rh-reader-bar rh-reader-bottom">
    <div class="rh-reader-progress">
      <input type="range" min="0" max="1000" value="0" data-role="progress" aria-label="${esc(options.labels.progress)}">
      <span data-role="progress-label"></span>
    </div>
    <div class="rh-reader-actions">
      <button class="rh-reader-btn" data-action="lock" title="${esc(options.labels.lock)}" aria-label="${esc(options.labels.lock)}">${icon("lock")}<span class="rh-reader-btn-label">${esc(options.labels.lock)}</span></button>
      <button class="rh-reader-btn" data-action="autoplay" title="${esc(options.labels.autoplay)}" aria-label="${esc(options.labels.autoplay)}">${icon("play")}<span class="rh-reader-btn-label">${esc(options.labels.autoplay)}</span></button>
      <button class="rh-reader-btn" data-action="speed" title="${esc(options.labels.speed)}" aria-label="${esc(options.labels.speed)}">${icon("speed")}<span class="rh-reader-value" data-role="speed"></span></button>
      <button class="rh-reader-btn" data-action="next-chapter" title="${esc(options.labels.nextChapter)}" aria-label="${esc(options.labels.nextChapter)}">${icon("next")}<span class="rh-reader-btn-label">${esc(options.labels.nextChapter)}</span></button>
      <button class="rh-reader-btn" data-action="crop" title="${esc(options.labels.crop)}" aria-label="${esc(options.labels.crop)}">${icon("crop")}<span class="rh-reader-btn-label">${esc(options.labels.crop)}</span></button>
      <button class="rh-reader-btn" data-action="mode" title="${esc(options.labels.mode)}" aria-label="${esc(options.labels.mode)}">${icon("mode")}<span class="rh-reader-value" data-role="mode"></span></button>
    </div>
  </footer>
</div>`;

  const container = must(root.querySelector<HTMLElement>(".rh-reader"), "container");
  const stage = must(root.querySelector<HTMLElement>('[data-role="stage"]'), "stage");
  const pageEl = must(root.querySelector<HTMLElement>('[data-role="page"]'), "page");
  const topBar = must(root.querySelector<HTMLElement>(".rh-reader-top"), "top bar");
  const bottomBar = must(root.querySelector<HTMLElement>(".rh-reader-bottom"), "bottom bar");
  const titleEl = must(root.querySelector<HTMLElement>('[data-role="title"]'), "title");
  const toastEl = must(root.querySelector<HTMLElement>('[data-role="toast"]'), "toast");
  const tocEl = must(root.querySelector<HTMLElement>('[data-role="toc"]'), "toc");
  const tocList = must(root.querySelector<HTMLElement>('[data-role="toc-list"]'), "toc list");
  const bookmarkPanel = must(root.querySelector<HTMLElement>('[data-role="bookmarks"]'), "bookmarks");
  const bookmarkList = must(
    root.querySelector<HTMLElement>('[data-role="bookmark-list"]'),
    "bookmark list",
  );
  const bookmarkToggle = must(
    root.querySelector<HTMLElement>('[data-role="bookmark-toggle"]'),
    "bookmark toggle",
  );
  const progress = must(root.querySelector<HTMLInputElement>('[data-role="progress"]'), "progress");
  const progressLabel = must(root.querySelector<HTMLElement>('[data-role="progress-label"]'), "progress label");
  const speedLabel = must(root.querySelector<HTMLElement>('[data-role="speed"]'), "speed label");
  const modeLabel = must(root.querySelector<HTMLElement>('[data-role="mode"]'), "mode label");

  let hideTimer: number | undefined;
  let autoplayTimer: number | undefined;
  let toastTimer: number | undefined;
  let longPressTimer: number | undefined;
  let longPressed = false;
  let tocOpen = false;
  let bookmarksOpen = false;
  let bookmarks: ReaderBookmark[] = (options.bookmarks ?? []).map((bookmark) => ({ ...bookmark }));
  let pointerStart: { x: number; y: number; time: number; scrollTop: number } | null = null;
  /** A4 手势：缩放 / 双指 / 双击 / 下拉退出。 */
  let zoom = 1;
  let zoomX = 0;
  let zoomY = 0;
  let pinching = false;
  let pinchStart: {
    distance: number;
    scale: number;
    cx: number;
    cy: number;
    tx: number;
    ty: number;
  } | null = null;
  let pendingTapTimer: number | undefined;
  let pendingTap: (() => void) | null = null;
  let lastTap: { time: number; x: number; y: number } | null = null;
  let panLast: { x: number; y: number } | null = null;
  let didPan = false;
  /** 单指横滑（左右分页）：pointercancel 之后仍能拿到触摸序列，用于翻页。 */
  let swipeStart: { x: number; y: number; time: number } | null = null;

  const snap = (): ReaderSnapshot => controller.snapshot();
  const loading = new Map<number, Promise<boolean>>();
  const initialPage = options.initial?.pageIndex ?? 0;
  /** 每次重绘递增，异步裁图完成后用它判断是否已经过期。 */
  let renderToken = 0;
  /** 双页拆分：当前显示左半（0）还是右半（1）。 */
  let splitHalf: 0 | 1 = 0;
  /** 条漫滚动同步期间忽略 scroll 事件，避免程序滚动反过来改页码。 */
  let webtoonSync = false;

  /** 手机紧凑模式（只显示图标 + 数值），断点与 CSS 的 max-width:640px / 矮视口一致。 */
  const compactMedia =
    typeof window.matchMedia === "function"
      ? window.matchMedia("(max-width: 640px), (max-height: 480px) and (pointer: coarse)")
      : null;
  const compactUi = (): boolean => compactMedia?.matches ?? false;
  const onCompactChange = (): void => renderBars();

  function pageHtml(page: ReaderPage, index = 0, eager = false): string {
    if (page.kind === "image" && page.content) {
      const url = esc(page.content);
      return `<img class="rh-reader-image" src="${url}" data-src="${url}" alt="" loading="${eager || index < 2 ? "eager" : "lazy"}">`;
    }
    if (page.kind === "html" && page.content) {
      // EPUB 正文已在 SDK 侧白名单清洗，这里直接渲染以保留标题 / 粗体 / 图片。
      return `<div class="rh-reader-html">${page.content}</div>`;
    }
    if (page.kind === "text") {
      const paragraphs = (page.content ?? "")
        .split(/\n{2,}/)
        .map((part) => part.trim())
        .filter(Boolean)
        .map((part) => `<p>${esc(part)}</p>`)
        .join("");
      return `<div class="rh-reader-text">${paragraphs}</div>`;
    }
    return `<div class="rh-reader-placeholder">${esc(page.caption ?? "")}</div>`;
  }

  /** 裁剪 / 拆分是异步的：先显示原图，拿到像素信息后原位替换成 canvas。 */
  async function enhanceImages(token: number): Promise<void> {
    const crop = snap().cropMode;
    if (crop !== "trim" && crop !== "split") return;
    const half = crop === "split" && snap().mode === "paged" ? splitHalf : null;
    const images = [...pageEl.querySelectorAll<HTMLImageElement>("img.rh-reader-image[data-src]")];
    for (const img of images) {
      const url = img.dataset.src ?? "";
      if (!url) continue;
      const info = await loadImageInfo(url);
      if (!info || token !== renderToken || !img.isConnected) continue;
      const wide = info.width / info.height >= SPREAD_RATIO;
      const canvas = drawImageRegion(info, info.box, half !== null && wide ? half : null);
      if (canvas) {
        if (wide) canvas.classList.add("is-spread");
        img.replaceWith(canvas);
      }
    }
  }

  /** 标记跨页大图（左右分页时占满宽度），并更新左右半页标识。 */
  async function markSpread(token: number): Promise<void> {
    if (snap().mode !== "paged") {
      container.dataset.spread = "";
      container.dataset.split = "";
      renderBars();
      return;
    }
    const page = controller.currentPage;
    if (page?.kind !== "image" || !page.content) {
      container.dataset.spread = "";
      container.dataset.split = "";
      renderBars();
      return;
    }
    const info = await loadImageInfo(page.content);
    if (!info || token !== renderToken) return;
    const wide = info.width / info.height >= SPREAD_RATIO;
    pageEl.querySelector("img.rh-reader-image")?.classList.toggle("is-spread", wide);
    container.dataset.spread = wide ? "true" : "false";
    container.dataset.split =
      wide && snap().cropMode === "split" ? (splitHalf === 0 ? "left" : "right") : "";
    renderBars();
  }

  /** 条漫：整章页面连续拼接，滚动位置决定当前页。 */
  function renderWebtoon(): void {
    const chapter = controller.currentChapter;
    const token = ++renderToken;
    const index = snap().pageIndex;
    pageEl.classList.remove("rh-reader-page-in");
    pageEl.innerHTML = `<div class="rh-reader-webtoon">${chapter.pages
      .map((page, pageIndex) => `<div class="rh-reader-webtoon-item" data-page-index="${pageIndex}">${pageHtml(page, pageIndex, true)}</div>`)
      .join("")}</div>`;
    const images = [...pageEl.querySelectorAll<HTMLImageElement>(".rh-reader-webtoon img")];
    const settle =
      snap().cropMode === "trim" || snap().cropMode === "split"
        // 裁图会换成 canvas：等它完成，布局尺寸才稳定。
        ? enhanceImages(token)
        : Promise.all(
          images.map((img) =>
            img.complete
              ? Promise.resolve()
              : new Promise<void>((resolve) => {
                img.addEventListener("load", () => resolve(), { once: true });
                img.addEventListener("error", () => resolve(), { once: true });
              })),
        );
    void Promise.race([
      settle,
      new Promise<void>((resolve) => window.setTimeout(resolve, 1200)),
    ]).then(() => {
      if (token === renderToken) scrollWebtoonToPage(index, false);
    });
  }

  function scrollWebtoonToPage(index: number, smooth: boolean): void {
    const item = pageEl.querySelector<HTMLElement>(
      `.rh-reader-webtoon-item[data-page-index="${index}"]`,
    );
    if (!item) return;
    webtoonSync = true;
    pageEl.scrollTo({ top: Math.max(0, item.offsetTop - 6), behavior: smooth ? "smooth" : "auto" });
    window.setTimeout(() => {
      webtoonSync = false;
    }, smooth ? 340 : 80);
  }

  function renderPage(): void {
    const token = ++renderToken;
    if (snap().mode === "webtoon") {
      renderWebtoon();
      return;
    }
    const page = controller.currentPage;
    pageEl.innerHTML = page ? pageHtml(page) : "";
    pageEl.classList.remove("rh-reader-page-in");
    void pageEl.offsetWidth;
    pageEl.classList.add("rh-reader-page-in");
    pageEl.scrollTop = 0;
    void enhanceImages(token);
    void markSpread(token);
  }

  function renderBars(): void {
    const state = snap();
    topBar.classList.toggle("is-hidden", !state.barsVisible);
    bottomBar.classList.toggle("is-hidden", !state.barsVisible);
    titleEl.textContent = controller.currentChapter.title;
    progress.value = String(Math.round(controller.progress() * 1000));
    const split = container.dataset.split;
    progressLabel.textContent = `${state.pageIndex + 1} / ${state.pageCount}${
      split ? ` · ${split === "left" ? options.labels.splitLeft : options.labels.splitRight}` : ""
    }`;
    speedLabel.textContent = compactUi()
      ? `${state.speed}×`
      : `${options.labels.speed} ${state.speed}×`;
    modeLabel.textContent = compactUi()
      ? options.labels.modeNames[state.mode]
      : `${options.labels.mode} · ${options.labels.modeNames[state.mode]}`;
    const autoplayBtn = root.querySelector<HTMLElement>('[data-action="autoplay"]');
    autoplayBtn?.classList.toggle("is-active", state.autoplay);
    const lockBtn = root.querySelector<HTMLElement>('[data-action="lock"]');
    lockBtn?.classList.toggle("is-active", state.locked);
    const cropBtn = root.querySelector<HTMLElement>('[data-action="crop"]');
    cropBtn?.classList.toggle("is-active", state.cropMode !== "off");
    if (bookmarksOpen) renderBookmarks();
  }

  function renderToc(): void {
    tocEl.classList.toggle("is-hidden", !tocOpen);
    if (!tocOpen) return;
    tocList.innerHTML = options.chapters
      .map((chapter, index) => {
        const current = index === snap().chapterIndex ? " is-current" : "";
        return `<button class="rh-reader-toc-item${current}" data-toc="${index}">${index + 1}. ${esc(chapter.title)}</button>`;
      })
      .join("");
  }

  function renderBookmarks(): void {
    bookmarkPanel.classList.toggle("is-hidden", !bookmarksOpen);
    if (!bookmarksOpen) return;
    const state = snap();
    const current = findBookmark(bookmarks, state.chapterIndex, state.pageIndex);
    bookmarkToggle.textContent = current
      ? options.labels.bookmarkRemove
      : options.labels.bookmarkAdd;
    bookmarkToggle.classList.toggle("is-active", Boolean(current));
    const ordered = sortBookmarks(bookmarks);
    bookmarkList.innerHTML = ordered.length
      ? ordered
          .map((bookmark) => {
            const active =
              bookmark.chapterIndex === state.chapterIndex &&
              bookmark.pageIndex === state.pageIndex
                ? " is-current"
                : "";
            const excerpt = bookmark.excerpt
              ? `<span class="rh-reader-bookmark-sub">${esc(bookmark.excerpt)}</span>`
              : "";
            return `<button class="rh-reader-toc-item${active}" data-bookmark-jump="${bookmark.chapterIndex}:${bookmark.pageIndex}">
<div>${esc(bookmark.chapterTitle)} · P${bookmark.pageIndex + 1}</div>${excerpt}</button>`;
          })
          .join("")
      : `<div class="rh-reader-bookmark-empty">${esc(options.labels.bookmarkEmpty)}</div>`;
  }

  function emitBookmarks(): void {
    options.onBookmarksChange?.(bookmarks.map((bookmark) => ({ ...bookmark })));
  }

  function toggleCurrentBookmark(): void {
    const state = snap();
    const before = bookmarks.length;
    bookmarks = toggleBookmark(bookmarks, {
      chapterIndex: state.chapterIndex,
      pageIndex: state.pageIndex,
      chapterTitle: controller.currentChapter.title,
      excerpt: pageExcerpt(controller.currentPage),
      createdAt: Date.now(),
    });
    const added = bookmarks.length > before;
    emitBookmarks();
    renderBookmarks();
    showToast(added ? options.labels.bookmarkAdded : options.labels.bookmarkRemoved);
  }

  function emitProgress(): void {
    options.onProgress?.(snap());
  }

  function emitChapter(): void {
    options.onChapterChange?.(snap().chapterIndex, controller.currentChapter);
  }

  /** 确保某一章已加载；同一章的并发请求复用同一个 Promise。 */
  function ensureChapter(index: number): Promise<boolean> {
    const chapter = options.chapters[index];
    if (!chapter?.lazy || !options.loadChapter) return Promise.resolve(true);
    const pending = loading.get(index);
    if (pending) return pending;
    const task = (async () => {
      try {
        const loaded = await options.loadChapter?.(index);
        if (!loaded) return false;
        controller.replaceChapter(index, loaded);
        return true;
      } catch {
        return false;
      } finally {
        loading.delete(index);
      }
    })();
    loading.set(index, task);
    return task;
  }

  /** 目录 / 下一章共用：切章后按需加载，再刷新界面。 */
  async function selectChapter(index: number, page = 0): Promise<void> {
    if (index < 0 || index >= options.chapters.length) return;
    if (!controller.setChapter(index, page)) return;
    splitHalf = 0;
    resetZoom();
    if (tocOpen) {
      tocOpen = false;
      renderToc();
    }
    // 先渲染占位（或已缓存内容），再等待按需加载，避免切章时停在上一条内容。
    renderPage();
    renderBars();
    const ok = await ensureChapter(index);
    renderPage();
    renderBars();
    emitChapter();
    emitProgress();
    scheduleHide();
    if (!ok) showToast(options.labels.chapterLoadFail);
  }

  function showToast(message: string, duration = 1600): void {
    toastEl.textContent = message;
    toastEl.classList.add("is-visible");
    if (toastTimer) window.clearTimeout(toastTimer);
    toastTimer = window.setTimeout(() => toastEl.classList.remove("is-visible"), duration);
  }

  function scheduleHide(): void {
    if (hideTimer) window.clearTimeout(hideTimer);
    const state = snap();
    if (state.locked || !state.barsVisible || tocOpen || bookmarksOpen) return;
    hideTimer = window.setTimeout(() => {
      controller.hideBars();
      renderBars();
    }, 3000);
  }

  function showBars(): void {
    controller.showBars();
    renderBars();
    scheduleHide();
  }

  function toggleBars(): void {
    if (snap().locked) return;
    controller.toggleBars();
    renderBars();
    scheduleHide();
  }

  function clampZoomPan(): void {
    const maxX = Math.max(0, pageEl.clientWidth * (zoom - 1));
    const maxY = Math.max(0, pageEl.clientHeight * (zoom - 1));
    zoomX = Math.min(0, Math.max(-maxX, zoomX));
    zoomY = Math.min(0, Math.max(-maxY, zoomY));
  }

  function applyZoom(): void {
    clampZoomPan();
    pageEl.style.transform =
      zoom <= 1.001 ? "" : `translate(${zoomX}px, ${zoomY}px) scale(${zoom})`;
    container.classList.toggle("is-zoomed", zoom > 1.001);
  }

  function resetZoom(): void {
    if (zoom === 1 && zoomX === 0 && zoomY === 0) return;
    zoom = 1;
    zoomX = 0;
    zoomY = 0;
    applyZoom();
  }

  /** 双击在页面坐标处放大到 2×，再次双击还原。 */
  function toggleZoomAt(clientX: number, clientY: number): void {
    if (snap().mode !== "paged") return;
    if (zoom > 1) {
      resetZoom();
      return;
    }
    const rect = stage.getBoundingClientRect();
    const x = clientX - rect.left;
    const y = clientY - rect.top;
    zoom = 2;
    zoomX = x - x * zoom;
    zoomY = y - y * zoom;
    applyZoom();
  }

  /** 缩放上限：与双指手势共用同一套 zoom / zoomX / zoomY。 */
  const ZOOM_MAX = 4;
  /** 滚轮灵敏度：100 的一格 ≈ 1.16×，触控板的小步长也能连续缩放。 */
  const WHEEL_SENSITIVITY = 0.0015;

  /** 以指针为锚点缩放：指针下的内容点在缩放前后保持不动。 */
  function zoomAt(next: number, clientX: number, clientY: number): void {
    const clamped = Math.min(ZOOM_MAX, Math.max(1, next));
    if (clamped <= 1.001) {
      resetZoom();
      return;
    }
    const rect = stage.getBoundingClientRect();
    const x = clientX - rect.left;
    const y = clientY - rect.top;
    const px = (x - zoomX) / zoom;
    const py = (y - zoomY) / zoom;
    zoom = clamped;
    zoomX = x - px * zoom;
    zoomY = y - py * zoom;
    applyZoom();
  }

  /** PC 滚轮缩放（2026-10-21）：仅分页模式；回到 1× 后把向下滚动交还默认行为。 */
  const onWheel = (event: WheelEvent): void => {
    if (snap().mode !== "paged" || isInteractive(event.target)) return;
    // deltaMode：0 像素 / 1 行 / 2 页，统一折算成像素。
    const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 400 : 1;
    const delta = event.deltaY * unit;
    if (delta === 0) return;
    if (zoom <= 1.001 && delta > 0) return;
    event.preventDefault();
    zoomAt(zoom * Math.exp(-delta * WHEEL_SENSITIVITY), event.clientX, event.clientY);
  };

  let autoplayBusy = false;

  async function autoplayStep(): Promise<void> {
    if (autoplayBusy) return;
    autoplayBusy = true;
    try {
      const step = controller.nextPage();
      if (step === "end") {
        controller.setAutoplay(false);
        renderBars();
        showToast(options.labels.lastPage);
        syncAutoplay();
        return;
      }
      splitHalf = 0;
      resetZoom();
      if (step === "chapter") {
        const ok = await ensureChapter(snap().chapterIndex);
        if (!ok) showToast(options.labels.chapterLoadFail);
        emitChapter();
      }
      renderPage();
      renderBars();
      emitProgress();
      scheduleHide();
    } finally {
      autoplayBusy = false;
    }
  }

  function syncAutoplay(): void {
    if (autoplayTimer) window.clearInterval(autoplayTimer);
    autoplayTimer = undefined;
    if (!snap().autoplay) return;
    const interval = Math.max(700, Math.round(4200 / snap().speed));
    autoplayTimer = window.setInterval(() => void autoplayStep(), interval);
  }

  /** 双页拆分：跨页大图先翻左 / 右半页，再进入下一页。 */
  async function maybeSplitTurn(direction: "prev" | "next"): Promise<boolean> {
    if (snap().mode !== "paged" || snap().cropMode !== "split") return false;
    const page = controller.currentPage;
    if (page?.kind !== "image" || !page.content) return false;
    const info = await loadImageInfo(page.content);
    if (!info || info.width / info.height < SPREAD_RATIO) return false;
    if (direction === "next" && splitHalf === 0) {
      splitHalf = 1;
      renderPage();
      renderBars();
      return true;
    }
    if (direction === "prev" && splitHalf === 1) {
      splitHalf = 0;
      renderPage();
      renderBars();
      return true;
    }
    splitHalf = 0;
    return false;
  }

  /** 条漫翻页 = 滚动到相邻页；章末再切章。 */
  async function webtoonTurn(direction: "prev" | "next"): Promise<void> {
    const step = direction === "next" ? controller.nextPage() : controller.prevPage();
    if (step === "end") {
      showToast(options.labels.lastPage);
      return;
    }
    if (step === "start") {
      showToast(options.labels.firstPage);
      return;
    }
    if (step === "chapter") {
      const ok = await ensureChapter(snap().chapterIndex);
      if (!ok) showToast(options.labels.chapterLoadFail);
      emitChapter();
      renderPage();
      scrollWebtoonToPage(
        direction === "prev" ? controller.pageCount - 1 : 0,
        false,
      );
      renderBars();
      emitProgress();
      scheduleHide();
      return;
    }
    scrollWebtoonToPage(snap().pageIndex, true);
    renderBars();
    emitProgress();
    scheduleHide();
  }

  async function turnPage(direction: "prev" | "next"): Promise<void> {
    if (snap().mode === "webtoon") {
      await webtoonTurn(direction);
      return;
    }
    if (await maybeSplitTurn(direction)) return;
    resetZoom();
    const step = direction === "next" ? controller.nextPage() : controller.prevPage();
    if (step === "end") {
      showToast(options.labels.lastPage);
      return;
    }
    if (step === "start") {
      showToast(options.labels.firstPage);
      return;
    }
    if (step === "chapter") {
      const ok = await ensureChapter(snap().chapterIndex);
      if (ok && direction === "prev") controller.setPage(controller.pageCount - 1);
      if (!ok) showToast(options.labels.chapterLoadFail);
      emitChapter();
    }
    renderPage();
    renderBars();
    emitProgress();
    scheduleHide();
  }

  function unlock(): void {
    controller.unlock();
    renderBars();
    showToast(options.labels.unlockedHint);
    scheduleHide();
  }

  function isInteractive(target: EventTarget | null): boolean {
    return target instanceof Element
      && !!target.closest(".rh-reader-bar, .rh-reader-toc, button, input");
  }

  /** 单击分区；左右分页下延迟 240ms 以便识别双击放大。 */
  function handleTap(clientX: number, clientY: number, zone: "prev" | "next" | "toggle"): void {
    if (snap().mode === "paged") {
      const now = Date.now();
      if (
        lastTap
        && now - lastTap.time < 300
        && Math.hypot(clientX - lastTap.x, clientY - lastTap.y) < 28
      ) {
        lastTap = null;
        if (pendingTapTimer) {
          window.clearTimeout(pendingTapTimer);
          pendingTapTimer = undefined;
        }
        pendingTap = null;
        toggleZoomAt(clientX, clientY);
        return;
      }
      lastTap = { time: now, x: clientX, y: clientY };
      if (pendingTapTimer) window.clearTimeout(pendingTapTimer);
      pendingTap = () => {
        if (snap().locked) {
          if (zone === "prev") void turnPage("prev");
          if (zone === "next") void turnPage("next");
          return;
        }
        if (zone === "toggle") toggleBars();
        else void turnPage(zone);
      };
      pendingTapTimer = window.setTimeout(() => {
        pendingTapTimer = undefined;
        const action = pendingTap;
        pendingTap = null;
        action?.();
      }, 240);
      return;
    }
    if (snap().locked) {
      if (zone === "prev") void turnPage("prev");
      if (zone === "next") void turnPage("next");
      return;
    }
    if (zone === "toggle") toggleBars();
    else void turnPage(zone);
  }

  const onPointerDown = (event: PointerEvent): void => {
    if (isInteractive(event.target)) return;
    if (!event.isPrimary || pinching) return;
    const rect = stage.getBoundingClientRect();
    const zone = resolveTapZone(event.clientX - rect.left, event.clientY - rect.top, rect.width, rect.height, snap().mode);
    pointerStart = { x: event.clientX, y: event.clientY, time: Date.now(), scrollTop: pageEl.scrollTop };
    longPressed = false;
    didPan = false;
    panLast = zoom > 1.001 ? { x: event.clientX, y: event.clientY } : null;
    if (snap().locked && zone === "toggle") {
      longPressTimer = window.setTimeout(() => {
        longPressTimer = undefined;
        longPressed = true;
        pointerStart = null;
        panLast = null;
        unlock();
      }, 600);
    }
  };

  const onPointerMove = (event: PointerEvent): void => {
    if (!panLast || zoom <= 1.001 || pinching) return;
    zoomX += event.clientX - panLast.x;
    zoomY += event.clientY - panLast.y;
    panLast = { x: event.clientX, y: event.clientY };
    didPan = true;
    applyZoom();
  };

  const onPointerUp = (event: PointerEvent): void => {
    if (longPressTimer) {
      window.clearTimeout(longPressTimer);
      longPressTimer = undefined;
    }
    if (longPressed) {
      longPressed = false;
      pointerStart = null;
      panLast = null;
      return;
    }
    if (pinching) {
      pointerStart = null;
      panLast = null;
      return;
    }
    if (didPan) {
      didPan = false;
      pointerStart = null;
      panLast = null;
      return;
    }
    panLast = null;
    if (!pointerStart) return;
    const dx = event.clientX - pointerStart.x;
    const dy = event.clientY - pointerStart.y;
    const dt = Date.now() - pointerStart.time;
    const start = pointerStart;
    pointerStart = null;
    // 下拉退出：从页面顶部向下拖过阈值（缩放状态下不触发，避免误退）。
    if (dy > 90 && Math.abs(dx) < 80 && zoom <= 1.001 && start.scrollTop <= 0) {
      options.onExit();
      return;
    }
    // 横滑翻页走 touch 事件（见 onTouchStart/onTouchEnd）：Chrome 在横向拖动时会先
    // pointercancel，用 pointerup 判断会被吞掉；这里只保留点击分区。
    if (Math.hypot(dx, dy) > 12 || dt > 600) return;
    const rect = stage.getBoundingClientRect();
    const zone = resolveTapZone(
      start.x - rect.left,
      start.y - rect.top,
      rect.width,
      rect.height,
      snap().mode,
    );
    handleTap(event.clientX, event.clientY, zone);
  };

  const onPointerCancel = (): void => {
    if (longPressTimer) window.clearTimeout(longPressTimer);
    longPressTimer = undefined;
    pointerStart = null;
    panLast = null;
    didPan = false;
    longPressed = false;
  };

  // 双指缩放：pan-y 已禁用浏览器自带 pinch-zoom，这里自己算比例与平移。
  const touchDistance = (touches: TouchList): number =>
    Math.hypot(
      touches[0].clientX - touches[1].clientX,
      touches[0].clientY - touches[1].clientY,
    );

  const onTouchStart = (event: TouchEvent): void => {
    if (snap().mode !== "paged") return;
    if (event.touches.length === 1) {
      const touch = event.touches[0];
      swipeStart = { x: touch.clientX, y: touch.clientY, time: Date.now() };
      return;
    }
    if (event.touches.length !== 2) return;
    swipeStart = null;
    if (pendingTapTimer) {
      window.clearTimeout(pendingTapTimer);
      pendingTapTimer = undefined;
    }
    pendingTap = null;
    pinching = true;
    pointerStart = null;
    const rect = stage.getBoundingClientRect();
    const a = event.touches[0];
    const b = event.touches[1];
    pinchStart = {
      distance: Math.max(1, touchDistance(event.touches)),
      scale: zoom,
      cx: (a.clientX + b.clientX) / 2 - rect.left,
      cy: (a.clientY + b.clientY) / 2 - rect.top,
      tx: zoomX,
      ty: zoomY,
    };
  };

  const onTouchMove = (event: TouchEvent): void => {
    if (!pinchStart || event.touches.length !== 2) return;
    event.preventDefault();
    const next = Math.min(
      4,
      Math.max(1, pinchStart.scale * (touchDistance(event.touches) / pinchStart.distance)),
    );
    zoom = next;
    zoomX = pinchStart.cx - (pinchStart.cx - pinchStart.tx) * (next / pinchStart.scale);
    zoomY = pinchStart.cy - (pinchStart.cy - pinchStart.ty) * (next / pinchStart.scale);
    applyZoom();
  };

  const onTouchEnd = (event: TouchEvent): void => {
    // 单指横滑翻页：Chrome 在拖动开始时就 pointercancel，只能靠 touchend 收尾。
    if (event.type === "touchend" && event.touches.length === 0 && swipeStart && !pinching) {
      const touch = event.changedTouches[0];
      const start = swipeStart;
      swipeStart = null;
      const dx = (touch?.clientX ?? start.x) - start.x;
      const dy = (touch?.clientY ?? start.y) - start.y;
      const dt = Date.now() - start.time;
      if (zoom <= 1.001 && Math.abs(dx) > 48 && Math.abs(dy) < 40 && dt < 600) {
        void turnPage(dx < 0 ? "next" : "prev");
        return;
      }
    }
    if (event.touches.length >= 2 || !pinchStart) return;
    pinchStart = null;
    pinching = false;
    if (zoom < 1.02) resetZoom();
    else applyZoom();
  };

  const onClick = (event: MouseEvent): void => {
    const target = event.target as HTMLElement | null;
    const tocItem = target?.closest<HTMLElement>("[data-toc]");
    if (tocItem) {
      const index = Number(tocItem.dataset.toc ?? "0");
      void selectChapter(index, 0);
      return;
    }
    const bookmarkItem = target?.closest<HTMLElement>("[data-bookmark-jump]");
    if (bookmarkItem) {
      const [chapterIndex, pageIndex] = (bookmarkItem.dataset.bookmarkJump ?? "0:0")
        .split(":")
        .map((part) => Number(part));
      bookmarksOpen = false;
      renderBookmarks();
      void selectChapter(chapterIndex ?? 0, pageIndex ?? 0);
      return;
    }
    const el = target?.closest<HTMLElement>("[data-action]");
    if (!el) return;
    switch (el.dataset.action) {
      case "exit":
        options.onExit();
        return;
      case "toc":
        bookmarksOpen = false;
        renderBookmarks();
        tocOpen = true;
        if (hideTimer) window.clearTimeout(hideTimer);
        renderToc();
        return;
      case "close-toc":
        tocOpen = false;
        renderToc();
        scheduleHide();
        return;
      case "bookmarks":
        tocOpen = false;
        renderToc();
        bookmarksOpen = true;
        if (hideTimer) window.clearTimeout(hideTimer);
        renderBookmarks();
        return;
      case "close-bookmarks":
        bookmarksOpen = false;
        renderBookmarks();
        scheduleHide();
        return;
      case "bookmark-toggle":
        toggleCurrentBookmark();
        return;
      case "lock": {
        const locked = controller.toggleLock();
        renderBars();
        showToast(locked ? options.labels.lockedHint : options.labels.unlockedHint, 2200);
        if (!locked) scheduleHide();
        return;
      }
      case "autoplay":
        controller.toggleAutoplay();
        renderBars();
        syncAutoplay();
        showToast(controller.snapshot().autoplay ? `${options.labels.autoplay} ${controller.snapshot().speed}×` : options.labels.autoplay);
        scheduleHide();
        return;
      case "speed": {
        const speed = controller.cycleSpeed();
        renderBars();
        syncAutoplay();
        showToast(`${options.labels.speed} ${speed}×`);
        scheduleHide();
        return;
      }
      case "next-chapter": {
        const next = snap().chapterIndex + 1;
        if (next >= options.chapters.length) {
          showToast(options.labels.lastChapter);
          return;
        }
        void selectChapter(next, 0);
        return;
      }
      case "crop": {
        const mode = controller.toggleCrop();
        container.dataset.crop = mode;
        splitHalf = 0;
        resetZoom();
        renderPage();
        renderBars();
        showToast(options.labels.cropModes[mode]);
        scheduleHide();
        return;
      }
      case "mode": {
        const mode = controller.cycleMode();
        container.dataset.mode = mode;
        splitHalf = 0;
        resetZoom();
        renderPage();
        renderBars();
        showToast(options.labels.modeNames[mode]);
        scheduleHide();
        return;
      }
      default:
        return;
    }
  };

  const onProgressInput = (): void => {
    if (hideTimer) window.clearTimeout(hideTimer);
    controller.setProgress(Number(progress.value) / 1000);
    splitHalf = 0;
    if (snap().mode === "webtoon") {
      scrollWebtoonToPage(snap().pageIndex, false);
    } else {
      resetZoom();
      pageEl.innerHTML = pageHtml(controller.currentPage);
      const token = ++renderToken;
      void enhanceImages(token);
      void markSpread(token);
    }
    renderBars();
    emitProgress();
  };

  const onProgressChange = (): void => scheduleHide();

  let scrollRaf = 0;
  const onPageScroll = (): void => {
    if (snap().mode !== "webtoon" || webtoonSync || scrollRaf) return;
    scrollRaf = requestAnimationFrame(() => {
      scrollRaf = 0;
      if (snap().mode !== "webtoon" || webtoonSync) return;
      const marker = pageEl.scrollTop + pageEl.clientHeight * 0.35;
      const items = pageEl.querySelectorAll<HTMLElement>(".rh-reader-webtoon-item");
      let index = 0;
      for (const item of items) {
        if (item.offsetTop <= marker) index = Number(item.dataset.pageIndex ?? "0");
        else break;
      }
      if (controller.setPage(index)) {
        renderBars();
        emitProgress();
      }
    });
  };

  stage.addEventListener("pointerdown", onPointerDown);
  stage.addEventListener("pointermove", onPointerMove);
  stage.addEventListener("pointerup", onPointerUp);
  stage.addEventListener("pointercancel", onPointerCancel);
  stage.addEventListener("touchstart", onTouchStart, { passive: true });
  stage.addEventListener("touchmove", onTouchMove, { passive: false });
  stage.addEventListener("touchend", onTouchEnd, { passive: true });
  stage.addEventListener("touchcancel", onTouchEnd, { passive: true });
  stage.addEventListener("wheel", onWheel, { passive: false });
  pageEl.addEventListener("scroll", onPageScroll, { passive: true });
  root.addEventListener("click", onClick);
  progress.addEventListener("input", onProgressInput);
  progress.addEventListener("change", onProgressChange);
  tocEl.addEventListener("pointerdown", (event) => event.stopPropagation());
  // 横竖屏切换 / 分屏拖动会跨过紧凑断点：只重绘上下框文案，不重排页面。
  if (compactMedia) {
    if (typeof compactMedia.addEventListener === "function") {
      compactMedia.addEventListener("change", onCompactChange);
    } else {
      compactMedia.addListener(onCompactChange);
    }
  }

  container.dataset.mode = snap().mode;
  container.dataset.crop = snap().cropMode;
  renderPage();
  renderBars();
  scheduleHide();

  // 首章可能是 lazy 占位（直接以章节索引打开远程来源时）；进入后立即补载。
  if (options.chapters[snap().chapterIndex]?.lazy) {
    void ensureChapter(snap().chapterIndex).then((ok) => {
      if (!ok) {
        showToast(options.labels.chapterLoadFail);
        return;
      }
      if (initialPage > 0) controller.setPage(initialPage);
      renderPage();
      renderBars();
      emitChapter();
      emitProgress();
    });
  }

  return {
    destroy(): void {
      if (hideTimer) window.clearTimeout(hideTimer);
      if (autoplayTimer) window.clearInterval(autoplayTimer);
      if (toastTimer) window.clearTimeout(toastTimer);
      if (longPressTimer) window.clearTimeout(longPressTimer);
      if (pendingTapTimer) window.clearTimeout(pendingTapTimer);
      if (scrollRaf) cancelAnimationFrame(scrollRaf);
      stage.removeEventListener("pointerdown", onPointerDown);
      stage.removeEventListener("pointermove", onPointerMove);
      stage.removeEventListener("pointerup", onPointerUp);
      stage.removeEventListener("pointercancel", onPointerCancel);
      stage.removeEventListener("touchstart", onTouchStart);
      stage.removeEventListener("touchmove", onTouchMove);
      stage.removeEventListener("touchend", onTouchEnd);
      stage.removeEventListener("touchcancel", onTouchEnd);
      stage.removeEventListener("wheel", onWheel);
      pageEl.removeEventListener("scroll", onPageScroll);
      root.removeEventListener("click", onClick);
      progress.removeEventListener("input", onProgressInput);
      progress.removeEventListener("change", onProgressChange);
      if (compactMedia) {
        if (typeof compactMedia.removeEventListener === "function") {
          compactMedia.removeEventListener("change", onCompactChange);
        } else {
          compactMedia.removeListener(onCompactChange);
        }
      }
      root.innerHTML = "";
    },
  };
}
