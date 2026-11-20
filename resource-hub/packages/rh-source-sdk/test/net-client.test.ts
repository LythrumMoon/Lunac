import assert from "node:assert/strict";
import { test } from "node:test";

import { RspError } from "../src/contract.js";
import type { HostRequestInit, HostResponse, NetApi } from "../src/host.js";
import { createNetClient } from "../src/net-client.js";

function okResponse(url: string, body = "ok"): HostResponse {
  return { url, status: 200, headers: {}, body };
}

function countingNet(): { net: NetApi; calls: HostRequestInit[] } {
  const calls: HostRequestInit[] = [];
  return {
    calls,
    net: {
      async request(init: HostRequestInit): Promise<HostResponse> {
        calls.push(init);
        return okResponse(init.url, `body-${calls.length}`);
      },
    },
  };
}

test("缓存：GET 命中，POST 不缓存", async () => {
  const { net, calls } = countingNet();
  const client = createNetClient(net, { cacheTtlMs: 60_000 });

  const first = await client.request({ url: "https://api.test/list" });
  const second = await client.request({ url: "https://api.test/list" });
  assert.equal(calls.length, 1);
  assert.equal(second.body, first.body);

  await client.request({ url: "https://api.test/list", method: "POST", body: "q=1" });
  await client.request({ url: "https://api.test/list", method: "POST", body: "q=1" });
  assert.equal(calls.length, 3);
});

test("缓存：TTL 过期后重新请求，clearCache 立即失效", async () => {
  const { net, calls } = countingNet();
  let clock = 0;
  const client = createNetClient(net, {
    cacheTtlMs: 1_000,
    now: () => clock,
  });

  await client.request({ url: "https://api.test/a" });
  clock = 500;
  await client.request({ url: "https://api.test/a" });
  assert.equal(calls.length, 1);

  clock = 1_001;
  await client.request({ url: "https://api.test/a" });
  assert.equal(calls.length, 2);

  client.clearCache();
  await client.request({ url: "https://api.test/a" });
  assert.equal(calls.length, 3);
});

test("限速：令牌桶按 rps 等待", async () => {
  const { net } = countingNet();
  let clock = 0;
  const waits: number[] = [];
  const client = createNetClient(net, {
    cacheTtlMs: 0,
    rateLimit: { rps: 2, burst: 1 },
    now: () => clock,
    sleep: async (ms) => {
      waits.push(ms);
      clock += ms;
    },
  });

  await client.request({ url: "https://api.test/1" });
  await client.request({ url: "https://api.test/2" });

  assert.equal(waits.length, 1);
  assert.ok(waits[0]! >= 450 && waits[0]! <= 500, `wait=${waits[0]}`);
});

test("取消：请求前 abort 直接拒绝且不触网", async () => {
  const { net, calls } = countingNet();
  const client = createNetClient(net);
  const controller = new AbortController();
  controller.abort();

  await assert.rejects(
    client.request({ url: "https://api.test/a", signal: controller.signal }),
    (error: unknown) =>
      error instanceof RspError && error.code === "network" && error.message.includes("取消"),
  );
  assert.equal(calls.length, 0);
});

test("取消：请求中 abort 立即拒绝，迟到结果被丢弃", async () => {
  let release: ((value: HostResponse) => void) | undefined;
  const net: NetApi = {
    request: () =>
      new Promise<HostResponse>((resolve) => {
        release = resolve;
      }),
  };
  const client = createNetClient(net);
  const controller = new AbortController();
  const pending = client.request({ url: "https://api.test/slow", signal: controller.signal });
  await Promise.resolve();

  controller.abort();
  await assert.rejects(pending, (error: unknown) => error instanceof RspError);
  release?.(okResponse("https://api.test/slow"));
});

test("超时：未显式给 timeoutMs 时补 8s，显式值原样透传", async () => {
  const { net, calls } = countingNet();
  const client = createNetClient(net);

  await client.request({ url: "https://api.test/a" });
  await client.request({ url: "https://api.test/b", timeoutMs: 2_000 });

  assert.equal(calls[0]?.timeoutMs, 8_000);
  assert.equal(calls[1]?.timeoutMs, 2_000);
});
