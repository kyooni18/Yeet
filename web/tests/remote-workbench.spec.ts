import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

async function emit(page: Page, patch: Record<string, unknown>, sequence = 2) {
  await page.evaluate(({ patch, sequence }) => {
    (window as unknown as { __yeetEmit: (message: unknown) => void }).__yeetEmit({
      type: 'state_update', version: 1, sequence, revision: sequence, patch,
    })
  }, { patch, sequence })
}

test.beforeEach(async ({ page }) => {
  await page.route('https://static.cloudflareinsights.com/**', route => route.fulfill({ body: '' }))
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('large workspace and session lists scroll without displacing navigation or settings', async ({ page }) => {
  await emit(page, {
    known_workspaces: Array.from({ length: 50 }, (_, i) => ({
      id: i ? `/work/${i}` : '/Users/test/Code/Rust/Yeet',
      path: i ? `/work/${i}` : '/Users/test/Code/Rust/Yeet',
      display_name: i ? `Workspace ${i}` : 'Yeet', updated_at: new Date().toISOString(), session_count: 1, is_current: !i,
    })),
    saved_sessions: Array.from({ length: 60 }, (_, i) => ({
      id: `session-${i}`, title: `Session ${i}`, updated_at: new Date().toISOString(), model: 'gpt-5.6-sol', message_count: 1,
    })),
  })
  const sidebar = page.locator('#sidebar')
  if (!await sidebar.isVisible()) await page.getByRole('button', { name: 'Toggle sidebar', exact: true }).click()
  await sidebar.getByRole('button', { name: 'Switch workspace' }).click()
  const list = page.getByRole('region', { name: 'Workspaces', exact: true })
  await expect(list.getByRole('button')).toHaveCount(50)
  expect(await list.evaluate(el => el.scrollHeight > el.clientHeight)).toBe(true)
  await list.getByRole('button').last().focus()
  await expect(list.getByRole('button').last()).toBeInViewport()
  expect(await list.evaluate(el => el.scrollTop)).toBeGreaterThan(0)
  await expect(sidebar.getByRole('button', { name: 'Settings', exact: true })).toBeInViewport()
  const scroll = sidebar.locator('.sidebar-scroll')
  expect(await scroll.evaluate(el => el.scrollHeight > el.clientHeight)).toBe(true)
  await sidebar.locator('.session-row').last().focus()
  await expect(sidebar.locator('.session-row').last()).toBeInViewport()
  await expect(sidebar.getByRole('button', { name: 'New session', exact: true })).toBeInViewport()
  await expect(sidebar.getByRole('button', { name: 'Settings', exact: true })).toBeInViewport()
  await sidebar.getByRole('searchbox', { name: 'Search sessions' }).fill('not a session')
  await expect(sidebar.getByText('No matching sessions')).toBeVisible()
})

test('workspace panel reads live changes and files, preserves the draft, and remembers dismissal', async ({ page }, testInfo) => {
  const editor = page.getByRole('textbox', { name: 'Message', exact: true })
  await editor.fill('Keep this draft while I browse')
  const panel = page.locator('#workspace-panel')
  if (!await panel.isVisible()) await page.getByRole('button', { name: 'Toggle workspace panel' }).click()
  await expect(panel.locator('.panel-row').first()).toContainText('src/limiter.ts')
  await expect(panel.locator('.panel-summary')).toContainText('2 changed files')
  await expect(panel.getByLabel('Context usage')).toHaveAttribute('value', '21')
  await expect(panel.getByText('42K / 200K tokens')).toBeAttached()
  await expect(panel.getByRole('button', { name: 'Model gpt-5.6-sol' })).toBeAttached()
  await expect(panel.getByLabel('Diff preview')).toContainText('refill')
  await panel.getByRole('button', { name: 'docs/notes.md' }).click()
  await expect(panel.getByRole('button', { name: 'docs/notes.md' })).toHaveAttribute('aria-pressed', 'true')
  await panel.getByRole('button', { name: 'Files', exact: true }).click()
  await panel.getByRole('button', { name: /^src/ }).click()
  await panel.getByRole('button', { name: /^limiter.ts/ }).click()
  await expect(panel.getByText('FILE DETAILS')).toBeVisible()
  await panel.getByRole('button', { name: 'Review changes →' }).click()
  await expect(panel.getByLabel('Diff preview')).toBeVisible()
  await expect(editor).toHaveValue('Keep this draft while I browse')
  const reads = () => page.evaluate(() => (window as unknown as { __yeetSent: { type: string }[] })
    .__yeetSent.filter(message => message.type === 'workspace_changes_request').length)
  const beforeRun = await reads()
  await emit(page, { is_streaming: true })
  await expect(page.locator('.send-button [data-mun="RoundedRectangle"]')).toHaveCount(1)
  await emit(page, { is_streaming: false }, 3)
  await expect.poll(reads).toBeGreaterThan(beforeRun)
  await page.screenshot({ path: testInfo.outputPath('workspace-panel.png') })
  await panel.getByRole('button', { name: 'Hide workspace panel' }).click()
  await expect(panel).toHaveCount(0)
  await expect(editor).toHaveValue('Keep this draft while I browse')
  await page.reload()
  await expect(page.getByText('Interface ready')).toBeVisible()
  await expect(panel).toHaveCount(0)
  await page.getByRole('button', { name: 'Toggle workspace panel' }).click()
  await expect(panel.locator('.panel-row').first()).toContainText('src/limiter.ts')
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
})

test('overlay drawers contain keyboard focus, dismiss with Escape, and return focus', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 0) >= 1280, 'Docked panels are deliberately non-modal')
  const toggle = page.getByRole('button', { name: 'Toggle workspace panel' })
  await toggle.click()
  const panel = page.getByRole('dialog', { name: 'Workspace panel' })
  await expect(panel).toHaveAttribute('aria-modal', 'true')
  await expect(page.locator('#main')).toHaveAttribute('inert', '')
  const last = panel.getByRole('button', { name: 'Agent activity' })
  await last.focus()
  await page.keyboard.press('Tab')
  await expect(panel.getByRole('button', { name: 'Refresh workspace' })).toBeFocused()
  await page.keyboard.press('Shift+Tab')
  await expect(last).toBeFocused()
  await page.keyboard.press('Escape')
  await expect(panel).toHaveCount(0)
  await expect(page.locator('#main')).not.toHaveAttribute('inert', '')
  await expect(toggle).toBeFocused()
  if ((page.viewportSize()?.width ?? 0) < 900) {
    const navToggle = page.getByRole('button', { name: 'Toggle sidebar', exact: true })
    await navToggle.click()
    await expect(page.getByRole('dialog', { name: 'Workspace navigation' })).toHaveAttribute('aria-modal', 'true')
    await page.keyboard.press('Escape')
    await expect(page.locator('#sidebar')).toHaveCount(0)
    await expect(navToggle).toBeFocused()
  }
})

