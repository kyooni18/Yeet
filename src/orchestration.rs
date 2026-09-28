//! Runtime ownership primitives shared by single- and multi-agent execution.
//!
//! The backend currently admits one top-level run at a time, but cancellation
//! and lifecycle ownership are keyed by run ID so additional worker runs can be
//! introduced without returning to a single global cancellation slot.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::Result;

use crate::{
    agent::AgentCoordinator,
    permission::PermissionBroker,
    project_settings::ProjectSettingsStore,
    session_store::SessionStore,
    tools::{BridgeHandle, ToolRegistry},
    workers::WorkerRegistry,
};

#[derive(Clone)]
pub(crate) struct AgentRuntimeFactory {
    bridge: BridgeHandle,
    workspace_root: PathBuf,
    workers: WorkerRegistry,
    permission: PermissionBroker,
    project_settings: ProjectSettingsStore,
    project_identity: String,
    store: SessionStore,
}

impl AgentRuntimeFactory {
    pub(crate) fn new(
        bridge: BridgeHandle,
        workspace_root: PathBuf,
        workers: WorkerRegistry,
        permission: PermissionBroker,
        project_settings: ProjectSettingsStore,
        project_identity: String,
        store: SessionStore,
    ) -> Self {
        Self {
            bridge,
            workspace_root,
            workers,
            permission,
            project_settings,
            project_identity,
            store,
        }
    }

    /// Builds a fresh coordinator with independent history and tool state.
    ///
    /// Project-backed service settings are loaded for each build so workers
    /// created later do not inherit stale Foundation/Web configuration.
    pub(crate) fn build(&self, active_session_id: Option<String>) -> Result<AgentCoordinator> {
        let project = self.project_settings.load()?;
        self.bridge.set_openai_flex(project.openai_flex);

        let mut registry = ToolRegistry::new_with_bridge_handle(
            self.bridge.clone(),
            self.workspace_root.clone(),
            self.workers.clone(),
            self.permission.clone(),
        )?;
        registry.configure_foundation_memory(
            project.foundation_memory.enabled,
            project.foundation_memory.backend,
            project.foundation_memory.server,
            self.project_identity.clone(),
        );
        registry.configure_web_backend(project.web.backend, project.web.server);

        let mut coordinator = AgentCoordinator::new(self.bridge.clone(), registry);
        if let Some(session_id) = active_session_id.as_ref() {
            coordinator.set_protected_write_paths([self.store.directory.join(session_id)]);
        }
        coordinator.set_session_runtime(self.store.clone(), active_session_id);
        Ok(coordinator)
    }
}

#[derive(Clone, Default)]
pub(crate) struct RunManager {
    inner: Arc<Mutex<RunManagerState>>,
}

#[derive(Default)]
struct RunManagerState {
    active: HashMap<String, Arc<AtomicBool>>,
}

impl RunManager {
    pub(crate) fn register(&self, run_id: impl Into<String>, cancel: Arc<AtomicBool>) {
        self.lock().active.insert(run_id.into(), cancel);
    }

    pub(crate) fn remove_matching(&self, run_id: &str, completed: &Arc<AtomicBool>) {
        let mut state = self.lock();
        if state
            .active
            .get(run_id)
            .is_some_and(|current| Arc::ptr_eq(current, completed))
        {
            state.active.remove(run_id);
        }
    }

    pub(crate) fn cancel_all(&self) -> usize {
        let state = self.lock();
        for cancel in state.active.values() {
            cancel.store(true, Ordering::Release);
        }
        state.active.len()
    }

    /// Detaches all runtime ownership without changing cancellation flags.
    ///
    /// Session replacement historically invalidates the live transcript first
    /// and lets the old worker observe that its run ID no longer owns the
    /// session. Preserve that behavior while making the ownership registry
    /// capable of holding multiple runs.
    pub(crate) fn detach_all(&self) {
        self.lock().active.clear();
    }

    fn lock(&self) -> MutexGuard<'_, RunManagerState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_all_marks_every_registered_run() {
        let manager = RunManager::default();
        let first = Arc::new(AtomicBool::new(false));
        let second = Arc::new(AtomicBool::new(false));
        manager.register("first", first.clone());
        manager.register("second", second.clone());

        assert_eq!(manager.cancel_all(), 2);
        assert!(first.load(Ordering::Acquire));
        assert!(second.load(Ordering::Acquire));
    }

    #[test]
    fn late_completion_cannot_remove_replacement_run() {
        let manager = RunManager::default();
        let original = Arc::new(AtomicBool::new(false));
        let replacement = Arc::new(AtomicBool::new(false));
        manager.register("run", original.clone());
        manager.register("run", replacement.clone());

        manager.remove_matching("run", &original);
        assert_eq!(manager.cancel_all(), 1);
        assert!(replacement.load(Ordering::Acquire));
        assert!(!original.load(Ordering::Acquire));
    }
}
