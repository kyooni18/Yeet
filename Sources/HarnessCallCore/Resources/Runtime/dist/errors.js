export class ProviderError extends Error {
    provider;
    retryable;
    cause;
    constructor(message, options) {
        super(message);
        this.name = "ProviderError";
        this.provider = options.provider;
        this.retryable = options.retryable ?? false;
        if (options.cause !== undefined)
            this.cause = options.cause;
    }
}
export class ProviderHTTPError extends ProviderError {
    status;
    responseBody;
    requestId;
    constructor(options) {
        super(`${options.provider} request failed with HTTP ${options.status} ${options.statusText}`, {
            provider: options.provider,
            retryable: options.retryable,
        });
        this.name = "ProviderHTTPError";
        this.status = options.status;
        if (options.responseBody !== undefined)
            this.responseBody = options.responseBody;
        if (options.requestId !== undefined)
            this.requestId = options.requestId;
    }
}
export class UnknownProviderError extends Error {
    constructor(provider) {
        super(`Unknown provider: ${provider}`);
        this.name = "UnknownProviderError";
    }
}
//# sourceMappingURL=errors.js.map