// JS 来源运行时（JS 源 v1，2026-10-18）。
//
// 宿主 QuickJS 每次调用都是**一次性运行时**：所以这里把「来源脚本 + 本次调用」
// 拼成一段代码交给 `engine.eval`，由宿主注入 `host.request(JSON)` 做同步网络。
// 脚本只需要导出一个工厂：
//
//   function createSource(rsp) {
//     return {
//       async search(query, page) { ... },
//       async chapters(work) { ... },
//       async pages(chapter) { ... },
//       async content(chapter) { ... },
//       async play(chapter) { ... },
//     };
//   }
//
// 返回值使用「轻量对象」而不是完整 RSP 类型：SDK 负责补 `ref` / `workId` / `index`，
// 这样来源作者不用关心协议内部字段。结果必须是 JSON 可序列化值。

import {
  RspError,
  type Chapter,
  type ComicPage,
  type Paged,
  type Playback,
  type ResourceSource,
  type SourceManifest,
  type TextChapter,
  type Work,
} from "./contract.js";
import type { LunacHostV3 } from "./host.js";

export interface JsSourceInput {
  manifest: SourceManifest;
  /** 来源脚本正文（用户导入的 .js）。 */
  script: string;
}

/** 一次调用的总预算：内部可能串行发多个请求。宿主 `engine.eval` 上限 30s。 */
const EVAL_TIMEOUT_MS = 30_000;

const RUNNER = String.raw`
;(async () => {
  const rsp = {
    request(options) {
      var merged = {};
      var defaults = (typeof args !== "undefined" && args && args.defaultHeaders) || {};
      for (var dk in defaults) {
        if (Object.prototype.hasOwnProperty.call(defaults, dk)) merged[dk] = defaults[dk];
      }
      var extra = (options && options.headers) || {};
      for (var hk in extra) {
        if (Object.prototype.hasOwnProperty.call(extra, hk)) merged[hk] = extra[hk];
      }
      var requestOptions = {};
      for (var ok in (options || {})) {
        if (Object.prototype.hasOwnProperty.call(options, ok)) requestOptions[ok] = options[ok];
      }
      requestOptions.headers = merged;
      const raw = host.request(JSON.stringify(requestOptions));
      let envelope;
      try {
        envelope = JSON.parse(raw);
      } catch {
        throw new Error("host.request 返回了非 JSON");
      }
      if (!envelope || envelope.ok !== true) {
        throw new Error((envelope && envelope.error) || "请求失败");
      }
      return envelope.response;
    },
  };
  if (typeof createSource !== "function") {
    throw new Error("JS 源必须定义 createSource(rsp)");
  }
  const source = createSource(rsp);
  const method = args.method;
  const fn = source && source[method];
  if (typeof fn !== "function") {
    throw new Error("JS 源没有实现 " + method);
  }
  return await fn.apply(source, args.params || []);
})()
`;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() !== "" ? value : undefined;
}

function absolute(baseUrl: string | undefined, value: string): string {
  if (!value) return value;
  try {
    return new URL(value, baseUrl).toString();
  } catch {
    return value;
  }
}

/** 沙箱回传的图片字节是 base64（`imageData`）；插件侧同进程解码。 */
export function decodeBase64(value: string): Uint8Array {
  const binary = atob(value);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) out[i] = binary.charCodeAt(i);
  return out;
}

export function toRspError(error: unknown, manifest: SourceManifest): RspError {
  if (error instanceof RspError) return error;
  const message = error instanceof Error ? error.message : String(error);
  if (message.includes("E_DENIED")) return new RspError("blocked", message, manifest.id);
  if (message.includes("E_TIMEOUT")) return new RspError("network", message, manifest.id);
  if (message.includes("E_LIMIT")) return new RspError("unsupported", message, manifest.id);
  return new RspError("parse", message.replace(/^E_SCRIPT:\s*/, ""), manifest.id);
}

function readTags(value: unknown): string[] | undefined {
  if (Array.isArray(value)) {
    const tags = value.map((item) => text(item)).filter((item): item is string => Boolean(item));
    return tags.length > 0 ? tags : undefined;
  }
  const raw = text(value);
  if (!raw) return undefined;
  const tags = raw.split(/[,，]/).map((item) => item.trim()).filter(Boolean);
  return tags.length > 0 ? tags : undefined;
}

function readStatus(value: unknown): Work["status"] {
  return value === "ongoing" || value === "completed" || value === "unknown" ? value : undefined;
}

