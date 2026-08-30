import type { ConcreteEdit } from "./types.js";
export interface RebaseResult {
    edits: ConcreteEdit[];
    offset: number | null;
    warning?: string;
}
export declare function rebaseEdits(previousText: string, currentText: string, edits: readonly ConcreteEdit[]): RebaseResult | null;
//# sourceMappingURL=rebase.d.ts.map