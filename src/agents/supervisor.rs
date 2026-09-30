//! Backend-facing owner of Agent Group runtimes.
//!
//! The backend opens a budget window per turn, replaces the group when the
//! session or agent mode changes, hands tools an `AgentGroupHandle`, and
//! reads frontend projections and pending notifications. It never sees
//! scheduler state or member coordinators.

use std::sync::Arc;

use crate::model::AgentTaskItem;

use super::{
    group::{AgentGroupRuntime, AgentLimits, AgentNotification, ChangeListener},
    handle::AgentGroupHandle,
    member::CoordinatorLauncher,
    runtime::AgentRuntimeFactory,
};

#[derive(Clone)]
pub(crate) struct AgentGroupSupervisor {
    active: AgentGroupRuntime,
}

impl AgentGroupSupervisor {
    pub(crate) fn new(factory: AgentRuntimeFactory, limits: AgentLimits) -> Self {
        Self {
            active: AgentGroupRuntime::new(Arc::new(CoordinatorLauncher::new(factory)), limits),
        }
    }

    /// Called on every group change, from member threads included.
    pub(crate) fn set_change_listener(&self, listener: ChangeListener) {
        self.active.set_change_listener(listener);
    }

    /// Stops every member and starts an empty group, so a new session or
    /// agent mode never inherits stale agents.
    pub(crate) fn replace_active_group(&self) {
        self.active.replace_group();
    }

    /// Opens a budget window for a new primary turn. Background agents keep
    /// running; foreground agents from an earlier turn are cancelled.
    pub(crate) fn begin_turn(&self) {
        self.active.begin_turn();
    }

    /// Cancels agents the primary is blocked on; background agents survive.
    pub(crate) fn cancel_foreground(&self) {
        self.active.cancel_foreground();
    }

    pub(crate) fn has_notifications(&self) -> bool {
        self.active.has_notifications()
    }

    pub(crate) fn take_notifications(&self) -> Vec<AgentNotification> {
        self.active.take_notifications()
    }

    pub(crate) fn handle(&self) -> AgentGroupHandle {
        AgentGroupHandle::new(self.active.clone())
    }

    pub(crate) fn task_items(&self) -> Vec<AgentTaskItem> {
        self.active.task_items()
    }
}