test('transcript follows growing output but preserves a reader’s scroll position', async ({ page }) => {
  const thread = page.getByRole('region', { name: 'Conversation', exact: true })
  const content = Array.from({ length: 70 }, (_, i) => `Paragraph ${i}: a readable update about the workspace.`).join('\n\n')
  const entries = (text: string) => [{ id: 'long-response', kind: { type: 'assistant', content: text } }]
  await emit(page, { conversation: entries(content) })
  await expect(page.getByText('Paragraph 69:', { exact: false })).toBeVisible()
  const distance = () => thread.evaluate(el => el.scrollHeight - el.clientHeight - el.scrollTop)
  await expect.poll(distance).toBeLessThan(3)
  await thread.evaluate(el => { el.scrollTop = 0 })
  const jump = page.getByRole('button', { name: 'Jump to latest' })
  await expect(jump).toBeVisible()
  // Same custom markdown element: ResizeObserver must see shadow-DOM growth.
  await emit(page, { conversation: entries(content + '\n\nNew output at the bottom.') }, 3)
  await expect.poll(() => thread.evaluate(el => el.scrollTop)).toBeLessThan(3)
  await jump.click()
  await expect.poll(distance).toBeLessThan(3)
  await expect(jump).toHaveCount(0)
  await emit(page, { conversation: entries(content + '\n\n' + 'More output.\n\n'.repeat(20)) }, 4)
  await expect.poll(distance).toBeLessThan(3)
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeInViewport()
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeInViewport()
  await emit(page, { conversation: [] }, 5)
  await expect(page.getByText('What should we work on?', { exact: true })).toBeInViewport()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeInViewport()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
})
