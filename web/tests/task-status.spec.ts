import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('surfaces the latest reasoning in a compact task-status dropdown', async ({ page }) => {
  await page.evaluate(() => {
    const emit = (window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit
    emit({
      type: 'conversation_entry', version: 1, sequence: 2, revision: 2,
      entry: { id: 'live-reasoning', kind: { type: 'reasoning', content: '', summary: '' } },
    })
    emit({
      type: 'state_update', version: 1, sequence: 3, revision: 3,
      patch: { is_streaming: true, active_reasoning_entry_id: 'live-reasoning' },
    })
    emit({
      type: 'reasoning_delta', version: 1, sequence: 4, revision: 4,
      entry_id: 'live-reasoning', delta: '', content: 'Comparing the responsive layout', summary: false, reset: true,
    })
  })

  const status = page.locator('.task-status-card')
  await expect(status).toBeVisible()
  await expect(status.locator('summary')).toContainText('Thinking')
  await expect(status.locator('summary')).toContainText('Comparing the responsive layout')
  await expect(status.locator('.task-status-body')).not.toBeAttached()
  await expect(page.locator('[data-latest-reasoning="true"]')).toHaveCount(1)

  await status.locator('summary').click()
  await expect(status.locator('.task-status-body')).toContainText('Comparing the responsive layout')
})
