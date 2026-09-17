import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __legacyCopies: string[]
  __yeetEmit: (message: Record<string, unknown>) => void
}

async function openWithRestrictedClipboard(page: Page, legacyCopySucceeds: boolean) {
  await page.addInitScript((copySucceeds) => {
    const copies: string[] = []
    Object.assign(window, { __legacyCopies: copies })
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: async () => {
          throw new DOMException('Clipboard permission denied', 'NotAllowedError')
        },
      },
    })
    Object.defineProperty(document, 'execCommand', {
      configurable: true,
      value: (command: string) => {
        if (command !== 'copy') return false
        const active = document.activeElement
        if (active instanceof HTMLTextAreaElement) {
          const start = active.selectionStart ?? 0
          const end = active.selectionEnd ?? active.value.length
          copies.push(active.value.slice(start, end))
        }
        return copySucceeds
      },
    })
  }, legacyCopySucceeds)
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
}

async function legacyCopies(page: Page) {
  return page.evaluate(() => (window as unknown as TestHooks).__legacyCopies)
}

test('code and tool copy fall back when Clipboard API permission is denied without losing focus', async ({ page }) => {
  await openWithRestrictedClipboard(page, true)

  const codeCopy = page.locator('.code-block [data-copy-code]').first()
  await codeCopy.focus()
  await page.keyboard.press('Enter')
  await expect(codeCopy).toHaveText('Copied')
  await expect(codeCopy).toBeFocused()
  await expect.poll(() => legacyCopies(page)).toEqual(expect.arrayContaining([expect.stringContaining('const transport')]))

  await expect(codeCopy).toHaveAttribute('aria-live', 'polite')
  await page.keyboard.press('Enter')
  await expect(codeCopy).toHaveText('Copied')
  await page.waitForTimeout(1600)
  await expect(codeCopy).toHaveText('Copy')

  const tool = page.getByTestId('tool-card').filter({ hasText: 'read_file' })
  await tool.locator('[data-tool-toggle]').click()
  const resultCopy = tool.getByRole('button', { name: 'Copy tool result' })
  await resultCopy.focus()
  await page.keyboard.press('Enter')
  await expect(resultCopy).toHaveText('Copied')
  await expect(resultCopy).toBeFocused()
  await expect(tool.getByRole('status')).toHaveText('Tool result copied')
  await expect.poll(() => legacyCopies(page)).toEqual(expect.arrayContaining(['App.vue loaded successfully']))

  const argumentsCopy = tool.getByRole('button', { name: 'Copy tool arguments' })
  await argumentsCopy.focus()
  await page.keyboard.press('Enter')
  await expect(argumentsCopy).toHaveText('Copied')
  await expect(argumentsCopy).toBeFocused()
  await expect(tool.getByRole('status')).toHaveText('Tool arguments copied')
  await expect.poll(() => legacyCopies(page)).toEqual(expect.arrayContaining([expect.stringContaining('web/src/App.vue')]))


  const mcp = page.locator('.mcp-card').filter({ hasText: 'MCP · Yeet-KY' })
  await mcp.locator('summary').click()
  const mcpCopy = mcp.getByRole('button', { name: 'Copy MCP output' })
  await mcpCopy.focus()
  await page.keyboard.press('Enter')
  await expect(mcpCopy).toHaveText('Copied')
  await expect(mcpCopy).toBeFocused()
  await expect(mcp.getByRole('status')).toHaveText('MCP output copied')
  await expect.poll(() => legacyCopies(page)).toEqual(expect.arrayContaining(['web/src']))
})

