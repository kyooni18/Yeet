#!/usr/bin/env node

import { createInterface } from "node:readline";
import process from "node:process";
import { AuthManager, type ResolvedCredential } from "./auth.js";
import { createDefaultCore } from "./defaults.js";
import type { ProviderFetchLog } from "./http.js";
import { McpManager } from "./mcp.js";
import { ModelMetadataCatalog } from "./model-metadata.js";
import { applyCacheCostPolicy, applyOpenAIFlexAuthPolicy, cheapestModel, estimateUsageCostUsd, estimatedRequestTokens } from "./pricing.js";
import { AnthropicProvider } from "./providers/anthropic.js";
import { GeminiProvider } from "./providers/gemini.js";
import { OpenAIChatProvider } from "./providers/openai-chat.js";
import { OpenAIProvider } from "./providers/openai.js";
import { OpenCodeProvider } from "./providers/opencode.js";
import { OpenRouterProvider } from "./providers/openrouter.js";
import { createRequestCapabilityRegistry } from "./request-capabilities.js";
import { withReasoningPolicy } from "./request-policy.js";
import { SkillRegistry } from "./skills.js";
import { parseModelId } from "./types.js";
import { createVisionCapability, VISION_CAPABILITY_ID } from "./vision.js";
import {
  BRIDGE_PROTOCOL_VERSION,
  type BridgeEvent,
  type BridgeCommand,
  type BridgeMessage,
  type NativeAppApprovalDecision,
  type NativeAppApprovalRequest,
  type OpenAICompatibleProviderConfig,
  type SerializedBridgeError,
} from "./bridge-protocol.js";

