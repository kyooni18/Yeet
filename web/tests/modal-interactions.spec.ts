import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('session controls owns focus, isolates the shell, and restores the invoker', async ({ page }) => {
  const invoker = page.locator('.composer-context')
  const panel = page.getByTestId('session-controls')
  const shell = page.locator('.conversation-pane')
  const sidebar = page.locator('.desktop-session-sidebar')

  await invoker.focus()
  await invoker.click()

  await expect(panel).toBeVisible()
  await expect(panel).toBeFocused()
  await expect(shell).toHaveJSProperty('inert', true)
  await expect(sidebar).toHaveJSProperty('inert', true)
  await expect(shell).toHaveAttribute('aria-hidden', 'true')

  await page.keyboard.press('Tab')
  await expect(panel.getByRole('button', { name: 'Close session controls' })).toBeFocused()

  await page.keyboard.press('Shift+Tab')
  await expect(panel.getByRole('button', { name: 'All settings' })).toBeFocused()

  await page.keyboard.press('Tab')
  await expect(panel.getByRole('button', { name: 'Close session controls' })).toBeFocused()

  await page.keyboard.press('Escape')
  await expect(page.getByTestId('status-sheet')).toBeHidden()
  await expect(invoker).toBeFocused()
  await expect(shell).toHaveJSProperty('inert', false)
  await expect(shell).not.toHaveAttribute('aria-hidden', 'true')
})

test('backdrop dismissal restores focus without leaking interaction to the shell', async ({ page }) => {
  const invoker = page.locator('.composer-context')
  const layer = page.getByTestId('status-sheet')

  await invoker.click()
  await expect(page.getByTestId('session-controls')).toBeFocused()

  await layer.click({ position: { x: 2, y: 2 } })
  await expect(layer).toBeHidden()
  await expect(invoker).toBeFocused()
})


test('phone session drawer owns focus, traps navigation, and restores its opener', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)

  const invoker = page.getByTestId('open-sessions')
  const drawer = page.getByTestId('session-drawer')
  const shell = page.locator('.conversation-pane')

  await invoker.click()
  await expect(drawer).toBeVisible()
  await expect(drawer).toBeFocused()
  await expect(drawer).toHaveAttribute('role', 'dialog')
  await expect(drawer).toHaveAttribute('aria-modal', 'true')
  await expect(shell).toHaveJSProperty('inert', true)
  await expect(shell).toHaveAttribute('aria-hidden', 'true')

  await page.keyboard.press('Tab')
  await expect(drawer.getByRole('button', { name: 'Close sessions' })).toBeFocused()

  await page.keyboard.press('Shift+Tab')
  await expect(drawer.getByRole('button', { name: 'Settings' })).toBeFocused()

  await page.keyboard.press('Tab')
  await expect(drawer.getByRole('button', { name: 'Close sessions' })).toBeFocused()

  await page.keyboard.press('Escape')
  await expect(drawer).toBeHidden()
  await expect(invoker).toBeFocused()
  await expect(shell).toHaveJSProperty('inert', false)
  await expect(shell).not.toHaveAttribute('aria-hidden', 'true')
})

test('phone session drawer backdrop closes without stealing return focus', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)

  const invoker = page.getByTestId('open-sessions')
  const drawer = page.getByTestId('session-drawer')
  const viewport = page.viewportSize()

  await invoker.click()
  await expect(drawer).toBeFocused()
  await drawer.click({ position: { x: Math.max(1, (viewport?.width ?? 320) - 2), y: 2 } })
  await expect(drawer).toBeHidden()
  await expect(invoker).toBeFocused()
})
