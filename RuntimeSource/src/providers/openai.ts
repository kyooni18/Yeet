import { fetchEmbeddings } from "../embeddings.js";
import { ProviderError } from "../errors.js";
import type { EmbeddingRequest, EmbeddingResult } from "../types.js";
import { providerFetch, readJson } from "../http.js";
import type { ProviderFetchLogger } from "../http.js";
import { parseSSE } from "../sse.js";
import type {
  ProviderCallRequest,
  CallResult,
  FetchLike,
  ImageAttachment,
  Message,
  ModelInfo,
  ProviderAdapter,
  StreamEvent,
  ToolChoice,
} from "../types.js";
import { normalizeFinishReason, normalizeModelInfo, normalizeToolCall, safeJsonParse, splitLeadingSystem, toolResultContent, usage } from "../util.js";

export interface OpenAIProviderOptions {
  id?: string;
  apiKey?: string;
  accessToken?: string;
  accountId?: string;
  baseUrl?: string;
  organization?: string;
  project?: string;
  /** Codex protocol version used for OAuth model discovery. */
  clientVersion?: string;
  fetch?: FetchLike;
  apiCallLogger?: ProviderFetchLogger;
}

function mapToolChoice(choice: ToolChoice | undefined): unknown {
  if (!choice) return undefined;
  if (typeof choice === "string") return choice;
  return { type: "function", name: choice.name };
}

function dataUrl(image: ImageAttachment): string {
  return `data:${image.mediaType};base64,${image.data}`;
}

function responseContent(message: Message, promptCacheSupported: boolean): unknown {
  if (!message.images?.length && !(promptCacheSupported && message.cacheBreakpoint)) return message.content ?? "";
  const content: any[] = [
    ...(message.content ? [{ type: "input_text", text: message.content }] : []),
    ...(message.images ?? []).map((image) => ({ type: "input_image", image_url: dataUrl(image) })),
  ];
  if (promptCacheSupported && message.cacheBreakpoint && content.length > 0) {
    content[content.length - 1] = {
      ...content[content.length - 1],
      prompt_cache_breakpoint: { mode: "explicit" },
    };
  }
  return content;
}

function mapInput(messages: Message[], promptCacheSupported: boolean, cacheHistory = false): unknown[] {
  // A coordinator-selected breakpoint is the immutable prefix for the turn.
  // Do not synthesize newer tool-result breakpoints when one already exists:
  // doing so rewrites the growing tool trace into the prompt cache every round.
  // Auto-boundaries remain a fallback for callers that only enable caching.
  const hasStableBreakpoint = messages.some((message) => message.cacheBreakpoint === true);
  const synthesizeHistoryBreakpoints = cacheHistory && !hasStableBreakpoint;
  const cacheIndices = new Set<number>();
  if (promptCacheSupported) {
    for (let index = messages.length - 1; index >= 0 && cacheIndices.size < 50; index--) {
      const message = messages[index]!;
      if (message.cacheBreakpoint || (synthesizeHistoryBreakpoints && !message.requestOnly &&
          (message.role === "user" || message.role === "tool"))) {
        cacheIndices.add(index);
      }
    }
  }
  const input: any[] = [];
  for (const [index, original] of messages.entries()) {
    const message = { ...original, cacheBreakpoint: cacheIndices.has(index) };
    if (message.role === "tool") {
      const output = toolResultContent(message);
      input.push({
        type: "function_call_output",
        call_id: message.toolCallId ?? message.toolResult?.toolCallId ?? "",
        output: promptCacheSupported && message.cacheBreakpoint
          ? [{ type: "input_text", text: output, prompt_cache_breakpoint: { mode: "explicit" } }]
          : output,
      });
      continue;
    }

    if (message.content || message.images?.length) {
      input.push({ role: message.role, content: responseContent(message, promptCacheSupported) });
    }

    if (message.role === "assistant") {
      for (const tool of message.toolCalls ?? []) {
        input.push({
          type: "function_call",
          call_id: tool.id,
          name: tool.name,
          arguments: typeof tool.arguments === "string" ? tool.arguments : JSON.stringify(tool.arguments),
        });
      }
    }
  }
  return input;
}

