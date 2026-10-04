//! Backend-facing owner of Agent Group runtimes.
//!
//! The backend replaces the group when the
//! session or agent mode changes, hands tools an `AgentGroupHandle`, and
//! reads frontend projections. It never sees
//! scheduler state or member coordinators.

use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::model::{AgentGroupItem, AgentTaskItem};

use super::{
    group::{AgentGroupCheckpoint, AgentGroupRuntime, AgentLimits, ChangeListener},
    handle::AgentGroupHandle,
    member::CoordinatorLauncher,
    runtime::AgentRuntimeFactory,
};

#[derive(Clone)]
pub(crate) struct AgentGroupSupervisor {
    active: AgentGroupRuntime,
    handle: AgentGroupHandle,
}

impl AgentGroupSupervisor {
    pub(crate) fn new(factory: AgentRuntimeFactory, limits: AgentLimits) -> Self {
        let active =
            AgentGroupRuntime::new(Arc::new(CoordinatorLauncher::new(factory.clone())), limits);
        Self {
            handle: AgentGroupHandle::new(active.clone(), factory),
            active,
        }
    }

    /// Called on every group change, from member threads included.
    pub(crate) fn set_change_listener(&self, listener: ChangeListener) {
        self.active.set_change_listener(listener);
    }

    /// Applies group limits to the active group and every later one.
    pub(crate) fn set_limits(&self, limits: AgentLimits) {
        self.active.set_limits(limits);
    }

    /// Stops every member and starts an empty group, so a new session or
    /// agent mode never inherits stale agents.
    pub(crate) fn replace_active_group(&self) {
        self.active.replace_group();
        self.handle.reset_group_context();
    }

    pub(crate) fn restore_checkpoint(&self, checkpoint: AgentGroupCheckpoint) -> Result<()> {
        self.handle.reset_group_context();
        self.active.restore_checkpoint(checkpoint)
    }

    pub(crate) fn cancel_group(&self) {
        self.active.cancel_group();
    }

    pub(crate) fn handle(&self) -> AgentGroupHandle {
        self.handle.clone()
    }

    pub(crate) fn task_items(&self) -> Vec<AgentTaskItem> {
        self.active.task_items()
    }

    pub(crate) fn group_item(&self) -> AgentGroupItem {
        self.active.group_item()
    }

    /// Queues a message the user typed to one member.
    pub(crate) fn message(&self, agent_id: &str, message: String) -> Result<()> {
        self.active.steer(parse_id(agent_id)?, message).map(|_| ())
    }

    /// Removes one member (stopping it first), or every stopped member when
    /// `agent_id` is `None`.
    pub(crate) fn remove(&self, agent_id: Option<&str>) -> Result<()> {
        match agent_id {
            Some(id) => self.active.remove(parse_id(id)?),
            None => {
                self.active.remove_stopped();
                Ok(())
            }
        }
    }

    /// Stops one member, or every member when `agent_id` is `None`.
    pub(crate) fn stop(&self, agent_id: Option<&str>) -> Result<()> {
        match agent_id {
            Some(id) => self.active.stop(parse_id(id)?),
            None => {
                self.active.stop_all();
                Ok(())
            }
        }
    }
}

fn parse_id(value: &str) -> Result<super::AgentId> {
    value
        .parse()
        .map_err(|_| anyhow!("invalid agent id: {value}"))
}
