import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('wide inspector close restores focus to its invoking toggle', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 0) < 900)

  const invoker = page.getByTestId('toggle-inspector')
  const inspector = page.getByTestId('activity-inspector')

  await expect(invoker).toHaveAttribute('aria-controls', 'activity-inspector')
  await expect(inspector).toHaveAttribute('id', 'activity-inspector')
  await invoker.focus()
  await invoker.click()
  await expect(inspector).toBeVisible()

  const close = inspector.getByRole('button', { name: 'Close inspector' })
  await close.focus()
  await close.click()

  await expect(inspector).toBeHidden()
  await expect(invoker).toBeFocused()
})


test('wide inspector Escape closes the panel and restores its invoking toggle', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 0) < 900)

  const invoker = page.getByTestId('toggle-inspector')
  const inspector = page.getByTestId('activity-inspector')

  await invoker.focus()
  await invoker.click()
  await expect(inspector).toBeVisible()

  await inspector.getByRole('button', { name: 'Close inspector' }).focus()
  await page.keyboard.press('Escape')

  await expect(inspector).toBeHidden()
  await expect(invoker).toBeFocused()
})
