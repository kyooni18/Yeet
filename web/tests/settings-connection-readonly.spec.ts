import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test('settings controls stay read-only until Remote is connected', async ({ page }) => {
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

  await page.goto('/settings/runtime', { waitUntil: 'domcontentloaded' })

  const connection = page.locator('.settings-connection')
  const notice = page.getByRole('status')
  const goal = page.getByRole('switch', { name: 'Goal' })
  const modelTrigger = page.getByTestId('runtime-model-picker').getByTestId('model-picker-trigger')

  await expect(connection).toContainText('connecting')
  await expect(notice).toContainText('read-only until Yeet Remote reconnects')
  await expect(goal).toBeDisabled()
  await expect(modelTrigger).toBeDisabled()

  const providersTab = page.getByRole('button', { name: 'Providers', exact: true })
  await expect(providersTab).toBeEnabled()
  await providersTab.click()
  await expect(page).toHaveURL(/\/settings\/providers$/)
  await expect(page.getByRole('heading', { name: 'Providers' })).toBeVisible()

  releaseProtocol()
  await expect(connection).toContainText('connected')
  await expect(notice).toHaveCount(0)

  await page.getByRole('button', { name: 'Runtime', exact: true }).click()
  await expect(goal).toBeEnabled()
  await expect(modelTrigger).toBeEnabled()
})
