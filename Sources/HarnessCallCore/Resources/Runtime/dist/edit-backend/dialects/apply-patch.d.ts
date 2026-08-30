import type { ApplyRequest, EditDialect } from "../types.js";
export declare class ApplyPatchDialect implements EditDialect {
    readonly id = "apply_patch";
    parse(input: string, context: {
        snapshots: Readonly<Record<string, string>>;
    }): ApplyRequest;
}
//# sourceMappingURL=apply-patch.d.ts.map