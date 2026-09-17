import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
})

test('unknown Remote paths recover to the conversation instead of rendering blank', async ({ page }) => {
  await page.goto('/stale/deep-link')

  await expect(page).toHaveURL(/\/$/)
  await expect(page.getByText('Interface ready')).toBeVisible()
  await expect(page.getByTestId('composer')).toBeVisible()
})
