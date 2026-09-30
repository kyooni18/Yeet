//! Builds independent `AgentCoordinator` instances with fresh history and
//! tool state for primary and delegated agents.

use std::path::PathBuf;

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
