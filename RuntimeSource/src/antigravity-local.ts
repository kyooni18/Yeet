import { execFile } from "node:child_process";
import { request as httpsRequest } from "node:https";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const SERVICE = "exa.language_server_pb.LanguageServerService";

export interface AntigravityLocalEndpoint {
  baseUrl: string;
  csrfToken: string;
}

export interface AntigravityAuthResult {
  hasValidAuth?: boolean;
  uiMessage?: string;
  projectId?: string;
  grantedScopes?: string[];
}

export interface AntigravityModelRecord {
  displayName?: string;
  maxTokens?: number;
  maxOutputTokens?: number;
  quotaInfo?: {
    remainingFraction?: number;
    resetTime?: string;
  };
  model?: string;
  modelProvider?: string;
  isInternal?: boolean;
  tagDescription?: string;
  [key: string]: unknown;
}

export interface AntigravityAvailableModelsResponse {
  response?: {
    models?: Record<string, AntigravityModelRecord>;
    [key: string]: unknown;
  };
  models?: Record<string, AntigravityModelRecord>;
  [key: string]: unknown;
}

export interface AntigravityLocalClientOptions {
  endpoint?: AntigravityLocalEndpoint;
  discover?: () => Promise<AntigravityLocalEndpoint | undefined>;
  request?: (
    endpoint: AntigravityLocalEndpoint,
    method: string,
    body: Record<string, unknown>,
    timeoutMs: number,
    signal?: AbortSignal,
  ) => Promise<Record<string, unknown>>;
  openApp?: () => Promise<void>;
}

function loopbackBaseUrl(value: string): string {
  const url = new URL(value);
  if (url.protocol !== "https:" || !["127.0.0.1", "localhost", "::1"].includes(url.hostname)) {
    throw new Error("Antigravity local endpoint must use HTTPS on loopback");
  }
  return url.toString().replace(/\/$/, "");
}

function asObject(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

async function localHttpsJson(
  endpoint: AntigravityLocalEndpoint,
  method: string,
  body: Record<string, unknown>,
  timeoutMs: number,
  signal?: AbortSignal,
): Promise<Record<string, unknown>> {
  const target = new URL(`${loopbackBaseUrl(endpoint.baseUrl)}/${SERVICE}/${encodeURIComponent(method)}`);
  return await new Promise<Record<string, unknown>>((resolve, reject) => {
    let settled = false;
    const finish = (callback: () => void) => {
      if (settled) return;
      settled = true;
      signal?.removeEventListener("abort", onAbort);
      callback();
    };
    const request = httpsRequest({
      protocol: target.protocol,
      hostname: target.hostname,
      port: target.port,
      path: target.pathname,
      method: "POST",
      rejectUnauthorized: false,
      headers: {
        "content-type": "application/json",
        "connect-protocol-version": "1",
        "x-codeium-csrf-token": endpoint.csrfToken,
      },
    }, (response) => {
      const chunks: Buffer[] = [];
      response.on("data", (chunk) => chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)));
      response.on("error", (error) => finish(() => reject(error)));
      response.on("end", () => {
        const text = Buffer.concat(chunks).toString("utf8");
        if ((response.statusCode ?? 500) < 200 || (response.statusCode ?? 500) >= 300) {
          finish(() => reject(new Error(`Antigravity local RPC ${method} failed with HTTP ${response.statusCode ?? "unknown"}: ${text.slice(0, 500)}`)));
          return;
        }
        try {
          finish(() => resolve(text ? asObject(JSON.parse(text)) : {}));
        } catch (error) {
          finish(() => reject(new Error(`Antigravity local RPC ${method} returned invalid JSON`, { cause: error })));
        }
      });
    });
    const onAbort = () => request.destroy(signal?.reason instanceof Error ? signal.reason : new Error("Antigravity request aborted"));
    signal?.addEventListener("abort", onAbort, { once: true });
    request.setTimeout(timeoutMs, () => request.destroy(new Error(`Antigravity local RPC ${method} timed out`)));
    request.on("error", (error) => finish(() => reject(error)));
    request.end(JSON.stringify(body));
  });
}

async function systemOpenAntigravity(): Promise<void> {
  if (process.platform !== "darwin") {
    throw new Error("Automatic Antigravity app launch is currently supported on macOS only");
  }
  await execFileAsync("/usr/bin/open", ["-a", "Antigravity"]);
}

