import { createHash, randomBytes } from "node:crypto";
import { createServer } from "node:http";
import type { FetchLike } from "./types.js";

export const GEMINI_OAUTH_CLIENT_ID = "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
export const GEMINI_OAUTH_SCOPES = [
  "https://www.googleapis.com/auth/cloud-platform",
  "https://www.googleapis.com/auth/userinfo.email",
  "https://www.googleapis.com/auth/userinfo.profile",
];

const CODE_ASSIST_BASE_URL = "https://cloudcode-pa.googleapis.com/v1internal";
const GEMINI_FREE_TIER = "free-tier";

export interface GeminiOAuthLoginResult {
  clientId: string;
  clientSecret?: string;
  scopes: string[];
  accessToken: string;
  refreshToken: string;
  idToken?: string;
  tokenType?: string;
  scope?: string;
  expiresIn: number;
}

export interface GeminiOAuthLoginOptions {
  fetch: FetchLike;
  openBrowser: (url: string) => Promise<void> | void;
  clientId?: string;
  clientSecret?: string;
  scopes?: string[];
  previousRefreshToken?: string;
  timeoutMs?: number;
}

export interface GeminiOAuthRefreshOptions {
  fetch: FetchLike;
  refreshToken: string;
  clientId?: string;
  clientSecret?: string;
}

export interface GeminiOAuthRefreshResult {
  accessToken: string;
  refreshToken?: string;
  tokenType?: string;
  scope?: string;
  expiresIn: number;
}

interface LoopbackResult {
  callbackUrl: string;
  waitForCallback: Promise<URL>;
  close: () => Promise<void>;
}

function pkceVerifier(): string {
  return randomBytes(48).toString("base64url");
}

function pkceChallenge(verifier: string): string {
  return createHash("sha256").update(verifier).digest("base64url");
}

async function loopbackCallback(statePath: string, timeoutMs: number): Promise<LoopbackResult> {
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

  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.once("listening", resolve);
    server.listen(0, "127.0.0.1");
  });
  const address = server.address();
  if (!address || typeof address === "string") {
    server.close();
    throw new Error("Failed to allocate Gemini OAuth loopback port");
  }

  const timer = setTimeout(() => {
    rejectCallback?.(new Error("Gemini browser authentication timed out"));
    resolveCallback = undefined;
    rejectCallback = undefined;
    server.close();
  }, timeoutMs);
  timer.unref();

  return {
    callbackUrl: `http://127.0.0.1:${address.port}${statePath}`,
    waitForCallback,
    close: async () => {
      clearTimeout(timer);
      if (!server.listening) return;
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}

function expiresIn(payload: Record<string, unknown>): number {
  const value = typeof payload.expires_in === "number"
    ? payload.expires_in
    : Number(payload.expires_in ?? 3600);
  return Number.isFinite(value) ? value : 3600;
}

export async function loginGeminiOAuth(options: GeminiOAuthLoginOptions): Promise<GeminiOAuthLoginResult> {
  const clientId = options.clientId?.trim() || GEMINI_OAUTH_CLIENT_ID;
  const clientSecret = options.clientSecret?.trim() || undefined;
  const scopes = options.scopes?.length ? [...options.scopes] : [...GEMINI_OAUTH_SCOPES];
  const state = randomBytes(32).toString("hex");
  const verifier = pkceVerifier();
  const challenge = pkceChallenge(verifier);
  const loopback = await loopbackCallback(`/oauth/gemini/${state}`, options.timeoutMs ?? 300_000);

  try {
    const authorize = new URL("https://accounts.google.com/o/oauth2/v2/auth");
    authorize.searchParams.set("client_id", clientId);
    authorize.searchParams.set("redirect_uri", loopback.callbackUrl);
    authorize.searchParams.set("response_type", "code");
    authorize.searchParams.set("scope", scopes.join(" "));
    authorize.searchParams.set("access_type", "offline");
    authorize.searchParams.set("prompt", "consent");
    authorize.searchParams.set("state", state);
    authorize.searchParams.set("code_challenge", challenge);
    authorize.searchParams.set("code_challenge_method", "S256");
    await options.openBrowser(authorize.toString());

    const callback = await loopback.waitForCallback;
    if (callback.searchParams.get("state") !== state) {
      throw new Error("Gemini OAuth callback state mismatch");
    }
    const error = callback.searchParams.get("error");
    if (error) {
      const description = callback.searchParams.get("error_description");
      throw new Error(`Gemini authorization failed: ${error}${description ? ` (${description})` : ""}`);
    }
    const code = callback.searchParams.get("code");
    if (!code) throw new Error("Gemini callback did not include an authorization code");

    const form = new URLSearchParams({
      client_id: clientId,
      code,
      code_verifier: verifier,
      grant_type: "authorization_code",
      redirect_uri: loopback.callbackUrl,
    });
    if (clientSecret && clientId !== GEMINI_OAUTH_CLIENT_ID) form.set("client_secret", clientSecret);

    const response = await options.fetch("https://oauth2.googleapis.com/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: form.toString(),
    });
    if (!response.ok) {
      throw new Error(`Gemini OAuth exchange failed (${response.status}): ${await response.text()}`);
    }
    const payload = object(await response.json());
    const accessToken = typeof payload.access_token === "string" ? payload.access_token : undefined;
    const refreshToken = typeof payload.refresh_token === "string"
      ? payload.refresh_token
      : options.previousRefreshToken;
    if (!accessToken || !refreshToken) {
      throw new Error("Gemini OAuth exchange did not return a reusable access and refresh token");
    }

    return {
      clientId,
      ...(clientSecret && clientId !== GEMINI_OAUTH_CLIENT_ID ? { clientSecret } : {}),
      scopes,
      accessToken,
      refreshToken,
      ...(typeof payload.id_token === "string" ? { idToken: payload.id_token } : {}),
      ...(typeof payload.token_type === "string" ? { tokenType: payload.token_type } : {}),
      ...(typeof payload.scope === "string" ? { scope: payload.scope } : {}),
      expiresIn: expiresIn(payload),
    };
  } finally {
    await loopback.close();
  }
}

