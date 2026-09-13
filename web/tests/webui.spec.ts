import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetSent: Array<Record<string, unknown>>
  __yeetEmit: (message: Record<string, unknown>) => void
  __yeetDisconnect: () => void
  __yeetRestart: () => void
}

async function sentCommands(page: Page) {
  return page.evaluate(() => {
    const sent = (window as unknown as TestHooks).__yeetSent
    return sent.filter((item) => item.type === 'command').map((item) => item.command as Record<string, unknown>)
  })
}

async function emit(page: Page, message: Record<string, unknown>) {
  await page.evaluate((payload) => {
    ;(window as unknown as TestHooks).__yeetEmit(payload)
  }, message)
}

async function activateDisclosure(page: Page, summary: ReturnType<Page['locator']>) {
  await summary.scrollIntoViewIfNeeded()
  await expect(summary).toBeVisible()
  if (await page.evaluate(() => navigator.maxTouchPoints > 0)) await summary.tap()
  else await summary.click()
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('renders semantic transcript and lazily expands tool details', async ({ page }) => {
  await expect(page.locator('.markdown-document h2', { hasText: 'Interface ready' })).toBeVisible()
  await expect(page.locator('.code-block')).toContainText('const transport')
  await expect(page.getByText('Reasoning trace')).toBeVisible()
  await expect(page.getByText('Inspecting workspace')).toBeVisible()
  await expect(page.getByText('Skill · frontend-review')).toBeVisible()
  await expect(page.getByText('MCP · Yeet-KY')).toBeVisible()

  const reasoning = page.locator('.reasoning-card').first()
  await expect(reasoning.locator('.reasoning-body')).not.toBeAttached()
  await activateDisclosure(page, reasoning.locator('summary'))
  await expect(reasoning.locator('.reasoning-body')).toContainText('semantic state')

  const skill = page.locator('.skill-card').first()
  await expect(skill.locator('.semantic-card-body')).not.toBeAttached()
  await activateDisclosure(page, skill.locator('summary'))
  await expect(skill.locator('.semantic-card-body')).toContainText('Responsive checks enabled')

  const mcp = page.locator('.mcp-card').first()
  await expect(mcp.locator('.semantic-output')).not.toBeAttached()
  await activateDisclosure(page, mcp.locator('summary'))
  await expect(mcp.locator('.semantic-output')).toContainText('web/src')

  const tool = page.getByTestId('tool-card').filter({ hasText: 'read_file' })
  await expect(tool.getByTestId('tool-details')).not.toBeAttached()
  const toolToggle = tool.locator('[data-tool-toggle]')
  await activateDisclosure(page, toolToggle)
  await expect(toolToggle).toHaveAttribute('aria-expanded', 'true')
  await expect(tool.getByTestId('tool-details')).toContainText('App.vue loaded successfully')
  if ((page.viewportSize()?.width ?? 1000) > 480) await expect(tool.getByText('82 ms')).toBeVisible()
  else await expect(tool.getByText('82 ms')).toBeHidden()
})


test('renders suppressed tool calls as neutral non-success terminal states', async ({ page }) => {
  await emit(page, {
    type: 'tool_update', version: 1, sequence: 2, revision: 2,
    entry: {
      id: 'tool-suppressed',
      kind: {
        type: 'toolCall',
        toolCall: {
          id: 'tool-suppressed',
          name: 'apply_file_edits',
          arguments: '{"changes":[]}',
          status: 'suppressed',
        },
      },
    },
    tool_call: null,
  })

  const tool = page.getByTestId('tool-card').filter({ hasText: 'apply_file_edits' })
  await expect(tool).toHaveClass(/tool-suppressed/)
  await expect(tool.locator('[data-tool-toggle]')).toContainText('Suppressed')
  await expect(tool.locator('[data-tool-toggle]')).not.toContainText('Done')
  await expect(tool.locator('.tool-status-icon')).toContainText('⊘')
  await expect(tool.locator('.tool-duration')).not.toHaveClass(/is-error/)
})

test('keeps activity chrome compact and sanitizes reasoning and tool previews', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await emit(page, {
    type: 'conversation_entry', version: 1, sequence: 2, revision: 2,
    entry: {
      id: 'reasoning-markdown',
      kind: { type: 'reasoning', content: 'Planning details', summary: '**Planning safe rebase** ***Inspecting conflict***' },
    },
  })
  await emit(page, {
    type: 'tool_update', version: 1, sequence: 3, revision: 3,
    entry: {
      id: 'tool-malformed',
      kind: {
        type: 'toolCall',
        toolCall: { id: 'tool-malformed', name: 'run_shell', arguments: '{"command":"git push', status: 'failed', error: 'push rejected' },
      },
    },
    tool_call: null,
  })

  const reasoning = page.locator('.reasoning-card').last()
  await expect(reasoning.locator('summary')).toContainText('Planning safe rebase Inspecting conflict')
  await expect(reasoning.locator('summary')).not.toContainText('**')

  const failedTool = page.getByTestId('tool-card').filter({ hasText: 'run_shell' })
  await expect(failedTool.locator('[data-tool-toggle]')).toContainText('git push')
  await expect(failedTool.locator('[data-tool-toggle]')).not.toContainText('{"command"')
  await expect(failedTool.locator('[data-tool-toggle]')).toContainText('Failed')

  const composerBox = await page.getByTestId('composer').boundingBox()
  expect(composerBox).not.toBeNull()
  expect(composerBox?.height ?? 999).toBeLessThanOrEqual(90)
  await expect(page.getByTestId('open-status')).not.toBeVisible()
  await expect(page.locator('.top-bar')).not.toContainText('Yeet')
})

