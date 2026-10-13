// ── 桌宠「输出框随内容封顶 + 窗口尺寸」纯逻辑自检（2026-10-13）────────────
// 本仓库**没有前端单测运行器**（package.json 只有 tsc / vite），故按 testing.md §测试 5 的
// 既有惯例，用**可被 node 直接跑**的自检脚本覆盖纯函数（与 pet-typewriter-selftest.mjs 同型）。
//
// 跑法（仓库根）：
//   node --experimental-strip-types scripts/pet-window-size-selftest.mjs
// exit 0 = 全过；非 0 = 有断言失败（node:assert 抛错即非 0）。
//
// 被测对象：app/src/plugins/builtin/pet-window-size.ts（纯函数，无 DOM / 无 tauri）。

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  IN_H,
  OUT_CAP_H,
  OUT_MIN_H,
  OUT_STEP,
  STAGE_GAP,
  STAGE_H,
  STAGE_MIN_H,
  STAGE_W,
  WIN_BASE_H,
  WIN_H_MIN,
  WIN_SIDE_MAX,
  WIN_W_DEFAULT,
  WIN_W_MIN,
  baseHeightFromWindow,
  clampOutTop,
  clampWindowSize,
  modelBaseScale,
  outCap,
  quantizeTopH,
  resizePlan,
  shouldResize,
  targetWindowHeight,
} from "../app/src/plugins/builtin/pet-window-size.ts";

let passed = 0;
function check(name, fn) {
  fn();
  passed++;
  console.log(`  ok - ${name}`);
}

console.log("pet-window-size 自检");

// ── 内容高 → 目标盒高（下限 75 / 上限 255 / 正常中间值 / 超过上限）────────
check("盒高：下限 75（无内容 / 内容不足预留带）", () => {
  assert.equal(clampOutTop(0), OUT_MIN_H);
  assert.equal(clampOutTop(40), OUT_MIN_H);
  assert.equal(clampOutTop(75), OUT_MIN_H);
});
check("盒高：正常中间值（贴合内容、向上取整）", () => {
  assert.equal(clampOutTop(120), 120);
  assert.equal(clampOutTop(120.4), 121); // 向上取整：宁可多 1px 也不要少 1px 出滚动条
  assert.equal(clampOutTop(200), 200);
});
check("盒高：超过上限 ⇒ 封顶（盒内滚动）", () => {
  assert.equal(clampOutTop(255), OUT_CAP_H);
  assert.equal(clampOutTop(400), OUT_CAP_H);
  assert.equal(clampOutTop(100000), OUT_CAP_H);
});
check("盒高：非有限数按无内容（下限）", () => {
  assert.equal(clampOutTop(NaN), OUT_MIN_H);
  assert.equal(clampOutTop(Infinity), OUT_MIN_H); // 非有限 ⇒ 当无内容处理
});

// ── 盒高量化（节流主手段）────────────────────────────────────────
check("量化：相对下限向上取整到 OUT_STEP 档位", () => {
  assert.equal(quantizeTopH(OUT_MIN_H), OUT_MIN_H);
  assert.equal(quantizeTopH(OUT_MIN_H + 1), OUT_MIN_H + OUT_STEP);
  assert.equal(quantizeTopH(OUT_MIN_H + OUT_STEP), OUT_MIN_H + OUT_STEP);
  assert.equal(quantizeTopH(OUT_MIN_H + OUT_STEP + 1), OUT_MIN_H + 2 * OUT_STEP);
});
check("量化：到顶即回上限（不越过 OUT_CAP_H）", () => {
  assert.equal(quantizeTopH(OUT_CAP_H), OUT_CAP_H);
  assert.equal(quantizeTopH(OUT_CAP_H + 50), OUT_CAP_H);
  assert.equal(quantizeTopH(1e9), OUT_CAP_H);
});

