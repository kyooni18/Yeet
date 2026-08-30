export type ProviderId = string;
export type ModelId = `${string}/${string}`;

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

export interface Message {
  role: MessageRole;
  content?: string;
  toolCalls?: ToolCall[];
  toolCallId?: string;
  name?: string;
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
  system?: string;
  tools?: ToolDefinition[];
  toolChoice?: ToolChoice;
  temperature?: number;
  maxTokens?: number;
  metadata?: Record<string, string>;
  timeoutMs?: number;
  retry?: RetryPolicy;
  signal?: AbortSignal;
  providerOptions?: Record<string, unknown>;
}

export interface CallRequest extends CallRequestBase {
  /** Canonical model identifier. Always provider/model. */
  model: ModelId;
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
}

export type FinishReason = "stop" | "length" | "tool_call" | "content_filter" | "error" | "unknown";

export interface CallResult {
  provider: ProviderId;
  /** Canonical model id when returned by CallCore; provider-local name inside adapters. */
  model: string;
  id?: string;
  text: string;
  toolCalls: ToolCall[];
  finishReason: FinishReason;
  usage?: Usage;
  raw?: unknown;
}

export type StreamEvent =
  | { type: "start"; provider: ProviderId; model: string; id?: string }
  | { type: "text-delta"; delta: string }
  | { type: "tool-call-delta"; index: number; id?: string; name?: string; argumentsDelta?: string }
  | { type: "tool-call"; index: number; toolCall: ToolCall }
  | { type: "finish"; finishReason: FinishReason; usage?: Usage; raw?: unknown };

export interface ProviderAdapter {
  readonly id: ProviderId;
  complete(request: ProviderCallRequest): Promise<CallResult>;
  stream(request: ProviderCallRequest): AsyncIterable<StreamEvent>;
  /** Return provider-local model identifiers exposed by the provider API. */
  listModels?(): Promise<string[]>;
}

export type FetchLike = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;
