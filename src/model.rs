use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::core::Usage;

pub const REASONING_LEVELS: &[&str] = &["auto", "low", "medium", "high"];

pub fn normalize_reasoning_level(value: &str) -> Option<&'static str> {
    let normalized = value.trim().to_ascii_lowercase();
    REASONING_LEVELS
        .iter()
        .copied()
        .find(|level| *level == normalized)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BridgeState {
    #[serde(default)]
    pub debate: Option<crate::debate::DebateState>,
    pub conversation_revision: u64,
    pub conversation: Option<Vec<ConversationEntry>>,
    pub active_assistant_entry_id: Option<String>,
    pub active_assistant_text: String,
    pub active_activity_entry_id: Option<String>,
    pub active_reasoning_entry_id: Option<String>,
    pub active_reasoning_text: String,
    pub active_reasoning_summary: String,
    pub is_streaming: bool,
    pub error_message: Option<String>,
    pub active_model: String,
    pub active_reasoning_level: String,
    pub active_model_context_length: Option<u64>,
    pub token_usage: Usage,
    pub credit_usage: u64,
    pub pending_shell_permission: Option<ShellPermission>,
    pub pending_native_app_permission: Option<NativeAppPermission>,
    pub available_models: Vec<String>,
    pub is_loading_models: bool,
    pub saved_sessions: Vec<SessionSummary>,
    pub current_session_id: Option<String>,
    pub active_run_id: Option<String>,
    pub available_capabilities: Vec<CapabilityToggleItem>,
    pub is_loading_capabilities: bool,
    pub auth_providers: Vec<AuthProviderItem>,
    pub auth_notice: Option<String>,
    pub auth_working: bool,
    pub provider_configurations: Vec<ProviderConfigurationItem>,
    pub providers_notice: Option<String>,
    pub providers_working: bool,
    pub openai_flex: bool,
    pub foundation_memory_enabled: bool,
    pub foundation_memory_server: String,
    pub foundation_memory_connected: bool,
    pub settings_notice: Option<String>,
    pub settings_working: bool,
    pub sandbox_settings: Option<SandboxSettingsState>,
    pub sandbox_notice: Option<String>,
    pub sandbox_working: bool,
}

