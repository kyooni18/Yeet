import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import DOMPurify from 'dompurify'
import MarkdownIt from 'markdown-it'
import {
  BookOpen,
  Check,
  ChevronDown,
  CircleAlert,
  Copy,
  FileText,
  Folder,
  Globe,
  Info,
  Lightbulb,
  MacWindow,
  Pencil,
  Plug,
  RotateCw,
  Search,
  Terminal,
  TriangleAlert,
  Wrench,
} from '@/components/Icons'
import type { ConversationEntry as Entry, ConversationToolCall, ModelActivity } from '@/remote/protocol'
import { remoteStore, useRemote } from '@/store/remoteStore'

const markdown = new MarkdownIt({ html: false, linkify: true, breaks: true, typographer: true })

markdown.renderer.rules.fence = (tokens, index) => {
  const token = tokens[index]
  const language = token.info.trim().split(/\s+/)[0] || ''
  const escapedLanguage = markdown.utils.escapeHtml(language)
  const label = markdown.utils.escapeHtml(language ? `${language} code block` : 'Code block')
  const codeClass = escapedLanguage ? ` class="language-${escapedLanguage}"` : ''
  return `<pre tabindex="0" role="region" aria-label="${label}"><code${codeClass}>${markdown.utils.escapeHtml(token.content)}</code></pre>\n`
}

markdown.renderer.rules.code_block = (tokens, index) =>
  `<pre tabindex="0" role="region" aria-label="Code block"><code>${markdown.utils.escapeHtml(tokens[index].content)}</code></pre>\n`

function Markdown({ content }: { content: string }) {
  const html = useMemo(() => DOMPurify.sanitize(markdown.render(content || '')), [content])
  return <div className="markdown" dangerouslySetInnerHTML={{ __html: html }} />
}

async function writeClipboardText(text: string): Promise<boolean> {
  const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null

  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    // Fall through to the selection-based fallback.
  }

  const textarea = document.createElement('textarea')
  textarea.value = text
  textarea.setAttribute('readonly', '')
  textarea.style.position = 'fixed'
  textarea.style.opacity = '0'
  textarea.style.pointerEvents = 'none'
  textarea.style.inset = '-10000px auto auto -10000px'
  document.body.append(textarea)

  let copied = false
  try {
    textarea.focus({ preventScroll: true })
    textarea.select()
    copied = document.execCommand('copy')
  } catch {
    copied = false
  } finally {
    textarea.remove()
    previousFocus?.focus({ preventScroll: true })
  }

  return copied
}

interface TraceDetail {
  title: string
  content: string
  monospaced?: boolean
  isError?: boolean
  hasBackground?: boolean
}

