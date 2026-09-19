import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

async function sessionSurface(page: Page) {
  const persistent = page.locator('.desktop-session-sidebar')
  if (await persistent.isVisible()) return persistent

  await page.getByTestId('open-sessions').click()
  const drawer = page.locator('.session-sidebar.is-drawer')
  await expect(drawer).toBeVisible()
  return drawer
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('workspace navigation supports arrows and Home/End without removing Tab traversal', async ({ page }) => {
  const sidebar = await sessionSurface(page)
  const list = sidebar.getByTestId('workspace-list')
  const buttons = list.locator('button:not(:disabled):visible')
  const count = await buttons.count()
  expect(count).toBeGreaterThanOrEqual(3)

  await buttons.first().focus()
  await page.keyboard.press('ArrowDown')
  await expect(buttons.nth(1)).toBeFocused()
  await page.keyboard.press('ArrowDown')
  await expect(buttons.nth(2)).toBeFocused()
  await page.keyboard.press('ArrowUp')
  await expect(buttons.nth(1)).toBeFocused()

  await page.keyboard.press('End')
  await expect(buttons.nth(count - 1)).toBeFocused()
  await page.keyboard.press('Home')
  await expect(buttons.first()).toBeFocused()

  const finePointer = await page.evaluate(() => matchMedia('(hover: hover) and (pointer: fine)').matches)
  if (finePointer) {
    await page.keyboard.press('Tab')
    await expect(buttons.first()).not.toBeFocused()
  }
})

test('keyboard navigation scrolls offscreen workspaces into view', async ({ page }) => {
  await page.evaluate(() => {
    (window as unknown as { __yeetEmit: (message: unknown) => void }).__yeetEmit({
      type: 'state_update', version: 1, sequence: 2, revision: 2,
      patch: { known_workspaces: Array.from({ length: 30 }, (_, index) => ({
        id: `workspace-${index}`, path: `/work/${index}`, display_name: `Workspace ${index}`,
        session_count: 0, is_current: false,
      })) },
    })
  })
  const sidebar = await sessionSurface(page)
  const list = sidebar.getByTestId('workspace-list')
  await list.locator('button').first().focus()
  await page.keyboard.press('End')
  const last = list.locator('button').last()
  await expect(last).toBeFocused()
  const bounds = await list.boundingBox()
  const item = await last.boundingBox()
  expect(item!.y).toBeGreaterThanOrEqual(bounds!.y)
  expect(item!.y + item!.height).toBeLessThanOrEqual(bounds!.y + bounds!.height + 1)
})
