// 拼音匹配（2026-10-22，阅读插件「拼音检索」的共享内核）。
//
// 宿主负责把标题转成全拼 / 首字母（`PinyinApi`），这里只做**纯函数**匹配：
//   · 查询归一化 = 小写 + 只留字母数字（中文 / 标点 / 空格全部去掉）；
//   · 全拼与首字母**都**参与：`douluo` 与 `dl` 都能命中「斗罗大陆」；
//   · 等级只分三档：0 不匹配 / 1 前缀 / 2 包含 —— 由调用方决定它在整体排序里的位置。
//
// 为什么单独成文件：移动端没有宿主 pinyin 能力时可以整块跳过；有能力的宿主也只共享
// 这一个口径，避免「插件里一份、SDK 里一份」的匹配规则漂移。

import type { PinyinEntry } from "./host.js";

/** 只保留 ASCII 字母数字并小写（与宿主编码口径配套）。 */
export function normalizeLatin(text: string): string {
  return text.toLowerCase().replace(/[^a-z0-9]/g, "");
}

export type PinyinMatch = 0 | 1 | 2;

/**
 * 查询与一条拼音编码的匹配等级。
 *
 * 注意：**不做「空查询 = 全匹配」** —— 空查询由调用方按「无过滤」处理，这里返回 0，
 * 免得所有条目都被算成命中。
 */
export function matchPinyin(
  query: string,
  entry: PinyinEntry | undefined | null,
): PinyinMatch {
  if (!entry) return 0;
  const q = normalizeLatin(query);
  if (!q) return 0;
  const full = normalizeLatin(entry.full);
  const initials = normalizeLatin(entry.initials);
  if ((full && full.startsWith(q)) || (initials && initials.startsWith(q))) return 1;
  if ((full && full.includes(q)) || (initials && initials.includes(q))) return 2;
  return 0;
}
