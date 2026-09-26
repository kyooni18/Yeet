import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
  __yeetSent: Array<Record<string, unknown>>
}

async function emitModels(page: Page) {
  await page.evaluate(() => {
    ;(window as unknown as TestHooks).__yeetEmit({
      type: 'state_update',
      version: 1,
      sequence: 2,
      revision: 2,
      patch: {
        active_model: 'openai/gpt-5.6-sol',
        available_models: [
          'openai/gpt-5.6-sol',
          'openai/gpt-5.6-luna',
          'anthropic/claude-opus-4.1',
          'google/gemini-3-pro',
        ],
        model_catalog: [],
      },
    })
  })
}

async function sentCommands(page: Page) {
  return page.evaluate(() => {
    const sent = (window as unknown as TestHooks).__yeetSent
    return sent
      .filter((item) => item.type === 'command')
      .map((item) => item.command as Record<string, unknown>)
  })
}

async function openModelSheet(page: Page) {
  const trigger = page.getByRole('button', { name: /Choose model, current/ })
  await trigger.focus()
  await trigger.click()

  const dialog = page.getByRole('dialog', { name: 'Choose model' })
  const search = dialog.getByPlaceholder('Search models')
  await expect(dialog).toBeVisible()
  await expect(dialog).toHaveAttribute('aria-modal', 'true')
  await expect(search).toBeFocused()
  return { trigger, dialog, search }
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await emitModels(page)
})

test('model sheet search starts focused and model rows remain keyboard reachable', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  const { dialog, search } = await openModelSheet(page)
  const rows = dialog.locator('.model-row')
  await expect(rows).toHaveCount(4)

  await search.press('Tab')
  await expect(rows.first()).toBeFocused()

  await page.keyboard.press('Shift+Tab')
  await expect(search).toBeFocused()
})

test('keyboard activation selects a model, closes the sheet, and restores the trigger', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  const { trigger, dialog, search } = await openModelSheet(page)
  const target = dialog.locator('.model-row').filter({ hasText: 'gpt-5.6-luna' })

  await search.press('Tab')
  await expect(dialog.locator('.model-row').first()).toBeFocused()
  await page.keyboard.press('Tab')
  await expect(target).toBeFocused()
  await page.keyboard.press('Enter')

  await expect(dialog).toHaveCount(0)
  await expect(trigger).toBeFocused()
  await expect.poll(async () => (await sentCommands(page))
    .filter((command) => command.type === 'select_model')
    .at(-1)?.model).toBe('openai/gpt-5.6-luna')
})

test('search filters the visible model rows before keyboard selection', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  const { dialog, search } = await openModelSheet(page)
  await search.fill('gemini')

  const rows = dialog.locator('.model-row')
  await expect(rows).toHaveCount(1)
  await expect(rows.first()).toContainText('gemini-3-pro')
  await rows.first().focus()
  await expect(rows.first()).toBeFocused()
  await page.keyboard.press('Enter')

  await expect(dialog).toHaveCount(0)
  await expect.poll(async () => (await sentCommands(page))
    .filter((command) => command.type === 'select_model')
    .at(-1)?.model).toBe('google/gemini-3-pro')
})

test('Escape closes the sheet from a focused model row and restores the trigger', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  const { trigger, dialog } = await openModelSheet(page)
  const row = dialog.locator('.model-row').last()
  await row.focus()
  await expect(row).toBeFocused()

  await page.keyboard.press('Escape')

  await expect(dialog).toHaveCount(0)
  await expect(trigger).toBeFocused()
})
