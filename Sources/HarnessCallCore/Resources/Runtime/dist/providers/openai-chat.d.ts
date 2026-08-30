import type { ProviderCallRequest, CallResult, FetchLike, ProviderAdapter, StreamEvent } from "../types.js";
export interface OpenAIChatProviderOptions {
    id?: string;
    apiKey?: string;
    baseUrl?: string;
    headers?: Record<string, string>;
    requireApiKey?: boolean;
    fetch?: FetchLike;
}
export declare class OpenAIChatProvider implements ProviderAdapter {
    #private;
    readonly id: string;
    constructor(options?: OpenAIChatProviderOptions);
    listModels(): Promise<string[]>;
    complete(request: ProviderCallRequest): Promise<CallResult>;
    stream(request: ProviderCallRequest): AsyncIterable<StreamEvent>;
}
//# sourceMappingURL=openai-chat.d.ts.map