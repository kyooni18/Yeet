import { createHash, randomBytes } from "node:crypto";
import { spawn } from "node:child_process";
import { chmod, mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

import type { FetchLike } from "./types.js";

export type AuthMethod = "none" | "api-key" | "browser" | "environment";

export interface AuthStatus {
  provider: string;
  authenticated: boolean;
  method: AuthMethod;
  expiresAt?: string;
  configDir: string;
}

export interface OAuthClientConfiguration {
  clientId: string;
  clientSecret?: string;
  projectId?: string;
  scopes?: string[];
}

export interface BrowserLoginOptions extends Partial<OAuthClientConfiguration> {
  timeoutMs?: number;
}

export type ResolvedCredential =
  | {
      kind: "api-key";
      value: string;
      source: "stored" | "environment" | "browser";
    }
  | {
      kind: "oauth";
      accessToken: string;
      source: "browser";
      expiresAt?: string;
      projectId?: string;
      accountId?: string;
    };

interface ApiKeyCredentialRecord {
  type: "api-key";
  value: string;
  source: "manual" | "browser";
  createdAt: string;
}

interface OAuthCredentialRecord {
  type: "oauth";
  accessToken: string;
  refreshToken?: string;
  idToken?: string;
  tokenType?: string;
  scope?: string;
  expiresAt?: string;
  accountId?: string;
  source: "browser";
  createdAt: string;
}

type CredentialRecord = ApiKeyCredentialRecord | OAuthCredentialRecord;

interface CredentialsFile {
  version: 1;
  providers: Record<string, CredentialRecord>;
  oauthClients: Record<string, OAuthClientConfiguration>;
}

export interface StoredOpenAICompatibleProviderConfiguration {
  id: string;
  baseUrl: string;
  headers?: Record<string, string>;
  requireApiKey?: boolean;
}

interface ConfigFile {
  version: 1;
  providers?: Record<string, StoredOpenAICompatibleProviderConfiguration>;
  [key: string]: unknown;
}

export interface AuthManagerOptions {
  configDir?: string;
  fetch?: FetchLike;
  openBrowser?: (url: string) => Promise<void> | void;
}

const ENV_KEYS: Record<string, string> = {
  openai: "OPENAI_API_KEY",
  anthropic: "ANTHROPIC_API_KEY",
  gemini: "GEMINI_API_KEY",
  opencode: "OPENCODE_API_KEY",
  "opencode-go": "OPENCODE_API_KEY",
  openrouter: "OPENROUTER_API_KEY",
};

const CREDENTIAL_ALIASES: Record<string, readonly string[]> = {
  opencode: ["opencode-go"],
  "opencode-go": ["opencode"],
};

const RESERVED_PROVIDER_IDS = new Set([
  "openai",
  "anthropic",
  "gemini",
  "openrouter",
  "opencode",
  "opencode-go",
]);

const GEMINI_DEFAULT_SCOPES = [
  "https://www.googleapis.com/auth/cloud-platform",
  "https://www.googleapis.com/auth/generative-language.retriever",
];

const OPENAI_OAUTH_CLIENT_ID = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_OAUTH_ISSUER = "https://auth.openai.com";
const OPENAI_OAUTH_SCOPES = "openid profile email offline_access api.connectors.read api.connectors.invoke";
const OPENAI_OAUTH_DEFAULT_PORT = 1455;
const OPENAI_OAUTH_FALLBACK_PORT = 1457;

function isoAfter(seconds: number): string {
  return new Date(Date.now() + Math.max(0, seconds) * 1_000).toISOString();
}

function asObject(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return value as Record<string, unknown>;
}

function normalizeProviderId(value: string): string {
  const id = value.trim();
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(id)) {
    throw new Error("Provider id must use only letters, numbers, '.', '_' or '-' and cannot contain '/'");
  }
  if (RESERVED_PROVIDER_IDS.has(id)) {
    throw new Error(`Provider id ${id} is reserved by a built-in provider`);
  }
  return id;
}

