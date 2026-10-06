// 用量与成本（A12）：**数据层 + 渲染层**，供两个宿主共用 —— 设置面板的「用量与成本」
// 与主界面表盘（`#token-dashboard`）的展开面板。
//
// 为什么要把它们抽出来（2026-09-29 用户定）：**逐日表格 + 价格表数据整体搬进
// 「点表盘展开」的面板**（对话进行中也能实时刷新），设置里只留「打开价格表 / 更新价格」
// 按钮与候选价格确认。两处都要算金额 ⇒ 算法只允许有一份，免得两边各算一遍、慢慢漂移
// （同预检 #39 ⑩「同一个值只写一处」）。
//
// 三条既有纪律不变：
//  ① 价格不写进代码（各家单价差十倍、官方还会调价）—— 价格表是用户可编辑的文件，这里只读它；
//  ② 「更新价格」不由前端自己抓（它没有网络也没有模型），任务交给 agent，agent 只能写候选文件；
//  ③ 金额一律在前端算：价格表随时会改，改完即时重算，不必再跑一趟 IPC。

import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n.js";

/** 汇总区间（天）。日志按天分片，这里是 30 个文件、一次 IPC 读完（`read_usage_range`）。 */
export const COST_RANGE_DAYS = 30;

export interface UsageHourTotals {
  /** 本地小时 0–23 */
  hour: number;
  turns: number;
  /** 真实 API 请求数（2026-10-06；旧宿主不返回该字段 → 0） */
  requests?: number;
  input: number;
  output: number;
  cacheRead: number;
  cacheCreate: number;
}
export interface UsageModelTotals {
  model: string;
  turns: number;
  /** 真实 API 请求数（见 `UsageHourTotals.requests`） */
  requests?: number;
  input: number;
  output: number;
  cacheRead: number;
  cacheCreate: number;
  /** **按本地小时**拆开的同一批量（与上面四个字段是同一批 token，只是再切一刀）。
   *  价格表里该模型有 `time_windows` 时**必须**用它逐桶计价；旧宿主不返回 → 空数组。 */
  hours?: UsageHourTotals[];
}
export interface UsageDay {
  date: string;
  turns: number;
  /** 真实 API 请求数（见 `UsageHourTotals.requests`） */
  requests?: number;
  input: number;
  output: number;
  cacheRead: number;
  cacheCreate: number;
  models: UsageModelTotals[];
}
/** 一条「时段价」：本地时间 `[from, to)` 内用这一档价。`days` 省略 = 每天，
 *  给的话是 ISO 周几（1=周一 … 7=周日）。**列表里第一条命中的生效**（顺序即优先级）。 */
export interface PricingWindow {
  days?: number[];
  from?: string;
  to?: string;
  input?: number;
  cache_read?: number;
  cache_write?: number;
  output?: number;
}
export interface PricingEntry {
  input?: number;
  cache_read?: number;
  cache_write?: number;
  output?: number;
  /** 时段价（可选）：命中的那一条**覆盖**上面四类价（2026-09-29）。 */
  time_windows?: PricingWindow[];
  /** 该模型的**法定节假日**（`YYYY-MM-DD`，本地口径）—— 与顶层同名时以本字段为准。
   *  见 `PricingFile.holidays`。 */
  holidays?: string[];
  source_url?: string;
  updated_at?: string;
}
export interface PricingFile {
  updated_at?: string;
  /** **法定节假日**（`YYYY-MM-DD`，本地口径）：这些天**全天按基础价（谷价）算**，
   *  日/时段的 `time_windows`（高峰价）一律不适用。
   *
   *  出处：DeepSeek 官方价目表脚注 —— 「北京时间周一至周五（**不含中国法定节假日**）
   *  9:00-12:00、14:00-18:00 为高峰时段；其余时段，包括周末及中国法定节假日全天均为空闲
   *  时段」。缺这条就会在节假日把全天按峰价高估（2026-10-02 国庆实测：多算 0.979 元）。
   *
   *  放在顶层是「日历属性」；换供应商要各用各的日历时，把 `holidays` 写进对应的
   *  `models.<名>` 即可覆盖（单价本来就是**按模型分桶**的）。 */
  holidays?: string[];
  models?: Record<string, PricingEntry>;
}
export interface PricingState {
  path: string;
  text: string;
  pendingPath: string;
  pendingText: string;
}

