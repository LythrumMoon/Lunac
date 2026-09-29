// 翻译插件 —— 2026-09-29（基础插件，用户定的清单见 plugins/kinds.ts）
//
// ── 三层结构（与宿主 translate.rs 一一对应）──────────────────────
//   ① 词典底座（**不花钱**）：`translate_lookup` —— 先查本地译文缓存，再打免 key 的
//      MyMemory（主译文）与有道词典（音标 / 释义 / 例句）。两层都没有结果就返回
//      `source: "none"`，本面板据此显示「用 AI 翻译」。
//   ② 模型补漏（**花钱**）：`translate_ai` —— **只在用户点按钮时**发起，不由系统自作主张。
//   ③ 缓存：两层的结果都落 SQLite（`<exe 根>\ModuleData\translate\cache.db`），
//      同一句问第二次不再出门（宿主那边实测 6~7ms 返回）。
//
// ── 为什么联网不在这个文件里 ──────────────────────────────────
// 前端插件跑在 WebView 里，CSP 不含任何远端域，`fetch()` 会被直接拦掉 —— 所有联网动作
// 都在宿主（见 translate.rs 顶部与 ai-spec §11 规则 67 的第 ⑥ 条）。
//
// ── 两条界面纪律（code-rules 预检 #40 / §13.5）────────────────────
//   · 一个动作最多一条提示，「来自缓存」「实际语言」这类状态靠小字自明，不弹提示；
//   · 失败只说「哪一步没成 + 用户能做什么」（宿主已经把状态码 / 响应体换成这句话了）。

import type { Plugin } from "../registry";
import { invoke } from "@tauri-apps/api/core";
import { t } from "../../i18n.js";

interface TranslateExample {
  src: string;
  dst: string;
}

interface TranslateResult {
  text: string;
  from: string;
  to: string;
  /** `dict` = 词典底座；`ai` = 模型补漏；`none` = 两层都没有结果 */
  source: string;
  /** 结果直接来自本地缓存（这次**没碰网络**） */
  cached: boolean;
  translation: string;
  phonetic: string;
  explains: string[];
  examples: TranslateExample[];
}

/** 源语言下拉：前一项是「自动」 */
const FROM_LANGS = ["auto", "zh-CN", "en", "ja", "ko"] as const;
/** 目标语言下拉（没有「自动」—— 目标语言必须明确，猜目标等于没做事） */
const TO_LANGS = ["zh-CN", "en", "ja", "ko"] as const;
/** 目标语言默认值：中文。用户主要用法是「把看到的英文弄成中文」。 */
const DEFAULT_TO = "zh-CN";
/** 上次选的语言。**按面板实例重建不清空**：同一个插件来回开关时语言不该跳回去。 */
let lastFrom = "auto";
let lastTo = DEFAULT_TO;

/** 面板内的全部可变状态。由 `attachTranslateListeners` 建一份并闭包持有 ——
 *  面板被整体替换 / 关掉之后，旧的那一份连同它的回调一起变成垃圾，
 *  不会像模块级单例那样把上一次的结果漏到下一次打开里。 */
interface PanelState {
  busy: boolean;
  res: TranslateResult | null;
  err: string;
}

export const translatePlugin: Plugin = {
  id: "translate",
  name: "Translate",
  keywords: [
    "translate", "translation", "dict", "dictionary", "meaning", "word",
    "翻译", "词典", "字典", "查词", "释义", "译文", "中英",
  ],
  description: "Translate text and look words up — 翻译与查词",
  icon: "🔤",
  badge: "Dict",

  async execute(input: string) {
    // 搜索栏里的文字直接当待译内容预填（`翻译 hello` 这类前缀一并剥掉 —— 用户是
    // 在「调起插件」，不是要把「翻译」两个字也译出来）。不自动发起查询：
    // 联网动作要由用户看见并确认（首屏永远是空的输入框 + 上一次的语言选择）。
    const seed = stripInvocation(input || "");
    return { type: "html", content: panelHtml(seed) };
  },
};

/** 剥掉调用词（`翻译 hello` / `translate hello` → `hello`）。
 *
 *  两个形态都要管，否则会闹一个很低级的笑话：用户在搜索栏里敲「翻译」把它调起来
 *  （这是**最常见的入口**），待译内容就被预填成「翻译」两个字。
 *    · `翻译`（整个查询就是调用词）⇒ 预填空；
 *    · `翻译 hello` ⇒ 预填 `hello`。
 *  只在**开头**匹配、且后面必须跟空白或到头 —— 否则 `translator` 会被切成 `or`。 */
function stripInvocation(input: string): string {
  const s = input.trim();
  const m = /^(翻译|translate|词典|字典|查词|dict)(\s+|$)/i.exec(s);
  return m ? s.slice(m[0].length).trim() : s;
}

