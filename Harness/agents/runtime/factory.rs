//! Builds independent `AgentCoordinator` instances with fresh history and
//! tool state for primary and delegated agents.

use std::path::PathBuf;

use anyhow::Result;

use crate::{
    agent::AgentCoordinator,
    agents::member::AgentRole,
    core::ModelInfo,
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

    /// Builds a coordinator with independent history and tool state.
    ///
    /// Project-backed service settings are loaded for each build so workers
    /// created later do not inherit stale Foundation/Web configuration.
    pub(crate) fn build(&self, active_session_id: Option<String>) -> Result<AgentCoordinator> {
        self.build_with_session(active_session_id, false)
    }

    /// Builds a nested Group coordinator with the owner's session tools but
    /// volatile model history, isolated from the Main Agent context file.
    pub(crate) fn build_group_scoped(
        &self,
        active_session_id: Option<String>,
    ) -> Result<AgentCoordinator> {
        self.build_with_session(active_session_id, true)
    }

    fn build_with_session(
        &self,
        active_session_id: Option<String>,
        volatile_group_context: bool,
    ) -> Result<AgentCoordinator> {
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
        if volatile_group_context {
            coordinator.use_volatile_group_context();
        }
        Ok(coordinator)
    }

    pub(crate) fn context_window_tokens(&self, model: &str) -> Option<u64> {
        self.bridge.context_length(model).ok().flatten()
    }

    /// Keep the requested provider/model when pricing or context metadata is
    /// absent. When the task allocation cannot cover the requested model's
    /// ordinary task envelope, choose the least costly same-provider option
    /// with enough context for the role.
    pub(crate) fn model_for_budget(
        &self,
        model: &str,
        role: AgentRole,
        cost_budget_usd: f64,
    ) -> String {
        let Some((provider, requested_id)) = model.split_once('/') else {
            return model.to_owned();
        };
        let Ok(models) = self.bridge.list_model_info(provider) else {
            return model.to_owned();
        };
        let requested = models
            .iter()
            .find(|info| info.id == requested_id || info.id == model);
        let Some(requested) = requested else {
            return model.to_owned();
        };
        let Some(requested_cost) = estimated_task_cost(requested) else {
            return model.to_owned();
        };
        if requested_cost <= cost_budget_usd {
            return model.to_owned();
        }
        let required_context = match role {
            AgentRole::Verifier => 16_000,
            AgentRole::Researcher | AgentRole::Implementer => 32_000,
        };
        let Some(candidate) = models
            .iter()
            .filter(|info| {
                info.context_length
                    .is_some_and(|length| length >= required_context)
            })
            .filter_map(|info| estimated_task_cost(info).map(|cost| (info, cost)))
            .filter(|(_, cost)| *cost <= cost_budget_usd)
            .min_by(|left, right| left.1.total_cmp(&right.1))
        else {
            return model.to_owned();
        };
        if candidate.1 < requested_cost {
            format!("{provider}/{}", candidate.0.id)
        } else {
            model.to_owned()
        }
    }
}

fn estimated_task_cost(info: &ModelInfo) -> Option<f64> {
    let pricing = info.pricing.as_ref()?;
    if pricing.get("currency")?.as_str()? != "USD"
        || pricing.get("unit")?.as_str()? != "per1MTokens"
    {
        return None;
    }
    let input = pricing.get("input")?.as_f64()?;
    let output = pricing.get("output")?.as_f64()?;
    Some((8_000.0 * input + 2_000.0 * output) / 1_000_000.0)
}
