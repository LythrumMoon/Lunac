// 声明式规则的求值原语。
//
// 只做「纯函数」：模板 / 正则 / JSONPath / 选择器抽取。
// 网络由宿主 API v3 负责（见 `host.ts`），这里不碰 fetch。

import {
  RspError,
  type FieldExtractor,
  type HttpRequestTemplate,
  type ListRule,
  type Selector,
} from "./contract.js";

export type RuleVars = Record<string, string | number | undefined>;

const PLACEHOLDER = /@([A-Za-z_][\w]*)/g;

/** 替换 `@keyword` / `@page` / `@value` 等占位符；未知占位符原样保留，便于排错。 */
export function interpolate(template: string, vars: RuleVars): string {
  return template.replace(PLACEHOLDER, (match, key: string) => {
    const value = vars[key];
    return value === undefined ? match : String(value);
  });
}

/** 支持 `/pattern/flags` 或裸 pattern；返回第一个捕获组，无捕获组时返回整段匹配。 */
export function applyRegex(value: string, pattern: string): string | undefined {
  const literal = /^\/(.*)\/([a-z]*)$/s.exec(pattern);
  const source = literal ? literal[1] : pattern;
  const flags = literal ? literal[2] : "";
  let regex: RegExp;
  try {
    regex = new RegExp(source, flags);
  } catch (error) {
    throw new RspError(
      "parse",
      `正则无效：${pattern} → ${error instanceof Error ? error.message : String(error)}`,
      undefined,
    );
  }
  const match = regex.exec(value);
  if (!match) return undefined;
  return match[1] ?? match[0];
}

