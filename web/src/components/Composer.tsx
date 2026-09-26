import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import {
  ArrowUp,
  BarChart3,
  ChevronRight,
  ChevronsUpDown,
  CornerDownLeft,
  FileText,
  Flag,
  Layers3,
  Lightbulb,
  MacWindow,
  Pencil,
  Photo,
  Plus,
  Square,
  Terminal,
  X,
} from '@/components/Icons'
import { ProviderIcon } from '@/components/ProviderIcon'
import { deleteAttachment, remoteFeatureSet, uploadAttachment } from '@/remote/attachments'
import { remoteStore, useRemote } from '@/store/remoteStore'
import { formatReset, formatTokens, reasoningName, shortModelName } from '@/ui/format'
import { readComposerDraft, writeComposerDraft } from '@/ui/composerDrafts'

const TOOLBAR_KEY = 'YeetRemoteComposerToolbarExpanded'
const MAX_ATTACHMENTS = 8
const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024

interface SlashCommand {
  command: string
  arguments?: string
  description: string
}

const SLASH_COMMANDS: SlashCommand[] = [
  { command: '/debate', arguments: 'TOPIC', description: 'Pro / Con / Jury debate' },
  { command: '/new', description: 'Start a new session' },
  { command: '/model', arguments: 'MODEL_ID', description: 'Select model' },
  { command: '/reasoning', arguments: 'auto|low|medium|high', description: 'Change reasoning level' },
  { command: '/sessions', description: 'View saved sessions' },
  { command: '/status', description: 'Runtime / usage status' },
  { command: '/goal', arguments: 'on|off|toggle|status', description: 'Control Goal mode' },
  { command: '/context', arguments: 'LENGTH|auto', description: 'Inspect / change context length' },
  { command: '/compact', description: 'Compact current model context' },
  { command: '/workspace', arguments: 'list|cd|add|remove|reset PATH', description: 'Manage session workspace' },
  { command: '/cd', arguments: 'PATH', description: 'Change session working directory' },
  { command: '/capabilities', description: 'Manage Skills / MCP / capabilities' },
  { command: '/attach', arguments: 'ID', description: 'Attach capability' },
  { command: '/detach', arguments: 'ID', description: 'Detach capability' },
  { command: '/skyline', arguments: 'on|off', description: 'Control Skyline coordination' },
  { command: '/image', arguments: 'PATH|clear', description: 'Queue image for next turn' },
  { command: '/permission', arguments: 'allow|deny', description: 'Respond to pending permission' },
  { command: '/allow', description: 'Allow pending permission once' },
  { command: '/deny', description: 'Deny pending permission' },
  { command: '/login', description: 'Provider authentication' },
  { command: '/provider', description: 'Custom provider settings' },
  { command: '/providers', description: 'Refresh provider list' },
  { command: '/settings', description: 'Runtime settings' },
  { command: '/permissions', description: 'Sandbox / permission settings' },
  { command: '/help', description: 'Command list' },
  { command: '/clear', description: 'Clear recent system notice' },
]

function isWeekWindowLabel(label: string): boolean {
  const value = label.toLowerCase()
  return value.includes('1w') || value.includes('week') || value.includes('7d')
}

function isFiveHourWindowLabel(label: string): boolean {
  const value = label.toLowerCase()
  return value.includes('5h')
    || value.includes('5 h')
    || value.includes('5-hour')
    || value.includes('5 hour')
}

function shortQuotaLabel(label: string): string {
  if (isFiveHourWindowLabel(label)) return '5h'
  if (isWeekWindowLabel(label)) return '1w'
  return label
}

function numericUsageField(value: unknown): number | null {
  const number = typeof value === 'number' ? value : Number(value)
  return Number.isFinite(number) ? number : null
}

interface ComposerAttachment {
  localID: string
  name: string
  mediaType: string
  byteCount: number
  previewURL?: string
  remoteID?: string
  isUploading: boolean
  error?: string
}

function attachmentAnnotation(line: string): boolean {
  const value = line.trim().toLowerCase()
  return value.startsWith('[')
    && value.endsWith(']')
    && (value.includes(' image') || value.includes(' file'))
}

function editableText(content: string): { text: string; hasAttachments: boolean } {
  const lines = content.split('\n')
  while (lines.at(-1) === '') lines.pop()

  let hasAttachments = false
  while (lines.length && attachmentAnnotation(lines.at(-1) || '')) {
    hasAttachments = true
    lines.pop()
    while (lines.at(-1) === '') lines.pop()
  }
  return { text: lines.join('\n'), hasAttachments }
}

