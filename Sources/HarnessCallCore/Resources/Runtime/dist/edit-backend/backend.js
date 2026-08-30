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
import { SnapshotStore, digestText } from "./snapshot.js";
import { addressableLines, decodeText, encodeText, formatNumbered } from "./text.js";
import { TransactionalWriter } from "./transaction.js";
const noopDiagnostics = { diagnose: async () => [] };
function rawDigest(raw) {
    return createHash("sha256").update(raw, "utf8").digest("hex");
}
function ensureSnapshotPath(snapshot, canonical) {
    if (snapshot.path !== canonical) {
        throw new Error(`Snapshot ${snapshot.handle} belongs to ${snapshot.path}, not ${canonical}.`);
    }
}
function propagateSeen(snapshot, edits, afterLineCount) {
    const points = [...snapshot.seen.values()];
    const result = new Set();
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
                if (line > edit.range.end)
                    line += newLength - oldLength;
            }
            else {
                const inserted = edit.text.replace(/\r\n/g, "\n").replace(/\n$/, "").split("\n").length;
                let position;
                switch (edit.at.kind) {
                    case "start":
                        position = 0;
                        break;
                    case "end":
                        position = Number.MAX_SAFE_INTEGER;
                        break;
                    case "before":
                        position = edit.at.line - 1;
                        break;
                    case "after":
                        position = edit.at.line;
                        break;
                }
                if (position < line)
                    line += inserted;
            }
        }
        if (!removed && line >= 1 && line <= afterLineCount)
            result.add(line);
    }
    const sorted = [...result].sort((a, b) => a - b);
    const ranges = [];
    for (const line of sorted) {
        const tail = ranges.at(-1);
        if (tail && tail.end + 1 === line)
            tail.end = line;
        else
            ranges.push({ start: line, end: line });
    }
    return ranges;
}
export class EditBackend {
    root;
    snapshots;
    diagnostics;
    transaction;
    selector;
    #blockResolver;
    #enforceSeenLines;
    #dialects = new Map();
    #guard;
    #queue = Promise.resolve();
    constructor(options) {
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
    async initialize() {
        this.#guard = await PathGuard.create(this.root);
        await this.transaction.initialize();
    }
    registerDialect(dialect) {
        this.#dialects.set(dialect.id, dialect);
    }
    async read(request) {
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
    async preflight(request) {
        const prepared = await this.#prepare(request);
        return {
            files: prepared.map(item => item.result),
            diagnostics: [],
        };
    }
    async apply(request) {
        return this.#serialize(async () => {
            const prepared = await this.#prepare(request);
            const states = prepared.flatMap(item => item.states);
            await this.transaction.commit(states);
            for (const item of prepared)
                item.postCommit?.();
            const changedPaths = prepared.flatMap(item => item.canonicalDestination ?? item.canonicalSource ?? []);
            const diagnostics = request.diagnostics === false ? [] : await this.diagnostics.diagnose(changedPaths);
            return { files: prepared.map(item => item.result), diagnostics };
        });
    }
    async applyDialect(input, options = {}) {
        const dialectId = options.dialect ?? this.selector.resolve(options.model);
        if (dialectId === "structured")
            throw new Error("structured dialect accepts ApplyRequest directly; call apply().");
        const dialect = this.#dialects.get(dialectId);
        if (!dialect)
            throw new Error(`Unknown edit dialect: ${dialectId}`);
        const parsed = await dialect.parse(input, {
            snapshots: options.snapshots ?? {},
            getSnapshotText: handle => this.snapshots.get(handle).text,
        });
        return this.apply({ ...parsed, ...(options.diagnostics !== undefined ? { diagnostics: options.diagnostics } : {}) });
    }
    async #prepare(request) {
        if (request.changes.length === 0)
            throw new Error("Apply request contains no changes.");
        const guard = this.#requireGuard();
        const sourcePaths = new Set();
        const targetPaths = new Set();
        const prepared = [];
        for (const change of request.changes) {
            const source = await guard.resolve(change.path, { allowMissing: change.fileOp?.kind === "create" });
            if (sourcePaths.has(source))
                throw new Error(`Multiple changes target the same source file: ${change.path}`);
            sourcePaths.add(source);
            prepared.push(await this.#prepareOne(change, source));
        }
        for (const item of prepared) {
            for (const state of item.states) {
                if (targetPaths.has(state.path))
                    throw new Error(`Multiple final states target the same file: ${state.path}`);
                targetPaths.add(state.path);
            }
        }
        return prepared;
    }
    async #prepareOne(change, canonical) {
        const guard = this.#requireGuard();
        if (change.fileOp?.kind === "create") {
            if (change.snapshot)
                throw new Error("Create operations must not carry a snapshot handle.");
            if (change.edits?.length)
                throw new Error("Create operations use fileOp.text and cannot include line edits.");
            try {
                await stat(canonical);
                throw new Error(`Refusing to create existing file: ${change.path}`);
            }
            catch (error) {
                if (error.code !== "ENOENT")
                    throw error;
            }
            const normalized = change.fileOp.text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
            const result = {
                path: guard.display(canonical),
                operation: "create",
                diff: compactDiff(guard.display(canonical), "", normalized),
                warnings: [],
            };
            const state = {
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
                    const diff = result.diff;
                    const snapshot = this.snapshots.record(canonical, normalized, diffSeenRanges(diff));
                    result.snapshot = snapshot.handle;
                },
            };
        }
        const raw = await readFile(canonical, "utf8");
        const fileStat = await stat(canonical);
        const envelope = decodeText(raw);
        const before = envelope.text;
        if (!change.snapshot)
            throw new Error(`Snapshot handle is required before editing existing file: ${change.path}`);
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
        const warnings = [];
        const liveMatches = digestText(before) === snapshot.digest && before === snapshot.text;
        if (!liveMatches) {
            const rebased = rebaseEdits(snapshot.text, before, requested);
            if (!rebased)
                throw new Error(`Snapshot ${snapshot.handle} is stale and its edit anchors cannot be safely rebased. Re-read ${change.path}.`);
            concrete = rebased.edits;
            if (rebased.warning)
                warnings.push(rebased.warning);
        }
        if (change.fileOp?.kind === "delete" && concrete.length > 0)
            throw new Error("Do not combine line edits with file deletion.");
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
            if (destination === canonical)
                throw new Error("Move destination is the same as source.");
            try {
                await stat(destination);
                throw new Error(`Move destination already exists: ${change.fileOp.destination}`);
            }
            catch (error) {
                if (error.code !== "ENOENT")
                    throw error;
            }
            const diff = compactDiff(sourceDisplay, before, after);
            const result = {
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
                    for (const range of diffSeenRanges(diff))
                        seen.push(range);
                    const next = this.snapshots.record(destination, after, seen);
                    result.snapshot = next.handle;
                },
            };
        }
        if (after === before)
            throw new Error(`Edits to ${change.path} produced no changes.`);
        const diff = compactDiff(sourceDisplay, before, after);
        const result = { path: sourceDisplay, operation: "update", diff, warnings };
        return {
            result,
            before,
            after,
            canonicalSource: canonical,
            canonicalDestination: canonical,
            states: [{ path: canonical, content: encodeText(after, envelope), mode, expectedDigest: rawDigest(raw) }],
            postCommit: () => {
                const seen = liveMatches ? propagateSeen(snapshot, concrete, addressableLines(after).length) : [];
                for (const range of diffSeenRanges(diff))
                    seen.push(range);
                const next = this.snapshots.record(canonical, after, seen);
                result.snapshot = next.handle;
            },
        };
    }
    #requireGuard() {
        if (!this.#guard)
            throw new Error("EditBackend.initialize() must be called before use.");
        return this.#guard;
    }
    async #serialize(operation) {
        const previous = this.#queue;
        let release;
        this.#queue = new Promise(resolve => { release = resolve; });
        await previous;
        try {
            return await operation();
        }
        finally {
            release();
        }
    }
}
//# sourceMappingURL=backend.js.map