import { useSyncExternalStore } from 'react'
import { RemoteTransport } from '@/remote/transport'
import {
  emptyBridgeState,
  type AuthProviderItem,
  type BridgeState,
  type ConnectionStatus,
  type ConversationEntry,
  type FrontendCommand,
  type ProviderUsageStatus,
  type SandboxAction,
  type SessionSummary,
  type WorkspaceSummary,
} from '@/remote/protocol'

export interface RemoteSnapshot {
  state: BridgeState
  entries: ConversationEntry[]
  connection: ConnectionStatus
  connectionError: string | null
  currentSession: SessionSummary | null
  currentWorkspace: WorkspaceSummary | null
  currentWorkspaceSessions: SessionSummary[]
  contextPercent: number | null
  activeProviderUsage: ProviderUsageStatus | null
  activeProvider: string | null
  revision: number
  sessionResetRevision: number
}

const providerUsageIdentity = (provider: string): string => {
  switch (provider.trim().toLowerCase()) {
    case 'codex':
    case 'codex-cli':
    case 'chatgpt':
      return 'codex-cli'
    case 'openai':
    case 'openai-chat':
    case 'openai-responses':
      return 'openai'
    case 'anthropic':
    case 'claude':
      return 'anthropic'
    case 'gemini':
    case 'google':
    case 'google-ai':
    case 'googleai':
      return 'gemini'
    case 'opencode-go':
      return 'opencode'
    default:
      return provider.trim().toLowerCase()
  }
}

const providerForModel = (state: BridgeState): string | null => {
  const active = state.active_model.trim()
  if (!active) return null

  const exact = state.model_catalog.find((item) => item.id === active)
  if (exact?.provider) return exact.provider

  const byModel = state.model_catalog.filter((item) => item.model === active)
  if (byModel.length === 1 && byModel[0].provider) return byModel[0].provider

  if (state.model_catalog.length) return null

  const value = active.toLowerCase()
  if (value.includes('claude')) return 'anthropic'
  if (value.includes('gemini')) return 'gemini'
  if (value.includes('codex')) return 'codex-cli'
  if (value.includes('gpt') || value.includes('o1') || value.includes('o3')) return 'openai'
  if (value.includes('grok')) return 'xai'
  if (value.includes('mistral')) return 'mistral'
  if (value.includes('deepseek')) return 'deepseek'
  if (value.includes('llama')) return 'meta'
  return null
}

const findActiveProviderUsage = (state: BridgeState): ProviderUsageStatus | null => {
  const selectedProvider = providerForModel(state)
  if (!selectedProvider) return null

  const identity = providerUsageIdentity(selectedProvider)
  const match = state.auth_providers.find(
    (item) => providerUsageIdentity(item.provider) === identity,
  )
  const usage = match?.usage
  return usage?.available ? usage : null
}

class RemoteStore {
  private state = emptyBridgeState()
  private entries: ConversationEntry[] = []
  private connection: ConnectionStatus = 'connecting'
  private connectionError: string | null = null
  private transport: RemoteTransport | null = null
  private listeners = new Set<() => void>()
  private initialized = false
  private revision = 0
  private sessionResetRevision = 0
  private eventQueue: Parameters<RemoteTransport['markApplied']>[0][] = []
  private flushFrame = 0
  private snapshot = this.makeSnapshot()

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  getSnapshot = (): RemoteSnapshot => this.snapshot
  getServerSnapshot = (): RemoteSnapshot => this.snapshot

  private makeSnapshot(): RemoteSnapshot {
    const currentSession = this.state.saved_sessions.find((session) => session.id === this.state.current_session_id) ?? null
    const currentWorkspace = this.state.known_workspaces.find(
      (workspace) => workspace.path === this.state.workspace_root || workspace.id === this.state.workspace_root,
    ) ?? this.state.known_workspaces.find((workspace) => workspace.is_current) ?? null
    const group = currentWorkspace
      ? this.state.workspace_session_groups.find((candidate) => candidate.workspace_id === currentWorkspace.id)
      : null
    const currentWorkspaceSessions = group?.sessions ?? this.state.saved_sessions
    const used = this.state.current_context_tokens
    const total = this.state.active_model_context_length
    const contextPercent = typeof used === 'number' && typeof total === 'number' && total > 0
      ? Math.min(100, Math.max(0, Math.round((used / total) * 100)))
      : null
    return {
      state: this.state,
      entries: this.entries,
      connection: this.connection,
      connectionError: this.connectionError,
      currentSession,
      currentWorkspace,
      currentWorkspaceSessions,
      contextPercent,
      activeProviderUsage: findActiveProviderUsage(this.state),
      activeProvider: providerForModel(this.state),
      revision: this.revision,
      sessionResetRevision: this.sessionResetRevision,
    }
  }

