import assert from "node:assert/strict";
import { test } from "node:test";

import type { DeclarativeRule, SourceManifest } from "../src/contract.js";
import { RspError } from "../src/contract.js";
import { createDeclarativeSource, isMediaUrl, playbackTypeFromUrl } from "../src/executor.js";
import type { NetApi } from "../src/host.js";
import { createMockHost } from "../src/mock-host.js";

const manifest: SourceManifest = {
  rsp: 1,
  id: "demo.api",
  name: "Demo API",
  version: "1.0.0",
  kind: ["comic"],
  engine: "declarative",
  entry: "rule.json",
  baseUrl: "https://api.test",
  capabilities: ["search", "chapters", "pages", "play"],
  headers: { "User-Agent": "ResourceHub/0.1" },
};

const rule: DeclarativeRule = {
  search: {
    request: { method: "GET", url: "/search", query: { q: "@keyword", page: "@page" } },
    list: { jsonPath: "$.data[*]" },
    fields: {
      id: { selector: { jsonPath: "id" } },
      title: { selector: { jsonPath: "title" } },
      cover: { selector: { jsonPath: "cover" } },
      tags: { selector: { jsonPath: "tags" }, template: "@value" },
    },
  },
  chapters: {
    request: { method: "GET", url: "@workUrl" },
    list: { jsonPath: "$.chapters[*]" },
    fields: {
      id: { selector: { jsonPath: "id" } },
      title: { selector: { jsonPath: "title" } },
    },
  },
  pages: {
    request: { method: "GET", url: "@chapterUrl" },
    list: { jsonPath: "$.images[*]" },
    fields: { url: { selector: { jsonPath: "src" } } },
  },
  play: {
    request: { method: "GET", url: "@chapterUrl" },
    list: { jsonPath: "$.episodes[*]" },
    fields: { url: { selector: { jsonPath: "url" } } },
  },
};

