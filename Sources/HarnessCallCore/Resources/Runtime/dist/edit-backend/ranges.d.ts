import type { LineRange } from "./types.js";
export declare class IntervalSet {
    #private;
    constructor(ranges?: readonly LineRange[]);
    add(start: number, end?: number): void;
    covers(start: number, end?: number): boolean;
    has(line: number): boolean;
    clone(): IntervalSet;
    toJSON(): LineRange[];
    values(): Generator<number>;
}
//# sourceMappingURL=ranges.d.ts.map