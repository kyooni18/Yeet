import test from "node:test";
import assert from "node:assert/strict";

import {
  AnthropicProvider,
  CallCore,
  GeminiProvider,
  OpenAIChatProvider,
  OpenAIProvider,
  OpenRouterProvider,
  ProviderHTTPError,
  UnknownProviderError,
} from "../dist/index.js";

function jsonResponse(body, init = {}) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
    ...init,
  });
}

function sseResponse(events) {
  return new Response(events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join("") + "data: [DONE]\n\n", {
    status: 200,
    headers: { "content-type": "text/event-stream" },
  });
}

test("CallCore routes by provider and rejects unknown providers", async () => {
  const core = new CallCore();
  await assert.rejects(
    () => core.complete({ model: "missing/x", messages: [{ role: "user", content: "hi" }] }),
    UnknownProviderError,
  );
});

test("OpenAI Responses normalizes text, usage, and tool calls", async () => {
  let sent;
  const provider = new OpenAIProvider({
    apiKey: "test",
    fetch: async (_url, init) => {
      sent = JSON.parse(init.body);
      return jsonResponse({
        id: "resp_1",
        model: "gpt-test",
        status: "completed",
        output: [
          { type: "message", content: [{ type: "output_text", text: "hello" }] },
          { type: "function_call", call_id: "call_1", name: "read_file", arguments: "{\"path\":\"a.ts\"}" },
        ],
        usage: { input_tokens: 7, output_tokens: 3, total_tokens: 10 },
      });
    },
  });

  const result = await provider.complete({
    model: "gpt-test",
    system: "be concise",
    messages: [{ role: "user", content: "inspect" }],
    tools: [{ name: "read_file", inputSchema: { type: "object" } }],
  });

  assert.equal(sent.instructions, "be concise");
  assert.equal(sent.tools[0].name, "read_file");
  assert.equal(result.text, "hello");
  assert.equal(result.finishReason, "tool_call");
  assert.deepEqual(result.toolCalls[0].arguments, { path: "a.ts" });
  assert.equal(result.usage.totalTokens, 10);
});

test("OpenAI Responses provider discovers available models", async () => {
  let request;
  const provider = new OpenAIProvider({
    apiKey: "test",
    fetch: async (url, init) => {
      request = { url: String(url), method: init.method, headers: init.headers };
      return jsonResponse({ data: [{ id: "gpt-test" }, { id: "gpt-test-2" }] });
    },
  });

  const models = await provider.listModels();
  assert.deepEqual(models, ["gpt-test", "gpt-test-2"]);
  assert.equal(request.url, "https://api.openai.com/v1/models");
  assert.equal(request.method, "GET");
  assert.equal(request.headers.authorization, "Bearer test");
});

test("Gemini provider discovers paginated models and removes resource prefixes", async () => {
  const requests = [];
  const provider = new GeminiProvider({
    apiKey: "test",
    fetch: async (url) => {
      requests.push(String(url));
      if (requests.length === 1) return jsonResponse({ models: [{ name: "models/gemini-a" }], nextPageToken: "next" });
      return jsonResponse({ models: [{ name: "models/gemini-b" }, { name: "models/gemini-a" }] });
    },
  });

  const models = await provider.listModels();
  assert.deepEqual(models, ["gemini-a", "gemini-b"]);
  assert.match(requests[0], /\/models\?pageSize=1000$/);
  assert.match(requests[1], /pageToken=next/);
});

test("OpenAI-compatible stream emits normalized deltas and assembled tool call", async () => {
  const provider = new OpenAIChatProvider({
    id: "compat",
    apiKey: "test",
    baseUrl: "https://example.invalid/v1",
    fetch: async () =>
      sseResponse([
        { id: "c1", model: "model-x", choices: [{ delta: { content: "he" }, finish_reason: null }] },
        {
          id: "c1",
          model: "model-x",
          choices: [
            {
              delta: {
                tool_calls: [
                  { index: 0, id: "tool_1", function: { name: "shell", arguments: "{\"cmd\":" } },
                ],
              },
              finish_reason: null,
            },
          ],
        },
        {
          id: "c1",
          model: "model-x",
          choices: [
            {
              delta: { tool_calls: [{ index: 0, function: { arguments: "\"pwd\"}" } }] },
              finish_reason: "tool_calls",
            },
          ],
        },
        { id: "c1", model: "model-x", choices: [], usage: { prompt_tokens: 2, completion_tokens: 4, total_tokens: 6 } },
      ]),
  });

  const events = [];
  for await (const event of provider.stream({ model: "model-x", messages: [{ role: "user", content: "go" }] })) {
    events.push(event);
  }

  assert.equal(events[0].type, "start");
  assert.ok(events.some((event) => event.type === "text-delta" && event.delta === "he"));
  const call = events.find((event) => event.type === "tool-call");
  assert.deepEqual(call.toolCall.arguments, { cmd: "pwd" });
  const finish = events.at(-1);
  assert.equal(finish.finishReason, "tool_call");
  assert.equal(finish.usage.totalTokens, 6);
});