/** 本地日期 `YYYY-MM-DD`（与 main.ts 的用量日志分片键同一套口径） */
export function costDateKey(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** 近 n 天的本地日期，升序（宿主按同样的顺序读回来） */
export function lastNDates(n: number): string[] {
  const today = new Date();
  const out: string[] = [];
  for (let i = n - 1; i >= 0; i--) {
    out.push(costDateKey(new Date(today.getFullYear(), today.getMonth(), today.getDate() - i)));
  }
  return out;
}

/** 面板的时间范围（2026-10-06 用户定）。
 *
 *  **单日 ⇒ 表格按小时出；区间 ⇒ 按天出**。为什么必须这么分：近 30 天逐小时就是 720 行，
 *  没人看得下去；而只看一天的量时，按小时才看得出「哪个时段在烧钱」。 */
export type UsageRange =
  | { kind: "today" }
  | { kind: "days"; days: number }
  | { kind: "date"; date: string };

export const DEFAULT_USAGE_RANGE: UsageRange = { kind: "today" };

/** 范围 ↔ `<select>` 的 option value（一进一出成对写在一处，别两头各拼一套）。 */
export function usageRangeValue(r: UsageRange): string {
  if (r.kind === "today") return "today";
  if (r.kind === "days") return `days:${r.days}`;
  return `date:${r.date}`;
}

/** 认不出来就退回默认范围（不抛错：这个值来自 DOM，坏掉的面板比坏掉的数据更糟）。 */
export function parseUsageRangeValue(v: string): UsageRange {
  if (v === "today") return { kind: "today" };
  const days = /^days:(\d+)$/.exec(v);
  if (days) return { kind: "days", days: Number(days[1]) };
  const date = /^date:(\d{4}-\d{2}-\d{2})$/.exec(v);
  if (date) return { kind: "date", date: date[1] };
  return DEFAULT_USAGE_RANGE;
}

/** 面板里「当前统计的模型」（2026-10-06 用户定）：`null` = **总计**（全部模型加在一起）。
 *  多模型混用（换过模型 / 用过子代理）时它才是一个可选项，单一模型时只把名字显示出来。 */
export type UsageModelFilter = string | null;

/** 这批数据里出现过的模型名（去重、升序）。**取自未裁剪的那份** ——
 *  否则「选了某一天 → 别的模型从选项里消失 → 切不回去」。 */
export function usageModels(d: UsageCostData): string[] {
  const set = new Set<string>();
  for (const day of d.days) for (const m of day.models) set.add(m.model);
  return [...set].sort((a, b) => a.localeCompare(b));
}

/** 把一天裁到只留某个模型。**日级那几个合计必须按留下的模型重算** ——
 *  它们原本是跨模型累加的，不重算就会出现「表里只有 A 的行，合计却是 A+B」。
 *  这天没有该模型的量 ⇒ 返回 `null`（整行不显示，别留一个全零行）。 */
function keepModel(day: UsageDay, model: string): UsageDay | null {
  const models = day.models.filter((m) => m.model === model);
  if (models.length === 0) return null;
  const sum = (pick: (m: UsageModelTotals) => number) => models.reduce((a, m) => a + pick(m), 0);
  return {
    ...day,
    models,
    turns: sum((m) => m.turns),
    requests: sum((m) => m.requests ?? 0),
    input: sum((m) => m.input),
    output: sum((m) => m.output),
    cacheRead: sum((m) => m.cacheRead),
    cacheCreate: sum((m) => m.cacheCreate),
  };
}

/** 把读回来的那份（近 `COST_RANGE_DAYS` 天）裁到所选范围 + 所选模型 —— 面板的头
 *  （未定价）与表格必须看同一段，否则会出现「表格里没有的量却把模型列进未定价」。 */
export function scopedUsageData(
  d: UsageCostData,
  r: UsageRange,
  model: UsageModelFilter = null,
): UsageCostData {
  const keep =
    r.kind === "today" ? new Set([costDateKey(new Date())])
    : r.kind === "date" ? new Set([r.date])
    : new Set(lastNDates(r.days));
  const days = d.days
    .filter((x) => keep.has(x.date))
    .map((x) => (model === null ? x : keepModel(x, model)))
    .filter((x): x is UsageDay => x !== null);
  return { ...d, days };
}

/** 本地时区相对 UTC 的偏移（分钟，东八区 = 480）。宿主用它把 `ts` 换算成本地小时 ——
 *  Rust 侧没有时区数据库，分时价需要知道「本地几点」，这个数只能由这里给。 */
export function localUtcOffsetMinutes(): number {
  return -new Date().getTimezoneOffset();
}

/** `YYYY-MM-DD` 的 ISO 周几（1=周一 … 7=周日）。用**本地日历**构造，不受时区影响；
 *  解析不出来时返回 0（调用方据此不命中任何带 `days` 的时段 → 退回基础价）。 */
export function weekdayOf(date: string): number {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  if (!m) return 0;
  const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  if (Number.isNaN(d.getTime())) return 0;
  return ((d.getDay() + 6) % 7) + 1;
}

/** `HH:MM` → 「零点起的分钟数」；写法不合法返回 null。**与宿主侧 `parse_hhmm` 同一套严格口径**：
 *  只认 `HH:MM`，不接受 `9:00` —— 宽松解析会把用户的意思读成另一个时刻。 */
function hhmmToMinutes(s: unknown): number | null {
  if (typeof s !== "string") return null;
  const m = /^(\d{2}):(\d{2})$/.exec(s);
  if (!m) return null;
  const h = Number(m[1]);
  const mi = Number(m[2]);
  if (h > 23 || mi > 59) return null;
  return h * 60 + mi;
}

export function parsePricing(text: string): PricingFile | null {
  const trimmed = (text || "").trim();
  if (!trimmed) return null;
  try {
    const v = JSON.parse(trimmed) as PricingFile;
    return v && typeof v === "object" ? v : null;
  } catch {
    return null; // 语法坏 = 没有价格（面板按「未定价」处理，不去猜）
  }
}

export function priceOf(pricing: PricingFile | null, model: string): PricingEntry | null {
  const e = pricing?.models?.[model];
  return e && typeof e === "object" ? e : null;
}

/** 这一天是不是价格表认定的**法定节假日**。
 *
 *  判据：模型级 `holidays` 优先，缺省回落到顶层 `holidays`。命中 ⇒ 当天**全天按基础价
 *  （谷价）**，`time_windows` 一律不适用（官方脚注：周一至周五**不含法定节假日**才算高峰）。
 *  日期是 `YYYY-MM-DD` 的**纯字符串比较**（不含日期逻辑，跨年也不会误判）。 */
export function isHoliday(pricing: PricingFile | null, model: string, date: string): boolean {
  const list = priceOf(pricing, model)?.holidays ?? pricing?.holidays;
  return Array.isArray(list) && list.includes(date);
}

/** 该模型在「**本地日期 + 本地小时**」这一格实际生效的价（分时价，2026-09-29）。
 *
 *  规则：基础四类价是**缺省价**；`time_windows` 里**第一条命中**的条目覆盖它（顺序即优先级）。
 *  **法定节假日全天不适用任何时段窗**（直接回基础价，见 `isHoliday`）。
 *  未定价的模型返回 `null`（调用方据此标「未定价」，**不许当 0**，见规则 63）。
 *
 *  **为什么收 `date` 而不是 `weekday`**：节假日判据要的是**具体哪一天**，光有周几会把
 *  「国庆的周五」与「平常的周五」算成同一档。周几在函数内部由 `weekdayOf(date)` 现推。
 *
 *  **粒度是整点**：桶只带 `hour`，所以这里用「该小时的起点」去比时段。时段边界都在整点时
 *  完全无损；若写成半点（如 `09:30-12:00`），落在 `09:00-10:00` 那一桶的量会按**基础价**算
 *  —— 误差 ≤ 1 小时，且方向是保守（宁少算不高算）。 */
export function priceAt(
  pricing: PricingFile | null,
  model: string,
  date: string,
  hour: number,
): PricingEntry | null {
  const base = priceOf(pricing, model);
  if (!base) return null;
  const windows = base.time_windows;
  if (!Array.isArray(windows) || isHoliday(pricing, model, date)) return base;
  const weekday = weekdayOf(date);
  const mins = hour * 60;
  for (const w of windows) {
    if (!w || typeof w !== "object") continue;
    const from = hhmmToMinutes(w.from);
    const to = hhmmToMinutes(w.to);
    if (from === null || to === null || from >= to) continue;
    if (mins < from || mins >= to) continue;
    if (Array.isArray(w.days) && !w.days.includes(weekday)) continue;
    return w;
  }
  return base;
}

/** 一天的金额。**必须逐模型算**：一天里换过模型的话，只按天合计就把两个模型的量
 *  混在一起了（单价差十倍）。`exact=false` 表示这天有模型没价格，金额只是已知部分 ——
 *  面板要如实标出来，不要假装它是个准确值。
 *
 *  **有 `hours` 桶就逐桶计价**（分时价要求「这一批 token 发生在几点」）：桶的粒度见
 *  `priceAt`。模型没有时段价时逐桶结果与「总量 × 基础价」逐分不差（同一单价），
 *  所以逐桶是安全的默认路径；`hours` 缺失（旧宿主）才退回总量法。 */
export function dayCost(day: UsageDay, pricing: PricingFile | null): { amount: number; exact: boolean } {
  let amount = 0;
  let exact = true;
  for (const m of day.models) {
    if (!priceOf(pricing, m.model)) {
      exact = false;
      continue;
    }
    if (m.hours && m.hours.length > 0) {
      for (const h of m.hours) {
        const p = priceAt(pricing, m.model, day.date, h.hour);
        if (!p) {
          exact = false;
          continue;
        }
        amount += (
          (h.input || 0) * (p.input || 0)
          + (h.cacheRead || 0) * (p.cache_read || 0)
          + (h.cacheCreate || 0) * (p.cache_write || 0)
          + (h.output || 0) * (p.output || 0)
        ) / 1e6;
      }
      continue;
    }
    const p = priceOf(pricing, m.model)!;
    amount += (
      (m.input || 0) * (p.input || 0)
      + (m.cacheRead || 0) * (p.cache_read || 0)
      + (m.cacheCreate || 0) * (p.cache_write || 0)
      + (m.output || 0) * (p.output || 0)
    ) / 1e6;
  }
  return { amount, exact };
}

/** 金额显示：单价是「元 / 百万 token」，单次提问常常只有几厘 —— 太小就多给两位小数，
 *  否则一律显示 0.00，等于把「花了很少」和「没花钱」显示成同一个样子。 */
export function fmtMoney(v: number): string {
  if (!(v > 0)) return "0.00";
  return v < 0.01 ? v.toFixed(4) : v.toFixed(2);
}

/** token 数显示（表格里用 k / M 缩写，与主界面表盘同风格） */
export function fmtTokenCount(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(2) + "M";
  if (n >= 1_000) return (n / 1_000).toFixed(1) + "k";
  return String(n);
}

/** 模型名显示：早期用量日志的模型名是空的（前端当时没记），显示成 `—` 而不是空单元格
 *  —— 空名字看起来像渲染坏了，而它确实会被算进「未定价」。 */
export function modelLabel(model: string): string {
  return model ? model : "—";
}

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    c === "&" ? "&amp;" : c === "<" ? "&lt;" : c === ">" ? "&gt;" : c === '"' ? "&quot;" : "&#39;");
}

