import { createHash } from "node:crypto";
import { AutoCompactor } from "./autoCompactor.js";
import type { ChatMessage, CompactCheckpoint, ModelCapability } from "./types.js";
import type { Usage } from "../types.js";

export class ContextAssembler {
  readonly #compactor: AutoCompactor;
  #checkpoint: CompactCheckpoint | undefined;
  #compactedOrdinaryCount = 0;
  #sourceMessageCount = 0;
  #sourceDigest: string | undefined;

  constructor(compactor: AutoCompactor) { this.#compactor = compactor; }

  async assemble(input: { transcript: readonly ChatMessage[]; optimizedTranscript?: readonly ChatMessage[]; capability: ModelCapability }): Promise<{ messages: ChatMessage[]; compacted: boolean; tokenUsage: { before: number; after: number }; auxiliaryUsage?: Usage }> {
    this.#reconcileTranscript(input.transcript);
    const source = input.optimizedTranscript ?? input.transcript;
    const pinned = source.filter((message) => message.role === "system" || message.role === "developer");
    const ordinary = source.filter((message) => message.role !== "system" && message.role !== "developer");
    const canonicalPinned = input.transcript.filter((message) => message.role === "system" || message.role === "developer");
    const canonicalOrdinary = input.transcript.filter((message) => message.role !== "system" && message.role !== "developer");
    const pending = ordinary.slice(this.#compactedOrdinaryCount);
    const canonicalPending = canonicalOrdinary.slice(this.#compactedOrdinaryCount);
    const result = await this.#compactor.buildContext({
      messages: [...pinned, ...pending],
      checkpointMessages: [...canonicalPinned, ...canonicalPending],
      capability: input.capability,
      ...(this.#checkpoint ? { previousCheckpoint: this.#checkpoint } : {}),
    });
    if (result.checkpoint) this.#checkpoint = result.checkpoint;
    this.#compactedOrdinaryCount += result.consumedMessages;
    return {
      messages: result.context,
      compacted: result.compacted,
      tokenUsage: { before: result.beforeTokens, after: result.afterTokens },
      ...(result.auxiliaryUsage ? { auxiliaryUsage: result.auxiliaryUsage } : {}),
    };
  }

  get checkpoint(): CompactCheckpoint | undefined { return this.#checkpoint; }
  restoreCheckpoint(checkpoint: CompactCheckpoint | undefined): void {
    if (checkpoint) this.#checkpoint = checkpoint;
    else this.#checkpoint = undefined;
  }

  reset(): void {
    this.#compactor.resetCostAccounting();
    this.#checkpoint = undefined;
    this.#compactedOrdinaryCount = 0;
    this.#sourceMessageCount = 0;
    this.#sourceDigest = undefined;
  }

  #reconcileTranscript(transcript: readonly ChatMessage[]): void {
    // Request-only system/developer messages are allowed to move or change
    // between model rounds (capability snapshots, repair guidance, etc.).
    // Checkpoints summarize ordinary conversation/tool history only, so
    // pinned-message churn must not reset the cumulative cost accounting or
    // discard an otherwise valid checkpoint.
    const ordinary = transcript.filter((message) => message.role !== "system" && message.role !== "developer");
    const isExtension = this.#sourceDigest === undefined
      || (this.#sourceMessageCount <= ordinary.length
        && transcriptDigest(ordinary.slice(0, this.#sourceMessageCount)) === this.#sourceDigest);
    if (!isExtension) {
      this.#checkpoint = undefined;
      this.#compactedOrdinaryCount = 0;
      this.#compactor.resetCostAccounting();
    }
    this.#sourceMessageCount = ordinary.length;
    this.#sourceDigest = transcriptDigest(ordinary);
  }
}

function transcriptDigest(messages: readonly ChatMessage[]): string {
  const hash = createHash("sha256");
  for (const message of messages) {
    hash.update(JSON.stringify([
      message.role,
      message.content,
      message.name ?? null,
      message.toolCallId ?? null,
      message.toolCalls ?? [],
    ]));
    hash.update("\n");
  }
  return hash.digest("hex");
}
