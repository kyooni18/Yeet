import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

async function openSettings(page: Page, section: string) {
  await installMockRemote(page)
  await page.goto(`/settings/${section}`)
  await expect(page.getByRole('heading', { name: section === 'sandbox' ? 'Sandbox & permissions' : section[0].toUpperCase() + section.slice(1) })).toBeVisible()
}

test('narrow settings navigation reveals a directly routed active section', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width > 899)

  await openSettings(page, 'sessions')

  const geometry = await page.locator('.settings-nav').evaluate((nav) => {
    const active = nav.querySelector<HTMLElement>('[aria-current="page"]')
    const navRect = nav.getBoundingClientRect()
    const activeRect = active?.getBoundingClientRect()
    return {
      scrollLeft: nav.scrollLeft,
      scrollWidth: nav.scrollWidth,
      clientWidth: nav.clientWidth,
      navLeft: navRect.left,
      navRight: navRect.right,
      activeLeft: activeRect?.left ?? Number.NaN,
      activeRight: activeRect?.right ?? Number.NaN,
    }
  })

  if (geometry.scrollWidth > geometry.clientWidth) {
    expect(geometry.scrollLeft).toBeGreaterThan(0)
    expect(geometry.activeLeft).toBeGreaterThanOrEqual(geometry.navLeft - 1)
    expect(geometry.activeRight).toBeLessThanOrEqual(geometry.navRight + 1)
  }

  await expect(page.locator('html')).not.toHaveCSS('overflow-x', 'scroll')

  await page.getByRole('button', { name: 'Sandbox', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Sandbox & permissions' })).toBeVisible()
  await page.locator('.settings-app').evaluate((app) => { app.scrollTop = 420 })
  const topbarBox = await page.locator('.settings-topbar').boundingBox()
  const stickyNavBox = await page.locator('.settings-nav').boundingBox()
  expect(topbarBox).not.toBeNull()
  expect(stickyNavBox).not.toBeNull()
  expect(stickyNavBox!.y).toBeGreaterThanOrEqual(topbarBox!.y + topbarBox!.height - 1)

  await page.goto('/settings/runtime')
  await expect(page.getByRole('heading', { name: 'Runtime' })).toBeVisible()
  const switchBox = await page.getByRole('switch', { name: 'Goal' }).boundingBox()
  expect(switchBox).not.toBeNull()
  expect(switchBox?.width ?? 0).toBeGreaterThanOrEqual(44)
  expect(switchBox?.height ?? 0).toBeGreaterThanOrEqual(44)
})


test('settings follows the shifted visual viewport', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'mobile-portrait')
  await openSettings(page, 'runtime')

  await page.evaluate(() => {
    document.documentElement.style.setProperty('--visual-viewport-top', '117px')
    document.documentElement.style.setProperty('--visual-viewport-height', '401px')
  })

  const shell = await page.locator('.settings-app').boundingBox()
  expect(shell).not.toBeNull()
  expect(Math.round(shell!.y)).toBe(117)
  expect(Math.round(shell!.height)).toBe(401)
})

test('settings controls expose stable names and selected state to assistive technology', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await openSettings(page, 'runtime')

  await expect(page.getByRole('switch', { name: 'Goal' })).toHaveAttribute('aria-checked', /true|false/)
  await expect(page.getByRole('switch', { name: 'Foundation memory' })).toHaveAttribute('aria-checked', /true|false/)
  const reasoning = page.getByRole('group', { name: 'Reasoning level' })
  await expect(reasoning.getByRole('button', { name: 'auto' })).toHaveAttribute('aria-pressed', /true|false/)

  await page.goto('/settings/sandbox')
  await expect(page.getByRole('heading', { name: 'Sandbox & permissions' })).toBeVisible()
  await expect(page.getByRole('group', { name: 'Sandbox preset' })).toBeVisible()
  await expect(page.getByRole('combobox', { name: 'Execution mode' })).toBeVisible()
  await expect(page.getByRole('combobox', { name: 'Workspace access mode' })).toBeVisible()
  await expect(page.getByRole('switch', { name: 'Auto approve' })).toHaveAttribute('aria-checked', /true|false/)
  await expect(page.getByRole('switch', { name: 'Scratch writable' })).toHaveAttribute('aria-checked', /true|false/)
  await expect(page.getByRole('textbox', { name: 'Workspace path' })).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Network host' })).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Network port' })).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Environment variable name' })).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Environment variable value' })).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Secret ID' })).toBeVisible()
})


