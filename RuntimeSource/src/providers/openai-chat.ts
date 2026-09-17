import { fetchEmbeddings } from "../embeddings.js";
import type { EmbeddingRequest, EmbeddingResult } from "../types.js";
import { createHash } from "node:crypto";
import { providerFetch, providerFetchAttempts, readJson } from "../http.js";
import { ProviderHTTPError } from "../errors.js";
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
  excludedModels?: string[];
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
      useContentCacheBreakpoints
        && cacheCapabilities.modes.includes("explicit")
        && request.promptCache !== false,
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

function isUnsupportedParameterError(error: unknown): error is ProviderHTTPError {
  if (!(error instanceof ProviderHTTPError) || error.status !== 400) return false;
  try {
    return JSON.parse(error.responseBody ?? "")?.error?.code === "unsupported_parameter";
  } catch {
    return /unsupported[_ ]parameter/i.test(error.responseBody ?? "");
  }
}

function strictToolProtocolInstruction(tools: ToolDefinition[] | undefined, choice: ToolChoice | undefined): string | undefined {
  if (!tools?.length || choice === "none") return undefined;
  const available = typeof choice === "object" ? tools.filter((tool) => tool.name === choice.name) : tools;
  if (!available.length) return undefined;
  const schemas = available.map((tool) => ({
    name: tool.name,
    ...(tool.description ? { description: tool.description } : {}),
    input_schema: tool.inputSchema,
  }));
  const required = choice === "required" || typeof choice === "object";
  return [
    "Yeet tool protocol. Return exactly one JSON object and nothing else.",
    `Available tools: ${JSON.stringify(schemas)}`,
    required
      ? "You must use an available tool before giving a final answer."
      : "Use tools whenever external inspection or action is needed; otherwise finish the task directly.",
    'For tool use: {"tool_calls":[{"name":"tool_name","arguments":{"key":"value"}}]}',
    'For a completed answer: {"final":"your answer"}',
    'Use exactly one of "tool_calls" or "final". Do not include call IDs; Yeet creates and tracks them.',
    "You may put multiple independent calls in tool_calls. Each arguments value must be one complete JSON object matching the tool input_schema.",
    'Tool results arrive in later user messages as {"tool_result":{"name":"tool_name","content":"..."}}. Never invent a tool result.',
    "Do not wrap the JSON in Markdown or add prose outside it.",
  ].join("\n");
}

function strictCompatibilityMessages(messages: Message[]): { messages: Message[]; system?: string } {
  const systemParts: string[] = [];
  const compatible: Message[] = [];
  for (const message of messages) {
    if (message.role === "system") {
      if (message.content) systemParts.push(message.content);
      continue;
    }
    if (message.role === "tool") {
      compatible.push({
        role: "user",
        content: JSON.stringify({
          tool_result: {
            ...(message.name ? { name: message.name } : {}),
            content: toolResultContent(message),
          },
        }),
      });
      continue;
    }
    if (message.role === "assistant" && message.toolCalls?.length) {
      compatible.push({
        role: "assistant",
        content: JSON.stringify({
          tool_calls: message.toolCalls.map((tool) => ({
            name: tool.name,
            arguments: tool.arguments,
          })),
        }),
      });
      continue;
    }
    compatible.push({
      role: message.role,
      content: message.content ?? "",
      ...(message.images?.length ? { images: message.images } : {}),
    });
  }
  return {
    messages: compatible,
    ...(systemParts.length ? { system: systemParts.join("\n\n") } : {}),
  };
}

function jsonObjectText(text: string): string {
  const trimmed = text.trim();
  const fenced = /^```(?:json)?\s*([\s\S]*?)\s*```$/i.exec(trimmed);
  return fenced?.[1]?.trim() ?? trimmed;
}

