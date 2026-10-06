// src/screenshot-overlay.ts
// 截屏框选覆盖窗的脚本（2026-10-06）。宿主 `screenshot_overlay_begin` 开这扇窗，
// 本页只做三件事：① 取回整张截图铺满；② 让用户拖一个框；③ 把**像素矩形**回给宿主。
//
// **为什么不让前端裁剪**：图是宿主的会话态（RGBA 留在内存里），前端只回矩形，
// 宿主按矩形切片即可 —— 几 MB 的图不必再走一次 IPC（见 screenshot.rs 的模块注释）。
//
// **坐标怎么算（与预检 #72 ④ 同一条）**：`img` 被拉满整窗（`width/height: 100%`），
// 而窗口本身铺满整块虚拟桌面 ⇒ 屏幕上就是 1:1；图片的**像素**坐标 =
// (鼠标 CSS 坐标 − img 渲染矩形左上角) × (naturalWidth ÷ 渲染宽)，即
// `scale = 渲染宽 ÷ naturalWidth` 的倒数。别拿 `screen.width` / `innerWidth` 去算 ——
// DPI 缩放、窗口尺寸都会让它错位。

import { invoke } from "@tauri-apps/api/core";
import { initI18n, loadSavedLanguage, t } from "./i18n.js";

type Shot = { dataUrl: string; width: number; height: number };

/** 小于这个边长（CSS px）就不算框选，当作「点了一下」= 取消。 */
const MIN_SIDE = 4;

async function main() {
  await initI18n();
  loadSavedLanguage();

  const img = document.querySelector<HTMLImageElement>("#shot-img");
  const root = document.querySelector<HTMLElement>("#shot-root");
  const dim = document.querySelector<HTMLElement>("#shot-dim");
  const box = document.querySelector<HTMLElement>("#shot-box");
  const sizeEl = document.querySelector<HTMLElement>("#shot-size");
  const hint = document.querySelector<HTMLElement>("#shot-hint");
  if (!img || !root || !dim || !box || !sizeEl || !hint) return;

  hint.textContent = t("ocr.shot_hint");

  let shot: Shot;
  try {
    shot = await invoke<Shot>("screenshot_overlay_image");
  } catch {
    // 取不到图（会话没了 / 抓帧失败）⇒ 别把用户晾在一层黑幕上
    void invoke("screenshot_overlay_cancel");
    return;
  }
  img.src = shot.dataUrl;

  let dragging = false;
  /** 结算只能有一次：finish / cancel 之后锁死，避免 mouseup 与 Esc 打架。 */
  let settled = false;
  let startX = 0;
  let startY = 0;

  const settle = (fn: () => void) => {
    if (settled) return;
    settled = true;
    fn();
  };
  const cancel = () => settle(() => void invoke("screenshot_overlay_cancel"));

  /** 框的位置 + 尺寸读数。坐标直接用 clientX/clientY（覆盖窗里没有别的偏移）。 */
  const paint = (cx: number, cy: number) => {
    const left = Math.min(startX, cx);
    const top = Math.min(startY, cy);
    const w = Math.abs(cx - startX);
    const h = Math.abs(cy - startY);
    box.style.left = `${left}px`;
    box.style.top = `${top}px`;
    box.style.width = `${w}px`;
    box.style.height = `${h}px`;
    sizeEl.textContent = `${Math.round(w)} × ${Math.round(h)}`;
    // 读数贴着框的右下角外侧；靠近边缘时折回框内，别被窗口裁掉
    sizeEl.style.left = `${left + w + 6}px`;
    sizeEl.style.top = `${top + h + 6}px`;
  };

  // 绑在**容器**上而不是 `img` 上：容器里还压着暗幕 / 框 / 读数 / 提示这几层，
  // 绑在 img 上时任何一层忘了 `pointer-events: none` 都会把拖拽吞掉（已踩过一次）。
  root.addEventListener("mousedown", e => {
    if (e.button !== 0 || settled) return;
    if (!img.naturalWidth) return; // 图还没解码完，量不出映射
    dragging = true;
    startX = e.clientX;
    startY = e.clientY;
    // 拖起来之后：暗幕交给「框外面那一圈巨大阴影」，框内因此恢复原亮度
    dim.hidden = true;
    box.hidden = false;
    sizeEl.hidden = false;
    paint(e.clientX, e.clientY);
    e.preventDefault();
  });

  window.addEventListener("mousemove", e => {
    if (dragging) paint(e.clientX, e.clientY);
  });

  window.addEventListener("mouseup", e => {
    if (!dragging || settled) return;
    dragging = false;

    const w = Math.abs(e.clientX - startX);
    const h = Math.abs(e.clientY - startY);
    if (w < MIN_SIDE || h < MIN_SIDE) {
      cancel(); // 「点了一下」= 取消，别弹一个 1×1 的识别
      return;
    }

    const natW = img.naturalWidth;
    const natH = img.naturalHeight;
    const ir = img.getBoundingClientRect();
    if (!natW || !natH || ir.width <= 0) {
      cancel();
      return;
    }
    // 渲染尺寸 → 图片像素；再用同一个 scale 把矩形搬过去
    const scaleX = natW / ir.width;
    const scaleY = natH / ir.height;
    const left = Math.min(startX, e.clientX) - ir.left;
    const top = Math.min(startY, e.clientY) - ir.top;
    const x = Math.round(left * scaleX);
    const y = Math.round(top * scaleY);
    const cw = Math.round(w * scaleX);
    const ch = Math.round(h * scaleY);

    settle(() => {
      void invoke("screenshot_overlay_finish", { x, y, width: cw, height: ch }).catch(() => {
        // 裁剪失败（会话没了 / 太小）⇒ 收掉这扇窗，别让它悬着
        void invoke("screenshot_overlay_cancel");
      });
    });
  });

  // Esc 取消
  window.addEventListener("keydown", e => {
    if (e.key === "Escape") cancel();
  });
  // 右键取消（ShareX 同款）
  window.addEventListener("contextmenu", e => {
    e.preventDefault();
    cancel();
  });
  // 拖动图片会触发浏览器默认的「拖拽图片」——那会把框选打断
  window.addEventListener("dragstart", e => e.preventDefault());
}

void main();
