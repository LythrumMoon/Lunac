import assert from "node:assert/strict";
import { test } from "node:test";

import type { SourceManifest } from "../src/contract.js";
import { createMockHost } from "../src/mock-host.js";
import { createVideoSource, looksLikeVideo, videoUnsupportedReason } from "../src/video-source.js";

const manifest: SourceManifest = {
  rsp: 1,
  id: "demo.videoext",
  name: "Demo VideoExt",
  version: "1.0.0",
  kind: ["anime"],
  engine: "js",
  entry: "source.js",
  baseUrl: "https://demo.test",
  capabilities: ["search", "chapters", "play"],
};

const SCRIPT = `
// ignore
import { VideoDetail } from "./core/core/uzVideo.js";
// ignore
const appConfig = { webSite: "https://demo.test" };

async function searchVideo(args) {
  const res = await req(appConfig.webSite + "/search?wd=" + encodeURIComponent(args.searchWord));
  const $ = cheerio.load(res.data);
  const data = $("div.list > div.item").map((index, element) => ({
    vod_id: $(element).find("a").attr("href"),
    vod_name: $(element).find(".name").text(),
    vod_pic: $(element).find("img").attr("data-src"),
  })).get();
  return JSON.stringify({ data });
}

async function getVideoDetail(args) {
  return JSON.stringify({
    data: {
      vod_name: "星海",
      vod_blurb: "一段简介",
      vod_class: "冒险,奇幻",
      vod_director: "导演甲",
      vod_play_from: "线路1$$$线路2",
      vod_play_url: "第1集$https://demo.test/c1.m3u8#第2集$https://demo.test/c2.m3u8$$$第1集$https://cdn.test/d1.mp4",
    },
  });
}

async function getVideoPlayUrl(args) {
  return JSON.stringify({
    urls: [{ name: "高清", url: args.url, headers: { Referer: "https://demo.test" } }],
  });
}
`;

