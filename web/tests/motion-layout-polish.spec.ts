import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})


test('sidebar and quick controls use real animated drawers and remain viewport-contained', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'no-preference' })

  await page.getByRole('button', { name: 'Open sidebar' }).click()
  const sidebar = page.locator('.remote-sidebar')
  await expect(sidebar).toHaveClass(/is-open/)
  await sidebar.evaluate(async (element) => {
    await Promise.all(element.getAnimations().map((animation) => animation.finished.catch(() => undefined)))
  })
  const sidebarBox = await sidebar.boundingBox()
  expect(sidebarBox).not.toBeNull()
  expect(sidebarBox!.x).toBeGreaterThanOrEqual(0)
  expect(sidebarBox!.y).toBeGreaterThanOrEqual(0)
  expect(sidebarBox!.x + sidebarBox!.width).toBeLessThanOrEqual(page.viewportSize()!.width)
  expect(sidebarBox!.y + sidebarBox!.height).toBeLessThanOrEqual(page.viewportSize()!.height)

  const sidebarDuration = await sidebar.evaluate((element) => getComputedStyle(element).transitionDuration)
  expect(sidebarDuration).not.toBe('0s')
  await page.getByRole('button', { name: 'Close sidebar' }).click()
  await expect(sidebar).not.toHaveClass(/is-open/)

  await page.getByRole('button', { name: 'Quick settings' }).click()
  const controls = page.locator('.quick-panel')
  await expect(controls).toHaveClass(/is-open/)
  await controls.evaluate(async (element) => {
    await Promise.all(element.getAnimations().map((animation) => animation.finished.catch(() => undefined)))
  })
  const controlsBox = await controls.boundingBox()
  expect(controlsBox).not.toBeNull()
  expect(controlsBox!.x).toBeGreaterThanOrEqual(0)
  expect(controlsBox!.y).toBeGreaterThanOrEqual(0)
  expect(controlsBox!.x + controlsBox!.width).toBeLessThanOrEqual(page.viewportSize()!.width)
  expect(controlsBox!.y + controlsBox!.height).toBeLessThanOrEqual(page.viewportSize()!.height)

  const controlsDuration = await controls.evaluate((element) => getComputedStyle(element).transitionDuration)
  expect(controlsDuration).not.toBe('0s')
})

test('reduced motion collapses drawer animation duration', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' })
  await page.getByRole('button', { name: 'Quick settings' }).click()

  const maxDurationMs = await page.locator('.quick-panel').evaluate((element) => {
    const parse = (value: string) => value.split(',').map((part) => {
      const trimmed = part.trim()
      return trimmed.endsWith('ms') ? Number.parseFloat(trimmed) : Number.parseFloat(trimmed) * 1000
    })
    return Math.max(...parse(getComputedStyle(element).transitionDuration))
  })
  expect(maxDurationMs).toBeLessThanOrEqual(1)
})
