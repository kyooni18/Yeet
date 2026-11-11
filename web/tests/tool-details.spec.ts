import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

function tool(id: string, name: string, args: unknown, result?: unknown, status = 'completed') {
  return { id, kind: { type: 'toolCall', toolCall: {
    id, name, arguments: JSON.stringify(args), status,
    ...(result === undefined ? {} : { rawResult: JSON.stringify(result), result: 'Captured tool result' }),
  } } }
}

async function reset(page: Page, entries: unknown[]) {
  await page.evaluate(conversation => {
    (window as any).__yeetEmit({ type: 'conversation_reset', version: 1, sequence: 2, revision: 2, conversation })
  }, entries)
}

async function noOverflow(page: Page) {
  const overflow = await page.evaluate(() => {
    const main = document.querySelector('#main')!.getBoundingClientRect()
    return {
      document: document.documentElement.scrollWidth > innerWidth,
      embeds: [...document.querySelectorAll('.tool-embed, .activity-group__header, .composer-surface')].some(node => {
        const box = node.getBoundingClientRect()
        return box.left < main.left - 1 || box.right > main.right + 1
      }),
    }
  })
  expect(overflow).toEqual({ document: false, embeds: false })
}

test.beforeEach(async ({ page }) => {
  await page.route('https://static.cloudflareinsights.com/**', route => route.fulfill({ body: '' }))
  await installMockRemote(page)
  await page.emulateMedia({ reducedMotion: 'reduce' })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('shell embed separates command, streams and exit state; raw inspection is keyboard operable', async ({ page }, info) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.emulateMedia({ colorScheme: 'dark' })
  await reset(page, [tool('shell', 'run_shell', { command: 'cargo test --workspace', workingDirectory: '/work/Yeet' }, {
    workingDirectory: '/work/Yeet', exitCode: 0, stdout: 'tests passed\nliteral **text** <script>not executable</script>\n', stderr: 'warning: unused import\n',
  })])
  const header = page.locator('.activity-group__header')
  await expect(header).toContainText('cargo test --workspace')
  await expect(header.locator('.trace-disclosure__chevron')).toBeVisible()
  await header.focus()
  await page.keyboard.press('Enter')
  await expect(header).toHaveAttribute('aria-expanded', 'true')
  await expect(header).toBeFocused()
  const terminal = page.locator('.tool-terminal')
  await expect(terminal).toContainText('/work/Yeet')
  await expect(terminal).toContainText('Finished · exit 0')
  await expect(terminal.locator('.tool-terminal__command')).toHaveText('cargo test --workspace')
  await expect(terminal.getByLabel('stdout', { exact: true })).toContainText('literal **text** <script>not executable</script>')
  await expect(terminal.getByLabel('stderr', { exact: true })).toContainText('warning: unused import')
  const raw = page.getByRole('button', { name: 'Raw input and output' })
  await expect(page.locator('.tool-raw')).toHaveCount(0)
  await raw.focus()
  await page.keyboard.press('Space')
  await expect(raw).toHaveAttribute('aria-expanded', 'true')
  await expect(raw).toBeFocused()
  await expect(page.locator('.tool-raw')).toContainText('cargo test --workspace')
  await expect(page.locator('.tool-raw')).toContainText('Captured tool result')
  await noOverflow(page)
  const commands = await page.evaluate(() => (window as any).__yeetSent.filter((item: any) =>
    item.type === 'command' && ['run_shell', 'shell_job'].includes(item.command?.type)))
  expect(commands).toEqual([]) // Expanding a historical record never executes it.
  await page.screenshot({ path: info.outputPath('shell-details-dark.png') })
  expect(errors).toEqual([])
})

test('edits show captured unified hunks, line numbers, rename and diagnostics without raw JSON', async ({ page }, info) => {
  await page.emulateMedia({ colorScheme: 'light' })
  await reset(page, [tool('edit', 'apply_file_edits', { changes: [{ path: 'src/layout.ts' }, { path: 'old.ts' }] }, {
    files: [
      { path: 'src/layout.ts', operation: 'update', diff: { hunks: [{ oldStart: 10, oldEnd: 12, newStart: 10, newEnd: 12,
        lines: [' context', '-const gap = 3', '+const gap = 8', ' end'], truncated: false }] } },
      { path: 'old.ts', destination: 'new.ts', operation: 'rename', diff: { hunks: [] } },
    ], diagnostics: [{ severity: 'warning', path: 'src/layout.ts', line: 11, column: 1, message: 'Review spacing at this breakpoint.' }],
  })])
  await expect(page.locator('.activity-group__header')).toContainText('Edit files')
  await page.locator('.activity-group__header').click()
  const diff = page.locator('.tool-diff').first()
  await expect(diff).toContainText('src/layout.ts')
  await expect(diff).toContainText('+1 −1')
  const added = diff.locator('.tool-diff__line--added')
  const removed = diff.locator('.tool-diff__line--removed')
  await expect(added).toContainText('+const gap = 8')
  await expect(removed).toContainText('−const gap = 3')
  await expect(added.locator('.tool-line-number').nth(1)).toHaveText('11')
  await expect(removed.locator('.tool-line-number').nth(0)).toHaveText('11')
  expect(await added.evaluate(el => getComputedStyle(el).backgroundColor)).not.toBe(await removed.evaluate(el => getComputedStyle(el).backgroundColor))
  await expect(page.locator('.tool-diff').nth(1)).toContainText('old.ts → new.ts')
  await expect(page.locator('.tool-diagnostic')).toContainText('warning · src/layout.ts:11:1')
  await expect(page.locator('.tool-diagnostic')).toContainText('Review spacing')
  await expect(page.locator('.tool-raw')).toHaveCount(0)
  await noOverflow(page)
  await page.screenshot({ path: info.outputPath('edit-details-light.png') })
})

test('source reads retain real numbering, escaped code and partial/missing states', async ({ page }, info) => {
  await page.emulateMedia({ colorScheme: 'dark' })
  await reset(page, [tool('read', 'read_file', { requests: [{ path: 'src/日本語.ts' }, { path: 'missing.ts' }, { path: 'large.ts' }] }, [
    { path: 'src/日本語.ts', startLine: 10, endLine: 12, totalLines: 90,
      lines: '10:ab12|const label = "<img onerror=alert(1)>"\n11:cd34|\n12:ef56|\t' + 'veryLongIdentifier'.repeat(40), nextStartLine: 13 },
    { path: 'missing.ts', error: 'File not found' },
    { path: 'large.ts', externalized: true, artifactId: 'source-artifact', startLine: 1, endLine: 900, totalLines: 900,
      preview: '1:aa|first line\n2:bb|second line\n3:cc|incomplete' },
  ])])
  await expect(page.locator('.activity-group__header')).toContainText('src/日本語.ts')
  await page.locator('.activity-group__header').click()
  const source = page.locator('.tool-source').first()
  await expect(source).toContainText('Lines 10–12 · 90 total')
  await expect(source.locator('.tool-line-number')).toHaveText(['10', '11', '12'])
  await expect(source.locator('.tool-code').first()).toHaveText('const label = "<img onerror=alert(1)>"')
  await expect(source.locator('img, script')).toHaveCount(0)
  await expect(source).not.toContainText('ab12')
  await expect(page.locator('.tool-source').nth(1)).toContainText('File not found')
  await expect(page.locator('.tool-source').nth(2)).toContainText('Partial source preview')
  await expect(page.locator('.tool-source').nth(2)).toContainText('source-artifact')
  await expect(page.locator('.tool-source').nth(2).locator('.tool-source__line')).toHaveCount(2)
  const scroll = source.locator('.tool-code-scroll')
  expect(await scroll.evaluate(el => el.scrollWidth > el.clientWidth)).toBe(true)
  await scroll.focus()
  await page.keyboard.press('ArrowRight')
  await expect.poll(() => scroll.evaluate(el => el.scrollLeft)).toBeGreaterThan(0)
  await noOverflow(page)
  await page.screenshot({ path: info.outputPath('source-details-dark.png') })
})

test('grouped tools preserve partial-output notices and readable generic errors', async ({ page }) => {
  await reset(page, [
    tool('live', 'run_shell', { command: 'cargo check' }, { status: 'running', elapsedMilliseconds: 2100,
      progress: { stdoutBytes: 90000, stdoutTail: 'checking sources…\n', stderrBytes: 0, stderrTail: null } }, 'running'),
    tool('external', 'apply_file_edits', { changes: [{ path: 'big.rs' }] }, { externalized: true, artifactId: 'diff-artifact', bytes: 90000 }),
    { id: 'search', kind: { type: 'toolCall', toolCall: { id: 'search', name: 'search_workspace', arguments: '{"query":"layout"}', status: 'failed', error: 'Workspace is unavailable. Reconnect and retry.' } } },
  ])
  const header = page.locator('.activity-group__header')
  await expect(header).toContainText('3 steps')
  await expect(header).toContainText('Failed')
  await header.click()
  const rows = page.locator('.activity-group__event .trace-disclosure__row')
  await expect(rows).toHaveCount(3)
  for (const row of await rows.all()) await row.click()
  await expect(page.locator('.tool-terminal')).toContainText('Latest output · 90,000 bytes total')
  await expect(page.locator('.tool-terminal')).toContainText('checking sources…')
  await expect(page.locator('.tool-terminal')).not.toContainText('Finished')
  await expect(page.locator('.tool-diff')).toContainText('No diff recorded')
  await expect(page.locator('.tool-details')).toContainText(['checking sources…', 'Diff stored externally', 'Workspace is unavailable'])
  await expect(page.locator('.trace-detail.is-error')).toContainText('Reconnect and retry')
  await expect(page.getByRole('button', { name: 'Raw input and output' })).toHaveCount(2)
  await noOverflow(page)
})
