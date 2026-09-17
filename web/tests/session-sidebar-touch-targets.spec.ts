import { expect, test, type Locator } from '@playwright/test'
import { installMockRemote } from './mockRemote'

async function expectMinSize(locator: Locator, size: number) {
  const box = await locator.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.width).toBeGreaterThanOrEqual(size)
  expect(box!.height).toBeGreaterThanOrEqual(size)
}

async function expectCompactHeight(locator: Locator, maxHeight: number) {
  const box = await locator.boundingBox()
  expect(box).not.toBeNull()
  expect(box!.height).toBeLessThan(maxHeight)
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
})

test('phone session drawer keeps primary controls touch-sized', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await page.getByRole('button', { name: 'Open sessions' }).click()

  const drawer = page.locator('.session-sidebar.is-drawer')
  await expect(drawer).toBeVisible()
  await expectMinSize(drawer.locator('.sidebar-close'), 44)
  await expectMinSize(drawer.locator('.workspace-add-button'), 44)
  await expectMinSize(drawer.locator('.sidebar-footer button'), 44)

  await drawer.locator('.workspace-add-button').click()
  await expectMinSize(drawer.locator('.workspace-open-form input'), 44)
  await expectMinSize(drawer.locator('.workspace-open-form button'), 44)
})

test('wide touch sidebar keeps persistent navigation targets touch-sized', async ({ page }) => {
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  test.skip(!touchFirst, 'Only applies to touch-first pointer contexts.')

  await page.setViewportSize({ width: 1180, height: 820 })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()

  await page.evaluate(() => {
    const sessions = Array.from({ length: 5 }, (_, index) => ({
      id: `touch-session-${index}`,
      title: `Touch session ${index + 1}`,
      updated_at: new Date(Date.now() - index * 60_000).toISOString(),
      model: 'gpt-5.6-sol',
      message_count: index + 1,
    }))
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
      type: 'state_update',
      version: 1,
      sequence: 2,
      revision: 2,
      patch: { saved_sessions: sessions, current_session_id: sessions[0].id },
    })
  })

  const sidebar = page.locator('.desktop-session-sidebar')
  await expect(sidebar).toBeVisible()
  await expectMinSize(sidebar.locator('.new-session-button'), 44)
  await expectMinSize(sidebar.locator('.workspace-add-button'), 44)
  await expectMinSize(sidebar.locator('.workspace-item').first(), 44)
  await expectMinSize(sidebar.locator('.workspace-session-toggle').first(), 44)
  await expectMinSize(sidebar.locator('.session-item').first(), 44)
  await expectMinSize(sidebar.locator('.sidebar-footer button'), 44)

  await sidebar.locator('.workspace-add-button').click()
  await expectMinSize(sidebar.locator('.workspace-open-form input'), 44)
  await expectMinSize(sidebar.locator('.workspace-open-form button'), 44)
})

test('desktop sidebar keeps its compact pointer-oriented density', async ({ page }) => {
  const finePointer = await page.evaluate(() => matchMedia('(hover: hover) and (pointer: fine)').matches)
  test.skip(!finePointer, 'Compact persistent-sidebar density is for mouse/trackpad contexts.')
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()

  const sidebar = page.locator('.desktop-session-sidebar')
  await expect(sidebar).toBeVisible()
  await expectCompactHeight(sidebar.locator('.workspace-add-button'), 44)
  await expectCompactHeight(sidebar.locator('.sidebar-footer button'), 44)

  await sidebar.locator('.workspace-add-button').click()
  await expectCompactHeight(sidebar.locator('.workspace-open-form input'), 44)
  await expectCompactHeight(sidebar.locator('.workspace-open-form button'), 44)
})
