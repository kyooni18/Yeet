import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
}

async function installFixture(page: Page) {
  await page.evaluate(() => {
    ;(window as unknown as TestHooks).__yeetEmit({
      type: 'conversation_reset',
      version: 1,
      sequence: 2,
      revision: 2,
      conversation: [
        { id: 'touch-user', kind: { type: 'user', content: 'Please inspect this.' } },
        {
          id: 'touch-tool-entry',
          kind: {
            type: 'toolCall',
            toolCall: {
              id: 'touch-tool',
              name: 'run_shell',
              arguments: JSON.stringify({ command: 'pnpm test' }),
              status: 'failed',
              result: '',
              error: 'Fixture failure',
            },
          },
        },
      ],
    })
  })
  await expect(page.locator('.activity-group__header')).toHaveAttribute('aria-expanded', 'true')
}

async function heights(page: Page, selector: string) {
  return page.locator(selector).evaluateAll((elements) =>
    elements.filter((element) => {
      const style = getComputedStyle(element)
      const rect = element.getBoundingClientRect()
      return style.visibility !== 'hidden' && style.display !== 'none' && rect.width > 0 && rect.height > 0
    }).map((element) => element.getBoundingClientRect().height),
  )
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await installFixture(page)
})

test('touch transcript interactions keep 44px hit targets', async ({ page }) => {
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  test.skip(!touchFirst, 'Requires a coarse touch pointer')

  for (const selector of [
    '.activity-group__header',
    '.trace-disclosure__row',
    '.message-actions button',
  ]) {
    const values = await heights(page, selector)
    expect(values.length).toBeGreaterThan(0)
    for (const value of values) expect(value).toBeGreaterThanOrEqual(44)
  }
})

test('fine-pointer transcript keeps compact activity chrome', async ({ page }) => {
  const finePointer = await page.evaluate(() => matchMedia('(hover: hover) and (pointer: fine)').matches)
  test.skip(!finePointer, 'Fine-pointer behavior only')

  const groupHeader = await page.locator('.activity-group__header').boundingBox()
  const traceRow = await page.locator('.trace-disclosure__row').first().boundingBox()
  const messageAction = await page.locator('.message-actions button').first().boundingBox()
  expect(groupHeader).not.toBeNull()
  expect(traceRow).not.toBeNull()
  expect(messageAction).not.toBeNull()
  expect(groupHeader!.height).toBeLessThan(44)
  expect(traceRow!.height).toBeLessThan(44)
  expect(messageAction!.height).toBeLessThan(44)
})