const SEARCH_HTML = `
<html><body>
  <div class="list">
    <div class="item">
      <a href="/vod/1"><span class="name">星海 &amp; 山海</span><img data-src="/img/1.jpg"></a>
    </div>
    <div class="item">
      <a href="/vod/2"><span class="name">风起</span><img data-src="https://cdn.test/2.jpg"></a>
    </div>
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
      return JSON.stringify({ ok: false, error: `unexpected: ${options.url}` });
    },
  });
}

test("视频源识别：显式函数 / BaseCode1，且不误伤漫画源 与 createSource", () => {
  assert.equal(looksLikeVideo(SCRIPT), true);
  assert.equal(looksLikeVideo("//#BaseCode1#"), true);
  assert.equal(looksLikeVideo("class X extends ComicSource {}"), false);
  assert.equal(looksLikeVideo("function createSource(rsp) { return {}; }"), false);
});

test("视频源：剥离 import 块，搜索走 req + cheerio", async () => {
  const source = createVideoSource({ manifest, script: SCRIPT }, { host: host() });
  const page = await source.search!("星海", 1);

  assert.equal(page.items.length, 2);
  assert.deepEqual(page.items[0], {
    ref: { sourceId: "demo.videoext", workId: "/vod/1", kind: "anime" },
    title: "星海 & 山海",
    author: undefined,
    cover: "https://demo.test/img/1.jpg",
    description: undefined,
    tags: undefined,
    status: undefined,
  });
  assert.equal(page.items[1]?.cover, "https://cdn.test/2.jpg");
});

test("视频源：detail 补简介 / 标签 / 作者", async () => {
  const source = createVideoSource({ manifest, script: SCRIPT }, { host: host() });
  const work = await source.detail!({
    sourceId: "demo.videoext",
    workId: "/vod/1",
    kind: "anime",
  });
  assert.equal(work.description, "一段简介");
  assert.deepEqual(work.tags, ["冒险", "奇幻"]);
  assert.equal(work.author, "导演甲");
});

test("视频源：章节按 vod_play_from / vod_play_url 展开", async () => {
  const source = createVideoSource({ manifest, script: SCRIPT }, { host: host() });
  const chapters = await source.chapters!({
    sourceId: "demo.videoext",
    workId: "/vod/1",
    kind: "anime",
  });

  assert.deepEqual(
    chapters.map((chapter) => [chapter.title, chapter.group]),
    [
      ["线路1 · 第1集", "线路1"],
      ["线路1 · 第2集", "线路1"],
      ["线路2 · 第1集", "线路2"],
    ],
  );
  assert.ok(chapters[0]?.id.includes("https://demo.test/c1.m3u8"));
});

test("视频源 源：播放地址映射为 Playback（hls / mp4 + 请求头）", async () => {
  const source = createVideoSource({ manifest, script: SCRIPT }, { host: host() });
  const chapters = await source.chapters!({
    sourceId: "demo.videoext",
    workId: "/vod/1",
    kind: "anime",
  });
  const playbacks = await source.play!(chapters[0]!);

  assert.equal(playbacks.length, 1);
  assert.equal(playbacks[0]?.url, "https://demo.test/c1.m3u8");
  assert.equal(playbacks[0]?.type, "hls");
  assert.deepEqual(playbacks[0]?.headers, { Referer: "https://demo.test" });
});

// ── uzVideo sniffer（2026-10-28）────────────────────────────────────
// 上游 `getVideoPlayUrl` 有时给播放页而不是直链：非直链交给宿主 WebView 嗅探。
const PAGE_SCRIPT = `
async function getVideoPlayUrl(args) {
  return JSON.stringify({ data: "https://play.demo.test/play/1" });
}
`;

const CHAPTER = { id: "https://play.demo.test/play/1", workId: "/vod/1", title: "第1集", index: 0 };

function sniffHost(urls: string[]) {
  return createMockHost({
    allowEval: true,
    engineRequest: () => JSON.stringify({ ok: false, error: "no net in this test" }),
    sniffMedia: () => urls,
  });
}

test("视频源 源：播放页地址交给宿主嗅探成直链（hls / mp4 各按扩展名）", async () => {
  const source = createVideoSource({ manifest, script: PAGE_SCRIPT }, {
    host: sniffHost(["https://cdn.test/x.m3u8", "https://cdn.test/y.mp4"]),
  });
  const playbacks = await source.play!(CHAPTER);

  assert.deepEqual(
    playbacks.map((item) => [item.url, item.type]),
    [
      ["https://cdn.test/x.m3u8", "hls"],
      ["https://cdn.test/y.mp4", "mp4"],
    ],
  );
});

test("视频源 源：嗅探不到直链时保留原地址（不让能播的来源回归）", async () => {
  const source = createVideoSource({ manifest, script: PAGE_SCRIPT }, { host: sniffHost([]) });
  const playbacks = await source.play!(CHAPTER);

  assert.deepEqual(
    playbacks.map((item) => [item.url, item.type]),
    [["https://play.demo.test/play/1", "mp4"]],
  );
});

test("视频源 源：直链不触发嗅探", async () => {
  let calls = 0;
  const base = createMockHost({
    allowEval: true,
    engineRequest: () => JSON.stringify({ ok: false, error: "no net in this test" }),
    sniffMedia: () => {
      calls += 1;
      return ["https://cdn.test/never.m3u8"];
    },
  });
  const source = createVideoSource({ manifest, script: SCRIPT }, { host: base });
  const chapters = await source.chapters!({
    sourceId: "demo.videoext",
    workId: "/vod/1",
    kind: "anime",
  });
  const playbacks = await source.play!(chapters[0]!);
  assert.equal(calls, 0);
  assert.equal(playbacks[0]?.url, "https://demo.test/c1.m3u8");
});

const HOME_SCRIPT = `
async function getHome() {
  return JSON.stringify({
    data: [
      { vod_id: "/home/1", vod_name: "首页一", vod_pic: "/img/home-1.jpg" },
      { vod_id: "/home/2", vod_name: "首页二" },
    ],
  });
}
`;

const CLASS_SCRIPT = `
async function getClassList() {
  return JSON.stringify({
    class: [
      { type_id: "1", type_name: "电影" },
      { type_id: "2", type_name: "剧集" },
    ],
  });
}

async function getVideoList(args) {
  return JSON.stringify({
    data: [
      {
        vod_id: args.type_id + "-" + args.page,
        vod_name: "列表 " + args.type_id + " P" + args.page,
        vod_pic: "/img/" + args.type_id + "-" + args.page + ".jpg",
      },
    ],
  });
}
`;

test("视频源 源：getHome 拉取总览并映射为 Work", async () => {
  const source = createVideoSource({ manifest, script: HOME_SCRIPT }, { host: host() });
  const page = await source.explore!();

  assert.equal(page.items.length, 2);
  assert.equal(page.items[0]?.title, "首页一");
  assert.equal(page.items[0]?.ref.workId, "/home/1");
  assert.equal(page.items[0]?.cover, "https://demo.test/img/home-1.jpg");
});

test("视频源 源：getHome 为空时退回分类列表", async () => {
  const script = `async function getHome() {
  return JSON.stringify({ data: [] });
}
${CLASS_SCRIPT}`;
  const source = createVideoSource({ manifest, script }, { host: host() });
  const page = await source.explore!();
  assert.deepEqual(
    page.items.map((item) => item.ref.workId),
    ["1-1", "2-1"],
  );
});

test("视频源 源：getClassList / getVideoList 聚合总览，并支持分区与分页", async () => {
  const source = createVideoSource({ manifest, script: CLASS_SCRIPT }, { host: host() });
  const overview = await source.explore!();
  assert.deepEqual(
    overview.items.map((item) => [item.ref.workId, item.title]),
    [
      ["1-1", "列表 1 P1"],
      ["2-1", "列表 2 P1"],
    ],
  );

  const section = await source.explore!("2", 3);
  assert.deepEqual(
    section.items.map((item) => [item.ref.workId, item.title, item.cover]),
    [["2-3", "列表 2 P3", "https://demo.test/img/2-3.jpg"]],
  );
});

test("视频源 源：没有浏览函数时 explore 给出 unsupported", async () => {
  const source = createVideoSource({ manifest, script: SCRIPT }, { host: host() });
  await assert.rejects(
    () => source.explore!(),
    (error: unknown) =>
      error instanceof Error && "code" in error && (error as { code: string }).code === "unsupported",
  );
});

// 真实 uzVideo 扩展（type 100/101）不直接拼对象字面量，而是 `new RepVideoList()`、
// `new VideoDetail()` 后逐字段赋值；`req` 回来用 `$(…)[0].attribs.href` 这种 cheerio 原生写法，
// 且常带 `console.log`。这些是线上脚本的公共形态，缺一样就整源跑不起来。
const REP_SCRIPT = `
// ignore
import { VideoDetail } from "./core/core/uzVideo.js";
// ignore
const appConfig = { webSite: "https://demo.test" };