const auth = new AuthManager();
await auth.ensure();
const skills = new SkillRegistry({ configDir: auth.configDir });
await skills.ensure();
const nativeApprovalWaiters = new Map<string, (approved: boolean) => void>();
function requestNativeAppApproval(request: NativeAppApprovalRequest, signal?: AbortSignal): Promise<boolean> {
  return new Promise((resolve) => {
    if (signal?.aborted) { resolve(false); return; }
    nativeApprovalWaiters.set(request.requestId, resolve);
    const onAbort = () => {
      nativeApprovalWaiters.get(request.requestId)?.(false);
      nativeApprovalWaiters.delete(request.requestId);
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    const event: BridgeEvent = { v: BRIDGE_PROTOCOL_VERSION, type: "native_app_approval_request", ...request };
    try { process.stdout.write(`${JSON.stringify(event)}\n`); }
    catch { nativeApprovalWaiters.delete(request.requestId); signal?.removeEventListener("abort", onAbort); resolve(false); }
  });
}
function denyPendingNativeApprovals(): void {
  for (const resolve of nativeApprovalWaiters.values()) resolve(false);
  nativeApprovalWaiters.clear();
}
const mcp = new McpManager({ configDir: auth.configDir, nativeAppApproval: requestNativeAppApproval });
await mcp.ensure();
const modelMetadata = new ModelMetadataCatalog();
function writeApiCallLog(entry: ProviderFetchLog): void {
  process.stderr.write(`api-call ${JSON.stringify(entry)}\n`);
}

const core = createDefaultCore({ apiCallLogger: writeApiCallLog });
const active = new Map<string, AbortController>();
const customProviders = new Map<string, OpenAICompatibleProviderConfig>();
let shuttingDown = false;

async function contextLength(model: string): Promise<number | undefined> {
  const parsed = parseModelId(model);
  let context: number | undefined;
  try {
    await refreshProvider(parsed.provider);
    context = (await core.listModelInfo(parsed.provider)).find((candidate) => candidate.id === parsed.model)?.contextLength;
  } catch {
    // Provider discovery is best-effort; the shared catalog is the fallback.
  }
  return context ?? await modelMetadata.contextLength(model);
}

async function enrichedModelInfo(provider: string): Promise<import("./types.js").ModelInfo[]> {
  const info = await core.listModelInfo(provider);
  try {
    return await Promise.all(info.map((model) => modelMetadata.enrich(provider, model)));
  } catch {
    return info;
  }
}

function auxiliaryPurpose(request: import("./types.js").CallRequest): boolean {
  return ["session-title", "context-compaction", "command-evaluation"].includes(request.metadata?.purpose ?? "");
}

async function costAwareRequest(request: import("./types.js").CallRequest): Promise<import("./types.js").CallRequest> {
  let prepared = request;
  const parsed = parseModelId(prepared.model);
  if (parsed.provider === "openai" && prepared.providerOptions?.service_tier === "flex") {
    const status = await auth.status("openai").catch(() => undefined);
    prepared = applyOpenAIFlexAuthPolicy(prepared, status?.method);
  }
  if (auxiliaryPurpose(prepared)) {
    try {
      const needed = estimatedRequestTokens(prepared) + (prepared.maxTokens ?? 1_024);
      const vendorPrefix = parsed.model.includes("/") ? `${parsed.model.split("/", 1)[0]}/` : undefined;
      const candidates = (await enrichedModelInfo(parsed.provider)).filter((candidate) =>
        !vendorPrefix || candidate.id.startsWith(vendorPrefix));
      const textCandidates = [] as import("./types.js").ModelInfo[];
      for (const candidate of candidates) {
        const full = `${parsed.provider}/${candidate.id}`;
        const supported = await modelMetadata.supportsTextGeneration(full).catch(() => undefined);
        if (supported !== false) textCandidates.push(candidate);
      }
      const cheapest = cheapestModel(textCandidates, prepared, needed);
      if (cheapest && cheapest.id !== parsed.model) {
        prepared = {
          ...prepared,
          model: `${parsed.provider}/${cheapest.id}` as import("./types.js").ModelId,
          metadata: {
            ...(prepared.metadata ?? {}),
            costRoutedFrom: request.model,
            costRoutedTo: `${parsed.provider}/${cheapest.id}`,
          },
        };
      }
    } catch {
      // Pricing/model discovery is an optimization; never block the lead task.
    }
  }
  const pricing = await modelMetadata.pricing(prepared.model).catch(() => undefined);
  return applyCacheCostPolicy(prepared, pricing);
}

async function usageWithEstimatedCost(
  request: import("./types.js").CallRequest,
  usage: import("./types.js").Usage | undefined,
): Promise<import("./types.js").Usage | undefined> {
  if (!usage) return undefined;
  const pricing = await modelMetadata.pricing(request.model).catch(() => undefined);
  const estimatedCostUsd = pricing ? estimateUsageCostUsd(pricing, usage, request) : undefined;
  return estimatedCostUsd === undefined ? usage : { ...usage, estimatedCostUsd };
}

async function compactionContextLength(model: string): Promise<number | undefined> {
  const parsed = parseModelId(model);
  // An OpenAI-compatible custom endpoint may not implement /models at all,
  // and model discovery must never sit on the critical path of a completion.
  // Unknown windows simply disable LLM compaction; deterministic tool-history
  // thinning still runs for every request.
  if (customProviders.has(parsed.provider)) {
    // Do not put a custom provider's optional /models endpoint on the critical
    // completion path. The shared catalog can still resolve many compatible
    // model ids without touching that endpoint.
    try { return await modelMetadata.contextLength(model); }
    catch { return undefined; }
  }
  return contextLength(model);
}

const harnessCapabilities = createRequestCapabilityRegistry({
  contextLength: compactionContextLength,
  complete: async (request) => {
    await refreshProvider(parseModelId(request.model).provider);
    const costAware = await costAwareRequest(request);
    const prepared = withReasoningPolicy(costAware);
    await refreshProvider(parseModelId(prepared.model).provider);
    const result = await core.complete(prepared);
    const usage = await usageWithEstimatedCost(prepared, result.usage);
    return { ...result, ...(usage ? { usage } : {}) };
  },
});

function write(message: BridgeMessage): void {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function serializedError(error: unknown): SerializedBridgeError {
  if (!(error instanceof Error)) return { name: "Error", message: String(error) };
  const value = error as Error & {
    provider?: unknown;
    retryable?: unknown;
    status?: unknown;
    responseBody?: unknown;
    requestId?: unknown;
    code?: unknown;
    server?: unknown;
  };
  const string = (candidate: unknown): string | undefined =>
    typeof candidate === "string" ? candidate : undefined;
  const boolean = (candidate: unknown): boolean | undefined =>
    typeof candidate === "boolean" ? candidate : undefined;
  const number = (candidate: unknown): number | undefined =>
    typeof candidate === "number" && Number.isFinite(candidate) ? candidate : undefined;
  const code = value.code === undefined || value.code === null
    ? undefined
    : typeof value.code === "string" || typeof value.code === "number"
      ? String(value.code)
      : undefined;
  const provider = string(value.provider);
  const retryable = boolean(value.retryable);
  const status = number(value.status);
  const responseBody = string(value.responseBody);
  const requestId = string(value.requestId);
  const server = string(value.server);
  return {
    name: value.name || "Error",
    message: value.message,
    ...(value.stack ? { stack: value.stack } : {}),
    ...(provider !== undefined ? { provider } : {}),
    ...(retryable !== undefined ? { retryable } : {}),
    ...(status !== undefined ? { status } : {}),
    ...(responseBody !== undefined ? { responseBody } : {}),
    ...(requestId !== undefined ? { requestId } : {}),
    ...(code !== undefined ? { code } : {}),
    ...(server !== undefined ? { server } : {}),
  };
}

function errorMessage(id: string, error: unknown): BridgeMessage {
  return { v: BRIDGE_PROTOCOL_VERSION, id, type: "error", error: serializedError(error) };
}

function validateCommand(value: unknown): BridgeCommand {
  if (!value || typeof value !== "object") throw new Error("Bridge command must be a JSON object");
  const command = value as Partial<BridgeCommand> & Record<string, unknown>;
  if (command.v !== BRIDGE_PROTOCOL_VERSION) throw new Error(`Unsupported bridge protocol version: ${String(command.v)}`);
  if (typeof command.id !== "string" || command.id.length === 0) throw new Error("Bridge command is missing id");
  if (typeof command.op !== "string") throw new Error("Bridge command is missing op");
  return value as BridgeCommand;
}

function apiKey(credential: ResolvedCredential | undefined): string | undefined {
  return credential?.kind === "api-key" ? credential.value : undefined;
}

function apiKeyOption(credential: ResolvedCredential | undefined): { apiKey: string } | {} {
  const value = apiKey(credential);
  return value !== undefined ? { apiKey: value } : {};
}

function openAIProviderOptions(credential: ResolvedCredential | undefined): Record<string, unknown> {
  if (credential?.kind === "oauth") {
    return {
      accessToken: credential.accessToken,
      ...(credential.accountId ? { accountId: credential.accountId } : {}),
    };
  }
  return apiKeyOption(credential);
}

async function refreshProvider(providerId: string): Promise<void> {
  const credential = await auth.resolve(providerId);
  switch (providerId) {
    case "openai":
      core.register(new OpenAIProvider({ ...openAIProviderOptions(credential), apiCallLogger: writeApiCallLog }));
      return;
    case "anthropic":
      core.register(new AnthropicProvider({ ...apiKeyOption(credential), apiCallLogger: writeApiCallLog }));
      return;
    case "gemini":
      core.register(new GeminiProvider(
        credential?.kind === "oauth"
          ? { accessToken: credential.accessToken, ...(credential.projectId ? { projectId: credential.projectId } : {}), apiCallLogger: writeApiCallLog }
          : { ...apiKeyOption(credential), apiCallLogger: writeApiCallLog },
      ));
      return;
    case "openrouter":
      core.register(new OpenRouterProvider({ ...apiKeyOption(credential), apiCallLogger: writeApiCallLog }));
      return;
    case "opencode":
    case "opencode-go":
      core.register(new OpenCodeProvider({ id: providerId, ...apiKeyOption(credential), apiCallLogger: writeApiCallLog }));
      return;
  }

  const custom = customProviders.get(providerId);
  if (!custom) return;
  const key = custom.apiKey ?? apiKey(credential);
  core.register(new OpenAIChatProvider({
    id: custom.id,
    baseUrl: custom.baseUrl,
    ...(key !== undefined ? { apiKey: key } : {}),
    ...(custom.headers !== undefined ? { headers: custom.headers } : {}),
    ...(custom.requireApiKey !== undefined ? { requireApiKey: custom.requireApiKey } : {}),
    apiCallLogger: writeApiCallLog,
  }));
}

for (const provider of ["openai", "anthropic", "gemini", "openrouter", "opencode", "opencode-go"]) await refreshProvider(provider);
for (const provider of await auth.listCustomProviders()) {
  customProviders.set(provider.id, { kind: "openai-compatible", ...provider });
  await refreshProvider(provider.id);
}

async function runComplete(command: Extract<BridgeCommand, { op: "complete" }>): Promise<void> {
  const controller = new AbortController();
  active.set(command.id, controller);
  try {
    await refreshProvider(parseModelId(command.request.model).provider);
    ensureVisionAttached(command.request);
    const costAware = await costAwareRequest(command.request);
    const preparation = await harnessCapabilities.prepareWithUsage(withReasoningPolicy(costAware));
    const result = await core.complete({ ...preparation.request, signal: controller.signal });
    const primaryUsage = await usageWithEstimatedCost(preparation.request, result.usage);
    write({
      v: BRIDGE_PROTOCOL_VERSION,
      id: command.id,
      type: "result",
      result: { ...result, usage: mergeUsageWithModelCall(primaryUsage, preparation.auxiliaryUsage) },
    });
  } catch (error) {
    write(errorMessage(command.id, error));
  } finally {
    active.delete(command.id);
  }
}

async function runStream(command: Extract<BridgeCommand, { op: "stream" }>): Promise<void> {
  const controller = new AbortController();
  active.set(command.id, controller);
  try {
    await refreshProvider(parseModelId(command.request.model).provider);
    ensureVisionAttached(command.request);
    const costAware = await costAwareRequest(command.request);
    const preparation = await harnessCapabilities.prepareWithUsage(withReasoningPolicy(costAware));
    for await (const event of core.stream({ ...preparation.request, signal: controller.signal })) {
      const preparedEvent = event.type === "finish"
        ? { ...event, usage: mergeUsageWithModelCall(
          await usageWithEstimatedCost(preparation.request, event.usage),
          preparation.auxiliaryUsage,
        ) }
        : event;
      write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "event", event: preparedEvent });
    }
    write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "done" });
  } catch (error) {
    write(errorMessage(command.id, error));
  } finally {
    active.delete(command.id);
  }
}

