import { expect, test, type Locator, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
}

async function emitEntry(page: Page, sequence: number, entry: Record<string, unknown>) {
  await page.evaluate(({ sequence, entry }) => {
    ;(window as unknown as TestHooks).__yeetEmit({
      type: 'conversation_entry',
      version: 1,
      sequence,
      revision: sequence,
      entry,
    })
  }, { sequence, entry })
}

async function addOverflowingEntries(page: Page) {
  const reasoningContent = Array.from({ length: 180 }, (_, index) => `Reasoning line ${index}: ${'detail '.repeat(9)}`).join('\n\n')
  const skillContent = Array.from({ length: 180 }, (_, index) => `Skill line ${index}: ${'output '.repeat(9)}`).join('\n\n')
  const mcpContent = Array.from({ length: 220 }, (_, index) => `MCP line ${index}: ${'payload '.repeat(12)}`).join('\n')
  const toolArguments = JSON.stringify({
    rows: Array.from({ length: 220 }, (_, index) => ({
      index,
      value: `argument-${index}-${'x'.repeat(40)}`,
    })),
  })
  const toolResult = Array.from({ length: 220 }, (_, index) => `Tool line ${index}: ${'result '.repeat(12)}`).join('\n')

  await emitEntry(page, 2, {
    id: 'overflow-reasoning',
    kind: { type: 'reasoning', content: reasoningContent, summary: 'Long reasoning' },
  })
  await emitEntry(page, 3, {
    id: 'overflow-skill',
    kind: { type: 'skill', name: 'overflow-skill', status: 'loaded', content: skillContent },
  })
  await emitEntry(page, 4, {
    id: 'overflow-mcp',
    kind: { type: 'mcp', server: 'Audit', name: 'long_output', content: mcpContent, isError: false },
  })
  await emitEntry(page, 5, {
    id: 'overflow-tool',
    kind: {
      type: 'toolCall',
      toolCall: {
        id: 'overflow-tool',
        name: 'overflow_tool',
        arguments: toolArguments,
        status: 'completed',
        result: toolResult,
      },
    },
  })
}

async function expectKeyboardScrollable(page: Page, viewport: Locator, label: string, previousControl: Locator) {
  await viewport.scrollIntoViewIfNeeded()
  await expect(viewport).toBeVisible()
  await expect(viewport).toHaveAttribute('tabindex', '0')
  await expect(viewport).toHaveAttribute('role', 'group')
  await expect(viewport).toHaveAttribute('aria-label', label)

  const dimensions = await viewport.evaluate((element) => ({
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
    clientWidth: element.clientWidth,
    scrollWidth: element.scrollWidth,
  }))
  expect(
    dimensions.scrollHeight > dimensions.clientHeight || dimensions.scrollWidth > dimensions.clientWidth,
    `${label} should overflow in this regression fixture`,
  ).toBe(true)

  await previousControl.focus()
  await page.keyboard.press('Tab')
  await expect(viewport).toBeFocused()

  const before = await viewport.evaluate((element) => element.scrollTop)
  await page.keyboard.press('PageDown')
  await expect.poll(() => viewport.evaluate((element) => element.scrollTop)).toBeGreaterThan(before)

  const accessibilityTree = await viewport.ariaSnapshot()
  expect(accessibilityTree).toContain(label)


  await page.keyboard.press('Shift+Tab')
  await expect(previousControl).toBeFocused()
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await addOverflowingEntries(page)
})

test('expanded reasoning, skill, and MCP output panes support keyboard scrolling', async ({ page }) => {
  const reasoning = page.locator('[data-entry-id="overflow-reasoning"]')
  const reasoningSummary = reasoning.locator('summary')
  await reasoningSummary.click()
  await expectKeyboardScrollable(page, reasoning.locator('.reasoning-body'), 'Reasoning details', reasoningSummary)

  const skill = page.locator('[data-entry-id="overflow-skill"]')
  const skillSummary = skill.locator('summary')
  await skillSummary.click()
  await expectKeyboardScrollable(page, skill.locator('.semantic-card-body'), 'Skill overflow-skill output', skillSummary)

  const mcp = page.locator('[data-entry-id="overflow-mcp"]')
  const mcpSummary = mcp.locator('summary')
  await mcpSummary.click()
  await expectKeyboardScrollable(page, mcp.locator('.semantic-output'), 'MCP output', mcpSummary)
})

test('expanded tool argument and result panes support keyboard scrolling', async ({ page }) => {
  const tool = page.locator('[data-entry-id="overflow-tool"]')
  await tool.locator('[data-tool-toggle]').click()
  const panes = tool.locator('.tool-card-details pre')
  await expect(panes).toHaveCount(2)

  const argumentCopy = tool.getByRole('button', { name: 'Copy tool arguments' })
  await expectKeyboardScrollable(page, panes.nth(0), 'Tool arguments', argumentCopy)

  const resultCopy = tool.getByRole('button', { name: 'Copy tool result' })
  await expectKeyboardScrollable(page, panes.nth(1), 'Tool result', resultCopy)
})