  private emit(): void {
    this.revision += 1
    this.snapshot = this.makeSnapshot()
    for (const listener of this.listeners) listener()
  }

  init(): void {
    if (this.initialized) return
    this.initialized = true
    this.transport = new RemoteTransport({
      onOpen: () => {
        this.connection = 'connected'
        this.connectionError = null
        this.requestRemoteState()
        this.emit()
      },
      onMessage: (message) => this.enqueue(message),
      onStatus: (status) => {
        this.connection = status
        this.emit()
      },
      onError: (message) => {
        this.connectionError = message
        this.emit()
      },
    })
    void this.transport.connect()
    window.addEventListener('online', this.handleOnline)
    window.addEventListener('offline', this.handleOffline)
  }

  destroy(): void {
    if (!this.initialized) return
    this.initialized = false
    if (this.flushFrame) cancelAnimationFrame(this.flushFrame)
    this.flushFrame = 0
    this.eventQueue = []
    this.transport?.close()
    this.transport = null
    window.removeEventListener('online', this.handleOnline)
    window.removeEventListener('offline', this.handleOffline)
  }

  private handleOnline = (): void => {
    if (this.connection === 'offline') this.transport?.reconnectAfterAuth()
  }

  private handleOffline = (): void => {
    this.connection = 'offline'
    this.emit()
  }

  private enqueue(message: Parameters<RemoteTransport['markApplied']>[0]): void {
    this.eventQueue.push(message)
    if (this.flushFrame) return
    this.flushFrame = requestAnimationFrame(() => this.flushEvents())
  }

  private flushEvents(): void {
    this.flushFrame = 0
    const batch = this.eventQueue.splice(0)
    for (const message of batch) {
      this.applyMessage(message)
      this.transport?.markApplied(message)
    }
    if (batch.length) this.emit()
  }

  private applyMessage(message: Parameters<RemoteTransport['markApplied']>[0]): void {
    switch (message.type) {
      case 'welcome':
        this.state.workspace_root = message.workspace
        if (message.session_id !== undefined) this.state.current_session_id = message.session_id
        return
      case 'snapshot':
        this.applyState(message.state)
        this.syncActiveStreams()
        this.state.conversation_revision = message.revision
        return
      case 'state_update':
        this.applyState(message.patch)
        if (message.patch.is_streaming === false) {
          this.clearStreamingEntries()
          this.clearActiveStreams()
        }
        this.state.conversation_revision = message.revision
        return
      case 'assistant_delta':
        this.markStreamingEntry(message.entry_id ?? null, 'assistant')
        this.state.active_assistant_entry_id = message.entry_id ?? null
        this.state.active_assistant_text = message.reset
          ? message.content
          : this.state.active_assistant_text + message.delta
        this.applyAssistantDelta(message.entry_id ?? null, message.reset, message.content, message.delta)
        this.state.conversation_revision = message.revision
        this.state.is_streaming = true
        return
      case 'reasoning_delta':
        this.markStreamingEntry(message.entry_id ?? null, 'reasoning')
        this.state.active_reasoning_entry_id = message.entry_id ?? null
        if (message.summary) {
          this.state.active_reasoning_summary = message.reset
            ? message.content
            : this.state.active_reasoning_summary + message.delta
        } else {
          this.state.active_reasoning_text = message.reset
            ? message.content
            : this.state.active_reasoning_text + message.delta
        }
        this.applyReasoningDelta(
          message.entry_id ?? null,
          message.summary,
          message.reset,
          message.content,
          message.delta,
        )
        this.state.conversation_revision = message.revision
        this.state.is_streaming = true
        return
      case 'conversation_entry':
      case 'tool_update':
      case 'activity_update':
        this.upsertEntry(message.entry)
        this.state.conversation_revision = message.revision
        return
      case 'conversation_reset':
        this.mergeConversation(message.conversation)
        this.state.conversation_revision = message.revision
        return
      case 'error':
        this.state.error_message = message.message
        return
      default:
        return
    }
  }

