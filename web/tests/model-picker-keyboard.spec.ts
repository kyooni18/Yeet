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

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('model search exposes combobox semantics without putting every model in the Tab order', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: {
      active_model: 'openai/gpt-5.6-sol',
      available_models: [
        'openai/gpt-5.6-sol',
        'openai/gpt-5.6-luna',
        'anthropic/claude-opus-4.1',
        'google/gemini-3-pro',
      ],
    },
  })

  const picker = page.locator('.top-bar').getByTestId('model-picker')
  await picker.getByTestId('model-picker-trigger').click()

  const search = picker.getByTestId('model-search')
  const options = picker.getByRole('option')
  await expect(search).toBeFocused()
  await expect(search).toHaveAttribute('role', 'combobox')
  await expect(search).toHaveAttribute('aria-autocomplete', 'list')
  await expect(search).toHaveAttribute('aria-expanded', 'true')
  await expect(search).toHaveAttribute('aria-activedescendant', /-option-\d+$/)
  await expect(options).toHaveCount(4)

  for (let index = 0; index < await options.count(); index += 1) {
    await expect(options.nth(index)).toHaveAttribute('tabindex', '-1')
  }

  await page.keyboard.press('Tab')
  await expect(picker.getByRole('button', { name: 'All', exact: true })).toBeFocused()

  await page.keyboard.press('Tab')
  await expect(picker.getByRole('button', { name: 'OpenAI', exact: true })).toBeFocused()
  await page.keyboard.press('Tab')
  await expect(picker.getByRole('button', { name: 'Claude (Anthropic)', exact: true })).toBeFocused()
  await page.keyboard.press('Tab')
  await expect(picker.getByRole('button', { name: 'Google', exact: true })).toBeFocused()

  await page.keyboard.press('Tab')
  await expect(picker.getByTestId('model-picker-popover')).not.toBeAttached()
  await expect.poll(async () => picker.evaluate((root) => !root.contains(document.activeElement))).toBe(true)
})

test('model options remain selectable by arrow navigation and pointer after leaving them out of Tab order', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  const picker = page.locator('.top-bar').getByTestId('model-picker')
  const trigger = picker.getByTestId('model-picker-trigger')
  await trigger.focus()
  await trigger.press('ArrowDown')

  const search = picker.getByTestId('model-search')
  await expect(search).toBeFocused()
  const activeId = await search.getAttribute('aria-activedescendant')
  expect(activeId).toBeTruthy()
  await search.press('ArrowDown')
  await search.press('Enter')
  await expect(picker.getByTestId('model-picker-popover')).not.toBeAttached()
  await expect(trigger).toBeFocused()

  await trigger.click()
  const option = picker.getByRole('option').last()
  const model = await option.getAttribute('data-model')
  expect(model).toBeTruthy()
  await option.click()
  await expect(picker.getByTestId('model-picker-popover')).not.toBeAttached()
  await expect(trigger).toBeFocused()
  await expect.poll(async () => (await sentCommands(page))
    .filter((command) => command.type === 'select_model' && command.model === model).length).toBeGreaterThanOrEqual(2)
})


test('search resets the active option to the best filtered match before Enter selection', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: {
      active_model: 'anthropic/claude-opus-4.1',
      available_models: [
        'openai/gpt-5.6-sol',
        'openai/gpt-5.6-luna',
        'openai/gpt-4.1',
        'anthropic/claude-opus-4.1',
      ],
    },
  })

  const picker = page.locator('.top-bar').getByTestId('model-picker')
  await picker.getByTestId('model-picker-trigger').click()
  const search = picker.getByTestId('model-search')
  await search.fill('gpt')

  const options = picker.getByRole('option')
  await expect(options).toHaveCount(3)
  await expect(search).toHaveAttribute('aria-activedescendant', await options.first().getAttribute('id') ?? '')

  await search.press('Enter')
  await expect.poll(async () => (await sentCommands(page))
    .filter((command) => command.type === 'select_model')
    .at(-1)?.model).toBe('openai/gpt-5.6-sol')
})


test('Escape closes the picker from a focused provider filter and restores the trigger', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: {
      active_model: 'openai/gpt-5.6-sol',
      available_models: [
        'openai/gpt-5.6-sol',
        'anthropic/claude-opus-4.1',
        'google/gemini-3-pro',
      ],
    },
  })

  const picker = page.locator('.top-bar').getByTestId('model-picker')
  const trigger = picker.getByTestId('model-picker-trigger')
  await trigger.click()
  await picker.getByTestId('model-search').press('Tab')
  const providerFilter = picker.getByRole('button', { name: 'All', exact: true })
  await expect(providerFilter).toBeFocused()

  await providerFilter.press('Escape')

  await expect(picker.getByTestId('model-picker-popover')).not.toBeAttached()
  await expect(trigger).toBeFocused()
})
