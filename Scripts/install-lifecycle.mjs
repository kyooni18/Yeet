#!/usr/bin/env node
// Installation owns Yeet processes only; never kill a shared Node/MCP client.
import { execFileSync, spawn } from 'node:child_process';
import { readFileSync, writeFileSync, mkdirSync, openSync, closeSync, existsSync, readdirSync, readlinkSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const names = new Set(['yeet', 'YeetRemote', 'yeet-remote']);
const sleep = ms => new Promise(r => setTimeout(r, ms));

function argv(pid) {
  if (process.platform === 'linux') {
    return readFileSync(`/proc/${pid}/cmdline`, 'utf8').split('\0').filter(Boolean);
  }
  // ps does not preserve argv boundaries for paths containing spaces on macOS.
  const program = `import ctypes,json,sys
lib=ctypes.CDLL(None,use_errno=True)
mib=(ctypes.c_int*3)(1,49,int(sys.argv[1]))
n=ctypes.c_size_t(1024*1024); b=ctypes.create_string_buffer(n.value)
if lib.sysctl(mib,3,b,ctypes.byref(n),None,0): raise OSError(ctypes.get_errno())
count=int.from_bytes(b.raw[:4],sys.byteorder); data=b.raw[4:n.value]
data=data[data.index(b'\\0')+1:].lstrip(b'\\0')
print(json.dumps([v.decode() for v in data.split(b'\\0')[:count]]))`;
  return JSON.parse(execFileSync('python3', ['-c', program, String(pid)], { encoding: 'utf8' }));
}

export function ownedProcesses(rows, uid, excluded = new Set()) {
  const owned = new Set(rows.filter(p => p.uid === uid && !excluded.has(p.pid)
    && (names.has(basename(p.executable)) || p.runtimeOwned)).map(p => p.pid));
  let changed = true;
  while (changed) {
    changed = false;
    for (const p of rows) {
      if (p.uid === uid && owned.has(p.ppid) && !owned.has(p.pid) && !excluded.has(p.pid)) {
        owned.add(p.pid); changed = true;
      }
    }
  }
  return rows.filter(p => owned.has(p.pid));
}

export function restartPlan(rows) {
  const services = [];
  let interactive = false;
  const terminals = [];
  let stdio = false;
  for (const p of rows) {
    if (!names.has(basename(p.executable))) continue;
    const args = p.argv.slice(1);
    if (args[0] === '__background-daemon') services.push({ args, cwd: args[1] });
    else if (args[0] === 'remote' || args[0] === '--remote') services.push({ args, cwd: p.cwd });
    else if (args[0] === 'mcpserver' && ['__daemon', 'run', 'start'].includes(args[1])) {
      services.push({ args: ['mcpserver', 'start', ...args.slice(2)], cwd: p.cwd });
    } else if (args[0] === 'mcpserver' && ['stdio', 'serve'].includes(args[1])) stdio = true;
    else if (p.tty && p.tty !== '?') {
      interactive = true; terminals.push({ args, cwd: p.cwd });
    }
  }
  if (!services.some(p => p.args[0] === 'mcpserver')) services.push({ args: ['mcpserver', 'start'] });
  return { services, interactive, terminals, stdio };
}

function inventory() {
  if (process.platform === 'linux') {
    const rows = [];
    for (const name of readdirSync('/proc').filter(name => /^\d+$/.test(name))) {
      try {
        const status = readFileSync(`/proc/${name}/status`, 'utf8');
        if (/^State:\s+Z/m.test(status)) continue;
        rows.push({ pid: +name, ppid: +status.match(/^PPid:\s+(\d+)/m)[1],
          uid: +status.match(/^Uid:\s+(\d+)/m)[1], tty: '?',
          executable: readlinkSync(`/proc/${name}/exe`).replace(/ \(deleted\)$/, '') });
        // Detect interactive clients without relying on procps on minimal images.
        const stdin = readlinkSync(`/proc/${name}/fd/0`);
        if (/^\/dev\/(?:pts\/|tty)/.test(stdin)) rows.at(-1).tty = stdin;
      } catch { /* Process exited, or belongs to another inaccessible user. */ }
    }
    return rows;
  }
  const lines = execFileSync('ps', ['-ww', '-axo', 'pid=,ppid=,uid=,tty=,stat=,comm='], { encoding: 'utf8' });
  const rows = [];
  for (const line of lines.split('\n')) {
    const match = line.match(/^\s*(\d+)\s+(\d+)\s+(\d+)\s+(\S+)\s+(\S+)\s+(.+)$/);
    if (!match) continue;
    const [, pid, ppid, uid, tty, stat, executable] = match;
    if (stat.startsWith('Z')) continue;
    rows.push({ pid: +pid, ppid: +ppid, uid: +uid, tty, executable: executable.trim() });
  }
  return rows;
}

function supervisors() {
  if (process.platform === 'darwin') {
    const domain = `gui/${process.getuid()}`;
    const labels = new Set(execFileSync('launchctl', ['list'], { encoding: 'utf8' })
      .split('\n').map(line => line.trim().split(/\s+/).at(-1)).filter(label => /(?:^|[.-])yeet(?:[.-]|$)/i.test(label)));
    const folder = join(process.env.HOME, 'Library', 'LaunchAgents');
    const agents = [];
    for (const file of existsSync(folder) ? readdirSync(folder) : []) {
      if (!file.endsWith('.plist')) continue;
      const path = join(folder, file);
      const plist = JSON.parse(execFileSync('plutil', ['-convert', 'json', '-o', '-', path], { encoding: 'utf8' }));
      if (labels.has(plist.Label)) { agents.push({ kind: 'launchd', name: plist.Label, path, domain }); labels.delete(plist.Label); }
    }
    if (labels.size) throw new Error(`Cannot locate LaunchAgent files for ${[...labels].join(', ')}; no processes stopped.`);
    return agents;
  }
  if (process.platform === 'linux' && existsSync('/run/systemd/system')) {
    try {
      return execFileSync('systemctl', ['--user', 'list-units', '--plain', '--no-legend', '--state=active', '--type=service'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] })
        .split('\n').map(line => line.trim().split(/\s+/)[0])
        .filter(name => /(?:^|[.-])yeet(?:[.-]|$)/i.test(name))
        .map(name => ({ kind: 'systemd', name }));
    } catch { return []; }
  }
  return [];
}

