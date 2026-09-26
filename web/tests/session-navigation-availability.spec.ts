import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type Hooks = {
  __yeetSent: Array<{ command?: { type?: string; session_id?: string } }>
}

async function openSessions(page: Page) {
  await page.getByRole('button', { name: 'Open sidebar' }).click()
  const sidebar = page.locator('.remote-sidebar')
  await expect(sidebar).toBeVisible()
  return sidebar
}

async function sentCommands(page: Page) {
  return page.evaluate(() => (window as unknown as Hooks).__yeetSent)
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('saved chats are unavailable offline without dismissing the drawer', async ({ page }) => {
  const sidebar = await openSessions(page)
  const target = sidebar.locator('[data-session-id="session-b"]')
  await expect(target).toBeEnabled()

  await page.evaluate(() => window.dispatchEvent(new Event('offline')))
  await expect(target).toBeDisabled()
  await expect(sidebar).toBeVisible()

  await expect.poll(async () => sentCommands(page)).not.toEqual(expect.arrayContaining([
    expect.objectContaining({ command: expect.objectContaining({ type: 'load_session', session_id: 'session-b' }) }),
  ]))
})

test('activating the already-current chat does not reload it', async ({ page }) => {
  const sidebar = await openSessions(page)
  const current = sidebar.locator('[data-session-id="session-a"]')
  await expect(current).toHaveAttribute('aria-current', 'page')

  const loadCount = () => sentCommands(page).then((items) =>
    items.filter((item) => item.command?.type === 'load_session').length
  )
  const before = await loadCount()
  await current.click()

  await expect.poll(loadCount).toBe(before)
  const desktopDocked = await page.evaluate(() =>
    matchMedia('(min-width: 1000px) and (hover: hover) and (pointer: fine)').matches
  )
  if (desktopDocked) await expect(sidebar).toHaveClass(/is-open/)
  else await expect(sidebar).not.toHaveClass(/is-open/)
})

test('saved-chat navigation sends once and keeps the PC dock available', async ({ page }) => {
  const sidebar = await openSessions(page)
  await sidebar.locator('[data-session-id="session-b"]').click()

  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ command: expect.objectContaining({ type: 'load_session', session_id: 'session-b' }) }),
  ]))
  const desktopDocked = await page.evaluate(() =>
    matchMedia('(min-width: 1000px) and (hover: hover) and (pointer: fine)').matches
  )
  if (desktopDocked) await expect(sidebar).toHaveClass(/is-open/)
  else await expect(sidebar).not.toHaveClass(/is-open/)
})
