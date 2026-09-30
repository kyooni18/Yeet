import { createHash } from "node:crypto";

import { providerFetch, providerFetchAttempts, readJson } from "../http.js";
import { geminiToolSchema } from "./gemini.js";
import type { ProviderFetchLogger } from "../http.js";
import { parseSSE } from "../sse.js";
import type {
  CallResult,
  FetchLike,
  Message,
  ModelInfo,
  ProviderAdapter,
  ProviderCallRequest,
  ProviderState,
  StreamEvent,
  ToolCall,
  ToolDefinition,
  Usage,
} from "../types.js";
import { safeJsonParse, splitSystem, toolResultContent, usage } from "../util.js";

export const ANTIGRAVITY_AGENT = "antigravity-preview-09-2026";
const ANTIGRAVITY_PROTOCOL = "gemini-interactions-antigravity";
const DEFAULT_BASE_URL = "https://generativelanguage.googleapis.com/v1beta";
const HARNESS_INSTRUCTION = [
  "You are running as a reasoning provider inside the Yeet harness.",
  "The Antigravity remote environment is scratch space only and is not the user's local workspace.",
  "Use the caller-provided Yeet function tools for local workspace or external-system operations.",
  "Do not claim that a local side effect succeeded unless the corresponding function result confirms it.",
].join(" ");

export interface AntigravityProviderOptions {
  id?: string;
  apiKey?: string;
  baseUrl?: string;
  fetch?: FetchLike;
  apiCallLogger?: ProviderFetchLogger;
}

type AntigravityStateData = {
  interactionId: string;
  environmentId?: string;
  providerFunctionNames?: Record<string, string>;
};

type ToolMapping = {
  originalName: string;
  providerName: string;
  definition: Record<string, unknown>;
};

function stateData(message: Message, provider: string, model: string): AntigravityStateData | undefined {
  const state = message.providerState;
  if (!state || state.provider !== provider || state.protocol !== ANTIGRAVITY_PROTOCOL) return undefined;
  if (state.model && state.model !== model) return undefined;
  const data = state.data;
  if (!data || typeof data !== "object" || Array.isArray(data)) return undefined;
  const record = data as Record<string, unknown>;
  if (typeof record.interactionId !== "string" || !record.interactionId) return undefined;
  const names = record.providerFunctionNames;
  return {
    interactionId: record.interactionId,
    ...(typeof record.environmentId === "string" && record.environmentId ? { environmentId: record.environmentId } : {}),
    ...(names && typeof names === "object" && !Array.isArray(names)
      ? { providerFunctionNames: names as Record<string, string> }
      : {}),
  };
}

function continuation(
  messages: Message[],
  provider: string,
  model: string,
): { index: number; state: AntigravityStateData } | undefined {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (!message || message.role !== "assistant") continue;
    const state = stateData(message, provider, model);
    if (state) return { index, state };
  }
  return undefined;
}

function antigravityState(
  interaction: any,
  provider: string,
  model: string,
  providerFunctionNames: Record<string, string> = {},
): ProviderState | undefined {
  const interactionId = typeof interaction?.id === "string" ? interaction.id : undefined;
  if (!interactionId) return undefined;
  const environmentId = typeof interaction?.environment_id === "string" ? interaction.environment_id : undefined;
  return {
    provider,
    protocol: ANTIGRAVITY_PROTOCOL,
    model,
    data: {
      interactionId,
      ...(environmentId ? { environmentId } : {}),
      ...(Object.keys(providerFunctionNames).length ? { providerFunctionNames } : {}),
    } satisfies AntigravityStateData,
  };
}

function providerToolName(name: string): string {
  const digest = createHash("sha256").update(name).digest("hex").slice(0, 12);
  const readable = name.replace(/[^A-Za-z0-9_]/g, "_").replace(/^_+|_+$/g, "").slice(0, 40) || "tool";
  return `yeet_${digest}_${readable}`;
}

function mapTools(tools: ToolDefinition[] | undefined): ToolMapping[] {
  return (tools ?? []).map((tool) => {
    const providerName = providerToolName(tool.name);
    return {
      originalName: tool.name,
      providerName,
      definition: {
        type: "function",
        name: providerName,
        ...(tool.description ? { description: tool.description } : {}),
        parameters: geminiToolSchema(tool.inputSchema),
      },
    };
  });
}

