import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('phone exposes offline state without opening secondary UI', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await page.evaluate(() => window.dispatchEvent(new Event('offline')))

  const status = page.getByRole('status')
  await expect(status).toBeVisible()
  await expect(status).toContainText('Offline')
  await expect(page.getByRole('button', { name: 'Send' })).toBeDisabled()
})

test('fatal transport failure exposes the protocol reason as an alert', async ({ page }) => {
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

  const failure = page.getByRole('alert', { name: 'Connection failed' })
  await expect(failure).toBeVisible()
  await expect(failure).toContainText('protocol_mismatch: Server requires Remote protocol v2.')
})

test('connected phone stays visually quiet', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await expect(page.locator('.connection-toast')).toHaveCount(0)
})
