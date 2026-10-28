import { mount } from '@mun/web'
import { remoteStore } from '@/store/remoteStore'
import { remote, startRemote } from '@/app/state'
import { beginEdit } from '@/app/composer'
import '@/app/markdown'
import '@/app/shellJob'
import App from '@/app/App.mun'
import '@/app/remote.css'

const root = document.getElementById('root')
if (!root) throw new Error('Yeet Remote root element is missing.')

startRemote(beginEdit)
mount(App, root)

// Document-level state the View tree cannot own.
let lastAppearance = ''
let lastTitle = ''
const sync = () => {
  const snapshot = remote.value
  const appearance = snapshot.state.runtime_settings?.appearance || 'auto'
  if (appearance !== lastAppearance) document.documentElement.dataset.appearance = lastAppearance = appearance
  const session = snapshot.currentSession?.title
  const title = snapshot.connection === 'auth-required' ? 'Authorize · Yeet' : session ? `${session} · Yeet` : 'Yeet'
  if (title !== lastTitle) document.title = lastTitle = title
}
remoteStore.subscribe(sync)
sync()
