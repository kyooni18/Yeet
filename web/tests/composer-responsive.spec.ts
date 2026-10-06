import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type Hooks = { __yeetEmit: (message: Record<string, unknown>) => void }

async function emitState(page: Parameters<typeof installMockRemote>[0], sequence: number, patch: Record<string, unknown>) {
  await page.evaluate(({ sequence, patch }) => {
    ;(window as unknown as Hooks).__yeetEmit({
      type: 'state_update',
      version: 1,
      sequence,
      revision: sequence,
      patch,
    })
  }, { sequence, patch })
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('portrait phones keep the composer compact with iOS-sized actions', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width > 600 || viewport.width >= viewport.height)

  const composer = page.locator('.composer-shell')
  const send = page.getByRole('button', { name: 'Send' })
  const textarea = page.getByRole('textbox', { name: 'Message' })

  const composerBox = await composer.boundingBox()
  const sendBox = await send.boundingBox()
  expect(composerBox).not.toBeNull()
  expect(sendBox).not.toBeNull()
  expect(composerBox!.height).toBeLessThanOrEqual(114)
  expect(Math.round(sendBox!.width)).toBe(48)
  expect(Math.round(sendBox!.height)).toBe(48)
  await expect(textarea).toHaveCSS('font-size', '16px')

  await textarea.fill('Short message')
  const documentWidth = await page.evaluate(() => document.documentElement.scrollWidth)
  expect(documentWidth).toBeLessThanOrEqual(viewport.width)
})

test('long phone drafts expand upward while send remains reachable', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width > 600 || viewport.width >= viewport.height)

  const composer = page.locator('.composer-shell')
  const textarea = page.getByRole('textbox', { name: 'Message' })
  const send = page.getByRole('button', { name: 'Send' })
  const initial = await composer.boundingBox()

  await textarea.fill(Array.from(
    { length: 14 },
    (_, index) => `Line ${index + 1}: keep the active task visible while composing.`,
  ).join('\n'))

  const expanded = await composer.boundingBox()
  const sendBox = await send.boundingBox()
  expect(initial).not.toBeNull()
  expect(expanded).not.toBeNull()
  expect(expanded!.height).toBeGreaterThan(initial!.height)
  expect(expanded!.height).toBeLessThanOrEqual(Math.min(230, viewport.height * 0.42))
  expect(sendBox).not.toBeNull()
  expect(Math.round(sendBox!.height)).toBe(48)
  expect(expanded!.y + expanded!.height).toBeLessThanOrEqual(viewport.height + 1)

  await textarea.fill('')
  const collapsed = await composer.boundingBox()
  expect(collapsed).not.toBeNull()
  expect(collapsed!.height).toBeLessThan(expanded!.height)
})

test('large backend errors stay in the transcript and do not displace the composer', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width >= 900)

  await emitState(page, 2, {
    error_message: `Backend failure: ${'detailed recovery context '.repeat(240)}final-sentinel`,
  })

  const error = page.locator('.info-card').filter({ hasText: 'Error' }).last()
  const composer = page.locator('.composer-shell')
  await expect(error).toContainText('final-sentinel')

  const errorBox = await error.boundingBox()
  const composerBox = await composer.boundingBox()
  expect(errorBox).not.toBeNull()
  expect(composerBox).not.toBeNull()
  expect(errorBox!.x).toBeGreaterThanOrEqual(0)
  expect(errorBox!.x + errorBox!.width).toBeLessThanOrEqual(viewport.width + 1)
  expect(composerBox!.y + composerBox!.height).toBeLessThanOrEqual(viewport.height + 1)

  const documentWidth = await page.evaluate(() => document.documentElement.scrollWidth)
  expect(documentWidth).toBeLessThanOrEqual(viewport.width)
})

test('stacked permission and failure state keeps mobile decisions reachable', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width >= 900)

  await emitState(page, 2, {
    error_message: `Backend failure: ${'recovery context '.repeat(80)}error-sentinel`,
    pending_shell_permission: {
      id: 'stacked-layout',
      kind: 'shell',
      command: `echo ${'permission-segment '.repeat(120)}command-sentinel`,
      operation: 'execute',
      reason: 'Review before running',
    },
  })

  const permission = page.locator('.composer-permission')
  const composer = page.locator('.composer-shell')
  const allow = permission.getByRole('button', { name: 'Allow' })
  await expect(permission).toBeVisible()
  await expect(permission).toContainText('command-sentinel')
  await expect(page.locator('.info-card').filter({ hasText: 'Error' }).last()).toContainText('error-sentinel')

  const composerBox = await composer.boundingBox()
  const allowBox = await allow.boundingBox()
  expect(composerBox).not.toBeNull()
  expect(allowBox).not.toBeNull()
  expect(composerBox!.y).toBeGreaterThanOrEqual(0)
  expect(composerBox!.y + composerBox!.height).toBeLessThanOrEqual(viewport.height + 1)
  expect(allowBox!.width).toBeGreaterThanOrEqual(44)
  expect(allowBox!.height).toBeGreaterThanOrEqual(44)
})

test('touch-first wide layouts keep native-sized composer controls', async ({ page }) => {
  const viewport = page.viewportSize()
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  test.skip(!touchFirst || !viewport || viewport.width < 700)

  const textarea = page.getByRole('textbox', { name: 'Message' })
  const add = page.getByRole('button', { name: 'Add attachment' })
  const send = page.getByRole('button', { name: 'Send' })

  for (const control of [add, send]) {
    const box = await control.boundingBox()
    expect(box).not.toBeNull()
    expect(Math.round(box!.width)).toBe(48)
    expect(Math.round(box!.height)).toBe(48)
  }
  expect(await textarea.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))).toBeGreaterThanOrEqual(16)

  await emitState(page, 2, { is_streaming: true })
  const stop = page.getByRole('button', { name: 'Stop' })
  await expect(stop).toBeVisible()
  const stopBox = await stop.boundingBox()
  expect(stopBox).not.toBeNull()
  expect(Math.round(stopBox!.width)).toBe(48)
  expect(Math.round(stopBox!.height)).toBe(48)
})

test('fine-pointer layouts use the compact macOS composer density', async ({ page }) => {
  const viewport = page.viewportSize()
  const finePointer = await page.evaluate(() => matchMedia('(hover: hover) and (pointer: fine)').matches)
  test.skip(!finePointer || !viewport || viewport.width < 900)

  const textarea = page.getByRole('textbox', { name: 'Message' })
  const send = page.getByRole('button', { name: 'Send' })
  const sendBox = await send.boundingBox()

  expect(sendBox).not.toBeNull()
  expect(Math.round(sendBox!.width)).toBe(30)
  expect(Math.round(sendBox!.height)).toBe(24)
  expect(await textarea.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))).toBe(13)
})
