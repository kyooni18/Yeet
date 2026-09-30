import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { ANTIGRAVITY_AGENT, AntigravityProvider, AuthManager } from "../dist/index.js";

function jsonResponse(body) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

function sseResponse(events) {
  const body = events
    .map((event) => `event: ${event.event_type}\ndata: ${JSON.stringify(event)}\n\n`)
    .join("") + "event: done\ndata: [DONE]\n\n";
  return new Response(body, {
    status: 200,
    headers: { "content-type": "text/event-stream" },
  });
}

test("Antigravity maps Yeet requests and native function calls", async () => {
  let requestUrl;
  let requestHeaders;
  let sent;
  const provider = new AntigravityProvider({
    apiKey: "gemini-test-key",
    fetch: async (input, init) => {
      requestUrl = String(input);
      requestHeaders = init.headers;
      sent = JSON.parse(init.body);
      const providerToolName = sent.tools[0].name;
      return jsonResponse({
        id: "int_1",
        agent: ANTIGRAVITY_AGENT,
        environment_id: "env_1",
        status: "requires_action",
        steps: [
          { type: "thought", summary: [{ type: "text", text: "Need the local file." }] },
          { type: "function_call", id: "call_1", name: providerToolName, arguments: { path: "README.md" } },
        ],
        usage: {
          total_input_tokens: 10,
          total_output_tokens: 4,
          total_tokens: 20,
          total_cached_tokens: 2,
          total_thought_tokens: 6,
        },
      });
    },
  });

  const result = await provider.complete({
    model: ANTIGRAVITY_AGENT,
    system: "Follow project instructions.",
    messages: [{ role: "user", content: "Inspect the readme." }],
    tools: [{
      name: "read_file",
      description: "Read a local workspace file.",
      inputSchema: {
        type: "object",
        properties: { path: { type: "string" } },
        required: ["path"],
      },
    }],
  });

  assert.equal(requestUrl, "https://generativelanguage.googleapis.com/v1beta/interactions");
  assert.equal(requestHeaders["x-goog-api-key"], "gemini-test-key");
  assert.equal(sent.agent, ANTIGRAVITY_AGENT);
  assert.equal(sent.environment, "remote");
  assert.equal(sent.input[0].type, "text");
  assert.equal(sent.input[0].text, "Inspect the readme.");
  assert.match(sent.system_instruction, /Follow project instructions\./);
  assert.match(sent.system_instruction, /Yeet harness/);
  assert.equal(sent.tools[0].type, "function");
  assert.match(sent.tools[0].name, /^yeet_[0-9a-f]{12}_read_file$/);
  assert.equal(sent.tool_choice.allowed_tools.mode, "auto");
  assert.deepEqual(sent.tool_choice.allowed_tools.tools, [sent.tools[0].name]);

  assert.equal(result.finishReason, "tool_call");
  assert.equal(result.reasoningSummary, "Need the local file.");
  assert.deepEqual(result.toolCalls, [{ id: "call_1", name: "read_file", arguments: { path: "README.md" } }]);
  assert.equal(result.providerState.data.interactionId, "int_1");
  assert.equal(result.providerState.data.environmentId, "env_1");
  assert.equal(result.providerState.data.providerFunctionNames.call_1, sent.tools[0].name);
  assert.equal(result.usage.totalTokens, 20);
  assert.equal(result.usage.cachedInputTokens, 2);
  assert.equal(result.usage.reasoningTokens, 6);
});

