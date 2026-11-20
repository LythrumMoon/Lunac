// 视频源（type 101）适配（2026-10-18）。
//
// 上游形态：一组全局函数 `getVideoPlayUrl` / `searchVideo` / `getVideoDetail`，
// 配合 `req` / `cheerio` / `UZUtils`，返回值用 `RepVideo*` / `VideoDetail` 数据类拼装
// （2026-10-28 按上游真实脚本补齐）。这里只翻译三条主链路：
//   searchVideo      → search
//   getVideoDetail   → chapters（vod_play_from / vod_play_url）
//   getVideoPlayUrl  → play
// 具体 API 由 `comic-shim.ts` 的 VIDEO_PRELUDE 在沙箱里补齐。

import {
  RspError,
  type Chapter,
  type Paged,
  type Playback,
  type ResourceSource,
  type SourceCallOptions,
  type SourceManifest,
  type Work,
} from "./contract.js";
import type { LunacHostV3 } from "./host.js";
import { toRspError } from "./js-source.js";
import { isMediaUrl, playbackTypeFromUrl } from "./executor.js";
import { VIDEO_PRELUDE, VIDEO_RUNNER, COMIC_PRELUDE } from "./comic-shim.js";

export interface VideoSourceInput {
  manifest: SourceManifest;
  script: string;
}

const EVAL_TIMEOUT_MS = 30_000;

/** 一次 `play()` 里最多嗅探几个播放页：隐藏 WebView 很贵，多线路时只挑前几个试。 */
const MAX_SNIFF_PER_PLAY = 3;

const UZ_METHODS = [
  "getClassList",
  "getSubclassList",
  "getVideoList",
  "getSubclassVideoList",
  "getVideoDetail",
  "getVideoPlayUrl",
  "searchVideo",
];

function hasMethod(script: string, name: string): boolean {
  return (
    new RegExp(`(?:async\\s+)?function\\s+${name}\\s*\\(`).test(script) ||
    new RegExp(`(?:const|let|var)\\s+${name}\\s*=`).test(script) ||
    new RegExp(`${name}\\s*[:=]\\s*(?:async\\s*)?function`).test(script)
  );
}

