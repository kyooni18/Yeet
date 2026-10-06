import { expect, test, type BrowserContext, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetSent: Array<Record<string, unknown>>
  __yeetEmit: (message: Record<string, unknown>) => void
  __yeetDisconnect: () => void
  __yeetRestart: () => void
}

async function emit(page: Page, message: Record<string, unknown>) {
  await page.evaluate((payload) => {
    ;(window as unknown as TestHooks).__yeetEmit(payload)
  }, message)
}

async function sentCommands(page: Page) {
  return page.evaluate(() => {
    const sent = (window as unknown as TestHooks).__yeetSent
    return sent
      .filter((item) => item.type === 'command')
      .map((item) => item.command as Record<string, unknown>)
  })
}

async function hellos(page: Page) {
  return page.evaluate(() =>
    (window as unknown as TestHooks).__yeetSent.filter((item) => item.type === 'hello')
  )
}

async function resumeState(page: Page) {
  return page.evaluate(() =>
    JSON.parse(sessionStorage.getItem('yeet.remote.resume.v1') || '{}') as Record<string, unknown>
  )
}

async function openSecondRemote(context: BrowserContext) {
  const page = await context.newPage()
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  return page
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('reconnect resumes with the same client and latest semantic cursor', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: { current_context_tokens: 43000 },
  })

  const before = await resumeState(page)
  const firstClientID = before.clientId
  const firstSessionID = before.sessionId
  expect(firstClientID).toBeTruthy()
  expect(firstSessionID).toBeTruthy()

  await page.evaluate(() => (window as unknown as TestHooks).__yeetDisconnect())

  await expect.poll(async () => (await hellos(page)).length).toBeGreaterThanOrEqual(2)
  const reconnectHello = (await hellos(page)).at(-1) as Record<string, unknown>
  expect(reconnectHello.client_id).toBe(firstClientID)
  expect(reconnectHello.session_id).toBe(firstSessionID)
  expect(Number(reconnectHello.last_sequence)).toBeGreaterThanOrEqual(2)
  expect(Number(reconnectHello.last_revision)).toBeGreaterThanOrEqual(2)

  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('interrupt requested during an outage is delivered after reconnect', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await emit(page, {
    type: 'assistant_delta',
    version: 1,
    sequence: 2,
    revision: 2,
    entry_id: 'live-a',
    delta: '',
    content: 'still running',
    reset: true,
  })
  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 3,
    revision: 2,
    patch: { is_streaming: true, active_run_id: 'run-reconnect' },
  })

  const stop = page.getByRole('button', { name: 'Stop' })
  await expect(stop).toBeVisible()

  await page.evaluate(() => (window as unknown as TestHooks).__yeetDisconnect())
  await stop.click()

  await expect.poll(async () => (await hellos(page)).length).toBeGreaterThanOrEqual(2)
  await expect.poll(async () =>
    (await sentCommands(page)).some((command) => command.type === 'interrupt')
  ).toBe(true)
})

test('independent tabs keep distinct Remote client identities', async ({ page, context }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  const firstClientID = (await resumeState(page)).clientId
  const second = await openSecondRemote(context)
  const secondClientID = (await resumeState(second)).clientId

  expect(firstClientID).toBeTruthy()
  expect(secondClientID).toBeTruthy()
  expect(secondClientID).not.toBe(firstClientID)

  await second.close()
})

test('permission prompts send semantic allow and deny commands', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: {
      pending_shell_permission: {
        id: 'shell-1',
        kind: 'shell',
        command: 'cargo test --all-targets',
        operation: 'execute',
        reason: 'Run verification',
      },
    },
  })

  let prompt = page.getByRole('alertdialog', { name: 'Shell permission requested' })
  await expect(prompt).toContainText('cargo test --all-targets')
  await expect(prompt.getByRole('button', { name: 'Deny' })).toBeFocused()
  await prompt.getByRole('button', { name: 'Allow' }).click()

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 3,
    revision: 3,
    patch: {
      pending_shell_permission: null,
      pending_native_app_permission: {
        id: 'native-1',
        server: 'Computer Use',
        tool: 'click',
        appName: 'Safari',
        operation: 'interact',
        reason: 'Use browser',
      },
    },
  })

  prompt = page.getByRole('alertdialog', { name: 'Native app permission requested' })
  await expect(prompt).toContainText('Safari · click')
  await prompt.getByRole('button', { name: 'Deny' }).click()

  await expect.poll(async () => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'resolve_permission', request_id: 'shell-1', granted: true }),
    expect.objectContaining({ type: 'resolve_permission', request_id: 'native-1', granted: false }),
  ]))
})


test('completed assistant text stays in turn order when the next run starts', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop')

  await emit(page, {
    type: 'assistant_delta',
    version: 1,
    sequence: 2,
    revision: 2,
    entry_id: 'assistant-1',
    delta: '',
    content: 'Previous final answer',
    reset: true,
  })
  await emit(page, {
    type: 'conversation_entry',
    version: 1,
    sequence: 3,
    revision: 3,
    entry: { id: 'assistant-1', kind: { type: 'assistant', content: 'Previous final answer' } },
  })
  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 4,
    revision: 4,
    patch: { is_streaming: false, active_assistant_entry_id: null, active_run_id: null },
  })

  await expect(page.getByText('Previous final answer', { exact: true })).toHaveCount(1)

  await emit(page, {
    type: 'conversation_entry',
    version: 1,
    sequence: 5,
    revision: 5,
    entry: { id: 'user-2', kind: { type: 'user', content: 'New user message' } },
  })
  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 6,
    revision: 6,
    patch: { is_streaming: true, active_run_id: 'run-2' },
  })

  await expect(page.getByText('Previous final answer', { exact: true })).toHaveCount(1)
  await expect(page.getByText('New user message', { exact: true })).toBeVisible()
  await expect(page.getByText('Preparing response', { exact: true })).toBeVisible()

  const transcriptText = await page.getByTestId('transcript').textContent()
  expect(transcriptText?.indexOf('Previous final answer')).toBeLessThan(transcriptText?.indexOf('New user message') ?? -1)
})
