// 插件运行时：宿主 API v3 选择 + 已导入来源的搜索编排。
//
// Lunac 注入 v3 时用真实宿主；否则退回 mock（浏览器 fetch），
// 保证「导入来源 → 搜索」在宿主未就绪时也能跑通并暴露可读错误。

import {
  RspError,
  createDeclarativeSource,
  createJsSource,
  createMockHost,
  createNetClient,
  createComicSource,
  createVideoSource,
  hasHostV3,
  looksLikeComic,
  looksLikeVideo,
  requireHostV3,
  type LunacHostV3,
  type NetApi,
  type ResourceSource,
  type Work,
} from "@resource-hub/source-sdk";

import type { SourceRecord } from "./store.js";

export interface SourceSearchResult {
  sourceId: string;
  sourceName: string;
  works: Work[];
  error?: string;
  /** 来源是否还有下一页（`Paged.next !== null`）；缺省按「没有更多」处理。 */
  hasMore?: boolean;
}

export interface SearchOptions {
  /** 新搜索 / 离开页面时取消旧搜索。 */
  signal?: AbortSignal;
  /** 单次请求超时；缺省 8s。 */
  timeoutMs?: number;
  /** 页码，缺省 1；>1 即「加载更多」的追加请求。 */
  page?: number;
  /** 每完成一个来源回调一次（用于显示加载进度），不影响结果。 */
  onSourceDone?: (result: SourceSearchResult, done: number, total: number) => void;
}

function perfNowMs(): number {
  return typeof performance !== "undefined" ? performance.now() : Date.now();
}

/** 来源并发上限：几十个来源同时跑 QuickJS 会把 CPU / 内存打满。 */
const SOURCE_CONCURRENCY = 4;

/** 按固定并发跑一组来源，结果顺序与入参一致。 */
async function mapLimited<T, R>(
  items: T[],
  limit: number,
  worker: (item: T) => Promise<R>,
): Promise<R[]> {
  const results = new Array<R>(items.length);
  let next = 0;
  const runners = Array.from({ length: Math.min(limit, items.length) }, async () => {
    while (true) {
      const index = next;
      next += 1;
      if (index >= items.length) return;
      results[index] = await worker(items[index]);
    }
  });
  await Promise.all(runners);
  return results;
}

/** 来源级耗时日志：慢来源（≥1s）用 warn，其余走 debug 便于对照时间线。 */
function logSourceRun(
  sourceId: string,
  method: "search" | "explore",
  started: number,
  works: number,
  error?: string,
): void {
  const ms = Math.round(perfNowMs() - started);
  const detail = { works, error };
  if (ms >= 1000) console.warn(`[resource-hub] source ${sourceId} ${method} ${ms}ms`, detail);
  else console.debug(`[resource-hub] source ${sourceId} ${method} ${ms}ms`, detail);
}

export function createSourceHost(): LunacHostV3 {
  return hasHostV3() ? requireHostV3() : createMockHost();
}

/**
 * 每个来源一份网络客户端（缓存 / 限速隔离）。
 * 宿主对象变化时整张表随之失效（WeakMap），不会把旧宿主的缓存带过来。
 */
const netClients = new WeakMap<LunacHostV3, Map<string, NetApi>>();

function netFor(record: SourceRecord, host: LunacHostV3): NetApi {
  let clients = netClients.get(host);
  if (!clients) {
    clients = new Map();
    netClients.set(host, clients);
  }
  const key = `${record.id}@${record.version}`;
  const existing = clients.get(key);
  if (existing) return existing;
  const client = createNetClient(host.net, {
    cacheTtlMs: 30_000,
    cacheMaxEntries: 64,
    rateLimit: record.manifest.rateLimit,
    timeoutMs: 8_000,
  });
  clients.set(key, client);
  return client;
}

/**
 * 视频类来源能否出现在总览 / 分类里。
 *
 * Kazumi 规则只有 `search` + `chapters`，没有分类接口；只要它声明了搜索规则，
 * 就用「空关键词搜索」当浏览入口（很多站点会返回全部 / 近期更新）。
 * 2026-10-21：此前总览只认 `explore` 能力，导致「分类 → 视频」整块空白。
 */
export function canBrowse(record: SourceRecord): boolean {
  if (record.manifest.capabilities.includes("explore")) return true;
  if (!record.manifest.kind.includes("anime") && !record.manifest.kind.includes("tv")) return false;
  // 只对 HTML 搜索规则做「空关键词浏览」：JSON API 规则通常必须带关键词，
  // 空查询只会换来一次必然失败的请求。
  return !!record.rule.search?.list?.xpath;
}

