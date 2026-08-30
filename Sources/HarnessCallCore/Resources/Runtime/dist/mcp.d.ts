import type { FetchLike } from "./types.js";
export declare const MCP_PROTOCOL_VERSION = "2026-07-28";
export type McpTransportKind = "stdio" | "http";
export interface McpStdioServerConfiguration {
    name: string;
    transport: "stdio";
    command: string;
    args?: string[];
    env?: Record<string, string>;
    cwd?: string;
}
export interface McpHTTPServerConfiguration {
    name: string;
    transport: "http";
    url: string;
    headers?: Record<string, string>;
}
export type McpServerConfiguration = McpStdioServerConfiguration | McpHTTPServerConfiguration;
export type McpServerStatus = McpServerConfiguration & {
    connected: boolean;
    protocol?: string;
    era?: "modern" | "legacy";
};
export interface McpTool {
    server: string;
    name: string;
    qualifiedName: string;
    title?: string;
    description?: string;
    inputSchema: Record<string, unknown>;
    outputSchema?: Record<string, unknown>;
    annotations?: Record<string, unknown>;
}
export interface McpResource {
    server: string;
    uri: string;
    name: string;
    title?: string;
    description?: string;
    mimeType?: string;
}
export interface McpPromptArgument {
    name: string;
    description?: string;
    required?: boolean;
}
export interface McpPrompt {
    server: string;
    name: string;
    qualifiedName: string;
    title?: string;
    description?: string;
    arguments?: McpPromptArgument[];
}
export interface McpCallToolResult {
    content: unknown[];
    structuredContent?: unknown;
    isError?: boolean;
    _meta?: Record<string, unknown>;
}
export interface McpReadResourceResult {
    contents: unknown[];
    _meta?: Record<string, unknown>;
}
export interface McpGetPromptResult {
    description?: string;
    messages: unknown[];
    _meta?: Record<string, unknown>;
}
export declare class McpError extends Error {
    readonly code: number | undefined;
    readonly data: unknown;
    readonly server: string | undefined;
    constructor(message: string, options?: {
        code?: number;
        data?: unknown;
        server?: string;
    });
}
export interface McpManagerOptions {
    configDir?: string;
    fetch?: FetchLike;
}
export declare class McpManager {
    #private;
    readonly configDir: string;
    readonly configPath: string;
    constructor(options?: McpManagerOptions);
    ensure(): Promise<void>;
    setServer(configuration: McpServerConfiguration): Promise<McpServerConfiguration>;
    removeServer(name: string): Promise<boolean>;
    listServers(): Promise<McpServerStatus[]>;
    listTools(server?: string): Promise<McpTool[]>;
    callTool(server: string, name: string, args?: Record<string, unknown>): Promise<McpCallToolResult>;
    callQualifiedTool(qualifiedName: string, args?: Record<string, unknown>): Promise<McpCallToolResult>;
    listResources(server?: string): Promise<McpResource[]>;
    readResource(server: string, uri: string): Promise<McpReadResourceResult>;
    listPrompts(server?: string): Promise<McpPrompt[]>;
    getPrompt(server: string, name: string, args?: Record<string, string>): Promise<McpGetPromptResult>;
    disconnect(name: string): Promise<void>;
    close(): Promise<void>;
}
//# sourceMappingURL=mcp.d.ts.map