export function createJsSource(input: JsSourceInput, deps: { host: LunacHostV3 }): ResourceSource {
  const { manifest, script } = input;
  const { host } = deps;
  if (script.trim() === "") {
    throw new RspError("unsupported", "JS 源脚本为空", manifest.id);
  }
  const kind = manifest.kind[0] ?? "comic";

  async function run(method: string, params: unknown[]): Promise<unknown> {
    try {
      return await host.engine.eval(`${script}\n${RUNNER}`, {
        timeoutMs: EVAL_TIMEOUT_MS,
        args: { method, params, defaultHeaders: manifest.headers ?? {} },
      });
    } catch (error) {
      throw toRspError(error, manifest);
    }
  }

  function toWorks(result: unknown, label: string): Paged<Work> {
    if (!isRecord(result) || !Array.isArray(result.items)) {
      throw new RspError("parse", `${label} 必须返回 { items: [...] }`, manifest.id);
    }
    const items: Work[] = result.items.map((raw, index) => {
      if (!isRecord(raw)) {
        throw new RspError("parse", `${label}.items[${index}] 不是对象`, manifest.id);
      }
      const workId = text(raw.id) ?? String(index);
      return {
        ref: { sourceId: manifest.id, workId, kind },
        title: text(raw.title) ?? workId,
        author: text(raw.author),
        cover: text(raw.cover)
          ? absolute(manifest.baseUrl, text(raw.cover) as string)
          : undefined,
        description: text(raw.description),
        tags: readTags(raw.tags),
        status: readStatus(raw.status),
        updatedAt: typeof raw.updatedAt === "number" ? raw.updatedAt : undefined,
      };
    });
    return {
      items,
      maxPage: typeof result.maxPage === "number" ? result.maxPage : undefined,
      next: text(result.next) ?? null,
    };
  }

  return {
    manifest,
    async search(query: string, page = 1): Promise<Paged<Work>> {
      return toWorks(await run("search", [query, page]), "search");
    },

    async explore(section = "", page = 1): Promise<Paged<Work>> {
      return toWorks(await run("explore", [section, page]), "explore");
    },

    async chapters(work): Promise<Chapter[]> {
      const result = await run("chapters", [{ id: work.workId, kind: work.kind }]);
      if (!Array.isArray(result)) {
        throw new RspError("parse", "chapters 必须返回数组", manifest.id);
      }
      return result.map((raw, index) => {
        const item = isRecord(raw) ? raw : {};
        const id = text(item.id) ?? String(index);
        return {
          id,
          workId: work.workId,
          title: text(item.title) ?? id,
          index,
          group: text(item.group),
          publishedAt: typeof item.publishedAt === "number" ? item.publishedAt : undefined,
        };
      });
    },

    async pages(chapter): Promise<ComicPage[]> {
      const result = await run("pages", [
        { id: chapter.id, workId: chapter.workId, title: chapter.title, index: chapter.index },
      ]);
      if (!Array.isArray(result)) {
        throw new RspError("parse", "pages 必须返回数组", manifest.id);
      }
      return result.map((raw, index) => {
        const item = isRecord(raw) ? raw : {};
        const url = text(item.url) ?? text(item.src) ?? text(raw) ?? "";
        if (!url) {
          throw new RspError("parse", `pages[${index}] 缺少 url`, manifest.id);
        }
        const headers = isRecord(item.headers)
          ? Object.fromEntries(
            Object.entries(item.headers)
              .filter((entry): entry is [string, string] => typeof entry[1] === "string"),
          )
          : undefined;
        return {
          index,
          url: absolute(manifest.baseUrl, url),
          ...(headers && Object.keys(headers).length > 0 ? { headers } : {}),
        };
      });
    },

    async content(chapter): Promise<TextChapter> {
      const result = await run("content", [
        { id: chapter.id, workId: chapter.workId, title: chapter.title, index: chapter.index },
      ]);
      if (typeof result === "string") {
        return { chapterId: chapter.id, format: "text", content: result };
      }
      if (!isRecord(result)) {
        throw new RspError("parse", "content 必须返回字符串或 { content }", manifest.id);
      }
      const format =
        result.format === "html" || result.format === "epub" ? result.format : "text";
      const content = text(result.content) ?? text(result.text) ?? "";
      return { chapterId: chapter.id, format, content };
    },

    async play(chapter): Promise<Playback[]> {
      const result = await run("play", [
        { id: chapter.id, workId: chapter.workId, title: chapter.title, index: chapter.index },
      ]);
      if (!Array.isArray(result)) {
        throw new RspError("parse", "play 必须返回数组", manifest.id);
      }
      return result.map((raw, index) => {
        const item = isRecord(raw) ? raw : {};
        const url = text(item.url);
        if (!url) {
          throw new RspError("parse", `play[${index}] 缺少 url`, manifest.id);
        }
        const type =
          item.type === "hls" || item.type === "mp4" || item.type === "dash" || item.type === "webview"
            ? item.type
            : /\.m3u8(\?|$)/i.test(url)
              ? "hls"
              : "mp4";
        const subtitles = Array.isArray(item.subtitles)
          ? item.subtitles
            .filter(isRecord)
            .map((subtitle) => ({
              label: text(subtitle.label) ?? "字幕",
              url: absolute(manifest.baseUrl, text(subtitle.url) ?? ""),
              lang: text(subtitle.lang),
            }))
            .filter((subtitle) => subtitle.url !== "")
          : undefined;
        return {
          url: absolute(manifest.baseUrl, url),
          type,
          ...(subtitles && subtitles.length > 0 ? { subtitles } : {}),
        };
      });
    },
  };
}
