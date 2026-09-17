import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('narrow activity inspector behaves as a focus-contained dialog', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)

  const invoker = page.getByTestId('toggle-inspector')
  const inspector = page.getByTestId('activity-inspector')
  const shell = page.locator('.conversation-pane')

  await invoker.click()
  await expect(inspector).toBeVisible()
  await expect(inspector).toBeFocused()
  await expect(inspector).toHaveAttribute('role', 'dialog')
  await expect(inspector).toHaveAttribute('aria-modal', 'true')
  await expect(inspector).toHaveAttribute('aria-labelledby', 'activity-inspector-title')
  await expect(shell).toHaveJSProperty('inert', true)
  await expect(shell).toHaveAttribute('aria-hidden', 'true')

  await page.keyboard.press('Tab')
  await expect(inspector.getByRole('button', { name: 'Close inspector' })).toBeFocused()

  const enabledButtons = inspector.locator('button:not(:disabled):visible')
  await enabledButtons.last().focus()
  await page.keyboard.press('Tab')
  await expect(inspector.getByRole('button', { name: 'Close inspector' })).toBeFocused()

  await page.keyboard.press('Shift+Tab')
  await expect(enabledButtons.last()).toBeFocused()

  await page.keyboard.press('Escape')
  await expect(inspector).toBeHidden()
  await expect(invoker).toBeFocused()
  await expect(shell).toHaveJSProperty('inert', false)
  await expect(shell).not.toHaveAttribute('aria-hidden', 'true')
})

test('narrow activity backdrop closes the inspector and restores touch opener focus', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)

  const invoker = page.getByTestId('toggle-inspector')
  await invoker.click()
  await expect(page.getByTestId('activity-inspector')).toBeFocused()

  await page.locator('.mobile-inspector-backdrop').click({ position: { x: 2, y: 2 } })
  await expect(page.getByTestId('activity-inspector')).toBeHidden()
  await expect(invoker).toBeFocused()
})

test('wide activity inspector stays modeless and leaves the conversation interactive', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 0) < 1200)

  const inspector = page.getByTestId('activity-inspector')
  const shell = page.locator('.conversation-pane')
  await page.getByTestId('toggle-inspector').click()

  await expect(inspector).toBeVisible()
  await expect(inspector).not.toHaveAttribute('role', 'dialog')
  await expect(inspector).not.toHaveAttribute('aria-modal', 'true')
  await expect(shell).toHaveJSProperty('inert', false)
  await expect(shell).not.toHaveAttribute('aria-hidden', 'true')
})

test('overlay activity inspector is modal before the docked breakpoint', async ({ page }) => {
  const width = page.viewportSize()?.width ?? 0
  test.skip(width < 900 || width >= 1200)

  const invoker = page.getByTestId('toggle-inspector')
  const inspector = page.getByTestId('activity-inspector')
  const shell = page.locator('.conversation-pane')
  const backdrop = page.locator('.mobile-inspector-backdrop')

  await invoker.click()
  await expect(inspector).toBeVisible()
  await expect(inspector).toBeFocused()
  await expect(inspector).toHaveAttribute('role', 'dialog')
  await expect(inspector).toHaveAttribute('aria-modal', 'true')
  await expect(shell).toHaveJSProperty('inert', true)
  await expect(shell).toHaveAttribute('aria-hidden', 'true')
  await expect(backdrop).toBeVisible()

  await backdrop.click({ position: { x: 2, y: 2 } })
  await expect(inspector).toBeHidden()
  await expect(invoker).toBeFocused()
  await expect(shell).toHaveJSProperty('inert', false)
})
