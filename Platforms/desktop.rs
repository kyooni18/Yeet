use std::{sync::mpsc, time::Duration};
use tauri::{Emitter, Manager};
use yeet::harness::{Harness, HarnessCommand, HarnessState};
use yeet::shared_ui::agents::{AgentAction, AgentUiEffect};
use yeet::shared_ui::agents_session::AgentProjection;
use yeet::shared_ui::application_session::{
    ApplicationProjection, ApplicationSession, ShellProjection as UiProjection,
};
use yeet::shared_ui::composer::{ComposerAction, ComposerUiEffect};
use yeet::shared_ui::composer_session::ComposerProjection;
use yeet::shared_ui::conversation::{ConversationAction, ConversationUiEffect};
use yeet::shared_ui::conversation_session::ConversationProjection;
use yeet::shared_ui::home::{HomeAction, HomeView, RecentViews, ResourceTarget, WorkspaceContent};
use yeet::shared_ui::settings::{SettingsAction, SettingsUiEffect};
use yeet::shared_ui::settings_session::SettingsProjection;
use yeet::shared_ui::shell::ShellAction;

#[derive(serde::Serialize)]
struct CoreConnection {
    workspace: String,
    state: Option<HarnessState>,
}

enum Control {
    Connect(String, mpsc::Sender<Result<CoreConnection, String>>),
    Command(HarnessCommand, mpsc::Sender<Result<(), String>>),
    ApplicationProjection(mpsc::Sender<Result<ApplicationProjection, String>>),
    HomeProjection(mpsc::Sender<Result<HomeProjection, String>>),
    UiProjection(mpsc::Sender<Result<UiProjection, String>>),
    ShellAction(ShellAction, mpsc::Sender<Result<(), String>>),
    HomeAction(HomeAction, mpsc::Sender<Result<(), String>>),
    AgentsProjection(mpsc::Sender<Result<AgentProjection, String>>),
    AgentAction(AgentAction, mpsc::Sender<Result<(), String>>),
    ConversationAction(ConversationAction, mpsc::Sender<Result<(), String>>),
    ComposerAction(ComposerAction, mpsc::Sender<Result<(), String>>),
    ToolbarAction(
        yeet::shared_ui::toolbar::ToolbarAction,
        mpsc::Sender<Result<(), String>>,
    ),
    SettingsAction(SettingsAction, mpsc::Sender<Result<(), String>>),
    Disconnect(mpsc::Sender<()>),
    Stop,
}

struct CoreHost(mpsc::Sender<Control>);

#[derive(Clone, serde::Serialize)]
struct UiMessage {
    version: u16,
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: Option<String>,
    application: yeet::shared_ui::application::ApplicationView,
    #[serde(flatten)]
    projection: UiProjection,
}

#[derive(Debug, Clone, serde::Serialize)]
struct HomeProjection {
    home_revision: u64,
    view: HomeView,
}

#[derive(Clone, serde::Serialize)]
struct HomeEffectMessage {
    version: u16,
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: Option<String>,
    open: Option<ResourceTarget>,
}

