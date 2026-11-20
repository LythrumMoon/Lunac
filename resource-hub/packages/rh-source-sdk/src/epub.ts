// EPUB 解析（B6 子项，2026-10-18）。
//
// 链路：ZIP → OPF → spine → 正文 + 图片 + 语义标签。
//  - 不依赖 DOM / 浏览器，Node 与移动端都能用；
//  - 不执行 EPUB 内的脚本 / CSS；图片转 data URL，标签走白名单；
//  - 章节标题优先取 NCX，其次 XHTML 的 h1-3 / `<title>`。
//
// 阅读器把每个 spine 项当成一页（HTML 页可滚动），进度仍用 pageIndex。

import { unzipSync } from "fflate";

import { RspError } from "./contract.js";

export interface EpubChapter {
  /** spine 项在压缩包里的相对路径。 */
  id: string;
  title: string;
  text: string;
  /** 清洗后的正文 HTML（保留标题 / 粗体 / 列表 / 图片）。 */
  html?: string;
}

export interface EpubBook {
  title: string;
  author?: string;
  chapters: EpubChapter[];
}

const ENTITIES: Record<string, string> = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  nbsp: " ",
  mdash: "—",
  ndash: "–",
  hellip: "…",
  lsquo: "‘",
  rsquo: "’",
  ldquo: "“",
  rdquo: "”",
};

function decodeEntities(value: string): string {
  return value.replace(/&(#x?[0-9a-f]+|[a-z]+);/gi, (match, name: string) => {
    if (name[0] === "#") {
      const code = name[1] === "x" || name[1] === "X"
        ? Number.parseInt(name.slice(2), 16)
        : Number.parseInt(name.slice(1), 10);
      return Number.isFinite(code) ? String.fromCodePoint(code) : match;
    }
    return ENTITIES[name.toLowerCase()] ?? match;
  });
}

/** HTML 片段 → 阅读器可用的纯文本（保留段落换行，丢弃标签）。 */
export function htmlToText(html: string): string {
  const text = html
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/<(script|style|head)\b[\s\S]*?<\/\1>/gi, "")
    .replace(/<br\s*\/?>/gi, "\n")
    .replace(/<li\b[^>]*>/gi, "· ")
    .replace(/<\/(p|div|section|article|h[1-6]|li|tr|blockquote|pre)>/gi, "\n\n")
    .replace(/<[^>]+>/g, "");
  return decodeEntities(text)
    .replace(/\r\n?/g, "\n")
    .replace(/[\t\f\v\u00a0 ]+/g, " ")
    .replace(/ *\n */g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

function decode(bytes: Uint8Array): string {
  return new TextDecoder("utf-8").decode(bytes);
}

/** 读取标签里的属性；单双引号都支持。 */
function attr(tag: string, name: string): string | undefined {
  const match = new RegExp(`\\b${name}\\s*=\\s*("([^"]*)"|'([^']*)')`, "i").exec(tag);
  if (!match) return undefined;
  return decodeEntities(match[2] ?? match[3] ?? "");
}

function normalizePath(path: string): string {
  const parts: string[] = [];
  for (const part of path.replace(/\\/g, "/").split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      parts.pop();
      continue;
    }
    parts.push(part);
  }
  return parts.join("/");
}

function resolvePath(baseDir: string, href: string): string {
  const clean = href.split("#")[0].split("?")[0];
  let decoded = clean;
  try {
    decoded = decodeURIComponent(clean);
  } catch {
    // 非法转义：保留原样
  }
  if (decoded.startsWith("/")) return normalizePath(decoded);
  return normalizePath(baseDir ? `${baseDir}/${decoded}` : decoded);
}

function dirname(path: string): string {
  const index = path.lastIndexOf("/");
  return index < 0 ? "" : path.slice(0, index);
}

/** 压缩包内的路径大小写可能不一致；先精确匹配，再忽略大小写。 */
function findFile(files: Record<string, Uint8Array>, path: string): string | undefined {
  if (Object.prototype.hasOwnProperty.call(files, path)) return path;
  const target = normalizePath(path).toLowerCase();
  return Object.keys(files).find((name) => normalizePath(name).toLowerCase() === target);
}

// ── 正文 HTML 清洗 + 图片内联 ───────────────────────────────────

const IMAGE_TYPES: Record<string, string> = {
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  png: "image/png",
  gif: "image/gif",
  webp: "image/webp",
  svg: "image/svg+xml",
};

/** 单张图片上限：避免整本漫画式 EPUB 把内存打满。 */
const MAX_IMAGE_BYTES = 4 * 1024 * 1024;