export interface UsageCostData {
  days: UsageDay[];
  pricing: PricingFile | null;
  pending: PricingFile | null;
  state: PricingState | null;
}

/** 取面板要用的一份数据（价格表状态 + 近 N 天用量）。任何一步失败都退回「没有它」，
 *  绝不让金额面板整个渲染不出来。 */
export async function loadUsageCost(): Promise<UsageCostData> {
  let state: PricingState | null = null;
  try {
    state = await invoke<PricingState>("get_pricing_state");
  } catch {}
  let days: UsageDay[] = [];
  try {
    // 必须带上时区偏移：宿主没有时区库，分时价要靠它把 `ts` 换算成本地小时
    days = await invoke<UsageDay[]>("read_usage_range", {
      dates: lastNDates(COST_RANGE_DAYS),
      utcOffsetMinutes: localUtcOffsetMinutes(),
    });
  } catch {}
  return {
    days,
    pricing: state ? parsePricing(state.text) : null,
    pending: state ? parsePricing(state.pendingText) : null,
    state,
  };
}

export interface UsageTotals {
  turns: number;
  /** 真实 API 请求数（2026-10-06） */
  requests: number;
  input: number;
  hit: number;
  write: number;
  output: number;
  amount: number;
  exact: boolean;
  unpriced: Set<string>;
}