// ── 盒高 → 目标窗口高（基准 510 + 超出下限部分）────────────────────────
check("目标窗高：下限 75 ⇒ 基准高（= 510）", () => {
  assert.equal(targetWindowHeight(WIN_BASE_H, OUT_MIN_H), WIN_BASE_H);
  assert.equal(WIN_BASE_H, 510);
});
check("目标窗高：中间值 / 封顶", () => {
  assert.equal(targetWindowHeight(WIN_BASE_H, 135), 510 + 60);
  assert.equal(targetWindowHeight(WIN_BASE_H, OUT_CAP_H), 510 + (OUT_CAP_H - OUT_MIN_H));
});
check("目标窗高：用户改过基准高（拖大过窗口）也照样叠加", () => {
  assert.equal(targetWindowHeight(700, 135), 700 + 60);
});

// ── 记忆 / 用户尺寸 clamp（打开时用）──────────────────────────────
check("尺寸 clamp：合法值原样（取整）", () => {
  assert.deepEqual(clampWindowSize(400, 620), { w: 400, h: 620 });
  assert.deepEqual(clampWindowSize(400.4, 620.6), { w: 400, h: 621 });
});
check("尺寸 clamp：越界 / 非法兜住", () => {
  assert.deepEqual(clampWindowSize(10, 10), { w: WIN_W_MIN, h: WIN_H_MIN });
  assert.deepEqual(clampWindowSize(9_999, 9_999), { w: WIN_SIDE_MAX, h: WIN_SIDE_MAX });
  assert.deepEqual(clampWindowSize(NaN, NaN), { w: WIN_W_DEFAULT, h: WIN_BASE_H });
});

// ── 基准高反推（用户拖拽记账 / 程序化恒等）────────────────────────
check("基准高反推：对程序化改动恒等（不会把自动调整误记成用户改动）", () => {
  for (const topH of [OUT_MIN_H, 135, OUT_CAP_H]) {
    const winH = targetWindowHeight(WIN_BASE_H, topH);
    assert.equal(baseHeightFromWindow(winH, topH), WIN_BASE_H);
  }
});
check("基准高反推：内容为 0（带高 = 下限）时 = 当前窗高", () => {
  assert.equal(baseHeightFromWindow(640, OUT_MIN_H), 640);
});

// ── shouldResize（差 1px 才发）───────────────────────────────────
check("应否下发：差 < 1px 视为同值", () => {
  assert.equal(shouldResize(510, 510), false);
  assert.equal(shouldResize(510, 510.5), false);
  assert.equal(shouldResize(510, 530), true);
});

// ── 节流：逐字增长不产生逐帧 resize ──────────────────────────────
check("节流：内容每次 +1px（逐字增长）只跨档才下发（远少于帧数）", () => {
  const heights = [];
  for (let c = 0; c <= OUT_CAP_H + 40; c += 1) heights.push(c); // 每「字」内容 +1px，共 ~295 拍
  const plan = resizePlan(heights);
  // 每档一个下发（75/95/.../255）+ 起始那一次；粗算上限 = 档数
  const maxSteps = Math.ceil((OUT_CAP_H - OUT_MIN_H) / OUT_STEP) + 1;
  assert.ok(plan.length <= maxSteps, `plan=${plan.length} 超过档数 ${maxSteps}`);
  assert.ok(plan.length < heights.length / 5, `下发次数 ${plan.length} 未明显少于拍数 ${heights.length}`);
  // 序列单调不减，且首尾正确
  for (let i = 1; i < plan.length; i++) assert.ok(plan[i] >= plan[i - 1]);
  assert.equal(plan[0], WIN_BASE_H); // 无内容
  assert.equal(plan[plan.length - 1], targetWindowHeight(WIN_BASE_H, OUT_CAP_H)); // 封顶
});
check("节流：内容不变不重复下发", () => {
  assert.deepEqual(resizePlan([100, 100, 100, 100]), [targetWindowHeight(WIN_BASE_H, quantizeTopH(100))]);
});

// ══════════════════════════════════════════════════════════════════
//  2026-10-13 第二轮：画布跟随窗口 + 模型基准「只缩不放」（modelBaseScale）
// ══════════════════════════════════════════════════════════════════

