import { expect, test, type Page } from '../../../web/node_modules/@playwright/test/index.js'
import { installMockRemote } from '../../../web/tests/mockRemote'

type Hooks = {
  __yeetSent: Array<{ type: string; command?: Record<string, unknown> }>
  __yeetEmit: (message: Record<string, unknown>) => void
}
async function emit(page: Page, message: Record<string, unknown>) {
  await page.evaluate(payload => (window as unknown as Hooks).__yeetEmit(payload), { version: 1, ...message })
}
async function commands(page: Page) {
  return page.evaluate(() => (window as unknown as Hooks).__yeetSent.filter(message => message.type === 'command').map(message => message.command))
}
async function connect(page: Page) {
  await page.goto('/')
  await page.getByRole('textbox', { name: 'Remote endpoint', exact: true }).fill('http://127.0.0.1:4187')
  await page.getByRole('button', { name: 'Connect', exact: true }).click()
}

test('authentication gates the workbench and session navigation adapts to phone layout', async ({ page }, testInfo) => {
  await installMockRemote(page)
  let authenticated = false
  await page.route('**/api/auth/status', route => route.fulfill({ contentType: 'application/json', body: JSON.stringify({ required: true, authenticated, key: true, passkey: false }) }))
  await page.route('**/api/auth/key', async route => {
    expect(route.request().postDataJSON()).toEqual({ key: 'test-access-key' })
    authenticated = true
    await route.fulfill({ contentType: 'application/json', body: '{"ok":true}' })
  })
  await connect(page)
  await expect(page.getByText('auth-required', { exact: true })).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toHaveCount(0)
  await page.getByRole('textbox', { name: 'Remote authentication key', exact: true }).fill('test-access-key')
  await page.getByRole('button', { name: 'Connect', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Search sessions', exact: true })).toBeVisible()
  await page.screenshot({ path: testInfo.outputPath('desktop-workbench.png'), fullPage: true })
  await page.getByRole('button', { name: 'Protocol review', exact: true }).click()
  await expect.poll(async () => (await commands(page)).some(command => command?.type === 'load_session' && command.session_id === 'session-b')).toBe(true)
  await emit(page, { type: 'state_update', sequence: 2, revision: 2, patch: { current_session_id: 'session-b' } })
  await expect(page.getByText('Protocol review', { exact: true })).toHaveCount(2)
  await page.setViewportSize({ width: 390, height: 844 })
  await expect(page.getByRole('textbox', { name: 'Search sessions', exact: true })).toHaveCount(0)
  await page.getByRole('button', { name: 'Sessions', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Search sessions', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Close sessions', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
})

test('phone submission, streaming and permission responses use Rust semantic commands', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await installMockRemote(page)
  await connect(page)
  const input = page.getByRole('textbox', { name: 'Message', exact: true })
  await expect(input).toBeVisible()
  await input.fill('Inspect the portable client')
  await page.getByRole('button', { name: 'Send', exact: true }).click()
  await expect.poll(async () => (await commands(page)).some(command => command?.type === 'submit' && command.text === 'Inspect the portable client')).toBe(true)
  await expect(input).toHaveValue('')
  await emit(page, { type: 'conversation_entry', sequence: 2, revision: 2, entry: { id: 'native-answer', kind: { type: 'assistant', content: '' } } })
  await emit(page, { type: 'assistant_delta', sequence: 3, revision: 3, entry_id: 'native-answer', reset: true, content: 'Portable response', delta: '' })
  await emit(page, { type: 'assistant_delta', sequence: 4, revision: 4, entry_id: 'native-answer', reset: false, content: 'Portable response streamed', delta: ' streamed' })
  await expect(page.getByText('Portable response streamed', { exact: true })).toBeVisible()
  await page.screenshot({ path: testInfo.outputPath('phone-workbench.png'), fullPage: true })
  await page.getByRole('button', { name: 'Interrupt', exact: true }).click()
  await expect.poll(async () => (await commands(page)).some(command => command?.type === 'interrupt')).toBe(true)
  await emit(page, { type: 'state_update', sequence: 5, revision: 5, patch: { is_streaming: false, pending_shell_permission: { id: 'shell-check', kind: 'shell', command: 'cargo test', operation: 'execute', reason: 'Verify core behavior' } } })
  await expect(page.getByText('Permission requested', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Allow', exact: true }).click()
  await expect.poll(async () => (await commands(page)).some(command => command?.type === 'allow_shell')).toBe(true)
  await page.getByRole('button', { name: 'Deny', exact: true }).click()
  await expect.poll(async () => (await commands(page)).some(command => command?.type === 'deny_shell')).toBe(true)
})
