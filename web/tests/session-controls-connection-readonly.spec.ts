import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test('session mutations stay read-only until Remote is connected', async ({ page }) => {
  let releaseProtocol!: () => void
  const protocolReady = new Promise<void>((resolve) => { releaseProtocol = resolve })

  await installMockRemote(page)
  await page.unroute('**/api/protocol')
  await page.route('**/api/protocol', async (route) => {
    await protocolReady
    await route.fulfill({
      contentType: 'application/json',
      body: JSON.stringify({ version: 1, minVersion: 1, maxVersion: 1, websocket: '/api/ws' }),
    })
  })

  await page.goto('/', { waitUntil: 'domcontentloaded' })

  const topModel = page.locator('.top-product-group').getByTestId('model-picker-trigger')
  const topGoal = page.getByTestId('toggle-goal')
  const inspector = page.getByTestId('toggle-inspector')
  const statusTrigger = page.getByTestId('open-status')

  await expect(topModel).toBeDisabled()
  await expect(topGoal).toBeDisabled()
  await expect(inspector).toBeEnabled()
  await expect(statusTrigger).toBeEnabled()
  await statusTrigger.click()

  const sheet = page.getByTestId('status-sheet')
  await expect(sheet).toBeVisible()
  await expect(sheet.getByRole('status')).toContainText('read-only while Yeet Remote reconnects')
  await expect(sheet.getByTestId('model-picker-trigger')).toBeDisabled()
  await expect(sheet.getByRole('button', { name: 'high', exact: true })).toBeDisabled()
  await expect(sheet.getByRole('button', { name: 'ON', exact: true })).toBeDisabled()
  await expect(sheet.getByRole('button', { name: 'All settings' })).toBeEnabled()

  releaseProtocol()

  await expect(sheet.getByRole('status')).toHaveCount(0)
  await expect(topModel).toBeEnabled()
  await expect(topGoal).toBeEnabled()
  await expect(sheet.getByTestId('model-picker-trigger')).toBeEnabled()
  await expect(sheet.getByRole('button', { name: 'high', exact: true })).toBeEnabled()
  await expect(sheet.getByRole('button', { name: 'ON', exact: true })).toBeEnabled()
})
