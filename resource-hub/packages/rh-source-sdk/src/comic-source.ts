// 漫画源适配（2026-10-18）。
//
// 上游形态：`class X extends ComicSource`，通过 `search.load` / `comic.loadInfo` /
// `comic.loadEp` 暴露能力。这里只把这三条主链路翻译成 RSP，不复制上游代码。
// 具体 API 由 `comic-shim.ts` 在沙箱里补齐（Network / HtmlDocument / 数据模型）。

import {
  RspError,
  type Chapter,
  type ComicPage,
  type Paged,
  type ResourceSource,
  type SourceManifest,
  type Work,
} from "./contract.js";
import type { LunacHostV3 } from "./host.js";
import { decodeBase64, toRspError } from "./js-source.js";
import { COMIC_PRELUDE, COMIC_RUNNER } from "./comic-shim.js";

export interface ComicSourceInput {
  manifest: SourceManifest;
  script: string;
}

/** 一次调用的总预算（搜索 / 章节 / 图片都可能串行发多个请求）。
 *  2026-10-21：图片页要串行拉 N 页（百合会一章 19 页），25s 会被宿主的 eval 看门狗
 *  打断，表现为「章节加载失败」。宿主侧上限是 30s，这里取满。 */
const EVAL_TIMEOUT_MS = 30_000;

const CLASS_RE = /^\s*class\s+([A-Za-z_$][\w$]*)\s+extends\s+ComicSource\b/m;

/** 上游脚本是否是 漫画形态（`class ... extends ComicSource`）。 */
export function looksLikeComic(script: string): boolean {
  return CLASS_RE.test(script);
}

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

export function createComicSource(
  input: ComicSourceInput,
  deps: { host: LunacHostV3 },
): ResourceSource {
  const { manifest, script } = input;
  const { host } = deps;
  const match = CLASS_RE.exec(script);
  const className = match?.[1];
  if (!className) {
    throw new RspError(
      "unsupported",
      "不是漫画源：缺少 class ... extends ComicSource",
      manifest.id,
    );
  }

  // 来源自己的持久化数据（登录令牌 / Cookie 等），键按来源 id 隔离。
  const dataKey = `source.${manifest.id}.data`;

  async function run(method: string, args: Record<string, unknown>): Promise<unknown> {
    const code = `${COMIC_PRELUDE}\n${script}\n${COMIC_RUNNER}`;
    let storedData: unknown;
    try {
      storedData = await host.storage.get(dataKey);
    } catch {
      storedData = undefined;
    }
    try {
      const result = await host.engine.eval(code, {
        timeoutMs: EVAL_TIMEOUT_MS,
        args: {
          ...args,
          method,
          className,
          settings: manifest.settings ?? {},
          defaultHeaders: manifest.headers ?? {},
          storedData,
        },
      });
      if (isRecord(result) && "__lunac_data" in result) {
        const next = result.__lunac_data;
        try {
          if (next === null || next === undefined) await host.storage.remove(dataKey);
          else await host.storage.set(dataKey, next);
        } catch {
          // 存储失败不阻断本次读取：下一次仍可重试。
        }
      }
      return result;
    } catch (error) {
      throw toRspError(error, manifest);
    }
  }

  function toWorks(result: unknown, label: string): Paged<Work> {
    if (!isRecord(result) || !Array.isArray(result.items)) {
      throw new RspError("parse", `漫画源 ${label} 返回了意外结构`, manifest.id);
    }
    const items: Work[] = result.items.map((raw, index) => {
      if (!isRecord(raw)) {
        throw new RspError("parse", `漫画源 ${label}.items[${index}] 不是对象`, manifest.id);
      }
      const workId = text(raw.id) ?? String(index);
      return {
        ref: { sourceId: manifest.id, workId, kind: manifest.kind[0] ?? "comic" },
        title: text(raw.title) ?? workId,
        author: text(raw.author),
        cover: text(raw.cover) ? absolute(manifest.baseUrl, text(raw.cover) as string) : undefined,
        description: text(raw.description),
        tags: Array.isArray(raw.tags)
          ? raw.tags.map((tag) => text(tag)).filter((tag): tag is string => Boolean(tag))
          : undefined,
        status:
          raw.status === "ongoing" || raw.status === "completed" ? raw.status : undefined,
      };
    });
    return {
      items,
      maxPage: typeof result.maxPage === "number" ? result.maxPage : undefined,
      next: null,
    };
  }

  return {
    manifest,

    async detail(ref): Promise<Work> {
      const result = await run("detail", { workId: ref.workId });
      if (!isRecord(result) || !isRecord(result.work)) {
        throw new RspError("parse", "漫画源 detail 返回了意外结构", manifest.id);
      }
      const work = toWorks({ items: [result.work] }, "detail").items[0];
      if (!work) throw new RspError("parse", "漫画源 detail 没有返回作品", manifest.id);
      return work;
    },

    async search(query: string, page = 1): Promise<Paged<Work>> {
      return toWorks(await run("search", { query, page }), "search");
    },

    async explore(section = "", page = 1): Promise<Paged<Work>> {
      return toWorks(await run("explore", { section, page }), "explore");
    },

    async chapters(work): Promise<Chapter[]> {
      const result = await run("chapters", { workId: work.workId });
      if (!isRecord(result) || !Array.isArray(result.chapters)) {
        throw new RspError("parse", "漫画源 chapters 返回了意外结构", manifest.id);
      }
      return result.chapters.map((raw, index) => {
        const item = isRecord(raw) ? raw : {};
        const id = text(item.id) ?? String(index);
        return {
          id,
          workId: work.workId,
          title: text(item.title) ?? id,
          index,
        };
      });
    },

    async pages(chapter): Promise<ComicPage[]> {
      const result = await run("pages", {
        workId: chapter.workId,
        chapterId: chapter.id,
      });
      if (!isRecord(result) || !Array.isArray(result.pages)) {
        throw new RspError("parse", "漫画源 pages 返回了意外结构", manifest.id);
      }
      return result.pages.map((raw, index) => {
        const item = isRecord(raw) ? raw : {};
        const url = text(item.url) ?? text(raw) ?? "";
        if (!url) {
          throw new RspError("parse", `漫画源 pages[${index}] 缺少 url`, manifest.id);
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
          ...(item.transform === true ? { transform: true } : {}),
        };
      });
    },

    async imageData(chapter, page): Promise<Uint8Array | null> {
      const result = await run("image", {
        url: page.url,
        headers: page.headers ?? {},
        workId: chapter.workId,
        chapterId: chapter.id,
      });
      const encoded = isRecord(result) ? text(result.data) : undefined;
      if (!encoded) return null;
      return decodeBase64(encoded);
    },

    async thumbnailData(url: string): Promise<Uint8Array | null> {
      // 封面没有 chapter 上下文 ⇒ 沙箱按 `onThumbnailLoad` 处理（不传 chapterId）。
      const result = await run("image", { url, headers: {}, workId: "", chapterId: "" });
      const encoded = isRecord(result) ? text(result.data) : undefined;
      if (!encoded) return null;
      return decodeBase64(encoded);
    },
  };
}
