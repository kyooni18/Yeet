import assert from "node:assert/strict";
import test from "node:test";

import {
  ApproximateTokenCounter,
  AutoCompactor,
  ModelCompactLLM,
  ContextAssembler,
  parseCheckpoint,
  RequestCompactor,
  serializeCompactInput,
} from "../dist/compact/index.js";

function message(id, role, content, extra = {}) {
  return { id, role, content, ...extra };
}

test("ApproximateTokenCounter accounts for UTF-8 width in multilingual text", async () => {
  const counter = new ApproximateTokenCounter();
  const ascii = await counter.countText("a".repeat(320));
  const korean = await counter.countText("한".repeat(320));

  assert.equal(ascii, 100);
  assert.equal(korean, 300);
  assert.ok(korean > ascii * 2);
});

test("checkpoint input uses compact JSON without pretty-print whitespace", () => {
  const serialized = serializeCompactInput({
    messages: [message("u1", "user", "hello"), message("a1", "assistant", "world")],
  });
  assert.equal(serialized.includes("\n"), false);
  assert.equal(serialized.includes("  \""), false);
  const parsed = JSON.parse(serialized);
  assert.equal(parsed.conversation.length, 2);
  assert.deepEqual(parsed.conversation[0], { role: "user", content: "hello" });
});

test("parseCheckpoint bounds durable state instead of allowing unbounded growth", () => {
  const checkpoint = parseCheckpoint(JSON.stringify({
    createdAt: "now",
    goal: "g".repeat(1_000),
    decisions: Array.from({ length: 30 }, (_, index) => `decision-${index}-` + "x".repeat(300)),
    constraints: Array.from({ length: 30 }, (_, index) => `constraint-${index}`),
    completed: Array.from({ length: 30 }, (_, index) => `completed-${index}`),
    pending: Array.from({ length: 30 }, (_, index) => `pending-${index}`),
    failures: Array.from({ length: 30 }, (_, index) => `failure-${index}`),
    files: Array.from({ length: 30 }, (_, index) => ({
      path: `src/${index}/` + "p".repeat(300),
      reason: "r".repeat(300),
      symbols: Array.from({ length: 10 }, (_, symbol) => `symbol-${symbol}-` + "s".repeat(100)),
    })),
    summary: "s".repeat(3_000),
  }));

  assert.ok(checkpoint.goal.length <= 300);
  assert.equal(checkpoint.decisions.length, 8);
  assert.equal(checkpoint.constraints.length, 8);
  assert.equal(checkpoint.completed.length, 12);
  assert.equal(checkpoint.pending.length, 8);
  assert.equal(checkpoint.failures.length, 6);
  assert.equal(checkpoint.files.length, 16);
  assert.ok(checkpoint.decisions.every((item) => [...item].length <= 120));
  assert.ok(checkpoint.files.every((file) => [...file.path].length <= 180 && file.symbols.length <= 4));
  assert.ok(checkpoint.summary.length <= 800);
});

test("AutoCompactor preserves pinned and recent messages while checkpointing history", async () => {
  const messages = [message("s", "system", "system")];
  for (let index = 0; index < 24; index++) messages.push(message(`u${index}`, "user", `turn ${index} ${"x".repeat(120)}`));
  const compactor = new AutoCompactor({
    tokens: new ApproximateTokenCounter(),
    llm: {
      async createCheckpoint({ messages: historical }) {
        assert.ok(historical.length > 0);
        return { checkpoint: {
          version: 1,
          createdAt: "2026-08-30T00:00:00Z",
          goal: "continue",
          decisions: [], constraints: [], completed: [], pending: [], files: [], failures: [], summary: "older turns",
        } };
      },
    },
    policy: { triggerRatio: 0, reservedOutputTokens: 0, reservedToolTokens: 0, safetyMarginTokens: 0, recentTokens: 800 },
  });

  const result = await compactor.buildContext({ messages, capability: { contextWindow: 10_000 } });
  assert.equal(result.compacted, true);
  assert.equal(result.context[0].role, "system");
  assert.match(result.context[1].content, /conversation_checkpoint/);
  assert.ok(result.context.length < messages.length);
  assert.ok(result.context.at(-1).id === messages.at(-1).id);
  assert.ok(result.consumedMessages > 0);
});

test("AutoCompactor keeps an assistant tool call with its following tool result", async () => {
  const messages = [message("s", "system", "system")];
  for (let index = 0; index < 9; index++) messages.push(message(`u${index}`, "user", `turn ${index}`));
  messages.push(message("a-tool", "assistant", "", { toolCalls: [{ id: "call", name: "read", arguments: "{}" }] }));
  messages.push(message("tool", "tool", "result", { toolCallId: "call" }));
  for (let index = 0; index < 11; index++) messages.push(message(`tail${index}`, "user", `tail ${index}`));

  let historicalIds = [];
  const compactor = new AutoCompactor({
    tokens: new ApproximateTokenCounter(),
    llm: {
      async createCheckpoint({ messages: historical }) {
        historicalIds = historical.map((item) => item.id);
        return { checkpoint: { version: 1, createdAt: "now", goal: null, decisions: [], constraints: [], completed: [], pending: [], files: [], failures: [], summary: "summary" } };
      },
    },
    policy: { triggerRatio: 0, reservedOutputTokens: 0, reservedToolTokens: 0, safetyMarginTokens: 0, recentTokens: 100 },
  });

  const result = await compactor.buildContext({ messages, capability: { contextWindow: 10_000 } });
  assert.equal(result.compacted, true);
  const callHistorical = historicalIds.includes("a-tool");
  const resultHistorical = historicalIds.includes("tool");
  assert.equal(callHistorical, resultHistorical);
  const callRecent = result.context.some((item) => item.id === "a-tool");
  const resultRecent = result.context.some((item) => item.id === "tool");
  assert.equal(callRecent, resultRecent);
});

