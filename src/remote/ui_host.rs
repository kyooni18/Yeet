//! Shared UI projection packaging and host capabilities for Remote clients.

use crate::{
    model::BridgeState,
    shared_ui::{
        agents::AgentUiEffect,
        agents_session::AgentProjection,
        application_session::{ApplicationProjection, ApplicationSession},
        composer::{ComposerDestination, ComposerUiEffect},
        composer_session::ComposerProjection,
        conversation::ConversationUiEffect,
        conversation_session::ConversationProjection,
        settings::SettingsUiEffect,
        settings_session::SettingsProjection,
    },
};

use super::protocol::{REMOTE_PROTOCOL_VERSION, ServerMessage};

pub(super) fn application_messages(
    projection: ApplicationProjection,
    request_id: Option<String>,
    effect: Option<AgentUiEffect>,
) -> [ServerMessage; 5] {
    [
        ServerMessage::UiState {
            version: REMOTE_PROTOCOL_VERSION,
            ui_revision: projection.ui_revision,
            request_id: request_id.clone(),
            state: projection.state,
            view: projection.view,
        },
        agents_message(projection.agents, request_id.clone(), effect),
        conversation_message(projection.conversation, request_id.clone(), None),
        composer_message(projection.composer, request_id.clone(), None),
        settings_message(projection.settings, request_id, None),
    ]
}

pub(super) fn agents_message(
    projection: AgentProjection,
    request_id: Option<String>,
    effect: Option<AgentUiEffect>,
) -> ServerMessage {
    ServerMessage::UiAgents {
        version: REMOTE_PROTOCOL_VERSION,
        agents_revision: projection.agents_revision,
        request_id,
        view: projection.view,
        effect,
    }
}

pub(super) fn conversation_message(
    projection: ConversationProjection,
    request_id: Option<String>,
    effect: Option<ConversationUiEffect>,
) -> ServerMessage {
    ServerMessage::UiConversation {
        version: REMOTE_PROTOCOL_VERSION,
        conversation_revision: projection.conversation_revision,
        request_id,
        view: projection.view,
        effect,
    }
}

pub(super) fn composer_message(
    projection: ComposerProjection,
    request_id: Option<String>,
    effect: Option<ComposerUiEffect>,
) -> ServerMessage {
    ServerMessage::UiComposer {
        version: REMOTE_PROTOCOL_VERSION,
        composer_revision: projection.composer_revision,
        request_id,
        view: projection.view,
        effect,
    }
}

pub(super) fn settings_message(
    projection: SettingsProjection,
    request_id: Option<String>,
    effect: Option<SettingsUiEffect>,
) -> ServerMessage {
    ServerMessage::UiSettings {
        version: REMOTE_PROTOCOL_VERSION,
        settings_revision: projection.settings_revision,
        request_id,
        view: projection.view,
        effect,
    }
}

pub(super) fn configure_composer_host(
    ui: &mut ApplicationSession,
    state: &BridgeState,
    workspace: Option<String>,
) -> bool {
    use ComposerDestination::*;
    let mut environment = ui.composer_environment().clone();
    if let Some(workspace) = workspace {
        environment.context.workspace = workspace;
    }
    environment.context.session_id = state.current_session_id.clone();
    environment.available = true;
    environment.supported_destinations = vec![
        Models,
        Sessions,
        Settings,
        Permissions,
        Auth,
        Providers,
        Capabilities,
    ];
    ui.configure_composer(environment, state).is_some()
}

