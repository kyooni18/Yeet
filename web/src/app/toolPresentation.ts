import type { ToolDiffFile, ToolDiffHunk, ToolOutputStream, ToolSourceFile, ToolTerminalDetail } from '@/remote/protocol'

// Formatting only. Tool semantics, captured source and diff lines come from Rust.
export function indexed<T>(items: T[] | undefined) {
  return (items ?? []).map((value, key) => ({ value, key }))
}

export function terminalStatus(detail: ToolTerminalDetail): string {
  const label = {
    waiting: 'Waiting for execution', running: 'Running', background_job: 'Running in background',
    succeeded: 'Finished', failed: 'Failed', cancelled: 'Cancelled', timed_out: 'Timed out', interrupted: 'Interrupted',
  }[detail.state] ?? 'Shell'
  const duration = detail.duration_ms == null ? '' : detail.duration_ms < 1000 ? `${detail.duration_ms} ms` : `${(detail.duration_ms / 1000).toFixed(1)} s`
  return [label, detail.exit_code == null ? '' : `exit ${detail.exit_code}`, duration].filter(Boolean).join(' · ')
}

/** WebKit on touch devices doesn't reliably arrow-scroll focused code regions.
 * Only handle horizontal arrows on the region itself, never nested inputs. */
export function onCodeKeyDown(event: KeyboardEvent) {
  if (event.target !== event.currentTarget || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return
  if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return
  const element = event.currentTarget as HTMLElement
  if (element.scrollWidth <= element.clientWidth) return
  event.preventDefault()
  element.scrollLeft += event.key === 'ArrowLeft' ? -48 : 48
}

export function streamNote(stream: ToolOutputStream): string {
  return [stream.tail ? 'Latest output' : '', stream.truncated ? 'Truncated' : '',
    stream.bytes == null ? '' : `${stream.bytes.toLocaleString()} bytes total`].filter(Boolean).join(' · ')
}

export function diffStats(file: ToolDiffFile): string {
  let added = 0, removed = 0
  for (const hunk of file.hunks) for (const line of hunk.lines) {
    if (line.kind === 'added') added++
    if (line.kind === 'removed') removed++
  }
  return `${file.truncated ? 'Preview · ' : ''}+${added} −${removed}`
}

export function hunkLabel(hunk: ToolDiffHunk): string {
  const range = (start: number, end: number | null) => end == null ? `${start}` : `${start},${Math.max(0, end - start + 1)}`
  return `@@ -${range(hunk.old_start, hunk.old_end)} +${range(hunk.new_start, hunk.new_end)} @@`
}

export function sourceRange(file: ToolSourceFile): string {
  return [file.availability === 'preview' ? 'Preview' : '', file.start_line == null ? '' : `Lines ${file.start_line}${file.end_line == null ? '' : `–${file.end_line}`}`,
    file.total_lines == null ? '' : `${file.total_lines.toLocaleString()} total`].filter(Boolean).join(' · ')
}

/** Presentation-only unified diff numbering; the patch itself comes from Rust. */
export function unifiedRows(lines: string[]) {
  let old = 0, next = 0
  return lines.map((text, index) => {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(text)
    if (hunk) { old = Number(hunk[1]); next = Number(hunk[2]) }
    const meta = !!hunk || /^(?:diff |index |--- |\+\+\+ |\\|[a-z])/.test(text)
    const kind = meta ? 'meta' : text.startsWith('+') ? 'added' : text.startsWith('-') ? 'removed' : 'context'
    const row = { index, text: meta ? text : text.slice(1), sign: kind === 'added' ? '+' : kind === 'removed' ? '−' : '',
      kind, old: !meta && old > 0 && kind !== 'added' ? old : null, new: !meta && next > 0 && kind !== 'removed' ? next : null }
    if (!meta) { if (kind !== 'added') old++; if (kind !== 'removed') next++ }
    return row
  })
}