test('copy controls report a visible and assistive failure when every copy path is unavailable', async ({ page }) => {
  await openWithRestrictedClipboard(page, false)

  const codeCopy = page.locator('.code-block [data-copy-code]').first()
  await codeCopy.focus()
  await page.keyboard.press('Enter')
  await expect(codeCopy).toHaveText('Copy failed')
  await expect(codeCopy).toBeFocused()

  const tool = page.getByTestId('tool-card').filter({ hasText: 'read_file' })
  await tool.locator('[data-tool-toggle]').click()
  const resultCopy = tool.getByRole('button', { name: 'Copy tool result' })
  await resultCopy.focus()
  await page.keyboard.press('Enter')
  await expect(resultCopy).toHaveText('Copy failed')
  await expect(resultCopy).toBeFocused()
  await expect(tool.getByRole('status')).toHaveText('Tool result could not be copied')


  const mcp = page.locator('.mcp-card').filter({ hasText: 'MCP · Yeet-KY' })
  await mcp.locator('summary').click()
  const mcpCopy = mcp.getByRole('button', { name: 'Copy MCP output' })
  await mcpCopy.focus()
  await page.keyboard.press('Enter')
  await expect(mcpCopy).toHaveText('Copy failed')
  await expect(mcpCopy).toBeFocused()
  await expect(mcp.getByRole('status')).toHaveText('MCP output could not be copied')
})


test('truncated MCP preview copies the complete output rather than only the visible prefix', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'laptop')
  await openWithRestrictedClipboard(page, true)
  const fullOutput = `begin-mcp-output\n${'payload '.repeat(2400)}\nend-mcp-output-sentinel`

  await page.evaluate((content) => {
    const emit = (window as unknown as TestHooks).__yeetEmit
    emit({
      type: 'conversation_entry',
      version: 1,
      sequence: 2,
      revision: 2,
      entry: {
        id: 'mcp-copy-full-output',
        kind: { type: 'mcp', server: 'Bulk-MCP', name: 'read_large_output', content, isError: false },
      },
    })
  }, fullOutput)

  const mcp = page.locator('.mcp-card').filter({ hasText: 'MCP · Bulk-MCP' })
  await mcp.locator('summary').click()
  await expect(mcp.locator('.semantic-output')).not.toContainText('end-mcp-output-sentinel')

  const mcpCopy = mcp.getByRole('button', { name: 'Copy MCP output' })
  await expect(mcpCopy).toHaveText('Copy full output')
  await mcpCopy.focus()
  await page.keyboard.press('Enter')
  await expect(mcpCopy).toHaveText('Copied')
  await expect(mcpCopy).toBeFocused()
  await expect.poll(async () => (await legacyCopies(page)).includes(fullOutput)).toBe(true)


  const expandOutput = mcp.locator('button.semantic-output-toggle[aria-expanded]')
  await expect(expandOutput).toHaveAttribute('aria-expanded', 'false')
  await expandOutput.click()
  await expect(expandOutput).toHaveAttribute('aria-expanded', 'true')
  await expect(expandOutput).toHaveText('Collapse output')
  await expect(mcp.locator('.semantic-output')).toContainText('end-mcp-output-sentinel')
  await expandOutput.click()
  await expect(expandOutput).toHaveAttribute('aria-expanded', 'false')


  const fullToolOutput = `begin-tool-output\n${'tool payload '.repeat(1400)}\nend-tool-output-sentinel`
  await page.evaluate((content) => {
    const emit = (window as unknown as TestHooks).__yeetEmit
    emit({
      type: 'conversation_entry',
      version: 1,
      sequence: 3,
      revision: 3,
      entry: {
        id: 'tool-output-disclosure',
        kind: {
          type: 'toolCall',
          toolCall: { id: 'tool-output-disclosure', name: 'large_result_tool', arguments: '{}', status: 'completed', result: content },
        },
      },
    })
  }, fullToolOutput)

  const tool = page.getByTestId('tool-card').filter({ hasText: 'large_result_tool' })
  await tool.locator('[data-tool-toggle]').click()
  await expect(tool.locator('pre').last()).not.toContainText('end-tool-output-sentinel')
  const expandToolOutput = tool.locator('button.tool-output-toggle[aria-expanded]')
  await expect(expandToolOutput).toHaveAttribute('aria-expanded', 'false')
  await expandToolOutput.click()
  await expect(expandToolOutput).toHaveAttribute('aria-expanded', 'true')
  await expect(tool.locator('pre').last()).toContainText('end-tool-output-sentinel')
})
