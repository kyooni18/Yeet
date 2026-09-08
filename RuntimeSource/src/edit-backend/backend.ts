import { createHash } from "node:crypto";
import { opendir, readFile, stat } from "node:fs/promises";
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
import { addressableLines, decodeText, encodeText, formatAnchored, formatNumbered } from "./text.js";
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
  ListFilesRequest,
  ListFilesResult,
  ModelDialectRule,
  ReadRequest,
  ReadResult,
  SearchRequest,
  SearchResult,
} from "./types.js";

const noopDiagnostics: DiagnosticsProvider = { diagnose: async () => [] };
const SEARCH_IGNORED_DIRECTORIES = new Set([
  ".git", ".yeet", ".transactions", ".build", ".swiftpm", ".cache", ".next", ".venv",
  ".astro", ".turbo", ".vite", "node_modules", "dist", "build", "built", "out",
  "coverage", "DerivedData", "Pods", "target", "vendor", "venv",
]);
const SEARCH_SOURCE_DIRECTORIES = new Set([
  "src", "source", "sources", "packages", "tests", "test", "runtimesource",
]);
const SEARCH_MAX_FILE_BYTES = 2 * 1024 * 1024;
const SEARCH_MAX_FILES = 10_000;

function searchEntryPriority(name: string): number {
  const normalized = name.toLowerCase();
  if (SEARCH_SOURCE_DIRECTORIES.has(normalized)) return 0;
  if (normalized === "docs" || normalized === "examples") return 1;
  return 2;
}

export interface EditBackendOptions {
  root: string;
  allowOutside?: boolean;
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

function freshAnchors(after: string, diff: CompactDiff, maxLines = 120): string | undefined {
  const lines = addressableLines(after);
  const chunks: string[] = [];
  let remaining = maxLines;
  for (const hunk of diff.hunks) {
    if (remaining <= 0) break;
    const start = Math.max(1, hunk.newStart);
    const end = Math.min(lines.length, hunk.newEnd);
    if (end < start) continue;
    const count = Math.min(remaining, end - start + 1);
    chunks.push(formatAnchored(lines.slice(start - 1, start - 1 + count), start));
    remaining -= count;
  }
  if (chunks.length === 0) return undefined;
  return chunks.join("\n");
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
  readonly #allowOutside: boolean;
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
    this.#allowOutside = options.allowOutside ?? false;
    this.registerDialect(new HashlineDialect());
    this.registerDialect(new ApplyPatchDialect());
    this.registerDialect(new SloppyDialect());
  }

  async initialize(): Promise<void> {
    this.#guard = await PathGuard.create(this.root, { allowOutside: this.#allowOutside });
    await this.transaction.initialize();
  }

  registerDialect(dialect: EditDialect): void {
    this.#dialects.set(dialect.id, dialect);
  }