function PermissionCard() {
  const remote = useRemote()
  const shell = remote.state.pending_shell_permission
  const native = remote.state.pending_native_app_permission
  const permission = shell ?? native
  const canRespond = remote.connection === 'connected'
  const prompt = useRef<HTMLDivElement>(null)
  const denyButton = useRef<HTMLButtonElement>(null)
  const allowButton = useRef<HTMLButtonElement>(null)
  const returnFocus = useRef<HTMLElement | null>(null)
  const previousPermission = useRef<string | null>(null)
  const previousCanRespond = useRef(canRespond)
  const permissionHadFocus = useRef(false)

  const permissionKey = shell
    ? `shell:${shell.id}`
    : native
      ? `native:${native.id}`
      : null

  const title = shell ? 'Shell permission requested' : 'Native app permission requested'
  const detail = shell
    ? shell.command
    : native
      ? `${native.appName || native.server} · ${native.tool}`
      : ''
  const reason = shell?.reason || native?.reason || ''
  const operation = shell?.operation || native?.operation || ''

  useEffect(() => {
    const previous = previousPermission.current
    if (permissionKey) {
      if (!previous) {
        const active = document.activeElement
        returnFocus.current = active instanceof HTMLElement && active !== document.body ? active : null
      }
      requestAnimationFrame(() => denyButton.current?.focus({ preventScroll: true }))
    } else if (previous) {
      const target = returnFocus.current
      const shouldRestore = permissionHadFocus.current
      returnFocus.current = null
      permissionHadFocus.current = false
      if (shouldRestore && target?.isConnected) {
        requestAnimationFrame(() => target.focus({ preventScroll: true }))
      }
    }
    previousPermission.current = permissionKey
  }, [permissionKey])

  useLayoutEffect(() => {
    if (!permissionKey || canRespond) return
    const root = prompt.current
    if (!root) return
    const active = document.activeElement
    const focusFellBackToPage = active === document.body || active === document.documentElement || active == null
    if (permissionHadFocus.current || focusFellBackToPage) {
      root.focus({ preventScroll: true })
    }
  }, [canRespond, permissionKey])

  useEffect(() => {
    const previous = previousCanRespond.current
    previousCanRespond.current = canRespond
    if (!permissionKey || previous === canRespond) return

    requestAnimationFrame(() => {
      if (!prompt.current) return
      if (!canRespond && permissionHadFocus.current) {
        prompt.current.focus({ preventScroll: true })
      } else if (canRespond && document.activeElement === prompt.current) {
        denyButton.current?.focus({ preventScroll: true })
      }
    })
  }, [canRespond, permissionKey])

  if (!permission) return null

  const describedBy = canRespond
    ? 'permission-reason permission-detail'
    : 'permission-reason permission-detail permission-connection-status'

  return (
    <div
      ref={prompt}
      className="composer-permission glass-panel"
      data-testid="permission-prompt"
      role="alertdialog"
      aria-live="assertive"
      aria-labelledby="permission-title"
      aria-describedby={describedBy}
      tabIndex={-1}
      onFocusCapture={() => {
        permissionHadFocus.current = true
      }}
      onBlurCapture={(event) => {
        const next = event.relatedTarget
        if (next instanceof Node && prompt.current?.contains(next)) return
        if (
          next instanceof HTMLElement
          && next !== document.body
          && next !== document.documentElement
        ) permissionHadFocus.current = false
      }}
      onKeyDown={(event) => {
        if (event.key !== 'Tab' || event.altKey || event.ctrlKey || event.metaKey) return
        if (!event.shiftKey && event.target === denyButton.current) {
          event.preventDefault()
          allowButton.current?.focus({ preventScroll: true })
        } else if (event.shiftKey && event.target === allowButton.current) {
          event.preventDefault()
          denyButton.current?.focus({ preventScroll: true })
        }
      }}
    >
      <div className="composer-permission__top">
        <span className="composer-permission__icon" aria-hidden="true">
          {shell ? <Terminal size={15} /> : <MacWindow size={15} />}
        </span>
        <span className="composer-permission__copy">
          <strong id="permission-title">{title}</strong>
          {operation && <small className="composer-permission__operation">{operation}</small>}
          {reason && <small id="permission-reason" className="composer-permission__reason">{reason}</small>}
          <code
            id="permission-detail"
            className="composer-permission__detail"
            tabIndex={0}
            aria-label="Requested command or tool"
          >
            {detail}
          </code>
          {!canRespond && (
            <small id="permission-connection-status" className="composer-permission__reason">
              Remote is disconnected. Reconnect to respond.
            </small>
          )}
        </span>
      </div>

      <div className="composer-permission__actions">
        <button ref={denyButton} disabled={!canRespond} onClick={() => remoteStore.denyPermission()}>Deny</button>
        <button ref={allowButton} disabled={!canRespond} className="permission-primary" onClick={() => remoteStore.allowPermission()}>Allow</button>
      </div>
    </div>
  )
}