function manage(service, action) {
  if (service.kind === 'systemd') execFileSync('systemctl', ['--user', action, service.name], { stdio: 'inherit', timeout: 20000 });
  else if (action === 'stop') execFileSync('launchctl', ['bootout', `${service.domain}/${service.name}`], { stdio: 'inherit', timeout: 20000 });
  else execFileSync('launchctl', ['bootstrap', service.domain, service.path], { stdio: 'inherit', timeout: 20000 });
}

function cwd(pid) {
  if (process.platform === 'linux') {
    return readlinkSync(`/proc/${pid}/cwd`);
  }
  const out = execFileSync('lsof', ['-a', '-p', String(pid), '-d', 'cwd', '-Fn'], { encoding: 'utf8' });
  return out.split('\n').find(line => line.startsWith('n'))?.slice(1);
}

async function stop(stateFile, roots) {
  const rows = inventory();
  for (const p of rows) {
    if (p.uid !== process.getuid() || !['node', 'nodejs'].includes(basename(p.executable))) continue;
    try {
      const args = argv(p.pid);
      p.runtimeOwned = args.some(arg => roots.some(root => resolve(arg).startsWith(`${resolve(root)}/dist/`)));
    } catch { /* The process may have exited during inventory. */ }
  }
  // If invoked from a Yeet tool, do not terminate the installer or its ancestors.
  const excluded = new Set([process.pid]);
  let parent = process.ppid;
  while (parent > 1 && !excluded.has(parent)) {
    excluded.add(parent); parent = rows.find(p => p.pid === parent)?.ppid ?? 1;
  }
  const owned = ownedProcesses(rows, process.getuid(), excluded);
  for (const p of owned) {
    if (names.has(basename(p.executable))) { p.argv = argv(p.pid); p.cwd = cwd(p.pid); }
  }
  const plan = restartPlan(owned);
  plan.supervisors = supervisors();
  writeFileSync(stateFile, JSON.stringify(plan), { mode: 0o600 });
  for (const service of plan.supervisors) manage(service, 'stop');
  console.log(`Stopping ${owned.length} Yeet processes and owned runtime/MCP children…`);
  await terminateProcesses(owned);
  const survivors = ownedProcesses(inventory(), process.getuid(), excluded);
  if (survivors.length) throw new Error(`Yeet processes remain (possibly supervised): ${survivors.map(p => p.pid).join(', ')}. Stop their supervisor and rerun installation.`);
}

