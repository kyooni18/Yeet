//! Agent-owned actions shared by run hosts and frontends.
//!
//! Independent of Skyline deployment and orchestration. Adapters interpret
//! these intents; this module never renders UI or starts a Skyline service.

use crate::model::{AgentMode, AutonomyMode, SandboxAction, SwarmSettings};
use serde::{Deserialize, Serialize};

/// Stable navigation intents shared by keyboard, mouse and other frontends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "index", rename_all = "snake_case")]
pub enum NavigationAction {
    Home,
    Session,
    Files,
    File(usize),
    CloseFile(usize),
    Diff(usize),
    CloseDiff(usize),
    NewDiff,
    Launcher,
}

/// A common envelope without forcing navigation through the backend transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "scope", content = "action", rename_all = "snake_case")]
pub enum Action {
    Navigate(NavigationAction),
    Command(FrontendCommand),
}

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
    /// Queues a user message for a group member; it runs in the background.
    MessageAgent {
        agent_id: String,
        message: String,
    },
    /// Launches a background group member on the active model.
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
    SetSwarmSettings {
        settings: SwarmSettings,
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
    fn commands_keep_existing_wire_format_and_actions_round_trip() {
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
        let action = Action::Navigate(NavigationAction::File(2));
        let encoded = serde_json::to_string(&action).unwrap();
        let decoded: Action = serde_json::from_str(&encoded).unwrap();
        assert!(matches!(
            decoded,
            Action::Navigate(NavigationAction::File(2))
        ));
    }
}
