import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

async function renderedHeights(page: Parameters<typeof installMockRemote>[0], selector: string): Promise<number[]> {
  return page.locator(selector).evaluateAll((elements) =>
    elements.map((element) => Math.round(element.getBoundingClientRect().height)),
  )
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('narrow transcript interactions keep touch-sized hit areas', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })

  for (const selector of [
    '.reasoning-card summary',
    '.semantic-card summary',
    '.tool-card-summary',
    '[data-copy-code]',
  ]) {
    const heights = await renderedHeights(page, selector)
    expect(heights.length).toBeGreaterThan(0)
    for (const height of heights) expect(height).toBeGreaterThanOrEqual(44)
  }

  await page.locator('.tool-card-summary').click()
  const toolCopyHeights = await renderedHeights(page, '.tool-copy-button')
  expect(toolCopyHeights.length).toBeGreaterThan(0)
  for (const height of toolCopyHeights) expect(height).toBeGreaterThanOrEqual(44)
})

test('wide transcript adapts disclosure and copy targets to pointer type', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width < 900)
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)

  for (const selector of [
    '.reasoning-card summary',
    '.semantic-card summary',
    '.tool-card-summary',
    '[data-copy-code]',
  ]) {
    const heights = await renderedHeights(page, selector)
    expect(heights.length).toBeGreaterThan(0)
    for (const height of heights) {
      if (touchFirst) expect(height).toBeGreaterThanOrEqual(44)
      else expect(height).toBeLessThan(44)
    }
  }
})
