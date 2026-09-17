import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

async function currentSessionSurface(page: Page) {
  if ((page.viewportSize()?.width ?? 1000) < 900) {
    await page.getByTestId('open-sessions').click()
    const drawer = page.getByTestId('session-drawer')
    await expect(drawer).toBeVisible()
    return { surface: drawer.locator('.session-sidebar.is-drawer'), drawer }
  }
  return { surface: page.locator('.desktop-session-sidebar'), drawer: null }
}

async function sentCommands(page: Page) {
  return page.evaluate(() => (
    (window as unknown as { __yeetSent: Array<{ command?: { type?: string; session_id?: string } }> }).__yeetSent
  ))
}

test('current-workspace saved chats are unavailable offline without dismissing navigation', async ({ page }) => {
  const { surface, drawer } = await currentSessionSurface(page)
  const target = surface.locator('[data-session-id="session-b"]')
  await expect(target).toBeEnabled()

  await page.evaluate(() => window.dispatchEvent(new Event('offline')))
  await expect(target).toBeDisabled()
  if (drawer) await expect(drawer).toBeVisible()

  await expect.poll(async () => sentCommands(page)).not.toEqual(expect.arrayContaining([
    expect.objectContaining({ command: expect.objectContaining({ type: 'load_session', session_id: 'session-b' }) }),
  ]))
})


test('activating the already-current chat does not reload or interrupt it', async ({ page }) => {
  const { surface, drawer } = await currentSessionSurface(page)
  const current = surface.locator('[data-session-id="session-a"]')
  await expect(current).toHaveAttribute('aria-current', 'page')

  await page.evaluate(() => {
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
      type: 'state_update', version: 1, sequence: 2, revision: 2,
      patch: { is_streaming: true, active_run_id: 'run-current-session' },
    })
  })
  const loadCount = () => sentCommands(page).then((items) => items.filter((item) => item.command?.type === 'load_session').length)
  const before = await loadCount()

  await current.click()

  await expect.poll(loadCount).toBe(before)
  if (drawer) await expect(drawer).toBeHidden()
})

test('successful mobile saved-chat navigation still sends and dismisses the drawer', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)

  const { surface, drawer } = await currentSessionSurface(page)
  await surface.locator('[data-session-id="session-b"]').click()

  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ command: expect.objectContaining({ type: 'load_session', session_id: 'session-b' }) }),
  ]))
  await expect(drawer!).toBeHidden()
})
