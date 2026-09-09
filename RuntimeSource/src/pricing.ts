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
  const inputCost = ordinary * rates.input
    + cached * (rates.cacheRead ?? rates.input)
    + cacheWrite * (rates.cacheWrite ?? rates.input);
  return multiplier * (inputCost + (output ?? 0) * rates.output) / TOKENS_PER_MILLION;
}

export function cacheBreakEvenReuses(pricing: ModelPricing, inputTokens = 0): number | undefined {
  const rates = effectiveRates(pricing, inputTokens);
  if (rates.cacheRead === undefined || rates.cacheWrite === undefined) return undefined;
  if (rates.cacheRead >= rates.input) return undefined;
  const premium = Math.max(0, rates.cacheWrite - rates.input);
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
  const breakEven = cacheBreakEvenReuses(pricing, estimatedInput);
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
