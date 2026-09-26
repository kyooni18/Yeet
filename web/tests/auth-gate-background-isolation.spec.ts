import { expect, test, type Page } from '@playwright/test'

async function mockAccessKeyAuth(page: Page) {
  await page.route('**/api/auth/status*', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        required: true,
        authenticated: false,
        key: true,
        passkey: false,
        enrollmentValid: false,
      }),
    })
  })
}

test.beforeEach(async ({ page }) => {
  await mockAccessKeyAuth(page)
})

for (const routeCase of [
  { name: 'conversation', path: '/' },
  { name: 'legacy settings URL', path: '/settings/runtime' },
]) {
  test(`authorization exclusively owns the ${routeCase.name} route and traps keyboard focus`, async ({ page }) => {
    await page.goto(routeCase.path)

    const dialog = page.getByRole('dialog', { name: 'Authorization required' })
    const key = page.getByLabel('Access key')

    await expect(dialog).toBeVisible()
    await expect(dialog).toHaveAttribute('aria-modal', 'true')
    await expect(page.locator('.remote-stage')).toHaveCount(0)
    await expect(key).toBeFocused()

    await page.keyboard.press('Tab')
    await expect(key).toBeFocused()
    await page.keyboard.press('Shift+Tab')
    await expect(key).toBeFocused()
  })
}