function toolChoice(request: ProviderCallRequest, mappings: ToolMapping[]): unknown {
  if (request.toolChoice === "none" || mappings.length === 0) return "none";
  const names = mappings.map((mapping) => mapping.providerName);
  if (request.toolChoice === "required") {
    return { allowed_tools: { mode: "any", tools: names } };
  }
  if (request.toolChoice && typeof request.toolChoice === "object") {
    const choice = request.toolChoice;
    const selected = mappings.find((mapping) => mapping.originalName === choice.name);
    return selected
      ? { allowed_tools: { mode: "any", tools: [selected.providerName] } }
      : { allowed_tools: { mode: "any", tools: names } };
  }
  return { allowed_tools: { mode: "auto", tools: names } };
}

function imageBlocks(message: Message): Record<string, unknown>[] {
  return (message.images ?? []).map((image) => ({
    type: "image",
    data: image.data,
    mime_type: image.mediaType,
  }));
}

function userContent(message: Message): Record<string, unknown>[] {
  return [
    ...(message.content ? [{ type: "text", text: message.content }] : []),
    ...imageBlocks(message),
  ];
}

function transcript(messages: Message[]): string {
  const lines: string[] = [];
  for (const message of messages) {
    if (message.role === "user") {
      if (message.content) lines.push(`User: ${message.content}`);
      if (message.images?.length) lines.push(`User attached ${message.images.length} image(s).`);
      continue;
    }
    if (message.role === "assistant") {
      if (message.content) lines.push(`Assistant: ${message.content}`);
      for (const call of message.toolCalls ?? []) {
        lines.push(`Assistant requested tool ${call.name} with ${JSON.stringify(call.arguments)}.`);
      }
      continue;
    }
    if (message.role === "tool") {
      lines.push(`Tool ${message.name ?? message.toolResult?.name ?? message.toolCallId ?? "unknown"}: ${toolResultContent(message)}`);
    }
  }
  return lines.join("\n\n");
}

function functionResult(
  message: Message,
  providerFunctionNames: Record<string, string> | undefined,
): Record<string, unknown> {
  const callId = message.toolCallId ?? message.toolResult?.toolCallId ?? "";
  const storedName = callId ? providerFunctionNames?.[callId] : undefined;
  const result = message.toolResult?.result !== undefined
    ? message.toolResult.result
    : safeJsonParse(toolResultContent(message));
  return {
    type: "function_result",
    call_id: callId,
    ...(storedName ? { name: storedName } : {}),
    result,
    ...(message.toolResult?.isError ? { is_error: true } : {}),
  };
}

function requestInput(
  messages: Message[],
  activeStart: number,
  state: AntigravityStateData | undefined,
): unknown {
  const active = messages.slice(activeStart).filter((message) => message.role !== "system");
  if (state) {
    const hasToolResults = active.some((message) => message.role === "tool");
    if (hasToolResults) {
      const steps: Record<string, unknown>[] = [];
      for (const message of active) {
        if (message.role === "tool") {
          steps.push(functionResult(message, state.providerFunctionNames));
        } else if (message.role === "user") {
          const content = userContent(message);
          if (content.length) steps.push({ type: "user_input", content });
        }
      }
      return steps;
    }
    const content = active.flatMap((message) => message.role === "user" ? userContent(message) : []);
    if (content.length) return content;
  }

  const nonSystem = messages.filter((message) => message.role !== "system");
  const onlyUsers = nonSystem.every((message) => message.role === "user");
  if (onlyUsers) {
    const content = nonSystem.flatMap(userContent);
    if (content.length) return content;
  }
  const history = transcript(nonSystem);
  return [{ type: "text", text: history || "Continue." }];
}

function systemInstruction(request: ProviderCallRequest): string {
  const split = splitSystem(request.messages, request.system);
  return split.system ? `${split.system}\n\n${HARNESS_INSTRUCTION}` : HARNESS_INSTRUCTION;
}

