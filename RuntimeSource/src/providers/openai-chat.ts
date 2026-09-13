import { fetchEmbeddings } from "../embeddings.js";
import type { EmbeddingRequest, EmbeddingResult } from "../types.js";
import { createHash } from "node:crypto";
import { providerFetch, providerFetchAttempts, readJson } from "../http.js";
import type { ProviderFetchLogger } from "../http.js";
import { parseSSE } from "../sse.js";
import { promptCacheCapabilities } from "../cache-capabilities.js";
import type {
  ProviderCallRequest,
  CallResult,
  FetchLike,
  ImageAttachment,
  Message,
  ModelInfo,
  ProviderAdapter,
  StreamEvent,
  ToolCall,
  ToolChoice,
  ToolDefinition,
} from "../types.js";
import { normalizeFinishReason, normalizeModelInfo, normalizeToolCall, safeJsonParse, selectStableAndRecentIndexes, splitLeadingSystem, toolResultContent, usage } from "../util.js";

export interface OpenAIChatProviderOptions {
  id?: string;
  apiKey?: string;
  baseUrl?: string;
  headers?: Record<string, string>;
  requireApiKey?: boolean;
  /** Route repeated requests with one stable provider-side session affinity key. */
  useContextSessionId?: boolean;
  /** Encode caller-selected message cache boundaries as cache_control blocks. */
  useContentCacheBreakpoints?: boolean;
  fetch?: FetchLike;
  apiCallLogger?: ProviderFetchLogger;
}

type ChatMessage = Record<string, unknown>;

function dataUrl(image: ImageAttachment): string {
  return `data:${image.mediaType};base64,${image.data}`;
}

function chatContent(message: Message, cacheBreakpoint = false): unknown {
  if (!message.images?.length) {
    if (!cacheBreakpoint) return message.content ?? "";
    return [{ type: "text", text: message.content ?? "", cache_control: { type: "ephemeral" } }];
  }
  const content: Record<string, unknown>[] = [
    ...(message.content ? [{ type: "text", text: message.content }] : []),
    ...message.images.map((image) => ({ type: "image_url", image_url: { url: dataUrl(image) } })),
  ];
  if (cacheBreakpoint && content.length > 0) {
    content[content.length - 1] = { ...content[content.length - 1], cache_control: { type: "ephemeral" } };
  }
  return content;
}

function firstString(value: any, keys: string[]): string | undefined {
  for (const key of keys) {
    if (typeof value?.[key] === "string" && value[key]) return value[key];
  }
  return undefined;
}

function reasoningDetailsText(value: any): string | undefined {
  if (!Array.isArray(value?.reasoning_details)) return undefined;
  const text = value.reasoning_details
    .filter((part: any) => part && typeof part === "object" && typeof part.text === "string")
    .map((part: any) => part.text)
    .join("");
  return text || undefined;
}

function reasoningText(value: any): string | undefined {
  return firstString(value, ["reasoning_content", "reasoning", "reasoning_text", "thinking"])
    ?? reasoningDetailsText(value);
}

function reasoningSummary(value: any): string | undefined {
  return firstString(value, ["reasoning_summary", "reasoning_summary_text", "thinking_summary"]);
}

function reasoningTokenCount(value: any): number | undefined {
  const candidates = [
    value?.completion_tokens_details?.reasoning_tokens,
    value?.completion_tokens_details?.thinking_tokens,
    value?.output_tokens_details?.reasoning_tokens,
    value?.output_tokens_details?.thinking_tokens,
    value?.reasoning_tokens,
    value?.thinking_tokens,
  ];
  return candidates.find((candidate) => typeof candidate === "number" && Number.isFinite(candidate) && candidate >= 0);
}

function mapToolChoice(choice: ToolChoice | undefined): unknown {
  if (!choice) return undefined;
  if (typeof choice === "string") {
    if (choice === "required") return "required";
    return choice;
  }
  return { type: "function", function: { name: choice.name } };
}

