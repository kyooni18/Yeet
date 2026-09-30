import type { CallRequest } from "./types.js";
import { parseModelId } from "./types.js";

type ReasoningLevel = "low" | "medium" | "high" | "xhigh" | "max";

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function selectedReasoningLevel(request: CallRequest): ReasoningLevel | undefined {
  const value = request.metadata?.reasoningLevel?.trim().toLowerCase();
  return (["low", "medium", "high", "xhigh", "max"] as const).find((level) => level === value);
}

function isAuxiliary(request: CallRequest): boolean {
  return request.metadata?.purpose === "session-title" || request.metadata?.purpose === "context-compaction";
}

function gptVersion(model: string): { major: number; minor: number } | undefined {
  const match = /^gpt-(\d+)(?:\.(\d+))?/.exec(model);
  if (!match) return undefined;
  return { major: Number(match[1]), minor: Number(match[2] ?? 0) };
}

function openAIAuxiliaryEffort(model: string): "minimal" | "none" | "low" {
  const version = gptVersion(model);
  if (!version) return "minimal";
  if (version.major > 5) return "low";
  if (version.major === 5 && version.minor >= 6) return "none";
  return "minimal";
}

function normalizeOpenAIEffort(model: string, requested: ReasoningLevel): ReasoningLevel {
  const version = gptVersion(model);
  if ((requested === "xhigh" || requested === "max")
      && !(version && (version.major > 5 || (version.major === 5 && version.minor >= 6)))) {
    return "high";
  }
  return requested;
}

function anthropicEffort(model: string, requested: ReasoningLevel): ReasoningLevel | undefined {
  const modern = /^claude-(?:fable|mythos|opus)-5(?:\.|-|$)/.test(model)
    || /^claude-sonnet-5(?:\.|-|$)/.test(model)
    || /^claude-opus-4[.-](?:7|8)(?:-|$)/.test(model)
    || /^claude-(?:sonnet|opus)-4[.-]6(?:-|$)/.test(model);
  if (!modern) return undefined;
  if ((requested === "xhigh" || requested === "max")
      && /^claude-(?:sonnet|opus)-4[.-]6(?:-|$)/.test(model)) return "high";
  return requested;
}

function geminiThinkingConfig(model: string, requested: ReasoningLevel): Record<string, unknown> | undefined {
  if (/^gemini-3(?:\.|-|$)/.test(model)) {
    let thinkingLevel: "low" | "medium" | "high" = requested === "xhigh" || requested === "max"
      ? "high"
      : requested;
    if (/^gemini-3-pro-preview(?:-|$)/.test(model) && thinkingLevel === "medium") thinkingLevel = "high";
    if (/^gemini-3\.1-flash-lite-image(?:-|$)/.test(model) && thinkingLevel !== "high") thinkingLevel = "high";
    return { thinkingLevel };
  }
  if (/^gemini-2\.5-(?:pro|flash|flash-lite)(?:-|$)/.test(model)) {
    const thinkingBudget = requested === "low" ? 1_024 : requested === "medium" ? 8_192 : 24_576;
    return { thinkingBudget };
  }
  return undefined;
}