function requestBody(request: ProviderCallRequest, provider: string, stream: boolean): Record<string, unknown> {
  const continued = continuation(request.messages, provider, request.model);
  const mappings = mapTools(request.tools);
  const state = continued?.state;
  return {
    ...(request.providerOptions ?? {}),
    agent: request.model,
    input: requestInput(request.messages, continued ? continued.index + 1 : 0, state),
    environment: state?.environmentId ?? "remote",
    ...(state?.interactionId ? { previous_interaction_id: state.interactionId } : {}),
    system_instruction: systemInstruction(request),
    tools: mappings.map((mapping) => mapping.definition),
    tool_choice: toolChoice(request, mappings),
    ...(request.providerMetadata && Object.keys(request.providerMetadata).length
      ? { labels: request.providerMetadata }
      : {}),
    ...(stream ? { stream: true } : {}),
  };
}

function textFromContent(content: unknown): string {
  if (!Array.isArray(content)) return "";
  return content
    .map((part: any) => part?.type === "text" && typeof part.text === "string" ? part.text : "")
    .join("");
}

function outputText(raw: any): string {
  if (typeof raw?.output_text === "string") return raw.output_text;
  if (!Array.isArray(raw?.steps)) return "";
  return raw.steps
    .filter((step: any) => step?.type === "model_output")
    .map((step: any) => textFromContent(step.content))
    .join("");
}

function thoughtSummary(raw: any): string | undefined {
  if (!Array.isArray(raw?.steps)) return undefined;
  const parts: string[] = [];
  for (const step of raw.steps) {
    if (step?.type !== "thought") continue;
    const summary = Array.isArray(step.summary) ? step.summary : step.summary ? [step.summary] : [];
    const text = textFromContent(summary);
    if (text) parts.push(text);
  }
  return parts.length ? parts.join("\n") : undefined;
}

function pendingToolCalls(
  raw: any,
  mappings: ToolMapping[],
): { calls: ToolCall[]; providerFunctionNames: Record<string, string> } {
  const steps = Array.isArray(raw?.steps) ? raw.steps : [];
  const executed = new Set(
    steps
      .filter((step: any) => step?.type === "function_result" && typeof step.call_id === "string")
      .map((step: any) => step.call_id),
  );
  const originalByProvider = new Map(mappings.map((mapping) => [mapping.providerName, mapping.originalName]));
  const calls: ToolCall[] = [];
  const providerFunctionNames: Record<string, string> = {};
  for (const step of steps) {
    if (step?.type !== "function_call" || typeof step.id !== "string" || executed.has(step.id)) continue;
    const originalName = typeof step.name === "string" ? originalByProvider.get(step.name) : undefined;
    if (!originalName) continue;
    calls.push({ id: step.id, name: originalName, arguments: step.arguments ?? {} });
    providerFunctionNames[step.id] = step.name;
  }
  return { calls, providerFunctionNames };
}

function normalizedUsage(raw: any, transportAttempts?: number): Usage | undefined {
  const value = raw?.usage;
  return usage(
    typeof value?.total_input_tokens === "number" ? value.total_input_tokens : undefined,
    typeof value?.total_output_tokens === "number" ? value.total_output_tokens : undefined,
    typeof value?.total_tokens === "number" ? value.total_tokens : undefined,
    typeof value?.total_cached_tokens === "number" ? value.total_cached_tokens : undefined,
    undefined,
    typeof value?.total_thought_tokens === "number" ? value.total_thought_tokens : undefined,
    transportAttempts,
  );
}

function finishReason(status: unknown, toolCalls: ToolCall[]): CallResult["finishReason"] {
  if (toolCalls.length) return "tool_call";
  if (status === "incomplete" || status === "budget_exceeded") return "length";
  if (status === "failed" || status === "cancelled") return "error";
  if (status === "completed") return "stop";
  return "unknown";
}

function interactionFailure(raw: any): Error | undefined {
  if (raw?.status !== "failed" && raw?.status !== "cancelled") return undefined;
  const error = Array.isArray(raw?.errors) ? raw.errors[0] : undefined;
  const message = typeof error?.message === "string"
    ? error.message
    : `Antigravity interaction ${String(raw?.status ?? "failed")}`;
  return new Error(message);
}