function bodyFor(request: ProviderCallRequest, stream: boolean, codex = false): Record<string, unknown> {
  const split = splitLeadingSystem(request.messages, request.system);
  const toolChoice = mapToolChoice(request.toolChoice);
  const promptCacheKey = request.metadata?.sessionId ?? request.contextKey;
  const promptCacheSupported = !codex && request.promptCache !== false && supportsPromptCacheOptions(request.model);
  const cacheOptions = promptCacheSupported
    ? { prompt_cache_options: { mode: "explicit", ttl: "30m" } }
    : {};
  const body: Record<string, unknown> = {
    ...(request.providerOptions ?? {}),
    model: request.model,
    input: mapInput(split.messages, promptCacheSupported, request.promptCache === true),
    stream,
    ...(split.system ? { instructions: split.system } : {}),
    ...(request.temperature !== undefined ? { temperature: request.temperature } : {}),
    ...(request.maxTokens !== undefined ? { max_output_tokens: request.maxTokens } : {}),
    ...(request.tools?.length
      ? {
          tools: request.tools.map((tool) => ({
            type: "function",
            name: tool.name,
            ...(tool.description ? { description: tool.description } : {}),
            parameters: tool.inputSchema,
            // These are provider-neutral schemas with optional fields and
            // operation unions, not strict Structured Outputs schemas.
            strict: false,
          })),
        }
      : {}),
    ...(toolChoice !== undefined ? { tool_choice: toolChoice } : {}),
    ...(request.metadata ? { metadata: request.metadata } : {}),
    ...(promptCacheKey ? { prompt_cache_key: promptCacheKey } : {}),
    ...cacheOptions,
  };
  if (codex) {
    // ChatGPT's Codex endpoint is stateless and streaming-only. These API
    // options are not supported by the subscription-backed endpoint.
    body.store = false;
    body.stream = true;
    body.instructions ??= "";
    for (const key of ["temperature", "top_p", "max_output_tokens", "metadata", "prompt_cache_options", "service_tier"]) {
      delete body[key];
    }
  }
  return body;
}

function supportsPromptCacheOptions(model: string): boolean {
  const match = /^gpt-(\d+)(?:\.(\d+))?/.exec(model);
  if (!match) return false;
  const major = Number(match[1]);
  const minor = Number(match[2] ?? 0);
  return major > 5 || (major === 5 && minor >= 6);
}

function finishReason(raw: any): ReturnType<typeof normalizeFinishReason> {
  const hasToolCall = (raw.output ?? []).some((item: any) => item.type === "function_call");
  if (hasToolCall) return "tool_call";
  if (raw.status === "completed") return "stop";
  if (raw.status === "incomplete") return normalizeFinishReason(raw.incomplete_details?.reason);
  return "unknown";
}

function outputText(raw: any): string {
  if (typeof raw.output_text === "string") return raw.output_text;
  return (raw.output ?? [])
    .filter((item: any) => item.type === "message")
    .flatMap((item: any) => item.content ?? [])
    .filter((part: any) => part.type === "output_text")
    .map((part: any) => part.text ?? "")
    .join("");
}

function reasoningSummary(raw: any): string | undefined {
  const text = (raw.output ?? [])
    .filter((item: any) => item.type === "reasoning")
    .flatMap((item: any) => item.summary ?? [])
    .map((part: any) => typeof part?.text === "string" ? part.text : "")
    .join("");
  return text || undefined;
}

function reasoningText(raw: any): string | undefined {
  // The official Responses API normally exposes summaries rather than hidden
  // chain-of-thought. Some compatible endpoints do return explicit plaintext
  // reasoning content; preserve it when present without manufacturing any.
  const text = (raw.output ?? [])
    .filter((item: any) => item.type === "reasoning")
    .flatMap((item: any) => item.content ?? item.reasoning ?? [])
    .map((part: any) => {
      if (typeof part === "string") return part;
      return typeof part?.text === "string" ? part.text : "";
    })
    .join("");
  return text || undefined;
}

export class OpenAIProvider implements ProviderAdapter {
  readonly id: string;
  readonly #apiKey: string | undefined;
  readonly #accessToken: string | undefined;
  readonly #accountId: string | undefined;
  readonly #baseUrl: string;
  readonly #organization: string | undefined;
  readonly #project: string | undefined;
  readonly #fetch: FetchLike | undefined;
  readonly #apiCallLogger: ProviderFetchLogger | undefined;
  readonly #clientVersion: string;

