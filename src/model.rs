use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::core::{ProviderUsageStatus, ProviderUsageWindow, Usage};

pub const REASONING_LEVELS: &[&str] = &["auto", "low", "medium", "high", "xhigh", "max"];

pub fn reasoning_levels_for_model(model: &str) -> &'static [&'static str] {
    const STANDARD: &[&str] = &["auto", "low", "medium", "high"];
    const EXTENDED: &[&str] = &["auto", "low", "medium", "high", "xhigh", "max"];

    let (provider, provider_model) = model.split_once('/').unwrap_or(("", model));
    let routed_model = if matches!(provider, "openrouter" | "opencode" | "opencode-go") {
        provider_model
            .split_once('/')
            .map(|(_, model)| model)
            .unwrap_or(provider_model)
    } else {
        provider_model
    };

    if let Some(version) = routed_model.strip_prefix("gpt-") {
        let numeric = version
            .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
            .next()
            .unwrap_or_default();
        let mut parts = numeric.split('.');
        let major = parts
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let minor = parts
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        if major > 5 || (major == 5 && minor >= 6) {
            return EXTENDED;
        }
    }

    if routed_model.starts_with("claude-fable-5")
        || routed_model.starts_with("claude-mythos-5")
        || routed_model.starts_with("claude-opus-5")
        || routed_model.starts_with("claude-sonnet-5")
        || routed_model.starts_with("claude-opus-4.7")
        || routed_model.starts_with("claude-opus-4-7")
        || routed_model.starts_with("claude-opus-4.8")
        || routed_model.starts_with("claude-opus-4-8")
    {
        return EXTENDED;
    }

    STANDARD
}

