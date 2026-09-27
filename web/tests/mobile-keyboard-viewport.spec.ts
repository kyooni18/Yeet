import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type ViewportPatch = { width?: number; height?: number; offsetTop?: number; offsetLeft?: number }
type Hooks = {
  __setTestVisualViewport: (patch: ViewportPatch) => void
  __yeetEmit: (message: Record<string, unknown>) => void
}

async function installVisualViewport(page: Page) {
  await page.addInitScript(() => {
    class FakeVisualViewport extends EventTarget {
      width = window.innerWidth
      height = window.innerHeight
      offsetTop = 0
      offsetLeft = 0
      pageTop = 0
      pageLeft = 0
      scale = 1
    }

    const viewport = new FakeVisualViewport()
    Object.defineProperty(window, 'visualViewport', { configurable: true, value: viewport })
    ;(window as unknown as Hooks).__setTestVisualViewport = (patch) => {
      const previousTop = viewport.offsetTop
      Object.assign(viewport, patch)
      viewport.pageTop = viewport.offsetTop
      viewport.pageLeft = viewport.offsetLeft
      viewport.dispatchEvent(new Event('resize'))
      if (viewport.offsetTop !== previousTop) viewport.dispatchEvent(new Event('scroll'))
    }
  })
}

test.beforeEach(async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)
  await installVisualViewport(page)
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('software keyboard keeps the iOS shell on the VisualViewport without stealing reader scroll', async ({ page }) => {
  await page.evaluate(() => {
    const emit = (window as unknown as Hooks).__yeetEmit
    const conversation = Array.from({ length: 44 }, (_, index) => ({
      id: `keyboard-history-${index}`,
      kind: index % 2
        ? { type: 'assistant', content: `Response ${index}: ${'response context '.repeat(10)}` }
        : { type: 'user', content: `Question ${index}: ${'scroll context '.repeat(7)}` },
    }))
    emit({ type: 'conversation_reset', version: 1, sequence: 2, revision: 2, conversation })
  })

  const transcript = page.locator('.conversation-scroll')
  const textarea = page.getByRole('textbox', { name: 'Message' })
  const fullHeight = page.viewportSize()!.height
  const keyboardHeight = Math.min(430, Math.max(1, fullHeight - 120))
  await transcript.evaluate((element) => element.scrollTo({ top: element.scrollHeight }))
  await expect.poll(() => transcript.evaluate((element) =>
    Math.round(element.scrollHeight - element.scrollTop - element.clientHeight)
  )).toBeLessThanOrEqual(2)
  await textarea.focus()

  await page.evaluate((height) => (window as unknown as Hooks).__setTestVisualViewport({ height, offsetTop: 18 }), keyboardHeight)
  await expect.poll(() => page.evaluate(() => document.documentElement.dataset.keyboardOpen)).toBe('true')

  const stageBox = await page.locator('.remote-stage').boundingBox()
  const composerBox = await page.locator('.composer-shell').boundingBox()
  expect(stageBox).not.toBeNull()
  expect(composerBox).not.toBeNull()
  expect(Math.abs(stageBox!.y - 18)).toBeLessThanOrEqual(1)
  expect(Math.abs(stageBox!.height - keyboardHeight)).toBeLessThanOrEqual(1)
  expect(composerBox!.y + composerBox!.height).toBeLessThanOrEqual(18 + keyboardHeight)
  await expect.poll(() => transcript.evaluate((element) =>
    Math.round(element.scrollHeight - element.scrollTop - element.clientHeight)
  )).toBeLessThanOrEqual(2)

  await transcript.evaluate((element) =>
    element.scrollTo({ top: Math.max(0, element.scrollHeight - element.clientHeight - 320) })
  )
  await expect.poll(() => transcript.evaluate((element) =>
    element.scrollHeight - element.scrollTop - element.clientHeight
  )).toBeGreaterThan(200)
  const before = await transcript.evaluate((element) => element.scrollTop)

  const resizedKeyboardHeight = Math.min(390, Math.max(1, fullHeight - 160))
  await page.evaluate((height) => (window as unknown as Hooks).__setTestVisualViewport({ height, offsetTop: 24 }), resizedKeyboardHeight)
  await expect.poll(() => transcript.evaluate((element) => element.scrollTop)).toBeCloseTo(before, 0)
  const after = await transcript.evaluate((element) => element.scrollTop)
  expect(Math.abs(after - before)).toBeLessThanOrEqual(3)

  await textarea.blur()
  expect(await page.evaluate(() => document.documentElement.dataset.keyboardOpen)).toBe('true')

  await page.evaluate((height) => (window as unknown as Hooks).__setTestVisualViewport({ height, offsetTop: 0 }), fullHeight)
  await expect.poll(() => page.evaluate(() => document.documentElement.dataset.keyboardOpen ?? 'false')).toBe('false')
})
