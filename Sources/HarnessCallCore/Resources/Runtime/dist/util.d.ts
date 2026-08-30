import type { FinishReason, Message, ToolCall, Usage } from "./types.js";
export declare function splitSystem(messages: Message[], explicit?: string): {
    system?: string;
    messages: Message[];
};
export declare function safeJsonParse(value: string): unknown;
export declare function normalizeFinishReason(value: unknown): FinishReason;
export declare function usage(input?: number, output?: number, total?: number, cached?: number): Usage | undefined;
export declare function normalizeToolCall(id: string | undefined, name: string | undefined, args: unknown, index: number): ToolCall;
/**
 * Normalize the model-list response shapes used by OpenAI-compatible APIs and
 * Gemini. Provider adapters return provider-local names, so Gemini's
 * `models/` resource prefix is removed here.
 */
export declare function normalizeModelIds(raw: unknown): string[];
//# sourceMappingURL=util.d.ts.map