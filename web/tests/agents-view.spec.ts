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

async function openAgents(page: Page) {
  await page.getByRole('button', { name: /Open sidebar/ }).click()
  await page.getByRole('button', { name: /Open Agents/ }).click()
  return page.getByRole('dialog', { name: 'Agent Group' })
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('shows the group objective, attributed member events, shared findings, result and separate budgets', async ({ page }) => {
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
        estimatedCostUsd: 0.08,
        budget: {
          outputLimitTokens: 12000, outputUsedTokens: 800, costLimitUsd: 2,
          estimatedCostUsedUsd: 0.08, coordinationReserveTokens: 900,
          synthesisReserveTokens: 1200, coordinationReserveCostUsd: 0.2,
          synthesisReserveCostUsd: 0.3,
          tasks: [{
            taskId: 'task-1', allocatedOutputTokens: 3000, usedOutputTokens: 800,
            remainingOutputTokens: 2200, allocatedCostUsd: 0.6,
            estimatedCostUsedUsd: 0.08, contextWindowTokens: 64000,
          }],
        },
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
  await expect(dialog).toContainText('800 / 12.0K')
  await expect(dialog).toContainText('$0.08 / $2.00')
  await expect(dialog).toContainText('64.0K context')

  await dialog.getByRole('button', { name: /Compare model latency/ }).click()
  await expect(dialog).toContainText('Filtered to Compare model latency')
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
  await dialog.getByLabel('Shared objective').fill('Compare two migration options and synthesize the tradeoffs.')
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
        budget: {
          outputLimitTokens: 0, outputUsedTokens: 0, costLimitUsd: 0,
          coordinationReserveTokens: 0, synthesisReserveTokens: 0,
          coordinationReserveCostUsd: 0, synthesisReserveCostUsd: 0, tasks: [],
        },
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
