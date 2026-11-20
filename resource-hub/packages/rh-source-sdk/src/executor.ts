// 声明式来源执行器：规则 + 宿主 API → RSP 的 ResourceSource。
//
// 只依赖 `host.ts` 的 net 能力；HTML 解析通过 `parseHtml` 注入，
// 浏览器默认用 DOMParser，Node / 测试可传入自己的实现或只用 JSON 规则。

import {
  RspError,
  type Chapter,
  type ChaptersRule,
  type CmsRule,
  type ComicPage,
  type DeclarativeRule,
  type ListRule,
  type Paged,
  type Playback,
  type ResourceSource,
  type SourceCallOptions,
  type SourceManifest,
  type TextChapter,
  type Work,
  type WorkRef,
} from "./contract.js";
import {
  buildRequestUrl,
  extractHtmlRows,
  extractJsonRows,
  interpolate,
  selectElements,
  type RuleVars,
} from "./evaluate.js";
import type { LunacHostV3, NetApi } from "./host.js";

export interface DeclarativeSourceDeps {
  host: LunacHostV3;
  /** 覆盖 `host.net`（缓存 / 限速 / 取消）；缺省直接用宿主网络。 */
  net?: NetApi;
  /** HTML → DOM；默认 DOMParser（浏览器）。 */
  parseHtml?(html: string): ParentNode;
}

export interface DeclarativeSourceInput {
  manifest: SourceManifest;
  rule: DeclarativeRule;
}

function defaultParseHtml(html: string): ParentNode {
  if (typeof DOMParser === "undefined") {
    throw new RspError("unsupported", "当前运行时没有 DOMParser，无法解析 HTML 规则", undefined);
  }
  return new DOMParser().parseFromString(html, "text/html");
}

function resolveUrl(baseUrl: string | undefined, value: string): string {
  if (!value) return "";
  if (/^https?:\/\//i.test(value) || !baseUrl) return value;
  try {
    // 交给 URL 处理站内绝对路径（/x）与子目录 baseUrl，避免手工拼错。
    return new URL(value, baseUrl).toString();
  } catch {
    return value;
  }
}

function interpolateMap(
  values: Record<string, string> | undefined,
  vars: RuleVars,
): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(values ?? {})) {
    out[key] = interpolate(value, vars);
  }
  return out;
}

