import type { ProviderFetchLogger } from "../http.js";
import type { FetchLike, ModelInfo } from "../types.js";
import { OpenAIChatProvider } from "./openai-chat.js";

/** Models documented at https://docs.exaone.ai (the API exposes no /models listing). */
export const EXAONE_MODELS = ["k-exaone_v2.5"] as const;

export interface ExaoneRateLimitWindow {
  window: "second" | "minute" | "day";
  limit: number;
  remaining: number;
}

export interface ExaoneRateLimitSnapshot {
  observedAt: string;
  windows: ExaoneRateLimitWindow[];
}

const RATE_LIMIT_WINDOWS = ["second", "minute", "day"] as const;
let latestRateLimits: ExaoneRateLimitSnapshot | undefined;

/** Parse EXAONE `x-{second,minute,day}-rate-limit[-remaining]` response headers. */
export function parseExaoneRateLimits(headers: Headers, observedAt = new Date()): ExaoneRateLimitSnapshot | undefined {
  const windows = RATE_LIMIT_WINDOWS.flatMap((window): ExaoneRateLimitWindow[] => {
    const limit = Number(headers.get(`x-${window}-rate-limit`));
    const remaining = Number(headers.get(`x-${window}-rate-limit-remaining`));
    if (!headers.has(`x-${window}-rate-limit`) || !Number.isFinite(limit) || limit <= 0 || !Number.isFinite(remaining)) return [];
    return [{ window, limit, remaining: Math.max(0, Math.min(limit, remaining)) }];
  });
  return windows.length ? { observedAt: observedAt.toISOString(), windows } : undefined;
}

/** Most recent rate-limit headers seen on any EXAONE response in this process. */
export function latestExaoneRateLimits(): ExaoneRateLimitSnapshot | undefined {
  return latestRateLimits;
}

export interface ExaoneProviderOptions {
  apiKey?: string;
  baseUrl?: string;
  fetch?: FetchLike;
  apiCallLogger?: ProviderFetchLogger;
}

/** LG AI Research EXAONE API Platform: OpenAI-compatible chat completions. */
export class ExaoneProvider extends OpenAIChatProvider {
  constructor(options: ExaoneProviderOptions = {}) {
    const baseFetch: FetchLike = options.fetch ?? ((input, init) => fetch(input, init));
    const recordingFetch: FetchLike = async (input, init) => {
      const response = await baseFetch(input, init);
      const snapshot = parseExaoneRateLimits(response.headers);
      if (snapshot) latestRateLimits = snapshot;
      return response;
    };
    super({
      id: "exaone",
      baseUrl: options.baseUrl ?? "https://api.exaone.ai/v1",
      requireApiKey: true,
      ...(options.apiKey ? { apiKey: options.apiKey } : {}),
      fetch: recordingFetch,
      ...(options.apiCallLogger ? { apiCallLogger: options.apiCallLogger } : {}),
    });
  }

  override async listModelInfo(): Promise<ModelInfo[]> {
    return EXAONE_MODELS.map((id) => ({ id, contextLength: 524_288 }));
  }
}
