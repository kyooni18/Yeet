import { randomUUID } from "node:crypto";

import {
  AntigravityLocalClient,
  type AntigravityAvailableModelsResponse,
  type AntigravityModelRecord,
} from "../antigravity-local.js";
import type {
  CallResult,
  Message,
  ModelInfo,
  ProviderAdapter,
  ProviderCallRequest,
  StreamEvent,
  ToolCall,
  ToolDefinition,
} from "../types.js";
import { toolResultContent } from "../util.js";

export interface AntigravityLocalProviderOptions {
  client?: AntigravityLocalClient;
}

type ModelEntry = {
  id: string;
  providerModel: string;
  record: AntigravityModelRecord;
};

type ToolEnvelope =
  | { type: "tool_call"; name: string; arguments?: unknown }
  | { type: "final"; text?: string };

function rawModels(payload: AntigravityAvailableModelsResponse): Record<string, AntigravityModelRecord> {
  const nested = payload.response?.models;
  if (nested && typeof nested === "object" && !Array.isArray(nested)) return nested;
  const direct = payload.models;
  return direct && typeof direct === "object" && !Array.isArray(direct) ? direct : {};
}

function modelEntries(payload: AntigravityAvailableModelsResponse): ModelEntry[] {
  const all = Object.entries(rawModels(payload)).flatMap(([id, value]): ModelEntry[] => {
    const providerModel = typeof value.model === "string" && value.model ? value.model : "";
    if (!providerModel) return [];
    return [{ id, providerModel, record: value }];
  });
  const visible = all.filter(({ record }) =>
    record.isInternal !== true
    && typeof record.displayName === "string"
    && record.displayName.trim().length > 0);
  return visible.length > 0 ? visible : all.filter(({ record }) => record.isInternal !== true);
}

function messageLine(message: Message): string {
  if (message.role === "tool") {
    return `Tool ${message.name ?? message.toolResult?.name ?? message.toolCallId ?? "unknown"} result: ${toolResultContent(message)}`;
  }
  if (message.role === "assistant") {
    const calls = (message.toolCalls ?? []).map((call) =>
      `tool ${call.name}(${JSON.stringify(call.arguments)})`).join("; ");
    return `Assistant: ${[message.content, calls].filter(Boolean).join("\n")}`;
  }
  const role = message.role === "system" ? "System" : "User";
  return `${role}: ${message.content ?? ""}`;
}

function toolProtocol(tools: ToolDefinition[], choice: ProviderCallRequest["toolChoice"]): string {
  if (choice === "none" || tools.length === 0) return "";
  const selected = choice && typeof choice === "object"
    ? tools.filter((tool) => tool.name === choice.name)
    : tools;
  const definitions = selected.map((tool) => ({
    name: tool.name,
    ...(tool.description ? { description: tool.description } : {}),
    inputSchema: tool.inputSchema,
  }));
  const requirement = choice === "required" || (choice && typeof choice === "object")
    ? "You must call one listed tool on this turn."
    : "Call a tool only when it is needed; otherwise return a final answer.";
  return [
    "",
    "YEET TOOL PROTOCOL",
    requirement,
    "Return exactly one JSON object with no markdown fences and no text outside it.",
    'For a tool call: {"type":"tool_call","name":"tool_name","arguments":{...}}',
    'For a final answer: {"type":"final","text":"answer"}',
    "Never invent a tool name. Tool arguments must satisfy the listed JSON schema.",
    `Available tools: ${JSON.stringify(definitions)}`,
  ].join("\n");
}

function promptFor(request: ProviderCallRequest): string {
  const transcript = request.messages.map(messageLine).join("\n\n");
  const system = request.system ? `System: ${request.system}\n\n` : "";
  return `${system}${transcript}${toolProtocol(request.tools ?? [], request.toolChoice)}`;
}

