// Model-pricing helpers shared by routing, cache policy, and usage telemetry.

import { parseModelId } from "./types.js";
import type { CallRequest, ModelCostRates, ModelInfo, ModelPricing, Usage } from "./types.js";

const TOKENS_PER_MILLION = 1_000_000;

export function effectiveRates(pricing: ModelPricing, inputTokens: number, mode?: string): ModelCostRates {
  if (mode && pricing.modes?.[mode]) return pricing.modes[mode]!;
  let rates: ModelCostRates = pricing;
  for (const tier of pricing.tiers ?? []) {
    if (inputTokens > tier.thresholdTokens) rates = tier;
  }
  return rates;
}

export function requestPricingMode(request: CallRequest): string | undefined {
  const tier = request.providerOptions?.service_tier;
  if (tier === "priority") return "fast";
  return undefined;
}

export function estimateUsageCostUsd(pricing: ModelPricing, usage: Usage, request?: CallRequest): number | undefined {
  const input = usage.inputTokens;
  const output = usage.outputTokens;
  if (input === undefined && output === undefined) return undefined;
  const rates = effectiveRates(pricing, input ?? 0, request ? requestPricingMode(request) : undefined);
  const cached = Math.min(input ?? 0, usage.cachedInputTokens ?? 0);
  const cacheWrite = Math.min(Math.max(0, (input ?? 0) - cached), usage.cacheWriteInputTokens ?? 0);
  const ordinary = Math.max(0, (input ?? 0) - cached - cacheWrite);
  const serviceTier = request?.providerOptions?.service_tier;
  const multiplier = serviceTier === "flex" ? 0.5 : 1;
  const cacheWriteRate = request ? requestCacheWriteRate(request, rates) : (rates.cacheWrite ?? rates.input);
  const inputCost = ordinary * rates.input
    + cached * (rates.cacheRead ?? rates.input)
    + cacheWrite * cacheWriteRate;
  return multiplier * (inputCost + (output ?? 0) * rates.output) / TOKENS_PER_MILLION;
}

function requestCacheWriteRate(request: CallRequest, rates: ModelCostRates): number {
  const { provider } = parseModelId(request.model);
  const requestedCacheControl = request.providerOptions?.cache_control;
  const ttl = requestedCacheControl && typeof requestedCacheControl === "object" && !Array.isArray(requestedCacheControl)
    ? (requestedCacheControl as Record<string, unknown>).ttl
    : undefined;
  // Anthropic's 1-hour cache write is 2x ordinary input. Cache reads keep the
  // model's normal cache-read rate. The default 5-minute write continues to
  // use live model pricing (currently 1.25x on supported Claude models).
  if ((provider === "anthropic" || provider === "claude") && ttl === "1h") return rates.input * 2;
  return rates.cacheWrite ?? rates.input;
}

export function inputCostEquivalentTokens(
  pricing: ModelPricing,
  usage: Usage,
  request: CallRequest,
): number | undefined {
  const input = usage.inputTokens;
  if (input === undefined) return undefined;
  const rates = effectiveRates(pricing, input, requestPricingMode(request));
  if (!(rates.input > 0)) return input;
  const cached = Math.min(input, usage.cachedInputTokens ?? 0);
  const cacheWrite = Math.min(Math.max(0, input - cached), usage.cacheWriteInputTokens ?? 0);
  const ordinary = Math.max(0, input - cached - cacheWrite);
  const cacheReadRate = rates.cacheRead ?? rates.input;
  const cacheWriteRate = requestCacheWriteRate(request, rates);
  return Math.max(0, Math.round(
    ordinary
      + cached * cacheReadRate / rates.input
      + cacheWrite * cacheWriteRate / rates.input,
  ));
}

export function cacheBreakEvenReuses(
  pricing: ModelPricing,
  inputTokens = 0,
  request?: CallRequest,
): number | undefined {
  const rates = effectiveRates(pricing, inputTokens);
  if (rates.cacheRead === undefined || rates.cacheWrite === undefined) return undefined;
  if (rates.cacheRead >= rates.input) return undefined;
  const cacheWriteRate = request ? requestCacheWriteRate(request, rates) : rates.cacheWrite;
  const premium = Math.max(0, cacheWriteRate - rates.input);
  if (premium === 0) return 0;
  return Math.ceil(premium / (rates.input - rates.cacheRead));
}

export function applyCacheCostPolicy(request: CallRequest, pricing: ModelPricing | undefined): CallRequest {
  if (!pricing || request.promptCache !== true) return request;
  const purpose = request.metadata?.purpose;
  if (purpose === "session-title" || purpose === "context-compaction") {
    return { ...request, promptCache: false };
  }
  const expected = Number(request.metadata?.expectedCacheReuses ?? "0");
  const estimatedInput = estimatedRequestTokens(request);
  const breakEven = cacheBreakEvenReuses(pricing, estimatedInput, request);
  if (breakEven !== undefined && expected < breakEven) return { ...request, promptCache: false };
  return request;
}

export function applyOpenAIFlexAuthPolicy(request: CallRequest, authMethod: string | undefined): CallRequest {
  if (parseModelId(request.model).provider !== "openai" || request.providerOptions?.service_tier !== "flex") {
    return request;
  }
  if (authMethod === "api-key" || authMethod === "environment") return request;
  const { service_tier: _serviceTier, ...providerOptions } = request.providerOptions;
  return {
    ...request,
    providerOptions,
    metadata: {
      ...(request.metadata ?? {}),
      flexSuppressed: authMethod === "browser" ? "browser-auth" : "non-api-key-auth",
    },
  };
}

export function estimatedRequestTokens(request: CallRequest): number {
  const messageChars = JSON.stringify(request.messages).length;
  const toolChars = request.tools ? JSON.stringify(request.tools).length : 0;
  return Math.max(1, Math.ceil((messageChars + toolChars) / 3));
}

export function estimatedRequestCostUsd(pricing: ModelPricing, request: CallRequest): number {
  const inputTokens = estimatedRequestTokens(request);
  const outputTokens = request.maxTokens ?? 1_024;
  const rates = effectiveRates(pricing, inputTokens, requestPricingMode(request));
  const multiplier = request.providerOptions?.service_tier === "flex" ? 0.5 : 1;
  return multiplier * (inputTokens * rates.input + outputTokens * rates.output) / TOKENS_PER_MILLION;
}

export function cheapestModel(
  models: ModelInfo[],
  request: CallRequest,
  minimumContextTokens: number,
): ModelInfo | undefined {
  return models
    .filter((model) => model.pricing && (model.contextLength ?? 0) >= minimumContextTokens)
    .map((model) => ({ model, cost: estimatedRequestCostUsd(model.pricing!, request) }))
    .sort((a, b) => a.cost - b.cost || a.model.id.localeCompare(b.model.id))[0]?.model;
}
