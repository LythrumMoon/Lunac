// ── VS Code Chat Participant handler ──────────────────────────────
// Path A: registers "lunac" as a native chat participant in VS Code's
// Chat view (same panel as GitHub Copilot Chat).
//
// Makes streaming HTTP requests directly to the provider API
// (zero intermediaries — no subprocess, no proxy, no token overhead).
//
// Supported providers: DeepSeek · OpenAI · Anthropic(Gemini) · Google
// · Zhipu(GLM) · Moonshot · Qwen · SiliconFlow · Custom (OpenAI-compatible)

import * as vscode from "vscode";
import { resolveConfig, PROVIDERS } from "./providers";

// ── System prompt ────────────────────────────────────────────────
// Mirrors chat.rs SYSTEM_PROMPT

const SYSTEM_PROMPT = [
  "You are an AI coding assistant. Respond concisely in the user's language.",
  "For code questions, provide clean, working solutions with brief explanations.",
  "For general questions, be direct and helpful.",
  "When explaining mathematical concepts, formulas, algorithms, or signal processing,",
  "ALWAYS use LaTeX notation: inline formulas in $...$ (e.g. $E = mc^2$),",
  "display formulas in $$...$$ (e.g. $$\\sum_{n=0}^{\\infty} x[n]z^{-n}$$).",
  "Use proper LaTeX for integrals, sums, matrices, Greek letters, subscripts, superscripts.",
  "For signal processing: Fourier transforms, convolution, Z-transform, filters, etc.",
  "should all be expressed as LaTeX formulas.",
].join(" ");

// ── Message type ─────────────────────────────────────────────────

interface Message {
  role: string;
  content: string;
}

// ── Chat participant registration ────────────────────────────────

export function registerChatParticipant(context: vscode.ExtensionContext) {
  const participant = vscode.chat.createChatParticipant("lunac", async (
    request: vscode.ChatRequest,
    _context: vscode.ChatContext,
    stream: vscode.ChatResponseStream,
    token: vscode.CancellationToken,
  ) => {
    const providerName = getProviderName();

    try {
      const config = resolveConfig(providerName);
      stream.progress("Connecting to " + providerName + "...");

      const messages = buildMessages(request);

      if (config.format === "anthropic") {
        await streamAnthropic(config, messages, stream, token);
      } else {
        await streamOpenAI(config, messages, stream, token);
      }
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      stream.markdown("**Lunac Error:** " + msg);
    }
  });

  // Set icon for the chat participant
  participant.iconPath = vscode.Uri.joinPath(
    context.extensionUri, "media", "icon.png"
  );

  // Register the "configure provider" command
  vscode.commands.registerCommand("lunac.configureProvider", async () => {
    const current = getProviderName();
    const items = Object.keys(PROVIDERS).map((name) => ({
      label: `${PROVIDERS[name].defaultModel}  ·  ${name}`,
      description: PROVIDERS[name].format === "anthropic" ? "Anthropic format" : "OpenAI format",
      picked: name === current,
      provider: name,
    }));
    // Add custom at the end
    items.push({
      label: "Custom endpoint",
      description: "Set lunac.apiUrl + lunac.apiKey + lunac.model in settings",
      picked: current === "custom",
      provider: "custom",
    });

    const choice = await vscode.window.showQuickPick(items, {
      placeHolder: "Select AI provider for Lunac chat...",
      title: "Lunac — Configure Provider",
    });
    if (choice) {
      const config = vscode.workspace.getConfiguration("lunac");
      await config.update("provider", choice.provider, vscode.ConfigurationTarget.Global);
      vscode.window.showInformationMessage(`Lunac provider set to: ${choice.provider}`);
    }
  });

  return participant;
}

// ── Helpers ──────────────────────────────────────────────────────

function getProviderName(): string {
  return vscode.workspace.getConfiguration("lunac").get<string>("provider") || "deepseek";
}

function buildMessages(request: vscode.ChatRequest): Message[] {
  const msgs: Message[] = [
    { role: "system", content: SYSTEM_PROMPT },
    { role: "user", content: request.prompt },
  ];
  return msgs;
}

function openaiEndpoint(apiUrl: string): string {
  let u = apiUrl.replace(/\/+$/, "");
  if (u.endsWith("/chat/completions")) {
    return u;
  }
  const last = u.split("/").pop() || "";
  const isVersionSeg =
    last.length >= 2 &&
    /^v\d+$/i.test(last);
  if (isVersionSeg) {
    return `${u}/chat/completions`;
  }
  return `${u}/v1/chat/completions`;
}

// ── OpenAI-compatible streaming ──────────────────────────────────