test("ContextAssembler only re-compacts history added after the previous checkpoint", async () => {
  const historicalBatches = [];
  const compactor = new AutoCompactor({
    tokens: new ApproximateTokenCounter(),
    llm: {
      async createCheckpoint({ previous, messages: historical }) {
        historicalBatches.push(historical.map((item) => item.id));
        return { checkpoint: {
          version: 1,
          createdAt: `checkpoint-${historicalBatches.length}`,
          goal: previous?.goal ?? "continue",
          decisions: [], constraints: [], completed: [], pending: [], files: [], failures: [],
          summary: `batch ${historicalBatches.length}`,
        } };
      },
    },
    policy: {
      triggerRatio: 0,
      reservedOutputTokens: 0,
      reservedToolTokens: 0,
      safetyMarginTokens: 0,
      recentTokens: 30,
      minimumMessages: 0,
    },
  });
  const assembler = new ContextAssembler(compactor);
  const first = [message("s", "system", "system")];
  for (let index = 0; index < 8; index++) first.push(message(`u${index}`, "user", `turn ${index} ${"x".repeat(150)}`));
  await assembler.assemble({ transcript: first, capability: { contextWindow: 10_000 } });

  const second = [...first, message("u8", "user", "turn 8 " + "x".repeat(150)), message("u9", "user", "turn 9 " + "x".repeat(150))];
  const result = await assembler.assemble({ transcript: second, capability: { contextWindow: 10_000 } });

  assert.equal(historicalBatches.length, 2);
  assert.ok(historicalBatches[0].length > 0);
  assert.ok(historicalBatches[1].length > 0);
  assert.equal(historicalBatches[1].some((id) => historicalBatches[0].includes(id)), false);
  assert.match(result.messages[1].content, /batch 2/);
});

test("ContextAssembler resets its checkpoint when the source transcript is replaced", async () => {
  const previousValues = [];
  const assembler = new ContextAssembler(new AutoCompactor({
    tokens: new ApproximateTokenCounter(),
    llm: {
      async createCheckpoint({ previous }) {
        previousValues.push(previous?.summary ?? null);
        return { checkpoint: { version: 1, createdAt: "now", goal: null, decisions: [], constraints: [], completed: [], pending: [], files: [], failures: [], summary: "checkpoint" } };
      },
    },
    policy: { triggerRatio: 0, reservedOutputTokens: 0, reservedToolTokens: 0, safetyMarginTokens: 0, recentTokens: 10, minimumMessages: 0 },
  }));
  const transcript = [message("s", "system", "system")];
  for (let index = 0; index < 4; index++) transcript.push(message(`a${index}`, "user", `alpha ${index}`));
  await assembler.assemble({ transcript, capability: { contextWindow: 10_000 } });

  const replacement = [message("s", "system", "system")];
  for (let index = 0; index < 4; index++) replacement.push(message(`b${index}`, "user", `beta ${index}`));
  await assembler.assemble({ transcript: replacement, capability: { contextWindow: 10_000 } });
  assert.deepEqual(previousValues, [null, null]);
});

test("ContextAssembler keeps cumulative compaction state across request-only pinned churn", async () => {
  let checkpoints = 0;
  const assembler = new ContextAssembler(new AutoCompactor({
    tokens: new ApproximateTokenCounter(),
    llm: {
      async createCheckpoint() {
        checkpoints += 1;
        return { checkpoint: { version: 1, createdAt: "now", goal: null, decisions: [], constraints: [], completed: [], pending: [], files: [], failures: [], summary: "checkpoint" } };
      },
    },
    policy: {
      triggerRatio: 1,
      reservedOutputTokens: 0,
      reservedToolTokens: 0,
      safetyMarginTokens: 0,
      recentTokens: 10,
      minimumMessages: 1_000,
      economicalContextTokens: 100_000,
      cumulativeTriggerTokens: 30,
      cumulativeMinimumRequests: 3,
    },
  }));

  const ordinary = [];
  for (let index = 0; index < 3; index++) {
    ordinary.push(message(`u${index}`, "user", `ordinary-${index} ${"x".repeat(500)}`));
    await assembler.assemble({
      transcript: [
        message("base", "system", "base system"),
        ...ordinary,
        message(`ephemeral-${index}`, "system", `capability snapshot ${index}`),
      ],
      capability: { contextWindow: 100_000 },
    });
  }

  assert.equal(checkpoints, 1);
});

