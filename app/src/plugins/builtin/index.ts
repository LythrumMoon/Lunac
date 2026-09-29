// Register all built-in plugins
//
// 2026-09-29：这里**只注册基础插件**（名单见 `../kinds.ts` 的 `BASE_PLUGIN_IDS`）。
// 拓展插件（剪贴板历史 / OCR / 文件转换 / 音乐歌词）不再编译进 bundle ——
// 它们从市场装进 `<exe 根>\Modules\<id>\`，由 `market.ts` 的 `refreshMarketPlugins()`
// 在启动时扫盘注册（见 `main.ts` 的 init 顺序）。**不要**在这里再 import 它们，
// 那会把整份插件代码拖回 bundle，`卸载 = 完全不存在` 就不成立了。
import { pluginRegistry } from "../registry";
import { quickLaunchPlugin } from "./quick-launch";
import { settingsPlugin } from "./settings";
import { webSearchPlugin } from "./web-search";
import { aiAgentPlugin } from "./ai-agent";
import { memoPlugin } from "./memo";
import { translatePlugin } from "./translate";
import toolEditor from "./tool-editor";

export function registerBuiltinPlugins() {
  pluginRegistry.register(quickLaunchPlugin);
  pluginRegistry.register(settingsPlugin);
  // Web Search — search the web via default browser
  pluginRegistry.register(webSearchPlugin);
  // AI Agent 放最后当兜底
  pluginRegistry.register(aiAgentPlugin);
  // 工具编辑器（MCP 桥管理）—— 归在「AI 助手」名下（见 kinds.ts 的 MERGED_INTO）：
  // 市场上不单独占一行，但仍是一个独立、可被搜到的插件。
  pluginRegistry.register(toolEditor);
  // 备忘录 — 本地自动保存 (点17)
  pluginRegistry.register(memoPlugin);
  // 翻译 — 词典做底座 + 模型补漏 + 译文落 SQLite（2026-09-29，基础插件）
  pluginRegistry.register(translatePlugin);
}