function mapTools(tools: ToolDefinition[] | undefined): unknown[] | undefined {
  return tools?.map((tool) => ({
    type: "function",
    function: {
      name: tool.name,
      ...(tool.description ? { description: tool.description } : {}),
      parameters: tool.inputSchema,
    },
  }));
}

function stableSessionId(value: string | undefined): string | undefined {
  if (!value) return undefined;
  if (value.length <= 256) return value;
  return `yeet-${createHash("sha256").update(value).digest("hex")}`;
}


function mapMessages(
  messages: Message[],
  explicitSystem?: string,
  useContentCacheBreakpoints = false,
  maxContentCacheBreakpoints = 4,
): ChatMessage[] {
  const { system, messages: rest } = splitLeadingSystem(messages, explicitSystem);
  const output: ChatMessage[] = [];
  if (system) output.push({ role: "system", content: system });
  const candidates = useContentCacheBreakpoints
    ? rest
      .map((message, index) => message.cacheBreakpoint === true ? index : -1)
      .filter((index) => index >= 0)
    : [];
  const cacheIndexes = selectStableAndRecentIndexes(candidates, maxContentCacheBreakpoints);

  for (const [index, message] of rest.entries()) {
    if (message.role === "tool") {
      const content = toolResultContent(message);
      output.push({
        role: "tool",
        content: cacheIndexes.has(index)
          ? [{ type: "text", text: content, cache_control: { type: "ephemeral" } }]
          : content,
        tool_call_id: message.toolCallId ?? message.toolResult?.toolCallId ?? "",
        ...(message.name ? { name: message.name } : {}),
      });
      continue;
    }

    if (message.role === "assistant" && message.toolCalls?.length) {
      output.push({
        role: "assistant",
        content: message.content ?? null,
        tool_calls: message.toolCalls.map((tool) => ({
          id: tool.id,
          type: "function",
          function: {
            name: tool.name,
            arguments: typeof tool.arguments === "string" ? tool.arguments : JSON.stringify(tool.arguments),
          },
        })),
      });
      continue;
    }

    output.push({ role: message.role, content: chatContent(message, cacheIndexes.has(index)) });
  }

  return output;
}

function requestBody(
  request: ProviderCallRequest,
  stream: boolean,
  providerId: string,
  useContextSessionId = false,
  useContentCacheBreakpoints = false,
): Record<string, unknown> {
  const cacheCapabilities = promptCacheCapabilities(providerId, request.model);
  const tools = mapTools(request.tools);
  const toolChoice = mapToolChoice(request.toolChoice);
  const sessionId = useContextSessionId && cacheCapabilities.sessionAffinity !== false
    ? stableSessionId(request.metadata?.sessionId ?? request.contextKey)
    : undefined;
  return {
    ...(request.providerOptions ?? {}),
    model: request.model,
    messages: mapMessages(
      request.messages,
      request.system,
      useContentCacheBreakpoints && request.promptCache !== false,
      cacheCapabilities.maxExplicitBreakpoints ?? 4,
    ),
    stream,
    ...(stream ? { stream_options: { include_usage: true } } : {}),
    ...(request.temperature !== undefined ? { temperature: request.temperature } : {}),
    ...(request.maxTokens !== undefined ? { max_tokens: request.maxTokens } : {}),
    ...(request.metadata ? { metadata: request.metadata } : {}),
    ...(sessionId ? { session_id: sessionId } : {}),
    ...(tools?.length ? { tools } : {}),
    ...(toolChoice !== undefined ? { tool_choice: toolChoice } : {}),
  };
}

export class OpenAIChatProvider implements ProviderAdapter {
  readonly id: string;
  readonly #apiKey: string | undefined;
  readonly #baseUrl: string;
  readonly #headers: Record<string, string>;
  readonly #requireApiKey: boolean;
  readonly #useContextSessionId: boolean;
  readonly #useContentCacheBreakpoints: boolean;
  readonly #fetch: FetchLike | undefined;
  readonly #apiCallLogger: ProviderFetchLogger | undefined;

