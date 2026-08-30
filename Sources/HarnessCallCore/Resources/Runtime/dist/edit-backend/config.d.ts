import type { ModelDialectRule } from "./types.js";
export interface EditBackendConfig {
    defaultDialect: string;
    models: ModelDialectRule[];
    enforceSeenLines: boolean;
    transactionDir: string;
}
export declare function configDirectory(): string;
export declare function loadEditBackendConfig(): Promise<EditBackendConfig>;
//# sourceMappingURL=config.d.ts.map