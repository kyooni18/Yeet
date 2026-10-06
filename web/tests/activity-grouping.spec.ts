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
  await expect(header).toContainText('Activity')
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

test('shared reasoning rows preserve transcript order and strip markdown from summaries', async ({ page }) => {
  await resetConversation(page, [
    {
      id: 'reasoning-1',
      kind: { type: 'reasoning', content: '', summary: '**Checking sources**' },
    },
    toolEntry('tool-20', 'completed', 'read_file'),
    {
      id: 'reasoning-2',
      kind: { type: 'reasoning', content: '', summary: '**Cross-checking claims**' },
    },
    toolEntry('tool-21', 'completed', 'run_shell'),
  ])

  const items = page.locator('.conversation-content > .trace-disclosure, .conversation-content > .activity-group')
  const reasoning = page.locator('.conversation-content > .trace-disclosure')
  const groups = page.locator('.activity-group')
  await expect(items).toHaveCount(4)
  await expect(reasoning).toHaveCount(2)
  await expect(groups).toHaveCount(2)
  await expect(items.nth(0)).toContainText('Checking sources')
  await expect(items.nth(2)).toContainText('Cross-checking claims')
  await expect(page.locator('.conversation-content')).not.toContainText('**')
  await groups.nth(0).locator('.activity-group__header').click()
  await expect(items.nth(1)).toContainText('Read file')
  await groups.nth(1).locator('.activity-group__header').click()
  await expect(items.nth(3)).toContainText(/Command|Run shell/)
})


test('current tool activity remains separate from shared reasoning details', async ({ page }) => {
  await resetConversation(page, [
    { id: 'old-reasoning', kind: { type: 'reasoning', content: '', summary: '**Inspecting inputs****Checking constraints**' } },
    toolEntry('current-read', 'running'),
  ])
  const group = page.locator('.activity-group')
  const header = group.locator('.activity-group__header')
  await expect(header).toContainText(/Reading file|Read file/)
  await expect(header).toContainText('src/file-current-read.rs')
  const reasoning = page.locator('.conversation-content > .trace-disclosure').filter({ hasText: 'Reasoning' })
  await expect(reasoning.locator('.trace-disclosure__row')).toContainText('Checking constraints')
  await expect(reasoning.locator('.trace-disclosure__row')).toHaveAttribute('aria-expanded', 'true')
  await expect(reasoning.locator('.trace-detail__content')).toContainText('Inspecting inputs')
})
