import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetSent: Array<Record<string, unknown>>
  __yeetEmit: (message: Record<string, unknown>) => void
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

test('model picker prefers structured provider and model metadata', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: {
      active_model: 'opaque-a',
      available_models: ['opaque-a', 'opaque-b'],
      model_catalog: [
        { id: 'opaque-a', provider: 'openai', model: 'gpt-5.6-sol', context_length: 200000 },
        { id: 'opaque-b', provider: 'anthropic', model: 'claude-opus-4.1', context_length: 180000 },
      ],
    },
  })

  const picker = page.locator('.top-bar').getByTestId('model-picker')
  const trigger = picker.getByTestId('model-picker-trigger')
  await expect(trigger).toHaveAttribute('aria-label', /Model: gpt-5\.6-sol, provider: OpenAI/)
  await trigger.click()
  await expect.poll(async () => (await sentCommands(page)).filter((command) => command.type === 'request_models').length).toBeGreaterThanOrEqual(2)
  await picker.getByTestId('model-search').fill('opus')
  const option = picker.getByRole('option')
  await expect(option).toHaveCount(1)
  await expect(option).toContainText('claude-opus-4.1')
  await expect(option).toContainText('Claude (Anthropic)')
  await expect(option).toContainText('180k context')
  await picker.getByTestId('model-search').press('Enter')

  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'select_model', model: 'opaque-b' }),
  ]))
})

test('model picker keeps Codex CLI models separate from OpenAI models', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: {
      active_model: 'codex-cli/gpt-5.6-codex',
      available_models: ['openai/gpt-5.6-sol', 'codex-cli/gpt-5.6-codex'],
      model_catalog: [
        { id: 'openai/gpt-5.6-sol', provider: 'openai', model: 'gpt-5.6-sol', context_length: 200000 },
        { id: 'codex-cli/gpt-5.6-codex', provider: 'codex-cli', model: 'gpt-5.6-codex', context_length: 200000 },
      ],
    },
  })

  const picker = page.locator('.top-bar').getByTestId('model-picker')
  await expect(picker.getByTestId('model-picker-trigger')).toHaveAttribute('aria-label', /provider: Codex CLI/)
  await picker.getByTestId('model-picker-trigger').click()
  const popover = picker.getByTestId('model-picker-popover')
  await expect(popover.getByRole('button', { name: 'OpenAI' })).toBeVisible()
  await expect(popover.getByRole('button', { name: 'Codex CLI' })).toBeVisible()
  await expect(picker.getByRole('option', { name: /gpt-5\.6-codex.*Codex CLI/ })).toBeVisible()
})

test('desktop Infinity toggle sends the semantic command and reflects state', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  const toggle = page.getByTestId('toggle-infinity')
  await expect(toggle).toHaveAttribute('aria-pressed', 'false')
  await toggle.click()
  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'set_infinity', enabled: true }),
  ]))

  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: { infinity_mode: true },
  })
  await expect(toggle).toHaveAttribute('aria-pressed', 'true')
})

test('desktop session controls use a compact dialog and omit unsupported attachments', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await expect(page.getByLabel('Attach files')).not.toBeAttached()
  await page.getByRole('button', { name: /^Session controls:/ }).click()

  const panel = page.getByTestId('session-controls')
  await expect(panel).toBeVisible()
  await expect(panel).toHaveAttribute('role', 'dialog')
  await expect(panel.locator('.mobile-sheet')).not.toBeAttached()

  const box = await panel.boundingBox()
  const viewport = page.viewportSize()
  expect(box).not.toBeNull()
  expect(viewport).not.toBeNull()
  expect(box?.width ?? 999).toBeLessThanOrEqual(430)
  expect(box?.y ?? 0).toBeGreaterThan(20)
  expect((box?.y ?? 0) + (box?.height ?? 0)).toBeLessThan((viewport?.height ?? 0) - 20)

  await expect(panel).toContainText('high')
  await expect(panel).toContainText('Ask first')
})

test('runtime settings reuse the model picker and gate OpenAI Flex by provider', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await page.goto('/settings/runtime')
  await expect(page.getByRole('heading', { name: 'Runtime' })).toBeVisible()

  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: {
      active_model: 'opaque-b',
      available_models: ['opaque-a', 'opaque-b'],
      model_catalog: [
        { id: 'opaque-a', provider: 'openai', model: 'gpt-5.6-sol', context_length: 200000 },
        { id: 'opaque-b', provider: 'anthropic', model: 'claude-opus-4.1', context_length: 180000 },
      ],
    },
  })

  const runtimePicker = page.getByTestId('runtime-model-picker').getByTestId('model-picker-trigger')
  await expect(runtimePicker).toHaveAttribute('aria-label', /Model: claude-opus-4\.1, provider: Claude \(Anthropic\)/)
  await expect(page.getByText('OpenAI Flex')).not.toBeAttached()
  await expect(page.getByRole('button', { name: /ask →/i })).toBeVisible()

  await emit(page, {
    type: 'state_update', version: 1, sequence: 3, revision: 3,
    patch: { active_model: 'opaque-a' },
  })
  await expect(page.getByText('OpenAI Flex')).toBeVisible()
})
