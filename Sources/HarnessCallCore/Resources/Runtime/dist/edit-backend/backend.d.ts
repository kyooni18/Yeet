import { ModelDialectSelector } from "./model-dialect.js";
import { SnapshotStore } from "./snapshot.js";
import { TransactionalWriter } from "./transaction.js";
import type { ApplyRequest, ApplyResult, BlockResolver, DiagnosticsProvider, EditDialect, ModelDialectRule, ReadRequest, ReadResult } from "./types.js";
export interface EditBackendOptions {
    root: string;
    snapshots?: SnapshotStore;
    blockResolver?: BlockResolver;
    diagnostics?: DiagnosticsProvider;
    enforceSeenLines?: boolean;
    transactionDir?: string;
    defaultDialect?: string;
    modelDialects?: ModelDialectRule[];
}
export declare class EditBackend {
    #private;
    readonly root: string;
    readonly snapshots: SnapshotStore;
    readonly diagnostics: DiagnosticsProvider;
    readonly transaction: TransactionalWriter;
    readonly selector: ModelDialectSelector;
    constructor(options: EditBackendOptions);
    initialize(): Promise<void>;
    registerDialect(dialect: EditDialect): void;
    read(request: ReadRequest): Promise<ReadResult>;
    preflight(request: ApplyRequest): Promise<ApplyResult>;
    apply(request: ApplyRequest): Promise<ApplyResult>;
    applyDialect(input: string, options?: {
        dialect?: string;
        model?: string;
        snapshots?: Record<string, string>;
        diagnostics?: boolean;
    }): Promise<ApplyResult>;
}
//# sourceMappingURL=backend.d.ts.map