test("RequestCompactor preserves source reads when no compaction checkpoint is available", async () => {
  const messages = [{ role: "system", content: "system" }];
  for (let index = 0; index < 6; index++) {
    const id = `read-${index}`;
    messages.push({ role: "assistant", toolCalls: [{ id, name: "read_file", arguments: { path: `f${index}.ts`, startLine: 1, endLine: 200 } }] });
    messages.push({
      role: "tool",
      toolCallId: id,
      name: "read_file",
      content: JSON.stringify({ path: `f${index}.ts`, snapshot: `s${index}`, startLine: 1, endLine: 200, totalLines: 500, lines: "x".repeat(10_000) }),
    });
  }
  const compactor = new RequestCompactor({
    contextLength: async () => undefined,
    complete: async () => { throw new Error("compact model should not be called"); },
  });
  const result = await compactor.compact({ model: "openai/gpt-5.6", messages });
  const oldTool = result.messages.find((item) => item.toolCallId === "read-0");
  const oldNearTool = result.messages.find((item) => item.toolCallId === "read-3");
  const previousRoundTool = result.messages.find((item) => item.toolCallId === "read-4");
  const recentTool = result.messages.find((item) => item.toolCallId === "read-5");
  assert.equal(oldTool.content.includes('"lines"'), true);
  assert.equal(oldTool.content.includes("contentOmitted"), false);
  assert.equal(oldNearTool.content.includes('"lines"'), true);
  assert.equal(previousRoundTool.content.includes('"lines"'), true);
  assert.equal(recentTool.content.includes('"lines"'), true);
  assert.deepEqual(result.messages[1].toolCalls[0].arguments, { path: "f0.ts", startLine: 1, endLine: 200 });
});

test("RequestCompactor keeps request-only overlays outside the reusable cache prefix", async () => {
  const compactor = new RequestCompactor({
    contextLength: async () => undefined,
    complete: async () => { throw new Error("compact model should not be called"); },
  });
  const result = await compactor.compactDetailed({
    model: "openai/gpt-5.6",
    contextKey: "session-cache-key",
    promptCache: true,
    messages: [
      { role: "system", content: "stable system" },
      { role: "user", content: "inspect" },
      { role: "assistant", toolCalls: [{ id: "read-1", name: "read_file", arguments: { path: "a.rs" } }] },
      { role: "tool", toolCallId: "read-1", name: "read_file", content: "source" },
      { role: "system", content: "capability snapshot changes each round", requestOnly: true },
      { role: "user", content: "retry guidance", requestOnly: true },
    ],
  });

  assert.equal(result.request.messages.at(-3).role, "tool");
  assert.equal(result.request.messages.at(-3).cacheBreakpoint, true);
  assert.equal(result.request.messages.at(-2).requestOnly, true);
  assert.equal(result.request.messages.at(-1).requestOnly, true);
  assert.equal(result.request.messages.at(-2).cacheBreakpoint, undefined);
  assert.equal(result.request.messages.at(-1).cacheBreakpoint, undefined);
});

test("RequestCompactor preserves a caller-selected stable cache prefix", async () => {
  const compactor = new RequestCompactor({
    contextLength: async () => undefined,
    complete: async () => { throw new Error("compact model should not be called"); },
  });
  const result = await compactor.compactDetailed({
    model: "openai/gpt-5.6-luna",
    contextKey: "session-cache-key",
    promptCache: true,
    messages: [
      { role: "system", content: "stable system" },
      { role: "user", content: "fix it", cacheBreakpoint: true },
      { role: "assistant", toolCalls: [{ id: "read-1", name: "read_file", arguments: { path: "a.rs" } }] },
      { role: "tool", toolCallId: "read-1", name: "read_file", content: "changing tool result" },
      { role: "system", content: "changing overlay", requestOnly: true, cacheBreakpoint: true },
    ],
  });

  assert.equal(result.request.messages[1].cacheBreakpoint, true);
  assert.equal(result.request.messages[3].cacheBreakpoint, undefined);
  assert.equal(result.request.messages[4].cacheBreakpoint, undefined);
});

test("RequestCompactor deterministically thins old source after four unknown-window requests", async () => {
  const messages = [{ role: "system", content: "system" }];
  for (let index = 0; index < 6; index++) {
    const id = `read-${index}`;
    messages.push({ role: "assistant", toolCalls: [{ id, name: "read_file", arguments: { path: `f${index}.ts`, startLine: 1, endLine: 160 } }] });
    messages.push({
      role: "tool",
      toolCallId: id,
      name: "read_file",
      content: JSON.stringify({ path: `f${index}.ts`, snapshot: `s${index}`, startLine: 1, endLine: 160, totalLines: 500, lines: "source".repeat(2_000) }),
    });
  }
  const compactor = new RequestCompactor({
    contextLength: async () => undefined,
    complete: async () => { throw new Error("compact model should not be called without a context window"); },
  });

  let result;
  for (let attempt = 0; attempt < 4; attempt++) {
    result = await compactor.compact({ model: "custom/model", contextKey: "session", messages });
  }

  const oldest = result.messages.find((item) => item.toolCallId === "read-0");
  const previousRound = result.messages.find((item) => item.toolCallId === "read-4");
  const newest = result.messages.find((item) => item.toolCallId === "read-5");
  assert.equal(oldest.content.includes('"lines"'), false);
  assert.equal(oldest.content.includes("contentOmitted"), true);
  assert.equal(previousRound.content.includes('"lines"'), true);
  assert.equal(newest.content.includes('"lines"'), true);
});

