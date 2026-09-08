#!/usr/bin/env node
import readline from "node:readline";
import process from "node:process";
import { EditBackend } from "./backend.js";
import { loadEditBackendConfig } from "./config.js";
import type { ApplyRequest, ListFilesRequest, ReadRequest, SearchRequest } from "./types.js";

interface RpcRequest {
  id: string | number;
  method: string;
  params?: unknown;
}

function rootFromArgs(): string {
  const index = process.argv.indexOf("--root");
  if (index >= 0 && process.argv[index + 1]) return process.argv[index + 1]!;
  return process.cwd();
}

const config = await loadEditBackendConfig();
const backend = new EditBackend({
  root: rootFromArgs(),
  allowOutside: process.argv.includes("--allow-outside"),
  enforceSeenLines: config.enforceSeenLines,
  transactionDir: config.transactionDir,
  defaultDialect: config.defaultDialect,
  modelDialects: config.models,
});
await backend.initialize();

async function dispatch(request: RpcRequest): Promise<unknown> {
  switch (request.method) {
    case "health":
      return { ok: true, version: "0.1.0" };
    case "read":
      return backend.read(request.params as ReadRequest);
    case "search":
      return backend.search(request.params as SearchRequest);
    case "listFiles":
      return backend.listFiles((request.params ?? {}) as ListFilesRequest);
    case "preflight":
      return backend.preflight(request.params as ApplyRequest);
    case "apply":
      return backend.apply(request.params as ApplyRequest);
    case "applyDialect": {
      const params = request.params as {
        input: string;
        dialect?: string;
        model?: string;
        snapshots?: Record<string, string>;
        diagnostics?: boolean;
      };
      return backend.applyDialect(params.input, {
        ...(params.dialect !== undefined ? { dialect: params.dialect } : {}),
        ...(params.model !== undefined ? { model: params.model } : {}),
        ...(params.snapshots !== undefined ? { snapshots: params.snapshots } : {}),
        ...(params.diagnostics !== undefined ? { diagnostics: params.diagnostics } : {}),
      });
    }
    default:
      throw new Error(`Unknown RPC method: ${request.method}`);
  }
}

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of rl) {
  if (!line.trim()) continue;
  let request: RpcRequest | undefined;
  try {
    request = JSON.parse(line) as RpcRequest;
    const result = await dispatch(request);
    process.stdout.write(`${JSON.stringify({ id: request.id, result })}\n`);
  } catch (error) {
    process.stdout.write(`${JSON.stringify({
      id: request?.id ?? null,
      error: { message: error instanceof Error ? error.message : String(error) },
    })}\n`);
  }
}


