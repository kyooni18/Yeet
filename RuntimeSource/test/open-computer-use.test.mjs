import assert from 'node:assert/strict';
import { mkdir, mkdtemp, writeFile, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { OPEN_COMPUTER_USE_RUNTIME, OpenComputerUse, discoverOpenComputerUse } from '../dist/index.js';

async function makePackage(configDir) {
  const root = path.join(configDir, 'computer-use', 'node_modules', 'open-computer-use');
  await mkdir(path.join(root, 'scripts', 'node-repl'), { recursive: true });
  await mkdir(path.join(root, 'bin'));
  await writeFile(path.join(root, 'package.json'), JSON.stringify({ name: 'open-computer-use', version: '1.1.2' }));
  for (const file of ['bin/open-computer-use', 'scripts/node-repl/open-computer-use-repl.mjs', 'scripts/node-repl/open-computer-use-kernel.mjs']) {
    await writeFile(path.join(root, file), '// fixture\n');
  }
  return root;
}

test('discovers only the Yeet-managed package and launches the upstream JS adapter without shell shims', async (t) => {
  const configDir = await mkdtemp(path.join(os.tmpdir(), 'yeet-ocu-'));
  t.after(() => rm(configDir, { recursive: true, force: true }));
  await assert.rejects(discoverOpenComputerUse({ configDir }), /yeet install computer-use/);
  const root = await makePackage(configDir);
  const installation = await discoverOpenComputerUse({ configDir });
  assert.equal(installation.version, '1.1.2');
  assert.deepEqual(installation.configuration, {
    name: OPEN_COMPUTER_USE_RUNTIME,
    transport: 'stdio',
    command: process.execPath,
    args: [path.join(root, 'scripts', 'node-repl', 'open-computer-use-repl.mjs'), '--', process.execPath, path.join(root, 'bin', 'open-computer-use'), 'mcp'],
  });
  await rm(path.join(root, 'scripts', 'node-repl', 'open-computer-use-kernel.mjs'));
  await assert.rejects(discoverOpenComputerUse({ configDir }), /yeet install computer-use/);
});

test('OpenComputerUse bounds a wedged call and resets only its runtime transport', async (t) => {
  const configDir = await mkdtemp(path.join(os.tmpdir(), 'yeet-codex-timeout-'));
  t.after(() => rm(configDir, { recursive: true, force: true }));
  await makePackage(configDir);

  let disconnects = 0;
  const mcp = {
    async setRuntimeServer() {},
    async callTool() {
      return await new Promise(() => {});
    },
    async disconnect(name) {
      assert.equal(name, OPEN_COMPUTER_USE_RUNTIME);
      disconnects += 1;
    },
  };
  const computerUse = new OpenComputerUse(mcp, { configDir });
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
