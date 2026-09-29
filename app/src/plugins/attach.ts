// ── 插件面板「挂载」统一出口 ──────────────────────────────────────
// 2026-09-27。主窗口的内嵌面板（main.ts 的 executePlugin）与插件悬浮窗
// （plugin-window.ts）共用这一份映射 —— 否则同一条 `if (plugin.id === "music")`
// 会存在两处，加了插件只改一边就会出现「内嵌能用、悬浮窗是死的」。
//
// **为什么是动态 import**：每个 `attach*Listeners` 都在模块顶层持有自己的状态与
// 定时器，必须**懒加载**；静态 import 会让没打开的插件也把模块跑起来。
//
// **这张表只剩基础插件**（2026-09-29 用户定：基础 = 备忘录 / AI 助手 / 网页搜索 /
// 设置 / 快速启动 / 翻译，见 kinds.ts 的 BASE_PLUGIN_IDS）。拓展插件（剪贴板历史 / OCR /
// 音乐歌词 / 文件转换）已不再编译进 bundle —— 它们的 `attach` / `detach` 由插件入口
// 模块自己导出，一律走下面 default 分支的 `market.ts::externalAttach()`。
// **不要**再往这张表里加拓展插件：那会把整份插件代码拖回 bundle，
// 「卸载 = 完全不存在于本应用」就不成立了。
//
// **不在表里的基础插件**（有意）：
//   · `ai-agent` —— 对话流就是主窗口本身，没有「挂载」这一步；
//   · `web-search` / `clipboard-history` —— 一次性结果，没有监听器要挂。
// 调用方拿到 `false` 应当视为「这个插件不需要挂载」，而不是错误。

import type { Plugin } from "./registry";
import { externalAttach, externalDetach } from "./market";

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
    case "translate": {
      const m = await import("./builtin/translate");
      m.attachTranslateListeners(root);
      return true;
    }
    default:
      // 拓展插件：入口模块自带 `attach(root)` 就调它（没有则 false，同「不需要挂载」）
      return await externalAttach(plugin.id, root);
  }
}

/** 面板关闭时的收尾。模块内大多另有 `isConnected` 自停兜底，这里是显式一次 ——
 *  不依赖 DOM 时序（与 main.ts 的 closePluginView 同一条纪律）。
 *
 *  2026-09-29：音乐 / 转换也成了拓展插件，收尾一律交给 `externalDetach`（调模块自己
 *  导出的 `detach`）。基础插件（settings / memo / quick-launch）的监听都挂在会被整体
 *  替换掉的结果区 DOM 上，本来就不需要显式收尾。 */
export function detachPluginListeners(pluginId: string): void {
  externalDetach(pluginId);
}
