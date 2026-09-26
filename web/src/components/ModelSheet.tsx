import { useEffect, useMemo, useRef, useState } from 'react'
import { CheckCircle2, ChevronRight, Cpu, Search, X } from '@/components/Icons'
import { ProviderIcon } from '@/components/ProviderIcon'
import type { ModelCatalogItem } from '@/remote/protocol'
import { remoteStore, useRemote } from '@/store/remoteStore'

function providerDisplayName(provider: string): string {
  switch (provider.toLowerCase()) {
    case 'openai': return 'OpenAI'
    case 'opencode': return 'OpenCode'
    case 'codex-cli':
    case 'codex': return 'Codex'
    case 'anthropic': return 'Anthropic'
    case 'claude': return 'Claude'
    case 'google': return 'Google'
    case 'gemini':
    case 'gemini-web': return 'Gemini'
    case 'xai': return 'xAI'
    case 'mistral': return 'Mistral'
    case 'openrouter': return 'OpenRouter'
    case 'groq': return 'Groq'
    case 'ollama': return 'Ollama'
    case 'deepseek': return 'DeepSeek'
    case 'azure': return 'Azure'
    case 'bedrock':
    case 'aws':
    case 'amazon-bedrock': return 'Amazon Bedrock'
    default: return provider || 'Other'
  }
}

function formatContext(value: number): string {
  if (value >= 1_000_000) {
    const scaled = value / 1_000_000
    return Number.isInteger(scaled) ? `${scaled}M` : `${scaled.toFixed(1)}M`
  }
  if (value >= 1_000) {
    const scaled = value / 1_000
    return Number.isInteger(scaled) ? `${scaled}k` : `${scaled.toFixed(1)}k`
  }
  return String(value)
}

function ContextBadge({ value }: { value: number }) {
  return <span className="model-context-badge">{formatContext(value)} ctx</span>
}

function ModelRow({
  item,
  selected,
  onSelect,
  disabled,
}: {
  item: ModelCatalogItem
  selected: boolean
  onSelect: () => void
  disabled: boolean
}) {
  return (
    <button className={`model-row${selected ? ' is-selected' : ''}`} onClick={onSelect} disabled={disabled}>
      <span className="model-row__provider">
        <ProviderIcon provider={item.provider || 'other'} size={28} background />
        {selected && (
          <span className="model-row__selected-badge">
            <CheckCircle2 size={10} strokeWidth={2.2} />
          </span>
        )}
      </span>

      <span className="model-row__copy">
        <strong>{item.model || item.id}</strong>
        {item.provider && <span>{providerDisplayName(item.provider)}</span>}
      </span>

      <span className="model-row__tail">
        {typeof item.context_length === 'number' && <ContextBadge value={item.context_length} />}
        {!selected && <ChevronRight size={9} strokeWidth={2} />}
      </span>
    </button>
  )
}

function ModelSection({
  title,
  provider,
  items,
  activeModel,
  onSelect,
  disabled,
}: {
  title: string
  provider?: string
  items: ModelCatalogItem[]
  activeModel: string
  onSelect: (model: string) => void
  disabled: boolean
}) {
  if (!items.length) return null

  return (
    <section className="model-section">
      <div className="model-section__heading">
        {provider
          ? <ProviderIcon provider={provider} size={15} />
          : <Cpu size={11} strokeWidth={2} />}
        <strong>{title}</strong>
        <span>{items.length}</span>
      </div>

      <div className="model-section__rows glass-panel">
        {items.map((item, index) => (
          <div className="model-row-wrap" key={item.id}>
            <ModelRow
              item={item}
              selected={item.id === activeModel}
              onSelect={() => onSelect(item.id)}
              disabled={disabled}
            />
            {index < items.length - 1 && <div className="model-row-divider" />}
          </div>
        ))}
      </div>
    </section>
  )
}