/** 相对路径按来源 `baseUrl` 解析，再拼 query；绝对 URL 原样保留。 */
export function buildRequestUrl(
  baseUrl: string | undefined,
  request: HttpRequestTemplate,
  vars: RuleVars,
): string {
  const raw = interpolate(request.url, vars);
  let url: URL;
  if (/^https?:\/\//i.test(raw)) {
    url = new URL(raw);
  } else {
    if (!baseUrl) {
      throw new RspError("parse", `相对地址缺少 baseUrl：${request.url}`, undefined);
    }
    const base = baseUrl.endsWith("/") ? baseUrl : `${baseUrl}/`;
    url = new URL(raw.replace(/^\//, ""), base);
  }
  for (const [key, value] of Object.entries(request.query ?? {})) {
    const resolved = interpolate(value, vars);
    if (resolved !== "") url.searchParams.set(key, resolved);
  }
  return url.toString();
}

type PathToken = string | number | "*";

function parseJsonPath(path: string): PathToken[] {
  const tokens: PathToken[] = [];
  let i = path.startsWith("$") ? 1 : 0;
  while (i < path.length) {
    const ch = path[i];
    if (ch === ".") {
      i += 1;
      if (path[i] === ".") {
        throw new RspError("unsupported", `暂不支持递归 JSONPath：${path}`, undefined);
      }
      const start = i;
      while (i < path.length && !".[]".includes(path[i])) i += 1;
      const key = path.slice(start, i);
      if (!key) throw new RspError("parse", `JSONPath 段为空：${path}`, undefined);
      tokens.push(key === "*" ? "*" : key);
      continue;
    }
    if (ch === "[") {
      const end = path.indexOf("]", i);
      if (end < 0) throw new RspError("parse", `JSONPath 缺少 ]：${path}`, undefined);
      const inner = path.slice(i + 1, end).trim();
      if (inner === "*") tokens.push("*");
      else if (/^\d+$/.test(inner)) tokens.push(Number(inner));
      else if (/^['"].*['"]$/.test(inner)) tokens.push(inner.slice(1, -1));
      else throw new RspError("unsupported", `不支持的 JSONPath 下标：${inner}`, undefined);
      i = end + 1;
      continue;
    }
    const start = i;
    while (i < path.length && !".[]".includes(path[i])) i += 1;
    const key = path.slice(start, i);
    if (!key) throw new RspError("parse", `JSONPath 无法解析：${path}`, undefined);
    tokens.push(key === "*" ? "*" : key);
  }
  return tokens;
}

/** JSONPath 子集：`$` / `a.b` / `[0]` / `[*]` / `['key']`；返回所有命中值。 */
export function jsonPathValues(value: unknown, path: string): unknown[] {
  let current: unknown[] = [value];
  for (const token of parseJsonPath(path)) {
    const next: unknown[] = [];
    for (const item of current) {
      if (token === "*") {
        if (Array.isArray(item)) next.push(...item);
        else if (item && typeof item === "object") next.push(...Object.values(item));
        continue;
      }
      if (typeof token === "number") {
        if (Array.isArray(item) && token >= 0 && token < item.length) next.push(item[token]);
        continue;
      }
      if (item && typeof item === "object" && Object.prototype.hasOwnProperty.call(item, token)) {
        next.push((item as Record<string, unknown>)[token]);
      }
    }
    current = next;
    if (current.length === 0) break;
  }
  return current;
}

export function jsonPathFirst(value: unknown, path: string): unknown {
  return jsonPathValues(value, path)[0];
}

function stringify(value: unknown): string {
  if (value === undefined || value === null) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return JSON.stringify(value);
}

function readJsonField(node: unknown, field: FieldExtractor, vars: RuleVars): string {
  const path = field.selector.jsonPath;
  if (!path) {
    throw new RspError("unsupported", "JSON 字段缺少 jsonPath", undefined);
  }
  let value = stringify(jsonPathFirst(node, path));
  if (field.regex) value = applyRegex(value, field.regex) ?? "";
  if (value !== "" && field.template) value = interpolate(field.template, { ...vars, value });
  return value;
}

/** JSON 列表（API 型来源）：`rule.list.jsonPath` 取节点，再按字段抽取。 */
export function extractJsonRows(
  root: unknown,
  rule: ListRule,
  vars: RuleVars = {},
): Record<string, string>[] {
  const listPath = rule.list.jsonPath;
  if (!listPath) {
    throw new RspError("unsupported", "JSON 列表规则缺少 list.jsonPath", undefined);
  }
  return jsonPathValues(root, listPath)
    .map((node) => {
      const row: Record<string, string> = {};
      for (const [key, field] of Object.entries(rule.fields)) {
        row[key] = readJsonField(node, field, vars);
      }
      return row;
    })
    .filter((row) => Object.values(row).some((value) => value !== ""));
}

/** XPath 在元素上下文里要按「相对当前节点」求值；`//x` → `.//x`。 */
function relativeXPath(xpath: string): string {
  const expr = xpath.trim();
  if (expr.startsWith(".")) return expr;
  if (expr.startsWith("//")) return `.${expr}`;
  if (expr.startsWith("/")) return `.${expr}`;
  return `./${expr}`;
}

function evaluateXPath(root: ParentNode, xpath: string): Node[] {
  const isDocument = typeof Document !== "undefined" && root instanceof Document;
  const owner = isDocument ? (root as Document) : root.ownerDocument;
  if (!owner || typeof owner.evaluate !== "function") {
    throw new RspError("unsupported", "当前运行时没有 XPath 支持", undefined);
  }
  const context: Node = isDocument ? (root as Document) : root;
  const expression = isDocument ? xpath.trim() : relativeXPath(xpath);
  const result = owner.evaluate(
    expression,
    context,
    null,
    XPathResult.ORDERED_NODE_SNAPSHOT_TYPE,
    null,
  );
  const nodes: Node[] = [];
  for (let i = 0; i < result.snapshotLength; i += 1) {
    const node = result.snapshotItem(i);
    if (node) nodes.push(node);
  }
  return nodes;
}

/**
 * CSS / XPath 选择器 → 节点列表（含文本 / 属性节点）。
 *
 * 字段抽取必须走这里：Kazumi 规则常用 `//div[2]/text()`，只收元素节点会把标题丢光。
 */
export function selectNodes(root: ParentNode, selector: Selector): Node[] {
  if (selector.css) {
    return [...root.querySelectorAll(selector.css)];
  }
  if (!selector.xpath) return [];
  return evaluateXPath(root, selector.xpath);
}

/** 只要元素节点（列表容器）；XPath 在元素上下文里按相对路径求值。 */
export function selectElements(root: ParentNode, selector: Selector): Element[] {
  return selectNodes(root, selector).filter(
    (node): node is Element => node.nodeType === 1,
  );
}

function nodeText(node: Node): string {
  // 文本 / CDATA / 属性节点直接取 nodeValue；元素取 textContent。
  if (node.nodeType === 1) return (node as Element).textContent ?? "";
  return node.nodeValue ?? "";
}

function readHtmlField(node: Element, field: FieldExtractor, vars: RuleVars): string {
  const target = selectNodes(node, field.selector)[0];
  if (!target) return "";
  let value: string;
  if (field.attr) {
    value =
      target.nodeType === 1
        ? ((target as Element).getAttribute(field.attr) ?? "")
        : "";
  } else {
    value = nodeText(target);
  }
  value = value.trim();
  if (field.regex) value = applyRegex(value, field.regex) ?? "";
  if (value !== "" && field.template) value = interpolate(field.template, { ...vars, value });
  return value;
}

/** 分类 / 标签链接的 href 判据（2026-10-21）：卡片里常见的「近期更新 / 类型」链接。 */
const TAG_LINK_RE = /(?:^|\/)(?:tags?|type|types|genre|genres|category|categories|vodtype)(?:[/\-_.?=]|$)/i;

/**
 * 规则没声明 tags 时，从卡片节点里兜底收集标签。
 *
 * 上游（Kazumi / Venera）的搜索卡片经常在卡片内放一条分类链接
 * （`/vodtype/1.html`、`/tag/热血`、文字如「近期更新」），但规则只抽
 * id / title。这里把它们收成 `tags`，筛选抽屉即可按「每个源自己的标签」分组。
 */
function collectHtmlRowTags(node: Element): string[] {
  const out = new Set<string>();
  for (const anchor of node.querySelectorAll("a[href], .tag, .tags, [class*='tag']")) {
    const href = anchor.getAttribute("href") ?? "";
    const classLike = anchor.tagName === "A" ? "" : (anchor.getAttribute("class") ?? "");
    if (!TAG_LINK_RE.test(href) && !/tag/i.test(classLike)) continue;
    const text = (anchor.textContent ?? "").trim();
    if (!text || text.length > 16) continue;
    out.add(text);
    if (out.size >= 4) break;
  }
  return [...out];
}

/** HTML 列表（网页型来源）：列表节点内逐字段抽取。 */
export function extractHtmlRows(
  root: ParentNode,
  rule: ListRule,
  vars: RuleVars = {},
): Record<string, string>[] {
  return selectElements(root, rule.list)
    .map((node) => {
      const row: Record<string, string> = {};
      for (const [key, field] of Object.entries(rule.fields)) {
        row[key] = readHtmlField(node, field, vars);
      }
      if (!row.tags) {
        const tags = collectHtmlRowTags(node);
        if (tags.length > 0) row.tags = tags.join(",");
      }
      return row;
    })
    .filter((row) => Object.values(row).some((value) => value !== ""));
}
