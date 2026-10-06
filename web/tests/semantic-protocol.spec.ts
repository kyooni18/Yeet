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

test('Home projections use their own revision and reset on welcome', async () => {
  const { RemoteStore } = await import('../../frontend/shared/state/remoteStore')
  const view: import('../src/remote/protocol').HomeView = {
    overview_label: 'Overview',
    summary: 'Workspace  ·  local  ·  working tree clean',
    recent: [],
    activity: [{ type: 'heading', value: 'Recent views' }],
    selected: null,
    inspector: null,
    open_label: 'Open view →',
    new_session_label: '+  New session',
    recent_empty: 'No recent objects',
    inspector_empty: 'Select a session, recent view, task or changed file.',
    usage_label: 'Usage details →',
  }
  const action: import('../src/remote/protocol').HomeAction = {
    type: 'open', value: { type: 'session', value: 'session-a' },
  }
  let events!: import('../../frontend/shared/remote/transport').RemoteTransportEvents
  let frame!: () => void
  let sentHome: typeof action | null = null
  const store = new RemoteStore({
    createTransport: handlers => {
      events = handlers
      return {
        connect: async () => {}, close: () => {}, send: () => true,
        sendHomeUi: next => { sentHome = next; return true },
        markApplied: () => {}, switchWorkspace: () => {}, reconnectAfterAuth: () => {},
      }
    },
    scheduleFrame: callback => { frame = callback; return 1 },
    cancelFrame: () => {},
  })
  store.init()
  events.onMessage({ type: 'ui_home', version: 1, home_revision: 4, view })
  events.onMessage({ type: 'ui_home', version: 1, home_revision: 3, view: { ...view, summary: 'stale' } })
  frame()
  expect(store.getSnapshot().home?.summary).toBe(view.summary)
  expect(store.sendHomeUi(action)).toBe(true)
  expect(sentHome).toEqual(action)

  events.onMessage({ type: 'welcome', version: 1, client_id: 'client', workspace: '/workspace', sequence: 0, revision: 0, resumed: true })
  frame()
  expect(store.getSnapshot().home).toBeNull()
  events.onMessage({ type: 'ui_home', version: 1, home_revision: 0, view })
  frame()
  expect(store.getSnapshot().home).toEqual(view)
  store.destroy()
})

test('Home actions use the revision-independent ui_home_action wire envelope', async () => {
  const { RemoteTransport } = await import('../../frontend/shared/remote/transport')
  const sent: unknown[] = []
  const listeners = new Map<string, ((event?: { data: unknown }) => void)[]>()
  const socket = {
    readyState: 0,
    send(data: string) { sent.push(JSON.parse(data)) },
    close() {},
    addEventListener(type: string, listener: (event?: { data: unknown }) => void) {
      listeners.set(type, [...(listeners.get(type) ?? []), listener])
    },
  }
  const transport = new RemoteTransport({
    onOpen: () => {}, onMessage: () => {}, onStatus: () => {}, onError: () => {},
  }, {
    request: async <T,>(path: string) => (path.endsWith('/auth/status')
      ? { required: false, authenticated: true }
      : { minVersion: 1, maxVersion: 1, websocket: '/api/ws' }) as T,
    websocketUrl: path => `ws://example.test${path}`,
    createSocket: () => socket as unknown as import('../../frontend/shared/remote/transport').RemoteSocket,
    storage: { getItem: () => null, setItem: () => {} },
    isOnline: () => true,
    now: () => 1,
    requestId: () => 'home-request',
    setTimeout: () => 1,
    clearTimeout: () => {},
    setInterval: () => 1,
    clearInterval: () => {},
  })
  await transport.connect()
  socket.readyState = 1
  listeners.get('open')?.forEach(listener => listener())
  listeners.get('message')?.forEach(listener => listener({ data: JSON.stringify({
    type: 'welcome', version: 1, client_id: 'client', workspace: '/workspace',
    sequence: 0, revision: 0, resumed: true,
  }) }))

  const action: import('../src/remote/protocol').HomeAction = {
    type: 'select', value: { type: 'file', value: '/workspace/readme.md' },
  }
  expect(transport.sendHomeUi(action)).toBe(true)
  expect(sent.at(-1)).toEqual({
    type: 'ui_home_action', version: 1, request_id: 'home-request', action,
  })
  transport.close()
})

test('Home browser store consumes production Rust projection and one-shot open intent', async () => {
  const { projectHome } = await import('./uiHomeHarness')
  const { RemoteStore } = await import('../../frontend/shared/state/remoteStore')
  const sessions = [{ id: 'session-a', title: 'First session' }, { id: 'session-b', title: 'Second session' }]
  const selected = await projectHome({ sessions, action: { type: 'select', value: { type: 'session', value: 'session-b' } } })
  const view = selected.view as import('../src/remote/protocol').HomeView
  expect(view.selected).toEqual({ type: 'session', value: 'session-b' })
  expect((await projectHome({ sessions, action: { type: 'select', value: { type: 'session', value: 'session-b' } }, deliver: false })).view)
    .toHaveProperty('selected', { type: 'session', value: 'session-a' })
  const opened = await projectHome({ sessions, action: { type: 'open', value: { type: 'session', value: 'session-b' } } })
  expect(opened.open).toEqual({ type: 'session', value: 'session-b' })
  expect((await projectHome({ sessions, action: { type: 'open', value: { type: 'session', value: 'missing' } } })).open).toBeNull()

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
  events.onMessage({ type: 'ui_home', version: 1, home_revision: selected.home_revision as number, view })
  events.onMessage({ type: 'ui_home_effect', version: 1, open: opened.open as import('../src/remote/protocol').ResourceTarget })
  frame()
  expect(store.getSnapshot().home?.selected).toEqual({ type: 'session', value: 'session-b' })
  const effect = store.getSnapshot().homeEffect
  expect(effect?.open).toEqual({ type: 'session', value: 'session-b' })
  expect(store.consumeHomeEffect(effect!.id)).toEqual(effect?.open)
  expect(store.consumeHomeEffect(effect!.id)).toBeNull()
  store.destroy()
})
