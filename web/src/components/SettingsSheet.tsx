import { useEffect, useRef } from 'react'
import { Settings2, X } from '@/components/Icons'
import { ProviderIcon } from '@/components/ProviderIcon'
import { remoteStore, useRemote } from '@/store/remoteStore'

function providerDisplayName(provider: string): string {
  if (provider.toLowerCase() === 'antigravity') return 'Google Antigravity'
  return provider
}

function Switch({
  checked,
  onChange,
  label,
  disabled = false,
}: {
  checked: boolean
  onChange: (value: boolean) => void
  label: string
  disabled?: boolean
}) {
  return (
    <button
      className="ios-switch"
      data-on={checked}
      onClick={() => onChange(!checked)}
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
    >
      <span />
    </button>
  )
}

export function SettingsSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const remote = useRemote()
  const dialogRef = useRef<HTMLElement>(null)

  useEffect(() => {
    if (!open) return
    requestAnimationFrame(() => {
      dialogRef.current?.querySelector<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])')
        ?.focus({ preventScroll: true })
    })
  }, [open])

  const cycleFocus = (event: React.KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'Tab' || !dialogRef.current) return
    const focusables = [...dialogRef.current.querySelectorAll<HTMLElement>(
      'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [href], [tabindex]:not([tabindex="-1"])',
    )].filter((element) => element.getClientRects().length > 0)
    if (!focusables.length) return

    const active = event.target instanceof HTMLElement
      ? event.target
      : document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null
    const current = active ? focusables.indexOf(active) : -1
    const next = event.shiftKey
      ? (current <= 0 ? focusables.length - 1 : current - 1)
      : (current < 0 || current >= focusables.length - 1 ? 0 : current + 1)
    event.preventDefault()
    focusables[next]?.focus({ preventScroll: false })
  }

  if (!open) return null
  const appearance = remote.state.runtime_settings?.appearance || 'auto'
  const canMutate = remote.connection === 'connected'

  return (
    <div className="sheet-layer" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <section
        ref={dialogRef}
        className="bottom-sheet settings-sheet"
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
        onKeyDown={cycleFocus}
      >
        <div className="sheet-handle" />
        <div className="sheet-header">
          <div><strong>Settings</strong><span>Remote runtime controls</span></div>
          <button className="panel-icon" onClick={onClose} aria-label="Close"><X size={15} /></button>
        </div>

        <div className="settings-scroll">
          <div className="settings-section">
            <div className="settings-label"><Settings2 size={13} /> Appearance</div>
            <div className="segmented-control">
              {['auto', 'light', 'dark'].map((item) => (
                <button
                  key={item}
                  className={appearance === item ? 'is-selected' : ''}
                  onClick={() => remoteStore.setAppearance(item)}
                  disabled={!canMutate}
                >
                  {item[0].toUpperCase() + item.slice(1)}
                </button>
              ))}
            </div>
          </div>

          <div className="settings-section settings-list">
            <div className="settings-row">
              <span><strong>OpenAI Flex</strong><small>Use flex processing when available</small></span>
              <Switch
                checked={remote.state.openai_flex}
                onChange={(value) => remoteStore.setOpenAiFlex(value)}
                label="OpenAI Flex"
                disabled={!canMutate}
              />
            </div>
            <div className="settings-row">
              <span><strong>Foundation Memory</strong><small>{remote.state.foundation_memory_connected ? 'Connected' : remote.state.foundation_memory_backend}</small></span>
              <Switch
                checked={remote.state.foundation_memory_enabled}
                onChange={(value) => remoteStore.setFoundationMemory(value)}
                label="Foundation Memory"
                disabled={!canMutate}
              />
            </div>
          </div>

          {!!remote.state.available_capabilities.length && (
            <div className="settings-section">
              <div className="settings-label">Capabilities</div>
              <div className="settings-list">
                {remote.state.available_capabilities.map((capability) => (
                  <div className="settings-row" key={capability.id}>
                    <span><strong>{capability.name}</strong><small>{capability.description}</small></span>
                    <Switch
                      checked={capability.enabled}
                      onChange={() => remoteStore.toggleCapability(capability.id)}
                      label={capability.name}
                      disabled={!canMutate}
                    />
                  </div>
                ))}
              </div>
            </div>
          )}

          {!!remote.state.auth_providers.length && (
            <div className="settings-section">
              <div className="settings-label">Providers</div>
              <div className="settings-list">
                {remote.state.auth_providers.map((provider) => (
                  <div className="settings-row provider-settings-row" key={provider.provider}>
                    <ProviderIcon provider={provider.provider} size={22} background />
                    <span>
                      <strong>{providerDisplayName(provider.provider)}</strong>
                      <small>{provider.authenticated ? provider.method : provider.error || 'Not signed in'}</small>
                    </span>
                    <button
                      className="small-action"
                      onClick={() => provider.authenticated
                        ? remoteStore.authLogout(provider.provider)
                        : remoteStore.authLogin(provider.provider)}
                      disabled={!canMutate}
                    >
                      {provider.authenticated ? 'Log out' : 'Log in'}
                    </button>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      </section>
    </div>
  )
}