// ── 参考模型区尺寸（唯一真相源）与缺省窗口自洽 ────────────────────
check("参考尺寸：STAGE_W/STAGE_H/STAGE_GAP 与缺省窗口自洽", () => {
  assert.equal(STAGE_W, 300);
  assert.equal(STAGE_H, 400);
  assert.equal(STAGE_GAP, WIN_W_DEFAULT - STAGE_W); // 缺省左右各 10
  assert.equal(STAGE_GAP, 20);
  // 缺省窗口（宽 320、高 510）减去留白 / 输出带下限 / 输入带 ⇒ 画布恰为参考尺寸
  assert.equal(WIN_W_DEFAULT - STAGE_GAP, STAGE_W); // 画布宽 = 参考宽
  assert.equal(WIN_BASE_H - OUT_MIN_H - 35, STAGE_H); // 画布高 = 参考高
});

// srcW / srcH = 模型**设计尺寸**（取一个「高」模型：宽 1000、高 2000）。
const SW = 1000;
const SH = 2000;
const REF_W = STAGE_W; // 300
const REF_H = STAGE_H; // 400
const refBase = Math.min(REF_W / SW, REF_H / SH); // = 0.2（按参考尺寸 contain）

check("基准：画布 = 参考尺寸 ⇒ base = 参考基准", () => {
  assert.equal(modelBaseScale(REF_W, REF_H, SW, SH, REF_W, REF_H), refBase);
});
check("基准：画布**更大** ⇒ base 不变（不放大，只多空间）", () => {
  assert.equal(modelBaseScale(REF_W * 2, REF_H * 2, SW, SH, REF_W, REF_H), refBase);
  assert.equal(modelBaseScale(9999, 9999, SW, SH, REF_W, REF_H), refBase); // 再大也不变
});
check("基准：画布**更小** ⇒ 按画布收（不裁切）", () => {
  const b = modelBaseScale(150, 200, SW, SH, REF_W, REF_H); // 半参考
  assert.equal(b, Math.min(150 / SW, 200 / SH)); // = 画布基准
  assert.ok(b < refBase, "更小画布应收缩");
});
check("基准：纵横比不同（画布更宽但更矮）仍取较小者", () => {
  // 画布 600×300：宽充裕、高不足 ⇒ 由高决定 = 300/2000 = 0.15 < 参考基准 0.2
  const b = modelBaseScale(600, 300, SW, SH, REF_W, REF_H);
  assert.equal(b, 300 / SH);
  assert.equal(b, 0.15);
});
check("基准：宽模型也不因画布变宽而放大", () => {
  // 宽模型 2000×1000：参考基准 = min(300/2000, 400/1000) = 0.15（由宽决定）
  const b = modelBaseScale(600, 400, 2000, 1000, REF_W, REF_H);
  assert.equal(b, REF_W / 2000); // = 0.15（不是画布基准 600/2000 = 0.3）
});
check("基准：由该式恒有 base*src ≤ 画布（横向 / 纵向都不溢出 = 不裁切）", () => {
  for (const w of [1, 50, 140, 300, 600, 1200]) {
    for (const h of [1, 90, 200, 400, 800]) {
      const b = modelBaseScale(w, h, SW, SH, REF_W, REF_H);
      assert.ok(b * SW <= w + 1e-9, `w=${w} 横向溢出 ${b * SW}`);
      assert.ok(b * SH <= h + 1e-9, `h=${h} 纵向溢出 ${b * SH}`);
    }
  }
});
check("基准：画布单调变大 ⇒ base 单调不减，且不小于参考后即封顶", () => {
  const seq = [0.5, 1, 2, 4].map(k => modelBaseScale(REF_W * k, REF_H * k, SW, SH, REF_W, REF_H));
  for (let i = 1; i < seq.length; i++) assert.ok(seq[i] >= seq[i - 1]);
  assert.ok(seq[0] < refBase); // 画布 < 参考 ⇒ 收缩（不放大）
  assert.equal(seq[1], refBase); // 画布 = 参考 ⇒ 参考基准
  assert.equal(seq[2], refBase); // 画布 > 参考 ⇒ **不变**（只多空间）
  assert.equal(seq[3], refBase);
});
check("宽度钳制：窗口比参考窄（含最小宽 160）时，模型宽不超出画布", () => {
  const stageW = WIN_W_MIN - STAGE_GAP; // 160 − 20 = 140（< 参考宽 300）
  assert.ok(stageW < STAGE_W);
  const b = modelBaseScale(stageW, REF_H, SW, SH, REF_W, REF_H);
  assert.equal(b, stageW / SW); // 由画布宽决定（比参考基准更小）
  assert.ok(b * SW <= stageW + 1e-9, "横向不留溢出");
});
check("基准：缺省参考（不传 / 非法）回落当前画布 = 只按画布 contain", () => {
  assert.equal(modelBaseScale(600, 400, SW, SH, 0, 0), Math.min(600 / SW, 400 / SH));
});

