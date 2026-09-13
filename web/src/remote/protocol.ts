export const REMOTE_PROTOCOL_VERSION = 1 as const
export const REMOTE_PROTOCOL_MIN_VERSION = 1 as const
export const REMOTE_PROTOCOL_MAX_VERSION = 1 as const

export interface RemoteProtocolInfo {
  version: number
  minVersion: number
  maxVersion: number
  websocket: string
}

export type ConnectionStatus =
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'offline'
  | 'auth-required'
  | 'failed'

export interface Usage {
  input_tokens?: number
  output_tokens?: number
  reasoning_tokens?: number
  cached_input_tokens?: number
  cache_creation_input_tokens?: number
  model_calls?: number
  [key: string]: unknown
}

export interface ProviderUsageWindow {
  id: string
  label: string
  usedPercent: number
  remainingPercent: number
  resetsAt?: string | null
}

export interface ProviderUsageStatus {
  provider: string
  available: boolean
  source: string
  fetchedAt: string
  plan?: string | null
  windows: ProviderUsageWindow[]
  message?: string | null
}

export interface AuthProviderItem {
  provider: string
  authenticated: boolean
  method: string
  expires_at?: string | null
  usage?: ProviderUsageStatus | null
  error?: string | null
}

export interface ProviderConfigurationItem {
  id: string
  base_url: string
  require_api_key: boolean
  header_count: number
}

export interface ModelCatalogItem {
  id: string
  provider: string
  model: string
  context_length?: number | null
}

export interface CapabilityToggleItem {
  id: string
  kind: string
  name: string
  description: string
  enabled: boolean
}

export interface ShellPermission {
  id: string
  kind: string
  command: string
  operation: string
  reason: string
}

export interface NativeAppPermission {
  id: string
  server: string
  tool: string
  bundleId?: string | null
  appName?: string | null
  operation: string
  reason: string
}

export interface SessionSummary {
  id: string
  title: string
  updated_at: string
  model: string
  message_count: number
}

export interface WorkspaceSummary {
  id: string
  path: string
  display_name: string
  updated_at?: string | null
  session_count: number
  is_current: boolean
}

export interface WorkspaceSessionGroup {
  workspace_id: string
  sessions: SessionSummary[]
}

export type ToolCallStatus = 'streaming' | 'completed' | 'failed' | 'suppressed'

export interface ConversationToolCall {
  id: string
  index?: number | null
  callID?: string | null
  name: string
  arguments: string
  status: ToolCallStatus
  detail?: string | null
  result?: unknown
  error?: string | null
  durationMs?: number | null
}

export interface ModelActivity {
  phase: unknown
  title: string
  detail?: string | null
  run_id?: string | null
  durationMs?: number | null
}

export type ConversationKind =
  | { type: 'user'; content: string }
  | { type: 'assistant'; content: string; toolCalls?: ConversationToolCall[] }
  | { type: 'reasoning'; content: string; summary?: string | null }
  | { type: 'activity'; activity: ModelActivity }
  | { type: 'toolCall'; toolCall: ConversationToolCall }
  | { type: 'skill'; name: string; content: string; status?: string | null }
  | { type: 'mcp'; server: string; name: string; content: string; isError: boolean }
  | { type: 'system'; content: string }
  | { type: 'error'; content: string }

export interface ConversationEntry {
  id: string
  kind: ConversationKind
  /** Browser-only transient flag; never sent back to the Remote backend. */
  uiStreaming?: boolean
}

export interface SandboxNetworkItem {
  host: string
  port?: number | null
}

export interface SandboxEnvironmentItem {
  key: string
  value: string
}

export interface SandboxLimitsState {
  wall_time_seconds: number
  max_stdout_bytes: number
  max_stderr_bytes: number
  max_memory_bytes: number
  max_processes: number
}

export interface SandboxSettingsState {
  preset: string
  execution_mode: string
  auto_approve: boolean
  workspace_mode: string
  workspace_paths: string[]
  scratch_writable: boolean
  network_allow: SandboxNetworkItem[]
  environment: SandboxEnvironmentItem[]
  secret_ids: string[]
  limits: SandboxLimitsState
}

