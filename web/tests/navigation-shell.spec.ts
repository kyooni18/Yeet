import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetSent: Array<Record<string, unknown>>
  __yeetEmit: (message: Record<string, unknown>) => void
}

async function emit(page: Page, message: Record<string, unknown>) {
  await page.evaluate((payload) => {
    ;(window as unknown as TestHooks).__yeetEmit(payload)
  }, message)
}

async function sentCommands(page: Page) {
  return page.evaluate(() => (window as unknown as TestHooks).__yeetSent
    .filter((item) => item.type === 'command')
    .map((item) => item.command as Record<string, unknown>))
}

const anotherSession = {
  id: 'session-c',
  title: 'Another project session',
  updated_at: '2026-09-09T20:00:00.000Z',
  model: 'gpt-5.6-sol',
  message_count: 6,
}

const manySessions = Array.from({ length: 7 }, (_, index) => ({
  id: `session-many-${index}`,
  title: `Session ${index + 1}`,
  updated_at: `2026-09-09T${String(20 - index).padStart(2, '0')}:00:00.000Z`,
  model: 'gpt-5.6-sol',
  message_count: index + 1,
}))

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('session navigation switches cleanly between persistent sidebar and drawer trigger', async ({ page }) => {
  const compact = (page.viewportSize()?.width ?? 0) < 900
  const sidebar = page.locator('.desktop-session-sidebar')
  const trigger = page.getByTestId('open-sessions')

  if (compact) {
    await expect(sidebar).toBeHidden()
    await expect(trigger).toBeVisible()
  } else {
    await expect(sidebar).toBeVisible()
    await expect(trigger).toBeHidden()
  }
})

test('desktop catalog exposes inactive workspace sessions and carries direct session intent across reconnect', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 0) < 900)

  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: {
      workspace_session_groups: [
        { workspace_id: '/Users/test/Code/Rust/AnotherProject', sessions: [anotherSession] },
      ],
    },
  })

  const sidebar = page.locator('.desktop-session-sidebar')
  const otherWorkspace = sidebar.locator('[data-workspace-id="/Users/test/Code/Rust/AnotherProject"]')
  await expect(otherWorkspace.getByText('Another project session')).toBeHidden()
  const disclosure = otherWorkspace.getByTestId('workspace-session-toggle')
  await expect(disclosure).toHaveAttribute('aria-label', 'Show chats for AnotherProject')
  await expect(disclosure).toHaveAttribute('aria-expanded', 'false')
  await disclosure.click()
  await expect(disclosure).toHaveAttribute('aria-label', 'Collapse chats for AnotherProject')
  await expect(disclosure).toHaveAttribute('aria-expanded', 'true')
  await expect(otherWorkspace.getByText('Another project session')).toBeVisible()
  await otherWorkspace.getByTestId('session-item').click()

  await expect.poll(async () => page.evaluate(() => {
    const hellos = (window as unknown as TestHooks).__yeetSent.filter((item) => item.type === 'hello')
    return hellos.at(-1)?.workspace
  })).toBe('/Users/test/Code/Rust/AnotherProject')

  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: { saved_sessions: [anotherSession], current_session_id: 'session-c' },
  })

  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'load_session', session_id: 'session-c' }),
  ]))
})

test('desktop current workspace previews recent chats and keeps the selected chat visible', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 0) < 900)

  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: { saved_sessions: manySessions, current_session_id: 'session-many-6' },
  })

  const activeWorkspace = page.locator('.desktop-session-sidebar .workspace-group.active')
  const sessions = activeWorkspace.getByTestId('session-item')
  await expect(sessions).toHaveCount(4)
  await expect(activeWorkspace.getByText('Session 7')).toBeVisible()

  const disclosure = activeWorkspace.getByTestId('workspace-session-toggle')
  await expect(disclosure).toHaveAttribute('aria-expanded', 'false')
  await disclosure.click()
  await expect(disclosure).toHaveAttribute('aria-expanded', 'true')
  await expect(sessions).toHaveCount(7)
  await disclosure.click()
  await expect(sessions).toHaveCount(4)
  await expect(activeWorkspace.getByText('Session 7')).toBeVisible()
})

