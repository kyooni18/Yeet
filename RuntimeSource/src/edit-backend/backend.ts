import { createHash } from "node:crypto";
import { readFile, stat } from "node:fs/promises";
import path from "node:path";
import { applyConcreteEdits, concretizeEdits, requiredSeenRanges } from "./apply.js";
import { compactDiff, diffSeenRanges } from "./diff.js";
import { ApplyPatchDialect } from "./dialects/apply-patch.js";
import { HashlineDialect } from "./dialects/hashline.js";
import { SloppyDialect } from "./dialects/sloppy.js";
import { ModelDialectSelector } from "./model-dialect.js";
import { PathGuard } from "./path-guard.js";
import { rebaseEdits } from "./rebase.js";
import { SnapshotStore, digestText, type Snapshot } from "./snapshot.js";
import { addressableLines, decodeText, encodeText, formatNumbered } from "./text.js";
import { TransactionalWriter, type PlannedFileState } from "./transaction.js";
import type {
  ApplyRequest,
  ApplyResult,
  BlockResolver,
  CompactDiff,
  ConcreteEdit,
  DiagnosticsProvider,
  EditDialect,
  FileApplyResult,
  FileChange,
  ModelDialectRule,
  ReadRequest,
  ReadResult,
} from "./types.js";

const noopDiagnostics: DiagnosticsProvider = { diagnose: async () => [] };

export interface EditBackendOptions {
  root: string;
  snapshots?: SnapshotStore;
  blockResolver?: BlockResolver;
  diagnostics?: DiagnosticsProvider;
  enforceSeenLines?: boolean;
  transactionDir?: string;
  defaultDialect?: string;
  modelDialects?: ModelDialectRule[];
}

interface PreparedFile {
  result: FileApplyResult;
  before?: string;
  after?: string;
  canonicalSource?: string;
  canonicalDestination?: string;
  states: PlannedFileState[];
  postCommit?: () => void;
}

function rawDigest(raw: string): string {
  return createHash("sha256").update(raw, "utf8").digest("hex");
}

function ensureSnapshotPath(snapshot: Snapshot, canonical: string): void {
  if (snapshot.path !== canonical) {
    throw new Error(`Snapshot ${snapshot.handle} belongs to ${snapshot.path}, not ${canonical}.`);
  }
}

function propagateSeen(snapshot: Snapshot, edits: readonly ConcreteEdit[], afterLineCount: number): Array<{ start: number; end: number }> {
  const points = [...snapshot.seen.values()];
  const result = new Set<number>();
  for (const oldLine of points) {
    let line = oldLine;
    let removed = false;
    for (const edit of edits) {
      if (edit.kind === "replace" || edit.kind === "delete") {
        const oldLength = edit.range.end - edit.range.start + 1;
        const newLength = edit.kind === "delete" ? 0 : edit.text.replace(/\r\n/g, "\n").replace(/\n$/, "").split("\n").length;
        if (line >= edit.range.start && line <= edit.range.end) {
          removed = true;
          break;
        }
        if (line > edit.range.end) line += newLength - oldLength;
      } else {
        const inserted = edit.text.replace(/\r\n/g, "\n").replace(/\n$/, "").split("\n").length;
        let position: number;
        switch (edit.at.kind) {
          case "start": position = 0; break;
          case "end": position = Number.MAX_SAFE_INTEGER; break;
          case "before": position = edit.at.line - 1; break;
          case "after": position = edit.at.line; break;
        }
        if (position < line) line += inserted;
      }
    }
    if (!removed && line >= 1 && line <= afterLineCount) result.add(line);
  }
  const sorted = [...result].sort((a, b) => a - b);
  const ranges: Array<{ start: number; end: number }> = [];
  for (const line of sorted) {
    const tail = ranges.at(-1);
    if (tail && tail.end + 1 === line) tail.end = line;
    else ranges.push({ start: line, end: line });
  }
  return ranges;
}

export class EditBackend {
  readonly root: string;
  readonly snapshots: SnapshotStore;
  readonly diagnostics: DiagnosticsProvider;
  readonly transaction: TransactionalWriter;
  readonly selector: ModelDialectSelector;
  readonly #blockResolver: BlockResolver | undefined;
  readonly #enforceSeenLines: boolean;
  readonly #dialects = new Map<string, EditDialect>();
  #guard?: PathGuard;
  #queue: Promise<void> = Promise.resolve();

  constructor(options: EditBackendOptions) {
    this.root = options.root;
    this.snapshots = options.snapshots ?? new SnapshotStore();
    this.diagnostics = options.diagnostics ?? noopDiagnostics;
    this.transaction = new TransactionalWriter(options.transactionDir);
    this.selector = new ModelDialectSelector(options.defaultDialect ?? "structured", options.modelDialects ?? []);
    this.#blockResolver = options.blockResolver;
    this.#enforceSeenLines = options.enforceSeenLines ?? true;
    this.registerDialect(new HashlineDialect());
    this.registerDialect(new ApplyPatchDialect());
    this.registerDialect(new SloppyDialect());
  }

