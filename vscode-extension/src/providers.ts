// ── Provider configuration ────────────────────────────────────────
// Mirrors chat.rs BUILTIN_PROVIDERS. All providers speak OpenAI-compatible
// /v1/chat/completions (streamed) except Anthropic which uses its own Messages API.

interface ProviderConfig {
  name: string;
  defaultUrl: string;
  defaultModel: string;
  format: "openai" | "anthropic";
  keyEnv: string;
}

export const PROVIDERS: Record<string, ProviderConfig> = {
  deepseek: {
    name: "deepseek",
    defaultUrl: "https://api.deepseek.com",
    defaultModel: "deepseek-flash",
    format: "openai",
    keyEnv: "DEEPSEEK_API_KEY",
  },
  openai: {
    name: "openai",
    defaultUrl: "https://api.openai.com",
    defaultModel: "gpt-4o-mini",
    format: "openai",
    keyEnv: "OPENAI_API_KEY",
  },
  anthropic: {
    name: "anthropic",
    defaultUrl: "https://api.anthropic.com",
    defaultModel: "claude-sonnet-4-20250514",
    format: "anthropic",
    keyEnv: "ANTHROPIC_API_KEY",
  },
  google: {
    name: "google",
    defaultUrl: "https://generativelanguage.googleapis.com/v1beta/openai",
    defaultModel: "gemini-2.5-flash",
    format: "openai",
    keyEnv: "GEMINI_API_KEY",
  },
  zhipu: {
    name: "zhipu",
    defaultUrl: "https://open.bigmodel.cn/api/paas/v4",
    defaultModel: "glm-4-plus",
    format: "openai",
    keyEnv: "ZHIPU_API_KEY",
  },
  moonshot: {
    name: "moonshot",
    defaultUrl: "https://api.moonshot.cn/v1",
    defaultModel: "moonshot-v1-auto",
    format: "openai",
    keyEnv: "MOONSHOT_API_KEY",
  },
  qwen: {
    name: "qwen",
    defaultUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    defaultModel: "qwen-plus",
    format: "openai",
    keyEnv: "DASHSCOPE_API_KEY",
  },
  siliconflow: {
    name: "siliconflow",
    defaultUrl: "https://api.siliconflow.cn",
    defaultModel: "Qwen/Qwen3-235B-A22B",
    format: "openai",
    keyEnv: "SILICONFLOW_API_KEY",
  },
};

export interface ResolvedConfig {
  model: string;
  apiUrl: string;
  apiKey: string;
  format: "openai" | "anthropic";
}

/** Resolve provider config from VS Code settings, with env-var fallback chain. */
export function resolveConfig(providerName: string): ResolvedConfig {
  const config = PROVIDERS[providerName];

  if (config) {
    // API key: settings → provider-specific env var（不再从 ANTHROPIC_API_KEY 全局回退）
    const apiKey =
      getSetting("apiKey") ||
      getEnv(config.keyEnv) ||
      "";
    const model = getSetting("model") || config.defaultModel;
    const apiUrl = getSetting("apiUrl") || config.defaultUrl;

    if (!apiKey && config.keyEnv) {
      throw new Error(
        `No API key found. Set lunac.apiKey in settings or ${config.keyEnv} env var.`
      );
    }
    return { model, apiUrl, apiKey, format: config.format };
  }

  // Custom provider
  if (providerName === "custom") {
    const apiUrl = getSetting("apiUrl");
    const apiKey = getSetting("apiKey") || "";
    const model = getSetting("model");
    const format = (getSetting("apiFormat") as "openai" | "anthropic") || "openai";

    if (!apiUrl) throw new Error("lunac.apiUrl required for custom provider.");
    if (!apiKey) throw new Error("lunac.apiKey required for custom provider.");
    if (!model) throw new Error("lunac.model required for custom provider.");

    return { model, apiUrl, apiKey, format };
  }

  throw new Error(`Unknown provider: ${providerName}`);
}

// ── Helpers ──────────────────────────────────────────────────────

function getSetting<T = string>(key: string): T {
  const vscode = require("vscode");
  const config = vscode.workspace.getConfiguration("lunac");
  return config.get(key) as T;
}

function getEnv(name: string): string {
  return process.env[name] || "";
}
