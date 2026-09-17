import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('focused mobile controls cannot scroll the document or pan the picker shell', async ({ page }, testInfo) => {
  test.skip(!testInfo.project.name.startsWith('mobile-'))

  const textarea = page.getByRole('textbox', { name: 'Message Yeet' })
  await textarea.focus()
  await page.evaluate(() => {
    window.scrollTo(0, 200)
    document.documentElement.scrollTop = 200
    document.body.scrollTop = 200
  })

  await expect.poll(() => page.evaluate(() => ({
    scrollX: window.scrollX,
    scrollY: window.scrollY,
    htmlPosition: getComputedStyle(document.documentElement).position,
    bodyPosition: getComputedStyle(document.body).position,
    appPosition: getComputedStyle(document.querySelector('#app')!).position,
  }))).toEqual({ scrollX: 0, scrollY: 0, htmlPosition: 'fixed', bodyPosition: 'fixed', appPosition: 'fixed' })

  const picker = page.locator('.top-bar').getByTestId('model-picker')
  await picker.getByTestId('model-picker-trigger').click()
  await expect(picker.getByTestId('model-search')).toBeFocused()
  await expect.poll(() => picker.evaluate((root) => ({
    pickerTouchAction: getComputedStyle(root).touchAction,
    listTouchAction: getComputedStyle(root.querySelector('.model-picker-list')!).touchAction,
  }))).toEqual({ pickerTouchAction: 'none', listTouchAction: 'pan-y' })
})
