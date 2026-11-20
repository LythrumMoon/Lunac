// 来源网络客户端：缓存 / 限速 / 取消 / 默认超时。
//
// 只包住 `NetApi`，不碰 DOM / Node / Tauri：Lunac 插件、独立应用与测试共用。
// · 缓存：只缓存 GET / HEAD 的成功响应，按 TTL + LRU 控制；
// · 限速：来源清单 `rateLimit` 的令牌桶，按来源实例隔离；
// · 取消：`AbortSignal` 在排队、请求前与请求中都会检查；宿主命令本身不可中断，
//   但调用方会立刻拿到「已取消」，迟到结果被丢弃；
// · 超时：未显式给 `timeoutMs` 时补 8s（RSP 约定）。

import { RspError } from "./contract.js";
import type { HostRequestInit, HostResponse, NetApi } from "./host.js";

export interface NetClientOptions {
  /** GET / HEAD 的成功响应缓存时长；0 = 关闭（默认 30s）。 */
  cacheTtlMs?: number;
  /** 缓存条数上限（默认 64）。 */
  cacheMaxEntries?: number;
  /** 来源清单里的 `rateLimit`；缺省 = 不限速。 */
  rateLimit?: { rps: number; burst: number };
  /** 单次请求默认超时（默认 8000ms）。 */
  timeoutMs?: number;
  /** 测试注入：当前时间（毫秒）。 */
  now?: () => number;
  /** 测试注入：等待（毫秒），必须可被 signal 打断。 */
  sleep?: (ms: number, signal?: AbortSignal) => Promise<void>;
}

export interface NetClient extends NetApi {
  /** 清空该来源的响应缓存。 */
  clearCache(): void;
}

interface CacheEntry {
  expiresAt: number;
  response: HostResponse;
}

function abortError(): RspError {
  return new RspError("network", "请求已取消", undefined);
}

function throwIfAborted(signal?: AbortSignal): void {
  if (signal?.aborted) throw abortError();
}

/** 默认等待：可被 AbortSignal 打断。 */
function defaultSleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(abortError());
      return;
    }
    let timer: ReturnType<typeof setTimeout>;
    const onAbort = (): void => {
      clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
      reject(abortError());
    };
    timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

/** 用调用方的 signal 包住一个不可中断的 Promise。 */
function withAbort<T>(promise: Promise<T>, signal?: AbortSignal): Promise<T> {
  if (!signal) return promise;
  if (signal.aborted) return Promise.reject(abortError());
  return new Promise<T>((resolve, reject) => {
    const onAbort = (): void => {
      signal.removeEventListener("abort", onAbort);
      reject(abortError());
    };
    signal.addEventListener("abort", onAbort, { once: true });
    promise.then(
      (value) => {
        signal.removeEventListener("abort", onAbort);
        resolve(value);
      },
      (error: unknown) => {
        signal.removeEventListener("abort", onAbort);
        reject(error);
      },
    );
  });
}

function stableHeaders(headers: Record<string, string> | undefined): string {
  if (!headers) return "";
  return Object.keys(headers)
    .sort()
    .map((key) => `${key.toLowerCase()}=${headers[key]}`)
    .join("\n");
}

function cacheKey(init: HostRequestInit, method: string): string {
  return [method, init.url, stableHeaders(init.headers), init.body ?? ""].join("\n");
}

/** 令牌桶：rps = 每秒补充的令牌，burst = 桶容量。 */
function createLimiter(
  options: { rps: number; burst: number } | undefined,
  now: () => number,
  sleep: (ms: number, signal?: AbortSignal) => Promise<void>,
): (signal?: AbortSignal) => Promise<void> {
  if (!options || options.rps <= 0 || options.burst <= 0) {
    return async (signal) => {
      throwIfAborted(signal);
    };
  }
  const rps = options.rps;
  const burst = options.burst;
  let tokens = burst;
  let last = now();
  return async (signal) => {
    for (;;) {
      throwIfAborted(signal);
      const current = now();
      if (current > last) {
        tokens = Math.min(burst, tokens + ((current - last) / 1000) * rps);
        last = current;
      }
      if (tokens >= 1) {
        tokens -= 1;
        return;
      }
      const waitMs = Math.max(1, Math.ceil(((1 - tokens) / rps) * 1000));
      await sleep(waitMs, signal);
    }
  };
}

export function createNetClient(
  net: NetApi,
  options: NetClientOptions = {},
): NetClient {
  const cacheTtlMs = options.cacheTtlMs ?? 30_000;
  const cacheMaxEntries = Math.max(0, options.cacheMaxEntries ?? 64);
  const timeoutMs = options.timeoutMs ?? 8_000;
  const now = options.now ?? (() => Date.now());
  const sleep = options.sleep ?? defaultSleep;
  const limit = createLimiter(options.rateLimit, now, sleep);
  const cache = new Map<string, CacheEntry>();

  function readCache(key: string): HostResponse | undefined {
    const hit = cache.get(key);
    if (!hit) return undefined;
    if (hit.expiresAt <= now()) {
      cache.delete(key);
      return undefined;
    }
    // LRU：命中后移到末尾。
    cache.delete(key);
    cache.set(key, hit);
    return hit.response;
  }

  function writeCache(key: string, response: HostResponse): void {
    if (cacheTtlMs <= 0 || cacheMaxEntries <= 0 || response.status >= 400) return;
    cache.delete(key);
    cache.set(key, { expiresAt: now() + cacheTtlMs, response });
    while (cache.size > cacheMaxEntries) {
      const oldest = cache.keys().next().value;
      if (oldest === undefined) break;
      cache.delete(oldest);
    }
  }

  return {
    async request(init: HostRequestInit): Promise<HostResponse> {
      throwIfAborted(init.signal);
      const method = (init.method ?? "GET").toUpperCase();
      const cacheable =
        cacheTtlMs > 0 && cacheMaxEntries > 0 && (method === "GET" || method === "HEAD");
      const key = cacheable ? cacheKey(init, method) : "";
      if (cacheable) {
        const hit = readCache(key);
        if (hit) return hit;
      }

      await limit(init.signal);
      throwIfAborted(init.signal);

      const response = await withAbort(
        net.request({ ...init, timeoutMs: init.timeoutMs ?? timeoutMs }),
        init.signal,
      );
      if (cacheable) writeCache(key, response);
      return response;
    },
    clearCache(): void {
      cache.clear();
    },
  };
}
