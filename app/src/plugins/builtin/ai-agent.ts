// AI Agent plugin — delegates streaming chat to main.ts
import type { Plugin } from "../registry";

export const aiAgentPlugin: Plugin = {
  id: "ai-agent",
  name: "AI Agent",
  keywords: [
    "ai", "chat", "ask", "help", "gpt", "assistant",
    "问", "对话", "智能", "助手",
  ],
  description: "Ask AI anything — code, writing, analysis & more",
  icon: "🤖",
  badge: "AI",

  async execute(input: string) {
    const trimmed = input.trim().toLowerCase();
    const triggerWords = ["ai", "chat", "ask", "help", "gpt", "assistant", "问", "对话", "智能", "助手"];
    const isJustKeyword = triggerWords.some(k => k === trimmed);
    if (isJustKeyword || !trimmed) {
      // Start fresh chat (not history) — user hit Enter on the AI Agent result
      (window as any).__lunac_start_ai_chat?.("");
    } else {
      (window as any).__lunac_start_ai_chat?.(input);
    }
    return { type: "text", content: "" };
  },
};