function normalizeProviderBaseUrl(value: string): string {
  let url: URL;
  try {
    url = new URL(value.trim());
  } catch {
    throw new Error(`Invalid provider base URL: ${value}`);
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("Provider base URL must use http:// or https://");
  }
  if (!url.hostname) throw new Error("Provider base URL must include a host");
  if (url.username || url.password) throw new Error("Provider base URL must not embed credentials");
  if (url.search || url.hash) throw new Error("Provider base URL must not include a query or fragment");
  return url.toString().replace(/\/$/, "");
}

function normalizeProviderHeaders(headers: Record<string, string> | undefined): Record<string, string> | undefined {
  if (!headers) return undefined;
  const normalized: Record<string, string> = {};
  for (const [rawName, rawValue] of Object.entries(headers)) {
    const name = rawName.trim();
    if (!name) throw new Error("Provider header names must not be empty");
    if (/\r|\n/.test(name) || /\r|\n/.test(rawValue)) {
      throw new Error(`Provider header ${name} must not contain line breaks`);
    }
    normalized[name] = rawValue;
  }
  return Object.keys(normalized).length > 0 ? normalized : undefined;
}

function normalizeStoredProvider(
  provider: StoredOpenAICompatibleProviderConfiguration,
): StoredOpenAICompatibleProviderConfiguration {
  const id = normalizeProviderId(provider.id);
  const baseUrl = normalizeProviderBaseUrl(provider.baseUrl);
  const headers = normalizeProviderHeaders(provider.headers);
  return {
    id,
    baseUrl,
    ...(headers ? { headers } : {}),
    ...(provider.requireApiKey !== undefined ? { requireApiKey: provider.requireApiKey } : {}),
  };
}

async function atomicJsonWrite(path: string, value: unknown, mode: number): Promise<void> {
  await mkdir(dirname(path), { recursive: true, mode: 0o700 });
  const tmp = `${path}.tmp-${String(process.pid)}-${randomBytes(6).toString("hex")}`;
  await writeFile(tmp, `${JSON.stringify(value, null, 2)}\n`, { encoding: "utf8", mode });
  await chmod(tmp, mode);
  await rename(tmp, path);
  await chmod(path, mode);
}

async function readJsonOr<T>(path: string, fallback: T): Promise<T> {
  try {
    return JSON.parse(await readFile(path, "utf8")) as T;
  } catch (error) {
    const code = (error as { code?: string }).code;
    if (code === "ENOENT") return fallback;
    throw error;
  }
}

function pkceVerifier(): string {
  return randomBytes(48).toString("base64url");
}

function pkceChallenge(verifier: string): string {
  return createHash("sha256").update(verifier).digest("base64url");
}

async function systemOpenBrowser(url: string): Promise<void> {
  const platform = process.platform;
  const child = platform === "darwin"
    ? spawn("open", [url], { detached: true, stdio: "ignore" })
    : platform === "win32"
      ? spawn("cmd", ["/c", "start", "", url], { detached: true, stdio: "ignore" })
      : spawn("xdg-open", [url], { detached: true, stdio: "ignore" });
  child.unref();
}

interface LoopbackResult {
  callbackUrl: string;
  waitForCallback: Promise<URL>;
  close: () => Promise<void>;
}

