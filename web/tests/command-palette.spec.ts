import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type Hooks = {
  __yeetSent: Array<{ command?: { type?: string; session_id?: string } }>
}

async function commands(page: Page) {
  return page.evaluate(() => (window as unknown as Hooks).__yeetSent)
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('the iOS-style sidebar is the primary session switcher', async ({ page }) => {
  await page.getByRole('button', { name: 'Open sidebar' }).click()

  const sidebar = page.locator('.remote-sidebar')
  await expect(sidebar).toHaveClass(/is-open/)
  await expect(sidebar.getByText('Sessions')).toBeVisible()
  await expect(sidebar.getByText('Remote WebUI')).toBeVisible()
  await expect(sidebar.getByText('Protocol review')).toBeVisible()
  await expect(page.locator('.main-viewport')).toHaveClass(/sidebar-open/)
  await expect(page.getByTestId('command-palette')).toHaveCount(0)
})

test('selecting a sidebar session sends load_session and keeps the desktop dock available', async ({ page }) => {
  await page.getByRole('button', { name: 'Open sidebar' }).click()
  const sidebar = page.locator('.remote-sidebar')

  await sidebar.locator('.sidebar-session').filter({ hasText: 'Protocol review' }).click()

  await expect.poll(async () => (await commands(page)).some((message) =>
    message.command?.type === 'load_session' && message.command?.session_id === 'session-b'
  )).toBe(true)

  const desktopDocked = await page.evaluate(() =>
    matchMedia('(min-width: 1000px) and (hover: hover) and (pointer: fine)').matches
  )
  if (desktopDocked) await expect(sidebar).toHaveClass(/is-open/)
  else await expect(sidebar).not.toHaveClass(/is-open/)
})

test('new session remains a direct sidebar action and preserves the desktop dock', async ({ page }) => {
  await page.getByRole('button', { name: 'Open sidebar' }).click()
  const sidebar = page.locator('.remote-sidebar')
  await page.getByRole('button', { name: 'New session' }).click()

  await expect.poll(async () => (await commands(page)).some((message) =>
    message.command?.type === 'new_session'
  )).toBe(true)

  const desktopDocked = await page.evaluate(() =>
    matchMedia('(min-width: 1000px) and (hover: hover) and (pointer: fine)').matches
  )
  if (desktopDocked) await expect(sidebar).toHaveClass(/is-open/)
  else await expect(sidebar).not.toHaveClass(/is-open/)
})

test('mobile session drawer has an explicit close target and stays viewport-contained', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1200) >= 900)

  await page.getByRole('button', { name: 'Open sidebar' }).click()
  const sidebar = page.locator('.remote-sidebar')
  const close = page.getByRole('button', { name: 'Close sidebar' })

  await expect(sidebar).toHaveClass(/is-open/)
  await expect(close).toBeVisible()
  await sidebar.evaluate(async (element) => {
    await Promise.all(element.getAnimations().map((animation) => animation.finished.catch(() => undefined)))
  })

  const box = await sidebar.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.x).toBeGreaterThanOrEqual(0)
  expect(box!.x + box!.width).toBeLessThanOrEqual(page.viewportSize()!.width)

  const closeBox = await close.boundingBox()
  expect(closeBox).not.toBeNull()
  expect(closeBox!.width).toBeGreaterThanOrEqual(44)
  expect(closeBox!.height).toBeGreaterThanOrEqual(44)

  await close.click()
  await expect(sidebar).not.toHaveClass(/is-open/)
})
