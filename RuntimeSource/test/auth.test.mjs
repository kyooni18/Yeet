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

test("AuthManager persists custom OpenAI-compatible endpoints without storing API keys in config.json", async () => {
  const configDir = await temporaryConfig();
  const auth = new AuthManager({ configDir });

  const saved = await auth.setCustomProvider({
    id: "local-llm",
    baseUrl: "http://127.0.0.1:1234/v1/",
    headers: { "X-Local-Client": "yeet" },
    requireApiKey: true,
  });
  assert.deepEqual(saved, {
    id: "local-llm",
    baseUrl: "http://127.0.0.1:1234/v1",
    headers: { "X-Local-Client": "yeet" },
    requireApiKey: true,
  });
  await auth.setApiKey("local-llm", "secret-local-key");

  assert.deepEqual(await auth.listCustomProviders(), [saved]);
  const config = JSON.parse(await readFile(path.join(configDir, "config.json"), "utf8"));
  assert.deepEqual(config.providers["local-llm"], saved);
  assert.equal(JSON.stringify(config).includes("secret-local-key"), false);

  assert.equal(await auth.removeCustomProvider("local-llm"), true);
  assert.deepEqual(await auth.listCustomProviders(), []);
  assert.equal(await auth.removeCustomProvider("local-llm"), false);
});

test("AuthManager accepts localhost HTTP endpoints and rejects unsafe provider identifiers and URL schemes", async () => {
  const configDir = await temporaryConfig();
  const auth = new AuthManager({ configDir });

  assert.equal(
    (await auth.setCustomProvider({ id: "ollama", baseUrl: "http://localhost:11434/v1" })).baseUrl,
    "http://localhost:11434/v1",
  );
  await assert.rejects(
    auth.setCustomProvider({ id: "openai", baseUrl: "http://localhost:8000/v1" }),
    /reserved/,
  );
  await assert.rejects(
    auth.setCustomProvider({ id: "bad/id", baseUrl: "http://localhost:8000/v1" }),
    /Provider id/,
  );
  await assert.rejects(
    auth.setCustomProvider({ id: "socket", baseUrl: "file:///tmp/model.sock" }),
    /http:\/\/ or https:\/\//,
  );
});

test("OpenCode Zen and Go share OPENCODE_API_KEY and stored credentials", async () => {
  const configDir = await temporaryConfig();
  const previous = process.env.OPENCODE_API_KEY;
  process.env.OPENCODE_API_KEY = "zen-env";
  try {
    const auth = new AuthManager({ configDir });
    assert.equal((await auth.status("opencode")).method, "environment");
    assert.equal((await auth.status("opencode-go")).method, "environment");
    assert.deepEqual(await auth.resolve("opencode-go"), { kind: "api-key", value: "zen-env", source: "environment" });

    delete process.env.OPENCODE_API_KEY;
    await auth.setApiKey("opencode", "zen-stored");
    assert.deepEqual(await auth.resolve("opencode-go"), { kind: "api-key", value: "zen-stored", source: "stored" });
    assert.equal((await auth.status("opencode-go")).authenticated, true);
  } finally {
    if (previous === undefined) delete process.env.OPENCODE_API_KEY;
    else process.env.OPENCODE_API_KEY = previous;
  }
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

test("OpenAI browser auth performs Codex OAuth PKCE and stores ChatGPT tokens", async () => {
  const configDir = await temporaryConfig();
  let tokenExchangeBody = "";
  const jwt = (payload) => `${Buffer.from(JSON.stringify({ alg: "none" })).toString("base64url")}.${Buffer.from(JSON.stringify(payload)).toString("base64url")}.signature`;
  const auth = new AuthManager({
    configDir,
    openBrowser: callbackOpener((authorize) => {
      assert.equal(authorize.origin, "https://auth.openai.com");
      assert.equal(authorize.searchParams.get("client_id"), "app_EMoamEEZ73f0CkXaXp7hrann");
      assert.equal(authorize.searchParams.get("response_type"), "code");
      assert.equal(authorize.searchParams.get("code_challenge_method"), "S256");
      assert.match(authorize.searchParams.get("scope"), /offline_access/);
      const redirect = new URL(authorize.searchParams.get("redirect_uri"));
      assert.equal(redirect.hostname, "localhost");
      assert.ok(["1455", "1457"].includes(redirect.port));
      const callback = new URL(redirect);
      callback.searchParams.set("code", "openai-code");
      callback.searchParams.set("state", authorize.searchParams.get("state"));
      return callback.toString();
    }),
    fetch: async (input, init) => {
      assert.equal(String(input), "https://auth.openai.com/oauth/token");
      tokenExchangeBody = String(init?.body ?? "");
      return new Response(JSON.stringify({
        id_token: jwt({ "https://api.openai.com/auth": { chatgpt_account_id: "acct-browser" } }),
        access_token: jwt({ exp: Math.floor(Date.now() / 1000) + 3600 }),
        refresh_token: "openai-refresh",
        token_type: "Bearer",
      }), { status: 200, headers: { "content-type": "application/json" } });
    },
  });

  const status = await auth.loginInBrowser("openai", { timeoutMs: 5_000 });
  assert.equal(status.method, "browser");
  assert.match(tokenExchangeBody, /grant_type=authorization_code/);
  assert.match(tokenExchangeBody, /code=openai-code/);
  assert.match(tokenExchangeBody, /code_verifier=/);
  const credential = await auth.resolve("openai");
  assert.equal(credential.kind, "oauth");
  assert.equal(credential.source, "browser");
  assert.equal(credential.accountId, "acct-browser");
  assert.match(credential.accessToken, /^ey/);
  assert.ok(credential.expiresAt);
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

test("expired OpenAI OAuth never falls back to API billing credentials", async () => {
  const configDir = await temporaryConfig();
  const auth = new AuthManager({ configDir });
  await auth.ensure();
  const { writeFile } = await import("node:fs/promises");
  await writeFile(path.join(configDir, "credentials.json"), JSON.stringify({
    version: 1, oauthClients: {}, providers: { openai: {
      type: "oauth", source: "browser", accessToken: "expired", accountId: "account",
      expiresAt: "2000-01-01T00:00:00.000Z",
    } },
  }));
  const previous = process.env.OPENAI_API_KEY;
  process.env.OPENAI_API_KEY = "must-not-use";
  try {
    await assert.rejects(() => auth.resolve("openai"), /OAuth session expired/);
  } finally {
    if (previous === undefined) delete process.env.OPENAI_API_KEY;
    else process.env.OPENAI_API_KEY = previous;
  }
});
