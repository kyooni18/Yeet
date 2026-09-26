import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('activity history is compact by default and expands in place', async ({ page }) => {
  const group = page.locator('.activity-group').first()
  const header = group.locator('.activity-group__header')

  await expect(header).toBeVisible()
  await expect(header).toHaveAttribute('aria-expanded', 'false')
  await expect(group.locator('.activity-group__events')).toHaveCount(0)

  await header.click()
  await expect(header).toHaveAttribute('aria-expanded', 'true')
  await expect.poll(async () => group.locator('.activity-group__event').count()).toBeGreaterThanOrEqual(4)
  await expect(group).toContainText('Read file')
  await expect(group).toContainText('frontend-review')
  await expect(group).toContainText('Yeet-KY · list_files')
})

test('activity tools disclose details inline without a secondary inspector', async ({ page }) => {
  const group = page.locator('.activity-group').first()
  await group.locator('.activity-group__header').click()

  const tool = group.locator('.trace-disclosure__row').filter({ hasText: 'Read file' }).first()
  await expect(tool).toBeVisible()
  await tool.click()
  await expect(tool).toHaveAttribute('aria-expanded', 'true')
  await expect(tool.locator('..')).toContainText('web/src/App.tsx')
})