fn publish_home_effect(app: &tauri::AppHandle, open: Option<ResourceTarget>) -> Result<(), String> {
    app.emit(
        "yeet://ui-home-effect",
        HomeEffectMessage {
            version: 1,
            kind: "ui_home_effect",
            request_id: None,
            open,
        },
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn home_projection(host: tauri::State<'_, CoreHost>) -> Result<HomeProjection, String> {
    let (reply, response) = mpsc::channel();
    host.0
        .send(Control::HomeProjection(reply))
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || response.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn application_projection(
    host: tauri::State<'_, CoreHost>,
) -> Result<ApplicationProjection, String> {
    let (reply, response) = mpsc::channel();
    host.0
        .send(Control::ApplicationProjection(reply))
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || response.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn ui_projection(host: tauri::State<'_, CoreHost>) -> Result<UiProjection, String> {
    let (reply, response) = mpsc::channel();
    host.0
        .send(Control::UiProjection(reply))
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || response.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn send_ui_action(
    action: ShellAction,
    host: tauri::State<'_, CoreHost>,
) -> Result<(), String> {
    let (reply, response) = mpsc::channel();
    host.0
        .send(Control::ShellAction(action, reply))
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || response.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn send_ui_home_action(
    action: HomeAction,
    host: tauri::State<'_, CoreHost>,
) -> Result<(), String> {
    let (reply, response) = mpsc::channel();
    host.0
        .send(Control::HomeAction(action, reply))
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || response.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
}

#[derive(Clone, serde::Serialize)]
struct AgentsMessage {
    effect: Option<AgentUiEffect>,
    version: u16,
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: Option<String>,
    #[serde(flatten)]
    projection: AgentProjection,
}

#[derive(Clone, serde::Serialize)]
struct ConversationMessage {
    version: u16,
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: Option<String>,
    effect: Option<ConversationUiEffect>,
    #[serde(flatten)]
    projection: ConversationProjection,
}
fn publish_conversation(
    app: &tauri::AppHandle,
    projection: ConversationProjection,
    effect: Option<ConversationUiEffect>,
) -> Result<(), String> {
    app.emit(
        "yeet://ui-conversation-event",
        ConversationMessage {
            version: 1,
            kind: "ui_conversation",
            request_id: None,
            projection,
            effect,
        },
    )
    .map_err(|error| error.to_string())
}
#[tauri::command]
async fn send_ui_conversation_action(
    action: ConversationAction,
    host: tauri::State<'_, CoreHost>,
) -> Result<(), String> {
    let (reply, response) = mpsc::channel();
    host.0
        .send(Control::ConversationAction(action, reply))
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || response.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
}

#[derive(Clone, serde::Serialize)]
struct ComposerMessage {
    version: u16,
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: Option<String>,
    effect: Option<ComposerUiEffect>,
    #[serde(flatten)]
    projection: ComposerProjection,
}
fn publish_composer(
    app: &tauri::AppHandle,
    projection: ComposerProjection,
    effect: Option<ComposerUiEffect>,
) -> Result<(), String> {
    app.emit(
        "yeet://ui-composer-event",
        ComposerMessage {
            version: 1,
            kind: "ui_composer",
            request_id: None,
            projection,
            effect,
        },
    )
    .map_err(|error| error.to_string())
}
#[tauri::command]
async fn send_ui_composer_action(
    action: ComposerAction,
    host: tauri::State<'_, CoreHost>,
) -> Result<(), String> {
    let (reply, response) = mpsc::channel();
    host.0
        .send(Control::ComposerAction(action, reply))
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || response.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
}

#[derive(Clone, serde::Serialize)]
struct SettingsMessage {
    version: u16,
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: Option<String>,
    effect: Option<SettingsUiEffect>,
    #[serde(flatten)]
    projection: SettingsProjection,
}
#[derive(Clone, serde::Serialize)]
struct HomeMessage {
    version: u16,
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: Option<String>,
    home_revision: u64,
    view: HomeView,
}
fn publish_home(app: &tauri::AppHandle, projection: &ApplicationProjection) -> Result<(), String> {
    app.emit(
        "yeet://ui-home-event",
        HomeMessage {
            version: 1,
            kind: "ui_home",
            request_id: None,
            home_revision: projection.home_revision,
            view: projection.home.clone(),
        },
    )
    .map_err(|error| error.to_string())
}
fn publish_settings(
    app: &tauri::AppHandle,
    projection: SettingsProjection,
    effect: Option<SettingsUiEffect>,
) -> Result<(), String> {
    app.emit(
        "yeet://ui-settings-event",
        SettingsMessage {
            version: 1,
            kind: "ui_settings",
            request_id: None,
            projection,
            effect,
        },
    )
    .map_err(|error| error.to_string())
}
#[tauri::command]
async fn send_ui_toolbar_action(
    action: yeet::shared_ui::toolbar::ToolbarAction,
    host: tauri::State<'_, CoreHost>,
) -> Result<(), String> {
    let (reply, response) = mpsc::channel();
    host.0
        .send(Control::ToolbarAction(action, reply))
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || response.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn send_ui_settings_action(
    action: SettingsAction,
    host: tauri::State<'_, CoreHost>,
) -> Result<(), String> {
    let (reply, response) = mpsc::channel();
    host.0
        .send(Control::SettingsAction(action, reply))
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || response.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
}

fn publish_application(
    app: &tauri::AppHandle,
    projection: ApplicationProjection,
    effect: Option<AgentUiEffect>,
) -> Result<(), String> {
    publish_home(app, &projection)?;
    app.emit(
        "yeet://ui-event",
        UiMessage {
            version: 1,
            kind: "ui_state",
            request_id: None,
            application: projection.application,
            projection: UiProjection {
                toolbar: projection.toolbar,
                ui_revision: projection.ui_revision,
                state: projection.state,
                view: projection.view,
            },
        },
    )
    .map_err(|error| error.to_string())?;
    publish_agents(app, projection.agents, effect)?;
    publish_conversation(app, projection.conversation, None)?;
    publish_composer(app, projection.composer, None)?;
    publish_settings(app, projection.settings, None)
}

fn publish_agents(
    app: &tauri::AppHandle,
    projection: AgentProjection,
    effect: Option<AgentUiEffect>,
) -> Result<(), String> {
    app.emit(
        "yeet://ui-agents-event",
        AgentsMessage {
            version: 1,
            kind: "ui_agents",
            request_id: None,
            projection,
            effect,
        },
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn agents_projection(host: tauri::State<'_, CoreHost>) -> Result<AgentProjection, String> {
    let (reply, response) = mpsc::channel();
    host.0
        .send(Control::AgentsProjection(reply))
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || response.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn send_ui_agent_action(
    action: AgentAction,
    host: tauri::State<'_, CoreHost>,
) -> Result<(), String> {
    let (reply, response) = mpsc::channel();
    host.0
        .send(Control::AgentAction(action, reply))
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || response.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
}

fn configure_composer_host(
    ui: &mut ApplicationSession,
    state: &HarnessState,
    workspace: Option<String>,
) -> bool {
    use yeet::shared_ui::composer::ComposerDestination::*;
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

// One worker owns the harness, serializing commands and workspace changes.
// No frontend-specific state or domain logic belongs in this adapter.
fn run_core(app: tauri::AppHandle, controls: mpsc::Receiver<Control>) {
    let mut harness: Option<Harness> = None;
    let mut conversation = None;
    let mut theme_cache = yeet::theme::ThemeProjectionCache::default();
    let mut ui = ApplicationSession::default();
    let mut git = yeet::harness::resources::GitRefresh::default();
    let mut recent = RecentViews::default();
    loop {
        match controls.recv_timeout(Duration::from_millis(16)) {
            Ok(Control::Connect(workspace, reply)) => {
                let result = Harness::embedded(&workspace)
                    .map(|next| {
                        if harness
                            .as_ref()
                            .is_none_or(|current| current.workspace() != next.workspace())
                        {
                            recent = RecentViews::default();
                        }
                        theme_cache.invalidate();
                        let mut connection = CoreConnection {
                            workspace: next.workspace().to_string_lossy().into_owned(),
                            state: next.latest_state().cloned(),
                        };
                        if let Some(state) = connection.state.as_mut() {
                            theme_cache.hydrate(&mut state.runtime_settings);
                        }
                        conversation = connection
                            .state
                            .as_ref()
                            .and_then(|state| state.conversation.clone());
                        // Preserve the current connection when constructing a new one fails.
                        if let Some(state) = connection.state.as_ref() {
                            git.refresh_git(next.workspace(), false);
                            ui.refresh(state);
                            configure_composer_host(
                                &mut ui,
                                state,
                                Some(connection.workspace.clone()),
                            );
                            ui.update_home_content(WorkspaceContent::collect(
                                state,
                                &recent,
                                &git.snapshot,
                            ));
                            let _ = publish_application(&app, ui.projection(), None);
                        }
                        harness = Some(next);
                        connection
                    })
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
            }
            Ok(Control::Command(command, reply)) => {
                let result = match harness.as_mut() {
                    Some(core) => {
                        let new_session = matches!(command, HarnessCommand::NewSession);
                        let theme_command = command.clone();
                        let result = core.send(command).map_err(|error| error.to_string());
                        if result.is_ok() {
                            theme_cache.invalidate_for_command(&theme_command);
                        }
                        if result.is_ok() && new_session {
                            let state = core.latest_state().cloned().unwrap_or_default();
                            ui.observe_composer_new_session(&state);
                            let _ = publish_composer(&app, ui.composer_projection(), None);
                        }
                        result
                    }
                    None => Err("Connect a local workspace first".into()),
                };
                let _ = reply.send(result);
            }
            Ok(Control::ApplicationProjection(reply)) => {
                let _ = reply.send(Ok(ui.projection()));
            }
            Ok(Control::HomeProjection(reply)) => {
                let application = ui.projection();
                let projection = HomeProjection {
                    home_revision: application.home_revision,
                    view: application.home,
                };
                let _ = reply.send(Ok(projection));
            }
            Ok(Control::UiProjection(reply)) => {
                let _ = reply.send(Ok(ui.shell_projection()));
            }
            Ok(Control::ShellAction(action, reply)) => {
                let state = harness
                    .as_ref()
                    .and_then(|core| core.latest_state())
                    .cloned()
                    .unwrap_or_default();
                let projection = ui.apply_shell(action, &state);
                let _ = reply.send(publish_application(&app, projection, None));
            }
            Ok(Control::HomeAction(action, reply)) => {
                let result = (|| -> Result<(), String> {
                    let core = harness.as_mut().ok_or("Connect a local workspace first")?;
                    let state = core
                        .latest_state()
                        .cloned()
                        .ok_or("Local workspace state unavailable")?;
                    let prepared = ui.prepare_home(action, &state);
                    if let Some(command) = prepared.effect.command.clone() {
                        let theme_command = command.clone();
                        core.send(command).map_err(|error| error.to_string())?;
                        theme_cache.invalidate_for_command(&theme_command);
                    }
                    if let Some(open) = prepared.effect.open.clone() {
                        publish_home_effect(&app, Some(open.clone()))?;
                        if let Some(title) = ui.home_resource_title(&open) {
                            recent.visit(open, title);
                        }
                    }
                    let (mut projection, effect) = ui.commit_home(prepared, &state);
                    if let Some(updated) = ui.update_home_content(WorkspaceContent::collect(
                        &state,
                        &recent,
                        &git.snapshot,
                    )) {
                        projection = updated;
                    }
                    if matches!(effect.command, Some(HarnessCommand::NewSession)) {
                        publish_composer(&app, projection.composer.clone(), None)?;
                    }
                    publish_application(&app, projection, None)
                })();
                let _ = reply.send(result);
            }
            Ok(Control::AgentsProjection(reply)) => {
                let result = if harness.is_some() {
                    Ok(ui.agents_projection())
                } else {
                    Err("Connect a local workspace first".into())
                };
                let _ = reply.send(result);
            }
            Ok(Control::AgentAction(action, reply)) => {
                let result = (|| -> Result<(), String> {
                    let core = harness.as_mut().ok_or("Connect a local workspace first")?;
                    let state = core
                        .latest_state()
                        .cloned()
                        .ok_or("Local workspace state unavailable")?;
                    let prepared = ui.prepare_agent(action, &state);
                    if let Some(command) = prepared.effect.command.clone() {
                        let theme_command = command.clone();
                        core.send(command).map_err(|error| error.to_string())?;
                        theme_cache.invalidate_for_command(&theme_command);
                    }
                    let (projection, effect) = ui.commit_agent(prepared, &state);
                    publish_application(&app, projection, Some((&effect).into()))
                })();
                let _ = reply.send(result);
            }
            Ok(Control::ConversationAction(action, reply)) => {
                let result = (|| -> Result<(), String> {
                    let core = harness.as_mut().ok_or("Connect a local workspace first")?;
                    let mut state = core
                        .latest_state()
                        .cloned()
                        .ok_or("Local workspace state unavailable")?;
                    if state.conversation.is_none() {
                        state.conversation = conversation.clone();
                    }
                    let prepared = ui.prepare_conversation(action, &state);
                    if let Some(command) = prepared.effect.command.clone() {
                        let theme_command = command.clone();
                        core.send(command).map_err(|error| error.to_string())?;
                        theme_cache.invalidate_for_command(&theme_command);
                    }
                    let (projection, effect) = ui.commit_conversation(prepared, &state);
                    publish_conversation(&app, projection.conversation, Some((&effect).into()))
                })();
                let _ = reply.send(result);
            }
            Ok(Control::ComposerAction(action, reply)) => {
                let result = (|| -> Result<(), String> {
                    let core = harness.as_mut().ok_or("Connect a local workspace first")?;
                    let mut state = core
                        .latest_state()
                        .cloned()
                        .ok_or("Local workspace state unavailable")?;
                    if state.conversation.is_none() {
                        state.conversation = conversation.clone();
                    }
                    let prepared = ui.prepare_composer(action, &state);
                    let new_session =
                        matches!(prepared.effect.command, Some(HarnessCommand::NewSession));
                    if let Some(command) = prepared.effect.command.clone() {
                        let theme_command = command.clone();
                        core.send(command).map_err(|error| error.to_string())?;
                        theme_cache.invalidate_for_command(&theme_command);
                    }
                    let (mut projection, effect) = ui.commit_composer(prepared, &state);
                    if new_session {
                        ui.observe_composer_new_session(&state);
                        projection = ui.projection();
                    }
                    publish_composer(&app, projection.composer, Some(effect.ui))
                })();
                let _ = reply.send(result);
            }
            Ok(Control::ToolbarAction(action, reply)) => {
                let result = (|| -> Result<(), String> {
                    let core = harness.as_mut().ok_or("Connect a local workspace first")?;
                    let mut state = core
                        .latest_state()
                        .cloned()
                        .ok_or("Local workspace state unavailable")?;
                    if state.conversation.is_none() {
                        state.conversation = conversation.clone();
                    }
                    let prepared = ui.prepare_toolbar(action, &state);
                    if let Some(command) = prepared.effect.command.clone() {
                        let theme_command = command.clone();
                        core.send(command).map_err(|error| error.to_string())?;
                        theme_cache.invalidate_for_command(&theme_command);
                    }
                    let (projection, _effect) = ui.commit_toolbar(prepared, &state);
                    publish_application(&app, projection, None)
                })();
                let _ = reply.send(result);
            }
            Ok(Control::SettingsAction(action, reply)) => {
                let result = (|| -> Result<(), String> {
                    let core = harness.as_mut().ok_or("Connect a local workspace first")?;
                    let mut state = core
                        .latest_state()
                        .cloned()
                        .ok_or("Local workspace state unavailable")?;
                    if state.conversation.is_none() {
                        state.conversation = conversation.clone();
                    }
                    let prepared = ui.prepare_settings(action, &state);
                    if let Some(command) = prepared.effect.command.clone() {
                        let theme_command = command.clone();
                        core.send(command).map_err(|error| error.to_string())?;
                        theme_cache.invalidate_for_command(&theme_command);
                    }
                    let (projection, effect) = ui.commit_settings(prepared, &state);
                    publish_settings(&app, projection.settings, Some(effect.ui))
                })();
                let _ = reply.send(result);
            }
            Ok(Control::Disconnect(reply)) => {
                harness = None;
                conversation = None;
                git = yeet::harness::resources::GitRefresh::default();
                recent = RecentViews::default();
                ui = ApplicationSession::default();
                let _ = reply.send(());
            }
            Ok(Control::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Some(core) = harness.as_mut() {
            // Bound draining so a busy stream never starves controls.
            for _ in 0..128 {
                let Some(mut event) = core.try_recv() else {
                    break;
                };
                // Harness updates omit unchanged conversation history. Normalize
                // that optimization at the transport boundary into full snapshots.
                if let Some(state) = event.state.as_mut() {
                    theme_cache.hydrate(&mut state.runtime_settings);
                    if state.conversation.is_none() {
                        state.conversation = conversation.clone();
                    } else {
                        conversation = state.conversation.clone();
                    }
                }
                let projection = event.state.as_ref().and_then(|state| {
                    let refreshed = ui.refresh(state).is_some();
                    let configured = configure_composer_host(&mut ui, state, None);
                    (refreshed || configured).then(|| ui.projection())
                });
                if let Ok(mut payload) = serde_json::to_value(event) {
                    payload["workspace"] =
                        serde_json::Value::String(core.workspace().to_string_lossy().into_owned());
                    let _ = app.emit("yeet://core-event", payload);
                }
                if let Some(projection) = projection {
                    let _ = publish_application(&app, projection, None);
                }
            }
            git.refresh_git(core.workspace(), false);
            if let Some(state) = core.latest_state() {
                let content = WorkspaceContent::collect(state, &recent, &git.snapshot);
                if let Some(projection) = ui.update_home_content(content) {
                    let _ = publish_application(&app, projection, None);
                }
            }
        }
    }
}

#[tauri::command]
async fn connect_core(
    workspace: String,
    host: tauri::State<'_, CoreHost>,
) -> Result<CoreConnection, String> {
    let (reply, response) = mpsc::channel();
    host.0
        .send(Control::Connect(workspace, reply))
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || response.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn send_core_command(
    command: HarnessCommand,
    host: tauri::State<'_, CoreHost>,
) -> Result<(), String> {
    if matches!(command, HarnessCommand::Shutdown) {
        return Err("The desktop host owns the core lifecycle".into());
    }
    let (reply, response) = mpsc::channel();
    host.0
        .send(Control::Command(command, reply))
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || response.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn disconnect_core(host: tauri::State<'_, CoreHost>) -> Result<(), String> {
    let (reply, response) = mpsc::channel();
    host.0
        .send(Control::Disconnect(reply))
        .map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || response.recv())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

fn main() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let bundled = app.path().resource_dir()?.join("runtime");
            let runtime = if bundled.join("dist/bridge.js").is_file() {
                bundled
            } else {
                // Cargo debug binaries use the same staged assets as packaging.
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime")
            };
            yeet::core::set_host_runtime_directory(runtime)?;
            let (commands, controls) = mpsc::channel();
            app.manage(CoreHost(commands));
            let handle = app.handle().clone();
            std::thread::Builder::new()
                .name("yeet-desktop-core".into())
                .spawn(move || run_core(handle, controls))?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect_core,
            send_core_command,
            disconnect_core,
            application_projection,
            home_projection,
            ui_projection,
            send_ui_action,
            send_ui_home_action,
            agents_projection,
            send_ui_agent_action,
            send_ui_conversation_action,
            send_ui_composer_action,
            send_ui_toolbar_action,
            send_ui_settings_action
        ])
        .build(tauri::generate_context!())
        .expect("initialize Yeet desktop");
    app.run(|handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            let _ = handle.state::<CoreHost>().0.send(Control::Stop);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use yeet::shared_ui::shell::{Placement, Surface, ViewKind};

    #[test]
    fn desktop_home_event_carries_shared_inventory_and_revision() {
        use yeet::shared_ui::home::{ResourceItem, ResourceKind, ResourceTarget};
        let mut session = ApplicationSession::default();
        let mut content = WorkspaceContent::default();
        content.sessions.push(ResourceItem::new(
            ResourceTarget::Session("session-1".into()),
            ResourceKind::Session,
            "Recent work",
        ));
        let projection = session.update_home_content(content).expect("home changed");
        let wire = serde_json::to_value(HomeMessage {
            version: 1,
            kind: "ui_home",
            request_id: None,
            home_revision: projection.home_revision,
            view: projection.home,
        })
        .unwrap();
        assert_eq!(wire["type"], "ui_home");
        assert_eq!(wire["home_revision"], 1);
        assert_eq!(wire["view"]["activity"][1]["value"]["title"], "Recent work");
        assert!(wire.get("sequence").is_none());
    }

    #[test]
    fn desktop_home_revision_tracks_home_changes_only_and_open_effect_is_typed() {
        use yeet::shared_ui::home::ResourceTarget;

        let mut session = ApplicationSession::default();
        let initial = session.projection().home_revision;
        session.apply_shell(ShellAction::OpenModels, &HarnessState::default());
        let after_shell_change = session.projection().home_revision;
        assert_eq!(after_shell_change, initial);

        let prepared = session.prepare_home(
            HomeAction::Open(ResourceTarget::Status),
            &HarnessState::default(),
        );
        assert_eq!(prepared.effect.open, Some(ResourceTarget::Status));
        let wire = serde_json::to_value(HomeEffectMessage {
            version: 1,
            kind: "ui_home_effect",
            request_id: None,
            open: prepared.effect.open,
        })
        .unwrap();
        assert_eq!(wire["type"], "ui_home_effect");
        assert_eq!(wire["open"]["type"], "status");
    }

    #[test]
    fn desktop_conversation_delivers_safe_edit_effect_and_shared_controls() {
        let mut state = HarnessState::default();
        state.conversation = Some(
            serde_json::from_value(serde_json::json!([
                {"id":"user", "kind":{"type":"user","content":"edit this"}}
            ]))
            .unwrap(),
        );
        let mut session = ApplicationSession::new(&state);
        let prepared =
            session.prepare_conversation(ConversationAction::Edit("user".into()), &state);
        let (projection, effect) = session.commit_conversation(prepared, &state);
        let wire = serde_json::to_value(ConversationMessage {
            version: 1,
            kind: "ui_conversation",
            request_id: None,
            projection: projection.conversation,
            effect: Some((&effect).into()),
        })
        .unwrap();
        assert_eq!(wire["effect"]["edit_draft"], "edit this");
        assert!(wire["effect"].get("command").is_none());
        assert_eq!(
            wire["view"]["items"][0]["controls"][1]["action"]["type"],
            "edit"
        );
        assert_eq!(wire["conversation_revision"], 1);
        assert!(wire.get("sequence").is_none());
    }

    #[test]
    fn desktop_agents_event_matches_remote_projection_and_safe_effects() {
        let state = HarnessState::default();
        let mut session = ApplicationSession::new(&state);
        let prepared = session.prepare_agent(AgentAction::CreateGroup, &state);
        let (projection, effect) = session.commit_agent(prepared, &state);
        let wire = serde_json::to_value(AgentsMessage {
            version: 1,
            kind: "ui_agents",
            request_id: None,
            projection: projection.agents,
            effect: Some((&effect).into()),
        })
        .unwrap();
        assert_eq!(wire["type"], "ui_agents");
        assert_eq!(wire["agents_revision"], 1);
        assert_eq!(wire["view"]["state"]["creating_group"], true);
        assert!(wire.get("projection").is_none());
        assert!(wire["effect"].get("command").is_none());
    }

    #[test]
    fn desktop_projects_shared_actions_in_remote_message_shape() {
        let mut first = ApplicationSession::default();
        let second = ApplicationSession::default();
        assert_eq!(first.shell_projection().ui_revision, 0);
        first.apply_shell(ShellAction::OpenModels, &HarnessState::default());
        let message = UiMessage {
            version: 1,
            kind: "ui_state",
            request_id: None,
            application: first.application_view(),
            projection: first.shell_projection(),
        };
        assert_eq!(message.projection.ui_revision, 1);
        assert_eq!(message.projection.view.dismiss, Some(Surface::Models));
        assert_eq!(
            message.projection.view.placement(ViewKind::Models),
            Placement::Overlay
        );
        assert!(
            !second.shell_projection().state.models,
            "UI intent is local to its host"
        );
        let wire = serde_json::to_value(message).unwrap();
        assert_eq!(wire["type"], "ui_state");
        assert_eq!(wire["version"], 1);
        assert_eq!(wire["ui_revision"], 1);
        assert_eq!(wire["application"]["content"], "home");
        assert!(wire["state"]["models"].as_bool().unwrap());
        assert!(wire["view"]["views"].is_array());
        assert!(wire.get("projection").is_none());
        assert_eq!(
            first
                .apply_shell(ShellAction::Dismiss, &HarnessState::default())
                .view
                .dismiss,
            None
        );
    }
}
