// Register all built-in plugins
import { pluginRegistry } from "../registry";
import { quickLaunchPlugin } from "./quick-launch";
import { settingsPlugin } from "./settings";
import { webSearchPlugin } from "./web-search";
import { aiAgentPlugin } from "./ai-agent";
import { clipboardHistoryPlugin } from "./clipboard-history";
import { ocrPlugin } from "./ocr";
import { memoPlugin } from "./memo";
import toolEditor from "./tool-editor";

export function registerBuiltinPlugins() {
  pluginRegistry.register(quickLaunchPlugin);
  pluginRegistry.register(settingsPlugin);
  pluginRegistry.register(clipboardHistoryPlugin);
  // Web Search — search the web via default browser
  pluginRegistry.register(webSearchPlugin);
  // AI Agent is last as fallback
  pluginRegistry.register(aiAgentPlugin);
  // Tool editor (MCP bridge management)
  pluginRegistry.register(toolEditor);
  // OCR 文字识别 — PaddleOCR-json (PP-OCRv4 模型，离线高精度)
  pluginRegistry.register(ocrPlugin);
  // 备忘录 — 本地自动保存 (点17)
  pluginRegistry.register(memoPlugin);
}
