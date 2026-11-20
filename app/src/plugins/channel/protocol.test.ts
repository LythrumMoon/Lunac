import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  CHANNEL_VERSION,
  ChannelError,
  ResizeCoalescer,
  checkMethod,
  isTrustedSource,
  makeEnvelope,
  makeError,
  negotiateChannelVersion,
  parseEnvelope,
  withTimeout,
} from "./protocol";

describe("信封校验", () => {
  it("接受合法 req，并原样保留 payload", () => {
    const envelope = parseEnvelope({
      v: 1,
      id: "a1",
      type: "req",
      method: "net.fetch",
      payload: { url: "https://example.com" },
      ts: 1,
    });
    expect(envelope).toEqual({
      v: 1,
      id: "a1",
      type: "req",
      method: "net.fetch",
      payload: { url: "https://example.com" },
      ts: 1,
    });
  });

  it("拒绝缺字段、未知 type、非法版本与不可克隆 payload", () => {
    expect(parseEnvelope(null)).toBeNull();
    expect(parseEnvelope({})).toBeNull();
    expect(parseEnvelope({ v: 0, id: "a", type: "req", method: "ping" })).toBeNull();
    expect(parseEnvelope({ v: 1, id: "a", type: "wat", method: "ping" })).toBeNull();
    expect(parseEnvelope({ v: 1, id: "", type: "req", method: "ping" })).toBeNull();
    expect(
      parseEnvelope({ v: 1, id: "a", type: "req", method: "ping", payload: { fn: () => 1 } }),
    ).toBeNull();
  });

  it("makeEnvelope / makeError 使用同一信道版本", () => {
    expect(makeEnvelope("event", "hello", "e1").v).toBe(CHANNEL_VERSION);
    const error = makeError("r1", "net.fetch", new ChannelError("E_DENIED", "no"));
    expect(error.type).toBe("err");
    expect(error.payload).toEqual({ code: "E_DENIED", message: "no" });
  });
});

describe("版本协商", () => {
  it("选双方都支持的最高版本", () => {
    expect(negotiateChannelVersion([1, 2, 3], [2, 3, 4])).toBe(3);
    expect(negotiateChannelVersion([1, 2], [1])).toBe(1);
  });

  it("无交集返回 null（宿主据此回 E_VERSION）", () => {
    expect(negotiateChannelVersion([1, 2], [3])).toBeNull();
    expect(negotiateChannelVersion([], [1])).toBeNull();
  });
});

describe("能力校验", () => {
  it("未知方法回 E_UNSUPPORTED", () => {
    const error = checkMethod("wat.do", []);
    expect(error?.code).toBe("E_UNSUPPORTED");
  });

  it("net.fetch 必须在清单里声明能力", () => {
    expect(checkMethod("net.fetch", [])?.code).toBe("E_DENIED");
    expect(checkMethod("net.fetch", ["net.fetch"])).toBeNull();
  });

  it("iframe 引擎拒绝 reuse", () => {
    expect(checkMethod("ui.reuse", ["layout.takeover"], "iframe")?.code).toBe("E_UNSUPPORTED");
  });

  it("process.http 走 net.sidecar.http", () => {
    expect(checkMethod("process.http", ["process.spawn"])?.code).toBe("E_DENIED");
    expect(checkMethod("process.http", ["process.spawn", "net.sidecar.http"])).toBeNull();
  });
});

describe("超时与来源校验", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("超时回 E_TIMEOUT", async () => {
    const pending = new Promise<never>(() => {});
    const guarded = withTimeout(pending, 50, "net.fetch");
    const assertion = expect(guarded).rejects.toMatchObject({ code: "E_TIMEOUT" });
    await vi.advanceTimersByTimeAsync(50);
    await assertion;
  });

  it("source / origin 都要对上，targetOrigin=* 不通过", () => {
    const win = {};
    expect(isTrustedSource(win, win, "http://asset.localhost", "http://asset.localhost")).toBe(true);
    expect(isTrustedSource({}, win, "http://asset.localhost", "http://asset.localhost")).toBe(false);
    expect(isTrustedSource(win, win, "http://evil.test", "http://asset.localhost")).toBe(false);
    expect(isTrustedSource(win, win, "http://asset.localhost", "*")).toBe(false);
  });
});

describe("resize 合并", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("间隔内只 flush 最后一次", async () => {
    const calls: { height: number; width?: number }[] = [];
    const coalescer = new ResizeCoalescer(20, (size) => calls.push(size));
    coalescer.push(100);
    coalescer.push(120);
    coalescer.push(140, 600);
    await vi.advanceTimersByTimeAsync(20);
    expect(calls).toEqual([{ height: 140, width: 600 }]);
    coalescer.dispose();
  });
});
