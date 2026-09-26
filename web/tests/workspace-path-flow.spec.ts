import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type Hooks = { __yeetSent: Array<Record<string, unknown>> }

async function openSessions(page: Page) {
  await page.getByRole('button', { name: 'Open sidebar' }).click()
  const sidebar = page.locator('.remote-sidebar')
  await expect(sidebar).toBeVisible()
  return sidebar
}

async function latestWorkspaceHello(page: Page) {
  return page.evaluate(() => {
    const sent = (window as unknown as Hooks).__yeetSent
    return sent.filter((item) => item.type === 'hello').at(-1)?.workspace
  })
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('workspace picker communicates expansion and closes after choosing the current workspace', async ({ page }) => {
  const sidebar = await openSessions(page)
  const picker = sidebar.locator('.workspace-picker__button')
  const initialHello = await latestWorkspaceHello(page)

  await expect(picker).toHaveAttribute('aria-expanded', 'false')
  await picker.click()
  await expect(picker).toHaveAttribute('aria-expanded', 'true')
  await sidebar.locator('.workspace-menu').getByRole('button', { name: 'Yeet' }).click()

  await expect(picker).toHaveAttribute('aria-expanded', 'false')
  expect(await latestWorkspaceHello(page)).toBe(initialHello)
  await expect(sidebar).toBeVisible()
})

test('choosing another workspace reconnects with that workspace while leaving navigation available', async ({ page }) => {
  const sidebar = await openSessions(page)
  const picker = sidebar.locator('.workspace-picker__button')
  await picker.click()
  await sidebar.locator('.workspace-menu').getByRole('button', { name: 'AnotherProject' }).click()

  await expect.poll(() => latestWorkspaceHello(page)).toBe('/Users/test/Code/Rust/AnotherProject')
  await expect(picker).toHaveAttribute('aria-expanded', 'false')
  await expect(sidebar).toBeVisible()
})

test('Escape closes the session drawer and clears any open workspace menu', async ({ page }) => {
  const sidebar = await openSessions(page)
  const picker = sidebar.locator('.workspace-picker__button')
  await picker.click()
  await expect(picker).toHaveAttribute('aria-expanded', 'true')

  await page.keyboard.press('Escape')
  await expect(page.locator('.remote-sidebar')).not.toHaveClass(/is-open/)

  await openSessions(page)
  await expect(page.locator('.workspace-picker__button')).toHaveAttribute('aria-expanded', 'false')
})
