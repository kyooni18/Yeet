import { toolResultContent } from "../util.js";
import { ApproximateTokenCounter } from "./approximateTokenCounter.js";
import { AutoCompactor } from "./autoCompactor.js";
import { ContextAssembler } from "./contextAssembler.js";
import { ModelCompactLLM, type YeetModelClient } from "./modelCompactLLM.js";
import type { ChatMessage } from "./types.js";
import type { CallRequest, Message, ModelId, ToolCall, Usage } from "../types.js";

export interface RequestCompactorOptions {
  contextLength(model: string): Promise<number | undefined>;
  complete(request: CallRequest): Promise<{ text: string; usage?: Usage }>;
}

/**
 * Stateful bridge-side adapter that preserves the caller's canonical
 * transcript while replacing old request context with a durable checkpoint.
 */
export class RequestCompactor {
  readonly #options: RequestCompactorOptions;
  readonly #assemblers = new Map<string, ContextAssembler>();
  readonly #unknownWindowRequests = new Map<string, number>();
  readonly #tokens = new ApproximateTokenCounter();

  constructor(options: RequestCompactorOptions) { this.#options = options; }

  async compact(request: CallRequest): Promise<CallRequest> {
    return (await this.compactDetailed(request)).request;
  }

  async compactDetailed(request: CallRequest): Promise<{ request: CallRequest; auxiliaryUsage?: Usage }> {
    // The coordinator owns recoverable windows and budgets. Do not silently
    // replace its working set with recursively generated checkpoints.
    if (request.metadata?.contextManagement === "recoverable-windows") return { request };
    const { stableMessages, requestOnlyMessages } = splitRequestOnly(request.messages);
    const contextWindow = await this.#options.contextLength(request.model);
    if (!contextWindow) {
      const key = request.contextKey ? `${request.contextKey}\0${request.model}` : undefined;
      const attempts = key ? this.#touchUnknownWindowRequest(key) : 1;
      // If the provider does not publish a context window we cannot safely
      // issue an LLM checkpoint. Keep exact source for the first few model
      // calls, then switch to deterministic thinning. The two newest tool
      // rounds remain verbatim, and an older read can still be replayed
      // explicitly with refresh=true when the lead genuinely needs it again.
      const optimizedMessages = optimizeToolHistory(stableMessages, { preserveSource: attempts < 4 });
      return { request: finalizeRequest(request, optimizedMessages, requestOnlyMessages) };
    }
    if (request.contextKey) this.#unknownWindowRequests.delete(`${request.contextKey}\0${request.model}`);

    // Compaction must stay on the same provider/model as the lead request.
    // A separate override can silently route this auxiliary call through a
    // different credential/provider and make context-mode fail independently.
    const compactModel = request.model;
    const assemblerKey = request.contextKey ? `${request.contextKey}\0${request.model}` : undefined;
    let assembler = assemblerKey ? this.#assemblers.get(assemblerKey) : undefined;
    if (assemblerKey && assembler) {
      // Touch the entry so insertion order acts as a tiny LRU.
      this.#assemblers.delete(assemblerKey);
      this.#assemblers.set(assemblerKey, assembler);
    }
    if (!assembler) {
      const client: YeetModelClient = {
        complete: async (input) => {
          const result = await this.#options.complete({
            model: input.model as ModelId,
            messages: input.messages,
            maxTokens: 4_096,
            metadata: { purpose: "context-compaction" },
          });
          return { text: result.text, ...(result.usage ? { usage: result.usage } : {}) };
        },
      };
      assembler = new ContextAssembler(new AutoCompactor({
        tokens: this.#tokens,
        llm: new ModelCompactLLM(client, compactModel),
      }));
      if (assemblerKey) {
        while (this.#assemblers.size >= 64) {
          const oldest = this.#assemblers.keys().next().value as string | undefined;
          if (oldest === undefined) break;
          this.#assemblers.delete(oldest);
        }
        this.#assemblers.set(assemblerKey, assembler);
      }
    }

    // Before the first durable checkpoint exists, source reads are the only
    // exact copy of code the lead model has. Do not replace them with metadata
    // merely to save tokens. Once a checkpoint exists, older source payloads
    // may be thinned because their facts have been summarized durably.
    const optimizedMessages = optimizeToolHistory(stableMessages, {
      preserveSource: assembler.checkpoint === undefined,
    });

