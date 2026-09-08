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
  for (const purpose of ["session-title", "context-compaction", "command-evaluation"]) {
    const prepared = withReasoningPolicy(request("openai/gpt-5.6", purpose));
    assert.deepEqual(prepared.providerOptions?.reasoning, { effort: "none", summary: "auto" });
  }
});

test("older Responses model families keep the legacy minimal auxiliary effort", () => {
  const prepared = withReasoningPolicy(request("openai/gpt-5.5", "command-evaluation"));
  assert.deepEqual(prepared.providerOptions?.reasoning, { effort: "minimal", summary: "auto" });
});

test("lead reasoning requests default to low effort", () => {
  const prepared = withReasoningPolicy(request("opencode-go/muse-spark-1.2-contributor", "lead"));
  assert.deepEqual(prepared.providerOptions?.reasoning, { effort: "low", summary: "auto" });
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

test("context compaction applies the auxiliary reasoning policy before provider dispatch", async () => {
  const requests = [];
  const capabilities = createRequestCapabilityRegistry({
    contextLength: async () => 10_000,
    complete: async (prepared) => {
      requests.push(prepared);
      return {
        text: JSON.stringify({
          version: 1,
          createdAt: "now",
          goal: null,
          decisions: [],
          constraints: [],
          completed: [],
          pending: [],
          files: [],
          failures: [],
          summary: "summary",
        }),
      };
    },
  });
  const messages = Array.from({ length: 40 }, (_, index) => ({
    role: index % 2 === 0 ? "user" : "assistant",
    content: "x".repeat(1_000),
  }));

  await capabilities.prepare({
    model: "openai/gpt-5.6",
    attachedCapabilities: ["context-mode"],
    messages,
  });

  assert.equal(requests.length, 1);
  assert.equal(requests[0].metadata?.purpose, "context-compaction");
  assert.deepEqual(requests[0].providerOptions?.reasoning, { effort: "none", summary: "auto" });
});