  constructor(options: OpenAIChatProviderOptions = {}) {
    this.id = options.id ?? "openai-compatible";
    this.#apiKey = options.apiKey;
    this.#baseUrl = (options.baseUrl ?? "https://api.openai.com/v1").replace(/\/$/, "");
    this.#headers = options.headers ?? {};
    this.#requireApiKey = options.requireApiKey ?? false;
    this.#useContextSessionId = options.useContextSessionId ?? false;
    this.#useContentCacheBreakpoints = options.useContentCacheBreakpoints ?? false;
    this.#fetch = options.fetch;
    this.#apiCallLogger = options.apiCallLogger;
  }

  #requestHeaders(): Record<string, string> {
    if (this.#requireApiKey && !this.#apiKey) throw new Error(`Missing API key for ${this.id}`);
    return {
      ...(this.#apiKey ? { authorization: `Bearer ${this.#apiKey}` } : {}),
      "content-type": "application/json",
      ...this.#headers,
    };
  }

  async embed(request: EmbeddingRequest): Promise<EmbeddingResult> {
    return fetchEmbeddings({ provider: this.id, baseUrl: this.#baseUrl,
      headers: this.#requestHeaders(), request, format: "openai",
      fetch: this.#fetch, apiCallLogger: this.#apiCallLogger });
  }

  async listModels(): Promise<string[]> {
    return (await this.listModelInfo()).map((model) => model.id);
  }

  async listModelInfo(): Promise<ModelInfo[]> {
    const response = await providerFetch(
      `${this.#baseUrl}/models`,
      { method: "GET", headers: this.#requestHeaders() },
      {
        provider: this.id,
        ...(this.#fetch ? { fetch: this.#fetch } : {}),
        ...(this.#apiCallLogger ? { apiCallLogger: this.#apiCallLogger } : {}),
      },
    );
    return normalizeModelInfo(await readJson(response));
  }

  async complete(request: ProviderCallRequest): Promise<CallResult> {
    const response = await providerFetch(
      `${this.#baseUrl}/chat/completions`,
      {
        method: "POST",
        headers: this.#requestHeaders(),
        body: JSON.stringify(requestBody(
          request,
          false,
          this.id,
          this.#useContextSessionId,
          this.#useContentCacheBreakpoints,
        )),
      },
      {
        provider: this.id,
        ...(this.#fetch ? { fetch: this.#fetch } : {}),
        ...(this.#apiCallLogger ? { apiCallLogger: this.#apiCallLogger } : {}),
        ...(request.timeoutMs !== undefined ? { timeoutMs: request.timeoutMs } : {}),
        ...(request.retry ? { retry: request.retry } : {}),
        ...(request.signal ? { signal: request.signal } : {}),
      },
    );
    const transportAttempts = providerFetchAttempts(response);

    const raw = await readJson<any>(response);
    const choice = raw.choices?.[0];
    const message = choice?.message ?? {};
    const toolCalls: ToolCall[] = (message.tool_calls ?? []).map((tool: any, index: number) =>
      normalizeToolCall(tool.id, tool.function?.name, safeJsonParse(tool.function?.arguments ?? ""), index),
    );

    const normalizedUsage = usage(
      raw.usage?.prompt_tokens,
      raw.usage?.completion_tokens,
      raw.usage?.total_tokens,
      raw.usage?.prompt_tokens_details?.cached_tokens,
      raw.usage?.prompt_tokens_details?.cache_write_tokens,
      reasoningTokenCount(raw.usage),
      transportAttempts,
    );
    const normalizedReasoning = reasoningText(message);
    const normalizedReasoningSummary = reasoningSummary(message);
    return {
      provider: this.id,
      model: raw.model ?? request.model,
      ...(raw.id ? { id: raw.id } : {}),
      text: typeof message.content === "string" ? message.content : "",
      ...(normalizedReasoning ? { reasoning: normalizedReasoning } : {}),
      ...(normalizedReasoningSummary ? { reasoningSummary: normalizedReasoningSummary } : {}),
      toolCalls,
      finishReason: normalizeFinishReason(choice?.finish_reason),
      ...(normalizedUsage ? { usage: normalizedUsage } : {}),
      raw,
    };
  }

  async *stream(request: ProviderCallRequest): AsyncIterable<StreamEvent> {
    const response = await providerFetch(
      `${this.#baseUrl}/chat/completions`,
      {
        method: "POST",
        headers: this.#requestHeaders(),
        body: JSON.stringify(requestBody(
          request,
          true,
          this.id,
          this.#useContextSessionId,
          this.#useContentCacheBreakpoints,
        )),
      },
      {
        provider: this.id,
        ...(this.#fetch ? { fetch: this.#fetch } : {}),
        ...(this.#apiCallLogger ? { apiCallLogger: this.#apiCallLogger } : {}),
        ...(request.timeoutMs !== undefined ? { timeoutMs: request.timeoutMs } : {}),
        ...(request.retry ? { retry: request.retry } : {}),
        ...(request.signal ? { signal: request.signal } : {}),
      },
    );
    const transportAttempts = providerFetchAttempts(response);

    let started = false;
    let finishReason = normalizeFinishReason(undefined);
    let finalUsage: ReturnType<typeof usage>;
    const tools = new Map<number, { id?: string; name?: string; argumentsText: string }>();

    for await (const message of parseSSE(response)) {
      if (message.data === "[DONE]") break;
      let raw: any;
      try {
        raw = JSON.parse(message.data);
      } catch {
        continue;
      }

      if (!started) {
        started = true;
        yield {
          type: "start",
          provider: this.id,
          model: raw.model ?? request.model,
          ...(raw.id ? { id: raw.id } : {}),
        };
      }

      if (raw.usage) {
        finalUsage = usage(
          raw.usage.prompt_tokens,
          raw.usage.completion_tokens,
          raw.usage.total_tokens,
          raw.usage.prompt_tokens_details?.cached_tokens,
          raw.usage.prompt_tokens_details?.cache_write_tokens,
          reasoningTokenCount(raw.usage),
          transportAttempts,
        );
      }

      const choice = raw.choices?.[0];
      if (!choice) continue;
      if (choice.finish_reason != null) finishReason = normalizeFinishReason(choice.finish_reason);
      const delta = choice.delta ?? {};
      const reasoning = reasoningText(delta);
      if (reasoning) {
        yield { type: "reasoning-delta", delta: reasoning };
      }
      const summary = reasoningSummary(delta);
      if (summary) {
        yield { type: "reasoning-summary-delta", delta: summary };
      }
      if (typeof delta.content === "string" && delta.content) {
        yield { type: "text-delta", delta: delta.content };
      }

      for (const toolDelta of delta.tool_calls ?? []) {
        const index = toolDelta.index ?? 0;
        const current = tools.get(index) ?? { argumentsText: "" };
        if (toolDelta.id) current.id = toolDelta.id;
        if (toolDelta.function?.name) current.name = toolDelta.function.name;
        if (toolDelta.function?.arguments) current.argumentsText += toolDelta.function.arguments;
        tools.set(index, current);
        yield {
          type: "tool-call-delta",
          index,
          ...(toolDelta.id ? { id: toolDelta.id } : {}),
          ...(toolDelta.function?.name ? { name: toolDelta.function.name } : {}),
          ...(toolDelta.function?.arguments ? { argumentsDelta: toolDelta.function.arguments } : {}),
        };
      }
    }

    for (const [index, tool] of [...tools.entries()].sort(([a], [b]) => a - b)) {
      yield {
        type: "tool-call",
        index,
        toolCall: normalizeToolCall(tool.id, tool.name, safeJsonParse(tool.argumentsText), index),
      };
    }

    yield {
      type: "finish",
      finishReason,
      ...(finalUsage ? { usage: finalUsage } : {}),
    };
  }
}