  async read(request: ReadRequest): Promise<ReadResult> {
    const guard = this.#requireGuard();
    const canonical = await guard.resolve(request.path, { allowSymlink: request.unsafe === true });
    const raw = await readFile(canonical, "utf8");
    const decoded = decodeText(raw);
    const lines = addressableLines(decoded.text);
    const startLine = request.startLine ?? 1;
    if (lines.length === 0) {
      const snapshot = this.snapshots.record(canonical, decoded.text, []);
      return {
        path: guard.display(canonical),
        snapshot: snapshot.handle,
        startLine: 1,
        endLine: 0,
        totalLines: 0,
        content: "",
        numbered: "",
        anchored: "",
      };
    }
    const requestedEndLine = request.endLine ?? startLine + 159;
    if (startLine < 1 || startLine > lines.length || requestedEndLine < startLine) {
      throw new Error(`Invalid read range ${startLine}..${requestedEndLine}; file has ${lines.length} lines.`);
    }
    const endLine = Math.min(requestedEndLine, lines.length);
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
      anchored: formatAnchored(selected, startLine),
    };
  }

  async search(request: SearchRequest): Promise<SearchResult> {
    const guard = this.#requireGuard();
    const query = request.query?.trim();
    if (!query) throw new Error("Search query must not be empty.");
    if ((request.path ?? ".").split(/[\\/]+/).some(part => SEARCH_IGNORED_DIRECTORIES.has(part))) {
      throw new Error("Search path points into generated or internal workspace state.");
    }
    const maxResults = Math.min(100, Math.max(1, request.maxResults ?? 20));
    const caseSensitive = request.caseSensitive ?? false;
    const regex = request.regex ?? false;
    let expression: RegExp | undefined;
    if (regex) {
      if (query.length > 1_024) throw new Error("Search regex is too long (maximum 1024 characters).");
      try { expression = new RegExp(query, caseSensitive ? "" : "i"); }
      catch (error) { throw new Error(`Invalid search regex: ${error instanceof Error ? error.message : String(error)}`); }
    }
    const needle = caseSensitive ? query : query.toLowerCase();
    const start = await guard.resolve(request.path ?? ".");
    const matches: SearchResult["matches"] = [];
    let filesScanned = 0;
    let truncated = false;

    const searchFile = async (canonical: string): Promise<void> => {
      if (filesScanned >= SEARCH_MAX_FILES || matches.length >= maxResults) {
        truncated = true;
        return;
      }
      const info = await stat(canonical);
      if (!info.isFile() || info.size > SEARCH_MAX_FILE_BYTES) return;
      filesScanned += 1;
      let raw: string;
      try { raw = await readFile(canonical, "utf8"); }
      catch { return; }
      if (raw.includes("\0")) return;
      const lines = decodeText(raw).text.split("\n");
      for (let index = 0; index < lines.length; index++) {
        const line = lines[index] ?? "";
        const matched = expression
          ? expression.test(line)
          : (caseSensitive ? line : line.toLowerCase()).includes(needle);
        if (!matched) continue;
        matches.push({
          path: guard.display(canonical),
          line: index + 1,
          text: line.length <= 320 ? line : `${line.slice(0, 317)}...`,
        });
        if (matches.length >= maxResults) {
          truncated = true;
          return;
        }
      }
    };

    const walk = async (canonical: string): Promise<void> => {
      if (truncated) return;
      const info = await stat(canonical);
      if (info.isFile()) return searchFile(canonical);
      if (!info.isDirectory()) return;
      const directory = await opendir(canonical);
      const entries = [];
      for await (const entry of directory) entries.push(entry);
      entries.sort((lhs, rhs) => searchEntryPriority(lhs.name) - searchEntryPriority(rhs.name) || lhs.name.localeCompare(rhs.name));
      for (const entry of entries) {
        if (truncated) break;
        if (entry.isSymbolicLink()) continue;
        if (entry.isDirectory() && SEARCH_IGNORED_DIRECTORIES.has(entry.name)) continue;
        const child = path.join(canonical, entry.name);
        if (entry.isDirectory()) await walk(child);
        else if (entry.isFile()) await searchFile(child);
      }
    };

    await walk(start);
    matches.sort((lhs, rhs) => lhs.path === rhs.path ? lhs.line - rhs.line : lhs.path < rhs.path ? -1 : 1);
    return { matches, filesScanned, truncated };
  }

  async listFiles(request: ListFilesRequest = {}): Promise<ListFilesResult> {
    const guard = this.#requireGuard();
    if ((request.path ?? ".").split(/[\\/]+/).some(part => SEARCH_IGNORED_DIRECTORIES.has(part))) {
      throw new Error("List path points into generated or internal workspace state.");
    }
    const maxResults = Math.min(500, Math.max(1, request.maxResults ?? 120));
    const maxDepth = Math.min(12, Math.max(0, request.maxDepth ?? 4));
    const start = await guard.resolve(request.path ?? ".");
    const entries: ListFilesResult["entries"] = [];
    let truncated = false;
    let resultLimitReached = false;
    let depthLimited = false;

    const append = (canonical: string, kind: "file" | "directory"): boolean => {
      if (entries.length >= maxResults) {
        truncated = true;
        resultLimitReached = true;
        return false;
      }
      entries.push({ path: guard.display(canonical), kind });
      return true;
    };

    const walk = async (canonical: string, depth: number): Promise<void> => {
      if (resultLimitReached) return;
      const info = await stat(canonical);
      if (info.isFile()) {
        append(canonical, "file");
        return;
      }
      if (!info.isDirectory()) return;
      const directory = await opendir(canonical);
      const children = [];
      for await (const entry of directory) children.push(entry);
      children.sort((lhs, rhs) => searchEntryPriority(lhs.name) - searchEntryPriority(rhs.name) || lhs.name.localeCompare(rhs.name));
      for (const entry of children) {
        if (resultLimitReached) break;
        if (entry.isSymbolicLink()) continue;
        if (entry.isDirectory() && SEARCH_IGNORED_DIRECTORIES.has(entry.name)) continue;
        const child = path.join(canonical, entry.name);
        if (entry.isDirectory()) {
          if (!append(child, "directory")) break;
          if (depth < maxDepth) await walk(child, depth + 1);
          else {
            const nested = await opendir(child);
            for await (const _ of nested) {
              truncated = true;
              depthLimited = true;
              break;
            }
          }
        } else if (entry.isFile()) {
          if (!append(child, "file")) break;
        }
      }
    };

    await walk(start, 0);
    return { entries, truncated, resultLimitReached, depthLimited };
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
      const source = await guard.resolve(change.path, {
        allowMissing: change.fileOp?.kind === "create",
        allowSymlink: request.unsafe === true,
      });
      if (sourcePaths.has(source)) throw new Error(`Multiple changes target the same source file: ${change.path}`);
      sourcePaths.add(source);
      prepared.push(await this.#prepareOne(change, source, request.unsafe === true));
    }
    for (const item of prepared) {
      for (const state of item.states) {
        if (targetPaths.has(state.path)) throw new Error(`Multiple final states target the same file: ${state.path}`);
        targetPaths.add(state.path);
      }
    }
    return prepared;
  }

  async #prepareOne(change: FileChange, canonical: string, unsafe: boolean): Promise<PreparedFile> {
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
      const anchors = freshAnchors(normalized, result.diff!);
      if (anchors) result.anchors = anchors;
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
    const snapshot = change.snapshot ? this.snapshots.get(change.snapshot) : undefined;
    if (snapshot) ensureSnapshotPath(snapshot, canonical);
    if (!snapshot && !unsafe) throw new Error(`Snapshot handle is required before editing existing file: ${change.path}`);
    const snapshotText = snapshot?.text ?? before;
    const requested = await concretizeEdits(change.path, snapshotText, change.edits ?? [], this.#blockResolver);
    if (this.#enforceSeenLines && !unsafe) {
      if (!snapshot) throw new Error(`Snapshot handle is required before editing existing file: ${change.path}`);
      const unseen = requiredSeenRanges(requested).filter(range => !snapshot.seen.covers(range.start, range.end));
      if (unseen.length > 0) {
        const rendered = unseen.slice(0, 8).map(range => `${range.start}${range.end === range.start ? "" : `..${range.end}`}`).join(", ");
        throw new Error(`Edit rejected because snapshot ${snapshot.handle} did not display required lines: ${rendered}. Re-read those lines first.`);
      }
    }

    let concrete = requested;
    const warnings: string[] = [];
    const liveMatches = snapshot ? digestText(before) === snapshot.digest && before === snapshot.text : true;
    if (snapshot && !liveMatches) {
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
      const destination = await guard.resolve(change.fileOp.destination, { allowMissing: true, allowSymlink: unsafe });
      if (destination === canonical) throw new Error("Move destination is the same as source.");
      try {
        await stat(destination);
        throw new Error(`Move destination already exists: ${change.fileOp.destination}`);
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      }
      const diff = compactDiff(sourceDisplay, before, after);
      const anchors = freshAnchors(after, diff);
      const result: FileApplyResult = {
        path: sourceDisplay,
        destination: guard.display(destination),
        operation: "move",
        diff,
        ...(anchors ? { anchors } : {}),
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
          const seen = liveMatches && snapshot !== undefined
            ? propagateSeen(snapshot, concrete, addressableLines(after).length)
            : [];
          for (const range of diffSeenRanges(diff)) seen.push(range);
          const next = this.snapshots.record(destination, after, seen);
          result.snapshot = next.handle;
        },
      };
    }

    if (after === before) throw new Error(`Edits to ${change.path} produced no changes.`);
    const diff: CompactDiff = compactDiff(sourceDisplay, before, after);
    const anchors = freshAnchors(after, diff);
    const result: FileApplyResult = {
      path: sourceDisplay,
      operation: "update",
      diff,
      ...(anchors ? { anchors } : {}),
      warnings,
    };
    return {
      result,
      before,
      after,
      canonicalSource: canonical,
      canonicalDestination: canonical,
      states: [{ path: canonical, content: encodeText(after, envelope), mode, expectedDigest: rawDigest(raw) }],
      postCommit: () => {
        const seen = liveMatches && snapshot !== undefined
          ? propagateSeen(snapshot, concrete, addressableLines(after).length)
          : [];
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

