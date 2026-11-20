import {
  RSP_VERSION,
  RspError,
  type RspCapability,
  type SourceEngine,
  type SourceKind,
  type SourceManifest,
} from "./contract.js";

const ID_RE = /^[a-z0-9._-]+$/;
const KINDS: readonly SourceKind[] = ["comic", "novel", "anime", "tv"];
const ENGINES: readonly SourceEngine[] = ["declarative", "js", "builtin"];
const CAPABILITIES: readonly RspCapability[] = [
  "search", "explore", "detail", "chapters", "pages", "content", "play", "auth",
];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function fail(message: string): never {
  throw new RspError("parse", message);
}

/**
 * 校验并归一化一份来源清单。
 *
 * 口径：**能静态判定的错误一律拒绝**（与 Lunac 插件清单同一条纪律）。
 * 不替来源猜默认值 —— 缺 `id` / `name` / `kind` / `capabilities` 直接报错。
 */
export function validateSourceManifest(raw: unknown): SourceManifest {
  if (!isRecord(raw)) fail("清单必须是对象");
  if (raw.rsp !== RSP_VERSION) fail(`rsp 必须是 ${RSP_VERSION}`);

  const id = typeof raw.id === "string" ? raw.id : "";
  if (!id || !ID_RE.test(id) || id.startsWith(".")) {
    fail("id 只允许 [a-z0-9._-]，且不能以 . 开头");
  }
  if (id.length > 48) fail("id 超过 48 字符");

  const name = typeof raw.name === "string" ? raw.name.trim() : "";
  if (!name) fail("name 不能为空");

  const version = typeof raw.version === "string" ? raw.version.trim() : "";
  if (!version) fail("version 不能为空");

  if (!Array.isArray(raw.kind) || raw.kind.length === 0) fail("kind 至少一项");
  const kind: SourceKind[] = [];
  for (const item of raw.kind) {
    if (typeof item !== "string" || !KINDS.includes(item as SourceKind)) {
      fail(`kind 不支持：${String(item)}`);
    }
    if (!kind.includes(item as SourceKind)) kind.push(item as SourceKind);
  }

  if (typeof raw.engine !== "string" || !ENGINES.includes(raw.engine as SourceEngine)) {
    fail(`engine 不支持：${String(raw.engine)}`);
  }

  const entry = typeof raw.entry === "string" ? raw.entry.trim() : "";
  if (!entry) fail("entry 不能为空");
  if (entry.startsWith("/") || entry.includes("..") || entry.includes("\\")) {
    fail("entry 必须是来源包内的相对路径（.js / .mjs / .json）");
  }
  if (!/\.(js|mjs|json)$/i.test(entry)) fail("entry 只接受 .js / .mjs / .json");

  if (!Array.isArray(raw.capabilities) || raw.capabilities.length === 0) {
    fail("capabilities 至少一项");
  }
  const capabilities: RspCapability[] = [];
  for (const item of raw.capabilities) {
    if (typeof item !== "string" || !CAPABILITIES.includes(item as RspCapability)) {
      fail(`capabilities 不支持：${String(item)}`);
    }
    if (!capabilities.includes(item as RspCapability)) {
      capabilities.push(item as RspCapability);
    }
  }

  const headers = raw.headers === undefined ? undefined : readStringMap(raw.headers, "headers");
  const settings = raw.settings === undefined ? undefined : readStringMap(raw.settings, "settings");
  const rateLimit = readRateLimit(raw.rateLimit);

  return {
    rsp: RSP_VERSION,
    id,
    name,
    version,
    kind,
    engine: raw.engine as SourceEngine,
    entry,
    baseUrl: readOptionalString(raw.baseUrl, "baseUrl"),
    homepage: readOptionalString(raw.homepage, "homepage"),
    license: readOptionalString(raw.license, "license"),
    minHost: readOptionalString(raw.minHost, "minHost"),
    headers,
    settings,
    capabilities,
    rateLimit,
    nsfw: raw.nsfw === true,
  };
}

function readOptionalString(value: unknown, field: string): string | undefined {
  if (value === undefined || value === null || value === "") return undefined;
  if (typeof value !== "string") fail(`${field} 必须是字符串`);
  return value;
}

function readStringMap(value: unknown, field: string): Record<string, string> {
  if (!isRecord(value)) fail(`${field} 必须是对象`);
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(value)) {
    if (typeof v !== "string") fail(`${field}.${k} 必须是字符串`);
    out[k] = v;
  }
  return out;
}

function readRateLimit(value: unknown): { rps: number; burst: number } | undefined {
  if (value === undefined || value === null) return undefined;
  if (!isRecord(value)) fail("rateLimit 必须是对象");
  const rps = Number(value.rps);
  const burst = Number(value.burst);
  if (!Number.isFinite(rps) || rps <= 0) fail("rateLimit.rps 必须是正数");
  if (!Number.isFinite(burst) || burst <= 0) fail("rateLimit.burst 必须是正数");
  return { rps, burst };
}
