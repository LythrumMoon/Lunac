import assert from "node:assert/strict";
import { test } from "node:test";

import type { SourceManifest } from "../src/contract.js";
import { createJsSource } from "../src/js-source.js";
import { createMockHost } from "../src/mock-host.js";

const manifest: SourceManifest = {
  rsp: 1,
  id: "demo.js",
  name: "Demo JS",
  version: "1.0.0",
  kind: ["comic"],
  engine: "js",
  entry: "source.js",
  baseUrl: "https://api.test",
  capabilities: ["search", "chapters", "pages", "content", "play"],
};

const SCRIPT = `
function createSource(rsp) {
  return {
    async search(query, page) {
      const res = rsp.request({ url: "https://api.test/search?q=" + encodeURIComponent(query) + "&page=" + page });
      const data = JSON.parse(res.body);
      return {
        items: data.data.map((item) => ({
          id: item.id,
          title: item.title,
          cover: item.cover,
          tags: item.tags,
        })),
        next: "2",
      };
    },
    async chapters(work) {
      const res = rsp.request({ url: "https://api.test/chapters/" + work.id });
      return JSON.parse(res.body);
    },
    async pages(chapter) {
      return [{ url: "/p/1.jpg" }, "https://cdn.test/2.jpg"];
    },
    async content(chapter) {
      return { format: "html", content: "<p>" + chapter.title + "</p>" };
    },
    async play(chapter) {
      return [{ url: "https://cdn.test/stream.m3u8" }];
    },
  };
}
`;

function envelope(payload: unknown): string {
  return JSON.stringify({
    ok: true,
    response: {
      url: "https://api.test/",
      status: 200,
      headers: { "content-type": "application/json" },
      body: JSON.stringify(payload),
    },
  });
}

function host() {
  return createMockHost({
    allowEval: true,
    engineRequest: (optionsJson: string) => {
      const options = JSON.parse(optionsJson) as { url: string };
      if (options.url.includes("/search")) {
        return envelope({
          data: [
            { id: "1", title: "星海", cover: "/img/1.jpg", tags: "科幻,冒险" },
            { id: "2", title: "山海" },
          ],
        });
      }
      if (options.url.includes("/chapters/")) {
        return envelope([{ id: "/c/1", title: "第 1 话" }]);
      }
      return JSON.stringify({ ok: false, error: `unexpected: ${options.url}` });
    },
  });
}

test("JS 源：search 映射为 Work 并补 ref / 绝对封面", async () => {
  const source = createJsSource({ manifest, script: SCRIPT }, { host: host() });
  const page = await source.search!("星海", 2);

  assert.equal(page.items.length, 2);
  assert.deepEqual(page.items[0], {
    ref: { sourceId: "demo.js", workId: "1", kind: "comic" },
    title: "星海",
    author: undefined,
    cover: "https://api.test/img/1.jpg",
    description: undefined,
    tags: ["科幻", "冒险"],
    status: undefined,
    updatedAt: undefined,
  });
  assert.equal(page.next, "2");
});

test("JS 源：chapters / pages / content / play 走同一套脚本", async () => {
  const source = createJsSource({ manifest, script: SCRIPT }, { host: host() });

  const chapters = await source.chapters!({ sourceId: "demo.js", workId: "/comic/1", kind: "comic" });
  assert.deepEqual(chapters[0], {
    id: "/c/1",
    workId: "/comic/1",
    title: "第 1 话",
    index: 0,
    group: undefined,
    publishedAt: undefined,
  });

  const pages = await source.pages!(chapters[0]!);
  assert.equal(pages[0]?.url, "https://api.test/p/1.jpg");
  assert.equal(pages[1]?.url, "https://cdn.test/2.jpg");

  const content = await source.content!(chapters[0]!);
  assert.equal(content.format, "html");
  assert.match(content.content, /第 1 话/);

  const playbacks = await source.play!(chapters[0]!);
  assert.equal(playbacks[0]?.type, "hls");
});

test("JS 源：manifest.headers 默认生效，单次请求头可覆盖", async () => {
  let captured: Record<string, string> = {};
  const headerHost = createMockHost({
    allowEval: true,
    engineRequest: (optionsJson: string) => {
      const options = JSON.parse(optionsJson) as { headers?: Record<string, string> };
      captured = options.headers ?? {};
      return envelope({ data: [] });
    },
  });
  const source = createJsSource(
    {
      manifest: {
        ...manifest,
        headers: {
          Referer: "https://ref.test/",
          Cookie: "sid=1",
          "User-Agent": "default-ua",
        },
      },
      script: `function createSource(rsp) {
  return {
    async search() {
      rsp.request({ url: "https://api.test/search", headers: { "X-Test": "1", "User-Agent": "custom-ua" } });
      return { items: [] };
    },
  };
}`,
    },
    { host: headerHost },
  );
  await source.search!("x", 1);

  assert.equal(captured["Referer"], "https://ref.test/");
  assert.equal(captured["Cookie"], "sid=1");
  assert.equal(captured["User-Agent"], "custom-ua");
  assert.equal(captured["X-Test"], "1");
});

test("JS 源：脚本错误映射为 RspError(parse)", async () => {
  const source = createJsSource(
    { manifest, script: "function createSource() { return { search() { throw new Error('boom'); } }; }" },
    { host: host() },
  );
  await assert.rejects(source.search!("x"), (error: unknown) => {
    assert.ok(error instanceof Error);
    assert.match(error.message, /boom/);
    return (error as { code?: string }).code === "parse";
  });
});
