//! Backend-facing owner of Agent Group runtimes.
//!
//! The backend asks the supervisor to replace the active group when a run or
//! session changes, hands tools an `AgentGroupHandle`, and reads frontend
//! projections. It never sees scheduler state or member coordinators.

use crate::model::AgentTaskItem;

use super::{
    group::{AgentGroupRuntime, AgentLimits},
    handle::AgentGroupHandle,
    runtime::{AgentRuntimeFactory, RunManager},
};

#[derive(Clone)]
pub(crate) struct AgentGroupSupervisor {
    active: AgentGroupRuntime,
}

impl AgentGroupSupervisor {
    pub(crate) fn new(
        factory: AgentRuntimeFactory,
        run_manager: RunManager,
        limits: AgentLimits,
    ) -> Self {
        Self {
            active: AgentGroupRuntime::new(factory, run_manager, limits),
        }
    }

    /// Cancels the active group's workers and starts an empty group, so a new
    /// run or session never inherits stale tasks.
    pub(crate) fn replace_active_group(&self) {
        self.active.replace_group();
    }

    pub(crate) fn handle(&self) -> AgentGroupHandle {
        AgentGroupHandle::new(self.active.clone())
    }

    pub(crate) fn task_items(&self) -> Vec<AgentTaskItem> {
        self.active.task_items()
    }
}
