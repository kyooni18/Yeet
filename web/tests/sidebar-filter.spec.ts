import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type Hooks = { __yeetEmit: (message: Record<string, unknown>) => void }

async function emit(page: Page, message: Record<string, unknown>) {
  await page.evaluate((payload) => (window as unknown as Hooks).__yeetEmit(payload), message)
}

async function openWorkspaceMenu(page: Page) {
  await page.getByRole('button', { name: 'Open sidebar' }).click()
  const sidebar = page.locator('.remote-sidebar')
  await expect(sidebar).toBeVisible()
  const picker = sidebar.locator('.workspace-picker__button')
  await picker.click()
  const menu = sidebar.locator('.workspace-menu')
  await expect(menu).toBeVisible()
  return { sidebar, picker, menu }
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('workspace catalog stays hidden until explicitly opened', async ({ page }) => {
  await page.getByRole('button', { name: 'Open sidebar' }).click()
  const sidebar = page.locator('.remote-sidebar')
  await expect(sidebar.locator('.workspace-menu')).toHaveCount(0)
  await expect(sidebar.locator('.workspace-picker__button')).toContainText('Yeet')
})

test('large workspace catalogs use a bounded scrollable menu', async ({ page }) => {
  const workspaces = Array.from({ length: 30 }, (_, index) => ({
    id: index === 0 ? '/Users/test/Code/Rust/Yeet' : `/work/${index}`,
    path: index === 0 ? '/Users/test/Code/Rust/Yeet' : `/work/${index}`,
    display_name: index === 0 ? 'Yeet' : `Workspace ${index}`,
    updated_at: '2026-09-24T00:00:00.000Z',
    session_count: index % 4,
    is_current: index === 0,
  }))
  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: { known_workspaces: workspaces },
  })

  const { menu } = await openWorkspaceMenu(page)
  const buttons = menu.getByRole('button')
  await expect(buttons).toHaveCount(30)

  const scroll = await menu.evaluate((element) => ({
    overflowY: getComputedStyle(element).overflowY,
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
  }))
  expect(['auto', 'scroll']).toContain(scroll.overflowY)
  expect(scroll.scrollHeight).toBeGreaterThan(scroll.clientHeight)

  const last = buttons.last()
  await last.focus()
  await expect(last).toBeFocused()
  const menuBox = await menu.boundingBox()
  const lastBox = await last.boundingBox()
  expect(menuBox).not.toBeNull()
  expect(lastBox).not.toBeNull()
  expect(lastBox!.y).toBeGreaterThanOrEqual(menuBox!.y)
  expect(lastBox!.y + lastBox!.height).toBeLessThanOrEqual(menuBox!.y + menuBox!.height + 1)
})
