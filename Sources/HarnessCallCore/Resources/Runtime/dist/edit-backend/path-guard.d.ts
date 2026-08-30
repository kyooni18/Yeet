export declare class PathGuard {
    readonly root: string;
    readonly realRoot: string;
    private constructor();
    static create(root: string): Promise<PathGuard>;
    resolve(input: string, options?: {
        allowMissing?: boolean;
    }): Promise<string>;
    display(canonical: string): string;
}
//# sourceMappingURL=path-guard.d.ts.map