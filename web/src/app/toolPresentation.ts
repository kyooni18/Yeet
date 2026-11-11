import type { ToolDiffFile, ToolDiffHunk, ToolOutputStream, ToolSourceFile, ToolTerminalDetail } from '@/remote/protocol'

// Formatting only. Tool semantics, captured source and diff lines come from Rust.
export function indexed<T>(items: T[] | undefined) {
  return (items ?? []).map((value, key) => ({ value, key }))
}

export function terminalStatus(detail: ToolTerminalDetail): string {
  const label = {
    waiting: 'Waiting for execution', running: 'Running', background_job: 'Running in background',
    succeeded: 'Finished', failed: 'Failed', cancelled: 'Cancelled', timed_out: 'Timed out', interrupted: 'Interrupted',
  }[detail.state]
  return detail.exit_code == null ? label : `${label} · exit ${detail.exit_code}`
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
  return [file.start_line == null ? '' : `Lines ${file.start_line}${file.end_line == null ? '' : `–${file.end_line}`}`,
    file.total_lines == null ? '' : `${file.total_lines.toLocaleString()} total`].filter(Boolean).join(' · ')
}