test('submits semantic commands and exposes interrupt while streaming', async ({ page }) => {
  const composer = page.getByLabel('Message Yeet')
  await composer.fill('Run responsive checks')
  await page.getByTestId('submit').click()

  await expect.poll(async () => page.evaluate(() => {
    const sent = (window as unknown as { __yeetSent: Array<Record<string, unknown>> }).__yeetSent
    return sent.some((item) => item.type === 'command' && (item.command as Record<string, unknown>)?.type === 'submit')
  })).toBe(true)

  await page.evaluate(() => {
    const emit = (window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit
    emit({ type: 'assistant_delta', version: 1, sequence: 2, revision: 2, entry_id: 'a1', delta: '', content: 'Streaming response from Yeet…', reset: true })
    emit({ type: 'assistant_delta', version: 1, sequence: 3, revision: 3, entry_id: 'a1', delta: ' done', content: '', reset: false })
    emit({ type: 'state_update', version: 1, sequence: 4, revision: 3, patch: { is_streaming: true } })
  })

  await expect(page.getByText('Streaming response from Yeet… done')).toBeVisible()
  await expect(page.getByTestId('interrupt')).toBeVisible()
  await page.getByTestId('interrupt').click()
  await expect.poll(async () => page.evaluate(() => {
    const sent = (window as unknown as { __yeetSent: Array<Record<string, unknown>> }).__yeetSent
    return sent.some((item) => item.type === 'command' && (item.command as Record<string, unknown>)?.type === 'interrupt')
  })).toBe(true)
})

test('live reconnect resumes from the last applied semantic cursor', async ({ page }) => {
  await page.evaluate(() => {
    const emit = (window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit
    emit({ type: 'state_update', version: 1, sequence: 2, revision: 2, patch: { current_context_tokens: 43000 } })
  })
  await expect(page.locator('.desktop-status').first()).toContainText('connected')
  await page.waitForTimeout(50)
  const assignedClientId = await page.evaluate(() => JSON.parse(sessionStorage.getItem('yeet.remote.resume.v1') || '{}').clientId)

  await page.evaluate(() => {
    ;(window as unknown as { __yeetDisconnect: () => void }).__yeetDisconnect()
  })

  await expect.poll(async () => page.evaluate(() => {
    const hellos = (window as unknown as { __yeetSent: Array<Record<string, unknown>> }).__yeetSent
      .filter((item) => item.type === 'hello')
    if (hellos.length < 2) return null
    return {
      firstClientId: hellos[0].client_id,
      reconnectClientId: hellos.at(-1)?.client_id,
      last_sequence: hellos.at(-1)?.last_sequence,
      last_revision: hellos.at(-1)?.last_revision,
    }
  })).toMatchObject({ last_sequence: 2, last_revision: 2 })
  const reconnectIdentity = await page.evaluate(() => {
    const hellos = (window as unknown as { __yeetSent: Array<Record<string, unknown>> }).__yeetSent
      .filter((item) => item.type === 'hello')
    return hellos.at(-1)?.client_id
  })
  expect(reconnectIdentity).toBe(assignedClientId)
  await expect(page.locator('.desktop-status').first()).toContainText('connected')
})

test('settings expose runtime, providers, capabilities, sandbox and sessions', async ({ page }) => {
  if ((page.viewportSize()?.width ?? 1000) < 900) {
    await page.getByTestId('open-sessions').click()
    await page.getByTestId('session-drawer').getByRole('button', { name: 'Settings' }).click()
  } else {
    await page.locator('.desktop-session-sidebar').getByRole('button', { name: 'Settings' }).click()
  }
  await expect(page).toHaveURL(/\/settings/)
  await expect(page.getByRole('heading', { name: 'Runtime' })).toBeVisible()
  await expect(page.getByText('Foundation memory')).toBeVisible()

  if ((page.viewportSize()?.width ?? 1000) < 900) {
    const runtimeHeading = page.getByRole('heading', { name: 'Runtime' })
    const settingsNavButton = page.getByRole('button', { name: 'Providers', exact: true })
    const headingFontSize = await runtimeHeading.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))
    const navButtonBox = await settingsNavButton.boundingBox()
    expect(headingFontSize).toBeLessThanOrEqual(22)
    expect(navButtonBox?.height ?? 999).toBeLessThanOrEqual(42)
  }

  await page.getByRole('button', { name: 'Providers', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Providers' })).toBeVisible()
  await expect(page.getByText('69% left')).toBeVisible()

  await page.getByRole('button', { name: 'Capabilities', exact: true }).click()
  await expect(page.getByText('Yeet MCP')).toBeVisible()

  await page.getByRole('button', { name: 'Sandbox', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Sandbox & permissions' })).toBeVisible()
  await expect(page.getByText('api.openai.com:443')).toBeVisible()
  await expect(page.getByText('OPENAI_API_KEY')).toBeVisible()

  await page.getByRole('button', { name: 'Sessions', exact: true }).click()
  await expect(page.getByText('2 saved sessions')).toBeVisible()
})

test('model picker searches, filters providers, and supports keyboard selection', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')
  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: {
      active_model: 'openai/gpt-5.6-sol',
      available_models: [
        'openai/gpt-5.6-sol',
        'openai/gpt-5.6-luna',
        'anthropic/claude-opus-4.1',
        'google/gemini-3-pro',
      ],
    },
  })

  const picker = page.locator('.top-bar').getByTestId('model-picker')
  await picker.getByTestId('model-picker-trigger').click()
  const search = picker.getByTestId('model-search')
  await search.fill('opus')
  await expect(picker.getByRole('option')).toHaveCount(1)
  await expect(picker.getByRole('option')).toContainText('claude-opus-4.1')
  await search.press('Enter')

  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'select_model', model: 'anthropic/claude-opus-4.1' }),
  ]))

  await picker.getByTestId('model-picker-trigger').click()
  await picker.getByRole('button', { name: 'Google', exact: true }).click()
  await expect(picker.getByRole('option')).toHaveCount(1)
  await expect(picker.getByRole('option')).toContainText('gemini-3-pro')
  await picker.getByTestId('model-search').press('Escape')
  await expect(picker.getByTestId('model-picker-popover')).not.toBeAttached()
})

