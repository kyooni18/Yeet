#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    sync::{Mutex, mpsc},
    time::Duration,
};
use tauri::{Emitter, Manager};
use yeet::harness::{Harness, HarnessCommand, HarnessState};
use yeet::shared_ui::agents::{AgentAction, AgentUiEffect};
use yeet::shared_ui::agents_session::{AgentProjection, AgentSession};
use yeet::shared_ui::shell::{ShellAction, ShellState, ShellView};

#[derive(serde::Serialize)]
struct CoreConnection {
    workspace: String,
    state: Option<HarnessState>,
}

enum Control {
    Connect(String, mpsc::Sender<Result<CoreConnection, String>>),
    Command(HarnessCommand, mpsc::Sender<Result<(), String>>),
    AgentsProjection(mpsc::Sender<Result<AgentProjection, String>>),
    AgentAction(AgentAction, mpsc::Sender<Result<(), String>>),
    Disconnect(mpsc::Sender<()>),
    Stop,
}

struct CoreHost(mpsc::Sender<Control>);

// A host owns local UI intent independently of the workspace agent runtime.
// Application transitions and composition are authored by the shared UI layer.
#[derive(Default)]
struct DesktopUi {
    state: ShellState,
    revision: u64,
}
struct UiHost(Mutex<DesktopUi>);

#[derive(Clone, serde::Serialize)]
struct UiProjection {
    ui_revision: u64,
    state: ShellState,
    view: ShellView,
}

#[derive(Clone, serde::Serialize)]
struct UiMessage {
    version: u16,
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: Option<String>,
    #[serde(flatten)]
    projection: UiProjection,
}

impl DesktopUi {
    fn projection(&self) -> UiProjection {
        UiProjection {
            ui_revision: self.revision,
            state: self.state.clone(),
            view: self.state.view(),
        }
    }

    fn apply(&mut self, action: ShellAction) -> UiMessage {
        self.state.apply(action);
        self.revision = self.revision.saturating_add(1);
        UiMessage {
            version: 1,
            kind: "ui_state",
            request_id: None,
            projection: self.projection(),
        }
    }
}

#[tauri::command]
fn ui_projection(host: tauri::State<'_, UiHost>) -> Result<UiProjection, String> {
    Ok(host
        .0
        .lock()
        .map_err(|error| error.to_string())?
        .projection())
}

#[tauri::command]
fn send_ui_action(
    action: ShellAction,
    host: tauri::State<'_, UiHost>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // Keep publication ordered with application of actions from this host.
    let mut ui = host.0.lock().map_err(|error| error.to_string())?;
    app.emit("yeet://ui-event", ui.apply(action))
        .map_err(|error| error.to_string())
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

// One worker owns the harness, serializing commands and workspace changes.
// No frontend-specific state or domain logic belongs in this adapter.
fn run_core(app: tauri::AppHandle, controls: mpsc::Receiver<Control>) {
    let mut harness: Option<Harness> = None;
    let mut conversation = None;
    let mut agents = AgentSession::new(&HarnessState::default());
    loop {
        match controls.recv_timeout(Duration::from_millis(16)) {
            Ok(Control::Connect(workspace, reply)) => {
                let result = Harness::embedded(&workspace)
                    .map(|next| {
                        let connection = CoreConnection {
                            workspace: next.workspace().to_string_lossy().into_owned(),
                            state: next.latest_state().cloned(),
                        };
                        conversation = connection
                            .state
                            .as_ref()
                            .and_then(|state| state.conversation.clone());
                        // Preserve the current connection when constructing a new one fails.
                        if let Some(state) = connection.state.as_ref()
                            && let Some(projection) = agents.refresh(state)
                        {
                            let _ = publish_agents(&app, projection, None);
                        }
                        harness = Some(next);
                        connection
                    })
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
            }
            Ok(Control::Command(command, reply)) => {
                let result = match harness.as_mut() {
                    Some(core) => core.send(command).map_err(|error| error.to_string()),
                    None => Err("Connect a local workspace first".into()),
                };
                let _ = reply.send(result);
            }
            Ok(Control::AgentsProjection(reply)) => {
                let result = if harness.is_some() {
                    Ok(agents.projection())
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
                    let prepared = agents.prepare(action, &state);
                    if let Some(command) = prepared.effect.command.clone() {
                        core.send(command).map_err(|error| error.to_string())?;
                    }
                    let (projection, effect) = agents.commit(prepared, &state);
                    publish_agents(&app, projection, Some((&effect).into()))
                })();
                let _ = reply.send(result);
            }
            Ok(Control::Disconnect(reply)) => {
                harness = None;
                conversation = None;
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
                    if state.conversation.is_none() {
                        state.conversation = conversation.clone();
                    } else {
                        conversation = state.conversation.clone();
                    }
                }
                let projection = event.state.as_ref().and_then(|state| agents.refresh(state));
                if let Ok(mut payload) = serde_json::to_value(event) {
                    payload["workspace"] =
                        serde_json::Value::String(core.workspace().to_string_lossy().into_owned());
                    let _ = app.emit("yeet://core-event", payload);
                }
                if let Some(projection) = projection {
                    let _ = publish_agents(&app, projection, None);
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
            app.manage(UiHost(Mutex::new(DesktopUi::default())));
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
            ui_projection,
            send_ui_action,
            agents_projection,
            send_ui_agent_action
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
    fn desktop_agents_event_matches_remote_projection_and_safe_effects() {
        let state = HarnessState::default();
        let mut session = AgentSession::new(&state);
        let prepared = session.prepare(AgentAction::CreateGroup, &state);
        let (projection, effect) = session.commit(prepared, &state);
        let wire = serde_json::to_value(AgentsMessage {
            version: 1,
            kind: "ui_agents",
            request_id: None,
            projection,
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
        let mut first = DesktopUi::default();
        let second = DesktopUi::default();
        assert_eq!(first.projection().ui_revision, 0);
        let message = first.apply(ShellAction::OpenModels);
        assert_eq!(message.projection.ui_revision, 1);
        assert_eq!(message.projection.view.dismiss, Some(Surface::Models));
        assert_eq!(
            message.projection.view.placement(ViewKind::Models),
            Placement::Overlay
        );
        assert!(
            !second.projection().state.models,
            "UI intent is local to its host"
        );
        let wire = serde_json::to_value(message).unwrap();
        assert_eq!(wire["type"], "ui_state");
        assert_eq!(wire["version"], 1);
        assert_eq!(wire["ui_revision"], 1);
        assert!(wire["state"]["models"].as_bool().unwrap());
        assert!(wire["view"]["views"].is_array());
        assert!(wire.get("projection").is_none());
        assert_eq!(
            first.apply(ShellAction::Dismiss).projection.view.dismiss,
            None
        );
    }
}