async function streamOpenAI(
  config: ReturnType<typeof resolveConfig>,
  messages: Message[],
  stream: vscode.ChatResponseStream,
  token: vscode.CancellationToken,
): Promise<void> {
  const body = JSON.stringify({
    model: config.model,
    messages,
    stream: true,
    stream_options: { include_usage: true },
  });

  const controller = new AbortController();
  token.onCancellationRequested(() => controller.abort());

  const resp = await fetch(openaiEndpoint(config.apiUrl), {
    method: "POST",
    headers: {
      "Authorization": `Bearer ${config.apiKey}`,
      "Content-Type": "application/json",
    },
    body,
    signal: controller.signal,
  });

  if (!resp.ok) {
    const text = await resp.text().catch(() => "");
    throw new Error(`${config.apiUrl} returned ${resp.status}: ${text.slice(0, 200)}`);
  }

  if (!resp.body) {
    throw new Error("No response body");
  }

  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let stopReason = "end_turn";

  // ── SSE parser (same logic as chat.rs SseParser) ──────────────
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;

    buffer += decoder.decode(value, { stream: true });

    // Process complete SSE frames
    while (true) {
      const pos = buffer.indexOf("\n\n");
      if (pos === -1) break;
      const frame = buffer.slice(0, pos);
      buffer = buffer.slice(pos + 2);

      let data = "";
      for (const line of frame.split("\n")) {
        if (line.startsWith("data: ")) {
          data = line.slice(6).trim();
        }
      }

      if (!data || data === "[DONE]") continue;

      try {
        const chunk = JSON.parse(data);
        const choice = chunk.choices?.[0];
        if (choice) {
          // Text delta
          const text = choice.delta?.content;
          if (text) {
            stream.markdown(text);
          }
          if (choice.finish_reason) {
            stopReason = choice.finish_reason;
          }
        }
      } catch {
        // skip malformed SSE lines
      }
    }
  }

  // Flush remaining buffer content
  decoder.decode();
}

// ── Anthropic Messages streaming ─────────────────────────────────

async function streamAnthropic(
  config: ReturnType<typeof resolveConfig>,
  messages: Message[],
  stream: vscode.ChatResponseStream,
  token: vscode.CancellationToken,
): Promise<void> {
  // Anthropic requires x-api-key header, not Bearer
  // Messages endpoint is POST /v1/messages with streaming

  // Build Anthropic-format messages (merge consecutive same-role)
  const systemMessages = messages.filter((m) => m.role === "system");
  const chatMessages = messages.filter((m) => m.role !== "system");

  const systemPrompt = systemMessages.map((m) => m.content).join("\n") || undefined;

  const anthMessages: Array<{ role: string; content: Array<{ type: string; text: string }> }> = [];
  for (const m of chatMessages) {
    const last = anthMessages[anthMessages.length - 1];
    if (last && last.role === m.role) {
      last.content.push({ type: "text", text: m.content });
      continue;
    }
    anthMessages.push({
      role: m.role,
      content: [{ type: "text", text: m.content }],
    });
  }

  const body: Record<string, unknown> = {
    model: config.model,
    messages: anthMessages,
    stream: true,
    max_tokens: 8192,
  };
  if (systemPrompt) {
    body.system = systemPrompt;
  }

  const anthUrl = `${config.apiUrl.replace(/\/+$/, "")}/v1/messages`;

  const controller = new AbortController();
  token.onCancellationRequested(() => controller.abort());

  const resp = await fetch(anthUrl, {
    method: "POST",
    headers: {
      "x-api-key": config.apiKey,
      "anthropic-version": "2023-06-01",
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
    signal: controller.signal,
  });

  if (!resp.ok) {
    const text = await resp.text().catch(() => "");
    throw new Error(`${anthUrl} returned ${resp.status}: ${text.slice(0, 200)}`);
  }

  if (!resp.body) {
    throw new Error("No response body");
  }

  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;

    buffer += decoder.decode(value, { stream: true });

    while (true) {
      const pos = buffer.indexOf("\n\n");
      if (pos === -1) break;
      const frame = buffer.slice(0, pos);
      buffer = buffer.slice(pos + 2);

      let eventType = "";
      let data = "";

      for (const line of frame.split("\n")) {
        if (line.startsWith("event: ")) {
          eventType = line.slice(7).trim();
        } else if (line.startsWith("data: ")) {
          data = line.slice(6).trim();
        }
      }

      if (!data) continue;

      // Only emit content_block_delta text
      if (eventType === "content_block_delta") {
        try {
          const parsed = JSON.parse(data);
          const text = parsed.delta?.text;
          if (text) {
            stream.markdown(text);
          }
        } catch {
          // skip
        }
      }
    }
  }
}
