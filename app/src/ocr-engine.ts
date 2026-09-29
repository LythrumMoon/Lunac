// ── OCR 引擎（PaddleOCR-json）的安装入口 ──────────────────────────
// 2026-09-29：从 `plugins/builtin/ocr.ts` 搬到这里。
//
// **为什么搬**：OCR 已归入**拓展插件**（见 `plugins/kinds.ts`），它的模块会被打包成
// 独立 ESM 投进 `Modules\ocr\`，不再随主 bundle 编译。但「设置 → OCR 引擎」那一节
// 仍由**基础插件 settings** 渲染 —— 若 settings 继续 `import("./ocr.js")`，整份 OCR
// 代码又会被拖回 bundle（正是 `Modules\` 化要消掉的耦合）。
//
// 引擎本身是**宿主资源**（下载到 `<exe 根>\paddle-ocr`，由 `src-tauri` 的 OCR 命令使用），
// 与「插件装没装」无关，所以这个「装引擎」的小封装放在宿主侧最合适：
// settings 与 ocr 插件各自 import 它，谁都不会因此把对方打进自己的包。
//
// 依赖的命令与事件都在宿主（见 `src-tauri/src/ocr*.rs`）：`ocr_engine_install`、
// `ocr-engine-progress` / `ocr-engine-ready` / `ocr-engine-error`。

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** 触发引擎下载安装。进度经 `ocr-engine-progress`/`ready`/`error` 事件回传。
 *  返回 Promise<boolean>：true = 安装成功。 */
export function installOcrEngine(
  onProgress?: (info: { percent: number; mb: number }) => void,
): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    const unlisteners: Array<() => void> = [];
    const cleanup = () => {
      for (const fn of unlisteners) { try { fn(); } catch { /* ignore */ } }
    };
    void (async () => {
      unlisteners.push(await listen("ocr-engine-progress", (ev) => {
        const { downloaded, total } = ev.payload as { downloaded: number; total: number };
        onProgress?.({
          percent: total > 0 ? Math.round((downloaded / total) * 100) : 0,
          mb: downloaded / 1048576,
        });
      }));
      unlisteners.push(await listen("ocr-engine-ready", () => { cleanup(); resolve(true); }));
      unlisteners.push(await listen("ocr-engine-error", () => { cleanup(); resolve(false); }));
      try {
        await invoke("ocr_engine_install");
      } catch {
        cleanup();
        resolve(false);
      }
    })();
  });
}
