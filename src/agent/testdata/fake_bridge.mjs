// Scripted stand-in for RuntimeSource/dist/bridge.js used by harness tests.
//
// Speaks the real bridge line protocol. Every `stream` request is appended to
// $YEET_FAKE_BRIDGE_LOG (one JSON request per line) and answered with the
// next scripted response from the JSON array in $YEET_FAKE_BRIDGE_QUEUE, or
// with a plain "ok" text reply when the queue is empty. Listing operations
// report no MCP servers or Skills; every other operation answers with a
// bridge error so callers exercise their unavailable paths.
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline";

const log = process.env.YEET_FAKE_BRIDGE_LOG;
const queue = process.env.YEET_FAKE_BRIDGE_QUEUE;
const write = (frame) => process.stdout.write(`${JSON.stringify({ v: 1, ...frame })}\n`);

function nextResponse() {
  if (!queue || !existsSync(queue)) return null;
  const items = JSON.parse(readFileSync(queue, "utf8") || "[]");
  const next = items.shift() ?? null;
  writeFileSync(queue, JSON.stringify(items));
  return next;
}

createInterface({ input: process.stdin }).on("line", (line) => {
  if (!line.trim()) return;
  const command = JSON.parse(line);
  const { id, op } = command;
  if (op === "ping") return write({ id, type: "pong" });
  if (op === "cancel") return;
  const empty = {
    "mcp-list-servers": { type: "mcp-servers", servers: [] },
    "skill-list": { type: "skills", skills: [] },
  }[op];
  if (empty) return write({ id, ...empty });
  if (op !== "stream") {
    return write({ id, type: "error", error: { name: "FakeBridge", message: `unsupported op ${op}` } });
  }
  if (log) appendFileSync(log, `${JSON.stringify(command.request)}\n`);
  const response = nextResponse() ?? { text: "ok" };
  write({ id, type: "event", event: { type: "start" } });
  if (response.text) write({ id, type: "event", event: { type: "text-delta", delta: response.text } });
  (response.toolCalls ?? []).forEach((toolCall, index) =>
    write({ id, type: "event", event: { type: "tool-call", index, toolCall } }));
  write({
    id,
    type: "event",
    event: {
      type: "finish",
      finishReason: response.toolCalls?.length ? "tool_call" : "stop",
      usage: { inputTokens: 10, outputTokens: 2, modelCalls: 1 },
    },
  });
  write({ id, type: "done" });
});