export function sourceFor(record: SourceRecord, host: LunacHostV3): ResourceSource {
  if (record.manifest.engine === "js") {
    if (!record.script) {
      throw new RspError("unsupported", "JS 源缺少脚本正文", record.id);
    }
    if (looksLikeComic(record.script)) {
      return createComicSource({ manifest: record.manifest, script: record.script }, { host });
    }
    if (looksLikeVideo(record.script)) {
      return createVideoSource({ manifest: record.manifest, script: record.script }, { host });
    }
    return createJsSource({ manifest: record.manifest, script: record.script }, { host });
  }
  // 2026-10-21：旧版转换器只给 Kazumi 番剧规则产出 search/chapters，清单里没有 play。
  // 读取时补一条嗅探规则（剧集页 → 隐藏 WebView 拦截 m3u8/mp4），已装来源无需重装。
  const needsSniff =
    (record.manifest.kind.includes("anime") || record.manifest.kind.includes("tv")) &&
    !!record.rule.chapters &&
    !record.rule.play &&
    !record.rule.sniff;
  const browse = canBrowse(record);
  const source = createDeclarativeSource(
    {
      manifest: record.manifest,
      rule: needsSniff ? { ...record.rule, sniff: { url: "@chapterUrl" } } : record.rule,
    },
    { host, net: netFor(record, host) },
  );
  // 没有 explore 规则的番剧源：空关键词搜索当作「全部 / 近期更新」。
  if (browse && !record.manifest.capabilities.includes("explore") && source.search) {
    const search = source.search.bind(source);
    source.explore = (_section = "", page = 1, options) => search("", page, options);
  }
  return source;
}

export function errorText(error: unknown): string {
  if (error instanceof RspError) return `${error.code}: ${error.message}`;
  return error instanceof Error ? error.message : String(error);
}

/** 并行搜索所有已导入且声明 search 能力的来源；单个来源失败不影响其它来源。 */
export async function searchSources(
  sources: SourceRecord[],
  query: string,
  host: LunacHostV3,
  options: SearchOptions = {},
): Promise<SourceSearchResult[]> {
  const runnable = sources.filter((source) => source.manifest.capabilities.includes("search"));
  const total = runnable.length;
  let done = 0;
  return mapLimited(
    runnable,
    SOURCE_CONCURRENCY,
    async (record): Promise<SourceSearchResult> => {
      // 已取消：不再启动新的来源。JS 源的 engine.eval 无法中断（30s 看门狗），
      // 但这里提前返回能保证「换源」这类带硬上限的调用按时收尾。
      if (options.signal?.aborted) {
        return { sourceId: record.id, sourceName: record.name, works: [], error: "aborted", hasMore: false };
      }
      let result: SourceSearchResult;
      const started = perfNowMs();
      try {
        const source = sourceFor(record, host);
        if (!source.search) {
          result = {
            sourceId: record.id,
            sourceName: record.name,
            works: [],
            error: "unsupported: search",
          };
        } else {
          const page = await source.search(query, options.page ?? 1, options);
          result = {
            sourceId: record.id,
            sourceName: record.name,
            works: page.items,
            hasMore: page.next !== null,
          };
        }
      } catch (error) {
        result = {
          sourceId: record.id,
          sourceName: record.name,
          works: [],
          error: errorText(error),
        };
      }
      done += 1;
      logSourceRun(record.id, "search", started, result.works.length, result.error);
      options.onSourceDone?.(result, done, total);
      return result;
    },
  );
}

/** 并行拉取各来源的总览（explore）；没有可浏览能力的来源直接跳过。 */
export async function exploreSources(
  sources: SourceRecord[],
  host: LunacHostV3,
  options: SearchOptions = {},
): Promise<SourceSearchResult[]> {
  const runnable = sources.filter(canBrowse);
  const total = runnable.length;
  let done = 0;
  return mapLimited(
    runnable,
    SOURCE_CONCURRENCY,
    async (record): Promise<SourceSearchResult> => {
      if (options.signal?.aborted) {
        return { sourceId: record.id, sourceName: record.name, works: [], error: "aborted", hasMore: false };
      }
      let result: SourceSearchResult;
      const started = perfNowMs();
      try {
        const source = sourceFor(record, host);
        if (!source.explore) {
          result = {
            sourceId: record.id,
            sourceName: record.name,
            works: [],
            error: "unsupported: explore",
          };
        } else {
          const page = await source.explore(undefined, options.page ?? 1, options);
          result = {
            sourceId: record.id,
            sourceName: record.name,
            works: page.items,
            hasMore: page.next !== null,
          };
        }
      } catch (error) {
        result = {
          sourceId: record.id,
          sourceName: record.name,
          works: [],
          error: errorText(error),
        };
      }
      done += 1;
      logSourceRun(record.id, "explore", started, result.works.length, result.error);
      options.onSourceDone?.(result, done, total);
      return result;
    },
  );
}
