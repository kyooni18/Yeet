import { spawn } from "node:child_process";
import { chmod, mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

import { parseSSE } from "./sse.js";
import type { FetchLike } from "./types.js";

export interface NativeAppApprovalRequest {
  requestId: string;
  server: string;
  tool: string;
  bundleId?: string;
  appName?: string;
  operation: string;
  message: string;
}

export const MCP_PROTOCOL_VERSION = "2026-07-28";
const LEGACY_MCP_PROTOCOL_VERSION = "2025-11-25";
const CLIENT_INFO = { name: "yeet", version: "0.1.0" } as const;
const CLIENT_CAPABILITIES = { elicitation: { form: {} } } as const;
const MCP_CONNECT_TIMEOUT_MS = 15_000;
const MCP_REQUEST_TIMEOUT_MS = 60_000;

export type McpTransportKind = "stdio" | "http";

export interface McpStdioServerConfiguration {
  name: string;
  transport: "stdio";
  command: string;
  args?: string[];
  env?: Record<string, string>;
  cwd?: string;
}

export interface McpHTTPServerConfiguration {
  name: string;
  transport: "http";
  url: string;
  headers?: Record<string, string>;
}

export type McpServerConfiguration = McpStdioServerConfiguration | McpHTTPServerConfiguration;

export type McpServerStatus = McpServerConfiguration & {
  connected: boolean;
  protocol?: string;
  era?: "modern" | "legacy";
};

export interface McpTool {
  server: string;
  name: string;
  qualifiedName: string;
  title?: string;
  description?: string;
  inputSchema: Record<string, unknown>;
  outputSchema?: Record<string, unknown>;
  annotations?: Record<string, unknown>;
}

export interface McpResource {
  server: string;
  uri: string;
  name: string;
  title?: string;
  description?: string;
  mimeType?: string;
}

export interface McpPromptArgument {
  name: string;
  description?: string;
  required?: boolean;
}

export interface McpPrompt {
  server: string;
  name: string;
  qualifiedName: string;
  title?: string;
  description?: string;
  arguments?: McpPromptArgument[];
}

export interface McpCallToolResult {
  content: unknown[];
  structuredContent?: unknown;
  isError?: boolean;
  /** Optional host-facing feedback supplied by an MCP tool implementation. */
  feedback?: unknown;
  _meta?: Record<string, unknown>;
}

export interface McpReadResourceResult {
  contents: unknown[];
  _meta?: Record<string, unknown>;
}

export interface McpGetPromptResult {
  description?: string;
  messages: unknown[];
  _meta?: Record<string, unknown>;
}

interface McpConfigFile {
  version: 1;
  servers: Record<string, McpServerConfiguration>;
}

interface JsonRpcErrorShape {
  code: number;
  message: string;
  data?: unknown;
}

interface JsonRpcResponse {
  jsonrpc: "2.0";
  id: string | number | null;
  result?: unknown;
  error?: JsonRpcErrorShape;
}

interface JsonRpcRequest {
  jsonrpc: "2.0";
  id: string | number;
  method: string;
  params?: unknown;
}

export type NativeAppApprovalHandler = (request: NativeAppApprovalRequest, signal?: AbortSignal) => Promise<boolean>;

export class McpError extends Error {
  readonly code: number | undefined;
  readonly data: unknown;
  readonly server: string | undefined;

  constructor(message: string, options: { code?: number; data?: unknown; server?: string } = {}) {
    super(message);
    this.name = "McpError";
    this.code = options.code;
    this.data = options.data;
    this.server = options.server;
  }
}

interface McpConnection {
  readonly configuration: McpServerConfiguration;
  readonly era: "modern" | "legacy" | undefined;
  readonly protocol: string | undefined;
  connect(signal?: AbortSignal): Promise<void>;
  request(method: string, params?: Record<string, unknown>, toolSchema?: Record<string, unknown>, signal?: AbortSignal): Promise<unknown>;
  close(): Promise<void>;
}

function abortReason(signal: AbortSignal): unknown {
  return signal.reason ?? new Error("MCP request cancelled");
}

async function withTimeout<T>(operation: (signal: AbortSignal) => Promise<T>, timeoutMs: number, label: string, parent?: AbortSignal): Promise<T> {
  if (parent?.aborted) throw abortReason(parent);
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(new McpError(`${label} timed out after ${timeoutMs}ms`)), timeoutMs);
  const onAbort = () => controller.abort(abortReason(parent!));
  parent?.addEventListener("abort", onAbort, { once: true });
  try { return await operation(controller.signal); }
  finally {
    clearTimeout(timer);
    parent?.removeEventListener("abort", onAbort);
  }
}

