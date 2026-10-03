import { Platform } from 'react-native'
import * as SecureStore from 'expo-secure-store'
import type { RemoteTransportAdapter, RemoteSocket } from '../../../shared/remote/transport'

let endpoint = Platform.OS === 'web' ? globalThis.location?.origin ?? '' : ''
const identities = new Map<string, string>()
let sessionCookie: string | null = null

export function remoteEndpoint(): string { return endpoint }
export async function configureEndpoint(value: string): Promise<void> {
  const url = new URL(value)
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) {
    throw new Error('Use an HTTP or HTTPS Yeet Remote origin without credentials or query parameters.')
  }
  if (Platform.OS === 'web' && url.origin !== globalThis.location.origin) {
    throw new Error('Serve this web client from the Yeet Remote origin or use the documented development proxy.')
  }
  const next = url.origin
  if (endpoint !== next) { identities.clear(); sessionCookie = null }
  endpoint = next
  if (Platform.OS !== 'web') {
    const stored = await SecureStore.getItemAsync(identityKey())
    if (stored) identities.set('yeet.remote.resume.v1', stored)
  }
}
function identityKey(): string { return `yeet.resume.${endpoint.replace(/[^a-zA-Z0-9_.-]/g, '_')}` }
export async function remoteRequest<T>(path: string, init: RequestInit = {}): Promise<T> {
  if (!endpoint) throw new Error('Enter the address of a running Yeet Remote server.')
  const response = await fetch(new URL(path, endpoint).href, {
    ...init,
    credentials: 'include',
    headers: { ...(sessionCookie && Platform.OS !== 'web' ? { Cookie: sessionCookie } : {}), ...init.headers },
  })
  // Browser cookies remain HttpOnly; native requests can retain the server cookie
  // in memory for the WebSocket handshake without storing the access key.
  if (Platform.OS !== 'web') {
    const cookie = response.headers.get('set-cookie')
    if (cookie) sessionCookie = cookie.split(';')[0]
  }
  if (!response.ok) {
    const body = await response.json().catch(() => ({})) as { error?: string }
    throw new Error(body.error ?? `Yeet Remote returned ${response.status}.`)
  }
  return response.json() as Promise<T>
}
export async function authenticate(key: string): Promise<void> {
  if (key.trim()) await remoteRequest('/api/auth/key', {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ key }),
  })
}
export const networkAdapter: RemoteTransportAdapter = {
  request: path => remoteRequest(path),
  websocketUrl: path => {
    const url = new URL(path ?? '/api/ws', endpoint)
    if (url.origin !== endpoint) throw new Error('Remote advertised a WebSocket on a different origin.')
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
    return url.href
  },
  createSocket: (url, protocol) => {
    // React Native's socket options accept headers; web uses its HttpOnly cookie.
    const Socket = WebSocket as unknown as new (url: string, protocol: string, options?: { headers: Record<string, string> }) => RemoteSocket
    return new Socket(url, protocol, Platform.OS === 'web' ? undefined : { headers: sessionCookie ? { Cookie: sessionCookie } : {} })
  },
  storage: {
    getItem: key => Platform.OS === 'web' ? globalThis.sessionStorage?.getItem(key) ?? null : identities.get(key) ?? null,
    setItem: (key, value) => {
      if (Platform.OS === 'web') globalThis.sessionStorage?.setItem(key, value)
      else { identities.set(key, value); void SecureStore.setItemAsync(identityKey(), value).catch(() => {}) }
    },
  },
  isOnline: () => Platform.OS !== 'web' || globalThis.navigator?.onLine !== false,
  now: () => Date.now(),
  requestId: () => `${Date.now()}-${Math.random().toString(36).slice(2)}`,
  setTimeout: (callback, ms) => setTimeout(callback, ms) as unknown as number,
  clearTimeout: handle => clearTimeout(handle),
  setInterval: (callback, ms) => setInterval(callback, ms) as unknown as number,
  clearInterval: handle => clearInterval(handle),
}
