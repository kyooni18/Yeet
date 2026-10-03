import type { BridgeState, FrontendCommand, RemoteServerMessage } from '../../../../shared/remote/protocol'
import type { RemoteTransportEvents } from '../../../../shared/remote/transport'

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
      const buffered: CoreEvent[] = []
      const applyEvent = (event: CoreEvent) => {
        if (event.workspace && event.workspace !== this.workspace) return
        if (event.state) this.snapshot(event.state)
        if (event.message && !event.state) this.events.onError(event.message)
      }
      const unsubscribe = await api.event.listen<CoreEvent>('yeet://core-event', event => {
        if (generation !== this.generation) return
        if (initializing) buffered.push(event.payload)
        else applyEvent(event.payload)
      })
      if (generation !== this.generation) { unsubscribe(); return }
      this.unsubscribe = unsubscribe
      const result = await api.core.invoke<{ workspace: string; state: BridgeState | null }>('connect_core', { workspace: this.workspace })
      if (generation !== this.generation) return
      this.workspace = result.workspace
      this.conversation = []
      this.connected = true
      this.events.onMessage({ version: 1, type: 'welcome', client_id: 'desktop', workspace: result.workspace, sequence: 0, revision: 0, resumed: false })
      if (result.state) this.snapshot(result.state)
      initializing = false
      for (const event of buffered) applyEvent(event)
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
  close(): void {
    this.generation++; this.connected = false; this.unsubscribe?.(); this.unsubscribe = null
    disconnecting = bridge()?.core.invoke('disconnect_core').catch(() => {}) ?? Promise.resolve()
  }
  switchWorkspace(workspace: string): void { this.workspace = workspace; this.connected = false; void this.connect() }
  reconnectAfterAuth(): void { void this.connect() }
  markApplied(_message: RemoteServerMessage): void {}
}
