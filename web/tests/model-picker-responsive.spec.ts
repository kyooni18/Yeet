import { expect, test, type Locator, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = { __yeetEmit: (message: Record<string, unknown>) => void }

async function emitModels(page: Page, count = 4) {
  const base = [
    'openai/gpt-5.6-sol',
    'anthropic/claude-opus-4.1',
    'google/gemini-3-pro',
    'xai/grok-4',
  ]
  const extras = Array.from({ length: Math.max(0, count - base.length) }, (_, index) => `openrouter/model-${index + 1}`)
  const availableModels = [...base, ...extras]
  await page.evaluate((models) => {
    ;(window as unknown as TestHooks).__yeetEmit({
      type: 'state_update',
      version: 1,
      sequence: 2,
      revision: 2,
      patch: {
        active_model: 'openai/gpt-5.6-sol',
        available_models: models,
        model_catalog: [],
      },
    })
  }, availableModels)
}

async function expectMinSize(locator: Locator, size: number) {
  const boxes = await locator.evaluateAll((elements) => elements.map((element) => {
    const rect = element.getBoundingClientRect()
    return {
      width: Math.round(rect.width),
      height: Math.round(rect.height),
      label: element.textContent?.trim() || element.getAttribute('aria-label') || '',
    }
  }))
  expect(boxes.length).toBeGreaterThan(0)
  for (const box of boxes) {
    expect(box.width, box.label).toBeGreaterThanOrEqual(size)
    expect(box.height, box.label).toBeGreaterThanOrEqual(size)
  }
}

async function openResponsiveSheet(page: Page, width: number, height: number, modelCount = 4) {
  await page.setViewportSize({ width, height })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await emitModels(page, modelCount)

  await page.getByRole('button', { name: /Choose model, current/ }).click()
  const dialog = page.getByRole('dialog', { name: 'Choose model' })
  await expect(dialog).toBeVisible()
  await expect(dialog.getByPlaceholder('Search models')).toBeFocused()
  await dialog.evaluate(async (element) => {
    await Promise.all(element.getAnimations().map((animation) => animation.finished.catch(() => undefined)))
  })
  return dialog
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
})

test('phone model sheet keeps search and model rows touch-sized and contained', async ({ page }) => {
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  test.skip(!touchFirst, 'Phone touch sizing applies to coarse-pointer environments.')
  const dialog = await openResponsiveSheet(page, 390, 844)
  const search = dialog.getByPlaceholder('Search models')

  await expectMinSize(dialog.locator('.sheet-search'), 44)
  await expectMinSize(dialog.getByRole('button', { name: 'Done' }), 44)
  await expectMinSize(dialog.locator('.model-row'), 44)
  await expect.poll(() => search.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))).toBeGreaterThanOrEqual(16)

  const box = await dialog.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.x).toBeGreaterThanOrEqual(0)
  expect(box!.x + box!.width).toBeLessThanOrEqual(390)
  expect(box!.y).toBeGreaterThanOrEqual(0)
  expect(box!.y + box!.height).toBeLessThanOrEqual(844)

  await search.fill('gemini')
  await expect(dialog.locator('.model-row')).toHaveCount(1)
  await expect(dialog.locator('.model-row')).toContainText('gemini-3-pro')
  await expectMinSize(dialog.getByRole('button', { name: 'Clear search' }), 44)
})

test('short landscape model sheet scrolls internally without escaping the viewport', async ({ page }) => {
  const dialog = await openResponsiveSheet(page, 852, 393, 18)
  const scroll = dialog.locator('.model-sheet__scroll')

  const metrics = await scroll.evaluate((element) => ({
    overflowY: getComputedStyle(element).overflowY,
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
    touchAction: getComputedStyle(element).touchAction,
  }))
  expect(['auto', 'scroll']).toContain(metrics.overflowY)
  expect(metrics.scrollHeight).toBeGreaterThan(metrics.clientHeight)
  expect(metrics.touchAction).toBe('pan-y pinch-zoom')

  const box = await dialog.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.y).toBeGreaterThanOrEqual(0)
  expect(box!.y + box!.height).toBeLessThanOrEqual(393)
  await expectMinSize(dialog.locator('.model-row').first(), 44)
})

test('wide fine-pointer model sheet preserves compact search chrome', async ({ page }) => {
  const finePointer = await page.evaluate(() => matchMedia('(hover: hover) and (pointer: fine)').matches)
  test.skip(!finePointer, 'Compact controls are for mouse/trackpad-style pointer contexts.')

  const dialog = await openResponsiveSheet(page, 1440, 900)
  const searchField = dialog.locator('.sheet-search')
  const search = dialog.getByPlaceholder('Search models')
  const done = dialog.getByRole('button', { name: 'Done' })

  const searchBox = await searchField.boundingBox()
  const doneBox = await done.boundingBox()
  expect(searchBox).not.toBeNull()
  expect(doneBox).not.toBeNull()
  expect(searchBox!.height).toBeLessThanOrEqual(44)
  expect(doneBox!.height).toBeLessThan(44)
  await expect.poll(() => search.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))).toBeLessThan(16)
})
