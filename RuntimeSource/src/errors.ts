export class ProviderError extends Error {
  readonly provider: string;
  readonly retryable: boolean;
  readonly cause?: unknown;

  constructor(message: string, options: { provider: string; retryable?: boolean; cause?: unknown }) {
    super(message);
    this.name = "ProviderError";
    this.provider = options.provider;
    this.retryable = options.retryable ?? false;
    if (options.cause !== undefined) this.cause = options.cause;
  }
}

export class ProviderHTTPError extends ProviderError {
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
  }) {
    super(`${options.provider} request failed with HTTP ${options.status} ${options.statusText}`, {
      provider: options.provider,
      retryable: options.retryable,
    });
    this.name = "ProviderHTTPError";
    this.status = options.status;
    if (options.responseBody !== undefined) this.responseBody = options.responseBody;
    if (options.requestId !== undefined) this.requestId = options.requestId;
  }
}

export class UnknownProviderError extends Error {
  constructor(provider: string) {
    super(`Unknown provider: ${provider}`);
    this.name = "UnknownProviderError";
  }
}
