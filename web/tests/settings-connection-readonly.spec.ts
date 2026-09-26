import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

async function openSettings(page: Parameters<typeof installMockRemote>[0]) {
  const panel = page.locator('.quick-panel')
  const panelOpen = (await panel.getAttribute('class'))?.includes('is-open') ?? false
  if (!panelOpen) await page.getByRole('button', { name: 'Quick settings' }).click()
  await expect(panel).toHaveClass(/is-open/)
  await panel.getByRole('button', { name: 'Settings', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Settings' })
  await expect(dialog).toBeVisible()
  return dialog
}

test('Settings keeps dismissal available but disables Remote mutations offline', async ({ page }) => {
  let dialog = await openSettings(page)
  await expect(dialog.getByRole('switch', { name: 'OpenAI Flex' })).toBeEnabled()
  await expect(dialog.getByRole('switch', { name: 'Foundation Memory' })).toBeEnabled()
  await dialog.getByRole('button', { name: 'Close' }).click()
  await expect(dialog).toHaveCount(0)

  await page.evaluate(() => window.dispatchEvent(new Event('offline')))

  dialog = await openSettings(page)
  await expect(dialog.getByRole('button', { name: 'Close' })).toBeEnabled()
  await expect(dialog.getByRole('button', { name: 'Auto', exact: true })).toBeDisabled()
  await expect(dialog.getByRole('button', { name: 'Light', exact: true })).toBeDisabled()
  await expect(dialog.getByRole('button', { name: 'Dark', exact: true })).toBeDisabled()
  await expect(dialog.getByRole('switch', { name: 'OpenAI Flex' })).toBeDisabled()
  await expect(dialog.getByRole('switch', { name: 'Foundation Memory' })).toBeDisabled()

  const capabilitySwitches = dialog.locator('.settings-section').filter({ hasText: 'Capabilities' }).getByRole('switch')
  for (let index = 0; index < await capabilitySwitches.count(); index += 1) {
    await expect(capabilitySwitches.nth(index)).toBeDisabled()
  }

  const providerActions = dialog.locator('.provider-settings-row .small-action')
  for (let index = 0; index < await providerActions.count(); index += 1) {
    await expect(providerActions.nth(index)).toBeDisabled()
  }
})