function UsagePopover({
  anchorRef,
  onClose,
}: {
  anchorRef: React.RefObject<HTMLButtonElement | null>
  onClose: () => void
}) {
  const remote = useRemote()
  const popover = useRef<HTMLDivElement>(null)
  const onCloseRef = useRef(onClose)
  onCloseRef.current = onClose
  const [position, setPosition] = useState({ left: 10, top: 10, width: 286 })
  const [positioned, setPositioned] = useState(false)
  const usage = remote.activeProviderUsage
  const windows = usage?.windows ?? []
  const cacheHitRate = numericUsageField(remote.state.token_usage.cache_hit_rate)
  const estimatedCost = numericUsageField(remote.state.token_usage.estimated_cost_usd)
  const modelCalls = numericUsageField(remote.state.token_usage.model_calls)
  const subtitle = remote.activeProvider
    ? usage?.plan?.trim()
      ? `${remote.activeProvider} · ${usage.plan.trim()}`
      : remote.activeProvider
    : 'Current model'

  useLayoutEffect(() => {
    let frame = 0
    const viewport = window.visualViewport

    const updatePosition = () => {
      if (frame) cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => {
        frame = 0
        const anchor = anchorRef.current
        if (!anchor) return

        const rect = anchor.getBoundingClientRect()
        const viewportLeft = viewport?.offsetLeft ?? 0
        const viewportTop = viewport?.offsetTop ?? 0
        const viewportWidth = viewport?.width ?? window.innerWidth
        const viewportHeight = viewport?.height ?? window.innerHeight
        const width = Math.min(304, Math.max(220, viewportWidth - 20))
        const panelHeight = popover.current?.offsetHeight ?? 260
        const gap = 10
        const minLeft = viewportLeft + gap
        const maxLeft = viewportLeft + viewportWidth - width - gap
        const left = Math.min(Math.max(minLeft, rect.right - width), Math.max(minLeft, maxLeft))
        const minTop = viewportTop + gap
        const maxTop = Math.max(minTop, viewportTop + viewportHeight - panelHeight - gap)
        const above = rect.top - panelHeight - gap
        const below = rect.bottom + gap
        const top = above >= minTop
          ? above
          : Math.min(Math.max(minTop, below), maxTop)

        setPosition({ left, top, width })
        setPositioned(true)
      })
    }

    const dismiss = (event: PointerEvent) => {
      const target = event.target
      const anchor = anchorRef.current
      if (!(target instanceof Node)) return
      if (anchor?.contains(target) || popover.current?.contains(target)) return
      onCloseRef.current()
    }
    const dismissOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onCloseRef.current()
    }

    updatePosition()
    window.addEventListener('resize', updatePosition)
    viewport?.addEventListener('resize', updatePosition)
    viewport?.addEventListener('scroll', updatePosition)
    document.addEventListener('scroll', updatePosition, true)
    document.addEventListener('pointerdown', dismiss, true)
    document.addEventListener('keydown', dismissOnEscape)
    const resizeObserver = typeof ResizeObserver === 'undefined'
      ? null
      : new ResizeObserver(updatePosition)
    if (anchorRef.current) resizeObserver?.observe(anchorRef.current)
    if (popover.current) resizeObserver?.observe(popover.current)

    return () => {
      if (frame) cancelAnimationFrame(frame)
      resizeObserver?.disconnect()
      window.removeEventListener('resize', updatePosition)
      viewport?.removeEventListener('resize', updatePosition)
      viewport?.removeEventListener('scroll', updatePosition)
      document.removeEventListener('scroll', updatePosition, true)
      document.removeEventListener('pointerdown', dismiss, true)
      document.removeEventListener('keydown', dismissOnEscape)
    }
  }, [anchorRef])

  return createPortal(
    <div
      ref={popover}
      id="provider-usage-popover"
      className="usage-popover glass-panel"
      role="dialog"
      aria-label="Usage details"
      data-positioned={positioned}
      style={{ left: position.left, top: position.top, width: position.width }}
    >
      <div className="usage-popover__header">
        <ProviderIcon provider={remote.activeProvider} size={20} background />
        <span>
          <strong>{shortModelName(remote.state.active_model)}</strong>
          <small>{subtitle}</small>
        </span>
        <button className="panel-icon" onClick={onClose} aria-label="Close usage"><X size={15} /></button>
      </div>

      <div className="usage-token-grid">
        <span><small>Input</small><strong>{formatTokens(remote.state.token_usage.input_tokens ?? 0)}</strong></span>
        <span><small>Output</small><strong>{formatTokens(remote.state.token_usage.output_tokens ?? 0)}</strong></span>
        {cacheHitRate != null ? (
          <span><small>Cache</small><strong>{Math.round(cacheHitRate * 100)}%</strong></span>
        ) : (
          <span><small>Cached</small><strong>{formatTokens(remote.state.token_usage.cached_input_tokens ?? 0)}</strong></span>
        )}
      </div>

      {windows.length > 0 ? (
        <div className="usage-quota-list">
          {windows.slice(0, 2).map((window) => (
            <div className="quota-row" key={window.id}>
              <span>
                <strong>{window.label}</strong>
                <small>{formatReset(window.resetsAt) ? `resets in ${formatReset(window.resetsAt)}` : ''}</small>
              </span>
              <b>{Math.round(window.remainingPercent)}%</b>
              <progress value={window.remainingPercent} max={100} />
            </div>
          ))}
        </div>
      ) : (
        <div className="usage-compact-row">
          {estimatedCost != null && estimatedCost > 0 && <span>{'$'}{estimatedCost.toFixed(4)}</span>}
          {remote.state.credit_usage > 0 && <span>{Math.trunc(remote.state.credit_usage)} credits</span>}
          {modelCalls != null && modelCalls > 0 && <span>{Math.trunc(modelCalls)} calls</span>}
        </div>
      )}
    </div>,
    document.body,
  )
}

