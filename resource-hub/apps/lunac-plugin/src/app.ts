import {
  RspError,
  compareVersion,
  convertMangaRule,
  htmlToText,
  looksLikeVideo,
  matchPinyin,
  parseEpub,
  parseRegistry,
  videoHasExplore,
  videoUnsupportedReason,
  validateSourceManifest,
  type Chapter,
  type ComicPage,
  type DeclarativeRule,
  type Playback,
  type PinyinEntry,
  type RegistryEntry,
  type ResourceSource,
  type SourceKind,
  type SourceManifest,
  type Work,
} from "@resource-hub/source-sdk";

import {
  mountReader,
  type ReaderChapter,
  type ReaderHandle,
  type ReaderSnapshot,
} from "@resource-hub/reader";

import { mountPlayer, stopAllPlayers, type PlayerHandle } from "./player.js";

import { DEMO_WORKS, demoWorkById, searchDemoWorks, type DemoWork } from "./demo-sources.js";
import { rhIcon } from "./icons.js";
import {
  canBrowse,
  createSourceHost,
  errorText,
  exploreSources,
  searchSources,
  sourceFor,
  type SourceSearchResult,
} from "./runtime.js";
import {
  addSource,
  buildLibraryBundle,
  isSaved,
  loadState,
  mergeLibraryBundle,
  mergeRegistryEntries,
  parseLibraryBundle,
  removeSource,
  saveProgress,
  saveState,
  setBookmarks,
  toggleSaved,
  type Progress,
  type RhState,
  type SourceRecord,
} from "./store.js";

// ── 宿主来源目录（A5，2026-10-18）────────────────────────────────────
//
// 来源的唯一持久化位置 = `<exe 根>\ModuleData\resource-hub\sources\<id>.json`，
// 由宿主 `resource_hub.rs` 管理；AI 的 `SourceInstall` 与这里的「导入来源」写同一处。
// localStorage 只作缓存 / 浏览器 harness 兜底（没有宿主时静默跳过）。
//
// 这里直接走 Tauri 内部 invoke（不引 `@tauri-apps/api` 依赖）：插件包保持零运行时依赖。
type TauriInvoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

function tauriInvoke(cmd: string, args?: Record<string, unknown>): Promise<unknown> | null {
  const internals = (globalThis as {
    __TAURI_INTERNALS__?: { invoke?: TauriInvoke };
  }).__TAURI_INTERNALS__;
  if (!internals?.invoke) return null;
  return internals.invoke(cmd, args);
}

/** 从宿主读来源并按**插件自己的 SDK** 校验；坏条目只跳过它自己。
 *  `null` = 没有宿主（浏览器 harness）；`[]` = 宿主可用但目录为空。 */
async function loadHostSources(): Promise<SourceRecord[] | null> {
  const call = tauriInvoke("resource_hub_sources");
  if (!call) return null;
  try {
    const raw = await call;
    if (!Array.isArray(raw)) return null;
    const out: SourceRecord[] = [];
    for (const item of raw) {
      try {
        if (!isRecord(item) || !isRecord(item.manifest)) continue;
        const manifest = validateSourceManifest(item.manifest);
        const script = typeof item.script === "string" ? item.script : "";
        if (manifest.engine === "js") {
          if (!script.trim()) continue;
        } else if (manifest.engine === "declarative") {
          if (!isRecord(item.rule)) continue;
        } else {
          continue;
        }
        const warnings = Array.isArray(item.warnings)
          ? item.warnings.filter((w): w is string => typeof w === "string")
          : [];
        out.push({
          id: manifest.id,
          name: manifest.name,
          version: manifest.version,
          kind: manifest.kind,
          manifest,
          rule: isRecord(item.rule) ? (item.rule as unknown as DeclarativeRule) : {},
          script: script.trim() ? script : undefined,
          warnings,
          importedAt:
            typeof item.installedAt === "number" ? item.installedAt * 1000 : Date.now(),
        });
      } catch {
        // 单条来源不合契约：跳过，不影响其它来源与插件本体。
      }
    }
    return out;
  } catch {
    return null;
  }
}

/** 写入宿主来源目录（浏览器 harness 里没有宿主 ⇒ 静默跳过）。 */
function persistHostSource(record: SourceRecord): void {
  void tauriInvoke("resource_hub_install_source", {
    source: {
      manifest: record.manifest,
      rule: record.rule,
      ...(record.script ? { script: record.script } : {}),
      warnings: record.warnings,
    },
    force: true,
  })?.catch(() => {});
}

function deleteHostSource(id: string): void {
  void tauriInvoke("resource_hub_delete_source", { id })?.catch(() => {});
}

/** 脚本自身的下载地址（jsDelivr / GitHub raw / 以 .js 结尾），不能当作站点 baseUrl。 */
function isScriptLocationUrl(url: string): boolean {
  return /\.(?:js|mjs)(?:\?|$)/i.test(url) ||
    /(?:^|\.)jsdelivr\.net\//i.test(url) ||
    /raw\.githubusercontent\.com\//i.test(url) ||
    /^https?:\/\/github\.com\//i.test(url);
}

/**
 * 从 JS 源脚本里推断**站点**根地址。
 *
 * 漫画源的 `url` 字段是脚本自身的更新地址（常见 jsDelivr / raw GitHub），
 * 不能当 baseUrl，否则封面、章节等相对地址会被拼到脚本仓库上。
 * 因此优先 `baseUrl` / `BASE_URL` / `webSite` 等站点字段，最后才看非脚本地址的 `url`。
 */
