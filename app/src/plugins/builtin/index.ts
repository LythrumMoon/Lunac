// Register all built-in plugins
import { pluginRegistry } from "../registry";
import { quickLaunchPlugin } from "./quick-launch";
import { settingsPlugin } from "./settings";
import { webSearchPlugin } from "./web-search";
import { aiAgentPlugin } from "./ai-agent";
import { clipboardHistoryPlugin } from "./clipboard-history";
import { ocrPlugin } from "./ocr";
import { memoPlugin } from "./memo";
import { musicPlugin } from "./music";
import { convertPlugin } from "./convert";
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
  // 音乐歌词 — LRCLIB 歌词抓取 + Spotify 播放控制（一个插件两件事，2026-09-27）
  pluginRegistry.register(musicPlugin);
  // 文件转换 — 图片 / 音频 / 视频互转（走本机 ffmpeg，2026-09-27）
  pluginRegistry.register(convertPlugin);
}
