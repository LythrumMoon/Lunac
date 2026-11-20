import assert from "node:assert/strict";
import { test } from "node:test";

import { strToU8, zipSync } from "fflate";

import type { DeclarativeRule, SourceManifest } from "../src/contract.js";
import { createDeclarativeSource } from "../src/executor.js";
import { htmlToText, parseEpub } from "../src/epub.js";
import { createMockHost } from "../src/mock-host.js";

const CONTAINER = `<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>`;

const OPF = `<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>测试书</dc:title>
    <dc:creator>夹具作者</dc:creator>
  </metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/>
    <item id="pic" href="images/pic.png" media-type="image/png"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="nav"/>
    <itemref idref="c1"/>
    <itemref idref="c2"/>
  </spine>
</package>`;

const NCX = `<?xml version="1.0" encoding="utf-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <navMap>
    <navPoint id="n1" playOrder="1">
      <navLabel><text>第一章 起风</text></navLabel>
      <content src="text/ch1.xhtml"/>
    </navPoint>
    <navPoint id="n2" playOrder="2">
      <navLabel><text>第二章 逆风</text></navLabel>
      <content src="text/ch2.xhtml"/>
    </navPoint>
  </navMap>
</ncx>`;

const CH1 = `<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>不应作为标题</title><style>p{color:red}</style></head>
  <body>
    <h1>第一章</h1>
    <p>第一段 &amp; 符号</p>
    <p><strong>粗体</strong>与<em>斜体</em></p>
    <p>第二段 &#20320; 好</p>
    <img src="../images/pic.png" alt="插图"/>
    <a href="https://evil.test" onclick="alert(1)">链接</a>
    <script>alert("x")</script>
  </body>
</html>`;

const CH2 = `<html><body><h2>第二章</h2><p>逆风的正文，足够长的一行用于通过最小长度检查。</p></body></html>`;

function buildEpub(): Uint8Array {
  return zipSync({
    mimetype: strToU8("application/epub+zip"),
    "META-INF/container.xml": strToU8(CONTAINER),
    "OEBPS/content.opf": strToU8(OPF),
    "OEBPS/toc.ncx": strToU8(NCX),
    "OEBPS/nav.xhtml": strToU8("<html><body><nav>目录</nav></body></html>"),
    "OEBPS/text/ch1.xhtml": strToU8(CH1),
    "OEBPS/text/ch2.xhtml": strToU8(CH2),
    "OEBPS/images/pic.png": new Uint8Array([0x89, 0x50, 0x4e, 0x47]),
  });
}

test("EPUB：按 spine 展开章节，NCX 标题优先，脚本与样式被丢弃", () => {
  const book = parseEpub(buildEpub());

  assert.equal(book.title, "测试书");
  assert.equal(book.author, "夹具作者");
  assert.equal(book.chapters.length, 2);
  assert.equal(book.chapters[0]?.title, "第一章 起风");
  assert.equal(book.chapters[1]?.title, "第二章 逆风");
  assert.match(book.chapters[0]?.text ?? "", /第一段 & 符号/);
  assert.match(book.chapters[0]?.text ?? "", /第二段 你 好/);
  assert.doesNotMatch(book.chapters[0]?.text ?? "", /alert|color:red|不应作为标题/);
  assert.match(book.chapters[1]?.text ?? "", /逆风的正文/);
});

test("EPUB：正文 HTML 保留语义标签，图片内联为 data URL，脚本 / 事件被清除", () => {
  const book = parseEpub(buildEpub());
  const html = book.chapters[0]?.html ?? "";

  assert.match(html, /<h1>第一章<\/h1>/);
  assert.match(html, /<strong>粗体<\/strong>/);
  assert.match(html, /<em>斜体<\/em>/);
  assert.match(html, /<img src="data:image\/png;base64,iVBORw=="/);
  assert.match(html, /alt="插图"/);
  assert.doesNotMatch(html, /<script|alert|onclick|evil\.test|color:red|<style/i);
});

test("EPUB：纯图片章节即使没有长文本也会保留", () => {
  const data = zipSync({
    mimetype: strToU8("application/epub+zip"),
    "META-INF/container.xml": strToU8(CONTAINER),
    "OEBPS/content.opf": strToU8(OPF),
    "OEBPS/toc.ncx": strToU8(NCX),
    "OEBPS/text/ch1.xhtml": strToU8(CH1),
    "OEBPS/text/ch2.xhtml": strToU8('<html><body><img src="../images/pic.png"/></body></html>'),
    "OEBPS/images/pic.png": new Uint8Array([0x89, 0x50, 0x4e, 0x47]),
  });
  const book = parseEpub(data);
  assert.equal(book.chapters.length, 2);
  assert.match(book.chapters[1]?.html ?? "", /data:image\/png/);
});

test("EPUB：坏包抛 parse，不返回半成品", () => {
  assert.throws(
    () => parseEpub(strToU8("not a zip")),
    (error: unknown) =>
      error instanceof Error && "code" in error && (error as { code: string }).code === "parse",
  );
});

test("htmlToText：段落、换行与实体", () => {
  const text = htmlToText("<div>甲<br/>乙</div><p>丙 &lt;丁&gt;</p>&#x4E00;");
  assert.equal(text, "甲\n乙\n\n丙 <丁>\n\n一");
});

const manifest: SourceManifest = {
  rsp: 1,
  id: "demo.novel",
  name: "Demo Novel",
  version: "1.0.0",
  kind: ["novel"],
  engine: "declarative",
  entry: "rule.json",
  baseUrl: "https://api.test",
  capabilities: ["chapters", "content"],
};

const rule: DeclarativeRule = {
  chapters: {
    request: { method: "GET", url: "@workUrl" },
    list: { jsonPath: "$.chapters[*]" },
    fields: {
      id: { selector: { jsonPath: "id" } },
      title: { selector: { jsonPath: "title" } },
    },
  },
  content: {
    request: { method: "GET", url: "@chapterUrl" },
    list: { jsonPath: "$.book[*]" },
    fields: { content: { selector: { jsonPath: "url" } } },
  },
  contentFormat: "epub",
};

test("执行器：contentFormat 透传到 TextChapter.format", async () => {
  const fetchImpl = (async () =>
    new Response(JSON.stringify({ book: [{ url: "https://files.test/book.epub" }] }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    })) as unknown as typeof fetch;
  const source = createDeclarativeSource(
    { manifest, rule },
    { host: createMockHost({ fetchImpl }) },
  );

  const content = await source.content!({
    id: "/book/ch1",
    workId: "/book",
    title: "全书",
    index: 0,
  });
  assert.equal(content.format, "epub");
  assert.equal(content.content, "https://files.test/book.epub");
});
