import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('browser title follows the active chat', async ({ page }) => {
  await expect(page).toHaveTitle('Remote WebUI · Yeet')

  await page.evaluate(() => {
    const emit = (window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit
    emit({
      version: 1,
      type: 'state_update',
      sequence: 2,
      revision: 2,
      patch: {
        workspace_root: '/Users/test/Code/Rust/AnotherProject',
        current_session_id: 'session-b',
        saved_sessions: [
          { id: 'session-b', title: 'Protocol review', updated_at: new Date().toISOString(), model: 'gpt-5.6-luna', message_count: 4 },
        ],
      },
    })
  })

  await expect(page).toHaveTitle('Protocol review · Yeet')
})

test('browser title identifies authorization routes', async ({ page }) => {
  await page.goto('/enroll')
  await expect(page).toHaveTitle('Authorize · Yeet')
})
