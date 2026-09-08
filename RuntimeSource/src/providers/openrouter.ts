import type { ProviderFetchLogger } from "../http.js";
import type { FetchLike } from "../types.js";
import { OpenAIChatProvider } from "./openai-chat.js";

export interface OpenRouterProviderOptions {
  apiKey?: string;
  appUrl?: string;
  appName?: string;
  fetch?: FetchLike;
  apiCallLogger?: ProviderFetchLogger;
}

export class OpenRouterProvider extends OpenAIChatProvider {
  constructor(options: OpenRouterProviderOptions = {}) {
    const headers: Record<string, string> = {};
    if (options.appUrl) headers["HTTP-Referer"] = options.appUrl;
    if (options.appName) headers["X-Title"] = options.appName;
    super({
      id: "openrouter",
      baseUrl: "https://openrouter.ai/api/v1",
      headers,
      requireApiKey: true,
      ...(options.apiKey ? { apiKey: options.apiKey } : {}),
      ...(options.fetch ? { fetch: options.fetch } : {}),
      ...(options.apiCallLogger ? { apiCallLogger: options.apiCallLogger } : {}),
    });
  }
}