function asObject(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return value as Record<string, unknown>;
}

function asString(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function modernParams(params: Record<string, unknown> = {}): Record<string, unknown> {
  const existingMeta = asObject(params._meta);
  return {
    ...params,
    _meta: {
      "io.modelcontextprotocol/protocolVersion": MCP_PROTOCOL_VERSION,
      "io.modelcontextprotocol/clientInfo": CLIENT_INFO,
      "io.modelcontextprotocol/clientCapabilities": CLIENT_CAPABILITIES,
      ...existingMeta,
    },
  };
}

function jsonRpcError(error: JsonRpcErrorShape, server?: string): McpError {
  return new McpError(error.message, {
    code: error.code,
    ...(error.data !== undefined ? { data: error.data } : {}),
    ...(server ? { server } : {}),
  });
}

function validateServerName(name: string): void {
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(name)) {
    throw new Error(`MCP server name must use letters, digits, dot, underscore, or dash: ${name}`);
  }
}

function normalizeConfiguration(configuration: McpServerConfiguration): McpServerConfiguration {
  validateServerName(configuration.name);
  if (configuration.transport === "stdio") {
    if (!configuration.command.trim()) throw new Error("MCP stdio command is required");
    return {
      ...configuration,
      command: configuration.command.trim(),
      ...(configuration.args ? { args: [...configuration.args] } : {}),
      ...(configuration.env ? { env: { ...configuration.env } } : {}),
    };
  }
  const url = new URL(configuration.url);
  if (url.protocol !== "http:" && url.protocol !== "https:") throw new Error("MCP HTTP URL must use http or https");
  return { ...configuration, url: url.toString() };
}

async function atomicJsonWrite(path: string, value: unknown): Promise<void> {
  await mkdir(dirname(path), { recursive: true, mode: 0o700 });
  const tmp = `${path}.tmp-${String(process.pid)}-${Math.random().toString(16).slice(2)}`;
  await writeFile(tmp, `${JSON.stringify(value, null, 2)}\n`, { encoding: "utf8", mode: 0o600 });
  await chmod(tmp, 0o600);
  await rename(tmp, path);
  await chmod(path, 0o600);
}

async function readConfig(path: string): Promise<McpConfigFile> {
  try {
    const parsed = JSON.parse(await readFile(path, "utf8")) as Partial<McpConfigFile>;
    return { version: 1, servers: parsed.servers ?? {} };
  } catch (error) {
    if ((error as { code?: string }).code === "ENOENT") return { version: 1, servers: {} };
    throw error;
  }
}

class StdioMcpConnection implements McpConnection {
  readonly configuration: McpStdioServerConfiguration;
  #child: any;
  #buffer = "";
  #nextId = 1;
  #pending = new Map<number, { resolve: (value: unknown) => void; reject: (error: unknown) => void }>();
  readonly #approvalHandler: NativeAppApprovalHandler | undefined;
  #era: "modern" | "legacy" | undefined;
  #protocol: string | undefined;
  #approvalControllers = new Set<AbortController>();

  constructor(configuration: McpStdioServerConfiguration, approvalHandler?: NativeAppApprovalHandler) {
    this.configuration = configuration;
    this.#approvalHandler = approvalHandler;
  }

