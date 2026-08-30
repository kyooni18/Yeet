import { CallCore } from "./core.js";
import type { FetchLike } from "./types.js";
export interface DefaultCoreOptions {
    openaiApiKey?: string;
    anthropicApiKey?: string;
    geminiApiKey?: string;
    geminiAccessToken?: string;
    geminiProjectId?: string;
    openrouterApiKey?: string;
    openrouterAppUrl?: string;
    openrouterAppName?: string;
    fetch?: FetchLike;
}
export declare function createDefaultCore(options?: DefaultCoreOptions): CallCore;
//# sourceMappingURL=defaults.d.ts.map