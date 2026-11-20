import {
  normalizeLegacyId,
  normalizeLegacyUpstream,
  type DeclarativeRule,
  type RegistryEntry,
  type SourceKind,
  type SourceManifest,
} from "@resource-hub/source-sdk";
import type { ReaderBookmark } from "@resource-hub/reader";

export interface LibraryEntry {
  workId: string;
  title: string;
  kind: SourceKind;
  author: string;
  cover?: string;
  chapterId?: string;
  chapterTitle?: string;
  updatedAt: number;
}

export interface SourceRecord {
  id: string;
  name: string;
  version: string;
  kind: SourceKind[];
  manifest: SourceManifest;
  /** 声明式规则；JS 源为空对象。 */
  rule: DeclarativeRule;
  /** JS 源脚本正文（`engine: "js"`）；声明式来源没有。 */
  script?: string;
  warnings: string[];
  importedAt: number;
}

export interface Progress {
  chapterId: string;
  chapterIndex: number;
  chapterTitle: string;
  pageIndex?: number;
  /** 视频播放位置（秒）与总时长；阅读器不使用。 */
  positionSec?: number;
  durationSec?: number;
  /** 最近一次阅读时间，用于「历史记录」排序。 */
  updatedAt?: number;
}

export interface RhState {
  library: LibraryEntry[];
  sources: SourceRecord[];
  /** 聚合仓库条目：安装时按内嵌规则或 `sourceUrl` 读取。 */
  registry: RegistryEntry[];
  progress: Record<string, Progress>;
  /** 每个作品的阅读器书签。 */
  bookmarks: Record<string, ReaderBookmark[]>;
  /** 是否已完成「localStorage → 宿主目录」的一次性迁移。 */
  hostMigrated?: boolean;
}

const KEY = "resource-hub.state.v1";

const EMPTY: RhState = {
  library: [],
  sources: [],
  registry: [],
  progress: {},
  bookmarks: {},
  hostMigrated: false,
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** 旧版 id 归一：来源记录自身 id 与 manifest.id 必须保持一致。 */
function migrateSource(source: SourceRecord): SourceRecord {
  const id = normalizeLegacyId(source.id ?? "");
  const manifestId = normalizeLegacyId(source.manifest?.id ?? id);
  if (id === source.id && manifestId === source.manifest?.id) return source;
  return { ...source, id, manifest: { ...source.manifest, id: manifestId } };
}

function migrateRegistryEntry(entry: RegistryEntry): RegistryEntry {
  const id = normalizeLegacyId(entry.id ?? "");
  const upstream = normalizeLegacyUpstream(entry.upstream ?? "manual");
  if (id === entry.id && upstream === entry.upstream) return entry;
  return { ...entry, id, upstream };
}

function dedupeById<T extends { id: string }>(items: T[]): T[] {
  const byId = new Map<string, T>();
  for (const item of items) byId.set(item.id, item);
  return [...byId.values()];
}

export function loadState(): RhState {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return { ...EMPTY };
    const parsed: unknown = JSON.parse(raw);
    if (!isRecord(parsed)) return { ...EMPTY };
    const sources = Array.isArray(parsed.sources)
      ? dedupeById((parsed.sources as SourceRecord[]).map(migrateSource))
      : [];
    const registry = Array.isArray(parsed.registry)
      ? dedupeById((parsed.registry as RegistryEntry[]).map(migrateRegistryEntry))
      : [];
    return {
      library: Array.isArray(parsed.library) ? (parsed.library as LibraryEntry[]) : [],
      sources,
      registry,
      progress: isRecord(parsed.progress) ? (parsed.progress as Record<string, Progress>) : {},
      bookmarks: isRecord(parsed.bookmarks)
        ? (parsed.bookmarks as Record<string, ReaderBookmark[]>)
        : {},
      hostMigrated: parsed.hostMigrated === true,
    };
  } catch {
    return { ...EMPTY };
  }
}

export function saveState(state: RhState): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(state));
  } catch {
    // 配额 / 隐私模式：界面继续用内存态，不弹错误打扰用户
  }
}

export function isSaved(state: RhState, workId: string): boolean {
  return state.library.some((entry) => entry.workId === workId);
}

export function toggleSaved(state: RhState, entry: Omit<LibraryEntry, "updatedAt">): RhState {
  const exists = state.library.some((item) => item.workId === entry.workId);
  const library = exists
    ? state.library.filter((item) => item.workId !== entry.workId)
    : [{ ...entry, updatedAt: Date.now() }, ...state.library];
  const next = { ...state, library };
  saveState(next);
  return next;
}

export function saveProgress(state: RhState, workId: string, progress: Progress): RhState {
  const stamped: Progress = { ...progress, updatedAt: Date.now() };
  const next: RhState = {
    ...state,
    progress: { ...state.progress, [workId]: stamped },
    library: state.library.map((item) =>
      item.workId === workId
        ? {
            ...item,
            chapterId: progress.chapterId,
            chapterTitle: progress.chapterTitle,
            updatedAt: Date.now(),
          }
        : item,
    ),
  };
  saveState(next);
  return next;
}

