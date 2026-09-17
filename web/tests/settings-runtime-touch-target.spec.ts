import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
})

test('mobile Runtime permission action meets the 44px touch target floor', async ({ page }) => {
  await page.setViewportSize({ width: 402, height: 874 })
  await page.goto('/settings/runtime')

  const permission = page.getByRole('button', { name: 'ask →', exact: true })
  await expect(permission).toBeVisible()
  const box = await permission.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.width).toBeGreaterThanOrEqual(44)
  expect(box!.height).toBeGreaterThanOrEqual(44)
})

test('wide Runtime permission action adapts to pointer type', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto('/settings/runtime')
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)

  const permission = page.getByRole('button', { name: 'ask →', exact: true })
  await expect(permission).toBeVisible()
  const box = await permission.boundingBox()
  expect(box).not.toBeNull()
  if (touchFirst) {
    expect(box!.width).toBeGreaterThanOrEqual(44)
    expect(box!.height).toBeGreaterThanOrEqual(44)
  } else {
    expect(box!.height).toBeLessThan(44)
  }
})
