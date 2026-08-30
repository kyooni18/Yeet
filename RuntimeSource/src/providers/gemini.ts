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

export interface GeminiProviderOptions {
  apiKey?: string;
  accessToken?: string;
  projectId?: string;
  baseUrl?: string;
  fetch?: FetchLike;
}

function functionNameForToolResult(message: Message): string {
  if (message.name) return message.name;
  if (message.toolCallId?.startsWith("gemini:")) {
    return message.toolCallId.split(":", 3)[1] ?? "unknown";
  }
  return message.toolCallId ?? "unknown";
}

function mapContents(messages: Message[]): unknown[] {
  return messages.map((message) => {
    if (message.role === "tool") {
      const parsed = safeJsonParse(message.content ?? "");
      return {
        role: "user",
        parts: [
          {
            functionResponse: {
              name: functionNameForToolResult(message),
              response: typeof parsed === "object" && parsed !== null ? parsed : { result: parsed },
            },
          },
        ],
      };
    }

    if (message.role === "assistant") {
      const parts: unknown[] = [];
      if (message.content) parts.push({ text: message.content });
      for (const tool of message.toolCalls ?? []) {
        parts.push({ functionCall: { name: tool.name, args: tool.arguments } });
      }
      return { role: "model", parts };
    }

    return { role: "user", parts: [{ text: message.content ?? "" }] };
  });
}

function toolConfig(choice: ToolChoice | undefined): unknown {
  if (!choice || choice === "auto") return { functionCallingConfig: { mode: "AUTO" } };
  if (choice === "none") return { functionCallingConfig: { mode: "NONE" } };
  if (choice === "required") return { functionCallingConfig: { mode: "ANY" } };
  return { functionCallingConfig: { mode: "ANY", allowedFunctionNames: [choice.name] } };
}

function bodyFor(request: ProviderCallRequest): Record<string, unknown> {
  const split = splitSystem(request.messages, request.system);
  return {
    ...(request.providerOptions ?? {}),
    contents: mapContents(split.messages),
    ...(split.system ? { systemInstruction: { parts: [{ text: split.system }] } } : {}),
    ...(request.tools?.length
      ? {
          tools: [
            {
              functionDeclarations: request.tools.map((tool) => ({
                name: tool.name,
                ...(tool.description ? { description: tool.description } : {}),
                parameters: tool.inputSchema,
              })),
            },
          ],
          toolConfig: toolConfig(request.toolChoice),
        }
      : {}),
    ...(request.temperature !== undefined || request.maxTokens !== undefined
      ? {
          generationConfig: {
            ...(request.temperature !== undefined ? { temperature: request.temperature } : {}),
            ...(request.maxTokens !== undefined ? { maxOutputTokens: request.maxTokens } : {}),
          },
        }
      : {}),
  };
}

function geminiFinish(value: unknown, hasTools: boolean): ReturnType<typeof normalizeFinishReason> {
  if (hasTools) return "tool_call";
  if (value === "STOP") return "stop";
  if (value === "MAX_TOKENS") return "length";
  if (["SAFETY", "RECITATION", "PROHIBITED_CONTENT", "BLOCKLIST"].includes(String(value))) return "content_filter";
  return normalizeFinishReason(value);
}

function normalizeGeminiTool(part: any, index: number) {
  const name = part.functionCall?.name ?? "unknown";
  const id = part.functionCall?.id ?? `gemini:${name}:${index}`;
  return normalizeToolCall(id, name, part.functionCall?.args ?? {}, index);
}

export class GeminiProvider implements ProviderAdapter {
  readonly id = "gemini";
  readonly #apiKey: string | undefined;
  readonly #accessToken: string | undefined;
  readonly #projectId: string | undefined;
  readonly #baseUrl: string;
  readonly #fetch: FetchLike | undefined;

  constructor(options: GeminiProviderOptions = {}) {
    this.#apiKey = options.apiKey;
    this.#accessToken = options.accessToken;
    this.#projectId = options.projectId;
    this.#baseUrl = (options.baseUrl ?? "https://generativelanguage.googleapis.com/v1beta").replace(/\/$/, "");
    this.#fetch = options.fetch;
  }

