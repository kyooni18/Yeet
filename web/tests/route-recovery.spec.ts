import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
})

test('unknown Remote paths still render the conversation shell instead of going blank', async ({ page }) => {
  await page.goto('/stale/deep-link')

  await expect(page.getByText('Interface ready')).toBeVisible()
  await expect(page.locator('.composer-shell')).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Message' })).toBeVisible()
})