/** 覆盖某个作品的书签；空列表时删键，避免状态无限增长。 */
export function setBookmarks(
  state: RhState,
  workId: string,
  bookmarks: ReaderBookmark[],
): RhState {
  const next = { ...state.bookmarks };
  if (bookmarks.length > 0) next[workId] = bookmarks;
  else delete next[workId];
  const updated = { ...state, bookmarks: next };
  saveState(updated);
  return updated;
}

export function addSource(state: RhState, source: SourceRecord): RhState {
  const sources = [source, ...state.sources.filter((item) => item.id !== source.id)];
  const next = { ...state, sources };
  saveState(next);
  return next;
}

export function removeSource(state: RhState, id: string): RhState {
  const next = { ...state, sources: state.sources.filter((item) => item.id !== id) };
  saveState(next);
  return next;
}

/** 合并一份聚合仓库（同 id 保留新导入的版本）。 */
export function mergeRegistryEntries(state: RhState, entries: RegistryEntry[]): RhState {
  const byId = new Map(state.registry.map((entry) => [entry.id, entry]));
  for (const entry of entries) byId.set(entry.id, entry);
  const next = { ...state, registry: [...byId.values()] };
  saveState(next);
  return next;
}

// ── 书架数据包：导出 / 导入（B3，2026-10-28）──────────────────────────
//
// **只带用户数据**：喜欢（`library`）、阅读进度 / 历史（`progress`）、书签（`bookmarks`）。
// 不带来源与聚合仓库：来源是能重新导入的公共资产，且正文有许可边界
// （见 `docs/resource-hub-design.md` §9.6），不该混进个人书架备份。
//
// 导入是**合并**不是覆盖：同一 `workId` 以 `updatedAt` 更晚者为准，书签按
// 「章:页」并集去重。这样「导出备份 → 又读了几章 → 导回旧备份」不会把新进度冲掉。

export const LIBRARY_BUNDLE_KIND = "lunac-resource-hub-library";
export const LIBRARY_BUNDLE_VERSION = 1;

export interface LibraryBundle {
  kind: typeof LIBRARY_BUNDLE_KIND;
  version: typeof LIBRARY_BUNDLE_VERSION;
  /** 导出时刻（epoch ms）；只用于展示，不参与合并。 */
  exportedAt: number;
  library: LibraryEntry[];
  progress: Record<string, Progress>;
  bookmarks: Record<string, ReaderBookmark[]>;
}

export interface LibraryMergeResult {
  state: RhState;
  libraryAdded: number;
  libraryUpdated: number;
  progressMerged: number;
  bookmarksAdded: number;
}

const KINDS: SourceKind[] = ["comic", "novel", "anime", "tv"];

function bundleEntry(raw: unknown): LibraryEntry | null {
  if (!isRecord(raw)) return null;
  const workId = typeof raw.workId === "string" ? raw.workId.trim() : "";
  const title = typeof raw.title === "string" ? raw.title.trim() : "";
  if (!workId || !title) return null;
  const kind = KINDS.includes(raw.kind as SourceKind) ? (raw.kind as SourceKind) : "novel";
  return {
    workId,
    title,
    kind,
    author: typeof raw.author === "string" ? raw.author : "",
    ...(typeof raw.cover === "string" && raw.cover ? { cover: raw.cover } : {}),
    ...(typeof raw.chapterId === "string" && raw.chapterId ? { chapterId: raw.chapterId } : {}),
    ...(typeof raw.chapterTitle === "string" && raw.chapterTitle
      ? { chapterTitle: raw.chapterTitle }
      : {}),
    updatedAt: typeof raw.updatedAt === "number" && Number.isFinite(raw.updatedAt) ? raw.updatedAt : 0,
  };
}

function bundleProgress(raw: unknown): Progress | null {
  if (!isRecord(raw)) return null;
  if (typeof raw.chapterId !== "string" || !raw.chapterId) return null;
  if (typeof raw.chapterTitle !== "string") return null;
  if (typeof raw.chapterIndex !== "number" || !Number.isFinite(raw.chapterIndex)) return null;
  const out: Progress = {
    chapterId: raw.chapterId,
    chapterIndex: raw.chapterIndex,
    chapterTitle: raw.chapterTitle,
  };
  if (typeof raw.pageIndex === "number" && Number.isFinite(raw.pageIndex)) out.pageIndex = raw.pageIndex;
  if (typeof raw.positionSec === "number" && Number.isFinite(raw.positionSec)) {
    out.positionSec = raw.positionSec;
  }
  if (typeof raw.durationSec === "number" && Number.isFinite(raw.durationSec)) {
    out.durationSec = raw.durationSec;
  }
  if (typeof raw.updatedAt === "number" && Number.isFinite(raw.updatedAt)) out.updatedAt = raw.updatedAt;
  return out;
}

