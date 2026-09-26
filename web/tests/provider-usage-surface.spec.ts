import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('provider usage lives in the composer toolbar and opens a compact detail popover', async ({ page }) => {
  const usageButton = page.locator('.usage-control > button')
  await expect(usageButton).toBeVisible()
  await expect(usageButton).toContainText('69%')

  await usageButton.click()
  await expect(usageButton).toHaveAttribute('aria-expanded', 'true')
  await expect(usageButton).toHaveAttribute('aria-controls', 'provider-usage-popover')
  await expect(usageButton).toHaveAttribute('aria-pressed', 'true')

  const popover = page.locator('.usage-popover')
  await expect(popover).toBeVisible()
  await expect(popover).toHaveAttribute('data-positioned', 'true')
  await expect(popover).toContainText('gpt-5.6-sol')
  await expect(popover).toContainText('team')
  await expect(popover).toContainText('Input')
  await expect(popover).toContainText('Output')
  await expect(popover).toContainText('5 hour')
  await expect(popover).toContainText('69%')

  const progress = popover.locator('progress').first()
  await expect(progress).toHaveAttribute('value', '69')
  await expect(progress).toHaveAttribute('max', '100')

  const buttonBox = await usageButton.boundingBox()
  const popoverBox = await popover.boundingBox()
  const viewport = page.viewportSize()
  expect(buttonBox).not.toBeNull()
  expect(popoverBox).not.toBeNull()
  expect(viewport).not.toBeNull()
  expect(popoverBox!.y + popoverBox!.height).toBeLessThanOrEqual(buttonBox!.y)
  expect(popoverBox!.x).toBeGreaterThanOrEqual(8)
  expect(popoverBox!.x + popoverBox!.width).toBeLessThanOrEqual(viewport!.width - 8)

  await page.keyboard.press('Escape')
  await expect(popover).toHaveCount(0)
  await expect(usageButton).toHaveAttribute('aria-expanded', 'false')
  await expect(usageButton).toHaveAttribute('aria-pressed', 'false')
})

test('quick controls expose the same usage state without desktop-only meters', async ({ page }) => {
  await page.getByRole('button', { name: 'Quick settings' }).click()
  const panel = page.locator('.quick-panel')

  await expect(panel).toHaveClass(/is-open/)
  await expect(panel.getByText('Usage', { exact: true })).toBeVisible()
  await expect(panel).toContainText('5 hour')
  await expect(panel).toContainText('69%')
  await expect(page.getByTestId('top-usage-meter')).toHaveCount(0)
})
