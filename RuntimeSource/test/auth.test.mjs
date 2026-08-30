import assert from "node:assert/strict";
import { mkdtemp, readFile, stat } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { AuthManager, GeminiProvider } from "../dist/index.js";

async function temporaryConfig() {
  return mkdtemp(path.join(os.tmpdir(), "yeet-auth-"));
}

function callbackOpener(transform = (url) => url) {
  return async (authorizationUrl) => {
    const authorize = new URL(authorizationUrl);
    const callback = transform(authorize);
    setTimeout(() => {
      fetch(callback).catch(() => {});
    }, 10);
  };
}

test("AuthManager stores API keys under the yeet config directory with restrictive permissions", async () => {
  const configDir = await temporaryConfig();
  const auth = new AuthManager({ configDir });
  await auth.ensure();

  assert.equal((await auth.status("openai")).method, "none");
  const status = await auth.setApiKey("openai", "sk-test");
  assert.equal(status.authenticated, true);
  assert.equal(status.method, "api-key");
  assert.deepEqual(await auth.resolve("openai"), { kind: "api-key", value: "sk-test", source: "stored" });

  assert.equal((await stat(configDir)).mode & 0o777, 0o700);
  assert.equal((await stat(path.join(configDir, "credentials.json"))).mode & 0o777, 0o600);
  assert.equal((await stat(path.join(configDir, "config.json"))).mode & 0o777, 0o600);
});

test("OpenRouter browser auth performs PKCE and stores the exchanged user key", async () => {
  const configDir = await temporaryConfig();
  let exchangeBody;
  const auth = new AuthManager({
    configDir,
    openBrowser: callbackOpener((authorize) => {
      assert.equal(authorize.origin, "https://openrouter.ai");
      assert.equal(authorize.searchParams.get("code_challenge_method"), "S256");
      const callback = new URL(authorize.searchParams.get("callback_url"));
      callback.searchParams.set("code", "openrouter-code");
      return callback.toString();
    }),
    fetch: async (input, init) => {
      assert.equal(String(input), "https://openrouter.ai/api/v1/auth/keys");
      exchangeBody = JSON.parse(String(init?.body));
      return new Response(JSON.stringify({ key: "sk-or-browser" }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    },
  });

  const status = await auth.loginInBrowser("openrouter", { timeoutMs: 5_000 });
  assert.equal(status.method, "browser");
  assert.equal(exchangeBody.code, "openrouter-code");
  assert.equal(exchangeBody.code_challenge_method, "S256");
  assert.ok(exchangeBody.code_verifier.length > 40);
  assert.deepEqual(await auth.resolve("openrouter"), {
    kind: "api-key",
    value: "sk-or-browser",
    source: "browser",
  });
});

test("Gemini browser auth stores OAuth tokens and GeminiProvider sends bearer auth", async () => {
  const configDir = await temporaryConfig();
  let tokenExchangeBody = "";
  const auth = new AuthManager({
    configDir,
    openBrowser: callbackOpener((authorize) => {
      assert.equal(authorize.origin, "https://accounts.google.com");
      const callback = new URL(authorize.searchParams.get("redirect_uri"));
      callback.searchParams.set("code", "google-code");
      callback.searchParams.set("state", authorize.searchParams.get("state"));
      return callback.toString();
    }),
    fetch: async (input, init) => {
      assert.equal(String(input), "https://oauth2.googleapis.com/token");
      tokenExchangeBody = String(init?.body ?? "");
      return new Response(JSON.stringify({
        access_token: "google-access",
        refresh_token: "google-refresh",
        expires_in: 3600,
        token_type: "Bearer",
      }), { status: 200, headers: { "content-type": "application/json" } });
    },
  });

  const status = await auth.loginInBrowser("gemini", {
    clientId: "desktop-client-id",
    clientSecret: "desktop-secret",
    projectId: "project-123",
    timeoutMs: 5_000,
  });
  assert.equal(status.method, "browser");
  assert.match(tokenExchangeBody, /code=google-code/);
  const credential = await auth.resolve("gemini");
  assert.equal(credential.kind, "oauth");
  assert.equal(credential.accessToken, "google-access");
  assert.equal(credential.projectId, "project-123");

  let headers;
  const provider = new GeminiProvider({
    accessToken: credential.accessToken,
    projectId: credential.projectId,
    fetch: async (_input, init) => {
      headers = init.headers;
      return new Response(JSON.stringify({
        candidates: [{ content: { parts: [{ text: "ok" }] }, finishReason: "STOP" }],
      }), { status: 200, headers: { "content-type": "application/json" } });
    },
  });
  const result = await provider.complete({ model: "gemini-test", messages: [{ role: "user", content: "hi" }] });
  assert.equal(result.text, "ok");
  assert.equal(headers.authorization, "Bearer google-access");
  assert.equal(headers["x-goog-user-project"], "project-123");

  const persisted = JSON.parse(await readFile(path.join(configDir, "credentials.json"), "utf8"));
  assert.equal(persisted.oauthClients.gemini.clientId, "desktop-client-id");
  assert.equal(persisted.providers.gemini.refreshToken, "google-refresh");
});
