//! Client-to-runtime routing and state fan-out for the background daemon.

use std::{path::Path, sync::mpsc, thread};

use anyhow::{Result, anyhow, bail};

use super::{
    ClientConnection, MAX_PENDING_ISOLATIONS, RuntimeIsolationEvent, SessionRuntime, Wake,
    allocate_runtime_id, broadcast_runtime_envelope, send_client_error, spawn_runtime,
    state_with_extension_commands,
};
use crate::{backend::SessionCatalog, extensions::ExtensionHost, model::HarnessEvent};

/// An isolated runtime being started off the daemon thread, with every client
/// waiting for it. Clients loading the same session join one startup instead
/// of each creating a runtime for that session.
pub(super) struct PendingIsolation {
    pub(super) runtime_id: u64,
    pub(super) session_id: Option<String>,
    pub(super) waiters: Vec<(u64, u64)>,
}

/// Sends a coalesced runtime state to its clients and tracks the workspace
/// session catalog it carries.
///
/// A runtime whose catalog changed since its previous state just re-read it
/// from disk, so it is the newest and becomes the workspace catalog. A runtime
/// whose catalog did not change but differs from the workspace catalog is
/// merely stale; the caller pushes the newer catalog into it.
pub(super) fn publish_runtime_state(
    clients: &mut Vec<ClientConnection>,
    extensions: &ExtensionHost,
    runtime: &mut SessionRuntime,
    mut envelope: HarnessEvent,
    session_catalog: &mut Option<SessionCatalog>,
    catalog_changed: &mut bool,
) {
    if let Some(state) = envelope.state.as_mut() {
        state.session_activity = runtime.service.session_activity().clone();
        state.extension_commands = extensions.command_items();
        extensions.publish_state(state);
        let refreshed = runtime
            .catalog
            .as_ref()
            .is_none_or(|previous| !previous.matches(state));
        if refreshed {
            let catalog = SessionCatalog::of(state);
            if session_catalog.as_ref() != Some(&catalog) {
                *session_catalog = Some(catalog.clone());
                *catalog_changed = true;
            }
            runtime.catalog = Some(catalog);
        }
    }
    broadcast_runtime_envelope(clients, runtime.id, &envelope);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn start_isolated_runtime(
    pending_isolations: &mut Vec<PendingIsolation>,
    next_runtime_id: &mut u64,
    isolation_tx: &mpsc::Sender<RuntimeIsolationEvent>,
    workspace: &Path,
    scope: Option<&str>,
    wake: &Wake,
    session_id: Option<String>,
    waiter: (u64, u64),
) -> Result<()> {
    if pending_isolations.len() >= MAX_PENDING_ISOLATIONS {
        bail!("too many background session runtimes are starting; retry in a moment");
    }
    let runtime_id = allocate_runtime_id(next_runtime_id);
    pending_isolations.push(PendingIsolation {
        runtime_id,
        session_id,
        waiters: vec![waiter],
    });
    let isolation_tx = isolation_tx.clone();
    let workspace = workspace.to_path_buf();
    let scope = scope.map(str::to_owned);
    let wake = wake.clone();
    thread::spawn(move || {
        let result = spawn_runtime(&workspace, runtime_id, scope.as_deref(), wake.clone());
        let _ = isolation_tx.send(RuntimeIsolationEvent { runtime_id, result });
        wake.notify();
    });
    Ok(())
}

pub(super) fn live_session_runtime_id(
    runtimes: &[SessionRuntime],
    session_id: &str,
) -> Option<u64> {
    runtimes
        .iter()
        .find(|runtime| {
            !runtime.service.is_closed()
                && runtime.service.current_session_id().as_deref() == Some(session_id)
        })
        .map(|runtime| runtime.id)
}

/// Moves a client onto a runtime and sends it that runtime's full snapshot.
pub(super) fn route_client_to_runtime(
    clients: &mut Vec<ClientConnection>,
    runtimes: &mut [SessionRuntime],
    extensions: &ExtensionHost,
    client_id: u64,
    runtime_id: u64,
) {
    let Some(client_index) = clients.iter().position(|client| client.id == client_id) else {
        return;
    };
    let Some(runtime) = runtimes.iter_mut().find(|runtime| runtime.id == runtime_id) else {
        send_client_error_to(
            clients,
            client_id,
            anyhow!("background session runtime is no longer available"),
        );
        return;
    };
    runtime.idle_since = None;
    clients[client_index].runtime_id = runtime_id;
    let envelope = HarnessEvent {
        kind: "state".into(),
        state: Some(state_with_extension_commands(
            runtime.service.state_snapshot(),
            extensions,
        )),
        message: None,
    };
    if clients[client_index].writer.send(&envelope).is_err() {
        clients.remove(client_index);
    }
}

pub(super) fn send_client_error_to(
    clients: &mut Vec<ClientConnection>,
    client_id: u64,
    error: anyhow::Error,
) {
    if let Some(client_index) = clients.iter().position(|client| client.id == client_id) {
        send_client_error(clients, client_index, error);
    }
}
