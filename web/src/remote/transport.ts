import { RemoteTransport as PortableRemoteTransport, type RemoteTransportAdapter, type RemoteTransportEvents } from '../../../frontend/shared/remote/transport'
import { fetchRemoteAuthStatus } from './auth'

export type { RemoteTransportEvents, RemoteTransportAdapter } from '../../../frontend/shared/remote/transport'

const adapter: RemoteTransportAdapter = {
  async request<T>(path: string): Promise<T> {
    if (path === '/api/auth/status') return await fetchRemoteAuthStatus() as T
    const response = await fetch(path, { credentials: 'same-origin', cache: 'no-store' })
    if (!response.ok) throw new Error(`Remote protocol endpoint returned ${response.status}.`)
    return response.json() as Promise<T>
  },
  websocketUrl(serverPath) {
    const configured = import.meta.env.VITE_YEET_WS_PATH as string | undefined
    const path = configured || serverPath || '/api/ws'
    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
    return `${protocol}//${location.host}${path}`
  },
  createSocket: (url, protocol) => new WebSocket(url, protocol),
  storage: {
    getItem: key => sessionStorage.getItem(key),
    setItem: (key, value) => sessionStorage.setItem(key, value),
  },
  isOnline: () => navigator.onLine,
  now: () => performance.now(),
  requestId: () => typeof crypto.randomUUID === 'function' ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`,
  setTimeout: (callback, milliseconds) => window.setTimeout(callback, milliseconds),
  clearTimeout: handle => window.clearTimeout(handle),
  setInterval: (callback, milliseconds) => window.setInterval(callback, milliseconds),
  clearInterval: handle => window.clearInterval(handle),
}

/** Browser adapter preserves same-origin authentication and per-tab affinity. */
export class RemoteTransport extends PortableRemoteTransport {
  constructor(events: RemoteTransportEvents) { super(events, adapter) }
}