test('model picker stays touch-friendly inside a narrow viewport', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)
  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: {
      active_model: 'openai/gpt-5.6-sol',
      available_models: ['openai/gpt-5.6-sol', 'anthropic/claude-sonnet-4.5', 'google/gemini-3-pro'],
    },
  })

  const picker = page.locator('.top-bar').getByTestId('model-picker')
  const trigger = picker.getByTestId('model-picker-trigger')
  await trigger.click()
  const popover = picker.getByTestId('model-picker-popover')
  await expect(popover).toBeVisible()

  const viewport = page.viewportSize()
  const popoverBox = await popover.boundingBox()
  const triggerBox = await trigger.boundingBox()
  const optionBox = await picker.getByRole('option').first().boundingBox()
  expect(viewport).not.toBeNull()
  expect(popoverBox).not.toBeNull()
  expect((popoverBox?.x ?? -1)).toBeGreaterThanOrEqual(0)
  expect((popoverBox?.x ?? 0) + (popoverBox?.width ?? 0)).toBeLessThanOrEqual((viewport?.width ?? 0) + 1)
  expect(triggerBox?.height ?? 0).toBeGreaterThanOrEqual(40)
  expect(optionBox?.height ?? 0).toBeGreaterThanOrEqual(44)

  expect(optionBox?.height ?? 999).toBeLessThanOrEqual(46)
})

