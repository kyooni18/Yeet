import { expect, test, type Locator } from '@playwright/test'
import { installMockRemote } from './mockRemote'

async function expectMinTarget(locator: Locator, size = 44) {
  const box = await locator.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.width).toBeGreaterThanOrEqual(size)
  expect(box!.height).toBeGreaterThanOrEqual(size)
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('wide touch layouts keep TopBar and composer controls touch-safe', async ({ page }) => {
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  const viewportWidth = page.viewportSize()?.width ?? 0
  test.skip(!touchFirst || viewportWidth < 900, 'Only applies to desktop-width touch layouts such as iPad landscape.')

  await expectMinTarget(page.getByRole('button', { name: 'Open sidebar' }))
  await expectMinTarget(page.getByRole('button', { name: 'New chat' }))
  await expectMinTarget(page.getByRole('button', { name: 'Quick settings' }))

  const model = page.getByRole('button', { name: /Choose model, current/ })
  const goal = page.getByRole('button', { name: 'Goal' })
  await expectMinTarget(model)
  await expectMinTarget(goal)

  await model.click()
  const dialog = page.getByRole('dialog', { name: 'Choose model' })
  const search = dialog.getByPlaceholder('Search models')
  await expect(dialog).toBeVisible()
  await expect(search).toBeFocused()
  await dialog.evaluate(async (element) => {
    await Promise.all(element.getAnimations().map((animation) => animation.finished.catch(() => undefined)))
  })

  const searchSurface = dialog.locator('.sheet-search')
  const searchBox = await searchSurface.boundingBox()
  expect(searchBox).not.toBeNull()
  expect(searchBox!.height).toBeGreaterThanOrEqual(44)
  await expect.poll(() => search.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))).toBeGreaterThanOrEqual(16)

  const rows = dialog.locator('.model-row')
  expect(await rows.count()).toBeGreaterThan(0)
  for (const row of await rows.all()) await expectMinTarget(row)
})
