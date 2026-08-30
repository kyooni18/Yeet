import type { AuthStatus, BrowserLoginOptions } from "./auth.js";
import type {
  McpCallToolResult,
  McpGetPromptResult,
  McpPrompt,
  McpReadResourceResult,
  McpResource,
  McpServerConfiguration,
  McpServerStatus,
  McpTool,
} from "./mcp.js";
import type { Skill, SkillSummary } from "./skills.js";
import type { CallRequest, CallResult, StreamEvent } from "./types.js";

export const BRIDGE_PROTOCOL_VERSION = 1 as const;

export type BridgeRequest = Omit<CallRequest, "signal">;

export interface OpenAICompatibleProviderConfig {
  kind: "openai-compatible";
  id: string;
  baseUrl: string;
  apiKey?: string;
  headers?: Record<string, string>;
  requireApiKey?: boolean;
}

export type BridgeCommand =
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "ping" }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "list-providers" }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "list-models"; provider: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "config-path" }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "auth-status"; provider: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "auth-set-api-key"; provider: string; apiKey: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "auth-login-browser"; provider: string; options?: BrowserLoginOptions }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "auth-logout"; provider: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "register-provider"; provider: OpenAICompatibleProviderConfig }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "unregister-provider"; provider: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "complete"; request: BridgeRequest }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "stream"; request: BridgeRequest }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "skill-list" }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "skill-load"; skill: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "skill-read"; skill: string; path: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "mcp-list-servers" }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "mcp-set-server"; server: McpServerConfiguration }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "mcp-remove-server"; server: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "mcp-list-tools"; server?: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "mcp-call-tool"; server: string; tool: string; arguments?: Record<string, unknown> }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "mcp-list-resources"; server?: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "mcp-read-resource"; server: string; uri: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "mcp-list-prompts"; server?: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "mcp-get-prompt"; server: string; prompt: string; arguments?: Record<string, string> }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "mcp-disconnect"; server?: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "cancel"; target: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; op: "shutdown" };

export interface SerializedBridgeError {
  name: string;
  message: string;
  stack?: string;
  provider?: string;
  retryable?: boolean;
  status?: number;
  responseBody?: string;
  requestId?: string;
  code?: number;
  server?: string;
}

export type BridgeMessage =
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "pong" }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "providers"; providers: string[] }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "models"; provider: string; models: string[] }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "config-path"; path: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "auth-status"; status: AuthStatus }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "registered"; provider: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "unregistered"; provider: string; removed: boolean }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "result"; result: CallResult }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "event"; event: StreamEvent }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "skills"; skills: SkillSummary[] }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "skill"; skill: Skill }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "skill-file"; content: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "mcp-servers"; servers: McpServerStatus[] }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "mcp-server"; server: McpServerConfiguration }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "mcp-removed"; removed: boolean }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "mcp-tools"; tools: McpTool[] }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "mcp-tool-result"; toolResult: McpCallToolResult }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "mcp-resources"; resources: McpResource[] }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "mcp-resource-result"; resourceResult: McpReadResourceResult }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "mcp-prompts"; prompts: McpPrompt[] }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "mcp-prompt-result"; promptResult: McpGetPromptResult }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "done" }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "cancelled"; target: string }
  | { v: typeof BRIDGE_PROTOCOL_VERSION; id: string; type: "error"; error: SerializedBridgeError };