test('uses desktop panes without terminal presentation', async ({ page }, testInfo) => {
  test.skip((page.viewportSize()?.width ?? 0) <= 1280)
  await expect(page.locator('.desktop-session-sidebar')).toBeVisible()
  await page.getByTestId('toggle-inspector').click()
  await expect(page.getByTestId('activity-inspector')).toBeVisible()
  await expect(page.locator('body')).not.toContainText('Ctrl+C')
  await expect(page.locator('body')).not.toContainText('COLSxROWS')
})

test('workspace list keeps every known project visible and reconnects with the same client', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 0) < 900)
  await expect.poll(async () => page.evaluate(() => {
    const resume = JSON.parse(sessionStorage.getItem('yeet.remote.resume.v1') || '{}')
    return resume.clientId ?? null
  })).not.toBeNull()
  const before = await page.evaluate(() => {
    const resume = JSON.parse(sessionStorage.getItem('yeet.remote.resume.v1') || '{}')
    return resume.clientId
  })

  const sidebar = page.locator('.desktop-session-sidebar')
  const workspaceItems = sidebar.getByTestId('workspace-item')
  await expect(workspaceItems).toHaveCount(2)
  await expect(workspaceItems.filter({ hasText: 'Yeet' })).toHaveAttribute('aria-current', 'location')
  await expect(workspaceItems.filter({ hasText: 'AnotherProject' })).toBeVisible()
  await workspaceItems.filter({ hasText: 'AnotherProject' }).click()

  await expect.poll(async () => page.evaluate(() => {
    const hellos = (window as unknown as TestHooks).__yeetSent.filter((item) => item.type === 'hello')
    return hellos.at(-1)?.workspace
  })).toBe('/Users/test/Code/Rust/AnotherProject')
  const after = await page.evaluate(() => {
    const hellos = (window as unknown as TestHooks).__yeetSent.filter((item) => item.type === 'hello')
    return hellos.at(-1)?.client_id
  })
  expect(after).toBe(before)
  await expect(sidebar.getByTestId('workspace-item').filter({ hasText: 'AnotherProject' })).toHaveAttribute('aria-current', 'location')
  await expect(sidebar.getByTestId('active-workspace-sessions')).toBeVisible()

  const resume = await page.evaluate(() => JSON.parse(sessionStorage.getItem('yeet.remote.resume.v1') || '{}'))
  expect(resume.workspace).toBe('/Users/test/Code/Rust/AnotherProject')
})