  private applyState(patch: Partial<BridgeState>): void {
    if ('conversation' in patch) {
      if (patch.conversation) this.mergeConversation(patch.conversation)
      else this.entries = []
    }
    for (const [key, value] of Object.entries(patch)) {
      if (key === 'conversation' || value === undefined) continue
      ;(this.state as unknown as Record<string, unknown>)[key] = value
    }
  }

  private mergeConversation(next: ConversationEntry[]): void {
    const existing = new Map(this.entries.map((entry) => [entry.id, entry]))
    this.entries = next.map((entry) => {
      const current = existing.get(entry.id)
      if (!current) return { ...entry, kind: { ...entry.kind } } as ConversationEntry
      current.kind = { ...entry.kind } as ConversationEntry['kind']
      if (entry.uiStreaming !== undefined) current.uiStreaming = entry.uiStreaming
      return current
    })
  }

  private upsertEntry(next: ConversationEntry): void {
    const current = this.entries.find((entry) => entry.id === next.id)
    if (current) {
      current.kind = { ...next.kind } as ConversationEntry['kind']
    } else {
      this.entries = [...this.entries, { ...next, kind: { ...next.kind } } as ConversationEntry]
    }
    const entry = current ?? this.entries.at(-1)
    if (!entry) return
    if (entry.id === this.state.active_assistant_entry_id && entry.kind.type === 'assistant') {
      if (this.state.active_assistant_text) entry.kind.content = this.state.active_assistant_text
      entry.uiStreaming = this.state.is_streaming
    }
    if (entry.id === this.state.active_reasoning_entry_id && entry.kind.type === 'reasoning') {
      if (this.state.active_reasoning_text) entry.kind.content = this.state.active_reasoning_text
      if (this.state.active_reasoning_summary) entry.kind.summary = this.state.active_reasoning_summary
      entry.uiStreaming = this.state.is_streaming
    }
  }

  private clearStreamingEntries(): void {
    for (const entry of this.entries) entry.uiStreaming = false
  }


  private clearActiveStreams(): void {
    this.state.active_assistant_entry_id = null
    this.state.active_assistant_text = ''
    this.state.active_reasoning_entry_id = null
    this.state.active_reasoning_text = ''
    this.state.active_reasoning_summary = ''
  }

  private markStreamingEntry(id: string | null, kind: 'assistant' | 'reasoning'): void {
    for (const entry of this.entries) {
      if (entry.uiStreaming && (entry.id !== id || entry.kind.type !== kind)) entry.uiStreaming = false
    }
    if (!id) return
    const entry = this.entries.find((candidate) => candidate.id === id && candidate.kind.type === kind)
    if (entry) entry.uiStreaming = true
  }

  private applyAssistantDelta(id: string | null, reset: boolean, content: string, delta: string): void {
    if (!id) return
    const entry = this.entries.find((candidate) => candidate.id === id)
    if (!entry || entry.kind.type !== 'assistant') return
    entry.kind.content = reset ? content : entry.kind.content + delta
    entry.uiStreaming = true
  }

  private applyReasoningDelta(
    id: string | null,
    summary: boolean,
    reset: boolean,
    content: string,
    delta: string,
  ): void {
    if (!id) return
    const entry = this.entries.find((candidate) => candidate.id === id)
    if (!entry || entry.kind.type !== 'reasoning') return
    if (summary) entry.kind.summary = reset ? content : (entry.kind.summary ?? '') + delta
    else entry.kind.content = reset ? content : entry.kind.content + delta
    entry.uiStreaming = true
  }

