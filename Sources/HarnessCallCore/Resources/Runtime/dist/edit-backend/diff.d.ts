import type { CompactDiff } from "./types.js";
export declare function compactDiff(path: string, before: string, after: string, context?: number): CompactDiff;
export declare function diffSeenRanges(diff: CompactDiff): Array<{
    start: number;
    end: number;
}>;
//# sourceMappingURL=diff.d.ts.map