// ── 插件面板「挂载」统一出口 ──────────────────────────────────────
// 2026-09-27。主窗口的内嵌面板（main.ts 的 executePlugin）与插件悬浮窗
// （plugin-window.ts）共用这一份映射 —— 否则同一条 `if (plugin.id === "music")`
// 会存在两处，加了插件只改一边就会出现「内嵌能用、悬浮窗是死的」。
//
// **为什么是动态 import**：每个 `attach*Listeners` 都在模块顶层持有自己的状态与
// 定时器（音乐 1s 轮询、转换的 convert-progress 监听），必须**懒加载**；
// 静态 import 会让没打开的插件也把模块跑起来（主窗口历史上就是这么做的）。
//
// **不在表里的插件**（有意）：
//   · `ocr` —— 它的根是 `document` 且自带 detached 双栏布局，只在主窗口里有意义；
//   · `ai-agent` —— 对话流就是主窗口本身，没有「挂载」这一步；
//   · `web-search` / `clipboard-history` —— 一次性结果，没有监听器要挂。
// 调用方拿到 `false` 应当视为「这个插件不需要挂载」，而不是错误。

import type { Plugin } from "./registry";

export async function attachPluginListeners(plugin: Plugin, root: HTMLElement): Promise<boolean> {
  switch (plugin.id) {
    case "settings": {
      const m = await import("./builtin/settings");
      await m.attachSettingsListeners(root);
      return true;
    }
    case "tool-editor": {
      const m = await import("./builtin/tool-editor");
      m.attachToolEditorListeners();
      return true;
    }
    case "quick-launch": {
      const m = await import("./builtin/quick-launch");
      m.attachQuickLaunchListeners(root);
      return true;
    }
    case "memo": {
      const m = await import("./builtin/memo");
      m.attachMemoListeners(root);
      return true;
    }
    case "music": {
      const m = await import("./builtin/music");
      await m.attachMusicListeners(root);
      return true;
    }
    case "convert": {
      const m = await import("./builtin/convert");
      await m.attachConvertListeners(root);
      return true;
    }
    default:
      return false;
  }
}

/** 面板关闭时的收尾。模块内大多另有 `isConnected` 自停兜底，这里是显式一次 ——
 *  不依赖 DOM 时序（与 main.ts 的 closePluginView 同一条纪律）。 */
export function detachPluginListeners(pluginId: string): void {
  switch (pluginId) {
    case "music":
      (window as any).__lunac_music_stop?.();
      break;
    case "convert":
      (window as any).__lunac_convert_stop?.();
      break;
    default:
      break;
  }
}
