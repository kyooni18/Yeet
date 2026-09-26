import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
}

async function emit(page: Page, message: Record<string, unknown>) {
  await page.evaluate((payload) => {
    ;(window as unknown as TestHooks).__yeetEmit(payload)
  }, message)
}

function assistantCopy(page: Page) {
  return page.locator('.message-block--assistant .message-actions button').first()
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: async (text: string) => {
          Object.assign(window, { copiedResponse: text })
        },
      },
    })
  })
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('message copy preserves raw markdown and keyboard focus', async ({ page }) => {
  const copy = assistantCopy(page)
  await expect(copy).toHaveAttribute('aria-label', 'Copy message')
  await copy.focus()
  await page.keyboard.press('Enter')

  await expect(copy).toHaveAttribute('aria-label', 'Copied')
  await expect(copy).toBeFocused()
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { copiedResponse?: string }).copiedResponse ?? ''
  )).toContain('## Interface ready')
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { copiedResponse?: string }).copiedResponse ?? ''
  )).toContain('const transport = "websocket"')
})

test('clipboard fallback copies successfully without losing focus', async ({ page }) => {
  await page.evaluate(() => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: async () => { throw new Error('Clipboard unavailable') } },
    })
    Object.defineProperty(document, 'execCommand', {
      configurable: true,
      value: (command: string) => {
        Object.assign(window, { fallbackCopyCommand: command })
        return command === 'copy'
      },
    })
  })

  const copy = assistantCopy(page)
  await copy.focus()
  await page.keyboard.press('Enter')

  await expect(copy).toHaveAttribute('aria-label', 'Copied')
  await expect(copy).toBeFocused()
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { fallbackCopyCommand?: string }).fallbackCopyCommand ?? ''
  )).toBe('copy')
})

test('copy failure is visible and keeps keyboard focus', async ({ page }) => {
  await page.evaluate(() => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: async () => { throw new Error('Clipboard unavailable') } },
    })
    Object.defineProperty(document, 'execCommand', {
      configurable: true,
      value: () => false,
    })
  })

  const copy = assistantCopy(page)
  await copy.focus()
  await page.keyboard.press('Enter')

  await expect(copy).toHaveAttribute('aria-label', 'Copy failed')
  await expect(copy).toHaveAttribute('title', 'Copy failed')
  await expect(copy).toBeFocused()
})

test('activity rows distinguish approval, timeout, and interruption states', async ({ page }) => {
  await emit(page, {
    type: 'conversation_reset',
    version: 1,
    sequence: 2,
    revision: 2,
    conversation: ['awaiting_permission', 'timed_out', 'interrupted'].map((status, index) => ({
      id: `state-${index}`,
      kind: {
        type: 'toolCall',
        toolCall: {
          id: `call-${index}`,
          name: 'run_shell',
          arguments: JSON.stringify({ command: `probe-${status}` }),
          status,
        },
      },
    })),
  })

  const group = page.locator('.activity-group').last()
  const header = group.locator('.activity-group__header')
  await expect(header).toHaveAttribute('aria-expanded', 'true')

  const rows = group.locator('.trace-disclosure__row')
  await expect(rows).toHaveCount(3)
  await expect(rows.filter({ hasText: 'probe-awaiting_permission' })).toContainText('Awaiting approval')
  await expect(rows.filter({ hasText: 'probe-timed_out' })).toContainText('Timed out')
  await expect(rows.filter({ hasText: 'probe-interrupted' })).toContainText('Interrupted')

  const collisions = await rows.evaluateAll((elements) => elements.some((row) => {
    const title = row.querySelector('.trace-disclosure__title')?.getBoundingClientRect()
    const status = row.querySelector('.trace-disclosure__status')?.getBoundingClientRect()
    return Boolean(title && status && title.right > status.left)
  }))
  expect(collisions).toBe(false)
})
