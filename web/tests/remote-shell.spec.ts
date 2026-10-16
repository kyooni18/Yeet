import { test, expect } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test('remote shell sends, navigates and hides the sidebar', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 })
  await installMockRemote(page)
  await page.goto('/')

  await expect(page.locator('yeet-markdown.assistant').first()).toBeVisible()
  const input = page.locator('textarea.input')
  await input.fill('hello there')
  await input.press('Enter')
  await expect(input).toHaveValue('')

  await page.getByRole('button', { name: 'Files' }).click()
  await expect(page.getByRole('button', { name: /README\.md/ })).toBeVisible()
  await page.getByRole('button', { name: 'Changes' }).click()
  await expect(page.locator('.line.add').first()).toBeVisible()

  await page.getByRole('button', { name: 'Toggle sidebar' }).click()
  await expect(page.locator('.sidebar')).toHaveCount(0)
})
