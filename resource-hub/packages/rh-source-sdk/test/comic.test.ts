import assert from "node:assert/strict";
import { createCipheriv, createHash, createHmac } from "node:crypto";
import { test } from "node:test";

import type { SourceManifest } from "../src/contract.js";
import { createMockHost } from "../src/mock-host.js";
import { createComicSource, looksLikeComic } from "../src/comic-source.js";
import { COMIC_PRELUDE } from "../src/comic-shim.js";

const manifest: SourceManifest = {
  rsp: 1,
  id: "demo.comic",
  name: "Demo Comic",
  version: "1.0.0",
  kind: ["comic"],
  engine: "js",
  entry: "source.js",
  baseUrl: "https://demo.test",
  capabilities: ["search", "chapters", "pages"],
};

const SCRIPT = `
class DemoSource extends ComicSource {
  name = "Demo Comic";
  key = "demo_comic";
  version = "1.0.0";
  url = "https://demo.test";
  settings = { lang: { default: "cn" } };

  search = {
    load: async (keyword, options, page) => {
      const res = await Network.get("https://demo.test/search?q=" + encodeURIComponent(keyword));
      const doc = new HtmlDocument(res.body);
      const comics = doc.querySelectorAll("div#main > div.comic-card").map((card) => ({
        id: card.querySelector("a").attributes["href"],
        title: card.querySelector("h3").text,
        cover: card.querySelector("img").attributes["src"],
        subTitle: card.querySelector("span.author").text,
      }));
      return { comics, maxPage: 1 };
    },
  };

  comic = {
    loadInfo: async (id) => {
      const res = await Network.get("https://demo.test/comic/" + id);
      const doc = new HtmlDocument(res.body);
      const chapters = new Map();
      doc.querySelectorAll("ul.chapters > li > a").forEach((node) => {
        chapters.set(node.attributes["data-id"], node.text);
      });
      return new ComicDetails({ title: "星海", description: "一段简介", tags: ["冒险"], chapters });
    },
    loadEp: async (comicId, epId) => {
      const res = await Network.get("https://demo.test/chapter/" + comicId + "/" + epId);
      const doc = new HtmlDocument(res.body);
      return {
        images: doc.querySelectorAll("div.reader img").map((img) => img.attributes["data-src"]),
      };
    },
  };
}
`;

const SEARCH_HTML = `
<html><body>
  <div id="main">
    <div class="comic-card">
      <a href="/comic/1"><h3>星海 &amp; 山海</h3><img src="/img/1.jpg"></a>
      <span class="author">作者甲</span>
    </div>
    <div class="comic-card">
      <a href="/comic/2"><h3>风起</h3><img src="https://cdn.test/2.jpg"></a>
      <span class="author">作者乙</span>
    </div>
  </div>
</body></html>`;

const INFO_HTML = `
<html><body>
  <ul class="chapters">
    <li><a data-id="c1">第 1 话</a></li>
    <li><a data-id="c2">第 2 话</a></li>
  </ul>
</body></html>`;

const PAGES_HTML = `
<html><body>
  <div class="reader">
    <img data-src="/p/1.jpg">
    <img data-src="https://cdn.test/2.jpg">
  </div>
</body></html>`;

function envelope(body: string): string {
  return JSON.stringify({
    ok: true,
    response: {
      url: "https://demo.test/",
      status: 200,
      headers: { "content-type": "text/html" },
      body,
    },
  });
}

function host() {
  return createMockHost({
    allowEval: true,
    engineRequest: (optionsJson: string) => {
      const options = JSON.parse(optionsJson) as { url: string };
      if (options.url.includes("/search")) return envelope(SEARCH_HTML);
      if (options.url.includes("/chapter/")) return envelope(PAGES_HTML);
      if (options.url.includes("/comic/")) return envelope(INFO_HTML);
      return JSON.stringify({ ok: false, error: `unexpected: ${options.url}` });
    },
  });
}

