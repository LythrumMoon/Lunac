// ── 桌宠「打字机」纯逻辑自检（2026-10-12）────────────────────────────
// 本仓库**没有前端单测运行器**（package.json 只有 tsc / vite，无 vitest / jest），
// 故按 testing.md §测试 5 的既有惯例，用**可被 node 直接跑**的自检脚本覆盖纯函数。
//
// 跑法（仓库根）：
//   node --experimental-strip-types scripts/pet-typewriter-selftest.mjs
// exit 0 = 全过；非 0 = 有断言失败（node:assert 抛错即非 0）。
//
// 被测对象：app/src/plugins/builtin/pet-typewriter.ts（纯函数，无 DOM / 无 tauri）。

import assert from "node:assert/strict";
import {
  ANIM_MAX_MS,
  RATE_DEFAULT,
  RATE_MAX,
  RATE_MIN,
  clampRate,
  normalizeReplyText,
  shouldAnimateDirectly,
  splitUnits,
} from "../app/src/plugins/builtin/pet-typewriter.ts";

let passed = 0;
function check(name, fn) {
  fn();
  passed++;
  console.log(`  ok - ${name}`);
}

console.log("pet-typewriter 自检");

// ── 换行归一（覆盖：连续换行 / 首尾换行 / 首尾空行 / 空串 / 纯空白）──
check("归一：换行 → 单空格、连续空白折叠、首尾丢弃", () => {
  assert.equal(normalizeReplyText("\n\n你好\n\n世界\n\n"), "你好 世界");
  assert.equal(normalizeReplyText("  a \t b \r\n c  "), "a b c");
  assert.equal(normalizeReplyText("行一\r\n行二\r行三\n行四"), "行一 行二 行三 行四");
});
check("归一：空串 / 纯空白 → 空串", () => {
  assert.equal(normalizeReplyText(""), "");
  assert.equal(normalizeReplyText("   \n\t\r\n  "), "");
});

// ── 单位切分（中英混排 / 标点吸附 / 全角逐字 / 词按空白切）──────────
check("切分：全角字符各自成单位", () => {
  assert.deepEqual(splitUnits("你好世界"), ["你", "好", "世", "界"]);
});
check("切分：西文按空白切成「词」，不是按字母", () => {
  assert.deepEqual(splitUnits("hello world foo"), ["hello ", "world ", "foo"]);
});
check("切分：标点吸附到前一个单位，不占节拍", () => {
  // 逗号并入「好」，句号并入「界」⇒ 4 个单位，而非 6 个
  assert.deepEqual(splitUnits("你好，世界。"), ["你", "好，", "世", "界。"]);
  assert.deepEqual(splitUnits("hello, world!"), ["hello, ", "world!"]);
});
check("切分：中英混排（全角逐字 + 词块）", () => {
  assert.deepEqual(splitUnits("你好 hello 世界"), ["你", "好 ", "hello ", "世", "界"]);
});
check("切分：不变量 join === 原文（对已归一文本）", () => {
  for (const raw of ["你好，世界。ok! 123 呢？", "a b c", "，行首标点", "", "   ", "\n\n 混合 a\n b  "]) {
    const t = normalizeReplyText(raw);
    assert.equal(splitUnits(t).join(""), t, `join 不等于原文：${JSON.stringify(t)}`);
  }
});
check("切分：空串 → 空数组", () => {
  assert.deepEqual(splitUnits(""), []);
});

// ── 打字速度夹取（只驱动逐字动画节拍；与语音语速相互独立）──────────
check("打字速度：越界 / 非法速率被夹取到 [RATE_MIN, RATE_MAX]", () => {
  assert.equal(clampRate(0), RATE_MIN);
  assert.equal(clampRate(99), RATE_MAX);
  assert.equal(clampRate(NaN), RATE_DEFAULT); // 非法 → 回落默认速率
  assert.equal(clampRate(-1), RATE_MIN);
  assert.equal(clampRate(1e9), RATE_MAX);
});

// ── 上限保护（超长文本直接整体显示）──────────────────────────────
check("上限：常规短文本走动画", () => {
  assert.equal(shouldAnimateDirectly(20, 2, ANIM_MAX_MS), false);
});
check("上限：超长文本直接整体显示（2000 字 @2/秒）", () => {
  assert.equal(shouldAnimateDirectly(2000, 2, ANIM_MAX_MS), true);
});
check("上限：空文本直接整体显示", () => {
  assert.equal(shouldAnimateDirectly(0, 2, ANIM_MAX_MS), true);
});

// ── 样例总时长换算（与真机验收口径一致，便于人工核对）────────────
check("样例：20 字全角 @2/秒 = 10 秒；10 词 @2/秒 = 5 秒", () => {
  const zh = "春眠不觉晓处处闻啼鸟夜来风雨声花落知多少"; // 20 个全角字符
  const zhUnits = splitUnits(normalizeReplyText(zh));
  assert.equal(zhUnits.length, 20);
  assert.equal((zhUnits.length * 1000) / RATE_DEFAULT, 10_000);

  const en = "one two three four five six seven eight nine ten"; // 10 个空格分隔词
  const enUnits = splitUnits(normalizeReplyText(en));
  assert.equal(enUnits.length, 10);
  assert.equal((enUnits.length * 1000) / RATE_DEFAULT, 5_000);
});

console.log(`\n全部通过：${passed} 项`);