async function loopbackCallback(
  statePath: string,
  timeoutMs: number,
  preferredPort = 0,
): Promise<LoopbackResult> {
  let resolveCallback: ((value: URL) => void) | undefined;
  let rejectCallback: ((reason?: unknown) => void) | undefined;
  const waitForCallback = new Promise<URL>((resolve, reject) => {
    resolveCallback = resolve;
    rejectCallback = reject;
  });

  const server = createServer((request, response) => {
    const host = request.headers.host ?? "127.0.0.1";
    const url = new URL(request.url ?? "/", `http://${host}`);
    if (url.pathname !== statePath) {
      response.statusCode = 404;
      response.end("Not found");
      return;
    }
    response.statusCode = 200;
    response.setHeader("content-type", "text/plain; charset=utf-8");
    response.end("Authentication complete. You can close this window.");
    resolveCallback?.(url);
    resolveCallback = undefined;
    rejectCallback = undefined;
  });

  const listen = (port: number) => new Promise<void>((resolve, reject) => {
    const onError = (error: Error) => {
      server.off("listening", onListening);
      reject(error);
    };
    const onListening = () => {
      server.off("error", onError);
      resolve();
    };
    server.once("error", onError);
    server.once("listening", onListening);
    server.listen(port, "127.0.0.1");
  });

  try {
    await listen(preferredPort);
  } catch (error) {
    if (preferredPort !== OPENAI_OAUTH_DEFAULT_PORT || (error as NodeJS.ErrnoException).code !== "EADDRINUSE") {
      throw error;
    }
    await listen(OPENAI_OAUTH_FALLBACK_PORT);
  }

  const address = server.address();
  if (!address || typeof address === "string") {
    server.close();
    throw new Error("Failed to allocate loopback OAuth port");
  }

  const timer = setTimeout(() => {
    rejectCallback?.(new Error("Browser authentication timed out"));
    resolveCallback = undefined;
    rejectCallback = undefined;
    server.close();
  }, timeoutMs);

  const close = async () => {
    clearTimeout(timer);
    await new Promise<void>((resolve) => server.close(() => resolve()));
  };

  return {
    callbackUrl: `http://127.0.0.1:${address.port}${statePath}`,
    waitForCallback,
    close,
  };
}

export class AuthManager {
  readonly configDir: string;
  readonly configPath: string;
  readonly credentialsPath: string;
  readonly #fetch: FetchLike;
  readonly #openBrowser: (url: string) => Promise<void> | void;

  constructor(options: AuthManagerOptions = {}) {
    this.configDir = options.configDir ?? process.env.YEET_CONFIG_DIR ?? join(homedir(), ".yeet");
    this.configPath = join(this.configDir, "config.json");
    this.credentialsPath = join(this.configDir, "credentials.json");
    this.#fetch = options.fetch ?? fetch;
    this.#openBrowser = options.openBrowser ?? systemOpenBrowser;
  }

  async ensure(): Promise<void> {
    await mkdir(this.configDir, { recursive: true, mode: 0o700 });
    await chmod(this.configDir, 0o700);
    try {
      await chmod(this.configPath, 0o600);
    } catch (error) {
      if ((error as { code?: string }).code !== "ENOENT") throw error;
      await atomicJsonWrite(this.configPath, { version: 1 } satisfies ConfigFile, 0o600);
    }
    try {
      await chmod(this.credentialsPath, 0o600);
    } catch (error) {
      if ((error as { code?: string }).code !== "ENOENT") throw error;
      await atomicJsonWrite(
        this.credentialsPath,
        { version: 1, providers: {}, oauthClients: {} } satisfies CredentialsFile,
        0o600,
      );
    }
  }

  async setApiKey(provider: string, apiKey: string): Promise<AuthStatus> {
    if (!provider.trim()) throw new Error("Provider id is required");
    if (!apiKey.trim()) throw new Error("API key must not be empty");
    const data = await this.#readCredentials();
    data.providers[provider] = {
      type: "api-key",
      value: apiKey.trim(),
      source: "manual",
      createdAt: new Date().toISOString(),
    };
    await this.#writeCredentials(data);
    return this.status(provider);
  }

  async listCustomProviders(): Promise<StoredOpenAICompatibleProviderConfiguration[]> {
    await this.ensure();
    const data = await this.#readConfig();
    const providers = data.providers;
    if (!providers || typeof providers !== "object" || Array.isArray(providers)) return [];

    const output: StoredOpenAICompatibleProviderConfiguration[] = [];
    for (const [key, value] of Object.entries(providers)) {
      if (!value || typeof value !== "object" || Array.isArray(value)) continue;
      const candidate = value as Partial<StoredOpenAICompatibleProviderConfiguration>;
      if (typeof candidate.baseUrl !== "string") continue;
      try {
        output.push(normalizeStoredProvider({
          id: typeof candidate.id === "string" ? candidate.id : key,
          baseUrl: candidate.baseUrl,
          ...(candidate.headers && typeof candidate.headers === "object" && !Array.isArray(candidate.headers)
            ? { headers: candidate.headers as Record<string, string> }
            : {}),
          ...(typeof candidate.requireApiKey === "boolean" ? { requireApiKey: candidate.requireApiKey } : {}),
        }));
      } catch {
        // Ignore malformed hand-edited entries so one bad provider does not
        // prevent Yeet from starting. Saving through the API validates them.
      }
    }
    return output.sort((a, b) => a.id.localeCompare(b.id));
  }

