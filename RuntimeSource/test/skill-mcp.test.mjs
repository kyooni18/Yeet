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

  const registry = new SkillRegistry({ configDir, includeCodexSkills: false });
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

test('SkillRegistry layers project over user skills and reads explicit-only agent metadata', async () => {
  const configDir = await tempConfig();
  const projectRoot = await tempConfig();
  const userRoot = path.join(configDir, 'skills', 'shared');
  const projectSkill = path.join(projectRoot, '.yeet', 'skills', 'shared');
  await mkdir(userRoot, { recursive: true });
  await mkdir(path.join(projectSkill, 'agents'), { recursive: true });
  await writeFile(path.join(userRoot, 'SKILL.md'), `---\nname: shared\ndescription: User copy\n---\nuser\n`);
  await writeFile(path.join(projectSkill, 'SKILL.md'), `---\nname: shared\ndescription: Project copy\nmetadata:\n  short-description: Project short\n---\nproject\n`);
  await writeFile(path.join(projectSkill, 'agents', 'openai.yaml'), `interface:\n  short_description: UI short\npolicy:\n  allow_implicit_invocation: false\n`);

  const registry = new SkillRegistry({ configDir, projectRoot, includeCodexSkills: false });
  const listed = await registry.list();
  assert.equal(listed.length, 1);
  assert.equal(listed[0].description, 'Project copy');
  assert.equal(listed[0].shortDescription, 'UI short');
  assert.equal(listed[0].allowImplicitInvocation, false);
  assert.equal(listed[0].source, 'project');
  assert.equal((await registry.load('shared')).instructions.trim(), 'project');
});

test('SkillRegistry accepts legacy lowercase skill.md repositories and can install them', async () => {
  const configDir = await tempConfig();
  const source = await tempConfig();
  await mkdir(path.join(source, 'docs'), { recursive: true });
  await mkdir(path.join(source, 'pdfs'), { recursive: true });
  await writeFile(path.join(source, 'docs', 'skill.md'), '# DOCX workflow\nUse the document workflow.\n');
  await writeFile(path.join(source, 'pdfs', 'skill.md'), '# PDF workflow\nUse the PDF workflow.\n');

  const registry = new SkillRegistry({ configDir, projectRoot: source, includeCodexSkills: false });
  const validated = await registry.validate(source);
  assert.deepEqual(validated.map((item) => item.name), ['docs', 'pdfs']);
  const installed = await registry.install(source);
  assert.deepEqual(installed.installed, ['docs', 'pdfs']);
  assert.match((await registry.load('docs')).instructions, /DOCX workflow/);
  assert.equal(await registry.remove('docs'), true);
  assert.equal(await registry.remove('docs'), false);
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
  assert.deepEqual(result.feedback, { status: 'completed' });
  const status = (await mcp.listServers())[0];
  assert.equal(status.era, 'legacy');
  assert.equal(status.protocol, '2025-11-25');
  await mcp.close();
});

test('McpManager can host an in-memory runtime without persisting or listing it as user MCP', async () => {
  const configDir = await tempConfig();
  const fixture = new URL('./fixtures/legacy-mcp.mjs', import.meta.url).pathname;
  const mcp = new McpManager({ configDir });
  await mcp.setRuntimeServer({ name: 'runtime-only', transport: 'stdio', command: process.execPath, args: [fixture] });
  assert.deepEqual(await mcp.listServers(), []);
  const tools = await mcp.listTools('runtime-only');
  assert.equal(tools[0].qualifiedName, 'runtime-only/legacy-tool');
  const result = await mcp.callTool('runtime-only', 'legacy-tool');
  assert.equal(result.content[0].text, 'legacy-ok');
  assert.equal(await mcp.removeRuntimeServer('runtime-only'), true);
  await assert.rejects(() => mcp.listTools('runtime-only'), /Unknown MCP server/);
  await mcp.close();
});

test('McpManager restarts stdio before legacy fallback when modern discovery exits the server', async () => {
  const configDir = await tempConfig();
  const fixture = new URL('./fixtures/legacy-exit-on-discover.mjs', import.meta.url).pathname;
  const mcp = new McpManager({ configDir });
  await mcp.setServer({ name: 'legacy-exit', transport: 'stdio', command: process.execPath, args: [fixture] });
  const tools = await mcp.listTools('legacy-exit');
  assert.equal(tools[0].qualifiedName, 'legacy-exit/legacy-exit-tool');
  const result = await mcp.callTool('legacy-exit', 'legacy-exit-tool');
  assert.equal(result.content[0].text, 'legacy-exit-ok');
  const status = (await mcp.listServers())[0];
  assert.equal(status.era, 'legacy');
  assert.equal(status.protocol, '2025-11-25');
  await mcp.close();
});

