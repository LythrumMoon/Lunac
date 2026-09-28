// ── Plugin registry ──────────────────────────────────────────────
// Luna plugin system: each plugin registers keywords and execution handler.
// When user types in the search bar, plugins matching the query are shown.
// If no plugin matches, the query is sent to the AI Agent (fallback).
//
// Search supports:
// 1. Exact / prefix / contains match on English text
// 2. Subsequence fuzzy match (fzf-style)
// 3. Pinyin match for Chinese keywords (full pinyin + first letters)
//    e.g. "jisuan" / "js" → matches "计算" (jì suàn)

import { pinyin } from "pinyin-pro";

export interface Plugin {
  id: string;
  name: string;
  keywords: string[];        // search keywords for matching
  description: string;
  icon: string;              // emoji icon
  badge?: string;            // short badge label
  execute: (input: string) => Promise<PluginResult>;
  /** 挂载钩子（可选，2026-09-28）。
   *
   *  **内置插件不走这里**：它们由 `attach.ts` 一张硬编码表按 id 分派（那份表要 import 各家
   *  模块、要传各自的参数，写在插件对象上会让每个内置插件都得自己实现一遍）。
   *  磁盘插件（第三方 / 用户自建）在入口模块里**具名导出 `attach(root)`** 即可被宿主调起来 ——
   *  这是「插件自己管自己的定时器与监听」的唯一约定，见 `Modules\插件开发规范.md`。 */
  attach?: (root: HTMLElement) => void | Promise<void>;
  /** 面板关闭时的收尾（可选）。与 `attach` 成对；不导出也没关系 —— 插件自己的
   *  `isConnected` 自停是兜底（与 main.ts 的 closePluginView 同一条纪律）。 */
  detach?: () => void;
  /** 插件声明的宿主能力（2026-09-28），如 `layout.takeover`。磁盘插件从清单带过来；
   *  内置插件在对象上自己写。宿主桥只放行声明过的能力，界面也如实列出（**是告知不是沙箱**）。 */
  permissions?: string[];
  /** Auto-generated pinyin tokens for Chinese keywords. Populated by register(). */
  _pinyinTokens?: string[];
}

export interface PluginResult {
  type: 'text' | 'html';
  content: string;
}

// ── Fuzzy matching ────────────────────────────────────────────────

/** Simple fuzzy match: all query chars appear in target in order (fzf-style). */
export function fuzzyScore(query: string, target: string): number {
  const q = query.toLowerCase();
  const t = target.toLowerCase();
  let qi = 0;
  let consecutive = 0;
  let prevIdx = -2;
  let score = 0;

  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] === q[qi]) {
      qi++;
      if (ti === prevIdx + 1) {
        consecutive++;
        score += consecutive * 3 + 2;
      } else {
        consecutive = 0;
        score += 2;
        // Word boundary bonus (after space, dot, dash, or uppercase start)
        if (ti === 0 || [' ', '.', '-', '_'].includes(t[ti - 1])) {
          score += 5;
        }
      }
      prevIdx = ti;
    }
  }

  if (qi < q.length) return 0; // Not all chars matched

  // Penalize long target relative to query
  const lenRatio = q.length / t.length;
  return score + Math.floor(lenRatio * 5);
}

// ── Pinyin helpers ────────────────────────────────────────────────

const CHINESE_RE = /[\u4e00-\u9fff]/;

/** Generate pinyin tokens for a Chinese string: full pinyin + first letters. */
function generatePinyinTokens(text: string): string[] {
  if (!CHINESE_RE.test(text)) return [];

  try {
    const tokens: string[] = [];

    // Full pinyin without tones: "计算" → "jisuan"
    const full = pinyin(text, { toneType: "none", type: "string" });
    // Remove spaces between characters for continuous matching
    const fullNoSpace = full.replace(/\s+/g, "");
    tokens.push(fullNoSpace);

    // First letters: "计算" → "js"
    const firstLetters = pinyin(text, {
      pattern: "first",
      toneType: "none",
      type: "string",
    }).replace(/\s+/g, "");
    if (firstLetters !== fullNoSpace) {
      tokens.push(firstLetters);
    }

    return tokens;
  } catch {
    // pinyin-pro may fail on rare characters or in restricted environments
    return [];
  }
}

