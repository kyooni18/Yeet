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

test('shared live reasoning displays its full details separately from activity', async ({ page }) => {
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

  const reasoning = page.locator('.conversation-content > .trace-disclosure').filter({ hasText: 'Thinking' })
  const row = reasoning.locator('.trace-disclosure__row')
  await expect(row).toContainText('Comparing the responsive layout')
  await expect(row).toHaveAttribute('aria-expanded', 'true')
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

  const reasoning = page.locator('.conversation-content > .trace-disclosure').filter({ hasText: 'Thinking' })
  const row = reasoning.locator('.trace-disclosure__row')
  await expect(row).toContainText('Checking live sources')
  await expect(row).not.toContainText('**')
  await expect(reasoning.locator('.trace-disclosure__details')).toContainText('Checking live sources')
  await expect(reasoning.locator('.trace-disclosure__details')).not.toContainText('**')
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


test('hidden reasoning and provider-managed tools show the live phase without invented reasoning', async ({ page }) => {
  await emit(page, { type: 'conversation_entry', version: 1, sequence: 2, revision: 2,
    entry: { id: 'provider-phase', kind: { type: 'activity', activity: { phase: 'reasoning', title: 'Thinking', detail: 'Waiting for a model summary' } } },
  })
  await emit(page, { type: 'state_update', version: 1, sequence: 3, revision: 3,
    patch: { is_streaming: true, active_activity_entry_id: 'provider-phase' },
  })
  const header = page.locator('.activity-group__header').last()
  await expect(header).toContainText('Thinking')
  await expect(page.locator('.activity-group__summary').last()).toHaveClass(/is-active/)
  await emit(page, { type: 'conversation_entry', version: 1, sequence: 4, revision: 4,
    entry: { id: 'provider-phase', kind: { type: 'activity', activity: { phase: 'provider-tool', title: 'Searching web', detail: 'Provider-managed tool' } } },
  })
  await expect(header).toContainText('Searching web')
})
