import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
}

async function emit(page: Page, message: Record<string, unknown>) {
  await page.evaluate((payload) => {
    ;(window as unknown as TestHooks).__yeetEmit(payload)
  }, message)
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('live reasoning stays compact in the activity header and expands inline', async ({ page }) => {
  await emit(page, {
    type: 'conversation_entry',
    version: 1,
    sequence: 2,
    revision: 2,
    entry: { id: 'live-reasoning', kind: { type: 'reasoning', content: '', summary: '' } },
  })
  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 3,
    revision: 3,
    patch: { is_streaming: true, active_reasoning_entry_id: 'live-reasoning' },
  })
  await emit(page, {
    type: 'reasoning_delta',
    version: 1,
    sequence: 4,
    revision: 4,
    entry_id: 'live-reasoning',
    delta: '',
    content: 'Comparing the responsive layout',
    summary: false,
    reset: true,
  })

  const group = page.locator('.activity-group').last()
  const header = group.locator('.activity-group__header')
  await expect(header).toContainText('Comparing the responsive layout')
  await expect(header).toHaveAttribute('aria-expanded', 'false')

  await header.click()
  await expect(header).toHaveAttribute('aria-expanded', 'true')
  const reasoning = group.locator('.trace-disclosure').filter({ hasText: 'Thinking' })
  const row = reasoning.locator('.trace-disclosure__row')
  await expect(row).toContainText('Thinking')
  await expect(row).toContainText('Comparing the responsive layout')
  await expect(row).toHaveAttribute('aria-expanded', 'false')

  await row.click()
  await expect(reasoning.locator('.trace-detail__content')).toHaveText('Comparing the responsive layout')
})

test('incomplete streamed reasoning markdown never leaks formatting markers', async ({ page }) => {
  await emit(page, {
    type: 'conversation_entry',
    version: 1,
    sequence: 2,
    revision: 2,
    entry: { id: 'markdown-reasoning', kind: { type: 'reasoning', content: '', summary: '' } },
  })
  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 3,
    revision: 3,
    patch: { is_streaming: true, active_reasoning_entry_id: 'markdown-reasoning' },
  })
  await emit(page, {
    type: 'reasoning_delta',
    version: 1,
    sequence: 4,
    revision: 4,
    entry_id: 'markdown-reasoning',
    delta: '',
    content: '**Checking live sources',
    summary: true,
    reset: true,
  })

  const group = page.locator('.activity-group').last()
  const header = group.locator('.activity-group__header')
  await expect(header).toContainText('Checking live sources')
  await expect(header).not.toContainText('**')

  await header.click()
  const row = group.locator('.trace-disclosure__row').filter({ hasText: 'Thinking' })
  await expect(row).toContainText('Checking live sources')
  await expect(row).not.toContainText('**')
  await row.click()
  await expect(group.locator('.trace-disclosure__details')).toContainText('Checking live sources')
  await expect(group.locator('.trace-disclosure__details')).not.toContainText('**')
})

test('completed tools stay collapsed while failed tools open prominently with status', async ({ page }) => {
  await emit(page, {
    type: 'conversation_reset',
    version: 1,
    sequence: 2,
    revision: 2,
    conversation: [{
      id: 'done-tool-entry',
      kind: {
        type: 'toolCall',
        toolCall: {
          id: 'done-tool',
          name: 'run_shell',
          arguments: JSON.stringify({ command: 'pnpm test' }),
          status: 'completed',
          result: 'ok',
        },
      },
    }],
  })

  let group = page.locator('.activity-group').last()
  let header = group.locator('.activity-group__header')
  await expect(header).toHaveAttribute('aria-expanded', 'false')

  await emit(page, {
    type: 'conversation_reset',
    version: 1,
    sequence: 3,
    revision: 3,
    conversation: [{
      id: 'failed-tool-entry',
      kind: {
        type: 'toolCall',
        toolCall: {
          id: 'failed-tool',
          name: 'run_shell',
          arguments: JSON.stringify({ command: 'pnpm test' }),
          status: 'failed',
          result: '',
          error: 'Compiler error',
        },
      },
    }],
  })

  group = page.locator('.activity-group').last()
  header = group.locator('.activity-group__header')
  await expect(header).toHaveAttribute('aria-expanded', 'true')
  const toolRow = group.locator('.trace-disclosure__row').filter({ hasText: 'Command' })
  await expect(toolRow).toContainText('Failed')
  await expect(toolRow).toContainText('pnpm test')
})