/** 合计（逐日累加 + 逐模型查价）。 */
export function sumUsageCost(d: UsageCostData): UsageTotals {
  const tot: UsageTotals = {
    turns: 0, requests: 0, input: 0, hit: 0, write: 0, output: 0,
    amount: 0, exact: true, unpriced: new Set<string>(),
  };
  for (const day of d.days) {
    tot.turns += day.turns;
    tot.requests += day.requests ?? 0;
    tot.input += day.input;
    tot.hit += day.cacheRead;
    tot.write += day.cacheCreate;
    tot.output += day.output;
    const c = dayCost(day, d.pricing);
    tot.amount += c.amount;
    if (!c.exact) tot.exact = false;
    for (const m of day.models) if (!priceOf(d.pricing, m.model)) tot.unpriced.add(m.model);
  }
  return tot;
}

/** 表格的列（顺序即面板上从左到右的顺序，2026-10-06 用户定）。
 *  「输入（命中缓存）/ 输入（未命中缓存）」= `cacheRead` / `input` 两个口径，
 *  再配上单独的「缓存写入」列 —— 三列各自乘自己的单价，加起来正是金额列。 */
const COL_KEYS = [
  "settings.cost_col_time",
  "settings.cost_col_requests",
  "settings.cost_col_hit_input",
  "settings.cost_col_miss_input",
  "settings.cost_col_write",
  "settings.cost_col_output",
  "settings.cost_col_amount",
];