async function processEndpointCandidates(): Promise<AntigravityLocalEndpoint[]> {
  const configuredUrl = process.env.YEET_ANTIGRAVITY_BASE_URL?.trim();
  const configuredCsrf = process.env.YEET_ANTIGRAVITY_CSRF_TOKEN?.trim();
  if (configuredUrl && configuredCsrf) {
    return [{ baseUrl: loopbackBaseUrl(configuredUrl), csrfToken: configuredCsrf }];
  }
  if (process.platform !== "darwin") return [];

  const { stdout: processes } = await execFileAsync("/bin/ps", ["axww", "-o", "pid=,command="], {
    maxBuffer: 8 * 1024 * 1024,
  });
  const matches: Array<{ pid: number; csrfToken: string }> = [];
  for (const line of processes.split("\n")) {
    const parsed = line.match(/^\s*(\d+)\s+(.*)$/);
    if (!parsed) continue;
    const command = parsed[2] ?? "";
    if (!command.includes("language_server") || !command.includes("--standalone")) continue;
    if (!command.includes("Antigravity.app") && !/--override_ide_name\s+antigravity(?:\s|$)/i.test(command)) continue;
    const csrf = command.match(/--csrf_token\s+([^\s]+)/)?.[1];
    if (!csrf) continue;
    matches.push({ pid: Number(parsed[1]), csrfToken: csrf });
  }

  const endpoints: AntigravityLocalEndpoint[] = [];
  for (const match of matches) {
    let listeners = "";
    try {
      ({ stdout: listeners } = await execFileAsync("/usr/sbin/lsof", [
        "-nP",
        "-a",
        "-p",
        String(match.pid),
        "-iTCP",
        "-sTCP:LISTEN",
      ], { maxBuffer: 2 * 1024 * 1024 }));
    } catch {
      continue;
    }
    const ports = new Set<number>();
    for (const found of listeners.matchAll(/(?:127\.0\.0\.1|\[::1\]):(\d+)/g)) {
      const port = Number(found[1]);
      if (Number.isInteger(port) && port > 0 && port <= 65535) ports.add(port);
    }
    for (const port of ports) {
      endpoints.push({ baseUrl: `https://127.0.0.1:${port}`, csrfToken: match.csrfToken });
    }
  }
  return endpoints;
}

export class AntigravityLocalClient {
  readonly #fixedEndpoint: AntigravityLocalEndpoint | undefined;
  readonly #discoverOverride: (() => Promise<AntigravityLocalEndpoint | undefined>) | undefined;
  readonly #request: NonNullable<AntigravityLocalClientOptions["request"]>;
  readonly #openApp: () => Promise<void>;
  #cachedEndpoint: { value: AntigravityLocalEndpoint; expiresAt: number } | undefined;

  constructor(options: AntigravityLocalClientOptions = {}) {
    this.#fixedEndpoint = options.endpoint
      ? { ...options.endpoint, baseUrl: loopbackBaseUrl(options.endpoint.baseUrl) }
      : undefined;
    this.#discoverOverride = options.discover;
    this.#request = options.request ?? localHttpsJson;
    this.#openApp = options.openApp ?? systemOpenAntigravity;
  }

