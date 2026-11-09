import type { ProviderFetchLogger } from "../http.js";
import type { FetchLike, ModelInfo } from "../types.js";
import { OpenAIChatProvider } from "./openai-chat.js";

/** Models documented at https://docs.exaone.ai (the API exposes no /models listing). */
export const EXAONE_MODELS = ["k-exaone_v2.5"] as const;

export interface ExaoneProviderOptions {
  apiKey?: string;
  baseUrl?: string;
  fetch?: FetchLike;
  apiCallLogger?: ProviderFetchLogger;
}

/** LG AI Research EXAONE API Platform: OpenAI-compatible chat completions. */
export class ExaoneProvider extends OpenAIChatProvider {
  constructor(options: ExaoneProviderOptions = {}) {
    super({
      id: "exaone",
      baseUrl: options.baseUrl ?? "https://api.exaone.ai/v1",
      requireApiKey: true,
      ...(options.apiKey ? { apiKey: options.apiKey } : {}),
      ...(options.fetch ? { fetch: options.fetch } : {}),
      ...(options.apiCallLogger ? { apiCallLogger: options.apiCallLogger } : {}),
    });
  }

  override async listModelInfo(): Promise<ModelInfo[]> {
    return EXAONE_MODELS.map((id) => ({ id, contextLength: 524_288 }));
  }
}
