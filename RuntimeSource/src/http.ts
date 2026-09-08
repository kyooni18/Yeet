import { isQuotaExhausted, ProviderError, ProviderHTTPError } from "./errors.js";
import type { FetchLike, RetryPolicy } from "./types.js";

export type ProviderFetchOutcome = "success" | "retry" | "error";

/** Metadata for one provider HTTP attempt. Request bodies and credentials are never included. */
export interface ProviderFetchLog {
  /** ISO-8601 time at which the attempt finished. */
  timestamp: string;
  provider: string;
  method: string;
  url: string;
  attempt: number;
  outcome: ProviderFetchOutcome;
  durationMs: number;
  status?: number;
  statusText?: string;
  requestId?: string;
  /** Actual backoff before the next attempt, when a retry was scheduled. */
  retryDelayMs?: number;
  error?: string;
}

export type ProviderFetchLogger = (entry: ProviderFetchLog) => void;
type ProviderFetchLogEntry = Omit<ProviderFetchLog, "timestamp">;

const DEFAULT_RETRY: Required<RetryPolicy> = {
  maxAttempts: 3,
  baseDelayMs: 300,
  maxDelayMs: 5_000,
  jitter: 0.2,
};

export interface ProviderFetchOptions {
  provider: string;
  fetch?: FetchLike;
  apiCallLogger?: ProviderFetchLogger;
  timeoutMs?: number;
  retry?: RetryPolicy;
  signal?: AbortSignal;
}

function redactedUrl(input: RequestInfo | URL): string {
  try {
    const raw = typeof Request !== "undefined" && input instanceof Request ? input.url : String(input);
    const url = new URL(raw);
    // Query parameters are not needed to identify the endpoint and can carry
    // API keys or OAuth tokens on compatible providers.
    return `${url.origin}${url.pathname}`;
  } catch {
    return "<invalid-url>";
  }
}

function emitLog(logger: ProviderFetchLogger | undefined, entry: ProviderFetchLogEntry): void {
  try {
    logger?.({ timestamp: new Date().toISOString(), ...entry });
  } catch {
    // Logging must never change request behavior.
  }
}

function elapsedMs(start: number): number {
  return Math.max(0, Math.round((Date.now() - start) * 100) / 100);
}

function isRetryableStatus(status: number): boolean {
  return status === 408 || status === 409 || status === 425 || status === 429 || status >= 500;
}

function retryAfterMs(response: Response): number | undefined {
  const value = response.headers.get("retry-after");
  if (!value) return undefined;

  const seconds = Number(value);
  if (Number.isFinite(seconds)) return Math.max(0, seconds * 1_000);

  const date = Date.parse(value);
  if (Number.isFinite(date)) return Math.max(0, date - Date.now());
  return undefined;
}

function delayForAttempt(attempt: number, policy: Required<RetryPolicy>, response?: Response): number {
  const fromHeader = response ? retryAfterMs(response) : undefined;
  if (fromHeader !== undefined) return Math.min(fromHeader, policy.maxDelayMs);

  const exponential = Math.min(policy.maxDelayMs, policy.baseDelayMs * 2 ** Math.max(0, attempt - 1));
  const spread = exponential * policy.jitter;
  return Math.max(0, exponential + (Math.random() * 2 - 1) * spread);
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  if (ms <= 0) return Promise.resolve();
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason ?? new DOMException("Aborted", "AbortError"));
      return;
    }

    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);

    const onAbort = () => {
      clearTimeout(timer);
      reject(signal?.reason ?? new DOMException("Aborted", "AbortError"));
    };
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

function makeAttemptSignal(signal: AbortSignal | undefined, timeoutMs: number | undefined): AbortSignal | undefined {
  const timeoutSignal = timeoutMs !== undefined ? AbortSignal.timeout(timeoutMs) : undefined;
  if (signal && timeoutSignal) return AbortSignal.any([signal, timeoutSignal]);
  return signal ?? timeoutSignal;
}