function parseEnvelope(raw: string): ToolEnvelope | undefined {
  const trimmed = raw.trim();
  const candidates = [
    trimmed,
    trimmed.replace(/^\s*```(?:json)?\s*/i, "").replace(/\s*```\s*$/i, ""),
  ];
  const firstBrace = trimmed.indexOf("{");
  const lastBrace = trimmed.lastIndexOf("}");
  if (firstBrace >= 0 && lastBrace > firstBrace) candidates.push(trimmed.slice(firstBrace, lastBrace + 1));
  for (const candidate of candidates) {
    try {
      const value = JSON.parse(candidate);
      if (!value || typeof value !== "object" || Array.isArray(value)) continue;
      if (value.type === "tool_call" && typeof value.name === "string") {
        return { type: "tool_call", name: value.name, arguments: value.arguments ?? {} };
      }
      if (value.type === "final") {
        return { type: "final", text: typeof value.text === "string" ? value.text : "" };
      }
    } catch {
      // Try the next extraction.
    }
  }
  return undefined;
}

export class AntigravityLocalProvider implements ProviderAdapter {
  readonly id = "antigravity";
  readonly #client: AntigravityLocalClient;
  #models = new Map<string, ModelEntry>();

  constructor(options: AntigravityLocalProviderOptions = {}) {
    this.#client = options.client ?? new AntigravityLocalClient();
  }

  async #entries(forceRefresh = false, signal?: AbortSignal): Promise<ModelEntry[]> {
    if (!forceRefresh && this.#models.size > 0) return [...this.#models.values()];
    const entries = modelEntries(await this.#client.getAvailableModels(
      forceRefresh || this.#models.size === 0,
      signal ? { signal } : {},
    ));
    this.#models = new Map(entries.map((entry) => [entry.id, entry]));
    return entries;
  }

  async listModels(): Promise<string[]> {
    return (await this.listModelInfo()).map((model) => model.id);
  }

  async listModelInfo(): Promise<ModelInfo[]> {
    return (await this.#entries(true)).map(({ id, record }) => {
      const contextLength = typeof record.maxTokens === "number" && Number.isSafeInteger(record.maxTokens) && record.maxTokens > 0
        ? record.maxTokens
        : undefined;
      return { id, ...(contextLength !== undefined ? { contextLength } : {}) };
    });
  }

  async complete(request: ProviderCallRequest): Promise<CallResult> {
    if (request.messages.some((message) => (message.images?.length ?? 0) > 0)) {
      throw new Error("Antigravity local GetModelResponse currently supports text requests only");
    }

    let entry = this.#models.get(request.model);
    if (!entry) {
      await this.#entries(true, request.signal);
      entry = this.#models.get(request.model);
    }
    if (!entry) throw new Error(`Antigravity model is not available for this account: ${request.model}`);

    const raw = await this.#client.getModelResponse(
      promptFor(request),
      entry.providerModel,
      {
        ...(request.timeoutMs !== undefined ? { timeoutMs: request.timeoutMs } : {}),
        ...(request.signal ? { signal: request.signal } : {}),
      },
    );
    const envelope = parseEnvelope(raw);
    const allowedTools = new Map((request.tools ?? []).map((tool) => [tool.name, tool]));
    const namedChoice = request.toolChoice && typeof request.toolChoice === "object" ? request.toolChoice.name : undefined;

    if (envelope?.type === "tool_call"
        && allowedTools.has(envelope.name)
        && (!namedChoice || namedChoice === envelope.name)
        && request.toolChoice !== "none") {
      const toolCall: ToolCall = {
        id: `antigravity:${randomUUID()}`,
        name: envelope.name,
        arguments: envelope.arguments ?? {},
      };
      return {
        provider: this.id,
        model: request.model,
        text: "",
        toolCalls: [toolCall],
        finishReason: "tool_call",
        raw,
      };
    }

    return {
      provider: this.id,
      model: request.model,
      text: envelope?.type === "final" ? envelope.text ?? "" : raw,
      toolCalls: [],
      finishReason: "stop",
      raw,
    };
  }

  async *stream(request: ProviderCallRequest): AsyncIterable<StreamEvent> {
    yield { type: "start", provider: this.id, model: request.model };
    const result = await this.complete(request);
    if (result.toolCalls.length > 0) {
      for (const [index, toolCall] of result.toolCalls.entries()) {
        yield { type: "tool-call-delta", index, id: toolCall.id, name: toolCall.name };
        yield { type: "tool-call", index, toolCall };
      }
    } else if (result.text) {
      yield { type: "text-delta", delta: result.text };
    }
    yield { type: "finish", finishReason: result.finishReason, raw: result.raw };
  }
}