function bundleBookmark(raw: unknown): ReaderBookmark | null {
  if (!isRecord(raw)) return null;
  if (typeof raw.chapterIndex !== "number" || !Number.isFinite(raw.chapterIndex)) return null;
  if (typeof raw.pageIndex !== "number" || !Number.isFinite(raw.pageIndex)) return null;
  if (typeof raw.chapterTitle !== "string") return null;
  const out: ReaderBookmark = {
    chapterIndex: raw.chapterIndex,
    pageIndex: raw.pageIndex,
    chapterTitle: raw.chapterTitle,
    createdAt:
      typeof raw.createdAt === "number" && Number.isFinite(raw.createdAt) ? raw.createdAt : 0,
  };
  if (typeof raw.excerpt === "string" && raw.excerpt) out.excerpt = raw.excerpt;
  return out;
}

function bookmarkKeyOf(bookmark: ReaderBookmark): string {
  return `${bookmark.chapterIndex}:${bookmark.pageIndex}`;
}

export function buildLibraryBundle(state: RhState): LibraryBundle {
  return {
    kind: LIBRARY_BUNDLE_KIND,
    version: LIBRARY_BUNDLE_VERSION,
    exportedAt: Date.now(),
    library: state.library,
    progress: state.progress,
    bookmarks: state.bookmarks,
  };
}

/** 解析用户粘贴的备份文本。形状不对就抛**可读**错误（界面直接显示这句话）。 */
export function parseLibraryBundle(raw: string): LibraryBundle {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("不是合法的 JSON");
  }
  if (!isRecord(parsed)) throw new Error("数据包必须是一个 JSON 对象");
  if (parsed.kind !== LIBRARY_BUNDLE_KIND) {
    throw new Error(`不是 Lunac 书架数据包（kind=${String(parsed.kind)}）`);
  }
  if (parsed.version !== LIBRARY_BUNDLE_VERSION) {
    throw new Error(`数据包版本不支持：${String(parsed.version)}`);
  }
  // 单条坏数据只丢弃该条，不整体失败（备份是用户自己的东西，宁可多救回几条）。
  const seenWorks = new Set<string>();
  const library: LibraryEntry[] = [];
  for (const item of Array.isArray(parsed.library) ? parsed.library : []) {
    const entry = bundleEntry(item);
    if (!entry || seenWorks.has(entry.workId)) continue;
    seenWorks.add(entry.workId);
    library.push(entry);
  }
  const progress: Record<string, Progress> = {};
  if (isRecord(parsed.progress)) {
    for (const [workId, item] of Object.entries(parsed.progress)) {
      if (!workId) continue;
      const parsedProgress = bundleProgress(item);
      if (parsedProgress) progress[workId] = parsedProgress;
    }
  }
  const bookmarks: Record<string, ReaderBookmark[]> = {};
  if (isRecord(parsed.bookmarks)) {
    for (const [workId, items] of Object.entries(parsed.bookmarks)) {
      if (!workId || !Array.isArray(items)) continue;
      const seenPages = new Set<string>();
      const list: ReaderBookmark[] = [];
      for (const item of items) {
        const bookmark = bundleBookmark(item);
        if (!bookmark) continue;
        const key = bookmarkKeyOf(bookmark);
        if (seenPages.has(key)) continue;
        seenPages.add(key);
        list.push(bookmark);
      }
      if (list.length > 0) bookmarks[workId] = list;
    }
  }
  return {
    kind: LIBRARY_BUNDLE_KIND,
    version: LIBRARY_BUNDLE_VERSION,
    exportedAt: typeof parsed.exportedAt === "number" ? parsed.exportedAt : 0,
    library,
    progress,
    bookmarks,
  };
}

export function mergeLibraryBundle(state: RhState, bundle: LibraryBundle): LibraryMergeResult {
  let libraryAdded = 0;
  let libraryUpdated = 0;
  const library = [...state.library];
  for (const incoming of bundle.library) {
    const index = library.findIndex((item) => item.workId === incoming.workId);
    if (index < 0) {
      library.unshift(incoming);
      libraryAdded += 1;
    } else if ((incoming.updatedAt ?? 0) > (library[index].updatedAt ?? 0)) {
      library[index] = incoming;
      libraryUpdated += 1;
    }
  }

  let progressMerged = 0;
  const progress = { ...state.progress };
  for (const [workId, incoming] of Object.entries(bundle.progress)) {
    const current = progress[workId];
    if (!current || (incoming.updatedAt ?? 0) > (current.updatedAt ?? 0)) {
      progress[workId] = incoming;
      progressMerged += 1;
    }
  }

  let bookmarksAdded = 0;
  const bookmarks = { ...state.bookmarks };
  for (const [workId, incoming] of Object.entries(bundle.bookmarks)) {
    const merged = [...(bookmarks[workId] ?? [])];
    const seen = new Set(merged.map(bookmarkKeyOf));
    for (const bookmark of incoming) {
      const key = bookmarkKeyOf(bookmark);
      if (seen.has(key)) continue;
      seen.add(key);
      merged.push(bookmark);
      bookmarksAdded += 1;
    }
    if (merged.length > 0) bookmarks[workId] = merged;
  }

  const next: RhState = { ...state, library, progress, bookmarks };
  saveState(next);
  return { state: next, libraryAdded, libraryUpdated, progressMerged, bookmarksAdded };
}
