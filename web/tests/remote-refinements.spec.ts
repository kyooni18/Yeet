import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

const emit = (page: Page, patch: object) => page.evaluate(patch => (window as any).__yeetEmit({ type: 'state_update', version: 1, sequence: 2, revision: 2, patch }), patch)

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.emulateMedia({ reducedMotion: 'reduce' })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('structured search, MCP and unknown tools are readable until raw inspection', async ({ page }) => {
  const tool = (id: string, name: string, args: object, result: object) => ({ id, kind: { type: 'toolCall', toolCall: { id, name, arguments: JSON.stringify(args), rawResult: JSON.stringify(result), result: 'Captured result', status: 'completed' } } })
  await page.evaluate(conversation => (window as any).__yeetEmit({ type: 'conversation_reset', version: 1, sequence: 2, revision: 2, conversation }), [
    tool('search', 'search_workspace', { query: 'layout', path: 'src' }, { matches: [{ path: 'src/layout.ts', line: 12, content: 'const layout = "<img src=x>"' }], truncated: true }),
    tool('mcp', 'mcp_foundation_search', { query: 'interface' }, { results: [{ text: 'Remember the navigation rule', source_id: 'source-1' }] }),
    tool('unknown', 'inspect_service', { service: 'worker' }, { content: [{ type: 'text', text: '{"state":"ready","jobs":3}' }] }),
  ])
  await page.locator('.activity-group__header').click()
  const rows = page.locator('.activity-group__event .trace-disclosure__row')
  await expect(rows).toHaveCount(3)
  for (let index = 0; index < 3; index++) await rows.nth(index).click()
  await expect(page.locator('.tool-details').first()).toContainText('Path: src/layout.ts')
  await expect(page.locator('.tool-details').first()).toContainText('Line: 12')
  await expect(page.locator('.tool-details').first()).toContainText('Truncated: Yes')
  await expect(page.locator('.tool-details').nth(1)).toContainText('Remember the navigation rule')
  await expect(page.locator('.tool-details').nth(2)).toContainText('State: ready')
  await expect(page.locator('.tool-raw')).toHaveCount(0)
  await expect(page.locator('.tool-details img')).toHaveCount(0)
  await expect(page.locator('.tool-details').first()).not.toContainText('"matches":')
  const raw = page.getByRole('button', { name: 'Raw input and output' }).first()
  await raw.click()
  await expect(raw).toHaveAttribute('aria-expanded', 'true')
  await expect(page.locator('.tool-raw')).toContainText('"matches"')
})

test('status distinguishes missing data, zero usage, and subscription versus metered cost', async ({ page }) => {
  const panel = page.locator('#workspace-panel')
  if (!await panel.isVisible()) await page.getByRole('button', { name: 'Toggle workspace panel' }).click()
  await expect(panel).toContainText('69% remaining')
  await expect(panel.locator('.status-row').filter({ hasText: 'Cost' })).toHaveCount(0)
  await emit(page, { auth_providers: [{ provider: 'openai', authenticated: true, method: 'api_key', usage: null }], current_context_tokens: null, token_usage: { input_tokens: 0 } })
  await expect(panel.locator('.status-row').filter({ hasText: 'Cost' })).toContainText('Not reported by server')
  await expect(panel.locator('.status-row').filter({ hasText: 'Input tokens' })).toContainText('0')
  await expect(panel.locator('.status-row').filter({ hasText: 'Output tokens' })).toContainText('Not reported')
  await expect(panel).toContainText('Usage limits not reported')
  await expect(page.locator('.composer-context')).toHaveAccessibleName('Context usage not reported')
})

test('file previews have numbered source and diff leaves room for the patch', async ({ page }, info) => {
  if (!await page.locator('#sidebar').isVisible()) await page.getByRole('button', { name: 'Toggle sidebar', exact: true }).click()
  await page.locator('#sidebar').getByRole('button', { name: 'Files', exact: true }).click()
  const tree = page.getByRole('region', { name: 'Directory contents' })
  await tree.getByRole('button', { name: /^src/ }).click()
  await tree.getByRole('button', { name: /^limiter.ts/ }).click()
  const source = page.getByRole('region', { name: 'File content for src/limiter.ts' })
  await expect(source.locator('.tool-line-number')).toHaveText(['1', '2', '3'])
  await expect(source).toContainText('<img onerror=alert(1)>')
  await expect(source.locator('img')).toHaveCount(0)
  await page.screenshot({ path: info.outputPath('file-preview.png') })
  await page.getByRole('button', { name: 'Review changes', exact: true }).click()
  if ((page.viewportSize()?.width ?? 0) >= 700) expect((await page.locator('.diff-files').boundingBox())!.width).toBeLessThanOrEqual(200)
  await expect(page.locator('#main .tool-diff__line--added').first().locator('.tool-line-number').nth(1)).toHaveText('11')
  await expect(page.locator('#main .tool-diff__line--removed').first().locator('.tool-line-number').first()).toHaveText('11')
  await page.screenshot({ path: info.outputPath('diff-review.png') })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
})

