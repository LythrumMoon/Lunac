// ── 桌宠「输出框随内容封顶 + 窗口尺寸」的纯逻辑（2026-10-13）──────────────
// 本模块**只做纯计算**（无 DOM / 无 tauri / 无副作用），因此可被 pet.ts 内联进插件包，
// 也可被 `scripts/pet-window-size-selftest.mjs` 用 node 直接跑单测（见该脚本）。
//
// 口径（用户 2026-10-13 裁决，逐字落实；同日第二轮补「画布 / 宽度跟随窗口」）：
//   · 输出框高度**跟内容走、但封顶**：下限 = 预留 75px（保住「模型不跳」的既有裁决），
//     上限 = OUT_CAP_H；**超过上限才在盒内滚动**（短 / 中回复不再出现滚动条）。
//   · 盒变高时**窗口跟着变高**（只动输出带，模型区与输入带不参与 ⇒ 模型不位移）；
//     内容少了窗口缩回基准高。
//   · 模型区（画布）**跟随窗口剩余空间**（`.pet-stage` 用 flex 分配）：画布变大时模型**不放大**、
//     只多空间；画布小于参考尺寸时按画布收（不裁切）。这条「只缩不放」由 modelBaseScale 保证。
//   · 窗口可调宽高、且**尺寸要记住**：打开时用记忆尺寸（再 clamp 到合法范围）。
//   · **最小模型区保护**（2026-10-13 第三轮）：输出带的上限**动态**取 `outCap(窗口基准高)`
//     （= 窗高 − IN_H − STAGE_MIN_H，再夹到 [OUT_MIN_H, OUT_CAP_H]）⇒ 窗矮则上限自动降低、
//     **绝不把模型区压到 0**；同时把最小窗高抬到 `WIN_H_MIN = OUT_MIN_H + IN_H + STAGE_MIN_H`
//     ⇒「窗高 < 该式」的状态根本不可达（三处口径同源，见 WIN_H_MIN 注释）。

/** 输出带**预留下限**（px）—— 与 PET_CSS 的 `.pet-top` 缺省 `flex-basis` 一致：
 *  保住「开合对话时模型不跳动」的既有裁决。 */
export const OUT_MIN_H = 75;
/** **参考模型区高**（px）—— 缺省窗口（510）减去输出带下限 75 与输入带 35 = 400。
 *  ⚠️ 2026-10-13 语义变更：**不再是 `.pet-stage` 的固定高度** —— 画布高度现由 flex 跟随窗口
 *  （= 窗高 − 输出带 − 输入带）。它现在只作「参考尺寸」：模型基准缩放（只缩不放）与基准窗高
 *  （WIN_BASE_H）都按它算。 */
export const STAGE_H = 400;
/** **参考模型区宽**（px）—— 缺省窗口宽（320）减去左右各 10 的留白 = 300（= WIN_W_DEFAULT −
 *  STAGE_GAP）。同样只作「参考尺寸」：画布宽度由 flex 跟随窗口（= 窗宽 − STAGE_GAP）。 */
export const STAGE_W = 300;
/** 模型区（画布）的**合法最小高度**（px）—— 「最小模型区保护」的核心常量（2026-10-13 第三轮）。
 *  取 **140**：① = 参考模型区高 STAGE_H(400) 的 35%；② = 最小窗口宽下的画布宽
 *  （WIN_W_MIN − STAGE_GAP = 160 − 20 = 140）；③ 于是在最小合法窗口下模型区仍是一个
 *  「不低于其宽度」的竖向可辨认区域，而不会退化成一条缝。
 *  它同时把**最小窗高**抬到 `OUT_MIN_H + IN_H + STAGE_MIN_H`（见 WIN_H_MIN）——
 *  于是「输出带封顶(255) + 输入带(35) 把模型区压到 0」那个状态**根本不可达**。 */
export const STAGE_MIN_H = 140;
/** 输入带高（px）—— 与 `.pet-bottom` 一致。 */
export const IN_H = 35;
/** 无内容、且模型区为参考尺寸时的**基准窗高**（= 75 + 400 + 35 = 510）。用户没手动改过就用它。
 *  程序化改动恒满足「窗高 = 基准高 + (输出带高 − OUT_MIN_H)」，见 targetWindowHeight。 */
export const WIN_BASE_H = OUT_MIN_H + STAGE_H + IN_H;
/** 输出框高度的**上限**（px）—— 取默认窗高的一半（510 / 2 = 255）。理由见模块头与 pet.ts 引用处：
 *  再高就会盖住大半个屏幕且离模型太远；≈13 行正文（`.72rem` × `line-height 1.6`）已够绝大多数回复。 */
export const OUT_CAP_H = 255;

