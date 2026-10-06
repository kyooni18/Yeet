import { useEffect, useRef, useState } from 'react'
import { Composer } from '@/components/Composer'
import { Conversation } from '@/components/Conversation'
import { AgentsSheet } from '@/components/AgentsSheet'
import { ModelSheet } from '@/components/ModelSheet'
import { QuickPanel } from '@/components/QuickPanel'
import { SettingsSheet } from '@/components/SettingsSheet'
import { Sidebar } from '@/components/Sidebar'
import { TopBar } from '@/components/TopBar'
import { remoteStore, useRemote } from '@/store/remoteStore'
import type { ShellAction, ShellState, ToolbarAction } from '@/remote/protocol'

/** Browser visuals and focus for the Rust-authored application shell. */
export function RemoteShell() {
  const { ui } = useRemote()
  const modelReturnFocus = useRef<HTMLElement | null>(null)
  const settingsReturnFocus = useRef<HTMLElement | null>(null)
  const navigationReturnFocus = useRef<HTMLElement | null>(null)
  const priorState = useRef<ShellState | null>(null)
  const [editRequest, setEditRequest] = useState<{ key: number; content: string } | null>(null)
  const view = ui?.view
  const visible = (kind: string) => view?.views.some(item => item.kind === kind && item.placement !== 'hidden') ?? false
  const sidebarVisible = visible('navigation')
  const controlsVisible = visible('inspector')
  const modelSheet = visible('models')
  const settings = visible('settings')
  const desktopLayout = view?.layout === 'expanded'

  const rememberFocus = (target: React.MutableRefObject<HTMLElement | null>) => {
    const active = document.activeElement
    target.current = active instanceof HTMLElement && active !== document.body ? active : null
  }
  const send = (action: ShellAction) => remoteStore.sendUi(action)
  const openNavigation = (action?: ToolbarAction) => {
    rememberFocus(navigationReturnFocus)
    if(action) remoteStore.sendToolbarUi(action); else send({ type: 'open_navigation' })
  }
  const openModel = (action?: ToolbarAction) => {
    if (!modelSheet) rememberFocus(modelReturnFocus)
    if(action) remoteStore.sendToolbarUi(action); else send({ type: 'open_models' })
  }
  const openSettings = (action?: ToolbarAction) => {
    if (!settings) rememberFocus(settingsReturnFocus)
    if(action) remoteStore.sendToolbarUi(action); else send({ type: 'open_settings' })
  }

  useEffect(() => {
    if (!ui) return
    for (const [kind, target] of [['models', modelReturnFocus], ['settings', settingsReturnFocus], ['navigation', navigationReturnFocus]] as const) {
      const wasOpen = priorState.current?.[kind]
      const open = ui.state[kind]
      if (wasOpen && !open) {
        const element = target.current
        target.current = null
        if (element?.isConnected) requestAnimationFrame(() => {
          if (element.isConnected) element.focus({ preventScroll: true })
        })
      }
    }
    priorState.current = ui.state
  }, [ui])

  useEffect(() => {
    const media = window.matchMedia('(min-width: 1000px) and (hover: hover) and (pointer: fine)')
    const update = () => remoteStore.sendUi({ type: 'set_layout', layout: media.matches ? 'expanded' : 'compact' })
    update()
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') remoteStore.sendUi({ type: 'dismiss' })
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  if (!view) return null

  return (
    <div className={[
      'remote-stage', 'app-shell', desktopLayout ? 'is-desktop' : 'is-compact',
      sidebarVisible ? 'navigation-open' : '', controlsVisible ? 'inspector-open' : '',
    ].filter(Boolean).join(' ')}>
      {view?.skip_conversation && (
        <a className="skip-link" href="#conversation-transcript" onClick={(event) => {
          event.preventDefault()
          document.getElementById('conversation-transcript')?.focus({ preventScroll: true })
        }}>Skip to conversation</a>
      )}
      {view?.views.map(item => {
        const open = item.placement !== 'hidden'
        switch (item.kind) {
          case 'navigation': return <Sidebar key={item.kind} open={open} desktopDocked={desktopLayout}
            onClose={() => send({ type: 'close_navigation' })} onSettings={openSettings}
            onAgents={() => send({ type: 'open_agents' })} />
          case 'workspace': return (
            <section key={item.kind} className={`main-viewport app-workspace${sidebarVisible ? ' sidebar-open' : ''}`} aria-label="Current session">
              {item.children.map(child => {
                switch (child) {
                  case 'session_header': return <TopBar key={child} sidebarOpen={sidebarVisible} controlsOpen={controlsVisible}
                    onToggleSidebar={openNavigation}
                    onToggleControls={(action) => action ? remoteStore.sendToolbarUi(action) : send({ type: 'toggle_inspector' })} />
                  case 'conversation': return <div key={child} className="workspace-transcript">
                    <Conversation onEditLast={(content) => setEditRequest({ key: Date.now(), content })} />
                  </div>
                  case 'composer': return <div key={child} className="workspace-composer">
                    <Composer onModel={openModel} onSessions={openNavigation}
                      onSettings={openSettings} onControls={() => send({ type: 'toggle_inspector' })} editRequest={editRequest} onEditConsumed={() => setEditRequest(null)} />
                  </div>
                }
              })}
            </section>
          )
          case 'inspector': return <QuickPanel key={item.kind} open={open} onClose={() => send({ type: 'close_inspector' })}
            onModel={openModel} onSettings={openSettings} />
          case 'models': return <ModelSheet key={item.kind} open={open} onClose={() => send({ type: 'close_models' })} />
          case 'settings': return <SettingsSheet key={item.kind} open={open} onClose={() => send({ type: 'close_settings' })} />
          case 'agents': return <AgentsSheet key={item.kind} open={open} onClose={() => send({ type: 'close_agents' })} />
        }
      })}
    </div>
  )
}
