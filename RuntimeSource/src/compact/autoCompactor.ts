import { createHash, randomUUID } from "node:crypto";
import { InvalidCheckpointError } from "./modelCompactLLM.js";
import type { ChatMessage, CompactCheckpoint, CompactLLM, CompactPolicy, CompactResult, ModelCapability, TokenCounter } from "./types.js";

const DEFAULT_POLICY: CompactPolicy = {
  triggerRatio: 0.70,
  reservedOutputTokens: 8_192,
  reservedToolTokens: 0,
  safetyMarginTokens: 4_096,
  recentTokens: 8_192,
  minimumMessages: 16,
  maxHistoricalToolChars: 4_000,
  economicalContextTokens: 24_000,
  cumulativeTriggerTokens: 28_000,
  cumulativeMinimumRequests: 4,
};

export class AutoCompactor {
  readonly #tokens: TokenCounter;
  readonly #llm: CompactLLM;
  readonly #policy: CompactPolicy;
  #presentedTokensSinceCheckpoint = 0;
  #requestsSinceCheckpoint = 0;
  #rejectedInput: string | undefined;

  constructor(options: { tokens: TokenCounter; llm: CompactLLM; policy?: Partial<CompactPolicy> }) {
    this.#tokens = options.tokens;
    this.#llm = options.llm;
    this.#policy = { ...DEFAULT_POLICY, ...options.policy };
  }

