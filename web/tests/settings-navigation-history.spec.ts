import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test('leaving Settings does not trap browser history on the Settings route', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()

  await page.getByRole('button', { name: 'Settings' }).click()
  await expect(page).toHaveURL(/\/settings/)

  await page.getByRole('button', { name: 'Back to conversation' }).click()
  await expect(page).toHaveURL(/\/$/)

  await page.goBack()
  await expect(page).toHaveURL(/\/$/)
  await expect(page.getByRole('textbox', { name: 'Message Yeet' })).toBeVisible()
})

test('direct Settings entry still returns safely to the conversation', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/settings/runtime')
  await expect(page.getByRole('heading', { name: 'Runtime' })).toBeVisible()

  await page.getByRole('button', { name: 'Back to conversation' }).click()
  await expect(page).toHaveURL(/\/$/)
  await expect(page.getByRole('textbox', { name: 'Message Yeet' })).toBeVisible()
})


test('invalid Settings section deep links canonicalize to Runtime without adding a history trap', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await page.goto('/settings/not-a-real-section')

  await expect(page.getByRole('heading', { name: 'Runtime' })).toBeVisible()
  await expect(page).toHaveURL(/\/settings\/runtime$/)

  await page.goBack()
  await expect(page).toHaveURL(/\/$/)
  await expect(page.getByRole('textbox', { name: 'Message Yeet' })).toBeVisible()
})