  async setCustomProvider(
    provider: StoredOpenAICompatibleProviderConfiguration,
  ): Promise<StoredOpenAICompatibleProviderConfiguration> {
    const normalized = normalizeStoredProvider(provider);
    const data = await this.#readConfig();
    const providers = data.providers && typeof data.providers === "object" && !Array.isArray(data.providers)
      ? { ...data.providers }
      : {};
    providers[normalized.id] = normalized;
    data.providers = providers;
    await this.#writeConfig(data);
    return normalized;
  }

  async removeCustomProvider(provider: string): Promise<boolean> {
    const id = provider.trim();
    if (!id) throw new Error("Provider id is required");
    const data = await this.#readConfig();
    const providers = data.providers && typeof data.providers === "object" && !Array.isArray(data.providers)
      ? { ...data.providers }
      : {};
    const removed = Object.prototype.hasOwnProperty.call(providers, id);
    if (!removed) return false;
    delete providers[id];
    if (Object.keys(providers).length > 0) data.providers = providers;
    else delete data.providers;
    await this.#writeConfig(data);
    return true;
  }

  async configureOAuthClient(provider: string, configuration: OAuthClientConfiguration): Promise<void> {
    if (!configuration.clientId.trim()) throw new Error("OAuth clientId is required");
    const data = await this.#readCredentials();
    data.oauthClients[provider] = configuration;
    await this.#writeCredentials(data);
  }

  async logout(provider: string): Promise<AuthStatus> {
    const data = await this.#readCredentials();
    delete data.providers[provider];
    await this.#writeCredentials(data);
    return this.status(provider);
  }

  async status(provider: string): Promise<AuthStatus> {
    await this.ensure();
    const data = await this.#readCredentials();
    const stored = this.#storedCredential(data, provider);
    if (stored) {
      return {
        provider,
        authenticated: stored.type === "api-key" || Boolean(stored.refreshToken) || !this.#expired(stored.expiresAt),
        method: stored.source === "browser" ? "browser" : "api-key",
        ...(stored.type === "oauth" && stored.expiresAt ? { expiresAt: stored.expiresAt } : {}),
        configDir: this.configDir,
      };
    }

    const envName = ENV_KEYS[provider];
    const envValue = envName ? process.env[envName] : undefined;
    return {
      provider,
      authenticated: Boolean(envValue),
      method: envValue ? "environment" : "none",
      configDir: this.configDir,
    };
  }

  async resolve(provider: string): Promise<ResolvedCredential | undefined> {
    await this.ensure();
    let data = await this.#readCredentials();
    let stored = this.#storedCredential(data, provider);

    if (stored?.type === "oauth" && this.#expiresSoon(stored.expiresAt)) {
      if (provider === "openai" && stored.refreshToken) {
        stored = await this.#refreshOpenAI(stored);
        data.providers.openai = stored;
        await this.#writeCredentials(data);
      } else if (provider === "gemini" && stored.refreshToken) {
        stored = await this.#refreshGemini(stored, data.oauthClients.gemini);
        data.providers.gemini = stored;
        await this.#writeCredentials(data);
      }
    }

    if (stored?.type === "api-key") {
      return {
        kind: "api-key",
        value: stored.value,
        source: stored.source === "browser" ? "browser" : "stored",
      };
    }
    if (stored?.type === "oauth" && !this.#expired(stored.expiresAt)) {
      const oauthClient = data.oauthClients[provider];
      return {
        kind: "oauth",
        accessToken: stored.accessToken,
        source: "browser",
        ...(stored.expiresAt ? { expiresAt: stored.expiresAt } : {}),
        ...(oauthClient?.projectId ? { projectId: oauthClient.projectId } : {}),
        ...(stored.accountId ? { accountId: stored.accountId } : {}),
      };
    }

    if (stored?.type === "oauth") {
      throw new Error(`${provider} OAuth session expired. Sign in again to continue using your subscription.`);
    }
    const envName = ENV_KEYS[provider];
    const envValue = envName ? process.env[envName] : undefined;
    if (envValue) return { kind: "api-key", value: envValue, source: "environment" };
    return undefined;
  }

