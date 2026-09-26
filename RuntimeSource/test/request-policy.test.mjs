import assert from "node:assert/strict";
import test from "node:test";

import { createRequestCapabilityRegistry } from "../dist/request-capabilities.js";
import { withReasoningPolicy } from "../dist/request-policy.js";

function request(model, purpose, providerOptions) {
  return {
    model,
    messages: [{ role: "user", content: "hello" }],
    ...(purpose ? { metadata: { purpose } } : {}),
    ...(providerOptions ? { providerOptions } : {}),
  };
}

test("GPT-5.6 auxiliary reasoning requests use the supported lowest effort", () => {
  for (const purpose of ["session-title", "context-compaction"]) {
    const prepared = withReasoningPolicy(request("openai/gpt-5.6", purpose));
    assert.deepEqual(prepared.providerOptions?.reasoning, { effort: "none", summary: "auto" });
  }
});

test("older Responses model families keep the legacy minimal auxiliary effort", () => {
  const prepared = withReasoningPolicy(request("openai/gpt-5.5", "context-compaction"));
  assert.deepEqual(prepared.providerOptions?.reasoning, { effort: "minimal", summary: "auto" });
});

test("ordinary agent requests default to low effort", () => {
  const prepared = withReasoningPolicy(request("opencode-go/muse-spark-1.2-contributor"));
  assert.deepEqual(prepared.providerOptions?.reasoning, { effort: "low", summary: "auto" });
});


test("request capability registry exposes no agent hierarchy marker", () => {
  const capabilities = createRequestCapabilityRegistry();
  assert.equal(capabilities.list().some((capability) => capability.id === "lead"), false);
  assert.equal(capabilities.list().some((capability) => capability.id === "context-mode"), false);
});

test("reasoning policy preserves explicit options and unsupported providers", () => {
  const explicit = request("openai/gpt-5.6", "context-compaction", {
    reasoning: { effort: "high", summary: "detailed" },
    service_tier: "priority",
  });
  assert.equal(withReasoningPolicy(explicit), explicit);

  const anthropic = request("anthropic/claude-sonnet-4-5", "context-compaction");
  assert.equal(withReasoningPolicy(anthropic), anthropic);
});
