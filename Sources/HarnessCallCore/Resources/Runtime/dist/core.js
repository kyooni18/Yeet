import { ProviderError, UnknownProviderError } from "./errors.js";
import { parseModelId } from "./types.js";
export class CallCore {
    #providers = new Map();
    constructor(providers = []) {
        for (const provider of providers)
            this.register(provider);
    }
    register(provider) {
        if (!provider.id || provider.id.includes("/")) {
            throw new TypeError(`Provider id must be a non-empty single path segment, got: ${provider.id}`);
        }
        this.#providers.set(provider.id, provider);
        return this;
    }
    unregister(provider) {
        return this.#providers.delete(provider);
    }
    has(provider) {
        return this.#providers.has(provider);
    }
    listProviders() {
        return [...this.#providers.keys()];
    }
    /**
     * Fetch provider-local model identifiers from a registered provider.
     * Providers are looked up by their configured id, so this works for both
     * built-in and dynamically registered OpenAI-compatible providers.
     */
    async listModels(provider) {
        const adapter = this.#providers.get(provider);
        if (!adapter)
            throw new UnknownProviderError(provider);
        if (!adapter.listModels) {
            throw new ProviderError(`Provider ${provider} does not support model discovery`, { provider });
        }
        return adapter.listModels();
    }
    async fetchAvailableModels(provider) {
        return this.listModels(provider);
    }
    async complete(request) {
        const parsed = parseModelId(request.model);
        const adapter = this.#providers.get(parsed.provider);
        if (!adapter)
            throw new UnknownProviderError(parsed.provider);
        const result = await adapter.complete({ ...request, model: parsed.model });
        return { ...result, provider: parsed.provider, model: request.model };
    }
    async *stream(request) {
        const parsed = parseModelId(request.model);
        const adapter = this.#providers.get(parsed.provider);
        if (!adapter)
            throw new UnknownProviderError(parsed.provider);
        for await (const event of adapter.stream({ ...request, model: parsed.model })) {
            if (event.type === "start") {
                yield { ...event, provider: parsed.provider, model: request.model };
            }
            else {
                yield event;
            }
        }
    }
}
//# sourceMappingURL=core.js.map