test("RequestCompactor fails open when checkpoint generation fails and omits responseFormat provider option", async () => {
  const messages = [
    { role: "system", content: "system" },
    { role: "user", content: "fix it" },
    { role: "assistant", toolCalls: [{ id: "read-1", name: "read_file", arguments: { path: "copy.swift" } }] },
    {
      role: "tool",
      toolCallId: "read-1",
      name: "read_file",
      content: JSON.stringify({ path: "copy.swift", snapshot: "s1", startLine: 1, endLine: 160, totalLines: 694, lines: "source ".repeat(2_000) }),
    },
  ];
  for (let index = 0; index < 16; index++) {
    messages.push({ role: index % 2 === 0 ? "user" : "assistant", content: "context ".repeat(900) });
  }

  let auxiliaryRequest;
  const compactor = new RequestCompactor({
    contextLength: async () => 16_000,
    complete: async (request) => {
      auxiliaryRequest = request;
      throw new Error("simulated provider rejection");
    },
  });

  const result = await compactor.compactDetailed({
    model: "opencode-go/muse-spark-1.2-contributor",
    contextKey: "session",
    messages,
  });

  assert.ok(auxiliaryRequest, "checkpoint generation should have been attempted");
  assert.equal(auxiliaryRequest.providerOptions?.responseFormat, undefined);
  const source = result.request.messages.find((item) => item.toolCallId === "read-1");
  assert.ok(source.content.includes('"lines"'));
  assert.equal(source.content.includes("contentOmitted"), false);
});

test("RequestCompactor preserves every result from the latest parallel tool round", async () => {
  const messages = [
    { role: "system", content: "system" },
    { role: "user", content: "inspect both files" },
    {
      role: "assistant",
      toolCalls: [
        { id: "copy", name: "read_file", arguments: { path: "copy.swift" } },
        { id: "main", name: "read_file", arguments: { path: "main.md" } },
      ],
    },
    {
      role: "tool",
      toolCallId: "copy",
      name: "read_file",
      content: JSON.stringify({ path: "copy.swift", snapshot: "s1", startLine: 1, endLine: 160, totalLines: 694, lines: "source".repeat(1_500) }),
    },
    {
      role: "tool",
      toolCallId: "main",
      name: "read_file",
      content: JSON.stringify({ path: "main.md", snapshot: "s2", startLine: 1, endLine: 0, totalLines: 0, lines: "" }),
    },
  ];
  const compactor = new RequestCompactor({
    contextLength: async () => undefined,
    complete: async () => { throw new Error("compact model should not be called"); },
  });

  const result = await compactor.compact({ model: "rainy/local", messages });
  const copy = result.messages.find((item) => item.toolCallId === "copy");
  const main = result.messages.find((item) => item.toolCallId === "main");
  assert.equal(copy.content.includes('"lines"'), true);
  assert.equal(copy.content.includes("contentOmitted"), false);
  assert.equal(main.content.includes('"lines"'), true);
});