test('McpManager forwards native-app elicitation and accepts only an explicit session approval', async () => {
  const configDir = await tempConfig();
  const fixture = new URL('./fixtures/native-approval.mjs', import.meta.url).pathname;
  let request;
  let resolveApproval;
  const approvalSeen = new Promise((resolve) => { resolveApproval = resolve; });
  const mcp = new McpManager({
    configDir,
    nativeAppApproval: async (value) => {
      request = value;
      resolveApproval();
      return new Promise((resolve) => { resolveApproval = () => resolve(true); });
    },
  });
  await mcp.setServer({ name: 'native', transport: 'stdio', command: process.execPath, args: [fixture] });
  const call = mcp.callTool('native', 'open');
  await approvalSeen;
  assert.equal(request.bundleId, 'org.blenderfoundation.blender');
  assert.equal(request.appName, 'Blender');
  assert.equal(request.server, 'native');
  assert.equal(request.tool, 'open');
  resolveApproval();
  const response = await call;
  const elicitation = JSON.parse(response.content[0].text);
  assert.equal(elicitation.action, 'accept');
  assert.equal(elicitation.scope, 'session');
  assert.equal('_meta' in elicitation, false);
  await mcp.close();
});

test('McpManager accepts the current Codex _meta native-app elicitation shape', async () => {
  const configDir = await tempConfig();
  const fixture = new URL('./fixtures/native-approval.mjs', import.meta.url).pathname;
  let request;
  const mcp = new McpManager({
    configDir,
    nativeAppApproval: async (value) => {
      request = value;
      return true;
    },
  });
  await mcp.setServer({
    name: 'native-modern',
    transport: 'stdio',
    command: process.execPath,
    args: [fixture],
    env: { NATIVE_APPROVAL_SHAPE: 'modern' },
  });
  const response = await mcp.callTool('native-modern', 'open');
  const elicitation = JSON.parse(response.content[0].text);
  assert.equal(request.bundleId, 'org.blenderfoundation.blender');
  assert.equal(request.appName, 'Blender');
  assert.equal(request.tool, 'get_app_state');
  assert.equal(elicitation.action, 'accept');
  assert.equal(elicitation.scope, 'session');
  await mcp.close();
});

test('McpManager declines native-app elicitation when the host denies it', async () => {
  const configDir = await tempConfig();
  const fixture = new URL('./fixtures/native-approval.mjs', import.meta.url).pathname;
  const mcp = new McpManager({ configDir, nativeAppApproval: async () => false });
  await mcp.setServer({ name: 'native-deny', transport: 'stdio', command: process.execPath, args: [fixture] });
  const response = await mcp.callTool('native-deny', 'open');
  assert.equal(JSON.parse(response.content[0].text).action, 'decline');
  await mcp.close();
});

test('McpManager declines a native-app elicitation when the call is cancelled or the connection closes', async () => {
  const configDir = await tempConfig();
  const fixture = new URL('./fixtures/native-approval.mjs', import.meta.url).pathname;
  let approvalSeen;
  const seen = new Promise((resolve) => { approvalSeen = resolve; });
  const mcp = new McpManager({
    configDir,
    nativeAppApproval: async (_request, signal) => new Promise((resolve) => {
      approvalSeen();
      signal?.addEventListener('abort', () => resolve(false), { once: true });
    }),
  });
  await mcp.setServer({ name: 'native-cancel', transport: 'stdio', command: process.execPath, args: [fixture] });
  const controller = new AbortController();
  const call = mcp.callTool('native-cancel', 'open', {}, controller.signal);
  await seen;
  controller.abort(new Error('cancelled by test'));
  await assert.rejects(call, /cancel/i);
  await mcp.close();
});

test('McpManager declines unsupported elicitation without invoking the approval host', async () => {
  const configDir = await tempConfig();
  const fixture = new URL('./fixtures/native-approval.mjs', import.meta.url).pathname;
  const mcp = new McpManager({
    configDir,
    nativeAppApproval: async () => { throw new Error('approval must not be requested'); },
  });
  await mcp.setServer({ name: 'native-unsupported', transport: 'stdio', command: process.execPath, args: [fixture], env: { NATIVE_APPROVAL_KIND: 'unsupported' } });
  const response = await mcp.callTool('native-unsupported', 'open');
  assert.equal(JSON.parse(response.content[0].text).action, 'decline');
  await mcp.close();
});

test('McpManager resolves an in-flight approval as denied when its stdio connection closes', async () => {
  const configDir = await tempConfig();
  const fixture = new URL('./fixtures/native-approval.mjs', import.meta.url).pathname;
  let seenResolve;
  const seen = new Promise((resolve) => { seenResolve = resolve; });
  const mcp = new McpManager({
    configDir,
    nativeAppApproval: async (_request, signal) => new Promise((resolve) => {
      seenResolve();
      signal?.addEventListener('abort', () => resolve(false), { once: true });
    }),
  });
  await mcp.setServer({ name: 'native-close', transport: 'stdio', command: process.execPath, args: [fixture] });
  const call = mcp.callTool('native-close', 'open');
  await seen;
  await mcp.disconnect('native-close');
  await assert.rejects(call, /disconnect|cancel/i);
  await mcp.close();
});
