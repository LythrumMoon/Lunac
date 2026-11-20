import type { SourceKind } from "@resource-hub/source-sdk";

/** 本地演示数据 —— 全部为自写内容，不包含任何第三方作品。 */
export interface DemoChapter {
  id: string;
  title: string;
  content?: string;
  pages?: number;
}

export interface DemoWork {
  id: string;
  title: string;
  kind: SourceKind;
  author: string;
  description: string;
  tags: string[];
  /** 演示封面：仅用于展示总览 / 书架的封面卡形态。 */
  cover?: string;
  status?: "ongoing" | "completed" | "unknown";
  updatedAt?: number;
  chapters: DemoChapter[];
}

const STAR_CHAPTERS: DemoChapter[] = [
  {
    id: "star-1",
    title: "第一章 · 夜航",
    content:
      "港口的灯在雾里散成一圈一圈的暖色。林晚把最后一箱书搬上船，回头看了一眼岸上。\n\n" +
      "没有送行的人。只有风把告示栏上的纸吹得哗哗响。她把缆绳解开，发动机低低地咳了两声，像在确认自己还活着。\n\n" +
      "“去星海。”她在航海日志上写下这三个字，然后把笔夹回封面。海面很平，平得像一张还没落笔的纸。",
  },
  {
    id: "star-2",
    title: "第二章 · 无信号的第七天",
    content:
      "第七天，无线电里只剩下静电。林晚开始用铅笔在舱壁上画航线，每一段都标上日期。她发现自己在等一个不会来的回复。\n\n" +
      "夜里她打开那本旧书，书页里夹着一张褪色的照片。照片背面写着一行字：如果迷路，就沿着北斗走。\n\n" +
      "她抬头，云层正好裂开一道缝。",
  },
  {
    id: "star-3",
    title: "第三章 · 回信",
    content:
      "信是在清晨到的。不是无线电，是一只撞在舷窗上的纸鸟。纸鸟的翅膀上写着一串坐标，字迹被海水晕开，却仍能辨认。\n\n" +
      "林晚把坐标抄进日志，忽然笑了。她终于知道，星海不是目的地，是有人愿意在另一端等你的意思。",
  },
];

const MOUNTAIN_CHAPTERS: DemoChapter[] = [
  { id: "mountain-1", title: "第 1 话 · 山口", pages: 6 },
  { id: "mountain-2", title: "第 2 话 · 云层以下", pages: 8 },
  { id: "mountain-3", title: "第 3 话 · 旧桥", pages: 5 },
  { id: "mountain-4", title: "第 4 话 · 回声", pages: 7 },
];

const WIND_CHAPTERS: DemoChapter[] = [
  { id: "wind-1", title: "第 1 集 · 起风", pages: undefined },
  { id: "wind-2", title: "第 2 集 · 逆风", pages: undefined },
  { id: "wind-3", title: "第 3 集 · 风停之后", pages: undefined },
];

function demoCover(prompt: string): string {
  return `https://coresg-normal.trae.ai/api/ide/v1/text_to_image?prompt=${encodeURIComponent(prompt)}&image_size=portrait_4_3`;
}

export const DEMO_WORKS: DemoWork[] = [
  {
    id: "demo-star",
    title: "星海来信",
    kind: "novel",
    author: "示例作者",
    description: "一个人、一艘旧船和一封迟到的信。",
    tags: ["科幻", "远航", "短篇"],
    cover: demoCover(
      "book cover art, a lone small boat on a misty sea at night, warm harbor lights in the distance, muted teal and amber palette, cinematic, minimal, portrait composition",
    ),
    status: "completed",
    updatedAt: Date.UTC(2026, 8, 12),
    chapters: STAR_CHAPTERS,
  },
  {
    id: "demo-mountain",
    title: "山与海",
    kind: "comic",
    author: "示例作者",
    description: "在山口与海风之间，寻找一张旧地图的尽头。",
    tags: ["冒险", "治愈", "条漫"],
    cover: demoCover(
      "illustrated comic cover, a mountain ridge meeting the sea, soft morning mist, clean line art, muted sage and slate palette, portrait composition",
    ),
    status: "ongoing",
    updatedAt: Date.UTC(2026, 9, 2),
    chapters: MOUNTAIN_CHAPTERS,
  },
  {
    id: "demo-wind",
    title: "风起之时",
    kind: "anime",
    author: "示例作者",
    description: "风从城墙之上越过，带来一个夏天的消息。",
    tags: ["奇幻", "番剧"],
    cover: demoCover(
      "anime key visual, wind sweeping over an ancient stone city wall, drifting clouds, warm summer light, painterly, portrait composition",
    ),
    status: "ongoing",
    updatedAt: Date.UTC(2026, 7, 21),
    chapters: WIND_CHAPTERS,
  },
];

/**
 * 内置示例的本地检索。
 *
 * `extraMatch` 只用于**额外命中**（目前是拼音）：原来的标题 / 作者 / 简介 / 标签子串
 * 匹配一字未改 —— 没有宿主 pinyin 能力、或拼音缓存未就绪时，行为与以前逐字一致。
 */
export function searchDemoWorks(
  query: string,
  extraMatch?: (work: DemoWork) => boolean,
): DemoWork[] {
  const q = query.trim().toLowerCase();
  if (!q) return DEMO_WORKS;
  return DEMO_WORKS.filter(
    (work) =>
      [work.title, work.author, work.description, ...work.tags]
        .join(" ")
        .toLowerCase()
        .includes(q) || (extraMatch ? extraMatch(work) : false),
  );
}

export function demoWorkById(id: string): DemoWork | undefined {
  return DEMO_WORKS.find((work) => work.id === id);
}
