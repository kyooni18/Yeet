import type { FetchLike } from "./types.js";
export type AuthMethod = "none" | "api-key" | "browser" | "environment";
export interface AuthStatus {
    provider: string;
    authenticated: boolean;
    method: AuthMethod;
    expiresAt?: string;
    configDir: string;
}
export interface OAuthClientConfiguration {
    clientId: string;
    clientSecret?: string;
    projectId?: string;
    scopes?: string[];
}
export interface BrowserLoginOptions extends Partial<OAuthClientConfiguration> {
    timeoutMs?: number;
}
export type ResolvedCredential = {
    kind: "api-key";
    value: string;
    source: "stored" | "environment" | "browser";
} | {
    kind: "oauth";
    accessToken: string;
    source: "browser";
    expiresAt?: string;
    projectId?: string;
};
export interface AuthManagerOptions {
    configDir?: string;
    fetch?: FetchLike;
    openBrowser?: (url: string) => Promise<void> | void;
}
export declare class AuthManager {
    #private;
    readonly configDir: string;
    readonly configPath: string;
    readonly credentialsPath: string;
    constructor(options?: AuthManagerOptions);
    ensure(): Promise<void>;
    setApiKey(provider: string, apiKey: string): Promise<AuthStatus>;
    configureOAuthClient(provider: string, configuration: OAuthClientConfiguration): Promise<void>;
    logout(provider: string): Promise<AuthStatus>;
    status(provider: string): Promise<AuthStatus>;
    resolve(provider: string): Promise<ResolvedCredential | undefined>;
    loginInBrowser(provider: string, options?: BrowserLoginOptions): Promise<AuthStatus>;
    ensureBaseDirectory(): Promise<void>;
}
//# sourceMappingURL=auth.d.ts.map