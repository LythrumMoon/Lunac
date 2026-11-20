// ── 主控芯片 ↔ 插件芯片：单信道协议（纯逻辑）────────────────────────
//
// 协议只做四件事：信封校验、版本协商、能力校验、超时 / 错误码。
// 不碰 DOM / postMessage / Tauri —— 宿主与 iframe 客户端都依赖这一份，
// 所以它必须能在 vitest 里直接跑（见 `protocol.test.ts`）。
//
// 设计底稿：`docs/todo.md` §待办 3-10 C 节。信道版本 `v` 与宿主桥
// `apiVersion` 是两条独立的版本线，不得合并。

export const CHANNEL_VERSION = 1 as const;
/** 宿主桥能力版本；信道落地时由 2 升到 3。 */
export const API_VERSION = 3 as const;

export type ChannelMessageType = "req" | "res" | "err" | "event";

export interface ChannelEnvelope {
  v: number;
  id: string;
  type: ChannelMessageType;
  method: string;
  payload?: unknown;
  ts?: number;
}

export type ChannelErrorCode =
  | "E_TIMEOUT"
  | "E_DENIED"
  | "E_UNSUPPORTED"
  | "E_VERSION"
  | "E_INVALID"
  | "E_NOT_READY"
  | "E_LIMIT"
  | "E_CANCELLED"
  | "E_HOST";

export class ChannelError extends Error {
  readonly code: ChannelErrorCode;

  constructor(code: ChannelErrorCode, message: string) {
    super(message);
    this.name = "ChannelError";
    this.code = code;
  }
}

const MESSAGE_TYPES: readonly ChannelMessageType[] = ["req", "res", "err", "event"];

/** 信道允许的 method 全集。未知 method 一律 `E_UNSUPPORTED`，不静默放行。 */
export const CHANNEL_METHODS = [
  // 宿主 → 插件
  "hello",
  "mount",
  "execute",
  "theme.changed",
  "locale.changed",
  "host.resize",
  "visibility.changed",
  "lifecycle",
  "ping",
  // 插件 → 宿主
  "ready",
  "ui.resize",
  "ui.status",
  "ui.notify",
  "net.fetch",
  "fs.pick",
  "fs.read",
  "fs.write",
  "clipboard.read",
  "clipboard.write",
  "screenshot.capture",
  "shell.openUrl",
  "process.start",
  "process.stop",
  "process.request",
  "process.http",
  "log",
  "bye",
  "cancel",
] as const;

export type ChannelMethod = (typeof CHANNEL_METHODS)[number];

const METHOD_SET = new Set<string>(CHANNEL_METHODS);

/** method → 清单里必须声明的能力；`undefined` = 基础方法，不需要额外能力。 */
const METHOD_CAPABILITY: Record<string, string> = {
  "ui.layout.takeover": "layout.takeover",
  "ui.window.float": "window.float",
  "ui.window.resize": "window.resize",
  "ui.notify": "notify",
  "net.fetch": "net.fetch",
  "fs.pick": "fs.pick",
  "fs.read": "fs.read",
  "fs.write": "fs.write",
  "clipboard.read": "clipboard.read",
  "clipboard.write": "clipboard.write",
  "screenshot.capture": "screenshot.capture",
  "shell.openUrl": "shell.openUrl",
  "process.start": "process.spawn",
  "process.stop": "process.spawn",
  "process.request": "process.spawn",
  "process.http": "net.sidecar.http",
};