test("Anthropic Messages normalizes tool use", async () => {
  let sent;
  const provider = new AnthropicProvider({
    apiKey: "test",
    fetch: async (_url, init) => {
      sent = JSON.parse(init.body);
      return jsonResponse({
        id: "msg_1",
        model: "claude-test",
        content: [
          { type: "text", text: "checking" },
          { type: "tool_use", id: "toolu_1", name: "read_file", input: { path: "x.ts" } },
        ],
        stop_reason: "tool_use",
        usage: { input_tokens: 5, output_tokens: 8 },
      });
    },
  });

  const result = await provider.complete({
    model: "claude-test",
    messages: [{ role: "system", content: "lead" }, { role: "user", content: "inspect" }],
    maxTokens: 100,
  });
  assert.equal(sent.system, "lead");
  assert.equal(sent.max_tokens, 100);
  assert.equal(result.finishReason, "tool_call");
  assert.equal(result.toolCalls[0].name, "read_file");
});

test("Gemini generateContent normalizes function calls", async () => {
  let requestedUrl;
  let sent;
  const provider = new GeminiProvider({
    apiKey: "test",
    fetch: async (url, init) => {
      requestedUrl = String(url);
      sent = JSON.parse(init.body);
      return jsonResponse({
        candidates: [
          {
            content: { parts: [{ text: "checking" }, { functionCall: { name: "shell", args: { cmd: "pwd" } } }] },
            finishReason: "STOP",
          },
        ],
        usageMetadata: { promptTokenCount: 3, candidatesTokenCount: 4, totalTokenCount: 7 },
      });
    },
  });

  const result = await provider.complete({
    model: "gemini-test",
    system: "lead",
    messages: [{ role: "user", content: "go" }],
    tools: [{ name: "shell", inputSchema: { type: "object" } }],
  });
  assert.match(requestedUrl, /models\/gemini-test:generateContent$/);
  assert.equal(sent.systemInstruction.parts[0].text, "lead");
  assert.equal(result.finishReason, "tool_call");
  assert.deepEqual(result.toolCalls[0].arguments, { cmd: "pwd" });
});

test("OpenRouter sets attribution headers and uses chat completions", async () => {
  let seen;
  const provider = new OpenRouterProvider({
    apiKey: "test",
    appUrl: "https://example.test",
    appName: "Harness",
    fetch: async (url, init) => {
      seen = { url: String(url), headers: init.headers };
      return jsonResponse({ choices: [{ message: { content: "ok" }, finish_reason: "stop" }] });
    },
  });

  const result = await provider.complete({ model: "openai/test", messages: [{ role: "user", content: "hi" }] });
  assert.equal(result.text, "ok");
  assert.equal(seen.url, "https://openrouter.ai/api/v1/chat/completions");
  assert.equal(seen.headers["HTTP-Referer"], "https://example.test");
  assert.equal(seen.headers["X-Title"], "Harness");
});

test("retry policy retries transient HTTP failures", async () => {
  let attempts = 0;
  const provider = new OpenAIChatProvider({
    id: "retry-test",
    apiKey: "test",
    fetch: async () => {
      attempts += 1;
      if (attempts === 1) return new Response("busy", { status: 503 });
      return jsonResponse({ choices: [{ message: { content: "ok" }, finish_reason: "stop" }] });
    },
  });

  const result = await provider.complete({
    model: "x",
    messages: [{ role: "user", content: "hi" }],
    retry: { maxAttempts: 2, baseDelayMs: 0, maxDelayMs: 0, jitter: 0 },
  });
  assert.equal(result.text, "ok");
  assert.equal(attempts, 2);
});

