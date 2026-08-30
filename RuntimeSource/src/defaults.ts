import { CallCore } from "./core.js";
import type { FetchLike } from "./types.js";
import { AnthropicProvider } from "./providers/anthropic.js";
import { GeminiProvider } from "./providers/gemini.js";
import { OpenAIProvider } from "./providers/openai.js";
import { OpenRouterProvider } from "./providers/openrouter.js";

export interface DefaultCoreOptions {
  openaiApiKey?: string;
  anthropicApiKey?: string;
  geminiApiKey?: string;
  geminiAccessToken?: string;
  geminiProjectId?: string;
  openrouterApiKey?: string;
  openrouterAppUrl?: string;
  openrouterAppName?: string;
  fetch?: FetchLike;
}

function env(name: string): string | undefined {
  const processLike = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process;
  return processLike?.env?.[name];
}

export function createDefaultCore(options: DefaultCoreOptions = {}): CallCore {
  const fetchOption = options.fetch ? { fetch: options.fetch } : {};
  const openaiApiKey = options.openaiApiKey ?? env("OPENAI_API_KEY");
  const anthropicApiKey = options.anthropicApiKey ?? env("ANTHROPIC_API_KEY");
  const geminiApiKey = options.geminiApiKey ?? env("GEMINI_API_KEY");
  const openrouterApiKey = options.openrouterApiKey ?? env("OPENROUTER_API_KEY");
  const geminiAccessToken = options.geminiAccessToken;

  return new CallCore([
    new OpenAIProvider({ ...(openaiApiKey ? { apiKey: openaiApiKey } : {}), ...fetchOption }),
    new AnthropicProvider({ ...(anthropicApiKey ? { apiKey: anthropicApiKey } : {}), ...fetchOption }),
    new GeminiProvider({
      ...(geminiApiKey ? { apiKey: geminiApiKey } : {}),
      ...(geminiAccessToken ? { accessToken: geminiAccessToken } : {}),
      ...(options.geminiProjectId ? { projectId: options.geminiProjectId } : {}),
      ...fetchOption,
    }),
    new OpenRouterProvider({
      ...(openrouterApiKey ? { apiKey: openrouterApiKey } : {}),
      ...(options.openrouterAppUrl ? { appUrl: options.openrouterAppUrl } : {}),
      ...(options.openrouterAppName ? { appName: options.openrouterAppName } : {}),
      ...fetchOption,
    }),
  ]);
}
