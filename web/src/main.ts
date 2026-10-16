import { mount } from '@mun/web'
import { remoteStore } from '@/store/remoteStore'
import { remote } from '@/app/state'
import '@/app/markdown'
import App from '@/app/App.mun'
import '@/app/remote.css'

const root = document.getElementById('root')
if (!root) throw new Error('Yeet Remote root element is missing.')

remoteStore.init()
remoteStore.subscribe(() => {
  const snapshot = remote.value
  document.documentElement.dataset.appearance = snapshot.state.runtime_settings?.appearance || 'auto'
  const title = snapshot.currentSession?.title
  document.title = snapshot.connection === 'auth-required' ? 'Authorize · Yeet' : title ? `${title} · Yeet` : 'Yeet'
})

mount(App, root)
