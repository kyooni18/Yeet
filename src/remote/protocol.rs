use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::model::{
    BridgeState, ConversationEntry, ConversationToolCall, FrontendCommand, ModelActivity,
};

pub const REMOTE_PROTOCOL_MIN_VERSION: u16 = 1;
pub const REMOTE_PROTOCOL_MAX_VERSION: u16 = 1;
pub const REMOTE_PROTOCOL_VERSION: u16 = REMOTE_PROTOCOL_MAX_VERSION;
pub const REMOTE_PROTOCOL_FEATURES: &[&str] = &[
    "attachments-v1",
    "files-v1",
    "turn-replay-v1",
    "shared-ui-v1",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        min_version: u16,
        max_version: u16,
        #[serde(default)]
        client_id: Option<String>,
        #[serde(default)]
        workspace: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        last_sequence: Option<u64>,
        #[serde(default)]
        last_revision: Option<u64>,
    },
    Command {
        version: u16,
        #[serde(default)]
        request_id: Option<String>,
        command: FrontendCommand,
    },
    UiAction {
        version: u16,
        #[serde(default)]
        request_id: Option<String>,
        action: crate::shared_ui::shell::ShellAction,
    },
    UiAgentAction {
        version: u16,
        #[serde(default)]
        request_id: Option<String>,
        action: crate::shared_ui::agents::AgentAction,
    },
    UiConversationAction {
        version: u16,
        #[serde(default)]
        request_id: Option<String>,
        action: crate::shared_ui::conversation::ConversationAction,
    },
    Ping {
        version: u16,
        #[serde(default)]
        nonce: Option<String>,
    },
}

impl ClientMessage {
    pub fn exact_version(&self) -> Option<u16> {
        match self {
            Self::Hello { .. } => None,
            Self::Command { version, .. }
            | Self::UiAction { version, .. }
            | Self::UiAgentAction { version, .. }
            | Self::UiConversationAction { version, .. }
            | Self::Ping { version, .. } => Some(*version),
        }
    }
}

// Snapshot messages intentionally carry the complete bridge state and are serialized
// immediately. Keeping the protocol shape direct avoids heap indirection at every
// encode/decode call site while the large payload remains isolated to snapshots.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    UiState {
        version: u16,
        ui_revision: u64,
        request_id: Option<String>,
        state: crate::shared_ui::shell::ShellState,
        view: crate::shared_ui::shell::ShellView,
    },
    UiAgents {
        version: u16,
        agents_revision: u64,
        request_id: Option<String>,
        view: crate::shared_ui::agents::AgentsView,
        effect: Option<crate::shared_ui::agents::AgentUiEffect>,
    },
    UiConversation {
        version: u16,
        conversation_revision: u64,
        request_id: Option<String>,
        view: crate::shared_ui::conversation::ConversationView,
        effect: Option<crate::shared_ui::conversation::ConversationUiEffect>,
    },
    Welcome {
        version: u16,
        client_id: String,
        workspace: String,
        session_id: Option<String>,
        sequence: u64,
        revision: u64,
        resumed: bool,
    },
    Snapshot {
        version: u16,
        sequence: u64,
        revision: u64,
        state: BridgeState,
    },
    StateUpdate {
        version: u16,
        sequence: u64,
        revision: u64,
        patch: Map<String, Value>,
    },
    AssistantDelta {
        version: u16,
        sequence: u64,
        revision: u64,
        entry_id: Option<String>,
        delta: String,
        content: String,
        reset: bool,
    },
    ReasoningDelta {
        version: u16,
        sequence: u64,
        revision: u64,
        entry_id: Option<String>,
        delta: String,
        content: String,
        summary: bool,
        reset: bool,
    },
    ConversationEntry {
        version: u16,
        sequence: u64,
        revision: u64,
        entry: ConversationEntry,
    },
    ConversationReset {
        version: u16,
        sequence: u64,
        revision: u64,
        conversation: Vec<ConversationEntry>,
    },
    ToolUpdate {
        version: u16,
        sequence: u64,
        revision: u64,
        entry: ConversationEntry,
        tool_call: Option<ConversationToolCall>,
    },
    ActivityUpdate {
        version: u16,
        sequence: u64,
        revision: u64,
        entry: ConversationEntry,
        activity: ModelActivity,
    },
    Ack {
        version: u16,
        request_id: Option<String>,
    },
    Error {
        version: u16,
        code: String,
        message: String,
        fatal: bool,
        request_id: Option<String>,
        supported_min_version: u16,
        supported_max_version: u16,
    },
    Pong {
        version: u16,
        nonce: Option<String>,
    },
}

impl ServerMessage {
    pub fn error(
        code: impl Into<String>,
        message: impl Into<String>,
        fatal: bool,
        request_id: Option<String>,
    ) -> Self {
        Self::Error {
            version: REMOTE_PROTOCOL_VERSION,
            code: code.into(),
            message: message.into(),
            fatal,
            request_id,
            supported_min_version: REMOTE_PROTOCOL_MIN_VERSION,
            supported_max_version: REMOTE_PROTOCOL_MAX_VERSION,
        }
    }

    pub fn sequence(&self) -> Option<u64> {
        match self {
            Self::Snapshot { sequence, .. }
            | Self::StateUpdate { sequence, .. }
            | Self::AssistantDelta { sequence, .. }
            | Self::ReasoningDelta { sequence, .. }
            | Self::ConversationEntry { sequence, .. }
            | Self::ConversationReset { sequence, .. }
            | Self::ToolUpdate { sequence, .. }
            | Self::ActivityUpdate { sequence, .. } => Some(*sequence),
            Self::UiState { .. }
            | Self::UiAgents { .. }
            | Self::UiConversation { .. }
            | Self::Welcome { .. }
            | Self::Ack { .. }
            | Self::Error { .. }
            | Self::Pong { .. } => None,
        }
    }

    pub fn revision(&self) -> Option<u64> {
        match self {
            Self::Snapshot { revision, .. }
            | Self::StateUpdate { revision, .. }
            | Self::AssistantDelta { revision, .. }
            | Self::ReasoningDelta { revision, .. }
            | Self::ConversationEntry { revision, .. }
            | Self::ConversationReset { revision, .. }
            | Self::ToolUpdate { revision, .. }
            | Self::ActivityUpdate { revision, .. } => Some(*revision),
            Self::UiState { .. }
            | Self::UiAgents { .. }
            | Self::UiConversation { .. }
            | Self::Welcome { .. }
            | Self::Ack { .. }
            | Self::Error { .. }
            | Self::Pong { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolMismatch {
    pub client_min: u16,
    pub client_max: u16,
}

pub fn negotiate_version(min_version: u16, max_version: u16) -> Result<u16, ProtocolMismatch> {
    let low = min_version.max(REMOTE_PROTOCOL_MIN_VERSION);
    let high = max_version.min(REMOTE_PROTOCOL_MAX_VERSION);
    if low > high {
        return Err(ProtocolMismatch {
            client_min: min_version,
            client_max: max_version,
        });
    }
    Ok(high)
}

pub fn validate_exact_version(version: u16) -> Result<(), ProtocolMismatch> {
    negotiate_version(version, version).map(|_| ())
}

pub fn decode_client_message(raw: &str) -> Result<ClientMessage, serde_json::Error> {
    serde_json::from_str(raw)
}
