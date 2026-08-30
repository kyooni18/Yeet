export interface PlannedFileState {
    path: string;
    content?: string;
    mode?: number;
    expectedDigest?: string;
    expectMissing?: boolean;
}
export declare class TransactionalWriter {
    #private;
    readonly journalDir: string;
    constructor(journalDir?: string);
    initialize(): Promise<void>;
    recoverPending(): Promise<void>;
    commit(states: readonly PlannedFileState[]): Promise<void>;
}
//# sourceMappingURL=transaction.d.ts.map