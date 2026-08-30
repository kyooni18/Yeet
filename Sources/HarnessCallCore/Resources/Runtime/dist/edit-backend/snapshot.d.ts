import { IntervalSet } from "./ranges.js";
import type { LineRange } from "./types.js";
export interface Snapshot {
    readonly handle: string;
    readonly path: string;
    readonly digest: string;
    readonly text: string;
    readonly generation: number;
    readonly seen: IntervalSet;
    createdAt: number;
    lastAccessAt: number;
}
export interface SnapshotStoreOptions {
    maxSnapshots?: number;
    maxBytes?: number;
    versionsPerPath?: number;
}
export declare function digestText(text: string): string;
export declare class SnapshotStore {
    #private;
    constructor(options?: SnapshotStoreOptions);
    record(path: string, text: string, seen?: readonly LineRange[]): Snapshot;
    get(handle: string): Snapshot;
    head(path: string): Snapshot | undefined;
    markSeen(handle: string, start: number, end?: number): void;
    relocate(from: string, to: string): void;
    invalidatePath(path: string): void;
}
//# sourceMappingURL=snapshot.d.ts.map