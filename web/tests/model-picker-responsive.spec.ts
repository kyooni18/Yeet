import { expect, test, type Locator, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = { __yeetEmit: (message: Record<string, unknown>) => void }

async function emitModels(page: Page) {
  await page.evaluate(() => {
    ;(window as unknown as TestHooks).__yeetEmit({
      type: 'state_update', version: 1, sequence: 2, revision: 2,
      patch: {
        active_model: 'openai/gpt-5.6-sol',
        available_models: [
          'openai/gpt-5.6-sol',
          'anthropic/claude-opus-4.1',
          'google/gemini-3-pro',
          'xai/grok-4',
        ],
      },
    })
  })
}

async function expectTouchSized(locator: Locator) {
  const boxes = await locator.evaluateAll((elements) => elements.map((element) => {
    const rect = element.getBoundingClientRect()
    return { width: Math.round(rect.width), height: Math.round(rect.height), label: element.textContent?.trim() || element.getAttribute('aria-label') || '' }
  }))
  expect(boxes.length).toBeGreaterThan(0)
  for (const box of boxes) {
    expect(box.width, box.label).toBeGreaterThanOrEqual(44)
    expect(box.height, box.label).toBeGreaterThanOrEqual(44)
  }
}

async function expectIOSFocusSafe(locator: Locator) {
  const fontSize = await locator.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))
  expect(fontSize).toBeGreaterThanOrEqual(16)
}

async function openTopbarPicker(page: Page, width: number, height: number) {
  await page.setViewportSize({ width, height })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await emitModels(page)
  const picker = page.locator('.top-bar').getByTestId('model-picker')
  await picker.getByTestId('model-picker-trigger').click()
  await expect(picker.getByTestId('model-picker-popover')).toBeVisible()
  await expect(picker.getByTestId('model-search')).toBeFocused()
  return picker
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
})

test('phone model picker keeps search, filters, and options touch-sized', async ({ page }) => {
  const picker = await openTopbarPicker(page, 390, 844)
  await expectTouchSized(picker.getByTestId('model-search'))
  await expectIOSFocusSafe(picker.getByTestId('model-search'))
  await expectTouchSized(picker.locator('.model-picker-providers button'))
  await expectTouchSized(picker.getByRole('option'))

  const popover = await picker.getByTestId('model-picker-popover').boundingBox()
  expect(popover).not.toBeNull()
  expect(popover!.x).toBeGreaterThanOrEqual(0)
  expect(popover!.x + popover!.width).toBeLessThanOrEqual(390)
  expect(popover!.y).toBeGreaterThanOrEqual(0)
  expect(popover!.y + popover!.height).toBeLessThanOrEqual(844)
})

test('short landscape model picker stays contained and touch-sized', async ({ page }) => {
  const picker = await openTopbarPicker(page, 852, 393)
  await expectTouchSized(picker.getByTestId('model-search'))
  await expectIOSFocusSafe(picker.getByTestId('model-search'))
  await expectTouchSized(picker.locator('.model-picker-providers button'))
  await expectTouchSized(picker.getByRole('option'))

  const popover = await picker.getByTestId('model-picker-popover').boundingBox()
  expect(popover).not.toBeNull()
  expect(popover!.y).toBeGreaterThanOrEqual(0)
  expect(popover!.y + popover!.height).toBeLessThanOrEqual(393)
})

test('wide fine-pointer model picker preserves compact pointer-oriented filters', async ({ page }) => {
  const finePointer = await page.evaluate(() => matchMedia('(hover: hover) and (pointer: fine)').matches)
  test.skip(!finePointer, 'Compact controls are for mouse/trackpad-style pointer contexts.')

  const picker = await openTopbarPicker(page, 1440, 900)
  const search = await picker.getByTestId('model-search').boundingBox()
  const filter = await picker.getByRole('button', { name: 'All', exact: true }).boundingBox()
  expect(search).not.toBeNull()
  expect(filter).not.toBeNull()
  const fontSize = await picker.getByTestId('model-search').evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))
  expect(search!.height).toBeLessThan(44)
  expect(filter!.height).toBeLessThan(44)
  expect(fontSize).toBeLessThan(16)
})
