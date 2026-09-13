import {
  decodeServerMessage,
  encodeCommand,
  encodeHello,
  encodePing,
  type FrontendCommand,
  type RemoteProtocolInfo,
  type RemoteResumeState,
  type RemoteServerMessage,
} from './protocol'
import { fetchRemoteAuthStatus } from './auth'

export interface RemoteTransportEvents {
  onOpen: () => void
  onMessage: (message: RemoteServerMessage) => void
  onStatus: (status: 'connecting' | 'reconnecting' | 'offline' | 'auth-required' | 'failed') => void
  onError: (message: string) => void
}

const RESUME_KEY = 'yeet.remote.resume.v1'

const fetchProtocolInfo = async (): Promise<RemoteProtocolInfo> => {
  const response = await fetch('/api/protocol', { credentials: 'same-origin', cache: 'no-store' })
  if (!response.ok) throw new Error(`Remote protocol endpoint returned ${response.status}.`)
  return response.json() as Promise<RemoteProtocolInfo>
}

const wsUrl = (serverPath?: string): string => {
  const configured = import.meta.env.VITE_YEET_WS_PATH as string | undefined
  const path = configured || serverPath || '/api/ws'
  const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${protocol}//${location.host}${path}`
}

const readResumeState = (): RemoteResumeState => {
  try {
    const value = sessionStorage.getItem(RESUME_KEY)
    if (!value) return {}
    const stored = JSON.parse(value) as RemoteResumeState
    // A hard reload retains tab/session affinity, but not the in-memory
    // semantic store that a sequence cursor refers to. Keep only identity
    // across reloads so the server sends a fresh authoritative snapshot.
    return {
      clientId: stored.clientId ?? null,
      workspace: stored.workspace ?? null,
      sessionId: stored.sessionId ?? null,
    }
  } catch {
    return {}
  }
}

const writeResumeState = (value: RemoteResumeState): void => {
  try {
    sessionStorage.setItem(RESUME_KEY, JSON.stringify({
      clientId: value.clientId ?? null,
      workspace: value.workspace ?? null,
      sessionId: value.sessionId ?? null,
    }))
  } catch { /* unavailable */ }
}

export class RemoteTransport {
  private socket: WebSocket | null = null
  private closedByClient = false
  private reconnectAttempt = 0
  private reconnectTimer: number | undefined
  private heartbeatTimer: number | undefined
  private lastMessageAt = 0
  private welcomed = false
  private pendingInterrupt = false
  private intentionalClosures = new WeakSet<WebSocket>()
  private resume = readResumeState()
  private lastReceivedSequence: number | null = this.resume.lastSequence ?? null
  private protocolInfo: RemoteProtocolInfo | null = null

  constructor(private readonly events: RemoteTransportEvents) {}

  async connect(): Promise<void> {
    this.closedByClient = false
    this.events.onStatus(this.reconnectAttempt > 0 ? 'reconnecting' : 'connecting')
    try {
      const auth = await fetchRemoteAuthStatus()
      if (auth.required && !auth.authenticated) {
        this.events.onStatus('auth-required')
        return
      }
      const protocol = await fetchProtocolInfo()
      this.protocolInfo = protocol
      if (protocol.maxVersion < 1 || protocol.minVersion > 1) {
        this.events.onStatus('failed')
        this.events.onError(`Remote protocol ${protocol.minVersion}..${protocol.maxVersion} is not supported by this WebUI.`)
        return
      }
      this.openSocket(protocol)
    } catch (error) {
      this.events.onError(error instanceof Error ? error.message : String(error))
      this.scheduleReconnect()
    }
  }

  reconnectAfterAuth(): void {
    this.reconnectAttempt = 0
    void this.connect()
  }

  switchWorkspace(workspace: string): void {
    const next = workspace.trim()
    if (!next) return
    this.resume.workspace = next
    this.resume.sessionId = null
    this.resume.lastSequence = null
    this.resume.lastRevision = null
    this.lastReceivedSequence = null
    writeResumeState(this.resume)
    this.reconnectAttempt = 0
    this.stopHeartbeat()
    const socket = this.socket
    if (socket) {
      this.intentionalClosures.add(socket)
      socket.close(1000, 'workspace switch')
    }
    this.socket = null
    this.welcomed = false
    this.closedByClient = false
    void this.connect()
  }

