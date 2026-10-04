use super::{AgentDecision, AgentId, AgentState, AgentStatus};
use anyhow::{Result, anyhow, bail};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, OnceLock, RwLock},
};

/// Cloneable shared service. No callbacks, provider calls or UI work happen under its lock.
#[derive(Clone, Default)]
pub struct AgentRegistry {
    states: Arc<RwLock<HashMap<AgentId, AgentState>>>,
}

/// The production registry; isolated registries can be injected in tests or embedded hosts.
pub fn global() -> &'static AgentRegistry {
    static REGISTRY: OnceLock<AgentRegistry> = OnceLock::new();
    REGISTRY.get_or_init(AgentRegistry::default)
}

impl AgentRegistry {
    pub fn register(
        &self,
        name: impl Into<String>,
        model: impl Into<String>,
        workspace: impl Into<PathBuf>,
        parent_agent: Option<AgentId>,
    ) -> Result<AgentId> {
        let mut states = self
            .states
            .write()
            .map_err(|_| anyhow!("agent registry lock poisoned"))?;
        if let Some(parent) = parent_agent
            && !states.contains_key(&parent)
        {
            bail!("unknown parent agent: {parent}");
        }
        let id = AgentId::new_v4();
        states.insert(
            id,
            AgentState {
                id,
                name: name.into(),
                model: model.into(),
                workspace: workspace.into(),
                decisions: Vec::new(),
                coworkers: Vec::new(),
                parent_agent,
                status: AgentStatus::Idle,
            },
        );
        Ok(id)
    }

    pub fn get(&self, id: AgentId) -> Result<AgentState> {
        self.states
            .read()
            .map_err(|_| anyhow!("agent registry lock poisoned"))?
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown agent: {id}"))
    }

    pub fn snapshots(&self) -> Result<Vec<AgentState>> {
        let mut snapshots: Vec<_> = self
            .states
            .read()
            .map_err(|_| anyhow!("agent registry lock poisoned"))?
            .values()
            .cloned()
            .collect();
        snapshots.sort_by_key(|state| state.id);
        Ok(snapshots)
    }

    pub fn record_decision(&self, id: AgentId, decision: AgentDecision) -> Result<()> {
        let mut states = self
            .states
            .write()
            .map_err(|_| anyhow!("agent registry lock poisoned"))?;
        states
            .get_mut(&id)
            .ok_or_else(|| anyhow!("unknown agent: {id}"))?
            .decisions
            .push(decision);
        Ok(())
    }

    /// Coworker relationships are symmetric and cannot refer to missing agents.
    pub fn connect(&self, first: AgentId, second: AgentId) -> Result<()> {
        let mut states = self
            .states
            .write()
            .map_err(|_| anyhow!("agent registry lock poisoned"))?;
        if first == second {
            bail!("an agent cannot be its own coworker");
        }
        if !states.contains_key(&first) || !states.contains_key(&second) {
            bail!("unknown coworker agent");
        }
        for (id, coworker) in [(first, second), (second, first)] {
            let state = states.get_mut(&id).expect("validated agent");
            if !state.coworkers.contains(&coworker) {
                state.coworkers.push(coworker);
            }
        }
        Ok(())
    }

    pub fn begin_run(
        &self,
        id: AgentId,
        model: impl Into<String>,
        workspace: impl Into<PathBuf>,
    ) -> Result<RunGuard> {
        let mut states = self
            .states
            .write()
            .map_err(|_| anyhow!("agent registry lock poisoned"))?;
        let state = states
            .get_mut(&id)
            .ok_or_else(|| anyhow!("unknown agent: {id}"))?;
        if state.status == AgentStatus::Running {
            bail!("agent is already running: {id}");
        }
        state.model = model.into();
        state.workspace = workspace.into();
        state.status = AgentStatus::Running;
        Ok(RunGuard {
            registry: self.clone(),
            id,
        })
    }

    /// Unregister idle agents and remove stale relationship references.
    pub fn unregister(&self, id: AgentId) -> Result<()> {
        let mut states = self
            .states
            .write()
            .map_err(|_| anyhow!("agent registry lock poisoned"))?;
        if states
            .get(&id)
            .is_some_and(|state| state.status == AgentStatus::Running)
        {
            bail!("cannot unregister a running agent");
        }
        states.remove(&id);
        for state in states.values_mut() {
            state.coworkers.retain(|coworker| *coworker != id);
            if state.parent_agent == Some(id) {
                state.parent_agent = None;
            }
        }
        Ok(())
    }
}

/// Restores idle on success, error, cancellation or unwind.
pub struct RunGuard {
    registry: AgentRegistry,
    id: AgentId,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        if let Ok(mut states) = self.registry.states.write()
            && let Some(state) = states.get_mut(&self.id)
        {
            state.status = AgentStatus::Idle;
        }
    }
}
