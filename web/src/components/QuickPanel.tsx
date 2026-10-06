import { BarChart3, CheckCircle2, ChevronRight, ChevronsUpDown, Cpu, Flag, Folder, Layers3, Lightbulb, Lock, Settings, ShieldCheck, X } from '@/components/Icons'
import { ProviderIcon } from '@/components/ProviderIcon'
import { remoteStore, useRemote } from '@/store/remoteStore'
import { formatReset, formatTokens, reasoningLevelsForModel, reasoningName, shortModelName } from '@/ui/format'

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
  onModel: () => void
  onSettings: () => void
}) {
  const remote = useRemote()
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
          <strong>{remote.state.is_streaming ? 'Working' : 'Controls'}</strong>
          <button className="panel-icon" onClick={onClose} aria-label="Close controls"><X size={15} /></button>
        </div>

        <div className="quick-panel__scroll">
          <label className="quick-workspace inset-surface">
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
          </label>

          <div className="quick-group inset-surface">
            <button
              className="quick-row"
              onClick={(event) => {
                event.currentTarget.focus({ preventScroll: true })
                onModel()
              }}
              disabled={!canMutate}
            >
              {remote.activeProvider ? <ProviderIcon provider={remote.activeProvider} size={16} /> : <Cpu size={13} />}
              <span>Model</span>
              <strong>{shortModelName(remote.state.active_model)}</strong>
              <ChevronRight size={13} />
            </button>
            <label className="quick-row">
              <Lightbulb size={14} strokeWidth={1.7} />
              <span>Reasoning</span>
              <strong>{reasoningName(remote.state.active_reasoning_level)}</strong>
              <select
                value={remote.state.active_reasoning_level || 'auto'}
                onChange={(event) => remoteStore.selectReasoning(event.target.value)}
                aria-label="Reasoning"
                disabled={!canMutate}
              >
                {reasoningLevelsForModel(remote.state.active_model).map((level) => <option key={level} value={level}>{reasoningName(level)}</option>)}
              </select>
            </label>
            <div className="quick-row">
              <Flag size={13} strokeWidth={1.7} />
              <span>Goal</span>
              <Toggle
                checked={remote.state.goal_mode}
                onChange={(value) => remoteStore.setGoal(value)}
                label="Goal mode"
                disabled={!canMutate}
              />
            </div>
          </div>

          {remote.composer?.permissions.map((permission) => (
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
          ))}

          {sandbox && (
            <div className="quick-section">
              <div className="quick-section__label"><Lock size={12} /> Sandbox</div>
              <div className="quick-group inset-surface">
                <label className="quick-row quick-row--plain">
                  <span>Preset</span>
                  <strong>{sandbox.preset}</strong>
                  <select
                    value={sandbox.preset}
                    onChange={(event) => remoteStore.updateSandbox({ type: 'apply_preset', preset: event.target.value })}
                    aria-label="Sandbox preset"
                    disabled={!canMutate}
                  >
                    {['safe', 'balanced', 'unlimited'].map((preset) => <option key={preset} value={preset}>{preset}</option>)}
                  </select>
                </label>
                <div className="quick-row">
                  <CheckCircle2 size={13} />
                  <span>Auto approve</span>
                  <Toggle
                    checked={sandbox.auto_approve}
                    onChange={(enabled) => remoteStore.updateSandbox({ type: 'set_auto_approve', enabled })}
                    label="Auto approve"
                    disabled={!canMutate}
                  />
                </div>
              </div>
            </div>
          )}

          {typeof current === 'number' && typeof total === 'number' && total > 0 && (
            <div className="context-panel inset-surface">
              <div>
                <Layers3 size={14} strokeWidth={1.65} />
                <span>Context</span>
                <strong>{formatTokens(current)} / {formatTokens(total)}</strong>
              </div>
              <progress value={Math.min(current / total, 1)} max={1} />
            </div>
          )}

          <div className="usage-panel inset-surface">
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
          </div>
        </div>

        <button
          className="quick-settings-button inset-surface"
          onClick={(event) => {
            event.currentTarget.focus({ preventScroll: true })
            onSettings()
          }}
        >
          <Settings size={14} />
          <span>Settings</span>
          <ChevronRight size={13} />
        </button>
      </aside>
    </>
  )
}
