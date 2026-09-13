export type ProviderId = string;
export type ModelId = `${string}/${string}`;

/** Provider metadata for one provider-local model. */
export interface ModelInfo {
  id: string;
  /** Maximum input context in tokens, when the provider publishes it. */
  contextLength?: number;
  /** Published text-token pricing in USD per one million tokens. */
  pricing?: ModelPricing;
}

export interface ModelCostRates {
  input: number;
  output: number;
  cacheRead?: number;
  cacheWrite?: number;
}

export interface ModelCostTier extends ModelCostRates {
  /** Tier applies to the whole request when input tokens exceed this threshold. */
  thresholdTokens: number;
}

export interface ModelPricing extends ModelCostRates {
  currency: "USD";
  unit: "per1MTokens";
  source: string;
  tiers?: ModelCostTier[];
  modes?: Record<string, ModelCostRates>;
}

export interface ParsedModelId {
  provider: ProviderId;
  model: string;
}

export function parseModelId(value: string): ParsedModelId {
  const slash = value.indexOf("/");
  if (slash <= 0 || slash === value.length - 1) {
    throw new TypeError(`Model must use provider/model form, got: ${value}`);
  }
  const provider = value.slice(0, slash).trim();
  const model = value.slice(slash + 1).trim();
  if (!provider || !model) throw new TypeError(`Model must use provider/model form, got: ${value}`);
  return { provider, model };
}

export function modelId(provider: ProviderId, model: string): ModelId {
  if (!provider.trim() || provider.includes("/")) {
    throw new TypeError(`Provider id must be a non-empty single path segment, got: ${provider}`);
  }
  if (!model.trim()) throw new TypeError("Model name must not be empty");
  return `${provider}/${model}` as ModelId;
}

export type MessageRole = "system" | "user" | "assistant" | "tool";

export interface ToolCall {
  id: string;
  name: string;
  arguments: unknown;
}

/**
 * A lifecycle update for a tool call.
 *
 * Tool execution is owned by the embedding Rust host, so
 * these values deliberately carry presentation-safe data instead of an
 * executor callback.  Hosts can use the same shape for an initial progress
 * update and for a validation/runtime failure without changing the model
 * request contract.
 */
export type ToolCallFeedbackStatus = "started" | "in-progress" | "completed" | "failed";

export interface ToolCallFeedback {
  toolCallId: string;
  status: ToolCallFeedbackStatus;
  index?: number;
  name?: string;
  message?: string;
  details?: unknown;
}

/** The normalized output of executing one tool call. */
export interface ToolCallResult {
  toolCallId: string;
  content?: string;
  /** Structured output when a tool does not have a text representation. */
  result?: unknown;
  isError?: boolean;
  index?: number;
  name?: string;
  raw?: unknown;
}

/** Provider-neutral inline image carried with a chat message. */
export interface ImageAttachment {
  /** MIME type accepted by the supported multimodal provider adapters. */
  mediaType: "image/png" | "image/jpeg" | "image/webp" | "image/gif";
  /** Raw image bytes encoded as base64 without a data-URL prefix. */
  data: string;
  /** Optional display/debug name. Providers do not receive local filesystem paths. */
  name?: string;
}

export interface Message {
  role: MessageRole;
  content?: string;
  /** Inline images associated with this message. Normally present only on user turns. */
  images?: ImageAttachment[];
  toolCalls?: ToolCall[];
  toolCallId?: string;
  name?: string;
  /** Optional structured execution result supplied by a host. */
  toolResult?: ToolCallResult;
  /** Optional host feedback associated with a tool result. */
  toolFeedback?: ToolCallFeedback[];
  /** Request-local guidance excluded from durable compaction and cache prefixes. */
  requestOnly?: boolean;
  /** Runtime-internal marker for the final reusable prompt-cache content block. */
  cacheBreakpoint?: boolean;
}

export interface ToolDefinition {
  name: string;
  description?: string;
  inputSchema: Record<string, unknown>;
}

export type ToolChoice = "auto" | "none" | "required" | { name: string };

export interface RetryPolicy {
  maxAttempts?: number;
  baseDelayMs?: number;
  maxDelayMs?: number;
  jitter?: number;
}

interface CallRequestBase {
  messages: Message[];
  /** Harness-local conversation identity. Providers may use it for cache bucketing but must not treat it as user metadata. */
  contextKey?: string;
  system?: string;
  tools?: ToolDefinition[];
  /** Candidate tools that capable providers may expose through native tool search. */
  deferredTools?: ToolDefinition[];
  toolChoice?: ToolChoice;
  temperature?: number;
  maxTokens?: number;
  metadata?: Record<string, string>;
  timeoutMs?: number;
  retry?: RetryPolicy;
  signal?: AbortSignal;
  providerOptions?: Record<string, unknown>;
  /** Opt in to caching the replayable conversation prefix for this request. */
  promptCache?: boolean;
}

