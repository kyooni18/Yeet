import { cpSync, existsSync, mkdirSync, rmSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const desktop = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const source = resolve(desktop, '../../RuntimeSource')
const target = resolve(desktop, 'src-tauri/runtime')
const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm'
const result = spawnSync(npm, ['run', 'build'], { cwd: source, stdio: 'inherit', shell: process.platform === 'win32' })
if (result.error) throw result.error
if (result.status !== 0) process.exit(result.status ?? 1)
if (!existsSync(resolve(source, 'dist/bridge.js'))) throw new Error('Runtime build did not produce dist/bridge.js')
rmSync(target, { recursive: true, force: true })
mkdirSync(target, { recursive: true })
for (const entry of ['dist', 'skills', 'package.json']) {
  cpSync(resolve(source, entry), resolve(target, entry), { recursive: true })
}
console.log('Prepared existing Yeet runtime assets for the desktop host.')