test("non-retryable HTTP error exposes provider and response body", async () => {
  const provider = new OpenAIChatProvider({
    id: "bad",
    apiKey: "test",
    fetch: async () => new Response("bad request", { status: 400, statusText: "Bad Request" }),
  });

  await assert.rejects(
    () => provider.complete({ model: "x", messages: [{ role: "user", content: "hi" }] }),
    (error) => {
      assert.ok(error instanceof ProviderHTTPError);
      assert.equal(error.provider, "bad");
      assert.equal(error.status, 400);
      assert.equal(error.responseBody, "bad request");
      return true;
    },
  );
});

test("OpenAI-compatible provider can call local no-auth endpoints", async () => {
  let authorization;
  const provider = new OpenAIChatProvider({
    id: "local",
    baseUrl: "http://127.0.0.1:8080/v1",
    fetch: async (_url, init) => {
      authorization = init.headers.authorization;
      return jsonResponse({ choices: [{ message: { content: "local" }, finish_reason: "stop" }] });
    },
  });
  const result = await provider.complete({ model: "local-model", messages: [{ role: "user", content: "hi" }] });
  assert.equal(authorization, undefined);
  assert.equal(result.text, "local");
});

test("OpenAI Responses stream emits text and function call events", async () => {
  const provider = new OpenAIProvider({
    apiKey: "test",
    fetch: async () =>
      sseResponse([
        { type: "response.created", response: { id: "resp_s", model: "gpt-test" } },
        { type: "response.output_text.delta", delta: "hi" },
        {
          type: "response.output_item.added",
          output_index: 1,
          item: { type: "function_call", call_id: "call_s", name: "read_file", arguments: "" },
        },
        { type: "response.function_call_arguments.delta", output_index: 1, delta: "{\"path\":" },
        { type: "response.function_call_arguments.done", output_index: 1, arguments: "{\"path\":\"x.ts\"}" },
        {
          type: "response.completed",
          response: {
            status: "completed",
            output: [{ type: "function_call", call_id: "call_s", name: "read_file", arguments: "{\"path\":\"x.ts\"}" }],
            usage: { input_tokens: 1, output_tokens: 2, total_tokens: 3 },
          },
        },
      ]),
  });
  const events = [];
  for await (const event of provider.stream({ model: "gpt-test", messages: [{ role: "user", content: "go" }] })) events.push(event);
  assert.ok(events.some((event) => event.type === "text-delta" && event.delta === "hi"));
  const tool = events.find((event) => event.type === "tool-call");
  assert.deepEqual(tool.toolCall.arguments, { path: "x.ts" });
  assert.equal(events.at(-1).finishReason, "tool_call");
});

test("Anthropic stream assembles input_json_delta", async () => {
  const provider = new AnthropicProvider({
    apiKey: "test",
    fetch: async () =>
      sseResponse([
        { type: "message_start", message: { id: "msg_s", model: "claude-test", usage: { input_tokens: 2 } } },
        { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "ok" } },
        { type: "content_block_start", index: 1, content_block: { type: "tool_use", id: "toolu_s", name: "shell", input: {} } },
        { type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: "{\"cmd\":" } },
        { type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: "\"pwd\"}" } },
        { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 5 } },
      ]),
  });
  const events = [];
  for await (const event of provider.stream({ model: "claude-test", messages: [{ role: "user", content: "go" }] })) events.push(event);
  const tool = events.find((event) => event.type === "tool-call");
  assert.deepEqual(tool.toolCall.arguments, { cmd: "pwd" });
  assert.equal(events.at(-1).finishReason, "tool_call");
  assert.equal(events.at(-1).usage.inputTokens, 2);
  assert.equal(events.at(-1).usage.outputTokens, 5);
});

test("Gemini stream emits text and function calls", async () => {
  const provider = new GeminiProvider({
    apiKey: "test",
    fetch: async () =>
      sseResponse([
        { candidates: [{ content: { parts: [{ text: "he" }] } }] },
        {
          candidates: [{ content: { parts: [{ functionCall: { name: "shell", args: { cmd: "pwd" } } }] }, finishReason: "STOP" }],
          usageMetadata: { promptTokenCount: 2, candidatesTokenCount: 3, totalTokenCount: 5 },
        },
      ]),
  });
  const events = [];
  for await (const event of provider.stream({ model: "gemini-test", messages: [{ role: "user", content: "go" }] })) events.push(event);
  assert.ok(events.some((event) => event.type === "text-delta" && event.delta === "he"));
  const tool = events.find((event) => event.type === "tool-call");
  assert.deepEqual(tool.toolCall.arguments, { cmd: "pwd" });
  assert.equal(events.at(-1).finishReason, "tool_call");
});
