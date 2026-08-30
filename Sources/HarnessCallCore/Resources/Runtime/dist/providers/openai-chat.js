import { providerFetch, readJson } from "../http.js";
import { parseSSE } from "../sse.js";
import { normalizeFinishReason, normalizeModelIds, normalizeToolCall, safeJsonParse, splitSystem, usage } from "../util.js";
function mapToolChoice(choice) {
    if (!choice)
        return undefined;
    if (typeof choice === "string") {
        if (choice === "required")
            return "required";
        return choice;
    }
    return { type: "function", function: { name: choice.name } };
}
function mapTools(tools) {
    return tools?.map((tool) => ({
        type: "function",
        function: {
            name: tool.name,
            ...(tool.description ? { description: tool.description } : {}),
            parameters: tool.inputSchema,
        },
    }));
}
function mapMessages(messages, explicitSystem) {
    const { system, messages: rest } = splitSystem(messages, explicitSystem);
    const output = [];
    if (system)
        output.push({ role: "system", content: system });
    for (const message of rest) {
        if (message.role === "tool") {
            output.push({
                role: "tool",
                content: message.content ?? "",
                tool_call_id: message.toolCallId ?? "",
                ...(message.name ? { name: message.name } : {}),
            });
            continue;
        }
        if (message.role === "assistant" && message.toolCalls?.length) {
            output.push({
                role: "assistant",
                content: message.content ?? null,
                tool_calls: message.toolCalls.map((tool) => ({
                    id: tool.id,
                    type: "function",
                    function: {
                        name: tool.name,
                        arguments: typeof tool.arguments === "string" ? tool.arguments : JSON.stringify(tool.arguments),
                    },
                })),
            });
            continue;
        }
        output.push({ role: message.role, content: message.content ?? "" });
    }
    return output;
}
function requestBody(request, stream) {
    const tools = mapTools(request.tools);
    const toolChoice = mapToolChoice(request.toolChoice);
    return {
        ...(request.providerOptions ?? {}),
        model: request.model,
        messages: mapMessages(request.messages, request.system),
        stream,
        ...(stream ? { stream_options: { include_usage: true } } : {}),
        ...(request.temperature !== undefined ? { temperature: request.temperature } : {}),
        ...(request.maxTokens !== undefined ? { max_tokens: request.maxTokens } : {}),
        ...(request.metadata ? { metadata: request.metadata } : {}),
        ...(tools?.length ? { tools } : {}),
        ...(toolChoice !== undefined ? { tool_choice: toolChoice } : {}),
    };
}
export class OpenAIChatProvider {
    id;
    #apiKey;
    #baseUrl;
    #headers;
    #requireApiKey;
    #fetch;
    constructor(options = {}) {
        this.id = options.id ?? "openai-compatible";
        this.#apiKey = options.apiKey;
        this.#baseUrl = (options.baseUrl ?? "https://api.openai.com/v1").replace(/\/$/, "");
        this.#headers = options.headers ?? {};
        this.#requireApiKey = options.requireApiKey ?? false;
        this.#fetch = options.fetch;
    }
    #requestHeaders() {
        if (this.#requireApiKey && !this.#apiKey)
            throw new Error(`Missing API key for ${this.id}`);
        return {
            ...(this.#apiKey ? { authorization: `Bearer ${this.#apiKey}` } : {}),
            "content-type": "application/json",
            ...this.#headers,
        };
    }
    async listModels() {
        const response = await providerFetch(`${this.#baseUrl}/models`, { method: "GET", headers: this.#requestHeaders() }, {
            provider: this.id,
            ...(this.#fetch ? { fetch: this.#fetch } : {}),
        });
        return normalizeModelIds(await readJson(response));
    }
    async complete(request) {
        const response = await providerFetch(`${this.#baseUrl}/chat/completions`, {
            method: "POST",
            headers: this.#requestHeaders(),
            body: JSON.stringify(requestBody(request, false)),
        }, {
            provider: this.id,
            ...(this.#fetch ? { fetch: this.#fetch } : {}),
            ...(request.timeoutMs !== undefined ? { timeoutMs: request.timeoutMs } : {}),
            ...(request.retry ? { retry: request.retry } : {}),
            ...(request.signal ? { signal: request.signal } : {}),
        });
        const raw = await readJson(response);
        const choice = raw.choices?.[0];
        const message = choice?.message ?? {};
        const toolCalls = (message.tool_calls ?? []).map((tool, index) => normalizeToolCall(tool.id, tool.function?.name, safeJsonParse(tool.function?.arguments ?? ""), index));
        const normalizedUsage = usage(raw.usage?.prompt_tokens, raw.usage?.completion_tokens, raw.usage?.total_tokens);
        return {
            provider: this.id,
            model: raw.model ?? request.model,
            ...(raw.id ? { id: raw.id } : {}),
            text: typeof message.content === "string" ? message.content : "",
            toolCalls,
            finishReason: normalizeFinishReason(choice?.finish_reason),
            ...(normalizedUsage ? { usage: normalizedUsage } : {}),
            raw,
        };
    }
    async *stream(request) {
        const response = await providerFetch(`${this.#baseUrl}/chat/completions`, {
            method: "POST",
            headers: this.#requestHeaders(),
            body: JSON.stringify(requestBody(request, true)),
        }, {
            provider: this.id,
            ...(this.#fetch ? { fetch: this.#fetch } : {}),
            ...(request.timeoutMs !== undefined ? { timeoutMs: request.timeoutMs } : {}),
            ...(request.retry ? { retry: request.retry } : {}),
            ...(request.signal ? { signal: request.signal } : {}),
        });
        let started = false;
        let finishReason = normalizeFinishReason(undefined);
        let finalUsage;
        const tools = new Map();
        for await (const message of parseSSE(response)) {
            if (message.data === "[DONE]")
                break;
            let raw;
            try {
                raw = JSON.parse(message.data);
            }
            catch {
                continue;
            }
            if (!started) {
                started = true;
                yield {
                    type: "start",
                    provider: this.id,
                    model: raw.model ?? request.model,
                    ...(raw.id ? { id: raw.id } : {}),
                };
            }
            if (raw.usage) {
                finalUsage = usage(raw.usage.prompt_tokens, raw.usage.completion_tokens, raw.usage.total_tokens);
            }
            const choice = raw.choices?.[0];
            if (!choice)
                continue;
            if (choice.finish_reason != null)
                finishReason = normalizeFinishReason(choice.finish_reason);
            const delta = choice.delta ?? {};
            if (typeof delta.content === "string" && delta.content) {
                yield { type: "text-delta", delta: delta.content };
            }
            for (const toolDelta of delta.tool_calls ?? []) {
                const index = toolDelta.index ?? 0;
                const current = tools.get(index) ?? { argumentsText: "" };
                if (toolDelta.id)
                    current.id = toolDelta.id;
                if (toolDelta.function?.name)
                    current.name = toolDelta.function.name;
                if (toolDelta.function?.arguments)
                    current.argumentsText += toolDelta.function.arguments;
                tools.set(index, current);
                yield {
                    type: "tool-call-delta",
                    index,
                    ...(toolDelta.id ? { id: toolDelta.id } : {}),
                    ...(toolDelta.function?.name ? { name: toolDelta.function.name } : {}),
                    ...(toolDelta.function?.arguments ? { argumentsDelta: toolDelta.function.arguments } : {}),
                };
            }
        }
        for (const [index, tool] of [...tools.entries()].sort(([a], [b]) => a - b)) {
            yield {
                type: "tool-call",
                index,
                toolCall: normalizeToolCall(tool.id, tool.name, safeJsonParse(tool.argumentsText), index),
            };
        }
        yield {
            type: "finish",
            finishReason,
            ...(finalUsage ? { usage: finalUsage } : {}),
        };
    }
}
//# sourceMappingURL=openai-chat.js.map