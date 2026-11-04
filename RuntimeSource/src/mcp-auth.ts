import { createHash, randomBytes } from "node:crypto";
import { createServer } from "node:http";
import { chmod, mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { join } from "node:path";
import type { FetchLike } from "./types.js";

type Credential = { url: string; access_token: string; refresh_token?: string; expires_at?: number; token_endpoint?: string; client_id?: string };
type Pending = { url: string; result: Promise<Credential>; close: () => void };

function secureUrl(value: string): URL {
  const url = new URL(value);
  if (url.username || url.password || (url.protocol !== "https:" && !(url.protocol === "http:" && ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname)))) {
    throw new Error("MCP authentication requires HTTPS (or loopback HTTP)");
  }
  return url;
}

export class McpAuth {
  readonly pending = new Map<string, Pending>();
  constructor(readonly dir: string, readonly fetcher: FetchLike) {}
  async read(): Promise<Record<string, Credential>> {
    try { return JSON.parse(await readFile(join(this.dir, "mcp-credentials.json"), "utf8")); }
    catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") return {}; throw error; }
  }
  async save(name: string, credential?: Credential): Promise<void> {
    const data = await this.read();
    if (credential) {
      secureUrl(credential.url);
      Object.defineProperty(data, name, { value: credential, enumerable: true, configurable: true, writable: true });
    } else delete data[name];
    await mkdir(this.dir, { recursive: true, mode: 0o700 });
    const path = join(this.dir, "mcp-credentials.json");
    const temporary = `${path}.${randomBytes(8).toString("hex")}.tmp`;
    await writeFile(temporary, JSON.stringify(data), { mode: 0o600 });
    await chmod(temporary, 0o600);
    await rename(temporary, path);
  }
  async json(url: string, init?: RequestInit): Promise<Record<string, any>> {
    secureUrl(url);
    const response = await this.fetcher(url, { ...init, redirect: "error", signal: AbortSignal.timeout(15_000) });
    if (!response.ok) throw new Error(`MCP authentication endpoint returned HTTP ${response.status}`);
    return await response.json() as Record<string, any>;
  }
  async token(endpoint: string, params: URLSearchParams, url: string, client_id: string): Promise<Credential> {
    const data = await this.json(endpoint, { method: "POST", headers: { "Content-Type": "application/x-www-form-urlencoded" }, body: params });
    if (typeof data.access_token !== "string" || data.token_type?.toLowerCase() !== "bearer") throw new Error("OAuth server did not return a bearer token");
    return { url, access_token: data.access_token, refresh_token: data.refresh_token, token_endpoint: endpoint, client_id,
      ...(typeof data.expires_in === "number" ? { expires_at: Date.now() + data.expires_in * 1000 } : {}) };
  }
  async bearer(name: string, url: string): Promise<string | undefined> {
    const data = await this.read();
    let credential = Object.hasOwn(data, name) ? data[name] : undefined;
    if (!credential || credential.url !== url) return;
    secureUrl(url);
    if (credential.expires_at && credential.expires_at <= Date.now() + 30_000) {
      if (!credential.refresh_token || !credential.token_endpoint || !credential.client_id) throw new Error(`Run yeet mcp login ${name} again: token expired`);
      const refreshed = await this.token(credential.token_endpoint, new URLSearchParams({ grant_type: "refresh_token", refresh_token: credential.refresh_token, client_id: credential.client_id, resource: url }), url, credential.client_id);
      credential = { ...refreshed, refresh_token: refreshed.refresh_token ?? credential.refresh_token };
      await this.save(name, credential);
    }
    return credential.access_token;
  }
  async start(name: string, resource: string, clientId?: string, scope?: string): Promise<string> {
    secureUrl(resource);
    this.pending.get(name)?.close();
    const base = new URL(resource);
    const discovery = new URL(`/.well-known/oauth-protected-resource${base.pathname === "/" ? "" : base.pathname}`, base);
    let protectedResource: Record<string, any>;
    try { protectedResource = await this.json(discovery.href); }
    catch { protectedResource = await this.json(new URL("/.well-known/oauth-protected-resource", base).href); }
    if (protectedResource.resource !== resource || !Array.isArray(protectedResource.authorization_servers) || !protectedResource.authorization_servers.length) throw new Error("Invalid MCP protected-resource metadata");
    const issuer = secureUrl(protectedResource.authorization_servers[0]);
    const metadata = await this.json(new URL(`/.well-known/oauth-authorization-server${issuer.pathname === "/" ? "" : issuer.pathname}`, issuer).href);
    if (metadata.issuer !== issuer.href && metadata.issuer !== issuer.href.replace(/\/$/, "")) throw new Error("OAuth issuer mismatch");
    if (!metadata.code_challenge_methods_supported?.includes("S256")) throw new Error("OAuth server must support PKCE S256");
    secureUrl(metadata.authorization_endpoint);
    secureUrl(metadata.token_endpoint);
    const verifier = randomBytes(32).toString("base64url");
    const state = randomBytes(32).toString("base64url");
    let resolveCode!: (code: string) => void;
    let rejectCode!: (error: Error) => void;
    const code = new Promise<string>((resolve, reject) => { resolveCode = resolve; rejectCode = reject; });
    void code.catch(() => {});
    const server = createServer((request, response) => {
      const callback = new URL(request.url ?? "/", "http://127.0.0.1");
      if (request.method !== "GET" || callback.pathname !== "/callback" || callback.searchParams.get("state") !== state) { response.writeHead(400).end("Invalid OAuth callback"); return; }
      const value = callback.searchParams.get("code");
      if (!value || callback.searchParams.has("error")) { response.writeHead(400).end("Authorization denied"); rejectCode(new Error("MCP authorization denied")); return; }
      response.writeHead(200, { "Content-Type": "text/plain", "Cache-Control": "no-store" }).end("Signed in to Yeet. You can close this window.");
      resolveCode(value);
    });
    await new Promise<void>((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
    const address = server.address() as { port: number };
    const redirect = `http://127.0.0.1:${address.port}/callback`;
    const timer = setTimeout(() => rejectCode(new Error("MCP login timed out")), 180_000);
    const close = () => { clearTimeout(timer); rejectCode(new Error("MCP login closed")); server.close(); server.closeAllConnections(); };
    try {
      if (!clientId) {
        if (!metadata.registration_endpoint) throw new Error("OAuth server requires --client-id (dynamic registration unavailable)");
        const registration = await this.json(metadata.registration_endpoint, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ client_name: "Yeet", redirect_uris: [redirect], grant_types: ["authorization_code", "refresh_token"], response_types: ["code"], token_endpoint_auth_method: "none" }) });
        clientId = registration.client_id;
      }
      if (!clientId) throw new Error("OAuth registration did not return a client ID");
      const id = clientId;
      const authorization = new URL(metadata.authorization_endpoint);
      const params = { response_type: "code", client_id: id, redirect_uri: redirect, state, code_challenge: createHash("sha256").update(verifier).digest("base64url"), code_challenge_method: "S256", resource };
      for (const [key, value] of Object.entries(params)) authorization.searchParams.set(key, value);
      const requestedScope = scope ?? protectedResource.scopes_supported?.join(" ");
      if (requestedScope) authorization.searchParams.set("scope", requestedScope);
      const result = code.then(value => this.token(metadata.token_endpoint, new URLSearchParams({ grant_type: "authorization_code", code: value, client_id: id, redirect_uri: redirect, code_verifier: verifier, resource }), resource, id)).finally(close);
      void result.catch(() => {});
      this.pending.set(name, { url: resource, result, close });
      return authorization.href;
    } catch (error) { close(); void code.catch(() => {}); throw error; }
  }
  async finish(name: string): Promise<void> {
    const pending = this.pending.get(name);
    if (!pending) throw new Error("No pending MCP login");
    try { await this.save(name, await pending.result); }
    finally { pending.close(); this.pending.delete(name); }
  }
}