  send(command: FrontendCommand): boolean {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN || !this.welcomed) {
      if (command.type === 'interrupt') {
        this.pendingInterrupt = true
        return true
      }
      return false
    }
    const requestId = typeof crypto.randomUUID === 'function' ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`
    this.socket.send(encodeCommand(command, requestId))
    if (command.type === 'load_session') {
      this.resume.sessionId = command.session_id
      writeResumeState(this.resume)
    }
    return true
  }

  close(): void {
    this.closedByClient = true
    this.welcomed = false
    if (this.reconnectTimer) window.clearTimeout(this.reconnectTimer)
    if (this.heartbeatTimer) window.clearInterval(this.heartbeatTimer)
    if (this.socket) {
      this.intentionalClosures.add(this.socket)
      this.socket.close(1000, 'client closed')
    }
    this.socket = null
  }

  markApplied(message: RemoteServerMessage): void {
    if (!this.isSequenced(message)) return
    this.resume.lastSequence = message.sequence
    this.resume.lastRevision = message.revision
    if (message.type === 'snapshot') this.resume.sessionId = message.state.current_session_id ?? null
    if (message.type === 'state_update' && 'current_session_id' in message.patch) {
      this.resume.sessionId = message.patch.current_session_id ?? null
    }
    // Identity/session affinity survives a reload. The semantic cursor stays
    // memory-only and is reused only for a live socket reconnect.
    writeResumeState(this.resume)
  }

  private openSocket(protocolInfo: RemoteProtocolInfo): void {
    this.socket?.close()
    this.welcomed = false
    this.lastReceivedSequence = this.resume.lastSequence ?? null
    const socket = new WebSocket(wsUrl(protocolInfo.websocket), 'yeet.remote.v1')
    this.socket = socket

    socket.addEventListener('open', () => {
      this.lastMessageAt = performance.now()
      socket.send(encodeHello(this.resume))
    })

    socket.addEventListener('message', (event) => {
      this.lastMessageAt = performance.now()
      if (typeof event.data !== 'string') return
      const message = decodeServerMessage(event.data)
      if (!message) return

      if (message.type === 'error') {
        this.events.onError(`${message.code}: ${message.message}`)
        if (message.fatal) {
          if (message.code === 'workspace_mismatch') {
            this.resume = {}
            this.lastReceivedSequence = null
            writeResumeState(this.resume)
            socket.close(1000, 'workspace changed')
            this.socket = null
            this.welcomed = false
            window.setTimeout(() => {
              this.closedByClient = false
              void this.connect()
            }, 0)
            return
          }
          this.events.onStatus('failed')
          this.closedByClient = true
          socket.close(4002, message.code)
          return
        }
      }

      if (message.type === 'welcome') {
        this.welcomed = true
        this.reconnectAttempt = 0
        if (!message.resumed) {
          this.resume.lastSequence = null
          this.resume.lastRevision = null
          this.lastReceivedSequence = null
        } else {
          this.lastReceivedSequence = this.resume.lastSequence ?? null
        }
        this.resume.clientId = message.client_id
        this.resume.workspace = message.workspace
        this.resume.sessionId = message.session_id ?? null
        writeResumeState(this.resume)
        this.events.onMessage(message)
        this.events.onOpen()
        if (this.pendingInterrupt) {
          this.pendingInterrupt = false
          this.send({ type: 'interrupt' })
        }
        this.startHeartbeat()
        return
      }

      if (this.isSequenced(message) && !this.acceptSequence(message)) return
      this.events.onMessage(message)
    })

    socket.addEventListener('error', () => {
      this.events.onError('Remote connection failed.')
    })

    socket.addEventListener('close', () => {
      this.stopHeartbeat()
      const intentional = this.intentionalClosures.has(socket)
      if (this.socket === socket) {
        this.socket = null
        this.welcomed = false
      }
      if (intentional || (this.socket && this.socket !== socket)) return
      if (this.closedByClient) return
      this.scheduleReconnect()
    })
  }

  private isSequenced(message: RemoteServerMessage): message is Extract<RemoteServerMessage, { sequence: number }> {
    return 'sequence' in message && typeof message.sequence === 'number' && message.type !== 'welcome'
  }

  private acceptSequence(message: Extract<RemoteServerMessage, { sequence: number }>): boolean {
    const previous = this.lastReceivedSequence
    if (previous != null && message.sequence <= previous && message.type !== 'snapshot') return false
    if (previous != null && message.sequence > previous + 1 && message.type !== 'snapshot') {
      this.socket?.close(4001, 'sequence gap')
      return false
    }
    this.lastReceivedSequence = message.sequence
    return true
  }

  private scheduleReconnect(): void {
    if (this.closedByClient) return
    this.reconnectAttempt += 1
    this.events.onStatus(navigator.onLine ? 'reconnecting' : 'offline')
    const delay = Math.min(12_000, 500 * 2 ** Math.min(this.reconnectAttempt - 1, 5))
    if (this.reconnectTimer) window.clearTimeout(this.reconnectTimer)
    this.reconnectTimer = window.setTimeout(() => void this.connect(), delay)
  }

  private startHeartbeat(): void {
    this.stopHeartbeat()
    this.heartbeatTimer = window.setInterval(() => {
      const socket = this.socket
      if (!socket || socket.readyState !== WebSocket.OPEN || !this.welcomed) return
      if (performance.now() - this.lastMessageAt > 25_000) {
        socket.close(4000, 'heartbeat timeout')
        return
      }
      socket.send(encodePing(`${Date.now()}`))
    }, 10_000)
  }

  private stopHeartbeat(): void {
    if (this.heartbeatTimer) window.clearInterval(this.heartbeatTimer)
    this.heartbeatTimer = undefined
  }
}
