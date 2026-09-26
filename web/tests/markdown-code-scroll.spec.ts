import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
}

async function addLongCodeBlock(page: Page) {
  await page.evaluate(() => {
    const content = `\u0060\u0060\u0060ts\n${Array.from({ length: 120 }, (_, index) => `const row_${index} = ${index}`).join('\n')}\n\u0060\u0060\u0060`
    ;(window as unknown as TestHooks).__yeetEmit({
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

test('overflowing fenced code is a named keyboard-scrollable region', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await addLongCodeBlock(page)

  const viewport = page.getByRole('region', { name: 'ts code block' }).last()
  await expect(viewport).toContainText('row_119')
  await expect(viewport).toHaveAttribute('tabindex', '0')

  const overflow = await viewport.evaluate((element) => ({
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
  }))
  expect(overflow.scrollHeight).toBeGreaterThan(overflow.clientHeight)

  await viewport.focus()
  await expect(viewport).toBeFocused()
  const before = await viewport.evaluate((element) => element.scrollTop)
  await page.keyboard.press('PageDown')
  await expect.poll(() => viewport.evaluate((element) => element.scrollTop)).toBeGreaterThan(before)

  const accessibilityTree = await viewport.ariaSnapshot()
  expect(accessibilityTree).toContain('ts code block')
})
