import type { CallRequest } from "./types.js";
import { parseModelId } from "./types.js";

function auxiliaryReasoningEffort(model: string): "minimal" | "none" {
  // GPT-5.6 dropped the legacy `minimal` effort. Its lowest supported value
  // is `none` (the family also accepts low/medium/high/xhigh/max). Keep the
  // older auxiliary default for earlier/OpenCode-compatible model families.
  const match = /^gpt-(\d+)(?:\.(\d+))?/.exec(model);
  if (!match) return "minimal";
  const major = Number(match[1]);
  const minor = Number(match[2] ?? 0);
  return major > 5 || (major === 5 && minor >= 6) ? "none" : "minimal";
}

/** Apply provider-specific defaults without overriding explicit caller choices. */
export function withReasoningPolicy(request: CallRequest): CallRequest {
  const parsed = parseModelId(request.model);
  // These model families are routed through the Responses protocol in the
  // built-in/OpenCode adapters. Other providers reject an OpenAI `reasoning`
  // field, so leave them untouched.
  if (!["openai", "codex-cli", "opencode", "opencode-go"].includes(parsed.provider)) return request;
  if (!/^(?:gpt-|o\d|muse-spark-|grok-)/.test(parsed.model)) return request;

  const existing = request.providerOptions ?? {};
  if (existing.reasoning !== undefined) return request;
  const purpose = request.metadata?.purpose;
  const effort = purpose === "session-title" || purpose === "context-compaction"
    ? auxiliaryReasoningEffort(parsed.model)
    : "low";
  return {
    ...request,
    // Ask Responses-compatible reasoning models for the summary surface they
    // are allowed to expose. Full reasoning is still forwarded only when a
    // provider explicitly emits plaintext reasoning events/content.
    providerOptions: { ...existing, reasoning: { effort, summary: "auto" } },
  };
}