export function ModelSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const remote = useRemote()
  const [query, setQuery] = useState('')
  const dialogRef = useRef<HTMLElement>(null)

  useEffect(() => {
    if (!open) {
      setQuery('')
      return
    }
    requestAnimationFrame(() => {
      dialogRef.current?.querySelector<HTMLInputElement>('input[placeholder="Search models"]')?.focus({ preventScroll: true })
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


  const filteredCatalog = useMemo(() => {
    const needle = query.trim().toLowerCase()
    if (!needle) return remote.state.model_catalog
    return remote.state.model_catalog.filter((item) =>
      item.id.toLowerCase().includes(needle)
      || item.model.toLowerCase().includes(needle)
      || item.provider.toLowerCase().includes(needle),
    )
  }, [query, remote.state.model_catalog])

  const fallbackModels = useMemo(() => {
    const needle = query.trim().toLowerCase()
    return remote.state.available_models
      .filter((model) => !needle || model.toLowerCase().includes(needle))
      .map<ModelCatalogItem>((model) => ({
        id: model,
        model: model.split(';').at(-1) || model,
        provider: '',
        context_length: null,
      }))
  }, [query, remote.state.available_models])

  const providers = useMemo(
    () => [...new Set(filteredCatalog.map((item) => item.provider))]
      .sort((a, b) => providerDisplayName(a).localeCompare(providerDisplayName(b))),
    [filteredCatalog],
  )

  const activeModel = remote.state.model_catalog.find((item) => item.id === remote.state.active_model) ?? null
  const visibleModelCount = remote.state.model_catalog.length ? filteredCatalog.length : fallbackModels.length

  const selectModel = (id: string) => {
    remoteStore.selectModel(id)
    onClose()
  }

  if (!open) return null

  return (
    <div className="sheet-layer" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <section
        ref={dialogRef}
        className="bottom-sheet model-sheet"
        role="dialog"
        aria-modal="true"
        aria-label="Choose model"
        onKeyDown={cycleFocus}
      >
        <div className="sheet-handle" />

        <div className="model-sheet__nav">
          <span />
          <strong>Choose Model</strong>
          <button onClick={onClose}>Done</button>
        </div>

        <div className="model-sheet__scroll">
          <label className="sheet-search glass-panel">
            <Search size={13} strokeWidth={1.9} />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search models"
              autoFocus
              autoCapitalize="none"
              autoCorrect="off"
            />
            {query && (
              <button className="model-search-clear" onClick={() => setQuery('')} aria-label="Clear search">
                <X size={10} strokeWidth={2.2} />
              </button>
            )}
          </label>

          {remote.state.model_catalog.length ? (
            <>
              {activeModel && (
                <section className="active-model-card">
                  <div className="active-model-card__label">Current Model</div>
                  <div className="active-model-card__body glass-panel">
                    <ProviderIcon provider={activeModel.provider} size={30} background />
                    <span>
                      <strong>{activeModel.model}</strong>
                      <small>{providerDisplayName(activeModel.provider)}</small>
                    </span>
                    {typeof activeModel.context_length === 'number' && <ContextBadge value={activeModel.context_length} />}
                  </div>
                </section>
              )}

              {providers.map((provider) => (
                <ModelSection
                  key={provider}
                  title={providerDisplayName(provider)}
                  provider={provider}
                  items={filteredCatalog.filter((item) => item.provider === provider)}
                  activeModel={remote.state.active_model}
                  onSelect={selectModel}
                  disabled={remote.connection !== 'connected'}
                />
              ))}
            </>
          ) : (
            <ModelSection
              title="Available"
              items={fallbackModels}
              activeModel={remote.state.active_model}
              onSelect={selectModel}
              disabled={remote.connection !== 'connected'}
            />
          )}

          {visibleModelCount === 0 && (
            <div className="model-empty">
              <Search size={20} />
              <strong>No matching models</strong>
              <span>Try searching by model name or provider.</span>
            </div>
          )}
        </div>
      </section>
    </div>
  )
}
