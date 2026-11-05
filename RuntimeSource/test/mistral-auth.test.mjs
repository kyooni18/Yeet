import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { AuthManager } from "../dist/index.js";

const json = (value, status = 200) => new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });
const processResponse = (overrides = {}) => ({ process_id: "process-123", sign_in_url: "https://console.mistral.ai/vibe/sign-in/process-123", poll_url: "https://console.mistral.ai/api/vibe/sign-in/process-123", expires_at: new Date(Date.now() + 60_000).toISOString(), ...overrides });

test("Mistral browser login exchanges PKCE for a stored key and reads plan usage", async (t) => {
  const configDir = await mkdtemp(join(tmpdir(), "yeet-mistral-"));
  t.after(() => rm(configDir, { recursive: true, force: true }));
  let challenge, opened, billingCalls = 0;
  const auth = new AuthManager({ configDir, openBrowser: (url) => { opened = url; }, fetch: async (url, init) => {
    url = String(url);
    if (url === "https://console.mistral.ai/api/vibe/sign-in") {
      const body = JSON.parse(init.body);
      assert.equal(body.code_challenge_method, "S256");
      challenge = body.code_challenge;
      return json(processResponse());
    }
    if (url.endsWith("/process-123/exchange")) {
      const body = JSON.parse(init.body);
      assert.equal(body.exchange_token, "exchange-secret");
      assert.equal(createHash("sha256").update(body.code_verifier).digest("base64url"), challenge);
      return json({ api_key: "vibe-key" });
    }
    if (url === "https://console.mistral.ai/api/vibe/sign-in/process-123") return json({ status: "completed", exchange_token: "exchange-secret" });
    assert.equal(init.headers.authorization, "Bearer vibe-key");
    assert.equal(init.redirect, "error");
    billingCalls++;
    if (url.endsWith("/billing/subscription")) return json({ plan_name: "INDIVIDUAL", vibe_payg_enabled: true });
    if (url.endsWith("/billing/budget")) return json({ vibe_budget: { usage_percentage: 42.5, reset_at: "2026-11-01T00:00:00Z" } });
    throw new Error(`unexpected URL ${url}`);
  } });
  assert.equal((await auth.loginInBrowser("mistral")).method, "browser");
  assert.equal(opened, processResponse().sign_in_url);
  assert.deepEqual(await auth.resolve("mistral"), { kind: "api-key", value: "vibe-key", source: "browser" });
  const usage = await auth.providerUsage("mistral");
  assert.equal(usage.plan, "INDIVIDUAL");
  assert.deepEqual(usage.windows[0], { id: "vibe_monthly", label: "Vibe monthly", usedPercent: 43, remainingPercent: 57, resetsAt: "2026-11-01T00:00:00.000Z" });
  assert.match(usage.message, /Pay-as-you-go/);
  await auth.providerUsage("mistral");
  assert.equal(billingCalls, 2);
  await auth.logout("mistral");
  assert.equal((await auth.providerUsage("mistral")).available, false);
});

test("Mistral login rejects untrusted URLs and denied attempts without storing credentials", async (t) => {
  const configDir = await mkdtemp(join(tmpdir(), "yeet-mistral-"));
  t.after(() => rm(configDir, { recursive: true, force: true }));
  let opened = false;
  const unsafe = new AuthManager({ configDir, openBrowser: () => { opened = true; }, fetch: async () => json(processResponse({ poll_url: "https://evil.example/api/vibe" })) });
  await assert.rejects(unsafe.loginInBrowser("mistral"), /untrusted URL/);
  assert.equal(opened, false);
  const denied = new AuthManager({ configDir, openBrowser: () => {}, fetch: async (url) => String(url).endsWith("/sign-in") ? json(processResponse()) : json({ status: "denied" }) });
  await assert.rejects(denied.loginInBrowser("mistral"), /denied/);
  assert.equal((await denied.status("mistral")).method, "none");
});

test("Mistral billing reports missing permission without inventing a quota", async (t) => {
  const configDir = await mkdtemp(join(tmpdir(), "yeet-mistral-"));
  t.after(() => rm(configDir, { recursive: true, force: true }));
  const auth = new AuthManager({ configDir, fetch: async (url) => String(url).endsWith("/subscription") ? json({ plan_name: "TEAM" }) : json({}, 403) });
  await auth.setApiKey("mistral", "test-key");
  const usage = await auth.providerUsage("mistral");
  assert.equal(usage.available, false);
  assert.equal(usage.plan, "TEAM");
  assert.deepEqual(usage.windows, []);
  assert.match(usage.message, /permission/);
});
