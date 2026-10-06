//! Cross-runtime plumbing: waking the daemon on published state, sharing the
//! workspace session catalog, and surviving poisoned locks.
//!
//! A background daemon hosts several `HarnessService` runtimes on one thread.
//! These helpers keep one runtime's failure or stale view from leaking into
//! the others.

use std::sync::{Mutex, MutexGuard, PoisonError};

use super::*;

/// Locks a mutex even if a panicking thread poisoned it. Backend state stays
/// structurally valid across a worker panic, and the daemon polls runtime
/// state every tick, so treating poison as fatal would turn one failed turn
/// into a crash of every session in the workspace.
pub(super) trait LockExt<T> {
    fn lock_or_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn lock_or_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The workspace-wide session list shared by every runtime's clients.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SessionCatalog {
    pub(crate) sessions: Vec<SessionSummary>,
    pub(crate) workspaces: Vec<WorkspaceSummary>,
    pub(crate) groups: Vec<WorkspaceSessionGroup>,
}

impl SessionCatalog {
    pub(crate) fn of(state: &HarnessState) -> Self {
        Self {
            sessions: state.saved_sessions.clone(),
            workspaces: state.known_workspaces.clone(),
            groups: state.workspace_session_groups.clone(),
        }
    }

    pub(crate) fn matches(&self, state: &HarnessState) -> bool {
        self.sessions == state.saved_sessions
            && self.workspaces == state.known_workspaces
            && self.groups == state.workspace_session_groups
    }

    pub(crate) fn read_from_store(store: &SessionStore, workspace: &Path) -> Result<Self> {
        let sessions = store.list(workspace)?;
        let (workspaces, groups) = store.list_workspace_catalog(workspace)?;
        Ok(Self {
            sessions,
            workspaces,
            groups,
        })
    }
}

pub(super) fn apply_session_catalog_locked(state: &mut SharedSession, catalog: &SessionCatalog) {
    state.state.saved_sessions = catalog.sessions.clone();
    state.state.known_workspaces = catalog.workspaces.clone();
    state.state.workspace_session_groups = catalog.groups.clone();
}

/// Backend event channel that also wakes the owning daemon loop, so published
/// state reaches clients immediately instead of on the next polling tick.
#[derive(Clone)]
pub(crate) struct EventSender {
    tx: mpsc::Sender<ServiceEvent>,
    wake: Option<Wake>,
}

impl EventSender {
    pub(super) fn new(tx: mpsc::Sender<ServiceEvent>, wake: Option<Wake>) -> Self {
        Self { tx, wake }
    }

    // Mirrors `mpsc::Sender::send` so existing call sites are unchanged.
    #[allow(clippy::result_large_err)]
    pub(crate) fn send(&self, event: ServiceEvent) -> Result<(), mpsc::SendError<ServiceEvent>> {
        let result = self.tx.send(event);
        if let Some(wake) = &self.wake {
            wake.notify();
        }
        result
    }
}

/// Daemon-facing view of a runtime. The daemon polls these every tick, so
/// they only read shared state and never block on the coordinator.
impl HarnessService {
    pub(crate) fn state_snapshot(&self) -> HarnessState {
        self.shared.lock_or_recover().state.clone()
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed
    }

    /// Adopts a session catalog another runtime in this workspace just read
    /// from disk, avoiding a redundant scan on the daemon thread.
    pub(crate) fn adopt_session_catalog(&self, catalog: &SessionCatalog) {
        {
            let mut shared = self.shared.lock_or_recover();
            if catalog.matches(&shared.state) {
                return;
            }
            shared.state.saved_sessions = catalog.sessions.clone();
            shared.state.known_workspaces = catalog.workspaces.clone();
            shared.state.workspace_session_groups = catalog.groups.clone();
        }
        self.publish_state();
    }
}