// ── Match helper ──────────────────────────────────────────────────

/**
 * Score a single target string against query.
 * Returns a score (0 = no match).
 * Priority: exact > prefix > contains > fuzzy.
 */
function matchTarget(query: string, target: string): number {
  const t = target.toLowerCase();
  if (t === query) return 100;
  if (t.startsWith(query)) return 80;
  if (t.includes(query)) return 60;
  return fuzzyScore(query, t) * 2;
}

// ── Registry ──────────────────────────────────────────────────────

class PluginRegistry {
  private plugins: Plugin[] = [];

  register(plugin: Plugin) {
    this.plugins.push(plugin);

    // Generate pinyin tokens for all Chinese keywords + plugin name
    const pinyinTokens: string[] = [];
    try {
      for (const kw of plugin.keywords) {
        generatePinyinTokens(kw).forEach(t => pinyinTokens.push(t));
      }
      generatePinyinTokens(plugin.name).forEach(t => {
        if (!pinyinTokens.includes(t)) pinyinTokens.push(t);
      });
    } catch {
      // pinyin generation failed — search falls back to keyword/name match only
    }
    plugin._pinyinTokens = pinyinTokens;

    // Diagnostic: log pinyin tokens for plugins with Chinese keywords
    if (pinyinTokens.length > 0) {
      console.log(`[lunac] Pinyin tokens for "${plugin.name}":`, pinyinTokens);
    }
  }

  /** 摘掉一个已注册的插件（**按 id**）。
   *
   *  为什么需要它（L1，2026-09-21）：第三方插件是**运行时**从磁盘注册进来的，用户装完 / 卸载后
   *  要重新同步一次。registry 本身不去重，直接二次 `register()` 会让结果区出现两行同名插件；
   *  所以「重新注册」的正确姿势永远是 `unregister(id)` → `register(plugin)`。
   *  （内置插件不需要它：它们在启动时注册一次，之后不再变。） */
  unregister(id: string) {
    const i = this.plugins.findIndex(p => p.id === id);
    if (i >= 0) this.plugins.splice(i, 1);
  }

  /** Search plugins by query. Uses fuzzy matching + keyword index + pinyin. */
  search(query: string): Plugin[] {
    const q = query.toLowerCase().trim();
    if (!q) return this.plugins.slice(0, 6);

    const scored: { plugin: Plugin; score: number }[] = [];
    const seen = new Set<string>();

    for (const plugin of this.plugins) {
      if (seen.has(plugin.id)) continue;

      let score = 0;
      const nameLower = plugin.name.toLowerCase();

      // ── Name match ──
      score = Math.max(score, matchTarget(q, nameLower));

      // ── Exact keyword match wins over everything ──
      // (e.g. typing "ocr" must show OCR #1, not fuzzy-matched settings)
      for (const kw of plugin.keywords) {
        if (kw.toLowerCase() === q) {
          score = Math.max(score, 200); // exact keyword override
        }
      }

      // ── Keyword match ──
      for (const kw of plugin.keywords) {
        const kwLower = kw.toLowerCase();
        score = Math.max(score, matchTarget(q, kwLower));
      }

      // ── Pinyin match ──
      if (plugin._pinyinTokens) {
        for (const py of plugin._pinyinTokens) {
          score = Math.max(score, matchTarget(q, py));
        }
      }

      // ── Description match (lower weight) ──
      const descLower = plugin.description.toLowerCase();
      if (descLower.includes(q)) {
        score = Math.max(score, 30);
      } else {
        score = Math.max(score, fuzzyScore(q, descLower));
      }

      if (score > 0) {
        scored.push({ plugin, score });
        seen.add(plugin.id);
      }
    }

    scored.sort((a, b) => b.score - a.score);
    return scored.slice(0, 8).map(s => s.plugin);
  }

  getAll(): Plugin[] {
    return [...this.plugins];
  }
}

export const pluginRegistry = new PluginRegistry();
