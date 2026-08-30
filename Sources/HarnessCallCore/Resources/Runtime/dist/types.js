export function parseModelId(value) {
    const slash = value.indexOf("/");
    if (slash <= 0 || slash === value.length - 1) {
        throw new TypeError(`Model must use provider/model form, got: ${value}`);
    }
    const provider = value.slice(0, slash).trim();
    const model = value.slice(slash + 1).trim();
    if (!provider || !model)
        throw new TypeError(`Model must use provider/model form, got: ${value}`);
    return { provider, model };
}
export function modelId(provider, model) {
    if (!provider.trim() || provider.includes("/")) {
        throw new TypeError(`Provider id must be a non-empty single path segment, got: ${provider}`);
    }
    if (!model.trim())
        throw new TypeError("Model name must not be empty");
    return `${provider}/${model}`;
}
//# sourceMappingURL=types.js.map