export async function refreshGeminiOAuth(options: GeminiOAuthRefreshOptions): Promise<GeminiOAuthRefreshResult> {
  const configuredClientId = options.clientId === "gemini-cli-core" ? undefined : options.clientId;
  const clientId = configuredClientId?.trim() || GEMINI_OAUTH_CLIENT_ID;
  const form = new URLSearchParams({
    client_id: clientId,
    refresh_token: options.refreshToken,
    grant_type: "refresh_token",
  });
  if (options.clientSecret && clientId !== GEMINI_OAUTH_CLIENT_ID) {
    form.set("client_secret", options.clientSecret);
  }

  const response = await options.fetch("https://oauth2.googleapis.com/token", {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: form.toString(),
  });
  if (!response.ok) {
    throw new Error(`Gemini OAuth refresh failed (${response.status}): ${await response.text()}`);
  }
  const payload = object(await response.json());
  const accessToken = typeof payload.access_token === "string" ? payload.access_token : undefined;
  if (!accessToken) throw new Error("Gemini OAuth refresh did not return an access token");
  return {
    accessToken,
    ...(typeof payload.refresh_token === "string" ? { refreshToken: payload.refresh_token } : {}),
    ...(typeof payload.token_type === "string" ? { tokenType: payload.token_type } : {}),
    ...(typeof payload.scope === "string" ? { scope: payload.scope } : {}),
    expiresIn: expiresIn(payload),
  };
}