const ALLOWED_TAGS = new Set([
  "p", "div", "span", "br", "hr",
  "h1", "h2", "h3", "h4", "h5", "h6",
  "strong", "b", "em", "i", "u", "s", "strike", "del", "ins", "small", "mark",
  "blockquote", "ul", "ol", "li", "dl", "dt", "dd",
  "table", "thead", "tbody", "tfoot", "tr", "td", "th", "caption",
  "img", "figure", "figcaption", "ruby", "rt", "rp", "sup", "sub",
  "pre", "code", "kbd", "samp", "var", "center",
]);

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/** 不依赖 Buffer / btoa，浏览器与 Node 都能用。 */
function toBase64(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const b0 = bytes[i];
    const b1 = i + 1 < bytes.length ? bytes[i + 1] : 0;
    const b2 = i + 2 < bytes.length ? bytes[i + 2] : 0;
    out += B64[b0 >> 2];
    out += B64[((b0 & 3) << 4) | (b1 >> 4)];
    out += i + 1 < bytes.length ? B64[((b1 & 15) << 2) | (b2 >> 6)] : "=";
    out += i + 2 < bytes.length ? B64[b2 & 63] : "=";
  }
  return out;
}

function escapeAttr(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

function imageMap(files: Record<string, Uint8Array>): Map<string, string> {
  const images = new Map<string, string>();
  for (const [name, bytes] of Object.entries(files)) {
    const ext = /\.([a-z0-9]+)$/i.exec(name)?.[1]?.toLowerCase();
    const type = ext ? IMAGE_TYPES[ext] : undefined;
    if (!type || bytes.length === 0 || bytes.length > MAX_IMAGE_BYTES) continue;
    images.set(normalizePath(name), `data:${type};base64,${toBase64(bytes)}`);
  }
  return images;
}

function lookupImage(images: Map<string, string>, path: string): string | undefined {
  const exact = images.get(path);
  if (exact) return exact;
  const target = path.toLowerCase();
  for (const [name, data] of images) {
    if (name.toLowerCase() === target) return data;
  }
  return undefined;
}

/**
 * XHTML → 阅读器可直接渲染的 HTML：
 *  - 移除脚本 / 样式 / 表单 / 媒体标签；
 *  - `<img>` 的相对路径解析到压缩包并转 data URL，找不到就删除；
 *  - 去掉事件、外链与内联样式，只保留语义标签。
 */
function sanitizeEpubHtml(raw: string, baseDir: string, images: Map<string, string>): string {
  let body = raw
    .replace(/<\?xml[\s\S]*?\?>/gi, "")
    .replace(/<!DOCTYPE[\s\S]*?>/gi, "")
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/<(script|style|head|iframe|object|embed|form|audio|video|canvas)\b[\s\S]*?<\/\1>/gi, "")
    .replace(/<(script|style|iframe|object|embed|form|input|button|link|meta|base)\b[^>]*>/gi, "");
  const bodyMatch = /<body\b[^>]*>([\s\S]*?)<\/body>/i.exec(body);
  if (bodyMatch) body = bodyMatch[1];
  body = body.replace(/<img\b[^>]*>/gi, (tag) => {
    const src = attr(tag, "src");
    if (!src) return "";
    if (/^data:/i.test(src)) return tag;
    const resolved = resolvePath(baseDir, src);
    const data = lookupImage(images, resolved);
    if (!data) return "";
    return `<img src="${data}" alt="${escapeAttr(attr(tag, "alt") ?? "")}">`;
  });
  body = body
    .replace(/\son\w+\s*=\s*("([^"]*)"|'([^']*)'|[^\s>]+)/gi, "")
    .replace(/\s(?:href|srcset|xlink:href)\s*=\s*("([^"]*)"|'([^']*)')/gi, "")
    .replace(/\sstyle\s*=\s*("([^"]*)"|'([^']*)')/gi, "");
  body = body.replace(/<\/?([a-z][a-z0-9]*)\b[^>]*>/gi, (tag, name: string) =>
    ALLOWED_TAGS.has(name.toLowerCase()) ? tag : "");
  return body.trim();
}

function firstText(xml: string, tag: string): string | undefined {
  const match = new RegExp(`<${tag}\\b[^>]*>([\\s\\S]*?)</${tag}>`, "i").exec(xml);
  if (!match) return undefined;
  const value = htmlToText(match[1]).replace(/\s+/g, " ").trim();
  return value || undefined;
}

function headingOf(html: string): string | undefined {
  const heading = /<h[1-3]\b[^>]*>([\s\S]*?)<\/h[1-3]>/i.exec(html)?.[1];
  const title = /<title\b[^>]*>([\s\S]*?)<\/title>/i.exec(html)?.[1];
  for (const raw of [heading, title]) {
    if (!raw) continue;
    const value = htmlToText(raw).split("\n")[0]?.trim();
    if (value) return value;
  }
  return undefined;
}

