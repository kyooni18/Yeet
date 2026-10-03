#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{sync::mpsc, time::Duration};
use tauri::{Emitter, Manager};
use yeet::harness::{Harness, HarnessCommand, HarnessState};

#[derive(serde::Serialize)]
struct CoreConnection {
    workspace: String,
    state: Option<HarnessState>,
}

enum Control {
    Connect(String, mpsc::Sender<Result<CoreConnection, String>>),
    Command(HarnessCommand, mpsc::Sender<Result<(), String>>),
    Disconnect(mpsc::Sender<()>),
    Stop,
}

struct CoreHost(mpsc::Sender<Control>);

// One worker owns the harness, serializing commands and workspace changes.
// No frontend-specific state or domain logic belongs in this adapter.
fn run_core(app: tauri::AppHandle, controls: mpsc::Receiver<Control>) {
    let mut harness: Option<Harness> = None;
    let mut conversation = None;
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
                if let Ok(mut payload) = serde_json::to_value(event) {
                    payload["workspace"] =
                        serde_json::Value::String(core.workspace().to_string_lossy().into_owned());
                    let _ = app.emit("yeet://core-event", payload);
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
            disconnect_core
        ])
        .build(tauri::generate_context!())
        .expect("initialize Yeet desktop");
    app.run(|handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            let _ = handle.state::<CoreHost>().0.send(Control::Stop);
        }
    });
}
