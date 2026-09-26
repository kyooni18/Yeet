import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
  __yeetSent: Array<Record<string, unknown>>
}

async function emitState(page: Page, sequence: number, patch: Record<string, unknown>) {
  await page.evaluate(({ sequence, patch }) => {
    ;(window as unknown as TestHooks).__yeetEmit({
      type: 'state_update',
      version: 1,
      sequence,
      revision: sequence,
      patch,
    })
  }, { sequence, patch })
}

async function sentCommands(page: Page) {
  return page.evaluate(() => (window as unknown as TestHooks).__yeetSent
    .filter((message) => message.type === 'command')
    .map((message) => message.command as Record<string, unknown>))
}

async function openSessions(page: Page) {
  await page.getByRole('button', { name: 'Open sidebar' }).click()
  const sidebar = page.locator('.remote-sidebar')
  await expect(sidebar).toHaveClass(/is-open/)
  return sidebar
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('drafts stay with their session instead of following navigation', async ({ page }) => {
  const composer = page.getByRole('textbox', { name: 'Message' })

  await composer.fill('unfinished note for session A')
  await emitState(page, 2, { current_session_id: 'session-b' })
  await expect(composer).toHaveValue('')

  await composer.fill('different note for session B')
  await emitState(page, 3, { current_session_id: 'session-a' })
  await expect(composer).toHaveValue('unfinished note for session A')

  await emitState(page, 4, { current_session_id: 'session-b' })
  await expect(composer).toHaveValue('different note for session B')
})

test('draft identity includes the workspace even when session ids collide', async ({ page }) => {
  const composer = page.getByRole('textbox', { name: 'Message' })

  await composer.fill('Yeet workspace draft')
  await emitState(page, 2, {
    workspace_root: '/Users/test/Code/Rust/AnotherProject',
    current_session_id: 'session-a',
  })
  await expect(composer).toHaveValue('')

  await composer.fill('AnotherProject draft')
  await emitState(page, 3, {
    workspace_root: '/Users/test/Code/Rust/Yeet',
    current_session_id: 'session-a',
  })
  await expect(composer).toHaveValue('Yeet workspace draft')

  await emitState(page, 4, {
    workspace_root: '/Users/test/Code/Rust/AnotherProject',
    current_session_id: 'session-a',
  })
  await expect(composer).toHaveValue('AnotherProject draft')
})

test('sending clears only the active context draft', async ({ page }) => {
  const composer = page.getByRole('textbox', { name: 'Message' })

  await composer.fill('keep this in session A')
  await emitState(page, 2, { current_session_id: 'session-b' })
  await expect(composer).toHaveValue('')
  await composer.fill('send from session B')
  await page.getByRole('button', { name: 'Send' }).click()
  await expect(composer).toHaveValue('')
  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'submit', text: 'send from session B' }),
  ]))

  await emitState(page, 3, { current_session_id: 'session-a' })
  await expect(composer).toHaveValue('keep this in session A')
})

test('real sidebar session navigation swaps drafts', async ({ page }) => {
  const composer = page.getByRole('textbox', { name: 'Message' })
  await composer.fill('draft that belongs to session A')

  let sidebar = await openSessions(page)
  await sidebar.locator('.sidebar-session').filter({ hasText: 'Protocol review' }).click()
  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'load_session', session_id: 'session-b' }),
  ]))
  await emitState(page, 2, { current_session_id: 'session-b' })
  await expect(composer).toHaveValue('')

  await composer.fill('draft that belongs to session B')
  sidebar = await openSessions(page)
  await sidebar.locator('.sidebar-session').filter({ hasText: 'Remote WebUI' }).click()
  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'load_session', session_id: 'session-a' }),
  ]))
  await emitState(page, 3, { current_session_id: 'session-a' })
  await expect(composer).toHaveValue('draft that belongs to session A')
})

test('connection loss and reconnect do not disturb the active draft', async ({ page }) => {
  const composer = page.getByRole('textbox', { name: 'Message' })
  await composer.fill('keep while reconnecting')

  await page.evaluate(() => window.dispatchEvent(new Event('offline')))
  await expect(composer).toHaveValue('keep while reconnecting')
  await expect(composer).toBeEnabled()

  await page.evaluate(() => window.dispatchEvent(new Event('online')))
  await expect(composer).toHaveValue('keep while reconnecting')
})

test('starting another unsaved chat clears the previous unsent draft', async ({ page }) => {
  const composer = page.getByRole('textbox', { name: 'Message' })
  await emitState(page, 2, { current_session_id: null })
  await expect(composer).toHaveValue('')
  await composer.fill('abandoned unsaved draft')

  const sidebar = await openSessions(page)
  await sidebar.getByRole('button', { name: 'New session' }).click()
  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'new_session' }),
  ]))
  await expect(composer).toHaveValue('')

  await emitState(page, 3, { current_session_id: null, conversation: [] })
  await expect(composer).toHaveValue('')
})

test('opening Settings and returning keeps the active draft', async ({ page }) => {
  const composer = page.getByRole('textbox', { name: 'Message' })
  await composer.fill('draft survives settings')

  const sidebar = await openSessions(page)
  await sidebar.getByRole('button', { name: 'Settings' }).click()
  const settings = page.getByRole('dialog', { name: 'Settings' })
  await expect(settings).toBeVisible()
  await settings.getByRole('button', { name: 'Close' }).click()
  await expect(settings).toHaveCount(0)

  await expect(composer).toHaveValue('draft survives settings')
})
