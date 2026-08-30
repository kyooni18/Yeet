import { addressableLines } from "./text.js";
import type { ConcreteEdit, LineRange } from "./types.js";

export interface RebaseResult {
  edits: ConcreteEdit[];
  offset: number | null;
  warning?: string;
}

function equalSequence(haystack: readonly string[], start: number, needle: readonly string[]): boolean {
  if (start < 0 || start + needle.length > haystack.length) return false;
  for (let index = 0; index < needle.length; index++) {
    if (haystack[start + index] !== needle[index]) return false;
  }
  return true;
}

function locateRange(previous: readonly string[], current: readonly string[], range: LineRange): LineRange | null {
  const needle = previous.slice(range.start - 1, range.end);
  if (needle.length === 0) return null;
  let candidates: number[] = [];
  for (let index = 0; index + needle.length <= current.length; index++) {
    if (equalSequence(current, index, needle)) candidates.push(index);
  }
  if (candidates.length === 1) {
    const start = candidates[0]! + 1;
    return { start, end: start + needle.length - 1 };
  }
  if (candidates.length === 0) return null;

  for (let radius = 1; radius <= 4 && candidates.length > 1; radius++) {
    const beforeStart = Math.max(0, range.start - 1 - radius);
    const before = previous.slice(beforeStart, range.start - 1);
    const after = previous.slice(range.end, Math.min(previous.length, range.end + radius));
    candidates = candidates.filter(index => {
      const beforePos = index - before.length;
      const afterPos = index + needle.length;
      const beforeOk = before.length === 0 || equalSequence(current, beforePos, before);
      const afterOk = after.length === 0 || equalSequence(current, afterPos, after);
      return beforeOk && afterOk;
    });
  }
  if (candidates.length !== 1) return null;
  const start = candidates[0]! + 1;
  return { start, end: start + needle.length - 1 };
}

function anchorRange(edit: ConcreteEdit): LineRange | null {
  if (edit.kind === "replace" || edit.kind === "delete") return edit.range;
  if (edit.at.kind === "before" || edit.at.kind === "after") return { start: edit.at.line, end: edit.at.line };
  return null;
}

function remapEdit(edit: ConcreteEdit, mapped: LineRange | null): ConcreteEdit {
  if (edit.kind === "replace") {
    if (!mapped) throw new Error("Missing rebase mapping for replacement.");
    return { ...edit, range: mapped };
  }
  if (edit.kind === "delete") {
    if (!mapped) throw new Error("Missing rebase mapping for deletion.");
    return { ...edit, range: mapped };
  }
  if (edit.at.kind === "before" || edit.at.kind === "after") {
    if (!mapped) throw new Error("Missing rebase mapping for insertion anchor.");
    return { ...edit, at: { ...edit.at, line: mapped.start } };
  }
  return edit;
}

export function rebaseEdits(previousText: string, currentText: string, edits: readonly ConcreteEdit[]): RebaseResult | null {
  const previous = addressableLines(previousText);
  const current = addressableLines(currentText);
  const mapped: ConcreteEdit[] = [];
  const offsets: number[] = [];

  for (const edit of edits) {
    const anchor = anchorRange(edit);
    if (!anchor) {
      mapped.push(edit);
      continue;
    }
    const located = locateRange(previous, current, anchor);
    if (!located) return null;
    offsets.push(located.start - anchor.start);
    mapped.push(remapEdit(edit, located));
  }

  if (offsets.length === 0) {
    return {
      edits: mapped,
      offset: null,
      warning: "The file changed after the snapshot, but the requested edits only target file boundaries.",
    };
  }
  const first = offsets[0]!;
  if (!offsets.every(offset => offset === first)) return null;
  return {
    edits: mapped,
    offset: first,
    warning: first === 0
      ? "The file changed after the snapshot; unchanged edit anchors were revalidated against the live file."
      : `The file changed after the snapshot; edit anchors were conservatively rebased by ${first > 0 ? "+" : ""}${first} lines.`,
  };
}


