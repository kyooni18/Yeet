import { test, expect } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test('remote shell sends, navigates and hides the sidebar', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 })
  await installMockRemote(page)
  await page.goto('/')

  await expect(page.locator('yeet-markdown').nth(1)).toBeVisible()
  const input = page.getByRole('textbox', { name: 'Message' })
  await input.fill('hello there')
  await input.press('Enter')
  await expect(input).toHaveValue('')

  await page.getByRole('button', { name: 'Files', exact: true }).first().click()
  await expect(page.getByRole('button', { name: /README\.md/ })).toBeVisible()
  await page.getByRole('button', { name: 'Diff', exact: true }).first().click()
  await expect(page.getByRole('button', { name: /src\/limiter\.ts/ })).toBeVisible()

  await page.getByRole('button', { name: 'Hide sidebar' }).click()
  await expect(page.getByRole('button', { name: 'Toggle sidebar' })).toBeVisible()
})
