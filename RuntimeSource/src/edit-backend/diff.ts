import { addressableLines } from "./text.js";
import type { CompactDiff, DiffHunk } from "./types.js";

const MAX_BODY_LINES = 120;

export function compactDiff(path: string, before: string, after: string, context = 3): CompactDiff {
  if (before === after) return { path, hunks: [] };
  const oldLines = addressableLines(before);
  const newLines = addressableLines(after);
  let prefix = 0;
  while (prefix < oldLines.length && prefix < newLines.length && oldLines[prefix] === newLines[prefix]) prefix++;
  let suffix = 0;
  while (
    suffix < oldLines.length - prefix &&
    suffix < newLines.length - prefix &&
    oldLines[oldLines.length - 1 - suffix] === newLines[newLines.length - 1 - suffix]
  ) suffix++;

  const oldStartIndex = Math.max(0, prefix - context);
  const newStartIndex = Math.max(0, prefix - context);
  const oldEndIndex = Math.min(oldLines.length, oldLines.length - suffix + context);
  const newEndIndex = Math.min(newLines.length, newLines.length - suffix + context);
  const leadingContext = oldLines.slice(oldStartIndex, prefix).map(line => ` ${line}`);
  const removed = oldLines.slice(prefix, oldLines.length - suffix).map(line => `-${line}`);
  const added = newLines.slice(prefix, newLines.length - suffix).map(line => `+${line}`);
  const trailingContext = oldLines.slice(oldLines.length - suffix, oldEndIndex).map(line => ` ${line}`);
  let lines = [...leadingContext, ...removed, ...added, ...trailingContext];
  let truncated = false;
  if (lines.length > MAX_BODY_LINES) {
    const head = lines.slice(0, 58);
    const tail = lines.slice(-58);
    lines = [...head, " ... diff preview truncated ...", ...tail];
    truncated = true;
  }
  const hunk: DiffHunk = {
    oldStart: oldStartIndex + 1,
    oldEnd: Math.max(oldStartIndex, oldEndIndex),
    newStart: newStartIndex + 1,
    newEnd: Math.max(newStartIndex, newEndIndex),
    lines,
    truncated,
  };
  return { path, hunks: [hunk] };
}

export function diffSeenRanges(diff: CompactDiff): Array<{ start: number; end: number }> {
  return diff.hunks
    .filter(hunk => hunk.newEnd >= hunk.newStart)
    .map(hunk => ({ start: hunk.newStart, end: hunk.newEnd }));
}


