// ── 桌宠输出框「打字机」的纯逻辑（2026-10-12）─────────────────────────
// 本模块**只做纯计算**（无 DOM / 无 tauri / 无副作用），因此可被 pet.ts 内联进插件包，
// 也可被 `scripts/pet-typewriter-selftest.mjs` 用 node 直接跑单测（见该脚本）。
//
// 三条口径（用户 2026-10-12 原话，逐字落实，勿自行发挥）：
//   · **每秒 2 个中文字、2 个整体单词**（默认速率，可调）；
//   · **标点迅速跳过** —— 标点不占自己的节拍，吸附到前一个单位一并揭示；
//   · **跳过换行符** —— 揭示前把换行归一（换行 → 单个空格、连续空白折叠、首尾空行丢弃），
//     从根上避免 `white-space:pre-wrap` 把换行渲染成「上方 / 下方空白框」。

/** 揭示速率的**可调范围**（单位 = 全角字符 / 西文词，每秒）。**只**用于打字机的逐字动画节拍
 *  （2026-10-12 起与语音语速相互独立，不再折算到宿主 `PetVoiceConfig.speed`）。 */
export const RATE_MIN = 1;
export const RATE_MAX = 4;
/** 默认速率 = 用户口径的「每秒 2 个」。 */
export const RATE_DEFAULT = 2;

/** 逐字动画的**时长上限**（毫秒）。按预估总时长封顶：超过就直接整体显示全文，
 *  避免一段 2000 字的回复按 2 单位/秒跑上十几分钟（用户要求「给上限保护」）。 */
export const ANIM_MAX_MS = 120_000;

/** 速率越界 / 非法值 → 夹到 `[RATE_MIN, RATE_MAX]`；保留两位小数（拉条步长 0.5，够用）。 */
export function clampRate(rate: number): number {
  if (typeof rate !== "number" || !Number.isFinite(rate)) return RATE_DEFAULT;
  const r = Math.round(rate * 100) / 100;
  return Math.min(RATE_MAX, Math.max(RATE_MIN, r));
}

/** **换行归一**（用户口径「跳过换行符」）：把 `\r` / `\n` / `\t` 等一切空白折叠成单个空格，
 *  再去掉首尾空白（= 丢弃首尾空行）。空串 / 纯空白 → 空串。
 *  这是 A（修上下空白框）与 B（逐字揭示）共用的唯一入口。 */
export function normalizeReplyText(raw: string): string {
  return raw.replace(/\s+/g, " ").trim();
}

/** 全角字符（中日韩文字 / 假名 / 谚文 / 全角形式）区段：**每个各占 1 个揭示单位**。
 *  用显式区段而不是 `\p{Script=Han}` —— 后者不含假名 / 谚文 / 全角标点，
 *  而用户口径是「中文 / 日文等**全角**字符每秒 2 个」。 */
const FULLWIDTH_RE =
  /[\u1100-\u11FF\u2E80-\u2EFF\u3000-\u303F\u3040-\u30FF\u3130-\u318F\u3400-\u4DBF\u4E00-\u9FFF\uA960-\uA97F\uAC00-\uD7AF\uF900-\uFAFF\uFE30-\uFE4F\uFF00-\uFFEF]/u;
/** 标点 / 符号（Unicode P* 与 S*）：**吸附到前一个单位**，不占自己的节拍。 */
const PUNCT_RE = /[\p{P}\p{S}]/u;
const SPACE_RE = /\s/u;

/** **文本 → 单位序列**（用户口径的「单位」定义）：
 *   · 全角字符：每个各算 1 个单位；
 *   · 连续的西文 / 数字串：算 1 个「词」单位（**按空白切出**，不是按字母）；
 *   · 空白：并进**前一个**单位（保留单空格外观）；
 *   · 标点：吸附到**前一个**单位（行首标点粘到下一个单位）—— **不占节拍**。
 *
 *  不变量：**对已归一的文本** `splitUnits(t).join("") === t`（拼接回原文，供逐字重渲染使用）。
 *  入参约定为 `normalizeReplyText` 的输出（此时无换行、无连续 / 首尾空白）——
 *  未归一时**行首空白会被丢弃**，故不保证 join 回原文。 */
export function splitUnits(text: string): string[] {
  const units: string[] = [];
  let buf = "";
  for (const ch of text) {
    if (SPACE_RE.test(ch)) {
      if (buf) {
        units.push(buf + ch); // 空格并进前一个单位
        buf = "";
      } else if (units.length) {
        units[units.length - 1] += ch;
      }
      continue;
    }
    if (PUNCT_RE.test(ch)) {
      if (buf) buf += ch; // 词内标点（如 "hello,"）随该词一并揭示
      else if (units.length) units[units.length - 1] += ch; // 吸附到前一个单位
      else buf += ch; // 行首标点：粘到下一个单位
      continue;
    }
    if (FULLWIDTH_RE.test(ch)) {
      if (buf) {
        units.push(buf);
        buf = "";
      }
      units.push(ch); // 全角字符各自成单位
      continue;
    }
    buf += ch; // 西文 / 数字等：攒成一个「词」单位
  }
  if (buf) units.push(buf);
  return units;
}

/** 是否**直接整体显示**（不做逐字动画）。判据：
 *  · 单位数为 0（空文本）；
 *  · 预估总时长 `unitCount / rate` 超过 `maxMs`（长文本上限保护）。
 *  速率非法时按默认速率估算。 */
export function shouldAnimateDirectly(unitCount: number, ratePerSec: number, maxMs: number): boolean {
  if (!(unitCount > 0)) return true;
  const r = clampRate(ratePerSec);
  return (unitCount * 1000) / r > maxMs;
}
