import assert from 'node:assert/strict';
import http from 'node:http';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { McpManager, SkillRegistry } from '../dist/index.js';

function listen(server) {
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => resolve(server.address()));
  });
}

async function tempConfig() {
  return mkdtemp(path.join(os.tmpdir(), 'yeet-core-'));
}

test('SkillRegistry progressively loads ~/.yeet/skills and blocks path escape', async () => {
  const configDir = await tempConfig();
  const root = path.join(configDir, 'skills', 'review-code');
  await mkdir(path.join(root, 'references'), { recursive: true });
  await writeFile(path.join(root, 'SKILL.md'), `---\nname: review-code\ndescription: Review code safely\n---\nInspect the diff first.\n`);
  await writeFile(path.join(root, 'references', 'checklist.md'), '# Checklist\nRun tests.\n');

  const registry = new SkillRegistry({ configDir });
  const listed = await registry.list();
  assert.equal(listed.length, 1);
  assert.equal(listed[0].name, 'review-code');
  assert.equal('instructions' in listed[0], false);

  const skill = await registry.load('review-code');
  assert.equal(skill.instructions.trim(), 'Inspect the diff first.');
  assert.deepEqual(skill.files, ['SKILL.md', 'references/checklist.md']);
  assert.match(await registry.readSkillFile('review-code', 'references/checklist.md'), /Run tests/);
  await assert.rejects(() => registry.readSkillFile('review-code', '../outside.txt'), /escapes its root/);
});

test('McpManager uses modern Streamable HTTP metadata and MCP parameter headers', async (t) => {
  const seen = [];
  const server = http.createServer(async (req, res) => {
    let body = '';
    for await (const chunk of req) body += chunk;
    const rpc = JSON.parse(body);
    seen.push({ method: rpc.method, params: rpc.params, headers: req.headers });

    const result = (() => {
      switch (rpc.method) {
        case 'tools/list':
          return { tools: [{ name: 'search', description: 'Search', inputSchema: { type: 'object', properties: { region: { type: 'string', 'x-mcp-header': 'Region' }, q: { type: 'string' } } } }] };
        case 'tools/call':
          return { content: [{ type: 'text', text: 'ok' }], structuredContent: { count: 1 }, isError: false };
        case 'resources/list':
          return { resources: [{ uri: 'file:///demo.txt', name: 'demo.txt', mimeType: 'text/plain' }] };
        case 'resources/read':
          return { contents: [{ uri: rpc.params.uri, mimeType: 'text/plain', text: 'demo' }] };
        case 'prompts/list':
          return { prompts: [{ name: 'summarize', description: 'Summarize' }] };
        case 'prompts/get':
          return { messages: [{ role: 'user', content: { type: 'text', text: 'Summarize' } }] };
        default:
          throw new Error(`unexpected ${rpc.method}`);
      }
    })();

    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ jsonrpc: '2.0', id: rpc.id, result }));
  });
  const address = await listen(server);
  t.after(() => server.close());

  const configDir = await tempConfig();
  const mcp = new McpManager({ configDir });
  await mcp.setServer({ name: 'demo', transport: 'http', url: `http://127.0.0.1:${address.port}/mcp` });

  const tools = await mcp.listTools('demo');
  assert.equal(tools[0].qualifiedName, 'demo/search');
  const call = await mcp.callQualifiedTool('demo/search', { region: '서울', q: 'hello' });
  assert.equal(call.isError, false);
  assert.deepEqual(call.structuredContent, { count: 1 });
  assert.equal((await mcp.listResources('demo'))[0].uri, 'file:///demo.txt');
  assert.equal((await mcp.readResource('demo', 'file:///demo.txt')).contents.length, 1);
  assert.equal((await mcp.listPrompts('demo'))[0].qualifiedName, 'demo/summarize');
  assert.equal((await mcp.getPrompt('demo', 'summarize')).messages.length, 1);

  const listRequest = seen.find((item) => item.method === 'tools/list');
  assert.equal(listRequest.headers['mcp-protocol-version'], '2026-07-28');
  assert.equal(listRequest.headers['mcp-method'], 'tools/list');
  assert.equal(listRequest.params._meta['io.modelcontextprotocol/protocolVersion'], '2026-07-28');

  const callRequest = seen.find((item) => item.method === 'tools/call');
  assert.equal(callRequest.headers['mcp-name'], 'search');
  assert.match(callRequest.headers['mcp-param-region'], /^=\?base64\?.+\?=$/);
  await mcp.close();
});

test('McpManager falls back to legacy initialize for stdio MCP servers', async () => {
  const configDir = await tempConfig();
  const fixture = new URL('./fixtures/legacy-mcp.mjs', import.meta.url).pathname;
  const mcp = new McpManager({ configDir });
  await mcp.setServer({ name: 'legacy', transport: 'stdio', command: process.execPath, args: [fixture] });
  const tools = await mcp.listTools('legacy');
  assert.equal(tools[0].qualifiedName, 'legacy/legacy-tool');
  const result = await mcp.callTool('legacy', 'legacy-tool');
  assert.equal(result.content[0].text, 'legacy-ok');
  const status = (await mcp.listServers())[0];
  assert.equal(status.era, 'legacy');
  assert.equal(status.protocol, '2025-11-25');
  await mcp.close();
});