function ensureVisionAttached(request: import("./types.js").CallRequest): void {
  if (!request.messages.some((message) => (message.images?.length ?? 0) > 0)) return;
  const attached = request.attachedCapabilities ?? harnessCapabilities.defaultAttached();
  if (!attached.includes(VISION_CAPABILITY_ID)) {
    throw new Error("Vision capability is not attached to this request");
  }
}

async function runMcpCallTool(command: Extract<BridgeCommand, { op: "mcp-call-tool" }>): Promise<void> {
  const controller = new AbortController();
  active.set(command.id, controller);
  try {
    const toolResult = await mcp.callTool(command.server, command.tool, command.arguments ?? {}, controller.signal);
    write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "mcp-tool-result", toolResult });
  } catch (error) {
    write(errorMessage(command.id, error));
  } finally {
    active.delete(command.id);
  }
}

function mergeUsageWithModelCall(primary: import("./types.js").Usage | undefined, auxiliary: import("./types.js").Usage | undefined): import("./types.js").Usage {
  const add = (a: number | undefined, b: number | undefined): number | undefined =>
    a === undefined && b === undefined ? undefined : (a ?? 0) + (b ?? 0);
  const values: Array<[keyof import("./types.js").Usage, number | undefined]> = [
    ["inputTokens", add(primary?.inputTokens, auxiliary?.inputTokens)],
    ["outputTokens", add(primary?.outputTokens, auxiliary?.outputTokens)],
    ["totalTokens", add(primary?.totalTokens, auxiliary?.totalTokens)],
    ["cachedInputTokens", add(primary?.cachedInputTokens, auxiliary?.cachedInputTokens)],
    ["cacheWriteInputTokens", add(primary?.cacheWriteInputTokens, auxiliary?.cacheWriteInputTokens)],
    ["reasoningTokens", add(primary?.reasoningTokens, auxiliary?.reasoningTokens)],
    ["modelCalls", (primary?.modelCalls ?? 1) + (auxiliary?.modelCalls ?? 0)],
    ["estimatedCostUsd", add(primary?.estimatedCostUsd, auxiliary?.estimatedCostUsd)],
  ];
  return Object.fromEntries(values.filter(([, value]) => value !== undefined)) as import("./types.js").Usage;
}

