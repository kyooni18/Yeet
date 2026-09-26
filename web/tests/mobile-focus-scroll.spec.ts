import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('focused mobile controls keep the document fixed while the model sheet pans internally', async ({ page }, testInfo) => {
  test.skip(!testInfo.project.name.startsWith('mobile-'))

  const textarea = page.getByRole('textbox', { name: 'Message' })
  await textarea.focus()

  await page.evaluate(() => {
    window.scrollTo(0, 200)
    document.documentElement.scrollTop = 200
    document.body.scrollTop = 200
  })

  await expect.poll(() => page.evaluate(() => ({
    scrollX: window.scrollX,
    scrollY: window.scrollY,
    htmlOverflow: getComputedStyle(document.documentElement).overflow,
    bodyOverflow: getComputedStyle(document.body).overflow,
  }))).toEqual({
    scrollX: 0,
    scrollY: 0,
    htmlOverflow: 'hidden',
    bodyOverflow: 'hidden',
  })

  await page.getByRole('button', { name: /Choose model, current/ }).click()
  const dialog = page.getByRole('dialog', { name: 'Choose model' })
  const search = dialog.getByPlaceholder('Search models')
  await expect(dialog).toBeVisible()
  await expect(search).toBeFocused()

  await expect.poll(() => dialog.evaluate((root) => ({
    sheetTouchAction: getComputedStyle(root).touchAction,
    listTouchAction: getComputedStyle(root.querySelector('.model-sheet__scroll')!).touchAction,
    searchFontSize: Number.parseFloat(getComputedStyle(root.querySelector<HTMLInputElement>('input[placeholder="Search models"]')!).fontSize),
  }))).toEqual({
    sheetTouchAction: 'pinch-zoom',
    listTouchAction: 'pan-y pinch-zoom',
    searchFontSize: 16,
  })

  const viewport = page.viewportSize()!
  const documentWidth = await page.evaluate(() => document.documentElement.scrollWidth)
  expect(documentWidth).toBeLessThanOrEqual(viewport.width)
})
