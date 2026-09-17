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
  { name: 'settings', path: '/settings/runtime' },
]) {
  test(`authorization dialog isolates the ${routeCase.name} route from assistive and keyboard interaction`, async ({ page }) => {
    await page.goto(routeCase.path)

    const dialog = page.getByRole('dialog', { name: 'Authorization required' })
    const routedInterface = page.locator('#app > :not(.auth-gate)').first()

    await expect(dialog).toBeVisible()
    await expect(dialog).toHaveAttribute('aria-modal', 'true')
    await expect(routedInterface).toHaveAttribute('inert', '')
    await expect(routedInterface).toHaveAttribute('aria-hidden', 'true')

    const isolation = await routedInterface.evaluate((element) => ({
      inert: (element as HTMLElement).inert,
      focusableDescendants: [...element.querySelectorAll<HTMLElement>('button, a[href], input, textarea, select, summary, [tabindex]')]
        .filter((candidate) => candidate.tabIndex >= 0)
        .length,
    }))

    expect(isolation.inert).toBe(true)
    expect(isolation.focusableDescendants).toBeGreaterThan(0)
    await expect(page.getByLabel('Access key')).toBeFocused()
  })
}