    const transcript = stableMessages.map((message, index) => toCompactMessage(message, index));
    const optimizedTranscript = optimizedMessages.map((message, index) => toCompactMessage(message, index));
    const toolTokens = request.tools?.length
      ? await this.#tokens.countText(JSON.stringify(request.tools))
      : 0;
    const overlayTokens = await this.#tokens.countMessages(requestOnlyMessages.map(toCompactMessage));
    const overheadTokens = toolTokens + overlayTokens;
    let assembled;
    try {
      assembled = await assembler.assemble({
        transcript,
        optimizedTranscript,
        capability: {
          contextWindow: Math.max(1, contextWindow - overheadTokens),
          ...(overheadTokens > 0 ? { requestOverheadTokens: overheadTokens } : {}),
          ...(request.maxTokens !== undefined ? { maxOutputTokens: request.maxTokens } : {}),
        },
      });
    } catch {
      // Compaction is an optimization, never a reason to fail the lead turn.
      // A provider-specific request rejection, transient failure, or malformed
      // checkpoint falls back to deterministic history thinning.
      const fallback = optimizeToolHistory(stableMessages, { preserveSource: true });
      return { request: finalizeRequest(request, fallback, requestOnlyMessages) };
    }
    // Retained IDs refer to the optimized originals. The checkpoint format is
    // text-only; round-tripping retained messages would lose images, structured
    // tool results/feedback, and the caller's stable cache breakpoint.
    const originals = new Map(optimizedMessages.map((message, index) => [`message:${index}`, message]));
    const preparedStable = !assembled.compacted && optimizedMessages === stableMessages
      ? stableMessages
      : assembled.messages.map((message) => originals.get(message.id) ?? fromCompactMessage(message));
    const prepared = finalizeRequest(request, preparedStable, requestOnlyMessages);
    return {
      request: prepared,
      ...(assembled.auxiliaryUsage ? { auxiliaryUsage: assembled.auxiliaryUsage } : {}),
    };
  }

  #touchUnknownWindowRequest(key: string): number {
    const next = (this.#unknownWindowRequests.get(key) ?? 0) + 1;
    this.#unknownWindowRequests.delete(key);
    this.#unknownWindowRequests.set(key, next);
    while (this.#unknownWindowRequests.size > 128) {
      const oldest = this.#unknownWindowRequests.keys().next().value as string | undefined;
      if (oldest === undefined) break;
      this.#unknownWindowRequests.delete(oldest);
    }
    return next;
  }
}

function splitRequestOnly(messages: Message[]): { stableMessages: Message[]; requestOnlyMessages: Message[] } {
  if (!messages.some((message) => message.requestOnly)) {
    return { stableMessages: messages, requestOnlyMessages: [] };
  }
  return {
    stableMessages: messages.filter((message) => !message.requestOnly),
    requestOnlyMessages: messages.filter((message) => message.requestOnly),
  };
}

function finalizeRequest(request: CallRequest, stableMessages: Message[], requestOnlyMessages: Message[]): CallRequest {
  // Caller-selected breakpoints represent an actually immutable prefix, for
  // example the current turn's user message. Preserve those instead of
  // moving the marker to the newest tool result every round. Moving it made
  // GPT-5.6 rewrite the whole growing conversation on every request.
  const hasStableBreakpoint = stableMessages.some((message) => message.cacheBreakpoint === true);
  const messages: Message[] = [
    ...stableMessages,
    ...requestOnlyMessages.map((message): Message => {
      if (!message.cacheBreakpoint) return message;
      const { cacheBreakpoint: _cacheBreakpoint, ...rest } = message;
      return rest;
    }),
  ];
  if (request.promptCache && !hasStableBreakpoint) {
    for (let index = stableMessages.length - 1; index >= 0; index -= 1) {
      const message = messages[index];
      if (!message || !canHostCacheBreakpoint(message)) continue;
      messages[index] = { ...message, cacheBreakpoint: true };
      break;
    }
  }
  if (messages === request.messages) return request;
  return { ...request, messages };
}

function canHostCacheBreakpoint(message: Message): boolean {
  if (message.role === "tool") {
    return Boolean(message.content || message.toolResult?.content || message.toolResult?.result !== undefined);
  }
  return Boolean(message.content || message.images?.length);
}

// Preserve complete results by tool-call round, not by individual result.
// A single assistant turn may issue several independent reads in parallel;
// dropping all but the final result makes the next model request believe the
// other reads never returned any source. Keep two rounds so one recovery turn
// cannot immediately evict the source it is trying to reuse.
const RECENT_TOOL_ROUNDS = 2;