  get era(): "modern" | "legacy" | undefined { return this.#era; }
  get protocol(): string | undefined { return this.#protocol; }

  async connect(signal?: AbortSignal): Promise<void> {
    if (this.#child) return;
    if (signal?.aborted) throw abortReason(signal);
    this.#spawnChild();

    try {
      await this.#rawRequest("server/discover", {}, true, signal);
      this.#era = "modern";
      this.#protocol = MCP_PROTOCOL_VERSION;
    } catch (error) {
      if (signal?.aborted) {
        await this.close();
        throw abortReason(signal);
      }
      const code = error instanceof McpError ? error.code : undefined;
      if (code !== -32601 && code !== -32022 && code !== undefined) {
        await this.close();
        throw error;
      }
      // Some legacy stdio servers terminate the process when they receive an
      // unknown pre-initialize method instead of replying with MethodNotFound.
      // In that case the modern probe has already consumed the process, so a
      // legacy initialize must start on a fresh stdio connection.
      if (!this.#child) this.#spawnChild();
      const result = asObject(await this.#rawRequest("initialize", {
        protocolVersion: LEGACY_MCP_PROTOCOL_VERSION,
        capabilities: CLIENT_CAPABILITIES,
        clientInfo: CLIENT_INFO,
      }, false, signal));
      this.#era = "legacy";
      this.#protocol = asString(result.protocolVersion) ?? LEGACY_MCP_PROTOCOL_VERSION;
      this.#send({ jsonrpc: "2.0", method: "notifications/initialized", params: {} });
    }
  }

  async request(method: string, params: Record<string, unknown> = {}, _toolSchema?: Record<string, unknown>, signal?: AbortSignal): Promise<unknown> {
    await this.connect(signal);
    const cancelApprovals = () => {
      for (const controller of this.#approvalControllers) controller.abort(abortReason(signal!));
    };
    signal?.addEventListener("abort", cancelApprovals, { once: true });
    try { return await this.#rawRequest(method, params, this.#era === "modern", signal); }
    finally { signal?.removeEventListener("abort", cancelApprovals); }
  }

  async close(): Promise<void> {
    const child = this.#child;
    this.#child = undefined;
    this.#abortApprovals();
    if (!child) return;
    try { child.stdin.end(); } catch { /* ignore */ }
    try { child.kill("SIGTERM"); } catch { /* ignore */ }
    for (const pending of this.#pending.values()) pending.reject(new McpError(`MCP server ${this.configuration.name} disconnected`));
    this.#pending.clear();
  }

  #spawnChild(): void {
    const env = { ...process.env, ...(this.configuration.env ?? {}) };
    const child = spawn(this.configuration.command, this.configuration.args ?? [], {
      stdio: ["pipe", "pipe", "pipe"],
      env,
      ...(this.configuration.cwd ? { cwd: this.configuration.cwd } : {}),
    });
    this.#child = child;
    this.#buffer = "";
    child.stdout.on("data", (chunk: any) => this.#consume(String(chunk)));
    child.stderr.on("data", (_chunk: any) => undefined);
    child.on("error", (cause: Error) => {
      if (this.#child !== child) return;
      this.#child = undefined;
      this.#abortApprovals();
      const error = new McpError(`MCP server ${this.configuration.name} failed to start: ${cause.message}`, { server: this.configuration.name });
      for (const pending of this.#pending.values()) pending.reject(error);
      this.#pending.clear();
    });
    child.on("exit", (code: number | null, signal: string | null) => {
      if (this.#child !== child) return;
      this.#child = undefined;
      this.#abortApprovals();
      const error = new McpError(`MCP server ${this.configuration.name} exited (${String(code ?? signal ?? "unknown")})`, { server: this.configuration.name });
      for (const pending of this.#pending.values()) pending.reject(error);
      this.#pending.clear();
    });
  }

  #send(value: unknown): void {
    if (!this.#child?.stdin?.writable) throw new McpError(`MCP server ${this.configuration.name} is not writable`);
    this.#child.stdin.write(`${JSON.stringify(value)}\n`);
  }

  #abortApprovals(): void {
    for (const controller of this.#approvalControllers) controller.abort(new McpError("MCP approval cancelled"));
    this.#approvalControllers.clear();
  }

  #rawRequest(method: string, params: Record<string, unknown>, modern: boolean, signal?: AbortSignal): Promise<unknown> {
    if (signal?.aborted) return Promise.reject(abortReason(signal));
    const id = this.#nextId++;
    const body = { jsonrpc: "2.0", id, method, params: modern ? modernParams(params) : params };
    return new Promise((resolve, reject) => {
      const cleanup = () => signal?.removeEventListener("abort", onAbort);
      const onAbort = () => {
        if (!this.#pending.delete(id)) return;
        cleanup();
        try {
          this.#send({
            jsonrpc: "2.0",
            method: "notifications/cancelled",
            params: modern
              ? modernParams({ requestId: id, reason: "Cancelled by Yeet" })
              : { requestId: id, reason: "Cancelled by Yeet" },
          });
        } catch { /* best effort */ }
        reject(signal ? abortReason(signal) : new Error("MCP request cancelled"));
      };
      this.#pending.set(id, {
        resolve: (value) => { cleanup(); resolve(value); },
        reject: (error) => { cleanup(); reject(error); },
      });
      signal?.addEventListener("abort", onAbort, { once: true });
      try { this.#send(body); }
      catch (error) { this.#pending.delete(id); cleanup(); reject(error); }
    });
  }

  #consume(chunk: string): void {
    this.#buffer += chunk.replace(/\r\n/g, "\n");
    while (true) {
      const newline = this.#buffer.indexOf("\n");
      if (newline < 0) break;
      const line = this.#buffer.slice(0, newline).trim();
      this.#buffer = this.#buffer.slice(newline + 1);
      if (!line) continue;
      let message: JsonRpcResponse | JsonRpcRequest;
      try { message = JSON.parse(line) as JsonRpcResponse | JsonRpcRequest; }
      catch { continue; }
      if (typeof (message as JsonRpcRequest).method === "string" && message.id !== null) {
        void this.#handleServerRequest(message as JsonRpcRequest);
        continue;
      }
      const response = message as JsonRpcResponse;
      if (message.id === null || typeof message.id !== "number") continue;
      const pending = this.#pending.get(message.id);
      if (!pending) continue;
      this.#pending.delete(message.id);
      if (response.error) pending.reject(jsonRpcError(response.error, this.configuration.name));
      else pending.resolve(response.result);
    }
  }

  async #handleServerRequest(message: JsonRpcRequest): Promise<void> {
    if (message.method !== "elicitation/create") {
      this.#respond(message.id, { error: { code: -32601, message: `Unsupported MCP client request: ${message.method}` } });
      return;
    }
    const params = asObject(message.params);
    const meta = asObject(params.meta);
    const toolParams = asObject(meta.tool_params);
    const rawApp = toolParams.app;
    const appString = typeof rawApp === "string" ? rawApp.trim() : undefined;
    const app = appString !== undefined
      ? (/^[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)+$/.test(appString) ? { bundleId: appString } : { name: appString })
      : asObject(rawApp);
    const bundleId = asString(app.bundleId) ?? asString(app.bundle_id)
      ?? asString(toolParams.bundleId) ?? asString(toolParams.bundle_id);
    const appName = asString(app.name) ?? asString(app.appName) ?? asString(toolParams.appName) ?? asString(toolParams.app_name);
    const approvalKind = asString(meta.codex_approval_kind);
    const mode = asString(params.mode);
    if (approvalKind !== "mcp_tool_call" || (mode !== undefined && mode !== "form") || (!bundleId && !appName)) {
      this.#respond(message.id, { result: { action: "decline" } });
      return;
    }
    const tool = asString(toolParams.tool) ?? asString(toolParams.name) ?? asString(meta.tool) ?? "unknown";
    const operation = asString(toolParams.operation) ?? asString(meta.operation) ?? "MCP tool call";
    const elicitationMessage = asString(params.message) ?? asString(params.reason) ?? operation;
    let approved = false;
    const approvalController = new AbortController();
    this.#approvalControllers.add(approvalController);
    try {
      approved = this.#approvalHandler
        ? await this.#approvalHandler({
          requestId: String(message.id),
          server: this.configuration.name,
          tool,
          ...(bundleId ? { bundleId } : {}),
          ...(appName ? { appName } : {}),
          operation,
          message: elicitationMessage,
        }, approvalController.signal)
        : false;
    } catch {
      approved = false;
    } finally {
      this.#approvalControllers.delete(approvalController);
    }
    if (!approved) {
      this.#respond(message.id, { result: { action: "decline" } });
      return;
    }
    // Approval is deliberately scoped to this active session. Do not emit a
    // persistent/global approval marker understood by other hosts.
    this.#respond(message.id, { result: {
      action: "accept",
      content: { source: "yeet-computer-use-approval", scope: "session", ...(bundleId ? { bundleId } : {}), ...(appName ? { appName } : {}) },
      scope: "session",
    } });
  }

