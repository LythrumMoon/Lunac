import assert from "node:assert/strict";
import { test } from "node:test";

import { matchPinyin, normalizeLatin } from "../src/pinyin.js";

const DL = { full: "douluo dalu", initials: "dldl" };

test("normalizeLatin 去掉标点与空白并小写", () => {
  assert.equal(normalizeLatin("Hello, 世界! 42"), "hello42");
  assert.equal(normalizeLatin("   "), "");
});

test("全拼前缀与包含都命中", () => {
  assert.equal(matchPinyin("douluo", DL), 1);
  assert.equal(matchPinyin("dalu", DL), 2);
  assert.equal(matchPinyin("Douluo Dalu", { full: "douluodalu", initials: "dldl" }), 1);
});

test("首字母前缀 / 包含命中", () => {
  assert.equal(matchPinyin("dl", DL), 1);
  assert.equal(matchPinyin("ld", DL), 2);
});

test("不相关查询与空查询不匹配", () => {
  assert.equal(matchPinyin("xyz", DL), 0);
  assert.equal(matchPinyin("", DL), 0);
  assert.equal(matchPinyin("斗罗", DL), 0, "中文查询不走拼音匹配");
});

test("缺失编码时安全降级为不匹配", () => {
  assert.equal(matchPinyin("dl", undefined), 0);
  assert.equal(matchPinyin("dl", null), 0);
  assert.equal(matchPinyin("dl", { full: "", initials: "" }), 0);
});