/** 表格的一行（按天或按小时，形状一样）。 */
interface CostRow {
  label: string;
  requests: number;
  /** 输入（命中缓存） */
  hit: number;
  /** 输入（未命中缓存） */
  miss: number;
  /** 缓存写入 */
  write: number;
  output: number;
  amount: number;
  exact: boolean;
}

function amountText(hasPrices: boolean, amount: number, exact: boolean): string {
  if (!hasPrices) return "—";
  return `${exact ? "" : "≥ "}¥${fmtMoney(amount)}`;
}

/** 按天：一行一天。**新的在上**（看用量几乎总是先看最近几天）。 */
function dayRows(days: UsageDay[], pricing: PricingFile | null): CostRow[] {
  return days
    .map((day) => {
      const c = dayCost(day, pricing);
      return {
        label: day.date,
        requests: day.requests ?? 0,
        hit: day.cacheRead,
        miss: day.input,
        write: day.cacheCreate,
        output: day.output,
        amount: c.amount,
        exact: c.exact,
      };
    })
    .reverse();
}

/** 按小时：一行一个小时（只列有量的那几个小时）。
 *
 *  **金额必须逐模型逐桶算** —— 同一小时里不同模型单价不同（甚至时段价不同），
 *  把一天的金额按比例摊到各小时上是错的。所以这里是「模型 × 小时」的汇总，
 *  与 `dayCost` 走的路径同源（都过 `priceAt`）。 */
