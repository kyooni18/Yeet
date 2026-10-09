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
  await expect(transcriptItems.nth(0)).toContainText('First full reasoning details')
  await expect(transcriptItems.nth(1)).toHaveClass(/activity-group/)
  await expect(transcriptItems.nth(2)).toHaveClass(/reasoning-trace/)
  await expect(transcriptItems.nth(2)).toContainText('Cross-checking claims')
  await expect(transcriptItems.nth(2)).toContainText('Second full reasoning details')
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
  await expect(reasoning).toContainText('Inspecting inputs')
  await expect(reasoning).toContainText('Raw provider reasoning details')
  await expect(page.locator('.activity-group .reasoning-trace')).toHaveCount(0)
})
