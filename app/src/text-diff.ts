// src/text-diff.ts
// 行级 diff（U1 代码变更 diff 卡，2026-10-06）。**纯函数、不碰 DOM、不引新依赖** ——
// 仓库里没有现成的 diff 实现（`similar` 这类 crate 也没进依赖树），而这里要的只是
// 「给人看的两栏差异」，不需要 patch / 合并能力。
//
// 三步走，每一步都为了**有界**：
//   ① **裁掉公共前后缀**：绝大多数改动只动文件的一小块，裁完之后要比较的中段通常只有几十行；
//   ② 中段跑 **LCS**（标准 DP + 回溯，交错出 `del` / `add`）—— 但只在
//      `|A| × |B| ≤ LCS_BUDGET` 时才跑（DP 是 O(n·m) 的）；
//   ③ 超预算 ⇒ 如实降级成「整段删 + 整段加」（不假装能对齐），并只留
//      `MAX_LINES` 行输出。
//
// ⚠️ **统计数字与截断无关**：`added` / `removed` 是裁完前后缀就定死的精确值，
// 不受 LCS 降级与 `MAX_LINES` 影响 —— 面板上的「+A −B」因此永远是对的。

/** 一行差异。`eq` = 未变（只作上下文出现），`del` = 旧侧独有，`add` = 新侧独有。 */
export interface DiffLine {
  kind: "eq" | "add" | "del";
  text: string;
}

export interface DiffResult {
  /** 新增行数（精确值） */
  added: number;
  /** 删除行数（精确值） */
  removed: number;
  /** 待渲染的行（**含少量上下文 eq 行**；可能是被截断的尾部） */
  lines: DiffLine[];
  /** `lines` 不是全部（改动太大）—— 渲染层要如实标一句 */
  truncated: boolean;
}

/** 中段 DP 的预算（`|A| × |B|` 格）。40 万格 ≈ 几毫秒，够用且不会卡住界面。 */
const LCS_BUDGET = 400_000;
/** 一次最多渲染多少行（超出就截断并如实标注）。 */
const MAX_LINES = 800;
/** 改动块前后各留几行上下文（**不留整份文件** —— 那是 diff 不是源码查看器）。 */
const CONTEXT = 3;

/** 按行切分。**行尾的 `\n` 不算一行**（`"a\nb\n"` 是 2 行，不是 3 行）；CRLF 归一成 LF。 */
function splitLines(s: string): string[] {
  if (s === "") return [];
  const norm = s.replace(/\r\n?/g, "\n");
  return norm.endsWith("\n") ? norm.slice(0, -1).split("\n") : norm.split("\n");
}

/** 标准 LCS DP + 回溯，交错输出 del / add / eq。**只给小中段用**（调用方判预算）。 */
function lcsDiff(a: string[], b: string[]): DiffLine[] {
  const n = a.length;
  const m = b.length;
  // dp[i][j] = a[i..] 与 b[j..] 的最长公共子序列长度
  const dp: Uint32Array[] = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      out.push({ kind: "eq", text: a[i] });
      i++;
      j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      out.push({ kind: "del", text: a[i] });
      i++;
    } else {
      out.push({ kind: "add", text: b[j] });
      j++;
    }
  }
  while (i < n) out.push({ kind: "del", text: a[i++] });
  while (j < m) out.push({ kind: "add", text: b[j++] });
  return out;
}

/**
 * 两段文本的行级差异。
 *
 * `oldText` 为空串 = 新文件（整份都是新增）；`newText` 为空串 = 文件被删（整份都是删除）。
 */
export function lineDiff(oldText: string, newText: string): DiffResult {
  const a = splitLines(oldText ?? "");
  const b = splitLines(newText ?? "");

  // ① 公共前缀 / 后缀（逐行比，一次扫描）
  let p = 0;
  while (p < a.length && p < b.length && a[p] === b[p]) p++;
  let s = 0;
  while (s < a.length - p && s < b.length - p && a[a.length - 1 - s] === b[b.length - 1 - s]) s++;

  const midA = a.slice(p, a.length - s);
  const midB = b.slice(p, b.length - s);

  const lines: DiffLine[] = [];
  for (let i = Math.max(0, p - CONTEXT); i < p; i++) lines.push({ kind: "eq", text: a[i] });

  // ② / ③ 中段：能对齐就对齐，预算不够就如实降级
  if (midA.length * midB.length <= LCS_BUDGET) {
    lines.push(...lcsDiff(midA, midB));
  } else {
    for (const t of midA) lines.push({ kind: "del", text: t });
    for (const t of midB) lines.push({ kind: "add", text: t });
  }

  const suffixStart = a.length - s;
  for (let i = suffixStart; i < Math.min(a.length, suffixStart + CONTEXT); i++) {
    lines.push({ kind: "eq", text: a[i] });
  }

  let truncated = false;
  if (lines.length > MAX_LINES) {
    lines.length = MAX_LINES;
    truncated = true;
  }

  // 统计是裁完前后缀就定死的精确值（与上面的截断 / 降级无关）
  return { added: midB.length, removed: midA.length, lines, truncated };
}
