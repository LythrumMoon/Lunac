// ── 下载进度的公共件：环形进度 + 实时速度（2026-09-30）─────────────
//
// **为什么单独一个文件**：两个**不同的 bundle** 都要用它 —— 主 bundle 的
// `plugins/builtin/settings.ts`（插件市场的下载按钮）与单独打包的 `plugins/builtin/ocr.ts`
// （插件内的引擎下载提示）。后者按插件打包成独立 ESM（见 `vite.plugins.config.ts`），
// 所以这份代码会在两个产物里各存一份 —— 这正是它**必须零依赖**的原因（别 import i18n：
// 那会把 i18n 整份拖进插件包）。文案由调用方给，这里只负责数字与形状。
//
// **速度为什么在前端算**：宿主只报事实（已下字节 + 总字节，见
// `plugin_market::DependencyProgress`）—— 速度是「相邻两次上报之间的差值」，
// 只有拿着时钟的那一端算才有意义。事件间隔不匀（宿主每 64KB 报一次，网快时一秒几十次），
// 直接拿差值当速度会跳得跟坏了一样，所以做一次**指数平滑**（EMA）。

/** 一次上报算出来的东西。 */
export interface DownloadSample {
  /** 0~100；`total = 0`（服务端没给长度）时为 -1，表示「算不出百分比」 */
  percent: number;
  /** 字节/秒；首次上报或刚重置时为 0（还没有差值可用） */
  speed: number;
  downloaded: number;
  total: number;
}

/**
 * 把宿主的上报折算成「百分比 + 速度」。
 *
 * 用法：`push(downloaded, total)` 每次上报调一次，返回当前该显示什么。
 * `reset()` 换一个文件（插件包 → 依赖 1 → 依赖 2）时调 —— 否则会把上一个文件的
 * 字节数当成本次基线，算出一个荒唐的速度。
 */
export class DownloadMeter {
  private lastBytes = 0;
  private lastAt = 0;
  private speed = 0;
  private primed = false;
  /** 两次速度采样之间的最小间隔（秒）：太密了差值全是噪声 */
  private static readonly MIN_DT = 0.4;

  push(downloaded: number, total: number, now: number = Date.now()): DownloadSample {
    const percent = total > 0 ? Math.min(100, Math.round((downloaded / total) * 100)) : -1;
    if (!this.primed) {
      this.primed = true;
      this.lastBytes = downloaded;
      this.lastAt = now;
      return { percent, speed: 0, downloaded, total };
    }
    const dt = (now - this.lastAt) / 1000;
    if (dt >= DownloadMeter.MIN_DT) {
      const instant = Math.max(0, (downloaded - this.lastBytes) / dt);
      // EMA：α = 0.35 是「跟得上变化」与「不抖」之间的折中
      this.speed = this.speed === 0 ? instant : this.speed * 0.65 + instant * 0.35;
      this.lastBytes = downloaded;
      this.lastAt = now;
    }
    return { percent, speed: this.speed, downloaded, total };
  }

  reset(): void {
    this.lastBytes = 0;
    this.lastAt = 0;
    this.speed = 0;
    this.primed = false;
  }
}

/** `12345678` → `11.8 MB`（用于「还没有总字节」时显示已下多少）。 */
export function formatBytes(n: number): string {
  if (!(n > 0)) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

/** `1234567` → `1.2 MB/s`；速度为 0 时给一个占位（不要显示「0 B/s」，那看着像卡死了）。 */
export function formatSpeed(bytesPerSec: number): string {
  if (!(bytesPerSec > 0)) return "—";
  return `${formatBytes(bytesPerSec)}/s`;
}

/**
 * 环形进度的 HTML（小圆圈 + 中间数字）。
 *
 * 用 SVG 而不是 conic-gradient：**圆心的洞必须是透明的** —— 设置面板与插件面板都是
 * 半透明玻璃底，拿一个纯色圆去盖会在两处露出不同的色块。`pathLength="100"` 让
 * `stroke-dashoffset` 直接等于「剩下的百分比」，不必自己算周长。
 */
export function ringHtml(percent: number): string {
  const known = percent >= 0;
  const offset = known ? 100 - Math.max(0, Math.min(100, percent)) : 100;
  const label = known ? String(Math.round(percent)) : "";
  return `
    <span class="dl-ring-wrap${known ? "" : " is-unknown"}">
      <svg class="dl-ring" viewBox="0 0 36 36" aria-hidden="true">
        <circle class="dl-ring-track" cx="18" cy="18" r="15.5" pathLength="100"></circle>
        <circle class="dl-ring-fill" cx="18" cy="18" r="15.5" pathLength="100"
                style="stroke-dashoffset:${offset}"></circle>
      </svg>
      <span class="dl-ring-pct">${label}</span>
    </span>`;
}

/** 更新一个已经画在页面上的环（`ringHtml` 产出的那个节点），返回是否找到了节点。 */
export function updateRing(root: ParentNode, percent: number): boolean {
  const wrap = root.querySelector<HTMLElement>(".dl-ring-wrap");
  const fill = root.querySelector<SVGCircleElement>(".dl-ring-fill");
  const pct = root.querySelector<HTMLElement>(".dl-ring-pct");
  if (!wrap || !fill) return false;
  const known = percent >= 0;
  wrap.classList.toggle("is-unknown", !known);
  fill.style.strokeDashoffset = String(known ? 100 - Math.max(0, Math.min(100, percent)) : 100);
  if (pct) pct.textContent = known ? String(Math.round(percent)) : "";
  return true;
}