// ══════════════════════════════════════════════════════════════════
//  2026-10-13 第三轮：最小模型区保护
//    · 动态输出上限 outCap（窗矮 ⇒ 上限降，绝不挤压模型区）
//    · 最小窗高 WIN_H_MIN 由本模块推导、清单与 clamp 三处同源
// ══════════════════════════════════════════════════════════════════

// ── 最小窗高：唯一真相源 = pet-window-size（清单是同值镜像）──────────
check("最小窗高：WIN_H_MIN = OUT_MIN_H + IN_H + STAGE_MIN_H（唯一真相源，无第二份硬编码）", () => {
  assert.equal(STAGE_MIN_H, 140);
  assert.equal(WIN_H_MIN, OUT_MIN_H + IN_H + STAGE_MIN_H);
  assert.equal(WIN_H_MIN, 75 + 35 + 140);
  assert.equal(WIN_H_MIN, 250);
  // 与参考模型区的关系：STAGE_MIN_H < STAGE_H，且最小窗下画布宽（WIN_W_MIN − STAGE_GAP）恰为 140
  assert.ok(STAGE_MIN_H < STAGE_H);
  assert.equal(STAGE_MIN_H, WIN_W_MIN - STAGE_GAP);
});
check("最小窗高：清单 window.minHeight / minWidth 与该推导逐字一致（同源，非第二份硬编码）", () => {
  const ps1 = readFileSync(new URL("./build-plugins.ps1", import.meta.url), "utf8");
  // 该脚本里**只有 pet** 声明了 `window` 段 ⇒ 断言式 `minWidth` / `minHeight` 各一条
  const minH = /minHeight\s*=\s*(\d+)/.exec(ps1);
  const minW = /minWidth\s*=\s*(\d+)/.exec(ps1);
  assert.ok(minH && minW, "build-plugins.ps1 里找不到 pet 的 minWidth/minHeight 声明");
  assert.equal(Number(minH[1]), WIN_H_MIN, "清单 minHeight 与 pet-window-size 的 WIN_H_MIN 不一致");
  assert.equal(Number(minW[1]), WIN_W_MIN, "清单 minWidth 与 WIN_W_MIN 不一致");
});
check("最小窗高：clampWindowSize 把过小 / 非法高度兜到 WIN_H_MIN（老记忆尺寸被纠正）", () => {
  assert.deepEqual(clampWindowSize(320, 200), { w: 320, h: WIN_H_MIN }); // 老用户存的 200 ⇒ 抬到 250
  assert.deepEqual(clampWindowSize(320, 10), { w: 320, h: WIN_H_MIN });
  assert.equal(clampWindowSize(320, WIN_H_MIN).h, WIN_H_MIN); // 刚好最小值不动
  assert.equal(clampWindowSize(320, WIN_H_MIN + 1).h, WIN_H_MIN + 1);
});

