import { computed, reactive, ref, shallowRef } from 'vue'
import { defineStore } from 'pinia'
import { RemoteTransport } from '@/remote/transport'
import {
  emptyBridgeState,
  type BridgeState,
  type ConnectionStatus,
  type ConversationEntry,
  type FrontendCommand,
  type RemoteServerMessage,
  type SandboxAction,
  type WorkspaceSummary,
} from '@/remote/protocol'

const mergeEntry = (target: ConversationEntry, next: ConversationEntry): void => {
  target.kind = next.kind
}

export const useRemoteStore = defineStore('remote', () => {
  const state = reactive<BridgeState>(emptyBridgeState())
  const entries = reactive<ConversationEntry[]>([])
  const connection = ref<ConnectionStatus>('connecting')
  const connectionError = ref<string | null>(null)
  const inspectorOpen = ref(false)
  const mobileSessionsOpen = ref(false)
  const mobileStatusOpen = ref(false)
  const transport = shallowRef<RemoteTransport | null>(null)
  const eventQueue: RemoteServerMessage[] = []
  let flushFrame = 0
  let initialized = false

  const currentSession = computed(() =>
    state.saved_sessions.find((session) => session.id === state.current_session_id) ?? null,
  )
  const knownWorkspaces = computed(() => state.known_workspaces)
  const permissionMode = computed(() => {
    const sandbox = state.sandbox_settings
    if (!sandbox) return 'unknown'
    if (sandbox.execution_mode === 'unlimited') return 'unlimited'
    return sandbox.auto_approve ? 'auto' : 'ask'
  })
  const contextPercent = computed(() => {
    const used = state.current_context_tokens
    const total = state.active_model_context_length
    if (!used || !total) return null
    return Math.min(100, Math.max(0, Math.round((used / total) * 100)))
  })
  const pendingPermission = computed(() => state.pending_shell_permission ?? state.pending_native_app_permission ?? null)

  function init(): void {
    if (initialized) return
    initialized = true
    const client = new RemoteTransport({
      onOpen: () => {
        connection.value = 'connected'
        connectionError.value = null
        for (const command of [
          { type: 'request_sessions' },
          { type: 'request_models' },
          { type: 'request_capabilities' },
          { type: 'request_auth' },
          { type: 'request_providers' },
          { type: 'request_settings' },
          { type: 'request_sandbox' },
        ] satisfies FrontendCommand[]) client.send(command)
      },
      onMessage: enqueue,
      onStatus: (next) => { connection.value = next },
      onError: (message) => { connectionError.value = message },
    })
    transport.value = client
    void client.connect()

    window.addEventListener('online', handleOnline)
    window.addEventListener('offline', handleOffline)
  }

  function destroy(): void {
    initialized = false
    transport.value?.close()
    transport.value = null
    window.removeEventListener('online', handleOnline)
    window.removeEventListener('offline', handleOffline)
  }

  function handleOnline(): void {
    if (connection.value === 'offline') transport.value?.reconnectAfterAuth()
  }

  function handleOffline(): void {
    connection.value = 'offline'
  }

  function reconnectAfterAuth(): void {
    transport.value?.reconnectAfterAuth()
  }

  function enqueue(message: RemoteServerMessage): void {
    eventQueue.push(message)
    if (flushFrame) return
    flushFrame = requestAnimationFrame(flushEvents)
  }

  function flushEvents(): void {
    flushFrame = 0
    const batch = eventQueue.splice(0)
    for (const message of batch) {
      applyMessage(message)
      transport.value?.markApplied(message)
    }
  }

  function applyMessage(message: RemoteServerMessage): void {
    switch (message.type) {
      case 'welcome':
        state.workspace_root = message.workspace
        if (message.session_id !== undefined) state.current_session_id = message.session_id
        return
      case 'snapshot':
        applyState(message.state)
        syncActiveStreamEntries()
        state.conversation_revision = message.revision
        return
      case 'state_update':
        applyState(message.patch)
        if (message.patch.is_streaming === false) clearStreamingEntries()
        state.conversation_revision = message.revision
        return
      case 'assistant_delta':
        markStreamingEntry(message.entry_id ?? null, 'assistant')
        state.active_assistant_entry_id = message.entry_id ?? null
        if (message.reset) state.active_assistant_text = message.content
        else state.active_assistant_text += message.delta
        applyAssistantStreamToEntry(message.entry_id ?? null, message.reset, message.content, message.delta)
        state.conversation_revision = message.revision
        state.is_streaming = true
        return
      case 'reasoning_delta':
        markStreamingEntry(message.entry_id ?? null, 'reasoning')
        state.active_reasoning_entry_id = message.entry_id ?? null
        if (message.summary) {
          if (message.reset) state.active_reasoning_summary = message.content
          else state.active_reasoning_summary += message.delta
        } else if (message.reset) {
          state.active_reasoning_text = message.content
        } else {
          state.active_reasoning_text += message.delta
        }
        applyReasoningStreamToEntry(message.entry_id ?? null, message.summary, message.reset, message.content, message.delta)
        state.conversation_revision = message.revision
        state.is_streaming = true
        return
      case 'conversation_entry':
        upsertEntry(message.entry)
        state.conversation_revision = message.revision
        return
      case 'conversation_reset':
        mergeConversation(message.conversation)
        state.conversation_revision = message.revision
        return
      case 'tool_update':
        upsertEntry(message.entry)
        state.conversation_revision = message.revision
        return
      case 'activity_update':
        upsertEntry(message.entry)
        state.conversation_revision = message.revision
        return
      case 'error':
        state.error_message = message.message
        return
      default:
        return
    }
  }

  function applyState(patch: Partial<BridgeState>): void {
    if ('conversation' in patch) {
      if (patch.conversation) mergeConversation(patch.conversation)
      else entries.splice(0, entries.length)
    }
    for (const [key, value] of Object.entries(patch)) {
      if (key === 'conversation' || value === undefined) continue
      ;(state as Record<string, unknown>)[key] = value
    }
  }

  function clearStreamingEntries(): void {
    for (const entry of entries) {
      if (entry.uiStreaming) entry.uiStreaming = false
    }
  }

  function markStreamingEntry(id: string | null, kind: 'assistant' | 'reasoning'): void {
    for (const entry of entries) {
      if (entry.uiStreaming && (entry.id !== id || entry.kind.type !== kind)) entry.uiStreaming = false
    }
    if (!id) return
    const entry = entries.find((candidate) => candidate.id === id && candidate.kind.type === kind)
    if (entry) entry.uiStreaming = true
  }

  function applyAssistantStreamToEntry(id: string | null, reset: boolean, content: string, delta: string): void {
    if (!id) return
    const entry = entries.find((candidate) => candidate.id === id)
    if (!entry || entry.kind.type !== 'assistant') return
    entry.kind.content = reset ? content : entry.kind.content + delta
    entry.uiStreaming = true
  }

  function applyReasoningStreamToEntry(id: string | null, summary: boolean, reset: boolean, content: string, delta: string): void {
    if (!id) return
    const entry = entries.find((candidate) => candidate.id === id)
    if (!entry || entry.kind.type !== 'reasoning') return
    if (summary) entry.kind.summary = reset ? content : (entry.kind.summary ?? '') + delta
    else entry.kind.content = reset ? content : entry.kind.content + delta
    entry.uiStreaming = true
  }

  function syncActiveStreamEntries(): void {
    clearStreamingEntries()
    if (!state.is_streaming) return
    if (state.active_assistant_entry_id) {
      const entry = entries.find((candidate) => candidate.id === state.active_assistant_entry_id)
      if (entry && entry.kind.type === 'assistant') {
        if (state.active_assistant_text) entry.kind.content = state.active_assistant_text
        entry.uiStreaming = true
      }
    }
    if (state.active_reasoning_entry_id) {
      const entry = entries.find((candidate) => candidate.id === state.active_reasoning_entry_id)
      if (entry && entry.kind.type === 'reasoning') {
        if (state.active_reasoning_text) entry.kind.content = state.active_reasoning_text
        if (state.active_reasoning_summary) entry.kind.summary = state.active_reasoning_summary
        entry.uiStreaming = true
      }
    }
  }

  function mergeConversation(nextEntries: ConversationEntry[]): void {
    const existing = new Map(entries.map((entry) => [entry.id, entry]))
    const ordered: ConversationEntry[] = []
    for (const next of nextEntries) {
      const current = existing.get(next.id)
      if (current) {
        mergeEntry(current, next)
        ordered.push(current)
      } else {
        ordered.push(reactive(next) as ConversationEntry)
      }
    }
    entries.splice(0, entries.length, ...ordered)
  }

  function upsertEntry(next: ConversationEntry): void {
    const current = entries.find((entry) => entry.id === next.id)
    if (current) mergeEntry(current, next)
    else entries.push(reactive(next) as ConversationEntry)
    const entry = current ?? entries.at(-1)
    if (!entry) return
    if (entry.id === state.active_assistant_entry_id && entry.kind.type === 'assistant') {
      if (state.active_assistant_text) entry.kind.content = state.active_assistant_text
      entry.uiStreaming = state.is_streaming
    } else if (entry.id === state.active_reasoning_entry_id && entry.kind.type === 'reasoning') {
      if (state.active_reasoning_text) entry.kind.content = state.active_reasoning_text
      if (state.active_reasoning_summary) entry.kind.summary = state.active_reasoning_summary
      entry.uiStreaming = state.is_streaming
    }
  }

  function send(command: FrontendCommand): boolean {
    connectionError.value = null
    const sent = transport.value?.send(command) ?? false
    if (!sent && connection.value === 'connected') connection.value = 'reconnecting'
    return sent
  }

  const submit = (text: string) => send({ type: 'submit', text })
  const interrupt = () => send({ type: 'interrupt' })
  const requestModels = () => send({ type: 'request_models' })
  const selectModel = (model: string) => send({ type: 'select_model', model })
  const selectReasoning = (level: string) => send({ type: 'select_reasoning', level })
  const setInfinity = (enabled: boolean) => send({ type: 'set_infinity', enabled })
  const loadSession = (session_id: string) => send({ type: 'load_session', session_id })
  const newSession = () => send({ type: 'new_session' })
  const toggleCapability = (id: string) => send({ type: 'toggle_capability', id })
  const setFoundationMemory = (enabled: boolean) => send({ type: 'set_foundation_memory', enabled })
  const setOpenAiFlex = (enabled: boolean) => send({ type: 'set_open_ai_flex', enabled })
  const updateSandbox = (action: SandboxAction) => send({ type: 'update_sandbox', action })
  const allowPermission = () => send(state.pending_shell_permission ? { type: 'allow_shell' } : { type: 'allow_native_app' })
  const denyPermission = () => send(state.pending_shell_permission ? { type: 'deny_shell' } : { type: 'deny_native_app' })
  const switchWorkspace = (workspace: string) => {
    const next = workspace.trim()
    if (!next) return
    Object.assign(state, emptyBridgeState())
    state.workspace_root = next
    entries.splice(0, entries.length)
    connectionError.value = null
    connection.value = 'connecting'
    transport.value?.switchWorkspace(next)
  }
  const selectWorkspace = (workspace: WorkspaceSummary) => switchWorkspace(workspace.path)

  return {
    state,
    entries,
    connection,
    connectionError,
    currentSession,
    knownWorkspaces,
    permissionMode,
    contextPercent,
    pendingPermission,
    inspectorOpen,
    mobileSessionsOpen,
    mobileStatusOpen,
    init,
    destroy,
    reconnectAfterAuth,
    send,
    submit,
    interrupt,
    requestModels,
    selectModel,
    selectReasoning,
    setInfinity,
    loadSession,
    newSession,
    toggleCapability,
    setFoundationMemory,
    setOpenAiFlex,
    updateSandbox,
    allowPermission,
    denyPermission,
    switchWorkspace,
    selectWorkspace,
  }
})
