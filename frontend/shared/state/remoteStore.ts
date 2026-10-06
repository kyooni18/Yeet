import type { ToolbarAction } from '../remote/protocol'
import type { RemoteClientTransport, RemoteTransportEvents } from '../remote/transport'
import {
  emptyBridgeState,
  type AuthProviderItem,
  type BridgeState,
  type ShellAction,
  type AgentAction,
  type ConversationAction,
  type ComposerAction,
  type SettingsAction,
  type SettingsView,
  type SettingsUiEffect,
  type ComposerView,
  type ComposerUiEffect,
  type ConversationView,
  type ConversationUiEffect,
  type AgentsView,
  type AgentUiEffect,
  type UiProjection,
  type ConnectionStatus,
  type ConversationEntry,
  type FrontendCommand,
  type ProviderUsageStatus,
  type SandboxAction,
  type SessionSummary,
  type WorkspaceSummary,
  type DiffAction,
  type DiffView,
  type WorkspaceChangesView,
  type WorkspaceFilesView,
  type HomeAction,
  type HomeView,
  type ResourceTarget,
} from '../remote/protocol'

export interface RemoteSnapshot {
  home: HomeView | null
  diffViews: DiffView[]
  workspaceFiles: WorkspaceFilesView | null
  workspaceChanges: WorkspaceChangesView | null
  homeEffect: { id: number; open: ResourceTarget } | null
  settings: SettingsView | null
  settingsEffect: { revision: number; value: SettingsUiEffect } | null
  composer: ComposerView | null
  composerEffect: { revision: number; value: ComposerUiEffect } | null
  conversation: ConversationView | null
  conversationEffect: { revision: number; value: ConversationUiEffect } | null
  agents: AgentsView | null
  agentEffect: { revision: number; value: AgentUiEffect } | null
  ui: UiProjection | null
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

export interface RemoteStoreOptions {
  createTransport(events: RemoteTransportEvents): RemoteClientTransport
  scheduleFrame(callback: () => void): number
  cancelFrame(handle: number): void
  subscribeConnectivity?(online: () => void, offline: () => void): () => void
}

export class RemoteStore {
  private unsubscribeConnectivity: (() => void) | undefined

