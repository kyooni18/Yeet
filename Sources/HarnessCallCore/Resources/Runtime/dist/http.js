import { ProviderError, ProviderHTTPError } from "./errors.js";
const DEFAULT_RETRY = {
    maxAttempts: 3,
    baseDelayMs: 300,
    maxDelayMs: 5_000,
    jitter: 0.2,
};
function isRetryableStatus(status) {
    return status === 408 || status === 409 || status === 425 || status === 429 || status >= 500;
}
function retryAfterMs(response) {
    const value = response.headers.get("retry-after");
    if (!value)
        return undefined;
    const seconds = Number(value);
    if (Number.isFinite(seconds))
        return Math.max(0, seconds * 1_000);
    const date = Date.parse(value);
    if (Number.isFinite(date))
        return Math.max(0, date - Date.now());
    return undefined;
}
function delayForAttempt(attempt, policy, response) {
    const fromHeader = response ? retryAfterMs(response) : undefined;
    if (fromHeader !== undefined)
        return Math.min(fromHeader, policy.maxDelayMs);
    const exponential = Math.min(policy.maxDelayMs, policy.baseDelayMs * 2 ** Math.max(0, attempt - 1));
    const spread = exponential * policy.jitter;
    return Math.max(0, exponential + (Math.random() * 2 - 1) * spread);
}
function sleep(ms, signal) {
    if (ms <= 0)
        return Promise.resolve();
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
function makeAttemptSignal(signal, timeoutMs) {
    const timeoutSignal = timeoutMs !== undefined ? AbortSignal.timeout(timeoutMs) : undefined;
    if (signal && timeoutSignal)
        return AbortSignal.any([signal, timeoutSignal]);
    return signal ?? timeoutSignal;
}
export async function providerFetch(input, init, options) {
    const fetchImpl = options.fetch ?? globalThis.fetch;
    if (!fetchImpl)
        throw new ProviderError("No fetch implementation is available", { provider: options.provider });
    const policy = { ...DEFAULT_RETRY, ...options.retry };
    if (policy.maxAttempts < 1)
        policy.maxAttempts = 1;
    let lastError;
    for (let attempt = 1; attempt <= policy.maxAttempts; attempt += 1) {
        const attemptSignal = makeAttemptSignal(options.signal, options.timeoutMs);
        try {
            const response = await fetchImpl(input, attemptSignal ? { ...init, signal: attemptSignal } : init);
            if (response.ok)
                return response;
            const retryable = isRetryableStatus(response.status);
            if (retryable && attempt < policy.maxAttempts) {
                await response.body?.cancel().catch(() => undefined);
                await sleep(delayForAttempt(attempt, policy, response), options.signal);
                continue;
            }
            const responseBody = await response.text().catch(() => undefined);
            throw new ProviderHTTPError({
                provider: options.provider,
                status: response.status,
                statusText: response.statusText,
                retryable,
                ...(responseBody !== undefined ? { responseBody } : {}),
                ...(response.headers.get("x-request-id") ? { requestId: response.headers.get("x-request-id") } : {}),
            });
        }
        catch (error) {
            if (error instanceof ProviderHTTPError)
                throw error;
            if (options.signal?.aborted)
                throw error;
            lastError = error;
            if (attempt >= policy.maxAttempts)
                break;
            await sleep(delayForAttempt(attempt, policy), options.signal);
        }
    }
    throw new ProviderError(`${options.provider} request failed`, {
        provider: options.provider,
        retryable: true,
        cause: lastError,
    });
}
export async function readJson(response) {
    return (await response.json());
}
//# sourceMappingURL=http.js.map