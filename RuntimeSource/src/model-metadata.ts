import { parseModelId, type FetchLike, type ModelInfo, type ModelPricing } from "./types.js";

const DEFAULT_MODELS_DEV_URL = "https://models.dev/api.json";

const providerAliases: Readonly<Record<string, readonly string[]>> = {
  "codex-cli": ["openai"],
  gemini: ["google"],
  "gemini-web": ["gemini", "google"],
  claude: ["anthropic"],
};

interface CatalogModel {
  contextLength?: number;
  pricing?: ModelPricing;
  textGeneration?: boolean;
}

interface CatalogIndex {
  byProvider: Map<string, Map<string, CatalogModel>>;
  byModel: Map<string, CatalogModel[]>;
}

export interface ModelMetadataCatalogOptions {
  fetch?: FetchLike;
  url?: string;
  maxAgeMs?: number;
}

const DEFAULT_MAX_AGE_MS = 6 * 60 * 60 * 1_000;

function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined;
}

function positiveInteger(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isSafeInteger(value) && value > 0) return value;
  if (typeof value === "string" && /^\d+$/.test(value)) {
    const parsed = Number(value);
    if (Number.isSafeInteger(parsed) && parsed > 0) return parsed;
  }
  return undefined;
}

function nonNegativeNumber(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isFinite(value) && value >= 0) return value;
  if (typeof value === "string" && value.trim() && Number.isFinite(Number(value)) && Number(value) >= 0) {
    return Number(value);
  }
  return undefined;
}

function parseRates(value: unknown): Pick<ModelPricing, "input" | "output" | "cacheRead" | "cacheWrite"> | undefined {
  const cost = record(value);
  const input = nonNegativeNumber(cost?.input);
  const output = nonNegativeNumber(cost?.output);
  if (input === undefined || output === undefined) return undefined;
  const cacheRead = nonNegativeNumber(cost?.cache_read ?? cost?.cacheRead);
  const cacheWrite = nonNegativeNumber(cost?.cache_write ?? cost?.cacheWrite);
  return {
    input,
    output,
    ...(cacheRead !== undefined ? { cacheRead } : {}),
    ...(cacheWrite !== undefined ? { cacheWrite } : {}),
  };
}

function modelPricing(model: Record<string, unknown>): ModelPricing | undefined {
  const rates = parseRates(model.cost);
  if (!rates) return undefined;
  const cost = record(model.cost)!;
  const tiers = Array.isArray(cost.tiers)
    ? cost.tiers.flatMap((value) => {
      const tier = record(value);
      const descriptor = record(tier?.tier);
      const thresholdTokens = descriptor?.type === "context" ? positiveInteger(descriptor.size) : undefined;
      const tierRates = parseRates(tier);
      return thresholdTokens !== undefined && tierRates
        ? [{ ...tierRates, thresholdTokens }]
        : [];
    }).sort((a, b) => a.thresholdTokens - b.thresholdTokens)
    : undefined;
  const modesValue = record(record(model.experimental)?.modes);
  const modes = modesValue
    ? Object.fromEntries(Object.entries(modesValue).flatMap(([name, value]) => {
      const rates = parseRates(record(value)?.cost);
      return rates ? [[name, rates]] : [];
    }))
    : undefined;
  return {
    ...rates,
    currency: "USD",
    unit: "per1MTokens",
    source: "models.dev",
    ...(tiers?.length ? { tiers } : {}),
    ...(modes && Object.keys(modes).length ? { modes } : {}),
  };
}

function normalizedModelId(value: string): string {
  const trimmed = value.trim();
  return trimmed.startsWith("models/") ? trimmed.slice("models/".length) : trimmed;
}

