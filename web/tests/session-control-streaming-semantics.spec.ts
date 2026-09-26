import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
  __yeetSent: Array<Record<string, unknown>>
}

async function emit(page: Page, message: Record<string, unknown>) {
  await page.evaluate((payload) => {
    ;(window as unknown as TestHooks).__yeetEmit(payload)
  }, message)
}

async function sentCommands(page: Page) {
  return page.evaluate(() => {
    const sent = (window as unknown as TestHooks).__yeetSent
    return sent
      .filter((item) => item.type === 'command')
      .map((item) => item.command as Record<string, unknown>)
  })
}

test('streaming keeps next-response controls mutable while message submission stays blocked', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: { is_streaming: true, active_run_id: 'run-live' },
  })

  const model = page.getByRole('button', { name: /Choose model, current/ })
  const reasoning = page.getByRole('combobox', { name: 'Reasoning' }).first()
  const goal = page.getByRole('button', { name: 'Goal' })
  const composer = page.getByRole('textbox', { name: 'Message' })

  await expect(model).toBeEnabled()
  await expect(model).toHaveAttribute('title', 'Model changes apply to the next response.')
  await expect(reasoning).toBeEnabled()
  await expect(goal).toBeEnabled()
  await expect(goal).toHaveAttribute('title', 'Goal changes apply to the next response.')

  const submitsBefore = (await sentCommands(page)).filter((command) => command.type === 'submit').length
  await composer.fill('queued follow-up')
  await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Send' })).toHaveCount(0)
  await composer.press('Enter')
  await expect.poll(async () =>
    (await sentCommands(page)).filter((command) => command.type === 'submit').length
  ).toBe(submitsBefore)

  await reasoning.selectOption('medium')
  await expect.poll(async () => (await sentCommands(page))
    .filter((command) => command.type === 'select_reasoning')
    .at(-1)?.level).toBe('medium')

  await goal.click()
  await expect.poll(async () => (await sentCommands(page))
    .filter((command) => command.type === 'set_goal')
    .at(-1)?.enabled).toBe(true)

  await page.getByRole('button', { name: 'Quick settings' }).click()
  const panel = page.locator('.quick-panel')
  await expect(panel).toHaveClass(/is-open/)
  await expect(panel.getByText('Working', { exact: true })).toBeVisible()
  await expect(panel.getByRole('button', { name: /Model/ })).toBeEnabled()
  await expect(panel.getByRole('combobox', { name: 'Reasoning' })).toBeEnabled()
  await expect(panel.getByRole('switch', { name: 'Goal mode' })).toBeEnabled()
})