test('desktop activity is a docked region with a keyboard-resizable separator', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 0) < 1200)

  await page.getByTestId('toggle-inspector').click()
  const conversation = page.locator('.conversation-pane')
  const inspector = page.getByTestId('activity-inspector')
  const handle = page.getByTestId('inspector-resize-handle')
  await expect(inspector).toBeVisible()
  await expect(handle).toBeVisible()

  const conversationBox = await conversation.boundingBox()
  const initialInspectorBox = await inspector.boundingBox()
  expect(conversationBox).not.toBeNull()
  expect(initialInspectorBox).not.toBeNull()
  expect((conversationBox?.x ?? 0) + (conversationBox?.width ?? 0)).toBeLessThanOrEqual((initialInspectorBox?.x ?? 0) + 1)

  await handle.focus()
  await handle.press('ArrowLeft')
  await expect.poll(async () => (await inspector.boundingBox())?.width ?? 0)
    .toBeGreaterThan((initialInspectorBox?.width ?? 0) + 10)
})

test('narrow phone keeps workspace catalog and activity inside touch-safe drawers', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)

  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: {
      workspace_session_groups: [
        { workspace_id: '/Users/test/Code/Rust/AnotherProject', sessions: [anotherSession] },
      ],
    },
  })

  await page.getByTestId('open-sessions').click()
  const drawer = page.getByTestId('session-drawer')
  await expect(drawer).toBeVisible()
  const otherWorkspace = drawer.locator('[data-workspace-id="/Users/test/Code/Rust/AnotherProject"]')
  await expect(otherWorkspace.getByText('Another project session')).toBeHidden()
  const disclosure = otherWorkspace.getByRole('button', { name: 'Show chats for AnotherProject' })
  const disclosureBox = await disclosure.boundingBox()
  expect(disclosureBox?.width ?? 0).toBeGreaterThanOrEqual(40)
  expect(disclosureBox?.height ?? 0).toBeGreaterThanOrEqual(40)
  await disclosure.click()
  await expect(otherWorkspace.getByText('Another project session')).toBeVisible()
  const sessionBox = await drawer.getByText('Another project session').locator('..').boundingBox()
  expect(sessionBox?.height ?? 0).toBeGreaterThanOrEqual(44)
  expect(sessionBox?.height ?? 999).toBeLessThanOrEqual(46)
  await page.getByRole('button', { name: 'Close sessions' }).click()

  await page.evaluate(() => {
    document.documentElement.style.setProperty('--visual-viewport-top', '28px')
    document.documentElement.style.setProperty('--visual-viewport-height', '300px')
  })
  await page.getByTestId('toggle-inspector').click()
  const inspector = page.getByTestId('activity-inspector')
  await expect(inspector).toBeVisible()
  const inspectorBox = await inspector.boundingBox()
  const viewport = page.viewportSize()
  expect(inspectorBox).not.toBeNull()
  expect(viewport).not.toBeNull()
  expect(inspectorBox?.x ?? -1).toBeGreaterThanOrEqual(8)
  expect(inspectorBox?.y ?? 0).toBeGreaterThanOrEqual(34)
  expect(inspectorBox?.y ?? 999).toBeLessThanOrEqual(48)
  expect((inspectorBox?.x ?? 0) + (inspectorBox?.width ?? 0)).toBeLessThanOrEqual((viewport?.width ?? 0) - 8)
  expect((inspectorBox?.y ?? 0) + (inspectorBox?.height ?? 0)).toBeLessThanOrEqual(328)
  await expect(page.getByTestId('inspector-resize-handle')).toBeHidden()
})