export interface CallRequest extends CallRequestBase {
  /** Canonical model identifier. Always provider/model. */
  model: ModelId;
  /**
   * Optional harness-side request capabilities attached by the embedding
   * client. `undefined` preserves the runtime defaults; an explicit empty
   * array disables every optional request capability for this call.
   */
  attachedCapabilities?: string[];
}

/** Request shape delivered to a provider adapter after routing. */
export interface ProviderCallRequest extends CallRequestBase {
  model: string;
}

export interface Usage {
  inputTokens?: number;
  outputTokens?: number;
  totalTokens?: number;
  cachedInputTokens?: number;
  /** Input tokens for calls where the provider explicitly reported cache-read telemetry. */
  cacheMeasuredInputTokens?: number;
  /** Input tokens for calls where cache-read telemetry was absent. */
  cacheUnreportedInputTokens?: number;
  cacheWriteInputTokens?: number;
  /** Input tokens normalized to the selected model's ordinary-input price. */
  costEquivalentInputTokens?: number;
  reasoningTokens?: number;
  modelCalls?: number;
  /** Provider HTTP attempts, including retries, used by the logical model call. */
  transportAttempts?: number;
  /** Estimated inference cost from the live model catalog, in USD. */
  estimatedCostUsd?: number;
  /** Provider-reported prompt-cache comparison outcome, when available. */
  providerCacheDiagnosticType?: PromptCacheDiagnostics["type"];
  providerCacheMissReason?: string;
  providerCacheMissedTokens?: number;
  providerComparisonReusableTokens?: number;
}

export type FinishReason = "stop" | "length" | "tool_call" | "content_filter" | "error" | "unknown";

export type PromptCacheMissReason =
  | "model_changed"
  | "prompt_cache_key_changed"
  | "tools_changed"
  | "text_format_changed"
  | "reasoning_effort_changed"
  | "verbosity_changed"
  | "context_compacted"
  | "input_changed"
  | "service_tier_changed";

/** Provider-reported comparison diagnostics from the OpenAI Responses API. */
export interface PromptCacheDiagnostics {
  type: "cache_hit" | "cache_miss" | "comparison_response_not_found" | "unavailable";
  cacheMissedTokens?: number;
  reason?: string;
  comparisonReusableTokens?: number;
}

export interface CallResult {
  provider: ProviderId;
  /** Canonical model id when returned by CallCore; provider-local name inside adapters. */
  model: string;
  id?: string;
  text: string;
  /** Full provider-emitted reasoning/thinking text, when the API exposes it. */
  reasoning?: string;
  /** Provider-emitted reasoning summary, when distinct from full reasoning. */
  reasoningSummary?: string;
  toolCalls: ToolCall[];
  /** Tool execution output supplied by an adapter/host, when available. */
  toolResults?: ToolCallResult[];
  /** Lifecycle/diagnostic feedback collected during the call. */
  toolFeedback?: ToolCallFeedback[];
  finishReason: FinishReason;
  usage?: Usage;
  /** Provider-reported prompt-cache comparison result, when explicitly available. */
  promptCacheDiagnostics?: PromptCacheDiagnostics;
  raw?: unknown;
}

export type StreamEvent =
  | { type: "start"; provider: ProviderId; model: string; id?: string }
  /** Full provider-emitted reasoning/thinking content. Never synthesized by CallCore. */
  | { type: "reasoning-delta"; delta: string }
  /** Provider-emitted reasoning summary content. Never synthesized by CallCore. */
  | { type: "reasoning-summary-delta"; delta: string }
  | { type: "text-delta"; delta: string }
  | { type: "tool-call-delta"; index: number; id?: string; name?: string; argumentsDelta?: string }
  | { type: "tool-call"; index: number; toolCall: ToolCall }
  | { type: "finish"; finishReason: FinishReason; usage?: Usage; promptCacheDiagnostics?: PromptCacheDiagnostics; raw?: unknown };

export interface EmbeddingRequest {
  model: string;
  input: string[];
  signal?: AbortSignal;
}
export interface EmbeddingResult {
  model: string;
  resolvedModel: string;
  source: string;
  vectors: number[][];
}

export interface ProviderAdapter {
  readonly id: ProviderId;
  embed?(request: EmbeddingRequest): Promise<EmbeddingResult>;
  complete(request: ProviderCallRequest): Promise<CallResult>;
  stream(request: ProviderCallRequest): AsyncIterable<StreamEvent>;
  /** Return provider-local model identifiers exposed by the provider API. */
  listModels?(): Promise<string[]>;
  /** Return provider-local model metadata exposed by the provider API. */
  listModelInfo?(): Promise<ModelInfo[]>;
}

export type FetchLike = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;
