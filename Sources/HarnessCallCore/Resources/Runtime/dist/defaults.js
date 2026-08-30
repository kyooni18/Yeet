import { CallCore } from "./core.js";
import { AnthropicProvider } from "./providers/anthropic.js";
import { GeminiProvider } from "./providers/gemini.js";
import { OpenAIProvider } from "./providers/openai.js";
import { OpenRouterProvider } from "./providers/openrouter.js";
function env(name) {
    const processLike = globalThis.process;
    return processLike?.env?.[name];
}
export function createDefaultCore(options = {}) {
    const fetchOption = options.fetch ? { fetch: options.fetch } : {};
    const openaiApiKey = options.openaiApiKey ?? env("OPENAI_API_KEY");
    const anthropicApiKey = options.anthropicApiKey ?? env("ANTHROPIC_API_KEY");
    const geminiApiKey = options.geminiApiKey ?? env("GEMINI_API_KEY");
    const openrouterApiKey = options.openrouterApiKey ?? env("OPENROUTER_API_KEY");
    const geminiAccessToken = options.geminiAccessToken;
    return new CallCore([
        new OpenAIProvider({ ...(openaiApiKey ? { apiKey: openaiApiKey } : {}), ...fetchOption }),
        new AnthropicProvider({ ...(anthropicApiKey ? { apiKey: anthropicApiKey } : {}), ...fetchOption }),
        new GeminiProvider({
            ...(geminiApiKey ? { apiKey: geminiApiKey } : {}),
            ...(geminiAccessToken ? { accessToken: geminiAccessToken } : {}),
            ...(options.geminiProjectId ? { projectId: options.geminiProjectId } : {}),
            ...fetchOption,
        }),
        new OpenRouterProvider({
            ...(openrouterApiKey ? { apiKey: openrouterApiKey } : {}),
            ...(options.openrouterAppUrl ? { appUrl: options.openrouterAppUrl } : {}),
            ...(options.openrouterAppName ? { appName: options.openrouterAppName } : {}),
            ...fetchOption,
        }),
    ]);
}
//# sourceMappingURL=defaults.js.map