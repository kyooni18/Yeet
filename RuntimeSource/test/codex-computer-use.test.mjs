import assert from 'node:assert/strict';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { CODEX_COMPUTER_USE_RUNTIME, CodexComputerUse, discoverCodexComputerUse } from '../dist/index.js';

test('discoverCodexComputerUse uses the newest bundled plugin and narrows it to the computer surface', async () => {
  const codexHome = await mkdtemp(path.join(os.tmpdir(), 'yeet-codex-home-'));
  const executable = process.execPath;
  const launcher = path.join(codexHome, 'launch.mjs');
  await writeFile(launcher, '// launcher\n');

  const base = path.join(codexHome, 'plugins', 'cache', 'openai-bundled', 'unified-computer-use');
  for (const version of ['26.9.1', '26.10.2']) {
    const root = path.join(base, version);
    await mkdir(root, { recursive: true });
    await writeFile(path.join(root, '.mcp.json'), JSON.stringify({
      mcpServers: {
        cua_repl: {
          command: executable,
          args: [launcher],
          enabled: true,
          enabled_tools: ['js', 'js_reset'],
          env: { CUA_REPL_ENABLED_SURFACES: 'browser,computer', CODEX_HOME: '/stale/home' },
        },
      },
    }));
  }

  const installation = await discoverCodexComputerUse({ codexHome });
  assert.equal(installation.pluginVersion, '26.10.2');
  assert.equal(installation.configuration.name, CODEX_COMPUTER_USE_RUNTIME);
  assert.equal(installation.configuration.transport, 'stdio');
  assert.equal(installation.configuration.env.CUA_REPL_ENABLED_SURFACES, 'computer');
  assert.equal(installation.configuration.env.CODEX_HOME, codexHome);
});

test('CodexComputerUse bounds a wedged call and resets only its runtime transport', async () => {
  const codexHome = await mkdtemp(path.join(os.tmpdir(), 'yeet-codex-timeout-'));
  const pluginRoot = path.join(
    codexHome,
    'plugins',
    'cache',
    'openai-bundled',
    'unified-computer-use',
    '26.10.2',
  );
  await mkdir(pluginRoot, { recursive: true });
  const launcher = path.join(codexHome, 'launch.mjs');
  await writeFile(launcher, '// launcher\n');
  await writeFile(path.join(pluginRoot, '.mcp.json'), JSON.stringify({
    mcpServers: {
      cua_repl: {
        command: process.execPath,
        args: [launcher],
        enabled: true,
        enabled_tools: ['js', 'js_reset'],
      },
    },
  }));

  let disconnects = 0;
  const mcp = {
    async setRuntimeServer() {},
    async callTool() {
      return await new Promise(() => {});
    },
    async disconnect(name) {
      assert.equal(name, CODEX_COMPUTER_USE_RUNTIME);
      disconnects += 1;
    },
  };
  const computerUse = new CodexComputerUse(mcp, { codexHome });
  const outcome = await Promise.race([
    computerUse.call('js', { code: 'await new Promise(() => {})', timeout_ms: 25 })
      .then(() => ({ kind: 'resolved' }), (error) => ({ kind: 'rejected', error })),
    new Promise((resolve) => setTimeout(() => resolve({ kind: 'hung' }), 250)),
  ]);

  assert.notEqual(outcome.kind, 'hung', 'Computer Use must not wait for the coarse outer MCP supervisor');
  assert.equal(outcome.kind, 'rejected');
  assert.match(String(outcome.error), /timed out after 25ms/i);
  assert.equal(disconnects, 1);
});
