import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test('enrollment transport failure stays accurate and can recover in place', async ({ page }) => {
  await installMockRemote(page)
  let attempts = 0
  await page.route(/\/api\/auth\/status\?enroll=probe$/, async (route) => {
    attempts += 1
    if (attempts === 1) {
      await route.fulfill({
        status: 502,
        contentType: 'application/json',
        body: JSON.stringify({ error: '502 Bad Gateway' }),
      })
      return
    }
    await route.fulfill({
      contentType: 'application/json',
      body: JSON.stringify({ required: true, authenticated: false, key: false, passkey: true, enrollmentValid: true }),
    })
  })

  await page.goto('/enroll?token=probe')

  await expect(page.getByRole('heading', { name: 'Register a passkey' })).toBeVisible()
  await expect(page.getByText('Yeet Remote could not verify this enrollment link.')).toBeVisible()
  await expect(page.getByText(/invalid or has expired/i)).toHaveCount(0)
  await expect(page.getByRole('alert')).toContainText('502 Bad Gateway')

  await page.getByRole('button', { name: 'Retry' }).click()

  await expect(page.getByText('This enrollment was authorized from the local Yeet terminal.')).toBeVisible()
  await expect(page.getByRole('alert')).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Retry' })).toHaveCount(0)
  expect(attempts).toBe(2)
})