function statusError(status: number, url: string, sourceId: string): RspError {
  if (status === 401) return new RspError("auth", `需要登录：${url}`, sourceId);
  if (status === 403) return new RspError("blocked", `来源拒绝访问：${url}`, sourceId);
  return new RspError("network", `HTTP ${status}：${url}`, sourceId);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function asText(value: unknown): string {
  if (typeof value === "string") return value.trim();
  if (typeof value === "number") return String(value);
  return "";
}

function stripTags(value: string): string {
  return value
    .replace(/<[^>]*>/g, " ")
    .replace(/&nbsp;/gi, " ")
    .replace(/\s+/g, " ")
    .trim();
}

/** `vod_play_from` / `vod_play_url` 的多线路分隔符。 */
function splitLines(value: unknown): string[] {
  return asText(value)
    .split("$$$")
    .map((line) => line.trim());
}

/** 地址本身是不是媒体直链（决定「直接用」还是「交给 WebView 嗅探」）。
 *  2026-10-28 由私有函数改为导出：视频 JS 兼容层（`video-source.ts`）的嗅探回退
 *  必须与 CMS / `rule.sniff` 用**同一条判据**，否则两条路径会对同一个 URL 给出不同结论。 */
export function isMediaUrl(url: string): boolean {
  // `mkv` 也算直链：先交给 `<video>` 试播（H.264 / AAC 的 MKV 多数能播），
  // 播不了时由播放器提示换线路，不该当成播放页去嗅探。
  return /\.(?:m3u8|mp4|m4v|flv|mkv)(?:\?|$)/i.test(url);
}

/** 按地址猜线路类型（CMS / 声明式 / uzVideo 视频源共用）。
 *  MKV 不单列：Chromium 对 Matroska 有部分解复用能力，统一按 mp4 试播。 */
export function playbackTypeFromUrl(url: string): Playback["type"] {
  const path = url.split(/[?#]/)[0]?.toLowerCase() ?? "";
  if (path.endsWith(".m3u8")) return "hls";
  if (path.endsWith(".mpd")) return "dash";
  return "mp4";
}

const CMS_UNSUPPORTED = "暂不支持搜索";

/**
 * Apple CMS v10（MacCMS）采集接口来源。
 *
 * 约定（公开聚合配置的事实标准）：
 *  - 搜索 / 列表：`?ac=detail&wd=<关键词>&pg=<页>`；
 *  - 详情：`?ac=detail&ids=<vod_id>`，`vod_play_url` 形如
 *    `线路1$第1集$https://a.m3u8#第2集$https://b.m3u8$$$线路2$…`；
 *  - 剧集地址直接作为 `Chapter.id`，播放时优先直链，不是直链再交给 WebView 嗅探。
 */
function createCmsSource(
  manifest: SourceManifest,
  cms: CmsRule,
  deps: DeclarativeSourceDeps,
): ResourceSource {
  const sourceId = manifest.id;
  const net = deps.net ?? deps.host.net;
  const api = cms.api.trim();
  if (!/^https?:\/\//i.test(api)) {
    throw new RspError("parse", `${sourceId} 的 cms.api 必须是 http(s) 绝对地址`, sourceId);
  }
  const origin = new URL(api).origin;
  const headers = { ...manifest.headers, ...cms.headers };

  async function fetchList(
    query: Record<string, string | number | undefined>,
    options?: SourceCallOptions,
  ): Promise<Record<string, unknown>[]> {
    const url = new URL(api);
    url.searchParams.set("ac", "detail");
    for (const [key, value] of Object.entries(query)) {
      if (value === undefined || value === "") continue;
      url.searchParams.set(key, String(value));
    }
    const response = await net.request({
      url: url.toString(),
      method: "GET",
      headers,
      timeoutMs: options?.timeoutMs ?? 8_000,
      signal: options?.signal,
    });
    if (response.status >= 400) {
      throw statusError(response.status, url.toString(), sourceId);
    }
    let payload: unknown;
    try {
      payload = JSON.parse(response.body.replace(/^\uFEFF/, ""));
    } catch {
      // 部分站点在关闭搜索时返回纯文本（如「暂不支持搜索」），照原样抛给用户。
      const plain = stripTags(response.body).slice(0, 40);
      throw new RspError(
        "unsupported",
        plain.includes(CMS_UNSUPPORTED) ? "该采集站未开放搜索" : `响应不是合法 JSON：${plain}`,
        sourceId,
      );
    }
    const root = isRecord(payload) ? payload : {};
    const data = isRecord(root.data) ? root.data : root;
    const list = data.list ?? root.list;
    const items = Array.isArray(list) ? list : isRecord(list) ? Object.values(list) : [];
    return items.filter(isRecord);
  }

  function toWork(item: Record<string, unknown>): Work | undefined {
    const id = asText(item.vod_id);
    if (!id) return undefined;
    const cover = asText(item.vod_pic);
    const content = asText(item.vod_content);
    const tags = asText(item.type_name)
      .split(/[,，/]/)
      .map((tag) => tag.trim())
      .filter(Boolean);
    return {
      ref: { sourceId, workId: id, kind: manifest.kind[0] ?? "tv" },
      title: asText(item.vod_name) || id,
      author: asText(item.vod_director) || undefined,
      cover: cover ? resolveUrl(origin, cover) : undefined,
      description: content ? stripTags(content) : undefined,
      tags: tags.length > 0 ? tags : undefined,
      extra: {
        remarks: asText(item.vod_remarks) || undefined,
        year: asText(item.vod_year) || undefined,
        area: asText(item.vod_area) || undefined,
      },
    };
  }

  const source: ResourceSource = {
    manifest,
    async search(query: string, page = 1, options?: SourceCallOptions): Promise<Paged<Work>> {
      const rows = await fetchList({ wd: query, pg: page }, options);
      return {
        items: rows.map(toWork).filter((work): work is Work => !!work),
        next: rows.length > 0 ? String(page + 1) : null,
      };
    },
    async explore(section = "", page = 1, options?: SourceCallOptions): Promise<Paged<Work>> {
      const rows = await fetchList({ t: section, pg: page }, options);
      return {
        items: rows.map(toWork).filter((work): work is Work => !!work),
        next: rows.length > 0 ? String(page + 1) : null,
      };
    },
    async chapters(ref: WorkRef, options?: SourceCallOptions): Promise<Chapter[]> {
      const rows = await fetchList({ ids: ref.workId }, options);
      const item = rows[0];
      if (!item) throw new RspError("parse", `采集接口没有返回详情：${ref.workId}`, sourceId);
      const froms = splitLines(item.vod_play_from);
      const lines = splitLines(item.vod_play_url);
      const chapters: Chapter[] = [];
      lines.forEach((line, lineIndex) => {
        const group = froms[lineIndex] || `线路${lineIndex + 1}`;
        for (const entry of line.split("#")) {
          const raw = entry.trim();
          if (!raw) continue;
          const at = raw.indexOf("$");
          const title = at >= 0 ? raw.slice(0, at).trim() : "";
          const url = at >= 0 ? raw.slice(at + 1).trim() : raw;
          if (!/^https?:\/\//i.test(url)) continue;
          chapters.push({
            id: url,
            workId: ref.workId,
            title: title || `${chapters.length + 1}`,
            index: chapters.length,
            ...(lines.length > 1 ? { group } : {}),
          });
        }
      });
      if (chapters.length === 0) {
        throw new RspError("parse", "详情里没有可播放的剧集地址", sourceId);
      }
      return chapters;
    },
    async play(chapter: Chapter, options?: SourceCallOptions): Promise<Playback[]> {
      const url = /^https?:\/\//i.test(chapter.id)
        ? chapter.id
        : resolveUrl(origin, chapter.id);
      const type: Playback["type"] = playbackTypeFromUrl(url);
      if (isMediaUrl(url)) {
        return [{ url, type, ...(Object.keys(headers).length > 0 ? { headers } : {}) }];
      }
      // 采集站给出的是播放页（如 `https://play.x.com/play/xxx`）：交给宿主嗅探。
      if (cms.sniff === false) {
        throw new RspError("unsupported", `剧集地址不是直链：${url}`, sourceId);
      }
      if (!deps.host.sniff) {
        throw new RspError("unsupported", "宿主未实现 WebView 嗅探（需 webview.sniff）", sourceId);
      }
      const urls = await deps.host.sniff.media(url, {
        timeoutMs: cms.sniffTimeoutMs ?? options?.timeoutMs,
      });
      return urls
        .filter((item) => item !== "")
        .map((item): Playback => ({
          url: item,
          type: playbackTypeFromUrl(item),
          ...(Object.keys(headers).length > 0 ? { headers } : {}),
        }));
    },
  };

  return source;
}

export function createDeclarativeSource(
  input: DeclarativeSourceInput,
  deps: DeclarativeSourceDeps,
): ResourceSource {
  const { manifest, rule } = input;
  if (rule.cms) return createCmsSource(manifest, rule.cms, deps);
  const sourceId = manifest.id;
  const parseHtml = deps.parseHtml ?? defaultParseHtml;
  const net = deps.net ?? deps.host.net;

  async function runListWithRoot(
    listRule: ListRule,
    vars: RuleVars,
    options?: SourceCallOptions,
  ): Promise<{ rows: Record<string, string>[]; root?: ParentNode }> {
    const url = buildRequestUrl(manifest.baseUrl, listRule.request, vars);
    const headers = {
      ...interpolateMap(manifest.headers, vars),
      ...interpolateMap(listRule.request.headers, vars),
    };
    const asJson = !!listRule.list.jsonPath;
    const response = await net.request({
      url,
      method: listRule.request.method,
      headers,
      body: listRule.request.body
        ? interpolate(listRule.request.body, vars)
        : undefined,
      timeoutMs: options?.timeoutMs ?? 8_000,
      signal: options?.signal,
    });
    if (response.status >= 400) {
      throw statusError(response.status, url, sourceId);
    }
    if (asJson) {
      let payload: unknown;
      try {
        payload = JSON.parse(response.body);
      } catch {
        throw new RspError("parse", `响应不是合法 JSON：${url}`, sourceId);
      }
      return { rows: extractJsonRows(payload, listRule, vars) };
    }
    const root = parseHtml(response.body);
    return { rows: extractHtmlRows(root, listRule, vars), root };
  }

  async function runList(
    listRule: ListRule,
    vars: RuleVars,
    options?: SourceCallOptions,
  ): Promise<Record<string, string>[]> {
    return (await runListWithRoot(listRule, vars, options)).rows;
  }

  function baseVars(): RuleVars {
    return { sourceId };
  }

  function rowsToWorks(rows: Record<string, string>[]): Work[] {
    const kind = manifest.kind[0] ?? "comic";
    return rows
      .map((row, index): Work => {
        const id = row.id || row.url || String(index);
        return {
          ref: { sourceId, workId: id, kind },
          title: row.title || id,
          author: row.author || undefined,
          cover: resolveUrl(manifest.baseUrl, row.cover ?? ""),
          description: row.description || undefined,
          tags: row.tags
            ? row.tags.split(/[,，]/).map((tag) => tag.trim()).filter(Boolean)
            : undefined,
        };
      })
      .filter((work) => work.title !== "");
  }

  function chapterVars(ref: WorkRef): RuleVars {
    const workUrl = resolveUrl(manifest.baseUrl, ref.workId);
    return { ...baseVars(), workId: ref.workId, workUrl };
  }

  function chapterVarsFromChapter(chapter: Chapter): RuleVars {
    const chapterUrl = resolveUrl(manifest.baseUrl, chapter.id);
    return {
      ...baseVars(),
      workId: chapter.workId,
      workUrl: resolveUrl(manifest.baseUrl, chapter.workId),
      chapterId: chapter.id,
      chapterUrl,
    };
  }

  const source: ResourceSource = {
    manifest,
    async search(query: string, page = 1, options?: SourceCallOptions): Promise<Paged<Work>> {
      if (!rule.search) {
        throw new RspError("unsupported", `${sourceId} 没有 search 规则`, sourceId);
      }
      const rows = await runList(rule.search, { ...baseVars(), keyword: query, page }, options);
      const items = rowsToWorks(rows);
      return { items, next: rows.length > 0 ? String(page + 1) : null };
    },
    async explore(section = "", page = 1, options?: SourceCallOptions): Promise<Paged<Work>> {
      if (!rule.explore) {
        throw new RspError("unsupported", `${sourceId} 没有 explore 规则`, sourceId);
      }
      const rows = await runList(rule.explore, { ...baseVars(), section, page }, options);
      const items = rowsToWorks(rows);
      return { items, next: rows.length > 0 ? String(page + 1) : null };
    },
    async chapters(ref: WorkRef, options?: SourceCallOptions): Promise<Chapter[]> {
      if (!rule.chapters) {
        throw new RspError("unsupported", `${sourceId} 没有 chapters 规则`, sourceId);
      }
      const chaptersRule: ChaptersRule = rule.chapters;
      const vars = chapterVars(ref);
      const { rows, root } = await runListWithRoot(chaptersRule, vars, options);

      // 扁平规则：list 的每一项就是章节。
      if (!chaptersRule.sub || !root) {
        return rows.map((row, index) => ({
          id: row.id || row.url || String(index),
          workId: ref.workId,
          title: row.title || `${index + 1}`,
          index,
        }));
      }

      // 两层规则：list 取线路，sub 在线路节点内取剧集（Kazumi chapterRoads/chapterResult）。
      const groups = selectElements(root, chaptersRule.list);
      const chapters: Chapter[] = [];
      groups.forEach((group, groupIndex) => {
        const groupTitle = `线路${groupIndex + 1}`;
        const episodes = extractHtmlRows(
          group,
          {
            request: chaptersRule.request,
            list: chaptersRule.sub!.list,
            fields: chaptersRule.sub!.fields,
          },
          vars,
        );
        for (const row of episodes) {
          const id = row.id || row.url;
          if (!id) continue;
          const index = chapters.length;
          const title = row.title || `${index + 1}`;
          chapters.push({
            id,
            workId: ref.workId,
            title: groups.length > 1 ? `${groupTitle} · ${title}` : title,
            index,
            ...(groups.length > 1 ? { group: groupTitle } : {}),
          });
        }
      });
      return chapters;
    },
    async pages(chapter: Chapter, options?: SourceCallOptions): Promise<ComicPage[]> {
      if (!rule.pages) {
        throw new RspError("unsupported", `${sourceId} 没有 pages 规则`, sourceId);
      }
      const rows = await runList(rule.pages, chapterVarsFromChapter(chapter), options);
      const headers = interpolateMap(manifest.headers, chapterVarsFromChapter(chapter));
      return rows.map((row, index) => ({
        index,
        url: resolveUrl(manifest.baseUrl, row.url || row.src || ""),
        ...(Object.keys(headers).length > 0 ? { headers } : {}),
      }));
    },
    async content(chapter: Chapter, options?: SourceCallOptions): Promise<TextChapter> {
      if (!rule.content) {
        throw new RspError("unsupported", `${sourceId} 没有 content 规则`, sourceId);
      }
      const rows = await runList(rule.content, chapterVarsFromChapter(chapter), options);
      const content = rows[0]?.content ?? rows[0]?.text ?? "";
      return {
        chapterId: chapter.id,
        format: rule.contentFormat ?? "text",
        content,
      };
    },
    async play(chapter: Chapter, options?: SourceCallOptions): Promise<Playback[]> {
      const vars = chapterVarsFromChapter(chapter);
      if (rule.sniff) {
        const pageUrl = resolveUrl(manifest.baseUrl, interpolate(rule.sniff.url, vars));
        if (!pageUrl) {
          throw new RspError("parse", `${sourceId} 的嗅探地址为空`, sourceId);
        }
        if (!deps.host.sniff) {
          throw new RspError(
            "unsupported",
            "宿主未实现 WebView 嗅探（需 webview.sniff）",
            sourceId,
          );
        }
        const urls = await deps.host.sniff.media(pageUrl, {
          timeoutMs: rule.sniff.timeoutMs ?? options?.timeoutMs,
        });
        const headers = interpolateMap(manifest.headers, vars);
        return urls
          .filter((url) => url !== "")
          .map((url): Playback => ({
            url,
            type: playbackTypeFromUrl(url),
            ...(Object.keys(headers).length > 0 ? { headers } : {}),
          }));
      }
      if (!rule.play) {
        throw new RspError("unsupported", `${sourceId} 没有 play 规则`, sourceId);
      }
      const rows = await runList(rule.play, vars, options);
      const headers = interpolateMap(manifest.headers, vars);
      return rows
        .map((row): Playback => {
          const url = resolveUrl(manifest.baseUrl, row.url || row.playUrl || "");
          const declared = row.type;
          const type: Playback["type"] =
            declared === "hls" || declared === "mp4" || declared === "dash" || declared === "webview"
              ? declared
              : playbackTypeFromUrl(url);
          return {
            url,
            type,
            ...(Object.keys(headers).length > 0 ? { headers } : {}),
          };
        })
        .filter((playback) => playback.url !== "");
    },
  };

  return source;
}