  private syncActiveStreams(): void {
    this.clearStreamingEntries()
    if (!this.state.is_streaming) return
    if (this.state.active_assistant_entry_id) {
      const entry = this.entries.find((candidate) => candidate.id === this.state.active_assistant_entry_id)
      if (entry?.kind.type === 'assistant') {
        if (this.state.active_assistant_text) entry.kind.content = this.state.active_assistant_text
        entry.uiStreaming = true
      }
    }
    if (this.state.active_reasoning_entry_id) {
      const entry = this.entries.find((candidate) => candidate.id === this.state.active_reasoning_entry_id)
      if (entry?.kind.type === 'reasoning') {
        if (this.state.active_reasoning_text) entry.kind.content = this.state.active_reasoning_text
        if (this.state.active_reasoning_summary) entry.kind.summary = this.state.active_reasoning_summary
        entry.uiStreaming = true
      }
    }
  }

  send(command: FrontendCommand): boolean {
    this.connectionError = null
    const sent = this.transport?.send(command) ?? false
    if (!sent && this.connection === 'connected') this.connection = 'reconnecting'
    this.emit()
    return sent
  }

  requestRemoteState(): void {
    for (const command of [
      { type: 'request_sessions' },
      { type: 'request_models' },
      { type: 'request_capabilities' },
      { type: 'request_auth' },
      { type: 'request_providers' },
      { type: 'request_settings' },
      { type: 'request_sandbox' },
    ] satisfies FrontendCommand[]) this.transport?.send(command)
  }

  refreshProviderUsage(): void {
    this.send({ type: 'request_auth' })
  }

  reconnectAfterAuth(): void {
    this.transport?.reconnectAfterAuth()
  }

  submit(text: string, attachmentIds?: string[]): boolean {
    return this.send({ type: 'submit', text, attachment_ids: attachmentIds })
  }

  interrupt(): boolean {
    return this.send({ type: 'interrupt' })
  }

  regenerateLast(): boolean {
    return this.send({ type: 'regenerate_last' })
  }

  editLast(text: string): boolean {
    return this.send({ type: 'edit_last', text })
  }

  selectModel(model: string): boolean {
    return this.send({ type: 'select_model', model })
  }

  selectReasoning(level: string): boolean {
    return this.send({ type: 'select_reasoning', level })
  }

  setGoal(enabled: boolean): boolean {
    return this.send({ type: 'set_goal', enabled })
  }

  loadSession(sessionId: string): boolean {
    return this.send({ type: 'load_session', session_id: sessionId })
  }

  newSession(): boolean {
    const sent = this.send({ type: 'new_session' })
    if (sent) {
      this.sessionResetRevision += 1
      this.emit()
    }
    return sent
  }

  toggleCapability(id: string): boolean {
    return this.send({ type: 'toggle_capability', id })
  }

  updateSandbox(action: SandboxAction): boolean {
    return this.send({ type: 'update_sandbox', action })
  }

  allowPermission(): boolean {
    return this.send(this.state.pending_shell_permission ? { type: 'allow_shell' } : { type: 'allow_native_app' })
  }

  denyPermission(): boolean {
    return this.send(this.state.pending_shell_permission ? { type: 'deny_shell' } : { type: 'deny_native_app' })
  }

  switchWorkspace(workspace: string): void {
    const next = workspace.trim()
    if (!next) return
    this.state = emptyBridgeState()
    this.state.workspace_root = next
    this.entries = []
    this.connectionError = null
    this.connection = 'connecting'
    this.transport?.switchWorkspace(next)
    this.emit()
  }

  setAppearance(appearance: string): boolean {
    return this.send({ type: 'set_appearance', appearance })
  }

  setOpenAiFlex(enabled: boolean): boolean {
    return this.send({ type: 'set_open_ai_flex', enabled })
  }

  setFoundationMemory(enabled: boolean): boolean {
    return this.send({ type: 'set_foundation_memory', enabled })
  }

  authLogin(provider: string): boolean {
    return this.send({ type: 'auth_login', provider })
  }

  authLogout(provider: string): boolean {
    return this.send({ type: 'auth_logout', provider })
  }

  authSetApiKey(provider: string, key: string): boolean {
    return this.send({ type: 'auth_set_api_key', provider, key })
  }

  getProvider(provider: string): AuthProviderItem | null {
    return this.state.auth_providers.find((item) => item.provider.toLowerCase() === provider.toLowerCase()) ?? null
  }
}

export const remoteStore = new RemoteStore()

export function useRemote(): RemoteSnapshot {
  return useSyncExternalStore(remoteStore.subscribe, remoteStore.getSnapshot, remoteStore.getServerSnapshot)
}
