import { providerFetch, readJson } from "../http.js";
import type { ProviderFetchLogger } from "../http.js";
import { parseSSE } from "../sse.js";
import type {
  ProviderCallRequest,
  CallResult,
  FetchLike,
  Message,
  ModelInfo,
  ProviderAdapter,
  StreamEvent,
  ToolChoice,
} from "../types.js";
import { normalizeFinishReason, normalizeModelInfo, normalizeToolCall, safeJsonParse, splitLeadingSystem, toolResultContent, usage } from "../util.js";

export interface AnthropicProviderOptions {
  id?: string;
  apiKey?: string;
  baseUrl?: string;
  version?: string;
  fetch?: FetchLike;
  apiCallLogger?: ProviderFetchLogger;
}

function mapToolChoice(choice: ToolChoice | undefined): unknown {
  if (!choice || choice === "auto") return { type: "auto" };
  if (choice === "none") return { type: "none" };
  if (choice === "required") return { type: "any" };
  return { type: "tool", name: choice.name };
}

function mapMessages(messages: Message[]): unknown[] {
  const output: any[] = [];
  for (const message of messages) {
    if (message.role === "tool") {
      const block = {
        type: "tool_result",
        tool_use_id: message.toolCallId ?? message.toolResult?.toolCallId ?? "",
        content: toolResultContent(message),
        ...(message.toolResult?.isError !== undefined ? { is_error: message.toolResult.isError } : {}),
      };
      const last = output.at(-1);
      if (last?.role === "user" && Array.isArray(last.content)) last.content.push(block);
      else output.push({ role: "user", content: [block] });
      continue;
    }

    if (message.role === "assistant") {
      const content: any[] = [];
      if (message.content) content.push({ type: "text", text: message.content });
      for (const tool of message.toolCalls ?? []) {
        content.push({ type: "tool_use", id: tool.id, name: tool.name, input: tool.arguments });
      }
      output.push({ role: "assistant", content });
      continue;
    }

    if (message.images?.length) {
      output.push({
        role: "user",
        content: [
          ...(message.content ? [{ type: "text", text: message.content }] : []),
          ...message.images.map((image) => ({
            type: "image",
            source: { type: "base64", media_type: image.mediaType, data: image.data },
          })),
        ],
      });
    } else {
      output.push({ role: "user", content: message.content ?? "" });
    }
  }
  return output;
}

function requestBody(request: ProviderCallRequest, stream: boolean): Record<string, unknown> {
  // Only the immutable leading system prefix belongs in Anthropic's top-level
  // system field. Request-local coordinator overlays intentionally remain at
  // the tail, where mapMessages represents them as user guidance. Hoisting
  // those volatile overlays invalidates the provider prompt prefix every
  // agent round.
  const split = splitLeadingSystem(request.messages, request.system);
  const requestedCacheControl = request.providerOptions?.cache_control;
  return {
    ...(request.providerOptions ?? {}),
    model: request.model,
    max_tokens: request.maxTokens ?? 4_096,
    messages: mapMessages(split.messages),
    stream,
    ...(split.system ? { system: split.system } : {}),
    ...(request.temperature !== undefined ? { temperature: request.temperature } : {}),
    ...(request.tools?.length
      ? {
          tools: request.tools.map((tool) => ({
            name: tool.name,
            ...(tool.description ? { description: tool.description } : {}),
            input_schema: tool.inputSchema,
          })),
        }
      : {}),
    ...(request.toolChoice ? { tool_choice: mapToolChoice(request.toolChoice) } : {}),
    ...(request.metadata ? { metadata: request.metadata } : {}),
    cache_control: requestedCacheControl ?? { type: "ephemeral" },
  };
}

function cacheCreationTokens(value: any): number | undefined {
  if (typeof value?.cache_creation_input_tokens === "number") return value.cache_creation_input_tokens;
  const creation = value?.cache_creation;
  if (!creation || typeof creation !== "object") return undefined;
  const fiveMinute = typeof creation.ephemeral_5m_input_tokens === "number" ? creation.ephemeral_5m_input_tokens : 0;
  const oneHour = typeof creation.ephemeral_1h_input_tokens === "number" ? creation.ephemeral_1h_input_tokens : 0;
  return fiveMinute + oneHour || undefined;
}