pub fn normalize_reasoning_level(value: &str) -> Option<&'static str> {
    let normalized = value.trim().to_ascii_lowercase();
    REASONING_LEVELS
        .iter()
        .copied()
        .find(|level| *level == normalized)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionCommandItem {
    pub extension_id: String,
    pub command: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    #[default]
    Single,
    Adaptive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskItem {
    pub id: String,
    pub role: String,
    pub objective: String,
    pub status: String,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyMode {
    #[default]
    Manual,
    Goal,
    Autonomous,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionActivity {
    Running,
    WaitingForPermission,
}

impl SessionActivity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::WaitingForPermission => "permission",
        }
    }
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
    pub goal_mode: bool,
    pub agent_mode: AgentMode,
    pub agent_tasks: Vec<AgentTaskItem>,
    pub autonomy_mode: AutonomyMode,
    pub error_message: Option<String>,
    pub active_model: String,
    pub active_reasoning_level: String,
    pub active_model_context_length: Option<u64>,
    pub current_context_tokens: Option<u64>,
    pub token_usage: Usage,
    pub credit_usage: u64,
    pub pending_shell_permission: Option<ShellPermission>,
    pub pending_native_app_permission: Option<NativeAppPermission>,
    pub extension_commands: Vec<ExtensionCommandItem>,
    pub available_models: Vec<String>,
    pub model_catalog: Vec<ModelCatalogItem>,
    pub is_loading_models: bool,
    pub saved_sessions: Vec<SessionSummary>,
    /// Ephemeral activity for all sessions owned by the background service.
    pub session_activity: std::collections::BTreeMap<String, SessionActivity>,
    pub known_workspaces: Vec<WorkspaceSummary>,
    pub workspace_session_groups: Vec<WorkspaceSessionGroup>,
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
    pub foundation_memory_backend: String,
    pub foundation_memory_server: String,
    pub foundation_memory_connected: bool,
    pub web_backend: String,
    pub web_server: String,
    pub settings_notice: Option<String>,
    pub settings_working: bool,
    pub runtime_settings: RuntimeSettingsState,
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
            goal_mode: self.goal_mode,
            agent_mode: self.agent_mode,
            agent_tasks: self.agent_tasks.clone(),
            autonomy_mode: self.autonomy_mode,
            error_message: self.error_message.clone(),
            active_model: self.active_model.clone(),
            active_reasoning_level: self.active_reasoning_level.clone(),
            active_model_context_length: self.active_model_context_length,
            current_context_tokens: self.current_context_tokens,
            token_usage: self.token_usage.clone(),
            credit_usage: self.credit_usage,
            pending_shell_permission: self.pending_shell_permission.clone(),
            pending_native_app_permission: self.pending_native_app_permission.clone(),
            extension_commands: self.extension_commands.clone(),
            available_models: self.available_models.clone(),
            model_catalog: self.model_catalog.clone(),
            is_loading_models: self.is_loading_models,
            saved_sessions: self.saved_sessions.clone(),
            session_activity: self.session_activity.clone(),
            known_workspaces: self.known_workspaces.clone(),
            workspace_session_groups: self.workspace_session_groups.clone(),
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
            foundation_memory_backend: self.foundation_memory_backend.clone(),
            foundation_memory_server: self.foundation_memory_server.clone(),
            foundation_memory_connected: self.foundation_memory_connected,
            web_backend: self.web_backend.clone(),
            web_server: self.web_server.clone(),
            settings_notice: self.settings_notice.clone(),
            settings_working: self.settings_working,
            runtime_settings: self.runtime_settings.clone(),
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
    pub usage: Option<ProviderUsageStatus>,
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
pub struct ModelCatalogItem {
    pub id: String,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub context_length: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct RuntimeSettingsState {
    pub appearance: String,
    pub theme_dark: String,
    pub theme_light: String,
    pub theme_dark_resolved: String,
    pub theme_light_resolved: String,
    pub theme_dark_palette: crate::theme::PaletteState,
    pub theme_light_palette: crate::theme::PaletteState,
    pub theme_catalog: Vec<crate::theme::ThemeCatalogItem>,
    pub theme_dark_warning: Option<String>,
    pub theme_light_warning: Option<String>,
    pub context_length_override: Option<u64>,
    pub jev_loop_mode: String,
}

impl Default for RuntimeSettingsState {
    fn default() -> Self {
        Self {
            appearance: "auto".into(),
            theme_dark: "kanagawa".into(),
            theme_light: "adwaita".into(),
            theme_dark_resolved: "kanagawa".into(),
            theme_light_resolved: "adwaita".into(),
            theme_dark_palette: crate::theme::Palette::kanagawa().into(),
            theme_light_palette: crate::theme::Palette::adwaita().into(),
            theme_catalog: crate::theme::catalog(),
            theme_dark_warning: None,
            theme_light_warning: None,
            context_length_override: None,
            jev_loop_mode: "off".into(),
        }
    }
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

impl SandboxSettingsState {
    pub fn permission_mode(&self) -> &'static str {
        if self.execution_mode == "unlimited" {
            "unlimited"
        } else if self.auto_approve {
            "auto"
        } else {
            "ask"
        }
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub model: String,
    pub message_count: usize,
}

impl SessionSummary {
    /// A single-line label shared by the session picker and sidebar.
    pub fn display_title(&self) -> String {
        let title = self.title.split_whitespace().collect::<Vec<_>>().join(" ");
        if title.is_empty() {
            "Untitled session".to_owned()
        } else {
            title
        }
    }

    pub fn updated_label(&self) -> String {
        self.updated_label_at(chrono::Utc::now())
    }

    fn updated_label_at(&self, now: chrono::DateTime<chrono::Utc>) -> String {
        let Ok(updated) = chrono::DateTime::parse_from_rfc3339(&self.updated_at) else {
            return "unknown date".to_owned();
        };
        let elapsed = now.signed_duration_since(updated);
        if elapsed.num_seconds() < 0 {
            updated.format("%Y-%m-%d").to_string()
        } else if elapsed.num_minutes() < 1 {
            format!("{}s", elapsed.num_seconds())
        } else if elapsed.num_hours() < 1 {
            format!("{}m ago", elapsed.num_minutes())
        } else if elapsed.num_days() < 1 {
            format!("{}h ago", elapsed.num_hours())
        } else if elapsed.num_days() < 7 {
            format!("{}d ago", elapsed.num_days())
        } else {
            updated.format("%Y-%m-%d").to_string()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceSummary {
    pub id: String,
    pub path: String,
    pub display_name: String,
    pub updated_at: Option<String>,
    pub session_count: usize,
    pub is_current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceSessionGroup {
    pub workspace_id: String,
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationEntry {
    pub id: String,
    pub kind: ConversationKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)] // Preserve the public serde/UI model shape without boxing variants.
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

    /// Backend-authored human-readable operation label. Frontends should not infer semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Optional target/purpose shown beside the operation type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, rename = "startedAt", skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, rename = "endedAt", skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(
        default,
        rename = "durationMs",
        skip_serializing_if = "Option::is_none"
    )]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(
        default,
        rename = "parentCallId",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_call_id: Option<String>,
    #[serde(
        default,
        rename = "parallelGroupId",
        skip_serializing_if = "Option::is_none"
    )]
    pub parallel_group_id: Option<String>,
    #[serde(default, rename = "jobId", skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Preparing,
    AwaitingPermission,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
    TimedOut,
    Suppressed,
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

pub use crate::actions::FrontendCommand;

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
mod reasoning_level_tests {
    use super::*;

    #[test]
    fn reasoning_levels_follow_model_capabilities() {
        assert_eq!(
            reasoning_levels_for_model("openai/gpt-5.6-sol"),
            &["auto", "low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            reasoning_levels_for_model("openai/gpt-6-astra"),
            &["auto", "low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            reasoning_levels_for_model("openrouter/anthropic/claude-sonnet-5"),
            &["auto", "low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            reasoning_levels_for_model("gemini/gemini-3.8-flash"),
            &["auto", "low", "medium", "high"]
        );
        assert_eq!(
            reasoning_levels_for_model("anthropic/claude-opus-4.8"),
            &["auto", "low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            reasoning_levels_for_model("anthropic/claude-sonnet-4-5"),
            &["auto", "low", "medium", "high"]
        );
        assert_eq!(
            reasoning_levels_for_model("anthropic/claude-sonnet-4-6"),
            &["auto", "low", "medium", "high"]
        );
    }
}
