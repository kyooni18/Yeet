import { useSyncExternalStore } from 'react'
import { AppState, Platform } from 'react-native'
import { RemoteStore } from '../../../shared/state/remoteStore'
import { RemoteTransport } from '../../../shared/remote/transport'
import { authenticate, configureEndpoint, networkAdapter } from '../platform/remote'

export const remoteStore = new RemoteStore({
  createTransport: events => new RemoteTransport(events, networkAdapter),
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
  await configureEndpoint(endpoint)
  await authenticate(key)
  remoteStore.init()
}
