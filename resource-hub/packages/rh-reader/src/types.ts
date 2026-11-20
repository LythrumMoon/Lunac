// 阅读器共享契约 —— 插件（Windows）与后续 iOS / Android 独立应用共用。
// 这里只放纯状态与几何，不依赖 DOM，方便单测与移动端复用。

export type ReaderMode = "vertical" | "paged" | "webtoon";

/**
 * 裁剪模式（按顺序循环）：
 *  - `off`：原图，保留阅读边距；
 *  - `margin`：收紧页边距；
 *  - `trim`：收紧边距并自动裁掉图片白边；
 *  - `split`：在 `trim` 基础上把跨页大图拆成左右两半。
 */
export type ReaderCropMode = "off" | "margin" | "trim" | "split";

export type TapZone = "prev" | "next" | "toggle";

export type PageKind = "text" | "html" | "placeholder" | "image";

export interface ReaderPage {
  id: string;
  kind: PageKind;
  /** 文本页正文；图片页可放 URL。 */
  content?: string;
  /** 图片页说明 / 占位文案。 */
  caption?: string;
}

export interface ReaderChapter {
  id: string;
  title: string;
  pages: ReaderPage[];
  /** 内容尚未加载（跨章按需加载）；阅读器进入该章时调用 `loadChapter`。 */
  lazy?: boolean;
}

export interface ReaderSnapshot {
  chapterIndex: number;
  pageIndex: number;
  pageCount: number;
  chapterCount: number;
  mode: ReaderMode;
  speed: number;
  locked: boolean;
  barsVisible: boolean;
  cropMode: ReaderCropMode;
  autoplay: boolean;
}

export interface ReaderInitialState {
  chapterIndex?: number;
  pageIndex?: number;
  mode?: ReaderMode;
  speed?: number;
  /** 兼容旧调用：`true` = `margin`，`false` = `off`。 */
  cropMode?: ReaderCropMode | boolean;
}

export const READER_SPEEDS = [0.5, 1, 1.5, 2, 3] as const;

export const READER_MODES: ReaderMode[] = ["vertical", "paged", "webtoon"];

export const READER_CROP_MODES: ReaderCropMode[] = ["off", "margin", "trim", "split"];

/**
 * 点击分区：
 *  - 上下模式（vertical / webtoon）：上 35% 上一页，中间 30% 唤出/收起双框，下 35% 下一页；
 *  - 左右模式（paged）：左 35% 上一页，中间 30% 唤出/收起双框，右 35% 下一页。
 *
 * 坐标单位为逻辑像素；宽高必须 > 0，否则按 toggle 处理。
 */
export function resolveTapZone(
  x: number,
  y: number,
  width: number,
  height: number,
  mode: ReaderMode,
): TapZone {
  if (width <= 0 || height <= 0) return "toggle";
  if (mode === "paged") {
    const ratio = x / width;
    if (ratio < 0.35) return "prev";
    if (ratio >= 0.65) return "next";
    return "toggle";
  }
  const ratio = y / height;
  if (ratio < 0.35) return "prev";
  if (ratio >= 0.65) return "next";
  return "toggle";
}