function panelHtml(seed: string): string {
  const langOptions = (list: readonly string[], cur: string) =>
    list
      .map((code) => {
        const label = t(`xl.lang.${code}`);
        const sel = code === cur ? " selected" : "";
        return `<option value="${code}"${sel}>${esc(label)}</option>`;
      })
      .join("");

  return `
    <div class="xl-root">
      <textarea id="xl-input" class="xl-input" rows="3" spellcheck="false"
                placeholder="${esc(t("xl.placeholder"))}">${esc(seed)}</textarea>
      <div class="xl-row">
        <select id="xl-from" class="xl-select">${langOptions(FROM_LANGS, lastFrom)}</select>
        <span class="xl-arrow">→</span>
        <select id="xl-to" class="xl-select">${langOptions(TO_LANGS, lastTo)}</select>
        <button type="button" id="xl-go" class="xl-go">${esc(t("xl.go"))}</button>
      </div>
      <div id="xl-result" class="xl-result"></div>
    </div>`;
}

// ── 挂载 ────────────────────────────────────────────────────────

export function attachTranslateListeners(root: HTMLElement): void {
  const input = root.querySelector("#xl-input") as HTMLTextAreaElement | null;
  const fromSel = root.querySelector("#xl-from") as HTMLSelectElement | null;
  const toSel = root.querySelector("#xl-to") as HTMLSelectElement | null;
  const goBtn = root.querySelector("#xl-go") as HTMLButtonElement | null;
  const box = root.querySelector("#xl-result") as HTMLElement | null;
  if (!input || !fromSel || !toSel || !goBtn || !box) return;

  const state: PanelState = { busy: false, res: null, err: "" };

  const paint = () => {
    box.innerHTML = resultHtml(state);
    // 「用 AI 翻译 / 重译」每次都要重绑：它随结果一起被整体替换
    const aiBtn = box.querySelector("#xl-ai") as HTMLButtonElement | null;
    aiBtn?.addEventListener("click", () => void run(true));
  };

  const run = async (useAi: boolean) => {
    const text = input.value.trim();
    if (!text || state.busy) return;
    lastFrom = fromSel.value;
    lastTo = toSel.value;
    state.busy = true;
    state.err = "";
    paint(); // 立刻进「翻译中」，别让按钮在慢请求里看着没反应
    try {
      state.res = await invoke<TranslateResult>(
        useAi ? "translate_ai" : "translate_lookup",
        { text, from: lastFrom, to: lastTo },
      );
    } catch (e) {
      state.err = String(e);
      state.res = null;
    } finally {
      state.busy = false;
      paint();
    }
  };

  goBtn.addEventListener("click", () => void run(false));
  // Enter 翻译、Shift+Enter 换行 —— 翻译框里换行是少数用法，默认给「翻译」更顺手
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void run(false);
    }
  });
}

// ── 结果渲染 ────────────────────────────────────────────────────

function resultHtml(state: PanelState): string {
  if (state.busy) {
    return `<div class="xl-hint">${esc(t("xl.busy"))}</div>`;
  }
  if (state.err) {
    return `<div class="xl-hint xl-err">${esc(state.err)}</div>`;
  }
  const r = state.res;
  if (!r) return "";

  // 两层都没结果：**这是唯一的引导位** —— 词典没有就去问模型（花钱的动作只在这里出现）
  if (r.source === "none") {
    return `<div class="xl-hint">${esc(t("xl.not_found"))}</div>
      <button type="button" id="xl-ai" class="xl-ai">${esc(t("xl.ai_go"))}</button>`;
  }

  const parts: string[] = [];
  if (r.translation) parts.push(`<div class="xl-trans">${esc(r.translation)}</div>`);
  if (r.phonetic) parts.push(`<div class="xl-phonetic">${esc(r.phonetic)}</div>`);
  if (r.explains.length > 0) {
    parts.push(
      `<ul class="xl-explains">${r.explains.map((x) => `<li>${esc(x)}</li>`).join("")}</ul>`,
    );
  }
  if (r.examples.length > 0) {
    parts.push(
      `<div class="xl-examples">${r.examples
        .map((e) => `<div class="xl-ex"><div class="xl-ex-src">${esc(e.src)}</div><div class="xl-ex-dst">${esc(e.dst)}</div></div>`)
        .join("")}</div>`,
    );
  }
  // 状态小字：来源 + 是否来自缓存（**不弹提示**，就地一行）
  const meta = r.cached ? t("xl.meta_cached") : r.source === "ai" ? t("xl.meta_ai") : t("xl.meta_dict");
  parts.push(`<div class="xl-meta">${esc(meta)}</div>`);
  // 词典结果旁给「用 AI 重译」：词典给的是词条释义，整句质量不如模型
  parts.push(`<button type="button" id="xl-ai" class="xl-ai">${esc(t("xl.ai_retry"))}</button>`);
  return parts.join("");
}

// ── Helpers ─────────────────────────────────────────────────────

function esc(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}
