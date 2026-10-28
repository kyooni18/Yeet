import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test('settings editor keeps its initial value and local draft across updates', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await page.getByRole('button', { name: 'Settings', exact: true }).click()
  await expect(page.getByText('Settings', { exact: true }).last()).toBeVisible()

  const emit = (revision: number, editor: object | null, working = false) => page.evaluate(({ revision, editor, working }) => {
    ;(window as unknown as { __yeetEmit: (message: unknown) => void }).__yeetEmit({
      type: 'ui_settings', version: 1, settings_revision: revision,
      view: { title: 'Settings', subtitle: '', sections: [], selected: null, editor, notice: null, working },
    })
  }, { revision, editor, working })
  const editor = { kind: 'web_endpoint', label: 'Endpoint', value: 'https://example.test', hint: '' }
  await emit(100, editor)
  const field = page.getByPlaceholder('Endpoint')
  await expect(field).toHaveValue('https://example.test')
  await field.fill('https://draft.test')
  await emit(101, editor, true)
  await expect(field).toHaveValue('https://draft.test')
  await expect(page.getByRole('button', { name: 'Save' })).toBeDisabled()
  await emit(102, editor)
  await page.getByRole('button', { name: 'Save' }).click()
  await expect.poll(() => page.evaluate(() => (window as unknown as { __yeetSent: Array<{ type: string; action?: unknown }> }).__yeetSent
    .some(message => message.type === 'ui_settings_action' && JSON.stringify(message.action) === JSON.stringify({ type: 'submit_editor', value: 'https://draft.test' })))).toBe(true)
})
