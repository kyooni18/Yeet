import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/', { waitUntil: 'domcontentloaded' })
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('skip navigation bypasses the workspace sidebar and focuses the conversation transcript', async ({ page }) => {
  const skipLink = page.getByRole('link', { name: 'Skip to conversation' })
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)

  if (touchFirst) await skipLink.focus()
  else await page.keyboard.press('Tab')

  await expect(skipLink).toBeFocused()
  await expect(skipLink).toBeVisible()
  await expect.poll(async () => (await skipLink.boundingBox())?.x ?? -1).toBeGreaterThanOrEqual(0)
  await expect.poll(async () => (await skipLink.boundingBox())?.y ?? -1).toBeGreaterThanOrEqual(0)

  const urlBeforeSkip = page.url()
  const historyLengthBeforeSkip = await page.evaluate(() => history.length)
  await page.keyboard.press('Enter')
  await expect(page.getByRole('main', { name: 'Conversation transcript' })).toBeFocused()
  await expect(page).toHaveURL(urlBeforeSkip)
  await expect.poll(() => page.evaluate(() => history.length)).toBe(historyLengthBeforeSkip)
})

test('skip navigation is unavailable while the mobile sessions dialog owns focus', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'mobile-portrait')

  const skipLink = page.getByRole('link', { name: 'Skip to conversation' })
  await page.getByRole('button', { name: 'Open sidebar' }).click()

  await expect(page.locator('.remote-sidebar')).toBeVisible()
  await expect(skipLink).toHaveCount(0)
})


test('skip navigation stays behind the authorization modal', async ({ page }) => {
  await page.route('**/api/auth/status', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ required: true, authenticated: false, key: false, passkey: false }),
    })
  })
  await page.reload({ waitUntil: 'domcontentloaded' })

  await expect(page.getByRole('dialog')).toBeVisible()
  await expect(page.getByRole('link', { name: 'Skip to conversation' })).toHaveCount(0)
})
