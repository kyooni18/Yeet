import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

async function renderedSize(page: Parameters<typeof installMockRemote>[0], selector: string) {
  return page.locator(selector).evaluate((element) => {
    const rect = element.getBoundingClientRect()
    return { width: Math.round(rect.width), height: Math.round(rect.height) }
  })
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await page.getByTestId('toggle-inspector').click()
})

test('narrow activity inspector close control is touch-sized', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  const size = await renderedSize(page, '.activity-inspector .inspector-header .icon-button')
  expect(size.width).toBeGreaterThanOrEqual(44)
  expect(size.height).toBeGreaterThanOrEqual(44)
})

test('wide activity inspector keeps pointer-appropriate close control density', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width < 900)
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  const size = await renderedSize(page, '.activity-inspector .inspector-header .icon-button')

  if (touchFirst) {
    expect(size.width).toBeGreaterThanOrEqual(44)
    expect(size.height).toBeGreaterThanOrEqual(44)
  } else {
    expect(size.width).toBeLessThan(44)
    expect(size.height).toBeLessThan(44)
  }
})


test('large touch docked inspector exposes a forgiving resize hit zone', async ({ page }) => {
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  test.skip(!touchFirst, 'Only applies to coarse-pointer touch layouts.')

  await page.setViewportSize({ width: 1366, height: 1024 })
  const inspector = page.getByTestId('activity-inspector')
  const handle = page.getByTestId('inspector-resize-handle')
  await expect(handle).toBeVisible()

  const initial = await inspector.boundingBox()
  const hit = await handle.evaluate((element) => {
    const rect = element.getBoundingClientRect()
    const x = rect.right - 22
    const y = rect.top + Math.min(100, rect.height / 2)
    const target = document.elementFromPoint(x, y)
    const gripWidth = Number.parseFloat(getComputedStyle(element, '::after').width)
    return {
      x,
      y,
      targetWidth: Math.round(rect.width),
      gripWidth,
      hitsHandle: target === element || element.contains(target),
    }
  })

  expect(initial).not.toBeNull()
  expect(hit.targetWidth).toBeGreaterThanOrEqual(44)
  expect(hit.gripWidth).toBeLessThanOrEqual(2)
  expect(hit.hitsHandle).toBe(true)

  await page.evaluate(({ x, y }) => {
    const target = document.elementFromPoint(x, y)
    target?.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, clientX: x, clientY: y, pointerType: 'touch' }))
    window.dispatchEvent(new PointerEvent('pointermove', { bubbles: true, clientX: x - 36, clientY: y, pointerType: 'touch' }))
    window.dispatchEvent(new PointerEvent('pointerup', { bubbles: true, clientX: x - 36, clientY: y, pointerType: 'touch' }))
  }, hit)

  await expect.poll(async () => (await inspector.boundingBox())?.width ?? 0)
    .toBeGreaterThan((initial?.width ?? 0) + 20)
})