function hourRows(day: UsageDay, pricing: PricingFile | null): CostRow[] {
  const byHour = new Map<number, CostRow>();
  for (const m of day.models) {
    for (const h of m.hours ?? []) {
      const row = byHour.get(h.hour) ?? {
        label: `${String(h.hour).padStart(2, "0")}:00`,
        requests: 0, hit: 0, miss: 0, write: 0, output: 0, amount: 0, exact: true,
      };
      row.requests += h.requests ?? 0;
      row.hit += h.cacheRead;
      row.miss += h.input;
      row.write += h.cacheCreate;
      row.output += h.output;
      const p = priceAt(pricing, m.model, day.date, h.hour);
      if (!p) {
        row.exact = false;
      } else {
        row.amount += (
          (h.input || 0) * (p.input || 0)
          + (h.cacheRead || 0) * (p.cache_read || 0)
          + (h.cacheCreate || 0) * (p.cache_write || 0)
          + (h.output || 0) * (p.output || 0)
        ) / 1e6;
      }
      byHour.set(h.hour, row);
    }
  }
  // 一天之内按时间**正序**读最自然（不像按天表格那样新的在上）
  return [...byHour.values()].sort((a, b) => a.label.localeCompare(b.label));
}

function rowsHtml(rows: CostRow[], hasPrices: boolean): string {
  return rows.map((r) =>
    `<tr><td>${esc(r.label)}</td><td>${r.requests}</td>` +
    `<td>${fmtTokenCount(r.hit)}</td><td>${fmtTokenCount(r.miss)}</td>` +
    `<td>${fmtTokenCount(r.write)}</td><td>${fmtTokenCount(r.output)}</td>` +
    `<td>${amountText(hasPrices, r.amount, r.exact)}</td></tr>`
  ).join("");
}

/** 用量表格（**含合计行**）。`range` 决定粒度：单日按小时、区间按天（见 `UsageRange`）。 */
export function renderUsageCostTable(d: UsageCostData, range: UsageRange): string {
  const hasPrices = Object.keys(d.pricing?.models || {}).length > 0;
  const tot = sumUsageCost(d);
  let rows: CostRow[];
  if (range.kind === "days") {
    rows = dayRows(d.days, d.pricing);
  } else {
    const date = range.kind === "today" ? costDateKey(new Date()) : range.date;
    const day = d.days.find((x) => x.date === date);
    rows = day ? hourRows(day, d.pricing) : [];
  }
  if (rows.length === 0) {
    return `<div class="settings-hint">${t("settings.cost_range_empty")}</div>`;
  }
  // 「缓存写入」这列在 DeepSeek 下**恒为 0**（它走自动前缀缓存，端点不返回
  // `cache_creation_input_tokens`）。数字本身是真的（没有数据），但看的人会以为
  // 「从没写过缓存」——所以只在这一列表头挂一句 title 解释（2026-10-02）。
  // **刻意不做成面板正文里的说明**：用户 2026-09-29 定过这个面板「只有数据，没有说明文字」。
  // 静态文案、不含引号，故直接插进属性即可（不需要 esc）。
  const head = `<tr>${COL_KEYS.map((k) =>
    k === "settings.cost_col_write"
      ? `<th title="${t("settings.cost_col_write_hint")}">${t(k)}</th>`
      : `<th>${t(k)}</th>`
  ).join("")}</tr>`;
  const totalRow = `<tr class="cost-total-row"><td>${t("settings.cost_total")}</td><td>${tot.requests}</td>` +
    `<td>${fmtTokenCount(tot.hit)}</td><td>${fmtTokenCount(tot.input)}</td>` +
    `<td>${fmtTokenCount(tot.write)}</td><td>${fmtTokenCount(tot.output)}</td>` +
    `<td>${amountText(hasPrices, tot.amount, tot.exact)}</td></tr>`;
  return `<table class="cost-table"><thead>${head}</thead><tbody>${rowsHtml(rows, hasPrices)}</tbody><tfoot>${totalRow}</tfoot></table>`;
}

