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
) -> [ServerMessage; 6] {
    [
        ServerMessage::UiState {
            toolbar: projection.toolbar,
            application: projection.application,
            version: REMOTE_PROTOCOL_VERSION,
            ui_revision: projection.ui_revision,
            request_id: request_id.clone(),
            state: projection.state,
            view: projection.view,
        },
        agents_message(projection.agents, request_id.clone(), effect),
        conversation_message(projection.conversation, request_id.clone(), None),
        composer_message(projection.composer, request_id.clone(), None),
        settings_message(projection.settings, request_id.clone(), None),
        ServerMessage::UiHome {
            version: REMOTE_PROTOCOL_VERSION,
            home_revision: projection.home_revision,
            request_id,
            view: projection.home,
        },
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

/// Keep toolbar delivery and UI commit on the serialized host lane.
pub(super) fn run_toolbar_action(
    shared: &std::sync::Arc<std::sync::Mutex<super::websocket::RuntimeShared>>,
    ui: &std::sync::Arc<std::sync::Mutex<ApplicationSession>>,
    service: &mut crate::harness::HarnessService,
    events: &tokio::sync::broadcast::Sender<ServerMessage>,
    action: crate::shared_ui::toolbar::ToolbarAction,
    request_id: Option<String>,
) -> Result<(), String> {
    let mut state = shared
        .lock()
        .map_err(|_| "remote runtime state lock poisoned")?;
    let mut ui = ui.lock().map_err(|_| "remote UI state lock poisoned")?;
    let prepared = ui.prepare_toolbar(action, &state.state);
    let workspace_switch = prepared.effect.workspace_switch.clone();
    if let Some(command) = prepared.effect.command.clone() {
        let theme_command = command.clone();
        service
            .send(command)
            .map_err(|error| format!("{error:#}"))?;
        state.theme_cache.invalidate_for_command(&theme_command);
    }
    let (projection, _) = ui.commit_toolbar(prepared, &state.state);
    for message in application_messages(projection, request_id, None) {
        let _ = events.send(message);
    }
    if let Some(workspace) = workspace_switch {
        let _ = events.send(ServerMessage::WorkspaceSwitchRequested {
            version: REMOTE_PROTOCOL_VERSION,
            id: workspace.id,
            path: workspace.path,
            source_workspace: workspace.source_workspace,
        });
    }
    Ok(())
}

pub(super) async fn enqueue_toolbar(
    commands: &std::sync::mpsc::Sender<super::websocket::RuntimeControl>,
    wake: &crate::background::Wake,
    action: crate::shared_ui::toolbar::ToolbarAction,
    request_id: Option<String>,
    timeout: std::time::Duration,
) -> anyhow::Result<()> {
    let (completion, result) = tokio::sync::oneshot::channel();
    commands
        .send(super::websocket::RuntimeControl::ToolbarAction {
            action,
            request_id,
            completion,
        })
        .map_err(|_| anyhow::anyhow!("semantic Remote runtime is no longer available"))?;
    wake.notify();
    match tokio::time::timeout(timeout, result).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(error))) => Err(anyhow::anyhow!(error)),
        Ok(Err(_)) => Err(anyhow::anyhow!(
            "semantic Remote runtime stopped before confirming UI action delivery"
        )),
        Err(_) => Err(anyhow::anyhow!("timed out delivering Remote UI action")),
    }
}