/** 窗口宽的默认值（px）—— 与清单 `window.width` 一致。 */
export const WIN_W_DEFAULT = 320;
/** 模型区左右留白合计（px）= `WIN_W_DEFAULT − STAGE_W`（缺省 10px × 2）。
 *  由 `.pet-stage` 的 `calc(100% − var(--pet-stage-gap))` 消费 ⇒ 缺省窗口下画布恰为参考宽。 */
export const STAGE_GAP = WIN_W_DEFAULT - STAGE_W;
/** 窗口宽 / 高的合法下限（px）。宽 160 与清单 `window.minWidth` 一致。
 *  ⚠️ **最小窗高由本模块推导、是唯一真相源**（2026-10-13 第三轮）：
 *  `WIN_H_MIN = OUT_MIN_H + IN_H + STAGE_MIN_H` = 75 + 35 + 140 = **250**（原 200）。
 *  消费者三处必须口径一致：① 本模块的 `clampWindowSize`（前端打开 / 拖拽 clamp）；
 *  ② 清单 `window.minHeight`（`scripts/build-plugins.ps1` 的 pet 块，值取本式 = 250）；
 *  ③ 宿主 `plugin_window_resize` / `plugin_window_resize_anchored` 的 clamp（`clamp_size` → `min_size`
 *  读**清单**的 `minHeight`，故天然同源）。**别在任一处硬编码第二份最小窗高。** */
export const WIN_W_MIN = 160;
export const WIN_H_MIN = OUT_MIN_H + IN_H + STAGE_MIN_H;
/** 窗口单边上限（px）—— 与宿主 `plugin_window_resize` 的 clamp 上限同源。 */
export const WIN_SIDE_MAX = 2000;

/** 盒高量化档位（px）：逐字动画增长时**只在跨过档位时才 resize**（节流，避免逐帧下发 IPC）。
 *  取 20 ≈ 一行正文高，肉眼几乎看不出档位跳变，却把一次动画的 resize 次数从「每字一次」
 *  压到最多 9 次（75→255 共 9 档）。 */
export const OUT_STEP = 20;

/** 输出带高度的**动态上限**（px）—— 「最小模型区保护」的另一半（2026-10-13 第三轮）。
 *  模型区永不小于 STAGE_MIN_H ⇒ 输出带 ≤ 窗高 − IN_H − STAGE_MIN_H；再把结果夹到
 *  `[OUT_MIN_H, OUT_CAP_H]`。`winH` 传**窗口基准高**（输出带处于下限 OUT_MIN_H 时的窗高，
 *  见 WIN_BASE_H / baseHeightFromWindow）：内容驱动的生长满足
 *  `窗高 = 基准高 + (带高 − OUT_MIN_H)` ⇒ 模型区高 = 基准高 − OUT_MIN_H − IN_H **与带高无关**，
 *  故以基准高推上限是**稳定**的；若改用「当前窗高」（= 基准高 + 带高 − 下限），带越高窗越高、
 *  上限又越大，会自我放大成正反馈。窗矮 ⇒ 上限自动降到下限（**不挤压模型区**）；
 *  窗高（基准 ≥ 450 时 `窗高 − 175 ≥ 255`）⇒ 取静态上限 OUT_CAP_H ⇒ **正常路径完全不变**。 */
export function outCap(winH: number): number {
  const h = Number.isFinite(winH) ? winH : WIN_BASE_H;
  return Math.min(OUT_CAP_H, Math.max(OUT_MIN_H, h - IN_H - STAGE_MIN_H));
}

/** 内容高 → 目标「输出带高」：夹到 `[OUT_MIN_H, cap]`，向上取整（宁可多 1px 也不要
 *  因取整少 1px 而平白多出滚动条）。非有限数按无内容（= 下限）。
 *  `cap` 缺省 = 静态上限 OUT_CAP_H（行为不变）；窗矮时由调用方传 `outCap(基准高)` 把它压低。 */
export function clampOutTop(contentH: number, cap = OUT_CAP_H): number {
  const c = Math.min(OUT_CAP_H, Math.max(OUT_MIN_H, Number.isFinite(cap) ? cap : OUT_CAP_H));
  if (typeof contentH !== "number" || !Number.isFinite(contentH)) return OUT_MIN_H;
  return Math.min(c, Math.max(OUT_MIN_H, Math.ceil(contentH)));
}

/** 盒高**量化到档位**（相对下限向上取整；到顶即回上限）。逐字增长时的节流主手段。
 *  `cap`（缺省 = OUT_CAP_H）是**动态上限**（= `outCap(基准高)`）：量化后的值不超过它。 */