function jsonResponse(payload: unknown): Response {
  return new Response(JSON.stringify(payload), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

test("执行器：explore 规则拉取来源总览（section / page 占位符）", async () => {
  const calls: string[] = [];
  const fetchImpl = (async (url: string | URL | Request) => {
    calls.push(String(url));
    return jsonResponse({ data: [{ id: "9", title: "新作" }] });
  }) as unknown as typeof fetch;
  const exploreRule: DeclarativeRule = {
    explore: {
      request: { method: "GET", url: "/home/@section", query: { page: "@page" } },
      list: { jsonPath: "$.data[*]" },
      fields: {
        id: { selector: { jsonPath: "id" } },
        title: { selector: { jsonPath: "title" } },
      },
    },
  };
  const source = createDeclarativeSource(
    { manifest, rule: exploreRule },
    { host: createMockHost({ fetchImpl }) },
  );
  const page = await source.explore!("推荐", 2);

  assert.equal(calls[0], "https://api.test/home/%E6%8E%A8%E8%8D%90?page=2");
  assert.deepEqual(page.items.map((item) => [item.ref.workId, item.title]), [["9", "新作"]]);
});

test("执行器：JSON 搜索映射为 Work，并带分页与 manifest headers", async () => {
  const calls: { url: string; init?: RequestInit }[] = [];
  const fetchImpl = (async (url: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return jsonResponse({
      data: [
        { id: "1", title: "星海", cover: "/img/1.jpg", tags: "科幻,冒险" },
        { id: "2", title: "山海", cover: "https://cdn.test/2.jpg" },
      ],
    });
  }) as unknown as typeof fetch;

  const source = createDeclarativeSource(
    { manifest, rule },
    { host: createMockHost({ fetchImpl }) },
  );
  const page = await source.search!("星海", 2);

  assert.equal(
    calls[0]?.url,
    "https://api.test/search?q=%E6%98%9F%E6%B5%B7&page=2",
  );
  assert.deepEqual(calls[0]?.init?.headers, { "User-Agent": "ResourceHub/0.1" });
  assert.equal(page.next, "3");
  assert.deepEqual(page.items[0], {
    ref: { sourceId: "demo.api", workId: "1", kind: "comic" },
    title: "星海",
    author: undefined,
    cover: "https://api.test/img/1.jpg",
    description: undefined,
    tags: ["科幻", "冒险"],
  });
  assert.equal(page.items[1]?.cover, "https://cdn.test/2.jpg");
});

test("执行器：403 → blocked", async () => {
  const fetchImpl = (async () =>
    new Response("denied", { status: 403 })) as unknown as typeof fetch;
  const source = createDeclarativeSource(
    { manifest, rule },
    { host: createMockHost({ fetchImpl }) },
  );
  await assert.rejects(
    source.search!("x"),
    (error: unknown) => error instanceof RspError && error.code === "blocked",
  );
});

test("执行器：章节使用 @workUrl，图片使用 @chapterUrl 并解析相对地址", async () => {
  const seen: string[] = [];
  const fetchImpl = (async (url: string | URL | Request) => {
    seen.push(String(url));
    if (String(url).endsWith("/comic/1")) {
      return jsonResponse({ chapters: [{ id: "/c/1", title: "第 1 话" }] });
    }
    return jsonResponse({ images: [{ src: "/p/1.jpg" }, { src: "https://cdn.test/2.jpg" }] });
  }) as unknown as typeof fetch;

  const source = createDeclarativeSource(
    { manifest, rule },
    { host: createMockHost({ fetchImpl }) },
  );
  const chapters = await source.chapters!({
    sourceId: "demo.api",
    workId: "/comic/1",
    kind: "comic",
  });
  assert.deepEqual(chapters, [
    { id: "/c/1", workId: "/comic/1", title: "第 1 话", index: 0 },
  ]);
  assert.equal(seen[0], "https://api.test/comic/1");

  const pages = await source.pages!(chapters[0]);
  assert.equal(seen[1], "https://api.test/c/1");
  assert.deepEqual(
    pages.map((page) => page.url),
    ["https://api.test/p/1.jpg", "https://cdn.test/2.jpg"],
  );
});

test("执行器：把 signal / timeoutMs 透传给网络层", async () => {
  const seen: Parameters<NetApi["request"]>[0][] = [];
  const net: NetApi = {
    async request(init) {
      seen.push(init);
      return {
        url: init.url,
        status: 200,
        headers: {},
        body: JSON.stringify({ data: [] }),
      };
    },
  };
  const source = createDeclarativeSource(
    { manifest, rule },
    { host: createMockHost(), net },
  );
  const controller = new AbortController();

  await source.search!("x", 1, { signal: controller.signal, timeoutMs: 1_234 });

  assert.equal(seen[0]?.timeoutMs, 1_234);
  assert.equal(seen[0]?.signal, controller.signal);
});

test("执行器：播放地址按扩展名推断 hls / mp4", async () => {
  const fetchImpl = (async () =>
    jsonResponse({
      episodes: [{ url: "https://cdn.test/a.m3u8" }, { url: "https://cdn.test/b.mp4" }],
    })) as unknown as typeof fetch;
  const source = createDeclarativeSource(
    { manifest, rule },
    { host: createMockHost({ fetchImpl }) },
  );
  const playback = await source.play!({
    id: "/c/1",
    workId: "/comic/1",
    title: "第 1 话",
    index: 0,
  });
  assert.deepEqual(
    playback.map((item) => item.type),
    ["hls", "mp4"],
  );
});

test("执行器：未声明的能力抛 unsupported", async () => {
  const source = createDeclarativeSource(
    { manifest, rule: {} },
    { host: createMockHost() },
  );
  await assert.rejects(
    source.search!("x"),
    (error: unknown) => error instanceof RspError && error.code === "unsupported",
  );
});

test("执行器：sniff 规则交给宿主 WebView 嗅探（@chapterUrl）", async () => {
  const sniffed: string[] = [];
  const host = createMockHost({
    sniffMedia: (url) => {
      sniffed.push(url);
      return ["https://cdn.test/a.m3u8?token=1", "https://cdn.test/b.mp4"];
    },
  });
  const source = createDeclarativeSource(
    {
      manifest: { ...manifest, kind: ["anime"], capabilities: ["search", "chapters", "play"] },
      rule: { sniff: { url: "@chapterUrl" } },
    },
    { host },
  );
  const playbacks = await source.play!({ id: "/ep/1", workId: "w1", title: "第 1 话", index: 0 });
  assert.deepEqual(sniffed, ["https://api.test/ep/1"]);
  assert.deepEqual(playbacks.map((item) => item.type), ["hls", "mp4"]);
  assert.deepEqual(playbacks.map((item) => item.url), [
    "https://cdn.test/a.m3u8?token=1",
    "https://cdn.test/b.mp4",
  ]);
});

const cmsManifest: SourceManifest = {
  rsp: 1,
  id: "cms.demo",
  name: "CMS Demo",
  version: "1.0.0",
  kind: ["tv"],
  engine: "declarative",
  entry: "rule.json",
  capabilities: ["search", "explore", "chapters", "play"],
};

const cmsItem = {
  vod_id: 42,
  vod_name: "庆余年 第一季",
  vod_pic: "/upload/1.jpg",
  vod_content: "<p>少年范闲的故事</p>",
  type_name: "国产剧,古装",
  vod_remarks: "更新至 46 集",
  vod_year: "2019",
  vod_area: "大陆",
  vod_play_from: "线路1$$$线路2",
  vod_play_url:
    "第1集$https://cdn.test/1.m3u8#第2集$https://cdn.test/2.m3u8$$$第1集$https://play.test/ep/1",
};

test("执行器：CMS 采集接口搜索 / 剧集 / 直链播放", async () => {
  const urls: string[] = [];
  const fetchImpl = (async (input: string | URL | Request) => {
    const url = String(input);
    urls.push(url);
    if (url.includes("ids=")) return jsonResponse({ list: [cmsItem] });
    return jsonResponse({ code: 1, list: [cmsItem] });
  }) as unknown as typeof fetch;
  const source = createDeclarativeSource(
    { manifest: cmsManifest, rule: { cms: { api: "https://cms.test/api.php/provide/vod" } } },
    { host: createMockHost({ fetchImpl }) },
  );

  const page = await source.search!("庆余年", 2);
  assert.equal(
    urls[0],
    "https://cms.test/api.php/provide/vod?ac=detail&wd=%E5%BA%86%E4%BD%99%E5%B9%B4&pg=2",
  );
  assert.deepEqual(page.items[0], {
    ref: { sourceId: "cms.demo", workId: "42", kind: "tv" },
    title: "庆余年 第一季",
    author: undefined,
    cover: "https://cms.test/upload/1.jpg",
    description: "少年范闲的故事",
    tags: ["国产剧", "古装"],
    extra: { remarks: "更新至 46 集", year: "2019", area: "大陆" },
  });

  const explore = await source.explore!("", 1);
  assert.equal(explore.items.length, 1);
  assert.ok(urls[1].includes("ac=detail&pg=1"));

  const chapters = await source.chapters!({ sourceId: "cms.demo", workId: "42", kind: "tv" });
  assert.equal(chapters.length, 3);
  assert.deepEqual(
    chapters.map((chapter) => [chapter.title, chapter.group, chapter.id]),
    [
      ["第1集", "线路1", "https://cdn.test/1.m3u8"],
      ["第2集", "线路1", "https://cdn.test/2.m3u8"],
      ["第1集", "线路2", "https://play.test/ep/1"],
    ],
  );

  const playback = await source.play!(chapters[0]);
  assert.deepEqual(playback.map((item) => [item.type, item.url]), [
    ["hls", "https://cdn.test/1.m3u8"],
  ]);
});

test("执行器：CMS 播放页回退到 WebView 嗅探", async () => {
  const sniffed: string[] = [];
  const fetchImpl = (async () => jsonResponse({ list: [cmsItem] })) as unknown as typeof fetch;
  const host = createMockHost({
    fetchImpl,
    sniffMedia: (url) => {
      sniffed.push(url);
      return ["https://cdn.test/sniffed.m3u8"];
    },
  });
  const source = createDeclarativeSource(
    { manifest: cmsManifest, rule: { cms: { api: "https://cms.test/api.php/provide/vod" } } },
    { host },
  );
  const chapters = await source.chapters!({ sourceId: "cms.demo", workId: "42", kind: "tv" });
  const playback = await source.play!(chapters[2]);

  assert.deepEqual(sniffed, ["https://play.test/ep/1"]);
  assert.deepEqual(playback.map((item) => [item.type, item.url]), [
    ["hls", "https://cdn.test/sniffed.m3u8"],
  ]);
});

test("执行器：线路类型判定（MKV 按 mp4 试播，但仍算直链）", () => {
  assert.equal(playbackTypeFromUrl("https://cdn.test/a.m3u8?token=1"), "hls");
  assert.equal(playbackTypeFromUrl("https://cdn.test/b.mpd"), "dash");
  assert.equal(playbackTypeFromUrl("https://cdn.test/c.MKV?sign=1"), "mp4");
  assert.equal(playbackTypeFromUrl("https://cdn.test/d.mp4"), "mp4");
  assert.equal(playbackTypeFromUrl("https://cdn.test/ep/1"), "mp4");
  // MKV 不单列类型，但仍是直链：不该交给 WebView 嗅探成播放页。
  assert.equal(isMediaUrl("https://cdn.test/x.mkv"), true);
  assert.equal(isMediaUrl("https://play.test/ep/1"), false);
});

test("执行器：CMS 的 MKV 直链不出 sniff，按 mp4 交给播放器", async () => {
  const sniffed: string[] = [];
  const item = { ...cmsItem, vod_play_url: "第1集$https://cdn.test/1.mkv" };
  const fetchImpl = (async () => jsonResponse({ list: [item] })) as unknown as typeof fetch;
  const host = createMockHost({
    fetchImpl,
    sniffMedia: (url) => {
      sniffed.push(url);
      return [];
    },
  });
  const source = createDeclarativeSource(
    { manifest: cmsManifest, rule: { cms: { api: "https://cms.test/api.php/provide/vod" } } },
    { host },
  );
  const chapters = await source.chapters!({ sourceId: "cms.demo", workId: "42", kind: "tv" });
  const playback = await source.play!(chapters[0]);

  assert.deepEqual(playback.map((line) => [line.type, line.url]), [
    ["mp4", "https://cdn.test/1.mkv"],
  ]);
  assert.deepEqual(sniffed, []);
});

test("执行器：CMS 关闭搜索时给出可读错误", async () => {
  const fetchImpl = (async () =>
    new Response("暂不支持搜索", { status: 200 })) as unknown as typeof fetch;
  const source = createDeclarativeSource(
    { manifest: cmsManifest, rule: { cms: { api: "https://cms.test/api.php/provide/vod" } } },
    { host: createMockHost({ fetchImpl }) },
  );
  await assert.rejects(
    source.search!("x"),
    (error: unknown) =>
      error instanceof RspError && error.code === "unsupported" && error.message.includes("未开放搜索"),
  );
});

