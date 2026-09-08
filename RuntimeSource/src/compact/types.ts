export type ChatRole = "system" | "developer" | "user" | "assistant" | "tool";

export interface CompactToolCall {
  id: string;
  name: string;
  arguments: string;
}

export interface ChatMessage {
  id: string;
  role: ChatRole;
  content: string;
  name?: string;
  toolCallId?: string;
  toolCalls?: CompactToolCall[];
  metadata?: Record<string, unknown>;
}

export interface ModelCapability {
  contextWindow: number;
  maxOutputTokens?: number;
  /** Input tokens paid on every request but not represented by chat messages, such as tool schemas. */
  requestOverheadTokens?: number;
}

export interface CompactPolicy {
  triggerRatio: number;
  reservedOutputTokens: number;
  reservedToolTokens: number;
  safetyMarginTokens: number;
  recentTokens: number;
  minimumMessages: number;
  maxHistoricalToolChars: number;
  /** Cost-control ceiling independent of the provider's theoretical window. */
  economicalContextTokens: number;
  /** Repeated-request token volume that triggers compaction below the per-request ceiling. */
  cumulativeTriggerTokens: number;
  /** Do not pay for checkpoint generation on very short exchanges. */
  cumulativeMinimumRequests: number;
}

export interface CompactFileReference {
  path: string;
  reason?: string;
  symbols?: string[];
}

export interface CompactCheckpoint {
  version: 1;
  createdAt: string;
  goal: string | null;
  decisions: string[];
  constraints: string[];
  completed: string[];
  pending: string[];
  files: CompactFileReference[];
  failures: string[];
  summary: string;
}

export interface TokenCounter {
  countMessages(messages: readonly ChatMessage[]): Promise<number>;
  countText(text: string): Promise<number>;
}

export interface CompactLLM {
  createCheckpoint(input: {
    previous?: CompactCheckpoint;
    messages: readonly ChatMessage[];
  }): Promise<{ checkpoint: CompactCheckpoint; usage?: import("../types.js").Usage }>;
}

export interface CompactResult {
  compacted: boolean;
  context: ChatMessage[];
  checkpoint?: CompactCheckpoint;
  beforeTokens: number;
  afterTokens: number;
  consumedMessages: number;
  auxiliaryUsage?: import("../types.js").Usage;
}
