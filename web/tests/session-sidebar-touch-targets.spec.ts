import { expect, test, type Locator } from '@playwright/test'
import { installMockRemote } from './mockRemote'

async function expectMinSize(locator: Locator, size: number) {
  const box = await locator.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.width).toBeGreaterThanOrEqual(size)
  expect(box!.height).toBeGreaterThanOrEqual(size)
}

async function openSessions(page: Parameters<typeof installMockRemote>[0]) {
  await page.getByRole('button', { name: 'Open sidebar' }).click()
  const sidebar = page.locator('.remote-sidebar')
  await expect(sidebar).toBeVisible()
  return sidebar
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
})

test('touch-first session drawer keeps navigation targets at least 44px', async ({ page }) => {
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  test.skip(!touchFirst, 'Touch-first behavior only')

  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  const sidebar = await openSessions(page)

  await expectMinSize(sidebar.getByRole('button', { name: 'Close sidebar' }), 44)
  await expectMinSize(sidebar.getByRole('button', { name: 'Settings', exact: true }), 44)
  await expectMinSize(sidebar.locator('.workspace-picker__button'), 44)
  await expectMinSize(sidebar.locator('.sidebar-session').first(), 44)
  await expectMinSize(sidebar.getByRole('button', { name: 'New session' }), 44)
})

test('wide touch layouts keep the same drawer and touch targets', async ({ page }) => {
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  test.skip(!touchFirst, 'Touch-first behavior only')

  await page.setViewportSize({ width: 1180, height: 820 })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  const sidebar = await openSessions(page)

  await expectMinSize(sidebar.locator('.workspace-picker__button'), 44)
  await expectMinSize(sidebar.locator('.sidebar-session').first(), 44)
  await expectMinSize(sidebar.getByRole('button', { name: 'New session' }), 44)
})

test('fine-pointer drawer keeps compact row density', async ({ page }) => {
  const finePointer = await page.evaluate(() => matchMedia('(hover: hover) and (pointer: fine)').matches)
  test.skip(!finePointer, 'Fine-pointer behavior only')

  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  const sidebar = await openSessions(page)

  const workspace = await sidebar.locator('.workspace-picker__button').boundingBox()
  const session = await sidebar.locator('.sidebar-session').first().boundingBox()
  expect(workspace).not.toBeNull()
  expect(session).not.toBeNull()
  expect(workspace!.height).toBeLessThan(44)
  expect(session!.height).toBeLessThan(44)
})