test('runtime sandbox and capability sections reflect backend working state', async ({ page }, testInfo) => {
  test.skip(!['desktop', 'mobile-small', 'ipad-portrait', 'ipad-landscape'].includes(testInfo.project.name))
  await openSettings(page, 'runtime')

  let nextSequence = 2
  const emitState = (patch: Record<string, unknown>) => {
    const sequence = nextSequence++
    return page.evaluate(({ nextPatch, sequence }) => {
      ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
        type: 'state_update', version: 1, sequence, revision: sequence, patch: nextPatch,
      })
    }, { nextPatch: patch, sequence })
  }

  await emitState({ settings_working: true, settings_notice: null })
  await expect(page.getByTestId('runtime-settings-status')).toHaveText('Saving workspace settings…')
  await expect(page.getByRole('switch', { name: 'Foundation memory' })).toBeDisabled()

  await emitState({ settings_working: false, settings_notice: 'Foundation memory enabled.' })
  await expect(page.getByTestId('runtime-settings-status')).toHaveText('Foundation memory enabled.')
  await expect(page.getByRole('switch', { name: 'Foundation memory' })).toBeEnabled()

  await page.getByRole('navigation', { name: 'Settings sections' }).getByRole('button', { name: 'Capabilities' }).click()
  await emitState({ available_capabilities: [], is_loading_capabilities: true })
  await expect(page.getByTestId('capabilities-loading')).toHaveText('Loading capabilities…')
  await expect(page.getByText('No capabilities have been reported yet.')).toHaveCount(0)

  await emitState({ is_loading_capabilities: false })
  await expect(page.getByText('No capabilities have been reported yet.')).toBeVisible()


  await emitState({
    available_capabilities: [
      { id: 'web-search', kind: 'capability', name: 'Web Search', description: 'Search the live web', enabled: true },
      { id: 'builtin:computer-use', kind: 'builtin', name: 'Computer Use', description: 'Control native apps', enabled: true },
      { id: 'builtin:skyline', kind: 'builtin', name: 'Skyline', description: 'Coordinate shared work', enabled: false },
      { id: 'skill:polaris', kind: 'skill', name: 'Polaris', description: 'Investigation workflow', enabled: true },
      { id: 'mcp:yeet', kind: 'mcp', name: 'Yeet MCP', description: 'Workspace execution tools', enabled: true },
      { id: 'builtin:shell', kind: 'builtin', name: 'Shell', description: 'Run workspace commands', enabled: true },
    ],
    is_streaming: true,
  })
  await expect(page.getByTestId('capabilities-locked')).toHaveText('Capability changes are locked while a response is running.')
  await expect(page.getByRole('switch')).toHaveCount(3)
  for (const name of ['Web Search', 'Computer Use', 'Skyline']) await expect(page.getByRole('switch', { name })).toBeDisabled()
  await expect(page.getByText('1/1 attached')).toBeVisible()
  await expect(page.getByText('1/1 enabled')).toHaveCount(2)
  await expect(page.getByRole('switch', { name: 'Polaris' })).toHaveCount(0)

  await page.locator('details').filter({ hasText: 'Skills' }).locator('summary').click()
  await expect(page.getByRole('switch', { name: 'Polaris' })).toBeDisabled()

  await emitState({ is_streaming: false })
  await expect(page.getByTestId('capabilities-locked')).toHaveCount(0)
  await expect(page.getByRole('switch', { name: 'Polaris' })).toBeEnabled()

  await page.getByRole('navigation', { name: 'Settings sections' }).getByRole('button', { name: 'Sandbox' }).click()
  await emitState({ sandbox_working: true, sandbox_notice: null })
  await expect(page.getByTestId('sandbox-settings-status')).toHaveText('Saving sandbox policy…')
  await expect(page.getByRole('switch', { name: 'Auto approve' })).toBeDisabled()
  await expect(page.getByRole('button', { name: 'Reset sandbox policy' })).toBeDisabled()

  await emitState({ sandbox_working: false, sandbox_notice: 'Sandbox policy could not be saved.' })
  await expect(page.getByTestId('sandbox-settings-status')).toHaveText('Sandbox policy could not be saved.')
  await expect(page.getByRole('switch', { name: 'Auto approve' })).toBeEnabled()
  await expect(page.getByRole('button', { name: 'Reset sandbox policy' })).toBeEnabled()
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)
  expect(overflow).toBeLessThanOrEqual(1)
})


