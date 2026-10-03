import { useSyncExternalStore } from 'react'
import { AppState, Platform } from 'react-native'
import { RemoteStore } from '../../../shared/state/remoteStore'
import { RemoteTransport } from '../../../shared/remote/transport'
import { authenticate, configureEndpoint, networkAdapter } from '../platform/remote'
import { DesktopTransport, isDesktopHost } from '../platform/desktop/transport'

let desktopWorkspace = ''
export const remoteStore = new RemoteStore({
  createTransport: events => {
    if (!isDesktopHost()) return new RemoteTransport(events, networkAdapter)
    const transport = new DesktopTransport(events)
    transport.setWorkspace(desktopWorkspace)
    return transport
  },
  scheduleFrame: callback => requestAnimationFrame(callback),
  cancelFrame: handle => cancelAnimationFrame(handle),
  subscribeConnectivity: (online, offline) => {
    if (Platform.OS === 'web') {
      globalThis.addEventListener('online', online); globalThis.addEventListener('offline', offline)
      return () => { globalThis.removeEventListener('online', online); globalThis.removeEventListener('offline', offline) }
    }
    const subscription = AppState.addEventListener('change', state => { if (state === 'active') online() })
    return () => subscription.remove()
  },
})
export function useRemote() { return useSyncExternalStore(remoteStore.subscribe, remoteStore.getSnapshot, remoteStore.getServerSnapshot) }
export async function connectRemote(endpoint: string, key: string): Promise<void> {
  remoteStore.destroy()
  if (isDesktopHost()) {
    desktopWorkspace = endpoint.trim()
    if (!desktopWorkspace) throw new Error('Enter a local workspace path.')
    remoteStore.init()
    return
  }
  await configureEndpoint(endpoint)
  await authenticate(key)
  remoteStore.init()
}
