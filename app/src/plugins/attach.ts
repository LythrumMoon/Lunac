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
//   · `ai-agent` —— 对话流就是主窗口本身，没有「挂载」这一步；
//   · `web-search` / `clipboard-history` —— 一次性结果，没有监听器要挂。
// 调用方拿到 `false` 应当视为「这个插件不需要挂载」，而不是错误。
//
// **磁盘插件（第三方 / 用户自建）走模块自带的 `attach(root)`**（2026-09-28 加）：
// 表里查不到就交给 `market.ts` 的 `externalAttach()` —— 它在插件入口模块里找具名/默认导出的
// `attach`。这样「谁需要挂载」这件事由插件自己声明，宿主不必为每个外部插件改一次硬编码表
// （Lunac 自己创建的插件也就跟着这条路走，不需要 dev 版与开发者环境）。
//
// **磁盘插件优先于这张硬编码表**（2026-09-28 补）：同一个 id 既编译进 bundle、又从市场装了
// 一份时（内置双轨期的音乐 / OCR），**用户亲手装的那份必须赢**。否则「在市场上点了下载、
// 装的却是 bundle 里的旧代码」—— 面板上显示新版本号，跑的却是旧的，用户无从察觉。

import type { Plugin } from "./registry";
import { externalAttach, externalDetach, hasDiskPlugin } from "./market";

export async function attachPluginListeners(plugin: Plugin, root: HTMLElement): Promise<boolean> {
  // 盘上有可用的一份 ⇒ 一律走插件自己的 `attach`，**不再往后回落内置表**：
  // 用户装的那份若没导出 `attach`，语义是「它不需要挂载」（`false`），而不是「请用内置那份」。
  if (hasDiskPlugin(plugin.id)) return await externalAttach(plugin.id, root);
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
      // 磁盘插件：入口模块自带 `attach(root)` 就调它（没有则 false，同「不需要挂载」）
      return await externalAttach(plugin.id, root);
  }
}

/** 面板关闭时的收尾。模块内大多另有 `isConnected` 自停兜底，这里是显式一次 ——
 *  不依赖 DOM 时序（与 main.ts 的 closePluginView 同一条纪律）。 */
export function detachPluginListeners(pluginId: string): void {
  // 与 attach 对称：磁盘插件优先。装的是用户那份，收尾也该收它那份 ——
  // 否则会去叫内置实现留下的全局钩子（收错了对象，用户那份的定时器还开着）。
  if (hasDiskPlugin(pluginId)) {
    externalDetach(pluginId);
    return;
  }
  switch (pluginId) {
    case "music":
      (window as any).__lunac_music_stop?.();
      break;
    case "convert":
      (window as any).__lunac_convert_stop?.();
      break;
    default:
      // 表里没有这个 id（`web-search` / `clipboard-history` / `ai-agent` 这类本来就不挂载的，
      // 或真的没挂上过）—— 兜底收一次 externalDetach：没钩子时它是空操作。
      externalDetach(pluginId);
      break;
  }
}
