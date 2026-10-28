import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

async function openSettings(page: Parameters<typeof installMockRemote>[0]) {
  const sidebar = page.getByRole('navigation', { name: 'Workspace navigation' })
  if (!await sidebar.isVisible()) await page.getByRole('button', { name: 'Toggle sidebar' }).click()
  await sidebar.getByRole('button', { name: 'Settings', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Settings' })
  await expect(dialog).toBeVisible()
  return dialog
}

test('Settings keeps dismissal available but disables Remote mutations offline', async ({ page }) => {
  let dialog = await openSettings(page)
  await expect(dialog.getByRole('switch', { name: 'OpenAI Flex' })).toBeDisabled()
  await dialog.getByRole('button', { name: 'Done' }).click()
  await expect(dialog).toHaveCount(0)

  await page.evaluate(() => window.dispatchEvent(new Event('offline')))

  dialog = await openSettings(page)
  await expect(dialog.getByRole('button', { name: 'Done' })).toBeEnabled()
  await expect(dialog.getByRole('button', { name: 'Auto', exact: true })).toBeDisabled()
  await expect(dialog.getByRole('button', { name: 'Light', exact: true })).toBeDisabled()
  await expect(dialog.getByRole('button', { name: 'Dark', exact: true })).toBeDisabled()
  await expect(dialog.getByRole('switch', { name: 'OpenAI Flex' })).toBeDisabled()

  const mutationControls = dialog.getByRole('switch')
  expect(await mutationControls.count()).toBeGreaterThan(0)
  for (const control of await mutationControls.all()) await expect(control).toBeDisabled()
  const buttons = dialog.getByRole('button').filter({ hasNotText: 'Done' })
  expect(await buttons.count()).toBeGreaterThan(3)
  for (const button of await buttons.all()) await expect(button).toBeDisabled()
})

test('Settings honors shared working state and Flex billing eligibility', async ({ page }) => {
  const dialog = await openSettings(page)
  await expect(dialog.getByRole('switch', { name: 'OpenAI Flex' })).toBeDisabled()
  let sequence = 20
  const emit = (state: Record<string, unknown>) => page.evaluate(({ state, sequence }) => {
    ;(window as unknown as { __yeetEmit: (message: unknown) => void }).__yeetEmit({ type: 'state_update', version: 1, sequence, revision: sequence, patch: state })
  }, { state, sequence: sequence++ })
  await emit({ active_model: 'openai/gpt-5.6-sol', auth_providers: [{ provider: 'openai', authenticated: true, method: 'api-key', expires_at: null, error: null }] })
  await expect(dialog.getByRole('switch', { name: 'OpenAI Flex' })).toBeEnabled()
  await emit({ settings_working: true })
  await expect(dialog.getByRole('button', { name: 'Light', exact: true })).toBeDisabled()
  await emit({ settings_working: false })
  await expect(dialog.getByRole('button', { name: 'Light', exact: true })).toBeEnabled()
})
