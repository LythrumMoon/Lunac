// 阅读器书签 —— 纯数据与纯函数，UI 与移动端共用。
// 书签只记录「章索引 + 页索引」，标题 / 摘要用于列表展示，不参与定位。

export interface ReaderBookmark {
  chapterIndex: number;
  pageIndex: number;
  chapterTitle: string;
  /** 添加时的正文摘要（图片页为空）。 */
  excerpt?: string;
  createdAt: number;
}

export function bookmarkKey(chapterIndex: number, pageIndex: number): string {
  return `${chapterIndex}:${pageIndex}`;
}

export function findBookmark(
  bookmarks: readonly ReaderBookmark[],
  chapterIndex: number,
  pageIndex: number,
): ReaderBookmark | undefined {
  return bookmarks.find(
    (bookmark) => bookmark.chapterIndex === chapterIndex && bookmark.pageIndex === pageIndex,
  );
}

/** 同一位置再次添加 = 移除；返回新数组，不修改入参。 */
export function toggleBookmark(
  bookmarks: readonly ReaderBookmark[],
  bookmark: ReaderBookmark,
): ReaderBookmark[] {
  const exists = findBookmark(bookmarks, bookmark.chapterIndex, bookmark.pageIndex);
  if (exists) {
    return bookmarks.filter(
      (item) =>
        item.chapterIndex !== bookmark.chapterIndex || item.pageIndex !== bookmark.pageIndex,
    );
  }
  return [...bookmarks, bookmark];
}

/** 按阅读位置排序（章 → 页），不改入参。 */
export function sortBookmarks(bookmarks: readonly ReaderBookmark[]): ReaderBookmark[] {
  return [...bookmarks].sort(
    (a, b) => a.chapterIndex - b.chapterIndex || a.pageIndex - b.pageIndex,
  );
}
