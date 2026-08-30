import type { ProviderCallRequest, CallResult, FetchLike, ProviderAdapter, StreamEvent } from "../types.js";
export interface AnthropicProviderOptions {
    apiKey?: string;
    baseUrl?: string;
    version?: string;
    fetch?: FetchLike;
}
export declare class AnthropicProvider implements ProviderAdapter {
    #private;
    readonly id = "anthropic";
    constructor(options?: AnthropicProviderOptions);
    listModels(): Promise<string[]>;
    complete(request: ProviderCallRequest): Promise<CallResult>;
    stream(request: ProviderCallRequest): AsyncIterable<StreamEvent>;
}
//# sourceMappingURL=anthropic.d.ts.map