test("漫画源识别：只认 class ... extends ComicSource", () => {
  assert.equal(looksLikeComic(SCRIPT), true);
  assert.equal(looksLikeComic("function createSource(rsp) { return {}; }"), false);
});

test("漫画源：搜索走 Network + HtmlDocument（后代 / 子选择器、属性、实体）", async () => {
  const source = createComicSource({ manifest, script: SCRIPT }, { host: host() });
  const page = await source.search!("星海", 1);

  assert.equal(page.items.length, 2);
  assert.deepEqual(page.items[0], {
    ref: { sourceId: "demo.comic", workId: "/comic/1", kind: "comic" },
    title: "星海 & 山海",
    author: "作者甲",
    cover: "https://demo.test/img/1.jpg",
    description: undefined,
    tags: undefined,
    status: undefined,
  });
  assert.equal(page.items[1]?.cover, "https://cdn.test/2.jpg");
});

test("漫画源 源：explore 展开 singlePageWithMultiPart 分区", async () => {
  const script = `
class ExploreSource extends ComicSource {
  name = "Explore Source";
  key = "explore_source";
  version = "1.0.0";
  url = "https://demo.test";
  explore = [{
    title: "推荐",
    type: "singlePageWithMultiPart",
    load: async () => ({
      "热门": [
        { id: "/comic/1", title: "甲" },
        { id: "/comic/2", title: "乙" },
      ],
      "新作": [{ id: "/comic/3", title: "丙", cover: "/c/3.jpg" }],
    }),
  }];
}
`;
  const source = createComicSource({ manifest, script }, { host: host() });
  const page = await source.explore!();

  assert.deepEqual(
    page.items.map((item) => [item.ref.workId, item.title]),
    [["/comic/1", "甲"], ["/comic/2", "乙"], ["/comic/3", "丙"]],
  );
  assert.equal(page.items[2]?.cover, "https://demo.test/c/3.jpg");
});

test("漫画源：detail 补简介与标签", async () => {
  const source = createComicSource({ manifest, script: SCRIPT }, { host: host() });
  const work = await source.detail!({
    sourceId: "demo.comic",
    workId: "/comic/1",
    kind: "comic",
  });
  assert.equal(work.description, "一段简介");
  assert.deepEqual(work.tags, ["冒险"]);
});

test("漫画源：详情页标签支持「分组 → 值」对象形态", async () => {
  // 百合会（真实来源）就是把 tags 写成对象：{作者:[…], 更新:[…], 标签:[…]}。
  const script = `
class GroupedTags extends ComicSource {
  name = "Grouped";
  key = "grouped";
  comic = {
    loadInfo: async (id) => new ComicDetails({
      title: "作品",
      tags: { 作者: ["作者甲"], 标签: ["冒险", "奇幻"], 空: [] },
      chapters: new Map(),
    }),
  };
}
`;
  const source = createComicSource(
    { manifest, script },
    { host: createMockHost({ allowEval: true }) },
  );
  const work = await source.detail!({
    sourceId: "demo.comic",
    workId: "1",
    kind: "comic",
  });
  assert.deepEqual(work.tags, ["作者：作者甲", "标签：冒险 / 奇幻"]);
});

test("漫画源：章节从 ComicDetails.chapters（Map）展开", async () => {
  const source = createComicSource({ manifest, script: SCRIPT }, { host: host() });
  const chapters = await source.chapters!({
    sourceId: "demo.comic",
    workId: "/comic/1",
    kind: "comic",
  });

  assert.deepEqual(
    chapters.map((chapter) => [chapter.id, chapter.title]),
    [["c1", "第 1 话"], ["c2", "第 2 话"]],
  );
});