async function handle(command: BridgeCommand): Promise<void> {
  switch (command.op) {
    case "embedding-models": {
      const models: string[] = [];
      // Preference is used only when creating an index. Existing indices never reselect.
      for (const [provider, model] of [["openai", "text-embedding-3-small"], ["gemini", "gemini-embedding-001"], ["openrouter", "openai/text-embedding-3-small"]]) {
        const credential = await auth.resolve(provider!);
        if (credential?.kind === "api-key") models.push(`${provider}/${model}`);
      }
      for (const provider of customProviders.keys()) {
        try {
          await refreshProvider(provider);
          for (const model of await core.listModels(provider)) {
            if (/embed|(?:^|[-/])(?:bge|e5|nomic)(?:[-/]|$)/i.test(model)) models.push(`${provider}/${model}`);
          }
        } catch { /* Custom selection remains available explicitly if discovery is unsupported. */ }
      }
      write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "embedding-models", models });
      return;
    }
    case "embed": {
      const controller = new AbortController();
      active.set(command.id, controller);
      try {
        await refreshProvider(parseModelId(command.model).provider);
        const result = await core.embed({ model: command.model, input: command.input, signal: controller.signal });
        write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "embedding-result", result });
      } finally { active.delete(command.id); }
      return;
    }
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
    case "list-model-info": {
      await refreshProvider(command.provider);
      const models = await enrichedModelInfo(command.provider);
      write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "model-info", provider: command.provider, modelInfo: models });
      return;
    }
    case "context-length": {
      const resolvedContextLength = await contextLength(command.model);
      write({
        v: BRIDGE_PROTOCOL_VERSION,
        id: command.id,
        type: "context-length",
        model: command.model,
        ...(resolvedContextLength !== undefined ? { contextLength: resolvedContextLength } : {}),
      });
      return;
    }
    case "list-harness-capabilities":
      write({
        v: BRIDGE_PROTOCOL_VERSION,
        id: command.id,
        type: "harness-capabilities",
        capabilities: harnessCapabilities.list(),
      });
      return;
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
    case "list-provider-configurations": {
      const providers: OpenAICompatibleProviderConfig[] = (await auth.listCustomProviders()).map((provider) => ({
        kind: "openai-compatible",
        ...provider,
      }));
      write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "provider-configurations", providerConfigurations: providers });
      return;
    }
    case "save-provider-configuration": {
      const provider = command.provider;
      if (provider.kind !== "openai-compatible") throw new Error(`Unsupported provider kind: ${String(provider.kind)}`);
      const saved = await auth.setCustomProvider({
        id: provider.id,
        baseUrl: provider.baseUrl,
        ...(provider.headers !== undefined ? { headers: provider.headers } : {}),
        ...(provider.requireApiKey !== undefined ? { requireApiKey: provider.requireApiKey } : {}),
      });
      if (provider.apiKey?.trim()) await auth.setApiKey(saved.id, provider.apiKey);
      const configured: OpenAICompatibleProviderConfig = { kind: "openai-compatible", ...saved };
      customProviders.set(saved.id, configured);
      await refreshProvider(saved.id);
      write({
        v: BRIDGE_PROTOCOL_VERSION,
        id: command.id,
        type: "provider-configuration",
        providerConfiguration: configured,
      });
      return;
    }
    case "remove-provider-configuration": {
      const removed = await auth.removeCustomProvider(command.provider);
      if (removed) {
        customProviders.delete(command.provider);
        core.unregister(command.provider);
        await auth.logout(command.provider);
      }
      write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "unregistered", provider: command.provider, removed });
      return;
    }
    case "register-provider": {
      const provider = command.provider;
      if (provider.kind !== "openai-compatible") throw new Error(`Unsupported provider kind: ${String(provider.kind)}`);
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
    case "skill-validate":
      write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "skills", skills: await skills.validate(command.source) });
      return;
    case "skill-install": {
      const result = await skills.install(command.source);
      write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "skill-install-result", ...result });
      return;
    }
    case "skill-remove":
      write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "skill-removed", removed: await skills.remove(command.skill) });
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
      void runMcpCallTool(command);
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
      if (command.server) await mcp.disconnect(command.server);
      else await mcp.close();
      write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "done" });
      return;

    case "cancel":
      active.get(command.target)?.abort(new Error("Cancelled by bridge client"));
      write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "cancelled", target: command.target });
      return;
    case "shutdown":
      shuttingDown = true;
      for (const controller of active.values()) controller.abort(new Error("Bridge shutting down"));
      active.clear();
      denyPendingNativeApprovals();
      await mcp.close();
      write({ v: BRIDGE_PROTOCOL_VERSION, id: command.id, type: "done" });
      setTimeout(() => process.exit(0), 0);
      return;
  }
}

