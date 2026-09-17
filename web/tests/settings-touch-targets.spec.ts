import { expect, test, type Locator, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

async function expectMinHeight(locator: Locator, height: number) {
  const box = await locator.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.height).toBeGreaterThanOrEqual(height)
}

async function openSettings(page: Page, section: string) {
  await page.goto(`/settings/${section}`)
  await expect(page.locator('.settings-section')).toBeVisible()
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
})

test('narrow settings keep native and custom controls touch-sized', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })

  await openSettings(page, 'runtime')
  await expectMinHeight(page.getByRole('group', { name: 'Reasoning level' }).locator('button').first(), 44)
  await expectMinHeight(page.getByRole('switch', { name: 'Goal' }), 44)

  await openSettings(page, 'sandbox')
  await expectMinHeight(page.getByRole('group', { name: 'Sandbox preset' }).locator('button').first(), 44)
  await expectMinHeight(page.getByRole('combobox', { name: 'Execution mode' }), 44)
  await expectMinHeight(page.getByRole('combobox', { name: 'Workspace access mode' }), 44)

  await openSettings(page, 'providers')
  await expectMinHeight(page.locator('.check-label'), 44)
})

test('wide settings adapt action density to pointer type', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 820 })
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)

  await openSettings(page, 'runtime')
  const controls = [
    page.getByRole('button', { name: 'Back to conversation' }),
    page.getByRole('button', { name: 'Runtime', exact: true }),
    page.getByRole('group', { name: 'Reasoning level' }).locator('button').first(),
    page.getByRole('switch', { name: 'Goal' }),
  ]

  for (const control of controls) {
    const box = await control.boundingBox()
    expect(box).not.toBeNull()
    if (touchFirst) expect(box!.height).toBeGreaterThanOrEqual(44)
    else expect(box!.height).toBeLessThan(44)
  }

  await openSettings(page, 'sandbox')
  for (const name of ['Execution mode', 'Workspace access mode']) {
    const select = page.getByRole('combobox', { name })
    const box = await select.boundingBox()
    expect(box).not.toBeNull()
    if (touchFirst) expect(box!.height).toBeGreaterThanOrEqual(44)
    else expect(box!.height).toBeLessThan(44)
  }

  await openSettings(page, 'providers')
  const checkboxRow = await page.locator('.check-label').boundingBox()
  expect(checkboxRow).not.toBeNull()
  if (touchFirst) expect(checkboxRow!.height).toBeGreaterThanOrEqual(44)
  else expect(checkboxRow!.height).toBeLessThan(44)
})
