// ── 本地决策模型（S1 形态 B：子进程 `llama-server`，2026-10-09）──────────────
//
// 这是一个**基础设施插件**（`kinds.ts` 的 `isHeadlessPlugin`）：**只提供能力，没有用户入口**。
//   · **不出现**在搜索结果里，插件市场行也**不给「打开」**；
//   · 它的**开关**在「设置 → AI → 本地模型决策」——由设置面板负责启停宿主托管的
//     `llama-server` 子进程（走 `app/src/plugins/sidecar.ts` 的 sidecar 桥）。
//   · 用户仍能在插件市场**下载 / 卸载**它（`local-model-<ver>.zip`）。
//
// 它服务两处「要不要把这条输入发给云模型」的决策（判定在 `core-agent/src/decide.rs`）：
//   ① 桌宠插件的消息发送决策；② agent 对话流的输入门。
//
// **模型与二进制都走清单 `dependencies` 随装下载**（llama-server archive + 468MB GGUF，
// sha256 校验）；`permissions:["process.spawn"]` 必须与清单 `sidecar` 段同时出现，否则整包拒装。
//
// **本文件刻意没有任何 UI 逻辑**：面板已删除，`execute` 只留一句兜底说明（见下）。

import type { Plugin, PluginResult } from "../registry";

const localModelPlugin: Plugin = {
  id: "local-model",
  name: "本地决策模型",
  keywords: ["本地模型", "local model", "llama", "llama-server", "决策", "离线", "gguf", "省钱"],
  description: "本地小模型（llama-server）—— 只参与「要不要把消息发给云模型」的决策，省钱",
  icon: "🧠",
  permissions: [],
  async execute(): Promise<PluginResult> {
    // 无面板（headless）：入口已从搜索移除、市场也不给「打开」。这里只留一句兜底说明 ——
    // 万一被别的路径调到，用户知道去哪开关，而不是看到一个空白面板。
    return {
      type: "html",
      content:
        `<div style="padding:12px 14px;font-size:.78rem;color:var(--text);line-height:1.7;">` +
        `本地决策模型没有独立界面 —— 请在「设置 → AI → 本地模型决策」里开关。</div>`,
    };
  },
};

export default localModelPlugin;
