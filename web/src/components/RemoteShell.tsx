import { useEffect, useRef, useState } from 'react'
import { Composer } from '@/components/Composer'
import { Conversation } from '@/components/Conversation'
import { ModelSheet } from '@/components/ModelSheet'
import { QuickPanel } from '@/components/QuickPanel'
import { SettingsSheet } from '@/components/SettingsSheet'
import { Sidebar } from '@/components/Sidebar'
import { TopBar } from '@/components/TopBar'

export function RemoteShell() {
  const [sidebar, setSidebar] = useState(false)
  const [controls, setControls] = useState(false)
  const [modelSheet, setModelSheet] = useState(false)
  const [settings, setSettings] = useState(false)
  const [desktopLayout, setDesktopLayout] = useState(false)
  const modelReturnFocus = useRef<HTMLElement | null>(null)
  const settingsReturnFocus = useRef<HTMLElement | null>(null)

  const [editRequest, setEditRequest] = useState<{ key: number; content: string } | null>(null)
  const sidebarVisible = sidebar && (desktopLayout || (!modelSheet && !settings))
  const controlsVisible = controls && (desktopLayout || (!modelSheet && !settings))

  const rememberFocus = (target: React.MutableRefObject<HTMLElement | null>) => {
    const active = document.activeElement
    target.current = active instanceof HTMLElement && active !== document.body ? active : null
  }

  const restoreFocus = (target: React.MutableRefObject<HTMLElement | null>) => {
    const element = target.current
    target.current = null
    if (!element?.isConnected) return
    requestAnimationFrame(() => {
      if (element.isConnected) element.focus({ preventScroll: true })
    })
  }

  const openModel = () => {
    if (!modelSheet) rememberFocus(modelReturnFocus)
    setModelSheet(true)
  }

  const closeModel = () => {
    setModelSheet(false)
    restoreFocus(modelReturnFocus)
  }

  const openSettings = () => {
    if (!settings) rememberFocus(settingsReturnFocus)
    setSettings(true)
  }

  const closeSettings = () => {
    setSettings(false)
    restoreFocus(settingsReturnFocus)
  }

  useEffect(() => {
    const media = window.matchMedia('(min-width: 1000px) and (hover: hover) and (pointer: fine)')
    const update = (initial = false) => {
      const desktop = media.matches
      setDesktopLayout(desktop)
      if (desktop) {
        if (initial) setSidebar(true)
      } else if (!initial) {
        setSidebar(false)
        setControls(false)
      }
    }

    update(true)
    const onChange = () => update(false)
    media.addEventListener('change', onChange)
    return () => media.removeEventListener('change', onChange)
  }, [])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      if (modelSheet) closeModel()
      else if (settings) closeSettings()
      else if (controls) setControls(false)
      else if (sidebar) setSidebar(false)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [controls, modelSheet, settings, sidebar])

  return (
    <div
      className={[
        'remote-stage',
        'app-shell',
        desktopLayout ? 'is-desktop' : 'is-compact',
        sidebarVisible ? 'navigation-open' : '',
        controlsVisible ? 'inspector-open' : '',
      ].filter(Boolean).join(' ')}
    >
      {!((sidebarVisible && !desktopLayout) || (controlsVisible && !desktopLayout) || modelSheet || settings) && (
        <a
          className="skip-link"
          href="#conversation-transcript"
          onClick={(event) => {
            event.preventDefault()
            document.getElementById('conversation-transcript')?.focus({ preventScroll: true })
          }}
        >
          Skip to conversation
        </a>
      )}

      <Sidebar
        open={sidebarVisible}
        desktopDocked={desktopLayout}
        onClose={() => setSidebar(false)}
        onSettings={openSettings}
      />
      <section className={`main-viewport app-workspace${sidebarVisible ? ' sidebar-open' : ''}`} aria-label="Current session">
        <TopBar
          sidebarOpen={sidebarVisible}
          controlsOpen={controlsVisible}
          onToggleSidebar={() => {
            if (!desktopLayout) setControls(false)
            setSidebar(true)
          }}
          onToggleControls={() => {
            if (!desktopLayout) setSidebar(false)
            setControls((value) => !value)
          }}
        />

        <div className="workspace-transcript">
          <Conversation onEditLast={(content) => setEditRequest({ key: Date.now(), content })} />
        </div>

        <div className="workspace-composer">
          <Composer
            onModel={openModel}
            onSessions={() => {
              if (!desktopLayout) setControls(false)
              setSidebar(true)
            }}
            onSettings={openSettings}
            editRequest={editRequest}
            onEditConsumed={() => setEditRequest(null)}
          />
        </div>
      </section>

      <QuickPanel
        open={controlsVisible}
        onClose={() => setControls(false)}
        onModel={openModel}
        onSettings={openSettings}
      />
      <ModelSheet open={modelSheet} onClose={closeModel} />
      <SettingsSheet open={settings} onClose={closeSettings} />
    </div>
  )
}
