import { expect, test } from '@playwright/test'
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('../../', import.meta.url))
let directory = '', workspace = '', origin = '', log = ''
let server: ChildProcess | undefined
const frames: string[] = []

test.beforeAll(async () => {
  execFileSync('npm', ['run', 'build'], { cwd: join(root, 'web'), stdio: 'inherit' })
  execFileSync('cargo', ['build', '--quiet', '--bin', 'yeet'], {
    cwd: root, env: { ...process.env, YEET_EXPERIMENTAL_WEBUI: '1' }, stdio: 'inherit', timeout: 150_000,
  })
  // macOS's per-user temp directory exceeds the Unix socket path limit.
  directory = mkdtempSync(join(process.platform === 'darwin' ? '/tmp' : tmpdir(), 'yeet-rf-'))
  workspace = join(directory, 'workspace')
  mkdirSync(workspace)
  mkdirSync(join(directory, 'config'))
  writeFileSync(join(directory, 'config/config.json'), JSON.stringify({ model: 'openai/gpt-5' }))
  const listener = createServer()
  await new Promise<void>(resolve => listener.listen(0, '127.0.0.1', resolve))
  const port = (listener.address() as { port: number }).port
  await new Promise<void>(resolve => listener.close(() => resolve()))
  origin = `http://127.0.0.1:${port}`
  server = spawn(join(root, 'target/debug/yeet'), ['remote', '--bind', `127.0.0.1:${port}`], {
    cwd: workspace, env: { ...process.env, YEET_CONFIG_DIR: join(directory, 'config'), YEET_RUNTIME_DIR: join(root, 'RuntimeSource') },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  server.stdout?.on('data', data => { log += data })
  server.stderr?.on('data', data => { log += data })
  await expect.poll(async () => {
    try { return (await fetch(`${origin}/api/protocol`)).status } catch { return 0 }
  }, { timeout: 20_000, message: 'isolated Remote server starts' }).toBe(200)
})
test.afterEach(async ({ page }, testInfo) => {
  if (testInfo.status !== testInfo.expectedStatus) {
    await testInfo.attach('server-log', { body: log + '\nFrames: ' + frames.join('\n'), contentType: 'text/plain' })
    await testInfo.attach('visible-state', { body: await page.locator('body').innerText(), contentType: 'text/plain' })
  }
})

test.afterAll(async () => {
  if (server && server.exitCode === null) {
    const exited = new Promise(resolve => server!.once('exit', resolve))
    server.kill('SIGTERM')
    await Promise.race([exited, new Promise(resolve => setTimeout(resolve, 3000))])
  }
  // These daemon PIDs belong exclusively to the private temp config above;
  // never stop or modify the developer's running Yeet/background service.
  if (directory) {
    const background = join(directory, 'config/background')
    try {
      for (const file of readdirSync(background).filter(file => file.endsWith('.pid'))) {
        const pid = Number(readFileSync(join(background, file), 'utf8').trim())
        if (Number.isSafeInteger(pid) && pid > 1) { try { process.kill(pid, 'SIGTERM') } catch { /* already stopped */ } }
      }
    } catch { /* no daemon was started */ }
    rmSync(directory, { recursive: true, force: true })
  }
})

test('real Rust Remote persists settings, custom providers and scheduled tasks across reload', async ({ page }, testInfo) => {
  const pageErrors: string[] = []
  page.on('pageerror', error => pageErrors.push(error.message))
  page.on('websocket', socket => {
    socket.on('framesent', event => {
      if (typeof event.payload === 'string' && JSON.parse(event.payload).type === 'ui_settings_action') frames.push(event.payload)
    })
    socket.on('framereceived', event => {
      if (typeof event.payload !== 'string') return
      const message = JSON.parse(event.payload)
      frames.push(['error', 'ui_settings'].includes(message.type) ? event.payload : message.type)
    })
    socket.on('socketerror', error => frames.push(error))
  })
  await page.route('https://static.cloudflareinsights.com/**', route => route.fulfill({ body: '', contentType: 'application/javascript' }))
  await page.addInitScript(workspace => sessionStorage.setItem('yeet.remote.resume.v1', JSON.stringify({ workspace, clientId: 'feature-smoke' })), workspace)
  await page.goto(origin)
  await expect(page.getByRole('textbox', { name: 'Message', exact: true }), log).toBeVisible()
  await expect(page.getByRole('button', { name: 'Switch workspace', exact: true })).toBeEnabled({ timeout: 30_000 }).catch(async error => {
    await testInfo.attach('server-log', { body: log + '\nFrames: ' + frames.join('\n'), contentType: 'text/plain' })
    throw error
  })
  await page.locator('#sidebar').getByRole('button', { name: 'Settings', exact: true }).click()
  let dialog = page.getByRole('dialog', { name: 'Settings', exact: true })
  await dialog.getByRole('button', { name: 'Dark', exact: true }).click()
  await expect(page.locator('html')).toHaveAttribute('data-appearance', 'dark')
  await dialog.getByRole('button', { name: 'Dark theme', exact: true }).click()
  await dialog.getByRole('textbox', { name: 'Dark theme', exact: true }).fill('quiet-night')
  await dialog.getByRole('button', { name: 'Save', exact: true }).click()
  await expect.poll(() => page.locator('html').evaluate(element => element.style.getPropertyValue('--bg'))).toBe('#111113')
  await expect(dialog.getByRole('textbox')).toHaveCount(0)
  await dialog.getByRole('button', { name: 'Context window', exact: true }).click()
  await dialog.getByRole('textbox', { name: 'Context window' }).fill('128k')
  await dialog.getByRole('button', { name: 'Save', exact: true }).click()
  await expect(dialog.getByRole('textbox', { name: 'Context window' })).toHaveCount(0)
  await dialog.getByRole('button', { name: 'Custom providers', exact: true }).click()
  dialog = page.getByRole('dialog', { name: 'Custom providers', exact: true })
  await dialog.getByLabel('Provider ID', { exact: true }).fill('smoke-test')
  await dialog.getByLabel('Base URL', { exact: true }).fill('http://127.0.0.1:9/v1')
  await dialog.getByRole('checkbox', { name: 'Requires API key' }).uncheck()
  await dialog.getByRole('button', { name: 'Save provider', exact: true }).click()
  await expect(dialog.getByText('smoke-test', { exact: true })).toBeVisible()
  await dialog.getByRole('button', { name: 'Done', exact: true }).click()

  await page.locator('#sidebar').getByRole('button', { name: 'Scheduled tasks', exact: true }).click()
  await page.getByRole('button', { name: 'New task', exact: true }).click()
  await page.getByLabel('Task name', { exact: true }).fill('Future smoke test')
  await page.getByLabel('Task prompt', { exact: true }).fill('Never run during this test')
  await page.getByLabel('Task model', { exact: true }).fill('openai/gpt-5')
  await page.getByLabel('Task schedule', { exact: true }).selectOption('at')
  await page.getByLabel('Task date and time', { exact: true }).fill('2099-01-01T12:00')
  await page.getByRole('button', { name: 'Save task', exact: true }).click()
  await expect(page.getByText('Task updated', { exact: true })).toBeVisible()
  await page.reload()
  await expect(page.locator('html')).toHaveAttribute('data-appearance', 'dark')
  await expect.poll(() => page.locator('html').evaluate(element => element.style.getPropertyValue('--bg'))).toBe('#111113')
  expect(JSON.parse(readFileSync(join(directory, 'config/config.json'), 'utf8')).contextLengthOverrides['openai/gpt-5']).toBe(128000)
  await page.locator('#sidebar').getByRole('button', { name: 'Scheduled tasks', exact: true }).click()
  await expect(page.getByText('Future smoke test', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Pause task', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Enable task', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Delete task…', exact: true }).click()
  await page.getByRole('button', { name: 'Confirm delete task', exact: true }).click()
  await expect(page.getByText('Future smoke test', { exact: true })).toHaveCount(0)

  const secondWorkspace = join(directory, 'second-workspace')
  mkdirSync(secondWorkspace)
  await page.getByRole('button', { name: 'Switch workspace', exact: true }).click()
  await page.getByRole('button', { name: 'Open another workspace…', exact: true }).click()
  dialog = page.getByRole('dialog', { name: 'Open workspace', exact: true })
  await dialog.getByRole('textbox', { name: 'Workspace folder', exact: true }).fill(secondWorkspace)
  await dialog.getByRole('button', { name: 'Open workspace', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Switch workspace', exact: true })).toContainText('second-workspace')
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  expect(pageErrors).toEqual([])
})