  #respond(id: string | number, response: Record<string, unknown>): void {
    try { this.#send({ jsonrpc: "2.0", id, ...response }); } catch { /* connection closed */ }
  }
}

function encodeHeaderValue(value: string): string {
  const visibleAscii = /^[\x20-\x7E]+$/.test(value);
  const trimmed = value.trim() === value;
  const sentinel = value.startsWith("=?base64?") && value.endsWith("?=");
  if (visibleAscii && trimmed && !sentinel) return value;
  return `=?base64?${Buffer.from(value, "utf8").toString("base64")}?=`;
}

interface HeaderBinding { path: string[]; name: string; type: "string" | "integer" | "boolean" }

function collectHeaderBindings(schema: Record<string, unknown>, path: string[] = []): HeaderBinding[] {
  const bindings: HeaderBinding[] = [];
  const properties = asObject(schema.properties);
  for (const [propertyName, rawProperty] of Object.entries(properties)) {
    const property = asObject(rawProperty);
    const nextPath = [...path, propertyName];
    const annotation = asString(property["x-mcp-header"]);
    const type = asString(property.type);
    if (annotation !== undefined) {
      if (!annotation || !/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(annotation)) {
        throw new McpError(`Invalid x-mcp-header annotation: ${annotation ?? ""}`);
      }
      if (type !== "string" && type !== "integer" && type !== "boolean") {
        throw new McpError(`x-mcp-header ${annotation} must annotate string, integer, or boolean`);
      }
      bindings.push({ path: nextPath, name: annotation, type });
    }
    if (type === "object" && property.properties) bindings.push(...collectHeaderBindings(property, nextPath));
  }
  const seen = new Set<string>();
  for (const binding of bindings) {
    const key = binding.name.toLowerCase();
    if (seen.has(key)) throw new McpError(`Duplicate x-mcp-header annotation: ${binding.name}`);
    seen.add(key);
  }
  return bindings;
}

