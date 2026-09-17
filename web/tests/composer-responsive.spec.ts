import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('portrait phones keep the composer compact with touch-safe actions', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width > 600 || viewport.width >= viewport.height)

  const composer = page.getByTestId('composer')
  const send = page.getByTestId('submit')
  const textarea = page.getByRole('textbox', { name: 'Message Yeet' })

  const composerBox = await composer.boundingBox()
  const sendBox = await send.boundingBox()
  expect(composerBox).not.toBeNull()
  expect(sendBox).not.toBeNull()
  expect(composerBox?.height ?? 999).toBeLessThanOrEqual(96)
  expect(sendBox?.width ?? 0).toBeGreaterThanOrEqual(44)
  expect(sendBox?.height ?? 0).toBeGreaterThanOrEqual(44)
  await expect(textarea).toHaveCSS('font-size', '16px')

  await textarea.fill('Short message')
  const singleLineBox = await composer.boundingBox()
  expect(singleLineBox).not.toBeNull()
  expect(singleLineBox?.height ?? 999).toBeLessThanOrEqual(96)

  const documentWidth = await page.evaluate(() => document.documentElement.scrollWidth)
  expect(documentWidth).toBeLessThanOrEqual(viewport.width)
})

test('long phone drafts expand upward without losing the send control', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width > 600 || viewport.width >= viewport.height)

  const composer = page.getByTestId('composer')
  const textarea = page.getByRole('textbox', { name: 'Message Yeet' })
  const send = page.getByTestId('submit')
  const initial = await composer.boundingBox()

  await textarea.fill(Array.from({ length: 14 }, (_, index) => `Line ${index + 1}: keep the active task visible while composing.`).join('\n'))

  const expanded = await composer.boundingBox()
  const sendBox = await send.boundingBox()
  expect(initial).not.toBeNull()
  expect(expanded).not.toBeNull()
  expect(expanded?.height ?? 0).toBeGreaterThan(initial?.height ?? 0)
  expect(expanded?.height ?? 999).toBeLessThanOrEqual(Math.min(230, viewport.height * 0.42))
  expect(sendBox?.height ?? 0).toBeGreaterThanOrEqual(44)
  expect((expanded?.y ?? 0) + (expanded?.height ?? 0)).toBeLessThanOrEqual(viewport.height - 8)

  await textarea.fill('')
  const collapsed = await composer.boundingBox()
  expect(collapsed).not.toBeNull()
  expect(collapsed?.height ?? 999).toBeLessThanOrEqual(96)
})

test('oversized backend errors stay inspectable without displacing the mobile composer', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width >= 900)

  await page.evaluate((message) => {
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
      type: 'error', version: 1, sequence: 2, revision: 2, message,
    })
  }, `Backend failure: ${'detailed recovery context '.repeat(240)}final-sentinel`)

  const error = page.locator('.inline-error')
  const composer = page.getByTestId('composer')
  await expect(error).toContainText('final-sentinel')

  const errorBox = await error.boundingBox()
  const composerBox = await composer.boundingBox()
  expect(errorBox).not.toBeNull()
  expect(composerBox).not.toBeNull()
  expect(errorBox!.y).toBeGreaterThanOrEqual(0)
  expect(composerBox!.y + composerBox!.height).toBeLessThanOrEqual(viewport.height + 1)

  const scrollState = await error.evaluate((element) => ({
    overflowY: getComputedStyle(element).overflowY,
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
    tabIndex: element.tabIndex,
  }))
  expect(scrollState.scrollHeight).toBeGreaterThan(scrollState.clientHeight)
  expect(['auto', 'scroll']).toContain(scrollState.overflowY)
  expect(scrollState.tabIndex).toBe(0)

  await error.focus()
  await expect(error).toBeFocused()
  await page.keyboard.press('PageDown')
  await expect.poll(() => error.evaluate((element) => element.scrollTop)).toBeGreaterThan(0)
})