function strictCompatibilityJsonResponse(
  text: string,
  responseId?: string,
): { recognized: boolean; text: string; toolCalls: ToolCall[] } {
  let value: unknown;
  try {
    value = JSON.parse(jsonObjectText(text));
  } catch {
    return { recognized: false, text, toolCalls: [] };
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return { recognized: false, text, toolCalls: [] };
  }
  const envelope = value as Record<string, unknown>;
  if (Array.isArray(envelope.tool_calls)) {
    const toolCalls = envelope.tool_calls.flatMap((raw, index) => {
      if (!raw || typeof raw !== "object" || Array.isArray(raw)) return [];
      const call = raw as Record<string, unknown>;
      if (typeof call.name !== "string" || !call.name.trim()) return [];
      let args = call.arguments ?? {};
      if (typeof args === "string") {
        try { args = JSON.parse(args); }
        catch { return []; }
      }
      if (!args || typeof args !== "object" || Array.isArray(args)) return [];
      return [{
        id: `${responseId ?? "compat"}-tool-${index}`,
        name: call.name,
        arguments: args,
      } satisfies ToolCall];
    });
    if (toolCalls.length > 0) return { recognized: true, text: "", toolCalls };
    if (typeof envelope.final !== "string") return { recognized: true, text: "", toolCalls: [] };
  }
  if (typeof envelope.final === "string") {
    return { recognized: true, text: envelope.final, toolCalls: [] };
  }
  return { recognized: false, text, toolCalls: [] };
}

function legacyStrictCompatibilityToolCalls(text: string, responseId?: string): ToolCall[] {
  const calls: ToolCall[] = [];
  const pattern = /<tool_call>\s*([\s\S]*?)\s*<\/tool_call>/g;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(text)) !== null) {
    try {
      const value = JSON.parse(match[1] ?? "") as Record<string, unknown>;
      const nested = value.function && typeof value.function === "object" && !Array.isArray(value.function)
        ? value.function as Record<string, unknown>
        : undefined;
      const name = typeof value.name === "string" ? value.name : typeof nested?.name === "string" ? nested.name : undefined;
      const rawArguments = value.arguments ?? nested?.arguments ?? {};
      const parsedArguments = typeof rawArguments === "string" ? JSON.parse(rawArguments) : rawArguments;
      if (!name || !parsedArguments || typeof parsedArguments !== "object" || Array.isArray(parsedArguments)) continue;
      calls.push({
        id: typeof value.id === "string" && value.id ? value.id : `${responseId ?? "compat"}-tool-${calls.length}`,
        name,
        arguments: parsedArguments,
      });
    } catch {
      // Keep malformed legacy output as text so the agent repair path can handle it.
    }
  }
  return calls;
}

function strictCompatibilityResponse(text: string, responseId?: string): { text: string; toolCalls: ToolCall[] } {
  const structured = strictCompatibilityJsonResponse(text, responseId);
  if (structured.recognized) return { text: structured.text, toolCalls: structured.toolCalls };
  const legacy = legacyStrictCompatibilityToolCalls(text, responseId);
  return { text: legacy.length ? "" : text, toolCalls: legacy };
}