function valueAtPath(value: Record<string, unknown>, path: string[]): unknown {
  let current: unknown = value;
  for (const segment of path) current = asObject(current)[segment];
  return current;
}

function toolParameterHeaders(schema: Record<string, unknown>, args: Record<string, unknown>): Record<string, string> {
  const headers: Record<string, string> = {};
  for (const binding of collectHeaderBindings(schema)) {
    const value = valueAtPath(args, binding.path);
    if (value === undefined) continue;
    if (binding.type === "string" && typeof value !== "string") continue;
    if (binding.type === "integer" && (!Number.isSafeInteger(value) || typeof value !== "number")) continue;
    if (binding.type === "boolean" && typeof value !== "boolean") continue;
    headers[`Mcp-Param-${binding.name}`] = encodeHeaderValue(String(value));
  }
  return headers;
}

class HTTPMcpConnection implements McpConnection {
  readonly configuration: McpHTTPServerConfiguration;
  readonly #fetch: FetchLike;
  #nextId = 1;

  constructor(configuration: McpHTTPServerConfiguration, fetchImpl: FetchLike) {
    this.configuration = configuration;
    this.#fetch = fetchImpl;
  }

  get era(): "modern" { return "modern"; }
  get protocol(): string { return MCP_PROTOCOL_VERSION; }
  async connect(signal?: AbortSignal): Promise<void> {
    if (signal?.aborted) throw abortReason(signal);
  }
  async close(): Promise<void> { /* no persistent session */ }