const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
input.on("line", (line) => {
  if (line.trim().length === 0) return;
  let raw: unknown;
  try { raw = JSON.parse(line); }
  catch (error) {
    process.stderr.write(`harness-call-core bridge: invalid JSON: ${String(error)}\n`);
    return;
  }
  if (raw && typeof raw === "object" && (raw as { type?: unknown }).type === "native_app_approval_decision") {
    const decision = raw as Partial<NativeAppApprovalDecision>;
    if (typeof decision.requestId !== "string" || (decision.decision !== "accept" && decision.decision !== "decline")) return;
    nativeApprovalWaiters.get(decision.requestId)?.(decision.decision === "accept");
    nativeApprovalWaiters.delete(decision.requestId);
    return;
  }
  if (shuttingDown) return;
  let command: BridgeCommand;
  try { command = validateCommand(raw); }
  catch (error) {
    const candidateId = raw && typeof raw === "object" && typeof (raw as { id?: unknown }).id === "string"
      ? String((raw as { id: string }).id) : "protocol";
    write(errorMessage(candidateId, error));
    return;
  }
  void handle(command).catch((error) => write(errorMessage(command.id, error)));
});

input.on("close", () => {
  for (const controller of active.values()) controller.abort(new Error("Bridge stdin closed"));
  active.clear();
  denyPendingNativeApprovals();
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
