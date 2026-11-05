import assert from "node:assert/strict";
import test from "node:test";
import { MistralProvider, createDefaultCore } from "../dist/index.js";

test("Mistral discovers models and sends tool-enabled chat requests to its hosted API", async () => {
  const calls = [];
  const provider = new MistralProvider({ apiKey: "vibe-key", fetch: async (url, init) => {
    calls.push({ url: String(url), ...init });
    const body = init.method === "GET"
      ? { data: [{ id: "devstral-test" }] }
      : { id: "response", model: "devstral-test", choices: [{ finish_reason: "tool_calls", message: { role: "assistant", content: null, tool_calls: [{ id: "abcdef123", type: "function", function: { name: "read_file", arguments: '{"path":"a"}' } }] } }] };
    return new Response(JSON.stringify(body), { headers: { "content-type": "application/json" } });
  } });
  assert.deepEqual(await provider.listModels(), ["devstral-test"]);
  const result = await provider.complete({ model: "devstral-test", messages: [{ role: "user", content: "read a" }], tools: [{ name: "read_file", inputSchema: { type: "object" } }] });
  assert.equal(calls[0].url, "https://api.mistral.ai/v1/models");
  assert.equal(calls[1].url, "https://api.mistral.ai/v1/chat/completions");
  assert.equal(calls[1].headers.authorization, "Bearer vibe-key");
  assert.equal(JSON.parse(calls[1].body).tools[0].function.name, "read_file");
  assert.deepEqual(result.toolCalls[0].arguments, { path: "a" });
  assert.ok(createDefaultCore().listProviders().includes("mistral"));
  await assert.rejects(new MistralProvider().listModels(), /Missing API key/);
});
