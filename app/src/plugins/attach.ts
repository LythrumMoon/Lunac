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

/** 按 id 调**基础插件**那套挂载监听。**内置插件与磁盘插件的 `reuse` 共用这一份映射** ——
 *  只写一处，别让「内置怎么挂」和「复用怎么挂」两份漂移。返回是否真的挂上了
 *  （不是基础插件 / 没有对应监听 ⇒ false）。 */
async function attachBaseListeners(id: string, root: HTMLElement): Promise<boolean> {
  switch (id) {
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
      return false;
  }
}

export async function attachPluginListeners(plugin: Plugin, root: HTMLElement): Promise<boolean> {
  // 基础插件：直接走那一份映射
  if (await attachBaseListeners(plugin.id, root)) return true;
  // 拓展插件：先调它自己入口模块导出的 `attach(root)`（没有则 false = 不需要挂载）
  const selfAttached = await externalAttach(plugin.id, root);
  // 再按清单里的 `reuse` **追加**挂基础插件那套监听（2026-10-05 用户要求：自定义插件
  // 按钮失效时允许「复用内置插件监听」）。这样 AI 生成的插件只要声明 `reuse: ["settings"]`
  // 之类，并照搬对应基础插件的面板 HTML，按钮就能真的工作 —— 而不是「只有后端有反应」。
  // 认不出的 id 只忽略并 warn（不因为一个笔误让整块面板挂掉）。
  let reused = false;
  for (const id of plugin.reuse ?? []) {
    if (id === plugin.id) continue;
    if (await attachBaseListeners(id, root)) reused = true;
    else console.warn(`[lunac] 插件 ${plugin.id} 声明了 reuse:${id}，但宿主里没有这个基础插件的监听，已忽略`);
  }
  return selfAttached || reused;
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
