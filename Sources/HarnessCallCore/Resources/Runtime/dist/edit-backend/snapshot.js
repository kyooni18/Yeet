import { createHash, randomBytes } from "node:crypto";
import { IntervalSet } from "./ranges.js";
export function digestText(text) {
    return createHash("sha256").update(text, "utf8").digest("hex");
}
function newHandle() {
    return `s_${randomBytes(12).toString("base64url")}`;
}
export class SnapshotStore {
    #maxSnapshots;
    #maxBytes;
    #versionsPerPath;
    #byHandle = new Map();
    #byPath = new Map();
    #generation = new Map();
    #bytes = 0;
    constructor(options = {}) {
        this.#maxSnapshots = options.maxSnapshots ?? 512;
        this.#maxBytes = options.maxBytes ?? 64 * 1024 * 1024;
        this.#versionsPerPath = options.versionsPerPath ?? 6;
    }
    record(path, text, seen = []) {
        const digest = digestText(text);
        const history = this.#byPath.get(path) ?? [];
        const existing = history.find(snapshot => snapshot.digest === digest && snapshot.text === text);
        if (existing) {
            for (const range of seen)
                existing.seen.add(range.start, range.end);
            existing.lastAccessAt = Date.now();
            const reordered = [existing, ...history.filter(item => item !== existing)];
            this.#byPath.set(path, reordered);
            return existing;
        }
        const generation = (this.#generation.get(path) ?? 0) + 1;
        this.#generation.set(path, generation);
        const now = Date.now();
        const snapshot = {
            handle: newHandle(),
            path,
            digest,
            text,
            generation,
            seen: new IntervalSet(seen),
            createdAt: now,
            lastAccessAt: now,
        };
        this.#byHandle.set(snapshot.handle, snapshot);
        this.#bytes += text.length * 2;
        const nextHistory = [snapshot, ...history];
        while (nextHistory.length > this.#versionsPerPath) {
            const removed = nextHistory.pop();
            if (removed)
                this.#drop(removed);
        }
        this.#byPath.set(path, nextHistory);
        this.#evict();
        return snapshot;
    }
    get(handle) {
        const snapshot = this.#byHandle.get(handle);
        if (!snapshot)
            throw new Error(`Unknown or expired snapshot handle: ${handle}`);
        snapshot.lastAccessAt = Date.now();
        return snapshot;
    }
    head(path) {
        const snapshot = this.#byPath.get(path)?.[0];
        if (snapshot)
            snapshot.lastAccessAt = Date.now();
        return snapshot;
    }
    markSeen(handle, start, end = start) {
        this.get(handle).seen.add(start, end);
    }
    relocate(from, to) {
        const source = this.#byPath.get(from);
        if (!source)
            return;
        this.#byPath.delete(from);
        const moved = source.map(snapshot => {
            const relocated = {
                ...snapshot,
                path: to,
                seen: snapshot.seen.clone(),
            };
            this.#byHandle.set(relocated.handle, relocated);
            return relocated;
        });
        const existing = this.#byPath.get(to) ?? [];
        const combined = [...moved, ...existing];
        const retained = combined.slice(0, this.#versionsPerPath);
        const retainedHandles = new Set(retained.map(snapshot => snapshot.handle));
        for (const snapshot of combined) {
            if (!retainedHandles.has(snapshot.handle))
                this.#drop(snapshot);
        }
        this.#byPath.set(to, retained);
        const generation = Math.max(this.#generation.get(to) ?? 0, ...retained.map(snapshot => snapshot.generation));
        this.#generation.set(to, generation);
    }
    invalidatePath(path) {
        const history = this.#byPath.get(path);
        if (!history)
            return;
        this.#byPath.delete(path);
        for (const snapshot of history)
            this.#drop(snapshot);
    }
    #drop(snapshot) {
        if (!this.#byHandle.delete(snapshot.handle))
            return;
        this.#bytes -= snapshot.text.length * 2;
    }
    #evict() {
        while (this.#byHandle.size > this.#maxSnapshots || this.#bytes > this.#maxBytes) {
            let oldest;
            for (const snapshot of this.#byHandle.values()) {
                if (!oldest || snapshot.lastAccessAt < oldest.lastAccessAt)
                    oldest = snapshot;
            }
            if (!oldest)
                break;
            const history = this.#byPath.get(oldest.path) ?? [];
            const next = history.filter(item => item.handle !== oldest.handle);
            if (next.length === 0)
                this.#byPath.delete(oldest.path);
            else
                this.#byPath.set(oldest.path, next);
            this.#drop(oldest);
        }
    }
}
//# sourceMappingURL=snapshot.js.map