test('settings current session row is idempotent while other sessions still load', async ({ page }, testInfo) => {
  test.skip(!['desktop', 'mobile-small', 'ipad-portrait', 'ipad-landscape'].includes(testInfo.project.name))
  await openSettings(page, 'sessions')

  const loadCommands = () => page.evaluate(() => (
    (window as unknown as { __yeetSent: Array<{ type?: string; command?: { type?: string; session_id?: string } }> }).__yeetSent
      .filter((item) => item.type === 'command' && item.command?.type === 'load_session')
  ))
  const current = page.getByRole('button', { name: /Remote WebUI/ })
  await expect(current).toHaveAttribute('aria-current', 'page')
  await expect(current).toHaveAttribute('aria-disabled', 'true')

  await page.evaluate(() => {
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
      type: 'state_update', version: 1, sequence: 2, revision: 2,
      patch: { is_streaming: true, active_run_id: 'settings-current-session' },
    })
  })
  const before = (await loadCommands()).length
  await current.evaluate((element) => (element as HTMLButtonElement).click())
  await expect.poll(async () => (await loadCommands()).length).toBe(before)

  const other = page.getByRole('button', { name: /Protocol review/ })
  await expect(other).not.toHaveAttribute('aria-disabled', 'true')
  await other.click()
  await expect.poll(async () => loadCommands()).toEqual(expect.arrayContaining([
    expect.objectContaining({ command: expect.objectContaining({ type: 'load_session', session_id: 'session-b' }) }),
  ]))
})

test('sessions use concise relative age and explain the empty state', async ({ page }, testInfo) => {
  test.skip(!['desktop', 'mobile-small', 'ipad-portrait', 'ipad-landscape'].includes(testInfo.project.name))
  await openSettings(page, 'sessions')

  const currentSession = page.getByRole('button', { name: /Remote WebUI/ })
  const currentAge = currentSession.locator('time')
  await expect(currentAge).toBeVisible()
  await expect(currentAge).toHaveText(/^(now|\d+m)$/)
  await expect(currentAge).toHaveAttribute('datetime', /T/)
  expect(await currentSession.innerText()).not.toMatch(/\d{4}-\d{2}-\d{2}T/)

  await page.evaluate(() => {
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
      type: 'state_update', version: 1, sequence: 2, revision: 2,
      patch: {
        saved_sessions: [{ id: 'singular', title: 'One message', updated_at: new Date().toISOString(), model: 'openai/gpt-5.6-sol', message_count: 1 }],
        current_session_id: 'singular',
      },
    })
  })
  await expect(page.getByText('1 saved session', { exact: true })).toBeVisible()
  await expect(page.getByText('openai/gpt-5.6-sol · 1 message', { exact: true })).toBeVisible()

  await page.evaluate(() => {
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
      type: 'state_update', version: 1, sequence: 3, revision: 3,
      patch: { saved_sessions: [], current_session_id: null },
    })
  })
  await expect(page.getByText('No saved sessions yet. Start a new session when you want a clean conversation.')).toBeVisible()
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)
  expect(overflow).toBeLessThanOrEqual(1)
})


