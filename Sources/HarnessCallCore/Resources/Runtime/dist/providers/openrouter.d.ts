import type { FetchLike } from "../types.js";
import { OpenAIChatProvider } from "./openai-chat.js";
export interface OpenRouterProviderOptions {
    apiKey?: string;
    appUrl?: string;
    appName?: string;
    fetch?: FetchLike;
}
export declare class OpenRouterProvider extends OpenAIChatProvider {
    constructor(options?: OpenRouterProviderOptions);
}
//# sourceMappingURL=openrouter.d.ts.map