test('phone uses drawers and sheets with a conversation-first layout', async ({ page }, testInfo) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)
  await expect(page.locator('.desktop-session-sidebar')).toBeHidden()
  await page.getByTestId('open-sessions').click()
  await expect(page.getByTestId('session-drawer')).toBeVisible()
  await expect(page.getByTestId('session-drawer').getByText('Protocol review')).toBeVisible()
  await page.getByRole('button', { name: 'Close sessions' }).click()

  await page.getByTestId('open-status').click()
  const statusSheet = page.getByTestId('status-sheet')
  await expect(statusSheet).toBeVisible()
  const statusHeading = page.getByText('Session settings')
  await expect(statusHeading).toBeVisible()
  const attachSkill = statusSheet.getByRole('button', { name: 'Attach Review skill' })
  await expect(attachSkill).toBeVisible()
  const statusHeadingFontSize = await statusHeading.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))
  const skillChipBox = await attachSkill.boundingBox()
  expect(statusHeadingFontSize).toBeLessThanOrEqual(17)
  expect(skillChipBox?.height ?? 999).toBeLessThanOrEqual(50)
  await attachSkill.click()
  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'toggle_capability', id: 'skill:review' }),
  ]))

  await statusSheet.getByRole('button', { name: 'Close session controls' }).click()
  const composer = page.getByTestId('composer')
  const topBar = page.locator('.top-bar')
  const modelTrigger = topBar.getByTestId('model-picker-trigger')
  const textarea = page.getByLabel('Message Yeet')
  const conversationCopy = page.locator('.assistant-message .markdown-document').first()
  const box = await composer.boundingBox()
  const topBox = await topBar.boundingBox()
  const modelBox = await modelTrigger.boundingBox()
  const viewport = page.viewportSize()
  const textareaFontSize = await textarea.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))
  const conversationFontSize = await conversationCopy.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))
  expect(box).not.toBeNull()
  expect(topBox).not.toBeNull()
  expect(modelBox).not.toBeNull()
  expect(viewport).not.toBeNull()
  expect(box?.x ?? 0).toBeGreaterThanOrEqual(8)
  expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual((viewport?.width ?? 0) - 8)
  expect((box?.y ?? 0) + (box?.height ?? 0)).toBeLessThanOrEqual((viewport?.height ?? 0) - 6)
  expect(topBox?.x ?? 0).toBeGreaterThanOrEqual(8)
  expect((topBox?.x ?? 0) + (topBox?.width ?? 0)).toBeLessThanOrEqual((viewport?.width ?? 0) - 8)
  expect(topBox?.height ?? 0).toBeGreaterThanOrEqual(50)
  expect(modelBox?.height ?? 0).toBeGreaterThanOrEqual(42)
  expect(textareaFontSize).toBeGreaterThanOrEqual(18)
  expect(conversationFontSize).toBeGreaterThanOrEqual(18)
  expect(textareaFontSize).toBeGreaterThan(statusHeadingFontSize)
  expect(conversationFontSize).toBeGreaterThan(statusHeadingFontSize)
  if ((viewport?.height ?? 0) > 500) {
    expect(topBox?.height ?? 0).toBeGreaterThanOrEqual(64)
    expect(box?.height ?? 0).toBeGreaterThanOrEqual(106)
    expect(textareaFontSize).toBeGreaterThanOrEqual(19.5)
    expect(conversationFontSize).toBeGreaterThanOrEqual(19)
  }

  await page.getByTestId('transcript').evaluate((element) => { element.scrollTop = element.scrollHeight })
  await emit(page, { type: 'state_update', version: 1, sequence: 2, revision: 2, patch: { conversation: [] } })
  const emptyTitle = page.getByRole('heading', { name: 'What can Yeet do for you?' })
  await expect(emptyTitle).toBeVisible()
  await expect.poll(async () => page.getByTestId('transcript').evaluate((element) => element.scrollTop)).toBe(0)
  const emptyTitleFontSize = await emptyTitle.evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize))
  expect(emptyTitleFontSize).toBeGreaterThanOrEqual((viewport?.height ?? 0) > 500 ? 34 : 30)
})

test('session, model, reasoning, and session-creation controls send semantic commands', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: { is_streaming: true, active_run_id: 'background-run' },
  })
  await page.getByRole('button', { name: /Protocol review/ }).click()
  await page.getByTestId('new-session').click()

  await page.getByRole('button', { name: 'Session controls' }).click()
  const statusSheet = page.getByTestId('status-sheet')
  const modelPicker = statusSheet.getByTestId('model-picker')
  await modelPicker.getByTestId('model-picker-trigger').click()
  await modelPicker.getByRole('option', { name: /gpt-5\.6-luna/i }).click()
  await page.getByRole('button', { name: 'low', exact: true }).click()

  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'load_session', session_id: 'session-b' }),
    expect.objectContaining({ type: 'new_session' }),
    expect.objectContaining({ type: 'select_model', model: 'gpt-5.6-luna' }),
    expect.objectContaining({ type: 'select_reasoning', level: 'low' }),
  ]))
})

