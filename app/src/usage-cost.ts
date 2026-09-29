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
  input: number;
  output: number;
  cacheRead: number;
  cacheCreate: number;
}
export interface UsageModelTotals {
  model: string;
  turns: number;
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
  source_url?: string;
  updated_at?: string;
}
export interface PricingFile {
  updated_at?: string;
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

/** 该模型在「**本地**星期几 + 本地小时」这一格实际生效的价（分时价，2026-09-29）。
 *
 *  规则：基础四类价是**缺省价**；`time_windows` 里**第一条命中**的条目覆盖它（顺序即优先级）。
 *  未定价的模型返回 `null`（调用方据此标「未定价」，**不许当 0**，见规则 63）。
 *
 *  **粒度是整点**：桶只带 `hour`，所以这里用「该小时的起点」去比时段。时段边界都在整点时
 *  完全无损；若写成半点（如 `09:30-12:00`），落在 `09:00-10:00` 那一桶的量会按**基础价**算
 *  —— 误差 ≤ 1 小时，且方向是保守（宁少算不高算）。 */
export function priceAt(
  pricing: PricingFile | null,
  model: string,
  weekday: number,
  hour: number,
): PricingEntry | null {
  const base = priceOf(pricing, model);
  if (!base) return null;
  const windows = base.time_windows;
  if (!Array.isArray(windows)) return base;
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
  const weekday = weekdayOf(day.date);
  for (const m of day.models) {
    if (!priceOf(pricing, m.model)) {
      exact = false;
      continue;
    }
    if (m.hours && m.hours.length > 0) {
      for (const h of m.hours) {
        const p = priceAt(pricing, m.model, weekday, h.hour);
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
    turns: 0, input: 0, hit: 0, write: 0, output: 0,
    amount: 0, exact: true, unpriced: new Set<string>(),
  };
  for (const day of d.days) {
    tot.turns += day.turns;
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

const COL_KEYS = [
  "settings.cost_col_date",
  "settings.cost_col_turns",
  "settings.cost_col_input",
  "settings.cost_col_hit",
  "settings.cost_col_write",
  "settings.cost_col_output",
  "settings.cost_col_amount",
];

function amountText(hasPrices: boolean, amount: number, exact: boolean): string {
  if (!hasPrices) return "—";
  return `${exact ? "" : "≥ "}¥${fmtMoney(amount)}`;
}

/** 逐日用量表格（**含总计行**）。新的在上 —— 看用量几乎总是先看最近几天。 */
export function renderUsageCostTable(d: UsageCostData): string {
  if (d.days.length === 0) {
    return `<div class="settings-hint">${t("settings.cost_no_usage", { days: String(COST_RANGE_DAYS) })}</div>`;
  }
  const hasPrices = Object.keys(d.pricing?.models || {}).length > 0;
  const tot = sumUsageCost(d);
  const head = `<tr>${COL_KEYS.map((k) => `<th>${t(k)}</th>`).join("")}</tr>`;
  const rows = d.days.slice().reverse().map((day) => {
    const c = dayCost(day, d.pricing);
    return `<tr><td>${day.date}</td><td>${day.turns}</td><td>${fmtTokenCount(day.input)}</td>` +
      `<td>${fmtTokenCount(day.cacheRead)}</td><td>${fmtTokenCount(day.cacheCreate)}</td>` +
      `<td>${fmtTokenCount(day.output)}</td><td>${amountText(hasPrices, c.amount, c.exact)}</td></tr>`;
  }).join("");
  const totalRow = `<tr class="cost-total-row"><td>${t("settings.cost_total")}</td><td>${tot.turns}</td>` +
    `<td>${fmtTokenCount(tot.input)}</td><td>${fmtTokenCount(tot.hit)}</td>` +
    `<td>${fmtTokenCount(tot.write)}</td><td>${fmtTokenCount(tot.output)}</td>` +
    `<td>${amountText(hasPrices, tot.amount, tot.exact)}</td></tr>`;
  return `<table class="cost-table"><thead>${head}</thead><tbody>${rows}</tbody><tfoot>${totalRow}</tfoot></table>`;
}

/** 表盘展开面板的正文：**只有数据，没有说明文字**（用户 2026-09-29 定：
 *  「只是移入数据，并不是移入说明」）—— 价格表时间 / 未定价模型 / 候选文件路径三样数据，
 *  加上逐日表格（含总计行）。 */
export function renderUsageCostPanel(d: UsageCostData): string {
  const hasPrices = Object.keys(d.pricing?.models || {}).length > 0;
  const tot = sumUsageCost(d);
  const cells: string[] = [];
  cells.push(
    `<span class="cost-kv"><span class="cost-k">${t("settings.cost_k_price")}</span>` +
    `<span class="cost-v">${hasPrices ? esc(d.pricing?.updated_at || "—") : "—"}</span></span>`,
  );
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
  return `<div class="cost-data">${cells.join("")}</div>${renderUsageCostTable(d)}`;
}