test("漫画源 源：图片读取 comic.loadEp", async () => {
  const source = createComicSource({ manifest, script: SCRIPT }, { host: host() });
  const pages = await source.pages!({
    id: "c1",
    workId: "/comic/1",
    title: "第 1 话",
    index: 0,
  });

  assert.deepEqual(
    pages.map((page) => page.url),
    ["https://demo.test/p/1.jpg", "https://cdn.test/2.jpg"],
  );
  // 没有声明 onImageLoad：逐页 headers / 后处理都不该凭空出现。
  assert.equal(pages[0]?.transform, undefined);
  assert.equal(pages[0]?.headers, undefined);
});

/** 加密图源（漫蛙吧形态）：图片是密文，必须按源自己的 onResponse 解成明文。 */
const ENCRYPTED_SCRIPT = `
class EncryptedComic extends ComicSource {
  name = "Encrypted";
  key = "encrypted";
  version = "1.0.0";
  url = "https://demo.test";
  search = { load: async () => ({ comics: [] }) };
  comic = {
    loadInfo: async () => new ComicDetails({ title: "加密", chapters: new Map([["c1", "第 1 话"]]) }),
    loadEp: async () => ({
      images: ["https://img.test/1.jpg"],
      headers: { Referer: "https://demo.test/" },
    }),
    onImageLoad: (url) => ({
      url,
      headers: { "User-Agent": "img-agent" },
      onResponse: (buffer) => {
        const view = new Uint8Array(buffer);
        for (let i = 0; i < view.length; i += 1) view[i] = view[i] ^ 0xff;
        return view;
      },
    }),
    onThumbnailLoad: (url) => ({
      url,
      onResponse: (buffer) => {
        const view = new Uint8Array(buffer);
        for (let i = 0; i < view.length; i += 1) view[i] = view[i] ^ 0x0f;
        return view;
      },
    }),
  };
}
`;

function encryptedHost(cipher: number[]) {
  const bodyBase64 = Buffer.from(cipher).toString("base64");
  return createMockHost({
    allowEval: true,
    engineRequest: (optionsJson: string) => {
      const options = JSON.parse(optionsJson) as { url: string };
      if (options.url === "https://img.test/1.jpg") {
        return JSON.stringify({
          ok: true,
          response: {
            url: options.url,
            status: 200,
            headers: { "content-type": "image/jpeg" },
            body: "",
            bodyBase64,
          },
        });
      }
      return JSON.stringify({ ok: false, error: `unexpected: ${options.url}` });
    },
  });
}

test("漫画源：onImageLoad 的 headers / onResponse 落进 pages 与 imageData", async () => {
  const cipher = [0x33, 0xe9, 0x0c, 0xed];
  const chapter = { id: "c1", workId: "1", title: "第 1 话", index: 0 };
  const source = createComicSource(
    { manifest, script: ENCRYPTED_SCRIPT },
    { host: encryptedHost(cipher) },
  );

  const pages = await source.pages!(chapter);
  assert.equal(pages.length, 1);
  assert.deepEqual(pages[0]?.headers, {
    Referer: "https://demo.test/",
    "User-Agent": "img-agent",
  });
  assert.equal(pages[0]?.transform, true);

  const bytes = await source.imageData!(chapter, pages[0]!);
  assert.deepEqual([...(bytes ?? [])], cipher.map((byte) => byte ^ 0xff));

  // 封面走另一条分支：没有 chapterId ⇒ onThumbnailLoad（§待办 3-20）。
  const cover = await source.thumbnailData!("https://img.test/1.jpg");
  assert.deepEqual([...(cover ?? [])], cipher.map((byte) => byte ^ 0x0f));
});

test("漫画源 源：非上游形态拒绝执行", () => {
  assert.throws(
    () =>
      createComicSource(
        { manifest, script: "function createSource() { return {}; }" },
        { host: host() },
      ),
    (error: unknown) =>
      error instanceof Error && "code" in error && (error as { code: string }).code === "unsupported",
  );
});