  #headers(): Record<string, string> {
    if (this.#accessToken) {
      return {
        authorization: `Bearer ${this.#accessToken}`,
        "content-type": "application/json",
        ...(this.#projectId ? { "x-goog-user-project": this.#projectId } : {}),
      };
    }
    if (!this.#apiKey) throw new Error("Missing API key or OAuth token for gemini");
    return { "x-goog-api-key": this.#apiKey, "content-type": "application/json" };
  }

  #modelPath(model: string): string {
    return model.startsWith("models/") ? model.slice("models/".length) : model;
  }

  async listModels(): Promise<string[]> {
    const models: string[] = [];
    const seen = new Set<string>();
    let pageToken: string | undefined;

    do {
      const url = new URL(`${this.#baseUrl}/models`);
      url.searchParams.set("pageSize", "1000");
      if (pageToken) url.searchParams.set("pageToken", pageToken);
      const response = await providerFetch(
        url,
        { method: "GET", headers: this.#headers() },
        {
          provider: this.id,
          ...(this.#fetch ? { fetch: this.#fetch } : {}),
        },
      );
      const raw = await readJson<any>(response);
      for (const model of normalizeModelIds(raw)) {
        if (!seen.has(model)) {
          seen.add(model);
          models.push(model);
        }
      }
      const next = typeof raw.nextPageToken === "string" ? raw.nextPageToken : undefined;
      if (!next || next === pageToken) break;
      pageToken = next;
    } while (pageToken);

    return models;
  }

  async complete(request: ProviderCallRequest): Promise<CallResult> {
    const response = await providerFetch(
      `${this.#baseUrl}/models/${encodeURIComponent(this.#modelPath(request.model))}:generateContent`,
      { method: "POST", headers: this.#headers(), body: JSON.stringify(bodyFor(request)) },
      {
        provider: this.id,
        ...(this.#fetch ? { fetch: this.#fetch } : {}),
        ...(request.timeoutMs !== undefined ? { timeoutMs: request.timeoutMs } : {}),
        ...(request.retry ? { retry: request.retry } : {}),
        ...(request.signal ? { signal: request.signal } : {}),
      },
    );
    const raw = await readJson<any>(response);
    const candidate = raw.candidates?.[0] ?? {};
    const parts = candidate.content?.parts ?? [];
    const toolCalls = parts
      .filter((part: any) => part.functionCall)
      .map((part: any, index: number) => normalizeGeminiTool(part, index));
    const normalizedUsage = usage(
      raw.usageMetadata?.promptTokenCount,
      raw.usageMetadata?.candidatesTokenCount,
      raw.usageMetadata?.totalTokenCount,
      raw.usageMetadata?.cachedContentTokenCount,
    );

    return {
      provider: this.id,
      model: request.model,
      text: parts.filter((part: any) => typeof part.text === "string").map((part: any) => part.text).join(""),
      toolCalls,
      finishReason: geminiFinish(candidate.finishReason, toolCalls.length > 0),
      ...(normalizedUsage ? { usage: normalizedUsage } : {}),
      raw,
    };
  }

  async *stream(request: ProviderCallRequest): AsyncIterable<StreamEvent> {
    const response = await providerFetch(
      `${this.#baseUrl}/models/${encodeURIComponent(this.#modelPath(request.model))}:streamGenerateContent?alt=sse`,
      { method: "POST", headers: this.#headers(), body: JSON.stringify(bodyFor(request)) },
      {
        provider: this.id,
        ...(this.#fetch ? { fetch: this.#fetch } : {}),
        ...(request.timeoutMs !== undefined ? { timeoutMs: request.timeoutMs } : {}),
        ...(request.retry ? { retry: request.retry } : {}),
        ...(request.signal ? { signal: request.signal } : {}),
      },
    );

    yield { type: "start", provider: this.id, model: request.model };
    let finish = "unknown" as ReturnType<typeof normalizeFinishReason>;
    let finalUsage: ReturnType<typeof usage>;
    let toolIndex = 0;

    for await (const event of parseSSE(response)) {
      let raw: any;
      try {
        raw = JSON.parse(event.data);
      } catch {
        continue;
      }
      const candidate = raw.candidates?.[0];
      if (raw.usageMetadata) {
        finalUsage = usage(
          raw.usageMetadata.promptTokenCount,
          raw.usageMetadata.candidatesTokenCount,
          raw.usageMetadata.totalTokenCount,
          raw.usageMetadata.cachedContentTokenCount,
        );
      }
      if (!candidate) continue;

      const parts = candidate.content?.parts ?? [];
      let hasTools = false;
      for (const part of parts) {
        if (typeof part.text === "string" && part.text) {
          yield { type: "text-delta", delta: part.text };
        }
        if (part.functionCall) {
          hasTools = true;
          const index = toolIndex++;
          const toolCall = normalizeGeminiTool(part, index);
          yield { type: "tool-call-delta", index, id: toolCall.id, name: toolCall.name };
          yield { type: "tool-call", index, toolCall };
        }
      }
      if (candidate.finishReason) finish = geminiFinish(candidate.finishReason, hasTools || finish === "tool_call");
      else if (hasTools) finish = "tool_call";
    }

    yield { type: "finish", finishReason: finish, ...(finalUsage ? { usage: finalUsage } : {}) };
  }
}