impl BridgeState {
    pub fn without_conversation(&self) -> Self {
        Self {
            debate: self.debate.clone(),
            conversation_revision: self.conversation_revision,
            conversation: None,
            active_assistant_entry_id: self.active_assistant_entry_id.clone(),
            active_assistant_text: self.active_assistant_text.clone(),
            active_activity_entry_id: self.active_activity_entry_id.clone(),
            active_reasoning_entry_id: self.active_reasoning_entry_id.clone(),
            active_reasoning_text: self.active_reasoning_text.clone(),
            active_reasoning_summary: self.active_reasoning_summary.clone(),
            is_streaming: self.is_streaming,
            error_message: self.error_message.clone(),
            active_model: self.active_model.clone(),
            active_reasoning_level: self.active_reasoning_level.clone(),
            active_model_context_length: self.active_model_context_length,
            token_usage: self.token_usage.clone(),
            credit_usage: self.credit_usage,
            pending_shell_permission: self.pending_shell_permission.clone(),
            pending_native_app_permission: self.pending_native_app_permission.clone(),
            available_models: self.available_models.clone(),
            is_loading_models: self.is_loading_models,
            saved_sessions: self.saved_sessions.clone(),
            current_session_id: self.current_session_id.clone(),
            active_run_id: self.active_run_id.clone(),
            available_capabilities: self.available_capabilities.clone(),
            is_loading_capabilities: self.is_loading_capabilities,
            auth_providers: self.auth_providers.clone(),
            auth_notice: self.auth_notice.clone(),
            auth_working: self.auth_working,
            provider_configurations: self.provider_configurations.clone(),
            providers_notice: self.providers_notice.clone(),
            providers_working: self.providers_working,
            openai_flex: self.openai_flex,
            foundation_memory_enabled: self.foundation_memory_enabled,
            foundation_memory_server: self.foundation_memory_server.clone(),
            foundation_memory_connected: self.foundation_memory_connected,
            settings_notice: self.settings_notice.clone(),
            settings_working: self.settings_working,
            sandbox_settings: self.sandbox_settings.clone(),
            sandbox_notice: self.sandbox_notice.clone(),
            sandbox_working: self.sandbox_working,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthProviderItem {
    pub provider: String,
    pub authenticated: bool,
    pub method: String,
    pub expires_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderConfigurationItem {
    pub id: String,
    pub base_url: String,
    pub require_api_key: bool,
    pub header_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxSettingsState {
    pub preset: String,
    pub execution_mode: String,
    pub auto_approve: bool,
    pub workspace_mode: String,
    pub workspace_paths: Vec<String>,
    pub scratch_writable: bool,
    pub network_allow: Vec<SandboxNetworkItem>,
    pub environment: Vec<SandboxEnvironmentItem>,
    pub secret_ids: Vec<String>,
    pub limits: SandboxLimitsState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxNetworkItem {
    pub host: String,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxEnvironmentItem {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxLimitsState {
    pub wall_time_seconds: u64,
    pub max_stdout_bytes: usize,
    pub max_stderr_bytes: usize,
    pub max_memory_bytes: u64,
    pub max_processes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityToggleItem {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellPermission {
    pub id: String,
    pub kind: String,
    pub command: String,
    pub operation: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NativeAppPermission {
    pub id: String,
    pub server: String,
    pub tool: String,
    pub bundle_id: Option<String>,
    pub app_name: Option<String>,
    pub operation: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub model: String,
    pub message_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationEntry {
    pub id: String,
    pub kind: ConversationKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ConversationKind {
    #[serde(rename = "user")]
    User { content: String },
    #[serde(rename = "assistant")]
    Assistant {
        content: String,
        #[serde(default, rename = "toolCalls")]
        tool_calls: Vec<ConversationToolCall>,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        content: String,
        summary: Option<String>,
    },
    #[serde(rename = "activity")]
    Activity { activity: ModelActivity },
    #[serde(rename = "toolCall")]
    ToolCall {
        #[serde(rename = "toolCall")]
        tool_call: ConversationToolCall,
    },
    #[serde(rename = "skill")]
    Skill {
        name: String,
        content: String,
        status: Option<String>,
    },
    #[serde(rename = "mcp")]
    Mcp {
        server: String,
        name: String,
        content: String,
        #[serde(rename = "isError")]
        is_error: bool,
    },
    #[serde(rename = "system")]
    System { content: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationToolCall {
    pub id: String,
    pub index: Option<i64>,
    #[serde(rename = "callID")]
    pub call_id: Option<String>,
    pub name: String,
    pub arguments: String,
    pub status: ToolCallStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolCallStatus {
    Streaming,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelActivity {
    pub phase: Value,
    pub title: String,
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BridgeEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
    pub state: Option<BridgeState>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FrontendCommand {
    StartDebate {
        topic: String,
        #[serde(default)]
        models: Option<crate::debate::DebateModels>,
    },
    Submit {
        text: String,
    },
    Interrupt,
    AllowShell,
    DenyShell,
    AllowNativeApp,
    DenyNativeApp,
    RequestModels,
    SelectModel {
        model: String,
    },
    SelectReasoning {
        level: String,
    },
    RequestSessions,
    LoadSession {
        session_id: String,
    },
    NewSession,
    RequestCapabilities,
    ToggleCapability {
        id: String,
    },
    RequestAuth,
    AuthLogin {
        provider: String,
    },
    AuthLogout {
        provider: String,
    },
    AuthSetApiKey {
        provider: String,
        key: String,
    },
    RequestProviders,
    SaveProvider {
        id: String,
        base_url: String,
        require_api_key: bool,
    },
    RemoveProvider {
        id: String,
    },
    RequestSettings,
    SetOpenAiFlex {
        enabled: bool,
    },
    SetFoundationMemory {
        enabled: bool,
    },
    RequestSandbox,
    UpdateSandbox {
        action: SandboxAction,
    },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SandboxAction {
    ApplyPreset { preset: String },
    SetExecutionMode { mode: String },
    SetAutoApprove { enabled: bool },
    SetWorkspaceMode { mode: String },
    AddWorkspacePath { path: String },
    RemoveWorkspacePath { path: String },
    SetScratchWritable { enabled: bool },
    AddNetwork { host: String, port: Option<u16> },
    RemoveNetwork { host: String, port: Option<u16> },
    SetEnvironment { key: String, value: String },
    RemoveEnvironment { key: String },
    AddSecret { id: String },
    RemoveSecret { id: String },
    SetLimit { name: String, value: u64 },
    Reset,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_decodes_legacy_swift_camel_case_fields() {
        let value = serde_json::json!({
            "id": "entry",
            "kind": {
                "type": "assistant",
                "content": "done",
                "toolCalls": [{
                    "id": "visible",
                    "index": 0,
                    "callID": "call-1",
                    "name": "read_file",
                    "arguments": "{\"path\":\"Cargo.toml\"}",
                    "status": "completed"
                }]
            }
        });
        let entry: ConversationEntry = serde_json::from_value(value).unwrap();
        match entry.kind {
            ConversationKind::Assistant { tool_calls, .. } => {
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].call_id.as_deref(), Some("call-1"));
            }
            _ => panic!("wrong conversation kind"),
        }
    }

    #[test]
    fn mcp_conversation_uses_is_error_compatibility_key() {
        let entry = ConversationEntry {
            id: "entry".into(),
            kind: ConversationKind::Mcp {
                server: "local".into(),
                name: "read".into(),
                content: "ok".into(),
                is_error: false,
            },
        };
        let value = serde_json::to_value(entry).unwrap();
        assert_eq!(value["kind"]["isError"], false);
        assert!(value["kind"].get("is_error").is_none());
    }

    #[test]
    fn compact_bridge_state_keeps_live_stream_fields_without_conversation() {
        let state = BridgeState {
            conversation_revision: 7,
            conversation: Some(vec![ConversationEntry {
                id: "assistant".into(),
                kind: ConversationKind::Assistant {
                    content: "old".into(),
                    tool_calls: vec![],
                },
            }]),
            active_assistant_entry_id: Some("assistant".into()),
            active_assistant_text: "streaming".into(),
            active_reasoning_entry_id: Some("reasoning".into()),
            active_reasoning_text: "working".into(),
            active_reasoning_summary: "summary".into(),
            current_session_id: Some("session".into()),
            ..BridgeState::default()
        };
        let compact = state.without_conversation();
        assert!(compact.conversation.is_none());
        assert_eq!(compact.conversation_revision, 7);
        assert_eq!(compact.active_assistant_text, "streaming");
        assert_eq!(compact.active_reasoning_text, "working");
        assert_eq!(compact.active_reasoning_summary, "summary");
        assert_eq!(compact.current_session_id.as_deref(), Some("session"));
    }
}