/** 上游脚本是否是 视频扩展形态（显式函数或 BaseCode1 模板）。 */
export function looksLikeVideo(script: string): boolean {
  if (/\/\/#BaseCode1#/.test(script)) return true;
  let hits = 0;
  for (const name of UZ_METHODS) if (hasMethod(script, name)) hits++;
  return hits >= 2;
}

/** 脚本是否提供首页 / 分类浏览，可接入总览 explore。 */
export function videoHasExplore(script: string): boolean {
  if (hasMethod(script, "getHome")) return true;
  return hasMethod(script, "getClassList") && hasMethod(script, "getVideoList");
}

/** 上游视频扩展里我们**明确不兼容**的形态；`null` = 可以尝试运行。
 *
 *  判据来自上游仓库取证（2026-10-28）：
 *  · `//#BaseCode1#` 模板正文只有 URL 常量，抓取逻辑在 uz 内置代码里且不公开；
 *  · `//@codeID:<非空>` 是加密源，需要 uz 自家运行时解密。
 *  安装前用它拦一道，免得用户装完才发现「搜索就报不支持」。 */
export type VideoUnsupportedReason = "basecode1" | "encrypted" | "unknown";

export function videoUnsupportedReason(script: string): VideoUnsupportedReason | null {
  if (/\/\/#BaseCode1#/.test(script)) return "basecode1";
  if (/^[ \t]*\/\/[ \t]*@codeID:[ \t]*\S+/m.test(script)) return "encrypted";
  if (!looksLikeVideo(script)) return "unknown";
  return null;
}

/** 上游约定：成对的 `// ignore` 之间是本地开发代码，不入运行时。 */
function stripIgnored(script: string): string {
  const lines = script.split(/\r?\n/);
  const out: string[] = [];
  let ignoring = false;
  for (const line of lines) {
    if (/^\s*\/\/\s*ignore\s*$/.test(line)) {
      ignoring = !ignoring;
      continue;
    }
    if (!ignoring) out.push(line);
  }
  return out.join("\n");
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

export function createVideoSource(
  input: VideoSourceInput,
  deps: { host: LunacHostV3 },
): ResourceSource {
  const { manifest, script } = input;
  const { host } = deps;

  if (/\/\/#BaseCode1#/.test(script)) {
    throw new RspError(
      "unsupported",
      "视频源 BaseCode1 嗅探模板暂不支持，请导入 type 101 显式函数扩展",
      manifest.id,
    );
  }

  const runnable = stripIgnored(script);

  async function run(method: string, args: Record<string, unknown>): Promise<unknown> {
    const code = `${COMIC_PRELUDE}\n${VIDEO_PRELUDE}\n${runnable}\n${VIDEO_RUNNER}`;
    try {
      return await host.engine.eval(code, {
        timeoutMs: EVAL_TIMEOUT_MS,
        args: { ...args, method, defaultHeaders: manifest.headers ?? {} },
      });
    } catch (error) {
      throw toRspError(error, manifest);
    }
  }

  /** 非直链（播放页）交给宿主 WebView 嗅探（2026-10-28，uzVideo sniffer）。
   *
   *  上游 type 101 的 `getVideoPlayUrl` 有时给的是**播放页地址**而不是 m3u8 / mp4；
   *  以前一律按扩展名猜成 mp4，播放器只会报「不支持的格式」。这里用 `isMediaUrl`
   *  （与 CMS / `rule.sniff` **同一条判据**）把非直链交给 `host.sniff.media` 换回真直链。
   *  嗅探不可用（旧宿主 / 未声明 `webview.sniff`）或嗅探不到时**原样保留** ——
   *  这是纯增强，不让原本能播的来源回归。 */
  async function sniffPlayPages(
    items: Playback[],
    options?: SourceCallOptions,
  ): Promise<Playback[]> {
    const out: Playback[] = [];
    let tried = 0;
    for (const item of items) {
      if (isMediaUrl(item.url) || !host.sniff || tried >= MAX_SNIFF_PER_PLAY) {
        out.push(item);
        continue;
      }
      tried += 1;
      try {
        const urls = await host.sniff.media(item.url, { timeoutMs: options?.timeoutMs });
        const found = urls
          .filter((url) => url !== "")
          .map((url): Playback => ({
            url: absolute(manifest.baseUrl, url),
            type: playbackTypeFromUrl(url),
            ...(item.headers ? { headers: item.headers } : {}),
          }));
        if (found.length > 0) {
          out.push(...found);
          continue;
        }
      } catch {
        // 嗅探失败（能力未声明 / WebView 超时）：退回原地址，由播放器给出可读错误。
      }
      out.push(item);
    }
    return out;
  }

  function workFrom(raw: unknown, index: number): Work {
    if (!isRecord(raw)) {
      throw new RspError("parse", `视频源 items[${index}] 不是对象`, manifest.id);
    }
    const workId = text(raw.id) ?? String(index);
    return {
      ref: { sourceId: manifest.id, workId, kind: manifest.kind[0] ?? "anime" },
      title: text(raw.title) ?? workId,
      author: text(raw.author),
      cover: text(raw.cover) ? absolute(manifest.baseUrl, text(raw.cover) as string) : undefined,
      description: text(raw.description),
      tags: Array.isArray(raw.tags)
        ? raw.tags.map((tag) => text(tag)).filter((tag): tag is string => Boolean(tag))
        : undefined,
      status: raw.status === "ongoing" || raw.status === "completed" ? raw.status : undefined,
    };
  }

  return {
    manifest,

    async detail(ref): Promise<Work> {
      const result = await run("detail", { workId: ref.workId });
      if (!isRecord(result) || !isRecord(result.work)) {
        throw new RspError("parse", "视频源 detail 返回了意外结构", manifest.id);
      }
      return workFrom(result.work, 0);
    },

    async search(query: string, page = 1): Promise<Paged<Work>> {
      const result = await run("search", { query, page });
      if (!isRecord(result) || !Array.isArray(result.items)) {
        throw new RspError("parse", "视频源 search 返回了意外结构", manifest.id);
      }
      return { items: result.items.map(workFrom), next: null };
    },

    async explore(section = "", page = 1): Promise<Paged<Work>> {
      if (!videoHasExplore(script)) {
        throw new RspError(
          "unsupported",
          "视频源没有实现 getHome / getClassList",
          manifest.id,
        );
      }
      const result = await run("explore", { section, page });
      if (!isRecord(result) || !Array.isArray(result.items)) {
        throw new RspError("parse", "视频源 explore 返回了意外结构", manifest.id);
      }
      return {
        items: result.items.map(workFrom),
        next: section ? String(page + 1) : null,
      };
    },

    async chapters(work): Promise<Chapter[]> {
      const result = await run("chapters", { workId: work.workId });
      if (!isRecord(result) || !Array.isArray(result.chapters)) {
        throw new RspError("parse", "视频源 chapters 返回了意外结构", manifest.id);
      }
      return result.chapters.map((raw, index) => {
        const item = isRecord(raw) ? raw : {};
        const id = text(item.id) ?? String(index);
        return {
          id,
          workId: work.workId,
          title: text(item.title) ?? id,
          index,
          group: text(item.group),
        };
      });
    },

    async play(chapter, options?: SourceCallOptions): Promise<Playback[]> {
      const result = await run("play", {
        workId: chapter.workId,
        chapterId: chapter.id,
      });
      if (!isRecord(result) || !Array.isArray(result.playbacks)) {
        throw new RspError("parse", "视频源 play 返回了意外结构", manifest.id);
      }
      const playbacks: Playback[] = result.playbacks.map((raw, index) => {
        const item = isRecord(raw) ? raw : {};
        const url = text(item.url) ?? text(raw) ?? "";
        if (!url) throw new RspError("parse", `视频源 play[${index}] 缺少 url`, manifest.id);
        const headers = isRecord(item.headers)
          ? Object.fromEntries(
              Object.entries(item.headers)
                .map(([key, value]) => [key, typeof value === "string" ? value : String(value)])
                .filter(([, value]) => value !== ""),
            )
          : undefined;
        return {
          url: absolute(manifest.baseUrl, url),
          type: playbackTypeFromUrl(url),
          headers,
        };
      });
      if (!playbacks.length) {
        throw new RspError("unsupported", "视频源没有返回可播放地址", manifest.id);
      }
      return sniffPlayPages(playbacks, options);
    },
  };
}