test("Antigravity continues the native interaction with Yeet tool results", async () => {
  const requests = [];
  const provider = new AntigravityProvider({
    apiKey: "gemini-test-key",
    fetch: async (_input, init) => {
      const sent = JSON.parse(init.body);
      requests.push(sent);
      if (requests.length === 1) {
        return jsonResponse({
          id: "int_tool",
          environment_id: "env_tool",
          status: "requires_action",
          steps: [{
            type: "function_call",
            id: "call_tool",
            name: sent.tools[0].name,
            arguments: { path: "a.txt" },
          }],
        });
      }
      return jsonResponse({
        id: "int_final",
        environment_id: "env_tool",
        status: "completed",
        steps: [{ type: "model_output", content: [{ type: "text", text: "The file is OK." }] }],
      });
    },
  });

  const tools = [{ name: "read_file", inputSchema: { type: "object" } }];
  const first = await provider.complete({
    model: ANTIGRAVITY_AGENT,
    messages: [{ role: "user", content: "Read a.txt" }],
    tools,
  });
  const second = await provider.complete({
    model: ANTIGRAVITY_AGENT,
    messages: [
      { role: "user", content: "Read a.txt" },
      {
        role: "assistant",
        content: first.text,
        toolCalls: first.toolCalls,
        providerState: first.providerState,
      },
      {
        role: "tool",
        toolCallId: "call_tool",
        name: "read_file",
        toolResult: {
          toolCallId: "call_tool",
          name: "read_file",
          result: { text: "hello" },
        },
      },
    ],
    tools,
  });

  const continuation = requests[1];
  assert.equal(continuation.previous_interaction_id, "int_tool");
  assert.equal(continuation.environment, "env_tool");
  assert.equal(continuation.input.length, 1);
  assert.equal(continuation.input[0].type, "function_result");
  assert.equal(continuation.input[0].call_id, "call_tool");
  assert.equal(continuation.input[0].name, requests[0].tools[0].name);
  assert.deepEqual(continuation.input[0].result, { text: "hello" });
  assert.equal(second.text, "The file is OK.");
  assert.equal(second.finishReason, "stop");
  assert.equal(second.providerState.data.interactionId, "int_final");
});

test("Antigravity streaming maps summaries, function deltas, and continuation state", async () => {
  const provider = new AntigravityProvider({
    apiKey: "gemini-test-key",
    fetch: async (_input, init) => {
      const sent = JSON.parse(init.body);
      const name = sent.tools[0].name;
      return sseResponse([
        {
          event_type: "interaction.created",
          interaction: { id: "int_stream", environment_id: "env_stream", status: "in_progress" },
        },
        {
          event_type: "step.start",
          index: 0,
          step: { type: "thought", summary: [{ type: "text", text: "Checking the workspace." }] },
        },
        {
          event_type: "step.start",
          index: 1,
          step: { type: "function_call", id: "call_stream", name, arguments: {} },
        },
        {
          event_type: "step.delta",
          index: 1,
          delta: { type: "arguments_delta", arguments: "{\"path\":\"src/main.rs\"}" },
        },
        { event_type: "step.stop", index: 1 },
        {
          event_type: "interaction.requires_action",
          interaction: {
            id: "int_stream",
            environment_id: "env_stream",
            status: "requires_action",
            usage: { total_input_tokens: 12, total_output_tokens: 2, total_tokens: 14 },
          },
        },
      ]);
    },
  });

  const events = [];
  for await (const event of provider.stream({
    model: ANTIGRAVITY_AGENT,
    messages: [{ role: "user", content: "Inspect main.rs" }],
    tools: [{ name: "read_file", inputSchema: { type: "object" } }],
  })) {
    events.push(event);
  }

  assert.equal(events[0].type, "start");
  assert.equal(events[0].id, "int_stream");
  assert.ok(events.some((event) => event.type === "reasoning-start"));
  assert.deepEqual(events.find((event) => event.type === "reasoning-summary-delta"), {
    type: "reasoning-summary-delta",
    delta: "Checking the workspace.",
  });
  const call = events.find((event) => event.type === "tool-call");
  assert.equal(call.toolCall.name, "read_file");
  assert.deepEqual(call.toolCall.arguments, { path: "src/main.rs" });
  const finish = events.at(-1);
  assert.equal(finish.type, "finish");
  assert.equal(finish.finishReason, "tool_call");
  assert.equal(finish.providerState.data.interactionId, "int_stream");
  assert.equal(finish.providerState.data.environmentId, "env_stream");
  assert.equal(finish.usage.totalTokens, 14);
});

test("Antigravity reuses a stored Gemini API key", async () => {
  const directory = await mkdtemp(join(tmpdir(), "yeet-antigravity-auth-"));
  try {
    const auth = new AuthManager({ configDir: directory });
    await auth.ensure();
    await auth.setApiKey("gemini", "shared-google-key");
    const credential = await auth.resolve("antigravity");
    assert.equal(credential.kind, "api-key");
    assert.equal(credential.value, "shared-google-key");
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