function lastUsefulLine(value: string): string {
  return value
    .replaceAll('****', '\n')
    .replaceAll('**', '')
    .split('\n')
    .map((line) => line.trim().replace(/^#{1,6}\s+/, '').replace(/^[*_`]+|[*_`]+$/g, '').trim())
    .filter(Boolean)
    .at(-1) ?? ''
}

function firstUsefulLine(value: string): string {
  return value
    .replaceAll('**', '')
    .split('\n')
    .map((line) => line.trim())
    .find(Boolean) ?? ''
}

function TraceDisclosure({
  icon,
  title,
  summary,
  status,
  isActive = false,
  details = [],
  metadata,
}: {
  icon: ReactNode
  title: string
  summary?: string | null
  status?: string | null
  isActive?: boolean
  details?: TraceDetail[]
  metadata?: string | null
}) {
  const [open, setOpen] = useState(false)
  const expandable = details.length > 0

  return (
    <div className="trace-disclosure">
      <button
        className={`trace-disclosure__row${expandable ? ' is-expandable' : ''}`}
        onClick={() => expandable && setOpen((value) => !value)}
        aria-expanded={expandable ? open : undefined}
      >
        <span className="trace-disclosure__icon">{icon}</span>
        <span className={`trace-disclosure__title${isActive ? ' is-active' : ''}`}>{title}</span>
        {summary && (
          <>
            <span className="trace-disclosure__dot">·</span>
            <span className="trace-disclosure__summary">{summary}</span>
          </>
        )}
        <span className="trace-disclosure__spacer" />
        {status && <span className="trace-disclosure__status">{status}</span>}
        {metadata && <span className="trace-disclosure__metadata">{metadata}</span>}
        {expandable && <ChevronDown size={9} className={open ? 'is-open' : ''} />}
      </button>

      {open && expandable && (
        <div className="trace-disclosure__details">
          {details.map((detail, index) => {
            const scrollable = detail.content.length > 600 || detail.content.split('\n').length > 12
            const detailLabel = `${detail.title || title} details`
            return (
              <div
                className={`trace-detail${detail.hasBackground ? ' has-background' : ''}`}
                key={`${detail.title}-${index}`}
              >
                {detail.title && <div className="trace-detail__title">{detail.title}</div>}
                <div
                  className={[
                    'trace-detail__content',
                    detail.monospaced ? 'is-monospaced' : '',
                    detail.isError ? 'is-error' : '',
                    scrollable ? 'is-scrollable' : '',
                  ].filter(Boolean).join(' ')}
                  tabIndex={scrollable ? 0 : undefined}
                  role={scrollable ? 'region' : undefined}
                  aria-label={scrollable ? detailLabel : undefined}
                >
                  {detail.content}
                </div>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}

interface ToolEditFileDelta {
  path: string
  operation?: string | null
  added?: number
  removed?: number
  previewLines: string[]
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (typeof value === 'string') {
    const raw = value.trim()
    if (!raw) return null
    try {
      const parsed = JSON.parse(raw)
      return parsed && typeof parsed === 'object' && !Array.isArray(parsed)
        ? parsed as Record<string, unknown>
        : null
    } catch {
      return null
    }
  }

  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null
}

function parseToolArguments(tool: ConversationToolCall): Record<string, unknown> | null {
  return asRecord(tool.arguments)
}

function stringField(args: Record<string, unknown> | null, ...keys: string[]): string | null {
  if (!args) return null
  for (const key of keys) {
    const value = args[key]
    if (typeof value === 'string' && value.trim()) return value.trim()
  }
  return null
}

function compactPath(value: string): string {
  const normalized = value.replaceAll('\\', '/')
  const pieces = normalized.split('/').filter(Boolean)
  return pieces.length > 2 ? pieces.slice(-2).join('/') : normalized
}

function readPaths(tool: ConversationToolCall): string[] {
  const args = parseToolArguments(tool)
  if (!args) return []

  const values: string[] = []
  const direct = args.path
  if (typeof direct === 'string' && direct) values.push(direct)

  if (Array.isArray(args.requests)) {
    for (const request of args.requests) {
      const record = asRecord(request)
      if (record && typeof record.path === 'string' && record.path) values.push(record.path)
    }
  }

  return [...new Set(values)]
}

function diffInfo(diff: Record<string, unknown> | null): {
  added?: number
  removed?: number
  previewLines: string[]
} {
  if (!diff || !Array.isArray(diff.hunks)) return { previewLines: [] }

  let added = 0
  let removed = 0
  const previewLines: string[] = []

  for (const hunkValue of diff.hunks) {
    const hunk = asRecord(hunkValue)
    if (!hunk || !Array.isArray(hunk.lines)) continue

    for (const lineValue of hunk.lines) {
      if (typeof lineValue !== 'string') continue
      if (lineValue.startsWith('+')) added += 1
      if (lineValue.startsWith('-')) removed += 1
      if (
        previewLines.length < 8
        && (lineValue.startsWith('+') || lineValue.startsWith('-'))
      ) previewLines.push(lineValue)
    }
  }

  return { added, removed, previewLines }
}

function editFilesFromArguments(tool: ConversationToolCall): ToolEditFileDelta[] {
  const args = parseToolArguments(tool)
  if (!args || !Array.isArray(args.changes)) return []

  const values: ToolEditFileDelta[] = []
  for (const changeValue of args.changes) {
    const change = asRecord(changeValue)
    if (!change || typeof change.path !== 'string' || !change.path) continue

    const fileOp = asRecord(change.fileOp)
    const operation = typeof fileOp?.kind === 'string' ? fileOp.kind : 'update'
    const destination = typeof fileOp?.destination === 'string' && fileOp.destination
      ? fileOp.destination
      : null

    values.push({
      path: destination || change.path,
      operation,
      previewLines: [],
    })
  }
  return values
}

function editFiles(tool: ConversationToolCall): ToolEditFileDelta[] {
  const argumentFiles = editFilesFromArguments(tool)
  const result = asRecord(tool.result)
  if (!result || !Array.isArray(result.files)) return argumentFiles

  const resultFiles: ToolEditFileDelta[] = []
  for (const fileValue of result.files) {
    const file = asRecord(fileValue)
    if (!file) continue

    const source = typeof file.path === 'string' ? file.path : ''
    const destination = typeof file.destination === 'string' ? file.destination : ''
    const path = destination || source
    if (!path) continue

    const info = diffInfo(asRecord(file.diff))
    resultFiles.push({
      path,
      operation: typeof file.operation === 'string' ? file.operation : null,
      added: info.added,
      removed: info.removed,
      previewLines: info.previewLines,
    })
  }

  if (!resultFiles.length) return argumentFiles
  const resultPaths = new Set(resultFiles.map((file) => file.path))
  return resultFiles.concat(argumentFiles.filter((file) => !resultPaths.has(file.path)))
}

function editDeltaText(file: ToolEditFileDelta): string | null {
  if (file.added == null || file.removed == null) return null
  if (file.added === 0 && file.removed === 0) return null
  if (file.removed === 0) return `+${file.added}`
  if (file.added === 0) return `−${file.removed}`
  return `+${file.added} −${file.removed}`
}

function editFileDescription(file: ToolEditFileDelta): string {
  const delta = editDeltaText(file)
  if (delta) return `${file.path}  ${delta}`
  if (file.operation) return `${file.path}  [${file.operation}]`
  return file.path
}

function editSummary(tool: ConversationToolCall): string | null {
  const files = editFiles(tool)
  const first = files[0]
  if (!first) return stringField(parseToolArguments(tool), 'purpose') || tool.detail || null

  const delta = editDeltaText(first)
  const firstText = [compactPath(first.path), delta].filter(Boolean).join(' ')
  return files.length === 1 ? firstText : `${firstText} · +${files.length - 1} files`
}

function formatUnknown(value: unknown): string {
  if (typeof value === 'string') return value
  if (value == null) return ''
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}

function toolIsActive(tool: ConversationToolCall): boolean {
  return ['preparing', 'running', 'started', 'awaiting_permission'].includes(tool.status as string)
}

function toolTitle(tool: ConversationToolCall): string {
  const active = toolIsActive(tool)
  const name = typeof tool.name === 'string' && tool.name.trim() ? tool.name.trim() : 'Tool call'
  switch (name) {
    case 'web_search': return active ? 'Searching web' : 'Web search'
    case 'web_read': return active ? 'Reading web page' : 'Web page'
    case 'run_shell': return active ? 'Running command' : 'Command'
    case 'shell_job': return active ? 'Checking command' : 'Command'
    case 'read_file':
    case 'read_files': return active ? 'Reading file' : 'Read file'
    case 'apply_file_edits': return active ? 'Editing files' : 'Edit files'
    case 'search_workspace': return active ? 'Searching code' : 'Code search'
    case 'list_files': return active ? 'Reading file list' : 'File list'
    case 'computer_use':
    case 'desktop_control': return active ? 'Controlling screen' : 'Screen control'
    default:
      if (name.startsWith('mcp_')) return active ? 'Running MCP' : 'MCP'
      if (name.startsWith('skill_')) return active ? 'Running Skill' : 'Skill'
      return tool.label || name.replaceAll('_', ' ')
  }
}

function toolIcon(tool: ConversationToolCall): ReactNode {
  const name = typeof tool.name === 'string' ? tool.name.trim() : ''
  switch (name) {
    case 'run_shell':
    case 'shell_job':
      return <Terminal size={12} strokeWidth={1.7} />
    case 'apply_file_edits':
      return <Pencil size={12} strokeWidth={1.7} />
    case 'read_file':
    case 'read_files':
      return <FileText size={12} strokeWidth={1.7} />
    case 'search_workspace':
      return <Search size={12} strokeWidth={1.7} />
    case 'list_files':
      return <Folder size={12} strokeWidth={1.7} />
    case 'computer_use':
    case 'desktop_control':
      return <MacWindow size={12} strokeWidth={1.7} />
    case 'web_search':
    case 'web_read':
      return <Globe size={12} strokeWidth={1.7} />
    default:
      if (name.startsWith('mcp_')) return <Plug size={12} strokeWidth={1.7} />
      if (name.startsWith('skill_')) return <BookOpen size={12} strokeWidth={1.7} />
      return <Wrench size={12} strokeWidth={1.7} />
  }
}

function toolSummary(tool: ConversationToolCall): string | null {
  const args = parseToolArguments(tool)
  const compactArguments = tool.arguments.trim().length > 100
    ? `${tool.arguments.trim().slice(0, 97)}…`
    : tool.arguments.trim()

  switch (tool.name) {
    case 'run_shell':
      return stringField(args, 'command') || tool.detail || null

    case 'apply_file_edits':
      return editSummary(tool)

    case 'read_file':
    case 'read_files': {
      const paths = readPaths(tool)
      const first = paths[0]
      if (!first) return stringField(args, 'purpose') || tool.detail || null
      const label = compactPath(first)
      return paths.length > 1 ? `${label} · +${paths.length - 1} files` : label
    }

    case 'web_search':
    case 'search_workspace':
      return stringField(args, 'query', 'q', 'search', 'text', 'pattern')
        || stringField(args, 'purpose')
        || tool.detail
        || null

    case 'web_read':
      return stringField(args, 'url', 'href', 'ref_id', 'refId')
        || stringField(args, 'purpose')
        || tool.detail
        || null

    default:
      return stringField(args, 'purpose') || tool.detail || compactArguments || null
  }
}

function toolStatus(tool: ConversationToolCall): string | null {
  switch (tool.status) {
    case 'completed': return '✓'
    case 'failed': return 'Failed'
    case 'timed_out': return 'Timed out'
    case 'awaiting_permission': return 'Awaiting approval'
    case 'cancelled': return 'Cancelled'
    case 'interrupted': return 'Interrupted'
    case 'preparing':
    case 'running':
      return null
    default:
      return tool.status ? tool.status.replaceAll('_', ' ') : null
  }
}

function durationText(tool: ConversationToolCall): string | null {
  const duration = tool.durationMs
  if (typeof duration !== 'number' || !Number.isFinite(duration) || duration < 0) return null
  if (duration < 1000) return `${Math.round(duration)} ms`
  return `${(duration / 1000).toFixed(1)} s`
}

function toolDetails(tool: ConversationToolCall): TraceDetail[] {
  const args = parseToolArguments(tool)
  const details: TraceDetail[] = []

  switch (tool.name) {
    case 'run_shell': {
      const command = stringField(args, 'command')
      const cwd = stringField(args, 'workingDirectory', 'working_directory', 'cwd')
      if (command) details.push({ title: 'Command', content: command, monospaced: true })
      if (cwd) details.push({ title: 'Working directory', content: cwd, monospaced: true })
      const result = formatUnknown(tool.result).trim()
      if (result) details.push({ title: 'Output', content: result, monospaced: true })
      break
    }

    case 'apply_file_edits': {
      const files = editFiles(tool)
      if (files.length) {
        details.push({
          title: 'Changed files',
          content: files.map(editFileDescription).join('\n'),
          monospaced: true,
        })

        const preview = files.flatMap((file) =>
          file.previewLines.length
            ? [`--- ${compactPath(file.path)}`, ...file.previewLines]
            : [],
        )
        if (preview.length) {
          details.push({
            title: 'Diff',
            content: preview.join('\n'),
            monospaced: true,
            hasBackground: true,
          })
        }
      }
      break
    }

    case 'read_file':
    case 'read_files': {
      const paths = readPaths(tool)
      if (paths.length) {
        details.push({
          title: paths.length === 1 ? 'File' : 'Files',
          content: paths.join('\n'),
          monospaced: true,
        })
      }
      break
    }

    default: {
      const raw = tool.arguments.trim()
      if (raw) {
        let content = raw
        const parsed = asRecord(raw)
        if (parsed) content = JSON.stringify(parsed, null, 2)
        details.push({ title: 'Input', content, monospaced: true })
      }

      const result = formatUnknown(tool.result).trim()
      if (result) {
        details.push({
          title: 'Result',
          content: result,
          monospaced: tool.name === 'shell_job',
        })
      }
      break
    }
  }

  if (tool.error?.trim()) {
    details.push({
      title: 'Error',
      content: tool.error.trim(),
      monospaced: tool.name === 'run_shell',
      isError: true,
    })
  }

  return details
}

function ToolTrace({ tool }: { tool: ConversationToolCall }) {
  return (
    <TraceDisclosure
      icon={toolIsActive(tool) ? <span className="mini-spinner trace-spinner" /> : toolIcon(tool)}
      title={toolTitle(tool)}
      summary={toolSummary(tool)}
      status={toolStatus(tool)}
      isActive={toolIsActive(tool)}
      details={toolDetails(tool)}
      metadata={durationText(tool)}
    />
  )
}

function ReasoningTrace({
  content,
  summary,
  isActive,
}: {
  content: string
  summary?: string | null
  isActive: boolean
}) {
  const readable = (value: string) => value.trim().replaceAll('****', '\n\n').replaceAll('**', '')
  const detail = readable(content)
  const modelSummary = summary?.trim() ? readable(summary) : ''
  const compact = modelSummary || detail
  if (!detail && !compact) return null

  return (
    <TraceDisclosure
      icon={<Lightbulb size={12} strokeWidth={1.65} />}
      title={isActive ? 'Thinking' : 'Reasoning'}
      summary={lastUsefulLine(compact || detail)}
      isActive={isActive}
      details={[...(modelSummary ? [{ title: 'Model summary', content: modelSummary }] : []), ...(detail ? [{ title: 'Reasoning', content: detail }] : [])]}
    />
  )
}

type ActivityEvent =
  | { type: 'reasoning'; id: string; content: string; summary?: string | null; isActive: boolean }
  | { type: 'activity'; id: string; activity: ModelActivity; isActive: boolean }
  | { type: 'tool'; tool: ConversationToolCall }
  | { type: 'skill'; id: string; name: string; content: string; status?: string | null }
  | { type: 'mcp'; id: string; server: string; name: string; content: string; isError: boolean }

interface ActivityGroup {
  id: string
  events: ActivityEvent[]
}

type DisplayItem =
  | { type: 'entry'; id: string; entry: Entry }
  | { type: 'activity'; id: string; group: ActivityGroup }

const TERMINAL_ACTIVITY = new Set(['done', 'completed', 'complete'])

function shouldDisplayReasoning(content: string, summary?: string | null): boolean {
  const normalizedContent = content.trim().toLowerCase()
  const normalizedSummary = summary?.trim().toLowerCase()

  if (
    TERMINAL_ACTIVITY.has(normalizedContent)
    && (normalizedSummary == null || TERMINAL_ACTIVITY.has(normalizedSummary))
  ) return false

  if (!normalizedContent && normalizedSummary && TERMINAL_ACTIVITY.has(normalizedSummary)) return false
  return true
}

function shouldDisplayActivity(activity: ModelActivity): boolean {
  if (typeof activity.phase !== 'string') return true
  const phase = activity.phase.trim().toLowerCase()
  if (phase !== 'done') return true

  const title = activity.title.trim().toLowerCase()
  const detail = activity.detail?.trim().toLowerCase()
  const genericTerminalDetail = detail ? TERMINAL_ACTIVITY.has(detail) : true

  if (TERMINAL_ACTIVITY.has(title) && genericTerminalDetail) return false

  const reasoningTitle = title === 'reasoning' || title === '추론' || title === 'reasoning status'
  if (reasoningTitle && detail && TERMINAL_ACTIVITY.has(detail)) return false

  return true
}

function activityEventID(event: ActivityEvent): string {
  switch (event.type) {
    case 'reasoning': return `reasoning:${event.id}`
    case 'activity': return `activity:${event.id}`
    case 'tool': return `tool:${event.tool.id}`
    case 'skill': return `skill:${event.id}`
    case 'mcp': return `mcp:${event.id}`
  }
}

function activityEventIsActive(event: ActivityEvent): boolean {
  switch (event.type) {
    case 'reasoning':
    case 'activity': return event.isActive
    case 'tool': return toolIsActive(event.tool)
    case 'skill': return event.status === 'running' || event.status === 'started'
    default: return false
  }
}

function activityEventHasFailure(event: ActivityEvent): boolean {
  if (event.type === 'tool') return event.tool.status === 'failed' || event.tool.status === 'timed_out'
  if (event.type === 'mcp') return event.isError
  return false
}

function activityEventAwaitsPermission(event: ActivityEvent): boolean {
  return event.type === 'tool' && event.tool.status === 'awaiting_permission'
}

function activityHeaderSummary(group: ActivityGroup): string {
  // Current work takes precedence over an earlier model summary.
  for (let index = group.events.length - 1; index >= 0; index -= 1) {
    const event = group.events[index]
    if (event.type === 'tool' && toolIsActive(event.tool)) {
      const detail = toolSummary(event.tool)
      return detail ? `${toolTitle(event.tool)} · ${detail}` : toolTitle(event.tool)
    }
    if (event.type === 'activity' && event.isActive) {
      const detail = event.activity.detail ? lastUsefulLine(event.activity.detail) : ''
      return detail ? `${event.activity.title} · ${detail}` : event.activity.title
    }
    if (event.type === 'reasoning' && event.isActive) {
      const value = lastUsefulLine(event.summary || event.content)
      if (value) return value
    }
  }

  for (let index = group.events.length - 1; index >= 0; index -= 1) {
    const event = group.events[index]
    if (event.type === 'reasoning') {
      const value = lastUsefulLine(event.summary || event.content)
      if (value) return value
    }
    if (event.type === 'activity') {
      const detail = event.activity.detail ? lastUsefulLine(event.activity.detail) : ''
      return detail ? `${event.activity.title} · ${detail}` : event.activity.title
    }
  }

  return 'Activity'
}

function formatDurationMs(duration: number | null | undefined): string | null {
  if (typeof duration !== 'number' || !Number.isFinite(duration) || duration < 0) return null
  if (duration < 1000) return `${Math.round(duration)}ms`
  return `${(duration / 1000).toFixed(duration >= 10_000 ? 0 : 1)}s`
}

function ActivityEventView({ event }: { event: ActivityEvent }) {
  switch (event.type) {
    case 'reasoning':
      return <ReasoningTrace content={event.content} summary={event.summary} isActive={event.isActive} />

    case 'activity':
      return (
        <TraceDisclosure
          icon={<Info size={12} strokeWidth={1.8} />}
          isActive={event.isActive}
          title={event.activity.title}
          summary={event.activity.detail}
          details={event.activity.detail ? [{ title: 'Details', content: event.activity.detail }] : []}
          metadata={formatDurationMs(event.activity.durationMs)}
        />
      )

    case 'tool':
      return <ToolTrace tool={event.tool} />

    case 'skill': {
      const active = event.status === 'running' || event.status === 'started'
      return (
        <TraceDisclosure
          icon={<BookOpen size={12} strokeWidth={1.8} />}
          title={event.name}
          summary={firstUsefulLine(event.content)}
          status={event.status}
          isActive={active}
          details={event.content ? [{ title: 'Skill', content: event.content }] : []}
        />
      )
    }

    case 'mcp':
      return (
        <TraceDisclosure
          icon={event.isError
            ? <CircleAlert size={12} strokeWidth={1.8} />
            : <Plug size={12} strokeWidth={1.8} />}
          title={event.server + ' · ' + event.name}
          summary={firstUsefulLine(event.content)}
          status={event.isError ? 'Failed' : null}
          details={event.content ? [{ title: 'MCP', content: event.content, isError: event.isError }] : []}
        />
      )
  }
}

function ActivityGroupView({ group }: { group: ActivityGroup }) {
  const active = group.events.some(activityEventIsActive)
  const hasFailure = group.events.some(activityEventHasFailure)
  const awaitsPermission = group.events.some(activityEventAwaitsPermission)
  const [open, setOpen] = useState(hasFailure || awaitsPermission)
  const summary = activityHeaderSummary(group)

  useEffect(() => {
    if (hasFailure || awaitsPermission) setOpen(true)
  }, [hasFailure, awaitsPermission])

  return (
    <div className="activity-group">
      <button
        className="activity-group__header"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
      >
        <span className={`activity-group__summary${active ? ' is-active' : ''}`}>{summary}</span>
        <ChevronDown size={9} className={open ? 'is-open' : ''} />
      </button>

      {open && (
        <div className="activity-group__events">
          {group.events.map((event) => (
            <div className="activity-group__event" key={activityEventID(event)}>
              <ActivityEventView event={event} />
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

function InfoCard({
  icon,
  title,
  detail,
}: {
  icon: ReactNode
  title: string
  detail?: string | null
}) {
  return (
    <div className="info-card-wrap">
      <div className="info-card glass-panel">
        <span className="info-card__icon">{icon}</span>
        <span className="info-card__copy">
          <strong>{title}</strong>
          {detail && <small>{detail}</small>}
        </span>
      </div>
    </div>
  )
}

function MessageActionRow({
  isCopied,
  copyFailed,
  onCopy,
  onEdit,
  onRegenerate,
}: {
  isCopied: boolean
  copyFailed: boolean
  onCopy: () => void
  onEdit?: (() => void) | null
  onRegenerate?: (() => void) | null
}) {
  const copyLabel = copyFailed ? 'Copy failed' : isCopied ? 'Copied' : 'Copy message'

  return (
    <div className="message-actions">
      <button onClick={onCopy} aria-label={copyLabel} title={copyLabel}>
        {copyFailed ? <TriangleAlert size={12} /> : isCopied ? <Check size={12} /> : <Copy size={12} />}
      </button>
      {onEdit && (
        <button onClick={onEdit} aria-label="Edit message">
          <Pencil size={12} />
        </button>
      )}
      {onRegenerate && (
        <button onClick={onRegenerate} aria-label="Regenerate response">
          <RotateCw size={12} />
        </button>
      )}
    </div>
  )
}

function ConversationEntry({
  entry,
  copied,
  copyFailed,
  isLastUser,
  isLastAssistant,
  streaming,
  copyMessage,
  editLast,
}: {
  entry: Entry
  copied: boolean
  copyFailed: boolean
  isLastUser: boolean
  isLastAssistant: boolean
  streaming: boolean
  copyMessage: (content: string, id: string) => void
  editLast: (content: string) => void
}) {
  switch (entry.kind.type) {
    case 'user': {
      const content = entry.kind.content
      return (
        <div className="message-block message-block--user">
          <div className="message-row message-row--user">
            <div className="user-bubble"><Markdown content={content} /></div>
          </div>
          <MessageActionRow
            isCopied={copied}
            copyFailed={copyFailed}
            onCopy={() => copyMessage(content, entry.id)}
            onEdit={isLastUser && !streaming ? () => editLast(content) : null}
          />
        </div>
      )
    }

    case 'assistant': {
      const content = entry.kind.content
      if (!content) return null
      return (
        <div className="message-block message-block--assistant">
          <div className="message-row message-row--assistant">
            <div className="assistant-message"><Markdown content={content} /></div>
          </div>
          <MessageActionRow
            isCopied={copied}
            copyFailed={copyFailed}
            onCopy={() => copyMessage(content, entry.id)}
            onRegenerate={isLastAssistant && !streaming ? () => remoteStore.regenerateLast() : null}
          />
        </div>
      )
    }

    case 'error':
      return <InfoCard icon={<TriangleAlert size={14} />} title="Error" detail={entry.kind.content} />

    case 'system':
      return <InfoCard icon={<Info size={14} />} title="System" detail={entry.kind.content} />

    default:
      return null
  }
}

function StreamingAssistant({ text: content }: { text: string }) {
  const lines = content.split('\n')
  const last = lines.pop() ?? ''
  const rest = lines.join('\n')

  return (
    <div className="streaming-assistant">
      {rest && <div className="streaming-assistant__rest"><Markdown content={rest} /></div>}
      {last && <div className="streaming-assistant__last">{last}</div>}
    </div>
  )
}

function buildDisplayItems(
  entries: Entry[],
  streaming: boolean,
  activeAssistantEntryID: string | null | undefined,
  activeReasoningEntryID: string | null | undefined,
  activeReasoningText: string,
  activeReasoningSummary: string,
  activeActivityEntryID: string | null | undefined,
): DisplayItem[] {
  const items: DisplayItem[] = []
  let pending: ActivityEvent[] = []
  const seenToolIDs = new Set<string>()
  const hidden = streaming
    ? new Set([activeAssistantEntryID, activeReasoningEntryID].filter((value): value is string => Boolean(value)))
    : new Set<string>()

  const appendTool = (tool: ConversationToolCall) => {
    if (seenToolIDs.has(tool.id)) return
    seenToolIDs.add(tool.id)
    pending.push({ type: 'tool', tool })
  }

  const flushActivity = () => {
    const first = pending[0]
    if (!first) return
    const id = `activity-group:${activityEventID(first)}`
    items.push({ type: 'activity', id, group: { id, events: pending } })
    pending = []
  }

  for (const entry of entries) {
    if (hidden.has(entry.id)) continue

    switch (entry.kind.type) {
      case 'reasoning':
        if (shouldDisplayReasoning(entry.kind.content, entry.kind.summary)) {
          pending.push({
            type: 'reasoning',
            id: entry.id,
            content: entry.kind.content,
            summary: entry.kind.summary,
            isActive: false,
          })
        }
        break

      case 'activity':
        if (shouldDisplayActivity(entry.kind.activity)) {
          pending.push({ type: 'activity', id: entry.id, activity: entry.kind.activity, isActive: streaming && entry.id === activeActivityEntryID })
        }
        break

      case 'toolCall':
        appendTool(entry.kind.toolCall)
        break

      case 'skill':
        pending.push({
          type: 'skill',
          id: entry.id,
          name: entry.kind.name,
          content: entry.kind.content,
          status: entry.kind.status,
        })
        break

      case 'mcp':
        pending.push({
          type: 'mcp',
          id: entry.id,
          server: entry.kind.server,
          name: entry.kind.name,
          content: entry.kind.content,
          isError: entry.kind.isError,
        })
        break

      case 'assistant':
        entry.kind.toolCalls?.forEach(appendTool)
        if (entry.kind.content) {
          flushActivity()
          items.push({ type: 'entry', id: `entry:${entry.id}`, entry })
        } else if (!entry.kind.toolCalls?.length) {
          flushActivity()
        }
        break

      case 'user':
      case 'system':
      case 'error':
        flushActivity()
        items.push({ type: 'entry', id: `entry:${entry.id}`, entry })
        break
    }
  }

  if (streaming && (activeReasoningText.trim() || activeReasoningSummary.trim())) {
    pending.push({
      type: 'reasoning',
      id: activeReasoningEntryID || 'live',
      content: activeReasoningText,
      summary: activeReasoningSummary || null,
      isActive: true,
    })
  }

  flushActivity()
  return items
}

interface ConversationScrollState {
  top: number
  following: boolean
}

const conversationScrollPositions = new Map<string, ConversationScrollState>()

export function Conversation({ onEditLast }: { onEditLast: (content: string) => void }) {
  const remote = useRemote()
  const scroller = useRef<HTMLDivElement>(null)
  const nearBottom = useRef(true)
  const [following, setFollowing] = useState(true)
  const viewKey = useMemo(() => JSON.stringify([
    remote.state.workspace_root ?? '',
    remote.state.current_session_id ?? null,
    remote.state.current_session_id == null ? remote.sessionResetRevision : 0,
  ]), [
    remote.state.workspace_root,
    remote.state.current_session_id,
    remote.sessionResetRevision,
  ])
  const [copiedEntryID, setCopiedEntryID] = useState<string | null>(null)
  const [copyFailedEntryID, setCopyFailedEntryID] = useState<string | null>(null)
  const copiedTimer = useRef<number | null>(null)

  const lastUserEntryID = useMemo(
    () => [...remote.entries].reverse().find((entry) => entry.kind.type === 'user')?.id ?? null,
    [remote.entries],
  )
  const lastAssistantEntryID = useMemo(
    () => [...remote.entries].reverse().find((entry) => entry.kind.type === 'assistant')?.id ?? null,
    [remote.entries],
  )
  const displayItems = useMemo(
    () => buildDisplayItems(
      remote.entries,
      remote.state.is_streaming,
      remote.state.active_assistant_entry_id,
      remote.state.active_reasoning_entry_id,
      remote.state.active_reasoning_text,
      remote.state.active_reasoning_summary,
      remote.state.active_activity_entry_id,
    ),
    [
      remote.entries,
      remote.state.is_streaming,
      remote.state.active_assistant_entry_id,
      remote.state.active_reasoning_entry_id,
      remote.state.active_reasoning_text,
      remote.state.active_reasoning_summary,
      remote.state.active_activity_entry_id,
      remote.revision,
    ],
  )
  const finalDisplayItem = displayItems.at(-1)
  const hasStreamingActivity = finalDisplayItem?.type === 'activity' && finalDisplayItem.group.events.length > 0

  const copyMessage = async (content: string, id: string) => {
    const copied = await writeClipboardText(content)
    setCopiedEntryID(copied ? id : null)
    setCopyFailedEntryID(copied ? null : id)
    if (copiedTimer.current) window.clearTimeout(copiedTimer.current)
    copiedTimer.current = window.setTimeout(() => {
      setCopiedEntryID(null)
      setCopyFailedEntryID(null)
    }, 1200)
  }

  const scrollLatest = (focus = false) => {
    const node = scroller.current
    if (!node) return
    nearBottom.current = true
    setFollowing(true)
    conversationScrollPositions.set(viewKey, { top: node.scrollHeight, following: true })
    node.scrollTo({ top: remote.entries.length ? node.scrollHeight : 0, behavior: 'auto' })
    if (focus) node.focus({ preventScroll: true })
  }

  useEffect(() => {
    const node = scroller.current
    if (!node || !nearBottom.current) return
    node.scrollTo({ top: node.scrollHeight, behavior: 'auto' })
  }, [
    remote.revision,
    remote.state.is_streaming,
    remote.state.active_assistant_text,
    remote.state.active_reasoning_text,
    remote.state.active_reasoning_summary,
  ])

  useEffect(() => {
    const node = scroller.current
    if (!node) return

    const saved = conversationScrollPositions.get(viewKey)
    const shouldFollow = saved?.following ?? true
    nearBottom.current = shouldFollow
    setFollowing(shouldFollow)

    const frame = requestAnimationFrame(() => {
      if (shouldFollow) {
        node.scrollTo({ top: node.scrollHeight, behavior: 'auto' })
        return
      }

      const maxTop = Math.max(0, node.scrollHeight - node.clientHeight)
      node.scrollTo({ top: Math.min(saved?.top ?? 0, maxTop), behavior: 'auto' })
    })
    return () => cancelAnimationFrame(frame)
  }, [viewKey])

  useEffect(() => {
    const node = scroller.current
    if (!node) return

    let frame = 0
    let followViewportChange = false

    const captureFollowPosition = () => {
      if (frame) return
      const distanceFromBottom = node.scrollHeight - node.scrollTop - node.clientHeight
      followViewportChange = distanceFromBottom < 160
    }

    const restoreFollowPosition = () => {
      if (frame) return
      frame = requestAnimationFrame(() => {
        frame = 0
        const shouldFollow = followViewportChange
        followViewportChange = false
        if (!shouldFollow) return
        nearBottom.current = true
        setFollowing(true)
        node.scrollTop = node.scrollHeight
      })
    }

    window.addEventListener('yeet:visual-viewport-will-change', captureFollowPosition)
    window.addEventListener('yeet:visual-viewport-change', restoreFollowPosition)
    return () => {
      if (frame) cancelAnimationFrame(frame)
      window.removeEventListener('yeet:visual-viewport-will-change', captureFollowPosition)
      window.removeEventListener('yeet:visual-viewport-change', restoreFollowPosition)
    }
  }, [])

  useEffect(() => {
    const node = scroller.current
    if (!node || typeof ResizeObserver === 'undefined') return

    let previousHeight = node.clientHeight
    const observer = new ResizeObserver(() => {
      const nextHeight = node.clientHeight
      if (nextHeight === previousHeight) return
      previousHeight = nextHeight
      if (nearBottom.current) node.scrollTop = node.scrollHeight
    })
    observer.observe(node)
    return () => observer.disconnect()
  }, [])

  useEffect(() => () => {
    if (copiedTimer.current) window.clearTimeout(copiedTimer.current)
  }, [])

  return (
    <div
      className="conversation-scroll"
      id="conversation-transcript"
      role="main"
      ref={scroller}
      data-testid="transcript"
      tabIndex={0}
      aria-label="Conversation transcript"
      onScroll={(event) => {
        const node = event.currentTarget
        const nextFollowing = node.scrollHeight - node.scrollTop - node.clientHeight < 160
        nearBottom.current = nextFollowing
        setFollowing(nextFollowing)
        conversationScrollPositions.set(viewKey, { top: node.scrollTop, following: nextFollowing })
      }}
      onKeyDown={(event) => {
        if (event.target !== event.currentTarget || event.altKey || event.ctrlKey || event.metaKey) return
        const node = event.currentTarget
        const pageDistance = Math.max(120, node.clientHeight - 80)
        if (event.key === 'PageUp') {
          event.preventDefault()
          node.scrollBy({ top: -pageDistance, behavior: 'auto' })
        } else if (event.key === 'PageDown') {
          event.preventDefault()
          node.scrollBy({ top: pageDistance, behavior: 'auto' })
        } else if (event.key === 'Home') {
          event.preventDefault()
          nearBottom.current = false
          setFollowing(false)
          node.scrollTo({ top: 0, behavior: 'auto' })
        } else if (event.key === 'End') {
          event.preventDefault()
          scrollLatest(false)
        }
      }}
    >
      <main className="conversation-content" aria-live="polite">
        {displayItems.map((item) => (
          item.type === 'activity'
            ? <ActivityGroupView key={item.id} group={item.group} />
            : (
              <ConversationEntry
                key={item.id}
                entry={item.entry}
                copied={copiedEntryID === item.entry.id}
                copyFailed={copyFailedEntryID === item.entry.id}
                isLastUser={item.entry.id === lastUserEntryID}
                isLastAssistant={item.entry.id === lastAssistantEntryID}
                streaming={remote.state.is_streaming}
                copyMessage={copyMessage}
                editLast={onEditLast}
              />
            )
        ))}

        {remote.state.is_streaming && (
          <div className="streaming-block">
            {remote.state.active_assistant_entry_id && remote.state.active_assistant_text
              ? <StreamingAssistant text={remote.state.active_assistant_text} />
              : !hasStreamingActivity && (
                  <div className="preparing-response">
                    <span className="mini-spinner" />
                    <span>Preparing response</span>
                  </div>
                )}
          </div>
        )}

        {remote.state.error_message && (
          <InfoCard
            icon={<TriangleAlert size={14} />}
            title="Error"
            detail={remote.state.error_message}
          />
        )}
      </main>
      {!following && (
        <button
          className="conversation-latest"
          type="button"
          onClick={() => scrollLatest(true)}
        >
          ↓ Latest
        </button>
      )}
    </div>
  )
}