function object(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

async function codeAssistRequest(
  fetch: FetchLike,
  accessToken: string,
  method: string,
  body: Record<string, unknown>,
): Promise<Record<string, unknown>> {
  const response = await fetch(`${CODE_ASSIST_BASE_URL}:${method}`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${accessToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    throw new Error(`Gemini Code Assist ${method} failed (${response.status}): ${await response.text()}`);
  }
  return object(await response.json());
}

function responseProject(value: unknown): string | undefined {
  if (typeof value === "string" && value.trim()) return value.trim();
  const id = object(value).id;
  return typeof id === "string" && id.trim() ? id.trim() : undefined;
}

function eligibilityError(load: Record<string, unknown>): Error {
  const ineligible = Array.isArray(load.ineligibleTiers)
    ? load.ineligibleTiers.map((entry) => object(entry))
    : [];
  const validation = ineligible.find((entry) =>
    typeof entry.validationUrl === "string" && entry.validationUrl.trim());
  if (validation) {
    const reason = typeof validation.reasonMessage === "string" && validation.reasonMessage.trim()
      ? validation.reasonMessage.trim()
      : "Google requires account validation";
    return new Error(
      `Gemini Code Assist account validation required: ${reason}. `
      + `Complete verification at ${String(validation.validationUrl)} and sign in again.`,
    );
  }
  const messages = ineligible.flatMap((entry) =>
    typeof entry.reasonMessage === "string" && entry.reasonMessage.trim()
      ? [entry.reasonMessage.trim()]
      : []);
  if (messages.length) return new Error(`Gemini Code Assist account is not eligible: ${messages.join("; ")}`);
  return new Error(
    "Gemini Code Assist requires a Google Cloud project for this account. "
    + "Set GOOGLE_CLOUD_PROJECT or GOOGLE_CLOUD_PROJECT_ID and sign in again.",
  );
}

export async function setupGeminiCodeAssist(
  fetch: FetchLike,
  accessToken: string,
  requestedProjectId?: string,
): Promise<string> {
  const projectId = requestedProjectId?.trim() || undefined;
  if (projectId && /^\d+$/.test(projectId)) {
    throw new Error(
      "GOOGLE_CLOUD_PROJECT must be the string project ID, not the numeric project number.",
    );
  }

  const metadata = {
    ideType: "IDE_UNSPECIFIED",
    platform: "PLATFORM_UNSPECIFIED",
    pluginType: "GEMINI",
    ...(projectId ? { duetProject: projectId } : {}),
  };

  const load = await codeAssistRequest(fetch, accessToken, "loadCodeAssist", {
    ...(projectId ? { cloudaicompanionProject: projectId } : {}),
    metadata,
  });

  if (load.currentTier) {
    const loadedProject = responseProject(load.cloudaicompanionProject);
    if (loadedProject) return loadedProject;
    if (projectId) return projectId;
    throw eligibilityError(load);
  }

  const allowedTiers = Array.isArray(load.allowedTiers)
    ? load.allowedTiers.map((entry) => object(entry))
    : [];
  const tier = allowedTiers.find((candidate) => candidate.isDefault === true);
  const tierId = typeof tier?.id === "string" && tier.id ? tier.id : "legacy-tier";

  const freeTier = tierId === GEMINI_FREE_TIER;
  const onboardMetadata = freeTier
    ? {
        ideType: "IDE_UNSPECIFIED",
        platform: "PLATFORM_UNSPECIFIED",
        pluginType: "GEMINI",
      }
    : metadata;

  if (!freeTier && !projectId) throw eligibilityError(load);

  let operation = await codeAssistRequest(fetch, accessToken, "onboardUser", {
    tierId,
    ...(freeTier ? {} : { cloudaicompanionProject: projectId }),
    metadata: onboardMetadata,
  });

  if (operation.done !== true && typeof operation.name === "string" && operation.name) {
    const operationName = operation.name;
    for (let attempt = 0; operation.done !== true && attempt < 60; attempt += 1) {
      await new Promise((resolve) => setTimeout(resolve, 5_000));
      const response = await fetch(`${CODE_ASSIST_BASE_URL}/${operationName}`, {
        headers: { authorization: `Bearer ${accessToken}` },
      });
      if (!response.ok) {
        throw new Error(
          `Gemini Code Assist onboarding poll failed (${response.status}): ${await response.text()}`,
        );
      }
      operation = object(await response.json());
    }
  }

  if (operation.done !== true) throw new Error("Gemini Code Assist onboarding did not complete");

  const onboardedProject = responseProject(object(operation.response).cloudaicompanionProject);
  if (onboardedProject) return onboardedProject;
  if (projectId) return projectId;
  throw eligibilityError(load);
}
