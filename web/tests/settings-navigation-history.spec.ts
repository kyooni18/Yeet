import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

async function openSettings(page: Parameters<typeof installMockRemote>[0]) {
  const sidebar = page.locator('.remote-sidebar')
  const isOpen = (await sidebar.getAttribute('class'))?.includes('is-open') ?? false
  if (!isOpen) {
    await page.getByRole('button', { name: 'Open sidebar' }).click()
    await expect(sidebar).toHaveClass(/is-open/)
    await sidebar.evaluate(async (element) => {
      await Promise.all(element.getAnimations().map((animation) => animation.finished.catch(() => undefined)))
    })
  }
  await sidebar.getByRole('button', { name: 'Settings', exact: true }).click()
  const settings = page.getByRole('dialog', { name: 'Settings' })
  await expect(settings).toBeVisible()
  return settings
}

test('Settings is an in-place sheet that can be dismissed and reopened without history changes', async ({ page }) => {
  const before = await page.evaluate(() => ({ href: location.href, length: history.length }))

  let settings = await openSettings(page)
  await settings.getByRole('button', { name: 'Close' }).click()
  await expect(settings).toHaveCount(0)

  settings = await openSettings(page)
  await page.keyboard.press('Escape')
  await expect(settings).toHaveCount(0)

  const after = await page.evaluate(() => ({ href: location.href, length: history.length }))
  expect(after).toEqual(before)
  await expect(page.getByRole('textbox', { name: 'Message' })).toBeVisible()
})
