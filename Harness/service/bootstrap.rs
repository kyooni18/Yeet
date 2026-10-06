//! Initial live session state for a freshly spawned `HarnessService`.
//!
//! Populates the pieces a client needs before any command arrives: model
//! catalog cache, the workspace session catalog (with a single-workspace
//! fallback so startup survives an unreadable cross-workspace scan), sandbox
//! and project-service settings, and capability toggles. Session-only
//! capabilities are never inherited from project settings.

use super::*;
use crate::project_settings::ProjectSettings;

pub(super) fn initial_session(
    model: String,
    reasoning_level: String,
    workspace_root: &Path,
    config: &ConfigStore,
    store: &SessionStore,
    project: ProjectSettings,
) -> SharedSession {
    let mut session = SharedSession::new(model, reasoning_level);
    session.meta.working_directory = Some(workspace_root.display().to_string());
    session.meta.context_roots = vec![workspace_root.display().to_string()];
    if let Ok(catalog) = config.model_catalog_cache() {
        session.state.available_models = catalog.iter().map(|item| item.id.clone()).collect();
        session.state.model_catalog = catalog;
    }
    if let Ok((workspaces, session_groups)) = store.list_workspace_catalog(workspace_root) {
        let current_workspace_id = workspaces
            .iter()
            .find(|workspace| workspace.is_current)
            .map(|workspace| workspace.id.as_str());
        session.state.saved_sessions = current_workspace_id
            .and_then(|id| {
                session_groups
                    .iter()
                    .find(|group| group.workspace_id == id)
                    .map(|group| group.sessions.clone())
            })
            .unwrap_or_default();
        session.state.known_workspaces = workspaces;
        session.state.workspace_session_groups = session_groups;
    } else {
        // Keep startup resilient if the cross-workspace catalog cannot be built.
        // This fallback repeats the scan only on the exceptional path.
        session.state.saved_sessions = store.list(workspace_root).unwrap_or_default();
    }
    session.state.sandbox_settings = SandboxStore::new(workspace_root)
        .and_then(|store| store.load())
        .ok()
        .map(|policy| sandbox_settings_state(&policy));
    session.state.openai_flex = project.openai_flex;
    session.state.foundation_memory_enabled = project.foundation_memory.enabled;
    session.state.foundation_memory_backend = project.foundation_memory.backend.as_str().into();
    session.state.foundation_memory_server = project.foundation_memory.server.clone();
    session.state.web_backend = project.web.backend.as_str().into();
    session.state.web_server = project.web.server.clone();
    session.meta.attached_capabilities = project.capabilities.attached.map(|values| {
        values
            .into_iter()
            .filter(|value| !session_only_capability(value))
            .collect()
    });
    session.meta.disabled_capabilities = project
        .capabilities
        .disabled
        .into_iter()
        .filter(|value| !session_only_capability(value))
        .collect();
    session
}