  async request(method: string, params: Record<string, unknown> = {}, toolSchema?: Record<string, unknown>, signal?: AbortSignal): Promise<unknown> {
    if (signal?.aborted) throw abortReason(signal);
    const id = this.#nextId++;
    const finalParams = modernParams(params);
    const headers: Record<string, string> = {
      "content-type": "application/json",
      accept: "application/json, text/event-stream",
      "MCP-Protocol-Version": MCP_PROTOCOL_VERSION,
      "Mcp-Method": method,
      ...(this.configuration.headers ?? {}),
    };
    const name = method === "resources/read" ? asString(params.uri) : asString(params.name);
    if (name && (method === "tools/call" || method === "resources/read" || method === "prompts/get")) {
      headers["Mcp-Name"] = encodeHeaderValue(name);
    }
    if (method === "tools/call" && toolSchema) {
      Object.assign(headers, toolParameterHeaders(toolSchema, asObject(params.arguments)));
    }

    const response = await this.#fetch(this.configuration.url, {
      method: "POST",
      headers,
      body: JSON.stringify({ jsonrpc: "2.0", id, method, params: finalParams }),
      ...(signal ? { signal } : {}),
    });
    const contentType = response.headers.get("content-type") ?? "";
    if (!response.ok && !contentType.includes("application/json")) {
      throw new McpError(`MCP HTTP ${response.status}: ${await response.text()}`, { server: this.configuration.name });
    }

    let message: JsonRpcResponse | undefined;
    if (contentType.includes("text/event-stream")) {
      for await (const event of parseSSE(response)) {
        if (!event.data) continue;
        const candidate = JSON.parse(event.data) as JsonRpcResponse;
        if (candidate.id === id) message = candidate;
      }
    } else {
      message = await response.json() as JsonRpcResponse;
    }
    if (!message) throw new McpError("MCP HTTP response did not include the final JSON-RPC response", { server: this.configuration.name });
    if (message.error) throw jsonRpcError(message.error, this.configuration.name);
    return message.result;
  }
}

export interface McpManagerOptions {
  configDir?: string;
  fetch?: FetchLike;
  nativeAppApproval?: NativeAppApprovalHandler;
}

export class McpManager {
  readonly configDir: string;
  readonly configPath: string;
  readonly #fetch: FetchLike;
  readonly #nativeAppApproval: NativeAppApprovalHandler | undefined;
  readonly #connections = new Map<string, McpConnection>();

  constructor(options: McpManagerOptions = {}) {
    this.configDir = options.configDir ?? process.env.YEET_CONFIG_DIR ?? join(homedir(), ".yeet");
    this.configPath = join(this.configDir, "mcp.json");
    this.#fetch = options.fetch ?? fetch;
    this.#nativeAppApproval = options.nativeAppApproval;
  }

  async ensure(): Promise<void> {
    await mkdir(this.configDir, { recursive: true, mode: 0o700 });
    await chmod(this.configDir, 0o700);
    try { await chmod(this.configPath, 0o600); }
    catch (error) {
      if ((error as { code?: string }).code !== "ENOENT") throw error;
      await atomicJsonWrite(this.configPath, { version: 1, servers: {} } satisfies McpConfigFile);
    }
  }

  async setServer(configuration: McpServerConfiguration): Promise<McpServerConfiguration> {
    await this.ensure();
    const normalized = normalizeConfiguration(configuration);
    await this.disconnect(normalized.name);
    const config = await readConfig(this.configPath);
    config.servers[normalized.name] = normalized;
    await atomicJsonWrite(this.configPath, config);
    return normalized;
  }

  async removeServer(name: string): Promise<boolean> {
    await this.ensure();
    await this.disconnect(name);
    const config = await readConfig(this.configPath);
    const existed = name in config.servers;
    delete config.servers[name];
    await atomicJsonWrite(this.configPath, config);
    return existed;
  }

