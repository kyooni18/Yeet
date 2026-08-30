import type { ApplyRequest, EditDialect } from "../types.js";
export declare class SloppyDialect implements EditDialect {
    readonly id = "sloppy";
    parse(input: string, context: {
        getSnapshotText(handle: string): string;
    }): ApplyRequest;
}
//# sourceMappingURL=sloppy.d.ts.map