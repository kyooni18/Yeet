import { AnthropicProvider } from "./anthropic.js";
import { GeminiProvider } from "./gemini.js";
import { OpenAIChatProvider } from "./openai-chat.js";
import { OpenAIProvider } from "./openai.js";
import type { ProviderFetchLogger } from "../http.js";
import type {
  CallResult,
  FetchLike,
  ModelInfo,
  ProviderAdapter,
  ProviderCallRequest,
  StreamEvent,
} from "../types.js";

const DEFAULT_CATALOG_URL = "https://models.dev/api.json";

type OpenCodeProtocol = "responses" | "chat" | "anthropic" | "google";

interface OpenCodeModelRoute {
  protocol: OpenCodeProtocol;
  contextLength?: number;
}

export interface OpenCodeProviderOptions {
  id?: "opencode" | "opencode-go";
  apiKey?: string;
  baseUrl?: string;
  catalogUrl?: string;
  fetch?: FetchLike;
  apiCallLogger?: ProviderFetchLogger;
}

function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined;
}

function positiveInteger(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isSafeInteger(value) && value > 0) return value;
  return undefined;
}

function protocolForPackage(value: unknown): OpenCodeProtocol {
  switch (value) {
    case "@ai-sdk/openai": return "responses";
    case "@ai-sdk/anthropic": return "anthropic";
    case "@ai-sdk/google": return "google";
    default: return "chat";
  }
}

function fallbackProtocol(provider: string, model: string): OpenCodeProtocol {
  if (model.startsWith("gemini-")) return "google";
  if (model.startsWith("claude-")) return "anthropic";
  if (model.startsWith("qwen")) return "anthropic";
  if (provider === "opencode-go" && model.startsWith("minimax-")) return "anthropic";
  if (/^(?:gpt-|grok-|muse-spark-)/.test(model)) return "responses";
  return "chat";
}

function withoutTrailingV1(baseUrl: string): string {
  return baseUrl.replace(/\/v1\/?$/, "");
}

export class OpenCodeProvider implements ProviderAdapter {
  readonly id: "opencode" | "opencode-go";
  readonly #fetch: FetchLike;
  readonly #catalogUrl: string;
  readonly #chat: OpenAIChatProvider;
  readonly #responses: OpenAIProvider;
  readonly #anthropic: AnthropicProvider;
  readonly #google: GeminiProvider;
  #routesPromise: Promise<Map<string, OpenCodeModelRoute>> | undefined;

  constructor(options: OpenCodeProviderOptions = {}) {
    this.id = options.id ?? "opencode";
    const baseUrl = (options.baseUrl ?? (
      this.id === "opencode-go"
        ? "https://opencode.ai/zen/go/v1"
        : "https://opencode.ai/zen/v1"
    )).replace(/\/$/, "");
    this.#fetch = options.fetch ?? globalThis.fetch.bind(globalThis);
    this.#catalogUrl = options.catalogUrl ?? DEFAULT_CATALOG_URL;

    const shared = {
      ...(options.apiKey ? { apiKey: options.apiKey } : {}),
      ...(options.fetch ? { fetch: options.fetch } : {}),
      ...(options.apiCallLogger ? { apiCallLogger: options.apiCallLogger } : {}),
    };
    this.#chat = new OpenAIChatProvider({
      id: this.id,
      baseUrl,
      requireApiKey: true,
      ...shared,
    });
    this.#responses = new OpenAIProvider({ id: this.id, baseUrl, ...shared });
    this.#anthropic = new AnthropicProvider({ id: this.id, baseUrl: withoutTrailingV1(baseUrl), ...shared });
    this.#google = new GeminiProvider({ id: this.id, baseUrl, ...shared });
  }

  async listModels(): Promise<string[]> {
    return (await this.listModelInfo()).map((model) => model.id);
  }

  async listModelInfo(): Promise<ModelInfo[]> {
    const discovered = await this.#chat.listModelInfo();
    let routes: Map<string, OpenCodeModelRoute> | undefined;
    try {
      routes = await this.#routes();
    } catch {
      // Model discovery is still useful when models.dev is temporarily unavailable.
    }
    return discovered.map((model) => {
      const contextLength = model.contextLength ?? routes?.get(model.id)?.contextLength;
      return contextLength !== undefined ? { ...model, contextLength } : model;
    });
  }

  async complete(request: ProviderCallRequest): Promise<CallResult> {
    return (await this.#adapter(request.model)).complete(request);
  }

  async *stream(request: ProviderCallRequest): AsyncIterable<StreamEvent> {
    yield* (await this.#adapter(request.model)).stream(request);
  }

  async #adapter(model: string): Promise<ProviderAdapter> {
    let protocol = fallbackProtocol(this.id, model);
    try {
      protocol = (await this.#routes()).get(model)?.protocol ?? protocol;
    } catch {
      // Keep current OpenCode families usable even if models.dev cannot be reached.
    }

    switch (protocol) {
      case "responses": return this.#responses;
      case "anthropic": return this.#anthropic;
      case "google": return this.#google;
      case "chat": return this.#chat;
    }
  }

  #routes(): Promise<Map<string, OpenCodeModelRoute>> {
    if (this.#routesPromise) return this.#routesPromise;
    const pending = this.#loadRoutes();
    this.#routesPromise = pending;
    pending.catch(() => {
      if (this.#routesPromise === pending) this.#routesPromise = undefined;
    });
    return pending;
  }

  async #loadRoutes(): Promise<Map<string, OpenCodeModelRoute>> {
    const response = await this.#fetch(this.#catalogUrl, {
      method: "GET",
      headers: { accept: "application/json" },
    });
    if (!response.ok) throw new Error(`OpenCode model metadata request failed with HTTP ${response.status}`);

    const root = record(await response.json()) ?? {};
    const provider = record(root[this.id]);
    const models = record(provider?.models);
    if (!provider || !models) return new Map();
    const providerPackage = provider.npm;
    const routes = new Map<string, OpenCodeModelRoute>();

    for (const [catalogModelId, value] of Object.entries(models)) {
      const model = record(value);
      if (!model) continue;
      const id = typeof model.id === "string" && model.id.trim() ? model.id.trim() : catalogModelId;
      const packageName = record(model.provider)?.npm ?? providerPackage;
      const contextLength = positiveInteger(record(model.limit)?.context);
      routes.set(id, {
        protocol: protocolForPackage(packageName),
        ...(contextLength !== undefined ? { contextLength } : {}),
      });
    }
    return routes;
  }
}