function AttachmentChip({
  attachment,
  onRemove,
}: {
  attachment: ComposerAttachment
  onRemove: () => void
}) {
  const isImage = attachment.mediaType.startsWith('image/')
  const size = attachment.byteCount < 1024 * 1024
    ? `${Math.max(1, Math.round(attachment.byteCount / 1024))} KB`
    : `${(attachment.byteCount / (1024 * 1024)).toFixed(1)} MB`

  return (
    <div className={`attachment-chip${isImage ? ' is-image' : ''}`}>
      <div className="attachment-chip__body">
        {isImage ? (
          attachment.previewURL
            ? <img src={attachment.previewURL} alt="" />
            : <Photo size={18} />
        ) : (
          <>
            <FileText size={18} className="attachment-chip__file-icon" />
            <span className="attachment-chip__copy">
              <strong>{attachment.name}</strong>
              <small>{attachment.mediaType} · {size}</small>
            </span>
          </>
        )}
      </div>

      {attachment.isUploading && (
        <div className="attachment-chip__state"><span className="mini-spinner" /></div>
      )}
      {!attachment.isUploading && attachment.error && (
        <div className="attachment-chip__state is-error">!</div>
      )}

      <button className="attachment-chip__remove" onClick={onRemove} aria-label="Remove attachment">
        <X size={9} strokeWidth={2.4} />
      </button>
    </div>
  )
}

