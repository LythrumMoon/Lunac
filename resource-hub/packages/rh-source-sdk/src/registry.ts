import {
  RSP_VERSION,
  RspError,
  type DeclarativeRule,
  type SourceEngine,
  type SourceKind,
  type SourceManifest,
} from "./contract.js";

/**
 * 规则仓库（Registry）—— 把三个上游生态的「索引」聚合到同一份清单里。
 *
 * **重要边界**：
 *  - 这里只记录**上游地址与元数据**，不复制上游的规则 / 脚本内容；
 *    插件安装时再按 `sourceUrl` 读取（用户主动操作，正文只落在本机 ModuleData）。
 *  - `license` / `attribution` 必须保留，**不得**为了「看起来像自己的」而抹掉；
 *  - 聚合可以做的是：统一字段、去重、版本比较、补 warnings。
 */

export type Upstream = "manga" | "comic" | "video" | "manual";

/**
 * 旧版 id / upstream 迁移表。
 *
 * **只用于读取历史数据**：早期版本的 registry 与已装来源使用旧前缀，
 * 这里把它们归一成新前缀；新 id 一律由 `from*Index` 生成，不再引用这张表。
 */
const LEGACY_ID_PREFIXES: [string, string][] = [
  ["venera.", "comic."],
  ["kazumi.", "manga."],
  ["uzvideo.", "video."],
];

const LEGACY_UPSTREAM: Record<string, Upstream> = {
  venera: "comic",
  kazumi: "manga",
  uzvideo: "video",
};

/** 把旧版来源 id 归一成新前缀；已经是新前缀的原样返回。 */
export function normalizeLegacyId(id: string): string {
  const value = id.trim();
  for (const [old, next] of LEGACY_ID_PREFIXES) {
    if (value.startsWith(old)) return `${next}${value.slice(old.length)}`;
  }
  return value;
}

/** 把旧版 upstream 归一成新值；未知值按 `manual` 处理。 */
export function normalizeLegacyUpstream(value: string): Upstream {
  const key = value.trim();
  if (key in LEGACY_UPSTREAM) return LEGACY_UPSTREAM[key];
  return key === "manga" || key === "comic" || key === "video" || key === "manual"
    ? key
    : "manual";
}

export interface RegistryEntry {
  /** 全局 id：`<upstream>.<slug>`，与 RSP 清单 id 同规则。 */
  id: string;
  name: string;
  upstream: Upstream;
  /** 上游自己的 key / name / fileName。 */
  upstreamId: string;
  version: string;
  kind: SourceKind[];
  engine: SourceEngine;
  /** 上游规则 / 脚本地址（只链接，不内嵌）。 */
  sourceUrl?: string;
  homepage?: string;
  license?: string;
  attribution?: string;
  warnings: string[];
  /**
   * 自建（`upstream: "manual"`）来源的内嵌规则，便于离线安装。
   * 上游只放 `sourceUrl`，由插件在安装时按地址读取；
   * 本仓库不保存它们的规则 / 脚本正文。
   */
  source?: { manifest: SourceManifest; rule: DeclarativeRule };
}