function guessSourceBaseUrl(script: string): string {
  const patterns = [
    /(?:static\s+)?base(?:Url|URL)\s*[:=]\s*["'](https?:\/\/[^"']+)["']/g,
    /get\s+base(?:Url|URL)\s*\(\s*\)\s*\{\s*return\s+["'](https?:\/\/[^"']+)["']/g,
    /(?:const|let|var)\s+BASE_URL\s*=\s*["'](https?:\/\/[^"']+)["']/g,
    /(?:static\s+)?(?:webSite|siteUrl|apiUrl|host)\s*[:=]\s*["'](https?:\/\/[^"']+)["']/g,
    /_webSite\s*:\s*["'](https?:\/\/[^"']+)["']/g,
    /\burl\s*[:=]\s*["'](https?:\/\/[^"']+)["']/g,
  ];
  for (const pattern of patterns) {
    for (const match of script.matchAll(pattern)) {
      const found = match[1];
      if (!found || isScriptLocationUrl(found)) continue;
      // `url` 兜底只接受站点根地址，避免把登录页 / 章节页当成 baseUrl。
      if (pattern === patterns[patterns.length - 1]) {
        try {
          const path = new URL(found).pathname;
          if (path && path !== "/") continue;
        } catch {
          continue;
        }
      }
      // `let baseUrl = ...` 这类局部变量不作为站点地址。
      const prefix = script.slice(Math.max(0, (match.index ?? 0) - 12), match.index ?? 0);
      if (/(?:let|const|var)\s+$/.test(prefix)) continue;
      return found;
    }
  }
  // 推断不出站点时宁可为空：执行器会原样保留脚本返回的绝对地址。
  return "";
}

/**
 * 脚本是否提供首页 / 探索列表：漫画源的 `explore = [...]`，
 * 或视频源的 `getHome` / `getClassList + getVideoList`。
 * 只做静态探测：真正执行失败会作为单来源错误展示，不影响其它来源。
 */
function scriptHasExplore(script: string): boolean {
  if (/\bexplore\s*=\s*\[/.test(script)) return true;
  return looksLikeVideo(script) && videoHasExplore(script);
}

/**
 * 修复旧版本装错的 JS 源：早期把脚本的下载地址（jsDelivr / raw GitHub）当成
 * `baseUrl`，导致封面、章节等相对地址拼到脚本仓库上。这里按脚本重新推断；
 * 已经指向真实站点的来源保持不动。
 */
function repairSourceRecord(record: SourceRecord): SourceRecord | null {
  if (record.manifest.engine !== "js" || !record.script) return null;
  const previous = record.manifest.baseUrl ?? "";
  const baseUrl = guessSourceBaseUrl(record.script);
  // 推断出站点：与现值不同就纠正（旧版可能填了脚本 CDN 或登录页）。
  // 推断不出：只清掉明显是脚本下载地址的 baseUrl，避免把相对地址拼到仓库上。
  const baseChanged = baseUrl
    ? baseUrl !== previous
    : Boolean(previous && isScriptLocationUrl(previous));
  // 旧清单没有 explore：脚本带首页 / 探索列表时补上能力，总览才会去拉。
  const capabilities: SourceManifest["capabilities"] =
    !record.manifest.capabilities.includes("explore") && scriptHasExplore(record.script)
      ? [...record.manifest.capabilities, "explore"]
      : record.manifest.capabilities;
  if (!baseChanged && capabilities === record.manifest.capabilities) return null;
  const homepage =
    record.manifest.homepage && !isScriptLocationUrl(record.manifest.homepage)
      ? record.manifest.homepage
      : baseUrl || undefined;
  return {
    ...record,
    manifest: {
      ...record.manifest,
      ...(baseChanged ? { baseUrl: baseUrl || undefined } : {}),
      homepage,
      capabilities,
    },
  };
}

/** 更新来源时保留用户的按来源设置与首次安装时间。 */
function keepSourceUserData(previous: SourceRecord, next: SourceRecord): SourceRecord {
  const settings = previous.manifest.settings;
  return {
    ...next,
    importedAt: previous.importedAt,
    manifest:
      settings && Object.keys(settings).length > 0
        ? { ...next.manifest, settings }
        : next.manifest,
  };
}

/** 从 `{` 开始找到配对的 `}`（跳过字符串内的括号）。 */
function readBalancedBlock(text: string, start: number): number {
  let depth = 0;
  let quote = "";
  let escaped = false;
  for (let i = start; i < text.length; i += 1) {
    const ch = text[i];
    if (quote) {
      if (escaped) escaped = false;
      else if (ch === "\\") escaped = true;
      else if (ch === quote) quote = "";
      continue;
    }
    if (ch === '"' || ch === "'" || ch === "`") {
      quote = ch;
      continue;
    }
    if (ch === "{") depth += 1;
    else if (ch === "}") {
      depth -= 1;
      if (depth === 0) return i;
    }
  }
  return -1;
}

/** 按顶层逗号切分对象体（保留嵌套对象 / 数组）。 */
function splitTopLevelEntries(body: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let start = 0;
  let quote = "";
  let escaped = false;
  for (let i = 0; i < body.length; i += 1) {
    const ch = body[i];
    if (quote) {
      if (escaped) escaped = false;
      else if (ch === "\\") escaped = true;
      else if (ch === quote) quote = "";
      continue;
    }
    if (ch === '"' || ch === "'" || ch === "`") {
      quote = ch;
      continue;
    }
    if (ch === "{" || ch === "[" || ch === "(") depth += 1;
    else if (ch === "}" || ch === "]" || ch === ")") depth -= 1;
    else if (ch === "," && depth === 0) {
      parts.push(body.slice(start, i));
      start = i + 1;
    }
  }
  parts.push(body.slice(start));
  return parts;
}

interface SourceSettingField {
  key: string;
  title: string;
  defaultValue: string;
}

/**
 * 解析漫画源脚本里的 `settings = { ... }`，只取域名 / 地址类字段。
 * 用于「按来源覆盖域名」，不尝试完整还原脚本的设置系统。
 */
function parseSourceSettingFields(script: string): SourceSettingField[] {
  const head = /\bsettings\s*[:=]\s*\{/.exec(script);
  if (!head) return [];
  const open = script.indexOf("{", head.index);
  if (open < 0) return [];
  const close = readBalancedBlock(script, open);
  if (close < 0) return [];
  const fields: SourceSettingField[] = [];
  for (const entry of splitTopLevelEntries(script.slice(open + 1, close))) {
    const keyMatch = /^\s*([A-Za-z_$][\w$]*)\s*:/.exec(entry);
    if (!keyMatch) continue;
    const key = keyMatch[1];
    const titleMatch = /title\s*:\s*["']([^"']*)["']/.exec(entry);
    const title = titleMatch?.[1] ?? key;
    if (!/domain|host|base|api|url|site/i.test(key) && !/域名|地址|接口|站点/i.test(title)) {
      continue;
    }
    const defaultMatch = /default\s*:\s*["']([^"']*)["']/.exec(entry);
    fields.push({ key, title, defaultValue: defaultMatch?.[1] ?? "" });
  }
  return fields;
}

/** 脚本里的脚本仓库 / 文件扩展名，不能当作站点域名候选。 */
const SCRIPT_HOST_BLACKLIST =
  /(?:^|\.)(?:jsdelivr\.net|github\.com|githubusercontent\.com|unpkg\.com|npmjs\.com)$/i;
const FILE_EXTENSION_LIKE =
  /\.(?:js|mjs|cjs|json|html?|css|md|txt|png|jpe?g|gif|webp|svg|mp4|m3u8)$/i;

/** 把脚本里出现的地址归一成 host[:port]；非法或脚本仓库地址返回空串。 */
function normalizeHost(raw: string): string {
  const value = raw.trim();
  if (!value) return "";
  try {
    const url = new URL(/^https?:\/\//i.test(value) ? value : `https://${value}`);
    if (SCRIPT_HOST_BLACKLIST.test(url.hostname)) return "";
    if (!/^(?:localhost|\d+\.\d+\.\d+\.\d+|(?:[a-z0-9-]+\.)+[a-z]{2,})$/i.test(url.hostname)) {
      return "";
    }
    return url.host.toLowerCase();
  } catch {
    return "";
  }
}

/** 从来源脚本里提取候选站点域名（显式 URL + 裸域名），供域名探活使用。 */
function extractDomainCandidates(script: string): string[] {
  const out = new Set<string>();
  const push = (raw: string): void => {
    const host = normalizeHost(raw);
    if (host) out.add(host);
  };
  for (const match of script.matchAll(/https?:\/\/([^/\s"'`)]+)/gi)) push(match[1]);
  for (const match of script.matchAll(/\b((?:[a-z0-9-]+\.)+[a-z]{2,})\b/gi)) {
    if (FILE_EXTENSION_LIKE.test(match[1])) continue;
    push(match[1]);
  }
  return [...out];
}

/** 本地 / 回环地址用 http，其余用 https。 */
function probeUrlFor(host: string): string {
  if (/^(?:localhost|127\.0\.0\.1)(?::\d+)?$/i.test(host)) return `http://${host}/`;
  return `https://${host}/`;
}

/** 按字段原格式返回候选值：原值带 scheme 就补 scheme，否则只写 host。 */
function formatHostValue(template: string, host: string): string {
  if (!/^https?:\/\//i.test(template)) return host;
  const scheme = /^http:\/\//i.test(template) || probeUrlFor(host).startsWith("http:") ? "http" : "https";
  return `${scheme}://${host}`;
}

/** 图片 / 视频走宿主媒体代理（`permissions: ["media.proxy"]`）；没有宿主时原样返回。
 *  `extra` 是**逐页**请求头（`comic.loadEp` 的 headers 与 `onImageLoad` 的覆盖值）：
 *  图片 CDN 的 Referer 经常与站点主域不同，只带清单头会被判盗链。 */
function proxyMediaUrl(
  url: string,
  manifest: SourceManifest,
  extra?: Record<string, string>,
): string {
  const media = (globalThis as {
    __lunac_host?: {
      media?: {
        proxy?: (
          url: string,
          options?: { referer?: string; userAgent?: string; cookie?: string },
        ) => string;
      };
    };
  }).__lunac_host?.media;
  if (!media?.proxy) return url;
  const headers = { ...(manifest.headers ?? {}), ...(extra ?? {}) };
  const referer = headers["Referer"] || headers["referer"] || manifest.baseUrl;
  const userAgent = headers["User-Agent"] || headers["user-agent"];
  const cookie = headers["Cookie"] || headers["cookie"];
  try {
    return media.proxy(url, { referer, userAgent, cookie });
  } catch {
    return url;
  }
}

/** 已经是宿主代理地址：再次代理会把它当成公网 URL 再包一层（必然 403）。 */
function isProxiedMedia(url: string): boolean {
  return /^https?:\/\/lunac-media\.localhost\//i.test(url) || url.startsWith("lunac-media://");
}

/** 给播放器用的幂等代理：HLS 清单 / 分片会反复经过它。 */
function mediaProxyResolver(manifest: SourceManifest): (url: string) => string {
  return (url) => {
    if (isProxiedMedia(url) || !/^https?:\/\//i.test(url)) return url;
    return proxyMediaUrl(url, manifest);
  };
}

/** EPUB 等二进制：经宿主媒体代理下载（`connect-src` 已放行 lunac-media）。 */
async function fetchMediaBytes(url: string, manifest: SourceManifest): Promise<Uint8Array> {
  let response: Response;
  try {
    response = await fetch(proxyMediaUrl(url, manifest));
  } catch (error) {
    throw new RspError(
      "network",
      `下载失败：${error instanceof Error ? error.message : String(error)}`,
      manifest.id,
    );
  }
  if (!response.ok) {
    throw new RspError("network", `下载失败：HTTP ${response.status}`, manifest.id);
  }
  return new Uint8Array(await response.arrayBuffer());
}

/** 纯文本 → 阅读器页：每页两段，长书也不会一次塞进一屏。 */
function splitTextPages(prefix: string, text: string): ReaderChapter["pages"] {
  const paragraphs = text.split(/\n{2,}/).map((part) => part.trim()).filter(Boolean);
  const pages: ReaderChapter["pages"] = [];
  for (let i = 0; i < paragraphs.length; i += 2) {
    pages.push({
      id: `${prefix}-p${pages.length + 1}`,
      kind: "text",
      content: paragraphs.slice(i, i + 2).join("\n\n"),
    });
  }
  return pages;
}

type Tab = "overview" | "library" | "sources";
type LibraryTab = "favorites" | "history";
type SortMode = "updated" | "status" | "chapters";
type FilterTab = "sources" | "tags" | "status";

type View =
  | { kind: "overview" }
  | { kind: "library" }
  | { kind: "library-io"; mode: IoMode }
  | { kind: "sources" }
  | { kind: "import" }
  | { kind: "detail"; workId: string }
  | { kind: "reader"; workId: string; chapterIndex: number }
  | { kind: "player"; workId: string; chapterIndex: number };

/** 书架数据包的两种方向（B3，2026-10-28）。 */
type IoMode = "export" | "import";

/** A7 二期（2026-10-28）：聊天卡片点进来时携带的作品。
 *  只带「再打开」所需的最小字段（来源 + 作品 id + 类型 + 标题 / 作者）。 */
export interface OpenWork {
  sourceId: string;
  workId: string;
  kind: string;
  title: string;
  author?: string;
}

/** 默认导入地址：独立公开规则仓库（jsDelivr 镜像；raw 备选见 registry/README）。 */
const GITHUB_REGISTRY_URL =
  "https://cdn.jsdelivr.net/gh/LythrumMoon/lunac-rules@main/index.json";

interface GridItem {
  workId: string;
  title: string;
  kind: SourceKind;
  author?: string;
  cover?: string;
  /** 未过媒体代理的封面原地址：加密图源的封面要靠它让源自己解（§待办 3-20）。 */
  coverRaw?: string;
  sub?: string;
  sourceName?: string;
  sourceId?: string;
  desc?: string;
  tags?: string[];
  /** 同一作品在多个来源命中时的来源数量。 */
  sourceCount?: number;
}

interface ResultItem extends GridItem {
  tags: string[];
  status: Work["status"];
  updatedAt: number;
  chapterCount: number;
}

interface SourceSwitchItem {
  key: string;
  sourceName: string;
  title: string;
  kind: SourceKind;
  cover?: string;
  chapterCount?: number;
  latestChapter?: string;
}

const ZH = {
  brand: "阅读",
  tabs: { overview: "总览", library: "书架", sources: "来源" },
  libraryTabs: { favorites: "喜欢", history: "历史记录" },
  searchPlaceholder: "搜索作品、作者或标签",
  search: "搜索",
  noResults: "没有匹配的作品，换个关键词试试。",
  searching: "正在搜索来源",
  loadingSuggestions: "正在搜索建议…",
  suggestionEmpty: "没有匹配的条目",
  loadingSources: "正在加载来源",
  loadingMore: "继续下滑加载更多",
  loadingPage: "正在加载下一页",
  overviewPreview: "全部来源为随机预览；用左下角「筛选」选择具体来源查看完整内容。",
  sourceError: "来源失败",
  allSources: "全部来源",
  allTags: "全部标签",
  sortModes: { updated: "更新时间", status: "完结度", chapters: "级数" } as Record<SortMode, string>,
  filter: "筛选",
  allKinds: "全部",
  moreFilters: "更多筛选",
  filterTitle: "筛选",
  filterTabs: { sources: "来源", tags: "标签", status: "完结/连载" } as Record<FilterTab, string>,
  filterApply: "确定筛选",
  filterClear: "清空",
  filterNoTags: "暂无可筛选的标签",
  filterCurrent: "当前来源",
  filterTagEmptySource: "该源暂无可筛选的标签",
  sortBy: "排序",
  close: "关闭",
  play: "播放",
  statusNames: { completed: "已完结", ongoing: "连载中", unknown: "未知" } as Record<string, string>,
  category: { comic: "漫画", novel: "小说", video: "视频" },
  sourcesCount: (count: number) => `${count} 源`,
  sourceSwitch: "换源",
  sourceSwitchTitle: "换源",
  sourceSwitchSearching: "正在搜索其他来源…",
  sourceSettings: "设置",
  sourceSettingsTitle: "来源设置",
  sourceSettingsHint: "域名与请求头留空表示使用默认值；保存后立即用于该来源。",
  sourceSettingsSave: "保存",
  sourceSettingsSaved: "来源设置已保存",
  sourceHeadersTitle: "请求头",
  sourceHeadersHint: "用于站点反爬；保存后随该来源的请求发送。",
  headerUserAgent: "User-Agent",
  headerReferer: "Referer",
  headerCookie: "Cookie",
  sourceBlockedHint: "站点拒绝了请求，可在来源「设置」里补充 Cookie / User-Agent",
  probeDomains: "自动探活",
  probing: "探测中…",
  probeOk: "已找到可用域名并填入，保存后生效",
  probeNone: "没有探测到可用域名",
  probeNoFields: "该来源没有可探测的域名设置",
  sourceSwitchEmpty: "没有找到其他来源",
  retry: "重试",
  latest: "最新",
  chapterUnit: "章",
  readingChapters: "正在读取章节…",
  currentSource: "当前",
  favoritesEmpty: "还没有喜欢的作品。打开作品后点「收藏」。",
  historyEmpty: "还没有阅读记录。",
  libraryExport: "导出书架",
  libraryImport: "导入书架",
  libraryIoExportTitle: "导出书架",
  libraryIoImportTitle: "导入书架数据",
  libraryIoExportHint: "下面是你的喜欢 / 阅读进度 / 书签（不含来源）。复制后自己保存；换机或重装时粘贴回来即可。",
  libraryIoImportHint: "粘贴之前导出的书架数据，点「合并导入」。同一作品的进度以更新的一条为准，旧备份不会覆盖新进度。",
  libraryIoCopy: "复制",
  libraryIoCopied: "已复制到剪贴板",
  libraryIoCopyFail: "复制失败，请手动全选文本框复制",
  libraryIoMerge: "合并导入",
  libraryIoMerged: (added: number, updated: number, progress: number, bookmarks: number) =>
    `已合并：新增 ${added} 本 · 更新 ${updated} 本 · 进度 ${progress} 条 · 书签 ${bookmarks} 条`,
  libraryIoMergeFail: "导入失败",
  libraryIoEmpty: "书架还是空的，没有可备份的数据。",
  importTitle: "导入规则",
  importUrlHint: "输入规则仓库或来源 JSON 的 URL（默认本仓库的 GitHub 规则仓库）；也可以直接粘贴 JSON 离线导入。",
  importGo: "导入",
  importing: "导入中…",
  loadingChapters: "正在加载章节…",
  loadingChapter: "正在加载章节内容…",
  noChapters: "没有章节",
  chapterLoadFail: "章节加载失败",
  open: "打开",
  save: "收藏",
  saved: "已收藏",
  chapters: "章节",
  back: "返回",
  prev: "上一章",
  next: "下一章",
  comicPending: "图片页渲染将在来源执行器接入后启用。当前只演示阅读骨架。",
  sourcesTitle: "来源管理",
  sourceSearchPlaceholder: "搜索来源名称 / id / 域名",
  sourceSearchEmpty: "没有匹配的来源",
  builtin: "内置演示来源",
  imported: "已导入来源",
  registry: "聚合仓库",
  registryOnly: "可一键安装到本地",
  registryImported: "聚合仓库已导入",
  install: "安装",
  installed: "已安装",
  installOk: "来源已安装，可参与搜索",
  updateAvailable: "可更新",
  update: "更新",
  updateAll: "全部更新",
  updating: "更新中…",
  updatingAll: "正在更新",
  updateOk: "来源已更新",
  updateFail: "更新失败",
  updateAllDone: "全部更新完成",
  updateAllNone: "没有可更新的仓库来源",
  installAll: "一键安装",
  removeAll: "一键移除",
  installingAll: "正在安装",
  installAllDone: "一键安装完成",
  installAllNone: "没有可安装的仓库来源",
  removeAllDone: "已一键移除仓库来源",
  removeAllNone: "没有已安装的仓库来源",
  confirmRemoveAll: "确定移除所有已安装的仓库来源？",
  failed: "失败",
  registryNoRule: "该条目没有可安装的规则或脚本地址",
  videoUnsupported: {
    basecode1: "不支持 BaseCode1 模板源（抓取逻辑在上游 App 内，未公开）",
    encrypted: "不支持加密源（codeID），需要上游 App 运行时解密",
    unknown: "不是可识别的视频扩展形态",
  },
  noLicense: "未声明许可",
  importHint: "输入规则仓库或来源 JSON 的 URL。",
  import: "导入规则",
  importOk: "规则已保存，可参与搜索",
  importFail: "导入失败",
  warnings: "提示",
  remove: "移除",
  emptyLibrary: "书架为空。打开作品后点「收藏」。",
  cardTitle: "阅读",
  cardSub: "漫画 · 小说 · 番剧 · 电视剧",
  cardOpen: "打开阅读",
  demo: "演示",
  progress: "进度",
  reader: {
    exit: "退出",
    toc: "目录",
    lock: "锁定",
    autoplay: "自动播放",
    nextChapter: "下一章",
    crop: "裁剪",
    mode: "阅读模式",
    speed: "速度",
    modeNames: { vertical: "上下", paged: "左右", webtoon: "条漫" },
    cropModes: { off: "原图", margin: "页边距", trim: "裁白边", split: "双页拆分" },
    splitLeft: "左半页",
    splitRight: "右半页",
    lockedHint: "已锁定 · 长按屏幕中心解锁",
    unlockedHint: "已解锁",
    firstPage: "已经是第一页",
    lastPage: "已经是最后一页",
    lastChapter: "已经是最后一章",
    tocTitle: "目录",
    close: "关闭",
    progress: "阅读进度",
    loadingChapter: "正在加载章节内容…",
    chapterLoadFail: "章节加载失败",
    bookmarks: "书签",
    bookmarksTitle: "书签",
    bookmarkAdd: "添加当前页书签",
    bookmarkRemove: "移除当前页书签",
    bookmarkAdded: "已添加书签",
    bookmarkRemoved: "已移除书签",
    bookmarkEmpty: "还没有书签",
  },
  player: {
    exit: "退出",
    line: "线路",
    loading: "正在加载视频…",
    error: "视频加载失败",
    retry: "重试",
    unsupported: "当前视频格式暂不支持",
    noSource: "没有可用的播放地址",
    codecUnsupported: "该线路编码不受支持（MKV / HEVC），请换线路或换源",
    copyLink: "复制直链",
    copied: "已复制直链，可用本机播放器打开",
  },
  kind: { comic: "漫画", novel: "小说", anime: "番剧", tv: "电视剧" } as Record<SourceKind, string>,
};

const EN = {
  brand: "Read",
  tabs: { overview: "Overview", library: "Library", sources: "Sources" },
  libraryTabs: { favorites: "Favorites", history: "History" },
  searchPlaceholder: "Search works, authors or tags",
  search: "Search",
  noResults: "No matching works. Try another keyword.",
  searching: "Searching sources",
  loadingSuggestions: "Searching suggestions…",
  suggestionEmpty: "No matching items",
  loadingSources: "Loading sources",
  loadingMore: "Scroll for more",
  loadingPage: "Loading more results",
  overviewPreview: "All sources is a random preview. Use the filter at the bottom-left to browse one source.",
  sourceError: "Source error",
  allSources: "All sources",
  allTags: "All tags",
  sortModes: { updated: "Updated", status: "Status", chapters: "Chapters" } as Record<SortMode, string>,
  filter: "Filter",
  allKinds: "All",
  moreFilters: "More filters",
  filterTitle: "Filter",
  filterTabs: { sources: "Sources", tags: "Tags", status: "Status" } as Record<FilterTab, string>,
  filterApply: "Apply",
  filterClear: "Clear",
  filterNoTags: "No tags to filter",
  filterCurrent: "Current sources",
  filterTagEmptySource: "No tags for this source",
  sortBy: "Sort",
  close: "Close",
  play: "Play",
  statusNames: { completed: "Completed", ongoing: "Ongoing", unknown: "Unknown" } as Record<string, string>,
  category: { comic: "Comic", novel: "Novel", video: "Video" },
  sourcesCount: (count: number) => `${count} sources`,
  sourceSwitch: "Change source",
  sourceSwitchTitle: "Change source",
  sourceSwitchSearching: "Searching other sources…",
  sourceSettings: "Settings",
  sourceSettingsTitle: "Source settings",
  sourceSettingsHint: "Leave domains and headers blank for defaults. Saved values apply immediately.",
  sourceSettingsSave: "Save",
  sourceSettingsSaved: "Source settings saved",
  sourceHeadersTitle: "Request headers",
  sourceHeadersHint: "Used for anti-bot sites; sent with this source's requests.",
  headerUserAgent: "User-Agent",
  headerReferer: "Referer",
  headerCookie: "Cookie",
  sourceBlockedHint: "The site rejected the request; add Cookie / User-Agent in source settings",
  probeDomains: "Auto probe",
  probing: "Probing…",
  probeOk: "Reachable domain found; save to apply",
  probeNone: "No reachable domain found",
  probeNoFields: "This source has no domain settings to probe",
  sourceSwitchEmpty: "No other sources found",
  retry: "Retry",
  latest: "Latest",
  chapterUnit: "chapters",
  readingChapters: "Reading chapters…",
  currentSource: "Current",
  favoritesEmpty: "No favorites yet. Open a work and tap Save.",
  historyEmpty: "No reading history yet.",
  libraryExport: "Export library",
  libraryImport: "Import library",
  libraryIoExportTitle: "Export library",
  libraryIoImportTitle: "Import library data",
  libraryIoExportHint: "Your favorites, reading progress and bookmarks (sources excluded). Copy and keep it; paste it back after switching machines or reinstalling.",
  libraryIoImportHint: "Paste a previously exported library backup and tap Merge. For the same work the newer progress wins, so an old backup never overwrites newer progress.",
  libraryIoCopy: "Copy",
  libraryIoCopied: "Copied to clipboard",
  libraryIoCopyFail: "Copy failed — select the text and copy manually",
  libraryIoMerge: "Merge",
  libraryIoMerged: (added: number, updated: number, progress: number, bookmarks: number) =>
    `Merged: ${added} new · ${updated} updated · ${progress} progress · ${bookmarks} bookmarks`,
  libraryIoMergeFail: "Import failed",
  libraryIoEmpty: "The library is empty — nothing to back up.",
  importTitle: "Import rules",
  importUrlHint: "Enter a registry or source JSON URL (defaults to this repository's GitHub registry), or paste JSON for offline import.",
  importGo: "Import",
  importing: "Importing…",
  loadingChapters: "Loading chapters…",
  loadingChapter: "Loading chapter…",
  noChapters: "No chapters",
  chapterLoadFail: "Failed to load chapter",
  open: "Open",
  save: "Save",
  saved: "Saved",
  chapters: "Chapters",
  back: "Back",
  prev: "Previous",
  next: "Next",
  comicPending: "Image rendering will be enabled with the source engine. This is the reader skeleton.",
  sourcesTitle: "Sources",
  sourceSearchPlaceholder: "Search name / id / domain",
  sourceSearchEmpty: "No matching sources",
  builtin: "Built-in demo sources",
  imported: "Imported sources",
  registry: "Registry",
  registryOnly: "Installable locally",
  registryImported: "Registry imported",
  install: "Install",
  installed: "Installed",
  installOk: "Source installed; it can be searched now",
  updateAvailable: "Update available",
  update: "Update",
  updateAll: "Update all",
  updating: "Updating…",
  updatingAll: "Updating",
  updateOk: "Source updated",
  updateFail: "Update failed",
  updateAllDone: "Update all finished",
  updateAllNone: "No updatable registry sources",
  installAll: "Install all",
  removeAll: "Remove all",
  installingAll: "Installing",
  installAllDone: "Install all finished",
  installAllNone: "No installable registry sources",
  removeAllDone: "Removed registry sources",
  removeAllNone: "No installed registry sources",
  confirmRemoveAll: "Remove all installed registry sources?",
  failed: "failed",
  registryNoRule: "This entry has no installable rule or script URL",
  videoUnsupported: {
    basecode1: "BaseCode1 template sources are not supported (scraping lives in the upstream app)",
    encrypted: "Encrypted sources (codeID) are not supported; they need the upstream runtime",
    unknown: "Not a recognizable video extension",
  },
  noLicense: "No license declared",
  importHint: "Enter a registry or source JSON URL.",
  import: "Import rules",
  importOk: "Rule saved; it can be searched now",
  importFail: "Import failed",
  warnings: "Warnings",
  remove: "Remove",
  emptyLibrary: "Library is empty. Open a work and tap Save.",
  cardTitle: "Read",
  cardSub: "Comics · Novels · Anime · TV",
  cardOpen: "Open Read",
  demo: "Demo",
  progress: "Progress",
  reader: {
    exit: "Exit",
    toc: "Contents",
    lock: "Lock",
    autoplay: "Auto",
    nextChapter: "Next chapter",
    crop: "Crop",
    mode: "Mode",
    speed: "Speed",
    modeNames: { vertical: "Vertical", paged: "Paged", webtoon: "Webtoon" },
    cropModes: { off: "Original", margin: "Margins", trim: "Trim", split: "Split" },
    splitLeft: "Left half",
    splitRight: "Right half",
    lockedHint: "Locked · long-press center to unlock",
    unlockedHint: "Unlocked",
    firstPage: "First page",
    lastPage: "Last page",
    lastChapter: "Last chapter",
    tocTitle: "Contents",
    close: "Close",
    progress: "Reading progress",
    loadingChapter: "Loading chapter…",
    chapterLoadFail: "Failed to load chapter",
    bookmarks: "Bookmarks",
    bookmarksTitle: "Bookmarks",
    bookmarkAdd: "Bookmark this page",
    bookmarkRemove: "Remove this bookmark",
    bookmarkAdded: "Bookmark added",
    bookmarkRemoved: "Bookmark removed",
    bookmarkEmpty: "No bookmarks yet",
  },
  player: {
    exit: "Exit",
    line: "Line",
    loading: "Loading video…",
    error: "Failed to load video",
    retry: "Retry",
    unsupported: "This video format is not supported yet",
    noSource: "No playable source",
    codecUnsupported: "This line uses an unsupported codec (MKV / HEVC). Switch lines or sources.",
    copyLink: "Copy direct link",
    copied: "Direct link copied; open it in a local player",
  },
  kind: { comic: "Comic", novel: "Novel", anime: "Anime", tv: "TV" } as Record<SourceKind, string>,
};

type Dict = typeof ZH;

const LANG: Dict = navigator.language.toLowerCase().startsWith("zh") ? ZH : EN;

function esc(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function kindTag(kind: SourceKind): string {
  return `<span class="rh-tag">${esc(LANG.kind[kind])}</span>`;
}

function workKey(work: Work): string {
  return `${work.ref.sourceId}::${work.ref.workId}`;
}

/** 标题归一化：去掉空白、标点与符号，用于跨源合并 / 换源匹配。 */
function normalizeTitle(value: string): string {
  return value.toLowerCase().replace(/[\s\p{P}\p{S}]+/gu, "");
}

/** Fisher–Yates 洗牌；总览 / 预览按随机来源顺序取数，避免总是被前几个源占满。 */
function shuffle<T>(items: T[]): T[] {
  const out = [...items];
  for (let i = out.length - 1; i > 0; i -= 1) {
    const j = Math.floor(Math.random() * (i + 1));
    [out[i], out[j]] = [out[j], out[i]];
  }
  return out;
}

/** 搜索预览的相关度：标题全等 < 前缀 < 包含 < 作者命中 < 拼音前缀 < 拼音包含。 */
function suggestionRank(
  item: { title: string; author?: string },
  query: string,
  pinyinRank = 0,
): number {
  const title = item.title.toLowerCase();
  if (title === query) return 0;
  if (title.startsWith(query)) return 1;
  if (title.includes(query)) return 2;
  if ((item.author ?? "").toLowerCase().includes(query)) return 3;
  if (pinyinRank > 0) return 4 + pinyinRank;
  return 7;
}

function sameWorkTitle(a: string, b: string): boolean {
  const left = normalizeTitle(a);
  const right = normalizeTitle(b);
  if (!left || !right) return false;
  if (left === right) return true;
  return left.length >= 4 && right.length >= 4 && (left.includes(right) || right.includes(left));
}

/** 来源类别：漫画 / 小说 / 视频（anime + tv 合并为视频）。 */
function mediaCategory(kind: SourceKind): "comic" | "novel" | "video" {
  if (kind === "comic") return "comic";
  if (kind === "novel") return "novel";
  return "video";
}

function sameMediaKind(a: SourceKind, b: SourceKind): boolean {
  return mediaCategory(a) === mediaCategory(b);
}

function sourceCategories(kind: SourceKind[]): string[] {
  const categories: string[] = [];
  if (kind.includes("comic")) categories.push(LANG.category.comic);
  if (kind.includes("novel")) categories.push(LANG.category.novel);
  if (kind.includes("anime") || kind.includes("tv")) categories.push(LANG.category.video);
  return categories;
}

function chapterNumber(title: string): number | null {
  const match = title.match(/(\d+(?:\.\d+)?)/);
  return match ? Number(match[1]) : null;
}

/** 取最大集数 / 章节标题；列表顺序不一定是旧→新，因此按标题数字判断。 */
function latestChapterTitle(chapters: Chapter[]): string | undefined {
  if (chapters.length === 0) return undefined;
  let best = chapters[chapters.length - 1];
  let bestNumber = chapterNumber(best.title);
  for (const chapter of chapters) {
    const value = chapterNumber(chapter.title);
    if (value !== null && (bestNumber === null || value > bestNumber)) {
      best = chapter;
      bestNumber = value;
    }
  }
  return best.title;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

const STYLE = `
.rh-app{position:relative;height:100%;max-height:100vh;display:flex;flex-direction:column;min-height:0;overflow:hidden;color:var(--text);font:inherit}
.rh-app *{box-sizing:border-box}
.rh-tabs{display:flex;align-items:center;justify-content:space-around;gap:4px;padding:6px 12px calc(6px + env(safe-area-inset-bottom));border-top:1px solid var(--border-glass);background:rgba(var(--surface-rgb,22,24,27),.92);flex:none;position:sticky;bottom:0;z-index:5}
.rh-tab{display:inline-flex;align-items:center;justify-content:center;width:44px;height:36px;border:1px solid transparent;background:transparent;color:var(--text-dim);border-radius:8px;cursor:pointer}
.rh-tab:hover{background:rgba(var(--accent-rgb),.06);color:var(--text)}
.rh-tab.active{border-color:var(--border-glass);background:rgba(var(--accent-rgb),.10);color:var(--text)}
.rh-main{flex:1;min-height:0;overflow-y:auto;padding:16px}
.rh-toolbar{display:flex;flex-direction:column;gap:8px;margin-bottom:12px}
.rh-toolbar-row{display:flex;align-items:center;gap:10px;flex-wrap:wrap}
.rh-sort-chips{flex:1;min-width:0}
.rh-sort-label{flex:none;color:var(--text-muted);font-size:.78rem}
.rh-filter-badge{display:inline-flex;align-items:center;justify-content:center;min-width:16px;height:16px;padding:0 4px;border-radius:999px;background:rgba(var(--accent-rgb),.18);color:var(--text);font-size:.68rem;line-height:1}
.rh-chips{display:flex;gap:6px;overflow-x:auto;padding-bottom:2px}
.rh-chip{flex:none;border:1px solid var(--border-glass);background:transparent;color:var(--text-dim);border-radius:999px;padding:4px 10px;font:inherit;font-size:.78rem;cursor:pointer}
.rh-chip:hover{background:rgba(var(--accent-rgb),.06);color:var(--text)}
.rh-chip.active{border-color:rgba(var(--accent-rgb),.40);background:rgba(var(--accent-rgb),.12);color:var(--text)}
.rh-library-head{display:flex;gap:6px;margin-bottom:14px}
/* 书架右侧的导入 / 导出（B3）：靠右成组，和左侧两个图标标签分开。 */
.rh-library-io{margin-left:auto;display:flex;gap:6px}
.rh-icon-tab{display:inline-flex;align-items:center;justify-content:center;width:42px;height:34px;border:1px solid transparent;background:transparent;color:var(--text-dim);border-radius:8px;cursor:pointer}
.rh-icon-tab:hover{background:rgba(var(--accent-rgb),.06);color:var(--text)}
.rh-icon-tab.active{border-color:var(--border-glass);background:rgba(var(--accent-rgb),.10);color:var(--text)}
.rh-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(132px,1fr));gap:12px}
.rh-grid-card{display:flex;flex-direction:column;gap:6px;border:1px solid var(--border-glass);border-radius:10px;background:rgba(var(--ink-rgb),.025);padding:8px;cursor:pointer;text-align:left;color:var(--text);font:inherit;min-width:0;content-visibility:auto;contain-intrinsic-size:auto 248px}
.rh-grid-card:hover{background:rgba(var(--accent-rgb),.06)}
.rh-cover{position:relative;aspect-ratio:2/3;border-radius:7px;overflow:hidden;background:rgba(var(--ink-rgb),.06);display:flex;align-items:center;justify-content:center}
.rh-cover img{width:100%;height:100%;object-fit:cover;display:block}
.rh-cover-fallback{font-size:1.4rem;color:var(--text-muted)}
.rh-grid-title{font-size:.86rem;font-weight:600;line-height:1.3;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-grid-meta{font-size:.74rem;color:var(--text-muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-grid-tags{display:flex;flex-wrap:wrap;gap:4px;max-height:20px;overflow:hidden}
.rh-detail-tags{display:flex;flex-wrap:wrap;gap:6px;margin:6px 0}
.rh-drawer-backdrop{position:absolute;inset:0;background:rgba(var(--shade-rgb,0,0,0),calc(.35 * var(--shade-scale,1)));opacity:0;pointer-events:none;transition:opacity .25s;z-index:20}
.rh-drawer-backdrop.visible{opacity:1;pointer-events:auto}
.rh-drawer{position:absolute;left:14px;bottom:calc(96px + env(safe-area-inset-bottom));width:min(340px,calc(100% - 28px));max-height:min(62%,460px);display:flex;flex-direction:column;background:var(--surface-glass);backdrop-filter:blur(16px);-webkit-backdrop-filter:blur(16px);border:1px solid var(--border-glass);border-radius:12px;box-shadow:0 12px 32px rgba(var(--shade-rgb,0,0,0),calc(.32 * var(--shade-scale,1)));transform:translateY(10px) scale(.98);opacity:0;transition:transform .2s ease,opacity .2s ease;pointer-events:none;z-index:21}
.rh-drawer.visible{transform:none;opacity:1;pointer-events:auto}
.rh-drawer-current{display:flex;align-items:baseline;gap:8px;padding:8px 12px 0;font-size:.74rem;color:var(--text-muted);flex:none}
.rh-drawer-current-value{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--text)}
.rh-filter-group{margin-bottom:10px}
.rh-filter-group-head{display:flex;align-items:center;justify-content:space-between;gap:8px;margin:0 0 6px;font-size:.74rem;color:var(--text-muted)}
.rh-filter-dock{position:absolute;left:14px;bottom:calc(52px + env(safe-area-inset-bottom));z-index:6;display:flex;align-items:center;gap:6px;max-width:calc(100% - 28px);overflow-x:auto;padding-bottom:2px}
.rh-filter-fab{flex:none;display:inline-flex;align-items:center;gap:6px;max-width:calc(100% - 28px);padding:7px 12px;border:1px solid var(--border-glass);border-radius:999px;background:var(--surface-glass);backdrop-filter:blur(12px);-webkit-backdrop-filter:blur(12px);color:var(--text-dim);font:inherit;font-size:.78rem;cursor:pointer;box-shadow:0 6px 18px rgba(var(--shade-rgb,0,0,0),calc(.22 * var(--shade-scale,1)))}
.rh-filter-fab:hover{color:var(--text);border-color:rgba(var(--accent-rgb),.40)}
.rh-filter-fab-label{min-width:0;max-width:180px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-filter-pill{flex:none;display:inline-flex;align-items:center;gap:5px;padding:5px 7px 5px 10px;border:1px solid rgba(var(--accent-rgb),.45);border-radius:999px;background:rgba(var(--accent-rgb),.14);color:var(--text);font:inherit;font-size:.76rem;cursor:pointer}
.rh-filter-pill:hover{background:rgba(var(--accent-rgb),.24)}
.rh-filter-pill .rh-icon{opacity:.7}
.rh-drawer-head{display:flex;align-items:center;justify-content:space-between;padding:10px 12px;border-bottom:1px solid var(--border-glass);flex:none;font-size:.8rem;font-weight:600;color:var(--text-dim);letter-spacing:.04em;text-transform:uppercase}
.rh-icon-btn{display:inline-flex;align-items:center;justify-content:center;border:none;background:none;color:var(--text-dim);border-radius:4px;padding:3px 6px;cursor:pointer}
.rh-icon-btn:hover{background:rgba(var(--ink-rgb),.08);color:var(--text)}
.rh-filter-tabs{display:flex;gap:6px;padding:10px 12px 8px;flex:none}
.rh-filter-tab{padding:4px 14px;font-size:.72rem;color:var(--text-dim);background:transparent;border:1px solid var(--border-glass);border-radius:999px;cursor:pointer;font-family:inherit}
.rh-filter-tab:hover{color:var(--text);border-color:rgba(var(--accent-rgb),.40)}
.rh-filter-tab.on{color:var(--text);background:rgba(var(--accent-rgb),.12);border-color:rgba(var(--accent-rgb),.40)}
.rh-drawer-body{flex:1;min-height:0;overflow-y:auto;padding:6px 12px 12px}
.rh-drawer-foot{display:flex;gap:8px;justify-content:flex-end;padding:10px 12px;border-top:1px solid var(--border-glass);flex:none}
.rh-opt{display:flex;align-items:center;gap:8px;width:100%;padding:8px 6px;border:none;background:transparent;color:var(--text-dim);border-radius:6px;cursor:pointer;font:inherit;text-align:left}
.rh-opt:hover{background:rgba(var(--accent-rgb),.06);color:var(--text)}
.rh-opt.on{color:var(--text)}
.rh-opt-name{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-opt-meta{flex:none;font-size:.72rem;color:var(--text-muted)}
.rh-opt-box{position:relative;flex:none;display:inline-flex;align-items:center;justify-content:center;width:14px;height:14px;border:1px solid var(--border-glass);border-radius:4px}
.rh-opt.on .rh-opt-box{border-color:rgba(var(--accent-rgb),.65);background:rgba(var(--accent-rgb),.18)}
.rh-opt.on .rh-opt-box::after{content:"";width:7px;height:4px;margin-top:-2px;border-left:1.6px solid var(--text);border-bottom:1.6px solid var(--text);transform:rotate(-45deg)}
.rh-filter-tags{display:flex;flex-wrap:wrap;gap:6px}
.rh-sheet-backdrop{position:absolute;inset:0;background:rgba(var(--shade-rgb,0,0,0),calc(.35 * var(--shade-scale,1)));opacity:0;pointer-events:none;transition:opacity .25s;z-index:30}
.rh-sheet-backdrop.visible{opacity:1;pointer-events:auto}
.rh-sheet{position:absolute;left:0;right:0;bottom:0;height:58%;min-height:260px;max-height:72%;display:flex;flex-direction:column;background:var(--surface-glass);backdrop-filter:blur(16px);-webkit-backdrop-filter:blur(16px);border-top:1px solid var(--border-glass);border-radius:12px 12px 0 0;transform:translateY(100%);transition:transform .28s cubic-bezier(.4,0,.2,1);pointer-events:none;z-index:31}
.rh-sheet.visible{transform:translateY(0);pointer-events:auto}
.rh-sheet-head{display:flex;align-items:center;justify-content:space-between;gap:10px;padding:10px 12px;border-bottom:1px solid var(--border-glass);flex:none;font-size:.84rem;font-weight:600;color:var(--text-dim)}
.rh-sheet-actions{display:flex;align-items:center;gap:6px}
.rh-sheet-sub{padding:6px 12px 0;font-size:.76rem;color:var(--text-muted);flex:none;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-sheet-body{flex:1;min-height:0;overflow-y:auto;padding:6px 12px 12px}
.rh-settings-fields{display:flex;flex-direction:column;gap:10px;padding-top:6px}
.rh-settings-field{display:flex;flex-direction:column;gap:5px;font-size:.8rem;color:var(--text-dim)}
.rh-settings-field code{font-size:.72rem;color:var(--text-muted)}
.rh-app.sheet-open .rh-tabs{visibility:hidden;pointer-events:none}
.rh-switch-row{display:flex;align-items:center;gap:12px;width:100%;padding:10px 8px;border:none;border-bottom:1px solid var(--border-glass);background:transparent;color:var(--text);text-align:left;font:inherit;cursor:pointer}
.rh-switch-row:hover{background:rgba(var(--accent-rgb),.06)}
.rh-switch-cover{flex:none;width:48px;aspect-ratio:2/3;border-radius:6px;overflow:hidden;background:rgba(var(--ink-rgb),.06);display:flex;align-items:center;justify-content:center}
.rh-switch-cover img{width:100%;height:100%;object-fit:cover;display:block}
.rh-switch-main{flex:1;min-width:0}
.rh-switch-title{font-weight:600;font-size:.88rem;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-switch-sub{margin-top:4px;font-size:.74rem;color:var(--text-muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-switch-source{flex:none;display:flex;flex-direction:column;align-items:flex-end;gap:4px;max-width:120px;font-size:.76rem;color:var(--text-muted);text-align:right}
.rh-switch-source > span{max-width:100%;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-sources-head{display:flex;align-items:center;gap:10px;margin-bottom:12px}
.rh-import-view{display:flex;flex-direction:column;gap:10px;max-width:720px}
.rh-import-view .rh-input{width:100%;box-sizing:border-box}
.rh-import-json{min-height:120px;resize:vertical;font:12px/1.5 Consolas,"Cascadia Mono",monospace}
.rh-search{position:relative;display:flex;flex-direction:column;gap:8px;margin-bottom:14px}
.rh-search-row{display:flex;gap:8px}
.rh-suggest{display:none;max-height:320px;overflow-y:auto;padding:6px;background:var(--surface-glass);backdrop-filter:blur(16px);-webkit-backdrop-filter:blur(16px);border:1px solid var(--border-glass);border-radius:8px;box-shadow:0 8px 24px rgba(var(--shade-rgb,0,0,0),calc(.35 * var(--shade-scale,1)));z-index:12}
.rh-suggest.vis{display:flex;flex-direction:column}
.rh-suggest-item{display:flex;align-items:center;gap:10px;width:100%;padding:7px 8px;border:none;background:transparent;color:var(--text);border-radius:6px;cursor:pointer;font:inherit;text-align:left}
.rh-suggest-item:hover{background:rgba(var(--accent-rgb),.08)}
.rh-suggest-cover{flex:none;width:30px;height:42px;border-radius:4px;overflow:hidden;background:rgba(var(--ink-rgb),.06);display:flex;align-items:center;justify-content:center;font-size:.72rem;color:var(--text-muted)}
.rh-suggest-cover img{width:100%;height:100%;object-fit:cover;display:block}
.rh-suggest-main{flex:1;min-width:0}
.rh-suggest-title{font-size:.86rem;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-suggest-meta{font-size:.72rem;color:var(--text-muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-suggest-state{display:flex;align-items:center;justify-content:center;gap:8px;padding:14px 8px;font-size:.8rem;color:var(--text-muted)}
.rh-loading{display:flex;align-items:center;justify-content:center;gap:10px;padding:36px 8px;color:var(--text-muted);font-size:.86rem}
.rh-spinner{flex:none;width:16px;height:16px;border:2px solid rgba(var(--ink-rgb),.18);border-top-color:var(--accent);border-radius:50%;animation:rh-spin .8s linear infinite}
@keyframes rh-spin{to{transform:rotate(360deg)}}
.rh-source-btn{width:100%;justify-content:space-between;gap:6px}
.rh-source-label{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-source-menu{position:absolute;top:calc(100% + 6px);left:0;min-width:220px;max-width:min(320px,80vw);max-height:280px;overflow-y:auto;padding:6px;background:var(--surface-glass);backdrop-filter:blur(16px);-webkit-backdrop-filter:blur(16px);border:1px solid var(--border-glass);border-radius:8px;box-shadow:0 8px 24px rgba(var(--shade-rgb,0,0,0),calc(.35 * var(--shade-scale,1)));z-index:12}
.rh-source-menu.is-hidden{display:none}
.rh-menu-item{display:flex;align-items:center;gap:8px;width:100%;padding:8px 10px;border:none;background:transparent;color:var(--text-dim);border-radius:6px;cursor:pointer;font:inherit;text-align:left}
.rh-menu-item:hover{background:rgba(var(--accent-rgb),.06);color:var(--text)}
.rh-menu-item.on{background:rgba(var(--accent-rgb),.12);color:var(--text)}
.rh-menu-name{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rh-menu-meta{flex:none;font-size:.7rem;color:var(--text-muted)}
.rh-input{flex:1;min-width:0;background:rgba(var(--ink-rgb),.04);border:1px solid var(--border-glass);border-radius:6px;color:var(--text);padding:7px 10px;font:inherit;outline:none}
.rh-input:focus{border-color:rgba(var(--accent-rgb),.45)}
.rh-btn{display:inline-flex;align-items:center;gap:6px;border:1px solid var(--border-glass);background:rgba(var(--ink-rgb),.05);color:var(--text);border-radius:6px;padding:6px 12px;cursor:pointer;font:inherit;white-space:nowrap;flex:none}
.rh-btn:hover{background:rgba(var(--accent-rgb),.10)}
.rh-btn:disabled{opacity:.4;cursor:default}
.rh-icon{flex:none}
.rh-btn.primary{border-color:rgba(var(--accent-rgb),.42);background:rgba(var(--accent-rgb),.12)}
.rh-btn.icon-only{padding:7px 9px;justify-content:center}
.rh-list{display:flex;flex-direction:column;gap:8px}
.rh-more{display:flex;align-items:center;justify-content:center;gap:8px;padding:16px 8px;color:var(--text-muted);font-size:.78rem}
.rh-more.idle{cursor:pointer}
.rh-more.idle:hover{color:var(--text)}
.rh-source-loading{display:flex;align-items:center;justify-content:center;gap:10px;min-height:240px;padding:24px 8px;color:var(--text-muted);font-size:.86rem}
.rh-overview-hint{margin-top:12px;padding:10px 12px;border:1px dashed var(--border-glass);border-radius:8px;color:var(--text-muted);font-size:.78rem;text-align:center}
.rh-row{display:flex;align-items:flex-start;gap:12px;padding:12px 14px;border:1px solid var(--border-glass);border-radius:8px;background:rgba(var(--ink-rgb),.025)}
.rh-row:hover{background:rgba(var(--accent-rgb),.06)}
.rh-row-main{flex:1;min-width:0}
.rh-title{font-weight:600;margin-bottom:4px}
.rh-meta{color:var(--text-muted);font-size:.82rem;margin-bottom:4px}
.rh-desc{color:var(--text-dim);font-size:.86rem;line-height:1.45}
.rh-tag{display:inline-block;border:1px solid var(--border-glass);border-radius:999px;padding:1px 8px;margin-right:6px;font-size:.74rem;color:var(--text-dim)}
.rh-empty{color:var(--text-muted);padding:28px 8px;text-align:center}
.rh-notice{border:1px solid rgba(var(--accent-rgb),.35);background:rgba(var(--accent-rgb),.08);border-radius:8px;padding:10px 12px;margin-bottom:12px;color:var(--text-dim);font-size:.84rem;line-height:1.5}
.rh-notice.error{border-color:rgba(var(--danger-rgb),.45);background:rgba(var(--danger-rgb),.08)}
.rh-detail-head{display:flex;gap:14px;align-items:flex-start;margin-bottom:16px}
.rh-detail-cover{flex:none;width:132px;aspect-ratio:2/3;border-radius:8px;overflow:hidden;background:rgba(var(--ink-rgb),.06);display:flex;align-items:center;justify-content:center}
.rh-detail-cover img{width:100%;height:100%;object-fit:cover;display:block}
.rh-detail-title{font-size:1.2rem;font-weight:600;margin-bottom:6px}
.rh-actions{display:flex;gap:8px;flex-wrap:wrap;margin-top:10px}
.rh-chapter{display:flex;justify-content:space-between;gap:12px;padding:9px 12px;border-bottom:1px solid var(--border-glass);cursor:pointer}
.rh-chapter:hover{background:rgba(var(--accent-rgb),.06)}
.rh-chapter .rh-progress{color:var(--accent);font-size:.78rem}
.rh-main-reader{padding:0;position:relative;overflow:hidden}
#rh-reader-mount{position:absolute;inset:0}
.rh-source{border:1px solid var(--border-glass);border-radius:8px;padding:12px 14px;margin-bottom:10px}
.rh-source-search{margin:0 0 10px;width:100%}
.rh-source-section{margin-bottom:10px;border:1px solid var(--border-glass);border-radius:8px;overflow:hidden}
.rh-source-section-head{display:flex;align-items:center;gap:8px;padding:8px 10px;background:rgba(var(--ink-rgb),.04);flex-wrap:wrap}
.rh-source-section-toggle{display:inline-flex;align-items:center;gap:6px;flex:1;min-width:0;padding:2px 0;border:none;background:transparent;color:var(--text);font:inherit;font-size:.84rem;cursor:pointer;text-align:left}
.rh-source-section-actions{display:flex;gap:6px;flex-wrap:wrap}
.rh-collapse-caret{display:inline-flex;transition:transform .2s ease}
.rh-collapse-caret.open{transform:rotate(180deg)}
.rh-source-section-body{padding:10px}
.rh-source-head{display:flex;align-items:center;gap:10px;margin-bottom:6px}
.rh-source-name{font-weight:600}
.rh-warn{color:var(--text-muted);font-size:.8rem;line-height:1.5;margin-top:6px}
.rh-warn-stack{position:relative;margin-bottom:10px;padding-top:8px}
.rh-warn-stack.has-more::before,.rh-warn-stack.has-more::after{content:"";position:absolute;left:10px;right:10px;top:4px;height:100%;border:1px solid var(--border-glass);border-radius:8px;background:rgba(var(--ink-rgb),.035);pointer-events:none}
.rh-warn-stack.has-more::after{left:18px;right:18px;top:0;background:rgba(var(--ink-rgb),.02)}
.rh-warn-card{position:relative;z-index:1;display:flex;align-items:center;gap:8px;min-width:0;padding:8px 10px;border:1px solid var(--border-glass);border-left:2px solid rgba(var(--danger-rgb),.72);border-radius:8px;background:rgba(var(--shade-rgb),calc(.28 * var(--shade-scale,1)))}
.rh-warn-source{flex:none;font-size:.76rem;font-weight:600;color:var(--text)}
.rh-warn-text{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--text-muted);font-size:.74rem}
.rh-warn-close{flex:none;width:24px;height:24px}
.rh-warn-count{flex:none;min-width:18px;height:18px;padding:0 5px;border-radius:999px;background:rgba(var(--accent-rgb),.16);color:var(--text-dim);font-size:.68rem;line-height:18px;text-align:center}
.rh-import{display:flex;flex-direction:column;gap:8px;margin-top:12px}
.rh-import textarea{min-height:120px;resize:vertical;background:rgba(var(--ink-rgb),.04);border:1px solid var(--border-glass);border-radius:6px;color:var(--text);padding:8px 10px;font:inherit;outline:none}
.rh-hint{color:var(--text-muted);font-size:.8rem;line-height:1.5}
.rh-card{border:1px solid var(--border-glass);border-radius:8px;padding:12px 14px;background:rgba(var(--ink-rgb),.03)}
.rh-card-head{display:flex;align-items:baseline;gap:10px;margin-bottom:6px}
.rh-card-title{font-weight:600}
.rh-card-sub{color:var(--text-muted);font-size:.78rem}
.rh-card-body{color:var(--text-dim);font-size:.86rem;line-height:1.5;margin-bottom:10px}
`;

// ── 卡顿排查日志（2026-10-19）────────────────────────────────────
//
// `console.debug` 记录常规耗时（DevTools 默认折叠），`console.warn` 只留
// 超过阈值的慢事件。排查完成后可把 DEBUG_PERF 改为 false。
const DEBUG_PERF = true;

function perfNow(): number {
  return typeof performance !== "undefined" ? performance.now() : Date.now();
}

function perfDebug(label: string, detail?: Record<string, unknown>): void {
  if (!DEBUG_PERF) return;
  console.debug(`[resource-hub] ${label}`, detail ?? "");
}

function perfInfo(label: string, detail?: Record<string, unknown>): void {
  if (!DEBUG_PERF) return;
  console.info(`[resource-hub] ${label}`, detail ?? "");
}

function perfWarn(label: string, detail?: Record<string, unknown>): void {
  if (!DEBUG_PERF) return;
  console.warn(`[resource-hub] ${label}`, detail ?? "");
}

/** 结果网格一次渲染 30 张卡片；滚到底部先追加本地数据，再联网取下一页。 */
const RESULT_PAGE_SIZE = 30;
/** 「全部来源」只随机预览一个来源，不再为了凑满 30 张强制加载多个源。 */
const OVERVIEW_SOURCE_BATCH = 1;
/** 指定分类 / 来源时一次多抓几个：单源空关键词搜索可能没有结果。 */
const SCOPED_SOURCE_BATCH = 8;
/** 远程作品缓存上限：长时间浏览也不无限增长。 */
const REMOTE_WORK_CACHE_LIMIT = 2000;
/** 连续失败的来源冷却多久后再次参与总览随机预览。 */
const SOURCE_COOLDOWN_MS = 5 * 60_000;
/** 搜索预览每批来源数；拿到 5 个不同源就停。 */
const SUGGEST_SOURCE_BATCH = 4;
/** A7 二期：回传聊天窗的作品上限。聊天里只摆「够点开」的前几条，不摊开整页。 */
const RESULTS_PUBLISH_LIMIT = 8;
/** 换源搜索的硬上限：到点就带着已拿到的结果收尾，不让抽屉一直转圈。 */
const SOURCE_SWITCH_TIMEOUT_MS = 15_000;
/** 总览缓存：每个来源一条宿主 KV，避免单个值超过 256 KiB 上限。 */
const OVERVIEW_CACHE_PREFIX = "overview.v1.";
/** 每个来源缓存的作品条数：够首屏跨源混排，也控制缓存总体积。 */
const OVERVIEW_CACHE_WORKS = 24;
/** 缓存新鲜窗口；窗口内直接展示缓存，不联网。 */
const OVERVIEW_CACHE_TTL = 30 * 60_000;

interface OverviewCacheEntry {
  id: string;
  name: string;
  /** 来源版本 + 设置的签名；不一致时缓存作废。 */
  sig: string;
  at: number;
  works: Work[];
  /** 抓这一页时来源是否还有下一页；旧条目缺省按「有」处理（翻页时才请求）。 */
  hasMore?: boolean;
}

export class ResourceHubApp {
  private root: HTMLElement;
  private state: RhState;
  private view: View = { kind: "overview" };
  private query = "";
  /** A7（2026-10-27）：冷启动时来源目录还没读回来，自动搜索先排队，就绪后补跑一次。 */
  private queuedAutoSearch: { query: string; kind?: string } | null = null;
  private notice = "";
  private noticeError = false;
  /** 已关闭的来源错误提示；key = sourceId::error。 */
  private dismissedWarnings = new Set<string>();
  /** 当前堆叠最上方的警告与 5s 自动关闭计时器。 */
  private warningTimer: number | undefined;
  private warningTimerKey = "";
  private readerHandle: ReaderHandle | null = null;
  private playerHandle: PlayerHandle | null = null;
  /** 进入播放器时交给组件的播放列表（URL 已代理）。 */
  private pendingVideo: {
    workId: string;
    chapterId: string;
    chapterIndex: number;
    title: string;
    playbacks: Playback[];
    initialPosition: number;
    /** 把清单 / 分片 / 字幕地址交给宿主媒体代理。 */
    resolveUrl: (url: string) => string;
  } | null = null;
  /** 播放器进度回写用的当前会话。 */
  private videoSession: { workId: string; chapterId: string; chapterIndex: number; title: string } | null = null;
  private host = createSourceHost();
  /** 拼音编码缓存（标题 → full/initials）。宿主没有 `pinyin` 能力时整表为空 = 自动降级。 */
  private pinyinCache = new Map<string, PinyinEntry>();
  /** 已发出、尚未回来的编码请求（避免同一标题重复排队）。 */
  private pinyinPending = new Set<string>();
  /** 宿主编码失败一次后整轮停用（拼音是增强能力，不重试）。 */
  private pinyinBroken = false;
  private sourceResults: SourceSearchResult[] = [];
  private searching = false;
  private searchToken = 0;
  /** 新搜索 / 清空输入时取消上一轮在途请求。 */
  private searchAbort: AbortController | null = null;
  /** 书架子分页：喜欢 / 历史记录。 */
  private libraryTab: LibraryTab = "favorites";
  /** 总览排序：更新时间 / 完结度 / 级数。 */
  private sortMode: SortMode = "updated";
  /** 总览筛选（已应用）：来源 / 标签 / 完结状态；空集合 = 全部。 */
  private filterSources = new Set<string>();
  private filterTags = new Set<string>();
  private filterStatus = new Set<string>();
  /** 顶部便携类型筛选（漫画 / 小说 / 视频 / 番剧）；空集合 = 全部。 */
  private filterKinds = new Set<SourceKind>();
  /** 筛选抽屉：开关、当前分页与草稿（确认后才写入上面的已应用集合）。 */
  private filterOpen = false;
  private filterTab: FilterTab = "sources";
  private filterDraftSources = new Set<string>();
  private filterDraftTags = new Set<string>();
  private filterDraftStatus = new Set<string>();
  /** 来源管理页的搜索词与两个列表的折叠状态（2026-10-21）。 */
  private sourceQuery = "";
  private registryCollapsed = true;
  private importedCollapsed = true;
  /** 搜索建议下拉框：输入时实时模糊匹配，回车 / 搜索按钮才提交正式搜索。 */
  private suggestOpen = false;
  private suggestLoading = false;
  private suggestReady = false;
  private suggestQuery = "";
  private suggestSource = "";
  private suggestResults: SourceSearchResult[] = [];
  /** 预览是否已扫完所有来源；提前拿到 5 个源时为 false，回车后仍需跑全量搜索。 */
  private suggestComplete = false;
  private suggestToken = 0;
  private suggestAbort: AbortController | null = null;
  private suggestTimer: number | undefined;
  /** 总览来源数据（explore）：按批懒加载，滚到底部再抓下一批。 */
  private exploreResults: SourceSearchResult[] = [];
  private exploreQueue: SourceRecord[] = [];
  /** 每批抓几个来源：全部来源为 1，指定分类 / 来源时为 SCOPED_SOURCE_BATCH。 */
  private exploreBatchSize = OVERVIEW_SOURCE_BATCH;
  private exploreDone = true;
  private exploreBatchLoading = false;
  private exploreLoading = false;
  private exploreToken = 0;
  private exploreAbort: AbortController | null = null;
  private exploreSignature = "";
  private exploreLoadedAt = 0;
  /** 每个来源当前结果对应的缓存签名；缺条目 = 还没抓过。 */
  private overviewSigs = new Map<string, string>();
  /** 每个来源最近一次成功抓取的时间（指定来源预览判断新鲜度）。 */
  private overviewAt = new Map<string, number>();
  /** 宿主 KV 里的总览缓存读取任务（并发调用共用一次）。 */
  private overviewCacheLoad: Promise<boolean> | null = null;
  /** 正在预抓预览的来源；总览刷新跳过它们，避免重复请求。 */
  private previewPrefetching = new Set<string>();
  /** 统一加载条：当前进度与正在抓取的来源名。 */
  private loadingDone = 0;
  private loadingTotal = 0;
  private loadingCurrent = "";
  /** 每次用户滚动 / 滚轮 / 触摸才允许自动补一页，初始不自动续拉。 */
  private scrollArmed = false;
  private lastAutoMoreAt = 0;
  /** 结果列表缓存：数据 / 查询 / 筛选不变时不再重新排序去重。 */
  private itemsRevision = 0;
  private itemsCache: { key: string; items: ResultItem[] } | null = null;
  /** 卡顿排查：长任务观察 + 主线程心跳。 */
  private perfStarted = false;
  private longTaskObserver: PerformanceObserver | null = null;
  private heartbeatTimer: number | undefined;
  /** 结果网格当前显示到第几条；滚到底部时按 30 条递增。 */
  private resultLimit = RESULT_PAGE_SIZE;
  /** 搜索结果分页游标：来源 id → 已加载到的页码；只记录抓过首页的来源。 */
  private searchPages = new Map<string, number>();
  /** 搜索结果里还有下一页的来源 id。 */
  private searchMore = new Set<string>();
  /** 总览（explore）分页游标，语义同 searchPages / searchMore。 */
  private explorePages = new Map<string, number>();
  private exploreMore = new Set<string>();
  /** 正在联网取下一页；哨兵据此显示进度并阻止重复触发。 */
  private moreLoading = false;
  private moreAbort: AbortController | null = null;
  private resultsObserver: IntersectionObserver | null = null;
  /** 封面观察器：只有接近视口才写入 src，避免一次解码几百张原图。 */
  private coverObserver: IntersectionObserver | null = null;
  /** paint 合并到动画帧，避免一次交互内多次整树重建。 */
  private paintScheduled = false;
  private paintFrame: number | undefined;
  private paintTimer: number | undefined;
  /** 换源底部面板：当前作品、搜索结果与缓存。 */
  private sourceSwitchOpen = false;
  /** 首次打开时先隐藏插入，再在下一帧加 visible，保证滑入动画真实发生。 */
  private sourceSwitchEnter = false;
  /** 按来源覆盖域名的面板。 */
  private settingsOpen = false;
  private settingsSourceId = "";
  private settingsDraft: Record<string, string> = {};
  /** 请求头覆盖（User-Agent / Referer / Cookie），随 manifest.headers 保存。 */
  private settingsHeaders: Record<string, string> = {};
  /** 域名探活：进行中状态与代次（关闭 / 切换面板后丢弃迟到结果）。 */
  private probingSettings = false;
  private probeToken = 0;
  private sourceSwitchWorkId = "";
  private sourceSwitchTitle = "";
  private sourceSwitchKind: SourceKind = "comic";
  private sourceSwitchLoading = false;
  private sourceSwitchError = "";
  private sourceSwitchResults: SourceSwitchItem[] = [];
  private sourceSwitchCache = new Map<string, { at: number; items: SourceSwitchItem[] }>();
  /** 当前换源搜索的取消句柄：重试 / 关闭抽屉时终止上一轮。 */
  private sourceSwitchAbort: AbortController | null = null;
  /** 近期失败的来源：冷却期内不参与「全部来源」随机预览（手动选择仍可重试）。 */
  private sourceCooldown = new Map<string, number>();
  /** 导入页 URL、可选粘贴内容与进行中状态。 */
  private importUrl = GITHUB_REGISTRY_URL;
  private importJson = "";
  private importing = false;
  /** 书架数据包（B3）：文本框内容；导出时预填，导入时由用户粘贴。 */
  private libraryIoText = "";
  /** A7 二期：下一次搜索完成后是否把作品回传聊天窗（只由 `applySearch` 置位）。 */
  private publishPending = false;
  /** A7 二期：回传时带上的类型过滤（来自聊天卡片的 `kind`）。 */
  private publishKind: string | null = null;
  /** 一键安装进行中：避免重复触发。 */
  private batchInstalling = false;
  /** 一键更新进行中：避免重复触发。 */
  private batchUpdating = false;
  private remoteWorks = new Map<string, { record: SourceRecord; work: Work }>();
  private remoteChapters = new Map<string, Chapter[]>();
  /** 跨章按需加载缓存：key = `${workKey}#${chapterIndex}`。 */
  private remoteReaderChapters = new Map<string, ReaderChapter>();
  /** 加密图源解出来的章节图（`blob:`）：随实例销毁时回收，避免整本书的图片留在内存里。 */
  private readerImageBlobs = new Set<string>();
  /** 同上，封面那一份（§待办 3-20）。 */
  private coverBlobs = new Set<string>();
  private remoteChapterErrors = new Map<string, string>();
  private remoteLoading = new Set<string>();
  /** 详情补全只尝试一次；只在详情页触发，预览 / 阅读器不拉详情。 */
  private remoteDetailTried = new Set<string>();
  /** 远程来源进入阅读器时的完整章节表（未加载章为 lazy 占位）。 */
  private pendingRemote: { workKey: string; chapters: ReaderChapter[]; pageIndex: number } | null = null;
  /** `baseIndex`：阅读器内部章节序号 → 作品真实章节序号（单章挂载时使用）。
   *  `chapterIndex` / `pageIndex` 是**当前阅读位置**：`paintNow()` 每次都会重建整棵 DOM
   *  （顺带销毁并重挂阅读器），没有这两个值就回不到原位。 */
  private readerSession: {
    workKey: string;
    chapters: { id: string; title: string }[];
    baseIndex: number;
    chapterIndex: number;
    pageIndex: number;
  } | null = null;

  constructor(root: HTMLElement) {
    this.root = root;
    this.state = loadState();
    const repaired = this.state.sources.map((source) => repairSourceRecord(source) ?? source);
    if (repaired.some((source, index) => source !== this.state.sources[index])) {
      this.state = { ...this.state, sources: repaired };
      saveState(this.state);
    }
    root.addEventListener("click", (event) => {
      const started = perfNow();
      try {
        this.onClick(event);
      } finally {
        const ms = perfNow() - started;
        if (ms >= 50) {
          const el = (event.target as HTMLElement | null)?.closest<HTMLElement>("[data-action]");
          perfWarn(`click ${Math.round(ms)}ms`, { action: el?.dataset.action ?? "(none)" });
        }
      }
    });
    root.addEventListener("input", (event) => this.onInput(event));
    root.addEventListener("keydown", (event) => this.onKeyDown(event));
    // scroll 不冒泡，用捕获监听；wheel / touchmove 覆盖“页面不够高、没有 scroll”的情况。
    const arm = (): void => {
      this.scrollArmed = true;
      this.maybeAutoMore();
    };
    root.addEventListener("scroll", arm, true);
    root.addEventListener("wheel", arm, { capture: true, passive: true });
    root.addEventListener("touchmove", arm, { capture: true, passive: true });
    // 回到前台时把挂起的重绘立刻补上：后台被节流时 rAF / 定时器都可能还压着，
    // 不主动 flush 的话用户切回来第一眼看到的是旧界面（2026-10-31）。
    document.addEventListener("visibilitychange", this.onDocVisibility);
  }

  /** `visibilitychange` 的处理：只在「变回可见且确实有挂起重绘」时补一帧。 */
  private onDocVisibility = (): void => {
    if (document.visibilityState === "visible" && this.paintScheduled) {
      this.paintImmediate();
    }
  };

  /** 宿主卸载插件（关窗 / 收起内嵌面板 / 重新 execute）时调用。
   *
   *  必须**主动**收掉在跑的东西：iframe 被移除时浏览器不保证立刻回收 `<video>`
   *  上的 MSE 拉流，也不保证取消在途请求 —— 用户侧表现就是「关了还在响」。 */
  destroy(): void {
    this.pendingVideo = null;
    this.videoSession = null;
    this.pendingRemote = null;
    this.readerSession = null;
    this.playerHandle?.destroy();
    this.playerHandle = null;
    // 兜底：任何一条忘了 destroy 的路径留下的 `<video>` 也在这里停掉 ——
    // 脱离 DOM 的媒体元素浏览器不会回收，用户听到的是「退出后还在响」（见 player.ts 的 liveVideos）。
    stopAllPlayers();
    this.readerHandle?.destroy();
    this.readerHandle = null;
    // 加密图源的章节图是 blob URL：阅读器销毁后也要显式回收，否则整章图片一直占内存。
    for (const url of this.readerImageBlobs) {
      try {
        URL.revokeObjectURL(url);
      } catch {
        /* 已回收 */
      }
    }
    this.readerImageBlobs.clear();
    for (const url of this.coverBlobs) {
      try {
        URL.revokeObjectURL(url);
      } catch {
        /* 已回收 */
      }
    }
    this.coverBlobs.clear();
    this.root.removeEventListener("error", this.onCoverError, true);
    document.removeEventListener("visibilitychange", this.onDocVisibility);
    this.searchAbort?.abort();
    this.searchAbort = null;
    this.coverObserver?.disconnect();
    this.coverObserver = null;
    this.longTaskObserver?.disconnect();
    this.longTaskObserver = null;
    if (this.paintFrame !== undefined) window.cancelAnimationFrame(this.paintFrame);
    this.paintFrame = undefined;
    if (this.paintTimer !== undefined) window.clearTimeout(this.paintTimer);
    this.paintTimer = undefined;
    if (this.warningTimer !== undefined) window.clearTimeout(this.warningTimer);
    this.warningTimer = undefined;
    if (this.suggestTimer !== undefined) window.clearTimeout(this.suggestTimer);
    this.suggestTimer = undefined;
    if (this.heartbeatTimer !== undefined) window.clearInterval(this.heartbeatTimer);
    this.heartbeatTimer = undefined;
    this.paintScheduled = false;
  }

  mount(): void {
    this.startPerfWatch();
    // 封面兜底：`error` 不冒泡，只能在捕获阶段听（§待办 3-20）。
    this.root.addEventListener("error", this.onCoverError, true);
    this.paint();
    // 宿主的来源目录是**真相源**；localStorage 只是缓存。异步读入后重绘一次。
    void this.hydrateHostSources();
    void this.refreshOverview();
  }

  /**
   * A7 对话入口（2026-10-27）：AI 带着搜索词打开阅读 ⇒ 落进搜索框、切回总览并立即搜索。
   *
   * 与用户手打一致：走 `paint()`（输入框按 `this.query` 重绘）+ `commitSearch()`（同一套
   * 取消在途请求 / 建议复用 / 分页游标重置），不另开一条搜索路径。`kind` 只认四个合法
   * 枚举（与 AI 工具 `ResourceHubOpen` 同口径），非法值忽略而不是猜。
   *
   * **冷启动**：全新实例 / 清过缓存时 `state.sources` 可能为空，而宿主来源目录是异步读回来的
   *（`hydrateHostSources`）。这时立刻搜只会搜到 0 条 —— 因此把请求排进 `queuedAutoSearch`，
   * 等来源就绪后再补跑一次（`flushQueuedAutoSearch`），不让「AI 已经开了搜索」变成空结果。
   */
  applySearch(query: string, kind?: string): void {
    const q = query.trim();
    if (!q) return;
    const kinds: SourceKind[] = ["comic", "novel", "anime", "tv"];
    if (kind && kinds.includes(kind as SourceKind)) {
      this.filterKinds = new Set<SourceKind>([kind as SourceKind]);
    }
    this.query = q;
    this.view = { kind: "overview" };
    // 下拉框 / 筛选抽屉 / 换源面板都收起来，否则会盖住刚出来的结果。
    this.suggestOpen = false;
    this.filterOpen = false;
    this.sourceSwitchOpen = false;
    this.queuedAutoSearch = this.state.sources.length === 0 ? { query: q, kind } : null;
    // A7 二期：这一轮搜完要把作品回传聊天窗（只有 AI 发起的搜索才回传）。
    this.publishPending = true;
    this.publishKind = kind && kinds.includes(kind as SourceKind) ? kind : null;
    this.paint();
    this.commitSearch();
  }

  /** A7 二期（2026-10-28）：从聊天卡片点进来 —— 直接打开该作品的详情页。
   *
   *  作品是**上一条消息的搜索结果**里的一条，所以这里只用回传的最少字段重建 `Work`
   *  （`sourceId` / `workId` / `kind` / 标题 / 作者），塞进 `remoteWorks` 缓存后走与
   *  「搜索结果卡」完全相同的 `view = detail` 路径（章节加载由详情页自己发起）。
   *  来源已被卸载时**退回按标题搜索** —— 不把用户丢进一个空白详情页。 */
  openPublishedWork(input: OpenWork): void {
    const record = this.state.sources.find((item) => item.id === input.sourceId);
    if (!record) {
      this.applySearch(input.title);
      return;
    }
    const kinds: SourceKind[] = ["comic", "novel", "anime", "tv"];
    const kind = kinds.includes(input.kind as SourceKind) ? (input.kind as SourceKind) : "comic";
    const work: Work = {
      ref: { sourceId: record.id, workId: input.workId, kind },
      title: input.title,
      ...(input.author ? { author: input.author } : {}),
    };
    const key = workKey(work);
    this.rememberRemoteWork(key, record, work);
    this.view = { kind: "detail", workId: key };
    this.suggestOpen = false;
    this.filterOpen = false;
    this.sourceSwitchOpen = false;
    this.notice = "";
    this.paint();
  }

  /** A7 二期：把这一轮搜索的前几条作品广播给宿主（`resource_hub_publish_results`）。
   *  只发数据：不开窗、不写盘；宿主只往有同 query 卡片的窗口里填。
   *  没有宿主（浏览器 harness）时 `tauriInvoke` 返回 null，静默跳过。 */
  private publishResults(query: string, results: SourceSearchResult[], kind: string | null): void {
    const items: Array<{
      sourceId: string;
      sourceName: string;
      workId: string;
      kind: SourceKind;
      title: string;
      author?: string;
    }> = [];
    const seen = new Set<string>();
    for (const result of results) {
      for (const work of result.works) {
        const key = workKey(work);
        if (seen.has(key)) continue;
        seen.add(key);
        items.push({
          sourceId: work.ref.sourceId,
          sourceName: result.sourceName,
          workId: work.ref.workId,
          kind: work.ref.kind,
          title: work.title,
          ...(work.author ? { author: work.author } : {}),
        });
        if (items.length >= RESULTS_PUBLISH_LIMIT) break;
      }
      if (items.length >= RESULTS_PUBLISH_LIMIT) break;
    }
    const kindFilter = kind ?? undefined;
    const payload = JSON.stringify({ query, ...(kindFilter ? { kind: kindFilter } : {}), items });
    void tauriInvoke("resource_hub_publish_results", { payload })?.catch(() => undefined);
  }

  /** 来源目录异步就绪后补跑一次被排队的自动搜索（只在「当时一个来源都没有」时才有值）。 */
  private flushQueuedAutoSearch(): void {
    const queued = this.queuedAutoSearch;
    if (!queued || this.state.sources.length === 0) return;
    this.queuedAutoSearch = null;
    this.query = queued.query;
    // 排队的一定是 AI 发起的那一轮：首次空数据跑已经把 `publishPending` 消费掉了，
    // 补跑必须重新置位，否则冷启动（来源目录还没回来）永远回传不了结果。
    this.publishPending = true;
    this.publishKind = queued.kind ?? null;
    this.paint();
    this.commitSearch();
  }

  /**
   * 卡顿取证：长任务（>50ms）直接 warn；另用 500ms 心跳测主线程阻塞。
   * 页面卡死时日志会在恢复后补出，能给出阻塞时长与当时的界面状态。
   */
  private startPerfWatch(): void {
    if (this.perfStarted) return;
    this.perfStarted = true;
    perfInfo("perf watch start", {
      api: (globalThis as { __lunac_host?: { apiVersion?: number } }).__lunac_host?.apiVersion,
      sources: this.state.sources.length,
    });
    try {
      this.longTaskObserver = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          const ms = Math.round(entry.duration);
          if (ms >= 50) perfWarn(`longtask ${ms}ms`, { at: Math.round(entry.startTime) });
        }
      });
      this.longTaskObserver.observe({ entryTypes: ["longtask"] });
    } catch {
      perfDebug("longtask observer unsupported");
    }
    let last = perfNow();
    this.heartbeatTimer = window.setInterval(() => {
      const now = perfNow();
      const gap = now - last;
      last = now;
      if (gap >= 1200) {
        perfWarn(`main thread blocked ${Math.round(gap)}ms`, {
          view: this.view.kind,
          query: this.query || undefined,
          sources: this.state.sources.length,
          loading: this.exploreLoading || this.searching,
        });
      }
    }, 500);
  }

  private async hydrateHostSources(): Promise<void> {
    const host = await loadHostSources();
    if (!host) return;
    const hostIds = new Set(host.map((item) => item.id));
    // 旧版本只写 localStorage：宿主目录为空时把本地来源迁过去。
    // 宿主已有文件说明同步早已发生 —— 此时宿主才是唯一真相源，外部删掉的文件
    // 不会在下次启动时被 localStorage 复活。
    const firstMigration = !this.state.hostMigrated && host.length === 0;
    if (firstMigration) {
      for (const local of this.state.sources) {
        if (!hostIds.has(local.id)) persistHostSource(local);
      }
    }
    const byId = new Map<string, SourceRecord>();
    if (firstMigration) {
      for (const local of this.state.sources) byId.set(local.id, local);
    }
    for (const item of host) {
      // 宿主目录是真相源：旧版本装错的 baseUrl 在这里顺手修正并写回。
      const repaired = repairSourceRecord(item);
      const record = repaired ?? item;
      byId.set(record.id, record);
      if (repaired) persistHostSource(repaired);
    }
    this.state = { ...this.state, sources: [...byId.values()], hostMigrated: true };
    this.sourceSwitchCache.clear();
    saveState(this.state);
    this.paint();
    // A7：来源目录到位后补跑排队中的自动搜索（冷启动时 applySearch 可能先于本函数发生）。
    this.flushQueuedAutoSearch();
    // 宿主目录可能带来新来源 / 新能力：强制重拉总览。
    void this.refreshOverview(true);
  }

  /** 合并到动画帧：一次交互里多次状态变化只重建一次整树。 */
  private paint(): void {
    // ── 窗口不可见时**同步重绘**（2026-10-31 报障：长期后台打开时「加载数据不进加载页」）──
    //
    // rAF 在窗口被遮挡 / 长期在后台时会被挂起，`setTimeout` 也会被节流
    //（Chromium 的后台计时器最狠能到 1 次/分钟）。这时如果还等「下一帧」，
    // 「开始加载」这个状态可能要等用户切回来才画出来 —— 观感就是点了没反应。
    // 只有确知不可见时才同步重绘；可见时仍走合并，避免一次交互里重复整树重建。
    if (document.visibilityState === "hidden") {
      this.paintImmediate();
      return;
    }
    if (this.paintScheduled) return;
    this.paintScheduled = true;
    const run = (): void => {
      if (!this.paintScheduled) return;
      this.paintScheduled = false;
      if (this.paintFrame !== undefined) window.cancelAnimationFrame(this.paintFrame);
      this.paintFrame = undefined;
      if (this.paintTimer !== undefined) window.clearTimeout(this.paintTimer);
      this.paintTimer = undefined;
      this.paintNow();
    };
    this.paintFrame = window.requestAnimationFrame(run);
    // 窗口隐藏 / 后台标签页会暂停 rAF：120ms 后兜底重绘，避免界面停在旧状态。
    this.paintTimer = window.setTimeout(run, 120);
  }

  /** 取消挂起的合并重绘并按当前状态立刻重画一次。
   *
   *  只给「用户马上要看到反馈」的那几处用（开始加载 / 开始搜索）：
   *  挂起的 rAF 若已被后台节流，等它等于让界面停在旧状态。 */
  private paintImmediate(): void {
    if (this.paintFrame !== undefined) window.cancelAnimationFrame(this.paintFrame);
    this.paintFrame = undefined;
    if (this.paintTimer !== undefined) window.clearTimeout(this.paintTimer);
    this.paintTimer = undefined;
    this.paintScheduled = false;
    this.paintNow();
  }

  private paintNow(): void {
    // ── 播放器已挂载时**不重绘**（2026-10-31 报障：同一段声音隔约 0.5s 又响一次）──
    //
    // `paintNow` 会整树重建，并在 `view.kind === "player"` 时重新 `mountPlayerView()`：
    // 旧 `<video>` 被 destroy、新 `<video>` 从 `pendingVideo.initialPosition` 重新起播。
    // 于是任何一次「迟到的重绘」（最常见的是开插件时 `hydrateHostSources()` /
    // `refreshOverview()` 的异步收尾）都会让刚播了半秒的视频**从头再来一遍** ——
    // 听感就是「两段音轨、前后差 0.5s」，而且每条线路都会，因为触发点是重绘时机不是流。
    //
    // 播放器界面是自包含的（进度、换线、字幕都在 player.ts 里），没有非重绘不可的状态；
    // 退出播放器时 `onExit` 会先 destroy 再把 view 改成 detail ⇒ 下一次 paint 正常重建。
    // 判据还要求「挂着的正是这一集」：将来若有路径在播放中换集，重绘必须放行，不能被这条卡住。
    //
    // 还要要求**播放器真的还在文档里**（2026-10-31 补充）：宿主重开插件走
    // `plugin-window-input` ⇒ `render()` 把结果区整段换掉，实例却还是这一份。
    // 那时若仍然早退，界面回到卡片、`<video>` 留在后台出声 —— 必须放行重建（下面会先 destroy）。
    const mounted = this.videoSession;
    if (
      this.view.kind === "player"
      && this.playerHandle
      && this.playerHandle.isAttached()
      && this.pendingVideo
      && mounted
      && mounted.workId === this.pendingVideo.workId
      && mounted.chapterId === this.pendingVideo.chapterId
    ) {
      return;
    }
    const started = perfNow();
    this.coverObserver?.disconnect();
    this.coverObserver = null;
    this.readerHandle?.destroy();
    this.readerHandle = null;
    this.playerHandle?.destroy();
    this.playerHandle = null;
    // 整树重建前再兜一次：脱离 DOM 的 `<video>` 浏览器不会回收（「退出后还在响」）。
    stopAllPlayers();
    // 播放器没有可播放地址（例如页面被外部状态重绘）：退回详情页，避免死路。
    if (this.view.kind === "player" && !this.pendingVideo) {
      this.view = { kind: "detail", workId: this.view.workId };
    }
    const bare = this.view.kind === "reader" || this.view.kind === "player";
    const nav = bare
      ? ""
      : `<nav class="rh-tabs">${this.tabButton("overview")}${this.tabButton("library")}${this.tabButton("sources")}</nav>`;
    this.root.innerHTML = `<style>${STYLE}</style><div class="rh-app${this.sourceSwitchOpen || this.settingsOpen ? " sheet-open" : ""}">
<main class="rh-main${bare ? " rh-main-reader" : ""}">${this.noticeHtml()}${this.bodyHtml()}</main>
${nav}
${bare || this.view.kind !== "overview" ? "" : this.filterDockHtml()}
${bare ? "" : this.filterDrawerHtml()}
${bare ? "" : this.sourceSwitchSheetHtml()}
${bare || !this.settingsOpen ? "" : this.settingsSheetHtml()}
</div>`;
    if (this.view.kind === "reader") {
      this.mountReaderView(this.view.workId, this.view.chapterIndex);
    }
    if (this.view.kind === "player") {
      this.mountPlayerView();
    }
    this.observeCovers();
    if (this.sourceSwitchEnter && this.sourceSwitchOpen) {
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          if (!this.sourceSwitchOpen) return;
          this.root.querySelector<HTMLElement>(".rh-sheet")?.classList.add("visible");
          this.root.querySelector<HTMLElement>(".rh-sheet-backdrop")?.classList.add("visible");
          this.sourceSwitchEnter = false;
        });
      });
    }
    this.observeResultsEnd();
    this.syncWarningTimer();
    const ms = perfNow() - started;
    if (ms >= 30) {
      perfWarn(`paint ${Math.round(ms)}ms`, {
        view: this.view.kind,
        sources: this.state.sources.length,
        cards: this.view.kind === "overview" ? this.resultItems().length : undefined,
      });
    }
  }

  private tabButton(tab: Tab): string {
    const active =
      this.view.kind === tab
      || (tab === "overview" && (this.view.kind === "detail" || this.view.kind === "reader" || this.view.kind === "player"))
      || (tab === "library" && this.view.kind === "library-io")
      || (tab === "sources" && this.view.kind === "import");
    const label = esc(LANG.tabs[tab]);
    return `<button class="rh-tab${active ? " active" : ""}" data-action="tab" data-tab="${tab}" title="${label}" aria-label="${label}">${rhIcon(tab, 18)}</button>`;
  }

  private noticeHtml(): string {
    if (!this.notice) return "";
    return `<div class="rh-notice${this.noticeError ? " error" : ""}">${esc(this.notice)}</div>`;
  }

  private bodyHtml(): string {
    switch (this.view.kind) {
      case "overview":
        return this.overviewHtml();
      case "library":
        return this.libraryHtml();
      case "library-io":
        return this.libraryIoHtml();
      case "sources":
        return this.sourcesHtml();
      case "import":
        return this.importHtml();
      case "detail":
        return this.detailHtml(this.view.workId);
      case "reader":
        return this.readerHtml(this.view.workId, this.view.chapterIndex);
      case "player":
        return `<div id="rh-player-mount"></div>`;
    }
  }

  private overviewHtml(): string {
    return `<div class="rh-search">
<div class="rh-search-row">
<input id="rh-q" class="rh-input" value="${esc(this.query)}" placeholder="${esc(LANG.searchPlaceholder)}" autocomplete="off">
<button class="rh-btn primary icon-only" data-action="search" title="${esc(LANG.search)}" aria-label="${esc(LANG.search)}">${rhIcon("search", 16)}</button>
</div>
<div id="rh-suggest" class="rh-suggest${this.suggestOpen ? " vis" : ""}">${this.suggestHtml()}</div>
</div>${this.toolbarHtml()}<div id="rh-results" class="rh-list">${this.resultsHtml()}</div>`;
  }

  private toolbarHtml(): string {
    const sortChips = (["updated", "status", "chapters"] as SortMode[])
      .map((mode) => `<button class="rh-chip${this.sortMode === mode ? " active" : ""}" data-action="sort-mode" data-mode="${mode}">${esc(LANG.sortModes[mode])}</button>`)
      .join("");
    const quickKindLabels: Record<SourceKind, string> = {
      comic: LANG.kind.comic,
      novel: LANG.kind.novel,
      tv: LANG.category.video,
      anime: LANG.kind.anime,
    };
    const kindChips = (["comic", "novel", "tv", "anime"] as SourceKind[])
      .map((kind) => `<button class="rh-chip${this.filterKinds.has(kind) ? " active" : ""}" data-action="kind-filter" data-kind="${kind}">${esc(quickKindLabels[kind])}</button>`)
      .join("");
    const allKindChip = `<button class="rh-chip${this.filterKinds.size === 0 ? " active" : ""}" data-action="kind-filter" data-kind="">${esc(LANG.allKinds)}</button>`;
    return `<div class="rh-toolbar">
<div class="rh-toolbar-row rh-kind-row">
<span class="rh-sort-label">${esc(LANG.filter)}</span>
<div class="rh-chips">${allKindChip}${kindChips}</div>
</div>
<div class="rh-toolbar-row">
<span class="rh-sort-label">${esc(LANG.sortBy)}</span>
<div class="rh-chips rh-sort-chips">${sortChips}</div>
</div></div>`;
  }

  /** 左下角筛选入口 + 已选标签：标签独立成 pill，点 × 立即取消（2026-10-21）。 */
  private filterDockHtml(): string {
    return `<div class="rh-filter-dock">${this.filterFabHtml()}${this.appliedTagPillsHtml()}</div>`;
  }

  private appliedTagPillsHtml(): string {
    return [...this.filterTags]
      .map(
        (tag) =>
          `<button class="rh-filter-pill" data-action="remove-tag" data-tag="${esc(tag)}" title="${esc(LANG.remove)}"><span>${esc(tag)}</span>${rhIcon("close", 12)}</button>`,
      )
      .join("");
  }

  /** 左下角筛选入口：直接显示当前筛选的源，避免用户不知道结果被谁限定。 */
  private filterFabHtml(): string {
    const count = this.activeFilterCount();
    return `<button class="rh-filter-fab" data-action="open-filter" title="${esc(LANG.moreFilters)}" aria-label="${esc(LANG.moreFilters)}">${rhIcon("filter", 16)}<span class="rh-filter-fab-label">${esc(this.filterScopeLabel())}</span>${count ? `<span class="rh-filter-badge">${count}</span>` : ""}</button>`;
  }

  private filterScopeLabel(): string {
    if (this.filterSources.size === 0) return LANG.allSources;
    return [...this.filterSources]
      .map((id) => this.state.sources.find((source) => source.id === id)?.name ?? id)
      .join("、");
  }

  private filterDrawerHtml(): string {
    const visible = this.filterOpen ? " visible" : "";
    const tabs = (["sources", "tags", "status"] as FilterTab[])
      .map((tab) => `<button class="rh-filter-tab${this.filterTab === tab ? " on" : ""}" data-action="filter-tab" data-tab="${tab}">${esc(LANG.filterTabs[tab])}</button>`)
      .join("");
    return `<div class="rh-drawer-backdrop${visible}" data-action="close-filter"></div>
<aside class="rh-drawer${visible}" aria-hidden="${this.filterOpen ? "false" : "true"}">
<div class="rh-drawer-head"><span>${esc(LANG.filterTitle)}</span><button class="rh-icon-btn" data-action="close-filter" title="${esc(LANG.close)}" aria-label="${esc(LANG.close)}">${rhIcon("close", 16)}</button></div>
<div class="rh-drawer-current"><span>${esc(LANG.filterCurrent)}</span><span class="rh-drawer-current-value">${esc(this.filterScopeLabel())}</span></div>
<div class="rh-filter-tabs">${tabs}</div>
<div class="rh-drawer-body">${this.filterOptionsHtml()}</div>
<div class="rh-drawer-foot">
<button class="rh-btn" data-action="filter-clear">${esc(LANG.filterClear)}</button>
<button class="rh-btn primary" data-action="filter-apply">${esc(LANG.filterApply)}</button>
</div>
</aside>`;
  }

  private filterOptionsHtml(): string {
    if (this.filterTab === "sources") {
      const all = `<button class="rh-opt${this.filterDraftSources.size === 0 ? " on" : ""}" data-action="filter-pick" data-kind="sources" data-value=""><span class="rh-opt-box"></span><span class="rh-opt-name">${esc(LANG.allSources)}</span></button>`;
      const rows = this.state.sources
        .map((source) => {
          const categories = sourceCategories(source.manifest.kind).join(" / ");
          return `<button class="rh-opt${this.filterDraftSources.has(source.id) ? " on" : ""}" data-action="filter-pick" data-kind="sources" data-value="${esc(source.id)}"><span class="rh-opt-box"></span><span class="rh-opt-name">${esc(source.name)}</span>${categories ? `<span class="rh-opt-meta">${esc(categories)}</span>` : ""}</button>`;
        })
        .join("");
      return all + rows;
    }
    if (this.filterTab === "tags") {
      const groups = this.tagGroups(this.filterDraftSources);
      if (groups.length === 0) return `<div class="rh-empty">${esc(LANG.filterNoTags)}</div>`;
      return groups
        .map(
          (group) => `<div class="rh-filter-group">
<div class="rh-filter-group-head"><span>${esc(group.name)}</span>${group.tags.length === 0 ? `<span class="rh-opt-meta">${esc(LANG.filterTagEmptySource)}</span>` : ""}</div>
${group.tags.length > 0
  ? `<div class="rh-filter-tags">${group.tags
      .map((tag) => `<button class="rh-chip${this.filterDraftTags.has(tag) ? " active" : ""}" data-action="filter-pick" data-kind="tags" data-value="${esc(tag)}">${esc(tag)}</button>`)
      .join("")}</div>`
  : ""}
</div>`,
        )
        .join("");
    }
    return (["completed", "ongoing"] as const)
      .map((status) => `<button class="rh-opt${this.filterDraftStatus.has(status) ? " on" : ""}" data-action="filter-pick" data-kind="status" data-value="${status}"><span class="rh-opt-box"></span><span>${esc(LANG.statusNames[status])}</span></button>`)
      .join("");
  }

  private activeFilterCount(): number {
    return this.filterSources.size + this.filterTags.size + this.filterStatus.size;
  }

  /**
   * 标签按来源分组（2026-10-21）：一个源只展示自己的标签。
   *
   * 来源来自三处：总览结果、本轮搜索结果、详情页回填（remoteWorks）；
   * 选中的源即使还没抓到标签也会列出，提示「该源暂无可筛选的标签」。
   */
  private tagGroups(sourceIds: Set<string>): { id: string; name: string; tags: string[] }[] {
    const groups = new Map<string, Set<string>>();
    const wants = (id: string): boolean => sourceIds.size === 0 || sourceIds.has(id);
    const add = (id: string, tags?: string[]): void => {
      if (!wants(id)) return;
      const set = groups.get(id) ?? new Set<string>();
      for (const tag of tags ?? []) {
        const text = tag.trim();
        if (text) set.add(text);
      }
      groups.set(id, set);
    };
    for (const result of [...this.exploreResults, ...this.sourceResults]) {
      for (const work of result.works) add(result.sourceId, work.tags);
    }
    for (const entry of this.remoteWorks.values()) add(entry.record.id, entry.work.tags);
    if (sourceIds.size === 0) {
      for (const work of DEMO_WORKS) add("demo", work.tags);
    }
    for (const id of sourceIds) {
      if (!groups.has(id)) groups.set(id, new Set());
    }
    return [...groups]
      .map(([id, tags]) => ({
        id,
        name: id === "demo" ? LANG.demo : (this.state.sources.find((source) => source.id === id)?.name ?? id),
        tags: [...tags],
      }))
      .filter((group) => group.tags.length > 0 || sourceIds.has(group.id));
  }

  /** 当前分类 / 来源筛选下的搜索范围（搜索与建议共用，2026-10-21）。 */
  private sourceScope(): SourceRecord[] {
    const sources = this.filterSources;
    const kinds = this.filterKinds;
    return this.state.sources.filter((source) => {
      if (sources.size > 0 && !sources.has(source.id)) return false;
      if (kinds.size > 0 && !source.manifest.kind.some((kind) => kinds.has(kind))) return false;
      return true;
    });
  }

  /** 当前分类 / 来源筛选的签名：建议缓存与搜索范围共用。 */
  private scopeSignature(): string {
    return `${[...this.filterSources].sort().join(",")}|${[...this.filterKinds].sort().join(",")}`;
  }

  private matchesFilters(
    work: { kind?: SourceKind; tags?: string[]; status?: Work["status"] },
    sourceId = "",
  ): boolean {
    if (this.filterKinds.size > 0 && (!work.kind || !this.filterKinds.has(work.kind))) return false;
    if (this.filterSources.size > 0 && !this.filterSources.has(sourceId)) return false;
    if (this.filterTags.size > 0 && !(work.tags ?? []).some((tag) => this.filterTags.has(tag))) return false;
    if (this.filterStatus.size > 0 && !this.filterStatus.has(work.status ?? "unknown")) return false;
    return true;
  }

  private resultItems(): ResultItem[] {
    const cacheKey = this.itemsKey();
    if (this.itemsCache?.key === cacheKey) return this.itemsCache.items;

    const started = perfNow();
    let scanned = 0;
    // 总览与搜索共用：来源范围由左下角「更多筛选」决定，内置示例不参与指定来源。
    const overview = this.query.trim() === "";
    // 拼音是增强能力：首屏先按子串出结果，编码回来后清缓存重绘一次。
    if (!overview) {
      this.warmPinyin(
        [
          ...DEMO_WORKS.map((work) => work.title),
          ...[...this.remoteWorks.values()].map((cached) => cached.work.title),
          ...this.activeResults().flatMap((result) => result.works.map((work) => work.title)),
        ],
        () => {
          this.itemsCache = null;
          this.renderResults();
        },
      );
    }
    const demo = searchDemoWorks(
      this.query,
      (work) => this.pinyinRank(work.title, this.query) > 0,
    )
      .filter((work) => this.matchesFilters(work, "demo"))
      .map((work) => {
        const status = work.status ?? "unknown";
        return {
          workId: work.id,
          title: work.title,
          kind: work.kind,
          author: work.author,
          cover: work.cover,
          sub: LANG.statusNames[status],
          sourceName: LANG.demo,
          sourceId: "demo",
          desc: work.description,
          tags: work.tags,
          status,
          updatedAt: work.updatedAt ?? 0,
          chapterCount: work.chapters.length,
        } satisfies ResultItem;
      });

    const remoteStreams: ResultItem[][] = [];
    for (const result of this.activeResults()) {
      if (this.filterSources.size > 0 && !this.filterSources.has(result.sourceId)) continue;
      const record = this.state.sources.find((source) => source.id === result.sourceId);
      const stream: ResultItem[] = [];
      for (const raw of result.works) {
        scanned += 1;
        const key = workKey(raw);
        // 打开详情后补到的简介 / 标签回填到同一 work，卡片随之更新。
        const work = this.remoteWorks.get(key)?.work ?? raw;
        if (!this.matchesFilters({ kind: work.ref.kind, tags: work.tags, status: work.status }, result.sourceId)) {
          continue;
        }
        const status = work.status ?? "unknown";
        stream.push({
          workId: key,
          title: work.title,
          kind: work.ref.kind,
          author: work.author,
          cover: work.cover && record ? proxyMediaUrl(work.cover, record.manifest) : work.cover,
          coverRaw: work.cover,
          sub: LANG.statusNames[status],
          sourceName: result.sourceName,
          sourceId: result.sourceId,
          desc: work.description,
          tags: work.tags ?? [],
          status,
          updatedAt: work.updatedAt ?? 0,
          chapterCount: this.remoteChapters.get(key)?.length ?? 0,
        });
      }
      if (stream.length > 0) remoteStreams.push(stream);
    }

    const streams = [demo, ...remoteStreams].filter((stream) => stream.length > 0);
    let ordered: ResultItem[];
    if (overview) {
      // 每个来源内部先按当前排序，再轮流取；第一段 30 张会尽量覆盖不同来源。
      const queues = streams.map((stream) => this.sortResults(stream));
      const max = queues.reduce((value, queue) => Math.max(value, queue.length), 0);
      ordered = [];
      for (let index = 0; index < max; index += 1) {
        for (const queue of queues) {
          if (index < queue.length) ordered.push(queue[index]);
        }
      }
    } else {
      ordered = streams.flat();
    }

    const groups = new Map<string, ResultItem[]>();
    for (const item of ordered) {
      const key = `${mediaCategory(item.kind)}::${normalizeTitle(item.title)}`;
      const group = groups.get(key);
      if (group) group.push(item);
      else groups.set(key, [item]);
    }
    const merged = [...groups.values()].map((group) => {
      const primary = group.find((item) => item.cover) ?? group[0];
      return {
        ...primary,
        sourceCount: new Set(group.map((item) => item.sourceId ?? item.workId.split("::")[0])).size,
        chapterCount: Math.max(...group.map((item) => item.chapterCount)),
        updatedAt: Math.max(...group.map((item) => item.updatedAt)),
      } satisfies ResultItem;
    });
    const items = overview ? merged : this.sortResults(merged);
    this.itemsCache = { key: cacheKey, items };
    const ms = perfNow() - started;
    if (ms >= 20) {
      perfWarn(`resultItems ${Math.round(ms)}ms`, {
        sources: this.activeResults().length,
        works: scanned,
        items: items.length,
      });
    }
    return items;
  }

  /** 结果缓存键：查询、来源、筛选、排序与数据版本。 */
  private itemsKey(): string {
    return [
      this.query,
      this.sortMode,
      [...this.filterKinds].join(","),
      [...this.filterSources].sort().join(","),
      [...this.filterTags].sort().join(","),
      [...this.filterStatus].sort().join(","),
      String(this.itemsRevision),
    ].join("|");
  }

  /** 来源数据 / 详情回填变化时让结果缓存失效。 */
  private touchItems(): void {
    this.itemsRevision += 1;
  }

  private sortResults(items: ResultItem[]): ResultItem[] {
    const rank = (status: Work["status"]): number =>
      status === "completed" ? 0 : status === "ongoing" ? 1 : 2;
    return [...items].sort((a, b) => {
      if (this.sortMode === "status") return rank(a.status) - rank(b.status);
      if (this.sortMode === "chapters") return b.chapterCount - a.chapterCount;
      return b.updatedAt - a.updatedAt;
    });
  }

  private resultsHtml(): string {
    const items = this.resultItems();
    const warnings = `<div id="rh-warnings">${this.warningsHtml()}</div>`;
    const searching = this.query.trim() !== "";
    const busy = searching ? this.searching : this.exploreLoading;
    // 总览与搜索结果一致：先显示本地窗口，滚到底部再联网取各来源的下一页。
    const shown = items.slice(0, this.resultLimit);
    const hasMore = shown.length < items.length || this.pagedCandidates().length > 0;
    // 「全部来源」的预览提示：只在确实没有更多数据时出现，避免和「加载更多」并存。
    const hint = !searching && this.filterSources.size === 0 && !busy && !hasMore
      ? `<div class="rh-overview-hint">${esc(LANG.overviewPreview)}</div>`
      : "";
    const loader = busy || hasMore || this.moreLoading
      ? `<div id="rh-more" class="rh-more${busy || this.moreLoading ? "" : " idle"}"${busy || this.moreLoading ? "" : ` data-action="load-more"`}>${this.loadingInnerHtml()}</div>`
      : "";
    // 还没有任何结果时用整块加载界面（总览 / 指定来源 / 搜索共用）。
    if (busy && items.length === 0) {
      return `${warnings}<div id="rh-source-loading" class="rh-source-loading">${this.loadingInnerHtml()}</div>`;
    }
    if (items.length === 0) {
      return `${warnings}${hint}${busy || hasMore ? "" : `<div class="rh-empty">${esc(LANG.noResults)}</div>`}${loader}`;
    }
    return `${warnings}<div class="rh-grid">${shown.map((item) => this.gridCard(item)).join("")}</div>${hint}${loader}`;
  }

  /** 统一加载文案：来源进度 + 当前来源名；空闲时提示继续下滑。 */
  private loadingInnerHtml(): string {
    if (this.moreLoading) {
      return `<span class="rh-spinner"></span><span>${esc(LANG.loadingPage)}</span>`;
    }
    const searching = this.query.trim() !== "";
    const busy = searching ? this.searching : this.exploreLoading;
    if (!busy) return `<span>${esc(LANG.loadingMore)}</span>`;
    const label = searching ? LANG.searching : LANG.loadingSources;
    const progress = this.loadingTotal > 0
      ? ` ${Math.min(this.loadingDone + 1, this.loadingTotal)}/${this.loadingTotal}`
      : "";
    const current = this.loadingCurrent ? ` · ${this.loadingCurrent}` : "";
    return `<span class="rh-spinner"></span><span>${esc(label)}${progress}${esc(current)}</span>`;
  }

  /** 只更新加载条，不重绘整个网格（来源逐个完成时调用）。 */
  private updateLoadingDom(): void {
    const html = this.loadingInnerHtml();
    for (const selector of ["#rh-more", "#rh-source-loading"]) {
      const el = this.root.querySelector<HTMLElement>(selector);
      if (el) el.innerHTML = html;
    }
  }

  /** 总览（空查询）看 explore，正式搜索看本轮搜索结果。 */
  private activeResults(): SourceSearchResult[] {
    return this.query.trim() === "" ? this.exploreResults : this.sourceResults;
  }

  /** 站点拒绝 / 限流时补一句可操作的提示，不改变原始错误。 */
  private sourceErrorText(error: string): string {
    return /\bblocked\b|HTTP (?:401|403|429|503)\b/i.test(error)
      ? `${error} · ${LANG.sourceBlockedHint}`
      : error;
  }

  /** 当前仍未关闭的来源错误；每张卡对应一个来源。 */
  private activeWarnings(): { key: string; name: string; message: string }[] {
    return this.activeResults()
      .filter((result) => this.filterSources.size === 0 || this.filterSources.has(result.sourceId))
      .filter((result) => result.error)
      .map((result) => {
        const message = this.sourceErrorText(result.error ?? "");
        return { key: `${result.sourceId}::${message}`, name: result.sourceName, message };
      })
      .filter((warning) => !this.dismissedWarnings.has(warning.key));
  }

  /** 扑克牌式堆叠：只展开最上方一张，关闭后自动露出下一张。 */
  private warningsHtml(): string {
    const warnings = this.activeWarnings();
    if (warnings.length === 0) return "";
    const top = warnings[0];
    const count = warnings.length > 1 ? `<span class="rh-warn-count">${warnings.length}</span>` : "";
    return `<div class="rh-warn-stack${warnings.length > 1 ? " has-more" : ""}">
<div class="rh-warn-card">
<span class="rh-warn-source">${esc(top.name)}</span>
<span class="rh-warn-text">${esc(LANG.sourceError)}: ${esc(top.message)}</span>
<button class="rh-icon-btn rh-warn-close" data-action="dismiss-warning" data-key="${esc(top.key)}" title="${esc(LANG.close)}" aria-label="${esc(LANG.close)}">${rhIcon("close", 14)}</button>
${count}
</div></div>`;
  }

  /** 最上方警告 5s 后自动关闭；集合变化时重新计时。 */
  private syncWarningTimer(): void {
    const top = this.activeWarnings()[0];
    if (!top) {
      if (this.warningTimer) window.clearTimeout(this.warningTimer);
      this.warningTimer = undefined;
      this.warningTimerKey = "";
      return;
    }
    if (this.warningTimer && this.warningTimerKey === top.key) return;
    if (this.warningTimer) window.clearTimeout(this.warningTimer);
    this.warningTimerKey = top.key;
    this.warningTimer = window.setTimeout(() => {
      this.warningTimer = undefined;
      this.warningTimerKey = "";
      this.dismissWarning(top.key);
    }, 5000);
  }

  private dismissWarning(key: string): void {
    if (!key) return;
    this.dismissedWarnings.add(key);
    if (this.dismissedWarnings.size > 100) this.dismissedWarnings.clear();
    if (this.warningTimer) window.clearTimeout(this.warningTimer);
    this.warningTimer = undefined;
    this.warningTimerKey = "";
    // 只换警告区，不重绘整张网格（图片来源多时重绘代价很高）。
    const box = this.root.querySelector<HTMLElement>("#rh-warnings");
    if (box) {
      box.innerHTML = this.warningsHtml();
      this.syncWarningTimer();
    } else {
      this.renderResults();
    }
  }

  /** 输入中的建议：本地缓存即时匹配 + 远程来源补全。 */
  private suggestHtml(): string {
    if (this.query.trim() === "") return "";
    const items = this.suggestItems();
    const loading = this.suggestLoading
      ? `<div class="rh-suggest-state"><span class="rh-spinner"></span><span>${esc(LANG.loadingSuggestions)}</span></div>`
      : "";
    if (items.length === 0) {
      return loading || `<div class="rh-suggest-state">${esc(LANG.suggestionEmpty)}</div>`;
    }
    return `${loading}${items.map((item) => `<button class="rh-suggest-item" data-action="pick-suggestion" data-work="${esc(item.workId)}">
<span class="rh-suggest-cover">${item.cover ? `<img src="${esc(item.cover)}" alt="" loading="lazy">` : esc(item.title.slice(0, 1))}</span>
<span class="rh-suggest-main"><span class="rh-suggest-title">${esc(item.title)}</span><span class="rh-suggest-meta">${esc(item.sourceName ?? "")}${item.author ? ` · ${esc(item.author)}` : ""}</span></span>
</button>`).join("")}`;
  }

  /**
   * 预热拼音缓存（2026-10-22，阅读插件「拼音检索」）。
   *
   * 宿主没有 `pinyin` 能力（旧宿主 / 移动端）时**直接返回** ⇒ 纯子串匹配照旧；
   * 已缓存 / 已排队的标题不会再发请求。编码回来后调用 `onReady`，让调用方重绘
   * （首屏先按子串显示，拼音结果随后补上）。
   */
  private warmPinyin(titles: string[], onReady?: () => void): void {
    const api = this.host.pinyin;
    if (!api || this.pinyinBroken) return;
    const todo = [...new Set(
      titles
        .map((title) => title.trim())
        .filter((title) => title && !this.pinyinCache.has(title) && !this.pinyinPending.has(title)),
    )];
    if (todo.length === 0) return;
    for (const title of todo) this.pinyinPending.add(title);
    void (async () => {
      for (let i = 0; i < todo.length; i += 200) {
        const chunk = todo.slice(i, i + 200);
        try {
          const entries = await api.encode(chunk);
          chunk.forEach((title, index) => {
            const entry = entries[index];
            // 空结果也缓存：只有标点的标题不该被反复排队（否则预热 → 重绘会自激）。
            if (entry) this.pinyinCache.set(title, entry);
          });
        } catch {
          // 拼音是增强能力：宿主拒绝 / 出错就整体关掉，子串匹配不受影响；
          // 不重试也是为了避免「预热失败 → 重绘 → 再预热」自激。
          this.pinyinBroken = true;
          this.pinyinPending.clear();
          break;
        } finally {
          for (const title of chunk) this.pinyinPending.delete(title);
        }
      }
      onReady?.();
    })();
  }

  /** 拼音匹配等级（0 不匹配 / 1 前缀 / 2 包含）；缓存未就绪时恒为 0。 */
  private pinyinRank(title: string, query: string): number {
    return matchPinyin(query, this.pinyinCache.get(title.trim()));
  }

  /** 建议排序：子串优先，拼音命中排在后面，仍比「都不命中」靠前。 */
  private suggestionRankOf(item: { title: string; author?: string }, query: string): number {
    return suggestionRank(item, query, this.pinyinRank(item.title, query));
  }

  private suggestItems(): ResultItem[] {
    const query = this.query.trim().toLowerCase();
    // 首屏先按子串出结果；拼音编码回来后补一次重绘（`suggestOpen` 还在才需要）。
    this.warmPinyin(
      [
        ...DEMO_WORKS.map((work) => work.title),
        ...[...this.remoteWorks.values()].map((cached) => cached.work.title),
        ...this.suggestResults.flatMap((result) => result.works.map((work) => work.title)),
      ],
      () => {
        if (this.suggestOpen) this.renderSuggest();
      },
    );
    const out: ResultItem[] = [];
    const seen = new Set<string>();
    const push = (item: ResultItem): void => {
      if (seen.has(item.workId)) return;
      seen.add(item.workId);
      out.push(item);
    };
    for (const result of this.suggestResults) {
      const record = this.state.sources.find((source) => source.id === result.sourceId);
      for (const work of result.works) {
        push({
          workId: workKey(work),
          title: work.title,
          kind: work.ref.kind,
          author: work.author,
          cover: work.cover && record ? proxyMediaUrl(work.cover, record.manifest) : work.cover,
          coverRaw: work.cover,
          sub: "",
          sourceName: result.sourceName,
          sourceId: result.sourceId,
          desc: work.description,
          tags: work.tags ?? [],
          status: work.status ?? "unknown",
          updatedAt: work.updatedAt ?? 0,
          chapterCount: 0,
        });
      }
    }
    for (const work of searchDemoWorks(
      this.query,
      (demo) => this.pinyinRank(demo.title, this.query) > 0,
    )) {
      push({
        workId: work.id,
        title: work.title,
        kind: work.kind,
        author: work.author,
        cover: work.cover,
        sub: "",
        sourceName: LANG.demo,
        sourceId: "demo",
        desc: work.description,
        tags: work.tags,
        status: work.status ?? "unknown",
        updatedAt: work.updatedAt ?? 0,
        chapterCount: work.chapters.length,
      });
    }
    // 已缓存过的远程作品也参与即时模糊匹配（无需等网络）。
    for (const [key, cached] of this.remoteWorks) {
      const title = cached.work.title.toLowerCase();
      const pinyinHit = this.pinyinRank(cached.work.title, this.query) > 0;
      if (
        query &&
        !title.includes(query) &&
        !(cached.work.author ?? "").toLowerCase().includes(query) &&
        !pinyinHit
      ) {
        continue;
      }
      push({
        workId: key,
        title: cached.work.title,
        kind: cached.work.ref.kind,
        author: cached.work.author,
        cover: this.remoteCover(key),
        coverRaw: cached.work.cover,
        sub: "",
        sourceName: cached.record.name,
        sourceId: cached.record.id,
        desc: cached.work.description,
        tags: cached.work.tags ?? [],
        status: cached.work.status ?? "unknown",
        updatedAt: cached.work.updatedAt ?? 0,
        chapterCount: this.remoteChapters.get(key)?.length ?? 0,
      });
    }
    // 每个来源只留最相关的一条，最终最多 5 条且来源互不重复。
    const best = new Map<string, ResultItem>();
    for (const item of out) {
      const sourceId = item.sourceId ?? "demo";
      const current = best.get(sourceId);
      const rank = this.suggestionRankOf(item, query);
      if (!current || rank < this.suggestionRankOf(current, query)) {
        best.set(sourceId, item);
      }
    }
    return [...best.values()]
      .sort((a, b) => this.suggestionRankOf(a, query) - this.suggestionRankOf(b, query))
      .slice(0, 5);
  }

  private renderSuggest(): void {
    const box = this.root.querySelector<HTMLElement>("#rh-suggest");
    if (!box) return;
    box.classList.toggle("vis", this.suggestOpen && this.query.trim() !== "");
    box.innerHTML = this.suggestHtml();
  }

  /** 远程作品缓存：按最近使用排序，超过上限时淘汰最旧的条目。 */
  private rememberRemoteWork(key: string, record: SourceRecord, work: Work): void {
    this.remoteWorks.delete(key);
    this.remoteWorks.set(key, { record, work });
    while (this.remoteWorks.size > REMOTE_WORK_CACHE_LIMIT) {
      const oldest = this.remoteWorks.keys().next().value;
      if (oldest === undefined) break;
      this.remoteWorks.delete(oldest);
    }
  }

  /** 记录来源健康度：失败进入冷却，成功立即恢复。 */
  private noteSourceHealth(sourceId: string, error?: string): void {
    if (error) this.sourceCooldown.set(sourceId, Date.now() + SOURCE_COOLDOWN_MS);
    else this.sourceCooldown.delete(sourceId);
  }

  private remoteCover(key: string): string | undefined {
    const remote = this.remoteWorks.get(key);
    if (!remote?.work.cover) return undefined;
    return proxyMediaUrl(remote.work.cover, remote.record.manifest);
  }

  /** 封面加载失败时的兜底（§待办 3-20）。
   *
   *  加密图源（漫蛙吧这类）的**封面**和章节图一样是密文：直连 `<img>` 只能得到坏图。
   *  封面没有「章节」这个上下文，走源的 `onThumbnailLoad` —— 由沙箱取字节、按源自己的
   *  规则解成明文，再换成 `blob:` URL。**失败就保持坏图**，不弹提示（与现状观感一致）。 */
  private async repairCover(img: HTMLImageElement): Promise<void> {
    const sourceId = img.dataset.thumbSource ?? "";
    const raw = img.dataset.thumbUrl ?? "";
    if (!sourceId || !raw || img.dataset.thumbRepair === "1") return;
    img.dataset.thumbRepair = "1";
    const record = this.state.sources.find((source) => source.id === sourceId);
    if (!record) return;
    let source: ResourceSource;
    try {
      source = sourceFor(record, this.host);
    } catch {
      return;
    }
    if (!source.thumbnailData) return;
    try {
      const bytes = await source.thumbnailData(raw);
      if (!bytes || bytes.length === 0 || !img.isConnected) return;
      const url = URL.createObjectURL(new Blob([new Uint8Array(bytes)]));
      this.coverBlobs.add(url);
      img.src = url;
    } catch (error) {
      perfWarn("cover transform failed", {
        source: sourceId,
        url: raw,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  }

  /** `error` 不冒泡，只能在 root 上**捕获阶段**听（封面有 lazy 与直接 src 两种写法）。 */
  private onCoverError = (event: Event): void => {
    const target = event.target;
    if (target instanceof HTMLImageElement && target.dataset.thumbUrl) {
      void this.repairCover(target);
    }
  };

  private detailHtml(workId: string): string {
    const remote = this.remoteWorks.get(workId);
    if (remote) return this.remoteDetailHtml(workId, remote.work);
    const work = demoWorkById(workId);
    if (!work) return `<div class="rh-empty">${esc(LANG.noResults)}</div>`;
    const saved = isSaved(this.state, work.id);
    const progress = this.state.progress[work.id];
    const status = work.status ?? "unknown";
    const cover = work.cover
      ? `<img src="${esc(work.cover)}" alt="" loading="lazy">`
      : `<span class="rh-cover-fallback">${esc(work.title.slice(0, 1))}</span>`;
    return `<div class="rh-detail-head">
<div class="rh-detail-cover">${cover}</div>
<div class="rh-row-main">
<div class="rh-detail-title">${esc(work.title)} ${kindTag(work.kind)} <span class="rh-tag">${esc(LANG.statusNames[status])}</span> <span class="rh-tag">${esc(LANG.demo)}</span></div>
<div class="rh-meta">${esc(work.author)}</div>
${work.tags.length ? `<div class="rh-detail-tags">${work.tags.map((tag) => `<span class="rh-tag">${esc(tag)}</span>`).join(" ")}</div>` : ""}
<div class="rh-desc">${esc(work.description)}</div>
<div class="rh-actions">
<button class="rh-btn" data-action="back">${rhIcon("back")}<span>${esc(LANG.back)}</span></button>
<button class="rh-btn primary" data-action="resume-play" data-work="${esc(work.id)}">${rhIcon("play")}<span>${esc(LANG.play)}</span></button>
<button class="rh-btn" data-action="open-source-switch" data-work="${esc(work.id)}">${rhIcon("swap")}<span>${esc(LANG.sourceSwitch)}</span></button>
<button class="rh-btn${saved ? " primary" : ""}" data-action="toggle-save" data-work="${esc(work.id)}">${rhIcon("bookmark")}<span>${esc(saved ? LANG.saved : LANG.save)}</span></button>
</div></div></div>
<div class="rh-meta">${esc(LANG.chapters)} · ${work.chapters.length}</div>
<div class="rh-list">${work.chapters.map((chapter, index) => {
      const current = progress?.chapterIndex === index;
      return `<div class="rh-chapter" data-action="open-chapter" data-work="${esc(work.id)}" data-index="${index}">
<span>${esc(chapter.title)}</span>${current ? `<span class="rh-progress">${esc(LANG.progress)}</span>` : ""}</div>`;
    }).join("")}</div>`;
  }

  private remoteDetailHtml(key: string, work: Work): string {
    const saved = isSaved(this.state, key);
    const record = this.remoteWorks.get(key)?.record;
    const chapters = this.remoteChapters.get(key);
    const error = this.remoteChapterErrors.get(key);
    const loading = this.remoteLoading.has(key);
    const progress = this.state.progress[key];
    if (!chapters && !error && !loading) void this.loadRemoteChapters(key);
    this.ensureRemoteDetail(key);

    let chapterHtml: string;
    if (loading) {
      chapterHtml = `<div class="rh-empty">${esc(LANG.loadingChapters)}</div>`;
    } else if (error) {
      chapterHtml = `<div class="rh-warn">${esc(error)}</div>`;
    } else if (!chapters || chapters.length === 0) {
      chapterHtml = `<div class="rh-empty">${esc(LANG.noChapters)}</div>`;
    } else {
      chapterHtml = chapters
        .map((chapter, index) => {
          const current = progress?.chapterIndex === index;
          return `<div class="rh-chapter" data-action="open-chapter" data-work="${esc(key)}" data-index="${index}">
<span>${esc(chapter.title)}</span>${current ? `<span class="rh-progress">${esc(LANG.progress)}</span>` : ""}</div>`;
        })
        .join("");
    }

    const status = work.status ?? "unknown";
    const coverUrl = this.remoteCover(key);
    // 详情页封面：带上来源与原地址，密文封面由 repairCover 让源自己解（§待办 3-20）。
    const thumb = record && work.cover
      ? ` data-thumb-source="${esc(record.id)}" data-thumb-url="${esc(work.cover)}"`
      : "";
    const cover = coverUrl
      ? `<img src="${esc(coverUrl)}"${thumb} alt="" loading="lazy">`
      : `<span class="rh-cover-fallback">${esc(work.title.slice(0, 1))}</span>`;

    return `<div class="rh-detail-head">
<div class="rh-detail-cover">${cover}</div>
<div class="rh-row-main">
<div class="rh-detail-title">${esc(work.title)} ${kindTag(work.ref.kind)} <span class="rh-tag">${esc(LANG.statusNames[status])}</span>${record ? ` <span class="rh-tag">${esc(record.name)}</span>` : ""}</div>
<div class="rh-meta">${esc(work.author ?? "")}</div>
${work.tags?.length ? `<div class="rh-detail-tags">${work.tags.map((tag) => `<span class="rh-tag">${esc(tag)}</span>`).join(" ")}</div>` : ""}
<div class="rh-desc">${esc(work.description ?? "")}</div>
<div class="rh-actions">
<button class="rh-btn" data-action="back">${rhIcon("back")}<span>${esc(LANG.back)}</span></button>
<button class="rh-btn primary" data-action="resume-play" data-work="${esc(key)}">${rhIcon("play")}<span>${esc(LANG.play)}</span></button>
<button class="rh-btn" data-action="open-source-switch" data-work="${esc(key)}">${rhIcon("swap")}<span>${esc(LANG.sourceSwitch)}</span></button>
<button class="rh-btn${saved ? " primary" : ""}" data-action="toggle-save" data-work="${esc(key)}">${rhIcon("bookmark")}<span>${esc(saved ? LANG.saved : LANG.save)}</span></button>
</div></div></div>
<div class="rh-meta">${esc(LANG.chapters)} · ${chapters ? chapters.length : "…"}</div>
<div class="rh-list">${chapterHtml}</div>`;
  }

  private async loadRemoteChapters(key: string): Promise<Chapter[]> {
    const cached = this.remoteChapters.get(key);
    if (cached) return cached;
    if (this.remoteLoading.has(key)) return [];
    const entry = this.remoteWorks.get(key);
    if (!entry) return [];

    this.remoteLoading.add(key);
    this.remoteChapterErrors.delete(key);
    const started = perfNow();
    try {
      const source = sourceFor(entry.record, this.host);
      if (!source.chapters) {
        throw new RspError("unsupported", `${entry.record.name} 没有 chapters 规则`, entry.record.id);
      }
      // 详情补全（简介 / 标签）只在详情页触发，阅读器 / 播放器路径不做额外请求。
      const chapters = await source.chapters(entry.work.ref);
      this.remoteChapters.set(key, chapters);
      this.touchItems();
      const ms = perfNow() - started;
      const detail = { source: entry.record.id, chapters: chapters.length, ms: Math.round(ms) };
      if (ms >= 1500) perfWarn(`chapters slow ${Math.round(ms)}ms`, detail);
      else perfDebug(`chapters ${Math.round(ms)}ms`, detail);
      return chapters;
    } catch (error) {
      this.remoteChapterErrors.set(key, errorText(error));
      return [];
    } finally {
      this.remoteLoading.delete(key);
      if (this.view.kind === "detail" && this.view.workId === key) this.paint();
    }
  }

  /**
   * 详情页专用：explore / search 只给了标题时，补一次简介 / 标签 / 封面。
   * 预览卡片、阅读器与播放器路径都不调用这里，避免额外拉取详情页。
   */
  private ensureRemoteDetail(key: string): void {
    const entry = this.remoteWorks.get(key);
    if (!entry) return;
    if (this.remoteDetailTried.has(key)) return;
    if (entry.work.description && entry.work.tags?.length) return;
    const source = (() => {
      try {
        return sourceFor(entry.record, this.host);
      } catch {
        return null;
      }
    })();
    if (!source?.detail) return;
    this.remoteDetailTried.add(key);
    const started = perfNow();
    void source
      .detail(entry.work.ref)
      .then((detail) => {
        this.rememberRemoteWork(key, entry.record, {
          ...entry.work,
          author: detail.author ?? entry.work.author,
          cover: entry.work.cover ?? detail.cover,
          description: detail.description ?? entry.work.description,
          tags: detail.tags?.length ? detail.tags : entry.work.tags,
        });
        this.touchItems();
        const ms = perfNow() - started;
        const info = { source: entry.record.id, ms: Math.round(ms) };
        if (ms >= 1500) perfWarn(`detail slow ${Math.round(ms)}ms`, info);
        else perfDebug(`detail ${Math.round(ms)}ms`, info);
        if (this.view.kind === "detail" && this.view.workId === key) this.paint();
      })
      .catch((error) => {
        perfDebug(`detail failed ${Math.round(perfNow() - started)}ms`, {
          source: entry.record.id,
          error: String(error),
        });
      });
  }

  private readerHtml(workId: string, _chapterIndex: number): string {
    if (!demoWorkById(workId) && !this.remoteWorks.has(workId)) {
      return `<div class="rh-empty">${esc(LANG.noResults)}</div>`;
    }
    return `<div id="rh-reader-mount"></div>`;
  }

  private mountReaderView(workId: string, chapterIndex: number): void {
    const mount = this.root.querySelector<HTMLElement>("#rh-reader-mount");
    if (!mount) return;

    // 远程来源：`pendingRemote` 建好之后**不在这里清空** —— `paintNow()` 每次都会重建整棵 DOM，
    // 清掉后重绘就只剩一个空的 `#rh-reader-mount`，用户看到的是「阅读卡死 / 白屏」（2026-10-29 报障）。
    // 重绘时按 `readerSession` 里的当前章节 / 页码原位挂回。
    if (this.pendingRemote && this.pendingRemote.workKey === workId) {
      const pending = this.pendingRemote;
      const resume = this.readerSession && this.readerSession.workKey === workId ? this.readerSession : null;
      const resumeChapter = resume ? resume.chapterIndex : chapterIndex;
      const resumePage = resume ? resume.pageIndex : pending.pageIndex;
      this.readerSession = {
        workKey: workId,
        chapters: pending.chapters.map((chapter) => ({ id: chapter.id, title: chapter.title })),
        baseIndex: 0,
        chapterIndex: resumeChapter,
        pageIndex: resumePage,
      };
      this.readerHandle = mountReader(mount, {
        chapters: pending.chapters,
        initial: { chapterIndex: resumeChapter, pageIndex: resumePage },
        labels: LANG.reader,
        bookmarks: this.state.bookmarks[workId] ?? [],
        loadChapter: (index) => this.loadRemoteReaderChapter(workId, index),
        onExit: () => {
          this.view = { kind: "detail", workId };
          this.paint();
        },
        onProgress: (snapshot) => this.persistReaderProgress(snapshot),
        onBookmarksChange: (bookmarks) => {
          this.state = setBookmarks(this.state, workId, bookmarks);
        },
        onChapterChange: (index) => {
          this.view = { kind: "reader", workId, chapterIndex: index };
          if (this.readerSession) this.readerSession.chapterIndex = index;
        },
      });
      return;
    }

    // 重绘后 `pendingRemote` 已不在（或换了作品），但会话还在：按会话重建章节表再挂一次，
    // 否则这里会直接 return，留下一块空白面板。
    const alive = this.readerSession;
    if (alive && alive.workKey === workId && alive.chapters.length > 0) {
      const chapters: ReaderChapter[] = alive.chapters.map((item, index) => {
        const cached = this.remoteReaderChapters.get(this.remoteChapterKey(workId, index));
        return cached ?? {
          id: item.id,
          title: item.title,
          pages: [{
            id: `${item.id}-loading`,
            kind: "placeholder" as const,
            caption: LANG.reader.loadingChapter,
          }],
          lazy: true,
        };
      });
      this.pendingRemote = { workKey: workId, chapters, pageIndex: alive.pageIndex };
      this.mountReaderView(workId, alive.chapterIndex);
      return;
    }

    const work = demoWorkById(workId);
    if (!work) {
      // 既没有远程章节也没有演示数据：退回详情页，别把用户留在一块空白上。
      this.view = { kind: "detail", workId };
      this.paint();
      return;
    }
    const saved = this.state.progress[workId];
    const initialPage = saved && saved.chapterIndex === chapterIndex ? saved.pageIndex ?? 0 : 0;
    this.readerSession = {
      workKey: workId,
      chapters: work.chapters.map((chapter) => ({ id: chapter.id, title: chapter.title })),
      baseIndex: 0,
      chapterIndex,
      pageIndex: initialPage,
    };
    this.readerHandle = mountReader(mount, {
      chapters: this.buildReaderChapters(work),
      initial: { chapterIndex, pageIndex: initialPage },
      labels: LANG.reader,
      bookmarks: this.state.bookmarks[workId] ?? [],
      onExit: () => {
        this.view = { kind: "detail", workId };
        this.paint();
      },
      onProgress: (snapshot) => this.persistReaderProgress(snapshot),
      onBookmarksChange: (bookmarks) => {
        this.state = setBookmarks(this.state, workId, bookmarks);
      },
      onChapterChange: (index) => {
        // 章节切在阅读器内部完成；这里只同步 view，避免重挂阅读器。
        this.view = { kind: "reader", workId, chapterIndex: index };
        if (this.readerSession) this.readerSession.chapterIndex = index;
      },
    });
  }

  private persistReaderProgress(snapshot: ReaderSnapshot): void {
    const session = this.readerSession;
    if (!session) return;
    const chapter = session.chapters[snapshot.chapterIndex];
    if (!chapter) return;
    // 记住当前位置：整树重绘后按它原位挂回（见 mountReaderView）。
    session.chapterIndex = snapshot.chapterIndex;
    session.pageIndex = snapshot.pageIndex;
    this.state = saveProgress(this.state, session.workKey, {
      chapterId: chapter.id,
      chapterIndex: session.baseIndex + snapshot.chapterIndex,
      chapterTitle: chapter.title,
      pageIndex: snapshot.pageIndex,
    });
  }

  private mountPlayerView(): void {
    const mount = this.root.querySelector<HTMLElement>("#rh-player-mount");
    const pending = this.pendingVideo;
    if (!mount || !pending) return;
    this.videoSession = {
      workId: pending.workId,
      chapterId: pending.chapterId,
      chapterIndex: pending.chapterIndex,
      title: pending.title,
    };
    this.playerHandle = mountPlayer(mount, {
      title: pending.title,
      playbacks: pending.playbacks,
      initialPosition: pending.initialPosition,
      labels: LANG.player,
      resolveUrl: pending.resolveUrl,
      onProgress: (position, duration) => this.persistVideoProgress(position, duration),
      onExit: () => {
        // 退出时**立刻**拆播放器：`paint()` 走 rAF + 120ms 兜底，窗口被隐藏 / 后台
        // 节流时兜底可能延后到 1s 以上，期间视频还在拉流（用户听到「退出后还在响」）。
        this.playerHandle?.destroy();
        this.playerHandle = null;
        this.pendingVideo = null;
        this.view = { kind: "detail", workId: pending.workId };
        this.paint();
      },
    });
  }

  /** 播放器 5 秒节流回写；`saveProgress` 要求带章节信息，所以用会话补全。 */
  private persistVideoProgress(position: number, duration: number): void {
    const session = this.videoSession;
    if (!session) return;
    this.state = saveProgress(this.state, session.workId, {
      chapterId: session.chapterId,
      chapterIndex: session.chapterIndex,
      chapterTitle: session.title,
      positionSec: Math.max(0, Math.floor(position)),
      durationSec: Number.isFinite(duration) ? Math.max(0, Math.floor(duration)) : undefined,
    });
  }

  /**
   * 进入视频播放：`play` 规则解析出线路 → 地址交给宿主媒体代理 →
   * 播放器自己处理 HLS 的分片与字幕。
   */
  private async openVideoChapter(workId: string, index: number, positionSec = 0): Promise<void> {
    const entry = this.remoteWorks.get(workId);
    if (!entry) return;
    const chapters = this.remoteChapters.get(workId) ?? (await this.loadRemoteChapters(workId));
    const chapter = chapters[index];
    if (!chapter) {
      this.notice = LANG.noChapters;
      this.noticeError = true;
      this.paint();
      return;
    }
    this.notice = LANG.player.loading;
    this.noticeError = false;
    this.paint();
    try {
      const source = sourceFor(entry.record, this.host);
      if (!source.play) {
        throw new RspError("unsupported", `${entry.record.name} 没有 play 规则`, entry.record.id);
      }
      const playbacks = await source.play(chapter);
      this.pendingVideo = {
        workId,
        chapterId: chapter.id,
        chapterIndex: index,
        title: `${entry.work.title} · ${chapter.title}`,
        playbacks,
        initialPosition: positionSec,
        resolveUrl: mediaProxyResolver(entry.record.manifest),
      };
      this.state = saveProgress(this.state, workId, {
        chapterId: chapter.id,
        chapterIndex: index,
        chapterTitle: chapter.title,
        positionSec,
      });
      this.view = { kind: "player", workId, chapterIndex: index };
      this.notice = "";
      this.paint();
    } catch (error) {
      this.notice = `${LANG.player.error}: ${errorText(error)}`;
      this.noticeError = true;
      this.paint();
    }
  }

  private buildReaderChapters(work: DemoWork): ReaderChapter[] {
    return work.chapters.map((chapter) => {
      if (work.kind === "novel") {
        const pages = splitTextPages(chapter.id, chapter.content ?? "");
        if (pages.length === 0) {
          pages.push({ id: `${chapter.id}-p1`, kind: "placeholder" as const, caption: chapter.title });
        }
        return { id: chapter.id, title: chapter.title, pages };
      }
      if (work.kind === "comic") {
        const count = chapter.pages ?? 0;
        const pages = Array.from({ length: Math.max(1, count) }, (_, index) => ({
          id: `${chapter.id}-p${index + 1}`,
          kind: "placeholder" as const,
          caption: count > 0 ? `P${index + 1}` : LANG.comicPending,
        }));
        return { id: chapter.id, title: chapter.title, pages };
      }
      return {
        id: chapter.id,
        title: chapter.title,
        pages: [{ id: `${chapter.id}-p1`, kind: "placeholder" as const, caption: chapter.title }],
      };
    });
  }

  private libraryHtml(): string {
    const items = this.libraryTab === "favorites" ? this.favoriteItems() : this.historyItems();
    // 导入 / 导出（B3）：动作按钮按 §界面 4 走「图标 + 文字」，放在标签右侧。
    const head = `<div class="rh-library-head">
<button class="rh-icon-tab${this.libraryTab === "favorites" ? " active" : ""}" data-action="library-tab" data-tab="favorites" title="${esc(LANG.libraryTabs.favorites)}" aria-label="${esc(LANG.libraryTabs.favorites)}">${rhIcon("heart", 18)}</button>
<button class="rh-icon-tab${this.libraryTab === "history" ? " active" : ""}" data-action="library-tab" data-tab="history" title="${esc(LANG.libraryTabs.history)}" aria-label="${esc(LANG.libraryTabs.history)}">${rhIcon("history", 18)}</button>
<div class="rh-library-io">
<button class="rh-btn" data-action="library-export">${rhIcon("export")}<span>${esc(LANG.libraryExport)}</span></button>
<button class="rh-btn" data-action="library-import">${rhIcon("import")}<span>${esc(LANG.libraryImport)}</span></button>
</div>
</div>`;
    if (items.length === 0) {
      const empty = this.libraryTab === "favorites" ? LANG.favoritesEmpty : LANG.historyEmpty;
      return `${head}<div class="rh-empty">${esc(empty)}</div>`;
    }
    return `${head}<div class="rh-grid">${items.map((item) => this.gridCard(item)).join("")}</div>`;
  }

  private favoriteItems(): GridItem[] {
    return [...this.state.library]
      .sort((a, b) => b.updatedAt - a.updatedAt)
      .map((entry) => ({
        workId: entry.workId,
        title: entry.title,
        kind: entry.kind,
        author: entry.author,
        cover: entry.cover ?? this.remoteCover(entry.workId) ?? demoWorkById(entry.workId)?.cover,
        sub: entry.chapterTitle,
      }));
  }

  private historyItems(): GridItem[] {
    const rows = Object.entries(this.state.progress).map(([workId, progress]) => {
      const saved = this.state.library.find((item) => item.workId === workId);
      const remote = this.remoteWorks.get(workId)?.work;
      const demo = demoWorkById(workId);
      return {
        updatedAt: progress.updatedAt ?? 0,
        item: {
          workId,
          title: saved?.title ?? remote?.title ?? demo?.title ?? progress.chapterTitle,
          kind: saved?.kind ?? remote?.ref.kind ?? demo?.kind ?? "novel",
          author: saved?.author ?? remote?.author ?? demo?.author,
          cover: saved?.cover ?? this.remoteCover(workId) ?? demo?.cover,
          coverRaw: this.remoteWorks.get(workId)?.work.cover,
          sub: progress.chapterTitle,
        } satisfies GridItem,
      };
    });
    rows.sort((a, b) => b.updatedAt - a.updatedAt);
    return rows.map((row) => row.item);
  }

  private gridCard(item: GridItem): string {
    // `data-thumb-*` 只在「有来源 + 有原地址」时写：封面加载失败时由 repairCover 让源自己解
    // （加密图源的封面是密文，直连只会是坏图，见 §待办 3-20）。
    const thumb = item.coverRaw && item.sourceId
      ? ` data-thumb-source="${esc(item.sourceId)}" data-thumb-url="${esc(item.coverRaw)}"`
      : "";
    const cover = item.cover
      ? `<img data-src="${esc(item.cover)}"${thumb} alt="" loading="lazy" decoding="async">`
      : `<span class="rh-cover-fallback">${esc(item.title.slice(0, 1))}</span>`;
    const meta = [
      kindTag(item.kind),
      item.sourceName ? `<span class="rh-tag">${esc(item.sourceName)}</span>` : "",
      item.sourceCount && item.sourceCount > 1
        ? `<span class="rh-tag">${esc(LANG.sourcesCount(item.sourceCount))}</span>`
        : "",
      item.sub ? `<span class="rh-tag">${esc(item.sub)}</span>` : "",
    ]
      .filter(Boolean)
      .join(" ");
    const tags = (item.tags ?? [])
      .slice(0, 3)
      .map((tag) => `<span class="rh-tag">${esc(tag)}</span>`)
      .join(" ");
    return `<button class="rh-grid-card" data-action="open-work" data-work="${esc(item.workId)}" title="${esc(item.title)}">
<div class="rh-cover">${cover}</div>
<div class="rh-grid-title">${esc(item.title)}</div>
<div class="rh-grid-meta">${meta || esc(item.author ?? "")}</div>
${tags ? `<div class="rh-grid-tags">${tags}</div>` : ""}
</button>`;
  }

  private sourcesHtml(): string {
    return `<div class="rh-sources-head">
<div class="rh-hint">${esc(LANG.sourcesTitle)}</div>
<button class="rh-btn primary" style="margin-left:auto" data-action="open-import">${rhIcon("import")}<span>${esc(LANG.import)}</span></button>
</div>
<input id="rh-source-q" class="rh-input rh-source-search" value="${esc(this.sourceQuery)}" placeholder="${esc(LANG.sourceSearchPlaceholder)}" autocomplete="off">
<div id="rh-source-lists">${this.sourceListsHtml()}</div>`;
  }

  /** 来源列表：聚合仓库 / 已导入两个可折叠分区，均受搜索词过滤（2026-10-21）。 */
  private sourceListsHtml(): string {
    const imported = this.state.sources.filter((source) =>
      this.matchesSourceQuery(source.name, source.id, source.manifest.baseUrl, source.manifest.homepage),
    );
    const importedBody = imported.length
      ? imported.map((source) => this.sourceHtml(source)).join("")
      : `<div class="rh-empty">${esc(this.sourceQuery.trim() ? LANG.sourceSearchEmpty : LANG.importHint)}</div>`;
    return `${this.registryHtml()}${this.collapseSection(
      "imported",
      LANG.imported,
      imported.length,
      importedBody,
    )}`;
  }

  private matchesSourceQuery(...values: (string | undefined)[]): boolean {
    const query = this.sourceQuery.trim().toLowerCase();
    if (!query) return true;
    return values.some((value) => (value ?? "").toLowerCase().includes(query));
  }

  private collapseSection(
    section: "registry" | "imported",
    title: string,
    count: number,
    body: string,
    actions = "",
  ): string {
    const collapsed = section === "registry" ? this.registryCollapsed : this.importedCollapsed;
    return `<div class="rh-source-section">
<div class="rh-source-section-head">
<button class="rh-source-section-toggle" data-action="toggle-source-section" data-section="${section}" aria-expanded="${collapsed ? "false" : "true"}">
<span class="rh-collapse-caret${collapsed ? "" : " open"}">${rhIcon("chevron", 14)}</span>
<span>${esc(title)}</span><span class="rh-opt-meta">${count}</span>
</button>
${actions}
</div>
${collapsed ? "" : `<div class="rh-source-section-body">${body}</div>`}
</div>`;
  }

  private importHtml(): string {
    return `<div class="rh-import-view">
<div class="rh-detail-title">${esc(LANG.importTitle)}</div>
<div class="rh-hint">${esc(LANG.importUrlHint)}</div>
<input id="rh-import-url" class="rh-input" value="${esc(this.importUrl)}" placeholder="https://..." autocomplete="off" spellcheck="false">
<textarea id="rh-import-json" class="rh-input rh-import-json" placeholder="或粘贴 registry/index.json / {manifest, rule}（可选，离线导入）" spellcheck="false">${esc(this.importJson)}</textarea>
<div class="rh-actions">
<button class="rh-btn" data-action="back-to-sources">${rhIcon("back")}<span>${esc(LANG.back)}</span></button>
<button class="rh-btn primary" data-action="import-url"${this.importing ? " disabled" : ""}>${rhIcon("link")}<span>${esc(this.importing ? LANG.importing : LANG.importGo)}</span></button>
</div>
</div>`;
  }

  /** 书架数据包页（B3，2026-10-28）。
   *
   *  两个方向共用同一个 textarea 与既有「导入规则」页的样式（`rh-import-view` /
   *  `rh-import-json` / `rh-actions`）：不新造弹层，主窗内嵌与独立窗是同一份布局。
   *  导出页的文本框只读，导入页可编辑；按钮可用态跟着文本走（见 onInput）。 */
  private libraryIoHtml(): string {
    const exporting = this.view.kind === "library-io" && this.view.mode === "export";
    const title = exporting ? LANG.libraryIoExportTitle : LANG.libraryIoImportTitle;
    const hint = exporting ? LANG.libraryIoExportHint : LANG.libraryIoImportHint;
    const action = exporting
      ? `<button class="rh-btn primary" data-action="io-copy">${rhIcon("export")}<span>${esc(LANG.libraryIoCopy)}</span></button>`
      : `<button class="rh-btn primary" data-action="io-merge"${this.libraryIoText.trim() ? "" : " disabled"}>${rhIcon("import")}<span>${esc(LANG.libraryIoMerge)}</span></button>`;
    return `<div class="rh-import-view">
<div class="rh-detail-title">${esc(title)}</div>
<div class="rh-hint">${esc(hint)}</div>
<textarea id="rh-io-json" class="rh-input rh-import-json" spellcheck="false"${exporting ? " readonly" : ""}>${esc(this.libraryIoText)}</textarea>
<div class="rh-actions">
<button class="rh-btn" data-action="io-back">${rhIcon("back")}<span>${esc(LANG.back)}</span></button>
${action}
</div>
</div>`;
  }

  /** 复制导出文本；失败不弹原生对话框，只提示并全选文本框让用户手动复制。 */
  private async copyLibraryIo(): Promise<void> {
    let copied = true;
    try {
      await navigator.clipboard.writeText(this.libraryIoText);
    } catch {
      copied = false;
    }
    this.notice = copied ? LANG.libraryIoCopied : LANG.libraryIoCopyFail;
    this.noticeError = !copied;
    this.paint();
    if (!copied) {
      const box = this.root.querySelector<HTMLTextAreaElement>("#rh-io-json");
      box?.focus();
      box?.select();
    }
  }

  /** 合并导入：解析 → 按 `updatedAt` 合并 → 回书架并显示计数。
   *
   *  以 textarea 的当前值为准（用户可能刚粘贴 / 改过），而不是 `libraryIoText` 的旧快照。
   *  失败只显示可读原因（`parseLibraryBundle` 的报错就是给用户看的一句话）。 */
  private mergeLibraryIo(): void {
    const box = this.root.querySelector<HTMLTextAreaElement>("#rh-io-json");
    const text = (box?.value ?? this.libraryIoText).trim();
    if (!text) return; // 空文本时按钮本来就是 disabled，这里只兜底
    try {
      const result = mergeLibraryBundle(this.state, parseLibraryBundle(text));
      this.state = result.state;
      this.view = { kind: "library" };
      this.notice = LANG.libraryIoMerged(
        result.libraryAdded,
        result.libraryUpdated,
        result.progressMerged,
        result.bookmarksAdded,
      );
      this.noticeError = false;
      this.paint();
    } catch (error) {
      this.notice = `${LANG.libraryIoMergeFail}: ${error instanceof Error ? error.message : String(error)}`;
      this.noticeError = true;
      this.paint();
    }
  }

  /** 仓库版本高于已安装版本、且带规则 / 脚本地址的条目。 */
  private updatableRegistryEntries(): { entry: RegistryEntry; installed: SourceRecord }[] {
    const out: { entry: RegistryEntry; installed: SourceRecord }[] = [];
    for (const entry of this.state.registry) {
      if (!entry.source && !entry.sourceUrl) continue;
      const installed = this.state.sources.find((source) => source.id === entry.id);
      if (!installed) continue;
      if (compareVersion(entry.version, installed.version) > 0) out.push({ entry, installed });
    }
    return out;
  }

  private registryHtml(): string {
    if (this.state.registry.length === 0) return "";
    const visible = this.state.registry.filter((entry) =>
      this.matchesSourceQuery(entry.name, entry.id, entry.homepage),
    );
    const installable = this.state.registry.filter(
      (entry) => entry.source || entry.sourceUrl,
    ).length;
    const installed = this.state.registry.filter((entry) =>
      this.state.sources.some((source) => source.id === entry.id),
    ).length;
    const updatable = this.updatableRegistryEntries().length;
    const actions = `<div class="rh-source-section-actions">
<button class="rh-btn" data-action="install-all-registry"${this.batchInstalling || installable === 0 ? " disabled" : ""}>${rhIcon("import")}<span>${esc(this.batchInstalling ? LANG.installingAll : LANG.installAll)}</span></button>
<button class="rh-btn" data-action="update-all-registry"${this.batchUpdating || updatable === 0 ? " disabled" : ""}>${rhIcon("import")}<span>${esc(this.batchUpdating ? LANG.updatingAll : LANG.updateAll)}</span></button>
<button class="rh-btn" data-action="remove-all-registry"${installed === 0 ? " disabled" : ""}>${rhIcon("trash")}<span>${esc(LANG.removeAll)}</span></button>
</div>`;
    const body = visible.length
      ? visible.map((entry) => this.registryRow(entry)).join("")
      : `<div class="rh-empty">${esc(LANG.sourceSearchEmpty)}</div>`;
    return this.collapseSection(
      "registry",
      `${LANG.registry} · ${LANG.registryOnly}`,
      visible.length,
      body,
      actions,
    );
  }

  private registryRow(entry: RegistryEntry): string {
    const installedSource = this.state.sources.find((source) => source.id === entry.id);
    const installable = Boolean(entry.source || entry.sourceUrl);
    const updatable = installedSource && installable
      ? compareVersion(entry.version, installedSource.version) > 0
      : false;
    const meta = [
      `v${entry.version}`,
      updatable && installedSource ? `${LANG.installed} v${installedSource.version}` : "",
      entry.license ?? LANG.noLicense,
    ]
      .filter(Boolean)
      .join(" · ");
    const action = !installable
      ? ""
      : !installedSource
        ? `<button class="rh-btn primary" style="margin-left:auto" data-action="install-registry" data-id="${esc(entry.id)}">${rhIcon("import")}<span>${esc(LANG.install)}</span></button>`
        : `${updatable
            ? `<span class="rh-tag" style="margin-left:auto">${esc(LANG.updateAvailable)}</span>
<button class="rh-btn primary" data-action="update-registry" data-id="${esc(entry.id)}">${rhIcon("import")}<span>${esc(LANG.update)}</span></button>`
            : `<span class="rh-tag" style="margin-left:auto">${esc(LANG.installed)}</span>`}
<button class="rh-btn" data-action="remove-source" data-id="${esc(entry.id)}">${rhIcon("trash")}<span>${esc(LANG.remove)}</span></button>`;
    return `<div class="rh-source"><div class="rh-source-head">
<span class="rh-source-name">${esc(entry.name)}</span>
${action}
</div><div class="rh-hint">${esc(meta)}</div></div>`;
  }

  private sourceHtml(source: SourceRecord): string {
    const warnings = source.warnings.length
      ? `<div class="rh-warn">${esc(LANG.warnings)}: ${esc(source.warnings.join("；"))}</div>`
      : "";
    const categories = sourceCategories(source.manifest.kind)
      .map((category) => `<span class="rh-tag">${esc(category)}</span>`)
      .join("");
    const settingsButton = `<button class="rh-btn" style="margin-left:auto" data-action="open-source-settings" data-id="${esc(source.id)}">${rhIcon("link")}<span>${esc(LANG.sourceSettings)}</span></button>`;
    const registryEntry = this.state.registry.find((entry) => entry.id === source.id);
    const updatable =
      registryEntry && (registryEntry.source || registryEntry.sourceUrl)
        ? compareVersion(registryEntry.version, source.version) > 0
        : false;
    const updateTag = updatable
      ? `<span class="rh-tag">${esc(LANG.updateAvailable)}</span>`
      : "";
    const updateButton = updatable
      ? `<button class="rh-btn primary"${settingsButton ? "" : ' style="margin-left:auto"'} data-action="update-registry" data-id="${esc(source.id)}">${rhIcon("import")}<span>${esc(LANG.update)}</span></button>`
      : "";
    return `<div class="rh-source"><div class="rh-source-head">
<span class="rh-source-name">${esc(source.name)}</span>
${categories}
<span class="rh-hint">v${esc(source.version)}</span>
${updateTag}
${settingsButton}
${updateButton}
<button class="rh-btn"${settingsButton || updateButton ? "" : ' style="margin-left:auto"'} data-action="remove-source" data-id="${esc(source.id)}">${rhIcon("trash")}<span>${esc(LANG.remove)}</span></button>
</div><div class="rh-hint">${esc(source.manifest.engine)}</div>${warnings}</div>`;
  }

  private openFilter(): void {
    this.filterDraftSources = new Set(this.filterSources);
    this.filterDraftTags = new Set(this.filterTags);
    this.filterDraftStatus = new Set(this.filterStatus);
    this.filterTab = "sources";
    this.filterOpen = true;
    this.paint();
  }

  private pickFilter(el: HTMLElement): void {
    const kind = el.dataset.kind as FilterTab;
    const value = el.dataset.value ?? "";
    if (kind === "tags") {
      if (this.filterDraftTags.has(value)) this.filterDraftTags.delete(value);
      else this.filterDraftTags.add(value);
      el.classList.toggle("active", this.filterDraftTags.has(value));
      return;
    }
    const set = kind === "sources" ? this.filterDraftSources : this.filterDraftStatus;
    if (kind === "sources" && value === "") {
      set.clear();
      for (const node of this.root.querySelectorAll<HTMLElement>('[data-action="filter-pick"][data-kind="sources"]')) {
        node.classList.toggle("on", node.dataset.value === "");
      }
      return;
    }
    if (set.has(value)) set.delete(value);
    else set.add(value);
    el.classList.toggle("on", set.has(value));
    if (kind === "sources") {
      const all = this.root.querySelector<HTMLElement>('[data-action="filter-pick"][data-kind="sources"][data-value=""]');
      all?.classList.toggle("on", this.filterDraftSources.size === 0);
    }
  }

  private workMeta(workId: string): { title: string; kind: SourceKind } | null {
    const remote = this.remoteWorks.get(workId);
    if (remote) return { title: remote.work.title, kind: remote.work.ref.kind };
    const demo = demoWorkById(workId);
    if (demo) return { title: demo.title, kind: demo.kind };
    return null;
  }

  private sourceSwitchCacheKey(): string {
    return `${this.sourceSwitchKind}::${normalizeTitle(this.sourceSwitchTitle)}`;
  }

  private openSourceSwitch(workId: string): void {
    const meta = this.workMeta(workId);
    if (!meta) return;
    this.sourceSwitchWorkId = workId;
    this.sourceSwitchTitle = meta.title;
    this.sourceSwitchKind = meta.kind;
    this.sourceSwitchOpen = true;
    this.sourceSwitchEnter = true;
    this.sourceSwitchError = "";
    this.sourceSwitchLoading = false;
    const cached = this.sourceSwitchCache.get(this.sourceSwitchCacheKey());
    const fresh = cached !== undefined && Date.now() - cached.at < 10 * 60_000;
    this.sourceSwitchResults = fresh ? cached.items : [];
    this.paint();
    if (!fresh) void this.runSourceSwitchSearch(false);
  }

  private closeSourceSwitch(): void {
    this.sourceSwitchAbort?.abort();
    this.sourceSwitchAbort = null;
    this.sourceSwitchOpen = false;
    this.sourceSwitchEnter = false;
    this.sourceSwitchLoading = false;
    this.sourceSwitchError = "";
    this.paint();
  }

  /** 按当前作品标题搜索同类型来源；结果按作品 + 类型缓存。 */
  private async runSourceSwitchSearch(force: boolean): Promise<void> {
    const cacheKey = this.sourceSwitchCacheKey();
    const cached = this.sourceSwitchCache.get(cacheKey);
    if (!force && cached && Date.now() - cached.at < 10 * 60_000) {
      this.sourceSwitchResults = cached.items;
      this.paint();
      return;
    }
    // 重试 / 重新搜索：先终止上一轮，否则旧请求的结果会盖住新结果（以前这里直接 return）。
    this.sourceSwitchAbort?.abort();
    const controller = new AbortController();
    this.sourceSwitchAbort = controller;
    const deadline = window.setTimeout(() => controller.abort(), SOURCE_SWITCH_TIMEOUT_MS);
    // 只跑同类型来源：换源只关心 tv / anime / comic 里的同一类。
    // 不筛就会把 110 个来源（含 30s 看门狗的 JS video 源）全跑一遍 —— 抽屉一直转圈。
    const scoped = this.state.sources.filter((source) =>
      source.manifest.kind.some((kind) => sameMediaKind(kind, this.sourceSwitchKind)),
    );
    this.sourceSwitchLoading = true;
    this.sourceSwitchError = "";
    this.sourceSwitchResults = [];
    this.paint();

    const items: SourceSwitchItem[] = [];
    const seen = new Set<string>();
    const collect = (result: SourceSearchResult): boolean => {
      const record = this.state.sources.find((source) => source.id === result.sourceId);
      if (!record) return false;
      let added = false;
      for (const work of result.works) {
        if (!sameMediaKind(work.ref.kind, this.sourceSwitchKind)) continue;
        if (!sameWorkTitle(this.sourceSwitchTitle, work.title)) continue;
        const key = workKey(work);
        if (seen.has(key)) continue;
        seen.add(key);
        this.rememberRemoteWork(key, record, work);
        items.push({
          key,
          sourceName: result.sourceName,
          title: work.title,
          kind: work.ref.kind,
          cover: work.cover ? proxyMediaUrl(work.cover, record.manifest) : undefined,
        });
        added = true;
      }
      return added;
    };

    try {
      await searchSources(scoped, this.sourceSwitchTitle, this.host, {
        signal: controller.signal,
        // 边搜边出结果：单个来源返回就追加，用户不用等全部跑完。
        onSourceDone: (result) => {
          if (this.sourceSwitchAbort !== controller) return;
          if (!collect(result)) return;
          this.sourceSwitchResults = this.sortSourceSwitchItems(items);
          this.paint();
        },
      });
      if (this.sourceSwitchAbort !== controller) return;
      this.sourceSwitchResults = this.sortSourceSwitchItems(items);
      this.sourceSwitchCache.set(cacheKey, { at: Date.now(), items: this.sourceSwitchResults });
      this.sourceSwitchLoading = false;
      this.sourceSwitchError = "";
      this.paint();
      if (this.sourceSwitchResults.length > 0) void this.enrichSourceSwitchItems(this.sourceSwitchResults);
    } catch (error) {
      if (this.sourceSwitchAbort !== controller) return;
      this.sourceSwitchLoading = false;
      this.sourceSwitchError = errorText(error);
      this.paint();
    } finally {
      window.clearTimeout(deadline);
      if (this.sourceSwitchAbort === controller) this.sourceSwitchAbort = null;
    }
  }

  /** 当前作品排最前，其余按来源名排序。 */
  private sortSourceSwitchItems(items: SourceSwitchItem[]): SourceSwitchItem[] {
    return [...items].sort((a, b) => {
      if (a.key === this.sourceSwitchWorkId) return -1;
      if (b.key === this.sourceSwitchWorkId) return 1;
      return a.sourceName.localeCompare(b.sourceName, "zh-CN");
    });
  }

  /** 后台读取每个来源的最新章节标题；单行完成后只更新该行，避免列表滚动位置跳动。 */
  private async enrichSourceSwitchItems(items: SourceSwitchItem[]): Promise<void> {
    const queue = items.map((item, index) => ({ item, index }));
    const workers = Math.min(3, queue.length);
    await Promise.all(
      Array.from({ length: workers }, async () => {
        for (;;) {
          const next = queue.shift();
          if (!next) return;
          const chapters = await this.loadChaptersSilently(next.item.key);
          next.item.chapterCount = chapters.length;
          next.item.latestChapter = latestChapterTitle(chapters);
          if (this.sourceSwitchOpen && this.sourceSwitchResults[next.index] === next.item) {
            this.updateSourceSwitchRow(next.index, next.item);
          }
        }
      }),
    );
  }

  private async loadChaptersSilently(key: string): Promise<Chapter[]> {
    const cached = this.remoteChapters.get(key);
    if (cached) return cached;
    const entry = this.remoteWorks.get(key);
    if (!entry) return [];
    try {
      const source = sourceFor(entry.record, this.host);
      if (!source.chapters) return [];
      const chapters = await source.chapters(entry.work.ref);
      this.remoteChapters.set(key, chapters);
      return chapters;
    } catch {
      return [];
    }
  }

  private updateSourceSwitchRow(index: number, item: SourceSwitchItem): void {
    const row = this.root.querySelector<HTMLElement>(`[data-switch-index="${index}"]`);
    if (!row) return;
    const sub = row.querySelector<HTMLElement>(".rh-switch-sub");
    if (sub) sub.textContent = this.sourceSwitchSub(item);
  }

  private sourceSwitchSub(item: SourceSwitchItem): string {
    if (item.latestChapter) return `${LANG.latest} · ${item.latestChapter}`;
    if (item.chapterCount && item.chapterCount > 0) return `${item.chapterCount} ${LANG.chapterUnit}`;
    return LANG.readingChapters;
  }

  private sourceSwitchRow(item: SourceSwitchItem, index: number): string {
    const cover = item.cover
      ? `<img src="${esc(item.cover)}" alt="" loading="lazy">`
      : `<span class="rh-cover-fallback">${esc(item.title.slice(0, 1))}</span>`;
    const current = item.key === this.sourceSwitchWorkId;
    return `<button class="rh-switch-row" data-action="switch-source" data-index="${index}" data-switch-index="${index}">
<div class="rh-switch-cover">${cover}</div>
<div class="rh-switch-main">
<div class="rh-switch-title">${esc(item.title)}</div>
<div class="rh-switch-sub">${esc(this.sourceSwitchSub(item))}</div>
</div>
<div class="rh-switch-source"><span>${esc(item.sourceName)}</span>${current ? `<span class="rh-tag">${esc(LANG.currentSource)}</span>` : ""}</div>
</button>`;
  }

  private sourceSwitchSheetHtml(): string {
    const visible = this.sourceSwitchOpen && !this.sourceSwitchEnter ? " visible" : "";
    let body: string;
    if (this.sourceSwitchLoading && this.sourceSwitchResults.length === 0) {
      body = `<div class="rh-empty">${esc(LANG.sourceSwitchSearching)}</div>`;
    } else if (this.sourceSwitchError) {
      body = `<div class="rh-empty">${esc(this.sourceSwitchError)}</div>
<div class="rh-actions" style="justify-content:center"><button class="rh-btn primary" data-action="source-switch-retry">${rhIcon("refresh")}<span>${esc(LANG.retry)}</span></button></div>`;
    } else if (this.sourceSwitchResults.length === 0) {
      body = `<div class="rh-empty">${esc(LANG.sourceSwitchEmpty)}</div>`;
    } else {
      body = this.sourceSwitchResults.map((item, index) => this.sourceSwitchRow(item, index)).join("");
      // 仍在搜索：结果先展示，底部补一行进度，避免看起来「已经搜完了」。
      if (this.sourceSwitchLoading) {
        body += `<div class="rh-empty">${esc(LANG.sourceSwitchSearching)}</div>`;
      }
    }
    return `<div class="rh-sheet-backdrop${visible}" data-action="close-source-switch"></div>
<aside class="rh-sheet${visible}" aria-hidden="${this.sourceSwitchOpen ? "false" : "true"}">
<div class="rh-sheet-head">
<span>${esc(LANG.sourceSwitchTitle)}</span>
<div class="rh-sheet-actions">
<button class="rh-btn" data-action="source-switch-retry"${this.sourceSwitchLoading ? " disabled" : ""}>${rhIcon("refresh")}<span>${esc(LANG.retry)}</span></button>
<button class="rh-icon-btn" data-action="close-source-switch" title="${esc(LANG.close)}" aria-label="${esc(LANG.close)}">${rhIcon("close", 16)}</button>
</div>
</div>
<div class="rh-sheet-sub">${esc(this.sourceSwitchTitle)}${this.sourceSwitchTitle ? " · " : ""}${esc(LANG.kind[this.sourceSwitchKind])}</div>
<div class="rh-sheet-body">${body}</div>
</aside>`;
  }

  private settingsSheetHtml(): string {
    const source = this.state.sources.find((item) => item.id === this.settingsSourceId);
    if (!source) return "";
    // 声明式来源没有脚本设置，但仍允许覆盖请求头（反爬）。
    const fields = source.script ? parseSourceSettingFields(source.script) : [];
    const body = fields
      .map((field) => {
        const value =
          this.settingsDraft[field.key] ?? source.manifest.settings?.[field.key] ?? "";
        return `<label class="rh-settings-field"><span>${esc(field.title)} <code>${esc(field.key)}</code></span><input class="rh-input" data-setting-key="${esc(field.key)}" value="${esc(value)}" placeholder="${esc(field.defaultValue)}" autocomplete="off" spellcheck="false"></label>`;
      })
      .join("");
    const headerFields = [
      { key: "User-Agent", label: LANG.headerUserAgent, placeholder: "Mozilla/5.0 …" },
      { key: "Referer", label: LANG.headerReferer, placeholder: "https://…" },
      { key: "Cookie", label: LANG.headerCookie, placeholder: "sid=…" },
    ];
    const headersBody = headerFields
      .map((field) => {
        const value = this.settingsHeaders[field.key] ?? "";
        return `<label class="rh-settings-field"><span>${esc(field.label)}</span><input class="rh-input" data-header-key="${esc(field.key)}" value="${esc(value)}" placeholder="${esc(field.placeholder)}" autocomplete="off" spellcheck="false"></label>`;
      })
      .join("");
    return `<div class="rh-sheet-backdrop visible" data-action="close-source-settings"></div>
<aside class="rh-sheet visible" aria-hidden="false">
<div class="rh-sheet-head"><span>${esc(LANG.sourceSettingsTitle)}</span>
<div class="rh-sheet-actions">
<button class="rh-btn" data-action="probe-source-settings"${fields.length === 0 || this.probingSettings ? " disabled" : ""}>${rhIcon("search")}<span>${esc(this.probingSettings ? LANG.probing : LANG.probeDomains)}</span></button>
<button class="rh-btn primary" data-action="save-source-settings"><span>${esc(LANG.sourceSettingsSave)}</span></button>
<button class="rh-icon-btn" data-action="close-source-settings" title="${esc(LANG.close)}" aria-label="${esc(LANG.close)}">${rhIcon("close", 16)}</button>
</div></div>
<div class="rh-sheet-sub">${esc(source.name)} · ${esc(LANG.sourceSettingsHint)}</div>
<div class="rh-sheet-body"><div class="rh-settings-fields">${body || `<div class="rh-empty">${esc(LANG.sourceSettingsHint)}</div>`}</div>
<div class="rh-hint" style="margin:14px 0 6px">${esc(LANG.sourceHeadersTitle)}</div>
<div class="rh-settings-fields">${headersBody}</div>
<div class="rh-hint">${esc(LANG.sourceHeadersHint)}</div></div>
</aside>`;
  }

  /** 保存某个来源的域名与请求头覆盖；留空即删除对应键。 */
  private saveSourceSettings(): void {
    const source = this.state.sources.find((item) => item.id === this.settingsSourceId);
    if (!source) return;
    this.probeToken += 1;
    this.probingSettings = false;
    const settings: Record<string, string> = { ...(source.manifest.settings ?? {}) };
    const fields = source.script ? parseSourceSettingFields(source.script) : [];
    for (const field of fields) {
      const value = (this.settingsDraft[field.key] ?? "").trim();
      if (value) settings[field.key] = value;
      else delete settings[field.key];
    }
    const headers: Record<string, string> = {};
    for (const [key, value] of Object.entries(this.settingsHeaders)) {
      const trimmed = value.trim();
      if (trimmed) headers[key] = trimmed;
    }
    const manifest = validateSourceManifest({
      ...source.manifest,
      settings: Object.keys(settings).length ? settings : undefined,
      headers: Object.keys(headers).length ? headers : undefined,
    });
    const updated: SourceRecord = { ...source, manifest };
    this.state = {
      ...this.state,
      sources: this.state.sources.map((item) => (item.id === source.id ? updated : item)),
    };
    saveState(this.state);
    persistHostSource(updated);
    this.sourceSwitchCache.clear();
    this.settingsOpen = false;
    this.settingsSourceId = "";
    this.settingsDraft = {};
    this.settingsHeaders = {};
    this.notice = LANG.sourceSettingsSaved;
    this.noticeError = false;
    this.invalidateOverview();
    this.paint();
    void this.refreshOverview(true);
  }

  /**
   * 域名自动探活：按当前值 / 默认值 / 脚本里出现的域名逐个请求根地址，
   * 命中后填入草稿（不自动保存），由用户确认保存。
   */
  private async probeSourceSettings(): Promise<void> {
    if (this.probingSettings) return;
    const source = this.state.sources.find((item) => item.id === this.settingsSourceId);
    if (!source?.script) return;
    const fields = parseSourceSettingFields(source.script);
    if (fields.length === 0) {
      this.notice = LANG.probeNoFields;
      this.noticeError = true;
      this.paint();
      return;
    }
    const hosts = extractDomainCandidates(source.script);
    const token = ++this.probeToken;
    this.probingSettings = true;
    this.notice = LANG.probing;
    this.noticeError = false;
    this.paint();

    for (const field of fields) {
      const templates = [
        this.settingsDraft[field.key],
        source.manifest.settings?.[field.key],
        field.defaultValue,
      ].filter((value): value is string => Boolean(value && value.trim()));
      const pool = new Set<string>();
      for (const value of templates) {
        const host = normalizeHost(value);
        if (host) pool.add(host);
      }
      for (const host of hosts) pool.add(host);
      for (const host of [...pool].slice(0, 12)) {
        let reachable = false;
        try {
          const response = await this.host.net.request({
            url: probeUrlFor(host),
            method: "GET",
            timeoutMs: 4000,
            maxBytes: 2048,
          });
          reachable = response.status > 0 && response.status < 500;
        } catch {
          reachable = false;
        }
        if (token !== this.probeToken) return;
        if (!reachable) continue;
        this.probingSettings = false;
        this.settingsDraft[field.key] = formatHostValue(templates[0] ?? "", host);
        this.notice = `${LANG.probeOk}: ${host}`;
        this.noticeError = false;
        this.paint();
        return;
      }
    }

    this.probingSettings = false;
    this.notice = LANG.probeNone;
    this.noticeError = true;
    this.paint();
  }

  private resumePlay(workId: string): void {
    const progress = this.state.progress[workId];
    if (this.remoteWorks.has(workId)) {
      void this.resumeRemote(workId, progress);
      return;
    }
    const work = demoWorkById(workId);
    if (!work) return;
    if (work.kind === "anime" || work.kind === "tv") {
      // 演示来源只有剧集结构，没有真实播放地址。
      this.notice = LANG.player.noSource;
      this.noticeError = false;
      this.paint();
      return;
    }
    const index = progress && work.chapters[progress.chapterIndex] ? progress.chapterIndex : 0;
    this.openChapter(workId, index);
  }

  private async resumeRemote(workId: string, progress: Progress | undefined): Promise<void> {
    const chapters = this.remoteChapters.get(workId) ?? (await this.loadRemoteChapters(workId));
    if (chapters.length === 0) {
      this.notice = LANG.noChapters;
      this.noticeError = true;
      this.paint();
      return;
    }
    const index = progress && chapters[progress.chapterIndex] ? progress.chapterIndex : 0;
    const kind = this.remoteWorks.get(workId)?.work.ref.kind;
    if (kind === "anime" || kind === "tv") {
      const position = progress && progress.chapterIndex === index ? progress.positionSec ?? 0 : 0;
      await this.openVideoChapter(workId, index, position);
      return;
    }
    await this.openRemoteChapter(workId, index, progress?.pageIndex ?? 0);
  }

  private onClick(event: MouseEvent): void {
    const target = event.target as HTMLElement | null;
    if (this.suggestOpen && !target?.closest(".rh-search")) {
      this.suggestOpen = false;
      this.renderSuggest();
    }
    const el = target?.closest<HTMLElement>("[data-action]");
    if (!el) return;
    const action = el.dataset.action;
    switch (action) {
      case "tab":
        this.view = { kind: (el.dataset.tab as Tab) ?? "overview" };
        this.filterOpen = false;
        this.sourceSwitchOpen = false;
        this.sourceSwitchEnter = false;
        this.notice = "";
        this.resetResultWindow();
        this.paint();
        if (this.view.kind === "overview") void this.refreshOverview();
        return;
      case "library-tab":
        this.libraryTab = (el.dataset.tab as LibraryTab) ?? "favorites";
        this.paint();
        return;
      case "library-export": {
        const empty =
          this.state.library.length === 0 && Object.keys(this.state.progress).length === 0;
        if (empty) {
          this.notice = LANG.libraryIoEmpty;
          this.noticeError = true;
          this.paint();
          return;
        }
        this.libraryIoText = JSON.stringify(buildLibraryBundle(this.state), null, 2);
        this.view = { kind: "library-io", mode: "export" };
        this.notice = "";
        this.paint();
        return;
      }
      case "library-import":
        this.libraryIoText = "";
        this.view = { kind: "library-io", mode: "import" };
        this.notice = "";
        this.paint();
        return;
      case "io-back":
        this.view = { kind: "library" };
        this.notice = "";
        this.paint();
        return;
      case "io-copy":
        void this.copyLibraryIo();
        return;
      case "io-merge":
        this.mergeLibraryIo();
        return;
      case "open-filter":
        this.openFilter();
        return;
      case "close-filter":
        this.filterOpen = false;
        this.paint();
        return;
      case "filter-tab":
        this.filterTab = (el.dataset.tab as FilterTab) ?? "sources";
        this.paint();
        return;
      case "filter-pick":
        this.pickFilter(el);
        return;
      case "remove-tag": {
        const tag = el.dataset.tag ?? "";
        this.filterTags.delete(tag);
        this.filterDraftTags.delete(tag);
        this.resetResultWindow();
        this.touchItems();
        this.paint();
        return;
      }
      case "filter-clear":
        this.filterDraftSources.clear();
        this.filterDraftTags.clear();
        this.filterDraftStatus.clear();
        this.paint();
        return;
      case "filter-apply":
        this.filterSources = new Set(this.filterDraftSources);
        this.filterTags = new Set(this.filterDraftTags);
        this.filterStatus = new Set(this.filterDraftStatus);
        this.filterOpen = false;
        this.resetResultWindow();
        this.paint();
        // 选源后需要重新抓该源的总览（此前可能从未加载过）。
        if (this.view.kind === "overview" && this.query.trim() === "") void this.refreshOverview(true);
        return;
      case "kind-filter": {
        const kind = el.dataset.kind as SourceKind | "" | undefined;
        if (!kind) this.filterKinds.clear();
        else if (this.filterKinds.has(kind)) this.filterKinds.delete(kind);
        else this.filterKinds.add(kind);
        this.resetResultWindow();
        this.paint();
        // 「分类 → 视频 / 番剧」要补抓此前未加载的源（含空关键词浏览的番剧源）。
        if (this.view.kind === "overview" && this.query.trim() === "") void this.refreshOverview(true);
        return;
      }
      case "dismiss-warning":
        this.dismissWarning(el.dataset.key ?? "");
        return;
      case "load-more":
        this.scrollArmed = true;
        void this.loadMoreResults();
        return;
      case "sort-mode":
        this.sortMode = (el.dataset.mode as SortMode) ?? "updated";
        this.resetResultWindow();
        this.paint();
        return;
      case "search":
        this.commitSearch();
        return;
      case "pick-suggestion": {
        const workId = el.dataset.work ?? "";
        this.suggestOpen = false;
        this.view = { kind: "detail", workId };
        this.notice = "";
        this.paint();
        return;
      }
      case "resume-play":
        this.resumePlay(el.dataset.work ?? "");
        return;
      case "open-source-switch":
        this.openSourceSwitch(el.dataset.work ?? "");
        return;
      case "close-source-switch":
        this.closeSourceSwitch();
        return;
      case "source-switch-retry":
        void this.runSourceSwitchSearch(true);
        return;
      case "open-source-settings": {
        const id = el.dataset.id ?? "";
        const source = this.state.sources.find((item) => item.id === id);
        if (!source) return;
        this.probeToken += 1;
        this.probingSettings = false;
        this.settingsSourceId = id;
        this.settingsDraft = { ...(source.manifest.settings ?? {}) };
        this.settingsHeaders = { ...(source.manifest.headers ?? {}) };
        this.settingsOpen = true;
        this.paint();
        return;
      }
      case "close-source-settings":
        this.probeToken += 1;
        this.probingSettings = false;
        this.settingsOpen = false;
        this.settingsSourceId = "";
        this.settingsDraft = {};
        this.settingsHeaders = {};
        this.paint();
        return;
      case "save-source-settings":
        this.saveSourceSettings();
        return;
      case "probe-source-settings":
        void this.probeSourceSettings();
        return;
      case "switch-source": {
        const index = Number(el.dataset.index ?? "-1");
        const item = this.sourceSwitchResults[index];
        if (!item) return;
        this.sourceSwitchOpen = false;
        this.sourceSwitchEnter = false;
        this.sourceSwitchLoading = false;
        if (item.key !== this.sourceSwitchWorkId) {
          this.view = { kind: "detail", workId: item.key };
          this.notice = "";
        }
        this.paint();
        return;
      }
      case "open-work":
        this.sourceSwitchOpen = false;
        this.sourceSwitchEnter = false;
        this.view = { kind: "detail", workId: el.dataset.work ?? "" };
        this.notice = "";
        this.paint();
        return;
      case "open-chapter": {
        const workId = el.dataset.work ?? "";
        const index = Number(el.dataset.index ?? "0");
        if (this.remoteWorks.has(workId)) void this.openRemoteChapter(workId, index);
        else this.openChapter(workId, index);
        return;
      }
      case "back":
        this.view =
          this.view.kind === "reader" || this.view.kind === "player"
            ? { kind: "detail", workId: this.view.workId }
            : { kind: "overview" };
        this.suggestOpen = false;
        this.paint();
        if (this.view.kind === "overview") void this.refreshOverview();
        return;
      case "open-import":
        this.view = { kind: "import" };
        this.notice = "";
        this.paint();
        return;
      case "back-to-sources":
        this.view = { kind: "sources" };
        this.notice = "";
        this.paint();
        return;
      case "import-url":
        void this.importFromUrl();
        return;
      case "toggle-save": {
        const key = el.dataset.work ?? "";
        const remote = this.remoteWorks.get(key);
        if (remote) {
          this.state = toggleSaved(this.state, {
            workId: key,
            title: remote.work.title,
            kind: remote.work.ref.kind,
            author: remote.work.author ?? "",
            cover: this.remoteCover(key),
          });
          this.paint();
          return;
        }
        const work = demoWorkById(key);
        if (!work) return;
        this.state = toggleSaved(this.state, {
          workId: work.id,
          title: work.title,
          kind: work.kind,
          author: work.author,
          cover: work.cover,
        });
        this.paint();
        return;
      }
      case "install-registry":
        this.installRegistryEntry(el.dataset.id ?? "");
        return;
      case "install-all-registry":
        void this.installAllRegistryEntries();
        return;
      case "update-registry":
        void this.updateRegistryEntry(el.dataset.id ?? "");
        return;
      case "update-all-registry":
        void this.updateAllRegistryEntries();
        return;
      case "remove-all-registry":
        this.removeAllRegistrySources();
        return;
      case "toggle-source-section": {
        const section = el.dataset.section;
        if (section === "registry") this.registryCollapsed = !this.registryCollapsed;
        else if (section === "imported") this.importedCollapsed = !this.importedCollapsed;
        const lists = this.root.querySelector<HTMLElement>("#rh-source-lists");
        if (lists) lists.innerHTML = this.sourceListsHtml();
        else this.paint();
        return;
      }
      case "remove-source": {
        const id = el.dataset.id ?? "";
        this.state = removeSource(this.state, id);
        deleteHostSource(id);
        this.forgetOverviewSource(id);
        this.filterSources.delete(id);
        this.filterDraftSources.delete(id);
        this.sourceSwitchCache.clear();
        this.invalidateOverview();
        this.paint();
        void this.refreshOverview(true);
        return;
      }
      default:
        return;
    }
  }

  private onInput(event: Event): void {
    const target = event.target as HTMLInputElement | HTMLTextAreaElement | null;
    const headerKey = (target as HTMLElement | null)?.dataset.headerKey;
    if (headerKey !== undefined) {
      this.settingsHeaders[headerKey] = target?.value ?? "";
      return;
    }
    const settingKey = (target as HTMLElement | null)?.dataset.settingKey;
    if (settingKey !== undefined) {
      this.settingsDraft[settingKey] = target?.value ?? "";
      return;
    }
    if (target?.id === "rh-import-url") {
      this.importUrl = target.value;
      return;
    }
    if (target?.id === "rh-import-json") {
      this.importJson = target.value;
      return;
    }
    if (target?.id === "rh-io-json") {
      this.libraryIoText = target.value;
      // 「合并导入」的可用态只切 disabled，不整体重绘（保住焦点与光标）。
      const merge = this.root.querySelector<HTMLButtonElement>('[data-action="io-merge"]');
      if (merge) merge.disabled = target.value.trim() === "";
      return;
    }
    if (target?.id === "rh-source-q") {
      this.sourceQuery = target.value;
      // 只重绘列表，输入框保持焦点与光标。
      const lists = this.root.querySelector<HTMLElement>("#rh-source-lists");
      if (lists) lists.innerHTML = this.sourceListsHtml();
      return;
    }
    if (target?.id !== "rh-q") return;
    this.query = target.value;
    this.refreshSuggest();
  }

  private onKeyDown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      if (this.suggestOpen) {
        this.suggestOpen = false;
        this.renderSuggest();
        return;
      }
      if (this.settingsOpen) {
        this.settingsOpen = false;
        this.settingsSourceId = "";
        this.settingsDraft = {};
        this.paint();
        return;
      }
      if (this.sourceSwitchOpen) {
        this.closeSourceSwitch();
        return;
      }
      if (this.filterOpen) {
        this.filterOpen = false;
        this.paint();
        return;
      }
      // 书架数据包页（B3）：Esc 回书架，不整窗退出（与「导入规则」页同语义）。
      if (this.view.kind === "library-io") {
        this.view = { kind: "library" };
        this.notice = "";
        this.paint();
        return;
      }
    }
    if (event.key !== "Enter") return;
    const target = event.target as HTMLElement | null;
    if (target?.id === "rh-import-url") {
      event.preventDefault();
      void this.importFromUrl();
      return;
    }
    if (target?.id === "rh-q") {
      event.preventDefault();
      this.commitSearch();
    }
  }

  /** 输入时只刷新建议下拉框；正式搜索必须回车 / 点搜索按钮。 */
  private refreshSuggest(): void {
    const query = this.query.trim();
    if (this.suggestTimer) {
      window.clearTimeout(this.suggestTimer);
      this.suggestTimer = undefined;
    }
    if (!query) {
      this.suggestToken += 1;
      this.suggestAbort?.abort();
      this.suggestAbort = null;
      this.suggestLoading = false;
      this.suggestReady = false;
      this.suggestComplete = false;
      this.suggestResults = [];
      this.suggestQuery = "";
      this.suggestSource = "";
      this.suggestOpen = false;
      this.sourceResults = [];
      this.searching = false;
      this.searchToken += 1;
      this.searchAbort?.abort();
      this.searchAbort = null;
      this.resetResultWindow();
      this.renderSuggest();
      this.renderResults();
      return;
    }
    this.suggestOpen = true;
    this.renderSuggest();
    this.suggestTimer = window.setTimeout(() => void this.runSuggestSearch(), 300);
  }

  private async runSuggestSearch(): Promise<void> {
    const query = this.query.trim();
    const token = ++this.suggestToken;
    this.suggestAbort?.abort();
    this.suggestAbort = null;
    if (!query) return;
    const controller = new AbortController();
    this.suggestAbort = controller;
    this.suggestLoading = true;
    this.suggestReady = false;
    this.suggestComplete = false;
    this.suggestQuery = query;
    this.suggestSource = this.scopeSignature();
    this.suggestResults = [];
    this.renderSuggest();
    try {
      // 分段预览：每批少量来源，拿到 5 个不同源就停，避免快速键入时拖垮全部来源。
      const sources = shuffle(this.sourceScope());
      const collected: SourceSearchResult[] = [];
      let foundSources = 0;
      let complete = true;
      for (let index = 0; index < sources.length; index += SUGGEST_SOURCE_BATCH) {
        if (token !== this.suggestToken || controller.signal.aborted) return;
        const batch = sources.slice(index, index + SUGGEST_SOURCE_BATCH);
        const results = await searchSources(batch, query, this.host, {
          signal: controller.signal,
        });
        if (token !== this.suggestToken || controller.signal.aborted) return;
        const records = new Map(batch.map((record) => [record.id, record]));
        for (const result of results) {
          collected.push(result);
          if (result.works.length > 0) foundSources += 1;
          const record = records.get(result.sourceId);
          if (!record) continue;
          for (const work of result.works) {
            this.rememberRemoteWork(workKey(work), record, work);
          }
        }
        this.suggestResults = [...collected];
        const moreBatches = index + SUGGEST_SOURCE_BATCH < sources.length;
        this.suggestLoading = moreBatches && foundSources < 5;
        this.renderSuggest();
        if (foundSources >= 5) {
          complete = false;
          break;
        }
      }
      this.suggestComplete = complete;
    } finally {
      if (token === this.suggestToken) {
        this.suggestLoading = false;
        this.suggestReady = true;
        this.renderSuggest();
      }
      if (this.suggestAbort === controller) this.suggestAbort = null;
    }
  }

  /** 回车 / 搜索按钮：收起下拉框，进入正式数据加载。 */
  private commitSearch(): void {
    const query = this.query.trim();
    if (this.suggestTimer) {
      window.clearTimeout(this.suggestTimer);
      this.suggestTimer = undefined;
    }
    const reusable =
      this.suggestReady
      && this.suggestComplete
      && this.suggestQuery === query
      && this.suggestSource === this.scopeSignature();
    this.suggestOpen = false;
    this.suggestLoading = false;
    this.resetResultWindow();
    if (!query) {
      this.suggestToken += 1;
      this.suggestAbort?.abort();
      this.suggestAbort = null;
      this.sourceResults = [];
      this.searching = false;
      this.loadingDone = 0;
      this.loadingTotal = 0;
      this.loadingCurrent = "";
      this.touchItems();
      this.renderSuggest();
      this.renderResults();
      return;
    }
    if (reusable) {
      this.suggestToken += 1;
      this.suggestAbort?.abort();
      this.suggestAbort = null;
      this.searchAbort?.abort();
      this.searchAbort = null;
      const results = this.suggestResults;
      const token = ++this.searchToken;
      // A7 二期：复用建议结果也算一轮正式搜索 —— 回传请求在这里消费掉，避免漏到下一次。
      const publish = this.publishPending;
      const publishKind = this.publishKind;
      this.publishPending = false;
      this.publishKind = null;
      // 建议已就绪也先给出正式搜索的加载反馈，再切到结果。
      this.sourceResults = [];
      this.searching = true;
      this.loadingDone = 0;
      this.loadingTotal = 0;
      this.loadingCurrent = "";
      this.touchItems();
      this.renderSuggest();
      this.renderResults();
      window.setTimeout(() => {
        if (token !== this.searchToken) return;
        this.sourceResults = results;
        this.searching = false;
        this.touchItems();
        this.renderResults();
        if (publish) this.publishResults(query, results, publishKind);
      }, 150);
      return;
    }
    this.suggestToken += 1;
    this.suggestAbort?.abort();
    this.suggestAbort = null;
    this.renderSuggest();
    void this.runRemoteSearch();
  }

  private renderResults(): void {
    const started = perfNow();
    this.coverObserver?.disconnect();
    this.coverObserver = null;
    const box = this.root.querySelector<HTMLElement>("#rh-results");
    if (box) box.innerHTML = this.resultsHtml();
    this.observeResultsEnd();
    this.observeCovers();
    this.syncWarningTimer();
    const ms = perfNow() - started;
    if (ms >= 30) {
      perfWarn(`renderResults ${Math.round(ms)}ms`, {
        items: box ? this.resultItems().length : undefined,
        limit: this.resultLimit,
        sources: this.activeResults().length,
      });
    }
  }

  /** 数据换了一批（搜索 / 筛选 / 排序 / 刷新总览）：从第一段重新开始。 */
  private resetResultWindow(): void {
    this.resultLimit = RESULT_PAGE_SIZE;
  }

  /** 滚动到本段末尾时再渲染下一段；没有更多时哨兵不存在，观察器自然停止。 */
  private observeResultsEnd(): void {
    this.resultsObserver?.disconnect();
    this.resultsObserver = null;
    const sentinel = this.root.querySelector<HTMLElement>("#rh-more");
    if (!sentinel) return;
    const root = this.root.querySelector<HTMLElement>(".rh-main");
    this.resultsObserver = new IntersectionObserver(
      (entries) => {
        if (!entries.some((entry) => entry.isIntersecting)) return;
        this.maybeAutoMore();
      },
      { root, rootMargin: "320px 0px" },
    );
    this.resultsObserver.observe(sentinel);
  }

  /** 封面进入视口附近才写 src；避免一次解码几百张原图。 */
  private observeCovers(): void {
    this.coverObserver?.disconnect();
    this.coverObserver = null;
    const images = Array.from(this.root.querySelectorAll<HTMLImageElement>("img[data-src]"));
    if (images.length === 0) return;
    if (typeof IntersectionObserver === "undefined") {
      for (const img of images) this.loadCover(img);
      return;
    }
    const root = this.root.querySelector<HTMLElement>(".rh-main");
    this.coverObserver = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          const img = entry.target as HTMLImageElement;
          this.coverObserver?.unobserve(img);
          this.loadCover(img);
        }
      },
      { root, rootMargin: "300px 0px" },
    );
    for (const img of images) this.coverObserver.observe(img);
  }

  private loadCover(img: HTMLImageElement): void {
    const src = img.dataset.src;
    if (!src) return;
    delete img.dataset.src;
    img.src = src;
  }

  /**
   * 自动补页的唯一入口：必须由真实滚动“上膛”，并带 350ms 冷却。
   * 来源很多时，哨兵常驻视口不会再把所有来源一次性拉完。
   */
  private maybeAutoMore(): void {
    if (!this.scrollArmed || this.moreLoading) return;
    if (Date.now() - this.lastAutoMoreAt < 350) return;
    const sentinel = this.root.querySelector<HTMLElement>("#rh-more");
    if (!sentinel) return;
    const box = this.root.querySelector<HTMLElement>(".rh-main");
    if (!box) return;
    const sentinelRect = sentinel.getBoundingClientRect();
    const boxRect = box.getBoundingClientRect();
    if (sentinelRect.top > boxRect.bottom + 320 || sentinelRect.bottom < boxRect.top) return;
    this.scrollArmed = false;
    this.lastAutoMoreAt = Date.now();
    void this.loadMoreResults();
  }

  /** 来源清单 / 设置变化后让下一次总览刷新绕过内存缓存。 */
  private invalidateOverview(): void {
    this.exploreSignature = "";
    this.exploreLoadedAt = 0;
  }

  /** 来源版本 + 设置签名；任一项变化就让该来源的缓存失效。 */
  private overviewSourceSig(source: SourceRecord): string {
    return `${source.version}:${JSON.stringify(source.manifest.settings ?? {})}`;
  }

  private overviewCacheKey(sourceId: string): string {
    return `${OVERVIEW_CACHE_PREFIX}${sourceId}`;
  }

  /** 只留卡片需要的字段，避免单个 KV 值超过宿主 256 KiB 上限。 */
  private cacheWorks(works: Work[]): Work[] {
    return works.slice(0, OVERVIEW_CACHE_WORKS).map((work) => ({
      ref: work.ref,
      title: work.title,
      ...(work.author ? { author: work.author } : {}),
      ...(work.cover ? { cover: work.cover } : {}),
      ...(work.description
        ? {
            description: work.description
              .replace(/<[^>]*>/g, " ")
              .replace(/\s+/g, " ")
              .trim()
              .slice(0, 200),
          }
        : {}),
      ...(work.tags?.length ? { tags: work.tags.slice(0, 6) } : {}),
      ...(work.status ? { status: work.status } : {}),
      ...(work.updatedAt ? { updatedAt: work.updatedAt } : {}),
    }));
  }

  /** 写一个来源的总览缓存；失败（无宿主 / 超限）只影响下次启动速度。 */
  private saveOverviewCache(result: SourceSearchResult): void {
    if (result.error) return;
    const record = this.state.sources.find((source) => source.id === result.sourceId);
    if (!record) return;
    const entry: OverviewCacheEntry = {
      id: record.id,
      name: record.name,
      sig: this.overviewSourceSig(record),
      at: Date.now(),
      works: this.cacheWorks(result.works),
      hasMore: result.hasMore === true,
    };
    this.overviewAt.set(record.id, entry.at);
    void this.host.storage.set(this.overviewCacheKey(record.id), entry).catch(() => undefined);
  }

  /** 启动时把宿主 KV 里的总览缓存读进内存；返回是否命中。 */
  private hydrateOverviewCache(): Promise<boolean> {
    if (!this.overviewCacheLoad) this.overviewCacheLoad = this.loadOverviewCache();
    return this.overviewCacheLoad;
  }

  private async loadOverviewCache(): Promise<boolean> {
    let keys: string[] = [];
    try {
      keys = (await this.host.storage.keys()).filter((key) => key.startsWith(OVERVIEW_CACHE_PREFIX));
    } catch {
      return false;
    }
    if (keys.length === 0) return false;
    const installed = new Map(this.state.sources.map((source) => [source.id, source]));
    const results: SourceSearchResult[] = [];
    let oldest = Number.POSITIVE_INFINITY;
    const loadOne = async (key: string): Promise<void> => {
      const sourceId = key.slice(OVERVIEW_CACHE_PREFIX.length);
      const record = installed.get(sourceId);
      // 已卸载来源的缓存不在这里删：宿主来源目录是异步读入的。
      if (!record) return;
      let entry: OverviewCacheEntry | undefined;
      try {
        entry = await this.host.storage.get<OverviewCacheEntry>(key);
      } catch {
        return;
      }
      if (!entry || !Array.isArray(entry.works)) return;
      if (entry.sig !== this.overviewSourceSig(record)) return;
      results.push({ sourceId, sourceName: record.name, works: entry.works });
      this.overviewSigs.set(sourceId, entry.sig);
      this.overviewAt.set(sourceId, entry.at ?? 0);
      // 缓存只存首页；旧条目没有 hasMore 字段时按「可能还有」处理，翻页失败再收敛。
      this.explorePages.set(sourceId, 1);
      if (entry.hasMore !== false) this.exploreMore.add(sourceId);
      oldest = Math.min(oldest, entry.at ?? 0);
    };
    // **限并发 8**（2026-10-31）：以前是 `keys.length` 个 `storage.get()` 一起打给宿主，
    // 几十个来源时 IPC 队列被塞满，反而把这几十毫秒拖成肉眼可见的空白期。
    for (let start = 0; start < keys.length; start += 8) {
      await Promise.all(keys.slice(start, start + 8).map((key) => loadOne(key)));
    }
    if (results.length === 0) return false;
    this.exploreResults = results;
    this.touchItems();
    this.exploreLoadedAt = Number.isFinite(oldest) ? oldest : 0;
    for (const result of results) {
      const record = installed.get(result.sourceId);
      if (!record) continue;
      for (const work of result.works) this.rememberRemoteWork(workKey(work), record, work);
    }
    return true;
  }

  /** 卸载来源时一并删掉它的总览缓存。 */
  private forgetOverviewSource(sourceId: string): void {
    this.overviewSigs.delete(sourceId);
    this.overviewAt.delete(sourceId);
    this.explorePages.delete(sourceId);
    this.exploreMore.delete(sourceId);
    this.exploreResults = this.exploreResults.filter((result) => result.sourceId !== sourceId);
    this.touchItems();
    void this.host.storage.remove(this.overviewCacheKey(sourceId)).catch(() => undefined);
  }

  /**
   * 安装 / 更新来源后立刻抓一次预览并写缓存；
   * 失败不阻断安装，留给总览刷新重试。
   */
  private async prefetchSourcePreview(record: SourceRecord): Promise<void> {
    if (!canBrowse(record)) return;
    if (this.previewPrefetching.has(record.id)) return;
    this.previewPrefetching.add(record.id);
    try {
      const [result] = await exploreSources([record], this.host);
      if (!result || result.error) return;
      const index = this.exploreResults.findIndex((item) => item.sourceId === result.sourceId);
      if (index >= 0) this.exploreResults[index] = result;
      else this.exploreResults.push(result);
      this.overviewSigs.set(result.sourceId, this.overviewSourceSig(record));
      this.explorePages.set(result.sourceId, 1);
      if (result.hasMore) this.exploreMore.add(result.sourceId);
      else this.exploreMore.delete(result.sourceId);
      for (const work of result.works) this.rememberRemoteWork(workKey(work), record, work);
      this.saveOverviewCache(result);
      this.touchItems();
      if (this.query.trim() === "" && this.view.kind === "overview") this.renderResults();
    } catch {
      // 预览失败留给总览刷新重试。
    } finally {
      this.previewPrefetching.delete(record.id);
    }
  }

  /**
   * 总览：先用宿主 KV 缓存立即渲染，再按批懒加载缺失 / 过期的来源。
   * 缓存完整且未过期时完全不联网；缺失来源才抓，卡片不会被清空。
   */
  private async refreshOverview(force = false): Promise<void> {
    // 左下角「更多筛选」选定来源时，只加载这些来源（含「空关键词浏览」的番剧源）。
    const wanted = this.filterSources;
    const kinds = this.filterKinds;
    const sources = this.state.sources.filter((source) => {
      if (!canBrowse(source)) return false;
      if (wanted.size > 0 && !wanted.has(source.id)) return false;
      return kinds.size === 0 || source.manifest.kind.some((kind) => kinds.has(kind));
    });
    const signature = sources
      .map((source) => `${source.id}@${source.version}:${JSON.stringify(source.manifest.settings ?? {})}`)
      .join("|") + `|f:${[...wanted].sort().join(",")}|k:${[...kinds].sort().join(",")}`;
    this.exploreBatchSize = wanted.size > 0 || kinds.size > 0 ? SCOPED_SOURCE_BATCH : OVERVIEW_SOURCE_BATCH;
    // 先把「加载中」画出来，再去读宿主 KV 缓存（2026-10-31 报障：
    // 长期后台打开时「加载数据不进加载页」）。
    // `hydrateOverviewCache()` 要 `storage.keys()` + 每个来源一次 `storage.get()`，
    // 来源多 / 长会话后就是几十次 IPC；以前它排在建加载态之前 ⇒ 这段时间界面毫无反馈。
    if (force || this.exploreResults.length === 0) {
      this.exploreLoading = true;
      this.loadingDone = 0;
      this.loadingCurrent = "";
      this.loadingTotal = 0;
      this.paintImmediate();
    }
    const cached = await this.hydrateOverviewCache();
    const token = ++this.exploreToken;
    this.exploreAbort?.abort();
    this.exploreAbort = null;

    if (sources.length === 0) {
      // 当前分类 / 来源组合下没有可浏览的来源：保留已抓到的卡片，交给筛选层隐藏。
      this.exploreQueue = [];
      this.exploreDone = true;
      this.exploreLoading = false;
      this.exploreSignature = signature;
      this.exploreLoadedAt = Date.now();
      this.resultLimit = RESULT_PAGE_SIZE;
      this.loadingDone = 0;
      this.loadingTotal = 0;
      this.loadingCurrent = "";
      this.touchItems();
      this.renderResults();
      return;
    }

    // 缓存里有、但已卸载的来源不再展示（缓存键等卸载时再删）。
    // 注意：按「已安装且可浏览」而非当前筛选范围，切换分类 / 来源时不丢已抓到的卡片。
    const installedIds = new Set(
      this.state.sources.filter(canBrowse).map((source) => source.id),
    );
    this.exploreResults = this.exploreResults.filter((result) => installedIds.has(result.sourceId));
    for (const id of [...this.overviewSigs.keys()]) {
      if (!installedIds.has(id)) {
        this.overviewSigs.delete(id);
        this.explorePages.delete(id);
        this.exploreMore.delete(id);
      }
    }
    this.touchItems();
    // 近期失败的来源先冷却：随机预览不去撞已知坏源；全都在冷却时才回退到全量。
    const now = Date.now();
    const active = sources.filter((source) => (this.sourceCooldown.get(source.id) ?? 0) <= now);
    const candidates = active.length > 0 ? active : sources;
    const missing = candidates.filter(
      (source) =>
        this.overviewSigs.get(source.id) !== this.overviewSourceSig(source)
        && !this.previewPrefetching.has(source.id),
    );
    // 有可复用缓存且未过期：直接展示上次的单源预览，不再联网。
    if (cached && this.exploreResults.length > 0 && Date.now() - this.exploreLoadedAt < OVERVIEW_CACHE_TTL) {
      this.exploreSignature = signature;
    }
    const fresh = signature === this.exploreSignature
      && Date.now() - this.exploreLoadedAt < OVERVIEW_CACHE_TTL;
    perfInfo("overview", {
      sources: sources.length,
      cached,
      missing: missing.length,
      fresh,
      force,
    });
    if (!force && fresh && this.exploreResults.length > 0) {
      this.exploreQueue = [];
      this.exploreDone = true;
      this.exploreLoading = false;
      this.loadingDone = 0;
      this.loadingTotal = 0;
      this.loadingCurrent = "";
      this.renderResults();
      return;
    }

    // 全部来源只随机预览一个来源：不再为了凑满 30 张而连续抓多个源。
    this.exploreQueue = shuffle(missing.length > 0 ? missing : candidates).slice(0, this.exploreBatchSize);
    this.exploreSignature = signature;
    if (!cached) this.resetResultWindow();
    this.loadingDone = 0;
    this.loadingCurrent = "";
    this.loadingTotal = this.exploreQueue.length;
    this.exploreLoading = true;
    this.renderResults();
    await this.loadNextOverviewBatch(token);
    if (token !== this.exploreToken) return;
    this.exploreQueue = [];
    this.exploreDone = true;
    this.exploreLoading = false;
    this.loadingDone = this.loadingTotal;
    this.exploreLoadedAt = Date.now();
    perfInfo("overview ready", {
      previewSources: this.exploreResults.length,
      results: this.exploreResults.reduce((sum, item) => sum + item.works.length, 0),
    });
    this.renderResults();
  }

  /**
   * 抓一个小批来源（正常只有 1 个）；同一来源的新结果原位替换旧结果。
   */
  private async loadNextOverviewBatch(token: number): Promise<void> {
    if (this.exploreBatchLoading || token !== this.exploreToken) return;
    this.exploreBatchLoading = true;
    try {
      const batch = this.exploreQueue.splice(0, this.exploreBatchSize);
      if (batch.length === 0) return;
      const controller = new AbortController();
      this.exploreAbort = controller;
      const started = perfNow();
      const names = batch.map((item) => item.name).join(" / ");
      perfInfo("batch start", { sources: names, queue: this.exploreQueue.length + batch.length });
      const results = await exploreSources(batch, this.host, {
        signal: controller.signal,
        onSourceDone: (result) => {
          this.loadingDone = Math.min(this.loadingDone + 1, this.loadingTotal || 1);
          this.loadingCurrent = result.sourceName;
          this.updateLoadingDom();
        },
      });
      if (token !== this.exploreToken) return;
      {
        const ms = perfNow() - started;
        const works = results.reduce((sum, item) => sum + item.works.length, 0);
        const detail = {
          sources: names,
          results: results.length,
          works,
          errors: results.filter((item) => item.error).length,
          ms: Math.round(ms),
        };
        if (ms >= 1200) perfWarn(`batch slow ${Math.round(ms)}ms`, detail);
        else perfDebug(`batch ${Math.round(ms)}ms`, detail);
      }
      for (const result of results) {
        const index = this.exploreResults.findIndex((item) => item.sourceId === result.sourceId);
        if (index >= 0) this.exploreResults[index] = result;
        else this.exploreResults.push(result);
        const record = batch.find((item) => item.id === result.sourceId);
        if (!record) continue;
        this.overviewSigs.set(result.sourceId, this.overviewSourceSig(record));
        if (!result.error) this.saveOverviewCache(result);
        // 每次（重新）抓首页都把游标归位：之前翻到的第 N 页作废。
        if (!result.error) {
          this.explorePages.set(result.sourceId, 1);
          if (result.hasMore) this.exploreMore.add(result.sourceId);
          else this.exploreMore.delete(result.sourceId);
        }
        this.noteSourceHealth(result.sourceId, result.error);
        for (const work of result.works) {
          this.rememberRemoteWork(workKey(work), record, work);
        }
      }
      this.touchItems();
    } finally {
      this.exploreBatchLoading = false;
    }
  }

  /**
   * 「加载更多」：本地窗口还有数据就先放大窗口；展示完再向各来源取下一页。
   * 联网期间 `moreLoading` 为真，哨兵显示进度并阻止重复触发。
   */
  private async loadMoreResults(): Promise<void> {
    if (this.moreLoading) return;
    if (this.resultLimit < this.resultItems().length) {
      this.resultLimit += RESULT_PAGE_SIZE;
      this.renderResults();
      return;
    }
    const candidates = this.pagedCandidates();
    if (candidates.length === 0) {
      this.renderResults();
      return;
    }
    const searching = this.query.trim() !== "";
    const pages = searching ? this.searchPages : this.explorePages;
    const more = searching ? this.searchMore : this.exploreMore;
    const target = searching ? this.sourceResults : this.exploreResults;
    this.moreAbort?.abort();
    const controller = new AbortController();
    this.moreAbort = controller;
    this.moreLoading = true;
    this.loadingDone = 0;
    this.loadingTotal = candidates.length;
    this.loadingCurrent = "";
    this.renderResults();
    // 总览的来源是分批抓进来的，各来源的下一页页码不一定相同：按页码分组请求。
    const groups = new Map<number, SourceRecord[]>();
    for (const record of candidates) {
      const page = (pages.get(record.id) ?? 1) + 1;
      const group = groups.get(page);
      if (group) group.push(record);
      else groups.set(page, [record]);
    }
    const onSourceDone = (result: SourceSearchResult, done: number, total: number): void => {
      this.loadingDone = done;
      this.loadingTotal = total;
      this.loadingCurrent = result.sourceName;
      this.updateLoadingDom();
    };
    try {
      for (const [page, records] of [...groups.entries()].sort((a, b) => a[0] - b[0])) {
        if (controller.signal.aborted) break;
        const results = searching
          ? await searchSources(records, this.query.trim(), this.host, {
              page,
              signal: controller.signal,
              onSourceDone,
            })
          : await exploreSources(records, this.host, {
              page,
              signal: controller.signal,
              onSourceDone,
            });
        if (controller.signal.aborted) break;
        this.appendPagedResults(target, results, page, pages, more);
        this.touchItems();
        this.renderResults();
      }
    } finally {
      if (this.moreAbort === controller) this.moreAbort = null;
      this.moreLoading = false;
    }
    this.resultLimit += RESULT_PAGE_SIZE;
    this.touchItems();
    this.renderResults();
  }

  /** 「加载更多」要联网的来源：与当前视图同范围、抓过首页且还有下一页。 */
  private pagedCandidates(): SourceRecord[] {
    const searching = this.query.trim() !== "";
    const pages = searching ? this.searchPages : this.explorePages;
    const more = searching ? this.searchMore : this.exploreMore;
    const scoped = searching
      ? this.sourceScope()
      : this.state.sources.filter((source) => {
          if (!canBrowse(source)) return false;
          if (this.filterSources.size > 0 && !this.filterSources.has(source.id)) return false;
          return this.filterKinds.size === 0
            || source.manifest.kind.some((kind) => this.filterKinds.has(kind));
        });
    return scoped.filter((source) => pages.has(source.id) && more.has(source.id));
  }

  /**
   * 把某一页追加进已有结果：同来源累加并按 workKey 去重。
   * 出错（含 abort）不动游标，下次还能重试同一页。
   */
  private appendPagedResults(
    target: SourceSearchResult[],
    results: SourceSearchResult[],
    page: number,
    pages: Map<string, number>,
    more: Set<string>,
  ): void {
    for (const result of results) {
      if (result.error) {
        more.delete(result.sourceId);
        continue;
      }
      const record = this.state.sources.find((source) => source.id === result.sourceId);
      if (!record) continue;
      const existing = target.find((item) => item.sourceId === result.sourceId);
      const seen = new Set(existing?.works.map((work) => workKey(work)) ?? []);
      const fresh = result.works.filter((work) => !seen.has(workKey(work)));
      if (existing) existing.works = [...existing.works, ...fresh];
      else target.push({ ...result });
      for (const work of fresh) this.rememberRemoteWork(workKey(work), record, work);
      pages.set(result.sourceId, page);
      // 声明还有下一页但这一页没有新作品（站点忽略 pg）时也当作到底，避免无限翻同一页。
      if (result.hasMore === true && fresh.length > 0) more.add(result.sourceId);
      else more.delete(result.sourceId);
    }
  }

  private async runRemoteSearch(): Promise<void> {
    const query = this.query.trim();
    // A7 二期：这次搜索要不要把作品回传聊天窗（消费掉，避免漏到下一轮手搜）。
    const publish = this.publishPending;
    const publishKind = this.publishKind;
    this.publishPending = false;
    this.publishKind = null;
    const token = ++this.searchToken;
    this.searchAbort?.abort();
    this.moreAbort?.abort();
    this.moreAbort = null;
    this.moreLoading = false;
    this.searchPages.clear();
    this.searchMore.clear();
    if (!query) {
      this.searchAbort = null;
      this.sourceResults = [];
      this.searching = false;
      this.touchItems();
      this.renderResults();
      return;
    }
    const sources = this.sourceScope();
    const controller = new AbortController();
    this.searchAbort = controller;
    this.searching = true;
    this.sourceResults = [];
    this.resetResultWindow();
    const started = perfNow();
    perfInfo("search start", { query, sources: sources.length });
    this.loadingDone = 0;
    this.loadingCurrent = "";
    this.loadingTotal = sources.filter(
      (source) => source.manifest.capabilities.includes("search"),
    ).length;
    this.touchItems();
    this.renderResults();

    try {
      const results = await searchSources(sources, query, this.host, {
        signal: controller.signal,
        onSourceDone: (result, done, total) => {
          this.loadingDone = done;
          this.loadingTotal = total;
          this.loadingCurrent = result.sourceName;
          this.updateLoadingDom();
        },
      });
      if (token !== this.searchToken || controller.signal.aborted) return;

      const records = new Map(sources.map((record) => [record.id, record]));
      for (const result of results) {
        const record = records.get(result.sourceId);
        if (!record) continue;
        for (const work of result.works) {
          this.rememberRemoteWork(workKey(work), record, work);
        }
        // 首页游标：只有成功的来源才允许「加载更多」继续翻页。
        if (result.error) continue;
        this.searchPages.set(result.sourceId, 1);
        if (result.hasMore) this.searchMore.add(result.sourceId);
      }
      this.sourceResults = results;
      this.searching = false;
      this.loadingDone = this.loadingTotal;
      this.touchItems();
      {
        const ms = perfNow() - started;
        const detail = {
          query,
          sources: results.length,
          works: results.reduce((sum, item) => sum + item.works.length, 0),
          errors: results.filter((item) => item.error).length,
          ms: Math.round(ms),
        };
        if (ms >= 2000) perfWarn(`search slow ${Math.round(ms)}ms`, detail);
        else perfDebug(`search ${Math.round(ms)}ms`, detail);
      }
      this.renderResults();
      if (publish) this.publishResults(query, results, publishKind);
    } finally {
      if (this.searchAbort === controller) this.searchAbort = null;
    }
  }

  private remoteChapterKey(workId: string, index: number): string {
    return `${workId}#${index}`;
  }

  /**
   * 章节图片地址（2026-10-29）。
   *
   * 两个来源侧的坑都在这里收口：
   *  · **逐页 headers**：`loadEp` 返回的 headers / `onImageLoad` 的 Referer 与清单头常常不同，
   *    只带清单头会被图片 CDN 判盗链（403 / 空图）。
   *  · **图片字节变换**：加密图源（漫蛙吧的 AES-CBC 等）把密文当 jpg 发，直连 `<img>` 只能
   *    得到坏图 —— 交给源自己的 `onResponse` 解密，再换成 blob URL 挂进阅读器。
   *  任意一页失败都只退回代理地址，不影响同一章的其他页。
   */
  private async resolveComicImageUrls(
    entry: { record: SourceRecord },
    source: ResourceSource,
    chapter: Chapter,
    pages: ComicPage[],
  ): Promise<string[]> {
    const urls = pages.map((page) =>
      proxyMediaUrl(page.url, entry.record.manifest, page.headers),
    );
    if (!source.imageData) return urls;
    const pending = pages
      .map((page, index) => ({ page, index }))
      .filter((item) => item.page.transform);
    if (pending.length === 0) return urls;
    let cursor = 0;
    const worker = async (): Promise<void> => {
      while (cursor < pending.length) {
        const current = pending[cursor++];
        try {
          const bytes = await source.imageData!(chapter, current.page);
          if (!bytes || bytes.length === 0) continue;
          const blobUrl = URL.createObjectURL(new Blob([new Uint8Array(bytes)]));
          this.readerImageBlobs.add(blobUrl);
          urls[current.index] = blobUrl;
        } catch (error) {
          perfWarn("comic image transform failed", {
            source: entry.record.id,
            chapter: chapter.id,
            url: current.page.url,
            error: error instanceof Error ? error.message : String(error),
          });
        }
      }
    };
    await Promise.all(Array.from({ length: Math.min(3, pending.length) }, () => worker()));
    return urls;
  }

  /** 按需加载单章内容；跨章翻页 / 目录跳转都会走这里，成功后按 key 缓存。 */
  private async loadRemoteReaderChapter(workId: string, index: number): Promise<ReaderChapter | null> {
    const key = this.remoteChapterKey(workId, index);
    const cached = this.remoteReaderChapters.get(key);
    if (cached) return cached;
    const entry = this.remoteWorks.get(workId);
    const chapter = this.remoteChapters.get(workId)?.[index];
    if (!entry || !chapter) return null;
    const kind = entry.work.ref.kind;
    if (kind !== "comic" && kind !== "novel") return null;

    try {
      const source = sourceFor(entry.record, this.host);
      let readerChapter: ReaderChapter;
      if (kind === "comic") {
        if (!source.pages) {
          throw new RspError("unsupported", `${entry.record.name} 没有 pages 规则`, entry.record.id);
        }
        const pages = await source.pages(chapter);
        const imageUrls = await this.resolveComicImageUrls(entry, source, chapter, pages);
        readerChapter = {
          id: chapter.id,
          title: chapter.title,
          pages: imageUrls.map((url, pageIndex) => ({
            id: `${chapter.id}-p${pageIndex + 1}`,
            kind: "image" as const,
            content: url,
          })),
        };
      } else {
        if (!source.content) {
          throw new RspError("unsupported", `${entry.record.name} 没有 content 规则`, entry.record.id);
        }
        const text = await source.content(chapter);
        let pages: ReaderChapter["pages"];
        if (text.format === "epub") {
          // content 给的是 EPUB 地址：下载 → 解析 spine → HTML 页（图片已内联）。
          const bytes = await fetchMediaBytes(text.content.trim(), entry.record.manifest);
          const book = parseEpub(bytes);
          pages = book.chapters.flatMap((section, sectionIndex) => {
            const id = `${chapter.id}-s${sectionIndex + 1}`;
            if (section.html && section.html.trim()) {
              return [{ id, kind: "html" as const, content: section.html, caption: section.title }];
            }
            return splitTextPages(id, section.text);
          });
        } else if (text.format === "html") {
          pages = splitTextPages(chapter.id, htmlToText(text.content));
        } else {
          pages = splitTextPages(chapter.id, text.content);
        }
        if (pages.length === 0) {
          pages.push({ id: `${chapter.id}-p1`, kind: "placeholder" as const, caption: chapter.title });
        }
        readerChapter = { id: chapter.id, title: chapter.title, pages };
      }
      this.remoteReaderChapters.set(key, readerChapter);
      return readerChapter;
    } catch (error) {
      // 不要静默：章节打不开时把来源 / 章节 / 原因留给控制台，
      // 否则界面上只剩一句「章节加载失败」，无法判断是上游反爬还是解析规则失配。
      perfWarn("chapter load failed", {
        source: entry.record.id,
        chapter: chapter.id,
        error: error instanceof Error ? error.message : String(error),
      });
      return null;
    }
  }

  private async openRemoteChapter(workId: string, index: number, pageIndex = 0): Promise<void> {
    const entry = this.remoteWorks.get(workId);
    if (!entry) return;
    const chapters = this.remoteChapters.get(workId) ?? (await this.loadRemoteChapters(workId));
    const chapter = chapters[index];
    if (!chapter) {
      this.notice = LANG.noChapters;
      this.noticeError = true;
      this.paint();
      return;
    }

    const kind = entry.work.ref.kind;
    if (kind !== "comic" && kind !== "novel") {
      const saved = this.state.progress[workId];
      const position = saved && saved.chapterId === chapter.id ? saved.positionSec ?? 0 : 0;
      await this.openVideoChapter(workId, index, position);
      return;
    }

    this.notice = LANG.loadingChapter;
    this.noticeError = false;
    this.view = { kind: "detail", workId };
    this.paint();

    try {
      const loaded = await this.loadRemoteReaderChapter(workId, index);
      if (!loaded) {
        throw new RspError("parse", LANG.chapterLoadFail, entry.record.id);
      }
      // 未加载的章节先挂占位；阅读器进入时再按需拉取，避免一次请求整本书。
      const readerChapters: ReaderChapter[] = chapters.map((item, chapterIndex) => {
        if (chapterIndex === index) return loaded;
        const cached = this.remoteReaderChapters.get(this.remoteChapterKey(workId, chapterIndex));
        return cached ?? {
          id: item.id,
          title: item.title,
          pages: [{
            id: `${item.id}-loading`,
            kind: "placeholder" as const,
            caption: LANG.reader.loadingChapter,
          }],
          lazy: true,
        };
      });

      this.pendingRemote = { workKey: workId, chapters: readerChapters, pageIndex };
      this.readerSession = {
        workKey: workId,
        chapters: chapters.map((item) => ({ id: item.id, title: item.title })),
        baseIndex: 0,
        chapterIndex: index,
        pageIndex,
      };
      this.state = saveProgress(this.state, workId, {
        chapterId: chapter.id,
        chapterIndex: index,
        chapterTitle: chapter.title,
        pageIndex,
      });
      this.view = { kind: "reader", workId, chapterIndex: index };
      this.notice = "";
      this.paint();
    } catch (error) {
      this.notice = `${LANG.chapterLoadFail}: ${errorText(error)}`;
      this.noticeError = true;
      this.paint();
    }
  }

  private openChapter(workId: string, index: number): void {
    const work = demoWorkById(workId);
    if (!work || index < 0 || index >= work.chapters.length) return;
    if (work.kind === "anime" || work.kind === "tv") {
      // 演示来源只有剧集结构，没有真实播放地址。
      this.notice = LANG.player.noSource;
      this.noticeError = false;
      this.paint();
      return;
    }
    const chapter = work.chapters[index];
    const saved = this.state.progress[workId];
    const pageIndex = saved && saved.chapterIndex === index ? saved.pageIndex ?? 0 : 0;
    this.state = saveProgress(this.state, workId, {
      chapterId: chapter.id,
      chapterIndex: index,
      chapterTitle: chapter.title,
      pageIndex,
    });
    this.view = { kind: "reader", workId, chapterIndex: index };
    this.notice = "";
    this.paint();
  }

  /** 从聚合仓库安装来源：自建条目用内嵌规则，上游条目按 `sourceUrl` 读取规则 / 脚本。 */
  private async installRegistryEntry(id: string): Promise<void> {
    const entry = this.state.registry.find((item) => item.id === id);
    if (!entry) return;
    if (!entry.source && !entry.sourceUrl) {
      this.notice = LANG.registryNoRule;
      this.noticeError = true;
      this.paint();
      return;
    }
    this.notice = LANG.importing;
    this.noticeError = false;
    this.paint();
    try {
      let record: SourceRecord;
      if (entry.source) {
        record = this.recordFromEmbedded(entry);
      } else {
        record = await this.recordFromSourceUrl(entry);
      }
      this.state = addSource(this.state, record);
      persistHostSource(record);
      // 安装完成即抓一次预览并落缓存，总览首屏不用等这个来源。
      await this.prefetchSourcePreview(record);
      this.sourceSwitchCache.clear();
      this.notice = LANG.installOk;
      this.noticeError = false;
    } catch (error) {
      this.notice = `${LANG.importFail}: ${error instanceof RspError ? error.message : String(error)}`;
      this.noticeError = true;
    }
    this.paint();
    this.invalidateOverview();
    void this.refreshOverview(true);
  }

  /** 一键安装：串行处理尚未安装的仓库条目，单个失败不阻断其余。 */
  private async installAllRegistryEntries(): Promise<void> {
    if (this.batchInstalling) return;
    const pending = this.state.registry.filter(
      (entry) =>
        Boolean(entry.source || entry.sourceUrl) &&
        !this.state.sources.some((source) => source.id === entry.id),
    );
    if (pending.length === 0) {
      this.notice = LANG.installAllNone;
      this.noticeError = false;
      this.paint();
      return;
    }
    this.batchInstalling = true;
    this.notice = `${LANG.installingAll} 0/${pending.length}`;
    this.noticeError = false;
    this.paint();
    let ok = 0;
    const failed: string[] = [];
    for (let i = 0; i < pending.length; i += 1) {
      const entry = pending[i];
      try {
        const record = entry.source
          ? this.recordFromEmbedded(entry)
          : await this.recordFromSourceUrl(entry);
        this.state = addSource(this.state, record);
        persistHostSource(record);
        await this.prefetchSourcePreview(record);
        ok += 1;
      } catch (error) {
        failed.push(`${entry.name}：${error instanceof RspError ? error.message : String(error)}`);
      }
      this.notice = `${LANG.installingAll} ${i + 1}/${pending.length}`;
      this.paint();
    }
    this.batchInstalling = false;
    this.sourceSwitchCache.clear();
    this.notice = failed.length
      ? `${LANG.installAllDone}: ${ok}/${pending.length}；${failed.length} ${LANG.failed}：${failed.slice(0, 3).join("；")}`
      : `${LANG.installAllDone}: ${ok}`;
    this.noticeError = failed.length > 0;
    this.paint();
    this.invalidateOverview();
    void this.refreshOverview(true);
  }

  /** 更新单个仓库来源：重新读取规则 / 脚本，保留用户设置与安装时间。 */
  private async updateRegistryEntry(id: string): Promise<void> {
    const entry = this.state.registry.find((item) => item.id === id);
    const previous = this.state.sources.find((source) => source.id === id);
    if (!entry || !previous) return;
    if (!entry.source && !entry.sourceUrl) {
      this.notice = LANG.registryNoRule;
      this.noticeError = true;
      this.paint();
      return;
    }
    this.notice = LANG.updating;
    this.noticeError = false;
    this.paint();
    try {
      const record = entry.source
        ? this.recordFromEmbedded(entry)
        : await this.recordFromSourceUrl(entry);
      const merged = keepSourceUserData(previous, record);
      this.state = addSource(this.state, merged);
      persistHostSource(merged);
      await this.prefetchSourcePreview(merged);
      this.sourceSwitchCache.clear();
      this.notice = LANG.updateOk;
      this.noticeError = false;
    } catch (error) {
      this.notice = `${LANG.updateFail}: ${error instanceof RspError ? error.message : String(error)}`;
      this.noticeError = true;
    }
    this.paint();
    this.invalidateOverview();
    void this.refreshOverview(true);
  }

  /** 全部更新：串行处理所有可更新条目，单个失败不阻断其余。 */
  private async updateAllRegistryEntries(): Promise<void> {
    if (this.batchUpdating) return;
    const pending = this.updatableRegistryEntries();
    if (pending.length === 0) {
      this.notice = LANG.updateAllNone;
      this.noticeError = false;
      this.paint();
      return;
    }
    this.batchUpdating = true;
    this.notice = `${LANG.updatingAll} 0/${pending.length}`;
    this.noticeError = false;
    this.paint();
    let ok = 0;
    const failed: string[] = [];
    for (let i = 0; i < pending.length; i += 1) {
      const { entry, installed } = pending[i];
      try {
        const record = entry.source
          ? this.recordFromEmbedded(entry)
          : await this.recordFromSourceUrl(entry);
        const merged = keepSourceUserData(installed, record);
        this.state = addSource(this.state, merged);
        persistHostSource(merged);
        await this.prefetchSourcePreview(merged);
        ok += 1;
      } catch {
        failed.push(entry.name);
      }
      this.notice = `${LANG.updatingAll} ${i + 1}/${pending.length}`;
      this.paint();
    }
    this.batchUpdating = false;
    this.sourceSwitchCache.clear();
    this.notice = failed.length
      ? `${LANG.updateAllDone}: ${ok}/${pending.length}；${failed.length} ${LANG.failed}：${failed.slice(0, 5).join("、")}`
      : `${LANG.updateAllDone}: ${ok}`;
    this.noticeError = failed.length > 0;
    this.paint();
    this.invalidateOverview();
    void this.refreshOverview(true);
  }

  /** 一键移除：只删仓库里已安装的来源，保留用户自行导入的其它来源。 */
  private removeAllRegistrySources(): void {
    const ids = new Set(this.state.registry.map((entry) => entry.id));
    const targets = this.state.sources.filter((source) => ids.has(source.id));
    if (targets.length === 0) {
      this.notice = LANG.removeAllNone;
      this.noticeError = false;
      this.paint();
      return;
    }
    if (typeof confirm === "function" && !confirm(LANG.confirmRemoveAll)) return;
    for (const source of targets) {
      this.state = removeSource(this.state, source.id);
      deleteHostSource(source.id);
      this.forgetOverviewSource(source.id);
      this.filterSources.delete(source.id);
      this.filterDraftSources.delete(source.id);
    }
    this.sourceSwitchCache.clear();
    this.notice = `${LANG.removeAllDone}: ${targets.length}`;
    this.noticeError = false;
    this.invalidateOverview();
    this.paint();
    void this.refreshOverview(true);
  }

  private recordFromEmbedded(entry: RegistryEntry): SourceRecord {
    const embedded = entry.source;
    if (!embedded) throw new RspError("parse", LANG.registryNoRule, entry.id);
    // 与上游条目一致：仓库条目的 id / 名称 / 版本是安装与更新比对的唯一口径。
    const manifest = validateSourceManifest({
      ...embedded.manifest,
      id: entry.id,
      name: entry.name,
      version: entry.version,
      kind: entry.kind.length > 0 ? entry.kind : embedded.manifest.kind,
    });
    if (manifest.engine !== "declarative") {
      throw new RspError("unsupported", "当前只支持 declarative 来源", manifest.id);
    }
    return {
      id: manifest.id,
      name: manifest.name,
      version: manifest.version,
      kind: manifest.kind,
      manifest,
      rule: embedded.rule,
      warnings: [],
      importedAt: Date.now(),
    };
  }

  /** 上游条目：读取 `sourceUrl`，漫画规则转声明式规则，漫画 / 视频源存脚本正文。 */
  private async recordFromSourceUrl(entry: RegistryEntry): Promise<SourceRecord> {
    const url = entry.sourceUrl;
    if (!url) throw new RspError("parse", LANG.registryNoRule, entry.id);
    const response = await this.host.net.request({
      url,
      method: "GET",
      timeoutMs: 20_000,
      maxBytes: 2 * 1024 * 1024,
    });
    if (response.status >= 400) {
      throw new RspError("network", `HTTP ${response.status}: ${url}`, entry.id);
    }
    if (entry.engine === "declarative") {
      let parsed: unknown;
      try {
        parsed = JSON.parse(response.body.replace(/^\uFEFF/, ""));
      } catch {
        throw new RspError("parse", "上游规则不是合法 JSON", entry.id);
      }
      const converted = convertMangaRule(parsed);
      const manifest = validateSourceManifest({
        ...converted.manifest,
        id: entry.id,
        name: entry.name,
        version: entry.version,
        kind: entry.kind.length > 0 ? entry.kind : converted.manifest.kind,
      });
      return {
        id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        kind: manifest.kind,
        manifest,
        rule: converted.rule,
        warnings: converted.warnings,
        importedAt: Date.now(),
      };
    }
    const script = response.body;
    if (!script.trim()) throw new RspError("parse", "上游脚本为空", entry.id);
    const isVideo = entry.upstream === "video";
    if (isVideo) {
      // 上游仓库里有一部分是 BaseCode1 / 加密源，装了也只会在首次搜索时报「不支持」
      // ⇒ 读到正文就判形态，导入阶段直接拒掉，失败原因给用户看得懂的话。
      const reason = videoUnsupportedReason(script);
      if (reason) throw new RspError("unsupported", LANG.videoUnsupported[reason], entry.id);
    }
    const manifest = validateSourceManifest({
      rsp: 1,
      id: entry.id,
      name: entry.name,
      version: entry.version,
      kind: entry.kind.length > 0 ? entry.kind : [isVideo ? "anime" : "comic"],
      engine: "js",
      entry: "source.js",
      baseUrl: guessSourceBaseUrl(script),
      capabilities: isVideo
        ? videoHasExplore(script)
          ? ["search", "explore", "chapters", "play"]
          : ["search", "chapters", "play"]
        : scriptHasExplore(script)
          ? ["search", "explore", "chapters", "pages"]
          : ["search", "chapters", "pages"],
    });
    return {
      id: manifest.id,
      name: manifest.name,
      version: manifest.version,
      kind: manifest.kind,
      manifest,
      rule: {},
      script,
      warnings: [],
      importedAt: Date.now(),
    };
  }

  private async importFromUrl(): Promise<void> {
    const pasted = this.importJson.trim();
    const url = this.importUrl.trim();
    if ((!pasted && !url) || this.importing) return;
    this.importing = true;
    this.notice = "";
    this.noticeError = false;
    this.paint();
    try {
      let parsed: unknown;
      if (pasted) {
        try {
          parsed = JSON.parse(pasted.replace(/^\uFEFF/, ""));
        } catch {
          throw new RspError("parse", "粘贴内容不是合法 JSON", undefined);
        }
      } else {
        const response = await this.host.net.request({ url, method: "GET", timeoutMs: 15_000 });
        if (response.status >= 400) {
          throw new RspError("network", `HTTP ${response.status}: ${url}`, undefined);
        }
        try {
          parsed = JSON.parse(response.body.replace(/^\uFEFF/, ""));
        } catch {
          throw new RspError("parse", "响应不是合法 JSON", undefined);
        }
      }
      this.notice = this.applyImported(parsed);
      this.noticeError = false;
      this.importJson = "";
      this.view = { kind: "sources" };
    } catch (error) {
      this.notice = `${LANG.importFail}: ${error instanceof RspError ? error.message : String(error)}`;
      this.noticeError = true;
    } finally {
      this.importing = false;
      this.paint();
      this.invalidateOverview();
      void this.refreshOverview(true);
    }
  }

  /** 把 URL 拿到的 JSON 落到本地状态；返回给用户看的结果文案。 */
  private applyImported(parsed: unknown): string {
    if (isRecord(parsed) && isRecord(parsed.manifest) && isRecord(parsed.rule)) {
      const manifest = validateSourceManifest(parsed.manifest);
      if (manifest.engine !== "declarative") {
        throw new RspError("unsupported", "当前只支持 declarative 来源", manifest.id);
      }
      const record: SourceRecord = {
        id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        kind: manifest.kind,
        manifest,
        rule: parsed.rule as unknown as DeclarativeRule,
        warnings: [],
        importedAt: Date.now(),
      };
      this.state = addSource(this.state, record);
      persistHostSource(record);
      void this.prefetchSourcePreview(record);
      this.sourceSwitchCache.clear();
      return LANG.importOk;
    }
    if (
      typeof parsed === "object"
      && parsed !== null
      && "rsp" in parsed
      && "entries" in parsed
    ) {
      const bundle = parseRegistry(parsed);
      this.state = mergeRegistryEntries(this.state, bundle.entries);
      this.sourceSwitchCache.clear();
      return `${LANG.registryImported}: ${bundle.entries.length}`;
    }
    const converted = convertMangaRule(parsed);
    const record: SourceRecord = {
      id: converted.manifest.id,
      name: converted.manifest.name,
      version: converted.manifest.version,
      kind: converted.manifest.kind,
      manifest: converted.manifest as SourceManifest,
      rule: converted.rule as DeclarativeRule,
      warnings: converted.warnings,
      importedAt: Date.now(),
    };
    this.state = addSource(this.state, record);
    persistHostSource(record);
    void this.prefetchSourcePreview(record);
    this.sourceSwitchCache.clear();
    return converted.warnings.length
      ? `${LANG.importOk}；${LANG.warnings}: ${converted.warnings.join("；")}`
      : LANG.importOk;
  }
}

export function demoWorks(): DemoWork[] {
  return DEMO_WORKS;
}
