import { useEffect, useRef } from 'react'
import { Settings2, Database, Globe, Cpu, Brain, Layers3, ShieldCheck, Bot, KeyRound, Sparkles, X } from '@/components/Icons'
import { ProviderIcon } from '@/components/ProviderIcon'
import type { SettingsIcon } from '../../../frontend/shared/remote/protocol'
import { remoteStore, useRemote } from '@/store/remoteStore'

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
  const settings = remote.settings
  const icons = { appearance: Settings2, settings: Settings2, memory: Database, web: Globe, model: Cpu, reasoning: Brain, context: Layers3, theme: Settings2, policy: ShieldCheck, agents: Bot, permissions: ShieldCheck, provider: KeyRound, capabilities: Sparkles } satisfies Record<SettingsIcon, typeof Settings2>
  const canMutate = remote.connection === 'connected'

  return (
    <div className="sheet-layer" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <section
        ref={dialogRef}
        className="bottom-sheet settings-sheet"
        role="dialog"
        aria-modal="true"
        aria-label={settings?.title ?? 'Settings'}
        onKeyDown={cycleFocus}
      >
        <div className="sheet-handle" />
        <div className="sheet-header">
          <div><strong>{settings?.title ?? 'Settings'}</strong><span>{settings?.subtitle}</span></div>
          <button className="panel-icon" onClick={onClose} aria-label="Close"><X size={15} /></button>
        </div>

        <div className="settings-scroll">
          {settings?.sections.map((section) => {
            const SectionIcon = icons[section.icon]
            return <div className="settings-section" key={section.id}>
              <div className="settings-label"><SectionIcon size={13} /> {section.label}</div>
              <div className="settings-list">
                {section.controls.map((control) => control.kind === 'choice'
                  ? <div className="segmented-control" key={control.id} aria-label={control.label}>
                    {control.options.map((option) => <button key={option.value}
                      className={option.selected ? 'is-selected' : ''}
                      disabled={!canMutate || !control.enabled}
                      onClick={() => remoteStore.sendSettingsUi(option.action)}>{option.label}</button>)}
                  </div>
                  : <div className={`settings-row${control.provider ? ' provider-settings-row' : ''}`} key={control.id}>
                    {control.provider && <ProviderIcon provider={control.provider} size={22} background />}
                    <span><strong>{control.label}</strong><small>{control.detail}</small></span>
                    <button className={control.kind === 'toggle' ? 'ios-switch' : 'small-action'}
                      data-on={control.checked ?? undefined}
                      role={control.kind === 'toggle' ? 'switch' : undefined}
                      aria-checked={control.kind === 'toggle' ? control.checked ?? false : undefined}
                      aria-label={control.kind === 'toggle' ? control.label : undefined}
                      disabled={!canMutate || !control.enabled}
                      onClick={() => remoteStore.sendSettingsUi(control.action)}>
                      {control.kind === 'toggle' ? <span /> : control.action_label}
                    </button>
                  </div>)}
              </div>
            </div>
          })}
          {settings?.notice && <p role="status">{settings.notice}</p>}
        </div>
      </section>
    </div>
  )
}
