import { Fragment } from 'react'
import type { ToolbarAction } from '@/remote/protocol'
import { BarChart3, CheckCircle2, ChevronRight, ChevronsUpDown, Cpu, Flag, Folder, Layers3, Lightbulb, Lock, Settings, ShieldCheck, X } from '@/components/Icons'
import { ProviderIcon } from '@/components/ProviderIcon'
import { remoteStore, useRemote } from '@/store/remoteStore'
import { formatReset, formatTokens, shortModelName } from '@/ui/format'

function Toggle({
  checked,
  onChange,
  label,
  disabled = false,
}: {
  checked: boolean
  onChange: (checked: boolean) => void
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

export function QuickPanel({
  open,
  onClose,
  onModel,
  onSettings,
}: {
  open: boolean
  onClose: () => void
  onModel: (action?: ToolbarAction) => void
  onSettings: (action?: ToolbarAction) => void
}) {
  const remote = useRemote()
  const toolbar = remote.ui?.toolbar
  const response = toolbar?.groups.find(group => group.id === 'quick_response')?.controls ?? []
  const sandboxControls = toolbar?.sandbox_controls ?? []
  const sandboxPreset = sandboxControls.find(control => control.id === 'sandbox_preset')
  const autoApprove = sandboxControls.find(control => control.id === 'auto_approve')
  const sandbox = remote.state.sandbox_settings
  const current = remote.state.current_context_tokens
  const total = remote.state.active_model_context_length
  const canMutate = remote.connection === 'connected'

  return (
    <>
      <div className={`panel-backdrop controls-backdrop${open ? ' is-open' : ''}`} onClick={onClose} aria-hidden={!open} />
      <aside className={`quick-panel${open ? ' is-open' : ''}`} aria-hidden={!open}>
        <div className="quick-panel__header">
          <span className={`connection-dot connection-dot--${remote.connection === 'connected' ? 'ok' : 'warn'}`} />
          <strong>{toolbar?.quick_title}</strong>
          <button className="panel-icon" onClick={onClose} aria-label="Close controls"><X size={15} /></button>
        </div>

        <div className="quick-panel__scroll">
          {toolbar?.quick_sections.map(section => <Fragment key={section}>
            {section === 'workspace' && (<label className="quick-workspace inset-surface">
            <Folder size={13} />
            <span>
              <strong>{remote.currentWorkspace?.display_name || remote.state.workspace_root || 'Workspace'}</strong>
              <small>{remote.currentSession?.title || 'New Chat'}</small>
            </span>
            <ChevronsUpDown className="quick-workspace__indicator" size={12} strokeWidth={1.8} />
            <select
              value={remote.currentWorkspace?.path || remote.state.workspace_root || ''}
              onChange={(event) => remoteStore.switchWorkspace(event.target.value)}
              aria-label="Workspace"
            >
              {remote.state.known_workspaces.map((workspace) => (
                <option key={workspace.id} value={workspace.path}>{workspace.display_name}</option>
              ))}
            </select>
          </label>)}
            {section === 'response' && (<div className="quick-group inset-surface">
            {response.map(control => control.kind === 'choice'
              ? <label className="quick-row" key={control.id}>
                <Lightbulb size={14} strokeWidth={1.7}/><span>{control.label}</span>
                <strong>{control.options.find(option => option.value === control.value)?.label}</strong>
                <select value={control.value} aria-label={control.label} disabled={!canMutate || !control.enabled}
                  onChange={event => remoteStore.sendToolbarUi({type:'choose',value:{id:control.id,value:event.target.value}})}>
                  {control.options.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
                </select>
              </label>
              : control.kind === 'toggle' ? <div className="quick-row" key={control.id}>
                <Flag size={13}/><span>Goal</span><Toggle checked={control.pressed ?? false} label={control.label}
                  disabled={!canMutate || !control.enabled} onChange={() => remoteStore.sendToolbarUi({type:'activate',value:control.id})}/>
              </div> : <button className="quick-row" key={control.id} disabled={!canMutate || !control.enabled}
                onClick={event => {event.currentTarget.focus({preventScroll:true});onModel({type:'activate',value:control.id})}}>
                {remote.activeProvider ? <ProviderIcon provider={remote.activeProvider} size={16}/> : <Cpu size={13}/>}
                <span>{control.label}</span><strong>{shortModelName(control.value)}</strong><ChevronRight size={13}/>
              </button>)}
          </div>)}
            {section === 'permissions' && (remote.composer?.permissions.map((permission) => (
            <div className="permission-panel inset-surface" key={`${permission.target.kind}:${permission.target.id}`}>
              <div>
                <ShieldCheck size={14} />
                <span>
                  <strong>{permission.operation || permission.title}</strong>
                  <small>{permission.detail}</small>
                </span>
              </div>
              <div className="permission-actions">
                {permission.controls.map((control, index) => (
                  <button
                    key={control.label}
                    disabled={!canMutate || !control.enabled}
                    className={index === 1 ? 'permission-primary' : undefined}
                    onClick={() => remoteStore.sendComposerUi(control.action)}
                  >{control.label}</button>
                ))}
              </div>
            </div>
          )))}
            {section === 'sandbox' && sandbox && (<div className="quick-section">
              <div className="quick-section__label"><Lock size={12} /> Sandbox</div>
              <div className="quick-group inset-surface">
                <label className="quick-row quick-row--plain">
                  <span>Preset</span>
                  <strong>{sandbox.preset}</strong>
                  <select
                    value={sandboxPreset?.value ?? sandbox.preset}
                    onChange={(event) => sandboxPreset
                      ? remoteStore.sendToolbarUi({type:'choose',value:{id:'sandbox_preset',value:event.target.value}})
                      : remoteStore.updateSandbox({type:'apply_preset',preset:event.target.value})}
                    aria-label="Sandbox preset"
                    disabled={!canMutate || (sandboxPreset ? !sandboxPreset.enabled : false)}
                  >
                    {(sandboxPreset?.options ?? ['safe', 'balanced', 'unlimited'].map(value => ({value,label:value,description:''}))).map((option) => <option key={option.value} value={option.value} title={option.description}>{option.label.toLowerCase()}</option>)}
                  </select>
                </label>
                <div className="quick-row">
                  <CheckCircle2 size={13} />
                  <span>Auto approve</span>
                  <Toggle
                    checked={autoApprove?.pressed ?? sandbox.auto_approve}
                    onChange={(enabled) => autoApprove
                      ? remoteStore.sendToolbarUi({type:'choose',value:{id:'auto_approve',value:String(enabled)}})
                      : remoteStore.updateSandbox({type:'set_auto_approve',enabled})}
                    label="Auto approve"
                    disabled={!canMutate || (autoApprove ? !autoApprove.enabled : false)}
                  />
                </div>
              </div>
            </div>)}
            {section === 'context' && (<div className="context-panel inset-surface">
              <div>
                <Layers3 size={14} strokeWidth={1.65} />
                <span>Context</span>
                <strong>{formatTokens(current ?? 0)} / {formatTokens(total ?? 0)}</strong>
              </div>
              <progress value={Math.min((current ?? 0) / (total ?? 1), 1)} max={1} />
            </div>)}
            {section === 'usage' && (<div className="usage-panel inset-surface">
            <div className="usage-panel__heading">
              <BarChart3 size={15} strokeWidth={1.7} />
              <span>
                <strong>Usage</strong>
                <small>{shortModelName(remote.state.active_model)}</small>
              </span>
            </div>
            {remote.activeProviderUsage?.windows.slice(0, 2).map((window) => (
              <div className="quota-row" key={window.id}>
                <span>
                  <strong>{window.label}</strong>
                  <small>{formatReset(window.resetsAt) ? `resets in ${formatReset(window.resetsAt)}` : ''}</small>
                </span>
                <b>{Math.round(window.remainingPercent)}%</b>
                <progress value={window.remainingPercent} max={100} />
              </div>
            ))}
            {!remote.activeProviderUsage?.windows.length && (
              <div className="usage-token-grid">
                <span><small>Input</small><strong>{formatTokens(remote.state.token_usage.input_tokens ?? 0)}</strong></span>
                <span><small>Output</small><strong>{formatTokens(remote.state.token_usage.output_tokens ?? 0)}</strong></span>
              </div>
            )}
          </div>)}
          </Fragment>)}
        </div>

        {toolbar?.groups.find(group => group.id === 'quick_footer')?.controls.map(control => <button key={control.id}
          className="quick-settings-button inset-surface" disabled={!control.enabled}
          onClick={event => {event.currentTarget.focus({preventScroll:true});onSettings({type:'activate',value:control.id})}}>
          <Settings size={14}/><span>{control.label}</span><ChevronRight size={13}/>
        </button>)}
      </aside>
    </>
  )
}