test('stacked permission and failure notices keep mobile decisions and composer reachable', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width >= 900)

  await page.evaluate(({ errorMessage, command }) => {
    const emit = (window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit
    emit({ type: 'error', version: 1, sequence: 2, revision: 2, message: errorMessage })
    emit({
      type: 'state_update', version: 1, sequence: 3, revision: 3,
      patch: {
        pending_shell_permission: {
          id: 'stacked-layout', kind: 'shell', command, operation: 'execute', reason: 'Review before running',
        },
      },
    })
  }, {
    errorMessage: `Backend failure: ${'recovery context '.repeat(160)}error-sentinel`,
    command: `echo ${'permission-segment '.repeat(220)}command-sentinel`,
  })

  const composerZone = page.locator('.composer-zone')
  const permission = page.getByTestId('permission-prompt')
  const composer = page.getByTestId('composer')
  await expect(permission).toBeVisible()
  await expect(page.locator('.inline-error')).toContainText('error-sentinel')
  await expect(permission.locator('#permission-detail')).toContainText('command-sentinel')

  const zoneBox = await composerZone.boundingBox()
  const composerBox = await composer.boundingBox()
  const allowBox = await permission.getByRole('button', { name: 'Allow' }).boundingBox()
  expect(zoneBox).not.toBeNull()
  expect(composerBox).not.toBeNull()
  expect(allowBox).not.toBeNull()
  expect(zoneBox!.y).toBeGreaterThanOrEqual(0)
  expect(composerBox!.y + composerBox!.height).toBeLessThanOrEqual(viewport.height + 1)
  expect(allowBox!.y).toBeGreaterThanOrEqual(0)
})


test('wide touch composer keeps primary controls touch-safe without switching layouts', async ({ page }) => {
  const viewport = page.viewportSize()
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  test.skip(!touchFirst || !viewport || viewport.width < 900)

  const textarea = page.getByRole('textbox', { name: 'Message Yeet' })
  const sessionControls = page.locator('.composer-context')
  const send = page.getByTestId('submit')

  for (const control of [sessionControls, send]) {
    const box = await control.boundingBox()
    expect(box).not.toBeNull()
    expect(box!.height).toBeGreaterThanOrEqual(44)
  }
  const sendBox = await send.boundingBox()
  expect(sendBox!.width).toBeGreaterThanOrEqual(44)
  expect(await textarea.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))).toBeGreaterThanOrEqual(16)
  const textareaBox = await textarea.boundingBox()
  expect(textareaBox).not.toBeNull()
  expect(textareaBox!.height).toBeGreaterThanOrEqual(44)

  await page.evaluate(() => {
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
      type: 'state_update', version: 1, sequence: 2, revision: 2, patch: { is_streaming: true },
    })
  })
  const stop = page.getByTestId('interrupt')
  await expect(stop).toBeVisible()
  const stopBox = await stop.boundingBox()
  expect(stopBox).not.toBeNull()
  expect(stopBox!.width).toBeGreaterThanOrEqual(44)
  expect(stopBox!.height).toBeGreaterThanOrEqual(44)
})

test('wide fine-pointer composer preserves compact desktop density', async ({ page }) => {
  const viewport = page.viewportSize()
  const finePointer = await page.evaluate(() => matchMedia('(hover: hover) and (pointer: fine)').matches)
  test.skip(!finePointer || !viewport || viewport.width < 900)

  const textarea = page.getByRole('textbox', { name: 'Message Yeet' })
  const sessionControls = await page.locator('.composer-context').boundingBox()
  const send = await page.getByTestId('submit').boundingBox()
  expect(sessionControls).not.toBeNull()
  expect(send).not.toBeNull()
  expect(sessionControls!.height).toBeLessThan(44)
  expect(send!.height).toBeLessThan(44)
  expect(await textarea.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))).toBeLessThan(16)
})
