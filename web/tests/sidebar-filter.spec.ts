import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
}

const matchingSession = {
  id: 'session-search-target',
  title: 'Telemetry migration notes',
  updated_at: '2026-09-14T07:20:00.000Z',
  model: 'gpt-5.6-sol',
  message_count: 12,
}

const knownWorkspaces = [
  { id: '/Users/test/Code/Rust/Yeet', path: '/Users/test/Code/Rust/Yeet', display_name: 'Yeet', updated_at: '2026-09-14T07:25:00.000Z', session_count: 7, is_current: true },
  { id: '/Users/test/Code/Rust/AnotherProject', path: '/Users/test/Code/Rust/AnotherProject', display_name: 'AnotherProject', updated_at: '2026-09-14T07:20:00.000Z', session_count: 1, is_current: false },
  { id: '/Users/test/Code/Rust/SkylineTools', path: '/Users/test/Code/Rust/SkylineTools', display_name: 'SkylineTools', updated_at: '2026-09-14T07:10:00.000Z', session_count: 1, is_current: false },
  { id: '/Users/test/Code/Swift/Calcite', path: '/Users/test/Code/Swift/Calcite', display_name: 'Calcite', updated_at: '2026-09-14T07:00:00.000Z', session_count: 1, is_current: false },
]

async function emit(page: Page, message: Record<string, unknown>) {
  await page.evaluate((payload) => {
    ;(window as unknown as TestHooks).__yeetEmit(payload)
  }, message)
}

async function sessionSurface(page: Page) {
  if ((page.viewportSize()?.width ?? 0) >= 900) return page.locator('.desktop-session-sidebar')
  await page.getByTestId('open-sessions').click()
  return page.getByTestId('session-drawer')
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('large workspace catalogs expose search without adding clutter to small catalogs', async ({ page }) => {
  const surface = await sessionSurface(page)
  await expect(surface.getByTestId('workspace-filter')).toHaveCount(0)

  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: {
      known_workspaces: knownWorkspaces,
      workspace_session_groups: [
        { workspace_id: '/Users/test/Code/Rust/AnotherProject', sessions: [matchingSession] },
      ],
    },
  })

  const filter = surface.getByTestId('workspace-filter')
  await expect(filter).toBeVisible()
  const filterBox = await filter.boundingBox()
  expect(filterBox).not.toBeNull()
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  if (touchFirst) expect(filterBox?.height ?? 0).toBeGreaterThanOrEqual(44)
  else expect(filterBox?.height ?? 999).toBeLessThan(44)
})

test('filter reveals matching hidden chats and restores normal disclosure behavior when cleared', async ({ page }) => {
  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: {
      known_workspaces: knownWorkspaces,
      workspace_session_groups: [
        { workspace_id: '/Users/test/Code/Rust/AnotherProject', sessions: [matchingSession] },
      ],
    },
  })

  const surface = await sessionSurface(page)
  const filter = surface.getByTestId('workspace-filter')
  const targetWorkspace = surface.locator('[data-workspace-id="/Users/test/Code/Rust/AnotherProject"]')

  await expect(targetWorkspace.getByText(matchingSession.title)).toBeHidden()
  await expect(targetWorkspace.getByTestId('workspace-session-toggle')).toHaveAttribute('aria-expanded', 'false')

  await filter.fill('telemetry')
  await expect(surface.getByTestId('workspace-item')).toHaveCount(1)
  await expect(targetWorkspace).toBeVisible()
  await expect(targetWorkspace.getByText(matchingSession.title)).toBeVisible()
  await expect(targetWorkspace.getByTestId('workspace-session-toggle')).toHaveCount(0)

  await filter.fill('anotherproject')
  await expect(targetWorkspace).toBeVisible()
  await expect(targetWorkspace.getByText(matchingSession.title)).toBeVisible()

  await filter.fill('does-not-exist')
  await expect(surface.getByTestId('workspace-filter-empty')).toHaveText('No matching workspaces or chats.')

  await filter.press('Escape')
  await expect(filter).toHaveValue('')
  if ((page.viewportSize()?.width ?? 0) < 900) await expect(surface).toBeVisible()
  await expect(surface.getByTestId('workspace-item')).toHaveCount(4)
  await expect(targetWorkspace.getByText(matchingSession.title)).toBeHidden()
  await expect(targetWorkspace.getByTestId('workspace-session-toggle')).toHaveAttribute('aria-expanded', 'false')

  if ((page.viewportSize()?.width ?? 0) < 900) {
    await filter.press('Escape')
    await expect(surface).toBeHidden()
  }
})
