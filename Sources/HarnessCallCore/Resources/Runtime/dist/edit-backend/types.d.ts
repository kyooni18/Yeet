export interface LineRange {
    start: number;
    end: number;
}
export interface ReplaceEdit {
    kind: "replace";
    range: LineRange;
    text: string;
}
export interface DeleteEdit {
    kind: "delete";
    range: LineRange;
}
export interface InsertEdit {
    kind: "insert";
    at: {
        kind: "start";
    } | {
        kind: "end";
    } | {
        kind: "before";
        line: number;
    } | {
        kind: "after";
        line: number;
    };
    text: string;
}
export interface ReplaceBlockEdit {
    kind: "replaceBlock";
    line: number;
    text: string;
}
export interface InsertAfterBlockEdit {
    kind: "insertAfterBlock";
    line: number;
    text: string;
}
export interface DeleteBlockEdit {
    kind: "deleteBlock";
    line: number;
}
export type Edit = ReplaceEdit | DeleteEdit | InsertEdit | ReplaceBlockEdit | InsertAfterBlockEdit | DeleteBlockEdit;
export type ConcreteEdit = ReplaceEdit | DeleteEdit | InsertEdit;
export type FileOperation = {
    kind: "create";
    text: string;
    mode?: number;
} | {
    kind: "delete";
} | {
    kind: "move";
    destination: string;
};
export interface FileChange {
    path: string;
    snapshot?: string;
    edits?: Edit[];
    fileOp?: FileOperation;
}
export interface ApplyRequest {
    changes: FileChange[];
    diagnostics?: boolean;
}
export interface ReadRequest {
    path: string;
    startLine?: number;
    endLine?: number;
}
export interface ReadResult {
    path: string;
    snapshot: string;
    startLine: number;
    endLine: number;
    totalLines: number;
    content: string;
    numbered: string;
}
export interface DiffHunk {
    oldStart: number;
    oldEnd: number;
    newStart: number;
    newEnd: number;
    lines: string[];
    truncated: boolean;
}
export interface CompactDiff {
    path: string;
    hunks: DiffHunk[];
}
export interface Diagnostic {
    path: string;
    severity: "error" | "warning" | "info";
    message: string;
    line?: number;
    column?: number;
    source?: string;
}
export interface FileApplyResult {
    path: string;
    destination?: string;
    operation: "create" | "update" | "delete" | "move";
    snapshot?: string;
    diff?: CompactDiff;
    warnings: string[];
}
export interface ApplyResult {
    files: FileApplyResult[];
    diagnostics: Diagnostic[];
}
export interface BlockResolution {
    start: number;
    end: number;
}
export interface BlockResolverInput {
    path: string;
    text: string;
    line: number;
}
export type BlockResolver = (input: BlockResolverInput) => BlockResolution | null | Promise<BlockResolution | null>;
export interface DiagnosticsProvider {
    diagnose(paths: readonly string[]): Promise<Diagnostic[]>;
}
export interface DialectParseContext {
    snapshots: Readonly<Record<string, string>>;
    getSnapshotText(handle: string): string;
}
export interface EditDialect {
    readonly id: string;
    parse(input: string, context: DialectParseContext): ApplyRequest | Promise<ApplyRequest>;
}
export interface ModelDialectRule {
    match: string;
    dialect: string;
}
//# sourceMappingURL=types.d.ts.map