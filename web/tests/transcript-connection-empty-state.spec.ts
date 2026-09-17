import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
  __yeetDisconnect: () => void
}

async function emptyConversation(page: Page) {
  await page.evaluate(() => {
    ;(window as unknown as TestHooks).__yeetEmit({
      type: 'state_update',
      version: 1,
      sequence: 2,
      revision: 2,
      patch: { conversation: [] },
    })
  })
  await expect(page.locator('.conversation-entry')).toHaveCount(0)
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await emptyConversation(page)
})

test('connected empty conversation keeps the new-session onboarding', async ({ page }) => {
  await expect(page.getByRole('heading', { name: 'What can Yeet do for you?' })).toBeVisible()
  await expect(page.locator('.empty-transcript')).not.toHaveAttribute('role', 'status')
})

test('empty transcript reports reconnecting instead of looking like a fresh ready chat', async ({ page }) => {
  const rendered = await page.evaluate(async () => {
    ;(window as unknown as TestHooks).__yeetDisconnect()
    await Promise.resolve()
    await Promise.resolve()
    const empty = document.querySelector<HTMLElement>('.empty-transcript')
    return {
      heading: empty?.querySelector('h1')?.textContent ?? '',
      detail: empty?.querySelector('p')?.textContent ?? '',
      role: empty?.getAttribute('role'),
    }
  })

  expect(rendered.heading).toBe('Reconnecting to Yeet…')
  expect(rendered.detail).toContain('connection')
  expect(rendered.role).toBe('status')
})

test('empty transcript exposes a stable offline state', async ({ page }) => {
  await page.evaluate(async () => {
    window.dispatchEvent(new Event('offline'))
    await Promise.resolve()
    await Promise.resolve()
  })

  const empty = page.locator('.empty-transcript')
  await expect(empty).toHaveAttribute('role', 'status')
  await expect(page.getByRole('heading', { name: "You're offline" })).toBeVisible()
  await expect(empty).toContainText('draft')
  await expect(page.getByRole('heading', { name: 'What can Yeet do for you?' })).toHaveCount(0)
})
