import assert from "node:assert/strict";
import { test } from "node:test";

import { createMockHost, createMockStorage } from "../src/mock-host.js";
import { HOST_API_VERSION, findHost, hasHostV3, requireHostV3 } from "../src/host.js";
import { RspError } from "../src/contract.js";

test("mock storage：seed / set / get / remove / keys", async () => {
  const storage = createMockStorage({ a: 1 });
  assert.equal(await storage.get("a"), 1);
  assert.deepEqual(await storage.keys(), ["a"]);
  await storage.set("b", { x: 2 });
  assert.deepEqual(await storage.get("b"), { x: 2 });
  await storage.remove("a");
  assert.equal(await storage.get("a"), undefined);
  assert.deepEqual(await storage.keys(), ["b"]);
});

test("mock net：透传 method / headers / body，并解析 json", async () => {
  const calls: { url: string; init?: RequestInit }[] = [];
  const fetchImpl = (async (url: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { "Content-Type": "application/json", "X-Test": "1" },
    });
  }) as unknown as typeof fetch;

  const host = createMockHost({ fetchImpl });
  const response = await host.net.request({
    url: "https://api.test/search",
    method: "POST",
    headers: { "X-Token": "t" },
    body: "q=1",
    as: "json",
  });

  assert.equal(calls[0]?.url, "https://api.test/search");
  assert.equal(calls[0]?.init?.method, "POST");
  assert.deepEqual(calls[0]?.init?.headers, { "X-Token": "t" });
  assert.equal(calls[0]?.init?.body, "q=1");
  assert.equal(response.status, 200);
  assert.equal(response.headers["content-type"], "application/json");
  assert.deepEqual(response.json, { ok: true });
});

test("mock net：maxBytes 截断响应体", async () => {
  const fetchImpl = (async () =>
    new Response("0123456789", { status: 200 })) as unknown as typeof fetch;
  const host = createMockHost({ fetchImpl });
  const response = await host.net.request({ url: "https://api.test/", maxBytes: 4 });
  assert.equal(response.body, "0123");
});

test("mock net：fetch 失败 → RspError(network)", async () => {
  const fetchImpl = (async () => {
    throw new TypeError("boom");
  }) as unknown as typeof fetch;
  const host = createMockHost({ fetchImpl });
  await assert.rejects(
    host.net.request({ url: "https://api.test/" }),
    (error: unknown) =>
      error instanceof RspError && error.code === "network" && error.message.includes("boom"),
  );
});

test("mock engine：默认拒绝，allowEval 后可执行", async () => {
  const denied = createMockHost();
  await assert.rejects(
    denied.engine.eval("return 1"),
    (error: unknown) => error instanceof RspError && error.code === "unsupported",
  );
  const allowed = createMockHost({ allowEval: true });
  assert.equal(await allowed.engine.eval("args.n + 1", { args: { n: 1 } }), 2);
});

test("宿主探测：无桥 / 版本不符 / 完整 v3", () => {
  const scope = {} as Record<string, unknown>;
  assert.equal(hasHostV3(scope), false);
  assert.equal(findHost(scope), undefined);
  assert.throws(
    () => requireHostV3(scope),
    (error: unknown) => error instanceof RspError && error.code === "unsupported",
  );

  const host = createMockHost();
  const withHost = { __lunac_host: { ...host, apiVersion: HOST_API_VERSION - 1 } };
  assert.equal(hasHostV3(withHost), false);

  const full = { __lunac_host: host };
  assert.equal(hasHostV3(full), true);
  assert.equal(requireHostV3(full).apiVersion, HOST_API_VERSION);
});
