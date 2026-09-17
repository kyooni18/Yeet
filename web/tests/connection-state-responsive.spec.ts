import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('phone keeps offline state visible without opening secondary UI', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await page.evaluate(() => window.dispatchEvent(new Event('offline')))

  await expect(page.getByRole('status')).toContainText('Offline')
  await expect(page.getByRole('button', { name: /Connection: offline/ })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Send message' })).toBeDisabled()
})

test('fatal transport failure exposes the actual reason instead of only a generic failure', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await page.evaluate(() => {
    const emit = (window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit
    emit({
      type: 'error',
      code: 'protocol_mismatch',
      message: 'Server requires Remote protocol v2.',
      fatal: true,
    })
  })

  const failure = page.getByRole('alert')
  await expect(failure).toContainText('Connection failed')
  await expect(failure).toContainText('protocol_mismatch: Server requires Remote protocol v2.')
  await expect(page.getByRole('button', { name: /Connection: failed/ })).toBeVisible()
})

test('connected phone stays visually quiet', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await expect(page.locator('.connection-banner')).toHaveCount(0)
  await expect(page.getByRole('button', { name: /Connection: connected/ })).toBeHidden()
})
