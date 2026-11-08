import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

async function navigation(page: Page) {
  const toggle = page.getByRole('button', { name: 'Toggle sidebar', exact: true })
  // During an exit animation the old sidebar is still visible, but not open.
  if (await toggle.count() && await toggle.getAttribute('aria-expanded') === 'false') {
    await expect(page.locator('#sidebar')).toHaveCount(0)
    await toggle.click()
  }
  await expect(page.locator('#sidebar')).toBeVisible()
  return page.locator('#sidebar')
}
async function settings(page: Page, destination?: string) {
  await (await navigation(page)).getByRole('button', { name: 'Settings', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Settings', exact: true })
  if (destination) {
    await dialog.getByRole('button', { name: destination, exact: true }).click()
    await expect(page.locator('[data-sheet-dialog]')).toHaveAttribute('aria-label', destination === 'Manage capabilities' ? 'Capabilities' : destination)
  }
  return destination ? page.locator('[data-sheet-dialog]') : dialog
}
async function commands(page: Page) {
  return page.evaluate(() => (window as unknown as { __yeetSent: Array<{ command?: Record<string, unknown> }> }).__yeetSent.map(frame => frame.command).filter(Boolean))
}
async function patch(page: Page, value: Record<string, unknown>, sequence = 2) {
  await page.evaluate(({ value, sequence }) => (window as unknown as { __yeetEmit: (message: unknown) => void }).__yeetEmit({ type: 'state_update', version: 1, sequence, revision: sequence, patch: value }), { value, sequence })
}
test.beforeEach(async ({ page }) => {
  await page.route('https://static.cloudflareinsights.com/**', route => route.fulfill({ body: '' }))
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('settings destinations submit real provider, capability and guarded sandbox commands; sheets fit and trap focus', async ({ page }) => {
  let dialog = await settings(page, 'Custom providers')
  await expect(dialog).toHaveAttribute('aria-label', 'Custom providers')
  await dialog.getByLabel('Provider ID', { exact: true }).fill('test-provider')
  await dialog.getByLabel('Base URL', { exact: true }).fill('https://example.test/v1')
  await dialog.getByRole('button', { name: 'Save provider' }).click()
  await expect.poll(() => commands(page)).toContainEqual({ type: 'save_provider', id: 'test-provider', base_url: 'https://example.test/v1', require_api_key: true })
  await dialog.getByRole('button', { name: 'Done' }).click()
  await expect(dialog).toHaveCount(0)

  dialog = await settings(page, 'Authentication')
  await dialog.getByLabel('openai API key').fill('test-secret-not-a-real-key')
  await dialog.getByRole('button', { name: 'Save key' }).click()
  await expect(dialog.getByLabel('openai API key')).toHaveValue('')
  await expect.poll(() => commands(page)).toContainEqual({ type: 'auth_set_api_key', provider: 'openai', key: 'test-secret-not-a-real-key' })
  await dialog.getByRole('button', { name: 'Done' }).click()

  dialog = await settings(page, 'Manage capabilities')
  await dialog.getByLabel('Review', { exact: true }).click()
  await expect.poll(() => commands(page)).toContainEqual({ type: 'toggle_capability', id: 'skill:review' })
  await dialog.getByRole('button', { name: 'Done' }).click()

  dialog = await settings(page, 'Sandbox & permissions')
  await dialog.getByLabel('Sandbox preset', { exact: true }).selectOption('unlimited')
  expect((await commands(page)).some(command => command?.type === 'update_sandbox')).toBe(false)
  await dialog.getByRole('button', { name: 'Enable unlimited', exact: true }).click()
  await expect.poll(() => commands(page)).toContainEqual({ type: 'update_sandbox', action: { type: 'apply_preset', preset: 'unlimited' } })
  const bounds = await dialog.boundingBox()
  expect(bounds!.x).toBeGreaterThanOrEqual(0)
  expect(bounds!.y).toBeGreaterThanOrEqual(0)
  expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(page.viewportSize()!.height + 1)
  await expect(page.locator('#main')).toHaveAttribute('inert', '')
  await dialog.getByRole('button', { name: 'Done' }).focus()
  await page.keyboard.press('Shift+Tab')
  await expect(dialog.getByRole('button', { name: 'Reset policy…' })).toBeFocused()
  await page.keyboard.press('Tab')
  await expect(dialog.getByRole('button', { name: 'Done' })).toBeFocused()
  await page.keyboard.press('Escape')
  await expect(dialog).toHaveCount(0)
  await expect(page.locator('#main')).not.toHaveAttribute('inert', '')
})

test('scheduled tasks save typed schedules, retain drafts until server confirmation, and open results', async ({ page }) => {
  await (await navigation(page)).getByRole('button', { name: 'Scheduled tasks', exact: true }).click()
  await expect.poll(() => commands(page)).toContainEqual({ type: 'request_tasks' })
  await page.getByRole('button', { name: 'New task', exact: true }).click()
  await page.getByLabel('Task name', { exact: true }).fill('Daily review')
  await page.getByLabel('Task prompt', { exact: true }).fill('Review recent changes')
  await page.getByLabel('Interval in seconds', { exact: true }).fill('86400')
  await page.getByRole('button', { name: 'Save task', exact: true }).click()
  await expect.poll(async () => (await commands(page)).some(command => command?.type === 'manage_task')).toBe(true)
  const command = (await commands(page)).find(command => command?.type === 'manage_task')!
  expect(command).toMatchObject({ action: { action: 'save', id: null, draft: { name: 'Daily review', prompt: 'Review recent changes', schedule: { kind: 'interval', seconds: 86400 } } } })
  await expect(page.getByLabel('Task prompt', { exact: true })).toHaveValue('Review recent changes')
  await expect(page.getByRole('button', { name: 'Saving…' })).toBeDisabled()
  await patch(page, { tasks_response_id: command.request_id, tasks_notice: 'Task updated', scheduled_tasks: [{ id: 't1', draft: (command.action as { draft: unknown }).draft, enabled: true, running: false, next_run: null, last_run: null, last_result: 'Completed review', session_id: 'result-session' }] })
  await expect(page.getByRole('button', { name: 'Save task', exact: true })).toBeEnabled()
  await expect(page.getByText('Task updated', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Open result session', exact: true }).click()
  await expect.poll(() => commands(page)).toContainEqual({ type: 'load_session', session_id: 'result-session' })
})

test('agent creation uses the group reducer, and debate sends role-specific models', async ({ page }) => {
  await (await navigation(page)).getByRole('button', { name: 'Agents', exact: true }).click()
  await page.getByRole('button', { name: 'Create group', exact: true }).click()
  await page.getByLabel('Group objective', { exact: true }).fill('Inspect the project')
  await page.getByRole('button', { name: 'Create group', exact: true }).last().click()
  await expect.poll(() => commands(page)).toContainEqual({ type: 'create_agent_group', objective: 'Inspect the project' })
  expect((await commands(page)).some(command => command?.type === 'submit')).toBe(false)
  await (await navigation(page)).getByRole('button', { name: 'Debate', exact: true }).click()
  await page.getByLabel('Debate topic', { exact: true }).fill('Which design is more accessible?')
  await page.getByLabel('Pro model', { exact: true }).fill('provider/pro')
  await page.getByLabel('Con model', { exact: true }).fill('provider/con')
  await page.getByLabel('Jury model', { exact: true }).fill('provider/jury')
  await page.getByRole('button', { name: 'Start debate', exact: true }).click()
  await expect.poll(() => commands(page)).toContainEqual({ type: 'start_debate', topic: 'Which design is more accessible?', models: { pro: 'provider/pro', con: 'provider/con', jury: 'provider/jury' } })
})

test('session management renames and moves saved sessions and confirms deletion', async ({ page }) => {
  await (await navigation(page)).getByRole('button', { name: 'Manage Remote WebUI', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Manage session', exact: true })
  await expect(dialog.getByLabel('Session title')).toHaveValue('Remote WebUI')
  await dialog.getByLabel('Session title').fill('Renamed')
  await dialog.getByRole('button', { name: 'Rename session', exact: true }).click()
  await expect.poll(() => commands(page)).toContainEqual({ type: 'rename_session', session_id: 'session-a', title: 'Renamed' })
  await dialog.getByLabel('Destination workspace').fill('/work/other')
  await dialog.getByRole('button', { name: 'Move session', exact: true }).click()
  await expect.poll(() => commands(page)).toContainEqual({ type: 'move_session', session_id: 'session-a', workspace_path: '/work/other' })
  await dialog.getByRole('button', { name: 'Delete session…', exact: true }).click()
  expect((await commands(page)).some(command => command?.type === 'delete_session')).toBe(false)
  await dialog.getByRole('button', { name: 'Keep session', exact: true }).click()
  await dialog.getByRole('button', { name: 'Delete session…', exact: true }).click()
  await dialog.getByRole('button', { name: 'Permanently delete', exact: true }).click()
  await expect.poll(() => commands(page)).toContainEqual({ type: 'delete_session', session_id: 'session-a' })
})