  async initialize(): Promise<void> {
    this.#guard = await PathGuard.create(this.root);
    await this.transaction.initialize();
  }

  registerDialect(dialect: EditDialect): void {
    this.#dialects.set(dialect.id, dialect);
  }

  async read(request: ReadRequest): Promise<ReadResult> {
    const guard = this.#requireGuard();
    const canonical = await guard.resolve(request.path);
    const raw = await readFile(canonical, "utf8");
    const decoded = decodeText(raw);
    const lines = addressableLines(decoded.text);
    const startLine = request.startLine ?? 1;
    const endLine = request.endLine ?? Math.min(lines.length, startLine + 199);
    if (lines.length === 0) {
      const snapshot = this.snapshots.record(canonical, decoded.text, []);
      return { path: guard.display(canonical), snapshot: snapshot.handle, startLine: 1, endLine: 0, totalLines: 0, content: "", numbered: "" };
    }
    if (startLine < 1 || startLine > lines.length || endLine < startLine || endLine > lines.length) {
      throw new Error(`Invalid read range ${startLine}..${endLine}; file has ${lines.length} lines.`);
    }
    const selected = lines.slice(startLine - 1, endLine);
    const snapshot = this.snapshots.record(canonical, decoded.text, [{ start: startLine, end: endLine }]);
    return {
      path: guard.display(canonical),
      snapshot: snapshot.handle,
      startLine,
      endLine,
      totalLines: lines.length,
      content: selected.join("\n"),
      numbered: formatNumbered(selected, startLine),
    };
  }

  async preflight(request: ApplyRequest): Promise<ApplyResult> {
    const prepared = await this.#prepare(request);
    return {
      files: prepared.map(item => item.result),
      diagnostics: [],
    };
  }

