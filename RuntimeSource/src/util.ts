import type { FinishReason, Message, ModelInfo, ModelPricing, ToolCall, ToolCallResult, Usage } from "./types.js";

export function splitSystem(messages: Message[], explicit?: string): { system?: string; messages: Message[] } {
  const systemParts: string[] = [];
  if (explicit) systemParts.push(explicit);
  const rest: Message[] = [];
  for (const message of messages) {
    if (message.role === "system") {
      if (message.content) systemParts.push(message.content);
    } else {
      rest.push(message);
    }
  }
  return {
    ...(systemParts.length > 0 ? { system: systemParts.join("\n\n") } : {}),
    messages: rest,
  };
}

/**
 * Hoist only the stable system prefix. Later system messages are commonly
 * request-local agent guidance; moving them ahead of conversation history
 * destroys provider prefix-cache locality.
 */
export function splitLeadingSystem(messages: Message[], explicit?: string): { system?: string; messages: Message[] } {
  const systemParts: string[] = [];
  if (explicit) systemParts.push(explicit);
  let index = 0;
  while (index < messages.length && messages[index]?.role === "system") {
    const content = messages[index]?.content;
    if (content) systemParts.push(content);
    index += 1;
  }
  return {
    ...(systemParts.length > 0 ? { system: systemParts.join("\n\n") } : {}),
    messages: messages.slice(index),
  };
}

/** Preserve the oldest explicit cache boundary as the immutable base, then use
 * remaining provider capacity for the newest rolling boundaries. Callers pass
 * candidate indexes in ascending transcript order. */
export function selectStableAndRecentIndexes(indexes: number[], limit: number): Set<number> {
  const selected = new Set<number>();
  if (limit <= 0 || indexes.length === 0) return selected;
  selected.add(indexes[0]!);
  for (let position = indexes.length - 1; position > 0 && selected.size < limit; position--) {
    selected.add(indexes[position]!);
  }
  return selected;
}

export function safeJsonParse(value: string): unknown {
  if (!value) return {};
  try {
    return JSON.parse(value) as unknown;
  } catch {
    return value;
  }
}

export function normalizeFinishReason(value: unknown): FinishReason {
  if (typeof value !== "string") return "unknown";
  switch (value.toLowerCase()) {
    case "stop":
    case "end_turn":
    case "stop_sequence":
      return "stop";
    case "length":
    case "max_tokens":
    case "max_output_tokens":
      return "length";
    case "tool_calls":
    case "tool_use":
      return "tool_call";
    case "content_filter":
    case "safety":
    case "recitation":
      return "content_filter";
    default:
      return "unknown";
  }
}

export function usage(
  input?: number,
  output?: number,
  total?: number,
  cached?: number,
  cacheWrite?: number,
  reasoning?: number,
  transportAttempts?: number,
): Usage | undefined {
  if ([input, output, total, cached, cacheWrite, reasoning, transportAttempts].every((value) => value === undefined)) return undefined;
  return {
    ...(input !== undefined ? { inputTokens: input } : {}),
    ...(output !== undefined ? { outputTokens: output } : {}),
    ...(total !== undefined ? { totalTokens: total } : {}),
    ...(cached !== undefined ? { cachedInputTokens: cached } : {}),
    ...(input !== undefined && cached !== undefined ? { cacheMeasuredInputTokens: input } : {}),
    ...(input !== undefined && cached === undefined ? { cacheUnreportedInputTokens: input } : {}),
    ...(cacheWrite !== undefined ? { cacheWriteInputTokens: cacheWrite } : {}),
    ...(reasoning !== undefined ? { reasoningTokens: reasoning } : {}),
    ...(transportAttempts !== undefined ? { transportAttempts } : {}),
  };
}

export function normalizeToolCall(id: string | undefined, name: string | undefined, args: unknown, index: number): ToolCall {
  return {
    id: id ?? `tool-${index}`,
    name: name ?? "unknown",
    arguments: args,
  };
}

/**
 * Convert a structured tool result to the text field expected by provider
 * APIs.  Text content remains lossless; structured values are encoded once so
 * providers receive valid JSON rather than `[object Object]`.
 */
export function toolResultContent(message: Message): string {
  if (message.content !== undefined) return message.content;
  const result: ToolCallResult | undefined = message.toolResult;
  if (!result) return "";
  if (result.content !== undefined) return result.content;
  if (result.result === undefined) return "";
  if (typeof result.result === "string") return result.result;
  try {
    const encoded = JSON.stringify(result.result);
    return encoded === undefined ? String(result.result) : encoded;
  } catch {
    return String(result.result);
  }
}

/**
 * Normalize the model-list response shapes used by OpenAI-compatible APIs and
 * Gemini. Provider adapters return provider-local names, so Gemini's
 * `models/` resource prefix is removed here.
 */
export function normalizeModelIds(raw: unknown): string[] {
  return normalizeModelInfo(raw).map((model) => model.id);
}

