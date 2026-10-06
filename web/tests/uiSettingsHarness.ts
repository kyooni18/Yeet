/** Use the production Rust reducer as this test's UI host, never a TS replica. */
import { execFileSync, spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('../../', import.meta.url))
execFileSync('cargo', ['build', '--quiet', '--example', 'ui_settings_fixture'], { cwd: root, stdio: 'inherit' })
const binary = fileURLToPath(new URL('../../target/debug/examples/ui_settings_fixture', import.meta.url))

export function projectSettings(input: unknown): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const child = spawn(binary, [], { stdio: ['pipe', 'pipe', 'pipe'] })
    let output = ''
    let diagnostic = ''
    child.stdout.on('data', data => { output += data })
    child.stderr.on('data', data => { diagnostic += data })
    child.on('error', reject)
    child.on('close', code => {
      if (code !== 0) { reject(new Error(diagnostic || `Rust UI host exited ${code}`)); return }
      try { resolve(JSON.parse(output)) } catch (error) { reject(error) }
    })
    child.stdin.end(JSON.stringify(input) + '\n')
  })
}
