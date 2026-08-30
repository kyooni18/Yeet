import type { FetchLike, RetryPolicy } from "./types.js";
export interface ProviderFetchOptions {
    provider: string;
    fetch?: FetchLike;
    timeoutMs?: number;
    retry?: RetryPolicy;
    signal?: AbortSignal;
}
export declare function providerFetch(input: RequestInfo | URL, init: RequestInit, options: ProviderFetchOptions): Promise<Response>;
export declare function readJson<T>(response: Response): Promise<T>;
//# sourceMappingURL=http.d.ts.map