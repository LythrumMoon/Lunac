// 宿主 API v3 的 mock 实现。
//
// 用途：真实 Rust 能力（net / media / storage / engine）尚未落地时，
// 让插件、AI 工具与测试共用同一条调用链；接口不变，后续只替换实现。

import { RspError } from "./contract.js";
import {
  HOST_API_VERSION,
  type EngineApi,
  type HostRequestInit,
  type HostResponse,
  type LunacHostV3,
  type NetApi,
  type SniffApi,
  type StorageApi,
} from "./host.js";

export interface MockHostOptions {
  /** 注入 fetch（测试用）；默认取全局 fetch。 */
  fetchImpl?: typeof fetch;
  /** 预置存储。 */
  seed?: Record<string, unknown>;
  /** 是否允许 engine.eval 用 Function 执行；默认 false（抛 unsupported）。 */
  allowEval?: boolean;
  /**
   * 沙箱内 `host.request(JSON)` 的替身；返回宿主同款信封 JSON。
   * 缺省一律回 `{ ok: false }`（测试若不需要网络可不管）。
   */
  engineRequest?: (optionsJson: string) => string;
  /** WebView 嗅探替身（2026-10-21）；缺省返回空数组（即「没嗅到」）。 */
  sniffMedia?: (url: string) => string[] | Promise<string[]>;
}

export function createMockStorage(seed: Record<string, unknown> = {}): StorageApi {
  const data = new Map<string, unknown>(Object.entries(seed));
  return {
    async get<T = unknown>(key: string): Promise<T | undefined> {
      return data.get(key) as T | undefined;
    },
    async set(key: string, value: unknown): Promise<void> {
      data.set(key, value);
    },
    async remove(key: string): Promise<void> {
      data.delete(key);
    },
    async keys(): Promise<string[]> {
      return [...data.keys()];
    },
  };
}

export function createMockNet(options: MockHostOptions = {}): NetApi {
  return {
    async request(init: HostRequestInit): Promise<HostResponse> {
      const fetchImpl = options.fetchImpl ?? globalThis.fetch;
      if (typeof fetchImpl !== "function") {
        throw new RspError("unsupported", "当前运行时不支持 fetch", undefined);
      }
      const controller = new AbortController();
      const timeoutMs = init.timeoutMs ?? 15000;
      const timer = setTimeout(() => controller.abort(), timeoutMs);
      const onAbort = (): void => controller.abort();
      init.signal?.addEventListener("abort", onAbort, { once: true });
      try {
        const response = await fetchImpl(init.url, {
          method: init.method ?? "GET",
          headers: init.headers,
          body: init.body,
          signal: controller.signal,
        });
        const raw = await response.text();
        const maxBytes = init.maxBytes ?? 8 * 1024 * 1024;
        const body = raw.length > maxBytes ? raw.slice(0, maxBytes) : raw;
        const headers: Record<string, string> = {};
        response.headers.forEach((value, key) => {
          headers[key.toLowerCase()] = value;
        });
        const result: HostResponse = {
          url: response.url || init.url,
          status: response.status,
          headers,
          body,
        };
        if (init.as === "json") {
          try {
            result.json = JSON.parse(body);
          } catch {
            throw new RspError("parse", `响应不是合法 JSON：${init.url}`, undefined);
          }
        }
        return result;
      } catch (error) {
        if (error instanceof RspError) throw error;
        if (init.signal?.aborted) {
          throw new RspError("network", "请求已取消", undefined);
        }
        if (controller.signal.aborted) {
          throw new RspError("network", `请求超时（${timeoutMs}ms）：${init.url}`, undefined);
        }
        throw new RspError(
          "network",
          `请求失败：${init.url} → ${error instanceof Error ? error.message : String(error)}`,
          undefined,
        );
      } finally {
        clearTimeout(timer);
        init.signal?.removeEventListener("abort", onAbort);
      }
    },
  };
}

export function createMockEngine(options: MockHostOptions = {}): EngineApi {
  const request =
    options.engineRequest
    ?? ((_optionsJson: string) =>
      JSON.stringify({ ok: false, error: "mock 宿主未提供 host.request" }));
  return {
    async eval(code: string, evalOptions): Promise<unknown> {
      if (!options.allowEval) {
        throw new RspError(
          "unsupported",
          "mock 宿主默认不执行来源脚本（allowEval=false）",
          undefined,
        );
      }
      // 用**直接 eval** 模拟宿主 `ctx.eval` 的「完成值」语义：
      // `new Function` 的函数体不会返回最后一条语句的值。
      const fn = new Function(
        "args",
        "host",
        `"use strict";\nreturn eval(${JSON.stringify(code)});`,
      );
      return fn(evalOptions?.args, { request });
    },
  };
}

export function createMockSniff(options: MockHostOptions = {}): SniffApi {
  return {
    async media(url: string): Promise<string[]> {
      return options.sniffMedia ? await options.sniffMedia(url) : [];
    },
  };
}

export function createMockHost(options: MockHostOptions = {}): LunacHostV3 {
  return {
    apiVersion: HOST_API_VERSION,
    net: createMockNet(options),
    media: {
      proxy(url: string): string {
        return url;
      },
    },
    storage: createMockStorage(options.seed),
    engine: createMockEngine(options),
    sniff: createMockSniff(options),
  };
}
