import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('workspace path editor communicates its state and Escape restores the opener', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 0) < 900)

  const sidebar = page.locator('.desktop-session-sidebar')
  const opener = sidebar.getByTestId('open-workspace-path')

  await expect(opener).toHaveAttribute('aria-expanded', 'false')
  await expect(opener).toHaveAccessibleName('Open workspace by path')
  await opener.click()

  const editor = sidebar.getByTestId('workspace-open-form')
  const input = sidebar.getByTestId('workspace-path-input')
  await expect(editor).toBeVisible()
  await expect(opener).toHaveAttribute('aria-expanded', 'true')
  await expect(opener).toHaveAccessibleName('Cancel open workspace')
  await expect(opener).toHaveText('×')
  await expect(input).toBeFocused()

  await input.fill('~/Code/example')
  await input.press('Escape')

  await expect(editor).toBeHidden()
  await expect(opener).toBeFocused()
  await expect(opener).toHaveAttribute('aria-expanded', 'false')
  await expect(opener).toHaveAccessibleName('Open workspace by path')
})

test('mobile workspace path Escape closes only the nested editor before the sessions dialog', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)

  const sessionsOpener = page.getByTestId('open-sessions')
  await sessionsOpener.click()

  const drawer = page.getByTestId('session-drawer')
  const sidebar = drawer.locator('.session-sidebar.is-drawer')
  const workspaceOpener = sidebar.getByTestId('open-workspace-path')
  await expect(drawer).toBeVisible()
  await workspaceOpener.click()

  const editor = sidebar.getByTestId('workspace-open-form')
  const input = sidebar.getByTestId('workspace-path-input')
  await expect(input).toBeFocused()
  await input.press('Escape')

  await expect(editor).toBeHidden()
  await expect(drawer).toBeVisible()
  await expect(workspaceOpener).toBeFocused()

  await page.keyboard.press('Escape')
  await expect(drawer).toBeHidden()
  await expect(sessionsOpener).toBeFocused()
})
