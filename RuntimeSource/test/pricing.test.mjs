import assert from "node:assert/strict";
import test from "node:test";

import {
  applyCacheCostPolicy,
  applyOpenAIFlexAuthPolicy,
  cacheBreakEvenReuses,
  cheapestModel,
  effectiveRates,
  estimateUsageCostUsd,
} from "../dist/pricing.js";

const astra = {
  input: 10,
  output: 50,
  cacheRead: 1,
  cacheWrite: 12.5,
  currency: "USD",
  unit: "per1MTokens",
  source: "test",
  tiers: [{ input: 20, output: 75, cacheRead: 2, cacheWrite: 25, thresholdTokens: 272_000 }],
};

test("cost estimator applies cache components and long-context rates to the whole request", () => {
  assert.equal(cacheBreakEvenReuses(astra), 1);
  assert.equal(effectiveRates(astra, 272_000).input, 10);
  assert.equal(effectiveRates(astra, 272_001).input, 20);

  const normal = estimateUsageCostUsd(astra, {
    inputTokens: 100_000,
    cachedInputTokens: 60_000,
    cacheWriteInputTokens: 20_000,
    outputTokens: 10_000,
  });
  assert.equal(normal, 0.2 + 0.06 + 0.25 + 0.5);

  const long = estimateUsageCostUsd(astra, { inputTokens: 300_000, outputTokens: 10_000 });
  assert.equal(long, 6 + 0.75);
});

test("Flex halves estimated OpenAI inference cost", () => {
  const request = {
    model: "openai/gpt-6-astra",
    messages: [{ role: "user", content: "hello" }],
    providerOptions: { service_tier: "flex" },
  };
  assert.equal(
    estimateUsageCostUsd(astra, { inputTokens: 100_000, outputTokens: 10_000 }, request),
    0.75,
  );
});

test("OpenAI Flex is kept for API billing and suppressed for browser login", () => {
  const request = {
    model: "openai/gpt-6-astra",
    messages: [{ role: "user", content: "hello" }],
    providerOptions: { service_tier: "flex", reasoning: { effort: "low" } },
  };
  assert.equal(applyOpenAIFlexAuthPolicy(request, "api-key").providerOptions.service_tier, "flex");
  assert.equal(applyOpenAIFlexAuthPolicy(request, "environment").providerOptions.service_tier, "flex");
  const browser = applyOpenAIFlexAuthPolicy(request, "browser");
  assert.equal(browser.providerOptions.service_tier, undefined);
  assert.deepEqual(browser.providerOptions.reasoning, { effort: "low" });
  assert.equal(browser.metadata.flexSuppressed, "browser-auth");
});

test("cache policy pays a cache-write premium only when reuse can amortize it", () => {
  const base = {
    model: "openai/gpt-6-astra",
    messages: [{ role: "user", content: "hello" }],
    promptCache: true,
    metadata: { purpose: "general", expectedCacheReuses: "0" },
  };
  assert.equal(applyCacheCostPolicy(base, astra).promptCache, false);
  assert.equal(
    applyCacheCostPolicy({ ...base, metadata: { purpose: "lead", expectedCacheReuses: "1" } }, astra).promptCache,
    true,
  );
});

test("auxiliary routing chooses the lowest estimated price that fits the request", () => {
  const request = {
    model: "openai/gpt-6-astra",
    messages: [{ role: "user", content: "summarize" }],
    maxTokens: 500,
  };
  const luna = {
    ...astra,
    input: 0.2,
    output: 1.2,
    cacheRead: 0.02,
    cacheWrite: 0.25,
    tiers: undefined,
  };
  const picked = cheapestModel([
    { id: "gpt-6-astra", contextLength: 1_050_000, pricing: astra },
    { id: "gpt-5.6-luna", contextLength: 1_050_000, pricing: luna },
    { id: "tiny", contextLength: 2, pricing: { ...luna, input: 0, output: 0 } },
  ], request, 1_000);
  assert.equal(picked.id, "gpt-5.6-luna");
});
