import type { ToolDiffFile, ToolOutputStream, ToolSourceFile } from '@/remote/protocol'

// Formatting only. Tool semantics, status captions, diff counts and hunk headers come from Rust.
export function indexed<T>(items: T[] | undefined) {
  return (items ?? []).map((value, key) => ({ value, key }))
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
  return `${file.truncated ? 'Preview · ' : ''}+${file.added ?? 0} −${file.removed ?? 0}`
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
