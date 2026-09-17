import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
}

async function addLongCodeBlock(page: Page) {
  await page.evaluate(() => {
    const emit = (window as unknown as TestHooks).__yeetEmit
    const content = `\`\`\`ts\n${Array.from({ length: 120 }, (_, index) => `const row_${index} = ${index}`).join('\n')}\n\`\`\``
    emit({
      type: 'conversation_entry',
      version: 1,
      sequence: 2,
      revision: 2,
      entry: {
        id: 'keyboard-scroll-code',
        kind: { type: 'assistant', content },
      },
    })
  })
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await addLongCodeBlock(page)
})

test('overflowing fenced code is keyboard-focusable and scrollable', async ({ page }) => {
  const codeBlock = page.locator('.code-block').last()
  const copy = codeBlock.getByRole('button', { name: 'Copy' })
  const viewport = codeBlock.locator('pre')

  await expect(viewport).toContainText('row_119')
  await viewport.scrollIntoViewIfNeeded()
  await expect(viewport).toHaveAttribute('tabindex', '0')
  await expect(viewport).toHaveAttribute('aria-label', 'ts code block')

  const overflow = await viewport.evaluate((element) => ({
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
  }))
  expect(overflow.scrollHeight).toBeGreaterThan(overflow.clientHeight)

  await copy.focus()
  await page.keyboard.press('Tab')
  await expect(viewport).toBeFocused()

  const before = await viewport.evaluate((element) => element.scrollTop)
  await page.keyboard.press('PageDown')
  await expect.poll(() => viewport.evaluate((element) => element.scrollTop)).toBeGreaterThan(before)
})

test('code viewport has a readable accessibility name', async ({ page }) => {
  const viewport = page.locator('.code-block').last().locator('pre')
  await viewport.scrollIntoViewIfNeeded()
  const accessibilityTree = await viewport.ariaSnapshot()
  expect(accessibilityTree).toContain('ts code block')
})
