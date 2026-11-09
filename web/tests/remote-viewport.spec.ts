import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test('visual viewport resize and pan keep the shell and keyboard composer aligned', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900, 'Mobile viewport behavior')
  await page.addInitScript(() => {
    class Viewport extends EventTarget {
      width = innerWidth; height = innerHeight; offsetTop = 0; offsetLeft = 0; scale = 1
    }
    const viewport = new Viewport()
    Object.defineProperty(window, 'visualViewport', { configurable: true, value: viewport })
    ;(window as any).__viewport = (height: number, top: number) => {
      viewport.height = height; viewport.offsetTop = top
      viewport.dispatchEvent(new Event('resize')); viewport.dispatchEvent(new Event('scroll'))
    }
  })
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await page.getByRole('textbox', { name: 'Message', exact: true }).focus()
  const height = Math.max(250, Math.round(page.viewportSize()!.height * .65))
  await page.evaluate(height => (window as any).__viewport(height, 18), height)
  await expect.poll(() => page.locator('#root').evaluate(el => {
    const box = el.getBoundingClientRect(); return [Math.round(box.top), Math.round(box.height)]
  })).toEqual([18, height])
  const bounds = await page.locator('.composer-surface').boundingBox()
  expect(bounds!.y).toBeGreaterThanOrEqual(18)
  expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(height + 19)
  expect(await page.evaluate(() => window.scrollY)).toBe(0)
  await page.evaluate(() => (window as any).__viewport(window.innerHeight, 0))
  await expect.poll(() => page.locator('#root').evaluate(el => Math.round(el.getBoundingClientRect().top))).toBe(0)
})