  async apply(request: ApplyRequest): Promise<ApplyResult> {
    return this.#serialize(async () => {
      const prepared = await this.#prepare(request);
      const states = prepared.flatMap(item => item.states);
      await this.transaction.commit(states);
      for (const item of prepared) item.postCommit?.();
      const changedPaths = prepared.flatMap(item => item.canonicalDestination ?? item.canonicalSource ?? []);
      const diagnostics = request.diagnostics === false ? [] : await this.diagnostics.diagnose(changedPaths);
      return { files: prepared.map(item => item.result), diagnostics };
    });
  }

  async applyDialect(input: string, options: { dialect?: string; model?: string; snapshots?: Record<string, string>; diagnostics?: boolean } = {}): Promise<ApplyResult> {
    const dialectId = options.dialect ?? this.selector.resolve(options.model);
    if (dialectId === "structured") throw new Error("structured dialect accepts ApplyRequest directly; call apply().");
    const dialect = this.#dialects.get(dialectId);
    if (!dialect) throw new Error(`Unknown edit dialect: ${dialectId}`);
    const parsed = await dialect.parse(input, {
      snapshots: options.snapshots ?? {},
      getSnapshotText: handle => this.snapshots.get(handle).text,
    });
    return this.apply({ ...parsed, ...(options.diagnostics !== undefined ? { diagnostics: options.diagnostics } : {}) });
  }

  async #prepare(request: ApplyRequest): Promise<PreparedFile[]> {
    if (request.changes.length === 0) throw new Error("Apply request contains no changes.");
    const guard = this.#requireGuard();
    const sourcePaths = new Set<string>();
    const targetPaths = new Set<string>();
    const prepared: PreparedFile[] = [];
    for (const change of request.changes) {
      const source = await guard.resolve(change.path, { allowMissing: change.fileOp?.kind === "create" });
      if (sourcePaths.has(source)) throw new Error(`Multiple changes target the same source file: ${change.path}`);
      sourcePaths.add(source);
      prepared.push(await this.#prepareOne(change, source));
    }
    for (const item of prepared) {
      for (const state of item.states) {
        if (targetPaths.has(state.path)) throw new Error(`Multiple final states target the same file: ${state.path}`);
        targetPaths.add(state.path);
      }
    }
    return prepared;
  }

  async #prepareOne(change: FileChange, canonical: string): Promise<PreparedFile> {
    const guard = this.#requireGuard();
    if (change.fileOp?.kind === "create") {
      if (change.snapshot) throw new Error("Create operations must not carry a snapshot handle.");
      if (change.edits?.length) throw new Error("Create operations use fileOp.text and cannot include line edits.");
      try {
        await stat(canonical);
        throw new Error(`Refusing to create existing file: ${change.path}`);
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      }
      const normalized = change.fileOp.text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
      const result: FileApplyResult = {
        path: guard.display(canonical),
        operation: "create",
        diff: compactDiff(guard.display(canonical), "", normalized),
        warnings: [],
      };
      const state: PlannedFileState = {
        path: canonical,
        content: normalized,
        mode: change.fileOp.mode ?? 0o644,
        expectMissing: true,
      };
      return {
        result,
        after: normalized,
        canonicalDestination: canonical,
        states: [state],
        postCommit: () => {
          const diff = result.diff!;
          const snapshot = this.snapshots.record(canonical, normalized, diffSeenRanges(diff));
          result.snapshot = snapshot.handle;
        },
      };
    }

    const raw = await readFile(canonical, "utf8");
    const fileStat = await stat(canonical);
    const envelope = decodeText(raw);
    const before = envelope.text;
    if (!change.snapshot) throw new Error(`Snapshot handle is required before editing existing file: ${change.path}`);
    const snapshot = this.snapshots.get(change.snapshot);
    ensureSnapshotPath(snapshot, canonical);
    const requested = await concretizeEdits(change.path, snapshot.text, change.edits ?? [], this.#blockResolver);
    if (this.#enforceSeenLines) {
      const unseen = requiredSeenRanges(requested).filter(range => !snapshot.seen.covers(range.start, range.end));
      if (unseen.length > 0) {
        const rendered = unseen.slice(0, 8).map(range => `${range.start}${range.end === range.start ? "" : `..${range.end}`}`).join(", ");
        throw new Error(`Edit rejected because snapshot ${snapshot.handle} did not display required lines: ${rendered}. Re-read those lines first.`);
      }
    }

    let concrete = requested;
    const warnings: string[] = [];
    const liveMatches = digestText(before) === snapshot.digest && before === snapshot.text;
    if (!liveMatches) {
      const rebased = rebaseEdits(snapshot.text, before, requested);
      if (!rebased) throw new Error(`Snapshot ${snapshot.handle} is stale and its edit anchors cannot be safely rebased. Re-read ${change.path}.`);
      concrete = rebased.edits;
      if (rebased.warning) warnings.push(rebased.warning);
    }

    if (change.fileOp?.kind === "delete" && concrete.length > 0) throw new Error("Do not combine line edits with file deletion.");
    const after = concrete.length > 0 ? applyConcreteEdits(before, concrete).text : before;
    const mode = fileStat.mode & 0o777;
    const sourceDisplay = guard.display(canonical);

    if (change.fileOp?.kind === "delete") {
      return {
        result: { path: sourceDisplay, operation: "delete", warnings },
        before,
        canonicalSource: canonical,
        states: [{ path: canonical, expectedDigest: rawDigest(raw) }],
        postCommit: () => this.snapshots.invalidatePath(canonical),
      };
    }

    if (change.fileOp?.kind === "move") {
      const destination = await guard.resolve(change.fileOp.destination, { allowMissing: true });
      if (destination === canonical) throw new Error("Move destination is the same as source.");
      try {
        await stat(destination);
        throw new Error(`Move destination already exists: ${change.fileOp.destination}`);
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      }
      const diff = compactDiff(sourceDisplay, before, after);
      const result: FileApplyResult = {
        path: sourceDisplay,
        destination: guard.display(destination),
        operation: "move",
        diff,
        warnings,
      };
      return {
        result,
        before,
        after,
        canonicalSource: canonical,
        canonicalDestination: destination,
        states: [
          { path: canonical, expectedDigest: rawDigest(raw) },
          { path: destination, content: encodeText(after, envelope), mode, expectMissing: true },
        ],
        postCommit: () => {
          this.snapshots.invalidatePath(canonical);
          const seen = liveMatches ? propagateSeen(snapshot, concrete, addressableLines(after).length) : [];
          for (const range of diffSeenRanges(diff)) seen.push(range);
          const next = this.snapshots.record(destination, after, seen);
          result.snapshot = next.handle;
        },
      };
    }

    if (after === before) throw new Error(`Edits to ${change.path} produced no changes.`);
    const diff: CompactDiff = compactDiff(sourceDisplay, before, after);
    const result: FileApplyResult = { path: sourceDisplay, operation: "update", diff, warnings };
    return {
      result,
      before,
      after,
      canonicalSource: canonical,
      canonicalDestination: canonical,
      states: [{ path: canonical, content: encodeText(after, envelope), mode, expectedDigest: rawDigest(raw) }],
      postCommit: () => {
        const seen = liveMatches ? propagateSeen(snapshot, concrete, addressableLines(after).length) : [];
        for (const range of diffSeenRanges(diff)) seen.push(range);
        const next = this.snapshots.record(canonical, after, seen);
        result.snapshot = next.handle;
      },
    };
  }

  #requireGuard(): PathGuard {
    if (!this.#guard) throw new Error("EditBackend.initialize() must be called before use.");
    return this.#guard;
  }

  async #serialize<T>(operation: () => Promise<T>): Promise<T> {
    const previous = this.#queue;
    let release!: () => void;
    this.#queue = new Promise<void>(resolve => { release = resolve; });
    await previous;
    try {
      return await operation();
    } finally {
      release();
    }
  }
}


