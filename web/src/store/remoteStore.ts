import { useSyncExternalStore } from 'react'
import { RemoteStore } from '../../../frontend/shared/state/remoteStore'
import { RemoteTransport } from '@/remote/transport'

export { RemoteStore } from '../../../frontend/shared/state/remoteStore'
export type { RemoteSnapshot, RemoteStoreOptions } from '../../../frontend/shared/state/remoteStore'

export const remoteStore = new RemoteStore({
  createTransport: events => new RemoteTransport(events),
  scheduleFrame: callback => requestAnimationFrame(callback),
  cancelFrame: handle => cancelAnimationFrame(handle),
  subscribeConnectivity(online, offline) {
    window.addEventListener('online', online)
    window.addEventListener('offline', offline)
    return () => {
      window.removeEventListener('online', online)
      window.removeEventListener('offline', offline)
    }
  },
})

export function useRemote(store: RemoteStore = remoteStore) {
  return useSyncExternalStore(store.subscribe, store.getSnapshot, store.getServerSnapshot)
}