  async #probe(endpoint: AntigravityLocalEndpoint): Promise<boolean> {
    try {
      await this.#request(endpoint, "GetAuthStatus", {}, 1_500);
      return true;
    } catch {
      return false;
    }
  }

  async #discover(): Promise<AntigravityLocalEndpoint | undefined> {
    if (this.#fixedEndpoint) return this.#fixedEndpoint;
    if (this.#discoverOverride) return await this.#discoverOverride();
    const candidates = await processEndpointCandidates();
    for (const endpoint of candidates) {
      if (await this.#probe(endpoint)) return endpoint;
    }
    return undefined;
  }

  async #endpoint(launch: boolean): Promise<AntigravityLocalEndpoint> {
    if (this.#cachedEndpoint && this.#cachedEndpoint.expiresAt > Date.now()) return this.#cachedEndpoint.value;
    let endpoint = await this.#discover();
    if (!endpoint && launch) {
      await this.#openApp();
      for (let attempt = 0; attempt < 30 && !endpoint; attempt += 1) {
        await new Promise((resolve) => setTimeout(resolve, 500));
        endpoint = await this.#discover();
      }
    }
    if (!endpoint) {
      throw new Error("Google Antigravity is not running. Open Antigravity or use Yeet's Antigravity sign-in action.");
    }
    this.#cachedEndpoint = { value: endpoint, expiresAt: Date.now() + 5_000 };
    return endpoint;
  }

  async rpc(
    method: string,
    body: Record<string, unknown> = {},
    options: { launch?: boolean; timeoutMs?: number; signal?: AbortSignal } = {},
  ): Promise<Record<string, unknown>> {
    const endpoint = await this.#endpoint(options.launch ?? false);
    try {
      return await this.#request(endpoint, method, body, options.timeoutMs ?? 15_000, options.signal);
    } catch (error) {
      this.#cachedEndpoint = undefined;
      throw error;
    }
  }

  async getAuthStatus(options: { launch?: boolean } = {}): Promise<AntigravityAuthResult> {
    const response = await this.rpc("GetAuthStatus", {}, { launch: options.launch ?? false, timeoutMs: 3_000 });
    const auth = asObject(response.authResult ?? response.auth_result);
    return {
      ...(typeof auth.hasValidAuth === "boolean"
        ? { hasValidAuth: auth.hasValidAuth }
        : typeof auth.has_valid_auth === "boolean"
          ? { hasValidAuth: auth.has_valid_auth }
          : {}),
      ...(typeof auth.uiMessage === "string"
        ? { uiMessage: auth.uiMessage }
        : typeof auth.ui_message === "string"
          ? { uiMessage: auth.ui_message }
          : {}),
      ...(typeof auth.projectId === "string"
        ? { projectId: auth.projectId }
        : typeof auth.project_id === "string"
          ? { projectId: auth.project_id }
          : {}),
      ...(Array.isArray(auth.grantedScopes)
        ? { grantedScopes: auth.grantedScopes.filter((value): value is string => typeof value === "string") }
        : Array.isArray(auth.granted_scopes)
          ? { grantedScopes: auth.granted_scopes.filter((value): value is string => typeof value === "string") }
          : {}),
    };
  }

  async loginInBrowser(timeoutMs = 300_000): Promise<AntigravityAuthResult> {
    const existing = await this.getAuthStatus({ launch: true }).catch(() => ({ hasValidAuth: false }));
    if (existing.hasValidAuth) return existing;

    const response = await this.rpc("LoginWithBrowser", {}, { launch: true, timeoutMs });
    const immediate = asObject(response.authResult ?? response.auth_result);
    if (immediate.hasValidAuth === true || immediate.has_valid_auth === true) {
      return await this.getAuthStatus({ launch: true });
    }

    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const status = await this.getAuthStatus({ launch: true }).catch(() => ({ hasValidAuth: false }));
      if (status.hasValidAuth) return status;
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
    throw new Error("Antigravity browser authentication timed out");
  }

  async logout(): Promise<void> {
    await this.rpc("AuthLogout", {}, { launch: false, timeoutMs: 10_000 });
  }

  async getAvailableModels(
    forceRefresh = true,
    options: { signal?: AbortSignal; launch?: boolean } = {},
  ): Promise<AntigravityAvailableModelsResponse> {
    const auth = await this.getAuthStatus({ launch: options.launch ?? false });
    if (!auth.hasValidAuth) {
      throw new Error("Google Antigravity is not signed in. Use Yeet's Antigravity sign-in action first.");
    }
    return await this.rpc(
      "GetAvailableModels",
      { forceRefresh },
      { launch: options.launch ?? false, timeoutMs: 15_000, ...(options.signal ? { signal: options.signal } : {}) },
    ) as AntigravityAvailableModelsResponse;
  }

  async getModelResponse(
    prompt: string,
    model: string,
    options: { timeoutMs?: number; signal?: AbortSignal } = {},
  ): Promise<string> {
    const auth = await this.getAuthStatus({ launch: true });
    if (!auth.hasValidAuth) {
      throw new Error("Google Antigravity is not signed in. Use Yeet's Antigravity sign-in action first.");
    }
    const response = await this.rpc(
      "GetModelResponse",
      { prompt, model },
      { launch: true, timeoutMs: options.timeoutMs ?? 120_000, ...(options.signal ? { signal: options.signal } : {}) },
    );
    const text = response.response;
    if (typeof text !== "string") throw new Error("Antigravity GetModelResponse did not return text");
    return text;
  }
}
