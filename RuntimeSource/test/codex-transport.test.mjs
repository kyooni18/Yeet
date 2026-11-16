import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import test from "node:test";
import { OpenAIProvider } from "../dist/index.js";

test("Codex completes terminal SSE events without waiting for the server to close", async () => {
  for (const type of ["response.completed", "response.incomplete"]) {
    for (const stream of [false, true]) {
      let controller;
      let cancelled = false;
      const body = new ReadableStream({
        start(value) {
          controller = value;
          value.enqueue(new TextEncoder().encode(`data: ${JSON.stringify({ type, response: {
            id: "terminal", status: type.slice("response.".length), output: [], usage: {},
          } })}\n\n`));
          // Deliberately leave the connection open, as a proxy/keepalive can do.
        },
        cancel() { cancelled = true; },
      });
      const provider = new OpenAIProvider({
        id: "codex-cli", accessToken: "test", accountId: "account",
        fetch: async () => new Response(body, { headers: { "content-type": "text/event-stream" } }),
      });
      let timer;
      try {
        const request = { model: "test", messages: [{ role: "user", content: "hi" }] };
        const result = stream
          ? (async () => { const events = []; for await (const event of provider.stream(request)) events.push(event); return events.at(-1); })()
          : provider.complete(request);
        const terminal = await Promise.race([result, new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error("terminal event did not finish the request")), 1_000);
        })]);
        if (stream) assert.equal(terminal.type, "finish");
        else assert.equal(terminal.id, "terminal");
        assert.equal(cancelled, true);
      } finally {
        clearTimeout(timer);
        if (!cancelled) controller.close();
      }
    }
  }
});

test("Codex catalog refresh times out stalled headers and bodies instead of hanging", { timeout: 5_000 }, async (t) => {
  const server = createServer((request, response) => {
    if (request.url.startsWith("/body/")) {
      response.writeHead(200, { "content-type": "application/json" });
      response.write('{"models":[');
    }
    // Neither route finishes its response.
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  t.after(() => { server.closeAllConnections(); server.close(); });
  const timeout = AbortSignal.timeout.bind(AbortSignal);
  t.mock.method(AbortSignal, "timeout", (ms) => {
    assert.ok(ms > 0 && ms <= 30_000, "interactive catalog discovery needs a bounded deadline");
    return timeout(50);
  });
  for (const route of ["headers", "body"]) {
    const provider = new OpenAIProvider({
      id: "codex-cli", accessToken: "test", accountId: "account",
      baseUrl: `http://127.0.0.1:${server.address().port}/${route}`,
    });
    await assert.rejects(() => provider.listModels(), /timeout|aborted/i);
  }
});