export interface BridgeState {
  workspace_root?: string | null
  conversation_revision: number
  conversation?: ConversationEntry[] | null
  active_assistant_entry_id?: string | null
  active_assistant_text: string
  active_activity_entry_id?: string | null
  active_reasoning_entry_id?: string | null
  active_reasoning_text: string
  active_reasoning_summary: string
  is_streaming: boolean
  infinity_mode: boolean
  error_message?: string | null
  active_model: string
  active_reasoning_level: string
  active_model_context_length?: number | null
  current_context_tokens?: number | null
  token_usage: Usage
  credit_usage: number
  pending_shell_permission?: ShellPermission | null
  pending_native_app_permission?: NativeAppPermission | null
  available_models: string[]
  model_catalog: ModelCatalogItem[]
  is_loading_models: boolean
  saved_sessions: SessionSummary[]
  known_workspaces: WorkspaceSummary[]
  workspace_session_groups: WorkspaceSessionGroup[]
  current_session_id?: string | null
  active_run_id?: string | null
  available_capabilities: CapabilityToggleItem[]
  is_loading_capabilities: boolean
  auth_providers: AuthProviderItem[]
  auth_notice?: string | null
  auth_working: boolean
  provider_configurations: ProviderConfigurationItem[]
  providers_notice?: string | null
  providers_working: boolean
  openai_flex: boolean
  foundation_memory_enabled: boolean
  foundation_memory_server: string
  foundation_memory_connected: boolean
  settings_notice?: string | null
  settings_working: boolean
  sandbox_settings?: SandboxSettingsState | null
  sandbox_notice?: string | null
  sandbox_working: boolean
  debate?: unknown
}

export const emptyBridgeState = (): BridgeState => ({
  conversation_revision: 0,
  conversation: [],
  active_assistant_text: '',
  active_reasoning_text: '',
  active_reasoning_summary: '',
  is_streaming: false,
  infinity_mode: false,
  active_model: '',
  active_reasoning_level: 'auto',
  token_usage: {},
  credit_usage: 0,
  available_models: [],
  model_catalog: [],
  is_loading_models: false,
  saved_sessions: [],
  known_workspaces: [],
  workspace_session_groups: [],
  available_capabilities: [],
  is_loading_capabilities: false,
  auth_providers: [],
  auth_working: false,
  provider_configurations: [],
  providers_working: false,
  openai_flex: false,
  foundation_memory_enabled: false,
  foundation_memory_server: 'foundation',
  foundation_memory_connected: false,
  settings_working: false,
  sandbox_working: false,
})

export type SandboxAction =
  | { type: 'apply_preset'; preset: string }
  | { type: 'set_execution_mode'; mode: string }
  | { type: 'set_auto_approve'; enabled: boolean }
  | { type: 'set_workspace_mode'; mode: string }
  | { type: 'add_workspace_path'; path: string }
  | { type: 'remove_workspace_path'; path: string }
  | { type: 'set_scratch_writable'; enabled: boolean }
  | { type: 'add_network'; host: string; port?: number | null }
  | { type: 'remove_network'; host: string; port?: number | null }
  | { type: 'set_environment'; key: string; value: string }
  | { type: 'remove_environment'; key: string }
  | { type: 'add_secret'; id: string }
  | { type: 'remove_secret'; id: string }
  | { type: 'set_limit'; name: string; value: number }
  | { type: 'reset' }