  async loginInBrowser(provider: string, options: BrowserLoginOptions = {}): Promise<AuthStatus> {
    await this.ensure();
    switch (provider) {
      case "openrouter":
        await this.#loginOpenRouter(options);
        return this.status(provider);
      case "openai":
        await this.#loginOpenAI(options);
        return this.status(provider);
      case "gemini":
        await this.#loginGemini(options);
        return this.status(provider);
      case "anthropic":
        throw new Error("Browser auth for the direct Anthropic API is not exposed as a reusable public client flow; use an API key for the call core");
      default:
        throw new Error(`Browser authentication is not configured for provider ${provider}`);
    }
  }

  #expired(expiresAt: string | undefined): boolean {
    return expiresAt !== undefined && Date.parse(expiresAt) <= Date.now();
  }

  #storedCredential(data: CredentialsFile, provider: string): CredentialRecord | undefined {
    const direct = data.providers[provider];
    if (direct) return direct;
    for (const alias of CREDENTIAL_ALIASES[provider] ?? []) {
      const candidate = data.providers[alias];
      if (candidate) return candidate;
    }
    return undefined;
  }

  #expiresSoon(expiresAt: string | undefined): boolean {
    return expiresAt !== undefined && Date.parse(expiresAt) <= Date.now() + 60_000;
  }

  async #readCredentials(): Promise<CredentialsFile> {
    await this.ensureBaseDirectory();
    const data = await readJsonOr<CredentialsFile>(this.credentialsPath, {
      version: 1,
      providers: {},
      oauthClients: {},
    });
    data.providers ??= {};
    data.oauthClients ??= {};
    return data;
  }

  async #readConfig(): Promise<ConfigFile> {
    await this.ensureBaseDirectory();
    return readJsonOr<ConfigFile>(this.configPath, { version: 1 });
  }

  async ensureBaseDirectory(): Promise<void> {
    await mkdir(this.configDir, { recursive: true, mode: 0o700 });
    await chmod(this.configDir, 0o700);
  }

  async #writeCredentials(data: CredentialsFile): Promise<void> {
    await atomicJsonWrite(this.credentialsPath, data, 0o600);
  }

  async #writeConfig(data: ConfigFile): Promise<void> {
    await atomicJsonWrite(this.configPath, data, 0o600);
  }

  async #loginOpenRouter(options: BrowserLoginOptions): Promise<void> {
    const timeoutMs = options.timeoutMs ?? 180_000;
    const state = randomBytes(18).toString("base64url");
    const statePath = `/oauth/openrouter/${state}`;
    const verifier = pkceVerifier();
    const challenge = pkceChallenge(verifier);
    const loopback = await loopbackCallback(statePath, timeoutMs);

    try {
      const authorize = new URL("https://openrouter.ai/auth");
      authorize.searchParams.set("callback_url", loopback.callbackUrl);
      authorize.searchParams.set("code_challenge", challenge);
      authorize.searchParams.set("code_challenge_method", "S256");
      await this.#openBrowser(authorize.toString());

      const callback = await loopback.waitForCallback;
      const error = callback.searchParams.get("error");
      if (error) throw new Error(`OpenRouter authorization failed: ${error}`);
      const code = callback.searchParams.get("code");
      if (!code) throw new Error("OpenRouter callback did not include an authorization code");

      const response = await this.#fetch("https://openrouter.ai/api/v1/auth/keys", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ code, code_verifier: verifier, code_challenge_method: "S256" }),
      });
      if (!response.ok) throw new Error(`OpenRouter OAuth exchange failed (${response.status}): ${await response.text()}`);
      const payload = asObject(await response.json());
      const key = typeof payload.key === "string" ? payload.key : undefined;
      if (!key) throw new Error("OpenRouter OAuth exchange did not return an API key");

      const data = await this.#readCredentials();
      data.providers.openrouter = {
        type: "api-key",
        value: key,
        source: "browser",
        createdAt: new Date().toISOString(),
      };
      await this.#writeCredentials(data);
    } finally {
      await loopback.close();
    }
  }

  async #loginOpenAI(options: BrowserLoginOptions): Promise<void> {
    const timeoutMs = options.timeoutMs ?? 180_000;
    const clientId = options.clientId
      ?? process.env.CODEX_APP_SERVER_LOGIN_CLIENT_ID
      ?? OPENAI_OAUTH_CLIENT_ID;
    const state = randomBytes(32).toString("base64url");
    const statePath = "/auth/callback";
    const verifier = pkceVerifier();
    const challenge = pkceChallenge(verifier);
    const loopback = await loopbackCallback(statePath, timeoutMs, OPENAI_OAUTH_DEFAULT_PORT);
    const redirectUri = `http://localhost:${new URL(loopback.callbackUrl).port}${statePath}`;

    try {
      const authorize = new URL(`${OPENAI_OAUTH_ISSUER}/oauth/authorize`);
      authorize.searchParams.set("response_type", "code");
      authorize.searchParams.set("client_id", clientId);
      authorize.searchParams.set("redirect_uri", redirectUri);
      authorize.searchParams.set("scope", OPENAI_OAUTH_SCOPES);
      authorize.searchParams.set("code_challenge", challenge);
      authorize.searchParams.set("code_challenge_method", "S256");
      authorize.searchParams.set("id_token_add_organizations", "true");
      authorize.searchParams.set("codex_cli_simplified_flow", "true");
      authorize.searchParams.set("state", state);
      authorize.searchParams.set("originator", "codex_cli_rs");
      await this.#openBrowser(authorize.toString());

      const callback = await loopback.waitForCallback;
      if (callback.searchParams.get("state") !== state) throw new Error("OpenAI OAuth callback state mismatch");
      const error = callback.searchParams.get("error");
      if (error) throw new Error(`OpenAI authorization failed: ${error}`);
      const code = callback.searchParams.get("code");
      if (!code) throw new Error("OpenAI callback did not include an authorization code");

      const form = new URLSearchParams({
        grant_type: "authorization_code",
        code,
        redirect_uri: redirectUri,
        client_id: clientId,
        code_verifier: verifier,
      });
      const response = await this.#fetch(`${OPENAI_OAUTH_ISSUER}/oauth/token`, {
        method: "POST",
        headers: { "content-type": "application/x-www-form-urlencoded" },
        body: form.toString(),
      });
      if (!response.ok) throw new Error(`OpenAI OAuth exchange failed (${response.status}): ${await response.text()}`);
      const payload = asObject(await response.json());
      const accessToken = typeof payload.access_token === "string" ? payload.access_token : undefined;
      const refreshToken = typeof payload.refresh_token === "string" ? payload.refresh_token : undefined;
      const idToken = typeof payload.id_token === "string" ? payload.id_token : undefined;
      if (!accessToken || !refreshToken) {
        throw new Error("OpenAI OAuth exchange did not return access and refresh tokens");
      }

      const accountId = accountIdFromJwt(idToken) ?? accountIdFromJwt(accessToken);
      if (!accountId) throw new Error("OpenAI OAuth exchange did not include a ChatGPT account id");
      const expiresAt = jwtExpiresAt(accessToken);
      const latest = await this.#readCredentials();
      latest.providers.openai = {
        type: "oauth",
        accessToken,
        refreshToken,
        ...(idToken ? { idToken } : {}),
        ...(typeof payload.token_type === "string" ? { tokenType: payload.token_type } : {}),
        ...(typeof payload.scope === "string" ? { scope: payload.scope } : {}),
        ...(expiresAt ? { expiresAt } : {}),
        accountId,
        source: "browser",
        createdAt: new Date().toISOString(),
      };
      await this.#writeCredentials(latest);
    } finally {
      await loopback.close();
    }
  }

  async #loginGemini(options: BrowserLoginOptions): Promise<void> {
    const data = await this.#readCredentials();
    const existing = data.oauthClients.gemini;
    const clientSecret = options.clientSecret ?? existing?.clientSecret;
    const projectId = options.projectId ?? existing?.projectId;
    const client: OAuthClientConfiguration = {
      clientId: options.clientId ?? existing?.clientId ?? "",
      ...(clientSecret !== undefined ? { clientSecret } : {}),
      ...(projectId !== undefined ? { projectId } : {}),
      scopes: options.scopes ?? existing?.scopes ?? GEMINI_DEFAULT_SCOPES,
    };
    if (!client.clientId) {
      throw new Error("Gemini browser auth requires a Google OAuth Desktop clientId on first login");
    }
    data.oauthClients.gemini = client;
    await this.#writeCredentials(data);

    const timeoutMs = options.timeoutMs ?? 180_000;
    const state = randomBytes(18).toString("base64url");
    const statePath = `/oauth/gemini/${state}`;
    const verifier = pkceVerifier();
    const challenge = pkceChallenge(verifier);
    const loopback = await loopbackCallback(statePath, timeoutMs);

    try {
      const authorize = new URL("https://accounts.google.com/o/oauth2/v2/auth");
      authorize.searchParams.set("client_id", client.clientId);
      authorize.searchParams.set("redirect_uri", loopback.callbackUrl);
      authorize.searchParams.set("response_type", "code");
      authorize.searchParams.set("scope", (client.scopes ?? GEMINI_DEFAULT_SCOPES).join(" "));
      authorize.searchParams.set("access_type", "offline");
      authorize.searchParams.set("prompt", "consent");
      authorize.searchParams.set("state", state);
      authorize.searchParams.set("code_challenge", challenge);
      authorize.searchParams.set("code_challenge_method", "S256");
      await this.#openBrowser(authorize.toString());

      const callback = await loopback.waitForCallback;
      const returnedState = callback.searchParams.get("state");
      if (returnedState !== state) throw new Error("Gemini OAuth callback state mismatch");
      const error = callback.searchParams.get("error");
      if (error) throw new Error(`Gemini authorization failed: ${error}`);
      const code = callback.searchParams.get("code");
      if (!code) throw new Error("Gemini callback did not include an authorization code");

      const form = new URLSearchParams({
        client_id: client.clientId,
        code,
        code_verifier: verifier,
        grant_type: "authorization_code",
        redirect_uri: loopback.callbackUrl,
      });
      if (client.clientSecret) form.set("client_secret", client.clientSecret);

      const response = await this.#fetch("https://oauth2.googleapis.com/token", {
        method: "POST",
        headers: { "content-type": "application/x-www-form-urlencoded" },
        body: form.toString(),
      });
      if (!response.ok) throw new Error(`Gemini OAuth exchange failed (${response.status}): ${await response.text()}`);
      const payload = asObject(await response.json());
      const accessToken = typeof payload.access_token === "string" ? payload.access_token : undefined;
      if (!accessToken) throw new Error("Gemini OAuth exchange did not return an access token");
      const expiresIn = typeof payload.expires_in === "number" ? payload.expires_in : Number(payload.expires_in ?? 3600);

      const latest = await this.#readCredentials();
      latest.providers.gemini = {
        type: "oauth",
        accessToken,
        ...(typeof payload.refresh_token === "string" ? { refreshToken: payload.refresh_token } : {}),
        ...(typeof payload.token_type === "string" ? { tokenType: payload.token_type } : {}),
        ...(typeof payload.scope === "string" ? { scope: payload.scope } : {}),
        expiresAt: isoAfter(Number.isFinite(expiresIn) ? expiresIn : 3600),
        source: "browser",
        createdAt: new Date().toISOString(),
      };
      await this.#writeCredentials(latest);
    } finally {
      await loopback.close();
    }
  }

  async #refreshGemini(
    credential: OAuthCredentialRecord,
    client: OAuthClientConfiguration | undefined,
  ): Promise<OAuthCredentialRecord> {
    if (!credential.refreshToken || !client?.clientId) return credential;
    const form = new URLSearchParams({
      client_id: client.clientId,
      refresh_token: credential.refreshToken,
      grant_type: "refresh_token",
    });
    if (client.clientSecret) form.set("client_secret", client.clientSecret);

    const response = await this.#fetch("https://oauth2.googleapis.com/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: form.toString(),
    });
    if (!response.ok) throw new Error(`Gemini OAuth refresh failed (${response.status}): ${await response.text()}`);
    const payload = asObject(await response.json());
    const accessToken = typeof payload.access_token === "string" ? payload.access_token : undefined;
    if (!accessToken) throw new Error("Gemini OAuth refresh did not return an access token");
    const expiresIn = typeof payload.expires_in === "number" ? payload.expires_in : Number(payload.expires_in ?? 3600);
    return {
      ...credential,
      accessToken,
      ...(typeof payload.refresh_token === "string" ? { refreshToken: payload.refresh_token } : {}),
      ...(typeof payload.token_type === "string" ? { tokenType: payload.token_type } : {}),
      ...(typeof payload.scope === "string" ? { scope: payload.scope } : {}),
      expiresAt: isoAfter(Number.isFinite(expiresIn) ? expiresIn : 3600),
    };
  }

  async #refreshOpenAI(credential: OAuthCredentialRecord): Promise<OAuthCredentialRecord> {
    if (!credential.refreshToken) return credential;
    const clientId = process.env.CODEX_APP_SERVER_LOGIN_CLIENT_ID ?? OPENAI_OAUTH_CLIENT_ID;
    const response = await this.#fetch(`${OPENAI_OAUTH_ISSUER}/oauth/token`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ client_id: clientId, grant_type: "refresh_token", refresh_token: credential.refreshToken }),
    });
    if (!response.ok) throw new Error(`OpenAI OAuth refresh failed (${response.status}): ${await response.text()}`);
    const payload = asObject(await response.json());
    const accessToken = typeof payload.access_token === "string" ? payload.access_token : undefined;
    if (!accessToken) throw new Error("OpenAI OAuth refresh did not return an access token");
    const idToken = typeof payload.id_token === "string" ? payload.id_token : undefined;
    const expiresAt = jwtExpiresAt(accessToken);
    const accountId = accountIdFromJwt(idToken);
    return {
      ...credential,
      accessToken,
      ...(typeof payload.refresh_token === "string" ? { refreshToken: payload.refresh_token } : {}),
      ...(idToken ? { idToken } : {}),
      ...(typeof payload.token_type === "string" ? { tokenType: payload.token_type } : {}),
      ...(expiresAt ? { expiresAt } : {}),
      ...(accountId ? { accountId } : {}),
    };
  }
}

function jwtExpiresAt(token: string | undefined): string | undefined {
  if (!token) return undefined;
  try {
    const encoded = token.split(".")[1];
    if (!encoded) return undefined;
    const payload = asObject(JSON.parse(Buffer.from(encoded, "base64url").toString("utf8")));
    return typeof payload.exp === "number" && Number.isFinite(payload.exp)
      ? new Date(payload.exp * 1_000).toISOString()
      : undefined;
  } catch {
    return undefined;
  }
}

function accountIdFromJwt(token: string | undefined): string | undefined {
  if (!token) return undefined;
  try {
    const encoded = token.split(".")[1];
    if (!encoded) return undefined;
    const payload = asObject(JSON.parse(Buffer.from(encoded, "base64url").toString("utf8")));
    const auth = asObject(payload["https://api.openai.com/auth"]);
    return typeof auth.chatgpt_account_id === "string" ? auth.chatgpt_account_id : undefined;
  } catch {
    return undefined;
  }
}