/** iframe 引擎明确不支持的方法（`reuse` 依赖与宿主同 DOM）。 */
export const IFRAME_UNSUPPORTED = new Set<string>(["ui.reuse"]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isStructuredCloneable(value: unknown, seen: WeakSet<object>, depth: number): boolean {
  if (value === null || value === undefined) return true;
  const kind = typeof value;
  if (kind === "string" || kind === "number" || kind === "boolean" || kind === "bigint") return true;
  if (kind === "function" || kind === "symbol") return false;
  if (kind !== "object") return false;
  if (depth > 32) return false;

  const object = value as object;
  if (seen.has(object)) return true;
  // DOM 节点 / Window / Promise / WeakMap 等不可结构化克隆
  if ("nodeType" in (object as Record<string, unknown>)) return false;
  if (value instanceof Promise || value instanceof WeakMap || value instanceof WeakSet) return false;
  if (value instanceof Date || value instanceof RegExp) return true;
  if (ArrayBuffer.isView(value) || value instanceof ArrayBuffer) return true;

  seen.add(object);
  if (Array.isArray(value)) {
    return value.every((item) => isStructuredCloneable(item, seen, depth + 1));
  }
  return Object.values(value as Record<string, unknown>).every((item) =>
    isStructuredCloneable(item, seen, depth + 1),
  );
}

/** 解析并校验一封信。坏信封返回 `null`（调用方回 `E_INVALID`）。 */
export function parseEnvelope(raw: unknown): ChannelEnvelope | null {
  if (!isRecord(raw)) return null;
  const { v, id, type, method, payload, ts } = raw;
  if (typeof v !== "number" || !Number.isInteger(v) || v <= 0) return null;
  if (typeof id !== "string" || id.length === 0 || id.length > 128) return null;
  if (typeof type !== "string" || !MESSAGE_TYPES.includes(type as ChannelMessageType)) return null;
  if (typeof method !== "string" || method.length === 0 || method.length > 64) return null;
  if (ts !== undefined && (typeof ts !== "number" || !Number.isFinite(ts))) return null;
  if (!isStructuredCloneable(payload, new WeakSet(), 0)) return null;
  return { v, id, type: type as ChannelMessageType, method, payload, ts };
}

/** 选双方都支持的最高信道版本；无交集返回 `null`（宿主回 `E_VERSION`）。 */
export function negotiateChannelVersion(
  hostVersions: readonly number[],
  pluginVersions: readonly number[],
): number | null {
  const plugin = new Set(pluginVersions.filter((v) => Number.isInteger(v) && v > 0));
  const common = hostVersions.filter((v) => Number.isInteger(v) && v > 0 && plugin.has(v));
  return common.length > 0 ? Math.max(...common) : null;
}

/** 校验一条插件 → 宿主请求：未知方法 / iframe 禁用 / 缺能力。 */
export function checkMethod(
  method: string,
  permissions: readonly string[],
  engine: "dom" | "iframe" = "dom",
): ChannelError | null {
  if (!METHOD_SET.has(method)) {
    return new ChannelError("E_UNSUPPORTED", `未知方法：${method}`);
  }
  if (engine === "iframe" && IFRAME_UNSUPPORTED.has(method)) {
    return new ChannelError("E_UNSUPPORTED", `iframe 引擎不支持 ${method}`);
  }
  const required = METHOD_CAPABILITY[method];
  if (required && !permissions.includes(required)) {
    return new ChannelError("E_DENIED", `未声明能力：${required}`);
  }
  return null;
}

/** 请求超时；超时后回 `E_TIMEOUT`，不无限等待。 */
export function withTimeout<T>(promise: Promise<T>, ms: number, method: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new ChannelError("E_TIMEOUT", `${method} 超时（${ms}ms）`));
    }, ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

/** `source` / `origin` 双校验；`targetOrigin="*"` 永远不通过。 */
export function isTrustedSource(
  source: unknown,
  expectedWindow: unknown,
  origin: string,
  expectedOrigin: string,
): boolean {
  if (!expectedWindow || source !== expectedWindow) return false;
  if (!expectedOrigin || expectedOrigin === "*") return false;
  return origin === expectedOrigin;
}

/** `ui.resize` 合并：只保留最后一次，按间隔 flush；宿主侧再 clamp。 */
export class ResizeCoalescer {
  private pending: { height: number; width?: number } | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly intervalMs: number,
    private readonly flush: (size: { height: number; width?: number }) => void,
  ) {}

  push(height: number, width?: number): void {
    this.pending = { height, width };
    if (this.timer) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      const size = this.pending;
      this.pending = null;
      if (size) this.flush(size);
    }, this.intervalMs);
  }

  dispose(): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.pending = null;
  }
}

export function makeEnvelope(
  type: ChannelMessageType,
  method: string,
  id: string,
  payload?: unknown,
): ChannelEnvelope {
  return { v: CHANNEL_VERSION, id, type, method, payload, ts: Date.now() };
}

export function makeError(id: string, method: string, error: ChannelError): ChannelEnvelope {
  return makeEnvelope("err", method, id, { code: error.code, message: error.message });
}
