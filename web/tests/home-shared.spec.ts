import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'
import { projectHome } from './uiHomeHarness'

test('existing session sidebar renders shared Rust Home rows and opens through a host intent', async ({ page }) => {
  const sessions = [{ id: 'session-a', title: 'First shared session' }, { id: 'session-b', title: 'Second shared session' }]
  const projected = await projectHome({ sessions })
  const opened = await projectHome({ sessions, action: { type: 'open', value: { type: 'session', value: 'session-b' } } })
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  const sidebar = page.locator('.remote-sidebar')
  if (!(await sidebar.evaluate(element => element.classList.contains('is-open')))) {
    await page.getByRole('button', { name: 'Open sidebar' }).click()
  }
  await expect(sidebar).toHaveClass(/is-open/)
  await page.evaluate((home) => {
    (window as unknown as { __yeetEmit(message: unknown): void }).__yeetEmit({
      type: 'ui_home', version: 1, home_revision: home.home_revision, view: home.view,
    })
  }, projected)
  await expect(sidebar.locator('[data-session-id="session-a"]')).toContainText('First shared session')
  await expect(sidebar.locator('[data-session-id="session-b"]')).toContainText('Second shared session')
  await sidebar.locator('[data-session-id="session-b"]').click()
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __yeetSent: Array<Record<string, unknown>> }).__yeetSent
      .filter(message => message.type === 'ui_home_action').at(-1)?.action,
  )).toEqual({ type: 'open', value: { type: 'session', value: 'session-b' } })

  await page.evaluate((open) => {
    (window as unknown as { __yeetEmit(message: unknown): void }).__yeetEmit({
      type: 'ui_home_effect', version: 1, open,
    })
  }, opened.open)
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __yeetSent: Array<Record<string, unknown>> }).__yeetSent
      .filter(message => message.type === 'command').at(-1)?.command,
  )).toEqual(expect.objectContaining({ type: 'load_session', session_id: 'session-b' }))
})