/** Apply one provider-aware reasoning policy without overriding explicit raw provider choices. */
export function withReasoningPolicy(request: CallRequest): CallRequest {
  const parsed = parseModelId(request.model);
  const requested = selectedReasoningLevel(request);
  const auxiliary = isAuxiliary(request);
  const existing = request.providerOptions ?? {};
  const openCode = parsed.provider === "opencode" || parsed.provider === "opencode-go";
  const routedModel = parsed.model.includes("/") ? parsed.model.slice(parsed.model.indexOf("/") + 1) : parsed.model;
  const anthropicRoute = parsed.provider === "anthropic"
    || parsed.provider === "claude"
    || parsed.provider === "claude-api"
    || (openCode && routedModel.startsWith("claude-"));
  const geminiRoute = parsed.provider === "gemini"
    || parsed.provider === "gemini-web"
    || (openCode && routedModel.startsWith("gemini-"));

  if (["openai", "codex-cli", "opencode", "opencode-go"].includes(parsed.provider)
      && /^(?:gpt-|o\d|muse-spark-|grok-)/.test(routedModel)) {
    if (existing.reasoning !== undefined) {
      const reasoning = record(existing.reasoning);
      return reasoning.summary !== undefined ? request : {
        ...request, providerOptions: { ...existing, reasoning: { ...reasoning, summary: "auto" } },
      };
    }
    const effort = requested
      ? normalizeOpenAIEffort(routedModel, requested)
      : auxiliary
        ? openAIAuxiliaryEffort(routedModel)
        : "low";
    return {
      ...request,
      providerOptions: { ...existing, reasoning: { effort, summary: "auto" } },
    };
  }

  if (anthropicRoute) {
    const outputConfig = record(existing.output_config);
    const effort = anthropicEffort(routedModel, requested ?? "low");
    const adaptive = effort !== undefined || /^claude-mythos-preview(?:-|$)/.test(routedModel);
    const manual = /^claude-(?:sonnet|opus|haiku)-4(?:[.-](?:1|5))?(?:-|$)/.test(routedModel)
      || /^claude-3[.-]7-sonnet(?:-|$)/.test(routedModel);
    let thinking = existing.thinking;
    if (!auxiliary && thinking === undefined) {
      if (adaptive) thinking = { type: "adaptive", display: "summarized" };
      else if (manual && request.toolChoice !== "required" && typeof request.toolChoice !== "object") {
        const maximum = request.maxTokens ?? 4_096;
        const desired = requested === "low" ? 1_024 : requested === "medium" ? 8_192 : requested ? 24_576 : 1_024;
        const budget = Math.min(desired, Math.floor(maximum / 2));
        if (budget >= 1_024) thinking = { type: "enabled", budget_tokens: budget };
      }
    } else if (!auxiliary && ["adaptive", "enabled"].includes(String(record(thinking).type))
        && record(thinking).display === undefined && adaptive) {
      thinking = { ...record(thinking), display: "summarized" };
    }
    if (thinking === existing.thinking && (outputConfig.effort !== undefined || !effort || (!requested && !auxiliary))) return request;
    const options = {
      ...existing,
      ...(thinking !== undefined ? { thinking } : {}),
      ...(outputConfig.effort === undefined && effort && (requested || auxiliary)
        ? { output_config: { ...outputConfig, effort } } : {}),
    };
    return { ...request, providerOptions: options };
  }

  if (geminiRoute) {
    const generationConfig = record(existing.generationConfig);
    const supported = geminiThinkingConfig(routedModel, requested ?? "low");
    if (!supported) return request;
    const explicit = generationConfig.thinkingConfig;
    const thinkingConfig = {
      ...(explicit !== undefined ? record(explicit) : requested || auxiliary ? supported : {}),
      // Thought summaries power live reasoning status; leave explicit opt-outs intact.
      ...(!auxiliary && record(explicit).includeThoughts === undefined ? { includeThoughts: true } : {}),
    };
    if (auxiliary && explicit !== undefined) return request;
    return {
      ...request,
      providerOptions: {
        ...existing,
        generationConfig: { ...generationConfig, thinkingConfig },
      },
    };
  }

  if (parsed.provider === "openrouter" && existing.reasoning === undefined && (requested || auxiliary)) {
    const [vendor, ...modelParts] = parsed.model.split("/");
    const routedModel = modelParts.join("/");
    const baseEffort = requested ?? "low";
    const effort = vendor === "openai" && routedModel
      ? normalizeOpenAIEffort(routedModel, baseEffort)
      : baseEffort;
    return {
      ...request,
      providerOptions: { ...existing, reasoning: { effort } },
    };
  }

  // Custom OpenAI-compatible providers can opt into the common reasoning field.
  // If they reject it, the adapter's selective unsupported-parameter retry removes
  // only `reasoning` while preserving native tools and the rest of the request.
  if ([
    "openai", "codex-cli", "opencode", "opencode-go", "openrouter",
    "anthropic", "claude", "claude-api", "antigravity", "gemini", "gemini-web",
  ].includes(parsed.provider)) return request;

  if (requested && existing.reasoning === undefined
      && !["anthropic", "claude", "gemini", "gemini-web"].includes(parsed.provider)) {
    return {
      ...request,
      providerOptions: { ...existing, reasoning: { effort: requested } },
    };
  }

  return request;
}
