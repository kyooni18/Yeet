import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('model sheet traps focus and restores the model trigger on Escape', async ({ page }) => {
  const invoker = page.getByRole('button', { name: /Choose model, current/ })
  await invoker.focus()
  await invoker.click()

  const dialog = page.getByRole('dialog', { name: 'Choose model' })
  const search = dialog.getByPlaceholder('Search models')
  await expect(dialog).toBeVisible()
  await expect(dialog).toHaveAttribute('aria-modal', 'true')
  await expect(search).toBeFocused()

  const focusables = dialog.locator('button:visible, input:visible')
  const last = focusables.last()
  await last.focus()
  await page.keyboard.press('Tab')
  await expect.poll(() => dialog.evaluate((element) => element.contains(document.activeElement))).toBe(true)

  await page.keyboard.press('Shift+Tab')
  await expect.poll(() => dialog.evaluate((element) => element.contains(document.activeElement))).toBe(true)

  await page.keyboard.press('Escape')
  await expect(dialog).toHaveCount(0)
  await expect(invoker).toBeFocused()
})

test('model backdrop dismissal restores focus to its invoker', async ({ page }) => {
  const invoker = page.getByRole('button', { name: /Choose model, current/ })
  await invoker.click()
  const dialog = page.getByRole('dialog', { name: 'Choose model' })
  await expect(dialog).toBeVisible()

  await page.locator('.sheet-layer').click({ position: { x: 2, y: 2 } })
  await expect(dialog).toHaveCount(0)
  await expect(invoker).toBeFocused()
})

test('session drawer owns keyboard focus and restores its opener', async ({ page }) => {
  const desktopDocked = await page.evaluate(() =>
    matchMedia('(min-width: 1000px) and (hover: hover) and (pointer: fine)').matches
  )
  test.skip(desktopDocked, 'Desktop uses a non-modal session dock.')
  const invoker = page.getByRole('button', { name: 'Open sidebar' })
  await invoker.click()

  const dialog = page.getByRole('dialog', { name: 'Sessions' })
  await expect(dialog).toBeVisible()
  await expect(dialog).toHaveAttribute('aria-modal', 'true')
  await expect.poll(() => dialog.evaluate((element) => element.contains(document.activeElement))).toBe(true)

  for (let index = 0; index < 8; index += 1) {
    await page.keyboard.press('Tab')
    await expect.poll(() => dialog.evaluate((element) => element.contains(document.activeElement))).toBe(true)
  }

  await page.keyboard.press('Escape')
  await expect(page.locator('.remote-sidebar')).not.toHaveClass(/is-open/)
  await expect(invoker).toBeFocused()
})

test('session backdrop closes the drawer and returns focus to its opener', async ({ page }) => {
  const desktopDocked = await page.evaluate(() =>
    matchMedia('(min-width: 1000px) and (hover: hover) and (pointer: fine)').matches
  )
  test.skip(desktopDocked, 'Desktop uses a non-modal session dock.')
  const invoker = page.getByRole('button', { name: 'Open sidebar' })
  await invoker.click()
  await expect(page.getByRole('dialog', { name: 'Sessions' })).toBeVisible()

  const backdrop = page.locator('.sidebar-backdrop')
  const backdropBox = await backdrop.boundingBox()
  expect(backdropBox).not.toBeNull()
  await backdrop.click({ position: { x: backdropBox!.width - 2, y: 2 } })
  await expect(page.locator('.remote-sidebar')).not.toHaveClass(/is-open/)
  await expect(invoker).toBeFocused()
})

test('Settings is a keyboard-contained modal and returns focus to its opener', async ({ page }) => {
  await page.getByRole('button', { name: 'Quick settings' }).click()
  const panel = page.locator('.quick-panel')
  const invoker = panel.getByRole('button', { name: 'Settings', exact: true })
  await invoker.click()

  const dialog = page.getByRole('dialog', { name: 'Settings' })
  await expect(dialog).toBeVisible()
  await expect(dialog).toHaveAttribute('aria-modal', 'true')
  await expect.poll(() => dialog.evaluate((element) => element.contains(document.activeElement))).toBe(true)

  const focusables = dialog.locator('button:visible, input:visible, select:visible')
  const last = focusables.last()
  await last.focus()
  await page.keyboard.press('Tab')
  await expect.poll(() => dialog.evaluate((element) => element.contains(document.activeElement))).toBe(true)

  await page.keyboard.press('Escape')
  await expect(dialog).toHaveCount(0)
  await expect(invoker).toBeFocused()
})
