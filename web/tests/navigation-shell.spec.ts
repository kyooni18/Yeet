import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type Hooks = {
  __yeetSent: Array<Record<string, unknown>>
}

async function sentCommands(page: Page) {
  return page.evaluate(() => (window as unknown as Hooks).__yeetSent
    .filter((item) => item.type === 'command')
    .map((item) => item.command as Record<string, unknown>))
}

async function openSessions(page: Page) {
  await page.getByRole('button', { name: 'Open sidebar' }).click()
  const sidebar = page.locator('.remote-sidebar')
  await expect(sidebar).toBeVisible()
  return sidebar
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('session navigation uses an overlay drawer on touch and a persistent dock on desktop', async ({ page }) => {
  await expect(page.getByRole('button', { name: 'Open sidebar' })).toBeVisible()

  const sidebar = await openSessions(page)
  await expect(sidebar.getByText('Remote WebUI')).toBeVisible()
  await expect(sidebar.getByText('Protocol review')).toBeVisible()
  await expect(sidebar.locator('[data-session-id="session-a"]')).toHaveAttribute('aria-current', 'page')
})

test('selecting a saved chat sends load_session and preserves desktop navigation', async ({ page }) => {
  const sidebar = await openSessions(page)
  await sidebar.locator('[data-session-id="session-b"]').click()

  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'load_session', session_id: 'session-b' }),
  ]))
  const desktopDocked = await page.evaluate(() =>
    matchMedia('(min-width: 1000px) and (hover: hover) and (pointer: fine)').matches
  )
  if (desktopDocked) await expect(sidebar).toHaveClass(/is-open/)
  else await expect(sidebar).not.toHaveClass(/is-open/)
})

test('workspace picker switches the Remote transport without replacing the session drawer', async ({ page }) => {
  const sidebar = await openSessions(page)
  const picker = sidebar.locator('.workspace-picker__button')

  await expect(picker).toHaveAttribute('aria-expanded', 'false')
  await picker.click()
  await expect(picker).toHaveAttribute('aria-expanded', 'true')

  await sidebar.locator('.workspace-menu').getByRole('button', { name: 'AnotherProject' }).click()

  await expect.poll(async () => page.evaluate(() => {
    const sent = (window as unknown as Hooks).__yeetSent
    return sent.filter((item) => item.type === 'hello').at(-1)?.workspace
  })).toBe('/Users/test/Code/Rust/AnotherProject')
  await expect(picker).toHaveAttribute('aria-expanded', 'false')
  await expect(sidebar).toBeVisible()
})
