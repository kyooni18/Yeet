import { OpenAIChatProvider } from "./openai-chat.js";
export class OpenRouterProvider extends OpenAIChatProvider {
    constructor(options = {}) {
        const headers = {};
        if (options.appUrl)
            headers["HTTP-Referer"] = options.appUrl;
        if (options.appName)
            headers["X-Title"] = options.appName;
        super({
            id: "openrouter",
            baseUrl: "https://openrouter.ai/api/v1",
            headers,
            requireApiKey: true,
            ...(options.apiKey ? { apiKey: options.apiKey } : {}),
            ...(options.fetch ? { fetch: options.fetch } : {}),
        });
    }
}
//# sourceMappingURL=openrouter.js.map