export async function terminateProcesses(owned) {
  // Children first, including native tools and attached MCP servers.
  const currentRows = inventory();
  for (const p of owned.toReversed()) {
    if (!currentRows.some(row => row.pid === p.pid && row.uid === p.uid && row.executable === p.executable)) continue;
    try { process.kill(p.pid, 'SIGTERM'); } catch (e) { if (e.code !== 'ESRCH') throw e; }
  }
  await sleep(1000);
  for (const p of owned.toReversed()) {
    // Check identity again before escalation; never kill a recycled PID.
    const current = inventory().find(row => row.pid === p.pid);
    if (current?.uid === p.uid && current.executable === p.executable) {
      try { process.kill(p.pid, 'SIGKILL'); } catch (e) { if (e.code !== 'ESRCH') throw e; }
    }
  }
  await sleep(250);
}

async function restart(stateFile, binary) {
  const plan = JSON.parse(readFileSync(stateFile, 'utf8'));
  const logs = join(dirname(binary), '..', 'share', 'yeet', 'install-logs');
  mkdirSync(logs, { recursive: true });
  const env = { ...process.env, YEET_RUNTIME_DIR: resolve(dirname(binary), '..', 'share', 'yeet', 'runtime') };
  delete env.YEET_SOURCE_ROOT;
  for (const service of plan.supervisors ?? []) manage(service, 'start');
  for (const [i, service] of plan.services.entries()) {
    const kind = service.args[0] === 'mcpserver' ? /mcp/i : service.args[0].includes('remote') ? /remote/i : /background/i;
    if (plan.supervisors?.some(manager => kind.test(manager.name))) continue;
    const workdir = service.cwd && existsSync(service.cwd) ? service.cwd : process.cwd();
    if (service.args[0] === 'mcpserver') {
      execFileSync(binary, service.args, { cwd: workdir, env, stdio: 'inherit', timeout: 20000 });
    } else {
      const fd = openSync(join(logs, `service-${i}.log`), 'a', 0o600);
      const child = spawn(binary, service.args, { cwd: workdir, env, detached: true, stdio: ['ignore', fd, fd] });
      await new Promise((r, reject) => { child.once('spawn', r); child.once('error', reject); });
      await sleep(500);
      if (child.exitCode !== null && child.exitCode !== 0) throw new Error(`Service failed to restart; see ${logs}/service-${i}.log`);
      child.unref(); closeSync(fd);
    }
  }
  console.log('Yeet services restarted with the replacement binary and runtime.');
  if (plan.stdio) console.log('External MCP clients must reconnect their stdio transport to the replacement binary.');
  const quote = value => `'${String(value).replaceAll("'", "'\\''")}'`;
  for (const terminal of plan.terminals ?? []) {
    const command = `cd ${quote(terminal.cwd ?? process.cwd())} && exec ${quote(binary)} ${terminal.args.map(quote).join(' ')}`;
    if (process.platform === 'darwin') {
      // Reopen a terminal rather than attempting to reuse another shell's TTY.
      execFileSync('osascript', ['-e', `tell application "Terminal" to do script ${JSON.stringify(command)}`], { stdio: 'inherit', timeout: 20000 });
    } else if (process.env.DISPLAY || process.env.WAYLAND_DISPLAY) {
      const child = spawn('x-terminal-emulator', ['-e', '/bin/sh', '-c', command], { env, detached: true, stdio: 'ignore' });
      await new Promise((r, reject) => { child.once('spawn', r); child.once('error', reject); });
      child.unref();
    } else {
      console.log(`Reopen the saved terminal session: ${command}`);
    }
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [mode, stateFile, binary] = process.argv.slice(2);
  try {
    if (mode === 'stop') await stop(stateFile, process.argv.slice(4));
    else if (mode === 'restart') await restart(stateFile, binary);
    else throw new Error('Expected stop STATE or restart STATE BINARY');
  } catch (e) { console.error(`Yeet installation lifecycle: ${e.message}`); process.exitCode = 1; }
}
