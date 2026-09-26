import { expect, test, type Locator, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

async function expectMinHeight(locator: Locator, height = 44) {
  const sizes = await locator.evaluateAll((elements) => elements.map((element) => ({
    label: element.getAttribute('aria-label') || element.textContent?.trim() || element.className,
    height: element.getBoundingClientRect().height,
  })))
  expect(sizes.length).toBeGreaterThan(0)
  for (const item of sizes) expect(Math.round(item.height), String(item.label)).toBeGreaterThanOrEqual(height)
}

async function openQuickPanel(page: Page, width: number, height: number) {
  await page.setViewportSize({ width, height })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await page.getByRole('button', { name: 'Quick settings' }).click()
  const panel = page.locator('.quick-panel')
  await expect(panel).toHaveClass(/is-open/)
  return panel
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
})

test('phone Quick Settings opener remains touch-sized', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()

  const opener = page.getByRole('button', { name: 'Quick settings' })
  const box = await opener.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.width).toBeGreaterThanOrEqual(44)
  expect(box!.height).toBeGreaterThanOrEqual(44)
})

test('phone QuickPanel keeps interactive rows and dismissal touch-sized', async ({ page }) => {
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  test.skip(!touchFirst, 'Touch-first behavior only')

  const panel = await openQuickPanel(page, 390, 844)
  await expectMinHeight(panel.locator('.quick-row:visible'))
  await expectMinHeight(panel.locator('.quick-workspace:visible'))
  await expectMinHeight(panel.locator('.quick-settings-button:visible'))

  const close = panel.getByRole('button', { name: 'Close controls' })
  const closeBox = await close.boundingBox()
  expect(closeBox).not.toBeNull()
  expect(Math.round(closeBox!.width)).toBeGreaterThanOrEqual(44)
  expect(Math.round(closeBox!.height)).toBeGreaterThanOrEqual(44)
})

test('short landscape QuickPanel stays inside the viewport and scrolls internally', async ({ page }) => {
  const panel = await openQuickPanel(page, 852, 393)
  const box = await panel.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.y).toBeGreaterThanOrEqual(0)
  expect(box!.y + box!.height).toBeLessThanOrEqual(393)

  const scroll = panel.locator('.quick-panel__scroll')
  const metrics = await scroll.evaluate((element) => ({
    overflowY: getComputedStyle(element).overflowY,
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
  }))
  expect(['auto', 'scroll']).toContain(metrics.overflowY)
  expect(metrics.scrollHeight).toBeGreaterThanOrEqual(metrics.clientHeight)

  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  if (touchFirst) await expectMinHeight(panel.locator('.quick-row:visible'))
})

test('wide touch QuickPanel opens the touch-safe Model sheet', async ({ page }) => {
  const viewport = page.viewportSize()
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  test.skip(!touchFirst || !viewport || viewport.width < 900, 'Wide touch behavior only')

  const panel = await openQuickPanel(page, viewport.width, viewport.height)
  const model = panel.getByRole('button', { name: /Model/ })
  const modelBox = await model.boundingBox()
  expect(modelBox).not.toBeNull()
  expect(modelBox!.height).toBeGreaterThanOrEqual(44)

  await model.click()
  const dialog = page.getByRole('dialog', { name: 'Choose model' })
  const search = dialog.getByPlaceholder('Search models')
  await expect(search).toBeFocused()
  await dialog.evaluate(async (element) => {
    await Promise.all(element.getAnimations().map((animation) => animation.finished.catch(() => undefined)))
  })

  const searchBox = await dialog.locator('.sheet-search').boundingBox()
  expect(searchBox).not.toBeNull()
  expect(searchBox!.height).toBeGreaterThanOrEqual(44)
  await expect.poll(() => search.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))).toBeGreaterThanOrEqual(16)

  await expectMinHeight(dialog.locator('.model-row'))
})
