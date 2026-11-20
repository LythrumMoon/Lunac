import { test } from "node:test";
import assert from "node:assert/strict";

import {
  bookmarkKey,
  findBookmark,
  sortBookmarks,
  toggleBookmark,
  type ReaderBookmark,
} from "../src/index.js";

function bookmark(chapterIndex: number, pageIndex: number): ReaderBookmark {
  return {
    chapterIndex,
    pageIndex,
    chapterTitle: `第 ${chapterIndex + 1} 章`,
    excerpt: `第 ${pageIndex + 1} 页`,
    createdAt: 1000 + chapterIndex * 10 + pageIndex,
  };
}

test("书签：同一位置再次添加会移除，其它位置不受影响", () => {
  const first = toggleBookmark([], bookmark(0, 1));
  assert.equal(first.length, 1);

  const second = toggleBookmark(first, bookmark(1, 0));
  assert.equal(second.length, 2);

  const removed = toggleBookmark(second, bookmark(0, 1));
  assert.deepEqual(
    removed.map((item) => bookmarkKey(item.chapterIndex, item.pageIndex)),
    ["1:0"],
  );
});

test("书签：按章 / 页查找，位置不匹配返回 undefined", () => {
  const list = [bookmark(0, 2), bookmark(1, 0)];
  assert.equal(findBookmark(list, 1, 0)?.chapterTitle, "第 2 章");
  assert.equal(findBookmark(list, 1, 1), undefined);
});

test("书签：排序按阅读位置，且不修改入参", () => {
  const list = [bookmark(2, 0), bookmark(0, 5), bookmark(1, 1)];
  const sorted = sortBookmarks(list);
  assert.deepEqual(
    sorted.map((item) => bookmarkKey(item.chapterIndex, item.pageIndex)),
    ["0:5", "1:1", "2:0"],
  );
  assert.deepEqual(
    list.map((item) => bookmarkKey(item.chapterIndex, item.pageIndex)),
    ["2:0", "0:5", "1:1"],
  );
});