  constructor(options: OpenAIProviderOptions = {}) {
    this.id = options.id ?? "openai";
    this.#accessToken = options.accessToken;
    this.#accountId = options.accountId;
    this.#apiKey = options.apiKey;
    this.#baseUrl = (options.baseUrl
      ?? (this.#accessToken ? "https://chatgpt.com/backend-api/codex" : "https://api.openai.com/v1"))
      .replace(/\/$/, "");
    this.#organization = options.organization;
    this.#project = options.project;
    this.#fetch = options.fetch;
    this.#apiCallLogger = options.apiCallLogger;
    this.#clientVersion = options.clientVersion ?? "0.151.0";
  }

  #headers(request?: ProviderCallRequest): Record<string, string> {
    const token = this.#accessToken ?? this.#apiKey;
    if (!token) throw new Error(`Missing API key or OAuth token for ${this.id}`);
    if (this.#accessToken && !this.#accountId) {
      throw new Error(`Missing ChatGPT account id for ${this.id} OAuth credentials`);
    }
    const sessionId = this.#accessToken
      ? (request?.metadata?.sessionId ?? request?.contextKey)
      : undefined;
    const threadId = this.#accessToken
      ? (request?.metadata?.threadId ?? request?.metadata?.contextWindowId ?? request?.contextKey)
      : undefined;
    return {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
      ...(this.#accessToken ? {
        ...(this.#accountId ? { "ChatGPT-Account-ID": this.#accountId } : {}),
        originator: "codex_cli_rs",
        ...(sessionId ? { "session-id": sessionId } : {}),
        ...(threadId ? { "thread-id": threadId, "x-client-request-id": threadId } : {}),
      } : {}),
      ...(this.#organization ? { "OpenAI-Organization": this.#organization } : {}),
      ...(this.#project ? { "OpenAI-Project": this.#project } : {}),
    };
  }

  async embed(request: EmbeddingRequest): Promise<EmbeddingResult> {
    return fetchEmbeddings({ provider: this.id, baseUrl: this.#baseUrl,
      headers: this.#headers(), request, format: "openai",
      fetch: this.#fetch, apiCallLogger: this.#apiCallLogger });
  }

  async listModels(): Promise<string[]> {
    return (await this.listModelInfo()).map((model) => model.id);
  }

  async listModelInfo(): Promise<ModelInfo[]> {
    // The ChatGPT OAuth models endpoint requires the client version, while
    // the public OpenAI API accepts the plain /v1/models URL unchanged.
    const modelsUrl = this.#accessToken
      ? `${this.#baseUrl}/models?client_version=${encodeURIComponent(this.#clientVersion)}`
      : `${this.#baseUrl}/models`;
    const response = await providerFetch(
      modelsUrl,
      { method: "GET", headers: this.#headers() },
      {
        provider: this.id,
        ...(this.#fetch ? { fetch: this.#fetch } : {}),
        ...(this.#apiCallLogger ? { apiCallLogger: this.#apiCallLogger } : {}),
      },
    );
    return normalizeModelInfo(await readJson(response));
  }

  async complete(request: ProviderCallRequest): Promise<CallResult> {
    let raw: any;
    let streamedText = "";
    let streamedReasoning = "";
    let streamedSummary = "";
    const streamedTools: CallResult["toolCalls"] = [];
    if (this.#accessToken) {
      for await (const event of this.stream(request)) {
        if (event.type === "finish") raw = event.raw;
        else if (event.type === "text-delta") streamedText += event.delta;
        else if (event.type === "reasoning-delta") streamedReasoning += event.delta;
        else if (event.type === "reasoning-summary-delta") streamedSummary += event.delta;
        else if (event.type === "tool-call") streamedTools.push(event.toolCall);
      }
    } else {
      const response = await providerFetch(
        `${this.#baseUrl}/responses`,
        { method: "POST", headers: this.#headers(request), body: JSON.stringify(bodyFor(request, false)) },
        {
          provider: this.id,
          ...(this.#fetch ? { fetch: this.#fetch } : {}),
          ...(this.#apiCallLogger ? { apiCallLogger: this.#apiCallLogger } : {}),
          ...(request.timeoutMs !== undefined ? { timeoutMs: request.timeoutMs } : {}),
          ...(request.retry ? { retry: request.retry } : {}),
          ...(request.signal ? { signal: request.signal } : {}),
        },
      );
      raw = await readJson<any>(response);
    }
    const toolCalls = streamedTools.length ? streamedTools : (raw.output ?? [])
      .filter((item: any) => item.type === "function_call")
      .map((item: any, index: number) =>
        normalizeToolCall(item.call_id ?? item.id, item.name, safeJsonParse(item.arguments ?? ""), index),
      );
    const normalizedUsage = usage(
      raw.usage?.input_tokens,
      raw.usage?.output_tokens,
      raw.usage?.total_tokens,
      raw.usage?.input_tokens_details?.cached_tokens,
      raw.usage?.input_tokens_details?.cache_write_tokens,
      raw.usage?.output_tokens_details?.reasoning_tokens,
    );
    const normalizedReasoning = streamedReasoning || reasoningText(raw);
    const normalizedReasoningSummary = streamedSummary || reasoningSummary(raw);

    return {
      provider: this.id,
      model: raw.model ?? request.model,
      ...(raw.id ? { id: raw.id } : {}),
      text: streamedText || outputText(raw),
      ...(normalizedReasoning ? { reasoning: normalizedReasoning } : {}),
      ...(normalizedReasoningSummary ? { reasoningSummary: normalizedReasoningSummary } : {}),
      toolCalls,
      finishReason: toolCalls.length ? "tool_call" : finishReason(raw),
      ...(normalizedUsage ? { usage: normalizedUsage } : {}),
      raw,
    };
  }

  async *stream(request: ProviderCallRequest): AsyncIterable<StreamEvent> {
    const response = await providerFetch(
      `${this.#baseUrl}/responses`,
      { method: "POST", headers: this.#headers(request), body: JSON.stringify(bodyFor(request, true, Boolean(this.#accessToken))) },
      {
        provider: this.id,
        ...(this.#fetch ? { fetch: this.#fetch } : {}),
        ...(this.#apiCallLogger ? { apiCallLogger: this.#apiCallLogger } : {}),
        ...(request.timeoutMs !== undefined ? { timeoutMs: request.timeoutMs } : {}),
        ...(request.retry ? { retry: request.retry } : {}),
        ...(request.signal ? { signal: request.signal } : {}),
      },
    );

    let started = false;
    let completedRaw: any;
    const tools = new Map<number, { id?: string; name?: string; argumentsText: string }>();

    for await (const event of parseSSE(response)) {
      if (event.data === "[DONE]") break;
      let raw: any;
      try {
        raw = JSON.parse(event.data);
      } catch {
        continue;
      }

      if (raw.type === "error" || raw.type === "response.failed") {
        const error = raw.response?.error ?? raw.error ?? raw;
        throw new ProviderError(error.message ?? "OpenAI response failed", { provider: this.id, cause: raw });
      }

      if (raw.type === "response.created") {
        started = true;
        yield {
          type: "start",
          provider: this.id,
          model: raw.response?.model ?? request.model,
          ...(raw.response?.id ? { id: raw.response.id } : {}),
        };
        continue;
      }

      if (!started) {
        started = true;
        yield { type: "start", provider: this.id, model: request.model };
      }

      if (raw.type === "response.output_text.delta" && typeof raw.delta === "string") {
        yield { type: "text-delta", delta: raw.delta };
      } else if (raw.type === "response.reasoning_text.delta" && typeof raw.delta === "string") {
        // Supported by some Responses-compatible endpoints that expose
        // plaintext reasoning. OpenAI itself may never emit this event.
        yield { type: "reasoning-delta", delta: raw.delta };
      } else if (raw.type === "response.reasoning_summary_text.delta" && typeof raw.delta === "string") {
        yield { type: "reasoning-summary-delta", delta: raw.delta };
      } else if (raw.type === "response.output_item.added" && raw.item?.type === "function_call") {
        const index = raw.output_index ?? 0;
        const current: { id?: string; name?: string; argumentsText: string } = {
          argumentsText: raw.item.arguments ?? "",
        };
        if (raw.item.call_id ?? raw.item.id) current.id = raw.item.call_id ?? raw.item.id;
        if (raw.item.name) current.name = raw.item.name;
        tools.set(index, current);
        yield {
          type: "tool-call-delta",
          index,
          ...(current.id ? { id: current.id } : {}),
          ...(current.name ? { name: current.name } : {}),
        };
      } else if (raw.type === "response.function_call_arguments.delta") {
        const index = raw.output_index ?? 0;
        const current: { id?: string; name?: string; argumentsText: string } = tools.get(index) ?? {
          argumentsText: "",
        };
        current.argumentsText += raw.delta ?? "";
        tools.set(index, current);
        if (raw.delta) yield { type: "tool-call-delta", index, argumentsDelta: raw.delta };
      } else if (raw.type === "response.function_call_arguments.done") {
        const index = raw.output_index ?? 0;
        const current: { id?: string; name?: string; argumentsText: string } = tools.get(index) ?? {
          argumentsText: "",
        };
        if (typeof raw.arguments === "string") current.argumentsText = raw.arguments;
        tools.set(index, current);
      } else if (raw.type === "response.completed" || raw.type === "response.incomplete") {
        completedRaw = raw.response;
      }
    }

    if (!completedRaw) {
      throw new ProviderError("OpenAI stream ended before a completed response", { provider: this.id, retryable: true });
    }

    for (const [index, tool] of [...tools.entries()].sort(([a], [b]) => a - b)) {
      yield {
        type: "tool-call",
        index,
        toolCall: normalizeToolCall(tool.id, tool.name, safeJsonParse(tool.argumentsText), index),
      };
    }

    const normalizedUsage = completedRaw
      ? usage(
          completedRaw.usage?.input_tokens,
          completedRaw.usage?.output_tokens,
          completedRaw.usage?.total_tokens,
          completedRaw.usage?.input_tokens_details?.cached_tokens,
          completedRaw.usage?.input_tokens_details?.cache_write_tokens,
          completedRaw.usage?.output_tokens_details?.reasoning_tokens,
        )
      : undefined;
    yield {
      type: "finish",
      finishReason: tools.size > 0 ? "tool_call" : finishReason(completedRaw),
      ...(normalizedUsage ? { usage: normalizedUsage } : {}),
      ...(completedRaw ? { raw: completedRaw } : {}),
    };
  }
}