function optimizeToolHistory(
  messages: Message[],
  options: { preserveSource?: boolean } = {},
): Message[] {
  const toolResults = messages.filter((message) => message.role === "tool");
  if (toolResults.length === 0) return messages;
  const recentIds = recentToolCallIds(messages, RECENT_TOOL_ROUNDS);
  // Be conservative with malformed/orphaned histories: if no matching
  // assistant tool-call round can be found, at least preserve the newest
  // result rather than compacting every tool response at once.
  if (recentIds.size === 0) {
    const latestId = toolResults.at(-1)?.toolCallId;
    if (latestId) recentIds.add(latestId);
  }
  let changed = false;
  const optimized = messages.map((message) => {
    if (message.role === "tool" && !recentIds.has(message.toolCallId ?? "")) {
      if (options.preserveSource && isSourceReadTool(message.name)) return message;
      const content = toolResultContent(message);
      const compacted = compactToolResult(message.name, content);
      if (Buffer.byteLength(compacted) < Buffer.byteLength(content)) {
        changed = true;
        const { toolResult: _toolResult, ...rest } = message;
        return { ...rest, content: compacted };
      }
      return message;
    }
    if (message.role === "assistant" && message.toolCalls?.some((call) => !recentIds.has(call.id))) {
      const calls = message.toolCalls.map((call) => {
        if (recentIds.has(call.id)) return call;
        const compacted = compactToolArguments(call.name, call.arguments);
        if (Buffer.byteLength(stringify(compacted)) >= Buffer.byteLength(stringify(call.arguments))) return call;
        changed = true;
        return { ...call, arguments: compacted };
      });
      return { ...message, toolCalls: calls };
    }
    return message;
  });
  return changed ? optimized : messages;
}

function isSourceReadTool(name: string | undefined): boolean {
  return name === "read_file" || name === "read_files" || name === "read_artifact";
}

function recentToolCallIds(messages: Message[], roundLimit: number): Set<string> {
  const ids = new Set<string>();
  let rounds = 0;
  for (let index = messages.length - 1; index >= 0 && rounds < roundLimit; index--) {
    const message = messages[index];
    if (!message) continue;
    if (message.role !== "assistant" || !message.toolCalls?.length) continue;
    rounds += 1;
    for (const call of message.toolCalls) {
      if (call.id) ids.add(call.id);
    }
  }
  return ids;
}

function compactToolResult(name: string | undefined, content: string): string {
  const parsed = safeParse(content);
  if ((name === "read_file" || name === "read_files") && isObject(parsed)) {
    return JSON.stringify(pick(parsed, [
      "path", "snapshot", "startLine", "endLine", "totalLines", "fileFullyRead",
      "hasMore", "nextStartLine", "externalized", "artifactId", "preview",
    ], { historical: true, contentOmitted: true }));
  }
  if ((name === "read_file" || name === "read_files") && Array.isArray(parsed)) {
    return JSON.stringify(parsed.slice(0, 16).map((item) => isObject(item)
      ? pick(item, [
        "path", "snapshot", "startLine", "endLine", "totalLines", "duplicate",
        "fileFullyRead", "hasMore", "nextStartLine", "externalized", "artifactId", "preview",
      ], { historical: true, contentOmitted: true })
      : item));
  }
  if (name === "search_workspace" && isObject(parsed)) {
    const matches = Array.isArray(parsed.matches) ? parsed.matches : [];
    return JSON.stringify({
      matchCount: Array.isArray(parsed.matches) ? matches.length : (parsed.matchCount ?? 0),
      filesScanned: parsed.filesScanned,
      truncated: parsed.truncated,
      historical: true,
    });
  }
  if (name === "list_files" && isObject(parsed)) {
    const entries = Array.isArray(parsed.entries) ? parsed.entries : [];
    return JSON.stringify({
      entryCount: Array.isArray(parsed.entries) ? entries.length : (parsed.entryCount ?? 0),
      truncated: parsed.truncated,
      resultLimitReached: parsed.resultLimitReached,
      depthLimited: parsed.depthLimited,
      duplicate: parsed.duplicate,
      coveredBy: parsed.coveredBy,
      historical: true,
    });
  }
  if (name === "apply_file_edits" && isObject(parsed)) {
    const files = Array.isArray(parsed.files)
      ? parsed.files.slice(0, 40).map((file) => isObject(file) ? pick(file, ["path", "destination", "operation", "snapshot", "warnings"]) : file)
      : [];
    const diagnostics = Array.isArray(parsed.diagnostics)
      ? parsed.diagnostics.slice(0, 20).map((diagnostic) => isObject(diagnostic) ? pick(diagnostic, ["path", "severity", "message", "line", "column", "source"]) : diagnostic)
      : [];
    return JSON.stringify({ files, diagnostics, historical: true });
  }
  if (content.length <= 2_000) return content;
  return `${content.slice(0, 1_400)}\n[historical tool output: ${content.length - 1_800} chars omitted]\n${content.slice(-400)}`;
}

