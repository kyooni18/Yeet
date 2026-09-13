use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::model::{
    BridgeState, ConversationEntry, ConversationToolCall, FrontendCommand, ModelActivity,
};

pub const REMOTE_PROTOCOL_MIN_VERSION: u16 = 1;
pub const REMOTE_PROTOCOL_MAX_VERSION: u16 = 1;
pub const REMOTE_PROTOCOL_VERSION: u16 = REMOTE_PROTOCOL_MAX_VERSION;

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
            Self::Command { version, .. } | Self::Ping { version, .. } => Some(*version),
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
            Self::Welcome { .. } | Self::Ack { .. } | Self::Error { .. } | Self::Pong { .. } => {
                None
            }
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
            Self::Welcome { .. } | Self::Ack { .. } | Self::Error { .. } | Self::Pong { .. } => {
                None
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelCatalogItem, SessionSummary, WorkspaceSessionGroup, WorkspaceSummary};

    #[test]
    fn protocol_serializes_command_using_existing_frontend_command_shape() {
        let value = serde_json::to_value(ClientMessage::Command {
            version: REMOTE_PROTOCOL_VERSION,
            request_id: Some("request-1".into()),
            command: FrontendCommand::SelectModel {
                model: "gpt-5.6-sol".into(),
            },
        })
        .unwrap();

        assert_eq!(value["type"], "command");
        assert_eq!(value["version"], 1);
        assert_eq!(value["command"]["type"], "select_model");
        assert_eq!(value["command"]["model"], "gpt-5.6-sol");
    }

    #[test]
    fn protocol_negotiates_highest_common_version() {
        assert_eq!(negotiate_version(1, 3).unwrap(), 1);
        assert!(negotiate_version(2, 3).is_err());
        assert!(negotiate_version(0, 0).is_err());
    }

    #[test]
    fn required_websocket_commands_decode_as_existing_frontend_commands() {
        let messages = [
            r#"{"type":"command","version":1,"command":{"type":"submit","text":"hi"}}"#,
            r#"{"type":"command","version":1,"command":{"type":"interrupt"}}"#,
            r#"{"type":"command","version":1,"command":{"type":"select_model","model":"model"}}"#,
            r#"{"type":"command","version":1,"command":{"type":"select_reasoning","level":"high"}}"#,
            r#"{"type":"command","version":1,"command":{"type":"request_sessions"}}"#,
            r#"{"type":"command","version":1,"command":{"type":"load_session","session_id":"session"}}"#,
            r#"{"type":"command","version":1,"command":{"type":"new_session"}}"#,
            r#"{"type":"command","version":1,"command":{"type":"toggle_capability","id":"web_search"}}"#,
            r#"{"type":"command","version":1,"command":{"type":"auth_login","provider":"openai"}}"#,
            r#"{"type":"command","version":1,"command":{"type":"save_provider","id":"custom","base_url":"https://example.test","require_api_key":true}}"#,
            r#"{"type":"command","version":1,"command":{"type":"request_sandbox"}}"#,
            r#"{"type":"command","version":1,"command":{"type":"update_sandbox","action":{"type":"set_auto_approve","enabled":true}}}"#,
            r#"{"type":"command","version":1,"command":{"type":"allow_shell"}}"#,
            r#"{"type":"command","version":1,"command":{"type":"deny_native_app"}}"#,
        ];
        for raw in messages {
            assert!(
                matches!(
                    decode_client_message(raw),
                    Ok(ClientMessage::Command { .. })
                ),
                "{raw}"
            );
        }
    }

    #[test]
    fn protocol_mismatch_error_advertises_supported_range() {
        let value = serde_json::to_value(ServerMessage::error(
            "protocol_mismatch",
            "unsupported",
            true,
            None,
        ))
        .unwrap();
        assert_eq!(value["version"], REMOTE_PROTOCOL_VERSION);
        assert_eq!(value["supported_min_version"], REMOTE_PROTOCOL_MIN_VERSION);
        assert_eq!(value["supported_max_version"], REMOTE_PROTOCOL_MAX_VERSION);
        assert_eq!(value["fatal"], true);
    }

    #[test]
    fn malformed_messages_are_rejected() {
        assert!(decode_client_message("{not-json").is_err());
        assert!(decode_client_message(r#"{"type":"command","version":1}"#).is_err());
    }

    #[test]
    fn snapshot_round_trips_bridge_state() {
        let message = ServerMessage::Snapshot {
            version: REMOTE_PROTOCOL_VERSION,
            sequence: 7,
            revision: 3,
            state: BridgeState {
                active_model: "model".into(),
                conversation_revision: 3,
                ..BridgeState::default()
            },
        };
        let bytes = serde_json::to_vec(&message).unwrap();
        let decoded: ServerMessage = serde_json::from_slice(&bytes).unwrap();
        match decoded {
            ServerMessage::Snapshot {
                sequence,
                revision,
                state,
                ..
            } => {
                assert_eq!(sequence, 7);
                assert_eq!(revision, 3);
                assert_eq!(state.active_model, "model");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn protocol_v1_snapshot_carries_additive_semantic_catalogs() {
        let message = ServerMessage::Snapshot {
            version: REMOTE_PROTOCOL_VERSION,
            sequence: 9,
            revision: 4,
            state: BridgeState {
                active_model: "openai/gpt-5.6-sol".into(),
                model_catalog: vec![ModelCatalogItem {
                    id: "openai/gpt-5.6-sol".into(),
                    provider: "openai".into(),
                    model: "gpt-5.6-sol".into(),
                    context_length: Some(128_000),
                }],
                known_workspaces: vec![WorkspaceSummary {
                    id: "/tmp/project".into(),
                    path: "/tmp/project".into(),
                    display_name: "project".into(),
                    updated_at: Some("2026-09-10T00:00:00Z".into()),
                    session_count: 1,
                    is_current: true,
                }],
                workspace_session_groups: vec![WorkspaceSessionGroup {
                    workspace_id: "/tmp/project".into(),
                    sessions: vec![SessionSummary {
                        id: "session-1".into(),
                        title: "Session".into(),
                        updated_at: "2026-09-10T00:00:00Z".into(),
                        model: "openai/gpt-5.6-sol".into(),
                        message_count: 3,
                    }],
                }],
                ..BridgeState::default()
            },
        };

        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["state"]["model_catalog"][0]["provider"], "openai");
        assert_eq!(
            value["state"]["workspace_session_groups"][0]["sessions"][0]["id"],
            "session-1"
        );

        let decoded: ServerMessage = serde_json::from_value(value).unwrap();
        let ServerMessage::Snapshot { state, .. } = decoded else {
            panic!("expected snapshot");
        };
        assert_eq!(state.model_catalog[0].model, "gpt-5.6-sol");
        assert_eq!(state.workspace_session_groups[0].sessions.len(), 1);
    }

    #[test]
    fn protocol_v1_snapshot_without_new_catalog_fields_still_decodes() {
        let raw = r#"{
            "type":"snapshot",
            "version":1,
            "sequence":1,
            "revision":1,
            "state":{"active_model":"openai/gpt-5.6-sol","conversation_revision":1}
        }"#;
        let decoded: ServerMessage = serde_json::from_str(raw).unwrap();
        let ServerMessage::Snapshot { state, .. } = decoded else {
            panic!("expected snapshot");
        };
        assert!(state.model_catalog.is_empty());
        assert!(state.known_workspaces.is_empty());
        assert!(state.workspace_session_groups.is_empty());
    }
}
