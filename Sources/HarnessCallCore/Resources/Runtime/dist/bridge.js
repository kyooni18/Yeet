#!/usr/bin/env node
import { createInterface } from "node:readline";
import process from "node:process";
import { AuthManager } from "./auth.js";
import { createDefaultCore } from "./defaults.js";
import { McpManager } from "./mcp.js";
import { AnthropicProvider } from "./providers/anthropic.js";
import { GeminiProvider } from "./providers/gemini.js";
import { OpenAIChatProvider } from "./providers/openai-chat.js";
import { OpenAIProvider } from "./providers/openai.js";
import { OpenRouterProvider } from "./providers/openrouter.js";
import { SkillRegistry } from "./skills.js";
import { parseModelId } from "./types.js";
import { BRIDGE_PROTOCOL_VERSION, } from "./bridge-protocol.js";
const auth = new AuthManager();
await auth.ensure();
const skills = new SkillRegistry({ configDir: auth.configDir });
await skills.ensure();
const mcp = new McpManager({ configDir: auth.configDir });
await mcp.ensure();
const core = createDefaultCore();
const active = new Map();
const customProviders = new Map();
let shuttingDown = false;
function write(message) {
    process.stdout.write(`${JSON.stringify(message)}\n`);
}
function serializedError(error) {
    if (!(error instanceof Error))
        return { name: "Error", message: String(error) };
    const value = error;
    return {
        name: value.name || "Error",
        message: value.message,
        ...(value.stack ? { stack: value.stack } : {}),
        ...(value.provider !== undefined ? { provider: value.provider } : {}),
        ...(value.retryable !== undefined ? { retryable: value.retryable } : {}),
        ...(value.status !== undefined ? { status: value.status } : {}),
        ...(value.responseBody !== undefined ? { responseBody: value.responseBody } : {}),
        ...(value.requestId !== undefined ? { requestId: value.requestId } : {}),
        ...(value.code !== undefined ? { code: value.code } : {}),
        ...(value.server !== undefined ? { server: value.server } : {}),
    };
}
function errorMessage(id, error) {
    return { v: BRIDGE_PROTOCOL_VERSION, id, type: "error", error: serializedError(error) };
}
function validateCommand(value) {
    if (!value || typeof value !== "object")
        throw new Error("Bridge command must be a JSON object");
    const command = value;
    if (command.v !== BRIDGE_PROTOCOL_VERSION)
        throw new Error(`Unsupported bridge protocol version: ${String(command.v)}`);
    if (typeof command.id !== "string" || command.id.length === 0)
        throw new Error("Bridge command is missing id");
    if (typeof command.op !== "string")
        throw new Error("Bridge command is missing op");
    return value;
}
function apiKey(credential) {
    return credential?.kind === "api-key" ? credential.value : undefined;
}
function apiKeyOption(credential) {
    const value = apiKey(credential);
    return value !== undefined ? { apiKey: value } : {};
}
async function refreshProvider(providerId) {
    const credential = await auth.resolve(providerId);
    switch (providerId) {
        case "openai":
            core.register(new OpenAIProvider(apiKeyOption(credential)));
            return;
        case "anthropic":
            core.register(new AnthropicProvider(apiKeyOption(credential)));
            return;
        case "gemini":
            core.register(new GeminiProvider(credential?.kind === "oauth"
                ? { accessToken: credential.accessToken, ...(credential.projectId ? { projectId: credential.projectId } : {}) }
                : apiKeyOption(credential)));
            return;
        case "openrouter":
            core.register(new OpenRouterProvider(apiKeyOption(credential)));
            return;
    }
    const custom = customProviders.get(providerId);
    if (!custom)
        return;
    const key = custom.apiKey ?? apiKey(credential);
    core.register(new OpenAIChatProvider({
        id: custom.id,
        baseUrl: custom.baseUrl,
        ...(key !== undefined ? { apiKey: key } : {}),
        ...(custom.headers !== undefined ? { headers: custom.headers } : {}),
        ...(custom.requireApiKey !== undefined ? { requireApiKey: custom.requireApiKey } : {}),
    }));
}
for (const provider of ["openai", "anthropic", "gemini", "openrouter"])
    await refreshProvider(provider);