const workspaces = [
  { id: '/Users/test/Code/Rust/Yeet', path: '/Users/test/Code/Rust/Yeet', display_name: 'Yeet', updated_at: '2026-09-20T01:00:00.000Z', session_count: 1, is_current: true },
  { id: '/work/atlas', path: '/work/atlas', display_name: 'Atlas', updated_at: '2026-09-19T01:00:00.000Z', session_count: 1, is_current: false },
]

const grouped = (id: string, title: string, updated: string) => ({ id, title, updated_at: updated, model: 'gpt-5.6-sol', message_count: 2 })

/** The client shows one session list; a workspace is a row label, not a divider. */
test('the session list spans every workspace and opens into the session workspace', async ({ page }) => {
  await emit(page, {
    known_workspaces: workspaces,
    workspace_session_groups: [
      { workspace_id: workspaces[0].id, sessions: [grouped('session-a', 'Fix the limiter', '2026-09-20T01:00:00.000Z')] },
      { workspace_id: workspaces[1].id, sessions: [grouped('session-b', 'Atlas refactor', '2026-09-21T01:00:00.000Z')] },
    ],
    saved_sessions: [grouped('session-a', 'Fix the limiter', '2026-09-20T01:00:00.000Z')],
  })
  if (!await page.locator('#sidebar').isVisible()) await page.getByRole('button', { name: 'Toggle sidebar', exact: true }).click()
  const sidebar = page.locator('#sidebar')
  const rows = sidebar.locator('.session-row')
  await expect(rows).toHaveCount(2)
  // Newest first, across workspaces, with the owning workspace as a quiet label.
  await expect(rows.nth(0)).toContainText('Atlas refactor')
  await expect(rows.nth(0).locator('.session-workspace')).toHaveText('Atlas')
  await expect(rows.nth(1)).toContainText('Fix the limiter')
  await expect(rows.nth(1).locator('.session-workspace')).toHaveText('Yeet')
  await expect(sidebar).not.toContainText('WORKSPACES')
  await sidebar.getByRole('searchbox', { name: 'Search sessions' }).fill('atlas')
  await expect(rows).toHaveCount(1)
  await sidebar.getByRole('searchbox', { name: 'Search sessions' }).fill('')
  await rows.nth(0).click()
  // The session is loaded after its workspace handshake, never into the old one.
  await expect.poll(() => page.evaluate(() => (window as any).__yeetSent.filter((message: any) => message.type === 'command').map((message: any) => message.command?.type)))
    .toContain('load_session')
  const loaded = await page.evaluate(() => (window as any).__yeetSent.filter((message: any) => message.type === 'command' && message.command?.type === 'load_session').at(-1)?.command?.session_id)
  expect(loaded).toBe('session-b')
  await expect.poll(() => page.evaluate(() => (window as any).__yeetSent.filter((message: any) => message.type === 'hello').at(-1)?.workspace)).toBe('/work/atlas')
})

/** Touch-first phones get a field they can hit: one control row, 44px targets. */
test('the phone composer keeps touch-sized controls and focuses from the whole surface', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)
  const surface = page.locator('.composer-surface')
  await expect(page.locator('.composer-hint')).toBeHidden()
  await expect(page.locator('.composer-context')).toBeVisible()
  const targets = await surface.evaluate(element => [...element.querySelectorAll('button, select')]
    .filter(node => node.getClientRects().length > 0)
    .map(node => ({ label: node.getAttribute('aria-label') || node.tagName, height: Math.round(node.getBoundingClientRect().height) })))
  expect(targets.length).toBeGreaterThan(2)
  for (const target of targets) expect(target.height, target.label).toBeGreaterThanOrEqual(44)
  const controls = surface.locator('.composer-controls')
  const send = await controls.getByRole('button', { name: 'Send', exact: true }).boundingBox()
  const model = await controls.getByRole('button', { name: 'Choose model' }).boundingBox()
  expect(Math.abs(send!.y - model!.y)).toBeLessThanOrEqual(2)
  expect(send!.x).toBeGreaterThan(model!.x)
  const field = page.getByRole('textbox', { name: 'Message', exact: true })
  expect(Math.round((await field.boundingBox())!.height)).toBeGreaterThanOrEqual(44)
  // Tapping the bubble itself focuses the field instead of doing nothing.
  await surface.click({ position: { x: 8, y: 4 } })
  await expect(field).toBeFocused()
  await surface.getByRole('button', { name: 'Choose model' }).click()
  await expect(field).not.toBeFocused()
})