test('permission prompts remain actionable and use semantic allow/deny commands', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')
  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: {
      pending_shell_permission: {
        id: 'shell-1', kind: 'shell', command: 'cargo test --all-targets', operation: 'execute', reason: 'Run verification',
      },
    },
  })
  const prompt = page.getByTestId('permission-prompt')
  await expect(prompt).toContainText('cargo test --all-targets')
  await prompt.getByRole('button', { name: 'Allow' }).click()

  await emit(page, {
    type: 'state_update', version: 1, sequence: 3, revision: 3,
    patch: {
      pending_shell_permission: null,
      pending_native_app_permission: {
        id: 'native-1', server: 'Computer Use', tool: 'click', appName: 'Safari', operation: 'interact', reason: 'Use browser',
      },
    },
  })
  await expect(prompt).toContainText('Safari · click')
  await prompt.getByRole('button', { name: 'Deny' }).click()

  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'allow_shell' }),
    expect.objectContaining({ type: 'deny_native_app' }),
  ]))
})

test('reconnect preserves client/session affinity and queues interrupt during the outage', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')
  await emit(page, {
    type: 'assistant_delta', version: 1, sequence: 2, revision: 2, entry_id: 'live-a',
    delta: 'still running', content: 'still running', reset: true,
  })
  await emit(page, {
    type: 'state_update', version: 1, sequence: 3, revision: 2,
    patch: { is_streaming: true, active_run_id: 'run-reconnect' },
  })
  const resume = await page.evaluate(() => JSON.parse(sessionStorage.getItem('yeet.remote.resume.v1') || '{}')) as Record<string, unknown>

  await page.evaluate(() => (window as unknown as TestHooks).__yeetDisconnect())
  await page.getByTestId('interrupt').click()

  await expect.poll(async () => {
    const sent = await page.evaluate(() => (window as unknown as TestHooks).__yeetSent)
    return sent.filter((item) => item.type === 'hello').length
  }).toBeGreaterThanOrEqual(2)
  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'interrupt' }),
  ]))

  const hellos = await page.evaluate(() => (window as unknown as TestHooks).__yeetSent.filter((item) => item.type === 'hello'))
  const reconnectHello = hellos.at(-1) as Record<string, unknown>
  expect(reconnectHello.client_id).toBe(resume.clientId)
  expect(reconnectHello.session_id).toBe(resume.sessionId)
  expect(Number(reconnectHello.last_sequence)).toBeGreaterThanOrEqual(3)
  await expect(page.locator('.desktop-status').first()).toContainText('connected')

  await page.evaluate(() => (window as unknown as TestHooks).__yeetRestart())
  await expect(page.locator('.desktop-status').first()).toContainText('connected')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('independent tabs keep distinct Remote client identities', async ({ page, context }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')
  const firstClientId = await page.evaluate(() => JSON.parse(sessionStorage.getItem('yeet.remote.resume.v1') || '{}').clientId)
  const second = await context.newPage()
  await installMockRemote(second)
  await second.goto('/')
  await expect(second.getByText('Interface ready')).toBeVisible()
  const secondClientId = await second.evaluate(() => JSON.parse(sessionStorage.getItem('yeet.remote.resume.v1') || '{}').clientId)
  expect(firstClientId).toBeTruthy()
  expect(secondClientId).toBeTruthy()
  expect(secondClientId).not.toBe(firstClientId)
  await second.close()
})

