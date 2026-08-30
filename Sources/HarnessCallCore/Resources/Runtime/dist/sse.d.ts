export interface SSEMessage {
    event?: string;
    data: string;
    id?: string;
}
export declare function parseSSE(response: Response): AsyncGenerator<SSEMessage>;
//# sourceMappingURL=sse.d.ts.map