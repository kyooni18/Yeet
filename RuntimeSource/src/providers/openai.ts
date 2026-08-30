import { providerFetch, readJson } from "../http.js";
import { parseSSE } from "../sse.js";
import type {
  ProviderCallRequest,
  CallResult,
  FetchLike,
  Message,
  ProviderAdapter,
  StreamEvent,
  ToolChoice,
} from "../types.js";
import { normalizeFinishReason, normalizeModelIds, normalizeToolCall, safeJsonParse, splitSystem, usage } from "../util.js";

export interface OpenAIProviderOptions {
  apiKey?: string;
  baseUrl?: string;
  organization?: string;
  project?: string;
  fetch?: FetchLike;
}

function mapToolChoice(choice: ToolChoice | undefined): unknown {
  if (!choice) return undefined;
  if (typeof choice === "string") return choice;
  return { type: "function", name: choice.name };
}

function mapInput(messages: Message[]): unknown[] {
  const input: any[] = [];
  for (const message of messages) {
    if (message.role === "tool") {
      input.push({
        type: "function_call_output",
        call_id: message.toolCallId ?? "",
        output: message.content ?? "",
      });
      continue;
    }

    if (message.content) {
      input.push({ role: message.role, content: message.content });
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

function bodyFor(request: ProviderCallRequest, stream: boolean): Record<string, unknown> {
  const split = splitSystem(request.messages, request.system);
  const toolChoice = mapToolChoice(request.toolChoice);
  return {
    ...(request.providerOptions ?? {}),
    model: request.model,
    input: mapInput(split.messages),
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
          })),
        }
      : {}),
    ...(toolChoice !== undefined ? { tool_choice: toolChoice } : {}),
    ...(request.metadata ? { metadata: request.metadata } : {}),
  };
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

export class OpenAIProvider implements ProviderAdapter {
  readonly id = "openai";
  readonly #apiKey: string | undefined;
  readonly #baseUrl: string;
  readonly #organization: string | undefined;
  readonly #project: string | undefined;
  readonly #fetch: FetchLike | undefined;

  constructor(options: OpenAIProviderOptions = {}) {
    this.#apiKey = options.apiKey;
    this.#baseUrl = (options.baseUrl ?? "https://api.openai.com/v1").replace(/\/$/, "");
    this.#organization = options.organization;
    this.#project = options.project;
    this.#fetch = options.fetch;
  }

  #headers(): Record<string, string> {
    if (!this.#apiKey) throw new Error("Missing API key for openai");
    return {
      authorization: `Bearer ${this.#apiKey}`,
      "content-type": "application/json",
      ...(this.#organization ? { "OpenAI-Organization": this.#organization } : {}),
      ...(this.#project ? { "OpenAI-Project": this.#project } : {}),
    };
  }

  async listModels(): Promise<string[]> {
    const response = await providerFetch(
      `${this.#baseUrl}/models`,
      { method: "GET", headers: this.#headers() },
      {
        provider: this.id,
        ...(this.#fetch ? { fetch: this.#fetch } : {}),
      },
    );
    return normalizeModelIds(await readJson(response));
  }

  async complete(request: ProviderCallRequest): Promise<CallResult> {
    const response = await providerFetch(
      `${this.#baseUrl}/responses`,
      { method: "POST", headers: this.#headers(), body: JSON.stringify(bodyFor(request, false)) },
      {
        provider: this.id,
        ...(this.#fetch ? { fetch: this.#fetch } : {}),
        ...(request.timeoutMs !== undefined ? { timeoutMs: request.timeoutMs } : {}),
        ...(request.retry ? { retry: request.retry } : {}),
        ...(request.signal ? { signal: request.signal } : {}),
      },
    );
    const raw = await readJson<any>(response);
    const toolCalls = (raw.output ?? [])
      .filter((item: any) => item.type === "function_call")
      .map((item: any, index: number) =>
        normalizeToolCall(item.call_id ?? item.id, item.name, safeJsonParse(item.arguments ?? ""), index),
      );
    const normalizedUsage = usage(
      raw.usage?.input_tokens,
      raw.usage?.output_tokens,
      raw.usage?.total_tokens,
      raw.usage?.input_tokens_details?.cached_tokens,
    );

    return {
      provider: this.id,
      model: raw.model ?? request.model,
      ...(raw.id ? { id: raw.id } : {}),
      text: outputText(raw),
      toolCalls,
      finishReason: finishReason(raw),
      ...(normalizedUsage ? { usage: normalizedUsage } : {}),
      raw,
    };
  }

  async *stream(request: ProviderCallRequest): AsyncIterable<StreamEvent> {
    const response = await providerFetch(
      `${this.#baseUrl}/responses`,
      { method: "POST", headers: this.#headers(), body: JSON.stringify(bodyFor(request, true)) },
      {
        provider: this.id,
        ...(this.#fetch ? { fetch: this.#fetch } : {}),
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
        )
      : undefined;
    yield {
      type: "finish",
      finishReason: completedRaw ? finishReason(completedRaw) : tools.size > 0 ? "tool_call" : "unknown",
      ...(normalizedUsage ? { usage: normalizedUsage } : {}),
      ...(completedRaw ? { raw: completedRaw } : {}),
    };
  }
}
