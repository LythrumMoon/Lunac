// RSP v1 —— 资源源协议的唯一真相源。
//
// 只放「跨宿主共享」的契约：Lunac 插件、iOS / Android 独立应用、AI 工具都读这一份。
// 不要在这里引入 DOM / Node / Tauri 类型；它必须能在任何运行时里 import。

export const RSP_VERSION = 1 as const;

export type SourceKind = "comic" | "novel" | "anime" | "tv";

export type SourceEngine = "declarative" | "js" | "builtin";

export type RspErrorCode =
  | "network"
  | "parse"
  | "auth"
  | "blocked"
  | "unsupported"
  | "unknown";

/** 统一的来源错误。UI / AI / 日志都按 `code` 展示，不再各自猜文案。 */
export class RspError extends Error {
  readonly code: RspErrorCode;
  readonly sourceId?: string;

  constructor(code: RspErrorCode, message: string, sourceId?: string) {
    super(message);
    this.name = "RspError";
    this.code = code;
    this.sourceId = sourceId;
  }
}

export interface WorkRef {
  sourceId: string;
  workId: string;
  kind: SourceKind;
}

export interface Work {
  ref: WorkRef;
  title: string;
  altTitles?: string[];
  author?: string;
  cover?: string;
  description?: string;
  tags?: string[];
  status?: "ongoing" | "completed" | "unknown";
  updatedAt?: number;
  extra?: Record<string, unknown>;
}

export interface Chapter {
  id: string;
  workId: string;
  title: string;
  index: number;
  group?: string;
  publishedAt?: number;
}

export interface ComicPage {
  index: number;
  url: string;
  headers?: Record<string, string>;
  /** 源说明了 `onImageLoad.onResponse`（图片被加密 / 需要改字节）：直连只会是坏图。 */
  transform?: boolean;
}

export interface TextChapter {
  chapterId: string;
  format: "text" | "html" | "epub";
  content: string;
}

export interface Playback {
  url: string;
  type: "hls" | "mp4" | "dash" | "webview";
  headers?: Record<string, string>;
  subtitles?: { label: string; url: string; lang?: string }[];
  duration?: number;
}

export interface Paged<T> {
  items: T[];
  maxPage?: number;
  next?: string | null;
}

// ── 声明式规则 ──────────────────────────────────────────────────
// 选择器三选一；`jsonPath` 用于 API 型来源，`xpath` 兼容 Kazumi 规则。
export interface Selector {
  css?: string;
  xpath?: string;
  jsonPath?: string;
}

export interface FieldExtractor {
  selector: Selector;
  /** 取属性（如 href / src / data-src）；不写 = 取文本。 */
  attr?: string;
  /** 明确取文本（与 attr 二选一）。 */
  text?: boolean;
  /** 从取到的值里再抽一段（第一个捕获组）。 */
  regex?: string;
  /** 把抽取结果套进模板；`@value` 为占位符。 */
  template?: string;
}

export interface HttpRequestTemplate {
  method: "GET" | "POST";
  /** 相对 `manifest.baseUrl` 或绝对 URL；`@keyword` / `@page` / `@workUrl` 为占位符。 */
  url: string;
  query?: Record<string, string>;
  headers?: Record<string, string>;
  body?: string;
}

export interface ListRule {
  request: HttpRequestTemplate;
  list: Selector;
  fields: Record<string, FieldExtractor>;
  paging?: {
    type: "page" | "next";
    param?: string;
    start?: number;
  };
}

/**
 * 章节规则：除扁平列表外，支持「线路 → 剧集」两层结构
 * （Kazumi 的 `chapterRoads` 取线路，`chapterResult` 在线路内取剧集）。
 */
export interface ChaptersRule extends ListRule {
  /** 每个线路节点下的剧集列表；缺省时 `list` 本身就是章节。 */
  sub?: {
    list: Selector;
    fields: Record<string, FieldExtractor>;
  };
}

/**
 * Apple CMS v10（MacCMS）采集接口（2026-10-21）。
 *
 * 公开影视聚合配置（TVBox / LibreTV 等）里的站点绝大多数是这套接口：
 * `?ac=detail&wd=` 搜索、`?ac=detail&ids=` 详情、`vod_play_url` 里是「剧集名$地址#…」。
 * 写规则时只需要接口地址，搜索 / 总览 / 剧集 / 播放都由执行器按同一套约定完成。
 */