export function Composer({
  onModel,
  onSessions,
  onSettings,
  editRequest,
  onEditConsumed,
}: {
  onModel: () => void
  onSessions: () => void
  onSettings: () => void
  editRequest: { key: number; content: string } | null
  onEditConsumed: () => void
}) {
  const remote = useRemote()
  const draftContextKey = useMemo(() => JSON.stringify([
    remote.state.workspace_root ?? '',
    remote.state.current_session_id ?? null,
    remote.state.current_session_id == null ? remote.sessionResetRevision : 0,
  ]), [
    remote.state.workspace_root,
    remote.state.current_session_id,
    remote.sessionResetRevision,
  ])
  const draftContextKeyRef = useRef(draftContextKey)
  const [text, setText] = useState(() => readComposerDraft(draftContextKey))
  const [expanded, setExpanded] = useState(() => {
    try {
      const stored = localStorage.getItem(TOOLBAR_KEY)
      return stored == null ? true : stored === 'true'
    } catch {
      return true
    }
  })
  const [usageOpen, setUsageOpen] = useState(false)
  const [features, setFeatures] = useState<Set<string>>(new Set())
  const [attachments, setAttachments] = useState<ComposerAttachment[]>([])
  const [attachmentMenu, setAttachmentMenu] = useState(false)
  const [isEditingLast, setIsEditingLast] = useState(false)
  const [editingLastHasAttachments, setEditingLastHasAttachments] = useState(false)
  const textarea = useRef<HTMLTextAreaElement>(null)
  const imageInput = useRef<HTMLInputElement>(null)
  const fileInput = useRef<HTMLInputElement>(null)
  const composerShell = useRef<HTMLDivElement>(null)
  const usageButton = useRef<HTMLButtonElement>(null)

  const supportsAttachments = features.has('attachments-v1')
  const supportsFiles = features.has('files-v1')
  const hasAttachmentSupport = supportsAttachments || supportsFiles

  useEffect(() => {
    const previousKey = draftContextKeyRef.current
    if (previousKey === draftContextKey) return

    if (!isEditingLast) writeComposerDraft(previousKey, text)
    draftContextKeyRef.current = draftContextKey
    setText(readComposerDraft(draftContextKey))
    setIsEditingLast(false)
    setEditingLastHasAttachments(false)
    setAttachmentMenu(false)
  }, [draftContextKey])

  useEffect(() => {
    if (!isEditingLast) writeComposerDraft(draftContextKeyRef.current, text)
  }, [text, isEditingLast])

  useEffect(() => {
    void remoteFeatureSet().then(setFeatures).catch(() => setFeatures(new Set()))
  }, [])

  useEffect(() => {
    const node = textarea.current
    if (!node) return
    node.style.height = '0px'
    node.style.height = `${Math.min(Math.max(node.scrollHeight, 24), 150)}px`
  }, [text])

  useEffect(() => {
    const shell = composerShell.current
    if (!shell || typeof ResizeObserver === 'undefined') return
    const host = shell.closest<HTMLElement>('.main-viewport')
    if (!host) return

    const updateClearance = () => {
      const rect = shell.getBoundingClientRect()
      const viewport = window.visualViewport
      const viewportBottom = (viewport?.offsetTop ?? 0) + (viewport?.height ?? window.innerHeight)
      host.style.setProperty('--composer-overlay-clearance', `${Math.max(0, viewportBottom - rect.top)}px`)
    }

    const observer = new ResizeObserver(updateClearance)
    observer.observe(shell)
    window.addEventListener('resize', updateClearance)
    window.visualViewport?.addEventListener('resize', updateClearance)
    window.visualViewport?.addEventListener('scroll', updateClearance)
    updateClearance()

    return () => {
      observer.disconnect()
      window.removeEventListener('resize', updateClearance)
      window.visualViewport?.removeEventListener('resize', updateClearance)
      window.visualViewport?.removeEventListener('scroll', updateClearance)
      host.style.removeProperty('--composer-overlay-clearance')
    }
  }, [])

  useEffect(() => {
    if (!editRequest) return

    const prepared = editableText(editRequest.content)
    for (const attachment of attachments) {
      if (attachment.previewURL) URL.revokeObjectURL(attachment.previewURL)
      if (attachment.remoteID) void deleteAttachment(attachment.remoteID).catch(() => {})
    }
    setAttachments([])
    setAttachmentMenu(false)
    setText(prepared.text)
    setEditingLastHasAttachments(prepared.hasAttachments)
    setIsEditingLast(true)
    onEditConsumed()
    requestAnimationFrame(() => textarea.current?.focus())
  }, [editRequest])

  const usageLabel = useMemo(() => {
    const usage = remote.activeProviderUsage
    if (usage?.available && usage.windows.length) {
      const week = usage.windows.find((window) => isWeekWindowLabel(window.label))
        ?? usage.windows[0]
      const fiveHour = usage.windows.find((window) => isFiveHourWindowLabel(window.label))

      if (week && fiveHour && week.id !== fiveHour.id) {
        return `${shortQuotaLabel(week.label)} ${Math.round(week.remainingPercent)}% · ${shortQuotaLabel(fiveHour.label)} ${Math.round(fiveHour.remainingPercent)}%`
      }

      if (week) {
        const plan = usage.plan?.trim() || usage.provider
        return `${shortQuotaLabel(week.label)} ${Math.round(week.remainingPercent)}% · ${plan}`
      }
    }

    const estimatedCost = numericUsageField(remote.state.token_usage.estimated_cost_usd)
    if (estimatedCost != null && estimatedCost > 0) return `$${estimatedCost.toFixed(3)}`
    if (remote.state.credit_usage > 0) return `${Math.trunc(remote.state.credit_usage)} cr`

    const explicitTotal = numericUsageField(remote.state.token_usage.total_tokens)
    const total = explicitTotal
      ?? ((remote.state.token_usage.input_tokens ?? 0) + (remote.state.token_usage.output_tokens ?? 0))
    return total > 0 ? formatTokens(total) : 'API'
  }, [remote.activeProviderUsage, remote.state.credit_usage, remote.state.token_usage])

  const slashSuggestions = useMemo(() => {
    const value = text.trim()
    if (!value.startsWith('/') || value.includes(' ') || value.includes('\n')) return []
    const query = value.toLowerCase()
    return SLASH_COMMANDS
      .filter((item) => query === '/' || item.command.toLowerCase().startsWith(query))
      .slice(0, 7)
  }, [text])

  const attachmentsReady = attachments.every(
    (attachment) => !attachment.isUploading && !attachment.error && Boolean(attachment.remoteID),
  )

  const hasContent = Boolean(text.trim())
    || attachments.some((attachment) => Boolean(attachment.remoteID))
    || (isEditingLast && editingLastHasAttachments)

  const canSend = remote.connection === 'connected'
    && !remote.state.is_streaming
    && (isEditingLast || attachmentsReady)
    && hasContent

  const clearEditing = () => {
    setText(readComposerDraft(draftContextKeyRef.current))
    setIsEditingLast(false)
    setEditingLastHasAttachments(false)
  }

  const selectSlashCommand = (item: SlashCommand) => {
    const { command } = item
    if (command === '/model') {
      setText('')
      onModel()
      return
    }
    if (command === '/sessions') {
      setText('')
      onSessions()
      return
    }
    if (['/settings', '/permissions', '/provider', '/providers', '/login', '/capabilities'].includes(command)) {
      setText('')
      onSettings()
      return
    }
    setText(command + (item.arguments ? ' ' : ''))
    requestAnimationFrame(() => textarea.current?.focus())
  }

  const submit = () => {
    const value = text.trim()
    if (remote.connection !== 'connected' || remote.state.is_streaming) return

    if (!isEditingLast) {
      if (value === '/model') {
        setText('')
        onModel()
        return
      }
      if (value === '/sessions') {
        setText('')
        onSessions()
        return
      }
      if (['/settings', '/permissions', '/provider', '/providers', '/login', '/capabilities'].includes(value)) {
        setText('')
        onSettings()
        return
      }
    }

    if (isEditingLast) {
      if (!value && !editingLastHasAttachments) return
      if (remoteStore.editLast(value)) clearEditing()
      return
    }

    const ids = attachments.flatMap((attachment) => attachment.remoteID ? [attachment.remoteID] : [])
    if (!attachmentsReady || (!value && !ids.length)) return
    if (remoteStore.submit(value, ids)) {
      setText('')
      for (const attachment of attachments) {
        if (attachment.previewURL) URL.revokeObjectURL(attachment.previewURL)
      }
      setAttachments([])
    }
  }

  const toggleExpanded = () => {
    setUsageOpen(false)
    setExpanded((value) => {
      const next = !value
      try { localStorage.setItem(TOOLBAR_KEY, String(next)) } catch { /* unavailable */ }
      return next
    })
  }

  const removeAttachment = (localID: string) => {
    const attachment = attachments.find((candidate) => candidate.localID === localID)
    if (!attachment) return
    setAttachments((current) => current.filter((candidate) => candidate.localID !== localID))
    if (attachment.previewURL) URL.revokeObjectURL(attachment.previewURL)
    if (attachment.remoteID) void deleteAttachment(attachment.remoteID).catch(() => {})
  }

  const addFiles = (files: FileList | null, imagesOnly: boolean) => {
    if (!files?.length) return
    const remaining = Math.max(0, MAX_ATTACHMENTS - attachments.length)
    if (!remaining) return

    for (const file of Array.from(files).slice(0, remaining)) {
      if (imagesOnly && !file.type.startsWith('image/')) continue
      const localID = crypto.randomUUID()
      const previewURL = file.type.startsWith('image/') ? URL.createObjectURL(file) : undefined
      const tooLarge = file.size > MAX_ATTACHMENT_BYTES
      const draft: ComposerAttachment = {
        localID,
        name: file.name || `attachment-${localID.slice(0, 8)}`,
        mediaType: file.type || 'application/octet-stream',
        byteCount: file.size,
        previewURL,
        isUploading: !tooLarge,
        error: tooLarge ? 'Files must be 20 MiB or smaller.' : undefined,
      }

      setAttachments((current) => [...current, draft])
      if (tooLarge) continue

      void uploadAttachment(file, draft.name, draft.mediaType)
        .then((uploaded) => {
          setAttachments((current) => current.map((attachment) =>
            attachment.localID === localID
              ? { ...attachment, remoteID: uploaded.id, isUploading: false }
              : attachment,
          ))
        })
        .catch((error) => {
          setAttachments((current) => current.map((attachment) =>
            attachment.localID === localID
              ? {
                  ...attachment,
                  isUploading: false,
                  error: error instanceof Error ? error.message : String(error),
                }
              : attachment,
          ))
        })
    }

    setAttachmentMenu(false)
    if (imageInput.current) imageInput.current.value = ''
    if (fileInput.current) fileInput.current.value = ''
  }

  return (
    <div ref={composerShell} className="composer-shell">
      <PermissionCard />

{slashSuggestions.length > 0 && (
        <div className="slash-palette glass-panel">
          {slashSuggestions.map((item, index) => (
            <button key={item.command} onClick={() => selectSlashCommand(item)}>
              <span className="slash-palette__copy">
                <span className="slash-palette__command-line">
                  <strong>{item.command}</strong>
                  {item.arguments && <small>{item.arguments}</small>}
                </span>
                <span className="slash-palette__description">{item.description}</span>
              </span>
              <CornerDownLeft size={11} />
              {index < slashSuggestions.length - 1 && <span className="slash-palette__divider" />}
            </button>
          ))}
        </div>
      )}

      <div className={`composer-toolbar${expanded ? ' is-expanded' : ' is-collapsed'}`}>
        <div className="composer-toolbar__scroller">
          <div className="composer-toolbar__controls">
            <button
              className="toolbar-chip glass-capsule"
              onClick={(event) => {
                event.currentTarget.focus({ preventScroll: true })
                onModel()
              }}
              aria-label={`Choose model, current ${shortModelName(remote.state.active_model)}`}
              disabled={remote.connection !== 'connected'}
              title={remote.state.is_streaming ? 'Model changes apply to the next response.' : 'Choose model'}
            >
              <ProviderIcon provider={remote.activeProvider} size={16} />
              <span>{shortModelName(remote.state.active_model)}</span>
              <ChevronsUpDown size={10} />
            </button>

            <label className="toolbar-chip glass-capsule toolbar-select-chip">
              <Lightbulb size={15} strokeWidth={1.7} />
              <span>{reasoningName(remote.state.active_reasoning_level)}</span>
              <select
                value={remote.state.active_reasoning_level || 'auto'}
                onChange={(event) => remoteStore.selectReasoning(event.target.value)}
                aria-label="Reasoning"
                disabled={remote.connection !== 'connected'}
              >
                {['auto', 'low', 'medium', 'high'].map((level) => <option key={level} value={level}>{reasoningName(level)}</option>)}
              </select>
            </label>

            <button
              className={`toolbar-chip glass-capsule goal-chip${remote.state.goal_mode ? ' is-active' : ''}`}
              onClick={() => remoteStore.setGoal(!remote.state.goal_mode)}
              disabled={remote.connection !== 'connected'}
              aria-pressed={remote.state.goal_mode}
              title={remote.state.is_streaming ? 'Goal changes apply to the next response.' : 'Goal mode'}
            >
              <Flag size={14} fill={remote.state.goal_mode ? 'currentColor' : 'none'} />
              <span>Goal</span>
            </button>

            <button
              className="toolbar-chip glass-capsule"
              title={remote.contextPercent == null ? 'Context information unavailable' : `Context ${remote.contextPercent}%`}
            >
              <Layers3 size={15} strokeWidth={1.65} />
              <span>{remote.contextPercent == null ? 'Context' : `${remote.contextPercent}%`}</span>
            </button>

            <div className="usage-control">
              <button
                ref={usageButton}
                className={`toolbar-chip glass-capsule usage-trigger${usageOpen ? ' is-open' : ''}`}
                type="button"
                onClick={(event) => {
                  event.currentTarget.focus({ preventScroll: true })
                  setUsageOpen((value) => {
                    const next = !value
                    if (next) remoteStore.refreshProviderUsage()
                    return next
                  })
                }}
                aria-expanded={usageOpen}
                aria-controls="provider-usage-popover"
                aria-haspopup="dialog"
                aria-pressed={usageOpen}
                aria-label={`Usage: ${usageLabel}`}
              >
                <BarChart3 size={15} strokeWidth={1.7} />
                <span>{usageLabel}</span>
              </button>
              {usageOpen && <UsagePopover anchorRef={usageButton} onClose={() => setUsageOpen(false)} />}
            </div>
          </div>
        </div>

        <button className="toolbar-collapse glass-circle" onClick={toggleExpanded} aria-label={expanded ? 'Collapse toolbar' : 'Expand toolbar'}>
          <ChevronRight size={12} className={expanded ? 'is-expanded' : ''} />
        </button>
      </div>

      {isEditingLast && (
        <div className="edit-last-banner glass-panel">
          <Pencil size={12} />
          <span>
            <strong>Editing last message</strong>
            {editingLastHasAttachments && <small>Existing attachments will be preserved.</small>}
          </span>
          <button
            onClick={clearEditing}
            aria-label="Cancel editing"
          >
            <X size={11} strokeWidth={2.2} />
          </button>
        </div>
      )}

      {!!attachments.length && (
        <div className="attachment-strip">
          {attachments.map((attachment) => (
            <AttachmentChip
              key={attachment.localID}
              attachment={attachment}
              onRemove={() => removeAttachment(attachment.localID)}
            />
          ))}
        </div>
      )}

      {attachments.some((attachment) => attachment.error) && (
        <div className="attachment-error">
          {attachments.find((attachment) => attachment.error)?.error}
        </div>
      )}

      <div className="composer-row">
        {hasAttachmentSupport && (
          <div className="attachment-add-wrap">
            <button
              className="attachment-add glass-circle"
              onClick={() => setAttachmentMenu((value) => !value)}
              disabled={isEditingLast || attachments.length >= MAX_ATTACHMENTS || remote.state.is_streaming || remote.connection !== 'connected'}
              aria-label="Add attachment"
            >
              <Plus size={16} strokeWidth={2} />
            </button>

            {attachmentMenu && (
              <div className="attachment-menu glass-panel">
                {supportsAttachments && (
                  <button onClick={() => imageInput.current?.click()}>
                    <Photo size={14} />
                    <span>Photo</span>
                  </button>
                )}
                {supportsFiles && (
                  <button onClick={() => fileInput.current?.click()}>
                    <FileText size={14} />
                    <span>File</span>
                  </button>
                )}
              </div>
            )}

            <input
              ref={imageInput}
              hidden
              type="file"
              accept="image/*"
              multiple
              onChange={(event) => addFiles(event.target.files, true)}
            />
            <input
              ref={fileInput}
              hidden
              type="file"
              multiple
              onChange={(event) => addFiles(event.target.files, false)}
            />
          </div>
        )}

        <div className="composer-input glass-input">
          <textarea
            ref={textarea}
            value={text}
            onChange={(event) => setText(event.target.value)}
            onKeyDown={(event) => {
              if (event.key !== 'Enter' || event.shiftKey || event.nativeEvent.isComposing) return
              const explicitSend = event.metaKey || event.ctrlKey
              const touchFirst = window.matchMedia('(hover: none) and (pointer: coarse)').matches
              if (!explicitSend && (touchFirst || text.includes('\n'))) return
              event.preventDefault()
              submit()
            }}
            placeholder={
              remote.state.is_streaming
                ? 'Draft your next message…'
                : remote.connection === 'connected'
                  ? 'Message'
                  : 'Draft while reconnecting…'
            }
            rows={1}
            aria-label="Message"
          />
        </div>

        {remote.state.is_streaming ? (
          <button className="composer-send glass-circle is-stop" onClick={() => remoteStore.interrupt()} aria-label="Stop">
            <Square size={13} fill="currentColor" />
          </button>
        ) : (
          <button
            className="composer-send glass-circle"
            onClick={submit}
            disabled={!canSend}
            aria-label="Send"
          >
            <ArrowUp size={16} strokeWidth={2} />
          </button>
        )}
      </div>
    </div>
  )
}