function modelContextLength(entry: Record<string, unknown>): number | undefined {
  const limits = entry.limits && typeof entry.limits === "object" && !Array.isArray(entry.limits)
    ? entry.limits as Record<string, unknown>
    : undefined;
  const metadata = entry.metadata && typeof entry.metadata === "object" && !Array.isArray(entry.metadata)
    ? entry.metadata as Record<string, unknown>
    : undefined;
  const candidates = [
    entry.context_length,
    entry.contextLength,
    entry.context_window,
    entry.contextWindow,
    entry.max_context_length,
    entry.maxContextLength,
    entry.inputTokenLimit,
    entry.input_token_limit,
    limits?.context,
    limits?.contextLength,
    metadata?.context_length,
    metadata?.contextLength,
  ];
  for (const value of candidates) {
    if (typeof value === "number" && Number.isSafeInteger(value) && value > 0) return value;
    if (typeof value === "string" && /^\d+$/.test(value)) {
      const parsed = Number(value);
      if (Number.isSafeInteger(parsed) && parsed > 0) return parsed;
    }
  }
  return undefined;
}

/** Normalize model-list entries while retaining common context-window fields. */
export function normalizeModelInfo(raw: unknown): ModelInfo[] {
  const value = raw && typeof raw === "object" && !Array.isArray(raw)
    ? raw as Record<string, unknown>
    : undefined;
  const entries = Array.isArray(raw)
    ? raw
    : Array.isArray(value?.data)
      ? value.data
      : Array.isArray(value?.models)
        ? value.models
        : [];
  const result: ModelInfo[] = [];
  const seen = new Map<string, number>();

  const add = (rawId: string, entry?: Record<string, unknown>) => {
    const trimmed = rawId.trim();
    const id = trimmed.startsWith("models/") ? trimmed.slice("models/".length) : trimmed;
    if (!id) return;
    const length = entry ? modelContextLength(entry) : undefined;
    const pricing = entry ? normalizeProviderPricing(entry.pricing) : undefined;
    const existing = seen.get(id);
    if (existing !== undefined) {
      const current = result[existing]!;
      if ((length !== undefined && current.contextLength === undefined)
        || (pricing !== undefined && current.pricing === undefined)) {
        result[existing] = {
          ...current,
          ...(length !== undefined ? { contextLength: length } : {}),
          ...(pricing !== undefined ? { pricing } : {}),
        };
      }
      return;
    }
    seen.set(id, result.length);
    result.push({
      id,
      ...(length !== undefined ? { contextLength: length } : {}),
      ...(pricing !== undefined ? { pricing } : {}),
    });
  };

  for (const entry of entries) {
    if (typeof entry === "string") {
      add(entry);
      continue;
    }
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) continue;
    const candidate = entry as Record<string, unknown>;
    const rawId = typeof candidate.id === "string"
      ? candidate.id
      : typeof candidate.slug === "string"
        ? candidate.slug
        : typeof candidate.name === "string"
          ? candidate.name
          : undefined;
    if (!rawId) continue;
    add(rawId, candidate);
  }

  return result;
}

function normalizeProviderPricing(raw: unknown): ModelPricing | undefined {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return undefined;
  const pricing = raw as Record<string, unknown>;
  const perMillion = (value: unknown): number | undefined => {
    const parsed = typeof value === "number" ? value : typeof value === "string" ? Number(value) : NaN;
    return Number.isFinite(parsed) && parsed >= 0 ? parsed * 1_000_000 : undefined;
  };
  const input = perMillion(pricing.prompt);
  const output = perMillion(pricing.completion);
  if (input === undefined || output === undefined) return undefined;
  const cacheRead = perMillion(pricing.input_cache_read);
  const cacheWrite = perMillion(pricing.input_cache_write);
  const tiers = Array.isArray(pricing.overrides)
    ? pricing.overrides.flatMap((value) => {
      if (!value || typeof value !== "object" || Array.isArray(value)) return [];
      const override = value as Record<string, unknown>;
      const minimum = typeof override.min_prompt_tokens === "number"
        ? override.min_prompt_tokens
        : Number(override.min_prompt_tokens);
      const tierInput = perMillion(override.prompt);
      const tierOutput = perMillion(override.completion);
      if (!Number.isSafeInteger(minimum) || minimum <= 0 || tierInput === undefined || tierOutput === undefined) return [];
      const tierCacheRead = perMillion(override.input_cache_read);
      const tierCacheWrite = perMillion(override.input_cache_write);
      return [{
        input: tierInput,
        output: tierOutput,
        ...(tierCacheRead !== undefined ? { cacheRead: tierCacheRead } : {}),
        ...(tierCacheWrite !== undefined ? { cacheWrite: tierCacheWrite } : {}),
        thresholdTokens: minimum - 1,
      }];
    })
    : undefined;
  return {
    input,
    output,
    ...(cacheRead !== undefined ? { cacheRead } : {}),
    ...(cacheWrite !== undefined ? { cacheWrite } : {}),
    currency: "USD",
    unit: "per1MTokens",
    source: "provider",
    ...(tiers?.length ? { tiers } : {}),
  };
}
