import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

async function visibleSidebar(page: Page) {
  if ((page.viewportSize()?.width ?? 1000) < 900) {
    await page.getByTestId('open-sessions').click()
    const drawer = page.getByTestId('session-drawer')
    await expect(drawer).toBeVisible()
    return { sidebar: drawer.locator('.session-sidebar.is-drawer'), drawer }
  }
  return { sidebar: page.locator('.desktop-session-sidebar'), drawer: null }
}

async function sentCommands(page: Page) {
  return page.evaluate(() => (
    (window as unknown as { __yeetSent: Array<{ type?: string; command?: { type?: string } }> }).__yeetSent
  ))
}

test('New chat is unavailable offline without dismissing session navigation', async ({ page }) => {
  const { sidebar, drawer } = await visibleSidebar(page)
  const newChat = sidebar.getByTestId('new-session')
  await expect(newChat).toBeEnabled()

  await page.evaluate(() => window.dispatchEvent(new Event('offline')))
  await expect(newChat).toBeDisabled()

  if (drawer) await expect(drawer).toBeVisible()
  await expect.poll(async () => sentCommands(page)).not.toEqual(expect.arrayContaining([
    expect.objectContaining({ command: expect.objectContaining({ type: 'new_session' }) }),
  ]))
})

test('successful mobile New chat still sends once and dismisses the session drawer', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)

  const { sidebar, drawer } = await visibleSidebar(page)
  await sidebar.getByTestId('new-session').click()

  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ command: expect.objectContaining({ type: 'new_session' }) }),
  ]))
  await expect(drawer!).toBeHidden()
})