test("漫画源 源：Network.post 接受 Convert.encodeUtf8 的字节 body", async () => {
  const script = `
class BytePost extends ComicSource {
  name = "Byte Post";
  key = "byte_post";
  version = "1.0.0";
  url = "https://demo.test";
  search = {
    load: async (keyword) => {
      const res = await Network.post(
        "https://demo.test/s",
        { "Content-Type": "application/x-www-form-urlencoded" },
        Convert.encodeUtf8("k=" + encodeURIComponent(keyword)),
      );
      const doc = new HtmlDocument(res.body);
      return {
        comics: doc.querySelectorAll(".item").map((node) => ({
          id: node.attributes["href"],
          title: node.text.trim(),
        })),
        maxPage: 1,
      };
    },
  };
}
`;
  let postedBody = "";
  const postHost = createMockHost({
    allowEval: true,
    engineRequest: (optionsJson: string) => {
      const options = JSON.parse(optionsJson) as { method: string; body?: string };
      if (options.method === "POST") postedBody = options.body ?? "";
      return envelope(`<div class="item" href="/comic/1">海贼</div>`);
    },
  });
  const source = createComicSource({ manifest, script }, { host: postHost });
  const page = await source.search!("海贼", 1);
  assert.equal(postedBody, "k=%E6%B5%B7%E8%B4%BC");
  assert.equal(page.items[0]?.title, "海贼");
});

test("漫画源 源：manifest.headers 默认生效，单次请求头可覆盖", async () => {
  const script = `
class HeaderSource extends ComicSource {
  name = "Header Source";
  key = "header_source";
  version = "1.0.0";
  url = "https://demo.test";
  search = {
    load: async () => {
      await Network.get("https://demo.test/search", { "X-Test": "1", "User-Agent": "custom-ua" });
      return { comics: [{ id: "/c/1", title: "ok" }], maxPage: 1 };
    },
  };
}
`;
  let captured: Record<string, string> = {};
  const headerHost = createMockHost({
    allowEval: true,
    engineRequest: (optionsJson: string) => {
      const options = JSON.parse(optionsJson) as { headers?: Record<string, string> };
      captured = options.headers ?? {};
      return envelope(`<html></html>`);
    },
  });
  const source = createComicSource(
    {
      manifest: {
        ...manifest,
        headers: {
          Referer: "https://ref.test/",
          Cookie: "sid=abc",
          "User-Agent": "default-ua",
        },
      },
      script,
    },
    { host: headerHost },
  );
  await source.search!("x", 1);

  assert.equal(captured["Referer"], "https://ref.test/");
  assert.equal(captured["Cookie"], "sid=abc");
  assert.equal(captured["User-Agent"], "custom-ua");
  assert.equal(captured["X-Test"], "1");
});

test("漫画源 源：Network 的 options 形态（{ headers, body }）被解包成字符串头", async () => {
  const script = `
class OptionsSource extends ComicSource {
  name = "Options Source";
  key = "options_source";
  version = "1.0.0";
  url = "https://demo.test";
  search = {
    load: async () => {
      await Network.get("https://demo.test/search", {
        headers: { "X-Test": "1", "X-Obj": { a: 1 } },
      });
      await Network.post("https://demo.test/save", {
        headers: { "Content-Type": "application/json" },
        body: "{\\"k\\":1}",
      });
      return { comics: [{ id: "/c/1", title: "ok" }], maxPage: 1 };
    },
  };
}
`;
  const seen: Array<{ method: string; headers?: Record<string, unknown>; body?: unknown }> = [];
  const optionsHost = createMockHost({
    allowEval: true,
    engineRequest: (optionsJson: string) => {
      seen.push(JSON.parse(optionsJson) as (typeof seen)[number]);
      return envelope(`<html></html>`);
    },
  });
  const source = createComicSource({ manifest, script }, { host: optionsHost });
  await source.search!("x", 1);

  assert.equal(seen.length, 2);
  const get = seen[0];
  const post = seen[1];
  // 头值必须全是字符串：对象值退化成 JSON 文本，宿主侧 serde 才不会整次拒掉。
  assert.equal(get?.headers?.["X-Test"], "1");
  assert.equal(get?.headers?.["X-Obj"], '{"a":1}');
  for (const value of Object.values(get?.headers ?? {})) {
    assert.equal(typeof value, "string");
  }
  assert.equal(post?.method, "POST");
  assert.equal(post?.headers?.["Content-Type"], "application/json");
  assert.equal(post?.body, '{"k":1}');
});

