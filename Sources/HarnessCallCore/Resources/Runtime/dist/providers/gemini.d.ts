import type { ProviderCallRequest, CallResult, FetchLike, ProviderAdapter, StreamEvent } from "../types.js";
export interface GeminiProviderOptions {
    apiKey?: string;
    accessToken?: string;
    projectId?: string;
    baseUrl?: string;
    fetch?: FetchLike;
}
export declare class GeminiProvider implements ProviderAdapter {
    #private;
    readonly id = "gemini";
    constructor(options?: GeminiProviderOptions);
    listModels(): Promise<string[]>;
    complete(request: ProviderCallRequest): Promise<CallResult>;
    stream(request: ProviderCallRequest): AsyncIterable<StreamEvent>;
}
//# sourceMappingURL=gemini.d.ts.map