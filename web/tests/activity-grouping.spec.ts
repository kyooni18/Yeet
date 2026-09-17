import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
}

function toolEntry(id: string, status: 'completed' | 'streaming' | 'failed') {
  return {
    type: 'conversation_entry',
    version: 1,
    sequence: Number(id.replace('tool-', '')) + 2,
    revision: Number(id.replace('tool-', '')) + 2,
    entry: {
      id,
      kind: {
        type: 'toolCall',
        toolCall: {
          id,
          name: 'read_file',
          arguments: JSON.stringify({ path: `src/file-${id}.rs` }),
          status,
        },
      },
    },
  }
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('groups consecutive completed tool calls into one collapsed transcript block', async ({ page }) => {
  const entries = ['tool-1', 'tool-2', 'tool-3'].map((id) => toolEntry(id, 'completed'))
  await page.evaluate((entries) => {
    const emit = (window as unknown as TestHooks).__yeetEmit
    emit({ type: 'conversation_reset', version: 1, sequence: 2, revision: 2, conversation: entries.map((message) => message.entry) })
  }, entries)

  const group = page.getByTestId('activity-group')
  await expect(group).toHaveCount(1)
  await expect(group.locator('summary')).toContainText('3 tool calls')
  await expect(group.locator('summary')).toContainText('3 complete')
  await expect(group.locator('.tool-card')).toHaveCount(0)

  await group.locator('summary').click()
  await expect(group.locator('.tool-card')).toHaveCount(3)
})

test('keeps a live tool group open and promotes failures in the summary', async ({ page }) => {
  const entries = [toolEntry('tool-4', 'streaming'), toolEntry('tool-5', 'failed')]
  await page.evaluate((entries) => {
    const emit = (window as unknown as TestHooks).__yeetEmit
    emit({ type: 'conversation_reset', version: 1, sequence: 2, revision: 2, conversation: entries.map((message) => message.entry) })
  }, entries)

  const group = page.getByTestId('activity-group')
  await expect(group).toHaveClass(/is-live/)
  await expect(group).toHaveClass(/is-error/)
  await expect(group.locator('summary')).toContainText('1 running')
  await expect(group.locator('summary')).toContainText('1 failed')
  await expect(group.locator('.tool-card')).toHaveCount(2)
})
