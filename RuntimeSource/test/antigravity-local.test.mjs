import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  AntigravityLocalClient,
  AntigravityLocalProvider,
  AuthManager,
} from "../dist/index.js";

function fakeCatalog() {
  return {
    response: {
      userTier: "test-tier",
      models: {
        "gemini-2.5-flash-lite": {
          displayName: "Gemini 3.5 Flash Lite",
          model: "MODEL_GOOGLE_GEMINI_2_5_FLASH_LITE",
          maxTokens: 1_048_576,
          maxOutputTokens: 65_535,
          quotaInfo: { remainingFraction: 0.8, resetTime: "2026-10-02T00:00:00Z" },
        },
        "claude-sonnet-4-6": {
          displayName: "Claude Sonnet 4.6",
          model: "MODEL_CLAUDE_SONNET_4_6",
          maxTokens: 200_000,
          quotaInfo: { remainingFraction: 0.5 },
        },
        internal: {
          displayName: "Internal",
          model: "MODEL_INTERNAL",
          isInternal: true,
        },
      },
    },
  };
}

test("Antigravity local client uses the official local RPC session without OAuth tokens", async () => {
  const calls = [];
  const client = new AntigravityLocalClient({
    endpoint: { baseUrl: "https://127.0.0.1:61234", csrfToken: "test-csrf" },
    request: async (endpoint, method, body) => {
      calls.push({ endpoint, method, body });
      if (method === "GetAuthStatus") {
        return { authResult: { hasValidAuth: true, uiMessage: "signed in" } };
      }
      if (method === "GetAvailableModels") return fakeCatalog();
      if (method === "GetModelResponse") return { response: "ok" };
      throw new Error(`unexpected method ${method}`);
    },
  });

  assert.equal((await client.getAuthStatus()).hasValidAuth, true);
  const models = await client.getAvailableModels();
  assert.ok(models.response.models["gemini-2.5-flash-lite"]);
  assert.equal(
    await client.getModelResponse("hello", "MODEL_GOOGLE_GEMINI_2_5_FLASH_LITE"),
    "ok",
  );
  assert.deepEqual(calls.map((call) => call.method), [
    "GetAuthStatus",
    "GetAuthStatus",
    "GetAvailableModels",
    "GetAuthStatus",
    "GetModelResponse",
  ]);
});

test("AuthManager reports Antigravity browser auth and model quota through the local app", async () => {
  const configDir = await mkdtemp(path.join(os.tmpdir(), "yeet-antigravity-auth-"));
  let loginCalls = 0;
  const local = {
    async getAuthStatus() {
      return { hasValidAuth: true };
    },
    async loginInBrowser() {
      loginCalls += 1;
      return { hasValidAuth: true };
    },
    async logout() {},
    async getAvailableModels() {
      return fakeCatalog();
    },
  };
  const auth = new AuthManager({ configDir, antigravityLocalClient: local });

  const status = await auth.status("antigravity");
  assert.equal(status.authenticated, true);
  assert.equal(status.method, "browser");

  const loggedIn = await auth.loginInBrowser("antigravity");
  assert.equal(loggedIn.authenticated, true);
  assert.equal(loginCalls, 1);

  const usage = await auth.providerUsage("antigravity");
  assert.equal(usage.source, "antigravity-local");
  assert.equal(usage.plan, "test-tier");
  assert.ok(usage.windows.some((window) =>
    window.label === "Gemini 3.5 Flash Lite" && window.remainingPercent === 80));
});

test("Antigravity local provider maps account model ids and adapts Yeet tool calls", async () => {
  const modelCalls = [];
  const responses = [
    '{"type":"tool_call","name":"write_file","arguments":{"content":"ok"}}',
    '{"type":"final","text":"done"}',
  ];
  const client = {
    async getAvailableModels() {
      return fakeCatalog();
    },
    async getModelResponse(prompt, model) {
      modelCalls.push({ prompt, model });
      return responses.shift() ?? '{"type":"final","text":"done"}';
    },
  };
  const provider = new AntigravityLocalProvider({ client });

  const info = await provider.listModelInfo();
  assert.deepEqual(info.map((model) => model.id), [
    "gemini-2.5-flash-lite",
    "claude-sonnet-4-6",
  ]);
  assert.equal(info[0].contextLength, 1_048_576);

  const tool = {
    name: "write_file",
    description: "Write a test file.",
    inputSchema: {
      type: "object",
      properties: { content: { type: "string" } },
      required: ["content"],
    },
  };
  const user = { role: "user", content: "Write the test file." };
  const first = await provider.complete({
    model: "gemini-2.5-flash-lite",
    messages: [user],
    tools: [tool],
    toolChoice: "required",
  });
  assert.equal(first.finishReason, "tool_call");
  assert.equal(first.toolCalls[0].name, "write_file");
  assert.deepEqual(first.toolCalls[0].arguments, { content: "ok" });
  assert.equal(modelCalls[0].model, "MODEL_GOOGLE_GEMINI_2_5_FLASH_LITE");
  assert.match(modelCalls[0].prompt, /YEET TOOL PROTOCOL/);

  const second = await provider.complete({
    model: "gemini-2.5-flash-lite",
    messages: [
      user,
      { role: "assistant", toolCalls: first.toolCalls },
      {
        role: "tool",
        toolCallId: first.toolCalls[0].id,
        name: first.toolCalls[0].name,
        toolResult: {
          toolCallId: first.toolCalls[0].id,
          name: first.toolCalls[0].name,
          result: { ok: true },
        },
      },
    ],
    tools: [tool],
    toolChoice: "auto",
  });
  assert.equal(second.finishReason, "stop");
  assert.equal(second.text, "done");
  assert.equal(modelCalls[1].model, "MODEL_GOOGLE_GEMINI_2_5_FLASH_LITE");
  assert.match(modelCalls[1].prompt, /Tool write_file result/);
});
