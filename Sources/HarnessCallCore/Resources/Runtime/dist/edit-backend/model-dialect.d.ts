import type { ModelDialectRule } from "./types.js";
export declare class ModelDialectSelector {
    readonly defaultDialect: string;
    readonly rules: readonly ModelDialectRule[];
    constructor(defaultDialect?: string, rules?: readonly ModelDialectRule[]);
    resolve(modelId?: string): string;
}
//# sourceMappingURL=model-dialect.d.ts.map