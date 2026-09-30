import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, writeFileSync, readFileSync, existsSync, rmSync, readlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ownedProcesses, restartPlan, terminateProcesses } from './install-lifecycle.mjs';

test('shutdown includes owned MCP descendants, excludes other users and installer ancestors', () => {
  const rows = [
    { pid: 1, ppid: 0, uid: 5, executable: '/bin/yeet' },
    { pid: 2, ppid: 1, uid: 5, executable: 'node' },
    { pid: 3, ppid: 2, uid: 5, executable: 'external-mcp' },
    { pid: 4, ppid: 0, uid: 5, executable: 'node' },
    { pid: 6, ppid: 0, uid: 9, executable: '/bin/yeet' },
    { pid: 7, ppid: 1, uid: 5, executable: 'node' },
  ];
  assert.deepEqual(ownedProcesses(rows, 5, new Set([7])).map(p => p.pid), [1, 2, 3]);
});

test('build installation stages before shutdown, replaces stale runtime, then restarts', { skip: process.platform === 'win32' }, () => {
  const dir = mkdtempSync(join(tmpdir(), 'yeet-install-fixture-'));
  const root = join(dir, 'source');
  const prefix = join(dir, 'prefix');
  const commands = join(dir, 'commands');
  const log = join(dir, 'events');
  const put = (path, text, mode = 0o644) => { mkdirSync(join(path, '..'), { recursive: true }); writeFileSync(path, text, { mode }); };
  try {
    put(join(root, 'Cargo.toml'), 'fixture');
    put(join(root, 'RuntimeSource/package.json'), '{}');
    put(join(root, 'RuntimeSource/dist/bridge.js'), 'new runtime');
    put(join(root, 'RuntimeSource/skills/example/SKILL.md'), 'new skill');
    put(join(root, 'web/package.json'), '{}');
    put(join(root, 'Scripts/install-lifecycle.mjs'), 'fixture helper');
    copyFileSync(new URL('../install.sh', import.meta.url), join(root, 'install.sh'));
    put(join(prefix, 'bin/yeet'), '#!/bin/sh\necho old\n', 0o755);
    put(join(prefix, 'share/yeet/runtime/dist/stale.js'), 'stale');
    put(join(commands, 'npm'), '#!/bin/sh\necho build-npm >> "$TEST_LOG"\n', 0o755);
    put(join(commands, 'cargo'), '#!/bin/sh\necho build-cargo >> "$TEST_LOG"\nmkdir -p target/release\nprintf "#!/bin/sh\\necho new-binary\\n" > target/release/yeet\nchmod +x target/release/yeet\n', 0o755);
    put(join(commands, 'node'), `#!/bin/sh
case "$1" in
  */install-lifecycle.mjs) echo "$2" >> "$TEST_LOG"; [ "$2" != stop ] || echo '{}' > "$3" ;;
  *) exec "${process.execPath}" "$@" ;;
esac
`, 0o755);
    execFileSync('/bin/sh', [join(root, 'install.sh'), '--build'], {
      env: { ...process.env, PATH: `${commands}:${process.env.PATH}`, PREFIX: prefix, YEET_PREFIX: prefix, TEST_LOG: log, YEET_NO_PATH_UPDATE: '1' },
      stdio: 'pipe',
    });
    const events = readFileSync(log, 'utf8').trim().split('\n');
    assert.ok(events.indexOf('build-cargo') < events.indexOf('stop'));
    assert.ok(events.indexOf('stop') < events.indexOf('restart'));
    assert.equal(readFileSync(join(prefix, 'share/yeet/runtime/dist/bridge.js'), 'utf8'), 'new runtime');
    assert.equal(existsSync(join(prefix, 'share/yeet/runtime/dist/stale.js')), false);
    assert.match(readFileSync(join(prefix, 'bin/yeet'), 'utf8'), /new-binary/);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('shutdown signals the selected processes and preserves unrelated processes', { skip: process.platform === 'win32' }, async () => {
  const selected = spawn('/bin/sleep', ['60'], { stdio: 'ignore' });
  const unrelated = spawn('/bin/sleep', ['60'], { stdio: 'ignore' });
  const exited = new Promise(resolve => selected.once('exit', resolve));
  try {
    await new Promise((resolve, reject) => { selected.once('spawn', resolve); selected.once('error', reject); });
    await terminateProcesses([{ pid: selected.pid, uid: process.getuid(), executable: process.platform === 'linux' ? readlinkSync(`/proc/${selected.pid}/exe`) : '/bin/sleep' }]);
    await exited;
    assert.equal(selected.signalCode, 'SIGTERM');
    process.kill(unrelated.pid, 0);
  } finally { selected.kill(); unrelated.kill(); }
});

test('restart keeps exact arguments and paths, replaces MCP daemon mode with startup', () => {
  const plan = restartPlan([
    { executable: '/old/yeet', argv: ['yeet', '__background-daemon', '/work with spaces', 'scope'], tty: '?' },
    { executable: '/old/yeet', argv: ['yeet', 'remote', '--bind', '0.0.0.0:7331', '--origin', 'https://example.com'], tty: '?' },
    { executable: '/old/yeet', argv: ['yeet', 'mcpserver', '__daemon', '--port', '7442'], tty: '?' },
    { executable: '/old/yeet', argv: ['yeet', 'mcpserver', 'stdio'], tty: '?' },
    { executable: '/old/yeet', argv: ['yeet'], tty: 'ttys001' },
  ]);
  assert.equal(plan.services[0].cwd, '/work with spaces');
  assert.deepEqual(plan.services[0].args.slice(1), ['/work with spaces', 'scope']);
  assert.deepEqual(plan.services[2].args, ['mcpserver', 'start', '--port', '7442']);
  assert.equal(plan.interactive, true);
  assert.equal(plan.stdio, true);
});

test('additional PATH installations receive the same binary and complete runtime', () => {
  const dir = mkdtempSync(join(tmpdir(), 'yeet-copy-fixture-'));
  try {
    const primary = join(dir, 'primary/bin/yeet');
    const alternate = join(dir, 'alternate/bin/yeet');
    for (const path of [primary, alternate]) mkdirSync(join(path, '..'), { recursive: true });
    writeFileSync(primary, 'new binary', { mode: 0o755 });
    writeFileSync(alternate, 'old binary', { mode: 0o755 });
    const primaryRuntime = join(dir, 'primary/share/yeet/runtime');
    const alternateRuntime = join(dir, 'alternate/share/yeet/runtime');
    mkdirSync(primaryRuntime, { recursive: true }); mkdirSync(alternateRuntime, { recursive: true });
    writeFileSync(join(primaryRuntime, 'bridge.js'), 'new runtime');
    writeFileSync(join(alternateRuntime, 'stale.js'), 'old runtime');
    const state = join(dir, 'state.json');
    writeFileSync(state, JSON.stringify({ installations: [primary, alternate] }));
    execFileSync(process.execPath, [new URL('./install-lifecycle.mjs', import.meta.url).pathname, 'replace-copies', state, primary]);
    assert.equal(readFileSync(alternate, 'utf8'), 'new binary');
    assert.equal(readFileSync(join(alternateRuntime, 'bridge.js'), 'utf8'), 'new runtime');
    assert.equal(existsSync(join(alternateRuntime, 'stale.js')), false);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