test('provider actions identify the provider in their accessible names', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')
  await openSettings(page, 'providers')

  await page.evaluate(() => {
    const providers = [
      { provider: 'openai', authenticated: false, method: 'api_key', expires_at: null, error: null, usage: null },
      { provider: 'anthropic', authenticated: false, method: 'api_key', expires_at: null, error: null, usage: null },
      { provider: 'codex-cli', authenticated: false, method: 'browser', expires_at: null, error: null, usage: null },
      { provider: 'claude', authenticated: true, method: 'browser', expires_at: null, error: null, usage: null },
    ]
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
      type: 'state_update', version: 1, sequence: 2, revision: 2, patch: { auth_providers: providers },
    })
  })

  await expect(page.getByRole('button', { name: 'Sign in to Codex CLI' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Sign out of Claude Web' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Set API key for OpenAI' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Set API key for Claude (Anthropic API)' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Set key', exact: true })).toHaveCount(0)
})


test('provider backend working and notice state stays visible and blocks duplicate actions', async ({ page }, testInfo) => {
  test.skip(!['desktop', 'mobile-small', 'ipad-portrait', 'ipad-landscape'].includes(testInfo.project.name))
  await openSettings(page, 'providers')

  let nextSequence = 2
  const emitState = (patch: Record<string, unknown>) => {
    const sequence = nextSequence++
    return page.evaluate(({ nextPatch, sequence }) => {
      ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
        type: 'state_update', version: 1, sequence, revision: sequence, patch: nextPatch,
      })
    }, { nextPatch: patch, sequence })
  }

  await emitState({
    auth_working: true,
    auth_notice: 'Signing in to codex-cli…',
    providers_working: true,
    providers_notice: 'Saving local…',
  })

  await expect(page.getByTestId('auth-provider-status')).toHaveText('Signing in to codex-cli…')
  await expect(page.getByTestId('custom-provider-status')).toHaveText('Saving local…')
  await expect(page.getByRole('button', { name: 'Set API key for OpenAI' })).toBeDisabled()
  await expect(page.getByRole('button', { name: 'Save provider' })).toBeDisabled()
  await expect(page.getByRole('button', { name: 'Remove provider local' })).toBeDisabled()
  const busyOverflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)
  expect(busyOverflow).toBeLessThanOrEqual(1)

  await emitState({
    auth_working: false,
    auth_notice: 'Signed in to codex-cli.',
    providers_working: false,
    providers_notice: 'Saved provider local.',
  })

  await expect(page.getByTestId('auth-provider-status')).toHaveText('Signed in to codex-cli.')
  await expect(page.getByTestId('custom-provider-status')).toHaveText('Saved provider local.')
  await expect(page.getByRole('button', { name: 'Set API key for OpenAI' })).toBeEnabled()
  await expect(page.getByRole('button', { name: 'Save provider' })).toBeEnabled()
  await expect(page.getByRole('button', { name: 'Remove provider local' })).toBeEnabled()

  await emitState({
    auth_providers: [], auth_working: true, auth_notice: null,
    provider_configurations: [], providers_working: true, providers_notice: null,
  })
  await expect(page.getByTestId('auth-provider-status')).toHaveText('Loading authentication providers…')
  await expect(page.getByTestId('custom-provider-status')).toHaveText('Loading custom providers…')
})


test('failed provider submission preserves typed values for retry after reconnect', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')
  await openSettings(page, 'providers')

  const providerId = page.getByRole('textbox', { name: 'Provider ID' })
  const providerUrl = page.getByRole('textbox', { name: 'Base URL' })
  await providerId.fill('retry-provider')
  await providerUrl.fill('https://retry.example.com/v1')

  await page.evaluate(() => {
    ;(window as unknown as { __yeetDisconnect: () => void }).__yeetDisconnect()
    document.querySelector<HTMLFormElement>('.provider-form')?.requestSubmit()
  })

  await expect(providerId).toHaveValue('retry-provider')
  await expect(providerUrl).toHaveValue('https://retry.example.com/v1')

  await expect.poll(() => page.locator('.settings-connection').innerText()).toBe('Connected')
  await page.getByRole('button', { name: 'Save provider' }).click()
  await expect(providerId).toHaveValue('')
  await expect(providerUrl).toHaveValue('')
})

