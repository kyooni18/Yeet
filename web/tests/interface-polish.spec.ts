import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: {
      writeText: async (text: string) => { Object.assign(window, { copiedResponse: text }) },
    } })
  })
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('whole response copy preserves markdown and keyboard focus', async ({ page }) => {
  const copy = page.getByRole('button', { name: 'Copy response', exact: true })
  await copy.focus()
  await page.keyboard.press('Enter')
  await expect(copy).toHaveText('Copied')
  await expect(copy).toBeFocused()
  await expect.poll(() => page.evaluate(() => (window as unknown as { copiedResponse: string }).copiedResponse)).toContain('## Interface ready')
  await expect(page.getByRole('status').filter({ hasText: 'Response copied' })).toBeVisible()
})

test('activity summaries distinguish approval, timeout, and interruption', async ({ page }) => {
  await page.evaluate(() => {
    const emit = (window as unknown as { __yeetEmit: (message: unknown) => void }).__yeetEmit
    emit({ type: 'conversation_reset', version: 1, sequence: 2, revision: 2,
      conversation: ['awaiting_permission', 'timed_out', 'interrupted'].map((status, index) => ({
        id: `state-${index}`, kind: { type: 'toolCall', toolCall: {
          id: `call-${index}`, name: 'run_shell', arguments: '{}', status,
        } },
      })),
    })
  })
  const summary = page.getByTestId('activity-group').locator('summary').first()
  await expect(summary).toContainText('1 awaiting approval')
  await expect(summary).toContainText('1 failed')
  await expect(summary).toContainText('1 stopped')
})

test('semantic labels fit beside statuses and mobile replies use available width', async ({ page }, info) => {
  const collisions = await page.locator('.semantic-card > summary').evaluateAll((summaries) => summaries.some((summary) => {
    const heading = summary.querySelector('.semantic-heading')!.getBoundingClientRect()
    const status = summary.querySelector('.semantic-status')!.getBoundingClientRect()
    return heading.right > status.left
  }))
  expect(collisions).toBe(false)
  if ((page.viewportSize()?.width ?? 0) <= 600) {
    const reply = await page.locator('.assistant-content').boundingBox()
    expect(reply!.width).toBeGreaterThan(page.viewportSize()!.width - 60)
  }
  await page.screenshot({ path: `/tmp/yeet-overall-${info.project.name}.png`, fullPage: true })
})

test('response copy reports clipboard failure', async ({ page }) => {
  await page.evaluate(() => {
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: {
      writeText: async () => { throw new Error('Clipboard unavailable') },
    } })
    Object.defineProperty(document, 'execCommand', { configurable: true, value: () => false })
  })
  await page.getByRole('button', { name: 'Copy response', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Copy response', exact: true })).toHaveText('Copy failed')
  await expect(page.getByRole('status').filter({ hasText: 'Response could not be copied' })).toBeVisible()
})