export interface RegistryBundle {
  rsp: typeof RSP_VERSION;
  generatedAt: string;
  entries: RegistryEntry[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function str(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function slug(input: string): string {
  const ascii = input
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40);
  if (ascii) return ascii;
  let hash = 0x811c9dc5;
  for (let i = 0; i < input.length; i++) {
    hash ^= input.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return `source-${hash.toString(16).padStart(8, "0")}`;
}

function joinUrl(baseUrl: string, file: string): string | undefined {
  if (!file) return undefined;
  try {
    return new URL(file, baseUrl).toString();
  } catch {
    return undefined;
  }
}

function versionParts(version: string): number[] {
  return version
    .split(/[.+-]/)
    .map((part) => Number.parseInt(part, 10))
    .map((n) => (Number.isFinite(n) ? n : 0));
}

/** 数字段比较；`1.10.0 > 1.9.0`，非数字段按 0。 */
export function compareVersion(a: string, b: string): number {
  const left = versionParts(a);
  const right = versionParts(b);
  const length = Math.max(left.length, right.length);
  for (let i = 0; i < length; i++) {
    const diff = (left[i] ?? 0) - (right[i] ?? 0);
    if (diff !== 0) return diff;
  }
  return 0;
}

/** 漫画索引 → RegistryEntry[]。 */
export function fromMangaIndex(
  raw: unknown,
  baseUrl: string,
  attribution = "KazumiRules",
): RegistryEntry[] {
  if (!Array.isArray(raw)) throw new RspError("parse", "漫画索引必须是数组");
  const entries: RegistryEntry[] = [];
  for (const item of raw) {
    if (!isRecord(item)) continue;
    const name = str(item.name);
    if (!name) continue;
    const fileName = str(item.fileName) || `${name}.json`;
    entries.push({
      id: `manga.${slug(name)}`,
      name,
      upstream: "manga",
      upstreamId: name,
      version: str(item.version) || "0.0.0",
      kind: ["anime"],
      engine: "declarative",
      sourceUrl: joinUrl(baseUrl, fileName),
      homepage: baseUrl,
      license: "MIT",
      attribution,
      warnings: str(item.author) ? [`上游作者：${str(item.author)}`] : [],
    });
  }
  return entries;
}

/** 漫画源索引 → RegistryEntry[]。 */
export function fromComicIndex(
  raw: unknown,
  baseUrl: string,
  attribution = "Venera comic sources",
): RegistryEntry[] {
  if (!Array.isArray(raw)) throw new RspError("parse", "漫画索引必须是数组");
  const entries: RegistryEntry[] = [];
  for (const item of raw) {
    if (!isRecord(item)) continue;
    const name = str(item.name);
    const key = str(item.key) || name;
    if (!name || !key) continue;
    const remote = str(item.url);
    const local = str(item.fileName);
    entries.push({
      id: `comic.${slug(key)}`,
      name,
      upstream: "comic",
      upstreamId: key,
      version: str(item.version) || "0.0.0",
      kind: ["comic"],
      engine: "js",
      sourceUrl: remote || joinUrl(baseUrl, local),
      homepage: baseUrl,
      // Venera 本体是 GPL-3.0；单个源脚本的许可以上游文件为准，默认按「未声明」处理。
      license: undefined,
      attribution,
      warnings: [
        "安装时从上游地址读取 Venera 脚本",
        "单个源脚本的许可以上游文件为准",
      ],
    });
  }
  return entries;
}

/**
 * 视频扩展索引 → RegistryEntry[]。
 *
 * 视频扩展没有稳定的官方 index（扩展以 `uzAio.zip` 分发），所以这里接受一份
 * **用户/构建脚本整理出的条目数组**，字段尽量宽松。
 */
export function fromVideoIndex(
  raw: unknown,
  baseUrl: string,
  attribution = "uzVideo extensions",
): RegistryEntry[] {
  if (!Array.isArray(raw)) throw new RspError("parse", "视频索引必须是数组");
  const entries: RegistryEntry[] = [];
  for (const item of raw) {
    if (!isRecord(item)) continue;
    const name = str(item.name);
    if (!name) continue;
    const remote = str(item.url);
    const local = str(item.fileName);
    entries.push({
      id: `video.${slug(str(item.key) || name)}`,
      name,
      upstream: "video",
      upstreamId: str(item.key) || name,
      version: str(item.version) || "0.0.0",
      kind: ["anime"],
      engine: "js",
      sourceUrl: remote || joinUrl(baseUrl, local),
      homepage: baseUrl,
      // 上游仓库未见 LICENSE ⇒ 默认保留所有权利；只在用户安装时按地址读取。
      license: undefined,
      attribution,
      warnings: [
        "安装时从上游地址读取 uzVideo 脚本",
        "上游未见 LICENSE；使用前请自行确认授权",
      ],
    });
  }
  return entries;
}

/** 采集站默认 UA（宿主网络层不代填 UA）。 */
const CMS_USER_AGENT =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124 Safari/537.36";

/**
 * Apple CMS v10 采集接口索引 → RegistryEntry[]（2026-10-21）。
 *
 * 条目由本仓库生成：规则正文内嵌（只有接口地址），安装后直接用声明式执行器
 * 的 `cms` 分支完成搜索 / 总览 / 剧集 / 播放，不再依赖上游脚本。
 */
export function fromCmsIndex(
  raw: unknown,
  attribution = "公开 TVBox / LibreTV 配置聚合",
): RegistryEntry[] {
  if (!Array.isArray(raw)) throw new RspError("parse", "CMS 索引必须是数组");
  const entries: RegistryEntry[] = [];
  for (const item of raw) {
    if (!isRecord(item)) continue;
    const name = str(item.name);
    const api = str(item.api);
    if (!name || !/^https?:\/\//i.test(api)) continue;
    let origin: string | undefined;
    try {
      origin = new URL(api).origin;
    } catch {
      continue;
    }
    const id = `cms.${slug(str(item.key) || name)}`;
    const version = str(item.version) || "1.0.0";
    const homepage = str(item.homepage) || origin;
    entries.push({
      id,
      name,
      upstream: "manual",
      upstreamId: api,
      version,
      kind: ["tv"],
      engine: "declarative",
      homepage,
      attribution,
      warnings: [
        "自建声明式 CMS 规则：只调用公开采集接口，不保存内容",
        "接口地址与可用性随上游变化，失效时在来源管理中移除",
      ],
      source: {
        manifest: {
          rsp: RSP_VERSION,
          id,
          name,
          version,
          kind: ["tv"],
          engine: "declarative",
          entry: "rule.json",
          baseUrl: origin,
          homepage,
          // 采集站普遍拒绝空 UA；宿主网络层不代填，这里显式给一个浏览器 UA。
          headers: { "User-Agent": CMS_USER_AGENT },
          capabilities: ["search", "explore", "chapters", "play"],
        },
        rule: { cms: { api } },
      },
    });
  }
  return entries;
}

/**
 * 合并三类索引：全局 id 去重、版本高者胜、warnings 合并。
 *
 * 传 `generatedAt` 是为了让测试可复现；生产调用可以省略。
 */
export function mergeRegistry(
  entries: RegistryEntry[],
  generatedAt = new Date().toISOString(),
): RegistryBundle {
  const byId = new Map<string, RegistryEntry>();
  for (const entry of entries) {
    if (!entry.id || !entry.name) continue;
    const current = byId.get(entry.id);
    if (!current) {
      byId.set(entry.id, { ...entry, warnings: [...entry.warnings] });
      continue;
    }
    const newer = compareVersion(entry.version, current.version) > 0 ? entry : current;
    const older = newer === entry ? current : entry;
    byId.set(entry.id, {
      ...newer,
      warnings: [...new Set([...older.warnings, ...newer.warnings])],
    });
  }
  return {
    rsp: RSP_VERSION,
    generatedAt,
    entries: [...byId.values()].sort((a, b) => a.name.localeCompare(b.name)),
  };
}

/** 读取并校验一份 registry bundle（插件 / AI 工具共用）。 */
export function parseRegistry(raw: unknown): RegistryBundle {
  if (!isRecord(raw)) throw new RspError("parse", "registry 必须是对象");
  if (raw.rsp !== RSP_VERSION) throw new RspError("parse", `registry.rsp 必须是 ${RSP_VERSION}`);
  if (!Array.isArray(raw.entries)) throw new RspError("parse", "registry.entries 必须是数组");
  const entries: RegistryEntry[] = [];
  for (const item of raw.entries) {
    if (!isRecord(item)) continue;
    const id = normalizeLegacyId(str(item.id));
    const name = str(item.name);
    if (!id || !name) continue;
    const embedded = item.source;
    const source =
      isRecord(embedded) && isRecord(embedded.manifest) && isRecord(embedded.rule)
        ? {
            manifest: {
              ...(embedded.manifest as unknown as SourceManifest),
              id: normalizeLegacyId(str(embedded.manifest.id) || id),
            },
            rule: embedded.rule as unknown as DeclarativeRule,
          }
        : undefined;
    entries.push({
      id,
      name,
      upstream: normalizeLegacyUpstream(str(item.upstream) || "manual"),
      upstreamId: str(item.upstreamId) || id,
      version: str(item.version) || "0.0.0",
      kind: Array.isArray(item.kind) ? (item.kind as SourceKind[]) : ["anime"],
      engine: (str(item.engine) as SourceEngine) || "declarative",
      sourceUrl: str(item.sourceUrl) || undefined,
      homepage: str(item.homepage) || undefined,
      license: str(item.license) || undefined,
      attribution: str(item.attribution) || undefined,
      warnings: Array.isArray(item.warnings) ? item.warnings.map(String) : [],
      source,
    });
  }
  return {
    rsp: RSP_VERSION,
    generatedAt: str(raw.generatedAt) || new Date().toISOString(),
    entries,
  };
}
