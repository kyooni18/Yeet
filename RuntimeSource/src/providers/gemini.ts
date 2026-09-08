import { fetchEmbeddings } from "../embeddings.js";
import type { EmbeddingRequest, EmbeddingResult } from "../types.js";
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

export interface GeminiProviderOptions {
  id?: string;
  apiKey?: string;
  accessToken?: string;
  projectId?: string;
  baseUrl?: string;
  fetch?: FetchLike;
  apiCallLogger?: ProviderFetchLogger;
}

function functionNameForToolResult(message: Message): string {
  if (message.name) return message.name;
  if (message.toolResult?.name) return message.toolResult.name;
  if (message.toolCallId?.startsWith("gemini:")) {
    return message.toolCallId.split(":", 3)[1] ?? "unknown";
  }
  return message.toolCallId ?? "unknown";
}

type GeminiFunctionMetadata = { providerId?: string; thoughtSignature?: string };

function mapContents(messages: Message[], metadata: Map<string, GeminiFunctionMetadata>): unknown[] {
  return messages.map((message) => {
    if (message.role === "tool") {
      const parsed = safeJsonParse(toolResultContent(message));
      const callMetadata = message.toolCallId ? metadata.get(message.toolCallId) : undefined;
      return {
        role: "user",
        parts: [
          {
            functionResponse: {
              name: functionNameForToolResult(message),
              ...(callMetadata?.providerId ? { id: callMetadata.providerId } : {}),
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
        const callMetadata = metadata.get(tool.id);
        parts.push({
          functionCall: {
            name: tool.name,
            ...(callMetadata?.providerId ? { id: callMetadata.providerId } : {}),
            args: tool.arguments,
          },
          ...(callMetadata?.thoughtSignature ? { thoughtSignature: callMetadata.thoughtSignature } : {}),
        });
      }
      return { role: "model", parts };
    }

    return {
      role: "user",
      parts: [
        ...(message.content ? [{ text: message.content }] : []),
        ...(message.images ?? []).map((image) => ({ inlineData: { mimeType: image.mediaType, data: image.data } })),
      ],
    };
  });
}

function toolConfig(choice: ToolChoice | undefined): unknown {
  if (!choice || choice === "auto") return { functionCallingConfig: { mode: "AUTO" } };
  if (choice === "none") return { functionCallingConfig: { mode: "NONE" } };
  if (choice === "required") return { functionCallingConfig: { mode: "ANY" } };
  return { functionCallingConfig: { mode: "ANY", allowedFunctionNames: [choice.name] } };
}

// Gemini's function-declaration schema is a deliberately smaller dialect than
// the provider-neutral JSON Schemas exposed by Yeet/MCP. In particular, nested
// oneOf/anyOf branches containing only `required` constraints are rejected by
// the API. Lower unions into a permissive object shape here; Yeet's local tool
// implementation remains the authority that validates the actual arguments.
const GEMINI_SCHEMA_KEYS = new Set([
  "type",
  "description",
  "enum",
  "format",
  "nullable",
  "properties",
  "required",
  "items",
  "minimum",
  "maximum",
  "minLength",
  "maxLength",
  "pattern",
  "minItems",
  "maxItems",
]);

function schemaType(value: any): string | undefined {
  if (typeof value?.type === "string") return value.type.toLowerCase();
  if (value?.properties && typeof value.properties === "object") return "object";
  return undefined;
}

function mergeGeminiSchemas(left: any, right: any): Record<string, unknown> {
  if (!left || Object.keys(left).length === 0) return { ...(right ?? {}) };
  if (!right || Object.keys(right).length === 0) return { ...left };
  const leftType = schemaType(left);
  const rightType = schemaType(right);
  if (leftType && rightType && leftType !== rightType) return {};

  const type = leftType ?? rightType;
  const merged: Record<string, any> = {};
  if (type) merged.type = type;
  const descriptions = [left.description, right.description].filter((value) => typeof value === "string" && value.trim());
  if (descriptions.length) merged.description = [...new Set(descriptions)].join(" ");
  const enumValues = [...(Array.isArray(left.enum) ? left.enum : []), ...(Array.isArray(right.enum) ? right.enum : [])];
  if (enumValues.length) merged.enum = [...new Set(enumValues.map((value) => JSON.stringify(value)))].map((value) => JSON.parse(value));

  if (type === "object") {
    const properties: Record<string, unknown> = {};
    const names = new Set([
      ...Object.keys(left.properties ?? {}),
      ...Object.keys(right.properties ?? {}),
    ]);
    for (const name of names) {
      const a = left.properties?.[name];
      const b = right.properties?.[name];
      properties[name] = a && b ? mergeGeminiSchemas(a, b) : (a ?? b);
    }
    if (Object.keys(properties).length) merged.properties = properties;
    const leftRequired = new Set(Array.isArray(left.required) ? left.required : []);
    const rightRequired = new Set(Array.isArray(right.required) ? right.required : []);
    const required = [...leftRequired].filter((name) => rightRequired.has(name));
    if (required.length) merged.required = required;
  } else if (type === "array" && left.items && right.items) {
    merged.items = mergeGeminiSchemas(left.items, right.items);
  } else if (type === "array") {
    merged.items = left.items ?? right.items;
  }
  return merged;
}

function lowerGeminiSchema(input: unknown): Record<string, unknown> {
  if (!input || typeof input !== "object" || Array.isArray(input)) return {};
  const source = input as Record<string, any>;
  const output: Record<string, any> = {};
  const inferredType = schemaType(source);
  if (inferredType) output.type = inferredType;
  if (typeof source.description === "string" && source.description.trim()) output.description = source.description;
  if (typeof source.format === "string") output.format = source.format;
  if (typeof source.nullable === "boolean") output.nullable = source.nullable;
  if (Array.isArray(source.enum)) output.enum = source.enum;
  if (Object.prototype.hasOwnProperty.call(source, "const")) output.enum = [source.const];
  for (const key of ["minimum", "maximum", "minLength", "maxLength", "minItems", "maxItems"]) {
    if (typeof source[key] === "number" && Number.isFinite(source[key])) output[key] = source[key];
  }
  if (typeof source.pattern === "string") output.pattern = source.pattern;
  if (source.properties && typeof source.properties === "object" && !Array.isArray(source.properties)) {
    output.properties = Object.fromEntries(
      Object.entries(source.properties).map(([name, value]) => [name, lowerGeminiSchema(value)]),
    );
  }
  if (source.items && typeof source.items === "object" && !Array.isArray(source.items)) {
    output.items = lowerGeminiSchema(source.items);
  }
  if (output.type === "object" && Array.isArray(source.required)) {
    const known = new Set(Object.keys(output.properties ?? {}));
    const required = source.required.filter((name: unknown) => typeof name === "string" && (!known.size || known.has(name)));
    if (required.length) output.required = [...new Set(required)];
  }

  const alternatives = Array.isArray(source.oneOf)
    ? source.oneOf
    : Array.isArray(source.anyOf)
      ? source.anyOf
      : undefined;
  if (alternatives?.length) {
    const lowered = alternatives.map(lowerGeminiSchema);
    // Required-only alternatives intentionally lower to `{}`. Their OR
    // constraint cannot be represented faithfully in Gemini and is enforced
    // by the local tool implementation after the model returns arguments.
    const meaningful = lowered.filter((value) => Object.keys(value).length > 0);
    if (meaningful.length) {
      const union = meaningful.reduce((left, right) => mergeGeminiSchemas(left, right));
      const baseRequired = Array.isArray(output.required) ? [...output.required] : [];
      const merged = mergeGeminiSchemas(output, union);
      Object.assign(output, merged);
      if (baseRequired.length && output.type === "object") {
        output.required = [...new Set([...(output.required ?? []), ...baseRequired])];
      }
    }
  }
  return output;
}

function assertGeminiSchema(schema: Record<string, any>, path = "parameters"): void {
  for (const key of Object.keys(schema)) {
    if (!GEMINI_SCHEMA_KEYS.has(key)) throw new Error(`Unsupported Gemini tool-schema key at ${path}: ${key}`);
  }
  if (schema.required && schemaType(schema) !== "object") {
    throw new Error(`Invalid Gemini tool schema at ${path}: required is only valid for object schemas`);
  }
  if (schema.properties) {
    if (schemaType(schema) !== "object") throw new Error(`Invalid Gemini tool schema at ${path}: properties require object type`);
    for (const [name, child] of Object.entries(schema.properties)) {
      assertGeminiSchema(child as Record<string, any>, `${path}.properties.${name}`);
    }
  }
  if (schema.items) assertGeminiSchema(schema.items as Record<string, any>, `${path}.items`);
}

function geminiToolSchema(input: Record<string, unknown>): Record<string, unknown> {
  const schema = lowerGeminiSchema(input);
  if (schemaType(schema) !== "object") {
    if (Object.keys(schema).length === 0) return { type: "object", properties: {} };
    throw new Error("Gemini function parameters must use an object input schema");
  }
  assertGeminiSchema(schema);
  return schema;
}

function bodyFor(
  request: ProviderCallRequest,
  metadata: Map<string, GeminiFunctionMetadata>,
): Record<string, unknown> {
  // Preserve only the immutable leading system prefix in systemInstruction.
  // Later coordinator overlays vary per request and must stay at the tail so
  // they do not poison the reusable provider prefix.
  const split = splitLeadingSystem(request.messages, request.system);
  const providerOptions = request.providerOptions ?? {};
  const providerGenerationConfig =
    providerOptions.generationConfig && typeof providerOptions.generationConfig === "object"
      ? providerOptions.generationConfig as Record<string, unknown>
      : {};
  const namedToolChoice = request.toolChoice && typeof request.toolChoice === "object"
    ? request.toolChoice.name
    : undefined;
  if (namedToolChoice) {
    if (!request.tools?.some((tool) => tool.name === namedToolChoice)) {
      throw new Error(`Gemini named tool choice refers to an undeclared tool: ${namedToolChoice}`);
    }
  }
  return {
    ...(providerOptions.safetySettings !== undefined ? { safetySettings: providerOptions.safetySettings } : {}),
    ...(typeof providerOptions.cachedContent === "string" ? { cachedContent: providerOptions.cachedContent } : {}),
    contents: mapContents(split.messages, metadata),
    ...(split.system ? { systemInstruction: { parts: [{ text: split.system }] } } : {}),
    ...(request.tools?.length
      ? {
          tools: [
            {
              functionDeclarations: request.tools.map((tool) => ({
                name: tool.name,
                ...(tool.description ? { description: tool.description } : {}),
                parameters: geminiToolSchema(tool.inputSchema),
              })),
            },
          ],
          toolConfig: toolConfig(request.toolChoice),
        }
      : {}),
    ...(Object.keys(providerGenerationConfig).length || request.temperature !== undefined || request.maxTokens !== undefined
      ? {
          generationConfig: {
            ...providerGenerationConfig,
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

function normalizeGeminiTool(part: any, index: number, syntheticSequence = index) {
  const name = part.functionCall?.name ?? "unknown";
  const id = part.functionCall?.id ?? `gemini:${name}:${syntheticSequence}`;
  return normalizeToolCall(id, name, part.functionCall?.args ?? {}, index);
}

function geminiReasoningTokens(metadata: any): number | undefined {
  if (typeof metadata?.thoughtsTokenCount === "number" && metadata.thoughtsTokenCount >= 0) {
    return metadata.thoughtsTokenCount;
  }

  const prompt = metadata?.promptTokenCount;
  const candidates = metadata?.candidatesTokenCount;
  const total = metadata?.totalTokenCount;
  if ([prompt, candidates, total].every((value) => typeof value === "number" && Number.isFinite(value))) {
    const derived = total - prompt - candidates;
    if (derived >= 0) return derived;
  }
  return undefined;
}

export class GeminiProvider implements ProviderAdapter {
  readonly id: string;
  readonly #apiKey: string | undefined;
  readonly #accessToken: string | undefined;
  readonly #projectId: string | undefined;
  readonly #baseUrl: string;
  readonly #fetch: FetchLike | undefined;
  readonly #apiCallLogger: ProviderFetchLogger | undefined;
  readonly #functionMetadata = new Map<string, GeminiFunctionMetadata>();
  #syntheticFunctionSequence = 0;

  constructor(options: GeminiProviderOptions = {}) {
    this.id = options.id ?? "gemini";
    this.#apiKey = options.apiKey;
    this.#accessToken = options.accessToken;
    this.#projectId = options.projectId;
    this.#baseUrl = (options.baseUrl ?? "https://generativelanguage.googleapis.com/v1beta").replace(/\/$/, "");
    this.#fetch = options.fetch;
    this.#apiCallLogger = options.apiCallLogger;
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

  #rememberFunctionMetadata(part: any, normalizedId: string): void {
    const providerId = typeof part.functionCall?.id === "string" && part.functionCall.id
      ? part.functionCall.id
      : undefined;
    const thoughtSignature = typeof part.thoughtSignature === "string" && part.thoughtSignature
      ? part.thoughtSignature
      : undefined;
    if (!providerId && !thoughtSignature) return;
    this.#functionMetadata.set(normalizedId, {
      ...(providerId ? { providerId } : {}),
      ...(thoughtSignature ? { thoughtSignature } : {}),
    });
    while (this.#functionMetadata.size > 512) {
      const oldest = this.#functionMetadata.keys().next().value;
      if (typeof oldest !== "string") break;
      this.#functionMetadata.delete(oldest);
    }
  }

  async embed(request: EmbeddingRequest): Promise<EmbeddingResult> {
    return fetchEmbeddings({ provider: this.id, baseUrl: this.#baseUrl,
      headers: this.#headers(), request, format: "gemini",
      fetch: this.#fetch, apiCallLogger: this.#apiCallLogger });
  }

  async listModels(): Promise<string[]> {
    return (await this.listModelInfo()).map((model) => model.id);
  }

  async listModelInfo(): Promise<ModelInfo[]> {
    const models: ModelInfo[] = [];
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
      const next = typeof raw.nextPageToken === "string" ? raw.nextPageToken : undefined;
      if (!next || next === pageToken) break;
      pageToken = next;
    } while (pageToken);

    return models;
  }

  async complete(request: ProviderCallRequest): Promise<CallResult> {
    const response = await providerFetch(
      `${this.#baseUrl}/models/${encodeURIComponent(this.#modelPath(request.model))}:generateContent`,
      { method: "POST", headers: this.#headers(), body: JSON.stringify(bodyFor(request, this.#functionMetadata)) },
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
    const candidate = raw.candidates?.[0] ?? {};
    const parts = candidate.content?.parts ?? [];
    const reasoning = parts
      .filter((part: any) => part.thought === true && typeof part.text === "string")
      .map((part: any) => part.text)
      .join("");
    const toolCalls = parts
      .filter((part: any) => part.functionCall)
      .map((part: any, index: number) => {
        const toolCall = normalizeGeminiTool(part, index, this.#syntheticFunctionSequence++);
        this.#rememberFunctionMetadata(part, toolCall.id);
        return toolCall;
      });
    const normalizedUsage = usage(
      raw.usageMetadata?.promptTokenCount,
      raw.usageMetadata?.candidatesTokenCount,
      raw.usageMetadata?.totalTokenCount,
      raw.usageMetadata?.cachedContentTokenCount,
      undefined,
      geminiReasoningTokens(raw.usageMetadata),
    );

    return {
      provider: this.id,
      model: request.model,
      text: parts.filter((part: any) => part.thought !== true && typeof part.text === "string").map((part: any) => part.text).join(""),
      ...(reasoning ? { reasoning } : {}),
      toolCalls,
      finishReason: geminiFinish(candidate.finishReason, toolCalls.length > 0),
      ...(normalizedUsage ? { usage: normalizedUsage } : {}),
      raw,
    };
  }

  async *stream(request: ProviderCallRequest): AsyncIterable<StreamEvent> {
    const response = await providerFetch(
      `${this.#baseUrl}/models/${encodeURIComponent(this.#modelPath(request.model))}:streamGenerateContent?alt=sse`,
      { method: "POST", headers: this.#headers(), body: JSON.stringify(bodyFor(request, this.#functionMetadata)) },
      {
        provider: this.id,
        ...(this.#fetch ? { fetch: this.#fetch } : {}),
        ...(this.#apiCallLogger ? { apiCallLogger: this.#apiCallLogger } : {}),
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
          undefined,
          geminiReasoningTokens(raw.usageMetadata),
        );
      }
      if (!candidate) continue;

      const parts = candidate.content?.parts ?? [];
      let hasTools = false;
      for (const part of parts) {
        if (typeof part.text === "string" && part.text) {
          if (part.thought === true) yield { type: "reasoning-delta", delta: part.text };
          else yield { type: "text-delta", delta: part.text };
        }
        if (part.functionCall) {
          hasTools = true;
          const index = toolIndex++;
          const toolCall = normalizeGeminiTool(part, index, this.#syntheticFunctionSequence++);
          this.#rememberFunctionMetadata(part, toolCall.id);
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
