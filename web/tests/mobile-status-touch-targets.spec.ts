import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

async function openSessionControls(page: Page, width: number, height: number) {
  await page.setViewportSize({ width, height })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  const topStatus = page.getByTestId('open-status')
  if (await topStatus.isVisible()) await topStatus.click()
  else await page.getByRole('button', { name: /^Session controls:/ }).click()
  const panel = page.getByTestId('session-controls')
  await expect(panel).toBeVisible()
  return panel
}

async function expectButtonsAtLeast(panel: ReturnType<Page['getByTestId']>, height: number) {
  const sizes = await panel.locator('button:visible').evaluateAll((buttons) => buttons.map((button) => {
    const rect = button.getBoundingClientRect()
    return { label: button.getAttribute('aria-label') || button.textContent || '', height: Math.round(rect.height) }
  }))
  expect(sizes.length).toBeGreaterThan(0)
  for (const button of sizes) expect(button.height, button.label).toBeGreaterThanOrEqual(height)
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
})


test('phone session-controls opener is touch-sized', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()

  const opener = page.getByRole('button', { name: /^Session controls:/ })
  const box = await opener.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.width).toBeGreaterThanOrEqual(44)
  expect(box!.height).toBeGreaterThanOrEqual(44)
})


test('wide session-controls opener adapts to pointer type', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width < 900)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()

  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  const opener = page.getByRole('button', { name: /^Session controls:/ })
  const box = await opener.boundingBox()
  expect(box).not.toBeNull()
  if (touchFirst) {
    expect(box!.width).toBeGreaterThanOrEqual(44)
    expect(box!.height).toBeGreaterThanOrEqual(44)
  } else {
    expect(box!.height).toBeLessThan(44)
  }
})

test('phone session controls keep every visible action touch-sized', async ({ page }) => {
  const panel = await openSessionControls(page, 390, 844)
  await expectButtonsAtLeast(panel, 44)
  await expect(panel.getByRole('button', { name: 'high', exact: true })).toHaveAttribute('aria-pressed', 'true')
  await expect(panel.getByRole('button', { name: 'OFF', exact: true })).toHaveAttribute('aria-pressed', 'true')
})

test('short landscape session controls fit the viewport and remain touch-sized', async ({ page }) => {
  const panel = await openSessionControls(page, 852, 393)
  await expectButtonsAtLeast(panel, 44)
  const box = await panel.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.y).toBeGreaterThanOrEqual(0)
  expect(box!.y + box!.height).toBeLessThanOrEqual(393)
})

test('wide session controls adapt action density to pointer type', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width < 900)

  const panel = await openSessionControls(page, viewport.width, viewport.height)
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  if (touchFirst) {
    await expectButtonsAtLeast(panel, 44)
    return
  }

  const closeBox = await panel.getByRole('button', { name: 'Close session controls' }).boundingBox()
  const permissionBox = await panel.getByRole('button', { name: /Ask first/ }).boundingBox()
  expect(closeBox).not.toBeNull()
  expect(permissionBox).not.toBeNull()
  expect(closeBox!.height).toBeLessThan(44)
  expect(permissionBox!.height).toBeLessThan(44)
})


test('wide touch session controls keep model search and provider filters touch-sized', async ({ page }) => {
  const viewport = page.viewportSize()
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  test.skip(!touchFirst || !viewport || viewport.width < 900)

  const panel = await openSessionControls(page, viewport.width, viewport.height)
  await page.evaluate(() => {
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
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
  await panel.getByTestId('model-picker-trigger').click()

  const search = panel.getByTestId('model-search')
  await expect(search).toBeVisible()
  const searchBox = await search.boundingBox()
  expect(searchBox).not.toBeNull()
  expect(searchBox!.height).toBeGreaterThanOrEqual(44)

  const providerButtons = panel.locator('.model-picker-providers button:visible')
  expect(await providerButtons.count()).toBeGreaterThan(0)
  for (const button of await providerButtons.all()) {
    const box = await button.boundingBox()
    expect(box).not.toBeNull()
    expect(box!.width).toBeGreaterThanOrEqual(44)
    expect(box!.height).toBeGreaterThanOrEqual(44)
  }
})