export class AnthropicProvider implements ProviderAdapter {
  readonly id: string;
  readonly #apiKey: string | undefined;
  readonly #baseUrl: string;
  readonly #version: string;
  readonly #fetch: FetchLike | undefined;
  readonly #apiCallLogger: ProviderFetchLogger | undefined;

  constructor(options: AnthropicProviderOptions = {}) {
    this.id = options.id ?? "anthropic";
    this.#apiKey = options.apiKey;
    this.#baseUrl = (options.baseUrl ?? "https://api.anthropic.com").replace(/\/$/, "");
    this.#version = options.version ?? "2023-06-01";
    this.#fetch = options.fetch;
    this.#apiCallLogger = options.apiCallLogger;
  }

  #headers(): Record<string, string> {
    if (!this.#apiKey) throw new Error(`Missing API key for ${this.id}`);
    return {
      "x-api-key": this.#apiKey,
      "anthropic-version": this.#version,
      "content-type": "application/json",
    };
  }

  async listModels(): Promise<string[]> {
    return (await this.listModelInfo()).map((model) => model.id);
  }

  async listModelInfo(): Promise<ModelInfo[]> {
    const models: ModelInfo[] = [];
    const seen = new Set<string>();
    let afterId: string | undefined;

    do {
      const url = new URL(`${this.#baseUrl}/v1/models`);
      if (afterId) url.searchParams.set("after_id", afterId);
      const response = await providerFetch(
        url,
        { method: "GET", headers: this.#headers() },
        {
          provider: this.id,
          ...(this.#fetch ? { fetch: this.#fetch } : {}),
          ...(this.#apiCallLogger ? { apiCallLogger: this.#apiCallLogger } : {}),
        },
      );
      const raw = await readJson<any>(response);
      for (const model of normalizeModelInfo(raw)) {
        if (!seen.has(model.id)) {
          seen.add(model.id);
          models.push(model);
        }
      }
      const next = raw.has_more && typeof raw.last_id === "string" ? raw.last_id : undefined;
      if (!next || next === afterId) break;
      afterId = next;
    } while (afterId);

    return models;
  }

  async complete(request: ProviderCallRequest): Promise<CallResult> {
    const response = await providerFetch(
      `${this.#baseUrl}/v1/messages`,
      { method: "POST", headers: this.#headers(), body: JSON.stringify(requestBody(request, false)) },
      {
        provider: this.id,
        ...(this.#fetch ? { fetch: this.#fetch } : {}),
        ...(this.#apiCallLogger ? { apiCallLogger: this.#apiCallLogger } : {}),
        ...(request.timeoutMs !== undefined ? { timeoutMs: request.timeoutMs } : {}),
        ...(request.retry ? { retry: request.retry } : {}),
        ...(request.signal ? { signal: request.signal } : {}),
      },
    );
    const raw = await readJson<any>(response);
    const text = (raw.content ?? []).filter((b: any) => b.type === "text").map((b: any) => b.text).join("");
    const reasoning = (raw.content ?? [])
      .filter((b: any) => b.type === "thinking" && typeof b.thinking === "string")
      .map((b: any) => b.thinking)
      .join("");
    const reasoningSummary = (raw.content ?? [])
      .filter((b: any) => b.type === "thinking" && typeof b.summary === "string")
      .map((b: any) => b.summary)
      .join("");
    const toolCalls = (raw.content ?? [])
      .filter((b: any) => b.type === "tool_use")
      .map((b: any, index: number) => normalizeToolCall(b.id, b.name, b.input, index));
    const normalizedUsage = usage(
      raw.usage?.input_tokens,
      raw.usage?.output_tokens,
      undefined,
      raw.usage?.cache_read_input_tokens,
      cacheCreationTokens(raw.usage),
      raw.usage?.output_tokens_details?.thinking_tokens,
    );

    return {
      provider: this.id,
      model: raw.model ?? request.model,
      ...(raw.id ? { id: raw.id } : {}),
      text,
      ...(reasoning ? { reasoning } : {}),
      ...(reasoningSummary ? { reasoningSummary } : {}),
      toolCalls,
      finishReason: normalizeFinishReason(raw.stop_reason),
      ...(normalizedUsage ? { usage: normalizedUsage } : {}),
      raw,
    };
  }

  async *stream(request: ProviderCallRequest): AsyncIterable<StreamEvent> {
    const response = await providerFetch(
      `${this.#baseUrl}/v1/messages`,
      { method: "POST", headers: this.#headers(), body: JSON.stringify(requestBody(request, true)) },
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
    let model = request.model;
    let id: string | undefined;
    let finishReason = normalizeFinishReason(undefined);
    let inputTokens: number | undefined;
    let outputTokens: number | undefined;
    let cachedInputTokens: number | undefined;
    let cacheWriteInputTokens: number | undefined;
    let reasoningTokens: number | undefined;
    const tools = new Map<number, { id?: string; name?: string; json: string }>();

    for await (const event of parseSSE(response)) {
      let raw: any;
      try {
        raw = JSON.parse(event.data);
      } catch {
        continue;
      }

      if (raw.type === "message_start") {
        model = raw.message?.model ?? model;
        id = raw.message?.id ?? id;
        inputTokens = raw.message?.usage?.input_tokens ?? inputTokens;
        cachedInputTokens = raw.message?.usage?.cache_read_input_tokens ?? cachedInputTokens;
        cacheWriteInputTokens = cacheCreationTokens(raw.message?.usage) ?? cacheWriteInputTokens;
        reasoningTokens = raw.message?.usage?.output_tokens_details?.thinking_tokens ?? reasoningTokens;
        if (!started) {
          started = true;
          yield { type: "start", provider: this.id, model, ...(id ? { id } : {}) };
        }
        continue;
      }

      if (!started) {
        started = true;
        yield { type: "start", provider: this.id, model };
      }

      if (raw.type === "content_block_start" && raw.content_block?.type === "tool_use") {
        const index = raw.index ?? 0;
        tools.set(index, {
          id: raw.content_block.id,
          name: raw.content_block.name,
          json: raw.content_block.input && Object.keys(raw.content_block.input).length
            ? JSON.stringify(raw.content_block.input)
            : "",
        });
        yield {
          type: "tool-call-delta",
          index,
          ...(raw.content_block.id ? { id: raw.content_block.id } : {}),
          ...(raw.content_block.name ? { name: raw.content_block.name } : {}),
        };
      } else if (raw.type === "content_block_delta" && raw.delta?.type === "text_delta") {
        if (raw.delta.text) yield { type: "text-delta", delta: raw.delta.text };
      } else if (raw.type === "content_block_delta" && raw.delta?.type === "thinking_delta") {
        if (raw.delta.thinking) yield { type: "reasoning-delta", delta: raw.delta.thinking };
      } else if (raw.type === "content_block_delta" && raw.delta?.type === "thinking_summary_delta") {
        if (raw.delta.summary) yield { type: "reasoning-summary-delta", delta: raw.delta.summary };
      } else if (raw.type === "content_block_delta" && raw.delta?.type === "input_json_delta") {
        const index = raw.index ?? 0;
        const current = tools.get(index) ?? { json: "" };
        current.json += raw.delta.partial_json ?? "";
        tools.set(index, current);
        if (raw.delta.partial_json) {
          yield { type: "tool-call-delta", index, argumentsDelta: raw.delta.partial_json };
        }
      } else if (raw.type === "message_delta") {
        finishReason = normalizeFinishReason(raw.delta?.stop_reason);
        outputTokens = raw.usage?.output_tokens ?? outputTokens;
        reasoningTokens = raw.usage?.output_tokens_details?.thinking_tokens ?? reasoningTokens;
      }
    }

    for (const [index, tool] of [...tools.entries()].sort(([a], [b]) => a - b)) {
      yield {
        type: "tool-call",
        index,
        toolCall: normalizeToolCall(tool.id, tool.name, safeJsonParse(tool.json), index),
      };
    }

    const normalizedUsage = usage(
      inputTokens,
      outputTokens,
      undefined,
      cachedInputTokens,
      cacheWriteInputTokens,
      reasoningTokens,
    );
    yield {
      type: "finish",
      finishReason,
      ...(normalizedUsage ? { usage: normalizedUsage } : {}),
    };
  }
}
