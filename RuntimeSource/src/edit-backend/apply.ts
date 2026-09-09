import { addressableLines, joinAddressableLines, lineHash, payloadLines } from "./text.js";
import type { BlockResolver, ConcreteEdit, Edit, LineRange } from "./types.js";

interface PlannedSplice {
  start: number;
  end: number;
  replacement: string[];
  source: ConcreteEdit;
}

export interface ApplyTextResult {
  text: string;
  concrete: ConcreteEdit[];
}

function assertRange(range: LineRange, lineCount: number): void {
  if (!Number.isInteger(range.start) || !Number.isInteger(range.end) || range.start < 1 || range.end < range.start) {
    throw new Error(`Invalid line range ${range.start}..${range.end}`);
  }
  if (range.end > lineCount) throw new Error(`Line range ${range.start}..${range.end} exceeds file length ${lineCount}`);
}

function normalizedAnchor(expected: string): string {
  const value = expected.trim();
  // read_file renders anchors as `line:hash|source`. Models occasionally copy
  // the visible suffix into a hash field instead of extracting the four hex
  // characters. Accept both the canonical token and the rendered form so a
  // harmless formatting mistake does not look like stale source.
  const rendered = /^(?:\d+:)?([0-9a-f]{4})(?:\|.*)?$/i.exec(value);
  return (rendered?.[1] ?? value).toLowerCase();
}

function assertAnchor(lines: readonly string[], line: number, expected: string | undefined, label: string): void {
  if (!expected) return;
  const actual = lineHash(lines[line - 1] ?? "");
  const normalized = normalizedAnchor(expected);
  if (actual !== normalized) {
    throw new Error(
      `Edit anchor mismatch at ${label} line ${line}: expected ${line}:${normalized}, current snapshot is ${line}:${actual}. Re-read the file before editing.`,
    );
  }
}

export async function concretizeEdits(
  path: string,
  snapshotText: string,
  edits: readonly Edit[],
  blockResolver?: BlockResolver,
): Promise<ConcreteEdit[]> {
  const lines = addressableLines(snapshotText);
  const lineCount = lines.length;
  const concrete: ConcreteEdit[] = [];
  for (const edit of edits) {
    if (edit.kind === "replace" || edit.kind === "delete") {
      assertRange(edit.range, lineCount);
      assertAnchor(lines, edit.range.start, edit.range.startHash, "range start");
      assertAnchor(lines, edit.range.end, edit.range.endHash, "range end");
      concrete.push(edit);
      continue;
    }
    if (edit.kind === "insert") {
      if (edit.at.kind === "before" || edit.at.kind === "after") {
        assertRange({ start: edit.at.line, end: edit.at.line }, lineCount);
        assertAnchor(lines, edit.at.line, edit.at.hash, "insertion");
      }
      concrete.push(edit);
      continue;
    }
    assertRange({ start: edit.line, end: edit.line }, lineCount);
    assertAnchor(lines, edit.line, edit.hash, "block");
    if (!blockResolver) throw new Error(`Block edit requested for ${path}, but no block resolver is configured.`);
    const span = await blockResolver({ path, text: snapshotText, line: edit.line });
    if (!span) throw new Error(`Could not resolve syntactic block beginning at line ${edit.line} in ${path}.`);
    assertRange(span, lineCount);
    if (edit.kind === "replaceBlock") {
      concrete.push({ kind: "replace", range: span, text: edit.text });
    } else if (edit.kind === "deleteBlock") {
      concrete.push({ kind: "delete", range: span });
    } else {
      concrete.push({ kind: "insert", at: { kind: "after", line: span.end }, text: edit.text });
    }
  }
  return concrete;
}

export function requiredSeenRanges(edits: readonly ConcreteEdit[]): LineRange[] {
  const out: LineRange[] = [];
  for (const edit of edits) {
    if (edit.kind === "replace" || edit.kind === "delete") out.push(edit.range);
    else if (edit.at.kind === "before" || edit.at.kind === "after") out.push({ start: edit.at.line, end: edit.at.line });
  }
  return out;
}

function toSplice(edit: ConcreteEdit, lineCount: number): PlannedSplice {
  if (edit.kind === "replace") {
    assertRange(edit.range, lineCount);
    return { start: edit.range.start - 1, end: edit.range.end, replacement: payloadLines(edit.text), source: edit };
  }
  if (edit.kind === "delete") {
    assertRange(edit.range, lineCount);
    return { start: edit.range.start - 1, end: edit.range.end, replacement: [], source: edit };
  }
  switch (edit.at.kind) {
    case "start":
      return { start: 0, end: 0, replacement: payloadLines(edit.text), source: edit };
    case "end":
      return { start: lineCount, end: lineCount, replacement: payloadLines(edit.text), source: edit };
    case "before":
      assertRange({ start: edit.at.line, end: edit.at.line }, lineCount);
      return { start: edit.at.line - 1, end: edit.at.line - 1, replacement: payloadLines(edit.text), source: edit };
    case "after":
      assertRange({ start: edit.at.line, end: edit.at.line }, lineCount);
      return { start: edit.at.line, end: edit.at.line, replacement: payloadLines(edit.text), source: edit };
  }
}

function overlaps(a: PlannedSplice, b: PlannedSplice): boolean {
  const aInsert = a.start === a.end;
  const bInsert = b.start === b.end;
  if (aInsert && bInsert) return a.start === b.start;
  // Splice ranges use half-open intervals. An insertion at the end of a
  // replacement is therefore adjacent (for example, replace line 1 and
  // insert after line 1), not overlapping. Insertion at the beginning still
  // overlaps because it would make the edit order ambiguous.
  if (aInsert) return a.start >= b.start && a.start < b.end;
  if (bInsert) return b.start >= a.start && b.start < a.end;
  return a.start < b.end && b.start < a.end;
}

export function applyConcreteEdits(text: string, edits: readonly ConcreteEdit[]): ApplyTextResult {
  const keepFinalNewline = text.endsWith("\n");
  const lines = addressableLines(text);
  const splices = edits.map(edit => toSplice(edit, lines.length));
  for (let i = 0; i < splices.length; i++) {
    for (let j = i + 1; j < splices.length; j++) {
      if (overlaps(splices[i]!, splices[j]!)) throw new Error("Overlapping or ambiguous edits in one file are not allowed.");
    }
  }
  splices.sort((a, b) => b.start - a.start || b.end - a.end);
  const output = [...lines];
  for (const splice of splices) output.splice(splice.start, splice.end - splice.start, ...splice.replacement);
  return { text: joinAddressableLines(output, keepFinalNewline), concrete: [...edits] };
}

