// ── OCR 引擎（PaddleOCR-json）的安装入口 ──────────────────────────
// 2026-09-29：从 `plugins/builtin/ocr.ts` 搬到这里。
//
// **为什么搬**：OCR 已归入**拓展插件**（见 `plugins/kinds.ts`），它的模块会被打包成
// 独立 ESM 投进 `Modules\ocr\`，不再随主 bundle 编译。而「设置 → OCR 引擎」那一节
// 当时仍由**基础插件 settings** 渲染 —— 若 settings 继续 `import("./ocr.js")`，整份 OCR
// 代码又会被拖回 bundle（正是 `Modules\` 化要消掉的耦合）。
//
// **2026-09-30 改**：引擎不再由宿主自下载（`paddle_ocr::install_engine` 已删），
// 它成为 `ocr` 插件清单里的一条 `archive` 依赖。所以这里不再调什么「装引擎」命令，
// 而是调**通用的依赖安装** `install_plugin_dependencies` —— 引擎怎么来的只有一条路：
// 插件清单的 `dependencies[]`（见 ai-spec §3.5 与 plugin_market.rs）。
//
// 谁还用这份封装：`plugins/builtin/ocr.ts`（插件内的「引擎缺失 → 下载」提示）。
// 设置面板那节已删（用户要求：动作移进插件的依赖里，不再单开一个入口）。

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { DownloadMeter, type DownloadSample } from "./download-progress.js";

/** 进度回调收到的东西：`sample` 是百分比 + 速度，`phase` 是宿主当前在干什么。 */
export type OcrEngineProgress = DownloadSample & { phase: string };

/**
 * 触发 `ocr` 插件的依赖安装（= 引擎）。
 *
 * 返回 Promise<boolean>：true = 装好了。进度经 `plugin-install-progress` 事件回传，
 * 只认 `id === "ocr"` 的那几条（插件包本体那一段的 id 是空串，与这里无关）。
 *
 * 用 `try/finally` 收监听：这个封装可能被连点两次，漏收一个监听就会越挂越多。
 */
export function installOcrEngine(
  onProgress?: (info: OcrEngineProgress) => void,
): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    let unlisten: (() => void) | undefined;
    const meter = new DownloadMeter();
    void (async () => {
      try {
        unlisten = await listen("plugin-install-progress", (ev) => {
          const p = ev.payload as {
            id: string;
            phase: string;
            downloaded: number;
            total: number;
          };
          if (p.id !== "ocr") return;
          onProgress?.({ ...meter.push(p.downloaded, p.total), phase: p.phase });
        });
      } catch {
        /* 监听挂不上不该拦住安装本身 —— 大不了没有进度 */
      }
      let ok = false;
      try {
        await invoke("install_plugin_dependencies", { id: "ocr" });
        ok = true;
      } catch {
        ok = false;
      } finally {
        try { unlisten?.(); } catch { /* ignore */ }
      }
      resolve(ok);
    })();
  });
}
