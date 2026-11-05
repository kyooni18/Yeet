import type { ProviderFetchLogger } from "../http.js";
import type { FetchLike } from "../types.js";
import { OpenAIChatProvider } from "./openai-chat.js";

/** Mistral's hosted API, also used by Mistral Vibe with a Vibe API key. */
export class MistralProvider extends OpenAIChatProvider {
  constructor(options: { apiKey?: string; fetch?: FetchLike; apiCallLogger?: ProviderFetchLogger } = {}) {
    super({ id: "mistral", baseUrl: "https://api.mistral.ai/v1", requireApiKey: true, ...options });
  }
}
