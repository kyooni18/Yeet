import test from "node:test";
import assert from "node:assert/strict";

import {
  AnthropicProvider,
  CallCore,
  GeminiProvider,
  ModelMetadataCatalog,
  OpenCodeProvider,
  OpenAIChatProvider,
  OpenAIProvider,
  OpenRouterProvider,
  ProviderHTTPError,
  providerFetch,
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

test("OpenAI ChatGPT OAuth uses the Codex backend and account-scoped headers", async () => {
  let requestUrl;
  let requestHeaders;
  const provider = new OpenAIProvider({
    accessToken: "chatgpt-access-token",
    accountId: "acct-123",
    fetch: async (input, init) => {
      requestUrl = String(input);
      requestHeaders = init.headers;
      return sseResponse([{ type: "response.completed", response: {
        id: "chatgpt-response",
        model: "gpt-5.6",
        status: "completed",
        output_text: "hello from ChatGPT OAuth",
        output: [],
      } }]);
    },
  });

  const result = await provider.complete({
    model: "gpt-5.6",
    messages: [{ role: "user", content: "hello" }],
  });
  assert.equal(result.text, "hello from ChatGPT OAuth");
  assert.equal(requestUrl, "https://chatgpt.com/backend-api/codex/responses");
  assert.equal(requestHeaders.authorization, "Bearer chatgpt-access-token");
  assert.equal(requestHeaders["ChatGPT-Account-ID"], "acct-123");
  assert.equal(requestHeaders.originator, "codex_cli_rs");
});

test("multimodal image attachments are mapped to provider-native request formats", async () => {
  const image = { mediaType: "image/png", data: "aGVsbG8=", name: "pixel.png" };

  let openAIRequest;
  const openAI = new OpenAIProvider({
    apiKey: "test",
    fetch: async (_url, init) => {
      openAIRequest = JSON.parse(init.body);
      return jsonResponse({ model: "gpt-test", status: "completed", output: [] });
    },
  });
  await openAI.complete({ model: "gpt-test", messages: [{ role: "user", content: "inspect", images: [image] }] });
  assert.equal(openAIRequest.input[0].content[0].type, "input_text");
  assert.equal(openAIRequest.input[0].content[1].type, "input_image");
  assert.equal(openAIRequest.input[0].content[1].image_url, "data:image/png;base64,aGVsbG8=");

  let chatRequest;
  const chat = new OpenAIChatProvider({ id: "chat", baseUrl: "https://example.invalid/v1", fetch: async (_url, init) => {
    chatRequest = JSON.parse(init.body);
    return jsonResponse({ choices: [{ message: { content: "ok" }, finish_reason: "stop" }] });
  } });
  await chat.complete({ model: "model", messages: [{ role: "user", content: "inspect", images: [image] }] });
  assert.equal(chatRequest.messages[0].content[1].type, "image_url");
  assert.equal(chatRequest.messages[0].content[1].image_url.url, "data:image/png;base64,aGVsbG8=");

  let anthropicRequest;
  const anthropic = new AnthropicProvider({ apiKey: "test", fetch: async (_url, init) => {
    anthropicRequest = JSON.parse(init.body);
    return jsonResponse({ model: "claude-test", content: [], stop_reason: "end_turn", usage: {} });
  } });
  await anthropic.complete({ model: "claude-test", messages: [{ role: "user", content: "inspect", images: [image] }] });
  assert.deepEqual(anthropicRequest.messages[0].content[1].source, { type: "base64", media_type: "image/png", data: "aGVsbG8=" });

  let geminiRequest;
  const gemini = new GeminiProvider({ apiKey: "test", fetch: async (_url, init) => {
    geminiRequest = JSON.parse(init.body);
    return jsonResponse({ candidates: [{ content: { parts: [] }, finishReason: "STOP" }], usageMetadata: {} });
  } });
  await gemini.complete({ model: "gemini-test", messages: [{ role: "user", content: "inspect", images: [image] }] });
  assert.deepEqual(geminiRequest.contents[0].parts[1].inlineData, { mimeType: "image/png", data: "aGVsbG8=" });
});

test("provider-native prompt caching is enabled and cache telemetry is normalized", async () => {
  let openAIRequest;
  const openAI = new OpenAIProvider({
    apiKey: "test",
    fetch: async (_url, init) => {
      openAIRequest = JSON.parse(init.body);
      return jsonResponse({
        model: "gpt-5.6",
        status: "completed",
        output: [{ type: "message", content: [{ type: "output_text", text: "ok" }] }],
        usage: {
          input_tokens: 100,
          output_tokens: 20,
          total_tokens: 120,
          input_tokens_details: { cached_tokens: 70, cache_write_tokens: 10 },
          output_tokens_details: { reasoning_tokens: 8 },
        },
      });
    },
  });
  const openAIResult = await openAI.complete({
    model: "gpt-5.6",
    contextKey: "session-cache-key",
    messages: [
      { role: "system", content: "stable system" },
      { role: "user", content: "hello", cacheBreakpoint: true },
      { role: "system", content: "volatile request overlay", requestOnly: true },
    ],
  });
  assert.equal(openAIRequest.prompt_cache_key, "session-cache-key");
  assert.deepEqual(openAIRequest.prompt_cache_options, { mode: "explicit", ttl: "30m" });
  assert.equal(openAIRequest.instructions, "stable system");
  assert.deepEqual(openAIRequest.input[0].content[0].prompt_cache_breakpoint, { mode: "explicit" });
  assert.equal(openAIRequest.input[1].role, "system");
  assert.equal(openAIRequest.input[1].content, "volatile request overlay");
  assert.equal(openAIResult.usage.cachedInputTokens, 70);
  assert.equal(openAIResult.usage.cacheWriteInputTokens, 10);
  assert.equal(openAIResult.usage.reasoningTokens, 8);

  await openAI.complete({ model: "gpt-5.6", messages: [{ role: "user", content: "one shot" }] });
  assert.deepEqual(openAIRequest.prompt_cache_options, { mode: "explicit", ttl: "30m" });
  assert.equal(JSON.stringify(openAIRequest).includes("prompt_cache_breakpoint"), false);

  await openAI.complete({
    model: "gpt-5.6",
    messages: [
      { role: "assistant", toolCalls: [{ id: "call-cache", name: "read_file", arguments: { path: "a.rs" } }] },
      { role: "tool", toolCallId: "call-cache", content: "source", cacheBreakpoint: true },
    ],
  });
  assert.deepEqual(openAIRequest.input[1].output[0].prompt_cache_breakpoint, { mode: "explicit" });

  let anthropicRequest;
  const anthropic = new AnthropicProvider({
    apiKey: "test",
    fetch: async (_url, init) => {
      anthropicRequest = JSON.parse(init.body);
      return jsonResponse({
        model: "claude-test",
        content: [{ type: "text", text: "ok" }],
        stop_reason: "end_turn",
        usage: {
          input_tokens: 30,
          output_tokens: 5,
          cache_read_input_tokens: 20,
          cache_creation: { ephemeral_5m_input_tokens: 6, ephemeral_1h_input_tokens: 4 },
          output_tokens_details: { thinking_tokens: 3 },
        },
      });
    },
  });
  const anthropicResult = await anthropic.complete({ model: "claude-test", messages: [{ role: "user", content: "hello" }] });
  assert.deepEqual(anthropicRequest.cache_control, { type: "ephemeral" });
  assert.equal(anthropicResult.usage.cachedInputTokens, 20);
  assert.equal(anthropicResult.usage.cacheWriteInputTokens, 10);
  assert.equal(anthropicResult.usage.reasoningTokens, 3);
});

test("Anthropic and Gemini keep volatile coordinator overlays behind the stable system prefix", async () => {
  const messages = [
    { role: "system", content: "stable system" },
    { role: "user", content: "hello" },
    { role: "system", content: "volatile request overlay", requestOnly: true },
  ];

  let anthropicRequest;
  const anthropic = new AnthropicProvider({
    apiKey: "test",
    fetch: async (_url, init) => {
      anthropicRequest = JSON.parse(init.body);
      return jsonResponse({ model: "claude-test", content: [], stop_reason: "end_turn", usage: {} });
    },
  });
  await anthropic.complete({ model: "claude-test", messages });
  assert.equal(anthropicRequest.system, "stable system");
  assert.deepEqual(
    anthropicRequest.messages.map((message) => [message.role, message.content]),
    [["user", "hello"], ["user", "volatile request overlay"]],
  );

  let geminiRequest;
  const gemini = new GeminiProvider({
    apiKey: "test",
    fetch: async (_url, init) => {
      geminiRequest = JSON.parse(init.body);
      return jsonResponse({ candidates: [{ content: { parts: [] }, finishReason: "STOP" }], usageMetadata: {} });
    },
  });
  await gemini.complete({ model: "gemini-test", messages });
  assert.equal(geminiRequest.systemInstruction.parts[0].text, "stable system");
  assert.deepEqual(
    geminiRequest.contents.map((message) => [message.role, message.parts[0].text]),
    [["user", "hello"], ["user", "volatile request overlay"]],
  );
});

test("providers serialize structured tool results and preserve error state", async () => {
  const requests = [];
  const fetch = async (_url, init) => {
    requests.push(JSON.parse(init.body));
    return jsonResponse({ choices: [{ message: { content: "ok" }, finish_reason: "stop" }] });
  };

  const chat = new OpenAIChatProvider({ id: "structured", baseUrl: "https://example.invalid/v1", fetch });
  await chat.complete({
    model: "model",
    messages: [
      { role: "assistant", toolCalls: [{ id: "call-1", name: "lookup", arguments: { q: "yeet" } }] },
      { role: "tool", toolResult: { toolCallId: "call-1", result: { answer: 42 }, isError: true } },
    ],
  });
  assert.equal(requests[0].messages[1].tool_call_id, "call-1");
  assert.equal(requests[0].messages[1].content, '{"answer":42}');

  const anthropic = new AnthropicProvider({ apiKey: "test", fetch });
  await anthropic.complete({
    model: "model",
    messages: [{ role: "tool", toolResult: { toolCallId: "toolu-1", content: "denied", isError: true } }],
  });
  const anthropicMessage = requests[1].messages.at(-1).content[0];
  assert.equal(anthropicMessage.tool_use_id, "toolu-1");
  assert.equal(anthropicMessage.is_error, true);
});

test("multimodal messages map to provider-native image inputs", async () => {
  const image = { mediaType: "image/png", data: "YWJj", name: "frame.png" };

  let openAIRequest;
  const openAI = new OpenAIProvider({
    apiKey: "test",
    fetch: async (_url, init) => {
      openAIRequest = JSON.parse(init.body);
      return jsonResponse({ model: "gpt-test", status: "completed", output: [] });
    },
  });
  await openAI.complete({ model: "gpt-test", messages: [{ role: "user", content: "inspect", images: [image] }] });
  assert.deepEqual(openAIRequest.input[0].content, [
    { type: "input_text", text: "inspect" },
    { type: "input_image", image_url: "data:image/png;base64,YWJj" },
  ]);

  let chatRequest;
  const chat = new OpenAIChatProvider({
    id: "chat",
    baseUrl: "https://example.invalid/v1",
    fetch: async (_url, init) => {
      chatRequest = JSON.parse(init.body);
      return jsonResponse({ choices: [{ message: { content: "ok" }, finish_reason: "stop" }] });
    },
  });
  await chat.complete({ model: "vision-model", messages: [{ role: "user", content: "inspect", images: [image] }] });
  assert.deepEqual(chatRequest.messages[0].content, [
    { type: "text", text: "inspect" },
    { type: "image_url", image_url: { url: "data:image/png;base64,YWJj" } },
  ]);

  let anthropicRequest;
  const anthropic = new AnthropicProvider({
    apiKey: "test",
    fetch: async (_url, init) => {
      anthropicRequest = JSON.parse(init.body);
      return jsonResponse({ model: "claude-test", content: [], stop_reason: "end_turn", usage: {} });
    },
  });
  await anthropic.complete({ model: "claude-test", messages: [{ role: "user", content: "inspect", images: [image] }] });
  assert.deepEqual(anthropicRequest.messages[0].content, [
    { type: "text", text: "inspect" },
    { type: "image", source: { type: "base64", media_type: "image/png", data: "YWJj" } },
  ]);

  let geminiRequest;
  const gemini = new GeminiProvider({
    apiKey: "test",
    fetch: async (_url, init) => {
      geminiRequest = JSON.parse(init.body);
      return jsonResponse({ candidates: [{ content: { parts: [] }, finishReason: "STOP" }], usageMetadata: {} });
    },
  });
  await gemini.complete({ model: "gemini-test", messages: [{ role: "user", content: "inspect", images: [image] }] });
  assert.deepEqual(geminiRequest.contents[0].parts, [
    { text: "inspect" },
    { inlineData: { mimeType: "image/png", data: "YWJj" } },
  ]);
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

test("OpenAI OAuth model discovery supplies the ChatGPT client version", async () => {
  let requestedUrl;
  const provider = new OpenAIProvider({
    accessToken: "oauth-token",
    accountId: "account",
    fetch: async (url) => {
      requestedUrl = String(url);
      return jsonResponse({ models: [{ slug: "gpt-oauth-test", context_window: 200000 }] });
    },
  });

  assert.deepEqual(await provider.listModels(), ["gpt-oauth-test"]);
  assert.equal(requestedUrl, "https://chatgpt.com/backend-api/codex/models?client_version=0.151.0");
});

test("provider HTTP calls emit redacted structured attempt logs", async () => {
  const logs = [];
  let calls = 0;
  const response = await providerFetch(
    "https://api.example.test/v1/models?api_key=secret&region=us",
    { method: "GET" },
    {
      provider: "example",
      retry: { maxAttempts: 2, baseDelayMs: 0, jitter: 0 },
      fetch: async () => {
        calls += 1;
        return calls === 1
          ? new Response("busy", { status: 503, statusText: "Unavailable", headers: { "retry-after": "0" } })
          : new Response("ok", { status: 200, headers: { "x-request-id": "req-1" } });
      },
      apiCallLogger: (entry) => logs.push(entry),
    },
  );

  assert.equal(response.status, 200);
  assert.equal(logs.length, 2);
  assert.deepEqual(logs.map(({ outcome, attempt, status, statusText, retryDelayMs }) => ({ outcome, attempt, status, statusText, retryDelayMs })), [
    { outcome: "retry", attempt: 1, status: 503, statusText: "Unavailable", retryDelayMs: 0 },
    { outcome: "success", attempt: 2, status: 200, statusText: undefined, retryDelayMs: undefined },
  ]);
  assert.match(logs[0].timestamp, /^\d{4}-\d{2}-\d{2}T/);
  assert.equal(logs[0].url, "https://api.example.test/v1/models");
  assert.equal(logs[1].requestId, "req-1");
  assert.ok(logs.every(({ durationMs }) => durationMs >= 0));
  assert.ok(!JSON.stringify(logs).includes("secret"));
});

test("provider HTTP logs transport failures with a stable error name", async () => {
  const logs = [];
  await assert.rejects(
    () => providerFetch("https://api.example.test/v1/completions", { method: "POST" }, {
      provider: "example",
      retry: { maxAttempts: 1 },
      fetch: async () => { throw new TypeError("secret network detail"); },
      apiCallLogger: (entry) => logs.push(entry),
    }),
    /request failed/,
  );

  assert.deepEqual(logs.map(({ outcome, error }) => ({ outcome, error })), [{ outcome: "error", error: "TypeError" }]);
  assert.ok(!JSON.stringify(logs).includes("secret network detail"));
});

test("model discovery preserves provider context windows", async () => {
  const provider = new OpenRouterProvider({
    apiKey: "test",
    fetch: async () => jsonResponse({
      data: [
        { id: "meta/model-a", context_length: 131072 },
        { id: "meta/model-b" },
      ],
    }),
  });

  assert.deepEqual(await provider.listModelInfo(), [
    { id: "meta/model-a", contextLength: 131072 },
    { id: "meta/model-b" },
  ]);
  assert.deepEqual(await new CallCore([provider]).listModelInfo("openrouter"), [
    { id: "meta/model-a", contextLength: 131072 },
    { id: "meta/model-b" },
  ]);
});

test("OpenRouter model discovery preserves live provider pricing and long-context overrides", async () => {
  const provider = new OpenRouterProvider({
    apiKey: "test",
    fetch: async () => jsonResponse({
      data: [{
        id: "openai/gpt-6-astra",
        context_length: 1_050_000,
        pricing: {
          prompt: "0.00001",
          completion: "0.00005",
          input_cache_read: "0.000001",
          input_cache_write: "0.0000125",
          overrides: [{
            min_prompt_tokens: 272_000,
            prompt: "0.00002",
            completion: "0.000075",
            input_cache_read: "0.000002",
            input_cache_write: "0.000025",
          }],
        },
      }],
    }),
  });

  assert.deepEqual(await provider.listModelInfo(), [{
    id: "openai/gpt-6-astra",
    contextLength: 1_050_000,
    pricing: {
      input: 10,
      output: 50,
      cacheRead: 1,
      cacheWrite: 12.5,
      currency: "USD",
      unit: "per1MTokens",
      source: "provider",
      tiers: [{ input: 20, output: 75, cacheRead: 2, cacheWrite: 25, thresholdTokens: 271_999 }],
    },
  }]);
});

test("model metadata resolves context independently of the selected provider and caches the catalog", async () => {
  let fetchCount = 0;
  const catalog = new ModelMetadataCatalog({
    fetch: async () => {
      fetchCount += 1;
      return jsonResponse({
        openai: {
          id: "openai",
          models: {
            "gpt-test": { id: "gpt-test", limit: { context: 400000 } },
          },
        },
        google: {
          id: "google",
          models: {
            "gemini-test": { id: "gemini-test", limit: { context: 1048576 } },
          },
        },
        routerA: {
          id: "routerA",
          models: {
            "shared-model": { id: "shared-model", limit: { context: 131072 } },
          },
        },
        routerB: {
          id: "routerB",
          models: {
            "shared-model": { id: "shared-model", limit: { context: 65536 } },
          },
        },
      });
    },
  });

  assert.equal(await catalog.contextLength("openai/gpt-test"), 400000);
  assert.equal(await catalog.contextLength("gemini/gemini-test"), 1048576);
  assert.equal(await catalog.contextLength("custom-provider/gpt-test"), 400000);
  assert.equal(await catalog.contextLength("custom-provider/shared-model"), undefined);
  assert.equal(await catalog.contextLength("custom-provider/missing"), undefined);
  assert.equal(await catalog.pricing("custom-provider/gpt-test"), undefined);
  assert.equal(fetchCount, 1);
});

test("model metadata exposes live token pricing, cache rates, long-context tiers, and modes", async () => {
  const catalog = new ModelMetadataCatalog({
    fetch: async () => jsonResponse({
      openai: {
        id: "openai",
        models: {
          "gpt-6-astra": {
            id: "gpt-6-astra",
            modalities: { input: ["text", "image"], output: ["text"] },
            limit: { context: 1_050_000 },
            cost: {
              input: 10,
              output: 50,
              cache_read: 1,
              cache_write: 12.5,
              tiers: [{
                input: 20,
                output: 75,
                cache_read: 2,
                cache_write: 25,
                tier: { type: "context", size: 272_000 },
              }],
            },
            experimental: { modes: { fast: { cost: { input: 20, output: 100, cache_read: 2, cache_write: 25 } } } },
          },
        },
      },
    }),
  });

  assert.equal(await catalog.contextLength("openai/gpt-6-astra"), 1_050_000);
  assert.equal(await catalog.supportsTextGeneration("openai/gpt-6-astra"), true);
  assert.deepEqual(await catalog.pricing("openai/gpt-6-astra"), {
    input: 10,
    output: 50,
    cacheRead: 1,
    cacheWrite: 12.5,
    currency: "USD",
    unit: "per1MTokens",
    source: "models.dev",
    tiers: [{ input: 20, output: 75, cacheRead: 2, cacheWrite: 25, thresholdTokens: 272_000 }],
    modes: { fast: { input: 20, output: 100, cacheRead: 2, cacheWrite: 25 } },
  });
});

test("model metadata catalog refreshes after its pricing TTL", async () => {
  let fetchCount = 0;
  const catalog = new ModelMetadataCatalog({
    maxAgeMs: 0,
    fetch: async () => jsonResponse({
      openai: { id: "openai", models: { test: {
        id: "test",
        limit: { context: 1000 },
        cost: { input: ++fetchCount, output: 1 },
      } } },
    }),
  });
  assert.equal((await catalog.pricing("openai/test")).input, 1);
  assert.equal((await catalog.pricing("openai/test")).input, 2);
  assert.equal(fetchCount, 2);
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
        {
          id: "c1",
          model: "model-x",
          choices: [],
          usage: {
            prompt_tokens: 2,
            completion_tokens: 4,
            total_tokens: 6,
            completion_tokens_details: { reasoning_tokens: 3 },
          },
        },
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
  assert.equal(finish.usage.reasoningTokens, 3);
});

test("OpenAI-compatible provider preserves emitted reasoning and summary", async () => {
  const provider = new OpenAIChatProvider({
    id: "compat-reasoning",
    baseUrl: "https://example.invalid/v1",
    fetch: async () => sseResponse([
      {
        id: "reason-1",
        model: "reasoning-model",
        choices: [{
          delta: { reasoning_content: "inspect ", reasoning_summary: "Inspecting", content: "" },
          finish_reason: null,
        }],
      },
      {
        id: "reason-1",
        model: "reasoning-model",
        choices: [{ delta: { reasoning_content: "source", content: "answer" }, finish_reason: "stop" }],
      },
    ]),
  });

  const events = [];
  for await (const event of provider.stream({ model: "reasoning-model", messages: [{ role: "user", content: "go" }] })) events.push(event);
  assert.deepEqual(events.filter((event) => event.type === "reasoning-delta").map((event) => event.delta), ["inspect ", "source"]);
  assert.deepEqual(events.filter((event) => event.type === "reasoning-summary-delta").map((event) => event.delta), ["Inspecting"]);
  assert.ok(events.some((event) => event.type === "text-delta" && event.delta === "answer"));
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
        usageMetadata: { promptTokenCount: 3, candidatesTokenCount: 4, thoughtsTokenCount: 6, totalTokenCount: 13 },
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
  assert.equal(result.usage.reasoningTokens, 6);
});

test("Gemini lowers provider-neutral tool unions before sending function declarations", async () => {
  let sent;
  const provider = new GeminiProvider({
    apiKey: "test",
    fetch: async (_url, init) => {
      sent = JSON.parse(init.body);
      return jsonResponse({ candidates: [{ content: { parts: [{ text: "ok" }] }, finishReason: "STOP" }] });
    },
  });

  await provider.complete({
    model: "gemini-test",
    messages: [{ role: "user", content: "edit" }],
    tools: [{
      name: "apply_file_edits",
      inputSchema: {
        type: "object",
        additionalProperties: false,
        required: ["changes"],
        properties: {
          changes: {
            type: "array",
            minItems: 1,
            items: {
              type: "object",
              additionalProperties: false,
              required: ["path"],
              anyOf: [{ required: ["edits"] }, { required: ["fileOp"] }],
              properties: {
                path: { type: "string", minLength: 1 },
                edits: {
                  type: "array",
                  items: {
                    oneOf: [
                      { type: "object", required: ["kind", "text"], properties: { kind: { const: "replace" }, text: { type: "string" } } },
                      { type: "object", required: ["kind"], properties: { kind: { const: "delete" } } },
                    ],
                  },
                },
                fileOp: {
                  oneOf: [
                    { type: "object", required: ["kind", "text"], properties: { kind: { const: "create" }, text: { type: "string" } } },
                    { type: "object", required: ["kind"], properties: { kind: { const: "delete" } } },
                  ],
                },
              },
            },
          },
        },
      },
    }],
  });

  const parameters = sent.tools[0].functionDeclarations[0].parameters;
  const serialized = JSON.stringify(parameters);
  assert.ok(!serialized.includes("oneOf"));
  assert.ok(!serialized.includes("anyOf"));
  assert.ok(!serialized.includes("const"));
  assert.ok(!serialized.includes("additionalProperties"));
  assert.equal(parameters.properties.changes.items.properties.path.minLength, 1);
  assert.deepEqual(parameters.required, ["changes"]);
  assert.deepEqual(parameters.properties.changes.items.required, ["path"]);
  assert.deepEqual(parameters.properties.changes.items.properties.edits.items.required, ["kind"]);
  assert.deepEqual(parameters.properties.changes.items.properties.edits.items.properties.kind.enum.sort(), ["delete", "replace"]);
  assert.deepEqual(parameters.properties.changes.items.properties.fileOp.required, ["kind"]);
});

test("Gemini replays function call ids and thought signatures on tool results", async () => {
  const sent = [];
  let call = 0;
  const provider = new GeminiProvider({
    apiKey: "test",
    fetch: async (_url, init) => {
      sent.push(JSON.parse(init.body));
      call += 1;
      if (call === 1) {
        return jsonResponse({
          candidates: [{
            content: {
              parts: [{
                functionCall: { id: "fc-17", name: "read_file", args: { path: "a.rs" } },
                thoughtSignature: "sig-17",
              }],
            },
            finishReason: "STOP",
          }],
        });
      }
      return jsonResponse({ candidates: [{ content: { parts: [{ text: "done" }] }, finishReason: "STOP" }] });
    },
  });

  const first = await provider.complete({
    model: "gemini-test",
    messages: [{ role: "user", content: "read" }],
    tools: [{ name: "read_file", inputSchema: { type: "object", properties: { path: { type: "string" } } } }],
  });
  assert.equal(first.toolCalls[0].id, "fc-17");

  await provider.complete({
    model: "gemini-test",
    messages: [
      { role: "user", content: "read" },
      { role: "assistant", toolCalls: first.toolCalls },
      { role: "tool", toolCallId: "fc-17", name: "read_file", content: "source" },
    ],
    tools: [{ name: "read_file", inputSchema: { type: "object", properties: { path: { type: "string" } } } }],
  });

  const assistantPart = sent[1].contents[1].parts[0];
  assert.equal(assistantPart.functionCall.id, "fc-17");
  assert.equal(assistantPart.thoughtSignature, "sig-17");
  assert.equal(sent[1].contents[2].parts[0].functionResponse.id, "fc-17");
});

test("Gemini validates named tool choice and filters provider options", async () => {
  let sent;
  const provider = new GeminiProvider({
    apiKey: "test",
    fetch: async (_url, init) => {
      sent = JSON.parse(init.body);
      return jsonResponse({ candidates: [{ content: { parts: [{ text: "ok" }] }, finishReason: "STOP" }] });
    },
  });

  await assert.rejects(
    () => provider.complete({
      model: "gemini-test",
      messages: [{ role: "user", content: "go" }],
      tools: [{ name: "read_file", inputSchema: { type: "object" } }],
      toolChoice: { name: "missing_tool" },
    }),
    /undeclared tool/,
  );

  await provider.complete({
    model: "gemini-test",
    messages: [{ role: "user", content: "go" }],
    temperature: 0.2,
    providerOptions: {
      unknownRootOption: "must-not-leak",
      generationConfig: { topP: 0.8, temperature: 0.9 },
    },
  });
  assert.equal(sent.unknownRootOption, undefined);
  assert.equal(sent.generationConfig.topP, 0.8);
  assert.equal(sent.generationConfig.temperature, 0.2);
});

test("Gemini derives reasoning tokens from total usage when thoughtsTokenCount is omitted", async () => {
  const provider = new GeminiProvider({
    apiKey: "test",
    fetch: async () => jsonResponse({
      candidates: [{ content: { parts: [{ text: "answer" }] }, finishReason: "STOP" }],
      usageMetadata: { promptTokenCount: 3, candidatesTokenCount: 4, totalTokenCount: 11 },
    }),
  });

  const result = await provider.complete({ model: "gemini-test", messages: [{ role: "user", content: "go" }] });
  assert.equal(result.usage.reasoningTokens, 4);
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

test("OpenCode Zen discovers models and routes provider-native protocols", async () => {
  const requests = [];
  const provider = new OpenCodeProvider({
    id: "opencode",
    apiKey: "test",
    fetch: async (url, init = {}) => {
      const value = String(url);
      requests.push({ url: value, init });
      if (value === "https://models.dev/api.json") {
        return jsonResponse({
          opencode: {
            id: "opencode",
            npm: "@ai-sdk/openai-compatible",
            models: {
              "gpt-test": {
                id: "gpt-test",
                provider: { npm: "@ai-sdk/openai" },
                limit: { context: 400000 },
              },
              "claude-test": {
                id: "claude-test",
                provider: { npm: "@ai-sdk/anthropic" },
                limit: { context: 200000 },
              },
              "gemini-test": {
                id: "gemini-test",
                provider: { npm: "@ai-sdk/google" },
                limit: { context: 1000000 },
              },
              "kimi-test": {
                id: "kimi-test",
                limit: { context: 262144 },
              },
            },
          },
        });
      }
      if (value.endsWith("/models")) {
        return jsonResponse({ data: [{ id: "gpt-test" }, { id: "claude-test" }, { id: "gemini-test" }, { id: "kimi-test" }] });
      }
      if (value.endsWith("/responses")) {
        return jsonResponse({
          model: "gpt-test",
          status: "completed",
          output: [{ type: "message", content: [{ type: "output_text", text: "gpt" }] }],
        });
      }
      if (value.endsWith("/v1/messages")) {
        return jsonResponse({ model: "claude-test", content: [{ type: "text", text: "claude" }], stop_reason: "end_turn" });
      }
      if (value.includes("models/gemini-test:generateContent")) {
        return jsonResponse({ candidates: [{ content: { parts: [{ text: "gemini" }] }, finishReason: "STOP" }] });
      }
      if (value.endsWith("/chat/completions")) {
        return jsonResponse({ choices: [{ message: { content: "kimi" }, finish_reason: "stop" }] });
      }
      throw new Error(`Unexpected URL: ${value}`);
    },
  });

  assert.deepEqual(await provider.listModelInfo(), [
    { id: "gpt-test", contextLength: 400000 },
    { id: "claude-test", contextLength: 200000 },
    { id: "gemini-test", contextLength: 1000000 },
    { id: "kimi-test", contextLength: 262144 },
  ]);
  assert.equal((await provider.complete({ model: "gpt-test", messages: [{ role: "user", content: "hi" }] })).text, "gpt");
  assert.equal((await provider.complete({ model: "claude-test", messages: [{ role: "user", content: "hi" }] })).text, "claude");
  assert.equal((await provider.complete({ model: "gemini-test", messages: [{ role: "user", content: "hi" }] })).text, "gemini");
  assert.equal((await provider.complete({ model: "kimi-test", messages: [{ role: "user", content: "hi" }] })).text, "kimi");
  assert.ok(requests.some((request) => request.url === "https://opencode.ai/zen/v1/responses"));
  assert.ok(requests.some((request) => request.url === "https://opencode.ai/zen/v1/messages"));
  assert.ok(requests.some((request) => request.url.includes("https://opencode.ai/zen/v1/models/gemini-test:generateContent")));
  assert.ok(requests.some((request) => request.url === "https://opencode.ai/zen/v1/chat/completions"));
});

test("OpenCode Go uses its dedicated base URL and falls back to family routing", async () => {
  const requests = [];
  const provider = new OpenCodeProvider({
    id: "opencode-go",
    apiKey: "test",
    catalogUrl: "https://catalog.invalid/api.json",
    fetch: async (url) => {
      const value = String(url);
      requests.push(value);
      if (value === "https://catalog.invalid/api.json") return new Response("down", { status: 503 });
      if (value.endsWith("/responses")) {
        return jsonResponse({ model: "gpt-5.6-luna", status: "completed", output: [{ type: "message", content: [{ type: "output_text", text: "ok" }] }] });
      }
      throw new Error(`Unexpected URL: ${value}`);
    },
  });
  const result = await provider.complete({ model: "gpt-5.6-luna", messages: [{ role: "user", content: "hi" }] });
  assert.equal(result.text, "ok");
  assert.ok(requests.includes("https://opencode.ai/zen/go/v1/responses"));
});

test("OpenCode Go provider-native failures keep the opencode-go provider id", async () => {
  const provider = new OpenCodeProvider({
    id: "opencode-go",
    apiKey: "test",
    catalogUrl: "https://catalog.invalid/api.json",
    fetch: async (url) => {
      const value = String(url);
      if (value === "https://catalog.invalid/api.json") return new Response("down", { status: 503 });
      if (value.endsWith("/responses")) {
        return new Response('{"error":{"message":"bad request"}}', {
          status: 400,
          headers: { "content-type": "application/json" },
        });
      }
      throw new Error(`Unexpected URL: ${value}`);
    },
  });

  await assert.rejects(
    () => provider.complete({ model: "muse-spark-1.2-contributor", messages: [{ role: "user", content: "hi" }] }),
    (error) => error?.provider === "opencode-go" && error?.status === 400,
  );
});

test("OpenCode Go Responses synthesis request omits tools when the harness disables them", async () => {
  let sent;
  const provider = new OpenCodeProvider({
    id: "opencode-go",
    apiKey: "test",
    catalogUrl: "https://catalog.invalid/api.json",
    fetch: async (url, init = {}) => {
      const value = String(url);
      if (value === "https://catalog.invalid/api.json") return new Response("down", { status: 503 });
      if (value.endsWith("/responses")) {
        sent = JSON.parse(init.body);
        return jsonResponse({
          model: "muse-spark-1.2-contributor",
          status: "completed",
          output: [{ type: "message", content: [{ type: "output_text", text: "synthesized" }] }],
        });
      }
      throw new Error(`Unexpected URL: ${value}`);
    },
  });

  const result = await provider.complete({
    model: "muse-spark-1.2-contributor",
    messages: [{ role: "user", content: "use existing evidence" }],
    tools: undefined,
  });
  assert.equal(result.text, "synthesized");
  assert.equal("tools" in sent, false);
  assert.equal("tool_choice" in sent, false);
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

test("OpenAI Responses stream preserves exposed reasoning summary and plaintext reasoning", async () => {
  const provider = new OpenAIProvider({
    apiKey: "test",
    fetch: async () => sseResponse([
      { type: "response.created", response: { id: "resp_reason", model: "gpt-test" } },
      { type: "response.reasoning_summary_text.delta", delta: "Inspecting" },
      { type: "response.reasoning_text.delta", delta: "full reasoning" },
      { type: "response.output_text.delta", delta: "answer" },
      { type: "response.completed", response: { status: "completed", output: [], usage: {} } },
    ]),
  });
  const events = [];
  for await (const event of provider.stream({ model: "gpt-test", messages: [{ role: "user", content: "go" }] })) events.push(event);
  assert.ok(events.some((event) => event.type === "reasoning-summary-delta" && event.delta === "Inspecting"));
  assert.ok(events.some((event) => event.type === "reasoning-delta" && event.delta === "full reasoning"));
  assert.ok(events.some((event) => event.type === "text-delta" && event.delta === "answer"));
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

test("Anthropic stream preserves thinking deltas when provider exposes them", async () => {
  const provider = new AnthropicProvider({
    apiKey: "test",
    fetch: async () => sseResponse([
      { type: "message_start", message: { id: "msg_reason", model: "claude-test", usage: { input_tokens: 1 } } },
      { type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: "reasoning" } },
      { type: "content_block_delta", index: 0, delta: { type: "thinking_summary_delta", summary: "summary" } },
        {
          type: "message_delta",
          delta: { stop_reason: "end_turn" },
          usage: { output_tokens: 7, output_tokens_details: { thinking_tokens: 5 } },
        },
    ]),
  });
  const events = [];
  for await (const event of provider.stream({ model: "claude-test", messages: [{ role: "user", content: "go" }] })) events.push(event);
  assert.ok(events.some((event) => event.type === "reasoning-delta" && event.delta === "reasoning"));
  assert.ok(events.some((event) => event.type === "reasoning-summary-delta" && event.delta === "summary"));
  assert.equal(events.at(-1).usage.reasoningTokens, 5);
});

test("Gemini stream emits text and function calls", async () => {
  const provider = new GeminiProvider({
    apiKey: "test",
    fetch: async () =>
      sseResponse([
        { candidates: [{ content: { parts: [{ text: "he" }] } }] },
        {
          candidates: [{ content: { parts: [{ functionCall: { name: "shell", args: { cmd: "pwd" } } }] }, finishReason: "STOP" }],
          usageMetadata: { promptTokenCount: 2, candidatesTokenCount: 3, thoughtsTokenCount: 4, totalTokenCount: 9 },
        },
      ]),
  });
  const events = [];
  for await (const event of provider.stream({ model: "gemini-test", messages: [{ role: "user", content: "go" }] })) events.push(event);
  assert.ok(events.some((event) => event.type === "text-delta" && event.delta === "he"));
  const tool = events.find((event) => event.type === "tool-call");
  assert.deepEqual(tool.toolCall.arguments, { cmd: "pwd" });
  assert.equal(events.at(-1).finishReason, "tool_call");
  assert.equal(events.at(-1).usage.reasoningTokens, 4);
});

test("Gemini stream separates thought parts from assistant text", async () => {
  const provider = new GeminiProvider({
    apiKey: "test",
    fetch: async () => sseResponse([
      { candidates: [{ content: { parts: [{ thought: true, text: "reasoning" }, { text: "answer" }] }, finishReason: "STOP" }] },
    ]),
  });
  const events = [];
  for await (const event of provider.stream({ model: "gemini-test", messages: [{ role: "user", content: "go" }] })) events.push(event);
  assert.ok(events.some((event) => event.type === "reasoning-delta" && event.delta === "reasoning"));
  assert.ok(events.some((event) => event.type === "text-delta" && event.delta === "answer"));
});

for (const error of [
  { type: "insufficient_quota", code: "credit_balance_exhausted" },
  { code: "insufficient_quota" },
  { code: "credit_balance_exhausted" },
]) {
  test(`billing exhaustion fails immediately: ${JSON.stringify(error)}`, async () => {
    let attempts = 0;
    const logs = [];
    const body = JSON.stringify({ error });
    await assert.rejects(() => providerFetch("https://api.openai.com/v1/responses", {}, {
      provider: "openai",
      fetch: async () => { attempts++; return new Response(body, { status: 429 }); },
      apiCallLogger: (entry) => logs.push(entry),
      retry: { maxAttempts: 3, baseDelayMs: 0, maxDelayMs: 0 },
    }), (failure) => {
      assert.ok(failure instanceof ProviderHTTPError);
      assert.equal(failure.retryable, false);
      assert.equal(failure.responseBody, body);
      assert.equal(failure.status, 429);
      assert.match(failure.message, /quota or credits exhausted/);
      return true;
    });
    assert.equal(attempts, 1);
    assert.deepEqual(logs.map((entry) => entry.outcome), ["error"]);
  });
}

for (const body of [JSON.stringify({ error: { code: "rate_limit_exceeded" } }), "busy", "null"]) {
  test(`temporary 429 remains retryable: ${body}`, async () => {
    let attempts = 0;
    const response = await providerFetch("https://api.openai.com/v1/responses", {}, {
      provider: "openai",
      fetch: async () => ++attempts === 1
        ? new Response(body, { status: 429 })
        : jsonResponse({ ok: true }),
      retry: { maxAttempts: 2, baseDelayMs: 0, maxDelayMs: 0 },
    });
    assert.equal(attempts, 2);
    assert.deepEqual(await response.json(), { ok: true });
  });
}

test("Codex OAuth sends subscription-compatible bodies for complete and stream", async () => {
  const requests = [];
  const provider = new OpenAIProvider({
    apiKey: "must-not-use",
    accessToken: "oauth-token", accountId: "account",
    fetch: async (url, init) => {
      assert.equal(String(url), "https://chatgpt.com/backend-api/codex/responses");
      assert.equal(init.headers.authorization, "Bearer oauth-token");
      requests.push(JSON.parse(init.body));
      return sseResponse([{ type: "response.completed", response: {
        id: "oauth-response", model: "gpt-5.6-sol", status: "completed",
        output: [
          { type: "message", content: [{ type: "output_text", text: "OK" }] },
          { type: "function_call", call_id: "call-1", name: "read", arguments: '{"path":"a"}' },
        ], usage: { input_tokens: 2, output_tokens: 3, total_tokens: 5 },
      } }]);
    },
  });
  const request = {
    model: "gpt-5.6-sol", temperature: 0.5, maxTokens: 100,
    metadata: { label: "test" }, messages: [{ role: "user", content: "hi", cacheBreakpoint: true }],
    providerOptions: { store: true, top_p: 0.9, service_tier: "flex", reasoning: { effort: "low" } },
  };
  const result = await provider.complete(request);
  assert.equal(result.text, "OK");
  assert.equal(result.toolCalls[0].id, "call-1");
  assert.deepEqual(result.toolCalls[0].arguments, { path: "a" });
  assert.equal(result.usage.totalTokens, 5);
  for await (const _event of provider.stream(request)) {}
  assert.equal(requests.length, 2);
  for (const body of requests) {
    assert.equal(body.store, false);
    assert.equal(body.stream, true);
    assert.equal(body.instructions, "");
    assert.deepEqual(body.reasoning, { effort: "low" });
    assert.equal(body.input[0].content, "hi");
    for (const key of ["temperature", "top_p", "max_output_tokens", "metadata", "prompt_cache_options", "service_tier"]) {
      assert.equal(key in body, false);
    }
  }
});

for (const events of [
  [{ type: "response.failed", response: { error: { message: "usage limit reached" } } }],
  [{ type: "error", message: "invalid request" }],
  [{ type: "response.created", response: { id: "unfinished" } }],
]) {
  test(`Responses stream reports failure: ${events[0].type}`, async () => {
    const provider = new OpenAIProvider({ accessToken: "oauth", accountId: "account", fetch: async () => sseResponse(events) });
    await assert.rejects(() => provider.complete({ model: "test", messages: [] }), /usage limit reached|invalid request|ended before/);
  });
}

test("Codex complete retains streamed output when terminal output is empty", async () => {
  const provider = new OpenAIProvider({ accessToken: "oauth", accountId: "account", fetch: async () => sseResponse([
    { type: "response.output_text.delta", delta: "OK" },
    { type: "response.reasoning_summary_text.delta", delta: "Summary" },
    { type: "response.output_item.added", output_index: 1, item: { type: "function_call", call_id: "call", name: "read", arguments: "" } },
    { type: "response.function_call_arguments.delta", output_index: 1, delta: '{"path":"a"}' },
    { type: "response.completed", response: { status: "completed", output: [] } },
  ]) });
  const result = await provider.complete({ model: "test", messages: [] });
  assert.equal(result.text, "OK");
  assert.equal(result.reasoningSummary, "Summary");
  assert.equal(result.finishReason, "tool_call");
  assert.deepEqual(result.toolCalls[0].arguments, { path: "a" });
});


test("OpenAI keeps a caller-selected stable cache boundary across tool rounds", async () => {
  const sent = [];
  const provider = new OpenAIProvider({ apiKey: "test", fetch: async (_url, init) => {
    sent.push(JSON.parse(init.body));
    return jsonResponse({ model: "gpt-5.6-luna", status: "completed", output: [] });
  } });
  const tools = [{ name: "apply_file_edits", inputSchema: {
    type: "object", properties: { path: { type: "string" }, snapshot: { type: "string" } },
    required: ["path"], additionalProperties: false,
  } }];
  const messages = [
    { role: "system", content: "stable instructions" },
    { role: "user", content: "edit the file", cacheBreakpoint: true },
    { role: "assistant", toolCalls: [{ id: "r1", name: "read_file", arguments: { path: "a.rs" } }] },
    { role: "tool", toolCallId: "r1", content: "source text" },
  ];
  await provider.complete({ model: "gpt-5.6-luna", contextKey: "session", promptCache: true, tools,
    messages: [...messages, { role: "system", content: "round 1", requestOnly: true }] });
  await provider.complete({ model: "gpt-5.6-luna", contextKey: "session", promptCache: true, tools,
    messages: [...messages,
      { role: "assistant", toolCalls: [{ id: "r2", name: "read_file", arguments: { path: "b.rs" } }] },
      { role: "tool", toolCallId: "r2", content: "more source" },
      { role: "system", content: "round 2", requestOnly: true },
    ] });
  assert.deepEqual(sent[0].input.slice(0, 3), sent[1].input.slice(0, 3));
  for (const body of sent) {
    assert.equal(body.tools[0].strict, false);
    assert.deepEqual(body.tools[0].parameters, tools[0].inputSchema);
    assert.deepEqual(body.input[0].content[0].prompt_cache_breakpoint, { mode: "explicit" });
    assert.equal(typeof body.input[2].output, "string");
    assert.equal(typeof body.input.at(-1).content, "string");
    assert.equal(body.prompt_cache_key, "session");
  }
  assert.equal(typeof sent[1].input[4].output, "string");
  await provider.complete({ model: "gpt-5.6-luna", messages, promptCache: false });
  assert.equal(sent[2].prompt_cache_options, undefined);
  assert.equal(JSON.stringify(sent[2]).includes("prompt_cache_breakpoint"), false);
});

test("OpenAI still synthesizes a cache boundary when the caller provides none", async () => {
  let sent;
  const provider = new OpenAIProvider({ apiKey: "test", fetch: async (_url, init) => {
    sent = JSON.parse(init.body);
    return jsonResponse({ model: "gpt-5.6-luna", status: "completed", output: [] });
  } });
  await provider.complete({
    model: "gpt-5.6-luna",
    promptCache: true,
    messages: [
      { role: "user", content: "inspect" },
      { role: "assistant", toolCalls: [{ id: "r1", name: "read_file", arguments: { path: "a.rs" } }] },
      { role: "tool", toolCallId: "r1", content: "source text" },
    ],
  });
  assert.deepEqual(sent.input[2].output[0].prompt_cache_breakpoint, { mode: "explicit" });
});

test("OpenAI treats a request-only caller breakpoint as the stable turn boundary", async () => {
  let sent;
  const provider = new OpenAIProvider({ apiKey: "test", fetch: async (_url, init) => {
    sent = JSON.parse(init.body);
    return jsonResponse({ model: "gpt-5.6-luna", status: "completed", output: [] });
  } });
  await provider.complete({
    model: "gpt-5.6-luna",
    promptCache: true,
    messages: [
      { role: "user", content: "fix it" },
      { role: "system", content: "stable turn memory", requestOnly: true, cacheBreakpoint: true },
      { role: "assistant", toolCalls: [{ id: "r1", name: "read_file", arguments: { path: "a.rs" } }] },
      { role: "tool", toolCallId: "r1", content: "source text" },
    ],
  });
  assert.deepEqual(sent.input[1].content[0].prompt_cache_breakpoint, { mode: "explicit" });
  assert.equal(typeof sent.input[3].output, "string");
});
