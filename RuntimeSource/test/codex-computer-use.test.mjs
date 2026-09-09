import assert from 'node:assert/strict';
import { chmod, mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { CODEX_COMPUTER_USE_RUNTIME, discoverCodexComputerUse } from '../dist/index.js';

test('discoverCodexComputerUse uses the newest bundled plugin and narrows it to the computer surface', async () => {
  const codexHome = await mkdtemp(path.join(os.tmpdir(), 'yeet-codex-home-'));
  const executable = path.join(codexHome, 'cua-node');
  const launcher = path.join(codexHome, 'launch.mjs');
  await writeFile(executable, '#!/bin/sh\nexit 0\n');
  await chmod(executable, 0o755);
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
