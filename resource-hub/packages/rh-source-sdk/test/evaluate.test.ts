import assert from "node:assert/strict";
import { test } from "node:test";

import {
  applyRegex,
  buildRequestUrl,
  extractJsonRows,
  interpolate,
  jsonPathValues,
} from "../src/evaluate.js";
import { RspError } from "../src/contract.js";

test("interpolate：替换已知占位符，未知原样保留", () => {
  assert.equal(
    interpolate("https://x.test/s?q=@keyword&p=@page&x=@unknown", {
      keyword: "海贼",
      page: 2,
    }),
    "https://x.test/s?q=海贼&p=2&x=@unknown",
  );
});

test("applyRegex：/pattern/flags 取第一个捕获组，裸 pattern 取整段", () => {
  assert.equal(applyRegex("第12话 风起", "/第(\\d+)话/"), "12");
  assert.equal(applyRegex("ID=abc123", "abc\\d+"), "abc123");
  assert.equal(applyRegex("无匹配", "/x/"), undefined);
});

test("buildRequestUrl：相对路径按 baseUrl 解析并拼 query", () => {
  const url = buildRequestUrl(
    "https://api.test",
    {
      method: "GET",
      url: "/search",
      query: { q: "@keyword", page: "@page", empty: "" },
    },
    { keyword: "海贼", page: 3 },
  );
  assert.equal(url, "https://api.test/search?q=%E6%B5%B7%E8%B4%BC&page=3");
});

test("buildRequestUrl：绝对 URL 原样保留；缺 baseUrl 抛 parse", () => {
  assert.equal(
    buildRequestUrl(undefined, { method: "GET", url: "https://a.test/x" }, {}),
    "https://a.test/x",
  );
  assert.throws(
    () => buildRequestUrl(undefined, { method: "GET", url: "/x" }, {}),
    (error: unknown) => error instanceof RspError && error.code === "parse",
  );
});

test("jsonPathValues：嵌套对象、数组下标、通配符、带引号键", () => {
  const data = {
    data: {
      list: [
        { title: "A", tags: ["x", "y"] },
        { title: "B", tags: [] },
      ],
    },
    "odd-key": 1,
  };
  assert.deepEqual(jsonPathValues(data, "$.data.list[*].title"), ["A", "B"]);
  assert.deepEqual(jsonPathValues(data, "$.data.list[0].tags[*]"), ["x", "y"]);
  assert.deepEqual(jsonPathValues(data, "$['odd-key']"), [1]);
  assert.deepEqual(jsonPathValues(data, "data.list"), [data.data.list]);
});

test("extractJsonRows：列表 + 字段 regex/template，空行被过滤", () => {
  const payload = {
    items: [
      {
        name: "作品 A",
        url: "/comic/1",
        cover: "https://img.test/1.jpg",
      },
      { name: "", url: "", cover: "" },
    ],
  };
  const rows = extractJsonRows(
    payload,
    {
      request: { method: "GET", url: "/search" },
      list: { jsonPath: "$.items[*]" },
      fields: {
        title: { selector: { jsonPath: "name" }, regex: "作品 (.+)" },
        url: { selector: { jsonPath: "url" }, template: "https://a.test@value" },
        cover: { selector: { jsonPath: "cover" } },
      },
    },
    { keyword: "A" },
  );
  assert.deepEqual(rows, [
    { title: "A", url: "https://a.test/comic/1", cover: "https://img.test/1.jpg" },
  ]);
});

test("jsonPathValues：递归 JSONPath 抛 unsupported", () => {
  assert.throws(
    () => jsonPathValues({}, "$..title"),
    (error: unknown) => error instanceof RspError && error.code === "unsupported",
  );
});
