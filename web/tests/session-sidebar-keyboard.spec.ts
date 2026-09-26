import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type Hooks = { __yeetEmit: (message: Record<string, unknown>) => void }

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

test('session drawer follows normal keyboard traversal and Escape dismissal', async ({ page }) => {
  const sidebar = await openSessions(page)
  const workspace = sidebar.locator('.workspace-picker__button')
  await workspace.focus()
  await expect(workspace).toBeFocused()

  await page.keyboard.press('Tab')
  const focusedInside = await sidebar.evaluate((element) => element.contains(document.activeElement))
  expect(focusedInside).toBe(true)

  await page.keyboard.press('Escape')
  await expect(page.locator('.remote-sidebar')).not.toHaveClass(/is-open/)
})

test('keyboard focus can reach the end of a large scrollable workspace menu', async ({ page }) => {
  await page.evaluate(() => {
    const emit = (window as unknown as Hooks).__yeetEmit
    emit({
      type: 'state_update', version: 1, sequence: 2, revision: 2,
      patch: {
        known_workspaces: Array.from({ length: 30 }, (_, index) => ({
          id: index === 0 ? '/Users/test/Code/Rust/Yeet' : `/work/${index}`,
          path: index === 0 ? '/Users/test/Code/Rust/Yeet' : `/work/${index}`,
          display_name: index === 0 ? 'Yeet' : `Workspace ${index}`,
          session_count: 0,
          is_current: index === 0,
        })),
      },
    })
  })

  const sidebar = await openSessions(page)
  await sidebar.locator('.workspace-picker__button').click()
  const menu = sidebar.locator('.workspace-menu')
  const buttons = menu.getByRole('button')
  await expect(buttons).toHaveCount(30)

  const last = buttons.last()
  await last.focus()
  await expect(last).toBeFocused()

  const bounds = await menu.boundingBox()
  const item = await last.boundingBox()
  expect(bounds).not.toBeNull()
  expect(item).not.toBeNull()
  expect(item!.y).toBeGreaterThanOrEqual(bounds!.y)
  expect(item!.y + item!.height).toBeLessThanOrEqual(bounds!.y + bounds!.height + 1)
})