test('long transcripts, code, and tool output stay bounded while manual scroll is preserved', async ({ page }, testInfo) => {
  test.skip(!['desktop', 'mobile-small'].includes(testInfo.project.name))
  const conversation = Array.from({ length: 160 }, (_, index) => ({
    id: `history-${index}`,
    kind: index % 2 === 0
      ? { type: 'user', content: `Historical request ${index} ${'x'.repeat(120)}` }
      : { type: 'assistant', content: `Historical response ${index}\n\n${'content '.repeat(45)}` },
  }))
  conversation.push({
    id: 'large-code',
    kind: {
      type: 'assistant',
      content: `## Large code\n\n\`\`\`ts\n${`const excessivelyLongIdentifier = "${'z'.repeat(260)}";\n`.repeat(80)}\`\`\``,
    },
  })
  await emit(page, { type: 'conversation_reset', version: 1, sequence: 2, revision: 2, conversation })
  await expect(page.getByText('Large code')).toBeVisible()

  const largeCode = page.locator('.code-block').last()
  const codeBox = await largeCode.locator('pre').boundingBox()
  expect(codeBox?.height ?? 9999).toBeLessThanOrEqual(522)
  const selectedCode = await largeCode.locator('code').evaluate((node) => {
    const range = document.createRange()
    range.selectNodeContents(node)
    const selection = window.getSelection()
    selection?.removeAllRanges()
    selection?.addRange(range)
    return selection?.toString() ?? ''
  })
  expect(selectedCode).toContain('excessivelyLongIdentifier')
  await expect(largeCode.getByRole('button', { name: 'Copy' })).toBeVisible()

  const scroller = page.locator('.transcript-scroller')
  await scroller.evaluate((element) => {
    element.scrollTop = 0
    element.dispatchEvent(new Event('scroll'))
  })
  await emit(page, {
    type: 'conversation_entry', version: 1, sequence: 3, revision: 3,
    entry: { id: 'live-long', kind: { type: 'assistant', content: '' } },
  })
  await emit(page, {
    type: 'assistant_delta', version: 1, sequence: 4, revision: 3, entry_id: 'live-long',
    delta: 'new streaming tail', content: 'new streaming tail', reset: true,
  })
  await emit(page, { type: 'state_update', version: 1, sequence: 5, revision: 3, patch: { is_streaming: true } })
  await expect(page.getByText('new streaming tail')).toHaveCount(1)
  expect(await scroller.evaluate((element) => element.scrollTop)).toBeLessThan(10)

  const longResult = 'tool output '.repeat(5000)
  await emit(page, {
    type: 'tool_update', version: 1, sequence: 6, revision: 4,
    entry: {
      id: 'tool-long',
      kind: { type: 'toolCall', toolCall: { id: 'tool-long', name: 'read_file', arguments: '{"path":"huge.txt"}', status: 'completed', result: longResult } },
    },
    tool_call: null,
  })
  const tool = page.getByTestId('tool-card').filter({ hasText: 'read_file' }).last()
  const toolToggle = tool.locator('[data-tool-toggle]')
  await toolToggle.scrollIntoViewIfNeeded()
  await toolToggle.click()
  const details = tool.getByTestId('tool-details')
  await expect(details).toBeVisible()
  const detailBox = await details.locator('pre').last().boundingBox()
  expect(detailBox?.height ?? 9999).toBeLessThanOrEqual(442)

  await expect(details.getByRole('button', { name: 'Copy tool result' })).toBeVisible()
  const selectedToolOutput = await details.locator('pre').last().evaluate((node) => {
    const range = document.createRange()
    range.selectNodeContents(node)
    const selection = window.getSelection()
    selection?.removeAllRanges()
    selection?.addRange(range)
    return selection?.toString() ?? ''
  })
  expect(selectedToolOutput.length).toBeGreaterThan(100)
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(page.viewportSize()!.width + 1)
  await expect(page.locator('.code-block pre').last()).toBeVisible()
})

test('visual-viewport keyboard geometry keeps composer and sheets above the visible bottom', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)
  await page.evaluate(() => {
    document.documentElement.style.setProperty('--visual-viewport-top', '28px')
    document.documentElement.style.setProperty('--visual-viewport-height', '300px')
  })
  const app = await page.locator('.remote-app').boundingBox()
  expect(app?.y).toBeCloseTo(28, 0)
  expect(app?.height).toBeCloseTo(300, 0)

  await page.getByLabel('Message Yeet').focus()
  const composer = await page.getByTestId('composer').boundingBox()
  expect((composer?.y ?? 0) + (composer?.height ?? 0)).toBeLessThanOrEqual(329)
  expect(Number.parseFloat(await page.getByLabel('Message Yeet').evaluate((node) => getComputedStyle(node).fontSize))).toBeGreaterThanOrEqual(16)

  await page.getByTestId('open-status').click()
  const sheet = await page.getByTestId('status-sheet').boundingBox()
  expect(sheet?.y).toBeCloseTo(28, 0)
  expect((sheet?.y ?? 0) + (sheet?.height ?? 0)).toBeLessThanOrEqual(329)
})
