import { expect, test } from '@playwright/test'
import {
  REMOTE_PROTOCOL_VERSION,
  emptyBridgeState,
  type ModelCatalogItem,
  type WorkspaceSessionGroup,
  type WorkspaceSummary,
} from '../src/remote/protocol'

test('protocol v1 semantic catalogs default cleanly and carry structured identity', () => {
  const state = emptyBridgeState()
  expect(REMOTE_PROTOCOL_VERSION).toBe(1)
  expect(state.model_catalog).toEqual([])
  expect(state.known_workspaces).toEqual([])
  expect(state.workspace_session_groups).toEqual([])

  const model: ModelCatalogItem = {
    id: 'openai/gpt-5.6-sol',
    provider: 'openai',
    model: 'gpt-5.6-sol',
    context_length: 128_000,
  }
  const workspace: WorkspaceSummary = {
    id: '/tmp/project',
    path: '/tmp/project',
    display_name: 'project',
    updated_at: '2026-09-10T00:00:00Z',
    session_count: 1,
    is_current: true,
  }
  const sessionGroup: WorkspaceSessionGroup = {
    workspace_id: workspace.id,
    sessions: [{
      id: 'session-1',
      title: 'Session',
      updated_at: '2026-09-10T00:00:00Z',
      model: model.id,
      message_count: 3,
    }],
  }

  state.model_catalog = [model]
  state.known_workspaces = [workspace]
  state.workspace_session_groups = [sessionGroup]
  expect(state.model_catalog[0].provider).toBe('openai')
  expect(state.workspace_session_groups[0].sessions[0].id).toBe('session-1')
})

// Acceptance and runtime refresh commonly arrive before the next paint.
test('agent acceptance survives a batched refresh without replaying across reconnect', async () => {
  const { RemoteStore } = await import('../../frontend/shared/state/remoteStore')
  const { projectAgents } = await import('./uiAgentsHarness')
  const projected = await projectAgents({})
  const view = projected.view as import('../../frontend/shared/remote/protocol').AgentsView
  let events!: import('../../frontend/shared/remote/transport').RemoteTransportEvents
  let frame!: () => void
  const store = new RemoteStore({
    createTransport: handlers => {
      events = handlers
      return { connect: async () => {}, close: () => {}, send: () => true,
        markApplied: () => {}, switchWorkspace: () => {}, reconnectAfterAuth: () => {} }
    },
    scheduleFrame: callback => { frame = callback; return 1 },
    cancelFrame: () => {},
  })
  store.init()
  events.onMessage({ type: 'ui_agents', version: 1, agents_revision: 1, view,
    effect: { submitted_text: 'accepted objective', open_group_settings: false } })
  events.onMessage({ type: 'ui_agents', version: 1, agents_revision: 2, view, effect: null })
  frame()
  expect(store.getSnapshot().agentEffect?.value.submitted_text).toBe('accepted objective')
  events.onMessage({ type: 'ui_agents', version: 1, agents_revision: 0, view,
    effect: { submitted_text: 'stale acceptance', open_group_settings: false } })
  frame()
  expect(store.getSnapshot().agentEffect?.value.submitted_text).toBe('accepted objective')
  events.onMessage({ type: 'welcome', version: 1, client_id: 'client', workspace: '/workspace',
    sequence: 0, revision: 0, resumed: true })
  events.onMessage({ type: 'ui_agents', version: 1, agents_revision: 0, view, effect: null })
  frame()
  expect(store.getSnapshot().agentEffect).toBeNull()
  store.destroy()
})
