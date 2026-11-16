import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('long transcripts mount a bounded window and reveal earlier rows in place', async ({ page }) => {
  const conversation = Array.from({ length: 300 }, (_, i) => ({
    id: `m${i}`, kind: i % 2 ? { type: 'assistant', content: `Answer ${i}` } : { type: 'user', content: `Question ${i}` },
  }))
  await page.evaluate(conversation => {
    (window as unknown as { __yeetEmit: (message: unknown) => void }).__yeetEmit({
      type: 'conversation_reset', version: 1, sequence: 3, revision: 3, conversation,
    })
  }, conversation)
  await expect(page.getByText('Answer 299', { exact: true })).toBeAttached()
  const rows = page.locator('.conversation-content > .message-block')
  expect(await rows.count()).toBeLessThanOrEqual(120)
  await expect(page.getByText('Question 0', { exact: true })).toHaveCount(0)

  const earlier = page.getByRole('button', { name: /Show earlier messages/ })
  const scroller = page.getByRole('region', { name: 'Conversation' })
  await scroller.evaluate(element => { element.scrollTop = 0 })
  const anchor = page.getByText('Question 180', { exact: true })
  const before = await anchor.boundingBox()
  await earlier.click()
  await expect.poll(() => rows.count()).toBeGreaterThan(200)
  // Prepending keeps the reader's place instead of jumping.
  await expect.poll(async () => Math.abs((await anchor.boundingBox())!.y - before!.y)).toBeLessThan(4)
  await earlier.click()
  await expect(page.getByText('Question 0', { exact: true })).toBeAttached()
  await expect(earlier).toHaveCount(0)
})
