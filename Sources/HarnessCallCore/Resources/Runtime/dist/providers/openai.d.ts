import type { ProviderCallRequest, CallResult, FetchLike, ProviderAdapter, StreamEvent } from "../types.js";
export interface OpenAIProviderOptions {
    apiKey?: string;
    baseUrl?: string;
    organization?: string;
    project?: string;
    fetch?: FetchLike;
}
export declare class OpenAIProvider implements ProviderAdapter {
    #private;
    readonly id = "openai";
    constructor(options?: OpenAIProviderOptions);
    listModels(): Promise<string[]>;
    complete(request: ProviderCallRequest): Promise<CallResult>;
    stream(request: ProviderCallRequest): AsyncIterable<StreamEvent>;
}
//# sourceMappingURL=openai.d.ts.map