/** 右上角的时间范围下拉框：今天 / 近 7 天 / 近 30 天 ＋「具体日期」一组（只列**有量**的那几天）。 */
function rangeSelectHtml(d: UsageCostData, range: UsageRange): string {
  const cur = usageRangeValue(range);
  const opt = (v: string, label: string) =>
    `<option value="${esc(v)}"${v === cur ? " selected" : ""}>${esc(label)}</option>`;
  const parts = [
    opt("today", t("settings.cost_range_today")),
    opt("days:7", t("settings.cost_range_7d")),
    opt("days:30", t("settings.cost_range_30d")),
  ];
  // 日期取**未裁剪**的那份（`d.days`），否则选了某一天之后就再也切不回别的日期
  const dates = d.days.map((x) => x.date).slice().reverse();
  if (dates.length > 0) {
    parts.push(`<optgroup label="${esc(t("settings.cost_range_dates"))}">`);
    for (const dt of dates) parts.push(opt(`date:${dt}`, dt));
    parts.push("</optgroup>");
  }
  return `<select class="cost-range-select" id="cost-range-select" aria-label="${esc(t("settings.cost_col_time"))}">${parts.join("")}</select>`;
}

/** 左上角「当前统计的模型」那一格（2026-10-06 用户定）：
 *  没有模型 → `—`；只有一个 → 直接显示名字（不必给一个只有一项的下拉框）；
 *  多个（混用过模型 / 子代理）→ 下拉框，第一项是「总计」。 */
function modelChipHtml(d: UsageCostData, model: UsageModelFilter): string {
  const models = usageModels(d);
  const k = `<span class="cost-k">${t("settings.cost_k_model")}</span>`;
  if (models.length === 0) {
    return `<span class="cost-kv">${k}<span class="cost-v">—</span></span>`;
  }
  if (models.length === 1) {
    return `<span class="cost-kv">${k}<span class="cost-v">${esc(modelLabel(models[0]))}</span></span>`;
  }
  const opt = (v: string, label: string, sel: boolean) =>
    `<option value="${esc(v)}"${sel ? " selected" : ""}>${esc(label)}</option>`;
  const options = [
    opt("", t("settings.cost_model_all"), model === null),
    ...models.map((m) => opt(m, modelLabel(m), m === model)),
  ];
  return `<span class="cost-kv">${k}` +
    `<select class="cost-range-select" id="cost-model-select" aria-label="${esc(t("settings.cost_k_model"))}">${options.join("")}</select></span>`;
}

/** 表盘展开面板的正文：**只有数据，没有说明文字**（用户 2026-09-29 定：
 *  「只是移入数据，并不是移入说明」）—— 左上角数据（最后更新价格日期 / 当前统计的模型 /
 *  未定价 / 候选文件路径）、右上角时间范围下拉框，中间是逐日或逐小时的表格（含合计行）。 */
export function renderUsageCostPanel(
  d: UsageCostData,
  range: UsageRange,
  model: UsageModelFilter = null,
): string {
  const scoped = scopedUsageData(d, range, model);
  const hasPrices = Object.keys(d.pricing?.models || {}).length > 0;
  const tot = sumUsageCost(scoped);
  const cells: string[] = [];
  cells.push(
    `<span class="cost-kv"><span class="cost-k">${t("settings.cost_k_price")}</span>` +
    `<span class="cost-v">${hasPrices ? esc(d.pricing?.updated_at || "—") : "—"}</span></span>`,
  );
  cells.push(modelChipHtml(d, model));
  if (tot.unpriced.size > 0) {
    // 空模型名（早期日志没记模型名）**不能走 modelLabel**：它渲染成 `—`，
    // 与「值缺失」的占位符同形 —— 这一格就变成「未定价：—」，读起来正好相反。
    const names = [...tot.unpriced].map((m) =>
      m ? modelLabel(m) : t("settings.cost_model_unknown"));
    cells.push(
      `<span class="cost-kv"><span class="cost-k">${t("settings.cost_k_unpriced")}</span>` +
      `<span class="cost-v">${esc(names.join(", "))}</span></span>`,
    );
  }
  if (d.pending) {
    cells.push(
      `<span class="cost-kv"><span class="cost-k">${t("settings.cost_k_pending")}</span>` +
      `<span class="cost-v" title="${esc(d.state?.pendingPath || "")}">${esc(d.state?.pendingPath || "—")}</span></span>`,
    );
  }
  return `<div class="cost-head"><div class="cost-data">${cells.join("")}</div>` +
    `<div class="cost-range">${rangeSelectHtml(d, range)}</div></div>` +
    renderUsageCostTable(scoped, range);
}