function buildIndex(raw: unknown): CatalogIndex {
  const root = record(raw) ?? {};
  const byProvider = new Map<string, Map<string, CatalogModel>>();
  const byModel = new Map<string, CatalogModel[]>();

  for (const [catalogProviderId, providerValue] of Object.entries(root)) {
    const provider = record(providerValue);
    const models = record(provider?.models);
    if (!provider || !models) continue;

    const providerIds = new Set([catalogProviderId]);
    if (typeof provider.id === "string" && provider.id.trim()) providerIds.add(provider.id.trim());
    const providerModels = new Map<string, CatalogModel>();

    for (const [catalogModelId, modelValue] of Object.entries(models)) {
      const model = record(modelValue);
      if (!model) continue;
      const id = normalizedModelId(typeof model.id === "string" ? model.id : catalogModelId);
      if (!id) continue;
      const limit = record(model.limit);
      const contextLength = positiveInteger(limit?.context);
      const pricing = modelPricing(model);
      const modalities = record(model.modalities);
      const inputs = Array.isArray(modalities?.input) ? modalities.input : [];
      const outputs = Array.isArray(modalities?.output) ? modalities.output : [];
      const textGeneration = inputs.includes("text") && outputs.includes("text");
      if (contextLength === undefined && pricing === undefined) continue;
      const metadata: CatalogModel = {
        ...(contextLength !== undefined ? { contextLength } : {}),
        ...(pricing !== undefined ? { pricing } : {}),
        ...(inputs.length || outputs.length ? { textGeneration } : {}),
      };
      providerModels.set(id, metadata);
      const matches = byModel.get(id) ?? [];
      matches.push(metadata);
      byModel.set(id, matches);
    }

    for (const providerId of providerIds) byProvider.set(providerId, providerModels);
  }

  return { byProvider, byModel };
}

/**
 * Lazy, process-local lookup for model context windows from models.dev.
 *
 * Provider-specific matches are preferred. For arbitrary/custom provider ids,
 * an exact model-id match is accepted only when every catalog entry for that
 * model agrees on the same context length.
 */
export class ModelMetadataCatalog {
  readonly #fetch: FetchLike;
  readonly #url: string;
  readonly #maxAgeMs: number;
  #indexPromise: Promise<CatalogIndex> | undefined;
  #loadedAt = 0;

  constructor(options: ModelMetadataCatalogOptions = {}) {
    this.#fetch = options.fetch ?? globalThis.fetch.bind(globalThis);
    this.#url = options.url ?? DEFAULT_MODELS_DEV_URL;
    this.#maxAgeMs = options.maxAgeMs ?? DEFAULT_MAX_AGE_MS;
  }

  async contextLength(fullModelId: string): Promise<number | undefined> {
    return (await this.modelInfo(fullModelId))?.contextLength;
  }

  async pricing(fullModelId: string): Promise<ModelPricing | undefined> {
    return (await this.modelInfo(fullModelId))?.pricing;
  }

  async supportsTextGeneration(fullModelId: string): Promise<boolean | undefined> {
    return (await this.modelInfo(fullModelId))?.textGeneration;
  }

  async enrich(provider: string, info: ModelInfo): Promise<ModelInfo> {
    const catalog = await this.modelInfo(`${provider}/${info.id}`);
    if (!catalog) return info;
    return {
      ...catalog,
      ...info,
      ...(info.contextLength === undefined && catalog.contextLength !== undefined
        ? { contextLength: catalog.contextLength }
        : {}),
      ...(info.pricing === undefined && catalog.pricing !== undefined
        ? { pricing: catalog.pricing }
        : {}),
    };
  }

  async modelInfo(fullModelId: string): Promise<CatalogModel | undefined> {
    const parsed = parseModelId(fullModelId);
    const model = normalizedModelId(parsed.model);
    const index = await this.#index();

    const providerIds = [parsed.provider, ...(providerAliases[parsed.provider] ?? [])];
    for (const providerId of providerIds) {
      const direct = index.byProvider.get(providerId)?.get(model);
      if (direct !== undefined) return direct;
    }

    const globalMatches = index.byModel.get(model);
    if (!globalMatches?.length) return undefined;
    const contexts = new Set(globalMatches.flatMap((entry) => entry.contextLength === undefined ? [] : [entry.contextLength]));
    const contextLength = contexts.size === 1 ? contexts.values().next().value : undefined;
    // A custom/OpenAI-compatible endpoint can expose a familiar model id at a
    // completely different price (including zero for local inference). Global
    // model-id fallback is therefore safe for context only, never billing.
    return contextLength === undefined ? undefined : { contextLength };
  }

  #index(): Promise<CatalogIndex> {
    if (this.#indexPromise && Date.now() - this.#loadedAt < this.#maxAgeMs) return this.#indexPromise;
    const pending = this.#loadIndex();
    this.#indexPromise = pending;
    pending.then(() => {
      if (this.#indexPromise === pending) this.#loadedAt = Date.now();
    });
    pending.catch(() => {
      if (this.#indexPromise === pending) {
        this.#indexPromise = undefined;
        this.#loadedAt = 0;
      }
    });
    return pending;
  }

  async #loadIndex(): Promise<CatalogIndex> {
    const response = await this.#fetch(this.#url, {
      method: "GET",
      headers: { accept: "application/json" },
    });
    if (!response.ok) throw new Error(`Model metadata request failed with HTTP ${response.status}`);
    return buildIndex(await response.json());
  }
}
