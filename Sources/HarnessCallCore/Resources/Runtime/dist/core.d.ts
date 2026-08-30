import type { CallRequest, CallResult, ProviderAdapter, ProviderId, StreamEvent } from "./types.js";
export declare class CallCore {
    #private;
    constructor(providers?: Iterable<ProviderAdapter>);
    register(provider: ProviderAdapter): this;
    unregister(provider: ProviderId): boolean;
    has(provider: ProviderId): boolean;
    listProviders(): ProviderId[];
    /**
     * Fetch provider-local model identifiers from a registered provider.
     * Providers are looked up by their configured id, so this works for both
     * built-in and dynamically registered OpenAI-compatible providers.
     */
    listModels(provider: ProviderId): Promise<string[]>;
    fetchAvailableModels(provider: ProviderId): Promise<string[]>;
    complete(request: CallRequest): Promise<CallResult>;
    stream(request: CallRequest): AsyncIterable<StreamEvent>;
}
//# sourceMappingURL=core.d.ts.map