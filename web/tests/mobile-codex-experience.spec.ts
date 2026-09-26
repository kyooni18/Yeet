import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type Hooks = { __yeetEmit: (message: Record<string, unknown>) => void }

async function emit(page: Page, message: Record<string, unknown>) {
  await page.evaluate((payload) => (window as unknown as Hooks).__yeetEmit(payload), message)
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('mobile chrome keeps thread identity in the top bar and model state in the composer', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)

  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: {
      active_model: 'openai/gpt-5.6-sol',
      active_reasoning_level: 'high',
      current_session_id: 'session-mobile',
      saved_sessions: [{
        id: 'session-mobile',
        title: 'Fix mobile keyboard experience',
        model: 'openai/gpt-5.6-sol',
        updated_at: '2026-09-20T01:00:00.000Z',
        message_count: 12,
      }],
      workspace_session_groups: [{
        workspace_id: '/Users/test/Code/Rust/Yeet',
        sessions: [{
          id: 'session-mobile',
          title: 'Fix mobile keyboard experience',
          model: 'openai/gpt-5.6-sol',
          updated_at: '2026-09-20T01:00:00.000Z',
          message_count: 12,
        }],
      }],
    },
  })

  const topbar = page.locator('.remote-topbar')
  await expect(topbar.locator('.topbar-title')).toHaveText('Fix mobile keyboard experience')
  await expect(topbar.getByRole('button', { name: 'Open sidebar' })).toBeVisible()
  await expect(topbar.getByRole('button', { name: 'Quick settings' })).toBeVisible()
  await expect(topbar.locator('.toolbar-chip')).toHaveCount(0)

  const toolbar = page.locator('.composer-toolbar')
  await expect(toolbar).toContainText('gpt-5.6-sol')
  await expect(toolbar).toContainText(/high/i)

  const topBox = await topbar.boundingBox()
  expect(topBox).not.toBeNull()
  expect(Math.round(topBox?.height ?? 0)).toBe(56)
})

test('reading older output is not pulled to the bottom by a new entry', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)

  const conversation = Array.from({ length: 48 }, (_, index) => ({
    id: `history-${index}`,
    kind: index % 2
      ? { type: 'assistant', content: `Response ${index}: ${'working context '.repeat(12)}` }
      : { type: 'user', content: `Request ${index}: ${'older context '.repeat(8)}` },
  }))
  await emit(page, { type: 'conversation_reset', version: 1, sequence: 2, revision: 2, conversation })

  const transcript = page.locator('.conversation-scroll')
  await expect.poll(() => transcript.evaluate((element) => element.scrollHeight - element.clientHeight)).toBeGreaterThan(1000)
  await transcript.evaluate((element) => element.scrollTo({ top: 0 }))
  const before = await transcript.evaluate((element) => element.scrollTop)

  await emit(page, {
    type: 'conversation_entry', version: 1, sequence: 3, revision: 3,
    entry: { id: 'new-while-reading', kind: { type: 'assistant', content: 'New background result while you are reading.' } },
  })

  await expect(page.getByText('New background result while you are reading.')).toBeAttached()
  const after = await transcript.evaluate((element) => element.scrollTop)
  expect(Math.abs(after - before)).toBeLessThanOrEqual(2)
})

test('mobile session drawer stays narrow and dismisses outside the panel', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)

  await page.getByRole('button', { name: 'Open sidebar' }).click()
  const sidebar = page.locator('.remote-sidebar')
  await expect(sidebar).toHaveClass(/is-open/)
  await expect(sidebar.getByText('Sessions')).toBeVisible()

  const box = await sidebar.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.width).toBeGreaterThan(250)
  expect(box!.width).toBeLessThanOrEqual(300)

  const viewport = page.viewportSize()!
  await page.mouse.click(viewport.width - 6, 110)
  await expect(sidebar).not.toHaveClass(/is-open/)
})

test('mobile quick controls mirror the iOS right-side control drawer', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)

  await page.getByRole('button', { name: 'Quick settings' }).click()
  const panel = page.locator('.quick-panel')
  await expect(panel).toHaveClass(/is-open/)
  await expect(panel.getByText('Model', { exact: true })).toBeVisible()
  await expect(panel.getByText('Reasoning', { exact: true })).toBeVisible()
  await expect(panel.getByText('Context', { exact: true })).toBeVisible()

  const box = await panel.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.width).toBeGreaterThan(250)
  expect(box!.width).toBeLessThanOrEqual(300)

  await page.keyboard.press('Escape')
  await expect(panel).not.toHaveClass(/is-open/)
})
