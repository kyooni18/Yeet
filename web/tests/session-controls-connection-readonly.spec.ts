import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('Remote-backed session controls become read-only offline while local navigation remains available', async ({ page }) => {
  const model = page.getByRole('button', { name: /Choose model, current/ })
  const reasoning = page.getByRole('combobox', { name: 'Reasoning' }).first()
  const goal = page.getByRole('button', { name: 'Goal' })
  const quickSettings = page.getByRole('button', { name: 'Quick settings' })

  await expect(model).toBeEnabled()
  await expect(reasoning).toBeEnabled()
  await expect(goal).toBeEnabled()
  await expect(quickSettings).toBeEnabled()

  await quickSettings.click()
  const panel = page.locator('.quick-panel')
  await expect(panel).toHaveClass(/is-open/)
  await expect(panel.getByRole('button', { name: /Model/ })).toBeEnabled()
  await expect(panel.getByRole('combobox', { name: 'Reasoning' })).toBeEnabled()
  await expect(panel.getByRole('switch', { name: 'Goal mode' })).toBeEnabled()
  await expect(panel.getByRole('button', { name: 'Settings', exact: true })).toBeEnabled()

  await page.evaluate(() => window.dispatchEvent(new Event('offline')))

  await expect(model).toBeDisabled()
  await expect(reasoning).toBeDisabled()
  await expect(goal).toBeDisabled()
  await expect(quickSettings).toBeEnabled()

  await expect(panel.getByRole('button', { name: /Model/ })).toBeDisabled()
  await expect(panel.getByRole('combobox', { name: 'Reasoning' })).toBeDisabled()
  await expect(panel.getByRole('switch', { name: 'Goal mode' })).toBeDisabled()
  await expect(panel.getByRole('button', { name: 'Settings', exact: true })).toBeEnabled()

  const sandboxPreset = panel.getByRole('combobox', { name: 'Sandbox preset' })
  if (await sandboxPreset.count()) await expect(sandboxPreset).toBeDisabled()
  const autoApprove = panel.getByRole('switch', { name: 'Auto approve' })
  if (await autoApprove.count()) await expect(autoApprove).toBeDisabled()
})