export async function providerFetch(
  input: RequestInfo | URL,
  init: RequestInit,
  options: ProviderFetchOptions,
): Promise<Response> {
  const fetchImpl = options.fetch ?? globalThis.fetch;
  if (!fetchImpl) throw new ProviderError("No fetch implementation is available", { provider: options.provider });

  const policy: Required<RetryPolicy> = { ...DEFAULT_RETRY, ...options.retry };
  if (policy.maxAttempts < 1) policy.maxAttempts = 1;
  const method = String(init.method ?? "GET").toUpperCase();
  const url = redactedUrl(input);

  let lastError: unknown;

  for (let attempt = 1; attempt <= policy.maxAttempts; attempt += 1) {
    const attemptSignal = makeAttemptSignal(options.signal, options.timeoutMs);
    const startedAt = Date.now();
    try {
      const response = await fetchImpl(input, attemptSignal ? { ...init, signal: attemptSignal } : init);

      if (response.ok) {
        emitLog(options.apiCallLogger, {
          provider: options.provider,
          method,
          url,
          attempt,
          outcome: "success",
          durationMs: elapsedMs(startedAt),
          status: response.status,
          ...(response.headers.get("x-request-id") ? { requestId: response.headers.get("x-request-id")! } : {}),
        });
        return response;
      }

      // OpenAI uses 429 for both temporary throttling and exhausted billing
      // quota. Only the former can recover through backoff.
      let responseBody = response.status === 429
        ? await response.text().catch(() => undefined)
        : undefined;
      const retryable = isRetryableStatus(response.status)
        && !(response.status === 429 && isQuotaExhausted(responseBody));
      if (retryable && attempt < policy.maxAttempts) {
        const retryDelayMs = delayForAttempt(attempt, policy, response);
        emitLog(options.apiCallLogger, {
          provider: options.provider,
          method,
          url,
          attempt,
          outcome: "retry",
          durationMs: elapsedMs(startedAt),
          status: response.status,
          ...(response.statusText ? { statusText: response.statusText } : {}),
          ...(response.headers.get("x-request-id") ? { requestId: response.headers.get("x-request-id")! } : {}),
          retryDelayMs,
        });
        await response.body?.cancel().catch(() => undefined);
        await sleep(retryDelayMs, options.signal);
        continue;
      }

      if (response.status !== 429) responseBody = await response.text().catch(() => undefined);
      emitLog(options.apiCallLogger, {
        provider: options.provider,
        method,
        url,
        attempt,
        outcome: "error",
        durationMs: elapsedMs(startedAt),
        status: response.status,
        ...(response.statusText ? { statusText: response.statusText } : {}),
        ...(response.headers.get("x-request-id") ? { requestId: response.headers.get("x-request-id")! } : {}),
      });
      throw new ProviderHTTPError({
        provider: options.provider,
        status: response.status,
        statusText: response.statusText,
        retryable,
        ...(responseBody !== undefined ? { responseBody } : {}),
        ...(response.headers.get("x-request-id") ? { requestId: response.headers.get("x-request-id")! } : {}),
      });
    } catch (error) {
      if (error instanceof ProviderHTTPError) throw error;
      emitLog(options.apiCallLogger, {
        provider: options.provider,
        method,
        url,
        attempt,
        outcome: options.signal?.aborted ? "error" : (attempt >= policy.maxAttempts ? "error" : "retry"),
        durationMs: elapsedMs(startedAt),
        error: error instanceof Error ? error.name : "UnknownError",
      });
      if (options.signal?.aborted) throw error;
      lastError = error;
      if (attempt >= policy.maxAttempts) break;
      await sleep(delayForAttempt(attempt, policy), options.signal);
    }
  }

  throw new ProviderError(`${options.provider} request failed`, {
    provider: options.provider,
    retryable: true,
    cause: lastError,
  });
}

export async function readJson<T>(response: Response): Promise<T> {
  return (await response.json()) as T;
}
