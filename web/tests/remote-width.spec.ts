import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test('shell and composer fill available width, including empty chats and short drafts', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  const check = async () => {
    const bounds = await page.evaluate(() => {
      const root = document.querySelector('#root')!.getBoundingClientRect()
      const shell = document.querySelector('#root > div')!.getBoundingClientRect()
      const main = document.querySelector('#main')!.getBoundingClientRect()
      const composer = document.querySelector('.composer')!
      const surface = document.querySelector('.composer-surface')!
      const editor = document.querySelector('textarea')!
      const contentWidth = (el: Element) => {
        const style = getComputedStyle(el)
        return el.getBoundingClientRect().width - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight)
          - parseFloat(style.borderLeftWidth) - parseFloat(style.borderRightWidth)
      }
      return { root: root.width, shell: shell.width, main: main.width,
        composer: composer.getBoundingClientRect().width, available: contentWidth(composer),
        surface: surface.getBoundingClientRect().width, editorAvailable: contentWidth(surface),
        editor: editor.getBoundingClientRect().width, overflow: document.documentElement.scrollWidth > innerWidth }
    })
    expect(bounds.shell).toBeCloseTo(bounds.root, 0)
    expect(bounds.composer).toBeCloseTo(Math.min(bounds.main, 816), 0)
    expect(bounds.surface).toBeCloseTo(bounds.available, 0)
    expect(bounds.editor).toBeCloseTo(bounds.editorAvailable, 0)
    expect(bounds.overflow).toBe(false)
    if (page.viewportSize()!.width < 900) expect(bounds.main).toBeCloseTo(bounds.root, 0)
  }
  await check()
  await page.evaluate(() => {
    (window as unknown as { __yeetEmit: (message: unknown) => void }).__yeetEmit({
      type: 'conversation_reset', version: 1, sequence: 2, revision: 2, conversation: [],
    })
  })
  await expect(page.getByText('Interface ready')).toHaveCount(0)
  await check()
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Hi')
  await check()
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('A long draft '.repeat(100))
  await check()
})