export function quantizeTopH(topH: number, step = OUT_STEP, cap = OUT_CAP_H): number {
  const c = Math.min(OUT_CAP_H, Math.max(OUT_MIN_H, Number.isFinite(cap) ? cap : OUT_CAP_H));
  const t = Math.min(c, Math.max(OUT_MIN_H, topH));
  if (t >= c) return c;
  const q = OUT_MIN_H + Math.ceil((t - OUT_MIN_H) / step) * step;
  return Math.min(c, Math.max(OUT_MIN_H, q));
}

/** 输出带高 → 目标窗口高：基准高 + 超出下限的那部分（模型区 / 输入带不参与，故它们不位移）。 */
export function targetWindowHeight(baseH: number, topH: number): number {
  return baseH + (topH - OUT_MIN_H);
}

/** 记忆尺寸 / 用户尺寸 → clamp 到合法区间（打开时用）。非法值回落默认（宽）与基准高。 */
export function clampWindowSize(w: number, h: number): { w: number; h: number } {
  const cw =
    typeof w === "number" && Number.isFinite(w)
      ? Math.round(Math.min(WIN_SIDE_MAX, Math.max(WIN_W_MIN, w)))
      : WIN_W_DEFAULT;
  const ch =
    typeof h === "number" && Number.isFinite(h)
      ? Math.round(Math.min(WIN_SIDE_MAX, Math.max(WIN_H_MIN, h)))
      : WIN_BASE_H;
  return { w: cw, h: ch };
}

/** 目标窗口高是否与上次**成功下发**的不同（差 < 1px 视为同值，避免浮点抖动触发下发）。 */
export function shouldResize(prevH: number, nextH: number): boolean {
  return Math.abs(prevH - nextH) >= 1;
}

/** 从「当前窗高 + 当前输出带高」**反推基准高**（用户手动拖拽改尺寸后记账用）。
 *  因为程序化下发满足 `窗高 = 基准高 + (带高 - 下限)`，故该式对**程序化**改动是恒等的 ——
 *  于是「每次 resize 事件都记账」不会漂移，也不会把程序化改动误当成用户改动。 */
export function baseHeightFromWindow(curWindowH: number, topH: number): number {
  const base = curWindowH - (topH - OUT_MIN_H);
  return Number.isFinite(base) ? base : WIN_BASE_H;
}

/** 逐字增长时的**下发序列**（供实现与单测共用）：给一串「内容高」，返回每次实际下发的窗口高。
 *  连续增长（每次 +1px）不会产生逐帧 resize —— 只有跨过 `OUT_STEP` 档位才发一次。
 *  上限按 `outCap(baseH)` 取（窗矮 ⇒ 封顶更低，见 outCap）。 */
export function resizePlan(contentHeights: number[], baseH = WIN_BASE_H): number[] {
  const cap = outCap(baseH);
  const out: number[] = [];
  let last = -1;
  for (const c of contentHeights) {
    const target = targetWindowHeight(baseH, quantizeTopH(clampOutTop(c, cap), OUT_STEP, cap));
    if (last < 0 || shouldResize(last, target)) {
      out.push(target);
      last = target;
    }
  }
  return out;
}

/** 模型**基准缩放** = 「参考尺寸基准」与「当前画布基准」的较小者（用户 2026-10-13 口径）。
 *  · 参考基准 = `min(refW/srcW, refH/srcH)`：按**参考模型区**（缺省 300×400）contain；
 *  · 画布基准 = `min(hostW/srcW, hostH/srcH)`：按**当前画布** contain。
 *  取较小者 = **只缩不放**：画布比参考大 ⇒ 结果 = 参考基准（模型不放大、只是周围多出空间）；
 *  画布比参考小 ⇒ 结果 = 画布基准（按画布收，模型仍完整可见、不被裁切）。
 *  由该式恒有 `base*srcW ≤ hostW` 且 `base*srcH ≤ hostH`（模型在画布内不溢出）。
 *  `srcW/srcH` = 模型的**设计尺寸**（引擎侧 = mount 时的 `model.width/height`）；非法值按 1 兜底。 */
export function modelBaseScale(
  hostW: number,
  hostH: number,
  srcW: number,
  srcH: number,
  refW: number,
  refH: number,
): number {
  const w = Math.max(1, Number.isFinite(hostW) ? hostW : 1);
  const h = Math.max(1, Number.isFinite(hostH) ? hostH : 1);
  const sw = srcW > 0 && Number.isFinite(srcW) ? srcW : 1;
  const sh = srcH > 0 && Number.isFinite(srcH) ? srcH : 1;
  const rw = refW > 0 && Number.isFinite(refW) ? refW : w;
  const rh = refH > 0 && Number.isFinite(refH) ? refH : h;
  const canvasBase = Math.min(w / sw, h / sh);
  const refBase = Math.min(rw / sw, rh / sh);
  return Math.min(refBase, canvasBase);
}