test("漫画源 源：Network.fetchBytes 拿到非 UTF-8 原始字节", async () => {
  const script = `
class ByteFetch extends ComicSource {
  name = "Byte Fetch";
  key = "byte_fetch";
  version = "1.0.0";
  url = "https://demo.test";
  search = {
    load: async () => {
      const res = await Network.fetchBytes("GET", "https://demo.test/gbk");
      const bytes = new Uint8Array(res.body);
      let hex = "";
      for (let i = 0; i < bytes.length; i++) hex += ("0" + bytes[i].toString(16)).slice(-2);
      return { comics: [{ id: "/x", title: hex }], maxPage: 1 };
    },
  };
}
`;
  const gbkHost = createMockHost({
    allowEval: true,
    engineRequest: () =>
      JSON.stringify({
        ok: true,
        response: {
          url: "https://demo.test/gbk",
          status: 200,
          headers: { "content-type": "text/html; charset=gbk" },
          body: "\uFFFD\uFFFD",
          // GBK「海」= BA A3
          bodyBase64: "uqM=",
        },
      }),
  });
  const source = createComicSource({ manifest, script }, { host: gbkHost });
  const page = await source.search!("x", 1);
  assert.equal(page.items[0]?.title, "baa3");
});

test("漫画源 源：manifest.settings 覆盖脚本 loadSetting", async () => {
  const script = `
class SettingSource extends ComicSource {
  name = "Setting Source";
  key = "setting_source";
  version = "1.0.0";
  url = "https://dead.example";
  settings = { domains: { title: "主域名", type: "input", default: "dead.example" } };
  search = {
    load: async () => {
      const res = await Network.get("https://" + this.loadSetting("domains") + "/search?q=x");
      const doc = new HtmlDocument(res.body);
      return { comics: doc.querySelectorAll(".item").map((node) => ({ id: "1", title: node.text.trim() })), maxPage: 1 };
    },
  };
}
`;
  let requestedUrl = "";
  const settingHost = createMockHost({
    allowEval: true,
    engineRequest: (optionsJson: string) => {
      const options = JSON.parse(optionsJson) as { url: string };
      requestedUrl = options.url;
      return envelope(`<div class="item">海贼</div>`);
    },
  });
  const source = createComicSource(
    {
      manifest: { ...manifest, settings: { domains: "alive.example" } },
      script,
    },
    { host: settingHost },
  );
  const page = await source.search!("x", 1);
  assert.equal(requestedUrl, "https://alive.example/search?q=x");
  assert.equal(page.items[0]?.title, "海贼");
});

/** 在 Node 里直接求值兼容层 prelude，取出 Convert。 */
function convertSandbox() {
  const factory = new Function(`${COMIC_PRELUDE}\nreturn { Convert: Convert };`);
  return factory() as {
    Convert: {
      encodeUtf8(value: string): ArrayBuffer;
      encodeGbk(value: string): ArrayBuffer;
      md5(buffer: ArrayBuffer): ArrayBuffer;
      sha256(buffer: ArrayBuffer): ArrayBuffer;
      sha512(buffer: ArrayBuffer): ArrayBuffer;
      hmacString(key: ArrayBuffer, data: ArrayBuffer, algorithm?: string): string;
      decryptAesCbc(data: ArrayBuffer, key: ArrayBuffer, iv: ArrayBuffer): ArrayBuffer;
      decryptAesEcb(data: ArrayBuffer, key: ArrayBuffer): ArrayBuffer;
    };
  };
}