function compactToolArguments(name: string, value: unknown): unknown {
  if ((name === "read_file" || name === "read_files") && isObject(value)) {
    if (Array.isArray(value.requests)) {
      return {
        requests: value.requests.slice(0, 8).map((request) => isObject(request)
          ? pick(request, ["path", "startLine", "endLine"])
          : request),
      };
    }
    return pick(value, ["path", "startLine", "endLine"]);
  }
  if (name === "list_files" && isObject(value)) return pick(value, ["path", "maxResults", "maxDepth"]);
  if (name === "search_workspace" && isObject(value)) return pick(value, ["query", "path", "maxResults", "caseSensitive", "regex"]);
  if (name === "apply_file_edits" && isObject(value) && Array.isArray(value.changes)) {
    return {
      changes: value.changes.slice(0, 40).map((change) => {
        if (!isObject(change)) return change;
        const fileOp = isObject(change.fileOp) ? pick(change.fileOp, ["kind", "destination"]) : undefined;
        return {
          ...pick(change, ["path", "snapshot"]),
          editCount: Array.isArray(change.edits) ? change.edits.length : (change.editCount ?? 0),
          ...(fileOp ? { fileOp } : {}),
        };
      }),
      historical: true,
    };
  }
  const serialized = stringify(value);
  if (serialized.length <= 1_200) return value;
  if (isObject(value)) {
    return Object.fromEntries(Object.entries(value).slice(0, 30).map(([key, item]) => [key, compactArgumentValue(item)]));
  }
  return { historical: true, chars: serialized.length };
}

function compactArgumentValue(value: unknown): unknown {
  if (typeof value === "string") return value.length <= 240 ? value : `${value.slice(0, 237)}...`;
  if (typeof value === "number" || typeof value === "boolean" || value === null) return value;
  if (Array.isArray(value)) return { itemCount: value.length };
  if (isObject(value)) return { keys: Object.keys(value).slice(0, 20) };
  return String(value);
}

function safeParse(value: string): unknown {
  try { return JSON.parse(value); }
  catch { return undefined; }
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function pick(value: Record<string, unknown>, keys: string[], extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    ...Object.fromEntries(keys.filter((key) => value[key] !== undefined).map((key) => [
      key,
      ["summary", "error", "message", "warnings", "fallbackReason", "preview"].includes(key)
        ? boundedHistoricalMetadata(value[key])
        : value[key],
    ])),
    ...extra,
  };
}

// Diagnostic fields can themselves contain large strings, arrays or objects.
// Bounding only the number of files/diagnostics leaves those payloads unbounded.
function boundedHistoricalMetadata(value: unknown): unknown {
  const text = typeof value === "string" ? value : stringify(value);
  const chars = Array.from(text);
  if (chars.length <= 500) return value;
  const preview = chars.slice(0, 497).join("") + "...";
  return typeof value === "string" ? preview : { preview, contentOmitted: true };
}

function toCompactMessage(message: Message, index: number): ChatMessage {
  return {
    id: `message:${index}`,
    role: message.role,
    content: message.role === "tool" ? toolResultContent(message) : (message.content ?? ""),
    ...(message.name !== undefined ? { name: message.name } : {}),
    ...(message.toolCallId !== undefined ? { toolCallId: message.toolCallId } : {}),
    ...(message.toolCalls !== undefined ? { toolCalls: message.toolCalls.map((call) => ({ id: call.id, name: call.name, arguments: stringify(call.arguments) })) } : {}),
  };
}

function fromCompactMessage(message: ChatMessage): Message {
  return {
    role: message.role === "developer" ? "system" : message.role,
    content: message.content,
    ...(message.name !== undefined ? { name: message.name } : {}),
    ...(message.toolCallId !== undefined ? { toolCallId: message.toolCallId } : {}),
    ...(message.toolCalls !== undefined ? { toolCalls: message.toolCalls.map((call): ToolCall => ({ id: call.id, name: call.name, arguments: parseArguments(call.arguments) })) } : {}),
  };
}

function stringify(value: unknown): string {
  if (value === undefined) return "";
  if (typeof value === "string") return value;
  try { return JSON.stringify(value); }
  catch { return String(value); }
}

function parseArguments(value: string): unknown {
  try { return JSON.parse(value); }
  catch { return value; }
}
