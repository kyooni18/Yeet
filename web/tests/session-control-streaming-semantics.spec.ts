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
    return sent.filter((item) => item.type === 'command').map((item) => item.command as Record<string, unknown>)
  })
}

test('streaming model and reasoning controls explain next-response timing without becoming read-only', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()

  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: { is_streaming: true, active_run_id: 'run-live' },
  })

  const picker = page.locator('.top-bar').getByTestId('model-picker')
  await picker.getByTestId('model-picker-trigger').click()
  await expect(picker.getByText('Model changes apply to the next response.')).toBeVisible()
  await expect(picker.getByRole('option').first()).toBeEnabled()
  await picker.getByTestId('model-search').press('Escape')

  await page.getByRole('button', { name: /^Session controls:/ }).click()
  const sheet = page.getByTestId('status-sheet')
  const fields = sheet.locator('.session-control-field')
  const modelField = fields.filter({ hasText: 'Model' }).first()
  const reasoningField = fields.filter({ hasText: 'Reasoning' }).first()
  await expect(modelField.getByText('Next response', { exact: true })).toBeVisible()
  await expect(reasoningField.getByText('Next response', { exact: true })).toBeVisible()

  const medium = reasoningField.getByRole('button', { name: 'medium', exact: true })
  await expect(medium).toBeEnabled()
  await medium.click()
  await expect.poll(async () => (await sentCommands(page))
    .filter((command) => command.type === 'select_reasoning')
    .at(-1)?.level).toBe('medium')
})