export type FrontendCommand =
  | { type: 'submit'; text: string }
  | { type: 'interrupt' }
  | { type: 'allow_shell' }
  | { type: 'deny_shell' }
  | { type: 'allow_native_app' }
  | { type: 'deny_native_app' }
  | { type: 'request_models' }
  | { type: 'select_model'; model: string }
  | { type: 'select_reasoning'; level: string }
  | { type: 'set_infinity'; enabled: boolean }
  | { type: 'request_sessions' }
  | { type: 'load_session'; session_id: string }
  | { type: 'new_session' }
  | { type: 'request_capabilities' }
  | { type: 'toggle_capability'; id: string }
  | { type: 'request_auth' }
  | { type: 'auth_login'; provider: string }
  | { type: 'auth_logout'; provider: string }
  | { type: 'auth_set_api_key'; provider: string; key: string }
  | { type: 'request_providers' }
  | { type: 'save_provider'; id: string; base_url: string; require_api_key: boolean }
  | { type: 'remove_provider'; id: string }
  | { type: 'request_settings' }
  | { type: 'set_open_ai_flex'; enabled: boolean }
  | { type: 'set_foundation_memory'; enabled: boolean }
  | { type: 'request_sandbox' }
  | { type: 'update_sandbox'; action: SandboxAction }
  | { type: 'start_debate'; topic: string; models?: string[] | null }

export interface RemoteAuthStatus {
  required: boolean
  authenticated: boolean
  key: boolean
  passkey: boolean
  enrollmentValid?: boolean
}

export interface RemoteHello {
  type: 'hello'
  min_version: number
  max_version: number
  client_id?: string | null
  workspace?: string | null
  session_id?: string | null
  last_sequence?: number | null
  last_revision?: number | null
}

export type RemoteServerMessage =
  | { version: number; type: 'welcome'; client_id: string; workspace: string; session_id?: string | null; sequence: number; revision: number; resumed: boolean }
  | { version: number; type: 'snapshot'; sequence: number; revision: number; state: BridgeState }
  | { version: number; type: 'state_update'; sequence: number; revision: number; patch: Partial<BridgeState> }
  | { version: number; type: 'assistant_delta'; sequence: number; revision: number; entry_id?: string | null; delta: string; content: string; reset: boolean }
  | { version: number; type: 'reasoning_delta'; sequence: number; revision: number; entry_id?: string | null; delta: string; content: string; summary: boolean; reset: boolean }
  | { version: number; type: 'conversation_entry'; sequence: number; revision: number; entry: ConversationEntry }
  | { version: number; type: 'conversation_reset'; sequence: number; revision: number; conversation: ConversationEntry[] }
  | { version: number; type: 'tool_update'; sequence: number; revision: number; entry: ConversationEntry; tool_call?: ConversationToolCall | null }
  | { version: number; type: 'activity_update'; sequence: number; revision: number; entry: ConversationEntry; activity: ModelActivity }
  | { version: number; type: 'ack'; request_id?: string | null }
  | { version: number; type: 'error'; code: string; message: string; fatal: boolean; request_id?: string | null; supported_min_version: number; supported_max_version: number }
  | { version: number; type: 'pong'; nonce?: string | null }

export interface RemoteResumeState {
  clientId?: string | null
  workspace?: string | null
  sessionId?: string | null
  lastSequence?: number | null
  lastRevision?: number | null
}

export function encodeHello(resume: RemoteResumeState): string {
  return JSON.stringify({
    type: 'hello',
    min_version: REMOTE_PROTOCOL_MIN_VERSION,
    max_version: REMOTE_PROTOCOL_MAX_VERSION,
    client_id: resume.clientId ?? null,
    workspace: resume.workspace ?? null,
    session_id: resume.sessionId ?? null,
    last_sequence: resume.lastSequence ?? null,
    last_revision: resume.lastRevision ?? null,
  })
}

export function encodeCommand(command: FrontendCommand, requestId?: string): string {
  return JSON.stringify({
    type: 'command',
    version: REMOTE_PROTOCOL_VERSION,
    request_id: requestId ?? null,
    command,
  })
}

export function encodePing(nonce?: string): string {
  return JSON.stringify({ type: 'ping', version: REMOTE_PROTOCOL_VERSION, nonce: nonce ?? null })
}

export function decodeServerMessage(raw: string): RemoteServerMessage | null {
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return null
  }
  if (!parsed || typeof parsed !== 'object') return null
  const value = parsed as Record<string, unknown>

  return typeof value.type === 'string' ? value as RemoteServerMessage : null
}
