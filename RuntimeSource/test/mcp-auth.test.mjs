import assert from 'node:assert/strict';
import { mkdtemp, readFile, stat, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import test from 'node:test';
import { McpManager } from '../dist/mcp.js';

test('MCP bearer credentials persist separately, stay URL-bound, and are removed on logout', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'yeet-mcp-auth-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const manager = new McpManager({ configDir: dir, fetch: async (_input, init) => {
    assert.equal(new Headers(init.headers).get('Authorization'), 'Bearer secret');
    assert.equal(init.redirect, 'error');
    const rpc = JSON.parse(init.body);
    return Response.json({ jsonrpc: '2.0', id: rpc.id, result: { tools: [] } });
  } });
  await manager.setServer({ name: 'example', transport: 'http', url: 'https://example.com/mcp' });
  await manager.login('example', { token: 'secret' });
  assert.equal(await manager.auth.bearer('example', 'https://example.com/mcp'), 'secret');
  assert.deepEqual(await manager.listTools('example'), []);
  const second = new McpManager({ configDir: dir });
  assert.equal(await second.auth.bearer('example', 'https://example.com/mcp'), 'secret');
  assert.equal(await second.auth.bearer('example', 'https://other.example/mcp'), undefined);
  assert.ok(!(await readFile(join(dir, 'mcp.json'), 'utf8')).includes('secret'));
  assert.ok(!JSON.stringify(await manager.listServers()).includes('secret'));
  if (process.platform !== 'win32') assert.equal((await stat(join(dir, 'mcp-credentials.json'))).mode & 0o777, 0o600);
  await manager.login('example', { logout: true });
  assert.equal(await manager.auth.bearer('example', 'https://example.com/mcp'), undefined);
  await assert.rejects(manager.login('example', { token: 'bad\r\nheader' }));
  await manager.setServer({ name: 'local', transport: 'stdio', command: 'test' });
  await assert.rejects(manager.login('local', { token: 'secret' }), /HTTP/);
  await manager.setServer({ name: 'insecure', transport: 'http', url: 'http://example.com/mcp' });
  await assert.rejects(manager.login('insecure', { token: 'secret' }), /HTTPS/);
});

test('MCP OAuth checks state, sends PKCE and resource, persists and refreshes tokens', async t => {
  const dir = await mkdtemp(join(tmpdir(), 'yeet-mcp-oauth-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  let authorization;
  let refreshes = 0;
  const manager = new McpManager({ configDir: dir, fetch: async (input, init) => {
    const url = String(input);
    let result;
    if (url.includes('oauth-protected-resource')) result = { resource: 'https://example.com/mcp', authorization_servers: ['https://auth.example'], scopes_supported: ['mcp'] };
    else if (url.includes('oauth-authorization-server')) result = { issuer: 'https://auth.example', authorization_endpoint: 'https://auth.example/authorize', token_endpoint: 'https://auth.example/token', registration_endpoint: 'https://auth.example/register', code_challenge_methods_supported: ['S256'] };
    else if (url.endsWith('/register')) {
      assert.equal(JSON.parse(init.body).token_endpoint_auth_method, 'none');
      result = { client_id: 'yeet-public' };
    } else {
      const params = init.body;
      assert.equal(params.get('resource'), 'https://example.com/mcp');
      if (params.get('grant_type') === 'authorization_code') {
        assert.equal(params.get('code'), 'valid-code');
        assert.equal(createHash('sha256').update(params.get('code_verifier')).digest('base64url'), authorization.searchParams.get('code_challenge'));
        result = { access_token: 'first', token_type: 'Bearer', refresh_token: 'refresh', expires_in: 0 };
      } else {
        assert.equal(params.get('refresh_token'), 'refresh');
        refreshes++;
        result = { access_token: 'renewed', token_type: 'Bearer', expires_in: 3600 };
      }
    }
    return Response.json(result);
  } });
  t.after(() => manager.close());
  await manager.setServer({ name: 'example', transport: 'http', url: 'https://example.com/mcp' });
  authorization = new URL(await manager.login('example', {}));
  const callback = new URL(authorization.searchParams.get('redirect_uri'));
  callback.searchParams.set('state', 'wrong');
  callback.searchParams.set('code', 'valid-code');
  assert.equal((await fetch(callback)).status, 400);
  callback.searchParams.set('state', authorization.searchParams.get('state'));
  assert.equal((await fetch(callback)).status, 200);
  await manager.login('example', { finish: true });
  assert.equal(await manager.auth.bearer('example', 'https://example.com/mcp'), 'renewed');
  assert.equal(refreshes, 1);
});
