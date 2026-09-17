import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetSent: Array<Record<string, unknown>>
}

async function submittedTexts(page: Page) {
  return page.evaluate(() => (window as unknown as TestHooks).__yeetSent
    .filter((item) => item.type === 'command')
    .map((item) => item.command as Record<string, unknown>)
    .filter((command) => command.type === 'submit')
    .map((command) => String(command.text ?? '')))
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('plain Enter preserves multiline drafting on touch-first devices and sends on desktop', async ({ page }) => {
  const composer = page.getByRole('textbox', { name: 'Message Yeet' })
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)

  await composer.fill('first line')
  await composer.press('Enter')

  if (touchFirst) {
    await composer.type('second line')
    await expect(composer).toHaveValue('first line\nsecond line')
    await expect.poll(() => submittedTexts(page)).toEqual([])

    await page.getByTestId('submit').click()
    await expect.poll(() => submittedTexts(page)).toEqual(['first line\nsecond line'])
  } else {
    await expect(composer).toHaveValue('')
    await expect.poll(() => submittedTexts(page)).toEqual(['first line'])
  }
})

test('touch-first hardware keyboard shortcut can still submit with Control+Enter', async ({ page }) => {
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  test.skip(!touchFirst, 'Touch-first behavior only')

  const composer = page.getByRole('textbox', { name: 'Message Yeet' })
  await composer.fill('send from hardware keyboard')
  await composer.press('Control+Enter')

  await expect(composer).toHaveValue('')
  await expect.poll(() => submittedTexts(page)).toEqual(['send from hardware keyboard'])
})