  async buildContext(input: {
    messages: readonly ChatMessage[];
    checkpointMessages?: readonly ChatMessage[];
    capability: ModelCapability;
    previousCheckpoint?: CompactCheckpoint;
  }): Promise<CompactResult> {
    const messages = [...input.messages];
    const { pinned, ordinary } = this.#splitPinned(messages);
    const baseContext = input.previousCheckpoint
      ? [...pinned, this.#checkpointAsContextMessage(input.previousCheckpoint), ...ordinary]
      : messages;
    const beforeTokens = await this.#tokens.countMessages(baseContext);
    this.#presentedTokensSinceCheckpoint += beforeTokens + (input.capability.requestOverheadTokens ?? 0);
    this.#requestsSinceCheckpoint += 1;
    if (!this.#shouldCompact(beforeTokens, ordinary.length, input.capability)) {
      return {
        compacted: input.previousCheckpoint !== undefined,
        context: baseContext,
        ...(input.previousCheckpoint ? { checkpoint: input.previousCheckpoint } : {}),
        beforeTokens,
        afterTokens: beforeTokens,
        consumedMessages: 0,
      };
    }
    const usable = this.#usableTokens(input.capability);
    const recentBudget = Math.max(
      1,
      Math.min(this.#policy.recentTokens, Math.floor(Math.max(1, usable) * 0.75)),
    );
    const { historical, recent } = await this.#partitionOrdinary(ordinary, recentBudget);
    if (historical.length === 0) {
      return {
        compacted: input.previousCheckpoint !== undefined,
        context: baseContext,
        ...(input.previousCheckpoint ? { checkpoint: input.previousCheckpoint } : {}),
        beforeTokens,
        afterTokens: beforeTokens,
        consumedMessages: 0,
      };
    }
    // Ordinary lead requests may use a token-thinned transcript where older
    // tool payloads have been replaced by compact metadata. That form is fine
    // for presentation, but it is the wrong source for a durable checkpoint:
    // summarizing `contentOmitted` permanently discards the source facts the
    // checkpoint exists to preserve. Prefer the matching canonical historical
    // prefix when the caller can provide it.
    const checkpointOrdinary = input.checkpointMessages
      ? this.#splitPinned([...input.checkpointMessages]).ordinary
      : ordinary;
    const checkpointHistorical = checkpointOrdinary.length === ordinary.length
      ? checkpointOrdinary.slice(0, historical.length)
      : historical;
    const checkpointInput = {
      ...(input.previousCheckpoint ? { previous: input.previousCheckpoint } : {}),
      messages: this.#prepareHistorical(checkpointHistorical),
    };
    const inputKey = createHash("sha256").update(JSON.stringify(checkpointInput)).digest("hex");
    if (inputKey === this.#rejectedInput) {
      return {
        compacted: input.previousCheckpoint !== undefined,
        context: baseContext,
        ...(input.previousCheckpoint ? { checkpoint: input.previousCheckpoint } : {}),
        beforeTokens, afterTokens: beforeTokens, consumedMessages: 0,
      };
    }
    let checkpointResult;
    try {
      checkpointResult = await this.#llm.createCheckpoint(checkpointInput);
    } catch (error) {
      // Only completed, invalid generations are suppressed. Transport errors
      // remain retryable, and changing the historical input permits a new try.
      if (!(error instanceof InvalidCheckpointError)) throw error;
      this.resetCostAccounting();
      this.#rejectedInput = inputKey;
      return {
        compacted: input.previousCheckpoint !== undefined,
        context: baseContext,
        ...(input.previousCheckpoint ? { checkpoint: input.previousCheckpoint } : {}),
        beforeTokens, afterTokens: beforeTokens, consumedMessages: 0,
        auxiliaryUsage: error.usage ? withModelCall(error.usage) : { modelCalls: 1 },
      };
    }
    const checkpoint = checkpointResult.checkpoint;
    const context = [...pinned, this.#checkpointAsContextMessage(checkpoint), ...recent];
    const afterTokens = await this.#tokens.countMessages(context);
    // A verbose checkpoint can cost more on every subsequent request than
    // the history it replaces. Keep the original context in that case, while
    // reporting the auxiliary call that was actually paid for.
    if (afterTokens >= beforeTokens) {
      this.resetCostAccounting();
      this.#rejectedInput = inputKey;
      return {
        compacted: input.previousCheckpoint !== undefined,
        context: baseContext,
        ...(input.previousCheckpoint ? { checkpoint: input.previousCheckpoint } : {}),
        beforeTokens,
        afterTokens: beforeTokens,
        consumedMessages: 0,
        auxiliaryUsage: checkpointResult.usage
          ? withModelCall(checkpointResult.usage)
          : { modelCalls: 1 },
      };
    }
    this.#rejectedInput = undefined;
    this.#presentedTokensSinceCheckpoint = afterTokens;
    this.#requestsSinceCheckpoint = 0;
    return {
      compacted: true,
      context,
      checkpoint,
      beforeTokens,
      afterTokens,
      consumedMessages: historical.length,
      ...(checkpointResult.usage ? { auxiliaryUsage: withModelCall(checkpointResult.usage) } : { auxiliaryUsage: { modelCalls: 1 } }),
    };
  }

  #shouldCompact(tokens: number, messageCount: number, capability: ModelCapability): boolean {
    const usable = this.#usableTokens(capability);
    if (usable <= 0) return true;
    if (tokens >= usable * 0.90) return true;
    if (this.#requestsSinceCheckpoint >= this.#policy.cumulativeMinimumRequests
      && this.#presentedTokensSinceCheckpoint >= this.#policy.cumulativeTriggerTokens
      && tokens > this.#policy.recentTokens) return true;
    if (messageCount < this.#policy.minimumMessages) return false;
    const economicalTrigger = Math.min(
      usable * this.#policy.triggerRatio,
      this.#policy.economicalContextTokens,
    );
    return tokens >= economicalTrigger;
  }

  resetCostAccounting(): void {
    this.#rejectedInput = undefined;
    this.#presentedTokensSinceCheckpoint = 0;
    this.#requestsSinceCheckpoint = 0;
  }

  #usableTokens(capability: ModelCapability): number {
    const reservedOutputTokens = capability.maxOutputTokens ?? this.#policy.reservedOutputTokens;
    return capability.contextWindow
      - reservedOutputTokens
      - this.#policy.reservedToolTokens
      - this.#policy.safetyMarginTokens;
  }

  #splitPinned(messages: readonly ChatMessage[]): { pinned: ChatMessage[]; ordinary: ChatMessage[] } {
    const pinned = messages.filter((message) => message.role === "system" || message.role === "developer");
    const ordinary = messages.filter((message) => message.role !== "system" && message.role !== "developer");
    return { pinned, ordinary };
  }

  async #partitionOrdinary(
    ordinary: readonly ChatMessage[],
    recentTokenBudget: number,
  ): Promise<{ historical: ChatMessage[]; recent: ChatMessage[] }> {
    let recentStart = ordinary.length;
    let recentTokens = 0;
    while (recentStart > 0) {
      const candidate = ordinary[recentStart - 1];
      if (!candidate) break;
      const candidateTokens = await this.#tokens.countMessages([candidate]);
      if (recentStart < ordinary.length && recentTokens + candidateTokens > recentTokenBudget) break;
      recentTokens += candidateTokens;
      recentStart--;
    }
    recentStart = this.#moveToSafeBoundary(ordinary, recentStart);
    return { historical: ordinary.slice(0, recentStart), recent: ordinary.slice(recentStart) };
  }

  #moveToSafeBoundary(messages: readonly ChatMessage[], initial: number): number {
    let index = initial;
    while (index > 0) {
      const current = messages[index];
      if (!current || current.role !== "tool") break;
      index--;
    }
    const previous = messages[index - 1];
    if (previous?.role === "assistant" && previous.toolCalls?.length) index--;
    return Math.max(0, index);
  }

  #prepareHistorical(historical: readonly ChatMessage[]): ChatMessage[] {
    return historical.map((message) => {
      if (message.role !== "tool" || message.content.length <= this.#policy.maxHistoricalToolChars) return message;
      const limit = this.#policy.maxHistoricalToolChars;
      return { ...message, content: message.content.slice(0, limit) + `\n\n[tool output truncated for compaction: ${message.content.length - limit} chars omitted]` };
    });
  }

  #checkpointAsContextMessage(checkpoint: CompactCheckpoint): ChatMessage {
    return {
      id: `compact:${randomUUID()}`,
      role: "assistant",
      content: ["<conversation_checkpoint>", JSON.stringify(checkpoint), "</conversation_checkpoint>"].join("\n"),
      metadata: { internal: true, kind: "compact-checkpoint" },
    };
  }
}

function withModelCall(usage: import("../types.js").Usage): import("../types.js").Usage {
  return { ...usage, modelCalls: usage.modelCalls ?? 1 };
}
