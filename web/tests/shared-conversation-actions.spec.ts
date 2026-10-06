import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test('shared message controls deliver edits and regeneration through the UI host', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await page.getByRole('button', { name: 'Edit message' }).click()
  await expect(page.locator('.composer-input')).toHaveText('Inspect the **remote UI** and keep it responsive.')
  await page.getByRole('button', { name: 'Regenerate response' }).click()
  await expect.poll(() => page.evaluate(() => {
    const messages = (window as unknown as { __yeetSent: Array<{ type: string; command?: { type: string } }> }).__yeetSent
    return messages.some(message => message.type === 'command' && message.command?.type === 'regenerate_last')
  })).toBe(true)
})