async function runComplete(command) {
    const controller = new AbortController();
    active.set(command.id, controller);
    try {
        await refreshProvider(parseModelId(command.request.model).provider);
        const result = await core.complete({ ...command.request, signal: controller.signal });
        write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "result", result });
    }
    catch (error) {
        write(errorMessage(command.id, error));
    }
    finally {
        active.delete(command.id);
    }
}
async function runStream(command) {
    const controller = new AbortController();
    active.set(command.id, controller);
    try {
        await refreshProvider(parseModelId(command.request.model).provider);
        for await (const event of core.stream({ ...command.request, signal: controller.signal })) {
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "event", event });
        }
        write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "done" });
    }
    catch (error) {
        write(errorMessage(command.id, error));
    }
    finally {
        active.delete(command.id);
    }
}
async function handle(command) {
    switch (command.op) {
        case "ping":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "pong" });
            return;
        case "list-providers":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "providers", providers: core.listProviders() });
            return;
        case "list-models": {
            await refreshProvider(command.provider);
            const models = await core.listModels(command.provider);
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "models", provider: command.provider, models });
            return;
        }
        case "config-path":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "config-path", path: auth.configDir });
            return;
        case "auth-status":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "auth-status", status: await auth.status(command.provider) });
            return;
        case "auth-set-api-key": {
            const status = await auth.setApiKey(command.provider, command.apiKey);
            await refreshProvider(command.provider);
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "auth-status", status });
            return;
        }
        case "auth-login-browser": {
            const status = await auth.loginInBrowser(command.provider, command.options ?? {});
            await refreshProvider(command.provider);
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "auth-status", status });
            return;
        }
        case "auth-logout": {
            const status = await auth.logout(command.provider);
            await refreshProvider(command.provider);
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "auth-status", status });
            return;
        }
        case "register-provider": {
            const provider = command.provider;
            if (provider.kind !== "openai-compatible")
                throw new Error(`Unsupported provider kind: ${String(provider.kind)}`);
            customProviders.set(provider.id, provider);
            await refreshProvider(provider.id);
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "registered", provider: provider.id });
            return;
        }
        case "unregister-provider": {
            customProviders.delete(command.provider);
            const removed = core.unregister(command.provider);
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "unregistered", provider: command.provider, removed });
            return;
        }
        case "complete":
            void runComplete(command);
            return;
        case "stream":
            void runStream(command);
            return;
        case "skill-list":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "skills", skills: await skills.list() });
            return;
        case "skill-load":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "skill", skill: await skills.load(command.skill) });
            return;
        case "skill-read":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "skill-file", content: await skills.readSkillFile(command.skill, command.path) });
            return;
        case "mcp-list-servers":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "mcp-servers", servers: await mcp.listServers() });
            return;
        case "mcp-set-server":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "mcp-server", server: await mcp.setServer(command.server) });
            return;
        case "mcp-remove-server":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "mcp-removed", removed: await mcp.removeServer(command.server) });
            return;
        case "mcp-list-tools":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "mcp-tools", tools: await mcp.listTools(command.server) });
            return;
        case "mcp-call-tool":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "mcp-tool-result", toolResult: await mcp.callTool(command.server, command.tool, command.arguments ?? {}) });
            return;
        case "mcp-list-resources":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "mcp-resources", resources: await mcp.listResources(command.server) });
            return;
        case "mcp-read-resource":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "mcp-resource-result", resourceResult: await mcp.readResource(command.server, command.uri) });
            return;
        case "mcp-list-prompts":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "mcp-prompts", prompts: await mcp.listPrompts(command.server) });
            return;
        case "mcp-get-prompt":
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "mcp-prompt-result", promptResult: await mcp.getPrompt(command.server, command.prompt, command.arguments ?? {}) });
            return;
        case "mcp-disconnect":
            if (command.server)
                await mcp.disconnect(command.server);
            else
                await mcp.close();
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "done" });
            return;
        case "cancel":
            active.get(command.target)?.abort(new Error("Cancelled by bridge client"));
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "cancelled", target: command.target });
            return;
        case "shutdown":
            shuttingDown = true;
            for (const controller of active.values())
                controller.abort(new Error("Bridge shutting down"));
            active.clear();
            await mcp.close();
            write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "done" });
            setTimeout(() => process.exit(0), 0);
            return;
    }
}
const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
input.on("line", (line) => {
    if (shuttingDown || line.trim().length === 0)
        return;
    let raw;
    try {
        raw = JSON.parse(line);
    }
    catch (error) {
        process.stderr.write(`harness-call-core bridge: invalid JSON: ${String(error)}\n`);
        return;
    }
    let command;
    try {
        command = validateCommand(raw);
    }
    catch (error) {
        const candidateId = raw && typeof raw === "object" && typeof raw.id === "string"
            ? String(raw.id) : "protocol";
        write(errorMessage(candidateId, error));
        return;
    }
    void handle(command).catch((error) => write(errorMessage(command.id, error)));
});
input.on("close", () => {
    for (const controller of active.values())
        controller.abort(new Error("Bridge stdin closed"));
    active.clear();
    void mcp.close().finally(() => process.exit(0));
});
process.on("uncaughtException", (error) => {
    process.stderr.write(`harness-call-core bridge uncaughtException: ${error.stack ?? error.message}\n`);
    process.exitCode = 1;
});
process.on("unhandledRejection", (reason) => {
    process.stderr.write(`harness-call-core bridge unhandledRejection: ${String(reason)}\n`);
    process.exitCode = 1;
});
//# sourceMappingURL=bridge.js.map