function strictCompatibilityRequest(request: ProviderCallRequest): ProviderCallRequest {
  const {
    tools,
    deferredTools: _deferredTools,
    toolChoice,
    temperature: _temperature,
    maxTokens: _maxTokens,
    metadata: _metadata,
    providerOptions: _providerOptions,
    promptCache: _promptCache,
    ...compatible
  } = request;
  const toolProtocol = strictToolProtocolInstruction(tools, toolChoice);
  const mapped = strictCompatibilityMessages(compatible.messages);
  const system = [compatible.system, mapped.system, toolProtocol]
    .filter((value): value is string => Boolean(value))
    .join("\n\n");
  return {
    ...compatible,
    messages: mapped.messages,
    ...(system ? { system } : {}),
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
  readonly #excludedModels: Set<string>;
  readonly #strictCompatibilityModels = new Set<string>();

  constructor(options: OpenAIChatProviderOptions = {}) {
    this.id = options.id ?? "openai-compatible";
    this.#apiKey = options.apiKey;
    this.#baseUrl = (options.baseUrl ?? "https://api.openai.com/v1").replace(/\/$/, "");
    this.#headers = options.headers ?? {};
    this.#requireApiKey = options.requireApiKey ?? false;
    this.#useContextSessionId = options.useContextSessionId ?? false;
    this.#useContentCacheBreakpoints = options.useContentCacheBreakpoints ?? false;
    this.#excludedModels = new Set((options.excludedModels ?? []).map((model) => model.trim()).filter(Boolean));
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
    return normalizeModelInfo(await readJson(response))
      .filter((model) => !this.#excludedModels.has(model.id));
  }

  async complete(request: ProviderCallRequest): Promise<CallResult> {
    return this.#complete(request, this.#strictCompatibilityModels.has(request.model));
  }

  async #complete(request: ProviderCallRequest, strictCompatibility: boolean): Promise<CallResult> {
    const send = (candidate: ProviderCallRequest) => providerFetch(
      `${this.#baseUrl}/chat/completions`,
      {
        method: "POST",
        headers: this.#requestHeaders(),
        body: JSON.stringify(requestBody(
          candidate,
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
        ...(candidate.timeoutMs !== undefined ? { timeoutMs: candidate.timeoutMs } : {}),
        ...(candidate.retry ? { retry: candidate.retry } : {}),
        ...(candidate.signal ? { signal: candidate.signal } : {}),
      },
    );
    let response: Response;
    let compatibilityAttempts = 0;
    let usedStrictCompatibility = strictCompatibility;
    try {
      response = await send(strictCompatibility ? strictCompatibilityRequest(request) : request);
    } catch (error) {
      if (strictCompatibility || !isUnsupportedParameterError(error)) throw error;
      compatibilityAttempts = 1;
      usedStrictCompatibility = true;
      response = await send(strictCompatibilityRequest(request));
    }
    if (usedStrictCompatibility) this.#strictCompatibilityModels.add(request.model);
    const transportAttempts = providerFetchAttempts(response) + compatibilityAttempts;

    const raw = await readJson<any>(response);
    const choice = raw.choices?.[0];
    const message = choice?.message ?? {};
    const content = typeof message.content === "string" ? message.content : "";
    const nativeToolCalls: ToolCall[] = (message.tool_calls ?? []).map((tool: any, index: number) =>
      normalizeToolCall(tool.id, tool.function?.name, safeJsonParse(tool.function?.arguments ?? ""), index),
    );
    const compatibility = usedStrictCompatibility
      ? strictCompatibilityResponse(content, typeof raw.id === "string" ? raw.id : undefined)
      : { text: content, toolCalls: [] as ToolCall[] };
    const toolCalls = nativeToolCalls.length ? nativeToolCalls : compatibility.toolCalls;

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
      text: nativeToolCalls.length ? content : compatibility.text,
      ...(normalizedReasoning ? { reasoning: normalizedReasoning } : {}),
      ...(normalizedReasoningSummary ? { reasoningSummary: normalizedReasoningSummary } : {}),
      toolCalls,
      finishReason: toolCalls.length ? "tool_call" : normalizeFinishReason(choice?.finish_reason),
      ...(normalizedUsage ? { usage: normalizedUsage } : {}),
      raw,
    };
  }

  async *#strictCompatibilityStream(request: ProviderCallRequest): AsyncIterable<StreamEvent> {
    const fallback = await this.#complete(request, true);
    yield {
      type: "start",
      provider: this.id,
      model: fallback.model,
      ...(fallback.id ? { id: fallback.id } : {}),
    };
    if (fallback.reasoning) yield { type: "reasoning-delta", delta: fallback.reasoning };
    if (fallback.reasoningSummary) yield { type: "reasoning-summary-delta", delta: fallback.reasoningSummary };
    if (fallback.text) yield { type: "text-delta", delta: fallback.text };
    for (const [index, toolCall] of fallback.toolCalls.entries()) {
      yield { type: "tool-call", index, toolCall };
    }
    yield {
      type: "finish",
      finishReason: fallback.finishReason,
      ...(fallback.usage ? { usage: fallback.usage } : {}),
      ...(fallback.raw !== undefined ? { raw: fallback.raw } : {}),
    };
  }

  async *stream(request: ProviderCallRequest): AsyncIterable<StreamEvent> {
    if (this.#strictCompatibilityModels.has(request.model)) {
      yield* this.#strictCompatibilityStream(request);
      return;
    }
    let response: Response;
    try {
      response = await providerFetch(
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
    } catch (error) {
      if (!isUnsupportedParameterError(error)) throw error;
      yield* this.#strictCompatibilityStream(request);
      return;
    }
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
