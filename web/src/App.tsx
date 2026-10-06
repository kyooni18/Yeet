import { useEffect } from 'react'
import { AuthGate } from '@/components/AuthGate'
import { RemoteShell } from '@/components/RemoteShell'
import { remoteStore, useRemote } from '@/store/remoteStore'
import '@/styles/ios-remote.css'
import '@/styles/agents.css'
import '@/styles/yeet-design.css'

export default function App() {
  const remote = useRemote()

  useEffect(() => {
    remoteStore.init()
    return () => remoteStore.destroy()
  }, [])

  useEffect(() => {
    const root = document.documentElement
    const viewport = window.visualViewport

    const updateViewport = () => {
      const width = viewport?.width ?? window.innerWidth
      const height = viewport?.height ?? window.innerHeight
      const top = viewport?.offsetTop ?? 0
      const left = viewport?.offsetLeft ?? 0
      const bottom = Math.max(0, window.innerHeight - top - height)
      window.dispatchEvent(new Event('yeet:visual-viewport-will-change'))

      root.style.setProperty('--visual-viewport-top', `${top}px`)
      root.style.setProperty('--visual-viewport-left', `${left}px`)
      root.style.setProperty('--visual-viewport-width', `${width}px`)
      root.style.setProperty('--visual-viewport-height', `${height}px`)
      root.style.setProperty('--visual-viewport-bottom', `${bottom}px`)
      root.dataset.keyboardOpen = bottom > 80 ? 'true' : 'false'
      window.dispatchEvent(new Event('yeet:visual-viewport-change'))
    }

    updateViewport()
    viewport?.addEventListener('resize', updateViewport)
    viewport?.addEventListener('scroll', updateViewport)
    window.addEventListener('resize', updateViewport)

    return () => {
      viewport?.removeEventListener('resize', updateViewport)
      viewport?.removeEventListener('scroll', updateViewport)
      window.removeEventListener('resize', updateViewport)
    }
  }, [])

  useEffect(() => {
    const appearance = remote.state.runtime_settings?.appearance || 'auto'
    document.documentElement.dataset.appearance = appearance

    if (location.pathname === '/enroll' || remote.connection === 'auth-required') {
      document.title = 'Authorize · Yeet'
    } else {
      const title = remote.currentSession?.title
      document.title = title ? `${title} · Yeet` : 'Yeet Remote'
    }
  }, [remote.connection, remote.currentSession?.title, remote.state.runtime_settings?.appearance])

  if (location.pathname === '/enroll' || remote.connection === 'auth-required') return <AuthGate />
  return <RemoteShell />
}