const toHex = (buffer: ArrayBuffer) => Buffer.from(buffer).toString("hex");

function toArrayBuffer(buffer: Buffer): ArrayBuffer {
  const copy = new Uint8Array(buffer.length);
  copy.set(buffer);
  return copy.buffer;
}

test("漫画源 Convert：md5 / sha256 / sha512 与 Node 一致", () => {
  const { Convert } = convertSandbox();
  const input = Convert.encodeUtf8("abc");
  assert.equal(toHex(Convert.md5(input)), createHash("md5").update("abc").digest("hex"));
  assert.equal(toHex(Convert.sha256(input)), createHash("sha256").update("abc").digest("hex"));
  assert.equal(toHex(Convert.sha512(input)), createHash("sha512").update("abc").digest("hex"));
});

test("漫画源 Convert：encodeGbk 输出 GB2312 字节", () => {
  const { Convert } = convertSandbox();
  assert.equal(toHex(Convert.encodeGbk("abc")), "616263");
  assert.equal(toHex(Convert.encodeGbk("海贼")), "baa3d4f4");
});

test("漫画源 Convert：hmacString 返回小写十六进制", () => {
  const { Convert } = convertSandbox();
  const key = Convert.encodeUtf8("secret");
  const data = Convert.encodeUtf8("hello");
  for (const algorithm of ["sha256", "sha512", "md5"] as const) {
    assert.equal(
      Convert.hmacString(key, data, algorithm),
      createHmac(algorithm, "secret").update("hello").digest("hex"),
    );
  }
});

test("漫画源 Convert：AES-CBC / ECB 解密（128 / 192 / 256）", () => {
  const { Convert } = convertSandbox();
  const iv = Buffer.from("abcdef0123456789", "utf8");
  const plain = Buffer.from("0123456789abcdef", "utf8");
  for (const size of [16, 24, 32]) {
    const key = Buffer.from("k".repeat(size), "utf8");
    const cbc = createCipheriv(`aes-${size * 8}-cbc`, key, iv);
    cbc.setAutoPadding(false);
    const cipherText = Buffer.concat([cbc.update(plain), cbc.final()]);
    assert.equal(
      toHex(Convert.decryptAesCbc(toArrayBuffer(cipherText), toArrayBuffer(key), toArrayBuffer(iv))),
      plain.toString("hex"),
    );

    const ecb = createCipheriv(`aes-${size * 8}-ecb`, key, null);
    ecb.setAutoPadding(false);
    const ecbText = Buffer.concat([ecb.update(plain), ecb.final()]);
    assert.equal(
      toHex(Convert.decryptAesEcb(toArrayBuffer(ecbText), toArrayBuffer(key))),
      plain.toString("hex"),
    );
  }
});

test("漫画源：loadData / saveData 跨调用持久化（按来源 id 隔离）", async () => {
  const script = `
class DataSource extends ComicSource {
  name = "Data Demo";
  key = "data_demo";
  url = "https://demo.test";

  init() {
    const data = this.loadData() || { count: 0 };
    data.count += 1;
    this.saveData(data);
  }

  search = {
    load: async () => ({ comics: [{ id: "1", title: String((this.loadData() || {}).count) }] }),
  };
}`;
  const host = createMockHost({
    allowEval: true,
    seed: { "source.demo.comic.data": { count: 4 } },
  });
  const source = createComicSource({ manifest, script }, { host });
  const first = await source.search!("x", 1);
  assert.equal(first.items[0]?.title, "5");
  assert.deepEqual(await host.storage.get("source.demo.comic.data"), { count: 5 });

  const second = createComicSource({ manifest, script }, { host });
  const next = await second.search!("x", 1);
  assert.equal(next.items[0]?.title, "6");
});
