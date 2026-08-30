export declare class ProviderError extends Error {
    readonly provider: string;
    readonly retryable: boolean;
    readonly cause?: unknown;
    constructor(message: string, options: {
        provider: string;
        retryable?: boolean;
        cause?: unknown;
    });
}
export declare class ProviderHTTPError extends ProviderError {
    readonly status: number;
    readonly responseBody?: string;
    readonly requestId?: string;
    constructor(options: {
        provider: string;
        status: number;
        statusText: string;
        retryable: boolean;
        responseBody?: string;
        requestId?: string;
    });
}
export declare class UnknownProviderError extends Error {
    constructor(provider: string);
}
//# sourceMappingURL=errors.d.ts.map