test("RequestCompactor checkpoints canonical read source instead of contentOmitted metadata", async () => {
  const messages = [{ role: "system", content: "system" }];
  for (let index = 0; index < 9; index++) {
    const id = `read-${index}`;
    messages.push({
      role: "assistant",
      toolCalls: [{ id, name: "read_file", arguments: { path: `f${index}.swift`, startLine: 1, endLine: 160 } }],
    });
    messages.push({
      role: "tool",
      toolCallId: id,
      name: "read_file",
      content: JSON.stringify({
        path: `f${index}.swift`,
        snapshot: `s${index}`,
        startLine: 1,
        endLine: 160,
        totalLines: 160,
        lines: `SOURCE_${index}_` + "x".repeat(12_000),
      }),
    });
  }

  const compactRequests = [];
  const compactor = new RequestCompactor({
    contextLength: async () => 24_000,
    complete: async (request) => {
      compactRequests.push(request);
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

  await compactor.compact({ model: "rainy/local", messages });
  assert.equal(compactRequests.length, 1);
  const checkpointInput = compactRequests[0].messages.at(-1).content;
  assert.ok(checkpointInput.includes("SOURCE_0_"));
  assert.equal(checkpointInput.includes("contentOmitted"), false);
});

test("AutoCompactor retains a token-budgeted suffix instead of a fixed message count", async () => {
  const messages = [message("s", "system", "system")];
  for (let index = 0; index < 20; index++) {
    messages.push(message(`u${index}`, "user", index >= 18 ? "x".repeat(1_600) : "tiny"));
  }
  const compactor = new AutoCompactor({
    tokens: new ApproximateTokenCounter(),
    llm: {
      async createCheckpoint() {
        return { checkpoint: { version: 1, createdAt: "now", goal: null, decisions: [], constraints: [], completed: [], pending: [], files: [], failures: [], summary: "summary" } };
      },
    },
    policy: {
      triggerRatio: 0,
      reservedOutputTokens: 0,
      reservedToolTokens: 0,
      safetyMarginTokens: 0,
      recentTokens: 600,
      minimumMessages: 0,
    },
  });

  const result = await compactor.buildContext({ messages, capability: { contextWindow: 10_000 } });
  const retained = result.context.filter((item) => item.role !== "system" && !item.id.startsWith("compact:"));
  assert.ok(retained.length <= 2);
  assert.equal(retained.at(-1).id, "u19");
});

test("RequestCompactor reports checkpoint model usage", async () => {
  const messages = [{ role: "system", content: "system" }];
  for (let index = 0; index < 80; index++) messages.push({ role: "user", content: `turn ${index} ${"x".repeat(2_000)}` });
  const compactor = new RequestCompactor({
    contextLength: async () => 8_000,
    complete: async () => ({
      text: JSON.stringify({
        version: 1,
        createdAt: "now",
        goal: null,
        decisions: [], constraints: [], completed: [], pending: [], files: [], failures: [], summary: "summary",
      }),
      usage: { inputTokens: 123, outputTokens: 45, totalTokens: 168 },
    }),
  });

  const result = await compactor.compactDetailed({ model: "openai/gpt-5.6", messages });
  assert.equal(result.auxiliaryUsage.inputTokens, 123);
  assert.equal(result.auxiliaryUsage.outputTokens, 45);
  assert.equal(result.auxiliaryUsage.modelCalls, 1);
});

test("RequestCompactor always uses the lead request model for checkpoints", async () => {
  const compactRequests = [];
  const compactor = new RequestCompactor({
    contextLength: async () => 10_000,
    complete: async (request) => {
      compactRequests.push(request);
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
  await compactor.compact({ model: "openrouter/example/model", messages });

  assert.equal(compactRequests.length, 1);
  assert.equal(compactRequests[0].model, "openrouter/example/model");
  assert.equal(compactRequests[0].metadata?.purpose, "context-compaction");
});

test("AutoCompactor triggers on cumulative request cost below the per-request context ceiling", async () => {
  let checkpoints = 0;
  const compactor = new AutoCompactor({
    tokens: new ApproximateTokenCounter(),
    llm: {
      async createCheckpoint() {
        checkpoints += 1;
        return { checkpoint: { version: 1, createdAt: "now", goal: null, decisions: [], constraints: [], completed: [], pending: [], files: [], failures: [], summary: "summary" } };
      },
    },
    policy: {
      triggerRatio: 0.95,
      reservedOutputTokens: 0,
      reservedToolTokens: 0,
      safetyMarginTokens: 0,
      recentTokens: 500,
      minimumMessages: 100,
      economicalContextTokens: 100_000,
      cumulativeTriggerTokens: 2_000,
      cumulativeMinimumRequests: 3,
    },
  });
  const messages = [message("s", "system", "system")];
  for (let index = 0; index < 8; index++) messages.push(message(`u${index}`, "user", `turn ${index} ${"x".repeat(300)}`));

  const first = await compactor.buildContext({ messages, capability: { contextWindow: 100_000 } });
  const second = await compactor.buildContext({ messages, capability: { contextWindow: 100_000 } });
  const third = await compactor.buildContext({ messages, capability: { contextWindow: 100_000 } });

  assert.equal(first.compacted, false);
  assert.equal(second.compacted, false);
  assert.equal(third.compacted, true);
  assert.equal(checkpoints, 1);
});

test("AutoCompactor includes repeated tool-schema overhead in cumulative cost", async () => {
  let checkpoints = 0;
  const compactor = new AutoCompactor({
    tokens: new ApproximateTokenCounter(),
    llm: {
      async createCheckpoint() {
        checkpoints += 1;
        return { checkpoint: { version: 1, createdAt: "now", goal: null, decisions: [], constraints: [], completed: [], pending: [], files: [], failures: [], summary: "summary" } };
      },
    },
    policy: {
      triggerRatio: 0.99,
      reservedOutputTokens: 0,
      reservedToolTokens: 0,
      safetyMarginTokens: 0,
      recentTokens: 200,
      minimumMessages: 100,
      economicalContextTokens: 100_000,
      cumulativeTriggerTokens: 3_000,
      cumulativeMinimumRequests: 3,
    },
  });
  const messages = [message("s", "system", "system")];
  for (let index = 0; index < 8; index++) messages.push(message(`u${index}`, "user", `turn ${index} ${"x".repeat(100)}`));
  const capability = { contextWindow: 100_000, requestOverheadTokens: 900 };

  const first = await compactor.buildContext({ messages, capability });
  const second = await compactor.buildContext({ messages, capability });
  const third = await compactor.buildContext({ messages, capability });

  assert.equal(first.compacted, false);
  assert.equal(second.compacted, false);
  assert.equal(third.compacted, true);
  assert.equal(checkpoints, 1);
});

test("AutoCompactor honors an explicit output-token budget when computing usable context", async () => {
  let checkpoints = 0;
  const compactor = new AutoCompactor({
    tokens: new ApproximateTokenCounter(),
    llm: {
      async createCheckpoint() {
        checkpoints += 1;
        return { checkpoint: { version: 1, createdAt: "now", goal: null, decisions: [], constraints: [], completed: [], pending: [], files: [], failures: [], summary: "summary" } };
      },
    },
    policy: {
      triggerRatio: 0.7,
      reservedOutputTokens: 8_192,
      reservedToolTokens: 0,
      safetyMarginTokens: 0,
      recentTokens: 500,
      minimumMessages: 0,
      economicalContextTokens: 100_000,
      cumulativeTriggerTokens: 100_000,
      cumulativeMinimumRequests: 100,
    },
  });
  const messages = [message("s", "system", "system")];
  for (let index = 0; index < 10; index++) messages.push(message(`u${index}`, "user", "x".repeat(700)));

  const explicitSmallOutput = await compactor.buildContext({
    messages,
    capability: { contextWindow: 12_000, maxOutputTokens: 256 },
  });

  assert.equal(explicitSmallOutput.compacted, false);
  assert.equal(checkpoints, 0);
});

test("AutoCompactor default cost accounting checkpoints by the fourth repeated medium request", async () => {
  let checkpoints = 0;
  const compactor = new AutoCompactor({
    tokens: new ApproximateTokenCounter(),
    llm: {
      async createCheckpoint() {
        checkpoints += 1;
        return { checkpoint: { version: 1, createdAt: "now", goal: null, decisions: [], constraints: [], completed: [], pending: [], files: [], failures: [], summary: "summary" } };
      },
    },
  });
  const messages = [message("s", "system", "system")];
  for (let index = 0; index < 20; index++) messages.push(message(`u${index}`, "user", `turn ${index} ${"x".repeat(1_400)}`));

  const first = await compactor.buildContext({ messages, capability: { contextWindow: 128_000 } });
  const second = await compactor.buildContext({ messages, capability: { contextWindow: 128_000 } });
  const third = await compactor.buildContext({ messages, capability: { contextWindow: 128_000 } });
  const fourth = await compactor.buildContext({ messages, capability: { contextWindow: 128_000 } });

  assert.equal(first.compacted, false);
  assert.equal(second.compacted, false);
  assert.equal(third.compacted, false);
  assert.equal(fourth.compacted, true);
  assert.equal(checkpoints, 1);
});

test("RequestCompactor keeps a 25-call source-heavy workload well below raw replay volume", async () => {
  const compactor = new RequestCompactor({
    contextLength: async () => 128_000,
    complete: async () => ({
      text: JSON.stringify({
        version: 1,
        createdAt: "now",
        goal: "find bugs",
        decisions: [],
        constraints: [],
        completed: [],
        pending: ["continue focused inspection"],
        files: [],
        failures: [],
        summary: "Preserve established findings and only inspect genuinely uncovered evidence.",
      }),
    }),
  });
  const messages = [
    { role: "system", content: "system" },
    { role: "user", content: "find potential bugs" },
  ];
  let rawReplayChars = 0;
  let compactReplayChars = 0;

  for (let attempt = 0; attempt < 25; attempt++) {
    const id = `read-${attempt}`;
    messages.push({
      role: "assistant",
      toolCalls: [{ id, name: "read_file", arguments: { path: `src/file-${attempt}.ts`, startLine: 1, endLine: 160 } }],
    });
    messages.push({
      role: "tool",
      toolCallId: id,
      name: "read_file",
      content: JSON.stringify({
        path: `src/file-${attempt}.ts`,
        snapshot: `s${attempt}`,
        startLine: 1,
        endLine: 160,
        totalLines: 320,
        lines: `SOURCE_${attempt}_` + "x".repeat(12_000),
      }),
    });
    rawReplayChars += JSON.stringify(messages).length;
    const result = await compactor.compactDetailed({
      model: "opencode-go/muse-spark-1.2-contributor",
      contextKey: "regression-session",
      messages,
    });
    compactReplayChars += JSON.stringify(result.request.messages).length;
  }

  assert.ok(compactReplayChars < rawReplayChars * 0.45, `expected compact replay volume below 45%, got ${(compactReplayChars / rawReplayChars * 100).toFixed(1)}%`);
});

test("recoverable coordinator windows bypass recursive compaction without altering evidence", async () => {
  const compactor = new RequestCompactor({
    contextLength: async () => { throw new Error("coordinator owns the budget"); },
    complete: async () => { throw new Error("must not summarize recoverable windows"); },
  });
  const request = {
    model: "openai/test",
    metadata: { contextManagement: "recoverable-windows", contextWindowId: "window-2" },
    messages: [{ role: "tool", content: "exact compiler evidence ".repeat(20000), toolCallId: "call-1" }],
  };
  const result = await compactor.compactDetailed(request);
  assert.strictEqual(result.request, request);
  assert.equal(result.auxiliaryUsage, undefined);
});

test("RequestCompactor retains images and cache markers when old tool arguments are thinned", async () => {
  const current = { role: "user", content: "inspect screenshot", images: [{ mediaType: "image/png", data: "aGVsbG8=" }], cacheBreakpoint: true };
  const messages = [{ role: "system", content: "system" }, current];
  for (let i = 0; i < 3; i++) {
    messages.push({ role: "assistant", toolCalls: [{ id: `r${i}`, name: "read_file", arguments: { path: "a.txt", refresh: true } }] });
    messages.push({ role: "tool", toolCallId: `r${i}`, name: "read_file", content: "source", toolFeedback: [{ toolCallId: `r${i}`, status: "completed" }] });
  }
  const compactor = new RequestCompactor({ contextLength: async () => 100_000, complete: async () => { throw new Error("unexpected checkpoint"); } });
  const result = await compactor.compact({ model: "openai/test", messages, promptCache: true });
  assert.deepEqual(result.messages[1], current);
  assert.deepEqual(result.messages.at(-1).toolFeedback, messages.at(-1).toolFeedback);
  assert.equal(result.messages.at(-1).cacheBreakpoint, undefined);
  assert.equal(result.messages[2].toolCalls[0].arguments.refresh, undefined);
});

test("RequestCompactor budgets request-only overlays without summarizing them", async () => {
  let checkpointRequest;
  const compactor = new RequestCompactor({
    contextLength: async () => 20_000,
    complete: async (request) => {
      checkpointRequest = request;
      return { text: JSON.stringify({ version: 1, createdAt: "now", goal: null, decisions: [], constraints: [], completed: [], pending: [], files: [], failures: [], summary: "summary" }) };
    },
  });
  const messages = Array.from({ length: 10 }, (_, i) => ({ role: i % 2 ? "assistant" : "user", content: "context ".repeat(180) }));
  const overlay = { role: "system", content: "OVERLAY " + "z".repeat(32_000), requestOnly: true };
  const current = { role: "user", content: "current screenshot", images: [{ mediaType: "image/png", data: "aGVsbG8=" }], cacheBreakpoint: true };
  messages.push(current, overlay);
  const result = await compactor.compact({ model: "openai/test", messages, maxTokens: 1_000, promptCache: true });
  assert.ok(checkpointRequest, "overlay must count toward the input budget");
  assert.equal(JSON.stringify(checkpointRequest).includes("OVERLAY"), false);
  assert.deepEqual(result.messages.at(-1), overlay);
  assert.deepEqual(result.messages.at(-2), current);
  assert.ok(result.messages.some((message) => message.content?.includes("<conversation_checkpoint>")));
});

test("RequestCompactor preserves already compacted search and edit evidence", async () => {
  const messages = [
    { role: "assistant", toolCalls: [{ id: "search", name: "search_workspace", arguments: { query: "needle" } }, { id: "edit", name: "apply_file_edits", arguments: { changes: [{ path: "a", editCount: 3 }], historical: true } }] },
    { role: "tool", toolCallId: "search", name: "search_workspace", content: JSON.stringify({ historical: true, matchCount: 7, filesScanned: 9 }) },
    { role: "tool", toolCallId: "edit", name: "apply_file_edits", content: "ok" },
  ];
  for (let i = 0; i < 2; i++) {
    messages.push({ role: "assistant", toolCalls: [{ id: `r${i}`, name: "read_file", arguments: { path: "a" } }] });
    messages.push({ role: "tool", toolCallId: `r${i}`, content: "text" });
  }
  const compactor = new RequestCompactor({ contextLength: async () => undefined, complete: async () => { throw new Error("unexpected"); } });
  const result = await compactor.compact({ model: "openai/test", messages });
  assert.equal(JSON.parse(result.messages[1].content).matchCount, 7);
  assert.equal(result.messages[0].toolCalls[1].arguments.changes[0].editCount, 3);
});


test("expanding checkpoints retain evidence and usage without paying again for identical input", async () => {
  let calls = 0;
  const compactor = new AutoCompactor({
    tokens: new ApproximateTokenCounter(),
    llm: { async createCheckpoint() {
      calls++;
      return { checkpoint: { version: 1, createdAt: "now", goal: null, decisions: [], constraints: [], completed: [], pending: [], files: [], failures: [], summary: "x".repeat(4000) }, usage: { inputTokens: 100, outputTokens: 1000 } };
    } },
    policy: { triggerRatio: 0, minimumMessages: 0, recentTokens: 1, reservedOutputTokens: 0, safetyMarginTokens: 0 },
  });
  const messages = [message("a", "user", "old evidence"), message("b", "user", "current task")];
  const first = await compactor.buildContext({ messages, capability: { contextWindow: 10000 } });
  assert.deepEqual(first.context, messages);
  assert.equal(first.consumedMessages, 0);
  assert.equal(first.compacted, false);
  assert.equal(first.afterTokens, first.beforeTokens);
  assert.deepEqual(first.auxiliaryUsage, { inputTokens: 100, outputTokens: 1000, modelCalls: 1 });
  const second = await compactor.buildContext({ messages, capability: { contextWindow: 10000 } });
  assert.equal(calls, 1);
  assert.equal(second.auxiliaryUsage, undefined);
});

test("assembler reset clears cumulative request costs", async () => {
  let calls = 0;
  const assembler = new ContextAssembler(new AutoCompactor({
    tokens: new ApproximateTokenCounter(),
    llm: { async createCheckpoint() { calls++; throw new Error("unexpected compaction"); } },
    policy: { minimumMessages: 100, cumulativeMinimumRequests: 2, cumulativeTriggerTokens: 1, recentTokens: 1 },
  }));
  const input = { transcript: [message("a", "user", "a".repeat(100)), message("b", "user", "b".repeat(100))], capability: { contextWindow: 100000 } };
  await assembler.assemble(input);
  assembler.reset();
  await assembler.assemble(input);
  assert.equal(calls, 0);
});

test("structured tool text is thinned using the same content precedence as providers", async () => {
  const messages = [];
  for (let i = 0; i < 3; i++) {
    messages.push({ role: "assistant", toolCalls: [{ id: `t${i}`, name: "custom", arguments: {} }] });
    messages.push({ role: "tool", name: "custom", toolCallId: `t${i}`, toolResult: { content: "HEAD" + "x".repeat(5000) + "TAIL", result: { ignored: true } } });
  }
  const compactor = new RequestCompactor({ contextLength: async () => undefined, complete: async () => { throw new Error("unexpected call"); } });
  const result = await compactor.compact({ model: "test/model", messages });
  assert.match(result.messages[1].content, /^HEAD/);
  assert.match(result.messages[1].content, /TAIL$/);
  assert.ok(result.messages[1].content.length < 2100);
  assert.equal(result.messages[1].toolResult, undefined);
  assert.deepEqual(result.messages.slice(-4), messages.slice(-4));
  assert.equal(messages[1].toolResult.content.length, 5008);
});


test("deterministic thinning never expands small results", async () => {
  const messages = [];
  for (let i = 0; i < 3; i++) {
    messages.push({ role: "assistant", toolCalls: [{ id: `r${i}`, name: "search_workspace", arguments: { query: "x" } }, { id: `e${i}`, name: "apply_file_edits", arguments: { changes: [] } }] });
    messages.push({ role: "tool", toolCallId: `r${i}`, name: "search_workspace", content: "{}" });
    messages.push({ role: "tool", toolCallId: `e${i}`, name: "apply_file_edits", content: "{}" });
  }
  const compactor = new RequestCompactor({ contextLength: async () => undefined, complete: async () => { throw new Error("unexpected call"); } });
  const result = await compactor.compact({ model: "test/model", messages });
  assert.deepEqual(result.messages, messages);
});


test("historical edit diagnostics cannot replay unbounded nested metadata", async () => {
  const original = JSON.stringify({
    files: [{ path: "src/a.rs", snapshot: "exact-snapshot", warnings: [{ detail: "한".repeat(20000) }] }],
    diagnostics: [{ severity: "error", message: "e".repeat(20000), line: 7 }],
  });
  const messages = [
    { role: "assistant", toolCalls: [{ id: "edit", name: "apply_file_edits", arguments: {} }] },
    { role: "tool", name: "apply_file_edits", toolCallId: "edit", content: original },
  ];
  for (let i = 0; i < 2; i++) {
    messages.push({ role: "assistant", toolCalls: [{ id: `r${i}`, name: "read_file", arguments: {} }] });
    messages.push({ role: "tool", name: "read_file", toolCallId: `r${i}`, content: "recent evidence" });
  }
  const compactor = new RequestCompactor({ contextLength: async () => undefined, complete: async () => { throw new Error("unexpected call"); } });
  const result = await compactor.compact({ model: "test/model", messages });
  assert.ok(result.messages[1].content.length < 2000);
  const retained = JSON.parse(result.messages[1].content);
  assert.equal(retained.files[0].path, "src/a.rs");
  assert.equal(retained.files[0].snapshot, "exact-snapshot");
  assert.equal(retained.files[0].warnings.contentOmitted, true);
  assert.equal(retained.diagnostics[0].severity, "error");
  assert.equal(retained.diagnostics[0].line, 7);
  assert.deepEqual(result.messages.slice(-4), messages.slice(-4));
  assert.equal(messages[1].content, original);
});


test("malformed paid checkpoints retain usage and do not repeat for identical history", async () => {
  let calls = 0;
  const compactor = new AutoCompactor({
    tokens: new ApproximateTokenCounter(),
    llm: new ModelCompactLLM({ async complete() {
      calls++;
      return { text: "not valid JSON", usage: { inputTokens: 100, outputTokens: 20 } };
    } }, "test/model"),
    policy: { triggerRatio: 0, minimumMessages: 0, recentTokens: 1, reservedOutputTokens: 0, safetyMarginTokens: 0 },
  });
  const messages = [message("a", "user", "old evidence"), message("b", "user", "current task")];
  const input = { messages, capability: { contextWindow: 10000 } };
  const first = await compactor.buildContext(input);
  assert.deepEqual(first.context, messages);
  assert.equal(first.consumedMessages, 0);
  assert.deepEqual(first.auxiliaryUsage, { inputTokens: 100, outputTokens: 20, modelCalls: 1 });
  const second = await compactor.buildContext(input);
  assert.equal(calls, 1);
  assert.equal(second.auxiliaryUsage, undefined);
  await compactor.buildContext({ ...input, messages: [...messages, message("c", "user", "more evidence")] });
  assert.equal(calls, 2, "new historical input permits another attempt");
});

test("checkpoint transport failures can recover on the next attempt", async () => {
  let calls = 0;
  const compactor = new AutoCompactor({
    tokens: new ApproximateTokenCounter(),
    llm: new ModelCompactLLM({ async complete() {
      calls++;
      if (calls === 1) throw new Error("temporary network failure");
      return { text: JSON.stringify({ summary: "saved" }) };
    } }, "test/model"),
    policy: { triggerRatio: 0, minimumMessages: 0, recentTokens: 1, reservedOutputTokens: 0, safetyMarginTokens: 0 },
  });
  const input = { messages: [message("a", "user", "x".repeat(4000)), message("b", "user", "current")], capability: { contextWindow: 10000 } };
  await assert.rejects(compactor.buildContext(input), /temporary network failure/);
  const result = await compactor.buildContext(input);
  assert.equal(calls, 2);
  assert.equal(result.compacted, true);
});