async function searchVideo(args) {
  const backData = new RepVideoList();
  try {
    const pro = await req(appConfig.webSite + "/search?wd=" + encodeURIComponent(args.searchWord));
    const $ = cheerio.load(pro.data);
    $("div.list > div.item").each((index, element) => {
      const video = new VideoDetail();
      video.vod_id = $(element).find("a")[0].attribs.href;
      video.vod_name = $(element).find("span.name")[0].children[0].data;
      video.vod_pic = $(element).find("img")[0].attribs["data-src"];
      backData.data.push(video);
    });
    console.log("search", backData.data.length);
  } catch (error) {
    backData.error = error.toString();
  }
  return JSON.stringify(backData);
}

async function getVideoDetail(args) {
  const backData = new RepVideoDetail();
  try {
    const detail = new VideoDetail();
    detail.vod_id = args.url;
    detail.vod_name = "星海";
    detail.vod_content = "一段简介";
    detail.vod_play_from = "线路1";
    detail.vod_play_url = "第1集$https://cdn.test/1.m3u8";
    backData.data = detail;
  } catch (error) {
    backData.error = error.toString();
  }
  return JSON.stringify(backData);
}

async function getVideoPlayUrl(args) {
  const backData = new RepVideoPlayUrl();
  backData.urls = [{ name: "高清", url: args.url, headers: { Referer: appConfig.webSite } }];
  return JSON.stringify(backData);
}
`;

test("视频源：uzVideo 数据类（Rep* / VideoDetail）与 console 可用", async () => {
  const source = createVideoSource({ manifest, script: REP_SCRIPT }, { host: host() });

  const page = await source.search!("星海", 1);
  assert.deepEqual(
    page.items.map((item) => [item.ref.workId, item.title, item.cover]),
    [
      ["/vod/1", "星海 & 山海", "https://demo.test/img/1.jpg"],
      ["/vod/2", "风起", "https://cdn.test/2.jpg"],
    ],
  );

  const detail = await source.detail!({ sourceId: manifest.id, workId: "/vod/1", kind: "anime" });
  assert.equal(detail.title, "星海");
  assert.equal(detail.description, "一段简介");

  const chapters = await source.chapters!({ sourceId: manifest.id, workId: "/vod/1", kind: "anime" });
  assert.deepEqual(
    chapters.map((chapter) => [chapter.title, chapter.index]),
    [["第1集", 0]],
  );

  const playbacks = await source.play!(chapters[0]!);
  assert.deepEqual(playbacks, [
    { url: "https://cdn.test/1.m3u8", type: "hls", headers: { Referer: "https://demo.test" } },
  ]);
});

test("视频源：Crypto 是 CryptoJS 兼容层（MD5 / SHA1 / SHA256 / Base64 / AES）", async () => {
  const script = `
