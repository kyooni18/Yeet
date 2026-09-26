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

async function openAuthGate(page: Page, width: number, height: number) {
  await page.setViewportSize({ width, height })
  await page.goto('/')
  const dialog = page.getByRole('dialog')
  await expect(dialog).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Authorization required' })).toBeVisible()
  return dialog
}

test.beforeEach(async ({ page }) => {
  await mockAccessKeyAuth(page)
})


test('normal auth gate explains that authentication methods are still loading', async ({ page }) => {
  await page.unroute('**/api/auth/status*')
  await page.route('**/api/auth/status*', async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 800))
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ required: true, authenticated: false, key: true, passkey: false, enrollmentValid: false }),
    })
  })

  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'Authorization required' })).toBeVisible()
  const loading = page.getByRole('status', { name: 'Loading authentication methods' })
  await expect(loading).toHaveText('Loading authentication methods…')
  await expect(page.getByLabel('Access key')).not.toBeVisible()
  await expect(page.getByLabel('Access key')).toBeVisible()
  await expect(page.getByLabel('Access key')).toBeFocused()
})

test('phone auth actions are touch-sized and access key receives focus', async ({ page }) => {
  await openAuthGate(page, 390, 844)

  const input = page.getByLabel('Access key')
  const authorize = page.getByRole('button', { name: 'Authorize' })
  await expect(input).toBeFocused()

  const inputBox = await input.boundingBox()
  const buttonBox = await authorize.boundingBox()
  expect(inputBox).not.toBeNull()
  expect(buttonBox).not.toBeNull()
  expect(inputBox!.height).toBeGreaterThanOrEqual(44)
  expect(buttonBox!.height).toBeGreaterThanOrEqual(44)
})

test('keyboard-constrained auth error remains reachable instead of center-clipping', async ({ page }) => {
  await page.route('**/api/auth/key', async (route) => {
    await route.fulfill({
      status: 401,
      contentType: 'application/json',
      body: JSON.stringify({ error: 'invalid access key' }),
    })
  })
  const dialog = await openAuthGate(page, 390, 320)
  await page.getByLabel('Access key').fill('wrong-key')
  await page.getByRole('button', { name: 'Authorize' }).click()

  const alert = page.getByRole('alert')
  await expect(alert).toContainText('invalid access key')
  const card = page.locator('.auth-card')
  const cardBox = await card.boundingBox()
  expect(cardBox).not.toBeNull()
  expect(cardBox!.y).toBeGreaterThanOrEqual(0)

  const viewportFit = await dialog.evaluate((element) => ({
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
  }))
  expect(viewportFit.scrollHeight).toBeGreaterThanOrEqual(viewportFit.clientHeight)

  await alert.scrollIntoViewIfNeeded()
  const alertBox = await alert.boundingBox()
  expect(alertBox).not.toBeNull()
  expect(alertBox!.y).toBeGreaterThanOrEqual(0)
  expect(alertBox!.y + alertBox!.height).toBeLessThanOrEqual(320)
})

test('wide auth gate adapts action density to pointer type', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width < 900)
  await openAuthGate(page, viewport.width, viewport.height)

  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  const input = page.getByLabel('Access key')
  const authorize = await page.getByRole('button', { name: 'Authorize' }).boundingBox()
  const inputBox = await input.boundingBox()
  expect(authorize).not.toBeNull()
  expect(inputBox).not.toBeNull()

  if (touchFirst) {
    expect(authorize!.height).toBeGreaterThanOrEqual(44)
    expect(inputBox!.height).toBeGreaterThanOrEqual(44)
    expect(await input.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))).toBeGreaterThanOrEqual(16)
  } else {
    expect(authorize!.height).toBeLessThan(44)
  }
})
