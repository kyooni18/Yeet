import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import http from "node:http";
import readline from "node:readline";
import test from "node:test";

function listen(server) {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => resolve(server.address()));
  });
}

function createBridge(environment = {}) {
  const child = spawn(process.execPath, [new URL("../dist/bridge.js", import.meta.url).pathname], {
    stdio: ["pipe", "pipe", "pipe"],
    env: { ...process.env, ...environment },
  });
  const lines = readline.createInterface({ input: child.stdout, crlfDelay: Infinity });
  const waiters = [];
  const backlog = [];

  lines.on("line", (line) => {
    const message = JSON.parse(line);
    const index = waiters.findIndex((waiter) => waiter.predicate(message));
    if (index >= 0) {
      const [waiter] = waiters.splice(index, 1);
      waiter.resolve(message);
    } else {
      backlog.push(message);
    }
  });

  const waitFor = (predicate) => {
    const index = backlog.findIndex(predicate);
    if (index >= 0) return Promise.resolve(backlog.splice(index, 1)[0]);
    return new Promise((resolve) => waiters.push({ predicate, resolve }));
  };

  const send = (value) => child.stdin.write(`${JSON.stringify(value)}\n`);
  const close = async () => {
    if (child.exitCode === null) {
      const id = "shutdown";
      send({ v: 1, id, op: "shutdown" });
      await waitFor((message) => message.id === id && message.type === "done");
      await new Promise((resolve) => child.once("exit", resolve));
    }
  };

  return { child, send, waitFor, close };
}

test("bridge exposes complete and stream over NDJSON", async (t) => {
  const server = http.createServer(async (req, res) => {
    let body = "";
    for await (const chunk of req) body += chunk;
    if (req.method === "GET" && req.url?.endsWith("/models")) {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ data: [{ id: "mock-model" }] }));
      return;
    }
    const request = JSON.parse(body);

    if (request.stream) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      res.write(`data: ${JSON.stringify({ id: "stream-1", model: request.model, choices: [{ delta: { content: "hello " }, finish_reason: null }] })}\n\n`);
      res.write(`data: ${JSON.stringify({ id: "stream-1", model: request.model, choices: [{ delta: { content: "swift" }, finish_reason: "stop" }] })}\n\n`);
      res.write(`data: ${JSON.stringify({ choices: [], usage: { prompt_tokens: 2, completion_tokens: 2, total_tokens: 4 } })}\n\n`);
      res.end("data: [DONE]\n\n");
      return;
    }

    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({
      id: "complete-1",
      model: request.model,
      choices: [{ message: { content: "hello swift" }, finish_reason: "stop" }],
      usage: { prompt_tokens: 2, completion_tokens: 2, total_tokens: 4 },
    }));
  });
  const address = await listen(server);
  t.after(() => server.close());

  const bridge = createBridge();
  t.after(() => bridge.child.kill());

  bridge.send({
    v: 1,
    id: "register",
    op: "register-provider",
    provider: {
      kind: "openai-compatible",
      id: "mock",
      baseUrl: `http://127.0.0.1:${address.port}/v1`,
    },
  });
  assert.equal((await bridge.waitFor((m) => m.id === "register")).type, "registered");

  bridge.send({ v: 1, id: "models", op: "list-models", provider: "mock" });
  const models = await bridge.waitFor((m) => m.id === "models");
  assert.equal(models.type, "models");
  assert.deepEqual(models.models, ["mock-model"]);

  bridge.send({
    v: 1,
    id: "complete",
    op: "complete",
    request: { model: "mock/mock-model", messages: [{ role: "user", content: "hi" }] },
  });
  const complete = await bridge.waitFor((m) => m.id === "complete");
  assert.equal(complete.type, "result");
  assert.equal(complete.result.text, "hello swift");
  assert.equal(complete.result.usage.totalTokens, 4);

  bridge.send({
    v: 1,
    id: "stream",
    op: "stream",
    request: { model: "mock/mock-model", messages: [{ role: "user", content: "hi" }] },
  });

  let text = "";
  let sawFinish = false;
  while (true) {
    const message = await bridge.waitFor((m) => m.id === "stream");
    if (message.type === "event" && message.event.type === "text-delta") text += message.event.delta;
    if (message.type === "event" && message.event.type === "finish") {
      sawFinish = true;
      assert.equal(message.event.usage.totalTokens, 4);
    }
    if (message.type === "done") break;
  }
  assert.equal(text, "hello swift");
  assert.equal(sawFinish, true);

  await bridge.close();
});

test("bridge cancellation aborts an in-flight provider request", async (t) => {
  let sawRequest;
  const requestSeen = new Promise((resolve) => { sawRequest = resolve; });
  const server = http.createServer(async (req, res) => {
    for await (const _chunk of req) { /* drain */ }
    res.writeHead(200, { "content-type": "application/json" });
    res.flushHeaders();
    sawRequest();
    // Intentionally leave the body open. The bridge cancellation must abort it.
  });
  const address = await listen(server);
  t.after(() => server.closeAllConnections?.());
  t.after(() => server.close());

  const bridge = createBridge();
  t.after(() => bridge.child.kill());

  bridge.send({
    v: 1,
    id: "register-cancel",
    op: "register-provider",
    provider: { kind: "openai-compatible", id: "cancel-mock", baseUrl: `http://127.0.0.1:${address.port}/v1` },
  });
  await bridge.waitFor((m) => m.id === "register-cancel" && m.type === "registered");

  bridge.send({
    v: 1,
    id: "slow-call",
    op: "complete",
    request: { model: "cancel-mock/slow", messages: [{ role: "user", content: "wait" }] },
  });
  await requestSeen;

  bridge.send({ v: 1, id: "cancel-command", op: "cancel", target: "slow-call" });
  const ack = await bridge.waitFor((m) => m.id === "cancel-command");
  assert.equal(ack.type, "cancelled");

  const cancelledCall = await bridge.waitFor((m) => m.id === "slow-call");
  assert.equal(cancelledCall.type, "error");
  assert.match(cancelledCall.error.message, /cancel|abort/i);

  await bridge.close();
});


test("bridge manages API-key auth and exposes its yeet config path", async (t) => {
  const { mkdtemp } = await import("node:fs/promises");
  const os = await import("node:os");
  const path = await import("node:path");
  const configDir = await mkdtemp(path.join(os.tmpdir(), "yeet-bridge-"));
  const bridge = createBridge({ YEET_CONFIG_DIR: configDir, OPENAI_API_KEY: "" });
  t.after(() => bridge.child.kill());

  bridge.send({ v: 1, id: "config", op: "config-path" });
  const config = await bridge.waitFor((m) => m.id === "config");
  assert.equal(config.type, "config-path");
  assert.equal(config.path, configDir);

  bridge.send({ v: 1, id: "status-empty", op: "auth-status", provider: "openai" });
  const empty = await bridge.waitFor((m) => m.id === "status-empty");
  assert.equal(empty.status.authenticated, false);
  assert.equal(empty.status.method, "none");

  bridge.send({ v: 1, id: "set-key", op: "auth-set-api-key", provider: "openai", apiKey: "sk-bridge" });
  const set = await bridge.waitFor((m) => m.id === "set-key");
  assert.equal(set.type, "auth-status");
  assert.equal(set.status.authenticated, true);
  assert.equal(set.status.method, "api-key");

  bridge.send({ v: 1, id: "logout", op: "auth-logout", provider: "openai" });
  const logout = await bridge.waitFor((m) => m.id === "logout");
  assert.equal(logout.status.authenticated, false);

  await bridge.close();
});
