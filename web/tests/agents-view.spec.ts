import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
  __yeetSent: Record<string, unknown>[]
}

async function emit(page: Page, message: Record<string, unknown>) {
  await page.evaluate((payload) => {
    ;(window as unknown as TestHooks).__yeetEmit(payload)
  }, message)
}

const isDesktop = (page: Page) => page.evaluate(() =>
  matchMedia('(min-width: 1000px) and (hover: hover) and (pointer: fine)').matches)

/** Desktop shows Agents as a page; compact layouts keep the modal sheet. */
async function openAgents(page: Page) {
  if (await isDesktop(page)) {
    await page.getByRole('button', { name: /^Agents/ }).click()
    return page.getByRole('region', { name: 'Agent Group' })
  }
  await page.getByRole('button', { name: /Open sidebar/ }).click()
  await page.getByRole('button', { name: /^Agents/ }).click()
  return page.getByRole('dialog', { name: 'Agent Group' })
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('shows the group objective, attributed member events, shared findings and result', async ({ page }) => {
  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: {
      agent_group: {
        groupId: 'group-1',
        objective: 'Review the candidate models and return a recommendation.',
        status: 'running',
        finalResult: 'Model B best satisfies the constraints.',
        checkpointSummary: null,
        members: [{
          id: 'member-1', description: 'Compare model latency', role: 'researcher', model: 'gpt-6-luna',
          status: 'running', activityState: 'reasoning', taskStatus: 'running', summary: 'Collecting latency measurements.',
          startedAt: new Date().toISOString(), inputTokens: 1200, outputTokens: 300,
        }],
        activity: [],
        startedAt: new Date().toISOString(),
        inputTokens: 3200,
        outputTokens: 800,
        events: [
          { groupId: 'group-1', sequence: 1, memberId: null, taskId: null, at: new Date().toISOString(), kind: 'group_started', detail: 'Coordinator started.' },
          { groupId: 'group-1', sequence: 2, memberId: 'member-1', taskId: 'task-1', at: new Date().toISOString(), kind: 'member_reasoning', detail: 'Comparing measured latency across providers.' },
        ],
        sharedFindings: [{ memberId: 'member-1', taskId: 'task-1', at: new Date().toISOString(), summary: 'Model B has the lowest p95 latency.' }],
      },
      agent_tasks: [{ id: 'task-1', role: 'researcher', objective: 'Compare model latency', status: 'running' }],
    },
  })

  const dialog = await openAgents(page)
  await expect(dialog).toContainText('Review the candidate models and return a recommendation.')
  await expect(dialog).toContainText('Model B best satisfies the constraints.')
  await expect(dialog.getByRole('list', { name: 'Group activity events' })).toContainText('Comparing measured latency across providers.')
  await expect(dialog.getByRole('list', { name: 'Group activity events' })).toContainText('Compare model latency')
  await expect(dialog).toContainText('Model B has the lowest p95 latency.')

  await dialog.getByRole('button', { name: /Compare model latency/ }).click()
  await expect(dialog.getByRole('list', { name: 'Group activity events' })).toContainText('Comparing measured latency across providers.')

  await dialog.getByRole('button', { name: 'Cancel group' }).click()
  await dialog.getByRole('button', { name: 'Stop group' }).click()
  await expect.poll(() => page.evaluate(() => {
    const sent = (window as unknown as TestHooks).__yeetSent
    return ['cancel_agent_group', 'stop_agent_group'].every(type => sent.some(message => {
      const command = message.command as { type?: string; group_id?: string } | undefined
      return command?.type === type && command.group_id === 'group-1'
    }))
  })).toBe(true)

})

test('creates a shared group objective through the group lifecycle', async ({ page }) => {
  const dialog = await openAgents(page)
  await expect(dialog).toContainText('No Agent Group yet')
  await dialog.getByRole('button', { name: 'Create group' }).first().click()
  await dialog.getByLabel(/objective/i).fill('Compare two migration options and synthesize the tradeoffs.')
  await dialog.getByRole('button', { name: 'Create group' }).last().click()

  await expect.poll(() => page.evaluate(() => (window as unknown as TestHooks).__yeetSent.some(message => {
    const command = message.command as { type?: string; objective?: string } | undefined
    return command?.type === 'create_agent_group'
      && command.objective === 'Compare two migration options and synthesize the tradeoffs.'
  }))).toBe(true)
  const sent = await page.evaluate(() => (window as unknown as TestHooks).__yeetSent)
  expect(sent.some(message => (message.command as { type?: string } | undefined)?.type === 'spawn_agent')).toBe(false)

})

test('resumes a paused group using its stable group identity', async ({ page }) => {
  await emit(page, {
    type: 'state_update', version: 1, sequence: 2, revision: 2,
    patch: {
      agent_group: {
        groupId: 'group-paused', objective: 'Continue the saved analysis.', status: 'paused',
        members: [], activity: [], inputTokens: 0, outputTokens: 0,
        events: [], sharedFindings: [],
      },
    },
  })
  const dialog = await openAgents(page)
  await expect(dialog.getByRole('button', { name: 'New group' })).toHaveCount(0)
  await dialog.getByRole('button', { name: 'Resume group' }).click()

  await expect.poll(() => page.evaluate(() => (window as unknown as TestHooks).__yeetSent.some(message => {
    const command = message.command as { type?: string; group_id?: string } | undefined
    return command?.type === 'resume_agent_group' && command.group_id === 'group-paused'
  }))).toBe(true)

})
