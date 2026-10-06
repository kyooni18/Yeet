import type { BridgeState, FrontendCommand, RemoteServerMessage, ShellAction, UiProjection, AgentAction } from '../../../../shared/remote/protocol'
import type { RemoteTransportEvents } from '../../../../shared/remote/transport'

type AgentsEvent = Extract<RemoteServerMessage, { type: 'ui_agents' }>
type ApplicationProjection = UiProjection & { agents: Omit<AgentsEvent, 'version' | 'type' | 'request_id' | 'effect'> }
type UiEvent = Extract<RemoteServerMessage, { type: 'ui_state' }>
type CoreEvent = { workspace?: string; type: string; state: BridgeState | null; message: string | null }
type TauriBridge = {
  core: { invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> }
  event: { listen<T>(event: string, callback: (event: { payload: T }) => void): Promise<() => void> }
}
function bridge(): TauriBridge | undefined {
  return (globalThis as typeof globalThis & { __TAURI__?: TauriBridge }).__TAURI__
}
export function isDesktopHost(): boolean { return !!bridge() }
let disconnecting: Promise<unknown> = Promise.resolve()
export class DesktopTransport {
  private unsubscribe: (() => void) | null = null
  private generation = 0
  private sequence = 0
  private uiRevision = -1
  private agentsRevision = -1
  private conversation: BridgeState["conversation"] = []
  private connected = false
  private workspace = ''
  constructor(private events: RemoteTransportEvents) {}
  setWorkspace(workspace: string): void { this.workspace = workspace }
  async connect(): Promise<void> {
    const api = bridge()
    if (!api) return
    const generation = ++this.generation
    this.events.onStatus('connecting')
    try {
      await disconnecting
      if (generation !== this.generation) return
      this.unsubscribe?.()
      let initializing = true
      const buffered: (() => void)[] = []
      const applyUi = (event: UiEvent) => {
        if (event.ui_revision < this.uiRevision) return
        this.uiRevision = event.ui_revision
        this.events.onMessage(event)
      }
      const applyAgents = (event: AgentsEvent) => {
        if (event.agents_revision < this.agentsRevision) return
        this.agentsRevision = event.agents_revision
        this.events.onMessage(event)
      }
      const applyEvent = (event: CoreEvent) => {
        if (event.workspace && event.workspace !== this.workspace) return
        if (event.state) this.snapshot(event.state)
        if (event.message && !event.state) this.events.onError(event.message)
      }
      const unsubscribe = await api.event.listen<CoreEvent>('yeet://core-event', event => {
        if (generation !== this.generation) return
        if (initializing) buffered.push(() => applyEvent(event.payload))
        else applyEvent(event.payload)
      })
      if (generation !== this.generation) { unsubscribe(); return }
      this.unsubscribe = unsubscribe
      const unsubscribeUi = await api.event.listen<UiEvent>('yeet://ui-event', event => {
        if (generation !== this.generation) return
        if (initializing) buffered.push(() => applyUi(event.payload))
        else applyUi(event.payload)
      })
      if (generation !== this.generation) { unsubscribeUi(); unsubscribe(); return }
      this.unsubscribe = () => { unsubscribeUi(); unsubscribe() }
      const unsubscribeAgents = await api.event.listen<AgentsEvent>('yeet://ui-agents-event', event => {
        if (generation !== this.generation) return
        if (initializing) buffered.push(() => applyAgents(event.payload))
        else applyAgents(event.payload)
      })
      if (generation !== this.generation) { unsubscribeAgents(); unsubscribeUi(); unsubscribe(); return }
      this.unsubscribe = () => { unsubscribeAgents(); unsubscribeUi(); unsubscribe() }
      const result = await api.core.invoke<{ workspace: string; state: BridgeState | null }>('connect_core', { workspace: this.workspace })
      if (generation !== this.generation) return
      const projection = await api.core.invoke<ApplicationProjection>('application_projection')
      if (generation !== this.generation) return
      this.workspace = result.workspace
      this.conversation = []
      this.connected = true
      this.events.onMessage({ version: 1, type: 'welcome', client_id: 'desktop', workspace: result.workspace, sequence: 0, revision: 0, resumed: false })
      if (result.state) this.snapshot(result.state)
      applyUi({ version: 1, type: 'ui_state', ui_revision: projection.ui_revision, state: projection.state, view: projection.view })
      applyAgents({ version: 1, type: 'ui_agents', ...projection.agents })
      initializing = false
      for (const apply of buffered) apply()
      this.events.onOpen()
    } catch (error) {
      if (generation !== this.generation) return
      this.connected = false
      this.unsubscribe?.(); this.unsubscribe = null
      this.events.onStatus('failed'); this.events.onError(String(error))
    }
  }
  private snapshot(state: BridgeState): void {
    if (state.conversation != null) this.conversation = state.conversation
    state = { ...state, conversation: this.conversation }
    this.events.onMessage({ version: 1, type: 'snapshot', sequence: ++this.sequence, revision: state.conversation_revision, state })
  }
  send(command: FrontendCommand): boolean {
    if (!this.connected) return false
    void bridge()!.core.invoke('send_core_command', { command }).catch(error => this.events.onError(String(error)))
    return true
  }
  sendUi(action: ShellAction): boolean {
    if (!this.connected) return false
    void bridge()!.core.invoke('send_ui_action', { action }).catch(error => this.events.onError(String(error)))
    return true
  }
  sendAgentUi(action: AgentAction): boolean {
    if (!this.connected) return false
    void bridge()!.core.invoke('send_ui_agent_action', { action }).catch(error => this.events.onError(String(error)))
    return true
  }
  close(): void {
    this.generation++; this.connected = false; this.unsubscribe?.(); this.unsubscribe = null
    disconnecting = bridge()?.core.invoke('disconnect_core').catch(() => {}) ?? Promise.resolve()
  }
  switchWorkspace(workspace: string): void { this.workspace = workspace; this.connected = false; void this.connect() }
  reconnectAfterAuth(): void { void this.connect() }
  markApplied(_message: RemoteServerMessage): void {}
}