// ── 动态输出上限 outCap ──────────────────────────────────────────
check("动态上限：窗高刚好最小值 ⇒ 取下限 75（不挤压模型区）", () => {
  assert.equal(outCap(WIN_H_MIN), OUT_MIN_H);
  assert.equal(outCap(WIN_H_MIN), 75);
});
check("动态上限：窗高 − IN_H − STAGE_MIN_H 恰为下限时取 75", () => {
  assert.equal(outCap(OUT_MIN_H + IN_H + STAGE_MIN_H), OUT_MIN_H); // 同一式，换写法
});
check("动态上限：正常大窗（基准 510 / 之上）⇒ 取静态上限 OUT_CAP_H（正常路径完全不变）", () => {
  assert.equal(outCap(WIN_BASE_H), OUT_CAP_H);
  assert.equal(outCap(450), OUT_CAP_H); // 450 − 175 = 275 ≥ 255 ⇒ 封在 OUT_CAP_H
  assert.equal(outCap(100000), OUT_CAP_H);
});
check("动态上限：窗矮 ⇒ 上限随之下降，且恒在 [OUT_MIN_H, OUT_CAP_H] 内（单调不减）", () => {
  const seq = [WIN_H_MIN, 300, 400, 450, WIN_BASE_H].map(outCap);
  for (let i = 1; i < seq.length; i++) assert.ok(seq[i] >= seq[i - 1], "outCap 应单调不减");
  for (const v of seq) assert.ok(v >= OUT_MIN_H && v <= OUT_CAP_H, `outCap 越界：${v}`);
  assert.equal(outCap(300), 125); // 300 − 175 = 125（介于 75 与 255 之间）
});
check("动态上限：非有限数按基准高兜底（不产生 NaN）", () => {
  assert.equal(outCap(NaN), OUT_CAP_H);
  assert.equal(outCap(Infinity), OUT_CAP_H); // 非有限 ⇒ 按 WIN_BASE_H ⇒ 255
});
check("动态上限：clampOutTop / quantizeTopH 按传入 cap 收窄（不越 cap；缺省 cap 行为不变）", () => {
  const cap = outCap(300); // 125
  assert.equal(clampOutTop(100000, cap), cap);
  assert.equal(clampOutTop(120, cap), 120);
  assert.equal(quantizeTopH(100000, OUT_STEP, cap), cap);
  assert.ok(quantizeTopH(OUT_MIN_H + 1, OUT_STEP, cap) <= cap);
  // 缺省 cap = OUT_CAP_H ⇒ 与加动态口径之前逐字一致
  assert.equal(clampOutTop(100000), OUT_CAP_H);
  assert.equal(quantizeTopH(100000), OUT_CAP_H);
});

// ── 核心不变量：模型区「永不为 0」───────────────────────────────
check("模型区永不为 0：任意合法窗高下 窗高 − 盒高 − IN_H ≥ STAGE_MIN_H（满内容最坏情形）", () => {
  for (let H = WIN_H_MIN; H <= 2000; H += 3) {
    const box = outCap(H); // 内容够长 ⇒ 盒高恰为上限
    const stage = H - box - IN_H;
    assert.ok(stage >= STAGE_MIN_H, `窗高 ${H}：模型区 ${stage} < ${STAGE_MIN_H}`);
  }
});
check("模型区永不为 0：内容驱动路径（基准高 ≥ WIN_H_MIN）同理", () => {
  for (let base = WIN_H_MIN; base <= 2000; base += 7) {
    const cap = outCap(base);
    const box = quantizeTopH(clampOutTop(1e6, cap), OUT_STEP, cap); // 满内容
    const winH = targetWindowHeight(base, box);
    const stage = winH - box - IN_H;
    assert.ok(stage >= STAGE_MIN_H, `基准 ${base}：模型区 ${stage} < ${STAGE_MIN_H}`);
  }
});
check("病态不可达：不存在合法窗高 < WIN_H_MIN；且 H = WIN_H_MIN 时模型区恰为 STAGE_MIN_H", () => {
  assert.equal(clampWindowSize(300, 199).h, WIN_H_MIN); // 前端不放行
  assert.equal(clampWindowSize(300, WIN_H_MIN - 1).h, WIN_H_MIN);
  assert.equal(WIN_H_MIN - outCap(WIN_H_MIN) - IN_H, STAGE_MIN_H);
});

console.log(`\n全部通过：${passed} 项`);
