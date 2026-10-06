import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type Hooks = { __yeetSent: Array<Record<string, unknown>> }

async function openDestination(page: Page, name: 'Files' | 'Diff') {
  const sidebar = page.locator('.remote-sidebar')
  if (!(await sidebar.evaluate(element => element.classList.contains('is-open')))) {
    await page.getByRole('button', { name: 'Open sidebar' }).click()
  }
  await sidebar.getByRole('button', { name }).click()
}
const sent = (page: Page, type: string) => page.evaluate(
  (wanted) => (window as unknown as Hooks).__yeetSent.filter(message => message.type === wanted), type)

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('Files opens folders, shows file details and hands changed files to Diff', async ({ page }) => {
  await openDestination(page, 'Files')
  await page.getByRole('button', { name: /^src/ }).click()
  await expect(page.getByRole('navigation', { name: 'Location' })).toContainText('src')

  await page.getByRole('button', { name: /limiter\.ts/ }).click()
  const details = page.getByRole('complementary', { name: 'File details' })
  await expect(details).toContainText('src/limiter.ts')
  await expect(details).toContainText('Changed')

  await details.getByRole('button', { name: 'View changes' }).click()
  await expect(page.getByRole('region', { name: 'Diff' })).toBeVisible()
  const request = (await sent(page, 'workspace_changes_request')).at(-1)
  expect(request?.file).toBe('src/limiter.ts')
})

test('Diff lists changed files, renders added and removed lines, and requests the full file', async ({ page }) => {
  await openDestination(page, 'Diff')
  const patch = page.getByRole('region', { name: /Changes in src\/limiter\.ts/ })
  await expect(patch.locator('.patch-line.is-add')).toHaveCount(2)
  await expect(patch.locator('.patch-line.is-del')).toHaveCount(1)

  await page.getByRole('button', { name: 'Full file' }).click()
  await expect.poll(async () => (await sent(page, 'workspace_changes_request')).at(-1)?.full).toBe(true)
})
