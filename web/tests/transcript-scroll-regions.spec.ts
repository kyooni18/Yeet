import { expect, test, type Locator, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
}

async function setConversation(page: Page, conversation: Array<Record<string, unknown>>) {
  await page.evaluate((value) => {
    ;(window as unknown as TestHooks).__yeetEmit({
      type: 'state_update',
      version: 1,
      sequence: 2,
      revision: 2,
      patch: { conversation: value },
    })
  }, conversation)
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

  await setConversation(page, [
    {
      id: 'overflow-reasoning',
      kind: { type: 'reasoning', content: reasoningContent, summary: 'Long reasoning' },
    },
    {
      id: 'overflow-skill',
      kind: { type: 'skill', name: 'overflow-skill', status: 'loaded', content: skillContent },
    },
    {
      id: 'overflow-mcp',
      kind: { type: 'mcp', server: 'Audit', name: 'long_output', content: mcpContent, isError: false },
    },
    {
      id: 'overflow-tool-entry',
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
    },
  ])

  const group = page.locator('.activity-group').last()
  const header = group.locator('.activity-group__header')
  await expect(header).toHaveAttribute('aria-expanded', 'false')
  await header.click()
  await expect(header).toHaveAttribute('aria-expanded', 'true')
  return group
}

async function expectScrollableRegion(page: Page, region: Locator, label: string) {
  await region.scrollIntoViewIfNeeded()
  await expect(region).toBeVisible()
  await expect(region).toHaveAttribute('tabindex', '0')
  await expect(region).toHaveAttribute('role', 'region')
  await expect(region).toHaveAttribute('aria-label', label)

  const dimensions = await region.evaluate((element) => ({
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
    clientWidth: element.clientWidth,
    scrollWidth: element.scrollWidth,
  }))
  expect(
    dimensions.scrollHeight > dimensions.clientHeight || dimensions.scrollWidth > dimensions.clientWidth,
    `${label} should overflow in this regression fixture`,
  ).toBe(true)

  const before = await region.evaluate((element) => element.scrollTop)
  await page.keyboard.press('PageDown')
  await expect.poll(() => region.evaluate((element) => element.scrollTop)).toBeGreaterThan(before)

  const accessibilityTree = await region.ariaSnapshot()
  expect(accessibilityTree).toContain(label)
}

async function openTrace(group: Locator, text: string) {
  const trace = group.locator('.trace-disclosure').filter({ hasText: text }).first()
  const row = trace.locator('.trace-disclosure__row')
  await expect(row).toHaveAttribute('aria-expanded', 'false')
  await row.click()
  await expect(row).toHaveAttribute('aria-expanded', 'true')
  return { trace, row }
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('expanded reasoning, skill, and MCP details support keyboard scrolling', async ({ page }) => {
  const group = await addOverflowingEntries(page)
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)

  for (const fixture of [
    { text: 'Reasoning', label: 'Reasoning details' },
    { text: 'overflow-skill', label: 'Skill details' },
    { text: 'Audit · long_output', label: 'MCP details' },
  ]) {
    const { trace, row } = await openTrace(group, fixture.text)
    const region = trace.getByRole('region', { name: fixture.label })
    await row.focus()
    await page.keyboard.press('Tab')
    await expect(region).toBeFocused()
    await expectScrollableRegion(page, region, fixture.label)
    if (!touchFirst) {
      await page.keyboard.press('Shift+Tab')
      await expect(row).toBeFocused()
    }
  }
})

test('expanded tool input and result panes support keyboard scrolling', async ({ page }) => {
  const group = await addOverflowingEntries(page)
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  const { trace, row } = await openTrace(group, 'overflow tool')
  const input = trace.getByRole('region', { name: 'Input details' })
  const result = trace.getByRole('region', { name: 'Result details' })

  await row.focus()
  await page.keyboard.press('Tab')
  await expect(input).toBeFocused()
  await expectScrollableRegion(page, input, 'Input details')

  await page.keyboard.press('Tab')
  await expect(result).toBeFocused()
  await expectScrollableRegion(page, result, 'Result details')

  if (!touchFirst) {
    await page.keyboard.press('Shift+Tab')
    await expect(input).toBeFocused()
    await page.keyboard.press('Shift+Tab')
    await expect(row).toBeFocused()
  }
})