export interface CmsRule {
  /** 采集接口绝对地址，如 `https://x/api.php/provide/vod`；查询串由执行器追加。 */
  api: string;
  /** 额外的请求头（站点校验 Referer 时用）。 */
  headers?: Record<string, string>;
  /**
   * 剧集地址不是直链（m3u8 / mp4）时改用宿主 WebView 嗅探，缺省开启。
   * 需要插件声明 `webview.sniff`。
   */
  sniff?: boolean;
  sniffTimeoutMs?: number;
}

/** WebView 嗅探规则（2026-10-21）：把剧集页交给宿主隐藏 WebView，拦截媒体请求。 */
export interface SniffRule {
  /** 剧集页地址，支持 `@chapterUrl` / `@chapterId` / `@workUrl` 占位符。 */
  url: string;
  /** 最长等待毫秒数；缺省由宿主决定（15s）。 */
  timeoutMs?: number;
}

export interface DeclarativeRule {
  search?: ListRule;
  /** 总览 / 分类列表：`section` / `page` 可做 URL 占位符。 */
  explore?: ListRule;
  chapters?: ChaptersRule;
  pages?: ListRule;
  play?: ListRule;
  /** Apple CMS v10 采集接口（与上面的通用规则二选一）。 */
  cms?: CmsRule;
  /**
   * 播放地址靠 WebView 嗅探（Kazumi `useWebview`，2026-10-21）：
   * 打开 `url`（支持 `@chapterUrl` 等占位符）并拦截 m3u8 / mp4。
   * 与 `play` 二选一；宿主需声明 `permissions: ["webview.sniff"]`。
   */
  sniff?: SniffRule;
  content?: ListRule;
  /**
   * `content` 输出的解释方式（缺省 `text`）：
   *  - `text`：正文纯文本；
   *  - `html`：正文 HTML，阅读器按白名单转纯文本；
   *  - `epub`：输出的是 EPUB 文件的 http(s) 地址，由阅读器下载并按 spine 展开。
   */
  contentFormat?: TextFormat;
}

export type TextFormat = "text" | "html" | "epub";

// ── 清单 ────────────────────────────────────────────────────────
export interface SourceManifest {
  rsp: typeof RSP_VERSION;
  /** 目录名 / 唯一 id：`[a-z0-9._-]`，不以 `.` 开头。 */
  id: string;
  name: string;
  version: string;
  kind: SourceKind[];
  engine: SourceEngine;
  /** 相对来源包根目录；`declarative` 指向规则 JSON。 */
  entry: string;
  baseUrl?: string;
  homepage?: string;
  license?: string;
  minHost?: string;
  headers?: Record<string, string>;
  /**
   * 来源设置覆盖（键名与脚本 `settings` 一致，如 `domains` / `base_url`）。
   * JS 源读取设置时优先使用这里的值，便于域名失效后按来源切换。
   */
  settings?: Record<string, string>;
  capabilities: RspCapability[];
  rateLimit?: { rps: number; burst: number };
  nsfw?: boolean;
}

export type RspCapability =
  | "search"
  | "explore"
  | "detail"
  | "chapters"
  | "pages"
  | "content"
  | "play"
  | "auth";

export interface ResourceSource {
  manifest: SourceManifest;
  search?(query: string, page?: number, options?: SourceCallOptions): Promise<Paged<Work>>;
  explore?(section?: string, page?: number, options?: SourceCallOptions): Promise<Paged<Work>>;
  detail?(ref: WorkRef, options?: SourceCallOptions): Promise<Work>;
  chapters?(ref: WorkRef, options?: SourceCallOptions): Promise<Chapter[]>;
  pages?(chapter: Chapter, options?: SourceCallOptions): Promise<ComicPage[]>;
  /** 取回一张章节图片的**明文**字节（`transform: true` 的页才需要）。 */
  imageData?(chapter: Chapter, page: ComicPage, options?: SourceCallOptions): Promise<Uint8Array | null>;
  /** 取回一张封面 / 缩略图的**明文**字节（源用 `onThumbnailLoad` 解密时才有）。 */
  thumbnailData?(url: string, options?: SourceCallOptions): Promise<Uint8Array | null>;
  content?(chapter: Chapter, options?: SourceCallOptions): Promise<TextChapter>;
  play?(chapter: Chapter, options?: SourceCallOptions): Promise<Playback[]>;
}

/** 所有来源调用共用的取消 / 超时选项。 */
export interface SourceCallOptions {
  /** 切换页面 / 新搜索 / 离开详情时取消旧请求。 */
  signal?: AbortSignal;
  /** 单次请求超时；缺省由网络客户端补 8s。 */
  timeoutMs?: number;
}
