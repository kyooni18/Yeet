//! Harness commands shared by run hosts and frontends.
//!
//! Independent of Skyline deployment and orchestration. Adapters interpret
//! these intents; this module never renders UI or starts a Skyline service.

use crate::model::{AgentGroupSettings, AgentMode, AutonomyMode, SandboxAction};
use serde::{Deserialize, Serialize};

// FrontendCommand is defined here and re-exported by model for wire compatibility.

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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<crate::core::ImageAttachment>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachment_ids: Vec<String>,
    },
    Interrupt,
    RegenerateLast,
    EditLast {
        text: String,
    },
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
    SetGoal {
        enabled: bool,
    },
    SetAgentMode {
        mode: AgentMode,
    },
    SetAutonomyMode {
        mode: AutonomyMode,
    },
    /// Creates one durable Group Agent for a shared objective.
    CreateAgentGroup {
        objective: String,
    },
    /// Starts the stored Group Agent coordinator asynchronously.
    StartAgentGroup {
        group_id: String,
    },
    ResumeAgentGroup {
        group_id: String,
    },
    CancelAgentGroup {
        group_id: String,
    },
    StopAgentGroup {
        group_id: String,
    },
    InspectAgentGroup {
        group_id: String,
    },
    /// Advanced member control scoped to the active Group Agent.
    MessageAgent {
        agent_id: String,
        message: String,
    },
    /// Compatibility shortcut: turns the supplied task into a Group Agent objective.
    SpawnAgent {
        role: String,
        description: String,
        prompt: String,
    },
    /// Removes a member from the group, or every stopped member when
    /// `agent_id` is `None`.
    RemoveAgent {
        #[serde(default)]
        agent_id: Option<String>,
    },
    /// Stops a group member, or every member when `agent_id` is `None`.
    StopAgent {
        #[serde(default)]
        agent_id: Option<String>,
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
    SetAppearance {
        appearance: String,
    },
    SetTheme {
        mode: String,
        value: String,
    },
    SetContextLength {
        length: Option<u64>,
    },
    SetJevLoopMode {
        mode: String,
    },
    SetAgentGroupSettings {
        settings: AgentGroupSettings,
    },
    SetOpenAiFlex {
        enabled: bool,
    },
    SetFoundationMemory {
        enabled: bool,
    },
    SetServiceBackend {
        service: String,
        backend: String,
        #[serde(default)]
        server: Option<String>,
    },
    RequestSandbox,
    UpdateSandbox {
        action: SandboxAction,
    },
    ExtensionCommand {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_keep_existing_wire_format() {
        let command = FrontendCommand::Submit {
            text: "hello".into(),
            images: Vec::new(),
            attachment_ids: Vec::new(),
        };
        assert_eq!(
            serde_json::to_value(&command).unwrap(),
            serde_json::json!({
                "type": "submit", "text": "hello"
            })
        );
    }
}