test('failed sandbox additions preserve every unsent field', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')
  await openSettings(page, 'sandbox')

  const workspacePath = page.getByRole('textbox', { name: 'Workspace path' })
  const networkHost = page.getByRole('textbox', { name: 'Network host' })
  const networkPort = page.getByRole('textbox', { name: 'Network port' })
  const environmentKey = page.getByRole('textbox', { name: 'Environment variable name' })
  const environmentValue = page.getByRole('textbox', { name: 'Environment variable value' })
  const secretId = page.getByRole('textbox', { name: 'Secret ID' })

  await workspacePath.fill('src/generated')
  await networkHost.fill('internal.example.com')
  await networkPort.fill('8443')
  await environmentKey.fill('YEET_MODE')
  await environmentValue.fill('careful')
  await secretId.fill('OPENAI_API_KEY')

  await page.evaluate(() => {
    ;(window as unknown as { __yeetDisconnect: () => void }).__yeetDisconnect()
    for (const label of ['Workspace path', 'Network host', 'Environment variable name', 'Secret ID']) {
      document.querySelector<HTMLInputElement>(`[aria-label="${label}"]`)?.closest<HTMLFormElement>('form')?.requestSubmit()
    }
  })

  await expect(workspacePath).toHaveValue('src/generated')
  await expect(networkHost).toHaveValue('internal.example.com')
  await expect(networkPort).toHaveValue('8443')
  await expect(environmentKey).toHaveValue('YEET_MODE')
  await expect(environmentValue).toHaveValue('careful')
  await expect(secretId).toHaveValue('OPENAI_API_KEY')
})


test('custom provider removal requires a separate confirmation action', async ({ page }, testInfo) => {
  test.skip(!['desktop', 'mobile-small', 'ipad-portrait', 'ipad-landscape'].includes(testInfo.project.name))
  await openSettings(page, 'providers')

  const commandCount = () => page.evaluate(() => (
    (window as unknown as { __yeetSent: Array<{ type?: string }> }).__yeetSent
      .filter((item) => item.type === 'command').length
  ))
  const before = await commandCount()
  const remove = page.getByRole('button', { name: 'Remove provider local' })
  await remove.scrollIntoViewIfNeeded()
  const triggerBox = await remove.boundingBox()
  expect(triggerBox).not.toBeNull()

  await remove.click()
  const confirmation = page.getByRole('group', { name: 'Confirm removal of provider local' })
  await expect(confirmation).toBeVisible()
  expect(await commandCount()).toBe(before)

  await page.evaluate(() => {
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
      type: 'state_update', version: 1, sequence: 2, revision: 2,
      patch: { providers_notice: 'Provider list refreshed elsewhere.' },
    })
  })
  await expect(confirmation).toBeVisible()
  expect(await commandCount()).toBe(before)

  await page.evaluate(() => {
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
      type: 'state_update', version: 1, sequence: 3, revision: 3,
      patch: { provider_configurations: [{ id: 'local', base_url: 'http://127.0.0.1:11435/v1', require_api_key: false, header_count: 0 }] },
    })
  })
  await expect(confirmation).toBeHidden()
  await expect(remove).toBeVisible()
  await expect(remove).toBeFocused()
  expect(await commandCount()).toBe(before)

  const rearmedTriggerBox = await remove.boundingBox()
  expect(rearmedTriggerBox).not.toBeNull()

  await remove.click()
  await expect(confirmation).toBeVisible()

  const cancel = confirmation.getByRole('button', { name: 'Cancel removal of provider local' })
  const confirm = confirmation.getByRole('button', { name: 'Remove provider local' })
  await expect(cancel).toBeFocused()
  const cancelBox = await cancel.boundingBox()
  const confirmBox = await confirm.boundingBox()
  expect(cancelBox).not.toBeNull()
  expect(confirmBox).not.toBeNull()
  const triggerCenter = {
    x: rearmedTriggerBox!.x + rearmedTriggerBox!.width / 2,
    y: rearmedTriggerBox!.y + rearmedTriggerBox!.height / 2,
  }
  expect(triggerCenter.x).toBeGreaterThanOrEqual(cancelBox!.x)
  expect(triggerCenter.x).toBeLessThanOrEqual(cancelBox!.x + cancelBox!.width)
  expect(triggerCenter.y).toBeGreaterThanOrEqual(cancelBox!.y)
  expect(triggerCenter.y).toBeLessThanOrEqual(cancelBox!.y + cancelBox!.height)
  expect(triggerCenter.x < confirmBox!.x || triggerCenter.x > confirmBox!.x + confirmBox!.width).toBe(true)

  if ((page.viewportSize()?.width ?? 1000) < 900) {
    for (const control of [cancel, confirm]) {
      const box = await control.boundingBox()
      expect(box).not.toBeNull()
      expect(box!.width).toBeGreaterThanOrEqual(44)
      expect(box!.height).toBeGreaterThanOrEqual(44)
    }
  }

  await cancel.click()
  await expect(confirmation).toBeHidden()
  await expect(remove).toBeFocused()
  expect(await commandCount()).toBe(before)
})


