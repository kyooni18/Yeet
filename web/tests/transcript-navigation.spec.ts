import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
}

async function addLongTranscript(page: Page, count = 72) {
  await page.evaluate((entryCount) => {
    const emit = (window as unknown as TestHooks).__yeetEmit
    for (let index = 0; index < entryCount; index += 1) {
      emit({
        type: 'conversation_entry',
        version: 1,
        sequence: index + 2,
        revision: index + 2,
        entry: {
          id: `keyboard-transcript-${index}`,
          kind: {
            type: index % 2 ? 'assistant' : 'user',
            content: `Transcript item ${index}: ${'keep enough reading context while paging. '.repeat(8)}`,
          },
        },
      })
    }
  }, count)

  await page.waitForFunction(() => {
    const transcript = document.querySelector<HTMLElement>('[data-testid="transcript"]')
    return !!transcript && transcript.scrollHeight > transcript.clientHeight * 2
  })
}

async function bottomDistance(page: Page) {
  return page.getByTestId('transcript').evaluate((element) => element.scrollHeight - element.scrollTop - element.clientHeight)
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await addLongTranscript(page)
})

test('long transcripts support direct keyboard paging and edge navigation', async ({ page }) => {
  const transcript = page.getByTestId('transcript')

  await expect(transcript).toHaveAttribute('tabindex', '0')
  await expect(transcript).toHaveAttribute('aria-label', 'Conversation transcript')
  await transcript.focus()
  await expect(transcript).toBeFocused()

  await page.keyboard.press('Home')
  await expect.poll(() => transcript.evaluate((element) => element.scrollTop)).toBeLessThanOrEqual(1)

  await page.keyboard.press('PageDown')
  await expect.poll(() => transcript.evaluate((element) => element.scrollTop)).toBeGreaterThan(120)
  const afterPageDown = await transcript.evaluate((element) => element.scrollTop)

  await page.keyboard.press('PageUp')
  await expect.poll(() => transcript.evaluate((element) => element.scrollTop)).toBeLessThan(afterPageDown - 80)

  await page.keyboard.press('End')
  await expect.poll(() => bottomDistance(page)).toBeLessThanOrEqual(1)
})

test('jump to latest keeps keyboard focus in the transcript', async ({ page }) => {
  const transcript = page.getByTestId('transcript')
  await transcript.focus()
  await page.keyboard.press('Home')

  const latest = page.getByRole('button', { name: 'Latest' })
  await expect(latest).toBeVisible()
  await latest.focus()
  await page.keyboard.press('Enter')

  await expect.poll(() => bottomDistance(page)).toBeLessThanOrEqual(1)
  await expect(transcript).toBeFocused()
  await expect(latest).not.toBeAttached()

  const atBottom = await transcript.evaluate((element) => element.scrollTop)
  await page.keyboard.press('PageUp')
  await expect.poll(() => transcript.evaluate((element) => element.scrollTop)).toBeLessThan(atBottom - 80)
})

test('mobile jump to latest stays reachable above an expanded composer', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)

  const transcript = page.getByTestId('transcript')
  await transcript.focus()
  await page.keyboard.press('Home')
  const latest = page.getByRole('button', { name: 'Latest' })
  await expect(latest).toBeVisible()

  const composerInput = page.getByRole('textbox', { name: 'Message Yeet' })
  await composerInput.fill(Array.from({ length: 14 }, (_, index) => `Draft line ${index + 1}`).join('\n'))

  await expect.poll(async () => {
    const latestBox = await latest.boundingBox()
    const composerBox = await page.getByTestId('composer').boundingBox()
    if (!latestBox || !composerBox) return -1
    return composerBox.y - (latestBox.y + latestBox.height)
  }).toBeGreaterThanOrEqual(8)

  const latestOwnsCenterHit = await latest.evaluate((button) => {
    const rect = button.getBoundingClientRect()
    const hit = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2)
    return hit === button || button.contains(hit)
  })
  expect(latestOwnsCenterHit).toBe(true)

  await latest.click()
  await expect.poll(() => bottomDistance(page)).toBeLessThanOrEqual(1)
})


test('switching sessions resets transcript follow state to the new conversation tail', async ({ page }) => {
  const transcript = page.getByTestId('transcript')
  await transcript.focus()
  await page.keyboard.press('Home')
  await expect(page.getByRole('button', { name: 'Latest' })).toBeVisible()

  await page.evaluate(() => {
    const emit = (window as unknown as TestHooks).__yeetEmit
    const conversation = Array.from({ length: 96 }, (_, index) => ({
      id: `session-b-${index}`,
      kind: index % 2
        ? { type: 'assistant', content: `New session response ${index}: ${'latest context '.repeat(8)}` }
        : { type: 'user', content: `New session request ${index}: ${'different context '.repeat(8)}` },
    }))
    emit({
      type: 'state_update',
      version: 1,
      sequence: 74,
      revision: 74,
      patch: { current_session_id: 'session-b', conversation },
    })
  })

  await expect(page.getByText('New session response 95:')).toBeVisible()
  await expect.poll(() => bottomDistance(page)).toBeLessThanOrEqual(1)
  await expect(page.getByRole('button', { name: 'Latest' })).not.toBeAttached()
})