  async listServers(): Promise<McpServerStatus[]> {
    await this.ensure();
    const config = await readConfig(this.configPath);
    return Object.values(config.servers)
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((server) => {
        const connection = this.#connections.get(server.name);
        return {
          ...server,
          connected: Boolean(connection),
          ...(connection?.protocol ? { protocol: connection.protocol } : {}),
          ...(connection?.era ? { era: connection.era } : {}),
        };
      });
  }

  async listTools(server?: string, signal?: AbortSignal): Promise<McpTool[]> {
    const servers = await this.#targetServers(server);
    const output: McpTool[] = [];
    for (const configuration of servers) {
      const connection = await this.#connection(configuration.name, signal);
      let cursor: string | undefined;
      do {
        const result = asObject(await withTimeout(
          (requestSignal) => connection.request("tools/list", cursor ? { cursor } : {}, undefined, requestSignal),
          MCP_REQUEST_TIMEOUT_MS,
          `MCP tools/list for ${configuration.name}`,
          signal,
        ));
        const tools = Array.isArray(result.tools) ? result.tools : [];
        for (const raw of tools) {
          const tool = asObject(raw);
          const name = asString(tool.name);
          if (!name) continue;
          const inputSchema = asObject(tool.inputSchema);
          if (configuration.transport === "http") {
            try { collectHeaderBindings(inputSchema); } catch { continue; }
          }
          output.push({
            server: configuration.name,
            name,
            qualifiedName: `${configuration.name}/${name}`,
            inputSchema,
            ...(asString(tool.title) ? { title: asString(tool.title)! } : {}),
            ...(asString(tool.description) ? { description: asString(tool.description)! } : {}),
            ...(tool.outputSchema ? { outputSchema: asObject(tool.outputSchema) } : {}),
            ...(tool.annotations ? { annotations: asObject(tool.annotations) } : {}),
          });
        }
        cursor = asString(result.nextCursor);
      } while (cursor);
    }
    return output;
  }

  async callTool(server: string, name: string, args: Record<string, unknown> = {}, signal?: AbortSignal): Promise<McpCallToolResult> {
    const connection = await this.#connection(server, signal);
    const tool = (await this.listTools(server, signal)).find((candidate) => candidate.name === name);
    if (!tool) throw new McpError(`Unknown MCP tool ${server}/${name}`, { server });
    const result = asObject(await connection.request("tools/call", { name, arguments: args }, tool.inputSchema, signal));
    return {
      content: Array.isArray(result.content) ? result.content : [],
      ...(result.structuredContent !== undefined ? { structuredContent: result.structuredContent } : {}),
      ...(typeof result.isError === "boolean" ? { isError: result.isError } : {}),
      ...(result.feedback !== undefined ? { feedback: result.feedback } : {}),
      ...(result._meta ? { _meta: asObject(result._meta) } : {}),
    };
  }

  async callQualifiedTool(qualifiedName: string, args: Record<string, unknown> = {}, signal?: AbortSignal): Promise<McpCallToolResult> {
    const slash = qualifiedName.indexOf("/");
    if (slash <= 0 || slash === qualifiedName.length - 1) throw new Error(`MCP tool must use server/tool form: ${qualifiedName}`);
    return this.callTool(qualifiedName.slice(0, slash), qualifiedName.slice(slash + 1), args, signal);
  }

  async listResources(server?: string): Promise<McpResource[]> {
    const servers = await this.#targetServers(server);
    const output: McpResource[] = [];
    for (const configuration of servers) {
      const connection = await this.#connection(configuration.name);
      let cursor: string | undefined;
      do {
        const result = asObject(await connection.request("resources/list", cursor ? { cursor } : {}));
        for (const raw of Array.isArray(result.resources) ? result.resources : []) {
          const resource = asObject(raw);
          const uri = asString(resource.uri);
          const name = asString(resource.name);
          if (!uri || !name) continue;
          output.push({
            server: configuration.name,
            uri,
            name,
            ...(asString(resource.title) ? { title: asString(resource.title)! } : {}),
            ...(asString(resource.description) ? { description: asString(resource.description)! } : {}),
            ...(asString(resource.mimeType) ? { mimeType: asString(resource.mimeType)! } : {}),
          });
        }
        cursor = asString(result.nextCursor);
      } while (cursor);
    }
    return output;
  }

