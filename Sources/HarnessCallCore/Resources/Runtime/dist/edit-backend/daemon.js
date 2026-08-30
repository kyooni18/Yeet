#!/usr/bin/env node
import readline from "node:readline";
import process from "node:process";
import { EditBackend } from "./backend.js";
import { loadEditBackendConfig } from "./config.js";
function rootFromArgs() {
    const index = process.argv.indexOf("--root");
    if (index >= 0 && process.argv[index + 1])
        return process.argv[index + 1];
    return process.cwd();
}
const config = await loadEditBackendConfig();
const backend = new EditBackend({
    root: rootFromArgs(),
    enforceSeenLines: config.enforceSeenLines,
    transactionDir: config.transactionDir,
    defaultDialect: config.defaultDialect,
    modelDialects: config.models,
});
await backend.initialize();
async function dispatch(request) {
    switch (request.method) {
        case "health":
            return { ok: true, version: "0.1.0" };
        case "read":
            return backend.read(request.params);
        case "preflight":
            return backend.preflight(request.params);
        case "apply":
            return backend.apply(request.params);
        case "applyDialect": {
            const params = request.params;
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
    if (!line.trim())
        continue;
    let request;
    try {
        request = JSON.parse(line);
        const result = await dispatch(request);
        process.stdout.write(`${JSON.stringify({ id: request.id, result })}\n`);
    }
    catch (error) {
        process.stdout.write(`${JSON.stringify({
            id: request?.id ?? null,
            error: { message: error instanceof Error ? error.message : String(error) },
        })}\n`);
    }
}
//# sourceMappingURL=daemon.js.map