/** NCX 目录：`<text>` 与 `<content src>` 按出现顺序配对。 */
function ncxLabels(ncx: string, baseDir: string): Map<string, string> {
  const labels = new Map<string, string>();
  const texts = [...ncx.matchAll(/<text\b[^>]*>([\s\S]*?)<\/text>/gi)]
    .map((match) => htmlToText(match[1]).replace(/\s+/g, " ").trim());
  const sources = (ncx.match(/<content\b[^>]*>/gi) ?? [])
    .map((tag) => attr(tag, "src"))
    .filter((src): src is string => Boolean(src))
    .map((src) => resolvePath(baseDir, src));
  for (let i = 0; i < sources.length; i += 1) {
    const label = texts[i];
    if (label && !labels.has(sources[i])) labels.set(sources[i], label);
  }
  return labels;
}

/**
 * 解析 EPUB 字节流。失败一律抛 `parse`（坏包 / 缺 OPF / 无正文），
 * 调用方只需要展示错误文案。
 */
export function parseEpub(data: Uint8Array): EpubBook {
  let files: Record<string, Uint8Array>;
  try {
    files = unzipSync(data, {
      filter: (file) =>
        /\.(x?html?|opf|ncx|xml|jpe?g|png|gif|webp|svg)$/i.test(file.name),
    });
  } catch (error) {
    throw new RspError(
      "parse",
      `EPUB 不是合法 ZIP：${error instanceof Error ? error.message : String(error)}`,
      undefined,
    );
  }

  const containerKey = findFile(files, "META-INF/container.xml");
  let opfPath: string | undefined;
  if (containerKey) {
    const container = decode(files[containerKey]);
    const rootfile = /<rootfile\b[^>]*>/i.exec(container)?.[0];
    const fullPath = rootfile ? attr(rootfile, "full-path") : undefined;
    if (fullPath) opfPath = normalizePath(fullPath);
  }
  if (!opfPath || !findFile(files, opfPath)) {
    opfPath = Object.keys(files).find((name) => /\.opf$/i.test(name));
  }
  if (!opfPath) throw new RspError("parse", "EPUB 缺少 OPF 包文件", undefined);

  const opfKey = findFile(files, opfPath);
  if (!opfKey) throw new RspError("parse", "EPUB 的 OPF 路径无法解析", undefined);
  const opf = decode(files[opfKey]);
  const opfDir = dirname(opfKey);

  const title = firstText(opf, "dc:title") ?? firstText(opf, "title") ?? "EPUB";
  const author = firstText(opf, "dc:creator") ?? firstText(opf, "creator");
  const images = imageMap(files);

  const items = new Map<string, { href: string; type: string; properties: string }>();
  for (const tag of opf.match(/<item\b[^>]*>/gi) ?? []) {
    const id = attr(tag, "id");
    const href = attr(tag, "href");
    if (!id || !href) continue;
    items.set(id, {
      href: resolvePath(opfDir, href),
      type: attr(tag, "media-type") ?? "",
      properties: attr(tag, "properties") ?? "",
    });
  }

  const spineTag = /<spine\b[^>]*>/i.exec(opf)?.[0] ?? "";
  const spineBody = /<spine\b[^>]*>([\s\S]*?)<\/spine>/i.exec(opf)?.[1] ?? "";
  const idrefs = (spineBody.match(/<itemref\b[^>]*>/gi) ?? [])
    .filter((tag) => attr(tag, "linear") !== "no")
    .map((tag) => attr(tag, "idref"))
    .filter((id): id is string => Boolean(id));

  // NCX 只是标题来源；没有它也能靠 XHTML 标题继续。
  let labels = new Map<string, string>();
  const tocId = attr(spineTag, "toc");
  const tocItem = tocId ? items.get(tocId) : undefined;
  if (tocItem) {
    const tocKey = findFile(files, tocItem.href);
    if (tocKey) labels = ncxLabels(decode(files[tocKey]), dirname(tocKey));
  }

  const chapters: EpubChapter[] = [];
  for (const idref of idrefs) {
    const item = items.get(idref);
    if (!item) continue;
    if (item.properties.includes("nav") || item.properties.includes("cover")) continue;
    if (item.type && !/x?html/i.test(item.type)) continue;
    const key = findFile(files, item.href);
    if (!key) continue;
    const raw = decode(files[key]);
    const text = htmlToText(raw);
    const html = sanitizeEpubHtml(raw, dirname(key), images);
    // 纯图片章节（漫画 / 插图集）没有足够文本，但有 img 仍然要保留。
    if (text.length < 20 && !/<img\b/i.test(html)) continue;
    chapters.push({
      id: item.href,
      title: labels.get(item.href) ?? headingOf(raw) ?? `第 ${chapters.length + 1} 章`,
      text,
      html,
    });
  }

  if (chapters.length === 0) {
    throw new RspError("parse", "EPUB 没有可读章节", undefined);
  }

  return { title, author, chapters };
}