export class AntigravityProvider implements ProviderAdapter {
  readonly id: string;
  readonly #apiKey: string | undefined;
  readonly #baseUrl: string;
  readonly #fetch: FetchLike | undefined;
  readonly #apiCallLogger: ProviderFetchLogger | undefined;

  constructor(options: AntigravityProviderOptions = {}) {
    this.id = options.id ?? "antigravity";
    this.#apiKey = options.apiKey;
    this.#baseUrl = (options.baseUrl ?? DEFAULT_BASE_URL).replace(/\/$/, "");
    this.#fetch = options.fetch;
    this.#apiCallLogger = options.apiCallLogger;
  }

  #headers(): Record<string, string> {
    if (!this.#apiKey) throw new Error(`Missing API key for ${this.id}`);
    return {
      "content-type": "application/json",
      "x-goog-api-key": this.#apiKey,
    };
  }

  async listModels(): Promise<string[]> {
    return [ANTIGRAVITY_AGENT];
  }

  async listModelInfo(): Promise<ModelInfo[]> {
    return [{ id: ANTIGRAVITY_AGENT }];
  }

  async complete(request: ProviderCallRequest): Promise<CallResult> {
    const response = await providerFetch(
      `${this.#baseUrl}/interactions`,
      {
        method: "POST",
        headers: this.#headers(),
        body: JSON.stringify(requestBody(request, this.id, false)),
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
    const attempts = providerFetchAttempts(response);
    const raw = await readJson<any>(response);
    const failure = interactionFailure(raw);
    if (failure) throw failure;
    const mappings = mapTools(request.tools);
    const pending = pendingToolCalls(raw, mappings);
    const state = antigravityState(raw, this.id, request.model, pending.providerFunctionNames);
    const summary = thoughtSummary(raw);
    const resultUsage = normalizedUsage(raw, attempts);
    return {
      provider: this.id,
      model: request.model,
      ...(typeof raw?.id === "string" ? { id: raw.id } : {}),
      text: outputText(raw),
      ...(summary ? { reasoningSummary: summary } : {}),
      toolCalls: pending.calls,
      ...(state ? { providerState: state } : {}),
      finishReason: finishReason(raw?.status, pending.calls),
      ...(resultUsage ? { usage: resultUsage } : {}),
      raw,
    };
  }

  async *stream(request: ProviderCallRequest): AsyncIterable<StreamEvent> {
    const mappings = mapTools(request.tools);
    const originalByProvider = new Map(mappings.map((mapping) => [mapping.providerName, mapping.originalName]));
    const response = await providerFetch(
      `${this.#baseUrl}/interactions`,
      {
        method: "POST",
        headers: { ...this.#headers(), accept: "text/event-stream" },
        body: JSON.stringify(requestBody(request, this.id, true)),
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
    const attempts = providerFetchAttempts(response);
    let interaction: any = undefined;
    let started = false;
    let latestUsage: Usage | undefined;
    let toolCallCount = 0;
    const providerFunctionNames: Record<string, string> = {};
    const callByIndex = new Map<number, { id: string; originalName: string; providerName: string; arguments: string }>();

    for await (const message of parseSSE(response)) {
      if (!message.data || message.data === "[DONE]") continue;
      const event = safeJsonParse(message.data) as any;
      if (!event || typeof event !== "object") continue;
      const eventType = typeof event.event_type === "string" ? event.event_type : message.event;

      if (eventType === "interaction.created" || eventType === "interaction.start") {
        interaction = event.interaction ?? interaction;
        if (!started) {
          started = true;
          yield {
            type: "start",
            provider: this.id,
            model: request.model,
            ...(typeof interaction?.id === "string" ? { id: interaction.id } : {}),
          };
        }
        continue;
      }

      if (eventType === "step.start") {
        const index = typeof event.index === "number" ? event.index : 0;
        const step = event.step;
        if (step?.type === "model_output") {
          const initial = textFromContent(step.content);
          if (initial) yield { type: "text-delta", delta: initial };
        } else if (step?.type === "thought") {
          yield { type: "reasoning-start" };
          const initial = textFromContent(Array.isArray(step.summary) ? step.summary : step.summary ? [step.summary] : []);
          if (initial) yield { type: "reasoning-summary-delta", delta: initial };
        } else if (step?.type === "function_call" && typeof step.id === "string" && typeof step.name === "string") {
          const originalName = originalByProvider.get(step.name);
          if (originalName) {
            const initialArguments = step.arguments && Object.keys(step.arguments).length ? JSON.stringify(step.arguments) : "";
            callByIndex.set(index, { id: step.id, originalName, providerName: step.name, arguments: initialArguments });
            providerFunctionNames[step.id] = step.name;
            yield { type: "tool-call-delta", index, id: step.id, name: originalName };
            if (initialArguments) yield { type: "tool-call-delta", index, argumentsDelta: initialArguments };
          }
        } else if (typeof step?.type === "string" && step.type.endsWith("_call")) {
          const title = step.type === "google_search_call" ? "Searching web"
            : step.type === "code_execution_call" ? "Running remote code"
            : step.type === "url_context_call" ? "Reading web page"
            : `Running ${step.type.replace(/_call$/, "").replaceAll("_", " ")}`;
          yield { type: "activity", title, detail: "Antigravity remote environment" };
        }
        continue;
      }

      if (eventType === "step.delta") {
        const index = typeof event.index === "number" ? event.index : 0;
        const delta = event.delta;
        if (delta?.type === "text" && typeof delta.text === "string") {
          yield { type: "text-delta", delta: delta.text };
        } else if (delta?.type === "thought_summary") {
          const content = delta.content;
          const text = content?.type === "text" && typeof content.text === "string" ? content.text : "";
          if (text) yield { type: "reasoning-summary-delta", delta: text };
        } else if (delta?.type === "arguments_delta" && typeof delta.arguments === "string") {
          const call = callByIndex.get(index);
          if (call) {
            call.arguments += delta.arguments;
            yield { type: "tool-call-delta", index, argumentsDelta: delta.arguments };
          }
        }
        const totalUsage = event.metadata?.total_usage;
        if (totalUsage) latestUsage = normalizedUsage({ usage: totalUsage }, attempts);
        continue;
      }

      if (eventType === "step.stop") {
        const index = typeof event.index === "number" ? event.index : 0;
        const call = callByIndex.get(index);
        if (call) {
          const toolCall: ToolCall = {
            id: call.id,
            name: call.originalName,
            arguments: safeJsonParse(call.arguments),
          };
          toolCallCount += 1;
          yield { type: "tool-call", index, toolCall };
          callByIndex.delete(index);
        }
        continue;
      }

      if (eventType?.startsWith("interaction.")) {
        if (event.interaction && typeof event.interaction === "object") interaction = { ...interaction, ...event.interaction };
        if (typeof event.status === "string") interaction = { ...interaction, status: event.status };
        const terminal = [
          "interaction.completed",
          "interaction.requires_action",
          "interaction.incomplete",
          "interaction.budget_exceeded",
          "interaction.failed",
          "interaction.cancelled",
        ].includes(eventType);
        if (!terminal) continue;
        const failure = interactionFailure(interaction);
        if (failure) throw failure;
        if (!started) {
          started = true;
          yield {
            type: "start",
            provider: this.id,
            model: request.model,
            ...(typeof interaction?.id === "string" ? { id: interaction.id } : {}),
          };
        }
        const finalUsage = normalizedUsage(interaction, attempts) ?? latestUsage;
        const state = antigravityState(interaction, this.id, request.model, providerFunctionNames);
        yield {
          type: "finish",
          finishReason: toolCallCount > 0 ? "tool_call" : finishReason(interaction?.status, []),
          ...(finalUsage ? { usage: finalUsage } : {}),
          ...(state ? { providerState: state } : {}),
          raw: interaction,
        };
        return;
      }
    }

    if (!started) yield { type: "start", provider: this.id, model: request.model };
    const state = antigravityState(interaction, this.id, request.model, providerFunctionNames);
    yield {
      type: "finish",
      finishReason: toolCallCount > 0 ? "tool_call" : finishReason(interaction?.status, []),
      ...(latestUsage ? { usage: latestUsage } : {}),
      ...(state ? { providerState: state } : {}),
      ...(interaction ? { raw: interaction } : {}),
    };
  }
}
