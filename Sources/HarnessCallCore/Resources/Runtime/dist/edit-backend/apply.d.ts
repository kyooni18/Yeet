import type { BlockResolver, ConcreteEdit, Edit, LineRange } from "./types.js";
export interface ApplyTextResult {
    text: string;
    concrete: ConcreteEdit[];
}
export declare function concretizeEdits(path: string, snapshotText: string, edits: readonly Edit[], blockResolver?: BlockResolver): Promise<ConcreteEdit[]>;
export declare function requiredSeenRanges(edits: readonly ConcreteEdit[]): LineRange[];
export declare function applyConcreteEdits(text: string, edits: readonly ConcreteEdit[]): ApplyTextResult;
//# sourceMappingURL=apply.d.ts.map