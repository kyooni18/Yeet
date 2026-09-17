import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('wide touch layouts keep the top bar touch-safe without switching to the phone layout', async ({ page }) => {
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  const viewportWidth = page.viewportSize()?.width ?? 0
  test.skip(!touchFirst || viewportWidth < 900, 'Only applies to desktop-width touch layouts such as iPad landscape.')

  await expect(page.getByTestId('open-sessions')).toBeHidden()

  await page.evaluate(() => {
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
      type: 'state_update',
      version: 1,
      sequence: 2,
      revision: 2,
      patch: {
        active_model: 'openai/gpt-5.6-sol',
        available_models: [
          'openai/gpt-5.6-sol',
          'anthropic/claude-opus-4.1',
          'google/gemini-3-pro',
        ],
      },
    })
  })

  const model = page.getByTestId('model-picker-trigger')
  const goal = page.getByTestId('toggle-goal')
  const inspector = page.getByTestId('toggle-inspector')

  for (const control of [model, goal, inspector]) {
    await expect(control).toBeVisible()
    const box = await control.boundingBox()
    expect(box).not.toBeNull()
    expect(box!.height).toBeGreaterThanOrEqual(44)
  }

  for (const control of [goal, inspector]) {
    const box = await control.boundingBox()
    expect(box).not.toBeNull()
    expect(box!.width).toBeGreaterThanOrEqual(44)
  }

  await model.click()
  const picker = page.locator('.top-bar').getByTestId('model-picker')
  const search = picker.getByTestId('model-search')
  const filters = picker.locator('.model-picker-providers button')

  await expect(search).toBeFocused()
  const searchBox = await search.boundingBox()
  expect(searchBox).not.toBeNull()
  expect(searchBox!.height).toBeGreaterThanOrEqual(44)
  expect(await search.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))).toBeGreaterThanOrEqual(16)

  const filterBoxes = await filters.evaluateAll((elements) => elements.map((element) => {
    const rect = element.getBoundingClientRect()
    return { width: rect.width, height: rect.height }
  }))
  expect(filterBoxes.length).toBeGreaterThan(1)
  for (const box of filterBoxes) {
    expect(box.width).toBeGreaterThanOrEqual(44)
    expect(box.height).toBeGreaterThanOrEqual(44)
  }
})
