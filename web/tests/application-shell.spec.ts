import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('desktop navigation resizes the workspace without turning it into a page card', async ({ page }) => {
  const desktop = await page.evaluate(() =>
    matchMedia('(min-width: 1000px) and (hover: hover) and (pointer: fine)').matches
  )
  test.skip(!desktop)

  const workspace = page.locator('.main-viewport')
  const sidebar = page.locator('.remote-sidebar')
  await expect(sidebar).toHaveClass(/is-open/)

  const openBox = await workspace.boundingBox()
  expect(openBox).not.toBeNull()

  const openStyle = await workspace.evaluate((element) => {
    const style = getComputedStyle(element)
    return {
      position: style.position,
      transform: style.transform,
      borderRadius: style.borderRadius,
    }
  })
  expect(openStyle).toEqual({
    position: 'relative',
    transform: 'none',
    borderRadius: '0px',
  })

  await sidebar.getByRole('button', { name: 'Close sidebar' }).click()
  await expect(sidebar).not.toHaveClass(/is-open/)
  await expect.poll(async () => (await workspace.boundingBox())?.width ?? 0).toBeGreaterThan(openBox!.width)

  const closedBox = await workspace.boundingBox()
  expect(closedBox).not.toBeNull()
  expect(closedBox!.x).toBeLessThan(openBox!.x)
  expect(await workspace.evaluate((element) => getComputedStyle(element).transform)).toBe('none')
})

test('wide desktop controls dock beside the workspace instead of covering it', async ({ page }) => {
  const dockedInspector = await page.evaluate(() =>
    matchMedia('(min-width: 1180px) and (hover: hover) and (pointer: fine)').matches
  )
  test.skip(!dockedInspector)

  const workspace = page.locator('.main-viewport')
  const before = await workspace.boundingBox()
  expect(before).not.toBeNull()

  await page.getByRole('button', { name: 'Quick settings' }).click()
  const inspector = page.locator('.quick-panel')
  await expect(inspector).toHaveClass(/is-open/)
  await expect.poll(async () => (await workspace.boundingBox())?.width ?? Number.POSITIVE_INFINITY)
    .toBeLessThan(before!.width)

  const workspaceBox = await workspace.boundingBox()
  const inspectorBox = await inspector.boundingBox()
  expect(workspaceBox).not.toBeNull()
  expect(inspectorBox).not.toBeNull()
  expect(inspectorBox!.x).toBeGreaterThanOrEqual(workspaceBox!.x + workspaceBox!.width - 1)

  const inspectorStyle = await inspector.evaluate((element) => {
    const style = getComputedStyle(element)
    return { position: style.position, borderRadius: style.borderRadius }
  })
  expect(inspectorStyle).toEqual({ position: 'relative', borderRadius: '0px' })
})
