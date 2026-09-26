import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
}

async function setConversation(page: Page, conversation: Array<Record<string, unknown>>, sequence = 2) {
  await page.evaluate(({ conversation, sequence }) => {
    ;(window as unknown as TestHooks).__yeetEmit({
      type: 'state_update',
      version: 1,
      sequence,
      revision: sequence,
      patch: { conversation },
    })
  }, { conversation, sequence })
}

async function replaceWithLongToolTranscript(page: Page, count = 72) {
  const conversation = Array.from({ length: count }, (_, index) => ({
    id: `offscreen-tool-entry-${index}`,
    kind: {
      type: 'toolCall',
      toolCall: {
        id: `offscreen-tool-${index}`,
        name: `offscreen_tool_${index}`,
        arguments: JSON.stringify({ path: `/tmp/transcript-item-${index}` }),
        status: 'completed',
        result: null,
      },
    },
  }))
  await setConversation(page, conversation)

  const group = page.locator('.activity-group').last()
  const header = group.locator('.activity-group__header')
  await expect(header).toHaveAttribute('aria-expanded', 'false')
  await header.click()
  await expect(header).toHaveAttribute('aria-expanded', 'true')
  await expect(group.locator('.trace-disclosure__row')).toHaveCount(count)
  return group
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('offscreen activity controls retain readable accessibility semantics', async ({ page }) => {
  const group = await replaceWithLongToolTranscript(page)
  const transcript = page.getByTestId('transcript')
  await transcript.evaluate((element) => element.scrollTo({ top: 0, behavior: 'auto' }))
  await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))))

  const lastToggle = group.locator('.trace-disclosure__row').last()
  const state = await lastToggle.evaluate((element) => {
    const row = element as HTMLElement
    const transcript = row.closest<HTMLElement>('[data-testid="transcript"]')
    const rowRect = row.getBoundingClientRect()
    const transcriptRect = transcript?.getBoundingClientRect()
    return {
      innerText: row.innerText.trim(),
      textContent: row.textContent?.trim() ?? '',
      rowTop: rowRect.top,
      transcriptBottom: transcriptRect?.bottom ?? 0,
    }
  })

  expect(state.rowTop).toBeGreaterThan(state.transcriptBottom)
  expect(state.textContent.toLowerCase()).toContain('offscreen tool 71')
  expect(state.innerText).toContain('/tmp/transcript-item-71')
  await expect(lastToggle).toHaveAccessibleName(/offscreen tool 71.*transcript-item-71/i)

  const accessibilityTree = await transcript.ariaSnapshot()
  expect(accessibilityTree.toLowerCase()).toContain('offscreen tool 71')
  expect(accessibilityTree).toContain('/tmp/transcript-item-71')
})

test('tool traces stay identifiable when a tool name is missing', async ({ page }) => {
  await setConversation(page, [{
    id: 'unnamed-tool-entry',
    kind: {
      type: 'toolCall',
      toolCall: {
        id: 'unnamed-tool',
        arguments: JSON.stringify({ path: '/tmp/unnamed-tool-output' }),
        status: 'completed',
        result: null,
      },
    },
  }])

  const group = page.locator('.activity-group').last()
  await group.locator('.activity-group__header').click()
  const toggle = group.locator('.trace-disclosure__row').first()
  await expect(toggle).toContainText('Tool call')
  await expect(toggle).toContainText('unnamed-tool-output')
  await expect(toggle).toHaveAccessibleName(/Tool call.*unnamed-tool-output/i)
})

test('fenced code keeps language semantics while message actions remain accessible', async ({ page }) => {
  await setConversation(page, [
    { id: 'code-user', kind: { type: 'user', content: 'Show both examples.' } },
    {
      id: 'multi-code-assistant',
      kind: {
        type: 'assistant',
        content: '\u0060\u0060\u0060typescript\nconst answer = 42\n\u0060\u0060\u0060\n\n\u0060\u0060\u0060bash\necho ready\n\u0060\u0060\u0060',
        toolCalls: [],
      },
    },
  ])

  await expect(page.locator('pre code.language-typescript')).toContainText('const answer = 42')
  await expect(page.locator('pre code.language-bash')).toContainText('echo ready')
  await expect(page.getByRole('button', { name: 'Copy message' })).toHaveCount(2)
  await expect(page.getByRole('button', { name: 'Edit message' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Regenerate response' })).toBeVisible()
})
