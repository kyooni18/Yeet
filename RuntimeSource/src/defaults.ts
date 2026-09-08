import { CallCore } from "./core.js";
import type { ProviderFetchLogger } from "./http.js";
import type { FetchLike } from "./types.js";
import { AnthropicProvider } from "./providers/anthropic.js";
import { GeminiProvider } from "./providers/gemini.js";
import { OpenAIProvider } from "./providers/openai.js";
import { OpenCodeProvider } from "./providers/opencode.js";
import { OpenRouterProvider } from "./providers/openrouter.js";

export interface DefaultCoreOptions {
  openaiApiKey?: string;
  anthropicApiKey?: string;
  geminiApiKey?: string;
  geminiAccessToken?: string;
  geminiProjectId?: string;
  opencodeApiKey?: string;
  opencodeGoApiKey?: string;
  openrouterApiKey?: string;
  openrouterAppUrl?: string;
  openrouterAppName?: string;
  fetch?: FetchLike;
  apiCallLogger?: ProviderFetchLogger;
}

function env(name: string): string | undefined {
  const processLike = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process;
  return processLike?.env?.[name];
}

export function createDefaultCore(options: DefaultCoreOptions = {}): CallCore {
  const fetchOption = options.fetch ? { fetch: options.fetch } : {};
  const apiCallLoggerOption = options.apiCallLogger ? { apiCallLogger: options.apiCallLogger } : {};
  const openaiApiKey = options.openaiApiKey ?? env("OPENAI_API_KEY");
  const anthropicApiKey = options.anthropicApiKey ?? env("ANTHROPIC_API_KEY");
  const geminiApiKey = options.geminiApiKey ?? env("GEMINI_API_KEY");
  const opencodeApiKey = options.opencodeApiKey ?? env("OPENCODE_API_KEY");
  const opencodeGoApiKey = options.opencodeGoApiKey ?? opencodeApiKey;
  const openrouterApiKey = options.openrouterApiKey ?? env("OPENROUTER_API_KEY");
  const geminiAccessToken = options.geminiAccessToken;

  return new CallCore([
    new OpenAIProvider({ ...(openaiApiKey ? { apiKey: openaiApiKey } : {}), ...fetchOption, ...apiCallLoggerOption }),
    new AnthropicProvider({ ...(anthropicApiKey ? { apiKey: anthropicApiKey } : {}), ...fetchOption, ...apiCallLoggerOption }),
    new GeminiProvider({
      ...(geminiApiKey ? { apiKey: geminiApiKey } : {}),
      ...(geminiAccessToken ? { accessToken: geminiAccessToken } : {}),
      ...(options.geminiProjectId ? { projectId: options.geminiProjectId } : {}),
      ...fetchOption,
      ...apiCallLoggerOption,
    }),
    new OpenCodeProvider({ id: "opencode", ...(opencodeApiKey ? { apiKey: opencodeApiKey } : {}), ...fetchOption, ...apiCallLoggerOption }),
    new OpenCodeProvider({ id: "opencode-go", ...(opencodeGoApiKey ? { apiKey: opencodeGoApiKey } : {}), ...fetchOption, ...apiCallLoggerOption }),
    new OpenRouterProvider({
      ...(openrouterApiKey ? { apiKey: openrouterApiKey } : {}),
      ...(options.openrouterAppUrl ? { appUrl: options.openrouterAppUrl } : {}),
      ...(options.openrouterAppName ? { appName: options.openrouterAppName } : {}),
      ...fetchOption,
      ...apiCallLoggerOption,
    }),
  ]);
}
