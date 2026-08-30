import { spawn } from "node:child_process";
import { chmod, mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

import { parseSSE } from "./sse.js";
import type { FetchLike } from "./types.js";

export const MCP_PROTOCOL_VERSION = "2026-07-28";
const LEGACY_MCP_PROTOCOL_VERSION = "2025-11-25";
const CLIENT_INFO = { name: "yeet", version: "0.1.0" } as const;
const CLIENT_CAPABILITIES = {} as const;

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
  connect(): Promise<void>;
  request(method: string, params?: Record<string, unknown>, toolSchema?: Record<string, unknown>): Promise<unknown>;
  close(): Promise<void>;
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
  #era: "modern" | "legacy" | undefined;
  #protocol: string | undefined;

  constructor(configuration: McpStdioServerConfiguration) {
    this.configuration = configuration;
  }

  get era(): "modern" | "legacy" | undefined { return this.#era; }
  get protocol(): string | undefined { return this.#protocol; }

  async connect(): Promise<void> {
    if (this.#child) return;
    const env = { ...process.env, ...(this.configuration.env ?? {}) };
    this.#child = spawn(this.configuration.command, this.configuration.args ?? [], {
      stdio: ["pipe", "pipe", "pipe"],
      env,
      ...(this.configuration.cwd ? { cwd: this.configuration.cwd } : {}),
    });
    this.#child.stdout.on("data", (chunk: any) => this.#consume(String(chunk)));
    this.#child.stderr.on("data", (_chunk: any) => undefined);
    this.#child.on("exit", (code: number | null, signal: string | null) => {
      const error = new McpError(`MCP server ${this.configuration.name} exited (${String(code ?? signal ?? "unknown")})`, { server: this.configuration.name });
      for (const pending of this.#pending.values()) pending.reject(error);
      this.#pending.clear();
      this.#child = undefined;
    });

    try {
      await this.#rawRequest("server/discover", {}, true);
      this.#era = "modern";
      this.#protocol = MCP_PROTOCOL_VERSION;
    } catch (error) {
      const code = error instanceof McpError ? error.code : undefined;
      if (code !== -32601 && code !== -32022 && code !== undefined) {
        await this.close();
        throw error;
      }
      const result = asObject(await this.#rawRequest("initialize", {
        protocolVersion: LEGACY_MCP_PROTOCOL_VERSION,
        capabilities: {},
        clientInfo: CLIENT_INFO,
      }, false));
      this.#era = "legacy";
      this.#protocol = asString(result.protocolVersion) ?? LEGACY_MCP_PROTOCOL_VERSION;
      this.#send({ jsonrpc: "2.0", method: "notifications/initialized", params: {} });
    }
  }

  async request(method: string, params: Record<string, unknown> = {}): Promise<unknown> {
    await this.connect();
    return this.#rawRequest(method, params, this.#era === "modern");
  }

  async close(): Promise<void> {
    const child = this.#child;
    this.#child = undefined;
    if (!child) return;
    try { child.stdin.end(); } catch { /* ignore */ }
    try { child.kill("SIGTERM"); } catch { /* ignore */ }
    for (const pending of this.#pending.values()) pending.reject(new McpError(`MCP server ${this.configuration.name} disconnected`));
    this.#pending.clear();
  }

  #send(value: unknown): void {
    if (!this.#child?.stdin?.writable) throw new McpError(`MCP server ${this.configuration.name} is not writable`);
    this.#child.stdin.write(`${JSON.stringify(value)}\n`);
  }

  #rawRequest(method: string, params: Record<string, unknown>, modern: boolean): Promise<unknown> {
    const id = this.#nextId++;
    const body = { jsonrpc: "2.0", id, method, params: modern ? modernParams(params) : params };
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      try { this.#send(body); }
      catch (error) { this.#pending.delete(id); reject(error); }
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
      let message: JsonRpcResponse;
      try { message = JSON.parse(line) as JsonRpcResponse; }
      catch { continue; }
      if (message.id === null || typeof message.id !== "number") continue;
      const pending = this.#pending.get(message.id);
      if (!pending) continue;
      this.#pending.delete(message.id);
      if (message.error) pending.reject(jsonRpcError(message.error, this.configuration.name));
      else pending.resolve(message.result);
    }
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
  async connect(): Promise<void> { /* stateless modern transport */ }
  async close(): Promise<void> { /* no persistent session */ }

  async request(method: string, params: Record<string, unknown> = {}, toolSchema?: Record<string, unknown>): Promise<unknown> {
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
}

export class McpManager {
  readonly configDir: string;
  readonly configPath: string;
  readonly #fetch: FetchLike;
  readonly #connections = new Map<string, McpConnection>();

  constructor(options: McpManagerOptions = {}) {
    this.configDir = options.configDir ?? process.env.YEET_CONFIG_DIR ?? join(homedir(), ".yeet");
    this.configPath = join(this.configDir, "mcp.json");
    this.#fetch = options.fetch ?? fetch;
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

  async listTools(server?: string): Promise<McpTool[]> {
    const servers = await this.#targetServers(server);
    const output: McpTool[] = [];
    for (const configuration of servers) {
      const connection = await this.#connection(configuration.name);
      let cursor: string | undefined;
      do {
        const result = asObject(await connection.request("tools/list", cursor ? { cursor } : {}));
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

  async callTool(server: string, name: string, args: Record<string, unknown> = {}): Promise<McpCallToolResult> {
    const connection = await this.#connection(server);
    const tool = (await this.listTools(server)).find((candidate) => candidate.name === name);
    if (!tool) throw new McpError(`Unknown MCP tool ${server}/${name}`, { server });
    const result = asObject(await connection.request("tools/call", { name, arguments: args }, tool.inputSchema));
    return {
      content: Array.isArray(result.content) ? result.content : [],
      ...(result.structuredContent !== undefined ? { structuredContent: result.structuredContent } : {}),
      ...(typeof result.isError === "boolean" ? { isError: result.isError } : {}),
      ...(result._meta ? { _meta: asObject(result._meta) } : {}),
    };
  }

  async callQualifiedTool(qualifiedName: string, args: Record<string, unknown> = {}): Promise<McpCallToolResult> {
    const slash = qualifiedName.indexOf("/");
    if (slash <= 0 || slash === qualifiedName.length - 1) throw new Error(`MCP tool must use server/tool form: ${qualifiedName}`);
    return this.callTool(qualifiedName.slice(0, slash), qualifiedName.slice(slash + 1), args);
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

  async #connection(name: string): Promise<McpConnection> {
    const existing = this.#connections.get(name);
    if (existing) return existing;
    await this.ensure();
    const config = await readConfig(this.configPath);
    const configuration = config.servers[name];
    if (!configuration) throw new McpError(`Unknown MCP server: ${name}`, { server: name });
    const connection: McpConnection = configuration.transport === "stdio"
      ? new StdioMcpConnection(configuration)
      : new HTTPMcpConnection(configuration, this.#fetch);
    await connection.connect();
    this.#connections.set(name, connection);
    return connection;
  }
}