test('sandbox reset requires a separate confirmation action', async ({ page }, testInfo) => {
  test.skip(!['desktop', 'mobile-small', 'ipad-portrait', 'ipad-landscape'].includes(testInfo.project.name))
  await openSettings(page, 'sandbox')

  const resetCommands = () => page.evaluate(() => (
    (window as unknown as { __yeetSent: Array<Record<string, unknown>> }).__yeetSent
      .filter((item) => item.type === 'command')
      .map((item) => item.command as Record<string, unknown>)
      .filter((command) => command.type === 'update_sandbox' && (command.action as Record<string, unknown>)?.type === 'reset')
      .length
  ))

  const reset = page.getByRole('button', { name: 'Reset sandbox policy' })
  await reset.scrollIntoViewIfNeeded()
  const triggerBox = await reset.boundingBox()
  expect(triggerBox).not.toBeNull()
  const before = await resetCommands()

  await reset.click()
  const confirmation = page.getByRole('group', { name: 'Confirm sandbox policy reset' })
  await expect(confirmation).toBeVisible()
  expect(await resetCommands()).toBe(before)

  await page.evaluate(() => {
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
      type: 'state_update', version: 1, sequence: 2, revision: 2,
      patch: { sandbox_notice: 'Policy refreshed elsewhere.' },
    })
  })
  await expect(confirmation).toBeVisible()
  expect(await resetCommands()).toBe(before)

  await page.evaluate(() => {
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
      type: 'state_update', version: 1, sequence: 3, revision: 3,
      patch: {
        sandbox_settings: {
          preset: 'safe', execution_mode: 'sandboxed', auto_approve: false, workspace_mode: 'all', workspace_paths: [], scratch_writable: true,
          network_allow: [], environment: [], secret_ids: [],
          limits: { wall_time_seconds: 120, max_stdout_bytes: 524288, max_stderr_bytes: 524288, max_memory_bytes: 1073741824, max_processes: 16 },
        },
      },
    })
  })
  await expect(confirmation).toBeHidden()
  await expect(reset).toBeVisible()

  await expect(reset).toBeFocused()
  expect(await resetCommands()).toBe(before)

  await reset.click()
  await expect(confirmation).toBeVisible()

  const cancel = confirmation.getByRole('button', { name: 'Cancel sandbox reset' })
  const confirm = confirmation.getByRole('button', { name: 'Confirm sandbox reset' })
  await expect(cancel).toBeFocused()

  const cancelBox = await cancel.boundingBox()
  const confirmBox = await confirm.boundingBox()
  expect(cancelBox).not.toBeNull()
  expect(confirmBox).not.toBeNull()
  const triggerCenter = {
    x: triggerBox!.x + triggerBox!.width / 2,
    y: triggerBox!.y + triggerBox!.height / 2,
  }
  expect(triggerCenter.x).toBeGreaterThanOrEqual(cancelBox!.x)
  expect(triggerCenter.x).toBeLessThanOrEqual(cancelBox!.x + cancelBox!.width)
  expect(triggerCenter.y).toBeGreaterThanOrEqual(cancelBox!.y)
  expect(triggerCenter.y).toBeLessThanOrEqual(cancelBox!.y + cancelBox!.height)

  if ((page.viewportSize()?.width ?? 1000) < 900) {
    for (const control of [cancel, confirm]) {
      const box = await control.boundingBox()
      expect(box).not.toBeNull()
      expect(box!.width).toBeGreaterThanOrEqual(44)
      expect(box!.height).toBeGreaterThanOrEqual(44)
    }
  }

  await cancel.click()
  await expect(confirmation).toBeHidden()
  await expect(reset).toBeFocused()
  expect(await resetCommands()).toBe(before)

  await reset.click()
  await page.getByRole('button', { name: 'Runtime', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Runtime' })).toBeVisible()
  await page.getByRole('button', { name: 'Sandbox', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Sandbox & permissions' })).toBeVisible()
  await expect(page.getByRole('group', { name: 'Confirm sandbox policy reset' })).toBeHidden()
  expect(await resetCommands()).toBe(before)

  const resetAgain = page.getByRole('button', { name: 'Reset sandbox policy' })

  await resetAgain.click()
  await page.getByRole('group', { name: 'Confirm sandbox policy reset' }).getByRole('button', { name: 'Confirm sandbox reset' }).click()
  await expect.poll(resetCommands).toBe(before + 1)
})


test('network allowlist rejects invalid ports without broadening the rule', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')
  await openSettings(page, 'sandbox')

  const networkActions = () => page.evaluate(() => (
    (window as unknown as { __yeetSent: Array<{ type?: string; command?: { type?: string; action?: { type?: string; host?: string; port?: number | null } } }> }).__yeetSent
      .filter((item) => item.type === 'command' && item.command?.type === 'update_sandbox' && item.command.action?.type === 'add_network')
      .map((item) => item.command!.action!)
  ))
  const host = page.getByRole('textbox', { name: 'Network host' })
  const port = page.getByRole('textbox', { name: 'Network port' })
  const allow = page.getByRole('button', { name: 'Allow', exact: true })
  const before = (await networkActions()).length

  await host.fill('.internal.example.com')
  await expect(host).toHaveAttribute('aria-invalid', 'true')
  await expect(page.getByRole('alert')).toHaveText('Host must use DNS-style labels without leading or trailing dots or hyphens.')
  await expect(allow).toBeDisabled()
  expect(await networkActions()).toHaveLength(before)
  await expect(host).toHaveValue('.internal.example.com')

  await host.fill('internal.example.com')
  await expect(host).not.toHaveAttribute('aria-invalid', 'true')

  await host.fill('internal.example.com')
  await port.fill('abc')
  await expect(port).toHaveAttribute('aria-invalid', 'true')
  await expect(page.getByRole('alert')).toHaveText('Port must be a whole number from 1 to 65535.')
  await expect(allow).toBeDisabled()
  expect(await networkActions()).toHaveLength(before)
  await expect(host).toHaveValue('internal.example.com')
  await expect(port).toHaveValue('abc')

  await port.fill('70000')
  await expect(port).toHaveAttribute('aria-invalid', 'true')
  await expect(allow).toBeDisabled()
  expect(await networkActions()).toHaveLength(before)

  await port.fill('443')
  await expect(port).not.toHaveAttribute('aria-invalid', 'true')
  await expect(allow).toBeEnabled()
  await allow.click()
  await expect.poll(networkActions).toContainEqual(expect.objectContaining({ host: 'internal.example.com', port: 443 }))
  await expect(host).toHaveValue('')
  await expect(port).toHaveValue('')

  await host.fill('allports.example.com')
  await allow.click()
  await expect.poll(networkActions).toContainEqual(expect.objectContaining({ host: 'allports.example.com', port: null }))
})


test('sandbox text additions preserve backend-invalid values for correction', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')
  await openSettings(page, 'sandbox')

  const actions = (type: string) => page.evaluate((actionType) => (
    (window as unknown as { __yeetSent: Array<{ type?: string; command?: { type?: string; action?: Record<string, unknown> } }> }).__yeetSent
      .filter((item) => item.type === 'command' && item.command?.type === 'update_sandbox' && item.command.action?.type === actionType)
      .map((item) => item.command!.action!)
  ), type)

  const path = page.getByRole('textbox', { name: 'Workspace path' })
  const addPath = page.getByRole('button', { name: 'Add path' })
  const pathBefore = (await actions('add_workspace_path')).length
  await path.fill('../secrets')
  await expect(path).toHaveAttribute('aria-invalid', 'true')
  await expect(page.getByRole('alert')).toHaveText('Use a workspace-relative path without parent traversal, a leading slash, or backslashes.')
  await expect(addPath).toBeDisabled()
  expect(await actions('add_workspace_path')).toHaveLength(pathBefore)
  await expect(path).toHaveValue('../secrets')
  await path.fill('src/generated')
  await addPath.click()
  await expect.poll(() => actions('add_workspace_path')).toContainEqual(expect.objectContaining({ path: 'src/generated' }))
  await expect(path).toHaveValue('')

  const environmentKey = page.getByRole('textbox', { name: 'Environment variable name' })
  const environmentValue = page.getByRole('textbox', { name: 'Environment variable value' })
  const setEnvironment = page.getByRole('button', { name: 'Set', exact: true })
  const environmentBefore = (await actions('set_environment')).length
  await environmentKey.fill('9BAD')
  await environmentValue.fill('careful')
  await expect(environmentKey).toHaveAttribute('aria-invalid', 'true')
  await expect(page.getByRole('alert')).toHaveText('Environment names must start with a letter or underscore and contain only letters, numbers, and underscores.')
  await expect(setEnvironment).toBeDisabled()
  expect(await actions('set_environment')).toHaveLength(environmentBefore)
  await expect(environmentKey).toHaveValue('9BAD')
  await expect(environmentValue).toHaveValue('careful')
  await environmentKey.fill('_GOOD9')
  await setEnvironment.click()
  await expect.poll(() => actions('set_environment')).toContainEqual(expect.objectContaining({ key: '_GOOD9', value: 'careful' }))
  await expect(environmentKey).toHaveValue('')
  await expect(environmentValue).toHaveValue('')

  const secret = page.getByRole('textbox', { name: 'Secret ID' })
  const attachSecret = page.getByRole('button', { name: 'Attach secret' })
  const secretBefore = (await actions('add_secret')).length
  await secret.fill('bad secret!')
  await expect(secret).toHaveAttribute('aria-invalid', 'true')
  await expect(page.getByRole('alert')).toHaveText('Secret IDs may contain only letters, numbers, dots, hyphens, and underscores (128 characters max).')
  await expect(attachSecret).toBeDisabled()
  expect(await actions('add_secret')).toHaveLength(secretBefore)
  await expect(secret).toHaveValue('bad secret!')
  await secret.fill('OPENAI_API_KEY')
  await attachSecret.click()
  await expect.poll(() => actions('add_secret')).toContainEqual(expect.objectContaining({ id: 'OPENAI_API_KEY' }))
  await expect(secret).toHaveValue('')
})