  async readResource(server: string, uri: string): Promise<McpReadResourceResult> {
    const result = asObject(await (await this.#connection(server)).request("resources/read", { uri }));
    return {
      contents: Array.isArray(result.contents) ? result.contents : [],
      ...(result._meta ? { _meta: asObject(result._meta) } : {}),
    };
  }

  async listPrompts(server?: string): Promise<McpPrompt[]> {
    const servers = await this.#targetServers(server);
    const output: McpPrompt[] = [];
    for (const configuration of servers) {
      const connection = await this.#connection(configuration.name);
      let cursor: string | undefined;
      do {
        const result = asObject(await connection.request("prompts/list", cursor ? { cursor } : {}));
        for (const raw of Array.isArray(result.prompts) ? result.prompts : []) {
          const prompt = asObject(raw);
          const name = asString(prompt.name);
          if (!name) continue;
          const argumentsValue = Array.isArray(prompt.arguments)
            ? prompt.arguments.map((value) => {
                const argument = asObject(value);
                return {
                  name: asString(argument.name) ?? "",
                  ...(asString(argument.description) ? { description: asString(argument.description)! } : {}),
                  ...(typeof argument.required === "boolean" ? { required: argument.required } : {}),
                };
              }).filter((argument) => argument.name)
            : undefined;
          output.push({
            server: configuration.name,
            name,
            qualifiedName: `${configuration.name}/${name}`,
            ...(asString(prompt.title) ? { title: asString(prompt.title)! } : {}),
            ...(asString(prompt.description) ? { description: asString(prompt.description)! } : {}),
            ...(argumentsValue ? { arguments: argumentsValue } : {}),
          });
        }
        cursor = asString(result.nextCursor);
      } while (cursor);
    }
    return output;
  }

  async getPrompt(server: string, name: string, args: Record<string, string> = {}): Promise<McpGetPromptResult> {
    const result = asObject(await (await this.#connection(server)).request("prompts/get", { name, arguments: args }));
    return {
      messages: Array.isArray(result.messages) ? result.messages : [],
      ...(asString(result.description) ? { description: asString(result.description)! } : {}),
      ...(result._meta ? { _meta: asObject(result._meta) } : {}),
    };
  }

  async disconnect(name: string): Promise<void> {
    const connection = this.#connections.get(name);
    this.#connections.delete(name);
    await connection?.close();
  }

  async close(): Promise<void> {
    const connections = [...this.#connections.values()];
    this.#connections.clear();
    await Promise.all(connections.map((connection) => connection.close()));
  }

  async #targetServers(server?: string): Promise<McpServerConfiguration[]> {
    const config = await readConfig(this.configPath);
    if (server) {
      const found = config.servers[server];
      if (!found) throw new McpError(`Unknown MCP server: ${server}`, { server });
      return [found];
    }
    return Object.values(config.servers).sort((a, b) => a.name.localeCompare(b.name));
  }

  async #connection(name: string, signal?: AbortSignal): Promise<McpConnection> {
    const existing = this.#connections.get(name);
    if (existing) return existing;
    if (signal?.aborted) throw abortReason(signal);
    await this.ensure();
    const config = await readConfig(this.configPath);
    const configuration = config.servers[name];
    if (!configuration) throw new McpError(`Unknown MCP server: ${name}`, { server: name });
    const connection: McpConnection = configuration.transport === "stdio"
      ? new StdioMcpConnection(configuration, this.#nativeAppApproval)
      : new HTTPMcpConnection(configuration, this.#fetch);
    await withTimeout(
      (connectSignal) => connection.connect(connectSignal),
      MCP_CONNECT_TIMEOUT_MS,
      `MCP connection to ${name}`,
      signal,
    );
    this.#connections.set(name, connection);
    return connection;
  }
}
