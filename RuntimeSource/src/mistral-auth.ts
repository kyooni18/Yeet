import { createHash, randomBytes } from "node:crypto";
import type { FetchLike } from "./types.js";

// Mistral Vibe's public browser sign-in exchanges a PKCE-protected process
// for a scoped API key. Never send the verifier or token to returned URLs.
const apiBase = "https://console.mistral.ai/api";
const consoleOrigin = "https://console.mistral.ai";

function object(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

function trustedUrl(value: unknown, pathPrefix: string): string {
  if (typeof value !== "string") throw new Error("Mistral sign-in returned an invalid URL");
  let url: URL;
  try { url = new URL(value); } catch { throw new Error("Mistral sign-in returned an invalid URL"); }
  if (url.origin !== consoleOrigin || url.username || url.password || !url.pathname.startsWith(pathPrefix)) {
    throw new Error("Mistral sign-in returned an untrusted URL");
  }
  return url.toString();
}

export async function loginMistral(
  fetch: FetchLike,
  openBrowser: (url: string) => Promise<void> | void,
  timeoutMs = 180_000,
): Promise<string> {
  const deadline = Date.now() + Math.max(1, timeoutMs);
  const verifier = randomBytes(64).toString("base64url");
  const challenge = createHash("sha256").update(verifier).digest("base64url");
  const request = async (url: string, init: RequestInit = {}): Promise<Response> => {
    const remaining = deadline - Date.now();
    if (remaining <= 0) throw new Error("Mistral browser sign-in timed out");
    return fetch(url, { ...init, redirect: "error", signal: AbortSignal.timeout(remaining) });
  };
  const start = await request(`${apiBase}/vibe/sign-in`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ code_challenge: challenge, code_challenge_method: "S256" }),
  });
  if (!start.ok) throw new Error(`Mistral browser sign-in could not start (HTTP ${start.status})`);
  const process = object(await start.json());
  if (typeof process.process_id !== "string" || !/^[A-Za-z0-9_-]+$/.test(process.process_id)) throw new Error("Mistral sign-in returned an invalid process id");
  const signInUrl = trustedUrl(process.sign_in_url, "/");
  const pollUrl = trustedUrl(process.poll_url, "/api/");
  const expiresAt = typeof process.expires_at === "string" ? Date.parse(process.expires_at) : NaN;
  if (!Number.isFinite(expiresAt) || expiresAt <= Date.now()) throw new Error("Mistral sign-in process has expired or has invalid expiry");
  await openBrowser(signInUrl);
  while (Date.now() < deadline && Date.now() < expiresAt) {
    const response = await request(pollUrl);
    if (response.status === 410) throw new Error("Mistral browser sign-in expired");
    if (!response.ok) throw new Error(`Mistral browser sign-in polling failed (HTTP ${response.status})`);
    const result = object(await response.json());
    if (result.status === "completed") {
      if (typeof result.exchange_token !== "string" || !result.exchange_token) throw new Error("Mistral sign-in did not return an exchange token");
      const exchange = await request(`${apiBase}/vibe/sign-in/${encodeURIComponent(process.process_id)}/exchange`, {
        method: "POST", headers: { "content-type": "application/json" },
        body: JSON.stringify({ exchange_token: result.exchange_token, code_verifier: verifier }),
      });
      if (!exchange.ok) throw new Error(`Mistral sign-in exchange failed (HTTP ${exchange.status})`);
      const payload = object(await exchange.json());
      if (typeof payload.api_key !== "string" || !payload.api_key.trim()) throw new Error("Mistral sign-in did not return an API key");
      return payload.api_key.trim();
    }
    if (result.status !== "pending") throw new Error(`Mistral browser sign-in ${result.status === "denied" ? "was denied" : result.status === "expired" ? "expired" : "failed"}`);
    await new Promise((resolve) => setTimeout(resolve, Math.max(0, Math.min(3000, deadline - Date.now(), expiresAt - Date.now()))));
  }
  throw new Error("Mistral browser sign-in timed out or expired");
}
