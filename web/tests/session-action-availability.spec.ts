import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type Hooks = {
  __yeetSent: Array<{ command?: { type?: string } }>
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

test('New session is unavailable offline without dismissing navigation', async ({ page }) => {
  const sidebar = await openSessions(page)
  const newSession = sidebar.getByRole('button', { name: 'New session' })
  await expect(newSession).toBeEnabled()

  await page.evaluate(() => window.dispatchEvent(new Event('offline')))
  await expect(newSession).toBeDisabled()
  await expect(sidebar).toBeVisible()
  await expect(page.getByRole('button', { name: 'New chat' })).toBeDisabled()

  await expect.poll(async () => sentCommands(page)).not.toEqual(expect.arrayContaining([
    expect.objectContaining({ command: expect.objectContaining({ type: 'new_session' }) }),
  ]))
})

test('New session sends once and keeps desktop navigation available', async ({ page }) => {
  const sidebar = await openSessions(page)
  const before = (await sentCommands(page)).filter((item) => item.command?.type === 'new_session').length

  await sidebar.getByRole('button', { name: 'New session' }).click()

  await expect.poll(async () =>
    (await sentCommands(page)).filter((item) => item.command?.type === 'new_session').length
  ).toBe(before + 1)

  const desktopDocked = await page.evaluate(() =>
    matchMedia('(min-width: 1000px) and (hover: hover) and (pointer: fine)').matches
  )
  if (desktopDocked) await expect(sidebar).toHaveClass(/is-open/)
  else await expect(sidebar).not.toHaveClass(/is-open/)
})
