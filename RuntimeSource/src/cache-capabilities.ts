export type PromptCacheMode = "none" | "implicit" | "explicit" | "hybrid" | "resource";

export interface PromptCacheCapabilities {
  provider: string;
  model: string;
  modes: PromptCacheMode[];
  /** Conservative number of explicit lookup boundaries Yeet should emit. */
  maxLookupBreakpoints?: number;
  /** Maximum explicit cache markers Yeet should serialize for this route. */
  maxExplicitBreakpoints?: number;
  /** Maximum new prompt-cache writes one request may create. */
  maxNewWritesPerRequest?: number;
  /** Provider-documented minimum cacheable prefix/input size when known. */
  minCacheablePrefixTokens?: number;
  ttlValues?: string[];
  defaultTtl?: string;
  cacheWriteMultiplier?: number;
  cacheReadMultiplier?: number;
  sessionAffinity?: boolean;
}

function gptVersionAtLeast(model: string, major: number, minor: number): boolean {
  const match = /^gpt-(\d+)(?:\.(\d+))?/.exec(model);
  if (!match) return false;
  const currentMajor = Number(match[1]);
  const currentMinor = Number(match[2] ?? 0);
  return currentMajor > major || (currentMajor === major && currentMinor >= minor);
}

function anthropicMinimumCacheTokens(model: string): number | undefined {
  if (/^claude-(?:fable|mythos)-5(?:\.1)?(?:-|$)/.test(model) || /^claude-opus-5(?:-|$)/.test(model)) return 512;
  if (/^claude-mythos-preview/.test(model) || /^claude-opus-4[-.]7(?:-|$)/.test(model)) return 2_048;
  if (/^claude-opus-4[-.](?:5|6)(?:-|$)/.test(model)) return 4_096;
  if (
    /^claude-opus-4[-.]8(?:-|$)/.test(model)
    || /^claude-sonnet-5(?:-|$)/.test(model)
    || /^claude-sonnet-4[-.](?:5|6)(?:-|$)/.test(model)
    || /^claude-opus-4(?:[-.]1)?(?:-|$)/.test(model)
    || /^claude-sonnet-4(?:-|$)/.test(model)
  ) return 1_024;
  if (/^claude-haiku-4[-.]5(?:-|$)/.test(model)) return 4_096;
  if (/^claude-haiku-3[-.]5(?:-|$)/.test(model)) return 2_048;
  return undefined;
}

function geminiMinimumCacheTokens(model: string): number | undefined {
  if (/^gemini-(?:3\.8|3\.7|3\.6|3\.5)-flash/.test(model) || /^gemini-3\.1-pro-preview/.test(model)) {
    return 4_096;
  }
  if (/^gemini-2\.5-(?:flash|pro)/.test(model)) return 2_048;
  return undefined;
}

/**
 * Central provider/model cache contract used by adapters and diagnostics.
 * Values here are intentionally conservative when current public provider
 * references disagree. Keep adapter behavior dependent on this table rather
 * than scattering model-name/limit assumptions throughout serialization.
 */
export function promptCacheCapabilities(provider: string, model: string): PromptCacheCapabilities {
  if (provider === "openai") {
    if (gptVersionAtLeast(model, 5, 6)) {
      return {
        provider,
        model,
        modes: ["implicit", "explicit"],
        // GPT-5.6+ currently searches the latest 80 explicit breakpoints while
        // limiting each request to four new writes. Keep lookup capacity distinct
        // from write capacity so long append-only agent turns can still hit an
        // older prefix without paying for extra cache writes.
        maxLookupBreakpoints: 80,
        maxExplicitBreakpoints: 80,
        maxNewWritesPerRequest: 4,
        ttlValues: ["30m"],
        defaultTtl: "30m",
        cacheWriteMultiplier: 1.25,
        cacheReadMultiplier: 0.1,
      };
    }
    return { provider, model, modes: ["implicit"] };
  }

  if (provider === "anthropic" || provider === "claude") {
    const minimum = anthropicMinimumCacheTokens(model);
    const cacheReadMultiplier = /^claude-(?:fable|mythos)-5\.1(?:-|$)/.test(model) ? 0.025 : 0.1;
    return {
      provider,
      model,
      modes: ["implicit", "explicit", "hybrid"],
      maxExplicitBreakpoints: 4,
      maxNewWritesPerRequest: 4,
      ...(minimum !== undefined ? { minCacheablePrefixTokens: minimum } : {}),
      ttlValues: ["5m", "1h"],
      defaultTtl: "5m",
      cacheWriteMultiplier: 1.25,
      cacheReadMultiplier,
    };
  }

  if (provider === "gemini" || provider === "gemini-web") {
    const minimum = geminiMinimumCacheTokens(model);
    return {
      provider,
      model,
      modes: ["implicit", "resource"],
      ...(minimum !== undefined ? { minCacheablePrefixTokens: minimum } : {}),
    };
  }

  if (provider === "openrouter") {
    return {
      provider,
      model,
      modes: ["implicit", "explicit"],
      // Four is the tightest common explicit-marker limit among the upstream
      // routes Yeet currently targets. Providers that use only the last
      // marker safely ignore the earlier candidates.
      maxExplicitBreakpoints: 4,
      sessionAffinity: true,
    };
  }

  return { provider, model, modes: ["none"] };
}

export function supportsExplicitPromptCache(provider: string, model: string): boolean {
  return promptCacheCapabilities(provider, model).modes.includes("explicit");
}

/** Public Responses hosted tool search. Keep known 5.4 mini/nano exceptions
 * out of the path rather than inferring support from the version alone. */
export function supportsOpenAIHostedToolSearch(model: string): boolean {
  const match = /^gpt-(\d+)(?:\.(\d+))?/.exec(model);
  if (!match) return false;
  const major = Number(match[1]);
  const minor = Number(match[2] ?? 0);
  if (major > 5 || (major === 5 && minor >= 5)) return true;
  if (major !== 5 || minor !== 4) return false;
  return !model.startsWith("gpt-5.4-mini") && !model.startsWith("gpt-5.4-nano");
}

/** Anthropic deferred tool references are documented for Claude 4.5+ and
 * the current Fable/Mythos 5 families. Keep older 4.1-and-earlier models on
 * Yeet's provider-neutral search/load fallback. */
export function supportsAnthropicDeferredToolReferences(model: string): boolean {
  if (/^claude-(?:fable|mythos|opus)-5(?:-|$)/.test(model)) return true;
  const match = /^claude-(opus|sonnet|haiku)-4(?:[-.](\d+))/.exec(model);
  if (!match) return false;
  const minor = Number(match[2] ?? 0);
  return minor >= 5;
}