async function searchVideo() {
  const backData = new RepVideoList();
  const video = new VideoDetail();
  video.vod_id = "/crypto/1";
  const key = Crypto.enc.Hex.parse("2b7e151628aed2a6abf7158809cf4f3c");
  const iv = Crypto.enc.Hex.parse("000102030405060708090a0b0c0d0e0f");
  const plain = Crypto.enc.Hex.parse("6bc1bee22e409f96e93d7e117393172a");
  const cipher = Crypto.AES.encrypt(plain, key, {
    iv: iv,
    mode: Crypto.mode.CBC,
    padding: Crypto.pad.NoPadding,
  });
  const round = Crypto.AES.decrypt({ ciphertext: cipher.ciphertext }, key, {
    iv: iv,
    mode: Crypto.mode.CBC,
    padding: Crypto.pad.NoPadding,
  }).toString(Crypto.enc.Hex);
  const utf8Key = Crypto.enc.Utf8.parse("1234567890123456");
  const utf8Iv = Crypto.enc.Utf8.parse("abcdefghijklmnop");
  const textCipher = Crypto.AES.encrypt("星海", utf8Key, {
    iv: utf8Iv,
    mode: Crypto.mode.CBC,
    padding: Crypto.pad.Pkcs7,
  });
  const textRound = Crypto.AES.decrypt(textCipher, utf8Key, {
    iv: utf8Iv,
    mode: Crypto.mode.CBC,
    padding: Crypto.pad.Pkcs7,
  }).toString(Crypto.enc.Utf8);
  const ecbKey = Crypto.enc.Utf8.parse("f5d965df75336270");
  const ecbCipher = Crypto.AES.encrypt("pid-1", ecbKey, {
    mode: Crypto.mode.ECB,
    padding: Crypto.pad.Pkcs7,
  });
  const ecbRound = Crypto.AES.decrypt({ ciphertext: ecbCipher.ciphertext }, ecbKey, {
    mode: Crypto.mode.ECB,
    padding: Crypto.pad.Pkcs7,
  }).toString(Crypto.enc.Utf8);
  video.vod_name = [
    Crypto.MD5("abc").toString(),
    Crypto.SHA1("abc").toString(),
    Crypto.SHA256("abc").toString(),
    Crypto.enc.Base64.stringify(Crypto.enc.Utf8.parse("星海")),
    Crypto.enc.Base64.stringify(Crypto.lib.WordArray.create(new Uint8Array([1, 2, 3]))),
    cipher.ciphertext.toString(Crypto.enc.Hex),
    round,
    textRound,
    ecbRound,
  ].join("|");
  backData.data.push(video);
  return JSON.stringify(backData);
}

async function getVideoDetail() {
  return JSON.stringify(new RepVideoDetail());
}
`;
  const source = createVideoSource({ manifest, script }, { host: host() });
  const page = await source.search!("x", 1);

  assert.equal(
    page.items[0]?.title,
    [
      "900150983cd24fb0d6963f7d28e17f72",
      "a9993e364706816aba3e25717850c26c9cd0d89d",
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
      "5pif5rW3",
      "AQID",
      // NIST AES-128-CBC 向量（NoPadding）
      "7649abac8119b246cee98e9b12e9197d",
      "6bc1bee22e409f96e93d7e117393172a",
      "星海",
      "pid-1",
    ].join("|"),
  );
});

test("视频源 源：安装前识别不兼容形态（BaseCode1 / 加密 / 未知）", () => {
  assert.equal(videoUnsupportedReason(SCRIPT), null);
  assert.equal(videoUnsupportedReason(REP_SCRIPT), null);

  const basecode1 = "// ignore\n//@name:x\n//@version:1\n// ignore\nconst webSite = 'https://a.test';\n//#BaseCode1#\n";
  assert.equal(videoUnsupportedReason(basecode1), "basecode1");

  const encrypted = "//@name:[采] X\n//@codeID:ra5Z3wtXKshzSMK2\nasync function searchVideo() {}\n";
  assert.equal(videoUnsupportedReason(encrypted), "encrypted");

  // 模板占位的 `//@codeID:` 是空值，不算加密源。
  const emptyCodeId =
    "//@codeID:\nasync function searchVideo(args) { return '{}'; }\nasync function getVideoDetail(args) { return '{}'; }\n";
  assert.equal(videoUnsupportedReason(emptyCodeId), null);

  assert.equal(videoUnsupportedReason("class X extends ComicSource {}"), "unknown");
});

test("视频源 源：分类浏览按上游约定把分类标识放在 args.url", async () => {
  const script = `
async function getClassList() {
  return JSON.stringify({ data: [{ type_id: "7", type_name: "动漫" }] });
}

async function getVideoList(args) {
  return JSON.stringify({ data: [{ vod_id: args.url + "-" + args.page, vod_name: "分类 " + args.url }] });
}
`;
  const source = createVideoSource({ manifest, script }, { host: host() });
  const page = await source.explore!();
  assert.deepEqual(
    page.items.map((item) => [item.ref.workId, item.title]),
    [["7-1", "分类 7"]],
  );
});

test("视频源 源：BaseCode1 模板给出可读错误", () => {
  assert.throws(
    () =>
      createVideoSource(
        { manifest, script: "//#BaseCode1#\nconst webSite = 'https://demo.test';" },
        { host: host() },
      ),
    (error: unknown) =>
      error instanceof Error && "code" in error && (error as { code: string }).code === "unsupported",
  );
});
