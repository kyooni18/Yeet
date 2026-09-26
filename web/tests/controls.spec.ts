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
    return sent
      .filter((item) => item.type === 'command')
      .map((item) => item.command as Record<string, unknown>)
  })
}

async function openModelSheet(page: Page) {
  await page.getByRole('button', { name: /Choose model, current/ }).click()
  const dialog = page.getByRole('dialog', { name: 'Choose model' })
  await expect(dialog).toBeVisible()
  return dialog
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('model sheet prefers structured provider/model metadata and context lengths', async ({ page }) => {
  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: {
      active_model: 'opaque-a',
      available_models: ['opaque-a', 'opaque-b'],
      model_catalog: [
        { id: 'opaque-a', provider: 'openai', model: 'gpt-5.6-sol', context_length: 200000 },
        { id: 'opaque-b', provider: 'anthropic', model: 'claude-opus-4.1', context_length: 180000 },
      ],
    },
  })

  const dialog = await openModelSheet(page)
  await expect(dialog.locator('.active-model-card')).toContainText('gpt-5.6-sol')
  await expect(dialog.locator('.active-model-card')).toContainText('OpenAI')
  await expect(dialog.locator('.active-model-card')).toContainText('200k ctx')

  const search = dialog.getByPlaceholder('Search models')
  await search.fill('opus')
  const row = dialog.locator('.model-row')
  await expect(row).toHaveCount(1)
  await expect(row).toContainText('claude-opus-4.1')
  await expect(row).toContainText('Anthropic')
  await expect(row).toContainText('180k ctx')
  await row.click()

  await expect(dialog).toHaveCount(0)
  await expect.poll(async () => (await sentCommands(page))
    .filter((command) => command.type === 'select_model')
    .at(-1)?.model).toBe('opaque-b')
})

test('model sheet keeps Codex and OpenAI models in separate provider sections', async ({ page }) => {
  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: {
      active_model: 'codex-cli/gpt-5.6-codex',
      available_models: ['openai/gpt-5.6-sol', 'codex-cli/gpt-5.6-codex'],
      model_catalog: [
        { id: 'openai/gpt-5.6-sol', provider: 'openai', model: 'gpt-5.6-sol', context_length: 200000 },
        { id: 'codex-cli/gpt-5.6-codex', provider: 'codex-cli', model: 'gpt-5.6-codex', context_length: 200000 },
      ],
    },
  })

  const dialog = await openModelSheet(page)
  const sections = dialog.locator('.model-section')
  await expect(sections.filter({ hasText: 'OpenAI' })).toHaveCount(1)
  await expect(sections.filter({ hasText: 'Codex' })).toHaveCount(1)
  await expect(sections.filter({ hasText: 'OpenAI' }).locator('.model-row')).toContainText('gpt-5.6-sol')
  await expect(sections.filter({ hasText: 'Codex' }).locator('.model-row')).toContainText('gpt-5.6-codex')
})

test('Goal control sends the semantic command and reflects remote state', async ({ page }) => {
  const toggle = page.getByRole('button', { name: 'Goal' })
  await expect(toggle).toHaveAttribute('aria-pressed', 'false')

  await toggle.click()
  await expect.poll(async () => (await sentCommands(page))
    .filter((command) => command.type === 'set_goal')
    .at(-1)?.enabled).toBe(true)

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: { goal_mode: true },
  })
  await expect(toggle).toHaveAttribute('aria-pressed', 'true')
})

test('QuickPanel presents compact model, reasoning, goal, workspace, and sandbox controls', async ({ page }) => {
  await page.getByRole('button', { name: 'Quick settings' }).click()
  const panel = page.locator('.quick-panel')
  await expect(panel).toHaveClass(/is-open/)

  const box = await panel.boundingBox()
  const viewport = page.viewportSize()
  expect(box).not.toBeNull()
  expect(viewport).not.toBeNull()
  expect(box!.width).toBeLessThanOrEqual(430)
  expect(box!.height).toBeLessThanOrEqual(viewport!.height)

  await expect(panel.getByRole('button', { name: /Model/ })).toBeVisible()
  await expect(panel.getByRole('combobox', { name: 'Reasoning' })).toBeVisible()
  await expect(panel.getByRole('switch', { name: 'Goal mode' })).toBeVisible()
  await expect(panel.getByRole('combobox', { name: 'Workspace' })).toBeVisible()

  const sandboxPreset = panel.getByRole('combobox', { name: 'Sandbox preset' })
  if (await sandboxPreset.count()) await expect(sandboxPreset).toBeVisible()
})

test('Settings sheet sends Remote-backed settings mutations', async ({ page }) => {
  await page.getByRole('button', { name: 'Quick settings' }).click()
  const panel = page.locator('.quick-panel')
  await panel.getByRole('button', { name: 'Settings', exact: true }).click()

  const dialog = page.getByRole('dialog', { name: 'Settings' })
  await expect(dialog).toBeVisible()

  const flex = dialog.getByRole('switch', { name: 'OpenAI Flex' })
  const memory = dialog.getByRole('switch', { name: 'Foundation Memory' })
  await expect(flex).toBeEnabled()
  await expect(memory).toBeEnabled()

  const flexWasEnabled = await flex.getAttribute('aria-checked') === 'true'
  await flex.click()
  await expect.poll(async () => (await sentCommands(page))
    .filter((command) => command.type === 'set_open_ai_flex')
    .at(-1)?.enabled).toBe(!flexWasEnabled)

  const memoryWasEnabled = await memory.getAttribute('aria-checked') === 'true'
  await memory.click()
  await expect.poll(async () => (await sentCommands(page))
    .filter((command) => command.type === 'set_foundation_memory')
    .at(-1)?.enabled).toBe(!memoryWasEnabled)
})
