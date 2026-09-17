import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
}

async function replaceWithLongToolTranscript(page: Page, count = 72) {
  await page.evaluate((entryCount) => {
    const emit = (window as unknown as TestHooks).__yeetEmit
    const conversation = Array.from({ length: entryCount }, (_, index) => ({
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

    emit({
      type: 'state_update',
      version: 1,
      sequence: 2,
      revision: 2,
      patch: { conversation },
    })
  }, count)

  await expect(page.getByTestId('tool-card')).toHaveCount(count)
  await page.waitForFunction(() => {
    const transcript = document.querySelector<HTMLElement>('[data-testid="transcript"]')
    return !!transcript && transcript.scrollHeight > transcript.clientHeight * 2
  })
}

test('offscreen transcript controls retain readable accessibility semantics', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await replaceWithLongToolTranscript(page)

  const transcript = page.getByTestId('transcript')
  await transcript.evaluate((element) => element.scrollTo({ top: 0, behavior: 'auto' }))
  await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))))

  const lastTool = page.getByTestId('tool-card').last()
  const lastToggle = lastTool.locator('[data-tool-toggle]')
  const state = await lastToggle.evaluate((element) => {
    const html = element as HTMLElement
    return {
      innerText: html.innerText.trim(),
      textContent: html.textContent?.trim() ?? '',
      top: html.getBoundingClientRect().top,
      viewportHeight: window.innerHeight,
    }
  })

  expect(state.top).toBeGreaterThan(state.viewportHeight)
  expect(state.textContent).toContain('offscreen_tool_71')
  expect(state.innerText).toContain('offscreen_tool_71')

  const accessibilityTree = await transcript.ariaSnapshot()
  expect(accessibilityTree).toContain('offscreen_tool_71')
})


test('tool summaries stay identifiable when a tool name is missing', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()

  await page.evaluate(() => {
    const emit = (window as unknown as TestHooks).__yeetEmit
    emit({
      type: 'state_update',
      version: 1,
      sequence: 2,
      revision: 2,
      patch: {
        conversation: [{
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
        }],
      },
    })
  })

  const toggle = page.getByTestId('tool-card').getByRole('button').first()
  await expect(toggle).toContainText('Tool call')
  await expect(toggle).toHaveAccessibleName(/Tool call.*unnamed-tool-output.*Done/i)
})


test('code copy controls are distinguishable by fenced language', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()

  await page.evaluate(() => {
    const emit = (window as unknown as TestHooks).__yeetEmit
    emit({
      type: 'state_update',
      version: 1,
      sequence: 2,
      revision: 2,
      patch: {
        conversation: [{
          id: 'multi-code-assistant',
          kind: {
            type: 'assistant',
            content: '```typescript\nconst answer = 42\n```\n\n```bash\necho ready\n```',
            toolCalls: [],
          },
        }],
      },
    })
  })

  await expect(page.getByRole('button', { name: 'Copy typescript code' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Copy bash code' })).toBeVisible()
})