  constructor(private readonly options: RemoteStoreOptions) {}
  private ui: UiProjection | null = null
  private uiRevision = -1
  private homeRevision = -1
  private diffRevision = -1
  private diffViews: DiffView[] = []
  private workspaceFiles: WorkspaceFilesView | null = null
  private workspaceChanges: WorkspaceChangesView | null = null
  private filesRequest: string | null = null
  private changesRequest: string | null = null
  private home: HomeView | null = null
  private homeEffect: { id: number; open: ResourceTarget } | null = null
  private homeEffectId = 0
  private settingsRevision = -1
  private settings: SettingsView | null = null
  private settingsEffect: { revision: number; value: SettingsUiEffect } | null = null
  private composerRevision = -1
  private composer: ComposerView | null = null
  private composerEffect: { revision: number; value: ComposerUiEffect } | null = null
  private conversationRevision = -1
  private conversation: ConversationView | null = null
  private conversationEffect: { revision: number; value: ConversationUiEffect } | null = null
  private agentsRevision = -1
  private agents: AgentsView | null = null
  private agentEffect: { revision: number; value: AgentUiEffect } | null = null
  private layout: 'compact' | 'expanded' | null = null
  private state = emptyBridgeState()
  private entries: ConversationEntry[] = []
  private connection: ConnectionStatus = 'connecting'
  private connectionError: string | null = null
  private transport: RemoteClientTransport | null = null
  private listeners = new Set<() => void>()
  private initialized = false
  private revision = 0
  private sessionResetRevision = 0
  private eventQueue: Parameters<RemoteClientTransport['markApplied']>[0][] = []
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
      home: this.home,
      diffViews: this.diffViews,
      workspaceFiles: this.workspaceFiles,
      workspaceChanges: this.workspaceChanges,
      homeEffect: this.homeEffect,
      settings: this.settings,
      settingsEffect: this.settingsEffect,
      composer: this.composer,
      composerEffect: this.composerEffect,
      conversation: this.conversation,
      conversationEffect: this.conversationEffect,
      agents: this.agents,
      agentEffect: this.agentEffect,
      ui: this.ui,
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
    this.transport = this.options.createTransport({
      onOpen: () => {
        this.connection = 'connected'
        this.connectionError = null
        this.requestRemoteState()
        if (this.layout) this.transport?.sendUi?.({ type: 'set_layout', layout: this.layout })
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
    this.unsubscribeConnectivity = this.options.subscribeConnectivity?.(this.handleOnline, this.handleOffline)
  }

  destroy(): void {
    if (!this.initialized) return
    this.initialized = false
    if (this.flushFrame) this.options.cancelFrame(this.flushFrame)
    this.flushFrame = 0
    this.eventQueue = []
    this.transport?.close()
    this.transport = null
    this.unsubscribeConnectivity?.()
    this.unsubscribeConnectivity = undefined
  }

  private handleOnline = (): void => {
    if (this.connection === 'offline') this.transport?.reconnectAfterAuth()
  }

  private handleOffline = (): void => {
    this.connection = 'offline'
    this.emit()
  }

  private enqueue(message: Parameters<RemoteClientTransport['markApplied']>[0]): void {
    this.eventQueue.push(message)
    if (this.flushFrame) return
    this.flushFrame = this.options.scheduleFrame(() => this.flushEvents())
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

  private applyMessage(message: Parameters<RemoteClientTransport['markApplied']>[0]): void {
    switch (message.type) {
      case 'ui_home':
        if (message.home_revision < this.homeRevision) return
        this.homeRevision = message.home_revision
        this.home = message.view
        return
      case 'ui_diff':
        if (message.diff_revision < this.diffRevision) return
        this.diffRevision = message.diff_revision
        this.diffViews = message.views
        return
      case 'workspace_files':
        if (message.request_id === this.filesRequest) this.workspaceFiles = message.view
        return
      case 'workspace_changes':
        if (message.request_id === this.changesRequest) this.workspaceChanges = message.view
        return
      case 'ui_home_effect':
        if (message.open) this.homeEffect = { id: ++this.homeEffectId, open: message.open }
        return
      case 'ui_settings':
        if (message.settings_revision < this.settingsRevision) return
        this.settingsRevision = message.settings_revision
        this.settings = message.view
        if (message.effect?.accepted_editor != null || message.effect?.destination != null) {
          this.settingsEffect = { revision: message.settings_revision, value: message.effect }
        }
        return
      case 'ui_composer':
        if (message.composer_revision < this.composerRevision) return
        this.composerRevision = message.composer_revision
        this.composer = message.view
        if (message.effect?.accepted_editor != null || message.effect?.cancel_edit != null || message.effect?.destination != null || message.effect?.replace_editor != null) {
          this.composerEffect = { revision: message.composer_revision, value: message.effect }
        }
        return
      case 'ui_conversation':
        if (message.conversation_revision < this.conversationRevision) return
        this.conversationRevision = message.conversation_revision
        this.conversation = message.view
        if (message.effect?.copy != null || message.effect?.edit_draft != null) {
          this.conversationEffect = { revision: message.conversation_revision, value: message.effect }
        }
        return
      case 'ui_agents':
        if (message.agents_revision < this.agentsRevision) return
        this.agentsRevision = message.agents_revision
        this.agents = message.view
        // A runtime refresh may share a frame with an accepted action. Preserve
        // its editor effect until another meaningful effect or connection epoch.
        if (message.effect?.submitted_text != null || message.effect?.open_group_settings) {
          this.agentEffect = { revision: message.agents_revision, value: message.effect }
        }
        return
      case 'ui_state':
        if (this.layout && message.ui_revision === 0 && message.view.layout !== this.layout) return
        if (message.ui_revision < this.uiRevision) return
        this.uiRevision = message.ui_revision
        this.ui = { ui_revision: message.ui_revision, state: message.state, view: message.view, toolbar: message.toolbar, application: message.application }
        return
      case 'workspace_switch_requested':
        if (this.state.workspace_root !== message.source_workspace) return
        if (this.state.known_workspaces.some(workspace => workspace.id === message.id && workspace.path === message.path)) {
          this.switchWorkspace(message.path)
        }
        return
      case 'welcome':
        this.uiRevision = -1
        this.homeRevision = -1
        this.diffRevision = -1
        this.diffViews = []
        this.workspaceFiles = null
        this.workspaceChanges = null
        this.filesRequest = null
        this.changesRequest = null
        this.home = null
        this.homeEffect = null
        this.agentsRevision = -1
        this.agentEffect = null
        this.settingsRevision = -1
        this.settingsEffect = null
        this.composerRevision = -1
        this.composerEffect = null
        this.conversationRevision = -1
        this.conversationEffect = null
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

  consumeSettingsEffect(revision: number): SettingsUiEffect | null {
    if (this.settingsEffect?.revision !== revision) return null
    const effect = this.settingsEffect.value
    this.settingsEffect = null
    return effect
  }

  sendToolbarUi(action: ToolbarAction): boolean { return this.transport?.sendToolbarUi?.(action) ?? false }
  requestFiles(path: string, selected: string | null = null): boolean {
    this.filesRequest = this.transport?.requestWorkspaceFiles?.(path, selected) ?? null
    return this.filesRequest !== null
  }
  requestChanges(file: string | null = null, full = false): boolean {
    this.changesRequest = this.transport?.requestWorkspaceChanges?.(file, full) ?? null
    return this.changesRequest !== null
  }
  sendDiffUi(action: DiffAction): boolean { return this.transport?.sendDiffUi?.(action) ?? false }
  sendHomeUi(action: HomeAction): boolean { return this.transport?.sendHomeUi?.(action) ?? false }
  consumeHomeEffect(id: number): ResourceTarget | null {
    if (this.homeEffect?.id !== id) return null
    const open = this.homeEffect.open
    this.homeEffect = null
    return open
  }
  sendSettingsUi(action: SettingsAction): boolean {
    return this.transport?.sendSettingsUi?.(action) ?? false
  }

  consumeComposerEffect(revision: number): ComposerUiEffect | null {
    if (this.composerEffect?.revision !== revision) return null
    const effect = this.composerEffect.value
    this.composerEffect = null
    return effect
  }

  sendComposerUi(action: ComposerAction): boolean {
    return this.transport?.sendComposerUi?.(action) ?? false
  }

  consumeConversationEffect(revision: number): ConversationUiEffect | null {
    if (this.conversationEffect?.revision !== revision) return null
    const effect = this.conversationEffect.value
    this.conversationEffect = null
    return effect
  }

  sendConversationUi(action: ConversationAction): boolean {
    return this.transport?.sendConversationUi?.(action) ?? false
  }

  sendAgentUi(action: AgentAction): boolean {
    return this.transport?.sendAgentUi?.(action) ?? false
  }

  sendUi(action: ShellAction): boolean {
    if (action.type === 'set_layout') this.layout = action.layout
    return this.transport?.sendUi?.(action) ?? false
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

  createAgentGroup(objective: string): boolean {
    return this.send({ type: 'create_agent_group', objective })
  }

  startAgentGroup(groupId: string): boolean {
    return this.send({ type: 'start_agent_group', group_id: groupId })
  }

  resumeAgentGroup(groupId: string): boolean {
    return this.send({ type: 'resume_agent_group', group_id: groupId })
  }

  cancelAgentGroup(groupId: string): boolean {
    return this.send({ type: 'cancel_agent_group', group_id: groupId })
  }

  stopAgentGroup(groupId: string): boolean {
    return this.send({ type: 'stop_agent_group', group_id: groupId })
  }

  inspectAgentGroup(groupId: string): boolean {
    return this.send({ type: 'inspect_agent_group', group_id: groupId })
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
