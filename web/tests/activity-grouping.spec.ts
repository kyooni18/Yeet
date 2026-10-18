import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
}

function toolEntry(id: string, status: 'completed' | 'running' | 'failed', name = 'read_file') {
  return {
    id,
    kind: {
      type: 'toolCall',
      toolCall: {
        id,
        name,
        arguments: JSON.stringify({ path: `src/file-${id}.rs` }),
        status,
      },
    },
  }
}

async function resetConversation(page: Parameters<typeof installMockRemote>[0], conversation: Record<string, unknown>[]) {
  await page.evaluate((entries) => {
    const emit = (window as unknown as TestHooks).__yeetEmit
    emit({ type: 'conversation_reset', version: 1, sequence: 2, revision: 2, conversation: entries })
  }, conversation)
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('short completed activity stays collapsed until requested', async ({ page }) => {
  await resetConversation(page, ['tool-1', 'tool-2', 'tool-3'].map((id) => toolEntry(id, 'completed')))

  const group = page.locator('.activity-group')
  const header = group.locator('.activity-group__header')
  await expect(group).toHaveCount(1)
  await expect(header).toContainText('Read file')
  await expect(header).toContainText('src/file-tool-3.rs')
  await expect(header).not.toContainText('Activity')
  await expect(header).toHaveAttribute('aria-expanded', 'false')
  await expect(group.locator('.activity-group__events')).toHaveCount(0)

  await header.click()
  await expect(header).toHaveAttribute('aria-expanded', 'true')
  await expect(group.locator('.activity-group__event')).toHaveCount(3)
  await expect(group.locator('.trace-disclosure__row')).toHaveCount(3)

  await header.click()
  await expect(header).toHaveAttribute('aria-expanded', 'false')
  await expect(group.locator('.activity-group__events')).toHaveCount(0)
})

test('generic count and Running do not replace the latest meaningful action', async ({ page }) => {
  await resetConversation(page, [
    toolEntry('first', 'completed'),
    toolEntry('last', 'completed'),
    { id: 'opaque-phase', kind: { type: 'activity', activity: { phase: 'provider-tool', title: '6', detail: 'Running' } } },
  ])
  await page.evaluate(() => {
    ;(window as unknown as TestHooks).__yeetEmit({
      type: 'state_update', version: 1, sequence: 3, revision: 3,
      patch: { is_streaming: true, active_activity_entry_id: 'opaque-phase' },
    })
  })

  const group = page.locator('.activity-group')
  const header = group.locator('.activity-group__header')
  await expect(header).toContainText('Read file')
  await expect(header).toContainText('src/file-last.rs')
  await expect(header).not.toContainText(/6\s*[·.]\s*Running/)
  await expect(group.locator('.activity-group__summary')).toHaveClass(/is-active/)
  await expect(header).toHaveAttribute('aria-expanded', 'false')

  await header.click()
  await expect(group.locator('.activity-group__event')).toHaveCount(3)
  await expect(group).toContainText('Working')
})

test('a standalone count-and-phase activity is not shown as a fake action', async ({ page }) => {
  await resetConversation(page, [
    { id: 'numeric-phase', kind: { type: 'activity', activity: { phase: 'provider-tool', title: '6', detail: 'Running' } } },
  ])
  const header = page.locator('.activity-group__header')
  await expect(header).toContainText('Working')
  await expect(header).not.toContainText(/6\s*[·.]\s*Running/)
})

test('failures auto-open while running activity remains visibly active', async ({ page }) => {
  await resetConversation(page, [
    toolEntry('tool-4', 'running'),
    toolEntry('tool-5', 'failed'),
  ])

  const group = page.locator('.activity-group')
  const header = group.locator('.activity-group__header')
  await expect(header).toHaveAttribute('aria-expanded', 'true')
  await expect(group.locator('.activity-group__summary')).toHaveClass(/is-active/)
  await expect(group.locator('.activity-group__event')).toHaveCount(2)

  const rows = group.locator('.trace-disclosure__row')
  await expect(rows.nth(0)).toContainText(/Reading file|Read file/)
  await expect(rows.nth(1)).toContainText('Failed')
})

test('long completed runs remain compact by default', async ({ page }) => {
  const entries = Array.from({ length: 13 }, (_, index) => toolEntry(`tool-${index + 10}`, 'completed'))
  await resetConversation(page, entries)

  const group = page.locator('.activity-group')
  const header = group.locator('.activity-group__header')
  await expect(group).toHaveCount(1)
  await expect(header).toHaveAttribute('aria-expanded', 'false')
  await expect(group.locator('.activity-group__events')).toHaveCount(0)
})

test('reasoning displays separately from tool groups and preserves transcript order', async ({ page }) => {
  await resetConversation(page, [
    {
      id: 'reasoning-1',
      kind: { type: 'reasoning', content: 'First full reasoning details', summary: '**Checking sources**' },
    },
    toolEntry('tool-20', 'completed', 'read_file'),
    {
      id: 'reasoning-2',
      kind: { type: 'reasoning', content: 'Second full reasoning details', summary: '**Cross-checking claims**' },
    },
    toolEntry('tool-21', 'completed', 'run_shell'),
  ])

  const transcriptItems = page.locator('.conversation-content > .reasoning-trace, .conversation-content > .activity-group')
  const reasoning = page.locator('.reasoning-trace')
  const groups = page.locator('.activity-group')
  await expect(transcriptItems).toHaveCount(4)
  await expect(reasoning).toHaveCount(2)
  await expect(groups).toHaveCount(2)
  await expect(page.locator('.activity-group .reasoning-trace')).toHaveCount(0)
  await expect(transcriptItems.nth(0)).toHaveClass(/reasoning-trace/)
  await expect(transcriptItems.nth(0)).toContainText('Checking sources')
  await expect(reasoning.nth(0).locator('.trace-disclosure__details')).toHaveCount(0)
  await expect(reasoning.nth(0).getByRole('button')).toHaveCount(0)
  await expect(transcriptItems.nth(0)).not.toContainText('First full reasoning details')
  await expect(transcriptItems.nth(1)).toHaveClass(/activity-group/)
  await expect(transcriptItems.nth(2)).toHaveClass(/reasoning-trace/)
  await expect(transcriptItems.nth(2)).toContainText('Cross-checking claims')
  await expect(reasoning.nth(1).locator('.trace-disclosure__details')).toHaveCount(0)
  await expect(reasoning.nth(1).getByRole('button')).toHaveCount(0)
  await expect(transcriptItems.nth(2)).not.toContainText('Second full reasoning details')
  await expect(page.locator('.conversation-content')).not.toContainText('**')

  await groups.nth(0).locator('.activity-group__header').click()
  await expect(groups.nth(0)).toContainText('Read file')
  await groups.nth(1).locator('.activity-group__header').click()
  await expect(groups.nth(1)).toContainText(/Command|Run shell/)
})


test('current tool activity remains separate from the full reasoning trace', async ({ page }) => {
  await resetConversation(page, [
    { id: 'old-reasoning', kind: { type: 'reasoning', content: '**Raw provider reasoning details**', summary: '**Inspecting inputs****Checking constraints**' } },
    toolEntry('current-read', 'running'),
  ])
  const group = page.locator('.activity-group')
  const header = group.locator('.activity-group__header')
  await expect(header).toContainText(/Reading file|Read file/)
  await expect(header).toContainText('src/file-current-read.rs')
  const reasoning = page.locator('.reasoning-trace')
  await expect(reasoning).toHaveCount(1)
  await expect(reasoning).toContainText('Checking constraints')
  await expect(reasoning.locator('.trace-disclosure__details')).toHaveCount(0)
  await expect(reasoning.getByRole('button')).toHaveCount(0)
  await expect(reasoning).toContainText('Inspecting inputs')
  await expect(reasoning).not.toContainText('Raw provider reasoning details')
  await expect(page.locator('.activity-group .reasoning-trace')).toHaveCount(0)
})


test('single tools have one compact row, with output and timing behind disclosure', async ({ page }) => {
  await resetConversation(page, [
    { id: 'density-reasoning', kind: { type: 'reasoning', content: 'Full explanation', summary: 'Checking the relevant sources without taking up multiple lines in the transcript' } },
    { id: 'density-tool', kind: { type: 'toolCall', toolCall: {
      id: 'density-tool', name: 'run_shell', arguments: JSON.stringify({ command: 'cargo check' }),
      status: 'completed', durationMs: 1200, result: 'literal **output**',
    } } },
  ])
  const group = page.locator('.activity-group')
  const header = group.locator('.activity-group__header')
  const reasoning = page.locator('.reasoning-trace')
  await expect(group.locator('.trace-disclosure__row')).toHaveCount(1)
  await expect(header).toContainText('cargo check')
  await expect(header).not.toContainText(/Activity|✓|1.2 s/)
  await expect(header.locator('.trace-disclosure__status, .trace-disclosure__chevron-gap')).toHaveCount(0)
  await expect(reasoning).not.toContainText(/Thinking|Reasoning/)
  await expect(reasoning.locator('.trace-disclosure__details')).toHaveCount(0)
  const metrics = await page.locator('.conversation-content').evaluate(node => {
    const reasoning = node.querySelector('.reasoning-trace')!
    const group = node.querySelector('.activity-group')!
    const label = reasoning.querySelector('yeet-markdown')!
    return {
      gap: group.getBoundingClientRect().top - reasoning.getBoundingClientRect().bottom,
      nowrap: getComputedStyle(label).whiteSpace,
      overflow: node.scrollWidth > node.clientWidth,
    }
  })
  expect(metrics.gap).toBeLessThanOrEqual(2)
  expect(metrics.gap).toBeGreaterThanOrEqual(0)
  expect(metrics.nowrap).toBe('pre-wrap')
  expect(metrics.overflow).toBe(false)
  await header.click()
  await expect(group.locator('.trace-disclosure__row')).toHaveCount(1)
  await expect(group.locator('.trace-disclosure__details')).toContainText('literal **output**')
  await expect(group.locator('.trace-disclosure__details')).toContainText('1.2 s')

  await resetConversation(page, [{ id: 'no-details', kind: { type: 'toolCall', toolCall: {
    id: 'no-details', name: 'read_file', arguments: '{}', status: 'completed',
  } } }])
  await expect(group.locator('.trace-disclosure__row')).toHaveCount(1)
  await expect(group.getByRole('button')).toHaveCount(0)
  await expect(group.locator('.trace-disclosure__chevron, .trace-disclosure__status')).toHaveCount(0)
})

test('approval never spins, and exceptional status stays explicit', async ({ page }) => {
  for (const [status, label] of [['awaiting_permission', 'Awaiting approval'], ['timed_out', 'Timed out'], ['cancelled', 'Cancelled']] as const) {
    await resetConversation(page, [{ id: status, kind: { type: 'toolCall', toolCall: {
      id: status, name: 'run_shell', arguments: JSON.stringify({ command: 'cargo check' }), status,
    } } }])
    const header = page.locator('.activity-group__header')
    await expect(header).toContainText(label)
    await expect(header.getByRole('img', { name: 'Running' })).toHaveCount(0)
  }

  await resetConversation(page, [
    { id: 'cancelled-first', kind: { type: 'toolCall', toolCall: {
      id: 'cancelled-first', name: 'run_shell', arguments: '{"command":"check"}', status: 'cancelled',
    } } },
    toolEntry('completed-last', 'completed'),
  ])
  const groupHeader = page.locator('.activity-group__header')
  await expect(groupHeader).toHaveAttribute('aria-expanded', 'false')
  await expect(groupHeader).toContainText('Cancelled')
})
