import assert from "node:assert/strict";
import test from "node:test";
import { CallCore } from "../dist/core.js";
import { OpenAIProvider } from "../dist/providers/openai.js";
import { OpenAIChatProvider } from "../dist/providers/openai-chat.js";
import { OpenRouterProvider } from "../dist/providers/openrouter.js";
import { GeminiProvider } from "../dist/providers/gemini.js";
const response = value => new Response(JSON.stringify(value), { headers: { "content-type": "application/json" } });

test("embedding calls use Yeet core routing, configured credentials, endpoint and response ordering", async () => {
  let sent;
  const provider = new OpenAIChatProvider({
    id: "custom", baseUrl: "https://example.invalid/v1", apiKey: "test-only",
    fetch: async (url, init) => {
      sent = { url, headers: init.headers, body: JSON.parse(init.body) };
      return response({ model: "embed-v1", data: [{ index: 1, embedding: [3, 4] }, { index: 0, embedding: [1, 2] }] });
    },
  });
  const core = new CallCore([provider]);
  const result = await core.embed({ model: "custom/embed-v1", input: ["document", "query"] });
  assert.equal(sent.url, "https://example.invalid/v1/embeddings");
  assert.equal(sent.headers.authorization, "Bearer test-only");
  assert.deepEqual(sent.body, { model: "embed-v1", input: ["document", "query"], encoding_format: "float" });
  assert.equal(result.model, "custom/embed-v1");
  assert.deepEqual(result.vectors, [[1, 2], [3, 4]]);
  assert.equal(result.source.length, 64);
  assert.ok(!JSON.stringify(result).includes("test-only"));
});

test("OpenAI and OpenRouter expose embeddings independently of completion APIs", async () => {
  for (const [Adapter, expected] of [[OpenAIProvider, "https://api.openai.com/v1/embeddings"], [OpenRouterProvider, "https://openrouter.ai/api/v1/embeddings"]]) {
    let url;
    const provider = new Adapter({ apiKey: "fixture-key", fetch: async (target) => {
      url = target;
      return response({ model: "embedding-model", data: [{ index: 0, embedding: [0.1, 0.2] }] });
    } });
    assert.equal((await provider.embed({ model: "embedding-model", input: ["text"] })).vectors.length, 1);
    assert.equal(url, expected);
  }
});

test("Gemini embeddings reuse Yeet authentication and batch format", async () => {
  let sent;
  const provider = new GeminiProvider({ apiKey: "fixture-key", fetch: async (url, init) => {
    sent = { url, headers: init.headers, body: JSON.parse(init.body) };
    return response({ embeddings: [{ values: [1, 2, 3] }] });
  } });
  const result = await provider.embed({ model: "gemini-embedding-001", input: ["한국어 memory"] });
  assert.ok(sent.url.endsWith("/models/gemini-embedding-001:batchEmbedContents"));
  assert.equal(sent.headers["x-goog-api-key"], "fixture-key");
  assert.deepEqual(sent.body.requests[0], { model: "models/gemini-embedding-001", content: { parts: [{ text: "한국어 memory" }] } });
  assert.deepEqual(result.vectors, [[1, 2, 3]]);
});

test("embedding transport rejects incomplete, duplicated, invalid and mixed-dimensional vectors", async () => {
  const invalid = [
    { data: [] },
    { data: [{ index: 0, embedding: [0, 0] }] },
    { data: [{ index: 0, embedding: ["1", 2] }] },
    { data: [{ index: 1, embedding: [1, 2] }] },
    { data: [{ index: 0, embedding: [1, 2] }, { index: 0, embedding: [1, 2] }] },
    { data: [{ index: 0, embedding: [1, 2] }, { index: 1, embedding: [1, 2, 3] }] },
  ];
  for (const raw of invalid) {
    const provider = new OpenAIChatProvider({ fetch: async () => response(raw) });
    await assert.rejects(provider.embed({ model: "embed", input: raw.data.length === 2 ? ["one", "two"] : ["one"] }));
  }
});

test("provider source identity changes with endpoint but not credential rotation", async () => {
  const run = (baseUrl, apiKey) => new OpenAIChatProvider({ baseUrl, apiKey,
    fetch: async () => response({ data: [{ index: 0, embedding: [1, 2] }] }),
  }).embed({ model: "embed", input: ["test"] });
  const original = await run("https://one.invalid/v1", "key-a");
  assert.equal(original.source, (await run("https://one.invalid/v1", "key-b")).source);
  assert.notEqual(original.source, (await run("https://two.invalid/v1", "key-a")).source);
});
