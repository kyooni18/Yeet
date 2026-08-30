export function splitSystem(messages, explicit) {
    const systemParts = [];
    if (explicit)
        systemParts.push(explicit);
    const rest = [];
    for (const message of messages) {
        if (message.role === "system") {
            if (message.content)
                systemParts.push(message.content);
        }
        else {
            rest.push(message);
        }
    }
    return {
        ...(systemParts.length > 0 ? { system: systemParts.join("\n\n") } : {}),
        messages: rest,
    };
}
export function safeJsonParse(value) {
    if (!value)
        return {};
    try {
        return JSON.parse(value);
    }
    catch {
        return value;
    }
}
export function normalizeFinishReason(value) {
    if (typeof value !== "string")
        return "unknown";
    switch (value.toLowerCase()) {
        case "stop":
        case "end_turn":
        case "stop_sequence":
            return "stop";
        case "length":
        case "max_tokens":
        case "max_output_tokens":
            return "length";
        case "tool_calls":
        case "tool_use":
            return "tool_call";
        case "content_filter":
        case "safety":
        case "recitation":
            return "content_filter";
        default:
            return "unknown";
    }
}
export function usage(input, output, total, cached) {
    if ([input, output, total, cached].every((value) => value === undefined))
        return undefined;
    return {
        ...(input !== undefined ? { inputTokens: input } : {}),
        ...(output !== undefined ? { outputTokens: output } : {}),
        ...(total !== undefined ? { totalTokens: total } : {}),
        ...(cached !== undefined ? { cachedInputTokens: cached } : {}),
    };
}
export function normalizeToolCall(id, name, args, index) {
    return {
        id: id ?? `tool-${index}`,
        name: name ?? "unknown",
        arguments: args,
    };
}
/**
 * Normalize the model-list response shapes used by OpenAI-compatible APIs and
 * Gemini. Provider adapters return provider-local names, so Gemini's
 * `models/` resource prefix is removed here.
 */
export function normalizeModelIds(raw) {
    const value = raw && typeof raw === "object" && !Array.isArray(raw)
        ? raw
        : undefined;
    const entries = Array.isArray(raw)
        ? raw
        : Array.isArray(value?.data)
            ? value.data
            : Array.isArray(value?.models)
                ? value.models
                : [];
    const result = [];
    const seen = new Set();
    const add = (rawId) => {
        const trimmed = rawId.trim();
        const id = trimmed.startsWith("models/") ? trimmed.slice("models/".length) : trimmed;
        if (id && !seen.has(id)) {
            seen.add(id);
            result.push(id);
        }
    };
    for (const entry of entries) {
        if (typeof entry === "string") {
            add(entry);
            continue;
        }
        if (!entry || typeof entry !== "object" || Array.isArray(entry))
            continue;
        const candidate = entry;
        const rawId = typeof candidate.id === "string"
            ? candidate.id
            : typeof candidate.name === "string"
                ? candidate.name
                : undefined;
        if (!rawId)
            continue;
        add(rawId);
    }
    return result;
}
//# sourceMappingURL=util.js.map